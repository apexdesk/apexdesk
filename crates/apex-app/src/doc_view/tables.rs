//! Tables in the page view: grid geometry, pagination of rows (repeating
//! header rows), cell backgrounds and borders, and hit testing.

use apex_doc::{Align, Block, Border, Borders, Paragraph, TableMap, TableProps, VAlign};
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, pos2};

use super::{Layout, ParaLayout, Segment};
use crate::theme;

/// A table's grid on the page.
pub(super) struct TableBox {
    pub map: TableMap,
    /// Column edges relative to the content area's left edge (pixels).
    pub x: Vec<f32>,
    /// Tables in its cells (indices in the layout's list), in order.
    pub kids: Vec<usize>,
}

/// A table row placed on a page: the whole row, the part of a row split
/// across pages, or a copy of a header row repeated on a new page.
#[derive(Clone, Copy, Debug)]
pub(super) struct RowFrag {
    pub table: u32,
    pub row: u32,
    pub page: usize,
    pub y: f32,
    pub h: f32,
    pub ghost: bool,
    /// Holds the row's top edge / bottom edge (false for the inner edges
    /// of a row split across pages).
    pub top: bool,
    pub bottom: bool,
    /// Left edge of the column it's in (0 without columns).
    pub x: f32,
}

impl TableBox {
    pub fn new(map: TableMap, paras: &[Paragraph], content_w: f32, pt: f32) -> TableBox {
        let default = TableProps::default();
        let p = map.props(paras).unwrap_or(&default);
        let known: Vec<f32> = p.widths.iter().copied().filter(|w| *w > 0.0 && w.is_finite()).collect();
        // Columns without a width share what's left of the page (or the
        // average of the others).
        let fallback = if known.is_empty() { content_w / pt / map.cols as f32 } else { known.iter().sum::<f32>() / known.len() as f32 };
        let widths: Vec<f32> = (0..map.cols).map(|c| p.widths.get(c).copied().filter(|w| *w > 0.0 && w.is_finite()).unwrap_or(fallback).clamp(2.0, 5000.0) * pt).collect();
        let total: f32 = widths.iter().sum();
        let left = match p.align {
            Align::Center => (content_w - total) / 2.0,
            Align::Right => content_w - total,
            _ => p.indent * pt,
        };
        let mut x = Vec::with_capacity(map.cols + 1);
        x.push(left);
        for w in widths {
            x.push(x[x.len() - 1] + w);
        }
        TableBox { map, x, kids: Vec::new() }
    }

    /// The boxes of all tables, parents before the tables in their cells
    /// (`maps` from `Document::all_tables`); `width` gives the text width at
    /// a paragraph. Also returns the innermost table of each paragraph.
    pub fn all(maps: Vec<TableMap>, paras: &[Paragraph], width: &dyn Fn(usize) -> f32, pt: f32) -> (Vec<TableBox>, Vec<u32>) {
        let mut out: Vec<TableBox> = Vec::with_capacity(maps.len());
        let mut owner = vec![u32::MAX; paras.len()];
        // The last table at each depth: parents come before their tables,
        // so a table's parent is the last one a level up (a search back
        // through all would be quadratic with many tables side by side).
        let mut last: Vec<usize> = Vec::new();
        for m in maps {
            let parent = m.depth.checked_sub(1).and_then(|d| last.get(d).copied()).filter(|&k| k < out.len() && out[k].map.start <= m.start && m.end <= out[k].map.end);
            let (x0, w) = match parent {
                Some(k) => out[k].text_box(paras, m.start, pt),
                None => (0.0, width(m.start)),
            };
            let mut tb = TableBox::new(m, paras, w, pt);
            tb.x.iter_mut().for_each(|x| *x += x0);
            let k = out.len();
            for o in &mut owner[tb.map.start..tb.map.end.min(paras.len())] {
                *o = k as u32;
            }
            if let Some(pk) = parent {
                out[pk].kids.push(k);
            }
            last.truncate(tb.map.depth);
            last.resize(tb.map.depth, usize::MAX);
            last.push(k);
            out.push(tb);
        }
        (out, owner)
    }

