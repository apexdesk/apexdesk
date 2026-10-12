//! The document outline: headings, tables of contents built from them, and
//! moving a heading's whole section.

use std::collections::HashMap;
use std::sync::Arc;

use crate::{Attrs, Document, EditKind, Field, FieldKind, LINE_BREAK, Mark, OBJECT, ParaKind, ParaStyle, Paragraph, Pos, Selection};

/// Settings of a table of contents (Word's `TOC \o "1-3" \h`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TocOpts {
    /// Heading levels listed, 1..=9.
    pub levels: u8,
    pub page_numbers: bool,
    /// Page numbers at the right margin (else right after the text).
    pub right_align: bool,
    /// Dots between the text and a right aligned page number.
    pub leader: bool,
    /// Entries link to their headings.
    pub links: bool,
    /// A table of figures: the captions of this kind (see
    /// `caption_kind`) instead of the headings.
    pub caption: u8,
    /// An alphabetical index: the index entries (XE fields) instead.
    pub index: bool,
    /// In an index, a heading line before each initial letter.
    pub letters: bool,
    /// A bibliography: the document's sources.
    pub bib: bool,
    /// A table of authorities: the entries (TA fields) of this category
    /// (1..=16).
    pub toa: u8,
}

impl Default for TocOpts {
    fn default() -> Self {
        TocOpts { levels: 3, page_numbers: true, right_align: true, leader: true, links: true, caption: 0, index: false, letters: false, bib: false, toa: 0 }
    }
}

/// Folds text for sorting and matching index entries: lower case, without
/// accents.
pub fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            c => c,
        })
        .collect()
}

/// The parts of an index entry ("Main:Sub"), trimmed.
pub fn index_parts(t: &str) -> (String, Option<String>) {
    match t.split_once(':') {
        Some((m, s)) if !s.trim().is_empty() => (m.trim().to_string(), Some(s.trim().to_string())),
        Some((m, _)) => (m.trim().to_string(), None),
        None => (t.trim().to_string(), None),
    }
}

/// What index entries match by.
pub fn index_key(t: &str) -> String {
    match index_parts(t) {
        (m, Some(s)) => format!("{}:{}", fold(&m), fold(&s)),
        (m, None) => fold(&m),
    }
}

/// Marks a paragraph generated for a table of contents; the consecutive
/// marked paragraphs are one table, which updating rebuilds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TocMark {
    /// Level of the entry (1..=9); 0 for the "no entries" line.
    pub level: u8,
    pub opts: TocOpts,
}

/// Indent of a table of contents entry per level, in points (Word's TOC
/// styles step 11 pt).
pub const TOC_INDENT_PT: f32 = 11.0;

/// Plain text of a heading for its entry.
pub(crate) fn entry_text(p: &Paragraph) -> String {
    p.text.chars().filter(|&c| c != OBJECT).map(|c| if c == LINE_BREAK || c == '\t' { ' ' } else { c }).collect::<String>().trim().to_string()
}

impl Document {
    /// Headings of the outline: paragraph and level (1..=9), outside tables
    /// and tables of contents, with text.
    pub fn headings(&self) -> Vec<(usize, u8)> {
        self.paragraphs
            .iter()
            .enumerate()
            .filter_map(|(i, p)| match p.style.kind {
                ParaKind::Heading(l) if p.cell.is_none() && p.style.toc.is_none() && !entry_text(p).is_empty() => Some((i, l.max(1))),
                _ => None,
            })
            .collect()
    }

    /// End (exclusive) of the section of heading `para`: the next heading of
    /// the same or a higher level, or the end of the document.
    pub fn section_end(&self, para: usize) -> usize {
        let ParaKind::Heading(level) = self.paragraphs[para].style.kind else { return para + 1 };
        (para + 1..self.paragraphs.len())
            .find(|&i| {
                let p = &self.paragraphs[i];
                matches!(p.style.kind, ParaKind::Heading(l) if l <= level && p.cell.is_none() && p.style.toc.is_none())
            })
            .unwrap_or(self.paragraphs.len())
    }

