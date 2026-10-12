//! Printed pages of sheets, page setup and the page break preview.

use apex_sheet::addr::col_name;
use apex_sheet::{CellRef, Pages, Range, SheetPage};
use eframe::egui::{self, Color32, FontId, Pos2, Rect, RichText, Stroke, pos2, vec2};

use super::{AxisCache, Geo, HEADER_H, SheetView};
use crate::i18n::{tr, trf};
use crate::print::{self, HfBox, HfValues, Job, LinkTarget, Page, PT, Scope, hf_ui, margins_ui, paper_ui};
use crate::icons::Icon;
use crate::theme::Paper;
use crate::widgets::{self, DialogResult};

/// Sheet pixels per printed pixel at 100 % (sheet sizes are 96 dpi, like
/// printed pages).
const SHEET_PX: f32 = PT * 0.75;

pub struct SetupDialog {
    sheet: usize,
    page: SheetPage,
    tab: usize,
    which: usize,
    fit: bool,
    area: String,
    rows: String,
    cols: String,
    error: Option<String>,
}

#[derive(Default)]
pub struct PrintState {
    pub setup: Option<SetupDialog>,
    /// Pages of each printed area of the shown sheet for the preview,
    /// keyed by sheet and revision.
    pages: std::cell::RefCell<Option<((usize, u64), Vec<Pages>)>>,
    /// Page Layout view geometry, keyed by sheet, revision and the cells
    /// covered.
    pub layout: std::cell::RefCell<Option<((usize, u64, u32, u32), std::rc::Rc<super::layout_ui::PageGeo>)>>,
    /// Repeating texture of the sheet background, keyed by picture.
    pub background: std::cell::RefCell<Option<(u64, Option<egui::TextureHandle>)>>,
    /// File name for the headers and footers of the Page Layout view.
    pub file_name: String,
}

/// The printed pages of some sheets, before painting: how each sheet is
/// split into pages. Planning reads only the row and column sizes, so it
/// is quick for any sheet; the pages are painted one at a time.
pub struct PrintPlan {
    /// Sheet, its page setup, the pages of one of its print areas and the
    /// first of them in the job.
    sheets: Vec<(usize, SheetPage, Pages, usize)>,
    /// Name and first page (in the job) of each sheet.
    first_pages: Vec<(String, usize)>,
    values: HfValues,
    total: usize,
}

impl PrintPlan {
    /// Plans `sheets` of `wb`; `sel` prints only that range of each.
    pub fn new(wb: &apex_sheet::Workbook, sheets: &[usize], sel: Option<Range>, file: &str) -> PrintPlan {
        let mut plan = PrintPlan { sheets: Vec::new(), first_pages: Vec::new(), values: HfValues::now(file), total: 0 };
        for &i in sheets {
            let sh = &wb.sheets[i];
            let mut page = sh.page.clone();
            if let Some(sel) = sel {
                page.print_areas = vec![sel];
            }
            let all = page.paginate_all(sh);
            if !all.is_empty() {
                plan.first_pages.push((sh.name.clone(), plan.total));
            }
            // Each print area starts on a page of its own.
            for pages in all {
                let n = pages.count();
                plan.sheets.push((i, page.clone(), pages, plan.total));
                plan.total += n;
            }
        }
        plan
    }

    pub fn count(&self) -> usize {
        self.total
    }
}

/// The gray over what doesn't print in the Page Break Preview.
pub(super) const OUTSIDE: Color32 = Color32::from_black_alpha(40);