    /// Left edge and width of the text of the cell holding `para`.
    pub fn text_box(&self, paras: &[Paragraph], para: usize, pt: f32) -> (f32, f32) {
        let (r, c) = self.map.pos_of(paras, para);
        let (ar, ac) = self.map.anchor_of(r, c);
        let (_, cs) = self.map.span_of(ar, ac);
        let [_, left, _, right] = self.map.props(paras).map_or(TableProps::default().margins(), |p| p.margins());
        let (x0, x1) = (self.x[ac], self.x[(ac + cs).min(self.map.cols)]);
        (x0 + left * pt, (x1 - x0 - (left + right) * pt).max(8.0))
    }

    /// Column under content x `x`.
    pub fn col_at(&self, x: f32) -> usize {
        self.x.partition_point(|e| *e <= x).saturating_sub(1).min(self.map.cols - 1)
    }

    /// Paragraph range of the (anchor) cell at row `r` under content x `x`.
    pub fn cell_at(&self, r: usize, x: f32) -> (usize, usize) {
        let (ar, ac) = self.map.anchor_of(r.min(self.map.rows - 1), self.col_at(x));
        self.map.cell(ar, ac)
    }
}

/// Places the rows of a table on the pages, continuing at (`page`, `y`).
pub(super) struct Paginator<'a> {
    pub paras: &'a [Paragraph],
    pub tables: &'a [TableBox],
    pub layouts: &'a [ParaLayout],
    pub content_h: f32,
    pub pt: f32,
    pub page: usize,
    pub y: f32,
    /// Top of the rows on the current page (below repeated headers).
    pub top_y: f32,
    /// Where the text starts on a page (a column of a section that starts
    /// halfway down its first page starts lower).
    pub frame_top: &'a dyn Fn(usize) -> f32,
    pub segs: &'a mut Vec<Segment>,
    pub ghosts: &'a mut Vec<Segment>,
    pub frags: &'a mut Vec<RowFrag>,
}

