//! Tables on the grid: Insert > Table (Ctrl+T), Format as Table, the Table
//! Design menu, the totals row function list, the table look under the
//! cells and the filter buttons of tables whose filter is not the active one.

use apex_sheet::data::AutoFilter;
use apex_sheet::table::{self, FAMILIES, Table, TotalFn};
use apex_sheet::workbook::{TableError, TableId};

use super::f10::{self, F10Dlg, ref_edit, tool_window};
use super::*;
use crate::widgets::DialogResult;

pub(super) enum TableDlg {
    Create { range: String, header: bool, style: String },
    Resize { id: TableId, range: String },
    Rename { id: TableId, name: String },
}

pub(super) fn error_text(e: TableError) -> &'static str {
    match e {
        TableError::Overlap => tr("xtab.err_overlap"),
        TableError::Merged => tr("xtab.err_merged"),
        TableError::BadName => tr("xtab.err_name"),
        TableError::NameTaken => tr("xtab.err_name_taken"),
        TableError::BadRange | TableError::NotFound => tr("xtab.err_range"),
    }
}

/// Caption of a built-in style: "Medium 2" in the gallery's language.
fn style_caption(name: &str) -> String {
    match table::parse_style_name(name) {
        Some((0, k)) => trf("xtab.style_light", &[("n", &k)]),
        Some((1, k)) => trf("xtab.style_medium", &[("n", &k)]),
        Some((_, k)) => trf("xtab.style_dark", &[("n", &k)]),
        None => tr("xtab.style_none").to_string(),
    }
}

fn total_fn_label(f: TotalFn) -> &'static str {
    match f {
        TotalFn::None => tr("xtab.fn_none"),
        TotalFn::Average => tr("xtab.fn_average"),
        TotalFn::Count => tr("xtab.fn_count"),
        TotalFn::CountNums => tr("xtab.fn_count_nums"),
        TotalFn::Max => tr("xtab.fn_max"),
        TotalFn::Min => tr("xtab.fn_min"),
        TotalFn::Sum => tr("xtab.fn_sum"),
        TotalFn::StdDev => tr("xtab.fn_stddev"),
        TotalFn::Var => tr("xtab.fn_var"),
        TotalFn::Custom => tr("xtab.fn_custom"),
    }
}

/// Paints a small table in `style` (the gallery thumbnails).
pub(super) fn paint_thumbnail(p: &egui::Painter, r: Rect, style: &table::TableStyle, totals: bool) {
    let (rows, cols) = (5u32, 5u32);
    let mut t = Table::new(0, "T", Range::new(CellRef::new(0, 0), CellRef::new(rows - 1, cols - 1)), (0..cols).map(|i| format!("C{i}")).collect());
    t.style = style.clone();
    t.totals = totals;
    p.rect_filled(r, 0.0, Color32::WHITE);
    let (w, h) = (r.width() / cols as f32, r.height() / rows as f32);
    let cell = |row: u32, col: u32| Rect::from_min_size(r.min + vec2(col as f32 * w, row as f32 * h), vec2(w, h));
    for row in 0..rows {
        for col in 0..cols {
            let c = CellRef::new(row, col);
            let Some(l) = t.look(c) else { continue };
            let cr = cell(row, col);
            if let Some(f) = l.fill {
                p.rect_filled(cr, 0.0, theme::rgb(f));
            }
            // A dash of "text" in the cell's color.
            let tc = l.color.map_or(Color32::from_gray(90), theme::rgb);
            let y = cr.center().y;
            p.line_segment([pos2(cr.min.x + w * 0.2, y), pos2(cr.max.x - w * 0.2, y)], Stroke::new(if l.bold { 1.6 } else { 1.0 }, tc));
            for (line, a, b) in [(l.top, cr.left_top(), cr.right_top()), (l.bottom, cr.left_bottom(), cr.right_bottom()), (l.left, cr.left_top(), cr.left_bottom()), (l.right, cr.right_top(), cr.right_bottom())] {
                if let Some(line) = line {
                    p.line_segment([a, b], Stroke::new(1.0, line.color.map_or(Color32::BLACK, theme::rgb)));
                }
            }
        }
    }
    if style.name.is_empty() {
        p.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_gray(170)), StrokeKind::Inside);
    }
}

impl SheetView {
    pub(super) fn cursor_table(&self) -> Option<TableId> {
        self.wb.table_at(self.sheet, self.cursor)
    }

