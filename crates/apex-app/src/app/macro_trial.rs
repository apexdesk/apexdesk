//! Run Macro > Try: the macro runs on a copy of the workbook or document,
//! as usual but changing no file (it stops where it would), saving nothing
//! and leaving no OnKey/OnTime behind; then what it changed (cells, sheets,
//! paragraphs) is listed to apply it or throw it away. For macros received
//! from others.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use apex_sheet::{CellRef, Workbook};
use eframe::egui::{self, RichText};

use super::{App, View};
use crate::i18n::{tr, trf};

/// Cells listed at most (the count is of all).
const LIST: usize = 2000;

/// What a run changed in the workbook.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Changes {
    /// (sheet, cell, before, after): what the cell holds, as edited.
    pub cells: Vec<(String, CellRef, String, String)>,
    /// All the cells whose content changed.
    pub values: usize,
    /// Cells whose format alone changed.
    pub formats: usize,
    pub sheets_added: Vec<String>,
    pub sheets_removed: Vec<String>,
    /// (paragraph, before, after): a document's paragraphs whose text
    /// changed (1-based, as the document is after; empty when one went or
    /// came).
    pub paras: Vec<(usize, String, String)>,
    /// All the paragraphs whose text changed, and those whose formatting
    /// alone did.
    pub para_texts: usize,
    pub para_formats: usize,
}

