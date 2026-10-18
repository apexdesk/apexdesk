//! Text of presentation objects in OpenDocument (`draw:text-box`, the
//! paragraphs of shapes and of notes): `text:p` and nested `text:list`s
//! read into `apex_slides::TextBody` (a list's depth is the paragraph's
//! level), and written back with everything resolved (sizes, fonts,
//! colors, bullets) so that the file looks the same without our masters.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{Align, Attrs, Field, FieldKind, LINE_BREAK, OBJECT, Paragraph};
use apex_slides::text::{Bullet, ParaProps, Spacing, TextBody};
use apex_slides::{Chain, Object};

use crate::drawingml::{Node, TEXT};
use crate::odf_shape::{Styles, hex_color};
use crate::parse_length_pt;
use crate::xml::esc;

// --- reading -------------------------------------------------------------------------------

/// A level of a list style: its marker, where the marker and the text
/// start (space before the label, label width) and the marker's size.
#[derive(Clone, Debug)]
pub(crate) struct ListLevel {
    pub bullet: Bullet,
    pub indent: Option<(f32, f32)>,
    pub size: Option<f32>,
    pub font: Option<Arc<str>>,
}

/// The levels of each list style (`text:list-style`).
pub(crate) type ListStyles = HashMap<String, Vec<ListLevel>>;

/// Reads the list styles of a styles or content part.
pub(crate) fn read_list_styles(root: &Node, out: &mut ListStyles) {
    let mut all = Vec::new();
    root.find_all("list-style", &mut all);
    for ls in all {
        let Some(name) = ls.attr("style:name") else { continue };
        let mut levels = vec![ListLevel { bullet: Bullet::Char('•'), indent: None, size: None, font: None }; 9];
        for k in &ls.kids {
            let l = k.attr("text:level").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1).clamp(1, 9) - 1;
            let lv = &mut levels[l];
            if let Some(p) = k.child("list-level-properties") {
                let len = |a: &str| p.attr(a).and_then(parse_length_pt).filter(|v| v.is_finite());
                let (sb, ml) = (len("text:space-before"), len("text:min-label-width"));
                let al = p.child("list-level-label-alignment");
                let al_len = |a: &str| al.and_then(|n| n.attr(a)).and_then(parse_length_pt).filter(|v| v.is_finite());
                lv.indent = match (al_len("fo:margin-left"), al_len("fo:text-indent")) {
                    (Some(m), i) => Some((m + i.unwrap_or(0.0), -i.unwrap_or(0.0))),
                    _ if sb.is_some() || ml.is_some() => Some((sb.unwrap_or(0.0), ml.unwrap_or(0.0))),
                    _ => None,
                };
            }
            if let Some(tp) = k.child("text-properties") {
                lv.size = tp.attr("fo:font-size").and_then(|v| v.strip_suffix('%').and_then(|v| v.trim().parse::<f32>().ok())).filter(|v| v.is_finite()).map(|v| (v / 100.0).clamp(0.25, 4.0));
                lv.font = tp.attr("fo:font-family").map(|f| f.trim().trim_matches(['\'', '"'])).filter(|f| !f.is_empty()).map(Arc::from);
            }
            if let Some(v) = k.attr("text:bullet-relative-size").and_then(|v| v.strip_suffix('%').and_then(|v| v.trim().parse::<f32>().ok())) {
                lv.size = v.is_finite().then(|| (v / 100.0).clamp(0.25, 4.0));
            }
            lv.bullet = match k.local() {
                "list-level-style-bullet" => Bullet::Char(k.attr("text:bullet-char").and_then(|c| c.chars().next()).unwrap_or('•')),
                "list-level-style-number" => {
                    let fmt = k.attr("style:num-format").unwrap_or("1");
                    let suffix = k.attr("style:num-suffix").unwrap_or(".");
                    let start = k.attr("text:start-value").and_then(|v| v.parse().ok()).unwrap_or(1);
                    if fmt.is_empty() { Bullet::None } else { Bullet::Auto(Arc::from(scheme_of(fmt, suffix)), start) }
                }
                _ => lv.bullet.clone(),
            };
        }
        out.insert(name.to_string(), levels);
    }
}