impl Paginator<'_> {
    /// The paragraphs and tables in the paragraphs `a..b` of a cell of
    /// table `index`, in order.
    fn items(&self, index: usize, a: usize, b: usize) -> Vec<Item> {
        let kids = &self.tables[index].kids;
        let mut out = Vec::new();
        let mut i = a;
        let mut k = kids.partition_point(|&t| self.tables[t].map.start < a);
        while i < b {
            if let Some(&t) = kids.get(k).filter(|&&t| self.tables[t].map.start == i) {
                out.push(Item::Table(t));
                i = self.tables[t].map.end.max(i + 1);
                k += 1;
            } else {
                out.push(Item::Para(i));
                i += 1;
            }
        }
        out
    }

    fn item_h(&self, it: &Item) -> f32 {
        match *it {
            Item::Para(i) => self.layouts[i].space_before + self.para_h(i) + self.layouts[i].space_after,
            Item::Table(t) => self.heights(t).iter().sum(),
        }
    }

    /// Height of a cell paragraph's text (its length when it is turned).
    fn para_h(&self, i: usize) -> f32 {
        let g = self.layouts[i].galley.size();
        if self.layouts[i].rotate != 0 { g.x } else { g.y }
    }

    /// Height of the content of the paragraphs `a..b` of a cell.
    fn content(&self, index: usize, a: usize, b: usize) -> f32 {
        let (top, bottom) = self.pads(index);
        self.items(index, a, b).iter().map(|it| self.item_h(it)).sum::<f32>() + top + bottom
    }

    /// Space above and below the text of the cells of table `index`.
    fn pads(&self, index: usize) -> (f32, f32) {
        let m = &self.tables[index].map;
        let props = m.props(self.paras);
        let [top, _, bottom, _] = props.map_or(TableProps::default().margins(), |p| p.margins());
        // As in Word and LibreOffice, the lines between rows take room:
        // half of one above the text and half below.
        let cell = m.cell_props(self.paras, 0, 0).map(|c| &c.borders);
        let table = props.map(|p| &p.borders);
        let half = |side: fn(&Borders) -> Option<Border>| cell.and_then(side).or_else(|| table.and_then(side)).map_or(0.0, |b| b.width / 2.0);
        ((top + half(|b| b.top)) * self.pt, (bottom + half(|b| b.bottom)) * self.pt)
    }

    /// Natural row heights of table `index`; a merged cell stretches the
    /// last row it spans.
    fn heights(&self, index: usize) -> Vec<f32> {
        let m = &self.tables[index].map;
        let default = TableProps::default();
        let props = m.props(self.paras).unwrap_or(&default);
        let mut h = vec![0.0f32; m.rows];
        for r in 0..m.rows {
            let min = props.row_heights.get(r).copied().filter(|v| v.is_finite()).unwrap_or(0.0).max(0.0) * self.pt;
            h[r] = min.max(4.0 * self.pt);
            // An exact height, whatever the text.
            if props.row_rule(r).exact && min > 0.0 {
                h[r] = min;
                continue;
            }
            for c in 0..m.cols {
                let (ar, ac) = m.anchor_of(r, c);
                if ac != c {
                    continue;
                }
                let (rs, _) = m.span_of(ar, ac);
                if ar + rs - 1 != r {
                    continue;
                }
                let (a, b) = m.cell(ar, ac);
                let above: f32 = h[ar..r].iter().sum();
                h[r] = h[r].max(self.content(index, a, b) - above);
            }
        }
        h
    }

    pub fn table(&mut self, index: usize, tb: &TableBox) {
        let m = &tb.map;
        let default = TableProps::default();
        let props = m.props(self.paras).unwrap_or(&default);
        let h = self.heights(index);
        let hr = (props.header_rows as usize).min(m.rows);
        let header_h: f32 = h[..hr].iter().sum();
        let repeat = hr > 0 && hr < m.rows && header_h <= self.content_h / 2.0;
        self.top_y = (self.frame_top)(self.page);
        let mut r = 0;
        while r < m.rows {
            // Rows joined by vertical merges move to a new page together.
            let mut g = r + 1;
            let mut k = r;
            while k < g {
                for c in 0..m.cols {
                    let (ar, ac) = m.anchor_of(k, c);
                    g = g.max(ar + m.span_of(ar, ac).0);
                }
                k += 1;
            }
            let g = g.min(m.rows);
            let gh: f32 = h[r..g].iter().sum();
            let room = self.content_h - if repeat && r >= hr { header_h } else { 0.0 };
            // Rows may break across pages (Word's default): a row that
            // doesn't fit starts here when its first line does. Rows as
            // short as their set height, or shorter than a sixth of the
            // page, move whole: measured against Word's page breaks, our
            // heights are close enough for that but not to break them
            // between the same lines.
            let rule = props.row_rule(r);
            let min_h = props.row_heights.get(r).copied().filter(|v| v.is_finite()).unwrap_or(0.0).max(0.0) * self.pt;
            let short = min_h >= gh - 0.5 || gh < self.content_h / 6.0;
            if !short && !rule.cant_split && !rule.exact && g == r + 1 && self.y > self.top_y + 0.01 && self.y + gh > self.content_h && self.content_h - self.y >= self.first_line(index, r) {
                self.split_row(index, tb, r, &h, if repeat && r >= hr { hr } else { 0 });
                r = g;
                continue;
            }
            if self.y > self.top_y + 0.01 && self.y + gh > self.content_h && (gh <= room || self.content_h - self.y < self.content_h / 4.0) {
                self.new_page(index, &h, if repeat && r >= hr { hr } else { 0 });
            }
            if gh <= self.content_h - self.y {
                for row in r..g {
                    self.row(index, row, &h, false);
                }
            } else {
                // Too tall for a page: the rows go one by one, and a row
                // taller than a page splits between lines.
                for row in r..g {
                    if self.y > self.top_y + 0.01 && self.y + h[row] > self.content_h && h[row] <= self.content_h {
                        self.new_page(index, &h, if repeat && row >= hr { hr } else { 0 });
                    }
                    if self.y + h[row] <= self.content_h {
                        self.row(index, row, &h, false);
                    } else {
                        self.split_row(index, tb, row, &h, if repeat && row >= hr { hr } else { 0 });
                    }
                }
            }
            r = g;
        }
    }

    /// Height a row needs on a page to start there: the first line of its
    /// tallest-starting cell.
    fn first_line(&self, index: usize, r: usize) -> f32 {
        let m = &self.tables[index].map;
        let pad = self.pads(index).0;
        let mut need = 0.0f32;
        for c in 0..m.cols {
            if m.anchor_of(r, c) != (r, c) {
                continue;
            }
            let (a, b) = m.cell(r, c);
            let first = match self.items(index, a, b).first() {
                Some(Item::Para(i)) => {
                    let pl = &self.layouts[*i];
                    let rows = &pl.galley.rows;
                    pl.space_before + rows.get(1).map_or(pl.galley.size().y, |n| n.pos.y)
                }
                Some(Item::Table(t)) => self.heights(*t).first().copied().unwrap_or(0.0),
                None => 0.0,
            };
            need = need.max(pad + first);
        }
        need
    }

    fn new_page(&mut self, index: usize, h: &[f32], headers: usize) {
        self.page += 1;
        self.y = (self.frame_top)(self.page);
        for row in 0..headers {
            self.row(index, row, h, true);
        }
        self.top_y = self.y;
    }

    /// Places row `r` at the current position.
    fn row(&mut self, index: usize, r: usize, h: &[f32], ghost: bool) {
        let (page, top) = (self.page, self.y);
        self.place_row(index, r, h, ghost, page, top);
        self.y = top + h[r];
    }

    /// Places row `r` of table `index` (with row heights `h`) at `top` of
    /// frame `page`, with the tables in its cells.
    fn place_row(&mut self, index: usize, r: usize, h: &[f32], ghost: bool, page: usize, top: f32) {
        let tables = self.tables;
        let m = &tables[index].map;
        self.frags.push(RowFrag { table: index as u32, row: r as u32, page, y: top, h: h[r], ghost, top: true, bottom: true, x: 0.0 });
        for c in 0..m.cols {
            let (a, b) = m.cell(r, c);
            let (ar, ac) = m.anchor_of(r, c);
            if (ar, ac) != (r, c) {
                if !ghost {
                    // Covered cells keep their (hidden) paragraphs.
                    self.hide(a, b, page, top);
                }
                continue;
            }
            let (rs, _) = m.span_of(r, c);
            let total: f32 = h[r..(r + rs).min(h.len())].iter().sum();
            let free = (total - self.content(index, a, b)).max(0.0);
            let valign = m.cell_props(self.paras, r, c).map_or(VAlign::Top, |p| p.valign);
            let mut y = top
                + self.pads(index).0
                + match valign {
                    VAlign::Top => 0.0,
                    VAlign::Center => free / 2.0,
                    VAlign::Bottom => free,
                };
            for it in self.items(index, a, b) {
                y = self.place_item(&it, ghost, page, y);
            }
        }
    }

    /// Places a paragraph or a table of a cell at `y` of frame `page`;
    /// returns the y below it.
    fn place_item(&mut self, it: &Item, ghost: bool, page: usize, mut y: f32) -> f32 {
        match *it {
            Item::Para(i) => {
                let pl = &self.layouts[i];
                y += pl.space_before;
                let gy1 = pl.galley.size().y.max(1.0);
                let seg = Segment { para: i, page, y, gy0: 0.0, gy1, cell: true, hidden: false, x: 0.0, w: 0.0 };
                if ghost { self.ghosts.push(seg) } else { self.segs.push(seg) }
                y + self.para_h(i) + pl.space_after
            }
            Item::Table(t) => {
                let h = self.heights(t);
                for r in 0..h.len() {
                    self.place_row(t, r, &h, ghost, page, y);
                    y += h[r];
                }
                y
            }
        }
    }

    /// Hidden segments for the paragraphs `a..b` of a covered cell.
    fn hide(&mut self, a: usize, b: usize, page: usize, top: f32) {
        for i in a..b {
            let gy1 = self.layouts[i].galley.size().y.max(1.0);
            self.segs.push(Segment { para: i, page, y: top, gy0: 0.0, gy1, cell: true, hidden: true, x: 0.0, w: 0.0 });
        }
    }

    /// Places a row taller than the rest of the page, breaking each cell's
    /// text between lines.
    /// The first `headers` rows are repeated above it on the pages it goes on to.
    fn split_row(&mut self, index: usize, tb: &TableBox, r: usize, h: &[f32], headers: usize) {
        let m = &tb.map;
        let row_h = h[r];
        let (page0, top0) = (self.page, self.y);
        let ft = self.frame_top;
        let header_h: f32 = h[..headers].iter().sum();
        let frag_top = |k: usize, top0: f32| if k == 0 { top0 } else { ft(page0 + k) + header_h };
        let content_h = self.content_h;
        let cap = |k: usize| content_h - frag_top(k, top0);
        let pad = self.pads(index).0;
        let mark = self.frags.len();
        // Height used on each page.
        let mut used: Vec<f32> = vec![0.0];
        for c in 0..m.cols {
            let (a, b) = m.cell(r, c);
            let (ar, ac) = m.anchor_of(r, c);
            if (ar, ac) != (r, c) {
                self.hide(a, b, page0, top0);
                continue;
            }
            let mut k = 0;
            let mut ly = pad;
            for it in self.items(index, a, b) {
                let i = match it {
                    Item::Para(i) => i,
                    Item::Table(t) => {
                        // Tables in the cell break between their rows.
                        let h = self.heights(t);
                        for (rr, &hr) in h.iter().enumerate() {
                            if ly + hr > cap(k) && ly > pad {
                                used[k] = used[k].max(ly);
                                k += 1;
                                if used.len() <= k {
                                    used.push(0.0);
                                }
                                ly = 0.0;
                            }
                            self.place_row(t, rr, &h, false, page0 + k, frag_top(k, top0) + ly);
                            ly += hr;
                            used[k] = used[k].max(ly);
                        }
                        continue;
                    }
                };
                let pl = &self.layouts[i];
                ly += pl.space_before;
                let rows = &pl.galley.rows;
                let size_y = pl.galley.size().y;
                let mut seg_y = ly;
                let mut gy0 = 0.0;
                for (ri, row) in rows.iter().enumerate() {
                    let rt = row.pos.y;
                    let rb = rows.get(ri + 1).map_or(size_y, |n| n.pos.y);
                    if ly + (rb - rt) > cap(k) && ly > 0.0 {
                        if rt > gy0 {
                            self.segs.push(Segment { para: i, page: page0 + k, y: frag_top(k, top0) + seg_y, gy0, gy1: rt, cell: true, hidden: false, x: 0.0, w: 0.0 });
                        }
                        used[k] = used[k].max(ly);
                        k += 1;
                        if used.len() <= k {
                            used.push(0.0);
                        }
                        ly = 0.0;
                        seg_y = 0.0;
                        gy0 = rt;
                    }
                    ly += rb - rt;
                }
                if rows.is_empty() {
                    ly += size_y;
                }
                self.segs.push(Segment { para: i, page: page0 + k, y: frag_top(k, top0) + seg_y, gy0, gy1: size_y.max(gy0 + 1.0), cell: true, hidden: false, x: 0.0, w: 0.0 });
                ly += pl.space_after;
                used[k] = used[k].max(ly);
            }
            used[k] = used[k].max(ly + pad);
        }
        let last = used.len() - 1;
        for k in 1..=last {
            let mut y = ft(page0 + k);
            for row in 0..headers {
                self.place_row(index, row, h, true, page0 + k, y);
                y += h[row];
            }
        }
        // The minimum row height counts on the last page.
        let rest = (row_h - (0..last).map(cap).sum::<f32>()).max(0.0);
        for (k, u) in used.iter().enumerate() {
            let h = if k < last { cap(k) } else { u.max(rest.min(self.content_h)) };
            self.frags.push(RowFrag { table: index as u32, row: r as u32, page: page0 + k, y: frag_top(k, top0), h, ghost: false, top: k == 0, bottom: k == last, x: 0.0 });
        }
        // The row's fills go under those of the tables in its cells.
        self.frags[mark..].rotate_right(used.len());
        self.page = page0 + last;
        if last > 0 {
            self.top_y = frag_top(last, top0);
        }
        self.y = frag_top(last, top0) + if last == 0 { used[0].max(row_h) } else { used[last].max(rest.min(self.content_h)) };
    }
}