/// The parts of `view` outside every one of `rects`, as rectangles (in
/// horizontal bands).
pub(super) fn outside(view: Rect, rects: &[Rect]) -> Vec<Rect> {
    let rects: Vec<Rect> = rects.iter().map(|r| r.intersect(view)).filter(|r| r.is_positive()).collect();
    let mut ys: Vec<f32> = rects.iter().flat_map(|r| [r.min.y, r.max.y]).chain([view.min.y, view.max.y]).collect();
    ys.sort_by(f32::total_cmp);
    ys.dedup();
    let mut out = Vec::new();
    for w in ys.windows(2) {
        let (y0, y1) = (w[0], w[1]);
        let mut xs: Vec<(f32, f32)> = rects.iter().filter(|r| r.min.y <= y0 && r.max.y >= y1).map(|r| (r.min.x, r.max.x)).collect();
        xs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut x = view.min.x;
        for (a, b) in xs {
            if a > x {
                out.push(Rect::from_min_max(pos2(x, y0), pos2(a, y1)));
            }
            x = x.max(b);
        }
        if x < view.max.x {
            out.push(Rect::from_min_max(pos2(x, y0), pos2(view.max.x, y1)));
        }
    }
    out.retain(|r| r.is_positive());
    out
}

/// "$1:$3" / "1:3" -> rows 0..=2; "$A:$B" -> columns.
fn parse_span(s: &str, rows: bool) -> Option<Option<(u32, u32)>> {
    let s = s.trim().replace('$', "");
    if s.is_empty() {
        return Some(None);
    }
    let (a, b) = s.split_once(':').unwrap_or((&s, &s));
    let one = |t: &str| -> Option<u32> {
        let t = t.trim();
        if rows {
            t.parse::<u32>().ok().filter(|&n| n > 0).map(|n| n - 1)
        } else {
            apex_sheet::addr::parse_col(t)
        }
    };
    let (a, b) = (one(a)?, one(b)?);
    Some(Some((a.min(b), a.max(b))))
}

fn span_text(s: Option<(u32, u32)>, rows: bool) -> String {
    match s {
        None => String::new(),
        Some((a, b)) if rows => format!("${}:${}", a + 1, b + 1),
        Some((a, b)) => format!("${}:${}", col_name(a), col_name(b)),
    }
}

impl SheetView {
    /// Pages of each printed area of the shown sheet (cached for the page
    /// break preview).
    fn preview_pages(&self) -> Vec<Pages> {
        let key = (self.sheet, self.wb.revision);
        let mut c = self.print.pages.borrow_mut();
        if c.as_ref().is_none_or(|(k, _)| *k != key) {
            let sh = self.sh();
            *c = Some((key, sh.page.paginate_all(sh)));
        }
        c.as_ref().map(|(_, p)| p.clone()).unwrap_or_default()
    }

    /// Blue page borders, "Page N" labels and a gray area outside the
    /// printed areas.
    pub(super) fn paint_page_breaks(&self, p: &egui::Painter, g: &Geo) {
        if self.sh().view != apex_sheet::ViewMode::PageBreakPreview {
            return;
        }
        let all = self.preview_pages();
        if all.is_empty() {
            return;
        }
        let sh = self.sh();
        let p = p.with_clip_rect(g.view);
        let rects: Vec<Rect> = all.iter().map(|pages| g.range_rect(sh, &pages.area)).collect();
        // Outside the printed areas.
        for r in outside(g.view, &rects) {
            p.rect_filled(r, 0.0, OUTSIDE);
        }
        let blue = Color32::from_rgb(0x1F, 0x4E, 0xD8);
        let solid = Stroke::new(2.5, blue);
        let dashed = Stroke::new(1.5, blue);
        let page = sh.page.clone();
        let font = FontId::proportional((36.0 * g.z).clamp(14.0, 72.0));
        let mut first = 0;
        for (pages, &area) in all.iter().zip(&rects) {
            // Only the visible part of each line: dashing a line down a
            // huge area made millions of shapes.
            let seen = area.intersect(g.view.expand(10.0));
            if !seen.is_positive() {
                first += pages.count();
                continue;
            }
            for &(a, _) in pages.cols.iter().skip(1) {
                let x = g.x(sh, a);
                if !seen.x_range().contains(x) {
                    continue;
                }
                if page.col_breaks.contains(&a) {
                    p.vline(x, seen.y_range(), solid);
                } else {
                    p.extend(egui::Shape::dashed_line(&[pos2(x, seen.min.y), pos2(x, seen.max.y)], dashed, 6.0, 4.0));
                }
            }
            for &(a, _) in pages.rows.iter().skip(1) {
                let y = g.y(sh, a);
                if !seen.y_range().contains(y) {
                    continue;
                }
                if page.row_breaks.contains(&a) {
                    p.hline(seen.x_range(), y, solid);
                } else {
                    p.extend(egui::Shape::dashed_line(&[pos2(seen.min.x, y), pos2(seen.max.x, y)], dashed, 6.0, 4.0));
                }
            }
            p.rect_stroke(area, 0.0, solid, egui::StrokeKind::Middle);
            // Page numbers, in print order (the areas one after another).
            for n in 0..pages.count().min(400) {
                let Some(((r0, r1), (c0, c1))) = pages.page(n, page.across_first) else { break };
                let r = g.range_rect(sh, &Range::new(CellRef::new(r0, c0), CellRef::new(r1, c1)));
                if r.intersects(g.view) {
                    p.text(r.center(), egui::Align2::CENTER_CENTER, trf("print.page_n", &[("n", &(first + n + 1))]), font.clone(), Color32::from_rgba_unmultiplied(120, 120, 140, 110));
                }
            }
            first += pages.count();
        }
    }

