//! The corkboard of a book: a card a scene, by chapter, with its title,
//! synopsis, status, point of view and words. Dragging a card moves its
//! scene in the text; a double click opens it there.

use super::*;
use apex_doc::book::{BookNode, SceneInfo};

const CARD: Vec2 = vec2(220.0, 140.0);
const GAP: f32 = 14.0;

/// A scene card being dragged: its heading.
#[derive(Clone, Copy)]
struct CardDrag(usize);

/// Where a dragged card goes: before paragraph `.0`.
struct Drop(usize);

impl DocView {
    /// Chapters with their scenes, as the board shows them; scenes before
    /// the first chapter go under none.
    fn board_groups(nodes: &[BookNode]) -> Vec<(Option<&BookNode>, Vec<&BookNode>)> {
        let mut out: Vec<(Option<&BookNode>, Vec<&BookNode>)> = Vec::new();
        for n in nodes {
            if n.level == 1 {
                out.push((Some(n), Vec::new()));
            } else if let Some(last) = out.last_mut() {
                last.1.push(n);
            } else {
                out.push((None, vec![n]));
            }
        }
        out
    }

    pub(super) fn book_cards(&mut self, ui: &mut egui::Ui) {
        let (undo, redo, esc) = ui.input_mut(|i| (i.consume_key(Modifiers::COMMAND, Key::Z), i.consume_key(Modifiers::COMMAND, Key::Y) || i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z), i.consume_key(Modifiers::NONE, Key::Escape)));
        if undo {
            self.undo();
        } else if redo {
            self.redo();
        }
        if esc {
            self.book.view = super::book_ui::BookView::Text;
            self.focus = true;
            return;
        }
        let nodes = self.doc.book_nodes();
        let infos: Vec<Option<SceneInfo>> = nodes.iter().map(|n| self.doc.scene_info(n.para).cloned()).collect();
        let info_of = |para: usize| nodes.iter().position(|n| n.para == para).and_then(|i| infos[i].clone()).unwrap_or_default();
        let current = self.book_scene_at(self.sel.cursor.para);
        let len = self.doc.paragraphs.len();
        let mut drop: Option<(usize, Drop)> = None;
        let mut open: Option<usize> = None;
        let mut pick: Option<usize> = None;
        let mut add: Option<(usize, usize)> = None;
        let dark = ui.visuals().dark_mode;
        let bg = if dark { Color32::from_rgb(0x4A, 0x40, 0x35) } else { Color32::from_rgb(0xD9, 0xC4, 0xA3) };
        let (ink, weak) = if dark { (Color32::from_gray(235), Color32::from_gray(185)) } else { (Color32::from_gray(25), Color32::from_gray(70)) };
        egui::Frame::new().fill(bg).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt(("book_cards", self.uid)).auto_shrink([false, false]).show(ui, |ui| {
                ui.add_space(16.0);
                let groups = Self::board_groups(&nodes);
                if groups.is_empty() {
                    ui.vertical_centered(|ui| ui.label(RichText::new(tr("book.empty")).color(ink)));
                }
                let dragging = egui::DragAndDrop::has_payload_of_type::<CardDrag>(ui.ctx());
                for (chapter, scenes) in &groups {
                    ui.horizontal(|ui| {
                        ui.add_space(20.0);
                        let title = chapter.map_or_else(|| tr("book.no_chapter").to_string(), |c| c.title.clone());
                        ui.label(RichText::new(title).strong().size(16.0).color(ink));
                        if let Some(c) = chapter {
                            ui.label(RichText::new(trf("doc.words", &[("n", &c.words)])).size(12.0).color(weak));
                        }
                    });
                    ui.add_space(4.0);
                    let end = chapter.map_or_else(|| scenes.last().map_or(0, |s| s.end), |c| c.end);
                    egui::Frame::new().inner_margin(egui::Margin { left: 20, right: 20, top: 0, bottom: 0 }).show(ui, |ui| ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(GAP, GAP);
                        for s in scenes {
                            let (rect, resp) = ui.allocate_exact_size(CARD, Sense::click_and_drag());
                            paint_card(ui, rect, s, &info_of(s.para), current == Some(s.para));
                            let resp = resp.on_hover_text(tr("book.card_tip"));
                            if resp.double_clicked() {
                                open = Some(s.para);
                            } else if resp.clicked() {
                                pick = Some(s.para);
                            }
                            resp.dnd_set_drag_payload(CardDrag(s.para));
                            if dragging && let Some(p) = ui.ctx().pointer_interact_pos() && rect.expand(GAP / 2.0).contains(p) {
                                let right = p.x > rect.center().x;
                                let x = if right { rect.max.x + GAP / 2.0 } else { rect.min.x - GAP / 2.0 };
                                ui.painter().vline(x, rect.y_range(), Stroke::new(3.0, ui.visuals().selection.stroke.color));
                                if let Some(d) = resp.dnd_release_payload::<CardDrag>() {
                                    drop = Some((d.0, Drop(if right { s.end } else { s.para })));
                                }
                            }
                        }
                        // A new scene at the end of the chapter; a card dropped
                        // here goes there.
                        let (rect, resp) = ui.allocate_exact_size(vec2(CARD.x * 0.5, CARD.y), Sense::click());
                        let hot = resp.hovered() || (dragging && ui.ctx().pointer_interact_pos().is_some_and(|p| rect.contains(p)));
                        let c = if dark { Color32::from_white_alpha(if hot { 120 } else { 50 }) } else { Color32::from_black_alpha(if hot { 90 } else { 45 }) };
                        ui.painter().rect_stroke(rect, 6.0, Stroke::new(1.5, c), egui::StrokeKind::Inside);
                        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "+", egui::FontId::proportional(28.0), c);
                        let resp = resp.on_hover_text(tr("book.new_scene"));
                        if let Some(d) = resp.dnd_release_payload::<CardDrag>() {
                            drop = Some((d.0, Drop(end)));
                        } else if resp.clicked() {
                            add = Some((chapter.map_or(0, |c| c.para), end));
                        }
                    }));
                    ui.add_space(18.0);
                }
            });
        });
        if let Some((from, Drop(to))) = drop
            && to.min(len) != from
        {
            match self.doc.move_section(from, to, self.sel) {
                Some(s) => self.set_sel(s),
                None => self.notices.push(Notice::info(tr("nav.cant_move"))),
            }
        }
        if let Some(p) = pick {
            self.sel = Selection::caret(Pos::new(p, 0));
        }
        if let Some((from, at)) = add {
            let n = self.doc.headings().iter().filter(|&&(i, l)| l == 2 && i >= from && i < at).count() + 1;
            self.add_heading(at, 2, &trf("book.scene_n", &[("n", &n)]));
        }
        if let Some(p) = open {
            self.go_to_heading(p);
        }
    }
}

