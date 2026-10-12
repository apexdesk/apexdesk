//! Form controls over the cells (see [`apex_sheet::form_control`]):
//! painting them, operating them with a click (Ctrl+click or a right
//! click selects them to move, size or format them, as in Excel),
//! inserting them and the Format Control dialog.

use apex_doc::{Shape, ShapeKind};
use apex_sheet::{ControlKind, FormControl, SheetShape};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Stroke, StrokeKind, pos2, vec2};

use super::SheetView;
use crate::i18n::tr;
use crate::icons::Icon;
use crate::widgets::{self, DialogResult};

const FACE: Color32 = Color32::from_rgb(0xF0, 0xF0, 0xF0);
const EDGE: Color32 = Color32::from_rgb(0x8A, 0x8A, 0x8A);
const INK: Color32 = Color32::from_rgb(0x20, 0x20, 0x20);
const PICK: Color32 = Color32::from_rgb(0x33, 0x66, 0xCC);
/// Height of a list's row, sheet pixels.
const ROW: f32 = 16.0;

#[derive(Default)]
pub(super) struct ControlsState {
    dialog: Option<Dlg>,
    /// An open drop-down: sheet, control and its screen box.
    popup: Option<(usize, u32, Rect)>,
}

struct Dlg {
    sheet: usize,
    orig: SheetShape,
    c: FormControl,
}

pub(super) fn label_key(k: ControlKind) -> &'static str {
    match k {
        ControlKind::Button => "control.button",
        ControlKind::CheckBox => "control.checkbox",
        ControlKind::Radio => "control.radio",
        ControlKind::ComboBox => "control.combo",
        ControlKind::ListBox => "control.list",
        ControlKind::Spin => "control.spin",
        ControlKind::ScrollBar => "control.scroll",
        ControlKind::GroupBox => "control.group",
        ControlKind::Label => "control.label",
    }
}

fn icon(k: ControlKind) -> Icon {
    match k {
        ControlKind::Button => Icon::Control,
        ControlKind::CheckBox | ControlKind::Radio => Icon::Checkbox,
        ControlKind::ComboBox => Icon::Validation,
        ControlKind::ListBox => Icon::Bullets,
        ControlKind::Spin => Icon::ChevronUp,
        ControlKind::ScrollBar => Icon::ChevronDown,
        ControlKind::GroupBox => Icon::Borders,
        ControlKind::Label => Icon::TextBox,
    }
}

/// A small triangle pointing up, down, left or right.
fn arrow(p: &egui::Painter, c: Pos2, r: f32, dir: (f32, f32)) {
    let (dx, dy) = dir;
    let tip = pos2(c.x + dx * r, c.y + dy * r);
    let a = pos2(c.x - dx * r * 0.6 - dy * r, c.y - dy * r * 0.6 - dx * r);
    let b = pos2(c.x - dx * r * 0.6 + dy * r, c.y - dy * r * 0.6 + dx * r);
    p.add(egui::Shape::convex_polygon(vec![tip, a, b], INK, Stroke::NONE));
}

/// Box of the arrow buttons and thumb of a scroll bar: less, more, thumb.
fn scroll_parts(r: Rect, c: &FormControl, value: i32) -> (Rect, Rect, Rect) {
    let span = (c.max - c.min).max(1) as f32;
    let t = ((value - c.min) as f32 / span).clamp(0.0, 1.0);
    if c.horizontal {
        let s = r.height().min(r.width() / 3.0);
        let less = Rect::from_min_size(r.min, vec2(s, r.height()));
        let more = Rect::from_min_size(pos2(r.max.x - s, r.min.y), vec2(s, r.height()));
        let x = r.min.x + s + t * (r.width() - 3.0 * s);
        (less, more, Rect::from_min_size(pos2(x, r.min.y), vec2(s, r.height())))
    } else {
        let s = r.width().min(r.height() / 3.0);
        let less = Rect::from_min_size(r.min, vec2(r.width(), s));
        let more = Rect::from_min_size(pos2(r.min.x, r.max.y - s), vec2(r.width(), s));
        let y = r.min.y + s + t * (r.height() - 3.0 * s);
        (less, more, Rect::from_min_size(pos2(r.min.x, y), vec2(r.width(), s)))
    }
}

