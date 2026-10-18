//! Slicers and timelines ApexDesk keeps as they were: those of pivot
//! tables kept as they were (those ApexDesk can't recalculate) and those of
//! ordinary tables (Excel 2013), which ApexDesk has none of its own. Excel
//! still finds them connected: their cache parts (linked from the workbook
//! and listed in a workbook extension), their parts (linked from a sheet
//! and listed in a sheet extension), the frames that place them in the
//! sheet's drawing and their names (`Slicer_Region`, kept as ordinary
//! names but left out with their cache).
//!
//! What ties them together is noted in `KeptParts::xml`:
//! - "slicerCacheNeeds": cache part, newline, a pivot table part it filters;
//! - "slicerTable": cache part, newline, the table's name, tab, its id in
//!   the file, tab, the name of the column filtered (the ids of the table
//!   and column are set anew on save);
//! - "slicerCacheName": cache part, newline, the cache's name;
//! - "slicerBookExt": a workbook `<ext>` listing caches;
//! - "slicerSheetExt": the kept parts of a sheet (tab separated), newline,
//!   the sheet `<ext>` listing them;
//! - "slicerAnchor": slicer or timeline part, newline, the slicer's name,
//!   newline, its drawing anchor.
//! A cache is written when all its pivot tables are (or its table is
//! there), a slicer part with the slicers of the caches written; a part
//! mixing slicers of caches kept and of caches read as ApexDesk's keeps
//! only the first (ApexDesk writes the others in a part of its own).

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::kept::TableQuery;
use apex_doc::{KeptLink, KeptParts};
use apex_sheet::Workbook;

use crate::ooxml_keep::{ContentTypes, Reach, collect, read_rels, rel};
use crate::xml::{Ev, Reader, esc};
use crate::zip::{ZipReader, ZipWriter};

pub(crate) const REL_SLICER: &str = "http://schemas.microsoft.com/office/2007/relationships/slicer";
pub(crate) const REL_SLICER_CACHE: &str = "http://schemas.microsoft.com/office/2007/relationships/slicerCache";
const REL_TIMELINE: &str = "http://schemas.microsoft.com/office/2011/relationships/timeline";
const REL_TIMELINE_CACHE: &str = "http://schemas.microsoft.com/office/2011/relationships/timelineCache";
const REL_PIVOT_TABLE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/pivotTable";
/// Workbook extensions listing caches: slicer caches of Excel 2010 and
/// 2013, timeline caches.
const BOOK_EXT: [&str; 3] = ["{BBE1A952-AA13-448e-AADC-164F8A28A991}", "{46BE6895-7355-4a93-B00E-2C351335B9C9}", "{D0CA8CA8-9F24-4464-BF8E-62219DCF47F9}"];
/// Sheet extensions listing slicers (Excel 2010 and 2013) and timelines.
const SHEET_EXT: [&str; 3] = ["{A8765BA9-456A-4dab-B4F3-ACF838C121DE}", "{3A4CF648-6AED-40f4-86FF-DC5316D8AED3}", "{7E03D99C-DC04-49d9-9315-930204A7B6E9}"];

pub(crate) fn is_cache(ty: &str) -> bool {
    ty == REL_SLICER_CACHE || ty == REL_TIMELINE_CACHE
}

pub(crate) fn is_slicer(ty: &str) -> bool {
    ty == REL_SLICER || ty == REL_TIMELINE
}

/// Element of a slicer or timeline part standing for one of them.
fn item_local(ty: &str) -> &'static str {
    if ty == REL_TIMELINE { "timeline" } else { "slicer" }
}

/// Namespace declarations of the root element of `xml`, as attributes.
fn root_decls(xml: &str) -> Vec<(String, String)> {
    let mut r = Reader::new(xml);
    loop {
        match r.next() {
            Ev::Eof => return Vec::new(),
            Ev::Start(t) | Ev::Empty(t) => {
                return t.attrs().filter(|(k, _)| k.starts_with("xmlns")).map(|(k, v)| (k.to_string(), format!(r#" {k}="{}""#, esc(&v)))).collect();
            }
            _ => {}
        }
    }
}

/// Raw elements named `local` (with the root declarations they lack) and
/// the value of an attribute of each; `only` filters by it.
fn raw_elements(xml: &str, local: &str, only: &dyn Fn(&str) -> bool, key: &str) -> Vec<(String, String)> {
    let decls = root_decls(xml);
    let mut out = Vec::new();
    let mut r = Reader::new(xml);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) if t.local() == local => {
                let k = t.attr(key).unwrap_or_default().into_owned();
                if !only(&k) {
                    r.skip();
                    continue;
                }
                let own: Vec<&str> = t.attrs().map(|(k, _)| k).collect();
                let extra: String = decls.iter().filter(|(k, _)| !own.contains(&k.as_str())).map(|(_, d)| d.as_str()).collect();
                let body = r.raw_element();
                out.push((k, format!("<{}{}{extra}>{body}", t.name, t.raw_attrs())));
            }
            _ => {}
        }
    }
    out
}

