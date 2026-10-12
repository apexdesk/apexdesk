//! Higher-level editing commands: paste special, shifting cells, merging,
//! fills and series, multi-key sort, sheet management.

use super::*;
use crate::sheet::{HAlign, Line};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PasteWhat {
    #[default]
    All,
    Values,
    Formulas,
    Formats,
    AllExceptBorders,
    /// Notes only.
    Comments,
    /// Data validation rules only (see `Workbook::paste_validation`).
    Validation,
    /// Column widths only (see `Workbook::paste_col_widths`).
    ColumnWidths,
}

/// Arithmetic combining pasted numbers with the destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PasteOp {
    #[default]
    None,
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PasteSpecial {
    pub what: PasteWhat,
    pub op: PasteOp,
    pub skip_blanks: bool,
    pub transpose: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillDir {
    Down,
    Right,
    Up,
    Left,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeriesKind {
    Linear,
    Growth,
    /// Date steps: days, weekdays, months, years.
    Days,
    Weekdays,
    Months,
    Years,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeriesOpts {
    /// Fill down the columns (else right along the rows).
    pub down: bool,
    pub kind: SeriesKind,
    pub step: f64,
    pub stop: Option<f64>,
}

/// Border buttons of the toolbar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BorderPreset {
    None,
    All,
    Outside,
    Inside,
    InsideH,
    InsideV,
    Left,
    Right,
    Top,
    Bottom,
    TopBottom,
    ThickBox,
    DoubleBottom,
}

/// One sort level. `col` is a row when sorting left to right.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SortKey {
    pub col: u32,
    pub ascending: bool,
    pub on: SortOn,
    /// Custom order (a list like "Low, Mid, High"); values not in it go after.
    pub list: Option<Vec<String>>,
}

impl SortKey {
    pub fn new(col: u32, ascending: bool) -> SortKey {
        SortKey { col, ascending, ..Default::default() }
    }
}

/// What a sort level compares. For colors and icons, `ascending` puts the
/// matching cells on top.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SortOn {
    #[default]
    Values,
    /// Cells with this fill (`None` = no fill).
    CellColor(Option<u32>),
    FontColor(Option<u32>),
    /// Conditional formatting icon index.
    Icon(u8),
}

pub(super) fn offset_formula(mut cell: Cell, dr: i64, dc: i64) -> Cell {
    if let Content::Formula { expr, .. } = &mut cell.content {
        *expr = expr.offset(dr, dc);
    }
    cell
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => Some(*n),
        Value::Empty => Some(0.0),
        _ => None,
    }
}

impl Workbook {
    /// Pastes `clip` at `at` with paste-special options. Returns the pasted area.
    pub fn paste_special(&mut self, sheet: usize, at: CellRef, clip: &Clip, o: PasteSpecial) -> Range {
        let (rows, cols) = if o.transpose { (clip.cols, clip.rows) } else { (clip.rows, clip.cols) };
        if matches!(o.what, PasteWhat::Validation | PasteWhat::ColumnWidths) {
            // They need the source sheet: the caller uses their own functions.
            return Range::single(at);
        }
        let end = CellRef::new((at.row + rows.max(1) - 1).min(MAX_ROWS - 1), (at.col + cols.max(1) - 1).min(MAX_COLS - 1));
        let area = Range::new(at, end);
        let mut src: HashMap<(u32, u32), &Cell> = HashMap::default();
        for (dr, dc, cell) in &clip.cells {
            let k = if o.transpose { (*dc, *dr) } else { (*dr, *dc) };
            src.insert(k, cell);
        }
        let mut cells = Vec::new();
        for r in 0..=(end.row - at.row) {
            for c in 0..=(end.col - at.col) {
                let dst = CellRef::new(at.row + r, at.col + c);
                let s = src.get(&(r, c)).copied();
                let blank = s.is_none_or(|x| matches!(x.content, Content::Empty));
                if o.skip_blanks && blank {
                    continue;
                }
                let old = self.sheets[sheet].get(dst).cloned();
                let old_style = old.as_ref().map_or(0, |x| x.style);
                let src_style = s.map_or(0, |x| x.style);
                // Source position of this cell, for relative references.
                let (sr, sc) = if o.transpose { (clip.origin.row + c, clip.origin.col + r) } else { (clip.origin.row + r, clip.origin.col + c) };
                let moved = s.map(|x| offset_formula(x.clone(), dst.row as i64 - sr as i64, dst.col as i64 - sc as i64));
                let mut content = match o.what {
                    PasteWhat::Formats | PasteWhat::Comments => old.as_ref().map_or(Content::Empty, |x| x.content.clone()),
                    PasteWhat::Values => s.map_or(Content::Empty, |x| match x.value() {
                        Value::Empty => Content::Empty,
                        v => Content::Value(v.clone()),
                    }),
                    _ => moved.map_or(Content::Empty, |x| x.content),
                };
                if o.op != PasteOp::None && !matches!(o.what, PasteWhat::Formats | PasteWhat::Comments) {
                    let a = old.as_ref().map_or(Value::Empty, |x| x.value().clone());
                    let b = s.map_or(Value::Empty, |x| x.value().clone());
                    if let (Some(x), Some(y)) = (num(&a), num(&b)) {
                        let v = match o.op {
                            PasteOp::Add => Value::Number(x + y),
                            PasteOp::Sub => Value::Number(x - y),
                            PasteOp::Mul => Value::Number(x * y),
                            PasteOp::Div if y == 0.0 => Value::Error(ErrorKind::Div0),
                            PasteOp::Div => Value::Number(x / y),
                            PasteOp::None => unreachable!(),
                        };
                        content = Content::Value(v);
                    }
                }
                let style = match o.what {
                    PasteWhat::All | PasteWhat::Formats => src_style,
                    PasteWhat::Values | PasteWhat::Formulas | PasteWhat::Comments | PasteWhat::Validation | PasteWhat::ColumnWidths => old_style,
                    PasteWhat::AllExceptBorders => {
                        let mut st = self.styles.get(src_style).clone();
                        st.borders = self.styles.get(old_style).borders;
                        self.styles.intern(st)
                    }
                };
                // Everything brings the source's note and link along.
                let extra = match o.what {
                    PasteWhat::All | PasteWhat::AllExceptBorders => s.and_then(|x| x.extra.clone()),
                    PasteWhat::Comments => {
                        let mut cell = old.clone().unwrap_or_else(|| Cell::new(Content::Empty, 0));
                        cell.set_extra(|e| e.note = s.and_then(|x| x.note().cloned()));
                        cell.extra
                    }
                    _ => old.as_ref().and_then(|x| x.extra.clone()),
                };
                let new = Some(Cell { content, style, extra }).filter(|c| !c.is_blank());
                if new.is_some() || old.is_some() {
                    cells.push((dst, new));
                }
            }
        }
        if !cells.is_empty() {
            self.execute_as(Op::SetCells { sheet, cells }, "undo.paste");
        }
        area
    }

