//! The views the QA checks, each on a sample of medium size, and the
//! fingerprints of what they edit: the model in named parts (a
//! paragraph's text, a sheet's formats, a slide), to tell what an action
//! changed and whether undo, redo or a saved file brings it back.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

use apex_sheet::{CellRef, Workbook};

use super::super::{App, View};
use crate::app::audit;

/// A view to check, with what it shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Qv {
    Doc,
    /// Paragraphs [`DOC_SEL`] selected.
    DocSel,
    Sheet,
    /// Cells [`SHEET_SEL`] selected.
    SheetSel,
    Slides,
    /// The first slide's objects selected.
    SlidesSel,
    Pdf,
    Db,
    /// A document in the code view (View > Code).
    Code,
    /// A notebook: sections, pages, a canvas, a page type.
    Note,
    /// A workbook with macros, enabled, in the macro editor.
    Macros,
}

pub(super) const ALL: [Qv; 11] = [Qv::Doc, Qv::DocSel, Qv::Sheet, Qv::SheetSel, Qv::Slides, Qv::SlidesSel, Qv::Pdf, Qv::Db, Qv::Code, Qv::Note, Qv::Macros];

/// The paragraphs selected in [`Qv::DocSel`] (first and last): all of
/// them body text, alike (a format reaches all of them or none).
pub(super) const DOC_SEL: (usize, usize) = (1, 4);
/// The cells selected in [`Qv::SheetSel`] (A2:C11).
pub(super) const SHEET_SEL: ((u32, u32), (u32, u32)) = ((1, 0), (10, 2));

impl Qv {
    pub fn name(self) -> String {
        format!("{self:?}")
    }

    pub fn parse(s: &str) -> Option<Qv> {
        ALL.into_iter().find(|q| q.name() == s)
    }

    /// Opens the view in a blank window; `n` tells fixtures on disk apart.
    pub fn setup(self, app: &mut App, n: u64) {
        match self {
            Qv::Doc | Qv::DocSel | Qv::Code => {
                let mut v = crate::doc_view::DocView::new(medium_doc());
                if self == Qv::DocSel {
                    v.select_paras(DOC_SEL.0, DOC_SEL.1);
                }
                if self == Qv::Code {
                    v.toggle_md_source();
                }
                app.push_tab(View::Doc(Box::new(v)), None, None);
            }
            Qv::Sheet | Qv::SheetSel => {
                let mut v = crate::sheet_view::SheetView::new(medium_book());
                if self == Qv::SheetSel {
                    let ((r0, c0), (r1, c1)) = SHEET_SEL;
                    v.select_range(CellRef::new(r0, c0), CellRef::new(r1, c1));
                }
                app.push_tab(View::Sheet(Box::new(v)), None, None);
            }
            Qv::Slides | Qv::SlidesSel => {
                let mut v = crate::slides_view::SlidesView::new(medium_deck());
                if self == Qv::SlidesSel {
                    v.select_all();
                }
                app.push_tab(View::Slides(Box::new(v)), None, None);
            }
            Qv::Pdf => app.open_path(&medium_pdf()),
            Qv::Db => app.open_db_tab(&audit::db_fixture(n)),
            Qv::Note => app.open_note_tab(&audit::note_fixture(n)),
            Qv::Macros => {
                app.settings.macros_enabled = true;
                app.push_tab(View::Sheet(Box::new(crate::sheet_view::SheetView::new(audit::with_macros(medium_book())))), None, None);
                let id = app.tabs[app.active].id;
                app.enable_macros(id);
                app.open_macros();
            }
        }
        // What the samples reported while built isn't the actions'.
        super::common::take_errors();
    }
}

// --- the samples -----------------------------------------------------------------------

/// A document of a few pages: headings, lists, emphasis, a table, links.
pub(super) fn medium_doc() -> apex_doc::Document {
    let mut md = String::new();
    for s in 1..=6 {
        md.push_str(&format!("# Section {s}\n\nThe opening paragraph of section {s}, in plain words, long enough to wrap over more than one line of the page.\n\nA second paragraph of plain text that says little, but says it at some length, as reports do.\n\nA third one, as plain.\n\nAnd a fourth, the last before the lists.\n\n"));
        md.push_str(&format!("- First point of {s}\n- Second point, a little longer\n- Third point\n\n1. One\n2. Two\n\n"));
        md.push_str(&format!("| Item | Amount |\n|---|---|\n| Alpha {s} | {} |\n| Beta {s} | {} |\n\n", s * 10, s * 20));
        md.push_str("A closing paragraph with **bold**, *italic* and `code` words, a [link](https://example.com) and more text after it.\n\n");
    }
    apex_io::markdown::read(&md)
}

