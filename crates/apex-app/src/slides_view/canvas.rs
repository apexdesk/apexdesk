//! The slide being edited on the canvas: clicking objects to select them,
//! dragging them, their handles (size, turn, line ends), selecting in a
//! box, drawing new shapes and typing in their text.

use apex_doc::{Shape, ShapeKind};
use apex_slides::{Chain, Frame, ObjKind, Object, TextBody};
use eframe::egui::{self, Color32, CursorIcon, Pos2, Rect, Sense, Stroke, Vec2, pos2, vec2};

use super::objects::{self, Target};
use super::render::{self, Xf};
use super::{SlidesView, typing};
use crate::shapes::{self, BoxGeo, Handle};
use crate::theme;

/// What a new object drawn on the canvas is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tool {
    TextBox,
    /// A preset shape; lines may end in an arrow.
    Shape { preset: &'static str, arrow: bool },
}

impl Tool {
    /// The tool for a pick in the shapes gallery.
    pub fn of(t: shapes::Template) -> Tool {
        match (t.kind, t.preset) {
            (ShapeKind::TextBox, _) => Tool::TextBox,
            (ShapeKind::Line, None) if t.arrow => Tool::Shape { preset: "straightConnector1", arrow: true },
            (_, Some(p)) => Tool::Shape { preset: p, arrow: t.arrow },
            (k, None) => Tool::Shape { preset: k.preset(), arrow: t.arrow },
        }
    }

    fn is_line(self) -> bool {
        matches!(self, Tool::Shape { preset, .. } if preset == "line" || preset.contains("Connector"))
    }
}

/// A drag on the canvas.
pub enum Drag {
    /// Moving or sizing the selection with `handle`, from slide point
    /// `start`; `orig`: the frames then. `moved` once it changed them.
    Objects { handle: Handle, start: Pos2, orig: Vec<(u32, Frame)>, moved: bool },
    /// Selecting the objects in a box, from screen point `start`.
    Marquee { start: Pos2 },
    /// Selecting text.
    Text,
    /// Moving the selected text (`copy`: copying it) to `to`.
    MoveText { to: Option<apex_doc::Pos>, copy: bool },
    /// Drawing a new object from slide point `start`.
    Draw { start: Pos2, tool: Tool },
}

/// Whether `o` is drawn as a line (ends to drag, not a box).
fn is_line(o: &Object, ch: &Chain) -> bool {
    matches!(o.kind, ObjKind::Shape { connector: true, .. }) || ch.geometry(o).is_line()
}

/// A stand-in shape to reuse the handles of shapes in documents.
fn stand_in(f: &Frame, line: bool) -> Shape {
    let mut s = Shape::new(if line { ShapeKind::Line } else { ShapeKind::Rect });
    s.rotation = f.rot;
    s.flip_h = f.flip_h;
    s.flip_v = f.flip_v;
    s
}

fn geo(f: &Frame) -> BoxGeo {
    BoxGeo { x: f.x, y: f.y, w: f.w, h: f.h, rotation: f.rot, flip_h: f.flip_h, flip_v: f.flip_v }
}

fn frame_of(g: BoxGeo) -> Frame {
    Frame { x: g.x, y: g.y, w: g.w, h: g.h, rot: g.rotation, flip_h: g.flip_h, flip_v: g.flip_v }
}

/// The top-level object of `objs` under screen point `q` (the topmost).
pub fn hit(ch: &Chain, objs: &[Object], xf: Xf, q: Pos2) -> Option<u32> {
    for o in objs.iter().rev().filter(|o| !o.hidden) {
        let f = ch.frame(o);
        let rect = xf.rect(&f);
        if !rect.is_finite() {
            continue;
        }
        let s = stand_in(&f, is_line(o, ch));
        if shapes::hit(&s, rect, q) {
            return Some(o.id);
        }
    }
    None
}

