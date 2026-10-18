//! Formulas > Watch Window (cells to keep an eye on, from any sheet) and
//! Evaluate Formula (a formula worked out one part at a time).

use apex_sheet::{CellRef, EvalSteps, Value};
use eframe::egui::{self, Id, RichText};

use super::SheetView;
use crate::app::Notice;
use crate::i18n::tr;
use crate::widgets::FitWindow as _;

#[derive(Default)]
pub(super) struct WatchState {
    pub open: bool,
    /// Selected row: sheet and cell.
    sel: Option<(usize, CellRef)>,
    /// Evaluate Formula: the formula and the cells stepped into.
    pub eval: Option<Vec<EvalSteps>>,
    /// File name of the workbook (the Book column).
    pub book: String,
}

/// At most this many cells are added from one selection.
const MAX_ADD: usize = 500;

impl SheetView {
    pub(super) fn toggle_watch_window(&mut self) {
        self.audit.watch.open = !self.audit.watch.open;
    }

    /// Adds the cells of the selection with content to the Watch Window.
    pub(super) fn add_watches(&mut self) {
        let sheet = self.sheet;
        let mut cells: Vec<CellRef> = Vec::new();
        for r in self.areas() {
            if r.is_single() {
                cells.push(r.start);
            } else {
                self.sh().for_each_in(&r, |c, _| cells.push(c));
            }
        }
        let list = &mut self.wb.sheets[sheet].cell_watches;
        for c in cells.into_iter().take(MAX_ADD) {
            if !list.contains(&c) {
                list.push(c);
            }
        }
        self.audit.watch.open = true;
    }

    fn remove_watch(&mut self, s: usize, c: CellRef) {
        if let Some(sh) = self.wb.sheets.get_mut(s) {
            sh.cell_watches.retain(|x| *x != c);
        }
    }

