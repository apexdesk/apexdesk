//! Objects on the current slide: the selection and what is done to it
//! (moving, sizing, turning, deleting, stacking, grouping, aligning,
//! copying and pasting) and inserting new ones (text boxes, shapes,
//! pictures, tables, charts and equations), each one undo step.

use std::sync::{Arc, Mutex};

use apex_doc::{Align, VAnchor};
use apex_slides::color::Mod;
use apex_slides::edit::{self, AlignTo, Order};
use apex_slides::fill::StyleRefs;
use apex_slides::text::Autofit;
use apex_slides::{Chain, Deck, Frame, Geometry, ObjKind, Object, SColor, Scheme, TextBody};

use super::SlidesView;

/// Where pasted objects go from their place when it is taken, points.
pub const PASTE_STEP: f32 = 12.0;

/// Objects copied, for pasting on any slide of any presentation, with
/// the text put on the system clipboard (to know a paste is ours).
struct Clip {
    objects: Vec<Object>,
    text: String,
}

static CLIP: Mutex<Option<Clip>> = Mutex::new(None);

/// Gives `o` and what is in it ids from `next` on.
fn renumber(o: &mut Object, next: &mut u32) {
    o.walk_mut(&mut |x| {
        x.id = *next;
        *next += 1;
    });
}

/// Office's look for a new shape: filled with accent 1, outlined with it
/// darker, light text.
pub fn shape_style() -> StyleRefs {
    let accent = || SColor::scheme(Scheme::Accent(0));
    StyleRefs {
        line: Some((2, accent().with(Mod::Shade(50000)))),
        fill: Some((1, accent())),
        effect: Some((0, accent())),
        font: Some((Arc::from("minor"), Some(SColor::scheme(Scheme::Lt1)))),
    }
}

/// A new shape of preset `preset` in `frame`: centered text; lines and
/// connectors take no fill or text.
pub fn new_shape(id: u32, preset: &str, frame: Frame, arrow: bool) -> Object {
    let geom = Geometry::preset(preset);
    let line = geom.is_line() || preset.contains("Connector");
    let mut text = TextBody::plain("");
    text.body.anchor = Some(VAnchor::Middle);
    text.edit_props(0, |p| p.align = Some(Align::Center));
    let kind = ObjKind::Shape { text: (!line).then_some(text), text_box: false, connector: line };
    let mut o = Object::new(id, format!("{} {}", if line { "Connector" } else { "Shape" }, id.saturating_sub(1)), kind);
    o.sp.frame = Some(frame);
    o.sp.geom = Some(geom);
    let mut st = shape_style();
    if line {
        st.fill = None;
        st.line = Some((1, SColor::scheme(Scheme::Accent(0))));
        if arrow {
            o.sp.line = Some(apex_slides::Line { tail: Some(apex_slides::fill::Arrow::Triangle), ..Default::default() });
        }
    }
    o.style = Some(st);
    o
}

/// A new text box in `frame` that grows with its text.
pub fn new_text_box(id: u32, frame: Frame) -> Object {
    let mut t = TextBody::plain("");
    t.body.wrap = Some(true);
    t.body.autofit = Some(Autofit::Resize);
    Object::text_box(id, frame, t)
}

/// What the canvas edits: a slide, or a layout or master (master view).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Slide(usize),
    Layout(usize, usize),
    Master(usize),
}

/// The objects of `t`.
pub fn objs(deck: &Deck, t: Target) -> Option<&Vec<Object>> {
    match t {
        Target::Slide(i) => deck.slides.get(i).map(|s| &s.objects),
        Target::Layout(m, l) => deck.masters.get(m)?.layouts.get(l).map(|l| &l.objects),
        Target::Master(m) => deck.masters.get(m).map(|m| &m.objects),
    }
}

/// The objects of `t` to change (`t` must exist).
pub fn objs_mut(deck: &mut Deck, t: Target) -> &mut Vec<Object> {
    match t {
        Target::Slide(i) => &mut deck.slide_mut(i).objects,
        Target::Layout(m, l) => &mut deck.layout_mut(m, l).objects,
        Target::Master(m) => &mut deck.master_mut(m).objects,
    }
}

