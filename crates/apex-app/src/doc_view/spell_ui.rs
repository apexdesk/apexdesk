//! Spell and grammar checking in documents: red wavy underlines under
//! misspelled words, blue double ones under grammar mistakes, suggestions in
//! the context menu and the Spelling and Grammar dialog (F7).

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use apex_doc::{ParaKind, Pos, Selection};
use apex_spell::Issue;
use eframe::egui::{self, Color32, Pos2, Stroke, pos2};

use super::{DocView, ParaLayout, Segment, char_count};
use crate::i18n::tr;
use crate::{grammar, spell};

/// A misspelled word under the pointer when the context menu opened.
pub(super) struct MenuWord {
    para: usize,
    range: Range<usize>,
    word: String,
    suggestions: Vec<String>,
}

/// A grammar mistake under the pointer when the context menu opened.
pub(super) struct MenuIssue {
    para: usize,
    issue: Issue,
    text: String,
}

/// The Spelling dialog's progress through the document.
struct Run {
    dlg: spell::Dialog,
    /// The word shown (byte range in its paragraph).
    at: (usize, Range<usize>),
    /// Where the check started: it stops when it comes back there.
    origin: Pos,
    wrapped: bool,
}

#[derive(Default)]
pub(super) struct SpellState {
    /// Misspelled char ranges by paragraph text, for one spell generation.
    cache: RefCell<(u64, HashMap<u64, Arc<[Range<usize>]>>)>,
    /// Grammar mistakes (with their char ranges) by paragraph text.
    gcache: RefCell<(u64, HashMap<u64, Arc<[(Range<usize>, Issue)]>>)>,
    pub(super) menu: Option<MenuWord>,
    gmenu: Option<MenuIssue>,
    run: Option<Run>,
}

fn text_key(text: &str) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// Deleted text (tracked changes) isn't checked.
fn in_deleted(p: &apex_doc::Paragraph, off: usize) -> bool {
    p.has_deleted() && p.spans().any(|(s, e, a)| s <= off && off < e && a.is_deleted())
}

/// Byte ranges of `text` (sorted) as char ranges, walking forward once.
fn char_ranges<'a>(text: &'a str, ranges: impl Iterator<Item = Range<usize>> + 'a) -> impl Iterator<Item = Range<usize>> + 'a {
    let mut ci = 0;
    let mut last = 0;
    ranges.map(move |r| {
        ci += text[last..r.start].chars().count();
        let a = ci;
        ci += text[r.clone()].chars().count();
        last = r.end;
        a..ci
    })
}

/// What a paragraph's checks depend on: its text, deleted text, the
/// placeholders of its content controls and its languages.
type Proofing = (u64, Vec<Range<usize>>, Vec<(Range<usize>, Option<Arc<str>>, bool)>);

fn proofing(p: &apex_doc::Paragraph) -> Proofing {
    // Placeholder text of content controls isn't checked.
    let ph: Vec<Range<usize>> = apex_doc::para_controls(p, 0).into_iter().filter(|c| c.control.placeholder).map(|c| c.range).collect();
    let langs = spell::para_spans(p);
    let key = text_key(&p.text) ^ u64::from(p.has_deleted()) ^ ph.iter().chain(langs.iter().map(|x| &x.0)).fold(0u64, |h, r| h.wrapping_mul(31).wrapping_add((r.start as u64) << 32 | r.end as u64)) ^ langs.iter().fold(0u64, |h, x| h.wrapping_mul(17).wrapping_add(x.1.as_deref().map_or(1, text_key)).wrapping_add(u64::from(x.2)));
    (key, ph, langs)
}

impl DocView {
    /// Grammar mistakes of a paragraph, with their char ranges (cached).
    fn grammar_issues(&self, para: usize) -> Arc<[(Range<usize>, Issue)]> {
        let p = &self.doc.paragraphs[para];
        if p.style.kind == ParaKind::Code || p.text.is_empty() || !grammar::auto() {
            return Arc::from([]);
        }
        let generation = grammar::generation();
        let (key, ph, langs) = proofing(p);
        let mut cache = self.spell.gcache.borrow_mut();
        if cache.0 != generation || cache.1.len() > 20_000 {
            *cache = (generation, HashMap::new());
        }
        cache
            .1
            .entry(key)
            .or_insert_with(|| {
                let issues: Vec<Issue> = grammar::issues(&p.text, &langs).into_iter().filter(|i| !in_deleted(p, i.range.start) && !ph.iter().any(|x| x.start < i.range.end && i.range.start < x.end)).collect();
                char_ranges(&p.text, issues.iter().map(|i| i.range.clone())).zip(issues.iter().cloned()).collect()
            })
            .clone()
    }

