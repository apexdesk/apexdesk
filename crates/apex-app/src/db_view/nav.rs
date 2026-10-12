//! The navigation pane: the database's tables, queries, forms, reports
//! and linked tables, with a search box. A double click opens an object;
//! its menu opens it, designs it or deletes it.

use apex_db::objects::Kind;
use eframe::egui::{self, RichText};

use super::{Confirm, ConfirmAction, DbView, Pane};
use crate::i18n::{tr, trf};

/// What was asked of an item.
enum Act {
    Open,
    Design,
    Delete,
}

impl DbView {
    pub(super) fn nav_ui(&mut self, ui: &mut egui::Ui) {
        ui.add(egui::TextEdit::singleline(&mut self.nav_filter).hint_text(tr("db.nav_search")).desired_width(f32::INFINITY));
        ui.separator();
        let filter = self.nav_filter.to_lowercase();
        let keep = |n: &String| filter.is_empty() || n.to_lowercase().contains(&filter);
        let tables: Vec<String> = self.db.table_names().into_iter().filter(keep).collect();
        let sections: [(&str, Option<Kind>, Vec<String>); 5] = [
            ("db.nav_tables", None, tables),
            ("db.nav_queries", Some(Kind::Query), self.db.objects(Kind::Query).into_iter().filter(keep).collect()),
            ("db.nav_forms", Some(Kind::Form), self.db.objects(Kind::Form).into_iter().filter(keep).collect()),
            ("db.nav_reports", Some(Kind::Report), self.db.objects(Kind::Report).into_iter().filter(keep).collect()),
            ("db.nav_links", Some(Kind::Link), self.db.objects(Kind::Link).into_iter().filter(keep).collect()),
        ];
        let mut asked: Option<(Option<Kind>, String, Act)> = None;
        let mut new: Option<Option<Kind>> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (key, kind, names) in sections {
                let id = ui.make_persistent_id(("db_nav_sec", key));
                egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
                    .show_header(ui, |ui| {
                        ui.label(RichText::new(tr(key)).strong());
                        if ui.small_button("+").on_hover_text(tr("db.nav_new")).clicked() {
                            new = Some(kind);
                        }
                    })
                    .body(|ui| {
                        if names.is_empty() {
                            ui.label(RichText::new(tr("db.nav_none")).weak());
                        }
                        for n in names {
                            let chosen = self.nav_sel.as_ref().is_some_and(|(k, x)| *k == kind && *x == n);
                            let r = ui.selectable_label(chosen, &n);
                            if r.clicked() {
                                self.nav_sel = Some((kind, n.clone()));
                            }
                            if r.double_clicked() {
                                asked = Some((kind, n.clone(), Act::Open));
                            }
                            r.context_menu(|ui| {
                                if ui.button(tr("db.open")).clicked() {
                                    asked = Some((kind, n.clone(), Act::Open));
                                    ui.close();
                                }
                                if kind != Some(Kind::Link) && ui.button(tr("db.design")).clicked() {
                                    asked = Some((kind, n.clone(), Act::Design));
                                    ui.close();
                                }
                                if ui.button(tr("db.delete")).clicked() {
                                    asked = Some((kind, n.clone(), Act::Delete));
                                    ui.close();
                                }
                            });
                        }
                    });
            }
        });
        if let Some(kind) = new {
            match kind {
                None => self.design_table(None),
                Some(Kind::Link) => self.dialog = Some(super::transfer::Dialog::link()),
                Some(k) => self.ask_name(k),
            }
        }
        let Some((kind, name, act)) = asked else { return };
        match (kind, act) {
            (_, Act::Open) => self.open_object(kind, &name),
            (_, Act::Design) => self.design_object(kind, &name),
            (None, Act::Delete) => self.confirm = Some(Confirm { text: trf("db.confirm_delete_table", &[("name", &name)]), action: ConfirmAction::DeleteTable(name) }),
            (Some(k), Act::Delete) => self.confirm = Some(Confirm { text: trf("db.confirm_delete", &[("name", &name)]), action: ConfirmAction::DeleteObject(k, name) }),
        }
    }

    /// The object the Views buttons act on: the open pane's, else the one
    /// chosen in the navigation pane (`None` kind: a table).
    pub(super) fn view_target(&self) -> Option<(Option<Kind>, String)> {
        use super::sheet::Src;
        let open = match self.panes.get(self.cur) {
            Some(Pane::Sheet(s)) => Some(match &s.src {
                Src::Table(t) => (None, t.clone()),
                Src::Query(q, _) => (Some(Kind::Query), q.clone()),
                Src::Link(l) => (Some(Kind::Link), l.clone()),
            }),
            Some(Pane::Design(d)) => d.original.clone().map(|t| (None, t)),
            Some(Pane::Query(q)) => Some((Some(Kind::Query), q.name.clone())),
            Some(Pane::Form(f)) => Some((Some(Kind::Form), f.name.clone())),
            Some(Pane::FormDesign(f)) => Some((Some(Kind::Form), f.name.clone())),
            Some(Pane::Report(r)) => Some((Some(Kind::Report), r.name.clone())),
            _ => None,
        };
        open.or_else(|| self.nav_sel.clone())
    }

    /// Opens an object in its data view (a report in its pane).
    pub fn open_object(&mut self, kind: Option<Kind>, name: &str) {
        match kind {
            None => self.open_table(name),
            Some(Kind::Query) => self.open_query(name),
            Some(Kind::Form) => self.open_form(name),
            Some(Kind::Report) => self.open_report(name),
            Some(Kind::Link) => self.open_link(name),
            Some(_) => {}
        }
    }

    /// Opens an object in its design view (a report's pane holds both).
    pub fn design_object(&mut self, kind: Option<Kind>, name: &str) {
        match kind {
            None => self.design_table(Some(name)),
            Some(Kind::Query) => self.design_query(name),
            Some(Kind::Form) => self.design_form(name),
            Some(Kind::Report) => self.open_report(name),
            Some(Kind::Link) => self.open_link(name),
            Some(_) => {}
        }
    }
}
