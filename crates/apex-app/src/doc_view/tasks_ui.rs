//! Tasks in a notebook page: check boxes with a due date and a priority
//! (kept in the box's tag), shown as a label after the task.

use apex_doc::notebook::tasks::{TOP_PRIORITY, TaskMeta, is_date};
use apex_doc::{ContentControl, ControlAt, ControlKind, UNCHECKED};

use super::*;

/// Today as yyyy-mm-dd.
pub fn today() -> String {
    let (y, m, d, _, _) = crate::common::local_now();
    format!("{y:04}-{m:02}-{d:02}")
}

/// `date` (yyyy-mm-dd) `n` days later.
pub fn add_days(date: &str, n: i64) -> String {
    use apex_sheet::format::{civil_from_days, days_from_civil};
    let part = |r: std::ops::Range<usize>| date.get(r).and_then(|s| s.parse::<u32>().ok()).unwrap_or(1);
    let (y, m, d) = civil_from_days(days_from_civil(part(0..4) as i64, part(5..7), part(8..10)) + n);
    format!("{y:04}-{m:02}-{d:02}")
}

pub fn priority_name(p: u8) -> String {
    tr(match p {
        0 => "task.pri0",
        1 => "task.pri1",
        2 => "task.pri2",
        _ => "task.pri3",
    })
    .to_string()
}

/// How a task's label looks: its text and color.
pub fn task_label(meta: &TaskMeta, done: bool, today: &str) -> Option<(String, Color32)> {
    let mut parts = Vec::new();
    if let Some(d) = &meta.due {
        parts.push(d.clone());
    }
    if meta.priority > 0 {
        parts.push(priority_name(meta.priority));
    }
    if parts.is_empty() {
        return None;
    }
    let color = match &meta.due {
        _ if done => Color32::from_gray(150),
        Some(d) if d.as_str() < today => Color32::from_rgb(0xD0, 0x40, 0x40),
        Some(d) if d == today => Color32::from_rgb(0xD0, 0x80, 0x10),
        _ if meta.priority == TOP_PRIORITY => Color32::from_rgb(0xC0, 0x50, 0x30),
        _ => Color32::from_rgb(0x50, 0x70, 0xA0),
    };
    Some((parts.join(" · "), color))
}

impl DocView {
    /// The task box of paragraph `para`: its first check box.
    fn task_box(&self, para: usize) -> Option<ControlAt> {
        apex_doc::para_controls(self.doc.paragraphs.get(para)?, para).into_iter().find(|c| matches!(c.control.kind, ControlKind::CheckBox { .. }))
    }

    /// A task box at the start of the caret's paragraph.
    pub fn insert_task(&mut self) {
        let para = self.sel.cursor.para;
        if self.task_box(para).is_some() {
            return;
        }
        let start = Pos::new(para, 0);
        let mut plain = self.doc.typing_attrs(start);
        plain.control = None;
        let offset = self.sel.cursor.offset;
        let id = self.doc.next_control_id();
        self.doc.insert_control(Selection::caret(start), ContentControl::new(id, ControlKind::CheckBox { checked: false }), &UNCHECKED.to_string());
        let after = Pos::new(para, UNCHECKED.len_utf8());
        self.doc.insert_text(Selection::caret(after), " ", Some(plain));
        self.set_sel(Selection::caret(Pos::new(para, offset + after.offset + 1)));
        self.focus = true;
    }

    /// Embedded: "[] " or "[ ] " typed at the start of a paragraph
    /// becomes a task box.
    pub(crate) fn check_task_prompt(&mut self, typed: &str) {
        if !self.embedded || typed != " " || !self.sel.is_empty() {
            return;
        }
        let c = self.sel.cursor;
        let before = &self.doc.paragraphs[c.para].text[..c.offset];
        if before == "[] " || before == "[ ] " {
            self.doc.insert_text(Selection { anchor: Pos::new(c.para, 0), cursor: c }, "", None);
            self.set_sel(Selection::caret(Pos::new(c.para, 0)));
            self.insert_task();
        }
    }

    fn set_task_meta(&mut self, at: &ControlAt, meta: TaskMeta) {
        let c = ContentControl { tag: meta.tag(), ..(*at.control).clone() };
        let text = self.doc.paragraphs[at.para].text[at.range.clone()].to_string();
        let p = self.doc.set_control(at, c, &text, self.sel);
        self.set_sel(Selection::caret(p));
    }

