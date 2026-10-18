//! Spreadsheet editor: a custom-painted, virtualized grid.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use apex_sheet::sheet::ROTATION_STACKED;
use apex_sheet::{
    BorderPreset, Cell, CellRef, CellStyle, Clip, Content, FillDir, HAlign, Line, LineStyle, MAX_COLS, MAX_ROWS, Op, PasteOp, PasteSpecial, PasteWhat, Range, SelectionStats, SeriesKind, SeriesOpts, Sheet, SortKey,
    VAlign, Value, ViewMode, Workbook,
};
use eframe::egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use eframe::egui::{self, Color32, CursorIcon, Event, EventFilter, FontId, Id, Key, Modifiers, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};

use crate::app::Notice;
use crate::common::{self, Shell};
use crate::fonts::{self, Face};
use crate::i18n::{tr, trf};
use crate::icons::Icon;
use crate::theme::{self, Paper};
use crate::widgets;

mod analysis_ui;
mod audit_ui;
#[cfg_attr(not(feature = "empresa"), path = "sheet_view/bank_ui_free.rs")]
mod bank_ui;
mod tests_ui;
mod mcp_tools;
mod origin_ui;
mod cellstyles_ui;
mod cf_ui;
mod commands;
#[cfg(test)]
mod panes_tests;
mod controls_ui;
mod data_ui;
mod datatools_ui;
mod dialogs;
mod f10;
mod forecast_ui;
// Planning (block 11).
mod gantt;
mod gantt_export;
mod gantt_print;
mod plan_views;
mod plan_ui;
mod plan_fields_ui;
mod movecopy_ui;
mod views_ui;
mod links_ui;
mod macro_hooks;
mod queries_ui;
mod model_ui;
pub(crate) mod getdata_forms;
mod getdata_run;
mod getdata_ui;
mod autofill_ui;
mod paste_opts_ui;
mod contextual;
mod charttpl_ui;
mod formula;
mod images_ui;
mod layout_ui;
mod incell_ui;
mod names_ui;
mod shapes_ui;
mod ink_ui;
mod charts_ui;
mod notes_ui;
mod outline_ui;
mod picture;
mod pivot_ui;
mod slicer_ui;
mod watch_ui;
mod print_ui;
pub use print_ui::PrintPlan;
pub(crate) use charts_ui::{CHART_TYPES, apply_type as apply_chart_type, chart_type_menu};
mod review_ui;
mod toolstrip_ui;
mod speak_ui;
mod spell_ui;
mod tables_ui;
mod quick_ui;
mod toolbar;
mod window_ui;

pub use commands::{set_status_stats, status_stats};
pub use f10::set_custom_lists;
pub use images_ui::EXTENSIONS as PICTURE_EXTENSIONS;
use commands::{Brush, ClearKind, TabAction};
use data_ui::DataState;
use dialogs::{Dialog, FindState};

const HEADER_H: f32 = 22.0;
const MIN_ROW_HDR_W: f32 = 44.0;
const SCROLLBAR: f32 = 12.0;
/// Pixels per point for cell fonts.
const PT: f32 = 1.333;
const DEFAULT_PT: f32 = 11.0;
/// Copies of more cells go on the system clipboard as text only.
const RICH_COPY_CELLS: u64 = 50_000;
/// Width of one indent step, in sheet units.
/// Keys the cell editors keep: Escape too, so it cancels the edit instead of
/// dropping focus (which would commit it).
const EDITOR_KEYS: EventFilter = EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true };

const INDENT_PX: f32 = 12.0;
const LAST: CellRef = CellRef { row: MAX_ROWS - 1, col: MAX_COLS - 1 };

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EditOrigin {
    Cell,
    Bar,
}

#[derive(Clone, Copy, Debug)]
struct Edit {
    origin: EditOrigin,
    /// Started by typing over the cell: arrow keys commit and move.
    typed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    None,
    Cells,
    Cols,
    Rows,
    ResizeCol { col: u32, orig: Option<f32>, start_w: f32, start_x: f32 },
    ResizeRow { row: u32, orig: Option<f32>, start_h: f32, start_y: f32 },
    Fill { target: Range },
    /// Clicking cells while typing a formula inserts references.
    FormulaRef { at: usize, anchor: CellRef },
    ScrollX { grab: f32 },
    ScrollY { grab: f32 },
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    Cell(CellRef),
    ColHeader(u32),
    RowHeader(u32),
    ColBorder(u32),
    RowBorder(u32),
    Corner,
    FillHandle,
    None,
}

/// Screen geometry of the grid for the current frame.
#[derive(Clone)]
struct Geo {
    /// Top-left of the cell area (after headers).
    origin: Pos2,
    view: Rect,
    z: f32,
    fr: u32,
    fc: u32,
    /// Frozen area size in sheet units.
    fx: f32,
    fy: f32,
    /// First row and column of the frozen panes, and their offsets.
    top: CellRef,
    ox: f32,
    oy: f32,
    scroll: Vec2,
    /// Sheet-unit offsets of the visible columns/rows (plus one past the
    /// end), so painting doesn't sum custom sizes for every cell.
    xs: AxisCache,
    ys: AxisCache,
    /// Page Layout view: cells on pages. Offsets (`scroll` included) are
    /// then layout units, with the margins and gaps between pages.
    pl: Option<std::rc::Rc<layout_ui::PageGeo>>,
}

#[derive(Default, Clone)]
struct AxisCache {
    frozen: Vec<f32>,
    start: u32,
    pos: Vec<f32>,
}

impl AxisCache {
    fn build(frozen: u32, visible: std::ops::RangeInclusive<u32>, at: impl Fn(u32) -> f32, size: impl Fn(u32) -> f32) -> AxisCache {
        let fill = |from: u32, to: u32| {
            let mut v = Vec::with_capacity((to - from + 2) as usize);
            let mut p = at(from);
            for i in from..=to {
                v.push(p);
                p += size(i);
            }
            v
        };
        let frozen = if frozen > 0 && frozen < 2000 { fill(0, frozen) } else { Vec::new() };
        // Hidden rows can make the window span a million: past a few
        // thousand, positions are looked up when asked.
        let (a, b) = (*visible.start(), (*visible.end()).min(u32::MAX - 2).min(visible.start().saturating_add(4096)) + 1);
        AxisCache { frozen, start: a, pos: fill(a, b) }
    }

    fn get(&self, i: u32, slow: impl Fn(u32) -> f32) -> f32 {
        if let Some(&p) = self.frozen.get(i as usize) {
            return p;
        }
        match i.checked_sub(self.start).and_then(|k| self.pos.get(k as usize)) {
            Some(&p) => p,
            None => slow(i),
        }
    }
}

impl Geo {
    /// Offset of column `c` before scrolling (sheet or layout units).
    fn ux(&self, sh: &Sheet, c: u32) -> f32 {
        let at = |c: u32| self.xs.get(c, |c| sh.col_x(c));
        match &self.pl {
            Some(pl) => pl.cols.pos(c, at),
            None => at(c),
        }
    }

    fn uy(&self, sh: &Sheet, r: u32) -> f32 {
        let at = |r: u32| self.ys.get(r, |r| sh.row_y(r));
        match &self.pl {
            Some(pl) => pl.rows.pos(r, at),
            None => at(r),
        }
    }

    fn x(&self, sh: &Sheet, c: u32) -> f32 {
        let s = if c < self.fc { self.ox } else { self.scroll.x };
        self.origin.x + (self.ux(sh, c) - s) * self.z
    }

    fn y(&self, sh: &Sheet, r: u32) -> f32 {
        let s = if r < self.fr { self.oy } else { self.scroll.y };
        self.origin.y + (self.uy(sh, r) - s) * self.z
    }

    /// Right edge of column `c` (in the Page Layout view the next column
    /// may start on another page).
    fn x_end(&self, sh: &Sheet, c: u32) -> f32 {
        match &self.pl {
            Some(_) => self.x(sh, c) + sh.col_width(c) * self.z,
            // The last frozen column ends where the next one would, unscrolled.
            None if c < self.fc => self.origin.x + (self.ux(sh, c + 1) - self.ox) * self.z,
            None => self.x(sh, c + 1),
        }
    }

    fn y_end(&self, sh: &Sheet, r: u32) -> f32 {
        match &self.pl {
            Some(_) => self.y(sh, r) + sh.row_height(r) * self.z,
            None if r < self.fr => self.origin.y + (self.uy(sh, r + 1) - self.oy) * self.z,
            None => self.y(sh, r + 1),
        }
    }

    fn col_at(&self, sh: &Sheet, x: f32) -> u32 {
        let sx = (x - self.origin.x) / self.z;
        if let Some(pl) = &self.pl {
            return pl.cols.index_at(sx + self.scroll.x, |x| sh.col_at(x.max(0.0))).min(MAX_COLS - 1);
        }
        if sx < self.fx { sh.col_at(sx.max(0.0) + self.ox) } else { sh.col_at(sx + self.scroll.x) }.min(MAX_COLS - 1)
    }

    fn row_at(&self, sh: &Sheet, y: f32) -> u32 {
        let sy = (y - self.origin.y) / self.z;
        if let Some(pl) = &self.pl {
            return pl.rows.index_at(sy + self.scroll.y, |y| sh.row_at(y.max(0.0))).min(MAX_ROWS - 1);
        }
        if sy < self.fy { sh.row_at(sy.max(0.0) + self.oy) } else { sh.row_at(sy + self.scroll.y) }.min(MAX_ROWS - 1)
    }

    /// Screen position of sheet point `(x, y)` in the pane with frozen
    /// columns (`fx`) and/or rows (`fy`).
    fn point_to_screen(&self, sh: &Sheet, fx: bool, fy: bool, x: f32, y: f32) -> Pos2 {
        let (sx, sy) = (if fx { self.ox } else { self.scroll.x }, if fy { self.oy } else { self.scroll.y });
        let (ux, uy) = match &self.pl {
            Some(pl) => (pl.cols.pos_of(sh.col_at(x.max(0.0)), x, |c| sh.col_x(c)), pl.rows.pos_of(sh.row_at(y.max(0.0)), y, |r| sh.row_y(r))),
            None => (x, y),
        };
        pos2(self.origin.x + (ux - sx) * self.z, self.origin.y + (uy - sy) * self.z)
    }

    /// Sheet point under screen point `p` in a pane.
    fn screen_to_point(&self, pane: (bool, bool), p: Pos2) -> Pos2 {
        let sx = if pane.0 { self.ox } else { self.scroll.x };
        let sy = if pane.1 { self.oy } else { self.scroll.y };
        let (ux, uy) = ((p.x - self.origin.x) / self.z + sx, (p.y - self.origin.y) / self.z + sy);
        match &self.pl {
            Some(pl) => pos2(pl.cols.sheet_at(ux).max(0.0), pl.rows.sheet_at(uy).max(0.0)),
            None => pos2(ux.max(0.0), uy.max(0.0)),
        }
    }

    fn cell_rect(&self, sh: &Sheet, c: CellRef) -> Rect {
        Rect::from_min_max(pos2(self.x(sh, c.col), self.y(sh, c.row)), pos2(self.x_end(sh, c.col), self.y_end(sh, c.row)))
    }

    fn range_rect(&self, sh: &Sheet, r: &Range) -> Rect {
        Rect::from_min_max(pos2(self.x(sh, r.start.col), self.y(sh, r.start.row)), pos2(self.x_end(sh, r.end.col), self.y_end(sh, r.end.row)))
    }

    /// Columns visible in the scrolled pane.
    fn scroll_cols(&self, sh: &Sheet) -> std::ops::RangeInclusive<u32> {
        if self.pl.is_some() {
            return self.col_at(sh, self.origin.x)..=self.col_at(sh, self.view.max.x);
        }
        let a = sh.col_at(self.fx + self.scroll.x).max(self.fc).min(MAX_COLS - 1);
        let b = sh.col_at(self.scroll.x + self.view.width() / self.z).min(MAX_COLS - 1);
        a..=b.max(a)
    }

    fn scroll_rows(&self, sh: &Sheet) -> std::ops::RangeInclusive<u32> {
        if self.pl.is_some() {
            return self.row_at(sh, self.origin.y)..=self.row_at(sh, self.view.max.y);
        }
        let a = sh.row_at(self.fy + self.scroll.y).max(self.fr).min(MAX_ROWS - 1);
        let b = sh.row_at(self.scroll.y + self.view.height() / self.z).min(MAX_ROWS - 1);
        a..=b.max(a)
    }

    /// The panes of frozen rows and columns, each as the geometry of a
    /// sheet without frozen parts at that pane's offset, with its screen
    /// area. What spans panes (a selection) is drawn in each, clipped.
    fn panes(&self) -> Vec<(Geo, Rect)> {
        if self.fr == 0 && self.fc == 0 {
            return vec![(self.clone(), self.view)];
        }
        let fxs = self.origin.x + self.fx * self.z;
        let fys = self.origin.y + self.fy * self.z;
        let mut out = Vec::new();
        for (fc, fr) in [(true, true), (false, true), (true, false), (false, false)] {
            if (fc && self.fc == 0) || (fr && self.fr == 0) {
                continue;
            }
            let (x0, x1) = if fc { (self.origin.x, fxs) } else if self.fc > 0 { (fxs, self.view.max.x) } else { (self.origin.x, self.view.max.x) };
            let (y0, y1) = if fr { (self.origin.y, fys) } else if self.fr > 0 { (fys, self.view.max.y) } else { (self.origin.y, self.view.max.y) };
            let clip = Rect::from_min_max(pos2(x0, y0), pos2(x1, y1)).intersect(self.view);
            if !clip.is_positive() {
                continue;
            }
            let mut g = self.clone();
            (g.fr, g.fc, g.fx, g.fy) = (0, 0, 0.0, 0.0);
            g.scroll = vec2(if fc { self.ox } else { self.scroll.x }, if fr { self.oy } else { self.scroll.y });
            g.view = clip;
            out.push((g, clip));
        }
        out
    }

    /// The rows of `scroll_rows` that are not hidden: under a filter that
    /// leaves few rows, the window spans very many hidden ones.
    fn shown_rows(&self, sh: &Sheet) -> Vec<u32> {
        let r = self.scroll_rows(sh);
        let mut rows = sh.row_heights.shown(*r.start(), *r.end());
        // The Page Layout view of print areas shows their cells alone.
        if let Some(pl) = &self.pl {
            rows.retain(|&r| pl.rows.shows(r));
        }
        rows
    }
}

/// Runs of consecutive numbers in ascending `v`, as (first, last).
fn runs(v: &[u32]) -> Vec<(u32, u32)> {
    let mut out: Vec<(u32, u32)> = Vec::new();
    for &i in v {
        match out.last_mut() {
            Some(last) if last.1 + 1 == i => last.1 = i,
            _ => out.push((i, i)),
        }
    }
    out
}

pub struct SheetView {
    pub wb: Workbook,
    pub zoom: f32,
    pub notices: Vec<Notice>,
    /// A macro a form button asks to run (the app runs it).
    pub run_macro: Option<String>,
    /// Double and right clicks on cells the sheet's macros handle.
    pub cell_hooks: macro_hooks::CellHooks,
    uid: u64,
    sheet: usize,
    anchor: CellRef,
    cursor: CellRef,
    /// Scroll offset of the unfrozen pane, in sheet units.
    scroll: Vec2,
    /// Editor text shared by the formula bar and the in-cell editor.
    buf: String,
    buf_key: Option<(usize, CellRef, u64)>,
    /// Mixed formatting of the text being edited, and the text it fits.
    edit_runs: Option<(String, apex_sheet::rich::RichText)>,
    edit: Option<Edit>,
    /// Put the caret at the end of the editor on the next frame.
    caret_to_end: bool,
    drag: Drag,
    /// Last internal copy: the TSV text and the cells with formulas.
    clip: Option<(String, Clip, Option<usize>, bool)>,
    /// The Clipboard pane is open.
    clip_pane: bool,
    /// A paste was requested from the menu; accept it without focus.
    paste_expected: bool,
    renaming: Option<(usize, String)>,
    /// The name box of a tab being renamed takes the focus once.
    rename_focus: bool,
    move_copy: Option<movecopy_ui::MoveCopyDlg>,
    views_dlg: Option<views_ui::ViewsDlg>,
    links_dlg: Option<links_ui::LinksDlg>,
    /// Data > Queries & Connections.
    queries_dlg: Option<apex_io::ExternalData>,
    /// Data > Relationships / Measures.
    model_dlg: Option<Box<model_ui::ModelDlg>>,
    /// Data > Get Data: the query editor and refreshes.
    getdata: getdata_ui::GetDataState,
    /// The folder of the workbook's file, for links to other files.
    file_dir: Option<std::path::PathBuf>,
    fill_opts: Option<autofill_ui::FillOpts>,
    paste_opts: Option<paste_opts_ui::PasteOpts>,
    quick: quick_ui::QuickState,
    name_box: String,
    ensure_visible: bool,
    focus_grid: bool,
    find: Option<FindState>,
    dialog: Option<Dialog>,
    brush: Option<Brush>,
    /// Which shared pick-up `brush` is (see `painter::sync`).
    brush_serial: u64,
    /// Last colors, border and function used by the split buttons.
    last_color: Option<u32>,
    last_fill: Option<u32>,
    last_border: BorderPreset,
    border_line: Line,
    last_autosum: &'static str,
    show_formulas: bool,
    /// View > Focus Cell: the row and column of the active cell are tinted.
    focus_cell: bool,
    /// What was right-clicked (for the context menu).
    ctx_target: Hit,
    ctx: Option<egui::Context>,
    tab_drag: Option<usize>,
    /// The cut/copy marquee is shown (Esc hides it).
    marquee: bool,
    /// Selected autocomplete entry; the text for which the list was dismissed.
    ac_index: usize,
    ac_hidden: Option<String>,
    /// Screen rect of the active editor (to place formula popups).
    editor_rect: Option<Rect>,
    /// Give the editor its focus back next frame (after clicking a reference).
    refocus: bool,
    stats: Option<((usize, Range, u64), SelectionStats)>,
    /// Geometry from the last frame (for menu actions).
    rows_per_page: u32,
    /// Filter, validation and conditional formatting state.
    data: DataState,
    outline_cache: outline_ui::OutlineCell,
    audit: audit_ui::AuditState,
    notes: notes_ui::NotesState,
    /// Accessibility checker, edit ranges and the data form.
    review: review_ui::ReviewState,
    print: print_ui::PrintState,
    images: images_ui::ImagesState,
    spell: Option<spell_ui::SpellRun>,
    cell_styles: cellstyles_ui::CellStylesState,
    /// Visible cell area in sheet units (last frame).
    view_size: (f32, f32),
    /// Screen rectangle of the (active) pane of the grid in the last frame.
    pane_rect: Rect,
    f10: f10::F10State,
    /// The last formatting or insertion, for F4 and Ctrl+Y.
    repeat: Option<commands::Repeat>,
    /// More selected areas (Ctrl+click, Go To Special), valid while the
    /// sheet and the anchor they were made with stay the same.
    extra: Vec<Range>,
    extra_key: (usize, CellRef),
    /// Read Aloud.
    speak: speak_ui::SpeakState,
    /// Planning: the Gantt beside plan tables and its dialogs.
    plan: plan_ui::PlanState,
}

