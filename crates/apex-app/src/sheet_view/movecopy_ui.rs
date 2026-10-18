//! Move or Copy a sheet, within the workbook or to another one.

use super::*;
use crate::common::AppCmd;
use crate::widgets::DialogResult;

/// Where the sheet goes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Dest {
    This,
    /// Another open workbook (tab id).
    Book(u64),
    New,
}

pub(super) struct MoveCopyDlg {
    sheet: usize,
    dest: Dest,
    /// Index of the sheet to go before (past the end: at the end).
    before: usize,
    copy: bool,
}

impl SheetView {
    pub(super) fn open_move_copy(&mut self, sheet: usize) {
        self.settle_edit();
        self.move_copy = Some(MoveCopyDlg { sheet, dest: Dest::This, before: sheet + 1, copy: false });
    }

    /// The dialog is open and needs the sheets of the other workbooks.
    pub fn wants_books(&self) -> bool {
        self.move_copy.is_some()
    }

    pub(super) fn move_copy_dialog(&mut self, ctx: &egui::Context, shell: &mut Shell) {
        let Some(mut d) = self.move_copy.take() else { return };
        if let Dest::Book(id) = d.dest {
            if !shell.books.iter().any(|b| b.0 == id) {
                d.dest = Dest::This;
            }
        }
        // Moving the last visible sheet out would leave the workbook empty.
        let must_copy = d.dest != Dest::This && self.visible_sheets() < 2 && !self.wb.sheets[d.sheet].hidden;
        let names: Vec<String> = match d.dest {
            Dest::This => self.wb.sheets.iter().map(|s| s.name.clone()).collect(),
            Dest::Book(id) => shell.books.iter().find(|b| b.0 == id).map(|b| b.2.clone()).unwrap_or_default(),
            Dest::New => Vec::new(),
        };
        let res = widgets::dialog(ctx, "move_copy", tr("sheet.move_copy_title"), 340.0, |ui| {
            ui.label(trf("sheet.move_copy_what", &[("name", &self.wb.sheets[d.sheet].name)]));
            ui.add_space(4.0);
            ui.label(tr("sheet.to_book"));
            let label = |dest: Dest| match dest {
                Dest::This => shell.file_name.clone(),
                Dest::Book(id) => shell.books.iter().find(|b| b.0 == id).map(|b| b.1.clone()).unwrap_or_default(),
                Dest::New => tr("sheet.new_book").to_string(),
            };
            let before = d.dest;
            egui::ComboBox::from_id_salt("move_copy_book").width(ui.available_width()).selected_text(label(d.dest)).show_ui(ui, |ui| {
                ui.selectable_value(&mut d.dest, Dest::New, label(Dest::New));
                ui.selectable_value(&mut d.dest, Dest::This, label(Dest::This));
                for b in &shell.books {
                    ui.selectable_value(&mut d.dest, Dest::Book(b.0), b.1.clone());
                }
            });
            if d.dest != before {
                d.before = usize::MAX;
            }
            ui.add_space(4.0);
            ui.label(tr("sheet.before_sheet"));
            ui.add_enabled_ui(d.dest != Dest::New, |ui| {
                egui::Frame::new().stroke(ui.visuals().widgets.noninteractive.bg_stroke).inner_margin(4.0).show(ui, |ui| {
                    egui::ScrollArea::vertical().max_height(160.0).auto_shrink([false, true]).show(ui, |ui| {
                        for (i, n) in names.iter().enumerate() {
                            if ui.selectable_label(d.before == i, n).clicked() {
                                d.before = i;
                            }
                        }
                        if ui.selectable_label(d.before >= names.len(), tr("sheet.move_to_end")).clicked() {
                            d.before = usize::MAX;
                        }
                    });
                });
            });
            ui.add_space(4.0);
            let mut copy = d.copy || must_copy;
            if ui.add_enabled(!must_copy, egui::Checkbox::new(&mut copy, tr("sheet.create_copy"))).changed() {
                d.copy = copy;
            }
        });
        match res {
            DialogResult::None => self.move_copy = Some(d),
            DialogResult::Cancel => self.focus_grid = true,
            DialogResult::Ok => {
                let copy = d.copy || must_copy;
                if self.wb.protection.is_some() && (d.dest == Dest::This || !copy) {
                    self.notices.push(Notice::error(tr("sheet.book_protected")));
                    self.move_copy = Some(d);
                    return;
                }
                self.focus_grid = true;
                match d.dest {
                    Dest::This => self.move_copy_here(d.sheet, d.before, copy),
                    Dest::Book(id) => shell.cmd = Some(AppCmd::SheetToBook { sheet: d.sheet, target: Some(id), before: d.before, copy }),
                    Dest::New => shell.cmd = Some(AppCmd::SheetToBook { sheet: d.sheet, target: None, before: 0, copy }),
                }
            }
        }
    }

    /// Moves or copies `sheet` before `before` in this workbook.
    fn move_copy_here(&mut self, sheet: usize, before: usize, copy: bool) {
        let before = before.min(self.wb.sheets.len());
        if copy {
            let k = self.wb.duplicate_sheet(sheet);
            // The copy sits after the original; take it where asked.
            let to = if before > sheet { before } else { before.min(k) };
            if to != k {
                self.wb.execute(Op::MoveSheet { from: k, to });
                self.wb.merge_undo(2, "undo.copy_sheet");
            }
            self.select_sheet(to);
        } else {
            let to = if before > sheet { before - 1 } else { before };
            if to != sheet {
                self.wb.execute(Op::MoveSheet { from: sheet, to });
                self.sheet = to;
                self.buf_key = None;
            }
        }
    }

    /// Takes in sheet `i` of another workbook before `before` and shows it.
    pub fn adopt_sheet(&mut self, from: &Workbook, i: usize, before: usize) {
        self.settle_edit();
        let k = self.wb.adopt_sheet(from, i, before);
        if self.wb.take_blocked() {
            self.notices.push(Notice::error(tr("sheet.book_protected")));
            return;
        }
        self.select_sheet(k);
    }

    /// Removes sheet `i` after it moved to another workbook.
    pub fn sheet_moved_out(&mut self, i: usize) {
        self.remove_sheet_now(i);
    }

    /// Ctrl+click on a tab adds it to the group or takes it out;
    /// Shift+click groups the sheets from the active one to it.
    pub(super) fn group_tab(&mut self, i: usize, range: bool) {
        self.settle_edit();
        let g = &mut self.wb.group;
        if range {
            let (a, b) = (self.sheet.min(i), self.sheet.max(i));
            *g = (a..=b).filter(|&k| !self.wb.sheets[k].hidden).collect();
        } else {
            if g.is_empty() {
                g.push(self.sheet);
            }
            if let Some(k) = g.iter().position(|&x| x == i) {
                if i != self.sheet {
                    g.remove(k);
                }
            } else {
                g.push(i);
            }
            g.sort_unstable();
        }
        if self.wb.group.len() < 2 {
            self.wb.group.clear();
        }
    }

    /// Groups every visible sheet (or ungroups them).
    pub(super) fn group_all(&mut self, on: bool) {
        self.wb.group = if on { (0..self.wb.sheets.len()).filter(|&k| !self.wb.sheets[k].hidden).collect() } else { Vec::new() };
        if self.wb.group.len() < 2 {
            self.wb.group.clear();
        }
    }
}
