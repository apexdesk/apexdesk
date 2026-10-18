//! Fields: page numbers, dates, document properties, sequence numbers and
//! cross-references. A field is an `OBJECT` character with `Attrs::field`
//! set; views show its current value, computed when they lay the document
//! out, and files keep it as a field.

use std::collections::HashMap;
use std::sync::Arc;

use crate::{Bookmark, Document, EditKind, NoteKind, OBJECT, Pos, Selection};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FieldKind {
    Page,
    NumPages,
    Date,
    Time,
    Author,
    Title,
    FileName,
    /// DOCPROPERTY: a document property, built-in (Title, Company...) or
    /// custom, by name.
    DocProperty(String),
    /// SEQ: numbers captions and lists ("Figure 1", "Table 2"), by name.
    Seq(String),
    /// REF: the text of a bookmark.
    Ref(String),
    /// PAGEREF: the page of a bookmark.
    PageRef(String),
    /// NOTEREF: the number of the note in a bookmark.
    NoteRef(String),
    /// A table formula ("SUM(ABOVE)", "B2*2"): `{ =expr \# "picture" }`.
    Formula(String),
    /// XE: an entry of the alphabetical index ("Main:Sub"); shows nothing.
    IndexEntry(String),
    /// The pages of the index entries with this text, in an index entry
    /// (a result of the INDEX field, kept as text in files).
    IndexPages(String),
    /// CITATION: a source of the bibliography, by tag.
    Citation(String),
    /// TA: an entry of a table of authorities: its long citation, short
    /// citation and category (1..=7); shows nothing. An entry with only a
    /// short citation refers back to the long one.
    Authority { long: String, short: String, cat: u8 },
    /// The pages of the authorities with this long citation, in a table
    /// of authorities (text in files).
    AuthorityPages(String),
}

/// A field. Switches follow Word's field codes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Field {
    pub kind: FieldKind,
    /// `\h`: the result links to its target.
    pub hyperlink: bool,
    /// `\p`: "above" / "below" (and "on page N" for page references).
    pub position: bool,
    /// `\@`: date and time picture ("dd/MM/yyyy"); for formulas, the
    /// `\#` number picture ("#,##0.00").
    pub format: Option<String>,
    /// The last result (from the file or the last update), shown when the
    /// value can't be worked out.
    pub cached: String,
}

impl Field {
    pub fn new(kind: FieldKind) -> Field {
        let hyperlink = matches!(kind, FieldKind::Ref(_) | FieldKind::PageRef(_) | FieldKind::NoteRef(_));
        Field { kind, hyperlink, position: false, format: None, cached: String::new() }
    }