    pub(super) fn guess_header(&self, r: &Range) -> bool {
        if r.rows() < 2 {
            return false;
        }
        let sh = self.sh();
        let mut any = false;
        for col in r.start.col..=r.end.col.min(r.start.col + 200) {
            match sh.value(CellRef::new(r.start.row, col)) {
                Value::Text(_) => any = true,
                Value::Empty => {}
                _ => return false,
            }
        }
        any
    }

    /// Insert > Table: asks for the range (the data around the cursor).
    /// Inside a table, `style` restyles it instead.
    pub(super) fn open_create_table(&mut self, style: Option<&str>) {
        self.settle_edit();
        if let Some(id) = self.cursor_table() {
            if let (Some(s), Some(mut t)) = (style, self.wb.table(id).cloned()) {
                t.style.name = s.to_string();
                self.wb.set_table(id, t, "undo.table");
            }
            return;
        }
        let sel = self.sel();
        let r = if sel.is_single() {
            self.wb.current_region(self.sheet, self.cursor)
        } else {
            self.wb.clamp_to_used(self.sheet, &sel).map_or(sel, |u| Range::new(sel.start, u.end))
        };
        let header = self.guess_header(&r);
        self.anchor = r.start;
        self.cursor = r.end;
        let d = TableDlg::Create { range: f10::abs_range(&r), header, style: style.unwrap_or(table::DEFAULT_STYLE).to_string() };
        self.open_f10(F10Dlg::Table(d), Some(0));
    }

    pub(super) fn table_dialog(&mut self, ctx: &egui::Context, mut d: TableDlg) -> Option<TableDlg> {
        let focus = self.grid_has_focus(ctx);
        let mut pick = std::mem::take(&mut self.f10.pick);
        let res = match &mut d {
            TableDlg::Create { range, header, .. } => tool_window(ctx, "create_table", tr("xtab.create_title"), 300.0, focus, |ui| {
                ui.label(tr("xtab.where"));
                ref_edit(ui, range, 0, &mut pick, 280.0);
                ui.add_space(4.0);
                ui.checkbox(header, tr("xtab.has_headers"));
            }),
            TableDlg::Resize { range, .. } => tool_window(ctx, "resize_table", tr("xtab.resize_title"), 300.0, focus, |ui| {
                ui.label(tr("xtab.resize_prompt"));
                ref_edit(ui, range, 0, &mut pick, 280.0);
                ui.label(RichText::new(tr("xtab.resize_note")).small().weak());
            }),
            TableDlg::Rename { name, .. } => tool_window(ctx, "rename_table", tr("xtab.rename_title"), 280.0, focus, |ui| {
                ui.label(tr("xtab.name"));
                let r = ui.add(egui::TextEdit::singleline(name).desired_width(260.0));
                if !r.has_focus() && !r.lost_focus() && ui.memory(|m| m.focused().is_none()) {
                    r.request_focus();
                }
            }),
        };
        self.f10.pick = pick;
        match res {
            DialogResult::None => Some(d),
            DialogResult::Cancel => None,
            DialogResult::Ok => {
                let err = match &d {
                    TableDlg::Create { range, header, style } => match f10::parse_ref(&self.wb, self.sheet, range) {
                        Some((si, r)) if si == self.sheet => self.wb.create_table(si, r, *header, style).err(),
                        _ => Some(TableError::BadRange),
                    },
                    TableDlg::Resize { id, range } => match f10::parse_ref(&self.wb, self.sheet, range) {
                        Some((si, r)) if si == id.0 => self.wb.resize_table(*id, r).err(),
                        _ => Some(TableError::BadRange),
                    },
                    TableDlg::Rename { id, name } => self.wb.rename_table(*id, name).err(),
                };
                match err {
                    Some(e) => {
                        self.notices.push(Notice::info(error_text(e)));
                        Some(d)
                    }
                    None => None,
                }
            }
        }
    }

