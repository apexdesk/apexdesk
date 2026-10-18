//! From positioned characters to text structure: characters make lines
//! and segments (pieces of a line apart from the rest), ruled grids and
//! aligned columns make tables, and the page is cut into blocks in
//! reading order (recursive X-Y cuts, with columns kept together).

use super::content::{Char, PageImage, Shape};

/// Ascent and descent as a fraction of the font size (for line boxes).
pub const ASC: f32 = 0.78;
pub const DESC: f32 = 0.22;

/// A run of characters on one baseline, apart from its neighbours.
#[derive(Clone, Debug)]
pub struct Seg {
    pub chars: Vec<Char>,
    pub x0: f32,
    pub x1: f32,
    pub y: f32,
    pub size: f32,
}

impl Seg {
    fn new(chars: Vec<Char>) -> Seg {
        let x0 = chars.iter().map(|c| c.x).fold(f32::MAX, f32::min);
        let x1 = chars.iter().map(|c| c.x + c.w.max(0.0)).fold(f32::MIN, f32::max);
        // The baseline and size of the main text (not of superscripts).
        let size = chars.iter().map(|c| c.size).fold(0.0, f32::max);
        let y = chars.iter().filter(|c| c.size >= size * 0.9).map(|c| c.y).fold(f32::MIN, f32::max);
        Seg { chars, x0, x1, y, size }
    }

    pub fn top(&self) -> f32 {
        self.y - self.size * ASC
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.size * DESC
    }

    pub fn text(&self) -> String {
        self.chars.iter().map(|c| c.ch).collect()
    }
}

/// Segments on one baseline, left to right.
#[derive(Clone, Debug)]
pub struct Line {
    pub segs: Vec<Seg>,
    pub y: f32,
    pub size: f32,
    pub x0: f32,
    pub x1: f32,
}

impl Line {
    pub fn new(mut segs: Vec<Seg>) -> Line {
        segs.sort_by(|a, b| a.x0.total_cmp(&b.x0));
        let main = segs.iter().max_by(|a, b| (a.x1 - a.x0).total_cmp(&(b.x1 - b.x0))).map_or((0.0, 0.0), |s| (s.y, s.size));
        let size = segs.iter().map(|s| s.size).fold(main.1, f32::max);
        let x0 = segs.first().map_or(0.0, |s| s.x0);
        let x1 = segs.iter().map(|s| s.x1).fold(f32::MIN, f32::max);
        Line { segs, y: main.0, size, x0, x1 }
    }

    pub fn text(&self) -> String {
        self.segs.iter().map(Seg::text).collect::<Vec<_>>().join("\t")
    }
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF)
}

fn width(c: &Char) -> f32 {
    if c.w > 0.0 { c.w } else { c.size * 0.5 }
}

/// A space character standing for a gap.
fn space_after(c: &Char, gap: f32) -> Char {
    Char { ch: ' ', x: c.x + width(c), w: gap.max(0.0), ..c.clone() }
}

/// Whether a segment's text is only a list marker ("•", "1.", "a)").
pub fn is_marker(t: &str) -> bool {
    let t = t.trim();
    if t.chars().count() == 1 {
        return t.chars().next().is_some_and(is_bullet);
    }
    list_number(t).is_some_and(|(_, _, len)| len == t.len())
}

pub fn is_bullet(c: char) -> bool {
    matches!(c, '•' | '◦' | '▪' | '▫' | '■' | '□' | '●' | '○' | '–' | '-' | '*' | '·' | '\u{F0B7}' | '\u{F0A7}' | '➢' | '➤' | '►' | '✓' | '✔' | '❖' | '♦' | '◆' | '‣' | '⁃' | '∙')
}

/// A list number at the start of `t`: its value, format (index in
/// `NUM_FORMATS`) and length in bytes including the punctuation.
pub fn list_number(t: &str) -> Option<(u32, u8, usize)> {
    let b = t.as_bytes();
    let digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
    if (1..=3).contains(&digits) {
        let n: u32 = t[..digits].parse().ok()?;
        return match b.get(digits) {
            Some(b'.') if b.get(digits + 1).is_none_or(|c| !c.is_ascii_digit()) => Some((n, 1, digits + 1)),
            Some(b')') => Some((n, 2, digits + 1)),
            _ => None,
        };
    }
    let roman = b.iter().take_while(|c| matches!(c, b'i' | b'v' | b'x')).count();
    if (1..=4).contains(&roman) && matches!(b.get(roman), Some(b'.' | b')')) && roman_value(&t[..roman]).is_some() && (roman > 1 || b[0] == b'i') {
        return Some((roman_value(&t[..roman])?, 5, roman + 1));
    }
    if b.len() >= 2 && b[0].is_ascii_lowercase() && b[1] == b')' {
        return Some(((b[0] - b'a') as u32 + 1, 3, 2));
    }
    if b.len() >= 3 && b[0] == b'(' && b[1].is_ascii_lowercase() && b[2] == b')' {
        return Some(((b[1] - b'a') as u32 + 1, 3, 3));
    }
    None
}

