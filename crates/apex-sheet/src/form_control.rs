//! Form controls floating over the cells (buttons, check boxes, option
//! buttons, lists, spinners, scroll bars...), linked to a cell as in
//! Excel's Forms toolbar and Calc's form controls. A control is a
//! [`crate::SheetShape`] carrying a [`FormControl`], so it is placed,
//! moved, copied and undone like any shape.

use crate::value::Value;
use crate::{CellRef, Range, Workbook};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlKind {
    Button,
    CheckBox,
    Radio,
    ComboBox,
    ListBox,
    Spin,
    ScrollBar,
    GroupBox,
    Label,
}

impl ControlKind {
    pub const ALL: [ControlKind; 9] = [
        ControlKind::Button,
        ControlKind::CheckBox,
        ControlKind::Radio,
        ControlKind::ComboBox,
        ControlKind::ListBox,
        ControlKind::Spin,
        ControlKind::ScrollBar,
        ControlKind::GroupBox,
        ControlKind::Label,
    ];

    /// Excel's `ObjectType` in VML.
    pub fn vml(self) -> &'static str {
        match self {
            ControlKind::Button => "Button",
            ControlKind::CheckBox => "Checkbox",
            ControlKind::Radio => "Radio",
            ControlKind::ComboBox => "Drop",
            ControlKind::ListBox => "List",
            ControlKind::Spin => "Spin",
            ControlKind::ScrollBar => "Scroll",
            ControlKind::GroupBox => "GBox",
            ControlKind::Label => "Label",
        }
    }

    pub fn from_vml(s: &str) -> Option<ControlKind> {
        ControlKind::ALL.into_iter().find(|k| k.vml().eq_ignore_ascii_case(s))
    }

    /// Whether the control shows a caption.
    pub fn has_text(self) -> bool {
        matches!(self, ControlKind::Button | ControlKind::CheckBox | ControlKind::Radio | ControlKind::GroupBox | ControlKind::Label)
    }

    /// Whether the control writes to a linked cell.
    pub fn has_link(self) -> bool {
        !matches!(self, ControlKind::Button | ControlKind::GroupBox | ControlKind::Label)
    }

    pub fn has_items(self) -> bool {
        matches!(self, ControlKind::ComboBox | ControlKind::ListBox)
    }

    pub fn has_range(self) -> bool {
        matches!(self, ControlKind::Spin | ControlKind::ScrollBar)
    }

    /// Default size in sheet pixels.
    pub fn size(self) -> (f32, f32) {
        match self {
            ControlKind::Button => (96.0, 28.0),
            ControlKind::CheckBox | ControlKind::Radio | ControlKind::Label => (110.0, 20.0),
            ControlKind::ComboBox => (120.0, 22.0),
            ControlKind::ListBox => (120.0, 80.0),
            ControlKind::Spin => (20.0, 40.0),
            ControlKind::ScrollBar => (20.0, 100.0),
            ControlKind::GroupBox => (160.0, 100.0),
        }
    }
}

/// A form control's settings.
#[derive(Clone, Debug, PartialEq)]
pub struct FormControl {
    pub kind: ControlKind,
    /// Caption of buttons, boxes and labels.
    pub text: String,
    /// Linked cell as written in files (`$C$3`, `Sheet2!$C$3`); empty
    /// for none.
    pub link: String,
    /// Cells listing the items of a list or drop-down.
    pub items: String,
    /// Check box: 0 off, 1 on, 2 mixed; option button: 1 when chosen;
    /// lists: the chosen item from 1 (0 none); spinners and scroll bars:
    /// the value. Used when there is no linked cell.
    pub value: i32,
    pub min: i32,
    pub max: i32,
    pub inc: i32,
    pub page: i32,
    /// Lines a drop-down shows.
    pub lines: u32,
    /// Macro run by a click (Excel's `FmlaMacro`).
    pub macro_name: String,
    /// Scroll bar or spinner laid out left to right.
    pub horizontal: bool,
    /// xlsx settings not understood (`x:ClientData` children), written
    /// back as they came.
    pub extra: String,
    /// Likewise for ods: attributes of the `form:*` element and its
    /// children (properties, events, items).
    pub odf_attrs: String,
    pub odf_kids: String,
}

