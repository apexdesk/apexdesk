//! The designer of a UserForm in the macro editor: the form as it will
//! show, its controls placed with the mouse from a toolbox, and their
//! properties beside it, as in the VBA editor. Sizes are in points, as
//! the design keeps them; the canvas shows them as the running form
//! does. What changes is written back to the module's form storage.

use std::collections::{HashMap, HashSet};

use apex_vba::forms::{Control, Design, Kind};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};

use crate::i18n::tr;
use crate::icons::Icon;
use crate::widgets::{self, Face};

/// Pixels of a point (as a running form shows it).
const SCALE: f32 = 4.0 / 3.0;
/// The height of a MultiPage's tabs, in points.
const TABS: f32 = 18.0;
/// The title bar over the form, in pixels.
const TITLE: f32 = 26.0;
/// The room around the form, in pixels.
const MARGIN: f32 = 12.0;
/// What placing and moving snaps to, in points (the VBA editor's grid).
const GRID: f32 = 6.0;
/// The side of a sizing handle, in pixels.
const HANDLE: f32 = 6.0;
/// The steps of undo kept.
const UNDO: usize = 100;
/// The longest text a property takes.
const MAX_TEXT: usize = 4096;
/// The most controls a form may get here.
const MAX_CONTROLS: usize = 2000;

/// Where a control is: its index in the form's controls, then in its
/// container's (a MultiPage's pages, a page's or a Frame's controls).
type Path = Vec<usize>;

/// The controls the toolbox places.
const TOOLS: [Kind; 13] = [
    Kind::Label,
    Kind::TextBox,
    Kind::ComboBox,
    Kind::ListBox,
    Kind::CheckBox,
    Kind::OptionButton,
    Kind::ToggleButton,
    Kind::Frame,
    Kind::CommandButton,
    Kind::MultiPage,
    Kind::ScrollBar,
    Kind::SpinButton,
    Kind::Image,
];

/// What the mouse is doing on the canvas since it was pressed.
enum Drag {
    /// Moving the selected controls: where each one was.
    Move { from: Pos2, orig: Vec<(Path, f32, f32)>, moved: bool },
    /// Sizing a control (or the form, `None`) by a handle: which sides
    /// move (-1 left/top, 1 right/bottom) and its place and size before.
    Size { from: Pos2, grip: (i8, i8), target: Option<Path>, orig: [f32; 4] },
    /// Drawing a new control in a container: its corners, in points of the form.
    Create { container: Path, base: Pos2, from: Pos2, to: Pos2 },
}

/// A control as laid out on the canvas: its rectangle in points of the
/// form, what of it shows (its container's inside) and, for a Frame or a
/// MultiPage, its inside: where its controls go (a page's for a
/// MultiPage), where they start and the room they show in.
struct Shape {
    path: Path,
    rect: Rect,
    clip: Rect,
    inner: Option<(Path, Pos2, Rect)>,
}

/// What a frame of the designer asks of the editor.
#[derive(Default)]
pub(super) struct Out {
    /// The design changed: write it to the module.
    pub changed: bool,
    /// To the code of an event: object, event and its parameters.
    pub code: Option<(String, &'static str, &'static str)>,
}

/// A form being designed.
pub(super) struct Designer {
    /// The module it is of: its index and name.
    module: (usize, String),
    pub design: Design,
    /// It has controls that can't be written: shown, not changed.
    read_only: bool,
    /// The selected controls; none is the form.
    sel: Vec<Path>,
    /// The control the next click places (None: the pointer).
    tool: Option<Kind>,
    drag: Option<(Drag, Design)>,
    /// The page shown of each MultiPage.
    pages: HashMap<Path, usize>,
    undo: Vec<Design>,
    redo: Vec<Design>,
    /// Copied controls, and the text put on the clipboard for them.
    clip: Vec<Control>,
    clip_mark: String,
    /// When a property last changed (typing is one step of undo).
    last_edit: f64,
    /// A name being typed: the control and the text.
    naming: Option<(Path, String)>,
    name_error: Option<&'static str>,
}

fn px(pt: f32) -> f32 {
    pt * SCALE
}

fn snap(v: f32, on: bool) -> f32 {
    // (+ 0.0: no -0.)
    if on { (v / GRID).round() * GRID + 0.0 } else { v.round() + 0.0 }
}

fn color(c: Option<u32>, or: Color32) -> Color32 {
    c.map_or(or, |c| Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8))
}

/// The control at `path`.
fn get<'a>(d: &'a Design, path: &[usize]) -> Option<&'a Control> {
    let (first, rest) = path.split_first()?;
    let mut c = d.controls.get(*first)?;
    for &i in rest {
        c = c.children.get(i)?;
    }
    Some(c)
}

fn get_mut<'a>(d: &'a mut Design, path: &[usize]) -> Option<&'a mut Control> {
    let (first, rest) = path.split_first()?;
    let mut c = d.controls.get_mut(*first)?;
    for &i in rest {
        c = c.children.get_mut(i)?;
    }
    Some(c)
}

/// The controls of the container at `path` (the form's for an empty one).
fn list_mut<'a>(d: &'a mut Design, path: &[usize]) -> Option<&'a mut Vec<Control>> {
    if path.is_empty() {
        return Some(&mut d.controls);
    }
    get_mut(d, path).map(|c| &mut c.children)
}

/// The names of the design's controls and pages, in lower case.
fn names(d: &Design) -> HashSet<String> {
    d.all().iter().map(|c| c.name.to_lowercase()).collect()
}

/// Whether `name` is some control's (or page's) in the design.
fn taken(d: &Design, name: &str) -> bool {
    names(d).contains(&name.to_lowercase())
}

/// The first name `base` + a number not in `used`: Label1, Label2...
fn free_name(used: &HashSet<String>, base: &str) -> String {
    (1..).map(|n| format!("{base}{n}")).find(|n| !used.contains(&n.to_lowercase())).unwrap_or_default()
}

/// The first free name of the kind in the design.
fn fresh(d: &Design, base: &str) -> String {
    free_name(&names(d), base)
}

/// A control's name as VBA takes it: a letter, then letters, digits and
/// underscores, up to 40.
fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_alphabetic()) && name.chars().count() <= 40 && chars.all(|c| c.is_alphanumeric() || c == '_')
}

/// The tabs of a MultiPage over its rectangle, in points.
fn tabs(c: &Control, rect: Rect) -> Vec<Rect> {
    let mut x = rect.left() + 2.0;
    c.children
        .iter()
        .map(|p| {
            let w = (p.caption.chars().count() as f32 * 5.5 + 12.0).clamp(24.0, 160.0);
            let r = Rect::from_min_size(pos2(x, rect.top()), vec2(w, TABS));
            x += w;
            r
        })
        .collect()
}

