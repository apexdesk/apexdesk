//! The table: the pages of a type (or any pages) as rows and their
//! fields as columns, each cell edited in place.

use std::collections::HashMap;

use apex_doc::notebook::fold;
use apex_doc::notebook::query::Query;
use apex_doc::notebook::types::number;

use super::types::{line_edit, value_edit};
use super::*;

/// What a column shows.
#[derive(Clone, PartialEq)]
enum Col {
    Title,
    Field(String),
    Type,
    Section,
    Modified,
}

#[derive(Default)]
pub(super) struct TablePanel {
    /// The pages shown.
    pub query: Query,
    /// The column sorted by, and whether downwards.
    sort: Option<(Col, bool)>,
    /// The filters show.
    pub filters: bool,
    /// The name being given to save the query.
    pub naming: Option<String>,
}

/// The date (yyyy-mm-dd) of a time in seconds, here.
pub(super) fn day(secs: i64) -> String {
    date_text(secs).chars().take(10).collect()
}

/// Orders cell texts: numbers as numbers, the rest alphabetically.
fn cmp_cells(a: &str, b: &str) -> std::cmp::Ordering {
    match (number(a), number(b)) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        _ => fold(a).cmp(&fold(b)),
    }
}

impl NoteView {
    /// Shows the table of type `ty` (None: the open page's type, or, for
    /// a page with none, every page of its section).
    pub fn open_table(&mut self, ty: Option<i64>) {
        self.close_graph();
        self.flush();
        let ptype = ty.or_else(|| self.page.and_then(|p| self.nb.page(p)).and_then(|p| p.ptype));
        let section = if ptype.is_none() { self.section } else { None };
        self.table = Some(TablePanel { query: Query { ptype, section, ..Query::default() }, ..TablePanel::default() });
    }

    /// A cell's text, to sort by.
    fn cell_text(&self, id: i64, col: &Col) -> String {
        let Some(p) = self.nb.page(id) else { return String::new() };
        match col {
            Col::Title => p.title.clone(),
            Col::Field(f) => self.nb.field_value(id, f).to_string(),
            Col::Type => p.ptype.and_then(|t| self.nb.page_type(t)).map(|t| t.name.clone()).unwrap_or_default(),
            Col::Section => self.nb.section(p.section).map(|s| s.name.clone()).unwrap_or_default(),
            Col::Modified => day(p.modified),
        }
    }

    /// A new row: a page of the table's type, in its section (or the one
    /// shown).
    pub(super) fn add_row(&mut self, q: &Query) {
        let Some(s) = q.section.or(self.section).or_else(|| self.nb.live_sections().first().map(|s| s.id)) else { return };
        let title = tr("nb.untitled").to_string();
        let ty = q.ptype;
        self.change(|nb| {
            let id = nb.add_page(s, None, None, &title, now());
            if let Some(p) = nb.page_mut(id) {
                p.ptype = ty;
            }
        });
    }

