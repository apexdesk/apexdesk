//! Table commands: the Table menu, the grid picker, the style gallery,
//! table dialogs and editing helpers (Tab, cell blocks, column resizing).

use apex_doc::{Border, Borders, Edges, TABLE_STYLES, Table, TableLook, TablePart, TableProps, TableStyle, TextSep, VAlign};

use super::*;
use crate::widgets::DialogResult;

/// A column edge being dragged (canvas x).
pub struct ColDrag {
    /// First paragraph of the table.
    pub table: usize,
    pub edge: usize,
    pub start: f32,
    pub x: f32,
}

#[derive(Clone, Copy, PartialEq)]
pub enum BorderPreset {
    All,
    Outside,
    Inside,
    InsideH,
    InsideV,
    Top,
    Bottom,
    Left,
    Right,
    None,
}

const BORDER_PRESETS: [(BorderPreset, &str); 10] = [
    (BorderPreset::Bottom, "table.border_bottom"),
    (BorderPreset::Top, "table.border_top"),
    (BorderPreset::Left, "table.border_left"),
    (BorderPreset::Right, "table.border_right"),
    (BorderPreset::None, "table.border_none"),
    (BorderPreset::All, "table.border_all"),
    (BorderPreset::Outside, "table.border_outside"),
    (BorderPreset::Inside, "table.border_inside"),
    (BorderPreset::InsideH, "table.border_inside_h"),
    (BorderPreset::InsideV, "table.border_inside_v"),
];

pub struct BordersDlg {
    /// Top, bottom, left, right, inside horizontal, inside vertical.
    sides: [bool; 6],
    width: f32,
    color: u32,
    shading: Option<u32>,
    whole_table: bool,
    page: usize,
}

pub struct PropsDlg {
    align: Align,
    indent: f32,
    /// Width of the selected columns and minimum height of the selected
    /// rows (0 = automatic), in points.
    col_width: f32,
    row_height: f32,
    header: bool,
    valign: VAlign,
    orig: (Align, f32, f32, f32, bool, VAlign),
    page: usize,
}

pub enum TableDialog {
    Insert { rows: usize, cols: usize },
    Split { rows: usize, cols: usize },
    Props(Box<PropsDlg>),
    Borders(Box<BordersDlg>),
    /// Text to table (`to_table`) or table to text; the separator is 0 tabs,
    /// 1 commas, 2 semicolons, 3 other, 4 paragraphs.
    Convert { to_table: bool, sep: u8, other: String },
    Sort { col: usize, ascending: bool, header: bool },
    /// Cell margins: top, left, bottom, right (points).
    Margins([f32; 4]),
    Formula(Box<super::table_tools_ui::FormulaDlg>),
}

fn sep_of(sep: u8, other: &str) -> TextSep {
    match sep {
        0 => TextSep::Tab,
        1 => TextSep::Comma,
        2 => TextSep::Semicolon,
        3 => other.chars().next().map_or(TextSep::Tab, TextSep::Char),
        _ => TextSep::Paragraph,
    }
}

/// A 10 x 8 grid to pick the size of a new table; returns (rows, cols).
pub(crate) fn grid_picker(ui: &mut egui::Ui) -> Option<(usize, usize)> {
    const COLS: usize = 10;
    const ROWS: usize = 8;
    let (cell, gap) = (15.0, 3.0);
    let id = ui.id().with("table_grid");
    let mut hover: (usize, usize) = ui.data(|d| d.get_temp(id)).unwrap_or((0, 0));
    let label = if hover.0 > 0 { trf("table.grid_size", &[("cols", &hover.1), ("rows", &hover.0)]) } else { tr("table.insert_table").to_string() };
    ui.label(&label);
    let (rect, resp) = ui.allocate_exact_size(vec2(COLS as f32 * (cell + gap), ROWS as f32 * (cell + gap)), Sense::click());
    crate::widgets::name(&resp, egui::WidgetType::Button, &label, None);
    if let Some(p) = resp.hover_pos() {
        let c = ((p.x - rect.min.x) / (cell + gap)).floor().max(0.0) as usize + 1;
        let r = ((p.y - rect.min.y) / (cell + gap)).floor().max(0.0) as usize + 1;
        hover = (r.min(ROWS), c.min(COLS));
    } else if !ui.rect_contains_pointer(rect.expand(12.0)) {
        hover = (0, 0);
    }
    ui.data_mut(|d| d.insert_temp(id, hover));
    let v = ui.visuals();
    for r in 0..ROWS {
        for c in 0..COLS {
            let rr = Rect::from_min_size(rect.min + vec2(c as f32 * (cell + gap), r as f32 * (cell + gap)), vec2(cell, cell));
            let on = r < hover.0 && c < hover.1;
            ui.painter().rect_filled(rr, 1.0, if on { v.selection.bg_fill.gamma_multiply(0.5) } else { v.extreme_bg_color });
            ui.painter().rect_stroke(rr, 1.0, Stroke::new(1.0, if on { v.selection.stroke.color } else { v.widgets.inactive.bg_stroke.color }), egui::StrokeKind::Inside);
        }
    }
    (resp.clicked() && hover.0 > 0).then_some(hover)
}

