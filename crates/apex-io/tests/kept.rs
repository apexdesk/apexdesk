//! Parts ApexDesk doesn't edit but keeps: connections, queries, the data
//! model, pivot tables, custom XML and sensitivity labels.

use apex_doc::{CustomProp, Document, Paragraph, PropValue};
use apex_io::xml::{Ev, Reader};
use apex_io::zip::{ZipReader, ZipWriter};
use apex_sheet::table::Table;
use apex_sheet::{CellRef, Range, Workbook};

const LABEL: &str = "f42aa342-8706-4288-bd11-ebb85995028c";
const X15: &str = "http://schemas.microsoft.com/office/spreadsheetml/2010/11/main";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// A package with some parts replaced or added (`f` edits the text of
/// existing XML parts).
fn repack(data: &[u8], add: &[(&str, Vec<u8>)], f: &dyn Fn(&str, String) -> String) -> Vec<u8> {
    let z = ZipReader::new(data).unwrap();
    let mut w = ZipWriter::new();
    let names: Vec<String> = z.names().map(String::from).collect();
    for n in &names {
        if add.iter().any(|(p, _)| p == n) {
            continue;
        }
        let b = z.read(n).unwrap();
        match String::from_utf8(b.clone()) {
            Ok(s) if n.ends_with(".xml") || n.ends_with(".rels") => w.add(n, f(n, s).as_bytes(), true),
            _ => w.add(n, &b, true),
        }
    }
    for (p, b) in add {
        w.add(p, b, true);
    }
    w.finish()
}

fn before(s: String, tag: &str, what: &str) -> String {
    let i = s.rfind(tag).unwrap_or_else(|| panic!("{tag} in {s}"));
    format!("{}{what}{}", &s[..i], &s[i..])
}

