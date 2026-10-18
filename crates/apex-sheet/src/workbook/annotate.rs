//! Notes, hyperlinks and protection.

use super::*;
use crate::protect::allow;
use crate::sheet::{Link, Note};

impl Workbook {
    /// Sets (or with `None` removes) the note of a cell.
    pub fn set_note(&mut self, sheet: usize, c: CellRef, note: Option<Note>) {
        let label = if note.is_some() { "undo.note" } else { "undo.delete_note" };
        self.set_extra(sheet, c, label, |x| x.note = note);
    }

    /// Sets (or with `None` removes) the hyperlink of a cell.
    pub fn set_link(&mut self, sheet: usize, c: CellRef, link: Option<Link>) {
        let label = if link.is_some() { "undo.link" } else { "undo.remove_link" };
        self.set_extra(sheet, c, label, |x| x.link = link);
    }

    fn set_extra(&mut self, sheet: usize, c: CellRef, label: &'static str, f: impl FnOnce(&mut crate::sheet::CellExtra)) {
        let old = self.sheets[sheet].get(c);
        let mut cell = old.cloned().unwrap_or_else(|| Cell::new(Content::Empty, 0));
        cell.set_extra(f);
        let new = (!cell.is_blank()).then_some(cell);
        if new.as_ref() != old {
            self.execute_as(Op::SetCells { sheet, cells: vec![(c, new)] }, label);
        }
    }

    /// Removes the notes (or the hyperlinks) of the cells in `r`.
    pub fn clear_extras(&mut self, sheet: usize, r: &Range, links: bool) {
        let mut cells = Vec::new();
        self.sheets[sheet].for_each_in(r, |c, cell| {
            let has = if links { cell.link().is_some() } else { cell.note().is_some() };
            if has {
                let mut cell = cell.clone();
                cell.set_extra(|x| if links { x.link = None } else { x.note = None });
                cells.push((c, (!cell.is_blank()).then_some(cell)));
            }
        });
        if !cells.is_empty() {
            self.execute_as(Op::SetCells { sheet, cells }, if links { "undo.remove_link" } else { "undo.delete_note" });
        }
    }

    /// Shows or hides the notes of the cells in `r` (all notes with `None`).
    pub fn show_notes(&mut self, sheet: usize, r: Option<&Range>, visible: bool) {
        let mut cells = Vec::new();
        let all = Range::new(CellRef::new(0, 0), CellRef::new(MAX_ROWS - 1, MAX_COLS - 1));
        self.sheets[sheet].for_each_in(r.unwrap_or(&all), |c, cell| {
            if cell.note().is_some_and(|n| n.visible != visible) {
                let mut cell = cell.clone();
                cell.set_extra(|x| {
                    if let Some(n) = &mut x.note {
                        n.visible = visible;
                    }
                });
                cells.push((c, Some(cell)));
            }
        });
        if !cells.is_empty() {
            self.execute_as(Op::SetCells { sheet, cells }, "undo.note");
        }
    }

    /// Cells with a note, in column order.
    pub fn notes(&self, sheet: usize) -> Vec<(CellRef, &Note)> {
        let mut out = Vec::new();
        let s = &self.sheets[sheet];
        if let Some(ext) = s.extent() {
            s.for_each_in(&Range::new(CellRef::new(0, 0), ext), |c, cell| {
                if cell.note().is_some() {
                    out.push(c);
                }
            });
        }
        out.into_iter().filter_map(|c| Some((c, s.get(c)?.note()?))).collect()
    }

    /// Protects (or unprotects) a sheet, or the structure with `None`.
    pub fn set_protection(&mut self, sheet: Option<usize>, protection: Option<Protection>) {
        let label = if protection.is_some() { "undo.protect" } else { "undo.unprotect" };
        self.execute_as(Op::Protect { sheet, protection }, label);
    }

    /// Changes a sheet's page setup (undoable).
    pub fn set_page_setup(&mut self, sheet: usize, page: crate::SheetPage) {
        if self.sheets[sheet].page != page {
            self.execute_as(Op::PageSetup { sheet, page: Box::new(page) }, "undo.page_setup");
        }
    }

    /// Sets or removes the picture tiled behind the cells (undoable).
    pub fn set_background(&mut self, sheet: usize, background: Option<crate::SheetBackground>) {
        let mut page = self.sheets[sheet].page.clone();
        if page.background == background {
            return;
        }
        page.background = background;
        self.execute_as(Op::PageSetup { sheet, page: Box::new(page) }, "undo.background");
    }

