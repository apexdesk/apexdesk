//! The file inspector: what a workbook, document or presentation carries
//! that doesn't show on screen (macros, links to other files, hidden
//! sheets, rows and slides, comments, tracked changes, properties, data
//! connections, custom XML, embedded objects…), and removing each kind for
//! good as one undo step. Signatures belong to the file on disk, not to
//! the model: the app adds them.

use std::sync::Arc;

use apex_doc::{Document, Inspect, KeptLink, KeptParts, Selection, cleared_meta, is_file_link};
use apex_sheet::{CellRef, FileParts, Op, Workbook};
use apex_slides::object::{ObjKind, Object, Picture};
use apex_slides::{Deck, Presentation};

/// A kind of thing the inspector finds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Hidden {
    Macros,
    /// Formulas or links that read other files.
    ExternalLinks,
    HiddenSheets,
    HiddenRows,
    HiddenCols,
    /// Defined names marked as hidden.
    HiddenNames,
    Comments,
    Revisions,
    Properties,
    Signatures,
    /// Data connections, Power Query queries, the data model and the Get
    /// Data queries.
    Connections,
    /// Pivot tables of other programs, kept with their caches: a copy of
    /// all their source data.
    PivotCaches,
    CustomXml,
    /// Objects embedded by other programs (OLE).
    Embedded,
    HiddenText,
    Headers,
    HiddenSlides,
    /// Objects marked as hidden, and objects off the slide.
    HiddenObjects,
    SpeakerNotes,
    /// A book's snapshots (earlier versions of its text), scene synopses
    /// and daily word counts.
    BookNotes,
}

impl Hidden {
    /// Kinds that can also be shown instead of removed.
    pub fn can_show(self) -> bool {
        matches!(self, Hidden::HiddenSheets | Hidden::HiddenRows | Hidden::HiddenCols | Hidden::HiddenSlides | Hidden::HiddenObjects)
    }

    /// Name used by `apexdesk inspect --remove`.
    pub fn code(self) -> &'static str {
        match self {
            Hidden::Macros => "macros",
            Hidden::ExternalLinks => "links",
            Hidden::HiddenSheets => "hidden-sheets",
            Hidden::HiddenRows => "hidden-rows",
            Hidden::HiddenCols => "hidden-columns",
            Hidden::HiddenNames => "hidden-names",
            Hidden::Comments => "comments",
            Hidden::Revisions => "revisions",
            Hidden::Properties => "properties",
            Hidden::Signatures => "signatures",
            Hidden::Connections => "connections",
            Hidden::PivotCaches => "pivot-caches",
            Hidden::CustomXml => "custom-xml",
            Hidden::Embedded => "embedded",
            Hidden::HiddenText => "hidden-text",
            Hidden::Headers => "headers",
            Hidden::HiddenSlides => "hidden-slides",
            Hidden::HiddenObjects => "hidden-objects",
            Hidden::SpeakerNotes => "notes",
            Hidden::BookNotes => "book-notes",
        }
    }

    pub const ALL: [Hidden; 20] = [
        Hidden::Macros,
        Hidden::ExternalLinks,
        Hidden::HiddenSheets,
        Hidden::HiddenRows,
        Hidden::HiddenCols,
        Hidden::HiddenNames,
        Hidden::Comments,
        Hidden::Revisions,
        Hidden::Properties,
        Hidden::Signatures,
        Hidden::Connections,
        Hidden::PivotCaches,
        Hidden::CustomXml,
        Hidden::Embedded,
        Hidden::HiddenText,
        Hidden::Headers,
        Hidden::HiddenSlides,
        Hidden::HiddenObjects,
        Hidden::SpeakerNotes,
        Hidden::BookNotes,
    ];

    pub fn from_code(s: &str) -> Option<Hidden> {
        Hidden::ALL.into_iter().find(|h| h.code() == s)
    }
}

/// What was found of one kind: how many and which (names, addresses,
/// files; at most [`MAX_ITEMS`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub what: Hidden,
    pub count: usize,
    pub items: Vec<String>,
}

pub const MAX_ITEMS: usize = 50;

fn found(what: Hidden, items: Vec<String>) -> Option<Found> {
    let count = items.len();
    (count > 0).then(|| Found { what, count, items: items.into_iter().take(MAX_ITEMS).collect() })
}

