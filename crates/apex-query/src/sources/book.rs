//! Workbooks as sources: a table or sheet of this workbook or of another
//! file (xlsx, xls, xlsb, ods, csv), and every file of a folder combined.

use std::path::Path;

use apex_sheet::{CellRef, Range, Workbook};

use crate::table::{DataTable, Val, from_value, unique_names};
use crate::{Error, sources};

/// The values of `r` of sheet `si`: the first row names the columns when
/// `header`.
fn range_table(wb: &Workbook, si: usize, r: Range, header: bool) -> DataTable {
    let sh = &wb.sheets[si];
    let cell = |row: u32, col: u32| {
        let c = CellRef::new(row, col);
        let date = sh.get(c).is_some_and(|x| wb.styles.format(x.style).is_date());
        from_value(sh.value(c), date)
    };
    let cols = r.start.col..=r.end.col;
    let names: Vec<String> = if header { cols.clone().map(|c| cell(r.start.row, c).to_text()).collect() } else { vec![String::new(); r.cols() as usize] };
    let mut t = DataTable::new(unique_names(names));
    let first = r.start.row + header as u32;
    // As many rows as the cells allowed (a sheet's used range may be
    // all of it).
    let most = (crate::steps::MAX_CELLS / (r.cols() as usize).max(1)) as u32;
    let last = r.end.row.min(first.saturating_add(most).saturating_sub(1));
    if first <= last && most > 0 {
        t.rows = (first..=last).map(|row| cols.clone().map(|c| cell(row, c)).collect()).collect();
    }
    t
}

/// The used cells of a sheet.
fn used_range(wb: &Workbook, si: usize) -> Option<Range> {
    let sh = &wb.sheets[si];
    let end = sh.extent()?;
    let (mut r0, mut c0) = (end.row, end.col);
    for (c, _) in sh.iter() {
        r0 = r0.min(c.row);
        c0 = c0.min(c.col);
    }
    Some(Range::new(CellRef::new(r0, c0), end))
}

/// A table of `wb`: a table by name, a sheet by name, or (empty name) the
/// first sheet.
pub fn workbook_table(wb: &Workbook, item: &str, header: bool) -> Result<DataTable, Error> {
    let item = item.trim();
    if !item.is_empty()
        && let Some(id) = wb.find_table(item)
    {
        let t = wb.table(id).ok_or_else(|| Error::NoTable(item.into()))?;
        let data = t.data_range();
        let mut out = DataTable::new(t.columns.iter().map(|c| c.name.clone()).collect());
        let body = range_table(wb, id.0, data, false);
        // A table with only its header row has one blank data row.
        out.rows = body.rows.into_iter().filter(|r| r.iter().any(|v| !v.is_null())).collect();
        return Ok(out);
    }
    let si = if item.is_empty() { (!wb.sheets.is_empty()).then_some(0) } else { wb.sheet_index(item) };
    let si = si.ok_or_else(|| Error::NoTable(item.into()))?;
    Ok(match used_range(wb, si) {
        Some(r) => range_table(wb, si, r, header),
        None => DataTable::default(),
    })
}

/// What a workbook offers: its tables, then its sheets (name, is table).
pub fn items(wb: &Workbook) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = wb.tables().map(|(_, t)| (t.name.clone(), true)).collect();
    out.extend(wb.sheets.iter().map(|s| (s.name.clone(), false)));
    out
}

/// Opens a workbook file.
pub fn open(path: &Path) -> Result<Workbook, Error> {
    match apex_io::open(path) {
        Ok((apex_io::Loaded::Sheet(wb), _)) => Ok(*wb),
        Ok(_) => Err(Error::Parse(format!("{}: not a spreadsheet", path.display()))),
        Err(e) => Err(Error::Io(format!("{}: {}", path.display(), e.0))),
    }
}

/// A table or sheet of another workbook file.
pub fn file_table(path: &Path, item: &str, header: bool) -> Result<DataTable, Error> {
    workbook_table(&open(path)?, item, header)
}

/// Extensions a folder source reads when none is given.
const FOLDER_EXTS: [&str; 10] = ["csv", "tsv", "txt", "json", "xml", "xlsx", "xlsm", "xls", "xlsb", "ods"];

