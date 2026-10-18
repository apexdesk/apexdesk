//! Slicers of pivot tables in xlsx, as Excel 2010 keeps them: a slicer
//! cache per filtered field (`xl/slicerCaches/`, listed in a workbook
//! extension, with the field's items and which are selected), the slicers
//! of each sheet (`xl/slicers/`, listed in a sheet extension) and a
//! `sle:slicer` frame in the sheet's drawing that places each one.
//! Excel also names each cache (`Slicer_Region`, standing for #N/A).

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use apex_sheet::analysis::Slicer;
use apex_sheet::analysis::pivot::{self, FieldFilter, Grouping};
use apex_sheet::{CellRef, ImageFormat, SheetImage, Value, Workbook};

use crate::xlsx_pivot::{NS_X14, Native, ReadTable};
use crate::xml::{Ev, HEADER, Reader, esc};
use crate::zip::{ZipReader, ZipWriter};

const REL_SLICER: &str = "http://schemas.microsoft.com/office/2007/relationships/slicer";
const REL_SLICER_CACHE: &str = "http://schemas.microsoft.com/office/2007/relationships/slicerCache";
const CT_SLICER: &str = "application/vnd.ms-excel.slicer+xml";
const CT_SLICER_CACHE: &str = "application/vnd.ms-excel.slicerCache+xml";
/// Workbook extension listing the slicer caches.
const EXT_CACHES: &str = "{BBE1A952-AA13-448e-AADC-164F8A28A991}";
/// Sheet extension listing its slicers.
const EXT_LIST: &str = "{A8765BA9-456A-4dab-B4F3-ACF838C121DE}";
const NS_SLE: &str = "http://schemas.microsoft.com/office/drawing/2010/slicer";
const NS_MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const NS_MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
/// Row height of the buttons (EMU), Excel's default.
const ROW_HEIGHT: u32 = 241_300;

#[derive(Default)]
pub(crate) struct Out {
    pub book_rels: String,
    pub content_types: String,
    /// `<ext>` of workbook.xml.
    pub book_ext: String,
    /// `<definedName>` elements.
    pub names: String,
    /// Per sheet: relationships, `<ext>` of its extLst and drawing anchors.
    pub sheet_rels: Vec<String>,
    pub sheet_ext: Vec<String>,
    pub anchors: Vec<String>,
}

/// Name of a slicer cache for a field: `Slicer_` and the field name with
/// what isn't a letter or digit made `_`.
fn cache_name(field: &str, taken: &mut HashSet<String>) -> String {
    let base: String = format!("Slicer_{field}").chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
    let mut name = base.clone();
    let mut k = 1;
    while !taken.insert(name.to_lowercase()) {
        k += 1;
        name = format!("{base}{k}");
    }
    name
}

