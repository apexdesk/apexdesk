//! The form designer, with no code: controls are dragged to move them and
//! by their corner to resize them; the side panel sets the form (kind,
//! source, subform and the fields linking it) and the selected control
//! (text, field, the table a dropdown's values come from, a button's fixed
//! action). Changes are kept in the designer until saved; undo inside it
//! takes back moves and edits.

use apex_db::forms::{Action, Control, CtlKind, FormDef, FormKind};
use apex_db::objects::Kind;
use apex_db::sqlite::{Db, RowSource};
use eframe::egui::{self, Rect, RichText, Sense, Stroke, vec2};

use super::report_ui::{field_combo, source_combo};
use super::DbView;
use crate::i18n::{tr, trf};

/// Snapping grid in points.
const GRID: f32 = 4.0;

pub struct FormDesigner {
    pub name: String,
    pub def: FormDef,
    pub sel: Option<usize>,
    /// Earlier versions of the definition, and those undone.
    history: Vec<FormDef>,
    future: Vec<FormDef>,
    /// The definition when a drag started.
    drag_from: Option<FormDef>,
    /// The database's macros, for buttons that run one (read when first
    /// needed).
    macros: Option<Vec<String>>,
}

impl FormDesigner {
    pub fn new(name: &str, def: FormDef) -> FormDesigner {
        FormDesigner { name: name.to_string(), def, sel: None, history: Vec::new(), future: Vec::new(), drag_from: None, macros: None }
    }

    fn remember(&mut self, before: FormDef) {
        if before != self.def {
            self.history.push(before);
            self.future.clear();
            if self.history.len() > 200 {
                self.history.remove(0);
            }
        }
    }

    pub fn undo(&mut self) {
        if let Some(d) = self.history.pop() {
            self.future.push(std::mem::replace(&mut self.def, d));
            self.sel = None;
        }
    }

    pub fn redo(&mut self) {
        if let Some(d) = self.future.pop() {
            self.history.push(std::mem::replace(&mut self.def, d));
            self.sel = None;
        }
    }

    /// Adds a control of `kind` below the others.
    pub fn add(&mut self, kind: CtlKind) {
        let before = self.def.clone();
        let y = self.def.controls.iter().map(|c| c.y + c.h).fold(8.0, f32::max) + 6.0;
        let (w, h) = match kind {
            CtlKind::CheckBox => (22.0, 22.0),
            CtlKind::Button => (90.0, 26.0),
            _ => (160.0, 22.0),
        };
        let text = match kind {
            CtlKind::Label => tr("db.ctl.label").to_string(),
            CtlKind::Button => tr("db.ctl.button").to_string(),
            _ => String::new(),
        };
        self.def.controls.push(Control { kind, x: 12.0, y, w, h, text, ..Default::default() });
        self.def.height = self.def.height.max(y + h + 12.0);
        self.sel = Some(self.def.controls.len() - 1);
        self.remember(before);
    }

    pub fn remove_selected(&mut self) {
        let Some(i) = self.sel.take() else { return };
        let before = self.def.clone();
        self.def.controls.remove(i);
        self.remember(before);
    }

