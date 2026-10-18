//! Named cell styles in ODF: common `style:style` elements of the
//! table-cell family in styles.xml; cells' automatic styles name them as
//! their parent. ODF has no themes, so colors and fonts are written
//! resolved.

use std::fmt::Write as _;

use apex_sheet::cellstyles::{self, NamedStyle, part};
use apex_sheet::{CellStyle, Workbook};

use crate::xml::esc;

/// Styles LibreOffice puts in every file; they are only kept when cells use
/// them.
const LO_BUILTINS: [&str; 22] = [
    "Default", "Heading", "Heading 1", "Heading 2", "Text", "Note", "Footnote", "Hyperlink", "Status", "Good", "Neutral", "Bad", "Warning", "Error", "Accent", "Accent 1", "Accent 2", "Accent 3", "Result", "Result2", "Report", "Title",
];

pub(super) fn is_lo_builtin(name: &str) -> bool {
    LO_BUILTINS.contains(&name)
}

/// An NCName for a style name, as LibreOffice makes them ("Heading 1" ->
/// "Heading_20_1").
pub(super) fn odf_name(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        let ok = c.is_alphanumeric() && c.is_ascii() || c == '_' || (i > 0 && (c == '-' || c == '.'));
        if ok && !(i == 0 && c.is_ascii_digit()) {
            out.push(c);
        } else {
            let _ = write!(out, "_{:x}_", c as u32);
        }
    }
    out
}

/// The name behind an encoded style name (for files without display names).
pub(super) fn decode_name(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('_') {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 1..];
        match tail.find('_').and_then(|j| u32::from_str_radix(&tail[..j], 16).ok().and_then(char::from_u32).map(|c| (j, c))) {
            Some((j, c)) if j > 0 && j <= 6 => {
                out.push(c);
                rest = &tail[j + 1..];
            }
            _ => {
                out.push('_');
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The ODF name of a cell's named style (Default for Normal).
pub(super) fn parent_of(st: &CellStyle, wb: &Workbook) -> String {
    match st.named.as_deref().and_then(|n| wb.cell_styles.get(n)) {
        Some(ns) if !ns.is_normal() => odf_name(&ns.name),
        _ => "Default".into(),
    }
}

/// The part of a style's format it includes (the rest comes from Default).
pub(super) fn masked(ns: &NamedStyle) -> CellStyle {
    if ns.is_normal() {
        return CellStyle { named: None, ..ns.style.clone() };
    }
    let mut s = CellStyle::default();
    cellstyles::copy_parts(&mut s, &ns.style, ns.parts);
    s
}

/// Writes the common cell styles; `props` writes a format's properties
/// (with its number style name).
pub(super) fn write_named(wb: &Workbook, out: &mut String, numbers: &mut String, props: &dyn Fn(&CellStyle) -> String, number_style: &dyn Fn(&str, &str) -> Option<String>) {
    for (i, ns) in wb.saved_cell_styles().into_iter().enumerate() {
        let st = masked(ns);
        let (name, parent) = if ns.is_normal() { ("Default".to_string(), None) } else { (odf_name(&ns.name), Some("Default")) };
        let _ = write!(out, r#"<style:style style:name="{}" style:display-name="{}" style:family="table-cell""#, esc(&name), esc(if ns.is_normal() { "Default" } else { &ns.name }));
        if let Some(p) = parent {
            let _ = write!(out, r#" style:parent-style-name="{p}""#);
        }
        if ns.parts & part::NUMBER != 0 && &*st.num_fmt != "General" {
            let n = format!("SN{}", i + 1);
            if let Some(x) = number_style(&n, &st.num_fmt) {
                numbers.push_str(&x);
                let _ = write!(out, r#" style:data-style-name="{n}""#);
            }
        }
        out.push('>');
        out.push_str(&props(&st));
        out.push_str("</style:style>");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(odf_name("Heading 1"), "Heading_20_1");
        assert_eq!(odf_name("20% - Accent1"), "_32_0_25__20_-_20_Accent1");
        assert_eq!(decode_name("Heading_20_1"), "Heading 1");
        assert_eq!(decode_name(&odf_name("20% - Accent1")), "20% - Accent1");
        assert_eq!(decode_name("a_b"), "a_b");
    }

    #[test]
    fn roundtrip() {
        use apex_sheet::{CellRef, Range, Theme};
        let mut wb = Workbook::new();
        wb.set_input(0, CellRef::new(0, 0), "12");
        wb.set_theme(Theme::builtin(7));
        wb.apply_cell_style(0, &Range::parse("A1").unwrap(), "Good");
        wb.apply_cell_style(0, &Range::parse("A2").unwrap(), "Heading 1");
        wb.apply_cell_style(0, &Range::parse("A3").unwrap(), "Percent");
        let mut st = CellStyle::default();
        st.italic = true;
        st.fill = Some(0x123456);
        wb.set_cell_style(None, wb.style_from_format("My style", &st));
        wb.apply_cell_style(0, &Range::parse("B1").unwrap(), "My style");
        wb.modify_style(0, &Range::parse("B1").unwrap(), |s| s.bold = true);
        let mut good = wb.cell_styles.get("Good").unwrap().clone();
        good.style.fill = Some(0x00FF00);
        wb.set_cell_style(Some("Good"), good);

        let data = crate::ods::write(&wb);
        let z = crate::zip::ZipReader::new(&data).unwrap();
        let styles = z.read_string("styles.xml").unwrap();
        assert!(styles.contains(r#"style:name="My_20_style" style:display-name="My style""#), "{styles}");
        assert!(z.read_string("content.xml").unwrap().contains(r#"style:parent-style-name="Good""#));

        let back = crate::ods::read(&data).unwrap();
        let style = |a1: &str| back.styles.get(back.cell(0, CellRef::parse(a1).unwrap()).map_or(0, |c| c.style)).clone();
        assert_eq!((style("A1").named.as_deref(), style("A1").fill), (Some("Good"), Some(0x00FF00)));
        assert_eq!(back.cell_styles.get("Good").unwrap().style.fill, Some(0x00FF00));
        let b1 = style("B1");
        assert!(b1.bold && b1.italic && b1.named.as_deref() == Some("My style"));
        let mine = back.cell_styles.get("My style").unwrap();
        assert!(mine.style.italic && mine.style.fill == Some(0x123456) && mine.parts & part::FILL != 0);
        // Theme colors are written resolved.
        assert_eq!(style("A2").borders.bottom.color, Some(Theme::builtin(7).color(4)));
        assert_eq!(style("A3").named.as_deref(), Some("Percent"));
        assert_eq!(&*back.cell_styles.get("Percent").unwrap().style.num_fmt, "0%");
    }
}