impl SheetView {
    /// The cell with the cursor, for the QA.
    #[cfg(test)]
    pub fn audit_cursor(&self) -> CellRef {
        self.cursor
    }

    /// Whether a sheet tab is being renamed, for the QA.
    #[cfg(test)]
    pub fn audit_renaming(&self) -> bool {
        self.renaming.is_some()
    }

    /// The tool armed (a shape to draw, the pen), for the QA.
    #[cfg(test)]
    pub fn audit_tool(&self) -> String {
        format!("{:?} {:?}", self.images.shapes.armed().map(|t| t.kind), self.images.shapes.ink.mode)
    }

    /// What doesn't show in the accessibility tree (the tool in use, how
    /// cells show, the edits made to cells, the cell being edited, the
    /// Gantt beside a plan), for the button audit: a click that changes
    /// only this has done something.
    #[cfg(test)]
    pub fn audit_modes(&self) -> String {
        let shape = self.images.shapes.armed().map(|t| t.kind);
        let p = &self.plan;
        let gantt = format!("{} {:?} {:?} {} {} {:?}", p.hidden, p.scale, p.left, p.load, p.warnings, p.views.kind);
        format!("{:?} {shape:?} {} {} {} {} {:?} {gantt} {} {:?} {:?} {:?}", self.images.shapes.ink.mode, self.show_formulas, self.focus_cell, self.zoom, self.wb.undo_len(), self.sh().view, self.sheet, self.anchor, self.cursor, self.edit)
    }

    /// The sheet shown.
    pub fn sheet_index(&self) -> usize {
        self.sheet
    }

    pub fn new(mut wb: Workbook) -> Self {
        wb.enforce_protection = true;
        SheetView {
            wb,
            zoom: 1.0,
            notices: Vec::new(),
            run_macro: None,
            cell_hooks: Default::default(),
            uid: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            sheet: 0,
            anchor: CellRef::new(0, 0),
            cursor: CellRef::new(0, 0),
            scroll: Vec2::ZERO,
            buf: String::new(),
            buf_key: None,
            edit_runs: None,
            edit: None,
            caret_to_end: false,
            drag: Drag::None,
            clip: None,
            clip_pane: false,
            paste_expected: false,
            renaming: None,
            rename_focus: false,
            move_copy: None,
            views_dlg: None,
            links_dlg: None,
            queries_dlg: None,
            model_dlg: None,
            getdata: Default::default(),
            file_dir: None,
            fill_opts: None,
            paste_opts: None,
            quick: Default::default(),
            name_box: String::new(),
            ensure_visible: false,
            focus_grid: true,
            find: None,
            dialog: None,
            brush: None,
            brush_serial: 0,
            last_color: Some(0xFF0000),
            last_fill: Some(0xFFFF00),
            last_border: BorderPreset::Bottom,
            border_line: Line::THIN,
            last_autosum: "SUM",
            show_formulas: false,
            focus_cell: false,
            ctx_target: Hit::None,
            ctx: None,
            tab_drag: None,
            marquee: false,
            ac_index: 0,
            ac_hidden: None,
            editor_rect: None,
            refocus: false,
            stats: None,
            rows_per_page: 20,
            data: DataState::default(),
            outline_cache: Default::default(),
            audit: Default::default(),
            notes: Default::default(),
            review: Default::default(),
            print: Default::default(),
            images: Default::default(),
            cell_styles: Default::default(),
            spell: None,
            f10: Default::default(),
            repeat: None,
            extra: Vec::new(),
            extra_key: (0, CellRef::new(0, 0)),
            view_size: (1000.0, 600.0),
            pane_rect: Rect::NOTHING,
            speak: Default::default(),
            plan: Default::default(),
        }
    }

    /// What a screen reader says of the selection: the cell, what it
    /// shows and its formula; or the range selected.
    fn a11y_text(&self) -> String {
        let sel = self.sel();
        if !sel.is_single() {
            return format!("{} {}", sel_label(&sel, self.wb.calc.r1c1), tr("a11y.selected"));
        }
        let c = self.cursor;
        let shown = self.wb.display(self.sheet, c).0;
        let mut out = format!("{}: {}", c.to_a1(), if shown.is_empty() { tr("a11y.empty") } else { &shown });
        let edit = if self.wb.formula_hidden(self.sheet, c) { String::new() } else { self.wb.edit_text(self.sheet, c) };
        if edit.starts_with('=') {
            out.push_str(&format!(", {} {edit}", tr("a11y.formula")));
        }
        out
    }

    fn sel(&self) -> Range {
        Range::new(self.anchor, self.cursor)
    }

    /// Every selected area, the active one last.
    fn areas(&self) -> Vec<Range> {
        let mut v = if self.extra_key == (self.sheet, self.anchor) { self.extra.clone() } else { Vec::new() };
        v.push(self.sel());
        v
    }

    /// Selects several areas; the first one becomes active.
    fn select_areas(&mut self, areas: &[Range]) {
        let Some(first) = areas.first() else { return };
        // The active cell is the top-left one.
        self.anchor = first.end;
        self.cursor = first.start;
        self.extra = areas[1..].to_vec();
        self.extra_key = (self.sheet, self.anchor);
        self.ensure_visible = true;
    }

    fn sh(&self) -> &Sheet {
        &self.wb.sheets[self.sheet]
    }

    fn grid_id(&self) -> Id {
        Id::new(("grid", self.uid, self.f10.pane))
    }

    fn bar_id(&self) -> Id {
        Id::new(("fbar", self.uid))
    }

    fn cell_edit_id(&self) -> Id {
        Id::new(("cedit", self.uid))
    }

    /// Selects `a` to `b` (for tests driving the window).
    #[cfg(test)]
    pub(crate) fn select_range(&mut self, a: CellRef, b: CellRef) {
        self.select(a, false);
        self.select(b, true);
    }

    fn select(&mut self, c: CellRef, extend: bool) {
        if !self.wb.can_select(self.sheet, c) {
            return;
        }
        self.cursor = c;
        if !extend {
            self.anchor = c;
        }
        self.ensure_visible = true;
    }

    fn move_by(&mut self, dr: i32, dc: i32, extend: bool, jump: bool) {
        let from = self.cursor;
        let to = if jump {
            self.sh().jump(from, dr.signum(), dc.signum(), LAST)
        } else {
            CellRef::new((from.row as i64 + dr as i64).clamp(0, LAST.row as i64) as u32, (from.col as i64 + dc as i64).clamp(0, LAST.col as i64) as u32)
        };
        // Step over hidden rows and columns.
        let sh = self.sh();
        let mut to = to;
        for _ in 0..10_000 {
            let hidden_r = dr != 0 && sh.row_height(to.row) == 0.0;
            let hidden_c = dc != 0 && sh.col_width(to.col) == 0.0;
            if !hidden_r && !hidden_c {
                break;
            }
            let next = CellRef::new((to.row as i64 + dr.signum() as i64).clamp(0, LAST.row as i64) as u32, (to.col as i64 + dc.signum() as i64).clamp(0, LAST.col as i64) as u32);
            if next == to {
                to = from;
                break;
            }
            to = next;
        }
        self.select(to, extend);
    }

    pub(crate) fn select_sheet(&mut self, i: usize) {
        self.settle_edit();
        if !self.wb.group.contains(&i) {
            self.wb.group.clear();
        }
        self.sheet = i.min(self.wb.sheets.len() - 1);
        self.anchor = CellRef::new(0, 0);
        self.cursor = self.anchor;
        self.scroll = Vec2::ZERO;
        self.buf_key = None;
        self.data.popup = None;
        self.data.list = None;
    }

    // --- editing ------------------------------------------------------------

    fn begin_edit(&mut self, ctx: &egui::Context, origin: EditOrigin, typed: bool, text: Option<String>) {
        if !self.guard_edit() {
            return;
        }
        self.edit_runs = None;
        if let Some(t) = text {
            self.buf = t;
        } else {
            self.buf = self.wb.edit_text(self.sheet, self.cursor);
            if let Some(runs) = self.wb.cell(self.sheet, self.cursor).and_then(|c| c.rich()) {
                self.edit_runs = Some((self.buf.clone(), runs.clone()));
            }
        }
        self.edit = Some(Edit { origin, typed });
        self.caret_to_end = true;
        let id = if origin == EditOrigin::Cell { self.cell_edit_id() } else { self.bar_id() };
        ctx.memory_mut(|m| m.request_focus(id));
    }

    /// Commits the editor into the cursor cell (Enter, a click on another
    /// cell, saving). Returns false if the formula is invalid: the editor
    /// stays open and says why.
    pub fn commit_edit(&mut self) -> bool {
        self.commit_edit_as(true)
    }

    /// Commits the editor before a command (a toolstrip button, another
    /// tab or sheet, the focus going elsewhere). A formula still being
    /// written (`=SUM(` from AutoSum) isn't an error there: the editor
    /// stays open, as in other spreadsheets, for Enter to say what's wrong.
    pub(super) fn settle_edit(&mut self) -> bool {
        self.commit_edit_as(false)
    }

    fn commit_edit_as(&mut self, explain: bool) -> bool {
        if self.edit.is_none() {
            return true;
        }
        if let Err(e) = self.wb.check_formula(&self.buf) {
            if explain {
                self.notices.push(Notice::refused(trf("sheet.bad_formula", &[("error", &e)])));
            }
            return false;
        }
        if let Some(full) = self.value_completion() {
            self.buf = full;
        }
        if let Some(fixed) = crate::autocorrect::correct_text(&self.buf) {
            self.buf = fixed;
        }
        let old = self.wb.edit_text(self.sheet, self.cursor);
        let runs = self.current_runs();
        let old_runs = self.wb.cell(self.sheet, self.cursor).and_then(|c| c.rich()).cloned();
        if old != self.buf || runs != old_runs {
            if runs.is_some() || old_runs.is_some() {
                self.wb.set_rich_input(self.sheet, self.cursor, &self.buf.clone(), runs);
            } else {
                self.wb.set_input(self.sheet, self.cursor, &self.buf.clone());
            }
            self.table_follow_up(self.cursor);
            self.check_validation(self.cursor, self.buf.clone());
        }
        self.edit = None;
        self.buf_key = None;
        self.focus_grid = true;
        true
    }

    /// The runs of mixed formatting following the edits made so far.
    fn current_runs(&mut self) -> Option<apex_sheet::rich::RichText> {
        let (text, runs) = self.edit_runs.take()?;
        if self.buf.starts_with('=') {
            return None;
        }
        let runs = if text == self.buf { Some(runs) } else { apex_sheet::rich::follow_edit(&runs, &text, &self.buf) };
        let runs = runs.filter(|r| !r.is_empty());
        self.edit_runs = runs.clone().map(|r| (self.buf.clone(), r));
        runs
    }

    /// Bold, italic... on the selected part of the text being edited:
    /// true when it applied (a text, not a formula, with a selection).
    fn toggle_in_editor(&mut self, get: fn(&CellStyle) -> bool, set: fn(&mut CellStyle, bool)) -> bool {
        if self.edit.is_none() || self.buf.starts_with('=') {
            return false;
        }
        let Some(ctx) = self.ctx.clone() else { return false };
        let id = if ctx.memory(|m| m.has_focus(self.bar_id())) { self.bar_id() } else { self.cell_edit_id() };
        let Some(range) = egui::text_edit::TextEditState::load(&ctx, id).and_then(|s| s.cursor.char_range()) else { return false };
        let (a, b) = (range.primary.index.0.min(range.secondary.index.0), range.primary.index.0.max(range.secondary.index.0));
        if a == b {
            return false;
        }
        let byte = |k: usize| self.buf.char_indices().nth(k).map_or(self.buf.len(), |x| x.0);
        let (from, to) = (byte(a), byte(b));
        let runs = self.current_runs().unwrap_or_default();
        let st = self.style_at_cursor();
        // Which flag `set` changes, and its value at the selection start.
        let mut t = st.clone();
        set(&mut t, !get(&st));
        let which = (t.bold != st.bold, t.italic != st.italic, t.underline != st.underline);
        let font = apex_sheet::rich::font_at(&runs, from);
        let cur = match which {
            (true, _, _) => font.bold,
            (_, true, _) => font.italic,
            (_, _, true) => font.underline,
            _ => font.strike,
        };
        let on = !cur.unwrap_or(get(&st));
        let new = apex_sheet::rich::apply(&runs, self.buf.len(), from, to, |f| match which {
            (true, _, _) => f.bold = Some(on),
            (_, true, _) => f.italic = Some(on),
            (_, _, true) => f.underline = Some(on),
            _ => f.strike = Some(on),
        });
        self.edit_runs = Some((self.buf.clone(), new.unwrap_or_default()));
        true
    }

    fn cancel_edit(&mut self) {
        self.edit_runs = None;
        self.edit = None;
        self.buf_key = None;
        self.focus_grid = true;
    }

