//! OpenDocument text documents (.odt).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{Align, Attrs, Document, LINE_BREAK, OBJECT, LineRule, Mark, PageSetup, ParaKind, ParaStyle, Paragraph, bullet_char, num_format};

use crate::xml::{Ev, HEADER, Reader, Tag, esc};
use crate::zip::{ZipReader, ZipWriter};
use crate::{Error, is_mono_font, odt_img, odt_layout, parse_hex_color, parse_length_pt};

#[path = "odt_notes.rs"]
mod notes;

const MIME: &str = "application/vnd.oasis.opendocument.text";
const MONO_FONT: &str = "Liberation Mono";

// ---------------------------------------------------------------------------
// Reading

#[path = "odt_styles.rs"]
mod styles;

#[derive(Default, Clone)]
struct StyleDef {
    display: String,
    parent: Option<String>,
    next: Option<String>,
    outline: Option<u8>,
    list_style: Option<String>,
    align: Option<Align>,
    line: Option<(LineRule, f32)>,
    break_before: bool,
    keep_next: bool,
    keep_lines: bool,
    no_widows: bool,
    no_line_numbers: bool,
    /// No space between paragraphs of one style (`style:contextual-spacing`).
    contextual: bool,
    /// margin-left, margin-right, text-indent, margin-top, margin-bottom (pt).
    margins: [Option<f32>; 5],
    background: Option<u32>,
    attrs: Attrs,
    /// Weight, posture and underline set plain (not bold, italic or
    /// underlined over a parent that is).
    plain: [bool; 3],
    /// Some(is_monospace) when the style sets a font.
    mono: Option<bool>,
    /// Defined in office:automatic-styles (direct formatting).
    auto: bool,
    extra: odt_layout::ParaExtra,
}

#[derive(Default)]
struct Ctx {
    para: HashMap<String, StyleDef>,
    text: HashMap<String, StyleDef>,
    /// list style -> per level: (bullet?, list_format)
    lists: HashMap<String, Vec<(bool, u8)>>,
    page_layouts: HashMap<String, PageSetup>,
    master_layout: Option<String>,
    graphics: HashMap<String, odt_img::Graphic>,
    pictures: HashMap<String, Arc<[u8]>>,
    tables: crate::odt_table::TableStyles,
    layout: odt_layout::Styles,
    shapes: crate::odf_shape::Styles,
    /// Ruby styles: (under the text, alignment).
    rubies: HashMap<String, (bool, Option<Arc<str>>)>,
    /// Asian settings of the default paragraph style.
    asian: apex_doc::AsianDefaults,
    /// The document defaults and font of the default paragraph style
    /// (`None`: none, or the ones ApexDesk writes for its documents).
    defaults: Option<(apex_doc::DocDefaults, Option<Arc<str>>)>,
    /// The font of the default paragraph style when it is ours.
    own_font: Option<Arc<str>>,
}

/// The shapes of a `draw:g` (`raw`: what's inside it), sharing a group
/// id; `anchor`: the group's anchor type.
fn read_shape_group(raw: &str, anchor: &str, ctx: &Ctx) -> Vec<apex_doc::DocShape> {
    shape_group(raw, anchor, ctx, 0)
}

/// `read_shape_group` `depth` groups down; groups deeper than 64 are
/// left out, as they would overflow the stack.
fn shape_group(raw: &str, anchor: &str, ctx: &Ctx, depth: u32) -> Vec<apex_doc::DocShape> {
    let mut out = Vec::new();
    if depth > 64 {
        return out;
    }
    let mut r = Reader::new(raw);
    loop {
        let (ct, start) = match r.next() {
            Ev::Start(t) => (t, true),
            Ev::Empty(t) => (t, false),
            Ev::Eof => break,
            _ => continue,
        };
        let anchor = ct.attr("text:anchor-type").map_or(anchor.to_string(), |a| a.into_owned());
        match ct.name {
            "draw:frame" => {
                let inner = if start { r.raw_element() } else { "" };
                let mut sub = Reader::new(inner);
                if crate::odf_shape::frame_is_text_box(inner) {
                    let n = crate::drawingml::read_node(&mut sub, &ct, false);
                    out.extend(crate::odf_shape::read(&n, &ctx.shapes).map(|raw| crate::odf_shape::to_doc(raw, &anchor, &ctx.shapes)));
                } else if let Some(f) = odt_img::read_frame(&mut sub, &ct, start, &ctx.graphics, &ctx.pictures) {
                    out.push(match f {
                        odt_img::Framed::Float(d) => d,
                        odt_img::Framed::Image(img) => {
                            let (w, h) = (img.width, img.height);
                            apex_doc::DocShape::new(apex_doc::Shape::picture(img), w, h)
                        }
                    });
                }
            }
            "draw:g" => {
                let inner = if start { r.raw_element() } else { "" };
                out.extend(shape_group(inner, &anchor, ctx, depth + 1));
            }
            name if crate::odf_shape::is_shape(name) => {
                let n = crate::drawingml::read_node(&mut r, &ct, !start);
                out.extend(crate::odf_shape::read(&n, &ctx.shapes).map(|raw| crate::odf_shape::to_doc(raw, &anchor, &ctx.shapes)));
            }
            _ if start => r.skip(),
            _ => {}
        }
    }
    // Ink written as a line for each look of its strokes.
    if let Some(ink) = crate::odf_ink::merge(&out) {
        return vec![ink];
    }
    let id = apex_doc::fresh_group_id();
    for d in &mut out {
        d.group = Some(id);
    }
    out
}

fn kind_from_name(name: &str) -> Option<ParaKind> {
    let n = name.to_ascii_lowercase().replace("_20_", " ").replace('_', " ");
    if let Some(rest) = n.strip_prefix("heading") {
        if let Ok(l) = rest.trim().parse::<u8>() {
            return Some(ParaKind::Heading(l.clamp(1, 9)));
        }
    }
    Some(match n.as_str() {
        "title" => ParaKind::Title,
        "quotations" | "quote" => ParaKind::Quote,
        "preformatted text" | "source text" | "code" => ParaKind::Code,
        "list bullet" => ParaKind::Bullet,
        "list number" | "numbering 1" => ParaKind::Numbered,
        _ => return None,
    })
}

fn read_text_props(t: &Tag, a: &mut Attrs, mono: &mut Option<bool>) {
    // Language: "zxx" is LibreOffice's "no language" (not checked).
    match (t.attr("fo:language"), t.attr("fo:country")) {
        (Some(l), _) if l == "zxx" => a.no_proof = true,
        (Some(l), c) if !l.is_empty() && l != "none" => {
            let tag = match c.filter(|c| !c.is_empty() && c != "none") {
                Some(c) => format!("{l}-{c}"),
                None => l.into_owned(),
            };
            a.lang = Some(Arc::from(tag.as_str()));
        }
        _ => {}
    }
    if let Some(v) = t.attr("fo:font-weight") {
        a.bold = v == "bold" || v.parse::<u32>().is_ok_and(|w| w >= 600);
    }
    if let Some(v) = t.attr("fo:font-style") {
        a.italic = v == "italic" || v == "oblique";
    }
    if let Some(v) = t.attr("style:text-underline-style") {
        a.underline = v != "none";
    }
    if let Some(v) = t.attr("style:text-line-through-style") {
        a.strike = v != "none";
    }
    if let Some(v) = t.attr("style:text-position") {
        // Full size text moved up or down is a position effect.
        let full = v.split_whitespace().nth(1).and_then(|s| s.trim_end_matches('%').parse::<f32>().ok()).is_some_and(|s| s >= 99.0);
        a.superscript = !full && (v.starts_with("super") || v.trim_start().starts_with(|c: char| c.is_ascii_digit() && c != '0'));
        a.subscript = !full && (v.starts_with("sub") || v.trim_start().starts_with('-'));
    }
    let mut fx = a.fx();
    crate::odf_fx::read(t, &mut fx);
    if fx.double_strike {
        a.strike = false;
    }
    a.set_fx(|f| *f = fx);
    if let Some(pt) = t.attr("fo:font-size").and_then(|v| parse_length_pt(&v)) {
        a.size = Some((pt * 2.0).round() as u16);
    }
    if let Some(c) = t.attr("fo:color").and_then(|v| parse_hex_color(&v)) {
        a.color = Some(c);
    }
    let (shadow, hollow, relief) = (t.attr("fo:text-shadow"), t.attr("style:text-outline"), t.attr("style:font-relief"));
    a.text_fx = crate::text_fx_xml::odf_read(a.text_fx.take(), shadow.as_deref(), hollow.as_deref(), relief.as_deref(), a.color);
    if let Some(c) = t.attr("fo:background-color") {
        a.highlight = if c == "transparent" { None } else { parse_hex_color(&c) };
    }
    if let Some(f) = t.attr("style:font-name").or_else(|| t.attr("fo:font-family")) {
        let f = f.trim_matches('\'').to_string();
        let m = is_mono_font(&f);
        *mono = Some(m);
        a.font = if m { None } else { Some(Arc::from(f.as_str())) };
    }
    let mut x = a.asian();
    crate::odt_asian::read_text(t, &mut x);
    a.set_asian(|b| *b = x);
}