    /// Parses a field instruction (" PAGE \* MERGEFORMAT "). `None` for
    /// fields that aren't handled here (their result stays as plain text).
    pub fn parse(instr: &str, cached: &str) -> Option<Field> {
        if let Some(rest) = instr.trim_start().strip_prefix('=') {
            let (expr, switches) = rest.split_once('\\').map_or((rest, ""), |(e, s)| (e, s));
            let mut f = Field::new(FieldKind::Formula(expr.trim().to_string()));
            let w = words(&format!("\\{switches}"));
            f.format = w.iter().position(|s| s == "\\#").and_then(|i| w.get(i + 1)).cloned();
            f.cached = cached.to_string();
            return Some(f);
        }
        let w = words(instr);
        let name = w.first()?.to_ascii_uppercase();
        let arg = || w.get(1).filter(|a| !a.starts_with('\\')).cloned();
        let kind = match name.as_str() {
            "PAGE" => FieldKind::Page,
            "NUMPAGES" | "SECTIONPAGES" => FieldKind::NumPages,
            "DATE" | "CREATEDATE" | "SAVEDATE" | "PRINTDATE" => FieldKind::Date,
            "TIME" => FieldKind::Time,
            "AUTHOR" | "LASTSAVEDBY" => FieldKind::Author,
            "TITLE" => FieldKind::Title,
            "FILENAME" => FieldKind::FileName,
            "DOCPROPERTY" => FieldKind::DocProperty(arg()?),
            "SEQ" => FieldKind::Seq(arg()?),
            "REF" => FieldKind::Ref(arg()?),
            "PAGEREF" => FieldKind::PageRef(arg()?),
            "NOTEREF" => FieldKind::NoteRef(arg()?),
            "CITATION" => FieldKind::Citation(arg()?),
            "TA" => {
                let val = |sw: &str| w.iter().position(|x| x.eq_ignore_ascii_case(sw)).and_then(|i| w.get(i + 1)).filter(|a| !a.starts_with('\\')).cloned().unwrap_or_default();
                let cat = val("\\c").trim().parse::<u8>().unwrap_or(1).clamp(1, 16);
                let (long, short) = (val("\\l"), val("\\s"));
                if long.is_empty() && short.is_empty() {
                    return None;
                }
                FieldKind::Authority { long, short, cat }
            }
            "XE" => FieldKind::IndexEntry(arg()?.trim().to_string()),
            _ => return None,
        };
        let mut f = Field::new(kind);
        f.hyperlink = w.iter().any(|s| s.eq_ignore_ascii_case("\\h"));
        f.position = w.iter().any(|s| s.eq_ignore_ascii_case("\\p"));
        f.format = w.iter().position(|s| s == "\\@").and_then(|i| w.get(i + 1)).cloned();
        f.cached = cached.to_string();
        Some(f)
    }

    /// The instruction, as written to files.
    pub fn instr(&self) -> String {
        let mut s = match &self.kind {
            FieldKind::Page => "PAGE".to_string(),
            FieldKind::NumPages => "NUMPAGES".to_string(),
            FieldKind::Date => "DATE".to_string(),
            FieldKind::Time => "TIME".to_string(),
            FieldKind::Author => "AUTHOR".to_string(),
            FieldKind::Title => "TITLE".to_string(),
            FieldKind::FileName => "FILENAME".to_string(),
            FieldKind::DocProperty(n) => format!("DOCPROPERTY {}", quote(n)),
            FieldKind::Seq(n) => format!("SEQ {} \\* ARABIC", quote(n)),
            FieldKind::Ref(b) => format!("REF {}", quote(b)),
            FieldKind::PageRef(b) => format!("PAGEREF {}", quote(b)),
            FieldKind::NoteRef(b) => format!("NOTEREF {}", quote(b)),
            FieldKind::Citation(t) => format!("CITATION {} \\l 1033", quote(t)),
            FieldKind::IndexEntry(t) => format!("XE \"{}\"", t.replace('"', "")),
            FieldKind::Authority { long, short, cat } => {
                let mut s = String::from("TA");
                if !long.is_empty() {
                    s.push_str(&format!(" \\l \"{}\"", long.replace('"', "")));
                }
                if !short.is_empty() {
                    s.push_str(&format!(" \\s \"{}\"", short.replace('"', "")));
                }
                s.push_str(&format!(" \\c {cat}"));
                s
            }
            FieldKind::AuthorityPages(t) => format!("TOAPAGES \"{}\"", t.replace('"', "")),
            FieldKind::IndexPages(t) => format!("INDEXPAGES \"{}\"", t.replace('"', "")),
            FieldKind::Formula(e) => {
                let mut s = format!("={e}");
                if let Some(p) = &self.format {
                    s.push_str(&format!(" \\# \"{}\"", p.replace('"', "")));
                }
                return format!(" {s} ");
            }
        };
        if let Some(f) = &self.format {
            s.push_str(&format!(" \\@ \"{}\"", f.replace('"', "")));
        }
        if self.hyperlink && self.target().is_some() {
            s.push_str(" \\h");
        }
        if self.position {
            s.push_str(" \\p");
        }
        format!(" {s} ")
    }

    /// The bookmark a reference points to.
    pub fn target(&self) -> Option<&str> {
        match &self.kind {
            FieldKind::Ref(b) | FieldKind::PageRef(b) | FieldKind::NoteRef(b) => Some(b),
            _ => None,
        }
    }