    pub fn save(&mut self, v: &mut DbView) -> bool {
        let ok = v.save_object(Kind::Form, &self.name, &self.def.to_records());
        if ok {
            v.notices.push(crate::app::Notice::info(trf("db.saved", &[("name", &self.name)])));
        }
        ok
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, v: &mut DbView) {
        ui.horizontal(|ui| {
            if ui.button(tr("db.save")).clicked() {
                self.save(v);
            }
            if ui.button(tr("db.form_view")).clicked() && self.save(v) {
                v.open_form(&self.name);
            }
            ui.separator();
            if ui.add_enabled(!self.history.is_empty(), egui::Button::new("↶")).on_hover_text(tr("db.undo_design")).clicked() {
                self.undo();
            }
            if ui.add_enabled(!self.future.is_empty(), egui::Button::new("↷")).on_hover_text(tr("db.redo_design")).clicked() {
                self.redo();
            }
            ui.separator();
            for k in CtlKind::ALL {
                if ui.button(format!("+ {}", super::trk("db.ctl", k.key()))).clicked() {
                    self.add(k);
                }
            }
            if ui.add_enabled(self.sel.is_some(), egui::Button::new(tr("db.remove_control"))).clicked() {
                self.remove_selected();
            }
        });
        ui.separator();
        let before = self.def.clone();
        egui::Panel::right(egui::Id::new(("db_form_props", &self.name))).resizable(true).default_size(260.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.props(ui, v));
        });
        if self.drag_from.is_none() {
            self.remember(before);
        }
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| self.canvas(ui));
        if !ui.ctx().egui_wants_keyboard_input() {
            let (del, z, y) = ui.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, egui::Key::Delete), i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z), i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y)));
            if del {
                self.remove_selected();
            }
            if z {
                self.undo();
            }
            if y {
                self.redo();
            }
        }
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let size = vec2(self.def.width.max(100.0), self.def.height.max(60.0));
        let (resp, painter) = ui.allocate_painter(size + vec2(40.0, 40.0), Sense::click());
        let origin = resp.rect.min;
        let vis = ui.visuals().clone();
        let form = Rect::from_min_size(origin, size);
        painter.rect(form, 4.0, vis.faint_bg_color, Stroke::new(1.0, vis.widgets.inactive.bg_stroke.color), egui::StrokeKind::Inside);
        // Grid dots.
        let mut y = GRID * 4.0;
        while y < size.y {
            let mut x = GRID * 4.0;
            while x < size.x {
                painter.circle_filled(origin + vec2(x, y), 0.6, vis.weak_text_color());
                x += GRID * 4.0;
            }
            y += GRID * 4.0;
        }
        if resp.clicked() {
            self.sel = None;
        }
        // The form's own corner resizes it.
        let corner = Rect::from_center_size(form.max, vec2(10.0, 10.0));
        let cr = ui.interact(corner, ui.id().with("db_form_corner"), Sense::drag());
        painter.rect_filled(corner, 1.0, vis.widgets.inactive.bg_fill);
        if cr.drag_started() {
            self.drag_from = Some(self.def.clone());
        }
        if cr.dragged() {
            self.def.width = (self.def.width + cr.drag_delta().x).max(100.0);
            self.def.height = (self.def.height + cr.drag_delta().y).max(60.0);
        }
        let mut released = cr.drag_stopped();
        for i in 0..self.def.controls.len() {
            let c = self.def.controls[i].clone();
            let r = Rect::from_min_size(origin + vec2(c.x, c.y), vec2(c.w, c.h));
            let selected = self.sel == Some(i);
            let fill = match c.kind {
                CtlKind::Label => egui::Color32::TRANSPARENT,
                CtlKind::Button => vis.widgets.inactive.bg_fill,
                _ => vis.extreme_bg_color,
            };
            painter.rect(r, 2.0, fill, Stroke::new(if selected { 2.0 } else { 1.0 }, if selected { vis.selection.stroke.color } else { vis.widgets.noninteractive.bg_stroke.color }), egui::StrokeKind::Inside);
            let text = match c.kind {
                CtlKind::Label | CtlKind::Button => c.text.clone(),
                CtlKind::CheckBox => "☐".to_string(),
                CtlKind::ComboBox => format!("{} ▾", c.field),
                CtlKind::TextBox => c.field.clone(),
            };
            let align = if c.kind == CtlKind::Button { egui::Align2::CENTER_CENTER } else { egui::Align2::LEFT_CENTER };
            let at = if c.kind == CtlKind::Button { r.center() } else { r.left_center() + vec2(4.0, 0.0) };
            painter.with_clip_rect(r).text(at, align, text, egui::FontId::proportional(12.0), vis.text_color());
            let mr = ui.interact(r, ui.id().with(("db_ctl", i)), Sense::click_and_drag());
            if mr.clicked() || mr.drag_started() {
                self.sel = Some(i);
            }
            if mr.drag_started() {
                self.drag_from = Some(self.def.clone());
            }
            if mr.dragged() {
                let c = &mut self.def.controls[i];
                c.x = (c.x + mr.drag_delta().x).max(0.0);
                c.y = (c.y + mr.drag_delta().y).max(0.0);
            }
            released |= mr.drag_stopped();
            if selected {
                let handle = Rect::from_center_size(r.max, vec2(8.0, 8.0));
                painter.rect_filled(handle, 1.0, vis.selection.stroke.color);
                let hr = ui.interact(handle, ui.id().with(("db_ctl_size", i)), Sense::drag());
                if hr.drag_started() {
                    self.drag_from = Some(self.def.clone());
                }
                if hr.dragged() {
                    let c = &mut self.def.controls[i];
                    c.w = (c.w + hr.drag_delta().x).max(12.0);
                    c.h = (c.h + hr.drag_delta().y).max(12.0);
                }
                released |= hr.drag_stopped();
            }
        }
        if released {
            for c in &mut self.def.controls {
                c.x = snap(c.x);
                c.y = snap(c.y);
                c.w = snap(c.w);
                c.h = snap(c.h);
            }
            if let Some(b) = self.drag_from.take() {
                self.remember(b);
            }
        }
    }

    fn props(&mut self, ui: &mut egui::Ui, v: &DbView) {
        let db = &v.db;
        if self.macros.is_none() && self.sel.and_then(|i| self.def.controls.get(i)).is_some_and(|c| c.action == Action::RunMacro) {
            self.macros = Some(v.macro_names());
        }
        let d = &mut self.def;
        ui.label(RichText::new(tr("db.form")).strong());
        egui::Grid::new("db_form_grid").num_columns(2).show(ui, |ui| {
            ui.label(tr("db.r_title"));
            ui.text_edit_singleline(&mut d.title);
            ui.end_row();
            ui.label(tr("db.form_kind"));
            egui::ComboBox::from_id_salt("db_form_kind").selected_text(super::trk("db.fkind", d.kind.key())).show_ui(ui, |ui| {
                for k in FormKind::ALL {
                    ui.selectable_value(&mut d.kind, k, super::trk("db.fkind", k.key()));
                }
            });
            ui.end_row();
            ui.label(tr("db.source"));
            source_combo(ui, "db_form_source", db, &mut d.source);
            ui.end_row();
            if d.kind == FormKind::WithSubform {
                let cols = columns(db, &d.source);
                ui.label(tr("db.subform"));
                source_combo(ui, "db_form_sub", db, &mut d.sub_source);
                ui.end_row();
                ui.label(tr("db.master_field"));
                field_combo(ui, "db_form_master", &cols, &mut d.master_field);
                ui.end_row();
                let sub_cols = columns(db, &d.sub_source);
                ui.label(tr("db.child_field"));
                field_combo(ui, "db_form_child", &sub_cols, &mut d.child_field);
                ui.end_row();
            }
        });
        let cols = columns(db, &d.source);
        let Some(i) = self.sel.filter(|&i| i < d.controls.len()) else {
            ui.add_space(8.0);
            ui.label(RichText::new(tr("db.select_control")).weak());
            return;
        };
        ui.add_space(8.0);
        let c = &mut d.controls[i];
        ui.label(RichText::new(super::trk("db.ctl", c.kind.key())).strong());
        egui::Grid::new("db_ctl_grid").num_columns(2).show(ui, |ui| {
            if c.kind != CtlKind::Label {
                ui.label(tr("db.ctl_name")).on_hover_text(tr("db.ctl_name_tip"));
                ui.text_edit_singleline(&mut c.name);
                ui.end_row();
            }
            if matches!(c.kind, CtlKind::Label | CtlKind::Button) {
                ui.label(tr("db.ctl_text"));
                ui.text_edit_singleline(&mut c.text);
                ui.end_row();
            }
            if matches!(c.kind, CtlKind::TextBox | CtlKind::ComboBox | CtlKind::CheckBox) {
                ui.label(tr("db.ctl_field"));
                field_combo(ui, "db_ctl_field", &cols, &mut c.field);
                ui.end_row();
            }
            if c.kind == CtlKind::ComboBox {
                ui.label(tr("db.list_table"));
                source_combo(ui, "db_ctl_list", db, &mut c.list_table);
                ui.end_row();
                let lc = columns(db, &c.list_table);
                ui.label(tr("db.list_value"));
                field_combo(ui, "db_ctl_lv", &lc, &mut c.list_value);
                ui.end_row();
                ui.label(tr("db.list_show"));
                field_combo(ui, "db_ctl_ls", &lc, &mut c.list_show);
                ui.end_row();
            }
            if c.kind == CtlKind::Button {
                ui.label(tr("db.ctl_action"));
                egui::ComboBox::from_id_salt("db_ctl_action").selected_text(super::trk("db.action", c.action.key())).show_ui(ui, |ui| {
                    for a in Action::ALL {
                        ui.selectable_value(&mut c.action, a, super::trk("db.action", a.key()));
                    }
                });
                ui.end_row();
                if matches!(c.action, Action::OpenReport | Action::OpenForm) {
                    let kind = if c.action == Action::OpenReport { Kind::Report } else { Kind::Form };
                    ui.label(tr("db.ctl_target"));
                    field_combo(ui, "db_ctl_target", &db.objects(kind), &mut c.target);
                    ui.end_row();
                }
                if c.action == Action::RunMacro {
                    ui.label(tr("db.ctl_macro"));
                    let names = self.macros.clone().unwrap_or_default();
                    if names.is_empty() {
                        ui.text_edit_singleline(&mut c.target).on_hover_text(tr("db.no_macros"));
                    } else {
                        field_combo(ui, "db_ctl_macro", &names, &mut c.target);
                    }
                    ui.end_row();
                }
            }
            ui.label("X, Y");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut c.x).range(0.0..=2000.0));
                ui.add(egui::DragValue::new(&mut c.y).range(0.0..=2000.0));
            });
            ui.end_row();
            ui.label(tr("db.ctl_size"));
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut c.w).range(8.0..=2000.0));
                ui.add(egui::DragValue::new(&mut c.h).range(8.0..=2000.0));
            });
            ui.end_row();
        });
    }
}

fn snap(x: f32) -> f32 {
    (x / GRID).round() * GRID
}

fn columns(db: &Db, source: &str) -> Vec<String> {
    if source.is_empty() {
        return Vec::new();
    }
    db.columns(&RowSource::Table(source.to_string())).map(|c| c.0).unwrap_or_default()
}
