//! Pages drawn without pdfium: the shapes, pictures and text that
//! `apex_io::pdf` reads, painted with egui (fonts are the nearest ones
//! ApexDesk has, so pages look close to, not exactly as, printed), and the
//! annotations as simple marks.

use apex_io::pdf::Page;
use apex_io::pdf::edit::annots::{Annot, Kind};
use eframe::egui::{self, Align2, Color32, Pos2, Rect, Stroke, Vec2};

use super::pages::PageMap;

fn color(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Paints page `p` where `map` puts it.
pub fn paint(ui: &egui::Ui, painter: &egui::Painter, p: &Page, map: &PageMap) {
    let z = map.z;
    let at = |x: f32, y: f32| map.to_screen(x, y);
    let known: std::collections::HashSet<egui::FontFamily> = ui.ctx().fonts(|f| f.definitions().families.keys().cloned().collect());
    for s in &p.shapes {
        let pts: Vec<Pos2> = s.points.iter().map(|q| at(q[0], q[1])).collect();
        if pts.len() < 2 {
            continue;
        }
        if let Some(f) = s.fill {
            if s.rect {
                painter.rect_filled(Rect::from_points(&pts), 0.0, color(f));
            } else if pts.len() > 2 {
                painter.add(egui::Shape::convex_polygon(pts.clone(), color(f), Stroke::NONE));
            }
        }
        if let Some((c, w)) = s.stroke {
            let stroke = Stroke::new((w * z).max(0.6), color(c));
            if s.closed {
                painter.add(egui::Shape::closed_line(pts, stroke));
            } else {
                painter.add(egui::Shape::line(pts, stroke));
            }
        }
    }
    for im in &p.images {
        let r = map.rect_to_screen([im.x, im.y, im.x + im.w, im.y + im.h]);
        if !ui.is_rect_visible(r) {
            continue;
        }
        let key = crate::images::key(&im.data);
        match crate::images::texture(ui.ctx(), key, &im.data, false) {
            Some(t) => {
                // Pictures turn with the page.
                let sub = PageMap { rect: r, z: 1.0, w: r.width(), h: r.height(), turn: map.turn };
                let sub = if map.turn % 2 == 1 { PageMap { w: r.height(), h: r.width(), ..sub } } else { sub };
                sub.paint_texture(painter, t.id());
            }
            None => {
                painter.rect_filled(r, 0.0, Color32::from_gray(230));
            }
        }
    }
    // Each character where the file puts it (the fonts are the nearest
    // ones, so their widths differ: placed one by one, letters keep their
    // places and selections and marks fall on them), its baseline on the
    // file's.
    let angle = map.turn as f32 * std::f32::consts::FRAC_PI_2;
    for c in &p.chars {
        if c.ch.is_whitespace() || c.hidden || c.size * z < 1.0 {
            continue;
        }
        let base = at(c.x, c.y);
        if !painter.clip_rect().expand(c.size * z * 4.0).contains(base) {
            continue;
        }
        let style = p.fonts.get(c.font as usize);
        let (mut fid, _) = crate::fonts::font_for(style.map(|s| &*s.family), style.is_some_and(|s| s.bold), style.is_some_and(|s| s.italic), (c.size * z).max(1.0).min(400.0));
        // A family loaded for another context is not this one's (tests).
        if !known.contains(&fid.family) {
            fid = egui::FontId::proportional(fid.size);
        }
        let galley = painter.layout_no_wrap(c.ch.to_string(), fid, color(c.color));
        // From the galley's top to its baseline, in page points.
        let asc = galley.rows.first().and_then(|r| r.glyphs.first().map(|g| r.pos.y + g.pos.y)).unwrap_or(0.78 * c.size * z) / z;
        let pos = at(c.x, c.y - asc);
        if map.turn == 0 {
            painter.galley(pos, galley, color(c.color));
        } else {
            painter.add(egui::epaint::TextShape::new(pos, galley, color(c.color)).with_angle(angle));
        }
    }
}

/// Paints the annotations as marks (pdfium draws their own appearances).
pub fn paint_annots(painter: &egui::Painter, annots: &[Annot], map: &PageMap) {
    for a in annots {
        if a.hidden || a.reply_to.is_some() {
            continue;
        }
        let c = color(a.color);
        let stroke = Stroke::new((a.width * map.z).max(1.0), c);
        match a.kind {
            Kind::Highlight => {
                for q in &a.quads {
                    painter.rect_filled(map.rect_to_screen(*q), 0.0, Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 90));
                }
            }
            Kind::Underline | Kind::StrikeOut | Kind::Squiggly => {
                for q in &a.quads {
                    let y = if a.kind == Kind::StrikeOut { (q[1] + q[3]) / 2.0 } else { q[3] - 1.0 };
                    painter.line_segment([map.to_screen(q[0], y), map.to_screen(q[2], y)], Stroke::new(1.5, c));
                }
            }
            Kind::Note => {
                let r = map.rect_to_screen(a.rect);
                painter.rect(r, 2.0, c, Stroke::new(1.0, Color32::from_gray(60)), egui::StrokeKind::Inside);
            }
            Kind::Ink => {
                for s in &a.ink {
                    painter.add(egui::Shape::line(s.iter().map(|p| map.to_screen(p[0], p[1])).collect(), stroke));
                }
            }
            Kind::Line | Kind::Arrow => {
                if let Some(l) = a.line {
                    let (from, to) = (map.to_screen(l[0], l[1]), map.to_screen(l[2], l[3]));
                    painter.line_segment([from, to], stroke);
                    if a.kind == Kind::Arrow {
                        // An open head at the end, as written in the file.
                        let len = (a.width * 3.0 + 6.0) * map.z;
                        let back = (from - to).normalized();
                        for d in [0.45f32, -0.45] {
                            let (s, co) = d.sin_cos();
                            let v = Vec2::new(back.x * co - back.y * s, back.x * s + back.y * co);
                            painter.line_segment([to, to + v * len], stroke);
                        }
                    }
                }
            }
            Kind::Stamp => {
                // A rounded frame with the words in bold capitals, as large
                // as fit (as written in the file).
                let r = map.rect_to_screen(a.rect);
                let bw = (r.height() * 0.06).max(2.5 * map.z);
                painter.rect_stroke(r.shrink(bw / 2.0), r.height() * 0.18, Stroke::new(bw, c), egui::StrokeKind::Middle);
                let text = if a.stamp.is_empty() { "DRAFT".to_string() } else { a.stamp.to_uppercase() };
                let mut size = (r.height() * 0.55).max(4.0).min(400.0);
                let font = |size: f32| {
                    let f = crate::fonts::font_for(None, true, false, size).0;
                    // A family loaded for another context is not this one's (tests).
                    if painter.ctx().fonts(|x| x.definitions().families.contains_key(&f.family)) { f } else { egui::FontId::proportional(size) }
                };
                let w = painter.layout_no_wrap(text.clone(), font(size), c).size().x;
                if w > r.width() - 4.0 * bw {
                    size *= (r.width() - 4.0 * bw) / w;
                }
                painter.text(r.center(), Align2::CENTER_CENTER, text, font(size), c);
            }
            Kind::Square | Kind::FreeText | Kind::Redact => {
                let r = map.rect_to_screen(a.rect);
                if let Some(f) = a.fill {
                    painter.rect_filled(r, 0.0, color(f));
                }
                painter.rect_stroke(r, 0.0, stroke, egui::StrokeKind::Inside);
                let text = if a.kind == Kind::FreeText { a.contents.clone() } else { String::new() };
                if !text.is_empty() {
                    let size = (a.font_size * map.z).max(4.0).min(200.0);
                    painter.text(r.min + Vec2::splat(3.0), Align2::LEFT_TOP, text, egui::FontId::proportional(size), c);
                }
            }
            Kind::Circle => {
                let r = map.rect_to_screen(a.rect);
                painter.add(egui::Shape::ellipse_stroke(r.center(), r.size() / 2.0, stroke));
            }
            _ => {}
        }
    }
}