    /// True if the value depends on where the document breaks into pages.
    pub fn needs_pages(&self) -> bool {
        matches!(self.kind, FieldKind::Page | FieldKind::NumPages | FieldKind::PageRef(_) | FieldKind::IndexPages(_) | FieldKind::AuthorityPages(_)) || (self.position && self.target().is_some())
    }
}

fn quote(s: &str) -> String {
    // An empty name is written "" so the field reads back (REF "").
    if s.is_empty() || s.contains(char::is_whitespace) { format!("\"{s}\"") } else { s.to_string() }
}

/// Splits a field instruction into words, honouring double quotes.
pub fn words(instr: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut has = false;
    for c in instr.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                has = true;
            }
            c if c.is_whitespace() && !quoted => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            c => {
                cur.push(c);
                has = true;
            }
        }
    }
    if has {
        out.push(cur);
    }
    out
}

/// Local date and time: year, month, day, hour, minute, second.
pub type DateTime = (i64, u32, u32, u32, u32, u32);

/// Day of the week, 0 = Sunday.
fn weekday(y: i64, m: u32, d: u32) -> usize {
    let (y, m) = if m < 3 { (y - 1, m + 12) } else { (y, m) };
    let k = y.rem_euclid(100);
    let j = y.div_euclid(100);
    let h = (d as i64 + (13 * (m as i64 + 1)) / 5 + k + k / 4 + j / 4 + 5 * j).rem_euclid(7);
    ((h + 6) % 7) as usize
}

/// Formats a date with a Word date picture: d dd ddd dddd, M MM MMM MMMM,
/// yy yyyy, h hh H HH, m mm, s ss, AM/PM am/pm, and 'quoted text'.
/// `months` and `days` are the full names (January.., Sunday..).
pub fn format_date(pic: &str, t: DateTime, months: &[&str], days: &[&str]) -> String {
    let (y, mo, d, h, mi, s) = t;
    let chars: Vec<char> = pic.chars().collect();
    let has_ampm = pic.to_ascii_lowercase().contains("am/pm");
    let mut out = String::new();
    let mut i = 0;
    let name = |list: &[&str], i: usize, short: bool| -> String {
        let n = list.get(i).copied().unwrap_or("");
        if short { n.chars().take(3).collect() } else { n.to_string() }
    };
    while i < chars.len() {
        let c = chars[i];
        let mut n = 1;
        while i + n < chars.len() && chars[i + n] == c {
            n += 1;
        }
        match c {
            '\'' => {
                let end = chars[i + 1..].iter().position(|&x| x == '\'').map_or(chars.len(), |p| i + 1 + p);
                out.extend(&chars[i + 1..end]);
                i = end + 1;
                continue;
            }
            'd' => out.push_str(&match n {
                1 => d.to_string(),
                2 => format!("{d:02}"),
                3 => name(days, weekday(y, mo, d), true),
                _ => name(days, weekday(y, mo, d), false),
            }),
            'M' => out.push_str(&match n {
                1 => mo.to_string(),
                2 => format!("{mo:02}"),
                3 => name(months, mo.saturating_sub(1) as usize, true),
                _ => name(months, mo.saturating_sub(1) as usize, false),
            }),
            'y' => out.push_str(&if n <= 2 { format!("{:02}", y.rem_euclid(100)) } else { y.to_string() }),
            'H' => out.push_str(&if n == 1 { h.to_string() } else { format!("{h:02}") }),
            'h' => {
                let h12 = if h % 12 == 0 { 12 } else { h % 12 };
                let h = if has_ampm { h12 } else { h };
                out.push_str(&if n == 1 { h.to_string() } else { format!("{h:02}") });
            }
            'm' => out.push_str(&if n == 1 { mi.to_string() } else { format!("{mi:02}") }),
            's' => out.push_str(&if n == 1 { s.to_string() } else { format!("{s:02}") }),
            'A' | 'a' if chars.get(i..i + 5).is_some_and(|w| w.iter().map(char::to_ascii_lowercase).eq("am/pm".chars())) => {
                let pm = h >= 12;
                let t = if c == 'A' { if pm { "PM" } else { "AM" } } else if pm { "pm" } else { "am" };
                out.push_str(t);
                i += 5;
                continue;
            }
            _ => {
                for _ in 0..n {
                    out.push(c);
                }
            }
        }
        i += n;
    }
    out
}

