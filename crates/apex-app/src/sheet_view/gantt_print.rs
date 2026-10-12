//! Planning > Print Gantt: the table and its Gantt over as many pages as
//! they need, by a range of dates and of tasks. Every page repeats the
//! time scale and the table's header (and the table itself, if wanted),
//! with a header, a footer and a legend. The pages go to the print dialog,
//! which previews, prints and exports them as PDF.

use apex_sheet::workbook::TableId;
use eframe::egui::Shape;
use eframe::egui::epaint::ClippedShape;

use super::f10::parse_num;
use super::gantt::{Look, Scale, galley};
use super::gantt_export::Canvas;
use super::plan_ui::date_text;
use super::*;
use crate::print::{Job, PT, Page};
use crate::widgets::DialogResult;

/// The options of Print Gantt.
#[derive(Clone)]
pub(super) struct GanttPrint {
    pub id: TableId,
    /// Dates shown (empty: the whole plan).
    pub from: String,
    pub to: String,
    /// Tasks printed, by number (empty: all).
    pub first: String,
    pub last: String,
    pub scale: Scale,
    /// Zoom in percent.
    pub zoom: String,
    /// Paper in points, portrait width and height as given.
    pub paper_w: f32,
    pub paper_h: f32,
    pub landscape: bool,
    /// The table on every page (else on the first column of pages only).
    pub repeat_table: bool,
    pub legend: bool,
    /// Header and footer; &[Page], &[Pages], &[Date] and &[Plan] are
    /// replaced.
    pub header: String,
    pub footer: String,
    pub error: Option<&'static str>,
}

/// Page margins, in points.
const MARGIN: f32 = 36.0;
/// Room for the header and the footer, in points.
const HF: f32 = 20.0;
/// Room for the legend, in points.
const LEGEND: f32 = 22.0;

/// The pages the canvas is cut into: canvas rows (y0, y1) of the body and
/// canvas columns (x0, x1) of the Gantt, and whether the table shows.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Cut {
    pub rows: (f32, f32),
    pub cols: (f32, f32),
    pub table: bool,
}

/// Cuts `c` into pages whose content is `room` (canvas units), down the
/// rows of tasks `tasks` (a range of task indexes) first, then across.
pub(super) fn cut(c: &Canvas, room: Vec2, tasks: (usize, usize), repeat_table: bool) -> Vec<Cut> {
    // Rows: whole task rows per page.
    let body_h = (room.y - c.head_h).max(8.0);
    let mut bands: Vec<(f32, f32)> = Vec::new();
    for &(_, y0, y1) in c.rows.iter().filter(|r| r.0 >= tasks.0 && r.0 <= tasks.1) {
        match bands.last_mut() {
            Some(b) if y1 - b.0 <= body_h => b.1 = y1,
            _ => bands.push((y0, y1)),
        }
    }
    if bands.is_empty() {
        bands.push((c.head_h, c.head_h));
    }
    // Columns: the Gantt beside the table (which is left out when it
    // doesn't leave room for the Gantt).
    let fits = c.table_w < room.x * 0.7;
    let mut cols: Vec<((f32, f32), bool)> = Vec::new();
    let mut x = c.table_w;
    while x < c.size.x - 0.5 || cols.is_empty() {
        let table = fits && (cols.is_empty() || repeat_table);
        let w = if table { room.x - c.table_w } else { room.x };
        cols.push(((x, (x + w).min(c.size.x)), table));
        x += w;
    }
    let mut out = Vec::new();
    for (cols, table) in cols {
        for &rows in &bands {
            out.push(Cut { rows, cols, table });
        }
    }
    out
}

/// Copies what `src` paints in `from` to `to` (its top left corner).
fn blit(out: &mut Vec<ClippedShape>, src: &[ClippedShape], from: Rect, to: Pos2) {
    let d = to - from.min;
    for s in src {
        let clip = s.clip_rect.intersect(from);
        if clip.width() <= 0.0 || clip.height() <= 0.0 {
            continue;
        }
        let b = s.shape.visual_bounding_rect();
        if b.is_positive() && !b.intersects(clip) {
            continue;
        }
        let mut shape = s.shape.clone();
        shape.translate(d);
        out.push(ClippedShape { clip_rect: clip.translate(d), shape });
    }
}