/// Values of attribute `key` of the elements named `local`.
fn attrs_of(xml: &str, local: &str, key: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut r = Reader::new(xml);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == local => out.extend(t.attr(key).map(|v| v.into_owned())),
            _ => {}
        }
    }
    out
}

/// `xml` with only the `local` elements (slicers or timelines) whose cache
/// `keep` takes, and the names of those; `None` when it keeps them all.
fn filter_items(xml: &str, local: &str, keep: &dyn Fn(&str) -> bool) -> (Option<String>, Vec<String>) {
    let xml = xml.strip_prefix('\u{feff}').unwrap_or(xml);
    let mut out = String::with_capacity(xml.len());
    let mut names = Vec::new();
    let mut from = 0;
    let mut dropped = false;
    let mut r = Reader::new(xml);
    loop {
        let at = r.pos();
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == local => {
                let (name, cache) = (t.attr("name").unwrap_or_default().into_owned(), t.attr("cache").unwrap_or_default().into_owned());
                if !xml[at..r.pos()].ends_with("/>") {
                    r.skip();
                }
                if keep(&cache) {
                    names.push(name);
                } else {
                    out.push_str(&xml[from..at]);
                    from = r.pos();
                    dropped = true;
                }
            }
            _ => {}
        }
    }
    out.push_str(&xml[from..]);
    (dropped.then_some(out), names)
}

/// Tables of the file by id: their name and the names of their columns by
/// id.
fn file_tables(z: &ZipReader, sheets: &[String]) -> HashMap<String, (String, HashMap<String, String>)> {
    let mut out = HashMap::new();
    for path in sheets {
        for (_, ty, target, external) in read_rels(z, path) {
            if external || ty != crate::xlsx_table::REL_TABLE {
                continue;
            }
            let Ok(x) = z.read_string(&target) else { continue };
            let mut table: Option<(String, String)> = None;
            let mut cols = HashMap::new();
            let mut r = Reader::new(&x);
            loop {
                match r.next() {
                    Ev::Eof => break,
                    // `x14:table` in the extLst is not the table.
                    Ev::Start(t) | Ev::Empty(t) if t.local() == "table" && table.is_none() => {
                        table = t.attr("id").map(|v| v.into_owned()).zip(t.attr("displayName").or_else(|| t.attr("name")).map(|v| v.into_owned()));
                    }
                    Ev::Start(t) | Ev::Empty(t) if t.local() == "tableColumn" => {
                        if let (Some(id), Some(n)) = (t.attr("id"), t.attr("name")) {
                            cols.insert(id.into_owned(), crate::xlsx_table::unescape_x(&n));
                        }
                    }
                    _ => {}
                }
            }
            if let Some((id, name)) = table {
                out.insert(id, (name, cols));
            }
        }
    }
    out
}

