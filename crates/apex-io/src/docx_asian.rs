//! East Asian typography in docx: the Asian and complex script fonts and
//! languages of runs (`w:rFonts`, `w:lang`), emphasis marks, combined
//! characters and fitted text, phonetic guides (`w:ruby`), the Asian line
//! breaking of paragraphs (`w:kinsoku` and the like), the document grid
//! (`w:docGrid`) and text direction of sections, and the settings Word
//! keeps for Asian text (`w:characterSpacingControl`, custom line
//! breaking lists).

use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{AsianAttrs, AsianPara, Combine, Emphasis, FontAlign, GridKind, PageSetup, Ruby, TextFlow};

use crate::xml::{Ev, Reader, Tag, esc, local};

const TWIP: f32 = 20.0;

fn on(t: &Tag) -> bool {
    t.attr("w:val").is_none_or(|v| !matches!(v.as_ref(), "0" | "false" | "off" | "none"))
}

/// The token of a theme font name (`minorEastAsia`; the Latin ones,
/// `minorHAnsi`, stand for the theme's Latin font).
fn theme_token(v: &str) -> Option<&'static str> {
    use apex_doc::theme::{EA_MAJOR_TOKEN, EA_MINOR_TOKEN, MAJOR_TOKEN, MINOR_TOKEN};
    let major = v.starts_with("major");
    if !major && !v.starts_with("minor") {
        return None;
    }
    Some(match (major, v.ends_with("EastAsia")) {
        (true, true) => EA_MAJOR_TOKEN,
        (false, true) => EA_MINOR_TOKEN,
        (true, false) => MAJOR_TOKEN,
        (false, false) => MINOR_TOKEN,
    })
}

// --- runs ------------------------------------------------------------------------------

/// Reads the Asian part of a run property element into `a`; returns
/// whether the element is only Asian (nothing else to read in it).
pub(crate) fn read_rpr(t: &Tag, a: &mut AsianAttrs) -> bool {
    match t.local() {
        "rFonts" => {
            let ea = t.attr("w:eastAsiaTheme").and_then(|v| theme_token(&v).map(Arc::from)).or_else(|| t.attr("w:eastAsia").filter(|v| !v.is_empty()).map(|v| Arc::from(v.as_ref())));
            if ea.is_some() {
                a.font = ea;
            }
            // The Latin font repeated for complex scripts isn't one of their own.
            let latin = t.attr("w:ascii").or_else(|| t.attr("w:hAnsi"));
            if let Some(cs) = t.attr("w:cs").filter(|v| !v.is_empty() && Some(v) != latin.as_ref()) {
                a.cs_font = Some(Arc::from(cs.as_ref()));
            } else if let Some(th) = t.attr("w:cstheme") {
                let major = th.starts_with("major");
                a.cs_font = Some(Arc::from(if major { apex_doc::theme::CS_MAJOR_TOKEN } else { apex_doc::theme::CS_MINOR_TOKEN }));
            }
            if let Some(h) = t.attr("w:hint") {
                a.hint = h == "eastAsia";
            }
            false
        }
        "lang" => {
            if let Some(l) = t.attr("w:eastAsia").filter(|v| !v.is_empty()) {
                a.lang = Some(Arc::from(l.as_ref()));
            }
            if let Some(l) = t.attr("w:bidi").filter(|v| !v.is_empty()) {
                a.cs_lang = Some(Arc::from(l.as_ref()));
            }
            false
        }
        "rtl" => {
            a.rtl = on(t);
            true
        }
        "szCs" => {
            a.cs_size = t.attr("w:val").and_then(|v| v.parse().ok());
            false
        }
        "em" => {
            a.emphasis = match t.attr("w:val").as_deref() {
                Some("dot") => Some(Emphasis::Dot),
                Some("comma") => Some(Emphasis::Comma),
                Some("circle") => Some(Emphasis::Circle),
                Some("underDot") => Some(Emphasis::UnderDot),
                _ => None,
            };
            true
        }
        "eastAsianLayout" => {
            let c = Combine {
                id: t.attr("w:id").and_then(|v| v.parse().ok()),
                lines: t.attr("w:combine").is_some_and(|v| !matches!(v.as_ref(), "0" | "false" | "off")),
                letters: false,
                brackets: t.attr("w:combineBrackets").filter(|v| v != "none").map(|v| Arc::from(v.as_ref())),
                vert: t.attr("w:vert").is_some_and(|v| !matches!(v.as_ref(), "0" | "false" | "off")),
                vert_compress: t.attr("w:vertCompress").is_some_and(|v| !matches!(v.as_ref(), "0" | "false" | "off")),
            };
            a.combine = Some(c);
            true
        }
        "fitText" => {
            a.fit_text = t.attr("w:val").and_then(|v| v.parse().ok()).map(|w| (w, t.attr("w:id").and_then(|v| v.parse().ok()).unwrap_or(0)));
            true
        }
        _ => false,
    }
}

