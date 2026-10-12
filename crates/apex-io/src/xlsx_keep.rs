//! External data of a workbook, kept as it was: connections
//! (`xl/connections.xml`), query tables and the tables they fill, Power
//! Query (a custom XML `DataMashup` part), the data model
//! (`xl/model/item.data`), pivot tables and their caches. Nothing is run;
//! parts whose sheet or table is gone are left out.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::kept::TableQuery;
use apex_doc::{KeptLink, KeptParts};
use apex_sheet::{Sheet, Workbook};

use crate::ooxml_keep::{ContentTypes, Reach, collect, read_rels, rel, relative};
use crate::xml::{Ev, HEADER, Reader, esc};
use crate::zip::{ZipReader, ZipWriter};

const REL_CONNECTIONS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/connections";
const REL_PIVOT_CACHE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/pivotCacheDefinition";
const REL_PIVOT_TABLE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/pivotTable";
const REL_QUERY_TABLE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/queryTable";
const NS_RELS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// Workbook extensions kept: the data model and the pivot cache lists of
/// Excel 2010 and 2013.
const KEEP_EXT: [&str; 4] = ["{FCE2AD5D-F65C-4FA6-A056-5C36A1767C68}", "{876F7934-8845-4945-9796-88D515C7AA90}", "{841E416B-1EF1-43b6-AB56-02D37102CBD5}", "{983426D0-5260-488c-9760-48F4B6AC55F4}"];

/// Workbook relationships kept (custom XML aside).
fn book_rel(ty: &str, target: &str) -> bool {
    matches!(ty, REL_CONNECTIONS | REL_PIVOT_CACHE | REL_PIVOT_TABLE) || ty.ends_with("/powerPivotData") || target.starts_with("xl/model/")
}

/// Reads what the workbook keeps; `sheets` are the parts of its sheets,
/// `pivots` the pivot table parts read as ApexDesk's (not kept).
pub(crate) fn read(z: &ZipReader, wbxml: &str, sheets: &[String], pivots: &HashSet<String>, wb: &mut Workbook) {
    let ct = ContentTypes::read(z);
    let mut kept = KeptParts::default();
    // Our Get Data queries and tests are read, not kept.
    crate::ooxml_keep::read_custom_xml(z, &ct, "xl/workbook.xml", &mut kept, &|x| crate::queries_xml::is_ours(x) || crate::checks_xml::is_ours(x));
    for (_, ty, target, external) in read_rels(z, "xl/workbook.xml") {
        if !external && ty == crate::ooxml_keep::REL_CUSTOM_XML
            && let Ok(x) = z.read_string(&target)
        {
            if crate::queries_xml::is_ours(&x) {
                wb.queries = crate::queries_xml::read(&x);
            } else if crate::checks_xml::is_ours(&x) {
                wb.checks = crate::checks_xml::read(&x);
            }
        }
    }
    for (id, ty, target, external) in read_rels(z, "xl/workbook.xml") {
        if !external && book_rel(&ty, &target) && collect(z, &ct, &target, &mut kept) {
            kept.links.push(KeptLink { rel_type: ty, target, id });
        }
    }
    read_book_xml(wbxml, &mut kept);
    for (i, path) in sheets.iter().enumerate() {
        let Some(sh) = wb.sheets.get_mut(i) else { break };
        for (id, ty, target, external) in read_rels(z, path) {
            if external {
                continue;
            }
            if ty == REL_PIVOT_TABLE && pivots.contains(&target) {
                continue;
            }
            if (ty == REL_PIVOT_TABLE || ty == REL_QUERY_TABLE) && collect(z, &ct, &target, &mut kept) {
                sh.kept.push(KeptLink { rel_type: ty, target, id });
            } else if ty == crate::xlsx_table::REL_TABLE {
                read_table_query(z, &ct, &target, &mut kept, sh);
            }
        }
    }
    crate::xlsx_slicer_keep::read(z, &ct, wbxml, sheets, pivots, wb, &mut kept);
    crate::ooxml_keep::read_package(z, &ct, &mut kept);
    wb.kept = kept;
}