/// DrawingML's numbering scheme for an ODF number format and suffix.
fn scheme_of(fmt: &str, suffix: &str) -> String {
    let base = match fmt {
        "a" => "alphaLc",
        "A" => "alphaUc",
        "i" => "romanLc",
        "I" => "romanUc",
        _ => "arabic",
    };
    let end = match suffix {
        ")" => "ParenR",
        "" => "Plain",
        _ => "Period",
    };
    format!("{base}{end}")
}

/// ODF's number format and suffix for a DrawingML scheme.
fn format_of(scheme: &str) -> (&'static str, &'static str) {
    let fmt = if scheme.starts_with("alphaLc") {
        "a"
    } else if scheme.starts_with("alphaUc") {
        "A"
    } else if scheme.starts_with("romanLc") {
        "i"
    } else if scheme.starts_with("romanUc") {
        "I"
    } else {
        "1"
    };
    let suffix = if scheme.ends_with("ParenR") || scheme.ends_with("ParenBoth") {
        ")"
    } else if scheme.ends_with("Plain") {
        ""
    } else {
        "."
    };
    (fmt, suffix)
}

/// What reading text needs.
pub(crate) struct TextIn<'a> {
    pub st: &'a Styles,
    pub lists: &'a ListStyles,
}

/// Character settings of style `name` over `a`.
pub(crate) fn run_attrs(st: &Styles, name: Option<&str>, a: &mut Attrs) {
    let get = |k: &str| st.get(name, k);
    if let Some(w) = get("fo:font-weight") {
        a.bold = w == "bold" || w.parse::<u32>().is_ok_and(|v| v >= 600);
    }
    if let Some(v) = get("fo:font-style") {
        a.italic = v == "italic" || v == "oblique";
    }
    if let Some(v) = get("style:text-underline-style") {
        a.underline = v != "none";
    }
    if let Some(v) = get("style:text-line-through-style") {
        a.strike = v != "none";
    }
    if let Some(v) = get("fo:font-size").and_then(parse_length_pt) {
        a.size = Some((v * 2.0).round().clamp(1.0, 8000.0) as u16);
    }
    if let Some(c) = hex_color(get("fo:color")) {
        a.color = Some(c);
    }
    a.text_fx = crate::text_fx_xml::odf_read(a.text_fx.take(), get("fo:text-shadow"), get("style:text-outline"), get("style:font-relief"), a.color);
    if let Some(v) = get("style:text-position") {
        a.superscript = v.starts_with("super") || v.trim_start().starts_with(|c: char| c.is_ascii_digit() && c != '0');
        a.subscript = v.starts_with("sub") || v.trim_start().starts_with('-');
    }
    let transform = get("fo:text-transform");
    let variant = get("fo:font-variant");
    if transform.is_some() || variant.is_some() {
        let mut fx = a.fx.as_deref().cloned().unwrap_or_default();
        if let Some(t) = transform {
            fx.all_caps = t == "uppercase";
        }
        if let Some(v) = variant {
            fx.small_caps = v == "small-caps";
        }
        a.fx = if fx.is_default() { None } else { Some(Arc::new(fx)) };
    }
    if let Some(f) = get("fo:font-family").or_else(|| get("style:font-name")) {
        let f = f.trim().trim_matches(['\'', '"']);
        if !f.is_empty() {
            a.font = Some(Arc::from(f));
        }
    }
}

/// Paragraph settings of style `name`.
pub(crate) fn para_props(st: &Styles, name: Option<&str>, pp: &mut ParaProps) {
    let get = |k: &str| st.get(name, k);
    if let Some(a) = get("fo:text-align") {
        pp.align = Some(match a {
            "center" => Align::Center,
            "end" | "right" => Align::Right,
            "justify" => Align::Justify,
            _ => Align::Left,
        });
    }
    let len = |k: &str| get(k).and_then(parse_length_pt).filter(|v| v.is_finite());
    if let Some(v) = len("fo:margin-left") {
        pp.mar_l = Some(v);
    }
    if let Some(v) = len("fo:text-indent") {
        pp.indent = Some(v);
    }
    if let Some(v) = len("fo:margin-top") {
        pp.before = Some(Spacing::Pts(v));
    }
    if let Some(v) = len("fo:margin-bottom") {
        pp.after = Some(Spacing::Pts(v));
    }
    if let Some(v) = get("fo:line-height") {
        if let Some(p) = v.trim().strip_suffix('%').and_then(|p| p.trim().parse::<f32>().ok()) {
            pp.line = p.is_finite().then(|| Spacing::Pct((p / 100.0).clamp(0.0, 132.0)));
        } else if let Some(v) = parse_length_pt(v).filter(|v| v.is_finite()) {
            pp.line = Some(Spacing::Pts(v.clamp(0.0, 1584.0)));
        }
    }
}