fn roman_value(s: &str) -> Option<u32> {
    let v = |c: char| match c {
        'i' => 1,
        'v' => 5,
        'x' => 10,
        _ => 0,
    };
    let cs: Vec<u32> = s.chars().map(v).collect();
    let mut total = 0i32;
    for i in 0..cs.len() {
        if i + 1 < cs.len() && cs[i] < cs[i + 1] {
            total -= cs[i] as i32;
        } else {
            total += cs[i] as i32;
        }
    }
    (total > 0).then_some(total as u32)
}

/// Removes characters drawn twice at the same place (fake bold, shadows).
fn dedupe(chars: Vec<Char>) -> Vec<Char> {
    let mut out: Vec<Char> = Vec::with_capacity(chars.len());
    for c in chars {
        // Closer than half the glyph's width, so that double letters
        // ("ll") stay.
        let near = (0.5 * c.w).min(0.3 * c.size).max(0.05 * c.size).max(0.1);
        let dup = out.iter().rev().take(8).any(|p| p.ch == c.ch && !c.ch.is_whitespace() && (p.x - c.x).abs() < near && (p.y - c.y).abs() < 0.3 * c.size.max(1.0));
        if !dup {
            out.push(c);
        }
    }
    out
}

/// Characters to segments.
pub fn segments(chars: Vec<Char>, page_w: f32, page_h: f32) -> Vec<Seg> {
    let chars: Vec<Char> = chars.into_iter().filter(|c| c.x > -c.size && c.x < page_w + c.size && c.y > -c.size && c.y < page_h + 2.0 * c.size && c.size > 0.5 && c.size < 500.0).collect();
    let chars = dedupe(chars);
    let (upright, rotated): (Vec<Char>, Vec<Char>) = chars.into_iter().partition(|c| c.upright);
    // Runs in drawing order.
    let mut frags: Vec<Vec<Char>> = Vec::new();
    for c in upright {
        let join = frags.last().and_then(|f| f.last()).is_some_and(|l| {
            let s = l.size.max(c.size);
            let gap = c.x - (l.x + width(l));
            (c.y - l.y).abs() <= 0.25 * s && gap > -0.5 * s && gap <= 0.8 * s
        });
        if join {
            let f = frags.last_mut().expect("a run");
            let l = f.last().expect("a char");
            let gap = c.x - (l.x + width(l));
            let wide = if is_cjk(c.ch) || is_cjk(l.ch) { 0.4 } else { 0.15 };
            if gap > wide * c.size.min(l.size) && c.ch != ' ' && l.ch != ' ' {
                let sp = space_after(l, gap);
                f.push(sp);
            }
            f.push(c);
        } else {
            frags.push(vec![c]);
        }
    }
    // Runs on one baseline make a line.
    let mut frags: Vec<Seg> = frags.into_iter().filter(|f| f.iter().any(|c| !c.ch.is_whitespace())).map(Seg::new).collect();
    frags.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x0.total_cmp(&b.x0)));
    let mut lines: Vec<Vec<Seg>> = Vec::new();
    for f in frags {
        let fits = |l: &Vec<Seg>| {
            let ly = l[0].y;
            let s = l.iter().map(|x| x.size).fold(f.size, f32::max);
            (f.y - ly).abs() <= 0.45 * s && !l.iter().any(|o| overlap(o.x0, o.x1, f.x0, f.x1) > 0.3 * (o.x1 - o.x0).min(f.x1 - f.x0).max(0.5))
        };
        let n = lines.len();
        match (n.saturating_sub(6)..n).rev().find(|&i| fits(&lines[i])) {
            Some(i) => lines[i].push(f),
            None => lines.push(vec![f]),
        }
    }
    let mut out = Vec::new();
    for mut l in lines {
        l.sort_by(|a, b| a.x0.total_cmp(&b.x0));
        let mut cur: Option<Seg> = None;
        for f in l {
            cur = Some(match cur.take() {
                None => f,
                Some(mut s) => {
                    let size = s.size.max(f.size);
                    let gap = f.x0 - s.x1;
                    let marker = is_marker(&s.text()) && gap < 5.0 * size;
                    if gap <= 0.8 * size || marker {
                        let last = s.chars.last().expect("a char").clone();
                        if gap > 0.12 * size && last.ch != ' ' && f.chars.first().is_some_and(|c| c.ch != ' ') {
                            s.chars.push(space_after(&last, gap));
                        }
                        s.chars.extend(f.chars);
                        Seg::new(s.chars)
                    } else {
                        out.push(trim(s));
                        f
                    }
                }
            });
        }
        if let Some(s) = cur {
            out.push(trim(s));
        }
    }
    // Text not drawn left to right: one segment per run.
    let mut run: Vec<Char> = Vec::new();
    for c in rotated {
        if let Some(l) = run.last()
            && ((c.x - l.x).abs() + (c.y - l.y).abs() > 3.0 * c.size)
        {
            out.push(rotated_seg(std::mem::take(&mut run)));
        }
        run.push(c);
    }
    if !run.is_empty() {
        out.push(rotated_seg(run));
    }
    out.retain(|s| !s.chars.is_empty());
    out
}

fn rotated_seg(chars: Vec<Char>) -> Seg {
    let size = chars.iter().map(|c| c.size).fold(0.0, f32::max);
    let x0 = chars.iter().map(|c| c.x).fold(f32::MAX, f32::min);
    let y = chars.iter().map(|c| c.y).fold(f32::MAX, f32::min) + size * ASC;
    let x1 = x0 + size;
    Seg { chars: chars.into_iter().map(|c| Char { upright: true, ..c }).collect(), x0, x1, y, size }
}