    /// Keys that end or navigate out of an edit; handled before the text
    /// widgets see them.
    fn editor_keys(&mut self, ui: &egui::Ui) {
        let Some(edit) = self.edit else { return };
        let focused = ui.memory(|m| m.has_focus(self.bar_id()) || m.has_focus(self.cell_edit_id()));
        if !focused {
            return;
        }
        let consume = |k: Key, m: Modifiers| ui.input_mut(|i| i.consume_key(m, k));
        if consume(Key::Escape, Modifiers::NONE) {
            self.cancel_edit();
            return;
        }
        // F3 pastes a name into the formula.
        if self.buf.starts_with('=') && consume(Key::F3, Modifiers::NONE) {
            self.open_paste_name();
            return;
        }
        // F4 makes the reference at the caret absolute, mixed or relative.
        if self.buf.starts_with('=') && consume(Key::F4, Modifiers::NONE) {
            let id = self.editor_id(ui.ctx());
            let at = egui::text_edit::TextEditState::load(ui.ctx(), id).and_then(|s| s.cursor.char_range()).map_or(self.buf.chars().count(), |r| r.primary.index.0);
            if let Some((text, pos)) = apex_sheet::addr::toggle_absolute(&self.buf, at) {
                self.buf = text;
                set_caret(ui.ctx(), id, pos);
            }
            return;
        }
        // Ctrl+; and Ctrl+Shift+: type the date and the time.
        for (k, time) in [(Key::Semicolon, false), (Key::Colon, true)] {
            if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, k) || i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, k)) {
                let t = self.now_text(time);
                self.insert_in_editor(ui.ctx(), &t);
                return;
            }
        }
        // Ctrl+Enter puts the entry in every selected cell.
        if consume(Key::Enter, Modifiers::COMMAND) {
            let (sel, cur) = (self.sel(), self.cursor);
            let n = self.wb.undo_len();
            if self.commit_edit() && !sel.is_single() {
                self.wb.fill_from(self.sheet, &sel, cur);
                let steps = self.wb.undo_len().saturating_sub(n);
                self.wb.merge_undo(steps, "undo.fill");
            }
            return;
        }
        // Backspace or Delete drop the suggested completion first.
        if self.value_completion().is_some() && (consume(Key::Backspace, Modifiers::NONE) || consume(Key::Delete, Modifiers::NONE)) {
            self.ac_hidden = Some(self.buf.clone());
            return;
        }
        // Bold, italic, underline and strikethrough on the selected text.
        if !self.buf.starts_with('=') {
            for (k, get, set) in [
                (Key::B, (|s: &CellStyle| s.bold) as fn(&CellStyle) -> bool, (|s: &mut CellStyle, v| s.bold = v) as fn(&mut CellStyle, bool)),
                (Key::I, |s| s.italic, |s, v| s.italic = v),
                (Key::U, |s| s.underline, |s, v| s.underline = v),
                (Key::Num5, |s| s.strike, |s, v| s.strike = v),
            ] {
                if consume(k, Modifiers::COMMAND) {
                    self.toggle_in_editor(get, set);
                    return;
                }
            }
        }
        // Alt+Enter inserts a line break in the cell.
        if consume(Key::Enter, Modifiers::ALT) {
            self.buf.push('\n');
            self.caret_to_end = true;
            return;
        }
        let mv = if consume(Key::Enter, Modifiers::SHIFT) {
            Some((-1, 0))
        } else if consume(Key::Enter, Modifiers::NONE) {
            Some((1, 0))
        } else if consume(Key::Tab, Modifiers::SHIFT) {
            Some((0, -1))
        } else if consume(Key::Tab, Modifiers::NONE) {
            Some((0, 1))
        } else if edit.typed && edit.origin == EditOrigin::Cell && !self.formula_ref_point() {
            if consume(Key::ArrowDown, Modifiers::NONE) {
                Some((1, 0))
            } else if consume(Key::ArrowUp, Modifiers::NONE) {
                Some((-1, 0))
            } else if consume(Key::ArrowLeft, Modifiers::NONE) {
                Some((0, -1))
            } else if consume(Key::ArrowRight, Modifiers::NONE) {
                Some((0, 1))
            } else {
                None
            }
        } else {
            None
        };
        if let Some((dr, dc)) = mv {
            if self.commit_edit() {
                self.move_by(dr, dc, false, false);
            }
        }
    }

    /// The user is pointing at cells to build a formula: losing the editor
    /// focus must not commit.
    fn pointing(&self) -> bool {
        let over_popup = self.ctx.as_ref().is_some_and(|c| c.pointer_latest_pos().is_some_and(|p| c.layer_id_at(p).is_some_and(|l| l.id == Id::new(("autocomplete", self.uid)))));
        self.refocus || over_popup || matches!(self.drag, Drag::FormulaRef { .. }) || self.formula_ref_point()
    }

    /// True when the formula being typed can take a cell reference next.
    fn formula_ref_point(&self) -> bool {
        self.edit.is_some() && self.buf.starts_with('=') && self.buf.trim_end().ends_with(['=', '+', '-', '*', '/', '^', '(', ';', ',', ':', '<', '>', '&'])
    }

    // --- clipboard ------------------------------------------------------------

    pub(crate) fn copy(&mut self, ctx: &egui::Context, cut: bool) {
        if self.image_selected() {
            self.copy_image(ctx, cut);
            return;
        }
        let sel = self.sel();
        let r = self.wb.clamp_to_used(self.sheet, &sel).map(|u| Range::new(sel.start, u.end)).unwrap_or(Range::single(sel.start));
        if r.cell_count() > 5_000_000 {
            self.notices.push(Notice::error(tr("sheet.too_big_copy")));
            return;
        }
        let mut text = String::new();
        for row in r.start.row..=r.end.row {
            for col in r.start.col..=r.end.col {
                if col > r.start.col {
                    text.push('\t');
                }
                let (s, _) = self.wb.display(self.sheet, CellRef::new(row, col));
                if s.contains(['\t', '\n', '"']) {
                    text.push('"');
                    text.push_str(&s.replace('"', "\"\""));
                    text.push('"');
                } else {
                    text.push_str(&s);
                }
            }
            text.push('\n');
        }
        let mut clip = self.wb.copy(self.sheet, &r);
        // A saved workbook's cells can be linked to from another one.
        clip.source = self.file_dir.as_ref().map(|d| apex_sheet::ClipSource {
            file: d.join(&self.print.file_name).to_string_lossy().into_owned(),
            sheet: self.sh().name.to_string(),
        });
        self.to_system_clipboard(ctx, &text, &r, &clip);
        crate::clip_history::push(crate::clip_history::Entry { text: text.clone(), rich: crate::clip_history::Rich::Cells(std::sync::Arc::new(clip.clone())) });
        self.clip = Some((text, clip, Some(self.sheet), cut));
        self.marquee = true;
    }

    /// Puts copied cells on the system clipboard as text, as an HTML table
    /// other programs paste formatted, and as the cells themselves for
    /// other ApexDesk processes. Large copies go as text only.
    fn to_system_clipboard(&self, ctx: &egui::Context, text: &str, r: &Range, clip: &apex_sheet::Clip) {
        use crate::sysclip;
        let mut formats: Vec<(&str, &[u8])> = vec![(sysclip::TEXT, text.as_bytes())];
        let rich = (r.cell_count() <= RICH_COPY_CELLS).then(|| (apex_io::cells_clip::html(&self.wb, self.sheet, r), apex_io::cells_clip::xlsx(&self.wb, self.sheet, clip)));
        if let Some((html, xlsx)) = &rich {
            formats.push((sysclip::HTML, html.as_bytes()));
            formats.push((sysclip::CELLS, xlsx));
        }
        if !sysclip::write(&formats) {
            ctx.copy_text(text.to_string());
        }
    }

    /// The cells another program or another ApexDesk process put on the
    /// system clipboard with `text`: ApexDesk's own, else an HTML table
    /// (Excel, LibreOffice, Google Sheets, web pages) that holds all of
    /// the text.
    fn system_clip(&mut self, text: &str) -> Option<apex_sheet::Clip> {
        use crate::sysclip;
        let (name, bytes) = sysclip::read(&[sysclip::CELLS, sysclip::HTML])?;
        // Whatever another program put there: a reader's slip is no paste,
        // not the end of the program.
        fn guard<T>(f: impl FnOnce() -> Option<T>) -> Option<T> {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).ok().flatten()
        }
        if name == sysclip::CELLS
            && let Some(c) = guard(|| apex_io::cells_clip::read_xlsx(&bytes))
        {
            return Some(self.wb.adopt_clip(&c));
        }
        let html = if name == sysclip::HTML { bytes } else { sysclip::read(&[sysclip::HTML])?.1 };
        let t = guard(|| apex_io::cells_clip::read_html(&sysclip::text_of(&html)))?;
        let lines = parse_tsv(text).iter().filter(|r| r.iter().any(|c| !c.trim().is_empty())).count();
        if lines > t.rows as usize {
            return None;
        }
        Some(self.wb.clip_from_inputs(t.rows, t.cols, t.cells, t.col_widths))
    }

    /// Takes as the clip the cells copied in another workbook of ours:
    /// the ones that go with the clipboard's `text`, or with `None` the
    /// last thing copied in any window. Their source sheet isn't here.
    fn take_foreign_clip(&mut self, text: Option<&str>) {
        use crate::clip_history::Rich;
        let entry = match text {
            Some(t) => crate::clip_history::find(t),
            None => crate::clip_history::newest(),
        };
        let Some(e) = entry else { return };
        if self.clip.as_ref().is_some_and(|(t, ..)| *t == e.text) {
            return;
        }
        if let Rich::Cells(c) = &e.rich {
            self.clip = Some((e.text, self.wb.adopt_clip(c), None, false));
        }
    }

    fn paste(&mut self, text: &str) {
        // An empty clipboard pastes nothing (it must not clear cells).
        if text.is_empty() {
            return;
        }
        let target = self.sel();
        // Copied in another program (what we copied stays as it is).
        crate::clip_history::push(crate::clip_history::Entry { text: text.to_string(), rich: crate::clip_history::Rich::Text });
        self.take_foreign_clip(Some(text));
        if !self.clip.as_ref().is_some_and(|(t, ..)| t == text)
            && let Some(clip) = self.system_clip(text)
        {
            self.clip = Some((text.to_string(), clip, None, false));
        }
        if let Some((t, clip, src_sheet, cut)) = self.clip.clone() {
            if t == text {
                let before = self.wb.undo_len();
                self.wb.paste(self.sheet, &target, &clip);
                if let Some(src_sheet) = src_sheet.filter(|_| cut) {
                    // Clear the source cells that were not overwritten.
                    let src = Range::new(clip.origin, CellRef::new(clip.origin.row + clip.rows - 1, clip.origin.col + clip.cols - 1));
                    let dst = Range::new(target.start, CellRef::new(target.start.row + clip.rows - 1, target.start.col + clip.cols - 1));
                    let mut cells = Vec::new();
                    for row in src.start.row..=src.end.row {
                        for col in src.start.col..=src.end.col {
                            let c = CellRef::new(row, col);
                            if (src_sheet != self.sheet || !dst.contains(c)) && self.wb.cell(src_sheet, c).is_some() {
                                cells.push((c, None));
                            }
                        }
                    }
                    if !cells.is_empty() {
                        self.wb.execute(Op::SetCells { sheet: src_sheet, cells });
                    }
                    self.clip = None;
                }
                let end = CellRef::new((target.start.row + clip.rows - 1).max(target.end.row), (target.start.col + clip.cols - 1).max(target.end.col));
                self.anchor = target.start;
                self.cursor = CellRef::new(end.row.min(LAST.row), end.col.min(LAST.col));
                if !cut {
                    self.pasted_cells(before);
                }
                return;
            }
        }
        // External text: tab-separated values.
        let rows = parse_tsv(text);
        let mut inputs = Vec::new();
        for (dr, row) in rows.iter().enumerate() {
            for (dc, v) in row.iter().enumerate() {
                let c = CellRef::new(target.start.row + dr as u32, target.start.col + dc as u32);
                if c.row <= LAST.row && c.col <= LAST.col {
                    inputs.push((c, v.clone()));
                }
            }
        }
        if !inputs.is_empty() {
            let last = inputs.last().unwrap().0;
            let maxc = inputs.iter().map(|(c, _)| c.col).max().unwrap();
            self.wb.set_inputs(self.sheet, inputs);
            self.table_follow_up(target.start);
            self.anchor = target.start;
            self.cursor = CellRef::new(last.row, maxc);
        }
    }

    // --- commands ---------------------------------------------------------------

    fn style_at_cursor(&self) -> CellStyle {
        if let Some(st) = self.shape_text_style() {
            return st;
        }
        let id = self.wb.cell(self.sheet, self.cursor).map_or(0, |c| c.style);
        self.wb.styles.get(id).clone()
    }

    fn set_style(&mut self, f: impl Fn(&mut CellStyle) + 'static) {
        if self.shape_text_format(&f) {
            return;
        }
        let f: std::rc::Rc<dyn Fn(&mut CellStyle)> = std::rc::Rc::new(f);
        self.repeat = Some(commands::Repeat::Style(f.clone()));
        self.settle_edit();
        self.each_area("undo.format", |v, r| {
            let f = f.clone();
            v.wb.modify_style(v.sheet, r, move |s| f(s));
        });
    }

    /// Runs `f` on every selected area as one undo step.
    fn each_area(&mut self, label: &'static str, mut f: impl FnMut(&mut Self, &Range)) {
        let n = self.wb.undo_len();
        for r in self.areas() {
            f(self, &r);
        }
        let steps = self.wb.undo_len().saturating_sub(n);
        self.wb.merge_undo(steps, label);
    }

    fn toggle(&mut self, get: fn(&CellStyle) -> bool, set: fn(&mut CellStyle, bool)) {
        if let Some(st) = self.shape_text_style() {
            let on = !get(&st);
            self.shape_text_format(&move |s| set(s, on));
            return;
        }
        if self.toggle_in_editor(get, set) {
            return;
        }
        let on = !get(&self.style_at_cursor());
        self.set_style(move |s| set(s, on));
    }

    pub fn undo(&mut self) {
        self.cancel_edit();
        self.wb.undo();
        self.clamp_sheet();
    }

    pub fn redo(&mut self) {
        self.cancel_edit();
        self.wb.redo();
        self.clamp_sheet();
    }

    /// File > Inspect File: removes (or shows) what the inspector found
    /// of `what`, as one undo step.
    pub fn inspect_clean(&mut self, what: apex_io::inspect::Hidden, show: bool) -> bool {
        self.cancel_edit();
        let done = apex_io::inspect::clean_workbook(&mut self.wb, what, show);
        self.clamp_sheet();
        done
    }

    /// Check Before Sharing's Apply Brand: the organization's fonts and
    /// colors in place of the others, as one undo step.
    pub fn apply_brand(&mut self, b: &crate::brand::Brand) -> bool {
        self.cancel_edit();
        let done = self.wb.apply_brand(&b.fonts, &b.colors);
        self.buf_key = None;
        done
    }

    fn clamp_sheet(&mut self) {
        if self.sheet >= self.wb.sheets.len() {
            self.select_sheet(self.wb.sheets.len() - 1);
        }
        self.buf_key = None;
    }

    fn whole_cols(&self) -> bool {
        let s = self.sel();
        s.start.row == 0 && s.end.row == LAST.row
    }

    fn whole_rows(&self) -> bool {
        let s = self.sel();
        s.start.col == 0 && s.end.col == LAST.col
    }

    fn delete_rows(&mut self) {
        self.settle_edit();
        if self.ask_before_delete(origin_ui::Deletion::Rows) {
            return;
        }
        let s = self.sel();
        self.wb.execute(Op::DeleteRows { sheet: self.sheet, at: s.start.row, count: s.rows() });
    }

    fn delete_cols(&mut self) {
        self.settle_edit();
        if self.ask_before_delete(origin_ui::Deletion::Cols) {
            return;
        }
        let s = self.sel();
        self.wb.execute(Op::DeleteCols { sheet: self.sheet, at: s.start.col, count: s.cols() });
    }

    fn formula_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let nb_id = Id::new(("namebox", self.uid));
            let nb_focused = ui.memory(|m| m.has_focus(nb_id));
            if !nb_focused {
                self.name_box = self.name_box_label();
            }
            let mut nb_out = egui::TextEdit::singleline(&mut self.name_box).id(nb_id).desired_width(90.0).font(egui::TextStyle::Monospace).show(ui);
            let nb = nb_out.response.clone();
            if nb.gained_focus() {
                // Like Excel: a click selects the whole address, so typing
                // replaces it with a reference or a new name.
                let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(self.name_box.chars().count()));
                nb_out.state.cursor.set_char_range(Some(all));
                nb_out.state.store(ui.ctx(), nb_id);
            }
            if nb.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                let t = self.name_box.clone();
                if !self.name_box_name(&t) {
                    self.goto(&t);
                }
                self.focus_grid = true;
            }
            if widgets::tool_button(ui, widgets::Face::Icon(Icon::Function), false, &format!("{} (Shift+F3)", tr("fx.title"))).clicked() {
                self.open_functions();
            }
            let id = self.bar_id();
            if self.caret_to_end && self.edit.is_some_and(|e| e.origin == EditOrigin::Bar) {
                set_caret_end(ui.ctx(), id, &self.buf);
            }
            if self.edit.is_none() && self.buf.len() > HUGE_TEXT {
                // Laying out megabytes every frame would stall the UI: show
                // the start until the user actually edits the cell.
                let end = self.buf.char_indices().nth(1000).map_or(self.buf.len(), |x| x.0);
                let preview = format!("{}…", self.buf[..end].replace('\n', " "));
                let resp = ui.add(egui::Label::new(RichText::new(preview).monospace()).truncate().sense(Sense::click()));
                if resp.clicked() {
                    self.edit = Some(Edit { origin: EditOrigin::Bar, typed: false });
                    ui.memory_mut(|m| m.request_focus(id));
                }
                return;
            }
            let multiline = self.buf.contains('\n');
            let te = if multiline {
                egui::TextEdit::multiline(&mut self.buf).desired_rows(2)
            } else {
                egui::TextEdit::singleline(&mut self.buf)
            };
            let mono = egui::TextStyle::Monospace.resolve(ui.style());
            let color = ui.visuals().text_color();
            let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap: f32| ui.fonts_mut(|f| f.layout_job(formula::formula_job(text.as_str(), mono.clone(), color, wrap)));
            let resp = ui.add(te.id(id).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace).lock_focus(true).event_filter(EDITOR_KEYS).layouter(&mut layouter));
            if self.edit.is_some_and(|e| e.origin == EditOrigin::Bar) {
                self.editor_rect = Some(resp.rect);
            }
            if resp.gained_focus() && self.edit.is_none() {
                self.edit = Some(Edit { origin: EditOrigin::Bar, typed: false });
            }
            if resp.changed() && self.edit.is_none() {
                self.edit = Some(Edit { origin: EditOrigin::Bar, typed: false });
            }
            if resp.lost_focus() && self.edit.is_some_and(|e| e.origin == EditOrigin::Bar) && !self.pointing() {
                // Clicking elsewhere commits, like other spreadsheets.
                self.settle_edit();
            }
        });
    }

    // --- grid -------------------------------------------------------------------

    fn geo(&self, rect: Rect) -> Geo {
        let z = self.zoom;
        let sh = self.sh();
        // A pane of a split may go without headers and scroll bars.
        let ch = self.f10.split.chrome;
        let hdr_w = if ch.row_hdr { row_header_width(sh, self.scroll.y, rect.height() / z) * z } else { 0.0 };
        let origin = rect.min + vec2(hdr_w, if ch.col_hdr { HEADER_H * z } else { 0.0 });
        let view = Rect::from_min_max(origin, rect.max - vec2(if ch.vbar { SCROLLBAR } else { 0.0 }, if ch.hbar { SCROLLBAR } else { 0.0 }));
        // The Page Layout view has no frozen panes.
        let pl = (sh.view == ViewMode::PageLayout).then(|| self.page_geo(self.scroll.x + view.width() / z, self.scroll.y + view.height() / z));
        let (fr, fc) = if pl.is_some() { (0, 0) } else { (sh.frozen_rows, sh.frozen_cols) };
        let top = frozen_top(sh, fr, fc);
        let (ox, oy) = (sh.col_x(top.col), sh.row_y(top.row));
        let (fx, fy) = (sh.col_x(fc) - ox, sh.row_y(fr) - oy);
        // Next to frozen panes the scrolled pane starts at a whole column
        // and row, as in Excel: a cut one beside the frozen ones misleads.
        let mut scroll = self.scroll.max(vec2(ox, oy));
        if fc > 0 {
            scroll.x = sh.col_x(sh.col_at(fx + scroll.x)) - fx;
        }
        if fr > 0 {
            scroll.y = sh.row_y(sh.row_at(fy + scroll.y)) - fy;
        }
        let mut g = Geo { origin, view, z, fr, fc, fx, fy, top, ox, oy, scroll, xs: AxisCache::default(), ys: AxisCache::default(), pl };
        let (cols, rows) = (g.scroll_cols(sh), g.scroll_rows(sh));
        // A few extra rows/columns cover cells that spill or are partly visible.
        let cols = *cols.start()..=(*cols.end() + 2).min(MAX_COLS - 1);
        let rows = *rows.start()..=(*rows.end() + 2).min(MAX_ROWS - 1);
        g.xs = AxisCache::build(fc, cols, |c| sh.col_x(c), |c| sh.col_width(c));
        g.ys = AxisCache::build(fr, rows, |r| sh.row_y(r), |r| sh.row_height(r));
        g
    }

    /// Zoom to Selection: the zoom that fits the selected cells in the
    /// window, scrolled to them.
    pub(super) fn zoom_to_selection(&mut self) {
        let sh = &self.wb.sheets[self.sheet];
        let r = self.sel();
        let (w, h) = (sh.col_x(r.end.col + 1) - sh.col_x(r.start.col), sh.row_y(r.end.row + 1) - sh.row_y(r.start.row));
        // The headers grow with the zoom too.
        let (hw, hh) = (row_header_width(sh, self.scroll.y, self.view_size.1), HEADER_H);
        let (vw, vh) = ((self.view_size.0 + hw) * self.zoom, (self.view_size.1 + hh) * self.zoom);
        if w <= 0.0 || h <= 0.0 || self.view_size.0 <= 0.0 || self.view_size.1 <= 0.0 {
            return;
        }
        self.zoom = (vw / (w + hw)).min(vh / (h + hh)).clamp(0.25, 4.0);
        if sh.view == ViewMode::PageLayout {
            self.ensure_visible = true;
            return;
        }
        let (fx, fy) = frozen_size(sh);
        if r.start.col >= sh.frozen_cols {
            self.scroll.x = sh.col_x(r.start.col) - fx;
        }
        if r.start.row >= sh.frozen_rows {
            self.scroll.y = sh.row_y(r.start.row) - fy;
        }
    }

    fn scroll_to_cursor(&mut self, g: &Geo) {
        let sh = &self.wb.sheets[self.sheet];
        let c = self.cursor;
        let vw = g.view.width() / g.z;
        let vh = g.view.height() / g.z;
        // Frozen panes scroll by whole columns and rows (see `geo`): past
        // the far edge, go on to the next whole one so the cursor shows.
        if c.col >= g.fc {
            let (x, w) = (g.ux(sh, c.col), sh.col_width(c.col));
            if x < g.fx + g.scroll.x {
                self.scroll.x = x - g.fx;
            } else if x + w > g.scroll.x + vw {
                let s = x + w - vw;
                let s = if g.fc > 0 { ceil_to(s, g.fx, |v| sh.col_at(v), |i| sh.col_x(i)) } else { s };
                self.scroll.x = s.min(x - g.fx);
            }
        }
        if c.row >= g.fr {
            let (y, h) = (g.uy(sh, c.row), sh.row_height(c.row));
            if y < g.fy + g.scroll.y {
                self.scroll.y = y - g.fy;
            } else if y + h > g.scroll.y + vh {
                let s = y + h - vh;
                let s = if g.fr > 0 { ceil_to(s, g.fy, |v| sh.row_at(v), |i| sh.row_y(i)) } else { s };
                self.scroll.y = s.min(y - g.fy);
            }
        }
        self.scroll = self.scroll.max(Vec2::ZERO);
    }

    fn hit(&self, g: &Geo, p: Pos2, rect: Rect) -> Hit {
        let sh = self.sh();
        if p.x < g.origin.x && p.y < g.origin.y {
            return Hit::Corner;
        }
        if p.y < g.origin.y && p.x >= g.origin.x && p.x < g.view.max.x {
            let c = g.col_at(sh, p.x);
            let left = g.x(sh, c);
            let right = g.x_end(sh, c);
            // Wider for a finger.
            let near = if cfg!(target_os = "android") { 10.0 } else { 4.0 };
            if (right - p.x).abs() <= near {
                return Hit::ColBorder(c);
            }
            if (p.x - left).abs() <= near && c > 0 {
                return Hit::ColBorder(c - 1);
            }
            return Hit::ColHeader(c);
        }
        if p.x < g.origin.x && p.y >= g.origin.y && p.y < g.view.max.y && p.x >= rect.min.x {
            let r = g.row_at(sh, p.y);
            let top = g.y(sh, r);
            let bottom = g.y_end(sh, r);
            let near = if cfg!(target_os = "android") { 7.0 } else { 3.0 };
            if (bottom - p.y).abs() <= near {
                return Hit::RowBorder(r);
            }
            if (p.y - top).abs() <= near && r > 0 {
                return Hit::RowBorder(r - 1);
            }
            return Hit::RowHeader(r);
        }
        if g.view.contains(p) {
            let sel = self.sel();
            if self.edit.is_none() && sel.end.row < LAST.row && sel.end.col < LAST.col {
                let sr = g.range_rect(sh, &sel);
                if (p - sr.max).length() <= 5.0 {
                    return Hit::FillHandle;
                }
            }
            return Hit::Cell(CellRef::new(g.row_at(sh, p.y), g.col_at(sh, p.x)));
        }
        Hit::None
    }

    fn grid(&mut self, ui: &mut egui::Ui) {
        if !self.extra.is_empty() && self.extra_key != (self.sheet, self.anchor) {
            self.extra.clear();
        }
        let full = ui.available_rect_before_wrap();
        if self.split_grid(ui, full) {
            return;
        }
        // Outline bars take the left and top edges.
        let rect = Rect::from_min_max(full.min + self.outline_size(), full.max);
        self.grid_pane(ui, full, rect, 0, true);
    }

    /// One pane of the grid; only the active pane takes the pointer and
    /// the keyboard (the others scroll with the wheel).
    fn grid_pane(&mut self, ui: &mut egui::Ui, full: Rect, rect: Rect, pane: u8, active: bool) {
        let id = Id::new(("grid", self.uid, pane));
        let resp = ui.interact(rect, id, Sense::click_and_drag());
        let mut g = self.geo(rect);
        if !active {
            if resp.hovered() {
                let delta = ui.input(|i| i.smooth_scroll_delta);
                if delta != Vec2::ZERO {
                    self.scroll = (self.scroll - delta / g.z).max(Vec2::ZERO);
                    g = self.geo(rect);
                }
            }
            self.paint(&ui.painter_at(rect), &g, rect, ui.visuals());
            self.scrollbars(ui, &g, rect);
            return;
        }
        ui.memory_mut(|m| m.set_focus_lock_filter(id, EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true }));
        if widgets::a11y_on(ui.ctx()) {
            widgets::a11y_canvas(ui.ctx(), &resp, &trf("a11y.sheet", &[("name", &self.wb.sheets[self.sheet].name)]));
            widgets::announce(ui.ctx(), &self.a11y_text());
        }
        if self.focus_grid {
            resp.request_focus();
            self.focus_grid = false;
        }

        self.pointer(ui, &resp, &g, rect);
        if !self.notes_busy() && self.print.setup.is_none() && ui.memory(|m| m.allows_interaction(ui.layer_id())) && (resp.has_focus() || (self.paste_expected && ui.memory(|m| m.focused().is_none_or(|f| f == id)))) {
            self.keyboard(ui, &g);
        }
        // Wheel scrolling and ctrl+wheel zoom.
        if resp.hovered() {
            let (delta, zoom) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
            if zoom != 1.0 {
                self.zoom = (self.zoom * zoom).clamp(0.25, 4.0);
            } else if delta != Vec2::ZERO {
                self.scroll = (self.scroll - delta / g.z).max(Vec2::ZERO);
            }
        }
        // Frozen panes: rows and columns scrolled away above them stay away.
        self.scroll = self.scroll.max(vec2(g.ox, g.oy));
        if self.ensure_visible {
            g = self.geo(rect);
            self.scroll_to_cursor(&g);
            self.ensure_visible = false;
        }
        g = self.geo(rect);
        if g.pl.is_none() {
            self.note_rows_geo(g.origin, g.view, g.z, g.scroll.y, g.fr, g.fy, g.oy);
        }
        self.rows_per_page = ((g.view.height() / g.z) / self.sh().default_row_height).max(1.0) as u32;
        self.view_size = (g.view.width() / g.z, g.view.height() / g.z);
        self.pane_rect = rect;
        let cr = g.cell_rect(self.sh(), self.cursor);
        self.data.cursor_rect = (cr.width() > 0.5 && cr.height() > 0.5 && g.view.intersects(cr)).then_some(cr);

        let painter = ui.painter_at(rect);
        self.paint(&painter, &g, rect, ui.visuals());
        if !self.split_on() {
            self.outline_bars(ui, &g, full, rect);
        }
        self.scrollbars(ui, &g, rect);
        self.cell_editor(ui, &g);
        self.shape_editor(ui, &g);
        if active {
            self.slicers_ui(ui, &g);
            self.fill_options_button(ui, &g);
            self.paste_options_button(ui, &g);
            self.quick_button(ui, &g);
        }
    }

    fn pointer(&mut self, ui: &mut egui::Ui, resp: &egui::Response, g: &Geo, rect: Rect) {
        self.cell_hooks_frame(ui.ctx());
        let (pos, pressed, down, released, mods) =
            ui.input(|i| (i.pointer.interact_pos(), i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.primary_released(), i.modifiers));
        let Some(p) = pos else {
            // The context menu stays open with the pointer out of the window.
            resp.context_menu(|ui| self.context_menu(ui));
            return;
        };
        // The Auto Fill Options button (or its menu) takes the click.
        if self.drag == Drag::None && (self.fill_opts.as_ref().is_some_and(|f| f.button.is_some_and(|b| b.contains(p))) || self.paste_opts.as_ref().is_some_and(|o| o.button.is_some_and(|b| b.contains(p))) || self.quick.button.is_some_and(|b| b.contains(p))) {
            return;
        }
        if self.drag == Drag::None && self.image_pointer(ui, resp, g, p, pressed, down, mods) {
            return;
        }
        let hit = self.hit(g, p, rect);
        if resp.hovered() && self.drag == Drag::None && !self.image_hovered() {
            match hit {
                Hit::ColBorder(_) => ui.ctx().set_cursor_icon(CursorIcon::ResizeColumn),
                Hit::RowBorder(_) => ui.ctx().set_cursor_icon(CursorIcon::ResizeRow),
                Hit::FillHandle => ui.ctx().set_cursor_icon(CursorIcon::Crosshair),
                Hit::Cell(_) if self.brush.is_some() => ui.ctx().set_cursor_icon(CursorIcon::Copy),
                Hit::Cell(c) if self.edit.is_none() && self.on_checkbox(g, c, p) => ui.ctx().set_cursor_icon(CursorIcon::PointingHand),
                Hit::Cell(_) => ui.ctx().set_cursor_icon(CursorIcon::Cell),
                _ => {}
            }
        }
        if pressed && resp.hovered() {
            let bar_y = g.view.max.y;
            let bar_x = g.view.max.x;
            if p.y >= bar_y && p.x < bar_x {
                self.drag = Drag::ScrollX { grab: p.x };
                return;
            }
            if p.x >= bar_x && p.y < bar_y {
                self.drag = Drag::ScrollY { grab: p.y };
                return;
            }
            if let (Hit::Cell(c), true) = (hit, self.edit.is_none()) {
                if self.notes_click(c, mods) {
                    resp.request_focus();
                    return;
                }
            }
            if self.edit.is_none() && (self.audit_click(p) || self.data_click(g, p)) {
                resp.request_focus();
                return;
            }
            // A click on a check box toggles it.
            if let (Hit::Cell(c), true) = (hit, self.edit.is_none() && !mods.shift && !mods.command) {
                if self.checkbox_click(g, c, p) {
                    resp.request_focus();
                    return;
                }
            }
            // Typing a formula: clicking cells inserts references.
            if let Hit::Cell(c) = hit {
                if self.formula_ref_point() {
                    let at = self.buf.len();
                    self.buf.push_str(&self.point_ref(Range::single(c)));
                    self.drag = Drag::FormulaRef { at, anchor: c };
                    self.caret_to_end = true;
                    return;
                }
            }
            if self.edit.is_some() && !self.commit_edit() {
                return;
            }
            resp.request_focus();
            // Ctrl+click keeps the selection and starts another area.
            let keep = mods.command && !mods.shift && matches!(hit, Hit::Cell(_) | Hit::ColHeader(_) | Hit::RowHeader(_));
            let before = if keep { self.areas() } else { Vec::new() };
            match hit {
                Hit::Cell(c) => {
                    self.select(c, mods.shift);
                    self.drag = Drag::Cells;
                }
                Hit::ColHeader(c) => {
                    if mods.shift {
                        self.cursor = CellRef::new(LAST.row, c);
                    } else {
                        self.anchor = CellRef::new(0, c);
                        self.cursor = CellRef::new(LAST.row, c);
                    }
                    self.drag = Drag::Cols;
                }
                Hit::RowHeader(r) => {
                    if mods.shift {
                        self.cursor = CellRef::new(r, LAST.col);
                    } else {
                        self.anchor = CellRef::new(r, 0);
                        self.cursor = CellRef::new(r, LAST.col);
                    }
                    self.drag = Drag::Rows;
                }
                Hit::Corner => {
                    self.anchor = CellRef::new(0, 0);
                    self.cursor = LAST;
                }
                Hit::ColBorder(c) => {
                    let sh = self.sh();
                    self.drag = Drag::ResizeCol { col: c, orig: sh.col_widths.get(&c).copied(), start_w: sh.col_width(c), start_x: p.x };
                }
                Hit::RowBorder(r) => {
                    let sh = self.sh();
                    self.drag = Drag::ResizeRow { row: r, orig: sh.row_heights.get(&r).copied(), start_h: sh.row_height(r), start_y: p.y };
                }
                Hit::FillHandle => self.drag = Drag::Fill { target: self.sel() },
                Hit::None => {}
            }
            if keep {
                self.extra = before;
                self.extra_key = (self.sheet, self.anchor);
            }
        }
        if resp.double_clicked() {
            match hit {
                // In a pivot table: Show Details of the value.
                Hit::Cell(c) if c == self.cursor && self.wb.pivot_at(self.sheet, c).is_some() && self.show_pivot_details() => {}
                // BeforeDoubleClick first; it may cancel the editing.
                Hit::Cell(c) if c == self.cursor && self.cell_hooks.double => self.cell_hooks.clicked = Some((false, self.sheet, Range::single(c))),
                Hit::Cell(c) if c == self.cursor => self.begin_edit(ui.ctx(), EditOrigin::Cell, false, None),
                Hit::ColBorder(c) => self.autofit_cols(ui, c, c),
                _ => {}
            }
        }
        if resp.secondary_clicked() && !self.image_menu_hit(g, p) {
            self.ctx_target = hit;
            let sel = self.sel();
            match hit {
                Hit::Cell(c) if !sel.contains(c) => self.select(c, false),
                Hit::ColHeader(c) if !(self.whole_cols() && c >= sel.start.col && c <= sel.end.col) => {
                    self.anchor = CellRef::new(0, c);
                    self.cursor = CellRef::new(LAST.row, c);
                }
                Hit::RowHeader(r) if !(self.whole_rows() && r >= sel.start.row && r <= sel.end.row) => {
                    self.anchor = CellRef::new(r, 0);
                    self.cursor = CellRef::new(r, LAST.col);
                }
                _ => {}
            }
            // BeforeRightClick: it may close the menu.
            if self.cell_hooks.right && matches!(hit, Hit::Cell(_)) {
                self.cell_hooks.clicked = Some((true, self.sheet, self.sel()));
            }
        }
        resp.context_menu(|ui| self.context_menu(ui));

        if down {
            // Autoscroll while dragging a selection past the edges.
            let auto = matches!(self.drag, Drag::Cells | Drag::Fill { .. } | Drag::FormulaRef { .. } | Drag::Cols | Drag::Rows);
            if auto {
                let mut d = Vec2::ZERO;
                if p.x > g.view.max.x {
                    d.x = (p.x - g.view.max.x).min(60.0);
                } else if p.x < g.origin.x && !matches!(self.drag, Drag::Rows) && self.scroll.x > 0.0 {
                    d.x = -(g.origin.x - p.x).min(60.0);
                }
                if p.y > g.view.max.y {
                    d.y = (p.y - g.view.max.y).min(60.0);
                } else if p.y < g.origin.y && !matches!(self.drag, Drag::Cols) && self.scroll.y > 0.0 {
                    d.y = -(g.origin.y - p.y).min(60.0);
                }
                if d != Vec2::ZERO {
                    self.scroll = (self.scroll + d / g.z * 0.5).max(Vec2::ZERO);
                    ui.ctx().request_repaint();
                }
            }
            // min before max: a pane smaller than a pixel must not panic.
            let cp = pos2(p.x.min(g.view.max.x - 1.0).max(g.origin.x), p.y.min(g.view.max.y - 1.0).max(g.origin.y));
            let sh = self.sh();
            let cell = CellRef::new(g.row_at(sh, cp.y), g.col_at(sh, cp.x));
            match self.drag {
                Drag::Cells => self.cursor = cell,
                Drag::Cols => self.cursor = CellRef::new(LAST.row, cell.col),
                Drag::Rows => self.cursor = CellRef::new(cell.row, LAST.col),
                Drag::ResizeCol { col, start_w, start_x, .. } => {
                    let w = (start_w + (p.x - start_x) / g.z).max(4.0);
                    self.wb.sheets[self.sheet].col_widths.insert(col, w);
                }
                Drag::ResizeRow { row, start_h, start_y, .. } => {
                    let h = (start_h + (p.y - start_y) / g.z).max(4.0);
                    self.wb.sheets[self.sheet].row_heights.insert(row, h);
                }
                Drag::Fill { .. } => {
                    let s = self.sel();
                    let down_rows = cell.row.saturating_sub(s.end.row);
                    let right_cols = cell.col.saturating_sub(s.end.col);
                    let target = if down_rows >= right_cols {
                        Range::new(s.start, CellRef::new(s.end.row.max(cell.row), s.end.col))
                    } else {
                        Range::new(s.start, CellRef::new(s.end.row, s.end.col.max(cell.col)))
                    };
                    self.drag = Drag::Fill { target };
                }
                Drag::FormulaRef { at, anchor } => {
                    self.buf.truncate(at);
                    let r = Range::new(anchor, cell);
                    self.buf.push_str(&self.point_ref(r));
                    self.caret_to_end = true;
                }
                Drag::ScrollX { grab } => {
                    let (total, _) = self.scroll_extent(g);
                    self.scroll.x = (self.scroll.x + (p.x - grab) * total / g.view.width().max(1.0)).max(0.0);
                    self.drag = Drag::ScrollX { grab: p.x };
                }
                Drag::ScrollY { grab } => {
                    let (_, total) = self.scroll_extent(g);
                    self.scroll.y = (self.scroll.y + (p.y - grab) * total / (g.view.height().max(1.0))).max(0.0);
                    self.drag = Drag::ScrollY { grab: p.y };
                }
                Drag::None => {}
            }
        }
        if released || (!down && self.drag != Drag::None) {
            match self.drag {
                Drag::ResizeCol { col, orig, .. } => {
                    let sh = &mut self.wb.sheets[self.sheet];
                    let w = sh.col_width(col);
                    match orig {
                        Some(o) => sh.col_widths.insert(col, o),
                        None => sh.col_widths.remove(&col),
                    };
                    let sel = self.sel();
                    let cols: Vec<u32> = if self.whole_cols() && sel.contains(CellRef::new(0, col)) { (sel.start.col..=sel.end.col).collect() } else { vec![col] };
                    self.wb.execute(Op::ColWidths { sheet: self.sheet, sizes: cols.into_iter().map(|c| (c, Some(w))).collect() });
                }
                Drag::ResizeRow { row, orig, .. } => {
                    let sh = &mut self.wb.sheets[self.sheet];
                    let h = sh.row_height(row);
                    match orig {
                        Some(o) => sh.row_heights.insert(row, o),
                        None => sh.row_heights.remove(&row),
                    };
                    let sel = self.sel();
                    let rows: Vec<u32> = if self.whole_rows() && sel.contains(CellRef::new(row, 0)) { (sel.start.row..=sel.end.row).collect() } else { vec![row] };
                    self.wb.execute(Op::RowHeights { sheet: self.sheet, sizes: rows.into_iter().map(|r| (r, Some(h))).collect() });
                }
                Drag::Fill { target } => {
                    let s = self.sel();
                    if target != s {
                        self.drag_fill(s, target);
                        self.anchor = target.start;
                        self.cursor = target.end;
                    }
                }
                Drag::FormulaRef { .. } => self.refocus = true,
                Drag::Cells | Drag::Cols | Drag::Rows if self.brush.is_some() => self.apply_brush(),
                _ => {}
            }
            self.drag = Drag::None;
        }
    }

    fn keyboard(&mut self, ui: &mut egui::Ui, g: &Geo) {
        if self.edit.is_some() || self.list_keys(ui) || self.image_keys(ui) {
            return;
        }
        let (events, mods) = ui.input(|i| (i.events.clone(), i.modifiers));
        let mut started_typing = false;
        for ev in events {
            match ev {
                // Ctrl+Shift+C copies formatting (the format painter).
                Event::Copy if mods.shift => self.start_brush(false),
                Event::Copy => self.copy(ui.ctx(), false),
                Event::Cut => self.copy(ui.ctx(), true),
                // Ctrl+Alt+Shift+V pastes formatting; Ctrl+Shift+V or
                // Ctrl+Alt+V open paste special.
                Event::Paste(_) if mods.shift && mods.alt => {
                    self.paste_expected = false;
                    self.apply_brush();
                }
                Event::Paste(_) if (mods.shift || mods.alt) && !self.paste_expected => {
                    self.dialog = Some(Dialog::PasteSpecial(PasteSpecial::default()));
                }
                Event::Paste(t) => {
                    self.paste_expected = false;
                    self.paste(&t);
                }
                // Space on a check box checks it instead of typing (the
                // key's text is dropped with the rest).
                Event::Key { key: Key::Space, pressed: true, modifiers: m, .. } if !started_typing && !m.any() && self.checkbox_space() => started_typing = true,
                Event::Text(t) if !started_typing && !t.chars().all(char::is_control) && !mods.command => {
                    started_typing = true;
                    self.begin_edit(ui.ctx(), EditOrigin::Cell, true, Some(t));
                }
                Event::Key { key, pressed: true, modifiers: m, .. } if !started_typing => self.key(ui, key, m, g),
                _ => {}
            }
        }
        if started_typing {
            // Keep the typed text from reaching the editor twice.
            ui.input_mut(|i| i.events.retain(|e| !matches!(e, Event::Text(_))));
        }
    }

    fn key(&mut self, ui: &egui::Ui, key: Key, m: Modifiers, _g: &Geo) {
        if self.f10_key(key, m) {
            return;
        }
        let cmd = m.command;
        let page = self.rows_per_page as i32;
        match key {
            Key::ArrowDown if m.alt => self.open_list(),
            Key::ArrowRight if m.alt && m.shift => self.group_cmd(false),
            Key::ArrowLeft if m.alt && m.shift => self.group_cmd(true),
            Key::ArrowDown => self.move_by(1, 0, m.shift, cmd),
            Key::ArrowUp => self.move_by(-1, 0, m.shift, cmd),
            Key::ArrowRight => self.move_by(0, 1, m.shift, cmd),
            Key::ArrowLeft => self.move_by(0, -1, m.shift, cmd),
            Key::PageDown if cmd => self.select_sheet_step(1),
            Key::PageUp if cmd => self.select_sheet_step(-1),
            Key::PageDown => self.move_by(page, 0, m.shift, false),
            Key::PageUp => self.move_by(-page, 0, m.shift, false),
            Key::Home if cmd => self.select(CellRef::new(0, 0), m.shift),
            Key::Home => self.select(CellRef::new(self.cursor.row, 0), m.shift),
            Key::End if cmd => {
                let e = self.sh().extent().unwrap_or_default();
                self.select(e, m.shift);
            }
            Key::Enter | Key::Tab => {
                let (dr, dc) = match (key, m.shift) {
                    (Key::Enter, false) => (1, 0),
                    (Key::Enter, true) => (-1, 0),
                    (_, false) => (0, 1),
                    (_, true) => (0, -1),
                };
                self.move_by(dr, dc, false, false);
            }
            Key::F2 if m.shift => self.edit_note(),
            Key::M if cmd && m.alt => self.new_comment(),
            Key::F7 if !m.command && !m.shift => self.start_spelling(),
            Key::F2 => self.begin_edit(ui.ctx(), EditOrigin::Cell, false, None),
            Key::K if cmd => self.open_link_dialog(),
            Key::F3 if m.shift && cmd => self.open_create_names(),
            Key::F3 if m.shift => self.open_functions(),
            Key::F3 if cmd => self.open_name_manager(),
            Key::F3 => self.open_paste_name(),
            Key::F9 => self.calculate_now(),
            Key::F5 => self.dialog = Some(Dialog::GoTo(String::new())),
            Key::Delete => self.each_area("undo.clear", |v, r| v.wb.clear_contents(v.sheet, r)),
            Key::Backspace => {
                self.wb.clear_contents(self.sheet, &Range::single(self.cursor));
                self.begin_edit(ui.ctx(), EditOrigin::Cell, true, Some(String::new()));
            }
            Key::Escape => {
                // Esc drops the cut/copy marquee and the format painter.
                if let Some(c) = &mut self.clip {
                    c.3 = false;
                }
                self.marquee = false;
                self.brush = None;
            }
            Key::Space if cmd && m.shift => {
                self.anchor = CellRef::new(0, 0);
                self.cursor = LAST;
            }
            Key::Space if cmd => {
                self.anchor = CellRef::new(0, self.cursor.col);
                self.cursor = CellRef::new(LAST.row, self.cursor.col);
            }
            Key::Space if m.shift => {
                self.anchor = CellRef::new(self.cursor.row, 0);
                self.cursor = CellRef::new(self.cursor.row, LAST.col);
            }
            Key::A if cmd => {
                self.anchor = CellRef::new(0, 0);
                self.cursor = LAST;
            }
            Key::B if cmd => self.toggle(|s| s.bold, |s, v| s.bold = v),
            Key::I if cmd => self.toggle(|s| s.italic, |s, v| s.italic = v),
            Key::U if cmd => self.toggle(|s| s.underline, |s, v| s.underline = v),
            Key::Num5 if cmd && m.shift => self.set_num_fmt("0%"),
            Key::Num5 if cmd => self.toggle(|s| s.strike, |s, v| s.strike = v),
            Key::Num1 if cmd => self.open_format_dialog(0),
            Key::Num9 if cmd && m.shift => {
                let s = self.sel();
                self.hide(true, false, &s);
            }
            Key::Num9 if cmd => {
                let s = self.sel();
                self.hide(true, true, &s);
            }
            Key::Num0 if cmd && m.shift => {
                let s = self.sel();
                self.hide(false, false, &s);
            }
            Key::D if cmd => self.fill(FillDir::Down),
            Key::R if cmd => self.fill(FillDir::Right),
            Key::L if cmd && m.shift => self.toggle_filter(),
            Key::F if cmd => self.open_find(false),
            Key::H if cmd => self.open_find(true),
            Key::G if cmd => self.dialog = Some(Dialog::GoTo(String::new())),
            Key::Backtick if cmd => self.show_formulas = !self.show_formulas,
            Key::Equals if m.alt => self.auto_sum("SUM"),
            Key::Plus | Key::Equals if cmd && m.shift => self.dialog = Some(Dialog::Cells { delete: false, choice: 1 }),
            Key::Minus if cmd => self.dialog = Some(Dialog::Cells { delete: true, choice: 1 }),
            // Ctrl+Shift+> / < and Ctrl+] / [ resize the font.
            Key::Period if cmd && m.shift => self.grow_font(true),
            Key::Comma if cmd && m.shift => self.grow_font(false),
            Key::CloseBracket if cmd => self.grow_font(true),
            Key::OpenBracket if cmd => self.grow_font(false),
            Key::Z if cmd && m.shift => self.redo(),
            Key::Z if cmd => self.undo(),
            // Ctrl+Y repeats the last action when there's nothing to redo.
            Key::Y if cmd && !self.wb.can_redo() => self.repeat_last(),
            Key::Y if cmd => self.redo(),
            Key::F4 if m.is_none() => self.repeat_last(),
            // Ctrl+; today's date, Ctrl+Shift+: the time.
            Key::Semicolon if cmd => {
                let t = self.now_text(false);
                self.begin_edit(ui.ctx(), EditOrigin::Cell, true, Some(t));
            }
            Key::Colon if cmd => {
                let t = self.now_text(true);
                self.begin_edit(ui.ctx(), EditOrigin::Cell, true, Some(t));
            }
            _ => {}
        }
    }

    fn select_sheet_step(&mut self, d: i32) {
        let n = self.wb.sheets.len() as i32;
        let mut i = self.sheet as i32;
        for _ in 0..n {
            i = (i + d).rem_euclid(n);
            if !self.wb.sheets[i as usize].hidden {
                self.select_sheet(i as usize);
                return;
            }
        }
    }

    fn scroll_extent(&self, g: &Geo) -> (f32, f32) {
        let sh = self.sh();
        let ext = sh.extent().unwrap_or_default();
        let w = g.ux(sh, ext.col.max(self.cursor.col.min(MAX_COLS - 2)) + 2) - g.fx;
        let h = g.uy(sh, ext.row.max(self.cursor.row.min(MAX_ROWS - 2)) + 2) - g.fy;
        let vw = g.view.width() / g.z;
        let vh = g.view.height() / g.z;
        ((w + vw * 0.5).max(self.scroll.x + vw), (h + vh * 0.5).max(self.scroll.y + vh))
    }

    fn scrollbars(&mut self, ui: &egui::Ui, g: &Geo, rect: Rect) {
        let (tw, th) = self.scroll_extent(g);
        let p = ui.painter();
        let v = ui.visuals();
        let track = if v.dark_mode { Color32::from_gray(35) } else { Color32::from_gray(240) };
        let thumb = if v.dark_mode { Color32::from_gray(90) } else { Color32::from_gray(185) };
        let hbar = Rect::from_min_max(pos2(rect.min.x, g.view.max.y), pos2(g.view.max.x, rect.max.y));
        let vbar = Rect::from_min_max(pos2(g.view.max.x, rect.min.y), pos2(rect.max.x, g.view.max.y));
        let (show_h, show_v) = (hbar.height() > 1.0, vbar.width() > 1.0);
        p.rect_filled(hbar, 0.0, track);
        p.rect_filled(vbar, 0.0, track);
        p.rect_filled(Rect::from_min_max(g.view.max, rect.max), 0.0, track);
        let vw = g.view.width() / g.z;
        let vh = g.view.height() / g.z;
        let len_x = (vw / tw).clamp(0.05, 1.0) * g.view.width();
        let pos_x = g.origin.x + (self.scroll.x / tw) * g.view.width();
        let len_y = (vh / th).clamp(0.05, 1.0) * g.view.height();
        let pos_y = g.origin.y + (self.scroll.y / th) * g.view.height();
        if show_h {
            p.rect_filled(Rect::from_min_size(pos2(pos_x.min(g.view.max.x - len_x), hbar.min.y + 3.0), vec2(len_x.min(g.view.width()), SCROLLBAR - 6.0)), 3.0, thumb);
        }
        if show_v {
            p.rect_filled(Rect::from_min_size(pos2(vbar.min.x + 3.0, pos_y.min(g.view.max.y - len_y)), vec2(SCROLLBAR - 6.0, len_y.min(g.view.height()))), 3.0, thumb);
        }
    }

    fn cell_editor(&mut self, ui: &mut egui::Ui, g: &Geo) {
        let Some(edit) = self.edit else { return };
        let sh = &self.wb.sheets[self.sheet];
        let cr = g.cell_rect(sh, self.cursor);
        if edit.origin != EditOrigin::Cell {
            // Preview what is being typed in the formula bar.
            return;
        }
        let st = self.style_at_cursor();
        let size = style_pt(&st) * PT * g.z;
        let font = fonts::font_for(st.font.as_deref(), st.bold, st.italic, size).0;
        let lines = self.buf.lines().count().max(1) as f32;
        let text_w = ui.fonts_mut(|f| self.buf.lines().map(|l| f.layout_no_wrap(l.to_string(), font.clone(), Color32::BLACK).size().x).fold(0.0, f32::max));
        let w = (text_w + 16.0).max(cr.width()).min(g.view.max.x - cr.min.x).max(cr.width());
        let h = (size * 1.25 * lines + 6.0).max(cr.height());
        let rect = Rect::from_min_size(cr.min, vec2(w, h));
        let id = self.cell_edit_id();
        if self.caret_to_end {
            set_caret_end(ui.ctx(), id, &self.buf);
            self.caret_to_end = false;
        }
        ui.painter().rect_filled(rect.expand(1.0), 0.0, Paper::BG);
        self.editor_rect = Some(rect);
        let lf = font.clone();
        let ghost_font = font.clone();
        let runs = self.current_runs();
        let base = self.buf.clone();
        let z = g.z;
        let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap: f32| {
            let text = text.as_str();
            // Text with mixed formatting shows it while typing.
            if let Some(runs) = runs.as_ref().and_then(|r| if text == base { Some(r.clone()) } else { apex_sheet::rich::follow_edit(r, &base, text) }) {
                return ui.fonts_mut(|f| f.layout_job(rich_job(text, &runs, &st, Paper::TEXT, z, Some(wrap))));
            }
            ui.fonts_mut(|f| f.layout_job(formula::formula_job(text, lf.clone(), Paper::TEXT, wrap)))
        };
        let te = egui::TextEdit::multiline(&mut self.buf)
            .id(id)
            .font(font)
            .layouter(&mut layouter)
            .text_color(Paper::TEXT)
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::symmetric(3, 2))
            .desired_width(w)
            .lock_focus(true)
            .event_filter(EDITOR_KEYS)
            .background_color(Paper::BG);
        // As `ui.put`, with selected text that can be dragged.
        let drag = crate::edit_drag::begin(ui, id);
        let builder = egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::centered_and_justified(egui::Direction::TopDown));
        let out = ui.scope_builder(builder, |ui| te.interactive(!drag.grabbing()).show(ui)).inner;
        drag.finish(ui, &out, &mut self.buf);
        let resp = out.response.response;
        // The column value that would complete the text, shown selected.
        if let Some(full) = self.value_completion() {
            let rest: String = full.chars().skip(self.buf.chars().count()).collect();
            let (typed, extra) = ui.fonts_mut(|f| (f.layout_no_wrap(self.buf.clone(), ghost_font.clone(), Paper::TEXT).size().x, f.layout_no_wrap(rest.clone(), ghost_font.clone(), Paper::TEXT)));
            let at = pos2(rect.min.x + 3.0 + typed, rect.min.y + 2.0);
            ui.painter().rect_filled(Rect::from_min_size(at, extra.size()), 0.0, Paper::TEXT_SELECTION);
            ui.painter().galley(at, extra, Paper::TEXT);
        }
        ui.painter().rect_stroke(rect.expand(1.0), 0.0, Stroke::new(2.0, theme::SHEET_ACCENT), StrokeKind::Outside);
        if !resp.has_focus() && !self.pointing() && !ui.memory(|m| m.has_focus(self.bar_id())) {
            // Focus went elsewhere (another widget or window): commit.
            if ui.memory(|m| m.focused().is_some()) || resp.lost_focus() {
                self.settle_edit();
            } else {
                resp.request_focus();
            }
        }
    }

    // --- painting ---------------------------------------------------------------

    fn paint(&self, painter: &egui::Painter, g: &Geo, rect: Rect, visuals: &egui::Visuals) {
        let sh = self.sh();
        // The cells go dark with the dark workspace (screen only).
        let mark = theme::canvas_mark(painter);
        painter.rect_filled(g.view, 0.0, Paper::BG);
        self.paint_background(painter, g);
        // No more than a screen could show: a file may freeze a million.
        let frozen_cols: Vec<u32> = (g.top.col..g.fc).take(4096).collect();
        let frozen_rows: Vec<u32> = (g.top.row..g.fr).take(4096).collect();
        let scroll_cols: Vec<u32> = g.scroll_cols(sh).filter(|&c| g.pl.as_ref().is_none_or(|pl| pl.cols.shows(c))).collect();
        let scroll_rows: Vec<u32> = g.shown_rows(sh);
        let fx = g.origin.x + g.fx * g.z;
        let fy = g.origin.y + g.fy * g.z;
        let panes = [
            (&frozen_rows, &frozen_cols, Rect::from_min_max(g.origin, pos2(fx, fy))),
            (&frozen_rows, &scroll_cols, Rect::from_min_max(pos2(fx, g.origin.y), pos2(g.view.max.x, fy))),
            (&scroll_rows, &frozen_cols, Rect::from_min_max(pos2(g.origin.x, fy), pos2(fx, g.view.max.y))),
            (&scroll_rows, &scroll_cols, Rect::from_min_max(pos2(fx, fy), g.view.max)),
        ];
        for (rows, cols, clip) in panes {
            if rows.is_empty() || cols.is_empty() || clip.width() <= 0.0 || clip.height() <= 0.0 {
                continue;
            }
            let p = painter.with_clip_rect(clip.intersect(g.view));
            self.paint_pane(&p, g, rows, cols, clip, true, false);
        }
        // The marks of the cells go with them, under the pictures and
        // charts; the selection and its fill handle go over them.
        self.paint_issue_flags(painter, g);
        self.paint_note_marks(painter, g);
        self.paint_images(painter, g);
        self.paint_page_layout(painter, g, visuals.dark_mode);
        theme::darken_since(painter, mark);
        // Freeze lines.
        let line = Stroke::new(1.5, Color32::from_gray(150));
        if g.fc > 0 {
            painter.vline(fx, g.origin.y..=g.view.max.y, line);
        }
        if g.fr > 0 {
            painter.hline(g.origin.x..=g.view.max.x, fy, line);
        }
        self.paint_page_breaks(painter, g);
        self.paint_selection(painter, g);
        self.paint_data_overlays(painter, g);
        self.paint_table_overlays(painter, g);
        self.paint_audit(painter, g);
        self.paint_notes(painter, g);
        self.paint_image_handles(painter, g);
        self.paint_headers(painter, g, rect, visuals, &frozen_cols, &scroll_cols, &frozen_rows, &scroll_rows);
    }

    /// `wait`: decode pictures in cells now (printing).
    #[allow(clippy::too_many_arguments)]
    fn paint_pane(&self, p: &egui::Painter, g: &Geo, rows: &[u32], cols: &[u32], clip: Rect, grid_lines: bool, wait: bool) {
        let sh = self.sh();
        let (Some(&c0), Some(&c1)) = (cols.first(), cols.last()) else { return };
        let (Some(&r0), Some(&r1)) = (rows.first(), rows.last()) else { return };
        let x0 = g.x(sh, c0);
        let x1 = g.x_end(sh, c1).min(clip.max.x);
        let y0 = g.y(sh, r0);
        let y1 = g.y_end(sh, r1).min(clip.max.y);
        let grid = Stroke::new(1.0, Paper::GRID);
        let visible = Range::new(CellRef::new(r0, c0), CellRef::new(r1, c1));
        let merges: Vec<&Range> = sh.merges.iter().filter(|m| m.intersect(&visible).is_some()).collect();
        // The cells of the shown rows only: a filter can hide most of the
        // rows between the first and the last.
        let bands: Vec<Range> = runs(rows).into_iter().map(|(a, b)| Range::new(CellRef::new(a, c0), CellRef::new(b, c1))).collect();
        let each = |f: &mut dyn FnMut(CellRef, &apex_sheet::Cell)| {
            for band in &bands {
                sh.for_each_in(band, &mut *f);
            }
        };

        // Fills
        self.paint_table_fills(p, g, &visible);
        each(&mut |c, cell| {
            let st = self.wb.styles.get(cell.style);
            if let Some(f) = st.fill {
                p.rect_filled(g.cell_rect(sh, c), 0.0, theme::rgb(f));
            }
        });
        let cf = self.cf_map(&visible);
        self.paint_cf(p, g, &cf);
        // Grid lines (skipped over filled cells would need per-cell work; the
        // fill is drawn first and lines on top, like most spreadsheets).
        if grid_lines {
            for &c in cols {
                let x = g.x_end(sh, c).round() - 0.5;
                p.vline(x, y0..=y1, grid);
            }
            for &r in rows {
                let y = g.y_end(sh, r).round() - 0.5;
                p.hline(x0..=x1, y, grid);
            }
        }
        self.paint_table_lines(p, g, &visible);
        // Merged areas cover the grid lines inside them.
        for m in &merges {
            let rr = g.range_rect(sh, m);
            let fill = sh.get(m.start).and_then(|c| self.wb.styles.get(c.style).fill).map_or(Paper::BG, theme::rgb);
            p.rect_filled(rr.shrink(0.5), 0.0, fill);
            if grid_lines {
                p.rect_stroke(rr, 0.0, grid, StrokeKind::Inside);
            }
        }
        // Text, then borders on top.
        let mut borders: Vec<(Pos2, Pos2, Line)> = Vec::new();
        each(&mut |c, cell| {
            let st = self.wb.styles.get(cell.style);
            let table_st = self.table_text_style(c, cell.style);
            let st = table_st.as_ref().unwrap_or(st);
            let merge = merges.iter().find(|m| m.contains(c));
            if merge.is_some_and(|m| m.start != c) {
                return;
            }
            let rect = match merge {
                Some(m) => g.range_rect(sh, m),
                None => g.cell_rect(sh, c),
            };
            if rect.width() < 0.5 || rect.height() < 0.5 {
                return;
            }
            let b = &st.borders;
            if !b.is_empty() {
                for (l, a, e) in [(b.top, rect.left_top(), rect.right_top()), (b.bottom, rect.left_bottom(), rect.right_bottom()), (b.left, rect.left_top(), rect.left_bottom()), (b.right, rect.right_top(), rect.right_bottom())] {
                    if !l.is_none() {
                        borders.push((a, e, l));
                    }
                }
            }
            let value = cell.value();
            let formula = self.show_formulas && cell.formula().is_some();
            if !self.show_formulas {
                if let Some(pic) = cell.picture() {
                    incell_ui::paint_cell_picture(p, rect, pic, wait);
                    return;
                }
                if st.checkbox && matches!(value, Value::Bool(_) | Value::Empty) {
                    incell_ui::paint_checkbox(p, rect, st, matches!(value, Value::Bool(true)), g.z);
                    return;
                }
            }
            if value.is_empty() && !formula {
                return;
            }
            let cfr = cf.get(&c);
            if cfr.is_some_and(|r| r.hide_value) && !formula {
                return;
            }
            let cf_st;
            let st = match cfr {
                Some(r) if r.style.color.is_some() || r.style.bold.is_some() || r.style.italic.is_some() || r.style.underline.is_some() || r.style.strike.is_some() => {
                    cf_st = cf_ui::cf_cell_style(st, &r.style);
                    &cf_st
                }
                _ => st,
            };
            let (text, color) = if formula { (self.wb.edit_text(self.sheet, c), None) } else { self.wb.display_marked(self.sheet, c) };
            if text.is_empty() {
                return;
            }
            let color = cfr.and_then(|r| r.style.color).or(color).or(st.color).map_or(Paper::TEXT, theme::rgb);
            if let Some((left, right)) = text.split_once(apex_sheet::FILL).filter(|_| st.rotation == 0 && !st.wrap) {
                // Accounting: the symbol at the left edge, the number at the right.
                let pad = 3.0 * g.z;
                let (gl, gr) = (p.layout_job(cell_job(left, st, color, g.z, None)), p.layout_job(cell_job(right, st, color, g.z, None)));
                let y = |h: f32| match st.valign {
                    VAlign::Top => rect.min.y + 1.0,
                    VAlign::Center => rect.center().y - h / 2.0,
                    VAlign::Bottom => rect.max.y - h - 1.0,
                };
                let cp = p.with_clip_rect(rect.intersect(clip));
                if gl.size().x + gr.size().x > rect.width() - 2.0 * pad {
                    let n = ((rect.width() - 2.0 * pad) / (6.5 * g.z)).max(1.0) as usize;
                    let gh = p.layout_job(cell_job(&"#".repeat(n), st, color, g.z, None));
                    cp.galley(pos2(rect.max.x - pad - gh.size().x, y(gh.size().y)), gh, color);
                    return;
                }
                cp.galley(pos2(rect.min.x + pad, y(gl.size().y)), gl, color);
                cp.galley(pos2(rect.max.x - pad - gr.size().x, y(gr.size().y)), gr, color);
                return;
            }
            let text = if text.contains(apex_sheet::FILL) { text.replace(apex_sheet::FILL, "") } else { text };
            let halign = match st.halign {
                _ if formula => HAlign::Left,
                HAlign::General => match value {
                    Value::Number(_) => HAlign::Right,
                    Value::Bool(_) | Value::Error(_) => HAlign::Center,
                    _ => HAlign::Left,
                },
                h => h,
            };
            let pad = 3.0 * g.z;
            if st.rotation != 0 {
                paint_rotated(p, rect.intersect(clip), rect, &text, st, color, halign, g.z);
                return;
            }
            let indent = st.indent as f32 * INDENT_PX * g.z;
            let justify = halign == HAlign::Justify;
            let wrap = (st.wrap || justify).then_some(rect.width() - 2.0 * pad - indent);
            let room = if wrap.is_some() { rect.size() } else { vec2(g.view.width(), rect.height()) };
            let rich = cell.rich().filter(|_| !formula && matches!(value, Value::Text(t) if **t == *text));
            let shown = if rich.is_some() { &text } else { visible_part(&text, room, cell_px(st, g.z), wrap.is_some(), halign) };
            let mut job = match rich {
                Some(runs) => rich_job(shown, runs, st, color, g.z, wrap),
                None => cell_job(shown, st, color, g.z, wrap),
            };
            job.justify = justify;
            let mut galley = p.layout_job(job);
            let mut clip_rect = rect;
            if galley.size().x > rect.width() - 2.0 * pad - indent && wrap.is_none() {
                if let (Value::Number(v), false) = (value, formula) {
                    // General numbers drop decimals to fit; otherwise ### like
                    // other spreadsheets (numbers never overflow).
                    let room = rect.width() - 2.0 * pad - indent;
                    let mut fitted = None;
                    if self.wb.styles.format(cell.style).is_general() && v.abs() >= 1e-4 && v.abs() < 1e11 && v.fract() != 0.0 {
                        let dec = self.wb.locale.decimal.to_string();
                        let shown = text.rsplit_once(self.wb.locale.decimal).map_or(0, |(_, f)| f.len());
                        for d in (0..shown).rev() {
                            let s = format!("{:.*}", d, v);
                            let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').replace('.', &dec) } else { s };
                            let gl = p.layout_job(cell_job(&s, st, color, g.z, None));
                            if gl.size().x <= room {
                                fitted = Some(gl);
                                break;
                            }
                        }
                    }
                    galley = fitted.unwrap_or_else(|| {
                        let n = ((rect.width() - 2.0 * pad) / (6.5 * g.z)).max(1.0) as usize;
                        p.layout_job(cell_job(&"#".repeat(n), st, color, g.z, None))
                    });
                } else if halign == HAlign::Left && merge.is_none() {
                    // Text spills into empty neighbours on the right.
                    let mut end = c.col;
                    let need = rect.min.x + indent + galley.size().x + 2.0 * pad;
                    while g.x_end(sh, end) < need && end + 1 <= c1 + 64 && end + 1 < MAX_COLS {
                        let next = CellRef::new(c.row, end + 1);
                        if sh.get(next).is_some_and(|x| !x.value().is_empty()) {
                            break;
                        }
                        end += 1;
                    }
                    if end > c.col {
                        clip_rect.max.x = g.x_end(sh, end);
                    }
                }
            }
            let size = galley.size();
            let x = match halign {
                HAlign::Right => rect.max.x - pad - size.x - indent,
                HAlign::Center => rect.center().x - size.x / 2.0,
                _ => rect.min.x + pad + indent,
            };
            let y = match st.valign {
                VAlign::Top => rect.min.y + 1.0,
                VAlign::Center => rect.center().y - size.y / 2.0,
                VAlign::Bottom => rect.max.y - size.y - 1.0,
            };
            if clip_rect.max.x > rect.max.x {
                // Hide the grid lines under the spilled text.
                p.rect_filled(Rect::from_min_max(pos2(rect.max.x - 1.0, rect.min.y + 0.5), pos2(clip_rect.max.x - 1.0, rect.max.y - 1.0)), 0.0, Paper::BG);
            }
            let at = pos2(x, y.max(rect.min.y));
            // Text inside its cell keeps the pane's clip: a clip per cell
            // costs a draw call per cell.
            if clip_rect.contains_rect(Rect::from_min_size(at, size)) {
                p.galley(at, galley, color);
            } else {
                p.with_clip_rect(clip_rect.intersect(clip)).galley(at, galley, color);
            }
        });
        for (a, e, l) in borders {
            paint_line(p, a, e, l, l.color.map_or(Color32::BLACK, theme::rgb), false);
        }
    }

    /// The selection, in each frozen pane (clipped to it).
    fn paint_selection(&self, painter: &egui::Painter, g: &Geo) {
        for (pg, clip) in g.panes() {
            self.paint_selection_in(&painter.with_clip_rect(clip), &pg);
        }
    }

    fn paint_selection_in(&self, p: &egui::Painter, g: &Geo) {
        let sh = self.sh();
        if self.focus_cell {
            let cr = g.cell_rect(sh, self.cursor);
            let tint = Color32::from_rgba_unmultiplied(0x21, 0x73, 0x46, 36);
            for r in [
                Rect::from_min_max(pos2(g.view.min.x, cr.min.y), pos2(cr.min.x, cr.max.y)),
                Rect::from_min_max(pos2(cr.max.x, cr.min.y), pos2(g.view.max.x, cr.max.y)),
                Rect::from_min_max(pos2(cr.min.x, g.view.min.y), pos2(cr.max.x, cr.min.y)),
                Rect::from_min_max(pos2(cr.min.x, cr.max.y), pos2(cr.max.x, g.view.max.y)),
            ] {
                if r.is_positive() {
                    p.rect_filled(r, 0.0, tint);
                }
            }
        }
        let sel = self.sel();
        let accent = theme::SHEET_ACCENT;
        let areas = self.areas();
        for r in &areas[..areas.len() - 1] {
            let rr = g.range_rect(sh, r).intersect(g.view.expand(4.0));
            if rr.is_positive() {
                p.rect_filled(rr, 0.0, Paper::SELECTION);
                p.rect_stroke(rr, 0.0, Stroke::new(1.0, accent), StrokeKind::Inside);
            }
        }
        // Only the part of a huge selection that is on screen.
        let sr = g.range_rect(sh, &sel).intersect(g.view.expand(4.0));
        if !sel.is_single() {
            // Tint around the active cell, which keeps its own look.
            let cr = g.cell_rect(sh, self.cursor).intersect(sr);
            let parts = [
                Rect::from_min_max(sr.min, pos2(sr.max.x, cr.min.y)),
                Rect::from_min_max(pos2(sr.min.x, cr.max.y), sr.max),
                Rect::from_min_max(pos2(sr.min.x, cr.min.y), pos2(cr.min.x, cr.max.y)),
                Rect::from_min_max(pos2(cr.max.x, cr.min.y), pos2(sr.max.x, cr.max.y)),
            ];
            let parts = if cr.is_positive() { parts.to_vec() } else { vec![sr] };
            for r in parts.into_iter().filter(|r| r.is_positive()) {
                p.rect_filled(r, 0.0, Paper::SELECTION);
            }
        }
        p.rect_stroke(sr, 0.0, Stroke::new(2.0, accent), StrokeKind::Middle);
        if self.edit.is_none() && sel.end.row < LAST.row && sel.end.col < LAST.col {
            let h = Rect::from_center_size(g.range_rect(sh, &sel).max, vec2(6.0, 6.0));
            p.rect_filled(h.expand(1.0), 0.0, Color32::WHITE);
            p.rect_filled(h, 0.0, accent);
        }
        if let Drag::Fill { target } = self.drag {
            let tr = g.range_rect(sh, &target);
            p.rect_stroke(tr, 0.0, Stroke::new(1.0, Color32::from_gray(90)), StrokeKind::Middle);
        }
        // "Marching ants" for a pending cut/copy on this sheet.
        if let Some((_, clip, Some(s), _)) = &self.clip {
            if *s == self.sheet && self.marquee {
                let r = Range::new(clip.origin, CellRef::new(clip.origin.row + clip.rows - 1, clip.origin.col + clip.cols - 1));
                let rr = g.range_rect(sh, &r).intersect(g.view.expand(4.0));
                p.add(egui::Shape::dashed_line(&[rr.left_top(), rr.right_top(), rr.right_bottom(), rr.left_bottom(), rr.left_top()], Stroke::new(1.5, accent), 4.0, 3.0));
            }
        }
        // References of the formula being typed, in their colors.
        for (r, color) in self.edited_refs() {
            let rr = g.range_rect(sh, &r).intersect(g.view.expand(4.0));
            if rr.width() > 0.0 && rr.height() > 0.0 {
                p.rect_filled(rr, 0.0, color.gamma_multiply(0.08));
                p.rect_stroke(rr, 0.0, Stroke::new(2.0, color), StrokeKind::Inside);
            }
        }
        // Preview of the formula bar edit inside the cell.
        if self.edit.is_some_and(|e| e.origin == EditOrigin::Bar) {
            let cr = g.cell_rect(sh, self.cursor);
            p.rect_filled(cr.shrink(1.0), 0.0, Paper::BG);
            let job = cell_job(&self.buf, &CellStyle::default(), Paper::TEXT, g.z, None);
            let gal = p.layout_job(job);
            p.with_clip_rect(cr).galley(pos2(cr.min.x + 3.0, cr.max.y - gal.size().y - 1.0), gal, Paper::TEXT);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_headers(&self, p: &egui::Painter, g: &Geo, rect: Rect, v: &egui::Visuals, fcols: &[u32], scols: &[u32], frows: &[u32], srows: &[u32]) {
        let sh = self.sh();
        let bg = theme::header_bg(v);
        let active = theme::header_active_bg(v);
        let text = v.text_color();
        let line = Stroke::new(1.0, if v.dark_mode { Color32::from_gray(60) } else { Color32::from_gray(210) });
        let sel = self.sel();
        let font = FontId::proportional(12.0 * g.z.clamp(0.7, 1.6));
        let col_hdr = Rect::from_min_max(pos2(g.origin.x, rect.min.y), pos2(g.view.max.x, g.origin.y));
        let row_hdr = Rect::from_min_max(pos2(rect.min.x, g.origin.y), pos2(g.origin.x, g.view.max.y));
        p.rect_filled(col_hdr, 0.0, bg);
        p.rect_filled(row_hdr, 0.0, bg);
        // A finger on the headers selects columns and rows, or resizes
        // them at a border, instead of scrolling.
        crate::touch::grip(p, col_hdr, 0.0, None);
        crate::touch::grip(p, row_hdr, 0.0, None);
        let fx = g.origin.x + g.fx * g.z;
        let fy = g.origin.y + g.fy * g.z;
        // Row numbers of filtered data are blue, like other spreadsheets.
        let filter_rows = sh.filter.as_ref().filter(|f| !f.cols.is_empty()).and_then(|f| f.data_rows());
        // (The panes of a split without them.)
        let (fcols, scols) = if col_hdr.height() < 1.0 { (&[][..], &[][..]) } else { (fcols, scols) };
        let (frows, srows) = if row_hdr.width() < 1.0 { (&[][..], &[][..]) } else { (frows, srows) };
        for (cols, clip) in [(fcols, Rect::from_min_max(col_hdr.min, pos2(fx, col_hdr.max.y))), (scols, Rect::from_min_max(pos2(fx, col_hdr.min.y), col_hdr.max))] {
            let pc = p.with_clip_rect(clip);
            for &c in cols {
                let r = Rect::from_min_max(pos2(g.x(sh, c), col_hdr.min.y), pos2(g.x_end(sh, c), col_hdr.max.y));
                if r.width() < 0.5 {
                    // Hidden column: a double line marks it.
                    for dx in [-2.0, 1.0] {
                        pc.vline(r.min.x.round() + dx, r.y_range(), Stroke::new(1.0, theme::SHEET_ACCENT));
                    }
                    continue;
                }
                let on = c >= sel.start.col && c <= sel.end.col;
                if on {
                    pc.rect_filled(r, 0.0, active);
                    pc.hline(r.x_range(), r.max.y - 1.0, Stroke::new(2.0, theme::SHEET_ACCENT));
                }
                pc.vline(r.max.x.round() - 0.5, r.y_range(), line);
                if r.width() > 8.0 {
                    let label = if self.wb.calc.r1c1 { (c + 1).to_string() } else { apex_sheet::addr::col_name(c) };
                    pc.text(r.center(), egui::Align2::CENTER_CENTER, label, font.clone(), text);
                }
            }
        }
        for (rows, clip) in [(frows, Rect::from_min_max(row_hdr.min, pos2(row_hdr.max.x, fy))), (srows, Rect::from_min_max(pos2(row_hdr.min.x, fy), row_hdr.max))] {
            let pc = p.with_clip_rect(clip);
            for &r in rows {
                let rr = Rect::from_min_max(pos2(row_hdr.min.x, g.y(sh, r)), pos2(row_hdr.max.x, g.y_end(sh, r)));
                if rr.height() < 0.5 {
                    for dy in [-2.0, 1.0] {
                        pc.hline(rr.x_range(), rr.min.y.round() + dy, Stroke::new(1.0, theme::SHEET_ACCENT));
                    }
                    continue;
                }
                let on = r >= sel.start.row && r <= sel.end.row;
                if on {
                    pc.rect_filled(rr, 0.0, active);
                    pc.vline(rr.max.x - 1.0, rr.y_range(), Stroke::new(2.0, theme::SHEET_ACCENT));
                }
                pc.hline(rr.x_range(), rr.max.y.round() - 0.5, line);
                if rr.height() > 8.0 {
                    let filtered = filter_rows.is_some_and(|d| r >= d.start.row && r <= d.end.row);
                    pc.text(rr.center(), egui::Align2::CENTER_CENTER, (r + 1).to_string(), font.clone(), if filtered { theme::ACCENT } else { text });
                }
            }
        }
        let corner = Rect::from_min_max(rect.min, g.origin);
        if corner.width() >= 1.0 && corner.height() >= 1.0 {
            p.rect_filled(corner, 0.0, bg);
            p.line_segment([pos2(corner.max.x - 10.0, corner.max.y - 3.0), pos2(corner.max.x - 3.0, corner.max.y - 10.0)], line);
        }
        if corner.height() >= 1.0 {
            p.hline(rect.min.x..=g.view.max.x, g.origin.y - 0.5, line);
        }
        if corner.width() >= 1.0 {
            p.vline(g.origin.x - 0.5, rect.min.y..=g.view.max.y, line);
        }
    }
}

// --- helpers ----------------------------------------------------------------------

fn row_header_width(sh: &Sheet, scroll_y: f32, view_h: f32) -> f32 {
    let last = sh.row_at(scroll_y + view_h) + 1;
    let digits = last.max(sh.frozen_rows).to_string().len() as f32;
    (digits * 8.0 + 16.0).max(MIN_ROW_HDR_W)
}

/// Cell contents longer than this (bytes) are only previewed in the
/// formula bar until edited.
const HUGE_TEXT: usize = 100_000;

/// Font size of a style in points.
fn style_pt(st: &CellStyle) -> f32 {
    if st.font_size > 0 { st.font_size as f32 / 10.0 } else { DEFAULT_PT }
}

fn cell_px(st: &CellStyle, z: f32) -> f32 {
    style_pt(st) * PT * z
}

/// Name shown for cells without a font family.
fn default_font() -> &'static str {
    tr("font.default")
}