fn properties(m: &apex_doc::Meta) -> Vec<String> {
    let fields = [("title", &m.title), ("author", &m.author), ("subject", &m.subject), ("keywords", &m.keywords), ("comments", &m.description), ("category", &m.category), ("company", &m.company), ("manager", &m.manager)];
    let mut out: Vec<String> = fields.iter().filter(|(_, v)| !v.trim().is_empty()).map(|(n, v)| format!("{n}: {v}")).collect();
    out.extend(m.custom.iter().filter(|p| !apex_doc::kept::is_label_prop(&p.name)).map(|p| format!("{}: {}", p.name, p.value.text())));
    out
}

fn macros(m: &apex_doc::Macros) -> Vec<String> {
    let mut out = Vec::new();
    if m.project.is_some() {
        out.push("VBA".to_string());
    }
    out.extend(m.odf.iter().filter(|p| p.path.starts_with("Basic/") || p.path.starts_with("Scripts/")).filter(|p| !p.path.ends_with("-lc.xml") && !p.path.ends_with("-lb.xml")).map(|p| p.path.clone()));
    if out.is_empty() && !m.parts.is_empty() {
        // Ribbon customizations and the like, without code.
        out.extend(m.parts.iter().map(|p| p.path.clone()));
    }
    out
}

/// Custom XML parts (not the Power Query ones, which are connections).
fn custom_xml(k: &KeptParts) -> Vec<String> {
    k.parts.iter().filter(|p| p.path.starts_with("customXml/item") && !p.path.contains("Props") && !is_mashup(&p.data)).map(|p| p.path.clone()).collect()
}

fn is_mashup(data: &[u8]) -> bool {
    let x = crate::ooxml_keep::decode_xml(data).0;
    x.contains("DataMashup")
}

/// Removes the custom XML parts (the Power Query ones too with `mashup`).
fn drop_custom_xml(k: &mut KeptParts, mashup: bool) {
    let gone: Vec<String> = k.parts.iter().filter(|p| p.path.starts_with("customXml/") && (mashup || !is_mashup(&p.data) || p.path.contains("Props"))).map(|p| p.path.clone()).collect();
    // The properties part of a Power Query item stays with it.
    let keep_props: Vec<String> = if mashup { Vec::new() } else { k.parts.iter().filter(|p| p.path.starts_with("customXml/item") && !p.path.contains("Props") && is_mashup(&p.data)).map(|p| p.path.replace("item", "itemProps")).collect() };
    let gone: Vec<String> = gone.into_iter().filter(|g| !keep_props.contains(g)).collect();
    k.parts.retain(|p| !gone.contains(&p.path));
    k.links.retain(|l| !gone.contains(&l.target));
    k.package_links.retain(|l| !gone.contains(&l.target));
}

// --- workbooks ----------------------------------------------------------------------

fn col_name(c: u32) -> String {
    CellRef::new(0, c).to_string().trim_end_matches(char::is_numeric).to_string()
}

/// Rows hidden by hand (not by a filter) of sheet `si`.
fn hidden_rows(wb: &Workbook, si: usize) -> Vec<u32> {
    let sh = &wb.sheets[si];
    sh.row_heights.iter().filter(|(r, h)| **h == 0.0 && !sh.filter.as_ref().is_some_and(|f| f.range.start.row < **r && **r <= f.range.end.row)).map(|(r, _)| *r).collect()
}

fn hidden_cols(wb: &Workbook, si: usize) -> Vec<u32> {
    wb.sheets[si].col_widths.iter().filter(|(_, w)| **w == 0.0).map(|(c, _)| *c).collect()
}

fn file_cell_links(wb: &Workbook) -> Vec<(usize, CellRef, String)> {
    let mut out = Vec::new();
    for (si, sh) in wb.sheets.iter().enumerate() {
        for (c, cell) in sh.iter() {
            if let Some(l) = cell.link().filter(|l| is_file_link(&l.target) && !l.target.contains('!')) {
                out.push((si, c, l.target.clone()));
            }
        }
    }
    out
}

