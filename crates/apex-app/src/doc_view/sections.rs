//! Pagination by sections: each section has its own page size, margins
//! and text columns, and starts on a new page (or an even or odd one) or
//! right below the previous one. Text flows from column to column; the
//! columns of a section followed by a continuous one are balanced.
//!
//! The flow places text in "frames" (a column of a page) numbered one
//! after the other, so the table paginator, which knows only about pages,
//! places rows in frames; they're mapped to pages and columns at the end.

use apex_doc::{Columns, Document, PageSetup, PageVAlign, Paragraph, SectionStart};
use eframe::egui::{Vec2, vec2};

use super::{PAGE_GAP, ParaLayout, Segment, decor, notes::FootPager, tables};

/// A page's size and place on the canvas (pixels).
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct PageGeo {
    /// Canvas y of the page's top edge.
    pub top: f32,
    pub size: Vec2,
    /// Offset that centers a page narrower than the widest one.
    pub dx: f32,
    /// Left and top margins.
    pub margin: Vec2,
    pub content: Vec2,
    /// The section whose setup (and headers) the page has, and whether
    /// it's that section's first page.
    pub sect: usize,
    pub first: bool,
    /// Vertical text (tategaki): the text is laid out on the page turned
    /// a quarter to the left and drawn turned back (see `tategaki`).
    pub vertical: bool,
}

/// A line between two columns (content coordinates of its page).
#[derive(Clone, Copy, Debug)]
pub(super) struct Rule {
    pub page: usize,
    pub x: f32,
    pub y0: f32,
    pub y1: f32,
}

/// A section's geometry in pixels.
pub(super) struct Sect {
    pub first: usize,
    pub end: usize,
    pub start: SectionStart,
    /// The page as the text flows on it (turned for vertical text).
    pub setup: PageSetup,
    /// The page itself.
    pub page: PageSetup,
    /// Vertical text, with Asian characters upright.
    pub vertical: bool,
    pub upright: bool,
    /// Column left edges and widths from the section's text area's left.
    pub cols: Vec<(f32, f32)>,
    pub separator: bool,
    columns: Columns,
}

impl Sect {
    /// The sections of `doc`; vertical ones are turned when `turn`.
    pub fn all(doc: &Document, pt: f32, turn: bool) -> Vec<Sect> {
        doc.sections()
            .iter()
            .map(|s| {
                let mut out = Sect {
                    first: s.first,
                    end: s.end,
                    start: s.start,
                    setup: *s.page,
                    page: *s.page,
                    vertical: turn && s.page.vertical(),
                    upright: turn && s.page.upright(),
                    cols: Vec::new(),
                    separator: s.columns.separator,
                    columns: s.columns.clone(),
                };
                out.set_page(*s.page, pt);
                out
            })
            .collect()
    }

    /// Sets the page up (again, after the headers made room for
    /// themselves).
    pub fn set_page(&mut self, page: PageSetup, pt: f32) {
        self.page = page;
        self.setup = if self.vertical { page.flow_setup() } else { page };
        self.cols = self.columns.layout(self.setup.content_width().max(10.0)).into_iter().map(|(x, w)| (x * pt, w * pt)).collect();
    }

    /// The geometry of page `index` of the document.
    fn geo(&self, pt: f32, index: usize) -> PageGeo {
        let p = &self.page;
        PageGeo { top: 0.0, size: vec2(p.width, p.height) * pt, dx: 0.0, margin: vec2(p.text_left(index), p.text_top()) * pt, content: vec2(p.content_width().max(10.0), p.content_height().max(10.0)) * pt, sect: 0, first: false, vertical: self.vertical }
    }

    /// Width a paragraph of this section is laid out in.
    pub fn width(&self) -> f32 {
        self.cols.first().map_or(100.0, |c| c.1)
    }
}

/// The section of each paragraph.
pub(super) fn sect_of(sects: &[Sect], n: usize) -> Vec<u32> {
    let mut out = vec![0; n];
    for (k, s) in sects.iter().enumerate() {
        for v in out.iter_mut().take(s.end.min(n)).skip(s.first) {
            *v = k as u32;
        }
    }
    out
}