fn trim(mut s: Seg) -> Seg {
    while s.chars.last().is_some_and(|c| c.ch == ' ') {
        s.chars.pop();
    }
    while s.chars.first().is_some_and(|c| c.ch == ' ') {
        s.chars.remove(0);
    }
    if s.chars.is_empty() {
        return s;
    }
    Seg::new(s.chars)
}

fn overlap(a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    a1.min(b1) - a0.max(b0)
}

/// Segments to lines, top to bottom.
pub fn lines(mut segs: Vec<Seg>) -> Vec<Line> {
    segs.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x0.total_cmp(&b.x0)));
    let mut out: Vec<Vec<Seg>> = Vec::new();
    for s in segs {
        let fits = |l: &Vec<Seg>| {
            let size = l.iter().map(|x| x.size).fold(s.size, f32::max);
            (s.y - l[0].y).abs() <= 0.4 * size && !l.iter().any(|o| overlap(o.x0, o.x1, s.x0, s.x1) > 1.0)
        };
        match out.last_mut() {
            Some(l) if fits(l) => l.push(s),
            _ => out.push(vec![s]),
        }
    }
    out.into_iter().map(Line::new).collect()
}

// ---- Rules and ruled tables ---------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct HRule {
    pub y: f32,
    pub x0: f32,
    pub x1: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct VRule {
    pub x: f32,
    pub y0: f32,
    pub y1: f32,
}

/// Horizontal and vertical lines drawn on the page (strokes, thin filled
/// boxes and the sides of stroked boxes).
pub fn rules(shapes: &[Shape]) -> (Vec<HRule>, Vec<VRule>) {
    let mut h = Vec::new();
    let mut v = Vec::new();
    for s in shapes {
        let visible = |c: u32| c != 0xFFFFFF;
        let stroked = s.stroke.is_some_and(|(c, _)| visible(c));
        let filled = s.fill.is_some_and(visible);
        let pts = &s.points;
        if pts.len() < 2 {
            continue;
        }
        let x0 = pts.iter().map(|p| p[0]).fold(f32::MAX, f32::min);
        let x1 = pts.iter().map(|p| p[0]).fold(f32::MIN, f32::max);
        let y0 = pts.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
        let y1 = pts.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
        let axis = pts.windows(2).all(|w| (w[0][0] - w[1][0]).abs() < 0.5 || (w[0][1] - w[1][1]).abs() < 0.5);
        let is_box = (s.rect || (axis && s.closed && (4..=5).contains(&pts.len()))) && x1 - x0 > 0.0;
        if is_box && (filled || stroked) {
            let (w, ht) = (x1 - x0, y1 - y0);
            if ht <= 3.0 && w > 3.0 {
                h.push(HRule { y: (y0 + y1) / 2.0, x0, x1 });
            } else if w <= 3.0 && ht > 3.0 {
                v.push(VRule { x: (x0 + x1) / 2.0, y0, y1 });
            } else if stroked && w > 3.0 && ht > 3.0 {
                h.push(HRule { y: y0, x0, x1 });
                h.push(HRule { y: y1, x0, x1 });
                v.push(VRule { x: x0, y0, y1 });
                v.push(VRule { x: x1, y0, y1 });
            }
            continue;
        }
        if !stroked {
            continue;
        }
        let mut edges: Vec<([f32; 2], [f32; 2])> = pts.windows(2).map(|w| (w[0], w[1])).collect();
        if s.closed {
            edges.push((pts[pts.len() - 1], pts[0]));
        }
        for (a, b) in edges {
            if (a[1] - b[1]).abs() < 0.5 && (a[0] - b[0]).abs() > 2.0 {
                h.push(HRule { y: a[1], x0: a[0].min(b[0]), x1: a[0].max(b[0]) });
            } else if (a[0] - b[0]).abs() < 0.5 && (a[1] - b[1]).abs() > 2.0 {
                v.push(VRule { x: a[0], y0: a[1].min(b[1]), y1: a[1].max(b[1]) });
            }
        }
    }
    // Pieces of one line become one.
    h.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x0.total_cmp(&b.x0)));
    let mut hm: Vec<HRule> = Vec::new();
    for r in h {
        match hm.iter_mut().rev().take(8).find(|m| (m.y - r.y).abs() < 1.0 && r.x0 <= m.x1 + 2.0 && r.x1 >= m.x0 - 2.0) {
            Some(m) => {
                m.x0 = m.x0.min(r.x0);
                m.x1 = m.x1.max(r.x1);
            }
            None => hm.push(r),
        }
    }
    v.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y0.total_cmp(&b.y0)));
    let mut vm: Vec<VRule> = Vec::new();
    for r in v {
        match vm.iter_mut().rev().take(8).find(|m| (m.x - r.x).abs() < 1.0 && r.y0 <= m.y1 + 2.0 && r.y1 >= m.y0 - 2.0) {
            Some(m) => {
                m.y0 = m.y0.min(r.y0);
                m.y1 = m.y1.max(r.y1);
            }
            None => vm.push(r),
        }
    }
    (hm, vm)
}