/// What workbook `wb` carries that doesn't show.
pub fn workbook(wb: &Workbook) -> Vec<Found> {
    let mut out = Vec::new();
    out.extend(found(Hidden::Macros, macros(&wb.macros)));
    let mut links: Vec<String> = wb.externals.iter().map(|b| b.target.clone()).collect();
    for (_, _, t) in file_cell_links(wb) {
        if !links.contains(&t) {
            links.push(t);
        }
    }
    out.extend(found(Hidden::ExternalLinks, links));
    out.extend(found(Hidden::HiddenSheets, wb.sheets.iter().filter(|s| s.hidden).map(|s| s.name.clone()).collect()));
    let mut rows = Vec::new();
    let mut cols = Vec::new();
    for (si, sh) in wb.sheets.iter().enumerate() {
        rows.extend(hidden_rows(wb, si).into_iter().map(|r| format!("{}!{}", sh.name, r + 1)));
        cols.extend(hidden_cols(wb, si).into_iter().map(|c| format!("{}!{}", sh.name, col_name(c))));
    }
    out.extend(found(Hidden::HiddenRows, rows));
    out.extend(found(Hidden::HiddenCols, cols));
    out.extend(found(Hidden::HiddenNames, wb.names.iter().filter(|n| n.hidden && !n.name.starts_with("_xlnm")).map(|n| n.name.clone()).collect()));
    let mut notes = Vec::new();
    for sh in &wb.sheets {
        for (c, cell) in sh.iter() {
            if let Some(n) = cell.note() {
                let who = if n.author.is_empty() { String::new() } else { format!(" ({})", n.author) };
                notes.push(format!("{}!{c}{who}: {}", sh.name, n.text.chars().take(60).collect::<String>()));
            }
        }
    }
    out.extend(found(Hidden::Comments, notes));
    out.extend(found(Hidden::Properties, properties(&wb.meta)));
    let data = crate::external_data(wb);
    let mut conns: Vec<String> = data.connections.iter().map(|c| if c.source.is_empty() { c.name.clone() } else { format!("{}: {}", c.name, c.source) }).collect();
    conns.extend(data.queries.iter().map(|q| format!("Power Query: {}", q.name)));
    if data.data_model {
        conns.push("Data model".into());
    }
    conns.extend(wb.queries.iter().map(|q| q.name.clone()));
    out.extend(found(Hidden::Connections, conns));
    let pivots = wb.sheets.iter().flat_map(|sh| sh.kept.iter().filter(|l| is_pivot(l)).map(|l| format!("{}: {}", sh.name, l.target))).collect();
    out.extend(found(Hidden::PivotCaches, pivots));
    out.extend(found(Hidden::CustomXml, custom_xml(&wb.kept)));
    let mut objs = Vec::new();
    for sh in &wb.sheets {
        for im in &sh.images {
            if let Some(o) = im.object.as_ref().and_then(|o| o.ole()) {
                objs.push(format!("{}: {}", sh.name, o.prog_id));
            }
        }
    }
    out.extend(found(Hidden::Embedded, objs));
    out
}

fn wb_parts(wb: &Workbook) -> FileParts {
    FileParts { meta: wb.meta.clone(), macros: wb.macros.clone(), kept: wb.kept.clone(), sheet_kept: wb.sheets.iter().map(|s| s.kept.clone()).collect() }
}

fn is_pivot(l: &KeptLink) -> bool {
    l.rel_type.ends_with("/pivotTable") || l.rel_type.ends_with("/pivotCacheDefinition")
}

/// Drops the connections, the data model and the Power Query queries a
/// workbook keeps, with the query tables of its sheets.
fn drop_connections(p: &mut FileParts) {
    let conn = |l: &KeptLink| l.rel_type.ends_with("/connections") || l.rel_type.ends_with("/powerPivotData") || l.target.starts_with("xl/model/");
    p.kept.links.retain(|l| !conn(l));
    for k in &mut p.sheet_kept {
        k.retain(|l| !l.rel_type.ends_with("/queryTable"));
    }
    let mashups: Vec<String> = p.kept.parts.iter().filter(|x| x.path.starts_with("customXml/item") && !x.path.contains("Props") && is_mashup(&x.data)).map(|x| x.path.clone()).collect();
    let props: Vec<String> = mashups.iter().map(|m| m.replace("item", "itemProps")).collect();
    p.kept.parts.retain(|x| !mashups.contains(&x.path) && !props.contains(&x.path));
    p.kept.links.retain(|l| !mashups.contains(&l.target));
}

