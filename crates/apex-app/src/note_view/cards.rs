//! The pages of a section as cards: a title, the start of the text, the
//! date and the subpages, in a grid to see them at a glance.

use std::sync::Arc;

use apex_doc::notebook::PageKind;
use apex_doc::notebook::canvas::{Canvas, ItemKind};
use eframe::egui::Color32;

use super::panels::Drop;
use super::*;

const CARD_W: f32 = 230.0;
const CARD_H: f32 = 150.0;
const GAP: f32 = 14.0;

/// What a card shows.
struct Card {
    id: i64,
    title: String,
    text: String,
    modified: i64,
    subpages: usize,
    color: u32,
    /// What a canvas holds, drawn small.
    canvas: Option<Arc<Canvas>>,
    /// The first picture of a text page.
    picture: Option<Arc<[u8]>>,
}

impl NoteView {
    /// Shows the pages as cards (or as the list).
    pub fn set_cards(&mut self, on: bool) {
        self.flush();
        self.close_graph();
        self.cards = on;
        self.grid = on;
        let r = self.store.set_setting("cards", if on { "1" } else { "0" });
        self.report(r);
    }

    /// The cards to show: what the search found, or the section's pages
    /// (the top ones; their subpages are counted on them).
    fn cards(&self) -> Vec<Card> {
        let card = |id: i64| {
            let p = self.nb.page(id)?;
            let text: String = self.nb.texts.get(&id).map_or("", String::as_str).chars().take(400).collect();
            Some(Card {
                id,
                title: p.title.clone(),
                text,
                modified: p.modified,
                subpages: self.nb.subtree(id).iter().filter(|&&s| s != id && self.nb.is_live(s)).count(),
                color: self.nb.section(p.section).map_or(0, |s| s.color),
                canvas: self.previews.get(&id).map(|c| c.1.clone()),
                picture: self.pictures.get(&id).and_then(|p| p.1.clone()),
            })
        };
        if !self.search.trim().is_empty() {
            return self.nb.search(&self.search).iter().filter_map(|h| card(h.page)).collect();
        }
        let Some(s) = self.section else { return Vec::new() };
        self.nb.children(s, None).iter().filter_map(|p| card(p.id)).collect()
    }

    /// Reads the canvases among the cards that changed since last seen.
    fn read_previews(&mut self) {
        let ids: Vec<i64> = if self.search.trim().is_empty() {
            self.section.map(|s| self.nb.children(s, None).iter().map(|p| p.id).collect()).unwrap_or_default()
        } else {
            self.nb.search(&self.search).iter().map(|h| h.page).collect()
        };
        for id in ids {
            let Some(p) = self.nb.page(id) else { continue };
            let modified = p.modified;
            if p.kind != PageKind::Canvas {
                // A text page: its first picture (none while sealed).
                if self.pictures.get(&id).is_none_or(|c| c.0 != modified) {
                    let first = if self.nb.sealed.contains(&p.section) { None } else { self.store.load_page(id).ok().and_then(|d| d.images().first().map(|(_, i)| i.data.clone())) };
                    self.pictures.insert(id, (modified, first));
                }
                continue;
            }
            if self.previews.get(&id).is_some_and(|c| c.0 == modified) {
                continue;
            }
            if let Ok(c) = self.store.load_canvas(id) {
                self.previews.insert(id, (modified, Arc::new(c)));
            }
        }
    }