    /// Moves the section of heading `from` (the heading and everything up to
    /// the next heading of its level) before paragraph `to`, as one undo
    /// step. Returns the caret at the moved heading.
    pub fn move_section(&mut self, from: usize, to: usize, sel: Selection) -> Option<Selection> {
        if from >= self.paragraphs.len() || to > self.paragraphs.len() || !matches!(self.paragraphs[from].style.kind, ParaKind::Heading(_)) || self.in_table(from) {
            return None;
        }
        if to < self.paragraphs.len() && self.in_table(to) && self.table_bounds(to).is_some_and(|(a, _)| a != to) {
            return None;
        }
        let (s, e) = (from, self.section_end(from));
        if (s..=e).contains(&to) {
            return None;
        }
        let (lo, hi) = (s.min(to), e.max(to));
        let new: Vec<Paragraph> = if to < s {
            self.paragraphs[s..e].iter().chain(&self.paragraphs[to..s]).cloned().collect()
        } else {
            self.paragraphs[e..to].iter().chain(&self.paragraphs[s..e]).cloned().collect()
        };
        let at = if to < s { to } else { to - (e - s) };
        let caret = Selection::caret(Pos::new(at, 0));
        self.replace_paras(lo, hi, new, sel, caret, EditKind::Outline);
        Some(caret)
    }

    /// Moves paragraphs `from..end` before paragraph `to` (outside them),
    /// as one undo step. Tables move whole: none may be cut. Returns the
    /// moved paragraphs selected.
    pub fn move_paras(&mut self, from: usize, end: usize, to: usize, sel: Selection) -> Option<Selection> {
        let n = self.paragraphs.len();
        if from >= end || end > n || to > n || (from..=end).contains(&to) {
            return None;
        }
        let inside = |i: usize| i > 0 && i < n && self.in_table(i) && self.in_table(i - 1);
        if inside(from) || inside(end) || inside(to) {
            return None;
        }
        let (lo, hi) = (from.min(to), end.max(to));
        let new: Vec<Paragraph> = if to < from {
            self.paragraphs[from..end].iter().chain(&self.paragraphs[to..from]).cloned().collect()
        } else {
            self.paragraphs[end..to].iter().chain(&self.paragraphs[from..end]).cloned().collect()
        };
        let at = if to < from { to } else { to - (end - from) };
        let last = at + (end - from) - 1;
        let after = Selection { anchor: Pos::new(at, 0), cursor: Pos::new(last, self.paragraphs[end - 1].text.len()) };
        self.replace_paras(lo, hi, new, sel, after, EditKind::Outline);
        Some(after)
    }

    /// Changes the outline level of paragraphs `from..=to`: `f` maps a
    /// heading's level (None: body text) to the new one, as one undo step.
    pub fn set_outline_levels(&mut self, from: usize, to: usize, sel: Selection, f: &dyn Fn(Option<u8>) -> Option<u8>) {
        let styles = self.styles.clone();
        self.set_para_style(from, to, sel, &|st| {
            let cur = match st.kind {
                ParaKind::Heading(l) => Some(l),
                _ => None,
            };
            let new = f(cur).map(|l| l.clamp(1, 9));
            if new != cur {
                styles.restyle(st, new.map_or(ParaKind::Normal, ParaKind::Heading), 0);
            }
        });
    }

    /// Tables of contents: paragraph ranges `start..end`.
    pub fn toc_blocks(&self) -> Vec<(usize, usize)> {
        let mut out: Vec<(usize, usize)> = Vec::new();
        for (i, p) in self.paragraphs.iter().enumerate() {
            if p.style.toc.is_none() {
                continue;
            }
            match out.last_mut() {
                Some(b) if b.1 == i => b.1 = i + 1,
                _ => out.push((i, i + 1)),
            }
        }
        out
    }

