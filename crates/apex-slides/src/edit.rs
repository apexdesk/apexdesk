//! Changes to a deck: slides (new, duplicate, delete, move, hide, layout),
//! objects (delete, order, group, align and distribute) and text found
//! and replaced across slides. Callers take the deck from
//! `Presentation::edit`, which makes the undo step.

use std::sync::Arc;

use crate::combine::{self, Merge, Outline};
use crate::object::{Frame, Geometry, Group, ObjKind, Object, PhType, ShapeProps};
use crate::{Deck, Slide};

/// The object with this id in `objs` or in their groups.
pub fn find_mut(objs: &mut [Object], id: u32) -> Option<&mut Object> {
    for o in objs.iter_mut() {
        if o.id == id {
            return Some(o);
        }
        if let ObjKind::Group(g) = &mut o.kind {
            if g.children.iter().any(|c| c.max_id() >= id) {
                if let Some(f) = find_mut(&mut g.children, id) {
                    o.raw = None;
                    return Some(f);
                }
            }
        }
    }
    None
}

/// Placeholders of a new slide on `layout`: its title and content ones
/// (the footers are added by the header and footer settings).
fn layout_placeholders(deck: &Deck, master: usize, layout: usize) -> Vec<Object> {
    let Some(lay) = deck.masters.get(master).and_then(|m| m.layouts.get(layout)) else { return Vec::new() };
    let mut out = Vec::new();
    let mut id = 1;
    for o in &lay.objects {
        let Some(ph) = o.ph else { continue };
        if ph.kind.is_hf() {
            continue;
        }
        id += 1;
        let mut p = crate::build::placeholder(id, &o.name, ph.kind, ph.idx, None);
        p.ph = Some(ph);
        out.push(p);
    }
    out
}

/// Inserts a new slide on `layout` of `master` at `at`; returns its index.
pub fn add_slide(deck: &mut Deck, at: usize, master: usize, layout: usize) -> usize {
    let at = at.min(deck.slides.len());
    let master = master.min(deck.masters.len().saturating_sub(1));
    let layout = layout.min(deck.masters[master].layouts.len().saturating_sub(1));
    let mut s = Slide::new(deck.new_slide_id(), master, layout);
    s.objects = layout_placeholders(deck, master, layout);
    add_footers(deck, &mut s);
    deck.slides.insert(at, Arc::new(s));
    fix_sections(deck, at);
    at
}

/// The layout a new slide after slide `i` takes: "Title and Content"
/// after the title slide, else the same as slide `i`.
pub fn next_layout(deck: &Deck, i: usize) -> (usize, usize) {
    let Some(s) = deck.slides.get(i) else { return (0, 1) };
    let m = &deck.masters[s.master.min(deck.masters.len() - 1)];
    let kind = m.layouts.get(s.layout).map(|l| &*l.kind);
    if kind == Some("title") {
        if let Some(l) = m.layouts.iter().position(|l| &*l.kind == "obj") {
            return (s.master, l);
        }
    }
    (s.master, s.layout)
}

/// Copies of slides `idx` after the last of them; returns where they start.
pub fn duplicate_slides(deck: &mut Deck, idx: &[usize]) -> usize {
    let at = idx.iter().max().map_or(deck.slides.len(), |m| m + 1).min(deck.slides.len());
    let mut id = deck.new_slide_id();
    let copies: Vec<Arc<Slide>> = idx
        .iter()
        .filter_map(|&i| deck.slides.get(i))
        .map(|s| {
            let mut c = (**s).clone();
            c.id = id;
            id += 1;
            // The copy's objects kept as read use the original's relationships.
            if let Some(o) = &mut c.origin {
                o.same = false;
            }
            if let Some(n) = &mut c.notes {
                n.origin = None;
            }
            Arc::new(c)
        })
        .collect();
    let n = copies.len();
    for (k, c) in copies.into_iter().enumerate() {
        deck.slides.insert(at + k, c);
    }
    if n > 0 {
        fix_sections(deck, at);
    }
    at
}

