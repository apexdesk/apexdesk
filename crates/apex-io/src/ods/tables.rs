//! Tables in ODS. ODF has named database ranges but no table styles or
//! totals rows: a table is written as a database range over its header
//! and data (its look baked into the cells' formats), and read back with
//! a totals row when the row below holds SUBTOTALs of its columns.

use apex_sheet::formula::Expr;
use apex_sheet::functions::Func;
use apex_sheet::table::{Table, TotalFn, items, valid_name};
use apex_sheet::{CellRef, Content, Range, Value, Workbook};

/// Makes the named database range `range` of sheet `si` a table.
pub(super) fn add_table(wb: &mut Workbook, si: usize, name: &str, range: Range, header: bool, buttons: bool) {
    // Each names its columns: a book's worth of tables.
    let cols = wb.sheets.iter().flat_map(|s| &s.tables).map(|t| t.columns.len()).sum::<usize>();
    if !valid_name(name) || range.rows() < 1 + header as u32 || wb.sheets.iter().map(|s| s.tables.len()).sum::<usize>() >= 1024 || cols + range.cols() as usize > 1 << 18 || wb.find_table(name).is_some() {
        return;
    }
    let Some(sh) = wb.sheets.get(si) else { return };
    if sh.tables.iter().any(|t| t.range.intersect(&range).is_some()) || sh.merges.iter().any(|m| m.intersect(&range).is_some()) {
        return;
    }
    let mut names: Vec<String> = Vec::new();
    let mut unique = apex_sheet::table::UniqueNames::default();
    for (i, col) in (range.start.col..=range.end.col).enumerate() {
        let text = if header {
            match sh.value(CellRef::new(range.start.row, col)) {
                // Spaces count: Excel names columns ` ` or `Sum `.
                Value::Text(t) => t.to_string(),
                Value::Number(n) => n.to_string(),
                _ => String::new(),
            }
        } else {
            String::new()
        };
        names.push(unique.add(if text.is_empty() { format!("Column{}", i + 1) } else { text }));
    }
    let id = wb.tables().map(|(_, t)| t.id).max().unwrap_or(0) + 1;
    let mut t = Table::new(id, name, range, names);
    t.header = header;
    t.filter_button = buttons && header;
    // The look is in the cells already.
    t.style.name = String::new();
    infer_totals(wb, si, &mut t);
    // Totals formulas refer to the columns by name again (unless they
    // already do).
    if let Some(row) = t.totals_row() {
        for (i, c) in t.columns.iter().enumerate() {
            let Some(n) = c.total.subtotal() else { continue };
            let at = CellRef::new(row, t.col_at(i));
            let named = wb.sheets[si].get(at).and_then(|x| x.formula()).is_some_and(|e| e.expr().has_structured());
            if named {
                continue;
            }
            let mut src = format!("SUBTOTAL({n},");
            apex_sheet::table::StructRef { table: None, items: 0, cols: Some((c.name.as_str().into(), None)) }.write(&mut src, false, None);
            src.push(')');
            if let (Ok(e), Some(cell)) = (apex_sheet::formula::parse_canonical(&src), wb.sheets[si].get_mut(at)) {
                cell.content = Content::Formula { expr: e.into(), value: Value::Empty };
            }
        }
    }
    wb.sheets[si].tables.push(t);
}

/// A row right below the data whose formulas are SUBTOTALs of the table's
/// columns is its totals row.
fn infer_totals(wb: &Workbook, si: usize, t: &mut Table) {
    let row = t.range.end.row + 1;
    let sh = &wb.sheets[si];
    let (d0, d1) = (t.range.start.row + t.header as u32, t.range.end.row);
    let mut found = false;
    let mut fns = Vec::new();
    for (i, col) in (t.range.start.col..=t.range.end.col).enumerate() {
        let cell = sh.get(CellRef::new(row, col));
        let mut f = (TotalFn::None, None);
        match cell.map(|c| &c.content) {
            Some(Content::Formula { expr, .. }) => {
                if let Expr::Call { func: Some(Func::Subtotal), args, .. } = &*expr.expr() {
                    let n = match args.first() {
                        Some(Expr::Number(n)) => *n as u32,
                        _ => 0,
                    };
                    let whole = Range::new(CellRef::new(d0, col), CellRef::new(d1, col));
                    let covers = match args.get(1) {
                        Some(Expr::Range { sheet: None, a, b, .. }) => Range::new(CellRef::new(a.row, a.col), CellRef::new(b.row, b.col)) == whole,
                        // `Table1[Qty]`, as ApexDesk writes it.
                        Some(Expr::Structured(r)) => {
                            r.table.as_deref().is_none_or(|n| n.to_lowercase() == t.name.to_lowercase())
                                && r.items & !items::DATA == 0
                                && r.cols.as_ref().is_some_and(|(a, b)| b.is_none() && a.to_lowercase() == t.columns[i].name.to_lowercase())
                        }
                        _ => false,
                    };
                    match TotalFn::from_subtotal(n) {
                        Some(tf) if covers && args.len() == 2 => {
                            f.0 = tf;
                            found = true;
                        }
                        _ => return,
                    }
                } else {
                    return;
                }
            }
            Some(Content::Value(Value::Text(s))) => f.1 = Some(s.to_string()),
            None | Some(Content::Empty) => {}
            _ => return,
        }
        fns.push((i, f));
    }
    // A row of labels alone is data, not totals.
    if !found || sh.tables.iter().any(|o| o.range.contains(CellRef::new(row, t.range.start.col))) {
        return;
    }
    for (i, (f, label)) in fns {
        t.columns[i].total = f;
        t.columns[i].total_label = label;
    }
    t.totals = true;
    t.range.end.row = row;
}
