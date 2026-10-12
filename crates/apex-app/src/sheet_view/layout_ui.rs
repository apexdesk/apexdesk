//! Page Layout view (the cells on their printed pages, with margins and
//! headers and footers) and the sheet background.

use std::rc::Rc;
use std::sync::Arc;

use apex_sheet::{ImageFormat, MAX_COLS, MAX_ROWS, Sheet, SheetBackground, ViewMode};
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, pos2};

use super::{Geo, SheetView};
use crate::app::Notice;
use crate::i18n::tr;
use crate::images;
use crate::print::{self, HfBox, HfValues};
use crate::theme::Paper;

/// Space between pages, in sheet pixels.
pub(super) const GAP: f32 = 24.0;

/// The pages along one axis of the Page Layout view. Offsets are "layout
/// units": sheet pixels with the page margins and gaps added.
pub(super) struct PageAxis {
    /// Cells of each page (inclusive).
    spans: Vec<(u32, u32)>,
    /// Sheet offset (`col_x` / `row_y`) of each page's first cell.
    starts: Vec<f32>,
    /// Layout offset of each page's first cell.
    offs: Vec<f32>,
    /// Length of each page's cells.
    lens: Vec<f32>,
    /// Page length.
    page: f32,
    /// The cells run to the last index (the last page is not cut short).
    complete: bool,
    /// Only the cells of the pages show (a print area): the others go
    /// far off the pages.
    only: bool,
}

/// Where the cells off the pages of a print area go: nowhere in sight.
const OFF_PAGE: f32 = -1.0e7;

impl PageAxis {
    fn new(spans: Vec<(u32, u32)>, page: f32, lead: f32, room: f32, center: bool, last: u32, at: impl Fn(u32) -> f32) -> PageAxis {
        let complete = spans.last().is_some_and(|s| s.1 >= last);
        let (mut starts, mut offs, mut lens) = (Vec::new(), Vec::new(), Vec::new());
        for (k, &(a, b)) in spans.iter().enumerate() {
            let (s, len) = (at(a), at(b + 1) - at(a));
            let extra = if center { ((room - len) / 2.0).max(0.0) } else { 0.0 };
            starts.push(s);
            offs.push(GAP + k as f32 * (page + GAP) + lead + extra);
            lens.push(len);
        }
        PageAxis { spans, starts, offs, lens, page, complete, only: false }
    }

    /// The pages of the print areas only: `spans` in order, apart.
    fn areas(spans: Vec<(u32, u32)>, page: f32, lead: f32, room: f32, center: bool, at: impl Fn(u32) -> f32) -> PageAxis {
        let mut ax = PageAxis::new(spans, page, lead, room, center, 0, at);
        ax.complete = true;
        ax.only = true;
        ax
    }

    /// Whether cell `i` shows on a page.
    pub fn shows(&self, i: u32) -> bool {
        !self.only || self.spans.get(self.span_of(i)).is_some_and(|s| s.0 <= i && i <= s.1)
    }

    fn span_of(&self, i: u32) -> usize {
        self.spans.partition_point(|s| s.0 <= i).saturating_sub(1)
    }

    /// Layout offset of cell `i`; `at` gives sheet offsets.
    pub fn pos(&self, i: u32, at: impl Fn(u32) -> f32) -> f32 {
        if !self.shows(i) {
            return OFF_PAGE;
        }
        let k = self.span_of(i);
        self.offs[k] + at(i) - self.starts[k]
    }

    /// Layout offset of sheet offset `x` in cell `i` (floating objects).
    pub fn pos_of(&self, i: u32, x: f32, at: impl Fn(u32) -> f32) -> f32 {
        self.pos(i, &at) + x - at(i)
    }

    /// The page at layout offset `u`.
    pub fn page_at(&self, u: f32) -> usize {
        (((u - GAP) / (self.page + GAP)).floor().max(0.0) as usize).min(self.spans.len() - 1)
    }

    pub fn page_start(&self, k: usize) -> f32 {
        GAP + k as f32 * (self.page + GAP)
    }

    /// The cell at layout offset `u` (the nearest cell of its page in the
    /// margins); `at_inv` finds the cell at a sheet offset.
    pub fn index_at(&self, u: f32, at_inv: impl Fn(f32) -> u32) -> u32 {
        let k = self.page_at(u);
        let (a, b) = self.spans[k];
        let local = u - self.offs[k];
        if local <= 0.0 {
            return a;
        }
        at_inv(self.starts[k] + local).clamp(a, b)
    }