/// Draws one border line with its style.
fn paint_line(p: &egui::Painter, a: Pos2, b: Pos2, l: Line, color: Color32, preview: bool) {
    let horiz = (a.y - b.y).abs() < 0.01;
    let snap = |q: Pos2, w: f32| {
        let f = |v: f32| if (w as i32) % 2 == 1 { v.round() - 0.5 } else { v.round() };
        if horiz { pos2(q.x, f(q.y)) } else { pos2(f(q.x), q.y) }
    };
    let w = if preview { l.style.width().max(1.0) } else { l.style.width() };
    let (a, b) = (snap(a, w.max(1.0)), snap(b, w.max(1.0)));
    match l.style {
        LineStyle::None => {}
        LineStyle::Double => {
            let off = if horiz { vec2(0.0, 1.5) } else { vec2(1.5, 0.0) };
            let s = Stroke::new(1.0, color);
            p.line_segment([a - off, b - off], s);
            p.line_segment([a + off, b + off], s);
        }
        LineStyle::Dashed => p.extend(egui::Shape::dashed_line(&[a, b], Stroke::new(1.0, color), 4.0, 2.0)),
        LineStyle::Dotted => p.extend(egui::Shape::dashed_line(&[a, b], Stroke::new(1.0, color), 1.0, 1.5)),
        LineStyle::Hair => {
            p.line_segment([a, b], Stroke::new(1.0, color.gamma_multiply(0.55)));
        }
        _ => {
            p.line_segment([a, b], Stroke::new(w, color));
        }
    }
}

