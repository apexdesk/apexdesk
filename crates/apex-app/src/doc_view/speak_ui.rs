//! Read Aloud in documents: reads from the caret (or the selection) on,
//! paragraph by paragraph, highlighting the paragraph and the word being
//! read and keeping them in view.

use super::*;
use crate::read_aloud::{Item, Reader, Tick};

#[derive(Default)]
pub struct SpeakState {
    reader: Option<Reader>,
    /// Paragraph and byte range of each item.
    map: Vec<(usize, usize, usize)>,
}

/// The paragraph being read (its read byte range) and the word.
pub(super) struct Spoken {
    pub para: usize,
    pub range: (usize, usize),
    pub word: Option<(usize, usize)>,
}

impl DocView {
    pub(super) fn reading_aloud(&self) -> bool {
        self.speak.reader.is_some()
    }

    /// Review > Read Aloud: starts reading, or stops.
    pub(super) fn toggle_read_aloud(&mut self, ctx: &egui::Context) {
        if let Some(mut r) = self.speak.reader.take() {
            r.stop();
            return;
        }
        crate::speech::start(ctx);
        let (a, b) = if self.sel.is_empty() { (self.sel.cursor, None) } else { (self.sel.start(), Some(self.sel.end())) };
        let last = b.map_or(self.doc.paragraphs.len().saturating_sub(1), |b| b.para);
        let fallback = crate::read_aloud::default_lang();
        let mut items = Vec::new();
        let mut map = Vec::new();
        for i in a.para..=last.min(self.doc.paragraphs.len().saturating_sub(1)) {
            let p = &self.doc.paragraphs[i];
            let from = if i == a.para { a.offset.min(p.text.len()) } else { 0 };
            let to = match b {
                Some(b) if b.para == i => b.offset.min(p.text.len()),
                _ => p.text.len(),
            };
            let (Some(text), true) = (p.text.get(from..to), from <= to) else { continue };
            items.push(Item { text: text.to_string(), lang: para_lang(p, from).unwrap_or_else(|| fallback.clone()) });
            map.push((i, from, to));
        }
        if items.iter().all(|i| i.text.trim().is_empty()) {
            self.notices.push(Notice::info(tr("speak.nothing_doc")));
            return;
        }
        self.speak = SpeakState { reader: Some(Reader::new(items)), map };
    }

    /// Follows the reading and shows the player at the top of `area`.
    pub(super) fn read_aloud_frame(&mut self, ctx: &egui::Context, area: Rect) {
        let Some(r) = &mut self.speak.reader else { return };
        match r.tick(ctx) {
            Tick::Nothing => {}
            Tick::Moved => {
                if let Some(&(para, base, _)) = self.speak.map.get(r.cur) {
                    let at = base + r.word.map_or(0, |w| w.0);
                    self.sel = Selection::caret(self.doc.clamp(Pos::new(para, at)));
                    self.scroll_to_caret = true;
                }
            }
            Tick::Finished => self.speak.reader = None,
            Tick::Missing(why) => {
                self.speak.reader = None;
                self.notices.push(Notice::error(trf("speak.no_voice", &[("why", &why)])));
            }
        }
        if let Some(r) = &mut self.speak.reader
            && r.bar(ctx, area)
        {
            self.speak.reader = None;
        }
    }

    /// What is being read, for painting.
    pub(super) fn spoken(&self) -> Option<Spoken> {
        let r = self.speak.reader.as_ref()?;
        let &(para, from, to) = self.speak.map.get(r.cur)?;
        self.doc.paragraphs.get(para)?;
        // The text may have changed meanwhile: whole characters only.
        let at = |b: usize| self.doc.clamp(Pos::new(para, b)).offset;
        Some(Spoken { para, range: (at(from), at(to)), word: r.word.map(|(a, b)| (at(from + a), at(from + b))) })
    }
}

/// The language of paragraph text at byte `at` (from its run), if marked.
fn para_lang(p: &Paragraph, at: usize) -> Option<String> {
    let mut end = 0;
    for r in &p.runs {
        end += r.len;
        if at < end || end == p.text.len() {
            return r.attrs.lang.as_deref().map(crate::speech::norm_lang);
        }
    }
    None
}
