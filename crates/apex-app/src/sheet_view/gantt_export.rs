//! Planning > Export: the plan table with its Gantt beside it, as a
//! picture (PNG) or as a page of PDF (vector, with its fonts).

use apex_sheet::plan::Field;
use apex_sheet::workbook::TableId;
use eframe::egui::Shape;
use eframe::egui::epaint::ClippedShape;

use super::gantt::{Axis, Lane, Look, Scale, galley, paint_body, paint_scale};
use super::*;

/// Largest side of an exported picture, in pixels.
const MAX_PX: f32 = 8000.0;

/// The table and its Gantt painted from the top left corner.
pub(super) struct Canvas {
    pub shapes: Vec<ClippedShape>,
    pub size: Vec2,
    /// Width of the table on the left.
    pub table_w: f32,
    /// Height of what heads every page: the time scale and the table's
    /// header row.
    pub head_h: f32,
    /// Shown task rows: task index, top and bottom.
    pub rows: Vec<(usize, f32, f32)>,
}

impl SheetView {
    /// Asks where and saves the Gantt of plan `id`.
    pub(super) fn export_gantt(&mut self, id: TableId, pdf: bool) {
        self.settle_edit();
        let Some(ctx) = self.ctx.clone() else { return };
        let ext = if pdf { "pdf" } else { "png" };
        let name = self.wb.table(id).map(|t| t.name.clone()).unwrap_or_default();
        let dlg = crate::file_dialog::FileDialog::new().set_title(tr(if pdf { "plan.export_pdf" } else { "plan.export_png" })).add_filter(if pdf { "PDF" } else { "PNG" }, &[ext]).set_file_name(format!("{name}.{ext}"));
        dlg.save_file(crate::file_dialog::in_view(move |v: &mut SheetView, path: std::path::PathBuf| {
            let data = if pdf { v.gantt_pdf(&ctx, id) } else { v.gantt_png(&ctx, id, 2.0) };
            match data.map(|d| crate::common::write_out(&path, d)) {
                Some(Ok(())) => v.notices.push(Notice::info(trf("plan.exported", &[("path", &path.display())]))),
                _ => v.notices.push(Notice::error(tr("plan.export_failed"))),
            }
        }));
    }

    /// The table and its Gantt at `z` (units per sheet pixel), from the
    /// top left corner, and their size.
    pub(super) fn gantt_page(&mut self, ctx: &egui::Context, id: TableId, z: f32) -> Option<(Vec<ClippedShape>, Vec2)> {
        let c = self.gantt_canvas(ctx, id, z, None, self.plan.scale)?;
        Some((c.shapes, c.size))
    }