    /// Sheet offset shown at layout offset `u` (floating objects).
    pub fn sheet_at(&self, u: f32) -> f32 {
        let k = self.page_at(u);
        self.starts[k] + (u - self.offs[k]).clamp(0.0, self.lens[k])
    }

    /// Whole pages reach past layout offset `u`.
    fn covers(&self, u: f32) -> bool {
        self.complete || self.page_start(self.spans.len() - 1) > u
    }
}

/// Geometry of the Page Layout view.
pub(super) struct PageGeo {
    pub cols: PageAxis,
    pub rows: PageAxis,
    /// Sheet pixels per point of paper.
    pub unit: f32,
    /// Page rows and columns that print (without print areas, those from
    /// the top left page).
    pub printed: (usize, usize),
    /// With print areas: the number (from 0) of the printed page at each
    /// page row and column, if any; the others don't print and show empty.
    pub numbers: Option<Vec<Vec<Option<usize>>>>,
    /// Printed pages in all.
    pub total: usize,
    /// The print areas (their cells alone show).
    pub areas: Vec<apex_sheet::Range>,
}

impl PageGeo {
    fn build(sh: &Sheet, cols: u32, rows: u32) -> PageGeo {
        let p = &sh.page;
        if !p.print_areas.is_empty() {
            return PageGeo::of_areas(sh);
        }
        let scale = p.print_scale(sh);
        let unit = 1.0 / (apex_sheet::page::PT_PER_PX * scale);
        let (pw, ph) = p.paper();
        let (cw, ch) = p.content();
        let cols = PageAxis::new(p.tiles(sh, false, scale, cols), pw * unit, p.margin_left * unit, cw * unit, p.center_h, MAX_COLS - 1, |c| sh.col_x(c));
        let rows = PageAxis::new(p.tiles(sh, true, scale, rows), ph * unit, p.margin_top * unit, ch * unit, p.center_v, MAX_ROWS - 1, |r| sh.row_y(r));
        let printed = p.paginate(sh).map_or((0, 0), |pg| (pg.rows.len(), pg.cols.len()));
        PageGeo { cols, rows, unit, printed, numbers: None, total: printed.0 * printed.1, areas: Vec::new() }
    }

    /// The pages of the print areas alone, as printed: their page rows
    /// and columns in sheet order, each area on pages of its own.
    fn of_areas(sh: &Sheet) -> PageGeo {
        let p = &sh.page;
        let all = p.paginate_all(sh);
        let scale = all.first().map_or(1.0, |a| a.scale);
        let unit = 1.0 / (apex_sheet::page::PT_PER_PX * scale);
        let (pw, ph) = p.paper();
        let (cw, ch) = p.content();
        let cols = PageAxis::areas(merged(all.iter().flat_map(|a| a.cols.iter().copied())), pw * unit, p.margin_left * unit, cw * unit, p.center_h, |c| sh.col_x(c));
        let rows = PageAxis::areas(merged(all.iter().flat_map(|a| a.rows.iter().copied())), ph * unit, p.margin_top * unit, ch * unit, p.center_v, |r| sh.row_y(r));
        let mut numbers = vec![vec![None; cols.spans.len()]; rows.spans.len()];
        let mut total = 0;
        for pages in &all {
            for n in 0..pages.count() {
                if let Some((r, c)) = pages.page(n, p.across_first) {
                    let slot = &mut numbers[rows.span_of(r.0)][cols.span_of(c.0)];
                    slot.get_or_insert(total + n);
                }
            }
            total += pages.count();
        }
        PageGeo { cols, rows, unit, printed: (0, 0), numbers: Some(numbers), total, areas: all.iter().map(|a| a.area).collect() }
    }

    /// The printed page (from 0) at page row `ky` and column `kx`.
    pub fn number(&self, ky: usize, kx: usize, across_first: bool) -> Option<usize> {
        match &self.numbers {
            Some(n) => n.get(ky)?.get(kx).copied().flatten(),
            None if ky < self.printed.0 && kx < self.printed.1 => Some(if across_first { ky * self.printed.1 + kx } else { kx * self.printed.0 + ky }),
            None => None,
        }
    }
}