/// Deletes slides `idx` (never the last one left).
pub fn delete_slides(deck: &mut Deck, idx: &[usize]) {
    let mut idx: Vec<usize> = idx.iter().copied().filter(|&i| i < deck.slides.len()).collect();
    idx.sort_unstable();
    idx.dedup();
    if idx.len() >= deck.slides.len() {
        idx.remove(0);
    }
    for &i in idx.iter().rev() {
        let id = deck.slides.remove(i).id;
        for s in &mut deck.sections {
            s.slides.retain(|&x| x != id);
        }
    }
    deck.sections.retain(|s| !s.slides.is_empty());
}

/// Moves slides `idx` (kept in order) to before slide `to` of the
/// current order; returns their new first index.
pub fn move_slides(deck: &mut Deck, idx: &[usize], to: usize) -> usize {
    let mut idx: Vec<usize> = idx.iter().copied().filter(|&i| i < deck.slides.len()).collect();
    idx.sort_unstable();
    idx.dedup();
    let before = idx.iter().filter(|&&i| i < to).count();
    let mut moving = Vec::new();
    for &i in idx.iter().rev() {
        moving.push(deck.slides.remove(i));
    }
    moving.reverse();
    let at = to.saturating_sub(before).min(deck.slides.len());
    for (k, s) in moving.into_iter().enumerate() {
        deck.slides.insert(at + k, s);
    }
    rebuild_sections(deck);
    at
}

/// Keeps a slide inserted at `at` in the section of the slide before it.
fn fix_sections(deck: &mut Deck, at: usize) {
    if deck.sections.is_empty() {
        return;
    }
    let id = deck.slides[at].id;
    if at == 0 {
        deck.sections[0].slides.insert(0, id);
    } else {
        let prev = deck.slides[at - 1].id;
        if let Some(s) = deck.sections.iter_mut().find(|s| s.slides.contains(&prev)) {
            let p = s.slides.iter().position(|&x| x == prev).unwrap();
            s.slides.insert(p + 1, id);
        }
    }
    rebuild_sections(deck);
}

/// Sections cover consecutive slides: each keeps the slides from its
/// first one (in the current order) to the next section's first.
pub fn rebuild_sections(deck: &mut Deck) {
    if deck.sections.is_empty() {
        return;
    }
    let order: Vec<u32> = deck.slides.iter().map(|s| s.id).collect();
    let mut starts: Vec<(usize, usize)> = deck.sections.iter().enumerate().filter_map(|(k, s)| s.slides.iter().filter_map(|id| order.iter().position(|x| x == id)).min().map(|p| (p, k))).collect();
    starts.sort_unstable();
    if let Some(first) = starts.first_mut() {
        first.0 = 0;
    }
    let mut out = Vec::new();
    for (n, &(p, k)) in starts.iter().enumerate() {
        let end = starts.get(n + 1).map_or(order.len(), |x| x.0);
        let mut s = deck.sections[k].clone();
        s.slides = order[p..end.max(p)].to_vec();
        out.push(s);
    }
    deck.sections = out;
}

/// Starts a new section named `name` at slide `i`.
pub fn add_section(deck: &mut Deck, i: usize, name: &str, id: String) {
    if deck.slides.is_empty() {
        return;
    }
    let i = i.min(deck.slides.len() - 1);
    if deck.sections.is_empty() && i > 0 {
        deck.sections.push(crate::Section { name: "Default Section".into(), id: fresh_guid(deck, 0), slides: vec![deck.slides[0].id] });
    }
    deck.sections.retain(|s| s.slides.first() != Some(&deck.slides[i].id));
    deck.sections.push(crate::Section { name: name.into(), id, slides: vec![deck.slides[i].id] });
    rebuild_sections(deck);
}

/// A GUID for a section, from the slide ids (stable for tests).
pub fn fresh_guid(deck: &Deck, salt: u32) -> String {
    let n = deck.sections.len() as u32 * 7919 + salt + deck.slides.len() as u32;
    format!("{{{:08X}-A1B2-4C3D-8E9F-{:012X}}}", 0x5EC7_0000u32 + n, (deck.width as u64) << 16 | n as u64)
}

