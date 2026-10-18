//! The Mini Toolbar: common formatting next to a selection made with the
//! mouse, and over the context menu. It fades as the pointer moves away
//! and goes when the selection changes.

use super::*;
use crate::widgets::{self, Face};

pub(super) struct MiniBar {
    /// Where it was asked for (the pointer), on screen.
    at: Pos2,
    sel: Selection,
    /// Over the context menu: it doesn't fade.
    menu: bool,
    /// The pointer has been over it.
    touched: bool,
    /// Where the context menu is (it moves to fit on the screen).
    menu_rect: Option<Rect>,
}

/// Height of the bar, to place it above the pointer.
const HEIGHT: f32 = 64.0;

impl DocView {
    /// Shows the bar for the current selection near `at`.
    pub(super) fn show_minibar(&mut self, at: Pos2, menu: bool) {
        if !crate::settings::mini_toolbar() || self.brush.is_some() || (!menu && self.sel.is_empty()) {
            return;
        }
        self.minibar = Some(MiniBar { at, sel: self.sel, menu, touched: false, menu_rect: None });
    }

    /// Called from inside the context menu with its rectangle.
    pub(super) fn minibar_menu_rect(&mut self, r: Rect) {
        if let Some(b) = self.minibar.as_mut().filter(|b| b.menu) {
            b.menu_rect = Some(r);
        }
    }