/// Keeps the slicers and timelines of the pivot tables kept in `kept`
/// and those of the workbook's tables (`sheets`: the sheet parts;
/// `converted`: pivot table parts read as ApexDesk's).
pub(crate) fn read(z: &ZipReader, ct: &ContentTypes, wbxml: &str, sheets: &[String], converted: &HashSet<String>, wb: &mut Workbook, kept: &mut KeptParts) {
    let book_rels = read_rels(z, "xl/workbook.xml");
    if !book_rels.iter().any(|r| is_cache(&r.1)) {
        return;
    }
    // Pivot tables by (sheet id, name).
    let ids: Vec<String> = attrs_of(wbxml, "sheet", "sheetId");
    let mut pivots: HashMap<(String, String), String> = HashMap::new();
    for (i, path) in sheets.iter().enumerate() {
        for (_, ty, target, external) in read_rels(z, path) {
            if !external && ty == REL_PIVOT_TABLE {
                let name = z.read_string(&target).ok().and_then(|x| attrs_of(&x, "pivotTableDefinition", "name").pop()).unwrap_or_default();
                if let Some(id) = ids.get(i) {
                    pivots.insert((id.clone(), name), target);
                }
            }
        }
    }
    let tables = file_tables(z, sheets);
    // Caches kept (those whose pivot tables are all kept, or whose table
    // was read), by name.
    let mut caches: HashMap<String, String> = HashMap::new();
    for (id, ty, target, external) in &book_rels {
        if *external || !is_cache(ty) {
            continue;
        }
        let Ok(x) = z.read_string(target) else { continue };
        let Some(name) = attrs_of(&x, if ty == REL_SLICER_CACHE { "slicerCacheDefinition" } else { "timelineCacheDefinition" }, "name").pop() else { continue };
        let mut notes = Vec::new();
        if let Some((tid, col)) = attrs_of(&x, "tableSlicerCache", "tableId").pop().zip(attrs_of(&x, "tableSlicerCache", "column").pop()) {
            // A slicer of a table: kept while the table and its column
            // are there.
            let Some((table, cols)) = tables.get(&tid) else { continue };
            let Some(col) = cols.get(&col) else { continue };
            if !wb.sheets.iter().flat_map(|s| &s.tables).any(|t| t.name == *table) {
                continue;
            }
            notes.push(("slicerTable", format!("{target}\n{table}\t{tid}\t{col}")));
        } else {
            let mut ok = true;
            let mut r = Reader::new(&x);
            loop {
                match r.next() {
                    Ev::Eof => break,
                    Ev::Start(t) | Ev::Empty(t) if t.local() == "pivotTable" => {
                        let key = (t.attr("tabId").unwrap_or_default().into_owned(), t.attr("name").unwrap_or_default().into_owned());
                        match pivots.get(&key) {
                            Some(p) if !converted.contains(p) && kept.part(p).is_some() => notes.push(("slicerCacheNeeds", format!("{target}\n{p}"))),
                            _ => ok = false,
                        }
                    }
                    _ => {}
                }
            }
            if !ok || notes.is_empty() {
                continue;
            }
        }
        if collect(z, ct, target, kept) {
            kept.links.push(KeptLink { rel_type: ty.clone(), target: target.clone(), id: id.clone() });
            kept.xml.extend(notes.into_iter().map(|(k, v)| (k.to_string(), v)));
            kept.xml.push(("slicerCacheName".into(), format!("{target}\n{name}")));
            caches.insert(name, target.clone());
        }
    }
    if caches.is_empty() {
        return;
    }
    for (_, x) in raw_elements(wbxml, "ext", &|u| BOOK_EXT.iter().any(|k| k.eq_ignore_ascii_case(u)), "uri") {
        kept.xml.push(("slicerBookExt".into(), x));
    }
    for (i, path) in sheets.iter().enumerate() {
        let Some(sh) = wb.sheets.get_mut(i) else { break };
        let rels = read_rels(z, path);
        // Slicer and timeline parts showing kept caches (with just those
        // slicers when they show others too), and the names of what they
        // show.
        let mut parts: Vec<String> = Vec::new();
        let mut names: HashMap<String, String> = HashMap::new();
        for (id, ty, target, external) in &rels {
            if *external || !is_slicer(ty) {
                continue;
            }
            let Ok(x) = z.read_string(target) else { continue };
            let (only, shown) = filter_items(&x, item_local(ty), &|c| caches.contains_key(c));
            if shown.is_empty() || !collect(z, ct, target, kept) {
                continue;
            }
            if let Some((only, p)) = only.zip(kept.parts.iter_mut().find(|p| p.path == *target)) {
                p.data = Arc::from(only.into_bytes());
            }
            sh.kept.push(KeptLink { rel_type: ty.clone(), target: target.clone(), id: id.clone() });
            for n in shown {
                names.insert(n, target.clone());
            }
            parts.push(target.clone());
        }
        if parts.is_empty() {
            continue;
        }
        if let Ok(x) = z.read_string(path) {
            for (_, e) in raw_elements(&x, "ext", &|u| SHEET_EXT.iter().any(|k| k.eq_ignore_ascii_case(u)), "uri") {
                kept.xml.push(("slicerSheetExt".into(), format!("{}\n{e}", parts.join("\t"))));
            }
        }
        for (_, ty, target, external) in &rels {
            if *external || ty != crate::xlsx_drawing::REL_DRAWING {
                continue;
            }
            let Ok(x) = z.read_string(target) else { continue };
            for anchor in ["twoCellAnchor", "oneCellAnchor", "absoluteAnchor"] {
                for (_, a) in raw_elements(&x, anchor, &|_| true, "editAs") {
                    // The frame names the slicer or timeline it shows.
                    let shown = attrs_of(&a, "slicer", "name").into_iter().chain(attrs_of(&a, "timeslicer", "name")).find_map(|n| Some((names.get(&n)?, n)));
                    if let Some((part, name)) = shown {
                        kept.xml.push(("slicerAnchor".into(), format!("{part}\n{name}\n{a}")));
                    }
                }
            }
        }
    }
}