/// A table: its column and row edges and each cell's text.
#[derive(Clone, Debug)]
pub struct TableBlock {
    pub xs: Vec<f32>,
    pub ys: Vec<f32>,
    /// Cells by row and column: the lines in it, and its spans (0 for a
    /// cell covered by a merged one).
    pub cells: Vec<Vec<Cell>>,
    /// Drawn with rules (otherwise found by aligned text).
    pub ruled: bool,
    /// Columns whose text is right-aligned.
    pub right: Vec<bool>,
}

#[derive(Clone, Debug, Default)]
pub struct Cell {
    pub lines: Vec<Line>,
    pub col_span: u16,
    pub row_span: u32,
}

impl TableBlock {
    pub fn bbox(&self) -> [f32; 4] {
        [self.xs[0], self.ys[0], *self.xs.last().expect("edges"), *self.ys.last().expect("edges")]
    }

    /// The text of each cell.
    pub fn texts(&self) -> Vec<Vec<String>> {
        self.cells.iter().map(|r| r.iter().map(|c| c.lines.iter().map(|l| l.segs.iter().map(Seg::text).collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join(" ")).collect()).collect()
    }
}

fn cluster(mut v: Vec<f32>, tol: f32) -> Vec<f32> {
    v.sort_by(f32::total_cmp);
    let mut out: Vec<(f32, u32)> = Vec::new();
    for x in v {
        match out.last_mut() {
            Some((m, n)) if x - *m / *n as f32 <= tol => {
                *m += x;
                *n += 1;
            }
            _ => out.push((x, 1)),
        }
    }
    out.into_iter().map(|(m, n)| m / n as f32).collect()
}

/// Tables drawn as grids; takes the characters inside them.
pub fn grids(h: &[HRule], v: &[VRule], chars: &mut Vec<Char>, pw: f32, ph: f32) -> Vec<TableBlock> {
    const T: f32 = 2.5;
    if h.len() > 4000 || v.len() > 4000 {
        return Vec::new();
    }
    // Connected rules.
    let n = h.len() + v.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], i: usize) -> usize {
        let mut i = i;
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    for (i, a) in h.iter().enumerate() {
        for (j, b) in v.iter().enumerate() {
            if b.x >= a.x0 - T && b.x <= a.x1 + T && a.y >= b.y0 - T && a.y <= b.y1 + T {
                let (ra, rb) = (find(&mut parent, i), find(&mut parent, h.len() + j));
                parent[ra] = rb;
            }
        }
    }
    let mut groups: std::collections::BTreeMap<usize, (Vec<HRule>, Vec<VRule>)> = Default::default();
    for (i, r) in h.iter().enumerate() {
        let root = find(&mut parent, i);
        groups.entry(root).or_default().0.push(*r);
    }
    for (j, r) in v.iter().enumerate() {
        let root = find(&mut parent, h.len() + j);
        groups.entry(root).or_default().1.push(*r);
    }
    let mut out = Vec::new();
    for (_, (gh, gv)) in groups {
        if gh.len() < 2 || gv.len() < 2 {
            continue;
        }
        let xs = cluster(gv.iter().map(|r| r.x).collect(), T);
        let ys = cluster(gh.iter().map(|r| r.y).collect(), T);
        // Double lines count once.
        let xs: Vec<f32> = xs.iter().copied().fold(Vec::new(), |mut a: Vec<f32>, x| {
            if a.last().is_none_or(|l| x - l > 4.0) {
                a.push(x);
            }
            a
        });
        let ys: Vec<f32> = ys.iter().copied().fold(Vec::new(), |mut a: Vec<f32>, y| {
            if a.last().is_none_or(|l| y - l > 4.0) {
                a.push(y);
            }
            a
        });
        if xs.len() < 3 || ys.len() < 2 {
            continue;
        }
        // A frame around the whole page is not a table.
        if xs[xs.len() - 1] - xs[0] > pw * 0.97 && ys[ys.len() - 1] - ys[0] > ph * 0.9 {
            continue;
        }
        let (rows, cols) = (ys.len() - 1, xs.len() - 1);
        if rows > 2000 || cols > 60 {
            continue;
        }
        let has_v = |x: f32, y: f32| gv.iter().any(|r| (r.x - x).abs() < T && r.y0 - T <= y && y <= r.y1 + T);
        let has_h = |y: f32, x: f32| gh.iter().any(|r| (r.y - y).abs() < T && r.x0 - T <= x && x <= r.x1 + T);
        let mut cells = vec![vec![Cell::default(); cols]; rows];
        let mut taken = vec![vec![false; cols]; rows];
        // The cell each grid square belongs to.
        let mut origin = vec![vec![(0usize, 0usize); cols]; rows];
        for r in 0..rows {
            for c in 0..cols {
                if taken[r][c] {
                    continue;
                }
                let my = (ys[r] + ys[r + 1]) / 2.0;
                let mut cs = 1;
                while c + cs < cols && !has_v(xs[c + cs], my) && !taken[r][c + cs] {
                    cs += 1;
                }
                let mut rs = 1;
                while r + rs < rows && (c..c + cs).all(|k| !has_h(ys[r + rs], (xs[k] + xs[k + 1]) / 2.0) && !taken[r + rs][k]) {
                    rs += 1;
                }
                for rr in r..r + rs {
                    for cc in c..c + cs {
                        taken[rr][cc] = true;
                        origin[rr][cc] = (r, c);
                    }
                }
                cells[r][c].col_span = cs as u16;
                cells[r][c].row_span = rs as u32;
            }
        }
        // The text inside.
        let [bx0, by0, bx1, by1] = [xs[0], ys[0], xs[cols], ys[rows]];
        let mut inside: Vec<Vec<Vec<Char>>> = vec![vec![Vec::new(); cols]; rows];
        let mut any = false;
        chars.retain(|ch| {
            let cx = ch.x + width(ch) / 2.0;
            let cy = ch.y - ch.size * 0.3;
            if cx < bx0 || cx > bx1 || cy < by0 || cy > by1 {
                return true;
            }
            // Both edge lists are sorted.
            let c = xs.partition_point(|x| *x < cx).saturating_sub(1).min(cols - 1);
            let r = ys.partition_point(|y| *y < cy).saturating_sub(1).min(rows - 1);
            // The cell a merged area starts at.
            let (r0, c0) = origin[r][c];
            inside[r0][c0].push(ch.clone());
            any |= !ch.ch.is_whitespace();
            false
        });
        if !any && rows * cols < 4 {
            continue;
        }
        for (r, row) in inside.into_iter().enumerate() {
            for (c, cs) in row.into_iter().enumerate() {
                if !cs.is_empty() {
                    cells[r][c].lines = lines(segments_in_cell(cs));
                }
            }
        }
        let right = vec![false; cols];
        out.push(TableBlock { xs, ys, cells, ruled: true, right });
    }
    out
}