    /// Moves the cells in the band starting at `r` by `(dr, dc)`; cells in
    /// `clear` are emptied first. Used to insert/delete cells.
    fn shift_block(&mut self, sheet: usize, band: Range, dr: i64, dc: i64, clear: Option<Range>, label: &'static str) {
        let mut moved: Vec<(CellRef, Cell)> = Vec::new();
        if let Some(b) = self.clamp_to_used(sheet, &band) {
            self.sheets[sheet].for_each_in(&b, |c, cell| moved.push((c, cell.clone())));
        }
        let mut out: HashMap<CellRef, Option<Cell>> = HashMap::default();
        if let Some(cl) = clear.and_then(|c| self.clamp_to_used(sheet, &c)) {
            self.sheets[sheet].for_each_in(&cl, |c, _| {
                out.insert(c, None);
            });
        }
        for (c, _) in &moved {
            out.insert(*c, None);
        }
        for (c, cell) in moved {
            let (r, k) = (c.row as i64 + dr, c.col as i64 + dc);
            if r >= 0 && k >= 0 && r < MAX_ROWS as i64 && k < MAX_COLS as i64 {
                out.insert(CellRef::new(r as u32, k as u32), Some(cell));
            }
        }
        let mut cells: Vec<_> = out.into_iter().filter(|(c, v)| v.is_some() || self.sheets[sheet].get(*c).is_some()).collect();
        cells.sort_by_key(|(c, _)| *c);
        if !cells.is_empty() {
            self.execute_as(Op::SetCells { sheet, cells }, label);
        }
    }

    /// Inserts blank cells at `r`, shifting the rest down or right.
    pub fn insert_cells(&mut self, sheet: usize, r: &Range, down: bool) {
        let band = if down {
            Range::new(r.start, CellRef::new(MAX_ROWS - 1, r.end.col))
        } else {
            Range::new(r.start, CellRef::new(r.end.row, MAX_COLS - 1))
        };
        let (dr, dc) = if down { (r.rows() as i64, 0) } else { (0, r.cols() as i64) };
        self.shift_block(sheet, band, dr, dc, None, "undo.insert_cells");
    }

    /// Deletes the cells in `r`, shifting the rest up or left.
    pub fn delete_cells(&mut self, sheet: usize, r: &Range, up: bool) {
        let band = if up {
            Range::new(CellRef::new(r.end.row + 1, r.start.col), CellRef::new(MAX_ROWS - 1, r.end.col))
        } else {
            Range::new(CellRef::new(r.start.row, r.end.col + 1), CellRef::new(r.end.row, MAX_COLS - 1))
        };
        let (dr, dc) = if up { (-(r.rows() as i64), 0) } else { (0, -(r.cols() as i64)) };
        if (up && r.end.row + 1 >= MAX_ROWS) || (!up && r.end.col + 1 >= MAX_COLS) {
            self.clear_all(sheet, r);
            return;
        }
        self.shift_block(sheet, band, dr, dc, Some(*r), "undo.delete_cells");
    }

    /// Removes cells completely (contents and formatting).
    pub fn clear_all(&mut self, sheet: usize, r: &Range) {
        let mut cells = Vec::new();
        if let Some(r) = self.clamp_to_used(sheet, r) {
            self.sheets[sheet].for_each_in(&r, |c, _| cells.push((c, None)));
        }
        if !cells.is_empty() {
            self.execute_as(Op::SetCells { sheet, cells }, "undo.clear");
        }
    }

    /// Removes formatting, keeping contents.
    pub fn clear_formats(&mut self, sheet: usize, r: &Range) {
        let mut cells = Vec::new();
        if let Some(r) = self.clamp_to_used(sheet, r) {
            self.sheets[sheet].for_each_in(&r, |c, cell| {
                if cell.style != 0 {
                    cells.push((c, Cell::with_style(Some(cell), 0)));
                }
            });
        }
        if !cells.is_empty() {
            self.execute_as(Op::SetCells { sheet, cells }, "undo.clear_formats");
        }
    }

