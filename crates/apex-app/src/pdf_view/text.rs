//! The pages' text, read by `apex_io::pdf` in the background: selecting
//! and copying it, and searching it.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use apex_io::pdf::{Char, Page};
use eframe::egui::{self, Key};

use super::{PdfView, Selection};
use crate::i18n::{tr, trf};

#[derive(Default)]
pub struct TextLayer {
    /// The pages as read, and the revision they are of.
    pub pages: Option<Arc<Vec<Page>>>,
    rev: u64,
    asked: u64,
    rx: Option<Receiver<(u64, Vec<Page>)>>,
}

impl TextLayer {
    /// Reads the text of revision `rev` when it is not read yet; takes it
    /// when ready. `pictures`: with them (to draw the pages without pdfium).
    pub fn poll(&mut self, bytes: &Arc<[u8]>, password: Option<&str>, rev: u64, pictures: bool) {
        if let Some(rx) = &self.rx
            && let Ok((r, pages)) = rx.try_recv()
        {
            self.pages = Some(Arc::new(pages));
            self.rev = r;
            self.rx = None;
        }
        if self.rev == rev || self.asked == rev {
            return;
        }
        self.asked = rev;
        let (tx, rx) = channel();
        let bytes = bytes.clone();
        let pw = password.map(str::to_string);
        let spawned = apex_fs::thread::Builder::new().name("pdf-text".into()).spawn(move || {
            let pages = if pictures { apex_io::pdf::pages(&bytes, pw.as_deref()) } else { apex_io::pdf::text_pages(&bytes, pw.as_deref()) };
            let _ = tx.send((rev, pages.unwrap_or_default()));
        });
        if spawned.is_ok() {
            self.rx = Some(rx);
        }
    }

    pub fn page(&self, i: usize) -> Option<&Page> {
        self.pages.as_ref().and_then(|p| p.get(i))
    }
}

/// A character's box (x0, y0, x1, y1) on the page as shown.
pub fn char_box(c: &Char) -> [f32; 4] {
    let s = c.size.max(1.0);
    if c.upright {
        [c.x, c.y - s * 0.8, c.x + c.w.max(s * 0.2), c.y + s * 0.22]
    } else {
        [c.x - s * 0.5, c.y - s * 0.8, c.x + s * 0.5, c.y + s * 0.5]
    }
}

/// The page's text with spaces and line breaks between words and lines,
/// and for each character of it the char index it comes from (none for
/// the ones put in).
pub fn page_string(p: &Page) -> (Vec<char>, Vec<Option<usize>>) {
    let mut s = Vec::with_capacity(p.chars.len());
    let mut map = Vec::with_capacity(p.chars.len());
    let mut last: Option<&Char> = None;
    for (i, c) in p.chars.iter().enumerate() {
        if let Some(l) = last {
            let size = l.size.max(1.0);
            if (c.y - l.y).abs() > size * 0.5 {
                s.push('\n');
                map.push(None);
            } else if c.x - (l.x + l.w) > size * 0.2 && !l.ch.is_whitespace() && !c.ch.is_whitespace() {
                s.push(' ');
                map.push(None);
            }
        }
        s.push(c.ch);
        map.push(Some(i));
        last = Some(c);
    }
    (s, map)
}

/// Text of chars `a..=b` of a page.
pub fn text_of(p: &Page, a: usize, b: usize) -> String {
    let (s, map) = page_string(p);
    let start = map.iter().position(|m| m.is_some_and(|i| i >= a)).unwrap_or(s.len());
    let end = map.iter().rposition(|m| m.is_some_and(|i| i <= b)).map_or(0, |e| e + 1);
    if start >= end {
        return String::new();
    }
    s[start..end].iter().collect()
}

/// The char under (or nearest to, on the same line) a point.
pub fn char_at(p: &Page, x: f32, y: f32) -> Option<usize> {
    let mut best: Option<(f32, usize)> = None;
    for (i, c) in p.chars.iter().enumerate() {
        let b = char_box(c);
        let dx = if x < b[0] { b[0] - x } else if x > b[2] { x - b[2] } else { 0.0 };
        let dy = if y < b[1] { b[1] - y } else if y > b[3] { y - b[3] } else { 0.0 };
        if dy > c.size.max(4.0) * 1.5 {
            continue;
        }
        let d = dx + dy * 4.0;
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, i));
        }
    }
    best.map(|b| b.1)
}

/// The word around char `i`.
pub fn word_at(p: &Page, i: usize) -> (usize, usize) {
    let (s, map) = page_string(p);
    let Some(k) = map.iter().position(|m| *m == Some(i)) else { return (i, i) };
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '-' || c == '\'';
    let mut a = k;
    while a > 0 && is_word(s[a - 1]) && map[a - 1].is_some() {
        a -= 1;
    }
    let mut b = k;
    while b + 1 < s.len() && is_word(s[b + 1]) && map[b + 1].is_some() {
        b += 1;
    }
    (map[a].unwrap_or(i), map[b].unwrap_or(i))
}

#[derive(Default)]
pub struct Find {
    pub open: bool,
    pub focus: bool,
    pub query: String,
    pub case: bool,
    /// Matches: page and chars a..=b.
    pub hits: Vec<Selection>,
    pub cur: usize,
    /// What the matches are for.
    done: (String, bool, u64),
}