    /// Embedded: due date and priority of the task at the caret.
    pub(super) fn task_menu(&mut self, ui: &mut egui::Ui) {
        if !self.embedded {
            return;
        }
        let Some(at) = self.task_box(self.sel.cursor.para) else { return };
        let meta = TaskMeta::from_tag(&at.control.tag);
        let mut set = None;
        crate::widgets::menu_button(ui, tr("task.due"), |ui| {
            let today = today();
            for (key, days) in [("task.today", 0), ("task.tomorrow", 1), ("task.next_week", 7)] {
                let date = add_days(&today, days);
                if ui.radio(meta.due.as_deref() == Some(date.as_str()), format!("{}  ({date})", tr(key))).clicked() {
                    set = Some(TaskMeta { due: Some(date), ..meta.clone() });
                }
            }
            let id = Id::new(("task_date", self.uid));
            let mut text = ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_else(|| meta.due.clone().unwrap_or_else(|| today.clone()));
            ui.horizontal(|ui| {
                ui.label(tr("task.on"));
                let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(90.0).hint_text("2026-12-31"));
                let ok = is_date(text.trim());
                if ui.add_enabled(ok, egui::Button::new(tr("dialog.ok"))).clicked() || (ok && r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                    set = Some(TaskMeta { due: Some(text.trim().to_string()), ..meta.clone() });
                }
            });
            ui.data_mut(|d| d.insert_temp(id, text));
            if meta.due.is_some() && ui.button(tr("task.no_due")).clicked() {
                set = Some(TaskMeta { due: None, ..meta.clone() });
            }
        });
        crate::widgets::menu_button(ui, tr("task.priority"), |ui| {
            for p in (0..=TOP_PRIORITY).rev() {
                if ui.radio(meta.priority == p, priority_name(p)).clicked() {
                    set = Some(TaskMeta { priority: p, ..meta.clone() });
                }
            }
        });
        if let Some(m) = set {
            ui.data_mut(|d| d.remove::<String>(Id::new(("task_date", self.uid))));
            self.set_task_meta(&at, m);
            ui.close();
        }
    }

    /// Embedded: the task label after the last line of a paragraph with
    /// a task box.
    pub(super) fn paint_task_label(&self, p: &egui::Painter, para: &Paragraph, pl: &ParaLayout, seg: &Segment, galley_pos: Pos2) {
        if !self.embedded {
            return;
        }
        let Some(c) = apex_doc::para_controls(para, 0).into_iter().find(|c| matches!(c.control.kind, ControlKind::CheckBox { .. })) else { return };
        let done = matches!(c.control.kind, ControlKind::CheckBox { checked: true });
        let Some((text, color)) = task_label(&TaskMeta::from_tag(&c.control.tag), done, &today()) else { return };
        let Some(row) = pl.galley.rows.last() else { return };
        if row.pos.y < seg.gy0 - 0.5 || row.pos.y >= seg.gy1 - 0.5 {
            return;
        }
        let height = row.row.glyphs.first().map_or(row.row.size.y, |g| g.font_height);
        let font = egui::FontId::proportional((height * 0.72).max(8.0));
        let galley = p.layout_no_wrap(text, font, color);
        let o = galley_pos + row.pos.to_vec2();
        let width = pl.galley.job.wrap.max_width;
        let mut at = pos2(o.x + row.row.size.x + height * 0.5, o.y + (row.row.size.y - galley.size().y) / 2.0);
        // No room after the text: under its end, on the right.
        if width.is_finite() && row.pos.x + row.row.size.x + height * 0.5 + galley.size().x > width {
            at = pos2(galley_pos.x + width - galley.size().x, o.y + row.row.size.y);
        }
        let r = Rect::from_min_size(at, galley.size()).expand2(vec2(height * 0.25, 1.0));
        p.rect_filled(r, height * 0.2, color.gamma_multiply(0.15));
        p.galley(at, galley, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_task_label_tells_late_from_today() {
        assert_eq!(add_days("2026-12-30", 3), "2027-01-02");
        let late = TaskMeta { due: Some("2026-10-01".into()), priority: 0, col: None };
        let (text, red) = task_label(&late, false, "2026-10-03").unwrap();
        assert_eq!(text, "2026-10-01");
        assert_ne!(red, task_label(&late, true, "2026-10-03").unwrap().1);
        assert_eq!(task_label(&TaskMeta::default(), false, "2026-10-03"), None);
    }
}