/// Page spans of several areas along an axis, in order: the same span
/// once, overlapping ones as one.
fn merged(spans: impl Iterator<Item = (u32, u32)>) -> Vec<(u32, u32)> {
    let mut v: Vec<(u32, u32)> = spans.collect();
    v.sort();
    let mut out: Vec<(u32, u32)> = Vec::new();
    for s in v {
        match out.last_mut() {
            Some(last) if s.0 <= last.1 => last.1 = last.1.max(s.1),
            _ => out.push(s),
        }
    }
    out
}

impl SheetView {
    /// Page Layout geometry covering layout offsets up to `(x, y)`, for the
    /// shown sheet (cached).
    pub(super) fn page_geo(&self, x: f32, y: f32) -> Rc<PageGeo> {
        let sh = self.sh();
        let ext = sh.extent().unwrap_or_default();
        let mut need = (ext.col.max(self.cursor.col).saturating_add(1).next_power_of_two().max(64), ext.row.max(self.cursor.row).saturating_add(1).next_power_of_two().max(128));
        let mut cache = self.print.layout.borrow_mut();
        if let Some(((s, rev, c, r), _)) = &*cache {
            if (*s, *rev) == (self.sheet, self.wb.revision) {
                // Keep a larger cache.
                need = (need.0.max(*c), need.1.max(*r));
            }
        }
        loop {
            let key = (self.sheet, self.wb.revision, need.0, need.1);
            let pg = match &*cache {
                Some((k, pg)) if *k == key => pg.clone(),
                _ => {
                    let pg = Rc::new(PageGeo::build(sh, need.0, need.1));
                    *cache = Some((key, pg.clone()));
                    pg
                }
            };
            let (cx, cy) = (pg.cols.covers(x), pg.rows.covers(y));
            if cx && cy {
                return pg;
            }
            if !cx {
                need.0 = need.0.saturating_mul(2).min(MAX_COLS);
            }
            if !cy {
                need.1 = need.1.saturating_mul(2).min(MAX_ROWS);
            }
        }
    }

    /// File name for the `&F` field of the Page Layout view's headers.
    pub fn set_file_dir(&mut self, dir: Option<&std::path::Path>) {
        if self.file_dir.as_deref() != dir {
            self.file_dir = dir.map(std::path::Path::to_path_buf);
        }
    }

    pub fn set_file_name(&mut self, name: String) {
        if self.print.file_name != name {
            self.print.file_name = name;
        }
    }

    pub(super) fn set_view_mode(&mut self, mode: ViewMode) {
        let old = self.sh().view;
        if old == mode {
            return;
        }
        self.wb.sheets[self.sheet].view = mode;
        self.wb.modified = true;
        if old == ViewMode::PageLayout || mode == ViewMode::PageLayout {
            // Scroll offsets mean something else in the other view.
            self.scroll = egui::Vec2::ZERO;
            self.ensure_visible = true;
        }
    }

