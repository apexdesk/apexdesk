//! Reports: the settings (source, columns with their totals, grouping
//! levels with header, footer and page break, sorting, page header and
//! footer) at the left and the preview at the right. Printing and PDF
//! export go through the word processor's printing, from the report laid
//! out as a document ([`document`]); labels reuse mail merge's label sheets.

use std::sync::Arc;

use apex_db::objects::Kind;
use apex_db::report::{lay_out, Agg, Line, RField, RGroup, ReportDef};
use apex_db::sqlite::{Db, Filter, RowSource};
use apex_db::Value;
use apex_doc::{Attrs, Cell, Document, Paragraph, Table, TableProps};
use eframe::egui::{self, RichText};

use super::sheet::cell_text;
use super::{DbView, Request};
use crate::common::{AppCmd, Shell};
use crate::i18n::{tr, trf};

/// Points in a millimetre.
const MM: f32 = 72.0 / 25.4;

/// Most rows a report reads.
const MAX_ROWS: usize = 100_000;

pub struct ReportPane {
    pub name: String,
    pub def: ReportDef,
    /// The laid out lines of the preview and the revision they are of.
    lines: Option<(u64, Vec<Line>)>,
    error: Option<String>,
    /// The label sheet chosen for labels.
    label_sheet: usize,
}