/// `w:rFonts` attributes for the Asian and complex script fonts (after
/// the Latin ones).
fn rfonts_attrs(a: &AsianAttrs) -> String {
    let mut s = String::new();
    match a.font.as_deref() {
        Some(f @ (apex_doc::theme::EA_MAJOR_TOKEN | apex_doc::theme::EA_MINOR_TOKEN)) => {
            let _ = write!(s, r#" w:eastAsiaTheme="{}EastAsia""#, if f == apex_doc::theme::EA_MAJOR_TOKEN { "major" } else { "minor" });
        }
        Some(f @ (apex_doc::theme::MAJOR_TOKEN | apex_doc::theme::MINOR_TOKEN)) => {
            let _ = write!(s, r#" w:eastAsiaTheme="{}HAnsi""#, if f == apex_doc::theme::MAJOR_TOKEN { "major" } else { "minor" });
        }
        Some(f) => {
            let _ = write!(s, r#" w:eastAsia="{}""#, esc(f));
        }
        None => {}
    }
    match a.cs_font.as_deref() {
        Some(f @ (apex_doc::theme::CS_MAJOR_TOKEN | apex_doc::theme::CS_MINOR_TOKEN)) => {
            let _ = write!(s, r#" w:cstheme="{}Bidi""#, if f == apex_doc::theme::CS_MAJOR_TOKEN { "major" } else { "minor" });
        }
        Some(f) => {
            let _ = write!(s, r#" w:cs="{}""#, esc(f));
        }
        None => {}
    }
    if a.hint {
        s.push_str(r#" w:hint="eastAsia""#);
    }
    s
}

/// A whole `w:rFonts`: the Latin font (a name or theme token) and the
/// Asian and complex script ones.
pub(crate) fn write_rfonts(out: &mut String, latin: Option<&str>, a: Option<&AsianAttrs>) {
    let mut l = latin.map(crate::theme_xml::word_font_attrs).unwrap_or_default();
    if a.is_some_and(|a| a.cs_font.is_some()) {
        // The complex script font is the text's own.
        if let Some(i) = l.find(" w:cs=\"") {
            let end = l[i + 7..].find('"').map_or(l.len(), |e| i + 8 + e);
            l.replace_range(i..end, "");
        }
    }
    let extra = a.map(rfonts_attrs).unwrap_or_default();
    if !l.is_empty() || !extra.is_empty() {
        let sep = if l.is_empty() { "" } else { " " };
        let _ = write!(out, "<w:rFonts{sep}{l}{extra}/>");
    }
}

/// `w:lang` attributes for the Asian and complex script languages.
pub(crate) fn lang_attrs(a: &AsianAttrs) -> String {
    let mut s = String::new();
    if let Some(l) = &a.lang {
        let _ = write!(s, r#" w:eastAsia="{}""#, esc(l));
    }
    if let Some(l) = &a.cs_lang {
        let _ = write!(s, r#" w:bidi="{}""#, esc(l));
    }
    s
}

/// `w:fitText`, which goes after the shading.
pub(crate) fn write_fit(out: &mut String, a: &AsianAttrs) {
    if let Some((w, id)) = a.fit_text {
        let _ = write!(out, r#"<w:fitText w:val="{w}" w:id="{id}"/>"#);
    }
}

/// `w:rtl` and `w:em`, which go before the language.
pub(crate) fn write_em(out: &mut String, a: &AsianAttrs) {
    if a.rtl {
        out.push_str("<w:rtl/>");
    }
    if let Some(e) = a.emphasis {
        let v = match e {
            Emphasis::Dot | Emphasis::Disc => "dot",
            Emphasis::Comma => "comma",
            Emphasis::Circle => "circle",
            Emphasis::UnderDot => "underDot",
        };
        let _ = write!(out, r#"<w:em w:val="{v}"/>"#);
    }
}

/// `w:eastAsianLayout`, which goes after the language.
pub(crate) fn write_layout(out: &mut String, a: &AsianAttrs) {
    let Some(c) = &a.combine else { return };
    out.push_str("<w:eastAsianLayout");
    if let Some(id) = c.id {
        let _ = write!(out, r#" w:id="{id}""#);
    }
    if c.lines || c.letters {
        out.push_str(r#" w:combine="1""#);
    }
    if let Some(b) = c.brackets.as_deref() {
        let b = match b {
            "round" | "square" | "angle" | "curly" => b,
            _ if b.starts_with('[') => "square",
            _ if b.starts_with('<') || b.starts_with('〈') => "angle",
            _ if b.starts_with('{') => "curly",
            _ => "round",
        };
        let _ = write!(out, r#" w:combineBrackets="{b}""#);
    }
    if c.vert {
        out.push_str(r#" w:vert="1""#);
    }
    if c.vert_compress {
        out.push_str(r#" w:vertCompress="1""#);
    }
    out.push_str("/>");
}

// --- phonetic guides -----------------------------------------------------------------

/// Reads `w:rubyPr` (the reader is past its start tag).
pub(crate) fn read_ruby_pr(r: &mut Reader, ruby: &mut Ruby) {
    let mut depth = 1;
    loop {
        let ev = r.next();
        match ev {
            Ev::Eof => return,
            Ev::End(_) => {
                depth -= 1;
                if depth == 0 {
                    return;
                }
            }
            Ev::Start(t) | Ev::Empty(t) => {
                let val = t.attr("w:val");
                match t.local() {
                    "rubyAlign" => ruby.align = val.map(|v| Arc::from(v.as_ref())),
                    "hps" => ruby.size = val.and_then(|v| v.parse().ok()),
                    "hpsRaise" => ruby.raise = val.and_then(|v| v.parse().ok()),
                    "hpsBaseText" => ruby.base_size = val.and_then(|v| v.parse().ok()),
                    "lid" => ruby.lang = val.map(|v| Arc::from(v.as_ref())),
                    _ => {}
                }
                if matches!(ev, Ev::Start(_)) {
                    depth += 1;
                }
            }
            _ => {}
        }
    }
}

/// Reads `w:rt` (the reader is past its start tag): the guide text and
/// its run properties.
pub(crate) fn read_rt(r: &mut Reader, ruby: &mut Ruby) {
    let mut text = String::new();
    let mut depth = 1;
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::End(_) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Ev::Start(t) => match t.local() {
                "rPr" if ruby.docx_rpr.is_none() => ruby.docx_rpr = Some(Arc::from(format!("<w:rPr>{}", r.raw_element()).as_str())),
                "t" => text.push_str(&r.text()),
                _ => depth += 1,
            },
            _ => {}
        }
    }
    ruby.text = Arc::from(text.as_str());
}

/// Opens a `w:ruby` with its guide; the base runs follow, then
/// [`RUBY_END`].
pub(crate) fn write_ruby_start(out: &mut String, ruby: &Ruby, base_hp: u16) {
    let size = ruby.size.unwrap_or((base_hp / 2).max(2));
    let _ = write!(
        out,
        r#"<w:r><w:ruby><w:rubyPr><w:rubyAlign w:val="{}"/><w:hps w:val="{size}"/><w:hpsRaise w:val="{}"/><w:hpsBaseText w:val="{}"/><w:lid w:val="{}"/></w:rubyPr><w:rt><w:r>"#,
        esc(ruby.align.as_deref().unwrap_or("distributeSpace")),
        ruby.raise.unwrap_or(base_hp as i32 - 2),
        ruby.base_size.unwrap_or(base_hp),
        esc(ruby.lang.as_deref().unwrap_or("ja-JP"))
    );
    match &ruby.docx_rpr {
        Some(rpr) => out.push_str(rpr),
        None => {
            let _ = write!(out, r#"<w:rPr><w:sz w:val="{size}"/><w:szCs w:val="{size}"/></w:rPr>"#);
        }
    }
    let _ = write!(out, r#"<w:t xml:space="preserve">{}</w:t></w:r></w:rt><w:rubyBase>"#, esc(&ruby.text));
}

pub(crate) const RUBY_END: &str = "</w:rubyBase></w:ruby></w:r>";

// --- paragraphs ------------------------------------------------------------------------

/// Reads a paragraph property element of Asian typography; returns
/// whether it was one.
pub(crate) fn read_ppr(t: &Tag, p: &mut AsianPara) -> bool {
    let v = Some(on(t));
    match t.local() {
        "kinsoku" => p.kinsoku = v,
        "wordWrap" => p.word_wrap = v,
        "overflowPunct" => p.overflow_punct = v,
        "topLinePunct" => p.top_line_punct = v,
        "autoSpaceDE" => p.auto_space_de = v,
        "autoSpaceDN" => p.auto_space_dn = v,
        "snapToGrid" => p.snap_to_grid = v,
        "textAlignment" => {
            p.font_align = match t.attr("w:val").as_deref() {
                Some("top") => Some(FontAlign::Top),
                Some("center") => Some(FontAlign::Center),
                Some("baseline") => Some(FontAlign::Baseline),
                Some("bottom") => Some(FontAlign::Bottom),
                Some("auto") => Some(FontAlign::Auto),
                _ => None,
            }
        }
        _ => return false,
    }
    true
}

fn flag(out: &mut String, name: &str, v: Option<bool>) {
    match v {
        Some(true) => {
            let _ = write!(out, "<w:{name}/>");
        }
        Some(false) => {
            let _ = write!(out, r#"<w:{name} w:val="0"/>"#);
        }
        None => {}
    }
}

/// The line breaking elements (after the tabs, before the spacing).
pub(crate) fn write_ppr(out: &mut String, p: &AsianPara) {
    flag(out, "kinsoku", p.kinsoku);
    flag(out, "wordWrap", p.word_wrap);
    flag(out, "overflowPunct", p.overflow_punct);
    flag(out, "topLinePunct", p.top_line_punct);
    flag(out, "autoSpaceDE", p.auto_space_de);
    flag(out, "autoSpaceDN", p.auto_space_dn);
    flag(out, "snapToGrid", p.snap_to_grid);
}

/// `w:textAlignment` (after the alignment).
pub(crate) fn write_font_align(out: &mut String, p: &AsianPara) {
    if let Some(a) = p.font_align {
        let v = match a {
            FontAlign::Auto => "auto",
            FontAlign::Baseline => "baseline",
            FontAlign::Top => "top",
            FontAlign::Center => "center",
            FontAlign::Bottom => "bottom",
        };
        let _ = write!(out, r#"<w:textAlignment w:val="{v}"/>"#);
    }
}

// --- sections ------------------------------------------------------------------------

/// Reads `w:docGrid` and `w:textDirection` of a section.
pub(crate) fn read_sect(t: &Tag, page: &mut PageSetup) {
    match t.local() {
        "docGrid" => {
            let g = &mut page.grid;
            g.kind = match t.attr("w:type").as_deref() {
                Some("lines") => GridKind::Lines,
                Some("linesAndChars") => GridKind::LinesAndChars,
                Some("snapToChars") => GridKind::SnapToChars,
                _ => GridKind::None,
            };
            g.line_pitch = t.attr("w:linePitch").and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite() && *v > 0.0).map_or(0.0, |v| v / TWIP);
            g.char_space = t.attr("w:charSpace").and_then(|v| v.parse().ok()).unwrap_or(0);
        }
        "textDirection" => page.text_flow = t.attr("w:val").and_then(|v| TextFlow::from_word(&v)),
        _ => {}
    }
}

/// `w:textDirection` and `w:docGrid` of a section (after `w:titlePg`).
pub(crate) fn write_sect(out: &mut String, page: &PageSetup) {
    if let Some(f) = page.text_flow {
        let _ = write!(out, r#"<w:textDirection w:val="{}"/>"#, f.word());
    }
    let g = &page.grid;
    let ty = match g.kind {
        GridKind::None => "",
        GridKind::Lines => "lines",
        GridKind::LinesAndChars => "linesAndChars",
        GridKind::SnapToChars => "snapToChars",
    };
    if ty.is_empty() && g.line_pitch <= 0.0 {
        return;
    }
    out.push_str("<w:docGrid");
    if !ty.is_empty() {
        let _ = write!(out, r#" w:type="{ty}""#);
    }
    if g.line_pitch > 0.0 {
        let _ = write!(out, r#" w:linePitch="{}""#, (g.line_pitch * TWIP).round() as i32);
    }
    if g.char_space != 0 {
        let _ = write!(out, r#" w:charSpace="{}""#, g.char_space);
    }
    out.push_str("/>");
}

// --- defaults --------------------------------------------------------------------------

/// The Asian part of `w:docDefaults` in styles.xml.
pub(crate) fn read_defaults(styles: &str) -> apex_doc::AsianDefaults {
    let mut d = apex_doc::AsianDefaults::default();
    let mut text = AsianAttrs::default();
    let mut r = Reader::new(styles);
    let mut in_def = false;
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) if t.local() == "docDefaults" => in_def = true,
            Ev::End(n) if local(n) == "docDefaults" => break,
            Ev::Start(t) | Ev::Empty(t) if in_def => {
                if !read_ppr(&t, &mut d.para) {
                    read_rpr(&t, &mut text);
                }
            }
            _ => {}
        }
    }
    text.cs_size = None;
    d.text = (!text.is_empty()).then(|| Arc::new(text));
    d
}

// --- settings --------------------------------------------------------------------------

/// Settings elements for Asian text kept as they are.
const KEPT_SETTINGS: [&str; 5] = ["noPunctuationKerning", "characterSpacingControl", "strictFirstAndLastChars", "noLineBreaksAfter", "noLineBreaksBefore"];

/// Slot of the kept settings in `KeptParts::xml`.
pub(crate) const SETTINGS_SLOT: &str = "docx:settings-asian";

/// The Asian settings of `settings.xml`, to keep.
pub(crate) fn read_settings(xml: &str) -> Option<String> {
    let mut out = String::new();
    let mut r = Reader::new(xml);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Empty(t) if KEPT_SETTINGS.contains(&t.local()) => {
                let _ = write!(out, "<{}{}/>", t.name, t.raw_attrs());
            }
            Ev::Start(t) if KEPT_SETTINGS.contains(&t.local()) => {
                let name = t.name;
                let _ = write!(out, "<{name}{}>{}", t.raw_attrs(), r.raw_element());
            }
            Ev::End(n) if local(n) == "settings" => break,
            _ => {}
        }
    }
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kept_settings() {
        let xml = r#"<w:settings xmlns:w="w"><w:zoom w:percent="100"/><w:characterSpacingControl w:val="compressPunctuation"/><w:noLineBreaksAfter w:lang="ja-JP" w:val="$([\{"/></w:settings>"#;
        let s = read_settings(xml).unwrap();
        assert!(s.contains(r#"<w:characterSpacingControl w:val="compressPunctuation"/>"#) && s.contains("noLineBreaksAfter"));
        assert!(!s.contains("zoom"));
    }
}
