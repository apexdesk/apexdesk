//! Table Layout tools: AutoFit, distributing rows and columns, cell
//! margins, text direction, splitting a table and formulas.

use apex_doc::{Field, FieldKind, TableProps, TextDir};

use super::table_ui::TableDialog;
use super::*;
use crate::toolstrip;
use crate::widgets::DialogResult;

/// The Formula dialog: the formula (with its "="), the number picture and
/// the field being edited (selected), if any.
pub struct FormulaDlg {
    pub formula: String,
    pub picture: String,
    pub edit: Option<Selection>,
}

/// Functions offered by the Formula dialog's "Paste function".
const FUNCTIONS: [&str; 15] = ["ABS", "AND", "AVERAGE", "COUNT", "IF", "INT", "MAX", "MIN", "MOD", "NOT", "OR", "PRODUCT", "ROUND", "SIGN", "SUM"];

/// Number pictures offered by the Formula dialog.
const PICTURES: [&str; 7] = ["#,##0", "#,##0.00", "0", "0.00", "0%", "0.00%", "#,##0.00 €"];

impl DocView {
    /// The Cell Size and Alignment groups of the Table Layout tab.
    pub(super) fn table_tools_group(&mut self, ui: &mut egui::Ui, a: Color32) {
        toolstrip::large_menu(ui, Icon::Columns, tr("table.autofit"), "", a, |ui| {
            if ui.button(tr("table.autofit_contents")).clicked() {
                self.autofit_contents(ui.ctx());
                ui.close();
            }
            if ui.button(tr("table.autofit_window")).clicked() {
                let w = self.cell_text_width();
                self.doc.fit_table_width(self.sel, w);
                self.refocus = true;
                ui.close();
            }
        });
        toolstrip::column(ui, |ui| {
            if toolstrip::small(ui, Some(Icon::InsertRows), tr("table.distribute_rows"), "", a, false).clicked() {
                self.distribute_rows();
            }
            // Off when the columns are all of one width already.
            let uneven = self.doc.columns_uneven(self.sel);
            if ui.add_enabled_ui(uneven, |ui| toolstrip::small(ui, Some(Icon::InsertCols), tr("table.distribute_cols"), "", a, false)).inner.clicked() {
                self.doc.distribute_columns(self.sel);
                self.refocus = true;
            }
        });
        toolstrip::column(ui, |ui| {
            toolstrip::small_menu(ui, Some(Icon::Orientation), tr("table.text_dir"), "", a, |ui| {
                let cur = self.cell_text_dir();
                for (d, key) in [(TextDir::Horizontal, "table.dir_horizontal"), (TextDir::Up, "table.dir_up"), (TextDir::Down, "table.dir_down")] {
                    if ui.add(egui::Button::new(tr(key)).selected(cur == d)).clicked() {
                        self.doc.set_cell_props(self.sel, &move |c: &mut apex_doc::CellProps, _| c.text_dir = d);
                        self.refocus = true;
                        ui.close();
                    }
                }
            });
            if toolstrip::small(ui, Some(Icon::Margins), &format!("{}…", tr("table.cell_margins")), "", a, false).clicked() {
                self.open_margins_dialog();
            }
        });
        toolstrip::column(ui, |ui| {
            if toolstrip::small(ui, Some(Icon::Table), tr("table.split_table"), "", a, false).clicked() {
                self.split_table();
            }
            if toolstrip::small(ui, Some(Icon::Function), &format!("{}…", tr("table.formula")), "", a, false).clicked() {
                self.open_formula_dialog();
            }
        });
    }