/// What a cell holds: paragraphs and tables.
enum Item {
    Para(usize),
    Table(usize),
}

/// Stroke of a cell border; `None` for borders not drawn.
fn border_stroke(b: Border, pt: f32) -> Option<Stroke> {
    (!b.is_none()).then(|| Stroke::new((b.width * pt).max(1.0), theme::rgb(b.color)))
}

/// Light lines shown on screen for tables without borders.
const GRIDLINE: Color32 = Color32::from_rgb(0xC8, 0xD4, 0xE6);

impl super::DocView {
    /// Cell fills and the selected cells of the rows on the pages drawn.
    pub(super) fn paint_table_back(&self, p: &egui::Painter, l: &Layout, frags: &[RowFrag], origin: &dyn Fn(usize) -> Pos2, block: Option<&Block>) {
        let paras = &self.doc.paragraphs;
        for f in frags {
            let Some(tb) = l.tables.get(f.table as usize) else { continue };
            let m = &tb.map;
            let default = TableProps::default();
            let props = m.props(paras).unwrap_or(&default);
            let o = origin(f.page) + egui::vec2(f.x, 0.0);
            let r = f.row as usize;
            for c in 0..m.cols {
                let (ar, ac) = m.anchor_of(r, c);
                if ac != c {
                    continue;
                }
                let (_, cs) = m.span_of(ar, ac);
                let cell = m.cell_props(paras, ar, ac).cloned().unwrap_or_default();
                let look = props.cell_look(&self.doc.styles, &cell, ar, ac, m.rows, m.cols);
                let rect = Rect::from_min_max(pos2(o.x + tb.x[ac], o.y + f.y), pos2(o.x + tb.x[(ac + cs).min(m.cols)], o.y + f.y + f.h));
                if let Some(fill) = look.fill {
                    p.rect_filled(rect, 0.0, theme::rgb(fill));
                }
                if !f.ghost && block.is_some_and(|b| b.start == m.start && b.depth == m.depth && b.contains(ar, ac)) {
                    p.rect_filled(rect, 0.0, theme::Paper::TEXT_SELECTION);
                }
            }
        }
    }

