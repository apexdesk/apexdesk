//! Get Data queries saved in xlsx and ods and read back, next to the
//! custom XML the file already had.

use apex_io::zip::ZipReader;
use apex_sheet::query::{DbKind, FilterOp, Query, Source, Step};
use apex_sheet::{CellRef, Workbook};

fn book() -> Workbook {
    let mut wb = Workbook::new();
    wb.set_input(0, CellRef::new(0, 0), "x");
    let mut a = Query::new("Sales", Source::Text { location: "data/sales.csv".into(), delimiter: Some(';'), encoding: String::new(), header: true, skip: 0 });
    a.steps.push(Step::Filter { column: "Region".into(), op: FilterOp::Eq, value: "North".into(), case: true });
    a.table = Some("Sales".into());
    a.refresh_on_open = true;
    let b = Query::new("Db", Source::Database { kind: DbKind::Postgres, host: "db".into(), port: 5432, database: "shop".into(), user: "ann".into(), sql: "SELECT 1".into(), tls: true });
    wb.queries = vec![a, b];
    wb
}

#[test]
fn xlsx_keeps_queries() {
    let wb = book();
    let bytes = apex_io::xlsx::write(&wb);
    let z = ZipReader::new(&bytes).unwrap();
    let part = z.read_string("customXml/item1.xml").unwrap();
    assert!(part.contains("urn:apexdesk:queries"));
    // No password is ever written.
    assert!(!part.to_lowercase().contains("password"));
    assert!(z.read_string("[Content_Types].xml").unwrap().contains("/customXml/itemProps1.xml"));
    assert!(z.read_string("xl/_rels/workbook.xml.rels").unwrap().contains("../customXml/item1.xml"));
    let back = apex_io::xlsx::read(&bytes).unwrap();
    assert_eq!(back.queries, wb.queries);
    // Ours is not also kept as a foreign part: saving again writes it once.
    assert!(back.kept.parts.iter().all(|p| !p.path.starts_with("customXml/")));
    let again = apex_io::xlsx::write(&back);
    let z = ZipReader::new(&again).unwrap();
    assert!(z.read("customXml/item2.xml").is_err());
    assert_eq!(apex_io::xlsx::read(&again).unwrap().queries, wb.queries);
    // Without queries there is no part.
    let z2 = apex_io::xlsx::write(&Workbook::new());
    assert!(ZipReader::new(&z2).unwrap().read("customXml/item1.xml").is_err());
}

#[test]
fn ods_keeps_queries() {
    let wb = book();
    let bytes = apex_io::ods::write(&wb);
    let z = ZipReader::new(&bytes).unwrap();
    assert!(z.read_string("META-INF/manifest.xml").unwrap().contains(r#"manifest:full-path="ApexDesk/queries.xml""#));
    let back = apex_io::ods::read(&bytes).unwrap();
    assert_eq!(back.queries, wb.queries);
}

/// The workbook's tests (Formulas > Tests) go in a part of their own next
/// to the queries, in xlsx and ods, and are read back.
#[test]
fn tests_are_saved_with_the_book() {
    let mut wb = book();
    wb.checks = vec![apex_sheet::Check { name: "Totals".into(), sheet: "Sheet1".into(), formula: "SUM(A1:A3)=A4".into() }];
    let bytes = apex_io::xlsx::write(&wb);
    let z = ZipReader::new(&bytes).unwrap();
    assert!(z.read_string("customXml/itemChecks.xml").unwrap().contains("urn:apexdesk:checks"));
    let back = apex_io::xlsx::read(&bytes).unwrap();
    assert_eq!(back.checks, wb.checks);
    assert_eq!(back.queries, wb.queries);
    assert!(back.kept.parts.iter().all(|p| !p.path.starts_with("customXml/")));
    let again = apex_io::xlsx::read(&apex_io::xlsx::write(&back)).unwrap();
    assert_eq!(again.checks, wb.checks);
    let back = apex_io::ods::read(&apex_io::ods::write(&wb)).unwrap();
    assert_eq!(back.checks, wb.checks);
}