/// The paragraphs that differ between `before` and `after`: those between
/// the same first and last ones, side by side.
pub(super) fn doc_changes(before: &apex_doc::Document, after: &apex_doc::Document) -> Changes {
    let mut c = Changes::default();
    let (b, a) = (&before.paragraphs, &after.paragraphs);
    let head = b.iter().zip(a.iter()).take_while(|(x, y)| x == y).count();
    let tail = b[head..].iter().rev().zip(a[head..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (b, a) = (&b[head..b.len() - tail], &a[head..a.len() - tail]);
    for i in 0..b.len().max(a.len()) {
        let (x, y) = (b.get(i), a.get(i));
        let (tx, ty) = (x.map_or("", |p| p.text.as_str()), y.map_or("", |p| p.text.as_str()));
        if x == y {
            continue;
        }
        if tx == ty && x.is_some() == y.is_some() {
            c.para_formats += 1;
            continue;
        }
        c.para_texts += 1;
        if c.paras.len() < LIST {
            let shown = |t: &str| t.replace([apex_doc::OBJECT, apex_doc::LINE_BREAK], " ");
            c.paras.push((head + i + 1, shown(tx), shown(ty)));
        }
    }
    c
}

/// The cells and sheets that differ between `before` and `after`
/// (sheets matched by name).
pub(super) fn changes(before: &Workbook, after: &Workbook) -> Changes {
    let mut c = Changes::default();
    let index = |wb: &Workbook, name: &str| wb.sheets.iter().position(|s| s.name == name);
    for s in &after.sheets {
        if index(before, &s.name).is_none() {
            c.sheets_added.push(s.name.clone());
        }
    }
    for (bi, s) in before.sheets.iter().enumerate() {
        let Some(ai) = index(after, &s.name) else {
            c.sheets_removed.push(s.name.clone());
            continue;
        };
        let (b, a) = (&before.sheets[bi], &after.sheets[ai]);
        let refs: BTreeSet<(u32, u32)> = b.iter().chain(a.iter()).map(|(r, _)| (r.col, r.row)).collect();
        for (col, row) in refs {
            let r = CellRef::new(row, col);
            let (x, y) = (b.get(r), a.get(r));
            if content(x) != content(y) {
                c.values += 1;
                if c.cells.len() < LIST {
                    c.cells.push((s.name.clone(), r, before.edit_text(bi, r), after.edit_text(ai, r)));
                }
            } else if x.map_or(0, |c| c.style) != y.map_or(0, |c| c.style) {
                c.formats += 1;
            }
        }
    }
    c
}

/// What a cell holds, nothing when empty.
fn content(cell: Option<&apex_sheet::Cell>) -> Option<&apex_sheet::Content> {
    cell.map(|c| &c.content).filter(|c| !matches!(c, apex_sheet::Content::Empty))
}

/// What a try left: a workbook or a document.
enum Tried {
    Book(Box<Workbook>, Box<apex_vba::Outcome>),
    Doc(Box<apex_doc::Document>, Box<apex_vba::DocOutcome>),
}

/// A try that ended: what it left, to apply or not.
pub(crate) struct Trial {
    tab: u64,
    name: String,
    tried: Tried,
    pub(super) changes: Changes,
    /// Folders where it wanted to change files (it stopped there).
    pub(super) files: Vec<String>,
}

impl App {
    /// The try of macro `name` of tab `id` ended with `wb` and `out`.
    pub(super) fn finish_trial(&mut self, id: u64, name: String, wb: Workbook, out: apex_vba::Outcome, files: &Arc<Mutex<Vec<String>>>) {
        let Some(t) = self.tabs.iter().find(|t| t.id == id) else { return };
        let View::Sheet(v) = &t.view else { return };
        let changes = if out.changed { changes(&v.wb, &wb) } else { Changes::default() };
        let files = files.lock().map(|f| f.clone()).unwrap_or_default();
        self.macro_run.report = Some(Trial { tab: id, name, tried: Tried::Book(Box::new(wb), Box::new(out)), changes, files });
    }

    /// [`Self::finish_trial`] for a document.
    pub(super) fn finish_doc_trial(&mut self, id: u64, name: String, doc: apex_doc::Document, out: apex_vba::DocOutcome, files: &Arc<Mutex<Vec<String>>>) {
        let Some(t) = self.tabs.iter().find(|t| t.id == id) else { return };
        let View::Doc(v) = &t.view else { return };
        let changes = if out.changed { doc_changes(&v.doc, &doc) } else { Changes::default() };
        let files = files.lock().map(|f| f.clone()).unwrap_or_default();
        self.macro_run.report = Some(Trial { tab: id, name, tried: Tried::Doc(Box::new(doc), Box::new(out)), changes, files });
    }

    /// The report of a try, with Apply and Discard.
    pub(super) fn trial_dialog(&mut self, ctx: &egui::Context) {
        let Some(t) = &self.macro_run.report else { return };
        if !self.tabs.iter().any(|x| x.id == t.tab) {
            self.macro_run.report = None;
            return;
        }
        let (mut apply, mut discard) = (false, false);
        let c = &t.changes;
        let (changed, error, save) = match &t.tried {
            Tried::Book(_, out) => (out.changed, &out.error, out.save),
            Tried::Doc(_, out) => (out.changed, &out.error, out.save),
        };
        let nothing = !changed;
        let m = egui::Modal::new(egui::Id::new("macro_trial")).show(ctx, |ui| {
            ui.set_width(620.0);
            ui.heading(trf("macros.trial_title", &[("name", &t.name)]));
            ui.label(RichText::new(tr("macros.trial_note")).weak());
            ui.add_space(6.0);
            if let Some(e) = error {
                ui.label(RichText::new(trf("macros.trial_stopped", &[("error", &e.to_string())])).color(ui.visuals().warn_fg_color));
            }
            for f in &t.files {
                ui.label(RichText::new(trf("macros.trial_files", &[("folder", f)])).color(ui.visuals().warn_fg_color));
            }
            if save {
                ui.label(tr("macros.trial_save"));
            }
            if nothing {
                ui.label(tr("macros.trial_nothing"));
            } else {
                if c.values > 0 {
                    ui.label(trf("macros.trial_cells", &[("n", &c.values)]));
                }
                if c.formats > 0 {
                    ui.label(trf("macros.trial_formats", &[("n", &c.formats)]));
                }
                if !c.sheets_added.is_empty() {
                    ui.label(trf("macros.trial_added", &[("list", &c.sheets_added.join(", "))]));
                }
                if !c.sheets_removed.is_empty() {
                    ui.label(RichText::new(trf("macros.trial_removed", &[("list", &c.sheets_removed.join(", "))])).color(ui.visuals().warn_fg_color));
                }
                if c.para_texts > 0 {
                    ui.label(trf("macros.trial_paras", &[("n", &c.para_texts)]));
                }
                if c.para_formats > 0 {
                    ui.label(trf("macros.trial_para_formats", &[("n", &c.para_formats)]));
                }
                if c.values + c.formats + c.para_texts + c.para_formats == 0 && c.sheets_added.is_empty() && c.sheets_removed.is_empty() {
                    ui.label(tr("macros.trial_other"));
                }
            }
            if !c.paras.is_empty() {
                ui.add_space(6.0);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let row_h = ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().item_spacing.y;
                    egui::ScrollArea::vertical().max_height(260.0).show_rows(ui, row_h, c.paras.len(), |ui, rows| {
                        egui::Grid::new("macro_trial_paras").num_columns(3).striped(true).show(ui, |ui| {
                            if rows.start == 0 {
                                for k in ["macros.trial_para", "macros.trial_before", "macros.trial_after"] {
                                    ui.label(RichText::new(tr(k)).strong());
                                }
                                ui.end_row();
                            }
                            for (n, before, after) in &c.paras[rows] {
                                ui.label(n.to_string());
                                ui.label(RichText::new(short(before)).weak()).on_hover_text(before.as_str());
                                ui.label(short(after)).on_hover_text(after.as_str());
                                ui.end_row();
                            }
                        });
                    });
                });
            }
            if !c.cells.is_empty() {
                ui.add_space(6.0);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let row_h = ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().item_spacing.y;
                    egui::ScrollArea::vertical().max_height(260.0).show_rows(ui, row_h, c.cells.len(), |ui, rows| {
                        egui::Grid::new("macro_trial_cells").num_columns(3).striped(true).show(ui, |ui| {
                            if rows.start == 0 {
                                for k in ["macros.trial_cell", "macros.trial_before", "macros.trial_after"] {
                                    ui.label(RichText::new(tr(k)).strong());
                                }
                                ui.end_row();
                            }
                            for (sheet, r, before, after) in &c.cells[rows] {
                                ui.label(format!("{sheet}!{}", r.to_a1()));
                                ui.label(RichText::new(short(before)).weak()).on_hover_text(before.as_str());
                                ui.label(short(after)).on_hover_text(after.as_str());
                                ui.end_row();
                            }
                        });
                    });
                });
            }
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                discard = ui.button(tr("macros.trial_discard")).clicked();
                if !nothing && ui.button(RichText::new(tr("macros.trial_apply")).strong()).on_hover_text(tr("macros.trial_apply_tip")).clicked() {
                    apply = true;
                }
            });
        });
        if m.is_top_modal && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            discard = true;
        }
        if !(apply || discard) {
            return;
        }
        let Some(t) = self.macro_run.report.take() else { return };
        if apply && let Some(idx) = self.tabs.iter().position(|x| x.id == t.tab) {
            self.hold(idx);
            match (&mut self.tabs[idx].view, t.tried) {
                (View::Sheet(v), Tried::Book(wb, out)) => {
                    v.macro_done(Some(*wb), out.active_sheet, out.selection);
                    self.events_settled(t.tab);
                    self.macro_run.set_events(t.tab, out.events);
                }
                (View::Doc(v), Tried::Doc(doc, out)) => {
                    v.macro_done(Some(*doc), out.selection);
                    self.macro_run.set_events(t.tab, out.events);
                }
                _ => {}
            }
        }
    }
}