    pub(super) fn cards_ui(&mut self, ui: &mut egui::Ui) {
        self.read_previews();
        let cards = self.cards();
        let searching = !self.search.trim().is_empty();
        egui::ScrollArea::vertical().id_salt(("note_cards", self.uid)).auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::new().inner_margin(egui::Margin::same(20)).show(ui, |ui| {
                let heading = if searching {
                    trf("nb.results", &[("n", &cards.len())])
                } else {
                    self.section.and_then(|s| self.nb.section(s)).map(|s| s.name.clone()).unwrap_or_default()
                };
                ui.label(RichText::new(heading).size(22.0));
                ui.add_space(12.0);
                let cols = ((ui.available_width() + GAP) / (CARD_W + GAP)).floor().max(1.0) as usize;
                let dragging = match self.drag {
                    Some(Naming::Page(p)) if !searching => Some(p),
                    _ => None,
                };
                let mut drop_at = None;
                for row in cards.chunks(cols) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = GAP;
                        for c in row {
                            if let Some(before) = self.card_ui(ui, c, dragging) {
                                drop_at = Some(before);
                            }
                        }
                    });
                    ui.add_space(GAP);
                }
                if ui.input(|i| i.pointer.any_released())
                    && let Some(p) = dragging
                    && let Some(before) = drop_at
                {
                    self.drag = None;
                    self.drop_page(p, Drop::Before(before));
                }
            });
        });
        if self.drag.is_some() && ui.input(|i| !i.pointer.any_down()) {
            self.drag = None;
        }
    }

    /// One card; the card a page dragged onto it goes before.
    fn card_ui(&mut self, ui: &mut egui::Ui, c: &Card, dragging: Option<i64>) -> Option<i64> {
        let (rect, r) = ui.allocate_exact_size(vec2(CARD_W, CARD_H), Sense::click_and_drag());
        crate::widgets::name(&r, egui::WidgetType::Button, &c.title, None);
        let v = ui.visuals();
        let p = ui.painter();
        let stroke = if r.hovered() { v.widgets.hovered.bg_stroke } else { v.widgets.noninteractive.bg_stroke };
        p.rect(rect, 8.0, v.panel_fill, stroke, egui::StrokeKind::Inside);
        let band = egui::Rect::from_min_size(rect.min, vec2(rect.width(), 5.0));
        p.rect_filled(band, egui::CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 }, section_color(c.color));
        let inner = rect.shrink2(vec2(12.0, 10.0)).translate(vec2(0.0, 4.0));
        let clip = p.with_clip_rect(inner);
        let title = clip.layout(c.title.clone(), egui::FontId::proportional(15.0), v.strong_text_color(), inner.width());
        let th = title.size().y.min(40.0);
        clip.galley(inner.min, title, v.strong_text_color());
        let area = egui::Rect::from_min_max(inner.min + vec2(0.0, th + 6.0), inner.max - vec2(0.0, 20.0));
        if let Some(canvas) = &c.canvas {
            miniature(ui, &clip.with_clip_rect(area), area, canvas);
        } else {
            let text: String = c.text.split_whitespace().collect::<Vec<_>>().join(" ");
            // The first picture: beside the text, or alone without any.
            let tex = c.picture.as_ref().and_then(|b| crate::images::texture(ui.ctx(), crate::images::key(b), b, false));
            let mut text_area = area;
            if let Some(tex) = tex {
                let pic = if text.is_empty() { area } else { egui::Rect::from_min_max(egui::pos2(area.max.x - area.width() * 0.42, area.min.y), area.max) };
                let size = tex.size_vec2();
                let fit = (pic.width() / size.x).min(pic.height() / size.y);
                let shown = egui::Rect::from_center_size(pic.center(), size * fit);
                clip.image(tex.id(), shown, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                text_area.max.x = pic.min.x - 8.0;
            }
            if text_area.width() > 10.0 {
                let body = clip.layout(text, egui::FontId::proportional(12.5), v.weak_text_color(), text_area.width());
                clip.with_clip_rect(text_area).galley(text_area.min, body, v.weak_text_color());
            }
        }
        let mut foot = date_text(c.modified);
        if c.subpages > 0 {
            let sub = if c.subpages == 1 { tr("nb.one_subpage").to_string() } else { trf("nb.subpages", &[("n", &c.subpages)]) };
            foot = format!("{foot} · {sub}");
        }
        clip.text(egui::pos2(inner.min.x, inner.max.y), egui::Align2::LEFT_BOTTOM, foot, egui::FontId::proportional(11.0), v.weak_text_color());
        if r.clicked() {
            self.open_page(c.id);
            self.grid = false;
        }
        if r.drag_started() {
            self.drag = Some(Naming::Page(c.id));
        }
        let mut target = None;
        if let Some(d) = dragging
            && d != c.id
            && ui.rect_contains_pointer(rect)
        {
            let x = rect.min.x - GAP / 2.0;
            p.vline(x, rect.y_range(), egui::Stroke::new(3.0, v.selection.stroke.color));
            target = Some(c.id);
        }
        let sections: Vec<(i64, String)> = self.nb.live_sections().iter().filter(|s| Some(s.id) != self.section).map(|s| (s.id, s.name.clone())).collect();
        r.context_menu(|ui| {
            if ui.button(tr("nb.rename")).clicked() {
                self.naming = Some((Naming::Page(c.id), c.title.clone()));
                ui.close();
            }
            if !sections.is_empty() {
                crate::widgets::menu_button(ui, tr("nb.move_to"), |ui| {
                    for (s, name) in &sections {
                        if ui.button(name).clicked() {
                            self.drop_page(c.id, Drop::Section(*s));
                            ui.close();
                        }
                    }
                });
            }
            ui.separator();
            if ui.button(tr("nb.delete_page")).clicked() {
                self.delete_page(c.id);
                ui.close();
            }
        });
        target
    }
}

/// `canvas` drawn small in `area`: its pictures and map, its notes in their
/// colours, its cards and groups as outlines, its markers as dots.
fn miniature(ui: &egui::Ui, p: &egui::Painter, area: egui::Rect, canvas: &Canvas) {
    let rect = |i: &apex_doc::notebook::canvas::Item| egui::Rect::from_min_size(egui::pos2(i.rect[0], i.rect[1]), vec2(i.rect[2], i.rect[3]));
    let Some(all) = canvas.items.iter().map(rect).reduce(|a, b| a.union(b)) else { return };
    let s = (area.width() / all.width().max(1.0)).min(area.height() / all.height().max(1.0)).min(1.0);
    let off = area.center() - all.center().to_vec2() * s;
    let at = |r: egui::Rect| egui::Rect::from_min_size(off + r.min.to_vec2() * s, r.size() * s);
    let v = ui.visuals();
    let full = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    for i in &canvas.items {
        let r = at(rect(i));
        match &i.kind {
            ItemKind::Map(bytes) | ItemKind::Picture(bytes) => match crate::images::texture(ui.ctx(), crate::images::key(bytes), bytes, false) {
                Some(tex) => {
                    p.image(tex.id(), r, full, Color32::WHITE);
                }
                None => {
                    p.rect_filled(r, 2.0, v.faint_bg_color);
                }
            },
            ItemKind::Note(_) => {
                p.rect_filled(r, 2.0, rgb(i.color));
            }
            ItemKind::Page(_) => {
                p.rect(r, 2.0, v.extreme_bg_color, v.widgets.inactive.fg_stroke, egui::StrokeKind::Inside);
            }
            ItemKind::Group(_) => {
                p.rect_stroke(r, 3.0, egui::Stroke::new(1.0, rgb(i.color)), egui::StrokeKind::Inside);
            }
            ItemKind::Pin(_) => {
                p.circle_filled(r.min, 3.0, theme::NOTE_ACCENT);
            }
        }
    }
}

fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}
