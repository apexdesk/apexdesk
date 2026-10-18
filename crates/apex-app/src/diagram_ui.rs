//! Diagrams (what Office calls SmartArt) everywhere: the Insert > Diagram
//! gallery, their drawing, and the dialog that edits one in a document
//! or a workbook (slides have the Diagram Design tab instead).

use std::sync::Arc;

use apex_doc::{Diagram, EmbedKind, Embedded};
use apex_io::diagram_edit::{self, Item, Kind};
use eframe::egui::{self, Painter, Rect, Sense, vec2};

use crate::i18n::tr;
use crate::slides_view::diagram_edit::{COLOR_KEYS, STYLE_KEYS};

/// Paints the drawing of `d` scaled to `rect`.
pub fn paint(p: &Painter, rect: Rect, d: &Diagram) {
    let (sx, sy) = (rect.width() / d.size.0.max(1.0), rect.height() / d.size.1.max(1.0));
    for sh in &d.shapes {
        let r = Rect::from_min_size(rect.min + vec2(sh.x * sx, sh.y * sy), vec2(sh.w * sx, sh.h * sy));
        crate::shapes::paint(p, r, &sh.shape, sx, true);
    }
}

/// Sample items of a new diagram of family `kind`.
fn sample_items(kind: Kind) -> Vec<Item> {
    let t = tr("diagram.sample");
    let levels: &[usize] = match kind {
        Kind::Hierarchy | Kind::OrgChart => &[0, 1, 1],
        Kind::Matrix => &[0, 0, 0, 0],
        _ => &[0, 0, 0],
    };
    levels.iter().map(|&l| Item::new(l, t)).collect()
}

/// A new diagram of family `kind` in a `size` box (points), with sample
/// text to type over.
pub fn new_diagram(kind: Kind, size: (f32, f32)) -> Diagram {
    diagram_edit::new_diagram(kind, &sample_items(kind), size)
}

const TILE: egui::Vec2 = vec2(96.0, 60.0);

/// The families to pick from, each drawn; `current` is marked. Closes
/// the menu it is in when one is picked.
pub fn gallery(ui: &mut egui::Ui, current: Option<Kind>) -> Option<Kind> {
    if crate::toolstrip::collecting() {
        return None;
    }
    let mut pick = None;
    egui::Grid::new("diagram_gallery").num_columns(4).spacing([8.0, 8.0]).show(ui, |ui| {
        for (i, kind) in diagram_edit::KINDS.into_iter().enumerate() {
            let sample = ui.ctx().data_mut(|m| {
                let id = egui::Id::new(("diagram_sample", kind.key()));
                let items: Vec<Item> = sample_items(kind).into_iter().map(|i| Item::new(i.level, "")).collect();
                m.get_temp_mut_or_insert_with(id, || Arc::new(diagram_edit::new_diagram(kind, &items, (TILE.x * 2.0, TILE.y * 2.0)))).clone()
            });
            let r = ui
                .vertical(|ui| {
                    let (rect, resp) = ui.allocate_exact_size(TILE, Sense::click());
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tr(kind.key())));
                    let v = ui.visuals();
                    let on = current == Some(kind);
                    let bg = if resp.hovered() || on { v.widgets.hovered.weak_bg_fill } else { v.extreme_bg_color };
                    let p = ui.painter_at(rect);
                    p.rect_filled(rect, 3.0, bg);
                    if on {
                        p.rect_stroke(rect, 3.0, v.selection.stroke, egui::StrokeKind::Inside);
                    }
                    paint(&p, rect.shrink(4.0), &sample);
                    ui.add(egui::Label::new(egui::RichText::new(tr(kind.key())).small()).truncate());
                    resp
                })
                .inner;
            if r.on_hover_text(tr(kind.key())).clicked() {
                pick = Some(kind);
            }
            if i % 4 == 3 {
                ui.end_row();
            }
        }
    });
    if pick.is_some() {
        ui.close();
    }
    pick
}

/// The dialog editing a diagram of a document or workbook: its layout,
/// colors, style and text, shown as it will be.
pub struct Editor {
    /// The object as opened, to find it again.
    pub object: Arc<Embedded>,
    d: Diagram,
    text: String,
    /// Why the text typed can't be shown (a translation key).
    note: Option<&'static str>,
    /// Just opened: the text takes the keys, all selected to type over.
    fresh: bool,
}

impl Editor {
    /// None when `object` isn't a diagram.
    pub fn new(object: Arc<Embedded>) -> Option<Editor> {
        let d = object.diagram()?.clone();
        let text = diagram_edit::info(&d).map(|i| diagram_edit::outline_text(&i.items)).unwrap_or_default();
        Some(Editor { object, d, text, note: None, fresh: true })
    }

    /// Shows the dialog: `Some(Some(object))` to put back, `Some(None)`
    /// cancelled, `None` still open.
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<Option<Embedded>> {
        let res = crate::widgets::dialog(ctx, "diagram_editor", tr("diagram.edit_title"), 560.0, |ui| self.body(ui));
        match res {
            crate::widgets::DialogResult::Ok => Some(Some(Embedded { id: self.object.id, kind: EmbedKind::Diagram(self.d.clone()) })),
            crate::widgets::DialogResult::Cancel => Some(None),
            crate::widgets::DialogResult::None => None,
        }
    }