    /// Adds a picture on top of the others (undoable); returns its id.
    pub fn add_image(&mut self, sheet: usize, mut img: crate::SheetImage) -> u32 {
        let sh = &self.sheets[sheet];
        img.id = sh.next_image_id();
        if img.name.is_empty() {
            img.name = format!("Picture {}", img.id);
        }
        img.sanitize();
        let id = img.id;
        let mut list = sh.images.clone();
        list.push(img);
        self.execute_as(Op::Images { sheet, list }, "undo.insert_image");
        id
    }

    /// Replaces the picture with the same id (undoable).
    pub fn set_image(&mut self, sheet: usize, mut img: crate::SheetImage) {
        img.sanitize();
        let sh = &self.sheets[sheet];
        let Some(i) = sh.images.iter().position(|x| x.id == img.id) else { return };
        if sh.images[i] == img {
            return;
        }
        let mut list = sh.images.clone();
        list[i] = img;
        self.execute_as(Op::Images { sheet, list }, "undo.image");
    }

    pub fn remove_image(&mut self, sheet: usize, id: u32) {
        let mut list = self.sheets[sheet].images.clone();
        let n = list.len();
        list.retain(|x| x.id != id);
        if list.len() != n {
            self.execute_as(Op::Images { sheet, list }, "undo.delete_image");
        }
    }

    /// Moves a picture to the front (`front`) or to the back.
    pub fn reorder_image(&mut self, sheet: usize, id: u32, front: bool) {
        let mut list = self.sheets[sheet].images.clone();
        let Some(i) = list.iter().position(|x| x.id == id) else { return };
        let img = list.remove(i);
        if front {
            list.push(img);
        } else {
            list.insert(0, img);
        }
        if list != self.sheets[sheet].images {
            self.execute_as(Op::Images { sheet, list }, "undo.image");
        }
    }

    pub fn is_protected(&self, sheet: usize) -> bool {
        self.sheets[sheet].protection.is_some()
    }

    pub fn structure_protected(&self) -> bool {
        self.protection.is_some()
    }

    /// Whether a protected sheet lets users do `flag` ([`allow`]); true
    /// on unprotected sheets.
    pub fn allows(&self, sheet: usize, flag: u32) -> bool {
        self.sheets[sheet].protection.as_ref().is_none_or(|p| p.allows(flag))
    }

    pub fn cell_locked(&self, sheet: usize, c: CellRef) -> bool {
        let s = &self.sheets[sheet];
        s.protection.is_some() && !self.styles.get(s.get(c).map_or(0, |x| x.style)).unlocked && !self.in_open_edit_range(sheet, c)
    }

    /// Whether `c` is in an edit range that can be edited now.
    fn in_open_edit_range(&self, sheet: usize, c: CellRef) -> bool {
        self.sheets[sheet].edit_ranges.iter().any(|e| e.is_open() && e.contains(c))
    }

    /// Replaces the ranges editable on a protected sheet (undoable).
    pub fn set_edit_ranges(&mut self, sheet: usize, list: Vec<crate::protect::EditRange>) {
        if self.sheets[sheet].edit_ranges != list {
            self.execute_as(Op::EditRanges { sheet, list }, "undo.edit_ranges");
        }
    }

    /// The edit range whose password would let `c` be edited: `c` is
    /// locked now, and in a range with a password not given yet.
    pub fn edit_range_asking(&self, sheet: usize, c: CellRef) -> Option<usize> {
        if !self.cell_locked(sheet, c) {
            return None;
        }
        self.sheets[sheet].edit_ranges.iter().position(|e| !e.is_open() && e.contains(c))
    }

    /// Gives the password of an edit range; it stays unlocked for the rest
    /// of the session. False when the password is wrong.
    pub fn unlock_edit_range(&mut self, sheet: usize, index: usize, pw: &str) -> bool {
        let Some(e) = self.sheets.get_mut(sheet).and_then(|s| s.edit_ranges.get_mut(index)) else { return false };
        if !e.unlocks(pw) {
            return false;
        }
        e.unlocked = true;
        self.revision += 1;
        true
    }

