//! The PDF tab's Pages toolstrip tab and the page work of the thumbnails:
//! choosing pages (Ctrl and Shift clicks), dragging them to a new place,
//! turning, removing, duplicating and extracting them, adding blank pages
//! or pages of other files, and splitting the file.

use std::collections::BTreeSet;

use apex_io::pdf::edit::organize::parse_ranges;
use eframe::egui;

use super::{PdfView, Request};
use crate::i18n::{tr, trf};
use crate::icons::Icon;
use crate::toolstrip::{self, Body};
use crate::widgets::{self, DialogResult};

/// How to split the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitBy {
    Ranges,
    Every,
    Bookmarks,
}

pub struct SplitBox {
    pub by: SplitBy,
    pub ranges: String,
    pub every: usize,
    pub bad: bool,
}

impl PdfView {
    /// The pages chosen in the thumbnails, or the current one.
    pub fn chosen_pages(&self) -> Vec<usize> {
        let n = self.page_count();
        let v: Vec<usize> = self.page_sel.iter().copied().filter(|&i| i < n).collect();
        if v.is_empty() { vec![self.cur.min(n.saturating_sub(1))] } else { v }
    }

    /// A click on thumbnail `i`: with Ctrl it joins or leaves the chosen
    /// pages, with Shift the pages from the current one to it are chosen.
    pub fn click_thumb(&mut self, i: usize, m: egui::Modifiers) {
        if m.command {
            if !self.page_sel.remove(&i) {
                if self.page_sel.is_empty() {
                    self.page_sel.insert(self.cur);
                }
                self.page_sel.insert(i);
            }
        } else if m.shift {
            let (a, b) = (self.cur.min(i), self.cur.max(i));
            self.page_sel = (a..=b).collect();
        } else {
            self.page_sel.clear();
        }
        if !m.shift {
            self.go_to(i, 0.0);
        }
    }

    /// After pages changed: the choice goes, the view stays in range.
    pub(super) fn pages_changed(&mut self, sel: BTreeSet<usize>) {
        self.after_change();
        self.page_sel = sel;
        let n = self.page_count();
        if self.cur >= n {
            self.go_to(n.saturating_sub(1), 0.0);
        }
    }

    pub fn rotate_chosen(&mut self, quarter: i64) {
        let which = self.chosen_pages();
        self.doc.rotate_pages(&which, quarter);
        let sel = self.page_sel.clone();
        self.pages_changed(sel);
    }

    pub fn delete_chosen(&mut self) {
        let which = self.chosen_pages();
        if !self.doc.delete_pages(&which) {
            self.notices.push(crate::app::Notice::info(tr("pdf.pages_keep_one")));
            return;
        }
        self.pages_changed(BTreeSet::new());
    }

    pub fn duplicate_chosen(&mut self) {
        let which = self.chosen_pages();
        self.doc.duplicate_pages(&which);
        self.pages_changed(BTreeSet::new());
    }

    /// Moves the chosen pages (or page `i` when it is not among them)
    /// before page `to`.
    pub fn move_to(&mut self, i: usize, to: usize) {
        let mut which = self.chosen_pages();
        if !which.contains(&i) {
            which = vec![i];
        }
        let before = which.iter().filter(|&&p| p < to).count();
        self.doc.move_pages(&which, to);
        let first = to - before;
        self.pages_changed((first..first + which.len()).collect());
        self.go_to(first, 0.0);
    }

    pub fn insert_blank_page(&mut self, after: bool) {
        let at = self.cur + usize::from(after);
        self.doc.insert_blank(at);
        self.pages_changed(BTreeSet::new());
        self.go_to(at, 0.0);
    }

    /// Adds the files' pages before page `at` (the app reads them).
    pub fn insert_files(&mut self, files: Vec<(String, Vec<u8>)>, at: usize) -> Result<usize, String> {
        let mut added = 0;
        self.doc.begin("pdf.undo.insert_pages");
        for (name, data) in files {
            match self.doc.insert_from(data, None, &[], at + added) {
                Ok(n) => added += n,
                Err(e) => {
                    self.doc.end();
                    self.pages_changed(BTreeSet::new());
                    return Err(format!("{name}: {e}"));
                }
            }
        }
        self.doc.end();
        self.pages_changed(BTreeSet::new());
        Ok(added)
    }

