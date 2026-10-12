//! The tasks panel: every check box of the notebook, late, for today,
//! coming, without a date and done, each with a jump to its page. Or as a
//! board: cards in columns the user names, dragged from one to another;
//! the last column is for the tasks done.

use apex_doc::notebook::tasks::{Task, TaskMeta, column_name, column_of};
use apex_doc::{CHECKED, ControlKind, UNCHECKED};

use super::*;
use crate::doc_view::tasks_ui::{priority_name, task_label, today};

#[derive(Default)]
pub(super) struct TaskPanel {
    /// Only this section's tasks.
    section: Option<i64>,
    show_done: bool,
    board: bool,
    /// The columns being edited: each one's name before, and now.
    cols: Option<Vec<(Option<String>, String)>>,
}

/// Where the board's columns are kept, in the notebook's settings.
const COLUMNS_KEY: &str = "task_columns";
const VIEW_KEY: &str = "tasks_view";
const COLUMN_W: f32 = 270.0;

/// A card being dragged: its page and paragraph.
#[derive(Clone, Copy)]
struct TaskDrag(i64, usize);

/// The groups the panel shows, in order.
#[derive(Clone, Copy, PartialEq)]
enum When {
    Late,
    Today,
    Soon,
    Someday,
    Done,
}

impl When {
    const ALL: [When; 5] = [When::Late, When::Today, When::Soon, When::Someday, When::Done];

    fn of(t: &Task, today: &str) -> When {
        match t.meta.due.as_deref() {
            _ if t.done => When::Done,
            Some(d) if d < today => When::Late,
            Some(d) if d == today => When::Today,
            Some(_) => When::Soon,
            None => When::Someday,
        }
    }

    fn name(self) -> &'static str {
        match self {
            When::Late => "nb.tasks_late",
            When::Today => "nb.tasks_today",
            When::Soon => "nb.tasks_soon",
            When::Someday => "nb.tasks_someday",
            When::Done => "nb.tasks_done",
        }
    }
}

impl NoteView {
    pub fn open_tasks(&mut self) {
        self.flush();
        let tasks = self.tasks.take();
        self.close_graph();
        self.tasks = tasks;
        if self.tasks.is_none() {
            let board = self.store.setting(VIEW_KEY).as_deref() == Some("board");
            self.tasks = Some(TaskPanel { board, ..Default::default() });
        }
    }

    /// Tasks not done that are late or for today.
    pub fn tasks_due(&self) -> usize {
        let today = today();
        self.nb.all_tasks().iter().filter(|(_, t)| matches!(When::of(t, &today), When::Late | When::Today)).count()
    }

    /// Checks or unchecks the task of paragraph `para` of page `page`.
    pub(super) fn toggle_task(&mut self, page: i64, para: usize) {
        self.change_task(page, para, |_, done| *done = !*done);
    }

    /// Changes the box of the task of paragraph `para` of page `page`: its
    /// tag and whether it is checked.
    fn change_task(&mut self, page: i64, para: usize, f: impl FnOnce(&mut TaskMeta, &mut bool)) {
        self.edit_page(page, |d| {
            let Some(at) = d.paragraphs.get(para).and_then(|p| apex_doc::para_controls(p, para).into_iter().find(|c| matches!(c.control.kind, ControlKind::CheckBox { .. }))) else { return false };
            let mut checked = matches!(at.control.kind, ControlKind::CheckBox { checked: true });
            let mut meta = TaskMeta::from_tag(&at.control.tag);
            f(&mut meta, &mut checked);
            let c = apex_doc::ContentControl { kind: ControlKind::CheckBox { checked }, tag: meta.tag(), ..(*at.control).clone() };
            let sel = apex_doc::Selection::caret(apex_doc::Pos::new(para, 0));
            d.set_control(&at, c, &(if checked { CHECKED } else { UNCHECKED }).to_string(), sel);
            true
        });
        if self.page == Some(page) {
            self.flush();
        }
    }