impl SheetView {
    /// Paints control shape `s` in screen box `r` at zoom `z`.
    pub(super) fn paint_control(&self, p: &egui::Painter, r: Rect, s: &SheetShape, c: &FormControl, z: f32) {
        let p = p.with_clip_rect(p.clip_rect().intersect(r.expand(2.0)));
        let font = FontId::proportional(11.0 * z);
        let stroke = Stroke::new(1.0, EDGE);
        let value = self.wb.control_value(self.sheet, c);
        let mark = (13.0 * z).min(r.height() - 2.0).max(4.0);
        let text_at = |p: &egui::Painter, x: f32| {
            p.text(pos2(x, r.center().y), Align2::LEFT_CENTER, &c.text, font.clone(), INK);
        };
        match c.kind {
            ControlKind::Button => {
                p.rect_filled(r, 3.0 * z, FACE);
                p.rect_stroke(r, 3.0 * z, stroke, StrokeKind::Inside);
                p.text(r.center(), Align2::CENTER_CENTER, &c.text, font, INK);
            }
            ControlKind::CheckBox => {
                let b = Rect::from_min_size(pos2(r.min.x + 2.0 * z, r.center().y - mark / 2.0), vec2(mark, mark));
                p.rect_filled(b, 0.0, Color32::WHITE);
                p.rect_stroke(b, 0.0, stroke, StrokeKind::Inside);
                match value {
                    1 => {
                        let w = Stroke::new(1.8 * z, INK);
                        let pts = [pos2(b.min.x + mark * 0.2, b.center().y), pos2(b.min.x + mark * 0.42, b.max.y - mark * 0.22), pos2(b.max.x - mark * 0.18, b.min.y + mark * 0.2)];
                        p.line_segment([pts[0], pts[1]], w);
                        p.line_segment([pts[1], pts[2]], w);
                    }
                    0 => {}
                    _ => {
                        p.rect_filled(b.shrink(mark * 0.25), 0.0, EDGE);
                    }
                }
                text_at(&p, b.max.x + 4.0 * z);
            }
            ControlKind::Radio => {
                let cpos = pos2(r.min.x + 2.0 * z + mark / 2.0, r.center().y);
                p.circle_filled(cpos, mark / 2.0, Color32::WHITE);
                p.circle_stroke(cpos, mark / 2.0, stroke);
                if self.wb.radio_on(self.sheet, s.id) {
                    p.circle_filled(cpos, mark / 4.5, INK);
                }
                text_at(&p, cpos.x + mark / 2.0 + 4.0 * z);
            }
            ControlKind::ComboBox => {
                let items = self.wb.control_items(self.sheet, c);
                p.rect_filled(r, 0.0, Color32::WHITE);
                p.rect_stroke(r, 0.0, stroke, StrokeKind::Inside);
                let bw = r.height().min(r.width() / 2.0);
                let b = Rect::from_min_max(pos2(r.max.x - bw, r.min.y), r.max);
                p.rect_filled(b.shrink(1.0), 0.0, FACE);
                arrow(&p, b.center(), bw * 0.18, (0.0, 1.0));
                if let Some(t) = (value >= 1).then(|| items.get(value as usize - 1)).flatten() {
                    p.with_clip_rect(p.clip_rect().intersect(Rect::from_min_max(r.min, pos2(b.min.x, r.max.y)))).text(pos2(r.min.x + 3.0 * z, r.center().y), Align2::LEFT_CENTER, t, font, INK);
                }
            }
            ControlKind::ListBox => {
                let items = self.wb.control_items(self.sheet, c);
                p.rect_filled(r, 0.0, Color32::WHITE);
                p.rect_stroke(r, 0.0, stroke, StrokeKind::Inside);
                let h = ROW * z;
                for (i, t) in items.iter().enumerate() {
                    let row = Rect::from_min_size(pos2(r.min.x + 1.0, r.min.y + 1.0 + i as f32 * h), vec2(r.width() - 2.0, h));
                    if row.min.y > r.max.y {
                        break;
                    }
                    let on = value == i as i32 + 1;
                    if on {
                        p.rect_filled(row, 0.0, PICK);
                    }
                    p.text(pos2(row.min.x + 3.0 * z, row.center().y), Align2::LEFT_CENTER, t, font.clone(), if on { Color32::WHITE } else { INK });
                }
            }
            ControlKind::Spin => {
                let (a, b) = if c.horizontal { r.split_left_right_at_fraction(0.5) } else { r.split_top_bottom_at_fraction(0.5) };
                for (half, dir) in [(a, if c.horizontal { (-1.0, 0.0) } else { (0.0, -1.0) }), (b, if c.horizontal { (1.0, 0.0) } else { (0.0, 1.0) })] {
                    p.rect_filled(half, 0.0, FACE);
                    p.rect_stroke(half, 0.0, stroke, StrokeKind::Inside);
                    arrow(&p, half.center(), half.width().min(half.height()) * 0.22, dir);
                }
            }
            ControlKind::ScrollBar => {
                p.rect_filled(r, 0.0, Color32::from_gray(0xE6));
                let (less, more, thumb) = scroll_parts(r, c, value);
                for (b, dir) in [(less, if c.horizontal { (-1.0, 0.0) } else { (0.0, -1.0) }), (more, if c.horizontal { (1.0, 0.0) } else { (0.0, 1.0) })] {
                    p.rect_filled(b, 0.0, FACE);
                    p.rect_stroke(b, 0.0, stroke, StrokeKind::Inside);
                    arrow(&p, b.center(), b.width().min(b.height()) * 0.2, dir);
                }
                p.rect_filled(thumb.shrink(1.0), 2.0 * z, Color32::from_gray(0xB8));
                p.rect_stroke(r, 0.0, stroke, StrokeKind::Inside);
            }
            ControlKind::GroupBox => {
                let top = r.min.y + 6.0 * z;
                let frame = Rect::from_min_max(pos2(r.min.x, top), r.max);
                p.rect_stroke(frame, 2.0 * z, stroke, StrokeKind::Inside);
                if !c.text.is_empty() {
                    let g = p.layout_no_wrap(c.text.clone(), font, INK);
                    let at = pos2(r.min.x + 8.0 * z, top - g.size().y / 2.0);
                    // Paper behind the caption hides the line.
                    p.rect_filled(Rect::from_min_size(at - vec2(2.0, 0.0), g.size() + vec2(4.0, 0.0)), 0.0, Color32::WHITE);
                    p.galley(at, g, INK);
                }
            }
            ControlKind::Label => text_at(&p, r.min.x + 2.0 * z),
        }
    }