/// A small picture of a table style.
fn style_preview(ui: &mut egui::Ui, st: &TableStyle, look: TableLook, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(64.0, 46.0), Sense::click());
    crate::widgets::name(&resp, egui::WidgetType::SelectableLabel, &st.name, Some(selected));
    let p = ui.painter();
    let v = ui.visuals();
    if selected || resp.hovered() {
        p.rect_stroke(rect.expand(2.0), 3.0, Stroke::new(2.0, v.selection.stroke.color), egui::StrokeKind::Outside);
    }
    p.rect_filled(rect, 0.0, Paper::BG);
    let props = TableProps { look, ..Default::default() };
    let (rows, cols) = (5, 4);
    let (w, h) = ((rect.width() - 4.0) / cols as f32, (rect.height() - 4.0) / rows as f32);
    let cell = apex_doc::CellProps::default();
    let stroke = |b: Border| Stroke::new(if b.width > 1.0 { 1.5 } else { 1.0 }, theme::rgb(b.color));
    for r in 0..rows {
        for c in 0..cols {
            let l = props.cell_look_in(Some(st), &cell, r, c, rows, cols);
            let cr = Rect::from_min_size(rect.min + vec2(2.0 + c as f32 * w, 2.0 + r as f32 * h), vec2(w, h));
            if let Some(f) = l.fill {
                p.rect_filled(cr, 0.0, theme::rgb(f));
            }
            let ink = l.color.map_or(Color32::from_gray(if l.bold { 60 } else { 150 }), theme::rgb);
            p.hline(cr.min.x + 3.0..=cr.max.x - 3.0, cr.center().y, Stroke::new(if l.bold { 2.0 } else { 1.0 }, ink));
            for (b, a, e) in [(l.top, cr.left_top(), cr.right_top()), (l.bottom, cr.left_bottom(), cr.right_bottom()), (l.left, cr.left_top(), cr.left_bottom()), (l.right, cr.right_top(), cr.right_bottom())] {
                if !b.is_none() {
                    p.line_segment([a, e], stroke(b));
                }
            }
        }
    }
    resp
}

/// New or Modify Table Style: the table style being made (a new one has
/// no id yet).
pub(super) struct TableStyleDlg {
    st: TableStyle,
    name: String,
    /// Line for the borders the style draws.
    line: Border,
    /// Outer, inside horizontal and inside vertical lines.
    lines: [bool; 3],
    error: bool,
}

impl DocView {
    fn open_table_style(&mut self, st: TableStyle) {
        let b = st.borders;
        let line = [b.top, b.inside_h, b.left, b.inside_v, b.bottom].into_iter().flatten().find(|l| !l.is_none()).unwrap_or(Border::THIN);
        let on = |l: Option<Border>| l.is_some_and(|l| !l.is_none());
        let lines = [on(b.top) || on(b.left), on(b.inside_h), on(b.inside_v)];
        let name = if st.id.is_empty() {
            let names: Vec<String> = self.doc.styles.tables.iter().map(|t| t.name.to_string()).collect();
            (1..).map(|n| format!("{} {n}", tr("table.style_new_name"))).find(|n| !names.contains(n)).unwrap_or_default()
        } else {
            st.name.to_string()
        };
        self.table_style_dlg = Some(Box::new(TableStyleDlg { st, name, line, lines, error: false }));
    }

    pub(super) fn table_style_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.table_style_dlg.take() else { return };
        let title = if d.st.id.is_empty() { tr("table.style_new_title") } else { tr("table.style_modify_title") };
        let res = widgets::dialog(ctx, "table_style_dlg", title, 420.0, |ui| {
            table_style_page(ui, &mut d);
            ui.add_space(6.0);
            let st = styled_table(&d);
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(vec2(widgets::FORM_LABEL_W, 20.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.set_min_width(widgets::FORM_LABEL_W);
                    ui.label(tr("style.preview_label"));
                });
                style_preview(ui, &st, TableLook::default(), false);
            });
            if d.error {
                ui.colored_label(ui.visuals().error_fg_color, tr("style.bad_name"));
            }
        });
        match res {
            DialogResult::None => self.table_style_dlg = Some(d),
            DialogResult::Cancel => {}
            DialogResult::Ok => {
                let name = d.name.trim().to_string();
                let clash = self.doc.styles.tables.iter().any(|t| *t.name == *name && t.id != d.st.id) || TABLE_STYLES.iter().any(|t| t.name.eq_ignore_ascii_case(&name) || style_label(t).eq_ignore_ascii_case(&name));
                if name.is_empty() || clash {
                    d.error = true;
                    self.table_style_dlg = Some(d);
                    return;
                }
                let mut st = styled_table(&d);
                st.name = name.clone().into();
                let new = st.id.is_empty();
                if new {
                    st.id = self.doc.styles.new_table_id(&name).into();
                }
                let id = st.id.to_string();
                self.doc.set_table_style(st, self.sel);
                if new {
                    self.doc.set_table_props(self.sel, &move |p: &mut TableProps| p.style = Some(id.as_str().into()));
                }
                self.refocus = true;
            }
        }
    }
}

/// The style the dialog shows.
fn styled_table(d: &TableStyleDlg) -> TableStyle {
    let mut st = d.st.clone();
    let l = Some(d.line);
    let [outer, h, v] = d.lines;
    st.borders = Borders { top: outer.then_some(d.line), bottom: outer.then_some(d.line), left: outer.then_some(d.line), right: outer.then_some(d.line), inside_h: if h { l } else { None }, inside_v: if v { l } else { None } };
    st
}