    /// Draws (or with `BorderPreset::None` removes) borders over `r`.
    pub fn apply_borders(&mut self, sheet: usize, r: &Range, preset: BorderPreset, line: Line) {
        use crate::sheet::{LineStyle, border as b};
        let r = if r.cell_count() > 1_000_000 {
            match self.clamp_to_used(sheet, r) {
                Some(x) => x,
                None => return,
            }
        } else {
            *r
        };
        let line = match preset {
            BorderPreset::ThickBox => Line { style: LineStyle::Thick, ..line },
            BorderPreset::DoubleBottom => Line { style: LineStyle::Double, ..line },
            BorderPreset::None => Line::default(),
            _ => line,
        };
        let mut cache: HashMap<(u32, u8), u32> = HashMap::default();
        let mut cells = Vec::new();
        for row in r.start.row..=r.end.row {
            for col in r.start.col..=r.end.col {
                let (top, bottom, left, right) = (row == r.start.row, row == r.end.row, col == r.start.col, col == r.end.col);
                let edge = |on: bool, f: u8| if on { f } else { 0 };
                let outside = edge(top, b::TOP) | edge(bottom, b::BOTTOM) | edge(left, b::LEFT) | edge(right, b::RIGHT);
                let inner_h = edge(!top, b::TOP) | edge(!bottom, b::BOTTOM);
                let inner_v = edge(!left, b::LEFT) | edge(!right, b::RIGHT);
                let flags = match preset {
                    BorderPreset::None | BorderPreset::All => b::ALL,
                    BorderPreset::Outside | BorderPreset::ThickBox => outside,
                    BorderPreset::Inside => inner_h | inner_v,
                    BorderPreset::InsideH => inner_h,
                    BorderPreset::InsideV => inner_v,
                    BorderPreset::Left => edge(left, b::LEFT),
                    BorderPreset::Right => edge(right, b::RIGHT),
                    BorderPreset::Top => edge(top, b::TOP),
                    BorderPreset::Bottom | BorderPreset::DoubleBottom => edge(bottom, b::BOTTOM),
                    BorderPreset::TopBottom => edge(top, b::TOP) | edge(bottom, b::BOTTOM),
                };
                if flags == 0 {
                    continue;
                }
                let c = CellRef::new(row, col);
                let old = self.sheets[sheet].get(c).cloned();
                let old_style = old.as_ref().map_or(0, |x| x.style);
                let style = *cache.entry((old_style, flags)).or_insert_with(|| {
                    let mut st = self.styles.get(old_style).clone();
                    st.borders.set(flags, line);
                    self.styles.intern(st)
                });
                if style != old_style {
                    cells.push((c, Cell::with_style(old.as_ref(), style)));
                }
            }
        }
        if !cells.is_empty() {
            self.execute_as(Op::SetCells { sheet, cells }, "undo.borders");
        }
    }

    /// Whether merging `r` would discard values other than the top-left one.
    pub fn merge_loses_data(&self, sheet: usize, r: &Range, across: bool) -> bool {
        let mut lose = false;
        self.sheets[sheet].for_each_in(r, |c, cell| {
            let keeper = c.col == r.start.col && (across || c.row == r.start.row);
            if !keeper && !cell.value().is_empty() {
                lose = true;
            }
        });
        lose
    }

    /// Merges `r` (or each of its rows with `across`); clears the covered
    /// cells' contents like other spreadsheets. `center` also centers it.
    pub fn merge(&mut self, sheet: usize, r: &Range, across: bool, center: bool) {
        let parts: Vec<Range> = if across {
            (r.start.row..=r.end.row).map(|row| Range::new(CellRef::new(row, r.start.col), CellRef::new(row, r.end.col))).collect()
        } else {
            vec![*r]
        };
        let parts: Vec<Range> = parts.into_iter().filter(|p| !p.is_single()).collect();
        if parts.is_empty() {
            return;
        }
        let mut merges: Vec<Range> = self.sheets[sheet].merges.iter().filter(|m| m.intersect(r).is_none()).copied().collect();
        merges.extend(parts.iter().copied());
        let mut ops = vec![Op::Merges { sheet, merges }];
        let mut cells = Vec::new();
        for p in &parts {
            self.sheets[sheet].for_each_in(p, |c, cell| {
                if c != p.start && !matches!(cell.content, Content::Empty) {
                    cells.push((c, Cell::with_content(Some(cell), Content::Empty)));
                }
            });
        }
        if center {
            for p in &parts {
                let old = self.sheets[sheet].get(p.start).cloned();
                let mut st = self.styles.get(old.as_ref().map_or(0, |c| c.style)).clone();
                st.halign = HAlign::Center;
                let style = self.styles.intern(st);
                cells.push((p.start, Cell::with_style(old.as_ref(), style)));
            }
        }
        if !cells.is_empty() {
            ops.push(Op::SetCells { sheet, cells });
        }
        self.execute_as(Op::Batch(ops), "undo.merge");
    }

    /// Removes merged areas that intersect `r`.
    pub fn unmerge(&mut self, sheet: usize, r: &Range) {
        let merges: Vec<Range> = self.sheets[sheet].merges.iter().filter(|m| m.intersect(r).is_none()).copied().collect();
        if merges.len() != self.sheets[sheet].merges.len() {
            self.execute_as(Op::Merges { sheet, merges }, "undo.unmerge");
        }
    }

