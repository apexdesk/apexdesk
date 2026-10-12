//! Where a number comes from: what a cell reads (cells and ranges of any
//! sheet, linked workbooks) and the Get Data query that loaded it; and,
//! the other way, the formulas that would lose their cells when an area is
//! deleted.

use super::*;

/// One source of a cell's value.
#[derive(Clone, Debug, PartialEq)]
pub enum Origin {
    /// Cells of this workbook.
    Cells(usize, Range),
    /// Cells of a linked workbook ([`Workbook::externals`]), with the
    /// values this file saved from it.
    External { book: usize, sheet: String, range: Range },
    /// The cell is in the table a Get Data query loads (its index).
    Query(usize),
}

impl Workbook {
    /// What the value at `c` comes from.
    pub fn origins(&self, sheet: usize, c: CellRef) -> Vec<Origin> {
        let mut out: Vec<Origin> = Vec::new();
        if let Some(f) = self.sheets.get(sheet).and_then(|s| s.get(c)).and_then(|x| x.formula()) {
            out.extend(self.precedents(sheet, c).into_iter().map(|(s, r)| Origin::Cells(s, r)));
            let e = f.expr();
            let mut refs = Vec::new();
            external_refs(&e, &mut refs);
            for (name, r) in refs {
                let Some((key, sh)) = crate::external::split_external(&name) else { continue };
                if let Some((book, _)) = crate::external::find_book(&self.externals, key) {
                    let o = Origin::External { book, sheet: sh.to_string(), range: r };
                    if !out.contains(&o) {
                        out.push(o);
                    }
                }
            }
        }
        if let Some(q) = self.query_at(sheet, c) {
            out.push(Origin::Query(q));
        }
        out
    }

    /// The query whose table holds `c`.
    pub fn query_at(&self, sheet: usize, c: CellRef) -> Option<usize> {
        let t = self.sheets.get(sheet)?.tables.iter().find(|t| t.range.contains(c))?;
        self.queries.iter().position(|q| q.table.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(&t.name)))
    }

    /// Formulas outside `area` of `sheet` that read cells in it (the ones
    /// deleting it would break). `whole_sheet`: only other sheets' count.
    pub fn outside_dependents(&self, sheet: usize, area: Range, whole_sheet: bool) -> Vec<(usize, CellRef)> {
        let Some(sh) = self.sheets.get(sheet) else { return Vec::new() };
        let Some(ext) = sh.extent() else { return Vec::new() };
        // Only the used part matters; ranges reaching past it read nothing.
        let Some(used) = area.intersect(&Range::new(CellRef::new(0, 0), ext)) else { return Vec::new() };
        let mut cells: Vec<CellRef> = Vec::new();
        if (used.rows() as u64) * (used.cols() as u64) <= 200_000 {
            for r in used.start.row..=used.end.row {
                for col in used.start.col..=used.end.col {
                    cells.push(CellRef::new(r, col));
                }
            }
        } else {
            sh.for_each_in(&used, |c, _| cells.push(c));
        }
        let mut out: Vec<(usize, CellRef)> = Vec::new();
        for c in cells {
            for (s, d) in self.dependents(sheet, c) {
                let inside = s == sheet && (whole_sheet || area.contains(d));
                if !inside && !out.contains(&(s, d)) {
                    out.push((s, d));
                    if out.len() >= 1000 {
                        return out;
                    }
                }
            }
        }
        out.sort_unstable_by_key(|(s, c)| (*s, c.row, c.col));
        out
    }
}

/// The references into other workbooks of `e`: (sheet as written, range).
fn external_refs(e: &Expr, out: &mut Vec<(String, Range)>) {
    match e {
        Expr::Ref { sheet: Some(s), addr } if crate::external::is_external(s) => out.push((s.to_string(), Range::single(CellRef::new(addr.row, addr.col)))),
        Expr::Range { sheet: Some(s), a, b, .. } if crate::external::is_external(s) => out.push((s.to_string(), Range::new(CellRef::new(a.row, a.col), CellRef::new(b.row, b.col)))),
        Expr::Unary(_, x) | Expr::Percent(x) | Expr::Paren(x) => external_refs(x, out),
        Expr::Binary(_, a, b) => {
            external_refs(a, out);
            external_refs(b, out);
        }
        Expr::Call { args, .. } => args.iter().for_each(|a| external_refs(a, out)),
        Expr::Array(rows) => rows.iter().flatten().for_each(|a| external_refs(a, out)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::{Query, Source};

    fn c(a: &str) -> CellRef {
        CellRef::parse(a).unwrap()
    }

    #[test]
    fn origins_and_dependents() {
        let mut wb = Workbook::new();
        wb.sheets.push(Sheet::new("Data"));
        wb.set_input(1, c("A1"), "5");
        wb.set_input(1, c("A2"), "7");
        wb.set_input(0, c("B1"), "=SUM(Data!A1:A2)*2");
        wb.set_input(0, c("B2"), "=B1+1");
        assert_eq!(wb.origins(0, c("B2")), vec![Origin::Cells(0, Range::single(c("B1")))]);
        assert_eq!(wb.origins(0, c("B1")), vec![Origin::Cells(1, Range::new(c("A1"), c("A2")))]);
        assert!(wb.origins(1, c("A1")).is_empty());
        // A query's table.
        let mut t = crate::table::Table::new(1, "Sales", Range::new(c("D1"), c("E3")), vec!["Region".into(), "Total".into()]);
        t.header = true;
        wb.sheets[1].tables.push(t);
        let mut q = Query::new("Sales", Source::Text { location: "/data/sales.csv".into(), delimiter: None, encoding: String::new(), header: true, skip: 0 });
        q.table = Some("Sales".into());
        wb.queries.push(q);
        assert_eq!(wb.origins(1, c("E2")), vec![Origin::Query(0)]);
        // Deleting Data!A1:A2 (or the sheet) breaks B1.
        assert_eq!(wb.outside_dependents(1, Range::new(c("A1"), c("A5")), false), vec![(0, c("B1"))]);
        assert_eq!(wb.outside_dependents(1, Range::new(c("A1"), c("XFD1048576")), true), vec![(0, c("B1"))]);
        // Deleting B1 with B2 alongside breaks nothing outside.
        assert!(wb.outside_dependents(0, Range::new(c("B1"), c("B2")), false).is_empty());
        assert_eq!(wb.outside_dependents(0, Range::single(c("B1")), false), vec![(0, c("B2"))]);
    }
}