    pub(super) fn pages_tab(&mut self, b: &mut Body) {
        let acc = b.accent();
        let n = self.chosen_pages().len();
        b.group("pdf.g.pages", Icon::MultiPage, |ui| {
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::Redo), tr("pdf.rotate_right"), tr("pdf.rotate_pages_tip"), acc, false).clicked() {
                    self.rotate_chosen(1);
                }
                if toolstrip::small(ui, Some(Icon::Undo), tr("pdf.rotate_left"), tr("pdf.rotate_pages_tip"), acc, false).clicked() {
                    self.rotate_chosen(-1);
                }
            });
            if toolstrip::large(ui, Icon::Reject, tr("pdf.delete_pages"), &trf("pdf.delete_pages_tip", &[("n", &n)]), acc, false).clicked() {
                self.delete_chosen();
            }
            if toolstrip::large(ui, Icon::Copy, tr("pdf.duplicate_pages"), tr("pdf.duplicate_pages_tip"), acc, false).clicked() {
                self.duplicate_chosen();
            }
            if toolstrip::large(ui, Icon::Export, tr("pdf.extract_pages"), tr("pdf.extract_pages_tip"), acc, false).clicked() {
                self.requests.push(Request::Extract(self.chosen_pages()));
            }
        });
        b.group("pdf.g.insert", Icon::New, |ui| {
            if toolstrip::large(ui, Icon::New, tr("pdf.insert_blank"), tr("pdf.insert_blank_tip"), acc, false).clicked() {
                self.insert_blank_page(true);
            }
            if toolstrip::large(ui, Icon::Folder, tr("pdf.insert_file"), tr("pdf.insert_file_tip"), acc, false).clicked() {
                self.requests.push(Request::InsertFile(self.cur + 1));
            }
        });
        b.group("pdf.g.files", Icon::Merge, |ui| {
            if toolstrip::large(ui, Icon::Merge, tr("pdf.merge"), tr("pdf.merge_tip"), acc, false).clicked() {
                self.requests.push(Request::InsertFile(self.page_count()));
            }
            if toolstrip::large(ui, Icon::Split, tr("pdf.split"), tr("pdf.split_tip"), acc, false).clicked() {
                self.split_box = Some(SplitBox { by: SplitBy::Ranges, ranges: String::new(), every: 1, bad: false });
            }
        });
        b.group("pdf.g.number", Icon::PageNumber, |ui| {
            if toolstrip::large(ui, Icon::PageNumber, tr("pdf.stamp"), tr("pdf.stamp_tip"), acc, false).clicked() {
                self.open_stamp();
            }
        });
    }

    /// The pages' context menu (thumbnails).
    pub(super) fn thumb_menu(&mut self, ui: &mut egui::Ui) {
        if ui.button(tr("pdf.rotate_right")).clicked() {
            self.rotate_chosen(1);
            ui.close();
        }
        if ui.button(tr("pdf.rotate_left")).clicked() {
            self.rotate_chosen(-1);
            ui.close();
        }
        if ui.button(tr("pdf.duplicate_pages")).clicked() {
            self.duplicate_chosen();
            ui.close();
        }
        if ui.button(tr("pdf.extract_pages")).clicked() {
            self.requests.push(Request::Extract(self.chosen_pages()));
            ui.close();
        }
        if ui.button(tr("pdf.insert_blank")).clicked() {
            self.insert_blank_page(true);
            ui.close();
        }
        ui.separator();
        if ui.button(tr("pdf.delete_pages")).clicked() {
            self.delete_chosen();
            ui.close();
        }
    }

    /// The ranges the split dialog asks for, with a name for each file.
    pub fn split_ranges(&self, b: &SplitBox) -> Option<Vec<(String, usize, usize)>> {
        let n = self.page_count();
        let v: Vec<(String, usize, usize)> = match b.by {
            SplitBy::Ranges => parse_ranges(&b.ranges, n)?.into_iter().map(|(a, z)| (String::new(), a, z)).collect(),
            SplitBy::Every => {
                let k = b.every.max(1);
                (0..n).step_by(k).map(|a| (String::new(), a, (a + k - 1).min(n - 1))).collect()
            }
            SplitBy::Bookmarks => self.doc.bookmark_ranges(),
        };
        (!v.is_empty()).then_some(v)
    }

    pub(super) fn split_ui(&mut self, ctx: &egui::Context) {
        let Some(mut b) = self.split_box.take() else { return };
        let n = self.page_count();
        let res = widgets::dialog(ctx, "pdf_split", tr("pdf.split"), 360.0, |ui| {
            ui.radio_value(&mut b.by, SplitBy::Ranges, tr("pdf.split_ranges"));
            ui.indent("ranges", |ui| {
                let r = ui.add_enabled(b.by == SplitBy::Ranges, egui::TextEdit::singleline(&mut b.ranges).hint_text("1-3, 4-").desired_width(200.0));
                if r.changed() {
                    b.bad = false;
                }
            });
            ui.radio_value(&mut b.by, SplitBy::Every, tr("pdf.split_every"));
            ui.indent("every", |ui| {
                ui.add_enabled(b.by == SplitBy::Every, egui::DragValue::new(&mut b.every).range(1..=n.max(1)));
            });
            ui.radio_value(&mut b.by, SplitBy::Bookmarks, tr("pdf.split_bookmarks"));
            if b.bad {
                ui.colored_label(ui.visuals().error_fg_color, tr("pdf.split_bad"));
            }
        });
        match res {
            DialogResult::Ok => match self.split_ranges(&b) {
                Some(r) => self.requests.push(Request::Split(r)),
                None => {
                    b.bad = true;
                    self.split_box = Some(b);
                }
            },
            DialogResult::Cancel => {}
            DialogResult::None => self.split_box = Some(b),
        }
    }
}