/// Gives slide `i` another layout: placeholders the new layout has keep
/// their text; empty ones it lacks go, filled ones stay where they are.
pub fn set_layout(deck: &mut Deck, i: usize, master: usize, layout: usize) {
    let fresh = layout_placeholders(deck, master, layout);
    let lay_objs: Vec<Object> = deck.masters[master].layouts.get(layout).map(|l| l.objects.clone()).unwrap_or_default();
    let s = deck.slide_mut(i);
    s.master = master;
    s.layout = layout;
    let mut next = s.next_id();
    let mut kept: Vec<Object> = Vec::new();
    for o in std::mem::take(&mut s.objects) {
        match o.ph {
            Some(ph) if !ph.kind.is_hf() => {
                let has = crate::resolve::find_ph(&lay_objs, ph).is_some();
                let empty = o.text().is_none_or(|t| t.is_empty()) && matches!(o.kind, ObjKind::Shape { .. });
                if has || !empty {
                    let mut o = o;
                    if !has {
                        // Stays where it was, as a plain shape.
                        o.ph = None;
                    }
                    kept.push(o);
                }
            }
            _ => kept.push(o),
        }
    }
    for f in fresh {
        let ph = f.ph.unwrap();
        let taken = kept.iter().any(|o| o.ph.is_some_and(|p| crate::resolve::find_ph(&lay_objs, p).and_then(|x| x.ph) == Some(ph)));
        if !taken {
            let mut f = f;
            f.id = next;
            next += 1;
            kept.push(f);
        }
    }
    s.objects = kept;
}

/// Adds or removes the footer placeholders of a slide per the deck's
/// header and footer settings.
pub fn add_footers(deck: &Deck, s: &mut Slide) {
    let hf = &deck.hf;
    let title = deck.masters.get(s.master).and_then(|m| m.layouts.get(s.layout)).is_some_and(|l| &*l.kind == "title");
    let skip = hf.not_on_title && title;
    let want = |k: PhType| !skip && match k {
        PhType::Date => hf.date,
        PhType::Footer => hf.footer,
        PhType::SlideNum => hf.number,
        _ => false,
    };
    s.objects.retain(|o| o.ph.is_none_or(|p| !matches!(p.kind, PhType::Date | PhType::Footer | PhType::SlideNum) || want(p.kind)));
    let lay: Vec<Object> = deck.masters.get(s.master).and_then(|m| m.layouts.get(s.layout)).map(|l| l.objects.clone()).unwrap_or_default();
    for k in [PhType::Date, PhType::Footer, PhType::SlideNum] {
        if !want(k) || s.objects.iter().any(|o| o.is_placeholder(k)) {
            continue;
        }
        let idx = lay.iter().find(|o| o.is_placeholder(k)).and_then(|o| o.ph).map_or(0, |p| p.idx);
        let id = s.next_id();
        let name = match k {
            PhType::Date => "Date Placeholder",
            PhType::Footer => "Footer Placeholder",
            _ => "Slide Number Placeholder",
        };
        let mut o = crate::build::placeholder(id, &format!("{name} {}", id - 1), k, idx, None);
        if let Some(t) = o.text_mut() {
            match k {
                PhType::Footer => t.set_plain(&hf.footer_text),
                PhType::Date => t.set_plain(hf.fixed_date.as_deref().unwrap_or("")),
                _ => {}
            }
        }
        s.objects.push(o);
    }
}

/// Applies the header and footer settings to every slide (or to slide `only`).
pub fn apply_footers(deck: &mut Deck, only: Option<usize>) {
    for i in 0..deck.slides.len() {
        if only.is_some_and(|o| o != i) {
            continue;
        }
        let mut s = (*deck.slides[i]).clone();
        add_footers(deck, &mut s);
        // Footer text follows the settings.
        for o in &mut s.objects {
            if o.is_placeholder(PhType::Footer) {
                if let Some(t) = o.text_mut() {
                    if t.text() != deck.hf.footer_text {
                        t.set_plain(&deck.hf.footer_text);
                        o.raw = None;
                    }
                }
            }
        }
        let changed = s.objects.len() != deck.slides[i].objects.len() || s.objects.iter().any(|o| o.raw.is_none());
        if changed {
            *deck.slide_mut(i) = s;
        }
    }
}

/// Deletes objects `ids` of slide `i`, and their animations.
pub fn delete_objects(deck: &mut Deck, i: usize, ids: &[u32]) {
    let s = deck.slide_mut(i);
    delete_in(&mut s.objects, ids);
    let alive: Vec<u32> = {
        let mut v = Vec::new();
        for o in &s.objects {
            o.walk(&mut |x| v.push(x.id));
        }
        v
    };
    s.timing.retain_shapes(&|id| alive.contains(&id));
}