    // --- commands ----------------------------------------------------------------------

    fn edit_page(&mut self, f: impl FnOnce(&mut SheetPage, CellRef, Range)) {
        let (c, sel) = (self.cursor, self.sel());
        let mut page = self.sh().page.clone();
        f(&mut page, c, sel);
        self.wb.set_page_setup(self.sheet, page);
    }

    /// Breaks above the cursor row and left of its column (only one of them
    /// in the first row or column, or with whole rows or columns selected).
    pub(super) fn insert_page_break(&mut self) {
        let (rows, cols) = (self.whole_rows(), self.whole_cols());
        self.edit_page(|p, c, _| {
            if c.row > 0 && !cols {
                p.row_breaks.insert(c.row);
            }
            if c.col > 0 && !rows {
                p.col_breaks.insert(c.col);
            }
        });
    }

    pub(super) fn remove_page_break(&mut self) {
        self.edit_page(|p, c, _| {
            p.row_breaks.remove(&c.row);
            p.col_breaks.remove(&c.col);
        });
    }

    pub(super) fn reset_page_breaks(&mut self) {
        self.edit_page(|p, _, _| {
            p.row_breaks.clear();
            p.col_breaks.clear();
        });
    }

    pub(super) fn set_print_area(&mut self, on: bool) {
        let areas = if on { self.areas() } else { Vec::new() };
        self.edit_page(|p, _, _| p.print_areas = areas);
    }

    pub(super) fn orientation_items(&mut self, ui: &mut egui::Ui) {
        let landscape = self.sh().page.landscape;
        for (l, key) in [(false, "page.portrait"), (true, "page.landscape")] {
            if ui.radio(landscape == l, tr(key)).clicked() {
                self.edit_page(|p, _, _| p.landscape = l);
                ui.close();
            }
        }
    }

    pub(super) fn paper_items(&mut self, ui: &mut egui::Ui) {
        let page = self.sh().page.clone();
        for (n, w, h) in apex_doc::PAPER_SIZES {
            let on = (page.paper_width - w).abs() < 2.0 && (page.paper_height - h).abs() < 2.0;
            if ui.radio(on, n).clicked() {
                self.edit_page(|p, _, _| (p.paper_width, p.paper_height) = (w, h));
                ui.close();
            }
        }
    }

    pub(super) fn print_area_items(&mut self, ui: &mut egui::Ui) {
        if ui.button(tr("page.set_print_area")).clicked() {
            self.set_print_area(true);
            ui.close();
        }
        if ui.add_enabled(!self.sh().page.print_areas.is_empty(), egui::Button::new(tr("page.clear_print_area"))).clicked() {
            self.set_print_area(false);
            ui.close();
        }
    }