/// Values a view supplies to compute fields.
pub struct FieldCtx<'a> {
    /// Page of the field (1-based) and number of pages.
    pub page: usize,
    pub pages: usize,
    pub now: DateTime,
    /// Default date and time formats when a field has none.
    pub date: &'a str,
    pub time: &'a str,
    pub months: &'a [&'a str],
    pub days: &'a [&'a str],
    pub file: &'a str,
    /// Page (1-based) of a position.
    pub page_of: &'a dyn Fn(Pos) -> Option<usize>,
    /// The number a page (1-based) shows, in its section's format.
    pub page_label: &'a dyn Fn(usize) -> String,
    /// "above", "below" and "on page {n}".
    pub above: &'a str,
    pub below: &'a str,
    pub on_page: &'a str,
    /// Label of a note mark, as the view numbers it.
    pub note_label: &'a dyn Fn(Pos) -> Option<String>,
    /// Decimal separator of numbers (table formulas).
    pub decimal: char,
}

/// What fields refer to, gathered once per document change: bookmarks,
/// note numbers and sequence numbers.
#[derive(Default)]
pub struct Refs {
    bookmarks: HashMap<String, Bookmark>,
    /// Note marks in order with their kind and number (1-based, per kind,
    /// before the numbering's start value).
    pub notes: Vec<(Pos, NoteKind, usize)>,
    seqs: HashMap<Pos, usize>,
    /// Index entries (XE fields) outside indexes: position and text.
    pub index: Vec<(Pos, String)>,
    /// Table of authorities entries: position, long citation, category.
    pub authorities: Vec<(Pos, String, u8)>,
    /// Where each index key and long citation (folded) is.
    index_at: HashMap<String, Vec<Pos>>,
    authority_at: HashMap<String, Vec<Pos>>,
    /// Number of each cited source (lower case tag), by first citation.
    cites: HashMap<String, usize>,
}

impl Refs {
    /// The number of the SEQ field at `at`.
    pub fn seq(&self, at: Pos) -> Option<usize> {
        self.seqs.get(&at).copied()
    }

    pub fn bookmark(&self, name: &str) -> Option<&Bookmark> {
        self.bookmarks.get(&name.to_lowercase())
    }

    /// Continuous number of the note whose mark is at `p`.
    pub fn note_number(&self, p: Pos) -> Option<(NoteKind, usize)> {
        let i = self.notes.binary_search_by(|n| n.0.cmp(&p)).ok()?;
        Some((self.notes[i].1, self.notes[i].2))
    }

    /// The first note mark in `a..=b`.
    fn note_in(&self, a: Pos, b: Pos) -> Option<Pos> {
        let i = self.notes.partition_point(|n| n.0 < a);
        self.notes.get(i).filter(|n| n.0 <= b).map(|n| n.0)
    }

