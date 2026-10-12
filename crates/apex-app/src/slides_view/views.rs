//! The other views of a presentation: the slide sorter (every slide in a
//! grid, to reorder), the outline (titles and body text to type) and the
//! slide master (masters and layouts to edit).

use apex_slides::{ObjKind, TextBody, edit};
use eframe::egui::{self, Color32, Id, Rect, RichText, Sense, Stroke, pos2, vec2};

use super::{Mode, SlidesView, render};
use crate::i18n::{tr, trf};
use crate::theme;

/// Undo steps typing in the outline joins (per slide and part).
const OUTLINE_KEY: u64 = 0x0071_1E00;

/// A body's paragraphs as outline lines: a tab per level.
pub fn body_lines(t: &TextBody) -> String {
    t.paras.iter().map(|p| format!("{}{}", "\t".repeat(p.style.level as usize), p.text.replace(apex_doc::LINE_BREAK, " "))).collect::<Vec<_>>().join("\n")
}

/// Sets a body from outline lines (tabs give the levels), keeping the
/// paragraphs' formats where there were paragraphs.
pub fn set_body_lines(t: &mut TextBody, s: &str) {
    let old = std::mem::take(&mut t.paras);
    for (i, line) in s.split('\n').enumerate() {
        let level = line.chars().take_while(|c| *c == '\t').count().min(8) as u8;
        let text = &line[level as usize..];
        let like = old.get(i).or(old.last());
        let mut p = apex_doc::Paragraph::new(text);
        if let Some(l) = like {
            p.style = l.style;
            let attrs = l.runs.first().map(|r| r.attrs.clone()).unwrap_or_default();
            p.runs = vec![apex_doc::Run { len: text.len(), attrs }];
        }
        p.style.level = level;
        t.paras.push(p);
    }
}

impl SlidesView {
    /// Object ids of slide `i`'s title and first body placeholder.
    fn outline_parts(&self, i: usize) -> (Option<u32>, Option<u32>) {
        let Some(s) = self.pres.slides.get(i) else { return (None, None) };
        let title = s.objects.iter().find(|o| o.is_title()).map(|o| o.id);
        let body = s.objects.iter().find(|o| o.ph.is_some_and(|p| !p.kind.is_title() && !p.kind.is_hf()) && matches!(o.kind, ObjKind::Shape { .. })).map(|o| o.id);
        (title, body)
    }

    /// Sets the title (`body` false) or body text of slide `i` from the
    /// outline; typing joins one step.
    pub fn set_outline(&mut self, i: usize, body: bool, text: &str) {
        let (t, b) = self.outline_parts(i);
        let Some(id) = (if body { b } else { t }) else { return };
        let Some(s) = self.pres.slides.get(i) else { return };
        if self.read_only {
            return;
        }
        let key = OUTLINE_KEY ^ ((s.id as u64) << 24) ^ ((body as u64) << 20);
        let deck = self.pres.edit_merge("slides.undo.outline", key);
        if let Some(o) = deck.object_mut(i, id)
            && let Some(tb) = o.text_or_new()
        {
            if body {
                set_body_lines(tb, text);
            } else {
                tb.set_plain(&text.replace('\n', " "));
            }
            o.touch();
        }
    }