fn table_style_page(ui: &mut egui::Ui, d: &mut TableStyleDlg) {
    let color_row = |ui: &mut egui::Ui, label: &str, c: &mut Option<u32>, none: &str| {
        ui.label(label);
        let chip = match *c {
            Some(v) => RichText::new("■■").color(theme::rgb(v)),
            None => RichText::new(none),
        };
        crate::widgets::menu_button(ui, chip, |ui| {
            if let Some(v) = widgets::color_menu(ui, none, *c) {
                *c = v;
                ui.close();
            }
        });
        ui.end_row();
    };
    egui::Grid::new("table_style_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        ui.label(tr("style.name"));
        ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(200.0));
        ui.end_row();
        ui.label(tr("style.based_on"));
        egui::ComboBox::from_id_salt("table_style_base").width(200.0).selected_text(tr("table.style_pick_base")).show_ui(ui, |ui| {
            for b in TABLE_STYLES.iter() {
                if ui.selectable_label(false, style_label(b)).clicked() {
                    d.st = TableStyle { id: d.st.id.clone(), name: d.st.name.clone(), ..b.clone() };
                    let bl = b.borders;
                    d.line = [bl.top, bl.inside_h, bl.left, bl.inside_v].into_iter().flatten().find(|l| !l.is_none()).unwrap_or(Border::THIN);
                    let on = |l: Option<Border>| l.is_some_and(|l| !l.is_none());
                    d.lines = [on(bl.top) || on(bl.left), on(bl.inside_h), on(bl.inside_v)];
                }
            }
        });
        ui.end_row();
        ui.label(RichText::new(tr("table.borders")).strong());
        ui.end_row();
        ui.label("");
        ui.horizontal(|ui| {
            ui.checkbox(&mut d.lines[0], tr("border.outer"));
            ui.checkbox(&mut d.lines[1], tr("table.style_inside_h"));
            ui.checkbox(&mut d.lines[2], tr("table.style_inside_v"));
        });
        ui.end_row();
        let mut c = Some(d.line.color);
        color_row(ui, tr("border.line_color"), &mut c, tr("format.auto_color"));
        d.line.color = c.unwrap_or(0);
        widgets::number_row(ui, tr("table.style_width"), &mut d.line.width, 0.25..=6.0, " pt");
        ui.label(RichText::new(tr("table.look_header")).strong());
        ui.end_row();
        color_row(ui, tr("table.style_header_fill"), &mut d.st.header_fill, tr("table.no_color"));
        color_row(ui, tr("table.style_header_color"), &mut d.st.header_color, tr("format.auto_color"));
        ui.label("");
        ui.horizontal(|ui| {
            ui.checkbox(&mut d.st.header_bold, tr("format.bold"));
            let mut line = d.st.header_line.is_some();
            if ui.checkbox(&mut line, tr("table.style_header_line")).changed() {
                d.st.header_line = line.then_some(Border { width: 1.5, ..d.line });
            }
        });
        ui.end_row();
        ui.label(RichText::new(tr("table.look_banded_rows")).strong());
        ui.end_row();
        color_row(ui, tr("table.style_band_fill"), &mut d.st.band_fill, tr("table.no_color"));
        ui.label("");
        ui.checkbox(&mut d.st.first_col_bold, tr("table.style_first_col_bold"));
        ui.end_row();
    });
}

fn style_label(st: &TableStyle) -> String {
    crate::i18n::lookup(&format!("table.style.{}", st.id)).unwrap_or(&st.name).to_string()
}

impl DocView {
    pub(super) fn table_props(&self) -> Option<TableProps> {
        let m = self.layout.table_at(self.sel.cursor.para).map(|t| t.map.clone()).or_else(|| self.doc.table_map(self.sel.cursor.para))?;
        Some(m.props(&self.doc.paragraphs).cloned().unwrap_or_default())
    }

    pub(super) fn in_table(&self) -> bool {
        self.doc.in_table(self.sel.cursor.para)
    }

    fn table_done(&mut self, s: Option<Selection>) {
        if let Some(s) = s {
            self.set_sel(s);
            self.desired_x = None;
        }
        self.refocus = true;
    }

    /// The width a new table at the caret has room for, in points: in a
    /// cell, the cell's text width.
    pub(super) fn table_room(&self) -> f32 {
        let pt = PT * self.zoom;
        let para = self.sel.cursor.para;
        self.layout.table_at(para).filter(|_| self.doc.in_table(para)).map_or(self.doc.page.content_width(), |tb| tb.text_box(&self.doc.paragraphs, para, pt).1 / pt)
    }

    pub(super) fn insert_table(&mut self, rows: usize, cols: usize) {
        let width = self.table_room();
        let s = self.doc.insert_table(self.sel, rows, cols, width);
        self.table_done(Some(s));
    }

    /// Clears the cells of a selection spanning several cells; true if it
    /// did.
    pub(super) fn clear_block(&mut self) -> bool {
        if self.doc.block_selection(self.sel).is_none() {
            return false;
        }
        if let Some(p) = self.doc.clear_cells(self.sel) {
            self.sel = Selection::caret(p);
        }
        true
    }

    /// Tab and Shift+Tab in a table: the next or previous cell; Tab in the
    /// last cell adds a row.
    pub(super) fn tab_in_table(&mut self, back: bool) {
        let para = self.sel.cursor.para;
        match self.doc.next_cell(para, back) {
            Some(s) => self.set_sel(s),
            None if !back => {
                if let Some(s) = self.doc.table_insert_rows(Selection::caret(self.sel.cursor), true) {
                    // The caret goes to the first cell of the new row.
                    let first = self.doc.table_map(s.cursor.para).map(|m| {
                        let (r, _) = m.pos_of(&self.doc.paragraphs, s.cursor.para);
                        m.cell(r, 0).0
                    });
                    self.set_sel(Selection::caret(Pos::new(first.unwrap_or(s.cursor.para), 0)));
                }
            }
            None => {}
        }
        self.desired_x = None;
    }

    /// Pastes tab-separated text or copied cells into the cells from the
    /// caret's; false when the paste isn't for cells.
    pub(super) fn paste_cells(&mut self, text: &str, clip: Option<&[Paragraph]>) -> bool {
        if !self.in_table() || self.doc.container(self.sel.anchor.para).map(|c| c.0) != self.doc.container(self.sel.cursor.para).map(|c| c.0) {
            return false;
        }
        let grid: Vec<Vec<Vec<Paragraph>>> = match clip {
            Some(ps) if !ps.is_empty() && ps.iter().all(|p| p.cell.is_some()) => Table::from_paras(ps.to_vec()).rows.into_iter().map(|r| r.into_iter().map(|c| if c.props.covered { Vec::new() } else { c.paras }).collect()).collect(),
            Some(_) => return false,
            None if text.contains('\t') => {
                let text = text.strip_suffix('\n').unwrap_or(text);
                text.split('\n').map(|l| l.split('\t').map(|f| vec![Paragraph::new(f)]).collect()).collect()
            }
            None => return false,
        };
        let s = self.doc.fill_cells(self.sel, &grid);
        self.table_done(s);
        true
    }

    /// Column edge under a canvas point: the table's first paragraph and
    /// the edge.
    pub(super) fn edge_at(&self, p: Pos2) -> Option<(usize, usize)> {
        let l = &self.layout;
        if l.tables.is_empty() {
            return None;
        }
        let page = l.page_at(p.x - self.page_x, p.y);
        // Columns of a vertical page's tables run across it: no dragging.
        if l.geo_of(page).vertical {
            return None;
        }
        let o = self.content_origin(page);
        l.edge_hit(page, p.x - o.x, p.y - o.y, 3.0)
    }

