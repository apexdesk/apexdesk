//! Differences between two digests (the original and the file read back).

use std::collections::{BTreeMap, BTreeSet};

use super::model::Digest;
use super::result::Diff;

/// Examples kept per difference.
const EXAMPLES: usize = 3;

fn short(s: &str) -> String {
    let s = s.replace('\n', "⏎");
    if s.chars().count() > 70 { format!("{}…", s.chars().take(70).collect::<String>()) } else if s.is_empty() { "∅".into() } else { s }
}

/// Every kind that differs, with how many items and the first examples.
pub fn diff(a: &Digest, b: &Digest) -> Vec<Diff> {
    let mut out = Vec::new();
    let empty = Vec::new();
    let kinds: BTreeSet<&str> = a.lists.keys().chain(b.lists.keys()).copied().collect();
    for kind in kinds {
        let (x, y) = (a.lists.get(kind).unwrap_or(&empty), b.lists.get(kind).unwrap_or(&empty));
        if let Some(d) = list_diff(kind, x, y) {
            out.push(d);
        }
    }
    out
}

/// Differences item by item: how many per kind and the first examples.
#[derive(Default)]
pub struct Tally {
    kinds: BTreeMap<&'static str, (u64, Vec<String>)>,
}

impl Tally {
    /// Counts `p` → `q` for the item `key` names when they differ.
    pub fn add(&mut self, kind: &'static str, key: impl FnOnce() -> String, p: &str, q: &str) {
        if p == q {
            return;
        }
        let e = self.kinds.entry(kind).or_default();
        e.0 += 1;
        if e.1.len() < EXAMPLES {
            e.1.push(format!("{}: {} → {}", key(), short(p), short(q)));
        }
    }

    pub fn diffs(self) -> Vec<Diff> {
        self.kinds.into_iter().map(|(kind, (count, ex))| Diff { kind: kind.to_string(), count, detail: ex.join("; ") }).collect()
    }
}

/// Sequences: the part between the common head and tail differs.
fn list_diff(kind: &str, x: &[String], y: &[String]) -> Option<Diff> {
    let head = x.iter().zip(y).take_while(|(p, q)| p == q).count();
    let tail = x[head..].iter().rev().zip(y[head..].iter().rev()).take_while(|(p, q)| p == q).count();
    let (xm, ym) = (&x[head..x.len() - tail], &y[head..y.len() - tail]);
    if xm.is_empty() && ym.is_empty() {
        return None;
    }
    let mut examples = Vec::new();
    if x.len() != y.len() {
        examples.push(format!("{} → {} items", x.len(), y.len()));
    }
    // Items that only differ in place count once.
    let changed = xm.iter().zip(ym).filter(|(p, q)| p != q).count() + xm.len().abs_diff(ym.len());
    for i in 0..xm.len().max(ym.len()).min(EXAMPLES) {
        let (p, q) = (xm.get(i).map(String::as_str).unwrap_or(""), ym.get(i).map(String::as_str).unwrap_or(""));
        if p != q {
            examples.push(format!("#{}: {} → {}", head + i + 1, short(p), short(q)));
        }
    }
    Some(Diff { kind: kind.to_string(), count: changed.max(1) as u64, detail: examples.join("; ") })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(text: &[&str]) -> Digest {
        let mut d = Digest::default();
        d.lists.insert("text", text.iter().map(|s| s.to_string()).collect());
        d
    }

    #[test]
    fn lists_report_the_middle() {
        assert!(diff(&digest(&["a", "b"]), &digest(&["a", "b"])).is_empty());
        let d = diff(&digest(&["a", "b", "c"]), &digest(&["a", "x", "c"]));
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].count, 1);
        assert_eq!(d[0].detail, "#2: b → x");
        let d = diff(&digest(&["a", "b", "c"]), &digest(&["a", "c"]));
        assert_eq!(d[0].detail, "3 → 2 items; #2: b → ∅");
    }

    #[test]
    fn tally_counts_and_keeps_examples() {
        let mut t = Tally::default();
        t.add("cells.value", || "S!A1".into(), "1", "1");
        for i in 0..5 {
            t.add("cells.value", || format!("S!A{i}"), "x", "");
        }
        let d = t.diffs();
        assert_eq!(d[0].count, 5);
        assert_eq!(d[0].detail, "S!A0: x → ∅; S!A1: x → ∅; S!A2: x → ∅");
    }
}
