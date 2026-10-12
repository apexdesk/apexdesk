//! Citations and bibliographies: the document's sources, how citations
//! (CITATION fields) show them, and the entries of a bibliography.

use std::collections::HashMap;

use crate::{Document, FieldKind};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SourceKind {
    #[default]
    Book,
    Article,
    Web,
    Report,
    Other,
}

impl SourceKind {
    pub const ALL: [SourceKind; 5] = [SourceKind::Book, SourceKind::Article, SourceKind::Web, SourceKind::Report, SourceKind::Other];

    /// Its name in files.
    pub fn key(self) -> &'static str {
        match self {
            SourceKind::Book => "book",
            SourceKind::Article => "article",
            SourceKind::Web => "web",
            SourceKind::Report => "report",
            SourceKind::Other => "other",
        }
    }

    pub fn from_key(s: &str) -> SourceKind {
        SourceKind::ALL.into_iter().find(|k| k.key() == s).unwrap_or_default()
    }
}

/// A source that citations refer to by its tag.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Source {
    pub tag: String,
    pub kind: SourceKind,
    /// People as "Last, First", separated by ";" (or one organization).
    pub authors: String,
    pub title: String,
    pub year: String,
    pub city: String,
    pub publisher: String,
    /// Journal (articles) or site name (web pages).
    pub journal: String,
    pub volume: String,
    pub pages: String,
    pub url: String,
}

/// How citations and bibliographies are written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CiteStyle {
    #[default]
    Apa,
    Mla,
    /// Numbered in the order of first citation.
    Ieee,
    /// Chicago author-date: "(Doe and Roe 2020)".
    ChicagoAuthorDate,
    /// Chicago notes and bibliography: citations go in footnotes, in the
    /// note form "John Doe, *Title* (City: Publisher, 2020)."
    ChicagoNotes,
}

impl CiteStyle {
    pub const ALL: [CiteStyle; 5] = [CiteStyle::Apa, CiteStyle::Mla, CiteStyle::Ieee, CiteStyle::ChicagoAuthorDate, CiteStyle::ChicagoNotes];

