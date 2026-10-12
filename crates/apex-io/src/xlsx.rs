//! Office Open XML spreadsheets (.xlsx).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_sheet::format::date_to_serial;
use apex_sheet::formula::parse_canonical;
use apex_sheet::{CellRef, Content, ErrorKind, Range, Sheet, Value, Workbook};

use crate::xml::{Ev, HEADER, Reader, Tag, esc};
use crate::zip::{ZipReader, ZipWriter};
use crate::{Error, parse_hex_color};

// Unit conversions between Excel units and our logical pixels.
const COL_SCALE: f32 = 88.0 / 64.0;
pub(crate) const ROW_SCALE: f32 = 22.0 / 15.0;

/// A sheet's default row height or column width as read, kept usable:
/// none (or infinite) would have every row or column of the sheet shown.
pub(crate) fn default_size(v: f32, fallback: f32) -> f32 {
    if v.is_finite() { v.clamp(4.0, apex_sheet::sheet::MAX_SIZE) } else { fallback }
}

pub(crate) fn width_from_chars(w: f32) -> f32 {
    ((w * 7.0 + 5.0).round() * COL_SCALE).max(0.0)
}

/// A width in characters, at most 255 (the schema's limit; Excel caps
/// column widths there too).
fn width_to_chars(px: f32) -> f32 {
    (((px / COL_SCALE - 5.0) / 7.0 * 100.0).round() / 100.0).min(255.0)
}

/// Shared strings: `Arc<str>` values and the runs of the rich ones.
type Strings = (Vec<Arc<str>>, Vec<Option<Arc<apex_sheet::rich::RichText>>>);

const MAX_SHARED_STRINGS: usize = 5_000_000;

fn read_shared_strings(xml: &str, theme: &apex_sheet::Theme) -> Strings {
    let (mut out, mut rich) = (Vec::new(), Vec::new());
    let mut r = Reader::new(xml);
    // Past more strings than a sheet's cells could use, a small part of
    // empty `<si/>` would only fill memory.
    while out.len() < MAX_SHARED_STRINGS {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) if t.local() == "si" => {
                let (text, runs) = crate::xlsx_rich::read_rst(&mut r, theme);
                out.push(Arc::from(text));
                rich.push(runs.map(Arc::new));
            }
            Ev::Empty(t) if t.local() == "si" => {
                out.push(Arc::from(""));
                rich.push(None);
            }
            _ => {}
        }
    }
    (out, rich)
}

pub(crate) fn read_rels(xml: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    let mut r = Reader::new(xml);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "Relationship" => {
                if let (Some(id), Some(target)) = (t.attr("Id"), t.attr("Target").filter(|t| t.len() <= crate::docx::MAX_REL_TARGET)) {
                    m.insert(id.into_owned(), target.into_owned());
                }
            }
            _ => {}
        }
    }
    m
}

pub(crate) fn resolve_target(base_dir: &str, target: &str) -> String {
    // An absolute target is resolved from the root, ".." and all.
    let (base_dir, target) = match target.strip_prefix('/') {
        Some(abs) => ("", abs),
        None => (base_dir, target),
    };
    let mut parts: Vec<&str> = base_dir.split('/').filter(|s| !s.is_empty()).collect();
    for seg in target.split('/') {
        match seg {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            s => parts.push(s),
        }
    }
    parts.join("/")
}

pub fn read(data: &[u8]) -> Result<Workbook, Error> {
    let z = ZipReader::new(data)?;
    let mut wb = Workbook::empty();
    let rels = z.read_string("xl/_rels/workbook.xml.rels").map(|s| read_rels(&s)).unwrap_or_default();
    let theme_path = rels.values().find(|t| t.contains("theme/")).map_or_else(|| "xl/theme/theme1.xml".to_string(), |t| resolve_target("xl", t));
    let theme = z.read_string(&theme_path).map(|s| crate::theme_xml::read_theme(&s)).unwrap_or_default();
    let typed_rels = z.read_string("xl/_rels/workbook.xml.rels").map(|s| crate::xlsx_notes::read_rels(&s)).unwrap_or_default();
    let checkboxes = crate::xlsx_incell::read_checkbox_complements(&z, &typed_rels);
    let pictures = crate::xlsx_incell::read_cell_pictures(&z, &typed_rels);
    let (xfs, dxfs) = match z.read_string("xl/styles.xml") {
        Ok(s) => (crate::xlsx_styles::read_styles(&s, &mut wb, &theme, &checkboxes), crate::xlsx_data::read_dxfs(&s)),
        Err(_) => {
            wb.cell_styles = apex_sheet::CellStyles::new(theme, &wb.locale);
            (Vec::new(), Vec::new())
        }
    };
    let strings = z.read_string("xl/sharedStrings.xml").map(|s| read_shared_strings(&s, &wb.cell_styles.theme)).unwrap_or_default();
    let persons = z
        .read_string("xl/_rels/workbook.xml.rels")
        .ok()
        .and_then(|x| crate::xlsx_notes::read_rels(&x).into_values().find(|(t, _, _)| t == crate::xlsx_threaded::REL_PERSON))
        .and_then(|(_, target, _)| z.read_string(&resolve_target("xl", &target)).ok())
        .map(|x| crate::xlsx_threaded::read_persons(&x))
        .unwrap_or_default();
    let wbxml = z.read_string("xl/workbook.xml")?;
    let mut sheets: Vec<(String, String, bool)> = Vec::new();
    // Names (lowercase) and parts of the sheets so far.
    let mut seen: std::collections::HashSet<String> = Default::default();
    // Print areas and titles: (name, local sheet, reference).
    let mut names: Vec<(String, Option<usize>, String)> = Vec::new();
    // Data of the newer charts: (name, reference).
    let mut chart_names: Vec<(String, String)> = Vec::new();
    // Defined names: (name, local sheet, formula, comment, hidden).
    let mut user_names: Vec<(String, Option<usize>, String, String, bool)> = Vec::new();
    let mut read_only_rec = false;
    let mut r = Reader::new(&wbxml);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "fileSharing" => read_only_rec = t.attr("readOnlyRecommended").is_some_and(|v| v == "1" || v == "true"),
            Ev::Start(t) if t.local() == "definedName" => {
                let name = t.attr("name").unwrap_or_default().into_owned();
                let local = t.attr("localSheetId").and_then(|v| v.trim().parse().ok());
                let comment = t.attr("comment").unwrap_or_default().into_owned();
                let hidden = t.attr("hidden").is_some_and(|v| v == "1" || v == "true");
                let text = r.text();
                if name.starts_with("_xlnm.Print_") {
                    names.push((name, local, text));
                } else if name.starts_with("_xlchart.") {
                    chart_names.push((name, text));
                } else if !name.starts_with("_xlnm.") {
                    // Built-in names (_FilterDatabase...) are made again on save.
                    // Formulas look names up one by one: a book's worth.
                    if user_names.len() < 10_000 {
                        user_names.push((name, local, text, comment, hidden));
                    }
                }
            }
            Ev::Start(t) | Ev::Empty(t) if t.local() == "calcPr" => {
                wb.calc.manual = t.attr("calcMode").is_some_and(|v| v == "manual");
                wb.calc.r1c1 = t.attr("refMode").is_some_and(|v| v == "R1C1");
                wb.calc.as_shown = t.attr("fullPrecision").is_some_and(|v| v == "0" || v == "false");
                wb.calc.iterate = t.attr("iterate").is_some_and(|v| v == "1" || v == "true");
                if let Some(n) = t.attr("iterateCount").and_then(|v| v.parse().ok()) {
                    wb.calc.max_iter = n;
                }
                if let Some(d) = t.attr("iterateDelta").and_then(|v| v.parse().ok()) {
                    wb.calc.max_change = d;
                }
            }
            Ev::Start(t) | Ev::Empty(t) if t.local() == "workbookProtection" => wb.protection = crate::xlsx_notes::read_workbook_protection(&t),
            Ev::Start(t) | Ev::Empty(t) if t.local() == "workbookPr" => wb.calc.date1904 = t.attr("date1904").is_some_and(|v| v == "1" || v == "true"),
            Ev::Start(t) | Ev::Empty(t) if t.local() == "sheet" => {
                let name = t.attr("name").unwrap_or_default().into_owned();
                let rid = t.attr("r:id").or_else(|| t.attr("id")).unwrap_or_default();
                let hidden = t.attr("state").is_some_and(|v| v == "hidden" || v == "veryHidden");
                // A sheet listed again (same name or same part) is damage:
                // Excel repairs the file keeping the first one.
                if let Some(target) = rels.get(rid.as_ref()).map(|t| resolve_target("xl", t)) {
                    if seen.insert(name.to_lowercase()) && seen.insert(target.clone()) {
                        sheets.push((name, target, hidden));
                    }
                }
            }
            _ => {}
        }
    }
    if sheets.is_empty() {
        return Err(Error::new("the workbook has no sheets"));
    }
    wb.externals = crate::xlsx_external::read(&z, &rels, &wbxml);
    for (name, _, hidden) in &sheets {
        let mut sh = Sheet::new(name.clone());
        sh.hidden = *hidden;
        wb.sheets.push(sh);
    }
    if wb.sheets.iter().all(|s| s.hidden) {
        wb.sheets[0].hidden = false;
    }
    let sheet_names: Vec<String> = sheets.iter().map(|s| s.0.clone()).collect();
    crate::xlsx_views::read_book(&wbxml, &mut wb, &sheet_names);
    crate::xlsx_chart::set_theme(Some(&wb.cell_styles.theme));
    let theme = wb.cell_styles.theme.clone();
    let cells = read_cells(&z, &sheets, &mut wb.sheets, &theme, &xfs, &strings, &pictures);
    let mut chart_points = crate::xlsx_drawing::BOOK_CHART_POINTS;
    for (i, ((_, path, _), (st, read))) in sheets.iter().zip(cells).enumerate() {
        if let Ok(xml) = read {
            st.finish(&mut wb, i);
            crate::xlsx_views::read_sheet(&xml, &mut wb.sheets[i], &dxfs);
            crate::scenario_io::read_xlsx(&xml, &mut wb, i);
            let xml = crate::xlsx_views::strip(&xml);
            crate::xlsx_page::read_sheet_page(&xml, &mut wb.sheets[i].page);
            wb.sheets[i].cell_watches = crate::xlsx_page::read_cell_watches(&xml);
            crate::xlsx_data::read_tools(&xml, &mut wb.sheets[i], &dxfs);
            crate::xlsx_drawing::read_drawings(&z, path, &xml, &mut wb.sheets[i], &mut chart_points);
            crate::xlsx_ole::read(&z, path, &xml, &mut wb.sheets[i]);
            crate::xlsx_controls::read(&z, path, &mut wb.sheets[i]);
            wb.sheets[i].page.background = crate::xlsx_page::read_background(&z, path, &xml);
            crate::xlsx_chart::read_sparklines(&xml, &mut wb.sheets[i]);
            // The book's tables so far.
            let had = wb.sheets.iter().flat_map(|s| &s.tables).fold((0, 0), |(n, c), t| (n + 1, c + t.columns.len()));
            crate::xlsx_table::read_tables(&z, path, &xml, &mut wb.sheets[i], &dxfs, had);
            for (c, note, link) in crate::xlsx_notes::read_sheet_extras(&z, path, &xml, &mut wb.sheets[i], &persons) {
                wb.sheets[i].edit_extra(c, |x| {
                    if note.is_some() {
                        x.note = note;
                    }
                    if link.is_some() {
                        x.link = link;
                    }
                });
            }
        }
    }
    for (name, local, text) in names {
        crate::xlsx_page::apply_defined_name(&mut wb, &name, local, &text);
    }
    crate::xlsx_chart::set_theme(None);
    crate::xlsx_chart::resolve_names(&mut wb, &chart_names);
    read_user_names(&mut wb, user_names);
    wb.refresh_cell_styles();
    wb.normalize_externals();
    if wb.calc.date1904 {
        wb.shift_dates(apex_sheet::workbook::DAYS_1904);
    }
    wb.meta = crate::props::read_ooxml(&z);
    wb.meta.read_only_recommended = read_only_rec;
    let paths: Vec<String> = sheets.into_iter().map(|s| s.1).collect();
    // Pivot tables ApexDesk recalculates aren't kept as they were.
    let pivots = crate::xlsx_pivot::read(&z, &wbxml, &paths, &mut wb);
    crate::xlsx_keep::read(&z, &wbxml, &paths, &pivots.handled, &mut wb);
    crate::xlsx_slicer::read(&z, &wbxml, &paths, &pivots.tables, &mut wb);
    crate::xlsx_model::read_xlsx(&wbxml, &mut wb);
    wb.macros = crate::macro_parts::read(&z, "xl", &z.read_string("xl/_rels/workbook.xml.rels").unwrap_or_default());
    wb.finish_load();
    Ok(wb)
}