/// A sheet of 120 rows: text, numbers, dates and formulas (made once a
/// thread: it goes through a file).
pub(super) fn medium_book() -> Workbook {
    thread_local! {
        static BOOK: Workbook = build_book();
    }
    BOOK.with(Clone::clone)
}

fn build_book() -> Workbook {
    let mut wb = Workbook::empty();
    wb.sheets.push(apex_sheet::Sheet::new("Data"));
    wb.sheets.push(apex_sheet::Sheet::new("Other"));
    let mut inputs = vec![(CellRef::new(0, 0), "Item".to_string()), (CellRef::new(0, 1), "Amount".into()), (CellRef::new(0, 2), "Price".into()), (CellRef::new(0, 3), "Total".into()), (CellRef::new(0, 4), "Date".into())];
    for r in 1..=120u32 {
        inputs.push((CellRef::new(r, 0), format!("Item {r}")));
        inputs.push((CellRef::new(r, 1), format!("{}", r * 3 % 17)));
        inputs.push((CellRef::new(r, 2), format!("{}.5", r % 11)));
        inputs.push((CellRef::new(r, 3), format!("=B{0}*C{0}", r + 1)));
        inputs.push((CellRef::new(r, 4), format!("2026-{:02}-{:02}", r % 12 + 1, r % 28 + 1)));
    }
    inputs.push((CellRef::new(122, 3), "=SUM(D2:D121)".into()));
    wb.set_inputs(0, inputs);
    wb.set_input(1, CellRef::new(0, 0), "=Data!D123");
    // Read back from a file: no undo steps, as when opened.
    let dir = audit::temp_dir();
    let p = dir.join(format!("qa-medium-{}.xlsx", std::process::id()));
    apex_io::save_sheet(&wb, &p, apex_io::Format::Xlsx).expect("sample workbook");
    let (loaded, _) = apex_io::open(&p).expect("sample workbook");
    let _ = std::fs::remove_file(&p);
    match loaded {
        apex_io::Loaded::Sheet(wb) => *wb,
        _ => unreachable!(),
    }
}

/// A deck of twelve slides with text.
pub(super) fn medium_deck() -> apex_slides::Deck {
    use apex_slides::object::ObjKind;
    let mut deck = apex_slides::build::new_deck(0);
    for i in 1..12 {
        let n = deck.slides.len();
        apex_slides::edit::add_slide(&mut deck, n, 0, 1);
        let _ = i;
    }
    for i in 0..deck.slides.len() {
        for (k, o) in deck.slide_mut(i).objects.iter_mut().enumerate() {
            if let ObjKind::Shape { text, .. } = &mut o.kind {
                let t = if k == 0 { format!("Slide {}", i + 1) } else { format!("A point of slide {}\nAnother point\nA third one", i + 1) };
                *text = Some(apex_slides::text::TextBody::plain(&t));
            }
        }
    }
    deck
}

/// A PDF of a few pages (written once per process).
pub(super) fn medium_pdf() -> std::path::PathBuf {
    static P: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        let bytes = crate::headless::pdf(apex_tui::Book::Doc(&medium_doc()), "qa");
        let p = audit::temp_dir().join("qa-sample.pdf");
        std::fs::write(&p, bytes).unwrap();
        p
    })
    .clone()
}

// --- fingerprints ----------------------------------------------------------------------

fn hash(v: &impl std::fmt::Debug) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    format!("{v:?}").hash(&mut h);
    h.finish()
}

/// The model in named parts, each a hash.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Model(pub BTreeMap<String, u64>);

impl Model {
    fn put(&mut self, k: impl Into<String>, v: &impl std::fmt::Debug) {
        self.0.insert(k.into(), hash(v));
    }

    /// The parts that differ from `other` (or are only in one of them).
    pub fn diff(&self, other: &Model) -> Vec<String> {
        let mut keys: Vec<&String> = self.0.keys().chain(other.0.keys()).collect();
        keys.sort();
        keys.dedup();
        keys.into_iter().filter(|k| self.0.get(*k) != other.0.get(*k)).cloned().collect()
    }

    /// Only the parts `keep` wants.
    pub fn only(&self, keep: impl Fn(&str) -> bool) -> Model {
        Model(self.0.iter().filter(|(k, _)| keep(k)).map(|(k, v)| (k.clone(), *v)).collect())
    }
}