    /// The current value of field `f` whose character is at `at`.
    pub fn value(&self, doc: &Document, f: &Field, at: Pos, cx: &FieldCtx) -> String {
        let fallback = || f.cached.clone();
        let above_below = |p: Pos| if p < at { cx.above.to_string() } else { cx.below.to_string() };
        match &f.kind {
            FieldKind::Page => (cx.page_label)(cx.page),
            FieldKind::NumPages => cx.pages.to_string(),
            FieldKind::Date | FieldKind::Time => {
                let (y, mo, d, h, mi, s) = cx.now;
                match &f.format {
                    Some(p) if !p.is_empty() => format_date(p, (y, mo, d, h, mi, s), cx.months, cx.days),
                    _ if f.kind == FieldKind::Date => cx.date.to_string(),
                    _ => cx.time.to_string(),
                }
            }
            FieldKind::Author => if doc.meta.author.is_empty() { fallback() } else { doc.meta.author.clone() },
            FieldKind::Title => if doc.meta.title.is_empty() { fallback() } else { doc.meta.title.clone() },
            FieldKind::FileName => if cx.file.is_empty() { fallback() } else { cx.file.to_string() },
            FieldKind::DocProperty(n) => doc.meta.property(n).filter(|v| !v.is_empty()).unwrap_or_else(fallback),
            FieldKind::Seq(_) => self.seqs.get(&at).map_or_else(fallback, |n| n.to_string()),
            FieldKind::Ref(name) => {
                let Some(b) = self.bookmark(name) else { return fallback() };
                if f.position {
                    return above_below(b.start);
                }
                // The first few thousand characters: many fields over one
                // long bookmark would copy it again and again.
                let mut t = String::new();
                for p in b.start.para..=b.end.para.min(doc.paragraphs.len().saturating_sub(1)) {
                    if t.len() >= MAX_REF {
                        break;
                    }
                    if p > b.start.para {
                        t.push(' ');
                    }
                    let s = &doc.paragraphs[p].text;
                    let e = if p == b.end.para { b.end.offset.min(s.len()) } else { s.len() };
                    let a = if p == b.start.para { b.start.offset.min(e) } else { 0 };
                    let piece = s.get(a..e).unwrap_or_default();
                    t.extend(piece.chars().filter(|&c| c != OBJECT).map(|c| if c == '\n' { ' ' } else { c }).take(MAX_REF));
                }
                if t.trim().is_empty() { fallback() } else { t.trim().to_string() }
            }
            FieldKind::PageRef(name) => {
                let Some(b) = self.bookmark(name) else { return fallback() };
                let Some(pg) = (cx.page_of)(b.start) else { return fallback() };
                if f.position {
                    if pg == cx.page { above_below(b.start) } else { cx.on_page.replace("{n}", &(cx.page_label)(pg)) }
                } else {
                    (cx.page_label)(pg)
                }
            }
            FieldKind::NoteRef(name) => {
                let Some(b) = self.bookmark(name) else { return fallback() };
                if f.position {
                    return above_below(b.start);
                }
                self.note_in(b.start, b.end).and_then(|p| (cx.note_label)(p)).unwrap_or_else(fallback)
            }
            FieldKind::IndexEntry(_) | FieldKind::Authority { .. } => String::new(),
            FieldKind::AuthorityPages(long) => {
                let at = self.authority_at.get(&crate::fold(long)).map_or(&[][..], Vec::as_slice);
                let mut pages: Vec<usize> = at.iter().filter_map(|&p| (cx.page_of)(p)).collect();
                if pages.is_empty() {
                    return fallback();
                }
                pages.sort_unstable();
                pages.dedup();
                pages.iter().map(|&p| (cx.page_label)(p)).collect::<Vec<_>>().join(", ")
            }
            FieldKind::Citation(tag) => match doc.biblio.source(tag) {
                Some(s) => s.citation(doc.biblio.style, self.cites.get(&tag.to_lowercase()).copied().unwrap_or(0)),
                None => fallback(),
            },
            FieldKind::IndexPages(key) => {
                let at = self.index_at.get(&crate::index_key(key)).map_or(&[][..], Vec::as_slice);
                let mut pages: Vec<usize> = at.iter().filter_map(|&p| (cx.page_of)(p)).collect();
                if pages.is_empty() {
                    return fallback();
                }
                pages.sort_unstable();
                pages.dedup();
                pages.iter().map(|&p| (cx.page_label)(p)).collect::<Vec<_>>().join(", ")
            }
            FieldKind::Formula(e) => match doc.formula_value(at, e) {
                Ok(v) => crate::format_number(v, f.format.as_deref(), cx.decimal),
                Err(()) => "!Syntax Error".to_string(),
            },
        }
    }
}