/// The Gantt's legend: its kinds of bars, from `at`, `z` units per point.
fn legend(ctx: &egui::Context, out: &mut Vec<ClippedShape>, at: Pos2, z: f32, clip: Rect) {
    let items: [(&str, Color32, u8); 6] = [
        ("plan.legend_task", Color32::from_rgb(0x44, 0x72, 0xC4), 0),
        ("plan.legend_critical", Color32::from_rgb(0xD9, 0x3B, 0x3B), 0),
        ("plan.legend_progress", Color32::from_rgb(0x1F, 0x3E, 0x7A), 1),
        ("plan.legend_summary", Color32::from_rgb(0x30, 0x30, 0x30), 2),
        ("plan.legend_milestone", Color32::from_rgb(0x30, 0x30, 0x30), 3),
        ("plan.legend_baseline", Color32::from_rgb(0xA8, 0xAD, 0xB5), 2),
    ];
    let mut x = at.x;
    let h = 8.0 * z;
    for (key, color, kind) in items {
        let r = Rect::from_min_size(pos2(x, at.y + 3.0 * z), vec2(22.0 * z, h));
        let shape = match kind {
            3 => {
                let c = pos2(r.left() + h / 2.0 + 2.0 * z, r.center().y);
                let s = h / 2.0 + z;
                Shape::convex_polygon(vec![pos2(c.x, c.y - s), pos2(c.x + s, c.y), pos2(c.x, c.y + s), pos2(c.x - s, c.y)], color, Stroke::NONE)
            }
            2 => Shape::rect_filled(r.shrink2(vec2(0.0, h * 0.25)), 0.0, color),
            1 => Shape::rect_filled(r.shrink2(vec2(0.0, h * 0.32)), 0.0, color),
            _ => Shape::rect_filled(r, 2.0 * z, color),
        };
        out.push(ClippedShape { clip_rect: clip, shape });
        let g = galley(ctx, tr(key).to_string(), 9.0 * z, Color32::from_gray(60));
        let w = g.size().x;
        out.push(ClippedShape { clip_rect: clip, shape: Shape::galley(pos2(r.right() + 4.0 * z, r.center().y - g.size().y / 2.0), g, Color32::from_gray(60)) });
        x = r.right() + w + 16.0 * z;
    }
}

/// `&[Page]`, `&[Pages]`, `&[Date]` and `&[Plan]` of a header or footer.
pub(super) fn hf_text(s: &str, page: usize, pages: usize, date: &str, plan: &str) -> String {
    s.replace("&[Pages]", &pages.to_string()).replace("&[Page]", &page.to_string()).replace("&[Date]", date).replace("&[Plan]", plan)
}

impl GanttPrint {
    pub fn new(v: &SheetView, id: TableId) -> GanttPrint {
        let page = &v.wb.sheets[id.0].page;
        GanttPrint {
            id,
            from: String::new(),
            to: String::new(),
            first: String::new(),
            last: String::new(),
            scale: v.plan.scale,
            zoom: "100".into(),
            paper_w: page.paper_width,
            paper_h: page.paper_height,
            landscape: true,
            repeat_table: true,
            legend: true,
            header: "&[Plan]".into(),
            footer: tr("plan.print_footer").to_string(),
            error: None,
        }
    }

    /// The dates, tasks and zoom as typed, or the key of what is wrong.
    fn parse(&self, wb: &Workbook) -> Result<(Option<(f64, f64)>, Option<(usize, usize)>, f32), &'static str> {
        let date = |s: &str| if s.trim().is_empty() { Ok(None) } else { parse_num(wb, s).filter(|d| *d > 0.0).map(|d| Some(d.floor())).ok_or("plan.err_date") };
        let dates = match (date(&self.from)?, date(&self.to)?) {
            (None, None) => None,
            (a, b) => {
                let (a, b) = (a.unwrap_or(0.0), b.unwrap_or(f64::MAX));
                if a > b {
                    return Err("plan.print_bad_dates");
                }
                Some((a, b))
            }
        };
        let task = |s: &str| if s.trim().is_empty() { Ok(None) } else { s.trim().parse::<usize>().ok().filter(|n| *n > 0).map(|n| Some(n - 1)).ok_or("plan.print_bad_tasks") };
        let tasks = match (task(&self.first)?, task(&self.last)?) {
            (None, None) => None,
            (a, b) => Some((a.unwrap_or(0), b.unwrap_or(usize::MAX))),
        };
        let zoom = parse_num(wb, self.zoom.trim_end_matches('%')).filter(|z| (10.0..=400.0).contains(z)).ok_or("plan.print_bad_zoom")? as f32 / 100.0;
        Ok((dates, tasks, zoom))
    }
}