    /// Paper, margins, gaps between pages and the headers and footers,
    /// over the painted cells.
    pub(super) fn paint_page_layout(&self, p: &egui::Painter, g: &Geo, dark: bool) {
        let Some(pl) = &g.pl else { return };
        let sh = self.sh();
        let p = p.with_clip_rect(g.view);
        let desk = if dark { Color32::from_gray(58) } else { Color32::from_gray(205) };
        let sx = |u: f32| g.origin.x + (u - g.scroll.x) * g.z;
        let sy = |u: f32| g.origin.y + (u - g.scroll.y) * g.z;
        let (u0, u1) = (g.scroll.x, g.scroll.x + g.view.width() / g.z);
        let (v0, v1) = (g.scroll.y, g.scroll.y + g.view.height() / g.z);
        let (kx0, kx1) = (pl.cols.page_at(u0), pl.cols.page_at(u1));
        let (ky0, ky1) = (pl.rows.page_at(v0), pl.rows.page_at(v1));
        // Gaps around the pages.
        for k in kx0..=kx1 + 1 {
            let e = pl.cols.page_start(k);
            p.rect_filled(Rect::from_min_max(pos2(sx(e - GAP), g.view.min.y), pos2(sx(e), g.view.max.y)), 0.0, desk);
        }
        for k in ky0..=ky1 + 1 {
            let e = pl.rows.page_start(k);
            p.rect_filled(Rect::from_min_max(pos2(g.view.min.x, sy(e - GAP)), pos2(g.view.max.x, sy(e))), 0.0, desk);
        }
        // The pages of print areas end: nothing past the last ones.
        if pl.numbers.is_some() {
            let (ex, ey) = (sx(pl.cols.page_start(pl.cols.spans.len()) - GAP), sy(pl.rows.page_start(pl.rows.spans.len()) - GAP));
            p.rect_filled(Rect::from_min_max(pos2(ex, g.view.min.y), g.view.max), 0.0, desk);
            p.rect_filled(Rect::from_min_max(pos2(g.view.min.x, ey), g.view.max), 0.0, desk);
        }
        let page = &sh.page;
        let values = HfValues::now(&self.print.file_name);
        let total = pl.total;
        let margin = Stroke::new(1.0, if dark { Color32::from_gray(200) } else { Color32::from_gray(215) });
        for ky in ky0..=ky1 {
            for kx in kx0..=kx1 {
                let (x0, y0) = (pl.cols.page_start(kx), pl.rows.page_start(ky));
                let rect = Rect::from_min_max(pos2(sx(x0), sy(y0)), pos2(sx(x0 + pl.cols.page), sy(y0 + pl.rows.page)));
                let cells = Rect::from_min_max(pos2(sx(pl.cols.offs[kx]), sy(pl.rows.offs[ky])), pos2(sx(pl.cols.offs[kx] + pl.cols.lens[kx]), sy(pl.rows.offs[ky] + pl.rows.lens[ky])));
                let number = pl.number(ky, kx, page.across_first);
                // With print areas, a page that doesn't print shows empty,
                // and so do the cells of a page out of every area.
                if pl.numbers.is_some() {
                    let areas: Vec<Rect> = if number.is_some() { pl.areas.iter().map(|a| g.range_rect(sh, a)).collect() } else { Vec::new() };
                    for r in super::print_ui::outside(cells, &areas) {
                        p.rect_filled(r, 0.0, Paper::BG);
                    }
                }
                // The paper outside the page's cells.
                for r in [
                    Rect::from_min_max(rect.min, pos2(rect.max.x, cells.min.y)),
                    Rect::from_min_max(pos2(rect.min.x, cells.max.y), rect.max),
                    Rect::from_min_max(pos2(rect.min.x, cells.min.y), pos2(cells.min.x, cells.max.y)),
                    Rect::from_min_max(pos2(cells.max.x, cells.min.y), pos2(rect.max.x, cells.max.y)),
                ] {
                    if r.is_positive() {
                        p.rect_filled(r, 0.0, Paper::BG);
                    }
                }
                let k = g.z * pl.unit;
                let content = Rect::from_min_max(rect.min + egui::vec2(page.margin_left, page.margin_top) * k, rect.max - egui::vec2(page.margin_right, page.margin_bottom) * k);
                p.rect_stroke(content, 0.0, margin, egui::StrokeKind::Outside);
                p.rect_stroke(rect, 0.0, Stroke::new(1.0, Color32::from_gray(150)), egui::StrokeKind::Outside);
                if let Some(n) = number {
                    let first = page.first_page.map_or(1, |f| f as usize);
                    let cx = values.context(n + first, total, &sh.name);
                    let pt = crate::print::PT;
                    let b = HfBox { header: page.margin_header * k, footer: page.margin_footer * k, left: page.margin_left * k, right: page.margin_right * k };
                    print::paint_hf(&p, &page.hf, n, &cx, rect, b, k / pt, Paper::TEXT);
                }
            }
        }
    }

    // --- sheet background --------------------------------------------------------------

    /// Page Layout > Background: asks for a picture to tile behind the
    /// cells.
    pub(super) fn background_dialog(&mut self) {
        crate::file_dialog::FileDialog::new().set_title(tr("page.background")).add_filter(tr("image.filter"), &super::images_ui::EXTENSIONS).pick_file(crate::file_dialog::in_view(|v: &mut SheetView, path: std::path::PathBuf| match std::fs::read(&path) {
            Ok(b) => v.set_background_bytes(b.into()),
            Err(e) => v.notices.push(Notice::error(crate::i18n::trf("error.open", &[("error", &e.to_string())]))),
        }));
    }

    pub(super) fn set_background_bytes(&mut self, data: Arc<[u8]>) {
        self.settle_edit();
        match ImageFormat::sniff(&data).filter(|_| images::decode(&data).is_some()) {
            Some(format) => self.wb.set_background(self.sheet, Some(SheetBackground { data, format })),
            None => self.notices.push(Notice::error(tr("image.bad"))),
        }
    }

    pub(super) fn delete_background(&mut self) {
        self.wb.set_background(self.sheet, None);
    }

