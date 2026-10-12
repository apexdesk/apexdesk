//! The steps of a query, each one a function from a table to a table.

use std::collections::{HashMap, HashSet};

use apex_sheet::query::{AggFn, Aggregate, ColType, FilterOp, JoinKind, Step};

use crate::Error;
use crate::table::{DataTable, Val, convert, parse_date, unique_names};

/// Applies `step` to `t`; `other` gives the result of another query (for
/// appending and merging).
/// Most cells a step may make (a split into many parts, or a merge on a
/// key many rows share, multiply them).
pub(crate) const MAX_CELLS: usize = 1 << 25;

pub(crate) fn too_many() -> Error {
    Error::Invalid("the step makes too many cells".into())
}

pub fn apply(step: &Step, mut t: DataTable, other: &mut dyn FnMut(&str) -> Result<DataTable, Error>) -> Result<DataTable, Error> {
    t.square();
    match step {
        Step::PromoteHeaders => {
            if !t.rows.is_empty() {
                let first = t.rows.remove(0);
                t.columns = unique_names(first.iter().map(Val::to_text).collect());
                // The types were guessed with the names in: guess again.
                for r in &mut t.rows {
                    for v in r.iter_mut() {
                        if !matches!(v, Val::Null) {
                            *v = Val::Text(v.to_text());
                        }
                    }
                }
                t.types = vec![ColType::Any; t.width()];
                t.detect_types();
            }
        }
        Step::KeepRows { skip, take } => {
            let skip = (*skip).min(t.rows.len());
            t.rows.drain(..skip);
            if *take > 0 {
                t.rows.truncate(*take);
            }
        }
        Step::Filter { column, op, value, case } => {
            let c = t.need(column)?;
            let target = filter_target(value);
            t.rows.retain(|r| matches(&r[c], *op, value, *case, &target));
        }
        Step::RemoveColumns(cols) => {
            let drop: Vec<usize> = cols.iter().map(|c| t.need(c)).collect::<Result<_, _>>()?;
            let keep: Vec<usize> = (0..t.width()).filter(|i| !drop.contains(i)).collect();
            t = pick(&t, &keep);
        }
        Step::ChooseColumns(cols) => {
            let keep: Vec<usize> = cols.iter().map(|c| t.need(c)).collect::<Result<_, _>>()?;
            t = pick(&t, &keep);
        }
        Step::Rename(pairs) => {
            for (old, new) in pairs {
                let c = t.need(old)?;
                if new.trim().is_empty() || t.columns.iter().enumerate().any(|(i, n)| i != c && n.eq_ignore_ascii_case(new)) {
                    return Err(Error::DuplicateColumn(new.clone()));
                }
                t.columns[c] = new.clone();
            }
        }
        Step::Move { column, to } => {
            let c = t.need(column)?;
            let to = (*to).min(t.width() - 1);
            let mut order: Vec<usize> = (0..t.width()).filter(|&i| i != c).collect();
            order.insert(to, c);
            t = pick(&t, &order);
        }
        Step::ChangeType { column, ty } => {
            let c = t.need(column)?;
            for r in &mut t.rows {
                r[c] = convert(&r[c], *ty);
            }
            t.types[c] = *ty;
        }
        Step::Split { column, delimiter } => {
            let c = t.need(column)?;
            if delimiter.is_empty() {
                return Err(Error::Invalid("empty delimiter".into()));
            }
            let parts: Vec<Vec<String>> = t.rows.iter().map(|r| r[c].to_text().split(delimiter.as_str()).map(|s| s.trim().to_string()).collect()).collect();
            let n = parts.iter().map(Vec::len).max().unwrap_or(1).max(1);
            if t.rows.len().saturating_mul(t.width() + n) > MAX_CELLS {
                return Err(too_many());
            }
            let names: Vec<String> = (1..=n).map(|i| format!("{}.{i}", t.columns[c])).collect();
            let mut cols = t.columns[..c].to_vec();
            cols.extend(names);
            cols.extend(t.columns[c + 1..].iter().cloned());
            let mut out = DataTable::new(unique_names(cols));
            for (ti, ty) in t.types.iter().enumerate() {
                if ti < c {
                    out.types[ti] = *ty;
                } else if ti > c {
                    out.types[ti + n - 1] = *ty;
                }
            }
            for (r, p) in t.rows.iter().zip(parts) {
                let mut row = r[..c].to_vec();
                row.extend((0..n).map(|i| p.get(i).filter(|s| !s.is_empty()).map_or(Val::Null, |s| Val::Text(s.clone()))));
                row.extend(r[c + 1..].iter().cloned());
                out.rows.push(row);
            }
            // The new columns get their types like a text file's.
            let mut split = DataTable::new((0..n).map(|i| i.to_string()).collect());
            split.rows = out.rows.iter().map(|r| r[c..c + n].to_vec()).collect();
            split.detect_types();
            for (r, s) in out.rows.iter_mut().zip(split.rows) {
                r.splice(c..c + n, s);
            }
            t = out;
        }
        Step::AddColumn { name, formula } => {
            if name.trim().is_empty() || t.col(name).is_some() {
                return Err(Error::DuplicateColumn(name.clone()));
            }
            let e = crate::expr::parse(formula, &t).map_err(Error::Formula)?;
            for r in &mut t.rows {
                let v = crate::expr::eval(&e, r);
                r.push(v);
            }
            t.columns.push(name.clone());
            t.types.push(ColType::Any);
        }
        Step::Replace { column, find, replace, whole } => {
            let c = t.need(column)?;
            for r in &mut t.rows {
                let s = r[c].to_text();
                if *whole {
                    if s == *find {
                        r[c] = if replace.is_empty() { Val::Null } else { Val::Text(replace.clone()) };
                    }
                } else if !find.is_empty() && s.contains(find.as_str()) {
                    r[c] = Val::Text(crate::expr::replace_capped(&s, find, replace).ok_or_else(|| Error::Invalid("the step makes too long a text".into()))?);
                }
            }
            if t.types[c] != ColType::Text && t.types[c] != ColType::Any {
                let ty = t.types[c];
                for r in &mut t.rows {
                    r[c] = convert(&r[c], ty);
                }
            }
        }
        Step::RemoveDuplicates(cols) => {
            let idx: Vec<usize> = if cols.is_empty() { (0..t.width()).collect() } else { cols.iter().map(|c| t.need(c)).collect::<Result<_, _>>()? };
            let mut seen = HashSet::new();
            t.rows.retain(|r| seen.insert(idx.iter().map(|&i| r[i].key()).collect::<Vec<_>>()));
        }
        Step::Sort(keys) => {
            let idx: Vec<(usize, bool)> = keys.iter().map(|(c, asc)| t.need(c).map(|i| (i, *asc))).collect::<Result<_, _>>()?;
            t.rows.sort_by(|a, b| {
                for &(i, asc) in &idx {
                    let o = a[i].compare(&b[i]);
                    if o.is_ne() {
                        return if asc { o } else { o.reverse() };
                    }
                }
                std::cmp::Ordering::Equal
            });
        }
        Step::Group { by, aggs } => t = group(&t, by, aggs)?,
        Step::Pivot { column, values, func } => t = pivot(&t, column, values, *func)?,
        Step::Unpivot { columns, attribute, value } => {
            let idx: Vec<usize> = columns.iter().map(|c| t.need(c)).collect::<Result<_, _>>()?;
            let keep: Vec<usize> = (0..t.width()).filter(|i| !idx.contains(i)).collect();
            if t.rows.len().saturating_mul(idx.len()).saturating_mul(keep.len() + 2) > MAX_CELLS {
                return Err(too_many());
            }
            let mut names: Vec<String> = keep.iter().map(|&i| t.columns[i].clone()).collect();
            names.push(if attribute.is_empty() { "Attribute".into() } else { attribute.clone() });
            names.push(if value.is_empty() { "Value".into() } else { value.clone() });
            let mut out = DataTable::new(unique_names(names));
            for (k, &i) in keep.iter().enumerate() {
                out.types[k] = t.types[i];
            }
            for r in &t.rows {
                for &i in &idx {
                    if r[i].is_null() {
                        continue;
                    }
                    let mut row: Vec<Val> = keep.iter().map(|&k| r[k].clone()).collect();
                    row.push(Val::Text(t.columns[i].clone()));
                    row.push(r[i].clone());
                    out.rows.push(row);
                }
            }
            t = out;
        }
        Step::Append { query } => {
            let o = other(query)?;
            if t.rows.len().saturating_add(o.rows.len()).saturating_mul(t.width() + o.width()) > MAX_CELLS || t.width() + o.width() > 1 << 16 {
                return Err(too_many());
            }
            t = append(t, o);
        }
        Step::Merge { query, keys, join, columns, names } => {
            let o = other(query)?;
            t = merge(&t, &o, query, keys, *join, columns, names)?;
        }
    }
    Ok(t)
}