pub(crate) fn read_user_names(wb: &mut Workbook, list: Vec<(String, Option<usize>, String, String, bool)>) {
    // Solver models are saved as names local to their sheet.
    for (name, local, text, comment, hidden) in crate::solver_names::read(wb, list) {
        let scope = match local {
            Some(i) => match wb.sheets.get(i) {
                Some(s) => Some(s.name.clone()),
                None => continue,
            },
            None => None,
        };
        // What the engine can't read (links to other files...) keeps its
        // text so it is saved back unchanged.
        // Several areas (`Sheet1!A1:A4,Sheet1!C1:C4`) are a union.
        let t = text.trim();
        let expr = parse_canonical(t)
            .or_else(|e| if t.contains(',') { parse_canonical(&format!("({t})")).map_err(|_| e) } else { Err(e) })
            .unwrap_or_else(|_| apex_sheet::Expr::Name(t.into()));
        let mut d = apex_sheet::DefinedName::new(&name, scope, expr);
        d.comment = comment;
        d.hidden = hidden;
        wb.names.push(d);
    }
}

/// The workbook's names but those of the kept slicer caches left out
/// (`dropped`, lowercase).
fn write_user_names(wb: &Workbook, dropped: &std::collections::HashSet<String>) -> String {
    let mut out = String::new();
    for d in &wb.names {
        if d.scope.is_none() && dropped.contains(&d.name.to_lowercase()) {
            continue;
        }
        let local = match &d.scope {
            Some(s) => match wb.sheet_index(s) {
                Some(i) => format!(r#" localSheetId="{i}""#),
                None => continue,
            },
            None => String::new(),
        };
        let comment = if d.comment.is_empty() { String::new() } else { format!(r#" comment="{}""#, esc(&d.comment)) };
        let hidden = if d.hidden { r#" hidden="1""# } else { "" };
        let _ = write!(out, r#"<definedName name="{}"{local}{comment}{hidden}>{}</definedName>"#, esc(&d.name), esc(&name_formula(&d.expr)));
    }
    out
}

/// A defined name's formula; a union of areas without its parentheses,
/// as Excel writes it.
fn name_formula(e: &apex_sheet::Expr) -> String {
    match e {
        apex_sheet::Expr::Paren(inner) if matches!(**inner, apex_sheet::Expr::Binary(apex_sheet::formula::BinOp::Union, ..)) => inner.to_xlsx(),
        e => e.to_xlsx(),
    }
}

fn outline_level(t: &crate::xml::Tag<'_>) -> u8 {
    t.attr("outlineLevel").and_then(|v| v.parse::<u8>().ok()).unwrap_or(0).min(apex_sheet::MAX_LEVEL)
}

/// `pictures`: the picture of each value metadata record (`vm`).
#[cfg(test)]
fn read_sheet(xml: &str, wb: &mut Workbook, si: usize, xfs: &[u32], strings: &Strings, pictures: &[Option<Arc<apex_sheet::CellPicture>>]) {
    let mut st = SheetState::default();
    let theme = wb.cell_styles.theme.clone();
    read_sheet_piece(xml, &mut st, &mut wb.sheets[si], &theme, xfs, strings, pictures);
    st.finish(wb, si);
}

/// Reads the cells of each sheet in `paths` into `sheets`, several sheets
/// at once (each its own part and its own cells). Chart sheets and the like
/// have no worksheet part. Returns each sheet's state and its part without
/// the cells, for the readers of the rest of it.
fn read_cells(z: &ZipReader, paths: &[(String, String, bool)], sheets: &mut [Sheet], theme: &apex_sheet::Theme, xfs: &[u32], strings: &Strings, pictures: &[Option<Arc<apex_sheet::CellPicture>>]) -> Vec<(SheetState, Result<String, Error>)> {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let threads = cores.min(4).min(sheets.len());
    // Each sheet's pieces of cells are read apart too, by a few helpers.
    let helpers = (cores / threads).min(8);
    let read_one = |path: &str, sh: &mut Sheet| read_sheet_cells(z, path, sh, helpers, theme, xfs, strings, pictures);
    if threads <= 1 {
        return paths.iter().zip(sheets).map(|(p, sh)| read_one(&p.1, sh)).collect();
    }
    let work = std::sync::Mutex::new(paths.iter().zip(sheets.iter_mut()).enumerate());
    let mut out: Vec<Option<(SheetState, Result<String, Error>)>> = (0..paths.len()).map(|_| None).collect();
    std::thread::scope(|s| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let next = work.lock().unwrap_or_else(|e| e.into_inner()).next();
                        let Some((i, (p, sh))) = next else { return done };
                        done.push((i, read_one(&p.1, sh)));
                    }
                })
            })
            .collect();
        for w in workers {
            // A panic goes on to the caller (opening catches it).
            for (i, r) in w.join().unwrap_or_else(|e| std::panic::resume_unwind(e)) {
                out[i] = Some(r);
            }
        }
    });
    out.into_iter().map(|r| r.expect("every sheet read")).collect()
}

/// A piece of cells read apart, into a sheet of its own; or, when it
/// needs the pieces before it, the piece back to read in order.
type Apart = Result<(Sheet, SheetState), String>;

/// Reads the worksheet `path` into `sh`, its pieces of cells by `helpers`
/// threads at once, put together in order as they come back.
#[allow(clippy::too_many_arguments)]
fn read_sheet_cells(z: &ZipReader, path: &str, sh: &mut Sheet, helpers: usize, theme: &apex_sheet::Theme, xfs: &[u32], strings: &Strings, pictures: &[Option<Arc<apex_sheet::CellPicture>>]) -> (SheetState, Result<String, Error>) {
    let mut st = SheetState::default();
    if helpers <= 1 {
        // The cells are read as the part inflates; the other readers get
        // the part without them.
        let read = crate::xlsx_pieces::read(z, path, |piece, cells| {
            // Custom views are never among the cells.
            let piece = if cells { std::borrow::Cow::Borrowed(piece.as_str()) } else { crate::xlsx_views::strip(&piece) };
            read_sheet_piece(&piece, &mut st, sh, theme, xfs, strings, pictures);
        });
        return (st, read);
    }
    use std::sync::mpsc;
    let (jobs, inbox) = mpsc::sync_channel::<(usize, String, Option<f32>)>(helpers);
    let inbox = std::sync::Mutex::new(inbox);
    let (back, results) = mpsc::channel::<(usize, std::thread::Result<Apart>)>();
    std::thread::scope(|s| {
        for _ in 0..helpers {
            let (inbox, back) = (&inbox, back.clone());
            s.spawn(move || {
                loop {
                    let job = inbox.lock().unwrap_or_else(|e| e.into_inner()).recv();
                    let Ok((n, xml, row_height)) = job else { return };
                    // A panic is handed on, so the reader doesn't wait for it.
                    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let mut st = SheetState { alone: true, default_row_height: row_height, ..Default::default() };
                        let mut part = Sheet::new(String::new());
                        read_sheet_piece(&xml, &mut st, &mut part, theme, xfs, strings, pictures);
                        if st.failed { Err(xml) } else { Ok((part, st)) }
                    }));
                    if back.send((n, out)).is_err() {
                        return;
                    }
                }
            });
        }
        drop(back);
        let mut order = InOrder { next: 0, sent: 0, ready: std::collections::BTreeMap::new(), results };
        let read = crate::xlsx_pieces::read(z, path, |piece, cells| {
            if cells {
                // Pieces waiting stay few (each is MiBs of text).
                while order.sent - order.next >= 2 * helpers {
                    order.take(true).map(|p| join_piece(p, &mut st, sh, theme, xfs, strings, pictures));
                }
                let _ = jobs.send((order.sent, piece, st.default_row_height));
                order.sent += 1;
                while let Some(p) = order.take(false) {
                    join_piece(p, &mut st, sh, theme, xfs, strings, pictures);
                }
            } else {
                while let Some(p) = order.take(true) {
                    join_piece(p, &mut st, sh, theme, xfs, strings, pictures);
                }
                read_sheet_piece(&crate::xlsx_views::strip(&piece), &mut st, sh, theme, xfs, strings, pictures);
            }
        });
        drop(jobs);
        while let Some(p) = order.take(true) {
            join_piece(p, &mut st, sh, theme, xfs, strings, pictures);
        }
        (st, read)
    })
}