    pub(super) fn table_ui(&mut self, ui: &mut egui::Ui) {
        let Some(mut panel) = self.table.take() else { return };
        let mut close = false;
        let mut open = None;
        let mut rename = None;
        let mut set = None;
        let mut add = false;
        let fields: Vec<apex_doc::notebook::Field> = panel.query.ptype.and_then(|t| self.nb.page_type(t)).map(|t| t.fields.clone()).unwrap_or_default();
        let mut cols = vec![Col::Title];
        if panel.query.ptype.is_some() {
            cols.extend(fields.iter().map(|f| Col::Field(f.name.clone())));
        } else {
            cols.push(Col::Type);
        }
        cols.extend([Col::Section, Col::Modified]);
        let mut rows = self.nb.run_query(&panel.query, &day);
        if let Some((col, down)) = &panel.sort {
            let texts: HashMap<i64, String> = rows.iter().map(|&id| (id, self.cell_text(id, col))).collect();
            rows.sort_by(|a, b| cmp_cells(&texts[a], &texts[b]));
            if *down {
                rows.reverse();
            }
        }
        egui::Frame::new().inner_margin(egui::Margin::symmetric(24, 12)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading(tr("nb.table"));
                ui.add_space(16.0);
                let name = panel.query.ptype.and_then(|t| self.nb.page_type(t)).map_or_else(|| tr("nb.all_pages").to_string(), |t| t.name.clone());
                egui::ComboBox::from_id_salt(("note_table_type", self.uid)).selected_text(name).show_ui(ui, |ui| {
                    ui.selectable_value(&mut panel.query.ptype, None, tr("nb.all_pages"));
                    for t in &self.nb.types {
                        ui.selectable_value(&mut panel.query.ptype, Some(t.id), &t.name);
                    }
                });
                let name = panel.query.section.and_then(|s| self.nb.section(s)).map_or_else(|| tr("nb.all_sections").to_string(), |s| s.name.clone());
                egui::ComboBox::from_id_salt(("note_table_section", self.uid)).selected_text(name).show_ui(ui, |ui| {
                    ui.selectable_value(&mut panel.query.section, None, tr("nb.all_sections"));
                    for s in self.nb.live_sections() {
                        ui.selectable_value(&mut panel.query.section, Some(s.id), &s.name);
                    }
                });
                let n = panel.query.active_filters();
                let label = if n > 0 { format!("{} ({n})", tr("nb.filters")) } else { tr("nb.filters").to_string() };
                if ui.selectable_label(panel.filters, label).clicked() {
                    panel.filters = !panel.filters;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("nb.graph_close")).clicked() {
                        close = true;
                    }
                    self.saved_queries_ui(ui, &mut panel);
                });
            });
            if panel.filters {
                self.filters_ui(ui, &mut panel);
            }
            ui.add_space(8.0);
            egui::ScrollArea::both().id_salt(("note_table", self.uid)).auto_shrink([false, false]).show(ui, |ui| {
                egui::Grid::new(("note_table_grid", self.uid, panel.query.ptype)).striped(true).num_columns(cols.len()).spacing(vec2(14.0, 6.0)).show(ui, |ui| {
                    for col in &cols {
                        let name = match col {
                            Col::Title => tr("nb.col_title").to_string(),
                            Col::Field(f) => f.clone(),
                            Col::Type => tr("nb.page_type").to_string(),
                            Col::Section => tr("nb.col_section").to_string(),
                            Col::Modified => tr("nb.col_modified").to_string(),
                        };
                        let arrow = match &panel.sort {
                            Some((c, down)) if c == col => if *down { " ⏷" } else { " ⏶" },
                            _ => "",
                        };
                        if ui.add(egui::Button::new(RichText::new(format!("{name}{arrow}")).strong()).frame(false)).on_hover_text(tr("nb.sort_tip")).clicked() {
                            panel.sort = match &panel.sort {
                                Some((c, false)) if c == col => Some((col.clone(), true)),
                                Some((c, true)) if c == col => None,
                                _ => Some((col.clone(), false)),
                            };
                        }
                    }
                    ui.end_row();
                    for &id in &rows {
                        let Some(p) = self.nb.page(id) else { continue };
                        for col in &cols {
                            match col {
                                Col::Title => {
                                    ui.horizontal(|ui| {
                                        if let Some(t) = line_edit(ui, Id::new(("note_table_title", self.uid, id)), &p.title, tr("nb.untitled"), 180.0) {
                                            rename = Some((id, t));
                                        }
                                        if ui.small_button("↗").on_hover_text(tr("nb.open_page")).clicked() {
                                            open = Some(id);
                                        }
                                    });
                                }
                                Col::Field(name) => {
                                    let f = fields.iter().find(|f| &f.name == name).expect("a column per field");
                                    let value = self.nb.field_value(id, name);
                                    if let Some(v) = value_edit(ui, Id::new(("note_table_cell", self.uid, id, name)), f, value, 130.0) {
                                        set = Some((id, name.clone(), v));
                                    }
                                }
                                _ => {
                                    ui.label(RichText::new(self.cell_text(id, col)).weak());
                                }
                            }
                        }
                        ui.end_row();
                    }
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let count = if rows.len() == 1 { tr("nb.table_count_one").to_string() } else { trf("nb.table_count", &[("n", &rows.len())]) };
                    ui.label(RichText::new(count).weak());
                    if ui.button(format!("+ {}", tr("nb.table_add"))).clicked() {
                        add = true;
                    }
                });
            });
        });
        if let Some((id, t)) = rename {
            self.rename(Naming::Page(id), &t);
            if self.page == Some(id) {
                self.title = t.trim().to_string();
            }
        }
        if let Some((id, name, v)) = set {
            self.change(|nb| nb.set_field(id, &name, &v));
        }
        if add {
            self.add_row(&panel.query);
        }
        if !close {
            self.table = Some(panel);
        }
        if let Some(p) = open {
            self.table = None;
            self.open_page(p);
            self.grid = false;
        }
    }
}
