//! The notebook's toolstrip: the editor's tabs for the open page, and a
//! Notebook tab of its own (sections, pages, the trash).

use eframe::egui::Id;

use super::NoteView;
use crate::common::{self, Shell};
use crate::i18n::tr;
use crate::icons::Icon;
use crate::theme;
use crate::toolstrip::{self, Body, TabDef};

/// The editor's tabs a page has.
const EDITOR_TABS: [&str; 7] = ["home", "insert", "draw", "review", "picture", "shape", "table"];

impl NoteView {
    pub fn toolstrip_tabs(&self) -> Vec<TabDef> {
        // A canvas has no Home of its own: the Notebook tab is its tab.
        let mut v = vec![TabDef::new("notebook", "nb.tab")];
        if let Some(ed) = &self.editor {
            v.insert(0, TabDef::new("home", "toolstrip.home"));
            v.extend(ed.toolstrip_tabs().into_iter().filter(|t| t.id != "home" && EDITOR_TABS.contains(&t.id)));
        }
        v
    }

    pub fn toolstrip(&mut self, ui: &mut eframe::egui::Ui, tab: &str, shell: &mut Shell) {
        if tab == "notebook" || self.editor.is_none() {
            let id = Id::new(("note_toolstrip", tab));
            toolstrip::body(ui, id, theme::NOTE_ACCENT, |b| self.notebook_tab(b));
        } else if let Some(ed) = &mut self.editor {
            ed.toolstrip(ui, tab, shell);
        }
    }