fn rels(body: &str) -> Vec<u8> {
    format!(r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{body}</Relationships>"#).into_bytes()
}

fn rel(id: &str, ty: &str, target: &str) -> String {
    format!(r#"<Relationship Id="{id}" Type="{ty}" Target="{target}"/>"#)
}

fn ct(path: &str, ty: &str) -> String {
    format!(r#"<Override PartName="/{path}" ContentType="{ty}"/>"#)
}

/// A `DataMashup` part with one query.
fn mashup() -> Vec<u8> {
    let mut z = ZipWriter::new();
    z.add("Formulas/Section1.m", b"section Section1;\r\n\r\nshared Ventas = let\r\n    Origen = Csv.Document(File.Contents(\"C:\\datos\\ventas.csv\"))\r\nin\r\n    Origen;", true);
    let pkg = z.finish();
    let mut bin = vec![0u8; 4];
    bin.extend((pkg.len() as u32).to_le_bytes());
    bin.extend(&pkg);
    bin.extend([0u8; 16]);
    let b64 = base64(&bin);
    // Excel writes it in UTF-16.
    let x = format!(r#"<?xml version="1.0" encoding="utf-16"?><DataMashup xmlns="http://schemas.microsoft.com/DataMashup">{b64}</DataMashup>"#);
    [0xFF, 0xFE].into_iter().chain(x.encode_utf16().flat_map(u16::to_le_bytes)).collect()
}

fn base64(data: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            if k <= c.len() {
                out.push(A[(n >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn label_props() -> Vec<CustomProp> {
    let p = |f: &str, v: &str| CustomProp { name: format!("MSIP_Label_{LABEL}_{f}"), value: PropValue::Text(v.into()) };
    vec![p("Enabled", "true"), p("SetDate", "2026-09-01T10:00:00Z"), p("Method", "Privileged"), p("Name", "Confidencial"), p("SiteId", "72f988bf-86f1-41af-91ab-2d7cd011db47")]
}

fn label_info() -> Vec<u8> {
    format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><clbl:labelList xmlns:clbl="http://schemas.microsoft.com/office/2020/mipLabelMetadata"><clbl:label id="{{{}}}" enabled="1" method="Privileged" siteId="{{72f988bf-86f1-41af-91ab-2d7cd011db47}}" removed="0"/></clbl:labelList>"#, LABEL.to_uppercase()).into_bytes()
}

const MODEL: &[u8] = &[0x00, 0x01, 0x02, 0xFF, 0xFE, 0x10, 0x20, 0x30];
const CONNECTIONS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><connections xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><connection id="1" keepAlive="1" name="Consulta - Ventas" description="Conexión a la consulta Ventas" type="5" refreshedVersion="8" background="1" saveData="1"><dbPr connection="Provider=Microsoft.Mashup.OleDb.1;Data Source=$Workbook$;Location=Ventas;" command="SELECT * FROM [Ventas]"/></connection><connection id="2" name="ThisWorkbookDataModel" type="5" refreshedVersion="8" model="1"><dbPr connection="Data Model Connection" command="Model" commandType="1"/></connection></connections>"#;
const QUERY_TABLE: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><queryTable xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" name="Ventas" connectionId="1" autoFormatId="16" applyNumberFormats="0" applyBorderFormats="0" applyFontFormats="0" applyPatternFormats="0" applyAlignmentFormats="0" applyWidthHeightFormats="0"><queryTableRefresh nextId="4"><queryTableFields count="2"><queryTableField id="1" name="Producto" tableColumnId="1"/><queryTableField id="3" name="Importe" tableColumnId="3"/></queryTableFields></queryTableRefresh></queryTable>"#;
const CACHE: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><pivotCacheDefinition xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" r:id="rId1" refreshOnLoad="1" recordCount="2"><cacheSource type="worksheet"><worksheetSource name="Ventas"/></cacheSource><cacheFields count="1"><cacheField name="Producto" numFmtId="0"><sharedItems/></cacheField></cacheFields></pivotCacheDefinition>"#;
const RECORDS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><pivotCacheRecords xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="0"/>"#;
const PIVOT: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><pivotTableDefinition xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" name="Dinámica1" cacheId="7" dataCaption="Valores"><location ref="A3:A5" firstHeaderRow="1" firstDataRow="1" firstDataCol="0"/><pivotFields count="1"><pivotField axis="axisRow" showAll="0"><items count="1"><item t="default"/></items></pivotField></pivotFields></pivotTableDefinition>"#;

/// A workbook with a table filled by a Power Query query, the data model,
/// a pivot table on its second sheet and a sensitivity label.
fn sample_xlsx() -> Vec<u8> {
    let mut wb = Workbook::new();
    wb.sheets.push(apex_sheet::Sheet::new("Resumen"));
    wb.sheets[0].name = "Datos".into();
    wb.sheets[1].name = "Resumen".into();
    for (r, row) in [["Producto", "Importe"], ["Pan", "2"], ["Vino", "9"]].iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            wb.set_input(0, CellRef::new(r as u32, c as u32), v);
        }
    }
    wb.sheets[0].tables.push(Table::new(1, "Ventas", Range::parse("A1:B3").unwrap(), vec!["Producto".into(), "Importe".into()]));
    wb.meta.custom = label_props();
    let base = apex_io::xlsx::write(&wb);
    let main = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    let add: Vec<(&str, Vec<u8>)> = vec![
        ("xl/connections.xml", CONNECTIONS.as_bytes().to_vec()),
        ("xl/model/item.data", MODEL.to_vec()),
        ("xl/queryTables/queryTable1.xml", QUERY_TABLE.as_bytes().to_vec()),
        ("xl/tables/_rels/table1.xml.rels", rels(&rel("rId1", &format!("{main}/queryTable"), "../queryTables/queryTable1.xml"))),
        ("xl/pivotCache/pivotCacheDefinition1.xml", CACHE.as_bytes().to_vec()),
        ("xl/pivotCache/_rels/pivotCacheDefinition1.xml.rels", rels(&rel("rId1", &format!("{main}/pivotCacheRecords"), "pivotCacheRecords1.xml"))),
        ("xl/pivotCache/pivotCacheRecords1.xml", RECORDS.as_bytes().to_vec()),
        ("xl/pivotTables/pivotTable1.xml", PIVOT.as_bytes().to_vec()),
        ("xl/pivotTables/_rels/pivotTable1.xml.rels", rels(&rel("rId1", &format!("{main}/pivotCacheDefinition"), "../pivotCache/pivotCacheDefinition1.xml"))),
        ("xl/worksheets/_rels/sheet2.xml.rels", rels(&rel("rId1", &format!("{main}/pivotTable"), "../pivotTables/pivotTable1.xml"))),
        ("customXml/item1.xml", mashup()),
        ("customXml/itemProps1.xml", br#"<?xml version="1.0" encoding="UTF-8" standalone="no"?><ds:datastoreItem ds:itemID="{11111111-2222-3333-4444-555555555555}" xmlns:ds="http://schemas.openxmlformats.org/officeDocument/2006/customXml"><ds:schemaRefs/></ds:datastoreItem>"#.to_vec()),
        ("customXml/_rels/item1.xml.rels", rels(&rel("rId1", &format!("{main}/customXmlProps"), "itemProps1.xml"))),
        ("docMetadata/LabelInfo.xml", label_info()),
    ];
    repack(&base, &add, &|name, s| match name {
        "[Content_Types].xml" => before(
            s,
            "</Types>",
            &[
                ct("xl/connections.xml", "application/vnd.openxmlformats-officedocument.spreadsheetml.connections+xml"),
                ct("xl/model/item.data", "application/vnd.openxmlformats-officedocument.model+data"),
                ct("xl/queryTables/queryTable1.xml", "application/vnd.openxmlformats-officedocument.spreadsheetml.queryTable+xml"),
                ct("xl/pivotCache/pivotCacheDefinition1.xml", "application/vnd.openxmlformats-officedocument.spreadsheetml.pivotCacheDefinition+xml"),
                ct("xl/pivotCache/pivotCacheRecords1.xml", "application/vnd.openxmlformats-officedocument.spreadsheetml.pivotCacheRecords+xml"),
                ct("xl/pivotTables/pivotTable1.xml", "application/vnd.openxmlformats-officedocument.spreadsheetml.pivotTable+xml"),
                ct("customXml/itemProps1.xml", "application/vnd.openxmlformats-officedocument.customXmlProperties+xml"),
                ct("docMetadata/LabelInfo.xml", "application/vnd.ms-office.classificationlabels+xml"),
            ]
            .concat(),
        ),
        "_rels/.rels" => before(s, "</Relationships>", &rel("rId9", "http://schemas.microsoft.com/office/2020/02/relationships/classificationlabels", "docMetadata/LabelInfo.xml")),
        "xl/_rels/workbook.xml.rels" => before(
            s,
            "</Relationships>",
            &[
                rel("rId90", &format!("{main}/connections"), "connections.xml"),
                rel("rId91", &format!("{main}/powerPivotData"), "model/item.data"),
                rel("rId92", &format!("{main}/pivotCacheDefinition"), "pivotCache/pivotCacheDefinition1.xml"),
                rel("rId93", &format!("{main}/customXml"), "../customXml/item1.xml"),
            ]
            .concat(),
        ),
        "xl/workbook.xml" => {
            let s = s.replacen("<workbook ", &format!(r#"<workbook xmlns:x15="{X15}" "#), 1);
            before(s, "</workbook>", r#"<pivotCaches><pivotCache cacheId="7" r:id="rId92"/></pivotCaches><extLst><ext uri="{FCE2AD5D-F65C-4FA6-A056-5C36A1767C68}"><x15:dataModel><x15:modelTables><x15:modelTable id="Ventas" name="Ventas" connection="Consulta - Ventas"/></x15:modelTables></x15:dataModel></ext></extLst>"#)
        }
        "xl/tables/table1.xml" => s
            .replacen(r#" ref="A1:B3""#, r#" ref="A1:B3" tableType="queryTable""#, 1)
            .replacen(r#"<tableColumn id="1" name="Producto""#, r#"<tableColumn id="1" uniqueName="1" name="Producto" queryTableFieldId="1""#, 1)
            .replacen(r#"<tableColumn id="2" name="Importe""#, r#"<tableColumn id="3" uniqueName="3" name="Importe" queryTableFieldId="3""#, 1),
        _ => s,
    })
}

/// Checks that every relationship points at a part of the package, that
/// every part has a content type and that no override names a missing
/// part. Returns the parts.
fn check_package(data: &[u8]) -> Vec<String> {
    let z = ZipReader::new(data).unwrap();
    let names: Vec<String> = z.names().map(String::from).collect();
    let cts = z.read_string("[Content_Types].xml").unwrap();
    let (mut defaults, mut overrides) = (Vec::new(), Vec::new());
    let mut r = Reader::new(&cts);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Empty(t) | Ev::Start(t) if t.local() == "Default" => defaults.push(t.attr("Extension").unwrap().to_lowercase()),
            Ev::Empty(t) | Ev::Start(t) if t.local() == "Override" => overrides.push(t.attr("PartName").unwrap().trim_start_matches('/').to_string()),
            _ => {}
        }
    }
    let mut seen = std::collections::HashSet::new();
    for o in &overrides {
        assert!(names.contains(o), "override of a missing part {o}");
        assert!(seen.insert(o.clone()), "override twice: {o}");
    }
    for n in names.iter().filter(|n| *n != "[Content_Types].xml") {
        let ext = n.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
        assert!(overrides.contains(n) || defaults.contains(&ext), "no content type for {n}");
    }
    for n in names.iter().filter(|n| n.ends_with(".rels")) {
        let dir = n.trim_end_matches(".rels").rsplit_once("_rels/").map_or(String::new(), |(d, _)| d.trim_end_matches('/').to_string());
        let xml = z.read_string(n).unwrap();
        let mut ids = std::collections::HashSet::new();
        let mut r = Reader::new(&xml);
        loop {
            match r.next() {
                Ev::Eof => break,
                Ev::Empty(t) | Ev::Start(t) if t.local() == "Relationship" => {
                    assert!(ids.insert(t.attr("Id").unwrap().into_owned()), "id twice in {n}");
                    if t.attr("TargetMode").is_some_and(|m| m == "External") {
                        continue;
                    }
                    let target = t.attr("Target").unwrap();
                    let mut parts: Vec<&str> = if target.starts_with('/') { Vec::new() } else { dir.split('/').filter(|s| !s.is_empty()).collect() };
                    for seg in target.split('/') {
                        match seg {
                            ".." => {
                                parts.pop();
                            }
                            "" | "." => {}
                            s => parts.push(s),
                        }
                    }
                    let path = parts.join("/");
                    assert!(names.contains(&path), "{n} points at a missing part {path}");
                }
                _ => {}
            }
        }
    }
    names
}

fn part(data: &[u8], name: &str) -> Vec<u8> {
    ZipReader::new(data).unwrap().read(name).unwrap_or_else(|_| panic!("missing {name}"))
}

fn text(data: &[u8], name: &str) -> String {
    String::from_utf8(part(data, name)).unwrap()
}

#[test]
fn xlsx_keeps_external_data() {
    let file = sample_xlsx();
    check_package(&file);
    let wb = apex_io::xlsx::read(&file).unwrap();
    let ext = apex_io::external_data(&wb);
    assert_eq!(ext.connections.len(), 2);
    assert_eq!(ext.connections[0].name, "Consulta - Ventas");
    assert_eq!(ext.connections[0].description, "Conexión a la consulta Ventas");
    assert!(ext.connections[0].source.contains("Location=Ventas"));
    assert_eq!(ext.connections[0].command, "SELECT * FROM [Ventas]");
    assert_eq!(ext.queries.len(), 1);
    assert_eq!(ext.queries[0].name, "Ventas");
    assert!(ext.queries[0].formula.contains("ventas.csv"));
    assert!(ext.data_model);
    assert_eq!(ext.pivot_tables, 1);
    let labels = wb.kept.sensitivity_labels(&wb.meta);
    assert_eq!(labels.len(), 1);
    assert_eq!(labels[0].name, "Confidencial");

    let out = apex_io::xlsx::write(&wb);
    let names = check_package(&out);
    for p in ["xl/connections.xml", "xl/model/item.data", "xl/queryTables/queryTable1.xml", "xl/pivotCache/pivotCacheDefinition1.xml", "xl/pivotCache/pivotCacheRecords1.xml", "xl/pivotTables/pivotTable1.xml", "docMetadata/LabelInfo.xml"] {
        assert_eq!(part(&out, p), part(&file, p), "{p}");
    }
    // The Power Query part, under the name it got.
    let item = names.iter().find(|n| n.starts_with("customXml/item") && !n.contains("Props")).unwrap();
    assert_eq!(part(&out, item), mashup());
    let book = text(&out, "xl/workbook.xml");
    assert!(book.contains(r#"<pivotCache cacheId="7" r:id="rIdK"#), "{book}");
    assert!(book.contains("<x15:dataModel>") && book.contains(&format!(r#"xmlns:x15="{X15}""#)), "{book}");
    assert!(book.find("<pivotCaches>").unwrap() < book.find("<extLst>").unwrap());
    let table = text(&out, "xl/tables/table1.xml");
    assert!(table.contains(r#"tableType="queryTable""#) && table.contains(r#"<tableColumn id="3" uniqueName="3" queryTableFieldId="3" name="Importe""#), "{table}");
    assert!(text(&out, "xl/worksheets/_rels/sheet2.xml.rels").contains("pivotTables/pivotTable1.xml"));
    assert!(text(&out, "_rels/.rels").contains("docMetadata/LabelInfo.xml"));
    assert!(text(&out, "docProps/custom.xml").contains(&format!("MSIP_Label_{LABEL}_Name")));
    // And again: nothing is lost on a second save.
    let again = apex_io::xlsx::write(&apex_io::xlsx::read(&out).unwrap());
    let names2 = check_package(&again);
    assert_eq!(names.len(), names2.len());
    assert_eq!(apex_io::external_data(&apex_io::xlsx::read(&again).unwrap()), ext);
}

#[test]
fn xlsx_drops_orphans() {
    let wb = apex_io::xlsx::read(&sample_xlsx()).unwrap();
    // Without the sheet of the pivot table, its cache goes too.
    let mut w = wb.clone();
    w.sheets.remove(1);
    let out = apex_io::xlsx::write(&w);
    let names = check_package(&out);
    assert!(!names.iter().any(|n| n.contains("pivot")), "{names:?}");
    assert!(!text(&out, "xl/workbook.xml").contains("pivotCache"));
    assert!(names.contains(&"xl/connections.xml".to_string()) && names.contains(&"xl/model/item.data".to_string()));
    // Without the table, its query goes.
    let mut w = wb.clone();
    w.sheets[0].tables.clear();
    let names = check_package(&apex_io::xlsx::write(&w));
    assert!(!names.iter().any(|n| n.contains("queryTable")));
    // A table whose columns changed is no longer the query's.
    let mut w = wb.clone();
    w.sheets[0].tables[0].columns[1].name = "Precio".into();
    let out = apex_io::xlsx::write(&w);
    let names = check_package(&out);
    assert!(!names.iter().any(|n| n.contains("queryTable")));
    assert!(!text(&out, "xl/tables/table1.xml").contains("queryTable"));
    // A copied sheet doesn't take the pivot table along.
    let mut w = wb.clone();
    let copy = w.sheets[1].clone();
    w.sheets.push(copy);
    w.sheets[2].name = "Copia".into();
    let out = apex_io::xlsx::write(&w);
    check_package(&out);
    assert!(!ZipReader::new(&out).unwrap().contains("xl/worksheets/_rels/sheet3.xml.rels"));
}

#[test]
fn docx_keeps_custom_xml_and_labels() {
    let mut doc = Document::from_paragraphs(vec![Paragraph::new("Hola")]);
    doc.meta.custom = label_props();
    doc.biblio.sources = vec![apex_doc::Source { tag: "Doe20".into(), title: "Libro".into(), authors: "Doe, John".into(), year: "2020".into(), ..Default::default() }];
    let base = apex_io::docx::write(&doc);
    let main = R;
    let custom = br#"<?xml version="1.0" encoding="UTF-8"?><datos xmlns="urn:ejemplo"><cliente>ACME</cliente></datos>"#.to_vec();
    let props = br#"<?xml version="1.0" encoding="UTF-8" standalone="no"?><ds:datastoreItem ds:itemID="{AAAAAAAA-2222-3333-4444-555555555555}" xmlns:ds="http://schemas.openxmlformats.org/officeDocument/2006/customXml"><ds:schemaRefs><ds:schemaRef ds:uri="urn:ejemplo"/></ds:schemaRefs></ds:datastoreItem>"#.to_vec();
    let add: Vec<(&str, Vec<u8>)> = vec![
        ("customXml/item5.xml", custom.clone()),
        ("customXml/itemProps5.xml", props.clone()),
        ("customXml/_rels/item5.xml.rels", rels(&rel("rId1", &format!("{main}/customXmlProps"), "itemProps5.xml"))),
        ("docMetadata/LabelInfo.xml", label_info()),
    ];
    let file = repack(&base, &add, &|name, s| match name {
        "[Content_Types].xml" => before(s, "</Types>", &[ct("customXml/itemProps5.xml", "application/vnd.openxmlformats-officedocument.customXmlProperties+xml"), ct("docMetadata/LabelInfo.xml", "application/vnd.ms-office.classificationlabels+xml")].concat()),
        "_rels/.rels" => before(s, "</Relationships>", &rel("rId9", "http://schemas.microsoft.com/office/2020/02/relationships/classificationlabels", "docMetadata/LabelInfo.xml")),
        "word/_rels/document.xml.rels" => before(s, "</Relationships>", &rel("rId77", &format!("{main}/customXml"), "../customXml/item5.xml")),
        _ => s,
    });
    check_package(&file);
    let back = apex_io::docx::read(&file).unwrap();
    assert_eq!(back.biblio.sources.len(), 1);
    assert_eq!(back.kept.sensitivity_labels(&back.meta)[0].name, "Confidencial");
    let out = apex_io::docx::write(&back);
    let names = check_package(&out);
    // The bibliography is written once, from the sources.
    let items: Vec<&String> = names.iter().filter(|n| n.starts_with("customXml/item") && !n.contains("Props")).collect();
    assert_eq!(items.len(), 2, "{names:?}");
    let other = items.iter().find(|n| part(&out, n) == custom).expect("custom XML kept");
    let props_name = other.replace("item", "itemProps");
    assert_eq!(part(&out, &props_name), props);
    assert_eq!(apex_io::docx::read(&out).unwrap().biblio.sources, back.biblio.sources);
    assert_eq!(part(&out, "docMetadata/LabelInfo.xml"), label_info());
    // Twice.
    let again = apex_io::docx::write(&apex_io::docx::read(&out).unwrap());
    assert_eq!(check_package(&again).len(), names.len());
}

#[test]
fn odt_keeps_label_properties() {
    let mut doc = Document::from_paragraphs(vec![Paragraph::new("Hola")]);
    doc.meta.custom = label_props();
    let back = apex_io::odt::read(&apex_io::odt::write(&doc)).unwrap();
    assert_eq!(back.meta.sensitivity_labels()[0].name, "Confidencial");
    let mut wb = Workbook::new();
    wb.meta.custom = label_props();
    let back = apex_io::ods::read(&apex_io::ods::write(&wb)).unwrap();
    assert_eq!(back.meta.sensitivity_labels()[0].name, "Confidencial");
}

/// Two controls bound to the same custom XML node, one to an attribute,
/// a check box and one to the document title.
fn bound_docx() -> Vec<u8> {
    use apex_doc::{Attrs, ContentControl, ControlKind, DataBinding, ListItem};
    use std::sync::Arc;
    let store = "{AAAAAAAA-2222-3333-4444-555555555555}";
    let b = |xpath: &str| Some(DataBinding { xpath: xpath.into(), store_item: store.into(), prefixes: "xmlns:ns0='urn:ejemplo'".into() });
    let ctl = |id: u32, kind: ControlKind, binding: Option<DataBinding>| Some(Arc::new(ContentControl { binding, ..ContentControl::new(id, kind) }));
    let para = |label: &str, text: &str, c: Option<Arc<ContentControl>>| {
        let mut p = Paragraph::new("");
        p.push(label, Attrs::default());
        p.push(text, Attrs { control: c, ..Default::default() });
        p
    };
    let items = vec![ListItem { text: "Abierto".into(), value: "a".into() }, ListItem { text: "Bloqueado".into(), value: "b".into() }];
    let title = DataBinding { xpath: "/ns1:coreProperties[1]/ns0:title[1]".into(), store_item: "{6C3C8BC8-F283-45AE-878A-BAB7291924A1}".into(), prefixes: "xmlns:ns0='http://purl.org/dc/elements/1.1/' xmlns:ns1='http://schemas.openxmlformats.org/package/2006/metadata/core-properties'".into() };
    let mut doc = Document::from_paragraphs(vec![
        para("Cliente: ", "viejo", ctl(1, ControlKind::PlainText, b("/ns0:datos[1]/ns0:cliente[1]"))),
        para("Otra vez: ", "viejo", ctl(2, ControlKind::PlainText, b("/ns0:datos[1]/ns0:cliente[1]"))),
        para("Pagado: ", "☐", ctl(3, ControlKind::CheckBox { checked: false }, b("/ns0:datos[1]/ns0:pagado[1]"))),
        para("Estado: ", "Abierto", ctl(4, ControlKind::List { items, editable: false }, b("/ns0:datos[1]/@estado"))),
        para("Título: ", "Informe", ctl(5, ControlKind::PlainText, Some(title))),
    ]);
    doc.meta.title = "Informe".into();
    let base = apex_io::docx::write(&doc);
    let xml = r#"<?xml version="1.0" encoding="utf-16" standalone="no"?><ns0:datos xmlns:ns0="urn:ejemplo" estado="b"><ns0:cliente>ACME</ns0:cliente><ns0:pagado>true</ns0:pagado></ns0:datos>"#;
    let utf16: Vec<u8> = [0xFF, 0xFE].into_iter().chain(xml.encode_utf16().flat_map(u16::to_le_bytes)).collect();
    let props = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?><ds:datastoreItem ds:itemID="{store}" xmlns:ds="http://schemas.openxmlformats.org/officeDocument/2006/customXml"><ds:schemaRefs><ds:schemaRef ds:uri="urn:ejemplo"/></ds:schemaRefs></ds:datastoreItem>"#);
    let add: Vec<(&str, Vec<u8>)> = vec![("customXml/item1.xml", utf16), ("customXml/itemProps1.xml", props.into_bytes()), ("customXml/_rels/item1.xml.rels", rels(&rel("rId1", &format!("{R}/customXmlProps"), "itemProps1.xml")))];
    repack(&base, &add, &|name, s| match name {
        "[Content_Types].xml" => before(s, "</Types>", &ct("customXml/itemProps1.xml", "application/vnd.openxmlformats-officedocument.customXmlProperties+xml")),
        "word/_rels/document.xml.rels" => before(s, "</Relationships>", &rel("rId77", &format!("{R}/customXml"), "../customXml/item1.xml")),
        "docProps/core.xml" => s.replace(">Informe<", ">Informe 2026<"),
        _ => s,
    })
}

fn control_texts(d: &Document) -> Vec<String> {
    d.controls().into_iter().map(|c| d.paragraphs[c.para].text[c.range].to_string()).collect()
}

fn custom_item(data: &[u8]) -> String {
    let z = ZipReader::new(data).unwrap();
    let name = z.names().find(|n| n.starts_with("customXml/item") && !n.contains("Props") && n.ends_with(".xml")).unwrap().to_string();
    let b = z.read(&name).unwrap();
    assert_eq!(&b[..2], &[0xFF, 0xFE], "stays UTF-16");
    let units: Vec<u16> = b[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16(&units).unwrap()
}

#[test]
fn docx_controls_follow_custom_xml() {
    let file = bound_docx();
    check_package(&file);
    let mut doc = apex_io::docx::read(&file).unwrap();
    // The data wins over the text saved in the controls.
    assert_eq!(control_texts(&doc), ["ACME", "ACME", "☒", "Bloqueado", "Informe 2026"]);
    assert!(!doc.modified && !doc.can_undo());
    // Editing one control changes the node and its twin.
    let at = doc.controls()[1].clone();
    let c = (*at.control).clone();
    doc.set_control(&at, c, "Nueva SA", at.sel());
    assert!(apex_io::docx_bind::sync(&mut doc, &|_| "Escriba aquí".into()));
    assert_eq!(control_texts(&doc)[..2], ["Nueva SA", "Nueva SA"]);
    assert!(!apex_io::docx_bind::sync(&mut doc, &|_| String::new()));
    let at = doc.controls()[4].clone();
    doc.set_control(&at, (*at.control).clone(), "Título nuevo", at.sel());
    let at = doc.controls()[3].clone();
    let items = match &at.control.kind {
        apex_doc::ControlKind::List { items, .. } => items.clone(),
        _ => unreachable!(),
    };
    doc.set_control(&at, (*at.control).clone(), &items[0].text, at.sel());
    apex_io::docx_bind::sync(&mut doc, &|_| String::new());
    assert_eq!(doc.meta.title, "Título nuevo");
    let out = apex_io::docx::write(&doc);
    check_package(&out);
    let xml = custom_item(&out);
    assert!(xml.contains("<ns0:cliente>Nueva SA</ns0:cliente>") && xml.contains(r#"estado="a""#), "{xml}");
    assert!(text(&out, "docProps/core.xml").contains("Título nuevo"));
    let back = apex_io::docx::read(&out).unwrap();
    assert_eq!(control_texts(&back), ["Nueva SA", "Nueva SA", "☒", "Abierto", "Título nuevo"]);
    // Undoing an edit brings the data and the twin back too.
    let mut d = apex_io::docx::read(&file).unwrap();
    let at = d.controls()[0].clone();
    d.set_control(&at, (*at.control).clone(), "Otra", at.sel());
    apex_io::docx_bind::sync(&mut d, &|_| String::new());
    d.undo();
    apex_io::docx_bind::sync(&mut d, &|_| String::new());
    assert_eq!(control_texts(&d)[..2], ["ACME", "ACME"]);
    assert!(custom_item(&apex_io::docx::write(&d)).contains("<ns0:cliente>ACME</ns0:cliente>"));
    // Emptied: the twin shows its placeholder and the node is empty.
    let at = d.controls()[0].clone();
    d.set_control(&at, apex_doc::ContentControl { placeholder: true, ..(*at.control).clone() }, "Escriba aquí", at.sel());
    apex_io::docx_bind::sync(&mut d, &|_| "Escriba aquí".into());
    assert_eq!(control_texts(&d)[..2], ["Escriba aquí", "Escriba aquí"]);
    assert!(d.controls()[1].control.placeholder);
    assert!(custom_item(&apex_io::docx::write(&d)).contains("<ns0:cliente></ns0:cliente>"));
}

#[test]
fn docproperty_fields_round_trip() {
    use apex_doc::{Field, FieldKind, Pos, Selection};
    let mut d = Document::from_paragraphs(vec![Paragraph::new("Company: ")]);
    d.meta.company = "ACME".into();
    d.meta.custom.push(CustomProp { name: "Contrato".into(), value: PropValue::Text("C-7".into()) });
    // Saving stores the shown values as the fields' results.
    d.insert_field(Selection::caret(Pos::new(0, 9)), Field { cached: "ACME".into(), ..Field::new(FieldKind::DocProperty("Company".into())) });
    d.insert_field(Selection::caret(Pos::new(0, 0)), Field { cached: "C-7".into(), ..Field::new(FieldKind::DocProperty("Contrato".into())) });
    let want = [FieldKind::DocProperty("Contrato".into()), FieldKind::DocProperty("Company".into())];
    for back in [apex_io::docx::read(&apex_io::docx::write(&d)).unwrap(), apex_io::odt::read(&apex_io::odt::write(&d)).unwrap()] {
        let kinds: Vec<FieldKind> = back.fields().iter().map(|(_, f)| f.kind.clone()).collect();
        assert_eq!(kinds, want);
        assert_eq!(back.meta.property("contrato").as_deref(), Some("C-7"));
    }
}

#[test]
fn assembled_controls_reach_custom_xml() {
    let doc = apex_io::docx::read(&bound_docx()).unwrap();
    assert!(apex_doc::assemble::form_fields(&doc).iter().any(|n| n == "cliente"));
    let filled = apex_doc::merge::fill(&doc, &["Cliente".to_string()], &["Zeta SL".to_string()]);
    assert_eq!(control_texts(&filled)[..2], ["Zeta SL", "Zeta SL"]);
    let out = apex_io::docx::write(&filled);
    check_package(&out);
    assert!(custom_item(&out).contains("<ns0:cliente>Zeta SL</ns0:cliente>"));
    assert_eq!(control_texts(&apex_io::docx::read(&out).unwrap())[..2], ["Zeta SL", "Zeta SL"]);
}

const ODF_NS: &str =r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:chart="urn:oasis:names:tc:opendocument:xmlns:chart:1.0""#;

/// An odt with a chart in a paragraph and a picture anchored to the page.
fn odt_with_chart() -> Vec<u8> {
    let png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR\0\0\0\x10\0\0\0\x10\x08\x06\0\0\0".to_vec();
    let frame = |name: &str, anchor: &str, inner: &str| format!(r#"<draw:frame draw:name="{name}" text:anchor-type="{anchor}" svg:x="1in" svg:y="5in" svg:width="4in" svg:height="3in">{inner}</draw:frame>"#);
    let chart = frame("Chart", "as-char", r#"<draw:object xlink:href="./Object 1" xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad"/><draw:image xlink:href="./ObjectReplacements/Object 1" xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad"/>"#);
    let pic = frame("Pic", "page", r#"<draw:image xlink:href="Pictures/a.png" xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad"/>"#);
    let content = format!(r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content {ODF_NS} office:version="1.3"><office:body><office:text>{pic}<text:p>A chart: {chart}</text:p></office:text></office:body></office:document-content>"#);
    let sub = format!(r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content {ODF_NS} office:version="1.3"><office:body><office:chart><chart:chart chart:class="chart:bar"/></office:chart></office:body></office:document-content>"#);
    let man = r#"<?xml version="1.0" encoding="UTF-8"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3"><manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.text"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="Object 1/" manifest:media-type="application/vnd.oasis.opendocument.chart"/><manifest:file-entry manifest:full-path="Object 1/content.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="ObjectReplacements/Object 1" manifest:media-type="image/png"/><manifest:file-entry manifest:full-path="Pictures/a.png" manifest:media-type="image/png"/></manifest:manifest>"#;
    let mut w = ZipWriter::new();
    w.add("mimetype", b"application/vnd.oasis.opendocument.text", false);
    w.add("META-INF/manifest.xml", man.as_bytes(), true);
    w.add("content.xml", content.as_bytes(), true);
    w.add("Object 1/content.xml", sub.as_bytes(), true);
    // LibreOffice's replacement pictures are metafiles of its own.
    w.add("ObjectReplacements/Object 1", b"VCLMTF\x01\x00metafile", true);
    w.add("Pictures/a.png", &png, true);
    w.finish()
}

/// The ProgIDs of a document's objects and the number of pictures
/// without one.
fn objects_of(d: &Document) -> (Vec<String>, usize) {
    let mut ids = Vec::new();
    let mut pictures = 0;
    for p in &d.paragraphs {
        for r in &p.runs {
            let Some(img) = r.attrs.image.as_ref() else { continue };
            match img.object.as_ref().and_then(|o| o.ole()) {
                Some(o) => ids.push(o.prog_id.clone()),
                None => pictures += 1,
            }
        }
    }
    (ids, pictures)
}

/// Charts in odt (sub-documents with their replacement picture) survive
/// saving in odt and docx and back, and pictures anchored to the page
/// are kept.
#[test]
fn odt_keeps_charts_and_page_pictures() {
    let d = apex_io::odt::read(&odt_with_chart()).unwrap();
    let chart = vec!["opendocument.ChartDocument.1".to_string()];
    assert_eq!(objects_of(&d), (chart.clone(), 1));
    let odt = apex_io::odt::write(&d);
    assert!(text(&odt, "content.xml").contains("<draw:object xlink:href"));
    assert_eq!(objects_of(&apex_io::odt::read(&odt).unwrap()), (chart.clone(), 1));
    let docx = apex_io::docx::read(&apex_io::docx::write(&d)).unwrap();
    assert_eq!(objects_of(&docx).0, chart);
    // The replacement picture is kept as it is.
    let preview = |d: &Document| d.paragraphs.iter().flat_map(|p| &p.runs).find_map(|r| r.attrs.image.as_ref().filter(|i| i.object.is_some()).map(|i| i.data.clone())).unwrap();
    assert!(preview(&docx).starts_with(b"VCLMTF"));
    // Back in odt, a sub-document again (not an OLE object).
    let odt = apex_io::odt::write(&docx);
    assert!(text(&odt, "content.xml").contains("<draw:object xlink:href"));
    let back = apex_io::odt::read(&odt).unwrap();
    assert_eq!(objects_of(&back).0, chart);
    let o = back.paragraphs.iter().flat_map(|p| &p.runs).find_map(|r| r.attrs.image.as_ref()?.object.as_ref()?.ole().cloned()).unwrap();
    let sub = ZipReader::new(&o.data).unwrap();
    assert!(sub.read_string("content.xml").unwrap().contains("chart:bar"));
}
