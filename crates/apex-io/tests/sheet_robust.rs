//! Spreadsheet files that are odd, damaged or built to use up memory: they
//! open (or fail cleanly) and save in reasonable time and size.

use std::sync::Arc;

use apex_io::zip::{ZipReader, ZipWriter};
use apex_sheet::{CellRef, Content, Value, Workbook};

/// A copy of a package with each part passed through `f`.
fn rezip(bytes: &[u8], f: impl Fn(&str, Vec<u8>) -> Vec<u8>) -> Vec<u8> {
    let z = ZipReader::new(bytes).unwrap();
    let mut w = ZipWriter::new();
    for name in z.names() {
        let data = f(name, z.read(name).unwrap());
        w.add(name, &data, name != "mimetype");
    }
    w.finish()
}

#[test]
fn one_long_text_in_many_cells() {
    // Like POI's poc-shared-strings.xlsx: one shared string of 1 MB in
    // thousands of cells. The .ods repeats the rows instead of writing
    // the text again for each one.
    let text: Arc<str> = "x".repeat(1 << 20).into();
    let mut wb = Workbook::new();
    for r in 0..3000 {
        for c in 0..3 {
            wb.load_cell(0, CellRef::new(r, c), Content::Value(Value::Text(text.clone())), 0);
        }
    }
    wb.load_cell(0, CellRef::new(3000, 0), Content::Value(Value::Number(1.0)), 0);
    wb.finish_load();
    let ods = apex_io::ods::write(&wb);
    assert!(ods.len() < 1 << 20, "{}", ods.len());
    let back = apex_io::ods::read(&ods).unwrap();
    assert!(back.sheets[0].value(CellRef::new(2999, 2)) == &Value::Text(text.clone()));
    assert_eq!(back.sheets[0].value(CellRef::new(3000, 0)), &Value::Number(1.0));
    assert!(back.sheets[0].get(CellRef::new(3001, 0)).is_none());
    let xlsx = apex_io::xlsx::write(&wb);
    assert!(xlsx.len() < 1 << 20, "{}", xlsx.len());
}