/// The column (its left edge) nearest to content x `x` among a page's
/// segments; `None` when they're all in one column.
pub(super) fn column_at<'a>(segs: impl Iterator<Item = &'a Segment>, x: f32) -> Option<f32> {
    // (x, distance, width): on a tie, the narrower one (a column rather
    // than a full-width paragraph of another section).
    let mut best: Option<(f32, f32, f32)> = None;
    let mut first = None;
    let mut several = false;
    for s in segs {
        match first {
            None => first = Some(s.x),
            Some(f) if (f - s.x).abs() > 0.5 => several = true,
            _ => {}
        }
        let d = if x < s.x { s.x - x } else { (x - s.x - s.w).max(0.0) };
        if best.is_none_or(|b| d < b.1 || (d == b.1 && s.w < b.2)) {
            best = Some((s.x, d, s.w));
        }
    }
    best.filter(|_| several).map(|b| b.0)
}

/// A section's frames: frame `f0 + j` is column `j % n` of page
/// `p0 + j / n`; the first page's columns start at `t0`.
#[derive(Clone, Copy, Debug)]
struct Run {
    sect: usize,
    f0: usize,
    p0: usize,
    t0: f32,
    n: usize,
    /// On the first page: the section's text area from the page's (they
    /// differ when a continuous section changes the margins).
    xoff: f32,
    /// The first page's text height.
    h0: f32,
    /// Starts a new page (after `blank` empty pages).
    new_page: bool,
    blank: usize,
}

impl Run {
    fn local(&self, f: usize) -> usize {
        f.saturating_sub(self.f0)
    }

    fn page(&self, f: usize) -> usize {
        self.p0 + self.local(f) / self.n
    }

    fn top(&self, f: usize) -> f32 {
        if self.local(f) < self.n { self.t0 } else { 0.0 }
    }
}

pub(super) struct Paged {
    pub segs: Vec<Segment>,
    pub ghosts: Vec<Segment>,
    pub frags: Vec<tables::RowFrag>,
    pub geo: Vec<PageGeo>,
    pub rules: Vec<Rule>,
    pub max_size: Vec2,
    /// Page and y where the text ends (the endnotes follow).
    pub end: (usize, f32),
}

struct Flow<'a> {
    fp: &'a mut FootPager,
    paras: &'a [Paragraph],
    layouts: &'a [ParaLayout],
    tables: &'a [tables::TableBox],
    sects: &'a [Sect],
    pt: f32,
    segs: Vec<Segment>,
    ghosts: Vec<Segment>,
    frags: Vec<tables::RowFrag>,
    runs: Vec<Run>,
    /// Current frame and y in it.
    f: usize,
    y: f32,
    /// Lowest point reached on the current page (for a continuous section).
    low_page: usize,
    low: f32,
    /// The frame starts the document or follows a page break: like Word
    /// and LibreOffice, its first paragraph keeps its space before.
    hard_top: bool,
}

/// Places the paragraphs of a document on pages.
pub(super) fn paginate(paras: &[Paragraph], layouts: &[ParaLayout], tables: &[tables::TableBox], sects: &[Sect], pt: f32, fp: &mut FootPager) -> Paged {
    let mut fl = Flow { fp, paras, layouts, tables, sects, pt, segs: Vec::with_capacity(layouts.len()), ghosts: Vec::new(), frags: Vec::new(), runs: Vec::new(), f: 0, y: 0.0, low_page: 0, low: 0.0, hard_top: true };
    for (k, s) in sects.iter().enumerate() {
        fl.start_section(k);
        // Footnotes are placed once, so columns with them aren't balanced.
        let balance = s.cols.len() > 1 && sects.get(k + 1).is_some_and(|n| n.start == SectionStart::Continuous) && !fl.fp.has_marks(s.first..s.end);
        let mark = (fl.segs.len(), fl.ghosts.len(), fl.frags.len());
        fl.section(k, None);
        if balance {
            fl.balance(k, mark);
        }
    }
    fl.finish()
}