/// How the objects of `t` inherit.
pub fn chain(deck: &Deck, t: Target) -> Option<Chain<'_>> {
    objs(deck, t)?;
    Some(match t {
        Target::Slide(i) => Chain::for_slide(deck, &deck.slides[i]),
        Target::Layout(m, l) => Chain::for_layout(deck, m, l),
        Target::Master(m) => Chain::for_master(deck, m),
    })
}

impl SlidesView {
    /// What the canvas shows and edits.
    pub fn target(&self) -> Target {
        match (self.mode, self.master_at) {
            (super::Mode::Master, (m, Some(l))) => Target::Layout(m, l),
            (super::Mode::Master, (m, None)) => Target::Master(m),
            _ => Target::Slide(self.cur),
        }
    }

    /// The objects on the canvas.
    pub fn cur_objects(&self) -> &[Object] {
        objs(&self.pres.deck, self.target()).map_or(&[], |v| v.as_slice())
    }

    /// The objects on the canvas to change, as one undo step `what`
    /// (none when there is nothing to change).
    pub(super) fn edit_objects(&mut self, what: &'static str) -> Option<&mut Vec<Object>> {
        let t = self.target();
        if self.read_only || objs(&self.pres.deck, t).is_none() {
            return None;
        }
        Some(objs_mut(self.pres.edit(what), t))
    }

    /// The selected objects' ids that are on the canvas.
    pub fn selection(&self) -> Vec<u32> {
        let objs = self.cur_objects();
        self.sel.iter().copied().filter(|id| objs.iter().any(|o| o.id == *id)).collect()
    }

    /// Drops selected objects no longer there (after undo, deleting).
    pub(super) fn clamp_selection(&mut self) {
        self.sel = self.selection();
        if let Some(t) = &self.typing
            && !self.cur_objects().iter().any(|o| o.id == t.id)
        {
            self.typing = None;
        }
    }

    pub fn select_all(&mut self) {
        self.sel = self.cur_objects().iter().filter(|o| !o.hidden).map(|o| o.id).collect();
    }

    /// Selects the next object (the first with none), or the one before.
    pub fn select_next(&mut self, back: bool) {
        let objs = self.cur_objects();
        let n = objs.len();
        if n == 0 {
            return;
        }
        let at = self.sel.last().and_then(|id| objs.iter().position(|o| o.id == *id));
        let k = match (at, back) {
            (None, false) => 0,
            (None, true) => n - 1,
            (Some(k), false) => (k + 1) % n,
            (Some(k), true) => (k + n - 1) % n,
        };
        self.sel = vec![objs[k].id];
    }

    /// The frames of top-level objects `ids` as they show (placeholders
    /// take theirs from the layout).
    pub fn frames_of(&self, ids: &[u32]) -> Vec<(u32, Frame)> {
        let Some(ch) = chain(&self.pres.deck, self.target()) else { return Vec::new() };
        self.cur_objects().iter().filter(|o| ids.contains(&o.id)).map(|o| (o.id, ch.frame(o))).collect()
    }

    /// Sets frames of top-level objects, as one step (or joining the step
    /// being made, while dragging).
    pub fn set_frames(&mut self, what: &'static str, frames: &[(u32, Frame)], join: bool) {
        let t = self.target();
        if self.read_only || frames.is_empty() || objs(&self.pres.deck, t).is_none() {
            return;
        }
        let deck = if join { &mut self.pres.deck } else { self.pres.edit(what) };
        let objs = objs_mut(deck, t);
        for (id, f) in frames {
            if let Some(o) = objs.iter_mut().find(|o| o.id == *id) {
                let mut f = *f;
                f.sanitize();
                o.sp.frame = Some(f);
                // A table's columns and rows take the new size.
                if let apex_slides::ObjKind::Table(t) = &mut o.kind {
                    let (w, h): (f32, f32) = (t.cols.iter().sum(), t.rows.iter().map(|r| r.height).sum());
                    if w > 0.0 && f.w > 0.0 {
                        t.cols.iter_mut().for_each(|c| *c *= f.w / w);
                    }
                    if h > 0.0 && f.h > 0.0 {
                        t.rows.iter_mut().for_each(|r| r.height *= f.h / h);
                    }
                }
                o.touch();
            }
        }
        if join {
            self.pres.modified = true;
            self.pres.revision += 1;
        }
    }