/// Characters of a cell to segments (a cell's text is never split in
/// columns).
fn segments_in_cell(chars: Vec<Char>) -> Vec<Seg> {
    let segs = segments(chars, f32::MAX / 4.0, f32::MAX / 4.0);
    let mut by_line: Vec<Line> = lines(segs);
    by_line
        .drain(..)
        .map(|l| {
            let mut chars: Vec<Char> = Vec::new();
            for s in l.segs {
                if let Some(last) = chars.last().cloned() {
                    chars.push(space_after(&last, 0.0));
                }
                chars.extend(s.chars);
            }
            Seg::new(chars)
        })
        .collect()
}

// ---- Tables of aligned text ---------------------------------------------

/// Rows of segments in columns (at least 3 rows and 2 columns of short
/// texts or numbers); takes those lines out of `lines`.
pub fn aligned_tables(lines: &mut Vec<Line>) -> Vec<TableBlock> {
    split_at_columns(lines);
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].segs.len() < 2 {
            i += 1;
            continue;
        }
        let mut cols: Vec<(f32, f32)> = lines[i].segs.iter().map(|s| (s.x0, s.x1)).collect();
        let mut rows = vec![i];
        let size = lines[i].size;
        let mut j = i + 1;
        while j < lines.len() {
            let l = &lines[j];
            let prev = &lines[*rows.last().expect("a row")];
            // Rows with padding are far apart, but evenly so.
            let limit = match rows.len() {
                1 => 3.2 * size.max(prev.size),
                _ => (1.3 * (lines[rows[1]].y - lines[rows[0]].y)).clamp(2.6 * size, 4.5 * size),
            };
            if l.y - prev.y > limit || (l.size - size).abs() > 0.3 * size {
                break;
            }
            match fit(&cols, l) {
                Some(c) => cols = c,
                None => break,
            }
            rows.push(j);
            j += 1;
        }
        while rows.len() > 1 && lines[*rows.last().expect("a row")].segs.len() < 2 {
            rows.pop();
        }
        let multi = rows.iter().filter(|&&r| lines[r].segs.len() >= 2).count();
        let cells: Vec<&Seg> = rows.iter().flat_map(|&r| lines[r].segs.iter()).collect();
        let avg = cells.iter().map(|s| s.chars.len()).sum::<usize>() as f32 / cells.len().max(1) as f32;
        let numeric = cells.iter().filter(|s| is_numeric(&s.text())).count() as f32 / cells.len().max(1) as f32;
        let ok = rows.len() >= 3 && multi >= 3 && multi * 2 >= rows.len() && cols.len() >= 2 && (avg <= 28.0 || numeric >= 0.3) && cols.len() <= 40;
        if !ok {
            i += 1;
            continue;
        }
        cols.sort_by(|a, b| a.0.total_cmp(&b.0));
        let taken: Vec<Line> = lines.drain(rows[0]..=rows[rows.len() - 1]).collect();
        out.push(table_of(taken, &cols));
        i = rows[0];
    }
    out
}

