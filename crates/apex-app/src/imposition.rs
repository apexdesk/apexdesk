//! Pages per sheet and booklets: where each page of a print job goes on
//! the sides of paper printed, and which sides go in each pass when both
//! sides are printed by hand.

use eframe::egui::{Rect, Vec2, emath::TSTransform, pos2, vec2};

/// Printing on one or both sides of the paper.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Sides {
    #[default]
    One,
    /// Both sides by the printer, turned on the long edge.
    LongEdge,
    /// Both sides by the printer, turned on the short edge (booklets).
    ShortEdge,
    /// Odd sides first; the user turns the stack over for the even ones.
    Manual,
}

impl Sides {
    pub const ALL: [Sides; 4] = [Sides::One, Sides::LongEdge, Sides::ShortEdge, Sides::Manual];

    pub fn key(self) -> &'static str {
        match self {
            Sides::One => "print.sides_one",
            Sides::LongEdge => "print.sides_long",
            Sides::ShortEdge => "print.sides_short",
            Sides::Manual => "print.sides_manual",
        }
    }
}

/// Pages per sheet choices.
pub const PER_SHEET: [usize; 6] = [1, 2, 4, 6, 9, 16];

/// How the pages of a job are laid on paper.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Layout {
    pub per_sheet: usize,
    /// Two pages per side, ordered so that the folded stack reads in order.
    pub booklet: bool,
    pub sides: Sides,
}

impl Default for Layout {
    fn default() -> Layout {
        Layout { per_sheet: 1, booklet: false, sides: Sides::One }
    }
}

impl Layout {
    /// Each page printed on its own side, as it is.
    pub fn is_plain(&self) -> bool {
        self.per_sheet <= 1 && !self.booklet
    }
}

/// One side of paper printed.
#[derive(Clone, Debug, PartialEq)]
pub enum Side {
    /// A page of the job as it is.
    Page(usize),
    /// A side of `size` pixels holding pages (or blanks) in cells.
    Sheet { size: Vec2, cells: Vec<(Option<usize>, Rect)> },
}

/// Columns, rows and whether the sheet is turned for `n` pages per sheet
/// of pages of `paper` size: the grid where pages come out biggest.
pub fn grid(n: usize, paper: Vec2) -> (usize, usize, bool) {
    let mut best = (1, 1, false, 0.0f32);
    for cols in 1..=n {
        if n % cols != 0 {
            continue;
        }
        let rows = n / cols;
        for turned in [false, true] {
            let sheet = if turned { vec2(paper.y, paper.x) } else { paper };
            let cell = vec2(sheet.x / cols as f32, sheet.y / rows as f32);
            let scale = (cell.x / paper.x).min(cell.y / paper.y);
            // Ties keep the sheet as the pages are.
            if scale > best.3 + 1e-4 {
                best = (cols, rows, turned, scale);
            }
        }
    }
    (best.0, best.1, best.2)
}

/// The sides printed for `pages` (indexes into the job) of `paper` size.
pub fn sides(pages: &[usize], layout: Layout, paper: Vec2) -> Vec<Side> {
    if layout.is_plain() {
        return pages.iter().map(|&p| Side::Page(p)).collect();
    }
    let (per, order): (usize, Vec<Option<usize>>) = if layout.booklet {
        // Sheet k, front: last-2k | 2k+1; back: 2k+2 | last-2k-1 (1-based).
        let n = pages.len().div_ceil(4) * 4;
        let at = |i: usize| pages.get(i).copied();
        let mut v = Vec::with_capacity(n);
        for k in 0..n / 4 {
            v.extend([at(n - 1 - 2 * k), at(2 * k), at(2 * k + 1), at(n - 2 - 2 * k)]);
        }
        (2, v)
    } else {
        (layout.per_sheet, pages.iter().map(|&p| Some(p)).collect())
    };
    let (cols, rows, turned) = grid(per, paper);
    let size = if turned { vec2(paper.y, paper.x) } else { paper };
    let cell = vec2(size.x / cols as f32, size.y / rows as f32);
    order
        .chunks(per)
        .map(|chunk| {
            let cells = chunk
                .iter()
                .enumerate()
                .map(|(i, p)| (*p, Rect::from_min_size(pos2((i % cols) as f32 * cell.x, (i / cols) as f32 * cell.y), cell)))
                .collect();
            Side::Sheet { size, cells }
        })
        .collect()
}