    /// The board's columns: the ones the user named, or To do, Doing, Done.
    pub(super) fn task_columns(&self) -> Vec<String> {
        let cols: Vec<String> = self.store.setting(COLUMNS_KEY).map(|v| v.lines().map(column_name).filter(|c| !c.is_empty()).collect()).unwrap_or_default();
        if cols.len() >= 2 {
            return cols;
        }
        ["nb.col_todo", "nb.col_doing", "nb.col_done"].map(|k| tr(k).to_string()).to_vec()
    }

    pub(super) fn set_task_columns(&mut self, cols: &[String]) {
        let r = self.store.set_setting(COLUMNS_KEY, &cols.join("\n"));
        self.report(r);
    }

    /// Puts a task in column `col`: the last one checks it, any other
    /// unchecks it.
    pub(super) fn move_task(&mut self, page: i64, para: usize, col: usize, cols: &[String]) {
        // the columns are kept by name: keep the default ones in the
        // language they were first used in
        if self.store.setting(COLUMNS_KEY).is_none() {
            self.set_task_columns(cols);
        }
        let done = col + 1 == cols.len();
        let name = cols[col].clone();
        self.change_task(page, para, |meta, checked| {
            *checked = done;
            // a task done keeps the column it was in, to go back to it
            if !done {
                meta.col = Some(name);
            }
        });
    }

    /// Saves the columns edited; the tasks of a column renamed follow it.
    pub(super) fn apply_columns(&mut self, edited: &[(Option<String>, String)]) -> bool {
        let cols: Vec<String> = edited.iter().map(|(_, n)| column_name(n)).collect();
        if cols.len() < 2 || cols.iter().any(String::is_empty) || (1..cols.len()).any(|i| cols[..i].contains(&cols[i])) {
            self.notices.push(Notice::info(tr("nb.col_invalid")));
            return false;
        }
        for ((old, _), new) in edited.iter().zip(&cols) {
            let Some(old) = old.as_ref().filter(|o| *o != new) else { continue };
            let moved: Vec<(i64, usize)> = self.nb.all_tasks().iter().filter(|(_, t)| t.meta.col.as_ref() == Some(old)).map(|(p, t)| (*p, t.para)).collect();
            for (page, para) in moved {
                self.change_task(page, para, |meta, _| meta.col = Some(new.clone()));
            }
        }
        self.set_task_columns(&cols);
        true
    }

