//! Pivot tables in xlsx, the way Excel keeps them: a pivot cache
//! (`xl/pivotCache/pivotCacheDefinitionN.xml`, with the source range, its
//! fields and their items, and `pivotCacheRecordsN.xml`) and a pivot table
//! part per table (`xl/pivotTables/pivotTableN.xml`: location, fields on
//! each axis, value fields and style), linked from its sheet.
//!
//! Written tables refresh on load (`refreshOnLoad`), so Excel lays them out
//! again from the source. Excel's own tables are read back into editable
//! pivot tables when ApexDesk can do what they do; the others stay as kept
//! parts (see `xlsx_keep`). Tables on the data model only go in our
//! extension (see `pivot_xml`).

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use apex_sheet::analysis::pivot::{
    self, AggFunc, BaseField, BaseItem, CalcField, Field, FieldFilter, Grouping, LabelFilter, LabelOp, Layout, PivotSpec, ShowAs, SortOrder, ValueField, ValueFilter, ValueOp, CALC_BASE,
};
use apex_sheet::analysis::{Lang, StoredPivot};
use apex_sheet::{CellRef, Range, Value, Workbook};

use crate::xml::{Ev, HEADER, Reader, Tag, esc};
use crate::zip::{ZipReader, ZipWriter};

pub(crate) const REL_PIVOT_TABLE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/pivotTable";
pub(crate) const REL_PIVOT_CACHE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/pivotCacheDefinition";
const REL_RECORDS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/pivotCacheRecords";
const CT_TABLE: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.pivotTable+xml";
const CT_CACHE: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.pivotCacheDefinition+xml";
const CT_RECORDS: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.pivotCacheRecords+xml";
const NS_MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
pub(crate) const NS_X14: &str = "http://schemas.microsoft.com/office/spreadsheetml/2009/9/main";
/// The x14 pivot cache id slicers point at.
const EXT_CACHE_X14: &str = "{725AE2AE-9491-48be-B2B4-4EB974FC3084}";
/// Show values as the 2010 way (parent totals, ranks).
const EXT_DATA_FIELD_X14: &str = "{E15A36E0-9728-4e99-A89B-3F7291B0FE68}";
/// Marks the tables ApexDesk wrote (their spec is in our workbook
/// extension); unknown to Excel, which skips it.
const EXT_OURS: &str = "{9C4E2B7A-3D1F-4A8E-B6C5-7F0D2E1A9B38}";
/// Base items "previous" and "next" of "show values as".
const ITEM_PREVIOUS: u32 = 1_048_828;
const ITEM_NEXT: u32 = 1_048_829;
/// Records beyond this many cells aren't written (the cache refreshes on
/// load anyway).
const MAX_RECORD_CELLS: usize = 2_000_000;

// ---------------------------------------------------------------------------
// Cache items

/// The items of a source column: distinct values (texts ignoring case, as
/// pivot tables group them) and the item of each record.
struct Items {
    values: Vec<Value>,
    of_record: Vec<u32>,
}

#[derive(PartialEq, Eq, Hash)]
enum ItemKey {
    Num(u64),
    Text(String),
    Bool(bool),
    Err(String),
    Blank,
}

fn item_key(v: &Value) -> ItemKey {
    match v {
        Value::Number(n) => ItemKey::Num(if *n == 0.0 { 0 } else { n.to_bits() }),
        Value::Text(t) => ItemKey::Text(t.to_lowercase()),
        Value::Bool(b) => ItemKey::Bool(*b),
        Value::Error(e) => ItemKey::Err(e.as_str().to_string()),
        _ => ItemKey::Blank,
    }
}

fn items_of(vals: &[Value]) -> Items {
    let mut map: HashMap<ItemKey, u32> = HashMap::new();
    let mut out = Items { values: Vec::new(), of_record: Vec::with_capacity(vals.len()) };
    for v in vals {
        let v = if matches!(v, Value::Array(_) | Value::Lambda(_)) { &Value::Empty } else { v };
        let id = *map.entry(item_key(v)).or_insert_with(|| {
            out.values.push(v.clone());
            (out.values.len() - 1) as u32
        });
        out.of_record.push(id);
    }
    out
}

fn num_text(n: f64) -> String {
    let mut s = String::new();
    crate::xlsx::write_number(&mut s, n);
    s
}

