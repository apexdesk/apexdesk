//! Excel slicers of ordinary tables (Excel 2013), which ApexDesk has none
//! of its own: kept as they were, linked again to their table (whose ids
//! change on save), and left out cleanly with it.

use apex_io::zip::{ZipReader, ZipWriter};
use apex_sheet::{CellRef, Range, Workbook};

const X14: &str = "http://schemas.microsoft.com/office/spreadsheetml/2009/9/main";
const X15: &str = "http://schemas.microsoft.com/office/spreadsheetml/2010/11/main";
const SLE: &str = "http://schemas.microsoft.com/office/drawing/2010/slicer";
const REL_SLICER: &str = "http://schemas.microsoft.com/office/2007/relationships/slicer";
const REL_SLICER_CACHE: &str = "http://schemas.microsoft.com/office/2007/relationships/slicerCache";

/// Sheet Data with tables Other (F1:G3) and Sales (A1:C5), and an empty
/// sheet Report.
fn book() -> Workbook {
    let mut wb = Workbook::new();
    let rows = [["Region", "Product", "Qty"], ["North", "Pen", "3"], ["South", "Ink", "5"], ["North", "Ink", "7"], ["East", "Pen", "2"]];
    let mut inputs = Vec::new();
    for (r, row) in rows.iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            inputs.push((CellRef::new(r as u32, c as u32), v.to_string()));
        }
    }
    for (r, row) in [["Key", "Value"], ["a", "1"], ["b", "2"]].iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            inputs.push((CellRef::new(r as u32, 5 + c as u32), v.to_string()));
        }
    }
    wb.set_inputs(0, inputs);
    wb.sheets[0].name = "Data".into();
    wb.sheets.push(apex_sheet::Sheet::new("Report"));
    let other = wb.create_table(0, Range::parse("F1:G3").unwrap(), true, "TableStyleMedium2").unwrap();
    wb.rename_table(other, "Other").unwrap();
    let sales = wb.create_table(0, Range::parse("A1:C5").unwrap(), true, "TableStyleMedium2").unwrap();
    wb.rename_table(sales, "Sales").unwrap();
    wb
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