    pub(super) fn tasks_ui(&mut self, ui: &mut egui::Ui) {
        let Some(mut panel) = self.tasks.take() else { return };
        let today = today();
        let mut close = false;
        let mut toggle = None;
        let mut open = None;
        let mut moved = None;
        let mut view_changed = false;
        let mut apply_cols = None;
        let cols = self.task_columns();
        egui::Frame::new().inner_margin(egui::Margin::symmetric(24, 12)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading(tr("nb.tasks"));
                ui.add_space(16.0);
                let name = panel.section.and_then(|s| self.nb.section(s)).map_or_else(|| tr("nb.all_sections").to_string(), |s| s.name.clone());
                egui::ComboBox::from_id_salt(("note_tasks_section", self.uid)).selected_text(name).show_ui(ui, |ui| {
                    ui.selectable_value(&mut panel.section, None, tr("nb.all_sections"));
                    for s in self.nb.live_sections() {
                        ui.selectable_value(&mut panel.section, Some(s.id), &s.name);
                    }
                });
                ui.add_space(8.0);
                let was = panel.board;
                ui.selectable_value(&mut panel.board, false, tr("nb.tasks_list"));
                ui.selectable_value(&mut panel.board, true, tr("nb.tasks_board")).on_hover_text(tr("nb.tasks_board_tip"));
                if was != panel.board {
                    view_changed = true;
                }
                ui.add_space(8.0);
                if panel.board {
                    if ui.button(tr("nb.tasks_columns")).on_hover_text(tr("nb.tasks_columns_tip")).clicked() {
                        panel.cols = match panel.cols {
                            Some(_) => None,
                            None => Some(cols.iter().map(|c| (Some(c.clone()), c.clone())).collect()),
                        };
                    }
                } else {
                    ui.checkbox(&mut panel.show_done, tr("nb.tasks_show_done"));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("nb.graph_close")).clicked() {
                        close = true;
                    }
                });
            });
            ui.add_space(6.0);
            if panel.board && let Some(edit) = &mut panel.cols {
                if columns_editor(ui, edit) {
                    apply_cols = Some(edit.clone());
                }
                ui.add_space(6.0);
            }
            let all: Vec<(i64, &Task)> = self.nb.all_tasks().into_iter().filter(|(p, _)| panel.section.is_none_or(|s| self.nb.page(*p).is_some_and(|p| p.section == s))).collect();
            if all.is_empty() {
                ui.label(RichText::new(tr("nb.tasks_none")).weak());
                return;
            }
            if panel.board {
                moved = self.task_board(ui, &all, &cols, &today, &mut toggle, &mut open);
                return;
            }
            egui::ScrollArea::vertical().id_salt(("note_tasks", self.uid)).auto_shrink([false, false]).show(ui, |ui| {
                for when in When::ALL {
                    if when == When::Done && !panel.show_done {
                        continue;
                    }
                    let mut group: Vec<&(i64, &Task)> = all.iter().filter(|(_, t)| When::of(t, &today) == when).collect();
                    if group.is_empty() {
                        continue;
                    }
                    group.sort_by(|a, b| b.1.meta.priority.cmp(&a.1.meta.priority).then_with(|| a.1.meta.due.cmp(&b.1.meta.due)));
                    ui.add_space(8.0);
                    ui.label(RichText::new(format!("{} ({})", tr(when.name()), group.len())).strong());
                    ui.separator();
                    for (page, t) in group {
                        ui.horizontal(|ui| {
                            let mut done = t.done;
                            if ui.checkbox(&mut done, "").changed() {
                                toggle = Some((*page, t.para));
                            }
                            let text = if t.text.is_empty() { tr("nb.tasks_untitled").to_string() } else { t.text.clone() };
                            let text = if t.done { RichText::new(text).strikethrough().weak() } else { RichText::new(text) };
                            ui.label(text);
                            if let Some((label, color)) = task_label(&t.meta, t.done, &today) {
                                ui.label(RichText::new(label).small().color(color)).on_hover_text(priority_name(t.meta.priority));
                            }
                            let title = self.nb.page(*page).map(|p| p.title.clone()).unwrap_or_default();
                            if ui.link(RichText::new(title).small()).clicked() {
                                open = Some((*page, t.para));
                            }
                        });
                    }
                }
            });
        });
        if view_changed {
            let r = self.store.set_setting(VIEW_KEY, if panel.board { "board" } else { "list" });
            self.report(r);
        }
        if let Some(edit) = apply_cols
            && self.apply_columns(&edit)
        {
            panel.cols = None;
        }
        if !close {
            self.tasks = Some(panel);
        }
        if let Some((page, para, col)) = moved {
            self.move_task(page, para, col, &cols);
        }
        if let Some((page, para)) = toggle {
            self.toggle_task(page, para);
        }
        if let Some((page, para)) = open {
            self.tasks = None;
            self.open_page(page);
            self.grid = false;
            if let Some(ed) = &mut self.editor {
                ed.go_to_para(para);
            }
        }
    }
}