/// The transform that fits a page of `size` in the middle of `cell`
/// (booklet halves and grid cells keep a small gap between pages).
pub fn fit(cell: Rect, size: Vec2) -> TSTransform {
    let scale = (cell.width() / size.x).min(cell.height() / size.y);
    let used = size * scale;
    let off = cell.center() - used / 2.0;
    TSTransform::new(off.to_vec2(), scale)
}

/// The sides of the first and second pass when both sides are printed by
/// hand: the odd sides, then the even ones.
pub fn manual_passes(all: Vec<Side>) -> (Vec<Side>, Vec<Side>) {
    let mut odd = Vec::new();
    let mut even = Vec::new();
    for (i, s) in all.into_iter().enumerate() {
        if i % 2 == 0 { odd.push(s) } else { even.push(s) }
    }
    (odd, even)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A4: Vec2 = Vec2 { x: 794.0, y: 1123.0 };

    fn pages_of(s: &[Side]) -> Vec<Vec<Option<usize>>> {
        s.iter()
            .map(|s| match s {
                Side::Page(p) => vec![Some(*p)],
                Side::Sheet { cells, .. } => cells.iter().map(|c| c.0).collect(),
            })
            .collect()
    }

    #[test]
    fn grids() {
        assert_eq!(grid(1, A4), (1, 1, false));
        assert_eq!(grid(2, A4), (2, 1, true));
        assert_eq!(grid(4, A4), (2, 2, false));
        assert_eq!(grid(6, A4), (3, 2, true));
        assert_eq!(grid(9, A4), (3, 3, false));
        assert_eq!(grid(16, A4), (4, 4, false));
    }

    #[test]
    fn pages_per_sheet() {
        let l = Layout { per_sheet: 4, ..Layout::default() };
        let s = sides(&[0, 1, 2, 3, 4], l, A4);
        assert_eq!(pages_of(&s), vec![vec![Some(0), Some(1), Some(2), Some(3)], vec![Some(4)]]);
        let Side::Sheet { size, cells } = &s[0] else { panic!() };
        assert_eq!(*size, A4);
        assert_eq!(cells[3].1.min, pos2(A4.x / 2.0, A4.y / 2.0));
        assert_eq!(pages_of(&sides(&[3, 5], Layout::default(), A4)), vec![vec![Some(3)], vec![Some(5)]]);
    }

    #[test]
    fn booklet_order() {
        let l = Layout { booklet: true, ..Layout::default() };
        // 6 pages: padded to 8; sides 8|1, 2|7, 6|3, 4|5 (1-based).
        let s = sides(&[0, 1, 2, 3, 4, 5], l, A4);
        assert_eq!(pages_of(&s), vec![vec![None, Some(0)], vec![Some(1), None], vec![Some(5), Some(2)], vec![Some(3), Some(4)]]);
        let Side::Sheet { size, .. } = &s[0] else { panic!() };
        assert_eq!(*size, vec2(A4.y, A4.x));
        let (a, b) = manual_passes(s);
        assert_eq!(pages_of(&a), vec![vec![None, Some(0)], vec![Some(5), Some(2)]]);
        assert_eq!(pages_of(&b), vec![vec![Some(1), None], vec![Some(3), Some(4)]]);
    }

    #[test]
    fn fitting() {
        let t = fit(Rect::from_min_size(pos2(100.0, 0.0), vec2(100.0, 400.0)), vec2(200.0, 200.0));
        assert_eq!(t.scaling, 0.5);
        assert_eq!(t * pos2(0.0, 0.0), pos2(100.0, 150.0));
    }
}