impl FormControl {
    pub fn new(kind: ControlKind) -> FormControl {
        FormControl {
            kind,
            text: String::new(),
            link: String::new(),
            items: String::new(),
            value: 0,
            min: 0,
            max: 100,
            inc: 1,
            page: 10,
            lines: 8,
            macro_name: String::new(),
            horizontal: false,
            extra: String::new(),
            odf_attrs: String::new(),
            odf_kids: String::new(),
        }
    }

    /// Keeps values usable whatever a file said.
    pub fn sanitize(&mut self) {
        self.max = self.max.clamp(0, 30000);
        self.min = self.min.clamp(0, self.max);
        self.inc = self.inc.clamp(1, 30000);
        self.page = self.page.clamp(1, 30000);
        self.lines = self.lines.clamp(1, 100);
        if self.kind.has_range() {
            self.value = self.value.clamp(self.min, self.max);
        } else {
            self.value = self.value.clamp(0, 30000);
        }
    }
}

/// Sheet and range of a reference written in a control (`$C$3`,
/// `'My sheet'!$A$1:$A$9`), relative to sheet `at`.
pub fn resolve(wb: &Workbook, at: usize, text: &str) -> Option<(usize, Range)> {
    let t = text.trim().trim_start_matches('=');
    if t.is_empty() {
        return None;
    }
    let (sheet, cells) = match t.rsplit_once('!') {
        Some((s, c)) => {
            let s = s.trim_matches('\'').replace("''", "'");
            (wb.sheet_index(&s)?, c)
        }
        None => (at, t),
    };
    let r = Range::parse(&cells.replace('$', ""))?;
    (sheet < wb.sheets.len()).then_some((sheet, r))
}

impl Workbook {
    /// The state a control shows: its linked cell's value when it has one.
    pub fn control_value(&self, sheet: usize, c: &FormControl) -> i32 {
        let Some((s, r)) = c.kind.has_link().then(|| resolve(self, sheet, &c.link)).flatten() else { return c.value };
        match (c.kind, self.sheets[s].value(r.start)) {
            (ControlKind::CheckBox, Value::Bool(b)) => *b as i32,
            (ControlKind::CheckBox, Value::Error(_)) => 2,
            (ControlKind::CheckBox, Value::Empty) => 0,
            (ControlKind::Radio, _) => c.value,
            (_, Value::Number(n)) if n.is_finite() => n.round().clamp(-1.0e9, 1.0e9) as i32,
            (ControlKind::CheckBox, _) => c.value,
            _ => 0,
        }
    }

    /// Items of a list or drop-down, as the cells show them.
    pub fn control_items(&self, sheet: usize, c: &FormControl) -> Vec<String> {
        let Some((s, r)) = resolve(self, sheet, &c.items) else { return Vec::new() };
        let mut out = Vec::new();
        for row in r.start.row..=r.end.row.min(r.start.row + 999) {
            for col in r.start.col..=r.end.col.min(r.start.col + 99) {
                out.push(self.display(s, CellRef::new(row, col)).0);
            }
        }
        out
    }

    /// Option buttons that answer together with shape `id`: those linked
    /// to the same cell (or all those without one), in stacking order.
    pub fn radio_group(&self, sheet: usize, id: u32) -> Vec<u32> {
        let sh = &self.sheets[sheet];
        let Some(link) = sh.shapes.iter().find(|s| s.id == id).and_then(|s| s.control.as_ref()).map(|c| c.link.replace('$', "")) else { return Vec::new() };
        sh.shapes
            .iter()
            .filter(|s| s.control.as_ref().is_some_and(|c| c.kind == ControlKind::Radio && c.link.replace('$', "") == link))
            .map(|s| s.id)
            .collect()
    }