    /// Misspelled words of a paragraph as char ranges (cached).
    fn misspelled_chars(&self, para: usize) -> Arc<[Range<usize>]> {
        let p = &self.doc.paragraphs[para];
        if p.style.kind == ParaKind::Code || p.text.is_empty() {
            return Arc::from([]);
        }
        let generation = spell::generation();
        let (key, ph, langs) = proofing(p);
        let mut cache = self.spell.cache.borrow_mut();
        if cache.0 != generation || cache.1.len() > 20_000 {
            *cache = (generation, HashMap::new());
        }
        cache
            .1
            .entry(key)
            .or_insert_with(|| {
                let bad = spell::misspelled_in(&p.text, &langs).into_iter().filter(|r| !in_deleted(p, r.start) && !ph.iter().any(|x| x.start < r.end && r.start < x.end));
                char_ranges(&p.text, bad).collect()
            })
            .clone()
    }

    /// Red wavy lines under the misspelled words of a segment, blue double
    /// ones under its grammar mistakes.
    pub(super) fn paint_spelling(&self, p: &egui::Painter, seg: &Segment, pl: &ParaLayout, galley_pos: Pos2) {
        // (Not under turned cell text.)
        if pl.rotate != 0 {
            return;
        }
        // The word being typed isn't marked until the caret leaves it.
        let typing = (self.sel.is_empty() && self.sel.cursor.para == seg.para).then(|| char_count(&self.doc.paragraphs[seg.para].text, self.sel.cursor.offset));
        let z = self.zoom;
        if spell::auto() {
            let bad = self.misspelled_chars(seg.para);
            let stroke = Stroke::new(1.0_f32.max(0.8 * z), Color32::from_rgb(0xE0, 0x20, 0x20));
            underline(seg, pl, galley_pos, &bad, |r| typing == Some(r.end), z, |x0, x1, y| wavy(p, x0, x1, y, z, stroke));
        }
        let issues = self.grammar_issues(seg.para);
        if !issues.is_empty() {
            let ranges: Vec<Range<usize>> = issues.iter().map(|x| x.0.clone()).collect();
            let stroke = Stroke::new(1.0_f32.max(0.7 * z), spell::GRAMMAR_BLUE);
            let gap = 1.5 * z.max(0.75);
            // Not while the caret is in it or at its end: "el el" while
            // typing "el elefante".
            underline(seg, pl, galley_pos, &ranges, |r| typing.is_some_and(|t| r.start <= t && t <= r.end), z, |x0, x1, y| {
                p.line_segment([pos2(x0, y - gap / 2.0), pos2(x1, y - gap / 2.0)], stroke);
                p.line_segment([pos2(x0, y + gap / 2.0), pos2(x1, y + gap / 2.0)], stroke);
            });
        }
    }

    /// Remembers the misspelled word or grammar mistake at `pos` for the
    /// context menu.
    pub(super) fn spell_menu_at(&mut self, pos: Pos) {
        self.spell.menu = None;
        self.spell.gmenu = self.grammar_issues(pos.para).iter().find(|(_, i)| i.range.start <= pos.offset && pos.offset <= i.range.end).map(|(_, i)| MenuIssue {
            para: pos.para,
            text: self.doc.paragraphs[pos.para].text[i.range.clone()].to_string(),
            issue: i.clone(),
        });
        if !spell::ready() {
            return;
        }
        let text = &self.doc.paragraphs[pos.para].text;
        if let Some(r) = spell::misspelled_at(text, pos.offset) {
            let word = text[r.clone()].to_string();
            let suggestions = spell::suggest(&word, 6);
            self.spell.menu = Some(MenuWord { para: pos.para, range: r, word, suggestions });
        }
    }