/// Splits the segments of lines already in pieces where a wide gap
/// (narrower than the one that separates segments) falls where another
/// line has a segment start: columns set close together.
fn split_at_columns(lines: &mut [Line]) {
    let starts: Vec<(usize, f32)> = lines.iter().enumerate().filter(|(_, l)| l.segs.len() >= 2).flat_map(|(i, l)| l.segs.iter().map(move |s| (i, s.x0))).collect();
    for (i, l) in lines.iter_mut().enumerate() {
        if l.segs.len() < 2 {
            continue;
        }
        let mut segs = Vec::with_capacity(l.segs.len());
        for s in l.segs.drain(..) {
            let mut cut = Vec::new();
            let mut last: Option<&Char> = None;
            for (k, c) in s.chars.iter().enumerate() {
                if c.ch.is_whitespace() {
                    continue;
                }
                if let Some(p) = last {
                    let gap = c.x - (p.x + p.w);
                    if gap >= 0.45 * s.size && starts.iter().any(|&(j, x)| j != i && (x - c.x).abs() < 1.5) {
                        cut.push(k);
                    }
                }
                last = Some(c);
            }
            if cut.is_empty() {
                segs.push(s);
                continue;
            }
            let mut chars = s.chars;
            for &k in cut.iter().rev() {
                let tail = chars.split_off(k);
                segs.push(Seg::new(tail));
                while chars.last().is_some_and(|c| c.ch.is_whitespace()) {
                    chars.pop();
                }
            }
            segs.push(Seg::new(chars));
            let n = cut.len() + 1;
            let at = segs.len() - n;
            segs[at..].reverse();
        }
        l.segs = segs;
    }
}

fn is_numeric(t: &str) -> bool {
    let t = t.trim();
    !t.is_empty() && t.chars().any(|c| c.is_ascii_digit()) && t.chars().all(|c| c.is_ascii_digit() || " .,-+()%$€£¥/:".contains(c))
}

/// Fits a line's segments into columns (widening them, or adding one in
/// a gap); `None` if a segment crosses two columns.
fn fit(cols: &[(f32, f32)], l: &Line) -> Option<Vec<(f32, f32)>> {
    let mut cols = cols.to_vec();
    let mut used = vec![false; cols.len() + l.segs.len()];
    for s in &l.segs {
        let hits: Vec<usize> = (0..cols.len()).filter(|&k| overlap(cols[k].0, cols[k].1, s.x0, s.x1) > -1.0).collect();
        match hits.len() {
            0 => {
                // A new column between the others.
                cols.push((s.x0, s.x1));
                used[cols.len() - 1] = true;
            }
            1 => {
                let k = hits[0];
                if used[k] {
                    return None;
                }
                used[k] = true;
                cols[k] = (cols[k].0.min(s.x0), cols[k].1.max(s.x1));
            }
            _ => return None,
        }
    }
    // Widened columns must not run into each other.
    let mut sorted = cols.clone();
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
    if sorted.windows(2).any(|w| w[1].0 < w[0].1 + 2.0) {
        return None;
    }
    Some(cols)
}

fn table_of(lines: Vec<Line>, cols: &[(f32, f32)]) -> TableBlock {
    let n = cols.len();
    let col_of = |s: &Seg| cols.iter().position(|c| overlap(c.0, c.1, s.x0, s.x1) > -1.0).unwrap_or(0);
    let mut rows: Vec<(Vec<Vec<Seg>>, f32, f32)> = Vec::new();
    for l in lines {
        let mut row: Vec<Vec<Seg>> = vec![Vec::new(); n];
        let (top, bottom) = (l.y - l.size * ASC, l.y + l.size * DESC);
        for s in l.segs {
            row[col_of(&s)].push(s);
        }
        // A line with neither the first nor the last column, close under
        // a row, goes on that row's cells (wrapped text).
        let wrapped = row[0].is_empty() && row[n - 1].is_empty() && rows.last().is_some_and(|(_, _, b)| top - b < 0.6 * l.size);
        if wrapped && let Some((prev, _, b)) = rows.last_mut() {
            for (k, segs) in row.into_iter().enumerate() {
                prev[k].extend(segs);
            }
            *b = bottom;
            continue;
        }
        rows.push((row, top, bottom));
    }
    let mut xs: Vec<f32> = cols.iter().map(|c| c.0 - 3.0).collect();
    xs.push(cols[n - 1].1 + 3.0);
    let mut ys: Vec<f32> = rows.iter().map(|r| r.1 - 1.0).collect();
    ys.push(rows.last().map_or(0.0, |r| r.2 + 1.0));
    // Right-aligned columns: their right edges line up, their left ones don't.
    let mut right = vec![false; n];
    for (k, r) in right.iter_mut().enumerate() {
        let segs: Vec<&Seg> = rows.iter().flat_map(|row| row.0[k].iter()).collect();
        if segs.len() >= 2 {
            let spread = |v: Vec<f32>| v.iter().copied().fold(f32::MIN, f32::max) - v.iter().copied().fold(f32::MAX, f32::min);
            *r = spread(segs.iter().map(|s| s.x1).collect()) < 2.0 && spread(segs.iter().map(|s| s.x0).collect()) > 4.0;
        }
    }
    let cells = rows.into_iter().map(|(row, _, _)| row.into_iter().map(|segs| Cell { lines: super::layout::lines(segs), col_span: 1, row_span: 1 }).collect()).collect();
    TableBlock { xs, ys, cells, ruled: false, right }
}

// ---- Reading order ------------------------------------------------------

/// Something placed on the page.
#[derive(Clone, Debug)]
pub enum Item {
    Seg(Seg),
    Table(TableBlock),
    Image(PageImage),
}