    /// Whether option button `id` is chosen: its place in the group is in
    /// the linked cell, or it was the last one clicked.
    pub fn radio_on(&self, sheet: usize, id: u32) -> bool {
        let sh = &self.sheets[sheet];
        let Some(c) = sh.shapes.iter().find(|s| s.id == id).and_then(|s| s.control.as_deref()) else { return false };
        if let Some((s, r)) = resolve(self, sheet, &c.link) {
            if let Value::Number(n) = self.sheets[s].value(r.start) {
                return self.radio_group(sheet, id).iter().position(|g| *g == id).is_some_and(|i| (i + 1) as f64 == *n);
            }
        }
        c.value > 0
    }

    /// Operates control `id` as a click would: sets its value and its
    /// linked cell, as one undo step.
    pub fn operate_control(&mut self, sheet: usize, id: u32, value: i32) {
        let sh = &self.sheets[sheet];
        let Some(c) = sh.shapes.iter().find(|s| s.id == id).and_then(|s| s.control.as_deref()).cloned() else { return };
        let group = if c.kind == ControlKind::Radio { self.radio_group(sheet, id) } else { vec![id] };
        let mut list = sh.shapes.clone();
        for s in &mut list {
            if let Some(x) = s.control.as_deref_mut().filter(|_| group.contains(&s.id)) {
                x.value = if c.kind == ControlKind::Radio { (s.id == id) as i32 } else { value };
                x.sanitize();
            }
        }
        let n = self.undo_len();
        if list != self.sheets[sheet].shapes {
            self.execute_as(crate::workbook::Op::Shapes { sheet, list }, "undo.control");
        }
        if let Some((s, r)) = c.kind.has_link().then(|| resolve(self, sheet, &c.link)).flatten() {
            let text = match c.kind {
                ControlKind::CheckBox => match value {
                    0 => "FALSE".to_string(),
                    1 => "TRUE".to_string(),
                    _ => "=NA()".to_string(),
                },
                ControlKind::Radio => (group.iter().position(|g| *g == id).unwrap_or(0) + 1).to_string(),
                _ => value.to_string(),
            };
            self.set_input(s, r.start, &text);
        }
        self.merge_undo(self.undo_len().saturating_sub(n), "undo.control");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SheetShape;
    use apex_doc::{Shape, ShapeKind};

    fn control(wb: &mut Workbook, c: FormControl) -> u32 {
        let mut s = SheetShape::new(Shape::new(ShapeKind::Rect), CellRef::new(0, 3), 100.0, 20.0);
        s.control = Some(Box::new(c));
        wb.add_shape(0, s)
    }

    #[test]
    fn operate_sets_linked_cells() {
        let mut wb = Workbook::new();
        let mut c = FormControl::new(ControlKind::CheckBox);
        c.link = "$A$1".into();
        let id = control(&mut wb, c);
        wb.operate_control(0, id, 1);
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 0)), &Value::Bool(true));
        let c = wb.sheets[0].shapes[0].control.clone().unwrap();
        assert_eq!(wb.control_value(0, &c), 1);
        wb.undo();
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 0)), &Value::Empty);
        assert_eq!(wb.sheets[0].shapes[0].control.as_ref().unwrap().value, 0, "one undo step for both");
        assert_eq!(wb.sheets[0].shapes.len(), 1);
        // Option buttons sharing a cell answer with their position.
        let mut r = FormControl::new(ControlKind::Radio);
        r.link = "Sheet1!B1".into();
        let a = control(&mut wb, r.clone());
        let b = control(&mut wb, r);
        wb.operate_control(0, b, 1);
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 1)), &Value::Number(2.0));
        let on: Vec<i32> = wb.sheets[0].shapes.iter().filter(|s| s.id == a || s.id == b).map(|s| s.control.as_ref().unwrap().value).collect();
        assert_eq!(on, vec![0, 1]);
    }

    #[test]
    fn list_items() {
        let mut wb = Workbook::new();
        wb.set_input(0, CellRef::new(0, 0), "red");
        wb.set_input(0, CellRef::new(1, 0), "green");
        let mut c = FormControl::new(ControlKind::ListBox);
        c.items = "$A$1:$A$2".into();
        assert_eq!(wb.control_items(0, &c), vec!["red", "green"]);
    }
}
