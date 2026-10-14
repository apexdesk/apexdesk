//! Tools > Project Properties of the macro editor: the project's name,
//! description and password lock; opening a locked project with its
//! password; and Tools > References.

use apex_io::vba::VbaProject;
use eframe::egui::{self, RichText};

use crate::i18n::tr;

/// The Project Properties window, being filled in.
pub(super) struct Props {
    name: String,
    description: String,
    lock: bool,
    password: String,
    confirm: String,
    /// The project has a password now (none typed keeps it).
    had: bool,
    error: Option<&'static str>,
}

impl Props {
    pub(super) fn of(p: &VbaProject) -> Props {
        let had = p.has_password();
        Props { name: p.name.clone(), description: p.description.clone(), lock: had, password: String::new(), confirm: String::new(), had, error: None }
    }

    /// What OK does to `p`, or why it can't.
    fn apply(&self, p: &mut VbaProject) -> Result<(), &'static str> {
        let name = self.name.trim();
        if !apex_io::vba::is_identifier(name) {
            return Err("macros.prop_bad_name");
        }
        if self.lock && (self.password.is_empty() && !self.had) {
            return Err("macros.prop_no_password");
        }
        if self.password != self.confirm {
            return Err("macros.prop_mismatch");
        }
        let lock = match (self.lock, self.password.is_empty()) {
            (true, false) => Some(Some(self.password.as_str())),
            (true, true) => None,
            (false, _) => self.had.then_some(None),
        };
        if let Some(pw) = lock {
            p.set_password(pw).map_err(|_| "macros.locked")?;
        }
        p.name = name.to_string();
        p.description = self.description.clone();
        Ok(())
    }
}

/// The window; true when it changed the project. Closes itself (`open`
/// to None) on OK or Cancel.
pub(super) fn show(ctx: &egui::Context, open: &mut Option<Props>, project: &mut VbaProject) -> bool {
    let Some(d) = open.as_mut() else { return false };
    let mut keep = true;
    let mut done = None;
    egui::Window::new(tr("macros.prop_title")).id(egui::Id::new("macro_props")).open(&mut keep).resizable(false).collapsible(false).anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO).show(ctx, |ui| {
        ui.set_max_width(400.0);
        egui::Grid::new("macro_props_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label(tr("macros.prop_name"));
            ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(260.0));
            ui.end_row();
            ui.label(tr("macros.prop_description"));
            ui.add(egui::TextEdit::singleline(&mut d.description).desired_width(260.0));
            ui.end_row();
        });
        ui.separator();
        ui.checkbox(&mut d.lock, tr("macros.prop_lock"));
        ui.add_enabled_ui(d.lock, |ui| {
            egui::Grid::new("macro_props_pw").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label(tr("macros.prop_password"));
                ui.add(egui::TextEdit::singleline(&mut d.password).password(true).desired_width(200.0));
                ui.end_row();
                ui.label(tr("macros.prop_confirm"));
                ui.add(egui::TextEdit::singleline(&mut d.confirm).password(true).desired_width(200.0));
                ui.end_row();
            });
            if d.had {
                ui.label(RichText::new(tr("macros.prop_keep")).weak());
            }
        });
        ui.label(RichText::new(tr("macros.prop_lock_note")).weak().small());
        if let Some(e) = d.error {
            ui.label(RichText::new(tr(e)).color(ui.visuals().error_fg_color));
        }
        ui.horizontal(|ui| {
            if ui.button(tr("dialog.ok")).clicked() {
                done = Some(true);
            }
            if ui.button(tr("dialog.cancel")).clicked() {
                done = Some(false);
            }
        });
    });
    let changed = match done {
        Some(true) => match d.apply(project) {
            Ok(()) => true,
            Err(e) => {
                d.error = Some(e);
                return false;
            }
        },
        _ => false,
    };
    if !keep || done.is_some() {
        *open = None;
    }
    changed
}

