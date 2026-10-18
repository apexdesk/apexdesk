//! DrawingML theme parts (`xl/theme/theme1.xml`, `word/theme/theme1.xml`):
//! the color scheme and the major/minor latin fonts.

use std::fmt::Write;
use std::sync::Arc;

use apex_doc::theme::{SLOT_NAMES, Theme};

use crate::xml::{Ev, Reader, esc};

pub const CONTENT_TYPE: &str = "application/vnd.openxmlformats-officedocument.theme+xml";
pub const REL_TYPE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme";

/// Reads a theme part; missing pieces keep the Office defaults.
pub fn read_theme(xml: &str) -> Theme {
    let mut t = Theme::default();
    let mut r = Reader::new(xml);
    let mut slot: Option<usize> = None;
    // 1 major font, 2 minor font.
    let mut font = 0;
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(tag) | Ev::Empty(tag) => match tag.local() {
                "theme" => {
                    if let Some(n) = tag.attr("name") {
                        t.name = Arc::from(n.as_ref());
                    }
                }
                "clrScheme" => {
                    if let Some(n) = tag.attr("name") {
                        t.colors.name = Arc::from(n.as_ref());
                    }
                }
                "fontScheme" => {
                    if let Some(n) = tag.attr("name") {
                        t.fonts.name = Arc::from(n.as_ref());
                    }
                }
                "majorFont" => font = 1,
                "minorFont" => font = 2,
                "latin" if font > 0 => {
                    if let Some(f) = tag.attr("typeface").filter(|f| !f.is_empty()) {
                        let f: Arc<str> = Arc::from(f.as_ref());
                        if font == 1 {
                            t.fonts.major = f;
                        } else {
                            t.fonts.minor = f;
                        }
                    }
                }
                "ea" | "cs" | "font" if font > 0 => {
                    let key = if tag.local() == "font" { tag.attr("script").unwrap_or_default().to_string() } else { tag.local().to_string() };
                    let face = tag.attr("typeface").unwrap_or_default();
                    if !key.is_empty() && !(face.is_empty() && key.len() == 2) {
                        t.fonts.scripts.push((font == 1, Arc::from(key.as_str()), Arc::from(face.as_ref())));
                    }
                }
                "srgbClr" | "sysClr" => {
                    if let Some(i) = slot {
                        let v = if tag.local() == "sysClr" { tag.attr("lastClr") } else { tag.attr("val") };
                        if let Some(c) = v.and_then(|v| u32::from_str_radix(v.trim(), 16).ok()) {
                            t.colors.colors[i] = c & 0xFF_FFFF;
                        }
                    }
                }
                name => {
                    if let Some(i) = SLOT_NAMES.iter().position(|s| *s == name) {
                        slot = Some(i);
                    }
                }
            },
            Ev::End(n) => match crate::xml::local(n) {
                "majorFont" | "minorFont" => font = 0,
                name if SLOT_NAMES.contains(&name) => slot = None,
                _ => {}
            },
            _ => {}
        }
    }
    t
}

