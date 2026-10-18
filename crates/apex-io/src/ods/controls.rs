//! Form controls in .ods: the `form:*` elements of a sheet's
//! `office:forms` (their settings) and the `draw:control` shapes that
//! place them (see [`super::shapes`]). Cell references are kept in Excel
//! syntax in the model (`Sheet1!$B$1`) and turned into ODF's here.
//! Attributes and children not understood are written back as they came.

use std::collections::HashMap;
use std::fmt::Write as _;

use apex_sheet::addr::col_name;
use apex_sheet::{CellRef, ControlKind, FormControl, Sheet};

use crate::xml::{Ev, Reader, Tag, esc};

/// Attributes written from the model.
const KNOWN: [&str; 21] = [
    "form:name", "form:id", "xml:id", "form:control-implementation", "form:label", "form:linked-cell", "form:source-cell-range", "form:current-state", "form:state",
    "form:current-selected", "form:selected", "form:value", "form:current-value", "form:min-value", "form:max-value", "form:step-size", "form:page-step-size",
    "form:orientation", "form:dropdown", "form:size", "form:list-linkage-type",
];
const MAX_CONTROLS: usize = 2000;

/// `Sheet1.B1`, `$'My sheet'.$A$1:.$A$5` to `Sheet1!$B$1`, `'My sheet'!$A$1:$A$5`.
pub(super) fn from_odf(s: &str) -> String {
    let mut sheet = String::new();
    let mut cells = Vec::new();
    for part in s.trim().split(':') {
        let (sh, cell) = match part.rsplit_once('.') {
            Some((a, b)) => (a.trim_start_matches('$'), b),
            None => ("", part),
        };
        if sheet.is_empty() && !sh.is_empty() {
            sheet = sh.to_string();
        }
        match CellRef::parse(&cell.replace('$', "")) {
            Some(c) => cells.push(format!("${}${}", col_name(c.col), c.row + 1)),
            None => return String::new(),
        }
    }
    let cells = cells.join(":");
    if sheet.is_empty() { cells } else { format!("{sheet}!{cells}") }
}

/// The reverse of [`from_odf`]; references without a sheet are on `at`.
pub(super) fn to_odf(s: &str, at: &str) -> String {
    let (sheet, cells) = match s.trim().rsplit_once('!') {
        Some((a, b)) => (a.to_string(), b),
        None => {
            let plain = at.chars().all(|ch| ch.is_alphanumeric() || ch == '_');
            (if plain { at.to_string() } else { format!("'{}'", at.replace('\'', "''")) }, s.trim())
        }
    };
    if cells.is_empty() {
        return String::new();
    }
    cells.split(':').map(|c| format!("{sheet}.{}", c.replace('$', ""))).collect::<Vec<_>>().join(":")
}

fn kind_of(t: &Tag) -> Option<ControlKind> {
    let imp = t.attr("form:control-implementation").unwrap_or_default();
    Some(match t.name {
        "form:button" => ControlKind::Button,
        "form:checkbox" => ControlKind::CheckBox,
        "form:radio" => ControlKind::Radio,
        "form:listbox" if t.attr("form:dropdown").as_deref() == Some("true") => ControlKind::ComboBox,
        "form:listbox" => ControlKind::ListBox,
        "form:combobox" => ControlKind::ComboBox,
        "form:value-range" if imp.contains("Spin") => ControlKind::Spin,
        "form:value-range" => ControlKind::ScrollBar,
        "form:frame" => ControlKind::GroupBox,
        "form:fixed-text" => ControlKind::Label,
        _ => return None,
    })
}

