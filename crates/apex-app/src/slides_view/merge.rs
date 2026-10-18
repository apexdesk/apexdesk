//! Shape Format > Merge Shapes: the selected shapes' outlines, or their
//! text's (each letter's outline from its font), merged into drawn shapes.

use apex_slides::combine::{self, Merge, Outline};
use apex_slides::{Chain, Fill, Frame, ObjKind, Object, SColor, edit};
use eframe::egui::{self, Painter, Rect, pos2, vec2};
use skrifa::MetadataProvider;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};

use super::SlidesView;
use super::objects::chain;
use super::text::{self, Over};

/// Text is laid out this many pixels per point, for the letters' places
/// not to be rounded to whole points.
const SCALE: f32 = 4.0;

/// Whether `o` stands for its text: a text box, or a shape with neither
/// fill nor outline, holding text.
fn is_text(ch: &Chain, o: &Object) -> bool {
    let ObjKind::Shape { text: Some(t), text_box, .. } = &o.kind else { return false };
    !t.is_empty() && (*text_box || ch.fill(o).is_none() && ch.line(o).is_none())
}

impl SlidesView {
    /// The selected objects Merge Shapes takes, in the order they were
    /// selected: shapes with an area and text boxes.
    fn mergeable(&self) -> Vec<&Object> {
        let Some(ch) = chain(&self.pres.deck, self.target()) else { return Vec::new() };
        let objs = self.cur_objects();
        self.selection()
            .into_iter()
            .filter_map(|id| objs.iter().find(|o| o.id == id))
            .filter(|o| matches!(o.kind, ObjKind::Shape { connector: false, .. }) && (is_text(&ch, o) || !ch.geometry(o).is_line()))
            .collect()
    }

    /// Two shapes to merge, or text to make a shape of.
    pub fn can_merge(&self) -> bool {
        let m = self.mergeable();
        m.len() >= 2 || m.len() == 1 && chain(&self.pres.deck, self.target()).is_some_and(|ch| is_text(&ch, m[0]))
    }

    /// Merges the selection, as one step. A text box alone becomes the
    /// shape of its letters (with Union).
    pub fn merge_selected(&mut self, p: &Painter, how: Merge) -> bool {
        let Some(ch) = chain(&self.pres.deck, self.target()) else { return false };
        // (id, outline, the color of its letters when it stands for text).
        let picked: Vec<(u32, Outline, Option<egui::Color32>)> = self
            .mergeable()
            .into_iter()
            .map(|o| {
                let f = ch.frame(o);
                if is_text(&ch, o) {
                    let (outline, color) = text_outline(p, &ch, o, &f);
                    (o.id, outline, Some(color))
                } else {
                    (o.id, combine::outline(&ch.geometry(o), &f), None)
                }
            })
            .collect();
        let Some(&(first, _, first_text)) = picked.first() else { return false };
        if picked.len() < 2 && first_text.is_none() {
            return false;
        }
        let mut v = self.cur_objects().to_vec();
        if let Some(o) = v.iter_mut().find(|o| o.id == first) {
            if let Some(c) = first_text {
                // Letters take their color as fill; the text is gone.
                o.sp.fill = Some(Fill::Solid(SColor::rgb(u32::from(c.r()) << 16 | u32::from(c.g()) << 8 | u32::from(c.b()))));
                o.sp.line = Some(apex_slides::Line { fill: Some(Fill::None), ..Default::default() });
                o.kind = ObjKind::Shape { text: None, text_box: false, connector: false };
            } else if o.ph.is_some() {
                // A placeholder's look comes from its layout, which the new
                // shape doesn't follow: kept as it shows.
                o.sp.fill = Some(ch.fill(o).unwrap_or(Fill::None));
                o.sp.line = ch.line(o);
            }
        }
        let outlines: Vec<(u32, Outline)> = picked.into_iter().map(|(id, o, _)| (id, o)).collect();
        let made = edit::merge_in(&mut v, &outlines, how);
        if made.is_empty() {
            return false;
        }
        let Some(objs) = self.edit_objects("slides.undo.merge") else { return false };
        *objs = v;
        self.typing = None;
        self.sel = made;
        true
    }
}

