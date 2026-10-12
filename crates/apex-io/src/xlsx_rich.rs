//! Rich strings of xlsx (`<si>` and `<is>` with `<r><rPr>` runs).

use std::fmt::Write as _;
use std::sync::Arc;

use apex_sheet::rich::{RichRun, RichText, RunFont, Script};
use apex_sheet::{CellStyle, Theme};

use crate::xml::{Ev, Reader, esc};

/// Reads the content of an `<si>` or `<is>` element after its start tag,
/// up to its end: the text and its runs (`None` for plain text).
/// The text of a string item that is one `<t>` alone (no runs, no
/// phonetics), borrowed when it has no entities; the reader moves past it.
/// `None` leaves the reader where it was.
pub(crate) fn plain_rst<'a>(r: &mut Reader<'a>) -> Option<std::borrow::Cow<'a, str>> {
    let mut peek = r.clone();
    let Ev::Start(x) = peek.next() else { return None };
    if x.local() != "t" {
        return None;
    }
    let text = peek.text_cow();
    let Ev::End(_) = peek.next() else { return None };
    *r = peek;
    Some(text)
}

pub(crate) fn read_rst(r: &mut Reader, theme: &Theme) -> (String, Option<RichText>) {
    let mut text = String::new();
    let mut runs = RichText::new();
    let mut depth = 1;
    let mut in_rph = false;
    let mut font: Option<RunFont> = None;
    let mut in_rpr = false;
    while depth > 0 {
        let (x, open) = match r.next() {
            Ev::Start(x) => (x, true),
            Ev::Empty(x) => (x, false),
            Ev::End(x) => {
                match crate::xml::local(x) {
                    "rPh" => in_rph = false,
                    "rPr" => in_rpr = false,
                    _ => {}
                }
                depth -= 1;
                continue;
            }
            Ev::Eof => break,
            _ => continue,
        };
        if x.local() == "t" && open && !in_rph {
            let t = r.text();
            runs.push(RichRun { len: t.len(), font: font.clone().unwrap_or_default() });
            text.push_str(&t);
            continue;
        }
        if open {
            depth += 1;
        }
        match x.local() {
            "rPh" => in_rph = open,
            "r" => font = None,
            "rPr" => {
                in_rpr = open;
                font = Some(RunFont::default());
            }
            name if in_rpr => {
                let f = font.get_or_insert_with(RunFont::default);
                let val = x.attr("val");
                let on = val.as_deref().is_none_or(|v| v != "0" && v != "false");
                match name {
                    "b" => f.bold = Some(on),
                    "i" => f.italic = Some(on),
                    "strike" => f.strike = Some(on),
                    "u" => f.underline = Some(val.as_deref() != Some("none")),
                    "sz" => f.size = val.and_then(|v| v.trim().parse().ok()),
                    "rFont" => f.font = val.map(|v| Arc::from(v.as_ref())),
                    "color" => f.color = crate::xlsx_styles::read_color(&x, theme).map(|c| c.0),
                    "vertAlign" => {
                        f.script = match val.as_deref() {
                            Some("superscript") => Script::Super,
                            Some("subscript") => Script::Sub,
                            _ => Script::Normal,
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    (text, apex_sheet::rich::normalize(runs))
}

/// The runs of a text as `<r>` elements. A run's `rPr` lists its whole
/// font (Excel doesn't take the rest from the cell), so the cell's own
/// format fills what the run leaves alone.
pub(crate) fn write_runs(out: &mut String, text: &str, runs: &[RichRun], st: &CellStyle) {
    let mut at = 0;
    for run in runs {
        let part = &text[at..at + run.len];
        at += run.len;
        let f = &run.font;
        out.push_str("<r>");
        if !f.is_plain() {
            out.push_str("<rPr>");
            if f.bold.unwrap_or(st.bold) {
                out.push_str("<b/>");
            }
            if f.italic.unwrap_or(st.italic) {
                out.push_str("<i/>");
            }
            if f.strike.unwrap_or(st.strike) {
                out.push_str("<strike/>");
            }
            if let Some(c) = f.color.or(st.color) {
                let _ = write!(out, r#"<color rgb="FF{c:06X}"/>"#);
            }
            let _ = write!(out, r#"<sz val="{}"/>"#, f.size.unwrap_or(if st.font_size > 0 { st.font_size as f32 / 10.0 } else { 11.0 }));
            if f.underline.unwrap_or(st.underline) {
                out.push_str("<u/>");
            }
            match f.script {
                Script::Super => out.push_str(r#"<vertAlign val="superscript"/>"#),
                Script::Sub => out.push_str(r#"<vertAlign val="subscript"/>"#),
                Script::Normal => {}
            }
            let name = f.font.as_deref().or(st.font.as_deref()).unwrap_or("Calibri");
            let _ = write!(out, r#"<rFont val="{}"/>"#, esc(name));
            out.push_str("</rPr>");
        }
        let space = if part.starts_with(char::is_whitespace) || part.ends_with(char::is_whitespace) { r#" xml:space="preserve""# } else { "" };
        let _ = write!(out, "<t{space}>{}</t></r>", esc(part));
    }
}