    /// The style gallery; returns the style picked.
    pub(super) fn style_gallery(&mut self, ui: &mut egui::Ui, current: Option<&Table>) -> Option<String> {
        let mut out = None;
        let base = current.map(|t| t.style.clone()).unwrap_or_default();
        let (tw, th) = (46.0, 34.0);
        egui::ScrollArea::vertical().max_height(460.0).show(ui, |ui| {
            let mut entries: Vec<(usize, String)> = vec![(0, String::new())];
            for (fi, (prefix, n)) in FAMILIES.iter().enumerate() {
                entries.extend((1..=*n).map(|k| (fi, format!("{prefix}{k}"))));
            }
            for (fi, key) in [(0usize, "xtab.light"), (1, "xtab.medium"), (2, "xtab.dark")] {
                ui.label(RichText::new(tr(key)).strong());
                egui::Grid::new(("table_gallery", fi)).spacing(vec2(4.0, 4.0)).show(ui, |ui| {
                    for (i, (_, name)) in entries.iter().filter(|(f, _)| *f == fi).enumerate() {
                        let (r, resp) = ui.allocate_exact_size(vec2(tw, th), Sense::click());
                        let mut st = base.clone();
                        st.name = name.clone();
                        paint_thumbnail(ui.painter(), r.shrink(2.0), &st, false);
                        let chosen = current.is_some_and(|t| t.style.name == *name);
                        if chosen || resp.hovered() {
                            ui.painter().rect_stroke(r, 2.0, Stroke::new(2.0, theme::SHEET_ACCENT), StrokeKind::Inside);
                        }
                        if resp.on_hover_text(style_caption(name)).clicked() {
                            out = Some(name.clone());
                        }
                        if i % 7 == 6 {
                            ui.end_row();
                        }
                    }
                });
                ui.add_space(4.0);
            }
        });
        out
    }

    fn edit_table(&mut self, id: TableId, f: impl FnOnce(&mut Table)) {
        if let Some(mut t) = self.wb.table(id).cloned() {
            f(&mut t);
            self.wb.set_table(id, t, "undo.table");
        }
    }

    /// Table Design: name, size, style options and styles.
    pub(super) fn table_menu(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.cursor_table() else { return };
        let Some(t) = self.wb.table(id).cloned() else { return };
        ui.label(RichText::new(format!("{} {}", tr("xtab.name"), t.name)).weak());
        if ui.button(format!("{}…", tr("xtab.rename"))).clicked() {
            self.open_f10(F10Dlg::Table(TableDlg::Rename { id, name: t.name.clone() }), None);
            ui.close();
        }
        if ui.button(format!("{}…", tr("xtab.resize"))).clicked() {
            self.anchor = t.range.start;
            self.cursor = t.range.end;
            self.open_f10(F10Dlg::Table(TableDlg::Resize { id, range: f10::abs_range(&t.range) }), Some(0));
            ui.close();
        }
        if ui.button(format!("{}…", tr("xtab.summarize_pivot"))).clicked() {
            self.open_pivot_create(Some(t.name.clone()));
            ui.close();
        }
        if ui.button(tr("xtab.convert")).clicked() {
            self.settle_edit();
            self.wb.convert_table_to_range(id);
            ui.close();
        }
        ui.separator();
        ui.label(RichText::new(tr("xtab.options")).weak());
        let mut totals = t.totals;
        if ui.checkbox(&mut totals, tr("xtab.total_row")).changed() {
            self.settle_edit();
            self.wb.set_table_totals(id, totals);
        }
        let flags: [(&str, fn(&mut Table) -> &mut bool); 5] = [
            ("xtab.banded_rows", |t| &mut t.style.row_stripes),
            ("xtab.banded_cols", |t| &mut t.style.col_stripes),
            ("xtab.first_col", |t| &mut t.style.first_col),
            ("xtab.last_col", |t| &mut t.style.last_col),
            ("xtab.filter_button", |t| &mut t.filter_button),
        ];
        for (key, get) in flags {
            let mut tt = t.clone();
            let mut v = *get(&mut tt);
            let enabled = key != "xtab.filter_button" || t.header;
            if ui.add_enabled(enabled, egui::Checkbox::new(&mut v, tr(key))).changed() {
                self.settle_edit();
                self.edit_table(id, |x| *get(x) = v);
            }
        }
        ui.separator();
        crate::widgets::menu_button(ui, tr("xtab.styles"), |ui| {
            if let Some(name) = self.style_gallery(ui, Some(&t)) {
                self.edit_table(id, |x| x.style.name = name);
                ui.close();
            }
        });
    }

    // --- totals row ------------------------------------------------------------------

    /// The function button of a totals row cell under the cursor.
    fn totals_button(&self) -> Option<(TableId, usize, Rect)> {
        if self.edit.is_some() || !self.sel().is_single() {
            return None;
        }
        let r = self.data.cursor_rect?;
        let id = self.cursor_table()?;
        let t = self.wb.table(id)?;
        if t.totals_row() != Some(self.cursor.row) {
            return None;
        }
        let s = r.height().clamp(12.0, 20.0);
        Some((id, (self.cursor.col - t.range.start.col) as usize, Rect::from_min_size(pos2(r.max.x + 1.0, r.max.y - s), vec2(s, s))))
    }

