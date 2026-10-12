//! The Review menu of documents and the Word Count dialog.

use apex_doc::{ParaKind, Pos, Selection};
use eframe::egui::{self, Key, Modifiers};

use super::DocView;
use crate::autocorrect;
use crate::i18n::tr;

/// Word Count figures (of the selection when there is one).
pub(super) struct Counts {
    selection: bool,
    pages: usize,
    words: usize,
    chars: usize,
    chars_no_spaces: usize,
    paragraphs: usize,
    lines: usize,
}

impl DocView {

    pub(super) fn review_key(&mut self, key: Key, m: Modifiers) -> bool {
        if self.comment_key(key, m) || self.revision_key(key, m) {
            return true;
        }
        match key {
            Key::G if m.command && m.shift => self.open_word_count(),
            _ => return false,
        }
        true
    }

    pub(super) fn open_word_count(&mut self) {
        let (a, b) = if self.sel.is_empty() { (Pos::new(0, 0), self.doc.end()) } else { (self.sel.start(), self.sel.end()) };
        let mut c = Counts { selection: !self.sel.is_empty(), pages: 0, words: 0, chars: 0, chars_no_spaces: 0, paragraphs: 0, lines: 0 };
        for i in a.para..=b.para {
            let p = &self.doc.paragraphs[i];
            let s = if i == a.para { a.offset } else { 0 };
            let e = if i == b.para { b.offset } else { p.len() };
            // Deleted text (tracked changes) doesn't count.
            let visible = p.visible(s, e);
            let t: &str = &visible;
            c.words += apex_doc::count_words(t);
            // Pictures aren't characters.
            let chars = t.chars().filter(|&ch| ch != apex_doc::OBJECT);
            let (n, blank) = chars.fold((0, 0), |(n, b), ch| (n + 1, b + usize::from(ch.is_whitespace())));
            c.chars += n;
            c.chars_no_spaces += n - blank;
            c.paragraphs += usize::from(!t.trim().is_empty());
        }
        // Lines and pages come from the layout.
        if let (Some(first), Some(last)) = (self.layout.paras.get(a.para), self.layout.paras.get(b.para)) {
            let row_of = |pl: &super::ParaLayout, off: usize, text: &str| {
                let ci = super::char_count(text, off);
                pl.galley.layout_from_cursor(egui::text::CCursor::new(ci)).row
            };
            for i in a.para..=b.para {
                let pl = &self.layout.paras[i];
                let text = &self.doc.paragraphs[i].text;
                let r0 = if i == a.para { row_of(first, a.offset, text) } else { 0 };
                let r1 = if i == b.para { row_of(last, b.offset, text) } else { pl.galley.rows.len().saturating_sub(1) };
                c.lines += r1.saturating_sub(r0) + 1;
            }
            let page_of = |para: usize| self.layout.segs_of(para).first().map_or(0, |s| s.page);
            c.pages = if c.selection { page_of(b.para) - page_of(a.para) + 1 } else { self.layout.pages };
        }
        self.word_count = Some(c);
    }

    pub(super) fn word_count_dialog(&mut self, ctx: &egui::Context) {
        let Some(c) = &self.word_count else { return };
        let closed = crate::widgets::info_dialog(ctx, "word_count", tr("count.title"), 340.0, |ui| {
            crate::widgets::section(ui, tr(if c.selection { "count.selection" } else { "count.statistics" }));
            egui::Grid::new("counts").num_columns(2).min_col_width(crate::widgets::FORM_LABEL_W).spacing([40.0, 8.0]).show(ui, |ui| {
                for (key, n) in [
                    ("count.pages", c.pages),
                    ("count.words", c.words),
                    ("count.chars_no_spaces", c.chars_no_spaces),
                    ("count.chars", c.chars),
                    ("count.paragraphs", c.paragraphs),
                    ("count.lines", c.lines),
                ] {
                    ui.label(tr(key));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.strong(n.to_string()));
                    ui.end_row();
                }
            });
        });
        if closed {
            self.word_count = None;
            self.refocus = true;
        }
    }
}

// --- AutoCorrect ------------------------------------------------------------------------------