/// Deletes objects `ids` of `v` (a slide's, layout's or master's), in
/// groups too.
pub fn delete_in(v: &mut Vec<Object>, ids: &[u32]) {
    v.retain(|o| !ids.contains(&o.id));
    for o in v.iter_mut() {
        if let ObjKind::Group(g) = &mut o.kind {
            let n = g.children.len();
            delete_in(&mut g.children, ids);
            if g.children.len() != n {
                o.raw = None;
            }
        }
    }
}

/// A new object id among `v`.
pub fn next_id_in(v: &[Object]) -> u32 {
    v.iter().map(Object::max_id).max().unwrap_or(1).max(1) + 1
}

/// Stacking changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    Front,
    Back,
    Forward,
    Backward,
}

/// Restacks the top-level objects `ids` of slide `i`.
pub fn reorder(deck: &mut Deck, i: usize, ids: &[u32], how: Order) {
    reorder_in(&mut deck.slide_mut(i).objects, ids, how);
}

/// Restacks objects `ids` of `v`.
pub fn reorder_in(v: &mut Vec<Object>, ids: &[u32], how: Order) {
    match how {
        Order::Front | Order::Back => {
            let (mut sel, rest): (Vec<Object>, Vec<Object>) = std::mem::take(v).into_iter().partition(|o| ids.contains(&o.id));
            *v = if how == Order::Front {
                let mut r = rest;
                r.append(&mut sel);
                r
            } else {
                sel.extend(rest);
                sel
            };
        }
        Order::Forward => {
            for k in (0..v.len().saturating_sub(1)).rev() {
                if ids.contains(&v[k].id) && !ids.contains(&v[k + 1].id) {
                    v.swap(k, k + 1);
                }
            }
        }
        Order::Backward => {
            for k in 1..v.len() {
                if ids.contains(&v[k].id) && !ids.contains(&v[k - 1].id) {
                    v.swap(k, k - 1);
                }
            }
        }
    }
}

/// The box around frames.
pub fn union(frames: &[Frame]) -> Option<Frame> {
    let mut it = frames.iter().map(Frame::bounds);
    let (mut x0, mut y0, w, h) = it.next()?;
    let (mut x1, mut y1) = (x0 + w, y0 + h);
    for (x, y, w, h) in it {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x + w);
        y1 = y1.max(y + h);
    }
    Some(Frame::new(x0, y0, x1 - x0, y1 - y0))
}

/// Groups top-level objects `ids` of slide `i` (their frames must be
/// set); returns the group's id.
pub fn group(deck: &mut Deck, i: usize, ids: &[u32]) -> Option<u32> {
    group_in(&mut deck.slide_mut(i).objects, ids)
}

/// Groups objects `ids` of `objects` (their frames must be set).
pub fn group_in(objects: &mut Vec<Object>, ids: &[u32]) -> Option<u32> {
    let pos: Vec<usize> = objects.iter().enumerate().filter(|(_, o)| ids.contains(&o.id) && o.sp.frame.is_some()).map(|(k, _)| k).collect();
    if pos.len() < 2 {
        return None;
    }
    let id = next_id_in(objects);
    let at = *pos.last().unwrap() + 1 - pos.len();
    let mut children = Vec::new();
    for &k in pos.iter().rev() {
        children.push(objects.remove(k));
    }
    children.reverse();
    let frames: Vec<Frame> = children.iter().filter_map(|o| o.sp.frame).collect();
    let f = union(&frames)?;
    let mut g = Object::new(id, format!("Group {}", id - 1), ObjKind::Group(Box::new(Group { children, child: f })));
    g.sp.frame = Some(f);
    objects.insert(at, g);
    Some(id)
}

