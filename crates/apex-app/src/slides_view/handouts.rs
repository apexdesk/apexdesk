//! Printed forms of a deck besides one slide per page: notes pages (the
//! slide above its speaker notes) and handouts (1, 2, 3, 4, 6 or 9 slides
//! a page, with lines to write on beside each slide of the 3-up), on the
//! deck's notes page size, as PowerPoint prints them. Also the slides as
//! pictures, for File > Export and `apexdesk convert --to png`.

use apex_slides::Deck;
use eframe::egui::{self, Align2, Color32, FontId, Painter, Pos2, Rect, Stroke, Vec2, pos2, vec2};

use super::render;
use crate::print::{Job, Page, Scope};
use crate::shapes::PX_PT;

/// Slides a handout page can hold.
pub const HANDOUTS: [u8; 6] = [1, 2, 3, 4, 6, 9];

/// The notes page size in pixels (portrait 7.5 x 10 in when the deck has none).
fn page_size(deck: &Deck) -> Vec2 {
    let (w, h) = deck.notes_size;
    if w > 1.0 && h > 1.0 { vec2(w, h) * PX_PT } else { vec2(540.0, 720.0) * PX_PT }
}

/// `inner` of the slide's aspect, centred in `cell`.
fn fit(deck: &Deck, cell: Rect) -> Rect {
    let aspect = deck.width.max(1.0) / deck.height.max(1.0);
    let w = cell.width().min(cell.height() * aspect);
    Rect::from_center_size(cell.center(), vec2(w, w / aspect))
}

/// The page's number in its bottom right corner.
fn page_number(p: &Painter, size: Vec2, n: usize) {
    let at = pos2(size.x - 36.0 * PX_PT, size.y - 24.0 * PX_PT);
    p.text(at, Align2::RIGHT_BOTTOM, n.to_string(), FontId::proportional(10.0 * PX_PT), Color32::from_gray(80));
}

/// The cells of a handout page of `n` slides and, for the 3-up, the areas
/// with lines beside them.
pub fn cells(size: Vec2, n: u8) -> Vec<(Rect, Option<Rect>)> {
    let m = 36.0 * PX_PT;
    let area = Rect::from_min_max(pos2(m, m), pos2(size.x - m, size.y - m));
    let (cols, rows) = match n {
        1 => (1, 1),
        2 | 3 => (1, n as usize),
        4 => (2, 2),
        6 => (2, 3),
        _ => (3, 3),
    };
    let (cw, ch) = (area.width() / cols as f32, area.height() / rows as f32);
    let mut out = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            let cell = Rect::from_min_size(area.min + vec2(c as f32 * cw, r as f32 * ch), vec2(cw, ch)).shrink(8.0 * PX_PT);
            if n == 3 {
                let (left, right) = cell.split_left_right_at_fraction(0.5);
                out.push((left, Some(right.shrink2(vec2(8.0 * PX_PT, 0.0)))));
            } else {
                out.push((cell, None));
            }
        }
    }
    out
}

/// A slide in a cell with a thin frame around it.
fn framed(p: &Painter, deck: &Deck, i: usize, cell: Rect) {
    let r = fit(deck, cell);
    render::paint_slide(p, deck, &deck.slides[i], r, render::Opts { wait: true, ..Default::default() });
    p.rect_stroke(r, 0.0, Stroke::new(0.75, Color32::from_gray(120)), egui::StrokeKind::Outside);
}

/// Ruled lines to write on.
fn lines(p: &Painter, area: Rect) {
    let gap = 24.0 * PX_PT;
    let mut y = area.top() + gap;
    while y <= area.bottom() {
        p.hline(area.x_range(), y, Stroke::new(0.5, Color32::from_gray(150)));
        y += gap;
    }
}

/// The notes page of slide `i`: the slide on the upper part, its notes below.
fn notes_page(p: &Painter, deck: &Deck, i: usize, size: Vec2) {
    let top = Rect::from_min_max(pos2(size.x * 0.125, size.y * 0.075), pos2(size.x * 0.875, size.y * 0.42));
    framed(p, deck, i, top);
    let body = Rect::from_min_max(pos2(size.x * 0.1, size.y * 0.475), pos2(size.x * 0.9, size.y * 0.92));
    let text = deck.slides[i].notes_text();
    let galley = p.layout(text, FontId::proportional(12.0 * PX_PT), Color32::BLACK, body.width());
    p.with_clip_rect(body).galley(body.min, galley, Color32::BLACK);
}