/// A cell's text cut for the list.
fn short(s: &str) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() > 60 { format!("{}…", s.chars().take(60).collect::<String>()) } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_changed() {
        let mut a = Workbook::new();
        a.set_input(0, CellRef::new(0, 0), "1");
        a.set_input(0, CellRef::new(1, 0), "keep");
        let mut b = a.clone();
        b.set_input(0, CellRef::new(0, 0), "2");
        b.set_input(0, CellRef::new(2, 1), "new");
        b.sheets.push(apex_sheet::Sheet::new("Extra"));
        let c = changes(&a, &b);
        assert_eq!(c.values, 2);
        assert_eq!(c.cells[0], ("Sheet1".to_string(), CellRef::new(0, 0), "1".to_string(), "2".to_string()));
        assert_eq!(c.cells[1].1, CellRef::new(2, 1));
        assert_eq!(c.sheets_added, ["Extra"]);
        assert!(c.sheets_removed.is_empty() && c.formats == 0);
        assert_eq!(changes(&a, &a), Changes::default());
    }

    #[test]
    fn paragraphs_that_changed() {
        let a = apex_doc::Document::from_plain_text("one\ntwo\nthree\nfour");
        let mut b = a.clone();
        b.paragraphs[1].text = "TWO".into();
        b.paragraphs.insert(3, apex_doc::Paragraph::new("new"));
        let c = doc_changes(&a, &b);
        assert_eq!(c.paras, [(2, "two".to_string(), "TWO".to_string()), (4, String::new(), "new".to_string())]);
        assert_eq!((c.para_texts, c.para_formats), (2, 0));
        assert_eq!(doc_changes(&a, &a), Changes::default());
    }
}