/// The events of a control (`None`: of the form) that a running form
/// fires, with their parameters.
pub(super) fn events(kind: Option<Kind>) -> Vec<(&'static str, &'static str)> {
    const CANCEL: &str = "(ByVal Cancel As MSForms.ReturnBoolean)";
    let Some(kind) = kind else {
        return vec![("Activate", ""), ("Click", ""), ("Initialize", ""), ("QueryClose", "(Cancel As Integer, CloseMode As Integer)"), ("Terminate", "")];
    };
    let mut out = Vec::new();
    if !matches!(kind, Kind::TextBox | Kind::ScrollBar | Kind::SpinButton | Kind::MultiPage | Kind::Page) {
        out.push(("Click", ""));
    }
    if !matches!(kind, Kind::Label | Kind::Image | Kind::CommandButton | Kind::Frame | Kind::Page) {
        out.push(("Change", ""));
    }
    if kind != Kind::Page {
        out.push(("DblClick", CANCEL));
    }
    if !matches!(kind, Kind::Label | Kind::Image | Kind::Page) {
        out.extend([("Enter", ""), ("Exit", CANCEL), ("BeforeUpdate", CANCEL), ("AfterUpdate", "")]);
    }
    if matches!(kind, Kind::TextBox | Kind::ComboBox | Kind::ListBox | Kind::CheckBox | Kind::OptionButton | Kind::ToggleButton | Kind::CommandButton) {
        out.push(("KeyPress", "(ByVal KeyAscii As MSForms.ReturnInteger)"));
    }
    out.sort();
    out
}

/// The event a double click opens (the VBA editor's).
fn default_event(kind: Option<Kind>) -> (&'static str, &'static str) {
    match kind {
        Some(Kind::TextBox | Kind::ComboBox | Kind::ScrollBar | Kind::SpinButton | Kind::MultiPage) => ("Change", ""),
        _ => ("Click", ""),
    }
}

/// Every control laid out, in the order they are drawn (a container
/// before its controls; of a MultiPage, those of the page shown).
fn layout(d: &Design, pages: &HashMap<Path, usize>) -> Vec<Shape> {
    fn lay(list: &[Control], parent: &[usize], base: Pos2, clip: Rect, pages: &HashMap<Path, usize>, out: &mut Vec<Shape>) {
        for (i, c) in list.iter().enumerate() {
            let mut path = parent.to_vec();
            path.push(i);
            let rect = Rect::from_min_size(base + vec2(c.left, c.top), vec2(c.width.max(1.0), c.height.max(1.0)));
            let (kids, inner) = match c.kind {
                Kind::Frame => (Some(&c.children), Some((path.clone(), rect.min + vec2(2.0, 8.0), Rect::from_min_max(rect.min + vec2(1.0, 6.0), rect.max - vec2(1.0, 1.0))))),
                Kind::MultiPage if !c.children.is_empty() => {
                    let k = pages.get(&path).copied().unwrap_or(0).min(c.children.len() - 1);
                    let mut page = path.clone();
                    page.push(k);
                    (Some(&c.children[k].children), Some((page, rect.min + vec2(2.0, TABS + 2.0), Rect::from_min_max(rect.min + vec2(1.0, TABS + 1.0), rect.max - vec2(1.0, 1.0)))))
                }
                _ => (None, None),
            };
            out.push(Shape { path, rect, clip, inner: inner.clone() });
            if let (Some(kids), Some((p, b, r))) = (kids, inner) {
                lay(kids, &p, b, clip.intersect(r), pages, out);
            }
        }
    }
    let mut out = Vec::new();
    let form = Rect::from_min_size(Pos2::ZERO, vec2(d.width, d.height));
    lay(&d.controls, &[], Pos2::ZERO, form, pages, &mut out);
    out
}

impl Designer {
    /// The designer of module `index`, named `name`, with its form's streams.
    pub(super) fn open(index: usize, name: &str, streams: Option<&apex_vba::forms::Streams>) -> Designer {
        let design = streams.map(Design::read).unwrap_or_default();
        let read_only = streams.is_none() || design.all().iter().any(|c| c.kind == Kind::Other);
        Designer {
            module: (index, name.to_string()),
            design,
            read_only,
            sel: Vec::new(),
            tool: None,
            drag: None,
            pages: HashMap::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            clip: Vec::new(),
            clip_mark: String::new(),
            last_edit: f64::NEG_INFINITY,
            naming: None,
            name_error: None,
        }
    }

    /// Whether it is the designer of that module.
    pub(super) fn is(&self, index: usize, name: &str) -> bool {
        self.module.0 == index && self.module.1 == name
    }

    /// The names of the form's controls, for the code's object list.
    pub(super) fn objects(&self) -> Vec<(String, Kind)> {
        self.design.all().iter().filter(|c| c.kind != Kind::Other).map(|c| (c.name.clone(), c.kind)).collect()
    }