impl ReportPane {
    pub fn new(name: &str, def: ReportDef) -> ReportPane {
        ReportPane { name: name.to_string(), def, lines: None, error: None, label_sheet: 0 }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, v: &mut DbView, shell: &mut Shell) {
        // Without a source there are no rows to print or put on labels.
        let ready = self.has_source();
        let off = |r: egui::Response| r.on_disabled_hover_text(tr("db.report_no_source"));
        ui.horizontal(|ui| {
            if ui.button(tr("db.save")).clicked() {
                self.save(v);
            }
            if ui.button(tr("db.refresh")).clicked() {
                self.lines = None;
            }
            if off(ui.add_enabled(ready, egui::Button::new(tr("db.print")))).clicked() {
                shell.cmd = Some(AppCmd::Print);
            }
            if off(ui.add_enabled(ready, egui::Button::new(tr("db.export_pdf")))).clicked() {
                shell.cmd = Some(AppCmd::ExportPdf);
            }
            if off(ui.add_enabled(ready, egui::Button::new(tr("db.open_as_document")))).clicked() {
                v.requests.push(Request::OpenDoc(Box::new(document(&v.db, &self.def))));
            }
            ui.separator();
            let sheets = apex_doc::merge::LABELS;
            egui::ComboBox::from_id_salt("db_label_sheet").width(180.0).selected_text(sheets[self.label_sheet.min(sheets.len() - 1)].name).show_ui(ui, |ui| {
                for (i, s) in sheets.iter().enumerate() {
                    ui.selectable_value(&mut self.label_sheet, i, s.name);
                }
            });
            if off(ui.add_enabled(ready, egui::Button::new(tr("db.labels"))).on_hover_text(tr("db.labels_tip"))).clicked() {
                match labels(&v.db, &self.def, self.label_sheet) {
                    Ok(d) => v.requests.push(Request::OpenDoc(Box::new(d))),
                    Err(e) => v.error(e),
                }
            }
        });
        ui.separator();
        let changed = egui::Panel::left(egui::Id::new(("db_report_side", &self.name))).resizable(true).default_size(300.0).show(ui, |ui| egui::ScrollArea::vertical().show(ui, |ui| self.settings(ui, &v.db)).inner).inner;
        if changed {
            self.lines = None;
        }
        if !self.has_source() {
            ui.label(RichText::new(tr("db.report_no_source")).weak());
            return;
        }
        if self.lines.as_ref().is_none_or(|(r, _)| *r != v.revision) {
            match rows(&v.db, &self.def) {
                Ok((cols, rows)) => {
                    self.lines = Some((v.revision, lay_out(&self.def, &cols, &rows)));
                    self.error = None;
                }
                Err(e) => {
                    self.lines = Some((v.revision, Vec::new()));
                    self.error = Some(e.to_string());
                }
            }
        }
        if let Some(e) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        let Some((_, lines)) = &self.lines else { return };
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| preview(ui, &self.def, lines));
    }

    /// Whether the report has a table or query to read its rows from.
    pub fn has_source(&self) -> bool {
        !self.def.source.trim().is_empty()
    }

    pub fn document(&self, db: &Db) -> Document {
        document(db, &self.def)
    }

    fn save(&mut self, v: &mut DbView) {
        if v.save_object(Kind::Report, &self.name, &self.def.to_records()) {
            v.notices.push(crate::app::Notice::info(trf("db.saved", &[("name", &self.name)])));
        }
    }

    /// The settings; gives whether something changed.
    fn settings(&mut self, ui: &mut egui::Ui, db: &Db) -> bool {
        let before = self.def.clone();
        let d = &mut self.def;
        egui::Grid::new("db_report_head").num_columns(2).show(ui, |ui| {
            ui.label(tr("db.r_title"));
            ui.text_edit_singleline(&mut d.title);
            ui.end_row();
            ui.label(tr("db.source"));
            source_combo(ui, "db_report_source", db, &mut d.source);
            ui.end_row();
            ui.label(tr("db.page_header"));
            ui.text_edit_singleline(&mut d.page_header).on_hover_text(tr("db.page_codes"));
            ui.end_row();
            ui.label(tr("db.page_footer"));
            ui.text_edit_singleline(&mut d.page_footer).on_hover_text(tr("db.page_codes"));
            ui.end_row();
        });
        ui.checkbox(&mut d.grand_total, tr("db.grand_total"));
        ui.checkbox(&mut d.landscape, tr("db.landscape"));
        let columns = db.columns(&RowSource::Table(d.source.clone())).map(|c| c.0).unwrap_or_default();
        ui.add_space(6.0);
        ui.label(RichText::new(tr("db.r_fields")).strong());
        for c in &columns {
            let pos = d.fields.iter().position(|f| &f.name == c);
            let mut on = pos.is_some();
            ui.horizontal(|ui| {
                if ui.checkbox(&mut on, c).changed() {
                    if on {
                        d.fields.push(RField { name: c.clone(), ..Default::default() });
                    } else if let Some(i) = pos {
                        d.fields.remove(i);
                    }
                }
                if let Some(f) = d.fields.iter_mut().find(|f| &f.name == c) {
                    egui::ComboBox::from_id_salt(("db_rtotal", c)).width(90.0).selected_text(super::trk("db.agg", agg_key(f.total))).show_ui(ui, |ui| {
                        for a in Agg::ALL {
                            ui.selectable_value(&mut f.total, a, super::trk("db.agg", agg_key(a)));
                        }
                    });
                    ui.add(egui::TextEdit::singleline(&mut f.label).desired_width(90.0).hint_text(tr("db.r_label")));
                }
            });
        }
        ui.add_space(6.0);
        ui.label(RichText::new(tr("db.groups")).strong());
        let mut remove = None;
        for (i, g) in d.groups.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                field_combo(ui, ("db_rgroup", i), &columns, &mut g.field);
                ui.checkbox(&mut g.asc, "↑");
                if ui.small_button("×").clicked() {
                    remove = Some(i);
                }
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut g.header, tr("db.group_header"));
                ui.checkbox(&mut g.footer, tr("db.group_footer"));
                ui.checkbox(&mut g.page_break, tr("db.group_page_break"));
            });
        }
        if let Some(i) = remove {
            d.groups.remove(i);
        }
        if ui.button(tr("db.add_group")).clicked() {
            d.groups.push(RGroup { field: columns.first().cloned().unwrap_or_default(), ..Default::default() });
        }
        ui.add_space(6.0);
        ui.label(RichText::new(tr("db.sort")).strong());
        let mut remove = None;
        for (i, (f, asc)) in d.sort.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                field_combo(ui, ("db_rsort", i), &columns, f);
                ui.checkbox(asc, "↑");
                if ui.small_button("×").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            d.sort.remove(i);
        }
        if ui.button(tr("db.add_sort")).clicked() {
            d.sort.push((columns.first().cloned().unwrap_or_default(), true));
        }
        *d != before
    }
}