    /// Copies the first row/column of `r` (last for up/left) across it.
    pub fn fill_dir(&mut self, sheet: usize, r: &Range, dir: FillDir) {
        let vertical = matches!(dir, FillDir::Down | FillDir::Up);
        if (vertical && r.rows() < 2) || (!vertical && r.cols() < 2) {
            return;
        }
        let src_line = match dir {
            FillDir::Down => r.start.row,
            FillDir::Up => r.end.row,
            FillDir::Right => r.start.col,
            FillDir::Left => r.end.col,
        };
        let mut cells = Vec::new();
        for row in r.start.row..=r.end.row {
            for col in r.start.col..=r.end.col {
                let (sr, sc) = if vertical { (src_line, col) } else { (row, src_line) };
                if (sr, sc) == (row, col) {
                    continue;
                }
                let cell = self.sheets[sheet].get(CellRef::new(sr, sc)).cloned().map(|c| offset_formula(c, row as i64 - sr as i64, col as i64 - sc as i64));
                if cell.is_some() || self.sheets[sheet].get(CellRef::new(row, col)).is_some() {
                    cells.push((CellRef::new(row, col), cell));
                }
            }
        }
        if !cells.is_empty() {
            self.execute_as(Op::SetCells { sheet, cells }, "undo.fill");
        }
    }

    /// Copies cell `src` over the rest of `r`, its relative references
    /// moved (Ctrl+Enter).
    pub fn fill_from(&mut self, sheet: usize, r: &Range, src: CellRef) {
        let Some(cell) = self.sheets[sheet].get(src).cloned() else { return };
        let mut cells = Vec::new();
        for row in r.start.row..=r.end.row {
            for col in r.start.col..=r.end.col {
                if (row, col) != (src.row, src.col) {
                    cells.push((CellRef::new(row, col), Some(offset_formula(cell.clone(), row as i64 - src.row as i64, col as i64 - src.col as i64))));
                }
            }
        }
        if !cells.is_empty() {
            self.execute_as(Op::SetCells { sheet, cells }, "undo.fill");
        }
    }

    /// Fills a series from the first cell of each line of `r`.
    pub fn fill_series_opts(&mut self, sheet: usize, r: &Range, o: SeriesOpts) {
        let (lines, len) = if o.down { (r.cols(), r.rows()) } else { (r.rows(), r.cols()) };
        let mut cells = Vec::new();
        for line in 0..lines {
            let at = |i: u32| if o.down { CellRef::new(r.start.row + i, r.start.col + line) } else { CellRef::new(r.start.row + line, r.start.col + i) };
            let first = self.sheets[sheet].get(at(0)).cloned();
            let Some(Value::Number(start)) = first.as_ref().map(|c| c.value().clone()) else { continue };
            let style = first.map_or(0, |c| c.style);
            let mut v = start;
            for i in 1..len {
                v = match o.kind {
                    SeriesKind::Linear => v + o.step,
                    SeriesKind::Growth => v * o.step,
                    SeriesKind::Days => v + o.step,
                    SeriesKind::Weekdays => {
                        let mut d = v;
                        let mut n = o.step.round().max(1.0) as i64;
                        while n > 0 {
                            d += 1.0;
                            let (y, m, dd) = format::serial_to_date(d);
                            // 1970-01-01 was a Thursday; 0 = Sunday.
                            let wd = (format::days_from_civil(y, m, dd) + 4).rem_euclid(7);
                            if wd != 0 && wd != 6 {
                                n -= 1;
                            }
                        }
                        d
                    }
                    SeriesKind::Months | SeriesKind::Years => {
                        let months = if o.kind == SeriesKind::Years { o.step * 12.0 } else { o.step };
                        let (y, m, d) = format::serial_to_date(start);
                        let total = y as i64 * 12 + (m as i64 - 1) + (months * i as f64).round() as i64;
                        let (ny, nm) = (total.div_euclid(12), total.rem_euclid(12) + 1);
                        let dim = (format::date_to_serial(ny, nm + 1, 1) - format::date_to_serial(ny, nm, 1)) as u32;
                        format::date_to_serial(ny, nm, d.min(dim) as i64) + start.fract()
                    }
                };
                if o.stop.is_some_and(|s| if o.step >= 0.0 || o.kind == SeriesKind::Growth { v > s } else { v < s }) {
                    break;
                }
                cells.push((at(i), Some(Cell::new(Content::Value(Value::Number(v)), style))));
            }
        }
        if !cells.is_empty() {
            self.execute_as(Op::SetCells { sheet, cells }, "undo.fill");
        }
    }

    /// Sorts the rows of `r` by several keys.
    pub fn sort_by(&mut self, sheet: usize, r: &Range, keys: &[SortKey], case_sensitive: bool) {
        self.sort_range(sheet, r, keys, case_sensitive, false);
    }

    /// Fill, font color and conditional formatting icon of a cell, as seen
    /// (conditional formatting first).
    fn cell_marks(&self, sheet: usize, cf: Option<&crate::CfCache>, c: CellRef) -> (Option<u32>, Option<u32>, Option<u8>) {
        let res = cf.and_then(|cache| self.cf_eval(sheet, cache, c));
        let st = self.sheets[sheet].get(c).map(|x| self.styles.get(x.style));
        let fill = res.as_ref().and_then(|x| x.style.fill).or_else(|| st.and_then(|s| s.fill));
        let font = res.as_ref().and_then(|x| x.style.color).or_else(|| st.and_then(|s| s.color));
        (fill, font, res.and_then(|x| x.icon).map(|i| i.1))
    }