/// Removes (or with `show`, shows) what the inspector found of `what`
/// in `wb`, as one undo step. False when there was nothing to do.
pub fn clean_workbook(wb: &mut Workbook, what: Hidden, show: bool) -> bool {
    let mut ops = Vec::new();
    match what {
        Hidden::Macros | Hidden::Properties | Hidden::CustomXml | Hidden::Connections | Hidden::PivotCaches => {
            let mut p = wb_parts(wb);
            match what {
                Hidden::PivotCaches => {
                    if !p.sheet_kept.iter().flatten().any(is_pivot) {
                        return false;
                    }
                    p.kept.links.retain(|l| !is_pivot(l));
                    for k in &mut p.sheet_kept {
                        k.retain(|l| !is_pivot(l));
                    }
                }
                Hidden::Macros => p.macros = Default::default(),
                Hidden::Properties => p.meta = cleared_meta(&p.meta),
                Hidden::CustomXml => drop_custom_xml(&mut p.kept, false),
                _ => {
                    drop_connections(&mut p);
                    if !wb.queries.is_empty() {
                        ops.push(Op::Queries(Vec::new()));
                    }
                }
            }
            ops.push(Op::FileParts(Box::new(p)));
        }
        Hidden::ExternalLinks => {
            let steps = wb.undo_len();
            let n = wb.externals.len();
            for _ in 0..n {
                wb.break_external(0);
            }
            let mut cells: Vec<(usize, Vec<(CellRef, Option<apex_sheet::Cell>)>)> = Vec::new();
            for (si, c, _) in file_cell_links(wb) {
                let Some(mut cell) = wb.sheets[si].get(c).cloned() else { continue };
                cell.set_extra(|x| x.link = None);
                let cell = (cell.content != apex_sheet::Content::Empty || cell.extra.is_some() || cell.style != 0).then_some(cell);
                match cells.last_mut() {
                    Some((s, v)) if *s == si => v.push((c, cell)),
                    _ => cells.push((si, vec![(c, cell)])),
                }
            }
            let links = !cells.is_empty();
            if links {
                wb.execute_as(Op::Batch(cells.into_iter().map(|(sheet, cells)| Op::SetCells { sheet, cells }).collect()), "undo.inspect");
            }
            let done = wb.undo_len() - steps;
            wb.merge_undo(done, "undo.inspect");
            return n > 0 || links;
        }
        Hidden::HiddenSheets => {
            let hidden: Vec<usize> = (0..wb.sheets.len()).filter(|&i| wb.sheets[i].hidden).collect();
            if show {
                ops.extend(hidden.iter().map(|&index| Op::SheetProps { index, hidden: false, tab_color: wb.sheets[index].tab_color }));
            } else if hidden.len() < wb.sheets.len() {
                ops.extend(hidden.iter().rev().map(|&index| Op::RemoveSheet { index }));
            }
        }
        Hidden::HiddenRows | Hidden::HiddenCols => {
            let rows = what == Hidden::HiddenRows;
            for si in 0..wb.sheets.len() {
                let list = if rows { hidden_rows(wb, si) } else { hidden_cols(wb, si) };
                if list.is_empty() {
                    continue;
                }
                if show {
                    let sizes = list.into_iter().map(|i| (i, None)).collect();
                    ops.push(if rows { Op::RowHeights { sheet: si, sizes } } else { Op::ColWidths { sheet: si, sizes } });
                } else if rows {
                    ops.push(Op::DeleteRowsAt { sheet: si, rows: list });
                } else {
                    ops.extend(list.into_iter().rev().map(|at| Op::DeleteCols { sheet: si, at, count: 1 }));
                }
            }
        }
        Hidden::HiddenNames => {
            if wb.names.iter().any(|n| n.hidden && !n.name.starts_with("_xlnm")) {
                ops.push(Op::Names(wb.names.iter().filter(|n| !n.hidden || n.name.starts_with("_xlnm")).cloned().collect()));
            }
        }
        Hidden::Comments => {
            for (si, sh) in wb.sheets.iter().enumerate() {
                let cells: Vec<(CellRef, Option<apex_sheet::Cell>)> = sh
                    .iter()
                    .filter(|(_, c)| c.note().is_some())
                    .map(|(at, c)| {
                        let mut c = c.clone();
                        c.set_extra(|x| x.note = None);
                        (at, (c.content != apex_sheet::Content::Empty || c.extra.is_some() || c.style != 0).then_some(c))
                    })
                    .collect();
                if !cells.is_empty() {
                    ops.push(Op::SetCells { sheet: si, cells });
                }
            }
        }
        Hidden::Embedded => {
            for (si, sh) in wb.sheets.iter().enumerate() {
                if sh.images.iter().any(|i| i.object.as_ref().is_some_and(|o| o.ole().is_some())) {
                    let list = sh.images.iter().map(|i| if i.object.as_ref().is_some_and(|o| o.ole().is_some()) { apex_sheet::SheetImage { object: None, ..i.clone() } } else { i.clone() }).collect();
                    ops.push(Op::Images { sheet: si, list });
                }
            }
        }
        _ => {}
    }
    if ops.is_empty() {
        return false;
    }
    wb.execute_as(Op::Batch(ops), "undo.inspect");
    true
}