/// Text rotated by `st.rotation` degrees (counterclockwise) or stacked.
#[allow(clippy::too_many_arguments)]
fn paint_rotated(p: &egui::Painter, clip: Rect, rect: Rect, text: &str, st: &CellStyle, color: Color32, halign: HAlign, z: f32) {
    let pad = 3.0 * z;
    let p = p.with_clip_rect(clip);
    let text: String = text.chars().take(300).collect();
    if st.rotation == ROTATION_STACKED {
        // Characters one under another, punctuation in its vertical form
        // (、。「」 and the like), as in East Asian vertical text.
        let stacked: Vec<String> = apex_doc::to_vertical(&text).chars().filter(|c| *c != '\n').map(|c| c.to_string()).collect();
        let mut job = cell_job(&stacked.join("\n"), st, color, z, None);
        job.halign = egui::Align::Center;
        let g = p.layout_job(job);
        let h = g.size().y;
        let y = match st.valign {
            VAlign::Top => rect.min.y + 1.0,
            VAlign::Center => rect.center().y - h / 2.0,
            VAlign::Bottom => rect.max.y - h - 1.0,
        };
        let x = match halign {
            HAlign::Left => rect.min.x + pad + g.size().x / 2.0,
            HAlign::Right => rect.max.x - pad - g.size().x / 2.0,
            _ => rect.center().x,
        };
        p.galley(pos2(x, y), g, color);
        return;
    }
    let g = p.layout_job(cell_job(&text, st, color, z, None));
    let angle = -(st.rotation as f32).to_radians();
    let rot = egui::emath::Rot2::from_angle(angle);
    let (w, h) = (g.size().x, g.size().y);
    let pts = [vec2(0.0, 0.0), vec2(w, 0.0), vec2(0.0, h), vec2(w, h)].map(|v| rot * v);
    let (minx, maxx) = pts.iter().fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(v.x), b.max(v.x)));
    let (miny, maxy) = pts.iter().fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(v.y), b.max(v.y)));
    let (bw, bh) = (maxx - minx, maxy - miny);
    let bx = match halign {
        HAlign::Right => rect.max.x - pad - bw,
        HAlign::Center => rect.center().x - bw / 2.0,
        _ => rect.min.x + pad,
    };
    let by = match st.valign {
        VAlign::Top => rect.min.y + 1.0,
        VAlign::Center => rect.center().y - bh / 2.0,
        VAlign::Bottom => rect.max.y - bh - 1.0,
    };
    p.add(egui::epaint::TextShape::new(pos2(bx - minx, by - miny), g, color).with_angle(angle));
}