/// The pages of `deck` printed as `what` (slides for anything but notes
/// and handouts). `paint` paints on a page and returns its shapes.
pub fn pages(deck: &Deck, what: Scope, paint: &mut dyn FnMut(Vec2, &mut dyn FnMut(&Painter)) -> Page) -> Vec<Page> {
    let mut out = Vec::new();
    match what {
        Scope::Notes => {
            let size = page_size(deck);
            for i in 0..deck.slides.len() {
                out.push(paint(size, &mut |p| {
                    notes_page(p, deck, i, size);
                    page_number(p, size, i + 1);
                }));
            }
        }
        Scope::Handouts(n) => {
            let size = page_size(deck);
            let per = n.max(1) as usize;
            for (k, first) in (0..deck.slides.len()).step_by(per).enumerate() {
                out.push(paint(size, &mut |p| {
                    for (j, (cell, ruled)) in cells(size, n).into_iter().enumerate() {
                        if first + j < deck.slides.len() {
                            framed(p, deck, first + j, cell);
                            if let Some(r) = ruled {
                                lines(p, r);
                            }
                        }
                    }
                    page_number(p, size, k + 1);
                }));
            }
        }
        _ => {
            let size = vec2(deck.width, deck.height) * PX_PT;
            for s in &deck.slides {
                out.push(paint(size, &mut |p| render::paint_slide(p, deck, s, Rect::from_min_size(Pos2::ZERO, size), render::Opts { wait: true, ..Default::default() })));
            }
        }
    }
    out
}

/// The printed pages of a deck, with its pictures as textures.
pub fn job(ctx: &egui::Context, deck: &Deck, title: String, what: Scope) -> Job {
    let mut job = Job { title, author: deck.meta.author.clone(), ..Default::default() };
    job.pages = pages(deck, what, &mut |size, f| {
        let shapes = crate::print::capture(ctx, |p| f(p));
        Page { size, shapes, links: Vec::new(), tags: Vec::new() }
    });
    job.textures = super::pictures(deck).into_iter().filter_map(|b| crate::images::texture(ctx, crate::images::key(&b), &b, true)).collect();
    job
}

/// Slide `i` as a PNG `width` pixels wide.
pub fn slide_png(ctx: &egui::Context, deck: &Deck, i: usize, width: u32) -> Option<Vec<u8>> {
    let size = vec2(deck.width, deck.height) * PX_PT;
    let s = deck.slides.get(i)?;
    let shapes = crate::print::capture(ctx, |p| render::paint_slide(p, deck, s, Rect::from_min_size(Pos2::ZERO, size), render::Opts { wait: true, ..Default::default() }));
    let page = Page { size, shapes, links: Vec::new(), tags: Vec::new() };
    let img = crate::objects::page_image(ctx, &page, Rect::from_min_size(Pos2::ZERO, size), width as f32)?;
    crate::charts::encode_png(&img)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handout_cells() {
        let size = vec2(720.0, 960.0);
        for n in HANDOUTS {
            let c = cells(size, n);
            assert_eq!(c.len(), n as usize, "{n}");
            // Inside the page and not overlapping.
            for (i, (a, _)) in c.iter().enumerate() {
                assert!(Rect::from_min_size(Pos2::ZERO, size).contains_rect(*a));
                for (b, _) in &c[i + 1..] {
                    assert!(!a.intersects(*b), "{n}");
                }
            }
            assert_eq!(c.iter().filter(|(_, l)| l.is_some()).count(), if n == 3 { 3 } else { 0 });
        }
    }

    #[test]
    fn page_counts() {
        let mut deck = apex_slides::build::new_deck(0);
        for _ in 0..6 {
            let n = deck.slides.len();
            apex_slides::edit::add_slide(&mut deck, n, 0, 1);
        }
        let n = deck.slides.len();
        let count = |what| pages(&deck, what, &mut |size, _| Page { size, shapes: Vec::new(), links: Vec::new(), tags: Vec::new() }).len();
        assert_eq!(count(Scope::ActiveSheet), n);
        assert_eq!(count(Scope::Notes), n);
        assert_eq!(count(Scope::Handouts(6)), n.div_ceil(6));
        assert_eq!(count(Scope::Handouts(3)), n.div_ceil(3));
        assert_eq!(count(Scope::Handouts(1)), n);
    }
}