/// One file of a folder, by its extension.
fn file(path: &Path, header: bool) -> Result<DataTable, Error> {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    let read = || {
        let io = |e: std::io::Error| Error::Io(format!("{}: {e}", path.display()));
        if std::fs::metadata(path).map_err(io)?.len() > sources::MAX_BYTES {
            return Err(Error::Io(format!("{}: too big", path.display())));
        }
        std::fs::read(path).map_err(io)
    };
    match ext.as_str() {
        "json" => sources::json::table(&apex_io::decode_text(&read()?), ""),
        "xml" => sources::xml::table(&apex_io::decode_text(&read()?), ""),
        "xlsx" | "xlsm" | "xls" | "xlsb" | "ods" => file_table(path, "", header),
        "tsv" => Ok(sources::text::parse(&apex_io::decode_text(&read()?), Some('\t'), header, 0)),
        _ => Ok(sources::text::parse(&apex_io::decode_text(&read()?), None, header, 0)),
    }
}

/// The files of a folder with extension `ext` (empty: any known one),
/// sorted by name.
pub fn folder_files(dir: &Path, ext: &str) -> Result<Vec<std::path::PathBuf>, Error> {
    let want = ext.trim().trim_start_matches(['*', '.']).to_ascii_lowercase();
    let rd = std::fs::read_dir(dir).map_err(|e| Error::Io(format!("{}: {e}", dir.display())))?;
    let mut files: Vec<std::path::PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            // Lock files and hidden files are not data.
            if name.starts_with('.') || name.starts_with("~$") || name.starts_with(".~lock") {
                return false;
            }
            let e = p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
            if want.is_empty() { FOLDER_EXTS.contains(&e.as_str()) } else { e == want }
        })
        .collect();
    files.sort();
    Ok(files)
}

/// Every file of a folder, one after another (columns matched by name);
/// `file_column` puts the file name first.
pub fn folder(dir: &Path, ext: &str, header: bool, file_column: bool) -> Result<DataTable, Error> {
    let files = folder_files(dir, ext)?;
    if files.is_empty() {
        return Err(Error::Io(format!("{}: no files", dir.display())));
    }
    let mut out: Option<DataTable> = None;
    for f in files {
        let mut t = file(&f, header)?;
        if file_column {
            let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            t.columns.insert(0, "Source.Name".into());
            t.types.insert(0, apex_sheet::query::ColType::Text);
            for r in &mut t.rows {
                r.insert(0, Val::Text(name.clone()));
            }
            t.columns = unique_names(std::mem::take(&mut t.columns));
        }
        out = Some(match out {
            None => t,
            Some(a) => {
                if a.rows.len().saturating_add(t.rows.len()).saturating_mul(a.width() + t.width()) > crate::steps::MAX_CELLS || a.width() + t.width() > 1 << 16 {
                    return Err(crate::steps::too_many());
                }
                crate::steps::append(a, t)
            }
        });
    }
    Ok(out.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_sheet::Value;

    #[test]
    fn tables_and_sheets() {
        let mut wb = Workbook::new();
        wb.set_input(0, CellRef::new(1, 1), "Name");
        wb.set_input(0, CellRef::new(1, 2), "When");
        wb.set_input(0, CellRef::new(2, 1), "Ann");
        wb.set_input(0, CellRef::new(2, 2), "2024-01-02");
        wb.set_input(0, CellRef::new(3, 1), "Bo");
        let t = workbook_table(&wb, "", true).unwrap();
        assert_eq!(t.columns, ["Name", "When"]);
        assert_eq!(t.rows[0][1], Val::Date(45293.0));
        assert_eq!(t.rows[1], vec![Val::text("Bo"), Val::Null]);
        wb.create_table(0, Range::parse("B2:C4").unwrap(), true, "TableStyleLight1").unwrap();
        let t = workbook_table(&wb, "Table1", true).unwrap();
        assert_eq!(t.rows.len(), 2);
        assert!(matches!(workbook_table(&wb, "Nope", true), Err(Error::NoTable(_))));
        assert_eq!(items(&wb), vec![("Table1".to_string(), true), ("Sheet1".to_string(), false)]);
        assert!(matches!(wb.sheets[0].value(CellRef::new(2, 1)), Value::Text(_)));
    }
}