    /// The table of contents holding paragraph `para`.
    pub fn toc_at(&self, para: usize) -> Option<(usize, usize)> {
        self.paragraphs.get(para)?.style.toc?;
        let mut a = para;
        while a > 0 && self.paragraphs[a - 1].style.toc.is_some() {
            a -= 1;
        }
        let mut b = para + 1;
        while b < self.paragraphs.len() && self.paragraphs[b].style.toc.is_some() {
            b += 1;
        }
        Some((a, b))
    }

    /// The paragraphs `lo..hi` rebuilt with the table of contents `a..b`
    /// (empty to insert one before `a`) generated anew, and bookmarks added
    /// to the headings that lack one. `split` cuts paragraph `split.para`
    /// and puts the table after its first part.
    fn toc_plan(&self, a: usize, b: usize, split: Option<Pos>, opts: TocOpts, none_text: &str) -> (usize, usize, Vec<Paragraph>, usize) {
        let heads: Vec<(usize, u8)> = if opts.index || opts.bib || opts.toa > 0 {
            Vec::new()
        } else if opts.caption > 0 {
            self.captions(opts.caption).into_iter().map(|i| (i, 1)).filter(|&(i, _)| !(a..b).contains(&i)).collect()
        } else {
            self.headings().into_iter().filter(|&(i, l)| l <= opts.levels && !(a..b).contains(&i)).collect()
        };
        let refs = (opts.caption > 0).then(|| self.refs());
        let (mut lo, mut hi) = (a, b.max(a));
        if let Some(s) = split {
            (lo, hi) = (lo.min(s.para), hi.max(s.para + 1));
        }
        let mut names: HashMap<usize, Arc<str>> = HashMap::new();
        let mut adds: Vec<usize> = Vec::new();
        let first = self.new_hidden_name("_Toc");
        let mut next: u64 = first[4..].parse().unwrap_or(100_000_001);
        for &(h, _) in &heads {
            let p = &self.paragraphs[h];
            let existing = p.marks.iter().find(|m| !m.end && m.offset == 0 && m.name.starts_with("_Toc")).map(|m| m.name.clone());
            let name = existing.unwrap_or_else(|| {
                let n: Arc<str> = Arc::from(format!("_Toc{next:09}").as_str());
                next += 1;
                adds.push(h);
                (lo, hi) = (lo.min(h), hi.max(h + 1));
                n
            });
            names.insert(h, name);
        }
        let mut entries = if opts.index {
            self.index_entries(opts)
        } else if opts.bib {
            self.bib_entries(opts)
        } else if opts.toa > 0 {
            self.authority_entries(opts)
        } else {
            Vec::new()
        };
        for &(h, level) in &heads {
            let name = &names[&h];
            let link: Option<Arc<str>> = opts.links.then(|| Arc::from(format!("#{name}").as_str()));
            let attrs = Attrs { link: link.clone(), ..Default::default() };
            let mut p = Paragraph::new("");
            p.runs[0].attrs = attrs.clone();
            let text = match &refs {
                Some(r) => self.caption_text(h, r),
                None => entry_text(&self.paragraphs[h]),
            };
            p.push(&text, attrs.clone());
            if opts.page_numbers {
                p.push(if opts.right_align { "\t" } else { " " }, attrs.clone());
                let f = Field { hyperlink: opts.links, ..Field::new(FieldKind::PageRef(name.to_string())) };
                p.push(&OBJECT.to_string(), Attrs { field: Some(Arc::new(f)), ..attrs });
            }
            p.style = ParaStyle { toc: Some(TocMark { level, opts }), indent_left: (level - 1) as f32 * TOC_INDENT_PT, space_after: Some(5.0), ..Default::default() };
            entries.push(p);
        }
        if entries.is_empty() {
            entries.push(Paragraph::new(none_text).with_style(ParaStyle { toc: Some(TocMark { level: 0, opts }), ..Default::default() }));
        }
        let mut out = Vec::with_capacity(hi - lo + entries.len());
        let mut start = lo;
        let mut entries = Some(entries);
        for i in lo..hi {
            if i == a && split.is_none() {
                start = lo + out.len();
                out.extend(entries.take().unwrap_or_default());
            }
            if (a..b).contains(&i) {
                continue;
            }
            let mut p = self.paragraphs[i].clone();
            if adds.contains(&i) {
                let name = names[&i].clone();
                p.marks.push(Mark { name: name.clone(), offset: 0, end: false });
                p.marks.push(Mark { name, offset: p.len(), end: true });
            }
            match split {
                Some(s) if s.para == i => {
                    let tail = p.split_off(s.offset);
                    out.push(p);
                    start = lo + out.len();
                    out.extend(entries.take().unwrap_or_default());
                    out.push(tail);
                }
                _ => out.push(p),
            }
        }
        if let Some(e) = entries.take() {
            start = lo + out.len();
            out.extend(e);
        }
        (lo, hi, out, start)
    }