/// The differing parts, briefly.
pub(super) fn brief(parts: &[String]) -> String {
    let mut s = parts.iter().take(4).cloned().collect::<Vec<_>>().join(", ");
    if parts.len() > 4 {
        s.push_str(&format!(" and {} more", parts.len() - 4));
    }
    s
}

pub(super) fn doc_model(d: &apex_doc::Document) -> Model {
    let mut m = Model::default();
    for (i, p) in d.paragraphs.iter().enumerate() {
        m.put(format!("p{i:04} text"), &p.text);
        m.put(format!("p{i:04} format"), &(&p.runs, &p.style, &p.marks, &p.cell, &p.section));
    }
    m.put("page", &d.page);
    m.put("headers", &d.headers);
    m.put("styles", &d.styles);
    m.put("comments", &d.comments);
    m.put("columns", &d.columns);
    m.put("deco", &d.deco);
    m.put("notes", &d.notes);
    m.put("biblio", &d.biblio);
    m.put("meta", &d.meta);
    m.put("settings", &(&d.default_font, d.odd_even, d.track_changes, d.auto_hyphenation, &d.protection));
    m
}

/// A cell as it shows: its content, its style (not its id) and the rest.
fn cell_print(wb: &Workbook, c: &apex_sheet::Cell) -> String {
    format!("{:?} {:?} {:?}", c.content, wb.styles.get(c.style), c.extra)
}

pub(super) fn book_model(wb: &Workbook) -> Model {
    let mut m = Model::default();
    for (i, s) in wb.sheets.iter().enumerate() {
        let mut values = String::new();
        let mut formats = String::new();
        for (at, c) in s.iter() {
            values.push_str(&format!("{at:?}={:?};", c.content));
            formats.push_str(&format!("{at:?}={:?}{:?};", wb.styles.get(c.style), c.extra));
        }
        m.put(format!("s{i} values"), &values);
        m.put(format!("s{i} formats"), &formats);
        m.put(format!("s{i} layout"), &((&s.name, &s.col_widths, &s.row_heights, s.default_col_width, s.default_row_height, s.frozen_rows, s.frozen_cols), (&s.merges, s.hidden, s.tab_color, &s.row_levels, &s.col_levels, s.summary_below, s.summary_right)));
        m.put(format!("s{i} rules"), &(&s.filter, &s.validations, &s.cond_formats, &s.protection, &s.edit_ranges));
        m.put(format!("s{i} objects"), &(&s.images, &s.charts, &s.shapes, &s.sparklines, &s.tables));
        m.put(format!("s{i} page"), &s.page);
    }
    m.put("sheets", &wb.sheets.len());
    m.put("names", &wb.names);
    m.put("calc", &wb.calc);
    m.put("meta", &wb.meta);
    m.put("cell styles", &wb.cell_styles);
    m.put("analysis", &wb.analysis);
    m
}

pub(super) fn deck_model(d: &apex_slides::Deck) -> Model {
    let mut m = Model::default();
    for (i, s) in d.slides.iter().enumerate() {
        m.put(format!("slide{i:03}"), &(&s.objects, s.layout, s.master, &s.name, &s.notes, s.hidden, &s.transition, &s.timing, &s.background, &s.map, s.show_master, &s.comments));
    }
    m.put("size", &(d.width, d.height, d.notes_size, d.first_number));
    m.put("masters", &d.masters);
    m.put("sections", &d.sections);
    m.put("show", &d.show);
    m.put("footers", &d.hf);
    m.put("meta", &d.meta);
    m
}

fn db_model(db: &apex_db::sqlite::Db) -> Model {
    let mut m = Model::default();
    let c = db.conn();
    let rows = |sql: &str| -> Vec<String> {
        let Ok(mut st) = c.prepare(sql) else { return Vec::new() };
        let n = st.column_count();
        st.query_map([], |r| Ok((0..n).map(|i| format!("{:?}", r.get_ref(i).map(|v| v.to_owned()))).collect::<Vec<_>>().join("|")))
            .map(|it| it.flatten().collect())
            .unwrap_or_default()
    };
    m.put("schema", &rows("SELECT type, name, sql FROM sqlite_master ORDER BY type, name"));
    for t in db.table_names() {
        m.put(format!("table {t}"), &rows(&format!("SELECT * FROM \"{}\"", t.replace('"', "\"\""))));
    }
    m
}

/// Which tab has the model of tab `id`: itself, or the window of the
/// same file that holds it (View > New Window).
fn holder(app: &App, id: u64) -> Option<usize> {
    let i = app.tabs.iter().position(|t| t.id == id)?;
    let t = &app.tabs[i];
    match t.group() {
        Some(g) if !t.holds => app.tabs.iter().position(|o| o.group() == Some(g) && o.holds).or(Some(i)),
        _ => Some(i),
    }
}