    /// The sheet background, tiled from the top-left corner of the sheet in
    /// each pane (not in the Page Layout view, which shows the printed
    /// look).
    pub(super) fn paint_background(&self, p: &egui::Painter, g: &Geo) {
        let sh = self.sh();
        let Some(bg) = &sh.page.background else { return };
        if g.pl.is_some() {
            return;
        }
        let Some((w, h)) = apex_sheet::image::natural_size(&bg.data).filter(|(w, h)| *w >= 1.0 && *h >= 1.0) else { return };
        let Some(tex) = self.background_texture(p.ctx(), &bg.data) else { return };
        let fxs = g.origin.x + g.fx * g.z;
        let fys = g.origin.y + g.fy * g.z;
        for (frozen, clip) in [
            ((true, true), Rect::from_min_max(g.origin, pos2(fxs, fys))),
            ((false, true), Rect::from_min_max(pos2(fxs, g.origin.y), pos2(g.view.max.x, fys))),
            ((true, false), Rect::from_min_max(pos2(g.origin.x, fys), pos2(fxs, g.view.max.y))),
            ((false, false), Rect::from_min_max(pos2(fxs, fys), g.view.max)),
        ] {
            let clip = clip.intersect(g.view);
            if !clip.is_positive() {
                continue;
            }
            let (ox, oy) = (if frozen.0 { g.ox } else { g.scroll.x }, if frozen.1 { g.oy } else { g.scroll.y });
            // Sheet offsets of the pane's corners, in tiles.
            let uv = |q: Pos2| pos2(((q.x - g.origin.x) / g.z + ox) / w, ((q.y - g.origin.y) / g.z + oy) / h);
            p.image(tex.id(), clip, Rect::from_min_max(uv(clip.min), uv(clip.max)), Color32::WHITE);
        }
    }

