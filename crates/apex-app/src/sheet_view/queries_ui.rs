//! Data > Queries & Connections: the workbook's Get Data queries (edit,
//! refresh, delete), and the connections, Power Query queries and data
//! model the file brought along, read only: ApexDesk keeps those when
//! saving but doesn't run them; their Power Query queries convert into
//! Get Data queries ([`apex_query::m`]).

use super::*;

/// What a button of the list asks for.
enum Action {
    Edit(String),
    Refresh(String),
    Delete(String),
    RefreshAll,
    /// The Power Query query at this index into one of ours.
    Convert(usize),
}

impl SheetView {
    pub(super) fn open_queries(&mut self) {
        self.settle_edit();
        self.queries_dlg = Some(apex_io::external_data(&self.wb));
    }

    pub(super) fn queries_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &self.queries_dlg else { return };
        let mut action = None;
        let busy = self.getdata_busy();
        let ours = &self.wb.queries;
        let closed = widgets::info_dialog(ctx, "queries_connections", tr("queries.title"), 600.0, |ui| {
            egui::ScrollArea::vertical().max_height(420.0).auto_shrink([false, true]).show(ui, |ui| {
                widgets::section(ui, tr("getdata.queries"));
                if ours.is_empty() {
                    ui.label(RichText::new(tr("getdata.no_queries")).weak());
                }
                for q in ours {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width(390.0);
                            ui.label(RichText::new(&q.name).strong());
                            let place = match &q.table {
                                Some(t) => trf("getdata.into_table", &[("x", t)]),
                                None => tr("getdata.connection_only").to_string(),
                            };
                            ui.label(RichText::new(format!("{} · {} · {place}", tr(q.source.kind_key()), q.source.describe())).small().weak());
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("✕").on_hover_text(tr("getdata.delete_query")).clicked() {
                                action = Some(Action::Delete(q.name.clone()));
                            }
                            ui.add_enabled_ui(q.table.is_some() && !busy, |ui| {
                                if ui.small_button(tr("getdata.refresh")).clicked() {
                                    action = Some(Action::Refresh(q.name.clone()));
                                }
                            });
                            if ui.small_button(tr("getdata.edit")).clicked() {
                                action = Some(Action::Edit(q.name.clone()));
                            }
                        });
                    });
                    ui.add_space(2.0);
                }
                if ours.iter().any(|q| q.table.is_some()) {
                    ui.add_enabled_ui(!busy, |ui| {
                        if ui.button(tr("getdata.refresh_all")).clicked() {
                            action = Some(Action::RefreshAll);
                        }
                    });
                }
                if d.is_empty() {
                    return;
                }
                ui.add_space(6.0);
                ui.label(RichText::new(tr("queries.note")).weak());
                if !d.queries.is_empty() {
                    widgets::section(ui, tr("queries.queries"));
                    for (i, q) in d.queries.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.add_enabled_ui(!busy, |ui| {
                                if ui.small_button(tr("queries.convert")).on_hover_text(tr("queries.convert_tip")).clicked() {
                                    action = Some(Action::Convert(i));
                                }
                            });
                            ui.label(RichText::new(&q.name).strong());
                        });
                        egui::CollapsingHeader::new(RichText::new(tr("queries.formula")).small()).id_salt(("query", i)).show(ui, |ui| {
                            let mut f = q.formula.as_str();
                            ui.add(egui::TextEdit::multiline(&mut f).code_editor().desired_rows(3).desired_width(f32::INFINITY));
                        });
                    }
                }
                if !d.connections.is_empty() {
                    widgets::section(ui, tr("queries.connections"));
                    for c in &d.connections {
                        ui.label(RichText::new(&c.name).strong());
                        if !c.description.is_empty() {
                            ui.label(&c.description);
                        }
                        for (key, v) in [("queries.source", &c.source), ("queries.command", &c.command)] {
                            if !v.is_empty() {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(RichText::new(tr(key)).weak());
                                    ui.label(RichText::new(v).monospace().size(11.5));
                                });
                            }
                        }
                        ui.add_space(4.0);
                    }
                }
                if d.data_model || d.pivot_tables > 0 {
                    widgets::section(ui, tr("queries.other"));
                    if d.data_model {
                        ui.label(tr("queries.data_model"));
                    }
                    if d.pivot_tables > 0 {
                        ui.label(trf("queries.pivots", &[("n", &d.pivot_tables)]));
                    }
                }
            });
        });
        match action {
            Some(Action::Edit(n)) => {
                self.queries_dlg = None;
                self.edit_query(&n);
            }
            Some(Action::Refresh(n)) => self.ask_refresh(vec![n], false),
            Some(Action::RefreshAll) => self.refresh_all_queries(),
            Some(Action::Delete(n)) => self.delete_query(&n),
            Some(Action::Convert(i)) => self.convert_power_query(i),
            None => {}
        }
        if closed {
            self.queries_dlg = None;
            self.focus_grid = true;
        }
    }

    /// Converts the workbook's Power Query query `i` into a Get Data query
    /// and opens it in the editor; what didn't convert is said there.
    fn convert_power_query(&mut self, i: usize) {
        let Some(d) = &self.queries_dlg else { return };
        let Some(pq) = d.queries.get(i) else { return };
        let others: Vec<String> = d.queries.iter().map(|q| q.name.clone()).chain(self.wb.queries.iter().map(|q| q.name.clone())).filter(|n| *n != pq.name).collect();
        let (name, t) = (pq.name.clone(), apex_query::m::translate(&pq.name, &pq.formula, &others));
        let missing = t.untranslated.join(", ");
        self.queries_dlg = None;
        match t.query {
            Some(q) => {
                let note = (!missing.is_empty()).then(|| trf("queries.untranslated", &[("steps", &missing)]));
                self.edit_converted_query(q, note);
            }
            None => self.notices.push(Notice::error(trf("queries.cannot_convert", &[("x", &name), ("steps", &missing)]))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_sheet::query::{Query, Source, Step};

    #[test]
    fn convert_opens_the_power_query_query_in_the_editor() {
        let mut wb = Workbook::new();
        wb.sheets.push(apex_sheet::Sheet::new("Data"));
        wb.queries.push(Query::new("Ventas", Source::Table { name: "T".into() }));
        let mut v = SheetView::new(wb);
        let pq = |name: &str, formula: &str| apex_io::PowerQuery { name: name.into(), formula: formula.into() };
        let m = r#"let Origen = Csv.Document(File.Contents("C:\datos\ventas.csv"),[Delimiter=";"]), P = Table.PromoteHeaders(Origen), F = Table.FillDown(P, {"A"}) in F"#;
        v.queries_dlg = Some(apex_io::ExternalData { queries: vec![pq("Ventas", m), pq("Carpeta", r#"let S = Folder.Files("C:\x") in S"#)], ..Default::default() });
        v.convert_power_query(0);
        assert!(v.queries_dlg.is_none());
        let (q, msg) = v.getdata.editor.as_ref().map(|e| e.parts()).unwrap();
        // Named apart from the query already there; stops at what has no step.
        assert_eq!(q.name, "Ventas (2)");
        assert_eq!(q.steps, [Step::PromoteHeaders]);
        assert!(msg.is_some_and(|m| m.contains("F (Table.FillDown)")), "{msg:?}");
        // Nothing to convert: a notice, no editor.
        v.getdata.editor = None;
        v.queries_dlg = Some(apex_io::ExternalData { queries: vec![pq("Carpeta", r#"let S = Folder.Files("C:\x") in S"#)], ..Default::default() });
        v.convert_power_query(0);
        assert!(v.getdata.editor.is_none());
        assert!(v.notices.last().is_some_and(|n| n.error && n.text.contains("Folder.Files")));
    }
}