    /// Suggestions at the top of the context menu.
    pub(super) fn spell_context_items(&mut self, ui: &mut egui::Ui) {
        self.spelling_items(ui);
        self.grammar_items(ui);
    }

    fn grammar_items(&mut self, ui: &mut egui::Ui) {
        let Some(m) = &self.spell.gmenu else { return };
        if m.para >= self.doc.paragraphs.len() || self.doc.paragraphs[m.para].text.get(m.issue.range.clone()) != Some(m.text.as_str()) {
            return;
        }
        ui.set_max_width(320.0);
        ui.label(egui::RichText::new(grammar::message(m.issue.rule)).color(spell::GRAMMAR_BLUE));
        let mut chosen = None;
        for s in &m.issue.suggestions {
            if ui.button(egui::RichText::new(s).strong()).clicked() {
                chosen = Some(s.clone());
            }
        }
        if ui.button(tr("grammar.ignore")).clicked() {
            grammar::ignore(m.issue.rule, &m.text);
            ui.close();
        }
        if ui.button(tr("grammar.ignore_rule")).clicked() {
            grammar::turn_off(m.issue.rule);
            ui.close();
        }
        ui.separator();
        if let Some(s) = chosen {
            let (para, r) = (m.para, m.issue.range.clone());
            let sel = Selection { anchor: Pos::new(para, r.start), cursor: Pos::new(para, r.end) };
            let p = self.doc.insert_text(sel, &s, None);
            self.set_sel(Selection::caret(p));
            self.spell.gmenu = None;
            ui.close();
        }
    }

    fn spelling_items(&mut self, ui: &mut egui::Ui) {
        let Some(m) = &self.spell.menu else { return };
        if m.para >= self.doc.paragraphs.len() || self.doc.paragraphs[m.para].text.get(m.range.clone()) != Some(m.word.as_str()) {
            return;
        }
        let mut chosen = None;
        for s in &m.suggestions {
            if ui.button(egui::RichText::new(s).strong()).clicked() {
                chosen = Some(s.clone());
            }
        }
        if m.suggestions.is_empty() {
            ui.add_enabled(false, egui::Button::new(tr("spell.no_suggestions")));
        }
        if ui.button(tr("spell.ignore_all")).clicked() {
            spell::ignore_all(&m.word);
            ui.close();
        }
        if ui.button(tr("spell.add")).clicked() {
            spell::add_word(&m.word);
            ui.close();
        }
        ui.separator();
        if let Some(s) = chosen {
            let (para, r) = (m.para, m.range.clone());
            let sel = Selection { anchor: Pos::new(para, r.start), cursor: Pos::new(para, r.end) };
            let p = self.doc.insert_text(sel, &s, None);
            self.set_sel(Selection::caret(p));
            self.spell.menu = None;
            ui.close();
        }
    }

    // --- Spelling dialog ------------------------------------------------------------------

    /// The first misspelled word or grammar mistake at or after `from` and
    /// before `limit`.
    fn next_misspelling(&self, from: Pos, limit: Pos) -> Option<(usize, Range<usize>, Option<Issue>)> {
        for i in from.para..=limit.para.min(self.doc.paragraphs.len().saturating_sub(1)) {
            let p = &self.doc.paragraphs[i];
            if p.style.kind == ParaKind::Code {
                continue;
            }
            let lo = if i == from.para { from.offset } else { 0 };
            let hi = if i == limit.para { limit.offset } else { usize::MAX };
            let word = spell::misspelled_in(&p.text, &spell::para_spans(p)).into_iter().find(|r| r.start >= lo && r.start < hi && !in_deleted(p, r.start));
            let issue = self.grammar_issues(i).iter().map(|x| x.1.clone()).find(|x| x.range.start >= lo && x.range.start < hi);
            match (word, issue) {
                (Some(w), Some(g)) if g.range.start < w.start => return Some((i, g.range.clone(), Some(g))),
                (Some(w), _) => return Some((i, w, None)),
                (None, Some(g)) => return Some((i, g.range.clone(), Some(g))),
                (None, None) => {}
            }
        }
        None
    }