impl SheetView {
    pub(super) fn open_gantt_print(&mut self, id: TableId) {
        self.settle_edit();
        self.plan.print = Some(Box::new(GanttPrint::new(self, id)));
    }

    /// The pages of the Gantt with options `o`.
    pub(super) fn gantt_print_pages(&mut self, ctx: &egui::Context, o: &GanttPrint) -> Option<Vec<Page>> {
        let (dates, tasks, zoom) = o.parse(&self.wb).ok()?;
        let v = self.plan_view_cached(o.id)?;
        // Open-ended date ranges end at the plan's.
        let dates = dates.map(|(a, b)| {
            let (first, last) = v.schedule.tasks.iter().fold((v.plan.start, v.plan.start), |(x, y), t| (x.min(t.start), y.max(t.finish)));
            (if a <= 0.0 { first.floor() - 2.0 } else { a }, if b == f64::MAX { last.ceil() + 3.0 } else { b + 1.0 })
        });
        // Sheet pixels to page pixels, as sheets print.
        let z = zoom * PT * 0.75;
        let c = self.gantt_canvas(ctx, o.id, z, dates, o.scale)?;
        let (pw, ph) = if o.landscape { (o.paper_h, o.paper_w) } else { (o.paper_w, o.paper_h) };
        let size = vec2(pw * PT, ph * PT);
        let m = MARGIN * PT;
        let top = m + HF * PT;
        let bottom = size.y - m - HF * PT - if o.legend { LEGEND * PT } else { 0.0 };
        let room = vec2(size.x - 2.0 * m, (bottom - top).max(40.0));
        let tasks = tasks.unwrap_or((0, usize::MAX));
        let cuts = cut(&c, room, tasks, o.repeat_table);
        let plan = self.wb.table(o.id).map(|t| t.name.clone()).unwrap_or_default();
        let date = date_text(&self.wb, apex_sheet::format::now_serial());
        let n = cuts.len();
        let look = Look::paper(Color32::from_gray(243));
        let mut pages = Vec::new();
        for (i, k) in cuts.iter().enumerate() {
            let page = Rect::from_min_size(Pos2::ZERO, size);
            let mut out = vec![ClippedShape { clip_rect: page, shape: Shape::rect_filled(page, 0.0, Color32::WHITE) }];
            let mut x = m;
            let body_h = k.rows.1 - k.rows.0;
            if k.table {
                blit(&mut out, &c.shapes, Rect::from_min_max(Pos2::ZERO, pos2(c.table_w, c.head_h)), pos2(x, top));
                blit(&mut out, &c.shapes, Rect::from_min_max(pos2(0.0, k.rows.0), pos2(c.table_w, k.rows.1)), pos2(x, top + c.head_h));
                x += c.table_w;
            }
            blit(&mut out, &c.shapes, Rect::from_min_max(pos2(k.cols.0, 0.0), pos2(k.cols.1, c.head_h)), pos2(x, top));
            blit(&mut out, &c.shapes, Rect::from_min_max(pos2(k.cols.0, k.rows.0), pos2(k.cols.1, k.rows.1)), pos2(x, top + c.head_h));
            // A frame around what was printed.
            let used = Rect::from_min_size(pos2(m, top), vec2(x - m + k.cols.1 - k.cols.0, c.head_h + body_h));
            out.push(ClippedShape { clip_rect: page, shape: Shape::rect_stroke(used, 0.0, Stroke::new(1.0, look.grid), StrokeKind::Outside) });
            let hz = PT;
            for (text, y, right) in [(&o.header, m, false), (&o.footer, size.y - m - 12.0 * hz, true)] {
                let s = hf_text(text, i + 1, n, &date, &plan);
                if s.trim().is_empty() {
                    continue;
                }
                let g = galley(ctx, s, 10.0 * hz, look.weak);
                let gx = if right { size.x - m - g.size().x } else { m };
                out.push(ClippedShape { clip_rect: page, shape: Shape::galley(pos2(gx, y), g, look.weak) });
            }
            if o.legend {
                legend(ctx, &mut out, pos2(m, bottom + 4.0 * hz), hz, page);
            }
            pages.push(Page { size, shapes: out, links: Vec::new(), tags: Vec::new() });
        }
        Some(pages)
    }

