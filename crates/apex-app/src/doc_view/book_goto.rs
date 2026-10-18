//! Go to (Ctrl+P in a book): a chapter or scene found by typing its name.

use super::*;
use crate::palette::{fold, score};

const ROWS: usize = 12;

/// The search of Go to.
#[derive(Default)]
pub(super) struct GoTo {
    pub query: String,
    hot: usize,
    first: bool,
}

impl DocView {
    pub(super) fn open_go_to(&mut self) {
        self.book.go_to = Some(GoTo { first: true, ..GoTo::default() });
    }

    /// Chapters and scenes matching `q`, best first: (heading, chapter of
    /// a scene).
    pub(super) fn go_to_results(&self, q: &str) -> Vec<(usize, Option<usize>)> {
        let nodes = self.doc.book_nodes();
        let chapter_of = |i: usize| nodes[..i].iter().rev().find(|n| n.level == 1).filter(|_| nodes[i].level == 2).map(|n| n.para);
        let q = fold(q.trim());
        if q.is_empty() {
            return nodes.iter().enumerate().take(ROWS).map(|(i, n)| (n.para, chapter_of(i))).collect();
        }
        let mut scored: Vec<(i32, usize)> = nodes.iter().enumerate().filter_map(|(i, n)| Some((score(&fold(&n.title), &q)?, i))).collect();
        scored.sort_by_key(|&(s, i)| (std::cmp::Reverse(s), i));
        scored.into_iter().take(ROWS).map(|(_, i)| (nodes[i].para, chapter_of(i))).collect()
    }

    /// Goes to heading `para`: in a book seen in part, it is shown alone
    /// if it was not shown.
    pub(super) fn go_to_heading(&mut self, para: usize) {
        self.book.view = super::book_ui::BookView::Text;
        if self.scope_hidden().is_some_and(|h| h.get(para).copied().unwrap_or(false)) {
            self.show_only(&[para]);
        } else {
            self.go_to_para(para);
        }
        self.focus = true;
    }

    pub(super) fn go_to_ui(&mut self, ctx: &egui::Context) {
        let Some(mut g) = self.book.go_to.take() else { return };
        let (up, down, enter, esc) = ctx.input_mut(|i| (i.consume_key(Modifiers::NONE, Key::ArrowUp), i.consume_key(Modifiers::NONE, Key::ArrowDown), i.consume_key(Modifiers::NONE, Key::Enter), i.consume_key(Modifiers::NONE, Key::Escape)));
        if esc {
            self.focus = true;
            return;
        }
        let results = self.go_to_results(&g.query);
        let n = results.len();
        if down && n > 0 {
            g.hot = (g.hot + 1) % n;
        }
        if up && n > 0 {
            g.hot = (g.hot + n - 1) % n;
        }
        g.hot = g.hot.min(n.saturating_sub(1));
        let screen = ctx.content_rect();
        let width = (screen.width() - 32.0).min(480.0);
        let title = |p: usize| apex_doc::Paragraph::visible(&self.doc.paragraphs[p], 0, self.doc.paragraphs[p].len()).trim().to_string();
        let mut chosen = None;
        let resp = egui::Area::new(Id::new(("book_go_to", self.uid))).order(egui::Order::Foreground).fixed_pos(pos2(screen.center().x - width / 2.0, screen.min.y + 56.0)).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).inner_margin(8.0).show(ui, |ui| {
                ui.set_width(width);
                let r = ui.add(egui::TextEdit::singleline(&mut g.query).hint_text(tr("book.go_to_hint")).desired_width(f32::INFINITY).font(egui::FontId::proportional(15.0)).margin(vec2(8.0, 6.0)));
                if std::mem::take(&mut g.first) {
                    r.request_focus();
                }
                if r.changed() {
                    g.hot = 0;
                }
                ui.add_space(4.0);
                if results.is_empty() {
                    ui.label(RichText::new(tr("palette.none")).weak());
                }
                for (row, &(p, chapter)) in results.iter().enumerate() {
                    let (rect, r) = ui.allocate_exact_size(vec2(width, 28.0), Sense::click());
                    let name = title(p);
                    crate::widgets::name(&r, egui::WidgetType::SelectableLabel, &name, Some(row == g.hot));
                    if r.hovered() && ui.input(|i| i.pointer.delta() != Vec2::ZERO) {
                        g.hot = row;
                    }
                    let v = ui.visuals();
                    if row == g.hot {
                        ui.painter().rect_filled(rect, 4.0, v.selection.bg_fill.gamma_multiply(0.5));
                    }
                    let icon = if chapter.is_some() { Icon::Scene } else { Icon::Chapter };
                    let ir = Rect::from_center_size(pos2(rect.min.x + 16.0, rect.center().y), vec2(16.0, 16.0));
                    crate::icons::paint(ui.painter(), ir, icon, v.text_color());
                    let gname = ui.painter().layout_no_wrap(name, egui::FontId::proportional(14.0), v.text_color());
                    let x = rect.min.x + 34.0 + gname.size().x + 12.0;
                    ui.painter().galley(pos2(rect.min.x + 34.0, rect.center().y - gname.size().y / 2.0), gname, v.text_color());
                    if let Some(c) = chapter {
                        let clip = ui.painter().with_clip_rect(Rect::from_min_max(pos2(x, rect.min.y), pos2(rect.max.x - 8.0, rect.max.y)));
                        clip.text(pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, title(c), egui::FontId::proportional(12.0), v.weak_text_color());
                    }
                    if r.clicked() {
                        chosen = Some(p);
                    }
                }
            });
        });
        if enter && let Some(&(p, _)) = results.get(g.hot) {
            chosen = Some(p);
        }
        if let Some(p) = chosen {
            self.go_to_heading(p);
        } else if resp.response.clicked_elsewhere() {
            self.focus = true;
        } else {
            self.book.go_to = Some(g);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_to_finds_chapters_and_scenes_by_name() {
        let h = |t: &str, l: u8| {
            let mut p = Paragraph::new(t);
            p.style.kind = ParaKind::Heading(l);
            p
        };
        let doc = Document::from_paragraphs(vec![h("El puerto", 1), h("La niebla", 2), Paragraph::new("x"), h("La tormenta", 1), h("Naufragio", 2), Paragraph::new("y")]);
        let mut v = DocView::new(doc);
        v.make_book();
        assert_eq!(v.go_to_results("").len(), 4);
        assert_eq!(v.go_to_results("naufr"), vec![(4, Some(3))]);
        assert_eq!(v.go_to_results("TORMENTA"), vec![(3, None)]);
        assert_eq!(v.go_to_results("niébla")[0], (1, Some(0)));
        // A scene not shown is shown alone.
        v.show_only(&[1]);
        v.go_to_heading(4);
        assert_eq!(v.scope_ranges(), Some(vec![(4, 6)]));
        assert_eq!(v.sel.cursor, Pos::new(4, 0));
    }
}