// --- documents ----------------------------------------------------------------------

/// What document `d` carries that doesn't show.
pub fn document(d: &Document) -> Vec<Found> {
    let f = d.inspect();
    let mut out = Vec::new();
    out.extend(found(Hidden::Macros, macros(&d.macros)));
    out.extend(found(Hidden::ExternalLinks, d.file_links()));
    out.extend(found(Hidden::Comments, d.comments.iter().map(|c| format!("{}: {}", c.author, c.text.chars().take(60).collect::<String>())).collect()));
    let revs = d.changes();
    out.extend(found(Hidden::Revisions, revs.iter().map(|r| r.rev.author.to_string()).collect()));
    out.extend(found(Hidden::Properties, properties(&d.meta).into_iter().chain(d.property_fields()).collect()));
    out.extend(found(Hidden::CustomXml, custom_xml(&d.kept)));
    out.extend(found(Hidden::Embedded, d.ole_objects()));
    if f.hidden > 0 {
        out.push(Found { what: Hidden::HiddenText, count: f.hidden, items: Vec::new() });
    }
    if f.found(Inspect::Headers) {
        out.push(Found { what: Hidden::Headers, count: f.headers + f.watermark as usize, items: Vec::new() });
    }
    if let Some(b) = &d.book {
        let mut items: Vec<String> = b.snapshots.iter().map(|s| format!("{} {}", s.date, if s.label.is_empty() { &s.title } else { &s.label })).collect();
        items.extend(b.scenes.iter().filter(|s| !s.synopsis.is_empty()).map(|s| s.synopsis.chars().take(60).collect::<String>()));
        if !b.progress.is_empty() {
            items.push(format!("{} days of word counts", b.progress.len()));
        }
        out.extend(found(Hidden::BookNotes, items));
    }
    out
}

/// Removes what the inspector found of `what` in `d`, as one undo step.
pub fn clean_document(d: &mut Document, what: Hidden) -> bool {
    let sel = Selection::default();
    match what {
        Hidden::Macros if !d.macros.is_empty() => d.edit_parts(|p| p.macros = Default::default()),
        Hidden::Properties => d.remove_inspected(Inspect::Properties, sel),
        Hidden::CustomXml if !custom_xml(&d.kept).is_empty() => d.edit_parts(|p| drop_custom_xml(&mut p.kept, true)),
        Hidden::ExternalLinks => return d.remove_file_links(),
        Hidden::Embedded => return d.remove_ole(),
        Hidden::Comments | Hidden::Revisions => d.remove_inspected(Inspect::Comments, sel),
        Hidden::HiddenText => d.remove_inspected(Inspect::HiddenText, sel),
        Hidden::Headers => d.remove_inspected(Inspect::Headers, sel),
        Hidden::BookNotes if d.book.as_ref().is_some_and(|b| !b.snapshots.is_empty() || !b.progress.is_empty() || b.scenes.iter().any(|s| !s.synopsis.is_empty())) => d.edit_parts(|p| {
            if let Some(b) = &mut p.book {
                b.snapshots.clear();
                b.progress.clear();
                for s in &mut b.scenes {
                    s.synopsis.clear();
                }
            }
        }),
        _ => return false,
    }
    true
}

// --- presentations ------------------------------------------------------------------

fn off_slide(o: &Object, deck: &Deck) -> bool {
    o.sp.frame.is_some_and(|f| f.x + f.w <= 0.0 || f.y + f.h <= 0.0 || f.x >= deck.width || f.y >= deck.height)
}