/// The columns `idx` of `t`, in that order.
fn pick(t: &DataTable, idx: &[usize]) -> DataTable {
    DataTable {
        columns: idx.iter().map(|&i| t.columns[i].clone()).collect(),
        types: idx.iter().map(|&i| t.types[i]).collect(),
        rows: t.rows.iter().map(|r| idx.iter().map(|&i| r[i].clone()).collect()).collect(),
    }
}

/// What a filter compares with: the typed value as a number or a date.
fn filter_target(value: &str) -> Option<f64> {
    crate::table::parse_num(value.trim(), '.').or_else(|| crate::table::parse_num(value.trim(), ',')).or_else(|| parse_date(value))
}

fn matches(v: &Val, op: FilterOp, value: &str, case: bool, target: &Option<f64>) -> bool {
    let (text, want) = if case { (v.to_text(), value.to_string()) } else { (v.to_text().to_lowercase(), value.to_lowercase()) };
    let cmp = || match (v, target) {
        (Val::Num(n) | Val::Date(n), Some(t)) => n.partial_cmp(t),
        (Val::Null, _) => None,
        (Val::Bool(b), _) => Some(b.to_string().cmp(&want)),
        _ => Some(text.cmp(&want)),
    };
    match op {
        FilterOp::Eq => match (v, target) {
            (Val::Num(n) | Val::Date(n), Some(t)) => n == t,
            _ => text == want,
        },
        FilterOp::Ne => match (v, target) {
            (Val::Num(n) | Val::Date(n), Some(t)) => n != t,
            _ => text != want,
        },
        FilterOp::Gt => cmp().is_some_and(|o| o.is_gt()),
        FilterOp::Ge => cmp().is_some_and(|o| o.is_ge()),
        FilterOp::Lt => cmp().is_some_and(|o| o.is_lt()),
        FilterOp::Le => cmp().is_some_and(|o| o.is_le()),
        FilterOp::Contains => text.contains(&want),
        FilterOp::NotContains => !text.contains(&want),
        FilterOp::Begins => text.starts_with(&want),
        FilterOp::Ends => text.ends_with(&want),
        FilterOp::Empty => v.is_null(),
        FilterOp::NotEmpty => !v.is_null(),
    }
}

