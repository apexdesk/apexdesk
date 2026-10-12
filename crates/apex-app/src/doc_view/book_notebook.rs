//! A notebook beside a book: the pages of the notebooks open as cards in a
//! pane, and the card of a name written in the manuscript on hover.

use super::*;
use crate::note_view::shelf::{self, Card};

/// Text of a card shown on hover.
const HOVER_TEXT: usize = 280;
/// Seconds over a name before its card shows.
const HOVER_DELAY: f64 = 0.4;

#[derive(Default)]
pub(super) struct Shelf {
    pub open: bool,
    query: String,
    /// The card read in full: its notebook view and page.
    pick: Option<(u64, i64)>,
    /// For the app: open a notebook, or go to a card's page.
    pub ask: Option<common::AppCmd>,
    /// The names to look for (folded) and their card, for a revision of
    /// the cards.
    names: Option<(u64, Vec<String>, Vec<usize>)>,
    /// The card of the name under the pointer, and since when.
    hover: Option<((u64, i64), f64)>,
}

/// A card: its type, title, other names, where it is, fields and text.
pub(super) fn card_ui(ui: &mut egui::Ui, c: &Card, full: bool) {
    if let Some((name, color)) = &c.kind {
        let [r, g, b] = [(color >> 16) as u8, (color >> 8) as u8, *color as u8];
        ui.label(RichText::new(format!("● {name}")).small().color(Color32::from_rgb(r, g, b)));
    }
    ui.label(RichText::new(&c.title).strong().size(15.0));
    if !c.aliases.is_empty() {
        ui.label(RichText::new(c.aliases.join(", ")).italics().weak());
    }
    ui.label(RichText::new(format!("{} › {}", c.notebook, c.section)).small().weak());
    if !c.fields.is_empty() {
        ui.add_space(4.0);
        egui::Grid::new(("card_fields", c.uid, c.page)).num_columns(2).spacing(vec2(10.0, 2.0)).show(ui, |ui| {
            for (k, v) in &c.fields {
                ui.label(RichText::new(k).weak());
                ui.label(v);
                ui.end_row();
            }
        });
    }
    if !c.text.is_empty() {
        ui.add_space(4.0);
        let text = match c.text.char_indices().nth(HOVER_TEXT).filter(|_| !full) {
            Some((i, _)) => format!("{}…", &c.text[..i]),
            None => c.text.clone(),
        };
        ui.label(text);
    }
}

fn matches(c: &Card, q: &str) -> bool {
    let f = apex_doc::notebook::fold;
    q.is_empty() || std::iter::once(&c.title).chain(&c.aliases).chain(c.fields.iter().map(|f| &f.1)).chain([&c.text]).any(|s| f(s).contains(q))
}

impl DocView {
    pub(super) fn toggle_notebook_pane(&mut self) {
        self.book.shelf.open = !self.book.shelf.open;
        self.book.shelf.pick = None;
    }

    /// Asks for a notebook to open beside the book.
    fn pick_notebook(&self) {
        crate::file_dialog::FileDialog::new()
            .set_title(tr("book.nb_open").replace('…', ""))
            .add_filter(tr("nb.filter"), crate::note_view::EXTS)
            .pick_file(crate::file_dialog::in_view(|v: &mut DocView, p: std::path::PathBuf| {
                v.book.shelf.ask = Some(common::AppCmd::OpenNoteBeside(p));
            }));
    }

    /// The card of the name written under `local` (a point of the page),
    /// in a book.
    pub(super) fn card_under(&mut self, local: Pos2) -> Option<Card> {
        let (rev, cards) = shelf::cards();
        if cards.is_empty() || !self.is_book() {
            return None;
        }
        let at = self.char_under(local)?;
        if self.book.shelf.names.as_ref().is_none_or(|n| n.0 != rev) {
            let (mut names, mut owner) = (Vec::new(), Vec::new());
            for (i, c) in cards.iter().enumerate() {
                for n in c.names() {
                    names.push(n);
                    owner.push(i);
                }
            }
            self.book.shelf.names = Some((rev, names, owner));
        }
        let (_, names, owner) = self.book.shelf.names.as_ref()?;
        let (i, _) = apex_doc::notebook::links::name_at(&self.doc.paragraphs[at.para].text, at.offset, names)?;
        cards.get(owner[i]).cloned()
    }