    /// Applies a column drag (canvas x moved from `start` to `x`).
    pub(super) fn finish_col_drag(&mut self, d: ColDrag) {
        let pt = PT * self.zoom;
        let delta = (d.x - d.start) / pt;
        let Some(tb) = self.layout.table_at(d.table) else { return };
        // Widths as shown (files may leave some out).
        let shown: Vec<f32> = tb.x.windows(2).map(|w| ((w[1] - w[0]) / pt * 10.0).round() / 10.0).collect();
        let sel = Selection::caret(Pos::new(d.table, 0));
        let cur = self.table_props().filter(|_| self.doc.in_table(d.table)).map(|p| p.widths);
        if cur.as_ref().is_some_and(|w| w.len() != shown.len() || w.iter().any(|v| *v <= 0.0)) {
            self.doc.set_table_props(sel, &move |p: &mut TableProps| p.widths = shown.clone());
        }
        let delta = (delta * 10.0).round() / 10.0;
        self.doc.resize_column(d.table, d.edge, delta, self.sel);
    }

    fn apply_borders(&mut self, preset: BorderPreset, b: Border) {
        let set = move |cp: &mut apex_doc::CellProps, e: Edges| {
            let bd = &mut cp.borders;
            let v = Some(b);
            let none = Some(Border::NONE);
            match preset {
                BorderPreset::All => (bd.top, bd.bottom, bd.left, bd.right) = (v, v, v, v),
                BorderPreset::None => (bd.top, bd.bottom, bd.left, bd.right) = (none, none, none, none),
                BorderPreset::Outside => {
                    for (on, side) in [(e.top, &mut bd.top), (e.bottom, &mut bd.bottom), (e.left, &mut bd.left), (e.right, &mut bd.right)] {
                        if on {
                            *side = v;
                        }
                    }
                }
                BorderPreset::Inside | BorderPreset::InsideH | BorderPreset::InsideV => {
                    let h = preset != BorderPreset::InsideV;
                    let w = preset != BorderPreset::InsideH;
                    for (on, side) in [(h && !e.top, &mut bd.top), (h && !e.bottom, &mut bd.bottom), (w && !e.left, &mut bd.left), (w && !e.right, &mut bd.right)] {
                        if on {
                            *side = v;
                        }
                    }
                }
                BorderPreset::Top if e.top => bd.top = v,
                BorderPreset::Bottom if e.bottom => bd.bottom = v,
                BorderPreset::Left if e.left => bd.left = v,
                BorderPreset::Right if e.right => bd.right = v,
                _ => {}
            }
        };
        self.doc.set_cell_props(self.sel, &set);
        self.refocus = true;
    }

    fn set_shading(&mut self, c: Option<u32>) {
        self.doc.set_cell_props(self.sel, &move |cp: &mut apex_doc::CellProps, _| cp.shading = c);
        self.refocus = true;
    }

    pub(super) fn open_props_dialog(&mut self) {
        let Some(p) = self.table_props() else { return };
        let Some(b) = self.doc.cell_block(self.sel) else { return };
        let col_width = p.widths.get(b.c0).copied().unwrap_or(0.0);
        let row_height = p.row_heights.get(b.r0).copied().unwrap_or(0.0);
        let header = p.header_rows > 0;
        let valign = self.doc.table_map(b.start).and_then(|m| m.cell_props(&self.doc.paragraphs, b.r0, b.c0).map(|c| c.valign)).unwrap_or_default();
        let orig = (p.align, p.indent, col_width, row_height, header, valign);
        self.table_dialog = Some(TableDialog::Props(Box::new(PropsDlg { align: p.align, indent: p.indent, col_width, row_height, header, valign, orig, page: 0 })));
    }

    fn open_borders_dialog(&mut self) {
        let cp = self.doc.cell_block(self.sel).and_then(|b| self.doc.table_map(b.start).and_then(|m| m.cell_props(&self.doc.paragraphs, b.r0, b.c0).cloned()));
        let shading = cp.and_then(|c| c.shading);
        self.table_dialog = Some(TableDialog::Borders(Box::new(BordersDlg { sides: [true; 6], width: 0.5, color: 0, shading, whole_table: false, page: 0 })));
    }

    pub(super) fn toggle_header(&mut self) {
        let Some(b) = self.doc.cell_block(self.sel) else { return };
        let rows = (b.r1 + 1) as u32;
        self.doc.set_table_props(self.sel, &move |p: &mut TableProps| p.header_rows = if p.header_rows > 0 { 0 } else { rows });
        self.refocus = true;
    }

