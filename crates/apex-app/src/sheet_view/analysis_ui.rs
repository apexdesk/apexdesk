//! Data > What-If Analysis (Goal Seek, Data Table, Scenario Manager),
//! Solver, Data Analysis tools and Flash Fill.

use apex_sheet::analysis::solver::{Constraint, ConstraintOp, SolverGoal, SolverMethod, SolverModel, SolverResult, SolverStatus};
use apex_sheet::analysis::stats::{self, RandomDist, SampleMethod, Series, StatsError, TTestKind};
use apex_sheet::analysis::{Lang, Scenario};
use apex_sheet::workbook::{AnalysisError, GoalSeekResult, Output};

use super::f10::{self, F10Dlg, parse_num, plain_window, ref_edit, tool_window};
use super::*;
use crate::widgets::DialogResult;

pub(super) enum AnalysisDlg {
    GoalSeek { set: String, to: String, by: String },
    GoalStatus { set: CellRef, target: f64, r: GoalSeekResult, applied: bool },
    DataTable { row: String, col: String, range: Range },
    Scenarios(ScenarioMgr),
    Solver(Box<SolverDlg>),
    SolverResult { r: SolverResult, keep: bool, applied: bool, dlg: Box<SolverDlg> },
    DataAnalysis { selected: usize },
    Forecast(Box<super::forecast_ui::ForecastDlg>),
    Statistics,
}

pub(super) struct ScenarioMgr {
    selected: usize,
    mode: ScMode,
}

enum ScMode {
    List,
    Edit { orig: Option<String>, name: String, changing: String, comment: String },
    Values { orig: Option<String>, name: String, cells: Vec<CellRef>, comment: String, values: Vec<String> },
    Summary { cells: String },
}

pub(super) struct SolverDlg {
    objective: String,
    /// 0 max, 1 min, 2 value of.
    goal: u8,
    value: String,
    vars: String,
    cons: Vec<Constraint>,
    sel: Option<usize>,
    non_neg: bool,
    method: SolverMethod,
    max_time: String,
    max_iter: String,
    precision: String,
    multistart: bool,
    sub: Option<ConsEdit>,
    options: bool,
}

struct ConsEdit {
    index: Option<usize>,
    lhs: String,
    op: ConstraintOp,
    rhs: String,
}

pub(super) fn lang() -> Lang {
    Lang::from_tag(crate::i18n::current())
}

pub(super) fn error_text(e: &AnalysisError) -> String {
    match e {
        AnalysisError::NotAFormula(c) => trf("whatif.err_formula", &[("cell", &c.to_a1())]),
        AnalysisError::NotAValue(c) => trf("whatif.err_value", &[("cell", &c.to_a1())]),
        AnalysisError::BadRange => tr("whatif.err_range").to_string(),
        AnalysisError::NotFound => tr("whatif.err_not_found").to_string(),
        AnalysisError::NoPattern => tr("flash.no_pattern").to_string(),
        AnalysisError::NoData => tr("whatif.err_no_data").to_string(),
        AnalysisError::NoRelationship => tr("model.err_no_relationship").to_string(),
        AnalysisError::Stats(s) => stats_error(*s).to_string(),
        AnalysisError::Forecast(e) => tr(match e {
            apex_sheet::analysis::ets::EtsError::TooFew => "forecast.err_few",
            apex_sheet::analysis::ets::EtsError::Irregular => "forecast.err_irregular",
        })
        .to_string(),
    }
}

fn stats_error(e: StatsError) -> &'static str {
    match e {
        StatsError::NotEnoughData => tr("tools.err_data"),
        StatsError::SizeMismatch => tr("tools.err_size"),
        StatsError::BadParameter => tr("tools.err_param"),
    }
}

fn op_text(op: ConstraintOp) -> &'static str {
    match op {
        ConstraintOp::Le => "<=",
        ConstraintOp::Ge => ">=",
        ConstraintOp::Eq => "=",
        ConstraintOp::Int => "int",
        ConstraintOp::Bin => "bin",
    }
}

fn status_text(s: SolverStatus) -> &'static str {
    match s {
        SolverStatus::Optimal => tr("solver.st_optimal"),
        SolverStatus::Feasible => tr("solver.st_feasible"),
        SolverStatus::Infeasible => tr("solver.st_infeasible"),
        SolverStatus::Unbounded => tr("solver.st_unbounded"),
        SolverStatus::IterationLimit => tr("solver.st_limit"),
        SolverStatus::NotLinear => tr("solver.st_not_linear"),
        SolverStatus::ErrorValue => tr("solver.st_error"),
        SolverStatus::BadModel => tr("solver.st_bad_model"),
    }
}

/// Cells of a list of references (`A1:A3; C5`).
fn ref_list(wb: &Workbook, sheet: usize, text: &str) -> Option<Vec<Range>> {
    let mut out = Vec::new();
    for part in text.split([';', ',']).map(str::trim).filter(|p| !p.is_empty()) {
        let (si, r) = f10::parse_ref(wb, sheet, part)?;
        if si != sheet {
            return None;
        }
        out.push(r);
    }
    (!out.is_empty()).then_some(out)
}

fn ranges_text(rs: &[Range]) -> String {
    rs.iter().map(f10::abs_range).collect::<Vec<_>>().join(";")
}

/// Grid row: caption and a widget.
fn row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.label(label);
    add(ui);
    ui.end_row();
}

impl SheetView {
    fn cell_ref(&self, text: &str) -> Option<(usize, CellRef)> {
        f10::parse_ref(&self.wb, self.sheet, text).filter(|(_, r)| r.is_single()).map(|(s, r)| (s, r.start))
    }

    // --- Flash Fill ------------------------------------------------------------------

    pub(super) fn flash_fill_cmd(&mut self) {
        self.settle_edit();
        match self.wb.flash_fill(self.sheet, self.cursor) {
            Ok(n) => self.notices.push(Notice::info(trf("flash.filled", &[("n", &n)]))),
            // No pattern or no data: what the command needs, not a fault.
            Err(e) => self.notices.push(Notice::info(error_text(&e))),
        }
    }

    // --- openers -------------------------------------------------------------------------

    pub(super) fn open_goal_seek(&mut self) {
        let set = if self.sh().get(self.cursor).and_then(|c| c.formula()).is_some() { f10::abs_range(&Range::single(self.cursor)) } else { String::new() };
        self.open_f10(F10Dlg::Analysis(AnalysisDlg::GoalSeek { set, to: String::new(), by: String::new() }), Some(0));
    }

    pub(super) fn open_data_table(&mut self) {
        let range = self.sel();
        if range.rows() < 2 || range.cols() < 2 {
            self.notices.push(Notice::info(tr("dtable.select_first")));
            return;
        }
        self.open_f10(F10Dlg::Analysis(AnalysisDlg::DataTable { row: String::new(), col: String::new(), range }), Some(0));
    }

    pub(super) fn open_scenarios(&mut self) {
        self.open_f10(F10Dlg::Analysis(AnalysisDlg::Scenarios(ScenarioMgr { selected: 0, mode: ScMode::List })), None);
    }

