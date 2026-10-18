//! Planning: Insert > Planning (a blank plan, templates, converting a
//! table), the contextual Planning tab of a plan table and the Project
//! Settings dialog. The Gantt beside the table is in `gantt.rs`.

use std::rc::Rc;

use apex_sheet::format::NumFormat;
use apex_sheet::plan::{Calendar, DepType, Exception, Field, Warning, holidays};
use apex_sheet::workbook::{NewTask, PlanReport, PlanView, TableError, TableId};

use super::f10::parse_num;
use super::gantt::{GanttDrag, RowsGeo, Scale};
use super::*;
use crate::toolstrip::{self, Body};
use crate::widgets::DialogResult;
use crate::widgets::FitWindow as _;

/// The Planning state of a sheet view.
pub(super) struct PlanState {
    /// The Gantt beside plan tables is hidden (Planning > Gantt Chart).
    pub hidden: bool,
    pub scale: Scale,
    /// The day at the left edge of the Gantt (None: the project start).
    pub left: Option<f64>,
    /// Resource Usage under the Gantt.
    pub load: bool,
    pub drag: Option<GanttDrag>,
    /// Where the grid put its rows on the last frame.
    pub rows: Option<RowsGeo>,
    pub settings: Option<Box<SettingsDlg>>,
    pub custom: Option<Box<super::plan_fields_ui::CustomDlg>>,
    /// Print Gantt: its dialog, and its options for the next print job.
    pub print: Option<Box<super::gantt_print::GanttPrint>>,
    pub print_next: Option<Box<super::gantt_print::GanttPrint>>,
    /// Planning > Views: what the panel beside the plan shows.
    pub views: super::plan_views::ViewsState,
    pub warnings: bool,
    /// The scheduled plan shown, by workbook revision.
    cache: Option<(u64, TableId, Rc<PlanView>)>,
    names_lang: &'static str,
}

impl Default for PlanState {
    fn default() -> Self {
        PlanState { hidden: false, scale: Scale::Week, left: None, load: false, drag: None, rows: None, settings: None, custom: None, print: None, print_next: None, views: Default::default(), warnings: false, cache: None, names_lang: "" }
    }
}

/// Plan templates: the i18n key of their name and their tasks as (name
/// key, outline level, working days, predecessors).
type Template = (&'static str, &'static [(&'static str, u8, f64, &'static str)]);

pub(super) const TEMPLATES: [Template; 3] = [
    (
        "plan.tpl_construction",
        &[
            ("plan.t.design_phase", 0, 0.0, ""),
            ("plan.t.site_survey", 1, 5.0, ""),
            ("plan.t.architecture", 1, 15.0, "2"),
            ("plan.t.permits", 1, 20.0, "3"),
            ("plan.t.build_phase", 0, 0.0, ""),
            ("plan.t.foundations", 1, 10.0, "4"),
            ("plan.t.structure", 1, 25.0, "6"),
            ("plan.t.roof", 1, 10.0, "7"),
            ("plan.t.installations", 1, 15.0, "8SS+5"),
            ("plan.t.finishes", 1, 20.0, "9"),
            ("plan.t.handover", 0, 0.0, "10"),
        ],
    ),
    (
        "plan.tpl_event",
        &[
            ("plan.t.planning", 0, 0.0, ""),
            ("plan.t.budget", 1, 3.0, ""),
            ("plan.t.venue", 1, 5.0, "2"),
            ("plan.t.suppliers", 1, 10.0, "3"),
            ("plan.t.promotion", 0, 0.0, ""),
            ("plan.t.invitations", 1, 5.0, "3"),
            ("plan.t.registration", 1, 15.0, "6"),
            ("plan.t.event_day", 0, 0.0, ""),
            ("plan.t.setup", 1, 1.0, "4;7"),
            ("plan.t.event", 1, 1.0, "9"),
            ("plan.t.survey", 1, 3.0, "10"),
            ("plan.t.closing", 0, 0.0, "11"),
        ],
    ),
    (
        "plan.tpl_software",
        &[
            ("plan.t.preparation", 0, 0.0, ""),
            ("plan.t.requirements", 1, 5.0, ""),
            ("plan.t.pilot_plan", 1, 3.0, "2"),
            ("plan.t.deployment", 0, 0.0, ""),
            ("plan.t.test_env", 1, 3.0, "3"),
            ("plan.t.pilot", 1, 10.0, "5"),
            ("plan.t.training", 1, 5.0, "6SS+5"),
            ("plan.t.rollout", 1, 10.0, "6"),
            ("plan.t.migration", 1, 5.0, "6"),
            ("plan.t.go_live", 0, 0.0, "8;9"),
            ("plan.t.support", 0, 10.0, "10"),
        ],
    ),
];

/// The tasks of template `i` in the interface language.
pub(super) fn template_tasks(i: usize) -> Vec<NewTask> {
    TEMPLATES[i].1.iter().map(|(k, level, days, pred)| NewTask::new(apex_i18n::lookup(k).unwrap_or(k), *level, *days, pred)).collect()
}

