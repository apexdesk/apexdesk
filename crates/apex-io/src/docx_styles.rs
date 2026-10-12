//! Named styles in docx: changes to the built-in paragraph styles, custom
//! paragraph styles and character styles (see `apex_doc::Styles`).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{Align, Attrs, Document, ParaKind, ParaStyle, Paragraph, StyleDef, Styles, kind_id};

use crate::xml::{Ev, Reader, esc, local};
use crate::is_mono_font;

const TWIP: f32 = 20.0;

/// A style as the file defines it (its own properties, without the ones
/// it inherits).
#[derive(Default, Clone)]
struct Raw {
    char_style: bool,
    /// The default style of its type (Normal).
    default: bool,
    /// The paragraph style that stands for Normal: the default one, else
    /// (no default) the first one named like Normal. Only one per file.
    normal: bool,
    name: String,
    based_on: Option<String>,
    /// The linked style (a character style of a paragraph style or back).
    link: Option<String>,
    next: Option<String>,
    props: StyleDef,
}

fn on(v: Option<std::borrow::Cow<str>>) -> bool {
    v.is_none_or(|v| !matches!(v.as_ref(), "0" | "false" | "off" | "none"))
}

fn read_raw(xml: &str, theme: &apex_doc::Theme) -> HashMap<String, Raw> {
    let mut out = HashMap::new();
    let mut order = Vec::new();
    let mut r = Reader::new(xml);
    let mut cur: Option<(String, Raw)> = None;
    let mut in_ppr = false;
    loop {
        let ev = r.next();
        let is_start = matches!(ev, Ev::Start(_));
        match ev {
            Ev::End(n) if local(n) == "pPr" => in_ppr = false,
            Ev::Eof => break,
            Ev::Start(t) if t.local() == "style" => {
                let ty = t.attr("w:type").unwrap_or_default();
                if ty == "paragraph" || ty == "character" {
                    let id = t.attr("w:styleId").unwrap_or_default().into_owned();
                    let default = t.attr("w:default").is_some_and(|v| v == "1" || v == "true");
                    cur = Some((id, Raw { char_style: ty == "character", default, ..Default::default() }));
                } else {
                    r.skip();
                }
            }
            Ev::End(n) if local(n) == "style" => {
                if let Some((id, s)) = cur.take() {
                    order.push(id.clone());
                    out.insert(id, s);
                }
            }
            Ev::Start(t) | Ev::Empty(t) => {
                let Some((_, s)) = &mut cur else { continue };
                let val = || t.attr("w:val");
                let p = &mut s.props;
                // Asian fonts, languages and line breaking.
                if in_ppr && crate::docx_asian::read_ppr(&t, &mut p.asian_para) {
                    continue;
                }
                let mut x = p.asian.as_deref().cloned().unwrap_or_default();
                let only = crate::docx_asian::read_rpr(&t, &mut x);
                x.cs_size = None;
                if !x.is_empty() {
                    p.asian = Some(Arc::new(x));
                }
                if only {
                    continue;
                }
                let mut fx = p.fx.as_deref().cloned().unwrap_or_default();
                if crate::docx_fx::read(&t, &mut fx) {
                    if t.local() == "u" {
                        p.underline = Some(val().is_none_or(|v| v != "none"));
                    }
                    p.fx = (!fx.is_default()).then(|| std::sync::Arc::new(fx));
                    continue;
                }
                match t.local() {
                    "name" => s.name = val().unwrap_or_default().into_owned(),
                    "basedOn" => s.based_on = val().map(|v| v.into_owned()),
                    "link" => s.link = val().map(|v| v.into_owned()),
                    "next" => s.next = val().map(|v| v.into_owned()),
                    "pPr" => in_ppr = is_start,
                    "widowControl" if in_ppr => p.no_widow_control = Some(!on(val())),
                    "contextualSpacing" if in_ppr => p.contextual_spacing = Some(on(val())),
                    "shd" if in_ppr => p.shading = t.attr("w:fill").filter(|v| v != "auto").and_then(|v| crate::parse_hex_color(&v)).filter(|&c| c != 0xFFFFFF),
                    "pBdr" if is_start => {
                        let mut b = apex_doc::ParaBorders::default();
                        crate::docx_layout::read_pbdr(&mut r, &mut b);
                        p.borders = (!b.is_empty()).then_some(b);
                    }
                    "tabs" if is_start => {
                        let mut t = apex_doc::TabStops::default();
                        crate::docx_layout::read_tabs(&mut r, &mut t);
                        p.tabs = (!t.is_empty()).then_some(t);
                    }
                    "rFonts" => {
                        if let Some(f) = crate::theme_xml::read_word_font(&t) {
                            p.font = Some(f);
                        }
                    }
                    "sz" => p.size = val().and_then(|v| v.parse().ok()).filter(|&s: &u16| s > 0),
                    "b" => p.bold = Some(on(val())),
                    "i" => p.italic = Some(on(val())),
                    "color" => p.color = crate::theme_xml::read_word_color(&t, theme),
                    "jc" => {
                        p.align = Some(match val().as_deref() {
                            Some("center") => Align::Center,
                            Some("right") | Some("end") => Align::Right,
                            Some("both") | Some("distribute") => Align::Justify,
                            _ => Align::Left,
                        })
                    }
                    "spacing" => {
                        let tw = |k: &str| t.attr(k).and_then(|v| v.parse::<f32>().ok());
                        if let Some(b) = tw("w:before") {
                            p.space_before = Some(b / TWIP);
                        }
                        if let Some(a) = tw("w:after") {
                            p.space_after = Some(a / TWIP);
                        }
                        if let Some(l) = tw("w:line") {
                            if t.attr("w:lineRule").is_none_or(|v| v == "auto") {
                                p.line_spacing = Some((l / 240.0 * 100.0).round() / 100.0);
                            }
                        }
                    }
                    "ind" => {
                        let tw = |k: &str| t.attr(k).and_then(|v| v.parse::<f32>().ok());
                        if let Some(l) = tw("w:left").or_else(|| tw("w:start")) {
                            p.indent_left = Some(l / TWIP);
                        }
                        if let Some(f) = tw("w:firstLine") {
                            p.first_line = Some(f / TWIP);
                        } else if let Some(h) = tw("w:hanging") {
                            p.first_line = Some(-h / TWIP);
                        }
                    }
                    // Paragraph mark and revision properties aren't the style's.
                    "pPrChange" | "rPrChange" | "tblPr" | "tcPr" | "trPr" | "tblStylePr" => {
                        if is_start {
                            r.skip();
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    mark_normal(&mut out, &order);
    out
}

/// Sets `Raw::normal` on one paragraph style, in file order (`order`: the
/// ids as they come), so that the choice doesn't depend on hashing.
fn mark_normal(raws: &mut HashMap<String, Raw>, order: &[String]) {
    let named = |r: &Raw| ["normal", "default", "default style", "standard"].iter().any(|n| r.name.eq_ignore_ascii_case(n));
    let para = |id: &String| raws.get(id).filter(|r| !r.char_style);
    let pick = order.iter().find(|id| para(id).is_some_and(|r| r.default))
        .or_else(|| order.iter().find(|id| para(id).is_some_and(|r| r.name.eq_ignore_ascii_case("normal"))))
        .or_else(|| order.iter().find(|id| para(id).is_some_and(named)))
        .cloned();
    if let Some(r) = pick.and_then(|id| raws.get_mut(&id)) {
        r.normal = true;
    }
}

/// Lays `over` on top of `base` (fields `over` sets win).
fn merge(base: &mut StyleDef, over: &StyleDef) {
    macro_rules! take {
        ($($f:ident),*) => {$(if over.$f.is_some() { base.$f.clone_from(&over.$f); })*};
    }
    take!(font, size, bold, italic, underline, color, align, space_before, space_after, line_spacing, indent_left, first_line, shading, borders, tabs, no_widow_control, contextual_spacing);
    if let Some(o) = &over.fx {
        let mut fx = base.fx.as_deref().cloned().unwrap_or_default();
        fx.overlay(o);
        base.fx = Some(std::sync::Arc::new(fx));
    }
    if let Some(o) = &over.asian {
        let mut a = base.asian.as_deref().cloned().unwrap_or_default();
        a.overlay(o);
        base.asian = Some(Arc::new(a));
    }
    base.asian_para.overlay(&over.asian_para);
}

/// Normal, maybe under another name (older LibreOffice wrote "Default").
fn is_normal(raw: &Raw) -> bool {
    raw.normal
}

/// The file's Normal style.
fn normal_of(raws: &HashMap<String, Raw>) -> Option<&Raw> {
    raws.values().find(|r| r.normal)
}

/// Built-in kind of a style by its name, as `docx::kind_from_name` sees it.
fn builtin_kind(raw: &Raw) -> Option<ParaKind> {
    crate::docx::kind_from_name(&raw.name)
}

/// Properties of a style and its ancestors up to (not including) the
/// first built-in style or Normal.
fn own_chain(raws: &HashMap<String, Raw>, id: &str) -> StyleDef {
    let mut chain = Vec::new();
    let mut cur = Some(id.to_string());
    while let Some(i) = cur {
        if chain.len() > 12 {
            break;
        }
        let Some(s) = raws.get(&i) else { break };
        if !chain.is_empty() && (builtin_kind(s).is_some() || is_normal(s) || s.default || s.name.eq_ignore_ascii_case("default paragraph font")) {
            break;
        }
        chain.push(s);
        cur = s.based_on.clone();
    }
    let mut def = StyleDef::default();
    for s in chain.iter().rev() {
        merge(&mut def, &s.props);
    }
    def
}

/// Properties of a style with all its ancestors (Normal included).
fn full_chain(raws: &HashMap<String, Raw>, id: &str) -> StyleDef {
    let mut chain = Vec::new();
    let mut cur = Some(id.to_string());
    while let Some(i) = cur {
        let Some(s) = raws.get(&i).filter(|_| chain.len() <= 12) else { break };
        chain.push(s);
        cur = s.based_on.clone();
    }
    let mut def = StyleDef::default();
    for s in chain.iter().rev() {
        merge(&mut def, &s.props);
    }
    def
}

/// Space before and after paragraphs in `w:docDefaults` (points).
fn default_spacing(xml: &str) -> (Option<f32>, Option<f32>) {
    let mut r = Reader::new(xml);
    let mut in_def = false;
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) if t.local() == "docDefaults" => in_def = true,
            Ev::End(n) if local(n) == "docDefaults" => break,
            Ev::Start(t) | Ev::Empty(t) if in_def && t.local() == "spacing" => {
                let tw = |k: &str| t.attr(k).and_then(|v| v.parse::<f32>().ok()).map(|v| v / TWIP);
                return (tw("w:before"), tw("w:after"));
            }
            _ => {}
        }
    }
    (None, None)
}

/// Our defaults for a built-in kind, in `StyleDef` terms.
fn kind_defaults(kind: ParaKind) -> StyleDef {
    let st = ParaStyle { kind, ..Default::default() };
    let (before, after) = st.default_spacing();
    StyleDef {
        size: (!kind.inherits_size()).then(|| kind.base_size()),
        bold: Some(kind.base_bold()),
        italic: Some(kind == ParaKind::Quote),
        underline: Some(false),
        color: match kind {
            ParaKind::Quote => Some(0x555555),
            _ => None,
        },
        align: Some(Align::Left),
        space_before: Some(before),
        space_after: Some(after),
        ..Default::default()
    }
}

/// The fields of `def` that differ from our defaults for its kind.
fn diff_from_defaults(def: &StyleDef, kind: ParaKind) -> StyleDef {
    let d = kind_defaults(kind);
    let mut out = StyleDef::default();
    macro_rules! diff {
        ($($f:ident),*) => {$(if def.$f.is_some() && def.$f != d.$f { out.$f.clone_from(&def.$f); })*};
    }
    diff!(size, bold, italic, underline, color, align, space_before, space_after, shading, borders, tabs, no_widow_control, contextual_spacing);
    (out.asian, out.asian_para) = (def.asian.clone(), def.asian_para);
    // Monospaced fonts come from the code kind; theme fonts aren't read.
    if def.font.as_deref().is_some_and(|f| !is_mono_font(f)) {
        out.font.clone_from(&def.font);
    }
    out
}

/// Our id of a paragraph style of the file: built-in ones by kind.
fn map_id(raws: &HashMap<String, Raw>, id: &str) -> Arc<str> {
    match raws.get(id) {
        Some(r) if is_normal(r) => "Normal".into(),
        Some(r) => match builtin_kind(r) {
            Some(k) if !k.is_list() => kind_id(k).into(),
            _ => id.into(),
        },
        None => id.into(),
    }
}

/// Paragraph formatting of a style the paragraph doesn't set itself.
fn fill_para(st: &mut ParaStyle, d: &StyleDef) {
    if st.align == Align::Left {
        if let Some(a) = d.align {
            st.align = a;
        }
    }
    if st.space_before.is_none() {
        st.space_before = d.space_before;
    }
    if st.space_after.is_none() {
        st.space_after = d.space_after;
    }
    if st.shading.is_none() {
        st.shading = d.shading;
    }
    if d.no_widow_control == Some(true) {
        st.no_widow_control = true;
    }
    if d.contextual_spacing == Some(true) {
        st.contextual_spacing = true;
    }
    if st.borders.is_empty() {
        if let Some(b) = d.borders {
            st.borders = b;
        }
    }
    if st.tabs.is_empty() {
        if let Some(t) = d.tabs {
            st.tabs = t;
        }
    }
}

/// Reader side: interns paragraph style ids while reading paragraphs.
#[derive(Default)]
pub(crate) struct Ids {
    ids: Vec<String>,
    /// Where each id is in `ids`.
    index: HashMap<String, u16>,
}

impl Ids {
    /// A temporary `ParaStyle::custom` number for a style id (past the
    /// numbers there are, ids not seen yet share the last one).
    pub(crate) fn number(&mut self, id: &str) -> u16 {
        if let Some(n) = self.index.get(id) {
            return *n;
        }
        if self.ids.len() >= u16::MAX as usize - 1 {
            return self.ids.len() as u16;
        }
        self.ids.push(id.to_string());
        let n = self.ids.len() as u16;
        self.index.insert(id.to_string(), n);
        n
    }
}

impl Ids {
    /// The style id of a temporary number.
    pub(crate) fn id(&self, n: u16) -> Option<&str> {
        self.ids.get((n as usize).checked_sub(1)?).map(String::as_str)
    }
}

/// How paragraphs that keep no named style (the text of notes and their
/// separator) look: what their style chain and the document defaults
/// give them.
pub(crate) struct PlainLooks {
    raws: HashMap<String, Raw>,
    normal: Option<String>,
    defaults: Option<apex_doc::DocDefaults>,
    spacing: (Option<f32>, Option<f32>),
}

/// Font size (half points), space before and after (points) and line
/// spacing (a multiple) of a paragraph style, and the font it sets over
/// Normal's.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PlainLook {
    pub font: Option<Arc<str>>,
    pub size: Option<u16>,
    pub before: f32,
    pub after: f32,
    pub line: f32,
}

impl PlainLooks {
    pub(crate) fn new(xml: Option<&str>, theme: &apex_doc::Theme) -> PlainLooks {
        let raws = xml.map(|x| read_raw(x, theme)).unwrap_or_default();
        let normal = raws.iter().find(|(_, r)| r.normal).map(|(id, _)| id.clone());
        PlainLooks { raws, normal, defaults: xml.and_then(crate::docx_defaults::read), spacing: xml.map(default_spacing).unwrap_or_default() }
    }

    /// The look of a paragraph of style `id` (Normal without one or when
    /// the file lacks it).
    pub(crate) fn of(&self, id: Option<&str>) -> PlainLook {
        let id = id.filter(|i| self.raws.contains_key(*i)).or(self.normal.as_deref());
        let chain = id.map(|i| full_chain(&self.raws, i)).unwrap_or_default();
        let d = self.defaults.as_ref();
        let line = match d.and_then(|d| d.line) {
            Some((apex_doc::LineRule::Multiple, l)) if l > 0.0 => l,
            _ => 1.0,
        };
        let own = id.filter(|i| self.normal.as_deref() != Some(*i)).and_then(|i| own_chain(&self.raws, i).font).filter(|f| !is_mono_font(f));
        PlainLook {
            font: own,
            size: chain.size.or(d.and_then(|d| d.size)),
            before: chain.space_before.or(self.spacing.0).unwrap_or(0.0),
            after: chain.space_after.or(self.spacing.1).unwrap_or(0.0),
            line: chain.line_spacing.filter(|&l| l > 0.0).unwrap_or(line),
        }
    }
}

/// Turns the temporary style numbers and run style ids of the paragraphs
/// into document styles.
pub(crate) fn import(paras: &mut [Paragraph], styles_xml: Option<&str>, ids: &Ids, theme: &apex_doc::Theme) -> Styles {
    let raws = styles_xml.map(|x| read_raw(x, theme)).unwrap_or_default();
    let mut styles = Styles { theme: theme.clone(), ..Styles::default() };
    // Paragraph styles.
    let mut numbers: HashMap<u16, u16> = HashMap::new();
    let mut builtins: Vec<(ParaKind, String)> = Vec::new();
    let defaults = styles_xml.map(default_spacing);
    let normal_id = raws.iter().find(|(_, r)| r.normal).map(|(id, _)| id.clone());
    for p in paras.iter_mut() {
        let n = std::mem::take(&mut p.style.custom);
        let id = match n {
            0 => normal_id.as_ref(),
            n => ids.ids.get(n as usize - 1),
        };
        let Some(id) = id else { continue };
        let Some(raw) = raws.get(id) else { continue };
        let kind = p.style.kind;
        // What the whole style chain says (Normal's included): spacing
        // between paragraphs of one style, and the spacing of list items
        // (their kind's own defaults aren't Word's).
        let full = full_chain(&raws, id);
        if full.contextual_spacing == Some(true) {
            p.style.contextual_spacing = true;
        }
        if kind.is_list() {
            if let Some((db, da)) = defaults {
                // (Where it's the kind's own, as in our files, it stays unset.)
                let (kb, ka) = p.style.default_spacing();
                let st = &mut p.style;
                let before = full.space_before.or(db).unwrap_or(0.0);
                let after = full.space_after.or(da).unwrap_or(0.0);
                if st.space_before.is_none() && before != kb {
                    st.space_before = Some(before);
                }
                if st.space_after.is_none() && after != ka {
                    st.space_after = Some(after);
                }
                if st.line_spacing == 0.0 && st.line_rule == apex_doc::LineRule::Multiple {
                    st.line_spacing = full.line_spacing.unwrap_or(0.0);
                }
            }
        }
        if n == 0 {
            continue;
        }
        if builtin_kind(raw).is_some() || is_normal(raw) || kind.is_list() {
            if matches!(kind, ParaKind::Title | ParaKind::Heading(_) | ParaKind::Quote) && builtin_kind(raw) == Some(kind) && !builtins.iter().any(|(k, _)| *k == kind) {
                builtins.push((kind, id.clone()));
            }
            continue;
        }
        let custom = match numbers.get(&n) {
            Some(&c) => c,
            None => {
                let mut def = own_chain(&raws, id);
                def.font = def.font.filter(|f| !is_mono_font(f));
                def.id = id.as_str().into();
                def.name = if raw.name.is_empty() { id.as_str().into() } else { raw.name.as_str().into() };
                def.kind = if kind == ParaKind::Code { ParaKind::Normal } else { kind };
                def.next = raw.next.as_deref().map(|n| map_id(&raws, n)).filter(|n| **n != *id);
                def.linked = raw.link.is_some();
                styles.defs.push(def);
                let c = styles.defs.len() as u16;
                numbers.insert(n, c);
                c
            }
        };
        p.style.custom = custom;
        if let Some(d) = styles.custom(custom) {
            // Formatting the paragraph doesn't set itself comes from the style.
            let st = &mut p.style;
            fill_para(st, d);
            if st.line_spacing == 0.0 {
                if let Some(l) = d.line_spacing {
                    st.line_spacing = l;
                }
            }
            if st.indent_left == 0.0 && st.level == 0 {
                if let Some(l) = d.indent_left {
                    st.indent_left = l;
                }
            }
            if st.first_line == 0.0 {
                if let Some(f) = d.first_line {
                    st.first_line = f;
                }
            }
        }
    }
    // Space around normal paragraphs the file doesn't set: Normal's, else
    // the document defaults', else none (Word's default; Japanese
    // templates set no spacing at all).
    if let Some(x) = styles_xml {
        let (db, da) = default_spacing(x);
        let normal = normal_of(&raws).map(|r| &r.props);
        let before = normal.and_then(|n| n.space_before).or(db).unwrap_or(0.0);
        let after = normal.and_then(|n| n.space_after).or(da).unwrap_or(0.0);
        // Normal's line spacing beats the document defaults' (which apply
        // to paragraphs without one).
        if let Some(l) = normal.and_then(|n| n.line_spacing).filter(|&l| l > 0.0) {
            for p in paras.iter_mut().filter(|p| p.style.line_rule == apex_doc::LineRule::Multiple && p.style.line_spacing == 0.0) {
                p.style.line_spacing = l;
            }
        }
        let (db, da) = ParaStyle::default().default_spacing();
        // In cells unset spacing is none (see the table styles, which set
        // it), so there it's written out.
        for p in paras.iter_mut().filter(|p| p.style.kind == ParaKind::Normal) {
            if p.style.space_before.is_none() && (before != db || p.cell.is_some()) {
                p.style.space_before = Some(before);
            }
            if p.style.space_after.is_none() && (after != da || p.cell.is_some()) {
                p.style.space_after = Some(after);
            }
        }
        // Single spacing where the default is single too is the default
        // (so the file reads the same after saving it). The default of a
        // paragraph without spacing: Normal's when read, the document
        // defaults' when laid out, so both have to be single.
        let single = |l: f32| (l - 1.0).abs() < 0.005;
        let line = crate::docx_defaults::read(x).and_then(|d| d.line).filter(|l| l.0 == apex_doc::LineRule::Multiple).map(|l| l.1);
        if single(line.unwrap_or(1.0)) && normal.and_then(|n| n.line_spacing).is_none_or(single) {
            for p in paras.iter_mut().filter(|p| p.style.line_rule == apex_doc::LineRule::Multiple && p.style.line_spacing == 1.0) {
                if styles.custom(p.style.custom).is_none_or(|d| d.line_spacing.is_none()) && styles.builtin(p.style.kind).is_none_or(|d| d.line_spacing.is_none_or(single)) {
                    p.style.line_spacing = 0.0;
                }
            }
        }
    }
    // Normal as the file has it (what it doesn't set comes from the
    // document defaults).
    if let Some(n) = normal_of(&raws).filter(|r| r.name.eq_ignore_ascii_case("normal")) {
        let mut def = n.props.clone();
        def.font = def.font.filter(|f| !is_mono_font(f));
        if !def.is_empty() {
            // Widow control off in Normal is off in the styles based on it.
            if def.no_widow_control == Some(true) {
                for p in paras.iter_mut().filter(|p| styles.custom(p.style.custom).is_none_or(|d| d.no_widow_control.is_none())) {
                    p.style.no_widow_control = true;
                }
            }
            styles.defs.push(StyleDef { id: kind_id(ParaKind::Normal).into(), name: "Normal".into(), kind: ParaKind::Normal, builtin: true, ..def });
        }
    }
    // Changes to built-in styles, where they differ from ours.
    // Headings and titles without a size of their own have Normal's (ours
    // have one).
    let normal_size = normal_of(&raws).and_then(|r| r.props.size).or_else(|| styles_xml.and_then(crate::docx_defaults::read).and_then(|d| d.size));
    for (kind, id) in builtins {
        let mut chain = own_chain(&raws, &id);
        if !kind.inherits_size() && chain.size.is_none() {
            chain.size = normal_size;
        }
        let mut def = diff_from_defaults(&chain, kind);
        def.next = raws.get(&id).and_then(|r| r.next.as_deref()).map(|n| map_id(&raws, n)).filter(|n| &**n != "Normal");
        if def.is_empty() && def.next.is_none() {
            continue;
        }
        def.id = kind_id(kind).into();
        def.name = raws.get(&id).map(|r| r.name.as_str()).unwrap_or(&id).into();
        def.kind = kind;
        def.builtin = true;
        for p in paras.iter_mut().filter(|p| p.style.kind == kind && p.style.custom == 0) {
            fill_para(&mut p.style, &def);
        }
        styles.defs.push(def);
    }
    // Character styles linked to a paragraph style stand for it.
    let mut linked: HashMap<Arc<str>, Option<Arc<str>>> = HashMap::new();
    for p in paras.iter_mut() {
        for r in &mut p.runs {
            let Some(id) = r.attrs.char_style.clone() else { continue };
            let Some(para) = raws.get(&*id).filter(|r| r.char_style).and_then(|r| r.link.as_deref()).filter(|l| raws.get(*l).is_some_and(|r| !r.char_style)) else { continue };
            let target = linked.entry(id.clone()).or_insert_with(|| {
                let pid = map_id(&raws, para);
                match apex_doc::builtin_id(&pid) {
                    Some(ParaKind::Normal) => None,
                    Some(_) => Some(pid),
                    None => {
                        match styles.custom_number(&pid) {
                            Some(n) => styles.defs[n as usize - 1].linked = true,
                            None => {
                                let raw = &raws[para];
                                let mut def = own_chain(&raws, para);
                                def.font = def.font.filter(|f| !is_mono_font(f));
                                def.id = pid.clone();
                                def.name = if raw.name.is_empty() { pid.clone() } else { raw.name.as_str().into() };
                                def.linked = true;
                                styles.defs.push(def);
                            }
                        }
                        Some(pid)
                    }
                }
            });
            r.attrs.char_style = target.clone();
        }
    }
    // Character styles.
    let mut keep: HashMap<Arc<str>, bool> = HashMap::new();
    for p in paras.iter_mut() {
        for r in &mut p.runs {
            let Some(id) = r.attrs.char_style.clone() else { continue };
            if linked.values().any(|v| v.as_ref() == Some(&id)) {
                continue;
            }
            // Links have their own look.
            if r.attrs.link.is_some() {
                r.attrs.char_style = None;
                continue;
            }
            let ok = *keep.entry(id.clone()).or_insert_with(|| {
                let Some(raw) = raws.get(&*id).filter(|r| r.char_style) else { return false };
                let mut def = own_chain(&raws, &id);
                if def.font.as_deref().is_some_and(is_mono_font) || matches!(&*id, "Hyperlink" | "VerbatimChar" | "DefaultParagraphFont") {
                    return false;
                }
                def = StyleDef { font: def.font, size: def.size, bold: def.bold, italic: def.italic, underline: def.underline, color: def.color, asian: def.asian, ..Default::default() };
                if def.is_empty() {
                    return false;
                }
                def.id = id.clone();
                def.name = if raw.name.is_empty() { id.clone() } else { raw.name.as_str().into() };
                def.char_style = true;
                styles.defs.push(def);
                true
            });
            if !ok {
                r.attrs.char_style = None;
            }
        }
    }
    styles
}

// ---------------------------------------------------------------------------
// Writing

/// Whether the built-in style of `kind` is written from the document's
/// styles instead of our fixed definition.
pub(crate) fn overridden(doc: &Document, kind: ParaKind) -> bool {
    doc.styles.builtin(kind).is_some()
}

fn ppr(out: &mut String, d: &StyleDef, extra_first: &str, extra_last: &str) {
    let mut p = String::from(extra_first);
    match d.no_widow_control {
        Some(true) => p.push_str(r#"<w:widowControl w:val="0"/>"#),
        Some(false) => p.push_str("<w:widowControl/>"),
        None => {}
    }
    let deco = ParaStyle { shading: d.shading, borders: d.borders.unwrap_or_default(), tabs: d.tabs.unwrap_or_default(), ..Default::default() };
    crate::docx_layout::write_pbdr(&mut p, &deco);
    if let Some(c) = d.shading {
        let _ = write!(p, r#"<w:shd w:val="clear" w:color="auto" w:fill="{:06X}"/>"#, c & 0xFF_FFFF);
    }
    crate::docx_layout::write_tabs(&mut p, &deco);
    crate::docx_asian::write_ppr(&mut p, &d.asian_para);
    if d.space_before.is_some() || d.space_after.is_some() || d.line_spacing.is_some() {
        p.push_str("<w:spacing");
        if let Some(b) = d.space_before {
            let _ = write!(p, r#" w:before="{}""#, ((b * TWIP).round() as i32).max(0));
        }
        if let Some(a) = d.space_after {
            let _ = write!(p, r#" w:after="{}""#, ((a * TWIP).round() as i32).max(0));
        }
        if let Some(l) = d.line_spacing {
            let _ = write!(p, r#" w:line="{}" w:lineRule="auto""#, (l * 240.0).round() as i32);
        }
        p.push_str("/>");
    }
    if d.indent_left.is_some() || d.first_line.is_some() {
        p.push_str("<w:ind");
        if let Some(l) = d.indent_left {
            let _ = write!(p, r#" w:left="{}""#, (l * TWIP).round() as i32);
        }
        match d.first_line {
            Some(f) if f < 0.0 => {
                let _ = write!(p, r#" w:hanging="{}""#, (-f * TWIP).round() as i32);
            }
            Some(f) => {
                let _ = write!(p, r#" w:firstLine="{}""#, (f * TWIP).round() as i32);
            }
            None => {}
        }
        p.push_str("/>");
    }
    match d.contextual_spacing {
        Some(true) => p.push_str("<w:contextualSpacing/>"),
        Some(false) => p.push_str(r#"<w:contextualSpacing w:val="0"/>"#),
        None => {}
    }
    if let Some(a) = d.align {
        let v = match a {
            Align::Left => "left",
            Align::Center => "center",
            Align::Right => "right",
            Align::Justify => "both",
        };
        let _ = write!(p, r#"<w:jc w:val="{v}"/>"#);
    }
    p.push_str(extra_last);
    if !p.is_empty() {
        let _ = write!(out, "<w:pPr>{p}</w:pPr>");
    }
}

fn rpr(out: &mut String, d: &StyleDef) {
    let mut p = String::new();
    crate::docx_asian::write_rfonts(&mut p, d.font.as_deref(), d.asian.as_deref());
    for (v, on, off) in [(d.bold, "<w:b/><w:bCs/>", r#"<w:b w:val="0"/><w:bCs w:val="0"/>"#), (d.italic, "<w:i/><w:iCs/>", r#"<w:i w:val="0"/><w:iCs w:val="0"/>"#)] {
        match v {
            Some(true) => p.push_str(on),
            Some(false) => p.push_str(off),
            None => {}
        }
    }
    let fx = d.fx.as_deref().cloned().unwrap_or_default();
    crate::docx_fx::write_early(&mut p, &fx, false, false);
    if let Some(c) = d.color {
        let _ = write!(p, "<w:color {}/>", crate::theme_xml::word_color_attrs(c));
    }
    crate::docx_fx::write_late(&mut p, &fx);
    if let Some(s) = d.size {
        let _ = write!(p, r#"<w:sz w:val="{s}"/><w:szCs w:val="{s}"/>"#);
    }
    match d.underline {
        Some(true) => p.push_str(&crate::docx_fx::underline_xml(&fx)),
        Some(false) => p.push_str(r#"<w:u w:val="none"/>"#),
        None => {}
    }
    if let Some(a) = d.asian.as_deref().filter(|a| a.lang.is_some() || a.cs_lang.is_some()) {
        let _ = write!(p, "<w:lang{}/>", crate::docx_asian::lang_attrs(a));
    }
    if !p.is_empty() {
        let _ = write!(out, "<w:rPr>{p}</w:rPr>");
    }
}

/// Style definitions for styles.xml: changed built-in styles (complete,
/// with our defaults filled in), custom paragraph styles and character
/// styles.
pub(crate) fn styles_xml(doc: &Document) -> String {
    let mut s = String::new();
    for d in &doc.styles.defs {
        if d.builtin && !d.char_style {
            let kind = d.kind;
            let mut full = kind_defaults(kind);
            if kind == ParaKind::Normal {
                full = StyleDef::default();
            }
            merge(&mut full, d);
            let id = kind_id(kind);
            let name = match kind {
                ParaKind::Heading(n) => format!("heading {n}"),
                ParaKind::Normal => "Normal".into(),
                _ => id.to_string(),
            };
            let default = if kind == ParaKind::Normal { r#" w:default="1""# } else { "" };
            let _ = write!(s, r#"<w:style w:type="paragraph"{default} w:styleId="{id}"><w:name w:val="{name}"/>"#);
            if kind != ParaKind::Normal {
                s.push_str(r#"<w:basedOn w:val="Normal"/>"#);
            }
            match &d.next {
                Some(n) => {
                    let _ = write!(s, r#"<w:next w:val="{}"/>"#, esc(n));
                }
                None if kind != ParaKind::Normal => s.push_str(r#"<w:next w:val="Normal"/>"#),
                None => {}
            }
            link_xml(&mut s, doc, id);
            s.push_str("<w:qFormat/>");
            let (first, last) = match kind {
                ParaKind::Heading(n) => ("<w:keepNext/>".to_string(), format!(r#"<w:outlineLvl w:val="{}"/>"#, n.clamp(1, 9) - 1)),
                _ => (String::new(), String::new()),
            };
            ppr(&mut s, &full, &first, &last);
            rpr(&mut s, &full);
            s.push_str("</w:style>");
        } else if d.char_style {
            let _ = write!(s, r#"<w:style w:type="character" w:customStyle="1" w:styleId="{}"><w:name w:val="{}"/><w:basedOn w:val="DefaultParagraphFont"/>"#, esc(&d.id), esc(&d.name));
            match d.id.strip_suffix("Char").filter(|_| d.linked) {
                Some(p) => {
                    let _ = write!(s, r#"<w:link w:val="{}"/>"#, esc(p));
                }
                None => s.push_str("<w:qFormat/>"),
            }
            rpr(&mut s, d);
            s.push_str("</w:style>");
        } else {
            let _ = write!(s, r#"<w:style w:type="paragraph" w:customStyle="1" w:styleId="{}"><w:name w:val="{}"/><w:basedOn w:val="{}"/>"#, esc(&d.id), esc(&d.name), kind_id(d.kind));
            if let Some(n) = &d.next {
                let _ = write!(s, r#"<w:next w:val="{}"/>"#, esc(n));
            }
            link_xml(&mut s, doc, &d.id);
            s.push_str("<w:qFormat/>");
            let last = match d.kind {
                ParaKind::Heading(n) => format!(r#"<w:outlineLvl w:val="{}"/>"#, n.clamp(1, 9) - 1),
                _ => String::new(),
            };
            ppr(&mut s, d, "", &last);
            rpr(&mut s, d);
            s.push_str("</w:style>");
        }
    }
    s
}

/// `w:link` of a paragraph style whose character twin is written.
fn link_xml(out: &mut String, doc: &Document, id: &str) {
    let twin = format!("{id}Char");
    if doc.styles.by_id(&twin).is_some_and(|d| d.char_style && d.linked) {
        let _ = write!(out, r#"<w:link w:val="{}"/>"#, esc(&twin));
    }
}

/// A copy of the document where text in linked paragraph styles is in
/// their character style twins (`<id>Char`, as Word has them); `None` if
/// there is none.
pub(crate) fn linked_as_char(doc: &Document) -> Option<Document> {
    let is_para = |id: &str| !doc.styles.by_id(id).is_some_and(|d| d.char_style) && doc.styles.para_style_of(id).is_some();
    let used = doc.paragraphs.iter().flat_map(|p| &p.runs).filter_map(|r| r.attrs.char_style.as_deref()).any(is_para);
    if !used {
        return None;
    }
    let mut out = doc.clone();
    let mut twins: HashMap<Arc<str>, Arc<str>> = HashMap::new();
    for p in &mut out.paragraphs {
        for r in &mut p.runs {
            let Some(id) = r.attrs.char_style.clone().filter(|id| is_para(id)) else { continue };
            let twin = twins.entry(id.clone()).or_insert_with(|| format!("{id}Char").into()).clone();
            r.attrs.char_style = Some(twin);
        }
    }
    for (pid, twin) in twins {
        if out.styles.by_id(&twin).is_some() {
            continue;
        }
        let cf = doc.char_format(&ParaStyle::default(), &Attrs { char_style: Some(pid.clone()), ..Default::default() });
        let (kind, custom) = doc.styles.para_style_of(&pid).unwrap_or_default();
        let name = doc.styles.custom(custom).map_or_else(|| kind_id(kind).to_string(), |d| d.name.to_string());
        out.styles.defs.push(StyleDef { id: twin, name: format!("{name} Char").into(), char_style: true, linked: true, font: cf.font, size: Some(cf.size), bold: Some(cf.bold), italic: Some(cf.italic), underline: Some(cf.underline), color: cf.color, ..Default::default() });
    }
    Some(out)
}

/// The pStyle id of a paragraph with a custom style.
pub(crate) fn custom_id(doc: &Document, st: &ParaStyle) -> Option<String> {
    doc.styles.custom(st.custom).map(|d| d.id.to_string())
}

/// The rStyle of a run's character style.
pub(crate) fn run_style(a: &Attrs) -> Option<&str> {
    a.char_style.as_deref()
}
