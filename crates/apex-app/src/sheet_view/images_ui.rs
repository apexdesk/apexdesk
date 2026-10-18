//! Floating pictures: painting (over the cells, in every frozen pane and
//! in printed pages), selection handles, moving, resizing and rotating with
//! the mouse, the keyboard, the clipboard, Insert > Picture and the size and
//! properties dialog.

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};

use apex_sheet::{Anchor, ImageFormat, Sheet, SheetImage};
use eframe::egui::{self, Color32, CursorIcon, Event, Key, Modifiers, Pos2, Rect, RichText, Stroke, Vec2, pos2, vec2};

use super::{Geo, SheetView};
use crate::app::Notice;
use crate::i18n::{tr, trf};
use crate::images;
use crate::theme;
use crate::widgets::{self, DialogResult};

/// The last picture copied in any workbook and the hash of the pixels sent
/// to the clipboard, to recognize it when it is pasted back: here or in
/// another workbook it keeps its file, crop and rotation.
static CLIP: std::sync::Mutex<Option<(SheetImage, u64)>> = std::sync::Mutex::new(None);

/// Handle size and hit distance, in screen points.
const HANDLE: f32 = crate::touch::size(7.0);
const GRAB: f32 = 6.0;
/// Distance of the rotation handle above the top edge.
const ROT_ARM: f32 = crate::shapes::ROT_ARM;
const MIN_SIZE: f32 = 4.0;
/// Centimeters per sheet pixel (96 dpi).
const CM: f32 = 2.54 / 96.0;
/// File extensions offered by Insert > Picture.
pub const EXTENSIONS: [&str; 7] = ["png", "jpg", "jpeg", "jpe", "gif", "bmp", "webp"];

#[derive(Default)]
pub(super) struct ImagesState {
    /// Selected picture: sheet and id.
    pub(super) selected: Option<(usize, u32)>,
    /// A picture being dragged or edited, painted instead of the stored one.
    preview: Option<SheetImage>,
    drag: Option<ImgDrag>,
    /// The context menu is for the selected picture.
    menu: bool,
    dialog: Option<PropsDlg>,
    /// Clipboard reader running in the background.
    incoming: Option<Receiver<Option<Vec<u8>>>>,
    /// When the last text paste arrived.
    last_paste: Option<web_time::Instant>,
    /// The pointer is over a picture.
    hovering: bool,
    /// Charts (they share the picture entry points).
    pub(super) charts: super::charts_ui::ChartsState,
    /// Shapes (above charts).
    pub(super) shapes: super::shapes_ui::ShapesState,
    /// Insert > Object, and what the application is asked to do with an
    /// object.
    insert_object: crate::objects::InsertDlg,
    object_request: Option<crate::objects::Request>,
    /// Insert > QR code, and the code it edits (sheet, picture id).
    barcode: crate::barcode::Dialog,
    barcode_of: Option<(usize, u32)>,
    /// Insert > Library.
    library: crate::library::Dialog,
}