/// Combines the values of a group.
fn aggregate(vals: &[&Val], f: AggFn, rows: usize) -> Val {
    let nums = || vals.iter().filter_map(|v| if v.is_null() { None } else { v.as_num() });
    let date = vals.iter().any(|v| matches!(v, Val::Date(_)));
    match f {
        AggFn::Count => Val::Num(rows as f64),
        AggFn::CountDistinct => Val::Num(vals.iter().filter(|v| !v.is_null()).map(|v| v.key()).collect::<HashSet<_>>().len() as f64),
        AggFn::Sum => Val::Num(nums().sum()),
        AggFn::Average => {
            let v: Vec<f64> = nums().collect();
            if v.is_empty() { Val::Null } else { Val::Num(v.iter().sum::<f64>() / v.len() as f64) }
        }
        AggFn::Min | AggFn::Max => {
            let non_null: Vec<&&Val> = vals.iter().filter(|v| !v.is_null()).collect();
            if non_null.iter().any(|v| matches!(v, Val::Text(_))) {
                let best = non_null.iter().copied().copied().reduce(|a, b| if (b.compare(a).is_lt()) == (f == AggFn::Min) { b } else { a });
                return best.cloned().unwrap_or(Val::Null);
            }
            let r = nums().reduce(if f == AggFn::Min { f64::min } else { f64::max });
            match r {
                Some(x) if date => Val::Date(x),
                Some(x) => Val::Num(x),
                None => Val::Null,
            }
        }
        AggFn::First => vals.first().map_or(Val::Null, |v| (*v).clone()),
        AggFn::Last => vals.last().map_or(Val::Null, |v| (*v).clone()),
    }
}