/// `<pivotCaches>` and the kept extensions of workbook.xml, with the
/// namespaces of the root element they may use.
fn read_book_xml(xml: &str, kept: &mut KeptParts) {
    // (attribute, declaration) of each prefix of the root element.
    let mut decls: Vec<(String, String)> = Vec::new();
    let mut in_ext = false;
    let mut r = Reader::new(xml);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) if t.local() == "workbook" => {
                for (k, v) in t.attrs() {
                    if k.starts_with("xmlns:") {
                        decls.push((k.to_string(), format!(r#" {k}="{}""#, esc(&v))));
                    }
                }
            }
            Ev::Start(t) if t.local() == "extLst" => in_ext = true,
            Ev::End(n) if crate::xml::local(n) == "extLst" => in_ext = false,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "pivotCache" && !in_ext => {
                let own: Vec<&str> = t.attrs().map(|(k, _)| k).collect();
                let extra: String = decls.iter().filter(|(k, _)| !own.contains(&k.as_str())).map(|(_, d)| d.as_str()).collect();
                kept.xml.push(("pivotCache".into(), format!("<{}{}{extra}/>", t.name, t.raw_attrs())));
            }
            Ev::Start(t) if t.local() == "ext" && in_ext => {
                let keep = t.attr("uri").is_some_and(|u| KEEP_EXT.iter().any(|k| k.eq_ignore_ascii_case(&u)));
                if !keep {
                    r.skip();
                    continue;
                }
                let own: Vec<&str> = t.attrs().map(|(k, _)| k).collect();
                let extra: String = decls.iter().filter(|(k, _)| !own.contains(&k.as_str())).map(|(_, d)| d.as_str()).collect();
                let body = r.raw_element();
                kept.xml.push(("ext".into(), format!("<{}{}{extra}>{body}", t.name, t.raw_attrs())));
            }
            _ => {}
        }
    }
}

/// The query of a table part, when it has one: attached to the sheet's
/// table of the same name.
fn read_table_query(z: &ZipReader, ct: &ContentTypes, path: &str, kept: &mut KeptParts, sh: &mut Sheet) {
    let Some((id, ty, target, _)) = read_rels(z, path).into_iter().find(|(_, ty, _, ext)| ty == REL_QUERY_TABLE && !ext) else { return };
    let Ok(xml) = z.read_string(path) else { return };
    let mut name = None;
    let mut columns = Vec::new();
    let mut r = Reader::new(&xml);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "table" && name.is_none() => {
                name = t.attr("displayName").or_else(|| t.attr("name")).map(|v| v.into_owned());
            }
            Ev::Start(t) | Ev::Empty(t) if t.local() == "tableColumn" => {
                let col = crate::xlsx_table::unescape_x(&t.attr("name").unwrap_or_default());
                let cid = t.attr("id").and_then(|v| v.parse().ok()).unwrap_or(columns.len() as u32 + 1);
                let mut extra = String::new();
                for k in ["uniqueName", "queryTableFieldId"] {
                    if let Some(v) = t.attr(k) {
                        let _ = write!(extra, r#" {k}="{}""#, esc(&v));
                    }
                }
                columns.push((col, cid, extra));
            }
            _ => {}
        }
    }
    let Some(name) = name else { return };
    let Some(table) = sh.tables.iter_mut().find(|t| t.name == name) else { return };
    if collect(z, ct, &target, kept) {
        table.query = Some(Arc::new(TableQuery { link: KeptLink { rel_type: ty, target, id }, columns }));
    }
}

/// What a workbook being written gets of its kept parts.
#[derive(Default)]
pub(crate) struct BookOut {
    /// Workbook relationships.
    pub rels: String,
    /// Package relationships.
    pub package_rels: String,
    /// Content type overrides.
    pub content_types: String,
    /// `<pivotCaches>` and `<extLst>` of workbook.xml.
    pub pivot_caches: String,
    pub ext: String,
    /// Relationships to add to each sheet's part.
    pub sheet_rels: Vec<String>,
    /// Kept slicers and timelines: extensions and drawing anchors.
    pub slicers: crate::xlsx_slicer_keep::Out,
    /// The query of each table (by sheet), where it still fits the table.
    pub queries: Vec<Vec<Option<Arc<TableQuery>>>>,
}