    /// Moves the selection by (dx, dy) points.
    pub fn nudge(&mut self, dx: f32, dy: f32) {
        let fs: Vec<(u32, Frame)> = self.frames_of(&self.selection()).into_iter().map(|(id, f)| (id, Frame { x: f.x + dx, y: f.y + dy, ..f })).collect();
        self.set_frames("slides.undo.move_objects", &fs, false);
    }

    pub fn delete_selected(&mut self) {
        let ids = self.selection();
        if self.read_only || ids.is_empty() {
            return;
        }
        self.typing = None;
        match self.target() {
            Target::Slide(i) => edit::delete_objects(self.pres.edit("slides.undo.delete_objects"), i, &ids),
            _ => {
                if let Some(v) = self.edit_objects("slides.undo.delete_objects") {
                    edit::delete_in(v, &ids);
                }
            }
        }
        self.sel.clear();
    }

    pub fn order(&mut self, how: Order) {
        let ids = self.selection();
        if ids.is_empty() {
            return;
        }
        if let Some(v) = self.edit_objects("slides.undo.order") {
            edit::reorder_in(v, &ids, how);
        }
    }

    /// Groups the selection (two objects at least).
    pub fn group_selected(&mut self) {
        let ids = self.selection();
        if ids.len() < 2 {
            return;
        }
        // Frames set from what shows, so placeholders group where they are.
        let fs = self.frames_of(&ids);
        let Some(v) = self.edit_objects("slides.undo.group") else { return };
        for (id, f) in fs {
            if let Some(o) = v.iter_mut().find(|o| o.id == id) {
                o.sp.frame.get_or_insert(f);
            }
        }
        if let Some(g) = edit::group_in(v, &ids) {
            self.sel = vec![g];
        }
    }

    pub fn ungroup_selected(&mut self) {
        let ids = self.selection();
        let groups: Vec<u32> = self.cur_objects().iter().filter(|o| ids.contains(&o.id) && matches!(o.kind, ObjKind::Group(_))).map(|o| o.id).collect();
        if groups.is_empty() {
            return;
        }
        let Some(v) = self.edit_objects("slides.undo.ungroup") else { return };
        let mut sel = Vec::new();
        for g in groups {
            sel.extend(edit::ungroup_in(v, g));
        }
        self.sel = sel;
    }

    /// Aligns the selection to itself, or one object to the slide.
    pub fn align_selected(&mut self, how: AlignTo) {
        let fs = self.frames_of(&self.selection());
        if fs.is_empty() {
            return;
        }
        let page = (fs.len() == 1).then(|| Frame::new(0.0, 0.0, self.pres.width, self.pres.height));
        let mut frames: Vec<Frame> = fs.iter().map(|x| x.1).collect();
        edit::align(&mut frames, how, page);
        let out: Vec<(u32, Frame)> = fs.iter().map(|x| x.0).zip(frames).collect();
        self.set_frames("slides.undo.align", &out, false);
    }

    /// Turns the selection `deg` degrees clockwise.
    pub fn rotate_selected(&mut self, deg: f32) {
        let fs: Vec<(u32, Frame)> = self.frames_of(&self.selection()).into_iter().map(|(id, f)| (id, Frame { rot: (f.rot + deg).rem_euclid(360.0), ..f })).collect();
        self.set_frames("slides.undo.rotate", &fs, false);
    }

    pub fn flip_selected(&mut self, horizontal: bool) {
        let fs: Vec<(u32, Frame)> = self
            .frames_of(&self.selection())
            .into_iter()
            .map(|(id, f)| (id, if horizontal { Frame { flip_h: !f.flip_h, ..f } } else { Frame { flip_v: !f.flip_v, ..f } }))
            .collect();
        self.set_frames("slides.undo.flip", &fs, false);
    }