    /// The entries of an alphabetical index: main entries (level 1) with
    /// their subentries (level 2), sorted, each followed by its pages.
    fn index_entries(&self, opts: TocOpts) -> Vec<Paragraph> {
        let refs = self.refs();
        // Main entry: its text, whether it has pages itself, and subentries.
        let mut mains: Vec<(String, String, bool, Vec<(String, String)>)> = Vec::new();
        let mut main_at: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut subs: std::collections::HashSet<(usize, String)> = std::collections::HashSet::new();
        for (_, t) in &refs.index {
            let (m, sub) = index_parts(t);
            if m.is_empty() {
                continue;
            }
            let k = fold(&m);
            let i = match main_at.get(&k) {
                Some(&i) => i,
                None => {
                    main_at.insert(k.clone(), mains.len());
                    mains.push((k, m.clone(), false, Vec::new()));
                    mains.len() - 1
                }
            };
            match sub {
                Some(s) => {
                    let sk = fold(&s);
                    if subs.insert((i, sk.clone())) {
                        mains[i].3.push((sk, s));
                    }
                }
                None => mains[i].2 = true,
            }
        }
        mains.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        let entry = |text: &str, key: &str, level: u8, pages: bool| {
            let mut p = Paragraph::new(text);
            if pages && opts.page_numbers {
                p.push(if opts.right_align { "\t" } else { ", " }, Attrs::default());
                p.push(&OBJECT.to_string(), Attrs { field: Some(Arc::new(Field::new(FieldKind::IndexPages(key.to_string())))), ..Default::default() });
            }
            p.style = ParaStyle { toc: Some(TocMark { level, opts }), indent_left: (level - 1) as f32 * TOC_INDENT_PT, space_after: Some(0.0), ..Default::default() };
            p
        };
        let mut out = Vec::new();
        let mut letter = None;
        for (k, m, own, mut subs) in mains {
            let first = k.chars().next().map(|c| if c.is_alphabetic() { c.to_uppercase().collect::<String>() } else { "#".into() });
            if opts.letters && first != letter {
                letter.clone_from(&first);
                let mut h = Paragraph::new("");
                h.push(first.as_deref().unwrap_or(""), Attrs { bold: true, ..Default::default() });
                h.style = ParaStyle { toc: Some(TocMark { level: 0, opts }), space_before: Some(if out.is_empty() { 0.0 } else { 8.0 }), space_after: Some(2.0), ..Default::default() };
                out.push(h);
            }
            out.push(entry(&m, &m, 1, own));
            subs.sort();
            for (_, s) in subs {
                out.push(entry(&s, &format!("{m}:{s}"), 2, true));
            }
        }
        out
    }

