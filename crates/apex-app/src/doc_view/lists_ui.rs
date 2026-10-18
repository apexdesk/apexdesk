//! Home > Multilevel List: the gallery, Define New Multilevel List, Set
//! Numbering Value and numbered headings; and the markers of list items
//! and numbered headings.

use apex_doc::{LEVELS, ListDef, Marker, NumStyle};

use super::*;
use crate::widgets::DialogResult;

#[derive(Default)]
pub(super) struct ListState {
    /// Markers by paragraph, for the document revision they were made for.
    markers: std::cell::RefCell<(u64, Vec<Option<Marker>>)>,
    dialog: Option<ListDlg>,
}

enum ListDlg {
    Define { def: ListDef, level: usize, headings: bool },
    Value { new_list: bool, value: u32 },
}

fn style_key(s: NumStyle) -> &'static str {
    match s {
        NumStyle::Decimal => "lists.decimal",
        NumStyle::DecimalZero => "lists.decimal_zero",
        NumStyle::LowerLetter => "lists.lower_letter",
        NumStyle::UpperLetter => "lists.upper_letter",
        NumStyle::LowerRoman => "lists.lower_roman",
        NumStyle::UpperRoman => "lists.upper_roman",
        NumStyle::Bullet => "lists.bullet",
        NumStyle::None => "lists.none",
    }
}

/// The first items of levels 1..3 of `def`, as the gallery shows them.
fn sample(def: &ListDef) -> Vec<(usize, Marker)> {
    let mut c = [1u32; LEVELS];
    for (l, lv) in def.levels.iter().enumerate() {
        c[l] = lv.start;
    }
    (0..3).map(|l| (l, def.marker(l, &c))).collect()
}

fn paint_sample(ui: &egui::Ui, r: Rect, def: &ListDef) {
    let v = ui.visuals();
    let p = ui.painter();
    let font = egui::FontId::proportional(10.0);
    for (i, (l, m)) in sample(def).into_iter().enumerate() {
        let y = r.min.y + 10.0 + i as f32 * 16.0;
        let x = r.min.x + 6.0 + l as f32 * 12.0;
        let end = match m {
            Marker::Bullet(c) => {
                super::paint_bullet(p, pos2(x + 3.0, y), 2.5, c, v.text_color(), 1.0);
                x + 10.0
            }
            Marker::Text(t) => p.text(pos2(x, y), egui::Align2::LEFT_CENTER, t, font.clone(), v.text_color()).max.x + 4.0,
        };
        p.line_segment([pos2(end, y), pos2(r.max.x - 6.0, y)], Stroke::new(1.5, v.text_color().gamma_multiply(0.35)));
    }
}

impl DocView {
    /// The marker of paragraph `i` (list items and numbered headings).
    pub(super) fn marker(&self, i: usize) -> Option<Marker> {
        let mut m = self.lists.markers.borrow_mut();
        if m.0 != self.doc.revision || m.1.len() != self.doc.paragraphs.len() {
            *m = (self.doc.revision, self.doc.list_markers());
        }
        m.1.get(i).cloned().flatten()
    }

    pub(super) fn multilevel_menu(&mut self, ui: &mut egui::Ui) {
        let cur = self.cur_para().style;
        let current = self.doc.list_def_of(self.sel.start().para).cloned();
        let heading_now = self.doc.styles.list(self.doc.styles.heading_list).cloned();
        let mut pick: Option<(ListDef, bool)> = None;
        let tile = |ui: &mut egui::Ui, def: &ListDef, on: bool| {
            let (r, resp) = ui.allocate_exact_size(vec2(96.0, 56.0), Sense::click());
            let v = ui.visuals();
            ui.painter().rect(r, 3.0, if on { v.selection.bg_fill.gamma_multiply(0.35) } else if resp.hovered() { v.widgets.hovered.weak_bg_fill } else { v.extreme_bg_color }, v.widgets.noninteractive.bg_stroke, egui::StrokeKind::Inside);
            paint_sample(ui, r, def);
            resp.clicked()
        };
        ui.label(RichText::new(tr("lists.library")).small().strong());
        let presets = ListDef::presets();
        ui.horizontal(|ui| {
            for d in &presets[..4] {
                if tile(ui, d, cur.kind.is_list() && current.as_ref() == Some(d)) {
                    pick = Some((d.clone(), false));
                }
            }
        });
        ui.add_space(4.0);
        ui.label(RichText::new(tr("lists.headings")).small().strong());
        ui.horizontal(|ui| {
            for d in &presets[4..] {
                if tile(ui, d, heading_now.as_ref() == Some(d)) {
                    pick = Some((d.clone(), true));
                }
            }
        });
        if let Some((def, headings)) = pick {
            ui.close();
            self.use_list(&def, headings);
            return;
        }
        ui.separator();
        if ui.add_enabled(heading_now.is_some(), egui::Button::new(tr("lists.no_headings"))).clicked() {
            ui.close();
            self.doc.set_heading_numbering(None, self.sel);
        }
        if ui.button(format!("{}…", tr("lists.define"))).clicked() {
            ui.close();
            let headings = matches!(cur.kind, ParaKind::Heading(_));
            let def = current.unwrap_or_else(|| if headings { ListDef::headings() } else { ListDef::outline() });
            let level = if let ParaKind::Heading(h) = cur.kind { h as usize - 1 } else if cur.kind.is_list() { cur.level as usize } else { 0 };
            self.lists.dialog = Some(ListDlg::Define { def, level, headings });
        }
        if ui.add_enabled(cur.kind.is_list(), egui::Button::new(format!("{}…", tr("lists.set_value")))).clicked() {
            ui.close();
            let value = match self.marker(self.sel.start().para) {
                Some(Marker::Text(t)) => t.trim_end_matches(['.', ')']).rsplit('.').next().and_then(|n| n.parse().ok()).unwrap_or(1),
                _ => 1,
            };
            self.lists.dialog = Some(ListDlg::Value { new_list: true, value });
        }
    }