impl ImagesState {
    pub(super) fn deselect_picture(&mut self) {
        self.selected = None;
        self.menu = false;
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Handle {
    Move,
    /// Signs of the handle along the picture's width and height.
    Resize(i8, i8),
    Rotate,
}

struct ImgDrag {
    handle: Handle,
    start: Pos2,
    orig: SheetImage,
    moved: bool,
}

struct PropsDlg {
    sheet: usize,
    img: SheetImage,
    /// Natural size of the whole picture, sheet pixels.
    natural: (f32, f32),
    /// First frame: the click that opened it must not close it.
    fresh: bool,
}

/// Pixel hash of what `Context::copy_image` puts on the clipboard.
pub(super) fn pixel_hash(rgba: &[u8]) -> u64 {
    crate::images::pixel_hash(rgba)
}

/// Screen position of sheet point `(x, y)` in the pane with frozen
/// columns (`fx`) and/or rows (`fy`).
pub(super) fn to_screen(g: &Geo, sh: &Sheet, fx: bool, fy: bool, x: f32, y: f32) -> Pos2 {
    g.point_to_screen(sh, fx, fy, x, y)
}

/// The pane under a screen point: (frozen columns, frozen rows).
pub(super) fn pane_at(g: &Geo, p: Pos2) -> (bool, bool) {
    (g.fc > 0 && p.x < g.origin.x + g.fx * g.z, g.fr > 0 && p.y < g.origin.y + g.fy * g.z)
}

/// The pane a picture's handles are drawn in: where its corner is.
fn home_pane(g: &Geo, sh: &Sheet, img: &SheetImage) -> (bool, bool) {
    let (x, y) = img.pos(sh);
    (g.fc > 0 && x < g.ox + g.fx, g.fr > 0 && y < g.oy + g.fy)
}

/// Unrotated screen rectangle of a picture in a pane.
fn screen_rect(g: &Geo, pane: (bool, bool), sh: &Sheet, img: &SheetImage) -> Rect {
    let [x, y, w, h] = img.rect(sh);
    Rect::from_min_size(to_screen(g, sh, pane.0, pane.1, x, y), vec2(w, h) * g.z)
}

/// Screen point `p` in the picture's own axes, relative to its center.
fn local(rect: Rect, rotation: f32, p: Pos2) -> Vec2 {
    let d = p - rect.center();
    let (s, c) = rotation.to_radians().sin_cos();
    vec2(d.x * c + d.y * s, -d.x * s + d.y * c)
}

fn turn(rotation: f32, v: Vec2) -> Vec2 {
    let (s, c) = rotation.to_radians().sin_cos();
    vec2(v.x * c - v.y * s, v.x * s + v.y * c)
}

/// Handle positions of a picture on screen.
fn handles(rect: Rect, rotation: f32) -> Vec<(Handle, Pos2)> {
    let c = rect.center();
    let (hw, hh) = (rect.width() / 2.0, rect.height() / 2.0);
    let mut v = Vec::with_capacity(9);
    for sy in -1i8..=1 {
        for sx in -1i8..=1 {
            if sx != 0 || sy != 0 {
                v.push((Handle::Resize(sx, sy), c + turn(rotation, vec2(sx as f32 * hw, sy as f32 * hh))));
            }
        }
    }
    v.push((Handle::Rotate, c + turn(rotation, vec2(0.0, -hh - ROT_ARM))));
    v
}

fn resize_cursor(sx: i8, sy: i8, rotation: f32) -> CursorIcon {
    // Direction of the handle from the center, in 45 degree steps.
    let a = (sy as f32).atan2(sx as f32).to_degrees() + rotation;
    match ((a / 45.0).round() as i32).rem_euclid(4) {
        0 => CursorIcon::ResizeHorizontal,
        1 => CursorIcon::ResizeNwSe,
        2 => CursorIcon::ResizeVertical,
        _ => CursorIcon::ResizeNeSw,
    }
}

/// `orig` resized by dragging handle (sx, sy) by `d` sheet pixels. The
/// opposite side stays put; `keep` keeps the proportions.
fn resized(orig: &SheetImage, sh: &Sheet, sx: i8, sy: i8, d: Vec2, keep: bool) -> SheetImage {
    let l = turn(-orig.rotation, d);
    let (w, h) = (orig.width, orig.height);
    let (fx, fy) = (sx as f32, sy as f32);
    let mut nw = if sx != 0 { (w + fx * l.x).max(MIN_SIZE) } else { w };
    let mut nh = if sy != 0 { (h + fy * l.y).max(MIN_SIZE) } else { h };
    if keep {
        let k = match (sx, sy) {
            (0, _) => nh / h,
            (_, 0) => nw / w,
            _ => (nw / w).max(nh / h),
        };
        let k = k.max(MIN_SIZE / w.min(h));
        (nw, nh) = (w * k, h * k);
    }
    let [x, y, _, _] = orig.rect(sh);
    // The new center, from the fixed side, in the picture's axes.
    let shift = vec2(fx * (nw - w) / 2.0, fy * (nh - h) / 2.0);
    let c = pos2(x + w / 2.0, y + h / 2.0) + turn(orig.rotation, shift);
    let mut img = orig.clone();
    img.width = nw;
    img.height = nh;
    img.set_pos(sh, c.x - nw / 2.0, c.y - nh / 2.0);
    img
}

impl SheetView {
    /// The selected picture, if it is still on the current sheet.
    pub(super) fn selected_image(&self) -> Option<&SheetImage> {
        let (s, id) = self.images.selected?;
        (s == self.sheet).then(|| self.sh().image(id)).flatten()
    }

    /// A picture or a chart is selected.
    pub(super) fn image_selected(&self) -> bool {
        self.selected_image().is_some() || self.chart_selected() || self.shape_selected()
    }

    pub(super) fn image_hovered(&self) -> bool {
        self.images.hovering || self.chart_hovered() || self.shape_hovered()
    }

    /// A picture as shown now (the drag or dialog preview wins).
    fn shown<'a>(&'a self, img: &'a SheetImage) -> &'a SheetImage {
        match &self.images.preview {
            Some(p) if p.id == img.id && self.images.selected.is_some_and(|(s, _)| s == self.sheet) => p,
            _ => img,
        }
    }

    // --- painting ----------------------------------------------------------------------

    /// Paints the pictures of the current sheet in the four panes.
    pub(super) fn paint_images(&self, painter: &egui::Painter, g: &Geo) {
        let sh = self.sh();
        if sh.images.is_empty() && sh.charts.is_empty() && sh.sparklines.is_empty() && !self.shapes_to_paint() {
            return;
        }
        let fxs = g.origin.x + g.fx * g.z;
        let fys = g.origin.y + g.fy * g.z;
        let panes = [
            ((true, true), Rect::from_min_max(g.origin, pos2(fxs, fys))),
            ((false, true), Rect::from_min_max(pos2(fxs, g.origin.y), pos2(g.view.max.x, fys))),
            ((true, false), Rect::from_min_max(pos2(g.origin.x, fys), pos2(fxs, g.view.max.y))),
            ((false, false), Rect::from_min_max(pos2(fxs, fys), g.view.max)),
        ];
        for (pane, clip) in panes {
            let clip = clip.intersect(g.view);
            if clip.width() <= 0.0 || clip.height() <= 0.0 {
                continue;
            }
            self.paint_images_in(&painter.with_clip_rect(clip), g, pane, clip, false);
        }
    }

    /// Paints the pictures that show in `clip` (screen) of one pane.
    /// `wait`: decode now (printing) instead of showing a placeholder.
    pub(super) fn paint_images_in(&self, p: &egui::Painter, g: &Geo, pane: (bool, bool), clip: Rect, wait: bool) {
        self.paint_sparklines_in(p, g, clip);
        let sh = self.sh();
        let ctx = p.ctx();
        for stored in &sh.images {
            let img = self.shown(stored);
            let [x0, y0, x1, y1] = img.bounds(sh);
            let r = Rect::from_min_max(to_screen(g, sh, pane.0, pane.1, x0, y0), to_screen(g, sh, pane.0, pane.1, x1, y1));
            if !r.intersects(clip) {
                continue;
            }
            if let Some(o) = &img.object {
                crate::objects::paint(p, screen_rect(g, pane, sh, img), &img.data, o, img.rotation, wait);
                continue;
            }
            let tex = images::texture(ctx, images::key(&img.data), &img.data, wait);
            images::paint(p, tex.as_ref(), screen_rect(g, pane, sh, img), img.rotation, img.crop);
        }
        self.paint_charts_in(p, g, pane, clip, wait);
        self.paint_shapes_in(p, g, pane, clip, wait);
    }

    /// Outline and handles of the selected picture.
    pub(super) fn paint_image_handles(&self, painter: &egui::Painter, g: &Geo) {
        self.paint_chart_handles(painter, g);
        self.paint_shape_handles(painter, g);
        let Some(stored) = self.selected_image() else { return };
        let sh = self.sh();
        let img = self.shown(stored);
        let rect = screen_rect(g, home_pane(g, sh, img), sh, img);
        let p = painter.with_clip_rect(g.view);
        let accent = theme::SHEET_ACCENT;
        images::outline(&p, rect, img.rotation, Stroke::new(1.0, accent));
        let hs = handles(rect, img.rotation);
        let at = |h: Handle| hs.iter().find(|(k, _)| *k == h).map(|(_, a)| *a);
        crate::touch::grip(&p, hs.iter().fold(rect, |r, (_, a)| r.union(Rect::from_center_size(*a, Vec2::ZERO))), crate::touch::reach(GRAB), at(Handle::Resize(1, 1)).zip(at(Handle::Resize(-1, -1))));
        if let Some((_, top)) = hs.iter().find(|(h, _)| *h == Handle::Resize(0, -1)) {
            let rot = hs.last().map_or(*top, |(_, r)| *r);
            p.line_segment([*top, rot], Stroke::new(1.0, accent));
        }
        for (h, at) in hs {
            match h {
                Handle::Rotate => {
                    p.circle(at, HANDLE / 2.0 + 1.0, Color32::WHITE, Stroke::new(1.5, accent));
                }
                _ => {
                    let r = Rect::from_center_size(at, Vec2::splat(HANDLE));
                    p.rect(r, 1.0, Color32::WHITE, Stroke::new(1.0, accent), egui::StrokeKind::Middle);
                }
            }
        }
    }

    // --- pointer -----------------------------------------------------------------------

    /// The picture (topmost first) and handle under screen point `p`.
    fn image_hit(&self, g: &Geo, p: Pos2) -> Option<(u32, Handle)> {
        let sh = self.sh();
        if let Some(stored) = self.selected_image() {
            let img = self.shown(stored);
            let rect = screen_rect(g, home_pane(g, sh, img), sh, img);
            if let Some(h) = crate::touch::nearest(handles(rect, img.rotation), p, GRAB) {
                return Some((img.id, h));
            }
        }
        let pane = pane_at(g, p);
        for stored in sh.images.iter().rev() {
            let img = self.shown(stored);
            let rect = screen_rect(g, pane, sh, img);
            let l = local(rect, img.rotation, p);
            if l.x.abs() <= rect.width() / 2.0 && l.y.abs() <= rect.height() / 2.0 {
                return Some((img.id, Handle::Move));
            }
        }
        None
    }

    /// Mouse handling for pictures; true when it took the event.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn image_pointer(&mut self, ui: &egui::Ui, resp: &egui::Response, g: &Geo, p: Pos2, pressed: bool, down: bool, mods: Modifiers) -> bool {
        // Shapes lie above charts, charts above pictures.
        if self.images.drag.is_none() && self.images.charts.idle() && self.shape_pointer(ui, resp, g, p, pressed, down, mods) {
            self.images.hovering = false;
            return true;
        }
        if self.images.drag.is_none() && self.chart_pointer(ui, resp, g, p, pressed, down, mods) {
            self.images.hovering = false;
            return true;
        }
        if let Some(d) = &mut self.images.drag {
            if !d.moved && (p - d.start).length() > 3.0 {
                d.moved = true;
            }
            let (handle, start, moved) = (d.handle, d.start, d.moved);
            if down {
                if moved {
                    let orig = d.orig.clone();
                    let img = self.dragged(g, &orig, handle, start, p, mods);
                    self.images.preview = Some(img);
                    ui.ctx().request_repaint();
                }
                return true;
            }
            // Released.
            let d = self.images.drag.take();
            if let (Some(img), true) = (self.images.preview.take(), moved) {
                self.wb.set_image(self.sheet, img);
            } else if d.is_some() && resp.double_clicked() {
                // An embedded object opens, a code its dialog; a picture
                // shows its properties.
                match self.selected_image().map(|i| (i.id, i.object.clone())) {
                    Some((_, Some(o))) => self.images.object_request = Some(crate::objects::Request::Open(o)),
                    Some((id, None)) if self.edit_barcode(id) => {}
                    _ => self.open_image_dialog(),
                }
            }
            return true;
        }
        let target = if resp.hovered() && g.view.contains(p) && !self.formula_ref_point() { self.image_hit(g, p) } else { None };
        self.images.hovering = target.is_some();
        if let Some((id, h)) = target {
            let rot = self.sh().image(id).map_or(0.0, |i| i.rotation);
            ui.ctx().set_cursor_icon(match h {
                Handle::Move => CursorIcon::Move,
                Handle::Resize(sx, sy) => resize_cursor(sx, sy, rot),
                Handle::Rotate => CursorIcon::Grab,
            });
        }
        if pressed && resp.hovered() {
            let Some((id, handle)) = target else {
                self.images.selected = None;
                return false;
            };
            if self.edit.is_some() && !self.settle_edit() {
                return true;
            }
            resp.request_focus();
            // A picture with a macro runs it; Ctrl+click selects it.
            let picked = self.images.selected == Some((self.sheet, id));
            if let Some(m) = self.sh().image(id).map(|i| i.macro_name.clone()).filter(|m| !m.is_empty() && handle == Handle::Move && !picked && !mods.command) {
                self.images.selected = None;
                self.click_macro(&m);
                return true;
            }
            self.images.selected = Some((self.sheet, id));
            self.marquee = false;
            if let Some(orig) = self.sh().image(id).cloned() {
                self.images.drag = Some(ImgDrag { handle, start: p, orig, moved: false });
            }
            return true;
        }
        false
    }

    /// The picture as the drag leaves it.
    fn dragged(&self, g: &Geo, orig: &SheetImage, handle: Handle, start: Pos2, p: Pos2, mods: Modifiers) -> SheetImage {
        let sh = self.sh();
        let mut d = (p - start) / g.z;
        match handle {
            Handle::Move => {
                // Shift keeps it on a line.
                if mods.shift {
                    if d.x.abs() > d.y.abs() {
                        d.y = 0.0;
                    } else {
                        d.x = 0.0;
                    }
                }
                let mut img = orig.clone();
                let (x, y) = orig.pos(sh);
                img.set_pos(sh, x + d.x, y + d.y);
                img
            }
            Handle::Resize(sx, sy) => {
                let corner = sx != 0 && sy != 0;
                resized(orig, sh, sx, sy, d, mods.shift || orig.lock_aspect && corner)
            }
            Handle::Rotate => {
                let rect = screen_rect(g, home_pane(g, sh, orig), sh, orig);
                let v = p - rect.center();
                let mut a = (v.y.atan2(v.x).to_degrees() + 90.0).rem_euclid(360.0);
                if mods.shift {
                    a = (a / 15.0).round() * 15.0 % 360.0;
                }
                let mut img = orig.clone();
                img.rotation = a;
                img
            }
        }
    }

    /// Right click: true when it was on a picture (then the context menu
    /// is the picture's).
    pub(super) fn image_menu_hit(&mut self, g: &Geo, p: Pos2) -> bool {
        if self.shape_menu_hit(g, p) {
            self.images.menu = false;
            return true;
        }
        if self.chart_menu_hit(g, p) {
            self.images.menu = false;
            return true;
        }
        let hit = if g.view.contains(p) && !self.formula_ref_point() { self.image_hit(g, p) } else { None };
        self.images.menu = hit.is_some();
        if let Some((id, _)) = hit {
            self.images.selected = Some((self.sheet, id));
        }
        self.images.menu
    }

    pub(super) fn image_menu_open(&self) -> bool {
        (self.images.menu && self.selected_image().is_some()) || self.chart_menu_open() || self.shape_menu_open()
    }

    /// Brings the selected picture to the front or sends it to the back.
    pub(super) fn reorder_selected_image(&mut self, front: bool) {
        if let Some((_, id)) = self.images.selected {
            self.wb.reorder_image(self.sheet, id, front);
        }
    }

    pub(super) fn image_context_menu(&mut self, ui: &mut egui::Ui) {
        if self.shape_menu_open() {
            self.shape_context_menu(ui);
            return;
        }
        if self.chart_menu_open() {
            self.chart_context_menu(ui);
            return;
        }
        if let Some(o) = self.selected_image().and_then(|i| i.object.clone()) {
            match crate::objects::menu(ui, &o) {
                Some(crate::objects::MenuAction::Open) => self.images.object_request = Some(crate::objects::Request::Open(o)),
                Some(crate::objects::MenuAction::SaveAs) => crate::objects::save_as_file(&o),
                None => {}
            }
        }
        if ui.add(egui::Button::new(tr("edit.cut")).shortcut_text("Ctrl+X")).clicked() {
            self.copy_image(ui.ctx(), true);
            ui.close();
        }
        if ui.add(egui::Button::new(tr("edit.copy")).shortcut_text("Ctrl+C")).clicked() {
            self.copy_image(ui.ctx(), false);
            ui.close();
        }
        if ui.add(egui::Button::new(tr("image.delete")).shortcut_text("Del")).clicked() {
            self.delete_image();
            ui.close();
        }
        ui.separator();
        if ui.button(tr("image.front")).clicked() {
            if let Some((_, id)) = self.images.selected {
                self.wb.reorder_image(self.sheet, id, true);
            }
            ui.close();
        }
        if ui.button(tr("image.back")).clicked() {
            if let Some((_, id)) = self.images.selected {
                self.wb.reorder_image(self.sheet, id, false);
            }
            ui.close();
        }
        if ui.button(format!("{}…", tr("macros.assign"))).clicked() {
            if let Some((_, id)) = self.images.selected {
                self.open_assign_macro(super::macro_hooks::Clickable::Image(id));
            }
            ui.close();
        }
        if ui.button(tr("image.place_in_cell")).on_hover_text(tr("image.place_in_cell_tip")).clicked() {
            self.place_in_cell();
            ui.close();
        }
        ui.separator();
        if ui.button(format!("{}…", tr("image.size"))).clicked() {
            self.open_image_dialog();
            ui.close();
        }
    }

    // --- keyboard ----------------------------------------------------------------------

    /// Keys for the selected picture, and Ctrl+V of a picture-only
    /// clipboard. True when the grid must not see the events.
    pub(super) fn image_keys(&mut self, ui: &egui::Ui) -> bool {
        // The dialog takes Enter and Escape itself.
        if self.images.dialog.is_some() {
            return true;
        }
        let events = ui.input(|i| i.events.clone());
        for ev in &events {
            match ev {
                Event::Paste(_) => self.images.last_paste = Some(web_time::Instant::now()),
                // egui sends no paste when the clipboard has no text (only
                // the key release comes): a picture may be there.
                Event::Key { key: Key::V, pressed: false, modifiers: m, .. } if m.command && !m.shift && !m.alt => {
                    if self.images.last_paste.is_none_or(|t| t.elapsed().as_secs_f32() > 2.0) {
                        self.paste_clipboard_image(ui.ctx());
                    }
                }
                _ => {}
            }
        }
        if self.shape_keys(ui, &events) || self.chart_keys(ui, &events) {
            return true;
        }
        let Some(img) = self.selected_image().cloned() else { return false };
        for ev in events {
            match ev {
                Event::Copy => self.copy_image(ui.ctx(), false),
                Event::Cut => self.copy_image(ui.ctx(), true),
                Event::Paste(_) => {
                    // Text goes into the cells.
                    self.images.selected = None;
                    return false;
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
                    match key {
                        _ if nudge.is_some() => {
                            let d = nudge.unwrap_or_default();
                            let mut moved = img.clone();
                            let (x, y) = img.pos(self.sh());
                            moved.set_pos(self.sh(), x + d.x, y + d.y);
                            self.wb.set_image(self.sheet, moved);
                        }
                        Key::Delete | Key::Backspace => self.delete_image(),
                        Key::Escape => self.images.selected = None,
                        Key::Z if m.command && m.shift => self.redo(),
                        Key::Z | Key::Y if m.command => {
                            if key == Key::Z {
                                self.undo()
                            } else {
                                self.redo()
                            }
                        }
                        Key::Enter => self.open_image_dialog(),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        true
    }

    fn delete_image(&mut self) {
        if let Some((s, id)) = self.images.selected.take() {
            if s == self.sheet {
                self.wb.remove_image(self.sheet, id);
            }
        }
    }

    // --- clipboard ---------------------------------------------------------------------

    /// Copies the selected picture (cut removes it now, as Excel does).
    pub(super) fn copy_image(&mut self, ctx: &egui::Context, cut: bool) {
        if self.shape_selected() {
            self.copy_shape(ctx, cut);
            return;
        }
        if self.chart_selected() {
            self.copy_chart(ctx, cut);
            return;
        }
        let Some(img) = self.selected_image().cloned() else { return };
        let mut hash = 0;
        if let Some(d) = images::decode(&img.data) {
            // Straight alpha as is: the clipboard backend writes the
            // pixels unchanged into a PNG, which is not premultiplied.
            let ci = egui::ColorImage::from_rgba_premultiplied([d.width as usize, d.height as usize], &d.rgba);
            hash = pixel_hash(&d.rgba);
            ctx.copy_image(ci);
        }
        *CLIP.lock().unwrap_or_else(|e| e.into_inner()) = Some((img, hash));
        if cut {
            self.delete_image();
        }
    }

    /// Looks for a picture on the system clipboard in the background (the
    /// menu's Paste, or Ctrl+V with no text there).
    pub(super) fn paste_clipboard_image(&mut self, ctx: &egui::Context) {
        if self.images.incoming.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let started = apex_fs::thread::Builder::new().name("apex-clipboard".into()).spawn(move || {
            let _ = tx.send(images::read_clipboard_image());
            ctx.request_repaint();
        });
        if started.is_ok() {
            self.images.incoming = Some(rx);
        }
    }

    /// Takes a picture read from the clipboard, if one arrived.
    pub(super) fn images_poll(&mut self) {
        let Some(rx) = &self.images.incoming else { return };
        let got = match rx.try_recv() {
            Ok(v) => v,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => None,
        };
        self.images.incoming = None;
        let Some(bytes) = got else { return };
        if self.paste_chart_if_ours(&bytes) {
            return;
        }
        // Our own copy comes back as a PNG of the same pixels: paste the
        // original (its file, crop and rotation).
        let ours = CLIP.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some((img, hash)) = ours {
            if images::decode(&bytes).is_some_and(|d| pixel_hash(&d.rgba) == hash) {
                self.paste_image(img);
                return;
            }
        }
        self.insert_image_bytes(bytes.into(), "");
    }

    /// Puts a copy of `img` at the cursor.
    fn paste_image(&mut self, mut img: SheetImage) {
        self.settle_edit();
        let sh = self.sh();
        let (x, y) = (sh.col_x(self.cursor.col), sh.row_y(self.cursor.row));
        img.set_pos(sh, x, y);
        img.name.clear();
        let id = self.wb.add_image(self.sheet, img);
        if self.sh().image(id).is_some() {
            self.images.selected = Some((self.sheet, id));
        }
    }

    // --- inserting ---------------------------------------------------------------------

    /// Insert > Picture: asks for a file.
    pub(super) fn insert_image_dialog(&mut self) {
        crate::file_dialog::FileDialog::new().set_title(tr("image.insert")).add_filter(tr("image.filter"), &EXTENSIONS).pick_file(crate::file_dialog::in_view(|v: &mut SheetView, p: std::path::PathBuf| v.insert_image_file(&p)));
    }

    /// Inserts a picture file at the cursor.
    pub fn insert_image_file(&mut self, path: &Path) {
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        match apex_fs::read(path) {
            Ok(b) => self.insert_image_bytes(b.into(), &name),
            Err(e) => self.notices.push(Notice::error(trf("error.open", &[("error", &e.to_string())]))),
        }
    }

    /// Inserts a picture at the cursor, at its natural size (shrunk to fit
    /// the window) and selects it.
    fn insert_image_bytes(&mut self, data: Arc<[u8]>, name: &str) {
        self.settle_edit();
        let format = ImageFormat::sniff(&data).filter(|_| images::decode(&data).is_some());
        let (Some(format), Some((mut w, mut h))) = (format, apex_sheet::image::natural_size(&data)) else {
            self.notices.push(Notice::error(tr("image.bad")));
            return;
        };
        // Fit in about the visible area.
        let (vw, vh) = self.view_size;
        let k = (vw * 0.8 / w).min(vh * 0.8 / h).min(1.0);
        if k.is_finite() && k > 0.0 {
            (w, h) = (w * k, h * k);
        }
        let mut img = SheetImage::new(data, format, self.cursor, w, h);
        img.name = name.to_string();
        let id = self.wb.add_image(self.sheet, img);
        if self.sh().image(id).is_some() {
            self.images.selected = Some((self.sheet, id));
            self.ensure_visible = true;
        }
    }

    /// What the application is asked to do with an object.
    pub(crate) fn take_object_request(&mut self) -> Option<crate::objects::Request> {
        self.images.object_request.take()
    }

    /// Insert > QR code.
    pub(super) fn show_barcode(&mut self) {
        self.images.barcode.show();
        self.images.barcode_of = None;
    }

    /// Insert > Library.
    pub(super) fn show_library(&mut self) {
        self.images.library.show();
    }

    /// Opens picture `id` of this sheet in the QR code dialog; false if it
    /// isn't a code.
    fn edit_barcode(&mut self, id: u32) -> bool {
        let Some((kind, text)) = self.sh().image(id).and_then(|i| crate::barcode::from_alt(&i.alt)) else { return false };
        self.images.barcode.edit(kind, text);
        self.images.barcode_of = Some((self.sheet, id));
        true
    }

    fn barcode_dialog(&mut self, ctx: &egui::Context) {
        if let Some(p) = self.images.library.ui(ctx) {
            self.insert_image_file(&p);
        }
        if let Some(m) = self.images.barcode.ui(ctx) {
            self.put_barcode(m);
        }
    }

    /// A new code goes at the cursor; an edited one keeps its place and
    /// width.
    fn put_barcode(&mut self, m: crate::barcode::Made) {
        self.settle_edit();
        let old = self.images.barcode_of.take().and_then(|(sheet, id)| Some((sheet, self.wb.sheets.get(sheet)?.image(id)?.clone())));
        if let Some((sheet, old)) = old {
            let height = old.width * m.size.1 / m.size.0;
            let img = SheetImage { data: m.png, format: ImageFormat::Png, height, alt: m.alt, crop: [0.0; 4], ..old };
            self.wb.set_image(sheet, img);
            return;
        }
        // Points to sheet pixels.
        let mut img = SheetImage::new(m.png, ImageFormat::Png, self.cursor, m.size.0 / 0.75, m.size.1 / 0.75);
        img.alt = m.alt;
        img.name = m.name;
        let id = self.wb.add_image(self.sheet, img);
        if self.sh().image(id).is_some() {
            self.images.selected = Some((self.sheet, id));
            self.ensure_visible = true;
        }
    }

    /// Insert > Diagram: one of family `kind` with sample text at the
    /// cursor, opened to type over it.
    pub(super) fn insert_diagram(&mut self, kind: apex_io::diagram_edit::Kind) {
        self.settle_edit();
        // Sheet pixels; the diagram is in points.
        let (w, h) = (480.0, 288.0);
        let d = crate::diagram_ui::new_diagram(kind, (w * 0.75, h * 0.75));
        let obj = Arc::new(apex_doc::Embedded::new(apex_doc::EmbedKind::Diagram(d)));
        let mut img = SheetImage::new(Arc::from(&[][..]), ImageFormat::Png, self.cursor, w, h);
        img.object = Some(obj.clone());
        let id = self.wb.add_image(self.sheet, img);
        if self.sh().image(id).is_some() {
            self.images.selected = Some((self.sheet, id));
            self.ensure_visible = true;
        }
        self.images.object_request = Some(crate::objects::Request::Open(obj));
    }

    /// Insert > Object.
    pub(super) fn show_insert_object(&mut self) {
        self.images.insert_object.show();
    }

    /// The Insert > Object dialog; the object goes at the cursor.
    fn object_dialog(&mut self, ctx: &egui::Context) {
        let Some(m) = self.images.insert_object.ui(ctx) else { return };
        self.settle_edit();
        // Points to sheet pixels.
        let mut img = SheetImage::new(m.preview, ImageFormat::Png, self.cursor, m.size.0 / 0.75, m.size.1 / 0.75);
        img.object = Some(m.object.clone());
        let id = self.wb.add_image(self.sheet, img);
        if self.sh().image(id).is_some() {
            self.images.selected = Some((self.sheet, id));
            self.ensure_visible = true;
        }
        if m.edit {
            self.images.object_request = Some(crate::objects::Request::Open(m.object));
        }
    }

    // --- dialog ------------------------------------------------------------------------

    pub(super) fn open_image_dialog(&mut self) {
        let Some(img) = self.selected_image().cloned() else { return };
        let natural = apex_sheet::image::natural_size(&img.data).unwrap_or((img.width, img.height));
        self.images.preview = Some(img.clone());
        self.images.dialog = Some(PropsDlg { sheet: self.sheet, img, natural, fresh: true });
    }

    pub(super) fn image_dialog(&mut self, ctx: &egui::Context) {
        self.chart_dialogs(ctx);
        self.object_dialog(ctx);
        self.barcode_dialog(ctx);
        self.shape_dialog(ctx);
        self.control_dialog(ctx);
        self.assign_macro_dialog(ctx);
        let Some(d) = &mut self.images.dialog else { return };
        let mut res = widgets::dialog(ctx, "image-props", &tr("image.size"), 420.0, |ui| props_ui(ui, d));
        if std::mem::take(&mut d.fresh) && res == DialogResult::Cancel && !ctx.input(|i| i.key_pressed(Key::Escape)) {
            res = DialogResult::None;
        }
        let img = d.img.clone();
        let sheet = d.sheet;
        match res {
            DialogResult::None => {
                if self.images.drag.is_none() {
                    self.images.preview = Some(img);
                }
            }
            DialogResult::Ok => {
                self.images.dialog = None;
                self.images.preview = None;
                if sheet < self.wb.sheets.len() {
                    self.wb.set_image(sheet, img);
                }
            }
            DialogResult::Cancel => {
                self.images.dialog = None;
                self.images.preview = None;
            }
        }
    }

    /// Drops a selection that no longer exists (undo, another sheet).
    pub(super) fn images_frame(&mut self) {
        self.charts_frame();
        self.shapes_frame();
        if self.images.selected.is_some() && self.selected_image().is_none() {
            self.images.selected = None;
            self.images.drag = None;
            if self.images.dialog.is_none() {
                self.images.preview = None;
            }
        }
        self.images_poll();
    }
}

/// Picture size in cm with the aspect lock.
fn size_row(ui: &mut egui::Ui, label: &str, v: &mut f32) -> bool {
    ui.label(label);
    let mut cm = *v * CM;
    let changed = ui.add(egui::DragValue::new(&mut cm).range(0.05..=500.0).speed(0.05).max_decimals(2).suffix(" cm")).changed();
    ui.end_row();
    if changed {
        *v = cm / CM;
    }
    changed
}

fn props_ui(ui: &mut egui::Ui, d: &mut PropsDlg) {
    let img = &mut d.img;
    widgets::section(ui, tr("image.size_heading"));
    egui::Grid::new("image_size").num_columns(2).min_col_width(widgets::FORM_LABEL_W).spacing([12.0, 8.0]).show(ui, |ui| {
        let (w0, h0) = (img.width, img.height);
        if size_row(ui, &tr("image.height"), &mut img.height) && img.lock_aspect && h0 > 0.0 {
            img.width = w0 * img.height / h0;
        }
        if size_row(ui, &tr("image.width"), &mut img.width) && img.lock_aspect && w0 > 0.0 {
            img.height = h0 * img.width / w0;
        }
        ui.label(tr("image.rotation"));
        let mut r = img.rotation;
        if ui.add(egui::DragValue::new(&mut r).range(-360.0..=360.0).speed(1.0).max_decimals(1).suffix("°")).changed() {
            img.rotation = r.rem_euclid(360.0);
        }
        ui.end_row();
        ui.label("");
        ui.checkbox(&mut img.lock_aspect, tr("image.lock_aspect"));
        ui.end_row();
    });
    widgets::section(ui, tr("image.crop"));
    // Crop amounts in cm of the whole picture at its current scale;
    // cropping makes the shown picture smaller, as in Excel.
    let [l, t, r, b] = img.crop;
    let full = (img.width / (1.0 - l - r).max(0.01), img.height / (1.0 - t - b).max(0.01));
    let mut cm = [l * full.0 * CM, t * full.1 * CM, r * full.0 * CM, b * full.1 * CM];
    let mut changed = false;
    egui::Grid::new("image_crop").num_columns(4).spacing([12.0, 6.0]).show(ui, |ui| {
        for (i, key) in ["image.crop_left", "image.crop_top", "image.crop_right", "image.crop_bottom"].iter().enumerate() {
            ui.label(tr(key));
            let max = if i % 2 == 0 { full.0 } else { full.1 } * CM * 0.95;
            changed |= ui.add(egui::DragValue::new(&mut cm[i]).range(0.0..=max).speed(0.02).max_decimals(2).suffix(" cm")).changed();
            if i % 2 == 1 {
                ui.end_row();
            }
        }
    });
    if changed {
        let f = [cm[0] / CM / full.0, cm[1] / CM / full.1, cm[2] / CM / full.0, cm[3] / CM / full.1];
        img.crop = f.map(|v| if v.is_finite() { v.clamp(0.0, 0.95) } else { 0.0 });
        img.sanitize();
        let [l, t, r, b] = img.crop;
        img.width = full.0 * (1.0 - l - r);
        img.height = full.1 * (1.0 - t - b);
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let (nw, nh) = d.natural;
        ui.label(RichText::new(trf("image.original", &[("w", &format!("{:.2} cm", nw * CM)), ("h", &format!("{:.2} cm", nh * CM))])).weak());
        if ui.button(tr("image.reset")).clicked() {
            img.width = nw;
            img.height = nh;
            img.crop = [0.0; 4];
            img.rotation = 0.0;
        }
    });
    widgets::section(ui, tr("image.properties"));
    ui.radio_value(&mut img.anchor, Anchor::TwoCell, tr("image.move_size"));
    ui.radio_value(&mut img.anchor, Anchor::OneCell, tr("image.move_only"));
    ui.radio_value(&mut img.anchor, Anchor::Absolute, tr("image.absolute"));
    widgets::section(ui, tr("image.alt"));
    egui::Grid::new("image_alt").num_columns(2).min_col_width(widgets::FORM_LABEL_W).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label(tr("image.name"));
        ui.add(egui::TextEdit::singleline(&mut img.name).desired_width(280.0));
        ui.end_row();
        ui.label(tr("image.alt"));
        let id = ui.add(egui::TextEdit::multiline(&mut img.alt).desired_rows(3).desired_width(280.0).hint_text(tr("image.alt_hint"))).id;
        // Enter types a new line here instead of pressing OK.
        ui.data_mut(|m| m.insert_temp(id.with("multiline"), true));
        ui.end_row();
    });
    img.width = img.width.max(1.0);
    img.height = img.height.max(1.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_sheet::CellRef;

    fn pic() -> SheetImage {
        SheetImage::new(Arc::from(&b"x"[..]), ImageFormat::Png, CellRef::new(2, 2), 100.0, 50.0)
    }

    #[test]
    fn a_picture_goes_to_another_workbook_as_it_was() {
        let ctx = egui::Context::default();
        let png = crate::charts::encode_png(&egui::ColorImage::new([7, 5], vec![Color32::from_rgb(9, 80, 200); 35])).unwrap();
        let mut img = SheetImage::new(Arc::from(png.clone()), ImageFormat::Png, CellRef::new(2, 2), 70.0, 50.0);
        img.rotation = 30.0;
        img.alt = "logo".into();
        let mut a = SheetView::new(apex_sheet::Workbook::new());
        let id = a.wb.add_image(0, img);
        a.images.selected = Some((0, id));
        a.copy_image(&ctx, false);

        let mut b = SheetView::new(apex_sheet::Workbook::new());
        b.select_range(CellRef::new(4, 1), CellRef::new(4, 1));
        let (tx, rx) = mpsc::channel();
        tx.send(Some(png)).unwrap();
        b.images.incoming = Some(rx);
        b.images_poll();
        let got = &b.sh().images[0];
        assert_eq!((got.rotation, got.alt.as_str(), got.width, got.from), (30.0, "logo", 70.0, CellRef::new(4, 1)));
    }

    #[test]
    fn codes_open_again_and_keep_their_width() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        let mut v = SheetView::new(apex_sheet::Workbook::new());
        v.put_barcode(crate::barcode::made(&ctx, crate::barcode::Kind::Ean13, "841234567890").unwrap());
        let img = v.sh().images[0].clone();
        assert_eq!((img.alt.as_str(), img.from), ("EAN-13: 841234567890", CellRef::new(0, 0)));
        assert!(v.edit_barcode(img.id));
        assert_eq!(v.images.barcode.editing(), Some((crate::barcode::Kind::Ean13, "841234567890")));
        v.put_barcode(crate::barcode::made(&ctx, crate::barcode::Kind::Qr, "x").unwrap());
        assert_eq!(v.sh().images.len(), 1);
        let new = &v.sh().images[0];
        assert_eq!((new.alt.as_str(), new.width, new.id), ("QR: x", img.width, img.id));
        assert!((new.height - new.width).abs() < 0.01, "a QR code is square");
        // Undone in one step.
        v.wb.undo();
        assert_eq!(v.sh().images[0].alt, img.alt);
    }

    #[test]
    fn resize_keeps_the_opposite_side() {
        let sh = Sheet::new("S");
        let orig = pic();
        let [x, y, _, _] = orig.rect(&sh);
        // Right handle 20 px to the right: left edge stays.
        let r = resized(&orig, &sh, 1, 0, vec2(20.0, 5.0), false);
        assert_eq!((r.width, r.height), (120.0, 50.0));
        let [rx, ry, _, _] = r.rect(&sh);
        assert!((rx - x).abs() < 1e-3 && (ry - y).abs() < 1e-3);
        // Top-left corner with the aspect kept: bottom-right stays.
        let r = resized(&orig, &sh, -1, -1, vec2(-20.0, 0.0), true);
        assert!((r.width - 120.0).abs() < 1e-3 && (r.height - 60.0).abs() < 1e-3);
        let [rx, ry, rw, rh] = r.rect(&sh);
        assert!((rx + rw - (x + 100.0)).abs() < 1e-3 && (ry + rh - (y + 50.0)).abs() < 1e-3);
        // Rotated a quarter: dragging the right handle down widens it.
        let mut turned = pic();
        turned.rotation = 90.0;
        let r = resized(&turned, &sh, 1, 0, vec2(0.0, 30.0), false);
        assert!((r.width - 130.0).abs() < 1e-3, "{}", r.width);
        // Never below the minimum, whatever the drag.
        let r = resized(&orig, &sh, 1, 1, vec2(-1000.0, -1000.0), false);
        assert!(r.width >= MIN_SIZE && r.height >= MIN_SIZE);
    }

    #[test]
    fn handles_and_cursors() {
        let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 50.0));
        let hs = handles(r, 0.0);
        assert_eq!(hs.len(), 9);
        assert!(hs.contains(&(Handle::Resize(1, 1), pos2(100.0, 50.0))));
        assert!(hs.iter().any(|(h, p)| *h == Handle::Rotate && (p.y + ROT_ARM).abs() < 1e-3));
        assert_eq!(resize_cursor(1, 0, 0.0), CursorIcon::ResizeHorizontal);
        assert_eq!(resize_cursor(1, 0, 90.0), CursorIcon::ResizeVertical);
        assert_eq!(resize_cursor(1, 1, 0.0), CursorIcon::ResizeNwSe);
        let l = local(r, 90.0, pos2(50.0, 45.0));
        assert!((l - vec2(20.0, 0.0)).length() < 1e-3, "{l:?}");
    }
}