fn is_ole(o: &Object) -> bool {
    matches!(&o.kind, ObjKind::Other { what, .. } if matches!(&**what, "ole" | "object"))
}

fn walk<'a>(v: &'a [Object], f: &mut impl FnMut(&'a Object)) {
    for o in v {
        f(o);
        if let ObjKind::Group(g) = &o.kind {
            walk(&g.children, f);
        }
    }
}

/// What the presentation carries that doesn't show.
pub fn deck(d: &Deck) -> Vec<Found> {
    let mut out = Vec::new();
    out.extend(found(Hidden::Macros, macros(&d.macros)));
    let (mut links, mut comments, mut hidden, mut notes, mut objects, mut ole) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (i, s) in d.slides.iter().enumerate() {
        let n = i + 1;
        if s.hidden {
            hidden.push(format!("{n}: {}", s.title()));
        }
        comments.extend(s.comments.iter().map(|c| format!("{n} ({}): {}", c.author, c.text.chars().take(60).collect::<String>())));
        // pptx comments aren't read, but go back with the slide.
        if let Some(r) = slide_rels(d, s)
            && without_comments(&String::from_utf8_lossy(&r.data)).is_some()
        {
            comments.push(format!("{n}"));
        }
        if !s.notes_text().trim().is_empty() {
            notes.push(format!("{n}: {}", s.notes_text().chars().take(60).collect::<String>()));
        }
        walk(&s.objects, &mut |o| {
            if o.hidden || off_slide(o, d) {
                objects.push(format!("{n}: {}", o.name));
            }
            if is_ole(o) {
                ole.push(format!("{n}: {}", o.name));
            }
            for t in [o.click.as_deref(), if let ObjKind::Picture(p) = &o.kind { p.media.as_ref().and_then(|m| m.link.as_deref()) } else { None }].into_iter().flatten() {
                if is_file_link(t) && !links.iter().any(|l| l == t) {
                    links.push(t.to_string());
                }
            }
        });
    }
    out.extend(found(Hidden::ExternalLinks, links));
    out.extend(found(Hidden::HiddenSlides, hidden));
    out.extend(found(Hidden::HiddenObjects, objects));
    out.extend(found(Hidden::Comments, comments));
    out.extend(found(Hidden::SpeakerNotes, notes));
    out.extend(found(Hidden::Properties, properties(&d.meta)));
    out.extend(found(Hidden::Embedded, ole));
    out
}

fn map_objects(v: &mut Vec<Object>, f: &impl Fn(Object) -> Option<Object>) {
    let old = std::mem::take(v);
    for mut o in old {
        if let ObjKind::Group(g) = &mut o.kind {
            map_objects(&mut g.children, f);
        }
        if let Some(o) = f(o) {
            v.push(o);
        }
    }
}

