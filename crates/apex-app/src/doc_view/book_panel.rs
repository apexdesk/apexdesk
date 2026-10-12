//! The manuscript panel of a book: its chapters and scenes with their
//! words. Click to go, drag to reorder (the text moves), double-click to
//! rename; the menu of a row adds, renames and deletes.

use std::collections::HashSet;

use apex_doc::book::{BookNode, SceneInfo, SceneStatus};

use super::*;

/// What the panel lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Show {
    #[default]
    All,
    Chapters,
    Scenes,
}

#[derive(Default)]
pub(super) struct Panel {
    pub open: bool,
    pub show: Show,
    /// Chapters and scenes, for revision `rev`.
    nodes: Vec<BookNode>,
    rev: Option<u64>,
    /// Collapsed chapters (heading paragraphs; cleared when they move).
    collapsed: HashSet<usize>,
    /// The heading being renamed and its new title.
    rename: Option<(usize, String)>,
    focus_rename: bool,
    /// The data of each node's scene.
    infos: Vec<Option<SceneInfo>>,
    /// Filters of the scenes: status, point of view, tag.
    pub f_status: Option<SceneStatus>,
    pub f_pov: Option<String>,
    pub f_tag: Option<String>,
    /// The fields of the scene the caret is in, as typed.
    scene: Option<SceneEdit>,
    /// Headings picked with Ctrl+click, to be seen together.
    picked: Vec<usize>,
}

/// The fields of a scene being edited: they are saved when left.
struct SceneEdit {
    para: usize,
    rev: u64,
    synopsis: String,
    pov: String,
    tags: String,
    /// One of the fields has the focus.
    editing: bool,
}

/// Whether `tags` has `tag` or a tag under it ("place" has "place/city").
fn has_tag(tags: &[String], tag: &str) -> bool {
    tags.iter().any(|t| t == tag || t.strip_prefix(tag).is_some_and(|r| r.starts_with('/')))
}

/// The tags typed in a field: split at commas, trimmed, each once.
pub(super) fn parse_tags(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in s.split(',').map(|t| t.trim().trim_start_matches('#').trim()) {
        if !t.is_empty() && !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    }
    out
}

/// What a row asks for.
enum Act {
    Go(usize),
    Rename(usize),
    Delete(usize),
    Move(usize, usize),
    NewChapter(usize),
    NewScene(usize),
    ShowOnly(Vec<usize>),
}

impl Panel {
    fn refresh(&mut self, doc: &Document) {
        if self.rev == Some(doc.revision) {
            return;
        }
        let before: Vec<usize> = self.nodes.iter().map(|n| n.para).collect();
        self.nodes = doc.book_nodes();
        self.infos = self.nodes.iter().map(|n| if n.level == 2 { doc.scene_info(n.para).cloned() } else { None }).collect();
        if before != self.nodes.iter().map(|n| n.para).collect::<Vec<_>>() {
            self.collapsed.clear();
            self.picked.clear();
        }
        self.rev = Some(doc.revision);
    }

    /// The chapter (index in `nodes`) of node `i`.
    fn chapter_of(&self, i: usize) -> Option<usize> {
        self.nodes[..=i].iter().rposition(|n| n.level == 1)
    }

    fn filtered(&self) -> bool {
        self.f_status.is_some() || self.f_pov.is_some() || self.f_tag.is_some()
    }

    /// Whether scene `i` passes the filters.
    fn matches(&self, i: usize) -> bool {
        let info = self.infos.get(i).cloned().flatten().unwrap_or_default();
        self.f_status.is_none_or(|s| info.status == s) && self.f_pov.as_ref().is_none_or(|p| &info.pov == p) && self.f_tag.as_ref().is_none_or(|t| has_tag(&info.tags, t))
    }

    fn shown(&self, i: usize) -> bool {
        let n = &self.nodes[i];
        if n.level == 2 && self.show != Show::Chapters && !self.matches(i) {
            return false;
        }
        match self.show {
            Show::Chapters => n.level == 1,
            Show::Scenes => n.level == 2,
            Show::All => n.level == 1 || !self.chapter_of(i).is_some_and(|c| self.collapsed.contains(&self.nodes[c].para)),
        }
    }