/// The table part named `name`.
fn table_part(z: &ZipReader, name: &str) -> Option<(String, String)> {
    z.names().filter(|n| n.starts_with("xl/tables/table")).map(|n| (n.to_string(), z.read_string(n).unwrap())).find(|(_, x)| x.contains(&format!(r#" name="{name}""#)))
}

fn attr(x: &str, key: &str) -> String {
    x.split(&format!(r#" {key}=""#)).nth(1).and_then(|v| v.split('"').next()).unwrap_or_default().to_string()
}

/// `book` with an Excel 2013 slicer on column Region of table Sales on
/// sheet Report, as Excel saves one; Sales gets id 9 and its columns ids
/// 4 to 6 (Excel keeps ids of deleted tables and columns taken).
fn with_table_slicer(wb: &Workbook) -> Vec<u8> {
    let pkg = apex_io::xlsx::write(wb);
    let z = ZipReader::new(&pkg).unwrap();
    let (tpath, table) = table_part(&z, "Sales").unwrap();
    let table = table.replacen(&format!(r#" id="{}""#, attr(&table, "id")), r#" id="9""#, 1).replace(r#"<tableColumn id="1""#, r#"<tableColumn id="4""#).replace(r#"<tableColumn id="2""#, r#"<tableColumn id="5""#).replace(r#"<tableColumn id="3""#, r#"<tableColumn id="6""#);
    let mut book = z.read_string("xl/workbook.xml").unwrap();
    let names = r#"<definedName name="Slicer_Region">#N/A</definedName>"#;
    book = match book.find("</definedNames>") {
        Some(i) => format!("{}{names}{}", &book[..i], &book[i..]),
        None => book.replace("<calcPr", &format!("<definedNames>{names}</definedNames><calcPr")),
    };
    let ext = format!(r#"<ext uri="{{46BE6895-7355-4a93-B00E-2C351335B9C9}}" xmlns:x15="{X15}"><x15:slicerCaches xmlns:x14="{X14}"><x14:slicerCache r:id="rIdTS"/></x15:slicerCaches></ext>"#);
    book = match book.find("</extLst></workbook>") {
        Some(i) => format!("{}{ext}{}", &book[..i], &book[i..]),
        None => book.replace("</workbook>", &format!("<extLst>{ext}</extLst></workbook>")),
    };
    let book_rels = z.read_string("xl/_rels/workbook.xml.rels").unwrap().replace("</Relationships>", &format!(r#"<Relationship Id="rIdTS" Type="{REL_SLICER_CACHE}" Target="slicerCaches/slicerCache1.xml"/></Relationships>"#));
    let ct = z.read_string("[Content_Types].xml").unwrap().replace("</Types>", r#"<Override PartName="/xl/slicerCaches/slicerCache1.xml" ContentType="application/vnd.ms-excel.slicerCache+xml"/><Override PartName="/xl/slicers/slicer1.xml" ContentType="application/vnd.ms-excel.slicer+xml"/><Override PartName="/xl/drawings/drawing9.xml" ContentType="application/vnd.openxmlformats-officedocument.drawing+xml"/></Types>"#);
    let sheet = z.read_string("xl/worksheets/sheet2.xml").unwrap();
    assert!(!sheet.contains("<extLst>") && !sheet.contains("<drawing "));
    let sheet = sheet.replace("</worksheet>", &format!(r#"<drawing r:id="rId3"/><extLst><ext uri="{{3A4CF648-6AED-40f4-86FF-DC5316D8AED3}}" xmlns:x15="{X15}"><x14:slicerList xmlns:x14="{X14}"><x14:slicer r:id="rId2"/></x14:slicerList></ext></extLst></worksheet>"#));
    let sheet_rels = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId2" Type="{REL_SLICER}" Target="../slicers/slicer1.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing" Target="../drawings/drawing9.xml"/></Relationships>"#);
    let cache = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<slicerCacheDefinition xmlns="{X14}" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" mc:Ignorable="x" xmlns:x="http://schemas.openxmlformats.org/spreadsheetml/2006/main" name="Slicer_Region" sourceName="Region"><extLst><x:ext uri="{{2F2917AC-EB37-4324-AD4E-5DD8C200BD13}}" xmlns:x15="{X15}"><x15:tableSlicerCache tableId="9" column="4"/></x:ext></extLst></slicerCacheDefinition>"#
    );
    let slicer = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<slicers xmlns="{X14}" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" mc:Ignorable="x" xmlns:x="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><slicer name="Region" cache="Slicer_Region" caption="Region" rowHeight="241300"/></slicers>"#
    );
    let drawing = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><xdr:twoCellAnchor editAs="oneCell"><xdr:from><xdr:col>1</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>4</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>12</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice xmlns:sle15="http://schemas.microsoft.com/office/drawing/2012/slicer" Requires="sle15"><xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="2" name="Region"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></xdr:xfrm><a:graphic><a:graphicData uri="{SLE}"><sle:slicer xmlns:sle="{SLE}" name="Region"/></a:graphicData></a:graphic></xdr:graphicFrame></mc:Choice><mc:Fallback><xdr:sp macro="" textlink=""><xdr:nvSpPr><xdr:cNvPr id="0" name=""/><xdr:cNvSpPr><a:spLocks noTextEdit="1"/></xdr:cNvSpPr></xdr:nvSpPr><xdr:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1828800" cy="2524125"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></xdr:spPr><xdr:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Table slicer</a:t></a:r></a:p></xdr:txBody></xdr:sp></mc:Fallback></mc:AlternateContent><xdr:clientData/></xdr:twoCellAnchor></xdr:wsDr>"#
    );
    with_parts(
        &pkg,
        &[
            (tpath.as_str(), table),
            ("xl/workbook.xml", book),
            ("xl/_rels/workbook.xml.rels", book_rels),
            ("[Content_Types].xml", ct),
            ("xl/worksheets/sheet2.xml", sheet),
            ("xl/worksheets/_rels/sheet2.xml.rels", sheet_rels),
            ("xl/slicerCaches/slicerCache1.xml", cache),
            ("xl/slicers/slicer1.xml", slicer),
            ("xl/drawings/drawing9.xml", drawing),
        ],
    )
}

/// The relationships of a part: (id, type, target as written).
fn rels_of(z: &ZipReader, rels: &str) -> Vec<(String, String, String)> {
    let x = z.read_string(rels).unwrap_or_default();
    x.split("<Relationship ").skip(1).map(|r| (attr(&format!(" {r}"), "Id"), attr(&format!(" {r}"), "Type"), attr(&format!(" {r}"), "Target"))).collect()
}

/// Every XML part parses (end tags match, no attribute twice) and every
/// relationship reaches a part with its content type.
fn check_package(pkg: &[u8]) {
    use quick_xml::events::Event;
    let z = ZipReader::new(pkg).unwrap();
    let ct = z.read_string("[Content_Types].xml").unwrap();
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
        if n.contains("slicer") && !n.contains("_rels") {
            assert!(ct.contains(&format!(r#"PartName="/{n}""#)), "{n}");
        }
    }
    for n in z.names().filter(|n| n.ends_with(".rels")) {
        let dir = n.split("_rels/").next().unwrap().trim_end_matches('/');
        for (_, _, target) in rels_of(&z, n) {
            let mut path: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
            for seg in target.split('/') {
                match seg {
                    ".." => {
                        path.pop();
                    }
                    s => path.push(s),
                }
            }
            if !target.starts_with("http") {
                assert!(z.contains(&path.join("/")), "{n}: {target}");
            }
        }
    }
}

/// Checks the table slicer is there and linked: from the workbook to its
/// cache (pointing at table Sales and column Region by their written ids),
/// from sheet Report to its slicer and frame.
fn check_table_slicer(pkg: &[u8]) {
    check_package(pkg);
    let z = ZipReader::new(pkg).unwrap();
    let book = z.read_string("xl/workbook.xml").unwrap();
    assert_eq!(book.matches(r#"<definedName name="Slicer_Region">#N/A</definedName>"#).count(), 1, "{book}");
    assert_eq!(book.matches("{46BE6895-7355-4a93-B00E-2C351335B9C9}").count(), 1, "{book}");
    let ext = book.split("{46BE6895-7355-4a93-B00E-2C351335B9C9}").nth(1).unwrap();
    let rid = attr(ext.split("</ext>").next().unwrap(), "r:id");
    let book_rels = rels_of(&z, "xl/_rels/workbook.xml.rels");
    let cache = &book_rels.iter().find(|r| r.0 == rid && r.1 == REL_SLICER_CACHE).unwrap().2;
    let cache_xml = z.read_string(&format!("xl/{cache}")).unwrap();
    assert!(cache_xml.contains(r#"name="Slicer_Region""#), "{cache_xml}");
    let (_, table) = table_part(&z, "Sales").unwrap();
    let region = table.split("<tableColumn ").find(|c| c.contains(r#"name="Region""#)).unwrap();
    let tsc = cache_xml.split("<x15:tableSlicerCache").nth(1).unwrap();
    assert_eq!(attr(tsc, "tableId"), attr(&table, "id"), "{cache_xml}");
    assert_eq!(attr(tsc, "column"), attr(&format!(" {region}"), "id"), "{cache_xml}");
    let sheet = z.read_string("xl/worksheets/sheet2.xml").unwrap();
    assert_eq!(sheet.matches("{3A4CF648-6AED-40f4-86FF-DC5316D8AED3}").count(), 1, "{sheet}");
    let sheet_rels = rels_of(&z, "xl/worksheets/_rels/sheet2.xml.rels");
    let target = |ty: &str, id: &str| sheet_rels.iter().find(|r| r.0 == id && r.1.ends_with(ty)).map(|r| r.2.replace("../", "xl/"));
    let sid = attr(&format!(" {}", sheet.split("<x14:slicer ").nth(1).unwrap()), "r:id");
    let slicer = z.read_string(&target("/slicer", &sid).unwrap()).unwrap();
    assert!(slicer.contains(r#"<slicer name="Region" cache="Slicer_Region""#), "{slicer}");
    let did = attr(&format!(" {}", sheet.split("<drawing ").nth(1).unwrap()), "r:id");
    let drawing = z.read_string(&target("/drawing", &did).unwrap()).unwrap();
    assert_eq!(drawing.matches(r#"<sle:slicer xmlns:sle="http://schemas.microsoft.com/office/drawing/2010/slicer" name="Region"/>"#).count(), 1, "{drawing}");
    assert!(drawing.contains("Requires=\"sle15\""));
}

fn save(name: &str, data: &[u8]) {
    if let Ok(dir) = std::env::var("APEX_PIVOT_OUT") {
        std::fs::write(format!("{dir}/{name}"), data).unwrap();
    }
}

#[test]
fn excel_table_slicer_kept_and_relinked() {
    let pkg = with_table_slicer(&book());
    check_table_slicer(&pkg);
    let wb = apex_io::xlsx::read(&pkg).unwrap();
    assert!(wb.analysis.slicers.is_empty());
    assert_eq!(wb.sheets[0].tables.len(), 2);
    let out = apex_io::xlsx::write(&wb);
    check_table_slicer(&out);
    save("table_slicer.xlsx", &out);
    // Sales is now table 2, Region its column 1.
    let z = ZipReader::new(&out).unwrap();
    let cache = z.read_string("xl/slicerCaches/slicerCache1.xml").unwrap();
    assert!(cache.contains(r#"<x15:tableSlicerCache tableId="2" column="1"/>"#), "{cache}");
    assert_eq!(z.read("xl/slicers/slicer1.xml").unwrap(), ZipReader::new(&pkg).unwrap().read("xl/slicers/slicer1.xml").unwrap());
    // Twice.
    let back = apex_io::xlsx::read(&out).unwrap();
    let again = apex_io::xlsx::write(&back);
    check_table_slicer(&again);
    // Without table Other, Sales becomes table 1; renamed, it still is it.
    let mut fewer = apex_io::xlsx::read(&again).unwrap();
    fewer.sheets[0].tables.retain(|t| t.name != "Other");
    let id = fewer.find_table("Sales").unwrap();
    fewer.rename_table(id, "Ventas").unwrap();
    let out = apex_io::xlsx::write(&fewer);
    let z = ZipReader::new(&out).unwrap();
    assert!(z.read_string("xl/slicerCaches/slicerCache1.xml").unwrap().contains(r#"<x15:tableSlicerCache tableId="1" column="1"/>"#));
    assert!(z.read_string("xl/workbook.xml").unwrap().contains("Slicer_Region"));
}

#[test]
fn excel_table_slicer_goes_with_its_table() {
    let pkg = with_table_slicer(&book());
    for twice in [false, true] {
        let mut wb = apex_io::xlsx::read(&pkg).unwrap();
        if twice {
            wb = apex_io::xlsx::read(&apex_io::xlsx::write(&wb)).unwrap();
        }
        wb.sheets[0].tables.retain(|t| t.name != "Sales");
        let out = apex_io::xlsx::write(&wb);
        check_package(&out);
        save("table_slicer_gone.xlsx", &out);
        let z = ZipReader::new(&out).unwrap();
        assert_eq!(z.names().filter(|n| n.contains("slicer")).count(), 0);
        let book = z.read_string("xl/workbook.xml").unwrap();
        assert!(!book.contains("Slicer_Region") && !book.contains("46BE6895") && !book.contains("slicerCache"), "{book}");
        let sheet = z.read_string("xl/worksheets/sheet2.xml").unwrap();
        assert!(!sheet.contains("3A4CF648") && !sheet.contains("slicer") && !sheet.contains("<drawing "), "{sheet}");
        for n in z.names().filter(|n| n.ends_with(".rels")) {
            assert!(!z.read_string(n).unwrap().contains("slicer"), "{n}");
        }
        assert!(apex_io::xlsx::read(&out).unwrap().names.iter().all(|d| d.name != "Slicer_Region"));
    }
}
