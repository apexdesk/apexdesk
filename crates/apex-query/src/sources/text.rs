//! Delimited text (CSV, TSV, semicolons, pipes…).

use crate::DataTable;

/// A table from delimited text: `skip` lines dropped first, the next one
/// naming the columns when `header`. Without a delimiter it is guessed.
pub fn parse(text: &str, delimiter: Option<char>, header: bool, skip: usize) -> DataTable {
    let mut text = text.strip_prefix('\u{feff}').unwrap_or(text);
    for _ in 0..skip {
        match text.find('\n') {
            Some(i) => text = &text[i + 1..],
            None => text = "",
        }
    }
    let rows: Vec<Vec<String>> = match delimiter {
        Some(d) if !d.is_ascii() => text.lines().filter(|l| !l.is_empty()).map(|l| l.split(d).map(str::to_string).collect()).collect(),
        Some(d) => apex_io::csv::parse_records(text, d as u8),
        None => apex_io::csv::parse_records(text, apex_io::csv::detect_delimiter(text)),
    };
    // Blank lines are not rows.
    let rows = rows.into_iter().filter(|r| !(r.len() == 1 && r[0].trim().is_empty())).collect();
    DataTable::from_text_rows(rows, header)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Val;

    #[test]
    fn csv_variants() {
        let t = parse("Name;Amount\r\n\"Doe; J\";1,5\r\n\r\nRoe;2\r\n", None, true, 0);
        assert_eq!(t.columns, ["Name", "Amount"]);
        assert_eq!(t.rows, vec![vec![Val::text("Doe; J"), Val::Num(1.5)], vec![Val::text("Roe"), Val::Num(2.0)]]);
        let t = parse("report\nx|y\n1|2\n", Some('|'), false, 1);
        assert_eq!(t.columns, ["Column1", "Column2"]);
        assert_eq!(t.rows.len(), 2);
        let t = parse("a¦b\n1¦2\n", Some('¦'), true, 0);
        assert_eq!(t.rows, vec![vec![Val::Num(1.0), Val::Num(2.0)]]);
    }
}
