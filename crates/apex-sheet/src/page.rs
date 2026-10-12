//! Page setup of a sheet and its division into printed pages.

use std::collections::BTreeSet;
use std::sync::Arc;

pub use apex_doc::{HeaderFooter, HfText};

use crate::{Range, Sheet};

/// Points per sheet pixel (sizes are kept at 96 dpi).
pub const PT_PER_PX: f32 = 0.75;

/// How a sheet is shown (Excel's `sheetView/@view`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ViewMode {
    #[default]
    Normal,
    PageBreakPreview,
    /// The cells laid out on the printed pages, with their margins and
    /// headers and footers.
    PageLayout,
}

/// A picture tiled behind the cells of a sheet. It is only shown on
/// screen, never printed (like Excel's sheet background).
#[derive(Clone, PartialEq)]
pub struct SheetBackground {
    pub data: Arc<[u8]>,
    pub format: crate::ImageFormat,
}

impl std::fmt::Debug for SheetBackground {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SheetBackground({:?}, {} bytes)", self.format, self.data.len())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SheetPage {
    /// Portrait paper size in points.
    pub paper_width: f32,
    pub paper_height: f32,
    pub landscape: bool,
    /// Margins in points.
    pub margin_top: f32,
    pub margin_bottom: f32,
    pub margin_left: f32,
    pub margin_right: f32,
    pub margin_header: f32,
    pub margin_footer: f32,
    /// Percent, 10..=400 (used when `fit` is `None`).
    pub scale: u16,
    /// Fit to this many pages wide and tall (0 = as many as needed).
    pub fit: Option<(u16, u16)>,
    /// Print areas, each printed on pages of its own (none: the used
    /// cells print).
    pub print_areas: Vec<Range>,
    /// Rows and columns repeated on every page (inclusive).
    pub title_rows: Option<(u32, u32)>,
    pub title_cols: Option<(u32, u32)>,
    pub center_h: bool,
    pub center_v: bool,
    pub gridlines: bool,
    pub headings: bool,
    /// Page order: across then down (instead of down then across).
    pub across_first: bool,
    /// Manual breaks: a page starts at each of these rows / columns.
    pub row_breaks: BTreeSet<u32>,
    pub col_breaks: BTreeSet<u32>,
    pub hf: HeaderFooter,
    /// First page number (`None` = 1).
    pub first_page: Option<u32>,
    /// Sheet background (screen only). Kept with the page setup: the ODF
    /// format stores it as the page style's background image.
    pub background: Option<SheetBackground>,
}

impl Default for SheetPage {
    fn default() -> Self {
        // Excel's defaults ("Normal" margins).
        SheetPage {
            paper_width: 595.3,
            paper_height: 841.9,
            landscape: false,
            margin_top: 54.0,
            margin_bottom: 54.0,
            margin_left: 50.4,
            margin_right: 50.4,
            margin_header: 21.6,
            margin_footer: 21.6,
            scale: 100,
            fit: None,
            print_areas: Vec::new(),
            title_rows: None,
            title_cols: None,
            center_h: false,
            center_v: false,
            gridlines: false,
            headings: false,
            across_first: false,
            row_breaks: BTreeSet::new(),
            col_breaks: BTreeSet::new(),
            hf: HeaderFooter::default(),
            first_page: None,
            background: None,
        }
    }
}

/// The printed pages of a sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct Pages {
    /// Area that is printed.
    pub area: Range,
    /// Column spans (inclusive) of the page columns, left to right.
    pub cols: Vec<(u32, u32)>,
    /// Row spans (inclusive) of the page rows, top to bottom.
    pub rows: Vec<(u32, u32)>,
    /// Scale applied to the sheet (1.0 = 100 %).
    pub scale: f32,
}

impl Pages {
    pub fn count(&self) -> usize {
        self.cols.len() * self.rows.len()
    }

    /// Row and column span of page `n` (0-based) in print order.
    pub fn page(&self, n: usize, across_first: bool) -> Option<((u32, u32), (u32, u32))> {
        if n >= self.count() {
            return None;
        }
        let (r, c) = if across_first { (n / self.cols.len(), n % self.cols.len()) } else { (n % self.rows.len(), n / self.rows.len()) };
        Some((self.rows[r], self.cols[c]))
    }
}

impl SheetPage {
    /// Paper size in points as printed (landscape swaps the sides).
    pub fn paper(&self) -> (f32, f32) {
        if self.landscape { (self.paper_height, self.paper_width) } else { (self.paper_width, self.paper_height) }
    }

    /// Printable size in points.
    pub fn content(&self) -> (f32, f32) {
        let (w, h) = self.paper();
        ((w - self.margin_left - self.margin_right).max(36.0), (h - self.margin_top - self.margin_bottom).max(36.0))
    }