    /// F7: checks the document from the caret.
    pub(crate) fn start_spelling(&mut self) {
        let origin = self.sel.start();
        let mut run = Run { dlg: spell::Dialog { grammar: grammar::auto(), ..Default::default() }, at: (origin.para, origin.offset..origin.offset), origin, wrapped: false };
        if !spell::ready() {
            run.dlg.finish();
        }
        self.spell.run = Some(run);
        if spell::ready() {
            self.spell_advance(origin);
        }
    }

    /// Shows the next misspelled word from `from` (wrapping once), or the end.
    fn spell_advance(&mut self, from: Pos) {
        let Some(run) = &self.spell.run else { return };
        let (origin, wrapped) = (run.origin, run.wrapped);
        let mut hit = None;
        let mut wrap = wrapped;
        if !wrapped {
            hit = self.next_misspelling(from, self.doc.end());
            if hit.is_none() {
                wrap = true;
            }
        }
        if hit.is_none() {
            let from = if wrapped { from } else { Pos::new(0, 0) };
            hit = self.next_misspelling(from, origin);
        }
        let Some(run) = self.spell.run.as_mut() else { return };
        run.wrapped = wrap;
        match hit {
            Some((para, r, issue)) => {
                match issue {
                    Some(i) => run.dlg.found_issue(&self.doc.paragraphs[para].text, r.clone(), i.rule, i.suggestions),
                    None => run.dlg.found(&self.doc.paragraphs[para].text, r.clone()),
                }
                run.at = (para, r.clone());
                self.set_sel(Selection { anchor: Pos::new(para, r.start), cursor: Pos::new(para, r.end) });
            }
            None => run.dlg.finish(),
        }
    }

    pub(super) fn spell_dialog_open(&self) -> bool {
        self.spell.run.is_some()
    }

    pub(super) fn spelling_dialog(&mut self, ctx: &egui::Context) {
        let Some(run) = self.spell.run.as_mut() else { return };
        let Some(action) = run.dlg.ui(ctx) else { return };
        let (para, r) = run.at.clone();
        let word = run.dlg.word.clone();
        // The document may have changed under the dialog.
        let still = self.doc.paragraphs.get(para).and_then(|p| p.text.get(r.clone())) == Some(word.as_str());
        let after = Pos::new(para, r.end);
        match action {
            spell::Action::Close => {
                self.spell.run = None;
                self.refocus = true;
            }
            spell::Action::Recheck => self.spell_advance(Pos::new(para, r.start)),
            spell::Action::IgnoreOnce => self.spell_advance(if still { after } else { Pos::new(para, r.start) }),
            spell::Action::IgnoreAll => {
                spell::ignore_all(&word);
                self.spell_advance(Pos::new(para, r.start));
            }
            spell::Action::IgnoreRule => {
                if let Some(rule) = self.spell.run.as_ref().and_then(|x| x.dlg.rule) {
                    grammar::turn_off(rule);
                }
                self.spell_advance(Pos::new(para, r.start));
            }
            spell::Action::Add => {
                spell::add_word(&word);
                self.spell_advance(Pos::new(para, r.start));
            }
            spell::Action::Change(to) => {
                if still {
                    let sel = Selection { anchor: Pos::new(para, r.start), cursor: after };
                    let p = self.doc.insert_text(sel, &to, None);
                    self.shift_origin(para, r.end, to.len() as isize - word.len() as isize);
                    self.spell_advance(p);
                } else {
                    self.spell_advance(Pos::new(para, r.start));
                }
            }
            spell::Action::ChangeAll(to) => {
                self.doc.replace_all_opts(&word, &to, true, true);
                // Offsets past the change moved; start over from this paragraph.
                if let Some(run) = self.spell.run.as_mut()
                    && run.origin.para == para
                {
                    run.origin.offset = run.origin.offset.min(r.start);
                }
                let p = self.doc.clamp(Pos::new(para, r.start));
                self.spell_advance(p);
            }
        }
    }

    /// Keeps the check's starting point in place when text before it in
    /// the same paragraph changes length.
    fn shift_origin(&mut self, para: usize, at: usize, delta: isize) {
        if let Some(run) = self.spell.run.as_mut()
            && run.origin.para == para
            && run.origin.offset >= at
        {
            run.origin.offset = (run.origin.offset as isize + delta).max(0) as usize;
        }
    }
}