    fn use_list(&mut self, def: &ListDef, headings: bool) {
        if headings {
            self.doc.set_heading_numbering(Some(def), self.sel);
        } else {
            let (a, b) = self.para_range();
            self.doc.apply_list(a, b, self.sel, def);
        }
        self.focus = true;
    }

    pub(super) fn list_dialogs(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.lists.dialog.take() else { return };
        let res = match &mut d {
            ListDlg::Define { def, level, headings } => widgets::dialog(ctx, "doc_multilevel", tr("lists.define"), 460.0, |ui| define_ui(ui, def, level, headings)),
            ListDlg::Value { new_list, value } => widgets::dialog(ctx, "doc_list_value", tr("lists.set_value"), 300.0, |ui| {
                ui.radio_value(new_list, true, tr("lists.new_list"));
                ui.radio_value(new_list, false, tr("lists.continue"));
                ui.add_space(6.0);
                ui.add_enabled_ui(*new_list, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(tr("lists.value"));
                        ui.add(egui::DragValue::new(value).range(1..=32767));
                    });
                });
            }),
        };
        match res {
            DialogResult::None => self.lists.dialog = Some(d),
            DialogResult::Cancel => self.focus = true,
            DialogResult::Ok => match d {
                ListDlg::Define { def, headings, .. } => self.use_list(&def, headings),
                ListDlg::Value { new_list, value } => {
                    let para = self.sel.start().para;
                    self.doc.set_list_value(para, self.sel, new_list.then_some(value));
                    self.focus = true;
                }
            },
        }
    }
}

fn define_ui(ui: &mut egui::Ui, def: &mut ListDef, level: &mut usize, headings: &mut bool) {
    ui.horizontal_top(|ui| {
        let levels = ui.vertical(|ui| {
            ui.label(tr("lists.level"));
            for l in 0..LEVELS {
                if ui.selectable_label(*level == l, (l + 1).to_string()).clicked() {
                    *level = l;
                }
            }
        });
        // As tall as the levels: a separator takes all the height offered,
        // and a dialog offers the height it had, so it grew every frame.
        let rect = levels.response.rect;
        let (r, _) = ui.allocate_exact_size(vec2(ui.spacing().item_spacing.x, rect.height()), Sense::hover());
        ui.painter().vline(r.center().x, rect.y_range(), ui.visuals().widgets.noninteractive.bg_stroke);
        ui.vertical(|ui| {
            let lv = &mut def.levels[*level];
            egui::Grid::new("list_level").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.label(tr("lists.style"));
                let before = lv.style;
                egui::ComboBox::from_id_salt("list_style").selected_text(tr(style_key(lv.style))).width(180.0).show_ui(ui, |ui| {
                    for s in NumStyle::ALL {
                        ui.selectable_value(&mut lv.style, s, tr(style_key(s)));
                    }
                });
                if lv.style != before {
                    // Switching to or from bullets changes the label too.
                    if lv.style == NumStyle::Bullet {
                        lv.text = apex_doc::bullet_char(0, *level as u8).to_string();
                    } else if before == NumStyle::Bullet {
                        lv.text = format!("%{}.", *level + 1);
                    }
                }
                ui.end_row();
                ui.label(tr("lists.format"));
                ui.add(egui::TextEdit::singleline(&mut lv.text).desired_width(180.0));
                ui.end_row();
                ui.label("");
                ui.label(RichText::new(trf("lists.format_tip", &[("n", &(*level + 1))])).small().weak());
                ui.end_row();
                ui.label(tr("lists.start"));
                ui.add(egui::DragValue::new(&mut lv.start).range(0..=32767));
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut lv.legal, tr("lists.legal"));
                ui.end_row();
            });
            ui.add_space(6.0);
            ui.checkbox(headings, tr("lists.link_headings"));
            ui.add_space(6.0);
            ui.label(RichText::new(tr("lists.preview")).small().strong());
            let (r, _) = ui.allocate_exact_size(vec2(260.0, 56.0), Sense::hover());
            ui.painter().rect_filled(r, 3.0, ui.visuals().extreme_bg_color);
            paint_sample(ui, r, def);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Define New Multilevel List dialog keeps its size from frame to
    /// frame (it grew until it scrolled, moving what the pointer was on).
    #[test]
    fn define_dialog_keeps_its_size() {
        let ctx = egui::Context::default();
        let (mut def, mut level, mut headings) = (ListDef::outline(), 0, false);
        let mut heights = Vec::new();
        for _ in 0..8 {
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1280.0, 2000.0))), ..Default::default() };
            ctx.run_ui(input, |ui| {
                crate::widgets::dialog(ui.ctx(), "t", "t", 460.0, |ui| {
                    define_ui(ui, &mut def, &mut level, &mut headings);
                    heights.push(ui.min_rect().height());
                });
            }).textures_delta.clear();
        }
        assert!(heights[3..].windows(2).all(|w| w[0] == w[1]), "{heights:?}");
    }
}
