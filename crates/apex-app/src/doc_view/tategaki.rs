//! Vertical pages (tategaki). The text of a vertical section is laid out
//! and paginated on its page turned a quarter to the left (the section's
//! `flow_setup`): lines are rows of a galley whose Asian characters stand
//! upright once turned (`LayoutJob::vertical`), and tables, notes and
//! column rules are placed as on any page. Painting turns the page's
//! shapes back a quarter clockwise about its text area, so the lines run
//! down and follow each other from right to left; pictures are turned
//! back upright. Headers, footers and floating objects are drawn on the
//! page as it is, like Word does.
//!
//! "Flow" coordinates are the canvas coordinates of the text as laid out:
//! the text area of a vertical page starts at the same canvas point as on
//! screen, with x along the lines and y across them.

use std::f32::consts::FRAC_PI_2;
use std::sync::Arc;

use eframe::egui::epaint::text::{EM_BOX_MIDDLE, Galley};
use eframe::egui::epaint::{ClippedShape, Mesh, Shape};
use eframe::egui::layers::ShapeIdx;
use eframe::egui::{Color32, Key, Painter, Pos2, Rect, TextureId, Vec2, pos2};

use super::DocView;

/// The turn of a vertical page's text area: `o` is its top-left corner
/// (the same in flow and canvas coordinates), `w` its width on the page.
#[derive(Clone, Copy, Debug)]
pub(super) struct Turn {
    pub o: Pos2,
    pub w: f32,
}

impl Turn {
    /// Canvas point of a flow point.
    pub fn pt(&self, p: Pos2) -> Pos2 {
        pos2(self.o.x + self.w - (p.y - self.o.y), self.o.y + (p.x - self.o.x))
    }

    /// Flow point of a canvas point.
    pub fn back(&self, q: Pos2) -> Pos2 {
        pos2(self.o.x + (q.y - self.o.y), self.o.y + self.w - (q.x - self.o.x))
    }

    pub fn rect(&self, r: Rect) -> Rect {
        Rect::from_two_pos(self.pt(r.min), self.pt(r.max))
    }

    pub fn offset(self, v: Vec2) -> Turn {
        Turn { o: self.o + v, ..self }
    }
}

/// Where the next shape painted with `p` goes.
pub(super) fn mark(p: &Painter) -> ShapeIdx {
    p.ctx().graphics_mut(|g| g.entry(p.layer_id()).next_idx())
}

/// A painter for the text of a vertical page: not clipped (its clip rect
/// would be turned with what it paints; `with_clip_rect` can't widen it).
pub(super) fn unclipped(p: &Painter) -> Painter {
    let mut q = p.clone();
    q.set_clip_rect(Rect::EVERYTHING);
    q
}

/// Turns the shapes painted with `p` since `from`, clipped to `clip`.
pub(super) fn turn_painted(p: &Painter, from: ShapeIdx, t: &Turn, clip: Rect) {
    p.ctx().graphics_mut(|g| {
        let list = g.entry(p.layer_id());
        let end = list.next_idx().0;
        for i in from.0..end {
            list.mutate_shape(ShapeIdx(i), |c| turn_clipped(c, t, clip));
        }
    });
}

fn turn_clipped(c: &mut ClippedShape, t: &Turn, clip: Rect) {
    c.clip_rect = if c.clip_rect.is_finite() { t.rect(c.clip_rect).intersect(clip) } else { clip };
    turn_shape(&mut c.shape, t);
}

/// Turns a shape a quarter clockwise; pictures stay upright, filling the
/// turned rect (vertical text lays them out with their sides swapped).
pub(super) fn turn_shape(s: &mut Shape, t: &Turn) {
    match s {
        Shape::Noop | Shape::Callback(_) => {}
        Shape::Vec(v) => v.iter_mut().for_each(|s| turn_shape(s, t)),
        Shape::Circle(c) => c.center = t.pt(c.center),
        Shape::Ellipse(e) => {
            e.center = t.pt(e.center);
            e.radius = Vec2::new(e.radius.y, e.radius.x);
        }
        Shape::LineSegment { points, .. } => points.iter_mut().for_each(|p| *p = t.pt(*p)),
        Shape::Path(ps) => ps.points.iter_mut().for_each(|p| *p = t.pt(*p)),
        Shape::Rect(r) => r.rect = t.rect(r.rect),
        Shape::Text(x) => {
            x.pos = t.pt(x.pos);
            x.angle += FRAC_PI_2;
        }
        Shape::Mesh(m) => turn_mesh(Arc::make_mut(m), t),
        Shape::QuadraticBezier(b) => b.points.iter_mut().for_each(|p| *p = t.pt(*p)),
        Shape::CubicBezier(b) => b.points.iter_mut().for_each(|p| *p = t.pt(*p)),
    }
}

