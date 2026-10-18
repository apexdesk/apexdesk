//! Insert > PivotTable, the PivotTable Fields list (drag fields between
//! Filters, Columns, Rows and Values), value field settings, grouping,
//! layout and styles.

use std::sync::Arc;

use apex_sheet::analysis::Lang;
use apex_sheet::analysis::pivot::{self, AggFunc, BaseField, BaseItem, CALC_BASE, CalcField, DatePart, Field, FieldFilter, Grouping, LabelFilter, LabelOp, Layout, PivotCellKind, PivotSpec, ShowAs, ValueField, ValueFilter, ValueOp};

use super::f10::{self, F10Dlg, ref_edit, tool_window};
use super::*;
use crate::widgets::DialogResult;

#[derive(Default)]
pub(super) struct PivotPanel {
    /// The field list of this pivot was closed (until the cursor leaves it).
    pub closed: Option<usize>,
    search: String,
    pub slicer: super::slicer_ui::SlicerState,
}

pub(super) enum PivotDlg {
    /// `model`: on the workbook's data model instead of a range.
    Create { range: String, new_sheet: bool, dest: String, model: bool },
    Values(Box<ValueDlg>),
    Group(Box<GroupDlg>),
    Calc(Box<CalcDlg>),
    Filter(Box<FilterDlg>),
    Slicers { index: usize, checks: Vec<bool> },
}

pub(super) struct CalcDlg {
    index: usize,
    /// The calculated field being edited (None: a new one).
    current: Option<usize>,
    name: String,
    formula: String,
}

/// Label or value filter of a row or column field.
pub(super) struct FilterDlg {
    index: usize,
    cols: bool,
    fi: usize,
    by_value: bool,
    lop: LabelOp,
    vop: ValueOp,
    value: usize,
    a: String,
    b: String,
}

const LABEL_OPS: [&str; 14] = [
    "pivot.op_equals",
    "pivot.op_not_equals",
    "pivot.op_begins",
    "pivot.op_not_begins",
    "pivot.op_ends",
    "pivot.op_not_ends",
    "pivot.op_contains",
    "pivot.op_not_contains",
    "pivot.op_greater",
    "pivot.op_greater_eq",
    "pivot.op_less",
    "pivot.op_less_eq",
    "pivot.op_between",
    "pivot.op_not_between",
];

const VALUE_OPS: [&str; 14] = [
    "pivot.op_equals",
    "pivot.op_not_equals",
    "pivot.op_greater",
    "pivot.op_greater_eq",
    "pivot.op_less",
    "pivot.op_less_eq",
    "pivot.op_between",
    "pivot.op_not_between",
    "pivot.op_top_items",
    "pivot.op_bottom_items",
    "pivot.op_top_percent",
    "pivot.op_bottom_percent",
    "pivot.op_top_sum",
    "pivot.op_bottom_sum",
];

pub(super) struct ValueDlg {
    index: usize,
    vi: usize,
    name: String,
    func: AggFunc,
    show: u8,
    base: usize,
    item: u8,
    item_name: String,
    fmt: String,
}

pub(super) struct GroupDlg {
    index: usize,
    /// Rows (false) or columns (true), and the field's position.
    cols: bool,
    fi: usize,
    /// 0 none, 1 dates, 2 numbers.
    kind: u8,
    part: DatePart,
    start: String,
    end: String,
    by: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Area {
    List,
    Filters,
    Cols,
    Rows,
    Values,
}

/// What is being dragged in the field list.
#[derive(Clone, Copy, Debug)]
struct PDrag {
    from: Area,
    index: usize,
    col: u32,
}

const FUNCS: [(AggFunc, &str); 12] = [
    (AggFunc::Sum, "pivot.fn_sum"),
    (AggFunc::Count, "pivot.fn_count"),
    (AggFunc::Average, "pivot.fn_average"),
    (AggFunc::Max, "pivot.fn_max"),
    (AggFunc::Min, "pivot.fn_min"),
    (AggFunc::Product, "pivot.fn_product"),
    (AggFunc::CountNums, "pivot.fn_count_nums"),
    (AggFunc::StdDev, "pivot.fn_stddev"),
    (AggFunc::StdDevP, "pivot.fn_stddevp"),
    (AggFunc::Var, "pivot.fn_var"),
    (AggFunc::VarP, "pivot.fn_varp"),
    (AggFunc::DistinctCount, "pivot.fn_distinct"),
];

const SHOW_AS: [&str; 14] = [
    "pivot.show_normal",
    "pivot.show_grand",
    "pivot.show_col",
    "pivot.show_row",
    "pivot.show_parent",
    "pivot.show_diff",
    "pivot.show_running",
    "pivot.show_rank_asc",
    "pivot.show_rank_desc",
    "pivot.show_pct_of",
    "pivot.show_pct_diff",
    "pivot.show_pct_running",
    "pivot.show_parent_col",
    "pivot.show_index",
];

/// "Show values as" choices that need a base field, and a base item.
fn needs_base(show: u8) -> bool {
    (5..=11).contains(&show)
}

fn needs_item(show: u8) -> bool {
    matches!(show, 5 | 9 | 10)
}

/// The caption of a source column or calculated field.
fn col_name(spec: &PivotSpec, names: &[String], col: u32) -> String {
    if let Some(k) = col.checked_sub(pivot::MEASURE_BASE) {
        return spec.model.as_ref().and_then(|m| m.measures.get(k as usize)).cloned().unwrap_or_default();
    }
    match col.checked_sub(CALC_BASE) {
        Some(k) => spec.calculated.get(k as usize).map_or_else(String::new, |c| c.name.clone()),
        None => names.get(col as usize).cloned().unwrap_or_default(),
    }
}

const FORMATS: [&str; 7] = ["General", "0", "#,##0", "#,##0.00", "0%", "0.00%", "#,##0.00 \"€\""];

fn func_label(f: AggFunc) -> &'static str {
    FUNCS.iter().find(|x| x.0 == f).map_or("", |x| tr(x.1))
}

fn lang() -> Lang {
    Lang::from_tag(crate::i18n::current())
}

// --- spec editing ---------------------------------------------------------------------------

/// Whether the source column is used anywhere in the table.
fn uses(spec: &PivotSpec, col: u32) -> bool {
    spec.rows.iter().chain(&spec.cols).any(|f| f.col == col) || spec.filters.iter().any(|f| f.field.col == col) || spec.values.iter().any(|v| v.col == col)
}

/// Takes item `i` out of an area; returns its field settings (not for values).
fn take(spec: &mut PivotSpec, area: Area, i: usize) -> Option<Field> {
    match area {
        Area::Rows if i < spec.rows.len() => Some(spec.rows.remove(i)),
        Area::Cols if i < spec.cols.len() => Some(spec.cols.remove(i)),
        Area::Filters if i < spec.filters.len() => Some(spec.filters.remove(i).field),
        Area::Values if i < spec.values.len() => {
            spec.values.remove(i);
            None
        }
        _ => None,
    }
}

/// Puts a field into an area at `at` (clamped).
fn put(spec: &mut PivotSpec, area: Area, at: usize, col: u32, field: Option<Field>) {
    let f = field.filter(|f| f.col == col).unwrap_or_else(|| Field::new(col));
    match area {
        Area::Rows => {
            if spec.rows.iter().all(|x| x.col != col) {
                spec.rows.insert(at.min(spec.rows.len()), f);
            }
        }
        Area::Cols => {
            if spec.cols.iter().all(|x| x.col != col) {
                spec.cols.insert(at.min(spec.cols.len()), f);
            }
        }
        Area::Filters => {
            if spec.filters.iter().all(|x| x.field.col != col) {
                spec.filters.insert(at.min(spec.filters.len()), FieldFilter { field: Field { group: Grouping::None, ..f }, items: Vec::new() });
            }
        }
        Area::Values => spec.values.insert(at.min(spec.values.len()), ValueField { col, ..Default::default() }),
        Area::List => {}
    }
}

/// Moves item `i` of `from` to the end of `to` (a field sits in one of
/// rows, columns and filters at a time).
fn move_to(spec: &mut PivotSpec, from: Area, i: usize, col: u32, to: Area) {
    // Calculated fields only go to values.
    if col >= CALC_BASE && to != Area::Values {
        return;
    }
    let field = if from == Area::List { None } else { take(spec, from, i) };
    if to != Area::Values {
        for a in [Area::Rows, Area::Cols, Area::Filters] {
            if a != to {
                let pos = match a {
                    Area::Rows => spec.rows.iter().position(|f| f.col == col),
                    Area::Cols => spec.cols.iter().position(|f| f.col == col),
                    _ => spec.filters.iter().position(|f| f.field.col == col),
                };
                if let Some(p) = pos {
                    take(spec, a, p);
                }
            }
        }
    }
    let at = match to {
        Area::Rows => spec.rows.len(),
        Area::Cols => spec.cols.len(),
        Area::Filters => spec.filters.len(),
        Area::Values => spec.values.len(),
        Area::List => 0,
    };
    put(spec, to, at, col, field);
}