/// The part of `text` that can be seen in a `room` sized area: laying out
/// huge strings (megabytes in hostile files) would stall every frame.
/// Aligned text keeps the part that stays in view.
fn visible_part(text: &str, room: Vec2, px: f32, wrap: bool, halign: HAlign) -> &str {
    // Glyphs are never narrower than about a quarter of the font size.
    let per_line = (room.x / (px * 0.25)) as usize + 2;
    let max = if wrap { per_line * ((room.y / px) as usize + 2) } else { per_line };
    if text.len() <= max {
        return text;
    }
    let n = text.chars().count();
    if n <= max {
        return text;
    }
    let skip = match halign {
        HAlign::Right => n - max,
        HAlign::Center => (n - max) / 2,
        _ => 0,
    };
    let at = |k: usize| text.char_indices().nth(k).map_or(text.len(), |x| x.0);
    &text[at(skip)..at(skip + max)]
}

fn cell_job(text: &str, st: &CellStyle, color: Color32, z: f32, wrap: Option<f32>) -> LayoutJob {
    fonts::note_text(text);
    let size = cell_px(st, z);
    let stroke = Stroke::new((size / 14.0).max(1.0), color);
    let (font_id, synth_italic) = fonts::font_for(st.font.as_deref(), st.bold, st.italic, size);
    let fmt = TextFormat {
        font_id,
        color,
        italics: synth_italic,
        underline: if st.underline { stroke } else { Stroke::NONE },
        strikethrough: if st.strike { stroke } else { Stroke::NONE },
        ..Default::default()
    };
    let mut job = LayoutJob::single_section(text.to_string(), fmt);
    if let Some(w) = wrap {
        job.wrap.max_width = w.max(4.0);
    }
    job
}

