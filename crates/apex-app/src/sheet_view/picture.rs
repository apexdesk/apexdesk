//! Pictures of cells (Paste Special > Picture): fills, gridlines, borders
//! and text drawn off screen with the chart rasterizer.

use super::*;
use egui::Shape;

const GRID: Color32 = Color32::from_rgb(0xD9, 0xD9, 0xD9);

/// Renders `r` of `sheet` at `scale` pixels per sheet pixel, on white.
/// `None` when the range is empty or too big.
pub(super) fn range_picture(wb: &Workbook, ctx: &egui::Context, sheet: usize, r: &Range, scale: f32) -> Option<egui::ColorImage> {
    let sh = &wb.sheets[sheet];
    if r.cell_count() > 20_000 {
        return None;
    }
    let xs: Vec<f32> = std::iter::once(0.0).chain((r.start.col..=r.end.col).scan(0.0, |x, c| {
        *x += sh.col_width(c);
        Some(*x)
    })).collect();
    let ys: Vec<f32> = std::iter::once(0.0).chain((r.start.row..=r.end.row).scan(0.0, |y, row| {
        *y += sh.row_height(row);
        Some(*y)
    })).collect();
    let (w, h) = (*xs.last()?, *ys.last()?);
    let (pw, ph) = ((w * scale).round() as usize, (h * scale).round() as usize);
    if pw == 0 || ph == 0 || pw > 8192 || ph > 8192 {
        return None;
    }
    let ppp = ctx.pixels_per_point();
    // Laid out in points so text comes out at `scale` pixels per sheet pixel.
    let z = scale / ppp;
    let rect_of = |a: CellRef, b: CellRef| {
        let (c0, c1) = ((a.col - r.start.col) as usize, (b.col.min(r.end.col) - r.start.col) as usize + 1);
        let (r0, r1) = ((a.row - r.start.row) as usize, (b.row.min(r.end.row) - r.start.row) as usize + 1);
        Rect::from_min_max(pos2(xs[c0] * z, ys[r0] * z), pos2(xs[c1] * z, ys[r1] * z))
    };
    let merges: Vec<Range> = sh.merges.iter().filter(|m| m.intersect(r).is_some()).copied().collect();
    let mut fills = Vec::new();
    let mut lines = Vec::new();
    let mut texts = Vec::new();
    for row in r.start.row..=r.end.row {
        for col in r.start.col..=r.end.col {
            let c = CellRef::new(row, col);
            let merge = merges.iter().find(|m| m.contains(c));
            if merge.is_some_and(|m| m.start != c && (m.start.row >= r.start.row && m.start.col >= r.start.col)) {
                continue;
            }
            let rect = match merge {
                Some(m) => rect_of(c, m.end),
                None => rect_of(c, c),
            };
            if rect.width() <= 0.0 || rect.height() <= 0.0 {
                continue;
            }
            let cell = sh.get(c);
            let st = wb.styles.get(cell.map_or(0, |x| x.style));
            if let Some(f) = st.fill {
                fills.push(Shape::rect_filled(rect, 0.0, theme::rgb(f)));
            } else {
                lines.push(Shape::line_segment([rect.right_top(), rect.right_bottom()], Stroke::new(z, GRID)));
                lines.push(Shape::line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(z, GRID)));
            }
            let b = st.borders;
            for (l, a, e) in [(b.top, rect.left_top(), rect.right_top()), (b.bottom, rect.left_bottom(), rect.right_bottom()), (b.left, rect.left_top(), rect.left_bottom()), (b.right, rect.right_top(), rect.right_bottom())] {
                if !l.is_none() {
                    let color = l.color.map_or(Color32::BLACK, theme::rgb);
                    let s = Stroke::new(l.style.width().max(1.0) * z, color);
                    match l.style {
                        LineStyle::Dashed | LineStyle::Dotted => lines.extend(Shape::dashed_line(&[a, e], s, 4.0 * z, 2.0 * z)),
                        _ => lines.push(Shape::line_segment([a, e], s)),
                    }
                }
            }
            let Some(cell) = cell else { continue };
            let value = cell.value();
            if value.is_empty() {
                continue;
            }
            let (text, color) = wb.display(sheet, c);
            let color = color.or(st.color).map_or(Color32::BLACK, theme::rgb);
            let pad = 3.0 * z;
            let wrap = st.wrap.then_some(rect.width() - 2.0 * pad);
            let galley = ctx.fonts_mut(|f| f.layout_job(cell_job(&text, st, color, z, wrap)));
            let size = galley.size();
            let halign = match st.halign {
                HAlign::General => match value {
                    Value::Number(_) => HAlign::Right,
                    Value::Bool(_) | Value::Error(_) => HAlign::Center,
                    _ => HAlign::Left,
                },
                h => h,
            };
            let x = match halign {
                HAlign::Right => rect.max.x - pad - size.x,
                HAlign::Center => rect.center().x - size.x / 2.0,
                _ => rect.min.x + pad + st.indent as f32 * INDENT_PX * z,
            };
            let y = match st.valign {
                VAlign::Top => rect.min.y + z,
                VAlign::Center => rect.center().y - size.y / 2.0,
                VAlign::Bottom => rect.max.y - size.y - z,
            };
            texts.push(egui::epaint::ClippedShape { clip_rect: rect, shape: Shape::galley(pos2(x, y), galley, color) });
        }
    }
    let all = Rect::EVERYTHING;
    let mut shapes: Vec<egui::epaint::ClippedShape> = fills.into_iter().chain(lines).map(|shape| egui::epaint::ClippedShape { clip_rect: all, shape }).collect();
    shapes.extend(texts);
    let prims = ctx.tessellate(shapes, ppp);
    let atlas = ctx.fonts(|f| f.image());
    let mut img = egui::ColorImage::new([pw, ph], vec![Color32::WHITE; pw * ph]);
    for p in prims {
        if let egui::epaint::Primitive::Mesh(m) = p.primitive {
            if m.texture_id == egui::TextureId::default() {
                crate::charts::raster(&mut img, &m, &atlas, ppp);
            }
        }
    }
    Some(img)
}