    /// Whether every cell of `r` may be edited. Whole rows or columns of a
    /// protected sheet only count the used area and the formatted cells.
    pub fn can_edit(&self, sheet: usize, r: &Range) -> bool {
        let s = &self.sheets[sheet];
        if s.protection.is_none() {
            return true;
        }
        // Blank cells use the default style, which is locked.
        r.cell_count() <= 1 << 16
            && (r.start.row..=r.end.row).all(|row| (r.start.col..=r.end.col).all(|col| !self.cell_locked(sheet, CellRef::new(row, col))))
    }

    /// Whether the formula of a cell is hidden by protection.
    pub fn formula_hidden(&self, sheet: usize, c: CellRef) -> bool {
        let s = &self.sheets[sheet];
        s.protection.is_some() && self.styles.get(s.get(c).map_or(0, |x| x.style)).formula_hidden
    }

    /// Whether protection lets `op` through.
    pub fn allowed(&self, op: &Op) -> bool {
        let sheet_ok = |i: usize, flag: u32| self.sheets.get(i).is_none_or(|s| s.protection.as_ref().is_none_or(|p| p.allows(flag)));
        let structure = self.protection.is_none();
        match op {
            Op::SetCells { sheet, cells } => {
                let Some(p) = self.sheets.get(*sheet).and_then(|s| s.protection.as_ref()) else { return true };
                let s = &self.sheets[*sheet];
                cells.iter().all(|(c, new)| {
                    let old = s.get(*c);
                    if old == new.as_ref() {
                        return true;
                    }
                    let (os, ns) = (old.map_or(0, |x| x.style), new.as_ref().map_or(0, |x| x.style));
                    let (a, b) = (self.styles.get(os), self.styles.get(ns));
                    let locked = !a.unlocked && !self.in_open_edit_range(*sheet, *c);
                    let new = new.as_ref();
                    let empty = Content::Empty;
                    if locked && old.map_or(&empty, |x| &x.content) != new.map_or(&empty, |x| &x.content) {
                        return false;
                    }
                    // Notes count as objects.
                    if locked && old.and_then(|x| x.extra.as_deref()) != new.and_then(|x| x.extra.as_deref()) && !p.allows(allow::OBJECTS) {
                        return false;
                    }
                    if os != ns {
                        if a.unlocked != b.unlocked || a.formula_hidden != b.formula_hidden {
                            return false;
                        }
                        if locked && !p.allows(allow::FORMAT_CELLS) {
                            return false;
                        }
                    }
                    true
                })
            }
            Op::InsertRows { sheet, .. } | Op::InsertRowsAt { sheet, .. } => sheet_ok(*sheet, allow::INSERT_ROWS),
            Op::DeleteRows { sheet, .. } | Op::DeleteRowsAt { sheet, .. } => sheet_ok(*sheet, allow::DELETE_ROWS),
            Op::InsertCols { sheet, .. } => sheet_ok(*sheet, allow::INSERT_COLUMNS),
            Op::DeleteCols { sheet, .. } => sheet_ok(*sheet, allow::DELETE_COLUMNS),
            Op::ColWidths { sheet, .. } => sheet_ok(*sheet, allow::FORMAT_COLUMNS),
            Op::RowHeights { sheet, .. } => sheet_ok(*sheet, allow::FORMAT_ROWS),
            Op::Filter { sheet, .. } => sheet_ok(*sheet, allow::AUTOFILTER),
            Op::CondFormats { sheet, .. } => sheet_ok(*sheet, allow::FORMAT_CELLS),
            Op::Merges { sheet, .. } | Op::EditRanges { sheet, .. } | Op::Validations { sheet, .. } | Op::Outline { sheet, .. } | Op::Tables { sheet, .. } => sheet_ok(*sheet, 0),
            Op::Images { sheet, .. } | Op::Charts { sheet, .. } | Op::Sparklines { sheet, .. } | Op::Shapes { sheet, .. } => sheet_ok(*sheet, allow::OBJECTS),
            Op::Freeze { .. } | Op::Protect { .. } | Op::PageSetup { .. } | Op::Analysis(_) | Op::CellStyles(_) | Op::Names(_) | Op::Externals(_) | Op::Queries(_) | Op::FileParts(_) | Op::Checks(_) | Op::Calc(_) => true,
            Op::AddSheet { .. } | Op::RemoveSheet { .. } | Op::RenameSheet { .. } | Op::SheetProps { .. } | Op::MoveSheet { .. } => structure,
            // Filtering hides rows even where row formatting is not allowed.
            Op::Batch(ops) if ops.iter().any(|o| matches!(o, Op::Filter { .. })) => ops.iter().all(|o| matches!(o, Op::RowHeights { .. }) || self.allowed(o)),
            Op::Batch(ops) => ops.iter().all(|o| self.allowed(o)),
        }
    }