    /// Copies the selection (their text goes on the system clipboard,
    /// returned); `cut` deletes it. None with nothing selected.
    pub fn copy_selected(&mut self, cut: bool) -> Option<String> {
        let ids = self.selection();
        let ch = chain(&self.pres.deck, self.target())?;
        if ids.is_empty() {
            return None;
        }
        let objects: Vec<Object> = self
            .cur_objects()
            .iter()
            .filter(|o| ids.contains(&o.id))
            .map(|o| {
                let mut c = o.clone();
                c.sp.frame = Some(ch.frame(o));
                c
            })
            .collect();
        let mut text: String = objects.iter().flat_map(|o| o.text_bodies()).map(|t| t.text()).collect::<Vec<_>>().join("\n");
        if text.is_empty() {
            // Something the system clipboard can compare.
            text = " ".into();
        }
        *CLIP.lock().unwrap_or_else(|e| e.into_inner()) = Some(Clip { objects, text: text.clone() });
        if cut {
            self.delete_selected();
        }
        Some(text)
    }

    /// Forgets the objects copied (shared by every window).
    #[cfg(test)]
    pub fn clear_clip() {
        *CLIP.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// Whether there are copied objects to paste.
    pub fn can_paste() -> bool {
        CLIP.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|c| !c.objects.is_empty())
    }