impl Document {
    /// Gathers what fields refer to (see `Refs`).
    pub fn refs(&self) -> Refs {
        let mut r = Refs::default();
        let mut counts = [0usize; 2];
        let mut seq: HashMap<String, usize> = HashMap::new();
        let mut shorts: HashMap<String, (String, u8)> = HashMap::new();
        let mut any = false;
        for (i, p) in self.paragraphs.iter().enumerate() {
            if !p.marks.is_empty() {
                any = true;
            }
            if !crate::para_has_objects(p) {
                continue;
            }
            for (off, a) in crate::objects_of(p) {
                let at = Pos::new(i, off);
                if let Some(n) = &a.note {
                    let k = n.kind as usize;
                    counts[k] += 1;
                    r.notes.push((at, n.kind, counts[k]));
                }
                if let Some(f) = &a.field {
                    if let FieldKind::Seq(name) = &f.kind {
                        let c = seq.entry(name.to_lowercase()).or_insert(0);
                        *c += 1;
                        r.seqs.insert(at, *c);
                    }
                    if let FieldKind::Citation(t) = &f.kind {
                        let n = r.cites.len() + 1;
                        r.cites.entry(t.to_lowercase()).or_insert(n);
                    }
                    if let FieldKind::Authority { long, short, cat } = &f.kind
                        && p.style.toc.is_none()
                    {
                        // A short citation alone stands for its long one.
                        let entry = if long.is_empty() { shorts.get(&crate::fold(short)).cloned() } else { Some((long.clone(), *cat)) };
                        if let Some(e) = entry {
                            if !short.is_empty() && !long.is_empty() {
                                shorts.entry(crate::fold(short)).or_insert_with(|| e.clone());
                            }
                            r.authority_at.entry(crate::fold(&e.0)).or_default().push(at);
                            r.authorities.push((at, e.0, e.1));
                        }
                    }
                    if let FieldKind::IndexEntry(t) = &f.kind
                        && p.style.toc.is_none()
                        && !t.is_empty()
                    {
                        r.index_at.entry(crate::index_key(t)).or_default().push(at);
                        r.index.push((at, t.clone()));
                    }
                }
            }
        }
        if any {
            r.bookmarks = self.bookmarks().into_iter().map(|b| (b.name.to_lowercase(), b)).collect();
        }
        r
    }

    /// Inserts a field at the selection (replacing it). Returns its extent.
    pub fn insert_field(&mut self, sel: Selection, field: Field) -> Selection {
        let a = self.clamp(sel.start());
        let mut attrs = self.typing_attrs(a);
        (attrs.link, attrs.link_tip, attrs.note) = (None, None, None);
        attrs.field = Some(Arc::new(field));
        let end = self.insert_text(sel, &OBJECT.to_string(), Some(attrs));
        if let Some(e) = self.undo.last_mut() {
            e.kind = EditKind::Field;
        }
        Selection { anchor: Pos::new(end.para, end.offset - OBJECT.len_utf8()), cursor: end }
    }

    /// The field whose character is right after `p`.
    pub fn field_at(&self, p: Pos) -> Option<(Selection, Arc<Field>)> {
        let para = self.paragraphs.get(p.para)?;
        if !para.text.get(p.offset..)?.starts_with(OBJECT) {
            return None;
        }
        let f = para.attrs_at(p.offset + OBJECT.len_utf8()).field?;
        Some((Selection { anchor: p, cursor: Pos::new(p.para, p.offset + OBJECT.len_utf8()) }, f))
    }

    /// Every field: position and field, in document order.
    pub fn fields(&self) -> Vec<(Pos, Arc<Field>)> {
        let mut out = Vec::new();
        for (i, p) in self.paragraphs.iter().enumerate() {
            if !crate::para_has_objects(p) {
                continue;
            }
            for (off, a) in crate::objects_of(p) {
                if let Some(f) = &a.field {
                    out.push((Pos::new(i, off), f.clone()));
                }
            }
        }
        out
    }