    fn push_undo(&mut self, before: Design) {
        self.undo.push(before);
        if self.undo.len() > UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn undo(&mut self) -> bool {
        let Some(d) = self.undo.pop() else { return false };
        self.redo.push(std::mem::replace(&mut self.design, d));
        self.after_swap();
        true
    }

    fn redo(&mut self) -> bool {
        let Some(d) = self.redo.pop() else { return false };
        self.undo.push(std::mem::replace(&mut self.design, d));
        self.after_swap();
        true
    }

    fn after_swap(&mut self) {
        let d = &self.design;
        self.sel.retain(|p| get(d, p).is_some());
        self.naming = None;
        self.last_edit = f64::NEG_INFINITY;
    }

    /// The designer: tools, the form and the properties. `editable`: the
    /// project can be changed.
    pub(super) fn ui(&mut self, ui: &mut egui::Ui, editable: bool, form_name: &str) -> Out {
        let mut out = Out::default();
        let editable = editable && !self.read_only;
        let before = self.design.clone();
        // Typing in a property is one step of undo while it goes on.
        let mut stepped = false;
        if self.read_only {
            ui.label(RichText::new(tr("macros.form_other")).color(ui.visuals().warn_fg_color));
        }
        ui.add_enabled_ui(editable, |ui| stepped |= self.toolbar(ui));
        let width = ui.available_width();
        let height = ui.available_height();
        let props_w = 260.0f32.min(width * 0.4);
        ui.horizontal_top(|ui| {
            let canvas_w = (width - props_w - ui.spacing().item_spacing.x * 2.0 - 6.0).max(100.0);
            ui.allocate_ui_with_layout(vec2(canvas_w, height), egui::Layout::top_down(egui::Align::Min), |ui| {
                egui::ScrollArea::both().id_salt("form_canvas").auto_shrink([false, false]).show(ui, |ui| {
                    if let Some(code) = self.canvas(ui, editable, &mut stepped) {
                        out.code = Some(code);
                    }
                });
            });
            ui.separator();
            ui.allocate_ui_with_layout(vec2(props_w, height), egui::Layout::top_down(egui::Align::Min), |ui| {
                egui::ScrollArea::vertical().id_salt("form_props").auto_shrink([false, false]).show(ui, |ui| {
                    ui.add_enabled_ui(editable, |ui| self.properties(ui, form_name));
                });
            });
        });
        if self.drag.is_none() && self.design != before {
            // Steps of their own are in undo already.
            if !stepped {
                let now = ui.input(|i| i.time);
                if now - self.last_edit > 1.0 {
                    self.push_undo(before);
                }
                self.last_edit = now;
            }
            out.changed = true;
        }
        out
    }

    /// The tools, undo and the alignment of the selection. True when it
    /// changed the design (a step of its own).
    fn toolbar(&mut self, ui: &mut egui::Ui) -> bool {
        let mut stepped = false;
        ui.horizontal_wrapped(|ui| {
            if widgets::tool_button(ui, Face::Icon(Icon::Select), self.tool.is_none(), tr("macros.form_pointer")).clicked() {
                self.tool = None;
            }
            for k in TOOLS {
                if ui.selectable_label(self.tool == Some(k), k.type_name()).clicked() {
                    self.tool = if self.tool == Some(k) { None } else { Some(k) };
                }
            }
        });
        ui.horizontal(|ui| {
            if ui.add_enabled_ui(!self.undo.is_empty(), |ui| widgets::tool_button(ui, Face::Icon(Icon::Undo), false, tr("edit.undo"))).inner.clicked() {
                stepped |= self.undo();
            }
            if ui.add_enabled_ui(!self.redo.is_empty(), |ui| widgets::tool_button(ui, Face::Icon(Icon::Redo), false, tr("edit.redo"))).inner.clicked() {
                stepped |= self.redo();
            }
            ui.separator();
            if ui.add_enabled(!self.sel.is_empty(), egui::Button::new(tr("macros.delete"))).clicked() {
                stepped |= self.delete();
            }
            let siblings = self.sel.len() > 1 && self.sel.iter().all(|p| p[..p.len() - 1] == self.sel[0][..self.sel[0].len() - 1]);
            ui.add_enabled_ui(siblings, |ui| {
                ui.menu_button(tr("macros.form_align"), |ui| {
                    for (key, how) in [
                        ("format.align_left", 0),
                        ("format.align_center", 1),
                        ("format.align_right", 2),
                        ("format.valign_top", 3),
                        ("format.valign_middle", 4),
                        ("format.valign_bottom", 5),
                    ] {
                        if ui.button(tr(key)).clicked() {
                            self.align(how);
                            stepped = true;
                        }
                    }
                });
                ui.menu_button(tr("macros.form_same_size"), |ui| {
                    for (key, w, h) in [("macros.form_same_width", true, false), ("macros.form_same_height", false, true), ("macros.form_same_both", true, true)] {
                        if ui.button(tr(key)).clicked() {
                            self.same_size(w, h);
                            stepped = true;
                        }
                    }
                });
            });
        });
        stepped
    }

    /// The form with its controls, and what the mouse and keys do on it.
    /// A double click asks for the code of an event.
    fn canvas(&mut self, ui: &mut egui::Ui, editable: bool, stepped: &mut bool) -> Option<(String, &'static str, &'static str)> {
        let d = &self.design;
        let size = vec2(px(d.width) + 2.0 * MARGIN + HANDLE, px(d.height) + 2.0 * MARGIN + TITLE + HANDLE);
        let room = ui.available_size();
        let room = vec2(if room.x.is_finite() { room.x } else { 0.0 }, if room.y.is_finite() { room.y } else { 0.0 });
        let (resp, painter) = ui.allocate_painter(size.max(room), Sense::click_and_drag());
        let id = resp.id;
        let top = resp.rect.min + vec2(MARGIN, MARGIN + TITLE);
        let to_px = |p: Pos2| top + p.to_vec2() * SCALE;
        let to_pt = |p: Pos2| Pos2::ZERO + (p - top) / SCALE;
        let rect_px = |r: Rect| Rect::from_min_max(to_px(r.min), to_px(r.max));
        let shapes = layout(d, &self.pages);
        let v = ui.visuals().clone();

        // The form: its title bar and inside, with the grid.
        let client = rect_px(Rect::from_min_size(Pos2::ZERO, vec2(d.width, d.height)));
        let bar = Rect::from_min_max(client.min - vec2(0.0, TITLE), pos2(client.right(), client.top()));
        painter.rect_filled(bar.union(client).expand(1.0), 2.0, v.widgets.inactive.bg_stroke.color);
        painter.rect_filled(bar, 0.0, v.faint_bg_color);
        painter.text(bar.left_center() + vec2(8.0, 0.0), Align2::LEFT_CENTER, &d.caption, FontId::proportional(14.0), v.strong_text_color());
        painter.rect_filled(client, 0.0, color(d.back, v.panel_fill));
        let (gx, gy) = ((d.width / GRID) as usize, (d.height / GRID) as usize);
        if gx * gy <= 40_000 {
            let dot = v.weak_text_color().gamma_multiply(0.5);
            let shown = painter.clip_rect().intersect(client);
            for i in 1..=gx {
                let x = client.left() + px(i as f32 * GRID);
                if x < shown.left() || x > shown.right() {
                    continue;
                }
                for j in 1..=gy {
                    let y = client.top() + px(j as f32 * GRID);
                    if y >= shown.top() && y <= shown.bottom() {
                        painter.rect_filled(Rect::from_center_size(pos2(x, y), Vec2::splat(1.0)), 0.0, dot);
                    }
                }
            }
        }
        for s in &shapes {
            if let Some(c) = get(d, &s.path) {
                paint(&painter.with_clip_rect(rect_px(s.clip).intersect(painter.clip_rect())), c, rect_px(s.rect), &v, self.pages.get(&s.path).copied());
            }
        }
        // The selection, with its handles when it is one control or the form.
        let grips: Vec<(i8, i8)> = [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)].into();
        let handle = |r: Rect, g: (i8, i8)| {
            let x = [r.left(), r.center().x, r.right()][(g.0 + 1) as usize];
            let y = [r.top(), r.center().y, r.bottom()][(g.1 + 1) as usize];
            Rect::from_center_size(pos2(x, y), Vec2::splat(HANDLE))
        };
        let sel_color = v.selection.stroke.color;
        let mut handles: Vec<(Rect, (i8, i8), Option<Path>)> = Vec::new();
        if self.sel.is_empty() {
            for g in [(1, 0), (0, 1), (1, 1)] {
                handles.push((handle(client, g), g, None));
            }
        } else {
            for p in &self.sel {
                if let Some(s) = shapes.iter().find(|s| &s.path == p) {
                    painter.rect_stroke(rect_px(s.rect).expand(1.0), 0.0, Stroke::new(1.0, sel_color), StrokeKind::Outside);
                    if self.sel.len() == 1 {
                        for &g in &grips {
                            handles.push((handle(rect_px(s.rect).expand(1.0), g), g, Some(p.clone())));
                        }
                    }
                } else if p.len() > 1 && get(d, p).is_some_and(|c| c.kind == Kind::Page) {
                    // A page: its tab is marked.
                    let mp = &p[..p.len() - 1];
                    if let (Some(s), Some(c)) = (shapes.iter().find(|s| s.path == mp), get(d, mp))
                        && let Some(t) = tabs(c, s.rect).get(p[p.len() - 1])
                    {
                        painter.rect_stroke(rect_px(*t), 0.0, Stroke::new(1.5, sel_color), StrokeKind::Inside);
                    }
                }
            }
        }
        for (r, _, _) in &handles {
            painter.rect_filled(*r, 0.0, if editable { sel_color } else { v.weak_text_color() });
            painter.rect_stroke(*r, 0.0, Stroke::new(1.0, v.panel_fill), StrokeKind::Inside);
        }
        if let Some((Drag::Create { from, to, .. }, _)) = &self.drag {
            painter.rect_stroke(rect_px(Rect::from_two_pos(*from, *to)), 0.0, Stroke::new(1.0, sel_color), StrokeKind::Inside);
        }
        // The cursor over a handle, a control, or with a tool.
        if let Some(h) = resp.hover_pos() {
            if let Some((_, g, _)) = handles.iter().find(|x| x.0.contains(h)).filter(|_| editable) {
                ui.ctx().set_cursor_icon(match g {
                    (0, _) => egui::CursorIcon::ResizeVertical,
                    (_, 0) => egui::CursorIcon::ResizeHorizontal,
                    (a, b) if a == b => egui::CursorIcon::ResizeNwSe,
                    _ => egui::CursorIcon::ResizeNeSw,
                });
            } else if self.tool.is_some() && editable {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
        }

        // The mouse.
        let hit = |p: Pos2| shapes.iter().rev().find(|s| s.rect.contains(p) && s.clip.contains(p));
        let pressed = resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_pressed());
        let alt = ui.input(|i| i.modifiers.alt);
        let mut code = None;
        if pressed && let Some(at) = ui.input(|i| i.pointer.press_origin()) {
            resp.request_focus();
            let p = to_pt(at);
            let before = self.design.clone();
            if !editable {
                self.pick(hit(p).map(|s| s.path.clone()), false);
            } else if let Some((_, g, t)) = handles.iter().find(|x| x.0.contains(at)) {
                let orig = match t {
                    Some(t) => get(&self.design, t).map(|c| [c.left, c.top, c.width, c.height]),
                    None => Some([0.0, 0.0, self.design.width, self.design.height]),
                };
                if let Some(orig) = orig {
                    self.drag = Some((Drag::Size { from: at, grip: *g, target: t.clone(), orig }, before));
                }
            } else if self.tool.is_some() {
                // In the deepest Frame or page there.
                let (container, base) = shapes
                    .iter()
                    .rev()
                    .find_map(|s| s.inner.as_ref().filter(|i| i.2.contains(p) && s.clip.contains(p)).map(|i| (i.0.clone(), i.1)))
                    .unwrap_or((Vec::new(), Pos2::ZERO));
                self.drag = Some((Drag::Create { container, base, from: p, to: p }, before));
            } else {
                let ctrl = ui.input(|i| i.modifiers.command || i.modifiers.shift);
                let target = hit(p).map(|s| s.path.clone());
                self.pick(target, ctrl);
                if !ctrl && !self.sel.is_empty() && self.sel.iter().all(|s| get(&self.design, s).is_some_and(|c| c.kind != Kind::Page)) {
                    let orig = self.sel.iter().filter_map(|s| get(&self.design, s).map(|c| (s.clone(), c.left, c.top))).collect();
                    self.drag = Some((Drag::Move { from: at, orig, moved: false }, before));
                }
            }
        }
        if let Some(at) = ui.input(|i| i.pointer.interact_pos())
            && let Some((drag, _)) = &mut self.drag
        {
            let delta = (at - match drag {
                Drag::Move { from, .. } | Drag::Size { from, .. } => *from,
                Drag::Create { .. } => at,
            }) / SCALE;
            match drag {
                Drag::Move { orig, moved, from, .. } => {
                    if !*moved && (at - *from).length() > 3.0 {
                        *moved = true;
                    }
                    if *moved {
                        for (p, l, t) in orig.iter() {
                            if let Some(c) = get_mut(&mut self.design, p) {
                                c.left = snap(l + delta.x, !alt);
                                c.top = snap(t + delta.y, !alt);
                            }
                        }
                    }
                }
                Drag::Size { grip, target, orig, .. } => {
                    let [l, t, w, h] = *orig;
                    let side = |g: i8, start: f32, len: f32, d: f32, min: f32| match g {
                        -1 => {
                            let s = snap(start + d, !alt).min(start + len - min);
                            (s, start + len - s)
                        }
                        1 => (start, (snap(start + len + d, !alt) - start).max(min)),
                        _ => (start, len),
                    };
                    match target {
                        Some(path) => {
                            let (nl, nw) = side(grip.0, l, w, delta.x, 3.0);
                            let (nt, nh) = side(grip.1, t, h, delta.y, 3.0);
                            if let Some(c) = get_mut(&mut self.design, path) {
                                (c.left, c.top, c.width, c.height) = (nl, nt, nw.min(4000.0), nh.min(4000.0));
                            }
                        }
                        None => {
                            self.design.width = side(grip.0, 0.0, w, delta.x, 20.0).1.min(4000.0);
                            self.design.height = side(grip.1, 0.0, h, delta.y, 20.0).1.min(4000.0);
                        }
                    }
                }
                Drag::Create { to, .. } => *to = to_pt(at),
            }
        }
        if self.drag.is_some() && !ui.input(|i| i.pointer.primary_down()) {
            let (drag, before) = self.drag.take().unwrap();
            if let Drag::Create { container, base, from, to } = drag
                && let Some(kind) = self.tool.take()
            {
                let r = Rect::from_two_pos(from, to);
                let (l, t) = (snap(r.left() - base.x, !alt), snap(r.top() - base.y, !alt));
                let size = (r.width() >= 3.0 || r.height() >= 3.0).then(|| (snap(r.right() - base.x, !alt) - l, snap(r.bottom() - base.y, !alt) - t));
                self.create(kind, &container, l, t, size);
            }
            if self.design != before {
                self.push_undo(before);
                *stepped = true;
            }
        }
        // A click on a MultiPage's tab shows its page.
        if resp.clicked() && let Some(at) = resp.interact_pointer_pos() {
            let p = to_pt(at);
            if let Some(s) = hit(p)
                && let Some(c) = get(&self.design, &s.path).filter(|c| c.kind == Kind::MultiPage)
                && let Some(k) = tabs(c, s.rect).iter().position(|t| t.contains(p))
            {
                self.pages.insert(s.path.clone(), k);
                let mut page = s.path.clone();
                page.push(k);
                self.sel = vec![page];
            }
        }
        if resp.double_clicked() && let Some(at) = resp.interact_pointer_pos() {
            let p = to_pt(at);
            let target = hit(p).and_then(|s| get(&self.design, &s.path));
            if client.contains(at) || target.is_some() {
                let kind = target.map(|c| c.kind);
                let (event, params) = default_event(kind);
                code = Some((target.map_or_else(|| "UserForm".to_string(), |c| c.name.clone()), event, params));
            }
        }

        // The keys, while the form has the focus.
        if resp.has_focus() {
            ui.memory_mut(|m| m.set_focus_lock_filter(id, egui::EventFilter { horizontal_arrows: true, vertical_arrows: true, tab: false, escape: true }));
            let (events, mods) = ui.input(|i| (i.events.clone(), i.modifiers));
            for e in events {
                match e {
                    egui::Event::Copy if !self.sel.is_empty() => {
                        self.copy(ui.ctx());
                    }
                    egui::Event::Cut if editable && !self.sel.is_empty() => {
                        self.copy(ui.ctx());
                        *stepped |= self.delete();
                    }
                    egui::Event::Paste(t) if editable && !self.clip.is_empty() && t == self.clip_mark => {
                        *stepped |= self.paste();
                    }
                    egui::Event::Key { key, pressed: true, modifiers, .. } => match key {
                        egui::Key::Delete if editable => *stepped |= self.delete(),
                        egui::Key::Escape => {
                            self.tool = None;
                            self.sel.clear();
                        }
                        egui::Key::Z if modifiers.command && editable => *stepped |= if modifiers.shift { self.redo() } else { self.undo() },
                        egui::Key::Y if modifiers.command && editable => *stepped |= self.redo(),
                        egui::Key::A if modifiers.command => {
                            self.sel = (0..self.design.controls.len()).map(|i| vec![i]).collect();
                        }
                        egui::Key::ArrowLeft | egui::Key::ArrowRight | egui::Key::ArrowUp | egui::Key::ArrowDown if editable => {
                            let (dx, dy) = match key {
                                egui::Key::ArrowLeft => (-1.0, 0.0),
                                egui::Key::ArrowRight => (1.0, 0.0),
                                egui::Key::ArrowUp => (0.0, -1.0),
                                _ => (0.0, 1.0),
                            };
                            let step = if mods.command { 1.0 } else { GRID };
                            let before = self.design.clone();
                            for p in self.sel.clone() {
                                if let Some(c) = get_mut(&mut self.design, &p).filter(|c| c.kind != Kind::Page) {
                                    if modifiers.shift {
                                        c.width = (c.width + dx * step).clamp(3.0, 4000.0);
                                        c.height = (c.height + dy * step).clamp(3.0, 4000.0);
                                    } else {
                                        c.left += dx * step;
                                        c.top += dy * step;
                                    }
                                }
                            }
                            if self.design != before {
                                self.push_undo(before);
                                *stepped = true;
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
        code
    }

    /// A press on `target` (None: the form): it is selected, or
    /// added to or taken from the selection (`toggle`).
    fn pick(&mut self, target: Option<Path>, toggle: bool) {
        match target {
            None => {
                if !toggle {
                    self.sel.clear();
                }
            }
            Some(t) if toggle => {
                if let Some(i) = self.sel.iter().position(|s| *s == t) {
                    self.sel.remove(i);
                } else {
                    // Only controls in the same container go together.
                    self.sel.retain(|s| s.len() == t.len() && s[..s.len() - 1] == t[..t.len() - 1]);
                    self.sel.push(t);
                }
            }
            Some(t) => {
                if !self.sel.contains(&t) {
                    self.sel = vec![t];
                }
            }
        }
    }

    /// A new control of `kind` in `container`, at `left, top` with `size`
    /// (its usual one if None); it is selected.
    fn create(&mut self, kind: Kind, container: &[usize], left: f32, top: f32, size: Option<(f32, f32)>) {
        if self.design.all().len() >= MAX_CONTROLS {
            return;
        }
        let name = fresh(&self.design, kind.type_name());
        let mut c = Control::new(kind, &name);
        if matches!(kind, Kind::Label | Kind::CheckBox | Kind::OptionButton | Kind::ToggleButton | Kind::CommandButton | Kind::Frame) {
            c.caption = name.clone();
        }
        if kind == Kind::MultiPage {
            let mut used = names(&self.design);
            for _ in 0..2 {
                let page = free_name(&used, "Page");
                used.insert(page.to_lowercase());
                let mut p = Control::new(Kind::Page, &page);
                p.caption = page;
                c.children.push(p);
            }
        }
        c.left = left;
        c.top = top;
        if let Some((w, h)) = size {
            c.width = w.clamp(3.0, 4000.0);
            c.height = h.clamp(3.0, 4000.0);
        }
        let Some(list) = list_mut(&mut self.design, container) else { return };
        c.tab_index = list.len() as i32;
        list.push(c);
        let mut path = container.to_vec();
        path.push(list.len() - 1);
        self.sel = vec![path];
    }

    /// Deletes the selected controls (not the last page of a MultiPage).
    fn delete(&mut self) -> bool {
        let before = self.design.clone();
        let mut sel = std::mem::take(&mut self.sel);
        // From the last: a removal moves only what comes after it.
        sel.sort();
        sel.dedup();
        for p in sel.iter().rev() {
            let (i, parent) = p.split_last().expect("paths aren't empty");
            let is_page = get(&self.design, p).is_some_and(|c| c.kind == Kind::Page);
            if let Some(list) = list_mut(&mut self.design, parent)
                && *i < list.len()
                && (!is_page || list.len() > 1)
            {
                list.remove(*i);
                if is_page {
                    self.pages.remove(parent);
                }
            }
        }
        if self.design != before {
            self.push_undo(before);
            true
        } else {
            false
        }
    }

    fn copy(&mut self, ctx: &egui::Context) {
        self.clip = self.sel.iter().filter_map(|p| get(&self.design, p)).filter(|c| c.kind != Kind::Page).cloned().collect();
        self.clip_mark = self.clip.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(" ");
        ctx.copy_text(self.clip_mark.clone());
    }

    /// The copied controls beside the selection (in it, a page), a little
    /// down and right, with names of their own.
    fn paste(&mut self) -> bool {
        let container: Path = match self.sel.first() {
            Some(p) if get(&self.design, p).is_some_and(|c| c.kind == Kind::Page) => p.clone(),
            Some(p) => p[..p.len() - 1].to_vec(),
            None => Vec::new(),
        };
        let adding: usize = self.clip.iter().map(|c| {
            let mut v = vec![c];
            c.walk(&mut v);
            v.len()
        }).sum();
        if self.design.all().len() + adding > MAX_CONTROLS {
            return false;
        }
        let before = self.design.clone();
        let mut used = names(&self.design);
        let mut new_sel = Vec::new();
        for mut c in self.clip.clone() {
            rename_all(&mut used, &mut c);
            c.left += GRID;
            c.top += GRID;
            let Some(list) = list_mut(&mut self.design, &container) else { break };
            c.tab_index = list.len() as i32;
            list.push(c);
            let mut p = container.clone();
            p.push(list.len() - 1);
            new_sel.push(p);
        }
        // Pasted again, they go further.
        for c in &mut self.clip {
            c.left += GRID;
            c.top += GRID;
        }
        self.sel = new_sel;
        if self.design != before {
            self.push_undo(before);
            true
        } else {
            false
        }
    }

    /// Lines the selection up with its first control: `how` 0 lefts, 1
    /// centers, 2 rights, 3 tops, 4 middles, 5 bottoms.
    fn align(&mut self, how: u8) {
        let Some(first) = self.sel.first().and_then(|p| get(&self.design, p)).cloned() else { return };
        let before = self.design.clone();
        for p in self.sel.clone() {
            if let Some(c) = get_mut(&mut self.design, &p) {
                match how {
                    0 => c.left = first.left,
                    1 => c.left = first.left + (first.width - c.width) / 2.0,
                    2 => c.left = first.left + first.width - c.width,
                    3 => c.top = first.top,
                    4 => c.top = first.top + (first.height - c.height) / 2.0,
                    _ => c.top = first.top + first.height - c.height,
                }
            }
        }
        if self.design != before {
            self.push_undo(before);
        }
    }

    /// Gives the selection the width and/or height of its first control.
    fn same_size(&mut self, w: bool, h: bool) {
        let Some(first) = self.sel.first().and_then(|p| get(&self.design, p)).cloned() else { return };
        let before = self.design.clone();
        for p in self.sel.clone() {
            if let Some(c) = get_mut(&mut self.design, &p) {
                if w {
                    c.width = first.width;
                }
                if h {
                    c.height = first.height;
                }
            }
        }
        if self.design != before {
            self.push_undo(before);
        }
    }

    /// The properties of the selected control, or of the form.
    fn properties(&mut self, ui: &mut egui::Ui, form_name: &str) {
        ui.strong(tr("image.properties"));
        if self.sel.len() > 1 {
            ui.label(format!("{} ×", self.sel.len()));
            return;
        }
        let Some(path) = self.sel.first().cloned() else {
            let d = &mut self.design;
            egui::Grid::new("form_props_grid").num_columns(2).striped(true).show(ui, |ui| {
                ui.label("(Name)");
                ui.label(form_name);
                ui.end_row();
                text_row(ui, "Caption", &mut d.caption);
                num_row(ui, "Width", &mut d.width, 20.0..=4000.0);
                num_row(ui, "Height", &mut d.height, 20.0..=4000.0);
                color_row(ui, "BackColor", &mut d.back);
                ui.label("StartUpPosition");
                let names = ["0 - Manual", "1 - CenterOwner", "2 - CenterScreen", "3 - WindowsDefault"];
                egui::ComboBox::from_id_salt("form_startup").selected_text(names.get(d.start_up as usize).copied().unwrap_or("")).show_ui(ui, |ui| {
                    for (i, n) in names.iter().enumerate() {
                        ui.selectable_value(&mut d.start_up, i as i32, *n);
                    }
                });
                ui.end_row();
                if d.start_up == 0 {
                    num_row(ui, "Left", &mut d.left, 0.0..=4000.0);
                    num_row(ui, "Top", &mut d.top, 0.0..=4000.0);
                }
            });
            return;
        };
        let Some(kind) = get(&self.design, &path).map(|c| c.kind) else {
            self.sel.clear();
            return;
        };
        // The name: taken while typed when it is a good one.
        let current = get(&self.design, &path).map(|c| c.name.clone()).unwrap_or_default();
        let mut rename = None;
        let mut left = false;
        egui::Grid::new("ctl_props_name").num_columns(2).show(ui, |ui| {
            ui.label("(Name)");
            let text = match &mut self.naming {
                Some((p, t)) if *p == path => t,
                _ => {
                    self.name_error = None;
                    self.naming = Some((path.clone(), current.clone()));
                    &mut self.naming.as_mut().expect("just set").1
                }
            };
            let r = ui.add(egui::TextEdit::singleline(text).char_limit(40).desired_width(140.0));
            if r.changed() {
                rename = Some(text.trim().to_string());
            }
            left = r.lost_focus();
            ui.end_row();
        });
        if let Some(new) = rename {
            self.name_error = if !valid_name(&new) {
                Some("macros.name_invalid")
            } else if !new.eq_ignore_ascii_case(&current) && taken(&self.design, &new) {
                Some("macros.name_taken")
            } else {
                if let Some(c) = get_mut(&mut self.design, &path) {
                    c.name = new;
                }
                None
            };
        }
        // Left with a bad name: the control keeps its own.
        if left {
            self.naming = None;
            self.name_error = None;
        }
        if let Some(e) = self.name_error {
            ui.label(RichText::new(tr(e)).color(ui.visuals().error_fg_color));
        }
        let Some(c) = get_mut(&mut self.design, &path) else { return };
        let k = |ks: &[Kind]| ks.contains(&kind);
        egui::Grid::new("ctl_props_grid").num_columns(2).striped(true).show(ui, |ui| {
            ui.label("Type");
            ui.label(kind.type_name());
            ui.end_row();
            if k(&[Kind::Label, Kind::CheckBox, Kind::OptionButton, Kind::ToggleButton, Kind::CommandButton, Kind::Frame, Kind::Page]) {
                text_row(ui, "Caption", &mut c.caption);
            }
            if k(&[Kind::TextBox, Kind::ComboBox]) {
                text_row(ui, "Text", &mut c.value);
            }
            if k(&[Kind::CheckBox, Kind::OptionButton, Kind::ToggleButton]) {
                ui.label("Value");
                let names = [("0", "False"), ("1", "True"), ("", "Null")];
                let shown = names.iter().find(|n| n.0 == c.value).map_or("False", |n| n.1);
                egui::ComboBox::from_id_salt("ctl_value").selected_text(shown).show_ui(ui, |ui| {
                    for (v, n) in names {
                        if ui.selectable_label(c.value == v, n).clicked() {
                            c.value = v.to_string();
                        }
                    }
                });
                ui.end_row();
            }
            if k(&[Kind::ScrollBar, Kind::SpinButton]) {
                let mut v: i32 = c.value.trim().parse().unwrap_or(0);
                let (lo, hi) = (c.min.min(c.max), c.min.max(c.max));
                int_row(ui, "Value", &mut v, lo..=hi);
                if v.to_string() != c.value.trim() && !(c.value.trim().is_empty() && v == 0) {
                    c.value = v.to_string();
                }
                int_row(ui, "Min", &mut c.min, -32768..=32767);
                int_row(ui, "Max", &mut c.max, -32768..=32767);
                int_row(ui, "SmallChange", &mut c.small_change, 0..=32767);
                if kind == Kind::ScrollBar {
                    int_row(ui, "LargeChange", &mut c.large_change, 0..=32767);
                }
            }
            if kind != Kind::Page {
                num_row(ui, "Left", &mut c.left, -4000.0..=4000.0);
                num_row(ui, "Top", &mut c.top, -4000.0..=4000.0);
                num_row(ui, "Width", &mut c.width, 3.0..=4000.0);
                num_row(ui, "Height", &mut c.height, 3.0..=4000.0);
            }
            bool_row(ui, "Visible", &mut c.visible);
            bool_row(ui, "Enabled", &mut c.enabled);
            if k(&[Kind::TextBox, Kind::ComboBox, Kind::ListBox, Kind::CheckBox, Kind::OptionButton, Kind::ToggleButton]) {
                bool_row(ui, "Locked", &mut c.locked);
            }
            if kind == Kind::TextBox {
                bool_row(ui, "MultiLine", &mut c.multiline);
                int_row(ui, "MaxLength", &mut c.max_length, 0..=65535);
                ui.label("PasswordChar");
                ui.add(egui::TextEdit::singleline(&mut c.password_char).char_limit(1).desired_width(30.0));
                ui.end_row();
            }
            if k(&[Kind::Label, Kind::TextBox, Kind::CheckBox, Kind::OptionButton, Kind::ToggleButton, Kind::CommandButton]) {
                bool_row(ui, "WordWrap", &mut c.word_wrap);
            }
            if kind == Kind::OptionButton {
                text_row(ui, "GroupName", &mut c.group);
            }
            if kind == Kind::CommandButton {
                bool_row(ui, "Default", &mut c.default);
                bool_row(ui, "Cancel", &mut c.cancel);
            }
            if k(&[Kind::TextBox, Kind::ComboBox, Kind::ListBox, Kind::CheckBox, Kind::OptionButton, Kind::ToggleButton, Kind::ScrollBar, Kind::SpinButton]) {
                text_row(ui, "ControlSource", &mut c.control_source);
            }
            if k(&[Kind::ComboBox, Kind::ListBox]) {
                text_row(ui, "RowSource", &mut c.row_source);
                int_row(ui, "ColumnCount", &mut c.column_count, -1..=100);
            }
            if kind == Kind::ComboBox {
                ui.label("Style");
                egui::ComboBox::from_id_salt("ctl_style").selected_text(if c.drop_down_list { "2 - DropDownList" } else { "0 - DropDownCombo" }).show_ui(ui, |ui| {
                    ui.selectable_value(&mut c.drop_down_list, false, "0 - DropDownCombo");
                    ui.selectable_value(&mut c.drop_down_list, true, "2 - DropDownList");
                });
                ui.end_row();
            }
            if kind == Kind::ListBox {
                ui.label("MultiSelect");
                let names = ["0 - Single", "1 - Multi", "2 - Extended"];
                egui::ComboBox::from_id_salt("ctl_multi").selected_text(names.get(c.multi_select as usize).copied().unwrap_or("")).show_ui(ui, |ui| {
                    for (i, n) in names.iter().enumerate() {
                        ui.selectable_value(&mut c.multi_select, i as u8, *n);
                    }
                });
                ui.end_row();
            }
            if kind != Kind::Page {
                int_row(ui, "TabIndex", &mut c.tab_index, 0..=MAX_CONTROLS as i32);
                bool_row(ui, "TabStop", &mut c.tab_stop);
                color_row(ui, "BackColor", &mut c.back);
                color_row(ui, "ForeColor", &mut c.fore);
            }
            if !k(&[Kind::Image, Kind::ScrollBar, Kind::SpinButton, Kind::Page]) {
                num_row(ui, "Font size", &mut c.font_size, 1.0..=400.0);
                bool_row(ui, "Bold", &mut c.bold);
                bool_row(ui, "Italic", &mut c.italic);
            }
            text_row(ui, "ControlTipText", &mut c.tip);
            text_row(ui, "Tag", &mut c.tag);
        });
        // A MultiPage's pages: one more, or this one less.
        if matches!(kind, Kind::MultiPage | Kind::Page) {
            let mp: Path = if kind == Kind::Page { path[..path.len() - 1].to_vec() } else { path.clone() };
            ui.horizontal(|ui| {
                if ui.button(tr("macros.form_new_page")).clicked() && self.design.all().len() < MAX_CONTROLS {
                    let name = fresh(&self.design, "Page");
                    let mut p = Control::new(Kind::Page, &name);
                    p.caption = name;
                    if let Some(m) = get_mut(&mut self.design, &mp) {
                        m.children.push(p);
                        let k = m.children.len() - 1;
                        self.pages.insert(mp.clone(), k);
                        let mut page = mp.clone();
                        page.push(k);
                        self.sel = vec![page];
                    }
                }
                let pages = get(&self.design, &mp).map_or(0, |m| m.children.len());
                if kind == Kind::Page && ui.add_enabled(pages > 1, egui::Button::new(tr("macros.form_delete_page"))).clicked() {
                    self.delete();
                }
            });
        }
    }
}

/// Gives `c` and what it holds names not in `used` (when they are), and
/// adds them to it.
fn rename_all(used: &mut HashSet<String>, c: &mut Control) {
    if used.contains(&c.name.to_lowercase()) {
        let base = c.name.trim_end_matches(|ch: char| ch.is_ascii_digit());
        let base = if base.is_empty() { c.kind.type_name() } else { base };
        c.name = free_name(used, base);
    }
    used.insert(c.name.to_lowercase());
    for k in &mut c.children {
        rename_all(used, k);
    }
}

fn text_row(ui: &mut egui::Ui, label: &str, v: &mut String) {
    ui.label(label);
    ui.add(egui::TextEdit::singleline(v).char_limit(MAX_TEXT).desired_width(140.0));
    ui.end_row();
}

fn num_row(ui: &mut egui::Ui, label: &str, v: &mut f32, range: std::ops::RangeInclusive<f32>) {
    ui.label(label);
    ui.add(egui::DragValue::new(v).range(range).speed(1.0).max_decimals(2));
    ui.end_row();
}

fn int_row(ui: &mut egui::Ui, label: &str, v: &mut i32, range: std::ops::RangeInclusive<i32>) {
    ui.label(label);
    ui.add(egui::DragValue::new(v).range(range));
    ui.end_row();
}

fn bool_row(ui: &mut egui::Ui, label: &str, v: &mut bool) {
    ui.label(label);
    ui.checkbox(v, "");
    ui.end_row();
}

/// A color, or the usual one (unticked).
fn color_row(ui: &mut egui::Ui, label: &str, v: &mut Option<u32>) {
    ui.label(label);
    ui.horizontal(|ui| {
        let mut set = v.is_some();
        if ui.checkbox(&mut set, "").changed() {
            *v = set.then_some(0x00FF_FFFF);
        }
        if let Some(c) = v {
            let mut rgb = [(*c >> 16) as u8, (*c >> 8) as u8, *c as u8];
            if ui.color_edit_button_srgb(&mut rgb).changed() {
                *c = (rgb[0] as u32) << 16 | (rgb[1] as u32) << 8 | rgb[2] as u32;
            }
        }
    });
    ui.end_row();
}

/// Draws control `c` in `r` (pixels): `page` is the page a MultiPage shows.
fn paint(p: &egui::Painter, c: &Control, r: Rect, v: &egui::Visuals, page: Option<usize>) {
    let fore = color(c.fore, v.text_color());
    let font = FontId::proportional(px(c.font_size).clamp(6.0, 64.0));
    let edge = Stroke::new(1.0, v.weak_text_color().gamma_multiply(0.7));
    let field = color(c.back, v.extreme_bg_color);
    let face = color(c.back, v.widgets.inactive.weak_bg_fill);
    if let Some(b) = c.back.filter(|_| matches!(c.kind, Kind::Label | Kind::CheckBox | Kind::OptionButton | Kind::Frame)) {
        p.rect_filled(r, 0.0, color(Some(b), Color32::TRANSPARENT));
    }
    let label_at = |x: f32| pos2(x, r.center().y);
    match c.kind {
        Kind::Label => {
            p.text(r.min + vec2(1.0, 1.0), Align2::LEFT_TOP, &c.caption, font, fore);
        }
        Kind::TextBox | Kind::ListBox => {
            p.rect_filled(r, 0.0, field);
            p.rect_stroke(r, 0.0, edge, StrokeKind::Inside);
            p.text(r.min + vec2(3.0, 2.0), Align2::LEFT_TOP, &c.value, font, fore);
        }
        Kind::ComboBox => {
            p.rect_filled(r, 0.0, field);
            p.rect_stroke(r, 0.0, edge, StrokeKind::Inside);
            let b = Rect::from_min_max(pos2(r.right() - r.height().min(18.0), r.top()), r.max);
            p.rect_filled(b.shrink(1.0), 0.0, v.widgets.inactive.weak_bg_fill);
            p.text(b.center(), Align2::CENTER_CENTER, "▾", FontId::proportional(12.0), v.text_color());
            p.text(label_at(r.left() + 3.0), Align2::LEFT_CENTER, &c.value, font, fore);
        }
        Kind::CheckBox | Kind::OptionButton => {
            let b = Rect::from_center_size(pos2(r.left() + 8.0, r.center().y), Vec2::splat(11.0));
            if c.kind == Kind::CheckBox {
                p.rect_filled(b, 0.0, v.extreme_bg_color);
                p.rect_stroke(b, 0.0, edge, StrokeKind::Inside);
                if c.value == "1" {
                    p.text(b.center(), Align2::CENTER_CENTER, "✔", FontId::proportional(10.0), v.text_color());
                }
            } else {
                p.circle_filled(b.center(), 5.5, v.extreme_bg_color);
                p.circle_stroke(b.center(), 5.5, edge);
                if c.value == "1" {
                    p.circle_filled(b.center(), 2.5, v.text_color());
                }
            }
            p.text(label_at(r.left() + 18.0), Align2::LEFT_CENTER, &c.caption, font, fore);
        }
        Kind::CommandButton | Kind::ToggleButton => {
            let down = c.kind == Kind::ToggleButton && c.value == "1";
            p.rect_filled(r, 3.0, if down { v.widgets.active.weak_bg_fill } else { face });
            p.rect_stroke(r, 3.0, edge, StrokeKind::Inside);
            p.text(r.center(), Align2::CENTER_CENTER, &c.caption, font, fore);
        }
        Kind::Frame => {
            let b = Rect::from_min_max(r.min + vec2(0.0, 6.0), r.max);
            p.rect_stroke(b, 2.0, edge, StrokeKind::Inside);
            if !c.caption.is_empty() {
                let g = p.layout_no_wrap(c.caption.clone(), font, fore);
                let at = r.min + vec2(8.0, 0.0);
                p.rect_filled(Rect::from_min_size(at - vec2(2.0, 0.0), g.size() + vec2(4.0, 0.0)), 0.0, color(c.back, v.panel_fill));
                p.galley(at, g, fore);
            }
        }
        Kind::MultiPage | Kind::TabStrip => {
            let body = Rect::from_min_max(r.min + vec2(0.0, px(TABS)), r.max);
            p.rect_stroke(body, 0.0, edge, StrokeKind::Inside);
            if c.kind == Kind::MultiPage {
                let pt = Rect::from_min_size(Pos2::ZERO, r.size() / SCALE);
                let shown = page.unwrap_or(0).min(c.children.len().saturating_sub(1));
                for (k, (t, pg)) in tabs(c, pt).into_iter().zip(&c.children).enumerate() {
                    let t = Rect::from_min_max(r.min + t.min.to_vec2() * SCALE, r.min + t.max.to_vec2() * SCALE);
                    p.rect_filled(t, 0.0, if k == shown { v.panel_fill } else { face });
                    p.rect_stroke(t, 0.0, edge, StrokeKind::Inside);
                    p.text(t.center(), Align2::CENTER_CENTER, &pg.caption, FontId::proportional(11.0), fore);
                }
            }
        }
        Kind::SpinButton | Kind::ScrollBar => {
            p.rect_filled(r, 0.0, v.widgets.inactive.weak_bg_fill.gamma_multiply(0.8));
            p.rect_stroke(r, 0.0, edge, StrokeKind::Inside);
            let wide = r.width() > r.height();
            let (a, b) = if wide { ("◂", "▸") } else { ("▴", "▾") };
            let s = r.width().min(r.height());
            let (ra, rb) = if wide {
                (Rect::from_min_size(r.min, vec2(s, r.height())), Rect::from_min_max(pos2(r.right() - s, r.top()), r.max))
            } else {
                (Rect::from_min_size(r.min, vec2(r.width(), s.min(r.height() / 2.0))), Rect::from_min_max(pos2(r.left(), r.bottom() - s.min(r.height() / 2.0)), r.max))
            };
            for (rr, t) in [(ra, a), (rb, b)] {
                p.rect_filled(rr.shrink(1.0), 0.0, face);
                p.text(rr.center(), Align2::CENTER_CENTER, t, FontId::proportional(10.0), v.text_color());
            }
        }
        Kind::Image => {
            p.rect_filled(r, 0.0, color(c.back, v.faint_bg_color));
            p.rect_stroke(r, 0.0, edge, StrokeKind::Inside);
            if c.picture.is_some() {
                p.text(r.center(), Align2::CENTER_CENTER, "🖼", FontId::proportional(16.0), v.weak_text_color());
            }
        }
        Kind::Page | Kind::Other => {
            p.rect_filled(r, 0.0, v.faint_bg_color);
            p.rect_stroke(r, 0.0, Stroke::new(1.0, v.weak_text_color()), StrokeKind::Inside);
            p.text(r.center(), Align2::CENTER_CENTER, &c.name, FontId::proportional(11.0), v.weak_text_color());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn designer() -> Designer {
        let mut d = Designer::open(0, "UserForm1", Some(&Design::default().write("UserForm1")));
        d.create(Kind::CommandButton, &[], 12.0, 12.0, None);
        d.create(Kind::Frame, &[], 60.0, 12.0, Some((120.0, 90.0)));
        d.create(Kind::TextBox, &[1], 6.0, 6.0, None);
        d.create(Kind::MultiPage, &[], 12.0, 110.0, Some((200.0, 60.0)));
        d
    }

    #[test]
    fn controls_get_names_of_their_own() {
        let d = designer();
        let names: Vec<&str> = d.design.all().iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["CommandButton1", "Frame1", "TextBox1", "MultiPage1", "Page1", "Page2"]);
        assert_eq!(get(&d.design, &[0]).unwrap().caption, "CommandButton1");
        assert_eq!(d.sel, vec![vec![2]]);
        assert!(!d.read_only);
    }

    #[test]
    fn layout_finds_controls_inside_containers() {
        let mut d = designer();
        let shapes = layout(&d.design, &d.pages);
        let hit = |p: Pos2| shapes.iter().rev().find(|s| s.rect.contains(p) && s.clip.contains(p)).map(|s| s.path.clone());
        // The TextBox in the Frame: the Frame's inside starts 2 and 8 points in.
        assert_eq!(hit(pos2(60.0 + 2.0 + 6.0 + 1.0, 12.0 + 8.0 + 6.0 + 1.0)), Some(vec![1, 0]));
        assert_eq!(hit(pos2(61.0, 100.0)), Some(vec![1]));
        assert_eq!(hit(pos2(5.0, 5.0)), None);
        // A page's controls are its MultiPage's while it shows.
        d.create(Kind::Label, &[2, 1], 4.0, 4.0, None);
        d.pages.insert(vec![2], 1);
        let shapes = layout(&d.design, &d.pages);
        assert!(shapes.iter().any(|s| s.path == [2, 1, 0]));
        d.pages.insert(vec![2], 0);
        assert!(!layout(&d.design, &d.pages).iter().any(|s| s.path == [2, 1, 0]));
    }

    #[test]
    fn delete_paste_and_undo() {
        let mut d = designer();
        d.sel = vec![vec![1, 0], vec![0]];
        assert!(d.delete());
        assert_eq!(d.design.controls.len(), 2);
        assert!(d.design.controls[0].children.is_empty());
        assert!(d.undo());
        assert_eq!(d.design.controls.len(), 3);
        // The last page of a MultiPage stays.
        d.sel = vec![vec![2, 0], vec![2, 1]];
        d.delete();
        assert_eq!(d.design.controls[2].children.len(), 1);
        // A Frame pasted: it and its TextBox are renamed.
        d.sel = vec![vec![1]];
        let ctx = egui::Context::default();
        d.copy(&ctx);
        assert!(d.paste());
        let names: Vec<String> = d.design.all().iter().map(|c| c.name.clone()).collect();
        let mut unique = names.clone();
        unique.sort_by_key(|n| n.to_lowercase());
        unique.dedup_by_key(|n| n.to_lowercase());
        assert_eq!(unique.len(), names.len(), "{names:?}");
        assert_eq!(d.design.controls.len(), 4);
        assert_eq!(d.design.controls[3].left, 66.0);
        assert_eq!(d.sel, vec![vec![3]]);
    }

    #[test]
    fn the_design_written_reads_back() {
        let d = designer();
        let back = Design::read(&d.design.write("UserForm1"));
        let names = |d: &Design| d.all().iter().map(|c| (c.name.clone(), c.kind)).collect::<Vec<_>>();
        assert_eq!(names(&back), names(&d.design));
    }

    #[test]
    fn names_and_events() {
        assert!(valid_name("txtName_2") && !valid_name("2x") && !valid_name("a b") && !valid_name(""));
        assert_eq!(default_event(Some(Kind::TextBox)).0, "Change");
        assert_eq!(default_event(None).0, "Click");
        assert!(events(Some(Kind::CommandButton)).iter().any(|e| e.0 == "Click"));
        assert!(!events(Some(Kind::Label)).iter().any(|e| e.0 == "Change"));
        assert!(events(None).iter().any(|e| e.0 == "QueryClose"));
    }
}