    /// The entries of a table of authorities: the long citations of its
    /// category, sorted, each followed by its pages.
    fn authority_entries(&self, opts: TocOpts) -> Vec<Paragraph> {
        let refs = self.refs();
        let mut longs: Vec<(String, String)> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (_, l, c) in &refs.authorities {
            let k = fold(l);
            if *c == opts.toa && seen.insert(k.clone()) {
                longs.push((k, l.clone()));
            }
        }
        longs.sort();
        longs
            .into_iter()
            .map(|(_, l)| {
                let mut p = Paragraph::new(&l.replace(['\r', '\n'], " "));
                if opts.page_numbers {
                    p.push(if opts.right_align { "\t" } else { ", " }, Attrs::default());
                    p.push(&OBJECT.to_string(), Attrs { field: Some(Arc::new(Field::new(FieldKind::AuthorityPages(l.clone())))), ..Default::default() });
                }
                p.style = ParaStyle { toc: Some(TocMark { level: 1, opts }), space_after: Some(4.0), ..Default::default() };
                p
            })
            .collect()
    }

    /// The entries of a bibliography, one paragraph per source.
    fn bib_entries(&self, opts: TocOpts) -> Vec<Paragraph> {
        let style = self.biblio.style;
        self.bibliography_order()
            .into_iter()
            .map(|(s, n)| {
                let mut p = Paragraph::new("");
                for (t, it) in s.entry(style, n) {
                    p.push(&t, Attrs { italic: it, ..Default::default() });
                }
                p.style = ParaStyle { toc: Some(TocMark { level: 1, opts }), space_after: Some(6.0), ..Default::default() };
                p
            })
            .collect()
    }

    /// Marks `text` as the index entry `entry` after each paragraph's first
    /// occurrence of it (whole words, same case), outside indexes and tables
    /// of contents, as one undo step. Returns how many were marked.
    pub fn mark_index_entries(&mut self, text: &str, entry: &str, sel: Selection) -> usize {
        self.mark_all(text, FieldKind::IndexEntry(entry.to_string()), sel)
    }

    /// Puts a field of kind `kind` after each paragraph's first occurrence
    /// of `text` (see `mark_index_entries`).
    pub fn mark_all(&mut self, text: &str, kind: FieldKind, sel: Selection) -> usize {
        let mut hits = self.find_all_opts(text, true, true);
        hits.retain(|h| self.paragraphs[h.0.para].style.toc.is_none());
        hits.dedup_by_key(|h| h.0.para);
        let (Some(first), Some(last)) = (hits.first().map(|h| h.0.para), hits.last().map(|h| h.0.para)) else { return 0 };
        let before = self.paragraphs[first..=last].to_vec();
        for &(_, b) in hits.iter().rev() {
            let mut attrs = self.typing_attrs(b);
            (attrs.link, attrs.link_tip) = (None, None);
            attrs.field = Some(Arc::new(Field::new(kind.clone())));
            self.paragraphs[b.para].insert_str(b.offset, &OBJECT.to_string(), attrs);
        }
        self.record(first, before, last - first + 1, sel, sel, EditKind::Field);
        hits.len()
    }

    /// Inserts a table of contents at the caret (or updates the one there
    /// with new settings). Returns the caret after it.
    pub fn insert_toc(&mut self, sel: Selection, opts: TocOpts, none_text: &str) -> Selection {
        let p = self.clamp(sel.start());
        if let Some((a, b)) = self.toc_at(p.para) {
            self.rebuild_toc(a, b, opts, sel, none_text);
            return Selection::caret(Pos::new(a, 0));
        }
        if self.in_table(p.para) {
            return sel;
        }
        let (a, split) = if p.offset == 0 { (p.para, None) } else { (p.para + 1, Some(p)) };
        let (lo, hi, new, start) = self.toc_plan(a, a, split, opts, none_text);
        let n = new.iter().skip(start - lo).take_while(|q| q.style.toc.is_some()).count();
        let caret = Selection::caret(Pos::new(start + n, 0));
        self.replace_paras(lo, hi, new, sel, caret, EditKind::Toc);
        caret
    }

