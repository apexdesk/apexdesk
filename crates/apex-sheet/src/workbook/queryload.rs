//! Loading a Get Data query's result into its table: the table is resized
//! to the new rows and columns (or made on a new sheet the first time),
//! the cells written, and the query list saved, all in one undo step.

use super::*;
use crate::data::AutoFilter;
use crate::query::Query;
use crate::table::{Table, TableColumn};

/// A query result ready for the cells: column names, rows, and the number
/// format of each column (None: General).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QueryResult {
    pub names: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub formats: Vec<Option<String>>,
}

/// Why a result could not be loaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadError {
    NoQuery,
    /// The table would overlap another table (or merged cells).
    Overlap,
    /// More rows or columns than a sheet has.
    TooBig,
}

/// A table name made of `base` (a query name): letters, digits and `_`.
fn table_name_from(base: &str) -> String {
    let mut s: String = base.chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if !s.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_') {
        s.insert(0, '_');
    }
    if !crate::table::valid_name(&s) {
        s.insert_str(0, "Q_");
    }
    s
}

impl Workbook {
    /// Replaces the query list (one undo step), e.g. after editing a
    /// query that is not loaded again.
    pub fn set_queries(&mut self, list: Vec<Query>) {
        self.execute_as(Op::Queries(list), "undo.queries");
    }