    /// Whether `text` pasted from the system clipboard is what we copied.
    pub fn clip_is_ours(text: &str) -> bool {
        CLIP.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|c| c.text == text || text.is_empty())
    }

    /// Pastes the objects copied; the pasted ones are selected. False
    /// when there are none.
    pub fn paste_objects(&mut self) -> bool {
        let Some(objects) = CLIP.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|c| c.objects.clone()) else { return false };
        if objects.is_empty() {
            return false;
        }
        self.typing = None;
        let Some(v) = self.edit_objects("slides.undo.paste") else { return false };
        let mut next = edit::next_id_in(v);
        let mut sel = Vec::new();
        for mut o in objects {
            // Where the same object already is, the copy goes a bit aside.
            let f = o.sp.frame.unwrap_or_default();
            let taken = |f: Frame| v.iter().any(|x| x.sp.frame.is_some_and(|g| (g.x - f.x).abs() < 0.5 && (g.y - f.y).abs() < 0.5));
            let mut g = f;
            while taken(g) {
                g.x += PASTE_STEP;
                g.y += PASTE_STEP;
            }
            o.sp.frame = Some(g);
            // A second placeholder of a kind would confuse the layout.
            if let Some(ph) = o.ph
                && v.iter().any(|x| x.ph.is_some_and(|p| p.kind == ph.kind && p.idx == ph.idx))
            {
                o.ph = None;
            }
            o.detach();
            renumber(&mut o, &mut next);
            sel.push(o.id);
            v.push(o);
        }
        self.sel = sel;
        true
    }

    /// Duplicates the selection in place (Ctrl+D).
    pub fn duplicate_objects(&mut self) {
        if self.copy_selected(false).is_some() {
            self.paste_objects();
        }
    }

    /// Adds `o` on top (its id is set) and selects it.
    pub fn insert_object(&mut self, what: &'static str, mut o: Object) -> Option<u32> {
        self.typing = None;
        let v = self.edit_objects(what)?;
        let mut next = edit::next_id_in(v);
        renumber(&mut o, &mut next);
        let id = o.id;
        // New objects are named after their id, as PowerPoint does.
        if let Some(stem) = o.name.strip_suffix(" 0").or(o.name.strip_suffix(" -1")) {
            o.name = format!("{stem} {}", id.saturating_sub(1));
        }
        v.push(o);
        self.sel = vec![id];
        Some(id)
    }

    /// A frame of `w` by `h` points in the middle of the slide.
    pub fn centered(&self, w: f32, h: f32) -> Frame {
        let (w, h) = (w.min(self.pres.width), h.min(self.pres.height));
        Frame::new((self.pres.width - w) / 2.0, (self.pres.height - h) / 2.0, w, h)
    }

    pub fn insert_shape(&mut self, preset: &str, frame: Frame, arrow: bool) -> Option<u32> {
        self.insert_object("slides.undo.insert_shape", new_shape(0, preset, frame, arrow))
    }

    /// A text box, being typed in.
    pub fn insert_text_box(&mut self, frame: Frame) -> Option<u32> {
        let id = self.insert_object("slides.undo.insert_text_box", new_text_box(0, frame))?;
        self.start_typing(id);
        Some(id)
    }

    /// A picture (encoded file) in the middle, at its size shrunk to the
    /// slide. False if it isn't a picture.
    pub fn insert_picture_bytes(&mut self, data: Vec<u8>) -> bool {
        let Some((w, h)) = apex_io::pixel_size(&data).filter(|&(w, h)| w > 0 && h > 0) else { return false };
        let (w, h) = (w as f32 * 0.75, h as f32 * 0.75);
        let k = (self.pres.width * 0.9 / w).min(self.pres.height * 0.9 / h).min(1.0);
        let frame = self.centered(w * k, h * k);
        let image = apex_doc::Image::new(Arc::from(data), frame.w, frame.h);
        let mut o = Object::new(0, "Picture", ObjKind::Picture(Box::new(apex_slides::object::Picture { image, media: None })));
        o.sp.frame = Some(frame);
        o.sp.geom = Some(Geometry::preset("rect"));
        self.insert_object("slides.undo.insert_picture", o).is_some()
    }

    pub fn insert_table(&mut self, rows: usize, cols: usize) -> Option<u32> {
        let w = self.pres.width * 0.8;
        let t = apex_slides::Table::new(rows, cols, w);
        let h: f32 = t.rows.iter().map(|r| r.height).sum();
        let mut o = Object::new(0, "Table", ObjKind::Table(Box::new(t)));
        o.sp.frame = Some(self.centered(w, h));
        self.insert_object("slides.undo.insert_table", o)
    }

    /// A chart whose data is the small workbook `book`.
    pub fn insert_chart(&mut self, book: Arc<[u8]>) -> Option<u32> {
        let mut o = Object::new(0, "Chart", ObjKind::Chart(apex_doc::ChartObject { book }));
        o.sp.frame = Some(self.centered(self.pres.width * 0.6, self.pres.height * 0.6));
        self.insert_object("slides.undo.insert_chart", o)
    }

    /// An equation typed in the linear format.
    pub fn insert_equation(&mut self, linear: &str) -> Option<u32> {
        let mut o = Object::new(0, "Equation", ObjKind::Math(Arc::new(apex_doc::math::Equation::new(linear))));
        o.sp.frame = Some(self.centered(self.pres.width * 0.4, 60.0));
        self.insert_object("slides.undo.insert_equation", o)
    }

    /// Changes object `id` on the canvas with `f`, one step.
    pub fn change_object(&mut self, what: &'static str, id: u32, f: impl FnOnce(&mut Object)) {
        if let Some(v) = self.edit_objects(what)
            && let Some(o) = edit::find_mut(v, id)
        {
            f(o);
            o.touch();
        }
    }

    /// Changes the equation of object `id`.
    pub fn set_equation(&mut self, id: u32, linear: &str) {
        self.change_object("slides.undo.equation", id, |o| o.kind = ObjKind::Math(Arc::new(apex_doc::math::Equation::new(linear))));
    }

    /// Changes the objects selected with `f`, one step (formatting).
    pub fn change_selected(&mut self, what: &'static str, f: &dyn Fn(&mut Object)) {
        let ids = self.selection();
        if ids.is_empty() {
            return;
        }
        let Some(v) = self.edit_objects(what) else { return };
        for o in v.iter_mut().filter(|o| ids.contains(&o.id)) {
            f(o);
            o.touch();
        }
    }

    /// The first selected object.
    pub fn selected_object(&self) -> Option<&Object> {
        let id = *self.selection().first()?;
        self.cur_objects().iter().find(|o| o.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> SlidesView {
        let mut v = SlidesView::new(apex_slides::build::new_deck(0));
        v.set_layout(0, 5);
        v
    }

    #[test]
    fn insert_move_delete_undo() {
        let mut v = view();
        let n0 = v.pres.slides[0].objects.len();
        let id = v.insert_shape("ellipse", Frame::new(10.0, 10.0, 100.0, 50.0), false).unwrap();
        assert_eq!(v.selection(), vec![id]);
        v.nudge(5.0, -2.0);
        let f = v.frames_of(&[id])[0].1;
        assert_eq!((f.x, f.y), (15.0, 8.0));
        v.rotate_selected(90.0);
        v.flip_selected(true);
        let f = v.frames_of(&[id])[0].1;
        assert!(f.flip_h && f.rot == 90.0);
        v.delete_selected();
        assert_eq!(v.pres.slides[0].objects.len(), n0);
        assert!(v.selection().is_empty());
        v.undo();
        assert_eq!(v.pres.slides[0].objects.len(), n0 + 1);
        for _ in 0..4 {
            v.undo();
        }
        assert_eq!(v.pres.slides[0].objects.len(), n0);
    }

    #[test]
    fn order_group_align() {
        let mut v = view();
        let a = v.insert_shape("rect", Frame::new(0.0, 0.0, 50.0, 50.0), false).unwrap();
        let b = v.insert_shape("rect", Frame::new(100.0, 40.0, 20.0, 20.0), false).unwrap();
        v.sel = vec![b];
        v.order(Order::Back);
        assert_eq!(v.pres.slides[0].objects[0].id, b);
        v.sel = vec![a, b];
        v.align_selected(AlignTo::Left);
        assert_eq!(v.frames_of(&[b])[0].1.x, 0.0);
        v.group_selected();
        let g = v.selection();
        assert_eq!(g.len(), 1);
        assert!(matches!(v.selected_object().unwrap().kind, ObjKind::Group(_)));
        v.ungroup_selected();
        let mut sel = v.selection();
        sel.sort();
        assert_eq!(sel, vec![a, b]);
    }

    #[test]
    fn copy_paste_and_duplicate() {
        let mut v = view();
        let a = v.insert_shape("rect", Frame::new(10.0, 10.0, 50.0, 50.0), false).unwrap();
        let text = v.copy_selected(false).unwrap();
        assert!(SlidesView::clip_is_ours(&text));
        assert!(v.paste_objects());
        let b = v.selection()[0];
        assert_ne!(a, b);
        assert_eq!(v.frames_of(&[b])[0].1.x, 10.0 + PASTE_STEP);
        v.duplicate_objects();
        let c = v.selection()[0];
        assert_eq!(v.frames_of(&[c])[0].1.x, 10.0 + 2.0 * PASTE_STEP);
        // Cut and paste on another slide keeps the place.
        v.copy_selected(true);
        v.new_slide_with(0, 5);
        assert!(v.paste_objects());
        let d = v.selection()[0];
        assert_eq!(v.frames_of(&[d])[0].1.x, 10.0 + 2.0 * PASTE_STEP);
    }

    #[test]
    fn inserts() {
        let mut v = view();
        assert!(v.insert_table(3, 4).is_some());
        assert!(matches!(v.selected_object().unwrap().kind, ObjKind::Table(_)));
        let id = v.insert_equation("a^2+b^2=c^2").unwrap();
        v.set_equation(id, "x");
        assert!(matches!(&v.selected_object().unwrap().kind, ObjKind::Math(e) if e.source() == "x"));
        assert!(!v.insert_picture_bytes(b"not a picture".to_vec()));
        let tb = v.insert_text_box(Frame::new(0.0, 0.0, 100.0, 30.0)).unwrap();
        assert_eq!(v.typing.as_ref().map(|t| t.id), Some(tb));
        v.read_only = true;
        assert!(v.insert_shape("rect", Frame::new(0.0, 0.0, 1.0, 1.0), false).is_none());
    }
}