fn group(t: &DataTable, by: &[String], aggs: &[Aggregate]) -> Result<DataTable, Error> {
    let keys: Vec<usize> = by.iter().map(|c| t.need(c)).collect::<Result<_, _>>()?;
    let cols: Vec<Option<usize>> = aggs.iter().map(|a| if a.func == AggFn::Count && t.col(&a.column).is_none() { Ok(None) } else { t.need(&a.column).map(Some) }).collect::<Result<_, _>>()?;
    let mut order: Vec<Vec<String>> = Vec::new();
    let mut groups: HashMap<Vec<String>, Vec<usize>> = HashMap::new();
    for (ri, r) in t.rows.iter().enumerate() {
        let k: Vec<String> = keys.iter().map(|&i| r[i].key()).collect();
        groups.entry(k.clone()).or_insert_with(|| {
            order.push(k);
            Vec::new()
        }).push(ri);
    }
    let mut names: Vec<String> = keys.iter().map(|&i| t.columns[i].clone()).collect();
    names.extend(aggs.iter().map(|a| if a.name.trim().is_empty() { format!("{} {}", a.func.key(), a.column).trim().to_string() } else { a.name.clone() }));
    let mut out = DataTable::new(unique_names(names));
    for (k, &i) in keys.iter().enumerate() {
        out.types[k] = t.types[i];
    }
    for k in order {
        let rows = &groups[&k];
        let first = &t.rows[rows[0]];
        let mut row: Vec<Val> = keys.iter().map(|&i| first[i].clone()).collect();
        for (a, c) in aggs.iter().zip(&cols) {
            let vals: Vec<&Val> = match c {
                Some(c) => rows.iter().map(|&r| &t.rows[r][*c]).collect(),
                None => Vec::new(),
            };
            row.push(aggregate(&vals, a.func, rows.len()));
        }
        out.rows.push(row);
    }
    Ok(out)
}

