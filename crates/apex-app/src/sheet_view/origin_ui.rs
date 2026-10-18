//! Formulas > Where From: the tree of what the cursor cell's number comes
//! from (cells of any sheet, formulas, Get Data queries and the files they
//! read, linked workbooks), with the files' dates, and the formulas that
//! use it. Deleting rows, columns or a sheet other formulas read asks
//! first, listing them.

use std::collections::HashSet;

use apex_sheet::query::Source;
use apex_sheet::{CellRef, Origin, Range, Value};
use eframe::egui::{self, Color32, Id, RichText};

use super::SheetView;
use crate::i18n::{tr, trf};
use crate::widgets::FitWindow as _;

/// Levels of the tree shown at most.
const MAX_DEPTH: usize = 8;
/// Formula cells listed inside a range.
const MAX_CELLS: usize = 30;

#[derive(Default)]
pub(crate) struct OriginState {
    pub open: bool,
    /// A deletion waiting for the user: what, and the formulas it breaks.
    pending: Option<(Deletion, Vec<(usize, CellRef)>)>,
    /// The deletion the user confirmed (done without asking again).
    confirmed: Option<Deletion>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Deletion {
    Rows,
    Cols,
    Sheet(usize),
}

/// A file's modification date, or why there is none.
fn file_date(path: &str) -> String {
    if apex_sheet::query::is_url(path) {
        return tr("org.web").to_string();
    }
    // Looked for every frame: a share would get the user's sign-in each time.
    if cfg!(windows) && crate::links::on_share(path) {
        return tr("links.why_network").to_string();
    }
    match apex_fs::metadata(path).and_then(|m| m.modified()) {
        Ok(t) => {
            let (d, t) = crate::common::date_time_of(crate::common::local_time(t));
            trf("org.modified", &[("date", &format!("{d} {t}"))])
        }
        Err(_) => tr("org.not_found").to_string(),
    }
}

/// Where a query reads from, as text, and the file it reads (if one).
fn source_text(s: &Source) -> (String, Option<String>) {
    match s {
        Source::Text { location, .. } | Source::Json { location, .. } | Source::Xml { location, .. } | Source::Html { location, .. } | Source::Pdf { location, .. } => (location.clone(), Some(location.clone())),
        Source::Workbook { path, item, .. } => (format!("{path} · {item}"), Some(path.clone())),
        Source::Folder { path, .. } => (path.clone(), Some(path.clone())),
        Source::Sqlite { path, .. } | Source::DbFile { path, .. } => (path.clone(), Some(path.clone())),
        Source::Database { host, database, .. } => (format!("{host}/{database}"), None),
        Source::Table { name } => (trf("org.table", &[("name", name)]), None),
        Source::Query { name } => (trf("org.query", &[("name", name)]), None),
    }
}

impl SheetView {
    fn cell_name(&self, sheet: usize, c: CellRef) -> String {
        if sheet == self.sheet { c.to_a1() } else { format!("{}!{}", self.wb.sheets[sheet].name, c.to_a1()) }
    }

    fn range_name(&self, sheet: usize, r: &Range) -> String {
        if sheet == self.sheet { r.to_a1() } else { format!("{}!{}", self.wb.sheets[sheet].name, r.to_a1()) }
    }

    fn shown_value(&self, sheet: usize, c: CellRef) -> String {
        self.wb.display(sheet, c).0
    }

    pub(super) fn open_origin(&mut self) {
        self.settle_edit();
        self.audit.origin.open = !self.audit.origin.open;
    }

    /// One cell and, folded under it, what it comes from. `go`: a cell
    /// the user clicked.
    fn origin_cell(&self, ui: &mut egui::Ui, sheet: usize, c: CellRef, depth: usize, seen: &mut HashSet<(usize, CellRef)>, go: &mut Option<(usize, CellRef)>) {
        let formula = self.wb.sheets[sheet].get(c).and_then(|x| x.formula()).is_some();
        let text = format!("{}  =  {}", self.cell_name(sheet, c), self.shown_value(sheet, c));
        let origins = if depth < MAX_DEPTH && seen.insert((sheet, c)) { self.wb.origins(sheet, c) } else { Vec::new() };
        let kind = if formula { self.wb.edit_text(sheet, c) } else { tr("org.typed").to_string() };
        if origins.is_empty() {
            ui.horizontal(|ui| {
                if ui.link(text).clicked() {
                    *go = Some((sheet, c));
                }
                ui.label(RichText::new(kind).weak().monospace());
            });
            return;
        }
        let id = Id::new(("origin", self.uid, sheet, c.row, c.col, depth));
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, depth == 0)
            .show_header(ui, |ui| {
                if ui.link(text).clicked() {
                    *go = Some((sheet, c));
                }
                ui.label(RichText::new(kind).weak().monospace());
            })
            .body(|ui| {
                for o in origins {
                    self.origin_item(ui, &o, depth + 1, seen, go);
                }
            });
    }