    /// The Watch Window (a floating window).
    pub(super) fn watch_window(&mut self, ctx: &egui::Context) {
        if !self.audit.watch.open {
            return;
        }
        let mut open = true;
        let mut go: Option<(usize, CellRef)> = None;
        let mut add = false;
        let mut delete = None;
        let sel = self.audit.watch.sel;
        let rows: Vec<(usize, CellRef)> = self.wb.sheets.iter().enumerate().flat_map(|(s, sh)| sh.cell_watches.iter().map(move |c| (s, *c))).collect();
        let book = self.audit.watch.book.clone();
        let screen = ctx.content_rect();
        let at = egui::pos2(screen.center().x - 310.0, screen.max.y - 320.0);
        crate::widgets::esc_closes(ctx, Id::new(("watch_window", self.uid)), &mut open);
        egui::Window::new(tr("watch.title")).id(Id::new(("watch_window", self.uid))).default_pos(at).open(&mut open).collapsible(false).resizable(true).default_width(620.0).default_height(260.0).show_fit(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button(format!("{}…", tr("watch.add"))).on_hover_text(tr("watch.add_tip")).clicked() {
                    add = true;
                }
                if ui.add_enabled(sel.is_some(), egui::Button::new(tr("watch.delete"))).clicked() {
                    delete = sel;
                }
            });
            ui.separator();
            let widths = [110.0, 90.0, 60.0, 100.0];
            egui::ScrollArea::both().max_height(ui.available_height().max(120.0)).show(ui, |ui| {
                egui::Grid::new("watch_grid").striped(true).num_columns(5).min_col_width(40.0).show(ui, |ui| {
                    for key in ["watch.book", "watch.sheet", "watch.cell", "watch.value"] {
                        ui.label(RichText::new(tr(key)).strong());
                    }
                    ui.label(RichText::new(tr("watch.formula")).strong());
                    ui.end_row();
                    for (s, c) in &rows {
                        let sh = &self.wb.sheets[*s];
                        let value = self.wb.display(*s, *c).0;
                        let formula = match sh.get(*c).map(|x| &x.content) {
                            Some(apex_sheet::Content::Formula { .. }) => self.wb.edit_text(*s, *c),
                            _ => String::new(),
                        };
                        let on = sel == Some((*s, *c));
                        let cells = [book.as_str(), sh.name.as_str(), &c.to_a1(), &value];
                        for (t, w) in cells.iter().zip(widths) {
                            let r = ui.add(egui::Button::selectable(on, *t).right_text("").truncate().min_size(egui::vec2(w, 18.0)));
                            if r.clicked() {
                                self.audit.watch.sel = Some((*s, *c));
                            }
                            if r.double_clicked() {
                                go = Some((*s, *c));
                            }
                        }
                        let r = ui.add(egui::Button::selectable(on, RichText::new(formula).monospace()).right_text(""));
                        if r.clicked() {
                            self.audit.watch.sel = Some((*s, *c));
                        }
                        if r.double_clicked() {
                            go = Some((*s, *c));
                        }
                        ui.end_row();
                    }
                });
                if rows.is_empty() {
                    ui.label(RichText::new(tr("watch.empty")).weak());
                }
            });
        });
        if add {
            self.add_watches();
        }
        if let Some((s, c)) = delete {
            self.remove_watch(s, c);
            self.audit.watch.sel = None;
        }
        if let Some((s, c)) = go {
            self.go_to_cell(s, c);
        }
        if !open {
            self.audit.watch.open = false;
        }
    }

    pub(super) fn go_to_cell(&mut self, s: usize, c: CellRef) {
        self.settle_edit();
        if s != self.sheet {
            self.select_sheet(s);
        }
        self.anchor = c;
        self.cursor = c;
        self.ensure_visible = true;
        self.focus_grid = true;
    }

    // --- Evaluate Formula -----------------------------------------------------------------

    pub(super) fn open_evaluate(&mut self) {
        self.settle_edit();
        match self.wb.eval_steps(self.sheet, self.cursor) {
            Some(s) => self.audit.watch.eval = Some(vec![s]),
            None => self.notices.push(Notice::info(tr("evalf.no_formula"))),
        }
    }

    pub(super) fn evaluate_window(&mut self, ctx: &egui::Context) {
        let Some(mut stack) = self.audit.watch.eval.take() else { return };
        let mut open = true;
        let (mut step, mut into, mut out, mut restart) = (false, false, false, false);
        let loc = self.wb.locale;
        let top_done = stack.last().is_some_and(|s| s.done());
        let can_in = stack.last().and_then(|s| s.step_in_target(&self.wb)).is_some();
        let screen = ctx.content_rect();
        let at = egui::pos2(screen.center().x - 280.0, screen.min.y + 150.0);
        egui::Window::new(tr("evalf.title")).id(Id::new(("eval_formula", self.uid))).open(&mut open).collapsible(false).resizable(false).default_pos(at).min_width(560.0).show_fit(ctx, |ui| {
            egui::Grid::new("evalf_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label(RichText::new(tr("evalf.reference")).strong());
                ui.label(RichText::new(tr("evalf.evaluation")).strong());
                ui.end_row();
                for (k, s) in stack.iter().enumerate() {
                    let name = &self.wb.sheets[s.sheet].name;
                    ui.label(format!("{}!{}", apex_sheet::chart::quote_sheet(name), apex_sheet::chart::abs_a1(&apex_sheet::Range::single(s.cell))));
                    let v = s.view(&loc);
                    let current = k + 1 == stack.len();
                    let mut job = egui::text::LayoutJob::default();
                    let font = egui::FontId::monospace(13.0);
                    let color = if current { ui.visuals().strong_text_color() } else { ui.visuals().weak_text_color() };
                    let plain = egui::TextFormat { font_id: font.clone(), color, ..Default::default() };
                    job.append(&v.before, 0.0, plain.clone());
                    job.append(&v.next, 0.0, egui::TextFormat { underline: egui::Stroke::new(1.5, color), background: if current { ui.visuals().selection.bg_fill.gamma_multiply(0.35) } else { egui::Color32::TRANSPARENT }, ..plain.clone() });
                    job.append(&v.after, 0.0, plain);
                    job.wrap.max_width = 380.0;
                    egui::Frame::new().fill(ui.visuals().extreme_bg_color).inner_margin(6).show(ui, |ui| {
                        ui.set_min_width(380.0);
                        ui.label(job);
                    });
                    ui.end_row();
                }
            });
            ui.add_space(4.0);
            ui.label(RichText::new(tr("evalf.hint")).small().weak());
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let label = if top_done && stack.len() == 1 { tr("evalf.restart") } else { tr("evalf.evaluate") };
                let ev = ui.add_enabled(!(top_done && stack.len() > 1), egui::Button::new(label));
                if ev.clicked() {
                    if top_done && stack.len() == 1 {
                        restart = true;
                    } else {
                        step = true;
                    }
                }
                if ui.add_enabled(can_in, egui::Button::new(tr("evalf.step_in"))).clicked() {
                    into = true;
                }
                if ui.add_enabled(stack.len() > 1, egui::Button::new(tr("evalf.step_out"))).clicked() {
                    out = true;
                }
            });
        });
        if step {
            if let Some(s) = stack.last_mut() {
                s.step(&self.wb);
            }
        }
        if into {
            if let Some((sh, c)) = stack.last().and_then(|s| s.step_in_target(&self.wb)) {
                if let Some(s) = self.wb.eval_steps(sh, c) {
                    stack.push(s);
                }
            }
        }
        if out && stack.len() > 1 {
            if let Some(inner) = stack.pop() {
                let v = inner.result(&self.wb).unwrap_or_else(|| self.wb.sheets[inner.sheet].value(inner.cell).clone());
                let v = if v == Value::Empty { Value::Number(0.0) } else { v };
                if let Some(s) = stack.last_mut() {
                    s.replace_next(v);
                }
            }
        }
        if restart {
            if let Some(s) = stack.first().and_then(|s| self.wb.eval_steps(s.sheet, s.cell)) {
                stack = vec![s];
            }
        }
        if open {
            self.audit.watch.eval = Some(stack);
        }
    }
}
