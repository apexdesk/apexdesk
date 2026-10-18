//! Shapes and text boxes over the cells: painting, drawing them from the
//! gallery, selection, moving, resizing and turning them, their text, the
//! keyboard, the clipboard and the Format Shape dialog. They lie above
//! charts and share the picture entry points (see images_ui.rs).

use apex_sheet::{Anchor, CellRef, Sheet, SheetShape};
use eframe::egui::{self, CursorIcon, Event, Key, Modifiers, Pos2, Rect, vec2};

use super::images_ui::{pane_at, to_screen};
use super::{Geo, SheetView};
use crate::i18n::tr;
use crate::shapes::{self, BoxGeo, EditResult, FormatDlg, Handle, PX_PT, Template, TextEdit};
use crate::theme;
use crate::widgets::DialogResult;

/// Centimeters per sheet pixel (96 dpi).
const CM: f32 = 2.54 / 96.0;
const MIN_SIZE: f32 = 2.0;

impl ShapesState {
    pub(super) fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The shape the next drag draws, when one was picked.
    pub(super) fn armed(&self) -> Option<&crate::shapes::Template> {
        self.draw.as_ref()
    }
}

#[derive(Default)]
pub(super) struct ShapesState {
    /// Selected shape: sheet and id.
    pub(super) selected: Option<(usize, u32)>,
    /// A shape being dragged, edited or formatted, painted instead of the
    /// stored one.
    preview: Option<SheetShape>,
    drag: Option<ShapeDrag>,
    /// The shape picked in the gallery, drawn by the next drag.
    draw: Option<Template>,
    /// Drawing it: pane and the two corners, sheet pixels.
    drawing: Option<((bool, bool), Pos2, Pos2)>,
    menu: bool,
    dialog: Option<Dlg>,
    /// Last copied shape and the text put on the clipboard for it.
    clip: Option<(SheetShape, String)>,
    /// Text being typed into a shape: its id and the editor.
    edit: Option<(u32, TextEdit)>,
    hovering: bool,
    /// The Draw tab's tools, the pane the pointer draws in and the ink
    /// object drawn last (see ink_ui.rs).
    pub(super) ink: crate::ink::InkState,
    pub(super) ink_pane: (bool, bool),
    pub(super) ink_last: Option<u32>,
    /// Form controls: their dialog and open drop-down (see controls_ui.rs).
    pub(super) controls: super::controls_ui::ControlsState,
}

struct ShapeDrag {
    handle: Handle,
    start: Pos2,
    orig: SheetShape,
    moved: bool,
}

struct Dlg {
    sheet: usize,
    orig: SheetShape,
    d: FormatDlg,
    anchor: Anchor,
}

/// Sheet point under screen point `p` in a pane.
fn to_sheet(g: &Geo, pane: (bool, bool), p: Pos2) -> Pos2 {
    g.screen_to_point(pane, p)
}

fn home_pane(g: &Geo, sh: &Sheet, s: &SheetShape) -> (bool, bool) {
    let (x, y) = s.pos(sh);
    (g.fc > 0 && x < g.ox + g.fx, g.fr > 0 && y < g.oy + g.fy)
}

/// Unturned screen rectangle of a shape in a pane.
fn screen_rect(g: &Geo, pane: (bool, bool), sh: &Sheet, s: &SheetShape) -> Rect {
    let [x, y, w, h] = s.rect(sh);
    Rect::from_min_size(to_screen(g, sh, pane.0, pane.1, x, y), vec2(w, h) * g.z)
}

/// `orig` with the box `b` (sheet pixels).
fn with_box(sh: &Sheet, orig: &SheetShape, b: BoxGeo) -> SheetShape {
    let mut s = orig.clone();
    b.apply(&mut s.shape);
    s.width = b.w;
    s.height = b.h;
    s.set_pos(sh, b.x, b.y);
    s
}