/// A complete theme part (Office needs the format scheme too).
pub fn write_theme(t: &Theme) -> String {
    let mut s = String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#);
    s.push('\n');
    let _ = write!(s, r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="{}"><a:themeElements>"#, esc(&t.name));
    let _ = write!(s, r#"<a:clrScheme name="{}">"#, esc(&t.colors.name));
    for (i, name) in SLOT_NAMES.iter().enumerate() {
        let c = t.color(i as u8);
        match i {
            0 if c == 0 => {
                let _ = write!(s, r#"<a:{name}><a:sysClr val="windowText" lastClr="000000"/></a:{name}>"#);
            }
            1 if c == 0xFFFFFF => {
                let _ = write!(s, r#"<a:{name}><a:sysClr val="window" lastClr="FFFFFF"/></a:{name}>"#);
            }
            _ => {
                let _ = write!(s, r#"<a:{name}><a:srgbClr val="{c:06X}"/></a:{name}>"#);
            }
        }
    }
    s.push_str("</a:clrScheme>");
    let _ = write!(s, r#"<a:fontScheme name="{}">"#, esc(&t.fonts.name));
    for major in [true, false] {
        let tag = if major { "majorFont" } else { "minorFont" };
        let latin = if major { &t.fonts.major } else { &t.fonts.minor };
        let _ = write!(s, r#"<a:{tag}><a:latin typeface="{}"/>"#, esc(latin));
        // ea and cs are required, then the fonts by script.
        let face = |k: &str| t.fonts.scripts.iter().find(|(m, s, _)| *m == major && &**s == k).map(|x| esc(&x.2)).unwrap_or_default();
        let _ = write!(s, r#"<a:ea typeface="{}"/><a:cs typeface="{}"/>"#, face("ea"), face("cs"));
        for (_, script, f) in t.fonts.scripts.iter().filter(|(m, s, _)| *m == major && &**s != "ea" && &**s != "cs") {
            let _ = write!(s, r#"<a:font script="{}" typeface="{}"/>"#, esc(script), esc(f));
        }
        let _ = write!(s, "</a:{tag}>");
    }
    s.push_str("</a:fontScheme>");
    s.push_str(concat!(
        r#"<a:fmtScheme name="Office"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill>"#,
        r#"<a:solidFill><a:schemeClr val="phClr"><a:tint val="50000"/></a:schemeClr></a:solidFill>"#,
        r#"<a:solidFill><a:schemeClr val="phClr"><a:shade val="80000"/></a:schemeClr></a:solidFill></a:fillStyleLst>"#,
        r#"<a:lnStyleLst><a:ln w="6350" cap="flat" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="solid"/><a:miter lim="800000"/></a:ln>"#,
        r#"<a:ln w="12700" cap="flat" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="solid"/><a:miter lim="800000"/></a:ln>"#,
        r#"<a:ln w="19050" cap="flat" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="solid"/><a:miter lim="800000"/></a:ln></a:lnStyleLst>"#,
        r#"<a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst>"#,
        r#"<a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"><a:tint val="95000"/></a:schemeClr></a:solidFill>"#,
        r#"<a:solidFill><a:schemeClr val="phClr"><a:shade val="90000"/></a:schemeClr></a:solidFill></a:bgFillStyleLst></a:fmtScheme>"#,
        r#"</a:themeElements><a:objectDefaults/><a:extraClrSchemeLst/></a:theme>"#
    ));
    s
}

// ---------------------------------------------------------------------------
// WordprocessingML theme references

std::thread_local! {
    /// The theme of the document being written (run properties are written
    /// far from the document).
    static WRITING: std::cell::RefCell<Theme> = std::cell::RefCell::new(Theme::default());
}

/// Sets the theme the docx writers use on this thread.
pub(crate) fn set_writing_theme(t: &Theme) {
    WRITING.with(|w| *w.borrow_mut() = t.clone());
}

/// A font name with a theme font token resolved (for formats without
/// themes).
pub(crate) fn font_name(f: &str) -> String {
    WRITING.with(|w| w.borrow().font_name(f).to_string())
}

/// An Asian font name with a theme token (`+mn-ea`) resolved for text in
/// `lang`; None when the theme has no such font.
pub(crate) fn ea_font_name(f: &str, lang: Option<&str>) -> Option<String> {
    if !f.starts_with('+') {
        return Some(font_name(f));
    }
    WRITING.with(|w| w.borrow().ea_font(f, lang).map(|f| f.to_string()))
}

/// Word's names of the theme color slots.
const WORD_COLORS: [&str; 12] = ["text1", "background1", "text2", "background2", "accent1", "accent2", "accent3", "accent4", "accent5", "accent6", "hyperlink", "followedHyperlink"];

fn word_slot(name: &str) -> Option<u8> {
    let i = match name {
        "dark1" => 0,
        "light1" => 1,
        "dark2" => 2,
        "light2" => 3,
        n => WORD_COLORS.iter().position(|w| *w == n)?,
    };
    Some(i as u8)
}

/// A `w:color` (or shading) element's color: its theme color when it has
/// one (so the document's theme decides), else `w:val`.
pub(crate) fn read_word_color(t: &crate::xml::Tag, theme: &Theme) -> Option<u32> {
    if let Some(slot) = t.attr("w:themeColor").and_then(|n| word_slot(&n)) {
        let hex = |k: &str| t.attr(k).and_then(|v| u8::from_str_radix(v.trim(), 16).ok()).map(|v| v as f64 / 255.0);
        let tint = match (hex("w:themeTint"), hex("w:themeShade")) {
            (Some(v), _) => 1.0 - v,
            (None, Some(v)) => -(1.0 - v),
            _ => 0.0,
        };
        return Some(theme.resolve(apex_doc::ThemeColor::from_tint_f(slot, tint)));
    }
    t.attr("w:val").filter(|v| v != "auto").and_then(|v| crate::parse_hex_color(&v))
}

/// [`read_word_color`], with "auto" as black: set on a run or style, it
/// overrides the color it would inherit.
pub(crate) fn read_word_color_or_auto(t: &crate::xml::Tag, theme: &Theme) -> Option<u32> {
    read_word_color(t, theme).or_else(|| t.attr("w:val").filter(|v| v == "auto").map(|_| 0))
}

/// A `w:rFonts` element's font: a theme font token or a family name.
pub(crate) fn read_word_font(t: &crate::xml::Tag, theme: &apex_doc::Theme) -> Option<Arc<str>> {
    if let Some(th) = t.attr("w:asciiTheme").or_else(|| t.attr("w:hAnsiTheme")) {
        // The complex script or East Asian font of the theme, by name.
        let script = if th.ends_with("Bidi") { Some("cs") } else if th.ends_with("EastAsia") { Some("ea") } else { None };
        if let Some(k) = script {
            let major = th.starts_with("major");
            // (Without one, Word's complex script font is the Arabic one.)
            let find = |k: &str| theme.fonts.scripts.iter().find(|(m, s, f)| *m == major && &**s == k && !f.is_empty()).map(|x| x.2.clone());
            if let Some(f) = find(k).or_else(|| if k == "cs" { find("Arab") } else { None }) {
                return Some(f);
            }
        }
        if th.starts_with("major") {
            return Some(Arc::from(apex_doc::theme::MAJOR_TOKEN));
        }
        if th.starts_with("minor") {
            return Some(Arc::from(apex_doc::theme::MINOR_TOKEN));
        }
    }
    t.attr("w:ascii").or_else(|| t.attr("w:hAnsi")).map(|f| Arc::from(f.as_ref()))
}

/// Attributes of `w:rFonts` for a font name or theme font token.
pub(crate) fn word_font_attrs(f: &str) -> String {
    WRITING.with(|w| {
        let t = w.borrow();
        let name = esc(t.font_name(f)).into_owned();
        let theme = match f {
            apex_doc::theme::MAJOR_TOKEN => Some("major"),
            apex_doc::theme::MINOR_TOKEN => Some("minor"),
            _ => None,
        };
        match theme {
            Some(k) => format!(r#"w:ascii="{name}" w:hAnsi="{name}" w:cs="{name}" w:asciiTheme="{k}HAnsi" w:hAnsiTheme="{k}HAnsi""#),
            None => format!(r#"w:ascii="{name}" w:hAnsi="{name}" w:cs="{name}""#),
        }
    })
}

/// Attributes of `w:color`: the RGB, plus the theme color it matches.
pub(crate) fn word_color_attrs(c: u32) -> String {
    let c = c & 0xFF_FFFF;
    let tc = WRITING.with(|w| w.borrow().match_rgb(c));
    match tc {
        Some(tc) => {
            let mut s = format!(r#"w:val="{c:06X}" w:themeColor="{}""#, WORD_COLORS[tc.slot as usize % 12]);
            let f = tc.tint_f();
            if f > 0.0 {
                let _ = write!(s, r#" w:themeTint="{:02X}""#, ((1.0 - f) * 255.0).round() as u8);
            } else if f < 0.0 {
                let _ = write!(s, r#" w:themeShade="{:02X}""#, ((1.0 + f) * 255.0).round() as u8);
            }
            s
        }
        None => format!(r#"w:val="{c:06X}""#),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let t = Theme::builtin(12);
        let back = read_theme(&write_theme(&t));
        assert_eq!(back, t);
        let office = read_theme(&write_theme(&Theme::default()));
        assert_eq!(office, Theme::default());
    }

    fn themed_doc() -> apex_doc::Document {
        use apex_doc::{Attrs, Document, ParaKind, ParaStyle, Paragraph};
        let theme = Theme::builtin(7);
        let tint = theme.resolve(apex_doc::ThemeColor::from_tint_f(5, 0.4));
        let mut p = Paragraph::new("");
        p.push("acento ", Attrs { color: Some(theme.color(4)), ..Default::default() });
        p.push("claro ", Attrs { color: Some(tint), ..Default::default() });
        p.push("titular", Attrs { font: Some(Arc::from(apex_doc::theme::MAJOR_TOKEN)), ..Default::default() });
        p.push(" rgb", Attrs { color: Some(0x123456), ..Default::default() });
        let ps = vec![Paragraph::new("Título").with_style(ParaStyle { kind: ParaKind::Heading(1), ..Default::default() }), p];
        let mut doc = Document::from_paragraphs(ps);
        doc.styles.theme = theme;
        doc.default_font = Some(Arc::from(apex_doc::theme::MINOR_TOKEN));
        doc
    }

    #[test]
    fn docx_theme_roundtrip() {
        let doc = themed_doc();
        let data = crate::docx::write(&doc);
        let z = crate::zip::ZipReader::new(&data).unwrap();
        let body = z.read_string("word/document.xml").unwrap();
        assert!(body.contains(r#"w:themeColor="accent1""#), "{body}");
        assert!(body.contains(r#"w:themeColor="accent2" w:themeTint="99""#), "{body}");
        assert!(body.contains(r#"w:asciiTheme="majorHAnsi""#));
        assert!(z.read_string("word/styles.xml").unwrap().contains(r#"w:asciiTheme="minorHAnsi""#));
        assert!(z.read_string("word/theme/theme1.xml").unwrap().contains("Franklin Gothic"));
        let types = z.read_string("[Content_Types].xml").unwrap();
        assert!(types.contains(&format!(r#"<Override PartName="/word/theme/theme1.xml" ContentType="{CONTENT_TYPE}"/>"#)) && types.ends_with("</Types>"), "{types}");
        assert!(z.read_string("word/_rels/document.xml.rels").unwrap().contains(r#"Target="theme/theme1.xml""#));
        let back = crate::docx::read(&data).unwrap();
        assert_eq!(back.styles.theme, doc.styles.theme);
        assert_eq!(back.default_font.as_deref(), Some(apex_doc::theme::MINOR_TOKEN));
        assert_eq!(back.default_font_name().as_deref(), Some("Franklin Gothic Book"));
        let p = &back.paragraphs[1];
        for (i, want) in [(1, doc.paragraphs[1].attrs_at(1).color), (8, doc.paragraphs[1].attrs_at(8).color), (22, Some(0x123456))] {
            assert_eq!(p.attrs_at(i).color, want, "at {i}");
        }
        assert_eq!(p.attrs_at(16).font.as_deref(), Some(apex_doc::theme::MAJOR_TOKEN));
        let h = back.char_format(&back.paragraphs[0].style, &apex_doc::Attrs::default());
        assert_eq!(h.font.as_deref(), Some("Franklin Gothic Medium"));
    }

    #[test]
    fn odt_writes_resolved_fonts() {
        let doc = themed_doc();
        let data = crate::odt::write(&doc);
        let z = crate::zip::ZipReader::new(&data).unwrap();
        let content = z.read_string("content.xml").unwrap();
        assert!(content.contains("'Franklin Gothic Medium'") && !content.contains("+mj-lt"), "{content}");
        let styles = z.read_string("styles.xml").unwrap();
        assert!(styles.contains("'Franklin Gothic Book'") && !styles.contains("+mn-lt"));
        let back = crate::odt::read(&data).unwrap();
        assert_eq!(back.paragraphs[1].attrs_at(1).color, doc.paragraphs[1].attrs_at(1).color);
        assert_eq!(back.paragraphs[1].attrs_at(16).font.as_deref(), Some("Franklin Gothic Medium"));
    }
}