    /// The table style gallery and the style options.
    pub(super) fn style_menu(&mut self, ui: &mut egui::Ui) {
        let Some(props) = self.table_props() else { return };
        let look = props.look;
        let cur = props.style.clone();
        ui.label(RichText::new(tr("table.styles")).strong());
        let mut pick: Option<Option<String>> = None;
        let mut edit: Option<Option<TableStyle>> = None;
        let mut delete = None;
        let customs = self.doc.styles.tables.clone();
        egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
            egui::Grid::new("table_styles").spacing(vec2(8.0, 8.0)).show(ui, |ui| {
                if ui.add_sized(vec2(64.0, 46.0), egui::Button::new(tr("table.style_none")).selected(cur.is_none())).clicked() {
                    pick = Some(None);
                }
                let all = customs.iter().map(|st| (st, true)).chain(TABLE_STYLES.iter().map(|st| (st, false)));
                for (i, (st, own)) in all.enumerate() {
                    let r = style_preview(ui, st, look, cur.as_deref() == Some(&*st.id)).on_hover_text(style_label(st));
                    if r.clicked() {
                        pick = Some(Some(st.id.to_string()));
                    }
                    r.context_menu(|ui| {
                        if own && ui.button(format!("{}…", tr("style.modify"))).clicked() {
                            edit = Some(Some(st.clone()));
                            ui.close();
                        }
                        if ui.button(format!("{}…", tr("table.style_new_from"))).clicked() {
                            edit = Some(Some(TableStyle { id: "".into(), ..st.clone() }));
                            ui.close();
                        }
                        if own && ui.button(tr("style.delete")).clicked() {
                            delete = Some(st.id.to_string());
                            ui.close();
                        }
                    });
                    if i % 3 == 1 {
                        ui.end_row();
                    }
                }
            });
        });
        if ui.button(format!("{}…", tr("table.style_new"))).clicked() {
            let base = props.style_def(&self.doc.styles).cloned();
            edit = Some(Some(TableStyle { id: "".into(), ..base.unwrap_or(apex_doc::PLAIN) }));
        }
        if let Some(Some(st)) = edit {
            self.open_table_style(st);
            ui.close();
        }
        if let Some(id) = delete {
            self.doc.delete_table_style(&id, self.sel);
            ui.close();
        }
        if let Some(s) = pick {
            self.doc.set_table_props(self.sel, &move |p: &mut TableProps| p.style = s.as_deref().map(Into::into));
            self.refocus = true;
            ui.close();
        }
        ui.separator();
        ui.label(RichText::new(tr("table.style_options")).strong());
        let mut l = look;
        egui::Grid::new("table_look").show(ui, |ui| {
            ui.checkbox(&mut l.first_row, tr("table.look_header"));
            ui.checkbox(&mut l.first_col, tr("table.look_first_col"));
            ui.end_row();
            ui.checkbox(&mut l.last_row, tr("table.look_total"));
            ui.checkbox(&mut l.last_col, tr("table.look_last_col"));
            ui.end_row();
            ui.checkbox(&mut l.banded_rows, tr("table.look_banded_rows"));
            ui.checkbox(&mut l.banded_cols, tr("table.look_banded_cols"));
            ui.end_row();
        });
        if l != look {
            self.doc.set_table_props(self.sel, &move |p: &mut TableProps| p.look = l);
        }
    }

    pub(super) fn borders_menu(&mut self, ui: &mut egui::Ui) {
        for (preset, key) in BORDER_PRESETS {
            if ui.button(tr(key)).clicked() {
                self.apply_borders(preset, Border::THIN);
                ui.close();
            }
        }
        ui.separator();
        if ui.button(format!("{}…", tr("table.borders_shading"))).clicked() {
            self.open_borders_dialog();
            ui.close();
        }
    }

    pub(super) fn insert_table_items(&mut self, ui: &mut egui::Ui) {
        if let Some((r, c)) = grid_picker(ui) {
            self.insert_table(r, c);
            ui.close();
        }
        ui.separator();
        if ui.button(format!("{}…", tr("table.insert_table"))).clicked() {
            self.table_dialog = Some(TableDialog::Insert { rows: 2, cols: 5 });
            ui.close();
        }
        if !self.in_table() && ui.add_enabled(!self.sel.is_empty(), egui::Button::new(format!("{}…", tr("table.text_to_table")))).clicked() {
            self.table_dialog = Some(TableDialog::Convert { to_table: true, sep: 0, other: String::new() });
            ui.close();
        }
    }

    /// Row and column commands (the Table menu and the context menu).
    fn table_edit_items(&mut self, ui: &mut egui::Ui) {
        let sel = self.sel;
        let width = self.doc.page.content_width();
        crate::widgets::menu_button(ui, tr("table.insert"), |ui| {
            let mut s = None;
            if ui.button(tr("table.rows_above")).clicked() {
                s = Some(self.doc.table_insert_rows(sel, false));
            }
            if ui.button(tr("table.rows_below")).clicked() {
                s = Some(self.doc.table_insert_rows(sel, true));
            }
            if ui.button(tr("table.cols_left")).clicked() {
                s = Some(self.doc.table_insert_cols(sel, false, width));
            }
            if ui.button(tr("table.cols_right")).clicked() {
                s = Some(self.doc.table_insert_cols(sel, true, width));
            }
            if let Some(s) = s {
                self.table_done(s);
                ui.close();
            }
        });
        crate::widgets::menu_button(ui, tr("table.delete"), |ui| {
            let mut s = None;
            if ui.button(tr("table.delete_rows")).clicked() {
                s = Some(self.doc.table_delete_rows(sel));
            }
            if ui.button(tr("table.delete_cols")).clicked() {
                s = Some(self.doc.table_delete_cols(sel));
            }
            if ui.button(tr("table.delete_table")).clicked() {
                s = Some(self.doc.delete_table(sel));
            }
            if let Some(s) = s {
                self.table_done(s);
                ui.close();
            }
        });
        crate::widgets::menu_button(ui, tr("table.select"), |ui| {
            for (part, key) in [(TablePart::Cell, "table.select_cell"), (TablePart::Row, "table.select_row"), (TablePart::Column, "table.select_col"), (TablePart::Table, "table.select_table")] {
                if ui.button(tr(key)).clicked() {
                    let s = self.doc.select_table_part(sel, part);
                    self.table_done(s);
                    ui.close();
                }
            }
        });
        ui.separator();
        let block = self.doc.block_selection(sel);
        if ui.add_enabled(block.is_some(), egui::Button::new(tr("table.merge"))).clicked() {
            let s = self.doc.merge_cells(sel);
            self.table_done(s);
            ui.close();
        }
        if ui.button(format!("{}…", tr("table.split"))).clicked() {
            self.table_dialog = Some(TableDialog::Split { rows: 1, cols: 2 });
            ui.close();
        }
    }

    /// Rows & Columns and Merge groups of the Table Layout tab.
    pub(super) fn table_toolstrip_edit(&mut self, ui: &mut egui::Ui, a: Color32) {
        use crate::toolstrip;
        let sel = self.sel;
        let width = self.doc.page.content_width();
        let mut del = None;
        toolstrip::large_menu(ui, Icon::DeleteRows, tr("table.delete"), "", a, |ui| {
            for (icon, key, what) in [(Icon::DeleteRows, "table.delete_rows", 0), (Icon::DeleteCols, "table.delete_cols", 1), (Icon::Table, "table.delete_table", 2)] {
                if toolstrip::menu_item(ui, icon, tr(key)).clicked() {
                    del = Some(what);
                    ui.close();
                }
            }
        });
        let done = match del {
            Some(0) => Some(self.doc.table_delete_rows(sel)),
            Some(1) => Some(self.doc.table_delete_cols(sel)),
            Some(_) => Some(self.doc.delete_table(sel)),
            None => None,
        };
        if let Some(d) = done {
            self.table_done(d);
        }
        toolstrip::column(ui, |ui| {
            let mut done = None;
            if toolstrip::small(ui, Some(Icon::InsertRows), tr("table.rows_above"), "", a, false).clicked() {
                done = Some(self.doc.table_insert_rows(sel, false));
            }
            if toolstrip::small(ui, Some(Icon::InsertRows), tr("table.rows_below"), "", a, false).clicked() {
                done = Some(self.doc.table_insert_rows(sel, true));
            }
            toolstrip::row(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::InsertCols), tr("table.cols_left"), "", a, false).clicked() {
                    done = Some(self.doc.table_insert_cols(sel, false, width));
                }
                if toolstrip::small(ui, Some(Icon::InsertCols), tr("table.cols_right"), "", a, false).clicked() {
                    done = Some(self.doc.table_insert_cols(sel, true, width));
                }
            });
            if let Some(d) = done {
                self.table_done(d);
            }
        });
        toolstrip::column(ui, |ui| {
            let block = self.doc.block_selection(sel);
            ui.add_enabled_ui(block.is_some(), |ui| {
                if toolstrip::small(ui, Some(Icon::Merge), tr("table.merge"), "", a, false).clicked() {
                    let d = self.doc.merge_cells(sel);
                    self.table_done(d);
                }
            });
            if toolstrip::small(ui, Some(Icon::Columns), &format!("{}…", tr("table.split")), "", a, false).clicked() {
                self.table_dialog = Some(TableDialog::Split { rows: 1, cols: 2 });
            }
            toolstrip::small_menu(ui, Some(Icon::Select), tr("table.select"), "", a, |ui| {
                for (part, key) in [(TablePart::Cell, "table.select_cell"), (TablePart::Row, "table.select_row"), (TablePart::Column, "table.select_col"), (TablePart::Table, "table.select_table")] {
                    if ui.button(tr(key)).clicked() {
                        let d = self.doc.select_table_part(sel, part);
                        self.table_done(d);
                        ui.close();
                    }
                }
            });
        });
    }

    /// The Table menu of the menu bar.
    pub fn table_menu(&mut self, ui: &mut egui::Ui) {
        crate::widgets::menu_button(ui, tr("table.insert_table"), |ui| self.insert_table_items(ui));
        if !self.in_table() {
            return;
        }
        ui.separator();
        self.table_edit_items(ui);
        ui.separator();
        crate::widgets::menu_button(ui, tr("table.styles"), |ui| self.style_menu(ui));
        crate::widgets::menu_button(ui, tr("table.borders"), |ui| self.borders_menu(ui));
        crate::widgets::menu_button(ui, tr("table.shading"), |ui| {
            if let Some(c) = widgets::color_menu(ui, tr("table.no_color"), None) {
                self.set_shading(c);
                ui.close();
            }
        });
        let header = self.table_props().is_some_and(|p| p.header_rows > 0);
        if ui.add(egui::Button::new(tr("table.repeat_header")).selected(header)).clicked() {
            self.toggle_header();
            ui.close();
        }
        ui.separator();
        self.table_tools_items(ui);
        ui.separator();
        if ui.button(format!("{}…", tr("table.sort"))).clicked() {
            self.open_sort_dialog();
            ui.close();
        }
        if ui.button(format!("{}…", tr("table.table_to_text"))).clicked() {
            self.open_to_text_dialog();
            ui.close();
        }
        if ui.button(format!("{}…", tr("table.properties"))).clicked() {
            self.open_props_dialog();
            ui.close();
        }
    }

    /// Table section of the context menu (in a table).
    pub(super) fn table_context_items(&mut self, ui: &mut egui::Ui) {
        if !self.in_table() {
            return;
        }
        self.table_edit_items(ui);
        crate::widgets::menu_button(ui, tr("table.styles"), |ui| self.style_menu(ui));
        crate::widgets::menu_button(ui, tr("table.borders"), |ui| self.borders_menu(ui));
        if ui.button(format!("{}…", tr("table.properties"))).clicked() {
            self.open_props_dialog();
            ui.close();
        }
        ui.separator();
    }

    pub(super) fn table_dialogs(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.table_dialog.take() else { return };
        let (unit, _, unit_name) = dialogs::ruler_unit();
        let header_texts: Vec<String> = match &d {
            TableDialog::Sort { .. } => self.doc.table_map(self.sel.cursor.para).map_or(Vec::new(), |m| {
                (0..m.cols).map(|c| {
                    let (a, b) = m.cell(0, c);
                    (a..b).map(|i| self.doc.paragraphs[i].text.as_str()).collect::<Vec<_>>().join(" ")
                }).collect()
            }),
            _ => Vec::new(),
        };
        let tools = self.tools_dialog(ctx, &mut d);
        let res = match &mut d {
            TableDialog::Margins(_) | TableDialog::Formula(_) => tools.unwrap_or(DialogResult::None),
            TableDialog::Insert { rows, cols } => size_dialog(ctx, tr("table.insert_table"), rows, cols, 1000),
            TableDialog::Split { rows, cols } => size_dialog(ctx, tr("table.split"), rows, cols, 100),
            TableDialog::Props(p) => {
                let pages = [(Icon::Table, "table.table"), (Icon::InsertRows, "table.row"), (Icon::InsertCols, "table.column"), (Icon::CellFormat, "table.cell")];
                let mut page = p.page;
                let r = widgets::nav_dialog(ctx, "table_props", tr("table.properties"), &pages, &mut page, egui::vec2(400.0, 280.0), |ui, page| props_page(ui, p, unit, unit_name, page));
                p.page = page;
                r
            }
            TableDialog::Borders(b) => {
                let pages = [(Icon::Borders, "table.borders"), (Icon::Shading, "table.shading")];
                let mut page = b.page;
                let r = widgets::nav_dialog(ctx, "table_borders", tr("table.borders_shading"), &pages, &mut page, egui::vec2(400.0, 330.0), |ui, page| borders_page(ui, b, page));
                b.page = page;
                r
            }
            TableDialog::Convert { to_table, sep, other } => {
                let title = if *to_table { tr("table.text_to_table") } else { tr("table.table_to_text") };
                let to_table = *to_table;
                widgets::dialog(ctx, "table_convert", title, 320.0, |ui| {
                    widgets::section(ui, tr("table.separate_at"));
                    if !to_table {
                        ui.radio_value(sep, 4, tr("table.sep_paragraphs"));
                    }
                    ui.radio_value(sep, 0, tr("table.sep_tabs"));
                    ui.radio_value(sep, 1, tr("table.sep_commas"));
                    ui.radio_value(sep, 2, tr("table.sep_semicolons"));
                    ui.horizontal(|ui| {
                        ui.radio_value(sep, 3, tr("table.sep_other"));
                        if ui.add(egui::TextEdit::singleline(other).char_limit(1).desired_width(24.0)).changed() {
                            *sep = 3;
                        }
                    });
                })
            }
            TableDialog::Sort { col, ascending, header } => widgets::dialog(ctx, "table_sort", tr("table.sort"), 380.0, |ui| {
                let name = |c: usize| match header_texts.get(c).filter(|t| *header && !t.trim().is_empty()) {
                    Some(t) => t.clone(),
                    None => trf("table.column_n", &[("n", &(c + 1))]),
                };
                widgets::section(ui, tr("table.sort_by"));
                egui::ComboBox::from_id_salt("table_sort_col").width(220.0).selected_text(name(*col)).show_ui(ui, |ui| {
                    for c in 0..header_texts.len() {
                        ui.selectable_value(col, c, name(c));
                    }
                });
                widgets::section(ui, tr("table.sec_order"));
                ui.radio_value(ascending, true, tr("table.ascending"));
                ui.radio_value(ascending, false, tr("table.descending"));
                widgets::section(ui, tr("table.sec_options"));
                ui.checkbox(header, tr("table.has_header"));
            }),
        };
        match res {
            DialogResult::None => self.table_dialog = Some(d),
            DialogResult::Cancel => self.focus = true,
            DialogResult::Ok => {
                self.focus = true;
                self.apply_table_dialog(d);
            }
        }
    }

    fn apply_table_dialog(&mut self, d: TableDialog) {
        if self.apply_tools_dialog(&d) {
            return;
        }
        let sel = self.sel;
        match d {
            TableDialog::Insert { rows, cols } => self.insert_table(rows, cols),
            TableDialog::Split { rows, cols } => {
                let s = self.doc.split_cell(sel, rows, cols);
                self.table_done(s);
            }
            TableDialog::Props(p) => {
                let Some(b) = self.doc.cell_block(sel) else { return };
                let (align, indent, col_width, row_height, header, valign) = (p.align, p.indent, p.col_width, p.row_height, p.header, p.valign);
                let o = p.orig;
                let changed_w = (col_width - o.2).abs() > 0.01 && col_width > 0.0;
                let changed_h = (row_height - o.3).abs() > 0.01;
                let cols = self.layout.table_at(b.start).map_or(0, |t| t.map.cols);
                self.doc.set_table_props(sel, &move |t: &mut TableProps| {
                    t.align = align;
                    t.indent = if align == Align::Left { indent } else { 0.0 };
                    if changed_w {
                        if t.widths.len() < cols {
                            let avg = if t.widths.is_empty() { 72.0 } else { t.width() / t.widths.len() as f32 };
                            t.widths.resize(cols, avg);
                        }
                        for c in b.c0..=b.c1.min(t.widths.len().saturating_sub(1)) {
                            t.widths[c] = col_width.max(apex_doc::MIN_COL_WIDTH);
                        }
                    }
                    if changed_h {
                        if t.row_heights.len() <= b.r1 {
                            t.row_heights.resize(b.r1 + 1, 0.0);
                        }
                        for r in b.r0..=b.r1 {
                            t.row_heights[r] = row_height.max(0.0);
                        }
                    }
                    if header != o.4 {
                        t.header_rows = if header { (b.r1 + 1) as u32 } else { 0 };
                    }
                });
                if valign != o.5 {
                    self.doc.set_cell_props(sel, &move |c: &mut apex_doc::CellProps, _| c.valign = valign);
                }
            }
            TableDialog::Borders(b) => {
                let line = Border::new(b.width, b.color);
                let side = |on: bool| Some(if on { line } else { Border::NONE });
                let [top, bottom, left, right, ih, iv] = b.sides.map(side);
                let target = if b.whole_table { self.doc.select_table_part(sel, TablePart::Table).unwrap_or(sel) } else { sel };
                if b.whole_table {
                    let bs = Borders { top, bottom, left, right, inside_h: ih, inside_v: iv };
                    self.doc.set_table_props(sel, &move |t: &mut TableProps| t.borders = bs);
                    // The table's borders show through: cell borders go.
                    self.doc.set_cell_props(target, &move |c: &mut apex_doc::CellProps, _| c.borders = Borders::default());
                } else {
                    self.doc.set_cell_props(target, &move |c: &mut apex_doc::CellProps, e: Edges| {
                        c.borders.top = if e.top { top } else { ih };
                        c.borders.bottom = if e.bottom { bottom } else { ih };
                        c.borders.left = if e.left { left } else { iv };
                        c.borders.right = if e.right { right } else { iv };
                    });
                }
                let shading = b.shading;
                self.doc.set_cell_props(target, &move |c: &mut apex_doc::CellProps, _| c.shading = shading);
            }
            TableDialog::Convert { to_table: true, sep, other } => {
                let s = self.doc.text_to_table(sel, sep_of(sep, &other), self.doc.page.content_width());
                self.table_done(s);
            }
            TableDialog::Convert { to_table: false, sep, other } => {
                let s = self.doc.table_to_text(sel, sep_of(sep, &other));
                self.table_done(s);
            }
            TableDialog::Margins(_) | TableDialog::Formula(_) => {}
            TableDialog::Sort { col, ascending, header } => {
                if !self.doc.sort_table(sel, col, ascending, header) {
                    self.notices.push(Notice::info(tr("table.sort_merged")));
                }
                self.refocus = true;
            }
        }
    }
}