    /// The same commands as menu items (the Table menu).
    pub(super) fn table_tools_items(&mut self, ui: &mut egui::Ui) {
        crate::widgets::menu_button(ui, tr("table.autofit"), |ui| {
            if ui.button(tr("table.autofit_contents")).clicked() {
                self.autofit_contents(ui.ctx());
                ui.close();
            }
            if ui.button(tr("table.autofit_window")).clicked() {
                let w = self.cell_text_width();
                self.doc.fit_table_width(self.sel, w);
                self.refocus = true;
                ui.close();
            }
            if ui.button(tr("table.distribute_rows")).clicked() {
                self.distribute_rows();
                ui.close();
            }
            if ui.add_enabled(self.doc.columns_uneven(self.sel), egui::Button::new(tr("table.distribute_cols"))).clicked() {
                self.doc.distribute_columns(self.sel);
                self.refocus = true;
                ui.close();
            }
        });
        crate::widgets::menu_button(ui, tr("table.text_dir"), |ui| {
            let cur = self.cell_text_dir();
            for (d, key) in [(TextDir::Horizontal, "table.dir_horizontal"), (TextDir::Up, "table.dir_up"), (TextDir::Down, "table.dir_down")] {
                if ui.add(egui::Button::new(tr(key)).selected(cur == d)).clicked() {
                    self.doc.set_cell_props(self.sel, &move |c: &mut apex_doc::CellProps, _| c.text_dir = d);
                    self.refocus = true;
                    ui.close();
                }
            }
        });
        if ui.button(format!("{}…", tr("table.cell_margins"))).clicked() {
            self.open_margins_dialog();
            ui.close();
        }
        if ui.button(tr("table.split_table")).clicked() {
            self.split_table();
            ui.close();
        }
        if ui.button(format!("{}…", tr("table.formula"))).clicked() {
            self.open_formula_dialog();
            ui.close();
        }
    }

    fn cell_text_dir(&self) -> TextDir {
        let Some(b) = self.doc.cell_block(self.sel) else { return TextDir::Horizontal };
        self.doc.table_map(b.start).and_then(|m| m.cell_props(&self.doc.paragraphs, b.r0, b.c0).map(|c| c.text_dir)).unwrap_or_default()
    }

    /// Width the table at the caret can take: the page's text width, or
    /// the text width of the cell holding it (points).
    fn cell_text_width(&self) -> f32 {
        let pt = PT * self.zoom;
        let Some(m) = self.doc.table_map(self.sel.cursor.para) else { return self.doc.page.content_width() };
        if m.depth == 0 {
            return self.doc.page.content_width();
        }
        // The cell of the outer table that holds this one.
        let outer = self.layout.tables.iter().filter(|t| t.map.depth + 1 == m.depth && t.map.start <= m.start && m.end <= t.map.end).last();
        outer.map_or(self.doc.page.content_width(), |t| t.text_box(&self.doc.paragraphs, m.start, pt).1 / pt)
    }

    /// AutoFit to contents: each column as wide as its widest cell text
    /// (unwrapped), the whole shrunk to the width available.
    fn autofit_contents(&mut self, ctx: &egui::Context) {
        let Some(m) = self.doc.table_map(self.sel.cursor.para) else { return };
        let props = m.props(&self.doc.paragraphs).cloned().unwrap_or_default();
        let [_, ml, _, mr] = props.margins();
        let z = self.zoom;
        let pt = PT * z;
        let mut widths = vec![apex_doc::MIN_COL_WIDTH; m.cols];
        // Merged cells across columns only widen them when they don't fit.
        let mut spanned = Vec::new();
        for r in 0..m.rows {
            for c in 0..m.cols {
                if !m.is_anchor(r, c) {
                    continue;
                }
                let (a, b) = m.cell(r, c);
                let mut w: f32 = 0.0;
                for i in a..b {
                    if self.doc.table_depth(i) != Some(m.depth) {
                        continue;
                    }
                    let pl = self.layout_para(ctx, &self.doc.paragraphs[i], z, 100_000.0, None);
                    let text = pl.galley.rows.iter().map(|row| row.row.size.x).fold(0.0f32, f32::max);
                    w = w.max((pl.indent + text) / pt + 2.0);
                }
                let w = w + ml + mr;
                let cs = m.span_of(r, c).1;
                if cs == 1 {
                    widths[c] = widths[c].max(w);
                } else {
                    spanned.push((c, cs, w));
                }
            }
        }
        for (c, cs, w) in spanned {
            let have: f32 = widths[c..c + cs].iter().sum();
            if w > have {
                let extra = (w - have) / cs as f32;
                widths[c..c + cs].iter_mut().for_each(|x| *x += extra);
            }
        }
        let room = self.cell_text_width() - props.indent.max(0.0);
        let total: f32 = widths.iter().sum();
        if total > room && room > 0.0 {
            let k = room / total;
            widths.iter_mut().for_each(|w| *w = (*w * k).max(apex_doc::MIN_COL_WIDTH));
        }
        let widths: Vec<f32> = widths.iter().map(|w| (w * 10.0).round() / 10.0).collect();
        self.doc.set_column_widths(self.sel, &widths);
        self.refocus = true;
    }