    fn notebook_tab(&mut self, b: &mut Body) {
        let a = b.accent();
        // The page's own buttons, while it is in sight.
        let shown = !self.view_open();
        b.group("toolstrip.g.undo", Icon::Undo, |ui| {
            toolstrip::column(ui, |ui| {
                let undo = vec![tr("nb.undo_structure"); self.undo.len().min(20)];
                for _ in 0..common::history_button(ui, false, &undo) {
                    self.undo();
                }
                let redo = vec![tr("nb.undo_structure"); self.redo.len().min(20)];
                for _ in 0..common::history_button(ui, true, &redo) {
                    self.redo();
                }
            });
        });
        b.group("nb.g_add", Icon::New, |ui| {
            if toolstrip::large(ui, Icon::Doc, tr("nb.new_page"), tr("nb.new_page_tip"), a, false).clicked() {
                self.new_page(false);
            }
            toolstrip::column(ui, |ui| {
                if ui.add_enabled_ui(self.page.is_some(), |ui| toolstrip::small(ui, Some(Icon::Indent), tr("nb.new_subpage"), "", a, false)).inner.clicked() {
                    self.new_page(true);
                }
                if toolstrip::small(ui, Some(Icon::Folder), tr("nb.new_section"), "", a, false).clicked() {
                    self.new_section();
                }
                if ui.add_enabled_ui(shown && self.editor.is_some(), |ui| toolstrip::small(ui, Some(Icon::Checkbox), tr("nb.add_task"), tr("nb.add_task_tip"), a, false)).inner.clicked()
                    && let Some(ed) = &mut self.editor
                {
                    ed.insert_task();
                }
            });
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::Canvas), tr("nb.new_canvas"), tr("nb.new_canvas_tip"), a, false).clicked() {
                    self.new_canvas();
                }
                if ui.add_enabled_ui(shown && self.editor.is_some(), |ui| toolstrip::small(ui, Some(Icon::CrossRef), tr("nb.embed"), tr("nb.embed_tip"), a, false)).inner.clicked() {
                    self.finder = Some(super::links::Finder::new(super::links::Want::Embed(None)));
                }
                if toolstrip::small(ui, Some(Icon::Note), tr("nb.capture"), &format!("{} (Ctrl+Shift+I)", tr("nb.capture_tip")), a, false).clicked() {
                    self.capture_wanted = true;
                }
            });
        });
        b.group("nb.g_types", Icon::Template, |ui| {
            let types: Vec<(i64, String)> = self.nb.types.iter().map(|t| (t.id, t.name.clone())).collect();
            toolstrip::large_menu(ui, Icon::Template, tr("nb.new_typed"), tr("nb.new_typed_tip"), a, |ui| {
                if ui.button(tr("nb.source")).on_hover_text(tr("nb.new_source_tip")).clicked() {
                    self.new_source();
                    ui.close();
                }
                if ui.button(tr("nb.question")).on_hover_text(tr("nb.new_question_tip")).clicked() {
                    self.new_question();
                    ui.close();
                }
                ui.separator();
                for (id, name) in &types {
                    if ui.button(name).clicked() {
                        self.new_typed_page(*id);
                        ui.close();
                    }
                }
                if !types.is_empty() {
                    ui.separator();
                }
                if ui.button(format!("{}…", tr("nb.types"))).clicked() {
                    self.open_types(None);
                    ui.close();
                }
            });
            if toolstrip::large(ui, Icon::Field, tr("nb.types"), tr("nb.types_tip"), a, self.types_win.is_some()).clicked() {
                if self.types_win.is_some() {
                    self.types_win = None;
                } else {
                    self.open_types(None);
                }
            }
        });
        b.group("nb.g_views", Icon::Cards, |ui| {
            if toolstrip::large(ui, Icon::Table, tr("nb.table"), tr("nb.table_tip"), a, self.table.is_some()).clicked() {
                if self.table.is_some() {
                    self.table = None;
                } else {
                    self.open_table(None);
                }
            }
            if toolstrip::large(ui, Icon::ChartTimeline, tr("nb.timeline"), tr("nb.timeline_tip"), a, self.timeline.is_some()).clicked() {
                if self.timeline.is_some() {
                    self.timeline = None;
                } else {
                    self.open_timeline();
                }
            }
            if toolstrip::large(ui, Icon::Checkbox, tr("nb.tasks"), tr("nb.tasks_tip"), a, self.tasks.is_some()).clicked() {
                if self.tasks.is_some() {
                    self.tasks = None;
                } else {
                    self.open_tasks();
                }
            }
            if toolstrip::large(ui, Icon::Picture, tr("nb.gallery"), tr("nb.gallery_tip"), a, self.gallery.is_some()).clicked() {
                if self.gallery.is_some() {
                    self.gallery = None;
                } else {
                    self.open_gallery();
                }
            }
        });
        b.group("nb.g_find", Icon::Search, |ui| {
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::Doc), tr("nb.go_to"), &format!("{} (Ctrl+P)", tr("nb.go_to")), a, false).clicked() {
                    self.flush();
                    self.open_go_to();
                }
                if toolstrip::small(ui, Some(Icon::Calendar), tr("nb.today"), tr("nb.today_tip"), a, false).clicked() {
                    self.today();
                }
            });
        });
        b.group("nb.g_files", Icon::Object, |ui| {
            if ui.add_enabled_ui(shown && self.editor.is_some(), |ui| toolstrip::large(ui, Icon::Object, tr("nb.attach"), tr("nb.attach_tip"), a, false)).inner.clicked() {
                self.pick_attachment();
            }
            if toolstrip::large(ui, Icon::Folder, tr("nb.import_vault"), tr("nb.import_vault_tip"), a, false).clicked() {
                self.pick_vault();
            }
            toolstrip::large_menu(ui, Icon::Save, tr("nb.export"), tr("nb.export_tip"), a, |ui| self.export_menu(ui));
            let text_page = self.page.filter(|_| shown && self.editor.is_some());
            if ui.add_enabled_ui(text_page.is_some(), |ui| toolstrip::large(ui, Icon::TrackChanges, tr("nb.history"), tr("nb.history_tip"), a, self.history.is_some())).inner.clicked()
                && let Some(p) = text_page
            {
                self.open_history(p);
            }
        });
        b.group("nb.g_trash", Icon::Eraser, |ui| {
            let page = self.page.filter(|_| shown);
            if ui.add_enabled_ui(page.is_some(), |ui| toolstrip::large(ui, Icon::DeleteRows, tr("nb.delete_page"), "", a, false)).inner.clicked()
                && let Some(p) = page
            {
                self.delete_page(p);
            }
            if toolstrip::large(ui, Icon::Eraser, tr("nb.trash"), tr("nb.trash_tip"), a, self.trash_open).clicked() {
                let open = !self.trash_open;
                self.close_windows();
                self.trash_open = open;
            }
        });
    }

    /// The View tab's own groups.
    pub(crate) fn view_tab(&mut self, b: &mut Body) {
        let a = b.accent();
        let view = self.view_open();
        b.group("nb.g_view", Icon::Cards, |ui| {
            let cards = self.cards;
            let graph = self.graph.is_some();
            if toolstrip::large(ui, Icon::Bullets, tr("nb.list"), "", a, !cards && !view).clicked() {
                self.set_cards(false);
            }
            if toolstrip::large(ui, Icon::Cards, tr("nb.cards"), "", a, cards && !view).clicked() {
                self.set_cards(true);
            }
            if toolstrip::large(ui, Icon::Graph, tr("nb.graph"), tr("nb.graph_tip"), a, graph).clicked() {
                if graph {
                    self.close_graph();
                } else {
                    self.open_graph();
                }
            }
            let split = self.side.is_some();
            if ui.add_enabled_ui(!self.view_open() && (self.editor.is_some() || self.board.is_some() || split), |ui| toolstrip::large(ui, Icon::SideBySide, tr("nb.split"), tr("nb.split_tip"), a, split)).inner.clicked() {
                if split {
                    self.close_side();
                } else {
                    self.finder = Some(super::links::Finder::new(super::links::Want::Beside));
                }
            }
        });
        if let Some(ed) = self.editor.as_mut().filter(|_| !view) {
            b.group("toolstrip.g.zoom", Icon::ZoomIn, |ui| common::zoom_group(ui, &mut ed.zoom, a));
        }
    }
}