/// The paragraphs in `body` (a text box, a shape or a notes frame).
/// `base` is the object's own character look; `bullets`: whether list
/// paragraphs of this text get their marker from the list style (else
/// from the master, as outline placeholders do).
pub(crate) fn read_body(body: &Node, cx: &TextIn, base: &Attrs, bullets: bool) -> TextBody {
    let mut t = TextBody::default();
    walk_block(body, cx, base, bullets, None, 0, &mut t);
    if t.paras.is_empty() {
        t.paras.push(Paragraph::new(""));
    }
    t
}

fn walk_block(n: &Node, cx: &TextIn, base: &Attrs, bullets: bool, list: Option<(&str, u8)>, depth: u8, t: &mut TextBody) {
    for k in &n.kids {
        match k.local() {
            "p" | "h" => read_para(k, cx, base, bullets, list, depth, t),
            "list" => {
                let style = k.attr("text:style-name").or(list.map(|l| l.0));
                for item in &k.kids {
                    if matches!(item.local(), "list-item" | "list-header") {
                        let header = item.local() == "list-header";
                        walk_block(item, cx, base, bullets && !header, Some((style.unwrap_or(""), depth + 1)), depth + 1, t);
                    }
                }
            }
            _ => {}
        }
    }
}

fn read_para(n: &Node, cx: &TextIn, base: &Attrs, bullets: bool, list: Option<(&str, u8)>, depth: u8, t: &mut TextBody) {
    let style = n.attr("text:style-name");
    let mut p = Paragraph::new("");
    p.style.level = depth.saturating_sub(1).min(8);
    let mut pp = ParaProps::default();
    para_props(cx.st, style, &mut pp);
    match list {
        Some((ls, d)) if bullets => {
            match cx.lists.get(ls).and_then(|v| v.get(d.saturating_sub(1) as usize)) {
                Some(lv) => {
                    pp.bullet = Some(lv.bullet.clone());
                    if let Some((sb, ml)) = lv.indent {
                        pp.mar_l = Some(pp.mar_l.unwrap_or(0.0) + sb + ml);
                        pp.indent = Some(pp.indent.unwrap_or(0.0) - ml);
                    }
                    if lv.size.is_some() {
                        pp.bullet_size = lv.size;
                    }
                    if let Some(f) = &lv.font {
                        pp.bullet_font = Some(Some(f.clone()));
                    }
                }
                None => pp.bullet = Some(Bullet::Char('•')),
            }
        }
        Some(_) => {}
        None => pp.bullet = Some(Bullet::None),
    }
    let mut a = base.clone();
    run_attrs(cx.st, style, &mut a);
    let mut prev_space = true;
    walk_runs(n, cx, &a, &mut p, &mut prev_space);
    if p.text.is_empty() {
        p.runs = vec![apex_doc::Run { len: 0, attrs: a }];
    }
    t.paras.push(p);
    t.set_props(t.paras.len() - 1, pp);
}

fn walk_runs(n: &Node, cx: &TextIn, a: &Attrs, p: &mut Paragraph, prev_space: &mut bool) {
    for k in &n.kids {
        if k.name == TEXT {
            let mut s = String::with_capacity(k.text.len());
            for ch in k.text.chars() {
                if ch.is_whitespace() {
                    if !*prev_space {
                        s.push(' ');
                    }
                    *prev_space = true;
                } else {
                    s.push(ch);
                    *prev_space = false;
                }
            }
            if !s.is_empty() {
                p.push(&s, a.clone());
            }
            continue;
        }
        match k.local() {
            "span" => {
                let mut b = a.clone();
                run_attrs(cx.st, k.attr("text:style-name"), &mut b);
                walk_runs(k, cx, &b, p, prev_space);
            }
            "a" => {
                let mut b = a.clone();
                b.link = k.attr("xlink:href").map(Arc::from);
                walk_runs(k, cx, &b, p, prev_space);
            }
            "s" => {
                let c = k.attr("text:c").and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(1).min(1000);
                p.push(&" ".repeat(c), a.clone());
                *prev_space = false;
            }
            "tab" => {
                p.push("\t", a.clone());
                *prev_space = false;
            }
            "line-break" => {
                p.push(&LINE_BREAK.to_string(), a.clone());
                *prev_space = true;
            }
            "page-number" | "date" | "time" => {
                let page = k.local() == "page-number";
                let mut f = Field::new(if page { FieldKind::Page } else { FieldKind::Date });
                f.cached = k.text.clone();
                // A fixed date shows the text saved; the others, today's.
                f.format = Some(match k.local() {
                    "page-number" => "slidenum".into(),
                    _ if k.attr("text:fixed") == Some("true") => "fixed".into(),
                    "time" => "datetime10".into(),
                    _ => "datetime1".into(),
                });
                let mut b = a.clone();
                b.field = Some(Arc::new(f));
                p.push(&OBJECT.to_string(), b);
                *prev_space = false;
            }
            // Other fields and marks: their shown text.
            _ => walk_runs(k, cx, a, p, prev_space),
        }
    }
}