    /// The distinct fills, font colors and icons in line `k` of `r` (a
    /// column, or a row when `by_cols`), for the Sort dialog.
    pub fn sort_marks(&self, sheet: usize, r: &Range, k: u32, by_cols: bool) -> (Vec<Option<u32>>, Vec<Option<u32>>, Vec<u8>) {
        let cf = self.cf_prepare(sheet);
        let (mut fills, mut fonts, mut icons) = (Vec::new(), Vec::new(), Vec::new());
        let lines = if by_cols { r.start.col..=r.end.col } else { r.start.row..=r.end.row };
        for line in lines.take(100_000) {
            let c = if by_cols { CellRef::new(k, line) } else { CellRef::new(line, k) };
            let (f, t, i) = self.cell_marks(sheet, Some(&cf), c);
            if !fills.contains(&f) {
                fills.push(f);
            }
            if !fonts.contains(&t) {
                fonts.push(t);
            }
            if let Some(i) = i.filter(|i| !icons.contains(i)) {
                icons.push(i);
            }
        }
        (fills, fonts, icons)
    }

    /// Sorts the rows of `r` (or its columns when `by_cols`, left to right).
    pub fn sort_range(&mut self, sheet: usize, r: &Range, keys: &[SortKey], case_sensitive: bool, by_cols: bool) {
        use std::cmp::Ordering;
        let lines: Vec<u32> = if by_cols { (r.start.col..=r.end.col).collect() } else { (r.start.row..=r.end.row).collect() };
        let at = |line: u32, k: u32| if by_cols { CellRef::new(k, line) } else { CellRef::new(line, k) };
        let cf = keys.iter().any(|k| k.on != SortOn::Values).then(|| self.cf_prepare(sheet));
        let sh = &self.sheets[sheet];
        // Precomputed sort keys per line, so the comparison stays cheap.
        #[derive(PartialEq)]
        enum K<'a> {
            Match(bool),
            Val(&'a Value, Option<usize>),
        }
        let keyed: Vec<(u32, Vec<K>)> = lines
            .iter()
            .map(|&line| {
                let ks = keys
                    .iter()
                    .map(|k| {
                        let c = at(line, k.col);
                        match k.on {
                            SortOn::Values => {
                                let v = sh.value(c);
                                let pos = k.list.as_ref().and_then(|l| match v {
                                    Value::Empty => None,
                                    _ => {
                                        let t = self.display(sheet, c).0.to_lowercase();
                                        l.iter().position(|x| x.to_lowercase() == t)
                                    }
                                });
                                K::Val(v, pos)
                            }
                            SortOn::CellColor(want) => K::Match(self.cell_marks(sheet, cf.as_ref(), c).0 == want),
                            SortOn::FontColor(want) => K::Match(self.cell_marks(sheet, cf.as_ref(), c).1 == want),
                            SortOn::Icon(i) => K::Match(self.cell_marks(sheet, cf.as_ref(), c).2 == Some(i)),
                        }
                    })
                    .collect();
                (line, ks)
            })
            .collect();
        let mut order: Vec<usize> = (0..keyed.len()).collect();
        order.sort_by(|&a, &b| {
            for (i, k) in keys.iter().enumerate() {
                let o = match (&keyed[a].1[i], &keyed[b].1[i]) {
                    (K::Match(x), K::Match(y)) => {
                        let o = y.cmp(x);
                        if k.ascending { o } else { o.reverse() }
                    }
                    (K::Val(va, pa), K::Val(vb, pb)) => match (va.is_empty(), vb.is_empty()) {
                        // Empty cells always last.
                        (true, true) => Ordering::Equal,
                        (true, false) => Ordering::Greater,
                        (false, true) => Ordering::Less,
                        _ => {
                            let o = match (pa, pb) {
                                (Some(x), Some(y)) => x.cmp(y),
                                (Some(_), None) => Ordering::Less,
                                (None, Some(_)) => Ordering::Greater,
                                (None, None) => match (va, vb, case_sensitive) {
                                    (Value::Text(x), Value::Text(y), true) => x.cmp(y),
                                    _ => va.compare(vb),
                                },
                            };
                            if k.ascending { o } else { o.reverse() }
                        }
                    },
                    _ => Ordering::Equal,
                };
                if o != Ordering::Equal {
                    return o;
                }
            }
            Ordering::Equal
        });
        // Only the cells there are move (a whole row of a sheet used to
        // its last column is mostly empty): each place they leave is
        // cleared, unless another one comes to it.
        let first = if by_cols { r.start.col } else { r.start.row };
        let mut dest = vec![0u32; lines.len()];
        for (i, &src) in order.iter().enumerate() {
            dest[(keyed[src].0 - first) as usize] = lines[i];
        }
        let mut moved: Vec<(CellRef, Cell)> = Vec::new();
        sh.for_each_in(r, |c, cell| {
            let line = if by_cols { c.col } else { c.row };
            let dst = dest[(line - first) as usize];
            if dst != line {
                let d = dst as i64 - line as i64;
                let (dr, dc) = if by_cols { (0, d) } else { (d, 0) };
                moved.push((c, offset_formula(cell.clone(), dr, dc)));
            }
        });
        let mut out: HashMap<CellRef, Option<Cell>> = moved.iter().map(|(c, _)| (*c, None)).collect();
        for (c, cell) in moved {
            let (line, k) = if by_cols { (c.col, c.row) } else { (c.row, c.col) };
            out.insert(at(dest[(line - first) as usize], k), Some(cell));
        }
        let mut cells: Vec<(CellRef, Option<Cell>)> = out.into_iter().collect();
        cells.sort_by_key(|(c, _)| (c.row, c.col));
        self.execute_as(Op::SetCells { sheet, cells }, "undo.sort");
    }

