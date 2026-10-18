//! Tables (list objects) in xlsx: `xl/tables/tableN.xml` parts linked
//! from their worksheets with `<tableParts>`.

use std::fmt::Write as _;

use apex_doc::kept::TableQuery;
use apex_sheet::data::CfStyle;
use apex_sheet::table::{Table, TableColumn, TableStyle, TotalFn};
use apex_sheet::{Range, Sheet, Workbook};

use crate::xml::{Ev, HEADER, Reader, esc};
use crate::zip::ZipReader;

pub const REL_TABLE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/table";
pub const CT_TABLE: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.table+xml";

/// Reads the tables of a worksheet (`xml`, at `path` in the package).
/// `had` is the tables and columns the book has already: a book keeps at
/// most 1024 tables of 1 << 18 columns in all.
pub fn read_tables(z: &ZipReader, path: &str, xml: &str, sh: &mut Sheet, dxfs: &[CfStyle], had: (usize, usize)) {
    let (mut tables, mut cols) = had;
    let tail = xml.find("</sheetData>").map_or(xml, |i| &xml[i..]);
    if !tail.contains("tablePart") {
        return;
    }
    let mut ids = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut r = Reader::new(tail);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "tablePart" => {
                // A sheet's worth of tables, each read once.
                if let Some(id) = t.attr("r:id").or_else(|| t.attr("id")).filter(|id| ids.len() < 1024 && !seen.contains(id.as_ref())) {
                    seen.insert(id.to_string());
                    ids.push(id.into_owned());
                }
            }
            _ => {}
        }
    }
    let (dir, file) = path.rsplit_once('/').unwrap_or(("", path));
    let rels = z.read_string(&format!("{dir}/_rels/{file}.rels")).map(|s| crate::xlsx_notes::read_rels(&s)).unwrap_or_default();
    for id in ids {
        let Some((_, target, false)) = rels.get(&id) else { continue };
        let Ok(txml) = z.read_string(&crate::xlsx::resolve_target(dir, target)) else { continue };
        if tables >= 1024 {
            break;
        }
        if let Some((t, filter)) = parse_table(&txml, dxfs) {
            // Tables never overlap.
            if sh.tables.iter().any(|o| o.range.intersect(&t.range).is_some()) || t.columns.len() as u32 != t.range.cols() || cols + t.columns.len() > 1 << 18 {
                continue;
            }
            tables += 1;
            cols += t.columns.len();
            if let Some(f) = filter {
                if sh.filter.is_none() {
                    sh.filter = Some(f);
                }
            }
            sh.tables.push(t);
        }
    }
}