/// Moves item `i` of an area up or down.
fn shift(spec: &mut PivotSpec, area: Area, i: usize, up: bool) {
    fn sw<T>(v: &mut [T], i: usize, up: bool) {
        let j = if up { i.checked_sub(1) } else { Some(i + 1) };
        if let Some(j) = j.filter(|j| *j < v.len() && i < v.len()) {
            v.swap(i, j);
        }
    }
    match area {
        Area::Rows => sw(&mut spec.rows, i, up),
        Area::Cols => sw(&mut spec.cols, i, up),
        Area::Filters => sw(&mut spec.filters, i, up),
        Area::Values => sw(&mut spec.values, i, up),
        Area::List => {}
    }
}

/// A field of the list: check to add (numbers and measures to values,
/// the rest to rows), drag into an area.
fn field_row(ui: &mut egui::Ui, spec: &mut PivotSpec, changed: &mut bool, col: u32, label: RichText, to_values: bool) {
    let mut on = uses(spec, col);
    ui.horizontal(|ui| {
        if ui.checkbox(&mut on, "").changed() {
            if on {
                move_to(spec, Area::List, 0, col, if to_values { Area::Values } else { Area::Rows });
            } else {
                spec.rows.retain(|f| f.col != col);
                spec.cols.retain(|f| f.col != col);
                spec.filters.retain(|f| f.field.col != col);
                spec.values.retain(|v| v.col != col);
            }
            *changed = true;
        }
        ui.dnd_drag_source(Id::new(("pivot_field", col)), PDrag { from: Area::List, index: 0, col }, |ui| {
            ui.label(label);
        });
    });
}

/// The field list of a pivot table on the data model: every table with
/// its columns and measures (`spec.model` already names them all).
fn model_field_list(ui: &mut egui::Ui, spec: &mut PivotSpec, changed: &mut bool, tables: &[(String, Vec<String>)], measures: &[(String, String)], numeric: &[bool], q: &str) {
    let Some(ms) = spec.model.clone() else { return };
    let hit = |s: &str| q.is_empty() || s.to_lowercase().contains(q);
    let measure_rows = |ui: &mut egui::Ui, spec: &mut PivotSpec, changed: &mut bool, home: Option<&str>| {
        for (m, t) in measures {
            let here = match home {
                Some(h) => t.eq_ignore_ascii_case(h),
                None => !tables.iter().any(|(n, _)| n.eq_ignore_ascii_case(t)),
            };
            if here && hit(m) {
                let col = ms.measures.iter().position(|x| x.eq_ignore_ascii_case(m)).map_or(0, |k| pivot::MEASURE_BASE + k as u32);
                field_row(ui, spec, changed, col, RichText::new(format!("ƒx {m}")).italics(), true);
            }
        }
    };
    for (t, cols) in tables {
        let any = hit(t) || cols.iter().any(|c| hit(c)) || measures.iter().any(|(m, h)| h.eq_ignore_ascii_case(t) && hit(m));
        if !any {
            continue;
        }
        let used = cols.iter().any(|c| ms.cols.iter().position(|(a, b)| a.eq_ignore_ascii_case(t) && b.eq_ignore_ascii_case(c)).is_some_and(|k| uses(spec, k as u32)));
        let head = RichText::new(t).strong();
        egui::CollapsingHeader::new(if used { head.color(theme::SHEET_ACCENT) } else { head }).id_salt(("pivot_model_table", t)).default_open(tables.len() <= 3 || !q.is_empty()).show(ui, |ui| {
            for c in cols {
                if !hit(c) && !hit(t) {
                    continue;
                }
                let Some(k) = ms.cols.iter().position(|(a, b)| a.eq_ignore_ascii_case(t) && b.eq_ignore_ascii_case(c)) else { continue };
                field_row(ui, spec, changed, k as u32, RichText::new(c), numeric.get(k).copied().unwrap_or(false));
            }
            measure_rows(ui, spec, changed, Some(t));
        });
    }
    if measures.iter().any(|(_, h)| !tables.iter().any(|(n, _)| n.eq_ignore_ascii_case(h))) {
        ui.label(RichText::new(tr("model.measures")).strong());
        measure_rows(ui, spec, changed, None);
    }
    if tables.is_empty() {
        ui.label(RichText::new(tr("model.need_tables")).weak());
    }
}

/// A drop target of the field list (egui's own paints its frame with the
/// widget fill, which hides check boxes).
fn drop_zone(ui: &mut egui::Ui, fill: Color32, add: impl FnOnce(&mut egui::Ui)) -> Option<Arc<PDrag>> {
    let dragging = egui::DragAndDrop::has_payload_of_type::<PDrag>(ui.ctx());
    let mut frame = egui::Frame::new().inner_margin(egui::Margin::same(6)).corner_radius(4).begin(ui);
    add(&mut frame.content_ui);
    let response = frame.allocate_space(ui);
    let hot = dragging && response.contains_pointer();
    frame.frame.fill = fill;
    frame.frame.stroke = if hot { Stroke::new(1.5, theme::SHEET_ACCENT) } else { ui.visuals().widgets.noninteractive.bg_stroke };
    frame.paint(ui);
    response.dnd_release_payload::<PDrag>()
}

/// Paints a pivot style sample.
fn paint_thumbnail(p: &egui::Painter, r: Rect, style: &str) {
    use PivotCellKind as K;
    let rows = [[K::Header, K::ColLabel, K::ColLabel], [K::SubtotalLabel, K::SubtotalValue, K::SubtotalValue], [K::RowLabel, K::Value, K::Value], [K::RowLabel, K::Value, K::Value], [K::GrandLabel, K::GrandValue, K::GrandValue]];
    p.rect_filled(r, 0.0, Color32::WHITE);
    let (w, h) = (r.width() / 3.0, r.height() / rows.len() as f32);
    for (ri, kinds) in rows.iter().enumerate() {
        for (ci, k) in kinds.iter().enumerate() {
            let l = pivot::style_look(style, *k);
            let cr = Rect::from_min_size(r.min + vec2(ci as f32 * w, ri as f32 * h), vec2(w, h));
            if let Some(f) = l.fill {
                p.rect_filled(cr, 0.0, theme::rgb(f));
            }
            let tc = l.color.map_or(Color32::from_gray(90), theme::rgb);
            p.line_segment([pos2(cr.min.x + w * 0.2, cr.center().y), pos2(cr.max.x - w * 0.2, cr.center().y)], Stroke::new(if l.bold { 1.6 } else { 1.0 }, tc));
            if let Some(c) = l.top {
                p.line_segment([cr.left_top(), cr.right_top()], Stroke::new(1.0, theme::rgb(c)));
            }
            if let Some(c) = l.bottom {
                p.line_segment([cr.left_bottom(), cr.right_bottom()], Stroke::new(1.0, theme::rgb(c)));
            }
        }
    }
    p.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_gray(200)), StrokeKind::Inside);
}

/// The pivot style gallery; returns the style picked.
fn style_gallery(ui: &mut egui::Ui, current: &str) -> Option<String> {
    let mut out = None;
    egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
        for (fi, (prefix, n)) in pivot::STYLE_FAMILIES.iter().enumerate() {
            ui.label(RichText::new(tr(["xtab.light", "xtab.medium", "xtab.dark"][fi])).strong());
            egui::Grid::new(("pivot_gallery", fi)).spacing(vec2(4.0, 4.0)).show(ui, |ui| {
                for k in 1..=*n {
                    let name = format!("{prefix}{k}");
                    let (r, resp) = ui.allocate_exact_size(vec2(44.0, 34.0), Sense::click());
                    crate::widgets::name(&resp, egui::WidgetType::SelectableLabel, &name, Some(name == current));
                    paint_thumbnail(ui.painter(), r.shrink(2.0), &name);
                    if name == current || resp.hovered() {
                        ui.painter().rect_stroke(r, 2.0, Stroke::new(2.0, theme::SHEET_ACCENT), StrokeKind::Inside);
                    }
                    let caption = match fi {
                        0 => trf("pivot.style_light", &[("n", &k)]),
                        1 => trf("pivot.style_medium", &[("n", &k)]),
                        _ => trf("pivot.style_dark", &[("n", &k)]),
                    };
                    if resp.on_hover_text(caption).clicked() {
                        out = Some(name);
                    }
                    if k % 7 == 0 {
                        ui.end_row();
                    }
                }
            });
        }
    });
    out
}

