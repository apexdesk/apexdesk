//! Page types: the window that defines them (fields, color, template),
//! the open page's fields under its title, and new pages of a type.

use apex_doc::notebook::{Claim, ClaimKind, Field, FieldKind, PageKind, Status};

/// What a page is, as the page menu offers it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Kind {
    Plain,
    Source,
    Claim(ClaimKind),
    Type(i64),
}

use super::*;

fn kind_name(k: FieldKind) -> String {
    tr(match k {
        FieldKind::Text => "nb.field_text",
        FieldKind::Number => "nb.field_number",
        FieldKind::Date => "nb.field_date",
        FieldKind::Choice => "nb.field_choice",
        FieldKind::Check => "nb.field_check",
    })
    .to_string()
}

/// Whether `value` suits a field of kind `k`.
fn fits(k: FieldKind, value: &str) -> bool {
    let v = value.trim();
    v.is_empty()
        || match k {
            FieldKind::Number => apex_doc::notebook::types::number(v).is_some(),
            FieldKind::Date => apex_doc::notebook::tasks::is_date(v),
            _ => true,
        }
}

/// A one-line edit whose text is handed back when it loses focus
/// changed. `id` keeps what is typed meanwhile.
pub(super) fn line_edit(ui: &mut egui::Ui, id: Id, current: &str, hint: &str, width: f32) -> Option<String> {
    let buf = id.with("text");
    let mut text = ui.data_mut(|d| d.get_temp::<String>(buf)).unwrap_or_else(|| current.to_string());
    // Sized: in a grid a text edit would shrink to its column.
    let r = ui.add_sized(vec2(width, ui.spacing().interact_size.y), egui::TextEdit::singleline(&mut text).id(id).hint_text(hint));
    if r.lost_focus() {
        ui.data_mut(|d| d.remove::<String>(buf));
        return (text != current).then_some(text);
    }
    if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(buf, text));
    } else {
        ui.data_mut(|d| d.remove::<String>(buf));
    }
    None
}

/// Puts the focus on the edit `id` with all of `text` selected, so what is
/// typed replaces it (the edit may appear only in the next frame).
pub(super) fn focus_all(ctx: &egui::Context, id: Id, text: &str) {
    let mut st = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    st.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(text.chars().count()))));
    st.store(ctx, id);
    ctx.memory_mut(|m| m.request_focus(id));
}

/// The editor of a field's value, by its kind. Returns the new value.
pub(super) fn value_edit(ui: &mut egui::Ui, id: Id, f: &Field, value: &str, width: f32) -> Option<String> {
    match f.kind {
        FieldKind::Check => {
            let mut on = !value.is_empty();
            ui.checkbox(&mut on, "").changed().then(|| if on { "1".to_string() } else { String::new() })
        }
        FieldKind::Choice => {
            let mut pick = None;
            egui::ComboBox::from_id_salt(id).width(width).selected_text(value).show_ui(ui, |ui| {
                if ui.selectable_label(value.is_empty(), "—").clicked() {
                    pick = Some(String::new());
                }
                for o in &f.options {
                    if ui.selectable_label(value == o, o).clicked() {
                        pick = Some(o.clone());
                    }
                }
            });
            pick.filter(|p| p != value)
        }
        k => {
            let hint = if k == FieldKind::Date { tr("nb.date_hint").to_string() } else { String::new() };
            line_edit(ui, id, value, &hint, width).filter(|v| fits(k, v))
        }
    }
}

/// The page types window: which one is chosen.
#[derive(Default)]
pub(super) struct TypesWindow {
    sel: Option<i64>,
    /// A type just made: its name is selected to give it one.
    name_new: bool,
}

impl NoteView {
    pub fn open_types(&mut self, sel: Option<i64>) {
        let sel = sel.or_else(|| self.page.and_then(|p| self.nb.page(p)).and_then(|p| p.ptype)).or_else(|| self.nb.types.first().map(|t| t.id));
        self.close_windows();
        self.types_win = Some(TypesWindow { sel, name_new: false });
    }