    /// The outline pane: each slide's title and body text.
    pub(super) fn outline_pane(&mut self, ui: &mut egui::Ui) {
        let n = self.pres.slides.len();
        let mut go = None;
        let mut changes: Vec<(usize, bool, String)> = Vec::new();
        let mut sealed = false;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for i in 0..n {
                let (t, b) = self.outline_parts(i);
                let s = &self.pres.slides[i];
                let find = |id: Option<u32>| id.and_then(|id| s.objects.iter().find(|o| o.id == id)).and_then(|o| o.text()).cloned();
                let mut title = find(t).map(|x| x.text().replace('\n', " ")).unwrap_or_default();
                let mut body = find(b).map(|x| body_lines(&x)).unwrap_or_default();
                ui.horizontal(|ui| {
                    let cur = i == self.cur;
                    let num = RichText::new(format!("{}", i + 1)).strong().color(if cur { theme::SLIDES_ACCENT } else { ui.visuals().text_color() });
                    if ui.add(egui::Label::new(num).sense(Sense::click())).clicked() {
                        go = Some(i);
                    }
                    ui.vertical(|ui| {
                        let r = ui.add_enabled(t.is_some() && !self.read_only, egui::TextEdit::singleline(&mut title).hint_text(tr("slides.ph.title")).desired_width(f32::INFINITY).font(egui::TextStyle::Heading).id(Id::new(("outline_t", self.uid, s.id))));
                        if r.changed() {
                            changes.push((i, false, title.clone()));
                        }
                        if r.gained_focus() {
                            go = Some(i);
                        }
                        sealed |= r.lost_focus();
                        if b.is_some() {
                            let r = ui.add_enabled(!self.read_only, egui::TextEdit::multiline(&mut body).hint_text(tr("slides.ph.text")).desired_rows(1).desired_width(f32::INFINITY).id(Id::new(("outline_b", self.uid, s.id))));
                            if r.changed() {
                                changes.push((i, true, body.clone()));
                            }
                            if r.gained_focus() {
                                go = Some(i);
                            }
                            sealed |= r.lost_focus();
                        }
                    });
                });
                ui.add_space(6.0);
            }
        });
        for (i, body, text) in changes {
            self.set_outline(i, body, &text);
        }
        if sealed {
            self.pres.seal();
        }
        if let Some(i) = go
            && i != self.cur
        {
            self.go(i);
        }
    }

    /// The slide sorter: every slide in a grid, dragged to move.
    pub(super) fn sorter(&mut self, ui: &mut egui::Ui) {
        let deck = &self.pres.deck;
        let w = 180.0 * self.sorter_zoom;
        let h = w * deck.height / deck.width.max(1.0);
        let cell = vec2(w + 24.0, h + 30.0);
        let per_row = ((ui.available_width() - 16.0) / cell.x).floor().max(1.0) as usize;
        let n = deck.slides.len();
        let mut click = None;
        let mut open = None;
        let mut drop_at = None;
        let dragging = self.sorter_drag;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let origin = ui.cursor().min + vec2(8.0, 8.0);
            let rows = n.div_ceil(per_row);
            ui.allocate_space(vec2(per_row as f32 * cell.x, rows as f32 * cell.y + 16.0));
            let pointer = ui.input(|i| i.pointer.interact_pos());
            for i in 0..n {
                let (r, c) = (i / per_row, i % per_row);
                let at = origin + vec2(c as f32 * cell.x, r as f32 * cell.y);
                let rect = Rect::from_min_size(at + vec2(12.0, 6.0), vec2(w, h));
                let resp = ui.interact(rect, Id::new(("sorter", self.uid, i)), Sense::click_and_drag());
                crate::widgets::name(&resp, egui::WidgetType::SelectableLabel, &trf("slides.slide_n", &[("n", &(i + 1))]), Some(i == self.cur));
                let s = &deck.slides[i];
                let p = ui.painter();
                crate::theme::dark_paint(p, || render::paint_slide(p, deck, s, rect, render::Opts::default()));
                if s.hidden {
                    p.rect_filled(rect, 0.0, Color32::from_white_alpha(140));
                }
                let stroke = if i == self.cur { Stroke::new(3.0, theme::SLIDES_ACCENT) } else { Stroke::new(1.0, Color32::from_gray(170)) };
                p.rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Outside);
                p.text(pos2(rect.min.x, rect.max.y + 4.0), egui::Align2::LEFT_TOP, format!("{}", i + 1), egui::FontId::proportional(12.0), ui.visuals().text_color());
                if resp.clicked() {
                    click = Some(i);
                }
                if resp.double_clicked() {
                    open = Some(i);
                }
                if resp.drag_started() {
                    self.sorter_drag = Some(i);
                }
                // Where a dragged slide goes: before the one under the pointer.
                if let (Some(_), Some(q)) = (dragging, pointer) {
                    let slot = Rect::from_min_size(at, cell);
                    if slot.contains(q) {
                        let before = if q.x > slot.center().x { i + 1 } else { i };
                        let x = if before == i { slot.min.x + 6.0 } else { slot.max.x - 6.0 };
                        ui.painter().line_segment([pos2(x, rect.min.y), pos2(x, rect.max.y)], Stroke::new(3.0, theme::SLIDES_ACCENT));
                        drop_at = Some(before);
                    }
                }
            }
        });
        if let Some(i) = click {
            self.go(i);
        }
        if let Some(i) = open {
            self.go(i);
            self.mode = Mode::Normal;
        }
        if ui.input(|i| i.pointer.any_released())
            && let Some(from) = self.sorter_drag.take()
            && let Some(to) = drop_at
            && to != from
            && to != from + 1
            && !self.read_only
        {
            self.cur = edit::move_slides(self.pres.edit("slides.undo.move"), &[from], to);
        }
    }

    /// The master view's pane: each master and its layouts.
    pub(super) fn masters_pane(&mut self, ui: &mut egui::Ui) {
        let deck = &self.pres.deck;
        let w = (ui.available_width() - 26.0).max(40.0);
        let h = w * deck.height / deck.width.max(1.0);
        let mut pick = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (m, master) in deck.masters.iter().enumerate() {
                let items = std::iter::once(None).chain((0..master.layouts.len()).map(Some));
                for l in items {
                    let (tw, th, indent) = if l.is_none() { (w, h, 0.0) } else { (w * 0.8, h * 0.8, w * 0.2) };
                    let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), th + 10.0), Sense::click());
                    let r = Rect::from_min_size(pos2(row.min.x + 18.0 + indent, row.min.y + 4.0), vec2(tw, th));
                    render::paint_layout(ui.painter(), deck, m, l, r, render::Opts { edit: true, ..Default::default() });
                    let on = self.master_at == (m, l);
                    let stroke = if on { Stroke::new(2.5, theme::SLIDES_ACCENT) } else { Stroke::new(1.0, Color32::from_gray(170)) };
                    ui.painter().rect_stroke(r, 1.0, stroke, egui::StrokeKind::Outside);
                    let name = match l {
                        None => if master.name.is_empty() { trf("slides.master_n", &[("n", &(m + 1))]) } else { master.name.clone() },
                        Some(l) => master.layouts[l].name.clone(),
                    };
                    crate::widgets::name(&resp, egui::WidgetType::SelectableLabel, &name, Some(on));
                    if resp.on_hover_text(&name).clicked() {
                        pick = Some((m, l));
                    }
                }
                ui.separator();
            }
        });
        if let Some(p) = pick {
            self.stop_typing();
            self.sel.clear();
            self.master_at = p;
        }
    }

    /// Switches views (the master view starts on the current slide's layout).
    pub fn set_mode(&mut self, mode: Mode) {
        self.commit_notes();
        self.stop_typing();
        self.sel.clear();
        if mode == Mode::Master && self.mode != Mode::Master {
            self.master_at = self.pres.slides.get(self.cur).map_or((0, None), |s| (s.master, Some(s.layout)));
        }
        self.mode = mode;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outline_lines_round_trip() {
        let mut t = TextBody::plain("a\nb\nc");
        t.paras[1].style.level = 1;
        t.paras[0].runs[0].attrs.bold = true;
        let s = body_lines(&t);
        assert_eq!(s, "a\n\tb\nc");
        set_body_lines(&mut t, "x\n\t\ty\n");
        assert_eq!(t.paras.len(), 3);
        assert_eq!(t.paras[1].style.level, 2);
        assert_eq!(t.paras[1].text, "y");
        assert!(t.paras[0].runs[0].attrs.bold);
    }

    #[test]
    fn outline_edits_and_modes() {
        let mut v = SlidesView::new(apex_slides::build::new_deck(0));
        v.new_slide();
        v.set_outline(1, false, "Agenda");
        v.set_outline(1, false, "Agenda!");
        v.set_outline(1, true, "One\n\tTwo");
        assert_eq!(v.pres.slides[1].title(), "Agenda!");
        let (_, body) = &edit::outline(&v.pres.deck)[1];
        assert_eq!(body, &vec![(0, "One".to_string()), (1, "Two".to_string())]);
        v.undo();
        v.undo();
        assert_eq!(v.pres.slides[1].title(), "");
        v.set_mode(Mode::Master);
        assert_eq!(v.master_at, (0, Some(1)));
        assert_eq!(v.target(), super::super::objects::Target::Layout(0, 1));
        v.set_mode(Mode::Sorter);
        assert_eq!(v.target(), super::super::objects::Target::Slide(1));
    }

    #[test]
    fn master_objects_edit() {
        let mut v = SlidesView::new(apex_slides::build::new_deck(0));
        v.set_mode(Mode::Master);
        v.master_at = (0, None);
        let n = v.pres.masters[0].objects.len();
        let id = v.insert_shape("rect", apex_slides::Frame::new(0.0, 0.0, 20.0, 20.0), false).unwrap();
        assert_eq!(v.pres.masters[0].objects.len(), n + 1);
        v.nudge(10.0, 0.0);
        assert_eq!(v.frames_of(&[id])[0].1.x, 10.0);
        v.delete_selected();
        assert_eq!(v.pres.masters[0].objects.len(), n);
    }
}
