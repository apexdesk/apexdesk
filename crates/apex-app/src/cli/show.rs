//! Cells as text for the command line: an aligned table for people, and
//! CSV, TSV, JSON and Markdown for other programs.

use apex_sheet::{CellRef, Range, Value, Workbook, addr::col_name, format::general_number};
use unicode_width::UnicodeWidthStr;

/// A value without number formats (numbers as they compute, "." decimal).
pub fn raw(v: &Value) -> String {
    match v {
        Value::Empty => String::new(),
        Value::Number(n) => general_number(*n, '.'),
        Value::Text(t) => t.to_string(),
        Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Value::Error(e) => e.as_str().to_string(),
        Value::Array(m) => m.data.first().map(raw).unwrap_or_default(),
        Value::Lambda(_) => "#CALC!".to_string(),
    }
}

/// What a cell shows: its formatted value, its raw value, or its formula.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Shown {
    Display,
    Raw,
    Formula,
}

/// The text of every cell of `r`, row by row.
pub fn grid(wb: &Workbook, sheet: usize, r: Range, shown: Shown) -> Vec<Vec<String>> {
    let sh = &wb.sheets[sheet];
    (r.start.row..=r.end.row)
        .map(|row| {
            (r.start.col..=r.end.col)
                .map(|col| {
                    let c = CellRef::new(row, col);
                    match shown {
                        Shown::Display => wb.display(sheet, c).0,
                        Shown::Raw => raw(sh.value(c)),
                        Shown::Formula => wb.edit_text(sheet, c),
                    }
                })
                .collect()
        })
        .collect()
}

/// Rows as an aligned table with column letters and row numbers; numbers
/// align right.
pub fn table(wb: &Workbook, sheet: usize, r: Range, rows: &[Vec<String>]) -> String {
    let sh = &wb.sheets[sheet];
    let ncols = (r.end.col - r.start.col + 1) as usize;
    let head_w = (r.end.row + 1).to_string().len();
    let mut widths: Vec<usize> = (0..ncols).map(|i| col_name(r.start.col + i as u32).len()).collect();
    for row in rows {
        for (i, t) in row.iter().enumerate() {
            widths[i] = widths[i].max(t.width()).min(60);
        }
    }
    let mut out = String::new();
    out.push_str(&" ".repeat(head_w));
    for (i, w) in widths.iter().enumerate() {
        out.push_str(&format!(" | {:<w$}", col_name(r.start.col + i as u32)));
    }
    out.push('\n');
    out.push_str(&"-".repeat(head_w));
    for w in &widths {
        out.push_str("-+-");
        out.push_str(&"-".repeat(*w));
    }
    out.push('\n');
    for (k, row) in rows.iter().enumerate() {
        let rn = r.start.row + k as u32;
        out.push_str(&format!("{:>head_w$}", rn + 1));
        for (i, t) in row.iter().enumerate() {
            let t = clip(t, widths[i]);
            let pad = " ".repeat(widths[i] - t.width());
            let right = matches!(sh.value(CellRef::new(rn, r.start.col + i as u32)), Value::Number(_));
            if right {
                out.push_str(&format!(" | {pad}{t}"));
            } else {
                out.push_str(&format!(" | {t}{pad}"));
            }
        }
        let trimmed = out.trim_end_matches(' ').len();
        out.truncate(trimmed);
        out.push('\n');
    }
    out
}

/// `t` cut to `w` columns (with an ellipsis).
fn clip(t: &str, w: usize) -> String {
    if t.width() <= w {
        return t.replace('\n', " ");
    }
    let mut s = String::new();
    for ch in t.chars() {
        let ch = if ch == '\n' { ' ' } else { ch };
        if s.width() + unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0) + 1 > w {
            break;
        }
        s.push(ch);
    }
    s.push('…');
    s
}

/// CSV (RFC 4180 quoting) or TSV rows; text a spreadsheet would take for
/// a formula is kept as text.
pub fn delimited(rows: &[Vec<String>], delim: char) -> String {
    let mut out = String::new();
    for row in rows {
        let cells: Vec<String> = row
            .iter()
            .map(|t| {
                let t = &apex_io::csv::guard_formula(t);
                if t.contains([delim, '"', '\n', '\r']) {
                    format!("\"{}\"", t.replace('"', "\"\""))
                } else {
                    t.to_string()
                }
            })
            .collect();
        out.push_str(&cells.join(&delim.to_string()));
        out.push('\n');
    }
    out
}

/// A Markdown table; the first row is the header.
pub fn markdown(rows: &[Vec<String>]) -> String {
    let Some(first) = rows.first() else { return String::new() };
    let cell = |t: &str| t.replace('|', "\\|").replace('\n', "<br>");
    let line = |row: &Vec<String>| format!("| {} |\n", row.iter().map(|t| cell(t)).collect::<Vec<_>>().join(" | "));
    let mut out = line(first);
    out.push_str(&format!("|{}\n", "---|".repeat(first.len())));
    for row in &rows[1..] {
        out.push_str(&line(row));
    }
    out
}

/// A JSON string literal.
pub fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A value as JSON: numbers and booleans typed, empty cells null.
pub fn json_value(v: &Value) -> String {
    match v {
        Value::Empty => "null".into(),
        Value::Number(n) if n.is_finite() => general_number(*n, '.').replace('E', "e"),
        Value::Bool(b) => b.to_string(),
        v => json_str(&raw(v)),
    }
}

/// Rows of typed values as a JSON array of arrays.
pub fn json(wb: &Workbook, sheet: usize, r: Range) -> String {
    let sh = &wb.sheets[sheet];
    let rows: Vec<String> = (r.start.row..=r.end.row)
        .map(|row| {
            let cells: Vec<String> = (r.start.col..=r.end.col).map(|col| json_value(sh.value(CellRef::new(row, col)))).collect();
            format!("  [{}]", cells.join(", "))
        })
        .collect();
    format!("[\n{}\n]\n", rows.join(",\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        let rows = vec![vec!["a,b".to_string(), "say \"hi\"".to_string()], vec!["1".into(), "x|y".into()]];
        assert_eq!(delimited(&rows, ','), "\"a,b\",\"say \"\"hi\"\"\"\n1,x|y\n");
        assert_eq!(delimited(&rows, '\t'), "a,b\t\"say \"\"hi\"\"\"\n1\tx|y\n");
        assert_eq!(markdown(&rows), "| a,b | say \"hi\" |\n|---|---|\n| 1 | x\\|y |\n");
        assert_eq!(json_str("a\"\n"), "\"a\\\"\\n\"");
        assert_eq!(json_value(&Value::Number(2.5)), "2.5");
        assert_eq!(raw(&Value::Number(3.0)), "3");
    }
}