    /// Distribute rows: the selected rows get the average of their heights
    /// as shown.
    fn distribute_rows(&mut self) {
        let para = self.sel.cursor.para;
        let Some(tb) = self.layout.table_at(para).filter(|_| self.doc.in_table(para)) else { return };
        let index = self.layout.owner.get(para).copied().unwrap_or(u32::MAX);
        let pt = PT * self.zoom;
        let mut heights = vec![0.0f32; tb.map.rows];
        for f in self.layout.frags.iter().filter(|f| f.table == index && !f.ghost) {
            if let Some(h) = heights.get_mut(f.row as usize) {
                *h += f.h / pt;
            }
        }
        self.doc.distribute_rows(self.sel, &heights);
        self.refocus = true;
    }

    fn split_table(&mut self) {
        let s = self.doc.split_table(self.sel);
        if let Some(s) = s {
            self.set_sel(s);
            self.desired_x = None;
        }
        self.refocus = true;
    }

    pub(super) fn open_sort_dialog(&mut self) {
        self.table_dialog = Some(TableDialog::Sort { col: 0, ascending: true, header: self.table_props().is_some_and(|p| p.header_rows > 0) });
    }

    pub(super) fn open_to_text_dialog(&mut self) {
        self.table_dialog = Some(TableDialog::Convert { to_table: false, sep: 0, other: String::new() });
    }

    fn open_margins_dialog(&mut self) {
        let Some(p) = self.table_props() else { return };
        self.table_dialog = Some(TableDialog::Margins(p.margins()));
    }

    /// The Formula dialog, for the formula field at the caret or a new one
    /// (suggesting a sum of the numbers above or to the left).
    pub(super) fn open_formula_dialog(&mut self) {
        let at = self.sel.start();
        // Or the field just before the caret.
        let before = self.doc.paragraphs.get(at.para).is_some_and(|p| p.text.get(..at.offset).is_some_and(|t| t.ends_with(apex_doc::OBJECT)));
        let field = self.doc.field_at(at).or_else(|| before.then(|| self.doc.field_at(Pos::new(at.para, at.offset - apex_doc::OBJECT.len_utf8()))).flatten());
        if let Some((s, f)) = field {
            if let FieldKind::Formula(e) = &f.kind {
                self.table_dialog = Some(TableDialog::Formula(Box::new(FormulaDlg { formula: format!("={e}"), picture: f.format.clone().unwrap_or_default(), edit: Some(s) })));
                return;
            }
        }
        let suggest = if !self.in_table() {
            String::from("=")
        } else if self.doc.formula_value(at, "COUNT(ABOVE)").is_ok_and(|n| n > 0.0) {
            String::from("=SUM(ABOVE)")
        } else if self.doc.formula_value(at, "COUNT(LEFT)").is_ok_and(|n| n > 0.0) {
            String::from("=SUM(LEFT)")
        } else {
            String::from("=SUM(ABOVE)")
        };
        self.table_dialog = Some(TableDialog::Formula(Box::new(FormulaDlg { formula: suggest, picture: String::new(), edit: None })));
    }

