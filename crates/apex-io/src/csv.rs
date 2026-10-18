//! CSV/TSV import and export with delimiter and locale detection.

use std::borrow::Cow;

use apex_sheet::format::parse_input;
use apex_sheet::{CellRef, Content, Locale, MAX_COLS, MAX_ROWS, Value, Workbook};

/// Splits CSV text into records, handling quotes and embedded newlines.
pub fn parse_records(text: &str, delim: u8) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    for_each_record(text, delim, |rec| {
        rows.push(rec.iter().map(|f| f.to_string()).collect());
        true
    });
    rows
}

/// Calls `f` with the fields of each record in turn, until it returns
/// false. Fields without quotes are borrowed from `text`, so a big file is
/// read without a copy of all its fields.
pub fn for_each_record<'a>(text: &'a str, delim: u8, f: impl FnMut(&[Cow<'a, str>]) -> bool) {
    records(text, delim, true, f);
}

/// [`for_each_record`] on a piece of a file: unless it is the `last` one,
/// its unfinished record is left for the next piece. Returns where that
/// record starts, or `None` when `f` stopped.
fn records<'a>(text: &'a str, delim: u8, last: bool, mut f: impl FnMut(&[Cow<'a, str>]) -> bool) -> Option<usize> {
    let mut row: Vec<Cow<'a, str>> = Vec::new();
    // Pieces of a field with quotes; a field without is `text[start..i]`.
    let mut field = String::new();
    let mut in_quotes = false;
    let b = text.as_bytes();
    let mut i = 0;
    let mut start = 0;
    let mut record = 0;
    // How far the field is known to be blank, and whether it isn't: each
    // byte is looked at once (a field of spaces and quotes would be again
    // at every quote).
    let (mut ws_from, mut inked) = (0, false);
    let take = |field: &mut String, piece: &'a str| -> Cow<'a, str> {
        if field.is_empty() {
            Cow::Borrowed(piece)
        } else {
            field.push_str(piece);
            Cow::Owned(std::mem::take(field))
        }
    };
    while i < b.len() {
        let c = b[i];
        if in_quotes {
            if c == b'"' {
                if b.get(i + 1) == Some(&b'"') {
                    field.push_str(&text[start..i + 1]);
                    i += 2;
                    start = i;
                    continue;
                }
                field.push_str(&text[start..i]);
                in_quotes = false;
                i += 1;
                start = i;
                continue;
            }
            i += 1;
            continue;
        }
        if c == b'"' && field.is_empty() && {
            if ws_from < start {
                (ws_from, inked) = (start, false);
            }
            if !inked {
                inked = !text[ws_from..i].trim().is_empty();
                ws_from = i;
            }
            !inked
        } {
            in_quotes = true;
            i += 1;
            start = i;
            continue;
        }
        if c == delim || c == b'\n' || c == b'\r' {
            row.push(take(&mut field, &text[start..i]));
            if c != delim {
                if !f(&row) {
                    return None;
                }
                row.clear();
                if c == b'\r' && b.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
                record = i + 1;
            }
            i += 1;
            start = i;
            continue;
        }
        i += 1;
    }
    if !last {
        return Some(record);
    }
    let end = take(&mut field, &text[start..]);
    if !end.is_empty() || !row.is_empty() {
        row.push(end);
        if !f(&row) {
            return None;
        }
    }
    Some(text.len())
}

/// Picks the delimiter giving the most consistent field counts.
pub fn detect_delimiter(text: &str) -> u8 {
    let sample: String = text.lines().take(20).collect::<Vec<_>>().join("\n");
    let mut best = (b',', 0usize);
    for d in [b',', b';', b'\t', b'|'] {
        let recs = parse_records(&sample, d);
        if recs.is_empty() {
            continue;
        }
        let first = recs[0].len();
        if first < 2 {
            continue;
        }
        let consistent = recs.iter().filter(|r| r.len() == first).count();
        let score = consistent * 100 + first;
        if score > best.1 {
            best = (d, score);
        }
    }
    // One column of comma-decimal numbers ("2,5E-13", "5E-13"): the commas
    // split some lines in two, always into the halves of a number.
    let recs = parse_records(&sample, b',');
    let halves = |r: &Vec<String>| {
        let int = r[0].strip_prefix('-').unwrap_or(&r[0]);
        !int.is_empty() && int.bytes().all(|b| b.is_ascii_digit()) && apex_sheet::format::parse_number(&r.join(","), &Locale::ES).is_some()
    };
    if best.0 == b',' && recs.iter().any(|r| r.len() == 1) && recs.iter().any(|r| r.len() == 2) && recs.iter().all(|r| r.len() == 1 || (r.len() == 2 && halves(r))) {
        return b';';
    }
    best.0
}

pub fn read(data: &[u8], delim: Option<u8>) -> Workbook {
    // UTF-8 without a byte order mark (most files) is read in place.
    let text = match std::str::from_utf8(data) {
        Ok(s) if !s.starts_with('\u{feff}') => Cow::Borrowed(s),
        _ => Cow::Owned(crate::decode_text(data)),
    };
    let mut l = Loader::new(delim.unwrap_or_else(|| detect_delimiter(&text)));
    l.records(&text, true);
    l.finish()
}

/// Bytes read at a time by [`read_file`].
const PIECE: u64 = 8 << 20;

/// Reads a CSV file a piece at a time, so that a big one never is in
/// memory whole and the rows past the last one of a sheet are not even
/// read. `None` when the file is not UTF-8 (it is then read whole, see
/// [`read`]).
pub fn read_file(path: &std::path::Path, delim: Option<u8>) -> std::io::Result<Option<Workbook>> {
    read_pieces(apex_fs::File::open(path)?, delim, PIECE)
}

fn read_pieces(mut file: impl std::io::Read, delim: Option<u8>, piece: u64) -> std::io::Result<Option<Workbook>> {
    use std::io::Read;
    let mut buf: Vec<u8> = Vec::new();
    let mut loader: Option<Loader> = None;
    loop {
        let eof = (&mut file).take(piece).read_to_end(&mut buf)? == 0;
        // Pieces end after a line break: never inside a character.
        let cut = if eof {
            buf.len()
        } else {
            match buf.iter().rposition(|&b| b == b'\n') {
                Some(p) => p + 1,
                None => continue,
            }
        };
        let text = match std::str::from_utf8(&buf[..cut]) {
            Ok(t) if loader.is_some() || !t.starts_with('\u{feff}') => t,
            _ => return Ok(None),
        };
        let l = loader.get_or_insert_with(|| Loader::new(delim.unwrap_or_else(|| detect_delimiter(text))));
        match l.records(text, eof) {
            Some(used) if !eof => {
                buf.drain(..used);
            }
            _ => break,
        }
    }
    Ok(Some(loader.unwrap_or_else(|| Loader::new(delim.unwrap_or(b','))).finish()))
}

/// A workbook being filled with the records of a CSV file.
struct Loader {
    wb: Workbook,
    delim: u8,
    locale: Locale,
    formats: std::collections::HashMap<String, u32>,
    texts: apex_sheet::Interner,
    /// Next row.
    row: u32,
}

impl Loader {
    fn new(delim: u8) -> Loader {
        let mut wb = Workbook::empty();
        let locale = wb.locale;
        // Semicolon CSVs come from comma-decimal locales.
        let locale = if delim == b';' { Locale::ES } else if delim == b',' && locale.decimal == ',' { Locale::EN } else { locale };
        wb.sheets.push(apex_sheet::Sheet::new("Sheet1"));
        Loader { wb, delim, locale, formats: Default::default(), texts: Default::default(), row: 0 }
    }

    /// Loads the records of `text` (see [`records`]).
    fn records(&mut self, text: &str, last: bool) -> Option<usize> {
        records(text, self.delim, last, |rec| {
            // Rows past the last one of a sheet are not read (as in Excel).
            if self.row >= MAX_ROWS {
                return false;
            }
            self.record(rec);
            self.row += 1;
            true
        })
    }

    fn record(&mut self, rec: &[Cow<'_, str>]) {
        let (r, wb) = (self.row, &mut self.wb);
        for (c, f) in rec.iter().enumerate() {
            if c as u32 >= MAX_COLS || f.is_empty() {
                continue;
            }
            // A leading apostrophe is text in a file (as in Excel and
            // LibreOffice), not the "keep as text" mark of cell input.
            if f.starts_with('\'') {
                wb.load_cell(0, CellRef::new(r, c as u32), Content::Value(self.texts.text(f)), 0);
                continue;
            }
            let p = parse_input(f, &self.locale);
            let style = match &p.format {
                Some(code) => *self.formats.entry(code.clone()).or_insert_with(|| {
                    let s = apex_sheet::CellStyle { num_fmt: code.as_str().into(), ..Default::default() };
                    wb.styles.intern(s)
                }),
                None => 0,
            };
            let v = match p.value {
                Value::Empty => continue,
                v => self.texts.share(v),
            };
            wb.load_cell(0, CellRef::new(r, c as u32), Content::Value(v), style);
        }
    }

    fn finish(mut self) -> Workbook {
        self.wb.finish_load();
        self.wb
    }
}

/// Exports one sheet as displayed. Uses `;` when the locale has comma decimals.
pub fn write(wb: &Workbook, sheet: usize, delim: Option<char>) -> String {
    let delim = delim.unwrap_or(if wb.locale.decimal == ',' { ';' } else { ',' });
    let sh = &wb.sheets[sheet];
    let Some(ext) = sh.extent() else { return String::new() };
    // Cells by row, so that a lone cell far away doesn't walk the whole
    // rectangle up to it.
    let mut rows: std::collections::BTreeMap<u32, Vec<u32>> = Default::default();
    for (cr, _) in sh.iter() {
        rows.entry(cr.row).or_default().push(cr.col);
    }
    // Rows are padded to the same number of fields (as Excel and
    // LibreOffice do), unless that would make a huge file of separators.
    let pad = (ext.row as u64 + 1) * (ext.col as u64 + 1) <= 20_000_000;
    let mut out = String::new();
    let mut next = 0;
    for (&r, cols) in rows.iter_mut() {
        for _ in next..r {
            if pad {
                (0..ext.col).for_each(|_| out.push(delim));
            }
            out.push_str("\r\n");
        }
        next = r + 1;
        cols.sort_unstable();
        let last = if pad { ext.col } else { cols.last().copied().unwrap_or(0) };
        let mut cells = cols.iter().copied().peekable();
        for c in 0..=last {
            if c > 0 {
                out.push(delim);
            }
            if cells.next_if_eq(&c).is_none() {
                continue;
            }
            let cell = CellRef::new(r, c);
            let text = match sh.get(cell) {
                None => continue,
                Some(x) => match x.value() {
                    Value::Text(t) => guard_formula(t),
                    // General numbers with all their digits (15, as Excel
                    // writes them), not rounded to a column's width.
                    Value::Number(n) if wb.styles.format(x.style).is_general() => full_number(*n, wb.locale.decimal),
                    // As displayed: a number format can show text of its
                    // own (`"=cmd|..."0`), guarded as text is.
                    _ => guard_formula(&wb.display(sheet, cell).0),
                },
            };
            if text.contains([delim, '"', '\n', '\r']) || text.starts_with(' ') || text.ends_with(' ') {
                out.push('"');
                out.push_str(&text.replace('"', "\"\""));
                out.push('"');
            } else {
                out.push_str(&text);
            }
        }
        out.push_str("\r\n");
    }
    out
}

/// Text that another spreadsheet opening the file would take as a formula
/// (`=...`, `@...`, or `+`/`-` with a call or a link to another program,
/// as in `-2+cmd|' /c calc'!A1`) gets a leading apostrophe, so it stays
/// text there. Phone numbers ("+34 600 000 000") and signs stay as they are.
/// `t` with a `'` before it when a spreadsheet would take it as a formula.
pub fn guard_formula(t: &str) -> String {
    // Tabs and returns before it are skipped by some programs.
    let first = t.trim_start_matches(['\t', '\r']);
    let risky = match first.chars().next() {
        Some('=' | '@') => true,
        Some('+' | '-') => first.contains(['(', '|', '!']),
        _ => false,
    };
    if risky { format!("'{t}") } else { t.to_string() }
}

/// A number with up to 15 significant digits (in scientific notation when
/// very large or small).
fn full_number(n: f64, decimal: char) -> String {
    let n = format!("{:.14e}", n).parse::<f64>().unwrap_or(n);
    if n != 0.0 && !(1e-15..1e15).contains(&n.abs()) {
        let s = format!("{:E}", n).replace('.', &decimal.to_string());
        return if s.contains("E-") { s } else { s.replace('E', "E+") };
    }
    apex_sheet::format::edit_number(n, decimal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatted_numbers_are_guarded_too() {
        let mut wb = Workbook::new();
        let style = wb.styles.intern(apex_sheet::sheet::CellStyle { num_fmt: std::sync::Arc::from("\"=cmd|' /c calc'!A\"0"), ..Default::default() });
        wb.load_cell(0, CellRef::new(0, 0), Content::Value(Value::Number(1.0)), style);
        let out = write(&wb, 0, Some(','));
        assert!(out.starts_with("'="), "{out}");
        assert_eq!(guard_formula("\t=1+2"), "'\t=1+2");
        assert_eq!(guard_formula("\r@SUM(A1)"), "'\r@SUM(A1)");
    }

    #[test]
    fn text_that_would_run_as_a_formula() {
        assert_eq!(guard_formula("=HYPERLINK(\"http://x\")"), "'=HYPERLINK(\"http://x\")");
        assert_eq!(guard_formula("-2+3+cmd|' /C calc'!A0"), "'-2+3+cmd|' /C calc'!A0");
        assert_eq!(guard_formula("@SUM(A1)"), "'@SUM(A1)");
        assert_eq!(guard_formula("+34 600 000 000"), "+34 600 000 000");
        assert_eq!(guard_formula("-"), "-");
        assert_eq!(guard_formula("Ana"), "Ana");
    }

    #[test]
    fn records() {
        let r = parse_records("a,\"b,c\",\"d\"\"e\"\n1,2\r\n\"x\ny\",z", b',');
        assert_eq!(r, vec![vec!["a", "b,c", "d\"e"], vec!["1", "2"], vec!["x\ny", "z"]]);
        assert_eq!(detect_delimiter("a;b;c\n1;2,5;3\n"), b';');
        assert_eq!(detect_delimiter("a,b\n1,2\n"), b',');
        assert_eq!(detect_delimiter("a\tb\n1\t2\n"), b'\t');
    }

    /// Read in small pieces (records and quotes across their ends), a
    /// file gives the same cells as read whole.
    #[test]
    fn pieces() {
        let mut text = String::from("name,qty,note\r\n");
        for i in 0..300 {
            text.push_str(&format!("item {i},{},\"line one\nline \"\"{i}\"\" two, more\"\r\n", i * 3));
            if i % 50 == 0 {
                text.push_str("\r\n'007,,ü\n");
            }
        }
        text.push_str("last,1");
        let whole = read(text.as_bytes(), None);
        for piece in [1, 7, 64, 1000] {
            let wb = read_pieces(std::io::Cursor::new(text.as_bytes()), None, piece).unwrap().unwrap();
            let cells = |wb: &Workbook| wb.sheets[0].iter().map(|(c, x)| (c, x.content.clone(), x.style)).collect::<Vec<_>>();
            assert_eq!(cells(&wb), cells(&whole), "pieces of {piece}");
        }
        assert_eq!(whole.sheets[0].cell_count(), 3 + 300 * 3 + 6 * 2 + 2);
        // Not UTF-8: read whole instead.
        assert!(read_pieces(std::io::Cursor::new(b"a,\xe9\n".to_vec()), None, 2).unwrap().is_none());
    }

    #[test]
    fn semicolon_decimals() {
        let wb = read("nombre;importe\nPan;1,50\nLeche;1.234,5\n".as_bytes(), None);
        assert_eq!(wb.sheets[0].value(CellRef::new(1, 1)), &Value::Number(1.5));
        assert_eq!(wb.sheets[0].value(CellRef::new(2, 1)), &Value::Number(1234.5));
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 0)), &Value::text("nombre"));
    }

    #[test]
    fn numbers_and_apostrophes_round_trip() {
        // One column of numbers saved with comma decimals, all digits kept.
        for loc in [Locale::ES, Locale::EN] {
            let mut wb = Workbook::new();
            wb.locale = loc;
            let nums = [2.5e-13, 5e-13, 7.5e-13, 16.9444444444444, -57.5412415342, 1e20, 0.1 + 0.2];
            for (r, &n) in nums.iter().enumerate() {
                wb.load_cell(0, CellRef::new(r as u32, 0), Content::Value(Value::Number(n)), 0);
            }
            let text = write(&wb, 0, None);
            let back = read(text.as_bytes(), None);
            for (r, &n) in nums.iter().enumerate() {
                let want = if r == 6 { 0.3 } else { n };
                assert_eq!(back.sheets[0].value(CellRef::new(r as u32, 0)), &Value::Number(want), "{text}");
            }
            assert!(back.sheets[0].get(CellRef::new(0, 1)).is_none());
        }
        let wb = read("a,'\nb,''\nc,'3\n".as_bytes(), None);
        for (r, t) in ["'", "''", "'3"].iter().enumerate() {
            assert_eq!(wb.sheets[0].value(CellRef::new(r as u32, 1)), &Value::text(*t));
        }
    }

    #[test]
    fn far_cell() {
        let mut wb = Workbook::new();
        wb.set_input(0, CellRef::new(0, 1), "a");
        wb.set_input(0, CellRef::new(2, 0), "b,c");
        assert_eq!(write(&wb, 0, Some(',')), ",a\r\n,\r\n\"b,c\",\r\n");
        // A lone cell at the end of the sheet: no rectangle of separators.
        wb.set_input(0, CellRef::new(MAX_ROWS - 1, MAX_COLS - 1), "z");
        let out = write(&wb, 0, Some(','));
        assert!(out.len() < 3 * MAX_ROWS as usize, "{}", out.len());
        assert!(out.starts_with(",a\r\n\r\n\"b,c\"\r\n\r\n"));
        assert!(out.ends_with(&format!("{}z\r\n", ",".repeat(MAX_COLS as usize - 1))));
    }
}
