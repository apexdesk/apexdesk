//! The line numbers of the code view. They are placed from the rows of
//! the laid out code itself (not counted as rows of one height), so each
//! number sits on its line whatever the fonts, the zoom or the scroll; a
//! row that only continues a wrapped line has none.

use eframe::egui::{self, Align2, Color32, FontId, Galley, Pos2, Rect, pos2, vec2};

/// Space between the numbers and the edges of their column.
const PAD: f32 = 6.0;

/// The rows of `galley` that start a line: the line number (from 1) and
/// the row's top, relative to the galley.
pub(super) fn line_rows(galley: &Galley) -> Vec<(usize, f32)> {
    let mut out = Vec::new();
    let mut starts = true;
    for row in &galley.rows {
        if starts {
            out.push((out.len() + 1, row.rect().min.y));
        }
        starts = row.ends_with_newline;
    }
    out
}

/// Where line `line` (from 0) is in `galley` (relative to it): its first
/// row, or the last row past the end.
pub(super) fn line_rect(galley: &Galley, line: usize) -> Rect {
    let mut n = 0;
    let mut starts = true;
    for row in &galley.rows {
        if starts {
            if n == line {
                return row.rect();
            }
            n += 1;
        }
        starts = row.ends_with_newline;
    }
    galley.rows.last().map_or(Rect::from_min_size(Pos2::ZERO, vec2(0.0, galley.size().y)), |r| r.rect())
}

/// The width of the column for the numbers of `lines` lines.
pub(super) fn width(ui: &egui::Ui, font: &FontId, lines: usize) -> f32 {
    let digits = lines.to_string().len().max(2);
    let w = ui.fonts_mut(|f| f.glyph_width(font, '0'));
    digits as f32 * w + 2.0 * PAD
}

/// Paints the numbers of the lines of `galley` (shown at `pos`, visible in
/// `clip`) right-aligned in the column `rect`.
pub(super) fn paint(ui: &egui::Ui, rect: Rect, galley: &Galley, pos: Pos2, clip: Rect, font: &FontId, color: Color32) {
    let visible = Rect::from_x_y_ranges(rect.x_range(), clip.y_range());
    let painter = ui.painter_at(visible);
    for (n, y) in line_rows(galley) {
        let y = pos.y + y;
        if y > visible.max.y {
            break;
        }
        if y + font.size * 2.0 < visible.min.y {
            continue;
        }
        painter.text(pos2(rect.max.x - PAD, y), Align2::RIGHT_TOP, n.to_string(), font.clone(), color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::text::LayoutJob;

    #[test]
    fn wrapped_rows_have_no_number() {
        let ctx = egui::Context::default();
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        let job = LayoutJob::simple("uno\n\ndos tres cuatro cinco seis siete ocho\n😀 中文".into(), FontId::monospace(12.0), Color32::WHITE, 60.0);
        let galley = ctx.fonts_mut(|f| f.layout_job(job));
        assert!(galley.rows.len() > 5, "the long line wraps");
        let lines = line_rows(&galley);
        assert_eq!(lines.iter().map(|l| l.0).collect::<Vec<_>>(), [1, 2, 3, 4]);
        // Line 3 starts at the third row; line 4 after its wrapped rows.
        assert_eq!(lines[2].1, galley.rows[2].rect().min.y);
        let last = galley.rows.last().unwrap().rect().min.y;
        assert_eq!(lines[3].1, last);
        for (i, (_, y)) in lines.iter().enumerate() {
            assert_eq!(line_rect(&galley, i).min.y, *y);
        }
        assert_eq!(line_rect(&galley, 99).min.y, last);
    }
}