    pub(super) fn breaks_items(&mut self, ui: &mut egui::Ui) {
        if ui.button(tr("page.insert_break")).clicked() {
            self.insert_page_break();
            ui.close();
        }
        if ui.button(tr("page.remove_break")).clicked() {
            self.remove_page_break();
            ui.close();
        }
        if ui.button(tr("page.reset_breaks")).clicked() {
            self.reset_page_breaks();
            ui.close();
        }
    }

    /// Print gridlines and headings.
    pub(super) fn sheet_print_options(&mut self, ui: &mut egui::Ui) {
        let page = &self.sh().page;
        let (grid, heads) = (page.gridlines, page.headings);
        if crate::toolstrip::check(ui, grid, crate::icons::Icon::Gridlines, tr("page.print_gridlines")) {
            self.edit_page(|p, _, _| p.gridlines = !grid);
        }
        if crate::toolstrip::check(ui, heads, crate::icons::Icon::InsertCols, tr("page.print_headings")) {
            self.edit_page(|p, _, _| p.headings = !heads);
        }
    }

    // --- page setup dialog -------------------------------------------------------------

    pub fn open_page_setup(&mut self, tab: usize) {
        let page = self.sh().page.clone();
        self.print.setup = Some(SetupDialog {
            sheet: self.sheet,
            fit: page.fit.is_some(),
            area: page.print_areas.iter().map(|a| a.to_a1()).collect::<Vec<_>>().join(","),
            rows: span_text(page.title_rows, true),
            cols: span_text(page.title_cols, false),
            page,
            tab,
            which: 0,
            error: None,
        });
    }