fn parse_table(xml: &str, dxfs: &[CfStyle]) -> Option<(Table, Option<apex_sheet::data::AutoFilter>)> {
    let mut r = Reader::new(xml);
    let mut table: Option<Table> = None;
    let mut filter_xml: Option<&str> = None;
    // (The part is searched for it once: it may repeat.)
    let mut searched = false;
    let mut col: Option<TableColumn> = None;
    loop {
        match r.next() {
            Ev::Eof => break,
            // `x14:table` in the extLst is not the table.
            Ev::Start(t) | Ev::Empty(t) if t.local() == "table" && table.is_none() => {
                let range = Range::parse(&t.attr("ref")?)?;
                let name = t.attr("displayName").or_else(|| t.attr("name"))?.into_owned();
                let id = t.attr("id").and_then(|v| v.parse().ok()).unwrap_or(1);
                let mut tb = Table::new(id, name, range, Vec::new());
                tb.header = t.attr("headerRowCount").is_none_or(|v| v != "0");
                tb.totals = t.attr("totalsRowCount").is_some_and(|v| v != "0");
                tb.filter_button = false;
                table = Some(tb);
            }
            Ev::Start(t) | Ev::Empty(t) if t.local() == "autoFilter" => {
                if let Some(tb) = &mut table {
                    tb.filter_button = true;
                }
                // Parsed below with the sheet filter reader.
                if searched {
                    continue;
                }
                searched = true;
                let at = xml.find("<autoFilter").or_else(|| xml.find(":autoFilter"));
                let end = xml.find("</autoFilter>").map(|e| e + "</autoFilter>".len());
                filter_xml = match (at, end) {
                    (Some(a), Some(e)) if e > a => Some(&xml[a..e]),
                    (Some(a), _) => xml[a..].find("/>").map(|e| &xml[a..a + e + 2]),
                    _ => None,
                };
            }
            Ev::Start(t) | Ev::Empty(t) if t.local() == "tableColumn" => {
                let mut c = TableColumn::new(t.attr("name").map(|v| unescape_x(&v)).unwrap_or_default());
                if let Some(f) = t.attr("totalsRowFunction") {
                    c.total = TotalFn::from_xlsx(&f);
                }
                c.total_label = t.attr("totalsRowLabel").map(|v| v.into_owned());
                // A sheet is 16384 columns wide.
                if let Some(tb) = table.as_mut().filter(|tb| tb.columns.len() < 16384) {
                    tb.columns.push(c.clone());
                }
                col = Some(c);
            }
            Ev::Start(t) if t.local() == "calculatedColumnFormula" || t.local() == "totalsRowFormula" => {
                let calc = t.local() == "calculatedColumnFormula";
                let text = r.text();
                if let (Some(tb), Some(_)) = (&mut table, &col) {
                    let canon = apex_sheet::formula::parse_canonical(text.trim()).ok().map(|e| apex_sheet::workbook::unqualified(&e, &tb.name).to_canonical());
                    if let Some(last) = tb.columns.last_mut() {
                        if calc {
                            last.formula = canon;
                        } else {
                            last.total_formula = canon;
                        }
                    }
                }
            }
            Ev::End(n) if crate::xml::local(n) == "tableColumn" => col = None,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "tableStyleInfo" => {
                if let Some(tb) = &mut table {
                    let on = |k: &str| t.attr(k).is_some_and(|v| v == "1" || v == "true");
                    tb.style = TableStyle { name: t.attr("name").map(|v| v.into_owned()).unwrap_or_default(), row_stripes: on("showRowStripes"), col_stripes: on("showColumnStripes"), first_col: on("showFirstColumn"), last_col: on("showLastColumn") };
                }
            }
            _ => {}
        }
    }
    let mut t = table?;
    if !t.header {
        t.filter_button = false;
    }
    let filter = filter_xml.filter(|_| t.filter_button).and_then(|f| {
        let mut tmp = Sheet::new("t");
        crate::xlsx_data::read_tools(&format!("<sheetData/>{f}"), &mut tmp, dxfs);
        tmp.filter
    });
    Some((t, filter))
}