/// Collects a glyph's contours, flattened, placed by `at`.
struct Pen<'a> {
    at: &'a dyn Fn(f32, f32) -> [f64; 2],
    contours: Vec<Vec<[f64; 2]>>,
    last: (f32, f32),
}

impl Pen<'_> {
    fn point(&mut self, x: f32, y: f32) {
        if let Some(c) = self.contours.last_mut() {
            c.push((self.at)(x, y));
        }
        self.last = (x, y);
    }
}

impl OutlinePen for Pen<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.contours.push(Vec::new());
        self.point(x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.point(x, y);
    }

    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let (x0, y0) = self.last;
        for k in 1..=8 {
            let t = k as f32 / 8.0;
            let u = 1.0 - t;
            self.point(u * u * x0 + 2.0 * u * t * cx + t * t * x, u * u * y0 + 2.0 * u * t * cy + t * t * y);
        }
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let (x0, y0) = self.last;
        for k in 1..=12 {
            let t = k as f32 / 12.0;
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            self.point(a * x0 + b * cx0 + c * cx1 + d * x, a * y0 + b * cy0 + c * cy1 + d * y);
        }
    }

    fn close(&mut self) {}
}

/// The outline of the letters of `o`'s text, laid out as it shows in
/// frame `f`, in slide points, and the color of the first letter.
fn text_outline(p: &Painter, ch: &Chain, o: &Object, f: &Frame) -> (Outline, egui::Color32) {
    let mut color = None;
    let ObjKind::Shape { text: Some(t), .. } = &o.kind else { return (Vec::new(), egui::Color32::BLACK) };
    let t = text::with_fields(ch, t);
    let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(f.w * SCALE, f.h * SCALE));
    let placed = text::place_body(p, ch, o, &t, area, 0.0, area.center(), SCALE, Over::default());
    // Layout pixels to slide points, turned with the shape.
    let (cx, cy) = (f64::from(f.x + f.w / 2.0), f64::from(f.y + f.h / 2.0));
    let (sin, cos) = f64::from(f.rot).to_radians().sin_cos();
    let to_slide = |q: egui::Pos2| {
        let (dx, dy) = (f64::from(q.x / SCALE - f.w / 2.0), f64::from(q.y / SCALE - f.h / 2.0));
        [cx + dx * cos - dy * sin, cy + dx * sin + dy * cos]
    };
    let mut contours = Vec::new();
    for l in &placed.laid.lines {
        let g = &l.galley;
        let sections = &g.job.sections;
        let bytes: Vec<usize> = g.job.text.char_indices().map(|(b, _)| b).collect();
        let mut ci = 0usize;
        for row in &g.rows {
            for glyph in &row.row.glyphs {
                let b = bytes.get(ci).copied().unwrap_or(g.job.text.len());
                ci += 1;
                if glyph.chr.is_whitespace() {
                    continue;
                }
                let Some(sec) = sections.iter().find(|s| (s.byte_range.start.0..s.byte_range.end.0).contains(&b)) else { continue };
                let font = &sec.format.font_id;
                color.get_or_insert(sec.format.color);
                let base = vec2(l.x, l.y) + row.pos.to_vec2() + glyph.pos.to_vec2();
                let skew = if sec.format.italics { 0.2 } else { 0.0 };
                let at = |x: f32, y: f32| to_slide(placed.screen(base + vec2(x + y * skew, -y)));
                for (_, data) in crate::fonts::chain(&font.family) {
                    let Ok(face) = skrifa::FontRef::from_index(&data.font, data.index) else { continue };
                    let Some(gid) = face.charmap().map(glyph.chr) else { continue };
                    let Some(outline) = face.outline_glyphs().get(gid) else { continue };
                    let mut pen = Pen { at: &at, contours: Vec::new(), last: (0.0, 0.0) };
                    if outline.draw(DrawSettings::unhinted(Size::new(font.size), LocationRef::default()), &mut pen).is_ok() {
                        contours.extend(pen.contours.into_iter().filter(|c| c.len() > 2));
                    }
                    break;
                }
            }
            if row.ends_with_newline {
                ci += 1;
            }
        }
    }
    (combine::filled(contours), color.unwrap_or(egui::Color32::BLACK))
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_slides::{Geometry, TextBody};

    fn view() -> SlidesView {
        let mut v = SlidesView::new(apex_slides::build::new_deck(0));
        v.set_layout(0, 5);
        v
    }

    /// Runs `f` with a painter whose fonts are ApexDesk's.
    fn painting(f: impl FnOnce(&Painter)) {
        // A context of its own: on the shared one, other tests add font
        // families that are bound only from the next frame.
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        for _ in 0..2 {
            ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        }
        let mut f = Some(f);
        ctx.run_ui(Default::default(), |ui| {
            if let Some(f) = f.take() {
                f(ui.painter());
            }
        })
        .textures_delta
        .clear();
    }

    fn drawn(v: &SlidesView, id: u32) -> Outline {
        let o = v.cur_objects().iter().find(|o| o.id == id).unwrap();
        let Some(g @ Geometry::Custom(_)) = &o.sp.geom else { panic!("a drawn shape") };
        combine::outline(g, &o.sp.frame.unwrap())
    }

    #[test]
    fn shapes_merge_in_one_step() {
        let mut v = view();
        let a = v.insert_shape("rect", Frame::new(0.0, 0.0, 100.0, 100.0), false).unwrap();
        let b = v.insert_shape("ellipse", Frame::new(50.0, 50.0, 100.0, 100.0), false).unwrap();
        let line = v.insert_shape("line", Frame::new(0.0, 0.0, 10.0, 10.0), false).unwrap();
        let n = v.cur_objects().len();
        v.sel = vec![line, a];
        assert!(!v.can_merge(), "a line has no area");
        v.sel = vec![a, b];
        assert!(v.can_merge());
        painting(|p| assert!(v.merge_selected(p, Merge::Fragment)));
        assert_eq!(v.cur_objects().len(), n - 2 + 3);
        v.undo();
        assert_eq!(v.cur_objects().len(), n);
        v.sel = vec![a, b];
        painting(|p| assert!(v.merge_selected(p, Merge::Subtract)));
        let made = v.selection();
        assert_eq!(made.len(), 1);
        // The square less a quarter of the circle.
        let area: f64 = drawn(&v, made[0]).iter().map(|c| (0..c.len()).map(|i| c[i][0] * c[(i + 1) % c.len()][1] - c[(i + 1) % c.len()][0] * c[i][1]).sum::<f64>() / 2.0).sum::<f64>().abs();
        assert!((area - (10_000.0 - std::f64::consts::PI * 2500.0 / 4.0)).abs() < 60.0, "{area}");
    }

    #[test]
    fn text_becomes_its_letters() {
        let mut v = view();
        let id = v.cur_objects().iter().map(Object::max_id).max().unwrap_or(1) + 1;
        v.edit_objects("slides.undo.insert_text_box").unwrap().push(Object::text_box(id, Frame::new(10.0, 10.0, 300.0, 80.0), TextBody::plain("O")));
        v.sel = vec![id];
        assert!(v.can_merge());
        painting(|p| assert!(v.merge_selected(p, Merge::Union)));
        let made = v.selection();
        let o = drawn(&v, made[0]);
        // An O: its outside and its hole.
        assert_eq!(o.len(), 2, "{o:?}");
        let o = v.cur_objects().iter().find(|o| o.id == made[0]).unwrap();
        assert!(matches!(o.sp.fill, Some(Fill::Solid(_))));
        assert!(matches!(o.kind, ObjKind::Shape { text: None, .. }));
    }
}