    /// Stores the shown value of each field as its cached result (before
    /// saving, so files carry current values). Not an undo step: the values
    /// are a display detail.
    pub fn store_field_values(&mut self, values: &HashMap<Pos, String>) {
        let mut changed = false;
        for (&at, v) in values {
            let Some((sel, f)) = self.field_at(at) else { continue };
            if f.cached == *v {
                continue;
            }
            let f = Arc::new(Field { cached: v.clone(), ..(*f).clone() });
            self.paragraphs[at.para].map_attrs(sel.start().offset, sel.end().offset, &|a| a.field = Some(f.clone()));
            changed = true;
        }
        if changed {
            self.revision += 1;
        }
    }

    /// A hidden bookmark ("_Ref…") covering paragraph `para` (or its note
    /// mark at `note`), for a cross-reference to it; an existing one that
    /// covers it is reused. Undoable. Returns the name.
    pub fn reference_bookmark(&mut self, para: usize, note: Option<Pos>, sel: Selection) -> Option<String> {
        let p = self.paragraphs.get(para)?;
        let (s, e) = match note {
            Some(n) => (n.offset, n.offset + OBJECT.len_utf8()),
            None => (0, p.len()),
        };
        for b in self.bookmarks() {
            if b.start == Pos::new(para, s) && b.end == Pos::new(para, e) && (b.name.starts_with("_Ref") || b.name.starts_with("_Toc") || note.is_none() && !b.is_hidden()) {
                return Some(b.name.to_string());
            }
        }
        let name = self.new_hidden_name("_Ref");
        self.add_bookmark(&name, Selection { anchor: Pos::new(para, s), cursor: Pos::new(para, e) });
        // Keep the caret where it was when this step is undone or redone.
        if let Some(u) = self.undo.last_mut() {
            (u.sel_before, u.sel_after) = (sel, sel);
        }
        Some(name)
    }

    /// A new hidden bookmark name like Word's: prefix and nine digits.
    pub fn new_hidden_name(&self, prefix: &str) -> String {
        let mut max = 0u64;
        for p in &self.paragraphs {
            for m in &p.marks {
                if let Some(n) = m.name.strip_prefix(prefix).and_then(|d| d.parse::<u64>().ok()) {
                    max = max.max(n);
                }
            }
        }
        format!("{prefix}{:09}", max.max(100_000_000) + 1)
    }
}

