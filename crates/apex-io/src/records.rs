//! A sheet read as a list of records (mail merge data sources): the first
//! row names the fields, each following non-empty row is a record, with
//! the values as the cells show them.

use apex_sheet::{CellRef, Workbook};

/// Field names and records of `sheet`, or `None` when it has no data.
/// Blank header cells are named by `unnamed(column number)`.
pub fn records(wb: &Workbook, sheet: usize, unnamed: impl Fn(usize) -> String) -> Option<(Vec<String>, Vec<Vec<String>>)> {
    let ext = wb.sheets.get(sheet)?.extent()?;
    let text = |r: u32, c: u32| wb.display(sheet, CellRef::new(r, c)).0.trim().to_string();
    let mut fields: Vec<String> = (0..=ext.col).map(|c| text(0, c)).collect();
    while fields.last().is_some_and(String::is_empty) {
        fields.pop();
    }
    if fields.is_empty() {
        return None;
    }
    for (i, f) in fields.iter_mut().enumerate() {
        if f.is_empty() {
            *f = unnamed(i + 1);
        }
    }
    let records = (1..=ext.row).map(|r| (0..fields.len() as u32).map(|c| text(r, c)).collect::<Vec<_>>()).filter(|rec| rec.iter().any(|v| !v.is_empty())).collect();
    Some((fields, records))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_and_rows() {
        let wb = crate::csv::read(b"Name,,City\nAna,1,Lugo\n,,\nLuis,2,Vigo\n", None);
        let (f, r) = records(&wb, 0, |n| format!("Column {n}")).unwrap();
        assert_eq!(f, ["Name", "Column 2", "City"]);
        assert_eq!(r, [["Ana", "1", "Lugo"], ["Luis", "2", "Vigo"]]);
        assert!(records(&Workbook::new(), 0, |_| String::new()).is_none());
    }
}