    fn rebuild_toc(&mut self, a: usize, b: usize, opts: TocOpts, sel: Selection, none_text: &str) -> bool {
        let (lo, hi, new, _) = self.toc_plan(a, b, None, opts, none_text);
        if new.len() == hi - lo && new.iter().zip(&self.paragraphs[lo..hi]).all(|(x, y)| x == y) {
            return false;
        }
        let delta = new.len() as isize - (hi - lo) as isize;
        let fix = |p: Pos| -> Pos {
            if p.para >= b {
                Pos::new((p.para as isize + delta) as usize, p.offset)
            } else if p.para >= a {
                Pos::new(a, 0)
            } else {
                p
            }
        };
        let after = Selection { anchor: fix(sel.anchor), cursor: fix(sel.cursor) };
        self.replace_paras(lo, hi, new, sel, after, EditKind::Toc);
        true
    }

    /// Rebuilds the table of contents holding paragraph `para` from the
    /// headings. False if there's none or nothing changed.
    pub fn update_toc(&mut self, para: usize, sel: Selection, none_text: &str) -> bool {
        let Some((a, b)) = self.toc_at(para) else { return false };
        let opts = self.paragraphs[a].style.toc.map(|t| t.opts).unwrap_or_default();
        self.rebuild_toc(a, b, opts, sel, none_text)
    }

    /// The selection after the last undo step.
    pub fn last_sel_after(&self) -> Option<Selection> {
        self.undo.last().map(|u| u.sel_after)
    }

    /// Updates every table of contents. Returns the selection moved along.
    pub fn update_tocs(&mut self, mut sel: Selection, none_text: &str) -> (bool, Selection) {
        let mut any = false;
        // From the last, so earlier blocks keep their indices.
        for (a, _) in self.toc_blocks().into_iter().rev() {
            if self.update_toc(a, sel, none_text) {
                any = true;
                if let Some(u) = self.undo.last() {
                    sel = u.sel_after;
                }
            }
        }
        (any, sel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(t: &str, l: u8) -> Paragraph {
        Paragraph::new(t).with_style(ParaStyle { kind: ParaKind::Heading(l), ..Default::default() })
    }

    #[test]
    fn toc_insert_update() {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("Start"), h("One", 1), Paragraph::new("x"), h("Sub", 2), h("Deep", 4), h("Two", 1)]);
        let c = d.insert_toc(Selection::caret(Pos::new(0, 0)), TocOpts::default(), "none");
        let (a, b) = d.toc_blocks()[0];
        assert_eq!((a, b), (0, 3));
        assert_eq!(c.cursor, Pos::new(3, 0));
        assert_eq!(d.paragraphs[0].text, format!("One\t{OBJECT}"));
        assert_eq!(d.paragraphs[1].style.toc.unwrap().level, 2);
        assert_eq!(d.paragraphs[1].style.indent_left, TOC_INDENT_PT);
        // Entries link to bookmarks on their headings.
        let link = d.paragraphs[0].runs[0].attrs.link.clone().unwrap();
        let b = d.bookmark(&link[1..]).unwrap();
        assert_eq!(b.start, Pos::new(4, 0));
        let f = d.field_at(Pos::new(0, 4)).unwrap().1;
        assert_eq!(f.kind, FieldKind::PageRef(link[1..].to_string()));
        // One undo step removes it all, bookmarks included.
        d.undo();
        assert!(d.toc_blocks().is_empty() && d.bookmarks().is_empty());
        d.redo();
        // Updating after a heading changes; nothing to do the second time.
        d.paragraphs[8].text = "Two!".into();
        d.paragraphs[8].runs[0].len = 4;
        let sel = Selection::caret(Pos::new(8, 1));
        assert!(d.update_toc(1, sel, "none"));
        assert_eq!(d.paragraphs[2].text, format!("Two!\t{OBJECT}"));
        assert!(!d.update_toc(0, sel, "none"));
        let mut d2 = d.clone();
        d2.insert_toc(Selection::caret(Pos::new(0, 0)), TocOpts { levels: 1, page_numbers: false, ..Default::default() }, "none");
        assert_eq!(d2.toc_blocks(), vec![(0, 2)]);
        assert_eq!(d2.paragraphs[0].text, "One");
        // Splitting a paragraph; no headings.
        let mut e = Document::from_plain_text("abcd");
        e.insert_toc(Selection::caret(Pos::new(0, 2)), TocOpts::default(), "No entries");
        let t: Vec<&str> = e.paragraphs.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(t, ["ab", "No entries", "cd"]);
    }