/// The pieces read apart, handed out in the order they were sent.
struct InOrder {
    next: usize,
    sent: usize,
    ready: std::collections::BTreeMap<usize, Apart>,
    results: std::sync::mpsc::Receiver<(usize, std::thread::Result<Apart>)>,
}

impl InOrder {
    /// The next piece: waiting for it when `wait`, else only when back.
    fn take(&mut self, wait: bool) -> Option<Apart> {
        if self.next == self.sent {
            return None;
        }
        loop {
            if let Some(p) = self.ready.remove(&self.next) {
                self.next += 1;
                return Some(p);
            }
            let got = if wait { self.results.recv().ok() } else { self.results.try_recv().ok() };
            let (n, out) = got?;
            self.ready.insert(n, out.unwrap_or_else(|e| std::panic::resume_unwind(e)));
        }
    }
}

/// Puts a piece read apart into the sheet after the pieces before it.
fn join_piece(p: Apart, st: &mut SheetState, sh: &mut Sheet, theme: &apex_sheet::Theme, xfs: &[u32], strings: &Strings, pictures: &[Option<Arc<apex_sheet::CellPicture>>]) {
    let (mut part, ps) = match p {
        Ok(p) => p,
        Err(xml) => return read_sheet_piece(&xml, st, sh, theme, xfs, strings, pictures),
    };
    sh.append_cells(&mut part);
    for (r, h) in std::mem::take(part.row_heights.map_mut()) {
        sh.row_heights.insert(r, h);
    }
    sh.row_levels.append(&mut part.row_levels);
    // Read apart with addresses: where the piece ended is known.
    (st.row, st.next_col) = (ps.row, ps.next_col);
    st.shared.extend(ps.shared);
    st.arrays.extend(ps.arrays);
    for (cell, si, value, style) in ps.pending {
        if let Some((origin, e, cost)) = st.shared.get(&si)
            && let Some(spent) = st.shared_spent.checked_add(*cost).filter(|s| *s <= MAX_SHARED_COPIES)
        {
            st.shared_spent = spent;
            let mut e2 = (**e).clone();
            e2.offset(cell.row as i64 - origin.row as i64, cell.col as i64 - origin.col as i64);
            sh.load_cell(cell, Content::Formula { expr: Arc::new(e2).into(), value }, style);
        }
    }
}

/// Bytes of long shared formulas (past 256) copied into a sheet's cells:
/// a file of many cells sharing a huge formula would make a tree each.
const MAX_SHARED_COPIES: usize = 32 << 20;

/// What marking array formulas' areas may go over in a sheet: many areas
/// over the same cells stop here.
pub(crate) const MAX_ARRAY_WORK: usize = 1 << 25;

/// What reading a worksheet carries from one piece of its XML to the next.
#[derive(Default)]
struct SheetState {
    row: u32,
    next_col: u32,
    // shared formula index -> (origin, expr, what a copy costs)
    shared: HashMap<u32, (CellRef, Arc<apex_sheet::Expr>, usize)>,
    /// Spent on copies of long shared formulas (each cell gets a tree).
    shared_spent: usize,
    // Array formula areas: their other cells hold the spilled values.
    arrays: Vec<(Range, bool)>,
    default_row_height: Option<f32>,
    /// Cells of Google Sheets formulas: whether they are calculated here
    /// (their spilled cells then only keep the last values).
    google: HashMap<CellRef, bool>,
    /// The sheet view's top left cell (where frozen panes start).
    view_top: CellRef,
    /// Texts of the cells, kept once each.
    texts: apex_sheet::Interner,
    /// Reading a piece of cells apart from the pieces before it.
    alone: bool,
    /// The piece read apart needs the pieces before it after all (cells
    /// without their address, Google formulas): it is read again in order.
    failed: bool,
    /// Cells read apart that take a shared formula from a piece before:
    /// (cell, shared index, value, format).
    pending: Vec<(CellRef, u32, Value, u32)>,
}

impl SheetState {
    fn finish(self, wb: &mut Workbook, si: usize) {
        let mut work = 0;
        for (area, fixed) in self.arrays {
            if work > MAX_ARRAY_WORK {
                break;
            }
            work += if fixed { wb.sheets[si].mark_fixed_array(&area) } else { wb.sheets[si].mark_spilled(&area) };
        }
    }
}