    /// A repeating texture of the background picture (cached).
    fn background_texture(&self, ctx: &egui::Context, data: &Arc<[u8]>) -> Option<egui::TextureHandle> {
        let key = images::key(data);
        let mut c = self.print.background.borrow_mut();
        if let Some((k, t)) = &*c {
            if *k == key {
                return t.clone();
            }
        }
        let t = images::tiled_texture(ctx, key, data);
        *c = Some((key, t.clone()));
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_sheet::{CellRef, Workbook};

    fn png() -> Arc<[u8]> {
        let mut out = Vec::new();
        {
            let mut e = png::Encoder::new(&mut out, 2, 2);
            e.set_color(png::ColorType::Rgb);
            e.set_depth(png::BitDepth::Eight);
            e.write_header().unwrap().write_image_data(&[200; 12]).unwrap();
        }
        out.into()
    }

    #[test]
    fn page_axis() {
        let mut sh = Sheet::new("S");
        sh.col_widths.insert(3, 150.0);
        let page = sh.page.clone();
        let spans = page.tiles(&sh, false, 1.0, 40);
        let unit = 1.0 / apex_sheet::page::PT_PER_PX;
        let (pw, _) = page.paper();
        let ax = PageAxis::new(spans.clone(), pw * unit, page.margin_left * unit, page.content().0 * unit, false, MAX_COLS - 1, |c| sh.col_x(c));
        let at = |c: u32| sh.col_x(c);
        // Each page starts after its gap and left margin; cells run on
        // within a page.
        assert!((ax.pos(0, at) - (GAP + page.margin_left * unit)).abs() < 0.01);
        let (a, b) = spans[1];
        assert!((ax.pos(a, at) - (ax.page_start(1) + page.margin_left * unit)).abs() < 0.01);
        assert!((ax.pos(b, at) - ax.pos(a, at) - (sh.col_x(b) - sh.col_x(a))).abs() < 0.01);
        // Back from layout offsets to cells, margins going to the edge cells.
        for c in 0..30 {
            assert_eq!(ax.index_at(ax.pos(c, at) + 1.0, |x| sh.col_at(x)), c);
        }
        assert_eq!(ax.index_at(ax.page_start(1) + 1.0, |x| sh.col_at(x)), a);
        assert_eq!(ax.index_at(ax.page_start(1) - 2.0, |x| sh.col_at(x)), spans[0].1);
        // Floating objects: a sheet point and back.
        let x = sh.col_x(a) + 10.0;
        assert!((ax.sheet_at(ax.pos_of(a, x, at)) - x).abs() < 0.01);
    }

    /// With print areas the Page Layout view shows their pages alone, each
    /// area on pages of its own numbered as printed; the Page Break
    /// Preview grays the rest; printing gives each area its own pages.
    #[test]
    fn print_areas_on_the_pages() {
        let mut v = SheetView::new(Workbook::new());
        for r in 0..60 {
            v.wb.set_input(0, CellRef::new(r, 12), "x");
        }
        let areas = [apex_sheet::Range::parse("B2:D10").unwrap(), apex_sheet::Range::parse("F30:G40").unwrap()];
        let mut page = v.sh().page.clone();
        page.print_areas = areas.to_vec();
        v.wb.set_page_setup(0, page);
        v.set_view_mode(ViewMode::PageLayout);
        let pg = v.page_geo(10_000.0, 10_000.0);
        assert_eq!((pg.cols.spans.clone(), pg.rows.spans.clone()), (vec![(1, 3), (5, 6)], vec![(1, 9), (29, 39)]));
        assert_eq!((pg.number(0, 0, false), pg.number(1, 1, false), pg.number(0, 1, false), pg.number(1, 0, false), pg.total), (Some(0), Some(1), None, None, 2));
        // The cells out of the areas are nowhere on the pages.
        assert!(!pg.cols.shows(0) && !pg.cols.shows(4) && pg.cols.shows(2) && !pg.cols.shows(12));
        assert!(!pg.rows.shows(0) && !pg.rows.shows(20) && pg.rows.shows(35) && !pg.rows.shows(45));
        let at = |c: u32| v.sh().col_x(c);
        assert!(pg.cols.pos(0, at) < -1000.0 && pg.cols.pos(12, at) < -1000.0);
        // Hit testing only finds cells of the areas.
        for u in (0..4000).step_by(50) {
            let c = pg.cols.index_at(u as f32, |x| v.sh().col_at(x));
            assert!(pg.cols.shows(c), "{u}: {c}");
        }
        // Printing: two pages, one per area; the preview has both.
        let plan = super::super::print_ui::PrintPlan::new(&v.wb, &[0], None, "f");
        assert_eq!(plan.count(), 2);
        let pages = v.sh().page.paginate_all(v.sh());
        assert_eq!(pages.iter().map(|p| p.area).collect::<Vec<_>>(), areas);
        // Without print areas, pages tile the whole sheet again.
        let mut page = v.sh().page.clone();
        page.print_areas.clear();
        v.wb.set_page_setup(0, page);
        let pg = v.page_geo(1000.0, 1000.0);
        assert!(pg.numbers.is_none() && pg.cols.shows(0) && pg.cols.spans[0].0 == 0);
    }

    /// The gray of the Page Break Preview covers all but the areas.
    #[test]
    fn outside_the_areas() {
        let view = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 100.0));
        let areas = [Rect::from_min_max(pos2(10.0, 10.0), pos2(30.0, 30.0)), Rect::from_min_max(pos2(50.0, 20.0), pos2(90.0, 60.0))];
        let out = super::super::print_ui::outside(view, &areas);
        let area: f32 = out.iter().map(|r| r.area()).sum();
        assert!((area - (10_000.0 - 400.0 - 1600.0)).abs() < 0.01, "{area}");
        for r in &out {
            for a in &areas {
                assert!(!r.intersect(*a).is_positive(), "{r:?} {a:?}");
            }
        }
    }

    #[test]
    fn view_and_background() {
        let mut v = SheetView::new(Workbook::new());
        v.wb.set_input(0, CellRef::new(100, 20), "x");
        v.set_view_mode(ViewMode::PageLayout);
        assert_eq!(v.sh().view, ViewMode::PageLayout);
        let pg = v.page_geo(10_000.0, 30_000.0);
        assert!(pg.cols.covers(10_000.0) && pg.rows.covers(30_000.0));
        let pages = v.sh().page.paginate(v.sh()).unwrap();
        assert_eq!(pg.printed, (pages.rows.len(), pages.cols.len()));
        assert!(pg.printed.0 > 1 && pg.printed.1 > 1);
        // Undoable, unlike the view.
        v.set_background_bytes(png());
        assert!(v.sh().page.background.is_some());
        v.set_background_bytes(Arc::from(&b"not a picture"[..]));
        assert_eq!(v.notices.len(), 1);
        v.delete_background();
        assert!(v.sh().page.background.is_none());
        v.undo();
        assert!(v.sh().page.background.is_some());
        assert_eq!(v.sh().view, ViewMode::PageLayout);
    }
}