/// A text cell with mixed formatting: one section per run, each the cell's
/// format with the run's changes.
pub(super) fn rich_job(text: &str, runs: &[apex_sheet::rich::RichRun], st: &CellStyle, color: Color32, z: f32, wrap: Option<f32>) -> LayoutJob {
    fonts::note_text(text);
    use apex_sheet::rich::Script;
    let mut job = LayoutJob::default();
    let mut at = 0;
    for run in runs {
        let part = &text[at..(at + run.len).min(text.len())];
        at += run.len;
        let f = &run.font;
        let mut size = f.size.map_or_else(|| cell_px(st, z), |pt| pt * PT * z);
        let base = size;
        if f.script != Script::Normal {
            size *= 0.65;
        }
        let color = f.color.map_or(color, theme::rgb);
        let stroke = Stroke::new((size / 14.0).max(1.0), color);
        let (font_id, synth_italic) = fonts::font_for(f.font.as_deref().or(st.font.as_deref()), f.bold.unwrap_or(st.bold), f.italic.unwrap_or(st.italic), size);
        let fmt = TextFormat {
            font_id,
            color,
            italics: synth_italic,
            underline: if f.underline.unwrap_or(st.underline) { stroke } else { Stroke::NONE },
            strikethrough: if f.strike.unwrap_or(st.strike) { stroke } else { Stroke::NONE },
            valign: match f.script {
                Script::Super => egui::Align::TOP,
                Script::Sub => egui::Align::BOTTOM,
                Script::Normal => egui::Align::BOTTOM,
            },
            line_height: (f.script != Script::Normal).then_some(base * 1.2),
            ..Default::default()
        };
        job.append(part, 0.0, fmt);
    }
    if let Some(w) = wrap {
        job.wrap.max_width = w.max(4.0);
    }
    job
}