fn paint_card(ui: &egui::Ui, rect: Rect, s: &BookNode, info: &SceneInfo, current: bool) {
    let p = ui.painter();
    p.rect_filled(rect.translate(vec2(2.0, 3.0)), 6.0, Color32::from_black_alpha(50));
    p.rect_filled(rect, 6.0, Color32::from_rgb(0xFC, 0xFB, 0xF7));
    let status = super::book_ui::status_color(info.status);
    p.rect_filled(Rect::from_min_size(rect.min, vec2(rect.width(), 6.0)), egui::CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }, status);
    if current {
        p.rect_stroke(rect, 6.0, Stroke::new(2.0, theme::DOC_ACCENT), egui::StrokeKind::Outside);
    }
    let inner = rect.shrink2(vec2(10.0, 8.0));
    let ink = Color32::from_gray(30);
    let clip = p.with_clip_rect(inner);
    let title = clip.layout_no_wrap(s.title.clone(), egui::FontId::proportional(14.5), ink);
    let th = title.size().y;
    clip.galley(inner.min + vec2(0.0, 4.0), title, ink);
    let foot = 16.0;
    let body = Rect::from_min_max(inner.min + vec2(0.0, th + 8.0), pos2(inner.max.x, inner.max.y - foot));
    if !info.synopsis.is_empty() {
        let job = egui::text::LayoutJob::simple(info.synopsis.clone(), egui::FontId::proportional(12.5), Color32::from_gray(60), body.width());
        let g = ui.fonts_mut(|f| f.layout_job(job));
        p.with_clip_rect(body).galley(body.min, g, Color32::from_gray(60));
    }
    let weak = Color32::from_gray(110);
    let y = inner.max.y - foot / 2.0;
    let words = trf("doc.words", &[("n", &s.words)]);
    let w = p.text(pos2(inner.max.x, y), egui::Align2::RIGHT_CENTER, words, egui::FontId::proportional(11.0), weak);
    if !info.pov.is_empty() {
        let pov = Rect::from_min_max(pos2(inner.min.x, y - 8.0), pos2(w.min.x - 6.0, y + 8.0));
        p.with_clip_rect(pov).text(pos2(inner.min.x, y), egui::Align2::LEFT_CENTER, format!("👁 {}", info.pov), egui::FontId::proportional(11.0), weak);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_board_groups_scenes_by_chapter() {
        let h = |t: &str, l: u8| {
            let mut p = Paragraph::new(t);
            p.style.kind = ParaKind::Heading(l);
            p
        };
        let doc = Document::from_paragraphs(vec![h("Prólogo", 2), Paragraph::new("x"), h("Uno", 1), h("A", 2), h("B", 2), h("Dos", 1)]);
        let nodes = doc.book_nodes();
        let g = DocView::board_groups(&nodes);
        let names: Vec<(Option<&str>, Vec<&str>)> = g.iter().map(|(c, s)| (c.map(|c| c.title.as_str()), s.iter().map(|s| s.title.as_str()).collect())).collect();
        assert_eq!(names, vec![(None, vec!["Prólogo"]), (Some("Uno"), vec!["A", "B"]), (Some("Dos"), vec![])]);
    }
}