    /// The print job of the Gantt, for the print dialog.
    pub(super) fn gantt_print_job(&mut self, o: &GanttPrint) -> Option<Job> {
        let ctx = self.ctx.clone()?;
        let pages = self.gantt_print_pages(&ctx, o)?;
        let title = self.wb.table(o.id).map(|t| t.name.clone()).unwrap_or_default();
        Some(Job { title, pages, ..Default::default() })
    }

    /// Shows the Print Gantt dialog while it is open; OK opens the print
    /// dialog with its pages.
    pub(super) fn gantt_print_dialog(&mut self, ctx: &egui::Context, shell: &mut Shell) {
        let Some(mut d) = self.plan.print.take() else { return };
        let res = super::f10::tool_window(ctx, "plan_print", tr("plan.print"), 480.0, false, |ui| {
            egui::Grid::new("plan_print_grid").num_columns(2).spacing(vec2(12.0, 6.0)).show(ui, |ui| {
                ui.label(tr("plan.print_dates"));
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut d.from).desired_width(100.0).hint_text(tr("plan.print_start")));
                    ui.label("–");
                    ui.add(egui::TextEdit::singleline(&mut d.to).desired_width(100.0).hint_text(tr("plan.print_end")));
                });
                ui.end_row();
                ui.label(tr("plan.print_tasks"));
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut d.first).desired_width(60.0).hint_text("1"));
                    ui.label("–");
                    ui.add(egui::TextEdit::singleline(&mut d.last).desired_width(60.0).hint_text(tr("plan.print_all")));
                });
                ui.end_row();
                ui.label(tr("plan.timescale"));
                egui::ComboBox::from_id_salt("plan_print_scale").selected_text(tr(d.scale.key())).show_ui(ui, |ui| {
                    for s in Scale::ALL {
                        ui.selectable_value(&mut d.scale, s, tr(s.key()));
                    }
                });
                ui.end_row();
                ui.label(tr("plan.print_zoom"));
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut d.zoom).desired_width(50.0));
                    ui.label("%");
                });
                ui.end_row();
                ui.label(tr("plan.print_header"));
                ui.add(egui::TextEdit::singleline(&mut d.header).desired_width(280.0));
                ui.end_row();
                ui.label(tr("plan.print_footer_label"));
                ui.add(egui::TextEdit::singleline(&mut d.footer).desired_width(280.0));
                ui.end_row();
            });
            ui.label(RichText::new(tr("plan.print_fields")).small().weak());
            ui.checkbox(&mut d.repeat_table, tr("plan.print_repeat"));
            ui.checkbox(&mut d.legend, tr("plan.print_legend"));
            crate::print::orientation_ui(ui, &mut d.landscape);
            crate::print::paper_ui(ui, &mut d.paper_w, &mut d.paper_h, false);
            if let Some(e) = d.error {
                ui.colored_label(Color32::from_rgb(200, 40, 40), tr(e));
            }
        });
        match res {
            DialogResult::None => self.plan.print = Some(d),
            DialogResult::Cancel => {}
            DialogResult::Ok => {
                if let Err(e) = d.parse(&self.wb) {
                    d.error = Some(e);
                    self.plan.print = Some(d);
                    return;
                }
                // The next print job is the Gantt's.
                self.plan.print_next = Some(d);
                shell.cmd = Some(crate::common::AppCmd::Print);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::print::Scope;
    use crate::sheet_view::plan_ui::template_tasks;

    fn canvas(rows: usize) -> Canvas {
        Canvas { shapes: Vec::new(), size: vec2(1000.0, 40.0 + rows as f32 * 20.0), table_w: 300.0, head_h: 40.0, rows: (0..rows).map(|i| (i, 40.0 + i as f32 * 20.0, 60.0 + i as f32 * 20.0)).collect() }
    }

    /// Down the rows first, then across; whole rows; the table repeated
    /// or on the first column of pages only.
    #[test]
    fn cuts() {
        let c = canvas(10);
        // 4 rows and 400 units of Gantt a page: 3 x 2 pages.
        let k = cut(&c, vec2(700.0, 40.0 + 85.0), (0, usize::MAX), true);
        assert_eq!(k.len(), 6);
        assert_eq!(k[0], Cut { rows: (40.0, 120.0), cols: (300.0, 700.0), table: true });
        assert_eq!(k[2].rows, (200.0, 240.0));
        assert_eq!(k[3].cols, (700.0, 1000.0));
        assert!(k.iter().all(|x| x.table));
        let k = cut(&c, vec2(700.0, 40.0 + 85.0), (0, usize::MAX), false);
        assert_eq!(k.len(), 6);
        assert_eq!(k[3], Cut { rows: (40.0, 120.0), cols: (700.0, 1000.0), table: false });
        // Tasks 3 to 5 only.
        let k = cut(&c, vec2(700.0, 1000.0), (2, 4), true);
        assert_eq!(k.len(), 2);
        assert_eq!(k[0].rows, (80.0, 140.0));
        // A table wider than the page is left out.
        let k = cut(&c, vec2(350.0, 1000.0), (0, usize::MAX), true);
        assert!(!k[0].table);
        assert_eq!(k[0].cols, (300.0, 650.0));
        assert_eq!(hf_text("&[Plan] &[Page]/&[Pages]", 2, 5, "", "P"), "P 2/5");
    }

    /// Print Gantt makes the next print job: pages of the paper's size,
    /// more of them for a longer range of dates or a larger zoom.
    #[test]
    fn gantt_pages() {
        let mut wb = Workbook::new();
        wb.locale = apex_sheet::Locale::EN;
        let id = wb.insert_plan(0, CellRef::new(0, 0), apex_sheet::format::date_to_serial(2026, 10, 5), &template_tasks(0)).unwrap();
        let mut v = SheetView::new(wb);
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
        v.ctx = Some(ctx.clone());
        let mut o = GanttPrint::new(&v, id);
        let one = v.gantt_print_pages(&ctx, &o).unwrap();
        assert!(!one.is_empty());
        let (pw, ph) = (o.paper_h * PT, o.paper_w * PT);
        assert!(one.iter().all(|p| (p.size.x - pw).abs() < 0.5 && (p.size.y - ph).abs() < 0.5));
        o.zoom = "300".into();
        o.scale = Scale::Day;
        let many = v.gantt_print_pages(&ctx, &o).unwrap();
        assert!(many.len() > one.len(), "{} {}", many.len(), one.len());
        // Two weeks of the first three tasks fit on a page.
        o.zoom = "100".into();
        o.from = "10/5/2026".into();
        o.to = "10/18/2026".into();
        o.first = "1".into();
        o.last = "3".into();
        assert_eq!(v.gantt_print_pages(&ctx, &o).unwrap().len(), 1);
        o.first = "x".into();
        assert!(o.parse(&v.wb).is_err());
        o.first = "1".into();
        // The next print job is the Gantt's, once.
        v.plan.print_next = Some(Box::new(o));
        let job = v.print_job("plan", Scope::ActiveSheet);
        assert!(job.plan.is_none() && job.pages.len() == 1);
        let pdf = crate::pdf_out::to_pdf(&job, &[0], &mut crate::print::no_paint);
        assert!(pdf.starts_with(b"%PDF"));
        assert!(v.print_job("plan", Scope::ActiveSheet).plan.is_some());
    }
}