/// Column names of plans in the interface language (for new columns and
/// to recognise headers when converting a table).
pub(super) fn sync_field_names(state: &mut PlanState) {
    let lang = crate::i18n::current();
    if state.names_lang == lang {
        return;
    }
    state.names_lang = lang;
    let names = Field::ALL.iter().map(|f| apex_i18n::lookup(&format!("plan.field.{}", f.key())).unwrap_or("").to_string()).collect();
    apex_sheet::workbook::set_field_names(names);
}

/// A date for a text box.
pub(super) fn date_text(wb: &Workbook, day: f64) -> String {
    NumFormat::parse(wb.locale.short_date_code()).format_number(day.floor(), &wb.locale).0
}

/// The text of a scheduling warning (tasks by position, id - 1).
pub(super) fn warning_text(w: &Warning) -> String {
    let id = |i: usize| (i + 1).to_string();
    match w {
        Warning::Cycle(list) => trf("plan.warn_cycle", &[("ids", &list.iter().map(|i| id(*i)).collect::<Vec<_>>().join(", "))]),
        Warning::BadLink { task, pred } => trf("plan.warn_bad_link", &[("task", &id(*task)), ("pred", &id(*pred))]),
        Warning::SummaryLink { task, pred } => trf("plan.warn_summary_link", &[("task", &id(*task)), ("pred", &id(*pred))]),
        Warning::LinkConflict { task, pred } => trf("plan.warn_conflict", &[("task", &id(*task)), ("pred", &id(*pred))]),
        Warning::ConstraintMissed { task } => trf("plan.warn_missed", &[("task", &id(*task))]),
    }
}

impl SheetView {
    /// The task plan at the cursor.
    pub(super) fn cursor_plan(&self) -> Option<TableId> {
        self.wb.plan_at(self.sheet, self.cursor)
    }

    /// The plan the Gantt shows: the one at the cursor, else the first on
    /// the sheet.
    pub(super) fn shown_plan(&self) -> Option<TableId> {
        self.cursor_plan().or_else(|| self.wb.task_plans().into_iter().find(|id| id.0 == self.sheet))
    }

    /// The scheduled plan `id` (cached until the workbook changes).
    pub(super) fn plan_view_cached(&mut self, id: TableId) -> Option<Rc<PlanView>> {
        if let Some((rev, cid, v)) = &self.plan.cache
            && *rev == self.wb.revision
            && *cid == id
        {
            return Some(v.clone());
        }
        let v = Rc::new(self.wb.plan_view(id)?);
        self.plan.cache = Some((self.wb.revision, id, v.clone()));
        Some(v)
    }

    /// The data rows of the selection within plan `id`.
    fn plan_sel_rows(&self, id: TableId) -> Vec<u32> {
        let Some(t) = self.wb.table(id) else { return Vec::new() };
        let (d0, d1) = t.data_rows();
        let s = self.sel();
        (s.start.row.max(d0)..=s.end.row.min(d1)).collect()
    }

    fn today() -> f64 {
        apex_sheet::format::now_serial().floor()
    }

    // --- Insert > Planning ------------------------------------------------------------

    /// Inserts a plan at the cursor: template `tpl` or a blank one.
    pub(super) fn insert_plan_at_cursor(&mut self, tpl: Option<usize>) {
        self.settle_edit();
        sync_field_names(&mut self.plan);
        let tasks = tpl.map(template_tasks).unwrap_or_default();
        let start = Self::today();
        match self.wb.insert_plan(self.sheet, self.cursor, start, &tasks) {
            Ok(id) => {
                self.plan.hidden = false;
                self.plan.left = None;
                if let Some(t) = self.wb.table(id) {
                    self.anchor = CellRef::new(t.range.start.row + 1, t.range.start.col + 1);
                    self.cursor = self.anchor;
                }
            }
            Err(e) => self.plan_error(e),
        }
    }

    fn plan_error(&mut self, e: TableError) {
        let key = match e {
            TableError::Overlap | TableError::Merged => "plan.err_overlap",
            _ => "plan.err_table",
        };
        self.notices.push(Notice::error(tr(key)));
    }

    /// Makes the table at the cursor a plan.
    pub(super) fn convert_table_to_plan(&mut self) {
        self.settle_edit();
        sync_field_names(&mut self.plan);
        let Some(id) = self.cursor_table() else {
            self.notices.push(Notice::info(tr("plan.err_no_table")));
            return;
        };
        if let Err(e) = self.wb.convert_to_plan(id, Self::today()) {
            self.plan_error(e);
        }
        self.plan.hidden = false;
    }