#[test]
fn ods_without_tables() {
    let mut wb = Workbook::new();
    wb.set_input(0, CellRef::new(0, 0), "x");
    let ods = apex_io::ods::write(&wb);
    let content = |name: &str, data: Vec<u8>| {
        if name != "content.xml" {
            return data;
        }
        let s = String::from_utf8(data).unwrap();
        let (a, rest) = s.split_once("<office:spreadsheet").unwrap();
        let (_, b) = rest.split_once("</office:spreadsheet>").unwrap();
        format!("{a}<office:spreadsheet>{{}}</office:spreadsheet>{b}").into_bytes()
    };
    // No tables at all: one empty sheet, as LibreOffice opens it.
    let empty = rezip(&ods, |n, d| String::from_utf8(content(n, d)).unwrap().replace("{}", "").into_bytes());
    let back = apex_io::ods::read(&empty).unwrap();
    assert_eq!(back.sheets.len(), 1);
    assert!(back.sheets[0].extent().is_none());
    // Tables written as empty elements.
    let tables = rezip(&ods, |n, d| String::from_utf8(content(n, d)).unwrap().replace("{}", r#"<table:table table:name="A"/><table:table table:name="B"/>"#).into_bytes());
    let back = apex_io::ods::read(&tables).unwrap();
    assert_eq!(back.sheets.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["A", "B"]);
}

#[test]
fn ods_blank_styled_runs() {
    // Old files have the rest of each row (to column 1024) formatted:
    // such runs of blank cells whose format shows nothing are left out, and
    // shorter ones past a budget of a million cells, but a fill or borders
    // on blank cells is kept.
    let mut wb = Workbook::new();
    wb.set_input(0, CellRef::new(0, 0), "x");
    let ods = apex_io::ods::write(&wb);
    let cells = r#"<table:table-row><table:table-cell office:value-type="string"><text:p>a</text:p></table:table-cell><table:table-cell table:style-name="bold" table:number-columns-repeated="999"/><table:table-cell table:style-name="bold" table:number-columns-repeated="1016"/></table:table-row><table:table-row table:number-rows-repeated="5000"><table:table-cell table:style-name="bold" table:number-columns-repeated="999"/></table:table-row><table:table-row><table:table-cell table:style-name="bold" table:number-columns-repeated="999"/></table:table-row><table:table-row><table:table-cell table:style-name="bold" table:number-columns-repeated="999"/></table:table-row><table:table-row><table:table-cell table:style-name="fill" table:number-columns-repeated="300"/></table:table-row>"#;
    let styles = r##"<style:style style:name="bold" style:family="table-cell"><style:text-properties fo:font-weight="bold"/></style:style><style:style style:name="fill" style:family="table-cell"><style:table-cell-properties fo:background-color="#ff0000"/></style:style>"##;
    let doc = rezip(&ods, |n, d| {
        if n != "content.xml" {
            return d;
        }
        let s = String::from_utf8(d).unwrap();
        let s = s.replacen("<office:automatic-styles>", &format!("<office:automatic-styles>{styles}"), 1);
        let (a, rest) = s.split_once("<table:table-row").unwrap();
        let (_, b) = rest.rsplit_once("</table:table-row>").unwrap();
        format!("{a}{cells}{b}").into_bytes()
    });
    let back = apex_io::ods::read(&doc).unwrap();
    let sh = &back.sheets[0];
    assert_eq!(sh.value(CellRef::new(0, 0)), &Value::text("a"));
    assert!(sh.get(CellRef::new(0, 1500)).is_none());
    assert!(sh.get(CellRef::new(0, 500)).is_some() && sh.get(CellRef::new(1000, 500)).is_some());
    assert!(sh.get(CellRef::new(5002, 500)).is_none());
    assert!(sh.iter().count() <= 1_100_000);
    let fill = sh.get(CellRef::new(5003, 299)).expect("filled blank cell");
    assert_eq!(back.styles.get(fill.style).fill, Some(0xff0000));
}

#[test]
fn ods_value_repeated_past_the_columns() {
    let mut wb = Workbook::new();
    wb.set_input(0, CellRef::new(0, 0), "x");
    let ods = apex_io::ods::write(&wb);
    let cells = r#"<table:table-row><table:table-cell office:value-type="float" office:value="1" table:number-columns-repeated="4294967295"><text:p>1</text:p></table:table-cell><table:table-cell office:value-type="float" office:value="2"><text:p>2</text:p></table:table-cell></table:table-row>"#;
    let doc = rezip(&ods, |n, d| {
        if n != "content.xml" {
            return d;
        }
        let s = String::from_utf8(d).unwrap();
        let (a, rest) = s.split_once("<table:table-row").unwrap();
        let (_, b) = rest.rsplit_once("</table:table-row>").unwrap();
        format!("{a}{cells}{b}").into_bytes()
    });
    let back = apex_io::ods::read(&doc).unwrap();
    let sh = &back.sheets[0];
    assert_eq!(sh.value(CellRef::new(0, apex_sheet::MAX_COLS - 1)), &Value::Number(1.0));
    // Past the last column, not wrapped round to the first.
    assert_eq!(sh.value(CellRef::new(0, 0)), &Value::Number(1.0));
    assert_eq!(sh.iter().count(), apex_sheet::MAX_COLS as usize);
}

#[test]
fn xlsx_sheet_listed_many_times() {
    let mut wb = Workbook::new();
    wb.set_input(0, CellRef::new(0, 0), "x");
    let xlsx = apex_io::xlsx::write(&wb);
    let doc = rezip(&xlsx, |n, d| {
        if n != "xl/workbook.xml" {
            return d;
        }
        let s = String::from_utf8(d).unwrap();
        let start = s.find("<sheet ").unwrap();
        let end = start + s[start..].find("/>").unwrap() + 2;
        let sheet = &s[start..end];
        format!("{}{}{}", &s[..end], sheet.repeat(50_000), &s[end..]).into_bytes()
    });
    let back = apex_io::xlsx::read(&doc).unwrap();
    assert_eq!(back.sheets.len(), 1);
    assert_eq!(back.sheets[0].value(CellRef::new(0, 0)), &Value::text("x"));
}

#[test]
fn cells_past_the_last_column() {
    let mut wb = Workbook::new();
    wb.load_cell(0, CellRef::new(0, 3_000_000_000), Content::Value(Value::Number(1.0)), 0);
    wb.load_cell(0, CellRef::new(u32::MAX, 0), Content::Value(Value::Number(1.0)), 0);
    assert!(wb.sheets[0].extent().is_none());
}

#[test]
fn names_of_several_areas_and_empty_array_items() {
    let mut wb = Workbook::new();
    wb.set_input(0, CellRef::new(0, 0), "x");
    let xlsx = apex_io::xlsx::write(&wb);
    let names = r#"<definedNames><definedName name="areas">Sheet1!$C$10:$C$14,Sheet1!$C$16:$C$18</definedName><definedName name="empty_array">{1,,3}</definedName></definedNames>"#;
    let doc = rezip(&xlsx, |n, d| {
        if n != "xl/workbook.xml" {
            return d;
        }
        let s = String::from_utf8(d).unwrap();
        let at = s.find("</sheets>").unwrap() + "</sheets>".len();
        format!("{}{names}{}", &s[..at], &s[at..]).into_bytes()
    });
    let wb = apex_io::xlsx::read(&doc).unwrap();
    let texts = |wb: &Workbook| wb.names.iter().map(|d| (d.name.clone(), d.expr.to_canonical())).collect::<Vec<_>>();
    let want = vec![("areas".to_string(), "(Sheet1!$C$10:$C$14,Sheet1!$C$16:$C$18)".to_string()), ("empty_array".to_string(), "{1,,3}".to_string())];
    assert_eq!(texts(&wb), want);
    assert_eq!(texts(&apex_io::ods::read(&apex_io::ods::write(&wb)).unwrap()), want);
    let again = apex_io::xlsx::write(&wb);
    let z = ZipReader::new(&again).unwrap();
    assert!(z.read_string("xl/workbook.xml").unwrap().contains(">Sheet1!$C$10:$C$14,Sheet1!$C$16:$C$18</definedName>"));
    assert_eq!(texts(&apex_io::xlsx::read(&again).unwrap()), want);
}

#[test]
fn schema_limits_on_save() {
    // A column wider than Excel allows is saved at 255 characters, the
    // iteration limit uses ODF's table:maximum-difference.
    let mut wb = Workbook::new();
    wb.sheets[0].col_widths.insert(0, 5000.0);
    wb.set_input(0, CellRef::new(0, 0), "x");
    wb.calc.iterate = true;
    wb.calc.max_change = 0.5;
    let xlsx = apex_io::xlsx::write(&wb);
    let sheet = ZipReader::new(&xlsx).unwrap().read_string("xl/worksheets/sheet1.xml").unwrap();
    assert!(sheet.contains(r#"width="255""#), "{sheet}");
    let ods = apex_io::ods::write(&wb);
    let content = ZipReader::new(&ods).unwrap().read_string("content.xml").unwrap();
    assert!(content.contains(r#"table:maximum-difference="0.5""#));
    let back = apex_io::ods::read(&ods).unwrap();
    assert!(back.calc.iterate && back.calc.max_change == 0.5);
}

fn biff(out: &mut Vec<u8>, id: u16, body: &[u8]) {
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&(body.len() as u16).to_le_bytes());
    out.extend_from_slice(body);
}

#[test]
fn bare_biff8_worksheet_and_chart_only_book() {
    // A worksheet stream on its own (no compound file, no globals) whose
    // BOF has version 0, as SheetJS writes (tdf144732.xls).
    let mut d = Vec::new();
    biff(&mut d, 0x0809, &[0, 0, 0x10, 0, 0, 0, 0, 0]);
    let mut n = vec![0, 0, 1, 0, 0, 0];
    n.extend_from_slice(&2.5f64.to_le_bytes());
    biff(&mut d, 0x0203, &n);
    biff(&mut d, 0x000A, &[]);
    let wb = apex_io::xls::read(&d).unwrap();
    assert_eq!(wb.sheets[0].value(CellRef::new(0, 1)), &Value::Number(2.5));
    // Globals listing only a chart sheet (64130.xls): one empty sheet.
    let mut d = Vec::new();
    biff(&mut d, 0x0809, &[0x80, 0x05, 0x05, 0x00, 0, 0, 0, 0]);
    biff(&mut d, 0x0085, &[0x40, 0, 0, 0, 0, 2, 1, b'C']);
    biff(&mut d, 0x000A, &[]);
    biff(&mut d, 0x0809, &[0x80, 0x05, 0x20, 0x00, 0, 0, 0, 0]);
    biff(&mut d, 0x000A, &[]);
    let wb = apex_io::xls::read(&d).unwrap();
    assert_eq!(wb.sheets.len(), 1);
}

#[test]
fn number_split_by_texts() {
    // `"000-"0000"-"000` is one ODF number with an embedded text (two
    // number elements in a style are invalid), and reads back as an
    // equivalent code.
    let mut wb = Workbook::new();
    for (r, code) in ["\"000-\"0000\"-\"000", "###\"-\"####", "0\" \"00.00", "0 ?/?"].iter().enumerate() {
        let style = wb.styles.intern(apex_sheet::CellStyle { num_fmt: (*code).into(), ..Default::default() });
        wb.load_cell(0, CellRef::new(r as u32, 0), Content::Value(Value::Number(1234567.0)), style);
    }
    wb.finish_load();
    let ods = apex_io::ods::write(&wb);
    let content = ZipReader::new(&ods).unwrap().read_string("content.xml").unwrap();
    let styles = ZipReader::new(&ods).unwrap().read_string("styles.xml").unwrap();
    assert!((content.clone() + &styles).contains(r#"<number:embedded-text number:position="3">-</number:embedded-text>"#));
    let back = apex_io::ods::read(&ods).unwrap();
    let got: Vec<String> = (0..4).map(|r| back.styles.get(back.sheets[0].get(CellRef::new(r, 0)).unwrap().style).num_fmt.to_string()).collect();
    assert_eq!(got, ["\"000\"-0000-000", "#-####", "0 00.00", "0 ?/?"]);
}

#[test]
fn long_run_of_equal_blank_formatted_rows() {
    // More equal blank formatted rows than the reader repeats one row
    // element: they are written as several elements and all read back.
    let mut wb = Workbook::new();
    let fill = wb.styles.intern(apex_sheet::CellStyle { fill: Some(0xff0000), ..Default::default() });
    for r in 0..1500 {
        wb.load_cell(0, CellRef::new(r, 1), Content::Empty, fill);
    }
    wb.finish_load();
    let back = apex_io::ods::read(&apex_io::ods::write(&wb)).unwrap();
    let cell = back.sheets[0].get(CellRef::new(1499, 1)).expect("last formatted row");
    assert_eq!(back.styles.get(cell.style).fill, Some(0xff0000));
}

#[test]
fn deeply_nested_cell_text() {
    // A fuzzed cell with a hundred thousand nested paragraphs reads in
    // linear time (the style of each text run isn't searched up the stack).
    let mut wb = Workbook::new();
    wb.set_input(0, CellRef::new(0, 0), "x");
    let ods = apex_io::ods::write(&wb);
    let n = 100_000;
    let deep = format!("<text:p>{}y{}</text:p>", "<text:p>".repeat(n), "</text:p>".repeat(n));
    let doc = rezip(&ods, |name, d| if name == "content.xml" { String::from_utf8(d).unwrap().replacen("<text:p>x</text:p>", &deep, 1).into_bytes() } else { d });
    let t = std::time::Instant::now();
    let back = apex_io::ods::read(&doc).unwrap();
    assert!(t.elapsed().as_secs() < 10);
    assert!(matches!(back.sheets[0].value(CellRef::new(0, 0)), Value::Text(s) if s.ends_with('y')));
}

/// A cell of the default size keeps it in ods when the Default style is
/// larger (it isn't left to inherit Default's size).
#[test]
fn ods_default_size_under_a_larger_default_style() {
    let mut wb = Workbook::new();
    let mut n = wb.cell_styles.normal().unwrap().clone();
    n.style.font_size = 120;
    wb.set_cell_style(Some("Normal"), n);
    wb.set_input(0, CellRef::new(0, 0), "x");
    let id = wb.styles.intern(apex_sheet::CellStyle { bold: true, ..Default::default() });
    wb.sheets[0].get_mut(CellRef::new(0, 0)).unwrap().style = id;
    for _ in 0..2 {
        wb = apex_io::ods::read(&apex_io::ods::write(&wb)).unwrap();
        let st = wb.styles.get(wb.sheets[0].get(CellRef::new(0, 0)).unwrap().style);
        assert!(st.bold && st.font_size == 0, "{st:?}");
    }
}
