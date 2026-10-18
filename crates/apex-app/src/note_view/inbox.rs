//! Ideas caught from any tab with a shortcut (Ctrl+Shift+I): each a new
//! page of the section shown in the notebook.

use super::*;
use crate::widgets::FitWindow as _;

/// Longest title an idea takes from its first line.
const TITLE_CHARS: usize = 60;

/// An idea being written in the capture window.
pub struct Capture {
    pub text: String,
    focused: bool,
}

/// What the capture window ended with.
pub enum Outcome {
    Open,
    Keep(String),
    Cancel,
}

impl Capture {
    pub fn new(text: String) -> Capture {
        Capture { text, focused: false }
    }

    /// The capture window; `target` names where it goes.
    pub fn ui(&mut self, ctx: &egui::Context, target: Option<&str>) -> Outcome {
        let (keep_keys, esc) = ctx.input_mut(|i| (i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        let mut out = if esc { Outcome::Cancel } else { Outcome::Open };
        egui::Window::new(tr("nb.capture")).id(Id::new("note_capture")).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_TOP, vec2(0.0, 90.0)).show_fit(ctx, |ui| {
            ui.set_width(480.0);
            match target {
                Some(t) => ui.label(RichText::new(trf("nb.capture_to", &[("place", &t)])).weak()),
                None => ui.label(RichText::new(tr("nb.capture_nowhere")).color(ui.visuals().warn_fg_color)),
            };
            let te = ui.add(egui::TextEdit::multiline(&mut self.text).desired_rows(7).desired_width(f32::INFINITY).hint_text(tr("nb.capture_hint")));
            if !self.focused {
                te.request_focus();
                self.focused = true;
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new(tr("nb.capture_keys")).weak().small());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("dialog.cancel")).clicked() {
                        out = Outcome::Cancel;
                    }
                    let ready = target.is_some() && !self.text.trim().is_empty();
                    if ui.add_enabled(ready, egui::Button::new(tr("nb.capture_keep"))).clicked() || (ready && keep_keys) {
                        out = Outcome::Keep(self.text.trim().to_string());
                    }
                });
            });
        });
        out
    }
}

/// The title an idea takes: its first line, cut short.
pub fn idea_title(text: &str) -> String {
    let first = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
    let mut t: String = first.chars().take(TITLE_CHARS).collect();
    if first.chars().count() > TITLE_CHARS {
        t.push('…');
    }
    t
}

impl NoteView {
    /// Where the capture window says an idea goes: the section shown.
    pub fn capture_place(&self) -> Option<String> {
        let s = self.here_section()?;
        self.nb.section(s).map(|s| s.name.clone())
    }

    /// Keeps `text` as a new page on top of the section shown; the page
    /// open stays open.
    pub fn capture(&mut self, text: &str) -> i64 {
        self.flush();
        let steps = self.undo.len();
        let section = self.here();
        let title = idea_title(text);
        let mut id = 0;
        self.change(|nb| {
            let first = nb.children(section, None).first().map(|p| p.id);
            id = nb.add_page(section, None, None, &title, now());
            nb.move_page(id, section, None, first);
        });
        self.undo.truncate(steps + 1);
        let doc = Document::from_paragraphs(text.lines().map(Paragraph::new).collect());
        let plain = plain_text(&doc);
        let found = apex_doc::notebook::links::page_links(&doc);
        let r = self.store.save_page(id, &doc, &plain, &found, now());
        self.nb.texts.insert(id, plain);
        self.nb.links.insert(id, found);
        self.nb.tasks.insert(id, apex_doc::notebook::tasks::tasks_in(&doc));
        self.report(r);
        if self.page.is_none() {
            self.open_page(id);
        }
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_idea_is_titled_by_its_first_line() {
        assert_eq!(idea_title("\n  La lonja  \nmás"), "La lonja");
        assert_eq!(idea_title(&"a".repeat(70)).chars().count(), TITLE_CHARS + 1);
    }
}