    pub(super) fn page_setup_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.print.setup else { return };
        let pages = [(Icon::PageSize, "page.tab_page"), (Icon::Margins, "page.tab_margins"), (Icon::HeaderFooter, "page.tab_hf"), (Icon::Gridlines, "page.tab_sheet")];
        let mut tab = d.tab;
        let res = widgets::nav_dialog(ctx, "sheet-page-setup", tr("page.setup"), &pages, &mut tab, egui::vec2(500.0, 420.0), |ui, page| {
            let p = &mut d.page;
            match page {
                0 => {
                    paper_ui(ui, &mut p.paper_width, &mut p.paper_height, false);
                    print::orientation_ui(ui, &mut p.landscape);
                    widgets::section(ui, tr("page.scaling"));
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut d.fit, false, tr("page.adjust_to"));
                        ui.add_enabled(!d.fit, egui::DragValue::new(&mut p.scale).range(10..=400).suffix(" %"));
                    });
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut d.fit, true, tr("page.fit_to"));
                        let (mut w, mut h) = p.fit.unwrap_or((1, 0));
                        ui.add_enabled_ui(d.fit, |ui| {
                            ui.add(egui::DragValue::new(&mut w).range(0..=999));
                            ui.label(tr("page.pages_wide"));
                            ui.add(egui::DragValue::new(&mut h).range(0..=999));
                            ui.label(tr("page.pages_tall"));
                        });
                        p.fit = d.fit.then_some((w, h));
                    });
                    ui.label(RichText::new(tr("page.fit_hint")).small().weak());
                    widgets::section(ui, tr("page.numbering"));
                    ui.horizontal(|ui| {
                        ui.label(tr("page.first_number"));
                        let mut auto = p.first_page.is_none();
                        ui.checkbox(&mut auto, tr("page.auto"));
                        let mut n = p.first_page.unwrap_or(1);
                        ui.add_enabled(!auto, egui::DragValue::new(&mut n).range(1..=9999));
                        p.first_page = (!auto).then_some(n);
                    });
                }
                1 => {
                    let paper = if p.landscape { (p.paper_height, p.paper_width) } else { (p.paper_width, p.paper_height) };
                    margins_ui(ui, [&mut p.margin_top, &mut p.margin_bottom, &mut p.margin_left, &mut p.margin_right, &mut p.margin_header, &mut p.margin_footer], paper);
                    widgets::section(ui, tr("page.center_on_page"));
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut p.center_h, tr("page.horizontally"));
                        ui.checkbox(&mut p.center_v, tr("page.vertically"));
                    });
                }
                2 => hf_ui(ui, &mut p.hf, &mut d.which, true),
                _ => {
                    widgets::section(ui, tr("page.print_titles"));
                    widgets::form(ui, "sheet-print", |ui| {
                        widgets::form_row(ui, tr("page.print_area"), |ui| ui.add(egui::TextEdit::singleline(&mut d.area).desired_width(180.0).hint_text("A1:H40")));
                        widgets::form_row(ui, tr("page.rows_repeat"), |ui| ui.add(egui::TextEdit::singleline(&mut d.rows).desired_width(180.0).hint_text("$1:$1")));
                        widgets::form_row(ui, tr("page.cols_repeat"), |ui| ui.add(egui::TextEdit::singleline(&mut d.cols).desired_width(180.0).hint_text("$A:$A")));
                    });
                    widgets::section(ui, tr("page.print"));
                    ui.checkbox(&mut p.gridlines, tr("page.print_gridlines"));
                    ui.checkbox(&mut p.headings, tr("page.print_headings"));
                    widgets::section(ui, tr("page.order"));
                    ui.radio_value(&mut p.across_first, false, tr("page.down_then_over"));
                    ui.radio_value(&mut p.across_first, true, tr("page.over_then_down"));
                }
            }
            if let Some(e) = &d.error {
                ui.add_space(8.0);
                ui.label(RichText::new(e).color(ui.visuals().error_fg_color));
            }
        });
        d.tab = tab;
        let (ok, close) = (res == DialogResult::Ok, res == DialogResult::Cancel);
        if ok {
            // Several areas go separated by commas (or semicolons).
            let area = d.area.replace('$', "");
            let area: Option<Vec<Range>> = area.split([',', ';']).map(str::trim).filter(|a| !a.is_empty()).map(Range::parse).collect();
            match (area, parse_span(&d.rows, true), parse_span(&d.cols, false)) {
                (Some(a), Some(r), Some(c)) => {
                    let mut page = d.page.clone();
                    page.print_areas = a;
                    page.title_rows = r;
                    page.title_cols = c;
                    let sheet = d.sheet.min(self.wb.sheets.len() - 1);
                    self.print.setup = None;
                    self.wb.set_page_setup(sheet, page);
                    return;
                }
                _ => d.error = Some(tr("page.bad_ref").to_string()),
            }
        }
        if close {
            self.print.setup = None;
        }
    }

    // --- printed pages -----------------------------------------------------------------

    /// The pages of the sheets in `scope`, planned: each one is painted
    /// when it is needed ([`SheetView::paint_planned`]).
    pub fn print_job(&mut self, file: &str, scope: Scope) -> Job {
        // Planning > Print Gantt asked for this one.
        if let Some(g) = self.plan.print_next.take()
            && scope != Scope::Workbook
            && let Some(job) = self.gantt_print_job(&g)
        {
            return job;
        }
        let sel = (scope == Scope::Selection).then(|| self.sel());
        let sheets: Vec<usize> = match scope {
            Scope::Workbook => (0..self.wb.sheets.len()).filter(|&i| !self.wb.sheets[i].hidden).collect(),
            _ => vec![self.sheet],
        };
        let plan = PrintPlan::new(&self.wb, &sheets, sel, file);
        let outline = plan.first_pages.iter().map(|(name, first)| (name.clone(), 0, *first, 0.0)).collect();
        Job { title: file.to_string(), outline, plan: Some(plan), ..Default::default() }
    }

    /// Page `n` of a job planned by [`SheetView::print_job`].
    pub fn paint_planned(&mut self, ctx: &egui::Context, plan: &PrintPlan, n: usize) -> Option<Page> {
        let s = plan.sheets.partition_point(|x| x.3 <= n).checked_sub(1)?;
        let (i, page, pages, first) = plan.sheets.get(s)?;
        let (rows, cols) = pages.page(n - first, page.across_first)?;
        if *i >= self.wb.sheets.len() {
            return None;
        }
        let shown = std::mem::replace(&mut self.sheet, *i);
        let number = n + page.first_page.map_or(1, |f| f as usize);
        let p = self.print_page(ctx, page, pages.scale, rows, cols, &plan.values, number, plan.total, &plan.first_pages);
        self.sheet = shown;
        Some(p)
    }
    #[allow(clippy::too_many_arguments)]
    fn print_page(&self, ctx: &egui::Context, page: &SheetPage, scale: f32, rows: (u32, u32), cols: (u32, u32), values: &HfValues, number: usize, total: usize, sheets: &[(String, usize)]) -> Page {
        let sh = self.sh();
        let (pw, ph) = page.paper();
        let size = vec2(pw * PT, ph * PT);
        let content = Rect::from_min_max(pos2(page.margin_left * PT, page.margin_top * PT), pos2(size.x - page.margin_right * PT, size.y - page.margin_bottom * PT));
        let z = scale * SHEET_PX;
        // Column and row blocks: repeated titles (unless this page already
        // shows them), then the page's own cells.
        let title_cols = page.title_cols.filter(|t| cols.0 > t.1);
        let title_rows = page.title_rows.filter(|t| rows.0 > t.1);
        let col_blocks: Vec<(u32, u32)> = title_cols.into_iter().chain([cols]).collect();
        let row_blocks: Vec<(u32, u32)> = title_rows.into_iter().chain([rows]).collect();
        let span_w = |(a, b): (u32, u32)| (sh.col_x(b + 1) - sh.col_x(a)) * z;
        let span_h = |(a, b): (u32, u32)| (sh.row_y(b + 1) - sh.row_y(a)) * z;
        let hdr_w = if page.headings { ((rows.1 + 1).to_string().len() as f32 * 8.0 + 16.0) * z } else { 0.0 };
        let hdr_h = if page.headings { HEADER_H * z } else { 0.0 };
        let total_w = hdr_w + col_blocks.iter().map(|&b| span_w(b)).sum::<f32>();
        let total_h = hdr_h + row_blocks.iter().map(|&b| span_h(b)).sum::<f32>();
        let mut origin = content.min;
        if page.center_h {
            origin.x += ((content.width() - total_w) / 2.0).max(0.0);
        }
        if page.center_v {
            origin.y += ((content.height() - total_h) / 2.0).max(0.0);
        }
        let mut links = Vec::new();
        let shapes = print::capture(ctx, |p| {
            let mut y = origin.y + hdr_h;
            for (ri, &rb) in row_blocks.iter().enumerate() {
                let mut x = origin.x + hdr_w;
                let h = span_h(rb);
                for (ci, &cb) in col_blocks.iter().enumerate() {
                    let w = span_w(cb);
                    let pane = Rect::from_min_size(pos2(x, y), vec2(w, h));
                    self.print_pane(p, pane, rb, cb, z, page.gridlines, &mut links, sheets);
                    if page.headings {
                        self.print_headings(p, pane, rb, cb, z, (ri == 0).then_some(hdr_h), (ci == 0).then_some(hdr_w));
                    }
                    x += w;
                }
                y += h;
            }
            if page.headings {
                let corner = Rect::from_min_size(origin, vec2(hdr_w, hdr_h));
                p.rect_filled(corner, 0.0, Color32::from_gray(235));
                p.rect_stroke(corner, 0.0, Stroke::new(0.75, Color32::from_gray(150)), egui::StrokeKind::Inside);
            }
            let cx = values.context(number, total, &sh.name);
            let page_rect = Rect::from_min_size(Pos2::ZERO, size);
            let b = HfBox { header: page.margin_header * PT, footer: page.margin_footer * PT, left: page.margin_left * PT, right: page.margin_right * PT };
            print::paint_hf(p, &page.hf, number - page.first_page.map_or(1, |f| f as usize), &cx, page_rect, b, 1.0, Paper::TEXT);
        });
        Page { size, shapes, links, tags: Vec::new() }
    }

    #[allow(clippy::too_many_arguments)]
    fn print_pane(&self, p: &egui::Painter, pane: Rect, rows: (u32, u32), cols: (u32, u32), z: f32, grid: bool, links: &mut Vec<(Rect, LinkTarget)>, sheets: &[(String, usize)]) {
        let sh = self.sh();
        let g = Geo {
            origin: pane.min,
            view: pane,
            z,
            fr: 0,
            fc: 0,
            fx: 0.0,
            fy: 0.0,
            top: CellRef::new(0, 0),
            ox: 0.0,
            oy: 0.0,
            scroll: vec2(sh.col_x(cols.0), sh.row_y(rows.0)),
            pl: None,
            xs: AxisCache::build(0, cols.0..=cols.1, |c| sh.col_x(c), |c| sh.col_width(c)),
            ys: AxisCache::build(0, rows.0..=rows.1, |r| sh.row_y(r), |r| sh.row_height(r)),
        };
        let rs: Vec<u32> = (rows.0..=rows.1).collect();
        let cs: Vec<u32> = (cols.0..=cols.1).collect();
        let clip = pane.expand(1.0);
        self.paint_pane(&p.with_clip_rect(clip), &g, &rs, &cs, clip, grid, true);
        self.paint_images_in(&p.with_clip_rect(pane), &g, (false, false), pane, true);
        if grid {
            p.with_clip_rect(clip).rect_stroke(pane, 0.0, Stroke::new(1.0, Paper::GRID), egui::StrokeKind::Inside);
        }
        let area = Range::new(CellRef::new(rows.0, cols.0), CellRef::new(rows.1, cols.1));
        sh.for_each_in(&area, |c, cell| {
            let Some(link) = cell.link() else { return };
            let target = match link.place() {
                Some(place) => {
                    let name = place.split_once('!').map_or("", |(s, _)| s).trim_matches('\'');
                    match sheets.iter().find(|(n, _)| n == name) {
                        Some((_, first)) => LinkTarget::Page(*first, 0.0),
                        None => return,
                    }
                }
                None => LinkTarget::Uri(link.target.to_string()),
            };
            links.push((g.cell_rect(sh, c).intersect(pane), target));
        });
    }

    #[allow(clippy::too_many_arguments)]
    /// Column letters above a pane (with `hdr_h`) and row numbers left of
    /// it (with `hdr_w`).
    fn print_headings(&self, p: &egui::Painter, pane: Rect, rows: (u32, u32), cols: (u32, u32), z: f32, hdr_h: Option<f32>, hdr_w: Option<f32>) {
        let sh = self.sh();
        let bg = Color32::from_gray(235);
        let line = Stroke::new(0.75, Color32::from_gray(150));
        let font = FontId::proportional(11.0 * z);
        let text = Color32::from_gray(60);
        if let Some(hdr_h) = hdr_h {
            let top = pane.min.y - hdr_h;
            let mut x = pane.min.x;
            for c in cols.0..=cols.1 {
                let w = sh.col_width(c) * z;
                if w > 0.0 {
                    let r = Rect::from_min_size(pos2(x, top), vec2(w, hdr_h));
                    p.rect_filled(r, 0.0, bg);
                    p.rect_stroke(r, 0.0, line, egui::StrokeKind::Inside);
                    p.text(r.center(), egui::Align2::CENTER_CENTER, col_name(c), font.clone(), text);
                }
                x += w;
            }
        }
        if let Some(hdr_w) = hdr_w {
            let left = pane.min.x - hdr_w;
            let mut y = pane.min.y;
            for r in rows.0..=rows.1 {
                let h = sh.row_height(r) * z;
                if h > 0.0 {
                    let rr = Rect::from_min_size(pos2(left, y), vec2(hdr_w, h));
                    p.rect_filled(rr, 0.0, bg);
                    p.rect_stroke(rr, 0.0, line, egui::StrokeKind::Inside);
                    p.text(rr.center(), egui::Align2::CENTER_CENTER, (r + 1).to_string(), font.clone(), text);
                }
                y += h;
            }
        }
    }
}
