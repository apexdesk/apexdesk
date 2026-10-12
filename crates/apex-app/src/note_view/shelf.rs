//! What a book sees of the notebooks open: a card for each page, with its
//! names, type, fields and text. The book shows them in a pane beside the
//! manuscript, and the card of a name written in it on hover.

use std::sync::{Arc, Mutex};

use apex_doc::notebook::fold;

use super::*;

/// Text kept of a page for its card.
const TEXT_KEPT: usize = 1200;

#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    /// The notebook view it is in, and the page.
    pub uid: u64,
    pub page: i64,
    pub notebook: String,
    pub section: String,
    pub title: String,
    pub aliases: Vec<String>,
    /// Its type's name and color.
    pub kind: Option<(String, u32)>,
    pub fields: Vec<(String, String)>,
    pub text: String,
}

impl Card {
    /// Its title and aliases, folded, as a name in a text is looked for.
    pub fn names(&self) -> impl Iterator<Item = String> + '_ {
        std::iter::once(&self.title).chain(&self.aliases).map(|n| fold(n.trim())).filter(|n| n.chars().count() >= 3)
    }
}

/// The cards of each notebook open, by its view.
static OPEN: Mutex<Vec<(u64, Vec<Card>)>> = Mutex::new(Vec::new());
/// Every card, and a number that changes when they do.
static ALL: Mutex<(u64, Option<Arc<Vec<Card>>>)> = Mutex::new((0, None));

/// The cards of all notebooks open, and their revision.
pub fn cards() -> (u64, Arc<Vec<Card>>) {
    let Ok(mut all) = ALL.lock() else { return (0, Arc::default()) };
    if all.1.is_none() {
        let v: Vec<Card> = OPEN.lock().map(|o| o.iter().flat_map(|(_, v)| v.iter().cloned()).collect()).unwrap_or_default();
        all.1 = Some(Arc::new(v));
    }
    (all.0, all.1.clone().unwrap_or_default())
}

fn set(uid: u64, mine: Option<Vec<Card>>) {
    let Ok(mut o) = OPEN.lock() else { return };
    let at = o.iter().position(|(u, _)| *u == uid);
    if at.map(|i| &o[i].1) == mine.as_ref() {
        return;
    }
    if let Some(i) = at {
        o.remove(i);
    }
    if let Some(v) = mine.filter(|v| !v.is_empty()) {
        o.push((uid, v));
    }
    drop(o);
    if let Ok(mut all) = ALL.lock() {
        all.0 += 1;
        all.1 = None;
    }
}

impl NoteView {
    /// Tells the books open what this notebook's pages are.
    pub(super) fn publish_cards(&self) {
        let notebook = self.store.name();
        let mine: Vec<Card> = self
            .nb
            .pages
            .iter()
            .filter(|p| self.nb.is_live(p.id) && !p.title.trim().is_empty())
            .map(|p| {
                let text = self.nb.texts.get(&p.id).map(|t| t.trim()).unwrap_or_default();
                let cut = text.char_indices().nth(TEXT_KEPT).map_or(text.len(), |(i, _)| i);
                Card {
                    uid: self.uid,
                    page: p.id,
                    notebook: notebook.clone(),
                    section: self.nb.section(p.section).map(|s| s.name.clone()).unwrap_or_default(),
                    title: p.title.clone(),
                    aliases: p.aliases.clone(),
                    kind: p.ptype.and_then(|t| self.nb.page_type(t)).map(|t| (t.name.clone(), t.color)),
                    fields: p.fields.iter().filter(|f| !f.1.trim().is_empty()).cloned().collect(),
                    text: if cut < text.len() { format!("{}…", &text[..cut]) } else { text.to_string() },
                }
            })
            .collect();
        set(self.uid, Some(mine));
    }

    pub(super) fn withdraw_cards(&self) {
        set(self.uid, None);
    }

    pub fn uid(&self) -> u64 {
        self.uid
    }

    /// Opens page `id`, asked from a book.
    pub fn go_to_page(&mut self, id: i64) {
        if self.nb.is_live(id) {
            self.close_graph();
            self.open_page(id);
            self.grid = false;
        }
    }
}