    #[test]
    fn alphabetical_index() {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("zeta apple"), Paragraph::new("Árbol"), Paragraph::new("end")]);
        let xe = |d: &mut Document, p: Pos, t: &str| {
            d.insert_field(Selection::caret(p), Field::new(FieldKind::IndexEntry(t.into())));
        };
        xe(&mut d, Pos::new(0, 4), "Zeta");
        xe(&mut d, Pos::new(0, 0), "apple:green");
        xe(&mut d, Pos::new(1, 0), "árbol");
        xe(&mut d, Pos::new(2, 0), "Apple");
        assert_eq!(Field::parse(" XE \"a:b\" ", "").unwrap().kind, FieldKind::IndexEntry("a:b".into()));
        let opts = TocOpts { index: true, letters: true, right_align: false, ..Default::default() };
        d.insert_toc(Selection::caret(Pos::new(3, 0)), opts, "none");
        let (a, b) = d.toc_blocks()[0];
        let t: Vec<String> = d.paragraphs[a..b].iter().map(|p| p.text.replace(OBJECT, "#")).collect();
        assert_eq!(t, ["A", "apple, #", "green, #", "árbol, #", "Z", "Zeta, #"]);
        assert_eq!(d.paragraphs[a + 2].style.toc.unwrap().level, 2);
        let f = d.field_at(Pos::new(a + 2, 7)).unwrap().1;
        assert_eq!(f.kind, FieldKind::IndexPages("apple:green".into()));
        assert!(f.needs_pages());
        // Pages: distinct, sorted.
        let r = d.refs();
        assert_eq!(r.index.len(), 4);
        let page_of = |p: Pos| Some(if p.para == 0 { 2 } else { 1 });
        let label = |n: usize| n.to_string();
        let note = |_: Pos| None;
        let cx = crate::FieldCtx { page: 1, pages: 2, now: (2026, 1, 1, 0, 0, 0), date: "", time: "", months: &[], days: &[], file: "", page_of: &page_of, page_label: &label, above: "", below: "", on_page: "", note_label: &note, decimal: '.' };
        let f = Field::new(FieldKind::IndexPages("APPLE".into()));
        assert_eq!(r.value(&d, &f, Pos::new(a, 0), &cx), "1");
        xe(&mut d, Pos::new(0, OBJECT.len_utf8()), "apple");
        let r = d.refs();
        assert_eq!(r.value(&d, &f, Pos::new(a, 0), &cx), "1, 2");
        // Marking every occurrence: one per paragraph, not in the index.
        let mut m = Document::from_paragraphs(vec![Paragraph::new("cat and cat"), Paragraph::new("category"), Paragraph::new("a cat")]);
        assert_eq!(m.mark_index_entries("cat", "Cat", Selection::default()), 2);
        assert_eq!(m.paragraphs[0].text, format!("cat{OBJECT} and cat"));
        assert_eq!(m.paragraphs[2].text, format!("a cat{OBJECT}"));
        m.undo();
        assert_eq!(m.paragraphs[0].text, "cat and cat");
    }

    #[test]
    fn table_of_authorities() {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("a"), Paragraph::new("b"), Paragraph::new("c"), Paragraph::new("")]);
        let ta = |d: &mut Document, para: usize, instr: &str| {
            let f = Field::parse(instr, "").unwrap();
            d.insert_field(Selection::caret(Pos::new(para, 1)), f);
        };
        ta(&mut d, 0, r#" TA \l "Smith v. Jones, 1 U.S. 1 (1990)" \s "Smith" \c 1 "#);
        ta(&mut d, 1, r#" TA \s "Smith" "#);
        ta(&mut d, 1, r#" TA \l "Adams v. Bell" \c 1 "#);
        ta(&mut d, 2, r#" TA \l "Tax Act" \c 2 "#);
        let f = Field::parse(r#" TA \l "X" \s "Y" \c 3 "#, "").unwrap();
        assert_eq!(Field::parse(&f.instr(), "").unwrap(), f);
        let r = d.refs();
        assert_eq!(r.authorities.len(), 4);
        assert_eq!(r.authorities.iter().filter(|a| a.1.starts_with("Smith")).count(), 2);
        d.insert_toc(Selection::caret(Pos::new(3, 0)), TocOpts { toa: 1, ..Default::default() }, "none");
        let (a, b) = d.toc_blocks()[0];
        let t: Vec<String> = d.paragraphs[a..b].iter().map(|p| p.text.replace(OBJECT, "#")).collect();
        assert_eq!(t, ["Adams v. Bell\t#", "Smith v. Jones, 1 U.S. 1 (1990)\t#"]);
        let page_of = |p: Pos| Some(p.para + 1);
        let label = |n: usize| n.to_string();
        let note = |_: Pos| None;
        let cx = crate::FieldCtx { page: 1, pages: 2, now: (2026, 1, 1, 0, 0, 0), date: "", time: "", months: &[], days: &[], file: "", page_of: &page_of, page_label: &label, above: "", below: "", on_page: "", note_label: &note, decimal: '.' };
        let f = d.field_at(Pos::new(a + 1, "Smith v. Jones, 1 U.S. 1 (1990)\t".len())).unwrap().1;
        assert_eq!(d.refs().value(&d, &f, Pos::new(a, 0), &cx), "1, 2");
    }

    #[test]
    fn move_and_level_paragraphs() {
        let mut d = Document::from_paragraphs(["a", "b", "c", "d"].map(Paragraph::new).to_vec());
        let s = Selection::caret(Pos::new(0, 0));
        let after = d.move_paras(2, 4, 0, s).unwrap();
        assert_eq!(d.paragraphs.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(), ["c", "d", "a", "b"]);
        assert_eq!((after.anchor.para, after.cursor.para), (0, 1));
        assert!(d.move_paras(0, 2, 1, s).is_none());
        d.move_paras(0, 1, 4, s).unwrap();
        assert_eq!(d.paragraphs.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(), ["d", "a", "b", "c"]);
        d.set_outline_levels(0, 1, s, &|l| Some(l.map_or(2, |l| l - 1)));
        assert_eq!(d.paragraphs[0].style.kind, ParaKind::Heading(2));
        d.set_outline_levels(0, 0, s, &|l| l.map(|l| l + 1));
        assert_eq!(d.paragraphs[0].style.kind, ParaKind::Heading(3));
        d.set_outline_levels(0, 0, s, &|_| None);
        assert_eq!(d.paragraphs[0].style.kind, ParaKind::Normal);
        d.undo();
        assert_eq!(d.paragraphs[0].style.kind, ParaKind::Heading(3));
    }

    #[test]
    fn move_sections() {
        let mut d = Document::from_paragraphs(vec![h("A", 1), Paragraph::new("a"), h("A1", 2), Paragraph::new("a1"), h("B", 1), Paragraph::new("b")]);
        assert_eq!(d.section_end(0), 4);
        assert_eq!(d.section_end(2), 4);
        assert_eq!(d.headings().len(), 3);
        let c = d.move_section(4, 0, Selection::default()).unwrap();
        assert_eq!(c.cursor, Pos::new(0, 0));
        let t: Vec<&str> = d.paragraphs.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(t, ["B", "b", "A", "a", "A1", "a1"]);
        d.undo();
        let t: Vec<&str> = d.paragraphs.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(t, ["A", "a", "A1", "a1", "B", "b"]);
        // Down, to the end.
        let c = d.move_section(0, 6, Selection::default()).unwrap();
        assert_eq!(c.cursor, Pos::new(2, 0));
        assert_eq!(d.paragraphs[2].text, "A");
        // Into its own section: nothing.
        assert!(d.move_section(2, 3, Selection::default()).is_none());
    }
}