    /// Cell borders (and light gridlines for borderless cells on screen).
    pub(super) fn paint_table_borders(&self, p: &egui::Painter, l: &Layout, frags: &[RowFrag], origin: &dyn Fn(usize) -> Pos2, z: f32, screen: bool) {
        let paras = &self.doc.paragraphs;
        let pt = super::PT * z;
        let grid = Stroke::new(1.0, GRIDLINE);
        for f in frags {
            let Some(tb) = l.tables.get(f.table as usize) else { continue };
            let m = &tb.map;
            let default = TableProps::default();
            let props = m.props(paras).unwrap_or(&default);
            let o = origin(f.page) + egui::vec2(f.x, 0.0);
            let r = f.row as usize;
            let (y0, y1) = (o.y + f.y, o.y + f.y + f.h);
            for c in 0..m.cols {
                let (ar, ac) = m.anchor_of(r, c);
                if ac != c {
                    continue;
                }
                let (rs, cs) = m.span_of(ar, ac);
                let cell = m.cell_props(paras, ar, ac).cloned().unwrap_or_default();
                let look = props.cell_look(&self.doc.styles, &cell, ar, ac, m.rows, m.cols);
                let (x0, x1) = (o.x + tb.x[ac], o.x + tb.x[(ac + cs).min(m.cols)]);
                let line = |b: Border, a: Pos2, e: Pos2| match border_stroke(b, pt) {
                    Some(s) => {
                        p.line_segment([a, e], s);
                    }
                    None if screen => {
                        p.line_segment([a, e], grid);
                    }
                    None => {}
                };
                if f.top && r == ar {
                    line(look.top, pos2(x0, y0), pos2(x1, y0));
                }
                if f.bottom && r == ar + rs - 1 {
                    line(look.bottom, pos2(x0, y1), pos2(x1, y1));
                }
                line(look.left, pos2(x0, y0), pos2(x0, y1));
                line(look.right, pos2(x1, y0), pos2(x1, y1));
            }
        }
    }
}