/// Reads the `office:forms` whose start was just returned: the controls
/// of sheet `si` by their id.
pub(super) fn read_forms(r: &mut Reader, si: usize, out: &mut HashMap<(usize, String), FormControl>) {
    let mut depth = 1;
    loop {
        let (t, empty) = match r.next() {
            Ev::Start(t) => (t, false),
            Ev::Empty(t) => (t, true),
            Ev::End(_) => {
                depth -= 1;
                if depth == 0 {
                    return;
                }
                continue;
            }
            Ev::Eof => return,
            _ => continue,
        };
        let Some(kind) = kind_of(&t) else {
            if !empty {
                depth += 1;
            }
            continue;
        };
        let id = t.attr("form:id").or_else(|| t.attr("xml:id")).unwrap_or_default().into_owned();
        let mut c = FormControl::new(kind);
        let num = |k: &str| t.attr(k).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite()).map(|v| v.clamp(-1.0e6, 1.0e6).round() as i32);
        c.text = t.attr("form:label").unwrap_or_default().into_owned();
        c.link = from_odf(&t.attr("form:linked-cell").unwrap_or_default());
        c.items = from_odf(&t.attr("form:source-cell-range").unwrap_or_default());
        c.value = match kind {
            ControlKind::CheckBox => match t.attr("form:current-state").or_else(|| t.attr("form:state")).as_deref() {
                Some("checked") => 1,
                Some("unknown") => 2,
                _ => 0,
            },
            ControlKind::Radio => matches!(t.attr("form:current-selected").or_else(|| t.attr("form:selected")).as_deref(), Some("true")) as i32,
            ControlKind::Spin | ControlKind::ScrollBar => num("form:value").unwrap_or(0),
            _ => 0,
        };
        c.min = num("form:min-value").unwrap_or(0);
        c.max = num("form:max-value").unwrap_or(100);
        c.inc = num("form:step-size").unwrap_or(1);
        c.page = num("form:page-step-size").unwrap_or(10);
        c.lines = num("form:size").unwrap_or(8).max(1) as u32;
        c.horizontal = t.attr("form:orientation").as_deref() == Some("horizontal");
        for (k, v) in t.attrs() {
            if !KNOWN.contains(&k) {
                let _ = write!(c.odf_attrs, r#" {k}="{}""#, esc(&v));
            }
        }
        if !empty {
            let raw = r.raw_element();
            c.odf_kids = raw.rfind("</").map_or(raw, |i| &raw[..i]).to_string();
        }
        c.sanitize();
        if out.len() < MAX_CONTROLS && !id.is_empty() {
            out.insert((si, id), c);
        }
    }
}

/// Id of a control in the file.
pub(super) fn control_id(si: usize, id: u32) -> String {
    format!("ctl{}_{id}", si + 1)
}