/// Merges shapes of `objects` (Shape Format > Merge Shapes): `picked`
/// are their ids in the order they were selected, with their outlines
/// ([`combine::outline`], or their text's). The result takes the first
/// one's look, text and place in the stacking order; for Fragment every
/// piece is a shape. One shape alone is made a drawn shape (text to
/// shape). Returns the new shapes' ids, empty when nothing changed.
pub fn merge_in(objects: &mut Vec<Object>, picked: &[(u32, Outline)], how: Merge) -> Vec<u32> {
    let shapes: Vec<&(u32, Outline)> = picked.iter().filter(|(id, o)| !o.is_empty() && objects.iter().any(|x| x.id == *id && matches!(x.kind, ObjKind::Shape { connector: false, .. }))).collect();
    if shapes.is_empty() || shapes.len() < 2 && how != Merge::Union {
        return Vec::new();
    }
    let outlines: Vec<Outline> = shapes.iter().map(|s| s.1.clone()).collect();
    let made: Vec<(Frame, Geometry)> = combine::merge(&outlines, how).iter().filter_map(combine::to_geometry).collect();
    if made.is_empty() {
        return Vec::new();
    }
    let first = objects.iter().find(|o| o.id == shapes[0].0).cloned().expect("picked above");
    let mut at = objects.iter().position(|o| o.id == first.id).unwrap_or(objects.len());
    let gone: Vec<u32> = shapes.iter().map(|s| s.0).collect();
    at -= objects[..at].iter().filter(|o| gone.contains(&o.id)).count();
    objects.retain(|o| !gone.contains(&o.id));
    let mut ids = Vec::new();
    for (f, g) in made {
        let id = next_id_in(objects);
        let mut o = Object::new(id, format!("Freeform {}", id - 1), ObjKind::Shape { text: None, text_box: false, connector: false });
        o.sp = ShapeProps { frame: Some(f), geom: Some(g), ..first.sp.clone() };
        o.style = first.style.clone();
        // The text stays on the one shape a merge makes, as in PowerPoint.
        if how != Merge::Fragment
            && let ObjKind::Shape { text, .. } = &first.kind
        {
            o.kind = ObjKind::Shape { text: text.clone(), text_box: false, connector: false };
        }
        objects.insert(at + ids.len(), o);
        ids.push(id);
    }
    ids
}

/// Puts a group's objects back on the slide, where the group showed them.
pub fn ungroup(deck: &mut Deck, i: usize, id: u32) -> Vec<u32> {
    ungroup_in(&mut deck.slide_mut(i).objects, id)
}

/// Puts group `id` of `objects` back as its objects.
pub fn ungroup_in(objects: &mut Vec<Object>, id: u32) -> Vec<u32> {
    let Some(k) = objects.iter().position(|o| o.id == id) else { return Vec::new() };
    let ObjKind::Group(_) = &objects[k].kind else { return Vec::new() };
    let g = objects.remove(k);
    let f = g.sp.frame.unwrap_or_default();
    let ObjKind::Group(grp) = g.kind else { unreachable!() };
    let c = grp.child;
    let sx = if c.w > 0.0 { f.w / c.w } else { 1.0 };
    let sy = if c.h > 0.0 { f.h / c.h } else { 1.0 };
    let mut ids = Vec::new();
    for (n, mut o) in grp.children.into_iter().enumerate() {
        if let Some(of) = &mut o.sp.frame {
            of.x = f.x + (of.x - c.x) * sx;
            of.y = f.y + (of.y - c.y) * sy;
            of.w *= sx;
            of.h *= sy;
            of.rot = (of.rot + f.rot).rem_euclid(360.0);
        }
        o.raw = None;
        ids.push(o.id);
        objects.insert(k + n, o);
    }
    ids
}

/// Alignments of objects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlignTo {
    Left,
    Center,
    Right,
    Top,
    Middle,
    Bottom,
    /// Even spaces across, down.
    SpreadH,
    SpreadV,
}

/// Aligns frames `fs` to each other (or to the slide `page` for one).
pub fn align(fs: &mut [Frame], how: AlignTo, page: Option<Frame>) {
    let Some(b) = page.or_else(|| union(fs)) else { return };
    match how {
        AlignTo::Left => fs.iter_mut().for_each(|f| f.x = b.x),
        AlignTo::Center => fs.iter_mut().for_each(|f| f.x = b.x + (b.w - f.w) / 2.0),
        AlignTo::Right => fs.iter_mut().for_each(|f| f.x = b.x + b.w - f.w),
        AlignTo::Top => fs.iter_mut().for_each(|f| f.y = b.y),
        AlignTo::Middle => fs.iter_mut().for_each(|f| f.y = b.y + (b.h - f.h) / 2.0),
        AlignTo::Bottom => fs.iter_mut().for_each(|f| f.y = b.y + b.h - f.h),
        AlignTo::SpreadH | AlignTo::SpreadV => {
            let h = how == AlignTo::SpreadH;
            let mut order: Vec<usize> = (0..fs.len()).collect();
            order.sort_by(|&a, &c| if h { fs[a].x.total_cmp(&fs[c].x) } else { fs[a].y.total_cmp(&fs[c].y) });
            let total: f32 = fs.iter().map(|f| if h { f.w } else { f.h }).sum();
            let n = fs.len().max(2) as f32 - 1.0;
            let gap = ((if h { b.w } else { b.h }) - total) / n;
            let mut at = if h { b.x } else { b.y };
            for k in order {
                if h {
                    fs[k].x = at;
                    at += fs[k].w + gap;
                } else {
                    fs[k].y = at;
                    at += fs[k].h + gap;
                }
            }
        }
    }
}

