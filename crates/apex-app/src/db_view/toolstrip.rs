//! The database tab's toolstrip: Home (undo, the open pane's views, print),
//! Create (tables, queries, forms, reports and their wizards), External
//! data (import, export, linked tables) and Database tools (relations,
//! compact and repair, backup).

use apex_db::objects::Kind;
use eframe::egui::{self, Id};

use super::transfer::{self, ExportTo};
use super::wizards::{WizKind, Wizard};
use super::{DbView, Pane};
use crate::common::{self, AppCmd, Shell};
use crate::i18n::tr;
use crate::icons::Icon;
use crate::toolstrip::{self, Body, TabDef};
use crate::theme;

impl DbView {
    pub fn toolstrip_tabs(&self) -> Vec<TabDef> {
        vec![TabDef::new("home", "toolstrip.home"), TabDef::new("create", "db.tab_create"), TabDef::new("data", "db.tab_data"), TabDef::new("tools", "db.tab_tools")]
    }

    pub fn toolstrip(&mut self, ui: &mut egui::Ui, tab: &str, shell: &mut Shell) {
        let id = Id::new(("db_toolstrip", tab));
        toolstrip::body(ui, id, theme::DB_ACCENT, |b| match tab {
            "create" => self.create_tab(b),
            "data" => self.data_tab(b),
            "tools" => self.tools_tab(b),
            _ => self.home_tab(b, shell),
        });
    }

    fn home_tab(&mut self, b: &mut Body, shell: &mut Shell) {
        let acc = b.accent();
        b.group("toolstrip.g.undo", Icon::Undo, |ui| {
            toolstrip::column(ui, |ui| {
                for _ in 0..common::history_button(ui, false, &self.undo_names()) {
                    self.undo();
                }
                for _ in 0..common::history_button(ui, true, &self.redo_names()) {
                    self.redo();
                }
            });
        });
        b.group("db.g_views", Icon::Table, |ui| {
            let target = self.view_target();
            let cur = self.panes.get(self.cur);
            let datasheet = matches!(cur, Some(Pane::Sheet(_) | Pane::Form(_) | Pane::Report(_)));
            let design = matches!(cur, Some(Pane::Design(_) | Pane::Query(_) | Pane::FormDesign(_) | Pane::Report(_)));
            // A linked table's design is its server's.
            let can_design = target.as_ref().is_some_and(|(k, _)| *k != Some(Kind::Link));
            let r = ui.add_enabled_ui(target.is_some(), |ui| toolstrip::large(ui, Icon::Table, tr("db.view_data"), tr("db.view_data_tip"), acc, datasheet)).inner;
            if r.on_disabled_hover_text(tr("db.view_none")).clicked()
                && let Some((kind, name)) = &target
            {
                self.open_object(*kind, name);
            }
            let r = ui.add_enabled_ui(can_design, |ui| toolstrip::large(ui, Icon::Pencil, tr("db.design"), tr("db.view_design_tip"), acc, design)).inner;
            let why = if target.is_some() { tr("db.view_no_design") } else { tr("db.view_none") };
            if r.on_disabled_hover_text(why).clicked()
                && let Some((kind, name)) = &target
            {
                self.design_object(*kind, name);
            }
        });
        b.group("db.g_output", Icon::Doc, |ui| {
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::Doc), tr("db.print"), tr("db.print_tip"), acc, false).clicked() {
                    shell.cmd = Some(AppCmd::Print);
                }
                if toolstrip::small(ui, Some(Icon::Save), tr("db.export_pdf"), tr("db.export_pdf"), acc, false).clicked() {
                    shell.cmd = Some(AppCmd::ExportPdf);
                }
            });
        });
    }

    // --- View (the application adds the Window and Screen groups) ---------------------

    pub(crate) fn view_tab(&mut self, b: &mut Body) {
        let acc = b.accent();
        b.group("db.g_show", Icon::NavPane, |ui| {
            if toolstrip::large(ui, Icon::NavPane, tr("db.nav_pane"), tr("db.nav_pane_tip"), acc, self.nav_open).clicked() {
                self.nav_open = !self.nav_open;
            }
        });
    }

    fn create_tab(&mut self, b: &mut Body) {
        let acc = b.accent();
        b.group("db.g_tables", Icon::Table, |ui| {
            if toolstrip::large(ui, Icon::Table, tr("db.new_table"), tr("db.new_table_tip"), acc, false).clicked() {
                self.design_table(None);
            }
        });
        b.group("db.g_queries", Icon::Filter, |ui| {
            if toolstrip::large(ui, Icon::Filter, tr("db.new_query"), tr("db.new_query_tip"), acc, false).clicked() {
                self.ask_name(Kind::Query);
            }
        });
        b.group("db.g_forms", Icon::Control, |ui| {
            if toolstrip::large(ui, Icon::Control, tr("db.form_wizard"), tr("db.form_wizard_tip"), acc, false).clicked() {
                self.wizard = Some(Wizard::new(WizKind::Form, &self.db));
            }
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::Pencil), tr("db.new_form"), tr("db.new_form_tip"), acc, false).clicked() {
                    self.ask_name(Kind::Form);
                }
            });
        });
        b.group("db.g_reports", Icon::Doc, |ui| {
            if toolstrip::large(ui, Icon::Doc, tr("db.report_wizard"), tr("db.report_wizard_tip"), acc, false).clicked() {
                self.wizard = Some(Wizard::new(WizKind::Report, &self.db));
            }
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::Pencil), tr("db.new_report"), tr("db.new_report_tip"), acc, false).clicked() {
                    self.ask_name(Kind::Report);
                }
            });
        });
    }

    fn data_tab(&mut self, b: &mut Body) {
        let acc = b.accent();
        b.group("db.g_import", Icon::Folder, |ui| {
            if toolstrip::large(ui, Icon::Folder, tr("db.import"), tr("db.import_tip"), acc, false).clicked() {
                transfer::import(self);
            }
            if toolstrip::large(ui, Icon::Link, tr("db.link_table"), tr("db.link_table_tip"), acc, false).clicked() {
                self.dialog = Some(transfer::Dialog::link());
            }
        });
        b.group("db.g_export", Icon::Save, |ui| {
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::Sheet), tr("db.export_workbook"), tr("db.export_tip"), acc, false).clicked() {
                    transfer::export(self, ExportTo::Workbook);
                }
                if toolstrip::small(ui, Some(Icon::Doc), tr("db.export_csv"), tr("db.export_tip"), acc, false).clicked() {
                    transfer::export(self, ExportTo::Csv);
                }
                if toolstrip::small(ui, Some(Icon::Table), tr("db.export_db"), tr("db.export_tip"), acc, false).clicked() {
                    transfer::export(self, ExportTo::Database);
                }
            });
        });
    }

    fn tools_tab(&mut self, b: &mut Body) {
        let acc = b.accent();
        b.group("db.g_relations", Icon::Link, |ui| {
            if toolstrip::large(ui, Icon::Link, tr("db.relations"), tr("db.relations_tip"), acc, false).clicked() {
                self.open_relations();
            }
        });
        b.group("db.g_maintenance", Icon::Calculate, |ui| {
            if toolstrip::large(ui, Icon::Calculate, tr("db.compact"), tr("db.compact_tip"), acc, false).clicked() {
                self.compact();
            }
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::ErrorCheck), tr("db.repair"), tr("db.repair_tip"), acc, false).clicked() {
                    self.repair();
                }
                if toolstrip::small(ui, Some(Icon::Save), tr("db.backup"), tr("db.backup_tip"), acc, false).clicked() {
                    self.backup();
                }
            });
        });
    }
}