impl Layout {
    /// The table containing paragraph `para`.
    pub(super) fn table_at(&self, para: usize) -> Option<&TableBox> {
        self.tables.get(*self.owner.get(para)? as usize)
    }

    /// Row fragments on pages `a..=b`.
    pub(super) fn frags_on(&self, a: usize, b: usize) -> &[RowFrag] {
        let s = self.frags.partition_point(|f| f.page < a);
        let e = self.frags.partition_point(|f| f.page <= b);
        &self.frags[s..e]
    }

    pub(super) fn ghosts_on(&self, a: usize, b: usize) -> &[Segment] {
        let s = self.ghosts.partition_point(|f| f.page < a);
        let e = self.ghosts.partition_point(|f| f.page <= b);
        &self.ghosts[s..e]
    }

    /// The table row (not a repeated header) at content point (x, y) of a
    /// page: the table and the paragraph range of the cell there.
    pub(super) fn cell_hit(&self, page: usize, x: f32, y: f32) -> Option<(usize, usize)> {
        // The innermost table there (tables in cells come after their row).
        self.frags_on(page, page).iter().rev().filter(|f| !f.ghost && y >= f.y && y < f.y + f.h).find_map(|f| {
            let tb = self.tables.get(f.table as usize)?;
            let x = x - f.x;
            let slop = if tb.map.depth == 0 { 20.0 } else { 0.0 };
            (x >= tb.x[0] - slop && x <= tb.x[tb.x.len() - 1] + slop).then(|| tb.cell_at(f.row as usize, x))
        })
    }

    /// A column edge near content point (x, y) of a page: the table's first
    /// paragraph and the edge index (0 = the table's left edge).
    pub(super) fn edge_hit(&self, page: usize, x: f32, y: f32, slop: f32) -> Option<(usize, usize)> {
        for f in self.frags_on(page, page).iter().rev() {
            if y < f.y || y >= f.y + f.h {
                continue;
            }
            let tb = self.tables.get(f.table as usize)?;
            let (e, d) = tb.x.iter().enumerate().map(|(i, e)| (i, (e + f.x - x).abs())).min_by(|a, b| a.1.total_cmp(&b.1))?;
            if d <= slop {
                return Some((tb.map.start, e));
            }
        }
        None
    }
}