fn size_dialog(ctx: &egui::Context, title: &str, rows: &mut usize, cols: &mut usize, max_rows: usize) -> DialogResult {
    widgets::dialog(ctx, "table_size", title, 340.0, |ui| {
        widgets::section(ui, tr("table.sec_size"));
        widgets::form(ui, "table_size_grid", |ui| {
            widgets::form_row(ui, tr("table.num_cols").trim_end_matches(':'), |ui| ui.add(egui::DragValue::new(cols).range(1..=apex_doc::MAX_COLS).speed(0.2)));
            widgets::form_row(ui, tr("table.num_rows").trim_end_matches(':'), |ui| ui.add(egui::DragValue::new(rows).range(1..=max_rows).speed(0.2)));
        });
    })
}

fn props_page(ui: &mut egui::Ui, p: &mut PropsDlg, unit: f32, unit_name: &str, page: usize) {
    let len = |ui: &mut egui::Ui, v: &mut f32, min: f32| {
        let mut u = *v / unit;
        if ui.add(egui::DragValue::new(&mut u).range(min / unit..=2000.0 / unit).speed(0.02).max_decimals(2).suffix(format!(" {unit_name}"))).changed() {
            *v = u * unit;
        }
    };
    match page {
        0 => {
            widgets::section(ui, tr("table.table"));
            widgets::form(ui, "table_props_t", |ui| {
                widgets::form_row(ui, tr("table.alignment").trim_end_matches(':'), |ui| {
                    ui.selectable_value(&mut p.align, Align::Left, tr("format.align_left"));
                    ui.selectable_value(&mut p.align, Align::Center, tr("format.align_center"));
                    ui.selectable_value(&mut p.align, Align::Right, tr("format.align_right"));
                });
                if p.align == Align::Left {
                    widgets::form_row(ui, tr("table.indent").trim_end_matches(':'), |ui| len(ui, &mut p.indent, -500.0));
                }
            });
        }
        1 => {
            widgets::section(ui, tr("table.sec_size"));
            widgets::form(ui, "table_props_r", |ui| {
                widgets::form_row(ui, tr("table.row_height").trim_end_matches(':'), |ui| len(ui, &mut p.row_height, 0.0));
            });
            widgets::section(ui, tr("table.sec_options"));
            ui.checkbox(&mut p.header, tr("table.repeat_header"));
            ui.add_space(6.0);
            ui.label(RichText::new(tr("table.row_height_hint")).small().weak());
        }
        2 => {
            widgets::section(ui, tr("table.sec_size"));
            widgets::form(ui, "table_props_c", |ui| {
                widgets::form_row(ui, tr("table.col_width").trim_end_matches(':'), |ui| len(ui, &mut p.col_width, apex_doc::MIN_COL_WIDTH));
            });
        }
        _ => {
            widgets::section(ui, tr("table.valign").trim_end_matches(':'));
            ui.radio_value(&mut p.valign, VAlign::Top, tr("table.valign_top"));
            ui.radio_value(&mut p.valign, VAlign::Center, tr("table.valign_center"));
            ui.radio_value(&mut p.valign, VAlign::Bottom, tr("table.valign_bottom"));
        }
    }
}