    /// The Planning group of the Insert tab.
    pub(super) fn plan_insert_group(&mut self, b: &mut Body) {
        let a = b.accent();
        b.group("toolstrip.g.plan", Icon::Calendar, |ui| {
            toolstrip::large_menu(ui, Icon::Calendar, tr("plan.menu"), tr("plan.menu_tip"), a, |ui| {
                if toolstrip::menu_item(ui, Icon::Table, tr("plan.blank")).clicked() {
                    self.insert_plan_at_cursor(None);
                    ui.close();
                }
                ui.separator();
                ui.label(RichText::new(tr("plan.templates")).weak());
                for (i, (key, _)) in TEMPLATES.iter().enumerate() {
                    if toolstrip::menu_item(ui, Icon::Template, tr(key)).clicked() {
                        self.insert_plan_at_cursor(Some(i));
                        ui.close();
                    }
                }
                ui.separator();
                let table = self.cursor_table().is_some() && self.cursor_plan().is_none();
                if ui.add_enabled_ui(table, |ui| toolstrip::menu_item(ui, Icon::FormatAsTable, tr("plan.convert"))).inner.on_hover_text(tr("plan.convert_tip")).clicked() {
                    self.convert_table_to_plan();
                    ui.close();
                }
            });
        });
    }

    // --- the Planning tab -------------------------------------------------------------