    pub(super) fn totals_popup(&mut self, ctx: &egui::Context) {
        let Some((id, col, at)) = self.f10.total_menu else { return };
        let current = self.wb.table(id).and_then(|t| t.columns.get(col)).map(|c| c.total);
        let Some(current) = current else {
            self.f10.total_menu = None;
            return;
        };
        let mut picked = None;
        let area = egui::Area::new(Id::new(("totals_menu", self.uid))).order(egui::Order::Foreground).fixed_pos(at).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(150.0);
                for f in TotalFn::ALL.into_iter().filter(|f| *f != TotalFn::Custom) {
                    if ui.add(egui::Button::selectable(current == f, total_fn_label(f))).clicked() {
                        picked = Some(f);
                    }
                }
            });
        });
        if let Some(f) = picked {
            self.settle_edit();
            self.wb.set_total_fn(id, col, f);
            self.f10.total_menu = None;
        } else if ctx.input(|i| i.pointer.any_pressed() || i.key_pressed(Key::Escape)) && !area.response.contains_pointer() {
            self.f10.total_menu = None;
        }
    }

    // --- grid ---------------------------------------------------------------------------

    /// Filter buttons of tables whose filter is not the sheet's active one.
    fn inactive_table_buttons(&self, g: &Geo) -> Vec<(usize, u32, Rect)> {
        let sh = self.sh();
        let active = sh.filter.as_ref().map(|f| f.range);
        let mut out = Vec::new();
        for (i, t) in sh.tables.iter().enumerate() {
            if !t.filter_button || !t.header || Some(t.filter_range()) == active || sh.row_height(t.range.start.row) == 0.0 {
                continue;
            }
            for col in (g.top.col..g.fc).chain(g.scroll_cols(sh)) {
                if col < t.range.start.col || col > t.range.end.col {
                    continue;
                }
                let r = g.cell_rect(sh, CellRef::new(t.range.start.row, col));
                if r.width() < 4.0 || !r.intersects(g.view) {
                    continue;
                }
                let s = (r.height() - 3.0).min(17.0 * g.z).max(8.0f32.min(17.0 * g.z)).min(r.width() - 2.0);
                out.push((i, col, Rect::from_min_max(pos2(r.max.x - s - 2.0, r.max.y - s - 2.0), pos2(r.max.x - 2.0, r.max.y - 2.0))));
            }
        }
        out
    }

    /// Buttons over the grid: other tables' filter buttons, the totals list.
    pub(super) fn paint_table_overlays(&self, painter: &egui::Painter, g: &Geo) {
        if self.sh().tables.is_empty() {
            return;
        }
        let p = painter.with_clip_rect(g.view);
        for (_, _, r) in self.inactive_table_buttons(g) {
            data_ui::paint_drop_button(&p, r, false, None);
        }
        if let Some((_, _, r)) = self.totals_button() {
            data_ui::paint_drop_button(painter, r, false, None);
        }
    }

    /// A click on a table button; true if it took it.
    pub(super) fn table_click(&mut self, g: &Geo, p: Pos2) -> bool {
        if self.sh().tables.is_empty() {
            return false;
        }
        if let Some((id, col, r)) = self.totals_button() {
            if r.contains(p) {
                self.f10.total_menu = if self.f10.total_menu.is_some() { None } else { Some((id, col, r.left_bottom() + vec2(0.0, 2.0))) };
                return true;
            }
        }
        if let Some((i, col, r)) = self.inactive_table_buttons(g).into_iter().find(|(_, _, r)| r.expand(1.0).contains(p)) {
            // One filter per sheet: this table's becomes the active one.
            let range = self.sh().tables[i].filter_range();
            self.settle_edit();
            self.wb.set_filter(self.sheet, Some(AutoFilter::new(range)), "undo.filter");
            self.open_filter_popup(col, r.left_bottom() + vec2(0.0, 2.0));
            return true;
        }
        false
    }

    /// Ctrl+Shift+L in a table turns its filter buttons on or off.
    pub(super) fn toggle_table_filter(&mut self) -> bool {
        let Some(id) = self.cursor_table() else { return false };
        if !self.wb.table(id).is_some_and(|t| t.header) {
            return false;
        }
        self.edit_table(id, |t| t.filter_button = !t.filter_button);
        true
    }

    /// Whether `f` is a table's filter (tables grow it themselves).
    pub(super) fn is_table_filter(&self, f: &AutoFilter) -> bool {
        self.sh().tables.iter().any(|t| t.filter_range() == f.range)
    }

    /// Band fills of the tables in `visible` (under the cells' own fills).
    pub(super) fn paint_table_fills(&self, p: &egui::Painter, g: &Geo, visible: &Range) {
        let sh = self.sh();
        for t in &sh.tables {
            let Some(r) = t.range.intersect(visible) else { continue };
            if table::parse_style_name(&t.style.name).is_none() {
                continue;
            }
            for row in r.start.row..=r.end.row {
                for col in r.start.col..=r.end.col {
                    let c = CellRef::new(row, col);
                    let Some(fill) = t.look(c).and_then(|l| l.fill) else { continue };
                    if sh.get(c).is_some_and(|x| self.wb.styles.get(x.style).fill.is_some()) {
                        continue;
                    }
                    p.rect_filled(g.cell_rect(sh, c), 0.0, theme::rgb(fill));
                }
            }
        }
    }

    /// Lines of the table styles in `visible` (cells' own borders go on top).
    pub(super) fn paint_table_lines(&self, p: &egui::Painter, g: &Geo, visible: &Range) {
        let sh = self.sh();
        for t in &sh.tables {
            let Some(r) = t.range.intersect(visible) else { continue };
            if table::parse_style_name(&t.style.name).is_none() {
                continue;
            }
            for row in r.start.row..=r.end.row {
                for col in r.start.col..=r.end.col {
                    let c = CellRef::new(row, col);
                    let Some(l) = t.look(c) else { continue };
                    let rect = g.cell_rect(sh, c);
                    if rect.width() < 0.5 || rect.height() < 0.5 {
                        continue;
                    }
                    for (line, a, b) in [(l.top, rect.left_top(), rect.right_top()), (l.bottom, rect.left_bottom(), rect.right_bottom()), (l.left, rect.left_top(), rect.left_bottom()), (l.right, rect.right_top(), rect.right_bottom())] {
                        if let Some(line) = line {
                            paint_line(p, a, b, line, line.color.map_or(Color32::BLACK, theme::rgb), false);
                        }
                    }
                }
            }
        }
    }

    /// The text color and weight a table style gives cell `c` (None when
    /// the cell keeps its own style).
    pub(super) fn table_text_style(&self, c: CellRef, style: u32) -> Option<CellStyle> {
        let sh = self.sh();
        if sh.tables.is_empty() {
            return None;
        }
        let l = sh.table_at(c)?.look(c)?;
        if l.color.is_none() && !l.bold {
            return None;
        }
        let mut st = self.wb.styles.get(style).clone();
        if st.color.is_none() {
            st.color = l.color;
        }
        st.bold |= l.bold;
        Some(st)
    }

    /// The name box shows a table's name when the table is selected.
    pub(super) fn name_box_label(&self) -> String {
        let sel = self.sel();
        if let Some(t) = self.sh().tables.iter().find(|t| t.range == sel || t.data_range() == sel) {
            return t.name.clone();
        }
        if let Some(n) = self.wb.name_of_range(self.sheet, sel) {
            return n.to_string();
        }
        sel_label(&sel, self.wb.calc.r1c1)
    }

    /// Go To a table name: selects its data. True if `name` is a table.
    pub(super) fn goto_table(&mut self, name: &str) -> bool {
        let Some(id) = self.wb.find_table(name.trim()) else { return false };
        let Some(r) = self.wb.table(id).map(|t| t.data_range()) else { return false };
        if id.0 != self.sheet {
            self.select_sheet(id.0);
        }
        self.anchor = r.start;
        self.cursor = r.end;
        self.ensure_visible = true;
        true
    }

    /// Table names that complete the word being typed in a formula.
    pub(super) fn table_completions(&self, word: &str) -> Vec<String> {
        let w = word.to_lowercase();
        self.wb.tables().map(|(_, t)| t.name.clone()).filter(|n| n.to_lowercase().starts_with(&w) && n.len() > w.len()).collect()
    }

    /// After a cell edit: tables grow, rename columns and fill calculated
    /// columns (each its own undo step).
    pub(super) fn table_follow_up(&mut self, c: CellRef) {
        if !self.sh().tables.is_empty() {
            self.wb.table_after_input(self.sheet, c);
        }
    }
}

