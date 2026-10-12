//! The text the database tab stores its objects in (saved queries, forms,
//! reports, the relations diagram, linked tables): simple `key=value`
//! lines, with `[section]` lines starting each repeated part (a control of
//! a form, a field of a report). Values escape `\`, new lines and tabs.
//!
//! Unknown keys are kept by the readers that care (see [`Record::extra`]),
//! so a newer version's objects survive a round trip.

/// One part of an object: its keys in order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Record {
    /// The section name (empty for the object's own keys).
    pub section: String,
    pub pairs: Vec<(String, String)>,
}

impl Record {
    pub fn new(section: &str) -> Record {
        Record { section: section.to_string(), pairs: Vec::new() }
    }

    pub fn get(&self, key: &str) -> &str {
        self.pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()).unwrap_or("")
    }

    pub fn num(&self, key: &str, default: f32) -> f32 {
        self.get(key).parse().unwrap_or(default)
    }

    pub fn int(&self, key: &str, default: i64) -> i64 {
        self.get(key).parse().unwrap_or(default)
    }

    pub fn flag(&self, key: &str) -> bool {
        self.get(key) == "1"
    }

    pub fn set(&mut self, key: &str, value: impl ToString) -> &mut Record {
        self.pairs.push((key.to_string(), value.to_string()));
        self
    }

    pub fn set_flag(&mut self, key: &str, on: bool) -> &mut Record {
        self.set(key, if on { "1" } else { "0" })
    }

    /// Keys not in `known`, to write back unchanged.
    pub fn extra(&self, known: &[&str]) -> Vec<(String, String)> {
        self.pairs.iter().filter(|(k, _)| !known.contains(&k.as_str())).cloned().collect()
    }
}

fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c => o.push(c),
        }
    }
    o
}

fn unescape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            o.push(c);
            continue;
        }
        match it.next() {
            Some('n') => o.push('\n'),
            Some('r') => o.push('\r'),
            Some('t') => o.push('\t'),
            Some(c) => o.push(c),
            None => {}
        }
    }
    o
}

/// The text of records (the first is the object's own, without a section
/// line).
pub fn write(records: &[Record]) -> String {
    let mut out = String::new();
    for (i, r) in records.iter().enumerate() {
        if i > 0 || !r.section.is_empty() {
            out.push('[');
            out.push_str(&r.section);
            out.push_str("]\n");
        }
        for (k, v) in &r.pairs {
            out.push_str(k);
            out.push('=');
            out.push_str(&escape(v));
            out.push('\n');
        }
    }
    out
}

/// Records from text: the object's own first (possibly empty).
pub fn read(text: &str) -> Vec<Record> {
    let mut out = vec![Record::default()];
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(sec) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            out.push(Record::new(sec));
        } else if let Some((k, v)) = line.split_once('=') {
            out.last_mut().unwrap().pairs.push((k.trim().to_string(), unescape(v)));
        }
    }
    out
}

/// The kinds of objects stored in a database file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Query,
    Form,
    Report,
    /// Places of the tables on the relations diagram, and relations that
    /// are drawn but not enforced.
    Layout,
    /// A table of a database server shown as a table of this file.
    Link,
    /// Labels to print from a table or query.
    Labels,
}

impl Kind {
    pub const ALL: [Kind; 6] = [Kind::Query, Kind::Form, Kind::Report, Kind::Layout, Kind::Link, Kind::Labels];

    pub fn key(self) -> &'static str {
        match self {
            Kind::Query => "query",
            Kind::Form => "form",
            Kind::Report => "report",
            Kind::Layout => "layout",
            Kind::Link => "link",
            Kind::Labels => "labels",
        }
    }

    pub fn from_key(k: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|x| x.key() == k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut a = Record::default();
        a.set("name", "Orders\nby month").set("sql", "SELECT \\ 1");
        let mut b = Record::new("field");
        b.set("x", 12.5).set_flag("show", true);
        let text = write(&[a.clone(), b.clone()]);
        let back = read(&text);
        assert_eq!(back, vec![a, b]);
        assert_eq!(back[1].num("x", 0.0), 12.5);
        assert!(back[1].flag("show"));
        assert_eq!(back[1].extra(&["x"]), vec![("show".to_string(), "1".to_string())]);
    }
}