/// The `office:forms` of sheet `si`, or nothing without controls.
pub(super) fn write_forms(sh: &Sheet, si: usize) -> String {
    let mut s = String::new();
    for sc in &sh.shapes {
        let Some(c) = &sc.control else { continue };
        let id = control_id(si, sc.id);
        let (tag, imp) = match c.kind {
            ControlKind::Button => ("button", "CommandButton"),
            ControlKind::CheckBox => ("checkbox", "CheckBox"),
            ControlKind::Radio => ("radio", "RadioButton"),
            ControlKind::ComboBox | ControlKind::ListBox => ("listbox", "ListBox"),
            ControlKind::Spin => ("value-range", "SpinButton"),
            ControlKind::ScrollBar => ("value-range", "ScrollBar"),
            ControlKind::GroupBox => ("frame", "GroupBox"),
            ControlKind::Label => ("fixed-text", "FixedText"),
        };
        let name = if sc.shape.name.is_empty() { format!("Control {}", sc.id) } else { sc.shape.name.clone() };
        let _ = write!(s, r#"<form:{tag} form:name="{}" form:control-implementation="ooo:com.sun.star.form.component.{imp}" xml:id="{id}" form:id="{id}""#, esc(&name));
        if c.kind.has_text() {
            let _ = write!(s, r#" form:label="{}""#, esc(&c.text));
        }
        let link = to_odf(&c.link, &sh.name);
        if c.kind.has_link() && !link.is_empty() {
            let _ = write!(s, r#" form:linked-cell="{}""#, esc(&link));
        }
        match c.kind {
            ControlKind::CheckBox => {
                let st = ["unchecked", "checked", "unknown"][c.value.clamp(0, 2) as usize];
                let _ = write!(s, r#" form:current-state="{st}""#);
            }
            ControlKind::Radio => {
                // The cell gets the reference value: the button's place,
                // as Excel writes.
                let group = sh.shapes.iter().filter(|x| x.control.as_ref().is_some_and(|y| y.kind == ControlKind::Radio && y.link == c.link)).position(|x| x.id == sc.id);
                let _ = write!(s, r#" form:value="{}""#, group.unwrap_or(0) + 1);
                if c.value > 0 {
                    s.push_str(r#" form:current-selected="true""#);
                }
            }
            ControlKind::ComboBox | ControlKind::ListBox => {
                let items = to_odf(&c.items, &sh.name);
                if !items.is_empty() {
                    let _ = write!(s, r#" form:source-cell-range="{}""#, esc(&items));
                }
                let _ = write!(s, r#" form:dropdown="{}" form:size="{}" form:list-linkage-type="selection-indices""#, c.kind == ControlKind::ComboBox, c.lines);
            }
            ControlKind::Spin | ControlKind::ScrollBar => {
                let _ = write!(
                    s,
                    r#" form:min-value="{}" form:max-value="{}" form:step-size="{}" form:page-step-size="{}" form:value="{}" form:orientation="{}""#,
                    c.min,
                    c.max,
                    c.inc,
                    c.page,
                    c.value,
                    if c.horizontal { "horizontal" } else { "vertical" }
                );
            }
            _ => {}
        }
        s.push_str(&c.odf_attrs);
        if c.odf_kids.is_empty() {
            s.push_str("/>");
        } else {
            let _ = write!(s, ">{}</form:{tag}>", c.odf_kids);
        }
    }
    if s.is_empty() {
        return s;
    }
    format!(r#"<office:forms form:automatic-focus="false" form:apply-design-mode="false"><form:form form:name="Form" form:apply-filter="true" form:command-type="table" form:control-implementation="ooo:com.sun.star.form.component.Form">{s}</form:form></office:forms>"#)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references() {
        assert_eq!(from_odf("Sheet1.B1"), "Sheet1!$B$1");
        assert_eq!(from_odf("$'My sheet'.$A$1:.$A$5"), "'My sheet'!$A$1:$A$5");
        assert_eq!(from_odf("Sheet1.A1:Sheet1.A5"), "Sheet1!$A$1:$A$5");
        assert_eq!(to_odf("$B$1", "My sheet"), "'My sheet'.B1");
        assert_eq!(to_odf("Data!$A$1:$A$3", "S"), "Data.A1:Data.A3");
    }

    #[test]
    fn ods_roundtrip() {
        use apex_doc::{Shape, ShapeKind};
        use apex_sheet::{Anchor, SheetShape, Workbook};
        let mut wb = Workbook::new();
        let kinds = [ControlKind::CheckBox, ControlKind::ListBox, ControlKind::Spin, ControlKind::Button];
        for (i, k) in kinds.into_iter().enumerate() {
            let mut s = SheetShape::new(Shape::new(ShapeKind::Rect), CellRef::new(2 + i as u32 * 3, 3), 110.0, 40.0);
            s.anchor = Anchor::OneCell;
            let mut c = FormControl::new(k);
            c.text = if k.has_text() { format!("Control {i}") } else { String::new() };
            c.link = if k.has_link() { format!("Sheet1!$B${}", i + 1) } else { String::new() };
            c.items = if k == ControlKind::ListBox { "Sheet1!$A$1:$A$4".into() } else { String::new() };
            c.value = if k == ControlKind::Spin { 7 } else { (k == ControlKind::CheckBox) as i32 };
            if k == ControlKind::Button {
                c.odf_attrs = r#" form:button-type="push""#.into();
                c.odf_kids = r#"<form:properties><form:property form:property-name="DefaultControl" office:value-type="string" office:string-value="x"/></form:properties>"#.into();
            }
            s.control = Some(Box::new(c));
            wb.add_shape(0, s);
        }
        let back = crate::ods::read(&crate::ods::write(&wb)).unwrap();
        let sh = &back.sheets[0];
        assert_eq!(sh.shapes.len(), 4);
        for (a, b) in wb.sheets[0].shapes.iter().zip(&sh.shapes) {
            assert_eq!(a.control, b.control);
            assert_eq!((a.from, a.anchor), (b.from, b.anchor));
            assert!((a.width - b.width).abs() < 1.0);
        }
    }
}
