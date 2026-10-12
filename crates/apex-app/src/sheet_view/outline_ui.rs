//! Grouping (outline) bars beside the headers, the Group and Ungroup
//! commands and the Subtotals dialog.

use std::cell::RefCell;

use apex_sheet::{Group, SubtotalOpts};

use super::data_ui::DataDlg;
use super::*;

/// Width of one outline level in the bars.
const STEP: f32 = 16.0;

/// Groups of the sheet, cached per revision.
pub(super) struct OutlineCache {
    key: (usize, u64),
    /// Groups of each level (index 0 = level 1), sorted and disjoint.
    rows: Vec<Vec<Group>>,
    cols: Vec<Vec<Group>>,
    levels: (u8, u8),
}

pub(super) struct SubDlg {
    range: Range,
    names: Vec<String>,
    by: usize,
    func: usize,
    cols: Vec<bool>,
    replace: bool,
}

/// (SUBTOTAL function number, i18n key) as the dialog lists them.
const FUNCS: [(u8, &str); 11] = [
    (9, "subtotal.sum"),
    (3, "subtotal.count"),
    (1, "subtotal.average"),
    (4, "subtotal.max"),
    (5, "subtotal.min"),
    (6, "subtotal.product"),
    (2, "subtotal.count_nums"),
    (7, "subtotal.stdev"),
    (8, "subtotal.stdevp"),
    (10, "subtotal.var"),
    (11, "subtotal.varp"),
];

enum Act {
    Level(bool, u8),
    Toggle(bool, Group, bool),
}