/// Every match of `q` in the pages.
pub fn search(pages: &[Page], q: &str, case: bool) -> Vec<Selection> {
    let fold = |c: char| if case { c } else { c.to_lowercase().next().unwrap_or(c) };
    let q: Vec<char> = q.chars().map(fold).collect();
    let mut out = Vec::new();
    if q.is_empty() {
        return out;
    }
    for (pi, p) in pages.iter().enumerate() {
        let (s, map) = page_string(p);
        // Line breaks match spaces.
        let s: Vec<char> = s.into_iter().map(|c| if c == '\n' { ' ' } else { fold(c) }).collect();
        let mut i = 0;
        while i + q.len() <= s.len() {
            if s[i..i + q.len()] == q[..] {
                let a = map[i..i + q.len()].iter().flatten().next();
                let b = map[i..i + q.len()].iter().flatten().next_back();
                if let (Some(&a), Some(&b)) = (a, b) {
                    out.push(Selection { page: pi, a, b });
                }
                i += q.len();
            } else {
                i += 1;
            }
            if out.len() > 50_000 {
                return out;
            }
        }
    }
    out
}

impl PdfView {
    pub fn selected_text(&self) -> Option<String> {
        let s = self.sel?;
        let p = self.text.page(s.page)?;
        let t = text_of(p, s.a.min(s.b), s.a.max(s.b));
        (!t.is_empty()).then_some(t)
    }

    pub fn select_page(&mut self, i: usize) {
        if let Some(p) = self.text.page(i)
            && !p.chars.is_empty()
        {
            self.sel = Some(Selection { page: i, a: 0, b: p.chars.len() - 1 });
        }
    }

    /// The search bar above the pages.
    pub(super) fn find_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(tr("pdf.find"));
            let r = ui.add(egui::TextEdit::singleline(&mut self.find.query).desired_width(220.0).hint_text(tr("pdf.find_hint")));
            if std::mem::take(&mut self.find.focus) {
                r.request_focus();
            }
            ui.checkbox(&mut self.find.case, tr("pdf.match_case"));
            self.update_hits();
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            let back = ui.input(|i| i.modifiers.shift);
            let n = self.find.hits.len();
            // Nothing to go to without matches.
            let why = if self.find.query.is_empty() { tr("pdf.find_hint") } else { tr("pdf.no_matches") };
            if ui.add_enabled(n > 0, egui::Button::new("⏶")).on_hover_text(tr("pdf.find_prev")).on_disabled_hover_text(why).clicked() || enter && back {
                self.next_hit(false);
            }
            if ui.add_enabled(n > 0, egui::Button::new("⏷")).on_hover_text(tr("pdf.find_next")).on_disabled_hover_text(why).clicked() || enter && !back {
                self.next_hit(true);
            }
            if enter {
                r.request_focus();
            }
            if !self.find.query.is_empty() {
                ui.label(if n == 0 { tr("pdf.no_matches").to_string() } else { trf("pdf.match_of", &[("n", &(self.find.cur + 1)), ("total", &n)]) });
            }
            if self.text.pages.is_none() {
                ui.spinner();
            }
            let (cr, close) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
            let color = if close.hovered() { ui.visuals().strong_text_color() } else { ui.visuals().weak_text_color() };
            crate::icons::paint(ui.painter(), cr.shrink(3.0), crate::icons::Icon::Close, color);
            if close.on_hover_text(tr("dialog.close")).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                self.find.open = false;
                self.find.hits.clear();
            }
        });
    }

    fn update_hits(&mut self) {
        let key = (self.find.query.clone(), self.find.case, self.text.rev);
        if self.find.done == key {
            return;
        }
        let Some(pages) = self.text.pages.clone() else { return };
        self.find.hits = search(&pages, &self.find.query, self.find.case);
        // The first match from the page shown on.
        self.find.cur = self.find.hits.iter().position(|h| h.page >= self.cur).unwrap_or(0);
        self.find.done = key;
        if let Some(h) = self.find.hits.get(self.find.cur).copied() {
            self.show_hit(h);
        }
    }

    fn next_hit(&mut self, forward: bool) {
        let n = self.find.hits.len();
        if n == 0 {
            return;
        }
        self.find.cur = if forward { (self.find.cur + 1) % n } else { (self.find.cur + n - 1) % n };
        let h = self.find.hits[self.find.cur];
        self.show_hit(h);
    }

    fn show_hit(&mut self, h: Selection) {
        self.sel = Some(h);
        let top = self.text.page(h.page).and_then(|p| p.chars.get(h.a)).map_or(0.0, |c| (c.y - c.size * 3.0).max(0.0));
        self.go_to(h.page, top);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(ch: char, x: f32, y: f32) -> Char {
        Char { ch, x, y, w: 6.0, size: 10.0, font: 0, color: 0, upright: true, hidden: false }
    }

    fn page() -> Page {
        let mut chars = Vec::new();
        for (i, c) in "Hello".chars().enumerate() {
            chars.push(ch(c, 10.0 + i as f32 * 6.0, 20.0));
        }
        for (i, c) in "world".chars().enumerate() {
            chars.push(ch(c, 50.0 + i as f32 * 6.0, 20.0));
        }
        for (i, c) in "Next".chars().enumerate() {
            chars.push(ch(c, 10.0 + i as f32 * 6.0, 40.0));
        }
        Page { width: 200.0, height: 200.0, chars, ..Default::default() }
    }

    #[test]
    fn text_words_and_search() {
        let p = page();
        let (s, _) = page_string(&p);
        assert_eq!(s.iter().collect::<String>(), "Hello world\nNext");
        assert_eq!(text_of(&p, 3, 11), "lo world\nNe");
        assert_eq!(word_at(&p, 6), (5, 9));
        assert_eq!(char_at(&p, 52.0, 18.0), Some(5));
        let hits = search(&[p.clone(), p], "WORLD next", false);
        assert_eq!(hits.len(), 2);
        assert_eq!((hits[1].page, hits[1].a, hits[1].b), (1, 5, 13));
        assert!(search(&[page()], "WORLD", true).is_empty());
    }
}