fn read_styles(xml: &str, ctx: &mut Ctx) {
    let mut r = Reader::new(xml);
    let mut cur: Option<(String, bool, StyleDef)> = None;
    let mut cur_list: Option<(String, Vec<(bool, u8)>)> = None;
    let mut cur_layout: Option<(String, PageSetup)> = None;
    let mut in_auto = false;
    // In the default paragraph style; in a ruby style (its name).
    let mut in_default = false;
    let mut cur_ruby: Option<String> = None;
    loop {
        let ev = r.next();
        match ev {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) => {
                let is_start = matches!(ev, Ev::Start(_));
                match t.name {
                    "office:automatic-styles" => in_auto = is_start,
                    "style:default-style" => in_default = is_start && t.attr("style:family").as_deref() == Some("paragraph"),
                    "style:text-properties" | "style:paragraph-properties" if in_default => {
                        if t.name == "style:text-properties" {
                            let mut x = ctx.asian.text.as_deref().cloned().unwrap_or_default();
                            crate::odt_asian::read_text(&t, &mut x);
                            ctx.asian.text = (!x.is_empty()).then(|| Arc::new(x));
                        } else {
                            crate::odt_asian::read_para(&t, &mut ctx.asian.para);
                        }
                    }
                    "style:ruby-properties" => {
                        if let Some(n) = cur_ruby.take() {
                            ctx.rubies.insert(n, crate::odt_asian::read_ruby_style(&t));
                        }
                    }
                    "style:style" => {
                        if ctx.layout.start_style(&t, is_start) || ctx.tables.start(&t, is_start) {
                            continue;
                        }
                        let family = t.attr("style:family").unwrap_or_default();
                        if family == "ruby" {
                            cur_ruby = Some(t.attr("style:name").unwrap_or_default().into_owned());
                            continue;
                        }
                        if family != "paragraph" && family != "text" {
                            continue;
                        }
                        let name = t.attr("style:name").unwrap_or_default().into_owned();
                        let mut def = StyleDef {
                            display: t.attr("style:display-name").map(|v| v.into_owned()).unwrap_or_else(|| name.clone()),
                            parent: t.attr("style:parent-style-name").map(|v| v.into_owned()),
                            next: t.attr("style:next-style-name").map(|v| v.into_owned()),
                            outline: t.attr("style:default-outline-level").and_then(|v| v.parse().ok()),
                            list_style: t.attr("style:list-style-name").map(|v| v.into_owned()),
                            auto: in_auto,
                            ..Default::default()
                        };
                        odt_layout::style_attrs(&t, &mut def.extra);
                        let para = family == "paragraph";
                        if is_start {
                            cur = Some((name, para, def));
                        } else if para {
                            ctx.para.insert(name, def);
                        } else {
                            ctx.text.insert(name, def);
                        }
                    }
                    "style:paragraph-properties" => {
                        if let Some((_, _, s)) = &mut cur {
                            if let Some(a) = t.attr("fo:text-align") {
                                s.align = Some(match a.as_ref() {
                                    "center" => Align::Center,
                                    "end" | "right" => Align::Right,
                                    "justify" => Align::Justify,
                                    _ => Align::Left,
                                });
                            }
                            if let Some(lh) = t.attr("fo:line-height") {
                                if let Some(p) = lh.strip_suffix('%').and_then(|p| p.parse::<f32>().ok()) {
                                    s.line = Some((LineRule::Multiple, p / 100.0));
                                } else if let Some(v) = parse_length_pt(&lh) {
                                    s.line = Some((LineRule::Exact, round1(v)));
                                }
                            }
                            if let Some(v) = t.attr("style:line-height-at-least").and_then(|v| parse_length_pt(&v)) {
                                s.line = Some((LineRule::AtLeast, round1(v)));
                            }
                            if t.attr("fo:break-before").as_deref() == Some("page") {
                                s.break_before = true;
                            }
                            if t.attr("fo:keep-with-next").as_deref() == Some("always") {
                                s.keep_next = true;
                            }
                            if t.attr("fo:keep-together").as_deref() == Some("always") {
                                s.keep_lines = true;
                            }
                            if t.attr("fo:widows").as_deref() == Some("0") {
                                s.no_widows = true;
                            }
                            if ["style:contextual-spacing", "loext:contextual-spacing"].iter().any(|k| t.attr(k).as_deref() == Some("true")) {
                                s.contextual = true;
                            }
                            if t.attr("text:number-lines").as_deref() == Some("false") {
                                s.no_line_numbers = true;
                            }
                            for (i, k) in ["fo:margin-left", "fo:margin-right", "fo:text-indent", "fo:margin-top", "fo:margin-bottom"].iter().enumerate() {
                                if let Some(v) = t.attr(k).and_then(|v| parse_length_pt(&v)) {
                                    s.margins[i] = Some(round1(v));
                                }
                            }
                            if let Some(c) = t.attr("fo:background-color") {
                                s.background = parse_hex_color(&c).filter(|&c| c != 0xFFFFFF);
                            }
                            odt_layout::para_props(&t, &mut s.extra);
                        }
                    }
                    "style:tab-stops" | "style:tab-stop" | "style:drop-cap" => {
                        if let Some((_, true, s)) = &mut cur {
                            odt_layout::para_child(&t, &mut s.extra);
                        }
                    }
                    "style:columns" | "style:column" | "style:column-sep" => ctx.layout.columns(&t),
                    "style:table-properties" | "style:table-column-properties" | "style:table-row-properties" | "style:table-cell-properties" => ctx.tables.props(&t),
                    "style:text-properties" => {
                        if let Some((_, _, s)) = &mut cur {
                            read_text_props(&t, &mut s.attrs, &mut s.mono);
                            let a = &s.attrs;
                            let set = |k: &str| t.attr(k).is_some();
                            s.plain = [set("fo:font-weight") && !a.bold, set("fo:font-style") && !a.italic, set("style:text-underline-style") && !a.underline];
                        }
                    }
                    "text:list-style" => {
                        let name = t.attr("style:name").unwrap_or_default().into_owned();
                        cur_list = Some((name, vec![(true, 0); 10]));
                    }
                    "text:list-level-style-number" | "text:list-level-style-bullet" | "text:list-level-style-image" => {
                        if let Some((_, lv)) = &mut cur_list {
                            let l: usize = t.attr("text:level").and_then(|v| v.parse().ok()).unwrap_or(1);
                            if (1..=10).contains(&l) {
                                let lvl = (l - 1) as u8;
                                let fmt = t.attr("style:num-format").unwrap_or_default();
                                lv[l - 1] = if t.name != "text:list-level-style-number" || fmt.is_empty() {
                                    let c = t.attr("text:bullet-char").and_then(|c| c.chars().next()).unwrap_or('•');
                                    let f = if c == bullet_char(0, lvl) { 0 } else { (1..apex_doc::BULLETS.len() as u8).find(|&f| bullet_char(f, lvl) == c).unwrap_or(0) };
                                    (true, f)
                                } else {
                                    let paren = t.attr("style:num-suffix").is_some_and(|s| s.contains(')'));
                                    let f = match fmt.as_ref() {
                                        "a" => 3,
                                        "A" => 4,
                                        "i" => 5,
                                        "I" => 6,
                                        _ if paren => 2,
                                        _ => 1,
                                    };
                                    (false, if f == num_format(0, lvl) { 0 } else { f })
                                };
                            }
                        }
                    }
                    "style:page-layout" => {
                        ctx.layout.start_layout(&t, is_start);
                        let name = t.attr("style:name").unwrap_or_default().into_owned();
                        let mirror = t.attr("style:page-usage").as_deref() == Some("mirrored");
                        cur_layout = Some((name, PageSetup { mirror, ..PageSetup::default() }));
                    }
                    "style:page-layout-properties" => {
                        ctx.layout.layout_props(&t);
                        if let Some((_, p)) = &mut cur_layout {
                            let g = |k: &str| t.attr(k).and_then(|v| parse_length_pt(&v));
                            p.width = g("fo:page-width").unwrap_or(p.width);
                            p.height = g("fo:page-height").unwrap_or(p.height);
                            if let Some(m) = g("fo:margin") {
                                p.margin_top = m;
                                p.margin_bottom = m;
                                p.margin_left = m;
                                p.margin_right = m;
                            }
                            p.margin_top = g("fo:margin-top").unwrap_or(p.margin_top);
                            p.margin_bottom = g("fo:margin-bottom").unwrap_or(p.margin_bottom);
                            p.margin_left = g("fo:margin-left").unwrap_or(p.margin_left);
                            p.margin_right = g("fo:margin-right").unwrap_or(p.margin_right);
                            p.gutter = g("loext:margin-gutter").unwrap_or(p.gutter);
                            crate::odt_asian::read_page(&t, p);
                        }
                    }
                    "style:master-page" => {
                        if ctx.master_layout.is_none() || t.attr("style:name").as_deref() == Some("Standard") {
                            ctx.master_layout = t.attr("style:page-layout-name").map(|v| v.into_owned());
                        }
                    }
                    _ => {}
                }
            }
            Ev::End(n) => match n {
                "office:automatic-styles" => in_auto = false,
                "style:default-style" => in_default = false,
                "style:style" => {
                    ctx.tables.end();
                    ctx.layout.end();
                    if let Some((name, para, def)) = cur.take() {
                        if para {
                            ctx.para.insert(name, def);
                        } else {
                            ctx.text.insert(name, def);
                        }
                    }
                }
                "text:list-style" => {
                    if let Some((name, lv)) = cur_list.take() {
                        ctx.lists.insert(name, lv);
                    }
                }
                "style:page-layout" => {
                    ctx.layout.end();
                    if let Some((name, p)) = cur_layout.take() {
                        ctx.page_layouts.insert(name, p);
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
}

/// Merges `child` attrs over `base` (only non-default fields override).
fn overlay(base: &mut Attrs, child: &Attrs) {
    let d = Attrs::default();
    macro_rules! take {
        ($f:ident) => {
            if child.$f != d.$f {
                base.$f = child.$f.clone();
            }
        };
    }
    take!(bold);
    take!(italic);
    take!(underline);
    take!(strike);
    take!(code);
    take!(superscript);
    take!(subscript);
    take!(size);
    take!(color);
    take!(highlight);
    take!(font);
    take!(link);
    take!(link_tip);
    take!(lang);
    take!(no_proof);
    take!(text_fx);
    if let Some(x) = &child.asian {
        let mut m = base.asian();
        m.overlay(x);
        base.set_asian(|b| *b = m);
    }
    if let Some(fx) = &child.fx {
        let mut f = base.fx();
        f.overlay(fx);
        base.set_fx(|b| *b = f);
    }
}

fn round1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

struct Resolved {
    kind: Option<ParaKind>,
    /// Direct paragraph formatting (from the automatic style).
    direct: Option<StyleDef>,
    align: Align,
    attrs: Attrs,
    mono: bool,
    list_style: Option<String>,
}

impl Ctx {
    fn chain<'a>(map: &'a HashMap<String, StyleDef>, name: &str) -> Vec<&'a StyleDef> {
        let mut out = Vec::new();
        let mut cur = Some(name);
        while let Some(n) = cur {
            if out.len() > 12 {
                break;
            }
            let Some(s) = map.get(n) else { break };
            out.push(s);
            cur = s.parent.as_deref();
        }
        out
    }

    fn resolve_para(&self, name: Option<&str>) -> Resolved {
        let chain = name.map(|n| Self::chain(&self.para, n)).unwrap_or_default();
        let mut res = Resolved { kind: None, direct: None, align: Align::Left, attrs: Attrs::default(), mono: false, list_style: None };
        // The nearest font setting decides whether the paragraph is monospace.
        let mono = chain.iter().find_map(|s| s.mono.or_else(|| (kind_from_name(&s.display) == Some(ParaKind::Code)).then_some(true)));
        res.mono = mono == Some(true);
        // Kind: first named style in the chain that maps to a kind.
        for s in &chain {
            if let Some(k) = kind_from_name(&s.display).or_else(|| s.outline.map(|o| ParaKind::Heading(o.clamp(1, 9)))) {
                res.kind = Some(k);
                break;
            }
        }
        // Automatic styles carry direct formatting; named styles only give
        // the kind (their look comes from our own style defaults).
        if res.kind == Some(ParaKind::Code) && !res.mono {
            res.kind = None;
        }
        for s in &chain {
            if res.list_style.is_none() {
                res.list_style = s.list_style.clone();
            }
        }
        if let Some(s) = chain.first().filter(|s| s.auto) {
            res.align = s.align.unwrap_or(Align::Left);
            res.direct = Some((*s).clone());
            overlay(&mut res.attrs, &s.attrs);
        } else if let Some(a) = chain.iter().find_map(|s| s.align) {
            res.align = a;
        }
        res
    }

    fn text_attrs(&self, name: &str) -> (Attrs, bool) {
        let mut a = Attrs::default();
        let mut mono = false;
        for s in Self::chain(&self.text, name).iter().rev() {
            overlay(&mut a, &s.attrs);
            let n = s.display.to_ascii_lowercase();
            if n.contains("source") || n.contains("code") || n.contains("teletype") {
                mono = true;
            }
            if let Some(m) = s.mono {
                mono = m;
            }
        }
        (a, mono)
    }
}

struct BodyReader<'a> {
    ctx: &'a Ctx,
    out: Vec<Paragraph>,
    /// Spaces runs (`text:s`) may still make in all.
    spaces_left: usize,
    /// Paragraphs the body and its notes may still take, together.
    paras_left: usize,
    /// Open lists: style name and whether the list's first item restarts
    /// the numbering.
    lists: Vec<(Option<String>, bool)>,
    next_table: u32,
    /// Bytes repeated table cells may still copy.
    copy_left: usize,
    /// Tables open around the text being read; past `MAX_NESTING` a
    /// table's text is read as the cell's (and not recursively).
    table_depth: usize,
    /// Named paragraph styles, kept in `ParaStyle::custom` until the end.
    used: styles::Used,
    /// Inside a table of contents: its settings.
    toc: Option<apex_doc::TocOpts>,
    notes: crate::odt_comments::Reading,
    track: crate::odt_revisions::Reading,
    /// `text:track-changes` of the tracked changes.
    tracking: bool,
    /// Style names of the open text sections.
    sects: Vec<String>,
    /// Text sections opened outside tables so far.
    sects_opened: u32,
    marks: Vec<odt_layout::ParaMark>,
    /// Page-anchored shapes and pictures, for the next paragraph.
    shapes: Vec<apex_doc::DocShape>,
    pictures: Vec<apex_doc::Image>,
    /// Reading a header or footer.
    header: bool,
    /// Sources of the citations read.
    biblio: Vec<apex_doc::Source>,
    /// Content controls read so far (their ids).
    controls: u32,
    /// Phonetic guides read (tells adjacent ones apart), and the open one:
    /// where its base text starts and its style name.
    ruby_seq: u32,
    ruby: Option<(usize, Option<String>)>,
}

impl<'a> BodyReader<'a> {
    fn new(ctx: &'a Ctx) -> Self {
        BodyReader { ctx, out: Vec::new(), lists: Vec::new(), next_table: 0, copy_left: 64 << 20, spaces_left: 64 << 20, paras_left: apex_doc::MAX_PARAGRAPHS + 1, table_depth: 0, used: Default::default(), toc: None, notes: Default::default(), track: Default::default(), tracking: false, sects: Vec::new(), sects_opened: 0, marks: Vec::new(), shapes: Vec::new(), pictures: Vec::new(), header: false, biblio: Vec::new(), controls: 0, ruby_seq: 0, ruby: None }
    }

    fn read_para(&mut self, r: &mut Reader, t: &Tag, empty: bool) {
        let name = t.attr("text:style-name");
        let res = self.ctx.resolve_para(name.as_deref());
        let mut style = ParaStyle { align: res.align, ..Default::default() };
        let d = res.direct.clone().unwrap_or_default();
        // (Single spacing too: it is the default only where the style and
        // the defaults are single, see `read`.)
        if let Some((rule, v)) = d.line {
            style.line_rule = rule;
            style.line_spacing = if rule == LineRule::Multiple { (v * 100.0).round() / 100.0 } else { v };
        }
        style.page_break_before = d.break_before;
        style.keep_with_next = d.keep_next;
        style.keep_lines = d.keep_lines;
        style.no_widow_control = d.no_widows;
        style.no_line_numbers = d.no_line_numbers;
        style.contextual_spacing = d.contextual;
        style.shading = d.background;
        style.space_before = d.margins[3];
        style.space_after = d.margins[4];
        style.indent_right = d.margins[1].unwrap_or(0.0);
        let mut kind = res.kind.unwrap_or(ParaKind::Normal);
        if t.name == "text:h" {
            let lvl: u8 = t.attr("text:outline-level").and_then(|v| v.parse().ok()).unwrap_or(1);
            if kind != ParaKind::Title {
                kind = ParaKind::Heading(lvl.clamp(1, 9));
            }
        }
        if res.mono && kind == ParaKind::Normal {
            kind = ParaKind::Code;
        }
        if !self.lists.is_empty() && !matches!(kind, ParaKind::Heading(_) | ParaKind::Title) {
            let level = self.lists.len() - 1;
            let list_name = self.lists.iter().rev().find_map(|l| l.0.clone()).or(res.list_style.clone());
            let (bullet, format) = list_name.and_then(|n| self.ctx.lists.get(&n).and_then(|lv| lv.get(level).copied())).unwrap_or((true, 0));
            kind = if bullet { ParaKind::Bullet } else { ParaKind::Numbered };
            style.level = level.min(8) as u8;
            style.list_format = format;
            let fresh = std::mem::replace(&mut self.lists[level].1, false);
            if fresh && !bullet && crate::list_continues(&self.out, style.level) {
                style.restart = true;
            }
            let lvl = style.level as f32;
            if let Some(l) = d.margins[0] {
                style.indent_left = round1(l - 36.0 * (lvl + 1.0));
            }
            if let Some(f) = d.margins[2] {
                style.first_line = round1(f + 18.0);
            }
        } else {
            if let Some(l) = d.margins[0].filter(|&l| l > 0.0) {
                style.level = ((l + 0.05) / 36.0).floor().clamp(0.0, 8.0) as u8;
                style.indent_left = round1(l - 36.0 * style.level as f32);
            }
            style.first_line = d.margins[2].unwrap_or(0.0);
        }
        style.kind = kind;
        if let Some(n) = name.as_deref().and_then(|n| self.ctx.named_para(n)) {
            style.custom = self.used.number(&n);
        }
        let indent = d.margins[0].unwrap_or(if kind.is_list() { 36.0 * (style.level as f32 + 1.0) } else { 0.0 });
        odt_layout::apply_para(&mut style, &d.extra, indent);
        let mut p = Paragraph::new("");
        for sh in self.shapes.drain(..) {
            p.push(&OBJECT.to_string(), Attrs { shape: Some(Arc::new(sh)), ..Default::default() });
        }
        for img in self.pictures.drain(..) {
            p.push(&OBJECT.to_string(), Attrs { image: Some(Arc::new(img)), ..Default::default() });
        }
        let mut base = res.attrs.clone();
        if kind == ParaKind::Code {
            base.code = false;
        }
        if !empty {
            let mut stack: Vec<Attrs> = vec![base];
            let mut depth = 1;
            // A form text field being read: it and where its text starts.
            let mut form: Option<(apex_doc::ContentControl, usize)> = None;
            loop {
                let ev = r.next();
                match ev {
                    Ev::Eof => break,
                    Ev::Text(x) => {
                        // XML whitespace inside paragraphs collapses to single spaces.
                        let mut s = String::with_capacity(x.len());
                        let mut prev_space = p.text.ends_with(' ') || p.text.is_empty() && s.is_empty();
                        for ch in x.chars() {
                            // XML white space only (not no-break or en spaces).
                            if matches!(ch, ' ' | '\t' | '\n' | '\r') {
                                if !prev_space {
                                    s.push(' ');
                                }
                                prev_space = true;
                            } else {
                                s.push(ch);
                                prev_space = false;
                            }
                        }
                        p.push(&s, stack.last().unwrap().clone());
                    }
                    Ev::Start(ct) | Ev::Empty(ct) => {
                        let is_start = matches!(ev, Ev::Start(_));
                        // Each open element keeps its attributes: deeper ones are left out.
                        if is_start && depth > 256 {
                            r.skip();
                            continue;
                        }
                        let cur = stack.last().unwrap().clone();
                        match ct.name {
                            "text:s" => {
                                let n: usize = ct.attr("text:c").and_then(|v| v.parse().ok()).unwrap_or(1);
                                // Runs of spaces stop growing a paragraph
                                // past a megabyte: 20 bytes make 1000.
                                if p.text.len() < 1 << 20 && self.spaces_left > 0 {
                                    let n = n.min(1000).min(self.spaces_left);
                                    self.spaces_left -= n;
                                    p.push(&" ".repeat(n), cur);
                                }
                            }
                            "text:tab" => p.push("\t", cur),
                            "text:line-break" => p.push(&LINE_BREAK.to_string(), cur),
                            "text:span" | "text:a" => {
                                let mut a = cur;
                                if let Some(sn) = ct.attr("text:style-name").filter(|_| ct.name == "text:span") {
                                    if let Some(named) = self.ctx.named_text(&sn) {
                                        overlay(&mut a, &self.ctx.auto_text_attrs(&sn));
                                        a.char_style = Some(Arc::from(named.as_str()));
                                    } else {
                                        let (ta, mono) = self.ctx.text_attrs(&sn);
                                        overlay(&mut a, &ta);
                                        if mono && kind != ParaKind::Code {
                                            a.code = true;
                                        }
                                    }
                                }
                                if ct.name == "text:a" {
                                    a.link = ct.attr("xlink:href").map(|h| Arc::from(h.as_ref()));
                                    a.link_tip = ct.attr("office:title").filter(|t| !t.is_empty()).map(|t| Arc::from(t.as_ref()));
                                }
                                if is_start {
                                    stack.push(a);
                                    depth += 1;
                                }
                            }
                            "text:bookmark" | "text:bookmark-start" | "text:bookmark-end" => {
                                if let Some(n) = ct.attr("text:name").filter(|n| !n.is_empty()) {
                                    let name: Arc<str> = Arc::from(n.as_ref());
                                    let offset = p.text.len();
                                    if ct.name != "text:bookmark-end" {
                                        p.marks.push(Mark { name: name.clone(), offset, end: false });
                                    }
                                    if ct.name != "text:bookmark-start" {
                                        p.marks.push(Mark { name, offset, end: true });
                                    }
                                }
                                if is_start {
                                    r.skip();
                                }
                            }
                            // Watermarks are the page's, not the header's.
                            "draw:frame" | "draw:custom-shape" if self.header && ct.attr("draw:name").is_some_and(|n| n.to_ascii_lowercase().contains("watermark")) => {
                                if is_start {
                                    r.skip();
                                }
                            }
                            "draw:frame" => {
                                let math = crate::mathml::read_frame(r, is_start, &self.ctx.pictures);
                                let raw = if is_start && math.is_none() { r.raw_element() } else { "" };
                                let mut sub = Reader::new(raw);
                                if let Some((after, eq)) = math {
                                    *r = after;
                                    p.push(&OBJECT.to_string(), Attrs { math: Some(eq), image: None, link: None, link_tip: None, ..cur });
                                } else if crate::odf_shape::frame_is_text_box(raw) {
                                    let n = crate::drawingml::read_node(&mut sub, &ct, false);
                                    if let Some(sh) = crate::odf_shape::read_doc(&n, &self.ctx.shapes) {
                                        p.push(&OBJECT.to_string(), Attrs { shape: Some(Arc::new(sh)), image: None, link: None, link_tip: None, ..cur });
                                    }
                                } else if let Some(f) = odt_img::read_frame(&mut sub, &ct, is_start, &self.ctx.graphics, &self.ctx.pictures) {
                                    let (image, shape) = match f {
                                        odt_img::Framed::Image(img) => (Some(Arc::new(img)), None),
                                        odt_img::Framed::Float(d) => (None, Some(Arc::new(d))),
                                    };
                                    p.push(&OBJECT.to_string(), Attrs { image, shape, link: None, link_tip: None, ..cur });
                                }
                            }
                            name if crate::odf_shape::is_shape(name) => {
                                let n = crate::drawingml::read_node(r, &ct, !is_start);
                                if let Some(sh) = crate::odf_shape::read_doc(&n, &self.ctx.shapes) {
                                    p.push(&OBJECT.to_string(), Attrs { shape: Some(Arc::new(sh)), image: None, link: None, link_tip: None, ..cur });
                                }
                            }
                            "draw:g" if is_start => {
                                let anchor = ct.attr("text:anchor-type").map_or("paragraph".to_string(), |a| a.into_owned());
                                for sh in read_shape_group(r.raw_element(), &anchor, self.ctx) {
                                    p.push(&OBJECT.to_string(), Attrs { shape: Some(Arc::new(sh)), image: None, link: None, link_tip: None, ..cur.clone() });
                                }
                            }
                            "text:note" if is_start => {
                                let n = self.read_note(r, &ct);
                                p.push(&OBJECT.to_string(), Attrs { note: Some(Arc::new(n)), image: None, field: None, superscript: false, ..cur });
                            }
                            "text:page-number" | "text:page-count" | "text:date" | "text:time" | "text:author-name" | "text:initial-creator" | "text:creator" | "text:title" | "text:file-name" | "text:subject" | "text:keywords" | "text:description" | "text:user-defined" | "text:sequence" | "text:bookmark-ref" | "text:expression" | "text:table-formula" | "text:alphabetical-index-mark" => {
                                match notes::read_field(r, &ct, is_start) {
                                    Some(f) => p.push(&OBJECT.to_string(), Attrs { field: Some(Arc::new(f)), image: None, note: None, ..cur }),
                                    None if is_start => r.skip(),
                                    None => {}
                                }
                            }
                            "text:bibliography-mark" => {
                                let src = notes::read_biblio_mark(&ct);
                                let cached = if is_start { r.text() } else { String::new() };
                                if let Some(src) = src {
                                    let f = apex_doc::Field { cached, ..apex_doc::Field::new(apex_doc::FieldKind::Citation(src.tag.clone())) };
                                    if !self.biblio.iter().any(|b| b.tag.eq_ignore_ascii_case(&src.tag)) {
                                        self.biblio.push(src);
                                    }
                                    p.push(&OBJECT.to_string(), Attrs { field: Some(Arc::new(f)), image: None, note: None, ..cur });
                                }
                            }
                            "field:fieldmark-start" | "field:fieldmark" => {
                                let params = if is_start { crate::odt_forms::read_params(r) } else { Vec::new() };
                                self.controls += 1;
                                match crate::odt_forms::control(&ct, &params, self.controls) {
                                    Some((c, Some(text))) => p.push(&text, Attrs { control: Some(Arc::new(c)), ..cur }),
                                    Some((c, None)) if ct.name == "field:fieldmark-start" && form.is_none() && cur.control.is_none() => form = Some((c, p.text.len())),
                                    _ => {}
                                }
                            }
                            "field:fieldmark-separator" | "field:fieldmark-end" => {
                                if is_start {
                                    r.skip();
                                }
                                if ct.name == "field:fieldmark-separator" {
                                    if let Some(f) = &mut form {
                                        f.1 = p.text.len();
                                    }
                                } else if let Some((c, start)) = form.take() {
                                    let blank = crate::docx_forms::is_blank(&p.text[start..]);
                                    let c = Arc::new(apex_doc::ContentControl { placeholder: blank, ..c });
                                    if start == p.text.len() {
                                        p.push(apex_doc::FORM_BLANK, Attrs { control: Some(c), ..cur });
                                    } else {
                                        p.map_attrs(start, p.text.len(), &|a| a.control = Some(c.clone()));
                                    }
                                }
                            }
                            "loext:content-control" => {
                                self.controls += 1;
                                let mut a = cur;
                                if a.control.is_none() {
                                    a.control = Some(Arc::new(crate::odt_controls::read(&ct, self.controls)));
                                }
                                if is_start {
                                    stack.push(a);
                                    depth += 1;
                                }
                            }
                            "loext:list-item" => {
                                if let Some(c) = stack.last_mut().and_then(|a| a.control.as_mut()) {
                                    crate::odt_controls::list_item(&ct, Arc::make_mut(c));
                                }
                                if is_start {
                                    r.skip();
                                }
                            }
                            "office:annotation" | "office:annotation-end" => self.notes.element(r, &ct, is_start, &mut p),
                            "text:change-start" | "text:change-end" | "text:change" => {
                                self.track.element(&ct, p.text.len());
                                if is_start {
                                    r.skip();
                                }
                            }
                            "text:note" | "text:tracked-changes" => {
                                if is_start {
                                    r.skip();
                                }
                            }
                            // A phonetic guide: its base is text; the guide
                            // (after it) goes on the base's characters.
                            "text:ruby" => {
                                self.ruby = Some((p.text.len(), ct.attr("text:style-name").map(|n| n.into_owned())));
                                if is_start {
                                    stack.push(cur);
                                    depth += 1;
                                }
                            }
                            "text:ruby-text" => {
                                let guide = if is_start { r.text() } else { String::new() };
                                if let Some((start, sn)) = self.ruby.take().filter(|(s, _)| *s < p.text.len() && p.text.is_char_boundary(*s)) {
                                    let (below, align) = sn.and_then(|n| self.ctx.rubies.get(&n).cloned()).unwrap_or_default();
                                    self.ruby_seq += 1;
                                    // The guide's size, from its text style.
                                    let size = ct.attr("text:style-name").and_then(|n| self.ctx.text.get(n.as_ref())).and_then(|s| s.attrs.asian.as_ref().and_then(|a| a.size).or(s.attrs.size));
                                    let ruby = Arc::new(apex_doc::Ruby { text: Arc::from(guide.as_str()), align, below, size, seq: self.ruby_seq, ..Default::default() });
                                    p.map_attrs(start, p.text.len(), &|a| a.set_asian(|x| x.ruby = Some(ruby.clone())));
                                }
                            }
                            _ => {
                                if is_start {
                                    // Unknown inline container (fields etc.): keep its text.
                                    stack.push(cur);
                                    depth += 1;
                                }
                            }
                        }
                    }
                    Ev::End(_) => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                        stack.pop();
                    }
                }
            }
        }
        self.toc_mark(&mut style, &p, name.as_deref());
        p.style = style;
        if p.runs.is_empty() {
            p.runs.push(apex_doc::Run { len: 0, attrs: Attrs::default() });
        }
        let section = self.sects.iter().rev().find(|s| self.ctx.layout.has_columns(s)).cloned();
        for (k, p) in self.track.finish(p).into_iter().enumerate() {
            // Past what a document keeps (`capped` says some were left
            // out): a small file of empty paragraphs would take gigabytes.
            if self.out.len() > apex_doc::MAX_PARAGRAPHS || self.paras_left == 0 {
                break;
            }
            self.paras_left -= 1;
            self.out.push(p);
            self.marks.push(odt_layout::ParaMark { para: self.out.len() - 1, section: section.clone(), sects_opened: self.sects_opened, master: if k == 0 { d.extra.master.clone() } else { None }, page_number: if k == 0 { d.extra.page_number } else { None } });
        }
    }

    fn read_table(&mut self, r: &mut Reader, t: &Tag) {
        self.next_table += 1;
        let styles = &self.ctx.tables;
        let mut tbl = crate::odt_table::TableRead::new(self.next_table, self.out.len(), t, styles);
        let mut depth = 1;
        loop {
            let ev = r.next();
            match ev {
                Ev::Eof => break,
                Ev::Start(t) | Ev::Empty(t) => {
                    let is_start = matches!(ev, Ev::Start(_));
                    match t.name {
                        "table:table-column" => tbl.column(&t, styles),
                        "table:table-header-rows" => tbl.header(is_start),
                        "table:table-row" => tbl.start_row(&t, styles),
                        "table:table-cell" | "table:covered-table-cell" => {
                            // Cells make paragraphs of their own (empty and
                            // covered ones): they count against the cap too.
                            if self.paras_left == 0 {
                                if is_start {
                                    r.skip();
                                }
                                continue;
                            }
                            let start = self.out.len();
                            if is_start {
                                // The cell's content, up to its end tag.
                                let lists = std::mem::take(&mut self.lists);
                                self.read_body(r);
                                self.lists = lists;
                            }
                            let read = self.out.len();
                            tbl.cell(&t, t.name == "table:covered-table-cell", start, &mut self.out, styles, &mut self.copy_left);
                            self.paras_left = self.paras_left.saturating_sub(self.out.len().saturating_sub(read));
                            continue;
                        }
                        "office:annotation" | "table:table-source" | "office:dde-source" if is_start => {
                            r.skip();
                            continue;
                        }
                        _ => {}
                    }
                    if is_start {
                        depth += 1;
                    }
                }
                Ev::End(n) => {
                    if n == "table:table-header-rows" {
                        tbl.header(false);
                    }
                    if n == "table:table-row" {
                        let read = self.out.len();
                        tbl.end_row(&mut self.out);
                        self.paras_left = self.paras_left.saturating_sub(self.out.len().saturating_sub(read));
                    }
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
        tbl.finish(&mut self.out);
    }

    fn read_body(&mut self, r: &mut Reader) {
        // Notes and indexes hold bodies of their own, in one another
        // without end in a hostile file: deeper ones are left out. Table
        // cells are bodies too, as deep as tables nest.
        thread_local! {
            static BODIES: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
        }
        if BODIES.get() >= apex_doc::MAX_NESTING as u32 + 16 {
            r.skip();
            return;
        }
        BODIES.set(BODIES.get() + 1);
        struct Out;
        impl Drop for Out {
            fn drop(&mut self) {
                BODIES.set(BODIES.get() - 1);
            }
        }
        let _out = Out;
        let mut depth = 1;
        loop {
            let ev = r.next();
            match ev {
                Ev::Eof => return,
                Ev::Start(t) | Ev::Empty(t) => {
                    let is_start = matches!(ev, Ev::Start(_));
                    match t.name {
                        "text:p" | "text:h" => self.read_para(r, &t, !is_start),
                        "table:table" if is_start && self.table_depth < apex_doc::MAX_NESTING => {
                            self.table_depth += 1;
                            self.read_table(r, &t);
                            self.table_depth -= 1;
                        }
                        "text:table-of-content" | "text:alphabetical-index" | "text:bibliography" if is_start => self.read_toc(r),
                        // Sections and lists nested past any real use: each
                        // paragraph looks over those it is in.
                        "text:section" | "text:list" if is_start && self.sects.len() + self.lists.len() >= 64 => r.skip(),
                        "text:section" if is_start => {
                            self.sects.push(t.attr("text:style-name").unwrap_or_default().into_owned());
                            if self.table_depth == 0 {
                                self.sects_opened += 1;
                            }
                            depth += 1;
                        }
                        "text:list" => {
                            if is_start {
                                let cont = t.attr("text:continue-numbering").as_deref() == Some("true") || t.attr("text:continue-list").is_some();
                                self.lists.push((t.attr("text:style-name").map(|v| v.into_owned()), !cont));
                                depth += 1;
                            }
                        }
                        "text:tracked-changes" if is_start => {
                            self.tracking = t.attr("text:track-changes").as_deref() != Some("false");
                            // Deleted paragraphs are read like the body's.
                            let mut track = std::mem::take(&mut self.track);
                            track.read_regions(r, &mut |r, t, empty| {
                                let n = self.out.len();
                                self.read_para(r, t, empty);
                                self.out.split_off(n)
                            });
                            self.track = track;
                        }
                        "draw:g" if is_start => {
                            let anchor = t.attr("text:anchor-type").map_or("paragraph".to_string(), |a| a.into_owned());
                            self.shapes.extend(read_shape_group(r.raw_element(), &anchor, self.ctx));
                        }
                        "draw:frame" if is_start => {
                            // Shapes anchored to the page go to the next
                            // paragraph: text boxes, pictures and objects.
                            let raw = r.raw_element();
                            if crate::odf_shape::frame_is_text_box(raw) {
                                let n = crate::drawingml::read_node(&mut Reader::new(raw), &t, false);
                                self.shapes.extend(crate::odf_shape::read_doc(&n, &self.ctx.shapes));
                            } else if let Some(f) = odt_img::read_frame(&mut Reader::new(raw), &t, true, &self.ctx.graphics, &self.ctx.pictures) {
                                // Wrapped by the text: a picture of the paragraph, as
                                // those anchored to one are read.
                                match f {
                                    odt_img::Framed::Float(d) => self.shapes.push(d),
                                    odt_img::Framed::Image(img) => self.pictures.push(img),
                                }
                            }
                        }
                        "draw:frame" | "draw:rect" | "draw:ellipse" | "draw:circle" | "draw:line" | "draw:custom-shape" | "draw:path" | "draw:polyline" => {
                            // Shapes anchored to the page go to the next paragraph.
                            let n = crate::drawingml::read_node(r, &t, !is_start);
                            let text_box = n.child("text-box").is_some() && n.child("image").is_none();
                            if text_box || crate::odf_shape::is_shape(t.name) {
                                self.shapes.extend(crate::odf_shape::read_doc(&n, &self.ctx.shapes));
                            }
                        }
                        "office:forms" | "text:sequence-decls" | "text:variable-decls" | "office:annotation" | "text:tracked-changes" => {
                            if is_start {
                                r.skip();
                            }
                        }
                        _ => {
                            if is_start {
                                depth += 1;
                            }
                        }
                    }
                }
                Ev::End(n) => {
                    if n == "text:list" {
                        self.lists.pop();
                    } else if n == "text:section" {
                        self.sects.pop();
                    }
                    depth -= 1;
                    if depth == 0 {
                        return;
                    }
                }
                _ => {}
            }
        }
    }
}

/// Linked text boxes: (frame, next frame) names.
fn text_box_chains(xml: &str) -> Vec<(String, String)> {
    if !xml.contains("draw:chain-next-name") {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut frames: Vec<String> = Vec::new();
    let mut r = Reader::new(xml);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) if t.name == "draw:frame" => frames.push(t.attr("draw:name").unwrap_or_default().into_owned()),
            Ev::End("draw:frame") => {
                frames.pop();
            }
            Ev::Start(t) | Ev::Empty(t) if t.name == "draw:text-box" => {
                if let (Some(next), Some(name)) = (t.attr("draw:chain-next-name"), frames.last()) {
                    out.push((name.clone(), next.trim_start_matches('#').to_string()));
                }
            }
            _ => {}
        }
    }
    out
}

pub fn read(data: &[u8]) -> Result<Document, Error> {
    let z = ZipReader::new(data)?;
    let mut ctx = Ctx::default();
    let mut pages = crate::ods::page::Pages::default();
    ctx.pictures = odt_img::load_pictures(&z);
    let styles_src = z.read_string("styles.xml").ok();
    if let Some(s) = &styles_src {
        read_styles(s, &mut ctx);
        match crate::odt_defaults::read(s, &ctx.asian) {
            Some(Ok(d)) => ctx.defaults = Some(d),
            Some(Err(font)) => ctx.own_font = font,
            None => {}
        }
        odt_img::read_graphics(s, &mut ctx.graphics);
        ctx.shapes.read(s);
        let align = |name: &str| {
            Ctx::chain(&ctx.para, name).iter().find_map(|s| s.align).map(|a| match a {
                Align::Center => 1,
                Align::Right => 2,
                _ => 0,
            })
        };
        pages = crate::ods::page::read_pages(s, &align, 0);
    }
    let content = z.read_string("content.xml")?;
    // Headers and footers, with the automatic styles of styles.xml.
    let mut used = styles::Used::default();
    let mut raw_hf: Vec<(String, &'static str, Vec<Paragraph>)> = Vec::new();
    if let Some(src) = &styles_src {
        let mut r = Reader::new(src);
        let mut master = String::new();
        // Paragraphs all headers and footers may still take.
        let mut hf_paras_left = apex_doc::MAX_PARAGRAPHS + 1;
        // Spaces and copied cells too are counted over all of them.
        let mut hf_left: (usize, usize) = (64 << 20, 64 << 20);
        loop {
            match r.next() {
                Ev::Eof => break,
                Ev::Start(t) if t.name == "style:master-page" => master = t.attr("style:name").unwrap_or_default().into_owned(),
                Ev::Start(t) => {
                    let slot = match t.name {
                        "style:header" => "header",
                        "style:header-left" => "header-left",
                        "style:header-first" => "header-first",
                        "style:footer" => "footer",
                        "style:footer-left" => "footer-left",
                        "style:footer-first" => "footer-first",
                        _ => continue,
                    };
                    if t.attr("style:display").as_deref() == Some("false") {
                        r.skip();
                        continue;
                    }
                    let mut hr = BodyReader::new(&ctx);
                    hr.header = true;
                    hr.used = std::mem::take(&mut used);
                    hr.paras_left = hf_paras_left;
                    (hr.spaces_left, hr.copy_left) = hf_left;
                    hr.read_body(&mut r);
                    used = std::mem::take(&mut hr.used);
                    hf_paras_left = hr.paras_left;
                    hf_left = (hr.spaces_left, hr.copy_left);
                    // A table ends with a paragraph after it (as in Writer and Word).
                    if hr.out.last().is_some_and(|p| p.cell.is_some()) {
                        hr.out.push(Paragraph::new(""));
                    }
                    raw_hf.push((master.clone(), slot, hr.out));
                }
                _ => {}
            }
        }
    }
    read_styles(&content, &mut ctx);
    odt_img::read_graphics(&content, &mut ctx.graphics);
    ctx.shapes.read(&content);
    let mut br = BodyReader::new(&ctx);
    br.used = used;
    let mut r = Reader::new(&content);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) if t.name == "office:text" => br.read_body(&mut r),
            _ => {}
        }
    }
    if !br.shapes.is_empty() || !br.pictures.is_empty() {
        let mut p = Paragraph::new("");
        for sh in br.shapes.drain(..) {
            p.push(&OBJECT.to_string(), Attrs { shape: Some(Arc::new(sh)), ..Default::default() });
        }
        for img in br.pictures.drain(..) {
            p.push(&OBJECT.to_string(), Attrs { image: Some(Arc::new(img)), ..Default::default() });
        }
        br.out.push(p);
    }
    let body_len = br.out.len();
    let lens: Vec<usize> = raw_hf.iter().map(|h| h.2.len()).collect();
    for (_, _, paras) in &mut raw_hf {
        br.out.append(paras);
    }
    let named = styles::import(&ctx, &mut br.out, &br.used);
    let mut rest = br.out.split_off(body_len);
    let mut masters_hf: HashMap<String, apex_doc::Headers> = HashMap::new();
    let mut odd_even = false;
    // Each story's paragraphs, cut from the end (from the front would
    // move the rest each time: slow with thousands of them).
    let mut parts: Vec<Vec<Paragraph>> = lens.iter().rev().map(|&n| rest.split_off(rest.len().saturating_sub(n))).collect();
    parts.reverse();
    for ((master, slot, _), part) in raw_hf.iter().zip(parts) {
        let story = Arc::new(Document::from_paragraphs(part).paragraphs);
        let h = masters_hf.entry(master.clone()).or_default();
        let (header, kind) = match *slot {
            "header" => (true, 0),
            "footer" => (false, 0),
            "header-first" => (true, 1),
            "footer-first" => (false, 1),
            "header-left" => (true, 2),
            _ => (false, 2),
        };
        h.different_first |= kind == 1;
        odd_even |= kind == 2;
        h.set(apex_doc::HfSlot::of(header, kind), Some(story));
    }
    // Pages without their own header or footer show the normal one.
    for h in masters_hf.values_mut() {
        for header in [true, false] {
            let main = h.get(apex_doc::HfSlot::of(header, 0)).cloned().unwrap_or_else(|| Arc::new(vec![Paragraph::new("")]));
            for kind in 0..3 {
                let slot = apex_doc::HfSlot::of(header, kind);
                if h.get(slot).is_none() {
                    h.set(slot, Some(main.clone()));
                }
            }
        }
    }
    let notes = std::mem::take(&mut br.notes);
    let tracking = br.tracking;
    let biblio = std::mem::take(&mut br.biblio);
    let mut doc = Document::from_paragraphs(br.out);
    doc.biblio.sources = biblio;
    doc.book = crate::book_xml::read_odt(&z);
    doc.styles = named;
    doc.track_changes = tracking;
    doc.set_comments(notes.finish());
    if let Some(p) = ctx.master_layout.as_ref().and_then(|n| ctx.page_layouts.get(n)).or_else(|| ctx.page_layouts.iter().min_by_key(|(n, _)| n.as_str()).map(|(_, p)| p)) {
        doc.page = *p;
    }
    // Headers and footers of the same master page as the page layout.
    let master = pages.masters.get("Standard").or_else(|| pages.order.first().and_then(|n| pages.masters.get(n)));
    if let Some(m) = master {
        let l = pages.layout_of(m);
        let p = &mut doc.page;
        (p.margin_top, p.margin_header) = crate::ods::page::edge_margins(l.margins[0], l.header, m.has_header, p.margin_top, p.margin_header);
        (p.margin_bottom, p.margin_footer) = crate::ods::page::edge_margins(l.margins[1], l.footer, m.has_footer, p.margin_bottom, p.margin_footer);
    }
    doc.odd_even = odd_even;
    doc.link_text_boxes_by_name(&text_box_chains(&content));
    doc.tidy_shape_groups();
    odt_layout::apply(&mut doc, &br.marks, &ctx.layout, &pages, &masters_hf, &ctx.shapes);
    doc.styles.asian = ctx.asian.clone();
    if let Some((d, font)) = &ctx.defaults {
        doc.styles.defaults = Some(d.clone());
        doc.default_font = font.clone().filter(|f| !is_mono_font(f));
    } else if ctx.own_font.is_some() {
        doc.default_font = ctx.own_font.clone().filter(|f| !is_mono_font(f));
    } else if !styles_src.as_deref().is_some_and(crate::odt_defaults::has_default_style) {
        // No default style: LibreOffice's font, which is also the one we
        // write for it (so the text keeps its width in docx too).
        doc.default_font = Some(Arc::from("Liberation Serif"));
    }
    // Single spacing where the default is single too is the default.
    for i in 0..doc.paragraphs.len() {
        let st = &doc.paragraphs[i].style;
        if st.line_rule == LineRule::Multiple && (st.line_spacing - 1.0).abs() < 0.005 && (doc.styles.default_line(st) - 1.0).abs() < 0.005 {
            doc.paragraphs[i].style.line_spacing = 0.0;
        }
    }
    if let Some(s) = &styles_src {
        // Fonts of the defaults by their declared names.
        let faces = crate::odt_defaults::font_faces(s);
        if let Some(t) = doc.styles.asian.text.as_mut() {
            let t = Arc::make_mut(t);
            t.font = t.font.as_deref().map(|f| crate::odt_defaults::family(&faces, f));
            t.cs_font = t.cs_font.as_deref().map(|f| crate::odt_defaults::family(&faces, f));
        }
    }
    let base = doc.normal_size_pt();
    crate::odt_asian::fix_char_pitch(&mut doc.page, base);
    for p in &mut doc.paragraphs {
        if let Some(s) = &mut p.section {
            crate::odt_asian::fix_char_pitch(&mut s.page, base);
        }
    }
    if let (Some(m), Some(s)) = (odt_layout::base_master(&br.marks, &pages), &styles_src) {
        doc.deco.watermark = odt_layout::read_watermark(s, m, &ctx.pictures);
    }
    if let Ok(s) = z.read_string("meta.xml") {
        doc.meta = crate::props::read_odf(&s);
    }
    doc.macros.odf = crate::macro_parts::read_odf(&z);
    if let Some(x) = styles_src.as_deref().and_then(crate::odt_defaults::read_page) {
        doc.kept.xml.push((crate::odt_defaults::PAGE_SLOT.into(), x));
    }
    if let Some(x) = z.read_string("settings.xml").ok().and_then(|s| crate::odt_settings::read(&s)) {
        doc.kept.xml.push((crate::odt_settings::SLOT.into(), x));
    }
    doc.fonts.embed = z.read_string("settings.xml").is_ok_and(|s| crate::odt_fonts::embed_setting(&s));
    if let Some(s) = &styles_src {
        crate::odt_fonts::read(&z, s, &mut doc.fonts);
    }
    if let Ok(s) = z.read_string("content.xml") {
        crate::odt_fonts::read(&z, &s, &mut doc.fonts);
    }
    if z.read_string("settings.xml").is_ok_and(|s| crate::odt_forms::protect_form(&s)) {
        doc.protection = Some(apex_doc::Protection { limit: apex_doc::EditLimit::Forms, enforced: true, password: None });
    }
    if let Ok(s) = z.read_string("styles.xml") {
        doc.notes = notes::read_config(&s);
        doc.page.line_numbers = notes::read_line_numbering(&s);
        doc.auto_hyphenation = s.split("</style:default-style>").next().is_some_and(|d| d.contains(r#"style:family="paragraph""#) && d.contains(r#"fo:hyphenate="true""#));
        if s.contains(r#"text:numbered-entries="true""#) {
            doc.biblio.style = apex_doc::CiteStyle::Ieee;
        }
        let named = s.split(r#"apex:cite-style=""#).nth(1).and_then(|v| v.split('"').next());
        if let Some(st) = named.and_then(|n| apex_doc::CiteStyle::ALL.into_iter().find(|c| c.name() == n)) {
            doc.biblio.style = st;
        }
    }
    Ok(doc)
}

// ---------------------------------------------------------------------------
// Writing

fn named_style(kind: ParaKind) -> &'static str {
    match kind {
        ParaKind::Normal => "Standard",
        ParaKind::Heading(1) => "Heading_20_1",
        ParaKind::Heading(2) => "Heading_20_2",
        ParaKind::Heading(3) => "Heading_20_3",
        ParaKind::Heading(4) => "Heading_20_4",
        ParaKind::Heading(5) => "Heading_20_5",
        ParaKind::Heading(_) => "Heading_20_6",
        ParaKind::Title => "Title",
        ParaKind::Quote => "Quotations",
        ParaKind::Code => "Preformatted_20_Text",
        ParaKind::Bullet | ParaKind::Numbered => "List_20_Paragraph",
    }
}

fn list_style_name(bullet: bool, format: u8) -> String {
    match (bullet, format) {
        (true, 0) => "LBullet".into(),
        (false, 0) => "LNumber".into(),
        (true, f) => format!("LBullet{f}"),
        (false, f) => format!("LNumber{f}"),
    }
}

/// Exact length in points.
fn ptx(v: f32) -> String {
    format!("{}pt", (v * 100.0).round() / 100.0)
}

fn pt(v: f32) -> String {
    format!("{:.3}cm", v * 2.54 / 72.0)
}

fn text_props(a: &Attrs) -> String {
    let mut s = String::new();
    if a.no_proof {
        s.push_str(r#" fo:language="zxx" fo:country="none""#);
    } else if let Some((lang, country)) = a.lang.as_deref().and_then(crate::odt_asian::split_lang) {
        let _ = write!(s, r#" fo:language="{}""#, esc(lang));
        if let Some(country) = country {
            let _ = write!(s, r#" fo:country="{}""#, esc(country));
        }
    }
    if a.bold {
        s.push_str(r#" fo:font-weight="bold" style:font-weight-asian="bold" style:font-weight-complex="bold""#);
    }
    if a.italic {
        s.push_str(r#" fo:font-style="italic" style:font-style-asian="italic" style:font-style-complex="italic""#);
    }
    let fx = a.fx();
    if a.underline {
        s.push_str(&crate::odf_fx::underline_attrs(&fx));
    }
    if a.strike || fx.double_strike {
        let _ = write!(s, r#" style:text-line-through-style="solid" style:text-line-through-type="{}""#, if fx.double_strike { "double" } else { "single" });
    }
    s.push_str(&crate::odf_fx::attrs(&fx, a.superscript, a.subscript));
    if a.superscript {
        s.push_str(r#" style:text-position="super 58%""#);
    } else if a.subscript {
        s.push_str(r#" style:text-position="sub 58%""#);
    }
    if let Some(sz) = a.size {
        let _ = write!(s, r#" fo:font-size="{}pt""#, sz as f32 / 2.0);
    }
    if let Some(c) = a.color {
        let _ = write!(s, r##" fo:color="#{:06x}""##, c & 0xFF_FFFF);
    }
    if let Some(fx) = &a.text_fx {
        s.push_str(&crate::text_fx_xml::odf_attrs(fx, a.color.is_some()));
    }
    if let Some(h) = a.highlight {
        let _ = write!(s, r##" fo:background-color="#{:06x}""##, h & 0xFF_FFFF);
    }
    if a.code {
        let _ = write!(s, r#" fo:font-family="'{}'" style:font-family-generic="modern" style:font-pitch="fixed""#, MONO_FONT);
    } else if let Some(f) = &a.font {
        let _ = write!(s, r#" fo:font-family="'{}'""#, esc(&crate::theme_xml::font_name(f)));
    }
    if let Some(x) = &a.asian {
        s.push_str(&crate::odt_asian::text_attrs(x, a.lang.as_deref()));
    }
    s
}

fn styles_xml(doc: &Document, masters: &[odt_layout::Master], master_hf: &[(String, bool, bool)], hauto: &str, wm: bool, page_fill: &str) -> String {
    let font_token = doc.default_font_token();
    // Documents on the theme's body font have headings in its headings font.
    let heading_font = match font_token.as_deref() {
        Some(apex_doc::theme::MINOR_TOKEN) => format!(r#" fo:font-family="'{}'""#, esc(&doc.styles.theme.fonts.major)),
        _ => String::new(),
    };
    let mut s = String::from(HEADER);
    s.push_str(r#"<office:document-styles xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:drawooo="http://openoffice.org/2010/draw" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:loext="urn:org:documentfoundation:names:experimental:office:xmlns:loext:1.0" xmlns:field="urn:openoffice:names:experimental:ooo-ms-interop:xmlns:field:1.0" office:version="1.3"><office:styles>"#);
    if let Some((_, x)) = doc.kept.xml.iter().find(|(slot, _)| slot == crate::odt_defaults::PAGE_SLOT) {
        s.push_str(x);
    }
    s.push_str(&crate::odt_defaults::write(doc));
    // Normal of a document opened from a file is what it set (the rest
    // comes from the file's defaults).
    if !styles::overridden(doc, ParaKind::Normal) {
        let props = if doc.styles.defaults.is_some() { "" } else { r#"<style:paragraph-properties fo:margin-top="0cm" fo:margin-bottom="0.247cm" fo:line-height="115%"/>"# };
        let _ = write!(s, r#"<style:style style:name="Standard" style:family="paragraph" style:class="text">{props}</style:style>"#);
    }
    let _ = write!(s, r#"<style:style style:name="Heading" style:family="paragraph" style:parent-style-name="Standard" style:next-style-name="Standard" style:class="text"><style:paragraph-properties fo:margin-top="0.423cm" fo:margin-bottom="0.212cm" fo:keep-with-next="always"/><style:text-properties fo:font-weight="bold"{heading_font}/></style:style>"#);
    for n in (1..=9u8).filter(|&n| !styles::overridden(doc, ParaKind::Heading(n))) {
        let sz = ParaKind::Heading(n).base_size() as f32 / 2.0;
        let _ = write!(s, r#"<style:style style:name="Heading_20_{n}" style:display-name="Heading {n}" style:family="paragraph" style:parent-style-name="Heading" style:next-style-name="Standard" style:default-outline-level="{n}" style:class="text"><style:text-properties fo:font-size="{sz}pt" fo:font-weight="bold"/></style:style>"#, n = n, sz = sz);
    }
    if !styles::overridden(doc, ParaKind::Title) {
        s.push_str(r#"<style:style style:name="Title" style:family="paragraph" style:parent-style-name="Heading" style:next-style-name="Standard" style:class="chapter"><style:text-properties fo:font-size="26pt" fo:font-weight="bold"/></style:style>"#);
    }
    if !styles::overridden(doc, ParaKind::Quote) {
        s.push_str(r##"<style:style style:name="Quotations" style:family="paragraph" style:parent-style-name="Standard" style:class="html"><style:paragraph-properties fo:margin-left="1cm" fo:margin-right="1cm"/><style:text-properties fo:font-style="italic" fo:color="#555555"/></style:style>"##);
    }
    let _ = write!(s, r##"<style:style style:name="Preformatted_20_Text" style:display-name="Preformatted Text" style:family="paragraph" style:parent-style-name="Standard" style:class="html"><style:paragraph-properties fo:margin-top="0cm" fo:margin-bottom="0cm" fo:line-height="100%" fo:background-color="#f3f3f3"/><style:text-properties fo:font-family="'{}'" style:font-family-generic="modern" style:font-pitch="fixed" fo:font-size="10pt"/></style:style>"##, MONO_FONT);
    s.push_str(r#"<style:style style:name="List_20_Paragraph" style:display-name="List Paragraph" style:family="paragraph" style:parent-style-name="Standard" style:class="list"><style:paragraph-properties fo:margin-bottom="0.071cm"/></style:style>"#);
    s.push_str(r##"<style:style style:name="Internet_20_link" style:display-name="Internet link" style:family="text"><style:text-properties fo:color="#0563c1" style:text-underline-style="solid" style:text-underline-width="auto" style:text-underline-color="font-color"/></style:style>"##);
    s.push_str(&styles::styles_xml(doc));
    // List styles, one per list format in use.
    let mut used: Vec<(bool, u8)> = vec![(true, 0), (false, 0)];
    for p in &doc.paragraphs {
        let k = (p.style.kind == ParaKind::Bullet, p.style.list_format);
        if p.style.kind.is_list() && !used.contains(&k) {
            used.push(k);
        }
    }
    for (bullet, format) in used {
        let _ = write!(s, r#"<text:list-style style:name="{}">"#, list_style_name(bullet, format));
        for l in 1..=10u8 {
            let props = format!(r#"<style:list-level-properties text:list-level-position-and-space-mode="label-alignment"><style:list-level-label-alignment text:label-followed-by="listtab" text:list-tab-stop-position="{0}" fo:text-indent="-0.635cm" fo:margin-left="{0}"/></style:list-level-properties>"#, pt(36.0 * l as f32));
            if bullet {
                let _ = write!(s, r#"<text:list-level-style-bullet text:level="{}" text:bullet-char="{}">{}</text:list-level-style-bullet>"#, l, bullet_char(format, l - 1), props);
            } else {
                let f = num_format(format, l - 1);
                let fmt = ["1", "1", "1", "a", "A", "i", "I"][f as usize];
                let suffix = if matches!(f, 2 | 3) { ")" } else { "." };
                let _ = write!(s, r#"<text:list-level-style-number text:level="{}" style:num-suffix="{}" style:num-format="{}">{}</text:list-level-style-number>"#, l, suffix, fmt, props);
            }
        }
        s.push_str("</text:list-style>");
    }
    s.push_str(&notes::styles(doc));
    if !doc.shapes().is_empty() {
        s.push_str(&crate::odf_shape::common_styles());
    }
    s.push_str("</office:styles><office:automatic-styles>");
    s.push_str(hauto);
    if let Some(w) = doc.deco.watermark.as_ref().filter(|_| wm) {
        s.push_str(&odt_layout::watermark_styles(w));
    }
    for (i, m) in masters.iter().enumerate() {
        let pg = &m.page;
        let (_, hh, fh) = master_hf[i];
        let (top, bottom) = crate::ods::page::page_margins(pg.margin_top, pg.margin_header, pg.margin_bottom, pg.margin_footer, hh, fh);
        let usage = match m.start {
            apex_doc::SectionStart::EvenPage => r#" style:page-usage="left""#,
            apex_doc::SectionStart::OddPage => r#" style:page-usage="right""#,
            _ if pg.mirror => r#" style:page-usage="mirrored""#,
            _ => "",
        };
        let gutter = if pg.gutter > 0.0 { format!(r#" loext:margin-gutter="{}""#, pt(pg.gutter)) } else { String::new() };
        let _ = write!(
            s,
            r#"<style:page-layout style:name="pm{}"{usage}><style:page-layout-properties fo:page-width="{}" fo:page-height="{}" style:print-orientation="{}"{}/>"#,
            i + 1,
            pt(pg.width),
            pt(pg.height),
            if pg.width > pg.height { "landscape" } else { "portrait" },
            format!(r#"{}{gutter} style:num-format="{}"{}"#, odt_layout::layout_props([top, bottom, pg.margin_left, pg.margin_right], &doc.deco, page_fill), odt_layout::num_format_attr(m.hf.format), crate::odt_asian::page_attrs(pg, doc.normal_size_pt()))
        );
        crate::ods::page::hf_styles(&mut s, pg.margin_top, pg.margin_header, pg.margin_bottom, pg.margin_footer);
        s.push_str("</style:page-layout>");
    }
    s.push_str("</office:automatic-styles><office:master-styles>");
    for (i, m) in masters.iter().enumerate() {
        let next = m.next.map(|n| format!(r#" style:next-style-name="{}""#, masters[n].name)).unwrap_or_default();
        let _ = write!(s, r#"<style:master-page style:name="{}" style:page-layout-name="pm{}"{next}>"#, m.name, i + 1);
        s.push_str(&master_hf[i].0);
        s.push_str("</style:master-page>");
    }
    s.push_str("</office:master-styles></office:document-styles>");
    s
}

/// The headers and footers of a master page, and whether it has a header
/// and a footer. A watermark goes in the headers.
fn master_hf_xml(doc: &Document, hf: &odt_layout::MasterHf, mark: &str, w: &mut Shared, names: &mut Names, auto: &mut String) -> (String, bool, bool) {
    use apex_doc::HfSlot;
    let mut out = String::new();
    let mut shown = [false; 2];
    for header in [true, false] {
        let get = |kind: u8| hf.parts[HfSlot::of(header, kind).index()].as_ref();
        let extra = if header { mark } else { "" };
        let any = get(0).is_some() || (hf.different_first && get(1).is_some()) || (hf.odd_even && get(2).is_some()) || !extra.is_empty();
        if !any {
            continue;
        }
        shown[usize::from(!header)] = true;
        let tag = if header { "header" } else { "footer" };
        for (kind, suffix, on) in [(0, "", true), (2, "-left", hf.odd_even), (1, "-first", hf.different_first)] {
            if !on {
                continue;
            }
            let _ = write!(out, "<style:{tag}{suffix}>");
            out.push_str(extra);
            match get(kind) {
                Some(st) => {
                    let mut mini = Document::from_paragraphs(st.to_vec());
                    mini.styles = doc.styles.clone();
                    write_paras(&mini, &mut out, auto, w, names);
                }
                None => out.push_str("<text:p/>"),
            }
            let _ = write!(out, "</style:{tag}{suffix}>");
        }
    }
    (out, shown[0], shown[1])
}

pub(crate) fn write_text(out: &mut String, text: &str) {
    let mut spaces = 0usize;
    let mut first = true;
    let flush = |out: &mut String, n: &mut usize, at_start: bool| {
        if *n == 0 {
            return;
        }
        if at_start {
            let _ = write!(out, r#"<text:s text:c="{}"/>"#, n);
        } else {
            out.push(' ');
            if *n > 1 {
                let _ = write!(out, r#"<text:s text:c="{}"/>"#, *n - 1);
            }
        }
        *n = 0;
    };
    for ch in text.chars() {
        match ch {
            ' ' => spaces += 1,
            '\t' => {
                flush(out, &mut spaces, first);
                out.push_str("<text:tab/>");
                first = false;
            }
            '\n' | LINE_BREAK => {
                flush(out, &mut spaces, first);
                out.push_str("<text:line-break/>");
                first = false;
            }
            c => {
                flush(out, &mut spaces, first);
                first = false;
                let mut b = [0u8; 4];
                out.push_str(&esc(c.encode_utf8(&mut b)));
            }
        }
    }
    if spaces > 0 {
        let _ = write!(out, r#"<text:s text:c="{}"/>"#, spaces);
    }
}

/// Automatic style names of one file (content.xml, or styles.xml for the
/// headers and footers).
#[derive(Default)]
struct Names {
    para: HashMap<(String, String, String, String), String>,
    text: HashMap<Attrs, String>,
    prefix: &'static str,
}

/// Pictures, shapes and notes, shared by the body and the headers.
struct Shared {
    /// Groups of shapes written as one `draw:g`.
    groups: HashMap<u32, Vec<Arc<apex_doc::DocShape>>>,
    pictures: odt_img::Pictures,
    shapes: crate::odf_shape::Writer,
    z_min: i32,
    nw: notes::NoteWriter,
}

/// Writes the paragraphs and tables of `doc` (the body, or a header or
/// footer) to `body`, their automatic styles to `auto`. Returns the tracked
/// changes region and the sections' master pages.
fn write_paras(doc: &Document, body: &mut String, auto: &mut String, w: &mut Shared, names: &mut Names) -> (String, Vec<odt_layout::Master>) {
    let mut tocs = 0usize;
    // Open lists: (numbered?, list_format, item open?)
    let mut stack: Vec<(bool, u8, bool)> = Vec::new();
    let close = |body: &mut String, stack: &mut Vec<(bool, u8, bool)>| {
        if let Some((_, _, item)) = stack.pop() {
            if item {
                body.push_str("</text:list-item>");
            }
            body.push_str("</text:list>");
        }
    };
    let mut tables = crate::odt_table::TblWriter::new(doc);
    let notes = crate::odt_comments::Writing::new(doc);
    let mut revw = crate::odt_revisions::Writing::default();
    let mut sects = odt_layout::SectWriter::new(doc, auto);
    let mut numbers: Option<Vec<usize>> = None;
    for (idx, p) in doc.paragraphs.iter().enumerate() {
        let key = |i: usize| doc.paragraphs[i].cell.as_ref().map(|c| c.key());
        if idx > 0 && key(idx - 1) != key(idx) {
            // Lists don't cross cell or table boundaries.
            while !stack.is_empty() {
                close(body, &mut stack);
            }
        }
        if sects.starts(idx) {
            while !stack.is_empty() {
                close(body, &mut stack);
            }
            tables.close(body);
            sects.open(body, idx);
        }
        if !tables.before(body, auto, idx) {
            continue;
        }
        let st = &p.style;
        if st.kind.is_list() {
            let numbered = st.kind == ParaKind::Numbered;
            let depth = st.level as usize + 1;
            while stack.len() > depth {
                close(body, &mut stack);
            }
            let restart = numbered && (st.restart || st.level == 0 && numbers.get_or_insert_with(|| doc.list_numbers())[idx] == 1 && idx > 0 && doc.paragraphs[idx - 1].style.kind == ParaKind::Numbered);
            // Continue the numbering across a list reopened for a new format.
            let mut cont = false;
            if stack.len() == depth && (stack[depth - 1].0 != numbered || stack[depth - 1].1 != st.list_format || restart) {
                cont = numbered && !restart && stack[depth - 1].0;
                close(body, &mut stack);
            }
            if stack.len() == depth && stack[depth - 1].2 {
                body.push_str("</text:list-item>");
                stack[depth - 1].2 = false;
            }
            while stack.len() < depth {
                if let Some(top) = stack.last_mut() {
                    if !top.2 {
                        body.push_str("<text:list-item>");
                        top.2 = true;
                    }
                }
                let _ = write!(body, r#"<text:list text:style-name="{}""#, list_style_name(!numbered, st.list_format));
                if cont && stack.len() + 1 == depth {
                    body.push_str(r#" text:continue-numbering="true""#);
                }
                body.push('>');
                stack.push((numbered, st.list_format, false));
            }
            body.push_str("<text:list-item>");
            stack.last_mut().unwrap().2 = true;
        } else {
            while !stack.is_empty() {
                close(body, &mut stack);
            }
        }
        // Paragraph automatic style.
        let mut props = String::new();
        // Display equations are centered (LibreOffice needs it written).
        let align = if apex_doc::math::is_display(p) { Align::Center } else { st.align };
        match align {
            Align::Left => {}
            Align::Center => props.push_str(r#" fo:text-align="center""#),
            Align::Right => props.push_str(r#" fo:text-align="end""#),
            Align::Justify => props.push_str(r#" fo:text-align="justify""#),
        }
        // (At least 0 is a single line of the text's own height.)
        if st.line_spacing > 0.0 || st.line_rule == LineRule::AtLeast {
            match st.line_rule {
                LineRule::Multiple => {
                    let _ = write!(props, r#" fo:line-height="{}%""#, (st.line_spacing * 100.0).round() as u32);
                }
                LineRule::Exact => {
                    let _ = write!(props, r#" fo:line-height="{}""#, ptx(st.line_spacing));
                }
                LineRule::AtLeast => {
                    let _ = write!(props, r#" style:line-height-at-least="{}""#, ptx(st.line_spacing));
                }
            }
        }
        if st.page_break_before {
            props.push_str(r#" fo:break-before="page""#);
        }
        if st.keep_with_next {
            props.push_str(r#" fo:keep-with-next="always""#);
        }
        if st.keep_lines {
            props.push_str(r#" fo:keep-together="always""#);
        }
        if st.no_widow_control {
            props.push_str(r#" fo:widows="0" fo:orphans="0""#);
        }
        if st.no_line_numbers {
            props.push_str(r#" text:number-lines="false""#);
        }
        if st.contextual_spacing {
            props.push_str(r#" style:contextual-spacing="true""#);
        }
        if let Some(b) = st.space_before {
            let _ = write!(props, r#" fo:margin-top="{}""#, ptx(b.max(0.0)));
        }
        if let Some(a) = st.space_after {
            let _ = write!(props, r#" fo:margin-bottom="{}""#, ptx(a.max(0.0)));
        }
        if st.kind.is_list() {
            if st.indent_left != 0.0 || st.first_line != 0.0 {
                let _ = write!(props, r#" fo:margin-left="{}" fo:text-indent="{}""#, ptx(36.0 * (st.level as f32 + 1.0) + st.indent_left), ptx(st.first_line - 18.0));
            }
        } else {
            if st.level > 0 || st.indent_left != 0.0 {
                let _ = write!(props, r#" fo:margin-left="{}""#, ptx(36.0 * st.level as f32 + st.indent_left));
            }
            if st.first_line != 0.0 {
                let _ = write!(props, r#" fo:text-indent="{}""#, ptx(st.first_line));
            }
        }
        if st.indent_right != 0.0 {
            let _ = write!(props, r#" fo:margin-right="{}""#, ptx(st.indent_right));
        }
        if let Some(c) = st.shading {
            let _ = write!(props, r##" fo:background-color="#{:06x}""##, c & 0xFF_FFFF);
        }
        let parent = notes::toc_named(st).unwrap_or_else(|| styles::para_parent(doc, st));
        let indent = 36.0 * (st.level as f32 + if st.kind.is_list() { 1.0 } else { 0.0 }) + st.indent_left;
        let (layout, children) = odt_layout::para_xml(st, indent);
        props.push_str(&layout);
        if let Some(n) = sects.page_number_at(idx) {
            let _ = write!(props, r#" style:page-number="{n}""#);
        }
        let master = sects.master_at(idx).map(|m| format!(r#" style:master-page-name="{}""#, esc(m))).unwrap_or_default();
        let pname = if props.is_empty() && children.is_empty() && master.is_empty() {
            parent
        } else {
            let n = names.para.len();
            names.para
                .entry((parent, props, children, master))
                .or_insert_with_key(|(parent, props, children, master)| {
                    let name = format!("{}P{}", names.prefix, n + 1);
                    let _ = write!(*auto, r#"<style:style style:name="{}" style:family="paragraph" style:parent-style-name="{}"{master}>"#, name, esc(parent));
                    if children.is_empty() {
                        let _ = write!(*auto, "<style:paragraph-properties{props}/></style:style>");
                    } else {
                        let _ = write!(*auto, "<style:paragraph-properties{props}>{children}</style:paragraph-properties></style:style>");
                    }
                    name
                })
                .clone()
        };
        notes::toc_open(body, doc, idx, &mut tocs);
        let tag = match st.kind {
            ParaKind::Heading(_) | ParaKind::Title => "text:h",
            _ => "text:p",
        };
        // A whole inserted or deleted paragraph takes its break along (the
        // change reaches the next paragraph).
        let next_plain = doc.paragraphs.get(idx + 1).is_some_and(|n| n.cell.is_none()) && p.cell.is_none();
        let whole = p.whole_revision().filter(|_| next_plain);
        let whole_del = whole.filter(|r| r.kind == apex_doc::RevKind::Delete && !st.kind.is_list());
        let into_next = whole.is_some_and(|r| r.kind == apex_doc::RevKind::Insert);
        let para_start = body.len();
        let _ = write!(body, r#"<{} text:style-name="{}""#, tag, pname);
        match st.kind {
            ParaKind::Heading(n) => {
                let _ = write!(body, r#" text:outline-level="{}""#, n.clamp(1, 9));
            }
            ParaKind::Title => body.push_str(r#" text:outline-level="1""#),
            _ => {}
        }
        body.push('>');
        if whole_del.is_none() {
            revw.start_para(body);
        }
        // Bookmarks: a start and an end at the same place of the same
        // paragraph are written as one point bookmark. Comments are 3
        // (start) and 4 (end).
        let mut marks: Vec<(usize, u8, &str)> = Vec::new();
        // A form field is its own bookmark: one of its name would clash.
        let forms: Vec<&str> = p.runs.iter().filter_map(|r| r.attrs.control.as_ref()?.legacy.as_ref()).map(|f| f.name.as_str()).collect();
        // Editable ranges have no equivalent: only their text stays.
        for m in p.marks.iter().filter(|m| !apex_doc::is_perm_mark(&m.name)) {
            if apex_doc::is_comment_mark(&m.name) {
                marks.push((m.offset, if m.end { 4 } else { 3 }, &m.name));
                continue;
            }
            if forms.contains(&&*m.name) {
                continue;
            }
            let point = p.marks.iter().any(|o| o.name == m.name && o.offset == m.offset && o.end != m.end);
            match (point, m.end) {
                (true, true) => {}
                (true, false) => marks.push((m.offset, 0, &m.name)),
                (false, false) => marks.push((m.offset, 1, &m.name)),
                (false, true) => marks.push((m.offset, 2, &m.name)),
            }
        }
        marks.sort();
        let mut mi = 0;
        let emit_marks = |body: &mut String, upto: usize, mi: &mut usize| {
            while let Some(&(_, k, name)) = marks.get(*mi).filter(|m| m.0 <= upto) {
                *mi += 1;
                if k >= 3 {
                    body.push_str(notes.mark(name, k == 4));
                    continue;
                }
                let tag = ["text:bookmark", "text:bookmark-start", "text:bookmark-end"][k as usize];
                let _ = write!(body, r#"<{tag} text:name="{}"/>"#, esc(name));
            }
        };
        // The open control, and whether it is a form field.
        let mut open_ctl: Option<(u32, bool)> = None;
        let mut open_ruby: Option<Arc<apex_doc::Ruby>> = None;
        let close_ruby = |body: &mut String, auto: &mut String, r: &apex_doc::Ruby| {
            body.push_str("</text:ruby-base>");
            crate::odt_asian::open_ruby_text(body, auto, &names.prefix, r);
            write_text(body, &r.text);
            body.push_str("</text:ruby-text></text:ruby>");
        };
        for (s, e, a) in p.spans() {
            if s == e {
                continue;
            }
            let mut a = a.clone();
            if st.kind == ParaKind::Code {
                a.code = false;
            }
            // Phonetic guides wrap the runs of their base text.
            let ruby = a.asian.as_ref().and_then(|x| x.ruby.clone());
            if ruby.is_some() {
                a.set_asian(|x| x.ruby = None);
            }
            if ruby != open_ruby {
                if let Some(r) = open_ruby.take() {
                    close_ruby(body, auto, &r);
                }
                if let Some(r) = &ruby {
                    let (name, def) = crate::odt_asian::ruby_style(&names.prefix, r);
                    if !auto.contains(&format!(r#"style:name="{name}""#)) {
                        auto.push_str(&def);
                    }
                    let _ = write!(body, r#"<text:ruby text:style-name="{name}"><text:ruby-base>"#);
                }
                open_ruby = ruby;
            }
            let ctl = a.control.take();
            if ctl.as_ref().map(|c| c.id) != open_ctl.map(|o| o.0) {
                emit_marks(body, s, &mut mi);
                if let Some((_, form)) = open_ctl {
                    body.push_str(if form { crate::odt_forms::CLOSE } else { crate::odt_controls::CLOSE });
                }
                open_ctl = None;
                if let Some(c) = &ctl {
                    match &c.legacy {
                        Some(f) => {
                            crate::odt_forms::open(body, c, f, &p.text[s..e]);
                            // Check boxes and drop-downs are whole.
                            if matches!(c.kind, apex_doc::ControlKind::PlainText | apex_doc::ControlKind::RichText) {
                                open_ctl = Some((c.id, true));
                            }
                        }
                        None => {
                            crate::odt_controls::open(body, c);
                            open_ctl = Some((c.id, false));
                        }
                    }
                }
            }
            if ctl.as_ref().is_some_and(|c| c.legacy.is_some() && open_ctl.is_none()) {
                continue;
            }
            let rev = a.rev.take().filter(|_| whole_del.is_none());
            // Pictures and shapes in the formatting of their text.
            let span = if a.image.is_some() || a.shape.is_some() { notes::text_style(&a, &mut names.text, auto) } else { None };
            if let Some(img) = &a.image {
                emit_marks(body, s, &mut mi);
                let out = match revw.span(body, rev.as_ref()) {
                    Some(b) => b,
                    None => &mut *body,
                };
                if let Some(name) = &span {
                    let _ = write!(out, r#"<text:span text:style-name="{name}">"#);
                }
                for _ in p.text[s..e].matches(OBJECT) {
                    match img.object.as_ref().and_then(|o| o.diagram()) {
                        Some(d) => {
                            let anchor = if img.wrap == apex_doc::Wrap::Inline { r#"text:anchor-type="as-char""# } else { r#"text:anchor-type="paragraph""# };
                            crate::odf_ole::write_diagram(&mut w.shapes, out, d, (img.offset_x, 0.0), (img.width, img.height), anchor, 0);
                        }
                        None => w.pictures.frame(out, img),
                    }
                }
                if span.is_some() {
                    out.push_str("</text:span>");
                }
                continue;
            }
            if let Some(sh) = &a.shape {
                emit_marks(body, s, &mut mi);
                let out = match revw.span(body, rev.as_ref()) {
                    Some(b) => b,
                    None => &mut *body,
                };
                if let Some(name) = &span {
                    let _ = write!(out, r#"<text:span text:style-name="{name}">"#);
                }
                for _ in p.text[s..e].matches(OBJECT) {
                    if let Some(g) = sh.group
                        && let Some(members) = w.groups.get(&g)
                    {
                        // The first member writes the group.
                        if members.first().is_some_and(|m| Arc::ptr_eq(m, sh)) {
                            let z = (members.iter().map(|m| m.z).max().unwrap_or(0) as i64 - w.z_min as i64).max(0);
                            let _ = write!(out, r#"<draw:g text:anchor-type="paragraph" draw:z-index="{z}">"#);
                            for m in members.clone() {
                                if m.shape.picture.is_some() {
                                    w.pictures.floating_frame(out, &m, w.z_min);
                                } else {
                                    w.shapes.write_doc(out, &m, w.z_min);
                                }
                            }
                            out.push_str("</draw:g>");
                        }
                        continue;
                    }
                    if let Some(d) = sh.shape.picture.as_ref().and_then(|p| p.object.as_ref()).and_then(|o| o.diagram()) {
                        let z = (sh.z as i64 - w.z_min as i64).max(0);
                        crate::odf_ole::write_diagram(&mut w.shapes, out, d, (sh.x, sh.y), (sh.width, sh.height), r#"text:anchor-type="paragraph""#, z);
                    } else if sh.shape.picture.is_some() {
                        w.pictures.floating_frame(out, sh, w.z_min);
                    } else {
                        w.shapes.write_doc(out, sh, w.z_min);
                    }
                }
                if span.is_some() {
                    out.push_str("</text:span>");
                }
                continue;
            }
            if a.note.is_some() || a.field.is_some() {
                emit_marks(body, s, &mut mi);
                notes::write_object(body, &p.text[s..e], &a, doc, &mut w.nw, &mut names.text, auto);
                continue;
            }
            if let Some(eq) = &a.math {
                emit_marks(body, s, &mut mi);
                for _ in p.text[s..e].matches(OBJECT) {
                    w.pictures.formulas.frame(body, eq, apex_doc::math::is_display(p), a.size.unwrap_or(24) as f32 / 2.0);
                }
                continue;
            }
            let link = a.link.take();
            let tip = a.link_tip.take();
            let mut pos = s;
            while pos < e {
                emit_marks(body, pos, &mut mi);
                let next = marks.get(mi).map_or(e, |m| m.0.min(e));
                let text = &p.text[pos..next];
                pos = next;
                // Deleted text goes into its change region.
                let body = match revw.span(body, rev.as_ref()) {
                    Some(b) => b,
                    None => &mut *body,
                };
                if let Some(l) = &link {
                    let _ = write!(body, r#"<text:a xlink:type="simple" xlink:href="{}""#, esc(l));
                    if let Some(t) = &tip {
                        let _ = write!(body, r#" office:title="{}""#, esc(t));
                    }
                    body.push_str(r#" text:style-name="Internet_20_link">"#);
                }
                let char_style = a.char_style.as_deref().filter(|id| doc.styles.by_id(id).is_some()).map(styles::file_name);
                if a == Attrs::default() {
                    write_text(body, text);
                } else if let Some(cs) = char_style.as_ref().filter(|_| a == Attrs { char_style: a.char_style.clone(), ..Default::default() }) {
                    let _ = write!(body, r#"<text:span text:style-name="{}">"#, esc(cs));
                    write_text(body, text);
                    body.push_str("</text:span>");
                } else {
                    let n = names.text.len();
                    let name = names.text
                        .entry(a.clone())
                        .or_insert_with(|| {
                            let name = format!("{}T{}", names.prefix, n + 1);
                            let parent = char_style.as_ref().map(|c| format!(r#" style:parent-style-name="{}""#, esc(c))).unwrap_or_default();
                            let _ = write!(*auto, r#"<style:style style:name="{}" style:family="text"{}><style:text-properties{}/></style:style>"#, name, parent, text_props(&a));
                            name
                        })
                        .clone();
                    let _ = write!(body, r#"<text:span text:style-name="{}">"#, name);
                    write_text(body, text);
                    body.push_str("</text:span>");
                }
                if link.is_some() {
                    body.push_str("</text:a>");
                }
            }
        }
        if let Some(r) = open_ruby.take() {
            close_ruby(body, auto, &r);
        }
        if let Some((_, form)) = open_ctl {
            body.push_str(if form { crate::odt_forms::CLOSE } else { crate::odt_controls::CLOSE });
        }
        emit_marks(body, usize::MAX, &mut mi);
        revw.end_para(body, into_next);
        let _ = write!(body, "</{}>", tag);
        if let Some(r) = whole_del {
            let xml = body.split_off(para_start);
            revw.deleted_paragraph(r, &xml);
        }
        notes::toc_close(body, doc, idx);
    }
    while !stack.is_empty() {
        close(body, &mut stack);
    }
    tables.close(body);
    let tracked = revw.finish(body, doc.track_changes);
    sects.close(body);
    (tracked, sects.masters)
}

pub fn write(doc: &Document) -> Vec<u8> {
    // Sequence fields (captions) are written with their numbers.
    let fresh = doc.with_seq_results();
    let doc = fresh.as_ref().unwrap_or(doc);
    // ODF has no linked styles: their text gets a character style twin.
    let linked = crate::docx_styles::linked_as_char(doc);
    let doc = linked.as_ref().unwrap_or(doc);
    crate::theme_xml::set_writing_theme(&doc.styles.theme);
    let mut auto = String::new();
    let z_min = doc.shapes().iter().map(|(_, s)| s.z).min().unwrap_or(0);
    let mut w = Shared { groups: crate::docx_shape::group_plan(doc), pictures: odt_img::Pictures::default(), shapes: crate::odf_shape::Writer::default(), z_min, nw: notes::NoteWriter::default() };
    w.shapes.name_chains(doc);
    let mut body = String::with_capacity(doc.paragraphs.iter().map(|p| p.text.len() + 80).sum());
    let (tracked, masters) = write_paras(doc, &mut body, &mut auto, &mut w, &mut Names::default());
    let wm_href = doc.deco.watermark.as_ref().and_then(|m| m.picture.as_ref()).map(|d| w.pictures.file(d));
    // Headers and footers, with their automatic styles in styles.xml.
    let mut hnames = Names { prefix: "H", ..Default::default() };
    let mut hauto = String::new();
    let wm = doc.deco.watermark.as_ref().filter(|m| !m.is_empty() && (m.picture.is_none() || wm_href.is_some()));
    let master_hf: Vec<(String, bool, bool)> = masters
        .iter()
        .map(|m| {
            let mark = wm.map(|x| odt_layout::watermark_xml(x, &m.page, wm_href.as_deref())).unwrap_or_default();
            master_hf_xml(doc, &m.hf, &mark, &mut w, &mut hnames, &mut hauto)
        })
        .collect();
    hauto.push_str(&w.pictures.styles());
    hauto.push_str(&w.shapes.styles);

    let mut content = String::with_capacity(body.len() + auto.len() + 1024);
    content.push_str(HEADER);
    content.push_str(r#"<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:drawooo="http://openoffice.org/2010/draw" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:loext="urn:org:documentfoundation:names:experimental:office:xmlns:loext:1.0" xmlns:field="urn:openoffice:names:experimental:ooo-ms-interop:xmlns:field:1.0" office:version="1.3"><office:automatic-styles>"#);
    content.push_str(&auto);
    content.push_str(&w.pictures.styles());
    content.push_str(&w.shapes.styles);
    content.push_str("</office:automatic-styles><office:body><office:text>");
    content.push_str(&tracked);
    content.push_str(&body);
    content.push_str("</office:text></office:body></office:document-content>");

    let mut z = ZipWriter::new();
    z.add("mimetype", MIME.as_bytes(), false);
    let mut basic = String::new();
    for (path, mime, data) in crate::macro_parts::odf_files(&doc.macros) {
        basic.push_str(&format!(r#"<manifest:file-entry manifest:full-path="{}" manifest:media-type="{mime}"/>"#, crate::xml::esc(&path)));
        z.add(&path, &data, true);
    }
    let (font_decls, font_files) = crate::odt_fonts::write(&mut z, &doc.fonts);
    basic.push_str(&font_files);
    if let Some(b) = &doc.book {
        basic.push_str(crate::book_xml::ODT_MANIFEST);
        z.add(crate::book_xml::ODT_PART, crate::book_xml::to_xml(b).as_bytes(), true);
    }
    if let Some(s) = crate::odt_settings::write(doc) {
        basic.push_str(r#"<manifest:file-entry manifest:full-path="settings.xml" manifest:media-type="text/xml"/>"#);
        z.add("settings.xml", s.as_bytes(), true);
    }
    z.add("META-INF/manifest.xml", format!(
        r#"{}<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3"><manifest:file-entry manifest:full-path="/" manifest:version="1.3" manifest:media-type="{}"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="styles.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="meta.xml" manifest:media-type="text/xml"/>{}{basic}</manifest:manifest>"#,
        HEADER, MIME, w.pictures.manifest()
    ).as_bytes(), true);
    for (path, data) in w.pictures.files() {
        z.add(&path, data, false);
    }
    z.add("meta.xml", crate::props::odf_xml(&doc.meta).as_bytes(), true);
    // A gradient page background (a named gradient, as LibreOffice).
    let page_fill = match (&doc.deco.gradient, doc.deco.color) {
        (Some(g), Some(_)) => w.shapes.named.props(g),
        _ => String::new(),
    };
    let mut styles = styles_xml(doc, &masters, &master_hf, &hauto, wm.is_some(), &page_fill);
    if !font_decls.is_empty() {
        styles = styles.replacen("<office:styles>", &format!("{font_decls}<office:styles>"), 1);
    }
    if !w.shapes.named.xml.is_empty() {
        styles = styles.replacen("</office:styles>", &format!("{}</office:styles>", w.shapes.named.xml), 1);
    }
    z.add("styles.xml", styles.as_bytes(), true);
    z.add("content.xml", content.as_bytes(), true);
    z.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let d = crate::docx::tests::sample();
        let back = read(&write(&d)).unwrap();
        crate::docx::tests::check(&back, &d);
    }

    #[test]
    fn images() {
        let d = crate::docx::tests::with_images();
        let bytes = write(&d);
        let back = read(&bytes).unwrap();
        // Crops are stored as lengths of the (unknown) picture size: compare
        // the rest.
        let mut orig = d.clone();
        for (pos, img) in d.images() {
            orig.set_image(pos, apex_doc::Image { crop: back.image_at(pos).unwrap().1.crop, ..(*img).clone() });
        }
        crate::docx::tests::check_images(&back, &orig);
    }

    #[test]
    fn nested_tables() {
        let d = crate::docx::tests::nested();
        let bytes = write(&d);
        if let Some(dir) = std::env::var_os("APEX_DUMP") {
            std::fs::write(std::path::Path::new(&dir).join("nested.odt"), &bytes).unwrap();
        }
        let back = read(&bytes).unwrap();
        assert_eq!(crate::docx::tests::nest_shape(&back), crate::docx::tests::nest_shape(&d));
        let deep = back.all_tables()[2].start;
        assert_eq!(back.table_map(deep).unwrap().cell_props(&back.paragraphs, 0, 0).and_then(|p| p.shading), Some(0xFFEE00));
    }

    #[test]
    fn headers() {
        let d = crate::docx::tests::with_headers();
        let bytes = write(&d);
        if let Some(dir) = std::env::var_os("APEX_DUMP") {
            std::fs::write(std::path::Path::new(&dir).join("headers.odt"), &bytes).unwrap();
        }
        crate::docx::tests::check_headers(&read(&bytes).unwrap(), &d);
        let mut plain = crate::docx::tests::sample();
        plain.page.margin_header = 20.0;
        let back = read(&write(&plain)).unwrap();
        crate::docx::tests::check_headers(&back, &plain);
    }

    #[test]
    fn links_and_bookmarks() {
        let d = crate::docx::tests::linked();
        let bytes = write(&d);
        let back = read(&bytes).unwrap();
        crate::docx::tests::check_links(&back, &d);
        let content = ZipReader::new(&bytes).unwrap().read_string("content.xml").unwrap();
        assert!(content.contains(r#"<text:bookmark text:name="Punto"/>"#));
        assert!(content.contains(r##"xlink:href="#Seccion_2""##));
        assert!(content.contains(r#"office:title="Ejemplo &amp; más""#));
    }

    #[test]
    fn restart_numbering() {
        let mut ps = Vec::new();
        for t in ["a", "b"] {
            ps.push(Paragraph::new(t).with_style(ParaStyle { kind: ParaKind::Numbered, ..Default::default() }));
        }
        ps.push(Paragraph::new("x"));
        ps.push(Paragraph::new("c").with_style(ParaStyle { kind: ParaKind::Numbered, ..Default::default() }));
        let d = Document::from_paragraphs(ps);
        let back = read(&write(&d)).unwrap();
        assert_eq!(back.list_number(3), 1);
        assert_eq!(back.list_number(1), 2);
    }
}