/// Layout options of a spec; true when changed.
fn layout_menu(ui: &mut egui::Ui, spec: &mut PivotSpec) -> bool {
    let mut changed = false;
    ui.label(RichText::new(tr("pivot.report_layout")).weak());
    for (l, key) in [(Layout::Compact, "pivot.layout_compact"), (Layout::Outline, "pivot.layout_outline"), (Layout::Tabular, "pivot.layout_tabular")] {
        if ui.radio(spec.layout == l, tr(key)).clicked() {
            spec.layout = l;
            changed = true;
        }
    }
    ui.separator();
    changed |= ui.checkbox(&mut spec.subtotals, tr("pivot.subtotals")).changed();
    changed |= ui.checkbox(&mut spec.grand_rows, tr("pivot.grand_rows")).changed();
    changed |= ui.checkbox(&mut spec.grand_cols, tr("pivot.grand_cols")).changed();
    changed |= ui.checkbox(&mut spec.repeat_labels, tr("pivot.repeat_labels")).changed();
    changed
}

impl SheetView {
    /// Header texts of a source range (the field names).
    fn source_names(&self, sheet: usize, source: &Range) -> Vec<String> {
        let cols = source.cols().min(512);
        (0..cols)
            .map(|c| {
                let t = self.wb.display(sheet, CellRef::new(source.start.row, source.start.col + c)).0;
                if t.is_empty() { format!("Column{}", c + 1) } else { t }
            })
            .collect()
    }

    /// The field names of a pivot table: the header of its source range,
    /// or the model columns it can use.
    fn pivot_names(&self, spec: &PivotSpec) -> Option<Vec<String>> {
        if let Some(m) = &spec.model {
            return Some(m.cols.iter().map(|(_, c)| c.clone()).collect());
        }
        let si = self.wb.sheet_index(&spec.source_sheet)?;
        Some(self.source_names(si, &spec.source))
    }

    /// Whether a column of a model table holds numbers (its first values).
    fn model_numeric(&self, table: &str, col: &str) -> bool {
        let Some(id) = self.wb.find_table(table) else { return false };
        let Some(t) = self.wb.table(id) else { return false };
        let Some(ci) = t.column_index(col) else { return false };
        let r = t.data_range();
        let sh = &self.wb.sheets[id.0];
        (r.start.row..=r.end.row.min(r.start.row + 50)).map(|row| sh.value(CellRef::new(row, t.col_at(ci)))).find(|v| !v.is_empty()).is_some_and(|v| matches!(v, Value::Number(_)))
    }

    /// Whether a source column holds numbers (its first values).
    fn numeric_col(&self, sheet: usize, source: &Range, c: u32) -> bool {
        let sh = &self.wb.sheets[sheet];
        let col = source.start.col + c;
        (source.start.row + 1..=source.end.row.min(source.start.row + 50)).map(|r| sh.value(CellRef::new(r, col))).find(|v| !v.is_empty()).is_some_and(|v| matches!(v, Value::Number(_)))
    }

    pub(super) fn open_pivot_create(&mut self, source: Option<String>) {
        let range = match source {
            Some(s) => s,
            None => match self.cursor_table() {
                Some(id) => self.wb.table(id).map(|t| t.name.clone()).unwrap_or_default(),
                None => {
                    let sel = self.sel();
                    let r = if sel.is_single() { self.wb.current_region(self.sheet, self.cursor) } else { sel };
                    if r.is_single() && self.sh().value(r.start).is_empty() { String::new() } else { format!("{}!{}", f10::quoted_sheet(&self.sh().name), f10::abs_range(&r)) }
                }
            },
        };
        self.open_f10(F10Dlg::Pivot(PivotDlg::Create { range, new_sheet: true, dest: String::new(), model: false }), Some(0));
    }

