//! The Morph transition: objects with the same name on both slides move,
//! grow and turn from where they were to where they are, cross-fading
//! if they look different; the others fade out and in.

use std::collections::{HashMap, HashSet, VecDeque};

use apex_slides::{Chain, Deck, Frame, Object, Slide};
use eframe::egui::{Painter, Rect};

use super::render::{self, Opts, Xf};

/// Pairs of objects (old id, new id) that morph: same name, each used once.
pub fn pairs(old: &Slide, new: &Slide) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    // The old objects by name, in order.
    let mut by_name: HashMap<&str, VecDeque<u32>> = HashMap::new();
    for o in &old.objects {
        by_name.entry(morph_name(&o.name)).or_default().push_back(o.id);
    }
    let mut used = HashSet::new();
    for n in &new.objects {
        let name = morph_name(&n.name);
        if name.is_empty() {
            continue;
        }
        let Some(q) = by_name.get_mut(name) else { continue };
        while let Some(id) = q.pop_front() {
            if used.insert(id) {
                out.push((id, n.id));
                break;
            }
        }
    }
    out
}

/// The name objects are matched by: PowerPoint's forced matches start
/// with "!!"; plain names match too.
fn morph_name(n: &str) -> &str {
    n.trim().trim_start_matches("!!")
}

/// The frame at progress `k` between `a` and `b`, turning the short way.
pub fn lerp(a: &Frame, b: &Frame, k: f32) -> Frame {
    let m = |x: f32, y: f32| x + (y - x) * k;
    let mut turn = (b.rot - a.rot) % 360.0;
    if turn > 180.0 {
        turn -= 360.0;
    } else if turn < -180.0 {
        turn += 360.0;
    }
    let mut f = *b;
    f.x = m(a.x, b.x);
    f.y = m(a.y, b.y);
    f.w = m(a.w, b.w);
    f.h = m(a.h, b.h);
    f.rot = (a.rot + turn * k).rem_euclid(360.0);
    f
}

fn placed(o: &Object, f: Frame) -> Object {
    let mut o = o.clone();
    o.sp.frame = Some(f);
    o
}

/// Paints the morph from slide `old` to slide `new` at progress `k` in `r`.
pub fn paint(p: &Painter, deck: &Deck, old: &Slide, new: &Slide, r: Rect, k: f32) {
    let p = p.with_clip_rect(r.intersect(p.clip_rect()));
    let (oc, nc) = (Chain::for_slide(deck, old), Chain::for_slide(deck, new));
    let xf = Xf::for_slide(deck, r);
    let opts = Opts::default();
    let faded = |a: f32| {
        let mut q = p.clone();
        q.multiply_opacity(a.clamp(0.0, 1.0));
        q
    };
    // Both backgrounds and decorations, the new one fading in.
    render::paint_fill_rect(&p, &oc, &oc.background(), r, xf.sx, false);
    for (obj, c) in oc.decoration() {
        render::paint_object(&p, &c, obj, xf, xf.sx, opts);
    }
    let q = faded(k);
    render::paint_fill_rect(&q, &nc, &nc.background(), r, xf.sx, false);
    for (obj, c) in nc.decoration() {
        render::paint_object(&q, &c, obj, xf, xf.sx, opts);
    }
    let pairs = pairs(old, new);
    let (olds, news): (HashSet<u32>, HashSet<u32>) = pairs.iter().copied().unzip();
    // Objects only on one slide fade.
    let out = faded(1.0 - k);
    for o in old.objects.iter().filter(|o| !olds.contains(&o.id)) {
        render::paint_object(&out, &oc, o, xf, xf.sx, opts);
    }
    let inn = faded(k);
    for o in new.objects.iter().filter(|o| !news.contains(&o.id)) {
        render::paint_object(&inn, &nc, o, xf, xf.sx, opts);
    }
    // Pairs move; both are opaque at the middle, so a shape that looks
    // the same does not blink.
    let (qa, qb) = (faded(2.0 * (1.0 - k)), faded(2.0 * k));
    let by_id = |s: &'_ Slide| -> HashMap<u32, usize> {
        let mut m = HashMap::new();
        for (i, o) in s.objects.iter().enumerate() {
            m.entry(o.id).or_insert(i);
        }
        m
    };
    let (oi, ni) = (by_id(old), by_id(new));
    for (a, b) in &pairs {
        let (Some(oa), Some(ob)) = (oi.get(a).map(|&i| &old.objects[i]), ni.get(b).map(|&i| &new.objects[i])) else { continue };
        let f = lerp(&oc.frame(oa), &nc.frame(ob), k);
        render::paint_object(&qa, &oc, &placed(oa, f), xf, xf.sx, opts);
        render::paint_object(&qb, &nc, &placed(ob, f), xf, xf.sx, opts);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_slides::{ObjKind, TextBody};

    fn obj(id: u32, name: &str, x: f32) -> Object {
        let mut o = Object::text_box(id, Frame::new(x, 10.0, 100.0, 50.0), TextBody::plain("x"));
        o.name = name.into();
        o
    }

    #[test]
    fn same_names_pair_once() {
        let mut deck = apex_slides::build::new_deck(0);
        apex_slides::edit::add_slide(&mut deck, 1, 0, 6);
        let a = deck.slide_mut(0);
        a.objects = vec![obj(2, "Box", 0.0), obj(3, "!!Star", 0.0), obj(4, "Box", 0.0)];
        let b = deck.slide_mut(1);
        b.objects = vec![obj(7, "Star", 50.0), obj(8, "Box", 50.0), obj(9, "Other", 0.0)];
        let p = pairs(&deck.slides[0], &deck.slides[1]);
        assert_eq!(p, vec![(3, 7), (2, 8)]);
        assert!(matches!(deck.slides[1].objects[2].kind, ObjKind::Shape { .. }));
    }

    #[test]
    fn frames_move_and_turn_the_short_way() {
        let mut a = Frame::new(0.0, 0.0, 100.0, 50.0);
        let mut b = Frame::new(100.0, 50.0, 200.0, 150.0);
        a.rot = 350.0;
        b.rot = 10.0;
        let m = lerp(&a, &b, 0.5);
        assert_eq!((m.x, m.y, m.w, m.h), (50.0, 25.0, 150.0, 100.0));
        assert!(m.rot.abs() < 0.01 || (m.rot - 360.0).abs() < 0.01, "{}", m.rot);
        assert_eq!(lerp(&a, &b, 1.0).x, 100.0);
    }
}