fn box_of(sh: &Sheet, s: &SheetShape) -> BoxGeo {
    let (x, y) = s.pos(sh);
    BoxGeo::of(&s.shape, x, y, s.width, s.height)
}

impl SheetView {
    pub(super) fn selected_shape(&self) -> Option<&SheetShape> {
        let (s, id) = self.images.shapes.selected?;
        (s == self.sheet).then(|| self.sh().shapes.iter().find(|x| x.id == id)).flatten()
    }

    pub(super) fn shape_selected(&self) -> bool {
        self.selected_shape().is_some()
    }

    pub(super) fn shape_hovered(&self) -> bool {
        self.images.shapes.hovering || self.images.shapes.draw.is_some() || self.images.shapes.ink.active()
    }

    /// Shapes to paint even on a sheet with no other objects.
    pub(super) fn shapes_to_paint(&self) -> bool {
        !self.sh().shapes.is_empty() || self.images.shapes.drawing.is_some()
    }

    fn shown_shape<'a>(&'a self, s: &'a SheetShape) -> &'a SheetShape {
        let st = &self.images.shapes;
        match &st.preview {
            Some(p) if p.id == s.id && st.selected.is_some_and(|(sh, _)| sh == self.sheet) => p,
            _ => s,
        }
    }

    pub(super) fn select_shape(&mut self, id: u32) {
        self.images.shapes.selected = Some((self.sheet, id));
        self.images.deselect_picture();
        self.images.charts.deselect();
        self.marquee = false;
    }

    fn scale(g: &Geo) -> f32 {
        PX_PT * g.z
    }

    // --- painting ----------------------------------------------------------------------

    /// Paints the shapes that show in `clip` (screen) of one pane.
    pub(super) fn paint_shapes_in(&self, p: &egui::Painter, g: &Geo, pane: (bool, bool), clip: Rect, wait: bool) {
        let sh = self.sh();
        let editing = self.images.shapes.edit.as_ref().map(|(id, _)| *id);
        for stored in &sh.shapes {
            let s = self.shown_shape(stored);
            let [x0, y0, x1, y1] = s.bounds(sh);
            // Room for the outline and arrowheads.
            let m = s.shape.line_width * PX_PT * 3.0 + 4.0;
            let r = Rect::from_min_max(to_screen(g, sh, pane.0, pane.1, x0 - m, y0 - m), to_screen(g, sh, pane.0, pane.1, x1 + m, y1 + m));
            if !r.intersects(clip) {
                continue;
            }
            if let Some(c) = &s.control {
                self.paint_control(p, screen_rect(g, pane, sh, s), s, c, g.z);
                continue;
            }
            let text = wait || editing != Some(s.id);
            let shown = self.images.shapes.ink.shown(&s.shape);
            shapes::paint(p, screen_rect(g, pane, sh, s), &shown, Self::scale(g), text);
        }
        if let (Some(t), Some((dp, a, b)), false) = (self.images.shapes.draw, self.images.shapes.drawing, wait) {
            if dp == pane {
                let shift = p.ctx().input(|i| i.modifiers.shift);
                let bx = shapes::drawn(t.kind, a, b, shift);
                let mut s = t.shape();
                bx.apply(&mut s);
                let rect = Rect::from_min_size(to_screen(g, sh, pane.0, pane.1, bx.x, bx.y), vec2(bx.w, bx.h) * g.z);
                shapes::paint(p, rect, &s, Self::scale(g), false);
            }
        }
    }

    pub(super) fn paint_shape_handles(&self, painter: &egui::Painter, g: &Geo) {
        self.paint_ink_live(painter, g);
        let Some(stored) = self.selected_shape() else { return };
        let sh = self.sh();
        let s = self.shown_shape(stored);
        let rect = screen_rect(g, home_pane(g, sh, s), sh, s);
        shapes::paint_handles(&painter.with_clip_rect(g.view), &s.shape, rect, theme::SHEET_ACCENT);
    }

    // --- pointer -----------------------------------------------------------------------

