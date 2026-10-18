//! Thesaurus: the Synonyms pane (Shift+F7) and the context menu's
//! synonyms.

use crate::thesaurus::{self, Lookup};

use super::*;
use crate::widgets::FitWindow as _;

pub(super) struct ThesPane {
    query: String,
    /// The word being looked up (the query when searched).
    word: String,
    /// Words looked up before, for Back.
    back: Vec<String>,
    pick: Option<String>,
}

/// The synonym with the capitalization of the word it replaces.
fn match_case(word: &str, syn: &str) -> String {
    let mut w = word.chars();
    match w.next() {
        Some(c) if c.is_uppercase() && w.clone().all(|x| !x.is_alphabetic() || x.is_uppercase()) && word.chars().count() > 1 => syn.to_uppercase(),
        Some(c) if c.is_uppercase() => {
            let mut s = syn.chars();
            s.next().map(|f| f.to_uppercase().chain(s).collect()).unwrap_or_default()
        }
        _ => syn.to_string(),
    }
}

impl DocView {
    /// The word at the caret (or the selected text) and its extent.
    fn thes_word(&self) -> (Selection, String) {
        let s = self.sel_or_word();
        let text = if s.start().para == s.end().para { self.doc.text_range(s.start(), s.end()).replace('\u{AD}', "") } else { String::new() };
        (s, text.trim().to_string())
    }

    pub(super) fn open_thesaurus(&mut self) {
        let (_, w) = self.thes_word();
        self.thes = Some(ThesPane { query: w.clone(), word: w, back: Vec::new(), pick: None });
    }

    /// Replaces the word at the caret with a synonym.
    fn replace_with_synonym(&mut self, syn: &str) {
        let (s, w) = self.thes_word();
        if w.is_empty() {
            return;
        }
        let text = match_case(&w, syn);
        let attrs = self.doc.paragraphs[s.start().para].attrs_at(s.start().offset + w.chars().next().map_or(0, char::len_utf8));
        let p = self.doc.insert_text(Selection { anchor: s.start(), cursor: s.end() }, &text, Some(attrs));
        self.set_sel(Selection { anchor: Pos::new(p.para, p.offset - text.len()), cursor: p });
    }

    /// Synonyms in the context menu.
    pub(super) fn thesaurus_context_items(&mut self, ui: &mut egui::Ui) {
        let (_, w) = self.thes_word();
        if w.is_empty() || w.contains(char::is_whitespace) && w.chars().count() > 40 {
            return;
        }
        crate::widgets::menu_button(ui, tr("thes.menu"), |ui| {
            let syns = thesaurus::quick(ui.ctx(), &w, 10);
            if syns.is_empty() {
                ui.weak(tr("thes.none_short"));
            }
            for s in syns {
                if ui.button(&s).clicked() {
                    self.replace_with_synonym(&s);
                    ui.close();
                }
            }
            ui.separator();
            if ui.add(egui::Button::new(format!("{}…", tr("thes.title"))).shortcut_text("Shift+F7")).clicked() {
                self.open_thesaurus();
                ui.close();
            }
        });
    }