    pub(super) fn open_solver(&mut self) {
        let d = match self.wb.solver_model(self.sheet).cloned() {
            Some(m) => SolverDlg {
                objective: f10::abs_range(&Range::single(m.objective)),
                goal: match m.goal {
                    SolverGoal::Max => 0,
                    SolverGoal::Min => 1,
                    SolverGoal::Value(_) => 2,
                },
                value: match m.goal {
                    SolverGoal::Value(v) => f10::fmt_num(&self.wb, v),
                    _ => "0".into(),
                },
                vars: ranges_text(&m.variables),
                cons: m.constraints.clone(),
                sel: None,
                non_neg: m.non_negative,
                method: m.method,
                max_time: f10::fmt_num(&self.wb, m.max_time),
                max_iter: m.max_iter.to_string(),
                precision: f10::fmt_num(&self.wb, m.precision),
                multistart: m.multistart,
                sub: None,
                options: false,
            },
            None => {
                let m = SolverModel::default();
                let obj = if self.sh().get(self.cursor).and_then(|c| c.formula()).is_some() { f10::abs_range(&Range::single(self.cursor)) } else { String::new() };
                SolverDlg {
                    objective: obj,
                    goal: 0,
                    value: "0".into(),
                    vars: String::new(),
                    cons: Vec::new(),
                    sel: None,
                    non_neg: true,
                    method: m.method,
                    max_time: f10::fmt_num(&self.wb, m.max_time),
                    max_iter: m.max_iter.to_string(),
                    precision: f10::fmt_num(&self.wb, m.precision),
                    multistart: false,
                    sub: None,
                    options: false,
                }
            }
        };
        self.open_f10(F10Dlg::Analysis(AnalysisDlg::Solver(Box::new(d))), Some(0));
    }

    pub(super) fn open_data_analysis(&mut self) {
        self.open_f10(F10Dlg::Analysis(AnalysisDlg::DataAnalysis { selected: 0 }), None);
    }

    // --- dialogs ---------------------------------------------------------------------------

