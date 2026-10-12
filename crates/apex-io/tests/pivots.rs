//! Pivot tables and slicers in files: ApexDesk's own through xlsx and ods
//! (spec kept whole, refreshable after reopening), Excel's and
//! LibreOffice's native tables read into editable ones, and the ones
//! ApexDesk can't recalculate kept byte for byte.

use apex_io::zip::{ZipReader, ZipWriter};
use apex_sheet::analysis::pivot::{AggFunc, BaseField, BaseItem, Field, FieldFilter, LabelFilter, LabelOp, Layout, ModelSource, PivotSpec, ShowAs, SortOrder, ValueField, ValueFilter, ValueOp};
use apex_sheet::model::{DataModel, Measure, Relationship};
use apex_sheet::{CellRef, Range, Workbook};

/// Sales by region and product on sheet Data.
fn book() -> Workbook {
    let mut wb = Workbook::new();
    let rows = [
        ["Region", "Product", "Qty", "Price"],
        ["North", "Pen", "3", "1.5"],
        ["South", "Ink", "5", "4"],
        ["North", "Ink", "7", "4"],
        ["East", "Pen", "2", "1.5"],
        ["South", "Pen", "4", "1.5"],
        ["East", "Paper", "10", "0.5"],
    ];
    let mut inputs = Vec::new();
    for (r, row) in rows.iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            inputs.push((CellRef::new(r as u32, c as u32), v.to_string()));
        }
    }
    wb.set_inputs(0, inputs);
    wb.sheets[0].name = "Data".into();
    wb
}

fn source() -> Range {
    Range::parse("A1:D7").unwrap()
}

/// Region by product, sum of Qty.
fn simple_spec() -> PivotSpec {
    PivotSpec { source_sheet: "Data".into(), source: source(), rows: vec![Field::new(0)], values: vec![ValueField { col: 2, ..Default::default() }], ..Default::default() }
}

/// Filters, several functions, sorting, label and value filters, show as.
fn rich_spec() -> PivotSpec {
    let mut region = Field::new(0);
    region.sort = SortOrder::ValueDesc(0);
    region.hidden = vec!["East".into()];
    let mut product = Field::new(1);
    product.label_filter = Some(LabelFilter { op: LabelOp::BeginsWith, a: "P".into(), b: String::new() });
    product.name = Some("Item".into());
    region.value_filter = Some(ValueFilter { value: 0, op: ValueOp::TopItems, a: 2.0, b: 0.0 });
    PivotSpec {
        source_sheet: "Data".into(),
        source: source(),
        rows: vec![region],
        cols: vec![product],
        values: vec![
            ValueField { col: 2, ..Default::default() },
            ValueField { col: 2, func: AggFunc::Average, name: Some("Mean qty".into()), number_format: Some("0.00".into()), ..Default::default() },
            ValueField { col: 3, func: AggFunc::Max, ..Default::default() },
            ValueField { col: 2, func: AggFunc::Count, show_as: ShowAs::PercentOfGrand, ..Default::default() },
            ValueField { col: 2, show_as: ShowAs::DifferenceFrom(BaseField::Row(0), BaseItem::Named("North".into())), ..Default::default() },
        ],
        filters: vec![FieldFilter { field: Field::new(3), items: vec!["1.5".into(), "4".into()] }],
        layout: Layout::Tabular,
        grand_cols: false,
        empty_text: "-".into(),
        style: "PivotStyleMedium9".into(),
        ..Default::default()
    }
}

/// The cell texts of a pivot table's area.
fn grid(wb: &Workbook, i: usize) -> Vec<Vec<String>> {
    let p = &wb.analysis.pivots[i];
    let si = wb.sheet_index(&p.sheet).unwrap();
    let a = p.area.unwrap();
    (a.start.row..=a.end.row).map(|r| (a.start.col..=a.end.col).map(|c| wb.display(si, CellRef::new(r, c)).0).collect()).collect()
}

/// A package with one part replaced (or removed when `data` is None).
fn with_part(pkg: &[u8], name: &str, data: Option<&[u8]>) -> Vec<u8> {
    let z = ZipReader::new(pkg).unwrap();
    let mut w = ZipWriter::new();
    for n in z.names().map(str::to_string).collect::<Vec<_>>() {
        if n == name {
            if let Some(d) = data {
                w.add(&n, d, true);
            }
        } else {
            w.add(&n, &z.read(&n).unwrap(), true);
        }
    }
    w.finish()
}

/// workbook.xml without our extension, as Excel or LibreOffice save it.
fn without_ours(pkg: &[u8]) -> Vec<u8> {
    let z = ZipReader::new(pkg).unwrap();
    let x = z.read_string("xl/workbook.xml").unwrap();
    let a = x.find("<extLst>").unwrap();
    let b = x.find("</extLst>").unwrap() + "</extLst>".len();
    let stripped = format!("{}{}", &x[..a], &x[b..]);
    with_part(pkg, "xl/workbook.xml", Some(stripped.as_bytes()))
}

fn save(name: &str, data: &[u8]) {
    if let Ok(dir) = std::env::var("APEX_PIVOT_OUT") {
        std::fs::write(format!("{dir}/{name}"), data).unwrap();
    }
}

