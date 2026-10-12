//! MATCH of many keys at once (`MATCH(B:B, B:B, 0)`, a common way to
//! count distinct values with FREQUENCY): one pass over the lookup range
//! builds an index, instead of a search of the range for every key.

use std::collections::HashMap;

use crate::formula::Expr;
use crate::functions::{Env, Tab, lookup_pos};
use crate::value::{Matrix, Value};

/// A value as an exact MATCH compares it: numbers by value, text without
/// case, booleans.
#[derive(Hash, PartialEq, Eq)]
enum Key {
    Num(u64),
    Text(String),
    Bool(bool),
}

fn key(v: &Value) -> Option<Key> {
    Some(match v {
        Value::Number(n) if n.is_nan() => return None,
        Value::Number(n) => Key::Num(if *n == 0.0 { 0 } else { n.to_bits() }),
        Value::Text(t) => Key::Text(t.chars().flat_map(char::to_lowercase).collect()),
        Value::Bool(b) => Key::Bool(*b),
        _ => return None,
    })
}

impl Env<'_> {
    /// MATCH with an array of keys and exact matching (None: not this case,
    /// the call goes the usual way).
    pub(crate) fn match_many(&self, a: &[Expr]) -> Option<Value> {
        if !matches!(a.len(), 3) || !matches!(self.eval(&a[2]), Value::Number(n) if n == 0.0) {
            return None;
        }
        let Value::Array(keys) = self.eval(&a[0]) else { return None };
        let t = self.tab(&a[1]).ok()?;
        if t.cols() != 1 && t.rows() != 1 {
            return None;
        }
        // First position of each value in the lookup range.
        let mut index: HashMap<Key, u32> = HashMap::new();
        let mut add = |i: u32, v: &Value| {
            if let Some(k) = key(v) {
                index.entry(k).and_modify(|p| *p = (*p).min(i)).or_insert(i);
            }
        };
        match &t {
            Tab::R(ctx, s, r) => crate::functions::scan(*ctx, *s, r, &mut |c, v| add(c.row - r.start.row + c.col - r.start.col, v)),
            Tab::A(m) => m.data.iter().enumerate().for_each(|(i, v)| add(i as u32, v)),
        }
        let data = keys
            .data
            .iter()
            .map(|k| match k {
                Value::Error(e) => Value::Error(*e),
                // Wildcards and blanks: the usual search.
                Value::Text(s) if s.contains(['*', '?']) => lookup_pos(t.len() as u32, |i| t.nth(i as usize), k, 0, &self.locale).map_or_else(Value::Error, |i| Value::Number(i as f64 + 1.0)),
                Value::Empty => lookup_pos(t.len() as u32, |i| t.nth(i as usize), k, 0, &self.locale).map_or_else(Value::Error, |i| Value::Number(i as f64 + 1.0)),
                _ => match key(k).and_then(|k| index.get(&k)) {
                    Some(&i) => Value::Number(i as f64 + 1.0),
                    None => Value::Error(crate::value::ErrorKind::Na),
                },
            })
            .collect();
        Some(Matrix::new(keys.rows, keys.cols, data).into_value())
    }
}

#[cfg(test)]
mod tests {
    use crate::addr::CellRef;
    use crate::value::Value;
    use crate::workbook::Workbook;

    #[test]
    fn same_as_one_by_one() {
        let mut wb = Workbook::new();
        wb.locale = crate::format::Locale::EN;
        for (r, v) in ["b", "A", "B", "2", "TRUE", "a*", "", "x"].iter().enumerate() {
            wb.set_input(0, CellRef::new(r as u32, 0), v);
        }
        let keys = ["B", "a", "2", "TRUE", "a~*", "a*", "zz", "3", "x"];
        for (r, v) in keys.iter().enumerate() {
            wb.set_input(0, CellRef::new(r as u32, 2), v);
        }
        let many = wb.eval_formula(0, "=MATCH(C1:C9,A:A,0)");
        let Value::Array(m) = many else { panic!("{many:?}") };
        let one: Vec<Value> = (1..=keys.len()).map(|r| wb.eval_formula(0, &format!("=MATCH(C{r},A:A,0)"))).collect();
        assert_eq!(m.data, one);
        assert_eq!((&one[0], &one[8]), (&Value::Number(1.0), &Value::Number(8.0)));
        // Distinct values, the usual pattern: b, A, 2, TRUE ("B" repeats b,
        // and the wildcard in "a*" finds "A" first).
        assert_eq!(wb.eval_formula(0, "=SUM(IFERROR(--(MATCH(A1:A6,A:A,0)=ROW(A1:A6)),0))"), Value::Number(4.0));
    }
}