    /// Where node `from` goes when dropped on node `on` (`after`: below
    /// the last row): the paragraph it goes before.
    fn drop_at(&self, from: usize, on: usize, after: bool, len: usize) -> usize {
        if after {
            return len;
        }
        let (f, t) = (&self.nodes[from], &self.nodes[on]);
        match (f.level, t.level) {
            // A chapter goes before the chapter of the row.
            (1, _) => self.chapter_of(on).map_or(t.para, |c| self.nodes[c].para),
            // A scene dropped on a chapter goes at its end.
            (_, 1) => t.end,
            _ => t.para,
        }
    }
}

impl DocView {
    pub(super) fn toggle_book_panel(&mut self) {
        self.book.panel.open = !self.book.panel.open;
    }

    /// The panel, on the left of the page.
    pub(super) fn book_panel(&mut self, ui: &mut egui::Ui) {
        if !self.is_book() || !self.book.panel.open {
            return;
        }
        let mut panel = std::mem::take(&mut self.book.panel);
        panel.refresh(&self.doc);
        let mut act = None;
        egui::Panel::left(Id::new(("doc_book", self.uid)))
            .resizable(true)
            .default_size(250.0)
            .min_size(170.0)
            .frame(egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::same(6)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong(tr("book.panel"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("×").on_hover_text(tr("dialog.close")).clicked() {
                            panel.open = false;
                        }
                    });
                });
                ui.horizontal(|ui| {
                    ui.label(tr("book.show"));
                    let name = |s: Show| match s {
                        Show::All => tr("book.show_all"),
                        Show::Chapters => tr("book.show_chapters"),
                        Show::Scenes => tr("book.show_scenes"),
                    };
                    egui::ComboBox::from_id_salt(("book_show", self.uid)).selected_text(name(panel.show)).show_ui(ui, |ui| {
                        for s in [Show::All, Show::Chapters, Show::Scenes] {
                            ui.selectable_value(&mut panel.show, s, name(s));
                        }
                    });
                });
                self.book_filters(ui, &mut panel);
                ui.separator();
                let details = self.book_scene_at(self.sel.cursor.para);
                let below = if details.is_some() { 250.0 } else { 36.0 } + 150.0;
                egui::ScrollArea::vertical().id_salt("book_rows").auto_shrink([false, false]).max_height((ui.available_height() - below).max(80.0)).show(ui, |ui| {
                    act = self.book_rows(ui, &mut panel);
                    self.book_orphans(ui);
                });
                ui.separator();
                if let Some(para) = details {
                    self.book_scene_details(ui, &mut panel, para);
                    ui.separator();
                } else {
                    panel.scene = None;
                }
                self.book_goals_foot(ui);
                ui.separator();
                if !panel.picked.is_empty() {
                    ui.horizontal(|ui| {
                        if ui.button(trf("book.show_together", &[("n", &panel.picked.len())])).on_hover_text(tr("book.show_together_tip")).clicked() {
                            act = Some(Act::ShowOnly(std::mem::take(&mut panel.picked)));
                        }
                        if ui.small_button("✕").on_hover_text(tr("book.unpick")).clicked() {
                            panel.picked.clear();
                        }
                    });
                } else if !self.scoped() && panel.nodes.len() > 1 {
                    ui.label(RichText::new(tr("book.pick_hint")).weak().small());
                }
                ui.horizontal(|ui| {
                    if ui.button(tr("book.new_chapter")).clicked() {
                        act = Some(Act::NewChapter(self.sel.cursor.para));
                    }
                    if ui.button(tr("book.new_scene")).clicked() {
                        act = Some(Act::NewScene(self.sel.cursor.para));
                    }
                });
            });
        if !panel.open {
            self.focus = true;
        }
        self.book.panel = panel;
        match act {
            // A part of the book not shown is shown instead.
            Some(Act::Go(p)) if self.scope_hidden().is_some_and(|h| h.get(p).copied().unwrap_or(false)) => self.show_only(&[p]),
            Some(Act::Go(p)) => self.go_to_para(p),
            Some(Act::ShowOnly(ps)) => self.show_only(&ps),
            Some(Act::Rename(p)) => {
                let title = self.book.panel.nodes.iter().find(|n| n.para == p).map(|n| n.title.clone()).unwrap_or_default();
                self.book.panel.rename = Some((p, title));
                self.book.panel.focus_rename = true;
            }
            Some(Act::Delete(p)) => {
                if let Some(s) = self.doc.delete_section(p, self.sel) {
                    self.set_sel(s);
                }
            }
            Some(Act::Move(from, to)) => match self.doc.move_section(from, to, self.sel) {
                Some(s) => self.set_sel(s),
                None => self.notices.push(Notice::info(tr("nav.cant_move"))),
            },
            Some(Act::NewChapter(p)) => {
                self.sel = Selection::caret(Pos::new(p, 0));
                self.new_chapter();
            }
            Some(Act::NewScene(p)) => {
                self.sel = Selection::caret(Pos::new(p, 0));
                self.new_scene();
            }
            None => {}
        }
    }

    /// The heading of the scene paragraph `para` is in.
    pub(super) fn book_scene_at(&self, para: usize) -> Option<usize> {
        self.doc.headings().into_iter().take_while(|&(i, _)| i <= para).filter(|&(_, l)| l <= 2).last().filter(|h| h.1 == 2).map(|h| h.0)
    }

    fn book_filters(&mut self, ui: &mut egui::Ui, panel: &mut Panel) {
        let infos: Vec<SceneInfo> = self.doc.book.as_ref().map(|b| b.scenes.clone()).unwrap_or_default();
        let mut povs: Vec<String> = infos.iter().map(|s| s.pov.clone()).filter(|p| !p.is_empty()).collect();
        povs.sort();
        povs.dedup();
        let mut tags: Vec<String> = infos.iter().flat_map(|s| s.tags.clone()).collect();
        tags.sort();
        tags.dedup();
        ui.horizontal_wrapped(|ui| {
            ui.label(tr("book.filter"));
            let any = tr("book.filter_any");
            egui::ComboBox::from_id_salt(("book_f_status", self.uid))
                .width(90.0)
                .selected_text(panel.f_status.map_or(any, super::book_ui::status_name))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut panel.f_status, None, any);
                    for st in SceneStatus::ALL {
                        ui.selectable_value(&mut panel.f_status, Some(st), super::book_ui::status_name(st));
                    }
                })
                .response
                .on_hover_text(tr("book.status"));
            if !povs.is_empty() {
                egui::ComboBox::from_id_salt(("book_f_pov", self.uid))
                    .width(80.0)
                    .selected_text(panel.f_pov.clone().unwrap_or_else(|| tr("book.pov").to_string()))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut panel.f_pov, None, any);
                        for p in povs {
                            ui.selectable_value(&mut panel.f_pov, Some(p.clone()), p);
                        }
                    });
            }
            if !tags.is_empty() {
                egui::ComboBox::from_id_salt(("book_f_tag", self.uid))
                    .width(80.0)
                    .selected_text(panel.f_tag.as_ref().map_or_else(|| tr("book.tags").to_string(), |t| format!("#{t}")))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut panel.f_tag, None, any);
                        for t in tags {
                            ui.selectable_value(&mut panel.f_tag, Some(t.clone()), format!("#{t}"));
                        }
                    });
            }
            if panel.filtered() && ui.small_button("×").on_hover_text(tr("book.filter_clear")).clicked() {
                panel.f_status = None;
                panel.f_pov = None;
                panel.f_tag = None;
            }
        });
    }

    /// Status, point of view, tags and synopsis of the scene of heading
    /// `para`; typed fields are saved when left.
    fn book_scene_details(&mut self, ui: &mut egui::Ui, panel: &mut Panel, para: usize) {
        let info = self.doc.scene_info(para).cloned().unwrap_or_default();
        let rev = self.doc.revision;
        let stale = panel.scene.as_ref().is_none_or(|e| e.para != para || (e.rev != rev && !e.editing));
        if stale {
            panel.scene = Some(SceneEdit { para, rev, synopsis: info.synopsis.clone(), pov: info.pov.clone(), tags: info.tags.join(", "), editing: false });
        }
        let Some(e) = panel.scene.as_mut() else { return };
        e.rev = rev;
        let title = self.doc.paragraphs.get(para).map(|p| p.text.clone()).unwrap_or_default();
        ui.strong(RichText::new(title).size(13.0));
        let mut status = info.status;
        let mut editing = false;
        egui::Grid::new(("book_scene", self.uid)).num_columns(2).spacing([6.0, 4.0]).show(ui, |ui| {
            ui.label(tr("book.status"));
            egui::ComboBox::from_id_salt(("book_status", self.uid)).selected_text(super::book_ui::status_name(status)).show_ui(ui, |ui| {
                for st in SceneStatus::ALL {
                    ui.selectable_value(&mut status, st, super::book_ui::status_name(st));
                }
            });
            ui.end_row();
            ui.label(tr("book.pov"));
            let r = ui.add(egui::TextEdit::singleline(&mut e.pov).desired_width(f32::INFINITY));
            editing |= r.has_focus();
            if r.lost_focus() && e.pov.trim() != info.pov {
                let v = e.pov.trim().to_string();
                self.doc.edit_scene(para, self.sel, |s| s.pov = v);
            }
            ui.end_row();
            ui.label(tr("book.tags"));
            let r = ui.add(egui::TextEdit::singleline(&mut e.tags).hint_text(tr("book.tags_hint")).desired_width(f32::INFINITY));
            editing |= r.has_focus();
            if r.lost_focus() && parse_tags(&e.tags) != info.tags {
                let v = parse_tags(&e.tags);
                self.doc.edit_scene(para, self.sel, |s| s.tags = v);
            }
            ui.end_row();
        });
        ui.label(tr("book.synopsis"));
        let r = ui.add(egui::TextEdit::multiline(&mut e.synopsis).desired_rows(4).desired_width(f32::INFINITY));
        e.editing = editing || r.has_focus();
        if r.lost_focus() && e.synopsis.trim_end() != info.synopsis {
            let v = e.synopsis.trim_end().to_string();
            self.doc.edit_scene(para, self.sel, |s| s.synopsis = v);
        }
        if status != info.status {
            self.doc.edit_scene(para, self.sel, |s| s.status = status);
        }
    }

    /// Scene data whose heading is gone: give it to the current scene or
    /// delete it.
    fn book_orphans(&mut self, ui: &mut egui::Ui) {
        let orphans = self.doc.orphan_scenes();
        if orphans.is_empty() {
            return;
        }
        ui.add_space(6.0);
        let here = self.book_scene_at(self.sel.cursor.para);
        egui::CollapsingHeader::new(trf("book.orphans", &[("n", &orphans.len())])).id_salt(("book_orphans", self.uid)).show(ui, |ui| {
            ui.label(RichText::new(tr("book.orphans_tip")).weak().small());
            for o in orphans {
                let name = if o.synopsis.is_empty() { o.anchor.clone() } else { o.synopsis.chars().take(60).collect() };
                ui.horizontal(|ui| {
                    ui.painter().circle_filled(ui.cursor().min + vec2(4.0, 9.0), 3.5, super::book_ui::status_color(o.status));
                    ui.add_space(12.0);
                    ui.label(name);
                });
                ui.horizontal(|ui| {
                    if ui.add_enabled(here.is_some(), egui::Button::new(tr("book.attach")).small()).on_disabled_hover_text(tr("book.not_in_scene")).clicked() {
                        self.doc.place_orphan(&o.anchor, here, self.sel);
                    }
                    if ui.small_button(tr("book.discard")).clicked() {
                        self.doc.place_orphan(&o.anchor, None, self.sel);
                    }
                });
            }
        });
    }

    /// Gives heading `para` the title `title`, as one undo step.
    pub(super) fn rename_heading(&mut self, para: usize, title: &str) {
        let title = title.trim();
        let Some(p) = self.doc.paragraphs.get(para) else { return };
        if title.is_empty() || title == p.text {
            return;
        }
        let sel = Selection { anchor: Pos::new(para, 0), cursor: Pos::new(para, p.text.len()) };
        self.doc.insert_text(sel, title, None);
    }

    fn book_rows(&mut self, ui: &mut egui::Ui, panel: &mut Panel) -> Option<Act> {
        if panel.nodes.is_empty() {
            ui.label(RichText::new(tr("book.empty")).weak());
            return None;
        }
        let mut act = None;
        let cur = self.sel.cursor.para;
        let current = panel.nodes.iter().rposition(|n| n.para <= cur);
        let dragging = egui::DragAndDrop::has_payload_of_type::<usize>(ui.ctx());
        let last_shown = (0..panel.nodes.len()).rev().find(|&i| panel.shown(i));
        let row_h = 22.0;
        for i in 0..panel.nodes.len() {
            if !panel.shown(i) {
                continue;
            }
            let n = panel.nodes[i].clone();
            let indent = if n.level == 2 && panel.show == Show::All { 18.0 } else { 4.0 };
            if let Some((p, title)) = &mut panel.rename
                && *p == n.para
            {
                let r = ui.horizontal(|ui| {
                    ui.add_space(indent);
                    ui.add(egui::TextEdit::singleline(title).desired_width(f32::INFINITY))
                });
                let r = r.inner;
                if std::mem::take(&mut panel.focus_rename) {
                    r.request_focus();
                }
                if r.lost_focus() {
                    let (p, title) = panel.rename.take().unwrap_or_default();
                    if !ui.input(|i| i.key_pressed(Key::Escape)) {
                        self.rename_heading(p, &title);
                    }
                }
                continue;
            }
            let next = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), row_h));
            if !ui.is_rect_visible(next) {
                ui.allocate_space(next.size());
                continue;
            }
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click_and_drag());
            let vis = ui.visuals();
            if panel.picked.contains(&n.para) {
                ui.painter().rect_stroke(rect, 3.0, Stroke::new(1.5, vis.selection.stroke.color), egui::StrokeKind::Inside);
            }
            if Some(i) == current {
                ui.painter().rect_filled(rect, 3.0, vis.selection.bg_fill.gamma_multiply(0.35));
            } else if resp.hovered() {
                ui.painter().rect_filled(rect, 3.0, vis.widgets.hovered.weak_bg_fill);
            }
            let has_scenes = n.level == 1 && panel.nodes.get(i + 1).is_some_and(|x| x.level == 2);
            let collapsed = panel.collapsed.contains(&n.para);
            let tri = Rect::from_center_size(pos2(rect.min.x + indent + 5.0, rect.center().y), vec2(10.0, 10.0));
            if has_scenes && panel.show == Show::All {
                let pts = if collapsed {
                    vec![tri.left_top() + vec2(2.0, 0.0), tri.right_center(), tri.left_bottom() + vec2(2.0, 0.0)]
                } else {
                    vec![tri.left_top() + vec2(0.0, 2.0), tri.right_top() + vec2(0.0, 2.0), tri.center_bottom()]
                };
                ui.painter().add(egui::Shape::convex_polygon(pts, vis.text_color().gamma_multiply(0.7), Stroke::NONE));
            }
            if n.level == 2 {
                let st = panel.infos.get(i).cloned().flatten().unwrap_or_default().status;
                let c = super::book_ui::status_color(st);
                let at = if has_scenes && panel.show == Show::All { tri.center() } else { pos2(tri.center().x, rect.center().y) };
                ui.painter().circle_filled(at, 3.5, c);
            }
            let words = n.words.to_string();
            let wfont = egui::FontId::proportional(11.5);
            let wg = ui.painter().layout_no_wrap(words, wfont, vis.weak_text_color());
            let wpos = pos2(rect.max.x - wg.size().x - 4.0, rect.center().y - wg.size().y / 2.0);
            let font = egui::FontId::proportional(13.0);
            let color = if n.level == 1 { vis.strong_text_color() } else { vis.text_color() };
            let text_rect = Rect::from_min_max(pos2(tri.max.x + 4.0, rect.min.y), pos2(wpos.x - 6.0, rect.max.y));
            let g = ui.painter().layout_no_wrap(n.title.clone(), font, color);
            let clip = ui.painter().with_clip_rect(text_rect.intersect(ui.clip_rect()));
            clip.galley(pos2(text_rect.min.x, rect.center().y - g.size().y / 2.0), g, color);
            ui.painter().galley(wpos, wg, vis.weak_text_color());
            let resp = resp.on_hover_text(trf("doc.words", &[("n", &n.words)]));
            let on_tri = resp.interact_pointer_pos().is_some_and(|p| p.x < tri.max.x + 2.0);
            if resp.double_clicked() {
                act = Some(Act::Rename(n.para));
            } else if resp.clicked() && ui.input(|i| i.modifiers.command) {
                if let Some(k) = panel.picked.iter().position(|&p| p == n.para) {
                    panel.picked.remove(k);
                } else {
                    panel.picked.push(n.para);
                }
            } else if resp.clicked() {
                if has_scenes && on_tri && panel.show == Show::All {
                    if !panel.collapsed.remove(&n.para) {
                        panel.collapsed.insert(n.para);
                    }
                } else {
                    act = Some(Act::Go(n.para));
                }
            }
            resp.context_menu(|ui| {
                if ui.button(tr("book.show_only")).clicked() {
                    act = Some(Act::ShowOnly(vec![n.para]));
                }
                ui.separator();
                if ui.button(tr("book.rename")).clicked() {
                    act = Some(Act::Rename(n.para));
                }
                if ui.button(tr("book.new_chapter")).clicked() {
                    act = Some(Act::NewChapter(n.para));
                }
                if ui.button(tr("book.new_scene")).clicked() {
                    act = Some(Act::NewScene(n.para));
                }
                ui.separator();
                let del = if n.level == 1 { tr("book.delete_chapter") } else { tr("book.delete_scene") };
                if ui.button(del).clicked() {
                    act = Some(Act::Delete(n.para));
                }
            });
            resp.dnd_set_drag_payload(i);
            if dragging && let Some(p) = ui.ctx().pointer_interact_pos() && rect.contains(p) {
                let after = Some(i) == last_shown && p.y > rect.center().y;
                let y = if after { rect.max.y } else { rect.min.y };
                ui.painter().hline(rect.x_range(), y, Stroke::new(2.0, vis.selection.stroke.color));
                if let Some(from) = resp.dnd_release_payload::<usize>() {
                    let to = panel.drop_at(*from, i, after, self.doc.paragraphs.len());
                    act = Some(Act::Move(panel.nodes[*from].para, to));
                }
            }
        }
        act
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heading(text: &str, level: u8) -> Paragraph {
        let mut p = Paragraph::new(text);
        p.style.kind = ParaKind::Heading(level);
        p
    }

    fn titles(v: &DocView) -> Vec<String> {
        v.doc.book_nodes().into_iter().map(|n| n.title).collect()
    }

    #[test]
    fn the_panel_moves_renames_and_filters() {
        let doc = Document::from_paragraphs(vec![heading("Uno", 1), heading("A", 2), Paragraph::new("a"), heading("B", 2), Paragraph::new("b"), heading("Dos", 1), heading("C", 2), Paragraph::new("c")]);
        let mut v = DocView::new(doc);
        let mut p = Panel::default();
        p.refresh(&v.doc);
        // Scene C onto scene A: before it.
        assert_eq!(p.drop_at(4, 1, false, 8), 1);
        // Scene A onto chapter Dos: at its end.
        assert_eq!(p.drop_at(1, 3, false, 8), 8);
        // Chapter Dos onto scene B: before chapter Uno.
        assert_eq!(p.drop_at(3, 2, false, 8), 0);
        let s = v.doc.move_section(5, p.drop_at(3, 2, false, 8), v.sel).unwrap();
        v.set_sel(s);
        assert_eq!(titles(&v), ["Dos", "C", "Uno", "A", "B"]);
        v.rename_heading(0, "  Prólogo ");
        assert_eq!(titles(&v)[0], "Prólogo");
        p.refresh(&v.doc);
        p.show = Show::Chapters;
        assert_eq!((0..5).filter(|&i| p.shown(i)).count(), 2);
        p.show = Show::Scenes;
        assert_eq!((0..5).filter(|&i| p.shown(i)).count(), 3);
        p.show = Show::All;
        p.collapsed.insert(0);
        assert_eq!((0..5).filter(|&i| p.shown(i)).count(), 4);
    }

    #[test]
    fn scenes_are_filtered_by_their_data() {
        let doc = Document::from_paragraphs(vec![heading("Uno", 1), heading("A", 2), heading("B", 2), heading("C", 2)]);
        let mut v = DocView::new(doc);
        v.make_book();
        v.doc.edit_scene(1, v.sel, |s| {
            s.status = SceneStatus::Done;
            s.tags = parse_tags("lugar/ciudad, #viaje, viaje");
        });
        v.doc.edit_scene(2, v.sel, |s| s.pov = "Ana".into());
        assert_eq!(v.doc.scene_info(1).unwrap().tags, ["lugar/ciudad", "viaje"]);
        let mut p = Panel::default();
        p.refresh(&v.doc);
        let shown = |p: &Panel| (0..4).filter(|&i| p.shown(i)).map(|i| p.nodes[i].title.clone()).collect::<Vec<_>>();
        p.f_status = Some(SceneStatus::Done);
        assert_eq!(shown(&p), ["Uno", "A"]);
        // Scenes without data are drafts.
        p.f_status = Some(SceneStatus::Draft);
        assert_eq!(shown(&p), ["Uno", "B", "C"]);
        p.f_status = None;
        p.f_tag = Some("lugar".into());
        assert_eq!(shown(&p), ["Uno", "A"]);
        p.f_tag = None;
        p.f_pov = Some("Ana".into());
        assert_eq!(shown(&p), ["Uno", "B"]);
        assert_eq!(v.book_scene_at(0), None);
        assert_eq!(v.book_scene_at(3), Some(3));
    }
}