    /// The Synonyms pane.
    pub(super) fn thesaurus_ui(&mut self, ctx: &egui::Context) {
        let Some(mut pane) = self.thes.take() else { return };
        let mut open = true;
        let mut insert = None;
        let screen = ctx.content_rect();
        crate::widgets::esc_closes(ctx, Id::new(("thes", self.uid)), &mut open);
        egui::Window::new(tr("thes.title"))
            .id(Id::new(("thes", self.uid)))
            .open(&mut open)
            .default_pos(pos2(screen.max.x - 320.0, screen.min.y + 180.0))
            .default_size(vec2(280.0, 420.0))
            .resizable(true)
            .collapsible(false)
            .show_fit(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.add_enabled(!pane.back.is_empty(), egui::Button::new("⏴")).on_hover_text(tr("thes.back")).clicked()
                        && let Some(prev) = pane.back.pop()
                    {
                        pane.query = prev.clone();
                        pane.word = prev;
                        pane.pick = None;
                    }
                    let r = ui.add(egui::TextEdit::singleline(&mut pane.query).desired_width(ui.available_width() - 70.0));
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    // Searching again for the word shown does nothing.
                    let other = pane.query.trim() != pane.word;
                    if (ui.add_enabled(other, egui::Button::new(tr("thes.search"))).clicked() || enter) && other {
                        pane.back.push(std::mem::take(&mut pane.word));
                        pane.word = pane.query.trim().to_string();
                        pane.pick = None;
                    }
                });
                ui.separator();
                // The list takes what the buttons below leave, as tall as they
                // were last time: filling the window made it grow each frame.
                let foot_id = Id::new(("thes_foot", self.uid));
                let foot = ui.ctx().data(|d| d.get_temp::<f32>(foot_id)).unwrap_or(40.0);
                let mut look = None;
                egui::ScrollArea::vertical().auto_shrink([false, false]).max_height((ui.available_height() - foot).max(40.0)).show(ui, |ui| match thesaurus::lookup(ctx, &pane.word) {
                    _ if pane.word.is_empty() => {
                        ui.weak(tr("thes.type_word"));
                    }
                    Lookup::Loading => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(tr("thes.loading"));
                        });
                    }
                    Lookup::Missing => {
                        let lang = crate::spell::language_name(&crate::spell::language());
                        ui.label(crate::i18n::trf("thes.missing", &[("lang", &lang)]));
                    }
                    Lookup::Found(m) if m.is_empty() => {
                        ui.label(crate::i18n::trf("thes.no_results", &[("word", &pane.word)]));
                    }
                    Lookup::Found(m) => {
                        for meaning in m {
                            ui.label(egui::RichText::new(meaning.pos.trim_matches(['(', ')'])).small().weak());
                            ui.horizontal_wrapped(|ui| {
                                for w in &meaning.words {
                                    let r = ui.selectable_label(pane.pick.as_deref() == Some(w.as_str()), w);
                                    if r.clicked() {
                                        pane.pick = Some(w.clone());
                                    }
                                    if r.double_clicked() {
                                        look = Some(w.clone());
                                    }
                                }
                            });
                            ui.add_space(4.0);
                        }
                    }
                });
                let top = ui.cursor().min.y;
                ui.separator();
                ui.horizontal(|ui| {
                    let can = pane.pick.is_some() && !self.thes_word().1.is_empty();
                    if ui.add_enabled(can, egui::Button::new(tr("thes.insert"))).on_hover_text(tr("thes.insert_hint")).clicked() {
                        insert = pane.pick.clone();
                    }
                    if ui.add_enabled(pane.pick.is_some(), egui::Button::new(tr("thes.copy"))).clicked()
                        && let Some(p) = &pane.pick
                    {
                        ui.ctx().copy_text(p.clone());
                    }
                });
                let h = ui.min_rect().max.y - top + ui.spacing().item_spacing.y;
                ui.ctx().data_mut(|d| d.insert_temp(foot_id, h));
                if let Some(w) = look {
                    pane.back.push(std::mem::replace(&mut pane.word, w.clone()));
                    pane.query = w;
                    pane.pick = None;
                }
            });
        if let Some(s) = insert {
            self.replace_with_synonym(&s);
            self.focus = true;
        }
        if open {
            self.thes = Some(pane);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::match_case;

    #[test]
    fn keeps_case() {
        assert_eq!(match_case("Casa", "hogar"), "Hogar");
        assert_eq!(match_case("CASA", "hogar"), "HOGAR");
        assert_eq!(match_case("casa", "hogar"), "hogar");
        assert_eq!(match_case("A", "hogar"), "Hogar");
    }

    /// The window keeps its size frame after frame (it grew a little each
    /// one, without end).
    #[test]
    fn the_window_does_not_grow() {
        use super::super::DocView;
        use eframe::egui;
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let mut v = DocView::new(apex_doc::Document::from_paragraphs(vec![apex_doc::Paragraph::new("noche")]));
        v.open_thesaurus();
        let input = || egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 900.0))), ..Default::default() };
        let mut sizes = Vec::new();
        for _ in 0..60 {
            ctx.run_ui(input(), |ui| v.thesaurus_ui(ui.ctx())).textures_delta.clear();
            let id = egui::Id::new(("thes", v.uid));
            sizes.push(ctx.memory(|m| m.area_rect(id)).map(|r| r.height()));
        }
        assert_eq!(sizes[10], sizes[59], "{sizes:?}");
    }
}