fn pivot(t: &DataTable, column: &str, values: &str, func: AggFn) -> Result<DataTable, Error> {
    let pc = t.need(column)?;
    let vc = t.need(values)?;
    let keep: Vec<usize> = (0..t.width()).filter(|&i| i != pc && i != vc).collect();
    // New columns in the order their values first appear.
    let mut heads: Vec<(String, String)> = Vec::new();
    let mut head_at: HashMap<String, usize> = HashMap::new();
    for r in &t.rows {
        let k = r[pc].key();
        if !head_at.contains_key(&k) {
            head_at.insert(k.clone(), heads.len());
            heads.push((k, if r[pc].is_null() { "(null)".into() } else { r[pc].to_text() }));
        }
    }
    let mut order: Vec<Vec<String>> = Vec::new();
    let mut groups: HashMap<Vec<String>, Vec<usize>> = HashMap::new();
    for (ri, r) in t.rows.iter().enumerate() {
        let k: Vec<String> = keep.iter().map(|&i| r[i].key()).collect();
        groups.entry(k.clone()).or_insert_with(|| {
            order.push(k);
            Vec::new()
        }).push(ri);
    }
    let mut names: Vec<String> = keep.iter().map(|&i| t.columns[i].clone()).collect();
    names.extend(heads.iter().map(|(_, n)| n.clone()));
    let mut out = DataTable::new(unique_names(names));
    for (k, &i) in keep.iter().enumerate() {
        out.types[k] = t.types[i];
    }
    if order.len().saturating_mul(keep.len() + heads.len()) > MAX_CELLS {
        return Err(too_many());
    }
    for k in order {
        let rows = &groups[&k];
        let mut row: Vec<Val> = keep.iter().map(|&i| t.rows[rows[0]][i].clone()).collect();
        // The group's values under each new column.
        let mut by_head: Vec<Vec<&Val>> = vec![Vec::new(); heads.len()];
        for &r in rows {
            by_head[head_at[&t.rows[r][pc].key()]].push(&t.rows[r][vc]);
        }
        for vals in by_head {
            row.push(if vals.is_empty() { Val::Null } else { aggregate(&vals, func, vals.len()) });
        }
        out.rows.push(row);
    }
    Ok(out)
}

/// The rows of `b` after those of `a`, columns matched by name (new ones
/// added at the end).
pub fn append(mut a: DataTable, b: DataTable) -> DataTable {
    // `a`'s columns by name, and by name in any case (as `col`).
    let mut exact: HashMap<String, usize> = HashMap::new();
    let mut folded: HashMap<String, usize> = HashMap::new();
    for (i, c) in a.columns.iter().enumerate() {
        exact.entry(c.clone()).or_insert(i);
        folded.entry(c.to_ascii_lowercase()).or_insert(i);
    }
    let map: Vec<usize> = b
        .columns
        .iter()
        .enumerate()
        .map(|(bi, n)| match exact.get(n).or_else(|| folded.get(&n.to_ascii_lowercase())) {
            Some(&i) => i,
            None => {
                let i = a.columns.len();
                exact.insert(n.clone(), i);
                folded.entry(n.to_ascii_lowercase()).or_insert(i);
                a.columns.push(n.clone());
                a.types.push(b.types.get(bi).copied().unwrap_or(ColType::Any));
                i
            }
        })
        .collect();
    a.square();
    let w = a.width();
    for r in b.rows {
        let mut row = vec![Val::Null; w];
        for (v, &i) in r.into_iter().zip(&map) {
            row[i] = v;
        }
        a.rows.push(row);
    }
    a
}