/// Removes (or with `show`, shows) what the inspector found of `what`
/// in the presentation, as one undo step.
pub fn clean_deck(p: &mut Presentation, what: Hidden, show: bool) -> bool {
    let before = deck(&p.deck);
    if !before.iter().any(|f| f.what == what) {
        return false;
    }
    let (w, h) = (p.deck.width, p.deck.height);
    let d = p.edit("undo.inspect");
    match what {
        Hidden::Macros => d.macros = Default::default(),
        Hidden::Properties => d.meta = cleared_meta(&d.meta),
        Hidden::HiddenSlides if show => {
            for s in &mut d.slides {
                if s.hidden {
                    let s = Arc::make_mut(s);
                    s.hidden = false;
                    changed(s);
                }
            }
        }
        Hidden::HiddenSlides => {
            if d.slides.iter().any(|s| !s.hidden) {
                d.slides.retain(|s| !s.hidden);
            }
        }
        _ => {
            if what == Hidden::Comments {
                // pptx comments the slides' kept relationships still point
                // to, and their parts.
                let pkg = Arc::make_mut(&mut d.package);
                for part in &mut pkg.parts {
                    if part.path.starts_with("ppt/slides/_rels/")
                        && let Some(x) = without_comments(&String::from_utf8_lossy(&part.data))
                    {
                        part.data = x.into_bytes().into();
                    }
                }
                // The comments, and who wrote them (names, initials, ids).
                pkg.parts.retain(|p| !p.path.starts_with("ppt/comments/") && p.path != "ppt/commentAuthors.xml" && p.path != "ppt/authors.xml");
            }
            for s in &mut d.slides {
                let s = Arc::make_mut(s);
                let hit = std::cell::Cell::new(match what {
                    Hidden::Comments => !std::mem::take(&mut s.comments).is_empty(),
                    Hidden::SpeakerNotes => s.notes.take().is_some(),
                    _ => false,
                });
                map_objects(&mut s.objects, &|mut o: Object| {
                    let off = o.sp.frame.is_some_and(|f| f.x + f.w <= 0.0 || f.y + f.h <= 0.0 || f.x >= w || f.y >= h);
                    match what {
                        Hidden::HiddenObjects if show => {
                            hit.set(true);
                            o.hidden = false;
                            if let (true, Some(f)) = (off, o.sp.frame.as_mut()) {
                                f.x = f.x.clamp(0.0, (w - f.w).max(0.0));
                                f.y = f.y.clamp(0.0, (h - f.h).max(0.0));
                            }
                            o.raw = None;
                        }
                        Hidden::HiddenObjects if o.hidden || off => {
                            hit.set(true);
                            return None;
                        }
                        Hidden::Embedded if is_ole(&o) => {
                            let ObjKind::Other { preview, .. } = &o.kind else { return Some(o) };
                            hit.set(true);
                            let preview = preview.clone()?;
                            let f = o.sp.frame.unwrap_or_default();
                            o.kind = ObjKind::Picture(Box::new(Picture { image: apex_doc::Image::new(preview, f.w, f.h), media: None }));
                            o.raw = None;
                        }
                        Hidden::ExternalLinks => {
                            if o.click.as_deref().is_some_and(is_file_link) {
                                hit.set(true);
                                o.click = None;
                                o.raw = None;
                            }
                            if let ObjKind::Picture(pic) = &mut o.kind
                                && pic.media.as_ref().is_some_and(|m| m.link.as_deref().is_some_and(is_file_link))
                            {
                                hit.set(true);
                                pic.media = None;
                                o.raw = None;
                            }
                        }
                        _ => {}
                    }
                    Some(o)
                });
                // Written again when saved: the part as read would bring
                // back what was removed.
                if hit.get() || what == Hidden::Comments {
                    changed(s);
                }
            }
        }
    }
    true
}

/// Marks a slide (and its notes) changed, so saving writes it again.
fn changed(s: &mut apex_slides::Slide) {
    for o in [&mut s.origin].into_iter().chain(s.notes.as_mut().map(|n| &mut n.origin)).flatten() {
        o.same = false;
    }
}

/// The kept relationships of a slide read from a pptx.
fn slide_rels<'d>(d: &'d Deck, s: &apex_slides::Slide) -> Option<&'d apex_doc::KeptPart> {
    let o = s.origin.as_ref().filter(|o| o.src == apex_slides::Src::Pptx)?;
    let (dir, file) = o.part.rsplit_once('/')?;
    d.package.part(&format!("{dir}/_rels/{file}.rels"))
}