    pub(super) fn analysis_dialog(&mut self, ctx: &egui::Context, d: AnalysisDlg) -> Option<AnalysisDlg> {
        match d {
            AnalysisDlg::GoalSeek { set, to, by } => self.goal_seek_dialog(ctx, set, to, by),
            AnalysisDlg::Forecast(d) => self.forecast_dialog(ctx, d),
            AnalysisDlg::Statistics => self.statistics_dialog(ctx),
            AnalysisDlg::GoalStatus { set, target, r, applied } => {
                let found = trf(if r.found { "goal.found" } else { "goal.not_found" }, &[("cell", &set.to_a1())]);
                let res = tool_window(ctx, "goal_status", tr("goal.status_title"), 320.0, false, |ui| {
                    ui.label(found);
                    ui.add_space(6.0);
                    egui::Grid::new("goal_status_grid").show(ui, |ui| {
                        row(ui, tr("goal.target"), |ui| {
                            ui.label(f10::fmt_num(&self.wb, target));
                        });
                        row(ui, tr("goal.current"), |ui| {
                            ui.label(f10::fmt_num(&self.wb, r.achieved));
                        });
                    });
                });
                match res {
                    DialogResult::None => Some(AnalysisDlg::GoalStatus { set, target, r, applied }),
                    DialogResult::Ok => None,
                    DialogResult::Cancel => {
                        if applied && self.wb.undo_history().first() == Some(&"undo.goal_seek") {
                            self.wb.undo();
                            self.wb.discard_redo();
                        }
                        None
                    }
                }
            }
            AnalysisDlg::DataTable { row: mut row_in, col: mut col_in, range } => {
                let focus = self.grid_has_focus(ctx);
                let mut pick = std::mem::take(&mut self.f10.pick);
                let res = tool_window(ctx, "data_table", tr("dtable.title"), 300.0, focus, |ui| {
                    egui::Grid::new("dtable_grid").num_columns(2).show(ui, |ui| {
                        row_ref(ui, tr("dtable.row_input"), &mut row_in, 0, &mut pick);
                        row_ref(ui, tr("dtable.col_input"), &mut col_in, 1, &mut pick);
                    });
                    ui.label(RichText::new(trf("dtable.range", &[("range", &range.to_a1())])).small().weak());
                });
                self.f10.pick = pick;
                match res {
                    DialogResult::None => Some(AnalysisDlg::DataTable { row: row_in, col: col_in, range }),
                    DialogResult::Cancel => None,
                    DialogResult::Ok => {
                        let ri = if row_in.trim().is_empty() { None } else { self.cell_ref(&row_in).map(|x| x.1) };
                        let ci = if col_in.trim().is_empty() { None } else { self.cell_ref(&col_in).map(|x| x.1) };
                        let bad = (!row_in.trim().is_empty() && ri.is_none()) || (!col_in.trim().is_empty() && ci.is_none());
                        let r = if bad { Err(AnalysisError::BadRange) } else { self.wb.data_table(self.sheet, &range, ri, ci) };
                        match r {
                            Ok(_) => None,
                            Err(e) => {
                                self.notices.push(Notice::refused(error_text(&e)));
                                Some(AnalysisDlg::DataTable { row: row_in, col: col_in, range })
                            }
                        }
                    }
                }
            }
            AnalysisDlg::Scenarios(m) => self.scenario_dialog(ctx, m).map(AnalysisDlg::Scenarios),
            AnalysisDlg::Solver(d) => self.solver_dialog(ctx, d),
            AnalysisDlg::SolverResult { r, mut keep, applied, dlg } => {
                let res = tool_window(ctx, "solver_result", tr("solver.results"), 380.0, false, |ui| {
                    ui.label(status_text(r.status));
                    ui.add_space(8.0);
                    ui.add_enabled_ui(applied, |ui| {
                        ui.radio_value(&mut keep, true, tr("solver.keep"));
                        ui.radio_value(&mut keep, false, tr("solver.restore"));
                    });
                    if r.objective.is_finite() {
                        ui.add_space(4.0);
                        ui.label(RichText::new(format!("{} {}", tr("solver.objective_value"), f10::fmt_num(&self.wb, r.objective))).weak());
                    }
                });
                match res {
                    DialogResult::None => Some(AnalysisDlg::SolverResult { r, keep, applied, dlg }),
                    res => {
                        if applied && (!keep || res == DialogResult::Cancel) && self.wb.undo_history().first() == Some(&"undo.solver") {
                            self.wb.undo();
                            self.wb.discard_redo();
                        }
                        None
                    }
                }
            }
            AnalysisDlg::DataAnalysis { mut selected } => {
                let res = tool_window(ctx, "data_analysis", tr("tools.title"), 300.0, false, |ui| {
                    ui.label(tr("tools.pick"));
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                            ui.set_width(270.0);
                            for (i, t) in TOOLS.iter().enumerate() {
                                let r = ui.add(egui::Button::selectable(selected == i, tr(t.key())).min_size(vec2(260.0, 0.0)));
                                if r.clicked() {
                                    selected = i;
                                }
                                if r.double_clicked() {
                                    selected = i;
                                }
                            }
                        });
                    });
                });
                match res {
                    DialogResult::None => Some(AnalysisDlg::DataAnalysis { selected }),
                    DialogResult::Cancel => None,
                    DialogResult::Ok => {
                        let t = TOOLS[selected.min(TOOLS.len() - 1)];
                        let d = ToolDlg::new(t, &self.sel(), self.wb.locale.decimal, matches!(self.sh().value(self.sel().start), Value::Text(_)));
                        self.open_f10(F10Dlg::Tool(Box::new(d)), Some(0));
                        None
                    }
                }
            }
        }
    }

    fn goal_seek_dialog(&mut self, ctx: &egui::Context, mut set: String, mut to: String, mut by: String) -> Option<AnalysisDlg> {
        let focus = self.grid_has_focus(ctx);
        let mut pick = std::mem::take(&mut self.f10.pick);
        let res = tool_window(ctx, "goal_seek", tr("goal.title"), 300.0, focus, |ui| {
            egui::Grid::new("goal_grid").num_columns(2).show(ui, |ui| {
                row_ref(ui, tr("goal.set_cell"), &mut set, 0, &mut pick);
                row(ui, tr("goal.to_value"), |ui| {
                    ui.add(egui::TextEdit::singleline(&mut to).desired_width(150.0));
                });
                row_ref(ui, tr("goal.changing"), &mut by, 1, &mut pick);
            });
        });
        self.f10.pick = pick;
        match res {
            DialogResult::None => Some(AnalysisDlg::GoalSeek { set, to, by }),
            DialogResult::Cancel => None,
            DialogResult::Ok => {
                let (s, t, b) = (self.cell_ref(&set), parse_num(&self.wb, &to), self.cell_ref(&by));
                let r = match (s, t, b) {
                    (Some((si, sc)), Some(target), Some((bi, bc))) if si == bi => self.wb.goal_seek(si, sc, target, bc, 100, 0.001).map(|r| (sc, target, r)),
                    (_, None, _) => {
                        self.notices.push(Notice::error(tr("goal.err_number")));
                        return Some(AnalysisDlg::GoalSeek { set, to, by });
                    }
                    _ => Err(AnalysisError::BadRange),
                };
                match r {
                    Ok((sc, target, r)) => Some(AnalysisDlg::GoalStatus { set: sc, target, r, applied: r.value.is_finite() }),
                    Err(e) => {
                        self.notices.push(Notice::refused(error_text(&e)));
                        Some(AnalysisDlg::GoalSeek { set, to, by })
                    }
                }
            }
        }
    }

    // --- workbook statistics -------------------------------------------------------------

    pub(super) fn open_statistics(&mut self) {
        self.settle_edit();
        self.open_f10(F10Dlg::Analysis(AnalysisDlg::Statistics), None);
    }

    fn statistics_dialog(&mut self, ctx: &egui::Context) -> Option<AnalysisDlg> {
        let (sheet, book) = (self.wb.statistics(Some(self.sheet)), self.wb.statistics(None));
        let name = self.sh().name.clone();
        let mut close = false;
        let open = plain_window(ctx, "statistics", tr("stats.title"), 340.0, |ui| {
            let block = |ui: &mut egui::Ui, id: &str, s: &apex_sheet::workbook::BookStats, one: bool| {
                egui::Grid::new(id).num_columns(2).striped(true).min_col_width(140.0).show(ui, |ui| {
                    let mut line = |key: &'static str, v: String| {
                        ui.label(tr(key));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(v));
                        ui.end_row();
                    };
                    if one {
                        line("stats.last_cell", s.last.map_or_else(|| "—".into(), |c| c.to_a1()));
                    } else {
                        line("stats.sheets", s.sheets.to_string());
                    }
                    for (k, n) in [("stats.cells", s.cells), ("stats.formulas", s.formulas), ("stats.tables", s.tables), ("stats.pivots", s.pivots), ("stats.charts", s.charts), ("stats.images", s.images), ("stats.shapes", s.shapes), ("stats.comments", s.comments), ("stats.notes", s.notes)] {
                        line(k, n.to_string());
                    }
                });
            };
            ui.label(RichText::new(trf("stats.current", &[("name", &name)])).strong());
            block(ui, "stats_sheet", &sheet, true);
            ui.add_space(8.0);
            ui.label(RichText::new(tr("stats.workbook")).strong());
            block(ui, "stats_book", &book, false);
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("dialog.ok")).clicked() {
                    close = true;
                }
            });
        });
        (open && !close && !ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Enter))).then_some(AnalysisDlg::Statistics)
    }

    // --- scenarios -------------------------------------------------------------------------

    fn scenario_dialog(&mut self, ctx: &egui::Context, mut m: ScenarioMgr) -> Option<ScenarioMgr> {
        let focus = self.grid_has_focus(ctx);
        let scs: Vec<Scenario> = self.wb.scenarios(self.sheet).to_vec();
        m.selected = m.selected.min(scs.len().saturating_sub(1));
        match &mut m.mode {
            ScMode::List => {
                enum Act {
                    Add,
                    Edit,
                    Delete,
                    Show,
                    Summary,
                }
                let mut act = None;
                let mut sel = m.selected;
                let mut closed = false;
                let open = plain_window(ctx, "scenarios", tr("scenario.manager"), 420.0, |ui| {
                    ui.label(tr("scenario.list"));
                    // Narrower on a phone, with the buttons still beside it.
                    let list_w = (ui.available_width() - 140.0).clamp(130.0, 260.0);
                    ui.horizontal(|ui| {
                        egui::Frame::group(ui.style()).show(ui, |ui| {
                            ui.set_min_size(vec2(list_w, 150.0));
                            ui.set_max_width(list_w);
                            egui::ScrollArea::vertical().max_height(150.0).show(ui, |ui| {
                                if scs.is_empty() {
                                    ui.add(egui::Label::new(RichText::new(tr("scenario.none")).weak()).wrap());
                                }
                                for (i, s) in scs.iter().enumerate() {
                                    let r = ui.add(egui::Button::selectable(sel == i, &s.name).min_size(vec2(list_w - 10.0, 0.0)));
                                    if r.clicked() {
                                        sel = i;
                                    }
                                    if r.double_clicked() {
                                        act = Some(Act::Show);
                                    }
                                }
                            });
                        });
                        ui.vertical(|ui| {
                            let any = !scs.is_empty();
                            let w = vec2(120.0, 0.0);
                            if ui.add(egui::Button::new(format!("{}…", tr("scenario.add"))).min_size(w)).clicked() {
                                act = Some(Act::Add);
                            }
                            if ui.add_enabled(any, egui::Button::new(tr("scenario.delete")).min_size(w)).clicked() {
                                act = Some(Act::Delete);
                            }
                            if ui.add_enabled(any, egui::Button::new(format!("{}…", tr("scenario.edit"))).min_size(w)).clicked() {
                                act = Some(Act::Edit);
                            }
                            ui.add_space(6.0);
                            if ui.add_enabled(any, egui::Button::new(format!("{}…", tr("scenario.summary"))).min_size(w)).clicked() {
                                act = Some(Act::Summary);
                            }
                        });
                    });
                    if let Some(s) = scs.get(sel) {
                        ui.add_space(4.0);
                        egui::Grid::new("sc_info").num_columns(2).show(ui, |ui| {
                            row(ui, tr("scenario.changing"), |ui| {
                                ui.label(s.changing.iter().map(|c| c.to_a1()).collect::<Vec<_>>().join(";"));
                            });
                            row(ui, tr("scenario.comment"), |ui| {
                                ui.label(&s.comment);
                            });
                        });
                    }
                    ui.add_space(8.0);
                    ui.allocate_ui_with_layout(vec2(ui.available_width(), 24.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(tr("dialog.close")).clicked() {
                            closed = true;
                        }
                        if ui.add_enabled(!scs.is_empty(), egui::Button::new(RichText::new(tr("scenario.show")).strong())).clicked() {
                            act = Some(Act::Show);
                        }
                    });
                });
                m.selected = sel;
                if !open || closed {
                    return None;
                }
                match act {
                    Some(Act::Add) => {
                        let sel = self.sel();
                        m.mode = ScMode::Edit { orig: None, name: String::new(), changing: f10::abs_range(&sel), comment: trf("scenario.created_by", &[("user", &crate::common::user_name())]), };
                        self.f10.pick = f10::RefPick::new(self.sheet, Some(0));
                    }
                    Some(Act::Edit) => {
                        if let Some(s) = scs.get(m.selected) {
                            let changing = s.changing.iter().map(|c| f10::abs_range(&Range::single(*c))).collect::<Vec<_>>().join(";");
                            m.mode = ScMode::Edit { orig: Some(s.name.clone()), name: s.name.clone(), changing, comment: s.comment.clone() };
                            self.f10.pick = f10::RefPick::new(self.sheet, Some(0));
                        }
                    }
                    Some(Act::Delete) => {
                        if let Some(s) = scs.get(m.selected) {
                            if let Err(e) = self.wb.delete_scenario(self.sheet, &s.name) {
                                self.notices.push(Notice::refused(error_text(&e)));
                            }
                        }
                    }
                    Some(Act::Show) => {
                        if let Some(s) = scs.get(m.selected) {
                            if let Err(e) = self.wb.show_scenario(self.sheet, &s.name) {
                                self.notices.push(Notice::refused(error_text(&e)));
                            }
                        }
                    }
                    Some(Act::Summary) => {
                        // The result cells of the last report (kept in the file).
                        let last = self.wb.analysis.scenario_info.get(&self.sh().name).map_or(Vec::new(), |i| i.results.clone());
                        m.mode = ScMode::Summary { cells: last.iter().map(f10::abs_range).collect::<Vec<_>>().join(";") };
                        self.f10.pick = f10::RefPick::new(self.sheet, Some(0));
                    }
                    None => {}
                }
                Some(m)
            }
            ScMode::Edit { orig, name, changing, comment } => {
                let title = if orig.is_some() { tr("scenario.edit_title") } else { tr("scenario.add_title") };
                let mut pick = std::mem::take(&mut self.f10.pick);
                let res = tool_window(ctx, "scenario_edit", title, 340.0, focus, |ui| {
                    ui.label(tr("scenario.name"));
                    ui.add(egui::TextEdit::singleline(name).desired_width(320.0));
                    ui.label(tr("scenario.changing"));
                    ref_edit(ui, changing, 0, &mut pick, 320.0);
                    ui.label(RichText::new(tr("scenario.changing_hint")).small().weak());
                    ui.label(tr("scenario.comment"));
                    ui.add(egui::TextEdit::multiline(comment).desired_rows(2).desired_width(320.0));
                });
                self.f10.pick = pick;
                match res {
                    DialogResult::None => Some(m),
                    DialogResult::Cancel => Some(ScenarioMgr { selected: m.selected, mode: ScMode::List }),
                    DialogResult::Ok => {
                        let cells: Option<Vec<CellRef>> = ref_list(&self.wb, self.sheet, changing).map(|rs| rs.iter().flat_map(|r| (r.start.row..=r.end.row).flat_map(move |row| (r.start.col..=r.end.col).map(move |col| CellRef::new(row, col)))).take(33).collect());
                        match cells {
                            _ if name.trim().is_empty() => {
                                self.notices.push(Notice::error(tr("scenario.err_name")));
                                Some(m)
                            }
                            Some(cells) if cells.len() <= 32 => {
                                let old = orig.as_ref().and_then(|o| scs.iter().find(|s| s.name == *o));
                                let values = cells
                                    .iter()
                                    .map(|c| match old.and_then(|s| s.changing.iter().position(|x| x == c).and_then(|i| s.values.get(i))) {
                                        Some(v) => v.clone(),
                                        None => self.wb.edit_text(self.sheet, *c),
                                    })
                                    .collect();
                                Some(ScenarioMgr { selected: m.selected, mode: ScMode::Values { orig: orig.clone(), name: name.trim().to_string(), cells, comment: comment.clone(), values } })
                            }
                            Some(_) => {
                                self.notices.push(Notice::error(tr("scenario.err_too_many")));
                                Some(m)
                            }
                            None => {
                                self.notices.push(Notice::error(tr("whatif.err_range")));
                                Some(m)
                            }
                        }
                    }
                }
            }
            ScMode::Values { orig, name, cells, comment, values } => {
                let res = tool_window(ctx, "scenario_values", tr("scenario.values_title"), 320.0, focus, |ui| {
                    ui.label(tr("scenario.values_prompt"));
                    egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                        egui::Grid::new("sc_values").num_columns(2).show(ui, |ui| {
                            for (c, v) in cells.iter().zip(values.iter_mut()) {
                                row(ui, &format!("{}:", f10::abs_range(&Range::single(*c))), |ui| {
                                    ui.add(egui::TextEdit::singleline(v).desired_width(180.0));
                                });
                            }
                        });
                    });
                });
                match res {
                    DialogResult::None => Some(m),
                    DialogResult::Cancel => Some(ScenarioMgr { selected: m.selected, mode: ScMode::List }),
                    DialogResult::Ok => {
                        // An edited scenario keeps what the dialog doesn't show.
                        let base = orig.as_ref().and_then(|o| self.wb.scenarios(self.sheet).iter().find(|s| s.name.eq_ignore_ascii_case(o)).cloned()).unwrap_or(Scenario { locked: true, user: crate::common::user_name(), ..Default::default() });
                        let sc = Scenario { name: name.clone(), changing: cells.clone(), values: values.clone(), comment: comment.clone(), ..base };
                        let before = self.wb.undo_history().len();
                        if let Some(o) = orig.as_ref().filter(|o| !o.eq_ignore_ascii_case(name)) {
                            let _ = self.wb.delete_scenario(self.sheet, o);
                        }
                        match self.wb.add_scenario(self.sheet, sc) {
                            Ok(()) => {
                                let steps = self.wb.undo_history().len().saturating_sub(before);
                                self.wb.merge_undo(steps, "undo.scenario");
                                let sel = self.wb.scenarios(self.sheet).iter().position(|s| s.name.eq_ignore_ascii_case(name)).unwrap_or(0);
                                Some(ScenarioMgr { selected: sel, mode: ScMode::List })
                            }
                            Err(e) => {
                                self.notices.push(Notice::refused(error_text(&e)));
                                Some(m)
                            }
                        }
                    }
                }
            }
            ScMode::Summary { cells } => {
                let mut pick = std::mem::take(&mut self.f10.pick);
                let res = tool_window(ctx, "scenario_summary", tr("scenario.summary_title"), 320.0, focus, |ui| {
                    ui.label(tr("scenario.result_cells"));
                    ref_edit(ui, cells, 0, &mut pick, 300.0);
                });
                self.f10.pick = pick;
                match res {
                    DialogResult::None => Some(m),
                    DialogResult::Cancel => Some(ScenarioMgr { selected: m.selected, mode: ScMode::List }),
                    DialogResult::Ok => {
                        let list = if cells.trim().is_empty() { Some(Vec::new()) } else { ref_list(&self.wb, self.sheet, cells) };
                        let Some(list) = list else {
                            self.notices.push(Notice::error(tr("whatif.err_range")));
                            return Some(m);
                        };
                        let result: Vec<CellRef> = list.iter().flat_map(|r| (r.start.row..=r.end.row).flat_map(move |row| (r.start.col..=r.end.col).map(move |col| CellRef::new(row, col)))).take(200).collect();
                        match self.wb.scenario_summary(self.sheet, &result, lang()) {
                            Ok(s) => {
                                self.select_sheet(s);
                                None
                            }
                            Err(e) => {
                                self.notices.push(Notice::refused(error_text(&e)));
                                Some(m)
                            }
                        }
                    }
                }
            }
        }
    }

    // --- Solver ------------------------------------------------------------------------------

    fn solver_model(&self, d: &SolverDlg) -> Result<SolverModel, &'static str> {
        let (si, obj) = self.cell_ref(&d.objective).ok_or(tr("solver.err_objective"))?;
        if si != self.sheet {
            return Err(tr("solver.err_objective"));
        }
        let variables = ref_list(&self.wb, self.sheet, &d.vars).ok_or(tr("solver.err_vars"))?;
        let goal = match d.goal {
            0 => SolverGoal::Max,
            1 => SolverGoal::Min,
            _ => SolverGoal::Value(parse_num(&self.wb, &d.value).ok_or(tr("goal.err_number"))?),
        };
        let def = SolverModel::default();
        Ok(SolverModel {
            objective: obj,
            goal,
            variables,
            constraints: d.cons.clone(),
            method: d.method,
            non_negative: d.non_neg,
            max_time: parse_num(&self.wb, &d.max_time).filter(|v| *v >= 0.0).unwrap_or(def.max_time),
            max_iter: d.max_iter.trim().parse().unwrap_or(def.max_iter),
            precision: parse_num(&self.wb, &d.precision).filter(|v| *v > 0.0).unwrap_or(def.precision),
            multistart: d.multistart,
            seed: None,
            kept: self.wb.solver_model(self.sheet).map_or(Vec::new(), |m| m.kept.clone()),
        })
    }

    fn solver_dialog(&mut self, ctx: &egui::Context, mut d: Box<SolverDlg>) -> Option<AnalysisDlg> {
        let focus = self.grid_has_focus(ctx);
        // The constraint box over the main dialog.
        if let Some(mut c) = d.sub.take() {
            let mut pick = std::mem::take(&mut self.f10.pick);
            let mut add = false;
            let title = if c.index.is_some() { tr("solver.change_constraint") } else { tr("solver.add_constraint") };
            let res = tool_window(ctx, "solver_constraint", title, 420.0, focus, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(tr("solver.cell_ref"));
                        ref_edit(ui, &mut c.lhs, 2, &mut pick, 160.0);
                    });
                    ui.vertical(|ui| {
                        ui.label(" ");
                        egui::ComboBox::from_id_salt("solver_op").width(60.0).selected_text(op_text(c.op)).show_ui(ui, |ui| {
                            for op in [ConstraintOp::Le, ConstraintOp::Eq, ConstraintOp::Ge, ConstraintOp::Int, ConstraintOp::Bin] {
                                ui.selectable_value(&mut c.op, op, op_text(op));
                            }
                        });
                    });
                    ui.vertical(|ui| {
                        ui.label(tr("solver.constraint"));
                        let integral = matches!(c.op, ConstraintOp::Int | ConstraintOp::Bin);
                        ui.add_enabled_ui(!integral, |ui| ref_edit(ui, &mut c.rhs, 3, &mut pick, 160.0));
                    });
                });
                ui.add_space(4.0);
                if ui.button(tr("solver.add")).clicked() {
                    add = true;
                }
            });
            self.f10.pick = pick;
            let store = |d: &mut SolverDlg, c: &ConsEdit, wb: &Workbook, sheet: usize| -> bool {
                let Some((si, lhs)) = f10::parse_ref(wb, sheet, &c.lhs) else { return false };
                let integral = matches!(c.op, ConstraintOp::Int | ConstraintOp::Bin);
                if si != sheet || (!integral && c.rhs.trim().is_empty()) {
                    return false;
                }
                let con = Constraint { lhs, op: c.op, rhs: if integral { String::new() } else { c.rhs.trim().trim_start_matches('=').to_string() } };
                match c.index {
                    Some(i) if i < d.cons.len() => d.cons[i] = con,
                    _ => d.cons.push(con),
                }
                true
            };
            if add || res == DialogResult::Ok {
                if !store(&mut d, &c, &self.wb, self.sheet) {
                    self.notices.push(Notice::error(tr("solver.err_constraint")));
                    d.sub = Some(c);
                } else if add {
                    d.sub = Some(ConsEdit { index: None, lhs: String::new(), op: ConstraintOp::Le, rhs: String::new() });
                    self.f10.pick = f10::RefPick::new(self.sheet, Some(2));
                }
            } else if res == DialogResult::None {
                d.sub = Some(c);
            }
            if d.sub.is_none() {
                self.f10.pick = f10::RefPick::new(self.sheet, Some(1));
            }
            return Some(AnalysisDlg::Solver(d));
        }
        let mut pick = std::mem::take(&mut self.f10.pick);
        enum Act {
            Solve,
            Close,
            Add,
            Change,
            Delete,
            Reset,
        }
        let mut act = None;
        let open = plain_window(ctx, "solver", tr("solver.title"), 460.0, |ui| {
            egui::Grid::new("solver_grid").num_columns(2).show(ui, |ui| {
                row_ref(ui, tr("solver.set_objective"), &mut d.objective, 0, &mut pick);
                row(ui, tr("solver.to"), |ui| {
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut d.goal, 0, tr("solver.max"));
                        ui.radio_value(&mut d.goal, 1, tr("solver.min"));
                        ui.radio_value(&mut d.goal, 2, tr("solver.value_of"));
                        ui.add_enabled(d.goal == 2, egui::TextEdit::singleline(&mut d.value).desired_width(70.0));
                    });
                });
                row_ref(ui, tr("solver.by_changing"), &mut d.vars, 1, &mut pick);
            });
            ui.add_space(4.0);
            ui.label(tr("solver.subject_to"));
            ui.horizontal(|ui| {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_min_size(vec2(300.0, 110.0));
                    egui::ScrollArea::vertical().max_height(110.0).show(ui, |ui| {
                        for (i, c) in d.cons.iter().enumerate() {
                            let text = match c.op {
                                ConstraintOp::Int | ConstraintOp::Bin => format!("{} = {}", f10::abs_range(&c.lhs), op_text(c.op)),
                                _ => format!("{} {} {}", f10::abs_range(&c.lhs), op_text(c.op), c.rhs),
                            };
                            let r = ui.add(egui::Button::selectable(d.sel == Some(i), RichText::new(text).monospace()).min_size(vec2(290.0, 0.0)));
                            if r.clicked() {
                                d.sel = Some(i);
                            }
                            if r.double_clicked() {
                                act = Some(Act::Change);
                            }
                        }
                    });
                });
                ui.vertical(|ui| {
                    let w = vec2(110.0, 0.0);
                    if ui.add(egui::Button::new(format!("{}…", tr("solver.add"))).min_size(w)).clicked() {
                        act = Some(Act::Add);
                    }
                    let has = d.sel.is_some_and(|i| i < d.cons.len());
                    if ui.add_enabled(has, egui::Button::new(format!("{}…", tr("solver.change"))).min_size(w)).clicked() {
                        act = Some(Act::Change);
                    }
                    if ui.add_enabled(has, egui::Button::new(tr("solver.delete")).min_size(w)).clicked() {
                        act = Some(Act::Delete);
                    }
                    if ui.add(egui::Button::new(tr("solver.reset")).min_size(w)).clicked() {
                        act = Some(Act::Reset);
                    }
                });
            });
            ui.checkbox(&mut d.non_neg, tr("solver.non_negative"));
            ui.horizontal(|ui| {
                ui.label(tr("solver.method"));
                let name = |m: SolverMethod| if m == SolverMethod::SimplexLp { tr("solver.simplex") } else { tr("solver.grg") };
                egui::ComboBox::from_id_salt("solver_method").width(160.0).selected_text(name(d.method)).show_ui(ui, |ui| {
                    ui.selectable_value(&mut d.method, SolverMethod::Nonlinear, name(SolverMethod::Nonlinear));
                    ui.selectable_value(&mut d.method, SolverMethod::SimplexLp, name(SolverMethod::SimplexLp));
                });
                if ui.selectable_label(d.options, format!("{}…", tr("solver.options"))).clicked() {
                    d.options = !d.options;
                }
            });
            ui.label(RichText::new(if d.method == SolverMethod::SimplexLp { tr("solver.simplex_hint") } else { tr("solver.grg_hint") }).small().weak());
            if d.options {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    egui::Grid::new("solver_opts").num_columns(2).show(ui, |ui| {
                        row(ui, tr("solver.max_time"), |ui| {
                            ui.add(egui::TextEdit::singleline(&mut d.max_time).desired_width(80.0));
                        });
                        row(ui, tr("solver.iterations"), |ui| {
                            ui.add(egui::TextEdit::singleline(&mut d.max_iter).desired_width(80.0));
                        });
                        row(ui, tr("solver.precision"), |ui| {
                            ui.add(egui::TextEdit::singleline(&mut d.precision).desired_width(80.0));
                        });
                    });
                    ui.add_enabled(d.method == SolverMethod::Nonlinear, egui::Checkbox::new(&mut d.multistart, tr("solver.multistart")));
                });
            }
            ui.add_space(8.0);
            ui.allocate_ui_with_layout(vec2(ui.available_width(), 24.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("dialog.close")).clicked() {
                    act = Some(Act::Close);
                }
                if ui.button(RichText::new(tr("solver.solve")).strong()).clicked() {
                    act = Some(Act::Solve);
                }
            });
        });
        self.f10.pick = pick;
        if !open {
            return None;
        }
        match act {
            Some(Act::Add) => {
                d.sub = Some(ConsEdit { index: None, lhs: String::new(), op: ConstraintOp::Le, rhs: String::new() });
                self.f10.pick = f10::RefPick::new(self.sheet, Some(2));
            }
            Some(Act::Change) => {
                if let Some(i) = d.sel.filter(|i| *i < d.cons.len()) {
                    let c = &d.cons[i];
                    d.sub = Some(ConsEdit { index: Some(i), lhs: f10::abs_range(&c.lhs), op: c.op, rhs: c.rhs.clone() });
                    self.f10.pick = f10::RefPick::new(self.sheet, Some(2));
                }
            }
            Some(Act::Delete) => {
                if let Some(i) = d.sel.filter(|i| *i < d.cons.len()) {
                    d.cons.remove(i);
                    d.sel = None;
                }
            }
            Some(Act::Reset) => {
                d.cons.clear();
                d.sel = None;
            }
            Some(Act::Close) => {
                // Close keeps the model, like Excel.
                if let Ok(m) = self.solver_model(&d) {
                    if self.wb.solver_model(self.sheet) != Some(&m) {
                        self.wb.save_solver_model(self.sheet, m);
                    }
                }
                return None;
            }
            Some(Act::Solve) => match self.solver_model(&d) {
                Ok(m) => {
                    if self.wb.solver_model(self.sheet) != Some(&m) {
                        self.wb.save_solver_model(self.sheet, m.clone());
                    }
                    let r = self.wb.solve(self.sheet, &m);
                    let applied = r.is_solution() && self.wb.undo_history().first() == Some(&"undo.solver") && self.wb.solver_model(self.sheet) == Some(&m);
                    return Some(AnalysisDlg::SolverResult { r, keep: true, applied, dlg: d });
                }
                Err(e) => self.notices.push(Notice::error(e)),
            },
            None => {}
        }
        Some(AnalysisDlg::Solver(d))
    }

    // --- Data Analysis tools -----------------------------------------------------------------

    pub(super) fn tool_dialog(&mut self, ctx: &egui::Context, mut d: ToolDlg) -> Option<ToolDlg> {
        let focus = self.grid_has_focus(ctx);
        let mut pick = std::mem::take(&mut self.f10.pick);
        let res = tool_window(ctx, "analysis_tool", tr(d.tool.key()), 400.0, focus, |ui| d.ui(ui, &mut pick));
        self.f10.pick = pick;
        match res {
            DialogResult::None => Some(d),
            DialogResult::Cancel => None,
            DialogResult::Ok => match self.run_tool(&d) {
                Ok(()) => None,
                Err(e) => {
                    self.notices.push(Notice::error(e));
                    Some(d)
                }
            },
        }
    }

    fn run_tool(&mut self, d: &ToolDlg) -> Result<(), String> {
        let lang = lang();
        let bad_range = || tr("whatif.err_range").to_string();
        let input = |text: &str| f10::parse_ref(&self.wb, self.sheet, text).ok_or_else(bad_range);
        let num = |text: &str| parse_num(&self.wb, text).ok_or_else(|| tr("tools.err_param").to_string());
        let first = |si: usize, r: Range| -> Series { self.wb.read_series(si, &r, false, d.labels).into_iter().next().unwrap_or_default() };
        let st = |r: Result<apex_sheet::analysis::Table, StatsError>| r.map_err(|e| stats_error(e).to_string());
        let t = match d.tool {
            Tool::Random => {
                let vars = d.vars.trim().parse::<usize>().unwrap_or(1).max(1);
                let count = d.count.trim().parse::<usize>().map_err(|_| tr("tools.err_param").to_string())?;
                let (a, b) = (num(&d.p1), num(&d.p2));
                let dist = match d.dist {
                    0 => RandomDist::Uniform { from: a?, to: b? },
                    1 => RandomDist::Normal { mean: a?, sd: b? },
                    2 => RandomDist::Bernoulli { p: a? },
                    3 => RandomDist::Binomial { p: a?, trials: b? as u64 },
                    _ => RandomDist::Poisson { lambda: a? },
                };
                let seed = d.seed.trim().parse::<u64>().ok();
                st(stats::random_numbers(vars, count, dist, seed))?
            }
            _ => {
                let (si, r) = input(&d.input)?;
                match d.tool {
                    Tool::Anova => st(stats::anova_single(&self.wb.read_series(si, &r, d.by_rows, d.labels), num(&d.alpha)?, lang, d.by_rows))?,
                    Tool::Correlation => st(stats::correlation(&self.wb.read_series(si, &r, d.by_rows, d.labels), lang))?,
                    Tool::Covariance => st(stats::covariance(&self.wb.read_series(si, &r, d.by_rows, d.labels), lang))?,
                    Tool::RankPercentile => st(stats::rank_percentile(&self.wb.read_series(si, &r, d.by_rows, d.labels), lang))?,
                    Tool::Descriptive => {
                        let o = stats::DescriptiveOpts {
                            summary: d.summary,
                            confidence: if d.conf_on { Some(num(&d.conf)?) } else { None },
                            kth_largest: if d.large_on { d.large.trim().parse().ok() } else { None },
                            kth_smallest: if d.small_on { d.small.trim().parse().ok() } else { None },
                        };
                        st(stats::descriptive(&self.wb.read_series(si, &r, d.by_rows, d.labels), &o, lang))?
                    }
                    Tool::ExpSmoothing => st(stats::exp_smoothing(&first(si, r).values, num(&d.damping)?, d.std_errors))?,
                    Tool::MovingAverage => st(stats::moving_average(&first(si, r).values, d.interval.trim().parse().unwrap_or(3), d.std_errors))?,
                    Tool::Histogram => {
                        let data = first(si, r).nums();
                        let bins = if d.input2.trim().is_empty() {
                            None
                        } else {
                            let (bs, br) = input(&d.input2)?;
                            Some(first(bs, br))
                        };
                        let o = stats::HistogramOpts { pareto: d.pareto, cumulative: d.cumulative };
                        st(stats::histogram(&data, bins.as_ref().map(|b| b.nums()).as_deref(), bins.as_ref().and_then(|b| b.label.as_deref()), &o, lang))?
                    }
                    Tool::Sampling => {
                        let n = d.sample_n.trim().parse::<usize>().map_err(|_| tr("tools.err_param").to_string())?;
                        let m = if d.sample_random { SampleMethod::Random(n) } else { SampleMethod::Periodic(n) };
                        st(stats::sampling(&first(si, r).nums(), m, None))?
                    }
                    Tool::Regression => {
                        let y = first(si, r);
                        let (xs_i, xr) = input(&d.input2)?;
                        let xs = self.wb.read_series(xs_i, &xr, false, d.labels);
                        let o = stats::RegressionOpts { constant_zero: d.const_zero, confidence: if d.conf_on { num(&d.conf)? } else { 95.0 }, residuals: d.residuals, standardized: d.standardized };
                        st(stats::regression(&y, &xs, &o, lang))?
                    }
                    Tool::FTest | Tool::TTestPaired | Tool::TTestEqual | Tool::TTestUnequal => {
                        let a = first(si, r);
                        let (bi, br) = input(&d.input2)?;
                        let b = first(bi, br);
                        let alpha = num(&d.alpha)?;
                        match d.tool {
                            Tool::FTest => st(stats::f_test(&a, &b, alpha, lang))?,
                            Tool::TTestPaired => st(stats::t_test(&a, &b, TTestKind::Paired, num(&d.hyp)?, alpha, lang))?,
                            Tool::TTestEqual => st(stats::t_test(&a, &b, TTestKind::EqualVariance, num(&d.hyp)?, alpha, lang))?,
                            _ => st(stats::t_test(&a, &b, TTestKind::UnequalVariance, num(&d.hyp)?, alpha, lang))?,
                        }
                    }
                    Tool::Random => return Err(bad_range()),
                }
            }
        };
        let out = if d.new_sheet {
            let name = d.sheet_name.trim();
            Output::NewSheet(if name.is_empty() { lang.text("sheet_stats") } else { name.to_string() })
        } else {
            let (si, r) = f10::parse_ref(&self.wb, self.sheet, &d.out_range).ok_or_else(bad_range)?;
            Output::At { sheet: si, cell: r.start }
        };
        match self.wb.write_table(&t, &out, "undo.analysis_tool") {
            Ok((si, area)) => {
                if si != self.sheet {
                    self.select_sheet(si);
                }
                self.anchor = area.start;
                self.cursor = area.start;
                self.ensure_visible = true;
                Ok(())
            }
            Err(e) => Err(error_text(&e)),
        }
    }
}

