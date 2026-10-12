//! Character effects the text layout can't draw: raised and lowered
//! text, width scale, underline styles, double strike and the mark under
//! hidden text. They're applied to the laid out galley's meshes, so the
//! screen, printing and headers all show them.

use std::sync::Arc;

use apex_doc::{CharFx, UnderlineStyle};
use eframe::egui::epaint::Mesh;
use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{self, Color32, Galley, Rect, pos2};

/// What a run of characters (galley char indices `from..to`) needs.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Deco {
    pub from: usize,
    pub to: usize,
    /// Vertical move in pixels (negative raises).
    pub dy: f32,
    pub scale: f32,
    pub underline: Option<(UnderlineStyle, Color32)>,
    pub double_strike: Option<Color32>,
    /// Dotted mark under hidden text shown with the formatting marks.
    pub hidden_mark: bool,
    /// Small caps: the smaller capitals move down to the baseline.
    pub small_caps: bool,
    /// Line width in pixels.
    pub width: f32,
}

impl Deco {
    /// The decoration of a run with effects `fx`; none when the layout
    /// draws everything. `underline` is whether the run is underlined.
    pub fn of(fx: &CharFx, underline: bool, color: Color32, width: f32, pt: f32, marks: bool) -> Option<Deco> {
        let custom_underline = underline && fx.underline != UnderlineStyle::Single;
        let d = Deco {
            from: 0,
            to: 0,
            dy: -(fx.position as f32) / 2.0 * pt,
            scale: fx.scale(),
            underline: custom_underline.then(|| (fx.underline, fx.underline_color.map_or(color, crate::theme::rgb))),
            double_strike: fx.double_strike.then_some(color),
            hidden_mark: fx.hidden && marks,
            small_caps: fx.small_caps && !fx.all_caps,
            width,
        };
        (d.dy != 0.0 || d.scale != 1.0 || d.underline.is_some() || d.double_strike.is_some() || d.hidden_mark || d.small_caps).then_some(d)
    }
}

/// Text in capitals for all caps and small caps, one char for one so
/// offsets stay.
pub(super) fn caps(t: &str) -> String {
    t.chars().map(|c| {
        let mut u = c.to_uppercase();
        match (u.next(), u.next()) {
            (Some(x), None) => x,
            _ => c,
        }
    }).collect()
}

/// Splits text into runs of lowercase letters and the rest (small caps
/// show the first ones as smaller capitals).
pub(super) fn small_caps_parts(t: &str) -> Vec<(&str, bool)> {
    let mut out: Vec<(&str, bool)> = Vec::new();
    let mut start = 0;
    let mut cur = None;
    for (i, c) in t.char_indices() {
        let lower = c.is_lowercase();
        if cur.is_some_and(|l| l != lower) {
            out.push((&t[start..i], cur.unwrap_or(false)));
            start = i;
        }
        cur = Some(lower);
    }
    if start < t.len() {
        out.push((&t[start..], cur.unwrap_or(false)));
    }
    out
}

/// Appends text with effects to a job: capitals, smaller capitals for
/// small caps, and room for glyphs stretched by the width scale.
pub(super) fn append(ctx: &egui::Context, job: &mut LayoutJob, t: &str, lead: f32, fmt: TextFormat, fx: &CharFx) {
    let parts = if fx.small_caps && !fx.all_caps { small_caps_parts(t) } else { vec![(t, false)] };
    let mut lead = lead;
    let scale = fx.scale();
    for (part, lower) in parts {
        let mut f = fmt.clone();
        if lower {
            f.font_id.size *= 0.8;
        }
        let text = if fx.all_caps || fx.small_caps { caps(part) } else { part.to_string() };
        if scale == 1.0 {
            job.append(&text, lead, f);
            lead = 0.0;
            continue;
        }
        // Each glyph gets the extra advance its stretched width needs.
        let base = f.extra_letter_spacing;
        for (i, c) in text.char_indices() {
            let w = ctx.fonts_mut(|fs| fs.glyph_width(&f.font_id, c));
            let mut g = f.clone();
            g.extra_letter_spacing = base + w * (scale - 1.0);
            job.append(&text[i..i + c.len_utf8()], lead, g);
            lead = 0.0;
        }
    }
}