    pub(super) fn minibar_ui(&mut self, ui: &mut egui::Ui) {
        let Some(bar) = &self.minibar else { return };
        if bar.sel != self.sel || self.brush.is_some() || self.pics.dialog_open() {
            self.minibar = None;
            return;
        }
        let ctx = ui.ctx().clone();
        // Once the context menu has gone, it fades like the other.
        if bar.menu && !egui::Popup::is_any_open(&ctx) {
            self.minibar = Some(MiniBar { menu: false, touched: false, menu_rect: None, ..*bar });
        }
        let Some(bar) = &self.minibar else { return };
        let id = Id::new(("minibar", self.uid));
        let last = ctx.memory(|m| m.area_rect(id));
        let size = last.map_or(vec2(360.0, HEIGHT), |r| r.size());
        let pos = match (bar.menu, bar.menu_rect) {
            // Wait for the menu to be placed.
            (true, None) => {
                ctx.request_repaint();
                return;
            }
            // Above the context menu, else below it, else beside it: never over it.
            (true, Some(m)) => {
                let screen = ctx.content_rect();
                let x = m.min.x.min(screen.max.x - size.x).max(screen.min.x);
                if m.min.y - size.y - 6.0 >= screen.min.y {
                    pos2(x, m.min.y - size.y - 6.0)
                } else if m.max.y + size.y + 6.0 <= screen.max.y {
                    pos2(x, m.max.y + 6.0)
                } else if m.max.x + size.x + 6.0 <= screen.max.x {
                    pos2(m.max.x + 6.0, m.min.y)
                } else {
                    pos2(m.min.x - size.x - 6.0, m.min.y)
                }
            }
            // Above the pointer.
            (false, _) => pos2(bar.at.x - 20.0, bar.at.y - HEIGHT - 18.0),
        };
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        let dist = match (last, pointer) {
            (Some(r), Some(p)) => r.distance_to_pos(p),
            _ => 0.0,
        };
        let touched = bar.touched || dist == 0.0;
        // Word's fade: clear near the bar, faint further away, gone far off.
        let far = if touched { 120.0 } else { 220.0 };
        if !bar.menu && dist > far {
            self.minibar = None;
            return;
        }
        let opacity = if bar.menu || touched && dist < 20.0 { 1.0 } else { (1.0 - (dist - 20.0) / (far - 20.0)).clamp(0.25, 1.0) };
        if let Some(b) = &mut self.minibar {
            b.touched = touched;
        }
        let a = self.attrs_at_caret();
        let st = self.shape_para().unwrap_or(self.cur_para().style);
        let mut used = false;
        egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(pos).constrain(true).show(&ctx, |ui| {
            ui.set_opacity(opacity);
            egui::Frame::popup(ui.style()).inner_margin(4.0).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
                ui.horizontal(|ui| used |= self.minibar_font_row(ui, &a, &st));
                ui.horizontal(|ui| used |= self.minibar_style_row(ui, &a, &st));
            });
        });
        if used {
            self.focus = true;
        }
    }

    fn minibar_font_row(&mut self, ui: &mut egui::Ui, a: &Attrs, st: &ParaStyle) -> bool {
        let mut used = false;
        let default = tr("font.default");
        let cf = self.doc.char_format(st, a);
        let cur = cf.font.clone().or(self.doc.default_font_name()).unwrap_or_else(|| Arc::from(default));
        if let Some(f) = common::themed_font_picker(ui, Id::new(("mfont", self.uid)), &cur, 110.0) {
            let f: Option<Arc<str>> = if f.eq_ignore_ascii_case(default) { None } else { Some(Arc::from(f.as_str())) };
            self.format(move |x| x.font = f.clone());
            used = true;
        }
        if let Some(v) = common::size_box(ui, Id::new(("msize", self.uid)), cf.size as f32 / 2.0) {
            let hp = (v * 2.0).round() as u16;
            self.format(move |x| x.size = Some(hp));
            used = true;
        }
        if widgets::tool_button(ui, Face::Icon(Icon::GrowFont), false, &format!("{} (Ctrl+Shift+>)", tr("format.grow"))).clicked() {
            self.grow_font(true);
            used = true;
        }
        if widgets::tool_button(ui, Face::Icon(Icon::ShrinkFont), false, &format!("{} (Ctrl+Shift+<)", tr("format.shrink"))).clicked() {
            self.grow_font(false);
            used = true;
        }
        if widgets::tool_button(ui, Face::Icon(Icon::Brush), false, tr("toolbar.brush_hint")).clicked() {
            self.start_brush(false);
            used = true;
        }
        let mut kind = None;
        let r = widgets::tool_button(ui, Face::Icon(Icon::Styles), false, tr("toolstrip.g.styles"));
        egui::Popup::menu(&r).show(|ui| {
            for k in KINDS {
                if ui.add(egui::Button::selectable(st.kind == k && st.custom == 0, kind_label(k))).clicked() {
                    kind = Some(k);
                    ui.close();
                }
            }
        });
        if let Some(k) = kind {
            self.set_kind(k);
            used = true;
        }
        used
    }

    fn minibar_style_row(&mut self, ui: &mut egui::Ui, a: &Attrs, st: &ParaStyle) -> bool {
        let mut used = false;
        if widgets::tool_button(ui, Face::Text(RichText::new("B").strong()), a.bold, &format!("{} (Ctrl+B)", tr("format.bold"))).clicked() {
            self.toggle(|a| a.bold, |a, v| a.bold = v);
            used = true;
        }
        if widgets::tool_button(ui, Face::Text(RichText::new("I").italics()), a.italic, &format!("{} (Ctrl+I)", tr("format.italic"))).clicked() {
            self.toggle(|a| a.italic, |a, v| a.italic = v);
            used = true;
        }
        if widgets::tool_button(ui, Face::Text(RichText::new("U").underline()), a.underline, &format!("{} (Ctrl+U)", tr("format.underline"))).clicked() {
            self.toggle(|a| a.underline, |a, v| a.underline = v);
            used = true;
        }
        let last = self.last_highlight;
        let mut chosen = None;
        if widgets::split_button(ui, Face::IconBar(Icon::Highlight, last.map_or(Color32::TRANSPARENT, theme::rgb)), false, tr("format.highlight"), |ui| {
            chosen = widgets::color_menu(ui, tr("format.no_highlight"), a.highlight);
        }) {
            chosen = Some(last);
        }
        if let Some(c) = chosen {
            self.last_highlight = c;
            self.format(move |x| x.highlight = c);
            used = true;
        }
        let last = self.last_color;
        let mut chosen = None;
        if widgets::split_button(ui, Face::TextBar(RichText::new("A").strong(), last.map_or(Paper::TEXT, theme::rgb)), false, tr("format.text_color"), |ui| {
            chosen = widgets::color_menu(ui, tr("format.auto_color"), a.color);
        }) {
            chosen = Some(last);
        }
        if let Some(c) = chosen {
            self.last_color = c;
            self.format(move |x| x.color = c);
            used = true;
        }
        let mut list = None;
        if widgets::split_button(ui, Face::Icon(Icon::Bullets), st.kind == ParaKind::Bullet, &format!("{} (Ctrl+Shift+L)", tr("doc.bullets")), |ui| list = self.bullet_menu(ui)) {
            self.set_kind(ParaKind::Bullet);
            used = true;
        }
        if widgets::split_button(ui, Face::Icon(Icon::Numbering), st.kind == ParaKind::Numbered, tr("doc.numbering"), |ui| list = self.numbering_menu(ui)) {
            self.set_kind(ParaKind::Numbered);
            used = true;
        }
        if let Some((kind, f)) = list {
            self.set_list(kind, f);
            used = true;
        }
        used
    }
}