/// A grid row with a reference box.
fn row_ref(ui: &mut egui::Ui, label: &str, text: &mut String, key: u32, pick: &mut f10::RefPick) {
    ui.label(label);
    ref_edit(ui, text, key, pick, 180.0);
    ui.end_row();
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Tool {
    Anova,
    Correlation,
    Covariance,
    Descriptive,
    ExpSmoothing,
    FTest,
    Histogram,
    MovingAverage,
    Random,
    RankPercentile,
    Regression,
    Sampling,
    TTestPaired,
    TTestEqual,
    TTestUnequal,
}

const TOOLS: [Tool; 15] = [
    Tool::Anova,
    Tool::Correlation,
    Tool::Covariance,
    Tool::Descriptive,
    Tool::ExpSmoothing,
    Tool::FTest,
    Tool::Histogram,
    Tool::MovingAverage,
    Tool::Random,
    Tool::RankPercentile,
    Tool::Regression,
    Tool::Sampling,
    Tool::TTestPaired,
    Tool::TTestEqual,
    Tool::TTestUnequal,
];

impl Tool {
    fn key(self) -> &'static str {
        match self {
            Tool::Anova => "tools.anova",
            Tool::Correlation => "tools.correlation",
            Tool::Covariance => "tools.covariance",
            Tool::Descriptive => "tools.descriptive",
            Tool::ExpSmoothing => "tools.exp_smoothing",
            Tool::FTest => "tools.f_test",
            Tool::Histogram => "tools.histogram",
            Tool::MovingAverage => "tools.moving_average",
            Tool::Random => "tools.random",
            Tool::RankPercentile => "tools.rank",
            Tool::Regression => "tools.regression",
            Tool::Sampling => "tools.sampling",
            Tool::TTestPaired => "tools.t_paired",
            Tool::TTestEqual => "tools.t_equal",
            Tool::TTestUnequal => "tools.t_unequal",
        }
    }

    /// Tools that read two ranges.
    fn two_inputs(self) -> bool {
        matches!(self, Tool::FTest | Tool::TTestPaired | Tool::TTestEqual | Tool::TTestUnequal | Tool::Regression | Tool::Histogram)
    }

    fn grouped(self) -> bool {
        matches!(self, Tool::Anova | Tool::Correlation | Tool::Covariance | Tool::Descriptive | Tool::RankPercentile)
    }
}