// --- writing -------------------------------------------------------------------------------

fn pt(v: f32) -> String {
    let v = if v.is_finite() { v } else { 0.0 };
    format!("{}pt", (v * 100.0).round() / 100.0)
}

/// Automatic styles collected while writing, each written once.
#[derive(Default)]
pub(crate) struct AutoStyles {
    pub xml: String,
    names: HashMap<String, String>,
    /// Name prefix (content and styles parts use different ones).
    pub prefix: &'static str,
}

impl AutoStyles {
    pub(crate) fn new(prefix: &'static str) -> AutoStyles {
        AutoStyles { prefix, ..Default::default() }
    }

    /// The name of a style with this family and body, added if new.
    pub(crate) fn get(&mut self, family: &str, body: &str) -> String {
        let key = format!("{family}\u{0}{body}");
        if let Some(n) = self.names.get(&key) {
            return n.clone();
        }
        let tag = match family {
            "paragraph" => "P",
            "text" => "T",
            "graphic" => "gr",
            "drawing-page" => "dp",
            "list" => "L",
            _ => "S",
        };
        let name = format!("{}{tag}{}", self.prefix, self.names.len() + 1);
        if family == "list" {
            let _ = write!(self.xml, r#"<text:list-style style:name="{name}">{body}</text:list-style>"#);
        } else {
            let _ = write!(self.xml, r#"<style:style style:name="{name}" style:family="{family}">{body}</style:style>"#);
        }
        self.names.insert(key, name.clone());
        name
    }
}

fn hex(c: u32) -> String {
    format!("#{:06x}", c & 0xFFFFFF)
}

fn spacing(s: Spacing, size: f32) -> f32 {
    match s {
        Spacing::Pts(v) => v,
        Spacing::Pct(p) => p * size * 1.2,
    }
}

/// Writes the paragraphs of `t` (on object `o`, looked up through `ch`),
/// fully resolved.
pub(crate) fn write_body(out: &mut String, t: &TextBody, o: &Object, ch: &Chain, st: &mut AutoStyles) {
    let mut numbers: HashMap<u8, u32> = HashMap::new();
    for (i, p) in t.paras.iter().enumerate() {
        let (pp, first) = ch.para_run(o, t, i);
        let level = p.style.level.min(8);
        let mut props = String::new();
        let align = match pp.align.unwrap_or(Align::Left) {
            Align::Center => "center",
            Align::Right => "end",
            Align::Justify => "justify",
            Align::Left => "start",
        };
        let _ = write!(props, r#" fo:text-align="{align}""#);
        let listed = !matches!(pp.bullet, None | Some(Bullet::None));
        let (mar, ind) = (pp.mar_l.unwrap_or(0.0), pp.indent.unwrap_or(0.0));
        // In a list the list style places the label and the text.
        let (pm, pi) = if listed { (0.0, 0.0) } else { (mar, ind) };
        let _ = write!(props, r#" fo:margin-left="{}" fo:text-indent="{}""#, pt(pm), pt(pi));
        let _ = write!(props, r#" fo:margin-top="{}" fo:margin-bottom="{}""#, pt(pp.before.map_or(0.0, |s| spacing(s, first.size))), pt(pp.after.map_or(0.0, |s| spacing(s, first.size))));
        match pp.line {
            Some(Spacing::Pct(v)) => {
                let _ = write!(props, r#" fo:line-height="{}%""#, (v * 100.0).round());
            }
            Some(Spacing::Pts(v)) => {
                let _ = write!(props, r#" fo:line-height="{}""#, pt(v));
            }
            None => {}
        }
        let pstyle = st.get("paragraph", &format!("<style:paragraph-properties{props}/>{}", text_props(&first, None)));
        // A bulleted paragraph goes in a list as deep as its level.
        let bullet = pp.bullet.clone().unwrap_or(Bullet::None);
        // The label sits at the first line's indent, the text at the margin.
        let label = format!(r#"<style:list-level-properties text:space-before="{}" text:min-label-width="{}"/>"#, pt((mar + ind).max(0.0)), pt((-ind).max(0.0)));
        let bsize = format!("{}%", (pp.bullet_size.unwrap_or(1.0) * 100.0).round());
        let list = match &bullet {
            Bullet::None => None,
            Bullet::Char(c) => {
                let family = pp.bullet_font.clone().flatten().map_or(String::new(), |f| format!(r#" fo:font-family="'{}'""#, esc(&f.replace('\'', ""))));
                let color = match &pp.bullet_color {
                    Some(Some(c)) => format!(r#" fo:color="{}""#, hex(ch.color(c).rgb)),
                    _ => r#" style:use-window-font-color="true""#.to_string(),
                };
                let font = format!(r#"<style:text-properties{family}{color} fo:font-size="{bsize}"/>"#);
                Some(st.get("list", &format!(r#"<text:list-level-style-bullet text:level="{}" text:bullet-char="{}">{label}{font}</text:list-level-style-bullet>"#, level + 1, esc(&c.to_string()))))
            }
            Bullet::Auto(scheme, start) => {
                let (fmt, suffix) = format_of(scheme);
                let n = numbers.entry(level).or_insert(0);
                *n += 1;
                let _ = start;
                Some(st.get("list", &format!(r#"<text:list-level-style-number text:level="{}" style:num-format="{fmt}" style:num-suffix="{suffix}" text:start-value="{}">{label}<style:text-properties fo:font-size="{bsize}"/></text:list-level-style-number>"#, level + 1, start.saturating_add(*n - 1))))
            }
        };
        if !matches!(bullet, Bullet::Auto(..)) {
            numbers.remove(&level);
        }
        if let Some(l) = &list {
            let _ = write!(out, r#"<text:list text:style-name="{l}">"#);
            for _ in 0..level {
                out.push_str("<text:list-item><text:list>");
            }
            out.push_str("<text:list-item>");
        }
        let _ = write!(out, r#"<text:p text:style-name="{pstyle}">"#);
        let mut at = 0;
        for r in &p.runs {
            let end = (at + r.len).min(p.text.len());
            let text = p.text.get(at..end).unwrap_or("");
            at = end;
            if text.is_empty() {
                continue;
            }
            let f = ch.run(o, &pp, &r.attrs);
            let ts = st.get("text", &text_props(&f, r.attrs.text_fx.as_deref()));
            let _ = write!(out, r#"<text:span text:style-name="{ts}">"#);
            if let Some(link) = &r.attrs.link {
                let _ = write!(out, r#"<text:a xlink:type="simple" xlink:href="{}">"#, esc(link));
            }
            match r.attrs.field.as_deref() {
                Some(fl) if fl.kind == FieldKind::Page => out.push_str("<text:page-number>&lt;number&gt;</text:page-number>"),
                Some(fl) if fl.kind == FieldKind::Date => {
                    let _ = write!(out, "<text:date>{}</text:date>", esc(&fl.cached));
                }
                _ => crate::odt::write_text(out, &text.replace(OBJECT, "")),
            }
            if r.attrs.link.is_some() {
                out.push_str("</text:a>");
            }
            out.push_str("</text:span>");
        }
        out.push_str("</text:p>");
        if list.is_some() {
            out.push_str("</text:list-item>");
            for _ in 0..level {
                out.push_str("</text:list></text:list-item>");
            }
            out.push_str("</text:list>");
        }
    }
}

/// `style:text-properties` of a resolved run.
fn text_props(f: &apex_slides::resolve::RunFmt, fx: Option<&apex_doc::TextFx>) -> String {
    let mut p = format!(r#" fo:font-size="{}" fo:color="{}""#, pt(f.size), hex(f.color.rgb));
    if let Some(fx) = fx {
        p.push_str(&crate::text_fx_xml::odf_attrs(fx, true));
    }
    let font = f.font.replace('\'', "");
    let family = esc(&font);
    let _ = write!(p, r#" fo:font-family="'{family}'" style:font-family-asian="'{family}'" style:font-family-complex="'{family}'""#);
    let _ = write!(p, r#" style:font-size-asian="{0}" style:font-size-complex="{0}""#, pt(f.size));
    p.push_str(if f.bold { r#" fo:font-weight="bold" style:font-weight-asian="bold" style:font-weight-complex="bold""# } else { r#" fo:font-weight="normal" style:font-weight-asian="normal" style:font-weight-complex="normal""# });
    p.push_str(if f.italic { r#" fo:font-style="italic" style:font-style-asian="italic" style:font-style-complex="italic""# } else { r#" fo:font-style="normal" style:font-style-asian="normal" style:font-style-complex="normal""# });
    if f.underline {
        p.push_str(r#" style:text-underline-style="solid" style:text-underline-width="auto" style:text-underline-color="font-color""#);
    }
    if f.strike {
        p.push_str(r#" style:text-line-through-style="solid""#);
    }
    if f.baseline > 0.0 {
        p.push_str(r#" style:text-position="super 58%""#);
    } else if f.baseline < 0.0 {
        p.push_str(r#" style:text-position="sub 58%""#);
    }
    match f.caps {
        apex_slides::text::Caps::All => p.push_str(r#" fo:text-transform="uppercase""#),
        apex_slides::text::Caps::Small => p.push_str(r#" fo:font-variant="small-caps""#),
        apex_slides::text::Caps::None => {}
    }
    if f.spacing != 0.0 {
        let _ = write!(p, r#" fo:letter-spacing="{}""#, pt(f.spacing));
    }
    if let Some(h) = f.highlight {
        let _ = write!(p, r#" fo:background-color="{}""#, hex(h));
    }
    format!("<style:text-properties{p}/>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemes_round_trip() {
        for s in ["arabicPeriod", "alphaLcParenR", "romanUcPeriod", "arabicPlain"] {
            let (f, x) = format_of(s);
            assert_eq!(scheme_of(f, x), s);
        }
    }

    #[test]
    fn lists_give_levels_and_bullets() {
        let xml = r#"<draw:text-box><text:p>Top</text:p><text:list text:style-name="L1"><text:list-item><text:p>One</text:p><text:list><text:list-item><text:p>Two <text:span text:style-name="T1">bold</text:span></text:p></text:list-item></text:list></text:list-item></text:list></draw:text-box>"#;
        let root = crate::drawingml::read_fragment(xml);
        let body = &root.kids[0];
        let mut st = Styles::default();
        st.read(r#"<office:automatic-styles><style:style style:name="T1" style:family="text"><style:text-properties fo:font-weight="bold"/></style:style><text:list-style style:name="L1"><text:list-level-style-bullet text:level="1" text:bullet-char="-"/><text:list-level-style-number text:level="2" style:num-format="a" style:num-suffix=")"/></text:list-style></office:automatic-styles>"#);
        let mut lists = ListStyles::new();
        read_list_styles(&crate::drawingml::read_fragment(r#"<x><text:list-style style:name="L1"><text:list-level-style-bullet text:level="1" text:bullet-char="-"/><text:list-level-style-number text:level="2" style:num-format="a" style:num-suffix=")"/></text:list-style></x>"#), &mut lists);
        let t = read_body(body, &TextIn { st: &st, lists: &lists }, &Attrs::default(), true);
        assert_eq!(t.text(), "Top\nOne\nTwo bold");
        assert_eq!(t.paras.iter().map(|p| p.style.level).collect::<Vec<_>>(), vec![0, 0, 1]);
        assert_eq!(t.para_props(0).and_then(|p| p.bullet.clone()), Some(Bullet::None));
        assert_eq!(t.para_props(1).and_then(|p| p.bullet.clone()), Some(Bullet::Char('-')));
        assert_eq!(t.para_props(2).and_then(|p| p.bullet.clone()), Some(Bullet::Auto(Arc::from("alphaLcParenR"), 1)));
        assert!(t.paras[2].runs.last().unwrap().attrs.bold);
    }
}