    pub(super) fn plan_tab(&mut self, b: &mut Body) {
        let a = b.accent();
        let Some(id) = self.cursor_plan() else { return };
        sync_field_names(&mut self.plan);
        let rows = self.plan_sel_rows(id);
        // The tasks picked (blank rows are none), and whether any has a
        // link: deleting, unlinking and going to a task ask for them.
        let (tasks, linked) = match self.plan_view_cached(id) {
            Some(v) => {
                let t: Vec<usize> = rows.iter().filter_map(|r| v.task_at_row(*r)).collect();
                let linked = t.iter().any(|&i| v.input.get(i).is_some_and(|x| !x.links.is_empty()));
                (t.len(), linked)
            }
            None => (0, false),
        };
        b.group("plan.g.tasks", Icon::InsertRows, |ui| {
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::InsertRows), tr("plan.task_insert"), tr("plan.task_insert_tip"), a, false).clicked() {
                    self.settle_edit();
                    self.wb.plan_insert_tasks(id, self.sel().start.row, rows.len().max(1) as u32);
                }
                if ui.add_enabled_ui(tasks > 0, |ui| toolstrip::small(ui, Some(Icon::DeleteRows), tr("plan.task_delete"), tr("plan.task_delete_tip"), a, false)).inner.clicked() {
                    self.settle_edit();
                    self.wb.plan_delete_tasks(id, &rows);
                    self.clamp_to_plan(id);
                }
            });
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::Indent), tr("plan.indent"), tr("plan.indent_tip"), a, false).clicked() {
                    self.settle_edit();
                    self.wb.plan_indent(id, &rows, 1);
                }
                if toolstrip::small(ui, Some(Icon::Outdent), tr("plan.outdent"), tr("plan.outdent_tip"), a, false).clicked() {
                    self.settle_edit();
                    self.wb.plan_indent(id, &rows, -1);
                }
            });
            // Links join two tasks or more.
            let link = ui.add_enabled_ui(rows.len() > 1, |ui| {
                toolstrip::large_split(ui, Icon::Link, tr("plan.link"), tr("plan.link_tip"), a, |ui| {
                    for (k, key) in [(DepType::FS, "plan.link_fs"), (DepType::SS, "plan.link_ss"), (DepType::FF, "plan.link_ff"), (DepType::SF, "plan.link_sf")] {
                        if ui.button(tr(key)).clicked() {
                            self.settle_edit();
                            self.wb.plan_link(id, &rows, Some(k));
                            ui.close();
                        }
                    }
                })
            });
            if link.inner {
                self.settle_edit();
                self.wb.plan_link(id, &rows, Some(DepType::FS));
            }
            toolstrip::column(ui, |ui| {
                if ui.add_enabled_ui(linked, |ui| toolstrip::small(ui, Some(Icon::Eraser), tr("plan.unlink"), tr("plan.unlink_tip"), a, false)).inner.clicked() {
                    self.settle_edit();
                    self.wb.plan_link(id, &rows, None);
                }
                if toolstrip::small(ui, Some(Icon::Pencil), tr("plan.manual"), tr("plan.manual_tip"), a, false).clicked() {
                    self.settle_edit();
                    self.wb.plan_set_mode(id, &rows, true, tr("plan.mode_manual"), tr("plan.mode_auto"));
                }
                if toolstrip::small(ui, Some(Icon::Calculate), tr("plan.auto"), tr("plan.auto_tip"), a, false).clicked() {
                    self.settle_edit();
                    self.wb.plan_set_mode(id, &rows, false, tr("plan.mode_manual"), tr("plan.mode_auto"));
                }
            });
        });
        b.group("plan.g.schedule", Icon::Calendar, |ui| {
            if toolstrip::large(ui, Icon::Calendar, tr("plan.settings"), tr("plan.settings_tip"), a, self.plan.settings.is_some()).clicked() {
                self.open_plan_settings(id);
            }
            if toolstrip::large(ui, Icon::Save, tr("plan.baseline"), tr("plan.baseline_tip"), a, false).clicked() {
                self.settle_edit();
                self.wb.plan_save_baseline(id, Self::today());
            }
            let n = self.plan_view_cached(id).map_or(0, |v| v.warnings.len() + v.bad_links.iter().filter(|b| !b.is_empty()).count());
            let label = if n > 0 { trf("plan.warnings_n", &[("n", &n)]) } else { tr("plan.warnings").to_string() };
            if toolstrip::large(ui, Icon::ErrorCheck, &label, tr("plan.warnings_tip"), a, self.plan.warnings).clicked() {
                self.plan.warnings = !self.plan.warnings;
            }
        });
        b.group("plan.g.resources", Icon::Group, |ui| {
            if toolstrip::large(ui, Icon::Group, tr("plan.resource_sheet"), tr("plan.resource_sheet_tip"), a, false).clicked() {
                self.open_resource_sheet(id);
            }
            if toolstrip::large(ui, Icon::Scenario, tr("plan.level"), tr("plan.level_tip"), a, false).clicked() {
                self.settle_edit();
                if !self.wb.plan_level(id) {
                    self.notices.push(Notice::info(tr("plan.level_none")));
                }
            }
            toolstrip::column(ui, |ui| {
                // Only delays that leveling put in can be cleared.
                let leveled = self.plan_view_cached(id).is_some_and(|v| v.col(Field::LevelDelay).is_some_and(|c| v.rows.iter().any(|&r| !self.wb.sheets[id.0].value(CellRef::new(r, c)).is_empty())));
                let r = ui.add_enabled_ui(leveled, |ui| toolstrip::small(ui, Some(Icon::Undo), tr("plan.clear_level"), tr("plan.clear_level_tip"), a, false)).inner;
                if r.clicked() {
                    self.settle_edit();
                    self.wb.plan_clear_leveling(id);
                }
                if toolstrip::small(ui, Some(Icon::ChartColumn), tr("plan.load"), tr("plan.load_tip"), a, self.plan.load).clicked() {
                    self.plan.load = !self.plan.load;
                    self.plan.hidden = false;
                }
            });
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::Function), tr("plan.custom"), tr("plan.custom_tip"), a, self.plan.custom.is_some()).clicked() {
                    self.open_custom_fields(id);
                }
                toolstrip::small_menu(ui, Some(Icon::InsertCols), tr("plan.columns"), tr("plan.columns_tip"), a, |ui| {
                    let have = self.wb.plan_fields(id);
                    let mut add = None;
                    for f in Field::ALL.iter().filter(|f| !f.is_resource() && !f.is_earned() && **f != Field::Id) {
                        if ui.add_enabled(!have.contains(f), egui::Button::new(apex_sheet::workbook::field_name(*f))).clicked() {
                            add = Some(vec![*f]);
                        }
                    }
                    ui.separator();
                    crate::widgets::menu_button(ui, tr("plan.ev_columns"), |ui| {
                        let earned: Vec<Field> = Field::ALL.into_iter().filter(|f| f.is_earned()).collect();
                        if ui.add_enabled(earned.iter().any(|f| !have.contains(f)), egui::Button::new(tr("plan.ev_all"))).clicked() {
                            add = Some(earned.clone());
                        }
                        ui.separator();
                        for f in earned {
                            if ui.add_enabled(!have.contains(&f), egui::Button::new(apex_sheet::workbook::field_name(f))).clicked() {
                                add = Some(vec![f]);
                            }
                        }
                    });
                    if let Some(list) = add {
                        self.settle_edit();
                        self.wb.plan_add_fields(id, &list);
                        ui.close();
                    }
                });
            });
        });
        b.group("plan.g.gantt", Icon::ChartBar, |ui| {
            if toolstrip::large(ui, Icon::ChartBar, tr("plan.gantt"), tr("plan.gantt_tip"), a, !self.plan.hidden).clicked() {
                self.plan.hidden = !self.plan.hidden;
            }
            toolstrip::large_menu(ui, Icon::ZoomIn, tr("plan.timescale"), tr("plan.timescale_tip"), a, |ui| {
                for s in Scale::ALL {
                    if ui.radio(self.plan.scale == s, tr(s.key())).clicked() {
                        self.plan.scale = s;
                        ui.close();
                    }
                }
            });
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::DateTime), tr("plan.today"), tr("plan.today_tip"), a, false).clicked() {
                    self.plan.left = Some(Self::today() - 7.0 * self.plan.scale.days_per_unit());
                    self.plan.hidden = false;
                }
                if ui.add_enabled_ui(tasks > 0, |ui| toolstrip::small(ui, Some(Icon::Search), tr("plan.goto_task"), tr("plan.goto_task_tip"), a, false)).inner.clicked() {
                    self.gantt_scroll_to_task(id);
                }
            });
        });
        b.group("plan.g.views", Icon::Window, |ui| {
            let now = self.plan.views.kind;
            toolstrip::large_menu(ui, now.icon(), tr("plan.views"), tr("plan.views_tip"), a, |ui| {
                for k in super::plan_views::ViewKind::ALL {
                    if toolstrip::menu_item_on(ui, k.icon(), tr(k.key()), "", now == k).clicked() {
                        self.plan.views.kind = k;
                        self.plan.hidden = false;
                        ui.close();
                    }
                }
            });
        });
        b.group("plan.g.reports", Icon::Chart, |ui| {
            toolstrip::large_menu(ui, Icon::Chart, tr("plan.reports"), tr("plan.reports_tip"), a, |ui| {
                for k in PlanReport::ALL {
                    let icon = if k == PlanReport::EarnedValue { Icon::ChartLine } else { Icon::Sheet };
                    if toolstrip::menu_item(ui, icon, tr(k.key())).clicked() {
                        self.make_plan_report(id, k);
                        ui.close();
                    }
                }
            });
        });
        b.group("plan.g.export", Icon::Export, |ui| {
            if toolstrip::large(ui, Icon::Print, tr("plan.print"), tr("plan.print_tip"), a, self.plan.print.is_some()).clicked() {
                self.open_gantt_print(id);
            }
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::Picture), tr("plan.export_png"), tr("plan.export_png_tip"), a, false).clicked() {
                    self.export_gantt(id, false);
                }
                if toolstrip::small(ui, Some(Icon::Pdf), tr("plan.export_pdf"), tr("plan.export_pdf_tip"), a, false).clicked() {
                    self.export_gantt(id, true);
                }
            });
        });
    }

    /// Planning > Reports: the report on a new sheet, which is shown.
    pub(super) fn make_plan_report(&mut self, id: TableId, kind: PlanReport) {
        self.settle_edit();
        sync_field_names(&mut self.plan);
        let t = |k: &str| apex_i18n::lookup(k).unwrap_or(k).to_string();
        if let Some(s) = self.wb.plan_report(id, kind, &t) {
            self.select_sheet(s);
            self.cursor = CellRef::new(0, 0);
            self.anchor = self.cursor;
        }
    }

    /// Keeps the cursor in the plan after deleting rows.
    fn clamp_to_plan(&mut self, id: TableId) {
        if let Some(t) = self.wb.table(id) {
            let (_, d1) = t.data_rows();
            if self.cursor.row > d1 {
                self.cursor.row = d1;
            }
            self.anchor = self.cursor;
        }
    }

    fn open_resource_sheet(&mut self, id: TableId) {
        self.settle_edit();
        let existing = self.wb.table(id).and_then(|t| t.plan.as_ref()).map(|p| p.resources.clone()).unwrap_or_default();
        if let Some(rid) = (!existing.is_empty()).then(|| self.wb.find_table(&existing)).flatten() {
            self.select_sheet(rid.0);
            if let Some(t) = self.wb.table(rid) {
                self.cursor = CellRef::new(t.range.start.row + 1, t.range.start.col);
                self.anchor = self.cursor;
                self.ensure_visible = true;
            }
            return;
        }
        if let Some(rid) = self.wb.plan_resource_sheet(id, tr("plan.resource_sheet_name")) {
            self.select_sheet(rid.0);
        }
    }

    // --- Project Settings ---------------------------------------------------------------

    fn open_plan_settings(&mut self, id: TableId) {
        self.settle_edit();
        let Some(plan) = self.wb.table(id).and_then(|t| t.plan.as_deref()).cloned() else { return };
        let start = date_text(&self.wb, plan.start);
        let status = plan.status.map(|s| date_text(&self.wb, s)).unwrap_or_default();
        let year = apex_sheet::format::serial_to_date(plan.start).0.to_string();
        let mut d = SettingsDlg { id, plan, start, status, cal: 0, hours: String::new(), new_cal: String::new(), country: 0, year, exc_name: String::new(), exc_from: String::new(), exc_to: String::new(), exc_working: false };
        d.hours = hours_text(&d.plan.calendars[0]);
        self.plan.settings = Some(Box::new(d));
    }

    pub(super) fn plan_dialogs(&mut self, ctx: &egui::Context, shell: &mut Shell) {
        if let Some(mut d) = self.plan.settings.take() {
            match self.plan_settings_dialog(ctx, &mut d) {
                DialogResult::None => self.plan.settings = Some(d),
                DialogResult::Cancel => {}
                DialogResult::Ok => {
                    if let Err(msg) = d.apply_texts(&self.wb) {
                        self.notices.push(Notice::error(msg));
                        self.plan.settings = Some(d);
                    } else {
                        self.wb.set_plan(d.id, d.plan.clone());
                    }
                }
            }
        }
        self.custom_fields_dialog(ctx);
        self.gantt_print_dialog(ctx, shell);
        if self.plan.warnings {
            self.plan_warnings_window(ctx);
        }
    }

    fn plan_warnings_window(&mut self, ctx: &egui::Context) {
        let Some(id) = self.shown_plan() else {
            self.plan.warnings = false;
            return;
        };
        let Some(v) = self.plan_view_cached(id) else { return };
        let mut open = true;
        let mut go = None;
        egui::Window::new(tr("plan.warnings")).id(Id::new(("plan_warnings", self.uid))).open(&mut open).collapsible(false).default_width(420.0).show_fit(ctx, |ui| {
            let mut any = false;
            for (i, bad) in v.bad_links.iter().enumerate() {
                for b in bad {
                    any = true;
                    if ui.link(trf("plan.warn_bad_text", &[("task", &v.ids[i]), ("text", b)])).clicked() {
                        go = Some(i);
                    }
                }
            }
            for w in &v.warnings {
                any = true;
                let task = match w {
                    Warning::Cycle(l) => l.first().copied(),
                    Warning::BadLink { task, .. } | Warning::SummaryLink { task, .. } | Warning::LinkConflict { task, .. } | Warning::ConstraintMissed { task } => Some(*task),
                };
                if ui.link(warning_text(w)).clicked() {
                    go = task.and_then(|p| v.task_of_id(p + 1));
                }
            }
            if !any {
                ui.label(tr("plan.no_warnings"));
            }
        });
        if let Some(i) = go.and_then(|i| v.rows.get(i)) {
            self.cursor = CellRef::new(*i, v.col(Field::Name).unwrap_or(self.cursor.col));
            self.anchor = self.cursor;
            self.ensure_visible = true;
        }
        self.plan.warnings = open;
    }

    fn plan_settings_dialog(&mut self, ctx: &egui::Context, d: &mut SettingsDlg) -> DialogResult {
        let wb = &self.wb;
        super::f10::tool_window(ctx, "plan_settings", tr("plan.settings"), 520.0, false, |ui| {
            egui::Grid::new("plan_settings_grid").num_columns(2).spacing(vec2(12.0, 6.0)).show(ui, |ui| {
                ui.label(tr("plan.start"));
                ui.add(egui::TextEdit::singleline(&mut d.start).desired_width(140.0));
                ui.end_row();
                ui.label(tr("plan.status"));
                ui.add(egui::TextEdit::singleline(&mut d.status).desired_width(140.0).hint_text(tr("plan.status_today")));
                ui.end_row();
            });
            ui.separator();
            // The calendar being edited.
            if d.apply_hours().is_ok() {
                d.hours = hours_text(&d.plan.calendars[d.cal]);
            }
            ui.horizontal(|ui| {
                ui.label(tr("plan.calendar"));
                let names: Vec<String> = d.plan.calendars.iter().enumerate().map(|(i, c)| if i == 0 { format!("{} ({})", c.name, tr("plan.project_calendar")) } else { c.name.clone() }).collect();
                let before = d.cal;
                egui::ComboBox::from_id_salt("plan_cal").selected_text(names[d.cal].clone()).show_ui(ui, |ui| {
                    for (i, n) in names.iter().enumerate() {
                        ui.selectable_value(&mut d.cal, i, n);
                    }
                });
                if before != d.cal {
                    d.hours = hours_text(&d.plan.calendars[d.cal]);
                }
                ui.add(egui::TextEdit::singleline(&mut d.new_cal).desired_width(110.0).hint_text(tr("plan.new_calendar")));
                if ui.add_enabled(!d.new_cal.trim().is_empty(), egui::Button::new(tr("plan.add"))).clicked() {
                    let mut c = Calendar::standard(d.new_cal.trim());
                    c.inherit = true;
                    d.plan.calendars.push(c);
                    d.cal = d.plan.calendars.len() - 1;
                    d.hours = hours_text(&d.plan.calendars[d.cal]);
                    d.new_cal.clear();
                }
            });
            let cal = d.cal;
            let days = crate::i18n::tr("field.days").split('|').map(str::to_string).collect::<Vec<_>>();
            ui.horizontal_wrapped(|ui| {
                ui.label(tr("plan.workdays"));
                for i in 0..7 {
                    // field.days starts on Sunday; weekdays on Monday.
                    let name = days.get((i + 1) % 7).cloned().unwrap_or_default();
                    ui.checkbox(&mut d.plan.calendars[cal].weekdays[i], name);
                }
            });
            ui.horizontal(|ui| {
                ui.label(tr("plan.hours"));
                ui.add(egui::TextEdit::singleline(&mut d.hours).desired_width(180.0).hint_text("8-12, 13-17"));
                if cal > 0 {
                    ui.checkbox(&mut d.plan.calendars[cal].inherit, tr("plan.inherit"));
                }
            });
            ui.separator();
            ui.label(RichText::new(tr("plan.holidays")).strong());
            let mut remove = None;
            egui::ScrollArea::vertical().max_height(160.0).id_salt("plan_exc").show(ui, |ui| {
                egui::Grid::new("plan_exc_grid").num_columns(4).striped(true).show(ui, |ui| {
                    for (i, e) in d.plan.calendars[cal].exceptions.iter().enumerate() {
                        ui.label(&e.name);
                        let range = if e.to > e.from { format!("{} – {}", date_text(wb, e.from as f64), date_text(wb, e.to as f64)) } else { date_text(wb, e.from as f64) };
                        ui.label(range);
                        ui.label(if e.working { tr("plan.working") } else { tr("plan.nonworking") });
                        if ui.small_button("×").on_hover_text(tr("plan.remove")).clicked() {
                            remove = Some(i);
                        }
                        ui.end_row();
                    }
                });
            });
            if let Some(i) = remove {
                d.plan.calendars[cal].exceptions.remove(i);
            }
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut d.exc_name).desired_width(120.0).hint_text(tr("plan.exception_name")));
                ui.add(egui::TextEdit::singleline(&mut d.exc_from).desired_width(80.0).hint_text(tr("plan.from")));
                ui.add(egui::TextEdit::singleline(&mut d.exc_to).desired_width(80.0).hint_text(tr("plan.to")));
                ui.checkbox(&mut d.exc_working, tr("plan.working"));
                // Off until a date is typed in From.
                let from = parse_num(wb, &d.exc_from).filter(|v| *v > 0.0);
                if ui.add_enabled(from.is_some(), egui::Button::new(tr("plan.add"))).clicked() {
                    let to = parse_num(wb, &d.exc_to).filter(|v| *v > 0.0);
                    if let Some(from) = from {
                        let to = to.unwrap_or(from).max(from);
                        let name = if d.exc_name.trim().is_empty() { tr("plan.exception").to_string() } else { d.exc_name.trim().to_string() };
                        d.plan.calendars[cal].exceptions.push(Exception { name, from: from as i64, to: to as i64, working: d.exc_working });
                        d.plan.calendars[cal].exceptions.sort_by_key(|e| e.from);
                        d.exc_name.clear();
                        d.exc_from.clear();
                        d.exc_to.clear();
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label(tr("plan.add_holidays"));
                egui::ComboBox::from_id_salt("plan_country").selected_text(holidays::COUNTRIES[d.country].1).show_ui(ui, |ui| {
                    for (i, (_, n)) in holidays::COUNTRIES.iter().enumerate() {
                        ui.selectable_value(&mut d.country, i, *n);
                    }
                });
                ui.label(tr("plan.year"));
                ui.add(egui::TextEdit::singleline(&mut d.year).desired_width(50.0));
                if ui.button(tr("plan.add")).clicked()
                    && let Ok(y) = d.year.trim().parse::<i64>()
                    && (1900..=2200).contains(&y)
                {
                    let list = holidays::holidays(holidays::COUNTRIES[d.country].0, y);
                    d.plan.calendars[cal].add_holidays(&list);
                }
            });
        })
    }
}