    /// The card of the name under the pointer (`at`, a point of the page;
    /// `pointer`, on the screen), once it has rested there.
    pub(super) fn hover_card(&mut self, ctx: &egui::Context, at: Option<Pos2>, pointer: Pos2) {
        let card = at.and_then(|at| self.card_under(at));
        let now = ctx.input(|i| i.time);
        let Some(c) = card else {
            self.book.shelf.hover = None;
            return;
        };
        let key = (c.uid, c.page);
        let since = match self.book.shelf.hover {
            Some((k, t)) if k == key => t,
            _ => {
                self.book.shelf.hover = Some((key, now));
                now
            }
        };
        if now - since < HOVER_DELAY {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(HOVER_DELAY - (now - since) + 0.02));
            return;
        }
        egui::Area::new(Id::new(("book_card", self.uid))).order(egui::Order::Tooltip).fixed_pos(pointer + vec2(14.0, 18.0)).interactable(false).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(320.0);
                card_ui(ui, &c, false);
            });
        });
    }

    pub(super) fn notebook_pane(&mut self, ui: &mut egui::Ui) {
        if !self.is_book() || !self.book.shelf.open {
            return;
        }
        let (_, cards) = shelf::cards();
        let mut sh = std::mem::take(&mut self.book.shelf);
        let mut pick_file = false;
        let frame = egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(8, 6));
        egui::Panel::right(Id::new(("book_notebook", self.uid))).resizable(true).default_size(280.0).frame(frame).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(tr("book.notebook")).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("×").on_hover_text(tr("book.close")).clicked() {
                        sh.open = false;
                    }
                });
            });
            if cards.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new(tr("book.nb_none")).weak());
                ui.add_space(6.0);
                pick_file = ui.button(tr("book.nb_open")).clicked();
                return;
            }
            if let Some(c) = sh.pick.and_then(|(u, p)| cards.iter().find(|c| c.uid == u && c.page == p)) {
                ui.horizontal(|ui| {
                    if ui.button(format!("‹ {}", tr("book.nb_back"))).clicked() {
                        sh.pick = None;
                    }
                    if ui.button(tr("book.nb_go")).on_hover_text(tr("book.nb_go_tip")).clicked() {
                        sh.ask = Some(common::AppCmd::ShowNotePage(c.uid, c.page));
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical().id_salt("book_nb_card").auto_shrink([false, false]).show(ui, |ui| card_ui(ui, c, true));
                return;
            }
            sh.pick = None;
            ui.add(egui::TextEdit::singleline(&mut sh.query).hint_text(tr("book.nb_find")).desired_width(f32::INFINITY));
            ui.label(RichText::new(tr("book.nb_hover")).small().weak());
            ui.separator();
            let q = apex_doc::notebook::fold(sh.query.trim());
            let several = cards.iter().any(|c| c.uid != cards[0].uid);
            let bottom = 28.0;
            egui::ScrollArea::vertical().id_salt("book_nb_list").auto_shrink([false, false]).max_height(ui.available_height() - bottom).show(ui, |ui| {
                let mut last: Option<u64> = None;
                let mut any = false;
                for c in cards.iter().filter(|c| matches(c, &q)) {
                    if several && last != Some(c.uid) {
                        ui.label(RichText::new(&c.notebook).small().strong());
                        last = Some(c.uid);
                    }
                    any = true;
                    let label = match &c.kind {
                        Some((k, _)) => format!("{}  ·  {k}", c.title),
                        None => c.title.clone(),
                    };
                    if ui.selectable_label(false, label).on_hover_ui(|ui| card_ui(ui, c, false)).clicked() {
                        sh.pick = Some((c.uid, c.page));
                    }
                }
                if !any {
                    ui.label(RichText::new(tr("book.nb_no_match")).weak());
                }
            });
            pick_file = ui.small_button(tr("book.nb_open_other")).clicked();
        });
        self.book.shelf = sh;
        if pick_file {
            self.pick_notebook();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_is_found_by_any_of_its_words() {
        let c = Card {
            uid: 1,
            page: 2,
            notebook: "Mundo".into(),
            section: "Personajes".into(),
            title: "Ana Soler".into(),
            aliases: vec!["la farera".into()],
            kind: Some(("Personaje".into(), 0x3366CC)),
            fields: vec![("Edad".into(), "34".into())],
            text: "Vive en el faro.".into(),
        };
        for q in ["", "soler", "farera", "34", "faro"] {
            assert!(matches(&c, q), "{q}");
        }
        assert!(!matches(&c, "puerto"));
        assert_eq!(c.names().collect::<Vec<_>>(), ["ana soler", "la farera"]);
    }
}