/// Reads a piece of a worksheet that starts and ends between elements.
fn read_sheet_piece(xml: &str, st: &mut SheetState, sh: &mut Sheet, theme: &apex_sheet::Theme, xfs: &[u32], (strings, rich): &Strings, pictures: &[Option<Arc<apex_sheet::CellPicture>>]) {
    let mut r = Reader::new(xml);
    let SheetState { mut row, mut next_col, mut shared, mut shared_spent, mut arrays, mut default_row_height, mut google, view_top, mut texts, alone, mut failed, mut pending } = std::mem::take(st);
    let mut col_left: u32 = 1 << 16;
    st.view_top = view_top;
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "sheetPr" => {
                sh.code_name = t.attr("codeName").map(|v| v.to_string()).filter(|v| !v.is_empty());
                sh.lotus_eval = t.attr("transitionEvaluation").is_some_and(|v| v == "1" || v == "true");
            }
            Ev::Start(t) | Ev::Empty(t) if t.local() == "tabColor" => {
                sh.tab_color = t.attr("rgb").and_then(|v| parse_hex_color(&v));
            }
            Ev::Start(t) | Ev::Empty(t) if t.local() == "outlinePr" => {
                let off = |k: &str| t.attr(k).is_some_and(|v| v == "0" || v == "false");
                sh.summary_below = !off("summaryBelow");
                sh.summary_right = !off("summaryRight");
            }
            Ev::Start(t) | Ev::Empty(t) if t.local() == "sheetFormatPr" => {
                if let Some(h) = t.attr("defaultRowHeight").and_then(|v| v.parse::<f32>().ok()) {
                    default_row_height = Some(h);
                    sh.default_row_height = default_size((h * ROW_SCALE).round().max(8.0), apex_sheet::sheet::DEFAULT_ROW_HEIGHT);
                }
                if let Some(w) = t.attr("defaultColWidth").and_then(|v| v.parse::<f32>().ok()) {
                    sh.default_col_width = default_size(width_from_chars(w), apex_sheet::sheet::DEFAULT_COL_WIDTH);
                } else if let Some(w) = t.attr("baseColWidth").and_then(|v| v.parse::<f32>().ok()) {
                    sh.default_col_width = default_size(width_from_chars(w + 0.71), apex_sheet::sheet::DEFAULT_COL_WIDTH);
                }
            }
            Ev::Start(t) | Ev::Empty(t) if t.local() == "col" => {
                let min: u32 = t.attr("min").and_then(|v| v.parse().ok()).unwrap_or(1);
                let max: u32 = t.attr("max").and_then(|v| v.parse().ok()).unwrap_or(min);
                let hidden = t.attr("hidden").is_some_and(|v| v == "1" || v == "true");
                let w = t.attr("width").and_then(|v| v.parse::<f32>().ok());
                let level = outline_level(&t);
                // Columns set over and over cost each time: a sheet has
                // 16384, set a few times each at most.
                if min >= 1 && max >= min && max - min < 2000 && max - min < col_left {
                    col_left -= max - min + 1;
                    for c in min..=max {
                        if level > 0 {
                            sh.col_levels.insert(c - 1, level);
                        }
                        if hidden {
                            sh.col_widths.insert(c - 1, 0.0);
                        } else if let Some(w) = w {
                            let px = width_from_chars(w);
                            if (px - sh.default_col_width).abs() > 1.0 {
                                sh.col_widths.insert(c - 1, px);
                            }
                        }
                    }
                }
            }
            Ev::Start(t) | Ev::Empty(t) if t.local() == "sheetView" => {
                sh.view = crate::xlsx_page::view_mode(t.attr("view").as_deref());
                st.view_top = t.attr("topLeftCell").and_then(|v| CellRef::parse(&v)).unwrap_or_default();
            }
            Ev::Start(t) | Ev::Empty(t) if t.local() == "pane" => {
                if t.attr("state").is_some_and(|s| s == "frozen" || s == "frozenSplit") {
                    // The splits count the rows and columns shown frozen,
                    // from the view's top left cell on.
                    let n = |a: &str| t.attr(a).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0) as u32;
                    let (x, y) = (n("xSplit"), n("ySplit"));
                    let top = st.view_top;
                    sh.frozen_cols = if x > 0 { top.col.saturating_add(x) } else { 0 };
                    sh.frozen_rows = if y > 0 { top.row.saturating_add(y) } else { 0 };
                    sh.frozen_top = CellRef::new(if y > 0 { top.row } else { 0 }, if x > 0 { top.col } else { 0 });
                }
            }
            Ev::Start(t) | Ev::Empty(t) if t.local() == "mergeCell" => {
                if let Some(rg) = t.attr("ref").and_then(|v| Range::parse(&v)) {
                    sh.merges.push(rg);
                }
            }
            Ev::Start(t) | Ev::Empty(t) if t.local() == "row" => {
                if let Some(n) = t.attr("r").and_then(|v| v.parse::<u32>().ok()) {
                    row = n.saturating_sub(1);
                } else {
                    // Apart, where the row is isn't known.
                    failed |= alone;
                }
                next_col = 0;
                let level = outline_level(&t);
                if level > 0 {
                    sh.row_levels.insert(row, level);
                }
                let hidden = t.attr("hidden").is_some_and(|v| v == "1" || v == "true");
                if hidden {
                    sh.row_heights.insert(row, 0.0);
                } else if t.attr("customHeight").is_some_and(|v| v == "1" || v == "true") {
                    if let Some(h) = t.attr("ht").and_then(|v| v.parse::<f32>().ok()) {
                        if Some(h) != default_row_height {
                            sh.row_heights.insert(row, (h * ROW_SCALE).round());
                        }
                    }
                }
                if matches!(r_last_empty(&t), true) {
                    row += 1;
                }
            }
            Ev::End(n) if crate::xml::local(n) == "row" => row += 1,
            Ev::Start(t) if t.local() == "c" => {
                let [r_at, s_at, t_at, vm_at, cm_at] = t.attrs_of(["r", "s", "t", "vm", "cm"]);
                let at = r_at.and_then(|v| CellRef::parse(&v));
                failed |= alone && at.is_none();
                let cell = at.unwrap_or(CellRef::new(row, next_col));
                next_col = cell.col + 1;
                let style = s_at.and_then(|v| v.parse::<usize>().ok()).and_then(|i| xfs.get(i).copied()).unwrap_or(0);
                let ty = t_at.unwrap_or_default();
                let vm = vm_at.and_then(|v| v.trim().parse::<usize>().ok());
                // `cm` marks dynamic arrays; other array formulas keep their size.
                let dynamic = cm_at.is_some();
                let mut v_text: Option<std::borrow::Cow<str>> = None;
                let mut f_text: Option<String> = None;
                let mut f_shared: Option<(u32, bool)> = None;
                let mut f_array = false;
                let mut is_text: Option<std::borrow::Cow<str>> = None;
                let mut runs: Option<Arc<apex_sheet::rich::RichText>> = None;
                // Parse children of <c>.
                loop {
                    match r.next() {
                        Ev::Start(ct) => match ct.local() {
                            "v" => v_text = Some(r.text_cow()),
                            "f" => {
                                if ct.attr("t").as_deref() == Some("array") && !f_array {
                                    f_array = true;
                                    if let Some(area) = ct.attr("ref").and_then(|v| Range::parse(&v)) {
                                        arrays.push((area, !dynamic));
                                    }
                                }
                                let shared_si = if ct.attr("t").as_deref() == Some("shared") { ct.attr("si").and_then(|v| v.parse().ok()) } else { None };
                                let txt = r.text();
                                if let Some(s) = shared_si {
                                    f_shared = Some((s, !txt.is_empty()));
                                }
                                if !txt.is_empty() {
                                    f_text = Some(txt);
                                }
                            }
                            "is" => {
                                if let Some(text) = crate::xlsx_rich::plain_rst(&mut r) {
                                    is_text = Some(text);
                                } else {
                                    let (text, r2) = crate::xlsx_rich::read_rst(&mut r, theme);
                                    runs = r2.map(Arc::new);
                                    is_text = Some(text.into());
                                }
                            }
                            _ => r.skip(),
                        },
                        Ev::Empty(ct) if ct.local() == "f" => {
                            if ct.attr("t").as_deref() == Some("shared") {
                                if let Some(s) = ct.attr("si").and_then(|v| v.parse().ok()) {
                                    f_shared = Some((s, false));
                                }
                            }
                        }
                        Ev::End(_) | Ev::Eof => break,
                        _ => {}
                    }
                }
                let value = match &*ty {
                    "s" => {
                        let i = v_text.as_deref().and_then(|v| v.trim().parse::<usize>().ok());
                        runs = i.and_then(|i| rich.get(i).cloned().flatten());
                        i.and_then(|i| strings.get(i)).map(|s| Value::Text(s.clone())).unwrap_or(Value::Empty)
                    }
                    "inlineStr" => is_text.map(|t| texts.text(&t)).unwrap_or(Value::Empty),
                    "str" => v_text.map(|t| texts.text(&t)).unwrap_or(Value::Empty),
                    "b" => Value::Bool(v_text.as_deref().is_some_and(|v| v.trim() == "1" || v.trim() == "true")),
                    "e" => v_text.as_deref().and_then(ErrorKind::parse).map(Value::Error).unwrap_or(Value::Error(ErrorKind::Na)),
                    "d" => v_text.as_deref().and_then(parse_iso_datetime).map(Value::Number).unwrap_or(Value::Empty),
                    _ => v_text.as_deref().and_then(|v| v.trim().parse::<f64>().ok()).map(Value::Number).unwrap_or(Value::Empty),
                };
                let mut expr = None;
                let mut spilled = false;
                if let Some(ft) = &f_text {
                    if let Ok(e) = parse_canonical(ft) {
                        use apex_sheet::functions::google::{FromGoogle, from_google};
                        let google_kind = from_google(&e, &value);
                        // Google's spills follow the cells before them.
                        failed |= alone && google_kind.is_some();
                        let e = match google_kind {
                            None if f_array => e,
                            // Plain formulas predate dynamic arrays.
                            None => e.legacy(),
                            Some(FromGoogle::Formula(x)) => {
                                google.insert(cell, true);
                                x
                            }
                            Some(FromGoogle::Fixed(x)) => {
                                google.insert(cell, false);
                                x
                            }
                            Some(FromGoogle::Spilled) => {
                                let left = cell.col.checked_sub(1).and_then(|c| google.get(&CellRef::new(cell.row, c)));
                                let up = cell.row.checked_sub(1).and_then(|r| google.get(&CellRef::new(r, cell.col)));
                                let calculated = left.or(up).copied().unwrap_or(false);
                                google.insert(cell, calculated);
                                spilled = calculated;
                                e
                            }
                        };
                        let e = Arc::new(e);
                        if let Some((s, true)) = f_shared {
                            shared.insert(s, (cell, e.clone(), ft.len().saturating_sub(256)));
                        }
                        expr = Some(e);
                    }
                } else if let Some((s, _)) = f_shared {
                    if let Some((origin, e, cost)) = shared.get(&s) {
                        // Past the budget the cells keep their values.
                        if let Some(spent) = shared_spent.checked_add(*cost).filter(|s| *s <= MAX_SHARED_COPIES) {
                            shared_spent = spent;
                            let mut e2 = (**e).clone();
                            e2.offset(cell.row as i64 - origin.row as i64, cell.col as i64 - origin.col as i64);
                            expr = Some(Arc::new(e2));
                        }
                    } else if alone {
                        // Its first cell is in a piece before.
                        pending.push((cell, s, value.clone(), style));
                    }
                }
                let content = match expr {
                    // The formula of Google Sheets spills here again.
                    Some(_) if spilled => Content::Spill(value),
                    Some(expr) => Content::Formula { expr: expr.into(), value },
                    None => match value {
                        Value::Empty => Content::Empty,
                        v => Content::Value(v),
                    },
                };
                let plain = !matches!(content, Content::Value(Value::Text(_)));
                if !matches!(content, Content::Empty) || style != 0 {
                    sh.load_cell(cell, content, style);
                }
                if let (Some(runs), false) = (runs, plain) {
                    sh.edit_extra(cell, |x| x.rich = Some(runs));
                }
                // A picture in the cell (formulas like IMAGE are computed again).
                if let Some(pic) = vm.filter(|_| f_text.is_none() && f_shared.is_none()).and_then(|v| pictures.get(v.checked_sub(1)?)?.clone()) {
                    crate::xlsx_incell::apply_picture(sh, cell, pic);
                }
            }
            Ev::Empty(t) if t.local() == "c" => {
                let [r_at, s_at] = t.attrs_of(["r", "s"]);
                let at = r_at.and_then(|v| CellRef::parse(&v));
                failed |= alone && at.is_none();
                let cell = at.unwrap_or(CellRef::new(row, next_col));
                next_col = cell.col + 1;
                let style = s_at.and_then(|v| v.parse::<usize>().ok()).and_then(|i| xfs.get(i).copied()).unwrap_or(0);
                if style != 0 {
                    sh.load_cell(cell, Content::Empty, style);
                }
            }
            _ => {}
        }
    }
    *st = SheetState { row, next_col, shared, shared_spent, arrays, default_row_height, google, view_top: st.view_top, texts, alone, failed, pending };
}

// `<row .../>` never gets an `End` event; detect via a sentinel we can't
// observe from `Tag`, so rows are always advanced by `r` attributes instead.
fn r_last_empty(_t: &Tag) -> bool {
    false
}

pub(crate) fn parse_iso_datetime(s: &str) -> Option<f64> {
    let s = s.trim();
    let (date, time) = s.split_once('T').unwrap_or((s, ""));
    let mut it = date.split('-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let d: i64 = it.next()?.parse().ok()?;
    let mut serial = date_to_serial(y, m, d);
    if !time.is_empty() {
        let time = time.trim_end_matches('Z');
        let mut it = time.split(':');
        let h: f64 = it.next()?.parse().ok()?;
        let mi: f64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0.0);
        let se: f64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0.0);
        serial += (h * 3600.0 + mi * 60.0 + se) / 86400.0;
    }
    Some(serial)
}

// ---------------------------------------------------------------------------
// Writing

fn color_attr(c: u32) -> String {
    format!("FF{:06X}", c & 0xFF_FFFF)
}

/// Writes an xlsx package (an xlsm one if the workbook has a VBA project).
pub fn write(wb: &Workbook) -> Vec<u8> {
    write_as(wb, crate::Flavor::for_macros(&wb.macros))
}

/// Writes the package as xlsx, xlsm, xltx or xltm.
pub fn write_as(wb: &Workbook, flavor: crate::Flavor) -> Vec<u8> {
    if wb.calc.date1904 {
        let mut w = wb.clone();
        w.shift_dates(-apex_sheet::workbook::DAYS_1904);
        return write_book(&w, flavor);
    }
    write_book(wb, flavor)
}