    /// The shape (topmost first) and handle under screen point `p`.
    fn shape_hit(&self, g: &Geo, p: Pos2) -> Option<(u32, Handle)> {
        let sh = self.sh();
        if let Some(stored) = self.selected_shape() {
            let s = self.shown_shape(stored);
            let rect = screen_rect(g, home_pane(g, sh, s), sh, s);
            if let Some(h) = shapes::handle_at(&s.shape, rect, p) {
                return Some((s.id, h));
            }
        }
        let pane = pane_at(g, p);
        sh.shapes.iter().rev().map(|s| self.shown_shape(s)).find(|s| shapes::hit(&s.shape, screen_rect(g, pane, sh, s), p) && Self::control_hit(s, screen_rect(g, pane, sh, s), p)).map(|s| (s.id, Handle::Move))
    }

    /// Mouse handling for shapes; true when it took the event.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn shape_pointer(&mut self, ui: &egui::Ui, resp: &egui::Response, g: &Geo, p: Pos2, pressed: bool, down: bool, mods: Modifiers) -> bool {
        if self.images.shapes.ink.active() && self.ink_pointer(ui, resp, g, p, pressed, down) {
            return true;
        }
        if let Some(t) = self.images.shapes.draw {
            return self.draw_pointer(ui, resp, g, p, pressed, down, mods, t);
        }
        if let Some(d) = &mut self.images.shapes.drag {
            if !d.moved && (p - d.start).length() > 3.0 {
                d.moved = true;
            }
            let (handle, start, moved) = (d.handle, d.start, d.moved);
            if down {
                if moved {
                    let orig = d.orig.clone();
                    let sh = self.sh();
                    let s = if handle == Handle::Rotate {
                        let rect = screen_rect(g, home_pane(g, sh, &orig), sh, &orig);
                        let mut s = orig.clone();
                        s.shape.rotation = shapes::rotation_to(rect.center(), p, mods.shift);
                        s
                    } else {
                        let min = if orig.shape.kind.is_line() { 0.0 } else { MIN_SIZE };
                        with_box(sh, &orig, shapes::dragged(box_of(sh, &orig), handle, (p - start) / g.z, mods.shift, min))
                    };
                    self.images.shapes.preview = Some(s);
                    ui.ctx().request_repaint();
                }
                return true;
            }
            let d = self.images.shapes.drag.take();
            if let (Some(s), true) = (self.images.shapes.preview.take(), moved) {
                self.wb.set_shape(self.sheet, s);
            } else if d.is_some() && resp.double_clicked() {
                self.edit_shape_text(None);
            }
            return true;
        }
        let target = if resp.hovered() && g.view.contains(p) && !self.formula_ref_point() { self.shape_hit(g, p) } else { None };
        self.images.shapes.hovering = target.is_some();
        if let Some((id, h)) = target {
            let rot = self.sh().shapes.iter().find(|s| s.id == id).map_or(0.0, |s| s.shape.rotation);
            ui.ctx().set_cursor_icon(shapes::cursor(h, rot));
        }
        if pressed && resp.hovered() {
            let Some((id, handle)) = target else {
                self.images.shapes.selected = None;
                return false;
            };
            if self.edit.is_some() && !self.settle_edit() {
                return true;
            }
            resp.request_focus();
            // A plain click operates a form control; Ctrl+click selects it.
            let picked = self.images.shapes.selected == Some((self.sheet, id));
            if handle == Handle::Move && !picked && !mods.command {
                let sh = self.sh();
                if let Some(s) = sh.shapes.iter().find(|s| s.id == id && s.control.is_some()) {
                    let r = screen_rect(g, pane_at(g, p), sh, s);
                    self.images.shapes.selected = None;
                    self.click_control(id, r, p, g.z);
                    return true;
                }
                // A shape with a macro runs it.
                if let Some(m) = sh.shapes.iter().find(|s| s.id == id && !s.macro_name.is_empty()).map(|s| s.macro_name.clone()) {
                    self.images.shapes.selected = None;
                    self.click_macro(&m);
                    return true;
                }
            }
            self.select_shape(id);
            if let Some(orig) = self.sh().shapes.iter().find(|s| s.id == id).cloned() {
                self.images.shapes.drag = Some(ShapeDrag { handle, start: p, orig, moved: false });
            }
            return true;
        }
        false
    }

    /// Drawing the shape picked in the gallery.
    #[allow(clippy::too_many_arguments)]
    fn draw_pointer(&mut self, ui: &egui::Ui, resp: &egui::Response, g: &Geo, p: Pos2, pressed: bool, down: bool, mods: Modifiers, t: Template) -> bool {
        let st = &mut self.images.shapes;
        if let Some((pane, a, _)) = st.drawing {
            let b = to_sheet(g, pane, p);
            if down {
                st.drawing = Some((pane, a, b));
                ui.ctx().request_repaint();
                return true;
            }
            st.drawing = None;
            st.draw = None;
            let bx = if (b - a).length() * g.z < 4.0 {
                // A click: the default size.
                let (w, h) = shapes::default_size(t.kind);
                shapes::drawn(t.kind, a, a + vec2(w, h) * PX_PT, false)
            } else {
                shapes::drawn(t.kind, a, b, mods.shift)
            };
            let mut s = SheetShape::new(t.shape(), CellRef::new(0, 0), bx.w, bx.h);
            bx.apply(&mut s.shape);
            s.set_pos(self.sh(), bx.x, bx.y);
            let id = self.wb.add_shape(self.sheet, s);
            if self.sh().shapes.iter().any(|s| s.id == id) {
                self.select_shape(id);
                if t.kind == apex_doc::ShapeKind::TextBox {
                    self.edit_shape_text(None);
                }
            }
            return true;
        }
        if !(resp.hovered() && g.view.contains(p)) {
            return false;
        }
        ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        if pressed {
            if self.edit.is_some() && !self.settle_edit() {
                return true;
            }
            resp.request_focus();
            let pane = pane_at(g, p);
            let a = to_sheet(g, pane, p);
            self.images.shapes.drawing = Some((pane, a, a));
        }
        true
    }

    /// Right click: true when it was on a shape.
    pub(super) fn shape_menu_hit(&mut self, g: &Geo, p: Pos2) -> bool {
        self.images.shapes.draw = None;
        let hit = if g.view.contains(p) && !self.formula_ref_point() { self.shape_hit(g, p) } else { None };
        self.images.shapes.menu = hit.is_some();
        match hit {
            Some((id, _)) => self.select_shape(id),
            None => self.images.shapes.selected = None,
        }
        self.images.shapes.menu
    }

    pub(super) fn shape_menu_open(&self) -> bool {
        self.images.shapes.menu && self.shape_selected()
    }

    pub(super) fn shape_context_menu(&mut self, ui: &mut egui::Ui) {
        if ui.add(egui::Button::new(tr("edit.cut")).shortcut_text("Ctrl+X")).clicked() {
            self.copy_shape(ui.ctx(), true);
            ui.close();
        }
        if ui.add(egui::Button::new(tr("edit.copy")).shortcut_text("Ctrl+C")).clicked() {
            self.copy_shape(ui.ctx(), false);
            ui.close();
        }
        if ui.add(egui::Button::new(tr("shape.delete")).shortcut_text("Del")).clicked() {
            self.delete_shape();
            ui.close();
        }
        if self.selected_shape().is_some_and(|s| s.control.is_none()) && ui.button(format!("{}…", tr("macros.assign"))).clicked() {
            if let Some((_, id)) = self.images.shapes.selected {
                self.open_assign_macro(super::macro_hooks::Clickable::Shape(id));
            }
            ui.close();
        }
        if self.selected_shape().is_some_and(|s| s.shape.can_have_text() && s.control.is_none()) {
            ui.separator();
            if ui.button(tr("shape.edit_text")).clicked() {
                self.edit_shape_text(None);
                ui.close();
            }
        }
        ui.separator();
        shapes::order_menu(ui, |to| {
            if let Some((_, id)) = self.images.shapes.selected {
                self.wb.reorder_shape(self.sheet, id, to);
            }
        });
        ui.separator();
        let key = if self.selected_shape().is_some_and(|s| s.control.is_some()) { "control.format" } else { "shape.format" };
        if ui.button(format!("{}…", tr(key))).clicked() {
            self.open_shape_dialog(0);
            ui.close();
        }
    }

    // --- keyboard ----------------------------------------------------------------------

    /// Keys for shapes; true when the grid must not see the events.
    pub(super) fn shape_keys(&mut self, ui: &egui::Ui, events: &[Event]) -> bool {
        if self.ink_keys(ui.ctx(), events) {
            return true;
        }
        let st = &mut self.images.shapes;
        if st.edit.is_some() || st.dialog.is_some() {
            return true;
        }
        if st.draw.is_some() && events.iter().any(|e| matches!(e, Event::Key { key: Key::Escape, pressed: true, .. })) {
            st.draw = None;
            st.drawing = None;
            return true;
        }
        // Our own copy comes back as its text: paste the shape.
        // (A shape copied in another workbook is in the app's clipboard.)
        let ours = events.iter().find_map(|e| match e {
            Event::Paste(t) => match &st.clip {
                Some((s, c)) if c == t => Some(s.clone()),
                _ => match crate::clip_history::find(t)?.rich {
                    crate::clip_history::Rich::Shape(s) => Some((*s).clone()),
                    _ => None,
                },
            },
            _ => None,
        });
        if let Some(s) = ours {
            self.paste_shape(s);
            return true;
        }
        let Some(s) = self.selected_shape().cloned() else { return false };
        for ev in events {
            match ev {
                Event::Copy => self.copy_shape(ui.ctx(), false),
                Event::Cut => self.copy_shape(ui.ctx(), true),
                Event::Paste(_) => {
                    // Text goes into the cells.
                    self.images.shapes.selected = None;
                    return false;
                }
                Event::Text(t) if s.shape.can_have_text() => {
                    self.edit_shape_text(Some(t));
                    return true;
                }
                Event::Key { key, pressed: true, modifiers: m, .. } => {
                    let step = if m.shift { 10.0 } else { 1.0 };
                    let nudge = match key {
                        Key::ArrowLeft => Some(vec2(-step, 0.0)),
                        Key::ArrowRight => Some(vec2(step, 0.0)),
                        Key::ArrowUp => Some(vec2(0.0, -step)),
                        Key::ArrowDown => Some(vec2(0.0, step)),
                        _ => None,
                    };
                    if let Some(d) = nudge {
                        let mut moved = s.clone();
                        let (x, y) = s.pos(self.sh());
                        moved.set_pos(self.sh(), x + d.x, y + d.y);
                        self.wb.set_shape(self.sheet, moved);
                        continue;
                    }
                    match key {
                        Key::Delete | Key::Backspace => self.delete_shape(),
                        Key::Escape => self.images.shapes.selected = None,
                        Key::Z if m.command && m.shift => self.redo(),
                        Key::Z if m.command => self.undo(),
                        Key::Y if m.command => self.redo(),
                        Key::Num1 if m.command => self.open_shape_dialog(0),
                        Key::Enter | Key::F2 if s.shape.can_have_text() => self.edit_shape_text(None),
                        Key::Enter => self.open_shape_dialog(0),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        true
    }

    fn delete_shape(&mut self) {
        if let Some((s, id)) = self.images.shapes.selected.take() {
            if s == self.sheet {
                self.wb.remove_shape(self.sheet, id);
            }
        }
    }

    // --- clipboard ---------------------------------------------------------------------

    /// Copies the selected shape: its text goes to the clipboard, the shape
    /// stays here to be pasted back.
    pub(super) fn copy_shape(&mut self, ctx: &egui::Context, cut: bool) {
        let Some(s) = self.selected_shape().cloned() else { return };
        let mut text = s.shape.plain_text();
        if text.trim().is_empty() {
            text = s.shape.name.clone();
        }
        if text.is_empty() {
            text = tr(s.shape.kind.key()).to_string();
        }
        ctx.copy_text(text.clone());
        crate::clip_history::push(crate::clip_history::Entry { text: text.clone(), rich: crate::clip_history::Rich::Shape(std::sync::Arc::new(s.clone())) });
        self.images.shapes.clip = Some((s, text));
        if cut {
            self.delete_shape();
        }
    }

    pub(super) fn paste_shape(&mut self, mut s: SheetShape) {
        self.settle_edit();
        let sh = self.sh();
        s.set_pos(sh, sh.col_x(self.cursor.col), sh.row_y(self.cursor.row));
        s.shape.name.clear();
        let id = self.wb.add_shape(self.sheet, s);
        if self.sh().shapes.iter().any(|s| s.id == id) {
            self.select_shape(id);
        }
    }

    // --- inserting and text ------------------------------------------------------------

    /// The next drag on the grid draws `t`.
    pub(super) fn start_drawing(&mut self, t: Template) {
        self.settle_edit();
        self.images.shapes.draw = Some(t);
        self.images.shapes.drawing = None;
        self.images.shapes.selected = None;
    }

    /// Starts typing into the selected shape; `typed`: what was typed.
    fn edit_shape_text(&mut self, typed: Option<&str>) {
        let Some(s) = self.selected_shape().cloned() else { return };
        if s.control.is_some() {
            if typed.is_none() {
                self.open_control_dialog();
            }
            return;
        }
        if !s.shape.can_have_text() {
            self.open_shape_dialog(0);
            return;
        }
        let e = TextEdit::new(&s.shape, typed);
        self.images.shapes.edit = Some((s.id, e));
        self.images.shapes.preview = Some(s);
    }

    /// The format at the caret of the shape being typed into, as a
    /// cell's (for the ribbon).
    pub(super) fn shape_text_style(&self) -> Option<apex_sheet::CellStyle> {
        let (id, e) = self.images.shapes.edit.as_ref()?;
        let s = self.images.shapes.preview.as_ref().filter(|p| p.id == *id)?;
        Some(run_style(&e.rich.format_now(&s.shape.text)))
    }

    /// Formats the selected text of the shape being typed into as `f`
    /// formats cells; false when none is.
    pub(super) fn shape_text_format(&mut self, f: &dyn Fn(&mut apex_sheet::CellStyle)) -> bool {
        let Some((id, e)) = &mut self.images.shapes.edit else { return false };
        let Some(s) = self.images.shapes.preview.as_mut().filter(|p| p.id == *id) else { return false };
        e.rich.format(&mut s.shape.text, &|r| {
            let mut st = run_style(r);
            f(&mut st);
            r.bold = st.bold;
            r.italic = st.italic;
            r.underline = st.underline;
            r.font = st.font.clone();
            r.size = (st.font_size > 0).then(|| st.font_size as f32 / 10.0);
            r.color = st.color;
        });
        true
    }

    /// The text editor over the shape being typed into.
    pub(super) fn shape_editor(&mut self, ui: &mut egui::Ui, g: &Geo) {
        let Some((id, mut e)) = self.images.shapes.edit.take() else { return };
        let Some(stored) = self.sh().shapes.iter().find(|s| s.id == id) else { return };
        let mut s = self.images.shapes.preview.clone().filter(|p| p.id == id).unwrap_or_else(|| stored.clone());
        let sh = self.sh();
        let rect = screen_rect(g, home_pane(g, sh, &s), sh, &s);
        let eid = egui::Id::new(("shape-text", self.uid, id));
        match shapes::text_editor(ui, eid, rect, &mut s.shape, &mut e, Self::scale(g)) {
            EditResult::Editing => {
                self.images.shapes.preview = Some(s);
                self.images.shapes.edit = Some((id, e));
            }
            EditResult::Done => {
                self.images.shapes.preview = None;
                self.wb.set_shape(self.sheet, s);
            }
        }
    }

    // --- dialog ------------------------------------------------------------------------

    /// Changes the look of the selected shape (undoable).
    pub(super) fn change_shape_look(&mut self, f: impl Fn(&mut apex_doc::Shape)) {
        let Some(mut s) = self.selected_shape().cloned() else { return };
        let before = s.clone();
        f(&mut s.shape);
        s.shape.sanitize();
        if s != before {
            self.wb.set_shape(self.sheet, s);
        }
    }

    pub(super) fn open_shape_dialog(&mut self, tab: usize) {
        let Some(s) = self.selected_shape().cloned() else { return };
        if s.control.is_some() {
            self.open_control_dialog();
            return;
        }
        self.images.shapes.preview = Some(s.clone());
        let d = FormatDlg::new(s.shape.clone(), s.width, s.height, tab);
        self.images.shapes.dialog = Some(Dlg { sheet: self.sheet, anchor: s.anchor, orig: s, d });
    }

    pub(super) fn shape_dialog(&mut self, ctx: &egui::Context) {
        let Some(dlg) = &mut self.images.shapes.dialog else { return };
        let Dlg { d, anchor, .. } = dlg;
        let mut res = shapes::format_dialog(ctx, "shape-format", d, CM, (crate::icons::Icon::Freeze, "shape.tab_props"), |ui| {
            ui.label(tr("shape.anchor"));
            ui.add_space(4.0);
            for (v, key) in [(Anchor::TwoCell, "shape.move_size"), (Anchor::OneCell, "shape.move_only"), (Anchor::Absolute, "shape.absolute")] {
                ui.radio_value(anchor, v, tr(key));
            }
        });
        if std::mem::take(&mut dlg.d.fresh) && res == DialogResult::Cancel && !ctx.input(|i| i.key_pressed(Key::Escape)) {
            res = DialogResult::None;
        }
        let mut s = dlg.orig.clone();
        s.shape = dlg.d.shape.clone();
        s.width = dlg.d.w;
        s.height = dlg.d.h;
        s.anchor = dlg.anchor;
        let sheet = dlg.sheet;
        match res {
            DialogResult::None => {
                if self.images.shapes.drag.is_none() {
                    self.images.shapes.preview = Some(s);
                }
            }
            DialogResult::Ok => {
                self.images.shapes.dialog = None;
                self.images.shapes.preview = None;
                if sheet < self.wb.sheets.len() {
                    self.wb.set_shape(sheet, s);
                }
            }
            DialogResult::Cancel => {
                self.images.shapes.dialog = None;
                self.images.shapes.preview = None;
            }
        }
    }

    /// Drops a selection that no longer exists (undo, another sheet).
    pub(super) fn shapes_frame(&mut self) {
        let st = &self.images.shapes;
        let gone = st.selected.is_some() && self.selected_shape().is_none();
        if gone {
            let st = &mut self.images.shapes;
            st.selected = None;
            st.drag = None;
            st.edit = None;
            if st.dialog.is_none() {
                st.preview = None;
            }
        }
    }
}

/// A shape's run as a cell's style: what the ribbon shows and changes.
fn run_style(r: &apex_doc::ShapeRun) -> apex_sheet::CellStyle {
    apex_sheet::CellStyle {
        bold: r.bold,
        italic: r.italic,
        underline: r.underline,
        font: r.font.clone(),
        font_size: (r.size.unwrap_or(apex_doc::SHAPE_TEXT_PT) * 10.0).round().clamp(10.0, 4000.0) as u16,
        color: r.color,
        ..Default::default()
    }
}
