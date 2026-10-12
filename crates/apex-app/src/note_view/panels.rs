//! The notebook's side panels: its sections, the pages of the one shown
//! (or what a search found) and the trash.

use super::*;
use crate::icons::Icon;
use crate::widgets::Face;
use apex_doc::notebook::SECTION_COLORS;

const ROW_H: f32 = 26.0;
const INDENT: f32 = 16.0;
/// Below this width the sections panel shows only their colours.
const NARROW_W: f32 = 90.0;

/// Where a page dragged onto a row goes.
pub(super) enum Drop {
    Before(i64),
    After(i64),
    Under(i64),
    Section(i64),
}

impl NoteView {
    /// A row of the side panels: its rect and response, painted selected
    /// or under the pointer.
    fn row(ui: &mut egui::Ui, selected: bool) -> (egui::Rect, egui::Response) {
        let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click_and_drag());
        let v = ui.visuals();
        if selected {
            ui.painter().rect_filled(rect, 4.0, v.selection.bg_fill.gamma_multiply(0.45));
        } else if r.hovered() {
            ui.painter().rect_filled(rect, 4.0, v.widgets.hovered.weak_bg_fill);
        }
        (rect, r)
    }

    /// The name being typed in place of a row's; true when done.
    fn naming_ui(&mut self, ui: &mut egui::Ui, rect: egui::Rect) -> bool {
        let Some((_, text)) = self.naming.as_mut() else { return false };
        let r = ui.put(rect.shrink2(vec2(4.0, 2.0)), egui::TextEdit::singleline(text));
        if !r.has_focus() && !r.lost_focus() {
            r.request_focus();
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.naming = None;
            return false;
        }
        r.lost_focus()
    }

    pub(super) fn sections_ui(&mut self, ui: &mut egui::Ui) {
        // Narrowed down to the colours: names show on hover.
        let narrow = ui.available_width() < NARROW_W;
        ui.add_space(6.0);
        if !narrow {
            ui.add(egui::Label::new(RichText::new(tr("nb.sections")).strong()).truncate());
        }
        ui.add_space(4.0);
        let sections: Vec<(i64, String, u32)> = self.nb.live_sections().iter().map(|s| (s.id, s.name.clone(), s.color)).collect();
        let dragging = self.drag;
        let mut drop_on = None;
        let tags = !self.nb.tags().is_empty();
        let room = ui.available_height() - 70.0;
        egui::ScrollArea::vertical().id_salt("note_sections_scroll").auto_shrink([false, true]).max_height(if tags { room * 0.55 } else { room }).show(ui, |ui| {
            for (id, name, color) in &sections {
                let (rect, r) = Self::row(ui, self.section == Some(*id));
                if self.naming.as_ref().is_some_and(|n| n.0 == Naming::Section(*id)) {
                    if self.naming_ui(ui, rect)
                        && let Some((what, text)) = self.naming.take()
                    {
                        self.rename(what, &text);
                    }
                    continue;
                }
                let r = if narrow { r.on_hover_text(name) } else { r };
                if narrow {
                    let chip = egui::Rect::from_center_size(rect.center(), vec2((rect.width() - 10.0).clamp(6.0, 18.0), ROW_H - 10.0));
                    ui.painter().rect_filled(chip, 3.0, section_color(*color));
                }
                let bar = egui::Rect::from_min_size(rect.min + vec2(4.0, 5.0), vec2(5.0, ROW_H - 10.0));
                if !narrow {
                    ui.painter().rect_filled(bar, 2.0, section_color(*color));
                }
                let font = egui::FontId::proportional(14.0);
                let text_color = ui.visuals().text_color();
                // One line, cut short when the panel is narrow: wrapped,
                // names ran into the rows below.
                let mut job = egui::text::LayoutJob::simple_singleline(name.clone(), font, text_color);
                job.wrap = egui::text::TextWrapping { max_width: (rect.width() - 40.0).max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…'), ..Default::default() };
                let galley = ui.painter().layout_job(job);
                if !narrow {
                    ui.painter().galley(egui::pos2(rect.min.x + 16.0, rect.center().y - galley.size().y / 2.0), galley, text_color);
                }
                if !narrow && self.has_lock(*id) {
                    let c = if self.nb.sealed.contains(id) { text_color } else { ui.visuals().weak_text_color() };
                    crate::icons::paint(ui.painter(), egui::Rect::from_center_size(egui::pos2(rect.max.x - 14.0, rect.center().y), egui::Vec2::splat(14.0)), crate::icons::Icon::Protect, c);
                }
                crate::widgets::name(&r, egui::WidgetType::SelectableLabel, name, Some(self.section == Some(*id)));
                if r.clicked() {
                    self.close_graph();
                    self.show_section(*id);
                }
                if r.double_clicked() {
                    self.naming = Some((Naming::Section(*id), name.clone()));
                }
                if r.drag_started() {
                    self.drag = Some(Naming::Section(*id));
                }
                if let Some(what) = dragging
                    && ui.rect_contains_pointer(rect)
                {
                    let stroke = ui.visuals().selection.stroke;
                    if matches!(what, Naming::Page(_)) {
                        // A page goes into the section: mark all of it.
                        ui.painter().rect_filled(rect, 4.0, ui.visuals().selection.bg_fill.gamma_multiply(0.3));
                        ui.painter().rect_stroke(rect, 4.0, stroke, egui::StrokeKind::Inside);
                    } else {
                        // A section goes before this one: mark the gap.
                        ui.painter().hline(rect.x_range(), rect.min.y, stroke);
                    }
                    drop_on = Some(*id);
                }
                r.context_menu(|ui| {
                    if ui.button(tr("nb.rename")).clicked() {
                        self.naming = Some((Naming::Section(*id), name.clone()));
                        ui.close();
                    }
                    crate::widgets::menu_button(ui, tr("nb.color"), |ui| {
                        ui.horizontal(|ui| {
                            for c in SECTION_COLORS {
                                let (sw, sr) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
                                ui.painter().rect_filled(sw, 4.0, section_color(c));
                                if sr.clicked() {
                                    let id = *id;
                                    self.change(|nb| {
                                        if let Some(s) = nb.section_mut(id) {
                                            s.color = c;
                                        }
                                    });
                                    ui.close();
                                }
                            }
                        });
                    });
                    ui.separator();
                    let sealed = self.nb.sealed.contains(id);
                    if !self.has_lock(*id) {
                        if ui.button(tr("nb.lock_set")).clicked() {
                            self.ask_password(*id, true);
                            ui.close();
                        }
                    } else if sealed {
                        if ui.button(tr("nb.lock_unlock")).clicked() {
                            self.ask_password(*id, false);
                            ui.close();
                        }
                    } else {
                        if ui.button(tr("nb.lock_now")).clicked() {
                            self.lock_section_now(*id);
                            ui.close();
                        }
                        if ui.button(tr("nb.lock_remove")).clicked() {
                            self.remove_section_lock(*id);
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button(tr("nb.delete_section")).clicked() {
                        self.delete_section(*id);
                        ui.close();
                    }
                });
            }
        });
        // Dropped.
        if ui.input(|i| i.pointer.any_released())
            && let Some(target) = drop_on
            && let Some(what) = self.drag.take()
        {
            match what {
                Naming::Section(id) if id != target => self.change(|nb| nb.move_section(id, Some(target))),
                Naming::Page(p) => self.drop_page(p, Drop::Section(target)),
                _ => {}
            }
        }
        if tags && !narrow {
            self.tags_ui(ui, (ui.available_height() - 100.0).max(60.0));
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(6.0);
            let trash = self.nb.trash().len();
            let due = self.tasks_due();
            let new = tr("nb.new_section").to_string();
            let tasks = if due > 0 { format!("{} ({due})", tr("nb.tasks")) } else { tr("nb.tasks").to_string() };
            let bin = if trash > 0 { format!("{} ({trash})", tr("nb.trash")) } else { tr("nb.trash").to_string() };
            let mut buttons = |ui: &mut egui::Ui| {
                // A dot on a button with something waiting behind it.
                let dot = |ui: &egui::Ui, r: &egui::Response| ui.painter().circle_filled(r.rect.right_top() + vec2(-4.0, 4.0), 3.0, ui.visuals().selection.bg_fill);
                if crate::widgets::tool_button(ui, Face::Icon(Icon::Add), false, &new).clicked() {
                    self.new_section();
                }
                let r = crate::widgets::tool_button(ui, Face::Icon(Icon::Tasks), false, &tasks);
                if due > 0 {
                    dot(ui, &r);
                }
                if r.clicked() {
                    self.open_tasks();
                }
                let r = crate::widgets::tool_button(ui, Face::Icon(Icon::Trash), false, &bin);
                if trash > 0 {
                    dot(ui, &r);
                }
                if r.clicked() {
                    self.close_windows();
                    self.trash_open = true;
                }
            };
            if narrow {
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| buttons(ui));
            } else {
                ui.horizontal(|ui| buttons(ui));
                self.queries_list(ui);
            }
        });
    }

    pub(super) fn pages_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        let r = ui.add(egui::TextEdit::singleline(&mut self.search).id(Id::new(("note_search", self.uid))).hint_text(tr("nb.search_hint")).desired_width(f32::INFINITY));
        // Escape takes the focus away in the same frame.
        if (r.has_focus() || r.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.search.clear();
        }
        ui.add_space(6.0);
        if !self.search.trim().is_empty() {
            self.results_ui(ui);
            return;
        }
        let Some(section) = self.section else { return };
        if self.nb.sealed.contains(&section) {
            return self.sealed_ui(ui, section);
        }
        let tree: Vec<(usize, i64, String)> = self.nb.tree(section).into_iter().map(|(d, p)| (d, p.id, p.title.clone())).collect();
        let dragging = self.drag;
        let mut drop_at = None;
        egui::ScrollArea::vertical().id_salt("note_pages_scroll").auto_shrink([false, true]).max_height(ui.available_height() - 40.0).show(ui, |ui| {
            for (depth, id, title) in &tree {
                let (rect, r) = Self::row(ui, self.page == Some(*id));
                if self.naming.as_ref().is_some_and(|n| n.0 == Naming::Page(*id)) {
                    if self.naming_ui(ui, rect)
                        && let Some((what, text)) = self.naming.take()
                    {
                        self.rename(what, &text);
                        if self.page == Some(*id) {
                            self.title = text.trim().to_string();
                        }
                    }
                    continue;
                }
                let mut x = rect.min.x + 8.0 + *depth as f32 * INDENT;
                let text_color = ui.visuals().text_color();
                if self.nb.page(*id).is_some_and(|p| p.kind == apex_doc::notebook::PageKind::Canvas) {
                    crate::icons::paint(ui.painter(), egui::Rect::from_center_size(egui::pos2(x + 7.0, rect.center().y), egui::Vec2::splat(14.0)), crate::icons::Icon::Canvas, ui.visuals().weak_text_color());
                    x += 20.0;
                }
                if let Some(c) = self.nb.page(*id).and_then(|p| p.claim) {
                    ui.painter().circle_filled(egui::pos2(x + 4.0, rect.center().y), 4.0, super::claims::status_color(c.status));
                    x += 14.0;
                }
                let galley = ui.painter().layout_no_wrap(title.clone(), egui::FontId::proportional(14.0), text_color);
                ui.painter().with_clip_rect(rect).galley(egui::pos2(x, rect.center().y - galley.size().y / 2.0), galley, text_color);
                crate::widgets::name(&r, egui::WidgetType::SelectableLabel, title, Some(self.page == Some(*id)));
                if r.clicked() {
                    self.close_graph();
                    self.open_page(*id);
                    self.grid = false;
                }
                if r.double_clicked() {
                    self.naming = Some((Naming::Page(*id), title.clone()));
                }
                if r.drag_started() {
                    self.drag = Some(Naming::Page(*id));
                }
                if let Some(Naming::Page(d)) = dragging
                    && d != *id
                    && let Some(pos) = ui.ctx().pointer_hover_pos()
                    && rect.contains(pos)
                {
                    let stroke = ui.visuals().selection.stroke;
                    let at = if pos.y < rect.center().y {
                        ui.painter().hline(rect.x_range(), rect.min.y, stroke);
                        Drop::Before(*id)
                    } else if pos.x > x + 30.0 {
                        ui.painter().rect_stroke(rect, 4.0, stroke, egui::StrokeKind::Inside);
                        Drop::Under(*id)
                    } else {
                        ui.painter().hline(rect.x_range(), rect.max.y, stroke);
                        Drop::After(*id)
                    };
                    drop_at = Some(at);
                }
                let sections: Vec<(i64, String)> = self.nb.live_sections().iter().filter(|s| Some(s.id) != self.section).map(|s| (s.id, s.name.clone())).collect();
                r.context_menu(|ui| {
                    let text = self.nb.page(*id).is_some_and(|p| p.kind == apex_doc::notebook::PageKind::Text);
                    if text && Some(*id) != self.page && ui.button(tr("nb.open_beside")).clicked() {
                        self.open_beside(*id);
                        ui.close();
                    }
                    if ui.button(tr("nb.new_subpage")).clicked() {
                        self.open_page(*id);
                        self.new_page(true);
                        ui.close();
                    }
                    if ui.button(tr("nb.rename")).clicked() {
                        self.naming = Some((Naming::Page(*id), title.clone()));
                        ui.close();
                    }
                    if *depth > 0 && ui.button(tr("nb.promote")).clicked() {
                        let parent = self.nb.page(*id).and_then(|p| p.parent);
                        let up = parent.and_then(|p| self.nb.page(p)).map(|p| p.parent);
                        if let Some(up) = up {
                            let after = self.nb.children(section, up).iter().skip_while(|p| Some(p.id) != parent).nth(1).map(|p| p.id);
                            let id = *id;
                            self.change(|nb| {
                                nb.move_page(id, section, up, after);
                            });
                        }
                        ui.close();
                    }
                    self.type_menu(ui, *id);
                    if !sections.is_empty() {
                        crate::widgets::menu_button(ui, tr("nb.move_to"), |ui| {
                            for (s, name) in &sections {
                                if ui.button(name).clicked() {
                                    self.drop_page(*id, Drop::Section(*s));
                                    ui.close();
                                }
                            }
                        });
                    }
                    ui.separator();
                    if ui.button(tr("nb.delete_page")).clicked() {
                        self.delete_page(*id);
                        ui.close();
                    }
                });
            }
        });
        // What is being dragged, under the pointer.
        if let Some(what) = self.drag
            && let Some(pos) = ui.ctx().pointer_hover_pos()
        {
            let name = match what {
                Naming::Page(p) => self.nb.page(p).map(|p| p.title.clone()),
                Naming::Section(s) => self.nb.section(s).map(|s| s.name.clone()),
            };
            if let Some(name) = name {
                egui::Area::new(Id::new(("note_drag", self.uid))).order(egui::Order::Tooltip).fixed_pos(pos + vec2(14.0, 6.0)).interactable(false).show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| ui.label(name));
                });
            }
        }
        if ui.input(|i| i.pointer.any_released())
            && let Some(Naming::Page(p)) = self.drag
            && let Some(at) = drop_at.take()
        {
            self.drag = None;
            self.drop_page(p, at);
        }
        if ui.input(|i| !i.pointer.any_down()) && !ui.input(|i| i.pointer.any_released()) {
            self.drag = None;
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(6.0);
            if ui.button(format!("+ {}", tr("nb.new_page"))).clicked() {
                self.new_page(false);
            }
        });
    }

    /// Moves page `id` where it was dropped.
    pub(super) fn drop_page(&mut self, id: i64, at: Drop) {
        let place = |nb: &Notebook, target: i64| nb.page(target).map(|p| (p.section, p.parent));
        self.change(|nb| {
            match at {
                Drop::Before(t) => {
                    if let Some((s, parent)) = place(nb, t) {
                        nb.move_page(id, s, parent, Some(t));
                    }
                }
                Drop::After(t) => {
                    if let Some((s, parent)) = place(nb, t) {
                        let next = nb.children(s, parent).iter().skip_while(|p| p.id != t).nth(1).map(|p| p.id).filter(|&n| n != id);
                        nb.move_page(id, s, parent, next);
                    }
                }
                Drop::Under(t) => {
                    if let Some((s, _)) = place(nb, t) {
                        nb.move_page(id, s, Some(t), None);
                    }
                }
                Drop::Section(s) => {
                    nb.move_page(id, s, None, None);
                }
            };
        });
    }

    fn results_ui(&mut self, ui: &mut egui::Ui) {
        let hits = self.nb.search(&self.search);
        if hits.is_empty() {
            ui.label(RichText::new(tr("nb.no_results")).weak());
            return;
        }
        ui.label(RichText::new(trf("nb.results", &[("n", &hits.len())])).weak().small());
        egui::ScrollArea::vertical().id_salt("note_results_scroll").auto_shrink([false, true]).show(ui, |ui| {
            for h in &hits {
                let Some(p) = self.nb.page(h.page) else { continue };
                let section = self.nb.section(p.section).map(|s| (s.name.clone(), s.color));
                let r = egui::Frame::new().inner_margin(egui::Margin::symmetric(6, 4)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        if let Some((name, color)) = &section {
                            ui.label(RichText::new("■").color(section_color(*color)));
                            ui.label(RichText::new(&p.title).strong());
                            ui.label(RichText::new(name).weak().small());
                        }
                    });
                    let mut job = egui::text::LayoutJob::default();
                    let normal = egui::TextFormat { font_id: egui::FontId::proportional(12.0), color: ui.visuals().weak_text_color(), ..Default::default() };
                    match h.mark {
                        Some((a, b)) => {
                            job.append(&h.snippet[..a], 0.0, normal.clone());
                            job.append(&h.snippet[a..b], 0.0, egui::TextFormat { color: ui.visuals().text_color(), background: ui.visuals().selection.bg_fill.gamma_multiply(0.6), ..normal.clone() });
                            job.append(&h.snippet[b..], 0.0, normal);
                        }
                        None => job.append(&h.snippet, 0.0, normal),
                    }
                    job.wrap.max_width = ui.available_width();
                    ui.label(job);
                });
                let r = ui.interact(r.response.rect, Id::new(("note_hit", h.page)), Sense::click());
                if r.hovered() {
                    ui.painter().rect_stroke(r.rect, 4.0, ui.visuals().widgets.hovered.bg_stroke, egui::StrokeKind::Inside);
                }
                if r.clicked() {
                    self.close_graph();
                    self.open_page(h.page);
                    self.grid = false;
                    let first = self.search.split_whitespace().next().unwrap_or("").to_string();
                    if let Some(ed) = &mut self.editor {
                        ed.find_first(&first);
                    }
                }
                ui.separator();
            }
        });
    }

    pub(super) fn trash_ui(&mut self, ctx: &egui::Context) {
        if !self.trash_open {
            return;
        }
        let items = self.nb.trash();
        let mut open = true;
        let mut act = None;
        crate::widgets::esc_closes(ctx, Id::new(("note_trash", self.uid)), &mut open);
        egui::Window::new(tr("nb.trash")).id(Id::new(("note_trash", self.uid))).open(&mut open).collapsible(false).resizable(true).default_width(420.0).pivot(egui::Align2::CENTER_CENTER).default_pos(ctx.content_rect().center()).show(ctx, |ui| {
            ui.label(RichText::new(trf("nb.trash_note", &[("n", &apex_doc::notebook::TRASH_DAYS)])).weak().small());
            ui.add_space(6.0);
            if items.is_empty() {
                ui.label(RichText::new(tr("nb.trash_empty")).weak());
            }
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                for (t, at) in &items {
                    let (name, what) = match t {
                        Trashed::Section(s) => (self.nb.section(*s).map(|s| s.name.clone()).unwrap_or_default(), tr("nb.section")),
                        Trashed::Page(p) => (self.nb.page(*p).map(|p| p.title.clone()).unwrap_or_default(), tr("nb.page")),
                    };
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(name).strong());
                            ui.label(RichText::new(format!("{what} · {}", date_text(*at))).weak().small());
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(tr("nb.purge")).clicked() {
                                act = Some((*t, false));
                            }
                            if ui.button(tr("nb.restore")).clicked() {
                                act = Some((*t, true));
                            }
                        });
                    });
                    ui.separator();
                }
            });
        });
        match act {
            Some((t, true)) => self.restore(t),
            Some((t, false)) => self.purge(t),
            None => {}
        }
        if !open {
            self.trash_open = false;
        }
    }
}
