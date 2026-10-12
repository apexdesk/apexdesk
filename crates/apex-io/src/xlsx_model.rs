//! The data model in files: ApexDesk's relationships and measures as a
//! small XML element (an extension of workbook.xml in xlsx, a part of the
//! package in ods), and what can be read of an Excel data model.
//!
//! Excel keeps its model (tables, relationships, DAX measures) compressed
//! with XPress9 in `xl/model/item.data`; that part is kept byte for byte
//! but not decoded. The table and relationship lists Excel also writes to
//! workbook.xml (`x15:dataModel`) and the measures its pivot caches name
//! are read and shown.

use std::fmt::Write as _;

use apex_sheet::Workbook;
use apex_sheet::model::{DataModel, Measure, Relationship};

use crate::xml::{Ev, Reader, esc};

/// Our extension of workbook.xml (unknown to Excel, which skips it).
pub(crate) const EXT_URI: &str = "{5D3F7A2C-8B1E-4C6D-9A0F-3E2B7C9D1A64}";
pub(crate) const NS: &str = "urn:apexdesk:model";

/// `<apex:model>` with the relationships and measures (empty model: "").
pub(crate) fn model_xml(m: &DataModel, ns_decl: bool) -> String {
    if m.is_empty() {
        return String::new();
    }
    let mut x = String::new();
    let _ = write!(x, "<apex:model{}>", if ns_decl { format!(r#" xmlns:apex="{NS}""#) } else { String::new() });
    for r in &m.relationships {
        let _ = write!(x, r#"<apex:relationship fromTable="{}" fromColumn="{}" toTable="{}" toColumn="{}"{}/>"#, esc(&r.from_table), esc(&r.from_column), esc(&r.to_table), esc(&r.to_column), if r.active { "" } else { r#" active="0""# });
    }
    for ms in &m.measures {
        let _ = write!(x, r#"<apex:measure name="{}" table="{}""#, esc(&ms.name), esc(&ms.table));
        if !ms.format.is_empty() {
            let _ = write!(x, r#" format="{}""#, esc(&ms.format));
        }
        let _ = write!(x, ">{}</apex:measure>", esc(&ms.formula));
    }
    x.push_str("</apex:model>");
    x
}

/// The `apexdesk/model.xml` part of an ods package: the data model and
/// the pivot tables ("" when there is neither).
pub(crate) fn ods_part(wb: &Workbook) -> String {
    let pivots = crate::pivot_xml::pivots_xml(wb) + &crate::plan_xml::plans_xml(wb);
    let m = model_xml(&wb.analysis.model, true);
    if pivots.is_empty() {
        return m;
    }
    match m.strip_suffix("</apex:model>") {
        Some(body) => format!("{body}{pivots}</apex:model>"),
        None => format!(r#"<apex:model xmlns:apex="{NS}">{pivots}</apex:model>"#),
    }
}

/// Reads the first `<…:model>` element of our namespace in `xml`.
pub(crate) fn read_model_xml(xml: &str) -> Option<DataModel> {
    let mut r = Reader::new(xml);
    let mut m = DataModel::default();
    let mut inside = false;
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) if t.local() == "model" => inside = true,
            Ev::End(n) if inside && crate::xml::local(n) == "model" => return Some(m),
            Ev::Start(t) | Ev::Empty(t) if inside && t.local() == "relationship" => {
                let s = |k: &str| t.attr(k).unwrap_or_default().into_owned();
                let mut rel = Relationship::new(&s("fromTable"), &s("fromColumn"), &s("toTable"), &s("toColumn"));
                rel.active = t.attr("active").is_none_or(|v| v != "0" && v != "false");
                m.relationships.push(rel);
            }
            Ev::Start(t) if inside && t.local() == "measure" => {
                let s = |k: &str| t.attr(k).unwrap_or_default().into_owned();
                let (name, table, format) = (s("name"), s("table"), s("format"));
                let formula = r.text();
                m.measures.push(Measure { name, table, formula, format });
            }
            _ => {}
        }
    }
    inside.then_some(m)
}

/// The `<extLst>` of workbook.xml: the kept extensions (`kept`, a whole
/// `<extLst>` or "") plus ours.
pub(crate) fn ext_list(kept: &str, wb: &Workbook) -> String {
    let own = model_xml(&wb.analysis.model, false) + &crate::pivot_xml::pivots_xml(wb) + &crate::plan_xml::plans_xml(wb);
    if own.is_empty() {
        return kept.to_string();
    }
    let ext = format!(r#"<ext uri="{EXT_URI}" xmlns:apex="{NS}">{own}</ext>"#);
    match kept.strip_suffix("</extLst>") {
        Some(body) => format!("{body}{ext}</extLst>"),
        None => format!("<extLst>{ext}</extLst>"),
    }
}

/// Reads our extension from workbook.xml.
pub(crate) fn read_xlsx(wbxml: &str, wb: &mut Workbook) {
    let Some(i) = wbxml.find(EXT_URI) else { return };
    if let Some(m) = read_model_xml(&wbxml[i..]) {
        wb.analysis.model = m;
    }
    crate::plan_xml::read_plans(&wbxml[i..], wb);
}

/// What an Excel data model kept from the file shows: its tables and
/// relationships (workbook.xml) and the measures its pivot caches use.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExcelModel {
    /// The file has a data model part.
    pub present: bool,
    pub tables: Vec<String>,
    pub relationships: Vec<Relationship>,
    /// Measure names (their DAX is only in the compressed model).
    pub measures: Vec<String>,
}

/// Reads what can be read of the Excel data model the workbook keeps.
pub fn excel_model(wb: &Workbook) -> ExcelModel {
    let kept = &wb.kept;
    let mut out = ExcelModel { present: kept.parts.iter().any(|p| p.path.starts_with("xl/model/")), ..Default::default() };
    for (_, x) in kept.xml.iter().filter(|(slot, x)| slot == "ext" && x.contains("dataModel")) {
        let mut r = Reader::new(x);
        loop {
            match r.next() {
                Ev::Eof => break,
                Ev::Start(t) | Ev::Empty(t) => {
                    let s = |k: &str| t.attr(k).unwrap_or_default().into_owned();
                    match t.local() {
                        "modelTable" => {
                            let name = s("name");
                            if !name.is_empty() && !out.tables.contains(&name) {
                                out.tables.push(name);
                            }
                        }
                        "modelRelationship" => out.relationships.push(Relationship::new(&s("fromTable"), &s("fromColumn"), &s("toTable"), &s("toColumn"))),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
    for p in kept.parts.iter().filter(|p| p.path.contains("pivotCacheDefinition")) {
        let xml = String::from_utf8_lossy(&p.data);
        let mut r = Reader::new(&xml);
        loop {
            match r.next() {
                Ev::Eof => break,
                Ev::Start(t) | Ev::Empty(t) if t.local() == "cacheHierarchy" && t.attr("measure").is_some_and(|v| v == "1") => {
                    let unique = t.attr("uniqueName").unwrap_or_default();
                    let name = t.attr("caption").map(|c| c.into_owned()).filter(|c| !c.is_empty()).unwrap_or_else(|| unique.trim_start_matches("[Measures].").trim_matches(['[', ']']).to_string());
                    if !name.is_empty() && !out.measures.contains(&name) {
                        out.measures.push(name);
                    }
                }
                _ => {}
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_xml_roundtrip() {
        let mut m = DataModel::default();
        m.relationships.push(Relationship::new("Sales", "Product ID", "Products", "ID"));
        let mut r = Relationship::new("Sales", "Date", "Calendar", "Date");
        r.active = false;
        m.relationships.push(r);
        m.measures.push(Measure { name: "Big & <small>".into(), table: "Sales".into(), formula: "CALCULATE([Total],\n Sales[Region] = \"North\")".into(), format: "#,##0 \"€\"".into() });
        let x = model_xml(&m, true);
        assert_eq!(read_model_xml(&x), Some(m));
        assert_eq!(model_xml(&DataModel::default(), true), "");
    }
}