impl Item {
    pub fn bbox(&self) -> [f32; 4] {
        match self {
            Item::Seg(s) => [s.x0, s.top(), s.x1, s.bottom()],
            Item::Table(t) => t.bbox(),
            Item::Image(i) => [i.x, i.y, i.x + i.w, i.y + i.h],
        }
    }
}

/// A block of the page in reading order, with the left and right edges
/// of the column it is in.
pub struct Block {
    pub items: Vec<Item>,
    pub frame: (f32, f32),
}

enum Node {
    Leaf(Vec<usize>),
    /// Top to bottom.
    H(Vec<Node>),
    /// Left to right, with the gaps between.
    V(Vec<Node>, Vec<f32>),
}

fn span(ids: &[usize], boxes: &[[f32; 4]]) -> [f32; 4] {
    ids.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |a, &i| {
        let b = boxes[i];
        [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
    })
}

/// Gaps along one axis (0 = x, 1 = y) wider than `min`: (start, end).
fn gaps(ids: &[usize], boxes: &[[f32; 4]], axis: usize, min: f32) -> Vec<(f32, f32)> {
    let mut iv: Vec<(f32, f32)> = ids.iter().map(|&i| (boxes[i][axis], boxes[i][axis + 2])).collect();
    iv.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out = Vec::new();
    let mut end = f32::MIN;
    for (a, b) in iv {
        if end > f32::MIN && a - end > min {
            out.push((end, a));
        }
        end = end.max(b);
    }
    out
}

fn cut(ids: &[usize], boxes: &[[f32; 4]], size: f32, depth: u32) -> Node {
    if ids.len() <= 1 || depth > 40 {
        return leaf(ids, boxes);
    }
    // Columns: a gap from top to bottom, with text on both sides on more
    // than one line.
    let vg = gaps(ids, boxes, 0, (0.9 * size).max(6.0));
    if !vg.is_empty() {
        let mut parts: Vec<Vec<usize>> = vec![Vec::new(); vg.len() + 1];
        for &i in ids {
            let k = vg.iter().filter(|g| boxes[i][0] >= g.1 - 0.01).count();
            parts[k].push(i);
        }
        let lines_in = |p: &Vec<usize>| {
            let mut ys: Vec<f32> = p.iter().map(|&i| boxes[i][3]).collect();
            ys.sort_by(f32::total_cmp);
            ys.dedup_by(|a, b| (*a - *b).abs() < 2.0);
            ys.len()
        };
        if parts.iter().all(|p| lines_in(p) >= 2) {
            let gutters = vg.iter().map(|g| (g.0 + g.1) / 2.0).collect();
            return Node::V(parts.iter().map(|p| cut(p, boxes, size, depth + 1)).collect(), gutters);
        }
    }
    // Bands: cut at the gaps wider than the spacing of lines.
    let hg = gaps(ids, boxes, 1, 0.0);
    if hg.is_empty() {
        return leaf(ids, boxes);
    }
    let mut widths: Vec<f32> = hg.iter().map(|g| g.1 - g.0).collect();
    widths.sort_by(f32::total_cmp);
    let median = widths[widths.len() / 2];
    let big = (median * 1.8).max(0.5 * size);
    let cuts: Vec<(f32, f32)> = hg.into_iter().filter(|g| g.1 - g.0 > big).collect();
    if cuts.is_empty() {
        return leaf(ids, boxes);
    }
    let mut parts: Vec<Vec<usize>> = vec![Vec::new(); cuts.len() + 1];
    for &i in ids {
        let k = cuts.iter().filter(|g| boxes[i][1] >= g.1 - 0.01).count();
        parts[k].push(i);
    }
    Node::H(parts.iter().filter(|p| !p.is_empty()).map(|p| cut(p, boxes, size, depth + 1)).collect())
}

fn leaf(ids: &[usize], boxes: &[[f32; 4]]) -> Node {
    let mut v = ids.to_vec();
    v.sort_by(|&a, &b| boxes[a][3].total_cmp(&boxes[b][3]).then(boxes[a][0].total_cmp(&boxes[b][0])));
    Node::Leaf(v)
}

fn ids_of(n: &Node, out: &mut Vec<usize>) {
    match n {
        Node::Leaf(v) => out.extend(v),
        Node::H(c) | Node::V(c, _) => c.iter().for_each(|x| ids_of(x, out)),
    }
}

/// Bands split at the same columns join into one set of columns, and a
/// band inside one column goes on in it.
fn merge_columns(n: Node, boxes: &[[f32; 4]]) -> Node {
    match n {
        Node::Leaf(v) => Node::Leaf(v),
        Node::V(c, g) => Node::V(c.into_iter().map(|x| merge_columns(x, boxes)).collect(), g),
        Node::H(children) => {
            let mut out: Vec<Node> = Vec::new();
            for ch in children.into_iter().map(|x| merge_columns(x, boxes)) {
                if let Some(Node::V(cols, g)) = out.last_mut() {
                    match ch {
                        Node::V(cols2, g2) if g2.len() == g.len() && g.iter().zip(&g2).all(|(a, b)| (a - b).abs() < 12.0) => {
                            for (a, b) in cols.iter_mut().zip(cols2) {
                                let prev = std::mem::replace(a, Node::Leaf(Vec::new()));
                                *a = Node::H(vec![prev, b]);
                            }
                            continue;
                        }
                        other => {
                            let mut ids = Vec::new();
                            ids_of(&other, &mut ids);
                            let s = span(&ids, boxes);
                            // Inside column k: between the gutters around it.
                            let k = g.iter().filter(|&&x| s[0] >= x).count();
                            let lo = if k == 0 { f32::MIN } else { g[k - 1] };
                            let hi = g.get(k).copied().unwrap_or(f32::MAX);
                            if !ids.is_empty() && s[0] >= lo && s[2] <= hi && k < cols.len() {
                                let prev = std::mem::replace(&mut cols[k], Node::Leaf(Vec::new()));
                                cols[k] = Node::H(vec![prev, other]);
                                continue;
                            }
                            out.push(other);
                            continue;
                        }
                    }
                }
                out.push(ch);
            }
            if out.len() == 1 { out.pop().expect("one") } else { Node::H(out) }
        }
    }
}