impl SheetView {
    fn outline(&self) -> std::cell::Ref<'_, OutlineCache> {
        let key = (self.sheet, self.wb.revision);
        if self.outline_cache.borrow().key != key {
            let by_level = |rows: bool| {
                let mut v: Vec<Vec<Group>> = Vec::new();
                for g in self.wb.outline_groups(self.sheet, rows) {
                    if v.len() < g.level as usize {
                        v.resize(g.level as usize, Vec::new());
                    }
                    v[g.level as usize - 1].push(g);
                }
                v
            };
            let (rows, cols) = (by_level(true), by_level(false));
            let levels = (rows.len() as u8, cols.len() as u8);
            *self.outline_cache.borrow_mut() = OutlineCache { key, rows, cols, levels };
        }
        self.outline_cache.borrow()
    }

    /// Space taken by the row (left) and column (top) outline bars.
    pub(super) fn outline_size(&self) -> Vec2 {
        let (r, c) = self.outline().levels;
        let size = |l: u8| if l == 0 { 0.0 } else { (l as f32 + 1.0) * STEP + 2.0 };
        vec2(size(r), size(c))
    }

    /// Paints the outline bars around `grid` (the headers and cells) and
    /// handles clicks on their buttons.
    pub(super) fn outline_bars(&mut self, ui: &mut egui::Ui, g: &Geo, full: Rect, grid: Rect) {
        if grid.min == full.min {
            return;
        }
        let v = ui.visuals().clone();
        let painter = ui.painter_at(full);
        let bg = theme::header_bg(&v);
        let line = Stroke::new(1.0, v.text_color().gamma_multiply(0.55));
        let text = v.text_color();
        let font = FontId::proportional(10.0);
        painter.rect_filled(Rect::from_min_max(full.min, pos2(grid.min.x, full.max.y)), 0.0, bg);
        painter.rect_filled(Rect::from_min_max(full.min, pos2(full.max.x, grid.min.y)), 0.0, bg);
        let mut buttons: Vec<(Rect, Act)> = Vec::new();
        let button = |p: &egui::Painter, r: Rect, label: &str| {
            p.rect(r, 2.0, v.extreme_bg_color, line, egui::StrokeKind::Inside);
            p.text(r.center(), egui::Align2::CENTER_CENTER, label, font.clone(), text);
        };
        let sh = self.sh();
        let cache = self.outline();
        for rows in [true, false] {
            let (groups, levels) = if rows { (&cache.rows, cache.levels.0) } else { (&cache.cols, cache.levels.1) };
            if levels == 0 {
                continue;
            }
            // Position across the bar of a level, and the visible stretch.
            let across = |level: u8| if rows { full.min.x } else { full.min.y } + 2.0 + (level as f32 - 1.0) * STEP + STEP / 2.0;
            let along = |i: u32| if rows { g.y(sh, i) } else { g.x(sh, i) };
            let (lo, hi) = if rows { (g.origin.y, g.view.max.y) } else { (g.origin.x, g.view.max.x) };
            let clip = if rows { Rect::from_min_max(pos2(full.min.x, lo), pos2(grid.min.x, hi)) } else { Rect::from_min_max(pos2(lo, full.min.y), pos2(hi, grid.min.y)) };
            let p = painter.with_clip_rect(clip);
            let at = |a: f32, b: f32| if rows { pos2(a, b) } else { pos2(b, a) };
            let below = if rows { sh.summary_below } else { sh.summary_right };
            // Indices on screen (frozen ones start at 0).
            let (first, last) = if rows {
                (if g.fr > 0 { g.top.row } else { g.row_at(sh, g.origin.y) }, g.row_at(sh, g.view.max.y))
            } else {
                (if g.fc > 0 { g.top.col } else { g.col_at(sh, g.origin.x) }, g.col_at(sh, g.view.max.x))
            };
            let visible = groups.iter().flat_map(|lv| {
                let from = lv.partition_point(|x| x.end + 2 < first);
                lv[from..].iter().take_while(move |x| x.start <= last + 2)
            });
            for grp in visible {
                let (a, b) = (along(grp.start), along(grp.end + 1));
                if b < lo - STEP || a > hi + STEP {
                    continue;
                }
                let x = across(grp.level);
                let summary = if below || grp.start == 0 { grp.end + 1 } else { grp.start - 1 };
                let (s0, s1) = (along(summary), along(summary + 1));
                let collapsed = self.wb.is_collapsed(self.sheet, rows, grp);
                if !collapsed && b - a > 0.5 {
                    // Bracket along the detail with a tick at the far end.
                    let (from, to) = if below { (a + 2.0, b) } else { (a, b - 2.0) };
                    p.line_segment([at(x, from), at(x, to)], line);
                    let tick = if below { from } else { to };
                    p.line_segment([at(x, tick), at(x + 4.0, tick)], line);
                }
                if s1 - s0 > 4.0 {
                    let c = (s0 + s1) / 2.0;
                    let r = Rect::from_center_size(at(x, c), vec2(11.0, 11.0));
                    button(&p, r, if collapsed { "+" } else { "-" });
                    buttons.push((r, Act::Toggle(rows, *grp, !collapsed)));
                }
            }
            // Level buttons 1..=max+1 beside the headers.
            let band = if rows { (grid.min.y + g.origin.y) / 2.0 } else { (grid.min.x + g.origin.x) / 2.0 };
            for l in 1..=levels + 1 {
                let r = Rect::from_center_size(at(across(l), band), vec2(13.0, 13.0));
                button(&painter, r, &l.to_string());
                buttons.push((r, Act::Level(rows, l)));
            }
        }
        drop(cache);
        let strips = [Rect::from_min_max(full.min, pos2(grid.min.x, full.max.y)), Rect::from_min_max(full.min, pos2(full.max.x, grid.min.y))];
        for (i, s) in strips.into_iter().enumerate() {
            if s.width() < 1.0 || s.height() < 1.0 {
                continue;
            }
            let resp = ui.interact(s, self.grid_id().with(("outline", i)), Sense::click());
            let Some(p) = resp.interact_pointer_pos().filter(|_| resp.clicked()) else { continue };
            if let Some((_, act)) = buttons.iter().find(|(r, _)| r.expand(2.0).contains(p)) {
                self.settle_edit();
                match act {
                    Act::Level(rows, l) => self.wb.show_level(self.sheet, *rows, *l),
                    Act::Toggle(rows, grp, collapse) => self.wb.set_collapsed(self.sheet, *rows, grp, *collapse),
                }
                self.focus_grid = true;
            }
        }
    }

    // --- commands ------------------------------------------------------------------

    /// Groups or ungroups the selection; asks rows or columns when the
    /// selection is not whole rows or columns.
    pub(super) fn group_cmd(&mut self, ungroup: bool) {
        self.settle_edit();
        match (self.whole_rows(), self.whole_cols()) {
            (true, false) => self.group_now(ungroup, true),
            (false, true) => self.group_now(ungroup, false),
            _ => self.data.dialog = Some(DataDlg::Group { ungroup, rows: true }),
        }
    }

    pub(super) fn group_now(&mut self, ungroup: bool, rows: bool) {
        let sel = self.sel();
        let (a, b) = if rows { (sel.start.row, sel.end.row) } else { (sel.start.col, sel.end.col) };
        if ungroup {
            if !self.wb.ungroup(self.sheet, rows, a, b) {
                self.notices.push(Notice::info(tr("group.none")));
            }
        } else if self.wb.max_level(self.sheet, rows) >= apex_sheet::MAX_LEVEL && (a..=b).any(|i| self.wb.outline_levels(self.sheet, rows).get(&i) == Some(&apex_sheet::MAX_LEVEL)) {
            self.notices.push(Notice::error(tr("group.too_deep")));
        } else {
            self.wb.group(self.sheet, rows, a, b);
        }
    }

    /// The innermost group holding the cursor (or summarized by it).
    fn group_at_cursor(&self, rows: bool) -> Option<Group> {
        let i = if rows { self.cursor.row } else { self.cursor.col };
        let below = if rows { self.sh().summary_below } else { self.sh().summary_right };
        let groups = self.wb.outline_groups(self.sheet, rows);
        let inside = groups.iter().filter(|g| g.start <= i && i <= g.end).max_by_key(|g| g.level);
        let summary = || groups.iter().filter(|g| if below { g.end + 1 == i } else { g.start == i + 1 }).min_by_key(|g| g.level);
        inside.or_else(summary).copied()
    }

    /// Hide detail (collapse) or show detail (expand) at the cursor.
    pub(super) fn detail_cmd(&mut self, show: bool) {
        self.settle_edit();
        let rows = !self.whole_cols();
        match self.group_at_cursor(rows) {
            Some(g) => self.wb.set_collapsed(self.sheet, rows, &g, !show),
            None => self.notices.push(Notice::info(tr("group.none"))),
        }
    }

    pub(super) fn has_outline(&self) -> bool {
        let (r, c) = self.outline().levels;
        r + c > 0
    }

    pub(super) fn open_subtotals(&mut self) {
        self.settle_edit();
        let range = self.wb.current_region(self.sheet, self.cursor);
        if range.rows() < 2 {
            self.notices.push(Notice::info(tr("subtotal.no_data")));
            return;
        }
        let names: Vec<String> = (range.start.col..=range.end.col.min(range.start.col + 499))
            .map(|c| {
                let h = self.wb.display(self.sheet, CellRef::new(range.start.row, c)).0;
                if h.is_empty() { format!("{} {}", tr("sort.column"), apex_sheet::addr::col_name(c)) } else { h }
            })
            .collect();
        // Total the last numeric column by default.
        let numeric = |i: usize| matches!(self.sh().value(CellRef::new(range.start.row + 1, range.start.col + i as u32)), Value::Number(_));
        let last_num = (0..names.len()).rev().find(|&i| i > 0 && numeric(i)).unwrap_or(names.len() - 1);
        let cols = (0..names.len()).map(|i| i == last_num).collect();
        self.anchor = range.start;
        self.cursor = range.end;
        self.data.dialog = Some(DataDlg::Subtotals(Box::new(SubDlg { range, names, by: 0, func: 0, cols, replace: true })));
    }

    /// Group or Subtotals dialog; `None` when the dialog handled itself.
    pub(super) fn outline_dialog(&mut self, ctx: &egui::Context, d: &mut DataDlg) -> Option<widgets::DialogResult> {
        match d {
            DataDlg::Group { ungroup, rows } => {
                let title = tr(if *ungroup { "group.ungroup" } else { "group.group" });
                Some(widgets::dialog(ctx, "group", title, 260.0, |ui| {
                    ui.radio_value(rows, true, tr("group.rows"));
                    ui.radio_value(rows, false, tr("group.cols"));
                }))
            }
            DataDlg::Subtotals(s) => {
                let remove = std::cell::Cell::new(false);
                let res = widgets::dialog(ctx, "subtotals", tr("subtotal.title"), 380.0, |ui| {
                    ui.label(tr("subtotal.at_change"));
                    egui::ComboBox::from_id_salt("sub_by").width(340.0).selected_text(s.names[s.by].as_str()).show_ui(ui, |ui| {
                        for (i, n) in s.names.iter().enumerate() {
                            ui.selectable_value(&mut s.by, i, n.as_str());
                        }
                    });
                    ui.add_space(4.0);
                    ui.label(tr("subtotal.use"));
                    egui::ComboBox::from_id_salt("sub_fn").width(340.0).selected_text(tr(FUNCS[s.func].1)).show_ui(ui, |ui| {
                        for (i, f) in FUNCS.iter().enumerate() {
                            ui.selectable_value(&mut s.func, i, tr(f.1));
                        }
                    });
                    ui.add_space(4.0);
                    ui.label(tr("subtotal.add_to"));
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.set_width(340.0);
                        egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                            for (on, n) in s.cols.iter_mut().zip(&s.names) {
                                ui.checkbox(on, n.as_str());
                            }
                        });
                    });
                    ui.add_space(4.0);
                    ui.checkbox(&mut s.replace, tr("subtotal.replace"));
                    ui.add_space(4.0);
                    if ui.button(tr("subtotal.remove_all")).clicked() {
                        remove.set(true);
                    }
                });
                if remove.get() {
                    let r = self.wb.remove_subtotals(self.sheet, &s.range);
                    self.anchor = r.start;
                    self.cursor = r.end;
                    return Some(widgets::DialogResult::Cancel);
                }
                Some(res)
            }
            _ => None,
        }
    }

    pub(super) fn outline_dialog_ok(&mut self, d: DataDlg) {
        match d {
            DataDlg::Group { ungroup, rows } => self.group_now(ungroup, rows),
            DataDlg::Subtotals(s) => {
                let cols: Vec<u32> = s.cols.iter().enumerate().filter(|(_, on)| **on).map(|(i, _)| s.range.start.col + i as u32).collect();
                if cols.is_empty() {
                    return;
                }
                let o = SubtotalOpts {
                    by: s.range.start.col + s.by as u32,
                    func: FUNCS[s.func].0,
                    cols,
                    replace: s.replace,
                    total_label: tr("subtotal.total_label").to_string(),
                    grand_label: tr("subtotal.grand_total").to_string(),
                };
                let r = self.wb.subtotals(self.sheet, &s.range, &o);
                self.anchor = r.start;
                self.cursor = r.end;
            }
            _ => {}
        }
    }

    pub(super) fn outline_menu(&mut self, ui: &mut egui::Ui) {
        if ui.add(egui::Button::new(format!("{}…", tr("group.group"))).shortcut_text(tr("key.group"))).clicked() {
            self.group_cmd(false);
            ui.close();
        }
        if ui.add(egui::Button::new(format!("{}…", tr("group.ungroup"))).shortcut_text(tr("key.ungroup"))).clicked() {
            self.group_cmd(true);
            ui.close();
        }
        let any = self.has_outline();
        if ui.add_enabled(any, egui::Button::new(tr("group.hide_detail"))).clicked() {
            self.detail_cmd(false);
            ui.close();
        }
        if ui.add_enabled(any, egui::Button::new(tr("group.show_detail"))).clicked() {
            self.detail_cmd(true);
            ui.close();
        }
        if ui.add_enabled(any, egui::Button::new(tr("group.clear"))).clicked() {
            self.wb.clear_outline(self.sheet);
            ui.close();
        }
    }
}

pub(super) type OutlineCell = RefCell<OutlineCache>;

impl Default for OutlineCache {
    fn default() -> Self {
        // A key no view has, so the first frame fills it.
        OutlineCache { key: (usize::MAX, 0), rows: Vec::new(), cols: Vec::new(), levels: (0, 0) }
    }
}
