//! File > Organization Policies: the settings the organization fixes, each
//! with its value and where it comes from (for users and the help desk).

use eframe::egui::{self, RichText};

use super::App;
use crate::i18n::tr;
use crate::policy;
use crate::widgets;

impl App {
    pub(super) fn policy_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_policy {
            return;
        }
        let p = policy::current();
        let closed = widgets::info_dialog(ctx, "policies", tr("policy.title"), 620.0, |ui| {
            if p.set.is_empty() {
                ui.label(tr("policy.none"));
                ui.add_space(4.0);
                ui.label(RichText::new(tr("policy.where")).small().weak());
                return;
            }
            ui.label(RichText::new(tr("policy.intro")).weak());
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                egui::Grid::new("policy_grid").num_columns(3).striped(true).spacing([14.0, 6.0]).show(ui, |ui| {
                    ui.label(RichText::new(tr("policy.setting")).strong());
                    ui.label(RichText::new(tr("policy.value")).strong());
                    ui.label(RichText::new(tr("policy.source")).strong());
                    ui.end_row();
                    for s in &p.set {
                        ui.label(crate::i18n::lookup(&format!("policy.{}", s.item.name)).unwrap_or(s.item.name)).on_hover_text(s.item.name);
                        ui.label(s.values.join("\n"));
                        ui.label(RichText::new(&s.source).small().weak());
                        ui.end_row();
                    }
                });
                if !p.ignored.is_empty() {
                    ui.add_space(8.0);
                    ui.label(RichText::new(tr("policy.ignored")).strong());
                    for i in &p.ignored {
                        ui.label(RichText::new(i).small().weak());
                    }
                }
            });
        });
        if closed {
            self.show_policy = false;
        }
    }
}
