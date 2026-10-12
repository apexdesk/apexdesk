//! Excel 2003 XML spreadsheets (SpreadsheetML), which Excel and other
//! programs also save with the .xls extension: sheets, cell values,
//! formulas (in R1C1 style), number formats and merged cells.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use apex_sheet::format::date_to_serial;
use apex_sheet::value::ErrorKind;
use apex_sheet::{CellRef, CellStyle, Content, Locale, Range, Sheet, Value, Workbook};

use crate::Error;
use crate::xml::{Ev, Reader};

const NS: &str = "urn:schemas-microsoft-com:office:spreadsheet";

/// Whether `data` is a SpreadsheetML document.
pub(crate) fn is_xml(data: &[u8]) -> bool {
    let head = &data[..data.len().min(4096)];
    let text = String::from_utf8_lossy(head);
    let t = text.trim_start_matches('\u{feff}').trim_start();
    t.starts_with('<') && text.contains(NS)
}

/// The code of a named format of SpreadsheetML, or the code itself.
fn format_code(f: &str) -> &str {
    match f {
        "General" | "General Number" => "General",
        "Fixed" => "0.00",
        "Standard" => "#,##0.00",
        "Percent" => "0.00%",
        "Scientific" => "0.00E+00",
        "Short Date" => "m/d/yyyy",
        "Medium Date" => "d-mmm-yy",
        "Long Date" => "dddd, mmmm d, yyyy",
        "Short Time" => "h:mm",
        "Medium Time" => "h:mm AM/PM",
        "Long Time" => "h:mm:ss",
        "Currency" => "\"$\"#,##0.00",
        f => f,
    }
}