impl NoteView {
    /// The tasks as cards in columns. Returns a card dropped on a column:
    /// (page, paragraph, column).
    fn task_board(&self, ui: &mut egui::Ui, all: &[(i64, &Task)], cols: &[String], today: &str, toggle: &mut Option<(i64, usize)>, open: &mut Option<(i64, usize)>) -> Option<(i64, usize, usize)> {
        let mut moved = None;
        let dark = ui.visuals().dark_mode;
        let lane = if dark { Color32::from_gray(38) } else { Color32::from_gray(236) };
        let card = if dark { Color32::from_gray(56) } else { Color32::WHITE };
        egui::ScrollArea::horizontal().id_salt(("note_task_board", self.uid)).auto_shrink([false, false]).show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let height = ui.available_height();
                for (ci, name) in cols.iter().enumerate() {
                    let mut group: Vec<&(i64, &Task)> = all.iter().filter(|(_, t)| column_of(t, cols) == ci).collect();
                    group.sort_by(|a, b| b.1.meta.priority.cmp(&a.1.meta.priority).then_with(|| a.1.meta.due.cmp(&b.1.meta.due)));
                    let frame = egui::Frame::new().fill(lane).corner_radius(8.0).inner_margin(10.0);
                    let (_, dropped) = ui.dnd_drop_zone::<TaskDrag, ()>(frame, |ui| {
                        ui.vertical(|ui| {
                        ui.set_width(COLUMN_W);
                        ui.set_min_height(height - 24.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(name).strong());
                            ui.label(RichText::new(group.len().to_string()).weak());
                        });
                        ui.add_space(4.0);
                        egui::ScrollArea::vertical().id_salt(("note_task_lane", self.uid, ci)).auto_shrink([false, true]).show(ui, |ui| {
                            for (page, t) in group {
                                let id = Id::new(("note_task_card", self.uid, *page, t.para));
                                ui.dnd_drag_source(id, TaskDrag(*page, t.para), |ui| {
                                    egui::Frame::new().fill(card).corner_radius(6.0).inner_margin(8.0).stroke(ui.visuals().widgets.noninteractive.bg_stroke).show(ui, |ui| {
                                        ui.set_width(COLUMN_W - 16.0);
                                        ui.horizontal_wrapped(|ui| {
                                            let mut done = t.done;
                                            if ui.checkbox(&mut done, "").changed() {
                                                *toggle = Some((*page, t.para));
                                            }
                                            let text = if t.text.is_empty() { tr("nb.tasks_untitled").to_string() } else { t.text.clone() };
                                            // not selectable, so a drag on the text moves the card
                                            ui.add(egui::Label::new(if t.done { RichText::new(text).strikethrough().weak() } else { RichText::new(text) }).selectable(false));
                                        });
                                        ui.horizontal_wrapped(|ui| {
                                            if let Some((label, color)) = task_label(&t.meta, t.done, today) {
                                                ui.add(egui::Label::new(RichText::new(label).small().color(color)).selectable(false)).on_hover_text(priority_name(t.meta.priority));
                                            }
                                            let title = self.nb.page(*page).map(|p| p.title.clone()).unwrap_or_default();
                                            if ui.link(RichText::new(title).small()).clicked() {
                                                *open = Some((*page, t.para));
                                            }
                                        });
                                    });
                                });
                                ui.add_space(6.0);
                            }
                        });
                        });
                    });
                    if let Some(d) = dropped
                        && all.iter().any(|(p, t)| *p == d.0 && t.para == d.1 && column_of(t, cols) != ci)
                    {
                        moved = Some((d.0, d.1, ci));
                    }
                }
            });
        });
        moved
    }
}

/// The board's columns, to rename, add, order and remove. True when
/// they are to be applied.
fn columns_editor(ui: &mut egui::Ui, edit: &mut Vec<(Option<String>, String)>) -> bool {
    let mut apply = false;
    egui::Frame::group(ui.style()).fill(ui.visuals().faint_bg_color).show(ui, |ui| {
        let mut swap = None;
        let mut remove = None;
        ui.horizontal_wrapped(|ui| {
            let n = edit.len();
            for (i, (_, name)) in edit.iter_mut().enumerate() {
                ui.add(egui::TextEdit::singleline(name).desired_width(140.0));
                if ui.add_enabled(i > 0, egui::Button::new("◀").small()).on_hover_text(tr("nb.col_left")).clicked() {
                    swap = Some(i - 1);
                }
                if ui.add_enabled(i + 1 < n, egui::Button::new("▶").small()).on_hover_text(tr("nb.col_right")).clicked() {
                    swap = Some(i);
                }
                if ui.add_enabled(n > 2, egui::Button::new("×").small()).on_hover_text(tr("nb.col_remove")).clicked() {
                    remove = Some(i);
                }
                ui.add_space(10.0);
            }
            if ui.button(tr("nb.col_add")).clicked() {
                // before the last one: that one stays the column of the tasks done
                let at = edit.len().saturating_sub(1);
                edit.insert(at, (None, String::new()));
            }
        });
        if let Some(i) = swap {
            edit.swap(i, i + 1);
        }
        if let Some(i) = remove {
            edit.remove(i);
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr("nb.col_last_done")).weak().small());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("nb.col_apply")).clicked() {
                    apply = true;
                }
            });
        });
    });
    apply
}
