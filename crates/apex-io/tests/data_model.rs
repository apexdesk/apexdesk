//! The data model in files: relationships and measures through xlsx and
//! ods, pivot tables on the model after reopening, and what is read of an
//! Excel data model (which stays in the file).

use apex_io::zip::{ZipReader, ZipWriter};
use apex_sheet::analysis::pivot::{Field, ModelSource, PivotSpec, ValueField};
use apex_sheet::model::{DataModel, Measure, Relationship};
use apex_sheet::{CellRef, Range, Value, Workbook};

const X15: &str = "http://schemas.microsoft.com/office/spreadsheetml/2010/11/main";
const MAIN: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// Orders and customers, related by customer id, with two measures.
fn book() -> Workbook {
    let mut wb = Workbook::new();
    let mut inputs = Vec::new();
    for (r, row) in [["Order", "Customer", "Amount"], ["1", "C1", "10"], ["2", "C2", "25"], ["3", "C1", "5"]].iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            inputs.push((CellRef::new(r as u32, c as u32), v.to_string()));
        }
    }
    for (r, row) in [["Customer", "Country"], ["C1", "Spain"], ["C2", "Italy"]].iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            inputs.push((CellRef::new(r as u32, 5 + c as u32), v.to_string()));
        }
    }
    wb.set_inputs(0, inputs);
    for (r, name) in [("A1:C4", "Orders"), ("F1:G3", "Customers")] {
        let id = wb.create_table(0, Range::parse(r).unwrap(), true, "TableStyleMedium2").unwrap();
        wb.rename_table(id, name).unwrap();
    }
    let measures = vec![
        Measure { name: "Revenue".into(), table: "Orders".into(), formula: "SUM(Orders[Amount])".into(), format: "#,##0.00".into() },
        Measure { name: "Spain \"share\"".into(), table: "Orders".into(), formula: "DIVIDE(CALCULATE([Revenue], Customers[Country] = \"Spain\"), [Revenue])".into(), format: String::new() },
    ];
    wb.set_data_model(DataModel { relationships: vec![Relationship::new("Orders", "Customer", "Customers", "Customer")], measures });
    wb
}

/// Revenue by country, from a pivot table on the model.
fn by_country(wb: &Workbook) -> Vec<(String, Value)> {
    let mut ms = ModelSource::default();
    let country = ms.col_of("Customers", "Country");
    let revenue = ms.measure_of("Revenue");
    let share = ms.measure_of("Spain \"share\"");
    let spec = PivotSpec { rows: vec![Field::new(country)], values: vec![ValueField { col: revenue, ..Default::default() }, ValueField { col: share, ..Default::default() }], model: Some(ms), ..Default::default() };
    let t = wb.pivot_compute(&spec).unwrap();
    t.cells[1..].iter().map(|r| (format!("{:?}", r[0].value), r[1].value.clone())).chain(std::iter::once(("share".to_string(), t.cells.last().unwrap()[2].value.clone()))).collect()
}

fn check(wb: &Workbook, expect: &DataModel) {
    assert_eq!(&wb.analysis.model, expect);
    let got = by_country(wb);
    assert_eq!(got[0].1, Value::Number(25.0));
    assert_eq!(got[1].1, Value::Number(15.0));
    assert_eq!(got[2].1, Value::Number(40.0));
    assert_eq!(got[3].1, Value::Number(15.0 / 40.0));
}

#[test]
fn xlsx_and_ods_keep_the_model() {
    let wb = book();
    let model = wb.analysis.model.clone();
    check(&wb, &model);
    let data = apex_io::xlsx::write(&wb);
    let book_xml = ZipReader::new(&data).unwrap().read_string("xl/workbook.xml").unwrap();
    assert!(book_xml.contains("urn:apexdesk:model") && book_xml.ends_with("</extLst></workbook>"), "{book_xml}");
    let back = apex_io::xlsx::read(&data).unwrap();
    check(&back, &model);
    // Twice (the extension isn't duplicated).
    let again = apex_io::xlsx::write(&back);
    let book_xml = ZipReader::new(&again).unwrap().read_string("xl/workbook.xml").unwrap();
    assert_eq!(book_xml.matches("urn:apexdesk:model").count(), 1);

    let data = apex_io::ods::write(&wb);
    let z = ZipReader::new(&data).unwrap();
    assert!(z.read_string("META-INF/manifest.xml").unwrap().contains("apexdesk/model.xml"));
    let back = apex_io::ods::read(&data).unwrap();
    check(&back, &model);
    // No model, no part.
    let plain = apex_io::ods::write(&Workbook::new());
    assert!(ZipReader::new(&plain).unwrap().read_string("apexdesk/model.xml").is_err());
}