/// Applies `decos` to a galley's meshes.
pub(super) fn apply(galley: Arc<Galley>, decos: &[Deco]) -> Arc<Galley> {
    if decos.is_empty() {
        return galley;
    }
    let mut g = (*galley).clone();
    let mut start = 0usize;
    for prow in &mut g.rows {
        let n = prow.row.glyphs.len();
        let end = start + n;
        let first = start;
        start = end + usize::from(prow.ends_with_newline);
        if !decos.iter().any(|d| d.from < end && d.to > first) {
            continue;
        }
        let row = Arc::make_mut(&mut prow.row);
        let glyph_end = row.visuals.glyph_vertex_range.end;
        let deco_of = |ci: usize| decos.iter().position(|d| d.from <= ci && ci < d.to);
        // Smaller capitals sit on the baseline of the full size ones (or
        // of the row).
        let row_base = row.glyphs.iter().map(|g| g.pos.y).fold(f32::MIN, f32::max);
        for j in 0..n {
            let Some(di) = deco_of(first + j) else { continue };
            if !decos[di].small_caps {
                continue;
            }
            let run_base = (0..n).filter(|&k| deco_of(first + k) == Some(di) && row.glyphs[k].font_height > row.glyphs[j].font_height + 0.1).map(|k| row.glyphs[k].pos.y).fold(f32::MIN, f32::max);
            let base = if run_base > f32::MIN { run_base } else { row_base };
            let dy = base - row.glyphs[j].pos.y;
            if dy.abs() > 0.01 {
                let v0 = row.glyphs[j].first_vertex as usize;
                let v1 = row.glyphs.get(j + 1).map_or(glyph_end, |x| x.first_vertex as usize).clamp(v0, glyph_end.max(v0));
                for v in &mut row.visuals.mesh.vertices[v0..v1] {
                    v.pos.y += dy;
                }
                row.glyphs[j].pos.y += dy;
            }
        }
        // Glyphs move and stretch.
        for j in 0..n {
            let Some(di) = deco_of(first + j) else { continue };
            let d = &decos[di];
            let gl = &row.glyphs[j];
            let v0 = gl.first_vertex as usize;
            let v1 = row.glyphs.get(j + 1).map_or(glyph_end, |x| x.first_vertex as usize).max(v0);
            let gx = gl.pos.x;
            for v in &mut row.visuals.mesh.vertices[v0..v1.min(glyph_end)] {
                v.pos.y += d.dy;
                if d.scale != 1.0 {
                    v.pos.x = gx + (v.pos.x - gx) * d.scale;
                }
            }
        }
        // Lines under and through runs of glyphs with the same decoration.
        let mut j = 0;
        while j < n {
            let Some(di) = deco_of(first + j) else {
                j += 1;
                continue;
            };
            let d = &decos[di];
            let mut k = j + 1;
            while k < n && deco_of(first + k) == Some(di) {
                k += 1;
            }
            let glyphs = &row.glyphs[j..k];
            let mesh = &mut row.visuals.mesh;
            let g0 = &glyphs[0];
            let base = g0.pos.y + d.dy;
            let descent = (g0.font_height - g0.font_ascent).max(1.0);
            let under_y = base + descent * 0.45;
            let x0 = g0.pos.x;
            let x1 = glyphs.last().map_or(x0, |g| g.pos.x + g.advance_width);
            let w = d.width;
            if let Some((style, color)) = d.underline {
                if style == UnderlineStyle::Words {
                    // Under each word, not the spaces.
                    let mut a = None;
                    for g in glyphs {
                        if g.chr.is_whitespace() {
                            if let Some(ax) = a.take() {
                                line(mesh, ax, g.pos.x, under_y, w, color);
                            }
                        } else if a.is_none() {
                            a = Some(g.pos.x);
                        }
                    }
                    if let Some(ax) = a {
                        line(mesh, ax, x1, under_y, w, color);
                    }
                } else {
                    styled_line(mesh, style, x0, x1, under_y, w, color);
                }
            }
            if let Some(color) = d.double_strike {
                let mid = base - g0.font_ascent * 0.3;
                line(mesh, x0, x1, mid - w, w, color);
                line(mesh, x0, x1, mid + w, w, color);
            }
            if d.hidden_mark {
                styled_line(mesh, UnderlineStyle::Dotted, x0, x1, under_y + w, w.max(1.0), Color32::from_gray(120));
            }
            j = k;
        }
        row.visuals.mesh_bounds = row.visuals.mesh.calc_bounds();
    }
    g.num_vertices = g.rows.iter().map(|r| r.row.visuals.mesh.vertices.len()).sum();
    g.num_indices = g.rows.iter().map(|r| r.row.visuals.mesh.indices.len()).sum();
    g.mesh_bounds = g.rows.iter().fold(Rect::NOTHING, |b, r| b.union(r.row.visuals.mesh_bounds.translate(r.pos.to_vec2())));
    Arc::new(g)
}

fn line(mesh: &mut Mesh, x0: f32, x1: f32, y: f32, w: f32, color: Color32) {
    if x1 > x0 {
        mesh.add_colored_rect(Rect::from_min_max(pos2(x0, y - w / 2.0), pos2(x1, y + w / 2.0)), color);
    }
}

/// A line in one of the underline styles.
fn styled_line(mesh: &mut Mesh, style: UnderlineStyle, x0: f32, x1: f32, y: f32, w: f32, color: Color32) {
    // On and off lengths of dashed patterns.
    let pattern: &[f32] = match style {
        UnderlineStyle::Double => {
            line(mesh, x0, x1, y - w, w, color);
            line(mesh, x0, x1, y + w, w, color);
            return;
        }
        UnderlineStyle::Thick => {
            line(mesh, x0, x1, y + w / 2.0, w * 2.0, color);
            return;
        }
        UnderlineStyle::Wave => {
            let step = w.max(1.0);
            let (amp, period) = (w * 1.2, w * 5.0);
            let mut x = x0;
            while x < x1 {
                let yy = y + amp * (x / period * std::f32::consts::TAU).sin();
                line(mesh, x, (x + step).min(x1), yy, w, color);
                x += step;
            }
            return;
        }
        UnderlineStyle::Dotted => &[1.0, 1.5],
        UnderlineStyle::Dashed => &[4.0, 2.5],
        UnderlineStyle::DotDash => &[4.0, 2.0, 1.0, 2.0],
        UnderlineStyle::Single | UnderlineStyle::Words => {
            line(mesh, x0, x1, y, w, color);
            return;
        }
    };
    let mut x = x0;
    let mut i = 0;
    while x < x1 {
        let len = pattern[i % pattern.len()] * w.max(1.0);
        if i % 2 == 0 {
            line(mesh, x, (x + len).min(x1), y, w, color);
        }
        x += len;
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_keep_offsets() {
        assert_eq!(caps("straße ñ"), "STRAßE Ñ");
        assert_eq!(small_caps_parts("Hola MUNDO"), vec![("H", false), ("ola", true), (" MUNDO", false)]);
    }
}
