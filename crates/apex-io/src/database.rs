//! Access (`.mdb`, `.accdb`) and dBase (`.dbf`) files opened as workbooks
//! (read only): one sheet per table, the column names in a bold, frozen
//! first row and the rows below, dates and money with their formats.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use apex_db::{ColType, Column, Database, Table, Value as DbValue};
use apex_sheet::{CellRef, CellStyle, Content, NumFormat, Sheet, Value, Workbook};

use crate::Error;

fn db_error(e: apex_db::Error) -> Error {
    Error::new(e.to_string())
}

/// Reads the file at `path` (a dBase file with its memo file next to it).
pub fn read_path(path: &Path) -> Result<Workbook, Error> {
    Ok(to_workbook(&apex_db::open(path).map_err(db_error)?))
}

/// Reads an Access file from memory.
pub fn read_access(data: &[u8]) -> Result<Workbook, Error> {
    let f = apex_db::access::AccessFile::from_bytes(data.to_vec()).map_err(db_error)?;
    Ok(to_workbook(&f.read_all().map_err(db_error)?))
}

/// Reads a dBase file from memory (without its memo file).
pub fn read_dbase(data: &[u8]) -> Result<Workbook, Error> {
    let mut w = Vec::new();
    let t = apex_db::dbase::read(data, None, "Table", None, &mut w).map_err(db_error)?;
    Ok(to_workbook(&Database { tables: vec![t], ..Default::default() }))
}

/// A sheet name Excel accepts: at most 31 characters, none of `[]:*?/\`,
/// and not one already used.
fn sheet_name(name: &str, used: &mut HashSet<String>, next: &mut HashMap<String, usize>) -> String {
    let clean: String = name.chars().map(|c| if "[]:*?/\\".contains(c) { '_' } else { c }).collect();
    let clean = clean.trim_matches('\'').to_string();
    let base: String = if clean.is_empty() { "Table".into() } else { clean.chars().take(31).collect() };
    let mut n = base.clone();
    // Counting on from the last number given to this name: thousands of
    // tables called alike would otherwise take quadratic time.
    let k = next.entry(base.to_ascii_lowercase()).or_insert(2);
    while used.contains(&n.to_ascii_lowercase()) {
        let suffix = format!(" ({k})");
        n = base.chars().take(31 - suffix.chars().count()).collect::<String>() + &suffix;
        *k += 1;
    }
    used.insert(n.to_ascii_lowercase());
    n
}

pub fn to_workbook(db: &Database) -> Workbook {
    let mut wb = Workbook::empty();
    let head = wb.styles.intern(CellStyle { bold: true, ..Default::default() });
    let mut formats: HashMap<&'static str, u32> = HashMap::new();
    let (mut used, mut next) = (HashSet::new(), HashMap::new());
    for t in &db.tables {
        let name = sheet_name(&t.name, &mut used, &mut next);
        let si = wb.sheets.len();
        let mut sheet = Sheet::new(name);
        sheet.frozen_rows = 1;
        wb.sheets.push(sheet);
        for (c, col) in t.columns.iter().enumerate() {
            wb.load_cell(si, CellRef::new(0, c as u32), Content::Value(Value::text(col.name.as_str())), head);
        }
        let styles: Vec<u32> = t
            .columns
            .iter()
            .map(|col| {
                let code = match col.ty {
                    ColType::DateTime => "yyyy-mm-dd hh:mm",
                    ColType::Money => "#,##0.00",
                    _ => return 0,
                };
                *formats.entry(code).or_insert_with(|| wb.styles.intern(CellStyle { num_fmt: code.into(), ..Default::default() }))
            })
            .collect();
        for (r, row) in t.rows.iter().enumerate().take(apex_sheet::MAX_ROWS as usize - 1) {
            for (c, v) in row.iter().enumerate() {
                let mut style = styles.get(c).copied().unwrap_or(0);
                let value = match v {
                    DbValue::Null => continue,
                    DbValue::Bool(b) => Value::Bool(*b),
                    DbValue::Int(i) => Value::Number(*i as f64),
                    DbValue::Num(n) => Value::Number(*n),
                    DbValue::Money(m) => Value::Number(*m as f64 / 10_000.0),
                    DbValue::Decimal(s) => s.parse().map(Value::Number).unwrap_or_else(|_| Value::text(s.as_str())),
                    DbValue::Date(d) => {
                        // Dates without a time get a date format.
                        if d.fract() == 0.0 {
                            style = *formats.entry("yyyy-mm-dd").or_insert_with(|| wb.styles.intern(CellStyle { num_fmt: "yyyy-mm-dd".into(), ..Default::default() }));
                        }
                        Value::Number(*d)
                    }
                    other => Value::text(other.to_text()),
                };
                wb.load_cell(si, CellRef::new(r as u32 + 1, c as u32), Content::Value(value), style);
            }
        }
    }
    if wb.sheets.is_empty() {
        wb.sheets.push(Sheet::new("Sheet1"));
    }
    wb
}