    /// Copies sheet `i` right after it; returns the new index.
    pub fn duplicate_sheet(&mut self, i: usize) -> usize {
        let mut sh = self.sheets[i].clone();
        let base = format!("{} (", sh.name);
        sh.name = (2..).map(|n| format!("{base}{n})")).find(|n| self.sheet_index(n).is_none()).unwrap();
        sh.hidden = false;
        self.execute_as(Op::AddSheet { index: i + 1, sheet: Box::new(sh) }, "undo.duplicate_sheet");
        i + 1
    }

    /// Copies sheet `i` of another workbook in at `index` (the end when
    /// past it), bringing the formats its cells use; returns the new index.
    /// References to other sheets keep their names.
    pub fn adopt_sheet(&mut self, from: &Workbook, i: usize, index: usize) -> usize {
        let mut sh = from.sheets[i].clone();
        let mut ids: HashMap<u32, u32> = HashMap::default();
        sh.restyle(|id| *ids.entry(id).or_insert_with(|| self.styles.intern(from.styles.get(id).clone())));
        if self.sheet_index(&sh.name).is_some() {
            let base = format!("{} (", sh.name);
            sh.name = (2..).map(|n| format!("{base}{n})")).find(|n| self.sheet_index(n).is_none()).unwrap();
        }
        sh.hidden = false;
        let index = index.min(self.sheets.len());
        self.execute_as(Op::AddSheet { index, sheet: Box::new(sh) }, "undo.copy_sheet");
        index
    }

    /// A new workbook holding a copy of sheet `i` of `from` (Move or Copy
    /// to a new book), with its theme, locale and calculation options.
    pub fn book_from_sheet(from: &Workbook, i: usize) -> Workbook {
        let mut w = Workbook::empty();
        w.locale = from.locale;
        w.calc = from.calc;
        w.cell_styles = from.cell_styles.clone();
        w.adopt_sheet(from, i, 0);
        w.undo.clear();
        w.modified = true;
        w
    }