    /// The first area printed: the first print area, or the used cells.
    pub fn area(&self, sheet: &Sheet) -> Option<Range> {
        self.areas(sheet).first().copied()
    }

    /// The areas printed, each on pages of its own: the print areas, or
    /// the used cells.
    pub fn areas(&self, sheet: &Sheet) -> Vec<Range> {
        if !self.print_areas.is_empty() {
            return self.print_areas.clone();
        }
        self.used_area(sheet).into_iter().collect()
    }

    fn used_area(&self, sheet: &Sheet) -> Option<Range> {
        // Pictures and charts print too, even past the last used cell.
        let pictures = sheet.images.iter().map(|i| i.last_cell(sheet)).chain(sheet.charts.iter().map(|c| c.last_cell(sheet))).chain(sheet.shapes.iter().map(|s| s.last_cell(sheet))).reduce(|a, b| crate::CellRef::new(a.row.max(b.row), a.col.max(b.col)));
        let end = match (sheet.extent(), pictures) {
            (Some(e), Some(p)) => crate::CellRef::new(e.row.max(p.row), e.col.max(p.col)),
            (e, p) => e.or(p)?,
        };
        let mut a = Range::new(crate::CellRef::new(0, 0), end);
        // Merged cells reaching past the used cells print whole.
        for m in &sheet.merges {
            if m.start.row <= a.end.row && m.start.col <= a.end.col {
                a.end.row = a.end.row.max(m.end.row);
                a.end.col = a.end.col.max(m.end.col);
            }
        }
        Some(a)
    }

    /// Splits the (first) printed area into pages.
    pub fn paginate(&self, sheet: &Sheet) -> Option<Pages> {
        Some(self.paginate_area(sheet, self.area(sheet)?))
    }

    /// The pages of every printed area, in print order: each area starts
    /// on a page of its own.
    pub fn paginate_all(&self, sheet: &Sheet) -> Vec<Pages> {
        self.areas(sheet).into_iter().map(|a| self.paginate_area(sheet, a)).collect()
    }

    fn paginate_area(&self, sheet: &Sheet, area: Range) -> Pages {
        let (cw, ch) = self.content();
        let px = |v: f32| v * PT_PER_PX;
        let title_w: f32 = self.title_cols.map_or(0.0, |(a, b)| (a..=b.min(a + 1000)).map(|c| px(sheet.col_width(c))).sum());
        let title_h: f32 = self.title_rows.map_or(0.0, |(a, b)| (a..=b.min(a + 1000)).map(|r| px(sheet.row_height(r))).sum());
        let cols_w = |a: u32, b: u32| px(sheet.col_x(b + 1) - sheet.col_x(a));
        let rows_h = |a: u32, b: u32| px(sheet.row_y(b + 1) - sheet.row_y(a));
        let scale = match self.fit {
            None => self.scale.clamp(10, 400) as f32 / 100.0,
            Some((fw, fh)) => {
                // The largest scale (at most 100 %) that fits, at least 10 %.
                let mut s = 1.0f32;
                let fits = |pages: usize, n: u16| n == 0 || pages <= n as usize;
                for _ in 0..40 {
                    let cols = split(area.start.col, area.end.col, (cw / s - title_w).max(1.0), &self.col_breaks, |c| px(sheet.col_width(c)));
                    let rows = split(area.start.row, area.end.row, (ch / s - title_h).max(1.0), &self.row_breaks, |r| px(sheet.row_height(r)));
                    if fits(cols.len(), fw) && fits(rows.len(), fh) || s <= 0.1 {
                        break;
                    }
                    // Jump close to the needed scale, then refine.
                    let need_w = if fw > 0 { cw * fw as f32 / (cols_w(area.start.col, area.end.col) + title_w * fw as f32) } else { 1.0 };
                    let need_h = if fh > 0 { ch * fh as f32 / (rows_h(area.start.row, area.end.row) + title_h * fh as f32) } else { 1.0 };
                    s = (s * 0.97).min(need_w.min(need_h).max(0.1)).max(0.1);
                }
                s
            }
        };
        let cols = split(area.start.col, area.end.col, (cw / scale - title_w).max(1.0), &self.col_breaks, |c| px(sheet.col_width(c)));
        let rows = split(area.start.row, area.end.row, (ch / scale - title_h).max(1.0), &self.row_breaks, |r| px(sheet.row_height(r)));
        Pages { area, cols, rows, scale }
    }
}

impl SheetPage {
    /// Scale the sheet prints at (1.0 = 100 %), fitting included.
    pub fn print_scale(&self, sheet: &Sheet) -> f32 {
        match self.paginate(sheet) {
            Some(p) => p.scale,
            None if self.fit.is_some() => 1.0,
            None => self.scale.clamp(10, 400) as f32 / 100.0,
        }
    }