fn borders_page(ui: &mut egui::Ui, b: &mut BordersDlg, page: usize) {
    if page == 0 {
        widgets::section(ui, tr("table.sec_setting"));
        ui.horizontal(|ui| {
            if ui.button(tr("table.preset_none")).clicked() {
                b.sides = [false; 6];
            }
            if ui.button(tr("table.preset_box")).clicked() {
                b.sides = [true, true, true, true, false, false];
            }
            if ui.button(tr("table.preset_all")).clicked() {
                b.sides = [true; 6];
            }
        });
        widgets::section(ui, tr("table.sec_sides"));
        egui::Grid::new("table_sides").num_columns(2).min_col_width(widgets::FORM_LABEL_W).show(ui, |ui| {
            let [t, bo, l, r, ih, iv] = &mut b.sides;
            ui.checkbox(t, tr("table.border_top"));
            ui.checkbox(bo, tr("table.border_bottom"));
            ui.end_row();
            ui.checkbox(l, tr("table.border_left"));
            ui.checkbox(r, tr("table.border_right"));
            ui.end_row();
            ui.checkbox(ih, tr("table.border_inside_h"));
            ui.checkbox(iv, tr("table.border_inside_v"));
            ui.end_row();
        });
        widgets::section(ui, tr("table.sec_line"));
        widgets::form(ui, "table_line", |ui| {
            widgets::form_row(ui, tr("table.line_width").trim_end_matches(':'), |ui| {
                egui::ComboBox::from_id_salt("table_line_w").selected_text(format!("{} pt", b.width)).show_ui(ui, |ui| {
                    for w in [0.25f32, 0.5, 0.75, 1.0, 1.5, 2.25, 3.0, 4.5, 6.0] {
                        ui.selectable_value(&mut b.width, w, format!("{w} pt"));
                    }
                });
            });
            widgets::form_row(ui, tr("table.line_color").trim_end_matches(':'), |ui| {
                crate::widgets::menu_button(ui, RichText::new("■").color(theme::rgb(b.color)), |ui| {
                    if let Some(c) = widgets::color_menu(ui, tr("color.automatic"), Some(b.color)) {
                        b.color = c.unwrap_or(0);
                        ui.close();
                    }
                });
            });
        });
    } else {
        widgets::section(ui, tr("table.sec_fill"));
        widgets::form(ui, "table_fill", |ui| {
            widgets::form_row(ui, tr("table.shading").trim_end_matches(':'), |ui| {
                let chip = match b.shading {
                    Some(c) => RichText::new("■").color(theme::rgb(c)),
                    None => RichText::new(tr("table.no_color")),
                };
                crate::widgets::menu_button(ui, chip, |ui| {
                    if let Some(c) = widgets::color_menu(ui, tr("table.no_color"), b.shading) {
                        b.shading = c;
                        ui.close();
                    }
                });
            });
        });
    }
    widgets::section(ui, tr("table.apply_to").trim_end_matches(':'));
    ui.radio_value(&mut b.whole_table, false, tr("table.apply_cells"));
    ui.radio_value(&mut b.whole_table, true, tr("table.apply_table"));
}