/// The view with the model of tab `id`.
pub(super) fn view(app: &App, id: u64) -> Option<&View> {
    holder(app, id).map(|i| &app.tabs[i].view)
}

pub(super) fn view_mut(app: &mut App, id: u64) -> Option<&mut View> {
    holder(app, id).map(|i| &mut app.tabs[i].view)
}

/// The model of tab `id`; none once it's closed.
pub(super) fn model(app: &App, id: u64) -> Option<Model> {
    Some(match view(app, id)? {
        View::Doc(v) => {
            let mut m = doc_model(&v.whole_doc());
            if let Some(code) = v.code_text() {
                m.put("code", &code);
            }
            m
        }
        View::Sheet(v) => book_model(&v.wb),
        View::Slides(v) => deck_model(&v.pres.deck),
        View::Pdf(v) => {
            // Its pages: their dictionaries (annotations, turn) and what
            // they draw.
            let mut m = Model::default();
            let pages = v.doc.pages();
            m.put("pages", &pages.len());
            for (i, n) in pages.into_iter().enumerate() {
                m.put(format!("page{i:03}"), &(v.doc.dict(n), v.doc.page_content(n)));
            }
            m
        }
        View::Db(v) => db_model(&v.db),
        // Its sections, pages and types; not the text of the open page
        // (each page keeps its own undo, gone once another page opens)
        // nor when pages were last written (no undo gives that back).
        View::Note(v) => {
            let s = v.nb.structure();
            let mut m = Model::default();
            m.put("sections", &s.sections);
            for p in s.pages {
                m.put(format!("page{:04}", p.id), &apex_doc::notebook::Page { modified: 0, ..p });
            }
            m.put("types", &s.types);
            m.put("queries", &s.queries);
            m
        }
    })
}

/// The selected elements (paragraphs, cells, objects), each as the print
/// of its text and of all of it, and one print of everything else of the
/// same kind.
pub(super) struct Elements {
    pub sel: Vec<(u64, u64)>,
    pub rest: u64,
}

pub(super) fn elements(app: &App, id: u64, qv: Qv) -> Option<Elements> {
    match (view(app, id)?, qv) {
        (View::Doc(v), Qv::DocSel) => {
            let ps = &v.doc.paragraphs;
            let range = DOC_SEL.0..=DOC_SEL.1;
            let sel = range.clone().map(|i| ps.get(i).map_or((0, 0), |p| (hash(&p.text), hash(&(&p.runs, &p.style))))).collect();
            let rest: Vec<_> = ps.iter().enumerate().filter(|(i, _)| !range.contains(i)).map(|(_, p)| p).collect();
            Some(Elements { sel, rest: hash(&(rest, ps.len())) })
        }
        (View::Sheet(v), Qv::SheetSel) => {
            let wb = &v.wb;
            let s = wb.sheets.first()?;
            let ((r0, c0), (r1, c1)) = SHEET_SEL;
            let inside = |at: CellRef| (r0..=r1).contains(&at.row) && (c0..=c1).contains(&at.col);
            let mut sel = Vec::new();
            for r in r0..=r1 {
                for c in c0..=c1 {
                    let cell = s.get(CellRef::new(r, c));
                    sel.push((hash(&cell.map(|c| &c.content)), hash(&cell.map(|cell| cell_print(wb, cell)))));
                }
            }
            let rest: Vec<String> = s.iter().filter(|(at, _)| !inside(*at)).map(|(at, c)| format!("{at:?}{}", cell_print(wb, c))).collect();
            Some(Elements { sel, rest: hash(&rest) })
        }
        (View::Slides(v), Qv::SlidesSel) => {
            let slide = v.pres.deck.slides.get(v.cur)?;
            let text = |o: &apex_slides::object::Object| match &o.kind {
                apex_slides::object::ObjKind::Shape { text, .. } => hash(&text.as_ref().map(|t| t.text())),
                _ => 0,
            };
            let sel = v.sel.iter().map(|id| slide.objects.iter().find(|o| o.id == *id).map_or((0, 0), |o| (text(o), hash(o)))).collect();
            let rest: Vec<_> = slide.objects.iter().filter(|o| !v.sel.contains(&o.id)).collect();
            let others: Vec<_> = v.pres.deck.slides.iter().enumerate().filter(|(i, _)| *i != v.cur).map(|(_, s)| &s.objects).collect();
            Some(Elements { sel, rest: hash(&(rest, others)) })
        }
        _ => None,
    }
}