    /// Whether a click at screen point `p` finds control `s` (a group
    /// box only by its edge, so the cells inside stay reachable).
    pub(super) fn control_hit(s: &SheetShape, r: Rect, p: Pos2) -> bool {
        match s.control.as_deref() {
            Some(c) if c.kind == ControlKind::GroupBox => r.contains(p) && !r.shrink(6.0).contains(p),
            _ => true,
        }
    }

    /// A plain click on control `id` (screen box `r`) at `p`: operates it.
    pub(super) fn click_control(&mut self, id: u32, r: Rect, p: Pos2, z: f32) {
        let sheet = self.sheet;
        let Some(c) = self.sh().shapes.iter().find(|s| s.id == id).and_then(|s| s.control.as_deref()).cloned() else { return };
        self.settle_edit();
        let value = self.wb.control_value(sheet, &c);
        let step = |v: i32, d: i32| (v + d).clamp(c.min, c.max);
        let new = match c.kind {
            ControlKind::Button => {
                if !c.macro_name.is_empty() {
                    let name = c.macro_name.rsplit('!').next().unwrap_or("");
                    self.run_macro = Some(name.rsplit('.').next().unwrap_or(name).to_string());
                }
                return;
            }
            ControlKind::CheckBox => (value != 1) as i32,
            ControlKind::Radio => 1,
            ControlKind::ComboBox => {
                self.images.shapes.controls.popup = Some((sheet, id, r));
                return;
            }
            ControlKind::ListBox => {
                let i = ((p.y - r.min.y - 1.0) / (ROW * z)).floor() as i32 + 1;
                if i < 1 || i as usize > self.wb.control_items(sheet, &c).len() {
                    return;
                }
                i
            }
            ControlKind::Spin => {
                let more = if c.horizontal { p.x > r.center().x } else { p.y < r.center().y };
                step(value, if more { c.inc } else { -c.inc })
            }
            ControlKind::ScrollBar => {
                let (less, more, thumb) = scroll_parts(r, &c, value);
                let before = if c.horizontal { p.x < thumb.min.x } else { p.y < thumb.min.y };
                if less.contains(p) {
                    step(value, -c.inc)
                } else if more.contains(p) {
                    step(value, c.inc)
                } else if thumb.contains(p) {
                    return;
                } else {
                    step(value, if before { -c.page } else { c.page })
                }
            }
            ControlKind::GroupBox | ControlKind::Label => return,
        };
        self.wb.operate_control(sheet, id, new);
    }