    /// Page Layout view: the rows (or columns) from the first one to at
    /// least `upto`, split into pages at `scale` the way printing splits
    /// them (the last span may be cut short at `upto`).
    pub fn tiles(&self, sheet: &Sheet, rows: bool, scale: f32, upto: u32) -> Vec<(u32, u32)> {
        let (cw, ch) = self.content();
        let px = |v: f32| v * PT_PER_PX;
        let scale = scale.max(0.05);
        if rows {
            let title_h: f32 = self.title_rows.map_or(0.0, |(a, b)| (a..=b.min(a + 1000)).map(|r| px(sheet.row_height(r))).sum());
            split(0, upto.min(crate::MAX_ROWS - 1), (ch / scale - title_h).max(1.0), &self.row_breaks, |r| px(sheet.row_height(r)))
        } else {
            let title_w: f32 = self.title_cols.map_or(0.0, |(a, b)| (a..=b.min(a + 1000)).map(|c| px(sheet.col_width(c))).sum());
            split(0, upto.min(crate::MAX_COLS - 1), (cw / scale - title_w).max(1.0), &self.col_breaks, |c| px(sheet.col_width(c)))
        }
    }
}

/// Splits `first..=last` into spans no longer than `room`, also breaking
/// before each manual break. A span holds at least one item.
fn split(first: u32, last: u32, room: f32, breaks: &BTreeSet<u32>, size: impl Fn(u32) -> f32) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let mut start = first;
    let mut used = 0.0f32;
    let mut i = first;
    while i <= last {
        let s = size(i);
        let manual = i > start && breaks.contains(&i);
        if manual || (i > start && used + s > room) {
            out.push((start, i - 1));
            start = i;
            used = 0.0;
        }
        used += s;
        // Very long runs of empty hidden rows cost nothing to step over.
        if i == u32::MAX {
            break;
        }
        i += 1;
    }
    out.push((start, last));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CellRef;

    #[test]
    fn pagination() {
        let mut sh = Sheet::new("S");
        sh.put(CellRef::new(99, 9), Some(crate::Cell::new(crate::Content::Value(crate::Value::Number(1.0)), 0)));
        let mut p = SheetPage::default();
        let pages = p.paginate(&sh).unwrap();
        // 10 columns of 66pt = 660pt on 494.5pt: two page columns. 100 rows
        // of 16.5pt on 733.9pt: three page rows.
        assert_eq!(pages.cols, vec![(0, 6), (7, 9)]);
        assert_eq!(pages.rows.len(), 3);
        assert_eq!(pages.count(), 6);
        assert_eq!(pages.page(1, false), Some((pages.rows[1], (0, 6))));
        assert_eq!(pages.page(1, true), Some((pages.rows[0], (7, 9))));
        p.row_breaks.insert(10);
        let pages = p.paginate(&sh).unwrap();
        assert_eq!(pages.rows[0], (0, 9));
        p.row_breaks.clear();
        p.fit = Some((1, 0));
        let pages = p.paginate(&sh).unwrap();
        assert_eq!(pages.cols.len(), 1);
        assert!(pages.scale < 0.76 && pages.scale > 0.6, "{}", pages.scale);
        p.fit = Some((1, 1));
        let pages = p.paginate(&sh).unwrap();
        assert_eq!(pages.count(), 1);
        p.fit = None;
        p.print_areas = vec![Range::new(CellRef::new(0, 0), CellRef::new(4, 1))];
        p.title_rows = Some((0, 0));
        let pages = p.paginate(&sh).unwrap();
        assert_eq!((pages.cols.clone(), pages.rows.clone()), (vec![(0, 1)], vec![(0, 4)]));
        // Several areas: each on pages of its own, in order.
        p.print_areas.push(Range::new(CellRef::new(20, 3), CellRef::new(24, 4)));
        let all = p.paginate_all(&sh);
        assert_eq!(all.len(), 2);
        assert_eq!((all[1].cols.clone(), all[1].rows.clone()), (vec![(3, 4)], vec![(20, 24)]));
    }

    #[test]
    fn tiles_match_print() {
        let mut sh = Sheet::new("S");
        sh.put(CellRef::new(99, 9), Some(crate::Cell::new(crate::Content::Value(crate::Value::Number(1.0)), 0)));
        let mut p = SheetPage::default();
        p.row_breaks.insert(30);
        let pages = p.paginate(&sh).unwrap();
        let s = p.print_scale(&sh);
        let rows = p.tiles(&sh, true, s, 500);
        let n = pages.rows.len() - 1;
        assert_eq!(&rows[..n], &pages.rows[..n]);
        assert_eq!(rows[1].0, 30);
        assert_eq!(rows.last().unwrap().1, 500);
        let cols = p.tiles(&sh, false, s, 9);
        assert_eq!(cols, pages.cols);
        // Half the scale: about twice the cells on a page.
        let cols = p.tiles(&sh, false, 0.5, 30);
        assert_eq!(cols[0], (0, 13));
    }
}