fn turn_mesh(m: &mut Mesh, t: &Turn) {
    if m.texture_id == TextureId::default() {
        m.vertices.iter_mut().for_each(|v| v.pos = t.pt(v.pos));
        return;
    }
    // A picture: moved and stretched to the turned rect, not turned.
    let from = Rect::from_points(&m.vertices.iter().map(|v| v.pos).collect::<Vec<_>>());
    let to = t.rect(from);
    let scale = Vec2::new(to.width() / from.width().max(1e-3), to.height() / from.height().max(1e-3));
    for v in &mut m.vertices {
        v.pos = to.min + (v.pos - from.min) * scale;
    }
}

/// Centers the characters of each row of a vertical galley across it
/// (the middle of their em box on the middle of the row), as Word and
/// LibreOffice center vertical lines in their line pitch.
pub(super) fn center_rows(galley: Arc<Galley>) -> Arc<Galley> {
    let job = &galley.job;
    let bytes: Vec<usize> = job.text.char_indices().map(|(b, _)| b).collect();
    // The size of a char's text (0 for what isn't drawn: the room kept
    // for pictures and objects).
    let size_at = |ci: usize| {
        let b = bytes.get(ci).copied().unwrap_or(job.text.len());
        job.sections.iter().find(|s| s.byte_range.start.0 <= b && b < s.byte_range.end.0).filter(|s| s.format.color != Color32::TRANSPARENT).map_or(0.0, |s| s.format.font_id.size)
    };
    let mut shifts = Vec::with_capacity(galley.rows.len());
    let mut ci = 0;
    for r in &galley.rows {
        let n = r.row.glyphs.len();
        let (mut size, mut base) = (0.0f32, f32::MIN);
        for (k, g) in r.row.glyphs.iter().enumerate() {
            let s = size_at(ci + k);
            if s > 0.0 {
                (size, base) = (size.max(s), base.max(g.pos.y));
            }
        }
        ci += n + usize::from(r.ends_with_newline);
        shifts.push(if size <= 0.0 { 0.0 } else { r.row.size.y / 2.0 - (base - EM_BOX_MIDDLE * size) });
    }
    if shifts.iter().all(|d| d.abs() < 0.01) {
        return galley;
    }
    let mut g = (*galley).clone();
    for (r, d) in g.rows.iter_mut().zip(shifts) {
        if d.abs() < 0.01 {
            continue;
        }
        let row = Arc::make_mut(&mut r.row);
        for gl in &mut row.glyphs {
            gl.pos.y += d;
        }
        for v in &mut row.visuals.mesh.vertices {
            v.pos.y += d;
        }
        row.visuals.mesh_bounds = row.visuals.mesh_bounds.translate(Vec2::new(0.0, d));
    }
    Arc::new(g)
}

/// The arrow key that does on a vertical page what `key` does on a
/// horizontal one: Up and Down go along the line, Left goes to the next
/// line and Right to the previous one.
pub(super) fn arrow(key: Key) -> Key {
    match key {
        Key::ArrowUp => Key::ArrowLeft,
        Key::ArrowDown => Key::ArrowRight,
        Key::ArrowLeft => Key::ArrowDown,
        Key::ArrowRight => Key::ArrowUp,
        k => k,
    }
}

impl DocView {
    /// The turn of page `page` (canvas coordinates) when it's vertical.
    pub(super) fn turn_of(&self, page: usize) -> Option<Turn> {
        let g = self.layout.geo_of(page);
        g.vertical.then(|| Turn { o: self.content_origin(page), w: g.content.x })
    }

    /// The page under a canvas point, and the point in flow coordinates.
    pub(super) fn flow_point(&self, local: Pos2) -> (usize, Pos2) {
        let page = self.layout.page_at(local.x - self.page_x, local.y);
        (page, self.turn_of(page).map_or(local, |t| t.back(local)))
    }