impl SlidesView {
    /// The canvas: the slide, the selection and the pointer on it.
    pub(super) fn canvas_area(&mut self, ui: &mut egui::Ui, size: Vec2) {
        let full = vec2(size.x.max(ui.available_width()), size.y.max(ui.available_height()));
        let (area, resp) = ui.allocate_exact_size(full, Sense::click_and_drag());
        let r = Rect::from_center_size(area.center(), size);
        let xf = Xf::for_slide(&self.pres.deck, r);
        self.canvas_xf = Some(xf);
        let p = ui.painter_at(area);
        p.rect_filled(r.translate(vec2(2.0, 3.0)), 0.0, Color32::from_black_alpha(40));
        let typing_id = self.typing.as_ref().map(|t| t.id);
        let target = self.target();
        if objects::chain(&self.pres.deck, target).is_none() {
            return;
        }
        let opts = render::Opts { edit: true, typing: typing_id, ..Default::default() };
        // The slide goes dark with the dark workspace (screen only).
        crate::theme::dark_paint(&p, || match target {
            Target::Slide(i) => render::paint_slide(&p, &self.pres.deck, &self.pres.slides[i], r, opts),
            Target::Layout(m, l) => render::paint_layout(&p, &self.pres.deck, m, Some(l), r, opts),
            Target::Master(m) => render::paint_layout(&p, &self.pres.deck, m, None, r, opts),
        });
        p.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_gray(200)), egui::StrokeKind::Outside);
        self.paint_canvas_media(&p, xf);
        let Some(ch) = objects::chain(&self.pres.deck, target) else { return };
        let objs = self.cur_objects();

        // Where the text being typed is.
        let typing_cell = self.typing.as_ref().and_then(|t| t.cell);
        let placed = typing_id.and_then(|id| objs.iter().find(|o| o.id == id)).and_then(|o| {
            if let Some(rc) = typing_cell {
                return render::cell_text_placed(&p, &ch, o, xf, rc);
            }
            let empty = TextBody::plain("");
            let mut t = o.text().cloned().unwrap_or(empty);
            if t.paras.is_empty() {
                t = TextBody::plain("");
            }
            Some(render::text_placed(&p, &ch, o, &t, xf))
        });
        let sel_frames: Vec<(Frame, bool)> = objs.iter().filter(|o| self.sel.contains(&o.id)).map(|o| (ch.frame(o), is_line(o, &ch))).collect();
        let hover = resp.hover_pos();
        let hover_hit = hover.and_then(|q| hit(&ch, objs, xf, q));
        if let Some(t) = &mut self.typing {
            t.placed = placed;
            if let Some((q, word)) = t.pending.take() {
                if word {
                    self.typing_word(q);
                } else {
                    self.typing_click(q, false);
                }
            }
        }

        // The selection's outline and handles.
        let accent = theme::SLIDES_ACCENT;
        for (f, line) in &sel_frames {
            let rect = xf.rect(f);
            let s = stand_in(f, *line);
            if sel_frames.len() == 1 && self.typing.is_none() {
                shapes::paint_handles(&p, &s, rect, accent);
            } else if !line {
                crate::images::outline(&p, rect, f.rot, Stroke::new(if self.typing.is_some() { 1.0 } else { 1.5 }, accent));
            }
        }
        if self.typing.is_some() {
            self.paint_caret(&p, ui.input(|i| i.time));
            ui.ctx().request_repaint_after(web_time::Duration::from_millis(500));
        }

        // The pointer's look.
        if let Some(q) = hover {
            let icon = if self.tool.is_some() {
                CursorIcon::Crosshair
            } else if self.brush.is_some() {
                CursorIcon::Copy
            } else if let ([(f, line)], None) = (sel_frames.as_slice(), &self.typing) {
                shapes::handle_at(&stand_in(f, *line), xf.rect(f), q).map(|h| shapes::cursor(h, f.rot)).unwrap_or(if hover_hit.is_some() { CursorIcon::Move } else { CursorIcon::Default })
            } else if self.typing.as_ref().is_some_and(|t| Some(t.id) == hover_hit) {
                CursorIcon::Text
            } else if hover_hit.is_some() {
                CursorIcon::Move
            } else {
                CursorIcon::Default
            };
            ui.ctx().set_cursor_icon(icon);
        }

        if self.read_only {
            // Only selecting.
            if resp.clicked()
                && let Some(q) = resp.interact_pointer_pos()
            {
                self.sel = hover_hit.or_else(|| self.hit_at(xf, q)).into_iter().collect();
            }
            return;
        }
        let shift = ui.input(|i| i.modifiers.shift);
        if self.brush.is_some() {
            self.brush_mouse(&resp, xf, &p);
            return;
        }
        if resp.drag_started()
            && let Some(q) = ui.input(|i| i.pointer.press_origin())
        {
            let cell = self.cell_under(&p, xf, q);
            self.press(q, xf, shift, cell);
        }
        if resp.dragged()
            && let Some(q) = resp.interact_pointer_pos()
        {
            self.drag_to(q, xf, shift, &p);
        }
        if resp.drag_stopped() {
            self.release(ui.input(|i| i.pointer.interact_pos()), xf, shift);
        }
        if resp.double_clicked()
            && let Some(q) = resp.interact_pointer_pos()
        {
            let cell = self.cell_under(&p, xf, q);
            self.double_click(q, xf, cell);
        } else if resp.clicked()
            && let Some(q) = resp.interact_pointer_pos()
        {
            let cell = self.cell_under(&p, xf, q);
            self.click(q, xf, shift, cell);
        }
        if resp.secondary_clicked()
            && let Some(q) = resp.interact_pointer_pos()
            && let Some(id) = self.hit_at(xf, q)
            && !self.sel.contains(&id)
        {
            self.stop_typing();
            self.sel = vec![id];
        }
        resp.context_menu(|ui| self.object_menu(ui));
    }

    /// The mouse with the format painter on: a click on an object paints
    /// all its text, in the text being typed only the word clicked; a drag
    /// over text paints what it goes over. Nothing moves.
    fn brush_mouse(&mut self, resp: &egui::Response, xf: Xf, p: &egui::Painter) {
        if resp.drag_started()
            && let Some(q) = resp.ctx.input(|i| i.pointer.press_origin())
            && let Some(id) = self.hit_at(xf, q)
            && self.object(id).is_some_and(typing::can_type)
        {
            if self.typing.as_ref().is_some_and(|t| t.id == id) {
                self.typing_click(q, false);
            } else {
                self.stop_typing();
                self.start_typing(id);
                if let Some(t) = &mut self.typing {
                    t.pending = Some((q, false));
                }
            }
            self.drag = Some(Drag::Text);
        }
        if resp.dragged()
            && let Some(q) = resp.interact_pointer_pos()
        {
            self.drag_to(q, xf, false, p);
        }
        if resp.drag_stopped() && self.drag.take().is_some() {
            self.apply_brush();
        }
        if resp.clicked()
            && let Some(q) = resp.interact_pointer_pos()
            && let Some(id) = self.hit_at(xf, q)
        {
            if self.typing.as_ref().is_some_and(|t| t.id == id) {
                self.typing_click(q, false);
            } else {
                self.stop_typing();
                self.sel = vec![id];
            }
            self.apply_brush();
        }
    }

    fn hit_at(&self, xf: Xf, q: Pos2) -> Option<u32> {
        hit(&objects::chain(&self.pres.deck, self.target())?, self.cur_objects(), xf, q)
    }

    pub(super) fn object(&self, id: u32) -> Option<&Object> {
        self.cur_objects().iter().find(|o| o.id == id)
    }

    /// The cell of the table under screen point `q`, if a table is there.
    fn cell_under(&self, p: &egui::Painter, xf: Xf, q: Pos2) -> Option<(usize, usize)> {
        let o = self.object(self.hit_at(xf, q)?)?;
        let ch = objects::chain(&self.pres.deck, self.target())?;
        render::cell_at(p, &ch, o, xf, q)
    }

    /// Types in cell `rc` of table `id`, the caret where `q` is (the word
    /// under it with `word`).
    fn type_in_cell(&mut self, id: u32, rc: (usize, usize), q: Pos2, word: bool) {
        if self.typing.as_ref().is_some_and(|t| t.id == id && t.cell == Some(rc)) {
            if word {
                self.typing_word(q);
            } else {
                self.typing_click(q, false);
            }
            return;
        }
        self.start_typing_cell(id, rc.0, rc.1);
        if let Some(t) = &mut self.typing {
            t.pending = Some((q, word));
        }
    }

    fn click(&mut self, q: Pos2, xf: Xf, shift: bool, cell: Option<(usize, usize)>) {
        if let Some(tool) = self.tool.take() {
            // A click draws the object at its usual size.
            let (x, y) = xf.back(q);
            let (w, h) = match tool {
                Tool::TextBox => (144.0, 30.0),
                t if t.is_line() => (72.0, 0.0),
                _ => (72.0, 72.0),
            };
            self.insert_tool(tool, Frame::new(x, y, w, h));
            return;
        }
        let at = self.hit_at(xf, q);
        if let Some((id, typing_cell)) = self.typing.as_ref().map(|t| (t.id, t.cell)) {
            if at == Some(id) {
                match cell {
                    Some(rc) if typing_cell.is_some() && !shift => self.type_in_cell(id, rc, q, false),
                    _ => self.typing_click(q, shift),
                }
                return;
            }
            self.stop_typing();
        }
        let Some(id) = at else {
            self.sel.clear();
            return;
        };
        if shift {
            if let Some(k) = self.sel.iter().position(|x| *x == id) {
                self.sel.remove(k);
            } else {
                self.sel.push(id);
            }
            return;
        }
        let Some(o) = self.object(id) else { return };
        // A click on a selected table types in the cell clicked.
        if let Some(rc) = cell
            && self.sel == [id]
        {
            self.type_in_cell(id, rc, q, false);
            return;
        }
        // A click on a selected shape, or on an empty placeholder, types.
        let empty_ph = o.ph.is_some() && o.text().is_none_or(TextBody::is_empty);
        if typing::can_type(o) && (self.sel == [id] || empty_ph) {
            self.start_typing(id);
            if let Some(t) = &mut self.typing {
                t.pending = Some((q, false));
            }
        } else {
            self.sel = vec![id];
        }
    }

    fn double_click(&mut self, q: Pos2, xf: Xf, cell: Option<(usize, usize)>) {
        let Some(id) = self.hit_at(xf, q) else { return };
        let Some(o) = self.object(id) else { return };
        match &o.kind {
            ObjKind::Math(eq) => {
                let src = eq.source().to_string();
                self.equation = Some((Some(id), src.clone(), src.chars().count(), true));
            }
            ObjKind::Chart(_) => self.open_chart_data(id),
            ObjKind::Table(_) => {
                if let Some(rc) = cell {
                    self.type_in_cell(id, rc, q, true);
                }
            }
            ObjKind::Diagram(_) => {
                self.sel = vec![id];
                self.open_text_pane();
            }
            _ if typing::can_type(o) => {
                if self.typing.as_ref().is_none_or(|t| t.id != id) {
                    self.start_typing(id);
                }
                if let Some(t) = &mut self.typing {
                    t.pending = Some((q, true));
                }
            }
            _ => {}
        }
    }

    fn press(&mut self, q: Pos2, xf: Xf, shift: bool, cell: Option<(usize, usize)>) {
        let (x, y) = xf.back(q);
        if let Some(tool) = self.tool {
            self.drag = Some(Drag::Draw { start: pos2(x, y), tool });
            return;
        }
        let at = self.hit_at(xf, q);
        if let Some(t) = &self.typing {
            if at == Some(t.id) {
                // A press in the selected text drags it.
                if (cell.is_none() || cell == t.cell) && self.in_typing_selection(q) {
                    self.drag = Some(Drag::MoveText { to: None, copy: false });
                    return;
                }
                match cell {
                    Some(rc) if t.cell.is_some() => self.type_in_cell(t.id, rc, q, false),
                    _ => self.typing_click(q, false),
                }
                self.drag = Some(Drag::Text);
                return;
            }
            self.stop_typing();
        }
        // A handle of the one selected object.
        if let [id] = self.selection()[..]
            && let Some((_, f)) = self.frames_of(&[id]).first().copied()
            && let Some(o) = self.object(id)
        {
            let Some(ch) = objects::chain(&self.pres.deck, self.target()) else { return };
            let line = is_line(o, &ch);
            if let Some(h) = shapes::handle_at(&stand_in(&f, line), xf.rect(&f), q) {
                self.drag = Some(Drag::Objects { handle: h, start: pos2(x, y), orig: vec![(id, f)], moved: false });
                return;
            }
        }
        match at {
            Some(id) => {
                if !self.sel.contains(&id) {
                    if shift {
                        self.sel.push(id);
                    } else {
                        self.sel = vec![id];
                    }
                }
                let orig = self.frames_of(&self.selection());
                self.drag = Some(Drag::Objects { handle: Handle::Move, start: pos2(x, y), orig, moved: false });
            }
            None => {
                if !shift {
                    self.sel.clear();
                }
                self.drag = Some(Drag::Marquee { start: q });
            }
        }
    }

    fn drag_to(&mut self, q: Pos2, xf: Xf, shift: bool, p: &egui::Painter) {
        if let Some(Drag::MoveText { .. }) = self.drag {
            // Esc drops nothing.
            if p.ctx().input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
                self.drag = None;
                return;
            }
            let copy = p.ctx().input(|i| crate::doc_view::text_drag::copy_modifier(&i.modifiers));
            p.ctx().set_cursor_icon(if copy { egui::CursorIcon::Copy } else { egui::CursorIcon::Grabbing });
            let to = self.typing_pos(q);
            if let Some(to) = to {
                self.paint_text_drop(p, to);
            }
            self.drag = Some(Drag::MoveText { to, copy });
            return;
        }
        let (x, y) = xf.back(q);
        let accent = theme::SLIDES_ACCENT;
        match &mut self.drag {
            Some(Drag::Objects { handle, start, orig, moved }) => {
                let d = vec2(x - start.x, y - start.y);
                if !*moved && d.length() * xf.sx < 1.0 {
                    return;
                }
                let frames: Vec<(u32, Frame)> = match *handle {
                    Handle::Move => orig
                        .iter()
                        .map(|(id, f)| {
                            let g = shapes::dragged(geo(f), Handle::Move, d, shift, 1.0);
                            (*id, frame_of(g))
                        })
                        .collect(),
                    Handle::Rotate => orig
                        .iter()
                        .map(|(id, f)| {
                            let c = xf.rect(f).center();
                            (*id, Frame { rot: shapes::rotation_to(c, q, shift), ..*f })
                        })
                        .collect(),
                    h => orig.iter().map(|(id, f)| (*id, frame_of(shapes::dragged(geo(f), h, d, shift, 1.0)))).collect(),
                };
                let what = match *handle {
                    Handle::Move => "slides.undo.move_objects",
                    Handle::Rotate => "slides.undo.rotate",
                    _ => "slides.undo.size",
                };
                let join = *moved;
                *moved = true;
                self.set_frames(what, &frames, join);
            }
            Some(Drag::Marquee { start }) => {
                let r = Rect::from_two_pos(*start, q);
                p.rect(r, 0.0, accent.gamma_multiply(0.12), Stroke::new(1.0, accent), egui::StrokeKind::Inside);
            }
            Some(Drag::Text) => self.typing_click(q, true),
            // Handled above.
            Some(Drag::MoveText { .. }) => {}
            Some(Drag::Draw { start, tool }) => {
                let kind = if tool.is_line() { ShapeKind::Line } else { ShapeKind::Rect };
                let g = shapes::drawn(kind, *start, pos2(x, y), shift);
                let r = Rect::from_min_size(xf.pos(g.x, g.y), vec2(g.w * xf.sx, g.h * xf.sy));
                if tool.is_line() {
                    let (a, b) = g.ends();
                    p.line_segment([xf.pos(a.x, a.y), xf.pos(b.x, b.y)], Stroke::new(1.5, accent));
                } else {
                    p.rect_stroke(r, 0.0, Stroke::new(1.0, accent), egui::StrokeKind::Middle);
                }
            }
            None => {}
        }
    }

    fn release(&mut self, q: Option<Pos2>, xf: Xf, shift: bool) {
        let drag = self.drag.take();
        let Some(q) = q else { return };
        match drag {
            Some(Drag::Objects { moved: true, .. }) => self.pres.seal(),
            Some(Drag::MoveText { to: Some(to), copy }) => self.drop_typing_text(to, copy),
            Some(Drag::Marquee { start }) => {
                let r = Rect::from_two_pos(start, q);
                let Some(ch) = objects::chain(&self.pres.deck, self.target()) else { return };
                let mut add = Vec::new();
                for o in self.cur_objects().iter().filter(|o| !o.hidden) {
                    let (bx, by, bw, bh) = ch.frame(o).bounds();
                    let b = Rect::from_min_max(xf.pos(bx, by), xf.pos(bx + bw, by + bh));
                    if r.contains_rect(b) && !self.sel.contains(&o.id) {
                        add.push(o.id);
                    }
                }
                self.sel.extend(add);
            }
            Some(Drag::Draw { start, tool }) => {
                self.tool = None;
                let (x, y) = xf.back(q);
                let kind = if tool.is_line() { ShapeKind::Line } else { ShapeKind::Rect };
                let g = shapes::drawn(kind, start, pos2(x, y), shift);
                self.insert_tool(tool, frame_of(g));
            }
            _ => {}
        }
    }

    /// Inserts what `tool` draws in `frame`.
    pub fn insert_tool(&mut self, tool: Tool, frame: Frame) {
        self.tool = None;
        let mut f = frame;
        if !tool.is_line() {
            f.w = f.w.max(4.0);
            f.h = f.h.max(4.0);
        }
        match tool {
            Tool::TextBox => {
                f.h = f.h.max(24.0);
                self.insert_text_box(f);
            }
            Tool::Shape { preset, arrow } => {
                self.insert_shape(preset, f, arrow);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits_the_topmost() {
        let mut v = SlidesView::new(apex_slides::build::new_deck(0));
        v.set_layout(0, 5);
        let a = v.insert_shape("rect", Frame::new(0.0, 0.0, 100.0, 100.0), false).unwrap();
        let b = v.insert_shape("ellipse", Frame::new(50.0, 50.0, 100.0, 100.0), false).unwrap();
        let line = v.insert_shape("line", Frame::new(300.0, 300.0, 100.0, 0.0), false).unwrap();
        let s = &v.pres.slides[0];
        let ch = Chain::for_slide(&v.pres.deck, s);
        let xf = Xf { ox: 0.0, oy: 0.0, sx: 1.0, sy: 1.0 };
        assert_eq!(hit(&ch, &s.objects, xf, pos2(10.0, 10.0)), Some(a));
        assert_eq!(hit(&ch, &s.objects, xf, pos2(75.0, 75.0)), Some(b));
        assert_eq!(hit(&ch, &s.objects, xf, pos2(350.0, 302.0)), Some(line));
        assert_eq!(hit(&ch, &s.objects, xf, pos2(350.0, 320.0)), None);
    }

    #[test]
    fn tools_from_the_gallery() {
        let t = shapes::Template { kind: ShapeKind::Line, arrow: true, preset: None };
        assert_eq!(Tool::of(t), Tool::Shape { preset: "straightConnector1", arrow: true });
        assert!(Tool::of(t).is_line());
        let t = shapes::Template { kind: ShapeKind::TextBox, arrow: false, preset: None };
        assert_eq!(Tool::of(t), Tool::TextBox);
        let t = shapes::Template { kind: ShapeKind::Rect, arrow: false, preset: Some("cloud") };
        assert_eq!(Tool::of(t), Tool::Shape { preset: "cloud", arrow: false });
    }

    #[test]
    fn drawing_inserts() {
        let mut v = SlidesView::new(apex_slides::build::new_deck(0));
        v.insert_tool(Tool::Shape { preset: "star5", arrow: false }, Frame::new(10.0, 10.0, 1.0, 1.0));
        let o = v.selected_object().unwrap();
        assert_eq!(o.sp.frame.unwrap().w, 4.0);
        v.insert_tool(Tool::TextBox, Frame::new(10.0, 10.0, 100.0, 5.0));
        assert!(v.typing.is_some());
    }
}