fn agg_key(a: Agg) -> &'static str {
    if a == Agg::None { "none" } else { a.key() }
}

/// A dropdown of the tables and saved queries.
pub fn source_combo(ui: &mut egui::Ui, id: &str, db: &Db, source: &mut String) {
    egui::ComboBox::from_id_salt(id).width(180.0).selected_text(source.as_str()).show_ui(ui, |ui| {
        for t in db.table_names().into_iter().chain(db.view_names()) {
            let label = t.clone();
            ui.selectable_value(source, t, label);
        }
    });
}

pub fn field_combo(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, columns: &[String], field: &mut String) {
    egui::ComboBox::from_id_salt(ui.id().with(id)).width(140.0).selected_text(field.as_str()).show_ui(ui, |ui| {
        for c in columns {
            ui.selectable_value(field, c.clone(), c);
        }
    });
}

/// The source's rows sorted for the report (groups first).
pub fn rows(db: &Db, def: &ReportDef) -> Result<(Vec<String>, Vec<Vec<Value>>), apex_db::Error> {
    let mut sort: Vec<(String, bool)> = def.groups.iter().map(|g| (g.field.clone(), g.asc)).collect();
    sort.extend(def.sort.iter().cloned());
    let filter = Filter { sql: def.filter.clone(), ..Default::default() };
    let page = db.page(&RowSource::Table(def.source.clone()), &filter, &sort, 0, MAX_ROWS)?;
    Ok((page.columns, page.rows))
}

fn line_texts(def: &ReportDef, l: &Line) -> Option<(Vec<String>, bool)> {
    let n = def.fields.len();
    Some(match l {
        Line::Headings(h) => (h.clone(), true),
        Line::Detail(d) => (d.clone(), false),
        Line::GroupFooter { value, count, totals, .. } => (footer_cells(n, &trf("db.group_total", &[("value", value), ("n", count)]), totals), true),
        Line::GrandTotal { count, totals } => (footer_cells(n, &trf("db.grand_total_n", &[("n", count)]), totals), true),
        _ => return None,
    })
}

/// A footer row: the label in the first column unless it has a total.
fn footer_cells(n: usize, label: &str, totals: &[String]) -> Vec<String> {
    let mut cells: Vec<String> = (0..n).map(|i| totals.get(i).cloned().unwrap_or_default()).collect();
    if let Some(first) = cells.first_mut() {
        if first.is_empty() {
            *first = label.to_string();
        } else {
            *first = format!("{label}: {first}");
        }
    }
    cells
}

fn preview(ui: &mut egui::Ui, def: &ReportDef, lines: &[Line]) {
    let widths: Vec<f32> = def.fields.iter().map(|f| if f.width > 0.0 { f.width * 3.0 } else { 120.0 }).collect();
    let frame = egui::Frame::new().fill(ui.visuals().extreme_bg_color).inner_margin(16.0).stroke(ui.visuals().widgets.noninteractive.bg_stroke);
    frame.show(ui, |ui| {
        if !def.page_header.is_empty() {
            ui.label(RichText::new(&def.page_header).weak());
        }
        for (i, l) in lines.iter().enumerate() {
            match l {
                Line::Title(t) => {
                    ui.label(RichText::new(t).heading().strong());
                }
                Line::GroupHeader { level, field, value } => {
                    ui.add_space(4.0);
                    ui.label(RichText::new(format!("{}{field}: {value}", "    ".repeat(*level))).strong());
                }
                Line::PageBreak => {
                    ui.separator();
                    ui.label(RichText::new(tr("db.page_break")).weak().small());
                }
                l => {
                    let Some((cells, strong)) = line_texts(def, l) else { continue };
                    ui.horizontal(|ui| {
                        for (c, text) in cells.iter().enumerate() {
                            let w = widths.get(c).copied().unwrap_or(120.0);
                            let t = if strong { RichText::new(text).strong() } else { RichText::new(text) };
                            ui.add_sized([w, 16.0], egui::Label::new(t).truncate());
                        }
                    });
                    if matches!(l, Line::Headings(_)) {
                        ui.separator();
                    }
                }
            }
            if i > 5000 {
                ui.label(RichText::new("…").weak());
                break;
            }
        }
        if !def.page_footer.is_empty() {
            ui.separator();
            ui.label(RichText::new(def.page_footer.replace("&P", "1").replace("&N", "1").replace("&D", "")).weak());
        }
    });
}