/// The password asked to open a locked project: true once it matched.
pub(super) fn unlock_row(ui: &mut egui::Ui, typed: &mut Option<(String, bool)>, project: &VbaProject) -> bool {
    let Some((pw, wrong)) = typed.as_mut() else {
        if project.has_password() && ui.button(tr("macros.unlock")).on_hover_text(tr("macros.unlock_tip")).clicked() {
            *typed = Some((String::new(), false));
        }
        return false;
    };
    let r = ui.add(egui::TextEdit::singleline(pw).password(true).hint_text(tr("macros.prop_password")).desired_width(160.0));
    if pw.is_empty() {
        r.request_focus();
    }
    let enter = (r.has_focus() || r.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter));
    if enter || ui.button(tr("dialog.ok")).clicked() {
        if project.password_matches(pw) {
            *typed = None;
            return true;
        }
        *wrong = true;
        pw.clear();
    }
    if *wrong {
        ui.label(RichText::new(tr("macros.wrong_password")).color(ui.visuals().error_fg_color));
    }
    if ui.button(tr("dialog.cancel")).clicked() {
        *typed = None;
    }
    false
}

/// Tools > References: the project's libraries and whether their code runs
/// here. Read only: they are kept for Excel as they are.
pub(super) fn references(ctx: &egui::Context, open: &mut bool, project: &VbaProject) {
    if !*open {
        return;
    }
    use apex_vba::libraries::{Support, support};
    let refs = project.references();
    egui::Window::new(tr("macros.refs_title")).id(egui::Id::new("macro_refs")).open(open).resizable(true).collapsible(false).default_width(560.0).pivot(egui::Align2::CENTER_CENTER).default_pos(ctx.content_rect().center()).show(ctx, |ui| {
        ui.label(RichText::new(tr("macros.refs_note")).weak());
        ui.separator();
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            egui::Grid::new("macro_refs_grid").num_columns(3).striped(true).spacing([14.0, 6.0]).show(ui, |ui| {
                for key in ["macros.refs_name", "macros.refs_where", "macros.refs_state"] {
                    ui.label(RichText::new(tr(key)).strong());
                }
                ui.end_row();
                for r in &refs {
                    ui.label(&r.name);
                    let what = if r.description.is_empty() { r.path.clone() } else { r.description.clone() };
                    ui.label(what).on_hover_text(&r.path);
                    let (text, color) = match r.guid.as_deref().map(support) {
                        _ if r.project => (tr("macros.refs_project").to_string(), ui.visuals().error_fg_color),
                        Some(Support::Yes) => (tr("macros.refs_yes").to_string(), ui.visuals().text_color()),
                        Some(Support::Partial(what)) => (format!("{} ({what})", tr("macros.refs_partial")), ui.visuals().warn_fg_color),
                        _ => (tr("macros.refs_no").to_string(), ui.visuals().error_fg_color),
                    };
                    ui.label(RichText::new(text).color(color));
                    ui.end_row();
                }
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn properties_apply() {
        let bin = apex_io::vba::new_workbook_project("ThisWorkbook", &[]).unwrap();
        let mut p = apex_io::vba::read(&bin).unwrap();
        let mut d = Props::of(&p);
        d.name = "Bad name".into();
        assert_eq!(d.apply(&mut p), Err("macros.prop_bad_name"));
        d.name = "Tools".into();
        d.lock = true;
        assert_eq!(d.apply(&mut p), Err("macros.prop_no_password"));
        d.password = "pw".into();
        assert_eq!(d.apply(&mut p), Err("macros.prop_mismatch"));
        d.confirm = "pw".into();
        assert_eq!(d.apply(&mut p), Ok(()));
        let back = apex_io::vba::read(&p.write().unwrap()).unwrap();
        assert!(back.protected && back.password_matches("pw") && back.name == "Tools");
        // Opened, unlocked with no password typed.
        let mut back = back;
        back.protected = false;
        let mut d = Props::of(&back);
        assert!(d.had && d.lock);
        d.lock = false;
        assert_eq!(d.apply(&mut back), Ok(()));
        assert!(!apex_io::vba::read(&back.write().unwrap()).unwrap().has_password());
    }
}