/// A place text was found: slide, object id, text body among the
/// object's (`Object::text_bodies` order, 0 for shapes; notes are
/// `u32::MAX`), and the paragraph span.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub slide: usize,
    pub object: u32,
    pub body: usize,
    pub start: apex_doc::Pos,
    pub end: apex_doc::Pos,
}

/// The id standing for the notes in a [`Hit`].
pub const NOTES: u32 = u32::MAX;

/// Every place `needle` is on the slides and in the notes.
pub fn find_all(deck: &Deck, needle: &str, case: bool) -> Vec<Hit> {
    let mut out = Vec::new();
    if needle.is_empty() {
        return out;
    }
    let mut look = |slide: usize, object: u32, body: usize, t: &crate::TextBody| {
        let doc = apex_doc::Document::from_paragraphs(t.paras.clone());
        for (start, end) in doc.find_all(needle, case) {
            out.push(Hit { slide, object, body, start, end });
        }
    };
    for (i, s) in deck.slides.iter().enumerate() {
        for o in &s.objects {
            o.walk(&mut |x| {
                for (k, t) in x.text_bodies_here().into_iter().enumerate() {
                    look(i, x.id, k, t);
                }
            });
        }
        if let Some(n) = &s.notes {
            look(i, NOTES, 0, &n.text);
        }
    }
    out
}

/// Replaces every `needle` by `with` on the slides and in the notes;
/// returns how many.
pub fn replace_all(deck: &mut Deck, needle: &str, with: &str, case: bool) -> usize {
    if needle.is_empty() {
        return 0;
    }
    let mut total = 0;
    for i in 0..deck.slides.len() {
        let hits = find_all_in(&deck.slides[i], needle, case);
        if !hits {
            continue;
        }
        let s = deck.slide_mut(i);
        let mut sub = |t: &mut crate::TextBody| -> bool {
            let mut doc = apex_doc::Document::from_paragraphs(t.paras.clone());
            let n = doc.replace_all(needle, with, case);
            if n > 0 {
                t.paras = doc.paragraphs;
                total += n;
            }
            n > 0
        };
        for o in &mut s.objects {
            o.text_bodies_mut(&mut sub);
        }
        if let Some(n) = &mut s.notes {
            if sub(&mut n.text) {
                n.origin.as_mut().map(|o| o.same = false);
            }
        }
    }
    total
}

fn find_all_in(s: &Slide, needle: &str, case: bool) -> bool {
    let low = |x: &str| if case { x.to_string() } else { x.to_lowercase() };
    let n = low(needle);
    let mut any = false;
    for o in &s.objects {
        for t in o.text_bodies() {
            any |= low(&t.text()).contains(&n);
        }
    }
    any || s.notes.as_ref().is_some_and(|x| low(&x.text.text()).contains(&n))
}

/// The outline: for each slide its title and the body paragraphs with
/// their levels.
pub fn outline(deck: &Deck) -> Vec<(String, Vec<(u8, String)>)> {
    deck.slides
        .iter()
        .map(|s| {
            let mut body = Vec::new();
            for o in &s.objects {
                if let (Some(ph), Some(t)) = (o.ph, o.text()) {
                    if !ph.kind.is_title() && !ph.kind.is_hf() {
                        body.extend(t.paras.iter().filter(|p| !p.text.is_empty()).map(|p| (p.style.level, p.text.clone())));
                    }
                }
            }
            (s.title(), body)
        })
        .collect()
}