    pub fn name(self) -> &'static str {
        match self {
            CiteStyle::Apa => "APA",
            CiteStyle::Mla => "MLA",
            CiteStyle::Ieee => "IEEE",
            CiteStyle::ChicagoAuthorDate => "Chicago (author-date)",
            CiteStyle::ChicagoNotes => "Chicago (notes)",
        }
    }

    /// Citations go in footnotes.
    pub fn in_notes(self) -> bool {
        self == CiteStyle::ChicagoNotes
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Bibliography {
    pub sources: Vec<Source>,
    pub style: CiteStyle,
}

impl Bibliography {
    pub fn source(&self, tag: &str) -> Option<&Source> {
        self.sources.iter().find(|s| s.tag.eq_ignore_ascii_case(tag))
    }

    /// A tag not used yet, from the first author and the year ("Doe20").
    pub fn new_tag(&self, s: &Source) -> String {
        let last: String = s.people().first().map_or("Src".into(), |p| p.0.clone()).chars().filter(|c| c.is_alphanumeric()).take(8).collect();
        let yy: String = s.year.chars().filter(char::is_ascii_digit).collect();
        let base = format!("{}{}", if last.is_empty() { "Src" } else { &last }, &yy[yy.len().saturating_sub(2)..]);
        let mut tag = base.clone();
        let mut n = 0;
        while self.source(&tag).is_some() {
            tag = format!("{base}{}", (b'a' + (n % 26) as u8) as char);
            n += 1;
        }
        tag
    }
}

/// A piece of formatted text: italic or not.
pub type Piece = (String, bool);

impl Source {
    /// The authors as (last, first) names; an organization has no first name.
    pub fn people(&self) -> Vec<(String, String)> {
        self.authors
            .split(';')
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .map(|a| match a.split_once(',') {
                Some((l, f)) => (l.trim().to_string(), f.trim().to_string()),
                None => (a.to_string(), String::new()),
            })
            .collect()
    }

    /// The authors in a citation: "Doe", "Doe & Roe", "Doe et al."
    fn cite_names(&self, and: &str) -> String {
        let p = self.people();
        match p.len() {
            0 => String::new(),
            1 => p[0].0.clone(),
            2 => format!("{} {and} {}", p[0].0, p[1].0),
            _ => format!("{} et al.", p[0].0),
        }
    }

    fn year_or_nd(&self) -> String {
        if self.year.trim().is_empty() { "n.d.".into() } else { self.year.trim().to_string() }
    }

    /// The citation as shown in the text; `number` for numbered styles.
    pub fn citation(&self, style: CiteStyle, number: usize) -> String {
        let who = |and: &str| {
            let n = self.cite_names(and);
            if n.is_empty() { self.title.clone() } else { n }
        };
        match style {
            CiteStyle::Apa => format!("({}, {})", who("&"), self.year_or_nd()),
            CiteStyle::Mla => format!("({})", who("and")),
            CiteStyle::Ieee => format!("[{number}]"),
            CiteStyle::ChicagoAuthorDate => format!("({} {})", who("and"), self.year_or_nd()),
            CiteStyle::ChicagoNotes => self.note_form().into_iter().map(|p| p.0).collect(),
        }
    }

    fn place(&self) -> String {
        match (self.city.trim(), self.publisher.trim()) {
            ("", "") => String::new(),
            (c, "") => c.to_string(),
            ("", p) => p.to_string(),
            (c, p) => format!("{c}: {p}"),
        }
    }

    /// The note form of Chicago notes and bibliography: "John Doe and Ann
    /// Roe, *Title* (City: Publisher, 2020)."
    pub fn note_form(&self) -> Vec<Piece> {
        let mut out: Vec<Piece> = Vec::new();
        let mut t = |s: &str, it: bool| {
            if !s.is_empty() {
                out.push((s.to_string(), it));
            }
        };
        let names: Vec<String> = self.people().iter().map(|(l, f)| if f.is_empty() { l.clone() } else { format!("{f} {l}") }).collect();
        let names = match names.len() {
            0 => String::new(),
            1 => names[0].clone(),
            2 => format!("{} and {}", names[0], names[1]),
            3 => format!("{}, {}, and {}", names[0], names[1], names[2]),
            _ => format!("{} et al.", names[0]),
        };
        if !names.is_empty() {
            t(&format!("{names}, "), false);
        }
        let place = self.place();
        match self.kind {
            SourceKind::Article => {
                t(&format!("\u{201C}{},\u{201D} ", self.title), false);
                t(&self.journal, true);
                t(format!(" {}", self.volume).trim_end(), false);
                t(&format!(" ({})", self.year_or_nd()), false);
                if !self.pages.is_empty() {
                    t(&format!(": {}", self.pages), false);
                }
                t(".", false);
            }
            SourceKind::Web => {
                t(&format!("\u{201C}{},\u{201D} ", self.title), false);
                if !self.journal.is_empty() {
                    t(&format!("{}, ", self.journal), false);
                }
                t(&self.year_or_nd(), false);
                if !self.url.is_empty() {
                    t(&format!(", {}", self.url), false);
                }
                t(".", false);
            }
            _ => {
                t(&self.title, true);
                t(&format!(" ({}{}).", if place.is_empty() { String::new() } else { format!("{place}, ") }, self.year_or_nd()), false);
            }
        }
        out
    }

    /// The bibliography entry.
    pub fn entry(&self, style: CiteStyle, number: usize) -> Vec<Piece> {
        let mut out: Vec<Piece> = Vec::new();
        let mut t = |s: &str, it: bool| {
            if !s.is_empty() {
                out.push((s.to_string(), it));
            }
        };
        let people = self.people();
        let initials = |f: &str| f.split_whitespace().map(|w| format!("{}.", w.chars().next().unwrap_or(' '))).collect::<Vec<_>>().join(" ");
        let join = |v: Vec<String>, and: &str| match v.len() {
            0 => String::new(),
            1 => v[0].clone(),
            2 if style == CiteStyle::Ieee => format!("{} {and} {}", v[0], v[1]),
            n => format!("{}, {and} {}", v[..n - 1].join(", "), v[n - 1]),
        };
        let place = match (self.city.trim(), self.publisher.trim()) {
            ("", "") => String::new(),
            (c, "") => c.to_string(),
            ("", p) => p.to_string(),
            (c, p) => format!("{c}: {p}"),
        };
        let dot = |s: &str| if s.ends_with(['.', '?', '!']) { s.to_string() } else { format!("{s}.") };
        match style {
            CiteStyle::Apa => {
                let names = join(people.iter().map(|(l, f)| if f.is_empty() { l.clone() } else { format!("{l}, {}", initials(f)) }).collect(), "&");
                if names.is_empty() {
                    t(&format!("{} ", dot(&self.title)), true);
                } else {
                    t(&format!("{} ", dot(&names)), false);
                }
                t(&format!("({}). ", self.year_or_nd()), false);
                if !names.is_empty() {
                    match self.kind {
                        SourceKind::Article => t(&format!("{} ", dot(&self.title)), false),
                        _ => {
                            t(&self.title, true);
                            t(". ", false);
                        }
                    }
                }
                match self.kind {
                    SourceKind::Article => {
                        t(&self.journal, true);
                        if !self.volume.is_empty() {
                            t(", ", false);
                            t(&self.volume, true);
                        }
                        if !self.pages.is_empty() {
                            t(&format!(", {}", self.pages), false);
                        }
                        t(".", false);
                    }
                    SourceKind::Web => {
                        if !self.url.is_empty() {
                            t(&format!("Retrieved from {}", self.url), false);
                        }
                    }
                    _ => t(&dot(&place), false),
                }
            }
            CiteStyle::Mla => {
                let names: Vec<String> = people.iter().enumerate().map(|(i, (l, f))| if f.is_empty() { l.clone() } else if i == 0 { format!("{l}, {f}") } else { format!("{f} {l}") }).collect();
                let names = if names.len() > 2 { format!("{}, et al", names[0]) } else { join(names, "and") };
                if !names.is_empty() {
                    t(&format!("{} ", dot(&names)), false);
                }
                match self.kind {
                    SourceKind::Article => {
                        t(&format!("\u{201C}{}\u{201D} ", dot(&self.title)), false);
                        t(&self.journal, true);
                        t(&format!(" {}", self.volume).trim_end().to_string(), false);
                        t(&format!(" ({})", self.year_or_nd()), false);
                        if !self.pages.is_empty() {
                            t(&format!(": {}", self.pages), false);
                        }
                        t(".", false);
                    }
                    SourceKind::Web => {
                        t(&self.title, true);
                        t(". ", false);
                        if !self.journal.is_empty() {
                            t(&format!("{}, ", self.journal), false);
                        }
                        t(&format!("{}.", self.year_or_nd()), false);
                        if !self.url.is_empty() {
                            t(&format!(" <{}>.", self.url), false);
                        }
                    }
                    _ => {
                        t(&self.title, true);
                        t(". ", false);
                        t(&format!("{}{}.", if place.is_empty() { String::new() } else { format!("{place}, ") }, self.year_or_nd()), false);
                    }
                }
            }
            CiteStyle::ChicagoAuthorDate | CiteStyle::ChicagoNotes => {
                // "Doe, John Paul, and Ann Roe." with the year after the
                // names (author-date) or near the end (notes).
                let author_date = style == CiteStyle::ChicagoAuthorDate;
                let names: Vec<String> = people.iter().enumerate().map(|(i, (l, f))| if f.is_empty() { l.clone() } else if i == 0 { format!("{l}, {f}") } else { format!("{f} {l}") }).collect();
                let names = if names.len() > 10 { format!("{}, et al", names[..7].join(", ")) } else { join(names, "and") };
                if !names.is_empty() {
                    t(&format!("{} ", dot(&names)), false);
                }
                if author_date {
                    t(&format!("{}. ", self.year_or_nd()), false);
                }
                match self.kind {
                    SourceKind::Article => {
                        t(&format!("\u{201C}{}\u{201D} ", dot(&self.title)), false);
                        t(&self.journal, true);
                        t(format!(" {}", self.volume).trim_end(), false);
                        if !author_date {
                            t(&format!(" ({})", self.year_or_nd()), false);
                        }
                        if !self.pages.is_empty() {
                            t(&format!(": {}", self.pages), false);
                        }
                        t(".", false);
                    }
                    SourceKind::Web => {
                        t(&format!("\u{201C}{}\u{201D} ", dot(&self.title)), false);
                        if !self.journal.is_empty() {
                            t(&format!("{}. ", self.journal), false);
                        }
                        if !author_date {
                            t(&format!("{}. ", self.year_or_nd()), false);
                        }
                        if !self.url.is_empty() {
                            t(&format!("{}.", self.url), false);
                        }
                    }
                    _ => {
                        t(&self.title, true);
                        t(". ", false);
                        if author_date {
                            t(&dot(&place), false);
                        } else {
                            t(&format!("{}{}.", if place.is_empty() { String::new() } else { format!("{place}, ") }, self.year_or_nd()), false);
                        }
                    }
                }
            }
            CiteStyle::Ieee => {
                t(&format!("[{number}] "), false);
                let names = join(people.iter().map(|(l, f)| if f.is_empty() { l.clone() } else { format!("{} {l}", initials(f)) }).collect(), "and");
                if !names.is_empty() {
                    t(&format!("{names}, "), false);
                }
                match self.kind {
                    SourceKind::Article => {
                        t(&format!("\u{201C}{},\u{201D} ", self.title), false);
                        t(&self.journal, true);
                        if !self.volume.is_empty() {
                            t(&format!(", vol. {}", self.volume), false);
                        }
                        if !self.pages.is_empty() {
                            t(&format!(", pp. {}", self.pages), false);
                        }
                        t(&format!(", {}.", self.year_or_nd()), false);
                    }
                    SourceKind::Web => {
                        t(&format!("\u{201C}{},\u{201D} {}. ", self.title, self.year_or_nd()), false);
                        if !self.url.is_empty() {
                            t(&format!("[Online]. Available: {}", self.url), false);
                        }
                    }
                    _ => {
                        t(&self.title, true);
                        t(". ", false);
                        t(&format!("{}{}.", if place.is_empty() { String::new() } else { format!("{place}, ") }, self.year_or_nd()), false);
                    }
                }
            }
        }
        out
    }
}

impl Document {
    /// The number of each cited source (by lower case tag), in the order of
    /// first citation.
    pub fn cite_numbers(&self) -> HashMap<String, usize> {
        let mut out = HashMap::new();
        for (_, f) in self.fields() {
            if let FieldKind::Citation(tag) = &f.kind {
                let n = out.len() + 1;
                out.entry(tag.to_lowercase()).or_insert(n);
            }
        }
        out
    }

    /// Sources in bibliography order: cited ones by number (numbered
    /// styles), all by author and title otherwise.
    pub fn bibliography_order(&self) -> Vec<(&Source, usize)> {
        let nums = self.cite_numbers();
        let b = &self.biblio;
        let mut v: Vec<(&Source, usize)> = b.sources.iter().map(|s| (s, nums.get(&s.tag.to_lowercase()).copied().unwrap_or(0))).collect();
        if b.style == CiteStyle::Ieee {
            v.retain(|x| x.1 > 0);
            v.sort_by_key(|x| x.1);
        } else {
            let key = |s: &Source| crate::fold(&format!("{} {} {}", s.authors, s.title, s.year));
            v.sort_by_key(|x| key(x.0));
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> Source {
        Source { tag: "Doe20".into(), kind: SourceKind::Book, authors: "Doe, John Paul; Roe, Ann".into(), title: "Rust Things".into(), year: "2020".into(), city: "Madrid".into(), publisher: "Pub".into(), ..Default::default() }
    }

    fn text(p: &[Piece]) -> String {
        p.iter().map(|(t, it)| if *it { format!("*{t}*") } else { t.clone() }).collect()
    }

    #[test]
    fn formats() {
        let b = book();
        assert_eq!(b.citation(CiteStyle::Apa, 1), "(Doe & Roe, 2020)");
        assert_eq!(b.citation(CiteStyle::Mla, 1), "(Doe and Roe)");
        assert_eq!(b.citation(CiteStyle::Ieee, 3), "[3]");
        assert_eq!(text(&b.entry(CiteStyle::Apa, 1)), "Doe, J. P., & Roe, A. (2020). *Rust Things*. Madrid: Pub.");
        assert_eq!(text(&b.entry(CiteStyle::Mla, 1)), "Doe, John Paul, and Ann Roe. *Rust Things*. Madrid: Pub, 2020.");
        assert_eq!(text(&b.entry(CiteStyle::Ieee, 2)), "[2] J. P. Doe and A. Roe, *Rust Things*. Madrid: Pub, 2020.");
        let a = Source { kind: SourceKind::Article, authors: "Smith, Al".into(), title: "On X".into(), year: "1999".into(), journal: "J. Stuff".into(), volume: "4".into(), pages: "1-9".into(), ..Default::default() };
        assert_eq!(text(&a.entry(CiteStyle::Apa, 1)), "Smith, A. (1999). On X. *J. Stuff*, *4*, 1-9.");
        let mut bib = Bibliography { sources: vec![b.clone()], style: CiteStyle::Apa };
        assert_eq!(bib.new_tag(&b), "Doe20a");
        bib.sources.clear();
        assert_eq!(bib.new_tag(&b), "Doe20");
    }

    #[test]
    fn chicago() {
        let b = book();
        assert_eq!(b.citation(CiteStyle::ChicagoAuthorDate, 1), "(Doe and Roe 2020)");
        assert_eq!(text(&b.entry(CiteStyle::ChicagoAuthorDate, 1)), "Doe, John Paul, and Ann Roe. 2020. *Rust Things*. Madrid: Pub.");
        assert_eq!(text(&b.entry(CiteStyle::ChicagoNotes, 1)), "Doe, John Paul, and Ann Roe. *Rust Things*. Madrid: Pub, 2020.");
        assert_eq!(text(&b.note_form()), "John Paul Doe and Ann Roe, *Rust Things* (Madrid: Pub, 2020).");
        assert_eq!(b.citation(CiteStyle::ChicagoNotes, 1), "John Paul Doe and Ann Roe, Rust Things (Madrid: Pub, 2020).");
        let a = Source { kind: SourceKind::Article, authors: "Smith, Al".into(), title: "On X".into(), year: "1999".into(), journal: "J. Stuff".into(), volume: "4".into(), pages: "1-9".into(), ..Default::default() };
        assert_eq!(text(&a.entry(CiteStyle::ChicagoAuthorDate, 1)), "Smith, Al. 1999. \u{201C}On X.\u{201D} *J. Stuff* 4: 1-9.");
        assert_eq!(text(&a.entry(CiteStyle::ChicagoNotes, 1)), "Smith, Al. \u{201C}On X.\u{201D} *J. Stuff* 4 (1999): 1-9.");
    }
}