/// The input dialog of a Data Analysis tool.
pub(super) struct ToolDlg {
    tool: Tool,
    input: String,
    input2: String,
    by_rows: bool,
    labels: bool,
    alpha: String,
    new_sheet: bool,
    out_range: String,
    sheet_name: String,
    summary: bool,
    conf_on: bool,
    conf: String,
    large_on: bool,
    large: String,
    small_on: bool,
    small: String,
    interval: String,
    damping: String,
    std_errors: bool,
    pareto: bool,
    cumulative: bool,
    hyp: String,
    const_zero: bool,
    residuals: bool,
    standardized: bool,
    sample_random: bool,
    sample_n: String,
    vars: String,
    count: String,
    dist: u8,
    p1: String,
    p2: String,
    seed: String,
}

impl ToolDlg {
    fn new(tool: Tool, sel: &Range, dec: char, header: bool) -> ToolDlg {
        let input = if sel.is_single() { String::new() } else { f10::abs_range(sel) };
        ToolDlg {
            tool,
            input,
            input2: String::new(),
            by_rows: false,
            labels: header && !sel.is_single(),
            alpha: format!("0{dec}05"),
            new_sheet: true,
            out_range: String::new(),
            sheet_name: String::new(),
            summary: true,
            conf_on: tool == Tool::Descriptive,
            conf: "95".into(),
            large_on: false,
            large: "1".into(),
            small_on: false,
            small: "1".into(),
            interval: "3".into(),
            damping: format!("0{dec}3"),
            std_errors: false,
            pareto: false,
            cumulative: false,
            hyp: "0".into(),
            const_zero: false,
            residuals: false,
            standardized: false,
            sample_random: true,
            sample_n: "10".into(),
            vars: "1".into(),
            count: "10".into(),
            dist: 0,
            p1: "0".into(),
            p2: "1".into(),
            seed: String::new(),
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, pick: &mut f10::RefPick) {
        let t = self.tool;
        ui.strong(tr("tools.input"));
        egui::Grid::new("tool_input").num_columns(2).show(ui, |ui| {
            if t == Tool::Random {
                row(ui, tr("tools.num_vars"), |ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.vars).desired_width(80.0));
                });
                row(ui, tr("tools.num_random"), |ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.count).desired_width(80.0));
                });
                row(ui, tr("tools.distribution"), |ui| {
                    let names = ["tools.dist_uniform", "tools.dist_normal", "tools.dist_bernoulli", "tools.dist_binomial", "tools.dist_poisson"];
                    egui::ComboBox::from_id_salt("tool_dist").width(160.0).selected_text(tr(names[self.dist as usize % 5])).show_ui(ui, |ui| {
                        for (i, k) in names.iter().enumerate() {
                            ui.selectable_value(&mut self.dist, i as u8, tr(k));
                        }
                    });
                });
                let (a, b) = match self.dist {
                    0 => ("tools.between", Some("tools.and")),
                    1 => ("tools.mean", Some("tools.std_dev")),
                    2 => ("tools.p_value", None),
                    3 => ("tools.p_value", Some("tools.trials")),
                    _ => ("tools.lambda", None),
                };
                row(ui, tr(a), |ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.p1).desired_width(80.0));
                });
                if let Some(b) = b {
                    row(ui, tr(b), |ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.p2).desired_width(80.0));
                    });
                }
                row(ui, tr("tools.seed"), |ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.seed).desired_width(80.0));
                });
                return;
            }
            let (k1, k2) = match t {
                Tool::Regression => ("tools.input_y", "tools.input_x"),
                Tool::Histogram => ("tools.input_range", "tools.bin_range"),
                _ if t.two_inputs() => ("tools.var1", "tools.var2"),
                _ => ("tools.input_range", ""),
            };
            row_ref(ui, tr(k1), &mut self.input, 0, pick);
            if t.two_inputs() {
                row_ref(ui, tr(k2), &mut self.input2, 1, pick);
            }
            if matches!(t, Tool::TTestPaired | Tool::TTestEqual | Tool::TTestUnequal) {
                row(ui, tr("tools.hyp_diff"), |ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.hyp).desired_width(80.0));
                });
            }
            if t.grouped() {
                row(ui, tr("tools.grouped_by"), |ui| {
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut self.by_rows, false, tr("tools.columns"));
                        ui.radio_value(&mut self.by_rows, true, tr("tools.rows"));
                    });
                });
            }
        });
        if t == Tool::Random {
            return self.output_ui(ui, pick);
        }
        ui.checkbox(&mut self.labels, if t.grouped() && self.by_rows { tr("tools.labels_col") } else { tr("tools.labels") });
        match t {
            Tool::Anova | Tool::FTest | Tool::TTestPaired | Tool::TTestEqual | Tool::TTestUnequal => {
                ui.horizontal(|ui| {
                    ui.label(tr("tools.alpha"));
                    ui.add(egui::TextEdit::singleline(&mut self.alpha).desired_width(60.0));
                });
            }
            Tool::ExpSmoothing => {
                ui.horizontal(|ui| {
                    ui.label(tr("tools.damping"));
                    ui.add(egui::TextEdit::singleline(&mut self.damping).desired_width(60.0));
                });
            }
            Tool::MovingAverage => {
                ui.horizontal(|ui| {
                    ui.label(tr("tools.interval"));
                    ui.add(egui::TextEdit::singleline(&mut self.interval).desired_width(60.0));
                });
            }
            Tool::Regression => {
                ui.checkbox(&mut self.const_zero, tr("tools.const_zero"));
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.conf_on, tr("tools.conf_level"));
                    ui.add_enabled(self.conf_on, egui::TextEdit::singleline(&mut self.conf).desired_width(50.0));
                    ui.label("%");
                });
            }
            Tool::Sampling => {
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.sample_random, false, tr("tools.periodic"));
                    ui.radio_value(&mut self.sample_random, true, tr("tools.random_n"));
                    ui.add(egui::TextEdit::singleline(&mut self.sample_n).desired_width(60.0));
                });
            }
            _ => {}
        }
        self.output_ui(ui, pick);
        match t {
            Tool::Descriptive => {
                ui.checkbox(&mut self.summary, tr("tools.summary"));
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.conf_on, tr("tools.conf_mean"));
                    ui.add_enabled(self.conf_on, egui::TextEdit::singleline(&mut self.conf).desired_width(50.0));
                    ui.label("%");
                });
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.large_on, tr("tools.kth_largest"));
                    ui.add_enabled(self.large_on, egui::TextEdit::singleline(&mut self.large).desired_width(50.0));
                });
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.small_on, tr("tools.kth_smallest"));
                    ui.add_enabled(self.small_on, egui::TextEdit::singleline(&mut self.small).desired_width(50.0));
                });
            }
            Tool::ExpSmoothing | Tool::MovingAverage => {
                ui.checkbox(&mut self.std_errors, tr("tools.std_errors"));
            }
            Tool::Histogram => {
                ui.checkbox(&mut self.pareto, tr("tools.pareto"));
                ui.checkbox(&mut self.cumulative, tr("tools.cumulative"));
            }
            Tool::Regression => {
                ui.checkbox(&mut self.residuals, tr("tools.residuals"));
                ui.checkbox(&mut self.standardized, tr("tools.std_residuals"));
            }
            _ => {}
        }
    }

    fn output_ui(&mut self, ui: &mut egui::Ui, pick: &mut f10::RefPick) {
        ui.add_space(4.0);
        ui.strong(tr("tools.output"));
        egui::Grid::new("tool_output").num_columns(2).show(ui, |ui| {
            ui.radio_value(&mut self.new_sheet, false, tr("tools.output_range"));
            ref_edit(ui, &mut self.out_range, 2, pick, 180.0);
            ui.end_row();
            ui.radio_value(&mut self.new_sheet, true, tr("tools.new_sheet"));
            ui.add(egui::TextEdit::singleline(&mut self.sheet_name).desired_width(180.0));
            ui.end_row();
        });
        if pick.field == Some(2) && !self.out_range.is_empty() {
            self.new_sheet = false;
        }
    }
}