/// The most bytes (about) a REF field shows of its bookmark.
const MAX_REF: usize = 4096;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Paragraph, Selection};

    const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

    fn cx<'a>(page_of: &'a dyn Fn(Pos) -> Option<usize>, label: &'a dyn Fn(Pos) -> Option<String>) -> FieldCtx<'a> {
        FieldCtx { page: 2, pages: 5, now: (2026, 9, 25, 14, 5, 9), date: "25/09/2026", time: "14:05", months: &MONTHS, days: &DAYS, file: "a.docx", page_of, page_label: &|p| p.to_string(), above: "above", below: "below", on_page: "on page {n}", note_label: label, decimal: '.' }
    }

    #[test]
    fn parse_and_write() {
        let f = Field::parse(" PAGE   \\* MERGEFORMAT ", "3").unwrap();
        assert_eq!((f.kind.clone(), f.cached.as_str()), (FieldKind::Page, "3"));
        let f = Field::parse("PAGEREF _Toc123 \\h", "").unwrap();
        assert_eq!(f.kind, FieldKind::PageRef("_Toc123".into()));
        assert!(f.hyperlink);
        let f = Field::parse(r#"DATE \@ "d 'de' MMMM 'de' yyyy""#, "").unwrap();
        assert_eq!(f.format.as_deref(), Some("d 'de' MMMM 'de' yyyy"));
        assert_eq!(Field::parse(&f.instr(), "").unwrap(), f);
        assert!(Field::parse("MERGEFIELD Name", "").is_none());
        assert!(Field::parse("TOC \\o", "").is_none());
        assert_eq!(Field::parse(" SEQ Figure \\* ARABIC ", "1").unwrap().kind, FieldKind::Seq("Figure".into()));
        // A reference without a name (REF "") keeps its empty name.
        let f = Field::parse(r#" REF "" "#, "").unwrap();
        assert_eq!(f.kind, FieldKind::Ref(String::new()));
        assert_eq!(Field::parse(&f.instr(), "").unwrap(), f);
    }

    #[test]
    fn dates() {
        let t = (2026, 9, 25, 14, 5, 9);
        assert_eq!(format_date("dd/MM/yyyy", t, &MONTHS, &DAYS), "25/09/2026");
        assert_eq!(format_date("dddd, MMMM d, yyyy", t, &MONTHS, &DAYS), "Friday, September 25, 2026");
        assert_eq!(format_date("d 'de' MMM 'de' yy", t, &MONTHS, &DAYS), "25 de Sep de 26");
        assert_eq!(format_date("h:mm AM/PM", t, &MONTHS, &DAYS), "2:05 PM");
        assert_eq!(format_date("HH:mm:ss", t, &MONTHS, &DAYS), "14:05:09");
        assert_eq!(weekday(2000, 1, 1), 6);
    }

    #[test]
    fn values() {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("Intro"), Paragraph::new("Figure  shows it"), Paragraph::new("See ")]);
        d.meta.author = "Ana".into();
        d.insert_field(Selection::caret(Pos::new(1, 7)), Field::new(FieldKind::Seq("Figure".into())));
        let name = d.reference_bookmark(0, None, Selection::default()).unwrap();
        assert!(name.starts_with("_Ref"));
        assert_eq!(d.reference_bookmark(0, None, Selection::default()).unwrap(), name);
        let end = d.paragraphs[2].len();
        d.insert_field(Selection::caret(Pos::new(2, end)), Field::new(FieldKind::Ref(name.clone())));
        d.insert_field(Selection::caret(Pos::new(2, 0)), Field { position: true, ..Field::new(FieldKind::Ref(name.clone())) });
        d.insert_field(Selection::caret(Pos::new(0, 0)), Field::new(FieldKind::Author));
        let r = d.refs();
        let page_of = |_p: Pos| Some(1);
        let label = |_p: Pos| None;
        let c = cx(&page_of, &label);
        let fields = d.fields();
        let vals: Vec<String> = fields.iter().map(|(p, f)| r.value(&d, f, *p, &c)).collect();
        assert_eq!(vals, ["Ana", "1", "above", "Intro"]);
        let f = Field::new(FieldKind::PageRef(name));
        assert_eq!(r.value(&d, &Field { position: true, ..f.clone() }, Pos::new(2, 0), &c), "on page 1");
        assert_eq!(r.value(&d, &Field::new(FieldKind::PageRef("nope".into())), Pos::new(2, 0), &c), "");
        // Stored values go in the file.
        let mut map = HashMap::new();
        map.insert(fields[0].0, "Ana".to_string());
        d.store_field_values(&map);
        assert_eq!(d.field_at(fields[0].0).unwrap().1.cached, "Ana");
        // Document properties, built-in or custom, by name.
        d.meta.company = "ACME".into();
        d.meta.custom.push(crate::CustomProp { name: "Contrato".into(), value: crate::PropValue::Number(42.0) });
        let f = Field::parse(r#" DOCPROPERTY  "Company"  \* MERGEFORMAT "#, "old").unwrap();
        assert_eq!(Field::parse(&f.instr(), "old").unwrap(), f);
        assert_eq!(r.value(&d, &f, Pos::new(0, 0), &c), "ACME");
        assert_eq!(r.value(&d, &Field::new(FieldKind::DocProperty("contrato".into())), Pos::new(0, 0), &c), "42");
        assert_eq!(r.value(&d, &Field { cached: "kept".into(), ..Field::new(FieldKind::DocProperty("Nope".into())) }, Pos::new(0, 0), &c), "kept");
    }
}
