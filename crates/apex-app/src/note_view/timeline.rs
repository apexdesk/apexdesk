//! The timeline: the pages along the dates in their fields (or when they
//! were made or last changed), by year and month.

use apex_doc::notebook::{tasks::is_date, FieldKind};

use super::table::day;
use super::*;

/// Which date places a page.
#[derive(Clone, Default, PartialEq)]
pub(super) enum When {
    /// Any date field it has (a page may be in several places).
    #[default]
    Fields,
    /// The date field of that name.
    Field(String),
    Created,
    Modified,
}

#[derive(Default)]
pub(super) struct Timeline {
    pub when: When,
    pub section: Option<i64>,
    /// The latest first.
    pub newest: bool,
}

/// A page on the timeline: its date (yyyy-mm-dd), the page, and the field
/// that dates it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Stop {
    pub date: String,
    pub page: i64,
    pub field: String,
}

impl NoteView {
    pub fn open_timeline(&mut self) {
        self.close_graph();
        self.flush();
        self.timeline = Some(Timeline::default());
    }

    /// The names of the date fields of every type, once each.
    fn date_fields(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for f in self.nb.types.iter().flat_map(|t| &t.fields).filter(|f| f.kind == FieldKind::Date) {
            if !v.contains(&f.name) {
                v.push(f.name.clone());
            }
        }
        v
    }

    /// The stops of timeline `t`, in time order.
    pub(super) fn stops(&self, t: &Timeline) -> Vec<Stop> {
        let mut v = Vec::new();
        for p in self.nb.pages.iter().filter(|p| self.nb.is_live(p.id) && t.section.is_none_or(|s| s == p.section)) {
            let mut add = |date: String, field: &str| v.push(Stop { date, page: p.id, field: field.to_string() });
            match &t.when {
                When::Created => add(day(p.created), ""),
                When::Modified => add(day(p.modified), ""),
                When::Field(name) => {
                    let d = self.nb.field_value(p.id, name);
                    if is_date(d) {
                        add(d.to_string(), name);
                    }
                }
                When::Fields => {
                    let fields = p.ptype.and_then(|ty| self.nb.page_type(ty)).map(|ty| ty.fields.clone()).unwrap_or_default();
                    for f in fields.iter().filter(|f| f.kind == FieldKind::Date) {
                        let d = self.nb.field_value(p.id, &f.name);
                        if is_date(d) {
                            add(d.to_string(), &f.name);
                        }
                    }
                }
            }
        }
        let title = |id: i64| self.nb.page(id).map(|p| apex_doc::notebook::fold(&p.title)).unwrap_or_default();
        v.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| title(a.page).cmp(&title(b.page))));
        if t.newest {
            v.reverse();
        }
        v
    }

    pub(super) fn timeline_ui(&mut self, ui: &mut egui::Ui) {
        let Some(mut t) = self.timeline.take() else { return };
        let mut close = false;
        let mut open = None;
        let fields = self.date_fields();
        egui::Frame::new().inner_margin(egui::Margin::symmetric(24, 12)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading(tr("nb.timeline"));
                ui.add_space(16.0);
                let name = |w: &When| match w {
                    When::Fields => tr("nb.tl_fields").to_string(),
                    When::Field(f) => f.clone(),
                    When::Created => tr("nb.tl_created").to_string(),
                    When::Modified => tr("nb.tl_modified").to_string(),
                };
                egui::ComboBox::from_id_salt(("note_tl_when", self.uid)).selected_text(name(&t.when)).show_ui(ui, |ui| {
                    ui.selectable_value(&mut t.when, When::Fields, name(&When::Fields));
                    for f in &fields {
                        ui.selectable_value(&mut t.when, When::Field(f.clone()), f);
                    }
                    ui.separator();
                    ui.selectable_value(&mut t.when, When::Created, name(&When::Created));
                    ui.selectable_value(&mut t.when, When::Modified, name(&When::Modified));
                });
                let name = t.section.and_then(|s| self.nb.section(s)).map_or_else(|| tr("nb.all_sections").to_string(), |s| s.name.clone());
                egui::ComboBox::from_id_salt(("note_tl_section", self.uid)).selected_text(name).show_ui(ui, |ui| {
                    ui.selectable_value(&mut t.section, None, tr("nb.all_sections"));
                    for s in self.nb.live_sections() {
                        ui.selectable_value(&mut t.section, Some(s.id), &s.name);
                    }
                });
                ui.toggle_value(&mut t.newest, tr("nb.tl_newest"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("nb.graph_close")).clicked() {
                        close = true;
                    }
                });
            });
            ui.add_space(8.0);
            let stops = self.stops(&t);
            if stops.is_empty() {
                let empty = if t.when == When::Fields && fields.is_empty() { "nb.tl_no_fields" } else { "nb.tl_empty" };
                ui.label(RichText::new(tr(empty)).weak());
                return;
            }
            let months: Vec<String> = tr("field.months").split('|').map(|m| m.to_string()).collect();
            let line = ui.visuals().weak_text_color().gamma_multiply(0.5);
            let plain = ui.visuals().text_color();
            egui::ScrollArea::vertical().id_salt(("note_timeline", self.uid)).auto_shrink([false, false]).show(ui, |ui| {
                let x = ui.min_rect().left() + 90.0;
                let top = ui.cursor().top();
                let axis = ui.painter().add(egui::Shape::Noop);
                let (mut year, mut month) = ("", "");
                for s in &stops {
                    let Some(p) = self.nb.page(s.page) else { continue };
                    if &s.date[..4] != year {
                        year = &s.date[..4];
                        month = "";
                        ui.add_space(6.0);
                        ui.label(RichText::new(year).heading().strong());
                    }
                    if &s.date[5..7] != month {
                        month = &s.date[5..7];
                        let m: usize = month.parse().unwrap_or(1);
                        let name = months.get(m.wrapping_sub(1)).cloned().unwrap_or_else(|| month.to_string());
                        ui.horizontal(|ui| {
                            ui.add_space(100.0);
                            ui.label(RichText::new(name).strong().color(ui.visuals().weak_text_color()));
                        });
                    }
                    let color = p.ptype.and_then(|ty| self.nb.page_type(ty)).map_or(plain, |ty| {
                        let c = ty.color;
                        Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
                    });
                    let r = ui.horizontal(|ui| {
                        ui.add_sized(vec2(80.0, 18.0), egui::Label::new(RichText::new(&s.date[8..10]).weak()));
                        ui.add_space(20.0);
                        let title = if p.title.trim().is_empty() { tr("nb.untitled").to_string() } else { p.title.clone() };
                        if ui.link(RichText::new(title).color(plain)).clicked() {
                            open = Some(s.page);
                        }
                        let section = self.nb.section(p.section).map(|s| s.name.as_str()).unwrap_or_default();
                        let what = if s.field.is_empty() { section.to_string() } else { format!("{} · {section}", s.field) };
                        ui.label(RichText::new(what).small().weak());
                    });
                    ui.painter().circle_filled(egui::pos2(x, r.response.rect.center().y), 5.0, color);
                }
                let bottom = ui.min_rect().bottom();
                ui.painter().set(axis, egui::Shape::line_segment([egui::pos2(x, top), egui::pos2(x, bottom)], egui::Stroke::new(2.0, line)));
            });
        });
        if !close {
            self.timeline = Some(t);
        }
        if let Some(id) = open {
            self.close_graph();
            self.open_page(id);
            self.grid = false;
        }
    }
}