#[test]
fn xlsx_simple_pivot_roundtrip() {
    let mut wb = book();
    let i = wb.create_pivot(simple_spec(), None).unwrap();
    let before = grid(&wb, i);
    let data = apex_io::xlsx::write(&wb);
    save("simple.xlsx", &data);
    let z = ZipReader::new(&data).unwrap();
    let book_xml = z.read_string("xl/workbook.xml").unwrap();
    assert!(book_xml.contains(r#"<pivotCaches><pivotCache cacheId="1" r:id="rIdPC1"/></pivotCaches>"#), "{book_xml}");
    let cache = z.read_string("xl/pivotCache/pivotCacheDefinition1.xml").unwrap();
    assert!(cache.contains(r#"<worksheetSource ref="A1:D7" sheet="Data"/>"#) && cache.contains("refreshOnLoad=\"1\""), "{cache}");
    assert!(cache.contains(r#"<cacheField name="Region" numFmtId="0"><sharedItems count="3"><s v="North"/><s v="South"/><s v="East"/></sharedItems></cacheField>"#), "{cache}");
    let records = z.read_string("xl/pivotCache/pivotCacheRecords1.xml").unwrap();
    assert!(records.contains(r#"count="6""#));
    let table = z.read_string("xl/pivotTables/pivotTable1.xml").unwrap();
    assert!(table.contains(r#"<location ref="A3:B7" firstHeaderRow="1" firstDataRow="1" firstDataCol="1"/>"#), "{table}");
    assert!(table.contains(r#"<rowFields count="1"><field x="0"/></rowFields>"#) && table.contains(r#"<dataField name="Sum of Qty" fld="2" baseField="0" baseItem="0"/>"#), "{table}");
    let ct = z.read_string("[Content_Types].xml").unwrap();
    assert!(ct.contains("/xl/pivotTables/pivotTable1.xml") && ct.contains("/xl/pivotCache/pivotCacheRecords1.xml"));
    assert!(z.read_string("xl/worksheets/_rels/sheet2.xml.rels").unwrap().contains("../pivotTables/pivotTable1.xml"));

    let mut back = apex_io::xlsx::read(&data).unwrap();
    assert_eq!(back.analysis.pivots.len(), 1);
    assert_eq!(back.analysis.pivots[0], wb.analysis.pivots[0]);
    assert!(apex_io::external_data(&back).pivot_tables == 0, "not kept as well");
    // Refreshable and editable after reopening, one undo step each.
    back.set_input(0, CellRef::new(1, 2), "30");
    back.refresh_pivot(0).unwrap();
    assert_eq!(grid(&back, 0)[2], ["North", "37"]);
    let mut spec = back.analysis.pivots[0].spec.clone();
    spec.cols.push(Field::new(1));
    back.replace_pivot(0, spec).unwrap();
    assert_eq!(grid(&back, 0)[1][1], "Ink");
    assert!(back.undo());
    assert!(back.undo());
    assert_eq!(grid(&back, 0), before);
    // Saved again: one table, not two.
    let again = apex_io::xlsx::write(&back);
    let z = ZipReader::new(&again).unwrap();
    assert_eq!(z.names().filter(|n| n.starts_with("xl/pivotTables/pivotTable") && n.ends_with(".xml")).count(), 1);
    assert_eq!(apex_io::xlsx::read(&again).unwrap().analysis.pivots.len(), 1);
}

#[test]
fn xlsx_rich_pivot_and_slicer_roundtrip() {
    let mut wb = book();
    let i = wb.create_pivot(rich_spec(), Some((0, CellRef::new(0, 6)))).unwrap();
    let s = wb.add_slicer(i, 1, 400.0, 200.0).unwrap();
    wb.set_slicer_selection(s, vec!["Pen".into(), "Ink".into()]).unwrap();
    let before = grid(&wb, i);
    let data = apex_io::xlsx::write(&wb);
    save("rich.xlsx", &data);
    let back = apex_io::xlsx::read(&data).unwrap();
    assert_eq!(back.analysis.pivots, wb.analysis.pivots);
    assert_eq!(back.analysis.slicers, wb.analysis.slicers);
    assert_eq!(grid(&back, 0), before);

    // Without our extension the native parts say most of it.
    let native = apex_io::xlsx::read(&without_ours(&data)).unwrap();
    assert_eq!(native.analysis.pivots.len(), 1);
    let p = &native.analysis.pivots[0];
    let want = &wb.analysis.pivots[0];
    assert_eq!((p.sheet.as_str(), p.at), (want.sheet.as_str(), want.at));
    let s = &p.spec;
    assert_eq!((s.source_sheet.as_str(), s.source), ("Data", source()));
    assert_eq!(s.rows[0].col, 0);
    assert_eq!(s.rows[0].sort, SortOrder::ValueDesc(0));
    assert_eq!(s.rows[0].hidden, ["East"]);
    assert_eq!(s.rows[0].value_filter, want.spec.rows[0].value_filter);
    assert_eq!(s.cols[0].name.as_deref(), Some("Item"));
    assert_eq!(s.cols[0].label_filter, want.spec.cols[0].label_filter);
    assert_eq!(s.filters, want.spec.filters);
    let funcs: Vec<AggFunc> = s.values.iter().map(|v| v.func).collect();
    assert_eq!(funcs, [AggFunc::Sum, AggFunc::Average, AggFunc::Max, AggFunc::Count, AggFunc::Sum]);
    assert_eq!(s.values[1].name.as_deref(), Some("Mean qty"));
    assert_eq!(s.values[1].number_format.as_deref(), Some("0.00"));
    assert_eq!(s.values[3].show_as, ShowAs::PercentOfGrand);
    assert_eq!(s.values[4].show_as, want.spec.values[4].show_as);
    assert_eq!((s.layout, s.grand_cols, s.grand_rows, s.empty_text.as_str(), s.style.as_str()), (Layout::Tabular, false, true, "-", "PivotStyleMedium9"));
    // The slicer comes back from Excel's parts too.
    assert_eq!(native.analysis.slicers.len(), 1);
    let sl = &native.analysis.slicers[0];
    assert_eq!((sl.pivot, sl.col, sl.caption.as_str()), (0, 1, "Product"));
    assert!((sl.x - 400.0).abs() < 2.0 && (sl.y - 200.0).abs() < 2.0, "{sl:?}");
    assert_eq!(s.slicers.len(), 1);
    let mut sel = s.slicers[0].items.clone();
    sel.sort();
    assert_eq!(sel, ["Ink", "Pen"]);
    // Excel's names for slicer caches aren't user names, and saving again
    // makes one slicer, not two.
    assert!(back.names.is_empty() && native.names.is_empty());
    let again = apex_io::xlsx::write(&native);
    let z = ZipReader::new(&again).unwrap();
    assert_eq!(z.names().filter(|n| n.starts_with("xl/slicers/") || n.starts_with("xl/slicerCaches/")).count(), 2);
    assert_eq!(apex_io::xlsx::read(&again).unwrap().analysis.slicers, native.analysis.slicers);
}

/// Pivot tables on the data model only go in our extension.
#[test]
fn model_pivot_in_xlsx_and_ods() {
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
    wb.set_data_model(DataModel { relationships: vec![Relationship::new("Orders", "Customer", "Customers", "Customer")], measures: vec![Measure { name: "Revenue".into(), table: "Orders".into(), formula: "SUM(Orders[Amount])".into(), format: String::new() }] });
    let mut ms = ModelSource::default();
    let country = ms.col_of("Customers", "Country");
    let revenue = ms.measure_of("Revenue");
    let spec = PivotSpec { rows: vec![Field::new(country)], values: vec![ValueField { col: revenue, ..Default::default() }], model: Some(ms), ..Default::default() };
    let i = wb.create_pivot(spec, None).unwrap();
    wb.add_slicer(i, country, 300.0, 40.0).unwrap();
    let before = grid(&wb, i);
    for ods in [false, true] {
        let (name, data) = if ods { ("model.ods", apex_io::ods::write(&wb)) } else { ("model.xlsx", apex_io::xlsx::write(&wb)) };
        save(name, &data);
        let z = ZipReader::new(&data).unwrap();
        assert!(!z.names().any(|n| n.contains("pivotTable") || n.contains("slicer")), "{name}: no native parts");
        let mut back = if ods { apex_io::ods::read(&data) } else { apex_io::xlsx::read(&data) }.unwrap();
        assert_eq!(back.analysis.pivots, wb.analysis.pivots, "{name}");
        assert_eq!(back.analysis.slicers, wb.analysis.slicers, "{name}");
        assert_eq!(grid(&back, 0), before, "{name}");
        back.set_input(0, CellRef::new(1, 2), "100");
        back.refresh_pivot(0).unwrap();
        assert_eq!(grid(&back, 0)[2], ["Spain", "105"], "{name}");
    }
}

#[test]
fn ods_pivot_roundtrip() {
    let mut wb = book();
    let i = wb.create_pivot(simple_spec(), None).unwrap();
    let j = wb.create_pivot(rich_spec(), Some((0, CellRef::new(0, 6)))).unwrap();
    let s = wb.add_slicer(j, 1, 400.0, 200.0).unwrap();
    wb.set_slicer_selection(s, vec!["Pen".into()]).unwrap();
    let (g1, g2) = (grid(&wb, i), grid(&wb, j));
    let data = apex_io::ods::write(&wb);
    save("pivots.ods", &data);
    let content = ZipReader::new(&data).unwrap().read_string("content.xml").unwrap();
    assert!(content.contains(r#"<table:source-cell-range table:cell-range-address="Data.A1:Data.D7"/>"#), "{content}");
    assert_eq!(content.matches("<table:data-pilot-table ").count(), 2);
    let back = apex_io::ods::read(&data).unwrap();
    assert_eq!(back.analysis.pivots, wb.analysis.pivots);
    assert_eq!(back.analysis.slicers, wb.analysis.slicers);
    assert_eq!((grid(&back, 0), grid(&back, 1)), (g1, g2));

    // Without our part (as LibreOffice saves it) the data pilot tables are
    // read.
    let native = apex_io::ods::read(&with_part(&data, "apexdesk/model.xml", None)).unwrap();
    assert_eq!(native.analysis.pivots.len(), 2);
    let p = &native.analysis.pivots[0].spec;
    assert_eq!((p.rows.len(), p.values.len(), p.values[0].col, p.values[0].func), (1, 1, 2, AggFunc::Sum));
    let q = &native.analysis.pivots[1].spec;
    assert_eq!(q.filters, wb.analysis.pivots[1].spec.filters);
    assert_eq!(q.values.iter().map(|v| v.func).collect::<Vec<_>>(), [AggFunc::Sum, AggFunc::Average, AggFunc::Max, AggFunc::Count, AggFunc::Sum]);
    assert_eq!(q.rows[0].hidden, ["East"]);
    assert_eq!(q.values[4].show_as, wb.analysis.pivots[1].spec.values[4].show_as);
    assert!(!q.grand_cols && q.grand_rows);
}

/// A package with parts replaced or added.
fn with_parts(pkg: &[u8], parts: &[(&str, String)]) -> Vec<u8> {
    let z = ZipReader::new(pkg).unwrap();
    let mut w = ZipWriter::new();
    for n in z.names().map(str::to_string).collect::<Vec<_>>() {
        if !parts.iter().any(|(p, _)| *p == n) {
            w.add(&n, &z.read(&n).unwrap(), true);
        }
    }
    for (p, d) in parts {
        w.add(p, d.as_bytes(), true);
    }
    w.finish()
}

/// The pivot table parts headless LibreOffice 25 wrote for a data pilot on
/// Data!A1:C4 (rows Region, page field Product showing Pen, sum and
/// count of Qty), put into a workbook ApexDesk wrote; `cache_extra` goes
/// into the Product cache field.
fn libreoffice_book(cache_extra: &str) -> Vec<u8> {
    let mut wb = Workbook::new();
    let rows = [["Region", "Product", "Qty"], ["North", "Pen", "3"], ["South", "Ink", "5"], ["North", "Ink", "7"]];
    let mut inputs = Vec::new();
    for (r, row) in rows.iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            inputs.push((CellRef::new(r as u32, c as u32), v.to_string()));
        }
    }
    wb.set_inputs(0, inputs);
    wb.sheets[0].name = "Data".into();
    wb.sheets.push(apex_sheet::Sheet::new("Pivot"));
    let pkg = apex_io::xlsx::write(&wb);
    let z = ZipReader::new(&pkg).unwrap();
    const RELS: &str = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#;
    const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/";
    let book = z.read_string("xl/workbook.xml").unwrap().replace("</workbook>", r#"<pivotCaches><pivotCache cacheId="1" r:id="rIdLO"/></pivotCaches></workbook>"#);
    let book_rels = z.read_string("xl/_rels/workbook.xml.rels").unwrap().replace("</Relationships>", &format!(r#"<Relationship Id="rIdLO" Type="{REL}pivotCacheDefinition" Target="pivotCache/pivotCacheDefinition1.xml"/></Relationships>"#));
    let ct = z.read_string("[Content_Types].xml").unwrap().replace("</Types>", r#"<Override PartName="/xl/pivotTables/pivotTable1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.pivotTable+xml"/><Override PartName="/xl/pivotCache/pivotCacheDefinition1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.pivotCacheDefinition+xml"/><Override PartName="/xl/pivotCache/pivotCacheRecords1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.pivotCacheRecords+xml"/></Types>"#);
    let cache = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<pivotCacheDefinition xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" r:id="rId1" recordCount="3" createdVersion="3"><cacheSource type="worksheet"><worksheetSource ref="A1:C4" sheet="Data"/></cacheSource><cacheFields count="3"><cacheField name="Region" numFmtId="0"><sharedItems count="2"><s v="North"/><s v="South"/></sharedItems></cacheField><cacheField name="Product" numFmtId="0"><sharedItems count="2"><s v="Ink"/><s v="Pen"/></sharedItems>{cache_extra}</cacheField><cacheField name="Qty" numFmtId="0"><sharedItems containsSemiMixedTypes="0" containsString="0" containsNumber="1" containsInteger="1" minValue="3" maxValue="7" count="3"><n v="3"/><n v="5"/><n v="7"/></sharedItems></cacheField></cacheFields></pivotCacheDefinition>"#
    );
    let records = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<pivotCacheRecords xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" count="3"><r><x v="0"/><x v="1"/><x v="0"/></r><r><x v="1"/><x v="0"/><x v="1"/></r><r><x v="0"/><x v="0"/><x v="2"/></r></pivotCacheRecords>"#;
    let table = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<pivotTableDefinition xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" name="DataPilot1" cacheId="1" dataOnRows="0" applyNumberFormats="0" applyBorderFormats="0" applyFontFormats="0" applyPatternFormats="0" applyAlignmentFormats="0" applyWidthHeightFormats="0" dataCaption="Values" showDrill="0" useAutoFormatting="0" itemPrintTitles="1" indent="0" outline="1" outlineData="1" compact="1" compactData="1"><location ref="A4:C7" firstHeaderRow="1" firstDataRow="2" firstDataCol="1" rowPageCount="1" colPageCount="1"/><pivotFields count="3"><pivotField axis="axisRow" compact="0" showAll="0" defaultSubtotal="0"><items count="2"><item x="0"/><item x="1"/></items></pivotField><pivotField axis="axisPage" compact="0" showAll="0" defaultSubtotal="0"><items count="2"><item h="1" x="0"/><item x="1"/></items></pivotField><pivotField dataField="1" compact="0" showAll="0"/></pivotFields><rowFields count="1"><field x="0"/></rowFields><rowItems count="2"><i><x/></i><i t="grand"><x/></i></rowItems><colFields count="1"><field x="-2"/></colFields><colItems count="2"><i></i><i></i></colItems><pageFields count="1"><pageField fld="1" hier="-1"/></pageFields><dataFields count="2"><dataField name="Sum - Qty" fld="2" subtotal="sum" numFmtId="164"/><dataField name="Count - Qty*" fld="2" subtotal="count" numFmtId="164"/></dataFields><pivotTableStyleInfo name="PivotStyleLight16" showRowHeaders="1" showColHeaders="1" showRowStripes="0" showColStripes="0" showLastColumn="1"/></pivotTableDefinition>"#;
    with_parts(
        &pkg,
        &[
            ("xl/workbook.xml", book),
            ("xl/_rels/workbook.xml.rels", book_rels),
            ("[Content_Types].xml", ct),
            ("xl/worksheets/_rels/sheet2.xml.rels", format!(r#"{RELS}<Relationship Id="rId1" Type="{REL}pivotTable" Target="../pivotTables/pivotTable1.xml"/></Relationships>"#)),
            ("xl/pivotTables/pivotTable1.xml", table.to_string()),
            ("xl/pivotTables/_rels/pivotTable1.xml.rels", format!(r#"{RELS}<Relationship Id="rId1" Type="{REL}pivotCacheDefinition" Target="../pivotCache/pivotCacheDefinition1.xml"/></Relationships>"#)),
            ("xl/pivotCache/pivotCacheDefinition1.xml", cache),
            ("xl/pivotCache/_rels/pivotCacheDefinition1.xml.rels", format!(r#"{RELS}<Relationship Id="rId1" Type="{REL}pivotCacheRecords" Target="pivotCacheRecords1.xml"/></Relationships>"#)),
            ("xl/pivotCache/pivotCacheRecords1.xml", records.to_string()),
        ],
    )
}

#[test]
fn libreoffice_pivot_read_as_editable() {
    let mut wb = apex_io::xlsx::read(&libreoffice_book("")).unwrap();
    assert_eq!(wb.analysis.pivots.len(), 1);
    assert_eq!(apex_io::external_data(&wb).pivot_tables, 0);
    let p = &wb.analysis.pivots[0];
    assert_eq!((p.sheet.as_str(), p.spec.source_sheet.as_str(), p.spec.source), ("Pivot", "Data", Range::parse("A1:C4").unwrap()));
    assert_eq!(p.spec.rows.iter().map(|f| f.col).collect::<Vec<_>>(), [0]);
    assert_eq!(p.spec.filters, [FieldFilter { field: Field::new(1), items: vec!["Pen".into()] }]);
    assert_eq!(p.spec.values.iter().map(|v| (v.col, v.func)).collect::<Vec<_>>(), [(2, AggFunc::Sum), (2, AggFunc::Count)]);
    // Refreshed from the data: only Pen counts.
    wb.refresh_pivot(0).unwrap();
    let g = grid(&wb, 0);
    assert!(g.iter().any(|r| r.contains(&"North".to_string()) && r.contains(&"3".to_string())), "{g:?}");
    assert!(!g.iter().any(|r| r.contains(&"South".to_string())), "{g:?}");
    // Rearranged with the field list, and saved once.
    let mut spec = wb.analysis.pivots[0].spec.clone();
    spec.filters.clear();
    wb.replace_pivot(0, spec).unwrap();
    assert!(grid(&wb, 0).iter().any(|r| r.contains(&"South".to_string())));
    let again = apex_io::xlsx::write(&wb);
    let z = ZipReader::new(&again).unwrap();
    assert_eq!(z.names().filter(|n| n.starts_with("xl/pivotTables/pivotTable") && n.ends_with(".xml")).count(), 1);
    let back = apex_io::xlsx::read(&again).unwrap();
    assert_eq!(back.analysis.pivots[0].spec, wb.analysis.pivots[0].spec);
}

#[test]
fn unsupported_pivot_kept_byte_for_byte() {
    // Grouped items ApexDesk can't recalculate.
    let pkg = libreoffice_book(r#"<fieldGroup base="1"><discretePr count="2"><x v="0"/><x v="1"/></discretePr></fieldGroup>"#);
    let src = ZipReader::new(&pkg).unwrap();
    let wb = apex_io::xlsx::read(&pkg).unwrap();
    assert!(wb.analysis.pivots.is_empty());
    assert_eq!(apex_io::external_data(&wb).pivot_tables, 1);
    let out = apex_io::xlsx::write(&wb);
    let z = ZipReader::new(&out).unwrap();
    let find = |z: &ZipReader, part: &str| z.names().find(|n| n.contains(part) && !n.contains("_rels")).map(str::to_string).unwrap();
    for part in ["pivotTable", "pivotCacheDefinition", "pivotCacheRecords"] {
        assert_eq!(z.read(&find(&z, part)).unwrap(), src.read(&find(&src, part)).unwrap(), "{part}");
    }
    assert_eq!(z.names().filter(|n| n.contains("pivotTables/pivotTable") && n.ends_with(".xml")).count(), 1);
    let back = apex_io::xlsx::read(&out).unwrap();
    assert!(back.analysis.pivots.is_empty());
    assert_eq!(apex_io::external_data(&back).pivot_tables, 1);
}

/// Adds to a package from `libreoffice_book` an Excel slicer on Region of
/// its pivot table, as Excel saves one.
fn with_excel_slicer(pkg: &[u8]) -> Vec<u8> {
    const X14: &str = "http://schemas.microsoft.com/office/spreadsheetml/2009/9/main";
    const SLE: &str = "http://schemas.microsoft.com/office/drawing/2010/slicer";
    let z = ZipReader::new(pkg).unwrap();
    let book = z.read_string("xl/workbook.xml").unwrap().replace("<calcPr", r#"<definedNames><definedName name="Slicer_Region">#N/A</definedName></definedNames><calcPr"#).replace(
        "</workbook>",
        &format!(r#"<extLst><ext uri="{{BBE1A952-AA13-448e-AADC-164F8A28A991}}" xmlns:x14="{X14}"><x14:slicerCaches><x14:slicerCache r:id="rIdXS"/></x14:slicerCaches></ext></extLst></workbook>"#),
    );
    let book_rels = z.read_string("xl/_rels/workbook.xml.rels").unwrap().replace("</Relationships>", r#"<Relationship Id="rIdXS" Type="http://schemas.microsoft.com/office/2007/relationships/slicerCache" Target="slicerCaches/slicerCache1.xml"/></Relationships>"#);
    let ct = z.read_string("[Content_Types].xml").unwrap().replace("</Types>", r#"<Override PartName="/xl/slicerCaches/slicerCache1.xml" ContentType="application/vnd.ms-excel.slicerCache+xml"/><Override PartName="/xl/slicers/slicer1.xml" ContentType="application/vnd.ms-excel.slicer+xml"/><Override PartName="/xl/drawings/drawing1.xml" ContentType="application/vnd.openxmlformats-officedocument.drawing+xml"/></Types>"#);
    let sheet = z.read_string("xl/worksheets/sheet2.xml").unwrap().replace(
        "</worksheet>",
        &format!(r#"<drawing r:id="rId3"/><extLst><ext uri="{{A8765BA9-456A-4dab-B4F3-ACF838C121DE}}" xmlns:x14="{X14}"><x14:slicerList><x14:slicer r:id="rId2"/></x14:slicerList></ext></extLst></worksheet>"#),
    );
    let sheet_rels = z.read_string("xl/worksheets/_rels/sheet2.xml.rels").unwrap().replace("</Relationships>", r#"<Relationship Id="rId2" Type="http://schemas.microsoft.com/office/2007/relationships/slicer" Target="../slicers/slicer1.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing" Target="../drawings/drawing1.xml"/></Relationships>"#);
    let cache = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<slicerCacheDefinition xmlns="{X14}" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" mc:Ignorable="x" xmlns:x="http://schemas.openxmlformats.org/spreadsheetml/2006/main" name="Slicer_Region" sourceName="Region"><pivotTables><pivotTable tabId="2" name="DataPilot1"/></pivotTables><data><tabular pivotCacheId="1"><items count="2"><i x="0" s="1"/><i x="1" s="1"/></items></tabular></data></slicerCacheDefinition>"#
    );
    let slicer = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<slicers xmlns="{X14}" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" mc:Ignorable="x x14ac" xmlns:x="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:x14ac="http://schemas.microsoft.com/office/spreadsheetml/2009/9/ac"><slicer name="Region" cache="Slicer_Region" caption="Region" rowHeight="241300"/></slicers>"#
    );
    let drawing = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><xdr:twoCellAnchor editAs="oneCell"><xdr:from><xdr:col>5</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>8</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>12</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice xmlns:a14="http://schemas.microsoft.com/office/drawing/2010/main" Requires="a14"><xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="2" name="Region"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></xdr:xfrm><a:graphic><a:graphicData uri="{SLE}"><sle:slicer xmlns:sle="{SLE}" name="Region"/></a:graphicData></a:graphic></xdr:graphicFrame></mc:Choice><mc:Fallback><xdr:sp macro="" textlink=""><xdr:nvSpPr><xdr:cNvPr id="0" name=""/><xdr:cNvSpPr><a:spLocks noTextEdit="1"/></xdr:cNvSpPr></xdr:nvSpPr><xdr:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1828800" cy="2524125"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></xdr:spPr><xdr:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Slicer</a:t></a:r></a:p></xdr:txBody></xdr:sp></mc:Fallback></mc:AlternateContent><xdr:clientData/></xdr:twoCellAnchor></xdr:wsDr>"#
    );
    with_parts(
        pkg,
        &[
            ("xl/workbook.xml", book),
            ("xl/_rels/workbook.xml.rels", book_rels),
            ("[Content_Types].xml", ct),
            ("xl/worksheets/sheet2.xml", sheet),
            ("xl/worksheets/_rels/sheet2.xml.rels", sheet_rels),
            ("xl/slicerCaches/slicerCache1.xml", cache),
            ("xl/slicers/slicer1.xml", slicer),
            ("xl/drawings/drawing1.xml", drawing),
        ],
    )
}

/// The relationships of a part: (id, type, target as written).
fn rels_of(z: &ZipReader, rels: &str) -> Vec<(String, String, String)> {
    let x = z.read_string(rels).unwrap_or_default();
    x.split("<Relationship ").skip(1).map(|r| {
        let a = |k: &str| r.split(&format!(r#"{k}=""#)).nth(1).and_then(|v| v.split('"').next()).unwrap_or_default().to_string();
        (a("Id"), a("Type"), a("Target"))
    }).collect()
}

/// Checks that the package has the Excel slicer of `with_excel_slicer`,
/// linked from the workbook, the sheet and its drawing.
fn check_excel_slicer(pkg: &[u8]) {
    let z = ZipReader::new(pkg).unwrap();
    let book = z.read_string("xl/workbook.xml").unwrap();
    assert!(book.contains(r#"<definedName name="Slicer_Region">#N/A</definedName>"#), "{book}");
    assert_eq!(book.matches("{BBE1A952-AA13-448e-AADC-164F8A28A991}").count(), 1, "{book}");
    let rid = book.split(r#"<x14:slicerCache r:id=""#).nth(1).and_then(|v| v.split('"').next()).unwrap();
    let book_rels = rels_of(&z, "xl/_rels/workbook.xml.rels");
    let cache = &book_rels.iter().find(|r| r.0 == rid).unwrap().2;
    let cache_xml = z.read_string(&format!("xl/{cache}")).unwrap();
    assert!(cache_xml.contains(r#"name="Slicer_Region""#) && cache_xml.contains(r#"<pivotTable tabId="2" name="DataPilot1"/>"#));
    assert!(z.read_string("[Content_Types].xml").unwrap().contains(&format!(r#"<Override PartName="/xl/{cache}" ContentType="application/vnd.ms-excel.slicerCache+xml"/>"#)));
    // The pivot table sheet (tab 2) lists the slicer and places it.
    let sheet = z.read_string("xl/worksheets/sheet2.xml").unwrap();
    let sheet_rels = rels_of(&z, "xl/worksheets/_rels/sheet2.xml.rels");
    let target = |ty: &str, id: &str| sheet_rels.iter().find(|r| r.0 == id && r.1.ends_with(ty)).map(|r| r.2.replace("../", "xl/"));
    let sid = sheet.split(r#"<x14:slicer r:id=""#).nth(1).and_then(|v| v.split('"').next()).unwrap();
    let slicer = z.read_string(&target("/slicer", sid).unwrap()).unwrap();
    assert!(slicer.contains(r#"<slicer name="Region" cache="Slicer_Region""#), "{slicer}");
    let did = sheet.split(r#"<drawing r:id=""#).nth(1).and_then(|v| v.split('"').next()).unwrap();
    let drawing = z.read_string(&target("/drawing", did).unwrap()).unwrap();
    assert!(drawing.contains(r#"<sle:slicer xmlns:sle="http://schemas.microsoft.com/office/drawing/2010/slicer" name="Region"/>"#) && drawing.contains("<mc:Fallback>"), "{drawing}");
    assert_eq!(drawing.matches(r#"<sle:slicer xmlns:sle="http://schemas.microsoft.com/office/drawing/2010/slicer" name="Region"/>"#).count(), 1);
    // Every part the relationships name is there.
    for r in book_rels.iter().filter(|r| !r.2.starts_with("..")) {
        assert!(z.contains(&format!("xl/{}", r.2)), "{r:?}");
    }
    for r in &sheet_rels {
        assert!(z.contains(&r.2.replace("../", "xl/")), "{r:?}");
    }
    // No start tag left open before the next one.
    for n in z.names().filter(|n| n.ends_with(".xml")) {
        let x = z.read_string(n).unwrap();
        assert!(!x.contains("\"<") && !x.contains("\" <"), "{n}: {x}");
    }
}

#[test]
fn excel_slicer_of_kept_pivot_kept() {
    let pkg = with_excel_slicer(&libreoffice_book(r#"<fieldGroup base="1"><discretePr count="2"><x v="0"/><x v="1"/></discretePr></fieldGroup>"#));
    check_excel_slicer(&pkg);
    let wb = apex_io::xlsx::read(&pkg).unwrap();
    assert!(wb.analysis.pivots.is_empty() && wb.analysis.slicers.is_empty());
    let out = apex_io::xlsx::write(&wb);
    check_excel_slicer(&out);
    let src = ZipReader::new(&pkg).unwrap();
    let z = ZipReader::new(&out).unwrap();
    for part in ["xl/slicerCaches/slicerCache1.xml", "xl/slicers/slicer1.xml"] {
        assert_eq!(z.read(part).unwrap(), src.read(part).unwrap(), "{part}");
    }
    // Twice, and alongside a slicer of ApexDesk's own.
    let mut back = apex_io::xlsx::read(&out).unwrap();
    check_excel_slicer(&apex_io::xlsx::write(&back));
    let mut spec = PivotSpec { source_sheet: "Data".into(), source: Range::parse("A1:C4").unwrap(), rows: vec![Field::new(1)], ..Default::default() };
    spec.values.push(ValueField { col: 2, ..Default::default() });
    let i = back.create_pivot(spec, Some((0, CellRef::new(0, 5)))).unwrap();
    back.add_slicer(i, 0, 300.0, 100.0).unwrap();
    let both = apex_io::xlsx::write(&back);
    check_excel_slicer(&both);
    save("kept_slicer.xlsx", &both);
    let z = ZipReader::new(&both).unwrap();
    assert_eq!(z.names().filter(|n| n.starts_with("xl/slicerCaches/") && n.ends_with(".xml")).count(), 2);
    let again = apex_io::xlsx::read(&both).unwrap();
    assert_eq!(again.analysis.slicers.len(), 1);
    // Without its pivot table (sheet deleted), the slicer goes too.
    let mut gone = apex_io::xlsx::read(&pkg).unwrap();
    gone.sheets.remove(1);
    let z = ZipReader::new(&apex_io::xlsx::write(&gone)).unwrap().names().filter(|n| n.contains("slicer")).count();
    assert_eq!(z, 0);
}

/// Every XML part parses: end tags match, no attribute twice.
fn check_well_formed(pkg: &[u8]) {
    use quick_xml::events::Event;
    let z = ZipReader::new(pkg).unwrap();
    for n in z.names().filter(|n| n.ends_with(".xml") || n.ends_with(".rels")) {
        let x = z.read_string(n).unwrap();
        let mut r = quick_xml::Reader::from_str(&x);
        loop {
            match r.read_event() {
                Ok(Event::Eof) => break,
                Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                    for a in e.attributes() {
                        a.unwrap_or_else(|err| panic!("{n}: {err}"));
                    }
                }
                Err(err) => panic!("{n}: {err}\n{x}"),
                _ => {}
            }
        }
    }
}

/// The slicer parts of sheet Pivot (tab 2) and the names of their slicers.
fn slicer_parts(pkg: &[u8]) -> Vec<(String, Vec<String>)> {
    let z = ZipReader::new(pkg).unwrap();
    let sheet = z.read_string("xl/worksheets/sheet2.xml").unwrap();
    let rels = rels_of(&z, "xl/worksheets/_rels/sheet2.xml.rels");
    let mut out = Vec::new();
    for item in sheet.split("<x14:slicer r:id=\"").skip(1) {
        let id = item.split('"').next().unwrap();
        let path = rels.iter().find(|r| r.0 == id && r.1.ends_with("/slicer")).unwrap().2.replace("../", "xl/");
        let x = z.read_string(&path).unwrap();
        let names = x.split("<slicer name=\"").skip(1).map(|s| s.split('"').next().unwrap().to_string()).collect();
        out.push((path, names));
    }
    out
}

#[test]
fn mixed_slicer_part_split() {
    // The kept pivot table's slicer (Region) and one of ApexDesk's pivot
    // table on the same sheet (Product), saved in one part as Excel does.
    let pkg = with_excel_slicer(&libreoffice_book(r#"<fieldGroup base="1"><discretePr count="2"><x v="0"/><x v="1"/></discretePr></fieldGroup>"#));
    let mut wb = apex_io::xlsx::read(&pkg).unwrap();
    let mut spec = PivotSpec { source_sheet: "Data".into(), source: Range::parse("A1:C4").unwrap(), rows: vec![Field::new(1)], ..Default::default() };
    spec.values.push(ValueField { col: 2, ..Default::default() });
    let i = wb.create_pivot(spec, Some((1, CellRef::new(0, 10)))).unwrap();
    wb.add_slicer(i, 1, 500.0, 40.0).unwrap();
    let two = apex_io::xlsx::write(&wb);
    let parts = slicer_parts(&two);
    assert_eq!(parts.len(), 2, "{parts:?}");
    let (kept, ours) = (&parts[0].0, &parts[1].0);
    let z = ZipReader::new(&two).unwrap();
    let ours_xml = z.read_string(ours).unwrap();
    let item = format!("<slicer {}", ours_xml.split("<slicer ").nth(1).unwrap().split("</slicers>").next().unwrap());
    let merged = z.read_string(kept).unwrap().replace("</slicers>", &format!("{item}</slicers>"));
    let rels_path = "xl/worksheets/_rels/sheet2.xml.rels";
    let rid = rels_of(&z, rels_path).into_iter().find(|r| r.2.replace("../", "xl/") == *ours).unwrap().0;
    let rels = z.read_string(rels_path).unwrap().split("<Relationship ").filter(|r| !r.contains(&format!(r#"Id="{rid}""#))).collect::<Vec<_>>().join("<Relationship ");
    let sheet = z.read_string("xl/worksheets/sheet2.xml").unwrap().replace(&format!(r#"<x14:slicer r:id="{rid}"/>"#), "");
    let ct = z.read_string("[Content_Types].xml").unwrap().replace(&format!(r#"<Override PartName="/{ours}" ContentType="application/vnd.ms-excel.slicer+xml"/>"#), "");
    let mut w = ZipWriter::new();
    for n in z.names() {
        let data = match n {
            _ if n == ours => continue,
            _ if n == kept => merged.clone().into_bytes(),
            "xl/worksheets/_rels/sheet2.xml.rels" => rels.clone().into_bytes(),
            "xl/worksheets/sheet2.xml" => sheet.clone().into_bytes(),
            "[Content_Types].xml" => ct.clone().into_bytes(),
            _ => z.read(n).unwrap(),
        };
        w.add(n, &data, true);
    }
    let mixed = w.finish();
    assert_eq!(slicer_parts(&mixed).len(), 1);
    check_well_formed(&mixed);
    // ApexDesk's slicer is read as its own again, Excel's kept; each goes
    // to its part, twice.
    let mut pkg = mixed;
    for _ in 0..2 {
        let wb = apex_io::xlsx::read(&pkg).unwrap();
        assert_eq!(wb.analysis.pivots.len(), 1);
        assert_eq!(wb.analysis.slicers.len(), 1);
        assert_eq!(wb.analysis.slicers[0].caption, "Product");
        pkg = apex_io::xlsx::write(&wb);
        check_excel_slicer(&pkg);
        check_well_formed(&pkg);
        let parts = slicer_parts(&pkg);
        let names: Vec<&[String]> = parts.iter().map(|p| p.1.as_slice()).collect();
        assert_eq!(names, [&["Region".to_string()][..], &["Product".to_string()][..]], "{parts:?}");
        let z = ZipReader::new(&pkg).unwrap();
        let drawing = z.read_string("xl/drawings/drawing2.xml").unwrap();
        for n in ["Region", "Product"] {
            assert_eq!(drawing.matches(&format!(r#"<sle:slicer xmlns:sle="http://schemas.microsoft.com/office/drawing/2010/slicer" name="{n}"/>"#)).count(), 1, "{drawing}");
        }
        let book = z.read_string("xl/workbook.xml").unwrap();
        for n in ["Slicer_Region", "Slicer_Product"] {
            assert_eq!(book.matches(&format!(r#"<definedName name="{n}">"#)).count(), 1, "{book}");
        }
        assert_eq!(book.matches("<x14:slicerCache r:id=").count(), 2, "{book}");
    }
    save("mixed_slicers.xlsx", &pkg);
}