    /// Writes the result of query `name` of `queries` into its table and
    /// makes `queries` the workbook's list, as one undo step. A query with
    /// no table (or whose table is gone) gets a new sheet named after it;
    /// returns the table's name.
    pub fn load_query(&mut self, mut queries: Vec<Query>, name: &str, res: &QueryResult, label: &'static str) -> Result<String, LoadError> {
        let qi = queries.iter().position(|q| q.name.eq_ignore_ascii_case(name)).ok_or(LoadError::NoQuery)?;
        let ncols = res.names.len().max(1) as u32;
        let nrows = res.rows.len().max(1) as u32;
        let mut ops = Vec::new();
        let existing = queries[qi].table.as_deref().and_then(|t| self.find_table(t));
        let (si, old, start) = match existing {
            Some((si, ti)) => {
                let t = self.sheets[si].tables[ti].clone();
                let start = t.range.start;
                (si, Some((ti, t)), start)
            }
            None => {
                let si = self.sheets.len();
                let base: String = queries[qi].name.replace(['[', ']', ':', '*', '?', '/', '\\', '\''], "_").chars().take(31).collect();
                let base = if base.trim().is_empty() { "Query".to_string() } else { base };
                let sname = if self.sheet_index(&base).is_none() { base } else { self.unique_sheet_name(&base) };
                let sheet = Sheet::new(sname);
                ops.push(Op::AddSheet { index: si, sheet: Box::new(sheet) });
                (si, None, CellRef::new(0, 0))
            }
        };
        let totals = old.as_ref().is_some_and(|(_, t)| t.totals);
        let end_row = start.row as u64 + nrows as u64 + totals as u64;
        let end_col = start.col as u64 + ncols as u64 - 1;
        if end_row >= MAX_ROWS as u64 || end_col >= MAX_COLS as u64 {
            return Err(LoadError::TooBig);
        }
        let range = Range::new(start, CellRef::new(end_row as u32, end_col as u32));
        if let Some(sh) = self.sheets.get(si) {
            let own = old.as_ref().map(|(ti, _)| *ti);
            if sh.tables.iter().enumerate().any(|(i, t)| Some(i) != own && t.range.intersect(&range).is_some()) || sh.merges.iter().any(|m| m.intersect(&range).is_some()) {
                return Err(LoadError::Overlap);
            }
        }
        // Column names: unique and never empty.
        let mut unique = crate::table::UniqueNames::default();
        let names: Vec<String> = (0..ncols as usize).map(|i| unique.add(res.names.get(i).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| format!("Column{}", i + 1)))).collect();
        // Number formats of the columns.
        let styles: Vec<Option<u32>> = (0..ncols as usize)
            .map(|i| {
                let f = res.formats.get(i).cloned().flatten()?;
                Some(self.styles.intern(CellStyle { num_fmt: f.into(), ..CellStyle::default() }))
            })
            .collect();
        let sh = self.sheets.get(si);
        let get = |c: CellRef| sh.and_then(|s| s.get(c));
        let mut cells: Vec<(CellRef, Option<Cell>)> = Vec::new();
        // The old totals row, to move under the new data.
        let old_totals: Vec<(u32, Option<Cell>)> = match &old {
            Some((_, t)) if t.totals => (t.range.start.col..=t.range.end.col).map(|c| (c, get(CellRef::new(t.range.end.row, c)).cloned())).collect(),
            _ => Vec::new(),
        };
        let mut written: HashSet<CellRef> = HashSet::default();
        for (i, n) in names.iter().enumerate() {
            let c = CellRef::new(start.row, start.col + i as u32);
            cells.push((c, Cell::with_content(get(c), Content::Value(Value::text(n.as_str())))));
            written.insert(c);
        }
        for r in 0..nrows {
            for col in 0..ncols {
                let c = CellRef::new(start.row + 1 + r, start.col + col);
                let v = res.rows.get(r as usize).and_then(|row| row.get(col as usize)).cloned().unwrap_or(Value::Empty);
                let mut cell = Cell::with_content(get(c), if matches!(v, Value::Empty) { Content::Empty } else { Content::Value(v) });
                if let Some(st) = styles[col as usize] {
                    let base = cell.get_or_insert_with(|| Cell::new(Content::Empty, 0));
                    base.style = st;
                }
                cells.push((c, cell));
                written.insert(c);
            }
        }
        if totals {
            for (c, cell) in &old_totals {
                if *c <= range.end.col {
                    let at = CellRef::new(range.end.row, *c);
                    cells.push((at, cell.clone()));
                    written.insert(at);
                }
            }
        }
        // Cells of the old table the new one no longer covers.
        if let Some((_, t)) = &old {
            for r in t.range.start.row..=t.range.end.row {
                for c in t.range.start.col..=t.range.end.col {
                    let at = CellRef::new(r, c);
                    if !written.contains(&at) && get(at).is_some() {
                        cells.push((at, None));
                    }
                }
            }
        }
        // The table.
        let table = match &old {
            Some((_, t)) => {
                let mut nt = t.clone();
                nt.range = range;
                nt.header = true;
                nt.columns = names.iter().map(|n| t.columns.iter().find(|c| c.name.eq_ignore_ascii_case(n)).cloned().map(|mut c| {
                    c.name = n.clone();
                    c.formula = None;
                    c
                }).unwrap_or_else(|| TableColumn::new(n.clone()))).collect();
                nt
            }
            None => {
                let mut tn = table_name_from(&queries[qi].name);
                if !self.table_name_free(&tn, None) {
                    tn = (2..).map(|i| format!("{tn}{i}")).find(|n| self.table_name_free(n, None)).unwrap_or(tn);
                }
                let mut t = Table::new(self.tables().map(|(_, t)| t.id).max().unwrap_or(0) + 1, tn, range, names.clone());
                t.style.name = "TableStyleMedium2".into();
                t
            }
        };
        queries[qi].table = Some(table.name.clone());
        let mut list = self.sheets.get(si).map(|s| s.tables.clone()).unwrap_or_default();
        match &old {
            Some((ti, _)) => list[*ti] = table.clone(),
            None => list.push(table.clone()),
        }
        ops.push(Op::SetCells { sheet: si, cells });
        ops.push(Op::Tables { sheet: si, list });
        // The filter buttons follow the table (their criteria are dropped:
        // the rows are new).
        let filter = self.sheets.get(si).and_then(|s| s.filter.clone());
        let ours = match (&filter, &old) {
            (Some(f), Some((_, t))) => f.range == t.filter_range(),
            (None, _) => true,
            _ => false,
        };
        if table.filter_button && ours {
            if let (Some(f), Some(sh)) = (&filter, self.sheets.get(si)) {
                let sizes: Vec<(u32, Option<f32>)> = sh.row_heights.iter().filter(|(r, h)| **h == 0.0 && **r > f.range.start.row && **r <= f.range.end.row).map(|(r, _)| (*r, None)).collect();
                if !sizes.is_empty() {
                    ops.push(Op::RowHeights { sheet: si, sizes });
                }
            }
            ops.push(Op::Filter { sheet: si, filter: Some(AutoFilter::new(table.filter_range())) });
        }
        // A new sheet gets columns wide enough for their contents.
        if old.is_none() {
            let sizes: Vec<(u32, Option<f32>)> = (0..ncols as usize)
                .map(|i| {
                    let chars = std::iter::once(names[i].chars().count() + 3).chain(res.rows.iter().take(200).map(|r| r.get(i).map_or(0, |v| match v {
                        Value::Text(t) => t.chars().count(),
                        Value::Number(_) => 11,
                        _ => 6,
                    }))).max().unwrap_or(8);
                    (start.col + i as u32, Some((chars as f32 * 7.5 + 12.0).clamp(60.0, 320.0)))
                })
                .collect();
            ops.push(Op::ColWidths { sheet: si, sizes });
        }
        ops.push(Op::Queries(queries));
        self.execute_as(Op::Batch(ops), label);
        Ok(table.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::Source;

    fn result(rows: usize) -> QueryResult {
        QueryResult {
            names: vec!["Name".into(), "Qty".into()],
            rows: (0..rows).map(|i| vec![Value::text(format!("n{i}").as_str()), Value::Number(i as f64)]).collect(),
            formats: vec![None, Some("0.00".into())],
        }
    }

    #[test]
    fn loads_resizes_and_undoes() {
        let mut wb = Workbook::new();
        let q = vec![Query::new("Sales data", Source::Table { name: "X".into() })];
        let t = wb.load_query(q, "Sales data", &result(3), "undo.query_load").unwrap();
        assert_eq!(t, "Sales_data");
        assert_eq!(wb.sheets.len(), 2);
        assert_eq!(wb.sheets[1].name, "Sales data");
        let id = wb.find_table(&t).unwrap();
        assert_eq!(wb.table(id).unwrap().range, Range::parse("A1:B4").unwrap());
        assert_eq!(wb.queries[0].table.as_deref(), Some("Sales_data"));
        let st = wb.sheets[1].get(CellRef::new(3, 1)).unwrap().style;
        assert_eq!(&*wb.styles.get(st).num_fmt, "0.00");
        // Refresh with fewer rows: the table shrinks and the rest is cleared.
        let q = wb.queries.clone();
        wb.load_query(q, "Sales data", &result(1), "undo.query_load").unwrap();
        assert_eq!(wb.table(id).unwrap().range, Range::parse("A1:B2").unwrap());
        assert!(wb.sheets[1].get(CellRef::new(3, 0)).is_none());
        assert_eq!(wb.sheets.len(), 2);
        wb.undo();
        assert_eq!(wb.table(id).unwrap().range, Range::parse("A1:B4").unwrap());
        wb.undo();
        assert_eq!(wb.sheets.len(), 1);
        assert!(wb.queries.is_empty());
    }
}
