//! The form's fields over the pages: typing in text fields (Tab goes to
//! the next one), ticking check boxes and radio buttons, choosing from
//! lists, and signing empty signature fields. Values go into the file
//! with `set_field` (one undo step each; calculated fields follow).

use std::sync::Arc;

use apex_io::pdf::edit::forms::{Field, FieldKind};
use eframe::egui::{self, Color32, CursorIcon, FontId, Key, Rect, Sense, Stroke};

use super::pages::PageMap;
use super::{PdfView, Request, engine};
use crate::i18n::tr;

/// A text field being typed in.
pub struct FieldEdit {
    pub name: String,
    /// Which of its widgets.
    pub widget: usize,
    pub text: String,
    pub focus: bool,
}

fn field_fill() -> Color32 {
    Color32::from_rgba_unmultiplied(0xD6, 0xE4, 0xFF, 150)
}

fn is_text(f: &Field) -> bool {
    matches!(f.kind, FieldKind::Text { .. }) && !f.read_only
}

impl PdfView {
    /// The file's fields, read again when it changes.
    pub fn form_fields(&mut self) -> Arc<Vec<Field>> {
        if self.fields.0 != self.doc.revision {
            self.fields = (self.doc.revision, Arc::new(self.doc.fields()));
        }
        self.fields.1.clone()
    }

    /// Starts typing in text field `name` (its first widget).
    pub fn edit_field(&mut self, name: &str) {
        let fields = self.form_fields();
        if let Some(f) = fields.iter().find(|f| f.name == name && is_text(f)) {
            self.field_edit = Some(FieldEdit { name: f.name.clone(), widget: 0, text: f.value.clone(), focus: true });
            if let Some(w) = f.widgets.first() {
                self.go_to(w.page, (w.rect[1] - 60.0).max(0.0));
            }
        }
    }

    /// Writes what was typed; `next`: then the following text field.
    fn commit_field(&mut self, next: bool) {
        let Some(fe) = self.field_edit.take() else { return };
        let fields = self.form_fields();
        let at = fields.iter().position(|f| f.name == fe.name);
        if at.is_some_and(|k| fields[k].value != fe.text) {
            self.doc.set_field(&fe.name, &fe.text);
            self.after_change();
        }
        if next && let Some(k) = at {
            let n = fields.len();
            if let Some(f) = (1..n).map(|d| &fields[(k + d) % n]).find(|f| is_text(f)) {
                self.edit_field(&f.name.clone());
            }
        }
    }

