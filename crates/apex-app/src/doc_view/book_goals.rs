//! A book's goals: its words, words a day and a deadline, with how far
//! it has got at the foot of the manuscript panel. The words of each day
//! are kept in the book as it is written.

use super::*;
use crate::widgets::DialogResult;
use apex_sheet::format::days_from_civil;

/// A goal met.
const DONE: Color32 = Color32::from_rgb(0x2E, 0x9E, 0x5B);

/// Today, "YYYY-MM-DD" in local time.
pub(super) fn today() -> String {
    let (y, m, d, ..) = common::local_now();
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days of a "YYYY-MM-DD" date.
fn days(s: &str) -> Option<i64> {
    let t = s.trim();
    let ok = t.len() == 10 && t.bytes().enumerate().all(|(i, b)| if i == 4 || i == 7 { b == b'-' } else { b.is_ascii_digit() });
    if !ok {
        return None;
    }
    let (y, m, d) = (t[..4].parse().ok()?, t[5..7].parse().ok()?, t[8..].parse().ok()?);
    ((1..=12).contains(&m) && (1..=31).contains(&d)).then(|| days_from_civil(y, m, d))
}

/// Days left until the deadline (today included) and the words a day
/// they ask for, when there is a deadline and a book goal.
pub(super) fn pace(goal: &apex_doc::book::Goal, words: u32, today: &str) -> Option<(i64, u32)> {
    let left = days(&goal.deadline)? - days(today)? + 1;
    let missing = goal.words.saturating_sub(words);
    Some((left, if left > 0 { missing.div_ceil(left as u32) } else { missing }))
}

/// The goals being edited.
pub(super) struct GoalEdit {
    words: u32,
    daily: u32,
    deadline: String,
}

impl DocView {
    /// Keeps today's words in the book.
    pub(super) fn record_words(&mut self, words: usize) {
        self.book.session_start.get_or_insert(words);
        if let Some(b) = self.doc.book.as_mut() {
            b.record_day(&today(), words as u32);
        }
    }

    pub(super) fn open_goals(&mut self) {
        let g = self.doc.book.as_ref().map(|b| b.goal.clone()).unwrap_or_default();
        self.book.goals = Some(GoalEdit { words: g.words, daily: g.daily, deadline: g.deadline });
    }

    pub(super) fn goals_dialog(&mut self, ctx: &egui::Context) {
        let Some(g) = self.book.goals.as_mut() else { return };
        let bad = !g.deadline.trim().is_empty() && days(&g.deadline).is_none();
        let r = crate::widgets::dialog(ctx, "doc_book_goals", tr("book.goals_title"), 380.0, |ui| {
            egui::Grid::new("book_goals").num_columns(2).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
                ui.label(tr("book.goal_words"));
                ui.add(egui::DragValue::new(&mut g.words).range(0..=10_000_000).speed(100.0));
                ui.end_row();
                ui.label(tr("book.goal_daily"));
                ui.add(egui::DragValue::new(&mut g.daily).range(0..=100_000).speed(10.0));
                ui.end_row();
                ui.label(tr("book.goal_deadline"));
                ui.add(egui::TextEdit::singleline(&mut g.deadline).hint_text(tr("book.date_hint")).desired_width(120.0));
                ui.end_row();
            });
            if bad {
                ui.colored_label(ui.visuals().error_fg_color, tr("book.bad_date"));
            }
            ui.label(RichText::new(tr("book.goals_note")).weak().small());
        });
        match r {
            DialogResult::None => {}
            DialogResult::Ok if bad => {}
            DialogResult::Ok => {
                let g = self.book.goals.take().unwrap_or_else(|| unreachable!());
                let goal = apex_doc::book::Goal { words: g.words, daily: g.daily, deadline: g.deadline.trim().to_string() };
                if self.doc.book.as_ref().is_some_and(|b| b.goal != goal) {
                    self.doc.edit_parts(|p| {
                        if let Some(b) = p.book.as_mut() {
                            b.goal = goal;
                        }
                    });
                    self.focus = true;
                }
            }
            DialogResult::Cancel => self.book.goals = None,
        }
    }

    /// How far the book has got, at the foot of the panel.
    pub(super) fn book_goals_foot(&mut self, ui: &mut egui::Ui) {
        let (words, ..) = self.book_stats();
        let Some(b) = self.doc.book.as_ref() else { return };
        let today = today();
        let written = b.written_on(&today);
        let session = words.saturating_sub(self.book.session_start.unwrap_or(words));
        let g = &b.goal;
        let accent = theme::DOC_ACCENT;
        let bar = |ui: &mut egui::Ui, done: u32, of: u32, text: String| {
            let f = if of == 0 { 0.0 } else { (done as f32 / of as f32).min(1.0) };
            ui.add(egui::ProgressBar::new(f).text(text).fill(if done >= of { DONE } else { accent }).desired_height(16.0));
        };
        if g.words > 0 {
            bar(ui, words as u32, g.words, trf("book.of_words", &[("n", &words), ("m", &g.words)]));
        }
        if g.daily > 0 {
            bar(ui, written, g.daily, trf("book.today_of", &[("n", &written), ("m", &g.daily)]));
        } else {
            ui.label(trf("book.today", &[("n", &written)]));
        }
        ui.label(RichText::new(trf("book.session", &[("n", &session)])).weak());
        if let Some((left, per_day)) = pace(g, words as u32, &today) {
            let text = if words as u32 >= g.words {
                tr("book.goal_reached").to_string()
            } else if left > 0 {
                trf("book.pace", &[("d", &left), ("n", &per_day)])
            } else {
                tr("book.deadline_past").to_string()
            };
            ui.label(RichText::new(text).weak());
        }
        // The last two weeks, a bar a day.
        let days: Vec<(&str, u32)> = b.days().into_iter().rev().take(14).collect();
        if days.len() > 1 {
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
            let top = days.iter().map(|d| d.1).max().unwrap_or(0).max(g.daily).max(1) as f32;
            let w = rect.width() / 14.0;
            let mut hover = None;
            for (k, &(date, n)) in days.iter().enumerate() {
                let x = rect.max.x - (k as f32 + 1.0) * w;
                let h = (n as f32 / top * rect.height()).max(1.0);
                let r = Rect::from_min_max(pos2(x + 1.5, rect.max.y - h), pos2(x + w - 1.5, rect.max.y));
                let c = if g.daily > 0 && n >= g.daily { DONE } else { accent.gamma_multiply(0.8) };
                ui.painter().rect_filled(r, 1.5, c);
                if resp.hover_pos().is_some_and(|p| p.x >= x && p.x < x + w) {
                    hover = Some(format!("{date}: {}", trf("doc.words", &[("n", &n)])));
                }
            }
            if g.daily > 0 {
                let y = rect.max.y - g.daily as f32 / top * rect.height();
                ui.painter().hline(rect.x_range(), y, Stroke::new(1.0, ui.visuals().weak_text_color()));
            }
            if let Some(t) = hover {
                resp.on_hover_text(t);
            }
        }
        if ui.link(tr("book.set_goals")).clicked() {
            self.open_goals();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pace_comes_from_the_deadline() {
        let g = apex_doc::book::Goal { words: 10_000, daily: 0, deadline: "2026-10-12".into() };
        assert_eq!(pace(&g, 4_000, "2026-10-03"), Some((10, 600)));
        assert_eq!(pace(&g, 4_000, "2026-10-13"), Some((0, 6_000)));
        assert_eq!(pace(&apex_doc::book::Goal { deadline: "mañana".into(), ..g.clone() }, 0, "2026-10-03"), None);
        let mut v = DocView::new(Document::from_paragraphs(vec![Paragraph::new("una dos tres")]));
        v.make_book();
        v.record_words(3);
        v.record_words(5);
        assert_eq!(v.doc.book.as_ref().unwrap().written_on(&today()), 2);
        assert_eq!(v.book.session_start, Some(3));
        v.open_goals();
        assert_eq!(v.book.goals.as_ref().unwrap().words, 0);
    }
}