    /// A new page of type `ty` after the open one, starting with its
    /// template's text.
    pub fn new_typed_page(&mut self, ty: i64) {
        let s = match self.section {
            Some(s) => s,
            None => self.here(),
        };
        self.flush();
        let after = self.page.and_then(|p| self.nb.page(p)).map(|p| (p.parent, p.id));
        let mut id = 0;
        let title = tr("nb.untitled").to_string();
        self.change(|nb| {
            id = nb.add_page(s, after.and_then(|a| a.0), after.map(|a| a.1), &title, now());
            if let Some(p) = nb.page_mut(id) {
                p.ptype = Some(ty);
            }
        });
        let template = self.nb.page_type(ty).and_then(|t| t.template).filter(|t| self.nb.is_live(*t) && self.nb.page(*t).is_some_and(|p| p.kind == PageKind::Text));
        if let Some(t) = template {
            let r = self.store.load_page(t).and_then(|doc| {
                let text = plain_text(&doc);
                let links = apex_doc::notebook::links::page_links(&doc);
                self.store.save_page(id, &doc, &text, &links, now())?;
                self.nb.texts.insert(id, text);
                self.nb.links.insert(id, links);
                self.nb.tasks.insert(id, apex_doc::notebook::tasks::tasks_in(&doc));
                Ok(())
            });
            self.report(r);
        }
        self.close_graph();
        self.open_page(id);
        self.name_new = true;
        self.grid = false;
    }

    /// The page menu's choice of what the page is: a source, a question or
    /// a hypothesis, one of the notebook's types, or a plain page.
    pub(super) fn type_menu(&mut self, ui: &mut egui::Ui, id: i64) {
        let Some(p) = self.nb.page(id) else { return };
        let (ptype, source, claim) = (p.ptype, p.source.is_some(), p.claim.map(|c| c.kind));
        crate::widgets::menu_button(ui, tr("nb.page_type"), |ui| {
            let mut pick = None;
            if ui.radio(ptype.is_none() && !source && claim.is_none(), tr("nb.no_type")).clicked() {
                pick = Some(Kind::Plain);
            }
            if ui.radio(source, tr("nb.source")).clicked() {
                pick = Some(Kind::Source);
            }
            for k in [ClaimKind::Question, ClaimKind::Hypothesis] {
                if ui.radio(claim == Some(k), super::claims::kind_name(k)).clicked() {
                    pick = Some(Kind::Claim(k));
                }
            }
            if !self.nb.types.is_empty() {
                ui.separator();
            }
            for t in &self.nb.types {
                if ui.radio(ptype == Some(t.id), &t.name).clicked() {
                    pick = Some(Kind::Type(t.id));
                }
            }
            ui.separator();
            if ui.button(format!("{}…", tr("nb.types"))).clicked() {
                self.open_types(ptype);
                ui.close();
            }
            if let Some(k) = pick {
                self.set_kind(id, k);
                ui.close();
            }
        });
    }

    /// Makes page `id` one kind of page, and no other.
    pub(super) fn set_kind(&mut self, id: i64, kind: Kind) {
        self.change(|nb| {
            let Some(p) = nb.page_mut(id) else { return };
            p.source = match kind {
                Kind::Source => p.source.take().or_else(|| Some(apex_doc::Source { title: p.title.clone(), ..Default::default() })),
                _ => None,
            };
            p.claim = match kind {
                Kind::Claim(k) => Some(Claim { kind: k, status: p.claim.map_or(Status::Open, |c| c.status) }),
                _ => None,
            };
            p.ptype = match kind {
                Kind::Type(t) => Some(t),
                _ => None,
            };
        });
    }