/// A workbook's sheets as tables (to import them into a database): the
/// first row names the columns and each column takes the kind all its
/// values share (yes/no, whole numbers, numbers, dates by their format,
/// or text). Empty sheets are left out; formulas give their values.
pub fn from_workbook(wb: &Workbook) -> Database {
    let mut db = Database::default();
    for sh in &wb.sheets {
        let Some(end) = sh.extent() else { continue };
        let ncols = end.col as usize + 1;
        let mut names: Vec<String> = Vec::new();
        for c in 0..ncols {
            let v = sh.value(CellRef::new(0, c as u32));
            let mut n = match v {
                Value::Empty => format!("F{}", c + 1),
                v => v.to_text().map(|t| t.trim().to_string()).unwrap_or_default(),
            };
            if n.is_empty() || names.iter().any(|x| x.eq_ignore_ascii_case(&n)) {
                n = format!("{n}{}", c + 1);
            }
            names.push(n);
        }
        let is_date = |r: u32, c: u32| sh.get(CellRef::new(r, c)).is_some_and(|cell| NumFormat::parse(&wb.styles.get(cell.style).num_fmt).is_date());
        let mut kinds = vec![None::<ColType>; ncols];
        for r in 1..=end.row {
            for (c, kind) in kinds.iter_mut().enumerate() {
                let k = match sh.value(CellRef::new(r, c as u32)) {
                    Value::Empty => continue,
                    Value::Bool(_) => ColType::Bool,
                    Value::Number(_) if is_date(r, c as u32) => ColType::DateTime,
                    Value::Number(n) if n.fract() == 0.0 && n.abs() < 9e15 => ColType::Int64,
                    Value::Number(_) => ColType::Double,
                    _ => ColType::Memo,
                };
                *kind = Some(match (*kind, k) {
                    (None, k) => k,
                    (Some(a), b) if a == b => a,
                    (Some(ColType::Int64), ColType::Double) | (Some(ColType::Double), ColType::Int64) => ColType::Double,
                    _ => ColType::Memo,
                });
            }
        }
        let columns = names.iter().zip(&kinds).map(|(n, k)| Column::new(n.as_str(), k.unwrap_or(ColType::Memo))).collect();
        let mut t = Table::new(sh.name.as_str(), columns);
        for r in 1..=end.row {
            let row: Vec<DbValue> = (0..ncols)
                .map(|c| {
                    let v = sh.value(CellRef::new(r, c as u32));
                    match (v, kinds[c]) {
                        (Value::Empty, _) => DbValue::Null,
                        (Value::Bool(b), _) => DbValue::Bool(*b),
                        (Value::Number(n), Some(ColType::DateTime)) => DbValue::Date(*n),
                        (Value::Number(n), Some(ColType::Int64)) => DbValue::Int(*n as i64),
                        (Value::Number(n), Some(ColType::Double)) => DbValue::Num(*n),
                        (v, _) => v.to_text().map(|t| DbValue::Text(t.to_string())).unwrap_or(DbValue::Null),
                    }
                })
                .collect();
            if row.iter().any(|v| *v != DbValue::Null) {
                t.rows.push(row);
            }
        }
        db.tables.push(t);
    }
    db
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_become_sheets() {
        let mut t = Table::new("Orders/2024", vec![Column::new("Id", ColType::Int32), Column::new("Day", ColType::DateTime), Column::new("Note", ColType::Text)]);
        t.rows.push(vec![DbValue::Int(1), DbValue::Date(45352.0), DbValue::Null]);
        let db = Database { tables: vec![t.clone(), t], ..Default::default() };
        let wb = to_workbook(&db);
        assert_eq!(wb.sheets.len(), 2);
        assert_eq!(wb.sheets[0].name, "Orders_2024");
        assert_eq!(wb.sheets[1].name, "Orders_2024 (2)");
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 1)), &Value::text("Day"));
        assert_eq!(wb.sheets[0].value(CellRef::new(1, 1)), &Value::Number(45352.0));
        assert_eq!(wb.sheets[0].value(CellRef::new(1, 2)), &Value::Empty);
        // And back: the kinds come from the values.
        let back = from_workbook(&wb);
        assert_eq!(back.tables.len(), 2);
        let t = &back.tables[0];
        assert_eq!(t.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Id", "Day", "Note"]);
        assert_eq!(t.columns[0].ty, ColType::Int64);
        assert_eq!(t.columns[1].ty, ColType::DateTime);
        assert_eq!(t.rows[0][..2], [DbValue::Int(1), DbValue::Date(45352.0)]);
    }
}