fn bold() -> Attrs {
    Attrs { bold: true, ..Default::default() }
}

fn bold_para(text: &str, size: Option<u16>) -> Paragraph {
    let mut p = Paragraph::new("");
    p.runs.clear();
    p.push(text, Attrs { size, ..bold() });
    p
}

/// A header or footer line: `&P`, `&N` and `&D` become fields.
fn hf_para(text: &str) -> Paragraph {
    let mut p = Paragraph::new("");
    p.runs.clear();
    let mut rest = text;
    while let Some(i) = rest.find('&') {
        let kind = match rest[i + 1..].chars().next() {
            Some('P') => Some(apex_doc::FieldKind::Page),
            Some('N') => Some(apex_doc::FieldKind::NumPages),
            Some('D') => Some(apex_doc::FieldKind::Date),
            _ => None,
        };
        let Some(kind) = kind else {
            p.push(&rest[..=i], Attrs::default());
            rest = &rest[i + 1..];
            continue;
        };
        if i > 0 {
            p.push(&rest[..i], Attrs::default());
        }
        p.push(&apex_doc::OBJECT.to_string(), Attrs { field: Some(Arc::new(apex_doc::Field::new(kind))), ..Default::default() });
        rest = &rest[i + 2..];
    }
    if !rest.is_empty() {
        p.push(rest, Attrs::default());
    }
    if p.runs.is_empty() {
        p = Paragraph::new("");
    }
    p.style.align = apex_doc::Align::Center;
    p
}

/// A table of rows (with its heading row repeated on every page).
fn doc_table(widths: &[f32], rows: Vec<(Vec<String>, bool)>) -> Table {
    let props = TableProps { widths: widths.to_vec(), style: Some("PlainTable1".into()), header_rows: 1, ..Default::default() };
    let rows = rows
        .into_iter()
        .map(|(cells, strong)| {
            cells
                .into_iter()
                .map(|t| {
                    let mut c = Cell::empty();
                    c.paras = vec![if strong { bold_para(&t, None) } else { Paragraph::new(t) }];
                    c
                })
                .collect()
        })
        .collect();
    Table { props, rows }
}

/// Column widths in points filling `total`.
fn widths(def: &ReportDef, total: f32) -> Vec<f32> {
    let fixed: f32 = def.fields.iter().map(|f| f.width * MM).sum();
    let free = def.fields.iter().filter(|f| f.width <= 0.0).count().max(1) as f32;
    let share = ((total - fixed) / free).max(30.0);
    def.fields.iter().map(|f| if f.width > 0.0 { f.width * MM } else { share }).collect()
}