    /// The open page's fields, under its title.
    pub(super) fn fields_ui(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.page else { return };
        let Some(t) = self.nb.page(id).and_then(|p| p.ptype).and_then(|t| self.nb.page_type(t)).cloned() else { return };
        let mut set = None;
        let mut edit = false;
        let color = section_color(t.color);
        ui.add_space(4.0);
        egui::Frame::new().fill(color.gamma_multiply(0.10)).stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.6))).corner_radius(4.0).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&t.name).strong().color(color));
                if ui.link(RichText::new(tr("nb.edit_type")).small()).clicked() {
                    edit = true;
                }
            });
            if t.fields.is_empty() {
                ui.label(RichText::new(tr("nb.type_no_fields")).weak().small());
                return;
            }
            egui::Grid::new(("note_fields", self.uid, id)).num_columns(2).spacing(vec2(12.0, 4.0)).show(ui, |ui| {
                for (i, f) in t.fields.iter().enumerate() {
                    ui.label(RichText::new(&f.name).weak());
                    let value = self.nb.field_value(id, &f.name).to_string();
                    if let Some(v) = value_edit(ui, Id::new(("note_field", self.uid, id, i)), f, &value, 260.0) {
                        set = Some((f.name.clone(), v));
                    }
                    ui.end_row();
                }
            });
        });
        if let Some((name, v)) = set {
            self.change(|nb| nb.set_field(id, &name, &v));
        }
        if edit {
            self.open_types(Some(t.id));
        }
    }

    /// The page types window.
    pub(super) fn types_window(&mut self, ctx: &egui::Context) {
        let Some(mut w) = self.types_win.take() else { return };
        if w.sel.is_some_and(|s| self.nb.page_type(s).is_none()) {
            w.sel = self.nb.types.first().map(|t| t.id);
        }
        let mut open = true;
        let mut ops: Vec<Box<dyn FnOnce(&mut Notebook)>> = Vec::new();
        let mut add = false;
        crate::widgets::esc_closes(ctx, Id::new(("note_types", self.uid)), &mut open);
        egui::Window::new(tr("nb.types")).id(Id::new(("note_types", self.uid))).open(&mut open).collapsible(false).default_width(620.0).pivot(egui::Align2::CENTER_CENTER).default_pos(ctx.content_rect().center()).show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(150.0);
                    for t in &self.nb.types {
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                            ui.painter().circle_filled(r.center(), 4.5, section_color(t.color));
                            if ui.selectable_label(w.sel == Some(t.id), &t.name).clicked() {
                                w.sel = Some(t.id);
                            }
                        });
                    }
                    if self.nb.types.is_empty() {
                        ui.label(RichText::new(tr("nb.types_none")).weak().small());
                    }
                    ui.add_space(6.0);
                    if ui.button(format!("+ {}", tr("nb.new_type"))).clicked() {
                        add = true;
                    }
                });
                ui.separator();
                let Some(t) = w.sel.and_then(|s| self.nb.page_type(s)).cloned() else { return };
                ui.vertical(|ui| {
                    let tid = t.id;
                    egui::Grid::new(("note_type_head", self.uid)).num_columns(2).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
                        ui.label(tr("nb.type_name"));
                        // Asked until it has it: the page's editor may take it.
                        let name_id = Id::new(("note_type_name", self.uid, tid));
                        if w.name_new && ui.ctx().memory(|m| m.has_focus(name_id)) {
                            w.name_new = false;
                        } else if w.name_new {
                            focus_all(ui.ctx(), name_id, &t.name);
                        }
                        if let Some(n) = line_edit(ui, Id::new(("note_type_name", self.uid, tid)), &t.name, "", 220.0).filter(|n| !n.trim().is_empty()) {
                            ops.push(Box::new(move |nb| {
                                if let Some(t) = nb.page_type_mut(tid) {
                                    t.name = n.trim().to_string();
                                }
                            }));
                        }
                        ui.end_row();
                        ui.label(tr("nb.type_color"));
                        ui.horizontal(|ui| {
                            for c in apex_doc::notebook::SECTION_COLORS {
                                let (r, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
                                ui.painter().circle_filled(r.center(), 7.0, section_color(c));
                                if t.color == c {
                                    ui.painter().circle_stroke(r.center(), 8.5, egui::Stroke::new(1.5, ui.visuals().text_color()));
                                }
                                if resp.clicked() {
                                    ops.push(Box::new(move |nb| {
                                        if let Some(t) = nb.page_type_mut(tid) {
                                            t.color = c;
                                        }
                                    }));
                                }
                            }
                        });
                        ui.end_row();
                        ui.label(tr("nb.type_template"));
                        let name = t.template.and_then(|p| self.nb.page(p)).filter(|p| self.nb.is_live(p.id)).map_or_else(|| tr("nb.no_template").to_string(), |p| p.title.clone());
                        ui.horizontal(|ui| {
                            egui::ComboBox::from_id_salt(("note_type_template", self.uid)).width(220.0).selected_text(name).show_ui(ui, |ui| {
                                if ui.selectable_label(t.template.is_none(), tr("nb.no_template")).clicked() {
                                    ops.push(Box::new(move |nb| {
                                        if let Some(t) = nb.page_type_mut(tid) {
                                            t.template = None;
                                        }
                                    }));
                                }
                                for s in self.nb.live_sections() {
                                    for (_, p) in self.nb.tree(s.id) {
                                        if p.kind != PageKind::Text {
                                            continue;
                                        }
                                        let pid = p.id;
                                        if ui.selectable_label(t.template == Some(pid), &p.title).clicked() {
                                            ops.push(Box::new(move |nb| {
                                                if let Some(t) = nb.page_type_mut(tid) {
                                                    t.template = Some(pid);
                                                }
                                            }));
                                        }
                                    }
                                }
                            });
                            ui.label(RichText::new(tr("nb.type_template_tip")).weak().small());
                        });
                        ui.end_row();
                    });
                    ui.add_space(8.0);
                    ui.label(RichText::new(tr("nb.type_fields")).strong());
                    egui::Grid::new(("note_type_fields", self.uid, tid)).num_columns(4).spacing(vec2(8.0, 4.0)).show(ui, |ui| {
                        let n = t.fields.len();
                        for (i, f) in t.fields.iter().enumerate() {
                            let old = f.name.clone();
                            if let Some(new) = line_edit(ui, Id::new(("note_field_name", self.uid, tid, i)), &f.name, tr("nb.field_name"), 140.0) {
                                ops.push(Box::new(move |nb| nb.rename_field(tid, &old, new.trim())));
                            }
                            let mut kind = f.kind;
                            egui::ComboBox::from_id_salt(("note_field_kind", self.uid, tid, i)).width(110.0).selected_text(kind_name(kind)).show_ui(ui, |ui| {
                                for k in FieldKind::ALL {
                                    ui.selectable_value(&mut kind, k, kind_name(k));
                                }
                            });
                            if kind != f.kind {
                                ops.push(Box::new(move |nb| {
                                    if let Some(f) = nb.page_type_mut(tid).and_then(|t| t.fields.get_mut(i)) {
                                        f.kind = kind;
                                    }
                                }));
                            }
                            if f.kind == FieldKind::Choice {
                                let joined = f.options.join(", ");
                                if let Some(o) = line_edit(ui, Id::new(("note_field_opts", self.uid, tid, i)), &joined, tr("nb.field_options"), 180.0) {
                                    let opts: Vec<String> = o.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                                    ops.push(Box::new(move |nb| {
                                        if let Some(f) = nb.page_type_mut(tid).and_then(|t| t.fields.get_mut(i)) {
                                            f.options = opts;
                                        }
                                    }));
                                }
                            } else {
                                ui.label("");
                            }
                            ui.horizontal(|ui| {
                                for (on, label, to) in [(i > 0, "⏶", i.wrapping_sub(1)), (i + 1 < n, "⏷", i + 1)] {
                                    if ui.add_enabled(on, egui::Button::new(label).small()).clicked() {
                                        ops.push(Box::new(move |nb| {
                                            if let Some(t) = nb.page_type_mut(tid) {
                                                t.fields.swap(i, to);
                                            }
                                        }));
                                    }
                                }
                                if ui.small_button("×").on_hover_text(tr("nb.field_remove")).clicked() {
                                    ops.push(Box::new(move |nb| {
                                        if let Some(t) = nb.page_type_mut(tid) {
                                            t.fields.remove(i);
                                        }
                                    }));
                                }
                            });
                            ui.end_row();
                        }
                    });
                    if ui.button(format!("+ {}", tr("nb.field_add"))).clicked() {
                        let name = (1..).map(|n| format!("{} {n}", tr("nb.field_name"))).find(|n| !t.fields.iter().any(|f| &f.name == n)).unwrap_or_default();
                        focus_all(ui.ctx(), Id::new(("note_field_name", self.uid, tid, t.fields.len())), &name);
                        ops.push(Box::new(move |nb| {
                            if let Some(t) = nb.page_type_mut(tid) {
                                t.fields.push(Field::new(&name, FieldKind::Text));
                            }
                        }));
                    }
                    ui.add_space(8.0);
                    ui.separator();
                    ui.horizontal(|ui| {
                        let count = self.nb.pages_of_type(tid).len();
                        ui.label(RichText::new(trf("nb.type_pages", &[("n", &count)])).weak());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(tr("nb.type_delete")).clicked() {
                                ops.push(Box::new(move |nb| nb.delete_type(tid)));
                            }
                        });
                    });
                });
            });
        });
        if add {
            let mut id = 0;
            let name = tr("nb.new_type").to_string();
            self.change(|nb| id = nb.add_type(&name));
            w.sel = Some(id);
            w.name_new = true;
        }
        if !ops.is_empty() {
            self.change(|nb| {
                for op in ops {
                    op(nb);
                }
            });
        }
        if open {
            self.types_win = Some(w);
        }
    }
}