    /// The open drop-down of a combo box.
    pub(super) fn control_popup(&mut self, ctx: &egui::Context) {
        let Some((sheet, id, r)) = self.images.shapes.controls.popup else { return };
        let found = (sheet == self.sheet).then(|| self.sh().shapes.iter().find(|s| s.id == id).and_then(|s| s.control.as_deref()).cloned()).flatten();
        let Some(c) = found else {
            self.images.shapes.controls.popup = None;
            return;
        };
        let items = self.wb.control_items(sheet, &c);
        let value = self.wb.control_value(sheet, &c);
        let mut pick = None;
        let area = egui::Area::new(egui::Id::new(("control-popup", id))).order(egui::Order::Foreground).fixed_pos(r.left_bottom()).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(r.width());
                egui::ScrollArea::vertical().max_height(c.lines as f32 * 20.0).show(ui, |ui| {
                    for (i, t) in items.iter().enumerate() {
                        if ui.selectable_label(value == i as i32 + 1, t).clicked() {
                            pick = Some(i as i32 + 1);
                        }
                    }
                });
            });
        });
        let outside = ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !area.response.rect.contains(p) && !r.contains(p)));
        if let Some(v) = pick {
            self.wb.operate_control(sheet, id, v);
        }
        if pick.is_some() || outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.images.shapes.controls.popup = None;
        }
    }

    // --- inserting and formatting ------------------------------------------------------

    /// The Form Controls menu of the toolstrip.
    pub(super) fn form_controls_menu(&mut self, ui: &mut egui::Ui) {
        for k in ControlKind::ALL {
            if crate::toolstrip::menu_item(ui, icon(k), &tr(label_key(k))).clicked() {
                self.insert_control(k);
                ui.close();
            }
        }
    }

    /// Puts a control of kind `k` at the active cell and opens its
    /// settings.
    pub(super) fn insert_control(&mut self, k: ControlKind) {
        self.settle_edit();
        let (w, h) = k.size();
        let mut shape = Shape::new(ShapeKind::Rect);
        shape.fill = Some(0xFFFFFF);
        let n = self.sh().shapes.iter().filter(|s| s.control.as_ref().is_some_and(|c| c.kind == k)).count() + 1;
        let name = tr(label_key(k));
        shape.name = format!("{name} {n}");
        let mut s = SheetShape::new(shape, self.cursor, w, h);
        let mut c = FormControl::new(k);
        if k.has_text() {
            c.text = format!("{name} {n}");
        }
        if k == ControlKind::Radio {
            // Next to the last option button, in its group.
            if let Some(last) = self.sh().shapes.iter().rev().find(|s| s.control.as_ref().is_some_and(|c| c.kind == ControlKind::Radio)) {
                c.link = last.control.as_ref().map(|c| c.link.clone()).unwrap_or_default();
            }
        }
        s.control = Some(Box::new(c));
        s.anchor = apex_sheet::Anchor::OneCell;
        let id = self.wb.add_shape(self.sheet, s);
        if self.sh().shapes.iter().any(|s| s.id == id) {
            self.select_shape(id);
            self.open_control_dialog();
        }
    }

    pub(super) fn open_control_dialog(&mut self) {
        let Some(s) = self.selected_shape().cloned() else { return };
        let Some(mut c) = s.control.as_deref().cloned() else { return };
        if c.kind != ControlKind::Radio {
            c.value = self.wb.control_value(self.sheet, &c);
        }
        self.images.shapes.controls.dialog = Some(Dlg { sheet: self.sheet, orig: s, c });
    }

    pub(super) fn control_dialog(&mut self, ctx: &egui::Context) {
        self.control_popup(ctx);
        let Some(d) = &mut self.images.shapes.controls.dialog else { return };
        let title = format!("{} — {}", tr("control.format"), tr(label_key(d.c.kind)));
        let res = widgets::dialog(ctx, "control-format", &title, 420.0, |ui| dialog_ui(ui, &mut d.c));
        match res {
            DialogResult::None => {}
            DialogResult::Ok => {
                let d = self.images.shapes.controls.dialog.take().unwrap();
                let mut s = d.orig;
                let shown = self.wb.control_value(d.sheet, s.control.as_deref().unwrap_or(&d.c));
                let (id, value, set) = (s.id, d.c.value, d.c.kind.has_link() && d.c.kind != ControlKind::Radio);
                s.control = Some(Box::new(d.c));
                if d.sheet < self.wb.sheets.len() {
                    let n = self.wb.undo_len();
                    self.wb.set_shape(d.sheet, s);
                    // A value typed here goes to the linked cell too.
                    if set && value != shown {
                        self.wb.operate_control(d.sheet, id, value);
                    }
                    self.wb.merge_undo(self.wb.undo_len().saturating_sub(n), "undo.shape");
                }
            }
            DialogResult::Cancel => self.images.shapes.controls.dialog = None,
        }
    }
}