fn merge(a: &DataTable, b: &DataTable, query: &str, keys: &[(String, String)], join: JoinKind, columns: &[String], renamed: &[(String, String)]) -> Result<DataTable, Error> {
    if keys.is_empty() {
        return Err(Error::Invalid("no key columns".into()));
    }
    let ka: Vec<usize> = keys.iter().map(|(l, _)| a.need(l)).collect::<Result<_, _>>()?;
    let kb: Vec<usize> = keys.iter().map(|(_, r)| b.need(r)).collect::<Result<_, _>>()?;
    let bring: Vec<usize> = if columns.is_empty() { (0..b.width()).filter(|i| !kb.contains(i)).collect() } else { columns.iter().map(|c| b.need(c)).collect::<Result<_, _>>()? };
    let key = |r: &[Val], k: &[usize]| -> Option<Vec<String>> {
        // Null keys match nothing, as in SQL.
        k.iter().map(|&i| if r[i].is_null() { None } else { Some(r[i].key()) }).collect()
    };
    let mut index: HashMap<Vec<String>, Vec<usize>> = HashMap::new();
    for (i, r) in b.rows.iter().enumerate() {
        if let Some(k) = key(r, &kb) {
            index.entry(k).or_default().push(i);
        }
    }
    let mut names = a.columns.clone();
    let anti = join == JoinKind::Anti;
    if !anti {
        for &i in &bring {
            let n = &b.columns[i];
            names.push(match renamed.iter().find(|(o, _)| o == n) {
                Some((_, to)) => to.clone(),
                None if a.col(n).is_some() => format!("{query}.{n}"),
                None => n.clone(),
            });
        }
    }
    let mut out = DataTable::new(unique_names(names));
    out.types[..a.width()].copy_from_slice(&a.types);
    if !anti {
        for (k, &i) in bring.iter().enumerate() {
            out.types[a.width() + k] = b.types[i];
        }
    }
    let mut used = vec![false; b.rows.len()];
    for r in &a.rows {
        let found = key(r, &ka).and_then(|k| index.get(&k)).cloned().unwrap_or_default();
        if anti {
            if found.is_empty() {
                out.rows.push(r.clone());
            }
            continue;
        }
        if found.is_empty() {
            if matches!(join, JoinKind::Left | JoinKind::Full) {
                let mut row = r.clone();
                row.extend(std::iter::repeat_n(Val::Null, bring.len()));
                out.rows.push(row);
            }
            continue;
        }
        if out.rows.len().saturating_add(found.len()).saturating_mul(out.width()) > MAX_CELLS {
            return Err(too_many());
        }
        for bi in found {
            used[bi] = true;
            let mut row = r.clone();
            row.extend(bring.iter().map(|&i| b.rows[bi][i].clone()));
            out.rows.push(row);
        }
    }
    if matches!(join, JoinKind::Right | JoinKind::Full) {
        for (bi, br) in b.rows.iter().enumerate() {
            if used[bi] {
                continue;
            }
            let mut row = vec![Val::Null; a.width()];
            // The keys go into this side's key columns.
            for (&x, &y) in ka.iter().zip(&kb) {
                row[x] = br[y].clone();
            }
            row.extend(bring.iter().map(|&i| br[i].clone()));
            out.rows.push(row);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_sheet::query::Aggregate;

    fn sales() -> DataTable {
        let rows = [["Region", "Product", "Year", "Amount"], ["North", "A", "2023", "10"], ["South", "A", "2023", "5"], ["North", "B", "2024", "7"], ["North", "A", "2024", "3"], ["South", "B", "2024", ""]];
        DataTable::from_text_rows(rows.iter().map(|r| r.iter().map(|s| s.to_string()).collect()).collect(), true)
    }

    fn run(step: Step, t: DataTable) -> DataTable {
        apply(&step, t, &mut |_| Err(Error::Invalid("none".into()))).unwrap()
    }

    fn col(t: &DataTable, c: &str) -> Vec<String> {
        let i = t.col(c).unwrap();
        t.rows.iter().map(|r| r[i].to_text()).collect()
    }

    #[test]
    fn filter_sort_columns() {
        let t = run(Step::Filter { column: "Amount".into(), op: FilterOp::Ge, value: "5".into(), case: false }, sales());
        assert_eq!(col(&t, "Amount"), ["10", "5", "7"]);
        let t = run(Step::Filter { column: "region".into(), op: FilterOp::Eq, value: "north".into(), case: false }, sales());
        assert_eq!(t.rows.len(), 3);
        // Telling capitals apart.
        let t = run(Step::Filter { column: "region".into(), op: FilterOp::Eq, value: "north".into(), case: true }, sales());
        assert_eq!(t.rows.len(), 0);
        let t = run(Step::Filter { column: "region".into(), op: FilterOp::Contains, value: "Nor".into(), case: true }, sales());
        assert_eq!(t.rows.len(), 3);
        let t = run(Step::Filter { column: "Amount".into(), op: FilterOp::Empty, value: String::new(), case: false }, sales());
        assert_eq!(t.rows.len(), 1);
        let t = run(Step::Sort(vec![("Region".into(), false), ("Amount".into(), true)]), sales());
        assert_eq!(col(&t, "Amount"), ["", "5", "3", "7", "10"]);
        let t = run(Step::ChooseColumns(vec!["Amount".into(), "Region".into()]), sales());
        assert_eq!(t.columns, ["Amount", "Region"]);
        let t = run(Step::RemoveColumns(vec!["Year".into()]), sales());
        assert_eq!(t.columns, ["Region", "Product", "Amount"]);
        let t = run(Step::Move { column: "Amount".into(), to: 0 }, sales());
        assert_eq!(t.columns, ["Amount", "Region", "Product", "Year"]);
        let t = run(Step::Rename(vec![("Amount".into(), "Total".into())]), sales());
        assert_eq!(t.columns[3], "Total");
        assert!(apply(&Step::Rename(vec![("Amount".into(), "Year".into())]), sales(), &mut |_| unreachable!()).is_err());
        assert!(matches!(apply(&Step::Sort(vec![("Nope".into(), true)]), sales(), &mut |_| unreachable!()), Err(Error::MissingColumn(_))));
        let t = run(Step::KeepRows { skip: 1, take: 2 }, sales());
        assert_eq!(col(&t, "Amount"), ["5", "7"]);
        let t = run(Step::RemoveDuplicates(vec!["Region".into(), "Product".into()]), sales());
        assert_eq!(t.rows.len(), 4);
    }

    #[test]
    fn types_split_add_replace() {
        let t = run(Step::ChangeType { column: "Year".into(), ty: ColType::Text }, sales());
        assert_eq!(t.rows[0][2], Val::text("2023"));
        let t = run(Step::ChangeType { column: "Region".into(), ty: ColType::Number }, sales());
        assert_eq!(t.rows[0][0], Val::Null);
        let mut s = DataTable::new(vec!["Name".into(), "X".into()]);
        s.rows = vec![vec![Val::text("Doe, John, 12"), Val::Num(1.0)], vec![Val::text("Roe, Ann"), Val::Num(2.0)]];
        let t = run(Step::Split { column: "Name".into(), delimiter: ",".into() }, s);
        assert_eq!(t.columns, ["Name.1", "Name.2", "Name.3", "X"]);
        assert_eq!(t.rows[0], vec![Val::text("Doe"), Val::text("John"), Val::Num(12.0), Val::Num(1.0)]);
        assert_eq!(t.rows[1][2], Val::Null);
        let t = run(Step::AddColumn { name: "Double".into(), formula: "[Amount] * 2".into() }, sales());
        assert_eq!(col(&t, "Double"), ["20", "10", "14", "6", "0"]);
        let t = run(Step::Replace { column: "Region".into(), find: "orth".into(), replace: "ORTE".into(), whole: false }, sales());
        assert_eq!(col(&t, "Region")[0], "NORTE");
    }

    #[test]
    fn group_pivot_unpivot() {
        let aggs = vec![Aggregate { column: "Amount".into(), func: AggFn::Sum, name: "Total".into() }, Aggregate { column: String::new(), func: AggFn::Count, name: "Rows".into() }, Aggregate { column: "Amount".into(), func: AggFn::Max, name: String::new() }];
        let t = run(Step::Group { by: vec!["Region".into()], aggs }, sales());
        assert_eq!(t.columns, ["Region", "Total", "Rows", "max Amount"]);
        assert_eq!(t.rows[0], vec![Val::text("North"), Val::Num(20.0), Val::Num(3.0), Val::Num(10.0)]);
        assert_eq!(t.rows[1], vec![Val::text("South"), Val::Num(5.0), Val::Num(2.0), Val::Num(5.0)]);
        let base = run(Step::RemoveColumns(vec!["Product".into()]), sales());
        let t = run(Step::Pivot { column: "Year".into(), values: "Amount".into(), func: AggFn::Sum }, base);
        assert_eq!(t.columns, ["Region", "2023", "2024"]);
        assert_eq!(t.rows[0], vec![Val::text("North"), Val::Num(10.0), Val::Num(10.0)]);
        assert_eq!(t.rows[1], vec![Val::text("South"), Val::Num(5.0), Val::Num(0.0)]);
        let t = run(Step::Unpivot { columns: vec!["2023".into(), "2024".into()], attribute: "Year".into(), value: "Amount".into() }, t);
        assert_eq!(t.columns, ["Region", "Year", "Amount"]);
        assert_eq!(t.rows.len(), 4);
        assert_eq!(t.rows[1], vec![Val::text("North"), Val::text("2024"), Val::Num(10.0)]);
    }

    #[test]
    fn append_and_merge() {
        let mut regions = DataTable::new(vec!["Code".into(), "Label".into(), "Region".into()]);
        regions.rows = vec![vec![Val::text("North"), Val::text("Norte"), Val::text("x")], vec![Val::text("East"), Val::text("Este"), Val::text("y")]];
        let mut other = |n: &str| if n == "Regions" { Ok(regions.clone()) } else { Err(Error::NoQuery(n.into())) };
        let m = |join| Step::Merge { query: "Regions".into(), keys: vec![("Region".into(), "Code".into())], join, columns: vec![], names: vec![] };
        let t = apply(&m(JoinKind::Left), sales(), &mut other).unwrap();
        assert_eq!(t.columns, ["Region", "Product", "Year", "Amount", "Label", "Regions.Region"]);
        assert_eq!(t.rows.len(), 5);
        assert_eq!(col(&t, "Label"), ["Norte", "", "Norte", "Norte", ""]);
        let t = apply(&m(JoinKind::Inner), sales(), &mut other).unwrap();
        assert_eq!(t.rows.len(), 3);
        let t = apply(&m(JoinKind::Anti), sales(), &mut other).unwrap();
        assert_eq!(col(&t, "Region"), ["South", "South"]);
        let t = apply(&m(JoinKind::Full), sales(), &mut other).unwrap();
        assert_eq!(t.rows.len(), 6);
        assert_eq!(t.rows[5][0], Val::text("East"));
        let t = apply(&m(JoinKind::Right), sales(), &mut other).unwrap();
        assert_eq!(t.rows.len(), 4);
        // Their "Region" clashes with ours: named as asked, ours kept.
        let named = Step::Merge { query: "Regions".into(), keys: vec![("Region".into(), "Code".into())], join: JoinKind::Left, columns: vec!["Region".into(), "Label".into()], names: vec![("Region".into(), "Zone".into()), ("Label".into(), "Regions.Label".into())] };
        let t = apply(&named, sales(), &mut other).unwrap();
        assert_eq!(t.columns, ["Region", "Product", "Year", "Amount", "Zone", "Regions.Label"]);
        assert_eq!(col(&t, "Zone")[0], "x");
        assert!(apply(&Step::Append { query: "Nope".into() }, sales(), &mut other).is_err());
        let t = apply(&Step::Append { query: "Regions".into() }, sales(), &mut other).unwrap();
        assert_eq!(t.columns, ["Region", "Product", "Year", "Amount", "Code", "Label"]);
        assert_eq!(t.rows.len(), 7);
        assert_eq!(t.rows[6][0], Val::text("y"));
    }

    #[test]
    fn promote_headers() {
        let t = DataTable::from_text_rows(vec![vec!["junk".into(), "".into()], vec!["a".into(), "b".into()], vec!["1".into(), "x".into()]], false);
        let t = run(Step::KeepRows { skip: 1, take: 0 }, t);
        let t = run(Step::PromoteHeaders, t);
        assert_eq!(t.columns, ["a", "b"]);
        assert_eq!(t.rows, vec![vec![Val::Num(1.0), Val::text("x")]]);
    }
}