/// A relationships part without its relationships to comments (classic
/// and modern), or None when it has none.
fn without_comments(xml: &str) -> Option<String> {
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    let mut dropped = false;
    while let Some(i) = rest.find("<Relationship ") {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let mut end = tail.find('>').map_or(tail.len(), |e| e + 1);
        if !tail[..end].ends_with("/>")
            && let Some(c) = tail[end..].find("</Relationship>")
        {
            end += c + "</Relationship>".len();
        }
        let el = &tail[..end];
        if el.contains("relationships/comments\"") || el.contains("relationships/comments'") {
            dropped = true;
        } else {
            out.push_str(el);
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    dropped.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workbook_finds_and_cleans() {
        let mut wb = Workbook::new();
        wb.execute(Op::AddSheet { index: 1, sheet: Box::new(apex_sheet::Sheet::new("Secret")) });
        wb.execute(Op::SheetProps { index: 1, hidden: true, tab_color: None });
        wb.set_input(0, CellRef::new(0, 0), "1");
        wb.set_input(0, CellRef::new(1, 0), "2");
        wb.set_input(0, CellRef::new(2, 0), "=A2*2");
        wb.execute(Op::RowHeights { sheet: 0, sizes: vec![(0, Some(0.0))] });
        wb.sheets[0].edit_extra(CellRef::new(1, 0), |x| x.note = Some(apex_sheet::Note { text: "hi".into(), author: "Ana".into(), ..Default::default() }));
        wb.meta.author = "Ana".into();
        wb.macros.project = Some(Arc::new(vec![1]));
        let f = workbook(&wb);
        let kinds: Vec<Hidden> = f.iter().map(|f| f.what).collect();
        assert_eq!(kinds, [Hidden::Macros, Hidden::HiddenSheets, Hidden::HiddenRows, Hidden::Comments, Hidden::Properties]);
        assert_eq!(f[2].items, ["Sheet1!1"]);
        let steps = wb.undo_len();
        for k in kinds {
            assert!(clean_workbook(&mut wb, k, false));
        }
        assert_eq!(wb.undo_len(), steps + 5);
        assert!(workbook(&wb).is_empty());
        assert_eq!(wb.sheets.len(), 1);
        // The hidden row went, and the formula followed its cell.
        assert_eq!(wb.edit_text(0, CellRef::new(1, 0)), "=A1*2");
        assert!(wb.macros.project.is_none() && wb.meta.author.is_empty());
        for _ in 0..5 {
            wb.undo();
        }
        assert_eq!(workbook(&wb).len(), 5);
        // Shown instead of removed.
        assert!(clean_workbook(&mut wb, Hidden::HiddenRows, true));
        assert!(clean_workbook(&mut wb, Hidden::HiddenSheets, true));
        assert!(!wb.sheets[1].hidden && wb.sheets[0].row_heights.get(&0).is_none());
    }

    #[test]
    fn deck_finds_and_cleans() {
        let mut d = apex_slides::build::new_deck(0);
        let s = Arc::make_mut(&mut d.slides[0]);
        s.comments.push(apex_slides::Comment { author: "Ana".into(), text: "check".into(), ..Default::default() });
        let mut o = Object::new(9, "Away", ObjKind::Shape { text: None, text_box: false, connector: false });
        o.sp.frame = Some(apex_slides::object::Frame { x: -500.0, y: 0.0, w: 100.0, h: 100.0, ..Default::default() });
        s.objects.push(o);
        let mut p = Presentation::new(d);
        let f = deck(&p.deck);
        assert_eq!(f.iter().map(|f| f.what).collect::<Vec<_>>(), [Hidden::HiddenObjects, Hidden::Comments]);
        assert!(clean_deck(&mut p, Hidden::Comments, false));
        assert!(clean_deck(&mut p, Hidden::HiddenObjects, true));
        assert!(deck(&p.deck).is_empty());
        p.undo();
        assert!(clean_deck(&mut p, Hidden::HiddenObjects, false));
        assert!(p.deck.slides[0].find(9).is_none());
    }

    #[test]
    fn cleaned_slides_are_written_again() {
        let mut d = apex_slides::build::new_deck(0);
        let s = Arc::make_mut(&mut d.slides[0]);
        s.origin = apex_slides::Origin::pptx("ppt/slides/slide1.xml");
        let mut o = Object::new(9, "Away", ObjKind::Shape { text: None, text_box: false, connector: false });
        o.hidden = true;
        s.objects.push(o);
        let rels = r#"<Relationships><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments" Target="../comments/comment1.xml"/></Relationships>"#;
        let pkg = Arc::make_mut(&mut d.package);
        pkg.parts.push(apex_doc::KeptPart { path: "ppt/slides/_rels/slide1.xml.rels".into(), content_type: String::new(), data: rels.as_bytes().into() });
        pkg.parts.push(apex_doc::KeptPart { path: "ppt/comments/comment1.xml".into(), content_type: String::new(), data: b"<p:cmLst/>".as_slice().into() });
        let mut p = Presentation::new(d);
        assert!(deck(&p.deck).iter().any(|f| f.what == Hidden::Comments));
        assert!(clean_deck(&mut p, Hidden::HiddenObjects, false));
        assert!(p.deck.slides[0].origin.as_ref().is_some_and(|o| !o.same));
        assert!(clean_deck(&mut p, Hidden::Comments, false));
        let kept = &p.deck.package;
        assert!(kept.part("ppt/comments/comment1.xml").is_none());
        let r = String::from_utf8_lossy(&kept.part("ppt/slides/_rels/slide1.xml.rels").unwrap().data).into_owned();
        assert!(r.contains("slideLayout1") && !r.contains("comment1"));
        assert!(deck(&p.deck).is_empty());
    }
}