fn dialog_ui(ui: &mut egui::Ui, c: &mut FormControl) {
    let k = c.kind;
    widgets::form(ui, "control-form", |ui| {
        if k.has_text() {
            widgets::form_row(ui, &tr("control.text"), |ui| ui.add(egui::TextEdit::singleline(&mut c.text).desired_width(240.0)));
        }
        if k.has_items() {
            widgets::form_row(ui, &tr("control.items"), |ui| ui.add(egui::TextEdit::singleline(&mut c.items).hint_text("$A$1:$A$10").desired_width(160.0)));
        }
        if k.has_link() {
            widgets::form_row(ui, &tr("control.link"), |ui| ui.add(egui::TextEdit::singleline(&mut c.link).hint_text("$B$1").desired_width(160.0)));
        }
        match k {
            ControlKind::CheckBox => {
                widgets::form_row(ui, &tr("control.value"), |ui| {
                    ui.radio_value(&mut c.value, 0, tr("control.unchecked"));
                    ui.radio_value(&mut c.value, 1, tr("control.checked"));
                    ui.radio_value(&mut c.value, 2, tr("control.mixed"));
                });
            }
            ControlKind::ComboBox => {
                widgets::form_row(ui, &tr("control.lines"), |ui| ui.add(egui::DragValue::new(&mut c.lines).range(1..=100)));
            }
            ControlKind::Spin | ControlKind::ScrollBar => {
                for (key, v) in [("control.value", &mut c.value), ("control.min", &mut c.min), ("control.max", &mut c.max), ("control.inc", &mut c.inc)] {
                    widgets::form_row(ui, &tr(key), |ui| ui.add(egui::DragValue::new(v).range(0..=30000)));
                }
                if k == ControlKind::ScrollBar {
                    widgets::form_row(ui, &tr("control.page"), |ui| ui.add(egui::DragValue::new(&mut c.page).range(1..=30000)));
                }
                widgets::form_row(ui, "", |ui| ui.checkbox(&mut c.horizontal, tr("control.horizontal")));
            }
            ControlKind::Button => {
                widgets::form_row(ui, &tr("control.macro"), |ui| ui.add(egui::TextEdit::singleline(&mut c.macro_name).desired_width(200.0)));
            }
            _ => {}
        }
    });
    ui.add_space(6.0);
    ui.weak(tr("control.tip"));
    c.sanitize();
}
