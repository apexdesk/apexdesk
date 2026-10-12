//! The tables of the data model as columns of values, and the active
//! relationships as lookups from each row of the "many" table to its row
//! of the "one" table.

use std::collections::VecDeque;
use std::sync::Arc;

use crate::value::Value;
use crate::workbook::HashMap;

use super::Relationship;

#[derive(Clone, Debug, Default)]
pub struct TableData {
    pub name: String,
    pub columns: Vec<String>,
    /// Column by column, `rows` values each.
    pub cols: Vec<Vec<Value>>,
    pub rows: usize,
    /// Date formats of columns holding dates (kept in pivot labels).
    pub formats: Vec<Option<Arc<str>>>,
}

/// An active relationship, resolved.
#[derive(Clone, Debug)]
pub struct Link {
    pub from: usize,
    pub from_col: usize,
    pub to: usize,
    pub to_col: usize,
    /// The related row of `to` for each row of `from`.
    pub map: Vec<Option<u32>>,
}

#[derive(Clone, Debug, Default)]
pub struct ModelData {
    pub tables: Vec<TableData>,
    pub links: Vec<Link>,
}

/// The identity of a key value (text ignores case); blanks match nothing.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Key {
    Num(u64),
    Text(String),
    Bool(bool),
}

pub(crate) fn key(v: &Value) -> Option<Key> {
    match v {
        Value::Number(n) => Some(Key::Num(if *n == 0.0 { 0 } else { n.to_bits() })),
        Value::Text(t) if t.is_empty() => None,
        Value::Text(t) => Some(Key::Text(t.to_lowercase())),
        Value::Bool(b) => Some(Key::Bool(*b)),
        _ => None,
    }
}

impl ModelData {
    /// Resolves the active relationships whose tables and columns exist.
    pub fn new(tables: Vec<TableData>, rels: &[Relationship]) -> ModelData {
        let mut d = ModelData { tables, links: Vec::new() };
        for r in rels.iter().filter(|r| r.active) {
            let (Some(from), Some(to)) = (d.table(&r.from_table), d.table(&r.to_table)) else { continue };
            let (Some(fc), Some(tc)) = (d.column(from, &r.from_column), d.column(to, &r.to_column)) else { continue };
            if from == to || d.links.iter().any(|l| l.from == from && l.to == to) {
                continue;
            }
            let mut index: HashMap<Key, u32> = HashMap::default();
            for (i, v) in d.tables[to].cols[tc].iter().enumerate() {
                if let Some(k) = key(v) {
                    index.entry(k).or_insert(i as u32);
                }
            }
            let map = d.tables[from].cols[fc].iter().map(|v| key(v).and_then(|k| index.get(&k).copied())).collect();
            d.links.push(Link { from, from_col: fc, to, to_col: tc, map });
        }
        d
    }

    pub fn table(&self, name: &str) -> Option<usize> {
        let name = name.trim();
        self.tables.iter().position(|t| t.name.eq_ignore_ascii_case(name)).or_else(|| {
            let low = name.to_lowercase();
            self.tables.iter().position(|t| t.name.to_lowercase() == low)
        })
    }

    pub fn column(&self, t: usize, name: &str) -> Option<usize> {
        let low = name.trim().to_lowercase();
        self.tables.get(t)?.columns.iter().position(|c| c.trim().to_lowercase() == low)
    }

    pub fn rows(&self, t: usize) -> usize {
        self.tables.get(t).map_or(0, |x| x.rows)
    }

    pub fn value(&self, t: usize, c: usize, r: usize) -> &Value {
        static EMPTY: Value = Value::Empty;
        self.tables.get(t).and_then(|x| x.cols.get(c)).and_then(|col| col.get(r)).unwrap_or(&EMPTY)
    }

    /// No two rows share a (non-blank) value of the column.
    pub fn unique(&self, t: usize, c: usize) -> bool {
        let Some(col) = self.tables.get(t).and_then(|x| x.cols.get(c)) else { return false };
        let mut seen = std::collections::HashSet::new();
        col.iter().filter_map(key).all(|k| seen.insert(k))
    }

    /// Nearly every value of `t.c` is a value of `u.uc` (a few orphan keys
    /// are common in real data).
    pub fn covers(&self, t: usize, c: usize, u: usize, uc: usize) -> bool {
        let keys: std::collections::HashSet<Key> = self.tables[u].cols[uc].iter().filter_map(key).collect();
        let (mut n, mut hit) = (0usize, 0usize);
        for k in self.tables[t].cols[c].iter().filter_map(key) {
            n += 1;
            hit += keys.contains(&k) as usize;
        }
        hit > 0 && hit * 20 >= n * 19
    }

    /// The links leading from table `from` to table `to` ("many" to "one"
    /// all the way), shortest first; None when `to` isn't reachable.
    pub fn path(&self, from: usize, to: usize) -> Option<Vec<usize>> {
        if from == to {
            return Some(Vec::new());
        }
        let mut prev: Vec<Option<usize>> = vec![None; self.tables.len()];
        let mut seen = vec![false; self.tables.len()];
        seen[from] = true;
        let mut q = VecDeque::from([from]);
        while let Some(t) = q.pop_front() {
            for (li, l) in self.links.iter().enumerate() {
                if l.from == t && !seen[l.to] {
                    seen[l.to] = true;
                    prev[l.to] = Some(li);
                    if l.to == to {
                        let mut out = Vec::new();
                        let mut at = to;
                        while let Some(li) = prev[at] {
                            out.push(li);
                            at = self.links[li].from;
                        }
                        out.reverse();
                        return Some(out);
                    }
                    q.push_back(l.to);
                }
            }
        }
        None
    }

    /// Tables reachable from `from` (itself included).
    pub fn reachable(&self, from: usize) -> Vec<usize> {
        (0..self.tables.len()).filter(|t| self.path(from, *t).is_some()).collect()
    }

    /// The row of `to` that row `row` of `from` relates to.
    pub fn related_row(&self, from: usize, row: usize, to: usize) -> Option<usize> {
        let path = self.path(from, to)?;
        path.iter().try_fold(row, |r, li| self.links[*li].map.get(r).copied().flatten().map(|x| x as usize))
    }

    /// The values of column `c` of table `to` for each row of `from`
    /// (blank where no row relates).
    pub fn related_column(&self, from: usize, to: usize, c: usize) -> Option<Vec<Value>> {
        let path = self.path(from, to)?;
        let n = self.rows(from);
        let col = &self.tables[to].cols[c];
        Some(
            (0..n)
                .map(|r| {
                    let row = path.iter().try_fold(r, |r, li| self.links[*li].map.get(r).copied().flatten().map(|x| x as usize));
                    row.and_then(|x| col.get(x).cloned()).unwrap_or(Value::Empty)
                })
                .collect(),
        )
    }
}