impl DocView {
    /// Typed text after AutoCorrect's smart quotes.
    pub(super) fn autocorrect_typed<'a>(&self, text: &'a str) -> std::borrow::Cow<'a, str> {
        if !matches!(text, "\"" | "'") || !autocorrect::options().smart_quotes {
            return text.into();
        }
        let a = self.sel.start();
        let prev = self.doc.paragraphs[a.para].text[..a.offset].chars().next_back();
        let q = text.chars().next().unwrap_or('"');
        autocorrect::smart_quote(prev, q).to_string().into()
    }

    /// AutoCorrect of the word that ends at `end` (just typed `typed` after
    /// it, or pressed Enter when `typed` is empty); returns the new end.
    pub(super) fn autocorrect_after(&mut self, end: Pos, typed: &str) -> Pos {
        let o = autocorrect::options();
        let para = end.para;
        let Some(t) = self.doc.paragraphs.get(para).map(|p| p.text.clone()) else { return end };
        if self.doc.paragraphs[para].style.kind == ParaKind::Code {
            return end;
        }
        let word_end = end.offset;
        let start = t[..word_end].rfind(char::is_whitespace).map_or(0, |i| i + t[i..].chars().next().map_or(1, char::len_utf8));
        let word = &t[start..word_end];
        if word.is_empty() {
            return end;
        }
        // Lists: "* " or "1. " at the start of a plain paragraph.
        if o.lists && typed == " " && start == 0 && !self.doc.track_changes && self.doc.paragraphs[para].style.kind == ParaKind::Normal
            && let Some((kind, format)) = autocorrect::list_marker(word)
        {
            let len = word.len() + typed.len();
            let sel = Selection { anchor: Pos::new(para, 0), cursor: Pos::new(para, len.min(t.len())) };
            let p = self.doc.insert_text(sel, "", None);
            self.sel = Selection::caret(p);
            self.set_list(kind, format);
            return p;
        }
        // Punctuation around the word stays: "(teh)" -> "(the)".
        let lead = word.find(|c: char| c.is_alphanumeric() || c == '(').unwrap_or(word.len());
        let trail = word.rfind(|c: char| c.is_alphanumeric() || c == ')' || c == '.').map_or(lead, |i| i + word[i..].chars().next().map_or(1, char::len_utf8)).max(lead);
        let (mut a, mut b) = (start + lead, start + trail);
        let mut with = t[a..b].to_string();
        if o.replace {
            let symbol_suffix = || {
                // "..." or "--" stuck to a word: "done..." -> "done…".
                o.entries.iter().filter(|(k, _)| !k.chars().any(char::is_alphanumeric) && word.len() > k.len() && word.ends_with(k.as_str())).max_by_key(|(k, _)| k.len()).map(|(k, v)| (word_end - k.len(), word_end, v.clone()))
            };
            let found = autocorrect::replacement(&o.entries, word)
                .map(|r| (start, word_end, r))
                .or_else(|| autocorrect::replacement(&o.entries, &word[lead..trail]).map(|r| (start + lead, start + trail, r)))
                .or_else(symbol_suffix);
            if let Some((ra, rb, r)) = found {
                let r = if o.smart_quotes { r.replace('\'', "’") } else { r };
                // Keep the word's start in the span so it can get a capital.
                let from = ra.min(start + lead);
                with = format!("{}{r}{}", &t[from..ra], &t[rb.max(ra)..(start + trail).max(rb)]);
                (a, b) = (from, rb.max(start + trail));
            }
        }
        if o.capitalize && a == start + lead {
            let first_word = with.split(char::is_whitespace).next().unwrap_or("").trim_end_matches('.');
            if autocorrect::wants_capital(first_word) && autocorrect::starts_sentence(&t[..a]) {
                let n = with.chars().next().map_or(0, char::len_utf8);
                with = with[..n].to_uppercase() + &with[n..];
            }
        }
        if with == t[a..b] {
            return end;
        }
        let caret = self.sel;
        let new_b = self.doc.insert_text(Selection { anchor: Pos::new(para, a), cursor: Pos::new(para, b) }, &with, None);
        // Where the text after the word moved (tracked deletions stay).
        let delta = new_b.offset as isize - b as isize;
        // Keep the caret after what was typed.
        let fix = |p: Pos| if p.para == para && p.offset >= b { Pos::new(para, (p.offset as isize + delta) as usize) } else { p };
        self.sel = Selection { anchor: fix(caret.anchor), cursor: fix(caret.cursor) };
        fix(end)
    }
}