/// Planning > Project Settings.
pub(super) struct SettingsDlg {
    id: TableId,
    plan: apex_sheet::plan::Plan,
    start: String,
    status: String,
    /// The calendar being edited.
    cal: usize,
    hours: String,
    new_cal: String,
    country: usize,
    year: String,
    exc_name: String,
    exc_from: String,
    exc_to: String,
    exc_working: bool,
}

impl SettingsDlg {
    /// The working hours typed, into the calendar being edited.
    fn apply_hours(&mut self) -> Result<(), &'static str> {
        let h = parse_hours(&self.hours).ok_or(tr("plan.err_hours"))?;
        self.plan.calendars[self.cal].hours = h;
        Ok(())
    }

    fn apply_texts(&mut self, wb: &Workbook) -> Result<(), &'static str> {
        self.apply_hours()?;
        self.plan.start = parse_num(wb, &self.start).filter(|v| *v > 0.0).ok_or(tr("plan.err_date"))?.floor();
        self.plan.status = if self.status.trim().is_empty() { None } else { Some(parse_num(wb, &self.status).filter(|v| *v > 0.0).ok_or(tr("plan.err_date"))?.floor()) };
        Ok(())
    }
}

/// "8-12, 13-17" (hours may be 8:30 or 8.5).
pub(super) fn parse_hours(s: &str) -> Option<Vec<(f64, f64)>> {
    let hour = |t: &str| -> Option<f64> {
        let t = t.trim();
        let v = match t.split_once(':') {
            Some((h, m)) => h.trim().parse::<f64>().ok()? + m.trim().parse::<f64>().ok()? / 60.0,
            None => t.replace(',', ".").parse::<f64>().ok()?,
        };
        (0.0..=24.0).contains(&v).then_some(v)
    };
    let mut out = Vec::new();
    for part in s.split([';', ',']).map(str::trim).filter(|p| !p.is_empty()) {
        let (a, b) = part.split_once('-')?;
        let (a, b) = (hour(a)?, hour(b)?);
        if b <= a || out.last().is_some_and(|l: &(f64, f64)| a < l.1) {
            return None;
        }
        out.push((a, b));
    }
    (!out.is_empty()).then_some(out)
}