impl SheetView {
    /// The reference that pointing at `r` types into a formula: A1, or
    /// R1C1 relative to the cell being edited.
    fn point_ref(&self, r: Range) -> String {
        if !self.wb.calc.r1c1 {
            return if r.is_single() { r.start.to_a1() } else { r.to_a1() };
        }
        let at = self.cursor;
        let part = |tag: char, v: u32, here: u32| if v == here { tag.to_string() } else { format!("{tag}[{}]", v as i64 - here as i64) };
        let one = |c: CellRef| format!("{}{}", part('R', c.row, at.row), part('C', c.col, at.col));
        if r.is_single() { one(r.start) } else { format!("{}:{}", one(r.start), one(r.end)) }
    }
}

fn sel_label(r: &Range, r1c1: bool) -> String {
    if r.start.row == 0 && r.end.row == LAST.row && r.start.col == 0 && r.end.col == LAST.col {
        return tr("sheet.all").to_string();
    }
    if r.start.row == 0 && r.end.row == LAST.row {
        return if r1c1 { format!("C{}:C{}", r.start.col + 1, r.end.col + 1) } else { format!("{}:{}", apex_sheet::addr::col_name(r.start.col), apex_sheet::addr::col_name(r.end.col)) };
    }
    if r.start.col == 0 && r.end.col == LAST.col {
        return if r1c1 { format!("R{}:R{}", r.start.row + 1, r.end.row + 1) } else { format!("{}:{}", r.start.row + 1, r.end.row + 1) };
    }
    let rc = |c: CellRef| format!("R{}C{}", c.row + 1, c.col + 1);
    match (r1c1, r.is_single()) {
        (true, true) => rc(r.start),
        (true, false) => format!("{}:{}", rc(r.start), rc(r.end)),
        (false, true) => r.start.to_a1(),
        (false, false) => r.to_a1(),
    }
}

fn set_caret(ctx: &egui::Context, id: Id, pos: usize) {
    let mut st = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    st.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(pos))));
    st.store(ctx, id);
}

fn set_caret_end(ctx: &egui::Context, id: Id, text: &str) {
    let mut st = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    st.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(text.chars().count()))));
    st.store(ctx, id);
}

fn unique_name(wb: &Workbook, base: &str) -> String {
    let base = base.trim();
    (1..).map(|i| format!("{base}{i}")).find(|n| wb.sheet_index(n).is_none()).unwrap()
}

/// Parses tab-separated text (with Excel-style quoting) into rows of cells.
fn parse_tsv(text: &str) -> Vec<Vec<String>> {
    let text = text.strip_suffix('\n').unwrap_or(text);
    let text = text.strip_suffix('\r').unwrap_or(text);
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut chars = text.chars().peekable();
    let mut at_start = true;
    let mut quoted = false;
    while let Some(ch) = chars.next() {
        if quoted {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    cell.push('"');
                } else {
                    quoted = false;
                }
            } else {
                cell.push(ch);
            }
            continue;
        }
        match ch {
            '"' if at_start => {
                quoted = true;
                at_start = false;
            }
            '\t' => {
                row.push(std::mem::take(&mut cell));
                at_start = true;
            }
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
                at_start = true;
            }
            c => {
                cell.push(c);
                at_start = false;
            }
        }
    }
    row.push(cell);
    rows.push(row);
    rows
}

/// Adds or removes one decimal place in a number format code.
fn change_decimals(code: &str, delta: i32) -> String {
    let code = if code.eq_ignore_ascii_case("general") { "0" } else { code };
    let (first, rest) = match code.find(';') {
        Some(i) => (&code[..i], &code[i..]),
        None => (code, ""),
    };
    let Some(last_zero) = first.rfind(['0', '#']) else { return code.to_string() };
    let dot = first[..=last_zero].rfind('.');
    let new = if delta > 0 {
        match dot {
            Some(_) => format!("{}0{}", &first[..=last_zero], &first[last_zero + 1..]),
            None => format!("{}.0{}", &first[..=last_zero], &first[last_zero + 1..]),
        }
    } else {
        match dot {
            Some(d) if last_zero > d + 1 => format!("{}{}", &first[..last_zero], &first[last_zero + 1..]),
            Some(d) => format!("{}{}", &first[..d], &first[last_zero + 1..]),
            None => first.to_string(),
        }
    };
    format!("{new}{rest}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs frames of the Home tab over a sheet, with AccessKit on, and
    /// returns the named nodes of the last one: (label, toggled).
    fn a11y_nodes(v: &mut SheetView, ctx: &egui::Context) -> Vec<(String, Option<bool>)> {
        let mut shell = Shell { cmd: None, show_formula_bar: true, file_name: "a11y".into(), fullscreen: false, dark: None, others: Vec::new(), books: Vec::new(), side: None, recording: None };
        let mut nodes = Vec::new();
        for _ in 0..3 {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))), ..Default::default() };
            // (No fonts::apply: the families it registers are global, and
            // other tests' contexts would lay out with them unbound.)
            let mut out = ctx.run_ui(input, |ui| {
                egui::Panel::top("toolstrip").show(ui, |ui| v.toolstrip(ui, "home", &mut shell));
                v.ui(ui, &mut shell);
            });
            out.textures_delta.clear();
            if let Some(u) = out.platform_output.accesskit_update {
                nodes = u.nodes.iter().filter_map(|(_, n)| Some((n.label()?.to_string(), n.toggled().map(|t| t == egui::accesskit::Toggled::True)))).collect();
            }
        }
        nodes
    }

    #[test]
    fn screen_readers_get_names_and_the_cell() {
        let mut wb = Workbook::empty();
        wb.sheets.push(Sheet::new("Data"));
        wb.load_cell(0, CellRef::new(1, 1), Content::Value(Value::Number(42.0)), 0);
        let e = apex_sheet::formula::parse_canonical("B2*2").unwrap();
        wb.load_cell(0, CellRef::new(2, 1), Content::Formula { expr: e.into(), value: Value::Empty }, 0);
        wb.finish_load();
        wb.recalc_all();
        let mut v = SheetView::new(wb);
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        ctx.enable_accesskit();
        v.select(CellRef::new(1, 1), false);
        let nodes = a11y_nodes(&mut v, &ctx);
        let has = |l: &str| nodes.iter().any(|(n, _)| n == l);
        // The toolstrip's buttons drawn by hand have their names.
        assert!(has(tr("edit.paste")), "{nodes:?}");
        assert!(has(tr("format.bold")), "{nodes:?}");
        assert!(has(&trf("a11y.sheet", &[("name", &"Data")])));
        assert!(has("B2: 42"), "{nodes:?}");
        // Moving says the new cell, with its formula; pressed buttons say so.
        v.select(CellRef::new(2, 1), false);
        v.toggle(|s| s.bold, |s, b| s.bold = b);
        let nodes = a11y_nodes(&mut v, &ctx);
        assert!(nodes.iter().any(|(n, _)| n.starts_with("B3: 84") && n.ends_with("=B2*2")), "{nodes:?}");
        assert!(nodes.iter().any(|(n, t)| n == tr("format.bold") && *t == Some(true)), "{nodes:?}");
    }

    #[test]
    fn tsv() {
        assert_eq!(parse_tsv("a\tb\n1\t2\n"), vec![vec!["a", "b"], vec!["1", "2"]]);
        assert_eq!(parse_tsv("\"x\ty\"\tz"), vec![vec!["x\ty", "z"]]);
        assert_eq!(parse_tsv("\"say \"\"hi\"\"\""), vec![vec!["say \"hi\""]]);
    }

    #[test]
    fn decimals() {
        assert_eq!(change_decimals("General", 1), "0.0");
        assert_eq!(change_decimals("0.00", 1), "0.000");
        assert_eq!(change_decimals("0.00", -1), "0.0");
        assert_eq!(change_decimals("0.0", -1), "0");
        assert_eq!(change_decimals("#,##0.00 \"€\"", -1), "#,##0.0 \"€\"");
        assert_eq!(change_decimals("0%", 1), "0.0%");
    }

    /// Find Next walks the matches in reading order, across sheets, as
    /// the list of every match does.
    #[test]
    fn find_next_follows_reading_order() {
        let mut wb = Workbook::empty();
        for name in ["A", "B"] {
            wb.sheets.push(Sheet::new(name));
        }
        for s in 0..2 {
            for i in 0..40u32 {
                let (r, c) = ((i * 7 + s as u32 * 3) % 13, (i * 5) % 6);
                let text = if (i + s as u32) % 3 == 0 { format!("x{i}") } else { format!("y{i}") };
                wb.load_cell(s, CellRef::new(r, c), Content::Value(Value::text(text)), 0);
            }
        }
        wb.finish_load();
        let mut v = SheetView::new(wb);
        let mut f = FindState::new(false);
        f.text = "X".into();
        f.all_sheets = true;
        let all: Vec<(usize, CellRef)> = v.find_all(&f).into_iter().map(|(s, c, _)| (s, c)).collect();
        assert!(all.len() > 10);
        v.select(CellRef::new(0, 0), false);
        let mut seen = Vec::new();
        for _ in 0..all.len() + 1 {
            v.find_next(&f);
            seen.push((v.sheet, v.cursor));
        }
        // A1 of the first sheet may hold a match: it comes last, then again.
        let start = all.iter().position(|&(s, c)| (s, c) > (0, CellRef::new(0, 0))).unwrap();
        let expected: Vec<(usize, CellRef)> = all[start..].iter().chain(&all[..start]).chain(all.get(start)).copied().collect();
        assert_eq!(seen, expected);
    }

    /// A sheet of `n` rows (number, repeated text, formula) and the time of
    /// what a user does with it: frames, scrolling to the end, selecting a
    /// whole column, sorting and filtering it, finding.
    fn big_sheet_times(n: u32) -> Vec<(&'static str, f64)> {
        let mut wb = Workbook::empty();
        wb.sheets.push(Sheet::new("S"));
        for r in 0..n {
            wb.load_cell(0, CellRef::new(r, 0), Content::Value(Value::Number((r as u64 * 7919 % n as u64) as f64)), 0);
            wb.load_cell(0, CellRef::new(r, 1), Content::Value(Value::text(format!("item {}", r % 100))), 0);
            let e = apex_sheet::formula::parse_canonical(&format!("A{}*2", r + 1)).unwrap();
            wb.load_cell(0, CellRef::new(r, 2), Content::Formula { expr: e.into(), value: Value::Empty }, 0);
        }
        wb.finish_load();
        let mut v = SheetView::new(wb);
        let ctx = crate::headless::context();
        let mut shell = Shell { cmd: None, show_formula_bar: true, file_name: "big".into(), fullscreen: false, dark: None, others: Vec::new(), books: Vec::new(), side: None, recording: None };
        let mut frame = |v: &mut SheetView| {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))), ..Default::default() };
            let t = web_time::Instant::now();
            let mut out = ctx.run_ui(input, |ui| {
                crate::fonts::apply(ui.ctx());
                v.ui(ui, &mut shell);
                v.status(ui);
            });
            out.textures_delta.clear();
            t.elapsed().as_secs_f64()
        };
        let mut out = Vec::new();
        frame(&mut v);
        out.push(("frame", frame(&mut v)));
        v.select(CellRef::new(n - 1, 0), false);
        out.push(("scroll to end", frame(&mut v)));
        out.push(("frame at end", frame(&mut v)));
        v.anchor = CellRef::new(0, 0);
        v.cursor = CellRef::new(LAST.row, 0);
        out.push(("select column", frame(&mut v)));
        let t = web_time::Instant::now();
        v.sort(true);
        out.push(("sort column", t.elapsed().as_secs_f64()));
        out.push(("frame after sort", frame(&mut v)));
        v.select(CellRef::new(0, 1), false);
        let t = web_time::Instant::now();
        v.toggle_filter();
        v.set_col_filter(0, Some(apex_sheet::data::FilterKind::Values { items: vec!["7".to_string()], blanks: false }));
        out.push(("filter column", t.elapsed().as_secs_f64()));
        out.push(("frame filtered", frame(&mut v)));
        out.push(("next frame filtered", frame(&mut v)));
        v.clear_filters();
        let mut f = FindState::new(false);
        f.text = "item 99".into();
        v.select(CellRef::new(0, 0), false);
        let t = web_time::Instant::now();
        v.find_next(&f);
        out.push(("find next", t.elapsed().as_secs_f64()));
        out
    }

    // Each step of a sheet of 100k rows takes well under a second, and a
    // frame a few milliseconds. Filtering went row by row over the whole
    // table once per row (minutes), and a frame over a filter that leaves
    // one row painted the hundred thousand hidden ones (0.7 s).
    #[test]
    fn big_sheet_stays_responsive() {
        for (what, secs) in big_sheet_times(100_000) {
            eprintln!("{what}: {secs:.3} s");
            let limit = if what.contains("frame") { 0.3 } else { 3.0 };
            assert!(secs < limit, "{what}: {secs:.3} s");
        }
    }

    /// The same with a million rows (run by hand: `--ignored`).
    #[test]
    #[ignore]
    fn million_rows() {
        for (what, secs) in big_sheet_times(1_000_000) {
            eprintln!("{what}: {secs:.3} s");
        }
    }

    // A formula still being written (AutoSum's `=SUM(`) stays in the
    // editor when a toolstrip command runs; Enter says what's wrong.
    #[test]
    fn a_command_leaves_an_unfinished_formula_open() {
        let mut wb = Workbook::empty();
        wb.sheets.push(Sheet::new("Data"));
        let mut v = SheetView::new(wb);
        v.buf = "=SUM(".into();
        v.edit = Some(Edit { origin: EditOrigin::Cell, typed: true });
        v.toggle(|s| s.bold, |s, b| s.bold = b);
        v.select_sheet(0);
        assert!(v.notices.is_empty(), "{:?}", v.notices.iter().map(|n| &n.text).collect::<Vec<_>>());
        assert!(v.edit.is_some() && v.buf == "=SUM(");
        assert!(!v.commit_edit());
        assert!(v.notices.iter().any(|n| n.error));
        v.buf = "=SUM(1,2)".into();
        assert!(v.commit_edit() && v.edit.is_none());
        assert_eq!(v.wb.edit_text(0, CellRef::new(0, 0)), "=SUM(1,2)");
    }

    // A command that needs data selected first says so as a notice, not
    // as an error (nothing failed).
    #[test]
    fn missing_data_is_a_notice_not_an_error() {
        let mut wb = Workbook::empty();
        wb.sheets.push(Sheet::new("Data"));
        let mut v = SheetView::new(wb);
        v.toggle_filter();
        v.open_subtotals();
        v.open_forecast();
        v.open_data_table();
        v.flash_fill_cmd();
        assert_eq!(v.notices.len(), 5, "{:?}", v.notices.iter().map(|n| &n.text).collect::<Vec<_>>());
        assert!(v.notices.iter().all(|n| !n.error), "{:?}", v.notices.iter().map(|n| &n.text).collect::<Vec<_>>());
    }
}

/// The first row and column the frozen panes show (none when there are
/// no frozen rows or columns).
fn frozen_top(sh: &Sheet, fr: u32, fc: u32) -> CellRef {
    let t = sh.frozen_top;
    CellRef::new(if t.row < fr { t.row } else { 0 }, if t.col < fc { t.col } else { 0 })
}

/// Width and height of the frozen panes (sheet units).
fn frozen_size(sh: &Sheet) -> (f32, f32) {
    let top = frozen_top(sh, sh.frozen_rows, sh.frozen_cols);
    (sh.col_x(sh.frozen_cols) - sh.col_x(top.col), sh.row_y(sh.frozen_rows) - sh.row_y(top.row))
}

/// `s`, a scroll past `fixed`, moved on to the next whole column or row
/// unless it is at one.
fn ceil_to(s: f32, fixed: f32, at: impl Fn(f32) -> u32, start: impl Fn(u32) -> f32) -> f32 {
    let i = at(fixed + s);
    if start(i) - fixed < s { start(i + 1) - fixed } else { s }
}