    pub(super) fn pivot_dialog(&mut self, ctx: &egui::Context, mut d: PivotDlg) -> Option<PivotDlg> {
        let focus = self.grid_has_focus(ctx);
        match &mut d {
            PivotDlg::Create { range, new_sheet, dest, model } => {
                let mut pick = std::mem::take(&mut self.f10.pick);
                let tables = self.wb.tables().count();
                let res = tool_window(ctx, "pivot_create", tr("pivot.create_title"), 340.0, focus, |ui| {
                    ui.strong(tr("pivot.choose_data"));
                    ui.radio_value(model, false, tr("pivot.table_range"));
                    ui.add_enabled_ui(!*model, |ui| {
                        ref_edit(ui, range, 0, &mut pick, 320.0);
                    });
                    ui.add_enabled_ui(tables > 0, |ui| {
                        ui.radio_value(model, true, tr("model.use_model")).on_disabled_hover_text(tr("model.need_tables"));
                    });
                    ui.add_space(6.0);
                    ui.strong(tr("pivot.choose_place"));
                    ui.radio_value(new_sheet, true, tr("pivot.new_sheet"));
                    ui.radio_value(new_sheet, false, tr("pivot.existing_sheet"));
                    ui.add_enabled_ui(!*new_sheet, |ui| {
                        ui.label(tr("pivot.location"));
                        ref_edit(ui, dest, 1, &mut pick, 320.0);
                    });
                });
                if pick.field == Some(1) && !dest.is_empty() {
                    *new_sheet = false;
                }
                self.f10.pick = pick;
                match res {
                    DialogResult::None => Some(d),
                    DialogResult::Cancel => None,
                    DialogResult::Ok => {
                        let parsed = if *model { Some((usize::MAX, Range::default())) } else { f10::parse_ref(&self.wb, self.sheet, range) };
                        let Some((si, src)) = parsed else {
                            self.notices.push(Notice::error(tr("pivot.err_source")));
                            return Some(d);
                        };
                        let dest_at = if *new_sheet {
                            None
                        } else {
                            match f10::parse_ref(&self.wb, self.sheet, dest) {
                                Some((ds, r)) => Some((ds, r.start)),
                                None => {
                                    self.notices.push(Notice::error(tr("pivot.err_dest")));
                                    return Some(d);
                                }
                            }
                        };
                        if dest_at.is_some_and(|(ds, at)| ds == si && src.contains(at)) {
                            self.notices.push(Notice::error(tr("pivot.err_dest")));
                            return Some(d);
                        }
                        let spec = match self.wb.sheets.get(si) {
                            Some(sh) => PivotSpec { source_sheet: sh.name.clone(), source: src, lang: lang(), ..Default::default() },
                            None => PivotSpec { model: Some(pivot::ModelSource::default()), lang: lang(), ..Default::default() },
                        };
                        match self.wb.create_pivot(spec, dest_at) {
                            Ok(i) => {
                                if let Some(p) = self.wb.analysis.pivots.get(i) {
                                    let (name, at) = (p.sheet.clone(), p.at);
                                    if let Some(s) = self.wb.sheet_index(&name) {
                                        if s != self.sheet {
                                            self.select_sheet(s);
                                        }
                                        self.anchor = at;
                                        self.cursor = at;
                                        self.ensure_visible = true;
                                    }
                                }
                                self.f10.pivot.closed = None;
                                None
                            }
                            Err(e) => {
                                self.notices.push(Notice::refused(super::analysis_ui::error_text(&e)));
                                Some(d)
                            }
                        }
                    }
                }
            }
            PivotDlg::Values(v) => {
                let Some(p) = self.wb.analysis.pivots.get(v.index).cloned() else { return None };
                let Some(names) = self.pivot_names(&p.spec) else { return None };
                let Some(col) = p.spec.values.get(v.vi).map(|x| x.col) else { return None };
                let bases: Vec<(BaseField, String)> = p.spec.rows.iter().enumerate().map(|(i, f)| (BaseField::Row(i), names.get(f.col as usize).cloned().unwrap_or_default())).chain(p.spec.cols.iter().enumerate().map(|(i, f)| (BaseField::Col(i), names.get(f.col as usize).cloned().unwrap_or_default()))).collect();
                let res = tool_window(ctx, "pivot_values", tr("pivot.value_settings"), 360.0, focus, |ui| {
                    ui.label(format!("{} {}", tr("pivot.source_name"), col_name(&p.spec, &names, col)));
                    egui::Grid::new("pv_grid").num_columns(2).show(ui, |ui| {
                        ui.label(tr("pivot.custom_name"));
                        ui.add(egui::TextEdit::singleline(&mut v.name).desired_width(200.0));
                        ui.end_row();
                        ui.label(tr("pivot.summarize_by"));
                        egui::ComboBox::from_id_salt("pv_func").width(200.0).selected_text(func_label(v.func)).show_ui(ui, |ui| {
                            for (f, key) in FUNCS {
                                ui.selectable_value(&mut v.func, f, tr(key));
                            }
                        });
                        ui.end_row();
                        ui.label(tr("pivot.show_as"));
                        egui::ComboBox::from_id_salt("pv_show").width(200.0).selected_text(tr(SHOW_AS[v.show as usize % SHOW_AS.len()])).show_ui(ui, |ui| {
                            for (i, key) in SHOW_AS.iter().enumerate() {
                                ui.selectable_value(&mut v.show, i as u8, tr(key));
                            }
                        });
                        ui.end_row();
                        if needs_base(v.show) {
                            ui.label(tr("pivot.base_field"));
                            let shown = bases.get(v.base).map_or("", |b| b.1.as_str());
                            egui::ComboBox::from_id_salt("pv_base").width(200.0).selected_text(shown).show_ui(ui, |ui| {
                                for (i, (_, n)) in bases.iter().enumerate() {
                                    ui.selectable_value(&mut v.base, i, n);
                                }
                            });
                            ui.end_row();
                        }
                        if needs_item(v.show) {
                            ui.label(tr("pivot.base_item"));
                            ui.horizontal(|ui| {
                                egui::ComboBox::from_id_salt("pv_item").width(110.0).selected_text(match v.item {
                                    0 => tr("pivot.previous"),
                                    1 => tr("pivot.next"),
                                    _ => tr("pivot.item_named"),
                                }).show_ui(ui, |ui| {
                                    ui.selectable_value(&mut v.item, 0, tr("pivot.previous"));
                                    ui.selectable_value(&mut v.item, 1, tr("pivot.next"));
                                    ui.selectable_value(&mut v.item, 2, tr("pivot.item_named"));
                                });
                                if v.item == 2 {
                                    ui.add(egui::TextEdit::singleline(&mut v.item_name).desired_width(80.0));
                                }
                            });
                            ui.end_row();
                        }
                        ui.label(tr("pivot.number_format"));
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut v.fmt).desired_width(130.0).font(egui::TextStyle::Monospace));
                            crate::widgets::menu_button(ui, "…", |ui| {
                                for f in FORMATS {
                                    if ui.button(f).clicked() {
                                        v.fmt = if f == "General" { String::new() } else { f.to_string() };
                                        ui.close();
                                    }
                                }
                            });
                        });
                        ui.end_row();
                    });
                    if needs_base(v.show) && bases.is_empty() {
                        ui.label(RichText::new(tr("pivot.need_base")).small().weak());
                    }
                });
                match res {
                    DialogResult::None => Some(d),
                    DialogResult::Cancel => None,
                    DialogResult::Ok => {
                        let base = bases.get(v.base).map(|b| b.0);
                        let item = match v.item {
                            0 => BaseItem::Previous,
                            1 => BaseItem::Next,
                            _ => BaseItem::Named(v.item_name.clone()),
                        };
                        let show = match (v.show, base) {
                            (1, _) => ShowAs::PercentOfGrand,
                            (2, _) => ShowAs::PercentOfCol,
                            (3, _) => ShowAs::PercentOfRow,
                            (4, _) => ShowAs::PercentOfParent,
                            (5, Some(b)) => ShowAs::DifferenceFrom(b, item),
                            (6, Some(b)) => ShowAs::RunningTotal(b),
                            (7, Some(b)) => ShowAs::RankAsc(b),
                            (8, Some(b)) => ShowAs::RankDesc(b),
                            (9, Some(b)) => ShowAs::PercentOf(b, item),
                            (10, Some(b)) => ShowAs::PercentDifferenceFrom(b, item),
                            (11, Some(b)) => ShowAs::PercentRunningTotal(b),
                            (12, _) => ShowAs::PercentOfParentCol,
                            (13, _) => ShowAs::Index,
                            _ => ShowAs::Normal,
                        };
                        let mut spec = p.spec.clone();
                        if let Some(x) = spec.values.get_mut(v.vi) {
                            x.func = v.func;
                            x.show_as = show;
                            x.name = Some(v.name.trim().to_string()).filter(|n| !n.is_empty());
                            x.number_format = Some(v.fmt.trim().to_string()).filter(|n| !n.is_empty());
                        }
                        self.apply_pivot(v.index, spec);
                        None
                    }
                }
            }
            PivotDlg::Group(g) => {
                let res = tool_window(ctx, "pivot_group", tr("pivot.grouping"), 300.0, focus, |ui| {
                    ui.radio_value(&mut g.kind, 0, tr("pivot.group_none"));
                    ui.radio_value(&mut g.kind, 1, tr("pivot.group_dates"));
                    ui.add_enabled_ui(g.kind == 1, |ui| {
                        ui.horizontal(|ui| {
                            ui.add_space(18.0);
                            for (p, key) in [(DatePart::Years, "pivot.years"), (DatePart::Quarters, "pivot.quarters"), (DatePart::Months, "pivot.months"), (DatePart::Days, "pivot.days")] {
                                ui.radio_value(&mut g.part, p, tr(key));
                            }
                        });
                    });
                    ui.radio_value(&mut g.kind, 2, tr("pivot.group_numbers"));
                    ui.add_enabled_ui(g.kind == 2, |ui| {
                        egui::Grid::new("pg_grid").num_columns(2).show(ui, |ui| {
                            ui.label(tr("pivot.starting_at"));
                            ui.add(egui::TextEdit::singleline(&mut g.start).desired_width(100.0));
                            ui.end_row();
                            ui.label(tr("pivot.ending_at"));
                            ui.add(egui::TextEdit::singleline(&mut g.end).desired_width(100.0));
                            ui.end_row();
                            ui.label(tr("pivot.by"));
                            ui.add(egui::TextEdit::singleline(&mut g.by).desired_width(100.0));
                            ui.end_row();
                        });
                    });
                });
                match res {
                    DialogResult::None => Some(d),
                    DialogResult::Cancel => None,
                    DialogResult::Ok => {
                        let Some(p) = self.wb.analysis.pivots.get(g.index).cloned() else { return None };
                        let group = match g.kind {
                            1 => Grouping::Date(g.part),
                            2 => match (f10::parse_num(&self.wb, &g.start), f10::parse_num(&self.wb, &g.end), f10::parse_num(&self.wb, &g.by)) {
                                (Some(start), Some(end), Some(by)) if by > 0.0 && end >= start => Grouping::Number { start, end, by },
                                _ => {
                                    self.notices.push(Notice::error(tr("tools.err_param")));
                                    return Some(d);
                                }
                            },
                            _ => Grouping::None,
                        };
                        let mut spec = p.spec.clone();
                        let list = if g.cols { &mut spec.cols } else { &mut spec.rows };
                        if let Some(f) = list.get_mut(g.fi) {
                            f.group = group;
                            f.hidden.clear();
                        }
                        self.apply_pivot(g.index, spec);
                        None
                    }
                }
            }
            PivotDlg::Calc(c) => {
                let Some(p) = self.wb.analysis.pivots.get(c.index).cloned() else { return None };
                let Some(names) = self.pivot_names(&p.spec) else { return None };
                let mut remove = false;
                let mut add = false;
                let res = tool_window(ctx, "pivot_calc", tr("pivot.calc_title"), 380.0, focus, |ui| {
                    egui::Grid::new("pc_grid").num_columns(2).show(ui, |ui| {
                        ui.label(tr("pivot.calc_name"));
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut c.name).desired_width(200.0));
                            if !p.spec.calculated.is_empty() {
                                crate::widgets::menu_button(ui, "▾", |ui| {
                                    for (k, f) in p.spec.calculated.iter().enumerate() {
                                        if ui.button(&f.name).clicked() {
                                            c.current = Some(k);
                                            c.name = f.name.clone();
                                            c.formula = f.formula.clone();
                                            ui.close();
                                        }
                                    }
                                });
                            }
                        });
                        ui.end_row();
                        ui.label(tr("pivot.calc_formula"));
                        ui.add(egui::TextEdit::singleline(&mut c.formula).desired_width(260.0).font(egui::TextStyle::Monospace));
                        ui.end_row();
                    });
                    ui.add_space(4.0);
                    ui.label(tr("pivot.calc_fields"));
                    egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                        for n in &names {
                            if ui.selectable_label(false, n).on_hover_text(tr("pivot.calc_insert")).clicked() {
                                let plain = n.chars().all(|ch| ch.is_alphanumeric() || ch == '_');
                                let t = if plain { n.clone() } else { format!("'{}'", n.replace('\'', "''")) };
                                if !c.formula.trim_start().starts_with('=') {
                                    c.formula = format!("= {}", c.formula.trim());
                                }
                                c.formula.push_str(&t);
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        let label = if c.current.is_some() { tr("pivot.calc_modify") } else { tr("pivot.calc_add") };
                        if ui.button(label).clicked() {
                            add = true;
                        }
                        if ui.add_enabled(c.current.is_some(), egui::Button::new(tr("pivot.calc_delete"))).clicked() {
                            remove = true;
                        }
                    });
                });
                let mut spec = p.spec.clone();
                if remove {
                    if let Some(k) = c.current.take() {
                        let col = CALC_BASE + k as u32;
                        spec.calculated.remove(k);
                        spec.values.retain(|v| v.col != col);
                        for v in &mut spec.values {
                            if v.col > col {
                                v.col -= 1;
                            }
                        }
                        c.name.clear();
                        c.formula.clear();
                        self.apply_pivot(c.index, spec);
                    }
                    return Some(d);
                }
                match res {
                    DialogResult::Cancel => return None,
                    DialogResult::None if !add => return Some(d),
                    _ => {}
                }
                let close = res == DialogResult::Ok;
                let name = c.name.trim().to_string();
                if name.is_empty() || c.formula.trim().trim_start_matches('=').trim().is_empty() {
                    return if close { None } else { Some(d) };
                }
                if let Err(e) = pivot::calc::parse(&c.formula, &names, p.spec.lang == Lang::Es) {
                    self.notices.push(Notice::error(trf("pivot.calc_error", &[("t", &e)])));
                    return Some(d);
                }
                let k = match c.current.or_else(|| spec.calculated.iter().position(|f| f.name.eq_ignore_ascii_case(&name))) {
                    Some(k) => {
                        spec.calculated[k] = CalcField { name, formula: c.formula.trim().to_string() };
                        k
                    }
                    None => {
                        spec.calculated.push(CalcField { name, formula: c.formula.trim().to_string() });
                        spec.calculated.len() - 1
                    }
                };
                let col = CALC_BASE + k as u32;
                if !spec.values.iter().any(|v| v.col == col) {
                    spec.values.push(ValueField { col, ..Default::default() });
                }
                c.current = Some(k);
                self.apply_pivot(c.index, spec);
                if close { None } else { Some(d) }
            }
            PivotDlg::Filter(fd) => {
                let Some(p) = self.wb.analysis.pivots.get(fd.index).cloned() else { return None };
                let Some(names) = self.pivot_names(&p.spec) else { return None };
                let Some(field) = (if fd.cols { p.spec.cols.get(fd.fi) } else { p.spec.rows.get(fd.fi) }).cloned() else { return None };
                let fname = field.name.clone().unwrap_or_else(|| col_name(&p.spec, &names, field.col));
                let captions: Vec<String> = p.spec.values.iter().map(|v| v.name.clone().unwrap_or_else(|| trf("pivot.value_name", &[("fn", &func_label(v.func)), ("field", &col_name(&p.spec, &names, v.col))]))).collect();
                let title = if fd.by_value { trf("pivot.value_filter_title", &[("f", &fname)]) } else { trf("pivot.label_filter_title", &[("f", &fname)]) };
                let res = tool_window(ctx, "pivot_filter", &title, 420.0, focus, |ui| {
                    ui.label(tr("pivot.filter_show"));
                    ui.horizontal(|ui| {
                        if fd.by_value {
                            egui::ComboBox::from_id_salt("pf_value").width(140.0).selected_text(captions.get(fd.value).map_or("", |s| s.as_str())).show_ui(ui, |ui| {
                                for (i, c) in captions.iter().enumerate() {
                                    ui.selectable_value(&mut fd.value, i, c);
                                }
                            });
                            let k = ValueOp::ALL.iter().position(|o| *o == fd.vop).unwrap_or(0);
                            egui::ComboBox::from_id_salt("pf_vop").width(150.0).selected_text(tr(VALUE_OPS[k])).show_ui(ui, |ui| {
                                for (o, key) in ValueOp::ALL.iter().zip(VALUE_OPS) {
                                    ui.selectable_value(&mut fd.vop, *o, tr(key));
                                }
                            });
                        } else {
                            let k = LabelOp::ALL.iter().position(|o| *o == fd.lop).unwrap_or(0);
                            egui::ComboBox::from_id_salt("pf_lop").width(170.0).selected_text(tr(LABEL_OPS[k])).show_ui(ui, |ui| {
                                for (o, key) in LabelOp::ALL.iter().zip(LABEL_OPS) {
                                    ui.selectable_value(&mut fd.lop, *o, tr(key));
                                }
                            });
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut fd.a).desired_width(150.0));
                        let two = if fd.by_value { fd.vop.two() } else { fd.lop.two() };
                        if two {
                            ui.label(tr("pivot.and"));
                            ui.add(egui::TextEdit::singleline(&mut fd.b).desired_width(150.0));
                        }
                    });
                    if !fd.by_value {
                        ui.label(RichText::new(tr("pivot.wildcards")).small().weak());
                    }
                });
                match res {
                    DialogResult::None => Some(d),
                    DialogResult::Cancel => None,
                    DialogResult::Ok => {
                        let mut spec = p.spec.clone();
                        let list = if fd.cols { &mut spec.cols } else { &mut spec.rows };
                        let Some(f) = list.get_mut(fd.fi) else { return None };
                        if fd.by_value {
                            let num = |t: &str| f10::parse_num(&self.wb, t);
                            let (Some(a), b) = (num(&fd.a), num(&fd.b)) else {
                                self.notices.push(Notice::error(tr("tools.err_param")));
                                return Some(d);
                            };
                            if fd.vop.two() && b.is_none() {
                                self.notices.push(Notice::error(tr("tools.err_param")));
                                return Some(d);
                            }
                            f.value_filter = Some(ValueFilter { value: fd.value, op: fd.vop, a, b: b.unwrap_or(0.0) });
                        } else {
                            f.label_filter = Some(LabelFilter { op: fd.lop, a: fd.a.clone(), b: fd.b.clone() });
                        }
                        self.apply_pivot(fd.index, spec);
                        None
                    }
                }
            }
            PivotDlg::Slicers { index, checks } => {
                let Some(p) = self.wb.analysis.pivots.get(*index).cloned() else { return None };
                let Some(names) = self.pivot_names(&p.spec) else { return None };
                checks.resize(names.len(), false);
                let res = tool_window(ctx, "pivot_slicers", tr("slicer.insert_title"), 260.0, focus, |ui| {
                    egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                        for (n, on) in names.iter().zip(checks.iter_mut()) {
                            ui.checkbox(on, n);
                        }
                    });
                });
                match res {
                    DialogResult::None => Some(d),
                    DialogResult::Cancel => None,
                    DialogResult::Ok => {
                        self.settle_edit();
                        let before = self.wb.undo_len();
                        let (x0, y0) = self.slicer_origin(*index);
                        let mut n = 0;
                        for (c, on) in checks.iter().enumerate() {
                            if *on {
                                let off = n as f32 * 24.0;
                                if self.wb.add_slicer(*index, c as u32, x0 + off, y0 + off).is_ok() {
                                    n += 1;
                                }
                            }
                        }
                        let steps = self.wb.undo_len().saturating_sub(before);
                        self.wb.merge_undo(steps, "undo.slicer");
                        None
                    }
                }
            }
        }
    }

    /// Where new slicers of a pivot go: right of its table (sheet pixels).
    fn slicer_origin(&self, index: usize) -> (f32, f32) {
        let Some(p) = self.wb.analysis.pivots.get(index) else { return (0.0, 0.0) };
        let Some(si) = self.wb.sheet_index(&p.sheet) else { return (0.0, 0.0) };
        let sh = &self.wb.sheets[si];
        let a = p.area.unwrap_or(Range::single(p.at));
        let charts = sh.charts.iter().filter(|c| p.charts.contains(&c.id)).map(|c| c.rect(sh)[2]).fold(0.0f32, f32::max);
        (sh.col_x(a.end.col + 1).max(charts) + 20.0, sh.row_y(a.start.row))
    }

    pub(super) fn open_pivot_filter(&mut self, index: usize, cols: bool, fi: usize, by_value: bool) {
        let Some(p) = self.wb.analysis.pivots.get(index) else { return };
        let Some(f) = (if cols { p.spec.cols.get(fi) } else { p.spec.rows.get(fi) }) else { return };
        let mut d = FilterDlg { index, cols, fi, by_value, lop: LabelOp::Equals, vop: ValueOp::TopItems, value: 0, a: String::new(), b: String::new() };
        if by_value {
            match &f.value_filter {
                Some(v) => {
                    d.vop = v.op;
                    d.value = v.value;
                    d.a = f10::fmt_num(&self.wb, v.a);
                    d.b = if v.op.two() { f10::fmt_num(&self.wb, v.b) } else { String::new() };
                }
                None => d.a = "10".into(),
            }
        } else if let Some(l) = &f.label_filter {
            d.lop = l.op;
            d.a = l.a.clone();
            d.b = l.b.clone();
        }
        self.open_f10(F10Dlg::Pivot(PivotDlg::Filter(Box::new(d))), None);
    }

    pub(super) fn open_calc_field(&mut self, index: usize) {
        let n = self.wb.analysis.pivots.get(index).map_or(0, |p| p.spec.calculated.len());
        let d = CalcDlg { index, current: None, name: trf("pivot.calc_default", &[("n", &(n + 1).to_string())]), formula: "= 0".into() };
        self.open_f10(F10Dlg::Pivot(PivotDlg::Calc(Box::new(d))), None);
    }

    /// Show Details of the value under the cursor (a new sheet).
    pub(super) fn show_pivot_details(&mut self) -> bool {
        self.settle_edit();
        match self.wb.pivot_details(self.sheet, self.cursor) {
            Ok(s) => {
                self.select_sheet(s);
                self.anchor = CellRef::new(0, 0);
                self.cursor = CellRef::new(0, 0);
                true
            }
            Err(_) => false,
        }
    }

    /// Inserts a pivot chart for the pivot under the cursor.
    pub(super) fn insert_pivot_chart(&mut self) {
        let Some(index) = self.wb.pivot_at(self.sheet, self.cursor) else { return };
        self.settle_edit();
        let before = self.wb.undo_len();
        match self.wb.create_pivot_chart(index, apex_sheet::chart::ChartType::Column) {
            Ok(id) => {
                // Clear of the slicers of the table.
                let name = self.sh().name.clone();
                let right = self.wb.analysis.slicers.iter().filter(|s| s.pivot == index && s.sheet == name).map(|s| s.x + s.width).fold(f32::NEG_INFINITY, f32::max);
                if let Some(mut c) = self.sh().charts.iter().find(|c| c.id == id).cloned() {
                    let (x, y) = c.pos(self.sh());
                    if right.is_finite() && right + 20.0 > x {
                        c.set_pos(self.sh(), right + 20.0, y);
                        self.wb.set_chart(self.sheet, c);
                        let steps = self.wb.undo_len().saturating_sub(before);
                        self.wb.merge_undo(steps, "undo.insert_chart");
                    }
                }
            }
            Err(e) => self.notices.push(Notice::refused(super::analysis_ui::error_text(&e))),
        }
    }

    pub(super) fn open_slicers(&mut self) {
        let Some(index) = self.wb.pivot_at(self.sheet, self.cursor) else { return };
        self.open_f10(F10Dlg::Pivot(PivotDlg::Slicers { index, checks: Vec::new() }), None);
    }

    fn apply_pivot(&mut self, index: usize, spec: PivotSpec) {
        self.settle_edit();
        if let Err(e) = self.wb.replace_pivot(index, spec) {
            self.notices.push(Notice::refused(super::analysis_ui::error_text(&e)));
        }
    }

    fn open_value_settings(&mut self, index: usize, vi: usize) {
        let Some(v) = self.wb.analysis.pivots.get(index).and_then(|p| p.spec.values.get(vi)).cloned() else { return };
        let with_item = |show: u8, b: &BaseField, it: &BaseItem| {
            let (k, name) = match it {
                BaseItem::Previous => (0, String::new()),
                BaseItem::Next => (1, String::new()),
                BaseItem::Named(n) => (2, n.clone()),
            };
            (show, Some(*b), k, name)
        };
        let (show, base, item, item_name) = match &v.show_as {
            ShowAs::Normal => (0, None, 0, String::new()),
            ShowAs::PercentOfGrand => (1, None, 0, String::new()),
            ShowAs::PercentOfCol => (2, None, 0, String::new()),
            ShowAs::PercentOfRow => (3, None, 0, String::new()),
            ShowAs::PercentOfParent => (4, None, 0, String::new()),
            ShowAs::DifferenceFrom(b, it) => with_item(5, b, it),
            ShowAs::RunningTotal(b) => (6, Some(*b), 0, String::new()),
            ShowAs::RankAsc(b) => (7, Some(*b), 0, String::new()),
            ShowAs::RankDesc(b) => (8, Some(*b), 0, String::new()),
            ShowAs::PercentOf(b, it) => with_item(9, b, it),
            ShowAs::PercentDifferenceFrom(b, it) => with_item(10, b, it),
            ShowAs::PercentRunningTotal(b) => (11, Some(*b), 0, String::new()),
            ShowAs::PercentOfParentCol => (12, None, 0, String::new()),
            ShowAs::Index => (13, None, 0, String::new()),
        };
        let nrows = self.wb.analysis.pivots.get(index).map_or(0, |p| p.spec.rows.len());
        let base = match base {
            Some(BaseField::Row(i)) => i,
            Some(BaseField::Col(i)) => nrows + i,
            None => 0,
        };
        let d = ValueDlg { index, vi, name: v.name.clone().unwrap_or_default(), func: v.func, show, base, item, item_name, fmt: v.number_format.clone().unwrap_or_default() };
        self.open_f10(F10Dlg::Pivot(PivotDlg::Values(Box::new(d))), None);
    }

    fn open_group(&mut self, index: usize, cols: bool, fi: usize) {
        let Some(p) = self.wb.analysis.pivots.get(index).cloned() else { return };
        let Some(f) = (if cols { p.spec.cols.get(fi) } else { p.spec.rows.get(fi) }).cloned() else { return };
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        let mut date = false;
        if let Ok(src) = self.wb.pivot_source(&p.spec) {
            date = src.formats.get(f.col as usize).is_some_and(|x| x.is_some());
            if let Some(col) = src.cols.get(f.col as usize) {
                for v in col {
                    if let Value::Number(n) = v {
                        lo = lo.min(*n);
                        hi = hi.max(*n);
                    }
                }
            }
        }
        let (kind, part, start, end, by) = match f.group {
            Grouping::None => (0, DatePart::Months, lo, hi, 10.0),
            Grouping::Date(p) => (1, p, lo, hi, 10.0),
            Grouping::Number { start, end, by } => (2, DatePart::Months, start, end, by),
        };
        let kind = if kind == 0 && lo.is_finite() { if date { 1 } else { 2 } } else { kind };
        let num = |v: f64| if v.is_finite() { f10::fmt_num(&self.wb, v) } else { String::new() };
        let d = GroupDlg { index, cols, fi, kind, part, start: num(start), end: num(end), by: num(by) };
        self.open_f10(F10Dlg::Pivot(PivotDlg::Group(Box::new(d))), None);
    }

    /// PivotTable Analyze: field list, refresh, layout, styles.
    pub(super) fn pivot_menu(&mut self, ui: &mut egui::Ui) {
        let Some(index) = self.wb.pivot_at(self.sheet, self.cursor) else { return };
        let Some(p) = self.wb.analysis.pivots.get(index).cloned() else { return };
        if ui.button(tr("pivot.field_list")).clicked() {
            self.f10.pivot.closed = None;
            ui.close();
        }
        if ui.button(tr("pivot.refresh")).clicked() {
            self.settle_edit();
            if let Err(e) = self.wb.refresh_pivot(index) {
                self.notices.push(Notice::refused(super::analysis_ui::error_text(&e)));
            }
            ui.close();
        }
        if ui.add_enabled(self.wb.analysis.pivots.len() > 1, egui::Button::new(tr("pivot.refresh_all"))).clicked() {
            self.wb.refresh_all_pivots();
            ui.close();
        }
        if ui.button(tr("pivot.select")).clicked() {
            if let Some(a) = p.area {
                self.anchor = a.start;
                self.cursor = a.end;
            }
            ui.close();
        }
        ui.separator();
        crate::widgets::menu_button(ui, tr("pivot.layout"), |ui| {
            let mut spec = p.spec.clone();
            if layout_menu(ui, &mut spec) {
                self.apply_pivot(index, spec);
            }
        });
        crate::widgets::menu_button(ui, tr("pivot.styles"), |ui| {
            if let Some(s) = style_gallery(ui, &p.spec.style) {
                let mut spec = p.spec.clone();
                spec.style = s;
                self.apply_pivot(index, spec);
                ui.close();
            }
        });
        ui.separator();
        if ui.button(format!("{}…", tr("pivot.calc_field"))).clicked() {
            self.open_calc_field(index);
            ui.close();
        }
        if ui.button(tr("pivot.show_details")).clicked() {
            if !self.show_pivot_details() {
                self.notices.push(Notice::info(tr("pivot.no_details")));
            }
            ui.close();
        }
        if ui.button(tr("pivot.chart")).clicked() {
            self.insert_pivot_chart();
            ui.close();
        }
        if ui.button(format!("{}…", tr("slicer.insert"))).clicked() {
            self.open_slicers();
            ui.close();
        }
        ui.separator();
        if ui.button(tr("pivot.delete")).clicked() {
            self.settle_edit();
            if let Err(e) = self.wb.delete_pivot(index) {
                self.notices.push(Notice::refused(super::analysis_ui::error_text(&e)));
            }
            ui.close();
        }
    }

    // --- field list ------------------------------------------------------------------------

    /// The PivotTable Fields panel at the right, while the cursor is in a pivot.
    pub(super) fn pivot_panel(&mut self, ui: &mut egui::Ui) {
        let Some(index) = self.wb.pivot_at(self.sheet, self.cursor) else {
            self.f10.pivot.closed = None;
            return;
        };
        if self.f10.pivot.closed == Some(index) {
            return;
        }
        let Some(p) = self.wb.analysis.pivots.get(index).cloned() else { return };
        let mut spec = p.spec.clone();
        // On the data model every table column and measure gets a field
        // (only the used ones are read).
        let tables: Vec<(String, Vec<String>)> = if spec.model.is_some() { self.wb.tables().map(|(_, t)| (t.name.clone(), t.columns.iter().map(|c| c.name.clone()).collect())).collect() } else { Vec::new() };
        let measures: Vec<(String, String)> = self.wb.analysis.model.measures.iter().map(|m| (m.name.clone(), m.table.clone())).collect();
        if let Some(ms) = &mut spec.model {
            for (t, cols) in &tables {
                for c in cols {
                    ms.col_of(t, c);
                }
            }
            for (m, _) in &measures {
                ms.measure_of(m);
            }
        }
        let Some(names) = self.pivot_names(&spec) else { return };
        let numeric: Vec<bool> = match &spec.model {
            Some(ms) => ms.cols.iter().map(|(t, c)| self.model_numeric(t, c)).collect(),
            None => {
                let Some(si) = self.wb.sheet_index(&p.spec.source_sheet) else { return };
                (0..names.len() as u32).map(|c| self.numeric_col(si, &p.spec.source, c)).collect()
            }
        };
        let mut changed = false;
        let mut close = false;
        let mut open: Option<(Area, usize)> = None;
        let mut search = std::mem::take(&mut self.f10.pivot.search);
        let labels_of = |spec: &PivotSpec, f: &Field| -> Vec<String> {
            let probe = PivotSpec { rows: vec![f.clone()], cols: Vec::new(), values: Vec::new(), filters: Vec::new(), ..spec.clone() };
            self.wb.pivot_source(&probe).map(|src| pivot::field_labels(&src, f, spec.lang)).unwrap_or_default()
        };
        let calc_names: Vec<String> = p.spec.calculated.iter().map(|c| c.name.clone()).collect();
        let model_measures: Vec<String> = spec.model.as_ref().map(|m| m.measures.clone()).unwrap_or_default();
        let name_of = |col: u32| match (col.checked_sub(pivot::MEASURE_BASE), col.checked_sub(CALC_BASE)) {
            (Some(k), _) => model_measures.get(k as usize).cloned().unwrap_or_default(),
            (None, Some(k)) => calc_names.get(k as usize).cloned().unwrap_or_default(),
            _ => names.get(col as usize).cloned().unwrap_or_default(),
        };
        egui::Panel::right(Id::new(("pivot_panel", self.uid))).resizable(true).default_size(270.0).min_size(220.0).frame(egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::same(8))).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading(tr("pivot.fields_title"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::new("×").frame(false)).on_hover_text(tr("dialog.close")).clicked() {
                        close = true;
                    }
                });
            });
            ui.label(RichText::new(tr("pivot.choose_fields")).small());
            ui.add(egui::TextEdit::singleline(&mut search).hint_text(tr("pivot.search")).desired_width(f32::INFINITY));
            // The field list: check to add, drag into an area; dropping an
            // item back here removes it.
            let dropped = drop_zone(ui, ui.visuals().extreme_bg_color, |ui| {
                ui.set_min_width(ui.available_width());
                egui::ScrollArea::vertical().id_salt("pivot_fields").max_height(if spec.model.is_some() { 240.0 } else { 170.0 }).show(ui, |ui| {
                    let q = search.trim().to_lowercase();
                    if spec.model.is_some() {
                        model_field_list(ui, &mut spec, &mut changed, &tables, &measures, &numeric, &q);
                        return;
                    }
                    for (c, name) in names.iter().enumerate() {
                        if !q.is_empty() && !name.to_lowercase().contains(&q) {
                            continue;
                        }
                        let col = c as u32;
                        let mut on = uses(&spec, col);
                        ui.horizontal(|ui| {
                            if ui.checkbox(&mut on, "").changed() {
                                if on {
                                    let to = if numeric[c] { Area::Values } else { Area::Rows };
                                    move_to(&mut spec, Area::List, 0, col, to);
                                } else {
                                    spec.rows.retain(|f| f.col != col);
                                    spec.cols.retain(|f| f.col != col);
                                    spec.filters.retain(|f| f.field.col != col);
                                    spec.values.retain(|v| v.col != col);
                                }
                                changed = true;
                            }
                            ui.dnd_drag_source(Id::new(("pivot_field", c)), PDrag { from: Area::List, index: 0, col }, |ui| {
                                ui.label(name);
                            });
                        });
                    }
                    // Calculated fields go to values only.
                    for (k, name) in calc_names.iter().enumerate() {
                        if !q.is_empty() && !name.to_lowercase().contains(&q) {
                            continue;
                        }
                        let col = CALC_BASE + k as u32;
                        let mut on = uses(&spec, col);
                        ui.horizontal(|ui| {
                            if ui.checkbox(&mut on, "").changed() {
                                if on {
                                    move_to(&mut spec, Area::List, 0, col, Area::Values);
                                } else {
                                    spec.values.retain(|v| v.col != col);
                                }
                                changed = true;
                            }
                            ui.dnd_drag_source(Id::new(("pivot_calc_field", k)), PDrag { from: Area::List, index: 0, col }, |ui| {
                                ui.label(RichText::new(name).italics());
                            });
                        });
                    }
                });
            });
            if let Some(d) = dropped {
                if d.from != Area::List {
                    take(&mut spec, d.from, d.index);
                    changed = true;
                }
            }
            ui.add_space(4.0);
            ui.label(RichText::new(tr("pivot.drag_hint")).small());
            let areas = [(Area::Filters, "pivot.area_filters"), (Area::Cols, "pivot.area_cols"), (Area::Rows, "pivot.area_rows"), (Area::Values, "pivot.area_values")];
            let w = (ui.available_width() - 6.0) / 2.0;
            for pair in areas.chunks(2) {
                ui.horizontal_top(|ui| {
                for (area, key) in pair {
                    let items: Vec<(u32, String)> = match area {
                        Area::Filters => spec.filters.iter().map(|f| (f.field.col, f.field.name.clone().unwrap_or_else(|| name_of(f.field.col)))).collect(),
                        Area::Cols => spec.cols.iter().map(|f| (f.col, f.name.clone().unwrap_or_else(|| name_of(f.col)))).collect(),
                        Area::Rows => spec.rows.iter().map(|f| (f.col, f.name.clone().unwrap_or_else(|| name_of(f.col)))).collect(),
                        _ => spec.values.iter().map(|v| (v.col, v.name.clone().unwrap_or_else(|| if v.col >= pivot::MEASURE_BASE { name_of(v.col) } else { trf("pivot.value_name", &[("fn", &func_label(v.func)), ("field", &name_of(v.col))]) }))).collect(),
                    };
                    let fill = ui.visuals().faint_bg_color;
                    let dropped = ui.allocate_ui_with_layout(vec2(w, 100.0), egui::Layout::top_down(egui::Align::Min), |ui| drop_zone(ui, fill, |ui| {
                        ui.set_width(w - 14.0);
                        ui.set_min_height(90.0);
                        ui.label(RichText::new(tr(key)).strong().small());
                        for (i, (col, label)) in items.iter().enumerate() {
                            ui.horizontal(|ui| {
                                ui.dnd_drag_source(Id::new(("pivot_item", *area as u8, i)), PDrag { from: *area, index: i, col: *col }, |ui| {
                                    egui::Frame::new().fill(ui.visuals().widgets.inactive.bg_fill).corner_radius(3).inner_margin(egui::Margin::symmetric(4, 1)).show(ui, |ui| {
                                        ui.add(egui::Label::new(RichText::new(label).small()).truncate().selectable(false));
                                    });
                                });
                                if ui.small_button("…").clicked() {
                                    open = Some((*area, i));
                                }
                            });
                        }
                    })).inner;
                    if let Some(d) = dropped {
                        if d.from != *area {
                            move_to(&mut spec, d.from, d.index, d.col, *area);
                            changed = true;
                        }
                    }
                }
                });
                ui.add_space(2.0);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button(tr("pivot.refresh")).clicked() {
                    changed = true;
                }
                crate::widgets::menu_button(ui, tr("pivot.layout"), |ui| {
                    changed |= layout_menu(ui, &mut spec);
                });
                crate::widgets::menu_button(ui, tr("pivot.styles"), |ui| {
                    if let Some(s) = style_gallery(ui, &spec.style) {
                        spec.style = s;
                        changed = true;
                        ui.close();
                    }
                });
            });
            // The item menu opened with its button.
            let menu_id = Id::new(("pivot_item_menu", self.uid));
            if let Some(o) = open {
                let at = ui.ctx().pointer_latest_pos().unwrap_or_default();
                ui.data_mut(|d| d.insert_temp(menu_id, (o.0 as u8, o.1, at)));
            }
            let current: Option<(u8, usize, Pos2)> = ui.data(|d| d.get_temp(menu_id));
            if let Some((a, i, at)) = current {
                let area = [Area::List, Area::Filters, Area::Cols, Area::Rows, Area::Values][a as usize % 5];
                let mut done = false;
                let resp = egui::Area::new(menu_id.with("area")).order(egui::Order::Foreground).fixed_pos(at).show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_min_width(180.0);
                        ui.visuals_mut().button_frame = false;
                        let col = match area {
                            Area::Filters => spec.filters.get(i).map(|f| f.field.col),
                            Area::Cols => spec.cols.get(i).map(|f| f.col),
                            Area::Rows => spec.rows.get(i).map(|f| f.col),
                            Area::Values => spec.values.get(i).map(|v| v.col),
                            Area::List => None,
                        };
                        let Some(col) = col else {
                            done = true;
                            return;
                        };
                        if ui.button(tr("pivot.move_up")).clicked() {
                            shift(&mut spec, area, i, true);
                            changed = true;
                            done = true;
                        }
                        if ui.button(tr("pivot.move_down")).clicked() {
                            shift(&mut spec, area, i, false);
                            changed = true;
                            done = true;
                        }
                        ui.separator();
                        for (to, key) in [(Area::Filters, "pivot.to_filters"), (Area::Rows, "pivot.to_rows"), (Area::Cols, "pivot.to_cols"), (Area::Values, "pivot.to_values")] {
                            if to != area && ui.button(tr(key)).clicked() {
                                move_to(&mut spec, area, i, col, to);
                                changed = true;
                                done = true;
                            }
                        }
                        ui.separator();
                        if ui.button(tr("pivot.remove")).clicked() {
                            take(&mut spec, area, i);
                            changed = true;
                            done = true;
                        }
                        match area {
                            Area::Values => {
                                if ui.button(format!("{}…", tr("pivot.value_settings"))).clicked() {
                                    self.f10.dlg = None;
                                    done = true;
                                    ui.data_mut(|d| d.insert_temp(menu_id.with("values"), i));
                                }
                            }
                            Area::Rows | Area::Cols => {
                                if ui.button(format!("{}…", tr("pivot.group"))).clicked() {
                                    done = true;
                                    ui.data_mut(|d| d.insert_temp(menu_id.with("group"), (area == Area::Cols, i)));
                                }
                                let asc = spec.rows.iter().chain(&spec.cols).find(|f| f.col == col).is_none_or(|f| f.sort == pivot::SortOrder::LabelAsc);
                                let key = if asc { "pivot.sort_desc" } else { "pivot.sort_asc" };
                                if ui.button(tr(key)).clicked() {
                                    let list = if area == Area::Cols { &mut spec.cols } else { &mut spec.rows };
                                    if let Some(f) = list.get_mut(i) {
                                        f.sort = if asc { pivot::SortOrder::LabelDesc } else { pivot::SortOrder::LabelAsc };
                                    }
                                    changed = true;
                                    done = true;
                                }
                                ui.separator();
                                if ui.button(format!("{}…", tr("pivot.label_filters"))).clicked() {
                                    done = true;
                                    ui.data_mut(|d| d.insert_temp(menu_id.with("filter"), (area == Area::Cols, i, false)));
                                }
                                if ui.add_enabled(!spec.values.is_empty(), egui::Button::new(format!("{}…", tr("pivot.value_filters")))).clicked() {
                                    done = true;
                                    ui.data_mut(|d| d.insert_temp(menu_id.with("filter"), (area == Area::Cols, i, true)));
                                }
                                let list = if area == Area::Cols { &mut spec.cols } else { &mut spec.rows };
                                if let Some(f) = list.get_mut(i).filter(|f| f.label_filter.is_some() || f.value_filter.is_some()) {
                                    if ui.button(tr("pivot.clear_filter")).clicked() {
                                        f.label_filter = None;
                                        f.value_filter = None;
                                        changed = true;
                                        done = true;
                                    }
                                }
                            }
                            _ => {}
                        }
                        if area == Area::Filters {
                            ui.separator();
                            ui.label(RichText::new(tr("pivot.show_items")).weak());
                            if let Some(ff) = spec.filters.get(i).cloned() {
                                let labels = labels_of(&spec, &ff.field);
                                egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                                    let all = ff.items.is_empty();
                                    let mut a = all;
                                    if ui.checkbox(&mut a, tr("filter.select_all")).changed() {
                                        if let Some(f) = spec.filters.get_mut(i) {
                                            f.items.clear();
                                        }
                                        changed = true;
                                    }
                                    for l in &labels {
                                        let mut on = all || ff.items.iter().any(|x| x.eq_ignore_ascii_case(l));
                                        if ui.checkbox(&mut on, l).changed() {
                                            if let Some(f) = spec.filters.get_mut(i) {
                                                if f.items.is_empty() {
                                                    f.items = labels.clone();
                                                }
                                                if on {
                                                    f.items.push(l.clone());
                                                } else {
                                                    f.items.retain(|x| !x.eq_ignore_ascii_case(l));
                                                }
                                                if f.items.len() >= labels.len() {
                                                    f.items.clear();
                                                }
                                            }
                                            changed = true;
                                        }
                                    }
                                });
                            }
                        }
                    });
                });
                // The area may be shifted to fit the screen, so hit-test by layer.
                let press = ui.input(|inp| if inp.pointer.any_pressed() { inp.pointer.interact_pos() } else { None });
                let inside = press.is_some_and(|p| ui.ctx().layer_id_at(p) == Some(resp.response.layer_id));
                let clicked_out = press.is_some() && !inside && open.is_none();
                if done || clicked_out || ui.input(|inp| inp.key_pressed(Key::Escape)) {
                    ui.data_mut(|d| d.remove_temp::<(u8, usize, Pos2)>(menu_id));
                }
            }
        });
        self.f10.pivot.search = search;
        if close {
            self.f10.pivot.closed = Some(index);
        }
        if changed {
            // Measures come with their number format.
            if let Some(ms) = spec.model.clone() {
                for v in spec.values.iter_mut().filter(|v| v.number_format.is_none()) {
                    let fmt = v.col.checked_sub(pivot::MEASURE_BASE).and_then(|k| ms.measures.get(k as usize)).and_then(|n| self.wb.analysis.model.measure(n)).map(|m| m.format.clone()).filter(|f| !f.is_empty());
                    v.number_format = fmt;
                }
            }
            self.apply_pivot(index, spec);
        }
        let menu_id = Id::new(("pivot_item_menu", self.uid));
        if let Some(vi) = ui.data_mut(|d| d.remove_temp::<usize>(menu_id.with("values"))) {
            self.open_value_settings(index, vi);
        }
        if let Some((cols, fi)) = ui.data_mut(|d| d.remove_temp::<(bool, usize)>(menu_id.with("group"))) {
            self.open_group(index, cols, fi);
        }
        if let Some((cols, fi, by_value)) = ui.data_mut(|d| d.remove_temp::<(bool, usize, bool)>(menu_id.with("filter"))) {
            self.open_pivot_filter(index, cols, fi, by_value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_moves() {
        let mut s = PivotSpec::default();
        move_to(&mut s, Area::List, 0, 0, Area::Rows);
        move_to(&mut s, Area::List, 0, 2, Area::Values);
        move_to(&mut s, Area::List, 0, 1, Area::Rows);
        assert_eq!(s.rows.iter().map(|f| f.col).collect::<Vec<_>>(), vec![0, 1]);
        assert!(uses(&s, 2) && !uses(&s, 3));
        // Rows to columns keeps the field's settings.
        s.rows[1].group = Grouping::Date(DatePart::Years);
        move_to(&mut s, Area::Rows, 1, 1, Area::Cols);
        assert_eq!(s.cols[0].group, Grouping::Date(DatePart::Years));
        assert_eq!(s.rows.len(), 1);
        // A field in one axis only; values may repeat a field.
        move_to(&mut s, Area::List, 0, 1, Area::Filters);
        assert!(s.cols.is_empty() && s.filters.len() == 1);
        move_to(&mut s, Area::List, 0, 2, Area::Values);
        assert_eq!(s.values.len(), 2);
        shift(&mut s, Area::Values, 1, true);
        take(&mut s, Area::Values, 0);
        assert_eq!(s.values.len(), 1);
        shift(&mut s, Area::Rows, 0, true);
        assert_eq!(s.rows[0].col, 0);
    }
}