    pub(super) fn fields_ui(&mut self, ui: &mut egui::Ui, i: usize, map: &PageMap) {
        let fields = self.form_fields();
        if fields.is_empty() || self.read_only {
            return;
        }
        let simple = !engine::available();
        for f in fields.iter() {
            for (k, w) in f.widgets.iter().enumerate().filter(|(_, w)| w.page == i) {
                let r = map.rect_to_screen(w.rect);
                if r.width() < 2.0 || r.height() < 2.0 {
                    continue;
                }
                let id = ui.id().with(("pdf_field", f.name.as_str(), k));
                let size = if f.font_size > 0.0 { f.font_size } else { ((w.rect[3] - w.rect[1]) * 0.62).clamp(6.0, 12.0) } * map.z;
                // A size from the file, or a NaN, would stall or panic the layout.
                let size = size.max(1.0).min(400.0);
                match &f.kind {
                    FieldKind::Text { multiline, password, max_len, .. } => {
                        let editing = self.field_edit.as_ref().is_some_and(|e| e.name == f.name && e.widget == k);
                        if editing {
                            self.typing_ui(ui, r, size, *multiline, *password, *max_len);
                        } else {
                            let resp = ui.interact(r, id, Sense::click());
                            if simple && !f.value.is_empty() && !*password {
                                ui.painter().with_clip_rect(r).text(r.left_center() + egui::vec2(2.0, 0.0), egui::Align2::LEFT_CENTER, &f.value, FontId::proportional(size), Color32::BLACK);
                            }
                            if f.read_only {
                                continue;
                            }
                            if resp.hovered() {
                                ui.painter().rect_filled(r, 0.0, field_fill());
                                ui.ctx().set_cursor_icon(CursorIcon::Text);
                            }
                            if resp.clicked() {
                                self.commit_field(false);
                                self.field_edit = Some(FieldEdit { name: f.name.clone(), widget: k, text: f.value.clone(), focus: true });
                            }
                        }
                    }
                    FieldKind::Check | FieldKind::Radio => {
                        let resp = ui.interact(r, id, Sense::click());
                        let on = !w.on.is_empty() && w.on != "Off" && f.value == w.on;
                        if simple {
                            let p = ui.painter();
                            p.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::GRAY), egui::StrokeKind::Inside);
                            if on {
                                p.circle_filled(r.center(), r.width().min(r.height()) * 0.25, Color32::BLACK);
                            }
                        }
                        if f.read_only {
                            continue;
                        }
                        if resp.hovered() {
                            ui.painter().rect_filled(r, 0.0, field_fill());
                            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                        }
                        if resp.clicked() && !(on && f.kind == FieldKind::Radio) {
                            let v = if on { "Off".to_string() } else { w.on.clone() };
                            self.doc.set_field(&f.name, &v);
                            self.after_change();
                        }
                    }
                    FieldKind::Combo { options, .. } | FieldKind::List { options, .. } => {
                        if f.read_only {
                            continue;
                        }
                        let shown = options.iter().find(|o| o.0 == f.value).map_or(f.value.as_str(), |o| o.1.as_str());
                        let mut chosen = None;
                        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
                        egui::ComboBox::from_id_salt(id).width(r.width() - 6.0).selected_text(shown).show_ui(&mut child, |ui| {
                            for (v, t) in options {
                                if ui.selectable_label(*v == f.value, t).clicked() {
                                    chosen = Some(v.clone());
                                }
                            }
                        });
                        if let Some(v) = chosen {
                            self.doc.set_field(&f.name, &v);
                            self.after_change();
                        }
                    }
                    FieldKind::Signature if f.value.is_empty() => {
                        let resp = ui.interact(r, id, Sense::click()).on_hover_text(tr("pdf.sign_field_tip"));
                        if resp.hovered() {
                            ui.painter().rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(0xFF, 0xE0, 0x80, 90));
                            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                        }
                        if simple {
                            ui.painter().rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_rgb(0xC0, 0x90, 0x20)), egui::StrokeKind::Inside);
                        }
                        if resp.clicked() {
                            self.requests.push(Request::Sign(Some(f.name.clone())));
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    /// The text field being typed in, over its box.
    fn typing_ui(&mut self, ui: &mut egui::Ui, r: Rect, size: f32, multiline: bool, password: bool, max_len: Option<usize>) {
        let Some(fe) = self.field_edit.as_mut() else { return };
        ui.painter().rect_filled(r, 0.0, Color32::WHITE);
        ui.painter().rect_stroke(r, 0.0, Stroke::new(1.5, crate::theme::ACCENT), egui::StrokeKind::Outside);
        let te = if multiline { egui::TextEdit::multiline(&mut fe.text) } else { egui::TextEdit::singleline(&mut fe.text) };
        let mut te = te.font(FontId::proportional(size)).frame(egui::Frame::NONE).text_color(Color32::BLACK).desired_width(r.width() - 4.0).password(password);
        if let Some(n) = max_len.filter(|n| *n > 0) {
            te = te.char_limit(n);
        }
        let resp = ui.put(r.shrink(2.0), te);
        if std::mem::take(&mut fe.focus) {
            resp.request_focus();
        }
        if resp.lost_focus() {
            let (esc, tab) = ui.input(|i| (i.key_pressed(Key::Escape), i.key_pressed(Key::Tab)));
            if esc {
                self.field_edit = None;
            } else {
                self.commit_field(tab);
            }
        }
    }
}