fn write_book(wb: &Workbook, flavor: crate::Flavor) -> Vec<u8> {
    let mut z = ZipWriter::new();
    let n = wb.sheets.len();

    // Shared strings.
    let mut sst: Vec<Arc<str>> = Vec::new();
    let mut sst_idx: HashMap<Arc<str>, usize> = HashMap::new();
    // Cells often share one string allocation: look it up by address first
    // so long strings are not hashed once per cell.
    let mut by_ptr: HashMap<(usize, usize), usize> = HashMap::new();
    let mut sst_count = 0usize;
    let mut sheet_xml = Vec::with_capacity(n);
    let mut media = crate::xlsx_drawing::Media::default();
    let pictures = crate::xlsx_incell::CellPictures::collect(wb, &mut media);
    let dxfs = crate::xlsx_data::collect_dxfs(wb);
    let mut persons = Vec::new();
    let mut parts: Vec<_> = wb.sheets.iter().enumerate().map(|(i, sh)| crate::xlsx_notes::sheet_parts(sh, i + 1, &mut persons)).collect();
    for (i, sh) in wb.sheets.iter().enumerate() {
        crate::xlsx_page::background_rel(sh, &mut parts[i].rels, &mut media);
        crate::xlsx_ole::sheet(sh, i + 1, &mut parts[i], &mut media);
        crate::xlsx_controls::sheet(sh, i + 1, &mut parts[i]);
    }
    for (si, sh) in wb.sheets.iter().enumerate() {
        sheet_xml.push(write_sheet(wb, si, sh, &dxfs, &parts[si], &pictures.vm, &mut |s: &Arc<str>| {
            sst_count += 1;
            let key = (s.as_ptr() as usize, s.len());
            if let Some(&i) = by_ptr.get(&key) {
                return i;
            }
            let i = *sst_idx.entry(s.clone()).or_insert_with(|| {
                sst.push(s.clone());
                sst.len() - 1
            });
            by_ptr.insert(key, i);
            i
        }));
    }

    // Connections, queries, pivot tables and custom XML kept from the file.
    let keep = crate::xlsx_keep::write(wb, &mut z);
    let pivots = crate::xlsx_pivot::write(wb, &mut z, &keep.pivot_caches);
    let slicers = crate::xlsx_slicer::write(wb, &mut z, &pivots.natives);
    let tables = crate::xlsx_table::write_tables(wb, &dxfs, &keep.queries);
    for (i, t) in tables.iter().enumerate() {
        crate::xlsx_table::link_tables(&mut sheet_xml[i], &mut parts[i].rels, t);
        crate::xlsx_table::add_rels(&mut parts[i].rels, &keep.sheet_rels[i]);
        crate::xlsx_table::add_rels(&mut parts[i].rels, &pivots.sheet_rels[i]);
        crate::xlsx_table::add_rels(&mut parts[i].rels, &slicers.sheet_rels[i]);
    }

    let mut ct = String::from(HEADER);
    ct.push_str(r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/><Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/><Override PartName="/xl/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/><Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>"#);
    ct.push_str(crate::props::content_types(&wb.meta));
    let _ = write!(ct, r#"<Override PartName="/xl/workbook.xml" ContentType="{}"/>"#, flavor.workbook_type());
    let kept = crate::macro_parts::write(&mut z, &wb.macros, "xl", flavor);
    ct.push_str(&kept.types);
    for i in 1..=n {
        let _ = write!(ct, r#"<Override PartName="/xl/worksheets/sheet{}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#, i);
    }
    if parts.iter().any(|p| p.vml.is_some()) {
        ct.push_str(r#"<Default Extension="vml" ContentType="application/vnd.openxmlformats-officedocument.vmlDrawing"/>"#);
    }
    for (i, p) in parts.iter().enumerate() {
        if p.comments.is_some() {
            let _ = write!(ct, r#"<Override PartName="/xl/comments{}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.comments+xml"/>"#, i + 1);
        }
        if p.threads.is_some() {
            let _ = write!(ct, r#"<Override PartName="/xl/threadedComments/threadedComment{}.xml" ContentType="{}"/>"#, i + 1, crate::xlsx_threaded::CT_THREADED);
        }
    }
    if !persons.is_empty() {
        let _ = write!(ct, r#"<Override PartName="/xl/persons/person.xml" ContentType="{}"/>"#, crate::xlsx_threaded::CT_PERSON);
    }
    let mut drawings: Vec<Option<(String, String)>> = (0..wb.sheets.len()).map(|i| crate::xlsx_drawing::write_drawing(wb, i, &mut media)).collect();
    for (i, d) in drawings.iter_mut().enumerate() {
        let had = d.is_some();
        let anchors = keep.slicers.anchors[i].clone() + &slicers.anchors[i];
        crate::xlsx_slicer::add_anchors(d, &anchors);
        let ext = crate::xlsx_slicer::join_ext(&keep.slicers.sheet_ext[i], &slicers.sheet_ext[i], "x14:slicerList");
        crate::xlsx_slicer::patch_sheet(&mut sheet_xml[i], &mut parts[i].rels, i + 1, had, !anchors.is_empty(), &ext);
    }
    for (i, d) in drawings.iter().enumerate() {
        if d.is_some() {
            let _ = write!(ct, r#"<Override PartName="/xl/drawings/drawing{}.xml" ContentType="application/vnd.openxmlformats-officedocument.drawing+xml"/>"#, i + 1);
        }
    }
    for (path, _, ty) in &media.parts {
        let _ = write!(ct, r#"<Override PartName="/{path}" ContentType="{ty}"/>"#);
    }
    ct.push_str(&media.objects.content_types());
    for (ext, mime) in &media.types {
        let _ = write!(ct, r#"<Default Extension="{ext}" ContentType="{mime}"/>"#);
    }
    let arrays = wb.has_array_formulas();
    if arrays || !pictures.is_empty() {
        ct.push_str(r#"<Override PartName="/xl/metadata.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheetMetadata+xml"/>"#);
    }
    let checkboxes = crate::xlsx_incell::has_checkboxes(wb);
    if checkboxes {
        let _ = write!(ct, r#"<Override PartName="/xl/featurePropertyBag/featurePropertyBag.xml" ContentType="{}"/>"#, crate::xlsx_incell::CT_FPB);
    }
    let mut rich_rels = String::new();
    pictures.write(&mut z, &mut ct, &mut rich_rels);
    for (k, _) in tables.iter().flatten() {
        let _ = write!(ct, r#"<Override PartName="/xl/tables/table{k}.xml" ContentType="{}"/>"#, crate::xlsx_table::CT_TABLE);
    }
    let (ext_refs, ext_rels, ext_ct) = crate::xlsx_external::write(wb, &mut z, n + 5);
    ct.push_str(&ext_ct);
    ct.push_str(&keep.content_types);
    ct.push_str(&pivots.content_types);
    ct.push_str(&slicers.content_types);
    ct.push_str("</Types>");
    z.add("[Content_Types].xml", ct.as_bytes(), true);

    z.add("_rels/.rels", format!(r#"{HEADER}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" Target="docProps/app.xml"/>{}{}{}</Relationships>"#, crate::props::rels(&wb.meta), keep.package_rels, kept.root_rels).as_bytes(), true);
    crate::props::write_ooxml(&mut z, &wb.meta);

    let mut w = String::from(HEADER);
    w.push_str(r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">"#);
    if wb.meta.read_only_recommended {
        w.push_str(r#"<fileSharing readOnlyRecommended="1"/>"#);
    }
    if wb.calc.date1904 {
        w.push_str(r#"<workbookPr date1904="1"/>"#);
    }
    crate::xlsx_notes::write_workbook_protection(&mut w, wb);
    w.push_str("<bookViews><workbookView/></bookViews><sheets>");
    for (i, sh) in wb.sheets.iter().enumerate() {
        let _ = write!(w, r#"<sheet name="{}" sheetId="{}"{} r:id="rId{}"/>"#, esc(&sh.name), i + 1, if sh.hidden { r#" state="hidden""# } else { "" }, i + 1);
    }
    w.push_str("</sheets>");
    w.push_str(&ext_refs);
    let mut names = crate::xlsx_data::filter_names(wb) + &crate::xlsx_page::print_names(wb) + &write_user_names(wb, &keep.slicers.dropped) + &crate::solver_names::xlsx_names(wb) + &slicers.names;
    for (i, f) in media.chart_names.iter().enumerate() {
        let _ = write!(names, r#"<definedName name="_xlchart.v1.{i}" hidden="1">{}</definedName>"#, esc(f));
    }
    if !names.is_empty() {
        let _ = write!(w, "<definedNames>{names}</definedNames>");
    }
    let c = &wb.calc;
    let mode = if c.manual { r#" calcMode="manual""# } else { r#" fullCalcOnLoad="1""# };
    let iter = if c.iterate { format!(r#" iterate="1" iterateCount="{}" iterateDelta="{}""#, c.max_iter, c.max_change) } else { String::new() };
    let refs = if c.r1c1 { r#" refMode="R1C1""# } else { "" };
    let shown = if c.as_shown { r#" fullPrecision="0""# } else { "" };
    let _ = write!(w, r#"<calcPr calcId="191029"{mode}{iter}{refs}{shown}/>"#);
    crate::xlsx_views::write_book(&mut w, wb);
    w.push_str(&crate::xlsx_pivot::merge_caches(&keep.pivot_caches, &pivots.caches));
    w.push_str(&crate::xlsx_model::ext_list(&crate::xlsx_slicer::merge_ext(&keep.ext, &crate::xlsx_slicer::join_ext(&keep.slicers.book_ext, &slicers.book_ext, "x14:slicerCaches")), wb));
    w.push_str("</workbook>");
    z.add("xl/workbook.xml", w.as_bytes(), true);

    let mut rels = String::from(HEADER);
    rels.push_str(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#);
    for i in 1..=n {
        let _ = write!(rels, r#"<Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{}.xml"/>"#, i, i);
    }
    let _ = write!(rels, r#"<Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>"#, n + 1);
    let _ = write!(rels, r#"<Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/>"#, n + 2);
    let _ = write!(rels, r#"<Relationship Id="rId{}" Type="{}" Target="theme/theme1.xml"/>"#, n + 4, crate::theme_xml::REL_TYPE);
    if arrays || !pictures.is_empty() {
        let _ = write!(rels, r#"<Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sheetMetadata" Target="metadata.xml"/>"#, n + 3);
        // `cm="1"` marks formulas as dynamic arrays (no {} braces in Excel),
        // `vm="n"` points at pictures in cells.
        z.add("xl/metadata.xml", crate::xlsx_incell::metadata_xml(arrays, pictures.vm.len()).as_bytes(), true);
    }
    rels.push_str(&rich_rels);
    if checkboxes {
        let _ = write!(rels, r#"<Relationship Id="rIdFPB" Type="{}" Target="featurePropertyBag/featurePropertyBag.xml"/>"#, crate::xlsx_incell::REL_FPB);
        z.add("xl/featurePropertyBag/featurePropertyBag.xml", crate::xlsx_incell::feature_bags_xml().as_bytes(), true);
    }
    rels.push_str(&ext_rels);
    rels.push_str(&keep.rels);
    rels.push_str(&pivots.book_rels);
    rels.push_str(&slicers.book_rels);
    rels.push_str(&kept.main_rels);
    if !persons.is_empty() {
        let _ = write!(rels, r#"<Relationship Id="rIdP1" Type="{}" Target="persons/person.xml"/>"#, crate::xlsx_threaded::REL_PERSON);
        z.add("xl/persons/person.xml", crate::xlsx_threaded::write_persons(&persons).as_bytes(), true);
    }
    rels.push_str("</Relationships>");
    z.add("xl/_rels/workbook.xml.rels", rels.as_bytes(), true);

    z.add("xl/styles.xml", crate::xlsx_styles::write_styles(wb, &dxfs).as_bytes(), true);
    z.add("xl/theme/theme1.xml", crate::theme_xml::write_theme(&wb.cell_styles.theme).as_bytes(), true);

    let mut s = String::with_capacity(sst.iter().map(|x| x.len() + 20).sum::<usize>() + 200);
    s.push_str(HEADER);
    let _ = write!(s, r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="{}" uniqueCount="{}">"#, sst_count, sst.len());
    for t in &sst {
        let space = if t.starts_with(char::is_whitespace) || t.ends_with(char::is_whitespace) { r#" xml:space="preserve""# } else { "" };
        let _ = write!(s, "<si><t{}>{}</t></si>", space, esc(t));
    }
    s.push_str("</sst>");
    z.add("xl/sharedStrings.xml", s.as_bytes(), true);

    for (i, x) in sheet_xml.iter().enumerate() {
        z.add(&format!("xl/worksheets/sheet{}.xml", i + 1), x.as_bytes(), true);
        let p = &parts[i];
        if let Some(r) = &p.rels {
            z.add(&format!("xl/worksheets/_rels/sheet{}.xml.rels", i + 1), r.as_bytes(), true);
        }
        if let Some(c) = &p.comments {
            z.add(&format!("xl/comments{}.xml", i + 1), c.as_bytes(), true);
        }
        if let Some(t) = &p.threads {
            z.add(&format!("xl/threadedComments/threadedComment{}.xml", i + 1), t.as_bytes(), true);
        }
        if let Some(v) = &p.vml {
            z.add(&format!("xl/drawings/vmlDrawing{}.vml", i + 1), v.as_bytes(), true);
        }
        if let Some((d, r)) = &drawings[i] {
            z.add(&format!("xl/drawings/drawing{}.xml", i + 1), d.as_bytes(), true);
            z.add(&format!("xl/drawings/_rels/drawing{}.xml.rels", i + 1), r.as_bytes(), true);
        }
    }
    for (path, xml, _) in &media.parts {
        z.add(path, xml.as_bytes(), true);
    }
    for ((k, x), q) in tables.iter().flatten().zip(keep.queries.iter().flatten()) {
        z.add(&format!("xl/tables/table{k}.xml"), x.as_bytes(), true);
        if let Some(q) = q {
            z.add(&format!("xl/tables/_rels/table{k}.xml.rels"), crate::xlsx_keep::table_rels(q).as_bytes(), true);
        }
    }
    for (name, data) in &media.files {
        // Compressed formats don't shrink: store them.
        let packed = matches!(apex_sheet::ImageFormat::sniff(data), Some(apex_sheet::ImageFormat::Png | apex_sheet::ImageFormat::Jpeg | apex_sheet::ImageFormat::Gif | apex_sheet::ImageFormat::Webp));
        z.add(&format!("xl/media/{name}"), data, !packed);
    }
    media.objects.write_files(&mut z);
    z.finish()
}

/// Error text for a file: Excel only reads the classic errors and those
/// of dynamic arrays there; #CONNECT! is computed again on load.
fn xlsx_error(e: ErrorKind) -> &'static str {
    match e {
        ErrorKind::Connect => ErrorKind::Value.as_str(),
        e => e.as_str(),
    }
}

pub(crate) fn write_number(out: &mut String, n: f64) {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        let _ = write!(out, "{}", n as i64);
    } else {
        let _ = write!(out, "{}", n);
    }
}

/// `vm`: value metadata of the pictures in cells.
/// The `t` attribute of a formula cell with this value.
fn value_type(v: &Value) -> &'static str {
    match v {
        Value::Text(_) => r#" t="str""#,
        Value::Bool(_) => r#" t="b""#,
        Value::Error(_) => r#" t="e""#,
        _ => "",
    }
}

/// The last value of a formula cell.
fn write_cached(s: &mut String, v: &Value) {
    match v {
        Value::Number(n) => {
            s.push_str("<v>");
            write_number(s, *n);
            s.push_str("</v>");
        }
        Value::Text(x) => {
            let _ = write!(s, "<v>{}</v>", esc(x));
        }
        Value::Bool(b) => {
            let _ = write!(s, "<v>{}</v>", *b as u8);
        }
        Value::Error(e) => {
            let _ = write!(s, "<v>{}</v>", esc(xlsx_error(*e)));
        }
        Value::Empty | Value::Array(_) | Value::Lambda(_) => {}
    }
}

fn write_sheet(wb: &Workbook, si: usize, sh: &Sheet, dxfs: &[apex_sheet::data::CfStyle], parts: &crate::xlsx_notes::SheetParts, vm: &HashMap<(usize, CellRef), usize>, sst: &mut dyn FnMut(&Arc<str>) -> usize) -> String {
    let mut s = String::with_capacity(sh.cell_count() * 40 + 1024);
    s.push_str(HEADER);
    s.push_str(r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">"#);
    let outline_pr = !sh.summary_below || !sh.summary_right;
    let page_pr = crate::xlsx_page::sheet_pr(sh);
    if sh.tab_color.is_some() || outline_pr || page_pr.is_some() || sh.code_name.is_some() || sh.lotus_eval {
        s.push_str("<sheetPr");
        if let Some(c) = &sh.code_name {
            let _ = write!(s, r#" codeName="{}""#, esc(c));
        }
        if sh.lotus_eval {
            s.push_str(r#" transitionEvaluation="1""#);
        }
        s.push('>');
        if let Some(c) = sh.tab_color {
            let _ = write!(s, r#"<tabColor rgb="{}"/>"#, color_attr(c));
        }
        if outline_pr {
            let _ = write!(s, r#"<outlinePr summaryBelow="{}" summaryRight="{}"/>"#, sh.summary_below as u8, sh.summary_right as u8);
        }
        s.push_str(page_pr.unwrap_or_default());
        s.push_str("</sheetPr>");
    }
    if let Some(ext) = sh.extent() {
        let _ = write!(s, r#"<dimension ref="A1:{}"/>"#, ext.to_a1());
    }
    s.push_str("<sheetViews><sheetView workbookViewId=\"0\"");
    if si == wb.sheets.iter().position(|s| !s.hidden).unwrap_or(0) {
        s.push_str(r#" tabSelected="1""#);
    }
    if let Some(v) = crate::xlsx_page::view_attr(sh.view) {
        let _ = write!(s, r#" view="{v}""#);
    }
    let top = CellRef::new(if sh.frozen_rows > 0 { sh.frozen_top.row.min(sh.frozen_rows - 1) } else { 0 }, if sh.frozen_cols > 0 { sh.frozen_top.col.min(sh.frozen_cols - 1) } else { 0 });
    if top != CellRef::new(0, 0) {
        let _ = write!(s, r#" topLeftCell="{}""#, top.to_a1());
    }
    if sh.frozen_rows > 0 || sh.frozen_cols > 0 {
        let top_left = CellRef::new(sh.frozen_rows, sh.frozen_cols).to_a1();
        let pane = match (sh.frozen_rows > 0, sh.frozen_cols > 0) {
            (true, true) => "bottomRight",
            (true, false) => "bottomLeft",
            _ => "topRight",
        };
        s.push('>');
        s.push_str("<pane");
        if sh.frozen_cols > 0 {
            let _ = write!(s, r#" xSplit="{}""#, sh.frozen_cols - top.col);
        }
        if sh.frozen_rows > 0 {
            let _ = write!(s, r#" ySplit="{}""#, sh.frozen_rows - top.row);
        }
        let _ = write!(s, r#" topLeftCell="{}" activePane="{}" state="frozen"/></sheetView>"#, top_left, pane);
    } else {
        s.push_str("/>");
    }
    s.push_str("</sheetViews>");
    let def_h = sh.default_row_height / ROW_SCALE;
    let _ = write!(s, r#"<sheetFormatPr defaultRowHeight="{}""#, (def_h * 100.0).round() / 100.0);
    if (sh.default_col_width - 88.0).abs() > 0.5 {
        let _ = write!(s, r#" defaultColWidth="{}""#, width_to_chars(sh.default_col_width));
    }
    for (attr, levels) in [("outlineLevelRow", &sh.row_levels), ("outlineLevelCol", &sh.col_levels)] {
        if let Some(m) = levels.values().max() {
            let _ = write!(s, r#" {attr}="{m}""#);
        }
    }
    s.push_str("/>");
    let mut cols: Vec<u32> = sh.col_widths.keys().chain(sh.col_levels.keys()).copied().collect();
    cols.sort_unstable();
    cols.dedup();
    if !cols.is_empty() {
        s.push_str("<cols>");
        for c in cols {
            let _ = write!(s, r#"<col min="{}" max="{}""#, c + 1, c + 1);
            match sh.col_widths.get(&c) {
                Some(&w) if w <= 0.0 => s.push_str(r#" width="0" hidden="1" customWidth="1""#),
                Some(&w) => {
                    let _ = write!(s, r#" width="{}" customWidth="1""#, width_to_chars(w));
                }
                None => {
                    let _ = write!(s, r#" width="{}""#, width_to_chars(sh.default_col_width));
                }
            }
            if let Some(l) = sh.col_levels.get(&c) {
                let _ = write!(s, r#" outlineLevel="{l}""#);
            }
            s.push_str("/>");
        }
        s.push_str("</cols>");
    }
    s.push_str("<sheetData>");
    // Formulas with functions only Google Sheets has are written as it
    // writes them, with their last values (the spilled ones too).
    use apex_sheet::functions::google;
    let google_spills: Vec<Range> = sh
        .iter()
        .filter_map(|(c, cell)| match &cell.content {
            Content::Formula { expr, .. } if google::uses_google(&expr.expr()) => wb.spill_range(si, c),
            _ => None,
        })
        .collect();
    // Gather cells by row (storage is column-major).
    let mut rows: std::collections::BTreeMap<u32, Vec<(u32, &apex_sheet::Cell)>> = Default::default();
    for (c, cell) in sh.iter() {
        rows.entry(c.row).or_default().push((c.col, cell));
    }
    for &r in sh.row_heights.keys().chain(sh.row_levels.keys()) {
        rows.entry(r).or_default();
    }
    for (r, mut cells) in rows {
        cells.sort_by_key(|(c, _)| *c);
        let _ = write!(s, r#"<row r="{}""#, r + 1);
        if let Some(&h) = sh.row_heights.get(&r) {
            if h <= 0.0 {
                s.push_str(r#" hidden="1""#);
            } else {
                let _ = write!(s, r#" ht="{}" customHeight="1""#, ((h / ROW_SCALE) * 100.0).round() / 100.0);
            }
        }
        if let Some(l) = sh.row_levels.get(&r) {
            let _ = write!(s, r#" outlineLevel="{l}""#);
        }
        s.push('>');
        for (c, cell) in cells {
            let a1 = CellRef::new(r, c).to_a1();
            let style = if cell.style != 0 { format!(r#" s="{}""#, cell.style) } else { String::new() };
            match &cell.content {
                Content::Empty => {
                    let _ = write!(s, r#"<c r="{}"{}/>"#, a1, style);
                }
                Content::Spill(v) if google_spills.iter().any(|g| g.contains(CellRef::new(r, c))) => {
                    let _ = write!(s, r#"<c r="{}"{}{}><f>{}</f>"#, a1, style, value_type(v), esc(&google::spilled_to_google(v)));
                    write_cached(&mut s, v);
                    s.push_str("</c>");
                }
                Content::Value(v) | Content::Spill(v) => match v {
                    Value::Number(n) => {
                        let _ = write!(s, r#"<c r="{}"{}><v>"#, a1, style);
                        write_number(&mut s, *n);
                        s.push_str("</v></c>");
                    }
                    Value::Text(t) if let Some(runs) = cell.rich() => {
                        let _ = write!(s, r#"<c r="{}"{} t="inlineStr"><is>"#, a1, style);
                        crate::xlsx_rich::write_runs(&mut s, t, runs, wb.styles.get(cell.style));
                        s.push_str("</is></c>");
                    }
                    Value::Text(t) => {
                        let i = sst(t);
                        let _ = write!(s, r#"<c r="{}"{} t="s"><v>{}</v></c>"#, a1, style, i);
                    }
                    Value::Bool(b) => {
                        let _ = write!(s, r#"<c r="{}"{} t="b"><v>{}</v></c>"#, a1, style, *b as u8);
                    }
                    Value::Error(_) if let Some(m) = vm.get(&(si, CellRef::new(r, c))) => {
                        let _ = write!(s, r#"<c r="{}"{} t="e" vm="{m}"><v>#VALUE!</v></c>"#, a1, style);
                    }
                    Value::Error(e) => {
                        let _ = write!(s, r#"<c r="{}"{} t="e"><v>{}</v></c>"#, a1, style, esc(xlsx_error(*e)));
                    }
                    Value::Empty | Value::Array(_) | Value::Lambda(_) => {
                        let _ = write!(s, r#"<c r="{}"{}/>"#, a1, style);
                    }
                },
                Content::Formula { expr, value } => {
                    let expr = &*expr.expr();
                    let t = value_type(value);
                    let here = CellRef::new(r, c);
                    if google::uses_google(expr) {
                        let _ = write!(s, r#"<c r="{}"{}{}><f>{}</f>"#, a1, style, t, esc(&google::to_google(&wb.formula_for_xlsx(si, here, expr), value)));
                    } else if let Some((rows, cols)) = cell.extra.as_ref().and_then(|x| x.array_size) {
                        // An array formula of fixed size ({=...} in Excel).
                        let area = Range::new(here, CellRef::new(r + rows.max(1) - 1, c + cols.max(1) - 1));
                        let _ = write!(s, r#"<c r="{}"{}{}><f t="array" ref="{}">{}</f>"#, a1, style, t, area.to_a1(), esc(&wb.formula_for_xlsx(si, here, expr)));
                    } else if wb.is_array_formula(si, here) {
                        // A dynamic array: cm points at the XLDAPR metadata.
                        let area = wb.spill_range(si, here).unwrap_or(Range::single(here));
                        let _ = write!(s, r#"<c r="{}"{}{} cm="1"><f t="array" ref="{}">{}</f>"#, a1, style, t, area.to_a1(), esc(&wb.formula_for_xlsx(si, here, expr)));
                    } else if expr.needs_array() {
                        // One value from ranges taken whole (`SUM(A1:A3*B1:B3)`):
                        // a plain formula would read back with `@` on them.
                        let _ = write!(s, r#"<c r="{}"{}{}><f t="array" ref="{}">{}</f>"#, a1, style, t, a1, esc(&wb.formula_for_xlsx(si, here, expr)));
                    } else {
                        let _ = write!(s, r#"<c r="{}"{}{}><f>{}</f>"#, a1, style, t, esc(&wb.formula_for_xlsx(si, here, expr)));
                    }
                    write_cached(&mut s, value);
                    s.push_str("</c>");
                }
            }
        }
        s.push_str("</row>");
    }
    s.push_str("</sheetData>");
    crate::xlsx_notes::write_sheet_protection(&mut s, sh);
    crate::xlsx_notes::write_protected_ranges(&mut s, sh);
    crate::scenario_io::write_xlsx(&mut s, wb, &sh.name);
    crate::xlsx_data::write_autofilter(&mut s, sh, dxfs);
    crate::xlsx_views::write_sheet(&mut s, sh, dxfs);
    if !sh.merges.is_empty() {
        let _ = write!(s, r#"<mergeCells count="{}">"#, sh.merges.len());
        for m in &sh.merges {
            let _ = write!(s, r#"<mergeCell ref="{}"/>"#, m.to_a1());
        }
        s.push_str("</mergeCells>");
    }
    crate::xlsx_data::write_cf_dv(&mut s, sh, dxfs);
    s.push_str(&parts.hyperlinks);
    crate::xlsx_page::write_sheet_page(&mut s, sh);
    s.push_str(&parts.drawing);
    s.push_str(&parts.legacy_drawing);
    if sh.page.background.is_some() {
        let _ = write!(s, r#"<picture r:id="{}"/>"#, crate::xlsx_page::BACKGROUND_RID);
    }
    s.push_str(&parts.ole_objects);
    crate::xlsx_chart::write_sparklines(&mut s, sh);
    s.push_str("</worksheet>");
    let _ = wb;
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_sheet::sheet::{ROTATION_STACKED, border};
    use apex_sheet::{Borders, HAlign, Line, LineStyle};

    #[test]
    fn cells_read_apart_match_cells_read_in_order() {
        // Several pieces of cells: a shared formula started in the first and
        // used in the last, row heights, and rows without their address at
        // the end (that piece is read again in order).
        let mut x = String::from(r#"<worksheet><sheetFormatPr defaultRowHeight="15"/><sheetData>"#);
        x += r#"<row r="1"><c r="A1"><v>1</v></c><c r="B1"><f t="shared" si="0" ref="B1:B90000">A1*2</f><v>2</v></c></row>"#;
        for r in 2..=90_000 {
            let _ = write!(x, r#"<row r="{r}" ht="20" customHeight="1"><c r="A{r}" t="inlineStr"><is><t>t{}</t></is></c><c r="B{r}"><f t="shared" si="0"/></c><c r="C{r}" s="1"><f>A{r}&amp;"x"</f><v></v></c></row>"#, r % 7);
        }
        x += r#"<row><c><v>5</v></c><c><f>A90001+1</f></c></row></sheetData></worksheet>"#;
        let mut w = ZipWriter::new();
        w.add("s.xml", x.as_bytes(), true);
        let bytes = w.finish();
        let z = ZipReader::new(&bytes).unwrap();
        let theme = apex_sheet::Theme::default();
        let strings: Strings = (Vec::new(), Vec::new());
        let read = |helpers| {
            let mut sh = Sheet::new("S");
            let (st, rest) = read_sheet_cells(&z, "s.xml", &mut sh, helpers, &theme, &[0, 7], &strings, &[]);
            rest.unwrap();
            assert!(!st.failed);
            sh
        };
        let (apart, in_order) = (read(3), read(1));
        assert_eq!(apart.cell_count(), in_order.cell_count());
        assert!(apart.iter().zip(in_order.iter()).all(|(a, b)| a == b));
        assert_eq!(apart.row_heights.len(), 89_999);
        assert!(apart.row_heights == in_order.row_heights);
        assert_eq!(apart.get(CellRef::new(89_999, 1)).and_then(|c| c.formula()).unwrap().expr().to_canonical(), "A90000*2");
        assert_eq!(apart.get(CellRef::new(90_000, 1)).and_then(|c| c.formula()).unwrap().expr().to_canonical(), "A90001+1");
        assert_eq!(apart.get(CellRef::new(500, 2)).unwrap().style, 7);
    }

    #[test]
    fn roundtrip() {
        let mut wb = Workbook::new();
        wb.locale = apex_sheet::Locale::EN;
        wb.set_input(0, CellRef::new(0, 0), "Producto");
        wb.set_input(0, CellRef::new(1, 0), "12.5");
        wb.set_input(0, CellRef::new(2, 0), "=A2*2");
        wb.set_input(0, CellRef::new(3, 0), "=\"x\"&A1");
        wb.set_input(0, CellRef::new(4, 0), "15%");
        wb.modify_style(0, &Range::single(CellRef::new(0, 0)), |s| {
            s.bold = true;
            s.fill = Some(0xFFFF00);
            s.borders = Borders::sides(border::BOTTOM, Line::new(LineStyle::Double, Some(0xFF0000)));
            s.halign = HAlign::Center;
            s.font = Some(Arc::from("Liberation Serif"));
            s.indent = 2;
            s.rotation = -45;
        });
        wb.modify_style(0, &Range::single(CellRef::new(1, 0)), |s| {
            s.halign = HAlign::Justify;
            s.rotation = ROTATION_STACKED;
            s.borders = Borders::sides(border::ALL, Line::THIN);
        });
        wb.sheets[0].tab_color = Some(0x00B050);
        wb.sheets.push(Sheet::new("Hidden"));
        wb.sheets[1].hidden = true;
        wb.sheets[0].col_widths.insert(0, 150.0);
        wb.sheets[0].frozen_rows = 1;
        wb.sheets[0].merges.push(Range::parse("B1:C1").unwrap());
        let bytes = write(&wb);
        let mut back = read(&bytes).unwrap();
        back.locale = apex_sheet::Locale::EN;
        let sh = &back.sheets[0];
        assert_eq!(sh.value(CellRef::new(0, 0)), &Value::text("Producto"));
        assert_eq!(back.display(0, CellRef::new(2, 0)).0, "25");
        assert_eq!(back.display(0, CellRef::new(3, 0)).0, "xProducto");
        assert_eq!(back.display(0, CellRef::new(4, 0)).0, "15%");
        let st = back.styles.get(sh.get(CellRef::new(0, 0)).unwrap().style);
        assert!(st.bold);
        assert_eq!(st.fill, Some(0xFFFF00));
        assert_eq!(st.borders, Borders::sides(border::BOTTOM, Line::new(LineStyle::Double, Some(0xFF0000))));
        assert_eq!(st.halign, HAlign::Center);
        assert_eq!(st.font.as_deref(), Some("Liberation Serif"));
        assert_eq!((st.indent, st.rotation), (2, -45));
        let st2 = back.styles.get(sh.get(CellRef::new(1, 0)).unwrap().style);
        assert_eq!((st2.halign, st2.rotation), (HAlign::Justify, ROTATION_STACKED));
        assert_eq!(st2.borders, Borders::sides(border::ALL, Line::THIN));
        assert_eq!(sh.tab_color, Some(0x00B050));
        assert!(back.sheets[1].hidden && !sh.hidden);
        assert!((sh.col_width(0) - 150.0).abs() < 3.0);
        assert_eq!(sh.frozen_rows, 1);
        assert_eq!(sh.merges.len(), 1);
    }

    #[test]
    fn data_tools() {
        let wb = crate::xlsx_data::sample::workbook();
        let bytes = write(&wb);
        // APEX_DUMP=dir saves the file to check it with other programs.
        if let Some(d) = std::env::var_os("APEX_DUMP") {
            std::fs::write(std::path::Path::new(&d).join("data_tools.xlsx"), &bytes).unwrap();
        }
        let back = read(&bytes).unwrap();
        crate::xlsx_data::sample::check(&wb, &back, true);
    }

    #[test]
    fn page_setup() {
        let mut wb = Workbook::new();
        wb.set_input(0, CellRef::new(0, 0), "x");
        wb.sheets[0].name = "My 'sheet'".into();
        wb.sheets[0].page = crate::xlsx_page::tests::sample_page();
        wb.sheets.push(Sheet::new("Plain"));
        let mut p = apex_sheet::SheetPage { scale: 75, ..Default::default() };
        p.hf.footer = apex_sheet::HfText::center("&P");
        wb.sheets[1].page = p;
        let bytes = write(&wb);
        if let Some(d) = std::env::var_os("APEX_DUMP") {
            std::fs::write(std::path::Path::new(&d).join("page_setup.xlsx"), &bytes).unwrap();
        }
        let back = read(&bytes).unwrap();
        for i in 0..2 {
            crate::xlsx_page::tests::check_page(&wb.sheets[i].page, &back.sheets[i].page, 0.1);
        }
    }

    #[test]
    fn views_and_background() {
        use apex_sheet::{SheetBackground, ViewMode};
        let mut wb = Workbook::new();
        wb.set_input(0, CellRef::new(0, 0), "x");
        wb.sheets[0].view = ViewMode::PageLayout;
        let png = crate::xlsx_drawing::tests::png();
        wb.set_background(0, Some(SheetBackground { data: png.clone(), format: apex_sheet::ImageFormat::Png }));
        wb.sheets.push(Sheet::new("Two"));
        wb.sheets[1].view = ViewMode::PageBreakPreview;
        // The same picture as a floating image shares the media part.
        wb.add_image(1, apex_sheet::SheetImage::new(png.clone(), apex_sheet::ImageFormat::Png, CellRef::new(1, 1), 40.0, 30.0));
        wb.sheets.push(Sheet::new("Three"));
        let bytes = write(&wb);
        if let Some(d) = std::env::var_os("APEX_DUMP") {
            std::fs::write(std::path::Path::new(&d).join("views.xlsx"), &bytes).unwrap();
        }
        let z = crate::zip::ZipReader::new(&bytes).unwrap();
        let s1 = z.read_string("xl/worksheets/sheet1.xml").unwrap();
        assert!(s1.contains(r#"view="pageLayout""#) && s1.contains(r#"<picture r:id="rIdBg1"/>"#), "{s1}");
        let back = read(&bytes).unwrap();
        assert_eq!(back.sheets.iter().map(|s| s.view).collect::<Vec<_>>(), vec![ViewMode::PageLayout, ViewMode::PageBreakPreview, ViewMode::Normal]);
        assert_eq!(back.sheets[0].page.background, wb.sheets[0].page.background);
        assert_eq!(back.sheets[1].images.len(), 1);
        assert!(back.sheets[1].page.background.is_none());
        // Removing it is undoable.
        let mut wb = back;
        wb.set_background(0, None);
        assert!(wb.sheets[0].page.background.is_none());
        wb.undo();
        assert!(wb.sheets[0].page.background.is_some());
    }

    #[test]
    fn shared_formulas() {
        let sheet = r#"<worksheet><sheetData><row r="1"><c r="A1"><v>1</v></c><c r="B1"><f t="shared" ref="B1:B3" si="0">A1*2</f><v>2</v></c></row><row r="2"><c r="A2"><v>5</v></c><c r="B2"><f t="shared" si="0"/></c></row></sheetData></worksheet>"#;
        let mut wb = Workbook::empty();
        wb.sheets.push(Sheet::new("S"));
        read_sheet(sheet, &mut wb, 0, &[], &Default::default(), &[]);
        wb.finish_load();
        assert_eq!(wb.sheets[0].value(CellRef::new(1, 1)), &Value::Number(10.0));
    }

    /// Formulas of Google Sheets are calculated again; the ones that use
    /// what isn't known here keep their last values.
    #[test]
    fn google_sheets_formulas() {
        let f = |s: &str| s.replace('"', "&quot;");
        let sheet = format!(
            r#"<worksheet><sheetData><row r="1"><c r="A1" t="str"><f>{}</f><v>a</v></c><c r="B1" t="str"><f>{}</f><v>b</v></c><c r="C1" t="str"><f>{}</f><v>old</v></c><c r="D1"><f>{}</f><v>150</v></c></row></sheetData></worksheet>"#,
            f(r#"IFERROR(__xludf.DUMMYFUNCTION("SPLIT(""a,b,c"","","")"),"a")"#),
            f(r#"IFERROR(__xludf.DUMMYFUNCTION("""COMPUTED_VALUE"""),"b")"#),
            f(r#"IFERROR(__xludf.DUMMYFUNCTION("""COMPUTED_VALUE"""),"old")"#),
            f(r#"__xludf.DUMMYFUNCTION("GOOGLEFINANCE(""GOOG"")")"#),
        );
        let mut wb = Workbook::empty();
        wb.locale = apex_sheet::Locale::EN;
        wb.sheets.push(Sheet::new("S"));
        read_sheet(&sheet, &mut wb, 0, &[], &Default::default(), &[]);
        wb.finish_load();
        wb.recalc_all();
        let v = |c: u32| wb.sheets[0].value(CellRef::new(0, c)).clone();
        assert_eq!((v(0), v(1), v(2), v(3)), (Value::text("a"), Value::text("b"), Value::text("c"), Value::Number(150.0)));
        assert!(wb.is_array_formula(0, CellRef::new(0, 0)));
        // Written back as Google writes them.
        let bytes = write(&wb);
        let xml = crate::zip::ZipReader::new(&bytes).unwrap().read_string("xl/worksheets/sheet1.xml").unwrap();
        assert!(xml.contains(&f(r#"<f>IFERROR(__xludf.DUMMYFUNCTION("SPLIT(""a,b,c"","","")"),"a")</f>"#)), "{xml}");
        assert!(xml.contains("COMPUTED_VALUE"), "{xml}");
        let mut back = read(&bytes).unwrap();
        back.locale = apex_sheet::Locale::EN;
        back.recalc_all();
        assert_eq!(back.sheets[0].value(CellRef::new(0, 2)), &Value::text("c"));
        assert_eq!(back.sheets[0].value(CellRef::new(0, 3)), &Value::Number(150.0));
    }
}

#[cfg(test)]
mod stale_tests {
    use super::*;

    #[test]
    fn values_kept_otherwise_than_worked_out_are_noticed() {
        let mut wb = Workbook::new();
        wb.set_input(0, CellRef::new(0, 0), "450");
        wb.set_input(0, CellRef::new(1, 0), "=A1*2");
        wb.set_input(0, CellRef::new(2, 0), "=NOW()");
        let file = write(&wb);
        assert!(!read(&file).unwrap().loaded_stale);
        // The doubled value kept as ten times as much.
        let z = crate::zip::ZipReader::new(&file).unwrap();
        let mut w = crate::zip::ZipWriter::new();
        for n in z.names() {
            let mut b = z.read(n).unwrap();
            if n.ends_with("sheet1.xml") {
                let s = String::from_utf8(b).unwrap();
                assert!(s.contains("<v>900</v>"));
                b = s.replace("<v>900</v>", "<v>9000</v>").into_bytes();
            }
            w.add(n, &b, true);
        }
        assert!(read(&w.finish()).unwrap().loaded_stale);
    }
}