/// Writes the kept parts still in use and says how to link them.
pub(crate) fn write(wb: &Workbook, z: &mut ZipWriter) -> BookOut {
    let kept = &wb.kept;
    let mut out = BookOut::default();
    let mut reach = Reach::new(kept);
    // A part is linked from one sheet or table only (a copied sheet or
    // table doesn't take its pivot tables and queries along).
    let mut used: HashSet<&str> = HashSet::new();
    for sh in &wb.sheets {
        let mut qs = Vec::new();
        for t in &sh.tables {
            let q = t.query.as_ref().filter(|q| {
                let same = q.columns.len() == t.columns.len() && q.columns.iter().zip(&t.columns).all(|(a, b)| a.0 == b.name);
                same && used.insert(q.link.target.as_str()) && reach.add(&q.link.target)
            });
            qs.push(q.cloned());
        }
        out.queries.push(qs);
        let mut rels = String::new();
        for (j, l) in sh.kept.iter().enumerate() {
            // Slicers go once their caches are known.
            if crate::xlsx_slicer_keep::is_slicer(&l.rel_type) {
                continue;
            }
            if used.insert(l.target.as_str()) && reach.add(&l.target) {
                rels.push_str(&rel(&format!("rIdKS{}", j + 1), l, "xl/worksheets"));
            }
        }
        out.sheet_rels.push(rels);
    }
    // Pivot caches and workbook-level pivot tables go with the pivot tables
    // still there; connections and the data model stay anyway.
    let mut ids: HashMap<&str, String> = HashMap::new();
    for (i, l) in kept.links.iter().enumerate() {
        if l.rel_type == crate::ooxml_keep::REL_CUSTOM_XML || crate::xlsx_slicer_keep::is_cache(&l.rel_type) {
            continue;
        }
        let keep = if l.rel_type == REL_PIVOT_CACHE || l.rel_type == REL_PIVOT_TABLE { reach.has(&l.target) } else { reach.add(&l.target) };
        if keep {
            let id = format!("rIdK{}", i + 1);
            out.rels.push_str(&rel(&id, l, "xl"));
            ids.insert(l.id.as_str(), id);
        }
    }
    out.slicers = crate::xlsx_slicer_keep::write(wb, &out.queries, &mut reach, &mut out.rels, &mut out.sheet_rels, z);
    let caches: String = kept.xml.iter().filter(|(slot, _)| slot == "pivotCache").filter_map(|(_, x)| remap(x, &ids)).collect();
    if !caches.is_empty() {
        out.pivot_caches = format!("<pivotCaches>{caches}</pivotCaches>");
    }
    let ext: String = kept.xml.iter().filter(|(slot, _)| slot == "ext").filter_map(|(_, x)| remap(x, &ids)).collect();
    if !ext.is_empty() {
        out.ext = format!("<extLst>{ext}</extLst>");
    }
    let (cx, pkg, ct) = crate::ooxml_keep::write_common(z, kept, "xl", Some(reach));
    out.rels.push_str(&cx);
    out.package_rels = pkg;
    out.content_types = ct + &out.slicers.content_types;
    if !wb.queries.is_empty() {
        write_queries(wb, z, &mut out);
    }
    if !wb.checks.is_empty() {
        write_checks(wb, z, &mut out);
    }
    out
}