/// `<sharedItems>` of a cache field.
fn shared_items(x: &mut String, it: &Items) {
    let (mut s, mut n, mut b, mut e, mut blank, mut ints) = (false, false, false, false, false, true);
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for v in &it.values {
        match v {
            Value::Text(_) => s = true,
            Value::Number(v) => {
                n = true;
                ints &= v.fract() == 0.0;
                lo = lo.min(*v);
                hi = hi.max(*v);
            }
            Value::Bool(_) => b = true,
            Value::Error(_) => e = true,
            _ => blank = true,
        }
    }
    x.push_str("<sharedItems");
    if !s && !blank && !b && !e && n {
        x.push_str(r#" containsSemiMixedTypes="0""#);
    }
    if !s && (n || b || e || blank) {
        x.push_str(r#" containsString="0""#);
    }
    if [s, n, b, e].iter().filter(|t| **t).count() > 1 {
        x.push_str(r#" containsMixedTypes="1""#);
    }
    if n {
        let _ = write!(x, r#" containsNumber="1"{} minValue="{}" maxValue="{}""#, if ints { r#" containsInteger="1""# } else { "" }, num_text(lo), num_text(hi));
    }
    if blank {
        x.push_str(r#" containsBlank="1""#);
    }
    let _ = write!(x, r#" count="{}">"#, it.values.len());
    for v in &it.values {
        item_xml(x, v);
    }
    x.push_str("</sharedItems>");
}

fn item_xml(x: &mut String, v: &Value) {
    match v {
        Value::Text(t) => {
            let _ = write!(x, r#"<s v="{}"/>"#, esc(t));
        }
        Value::Number(n) => {
            let _ = write!(x, r#"<n v="{}"/>"#, num_text(*n));
        }
        Value::Bool(b) => {
            let _ = write!(x, r#"<b v="{}"/>"#, *b as u8);
        }
        Value::Error(e) => {
            let _ = write!(x, r#"<e v="{}"/>"#, esc(e.as_str()));
        }
        _ => x.push_str("<m/>"),
    }
}

// ---------------------------------------------------------------------------
// Writing

/// A pivot table written as Excel's, for the slicers that filter it.
pub(crate) struct Native {
    /// Index in `analysis.pivots`.
    pub index: usize,
    pub sheet: usize,
    pub name: String,
    pub cache_id: u32,
    /// Cache fields: name and items (the values of the source column).
    pub fields: Vec<(String, Vec<Value>)>,
    pub lang: Lang,
}

#[derive(Default)]
pub(crate) struct Out {
    pub natives: Vec<Native>,
    /// Relationships to add to each sheet's part.
    pub sheet_rels: Vec<String>,
    pub book_rels: String,
    pub content_types: String,
    /// `<pivotCache>` elements of workbook.xml.
    pub caches: String,
}

/// The source of a pivot table: its range (header row and records) and
/// every column read.
fn cache_source(wb: &Workbook, spec: &PivotSpec) -> Option<(usize, Range, pivot::Source)> {
    let si = wb.sheet_index(&spec.source_sheet)?;
    let range = wb.clamp_to_used(si, &spec.source)?;
    let probe = PivotSpec { rows: Vec::new(), cols: Vec::new(), values: Vec::new(), filters: Vec::new(), slicers: Vec::new(), calculated: Vec::new(), model: None, ..spec.clone() };
    let n = wb.pivot_source(&probe).ok()?.names.len() as u32;
    let all = PivotSpec { values: (0..n).map(|c| ValueField { col: c, ..Default::default() }).collect(), ..probe };
    let src = wb.pivot_source(&all).ok()?;
    Some((si, range, src))
}

/// Writes a pivot cache and a pivot table for each stored pivot table on
/// a sheet range; `kept_caches` are the `<pivotCaches>` kept from the file
/// (their ids and parts are left alone).
pub(crate) fn write(wb: &Workbook, z: &mut ZipWriter, kept_caches: &str) -> Out {
    let mut out = Out { sheet_rels: vec![String::new(); wb.sheets.len()], ..Default::default() };
    let mut cache_id = kept_caches.match_indices("cacheId=\"").filter_map(|(i, m)| kept_caches[i + m.len()..].split('"').next()?.parse::<u32>().ok()).max().unwrap_or(0);
    // Names of the kept tables, which ours mustn't repeat.
    let mut names: HashSet<String> = wb.kept.parts.iter().filter(|p| p.path.contains("pivotTables/pivotTable") && !p.path.ends_with(".rels")).filter_map(|p| table_name(&String::from_utf8_lossy(&p.data))).collect();
    let slicer_cols: HashSet<(usize, u32)> = wb.analysis.slicers.iter().map(|s| (s.pivot, s.col)).collect();
    let mut n = 0;
    for (pi, p) in wb.analysis.pivots.iter().enumerate() {
        if p.spec.model.is_some() {
            continue;
        }
        let Some(sheet) = wb.sheet_index(&p.sheet) else { continue };
        let Some((_, range, src)) = cache_source(wb, &p.spec) else { continue };
        let Ok(table) = wb.pivot_compute(&p.spec) else { continue };
        loop {
            n += 1;
            let taken = |path: String| wb.kept.part(&path).is_some();
            if !taken(format!("xl/pivotTables/pivotTable{n}.xml")) && !taken(format!("xl/pivotCache/pivotCacheDefinition{n}.xml")) && !taken(format!("xl/pivotCache/pivotCacheRecords{n}.xml")) {
                break;
            }
        }
        cache_id += 1;
        let mut k = names.len() + 1;
        let mut name = format!("PivotTable{k}");
        while names.contains(&name) {
            k += 1;
            name = format!("PivotTable{k}");
        }
        names.insert(name.clone());
        let items: Vec<Items> = src.cols.iter().map(|c| items_of(c)).collect();
        let records = src.records;
        let with_records = records * items.len().max(1) <= MAX_RECORD_CELLS;
        let x14 = wb.analysis.slicers.iter().any(|s| s.pivot == pi);
        let cache = cache_xml(&p.spec, range, &src, &items, cache_id, with_records, x14);
        let tab = table_xml(p, &name, cache_id, &src, &items, &table, &slicer_cols, pi);
        z.add(&format!("xl/pivotCache/pivotCacheDefinition{n}.xml"), cache.as_bytes(), true);
        if with_records {
            z.add(&format!("xl/pivotCache/_rels/pivotCacheDefinition{n}.xml.rels"), format!(r#"{HEADER}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL_RECORDS}" Target="pivotCacheRecords{n}.xml"/></Relationships>"#).as_bytes(), true);
            z.add(&format!("xl/pivotCache/pivotCacheRecords{n}.xml"), records_xml(&items, records).as_bytes(), true);
            let _ = write!(out.content_types, r#"<Override PartName="/xl/pivotCache/pivotCacheRecords{n}.xml" ContentType="{CT_RECORDS}"/>"#);
        }
        z.add(&format!("xl/pivotTables/pivotTable{n}.xml"), tab.as_bytes(), true);
        z.add(&format!("xl/pivotTables/_rels/pivotTable{n}.xml.rels"), format!(r#"{HEADER}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL_PIVOT_CACHE}" Target="../pivotCache/pivotCacheDefinition{n}.xml"/></Relationships>"#).as_bytes(), true);
        let _ = write!(out.content_types, r#"<Override PartName="/xl/pivotCache/pivotCacheDefinition{n}.xml" ContentType="{CT_CACHE}"/><Override PartName="/xl/pivotTables/pivotTable{n}.xml" ContentType="{CT_TABLE}"/>"#);
        let _ = write!(out.sheet_rels[sheet], r#"<Relationship Id="rIdPT{n}" Type="{REL_PIVOT_TABLE}" Target="../pivotTables/pivotTable{n}.xml"/>"#);
        let _ = write!(out.book_rels, r#"<Relationship Id="rIdPC{n}" Type="{REL_PIVOT_CACHE}" Target="pivotCache/pivotCacheDefinition{n}.xml"/>"#);
        let _ = write!(out.caches, r#"<pivotCache cacheId="{cache_id}" r:id="rIdPC{n}"/>"#);
        let fields = src.names.iter().cloned().zip(items.into_iter().map(|i| i.values)).collect();
        out.natives.push(Native { index: pi, sheet, name, cache_id, fields, lang: p.spec.lang });
    }
    out
}

/// `<pivotCaches>` with the kept caches (`kept`, a whole element or "")
/// and ours.
pub(crate) fn merge_caches(kept: &str, ours: &str) -> String {
    if ours.is_empty() {
        return kept.to_string();
    }
    match kept.strip_suffix("</pivotCaches>") {
        Some(body) => format!("{body}{ours}</pivotCaches>"),
        None => format!("<pivotCaches>{ours}</pivotCaches>"),
    }
}

fn table_name(xml: &str) -> Option<String> {
    let mut r = Reader::new(xml);
    loop {
        match r.next() {
            Ev::Eof => return None,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "pivotTableDefinition" => return t.attr("name").map(|v| v.into_owned()),
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn cache_xml(spec: &PivotSpec, range: Range, src: &pivot::Source, items: &[Items], cache_id: u32, with_records: bool, x14: bool) -> String {
    let mut x = String::from(HEADER);
    let _ = write!(x, r#"<pivotCacheDefinition xmlns="{NS_MAIN}" xmlns:r="{NS_R}""#);
    if with_records {
        x.push_str(r#" r:id="rId1""#);
    }
    let _ = write!(x, r#" refreshOnLoad="1" refreshedBy="ApexDesk" createdVersion="3" refreshedVersion="3" minRefreshableVersion="3" recordCount="{}">"#, src.records);
    let _ = write!(x, r#"<cacheSource type="worksheet"><worksheetSource ref="{}" sheet="{}"/></cacheSource>"#, range.to_a1(), esc(&spec.source_sheet));
    let _ = write!(x, r#"<cacheFields count="{}">"#, items.len() + spec.calculated.len());
    for (c, it) in items.iter().enumerate() {
        let date = src.formats.get(c).is_some_and(|f| f.is_some());
        let _ = write!(x, r#"<cacheField name="{}" numFmtId="{}">"#, esc(&src.names[c]), if date { 14 } else { 0 });
        shared_items(&mut x, it);
        x.push_str("</cacheField>");
    }
    for f in &spec.calculated {
        let _ = write!(x, r#"<cacheField name="{}" numFmtId="0" formula="{}" databaseField="0"/>"#, esc(&f.name), esc(f.formula.trim().trim_start_matches('=')));
    }
    x.push_str("</cacheFields>");
    if x14 {
        let _ = write!(x, r#"<extLst><ext uri="{EXT_CACHE_X14}" xmlns:x14="{NS_X14}"><x14:pivotCacheDefinition pivotCacheId="{cache_id}"/></ext></extLst>"#);
    }
    x.push_str("</pivotCacheDefinition>");
    x
}

fn records_xml(items: &[Items], records: usize) -> String {
    let mut x = String::with_capacity(records * items.len() * 12 + 200);
    x.push_str(HEADER);
    let _ = write!(x, r#"<pivotCacheRecords xmlns="{NS_MAIN}" xmlns:r="{NS_R}" count="{records}">"#);
    for r in 0..records {
        x.push_str("<r>");
        for it in items {
            let _ = write!(x, r#"<x v="{}"/>"#, it.of_record.get(r).copied().unwrap_or(0));
        }
        x.push_str("</r>");
    }
    x.push_str("</pivotCacheRecords>");
    x
}

/// Which label of the items of a column is which item.
fn label_index(it: &Items, lang: Lang) -> HashMap<String, u32> {
    it.values.iter().enumerate().map(|(i, v)| (pivot::item_text(v, Grouping::None, lang).to_lowercase(), i as u32)).collect()
}

#[allow(clippy::too_many_arguments)]
fn table_xml(p: &StoredPivot, name: &str, cache_id: u32, src: &pivot::Source, items: &[Items], t: &pivot::PivotTable, slicer_cols: &HashSet<(usize, u32)>, pi: usize) -> String {
    let s = &p.spec;
    let lang = s.lang;
    let nfields = items.len() + s.calculated.len();
    // Where each source column goes (the first time it is used).
    let mut axis: Vec<Option<(&'static str, &Field)>> = vec![None; nfields];
    let mut rows = Vec::new();
    let mut cols = Vec::new();
    let mut pages: Vec<(u32, &FieldFilter)> = Vec::new();
    for f in &s.rows {
        if let Some(a @ None) = axis.get_mut(f.col as usize) {
            *a = Some(("axisRow", f));
            rows.push(f.col as i64);
        }
    }
    for f in &s.cols {
        if let Some(a @ None) = axis.get_mut(f.col as usize) {
            *a = Some(("axisCol", f));
            cols.push(f.col as i64);
        }
    }
    for ff in &s.filters {
        if let Some(a @ None) = axis.get_mut(ff.field.col as usize) {
            *a = Some(("axisPage", &ff.field));
            pages.push((ff.field.col, ff));
        }
    }
    let field_of_value = |col: u32| -> Option<usize> {
        if col >= CALC_BASE {
            let k = (col - CALC_BASE) as usize;
            (k < s.calculated.len()).then_some(items.len() + k)
        } else {
            ((col as usize) < items.len()).then_some(col as usize)
        }
    };
    let values: Vec<(usize, &ValueField)> = s.values.iter().filter_map(|v| Some((field_of_value(v.col)?, v))).collect();
    if values.len() > 1 {
        cols.push(-2);
    }
    let indexes: Vec<HashMap<String, u32>> = items.iter().map(|it| label_index(it, lang)).collect();
    let top = if s.filters.is_empty() { 0 } else { s.filters.len() as u32 + 1 };
    let (h, w) = (t.rows().max(1) as u32, t.cols().max(1) as u32);
    let first = CellRef::new(p.at.row + top.min(h - 1), p.at.col);
    let last = CellRef::new(p.at.row + h - 1, p.at.col + w - 1);
    let data_row = (t.header_rows as u32).saturating_sub(top);

    let mut x = String::from(HEADER);
    let (compact, outline) = match s.layout {
        Layout::Compact => (true, true),
        Layout::Outline => (false, true),
        Layout::Tabular => (false, false),
    };
    let _ = write!(
        x,
        r#"<pivotTableDefinition xmlns="{NS_MAIN}" name="{}" cacheId="{cache_id}" applyNumberFormats="0" applyBorderFormats="0" applyFontFormats="0" applyPatternFormats="0" applyAlignmentFormats="0" applyWidthHeightFormats="1" dataCaption="{}" updatedVersion="3" minRefreshableVersion="3" useAutoFormatting="1" itemPrintTitles="1" createdVersion="3" indent="0""#,
        esc(name),
        esc(&if lang == Lang::Es { "Valores".to_string() } else { "Values".to_string() })
    );
    if !compact {
        x.push_str(r#" compact="0" compactData="0""#);
    }
    x.push_str(if outline { r#" outline="1" outlineData="1""# } else { r#" gridDropZones="1""# });
    if !s.grand_cols {
        x.push_str(r#" rowGrandTotals="0""#);
    }
    if !s.grand_rows {
        x.push_str(r#" colGrandTotals="0""#);
    }
    if !s.empty_text.is_empty() {
        let _ = write!(x, r#" showMissing="1" missingCaption="{}""#, esc(&s.empty_text));
    }
    x.push_str(r#" multipleFieldFilters="0">"#);
    let _ = write!(x, r#"<location ref="{}" firstHeaderRow="{}" firstDataRow="{data_row}" firstDataCol="{}""#, Range::new(first, last).to_a1(), if s.cols.is_empty() && values.len() > 1 { 0 } else { 1 }.min(data_row.max(1)), if s.rows.is_empty() { 0 } else { t.label_cols });
    if !pages.is_empty() {
        let _ = write!(x, r#" rowPageCount="{}" colPageCount="1""#, pages.len());
    }
    x.push_str("/>");
    let _ = write!(x, r#"<pivotFields count="{nfields}">"#);
    for c in 0..nfields {
        x.push_str("<pivotField");
        if let Some((a, _)) = axis[c] {
            let _ = write!(x, r#" axis="{a}""#);
        }
        if values.iter().any(|(f, _)| *f == c) {
            x.push_str(r#" dataField="1""#);
        }
        if c >= items.len() {
            x.push_str(r#" dragToRow="0" dragToCol="0" dragToPage="0""#);
        }
        if !compact {
            x.push_str(r#" compact="0""#);
        }
        if !outline {
            x.push_str(r#" outline="0""#);
        }
        x.push_str(r#" showAll="0""#);
        let Some((a, f)) = axis[c] else {
            if c < items.len() && (slicer_cols.contains(&(pi, c as u32)) || s.slicers.iter().any(|x| x.field.col as usize == c)) {
                // A slicer's field lists its items, like an axis field.
                if !s.subtotals {
                    x.push_str(r#" defaultSubtotal="0""#);
                }
                x.push('>');
                items_list(&mut x, &items[c], &[], None, s.subtotals);
                x.push_str("</pivotField>");
            } else {
                x.push_str("/>");
            }
            continue;
        };
        if let Some(n) = &f.name {
            let _ = write!(x, r#" name="{}""#, esc(n));
        }
        match f.sort {
            SortOrder::LabelAsc | SortOrder::ValueAsc(_) => x.push_str(r#" sortType="ascending""#),
            SortOrder::LabelDesc | SortOrder::ValueDesc(_) => x.push_str(r#" sortType="descending""#),
        }
        let page = pages.iter().find(|(pc, _)| *pc as usize == c).map(|(_, ff)| *ff);
        if page.is_some_and(|ff| ff.items.len() > 1) {
            x.push_str(r#" multipleItemSelectionAllowed="1""#);
        }
        if !s.subtotals {
            x.push_str(r#" defaultSubtotal="0""#);
        }
        x.push('>');
        let it = &items[c];
        let idx = &indexes[c];
        let hidden: HashSet<u32> = f.hidden.iter().filter_map(|h| idx.get(&h.to_lowercase()).copied()).collect();
        let shown: Option<HashSet<u32>> = page.filter(|ff| !ff.items.is_empty()).map(|ff| ff.items.iter().filter_map(|h| idx.get(&h.to_lowercase()).copied()).collect());
        let _ = a;
        items_list(&mut x, it, &hidden.into_iter().collect::<Vec<_>>(), shown.as_ref(), s.subtotals);
        if let SortOrder::ValueAsc(v) | SortOrder::ValueDesc(v) = f.sort {
            let _ = write!(x, r#"<autoSortScope><pivotArea dataOnly="0" outline="0" fieldPosition="0"><references count="1"><reference field="4294967294" count="1" selected="0"><x v="{v}"/></reference></references></pivotArea></autoSortScope>"#);
        }
        x.push_str("</pivotField>");
    }
    x.push_str("</pivotFields>");
    let list = |x: &mut String, tag: &str, fs: &[i64]| {
        if !fs.is_empty() {
            let _ = write!(x, r#"<{tag} count="{}">"#, fs.len());
            for f in fs {
                let _ = write!(x, r#"<field x="{f}"/>"#);
            }
            let _ = write!(x, "</{tag}>");
        }
    };
    list(&mut x, "rowFields", &rows);
    list(&mut x, "colFields", &cols);
    if !pages.is_empty() {
        let _ = write!(x, r#"<pageFields count="{}">"#, pages.len());
        for (c, ff) in &pages {
            let _ = write!(x, r#"<pageField fld="{c}""#);
            if ff.items.len() == 1 {
                if let Some(i) = indexes[*c as usize].get(&ff.items[0].to_lowercase()) {
                    let _ = write!(x, r#" item="{i}""#);
                }
            }
            x.push_str(r#" hier="-1"/>"#);
        }
        x.push_str("</pageFields>");
    }
    if !values.is_empty() {
        let _ = write!(x, r#"<dataFields count="{}">"#, values.len());
        for (fld, v) in &values {
            data_field(&mut x, s, *fld, v, &indexes, src);
        }
        x.push_str("</dataFields>");
    }
    let _ = write!(x, r#"<pivotTableStyleInfo name="{}" showRowHeaders="1" showColHeaders="1" showRowStripes="0" showColStripes="0" showLastColumn="1"/>"#, esc(&s.style));
    filters_xml(&mut x, s, &values);
    let _ = write!(x, r#"<extLst><ext uri="{EXT_OURS}" xmlns:apex="{}"><apex:pivot/></ext></extLst>"#, crate::xlsx_model::NS);
    x.push_str("</pivotTableDefinition>");
    x
}

/// `<items>` of a pivot field: every cache item, hidden ones marked, and
/// the default subtotal.
fn items_list(x: &mut String, it: &Items, hidden: &[u32], shown: Option<&HashSet<u32>>, subtotal: bool) {
    let n = it.values.len() + subtotal as usize;
    let _ = write!(x, r#"<items count="{n}">"#);
    for i in 0..it.values.len() as u32 {
        let h = hidden.contains(&i) || shown.is_some_and(|s| !s.contains(&i));
        let _ = write!(x, r#"<item{} x="{i}"/>"#, if h { r#" h="1""# } else { "" });
    }
    if subtotal {
        x.push_str(r#"<item t="default"/>"#);
    }
    x.push_str("</items>");
}

/// OOXML `showDataAs` of each "show values as", and the x14 one of those
/// Excel 2010 added.
fn show_as_key(s: &ShowAs) -> (Option<&'static str>, Option<&'static str>) {
    match s {
        ShowAs::Normal => (None, None),
        ShowAs::PercentOfGrand => (Some("percentOfTotal"), None),
        ShowAs::PercentOfRow => (Some("percentOfRow"), None),
        ShowAs::PercentOfCol => (Some("percentOfCol"), None),
        ShowAs::Index => (Some("index"), None),
        ShowAs::DifferenceFrom(..) => (Some("difference"), None),
        ShowAs::PercentOf(..) => (Some("percent"), None),
        ShowAs::PercentDifferenceFrom(..) => (Some("percentDiff"), None),
        ShowAs::RunningTotal(_) => (Some("runTotal"), None),
        ShowAs::PercentOfParent => (None, Some("percentOfParentRow")),
        ShowAs::PercentOfParentCol => (None, Some("percentOfParentCol")),
        ShowAs::PercentRunningTotal(_) => (None, Some("percentOfRunningTotal")),
        ShowAs::RankAsc(_) => (None, Some("rankAscending")),
        ShowAs::RankDesc(_) => (None, Some("rankDescending")),
    }
}

fn data_field(x: &mut String, s: &PivotSpec, fld: usize, v: &ValueField, indexes: &[HashMap<String, u32>], src: &pivot::Source) {
    x.push_str("<dataField");
    if let Some(n) = &v.name {
        let _ = write!(x, r#" name="{}""#, esc(n));
    } else {
        let field = if v.col >= CALC_BASE { s.calculated.get((v.col - CALC_BASE) as usize).map(|c| c.name.clone()).unwrap_or_default() } else { src.names.get(v.col as usize).cloned().unwrap_or_default() };
        let _ = write!(x, r#" name="{}""#, esc(&default_caption(v.func, &field, s.lang)));
    }
    let _ = write!(x, r#" fld="{fld}""#);
    let func = if v.func == AggFunc::DistinctCount { "count" } else { crate::pivot_xml::func_key(v.func) };
    if func != "sum" {
        let _ = write!(x, r#" subtotal="{func}""#);
    }
    let (key, x14) = show_as_key(&v.show_as);
    if let Some(k) = key {
        let _ = write!(x, r#" showDataAs="{k}""#);
    }
    let base = match &v.show_as {
        ShowAs::DifferenceFrom(b, i) | ShowAs::PercentOf(b, i) | ShowAs::PercentDifferenceFrom(b, i) => Some((*b, Some(i))),
        ShowAs::RunningTotal(b) | ShowAs::PercentRunningTotal(b) | ShowAs::RankAsc(b) | ShowAs::RankDesc(b) => Some((*b, None)),
        _ => None,
    };
    let (mut bf, mut bi) = (0u32, 0u32);
    if let Some((b, item)) = base {
        let f = match b {
            BaseField::Row(l) => s.rows.get(l),
            BaseField::Col(l) => s.cols.get(l),
        };
        if let Some(f) = f {
            bf = f.col;
            bi = match item {
                Some(BaseItem::Previous) => ITEM_PREVIOUS,
                Some(BaseItem::Next) => ITEM_NEXT,
                Some(BaseItem::Named(n)) => indexes.get(f.col as usize).and_then(|m| m.get(&n.to_lowercase())).copied().unwrap_or(0),
                None => 0,
            };
        }
    }
    let _ = write!(x, r#" baseField="{bf}" baseItem="{bi}""#);
    let fmt = v.number_format.clone().or_else(|| matches!(key, Some(k) if k.starts_with("percent")).then(|| "0.00%".to_string()).or_else(|| (x14 == Some("percentOfParentRow") || x14 == Some("percentOfParentCol") || x14 == Some("percentOfRunningTotal")).then(|| "0.00%".to_string())));
    if let Some(id) = fmt.as_deref().and_then(crate::xlsx_styles::builtin_id_for) {
        let _ = write!(x, r#" numFmtId="{id}""#);
    }
    match x14 {
        Some(k) => {
            let _ = write!(x, r#"><extLst><ext uri="{EXT_DATA_FIELD_X14}" xmlns:x14="{NS_X14}"><x14:dataField pivotShowAs="{k}"/></ext></extLst></dataField>"#);
        }
        None => x.push_str("/>"),
    }
}

/// Excel's caption of a value field ("Sum of Sales").
fn default_caption(f: AggFunc, field: &str, lang: Lang) -> String {
    let es = lang == Lang::Es;
    let w = match f {
        AggFunc::Sum => if es { "Suma de" } else { "Sum of" },
        AggFunc::Count | AggFunc::DistinctCount => if es { "Cuenta de" } else { "Count of" },
        AggFunc::Average => if es { "Promedio de" } else { "Average of" },
        AggFunc::Max => if es { "Máx. de" } else { "Max of" },
        AggFunc::Min => if es { "Mín. de" } else { "Min of" },
        AggFunc::Product => if es { "Producto de" } else { "Product of" },
        AggFunc::CountNums => if es { "Contar números de" } else { "Count Numbers of" },
        AggFunc::StdDev => if es { "DesvEst de" } else { "StdDev of" },
        AggFunc::StdDevP => if es { "DesvEstP de" } else { "StdDevp of" },
        AggFunc::Var => if es { "Var de" } else { "Var of" },
        AggFunc::VarP => if es { "VarP de" } else { "Varp of" },
    };
    format!("{w} {field}")
}

const LABEL_TYPES: [(LabelOp, &str); 14] = [
    (LabelOp::Equals, "captionEqual"),
    (LabelOp::NotEquals, "captionNotEqual"),
    (LabelOp::BeginsWith, "captionBeginsWith"),
    (LabelOp::NotBeginsWith, "captionNotBeginsWith"),
    (LabelOp::EndsWith, "captionEndsWith"),
    (LabelOp::NotEndsWith, "captionNotEndsWith"),
    (LabelOp::Contains, "captionContains"),
    (LabelOp::NotContains, "captionNotContains"),
    (LabelOp::Greater, "captionGreaterThan"),
    (LabelOp::GreaterEq, "captionGreaterThanOrEqual"),
    (LabelOp::Less, "captionLessThan"),
    (LabelOp::LessEq, "captionLessThanOrEqual"),
    (LabelOp::Between, "captionBetween"),
    (LabelOp::NotBetween, "captionNotBetween"),
];

const VALUE_TYPES: [(ValueOp, &str); 8] = [
    (ValueOp::Equals, "valueEqual"),
    (ValueOp::NotEquals, "valueNotEqual"),
    (ValueOp::Greater, "valueGreaterThan"),
    (ValueOp::GreaterEq, "valueGreaterThanOrEqual"),
    (ValueOp::Less, "valueLessThan"),
    (ValueOp::LessEq, "valueLessThanOrEqual"),
    (ValueOp::Between, "valueBetween"),
    (ValueOp::NotBetween, "valueNotBetween"),
];

/// The autofilter condition behind a comparison (`customFilter`
/// operator, or "" for equals).
fn operator(op: LabelOp) -> &'static str {
    match op {
        LabelOp::NotEquals | LabelOp::NotBeginsWith | LabelOp::NotEndsWith | LabelOp::NotContains => r#" operator="notEqual""#,
        LabelOp::Greater => r#" operator="greaterThan""#,
        LabelOp::GreaterEq => r#" operator="greaterThanOrEqual""#,
        LabelOp::Less => r#" operator="lessThan""#,
        LabelOp::LessEq => r#" operator="lessThanOrEqual""#,
        _ => "",
    }
}

fn custom_filters(x: &mut String, op: LabelOp, a: &str, b: &str) {
    match op {
        LabelOp::Between => {
            let _ = write!(x, r#"<customFilters and="1"><customFilter operator="greaterThanOrEqual" val="{}"/><customFilter operator="lessThanOrEqual" val="{}"/></customFilters>"#, esc(a), esc(b));
        }
        LabelOp::NotBetween => {
            let _ = write!(x, r#"<customFilters><customFilter operator="lessThan" val="{}"/><customFilter operator="greaterThan" val="{}"/></customFilters>"#, esc(a), esc(b));
        }
        _ => {
            let v = match op {
                LabelOp::BeginsWith | LabelOp::NotBeginsWith => format!("{a}*"),
                LabelOp::EndsWith | LabelOp::NotEndsWith => format!("*{a}"),
                LabelOp::Contains | LabelOp::NotContains => format!("*{a}*"),
                _ => a.to_string(),
            };
            let _ = write!(x, r#"<customFilters><customFilter{} val="{}"/></customFilters>"#, operator(op), esc(&v));
        }
    }
}

/// `<filters>`: the label and value filters of the row and column fields.
fn filters_xml(x: &mut String, s: &PivotSpec, values: &[(usize, &ValueField)]) {
    let mut body = String::new();
    let mut id = 0;
    for f in s.rows.iter().chain(&s.cols) {
        if let Some(l) = &f.label_filter {
            id += 1;
            let ty = LABEL_TYPES.iter().find(|(o, _)| *o == l.op).map_or("captionEqual", |(_, t)| t);
            let _ = write!(body, r#"<filter fld="{}" type="{ty}" evalOrder="-1" id="{id}" stringValue1="{}""#, f.col, esc(&l.a));
            if l.op.two() {
                let _ = write!(body, r#" stringValue2="{}""#, esc(&l.b));
            }
            body.push_str(r#"><autoFilter ref="A1"><filterColumn colId="0">"#);
            custom_filters(&mut body, l.op, &l.a, &l.b);
            body.push_str("</filterColumn></autoFilter></filter>");
        }
        if let Some(v) = &f.value_filter {
            if v.value >= values.len() {
                continue;
            }
            id += 1;
            let (ty, top10) = match v.op {
                ValueOp::TopItems => ("count", Some((true, false))),
                ValueOp::BottomItems => ("count", Some((false, false))),
                ValueOp::TopPercent => ("percent", Some((true, true))),
                ValueOp::BottomPercent => ("percent", Some((false, true))),
                ValueOp::TopSum => ("sum", Some((true, false))),
                ValueOp::BottomSum => ("sum", Some((false, false))),
                op => (VALUE_TYPES.iter().find(|(o, _)| *o == op).map_or("valueEqual", |(_, t)| t), None),
            };
            let _ = write!(body, r#"<filter fld="{}" type="{ty}" evalOrder="-1" id="{id}" iMeasureFld="{}"><autoFilter ref="A1"><filterColumn colId="0">"#, f.col, v.value);
            match top10 {
                Some((top, percent)) => {
                    let _ = write!(body, r#"<top10{}{} val="{}" filterVal="{}"/>"#, if top { "" } else { r#" top="0""# }, if percent { r#" percent="1""# } else { "" }, num_text(v.a), num_text(v.a));
                }
                None => {
                    let lop = match v.op {
                        ValueOp::NotEquals => LabelOp::NotEquals,
                        ValueOp::Greater => LabelOp::Greater,
                        ValueOp::GreaterEq => LabelOp::GreaterEq,
                        ValueOp::Less => LabelOp::Less,
                        ValueOp::LessEq => LabelOp::LessEq,
                        ValueOp::Between => LabelOp::Between,
                        ValueOp::NotBetween => LabelOp::NotBetween,
                        _ => LabelOp::Equals,
                    };
                    custom_filters(&mut body, lop, &num_text(v.a), &num_text(v.b));
                }
            }
            body.push_str("</filterColumn></autoFilter></filter>");
        }
    }
    if id > 0 {
        let _ = write!(x, r#"<filters count="{id}">{body}</filters>"#);
    }
}

// ---------------------------------------------------------------------------
// Reading

/// A cache field: its name, items and formula (calculated fields).
struct CacheField {
    name: String,
    items: Vec<Value>,
    formula: Option<String>,
}

struct Cache {
    sheet: String,
    range: Range,
    fields: Vec<CacheField>,
}

/// A pivot table read from Excel, for the slicers that filter it.
pub(crate) struct ReadTable {
    /// Index in `analysis.pivots`.
    pub index: usize,
    pub sheet: usize,
    pub name: String,
    /// Cache fields: name and items.
    pub fields: Vec<(String, Vec<Value>)>,
    pub lang: Lang,
    /// The cache part, which slicer caches name by its x14 id.
    pub cache_x14: Option<u32>,
}

/// What reading the pivot tables of a workbook found.
#[derive(Default)]
pub(crate) struct Read {
    /// Pivot table parts read (ours, or Excel's made editable): not to
    /// keep.
    pub handled: HashSet<String>,
    pub tables: Vec<ReadTable>,
    /// Our extension had the pivot tables and slicers.
    pub ours: bool,
}

/// Reads our extension's pivot tables, then the pivot tables of each sheet
/// (`sheets`: their parts) ApexDesk can recalculate.
pub(crate) fn read(z: &ZipReader, wbxml: &str, sheets: &[String], wb: &mut Workbook) -> Read {
    let mut out = Read::default();
    if let Some(i) = wbxml.find(crate::xlsx_model::EXT_URI) {
        out.ours = crate::pivot_xml::read_pivots_xml(&wbxml[i..], wb);
    }
    let numfmts = z.read_string("xl/styles.xml").map(|s| read_numfmts(&s)).unwrap_or_default();
    for (si, path) in sheets.iter().enumerate() {
        if si >= wb.sheets.len() {
            break;
        }
        for (_, ty, target, external) in crate::ooxml_keep::read_rels(z, path) {
            if external || ty != REL_PIVOT_TABLE {
                continue;
            }
            let Ok(xml) = z.read_string(&target) else { continue };
            if xml.contains(EXT_OURS) && out.ours {
                out.handled.insert(target);
                continue;
            }
            let Some(cache_path) = crate::ooxml_keep::read_rels(z, &target).into_iter().find(|(_, t, _, e)| t == REL_PIVOT_CACHE && !e).map(|r| r.2) else { continue };
            let Ok(cxml) = z.read_string(&cache_path) else { continue };
            let Some(cache) = read_cache(&cxml, wb) else { continue };
            let Some((p, name)) = read_table(&xml, &cache, wb, si, &numfmts) else { continue };
            let lang = p.spec.lang;
            wb.analysis.pivots.push(p);
            out.tables.push(ReadTable {
                index: wb.analysis.pivots.len() - 1,
                sheet: si,
                name,
                fields: cache.fields.into_iter().map(|f| (f.name, f.items)).collect(),
                lang,
                cache_x14: x14_cache_id(&cxml),
            });
            out.handled.insert(target);
        }
    }
    out
}

fn x14_cache_id(xml: &str) -> Option<u32> {
    let i = xml.find("pivotCacheId=\"")? + "pivotCacheId=\"".len();
    xml[i..].split('"').next()?.parse().ok()
}

/// Custom number formats of styles.xml by id.
fn read_numfmts(xml: &str) -> HashMap<u32, String> {
    let mut out = HashMap::new();
    let mut r = Reader::new(xml);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "numFmt" => {
                if let (Some(id), Some(code)) = (t.attr("numFmtId").and_then(|v| v.parse().ok()), t.attr("formatCode")) {
                    out.insert(id, code.into_owned());
                }
            }
            Ev::Start(t) if t.local() == "cellXfs" => break,
            _ => {}
        }
    }
    out
}

/// Reads a cache on a sheet range (or a table or name standing for one);
/// None when it holds what ApexDesk can't recalculate.
fn read_cache(xml: &str, wb: &Workbook) -> Option<Cache> {
    let mut r = Reader::new(xml);
    let mut cache = Cache { sheet: String::new(), range: Range::default(), fields: Vec::new() };
    let mut source = None;
    loop {
        let ev = r.next();
        let empty = matches!(ev, Ev::Empty(_));
        match ev {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) => match t.local() {
                "cacheSource" => {
                    if t.attr("type").as_deref() != Some("worksheet") {
                        return None;
                    }
                }
                "worksheetSource" => {
                    if t.attr("r:id").is_some() {
                        return None;
                    }
                    source = match (t.attr("name"), t.attr("ref"), t.attr("sheet")) {
                        (Some(n), _, _) => Some(resolve_name(wb, &n)?),
                        (None, Some(rf), Some(sh)) => Some((sh.into_owned(), Range::parse(&rf)?)),
                        _ => return None,
                    };
                }
                "cacheField" => {
                    if t.attr("databaseField").is_some_and(|v| v == "0") && t.attr("formula").is_none() {
                        return None;
                    }
                    let mut f = CacheField { name: t.attr("name").unwrap_or_default().into_owned(), items: Vec::new(), formula: t.attr("formula").map(|v| v.into_owned()) };
                    if !empty {
                        read_cache_field(&mut r, &mut f)?;
                    }
                    cache.fields.push(f);
                }
                // OLAP and data model caches.
                "cacheHierarchies" | "kpis" | "dimensions" | "measureGroups" | "maps" | "calculatedItems" | "calculatedMembers" | "tupleCache" => return None,
                _ => {}
            },
            _ => {}
        }
    }
    let (sheet, range) = source?;
    cache.sheet = sheet;
    cache.range = range;
    Some(cache)
}

/// Items of a cache field; None when it is grouped.
fn read_cache_field(r: &mut Reader, f: &mut CacheField) -> Option<()> {
    let mut depth = 1;
    let mut in_shared = false;
    loop {
        let ev = r.next();
        match ev {
            Ev::Eof => return Some(()),
            Ev::End(n) => {
                if crate::xml::local(n) == "sharedItems" {
                    in_shared = false;
                }
                depth -= 1;
                if depth == 0 {
                    return Some(());
                }
            }
            Ev::Start(t) | Ev::Empty(t) => {
                let empty = matches!(ev, Ev::Empty(_));
                match t.local() {
                    "fieldGroup" | "mpMap" => return None,
                    "sharedItems" => in_shared = !empty,
                    "s" | "n" | "b" | "e" | "m" | "d" if in_shared => f.items.push(cache_value(&t)),
                    _ => {}
                }
                if !empty {
                    depth += 1;
                }
            }
            _ => {}
        }
    }
}

fn cache_value(t: &Tag) -> Value {
    let v = t.attr("v").unwrap_or_default();
    match t.local() {
        "s" => Value::text(v.as_ref()),
        "n" => v.parse::<f64>().map_or(Value::Empty, Value::Number),
        "b" => Value::Bool(v == "1" || v == "true"),
        "e" => apex_sheet::ErrorKind::parse(&v).map_or(Value::Empty, Value::Error),
        "d" => crate::xlsx::parse_iso_datetime(&v).map_or(Value::Empty, Value::Number),
        _ => Value::Empty,
    }
}

/// The sheet and range a table or defined name stands for.
fn resolve_name(wb: &Workbook, name: &str) -> Option<(String, Range)> {
    for sh in &wb.sheets {
        if let Some(t) = sh.tables.iter().find(|t| t.name.eq_ignore_ascii_case(name)) {
            let mut r = t.range;
            if t.totals && r.end.row > r.start.row {
                r.end.row -= 1;
            }
            return Some((sh.name.clone(), r));
        }
    }
    let n = wb.names.iter().find(|n| n.is(name) && n.scope.is_none())?;
    let (sheet, r) = n.range()?;
    Some((sheet?.to_string(), r))
}

/// What a pivot field of a table holds.
#[derive(Default)]
struct PField {
    /// (cache item, hidden) of each item.
    items: Vec<(u32, bool)>,
    sort: Option<String>,
    sort_value: Option<usize>,
    name: Option<String>,
    compact: bool,
    outline: bool,
    subtotal: bool,
}

struct DField {
    fld: usize,
    func: AggFunc,
    name: Option<String>,
    fmt: Option<u32>,
    show: Option<String>,
    x14_show: Option<String>,
    base_field: u32,
    base_item: u32,
}

/// Reads a pivot table on `cache`; None when it does what ApexDesk can't.
fn read_table(xml: &str, cache: &Cache, wb: &Workbook, si: usize, numfmts: &HashMap<u32, String>) -> Option<(StoredPivot, String)> {
    let src_si = wb.sheet_index(&cache.sheet)?;
    let width = cache.range.cols() as usize;
    let db_fields = cache.fields.iter().filter(|f| f.formula.is_none()).count();
    if db_fields != width || cache.fields.iter().take(width).any(|f| f.formula.is_some()) {
        return None;
    }
    let mut r = Reader::new(xml);
    let mut name = String::new();
    let mut spec = PivotSpec { source_sheet: wb.sheets[src_si].name.clone(), source: cache.range, ..PivotSpec::default() };
    let mut location = None;
    let mut page_count = 0u32;
    let mut fields: Vec<PField> = Vec::new();
    let (mut rows, mut cols, mut pages, mut datas) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut filters: Vec<(usize, String, Option<usize>, String, String, Vec<(String, String)>, Option<(bool, bool, f64)>)> = Vec::new();
    let (mut table_compact, mut table_outline) = (true, true);
    let mut data_on_rows = false;
    let mut in_items_of: Option<usize> = None;
    let mut in_list = "";
    loop {
        let ev = r.next();
        let empty = matches!(ev, Ev::Empty(_));
        match ev {
            Ev::Eof => break,
            Ev::End(n) => match crate::xml::local(n) {
                "pivotField" => in_items_of = None,
                "rowFields" | "colFields" | "pageFields" | "dataFields" => in_list = "",
                _ => {}
            },
            Ev::Start(t) | Ev::Empty(t) => match t.local() {
                "pivotTableDefinition" => {
                    let flag = |k: &str, d: bool| t.attr(k).map_or(d, |v| v == "1" || v == "true");
                    name = t.attr("name").unwrap_or_default().into_owned();
                    spec.grand_cols = flag("rowGrandTotals", true);
                    spec.grand_rows = flag("colGrandTotals", true);
                    table_compact = flag("compact", true);
                    table_outline = flag("outline", false);
                    data_on_rows = flag("dataOnRows", false);
                    if flag("showMissing", true) {
                        spec.empty_text = t.attr("missingCaption").unwrap_or_default().into_owned();
                    }
                }
                "location" => {
                    location = Some(Range::parse(&t.attr("ref")?)?);
                    page_count = t.attr("rowPageCount").and_then(|v| v.parse().ok()).unwrap_or(0);
                }
                "pivotField" => {
                    let flag = |k: &str, d: bool| t.attr(k).map_or(d, |v| v == "1" || v == "true");
                    // Custom subtotal functions aren't ApexDesk's.
                    if ["sumSubtotal", "countASubtotal", "avgSubtotal", "maxSubtotal", "minSubtotal", "productSubtotal", "countSubtotal", "stdDevSubtotal", "stdDevPSubtotal", "varSubtotal", "varPSubtotal"].iter().any(|k| flag(k, false)) {
                        return None;
                    }
                    fields.push(PField {
                        sort: t.attr("sortType").map(|v| v.into_owned()),
                        name: t.attr("name").map(|v| v.into_owned()),
                        compact: flag("compact", true),
                        outline: flag("outline", true),
                        subtotal: flag("defaultSubtotal", true),
                        ..PField::default()
                    });
                    if !empty {
                        in_items_of = Some(fields.len() - 1);
                    }
                }
                "item" if in_items_of.is_some() => {
                    let f = &mut fields[in_items_of?];
                    // Items renamed or added by hand (calculated items).
                    if t.attr("n").is_some() || t.attr("f").is_some_and(|v| v == "1") {
                        return None;
                    }
                    if let (None, Some(x)) = (t.attr("t"), t.attr("x").and_then(|v| v.parse().ok())) {
                        f.items.push((x, t.attr("h").is_some_and(|v| v == "1" || v == "true")));
                    }
                }
                "reference" if in_items_of.is_some() => {
                    // autoSortScope: sorted by a value field.
                    if t.attr("field").is_some_and(|v| v == "4294967294" || v == "-2") {
                        let f = in_items_of?;
                        let mut rr = r.clone();
                        if let Ev::Start(x) | Ev::Empty(x) = rr.next() {
                            if x.local() == "x" {
                                fields[f].sort_value = x.attr("v").and_then(|v| v.parse().ok()).or(Some(0));
                            }
                        }
                    }
                }
                "rowFields" => in_list = "row",
                "colFields" => in_list = "col",
                "pageFields" => in_list = "page",
                "dataFields" => in_list = "data",
                "field" => {
                    let x: i64 = t.attr("x").and_then(|v| v.parse().ok())?;
                    match in_list {
                        "row" if x == -2 => {
                            if data_on_rows {
                                return None;
                            }
                        }
                        "row" => rows.push(x as usize),
                        "col" if x == -2 => {}
                        "col" => cols.push(x as usize),
                        _ => {}
                    }
                }
                "pageField" => {
                    let fld: usize = t.attr("fld").and_then(|v| v.parse().ok())?;
                    pages.push((fld, t.attr("item").and_then(|v| v.parse::<u32>().ok())));
                    if t.attr("hier").is_some_and(|v| v != "-1") {
                        return None;
                    }
                }
                "dataField" => {
                    let fld: usize = t.attr("fld").and_then(|v| v.parse().ok())?;
                    let func = match t.attr("subtotal") {
                        Some(s) => crate::pivot_xml::func_of(&s)?,
                        None => AggFunc::Sum,
                    };
                    let mut d = DField {
                        fld,
                        func,
                        name: t.attr("name").map(|v| v.into_owned()),
                        fmt: t.attr("numFmtId").and_then(|v| v.parse().ok()),
                        show: t.attr("showDataAs").map(|v| v.into_owned()),
                        x14_show: None,
                        base_field: t.attr("baseField").and_then(|v| v.parse().ok()).unwrap_or(0),
                        base_item: t.attr("baseItem").and_then(|v| v.parse().ok()).unwrap_or(0),
                    };
                    if !empty {
                        let body = r.raw_element();
                        if let Some(i) = body.find("pivotShowAs=\"") {
                            d.x14_show = body[i + 13..].split('"').next().map(str::to_string);
                        }
                    }
                    datas.push(d);
                }
                "pivotTableStyleInfo" => {
                    if let Some(n) = t.attr("name").filter(|n| !n.is_empty()) {
                        spec.style = n.into_owned();
                    }
                }
                "filter" => {
                    let fld: usize = t.attr("fld").and_then(|v| v.parse().ok())?;
                    let ty = t.attr("type").unwrap_or_default().into_owned();
                    let measure = t.attr("iMeasureFld").and_then(|v| v.parse().ok());
                    let s1 = t.attr("stringValue1").unwrap_or_default().into_owned();
                    let s2 = t.attr("stringValue2").unwrap_or_default().into_owned();
                    let mut customs = Vec::new();
                    let mut top10 = None;
                    if !empty {
                        let body = r.raw_element();
                        let mut br = Reader::new(body);
                        loop {
                            match br.next() {
                                Ev::Eof => break,
                                Ev::Start(c) | Ev::Empty(c) if c.local() == "customFilter" => customs.push((c.attr("operator").unwrap_or_default().into_owned(), c.attr("val").unwrap_or_default().into_owned())),
                                Ev::Start(c) | Ev::Empty(c) if c.local() == "top10" => {
                                    let flag = |k: &str, d: bool| c.attr(k).map_or(d, |v| v == "1" || v == "true");
                                    top10 = Some((flag("top", true), flag("percent", false), c.attr("val").and_then(|v| v.parse().ok()).unwrap_or(10.0)));
                                }
                                _ => {}
                            }
                        }
                    }
                    filters.push((fld, ty, measure, s1, s2, customs, top10));
                }
                // OLAP tables and formats of their own.
                "pivotHierarchies" | "conditionalFormats" | "rowHierarchiesUsage" | "colHierarchiesUsage" => return None,
                _ => {}
            },
            _ => {}
        }
    }
    let location = location?;
    if fields.len() != cache.fields.len() {
        return None;
    }
    // The table's own language (captions it shows) is guessed from its cells.
    spec.lang = guess_lang(wb, si, &location);
    let lang = spec.lang;
    let names: Vec<String> = cache.fields.iter().take(width).map(|f| f.name.clone()).collect();
    for f in cache.fields.iter().skip(width) {
        let formula = f.formula.clone().unwrap_or_default();
        if pivot::calc::parse(&formula, &names, false).is_err() {
            return None;
        }
        spec.calculated.push(CalcField { name: f.name.clone(), formula: format!("={}", formula.trim_start_matches('=')) });
    }
    let label = |fld: usize, x: u32| -> Option<String> { cache.fields.get(fld)?.items.get(x as usize).map(|v| pivot::item_text(v, Grouping::None, lang)) };
    let field = |fld: usize| -> Option<Field> {
        if fld >= width {
            return None;
        }
        let pf = &fields[fld];
        let sort = match (pf.sort.as_deref(), pf.sort_value) {
            (Some("descending"), Some(v)) => SortOrder::ValueDesc(v),
            (_, Some(v)) => SortOrder::ValueAsc(v),
            (Some("descending"), None) => SortOrder::LabelDesc,
            _ => SortOrder::LabelAsc,
        };
        let hidden = pf.items.iter().filter(|(_, h)| *h).filter_map(|(x, _)| label(fld, *x)).collect();
        Some(Field { col: fld as u32, sort, hidden, name: pf.name.clone(), ..Field::default() })
    };
    for &fld in &rows {
        spec.rows.push(field(fld)?);
    }
    for &fld in &cols {
        spec.cols.push(field(fld)?);
    }
    for &(fld, item) in &pages {
        let mut f = field(fld)?;
        let items = match item {
            Some(x) => vec![label(fld, x)?],
            None if f.hidden.is_empty() => Vec::new(),
            None => fields[fld].items.iter().filter(|(_, h)| !*h).filter_map(|(x, _)| label(fld, *x)).collect(),
        };
        f.hidden.clear();
        spec.filters.push(FieldFilter { field: f, items });
    }
    for d in &datas {
        let col = if d.fld < width { d.fld as u32 } else if d.fld < cache.fields.len() { CALC_BASE + (d.fld - width) as u32 } else { return None };
        let base = || -> BaseField {
            let bf = d.base_field as usize;
            if let Some(l) = spec.rows.iter().position(|f| f.col as usize == bf) {
                BaseField::Row(l)
            } else if let Some(l) = spec.cols.iter().position(|f| f.col as usize == bf) {
                BaseField::Col(l)
            } else {
                BaseField::Row(0)
            }
        };
        let item = || -> BaseItem {
            match d.base_item {
                ITEM_PREVIOUS => BaseItem::Previous,
                ITEM_NEXT => BaseItem::Next,
                x => label(d.base_field as usize, x).map_or(BaseItem::Previous, BaseItem::Named),
            }
        };
        let show_as = match d.x14_show.as_deref().or(d.show.as_deref()) {
            None | Some("normal") => ShowAs::Normal,
            Some("percentOfTotal") => ShowAs::PercentOfGrand,
            Some("percentOfRow") => ShowAs::PercentOfRow,
            Some("percentOfCol") => ShowAs::PercentOfCol,
            Some("index") => ShowAs::Index,
            Some("difference") => ShowAs::DifferenceFrom(base(), item()),
            Some("percent") => ShowAs::PercentOf(base(), item()),
            Some("percentDiff") => ShowAs::PercentDifferenceFrom(base(), item()),
            Some("runTotal") => ShowAs::RunningTotal(base()),
            Some("percentOfParentRow") | Some("percentOfParent") => ShowAs::PercentOfParent,
            Some("percentOfParentCol") => ShowAs::PercentOfParentCol,
            Some("percentOfRunningTotal") => ShowAs::PercentRunningTotal(base()),
            Some("rankAscending") => ShowAs::RankAsc(base()),
            Some("rankDescending") => ShowAs::RankDesc(base()),
            Some(_) => return None,
        };
        let number_format = d.fmt.filter(|id| *id != 0).and_then(|id| numfmts.get(&id).cloned().or_else(|| crate::xlsx_styles::builtin_numfmt(id).map(str::to_string)));
        spec.values.push(ValueField { col, func: d.func, show_as, name: d.name.clone(), number_format });
    }
    for (fld, ty, measure, s1, s2, customs, top10) in filters {
        let f = spec.rows.iter_mut().chain(spec.cols.iter_mut()).find(|f| f.col as usize == fld)?;
        if let Some((op, _)) = LABEL_TYPES.iter().find(|(_, t)| *t == ty) {
            f.label_filter = Some(LabelFilter { op: *op, a: s1, b: s2 });
            continue;
        }
        let value = measure.unwrap_or(0);
        let num = |i: usize| customs.get(i).and_then(|(_, v)| v.parse::<f64>().ok()).unwrap_or(0.0);
        let op = match ty.as_str() {
            "count" | "percent" | "sum" => {
                let (top, _, n) = top10.unwrap_or((true, false, 10.0));
                let op = match (ty.as_str(), top) {
                    ("count", true) => ValueOp::TopItems,
                    ("count", false) => ValueOp::BottomItems,
                    ("percent", true) => ValueOp::TopPercent,
                    ("percent", false) => ValueOp::BottomPercent,
                    (_, true) => ValueOp::TopSum,
                    (_, false) => ValueOp::BottomSum,
                };
                f.value_filter = Some(ValueFilter { value, op, a: n, b: 0.0 });
                continue;
            }
            t => VALUE_TYPES.iter().find(|(_, k)| *k == t).map(|(o, _)| *o)?,
        };
        f.value_filter = Some(ValueFilter { value, op, a: num(0), b: num(1) });
    }
    // Layout and subtotals from the row fields (or the table).
    let first_row = rows.first().map(|&f| &fields[f]);
    spec.layout = match first_row.map_or((table_compact, table_outline || table_compact), |f| (f.compact, f.outline)) {
        (_, false) => Layout::Tabular,
        (false, true) => Layout::Outline,
        (true, true) => Layout::Compact,
    };
    let axis_fields: Vec<&PField> = rows.iter().chain(&cols).map(|&f| &fields[f]).collect();
    spec.subtotals = axis_fields.is_empty() || axis_fields.iter().any(|f| f.subtotal);
    let top = if page_count > 0 { page_count + 1 } else { 0 };
    let at = CellRef::new(location.start.row.saturating_sub(top), location.start.col);
    let mut end = location.end;
    if page_count > 0 {
        end.col = end.col.max(at.col + 1);
    }
    Some((StoredPivot { spec, sheet: wb.sheets[si].name.clone(), at, area: Some(Range::new(at, end)), charts: Vec::new() }, name))
}

/// Spanish when the table's captions are ApexDesk's Spanish ones.
fn guess_lang(wb: &Workbook, si: usize, area: &Range) -> Lang {
    let es: Vec<String> = ["row_labels", "grand_total", "column_labels"].iter().map(|k| Lang::Es.text(k).to_lowercase()).collect();
    let mut found = false;
    let sh = &wb.sheets[si];
    let probe = Range::new(area.start, CellRef::new(area.end.row.min(area.start.row + 2000), area.end.col.min(area.start.col + 50)));
    sh.for_each_in(&probe, |_, cell| {
        if let Value::Text(t) = cell.value() {
            if es.iter().any(|e| t.to_lowercase() == *e) {
                found = true;
            }
        }
    });
    if found { Lang::Es } else { Lang::En }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_ignore_case() {
        let it = items_of(&[Value::text("North"), Value::text("north"), Value::Number(3.0), Value::Empty, Value::Number(3.0)]);
        assert_eq!(it.values.len(), 3);
        assert_eq!(it.of_record, vec![0, 0, 1, 2, 1]);
        let mut x = String::new();
        shared_items(&mut x, &it);
        assert!(x.contains(r#"containsMixedTypes="1""#) && x.contains(r#"containsBlank="1""#) && x.contains(r#"count="3""#), "{x}");
        let mut x = String::new();
        shared_items(&mut x, &items_of(&[Value::Number(1.5), Value::Number(2.0)]));
        assert!(x.starts_with(r#"<sharedItems containsSemiMixedTypes="0" containsString="0" containsNumber="1" minValue="1.5" maxValue="2""#), "{x}");
    }

    #[test]
    fn caches_merge() {
        assert_eq!(merge_caches("", ""), "");
        assert_eq!(merge_caches("", "<a/>"), "<pivotCaches><a/></pivotCaches>");
        assert_eq!(merge_caches("<pivotCaches><k/></pivotCaches>", "<a/>"), "<pivotCaches><k/><a/></pivotCaches>");
    }
}