impl Object {
    /// Text bodies of this object alone (not its group's children).
    pub fn text_bodies_here(&self) -> Vec<&crate::TextBody> {
        match &self.kind {
            ObjKind::Shape { text: Some(t), .. } => vec![t],
            ObjKind::Table(t) => t.rows.iter().flat_map(|r| r.cells.iter().map(|c| &c.text)).collect(),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::new_deck;
    use crate::TextBody;

    fn deck3() -> Deck {
        let mut d = new_deck(0);
        let (m, l) = next_layout(&d, 0);
        add_slide(&mut d, 1, m, l);
        add_slide(&mut d, 2, m, l);
        d
    }

    #[test]
    fn slides_move_duplicate_delete() {
        let mut d = deck3();
        assert_eq!(d.slides.len(), 3);
        assert_eq!(d.masters[0].layouts[d.slides[1].layout].kind.as_ref(), "obj");
        let ids: Vec<u32> = d.slides.iter().map(|s| s.id).collect();
        move_slides(&mut d, &[0], 3);
        assert_eq!(d.slides.iter().map(|s| s.id).collect::<Vec<_>>(), vec![ids[1], ids[2], ids[0]]);
        let at = duplicate_slides(&mut d, &[2]);
        assert_eq!(at, 3);
        assert_ne!(d.slides[3].id, d.slides[2].id);
        delete_slides(&mut d, &[0, 1, 2, 3]);
        assert_eq!(d.slides.len(), 1);
    }

    #[test]
    fn layouts_keep_text() {
        let mut d = deck3();
        let title = d.slides[1].objects.iter().find(|o| o.is_title()).unwrap().id;
        *d.object_mut(1, title).unwrap().text_mut().unwrap() = TextBody::plain("Hello");
        let blank = d.masters[0].layouts.iter().position(|l| &*l.kind == "blank").unwrap();
        set_layout(&mut d, 1, 0, blank);
        // The title had text: it stays, as a shape.
        assert_eq!(d.slides[1].objects.len(), 1);
        assert_eq!(d.slides[1].objects[0].text().unwrap().text(), "Hello");
        let two = d.masters[0].layouts.iter().position(|l| &*l.kind == "twoObj").unwrap();
        set_layout(&mut d, 2, 0, two);
        assert_eq!(d.slides[2].objects.len(), 3);
    }

    #[test]
    fn find_and_replace() {
        let mut d = deck3();
        let id = d.slides[1].objects[0].id;
        *d.object_mut(1, id).unwrap().text_mut().unwrap() = TextBody::plain("apple pie and apple");
        d.slide_mut(2).notes = Some(crate::Notes { text: TextBody::plain("Apple"), origin: None });
        assert_eq!(find_all(&d, "apple", false).len(), 3);
        assert_eq!(replace_all(&mut d, "apple", "pear", false), 3);
        assert_eq!(d.slides[2].notes_text(), "pear");
        assert_eq!(outline(&d)[1].0, "pear pie and pear");
    }

    #[test]
    fn group_ungroup_align() {
        let mut d = new_deck(0);
        let s = d.slide_mut(0);
        s.objects.clear();
        s.objects.push(Object::text_box(2, Frame::new(10.0, 10.0, 100.0, 50.0), TextBody::plain("a")));
        s.objects.push(Object::text_box(3, Frame::new(200.0, 100.0, 50.0, 50.0), TextBody::plain("b")));
        let g = group(&mut d, 0, &[2, 3]).unwrap();
        assert_eq!(d.slides[0].objects.len(), 1);
        assert_eq!(d.slides[0].objects[0].sp.frame, Some(Frame::new(10.0, 10.0, 240.0, 140.0)));
        d.object_mut(0, g).unwrap().sp.frame = Some(Frame::new(20.0, 10.0, 480.0, 140.0));
        let ids = ungroup(&mut d, 0, g);
        assert_eq!(ids, vec![2, 3]);
        assert_eq!(d.slides[0].objects[1].sp.frame.unwrap().x, 400.0);
        let mut fs = [Frame::new(0.0, 0.0, 10.0, 10.0), Frame::new(50.0, 20.0, 20.0, 10.0), Frame::new(100.0, 0.0, 10.0, 10.0)];
        align(&mut fs, AlignTo::SpreadH, None);
        assert_eq!(fs[1].x, 45.0);
        align(&mut fs, AlignTo::Bottom, None);
        assert_eq!(fs[0].y, 20.0);
    }
}