/// Draws `line(x0, x1, y)` under the char `ranges` (sorted) of a segment,
/// row by row, except those `skip` says.
fn underline(seg: &Segment, pl: &ParaLayout, galley_pos: Pos2, ranges: &[Range<usize>], skip: impl Fn(&Range<usize>) -> bool, z: f32, mut line: impl FnMut(f32, f32, f32)) {
    if ranges.is_empty() {
        return;
    }
    let mut start = 0usize;
    let mut k = 0;
    for row in &pl.galley.rows {
        let n = row.row.glyphs.len() + usize::from(row.ends_with_newline);
        let end = start + n;
        let visible = row.pos.y + row.row.size.y > seg.gy0 && row.pos.y < seg.gy1;
        while k < ranges.len() && ranges[k].end <= start {
            k += 1;
        }
        for r in ranges[k..].iter().take_while(|r| r.start < end) {
            if !visible || skip(r) {
                continue;
            }
            let x_at = |i: usize| row.pos.x + row.row.glyphs.get(i - start).map_or(row.row.size.x, |g| g.pos.x);
            let (a, b) = (r.start.max(start), r.end.min(end));
            if a >= b {
                continue;
            }
            let (x0, x1) = (x_at(a), x_at(b));
            let y = galley_pos.y + row.pos.y + row.row.size.y - 1.5 * z.max(0.75);
            line(galley_pos.x + x0, galley_pos.x + x1, y);
        }
        start = end;
    }
}

/// A zigzag underline from `x0` to `x1`.
fn wavy(p: &egui::Painter, x0: f32, x1: f32, y: f32, z: f32, stroke: Stroke) {
    let step = 2.0 * z.max(0.75);
    let mut pts = Vec::with_capacity(((x1 - x0) / step) as usize + 2);
    let mut x = x0;
    let mut up = false;
    while x < x1 {
        pts.push(pos2(x, if up { y - step / 2.0 } else { y + step / 2.0 }));
        x += step;
        up = !up;
    }
    pts.push(pos2(x1, if up { y - step / 2.0 } else { y + step / 2.0 }));
    p.add(egui::Shape::line(pts, stroke));
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_doc::Document;

    #[test]
    fn grammar_mistakes_are_underlined_offered_and_found_with_f7() {
        if spell::language().is_empty() {
            spell::init(&egui::Context::default(), "en_US", true);
        }
        // No dictionary installed: nothing to check against.
        if spell::language().is_empty() {
            return;
        }
        let start = web_time::Instant::now();
        while spell::loading() && start.elapsed() < web_time::Duration::from_secs(30) {
            apex_fs::thread::sleep(web_time::Duration::from_millis(10));
        }
        let mut v = DocView::new(Document::from_plain_text("One zyxxq zyxxq here."));
        let issues = v.grammar_issues(0);
        assert_eq!(issues.len(), 1);
        assert_eq!((issues[0].0.clone(), issues[0].1.rule), (4..15, "repeated"));
        // The context menu has it under the pointer.
        v.spell_menu_at(Pos::new(0, 6));
        assert_eq!(v.spell.gmenu.as_ref().map(|m| (m.text.as_str(), m.issue.suggestions.clone())), Some(("zyxxq zyxxq", vec!["zyxxq".to_string()])));
        // F7 stops at the misspelled word first, then at the mistake.
        if spell::ready() {
            v.start_spelling();
            let run = v.spell.run.as_ref().unwrap();
            assert_eq!((run.dlg.rule, run.at.1.clone()), (None, 4..9));
            spell::ignore_all("zyxxq");
            v.spell_advance(Pos::new(0, 4));
            let run = v.spell.run.as_ref().unwrap();
            assert_eq!((run.dlg.rule, run.at.1.clone(), run.dlg.replacement.as_str()), (Some("repeated"), 4..15, "zyxxq"));
        }
        // Ignored, it goes.
        grammar::ignore("repeated", "zyxxq zyxxq");
        assert!(v.grammar_issues(0).is_empty());
    }
}
