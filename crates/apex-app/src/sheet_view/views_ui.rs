//! View > Custom Views: show, add and delete named views of the workbook.

use apex_sheet::views::Shown;

use super::*;
use crate::widgets::DialogResult;

pub(super) struct ViewsDlg {
    selected: Option<usize>,
    /// The Add box: name, keep hidden rows/columns/filters, keep print
    /// settings.
    adding: Option<(String, bool, bool)>,
}

impl SheetView {
    pub(super) fn open_custom_views(&mut self) {
        self.settle_edit();
        let selected = (!self.wb.custom_views.is_empty()).then_some(0);
        self.views_dlg = Some(ViewsDlg { selected, adding: None });
    }

    /// The view shown now, for a new custom view.
    fn shown_now(&self) -> Shown {
        let sh = self.sh();
        let (fx, fy) = super::frozen_size(sh);
        let top_left = CellRef::new(sh.row_at(fy + self.scroll.y).max(sh.frozen_rows), sh.col_at(fx + self.scroll.x).max(sh.frozen_cols));
        Shown { sheet: self.sheet, zoom: self.zoom, top_left, cursor: self.cursor, selection: self.sel() }
    }

    pub(super) fn custom_views_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.views_dlg.take() else { return };
        if let Some((name, hidden, print)) = &mut d.adding {
            let mut ok = false;
            let res = widgets::dialog(ctx, "add_view", tr("views.add_title"), 320.0, |ui| {
                ui.label(tr("views.name"));
                let r = ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
                if !r.has_focus() && !r.lost_focus() {
                    r.request_focus();
                }
                ui.add_space(6.0);
                ui.label(tr("views.include"));
                ui.checkbox(print, tr("views.print"));
                ui.checkbox(hidden, tr("views.hidden"));
            });
            let mut cancel = false;
            match res {
                DialogResult::None => {}
                DialogResult::Cancel => cancel = true,
                DialogResult::Ok => ok = true,
            }
            if cancel {
                d.adding = None;
            } else if ok {
                let name = name.trim().to_string();
                if name.is_empty() {
                    self.notices.push(Notice::error(tr("views.no_name")));
                } else {
                    let (hidden, print) = (*hidden, *print);
                    self.wb.add_custom_view(&name, self.shown_now(), hidden, print);
                    // Adding closes the whole dialog, as in Excel.
                    self.focus_grid = true;
                    return;
                }
            }
            self.views_dlg = Some(d);
            return;
        }
        let mut action = None;
        let res = widgets::dialog(ctx, "custom_views", tr("views.title"), 380.0, |ui| {
            ui.label(tr("views.list"));
            egui::Frame::new().stroke(ui.visuals().widgets.noninteractive.bg_stroke).inner_margin(4.0).show(ui, |ui| {
                egui::ScrollArea::vertical().min_scrolled_height(140.0).max_height(180.0).auto_shrink([false, false]).show(ui, |ui| {
                    for (i, v) in self.wb.custom_views.iter().enumerate() {
                        let r = ui.selectable_label(d.selected == Some(i), &v.name);
                        if r.clicked() {
                            d.selected = Some(i);
                        }
                        if r.double_clicked() {
                            action = Some(1);
                        }
                    }
                });
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(d.selected.is_some(), egui::Button::new(tr("views.show"))).clicked() {
                    action = Some(1);
                }
                if ui.button(tr("views.add")).clicked() {
                    action = Some(2);
                }
                if ui.add_enabled(d.selected.is_some(), egui::Button::new(tr("views.delete"))).clicked() {
                    action = Some(3);
                }
            });
        });
        match (res, action) {
            (_, Some(2)) => d.adding = Some((String::new(), true, true)),
            (_, Some(3)) => {
                if let Some(i) = d.selected {
                    self.wb.delete_custom_view(i);
                    d.selected = (!self.wb.custom_views.is_empty()).then_some(i.min(self.wb.custom_views.len().saturating_sub(1)));
                }
            }
            (DialogResult::Ok, _) | (_, Some(1)) => {
                if let Some(i) = d.selected {
                    self.show_custom_view(i);
                }
                self.focus_grid = true;
                return;
            }
            (DialogResult::Cancel, _) => {
                self.focus_grid = true;
                return;
            }
            _ => {}
        }
        self.views_dlg = Some(d);
    }

    fn show_custom_view(&mut self, i: usize) {
        let Some(s) = self.wb.show_custom_view(i) else { return };
        self.select_sheet(s.sheet);
        self.zoom = s.zoom.clamp(0.1, 4.0);
        let sh = self.sh();
        let (fx, fy) = super::frozen_size(sh);
        self.scroll = vec2(sh.col_x(s.top_left.col) - fx, sh.row_y(s.top_left.row) - fy).max(Vec2::ZERO);
        // The active cell is a corner of the selection: anchor the other.
        let (r, c) = (s.selection, s.cursor);
        let row = if c.row == r.start.row { r.end.row } else { r.start.row };
        let col = if c.col == r.start.col { r.end.col } else { r.start.col };
        let corner = (c.row == r.start.row || c.row == r.end.row) && (c.col == r.start.col || c.col == r.end.col);
        self.cursor = c;
        self.anchor = if corner { CellRef::new(row, col) } else { c };
        self.buf_key = None;
    }
}