    fn origin_item(&self, ui: &mut egui::Ui, o: &Origin, depth: usize, seen: &mut HashSet<(usize, CellRef)>, go: &mut Option<(usize, CellRef)>) {
        match o {
            Origin::Cells(s, r) if r.rows() == 1 && r.cols() == 1 => self.origin_cell(ui, *s, r.start, depth, seen, go),
            Origin::Cells(s, r) => {
                let sh = &self.wb.sheets[*s];
                let (mut formulas, mut typed) = (Vec::new(), 0usize);
                sh.for_each_in(r, |c, cell| {
                    if cell.formula().is_some() {
                        formulas.push(c);
                    } else if !matches!(cell.value(), Value::Empty) {
                        typed += 1;
                    }
                });
                let query = self.wb.query_at(*s, r.start);
                let title = format!("{}  ·  {}", self.range_name(*s, r), trf("org.cells", &[("n", &formulas.len()), ("m", &typed)]));
                let id = Id::new(("origin_r", self.uid, *s, r.start.row, r.start.col, r.end.row, r.end.col, depth));
                egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
                    .show_header(ui, |ui| {
                        if ui.link(title).clicked() {
                            *go = Some((*s, r.start));
                        }
                    })
                    .body(|ui| {
                        if let Some(q) = query {
                            self.origin_query(ui, q, depth + 1, &mut HashSet::new());
                        }
                        for &c in formulas.iter().take(MAX_CELLS) {
                            self.origin_cell(ui, *s, c, depth + 1, seen, go);
                        }
                        if formulas.len() > MAX_CELLS {
                            ui.label(RichText::new(trf("fi.more", &[("n", &(formulas.len() - MAX_CELLS))])).weak());
                        }
                    });
            }
            Origin::External { book, sheet, range } => {
                let Some(b) = self.wb.externals.get(*book) else { return };
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(trf("org.linked", &[("name", &b.file_name())])).strong());
                    ui.label(format!("{sheet}!{}", range.to_a1()));
                    ui.label(RichText::new(format!("{} · {}", b.target, file_date(&b.target))).weak());
                });
            }
            Origin::Query(q) => self.origin_query(ui, *q, depth, &mut HashSet::new()),
        }
    }

    fn origin_query(&self, ui: &mut egui::Ui, i: usize, depth: usize, seen: &mut HashSet<usize>) {
        let Some(q) = self.wb.queries.get(i) else { return };
        if !seen.insert(i) || depth > MAX_DEPTH + 4 {
            return;
        }
        let id = Id::new(("origin_q", self.uid, i, depth));
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
            .show_header(ui, |ui| {
                ui.label(RichText::new(trf("org.query", &[("name", &q.name)])).strong().color(Color32::from_rgb(0x1E, 0x8E, 0x4E)));
            })
            .body(|ui| {
                let (text, file) = source_text(&q.source);
                ui.horizontal_wrapped(|ui| {
                    ui.label(text);
                    if let Some(f) = file {
                        ui.label(RichText::new(file_date(&f)).weak());
                    }
                });
                if !q.steps.is_empty() {
                    ui.label(RichText::new(trf("org.steps", &[("n", &q.steps.len())])).weak());
                }
                if let Source::Query { name } = &q.source {
                    if let Some(j) = self.wb.queries.iter().position(|x| x.name.eq_ignore_ascii_case(name)) {
                        self.origin_query(ui, j, depth + 1, seen);
                    }
                }
            });
    }

    pub(super) fn origin_window(&mut self, ctx: &egui::Context) {
        self.delete_prompt(ctx);
        if !self.audit.origin.open {
            return;
        }
        let mut open = true;
        let mut go = None;
        let (sheet, c) = (self.sheet, self.cursor);
        crate::widgets::esc_closes(ctx, Id::new(("origin", self.uid)), &mut open);
        egui::Window::new(tr("org.title")).id(Id::new(("origin", self.uid))).pivot(egui::Align2::RIGHT_TOP).default_pos(ctx.content_rect().right_top() + egui::vec2(-40.0, 140.0)).default_width(420.0).open(&mut open).show_fit(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                crate::widgets::section(ui, tr("org.from"));
                let mut seen = HashSet::new();
                self.origin_cell(ui, sheet, c, 0, &mut seen, &mut go);
                crate::widgets::section(ui, tr("org.used_by"));
                let deps = self.wb.dependents(sheet, c);
                if deps.is_empty() {
                    ui.label(RichText::new(tr("org.no_dependents")).weak());
                }
                for &(s, d) in deps.iter().take(MAX_CELLS) {
                    ui.horizontal(|ui| {
                        if ui.link(format!("{}  =  {}", self.cell_name(s, d), self.shown_value(s, d))).clicked() {
                            go = Some((s, d));
                        }
                        ui.label(RichText::new(self.wb.edit_text(s, d)).weak().monospace());
                    });
                }
                if deps.len() > MAX_CELLS {
                    ui.label(RichText::new(trf("fi.more", &[("n", &(deps.len() - MAX_CELLS))])).weak());
                }
            });
        });
        if let Some((s, c)) = go {
            self.go_to_cell(s, c);
        }
        if !open {
            self.audit.origin.open = false;
        }
    }

    // --- deleting what others read ----------------------------------------------

    /// Whether deleting `what` must wait: formulas elsewhere read it, and
    /// the user is asked first.
    pub(super) fn ask_before_delete(&mut self, what: Deletion) -> bool {
        if self.audit.origin.confirmed.take() == Some(what) {
            return false;
        }
        let s = self.sel();
        let all = super::LAST;
        let (sheet, area, whole) = match what {
            Deletion::Rows => (self.sheet, Range::new(CellRef::new(s.start.row, 0), CellRef::new(s.end.row, all.col)), false),
            Deletion::Cols => (self.sheet, Range::new(CellRef::new(0, s.start.col), CellRef::new(all.row, s.end.col)), false),
            Deletion::Sheet(i) => (i, Range::new(CellRef::new(0, 0), all), true),
        };
        let deps = self.wb.outside_dependents(sheet, area, whole);
        if deps.is_empty() {
            return false;
        }
        self.audit.origin.pending = Some((what, deps));
        true
    }

    fn delete_prompt(&mut self, ctx: &egui::Context) {
        let Some((what, deps)) = &self.audit.origin.pending else { return };
        let (what, deps) = (*what, deps.clone());
        let mut res = None;
        egui::Modal::new(Id::new(("del_deps", self.uid))).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.heading(tr("org.del_title"));
            ui.add_space(6.0);
            ui.label(trf("org.del_intro", &[("n", &deps.len())]));
            ui.add_space(4.0);
            egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                for &(s, d) in deps.iter().take(MAX_CELLS) {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(self.cell_name(s, d)).strong());
                        ui.label(RichText::new(self.wb.edit_text(s, d)).weak().monospace());
                    });
                }
                if deps.len() > MAX_CELLS {
                    ui.label(RichText::new(trf("fi.more", &[("n", &(deps.len() - MAX_CELLS))])).weak());
                }
            });
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("unsaved.cancel")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    res = Some(0);
                }
                if ui.button(tr("org.show")).clicked() {
                    res = Some(1);
                }
                if ui.button(RichText::new(tr("org.del_anyway")).strong()).clicked() {
                    res = Some(2);
                }
            });
        });
        match res {
            Some(0) => self.audit.origin.pending = None,
            Some(1) => {
                self.audit.origin.pending = None;
                let (s, c) = deps[0];
                self.go_to_cell(s, c);
                self.audit.origin.open = true;
            }
            Some(_) => {
                self.audit.origin.pending = None;
                self.audit.origin.confirmed = Some(what);
                self.run_deletion(what);
            }
            None => {}
        }
    }

    pub(super) fn run_deletion(&mut self, what: Deletion) {
        match what {
            Deletion::Rows => self.delete_rows(),
            Deletion::Cols => self.delete_cols(),
            Deletion::Sheet(i) => self.remove_sheet(i),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_sheet::Workbook;

    #[test]
    fn deleting_what_formulas_read_asks_first() {
        let mut wb = Workbook::new();
        wb.set_input(0, CellRef::new(0, 0), "5");
        wb.set_input(0, CellRef::new(2, 1), "=A1*2");
        let mut v = SheetView::new(wb);
        // Row 1 holds A1, which B3 reads.
        v.anchor = CellRef::new(0, 0);
        v.cursor = CellRef::new(0, 3);
        assert!(v.ask_before_delete(Deletion::Rows));
        assert_eq!(v.audit.origin.pending.as_ref().unwrap().1, vec![(0, CellRef::new(2, 1))]);
        // Confirmed: it goes ahead once.
        v.audit.origin.pending = None;
        v.audit.origin.confirmed = Some(Deletion::Rows);
        assert!(!v.ask_before_delete(Deletion::Rows));
        assert!(v.ask_before_delete(Deletion::Rows));
        // Row 5 is read by nobody.
        v.cursor = CellRef::new(4, 0);
        v.anchor = CellRef::new(4, 0);
        v.audit.origin.pending = None;
        assert!(!v.ask_before_delete(Deletion::Rows));
    }

    #[test]
    fn file_dates() {
        assert_eq!(file_date("/no/such/file.csv"), tr("org.not_found"));
        assert_eq!(file_date("https://example.com/a.csv"), tr("org.web"));
        let here = std::env::current_exe().unwrap();
        assert!(file_date(here.to_str().unwrap()).len() > 4);
    }
}