    /// The table and its Gantt at `z`, over `dates` (None: the whole plan,
    /// narrowed to fit a picture) on time scale `scale`.
    pub(super) fn gantt_canvas(&mut self, ctx: &egui::Context, id: TableId, z: f32, dates: Option<(f64, f64)>, scale: Scale) -> Option<Canvas> {
        let v = self.plan_view_cached(id)?;
        let t = self.wb.table(id)?.clone();
        let sheet = id.0;
        let sh = &self.wb.sheets[sheet];
        let strip = HEADER_H * z;
        let r = t.range;
        let cols: Vec<(u32, f32, f32)> = {
            let mut x = 0.0;
            (r.start.col..=r.end.col)
                .filter_map(|c| {
                    let w = sh.col_width(c) * z;
                    let out = (w > 0.0).then_some((c, x, w));
                    x += w;
                    out
                })
                .collect()
        };
        let table_w = cols.last().map_or(0.0, |c| c.1 + c.2);
        let y_of = |row: u32| strip + (sh.row_y(row) - sh.row_y(r.start.row)) * z;
        let height = y_of(r.end.row) + sh.row_height(r.end.row) * z;
        // The dates of the whole plan, with a margin.
        let (first, last) = v.schedule.tasks.iter().fold((v.plan.start, v.plan.start), |(a, b), o| (a.min(o.start), b.max(o.finish)));
        let mut px = scale.px() * z;
        let (from, to) = dates.unwrap_or((first.floor() - 2.0 * scale.days_per_unit().min(7.0), last.ceil() + 3.0 * scale.days_per_unit().min(7.0)));
        let to = to.max(from + 1.0);
        let mut gantt_w = ((to - from) as f32 * px).max(120.0 * z);
        if dates.is_none() && table_w + gantt_w > MAX_PX * z / 2.0 {
            // Very long plans: the days get narrower.
            let room = (MAX_PX * z / 2.0 - table_w).max(200.0 * z);
            px *= room / gantt_w;
            gantt_w = room;
        }
        let ax = Axis { x0: table_w, left: from, px };
        let size = vec2(table_w + gantt_w, height);
        let look = Look::paper(Color32::from_gray(243));
        let everything = Rect::from_min_size(Pos2::ZERO, size);
        let mut out: Vec<ClippedShape> = vec![ClippedShape { clip_rect: everything, shape: Shape::rect_filled(everything, 0.0, Color32::WHITE) }];

        // The table: its cells as they show, with the grid lines.
        let name_col = v.col(Field::Name);
        for row in r.start.row..=r.end.row {
            let h = sh.row_height(row) * z;
            if h <= 0.0 {
                continue;
            }
            let y = y_of(row);
            let header = t.header && row == r.start.row;
            for &(c, x, w) in &cols {
                let rect = Rect::from_min_size(pos2(x, y), vec2(w, h));
                if header {
                    out.push(ClippedShape { clip_rect: everything, shape: Shape::rect_filled(rect, 0.0, look.header) });
                }
                out.push(ClippedShape { clip_rect: everything, shape: Shape::rect_stroke(rect, 0.0, Stroke::new(1.0, look.grid), StrokeKind::Inside) });
                let cell = CellRef::new(row, c);
                let (text, color) = self.wb.display(sheet, cell);
                if text.is_empty() {
                    continue;
                }
                let mut st = self.wb.styles.get(self.wb.cell(sheet, cell).map_or(0, |x| x.style)).clone();
                st.bold |= header;
                let color = color.or(st.color).map_or(look.text, theme::rgb);
                let g = ctx.fonts_mut(|f| f.layout_job(cell_job(&text, &st, color, z, None)));
                let number = matches!(sh.value(cell), Value::Number(_));
                let indent = if Some(c) == name_col { st.indent as f32 * INDENT_PX * z } else { 0.0 };
                let tx = if number && !header { rect.right() - 3.0 * z - g.size().x } else { rect.left() + 3.0 * z + indent };
                out.push(ClippedShape { clip_rect: rect, shape: Shape::galley(pos2(tx, rect.center().y - g.size().y / 2.0), g, color) });
            }
        }

        // The Gantt beside it.
        let lanes: Vec<Lane> = v.rows.iter().enumerate().filter(|(_, row)| sh.row_height(**row) > 0.0).map(|(i, row)| Lane { task: i, y0: y_of(*row), y1: y_of(*row) + sh.row_height(*row) * z }).collect();
        let body = Rect::from_min_max(pos2(table_w, strip), size.to_pos2());
        let mut shapes = Vec::new();
        paint_body(ctx, &mut shapes, &v, &ax, &lanes, body, scale, &look, z, apex_sheet::format::now_serial().floor());
        let head = Rect::from_min_max(pos2(table_w, 0.0), pos2(size.x, strip));
        paint_scale(ctx, &mut shapes, &ax, head, scale, true, &look, z);
        if t.header {
            let y = y_of(r.start.row);
            paint_scale(ctx, &mut shapes, &ax, Rect::from_min_max(pos2(table_w, y), pos2(size.x, y + sh.row_height(r.start.row) * z)), scale, false, &look, z);
        }
        shapes.push(Shape::line_segment([pos2(table_w, 0.0), pos2(table_w, size.y)], Stroke::new(1.0, look.grid)));
        let gantt_clip = Rect::from_min_max(pos2(table_w, 0.0), size.to_pos2());
        out.extend(shapes.into_iter().map(|shape| ClippedShape { clip_rect: gantt_clip, shape }));
        // The plan's name over the table.
        let title = galley(ctx, t.name.clone(), 11.0 * z, look.weak);
        out.push(ClippedShape { clip_rect: everything, shape: Shape::galley(pos2(3.0 * z, strip / 2.0 - title.size().y / 2.0), title, look.weak) });
        let head_h = if t.header { y_of(r.start.row) + sh.row_height(r.start.row) * z } else { strip };
        let rows = lanes.iter().map(|l| (l.task, l.y0, l.y1)).collect();
        Some(Canvas { shapes: out, size, table_w, head_h, rows })
    }

    /// A PNG of the table and its Gantt, `scale` pixels per sheet pixel.
    pub(super) fn gantt_png(&mut self, ctx: &egui::Context, id: TableId, scale: f32) -> Option<Vec<u8>> {
        let ppp = ctx.pixels_per_point();
        let (shapes, size) = self.gantt_page(ctx, id, scale / ppp)?;
        let (w, h) = ((size.x * ppp).round() as usize, (size.y * ppp).round() as usize);
        if w == 0 || h == 0 || w > 16384 || h > 16384 {
            return None;
        }
        let prims = ctx.tessellate(shapes, ppp);
        let atlas = ctx.fonts(|f| f.image());
        let mut img = egui::ColorImage::new([w, h], vec![Color32::WHITE; w * h]);
        for p in prims {
            if let egui::epaint::Primitive::Mesh(m) = p.primitive
                && m.texture_id == egui::TextureId::default()
            {
                crate::charts::raster(&mut img, &m, &atlas, ppp);
            }
        }
        crate::charts::encode_png(&img)
    }

    /// A one-page PDF of the table and its Gantt.
    pub(super) fn gantt_pdf(&mut self, ctx: &egui::Context, id: TableId) -> Option<Vec<u8>> {
        let (shapes, size) = self.gantt_page(ctx, id, 1.0)?;
        let title = self.wb.table(id).map(|t| t.name.clone()).unwrap_or_default();
        let page = crate::print::Page { size, shapes, links: Vec::new(), tags: Vec::new() };
        let job = crate::print::Job { title, pages: vec![page], ..Default::default() };
        Some(crate::pdf_out::to_pdf(&job, &[0], &mut crate::print::no_paint))
    }
}