    /// Hides (size 0) or shows rows or columns.
    pub fn set_hidden(&mut self, sheet: usize, rows: bool, from: u32, to: u32, hide: bool) {
        let sizes: Vec<(u32, Option<f32>)> = (from..=to).map(|i| (i, hide.then_some(0.0))).collect();
        let (op, label) = if rows {
            (Op::RowHeights { sheet, sizes }, if hide { "undo.hide_rows" } else { "undo.show_rows" })
        } else {
            (Op::ColWidths { sheet, sizes }, if hide { "undo.hide_cols" } else { "undo.show_cols" })
        };
        self.execute_as(op, label);
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn manual_and_iterative_calculation() {
        let mut w = Workbook::new();
        let c = |a: &str| CellRef::parse(a).unwrap();
        let num = |w: &Workbook, a: &str| w.sheets[0].value(c(a)).as_number();
        w.set_input(0, c("A1"), "1");
        w.set_input(0, c("B1"), "=A1*2");
        w.set_input(0, c("C1"), "=B1+1");
        w.calc.manual = true;
        w.set_input(0, c("A1"), "5");
        assert_eq!(num(&w, "B1"), Some(2.0));
        assert!(w.needs_calc());
        // A new formula computes itself right away.
        w.set_input(0, c("D1"), "=A1+1");
        assert_eq!(num(&w, "D1"), Some(6.0));
        w.recalc_all();
        assert_eq!(num(&w, "C1"), Some(11.0));
        assert!(!w.needs_calc());
        w.calc.manual = false;
        // A circular reference: #CIRC! unless iterating.
        w.set_input(0, c("E1"), "=E2/2+10");
        w.set_input(0, c("E2"), "=E1");
        assert_eq!(w.circular_cell(), Some((0, c("E1"))));
        w.calc.iterate = true;
        w.recalc_all();
        assert!((num(&w, "E1").unwrap() - 20.0).abs() < 0.01, "{:?}", num(&w, "E1"));
        assert_eq!(w.circular_cell(), None);
    }


    #[test]
    fn sort_by_color_list_and_columns() {
        use crate::CellStyle;
        let mut w = Workbook::new();
        let c = |a: &str| CellRef::parse(a).unwrap();
        for (i, t) in ["High", "Low", "Mid", "Low"].iter().enumerate() {
            w.set_input(0, CellRef::new(i as u32, 0), t);
        }
        let r = Range::new(c("A1"), c("A4"));
        let list = vec!["Low".to_string(), "Mid".into(), "High".into()];
        w.sort_by(0, &r, &[SortKey { list: Some(list), ..SortKey::new(0, true) }], false);
        let col = |w: &Workbook| (0..4).map(|i| w.display(0, CellRef::new(i, 0)).0).collect::<Vec<_>>();
        assert_eq!(col(&w), ["Low", "Low", "Mid", "High"]);
        // Red cells on top, the rest keep their order.
        let red = w.styles.intern(CellStyle { fill: Some(0xFF0000), ..Default::default() });
        w.sheets[0].get_mut(c("A4")).unwrap().style = red;
        w.sort_by(0, &r, &[SortKey { on: SortOn::CellColor(Some(0xFF0000)), ..SortKey::new(0, true) }], false);
        assert_eq!(col(&w), ["High", "Low", "Low", "Mid"]);
        assert_eq!(w.sort_marks(0, &r, 0, false).0, [Some(0xFF0000), None]);
        // Left to right by row 1.
        let mut w = Workbook::new();
        for (i, t) in ["3", "1", "2"].iter().enumerate() {
            w.set_input(0, CellRef::new(0, i as u32), t);
            w.set_input(0, CellRef::new(1, i as u32), &format!("={}1*10", apex_addr(i as u32)));
        }
        w.sort_range(0, &Range::new(c("A1"), c("C2")), &[SortKey::new(0, true)], false, true);
        let row = |r: u32| (0..3).map(|i| w.display(0, CellRef::new(r, i)).0).collect::<Vec<_>>();
        assert_eq!(row(0), ["1", "2", "3"]);
        assert_eq!(row(1), ["10", "20", "30"]);
    }

    fn apex_addr(c: u32) -> String {
        crate::addr::col_name(c)
    }

    use super::*;

    fn wb() -> Workbook {
        let mut wb = Workbook::new();
        wb.set_inputs(0, vec![(CellRef::new(0, 0), "1".into()), (CellRef::new(0, 1), "2".into()), (CellRef::new(1, 0), "=A1*10".into())]);
        wb
    }

    fn val(wb: &Workbook, r: u32, c: u32) -> Value {
        wb.sheets[0].value(CellRef::new(r, c)).clone()
    }

    #[test]
    fn paste_special_modes() {
        let mut w = wb();
        let clip = w.copy(0, &Range::new(CellRef::new(0, 0), CellRef::new(1, 1)));
        // Transpose: A1:B2 -> D1:E2 with B1 going to D2.
        w.paste_special(0, CellRef::new(0, 3), &clip, PasteSpecial { transpose: true, ..Default::default() });
        assert_eq!(val(&w, 1, 3), Value::Number(2.0));
        assert_eq!(val(&w, 0, 3), Value::Number(1.0));
        // Values only: the formula result, not the formula.
        w.paste_special(0, CellRef::new(5, 0), &clip, PasteSpecial { what: PasteWhat::Values, ..Default::default() });
        assert!(w.sheets[0].get(CellRef::new(6, 0)).unwrap().formula().is_none());
        assert_eq!(val(&w, 6, 0), Value::Number(10.0));
        // Add onto the destination.
        w.paste_special(0, CellRef::new(5, 0), &clip, PasteSpecial { what: PasteWhat::Values, op: PasteOp::Add, ..Default::default() });
        assert_eq!(val(&w, 5, 0), Value::Number(2.0));
        assert_eq!(val(&w, 6, 0), Value::Number(20.0));
        assert_eq!(w.undo_history()[0], "undo.paste");
    }

    #[test]
    fn shift_cells() {
        let mut w = wb();
        w.insert_cells(0, &Range::single(CellRef::new(0, 0)), false);
        assert_eq!(val(&w, 0, 1), Value::Number(1.0));
        assert_eq!(val(&w, 0, 2), Value::Number(2.0));
        assert_eq!(val(&w, 0, 0), Value::Empty);
        w.delete_cells(0, &Range::single(CellRef::new(0, 0)), false);
        assert_eq!(val(&w, 0, 0), Value::Number(1.0));
        w.insert_cells(0, &Range::single(CellRef::new(0, 0)), true);
        assert_eq!(val(&w, 1, 0), Value::Number(1.0));
        w.undo();
        assert_eq!(val(&w, 0, 0), Value::Number(1.0));
    }

    #[test]
    fn merge_and_series() {
        let mut w = wb();
        let r = Range::new(CellRef::new(0, 0), CellRef::new(0, 1));
        assert!(w.merge_loses_data(0, &r, false));
        w.merge(0, &r, false, true);
        assert_eq!(w.sheets[0].merges, vec![r]);
        assert_eq!(val(&w, 0, 1), Value::Empty);
        w.unmerge(0, &r);
        assert!(w.sheets[0].merges.is_empty());
        w.set_input(0, CellRef::new(10, 0), "5");
        w.fill_series_opts(0, &Range::new(CellRef::new(10, 0), CellRef::new(14, 0)), SeriesOpts { down: true, kind: SeriesKind::Linear, step: 3.0, stop: Some(12.0) });
        assert_eq!(val(&w, 12, 0), Value::Number(11.0));
        assert_eq!(val(&w, 13, 0), Value::Empty);
        // Monthly dates keep the day (clamped to the month's length).
        w.set_input(0, CellRef::new(20, 0), "=DATE(2024,1,31)");
        let jan31 = format::date_to_serial(2024, 1, 31);
        w.set_inputs(0, vec![(CellRef::new(20, 0), format!("{jan31}"))]);
        w.fill_series_opts(0, &Range::new(CellRef::new(20, 0), CellRef::new(21, 0)), SeriesOpts { down: true, kind: SeriesKind::Months, step: 1.0, stop: None });
        assert_eq!(val(&w, 21, 0), Value::Number(format::date_to_serial(2024, 2, 29)));
    }

    #[test]
    fn fill_up_and_multi_sort() {
        let mut w = Workbook::new();
        w.set_inputs(0, vec![(CellRef::new(3, 0), "x".into())]);
        w.fill_dir(0, &Range::new(CellRef::new(0, 0), CellRef::new(3, 0)), FillDir::Up);
        assert_eq!(val(&w, 0, 0), Value::text("x"));
        let data = [("b", "2"), ("a", "2"), ("c", "1")];
        let inputs = data.iter().enumerate().flat_map(|(i, (k, v))| [(CellRef::new(10 + i as u32, 0), k.to_string()), (CellRef::new(10 + i as u32, 1), v.to_string())]).collect();
        w.set_inputs(0, inputs);
        w.sort_by(0, &Range::new(CellRef::new(10, 0), CellRef::new(12, 1)), &[SortKey::new(1, true), SortKey::new(0, true)], false);
        assert_eq!(val(&w, 10, 0), Value::text("c"));
        assert_eq!(val(&w, 11, 0), Value::text("a"));
        assert_eq!(val(&w, 12, 0), Value::text("b"));
        let i = w.duplicate_sheet(0);
        assert_eq!(w.sheets[i].name, "Sheet1 (2)");
    }

    #[test]
    fn auto_fill_options() {
        let mut w = Workbook::new();
        w.set_inputs(0, vec![(CellRef::new(0, 0), "1".into())]);
        w.modify_style(0, &Range::parse("A1").unwrap(), |s| s.bold = true);
        let (src, dst) = (Range::parse("A1").unwrap(), Range::parse("A1:A3").unwrap());
        w.auto_fill(0, &src, &dst, AutoFill::Smart);
        assert_eq!(val(&w, 2, 0), Value::Number(1.0));
        w.undo();
        w.auto_fill(0, &src, &dst, AutoFill::Series);
        assert_eq!(val(&w, 2, 0), Value::Number(3.0));
        w.undo();
        w.set_inputs(0, vec![(CellRef::new(2, 0), "x".into())]);
        w.auto_fill(0, &src, &dst, AutoFill::Formats);
        assert_eq!(val(&w, 2, 0), Value::text("x"));
        assert!(w.styles.get(w.cell(0, CellRef::new(2, 0)).unwrap().style).bold);
        w.undo();
        w.auto_fill(0, &src, &dst, AutoFill::Values);
        assert_eq!(val(&w, 1, 0), Value::Number(1.0));
        assert!(!w.styles.get(w.cell(0, CellRef::new(1, 0)).unwrap().style).bold);
        w.undo();
        let d = format::date_to_serial(2024, 1, 31);
        w.set_inputs(0, vec![(CellRef::new(0, 1), "2024-01-31".into())]);
        assert_eq!(val(&w, 0, 1), Value::Number(d));
        w.auto_fill(0, &Range::parse("B1").unwrap(), &Range::parse("B1:B3").unwrap(), AutoFill::Months);
        assert_eq!(val(&w, 1, 1), Value::Number(format::date_to_serial(2024, 2, 29)));
        w.undo();
        w.fill_series(0, &Range::parse("B1").unwrap(), &Range::parse("B1:B3").unwrap());
        assert_eq!(val(&w, 2, 1), Value::Number(d + 2.0));
    }

    #[test]
    fn adopt_sheet_from_another_book() {
        let mut a = Workbook::new();
        a.set_inputs(0, vec![(CellRef::new(0, 0), "5".into()), (CellRef::new(1, 0), "=A1*2".into())]);
        a.modify_style(0, &Range::parse("A1").unwrap(), |s| s.bold = true);
        let mut b = Workbook::new();
        b.modify_style(0, &Range::parse("C3").unwrap(), |s| s.italic = true);
        let k = b.adopt_sheet(&a, 0, 0);
        assert_eq!(k, 0);
        assert_eq!(b.sheets[0].name, "Sheet1 (2)");
        assert_eq!(b.sheets.len(), 2);
        assert!(b.styles.get(b.cell(0, CellRef::new(0, 0)).unwrap().style).bold);
        b.recalc_all();
        assert_eq!(val(&b, 1, 0), Value::Number(10.0));
        b.undo();
        assert_eq!(b.sheets.len(), 1);
        let c = Workbook::book_from_sheet(&a, 0);
        assert_eq!(c.sheets.len(), 1);
        assert_eq!(c.sheets[0].name, "Sheet1");
        assert!(!c.can_undo());
    }

    #[test]
    fn border_presets() {
        use crate::sheet::{LineStyle, border as b};
        let mut wb = Workbook::new();
        let r = Range::parse("B2:D4").unwrap();
        let flags = |wb: &Workbook, a: &str| wb.styles.get(wb.cell(0, CellRef::parse(a).unwrap()).map_or(0, |c| c.style)).borders.flags();
        wb.apply_borders(0, &r, BorderPreset::Outside, Line::THIN);
        assert_eq!(flags(&wb, "B2"), b::TOP | b::LEFT);
        assert_eq!(flags(&wb, "C3"), 0);
        assert_eq!(flags(&wb, "D4"), b::BOTTOM | b::RIGHT);
        wb.apply_borders(0, &r, BorderPreset::InsideH, Line::THIN);
        assert_eq!(flags(&wb, "C3"), b::TOP | b::BOTTOM);
        wb.apply_borders(0, &r, BorderPreset::DoubleBottom, Line::THIN);
        let st = wb.styles.get(wb.cell(0, CellRef::parse("C4").unwrap()).unwrap().style).clone();
        assert_eq!(st.borders.bottom.style, LineStyle::Double);
        wb.apply_borders(0, &r, BorderPreset::None, Line::THIN);
        assert_eq!(flags(&wb, "B2"), 0);
        assert_eq!(wb.undo_history()[0], "undo.borders");
    }
}