    /// Whether a change was refused by protection since the last call.
    pub fn take_blocked(&mut self) -> bool {
        std::mem::take(&mut self.blocked)
    }

    /// Whether the cursor may land on `c` of a protected sheet.
    pub fn can_select(&self, sheet: usize, c: CellRef) -> bool {
        let flag = if self.cell_locked(sheet, c) { allow::SELECT_LOCKED } else { allow::SELECT_UNLOCKED };
        self.allows(sheet, flag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_and_protection() {
        let mut wb = Workbook::new();
        let a1 = CellRef::new(0, 0);
        let b1 = CellRef::new(0, 1);
        wb.set_input(0, a1, "x");
        wb.set_note(0, a1, Some(Note { text: "hi".into(), author: "me".into(), ..Default::default() }));
        wb.set_link(0, b1, Some(Link { target: "#Sheet1!C3".into(), tip: String::new() }));
        // Editing keeps the note; clearing contents too.
        wb.set_input(0, a1, "y");
        assert_eq!(wb.sheets[0].get(a1).unwrap().note().unwrap().text, "hi");
        wb.clear_contents(0, &Range::single(a1));
        assert!(wb.sheets[0].get(a1).unwrap().note().is_some());
        assert_eq!(wb.notes(0).len(), 1);
        wb.show_notes(0, None, true);
        assert!(wb.notes(0)[0].1.visible);
        wb.clear_extras(0, &Range::new(a1, b1), true);
        assert!(wb.sheets[0].get(b1).is_none());
        wb.undo();
        assert!(wb.sheets[0].get(b1).unwrap().link().is_some());
        wb.set_note(0, a1, None);
        assert!(wb.sheets[0].get(a1).is_none());

        // Protection: locked cells refuse edits; unlocked ones take them.
        let c2 = CellRef::new(1, 2);
        wb.modify_style(0, &Range::single(c2), |s| s.unlocked = true);
        wb.enforce_protection = true;
        wb.set_protection(Some(0), Some(Protection::default()));
        wb.set_input(0, a1, "no");
        assert!(wb.take_blocked());
        assert!(wb.sheets[0].get(a1).is_none());
        wb.set_input(0, c2, "yes");
        assert!(!wb.take_blocked());
        assert_eq!(wb.display(0, c2).0, "yes");
        wb.modify_style(0, &Range::single(a1), |s| s.bold = true);
        assert!(wb.take_blocked());
        wb.execute(Op::InsertRows { sheet: 0, at: 0, count: 1 });
        assert!(wb.take_blocked());
        assert!(wb.can_edit(0, &Range::single(c2)) && !wb.can_edit(0, &Range::new(a1, c2)));
        // Structure.
        wb.set_protection(None, Some(Protection::default()));
        wb.execute(Op::RenameSheet { index: 0, name: "X".into() });
        assert!(wb.take_blocked());
        assert_eq!(wb.sheets[0].name, "Sheet1");
        wb.set_protection(None, None);
        wb.set_protection(Some(0), None);
        wb.set_input(0, a1, "ok");
        assert!(!wb.take_blocked());
    }

    #[test]
    fn edit_ranges_on_protected_sheet() {
        use crate::protect::EditRange;
        let mut wb = Workbook::new();
        let c = |a: &str| CellRef::parse(a).unwrap();
        let r = |a: &str| Range::parse(a).unwrap();
        wb.enforce_protection = true;
        wb.set_edit_ranges(0, vec![EditRange::new("Free", vec![r("A1:A3")], ""), EditRange::new("Prices", vec![r("C1:C3")], "pw")]);
        wb.undo();
        assert!(wb.sheets[0].edit_ranges.is_empty());
        wb.redo();
        assert_eq!(wb.sheets[0].edit_ranges.len(), 2);
        wb.set_protection(Some(0), Some(Protection::default()));
        // Ranges can't be changed while the sheet is protected.
        wb.set_edit_ranges(0, Vec::new());
        assert!(wb.take_blocked());
        assert_eq!(wb.sheets[0].edit_ranges.len(), 2);
        // Open range: editable; outside: locked.
        wb.set_input(0, c("A2"), "ok");
        assert!(!wb.take_blocked());
        assert_eq!(wb.display(0, c("A2")).0, "ok");
        assert!(wb.cell_locked(0, c("B2")) && !wb.cell_locked(0, c("A2")));
        assert_eq!(wb.edit_range_asking(0, c("A2")), None);
        assert_eq!(wb.edit_range_asking(0, c("B2")), None);
        // Range with a password: asks for it, then stays open.
        assert_eq!(wb.edit_range_asking(0, c("C2")), Some(1));
        wb.set_input(0, c("C2"), "no");
        assert!(wb.take_blocked());
        assert!(!wb.unlock_edit_range(0, 1, "bad"));
        assert!(wb.unlock_edit_range(0, 1, "pw"));
        assert_eq!(wb.edit_range_asking(0, c("C2")), None);
        wb.set_input(0, c("C2"), "yes");
        assert!(!wb.take_blocked());
        assert_eq!(wb.display(0, c("C2")).0, "yes");
        // Ranges follow inserted rows and come back when rows are deleted.
        wb.set_protection(Some(0), None);
        wb.execute(Op::InsertRows { sheet: 0, at: 0, count: 2 });
        assert_eq!(wb.sheets[0].edit_ranges[0].ranges, vec![r("A3:A5")]);
        wb.execute(Op::DeleteRows { sheet: 0, at: 0, count: 10 });
        assert!(wb.sheets[0].edit_ranges.is_empty());
        wb.undo();
        assert_eq!(wb.sheets[0].edit_ranges[1].ranges, vec![r("C3:C5")]);
    }

    #[test]
    fn images_undo_and_structure() {
        use crate::{ImageFormat, SheetImage};
        let mut wb = Workbook::new();
        let img = SheetImage::new(std::sync::Arc::from(&b"\x89PNG\r\n\x1a\n"[..]), ImageFormat::Png, CellRef::new(5, 2), 40.0, 30.0);
        let id = wb.add_image(0, img.clone());
        let id2 = wb.add_image(0, img);
        assert_eq!((id, id2), (1, 2));
        assert_eq!(wb.sheets[0].images[0].name, "Picture 1");
        wb.execute(Op::InsertRows { sheet: 0, at: 0, count: 2 });
        assert_eq!(wb.sheets[0].images[0].from, CellRef::new(7, 2));
        wb.execute(Op::DeleteRows { sheet: 0, at: 6, count: 3 });
        assert_eq!(wb.sheets[0].images[0].from, CellRef::new(6, 2));
        wb.undo();
        assert_eq!(wb.sheets[0].images[0].from, CellRef::new(7, 2));
        wb.execute(Op::DeleteRowsAt { sheet: 0, rows: vec![1, 7] });
        assert_eq!(wb.sheets[0].images[1].from, CellRef::new(6, 2));
        wb.undo();
        wb.execute(Op::DeleteCols { sheet: 0, at: 0, count: 1 });
        assert_eq!(wb.sheets[0].images[0].from.col, 1);
        wb.undo();
        let mut moved = wb.sheets[0].images[1].clone();
        moved.rotation = 45.0;
        wb.set_image(0, moved);
        assert_eq!(wb.sheets[0].image(2).unwrap().rotation, 45.0);
        wb.reorder_image(0, 2, false);
        assert_eq!(wb.sheets[0].images[0].id, 2);
        wb.remove_image(0, 1);
        assert_eq!(wb.sheets[0].images.len(), 1);
        wb.undo();
        wb.undo();
        wb.undo();
        assert_eq!(wb.sheets[0].image(2).unwrap().rotation, 0.0);
        // Objects protection blocks picture edits.
        wb.enforce_protection = true;
        wb.set_protection(Some(0), Some(Protection::default()));
        wb.remove_image(0, 1);
        assert!(wb.take_blocked());
        assert_eq!(wb.sheets[0].images.len(), 2);
        // Pictures widen the printed area.
        let a = wb.sheets[0].page.area(&wb.sheets[0]).unwrap();
        assert!(a.end.row >= 7);
    }
}