/// Writes the slicers of the pivot tables written as Excel's (`natives`).
pub(crate) fn write(wb: &Workbook, z: &mut ZipWriter, natives: &[Native]) -> Out {
    let n = wb.sheets.len();
    let mut out = Out { sheet_rels: vec![String::new(); n], sheet_ext: vec![String::new(); n], anchors: vec![String::new(); n], ..Out::default() };
    let mut taken: HashSet<String> = wb.names.iter().map(|d| d.name.to_lowercase()).collect();
    // One cache per (pivot table, field), shared by its slicers.
    let mut caches: HashMap<(usize, u32), String> = HashMap::new();
    let mut cache_list = String::new();
    let mut per_sheet: Vec<Vec<String>> = vec![Vec::new(); n];
    let mut slicer_names: HashSet<String> = crate::xlsx_slicer_keep::kept_names(&wb.kept).into_iter().map(|n| n.to_lowercase()).collect();
    // Part numbers not taken by kept slicers.
    let free = |dir: &str, stem: &str, from: usize| (from..).find(|m| wb.kept.part(&format!("xl/{dir}/{stem}{m}.xml")).is_none()).unwrap_or(from);
    let mut last_cache = 0;
    for (k, s) in wb.analysis.slicers.iter().enumerate() {
        let Some(nat) = natives.iter().find(|x| x.index == s.pivot) else { continue };
        let Some(si) = wb.sheet_index(&s.sheet) else { continue };
        let Some((field, items)) = nat.fields.get(s.col as usize) else { continue };
        let Some(p) = wb.analysis.pivots.get(s.pivot) else { continue };
        let cname = match caches.get(&(s.pivot, s.col)) {
            Some(c) => c.clone(),
            None => {
                let c = cache_name(field, &mut taken);
                let m = free("slicerCaches", "slicerCache", last_cache + 1);
                last_cache = m;
                let sel: Option<HashSet<String>> = p.spec.slicers.iter().find(|f| f.field.col == s.col).filter(|f| !f.items.is_empty()).map(|f| f.items.iter().map(|i| i.to_lowercase()).collect());
                let mut x = String::from(HEADER);
                let _ = write!(
                    x,
                    r#"<slicerCacheDefinition xmlns="{NS_X14}" xmlns:mc="{NS_MC}" mc:Ignorable="x" xmlns:x="{NS_MAIN}" name="{}" sourceName="{}"><pivotTables><pivotTable tabId="{}" name="{}"/></pivotTables><data><tabular pivotCacheId="{}"><items count="{}">"#,
                    esc(&c),
                    esc(field),
                    nat.sheet + 1,
                    esc(&nat.name),
                    nat.cache_id,
                    items.len()
                );
                for (i, v) in items.iter().enumerate() {
                    let on = sel.as_ref().is_none_or(|s| s.contains(&pivot::item_text(v, Grouping::None, nat.lang).to_lowercase()));
                    let _ = write!(x, r#"<i x="{i}"{}/>"#, if on { r#" s="1""# } else { "" });
                }
                x.push_str("</items></tabular></data></slicerCacheDefinition>");
                z.add(&format!("xl/slicerCaches/slicerCache{m}.xml"), x.as_bytes(), true);
                let _ = write!(out.content_types, r#"<Override PartName="/xl/slicerCaches/slicerCache{m}.xml" ContentType="{CT_SLICER_CACHE}"/>"#);
                let _ = write!(out.book_rels, r#"<Relationship Id="rIdSC{m}" Type="{REL_SLICER_CACHE}" Target="slicerCaches/slicerCache{m}.xml"/>"#);
                let _ = write!(cache_list, r#"<x14:slicerCache r:id="rIdSC{m}"/>"#);
                let _ = write!(out.names, r#"<definedName name="{}">#N/A</definedName>"#, esc(&c));
                caches.insert((s.pivot, s.col), c.clone());
                c
            }
        };
        let mut name = s.caption.clone();
        if name.is_empty() {
            name = field.clone();
        }
        let base = name.clone();
        let mut j = 1;
        while !slicer_names.insert(name.to_lowercase()) {
            j += 1;
            name = format!("{base} {j}");
        }
        per_sheet[si].push(format!(r#"<slicer name="{}" cache="{}" caption="{}" columnCount="{}" rowHeight="{ROW_HEIGHT}"/>"#, esc(&name), esc(&cname), esc(&s.caption), s.columns.max(1)));
        out.anchors[si].push_str(&anchor(wb, si, s, &name, 10_000 + k));
    }
    if !cache_list.is_empty() {
        out.book_ext = format!(r#"<ext uri="{EXT_CACHES}" xmlns:x14="{NS_X14}"><x14:slicerCaches>{cache_list}</x14:slicerCaches></ext>"#);
    }
    let mut last = 0;
    for (si, list) in per_sheet.iter().enumerate() {
        if list.is_empty() {
            continue;
        }
        let k = free("slicers", "slicer", last + 1);
        last = k;
        z.add(&format!("xl/slicers/slicer{k}.xml"), format!(r#"{HEADER}<slicers xmlns="{NS_X14}" xmlns:mc="{NS_MC}" mc:Ignorable="x" xmlns:x="{NS_MAIN}">{}</slicers>"#, list.concat()).as_bytes(), true);
        let _ = write!(out.content_types, r#"<Override PartName="/xl/slicers/slicer{k}.xml" ContentType="{CT_SLICER}"/>"#);
        out.sheet_rels[si] = format!(r#"<Relationship Id="rIdSL{k}" Type="{REL_SLICER}" Target="../slicers/slicer{k}.xml"/>"#);
        out.sheet_ext[si] = format!(r#"<ext uri="{EXT_LIST}" xmlns:x14="{NS_X14}"><x14:slicerList><x14:slicer r:id="rIdSL{k}"/></x14:slicerList></ext>"#);
    }
    out
}

/// The drawing anchor of a slicer: its frame for Excel 2010 and later, a
/// plain box for older programs.
fn anchor(wb: &Workbook, si: usize, s: &Slicer, name: &str, id: usize) -> String {
    let sh = &wb.sheets[si];
    let mut img = SheetImage::new(std::sync::Arc::from(&[][..]), ImageFormat::Png, CellRef::new(0, 0), s.width, s.height);
    img.set_pos(sh, s.x, s.y);
    let [from, to] = crate::xlsx_drawing::box_corners(sh, img.from, img.dx, img.dy, s.width, s.height);
    format!(
        r#"<xdr:twoCellAnchor editAs="oneCell"><xdr:from>{}</xdr:from><xdr:to>{}</xdr:to><mc:AlternateContent xmlns:mc="{NS_MC}"><mc:Choice xmlns:a14="http://schemas.microsoft.com/office/drawing/2010/main" Requires="a14"><xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="{id}" name="{}"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></xdr:xfrm><a:graphic><a:graphicData uri="{NS_SLE}"><sle:slicer xmlns:sle="{NS_SLE}" name="{}"/></a:graphicData></a:graphic></xdr:graphicFrame></mc:Choice><mc:Fallback><xdr:sp macro="" textlink=""><xdr:nvSpPr><xdr:cNvPr id="0" name=""/><xdr:cNvSpPr><a:spLocks noTextEdit="1"/></xdr:cNvSpPr></xdr:nvSpPr><xdr:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{}" cy="{}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></xdr:spPr><xdr:txBody><a:bodyPr vertOverflow="clip" horzOverflow="clip"/><a:lstStyle/><a:p><a:r><a:t>This shape represents a PivotTable slicer. Slicers are supported in Excel 2010 or later.</a:t></a:r></a:p></xdr:txBody></xdr:sp></mc:Fallback></mc:AlternateContent><xdr:clientData/></xdr:twoCellAnchor>"#,
        crate::xlsx_drawing::corner_xml(from),
        crate::xlsx_drawing::corner_xml(to),
        esc(name),
        esc(name),
        (s.width * 9525.0) as i64,
        (s.height * 9525.0) as i64
    )
}

/// Adds the slicer anchors to a sheet's drawing (made when it has none).
pub(crate) fn add_anchors(drawing: &mut Option<(String, String)>, anchors: &str) {
    if anchors.is_empty() {
        return;
    }
    match drawing {
        Some((x, _)) => {
            if let Some(i) = x.rfind("</xdr:wsDr>") {
                x.insert_str(i, anchors);
            }
        }
        None => {
            *drawing = Some((
                format!(r#"{HEADER}<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">{anchors}</xdr:wsDr>"#),
                format!(r#"{HEADER}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"></Relationships>"#),
            ));
        }
    }
}

/// Puts `<drawing>` (when the sheet had none and gets slicer frames) and
/// the slicer list extensions in a sheet's XML.
pub(crate) fn patch_sheet(xml: &mut String, rels: &mut Option<String>, n: usize, had_drawing: bool, anchors: bool, ext: &str) {
    if anchors && !had_drawing {
        crate::xlsx_table::add_rels(rels, &format!(r#"<Relationship Id="rIdSLD" Type="{}" Target="../drawings/drawing{n}.xml"/>"#, crate::xlsx_drawing::REL_DRAWING));
        // `<drawing>` goes before these, whichever comes first.
        let at = ["<legacyDrawing ", "<legacyDrawingHF ", "<drawingHF ", "<picture ", "<oleObjects>", "<controls>", "<webPublishItems", "<tableParts ", "<extLst>", "</worksheet>"].iter().filter_map(|m| xml.find(m)).min();
        if let Some(i) = at {
            xml.insert_str(i, r#"<drawing r:id="rIdSLD"/>"#);
        }
    }
    if ext.is_empty() {
        return;
    }
    match xml.rfind("</extLst></worksheet>") {
        Some(i) => xml.insert_str(i, ext),
        None => {
            if let Some(i) = xml.rfind("</worksheet>") {
                xml.insert_str(i, &format!("<extLst>{ext}</extLst>"));
            }
        }
    }
}

/// `<extLst>` of workbook.xml with the kept extensions (`kept`, a whole
/// element or "") and `ours`.
pub(crate) fn merge_ext(kept: &str, ours: &str) -> String {
    if ours.is_empty() {
        return kept.to_string();
    }
    match kept.strip_suffix("</extLst>") {
        Some(body) => format!("{body}{ours}</extLst>"),
        None => format!("<extLst>{ours}</extLst>"),
    }
}

/// Kept `<ext>` elements (`kept`) and ours (`ours`, one listing our
/// slicers or caches in a `list` element): ours go into the kept list of
/// the same kind (same extension uri) when there is one, as Excel wants
/// one per kind.
pub(crate) fn join_ext(kept: &str, ours: &str, list: &str) -> String {
    let (open, close) = (format!("<{list}>"), format!("</{list}>"));
    let items = ours.find(&open).and_then(|a| Some(&ours[a + open.len()..a + ours[a..].find(&close)?]));
    // Where the kept extension of our kind closes its list.
    let lower = kept.to_ascii_lowercase();
    let uri = ours.split(r#"uri=""#).nth(1).and_then(|v| v.split('"').next()).map(|u| format!(r#"uri="{}""#, u.to_ascii_lowercase()));
    let at = uri.and_then(|u| lower.find(&u)).and_then(|a| {
        let i = a + 1 + kept[a + 1..].find(&close)?;
        (!lower[a + 1..i].contains(r#"uri=""#)).then_some(i)
    });
    match (items, at) {
        (Some(items), Some(i)) => format!("{}{items}{}", &kept[..i], &kept[i..]),
        _ => format!("{kept}{ours}"),
    }
}

// ---------------------------------------------------------------------------
// Reading

struct CacheDef {
    name: String,
    source: String,
    tables: Vec<(u32, String)>,
    /// x14 id of the pivot cache.
    pivot_cache: Option<u32>,
    /// (cache item, selected).
    items: Vec<(u32, bool)>,
}

fn read_cache_def(xml: &str) -> Option<CacheDef> {
    let mut r = Reader::new(xml);
    let mut c = CacheDef { name: String::new(), source: String::new(), tables: Vec::new(), pivot_cache: None, items: Vec::new() };
    let mut tabular = false;
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) => match t.local() {
                "slicerCacheDefinition" => {
                    c.name = t.attr("name")?.into_owned();
                    c.source = t.attr("sourceName").unwrap_or_default().into_owned();
                }
                "pivotTable" => c.tables.push((t.attr("tabId").and_then(|v| v.parse().ok()).unwrap_or(0), t.attr("name").unwrap_or_default().into_owned())),
                "tabular" => {
                    tabular = true;
                    c.pivot_cache = t.attr("pivotCacheId").and_then(|v| v.parse().ok());
                }
                "i" if tabular => c.items.push((t.attr("x").and_then(|v| v.parse().ok()).unwrap_or(0), t.attr("s").is_some_and(|v| v == "1" || v == "true"))),
                // Slicers of tables (not pivot tables) and OLAP ones.
                "tableSlicerCache" | "olap" => tabular = false,
                _ => {}
            },
            _ => {}
        }
    }
    Some(c)
}

/// Reads the slicers of the pivot tables read from Excel (`tables`), and
/// drops the names Excel gives slicer caches.
pub(crate) fn read(z: &ZipReader, wbxml: &str, sheets: &[String], tables: &[ReadTable], wb: &mut Workbook) {
    let rels: HashMap<String, String> = crate::ooxml_keep::read_rels(z, "xl/workbook.xml").into_iter().filter(|r| r.1 == REL_SLICER_CACHE && !r.3).map(|(id, _, t, _)| (id, t)).collect();
    if rels.is_empty() {
        return;
    }
    // Sheet ids, in sheet order.
    let mut ids: Vec<u32> = Vec::new();
    let mut r = Reader::new(wbxml);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "sheet" => ids.push(t.attr("sheetId").and_then(|v| v.parse().ok()).unwrap_or(0)),
            _ => {}
        }
    }
    let mut caches: HashMap<String, (usize, u32)> = HashMap::new();
    let mut names: HashSet<String> = HashSet::new();
    for target in rels.values() {
        // Those of pivot tables kept as they were are kept with them.
        if wb.kept.links.iter().any(|l| l.target == *target) {
            continue;
        }
        let Some(c) = z.read_string(target).ok().and_then(|x| read_cache_def(&x)) else { continue };
        names.insert(c.name.to_lowercase());
        let by_table = c.tables.iter().find_map(|(tab, name)| tables.iter().find(|t| ids.get(t.sheet) == Some(tab) && t.name == *name));
        let Some(t) = by_table.or_else(|| tables.iter().find(|t| c.pivot_cache.is_some() && t.cache_x14 == c.pivot_cache)) else { continue };
        let Some(col) = t.fields.iter().position(|(n, _)| *n == c.source) else { continue };
        let items: &[Value] = &t.fields[col].1;
        let label = |x: u32| items.get(x as usize).map(|v| pivot::item_text(v, Grouping::None, t.lang));
        let sel: Vec<String> = if c.items.iter().all(|(_, s)| *s) { Vec::new() } else { c.items.iter().filter(|(_, s)| *s).filter_map(|(x, _)| label(*x)).collect() };
        let spec = &mut wb.analysis.pivots[t.index].spec;
        if !spec.slicers.iter().any(|f| f.field.col == col as u32) {
            spec.slicers.push(FieldFilter { field: pivot::Field::new(col as u32), items: sel });
        }
        caches.insert(c.name.clone(), (t.index, col as u32));
    }
    wb.names.retain(|d| !(d.scope.is_none() && names.contains(&d.name.to_lowercase())));
    if caches.is_empty() {
        return;
    }
    for (si, path) in sheets.iter().enumerate() {
        if si >= wb.sheets.len() {
            break;
        }
        let srels = crate::ooxml_keep::read_rels(z, path);
        let mut boxes: HashMap<String, (f32, f32, f32, f32)> = HashMap::new();
        for (_, ty, target, ext) in &srels {
            if !ext && ty == crate::xlsx_drawing::REL_DRAWING {
                if let Ok(d) = z.read_string(target) {
                    read_boxes(&d, &wb.sheets[si], &mut boxes);
                }
            }
        }
        for (_, ty, target, ext) in &srels {
            if *ext || ty != REL_SLICER {
                continue;
            }
            let Ok(x) = z.read_string(target) else { continue };
            let mut r = Reader::new(&x);
            loop {
                match r.next() {
                    Ev::Eof => break,
                    Ev::Start(t) | Ev::Empty(t) if t.local() == "slicer" => {
                        let a = |k: &str| t.attr(k).unwrap_or_default().into_owned();
                        let Some(&(pivot, col)) = caches.get(&a("cache")) else { continue };
                        let (x, y, width, height) = boxes.get(&a("name")).copied().unwrap_or((0.0, 0.0, 180.0, 220.0));
                        let caption = t.attr("caption").map(|v| v.into_owned()).unwrap_or_else(|| a("name"));
                        let columns = t.attr("columnCount").and_then(|v| v.parse::<u8>().ok()).unwrap_or(1).clamp(1, 20);
                        wb.analysis.slicers.push(Slicer { sheet: wb.sheets[si].name.clone(), x, y, width, height, pivot, col, caption, columns });
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Boxes (x, y, width, height in sheet pixels) of the slicer frames of a
/// drawing, by slicer name.
fn read_boxes(xml: &str, sh: &apex_sheet::Sheet, out: &mut HashMap<String, (f32, f32, f32, f32)>) {
    let mut r = Reader::new(xml);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) if t.local() == "twoCellAnchor" || t.local() == "oneCellAnchor" => {
                let body = r.raw_element();
                if !body.contains(NS_SLE) {
                    continue;
                }
                let mut name = None;
                let mut corners: Vec<(u32, u32, f32, f32)> = Vec::new();
                let mut br = Reader::new(body);
                let mut cur: Option<(u32, u32, f32, f32)> = None;
                let mut field = "";
                loop {
                    match br.next() {
                        Ev::Eof => break,
                        Ev::Start(c) if c.local() == "from" || c.local() == "to" => cur = Some((0, 0, 0.0, 0.0)),
                        Ev::End(n) if crate::xml::local(n) == "from" || crate::xml::local(n) == "to" => corners.extend(cur.take()),
                        Ev::Start(c) if cur.is_some() => field = c.local(),
                        Ev::Text(v) if cur.is_some() => {
                            let c = cur.as_mut().unwrap();
                            match field {
                                "col" => c.0 = v.trim().parse().unwrap_or(0),
                                "row" => c.1 = v.trim().parse().unwrap_or(0),
                                "colOff" => c.2 = crate::xlsx_drawing::emu(&v),
                                "rowOff" => c.3 = crate::xlsx_drawing::emu(&v),
                                _ => {}
                            }
                        }
                        Ev::Start(c) | Ev::Empty(c) if c.local() == "slicer" => name = c.attr("name").map(|v| v.into_owned()),
                        _ => {}
                    }
                }
                if let (Some(n), [from, to, ..]) = (name, corners.as_slice()) {
                    if let Some(img) = crate::xlsx_drawing::place_box(sh, *from, *to, true) {
                        let (x, y) = img.pos(sh);
                        out.insert(n, (x, y, img.width, img.height));
                    }
                }
            }
            _ => {}
        }
    }
}