    /// The caret is on a vertical page.
    pub(super) fn caret_vertical(&self) -> bool {
        // The caret's paragraph isn't laid out (the code view shows, or
        // the document changed since the last frame): no page to follow.
        let para = self.sel.cursor.para;
        if para >= self.layout.paras.len() || self.layout.segs_of(para).is_empty() {
            return false;
        }
        self.layout.geo_of(self.caret_at(self.sel.cursor).1).vertical
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_and_back() {
        let t = Turn { o: pos2(100.0, 50.0), w: 400.0 };
        // The start of the first line is the text area's top-right corner;
        // lines run down, the next one is to the left.
        assert_eq!(t.pt(pos2(100.0, 50.0)), pos2(500.0, 50.0));
        assert_eq!(t.pt(pos2(130.0, 50.0)), pos2(500.0, 80.0));
        assert_eq!(t.pt(pos2(100.0, 70.0)), pos2(480.0, 50.0));
        let p = pos2(321.0, 77.0);
        assert!((t.back(t.pt(p)) - p).length() < 1e-4);
        let r = t.rect(Rect::from_min_max(pos2(100.0, 50.0), pos2(160.0, 70.0)));
        assert_eq!(r, Rect::from_min_max(pos2(480.0, 50.0), pos2(500.0, 110.0)));
        assert_eq!(arrow(Key::ArrowLeft), Key::ArrowDown);
        // Text turns a quarter; a picture only moves to the turned rect.
        let mut s = Shape::Rect(eframe::egui::epaint::RectShape::filled(Rect::from_min_max(pos2(100.0, 50.0), pos2(160.0, 70.0)), 0.0, eframe::egui::Color32::RED));
        turn_shape(&mut s, &t);
        assert!(matches!(s, Shape::Rect(r) if r.rect.width() == 20.0 && r.rect.height() == 60.0));
    }

    fn laid_out(doc: apex_doc::Document) -> (DocView, eframe::egui::Context) {
        let ctx = eframe::egui::Context::default();
        crate::fonts::install(&ctx);
        let frame = || ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        frame();
        frame();
        let mut v = DocView::new(doc);
        v.layout = v.build_layout(&ctx, 1.0, (1, 0, 0, 0, 0), super::super::Layout::default());
        (v, ctx)
    }

    /// Japanese paragraphs on a vertical A4 page, horizontal digits in the
    /// first one, then a horizontal section on a new page.
    fn vertical_doc() -> apex_doc::Document {
        use apex_doc::{Columns, Document, Paragraph, Pos, Section, SectionStart, Selection};
        let line = "吾輩は猫である。名前はまだ無い。どこで生れたかとんと見当がつかぬ。".repeat(3);
        let mut paras: Vec<Paragraph> = (0..30).map(|_| Paragraph::new(&line)).collect();
        paras.insert(0, Paragraph::new("平成31年の話"));
        paras.push(Paragraph::new("Horizontal again."));
        let mut doc = Document::from_paragraphs(paras);
        let page = doc.page;
        doc.page.set_vertical(true);
        let last = doc.paragraphs.len() - 1;
        doc.paragraphs[last].section = Some(Box::new(Section { headers: Default::default(), start: SectionStart::NextPage, page, columns: Columns::default() }));
        let digits = Selection { anchor: Pos::new(0, 6), cursor: Pos::new(0, 8) };
        doc.format_range(digits, &|a| a.set_asian(|x| x.combine = Some(apex_doc::Combine { vert: true, vert_compress: true, ..Default::default() })));
        doc
    }

    #[test]
    fn vertical_pages() {
        use apex_doc::Pos;
        let (v, _ctx) = laid_out(vertical_doc());
        let l = &v.layout;
        let n = v.doc.paragraphs.len();
        let last = l.segs_of(n - 1)[0].page;
        assert!(last >= 2, "{last}");
        assert!((0..last).all(|p| l.geo_of(p).vertical) && !l.geo_of(last).vertical);
        // The page keeps its size; the text runs across its height.
        let g = l.geo_of(0);
        assert!(g.size.y > g.size.x && g.content.x < g.content.y);
        // Lines run down: the caret is across the line, moving down the
        // page along it; the second line is left of the first.
        let (a, b) = (v.caret_rect(Pos::new(1, 0)), v.caret_rect(Pos::new(1, 3)));
        assert!(a.width() > a.height() && (b.center().x - a.center().x).abs() < 0.5 && b.center().y > a.center().y + 5.0, "{a:?} {b:?}");
        let row2 = l.paras[1].galley.rows[0].char_count_excluding_newline().0;
        let c = v.caret_rect(Pos::new(1, (row2 + 1) * 3));
        assert!(c.center().x < a.center().x - 5.0 && (c.center().y - a.center().y).abs() < 20.0, "{a:?} {c:?}");
        // A click finds what's under it.
        for p in [Pos::new(1, 6), Pos::new(4, 9), Pos::new(1, (row2 + 2) * 3)] {
            assert_eq!(v.pos_at(v.caret_rect(p).center() + Vec2::new(0.0, 0.2)), p);
        }
        // The horizontal section's caret is upright.
        let h = v.caret_rect(Pos::new(n - 1, 2));
        assert!(h.height() > h.width());
        // The digits are drawn upright in one cell, turned back once the
        // page turns.
        let ov = &l.paras[0].overlays;
        assert_eq!(ov.len(), 1);
        assert!((ov[0].angle + FRAC_PI_2).abs() < 1e-4);
        let cell = |i: usize| l.paras[0].galley.pos_from_cursor(eframe::egui::text::CCursor::new(i)).min.x;
        let size = l.paras[0].galley.rows[0].row.glyphs[0].font_height;
        assert!((cell(4) - cell(2) - size).abs() < size * 0.3, "{} {}", cell(4) - cell(2), size);
    }

    #[test]
    fn horizontal_views_and_keys() {
        // Draft lays vertical sections out horizontally, like Word.
        let (mut v, ctx) = laid_out(vertical_doc());
        assert!(v.layout.geo_of(0).vertical);
        v.views.mode = super::super::views::ViewMode::Draft;
        v.layout = v.build_layout(&ctx, 1.0, (1, 0, 0, 0, 0), super::super::Layout::default());
        let a = v.caret_rect(apex_doc::Pos::new(1, 0));
        assert!(a.height() > a.width());
        assert_eq!(arrow(Key::ArrowUp), Key::ArrowLeft);
        assert_eq!(arrow(Key::ArrowRight), Key::ArrowUp);
    }
}