/// Undoes (or redoes) one step in tab `id`, as the Undo command does.
pub(super) fn undo(app: &mut App, id: u64, redo: bool) {
    match view_mut(app, id) {
        Some(View::Doc(v)) => if redo { v.redo() } else { v.undo() },
        Some(View::Sheet(v)) => if redo { v.redo() } else { v.undo() },
        Some(View::Slides(v)) => if redo { v.redo() } else { v.undo() },
        Some(View::Pdf(v)) => if redo { v.redo() } else { v.undo() },
        Some(View::Db(v)) => if redo { v.redo() } else { v.undo() },
        Some(View::Note(v)) => match &mut v.editor {
            // The page's own steps first, then the notebook's.
            Some(ed) if (redo && ed.doc.can_redo()) || (!redo && ed.doc.can_undo()) => if redo { ed.redo() } else { ed.undo() },
            _ => if redo { v.redo() } else { v.undo() },
        },
        None => {}
    }
}

/// Whether tab `id` has steps left to undo; none when it can't tell.
pub(super) fn can_undo(app: &App, id: u64) -> Option<bool> {
    match view(app, id)? {
        View::Doc(v) if v.code_text().is_none() => Some(v.doc.can_undo()),
        View::Doc(_) => None,
        View::Sheet(v) => Some(v.wb.can_undo()),
        View::Slides(v) => Some(v.pres.can_undo()),
        View::Pdf(v) => Some(!v.doc.undo_names().is_empty()),
        View::Db(v) => Some(!v.undo_names().is_empty()),
        View::Note(v) => Some(v.can_undo() || v.editor.as_ref().is_some_and(|ed| ed.doc.can_undo())),
    }
}

/// Whether something done in tab `id` can't be undone by design (deleted
/// for good from a notebook's trash).
pub(super) fn irreversible(app: &App, id: u64) -> bool {
    matches!(view(app, id), Some(View::Note(v)) if v.purged)
}

/// The models of the file formats of the view that edits `m`: saved in
/// `fmt` and read back.
pub(super) enum Snapshot {
    Doc(Box<apex_doc::Document>),
    Sheet(Box<Workbook>),
    Slides(Box<apex_slides::Deck>),
}

impl Snapshot {
    pub fn take(app: &App, id: u64) -> Option<Snapshot> {
        match view(app, id)? {
            View::Doc(v) if v.code_text().is_none() => Some(Snapshot::Doc(Box::new(v.whole_doc().into_owned()))),
            View::Sheet(v) => Some(Snapshot::Sheet(Box::new(v.wb.clone()))),
            View::Slides(v) => Some(Snapshot::Slides(Box::new(v.pres.deck.clone()))),
            _ => None,
        }
    }

    /// The native formats, and the parts each can hold.
    pub fn formats(&self) -> Vec<(apex_io::Format, fn(&str) -> bool)> {
        use apex_io::Format::*;
        let all: fn(&str) -> bool = |_| true;
        match self {
            Snapshot::Doc(_) => vec![(Docx, all), (Odt, all), (Markdown, |k| k.ends_with(" text"))],
            Snapshot::Sheet(_) => vec![(Xlsx, all), (Ods, all), (Csv, |k| k == "s0 values")],
            Snapshot::Slides(_) => vec![(Pptx, all), (Odp, all)],
        }
    }

    /// Saved as `fmt` into `dir` and read back: its model, or the error.
    pub fn round_trip(&self, fmt: apex_io::Format, dir: &std::path::Path, name: &str) -> Result<Model, String> {
        let ext = match fmt {
            apex_io::Format::Markdown => "md".to_string(),
            f => format!("{f:?}").to_lowercase(),
        };
        let p = dir.join(format!("{name}.{ext}"));
        let saved = match self {
            Snapshot::Doc(d) => apex_io::save_doc(d, &p, fmt),
            Snapshot::Sheet(w) => apex_io::save_sheet(w, &p, fmt),
            Snapshot::Slides(d) => apex_io::save_slides(d, &p, fmt),
        };
        saved.map_err(|e| format!("save: {e}"))?;
        let loaded = apex_io::open(&p).map_err(|e| format!("open: {e}"));
        let _ = std::fs::remove_file(&p);
        Ok(match loaded? {
            (apex_io::Loaded::Doc(d), _) => doc_model(&d),
            (apex_io::Loaded::Sheet(w), _) => book_model(&w),
            (apex_io::Loaded::Slides(d), _) => deck_model(&d),
        })
    }
}