/// `2012-01-31T10:30:00.000` as a date serial.
fn date_time(s: &str) -> Option<f64> {
    let (date, time) = s.split_once('T').unwrap_or((s, ""));
    let mut d = date.splitn(3, '-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut serial = if y <= 1899 && m == 12 && day == 31 { 0.0 } else { date_to_serial(y, m, day) };
    if !time.is_empty() {
        let mut t = time.splitn(3, ':').map(|p| p.parse::<f64>().ok());
        let (h, mi, sec) = (t.next().flatten().unwrap_or(0.0), t.next().flatten().unwrap_or(0.0), t.next().flatten().unwrap_or(0.0));
        serial += (h * 3600.0 + mi * 60.0 + sec) / 86400.0;
    }
    Some(serial)
}

fn index(t: &crate::xml::Tag, name: &str) -> Option<u32> {
    t.attr(name).and_then(|v| v.trim().parse::<u32>().ok()).filter(|&i| i >= 1).map(|i| i - 1)
}

pub(crate) fn read(data: &[u8]) -> Result<Workbook, Error> {
    let text = String::from_utf8_lossy(data);
    let mut r = Reader::new(text.trim_start_matches('\u{feff}'));
    let mut wb = Workbook::empty();
    // Style id -> number format code, then -> our style.
    let mut formats: HashMap<String, String> = HashMap::new();
    let mut styles: HashMap<String, u32> = HashMap::new();
    let mut style_id = String::new();
    let (mut row, mut col) = (0u32, 0u32);
    // The cell being read: position, style, formula.
    let mut cell: Option<(CellRef, u32, Option<String>)> = None;
    // Sheet names taken, in lower case (looking each up among all the
    // sheets would take hours for thousands named alike).
    let mut taken: HashSet<String> = HashSet::new();
    let mut suffix: HashMap<String, u32> = HashMap::new();
    loop {
        let (t, start) = match r.next() {
            Ev::Eof => break,
            Ev::Start(t) => (t, true),
            Ev::Empty(t) => (t, false),
            Ev::End(name) => {
                match crate::xml::local(name) {
                    "Row" => {
                        row += 1;
                        col = 0;
                    }
                    "Cell" => {
                        // A formula without a Data element.
                        if let Some((at, st, Some(f))) = cell.take() {
                            load_formula(&mut wb, at, st, &f, Value::Empty);
                        }
                    }
                    _ => {}
                }
                continue;
            }
            _ => continue,
        };
        match t.local() {
            "Style" => style_id = t.attr("ss:ID").map(|v| v.into_owned()).unwrap_or_default(),
            "NumberFormat" => {
                if let Some(f) = t.attr("ss:Format") {
                    formats.insert(style_id.clone(), format_code(&f).to_string());
                }
            }
            "Worksheet" => {
                let n = wb.sheets.len();
                // As many sheets as a binary workbook may have.
                if n >= 10_000 {
                    break;
                }
                // Excel's longest sheet name.
                let name: String = t.attr("ss:Name").map(|v| v.chars().take(31).collect()).filter(|v: &String| !v.is_empty()).unwrap_or_else(|| format!("Sheet{}", n + 1));
                let name = if taken.contains(&name.to_lowercase()) {
                    // Suffixes go on from the last one tried for the name.
                    let next = suffix.entry(name.to_lowercase()).or_insert(1);
                    let i = (*next..).find(|i| !taken.contains(&format!("{name}{i}").to_lowercase())).unwrap_or(*next);
                    *next = i + 1;
                    format!("{name}{i}")
                } else {
                    name
                };
                taken.insert(name.to_lowercase());
                wb.sheets.push(Sheet::new(name));
                row = 0;
                col = 0;
            }
            "Row" if !wb.sheets.is_empty() => {
                if let Some(i) = index(&t, "ss:Index") {
                    row = i;
                }
                col = 0;
                if !start {
                    row += 1;
                }
            }
            "Cell" if !wb.sheets.is_empty() => {
                if let Some(i) = index(&t, "ss:Index") {
                    col = i;
                }
                let at = CellRef::new(row, col);
                let across = t.attr("ss:MergeAcross").and_then(|v| v.parse::<u32>().ok()).unwrap_or(0).min(16383);
                let down = t.attr("ss:MergeDown").and_then(|v| v.parse::<u32>().ok()).unwrap_or(0).min(1 << 20);
                if across > 0 || down > 0 {
                    let si = wb.sheets.len() - 1;
                    wb.sheets[si].merges.push(Range::new(at, CellRef::new(row + down, col + across)));
                }
                let st = match t.attr("ss:StyleID") {
                    Some(id) => *styles.entry(id.to_string()).or_insert_with(|| match formats.get(id.as_ref()) {
                        Some(code) if code != "General" => wb.styles.intern(CellStyle { num_fmt: code.as_str().into(), ..Default::default() }),
                        _ => 0,
                    }),
                    None => 0,
                };
                let formula = t.attr("ss:Formula").map(|f| f.into_owned());
                col += 1 + across;
                if start {
                    cell = Some((at, st, formula));
                } else if let Some(f) = formula {
                    load_formula(&mut wb, at, st, &f, Value::Empty);
                } else if st != 0 {
                    let si = wb.sheets.len() - 1;
                    wb.load_cell(si, at, Content::Empty, st);
                }
            }
            // Notes are not read (their Data is not the cell's).
            "Comment" if start => r.skip(),
            "Data" if start => {
                let kind = t.attr("ss:Type").map(|v| v.into_owned()).unwrap_or_default();
                let s = r.text();
                let Some((at, st, formula)) = cell.take() else { continue };
                let value = match kind.as_str() {
                    "Number" => s.trim().parse::<f64>().map(Value::Number).unwrap_or_else(|_| Value::text(s)),
                    "Boolean" => Value::Bool(s.trim() == "1" || s.trim().eq_ignore_ascii_case("true")),
                    "DateTime" => date_time(s.trim()).map_or_else(|| Value::text(s.clone()), Value::Number),
                    "Error" => ErrorKind::parse(s.trim()).map_or_else(|| Value::text(s.clone()), Value::Error),
                    _ => Value::Text(Arc::from(s.as_str())),
                };
                match formula {
                    Some(f) => load_formula(&mut wb, at, st, &f, value),
                    None => {
                        let si = wb.sheets.len() - 1;
                        wb.load_cell(si, at, Content::Value(value), st);
                    }
                }
            }
            _ => {}
        }
    }
    if wb.sheets.is_empty() {
        return Err(Error::new("the spreadsheet has no worksheets"));
    }
    wb.finish_load();
    Ok(wb)
}

/// A formula in R1C1 style with its saved result (kept as a value when it
/// can't be read).
fn load_formula(wb: &mut Workbook, at: CellRef, style: u32, f: &str, value: Value) {
    let si = wb.sheets.len() - 1;
    let expr = apex_sheet::formula::r1c1_to_a1(f, at).and_then(|a1| apex_sheet::formula::parse(a1.trim_start_matches('='), &Locale::EN).ok());
    let content = match expr {
        Some(e) => Content::Formula { expr: e.into(), value },
        None => Content::Value(value),
    };
    wb.load_cell(si, at, content, style);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_formulas_formats_and_merges() {
        let doc = r#"<?xml version="1.0"?>
<?mso-application progid="Excel.Sheet"?>
<Workbook xmlns="urn:schemas-microsoft-com:office:spreadsheet" xmlns:ss="urn:schemas-microsoft-com:office:spreadsheet">
 <Styles><Style ss:ID="d"><NumberFormat ss:Format="Short Date"/></Style><Style ss:ID="p"><NumberFormat ss:Format="0.0%"/></Style></Styles>
 <Worksheet ss:Name="Data"><Table>
  <Row><Cell ss:MergeAcross="1"><Data ss:Type="String">Title <B>bold</B></Data></Cell><Cell><Data ss:Type="Boolean">1</Data></Cell></Row>
  <Row ss:Index="3"><Cell><Data ss:Type="Number">2</Data></Cell><Cell ss:Formula="=RC[-1]*3"><Data ss:Type="Number">6</Data></Cell>
   <Cell ss:Index="5" ss:StyleID="d"><Data ss:Type="DateTime">2012-01-31T12:00:00.000</Data></Cell><Cell ss:StyleID="p"><Data ss:Type="Number">0.25</Data></Cell></Row>
 </Table></Worksheet>
 <Worksheet ss:Name="Other"><Table><Row><Cell><Data ss:Type="Error">#DIV/0!</Data></Cell></Row></Table></Worksheet>
</Workbook>"#;
        assert!(is_xml(doc.as_bytes()));
        let wb = read(doc.as_bytes()).unwrap();
        let sh = &wb.sheets[0];
        assert_eq!((sh.name.as_str(), wb.sheets[1].name.as_str()), ("Data", "Other"));
        assert_eq!(sh.value(CellRef::new(0, 0)), &Value::text("Title bold"));
        assert_eq!(sh.merges, vec![Range::new(CellRef::new(0, 0), CellRef::new(0, 1))]);
        assert_eq!(sh.value(CellRef::new(0, 2)), &Value::Bool(true));
        let f = sh.get(CellRef::new(2, 1)).unwrap();
        assert_eq!(f.formula().unwrap().expr().to_canonical(), "A3*3");
        assert_eq!(f.value(), &Value::Number(6.0));
        let d = sh.get(CellRef::new(2, 4)).unwrap();
        assert_eq!((d.value(), &*wb.styles.get(d.style).num_fmt), (&Value::Number(40939.5), "m/d/yyyy"));
        let p = sh.get(CellRef::new(2, 5)).unwrap();
        assert_eq!(&*wb.styles.get(p.style).num_fmt, "0.0%");
        assert_eq!(wb.sheets[1].value(CellRef::new(0, 0)), &Value::Error(ErrorKind::Div0));
    }
}