    fn body(&mut self, ui: &mut egui::Ui) {
        let Some(info) = diagram_edit::info(&self.d) else {
            ui.label(tr("object.diagram"));
            return;
        };
        let free = info.kind.drawn() && !info.locked;
        ui.horizontal(|ui| {
            ui.label(tr("diagram.layout"));
            let mut pick = None;
            crate::widgets::menu_button(ui, tr(info.kind.key()), |ui| pick = gallery(ui, Some(info.kind)));
            if let Some(k) = pick {
                match diagram_edit::set_kind(&self.d, k) {
                    Some(d) => {
                        self.d = d;
                        self.note = None;
                    }
                    None => self.note = Some("slides.diagram.locked"),
                }
            }
            ui.add_enabled_ui(free, |ui| {
                ui.label(tr("slides.diagram.colors"));
                if let Some(c) = combo(ui, "diagram_colors", &info.colors, &diagram_edit::COLORS, &COLOR_KEYS)
                    && let Some(d) = diagram_edit::set_colors(&self.d, c)
                {
                    self.d = d;
                }
                ui.label(tr("slides.diagram.style"));
                if let Some(s) = combo(ui, "diagram_style", &info.style, &diagram_edit::STYLES, &STYLE_KEYS)
                    && let Some(d) = diagram_edit::set_style(&self.d, s)
                {
                    self.d = d;
                }
            });
        });
        if !info.kind.drawn() {
            ui.weak(tr("diagram.convert_tip"));
        }
        ui.add_space(4.0);
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(220.0);
                ui.label(tr("slides.diagram.text_pane_help"));
                let out = egui::TextEdit::multiline(&mut self.text).desired_rows(10).desired_width(f32::INFINITY).lock_focus(true).code_editor().show(ui);
                let r = out.response;
                ui.data_mut(|m| m.insert_temp(r.id.with("multiline"), true));
                if self.fresh {
                    // A dialog's first frame takes no focus: until it holds.
                    r.request_focus();
                    self.fresh = !r.has_focus();
                    let mut state = out.state;
                    let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(self.text.chars().count()));
                    state.cursor.set_char_range(Some(all));
                    state.store(ui.ctx(), r.id);
                }
                if r.changed() {
                    match diagram_edit::set_items(&self.d, &diagram_edit::parse_outline(&self.text)) {
                        Some(d) => {
                            self.d = d;
                            self.note = None;
                        }
                        None => self.note = Some(if info.locked { "slides.diagram.locked" } else { "slides.diagram.text_only" }),
                    }
                }
                if let Some(n) = self.note {
                    ui.colored_label(ui.visuals().warn_fg_color, tr(n));
                }
            });
            let w = ui.available_width().max(160.0);
            let (w0, h0) = (self.d.size.0.max(1.0), self.d.size.1.max(1.0));
            let k = (w / w0).min(220.0 / h0);
            let (rect, _) = ui.allocate_exact_size(vec2(w0 * k, h0 * k), Sense::hover());
            ui.painter().rect_filled(rect, 2.0, egui::Color32::WHITE);
            paint(&ui.painter_at(rect), rect, &self.d);
        });
    }
}

/// A choice of `ids`, shown by the names `keys`; the one picked.
fn combo(ui: &mut egui::Ui, id: &str, cur: &str, ids: &[&'static str], keys: &[&'static str]) -> Option<&'static str> {
    let mut pick = None;
    let shown = ids.iter().position(|i| *i == cur).map_or_else(String::new, |i| tr(keys[i]).to_string());
    egui::ComboBox::from_id_salt(id).selected_text(shown).show_ui(ui, |ui| {
        for (i, key) in ids.iter().zip(keys) {
            if ui.selectable_label(*i == cur, tr(key)).clicked() {
                pick = Some(*i);
            }
        }
    });
    pick
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_draw_every_family() {
        for kind in diagram_edit::KINDS {
            let d = new_diagram(kind, (200.0, 120.0));
            assert!(!d.shapes.is_empty(), "{kind:?}");
            assert_eq!(diagram_edit::info(&d).unwrap().kind, kind);
        }
    }

    #[test]
    fn editor_changes_layout_and_text() {
        let obj = Arc::new(Embedded::new(EmbedKind::Diagram(new_diagram(Kind::Process, (200.0, 120.0)))));
        let mut e = Editor::new(obj).unwrap();
        e.d = diagram_edit::set_kind(&e.d, Kind::Pyramid).unwrap();
        e.d = diagram_edit::set_items(&e.d, &diagram_edit::parse_outline("A\nB")).unwrap();
        let i = diagram_edit::info(&e.d).unwrap();
        assert_eq!(i.kind, Kind::Pyramid);
        assert_eq!(i.items.len(), 2);
    }
}