/// `_x000a_` escapes in column names.
pub(crate) fn unescape_x(s: &str) -> String {
    if !s.contains("_x") {
        return s.to_string();
    }
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("_x") {
        out.push_str(&rest[..i]);
        let code = rest.get(i + 2..i + 6).and_then(|h| u32::from_str_radix(h, 16).ok()).and_then(char::from_u32);
        match code {
            Some(c) if rest.get(i + 6..i + 7) == Some("_") => {
                out.push(c);
                rest = &rest[i + 7..];
            }
            _ => {
                out.push_str("_x");
                rest = &rest[i + 2..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Whether the sheet's filter is the filter of one of its tables (it is
/// then written inside the table, not on the sheet).
pub fn filter_in_table(sh: &Sheet) -> bool {
    sh.filter.as_ref().is_some_and(|f| sh.tables.iter().any(|t| t.filter_button && t.filter_range() == f.range))
}

/// The table parts of every sheet: for each sheet, `(file number, xml)`
/// for its tables. Numbers run over the workbook.
/// `queries` are the kept queries of the tables that still fit them.
pub fn write_tables(wb: &Workbook, dxfs: &[CfStyle], queries: &[Vec<Option<std::sync::Arc<TableQuery>>>]) -> Vec<Vec<(usize, String)>> {
    let mut n = 0;
    wb.sheets
        .iter()
        .enumerate()
        .map(|(si, sh)| {
            sh.tables
                .iter()
                .enumerate()
                .map(|(ti, t)| {
                    n += 1;
                    let q = queries.get(si).and_then(|v| v.get(ti)).and_then(|q| q.as_deref());
                    (n, table_xml(sh, t, n, dxfs, q))
                })
                .collect()
        })
        .collect()
}

fn table_xml(sh: &Sheet, t: &Table, n: usize, dxfs: &[CfStyle], query: Option<&TableQuery>) -> String {
    let mut s = String::from(HEADER);
    let name = esc(&t.name);
    let _ = write!(s, r#"<table xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" id="{n}" name="{name}" displayName="{name}" ref="{}""#, t.range.to_a1());
    if query.is_some() {
        s.push_str(r#" tableType="queryTable""#);
    }
    if !t.header {
        s.push_str(r#" headerRowCount="0""#);
    }
    if t.totals {
        s.push_str(r#" totalsRowCount="1""#);
    } else {
        s.push_str(r#" totalsRowShown="0""#);
    }
    s.push('>');
    if t.filter_button && t.header {
        match sh.filter.as_ref().filter(|f| f.range == t.filter_range()) {
            Some(f) => crate::xlsx_data::write_filter(&mut s, f, dxfs),
            None => {
                let _ = write!(s, r#"<autoFilter ref="{}"/>"#, t.filter_range().to_a1());
            }
        }
    }
    let _ = write!(s, r#"<tableColumns count="{}">"#, t.columns.len());
    for (i, c) in t.columns.iter().enumerate() {
        // A query's fields point at the columns by id.
        let (id, extra) = query.and_then(|q| q.columns.get(i)).map_or((i as u32 + 1, ""), |q| (q.1, q.2.as_str()));
        let _ = write!(s, r#"<tableColumn id="{id}"{extra} name="{}""#, esc(&c.name));
        if t.totals {
            if c.total != TotalFn::None {
                let _ = write!(s, r#" totalsRowFunction="{}""#, c.total.xlsx());
            } else if let Some(l) = &c.total_label {
                let _ = write!(s, r#" totalsRowLabel="{}""#, esc(l));
            }
        }
        let calc = c.formula.as_deref().and_then(|f| Workbook::table_formula_for_xlsx(t, f));
        let total = if c.total == TotalFn::Custom { c.total_formula.as_deref().and_then(|f| Workbook::table_formula_for_xlsx(t, f)) } else { None };
        if calc.is_none() && total.is_none() {
            s.push_str("/>");
            continue;
        }
        s.push('>');
        if let Some(f) = calc {
            let _ = write!(s, "<calculatedColumnFormula>{}</calculatedColumnFormula>", esc(&f));
        }
        if let Some(f) = total {
            let _ = write!(s, "<totalsRowFormula>{}</totalsRowFormula>", esc(&f));
        }
        s.push_str("</tableColumn>");
    }
    s.push_str("</tableColumns>");
    // Custom styles are not kept: only built-in names are written.
    if apex_sheet::table::parse_style_name(&t.style.name).is_some() {
        let b = |v: bool| v as u8;
        let st = &t.style;
        let _ = write!(s, r#"<tableStyleInfo name="{}" showFirstColumn="{}" showLastColumn="{}" showRowStripes="{}" showColumnStripes="{}"/>"#, esc(&st.name), b(st.first_col), b(st.last_col), b(st.row_stripes), b(st.col_stripes));
    }
    s.push_str("</table>");
    s
}

/// Adds relationship elements to a part's relationships.
pub fn add_rels(rels: &mut Option<String>, frag: &str) {
    if frag.is_empty() {
        return;
    }
    match rels {
        Some(r) => {
            if let Some(i) = r.rfind("</Relationships>") {
                r.insert_str(i, frag);
            }
        }
        None => *rels = Some(format!(r#"{HEADER}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{frag}</Relationships>"#)),
    }
}

/// Links the tables of sheet `n` (1-based): adds their relationships to
/// the sheet's rels part and `<tableParts>` to its XML.
pub fn link_tables(sheet_xml: &mut String, rels: &mut Option<String>, tables: &[(usize, String)]) {
    if tables.is_empty() {
        return;
    }
    let mut frag = String::new();
    let mut parts = format!(r#"<tableParts count="{}">"#, tables.len());
    for (k, _) in tables {
        let _ = write!(frag, r#"<Relationship Id="rIdT{k}" Type="{REL_TABLE}" Target="../tables/table{k}.xml"/>"#);
        let _ = write!(parts, r#"<tablePart r:id="rIdT{k}"/>"#);
    }
    parts.push_str("</tableParts>");
    add_rels(rels, &frag);
    // tableParts is the last element before extLst.
    let at = sheet_xml.rfind("<extLst>").or_else(|| sheet_xml.rfind("</worksheet>"));
    if let Some(i) = at {
        sheet_xml.insert_str(i, &parts);
    }
}

#[cfg(test)]
mod tests {
    use apex_sheet::table::TotalFn;
    use apex_sheet::{CellRef, Content, Locale, Range, Value, Workbook};

    fn sample() -> Workbook {
        let mut wb = Workbook::new();
        wb.locale = Locale::EN;
        let rows = [["Region", "Qty", "Price"], ["North", "2", "10"], ["South", "3", "20"], ["East", "5", "30"]];
        let mut inputs = Vec::new();
        for (r, row) in rows.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                inputs.push((CellRef::new(r as u32, c as u32), v.to_string()));
            }
        }
        wb.set_inputs(0, inputs);
        let id = wb.create_table(0, Range::parse("A1:C4").unwrap(), true, "TableStyleMedium9").unwrap();
        wb.set_input(0, CellRef::new(0, 3), "Amount");
        wb.table_after_input(0, CellRef::new(0, 3));
        wb.set_input(0, CellRef::new(1, 3), "=[@Qty]*[@Price]");
        wb.table_after_input(0, CellRef::new(1, 3));
        wb.set_table_totals(id, true);
        wb.set_total_fn(id, 1, TotalFn::Average);
        let mut t = wb.table(id).unwrap().clone();
        t.style.first_col = true;
        wb.set_table(id, t, "undo.table");
        wb.rename_table(id, "Sales").unwrap();
        wb.set_input(0, CellRef::new(0, 6), "=SUM(Sales[Amount])");
        // Second sheet with a table without filter buttons.
        wb.execute(apex_sheet::Op::AddSheet { index: 1, sheet: Box::new(apex_sheet::Sheet::new("Two")) });
        wb.set_inputs(1, vec![(CellRef::new(0, 0), "A".into()), (CellRef::new(1, 0), "1".into())]);
        let id2 = wb.create_table(1, Range::parse("A1:A2").unwrap(), true, "TableStyleLight1").unwrap();
        let mut t2 = wb.table(id2).unwrap().clone();
        t2.filter_button = false;
        wb.set_table(id2, t2, "undo.table");
        wb
    }

    fn check(back: &Workbook, full: bool) {
        let v = |s: usize, a: &str| back.sheets[s].value(CellRef::parse(a).unwrap()).clone();
        assert_eq!(v(0, "G1"), Value::Number(230.0));
        assert_eq!(v(0, "D4"), Value::Number(150.0));
        assert_eq!(v(0, "A5"), Value::text("Total"));
        assert!(matches!(v(0, "B5"), Value::Number(n) if (n - 10.0 / 3.0).abs() < 1e-9));
        if !full {
            return;
        }
        let t = &back.sheets[0].tables[0];
        assert_eq!(t.name, "Sales");
        assert_eq!(t.range.to_a1(), "A1:D5");
        assert!(t.totals && t.header && t.filter_button);
        assert_eq!(t.style.name, "TableStyleMedium9");
        assert!(t.style.first_col && t.style.row_stripes && !t.style.col_stripes);
        assert_eq!(t.columns[3].name, "Amount");
        assert_eq!(t.columns[3].formula.as_deref(), Some("[@Qty]*[@Price]"));
        assert_eq!(t.columns[1].total, TotalFn::Average);
        assert_eq!(t.columns[0].total_label.as_deref(), Some("Total"));
        assert_eq!(back.sheets[0].filter.as_ref().unwrap().range.to_a1(), "A1:D4");
        assert!(matches!(back.cell(0, CellRef::parse("G1").unwrap()).unwrap().content, Content::Formula { .. }));
        assert_eq!(back.edit_text(0, CellRef::parse("D3").unwrap()), "=[@Qty]*[@Price]");
        let t2 = &back.sheets[1].tables[0];
        assert!(!t2.filter_button && back.sheets[1].filter.is_none());
    }

    #[test]
    fn xlsx_roundtrip() {
        let wb = sample();
        check(&wb, true);
        let data = crate::xlsx::write(&wb);
        let z = crate::zip::ZipReader::new(&data).unwrap();
        let t1 = z.read_string("xl/tables/table1.xml").unwrap();
        assert!(t1.contains(r#"<calculatedColumnFormula>Sales[[#This Row],[Qty]]*Sales[[#This Row],[Price]]</calculatedColumnFormula>"#), "{t1}");
        assert!(t1.contains(r#"totalsRowFunction="average""#) && t1.contains(r#"<autoFilter ref="A1:D4"/>"#));
        let s1 = z.read_string("xl/worksheets/sheet1.xml").unwrap();
        assert!(s1.contains("<tableParts count=\"1\"><tablePart r:id=\"rIdT1\"/></tableParts></worksheet>"));
        assert!(!s1.contains("<autoFilter"), "the filter lives in the table");
        assert!(s1.contains("SUBTOTAL(101,Sales[Qty])"));
        let wbx = z.read_string("xl/workbook.xml").unwrap();
        assert!(!wbx.contains("_FilterDatabase"));
        assert!(z.read_string("[Content_Types].xml").unwrap().contains("/xl/tables/table2.xml"));
        let back = crate::xlsx::read(&data).unwrap();
        check(&back, true);
    }

    /// A table and a chart on the same sheet: both parts and their rels.
    #[test]
    fn table_with_chart() {
        let mut wb = sample();
        let c = wb.chart_from_range(0, Range::parse("A1:C4").unwrap(), apex_sheet::chart::ChartType::Column, None);
        wb.add_chart(0, c);
        for back in [crate::xlsx::read(&crate::xlsx::write(&wb)).unwrap(), crate::ods::read(&crate::ods::write(&wb)).unwrap()] {
            assert_eq!(back.sheets[0].tables.len(), 1);
            assert_eq!(back.sheets[0].charts.len(), 1);
            assert!(!back.sheets[0].charts[0].series.is_empty());
        }
    }

    #[test]
    fn ods_roundtrip() {
        let wb = sample();
        let data = crate::ods::write(&wb);
        let back = crate::ods::read(&data).unwrap();
        check(&back, false);
        // ODS has database ranges but no table styles: the look is kept
        // as cell formats and the table comes back with its name.
        let t = &back.sheets[0].tables[0];
        assert_eq!(t.name, "Sales");
        assert_eq!(t.range.to_a1(), "A1:D5");
        assert!(t.totals && t.filter_button);
        assert_eq!(t.columns[1].total, TotalFn::Average);
        let a1 = back.cell(0, CellRef::parse("A1").unwrap()).unwrap();
        assert!(back.styles.get(a1.style).bold && back.styles.get(a1.style).fill.is_some());
        assert_eq!(back.sheets[1].tables.len(), 1);
    }
}