pub(super) fn hours_text(c: &Calendar) -> String {
    let h = |v: f64| if v.fract().abs() < 1e-9 { format!("{v}") } else { format!("{}:{:02}", v.trunc(), (v.fract() * 60.0).round()) };
    c.hours.iter().map(|(a, b)| format!("{}-{}", h(*a), h(*b))).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hours() {
        assert_eq!(parse_hours("8-12, 13-17"), Some(vec![(8.0, 12.0), (13.0, 17.0)]));
        assert_eq!(parse_hours("8:30-14"), Some(vec![(8.5, 14.0)]));
        assert_eq!(parse_hours("12-8"), None);
        assert_eq!(parse_hours("8-12, 11-17"), None);
        assert_eq!(parse_hours(""), None);
        let mut c = Calendar::standard("x");
        c.hours = vec![(8.5, 13.0), (14.0, 17.5)];
        assert_eq!(hours_text(&c), "8:30-13, 14-17:30");
        assert_eq!(parse_hours(&hours_text(&c)), Some(c.hours.clone()));
    }

    /// Templates give plans with links that schedule without warnings.
    #[test]
    fn templates_schedule() {
        for i in 0..TEMPLATES.len() {
            let mut wb = Workbook::new();
            let tasks = template_tasks(i);
            let id = wb.insert_plan(0, CellRef::new(0, 0), apex_sheet::format::date_to_serial(2026, 10, 5), &tasks).unwrap();
            let v = wb.plan_view(id).unwrap();
            assert_eq!(v.rows.len(), tasks.len());
            assert!(v.warnings.is_empty(), "{i}: {:?}", v.warnings);
            assert!(v.bad_links.iter().all(|b| b.is_empty()));
            assert!(v.schedule.tasks.iter().any(|t| t.summary));
            assert!(v.schedule.tasks.iter().any(|t| t.critical));
        }
    }

    /// A sheet with a plan draws its Planning tab and the Gantt beside the
    /// grid, lined up with the rows, and exports it as PNG and PDF.
    #[test]
    fn gantt_frames_and_exports() {
        let mut wb = Workbook::new();
        let id = wb.insert_plan(0, CellRef::new(0, 0), apex_sheet::format::date_to_serial(2026, 10, 5), &template_tasks(0)).unwrap();
        let mut v = SheetView::new(wb);
        v.select(CellRef::new(2, 1), false);
        assert!(v.cursor_plan().is_some());
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let mut shell = crate::common::Shell { cmd: None, show_formula_bar: true, file_name: "plan".into(), fullscreen: false, dark: None, others: Vec::new(), books: Vec::new(), side: None, recording: None };
        for _ in 0..3 {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                egui::Panel::top("toolstrip").show(ui, |ui| v.toolstrip(ui, "plan", &mut shell));
                v.ui(ui, &mut shell);
            });
            out.textures_delta.clear();
        }
        assert!(v.plan.rows.is_some());
        let png = v.gantt_png(&ctx, id, 1.0).unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        let pdf = v.gantt_pdf(&ctx, id).unwrap();
        assert!(pdf.starts_with(b"%PDF"));
    }

    /// Planning is found by the command search (Alt+Q) and in the classic
    /// menus: Insert > Planning with its templates, and the Planning tab's
    /// new commands.
    #[test]
    fn planning_in_search_and_classic_menus() {
        let mut wb = Workbook::new();
        wb.insert_plan(0, CellRef::new(0, 0), apex_sheet::format::date_to_serial(2026, 10, 5), &template_tasks(0)).unwrap();
        let mut v = SheetView::new(wb);
        v.select(CellRef::new(2, 1), false);
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let mut shell = crate::common::Shell { cmd: None, show_formula_bar: true, file_name: "plan".into(), fullscreen: false, dark: None, others: Vec::new(), books: Vec::new(), side: None, recording: None };
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))), ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| {
            for style in [toolstrip::Style::Toolstrip, toolstrip::Style::Menu] {
                let mut labels = |tab: &'static str| {
                    toolstrip::offscreen_ui(ui, |ui| toolstrip::with_style(style, || toolstrip::collect(|| v.toolstrip(ui, tab, &mut shell)))).into_iter().map(|e| e.label).collect::<Vec<_>>()
                };
                let insert = labels("insert");
                for k in ["plan.menu", "plan.blank", "plan.tpl_construction", "plan.tpl_event", "plan.tpl_software", "plan.convert"] {
                    assert!(insert.iter().any(|l| l == tr(k)), "{style:?} {k}");
                }
                let plan = labels("plan");
                for k in ["plan.custom", "plan.reports", "plan.rep.earned", "plan.gantt"] {
                    assert!(plan.iter().any(|l| l == tr(k)), "{style:?} {k}: {plan:?}");
                }
            }
        });
        out.textures_delta.clear();
    }
}