/// Names of the slicers of the kept slicer parts.
pub(crate) fn kept_names(kept: &KeptParts) -> Vec<String> {
    kept.parts.iter().filter(|p| p.path.starts_with("xl/slicers/") && p.path.ends_with(".xml")).flat_map(|p| attrs_of(&String::from_utf8_lossy(&p.data), "slicer", "name")).collect()
}

/// Gives the relationship ids (`r:id`) of the list items in `xml` their new
/// values, leaving out the items whose part isn't written; `None` if none
/// is left.
fn filter_ids(xml: &str, ids: &HashMap<&str, String>) -> Option<String> {
    let mut out = String::new();
    let mut rest = xml;
    let mut any = false;
    while let Some(i) = rest.find(r#" r:id=""#) {
        let v0 = i + r#" r:id=""#.len();
        let v1 = v0 + rest[v0..].find('"')?;
        let start = rest[..i].rfind('<')?;
        let end = v1 + rest[v1..].find('>')? + 1;
        match ids.get(&rest[v0..v1]) {
            Some(new) => {
                any = true;
                out.push_str(&rest[..v0]);
                out.push_str(new);
                out.push_str(&rest[v1..end]);
            }
            None => out.push_str(&rest[..start]),
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    any.then_some(out)
}

/// The notes of slot `slot` about `part`: what follows its line.
fn notes<'a>(kept: &'a KeptParts, slot: &'a str, part: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    kept.xml.iter().filter(move |(s, _)| s == slot).filter_map(|(_, x)| x.split_once('\n')).filter(move |(p, _)| *p == part).map(|(_, rest)| rest)
}

/// `xml` with attributes of its first `local` element set to new values.
fn set_attrs(xml: &str, local: &str, values: &[(&str, String)]) -> String {
    let xml = xml.strip_prefix('\u{feff}').unwrap_or(xml);
    let mut r = Reader::new(xml);
    loop {
        let at = r.pos();
        match r.next() {
            Ev::Eof => return xml.to_string(),
            Ev::Start(t) | Ev::Empty(t) if t.local() == local => {
                let end = r.pos();
                let mut tag = xml[at..end].to_string();
                for (k, v) in values {
                    let pat = format!(r#" {k}=""#);
                    if let Some(v0) = tag.find(&pat).map(|i| i + pat.len())
                        && let Some(n) = tag[v0..].find('"')
                    {
                        tag.replace_range(v0..v0 + n, &esc(v));
                    }
                }
                return format!("{}{tag}{}", &xml[..at], &xml[end..]);
            }
            _ => {}
        }
    }
}

/// The ids (table, column) a table slicer cache gets, as `xlsx_table`
/// writes the tables: numbered over the workbook, columns by position or
/// by the id their query gives them. The table is found by name, else by
/// its id in the file (renamed); `None` if it or its column is gone.
fn table_ids(wb: &Workbook, queries: &[Vec<Option<Arc<TableQuery>>>], note: &str) -> Option<(usize, u32)> {
    let mut it = note.splitn(3, '\t');
    let (name, id, col) = (it.next()?, it.next()?, it.next()?);
    let all: Vec<(usize, usize, &apex_sheet::table::Table)> = wb.sheets.iter().enumerate().flat_map(|(si, s)| s.tables.iter().enumerate().map(move |(ti, t)| (si, ti, t))).collect();
    let n = all.iter().position(|(_, _, t)| t.name.eq_ignore_ascii_case(name)).or_else(|| all.iter().position(|(_, _, t)| t.id.to_string() == id))?;
    let (si, ti, t) = all[n];
    let ci = t.columns.iter().position(|c| c.name.eq_ignore_ascii_case(col))?;
    let q = queries.get(si).and_then(|v| v.get(ti)).and_then(|q| q.as_deref());
    Some((n + 1, q.and_then(|q| q.columns.get(ci)).map_or(ci as u32 + 1, |c| c.1)))
}

/// What the kept slicers add to a workbook being written.
#[derive(Default)]
pub(crate) struct Out {
    /// Workbook `<ext>` elements.
    pub book_ext: String,
    /// Per sheet: `<ext>` elements and drawing anchors.
    pub sheet_ext: Vec<String>,
    pub anchors: Vec<String>,
    /// Content type overrides of the parts written here.
    pub content_types: String,
    /// Names of the caches left out (lowercase): their defined names go
    /// with them.
    pub dropped: HashSet<String>,
}

/// Writes a kept part, as it was or as `data`.
fn add_part(z: &mut ZipWriter, kept: &KeptParts, path: &str, data: Option<&str>, ct: &mut String) {
    let Some(p) = kept.part(path) else { return };
    z.add(path, data.map_or(&p.data[..], str::as_bytes), true);
    if !p.content_type.is_empty() {
        let _ = write!(ct, r#"<Override PartName="/{}" ContentType="{}"/>"#, esc(path), esc(&p.content_type));
    }
}

/// Writes the kept caches still of use (once the pivot tables are in
/// `reach`; `queries`: those of the tables, as `xlsx_keep` writes them)
/// and the slicers of each sheet that show them. The workbook's
/// relationships go to `rels`, the sheets' to `sheet_rels`.
pub(crate) fn write(wb: &Workbook, queries: &[Vec<Option<Arc<TableQuery>>>], reach: &mut Reach<'_>, rels: &mut String, sheet_rels: &mut [String], z: &mut ZipWriter) -> Out {
    let kept = &wb.kept;
    let n = wb.sheets.len();
    let mut out = Out { sheet_ext: vec![String::new(); n], anchors: vec![String::new(); n], ..Out::default() };
    let mut book_ids: HashMap<&str, String> = HashMap::new();
    let mut written: HashSet<&str> = HashSet::new();
    for (i, l) in kept.links.iter().enumerate() {
        if !is_cache(&l.rel_type) {
            continue;
        }
        let ok = match notes(kept, "slicerTable", &l.target).next() {
            // The table's and column's ids as written now.
            Some(t) => match (table_ids(wb, queries, t), kept.part(&l.target)) {
                (Some((table, col)), Some(p)) => {
                    let x = set_attrs(&String::from_utf8_lossy(&p.data), "tableSlicerCache", &[("tableId", table.to_string()), ("column", col.to_string())]);
                    add_part(z, kept, &l.target, Some(&x), &mut out.content_types);
                    true
                }
                _ => false,
            },
            None => notes(kept, "slicerCacheNeeds", &l.target).all(|p| reach.has(p)) && reach.add(&l.target),
        };
        if ok {
            let id = format!("rIdK{}", i + 1);
            rels.push_str(&rel(&id, l, "xl"));
            book_ids.insert(l.id.as_str(), id);
            written.insert(l.target.as_str());
        }
    }
    // Caches written, by name.
    let mut shown: HashSet<&str> = HashSet::new();
    for (_, x) in kept.xml.iter().filter(|(s, _)| s == "slicerCacheName") {
        let Some((part, name)) = x.split_once('\n') else { continue };
        if written.contains(part) {
            shown.insert(name);
        } else {
            out.dropped.insert(name.to_lowercase());
        }
    }
    for (_, x) in kept.xml.iter().filter(|(s, _)| s == "slicerBookExt") {
        out.book_ext.extend(filter_ids(x, &book_ids));
    }
    let mut used: HashSet<&str> = HashSet::new();
    for (si, sh) in wb.sheets.iter().enumerate() {
        let mut ids: HashMap<&str, String> = HashMap::new();
        // Parts written and the slicers they keep.
        let mut parts: HashMap<&str, Vec<String>> = HashMap::new();
        for (j, l) in sh.kept.iter().enumerate() {
            if !is_slicer(&l.rel_type) || used.contains(l.target.as_str()) {
                continue;
            }
            let Some(p) = kept.part(&l.target) else { continue };
            let (only, names) = filter_items(&String::from_utf8_lossy(&p.data), item_local(&l.rel_type), &|c| shown.contains(c));
            if names.is_empty() {
                continue;
            }
            used.insert(l.target.as_str());
            add_part(z, kept, &l.target, only.as_deref(), &mut out.content_types);
            let id = format!("rIdKS{}", j + 1);
            sheet_rels[si].push_str(&rel(&id, l, "xl/worksheets"));
            ids.insert(l.id.as_str(), id);
            parts.insert(l.target.as_str(), names);
        }
        if parts.is_empty() {
            continue;
        }
        for (_, x) in kept.xml.iter().filter(|(s, _)| s == "slicerSheetExt") {
            let Some((list, e)) = x.split_once('\n') else { continue };
            if list.split('\t').any(|p| parts.contains_key(p)) {
                out.sheet_ext[si].extend(filter_ids(e, &ids));
            }
        }
        for (_, x) in kept.xml.iter().filter(|(s, _)| s == "slicerAnchor") {
            let mut it = x.splitn(3, '\n');
            let (Some(part), Some(name), Some(a)) = (it.next(), it.next(), it.next()) else { continue };
            if parts.get(part).is_some_and(|names| names.iter().any(|n| n == name)) {
                out.anchors[si].push_str(a);
            }
        }
    }
    out
}