/// The report as a document to print or export.
pub fn document(db: &Db, def: &ReportDef) -> Document {
    let lines = match rows(db, def) {
        // A new report whose data isn't chosen yet: its title alone.
        _ if def.source.trim().is_empty() => vec![Line::Title(def.title.clone())],
        Ok((cols, rows)) => lay_out(def, &cols, &rows),
        Err(e) => vec![Line::Title(def.title.clone()), Line::Detail(vec![e.to_string()])],
    };
    let mut page = apex_doc::PageSetup::A4;
    if def.landscape {
        std::mem::swap(&mut page.width, &mut page.height);
    }
    let w = widths(def, page.width - page.margin_left - page.margin_right);
    let mut paras = Vec::new();
    let mut table: Vec<(Vec<String>, bool)> = Vec::new();
    let mut headings: Option<Vec<String>> = None;
    let mut id = 1;
    let mut brk = false;
    let flush = |paras: &mut Vec<Paragraph>, table: &mut Vec<(Vec<String>, bool)>, id: &mut u32, brk: &mut bool| {
        if table.is_empty() {
            return;
        }
        let mut ps = doc_table(&w, std::mem::take(table)).into_paras(*id);
        *id += 1;
        if *brk {
            if let Some(p) = ps.first_mut() {
                p.style.page_break_before = true;
            }
            *brk = false;
        }
        paras.extend(ps);
        // Tables need a paragraph between them.
        paras.push(Paragraph::new(""));
    };
    for l in &lines {
        match l {
            Line::Title(t) => paras.push(bold_para(t, Some(32))),
            Line::Headings(h) => headings = Some(h.clone()),
            Line::GroupHeader { level, field, value } => {
                flush(&mut paras, &mut table, &mut id, &mut brk);
                let mut p = bold_para(&format!("{field}: {value}"), Some(if *level == 0 { 26 } else { 22 }));
                p.style.indent_left = *level as f32 * 12.0;
                p.style.keep_with_next = true;
                p.style.page_break_before = std::mem::take(&mut brk);
                paras.push(p);
            }
            Line::PageBreak => {
                flush(&mut paras, &mut table, &mut id, &mut brk);
                brk = true;
            }
            l => {
                if table.is_empty() {
                    if let Some(h) = &headings {
                        table.push((h.clone(), true));
                    }
                }
                if let Some(row) = line_texts(def, l) {
                    table.push(row);
                }
            }
        }
    }
    flush(&mut paras, &mut table, &mut id, &mut brk);
    let mut d = Document::from_paragraphs(paras);
    d.page = page;
    let header = (!def.page_header.is_empty()).then(|| vec![hf_para(&def.page_header)]);
    let footer = (!def.page_footer.is_empty()).then(|| vec![hf_para(&def.page_footer)]);
    d.set_simple_headers(header, footer);
    d.modified = false;
    d
}

/// Plain rows as a document (a datasheet printed).
pub fn table_doc(title: &str, columns: &[String], rows: &[Vec<Value>]) -> Document {
    let def = ReportDef { title: title.to_string(), fields: columns.iter().map(|c| RField { name: c.clone(), ..Default::default() }).collect(), page_footer: "&P / &N".into(), landscape: columns.len() > 6, ..Default::default() };
    let mut page = apex_doc::PageSetup::A4;
    if def.landscape {
        std::mem::swap(&mut page.width, &mut page.height);
    }
    let w = widths(&def, page.width - page.margin_left - page.margin_right);
    let mut t = vec![(columns.to_vec(), true)];
    t.extend(rows.iter().map(|r| (r.iter().map(cell_text).collect(), false)));
    let mut paras = vec![bold_para(title, Some(28))];
    paras.extend(doc_table(&w, t).into_paras(1));
    paras.push(Paragraph::new(""));
    let mut d = Document::from_paragraphs(paras);
    d.page = page;
    d.set_simple_headers(None, Some(vec![hf_para(&def.page_footer)]));
    d.modified = false;
    d
}

/// Labels: one per row, with the report's columns one per line.
pub fn labels(db: &Db, def: &ReportDef, sheet: usize) -> Result<Document, apex_db::Error> {
    let (cols, rows) = rows(db, def)?;
    let idx: Vec<usize> = def.fields.iter().filter_map(|f| cols.iter().position(|c| c == &f.name)).collect();
    let texts: Vec<String> = rows.iter().map(|r| idx.iter().map(|&i| cell_text(&r[i])).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n")).collect();
    let sheets = apex_doc::merge::LABELS;
    Ok(apex_doc::merge::labels(&sheets[sheet.min(sheets.len() - 1)], &texts))
}