impl Flow<'_> {
    fn run(&self) -> Run {
        self.runs[self.runs.len() - 1]
    }

    fn page_of(&self, f: usize) -> usize {
        self.run().page(f)
    }

    /// Text height of frame `f`'s page for the current section.
    fn bottom(&self, f: usize, limit: Option<f32>) -> f32 {
        let r = self.run();
        let h = if r.local(f) < r.n { r.h0 } else { self.sects[r.sect].setup.content_height().max(10.0) * self.pt };
        limit.map_or(h, |l| l.min(h))
    }

    fn note(&mut self) {
        let p = self.page_of(self.f);
        if p > self.low_page {
            self.low_page = p;
            self.low = self.y;
        } else {
            self.low = self.low.max(self.y);
        }
    }

    /// Opens section `k`'s first frame.
    fn start_section(&mut self, k: usize) {
        let s = &self.sects[k];
        let n = s.cols.len().max(1);
        let pt = self.pt;
        let Some(prev) = self.runs.last().copied() else {
            self.runs.push(Run { sect: k, f0: 0, p0: 0, t0: 0.0, n, xoff: 0.0, h0: s.setup.content_height().max(10.0) * pt, new_page: true, blank: 0 });
            return;
        };
        let cur_page = prev.page(self.f);
        let f0 = self.f + 1;
        // The page the text so far ends on, and its section.
        let ps = &self.sects[self.page_sect(cur_page)].setup;
        // (Horizontal and vertical text never share a page.)
        let same_size = (ps.width - s.setup.width).abs() < 0.5 && (ps.height - s.setup.height).abs() < 0.5 && self.sects[self.page_sect(cur_page)].vertical == s.vertical;
        let used = if self.low_page == cur_page { self.low } else { self.y };
        let room = ps.content_height() * pt - used;
        if s.start == SectionStart::Continuous && same_size && room > 20.0 * pt {
            let xoff = (s.setup.text_left(cur_page) - ps.text_left(cur_page)) * pt;
            let h0 = ps.content_height().max(10.0) * pt;
            self.runs.push(Run { sect: k, f0, p0: cur_page, t0: used, n, xoff, h0, new_page: false, blank: 0 });
            self.f = f0;
            self.y = used;
            return;
        }
        let mut p0 = cur_page + 1;
        let mut blank = 0;
        // Page numbers are 1-based: an even page has an odd index.
        let wrong = match s.start {
            SectionStart::EvenPage => p0 % 2 == 0,
            SectionStart::OddPage => p0 % 2 == 1,
            _ => false,
        };
        if wrong {
            p0 += 1;
            blank = 1;
        }
        self.runs.push(Run { sect: k, f0, p0, t0: 0.0, n, xoff: 0.0, h0: s.setup.content_height().max(10.0) * pt, new_page: true, blank });
        self.f = f0;
        self.y = 0.0;
        self.hard_top = true;
        self.low_page = p0;
        self.low = 0.0;
    }

    /// The section whose setup page `page` has: the one that started it.
    fn page_sect(&self, page: usize) -> usize {
        self.runs.iter().rev().find(|r| if r.new_page { r.p0 <= page } else { r.p0 < page }).map_or(0, |r| r.sect)
    }

    fn next_frame(&mut self) {
        self.hard_top = false;
        self.f += 1;
        self.y = self.run().top(self.f);
    }

    /// Places the paragraphs of section `k` from the current frame; with a
    /// `limit`, columns end there (to balance them).
    fn section(&mut self, k: usize, limit: Option<f32>) {
        let s = &self.sects[k];
        let (paras, layouts) = (self.paras, self.layouts);
        // Tables in cells are placed with their cell.
        let tops: Vec<usize> = (0..self.tables.len()).filter(|&t| self.tables[t].map.depth == 0).collect();
        let mut ti = tops.partition_point(|&t| self.tables[t].map.end <= s.first);
        let mut i = s.first;
        while i < s.end.min(layouts.len()) {
            let top = self.run().top(self.f);
            let at_top = self.y <= top + 0.01;
            let st = &paras[i].style;
            if i > s.first || k == 0 {
                if st.page_break_before && (!at_top || i > 0) && paras[i].cell.is_none() {
                    let r = self.run();
                    let j = r.local(self.f);
                    self.f = r.f0 + (j / r.n + 1) * r.n;
                    self.y = 0.0;
                    self.hard_top = true;
                } else if st.column_break_before && i > s.first && paras[i].cell.is_none() {
                    self.next_frame();
                }
            }
            if paras[i].cell.is_some() {
                while tops.get(ti).is_some_and(|&t| self.tables[t].map.end <= i) {
                    ti += 1;
                }
                if let Some((index, tb)) = tops.get(ti).map(|&t| (t, &self.tables[t])).filter(|(_, t)| t.map.start == i) {
                    let run = self.run();
                    let frame_top = move |f: usize| run.top(f);
                    let content_h = self.bottom(self.f, limit);
                    let mut pg = tables::Paginator {
                        paras,
                        tables: self.tables,
                        layouts,
                        content_h,
                        pt: self.pt,
                        page: self.f,
                        y: self.y,
                        top_y: 0.0,
                        frame_top: &frame_top,
                        segs: &mut self.segs,
                        ghosts: &mut self.ghosts,
                        frags: &mut self.frags,
                    };
                    pg.table(index, tb);
                    (self.f, self.y) = (pg.page, pg.y);
                    self.hard_top = false;
                    self.note();
                    i = tb.map.end.max(i + 1);
                    continue;
                }
            }
            self.paragraph(i, limit);
            i += 1;
        }
    }

    fn paragraph(&mut self, i: usize, limit: Option<f32>) {
        let pl = &self.layouts[i];
        let st = &self.paras[i].style;
        let (room_top, room_bottom) = decor::border_room(self.paras, i);
        let (room_top, room_bottom) = (room_top * self.pt, room_bottom * self.pt);
        let top = self.run().top(self.f);
        if self.y > top + 0.01 || std::mem::take(&mut self.hard_top) {
            self.y += pl.space_before;
        }
        self.y += room_top;
        let mut bottom = self.bottom(self.f, limit);
        // "Keep with next": a short paragraph moves to the next frame with
        // the first line of the following one.
        if st.keep_with_next && self.y > top + room_top + 0.01 && pl.galley.size().y < bottom / 2.0 {
            let next = self.layouts.get(i + 1).map_or(0.0, |n| n.space_before + n.galley.rows.first().map_or(0.0, |r| r.row.size.y));
            if self.y + pl.galley.size().y + pl.space_after + next > bottom {
                self.next_frame();
                self.y += room_top;
                bottom = self.bottom(self.f, limit);
            }
        }
        let rows = &pl.galley.rows;
        // How many lines fit in this frame decides what widow/orphan
        // control and keeping lines together do.
        let row_h = |ri: usize| rows.get(ri + 1).map_or(pl.galley.size().y, |n| n.pos.y) - rows[ri].pos.y;
        let mut forced_break = None;
        if rows.len() > 1 && self.y > top + room_top + 0.01 {
            let foot = self.fp.foot_h(self.page_of(self.f));
            let mut y = self.y;
            let fit = (0..rows.len()).take_while(|&ri| {
                y += row_h(ri);
                y <= bottom - foot
            });
            let fit = fit.count();
            let whole = pl.galley.size().y <= bottom - top;
            if fit < rows.len() && ((st.keep_lines && whole) || (!st.no_widow_control && fit == 1)) {
                // Keep together, or no orphan: all of it to the next frame.
                self.next_frame();
                self.y += room_top;
                bottom = self.bottom(self.f, limit);
            } else if !st.no_widow_control && fit + 1 == rows.len() && fit >= 2 {
                // No widow: one more line goes over.
                forced_break = Some(fit - 1);
            }
        }
        let mut seg_y = self.y;
        let mut gy0 = 0.0;
        let seg = |para, page, y, gy0, gy1| Segment { para, page, y, gy0, gy1, cell: false, hidden: false, x: 0.0, w: 0.0 };
        for (ri, row) in rows.iter().enumerate() {
            let rtop = row.pos.y;
            let rbottom = rows.get(ri + 1).map_or(pl.galley.size().y, |n| n.pos.y);
            let h = rbottom - rtop;
            let ftop = self.run().top(self.f);
            let page = self.page_of(self.f);
            let need = self.fp.need(i, pl, ri, page);
            if (forced_break == Some(ri) || self.y + h + need > bottom - self.fp.foot_h(page)) && self.y > ftop + 0.01 {
                if rtop > gy0 {
                    self.segs.push(seg(i, self.f, seg_y, gy0, rtop));
                }
                self.note();
                self.next_frame();
                bottom = self.bottom(self.f, limit);
                seg_y = self.y;
                gy0 = rtop;
            }
            self.y += h;
            let page = self.page_of(self.f);
            self.fp.place(i, pl, ri, page, self.y);
        }
        self.segs.push(seg(i, self.f, seg_y, gy0, pl.galley.size().y.max(gy0 + 1.0)));
        if rows.is_empty() {
            self.y += pl.galley.size().y;
        }
        self.y += room_bottom;
        self.note();
        self.y += pl.space_after;
    }

    /// Balances the columns of section `k` when it fits on the page it
    /// starts on: the lowest column end that keeps all of it there.
    fn balance(&mut self, k: usize, mark: (usize, usize, usize)) {
        let r = self.run();
        if r.local(self.f) >= r.n {
            return;
        }
        let state = (self.f, self.y, self.low_page, self.low);
        let (mut lo, mut hi) = (r.t0, self.bottom(r.f0, None));
        let again = |fl: &mut Flow, limit: f32| {
            fl.segs.truncate(mark.0);
            fl.ghosts.truncate(mark.1);
            fl.frags.truncate(mark.2);
            (fl.f, fl.y, fl.low_page, fl.low) = (r.f0, r.t0, r.p0, r.t0);
            fl.section(k, Some(limit));
            fl.run().local(fl.f) < r.n
        };
        for _ in 0..14 {
            if hi - lo < 1.0 {
                break;
            }
            let mid = (lo + hi) / 2.0;
            if again(self, mid) { hi = mid } else { lo = mid }
        }
        if !again(self, hi) {
            // Can't happen (it fit unlimited), but keep the plain layout.
            let _ = again(self, f32::MAX);
            (self.f, self.y, self.low_page, self.low) = state;
        }
        // The next section starts below the longest column.
        self.low_page = r.p0;
        self.low = self.low.max(self.y);
    }

    fn finish(mut self) -> Paged {
        let pt = self.pt;
        let end_page = self.page_of(self.f);
        let end = (end_page, if self.low_page == end_page { self.low.max(self.y) } else { self.y });
        let run_of = |runs: &[Run], f: usize| runs[runs.partition_point(|r| r.f0 <= f).saturating_sub(1)];
        // Frames to pages and columns.
        let runs = std::mem::take(&mut self.runs);
        let place = |s: &mut Segment| {
            let r = run_of(&runs, s.page);
            let j = r.local(s.page);
            let (x, w) = self.sects[r.sect].cols.get(j % r.n).copied().unwrap_or((0.0, 100.0));
            s.page = r.page(s.page);
            s.x = x + if j < r.n { r.xoff } else { 0.0 };
            s.w = w;
        };
        self.segs.iter_mut().for_each(place);
        self.ghosts.iter_mut().for_each(place);
        for fr in &mut self.frags {
            let r = run_of(&runs, fr.page);
            let j = r.local(fr.page);
            fr.x = self.sects[r.sect].cols.get(j % r.n).map_or(0.0, |c| c.0) + if j < r.n { r.xoff } else { 0.0 };
            fr.page = r.page(fr.page);
        }
        self.ghosts.sort_by_key(|g| g.page);
        self.frags.sort_by_key(|f| f.page);
        // Pages: each section's pages, from its first to the last text
        // placed (a blank page before an even or odd one is the previous
        // section's).
        let last = self.segs.iter().chain(&self.ghosts).map(|s| s.page).chain(self.frags.iter().map(|f| f.page)).chain(runs.last().map(|r| r.p0)).max().unwrap_or(0);
        let mut sect_of_page = vec![0usize; last + 1];
        for r in &runs {
            let from = if r.new_page { r.p0 } else { r.p0 + 1 };
            for p in sect_of_page.iter_mut().skip(from) {
                *p = r.sect;
            }
        }
        self.align_pages(&runs, last);
        let mut geo: Vec<PageGeo> = sect_of_page
            .iter()
            .enumerate()
            .map(|(p, &k)| PageGeo { sect: k, first: p == 0 || sect_of_page[p - 1] != k, ..self.sects[k].geo(pt, p) })
            .collect();
        let max_size = geo.iter().fold(Vec2::ZERO, |m, g| m.max(g.size));
        let mut top = PAGE_GAP;
        for g in &mut geo {
            g.top = top;
            g.dx = ((max_size.x - g.size.x) / 2.0).max(0.0);
            top += g.size.y + PAGE_GAP;
        }
        // Lines between columns, down to the lowest text of each page.
        let mut rules = Vec::new();
        for (ri, r) in runs.iter().enumerate() {
            let s = &self.sects[r.sect];
            if !s.separator || r.n < 2 {
                continue;
            }
            let para_end = s.end;
            let p_last = runs.get(ri + 1).map_or(last, |n| if n.new_page { n.p0.saturating_sub(n.blank + 1) } else { n.p0 });
            for p in r.p0..=p_last.min(last) {
                let y0 = if p == r.p0 { r.t0 } else { 0.0 };
                let y1 = self.segs.iter().filter(|g| g.page == p && (s.first..para_end).contains(&g.para)).map(|g| g.y + g.gy1 - g.gy0).fold(y0, f32::max);
                if y1 <= y0 + 1.0 {
                    continue;
                }
                let off = if p == r.p0 { r.xoff } else { 0.0 };
                for w in s.cols.windows(2) {
                    rules.push(Rule { page: p, x: off + (w[0].0 + w[0].1 + w[1].0) / 2.0, y0, y1 });
                }
            }
        }
        Paged { segs: self.segs, ghosts: self.ghosts, frags: self.frags, geo, rules, max_size, end }
    }

    /// Moves the text of the pages of sections aligned to the center, the
    /// bottom or both edges (single column pages only a section fills).
    fn align_pages(&mut self, runs: &[Run], last: usize) {
        for (ri, r) in runs.iter().enumerate() {
            let how = self.sects[r.sect].setup.v_align;
            if how == PageVAlign::Top || r.n != 1 || !r.new_page {
                continue;
            }
            let p_last = runs.get(ri + 1).map_or(last, |n| if n.new_page { n.p0.saturating_sub(n.blank + 1) } else { n.p0.saturating_sub(1) });
            for p in r.p0..=p_last.min(last) {
                let room = self.fp.limit(r.f0 + (p - r.p0));
                let used = self.segs.iter().chain(&self.ghosts).filter(|s| s.page == p).map(|s| s.y + s.gy1 - s.gy0).chain(self.frags.iter().filter(|f| f.page == p).map(|f| f.y + f.h)).fold(0.0, f32::max);
                let extra = room - used;
                if extra < 1.0 || used <= 0.0 {
                    continue;
                }
                // The tops of what moves together: paragraphs and tables.
                let mut blocks: Vec<f32> = self.segs.iter().filter(|s| s.page == p && !s.cell).map(|s| s.y).collect();
                let mut tables: Vec<(u32, f32)> = Vec::new();
                for f in self.frags.iter().filter(|f| f.page == p) {
                    match tables.iter_mut().find(|t| t.0 == f.table) {
                        Some(t) => t.1 = t.1.min(f.y),
                        None => tables.push((f.table, f.y)),
                    }
                }
                blocks.extend(tables.iter().map(|t| t.1));
                blocks.sort_by(f32::total_cmp);
                blocks.dedup_by(|a, b| (*a - *b).abs() < 0.01);
                let gaps = blocks.len().saturating_sub(1).max(1) as f32;
                let shift = |y: f32| match how {
                    PageVAlign::Center => extra / 2.0,
                    PageVAlign::Bottom => extra,
                    _ => extra * blocks.partition_point(|b| *b <= y + 0.01).saturating_sub(1) as f32 / gaps,
                };
                for s in self.segs.iter_mut().chain(self.ghosts.iter_mut()).filter(|s| s.page == p) {
                    s.y += shift(s.y);
                }
                for f in self.frags.iter_mut().filter(|f| f.page == p) {
                    f.y += shift(f.y);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::{DocView, Layout, PT};
    use apex_doc::{DropCap, Pos, Section, TabAlign, TabLeader, TabStop, TabStops};
    use eframe::egui;

    fn laid_out(doc: Document) -> (DocView, egui::Context) {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let frame = || ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        frame();
        frame();
        let mut v = DocView::new(doc);
        v.layout = v.build_layout(&ctx, 1.0, (1, 0, 0, 0, 0), Layout::default());
        (v, ctx)
    }

    #[test]
    fn footnotes_in_columns() {
        let text: Vec<String> = (0..80).map(|i| format!("Paragraph {i} with some words to fill the line and more.")).collect();
        let mut doc = Document::from_plain_text(&text.join("\n"));
        doc.columns = Columns::equal(2, 36.0);
        doc.paragraphs[60].section = Some(Box::new(Section { headers: Default::default(), start: SectionStart::Continuous, page: doc.page, columns: Columns::default() }));
        let at = Pos::new(5, 4);
        doc.insert_note(apex_doc::Selection::caret(at), apex_doc::NoteKind::Foot);
        let mut n = (*doc.note_at(at).unwrap().1).clone();
        n.set_text("A footnote in a column.");
        doc.set_note(at, n);
        let (v, _ctx) = laid_out(doc);
        let l = &v.layout;
        let np = &l.refs.notes.pages;
        assert!(np.first().is_some_and(|p| !p.foot.is_empty() && p.foot_h > 0.0));
        // Both columns stop above the notes.
        for s in l.segs.iter().filter(|s| s.page == 0) {
            assert!(s.y + (s.gy1 - s.gy0) <= l.content.y - np[0].foot_h + 1.0, "paragraph {}", s.para);
        }
        assert!(l.segs.iter().any(|s| s.page == 0 && s.x > 0.0));
    }

    #[test]
    fn columns_and_sections() {
        let text: Vec<String> = (0..60).map(|i| format!("Paragraph {i} with some words to fill the line.")).collect();
        let mut doc = Document::from_plain_text(&text.join("\n"));
        // 1..20: two balanced columns; 20..: a landscape section on a new
        // page; a column break at 10.
        doc.paragraphs[1].section = Some(Box::new(Section { headers: Default::default(), start: SectionStart::Continuous, page: doc.page, columns: Columns { separator: true, ..Columns::equal(2, 36.0) } }));
        doc.paragraphs[20].section = Some(Box::new(Section { headers: Default::default(), start: SectionStart::Continuous, page: doc.page, columns: Columns::default() }));
        let mut land = doc.page;
        land.set_landscape(true);
        doc.paragraphs[21].section = Some(Box::new(Section { headers: Default::default(), start: SectionStart::OddPage, page: land, columns: Columns::default() }));
        let (v, _ctx) = laid_out(doc);
        let l = &v.layout;
        let s1: Vec<_> = (1..20).flat_map(|i| l.segs_of(i).iter().copied()).collect();
        assert!(s1.iter().all(|s| s.page == 0));
        let xs: Vec<f32> = s1.iter().map(|s| s.x).collect();
        let right = xs.iter().copied().fold(0.0, f32::max);
        assert!(right > 100.0 && xs.contains(&0.0));
        // Balanced: the columns end at about the same height (within a
        // paragraph: two-line paragraphs don't split, no orphans).
        let end = |x: f32| s1.iter().filter(|s| s.x == x).map(|s| s.y + s.gy1 - s.gy0).fold(0.0, f32::max);
        assert!((end(0.0) - end(right)).abs() < 60.0, "{} {}", end(0.0), end(right));
        // The continuous section goes below them, full width.
        let s20 = l.segs_of(20)[0];
        assert!(s20.page == 0 && s20.x == 0.0 && s20.y >= end(0.0).max(end(right)) - 1.0);
        assert!(!l.rules.is_empty());
        // Odd page: page 0 is page 1, so the landscape section starts on
        // page 3 after a blank page 2.
        let s21 = l.segs_of(21)[0];
        assert_eq!(s21.page, 2);
        assert!(l.geo[2].size.x > l.geo[2].size.y && l.geo[1].size.x < l.geo[1].size.y);
        assert_eq!(l.page_segs(1, 1).count(), 0);
        // A click in the right column lands there.
        let p = s1.iter().find(|s| s.x == right).map(|s| s.para).unwrap_or(0);
        let r = v.caret_rect(Pos::new(p, 0));
        assert_eq!(v.pos_at(r.center()).para, p);
        assert!(l.page_at_y(r.center().y) == 0 && l.page_at_y(l.geo[2].top + 5.0) == 2);
    }

    #[test]
    fn tabs_and_drop_caps() {
        let words = "word ".repeat(80);
        let mut doc = Document::from_plain_text(&format!("Name\t12.50\n{words}"));
        doc.paragraphs[0].style.tabs = TabStops::from_slice(&[TabStop { pos: 200.0, align: TabAlign::Right, leader: TabLeader::Dots }]);
        doc.paragraphs[1].style.drop_cap = Some(DropCap::default());
        let (v, _ctx) = laid_out(doc);
        let pl = &v.layout.paras[0];
        let row = &pl.galley.rows[0];
        let last = row.row.glyphs.last().unwrap();
        let end = (row.pos.x + last.max_x()) / PT;
        assert!((end - 200.0).abs() < 2.0, "{end}");
        assert_eq!(pl.tab_fills.len(), 1);
        // Center, right and decimal stops in one row.
        let mut doc = Document::from_plain_text("a\tb\tc\t1.5");
        let stops = [(72.0, TabAlign::Center), (300.0, TabAlign::Right), (350.0, TabAlign::Decimal)];
        doc.paragraphs[0].style.tabs = TabStops::from_slice(&stops.map(|(pos, align)| TabStop { pos, align, leader: TabLeader::Dots }));
        let (v2, _ctx2) = laid_out(doc);
        let row = &v2.layout.paras[0].galley.rows[0];
        let at = |c: char| row.row.glyphs.iter().find(|g| g.chr == c).map(|g| (row.pos.x + g.pos.x) / PT).unwrap_or(-1.0);
        let end_c = row.row.glyphs.iter().find(|g| g.chr == 'c').map(|g| (row.pos.x + g.max_x()) / PT).unwrap_or(-1.0);
        assert!((end_c - 300.0).abs() < 2.0 && (at('.') - 350.0).abs() < 2.0, "c ends {end_c}, point at {}", at('.'));
        let cap = &v.layout.paras[1].galley;
        // The cap's row, three rows beside it and the rest under them.
        assert!(cap.rows[0].row.glyphs[0].font_height > 2.5 * cap.rows[1].row.glyphs[0].font_height);
        assert!(cap.rows[1..4].iter().all(|r| r.pos.x > 20.0));
        assert!(cap.rows[4].pos.x < 1.0 && cap.rows[4].pos.y > cap.rows[3].pos.y);
        // Characters are all there, in order.
        let n: usize = cap.rows.iter().map(|r| r.char_count_including_newline().0).sum();
        assert_eq!(n, v.doc.paragraphs[1].text.chars().count());
    }

    #[test]
    fn runs_map_frames() {
        let r = Run { sect: 1, f0: 10, p0: 3, t0: 50.0, n: 2, xoff: 0.0, h0: 500.0, new_page: false, blank: 0 };
        assert_eq!([r.page(10), r.page(11), r.page(12), r.page(15)], [3, 3, 4, 5]);
        assert_eq!([r.top(10), r.top(11), r.top(12)], [50.0, 50.0, 0.0]);
    }

    #[test]
    fn headers_of_centred_pages_keep_their_height() {
        let text: Vec<String> = (0..100).map(|i| format!("Paragraph {i}")).collect();
        let pages = |v_align| {
            let mut doc = Document::from_plain_text(&text.join("\n"));
            doc.page.v_align = v_align;
            doc.headers.set(apex_doc::HfSlot::Header, Some(std::sync::Arc::new(vec![apex_doc::Paragraph::new("Header")])));
            laid_out(doc).0.layout.pages
        };
        let top = pages(apex_doc::PageVAlign::Top);
        assert!((2..=4).contains(&top), "{top}");
        assert_eq!(pages(apex_doc::PageVAlign::Center), top);
    }

    /// 20 lines of text, then a table whose second row has `n` paragraphs.
    fn tall_row(n: usize, rule: apex_doc::RowRule, height: f32) -> (DocView, egui::Context) {
        let mut t = apex_doc::Table::new(2, 1, 300.0);
        t.rows[0][0].paras = vec![apex_doc::Paragraph::new("Head")];
        t.rows[1][0].paras = (0..n).map(|i| apex_doc::Paragraph::new(format!("Cell line {i}"))).collect();
        t.props.row_heights = vec![0.0, height];
        t.props.row_rules = vec![Default::default(), rule];
        let mut ps: Vec<apex_doc::Paragraph> = (0..20).map(|i| apex_doc::Paragraph::new(format!("Line {i}"))).collect();
        ps.extend(t.into_paras(1));
        ps.push(apex_doc::Paragraph::new("After"));
        laid_out(Document::from_paragraphs(ps))
    }

    #[test]
    fn table_rows_break_across_pages() {
        let frags = |v: &DocView| v.layout.frags.iter().filter(|f| f.row == 1 && !f.ghost).map(|f| (f.page, f.h)).collect::<Vec<_>>();
        // A row taller than the room left starts on the page and goes on.
        let (v, _ctx) = tall_row(50, Default::default(), 0.0);
        let f = frags(&v);
        assert_eq!(f.iter().map(|x| x.0).collect::<Vec<_>>(), [0, 1], "{f:?}");
        // Kept on one page, it moves whole.
        let (v, _ctx) = tall_row(35, apex_doc::RowRule { cant_split: true, exact: false }, 0.0);
        let f = frags(&v);
        assert_eq!(f.iter().map(|x| x.0).collect::<Vec<_>>(), [1], "{f:?}");
        // An exact height cuts the text.
        let (v, _ctx) = tall_row(50, apex_doc::RowRule { cant_split: false, exact: true }, 40.0);
        let f = frags(&v);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!((f[0].1 - 40.0 * PT).abs() < 0.5, "{f:?}");
    }
}
