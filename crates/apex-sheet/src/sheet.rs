//! Sparse sheet storage and cell styles.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::addr::{CellRef, Range};
use crate::data::{AutoFilter, CfRule, Validation};
use crate::format::NumFormat;
use crate::cell_formula::CellFormula;
use crate::value::Value;

#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    Empty,
    Value(Value),
    Formula { expr: CellFormula, value: Value },
    /// A value spilled by a dynamic array formula elsewhere (its anchor).
    /// Written by the calculation, never by the user.
    Spill(Value),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub content: Content,
    pub style: u32,
    /// Note and hyperlink (rare, so boxed).
    pub extra: Option<Box<CellExtra>>,
}

/// What a cell can carry besides its content and format.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CellExtra {
    pub note: Option<Note>,
    pub link: Option<Link>,
    /// Mixed formatting of a text value (dropped when the text changes).
    pub rich: Option<Arc<crate::rich::RichText>>,
    /// A picture that is the cell's value (Place in Cell); the content is
    /// then #VALUE!, what other programs see. Dropped when it changes.
    pub picture: Option<Arc<crate::image::CellPicture>>,
    /// Rows and columns of an array formula from a file (entered with
    /// Ctrl+Shift+Enter): its result is cut or filled to that size
    /// instead of spilling. Dropped when the formula changes.
    pub array_size: Option<(u32, u32)>,
}

/// A note (a classic comment) attached to a cell.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Note {
    pub text: String,
    pub author: String,
    /// Shown all the time, not only on hover.
    pub visible: bool,
    /// A modern (threaded) comment rather than a classic note.
    pub threaded: bool,
    /// Threaded comments: marked as resolved.
    pub resolved: bool,
    /// When it was written (`2024-05-01T10:30:00`), empty when unknown.
    pub date: String,
    pub replies: Vec<Reply>,
}

/// An answer in the thread of a comment.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Reply {
    pub text: String,
    pub author: String,
    pub date: String,
}

impl Note {
    /// The whole thread as plain text, for formats without replies.
    pub fn flat_text(&self) -> String {
        let mut s = self.text.clone();
        for r in &self.replies {
            s.push_str(&format!("\n\n{}: {}", r.author, r.text));
        }
        s
    }
}

/// A hyperlink: a URL (`https:`, `mailto:`, a file) or a place in the
/// workbook written `#Sheet!A1` (or `#Name`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Link {
    pub target: String,
    pub tip: String,
}

impl Link {
    /// The place in the workbook this link points to, if internal.
    pub fn place(&self) -> Option<&str> {
        self.target.strip_prefix('#')
    }
}

static EMPTY: Value = Value::Empty;

impl Cell {
    pub fn value(&self) -> &Value {
        match &self.content {
            Content::Empty => &EMPTY,
            Content::Value(v) => v,
            Content::Formula { value, .. } => value,
            Content::Spill(v) => v,
        }
    }

    pub fn formula(&self) -> Option<&CellFormula> {
        match &self.content {
            Content::Formula { expr, .. } => Some(expr),
            _ => None,
        }
    }

    pub fn is_blank(&self) -> bool {
        matches!(self.content, Content::Empty) && self.style == 0 && self.extra.is_none()
    }

    pub fn new(content: Content, style: u32) -> Cell {
        Cell { content, style, extra: None }
    }

    pub fn note(&self) -> Option<&Note> {
        self.extra.as_ref()?.note.as_ref()
    }

    pub fn link(&self) -> Option<&Link> {
        self.extra.as_ref()?.link.as_ref()
    }

    /// Changes the note or link, dropping the box when both are gone.
    pub fn set_extra(&mut self, f: impl FnOnce(&mut CellExtra)) {
        let mut x = self.extra.take().map(|b| *b).unwrap_or_default();
        f(&mut x);
        self.extra = (x != CellExtra::default()).then(|| Box::new(x));
    }

    /// `old` (if any) with other content, keeping its format, note and link;
    /// `None` when nothing would be left.
    pub fn with_content(old: Option<&Cell>, content: Content) -> Option<Cell> {
        let same = old.is_some_and(|c| c.content == content);
        let mut cell = Cell { content, style: old.map_or(0, |c| c.style), extra: old.and_then(|c| c.extra.clone()) };
        if !same && cell.extra.as_ref().is_some_and(|x| x.rich.is_some() || x.picture.is_some() || x.array_size.is_some()) {
            cell.set_extra(|x| {
                x.rich = None;
                x.picture = None;
                x.array_size = None;
            });
        }
        (!cell.is_blank()).then_some(cell)
    }

    /// The runs of mixed formatting of a text cell, when they fit its text.
    pub fn rich(&self) -> Option<&crate::rich::RichText> {
        let runs = self.extra.as_ref()?.rich.as_deref()?;
        match &self.content {
            Content::Value(crate::value::Value::Text(t)) if crate::rich::fits(runs, t) => Some(runs),
            _ => None,
        }
    }