/// A package with some parts added and XML parts edited.
fn repack(data: &[u8], add: &[(&str, Vec<u8>)], f: &dyn Fn(&str, String) -> String) -> Vec<u8> {
    let z = ZipReader::new(data).unwrap();
    let mut w = ZipWriter::new();
    for n in z.names().map(String::from).collect::<Vec<_>>() {
        let b = z.read(&n).unwrap();
        match String::from_utf8(b.clone()) {
            Ok(s) if n.ends_with(".xml") || n.ends_with(".rels") => w.add(&n, f(&n, s).as_bytes(), true),
            _ => w.add(&n, &b, true),
        }
    }
    for (p, b) in add {
        w.add(p, b, true);
    }
    w.finish()
}

fn before(s: String, tag: &str, what: &str) -> String {
    let i = s.rfind(tag).unwrap();
    format!("{}{what}{}", &s[..i], &s[i..])
}

const CACHE: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><pivotCacheDefinition xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" refreshOnLoad="1"><cacheSource type="external" connectionId="1"/><cacheFields count="0"/><cacheHierarchies count="2"><cacheHierarchy uniqueName="[Customers].[Country]" caption="Country" attribute="1" defaultMemberUniqueName="[Customers].[Country].[All]" allUniqueName="[Customers].[Country].[All]" dimensionUniqueName="[Customers]" displayFolder="" count="0" memberValueDatatype="130" unbalanced="0"/><cacheHierarchy uniqueName="[Measures].[Total Revenue]" caption="Total Revenue" measure="1" displayFolder="" measureGroup="Orders" count="0"/></cacheHierarchies></pivotCacheDefinition>"#;

#[test]
fn excel_model_is_listed_and_kept() {
    let wb = book();
    let base = apex_io::xlsx::write(&wb);
    let item = b"XPress9-compressed model bytes \x00\x01\x02".to_vec();
    let add: Vec<(&str, Vec<u8>)> = vec![("xl/model/item.data", item.clone()), ("xl/pivotCache/pivotCacheDefinition9.xml", CACHE.as_bytes().to_vec())];
    let data = repack(&base, &add, &|name, s| match name {
        "[Content_Types].xml" => before(s, "</Types>", r#"<Override PartName="/xl/model/item.data" ContentType="application/vnd.openxmlformats-officedocument.model+data"/><Override PartName="/xl/pivotCache/pivotCacheDefinition9.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.pivotCacheDefinition+xml"/>"#),
        "xl/_rels/workbook.xml.rels" => before(s, "</Relationships>", &format!(r#"<Relationship Id="rId91" Type="{MAIN}/powerPivotData" Target="model/item.data"/><Relationship Id="rId92" Type="{MAIN}/pivotCacheDefinition" Target="pivotCache/pivotCacheDefinition9.xml"/>"#)),
        "xl/workbook.xml" => {
            let s = s.replacen("<workbook ", &format!(r#"<workbook xmlns:x15="{X15}" "#), 1);
            let ext = r#"<ext uri="{FCE2AD5D-F65C-4FA6-A056-5C36A1767C68}"><x15:dataModel><x15:modelTables><x15:modelTable id="Orders_1" name="Orders" connection="WorksheetConnection_Orders"/><x15:modelTable id="Customers_2" name="Customers" connection="WorksheetConnection_Customers"/></x15:modelTables><x15:modelRelationships><x15:modelRelationship fromTable="Orders" fromColumn="Customer" toTable="Customers" toColumn="Customer"/></x15:modelRelationships></x15:dataModel></ext>"#;
            // Before our own extension, as Excel would order it.
            s.replacen("<extLst>", &format!("<extLst>{ext}"), 1)
        }
        _ => s,
    });
    let back = apex_io::xlsx::read(&data).unwrap();
    let m = apex_io::excel_model(&back);
    assert!(m.present);
    assert_eq!(m.tables, ["Orders", "Customers"]);
    assert_eq!(m.relationships, [Relationship::new("Orders", "Customer", "Customers", "Customer")]);
    assert_eq!(m.measures, ["Total Revenue"]);
    // Ours read back too, and everything written again.
    assert_eq!(back.analysis.model, wb.analysis.model);
    let out = apex_io::xlsx::write(&back);
    let z = ZipReader::new(&out).unwrap();
    assert_eq!(z.read("xl/model/item.data").unwrap(), item);
    let book_xml = z.read_string("xl/workbook.xml").unwrap();
    assert!(book_xml.contains("<x15:modelRelationship ") && book_xml.contains("urn:apexdesk:model"), "{book_xml}");
    assert!(apex_io::excel_model(&Workbook::new()) == apex_io::ExcelModel::default());
}