fn flatten(n: &Node, boxes: &[[f32; 4]], frame: Option<(f32, f32)>, out: &mut Vec<(Vec<usize>, Option<(f32, f32)>)>) {
    match n {
        Node::Leaf(v) => {
            if !v.is_empty() {
                out.push((v.clone(), frame));
            }
        }
        Node::H(c) => c.iter().for_each(|x| flatten(x, boxes, frame, out)),
        Node::V(c, _) => {
            for x in c {
                let mut ids = Vec::new();
                ids_of(x, &mut ids);
                let s = span(&ids, boxes);
                flatten(x, boxes, Some((s[0], s[2])), out);
            }
        }
    }
}

/// Converts flattened leaves with page frames filled in.
pub fn order_page(items: Vec<Item>, size: f32, page_frame: (f32, f32)) -> Vec<Block> {
    let boxes: Vec<[f32; 4]> = items.iter().map(Item::bbox).collect();
    let all: Vec<usize> = (0..items.len()).collect();
    let tree = merge_columns(cut(&all, &boxes, size, 0), &boxes);
    let mut leaves = Vec::new();
    flatten(&tree, &boxes, None, &mut leaves);
    let mut slots: Vec<Option<Item>> = items.into_iter().map(Some).collect();
    leaves.into_iter().map(|(ids, frame)| Block { items: ids.into_iter().filter_map(|i| slots[i].take()).collect(), frame: frame.unwrap_or(page_frame) }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(x: f32, y: f32, s: &str) -> Vec<Char> {
        s.chars().enumerate().map(|(i, ch)| Char { ch, x: x + i as f32 * 5.0, y, w: 5.0, size: 10.0, font: 0, color: 0, upright: true, hidden: false }).collect()
    }

    #[test]
    fn markers_and_numbers() {
        assert_eq!(list_number("1. Uno"), Some((1, 1, 2)));
        assert_eq!(list_number("12) Doce"), Some((12, 2, 3)));
        assert_eq!(list_number("1.2 Título"), None);
        assert_eq!(list_number("iv. Cuatro"), Some((4, 5, 3)));
        assert_eq!(list_number("b) Be"), Some((2, 3, 2)));
        assert!(is_marker("•"));
        assert!(is_marker("3."));
        assert!(!is_marker("Hola"));
    }

    #[test]
    fn words_lines_and_columns() {
        let mut chars = text(72.0, 100.0, "Hello");
        chars.extend(text(102.0, 100.0, "world"));
        chars.extend(text(300.0, 100.0, "Right"));
        let segs = segments(chars, 600.0, 800.0);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].text(), "Hello world");
        assert_eq!(segs[1].text(), "Right");
    }

    #[test]
    fn two_columns_read_down_each() {
        let mut chars = Vec::new();
        for i in 0..4 {
            let y = 100.0 + i as f32 * 14.0;
            chars.extend(text(72.0, y, &format!("left{i} text here")));
            chars.extend(text(320.0, y, &format!("right{i} text here")));
        }
        let segs = segments(chars, 600.0, 800.0);
        let items: Vec<Item> = segs.into_iter().map(Item::Seg).collect();
        let blocks = order_page(items, 10.0, (72.0, 540.0));
        let order: Vec<String> = blocks.iter().flat_map(|b| b.items.iter()).map(|i| match i {
            Item::Seg(s) => s.text(),
            _ => String::new(),
        }).collect();
        assert_eq!(order[0], "left0 text here");
        assert_eq!(order[3], "left3 text here");
        assert_eq!(order[4], "right0 text here");
    }

    #[test]
    fn aligned_columns_make_a_table() {
        let mut chars = Vec::new();
        for (i, (a, b, c)) in [("Date", "Item", "Amount"), ("01/02", "Coffee", "3.50"), ("02/02", "Bread", "1.20"), ("03/02", "Milk", "0.99")].iter().enumerate() {
            let y = 100.0 + i as f32 * 14.0;
            chars.extend(text(72.0, y, a));
            chars.extend(text(180.0, y, b));
            chars.extend(text(320.0, y, c));
        }
        let mut ls = lines(segments(chars, 600.0, 800.0));
        let tables = aligned_tables(&mut ls);
        assert_eq!(tables.len(), 1);
        assert!(ls.is_empty());
        let t = tables[0].texts();
        assert_eq!(t.len(), 4);
        assert_eq!(t[2], vec!["02/02", "Bread", "1.20"]);
    }
}