    /// The picture that is the cell's value, while its content is still
    /// the #VALUE! placeholder.
    pub fn picture(&self) -> Option<&crate::image::CellPicture> {
        let p = self.extra.as_ref()?.picture.as_deref()?;
        matches!(self.content, Content::Value(Value::Error(crate::value::ErrorKind::Value))).then_some(p)
    }

    /// A cell whose value is `picture`.
    pub fn with_picture(old: Option<&Cell>, picture: crate::image::CellPicture) -> Cell {
        let mut cell = Cell { content: Content::Value(Value::Error(crate::value::ErrorKind::Value)), style: old.map_or(0, |c| c.style), extra: old.and_then(|c| c.extra.clone()) };
        cell.set_extra(|x| {
            x.rich = None;
            x.picture = Some(Arc::new(picture));
        });
        cell
    }

    /// `old` (if any) with another style, keeping content, note and link.
    pub fn with_style(old: Option<&Cell>, style: u32) -> Option<Cell> {
        let cell = Cell { content: old.map_or(Content::Empty, |c| c.content.clone()), style, extra: old.and_then(|c| c.extra.clone()) };
        (!cell.is_blank()).then_some(cell)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum HAlign {
    #[default]
    General,
    Left,
    Center,
    Right,
    Justify,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum VAlign {
    Top,
    Center,
    #[default]
    Bottom,
}

/// Side flags, used to pick sides of [`Borders`].
pub mod border {
    pub const TOP: u8 = 1;
    pub const BOTTOM: u8 = 2;
    pub const LEFT: u8 = 4;
    pub const RIGHT: u8 = 8;
    pub const ALL: u8 = 15;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum LineStyle {
    #[default]
    None,
    Thin,
    Medium,
    Thick,
    Dashed,
    Dotted,
    Double,
    Hair,
}

impl LineStyle {
    /// Stroke width in points.
    pub fn width(self) -> f32 {
        match self {
            LineStyle::None => 0.0,
            LineStyle::Hair => 0.5,
            LineStyle::Thin | LineStyle::Dashed | LineStyle::Dotted => 1.0,
            LineStyle::Medium => 2.0,
            LineStyle::Thick | LineStyle::Double => 3.0,
        }
    }
}

/// One border line; `color` None is automatic (black).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Line {
    pub style: LineStyle,
    pub color: Option<u32>,
}

impl Line {
    pub const THIN: Line = Line { style: LineStyle::Thin, color: None };

    pub fn new(style: LineStyle, color: Option<u32>) -> Line {
        Line { style, color }
    }

    pub fn is_none(&self) -> bool {
        self.style == LineStyle::None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Borders {
    pub top: Line,
    pub bottom: Line,
    pub left: Line,
    pub right: Line,
}

impl Borders {
    pub const NONE: Borders = Borders { top: Line { style: LineStyle::None, color: None }, bottom: Line { style: LineStyle::None, color: None }, left: Line { style: LineStyle::None, color: None }, right: Line { style: LineStyle::None, color: None } };

    /// Borders with `line` on the sides in `flags`.
    pub fn sides(flags: u8, line: Line) -> Borders {
        let mut b = Borders::NONE;
        b.set(flags, line);
        b
    }

    pub fn is_empty(&self) -> bool {
        self.top.is_none() && self.bottom.is_none() && self.left.is_none() && self.right.is_none()
    }

    /// Sets the sides in `flags` to `line`.
    pub fn set(&mut self, flags: u8, line: Line) {
        if flags & border::TOP != 0 {
            self.top = line;
        }
        if flags & border::BOTTOM != 0 {
            self.bottom = line;
        }
        if flags & border::LEFT != 0 {
            self.left = line;
        }
        if flags & border::RIGHT != 0 {
            self.right = line;
        }
    }

    /// Flags of the sides that have a line.
    pub fn flags(&self) -> u8 {
        let mut f = 0;
        for (l, flag) in [(self.top, border::TOP), (self.bottom, border::BOTTOM), (self.left, border::LEFT), (self.right, border::RIGHT)] {
            if !l.is_none() {
                f |= flag;
            }
        }
        f
    }
}

/// `CellStyle::rotation` value for vertically stacked letters.
pub const ROTATION_STACKED: i16 = 255;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CellStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    /// Font family; None = default.
    pub font: Option<Arc<str>>,
    /// Font size in tenths of a point; 0 = default.
    pub font_size: u16,
    /// 0xRRGGBB
    pub color: Option<u32>,
    pub fill: Option<u32>,
    pub halign: HAlign,
    pub valign: VAlign,
    pub wrap: bool,
    /// Indentation level (each level is about 3 characters wide).
    pub indent: u8,
    /// Text rotation in degrees (-90..=90), or [`ROTATION_STACKED`].
    pub rotation: i16,
    pub borders: Borders,
    pub num_fmt: Arc<str>,
    /// Editable when the sheet is protected (cells are locked by default).
    pub unlocked: bool,
    /// The formula is not shown when the sheet is protected.
    pub formula_hidden: bool,
    /// Named cell style the format starts from (None = Normal).
    pub named: Option<Arc<str>>,
    /// TRUE and FALSE show as a check box that clicks toggle (Insert >
    /// Checkbox).
    pub checkbox: bool,
    /// Theme colors and fonts behind `color`, `fill`, the borders and `font`.
    pub theme: ThemeRefs,
}

/// Which colors and font of a [`CellStyle`] come from the theme. A
/// reference only counts while the RGB still matches it (see
/// `cellstyles::retheme`), so edits that set plain colors need not clear it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct ThemeRefs {
    pub color: Option<apex_doc::ThemeColor>,
    pub fill: Option<apex_doc::ThemeColor>,
    /// Top, bottom, left, right.
    pub borders: [Option<apex_doc::ThemeColor>; 4],
    /// 0 none, [`THEME_MAJOR`] or [`THEME_MINOR`].
    pub font: u8,
}

pub const THEME_MAJOR: u8 = 1;
pub const THEME_MINOR: u8 = 2;

impl Default for CellStyle {
    fn default() -> Self {
        CellStyle {
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            font: None,
            font_size: 0,
            color: None,
            fill: None,
            halign: HAlign::General,
            valign: VAlign::Bottom,
            wrap: false,
            indent: 0,
            rotation: 0,
            borders: Borders::NONE,
            num_fmt: Arc::from("General"),
            unlocked: false,
            formula_hidden: false,
            named: None,
            checkbox: false,
            theme: ThemeRefs::default(),
        }
    }
}

/// Interned styles; id 0 is always the default style.
#[derive(Clone, Debug)]
pub struct StyleTable {
    styles: Vec<CellStyle>,
    formats: Vec<NumFormat>,
    map: HashMap<CellStyle, u32>,
}

impl Default for StyleTable {
    fn default() -> Self {
        let d = CellStyle::default();
        let mut map = HashMap::new();
        map.insert(d.clone(), 0);
        StyleTable { styles: vec![d], formats: vec![NumFormat::general()], map }
    }
}

impl StyleTable {
    pub fn get(&self, id: u32) -> &CellStyle {
        self.styles.get(id as usize).unwrap_or(&self.styles[0])
    }

    pub fn format(&self, id: u32) -> &NumFormat {
        self.formats.get(id as usize).unwrap_or(&self.formats[0])
    }

    pub fn intern(&mut self, s: CellStyle) -> u32 {
        if let Some(&id) = self.map.get(&s) {
            return id;
        }
        let id = self.styles.len() as u32;
        self.formats.push(NumFormat::parse(&s.num_fmt));
        self.styles.push(s.clone());
        self.map.insert(s, id);
        id
    }

    pub fn len(&self) -> usize {
        self.styles.len()
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    pub fn iter(&self) -> impl Iterator<Item = (u32, &CellStyle)> {
        self.styles.iter().enumerate().map(|(i, s)| (i as u32, s))
    }

    /// Changes styles in place (theme colors, the Normal style in id 0).
    /// Cells keep their ids; styles that become equal stay separate ids.
    pub fn update_all(&mut self, mut f: impl FnMut(u32, &mut CellStyle)) {
        let mut changed = false;
        for (i, s) in self.styles.iter_mut().enumerate() {
            let before = s.clone();
            f(i as u32, s);
            if *s != before {
                changed = true;
                if s.num_fmt != before.num_fmt {
                    self.formats[i] = NumFormat::parse(&s.num_fmt);
                }
            }
        }
        if changed {
            self.map.clear();
            for (i, s) in self.styles.iter().enumerate() {
                self.map.entry(s.clone()).or_insert(i as u32);
            }
        }
    }
}

pub const DEFAULT_COL_WIDTH: f32 = 88.0;
pub const DEFAULT_ROW_HEIGHT: f32 = 22.0;
/// The largest row height or column width, in points.
pub const MAX_SIZE: f32 = 4000.0;

/// Custom row heights or column widths, with a lazily built prefix-sum
/// index so positions are found by binary search even when every row has
/// its own height. Reads go through `Deref` to the underlying map; writes
/// go through `insert`, `remove` or `map_mut`, which invalidate the index.
#[derive(Clone, Debug, Default)]
pub struct Sizes {
    map: BTreeMap<u32, f32>,
    index: std::sync::OnceLock<SizeIndex>,
}

#[derive(Clone, Debug, Default)]
struct SizeIndex {
    keys: Vec<u32>,
    /// Sum of the custom sizes of `keys[..i]`.
    cum: Vec<f32>,
}

impl std::ops::Deref for Sizes {
    type Target = BTreeMap<u32, f32>;

    fn deref(&self) -> &BTreeMap<u32, f32> {
        &self.map
    }
}

impl PartialEq for Sizes {
    fn eq(&self, other: &Self) -> bool {
        self.map == other.map
    }
}

impl Sizes {
    /// Sizes outside 0..=`MAX_SIZE` (as a file may give) are brought in:
    /// a negative one would have every row shown at once.
    pub fn insert(&mut self, k: u32, v: f32) -> Option<f32> {
        self.index = Default::default();
        self.map.insert(k, if v.is_finite() { v.clamp(0.0, MAX_SIZE) } else { 0.0 })
    }

    pub fn remove(&mut self, k: &u32) -> Option<f32> {
        self.index = Default::default();
        self.map.remove(k)
    }

    pub fn map_mut(&mut self) -> &mut BTreeMap<u32, f32> {
        self.index = Default::default();
        &mut self.map
    }

    fn index(&self) -> &SizeIndex {
        self.index.get_or_init(|| {
            let mut keys = Vec::with_capacity(self.map.len());
            let mut cum = Vec::with_capacity(self.map.len() + 1);
            let mut acc = 0.0f32;
            for (&k, &v) in &self.map {
                keys.push(k);
                cum.push(acc);
                acc += v;
            }
            cum.push(acc);
            SizeIndex { keys, cum }
        })
    }

    /// Start offset of index `i` given the default size.
    pub fn offset(&self, i: u32, default: f32) -> f32 {
        if self.map.is_empty() {
            return i as f32 * default;
        }
        let ix = self.index();
        let n = ix.keys.partition_point(|&k| k < i);
        (i - n as u32) as f32 * default + ix.cum[n]
    }

    /// Index containing offset `pos` (clamped to >= 0).
    pub fn index_at(&self, pos: f32, default: f32) -> u32 {
        if pos <= 0.0 {
            return 0;
        }
        if self.map.is_empty() {
            return (pos / default) as u32;
        }
        let ix = self.index();
        let start = |j: usize| (ix.keys[j] - j as u32) as f32 * default + ix.cum[j];
        // Last custom entry starting at or before `pos`.
        let n = ix.keys.len();
        let (mut lo, mut hi) = (0usize, n);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if start(mid) <= pos { lo = mid + 1 } else { hi = mid }
        }
        if lo == 0 {
            return (pos / default) as u32;
        }
        let j = lo - 1;
        let end = start(j) + (ix.cum[j + 1] - ix.cum[j]);
        if pos < end {
            return ix.keys[j];
        }
        ix.keys[j] + 1 + ((pos - end) / default) as u32
    }

    /// The indexes from `a` to `b` that are not hidden (size 0). A window
    /// over a filtered sheet may span a million hidden rows: only the
    /// shown ones are listed, for the view to paint.
    pub fn shown(&self, a: u32, b: u32) -> Vec<u32> {
        let mut out = Vec::new();
        if a > b {
            return out;
        }
        let mut next = a;
        for (&k, _) in self.map.range(a..=b).filter(|(_, h)| **h == 0.0) {
            out.extend(next..k);
            next = k + 1;
        }
        if next <= b {
            out.extend(next..=b);
        }
        out
    }
}

#[derive(Clone, Debug)]
pub struct Sheet {
    pub name: String,
    cols: Vec<crate::column::Column>,
    pub col_widths: Sizes,
    pub row_heights: Sizes,
    pub default_col_width: f32,
    pub default_row_height: f32,
    pub frozen_rows: u32,
    pub frozen_cols: u32,
    /// First row and column of the frozen panes. As in Excel, freezing a
    /// scrolled sheet keeps the rows and columns scrolled away above and
    /// left of the frozen ones out of view.
    pub frozen_top: CellRef,
    pub merges: Vec<Range>,
    pub hidden: bool,
    pub tab_color: Option<u32>,
    /// The sheet's name in VBA code (Excel's codeName), when it has one.
    pub code_name: Option<String>,
    /// Transition formula evaluation (Lotus 1-2-3 rules): text that is
    /// not a number counts as 0 in the sheet's formulas.
    pub lotus_eval: bool,
    pub filter: Option<AutoFilter>,
    pub validations: Vec<Validation>,
    /// Conditional formatting rules, highest priority first.
    pub cond_formats: Vec<CfRule>,
    /// Outline (grouping) level of rows and columns, 1..=7.
    pub row_levels: BTreeMap<u32, u8>,
    pub col_levels: BTreeMap<u32, u8>,
    /// Summary rows go below the detail (and columns to the right).
    pub summary_below: bool,
    pub summary_right: bool,
    pub protection: Option<crate::Protection>,
    /// Ranges that stay editable while the sheet is protected.
    pub edit_ranges: Vec<crate::protect::EditRange>,
    pub page: crate::page::SheetPage,
    /// Pictures over the cells, back to front.
    pub images: Vec<crate::image::SheetImage>,
    /// Charts over the cells, back to front.
    pub charts: Vec<crate::chart::Chart>,
    /// Shapes and text boxes over the cells, back to front.
    pub shapes: Vec<crate::shape::SheetShape>,
    pub sparklines: Vec<crate::chart::SparkGroup>,
    /// Tables (list objects).
    pub tables: Vec<crate::table::Table>,
    /// Cells of the Watch Window.
    pub cell_watches: Vec<CellRef>,
    /// This sheet's part of the workbook's custom views.
    pub custom_views: Vec<crate::views::SheetView>,
    /// Normal, page break preview or page layout (a view setting: not
    /// undoable).
    pub view: crate::ViewMode,
    /// Parts only linked from the sheet that are kept as they were (pivot
    /// tables…); the parts themselves are in the workbook's `kept`.
    pub kept: Vec<apex_doc::KeptLink>,
}

impl Sheet {
    pub fn new(name: impl Into<String>) -> Self {
        Sheet {
            name: name.into(),
            cols: Vec::new(),
            col_widths: Sizes::default(),
            row_heights: Sizes::default(),
            default_col_width: DEFAULT_COL_WIDTH,
            default_row_height: DEFAULT_ROW_HEIGHT,
            frozen_rows: 0,
            frozen_cols: 0,
            frozen_top: CellRef::new(0, 0),
            merges: Vec::new(),
            hidden: false,
            tab_color: None,
            code_name: None,
            lotus_eval: false,
            filter: None,
            validations: Vec::new(),
            cond_formats: Vec::new(),
            row_levels: BTreeMap::new(),
            col_levels: BTreeMap::new(),
            summary_below: true,
            summary_right: true,
            protection: None,
            edit_ranges: Vec::new(),
            page: Default::default(),
            images: Vec::new(),
            charts: Vec::new(),
            shapes: Vec::new(),
            sparklines: Vec::new(),
            tables: Vec::new(),
            cell_watches: Vec::new(),
            custom_views: Vec::new(),
            view: crate::ViewMode::Normal,
            kept: Vec::new(),
        }
    }

    #[inline]
    pub fn get(&self, c: CellRef) -> Option<&Cell> {
        self.cols.get(c.col as usize)?.get(c.row)
    }

    #[inline]
    pub fn value(&self, c: CellRef) -> &Value {
        self.get(c).map(Cell::value).unwrap_or(&EMPTY)
    }

    pub fn get_mut(&mut self, c: CellRef) -> Option<&mut Cell> {
        self.cols.get_mut(c.col as usize)?.get_mut(c.row)
    }

    /// Stores a cell (or removes it when `None` / blank). Returns the old cell.
    pub fn put(&mut self, c: CellRef, cell: Option<Cell>) -> Option<Cell> {
        match cell {
            Some(cell) if !cell.is_blank() => {
                let ci = c.col as usize;
                if self.cols.len() <= ci {
                    self.cols.resize_with(ci + 1, Default::default);
                }
                self.cols[ci].insert(c.row, cell)
            }
            _ => self.cols.get_mut(c.col as usize).and_then(|col| col.remove(c.row)),
        }
    }

    /// Raw cell write used by importers (see [`crate::Workbook::load_cell`]).
    /// Cells past the sheet's last row or column (damaged files) are dropped.
    pub fn load_cell(&mut self, c: CellRef, mut content: Content, style: u32) {
        if c.row >= crate::addr::MAX_ROWS || c.col >= crate::addr::MAX_COLS {
            return;
        }
        if let Content::Formula { expr, .. } = &mut content {
            if let Some(copy) = self.copy_of_neighbour(c, &expr.expr()) {
                *expr = copy;
            }
        }
        self.put(c, Some(Cell::new(content, style)));
    }

    /// `e`, the formula of `c`, as a copy of the one above or to the left
    /// (filled down or across) sharing its tree, when it is exactly that.
    pub(crate) fn copy_of_neighbour(&self, c: CellRef, e: &crate::formula::Expr) -> Option<CellFormula> {
        [(1, 0), (0, 1)].into_iter().find_map(|(dr, dc)| {
            let n = CellRef::new(c.row.checked_sub(dr as u32)?, c.col.checked_sub(dc as u32)?);
            self.get(n)?.formula()?.follow(dr, dc, e)
        })
    }

    /// Changes the note or link of a cell in place, without undo (importers).
    pub fn edit_extra(&mut self, c: CellRef, f: impl FnOnce(&mut CellExtra)) {
        let mut cell = self.get(c).cloned().unwrap_or_else(|| Cell::new(Content::Empty, 0));
        cell.set_extra(f);
        self.put(c, Some(cell));
    }

    /// An array formula of a file made before dynamic arrays (Ctrl+Shift+
    /// Enter): its area keeps its size ([`CellExtra::array_size`]).
    pub fn mark_fixed_array(&mut self, area: &Range) -> usize {
        if self.get(area.start).is_some_and(|c| c.formula().is_some()) {
            self.edit_extra(area.start, |x| x.array_size = Some((area.rows(), area.cols())));
        }
        self.mark_spilled(area)
    }

    /// Turns the values of an array formula's area (all but its first
    /// cell) into spilled values, as files store them (importers). Gives
    /// the columns and cells it went over (importers bound the total).
    pub fn mark_spilled(&mut self, area: &Range) -> usize {
        let mut work = 0;
        for ci in area.start.col as usize..=(area.end.col as usize).min(self.cols.len().saturating_sub(1)) {
            work += 1;
            for (row, cell) in self.cols[ci].range_mut(area.start.row, area.end.row) {
                work += 1;
                if CellRef::new(row, ci as u32) != area.start {
                    if let Content::Value(v) = std::mem::replace(&mut cell.content, Content::Empty) {
                        cell.content = Content::Spill(v);
                    }
                }
            }
        }
        work
    }

    /// Visits existing cells inside `r`, column by column.
    pub fn for_each_in(&self, r: &Range, mut f: impl FnMut(CellRef, &Cell)) {
        let last = (r.end.col as usize).min(self.cols.len().saturating_sub(1));
        if self.cols.is_empty() || r.start.col as usize > last {
            return;
        }
        for ci in r.start.col as usize..=last {
            for (row, cell) in self.cols[ci].range(r.start.row, r.end.row) {
                f(CellRef::new(row, ci as u32), cell);
            }
        }
    }

    /// Replaces the format of every cell (moving a sheet between books).
    pub fn restyle(&mut self, mut f: impl FnMut(u32) -> u32) {
        for cell in self.cols.iter_mut().flat_map(crate::column::Column::values_mut) {
            cell.style = f(cell.style);
        }
    }

    /// The existing cells of column `col` from row `lo` to row `hi`, in
    /// order (a search can stop at the first it wants).
    pub fn column_cells(&self, col: u32, lo: u32, hi: u32) -> impl Iterator<Item = (u32, &Cell)> {
        self.cols.get(col as usize).into_iter().flat_map(move |c| c.range(lo, hi))
    }

    pub fn iter(&self) -> impl Iterator<Item = (CellRef, &Cell)> {
        self.cols.iter().enumerate().flat_map(|(ci, col)| col.iter().map(move |(r, c)| (CellRef::new(r, ci as u32), c)))
    }

    /// Moves the cells of `other` here, where its rows are below this
    /// sheet's (importers that read parts of a sheet apart and put them
    /// together): whole blocks of cells instead of a cell at a time.
    pub fn append_cells(&mut self, other: &mut Sheet) {
        let cols = std::mem::take(&mut other.cols);
        if self.cols.len() < cols.len() {
            self.cols.resize_with(cols.len(), Default::default);
        }
        for (mine, col) in self.cols.iter_mut().zip(cols) {
            mine.append(col);
        }
    }

    pub fn cell_count(&self) -> usize {
        self.cols.iter().map(crate::column::Column::len).sum()
    }

    /// Bottom-right corner of used cells, or `None` if the sheet is empty.
    pub fn extent(&self) -> Option<CellRef> {
        let mut max_row = None;
        let mut max_col = None;
        for (ci, col) in self.cols.iter().enumerate() {
            if let Some(r) = col.last_row() {
                max_row = Some(max_row.map_or(r, |m: u32| m.max(r)));
                max_col = Some(ci as u32);
            }
        }
        Some(CellRef::new(max_row?, max_col?))
    }

    /// Next non-empty cell boundary in a direction (Ctrl+Arrow behaviour).
    pub fn jump(&self, from: CellRef, drow: i32, dcol: i32, max: CellRef) -> CellRef {
        let filled = |c: CellRef| self.get(c).is_some_and(|x| !x.value().is_empty());
        let step = |c: CellRef| -> Option<CellRef> {
            let r = c.row as i64 + drow as i64;
            let k = c.col as i64 + dcol as i64;
            (r >= 0 && k >= 0 && r <= max.row as i64 && k <= max.col as i64).then(|| CellRef::new(r as u32, k as u32))
        };
        let Some(mut next) = step(from) else { return from };
        if filled(from) && filled(next) {
            // Move to the last filled cell of this block.
            let mut cur = next;
            while let Some(n) = step(cur) {
                if !filled(n) {
                    break;
                }
                cur = n;
            }
            return cur;
        }
        // Skip empties to the next filled cell (or the edge).
        loop {
            if filled(next) {
                return next;
            }
            match step(next) {
                Some(n) => next = n,
                None => return next,
            }
        }
    }

    pub fn col_width(&self, c: u32) -> f32 {
        *self.col_widths.get(&c).unwrap_or(&self.default_col_width)
    }

    pub fn row_height(&self, r: u32) -> f32 {
        *self.row_heights.get(&r).unwrap_or(&self.default_row_height)
    }

    /// Left edge of column `c` in pixels.
    pub fn col_x(&self, c: u32) -> f32 {
        self.col_widths.offset(c, self.default_col_width)
    }

    pub fn row_y(&self, r: u32) -> f32 {
        self.row_heights.offset(r, self.default_row_height)
    }

    /// Column containing pixel `x` (clamped to >= 0).
    pub fn col_at(&self, x: f32) -> u32 {
        self.col_widths.index_at(x, self.default_col_width)
    }

    pub fn row_at(&self, y: f32) -> u32 {
        self.row_heights.index_at(y, self.default_row_height)
    }

    // --- structural edits (cell storage only; formulas are handled by the workbook) ---

    /// Inserts or deletes one row at each of `points` (ascending, in
    /// coordinates before the edit; inserts may repeat a point). Returns the
    /// removed cells.
    pub(crate) fn shift_rows_at(&mut self, points: &[u32], insert: bool) -> Vec<(CellRef, Cell)> {
        let Some(&first) = points.first() else { return Vec::new() };
        let mut removed = Vec::new();
        for (ci, col) in self.cols.iter_mut().enumerate() {
            for (r, cell) in col.split_off(first) {
                match map_index(r, points, insert) {
                    Some(nr) if nr < crate::addr::MAX_ROWS => {
                        col.insert(nr, cell);
                    }
                    Some(_) => {}
                    None => removed.push((CellRef::new(r, ci as u32), cell)),
                }
            }
        }
        remap_map(self.row_heights.map_mut(), points, insert);
        remap_map(&mut self.row_levels, points, insert);
        // Areas are few: shift them one point at a time, bottom up.
        for &p in points.iter().rev() {
            self.shift_areas(true, p, if insert { 1 } else { -1 });
        }
        removed
    }

    pub(crate) fn shift_rows(&mut self, at: u32, count: i64) -> Vec<(CellRef, Cell)> {
        let mut removed = Vec::new();
        for (ci, col) in self.cols.iter_mut().enumerate() {
            for (r, cell) in col.split_off(at) {
                if count < 0 && (r as i64) < at as i64 - count {
                    removed.push((CellRef::new(r, ci as u32), cell));
                    continue;
                }
                let nr = r as i64 + count;
                if nr < crate::addr::MAX_ROWS as i64 {
                    col.insert(nr as u32, cell);
                }
            }
        }
        shift_map(self.row_heights.map_mut(), at, count);
        shift_map(&mut self.row_levels, at, count);
        self.shift_areas(true, at, count);
        removed
    }

    pub(crate) fn shift_cols(&mut self, at: u32, count: i64) -> Vec<(CellRef, Cell)> {
        let mut removed = Vec::new();
        let at_us = at as usize;
        if at_us < self.cols.len() {
            if count > 0 {
                for _ in 0..count {
                    self.cols.insert(at_us, Default::default());
                }
                self.cols.truncate(crate::addr::MAX_COLS as usize);
            } else {
                let end = (at_us + (-count) as usize).min(self.cols.len());
                for (i, col) in self.cols.drain(at_us..end).enumerate() {
                    for (r, cell) in col.into_cells() {
                        removed.push((CellRef::new(r, (at_us + i) as u32), cell));
                    }
                }
            }
        }
        shift_map(self.col_widths.map_mut(), at, count);
        shift_map(&mut self.col_levels, at, count);
        self.shift_areas(false, at, count);
        removed
    }

    /// Moves merges, the filter, validations and conditional formats.
    fn shift_areas(&mut self, rows: bool, at: u32, count: i64) {
        use crate::data::{shift_range, shift_ranges};
        self.merges = shift_ranges(&self.merges, rows, at, count);
        if let Some(f) = &mut self.filter {
            match shift_range(f.range, rows, at, count) {
                Some(r) => {
                    f.range = r;
                    if !rows {
                        f.cols.retain_mut(|c| match shift_range(Range::single(CellRef::new(0, c.col)), false, at, count) {
                            Some(n) => {
                                c.col = n.start.col;
                                true
                            }
                            None => false,
                        });
                    }
                }
                None => self.filter = None,
            }
        }
        for v in &mut self.validations {
            v.ranges = shift_ranges(&v.ranges, rows, at, count);
        }
        self.validations.retain(|v| !v.ranges.is_empty());
        for c in &mut self.cond_formats {
            c.ranges = shift_ranges(&c.ranges, rows, at, count);
        }
        self.cond_formats.retain(|c| !c.ranges.is_empty());
        for e in &mut self.edit_ranges {
            e.ranges = shift_ranges(&e.ranges, rows, at, count);
        }
        self.edit_ranges.retain(|e| !e.ranges.is_empty());
        for img in &mut self.images {
            img.shift(rows, at, count);
        }
        for c in &mut self.charts {
            c.shift(rows, at, count);
        }
        for s in &mut self.shapes {
            s.shift(rows, at, count);
        }
        for g in &mut self.sparklines {
            g.shift(rows, at, count);
        }
        self.sparklines.retain(|g| !g.sparklines.is_empty());
        self.tables.retain_mut(|t| t.shift(rows, at, count));
        self.cell_watches.retain_mut(|c| match shift_range(Range::single(*c), rows, at, count) {
            Some(r) => {
                *c = r.start;
                true
            }
            None => false,
        });
    }

    /// The table that holds cell `c`.
    pub fn table_at(&self, c: CellRef) -> Option<&crate::table::Table> {
        self.tables.iter().find(|t| t.range.contains(c))
    }

    /// An id no picture of the sheet uses.
    pub fn next_image_id(&self) -> u32 {
        self.images.iter().map(|i| i.id).max().unwrap_or(0) + 1
    }

    pub fn image(&self, id: u32) -> Option<&crate::image::SheetImage> {
        self.images.iter().find(|i| i.id == id)
    }
}

/// Where index `v` goes when one row is inserted (or deleted) at each of
/// `points`; `None` if it is deleted.
pub(crate) fn map_index(v: u32, points: &[u32], insert: bool) -> Option<u32> {
    if insert {
        Some(v + points.partition_point(|&p| p <= v) as u32)
    } else if points.binary_search(&v).is_ok() {
        None
    } else {
        Some(v - points.partition_point(|&p| p < v) as u32)
    }
}

fn remap_map<V>(m: &mut BTreeMap<u32, V>, points: &[u32], insert: bool) {
    let Some(&first) = points.first() else { return };
    let tail = m.split_off(&first);
    let mut moved: BTreeMap<u32, V> = tail.into_iter().filter_map(|(k, v)| map_index(k, points, insert).filter(|&n| n < crate::addr::MAX_ROWS).map(|n| (n, v))).collect();
    m.append(&mut moved);
}

fn shift_map<V>(m: &mut BTreeMap<u32, V>, at: u32, count: i64) {
    let tail = m.split_off(&at);
    for (k, v) in tail {
        if count < 0 && (k as i64) < at as i64 - count {
            continue;
        }
        m.insert((k as i64 + count) as u32, v);
    }
}

#[cfg(test)]
fn index_at(pos: f32, default: f32, overrides: &BTreeMap<u32, f32>) -> u32 {
    if pos <= 0.0 {
        return 0;
    }
    // Walk overrides in order; between them sizes are uniform.
    let mut idx: u32 = 0;
    let mut y = 0.0f32;
    for (&k, &size) in overrides {
        let span = (k - idx) as f32 * default;
        if y + span > pos {
            return idx + ((pos - y) / default) as u32;
        }
        y += span;
        if y + size > pos {
            return k;
        }
        y += size;
        idx = k + 1;
    }
    idx + ((pos - y) / default) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry() {
        let mut s = Sheet::new("S");
        s.col_widths.insert(1, 200.0);
        assert_eq!(s.col_x(0), 0.0);
        assert_eq!(s.col_x(2), DEFAULT_COL_WIDTH + 200.0);
        assert_eq!(s.col_at(DEFAULT_COL_WIDTH + 10.0), 1);
        assert_eq!(s.col_at(DEFAULT_COL_WIDTH + 199.0), 1);
        assert_eq!(s.col_at(DEFAULT_COL_WIDTH + 201.0), 2);
        assert_eq!(s.col_at(DEFAULT_COL_WIDTH * 3.0 + 201.0), 4);
    }

    #[test]
    fn jump() {
        let mut s = Sheet::new("S");
        for r in 2..6 {
            s.put(CellRef::new(r, 0), Some(Cell::new(Content::Value(Value::Number(1.0)), 0)));
        }
        let max = CellRef::new(100, 10);
        assert_eq!(s.jump(CellRef::new(0, 0), 1, 0, max), CellRef::new(2, 0));
        assert_eq!(s.jump(CellRef::new(2, 0), 1, 0, max), CellRef::new(5, 0));
        assert_eq!(s.jump(CellRef::new(5, 0), 1, 0, max), CellRef::new(100, 0));
    }

    #[test]
    fn sizes_index_matches_linear_scan() {
        let mut sh = Sheet::new("S");
        for r in [0u32, 3, 4, 10, 11, 12, 500] {
            sh.row_heights.insert(r, 5.0 + (r % 7) as f32 * 9.0);
        }
        sh.row_heights.insert(7, 0.0); // hidden row
        let d = sh.default_row_height;
        for r in 0..600 {
            let linear = r as f32 * d + sh.row_heights.range(..r).map(|(_, h)| h - d).sum::<f32>();
            assert!((sh.row_y(r) - linear).abs() < 1e-3, "row_y({r})");
        }
        let mut y = 0.0;
        while y < 13_000.0 {
            assert_eq!(sh.row_at(y), index_at(y, d, &sh.row_heights), "row_at({y})");
            y += 1.7;
        }
    }

    #[test]
    fn shown_rows_skip_hidden_ones() {
        let mut sh = Sheet::new("S");
        for r in 5..900_000 {
            sh.row_heights.insert(r, 0.0);
        }
        sh.row_heights.insert(3, 40.0);
        sh.row_heights.insert(700_000, 30.0);
        assert_eq!(sh.row_heights.shown(2, 999_999).len(), 3 + 1 + 100_000);
        assert_eq!(sh.row_heights.shown(0, 900_001)[..6], [0, 1, 2, 3, 4, 700_000]);
        assert_eq!(sh.row_heights.shown(10, 20), Vec::<u32>::new());
        assert_eq!(sh.row_heights.shown(4, 4), vec![4]);
        assert_eq!(sh.row_heights.shown(9, 8), Vec::<u32>::new());
    }
}