    /// Margins and Formula dialogs; `None` when `d` is another dialog.
    pub(super) fn tools_dialog(&self, ctx: &egui::Context, d: &mut TableDialog) -> Option<DialogResult> {
        let (unit, _, unit_name) = dialogs::ruler_unit();
        Some(match d {
            TableDialog::Margins(m) => widgets::dialog(ctx, "table_margins", tr("table.cell_margins"), 360.0, |ui| {
                widgets::section(ui, tr("table.default_margins").trim_end_matches(':'));
                egui::Grid::new("table_margins_grid").num_columns(4).spacing([12.0, 8.0]).show(ui, |ui| {
                    for (row, pair) in [(0, [(0, "table.m_top"), (2, "table.m_bottom")]), (1, [(1, "table.m_left"), (3, "table.m_right")])] {
                        let _ = row;
                        for (k, key) in pair {
                            ui.label(tr(key));
                            let mut u = m[k] / unit;
                            if ui.add(egui::DragValue::new(&mut u).range(0.0..=200.0 / unit).speed(0.02).max_decimals(2).suffix(format!(" {unit_name}"))).changed() {
                                m[k] = u * unit;
                            }
                        }
                        ui.end_row();
                    }
                });
            }),
            TableDialog::Formula(f) => widgets::dialog(ctx, "table_formula", tr("table.formula"), 440.0, |ui| {
                widgets::section(ui, tr("table.formula"));
                ui.add(egui::TextEdit::singleline(&mut f.formula).desired_width(f32::INFINITY));
                ui.add_space(4.0);
                ui.label(RichText::new(tr("table.formula_hint")).weak().small());
                widgets::section(ui, tr("table.sec_options"));
                widgets::form(ui, "table_formula_form", |ui| {
                    widgets::form_row(ui, tr("table.number_format").trim_end_matches(':'), |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.picture).desired_width(150.0));
                        egui::ComboBox::from_id_salt("formula_pic").selected_text("…").width(40.0).show_ui(ui, |ui| {
                            for p in PICTURES {
                                if ui.selectable_label(f.picture == p, p).clicked() {
                                    f.picture = p.to_string();
                                }
                            }
                        });
                    });
                    widgets::form_row(ui, tr("table.paste_function").trim_end_matches(':'), |ui| {
                        egui::ComboBox::from_id_salt("formula_fn").selected_text("…").width(90.0).show_ui(ui, |ui| {
                            for name in FUNCTIONS {
                                if ui.selectable_label(false, name).clicked() {
                                    if !f.formula.starts_with('=') {
                                        f.formula.insert(0, '=');
                                    }
                                    f.formula.push_str(&format!("{name}()"));
                                }
                            }
                        });
                    });
                });
                widgets::section(ui, tr("table.sec_result"));
                let expr = f.formula.trim().trim_start_matches('=').to_string();
                let preview = self.doc.formula_value(f.edit.map_or(self.sel.start(), |s| s.start()), &expr);
                match preview {
                    Ok(v) => ui.label(format!("= {}", apex_doc::format_number(v, Some(f.picture.as_str()).filter(|p| !p.is_empty()), common::decimal()))),
                    Err(()) => ui.label(RichText::new(tr("table.formula_error")).color(ui.visuals().error_fg_color)),
                };
            }),
            _ => return None,
        })
    }

    /// Applies the Margins or Formula dialog; false for other dialogs.
    pub(super) fn apply_tools_dialog(&mut self, d: &TableDialog) -> bool {
        match d {
            TableDialog::Margins(m) => {
                let m = *m;
                self.doc.set_table_props(self.sel, &move |p: &mut TableProps| p.cell_margins = Some(m));
            }
            TableDialog::Formula(f) => {
                let expr = f.formula.trim().trim_start_matches('=').trim();
                if expr.is_empty() {
                    return true;
                }
                let picture = f.picture.trim();
                let instr = if picture.is_empty() { format!("={expr}") } else { format!("={expr} \\# \"{picture}\"") };
                let Some(field) = Field::parse(&instr, "") else { return true };
                let s = self.doc.insert_field(f.edit.unwrap_or(self.sel), field);
                self.set_sel(Selection::caret(s.cursor));
                self.refocus = true;
            }
            _ => return false,
        }
        true
    }
}