/// The workbook's tests as a custom XML part of their own.
fn write_checks(wb: &Workbook, z: &mut ZipWriter, out: &mut BookOut) {
    let path = crate::checks_xml::XLSX_PATH;
    z.add(path, crate::checks_xml::write(&wb.checks).as_bytes(), true);
    z.add("customXml/itemPropsChecks.xml", format!(r#"{HEADER}<ds:datastoreItem ds:itemID="{{5B1E7C2A-3D4F-4E6A-8B9C-0D1E2F3A4B5C}}" xmlns:ds="http://schemas.openxmlformats.org/officeDocument/2006/customXml"><ds:schemaRefs><ds:schemaRef ds:uri="{}"/></ds:schemaRefs></ds:datastoreItem>"#, crate::checks_xml::NS).as_bytes(), true);
    z.add("customXml/_rels/itemChecks.xml.rels", format!(r#"{HEADER}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXmlProps" Target="itemPropsChecks.xml"/></Relationships>"#).as_bytes(), true);
    let _ = write!(out.rels, r#"<Relationship Id="rIdApexT" Type="{}" Target="../{path}"/>"#, crate::ooxml_keep::REL_CUSTOM_XML);
    out.content_types.push_str(r#"<Override PartName="/customXml/itemPropsChecks.xml" ContentType="application/vnd.openxmlformats-officedocument.customXmlProperties+xml"/>"#);
}

/// The Get Data queries as `customXml/item1.xml` (kept custom XML parts
/// are numbered from 2).
fn write_queries(wb: &Workbook, z: &mut ZipWriter, out: &mut BookOut) {
    z.add("customXml/item1.xml", crate::queries_xml::write(&wb.queries).as_bytes(), true);
    z.add("customXml/itemProps1.xml", format!(r#"{HEADER}<ds:datastoreItem ds:itemID="{{8C3D6F1A-2B4E-4C7D-9E0F-A1B2C3D4E5F6}}" xmlns:ds="http://schemas.openxmlformats.org/officeDocument/2006/customXml"><ds:schemaRefs><ds:schemaRef ds:uri="{}"/></ds:schemaRefs></ds:datastoreItem>"#, crate::queries_xml::NS).as_bytes(), true);
    z.add("customXml/_rels/item1.xml.rels", format!(r#"{HEADER}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXmlProps" Target="itemProps1.xml"/></Relationships>"#).as_bytes(), true);
    let _ = write!(out.rels, r#"<Relationship Id="rIdApexQ" Type="{}" Target="../customXml/item1.xml"/>"#, crate::ooxml_keep::REL_CUSTOM_XML);
    out.content_types.push_str(r#"<Override PartName="/customXml/itemProps1.xml" ContentType="application/vnd.openxmlformats-officedocument.customXmlProperties+xml"/>"#);
}

/// Gives the relationship ids of an element their new values; `None` when
/// one points at a part that is not written.
fn remap(xml: &str, ids: &HashMap<&str, String>) -> Option<String> {
    // Prefixes of the relationships namespace ("r" in practice).
    let mut prefixes = vec!["r".to_string()];
    let mut rest = xml;
    while let Some(i) = rest.find(&format!(r#"="{NS_RELS}""#)) {
        if let Some(p) = rest[..i].rsplit_once("xmlns:").map(|(_, p)| p.trim()).filter(|p| !prefixes.iter().any(|x| x == p)) {
            prefixes.push(p.to_string());
        }
        rest = &rest[i + 1..];
    }
    let mut out = xml.to_string();
    for p in prefixes {
        let pat = format!(r#" {p}:id=""#);
        let mut from = 0;
        while let Some(i) = out[from..].find(&pat).map(|i| i + from) {
            let v0 = i + pat.len();
            let v1 = v0 + out[v0..].find('"')?;
            let new = ids.get(&out[v0..v1])?.clone();
            out.replace_range(v0..v1, &new);
            from = v0 + new.len();
        }
    }
    Some(out)
}

/// The relationships part of a table filled by a query.
pub(crate) fn table_rels(q: &TableQuery) -> String {
    format!(r#"{HEADER}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL_QUERY_TABLE}" Target="{}"/></Relationships>"#, esc(&relative("xl/tables", &q.link.target)))
}

/// A data connection of a workbook, as Data > Queries & Connections lists
/// it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DataConnection {
    pub name: String,
    pub description: String,
    /// Connection string, URL or file it reads.
    pub source: String,
    /// The query it runs (SQL, table name…), if any.
    pub command: String,
}

/// A Power Query query: its name and its M formula.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PowerQuery {
    pub name: String,
    pub formula: String,
}

/// The external data a workbook keeps.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExternalData {
    pub connections: Vec<DataConnection>,
    pub queries: Vec<PowerQuery>,
    /// It has a data model (Power Pivot).
    pub data_model: bool,
    /// Pivot tables kept (not recalculated by ApexDesk).
    pub pivot_tables: usize,
}

impl ExternalData {
    pub fn is_empty(&self) -> bool {
        self.connections.is_empty() && self.queries.is_empty() && !self.data_model && self.pivot_tables == 0
    }
}

/// What external data the workbook keeps.
pub fn external_data(wb: &Workbook) -> ExternalData {
    let kept = &wb.kept;
    let mut d = ExternalData { data_model: kept.links.iter().any(|l| book_rel(&l.rel_type, &l.target) && l.target.starts_with("xl/model/")), ..ExternalData::default() };
    d.pivot_tables = wb.sheets.iter().flat_map(|s| &s.kept).filter(|l| l.rel_type == REL_PIVOT_TABLE).map(|l| &l.target).collect::<HashSet<_>>().len();
    if let Some(p) = kept.links.iter().find(|l| l.rel_type == REL_CONNECTIONS).and_then(|l| kept.part(&l.target)) {
        d.connections = read_connections(&String::from_utf8_lossy(&p.data));
    }
    for p in kept.parts.iter().filter(|p| p.path.starts_with("customXml/item") && !p.path.contains("Props")) {
        let x = crate::ooxml_keep::decode_xml(&p.data).0;
        if x.contains("DataMashup") {
            d.queries.extend(read_mashup(&x));
        }
    }
    d
}

fn read_connections(xml: &str) -> Vec<DataConnection> {
    let mut out: Vec<DataConnection> = Vec::new();
    let mut r = Reader::new(xml);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) => {
                let s = |k: &str| t.attr(k).unwrap_or_default().into_owned();
                match t.local() {
                    "connection" => out.push(DataConnection { name: s("name"), description: s("description"), source: s("sourceFile"), command: String::new() }),
                    "dbPr" | "olapPr" => {
                        if let Some(c) = out.last_mut() {
                            c.source = s("connection");
                            c.command = s("command");
                        }
                    }
                    "webPr" => {
                        if let Some(c) = out.last_mut() {
                            c.source = s("url");
                        }
                    }
                    "textPr" => {
                        if let Some(c) = out.last_mut().filter(|c| c.source.is_empty()) {
                            c.source = s("sourceFile");
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    out
}

/// The queries of a `DataMashup` part: base64 of a small header and a zip
/// package whose `Formulas/Section1.m` holds the M code (MS-QDEFF).
fn read_mashup(xml: &str) -> Vec<PowerQuery> {
    let mut r = Reader::new(xml);
    let mut text = String::new();
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) if t.local() == "DataMashup" => {
                text = r.text();
                break;
            }
            _ => {}
        }
    }
    let clean: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let Some(bin) = crate::markdown::base64_decode(&clean) else { return Vec::new() };
    let Some(len) = bin.get(4..8).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize) else { return Vec::new() };
    let Some(pkg) = bin.get(8..8 + len) else { return Vec::new() };
    let Ok(z) = ZipReader::new(pkg) else { return Vec::new() };
    let Ok(m) = z.read_string("Formulas/Section1.m") else { return Vec::new() };
    parse_section(&m)
}

/// `shared Name = formula;` members of an M section document.
fn parse_section(m: &str) -> Vec<PowerQuery> {
    let mut out = Vec::new();
    let starts: Vec<usize> = m.match_indices("shared ").map(|(i, _)| i).filter(|&i| i == 0 || m[..i].ends_with(['\n', ';', ' ', '\t', '\r'])).collect();
    for (k, &i) in starts.iter().enumerate() {
        let end = starts.get(k + 1).copied().unwrap_or(m.len());
        let member = &m[i + "shared ".len()..end];
        let Some(eq) = member.find('=') else { continue };
        let name = member[..eq].trim();
        let name = name.strip_prefix("#\"").and_then(|n| n.strip_suffix('"')).unwrap_or(name).replace("\"\"", "\"");
        let formula = member[eq + 1..].trim().trim_end_matches(';').trim().to_string();
        out.push(PowerQuery { name, formula });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_members() {
        let m = "section Section1;\r\n\r\nshared Ventas = let\r\n    Origen = Csv.Document(File.Contents(\"C:\\v.csv\"))\r\nin\r\n    Origen;\r\n\r\nshared #\"Otra consulta\" = 1;";
        let q = parse_section(m);
        assert_eq!(q.len(), 2);
        assert_eq!(q[0].name, "Ventas");
        assert!(q[0].formula.starts_with("let") && q[0].formula.ends_with("Origen"));
        assert_eq!((q[1].name.as_str(), q[1].formula.as_str()), ("Otra consulta", "1"));
    }

    #[test]
    fn ids_remapped() {
        let ids: HashMap<&str, String> = [("rId3", "rIdK2".to_string())].into_iter().collect();
        assert_eq!(remap(r#"<pivotCache cacheId="1" r:id="rId3"/>"#, &ids).as_deref(), Some(r#"<pivotCache cacheId="1" r:id="rIdK2"/>"#));
        assert_eq!(remap(r#"<pivotCache cacheId="1" r:id="rId4"/>"#, &ids), None);
        assert_eq!(remap(r#"<x15:dataModel><x15:modelTable id="T" name="T" connection="C"/></x15:dataModel>"#, &ids).as_deref(), Some(r#"<x15:dataModel><x15:modelTable id="T" name="T" connection="C"/></x15:dataModel>"#));
    }
}
