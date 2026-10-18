//! The Developer tab (File > Options > General shows it): the VBA editor,
//! the macros and their security, and in workbooks the form controls.
//! Off at first, as in Office; it shows after the View tab in workbooks
//! and documents.

use eframe::egui::{self, Id};

use super::{App, View};
use crate::common::{self, Shell};
use crate::i18n::tr;
use crate::toolstrip::{self, TabDef};

/// Id of the Developer tab.
pub(super) const DEVELOPER: &str = "developer";

/// `tabs` with the Developer tab after the View tab, when it shows (`on`).
pub(super) fn with_developer_tab(mut tabs: Vec<TabDef>, on: bool) -> Vec<TabDef> {
    if on && !tabs.iter().any(|t| t.id == DEVELOPER) {
        let at = tabs.iter().position(|t| t.id == super::view_tab::VIEW).map_or(tabs.len(), |i| i + 1);
        tabs.insert(at, TabDef::new(DEVELOPER, "developer.tab"));
    }
    tabs
}

impl App {
    /// The Developer tab's body.
    pub(super) fn developer_tab(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        let accent = self.accent();
        let kind = self.toolstrip_kind();
        let names = self.tabs.get(self.active).map(|t| t.id).map(|id| self.macro_names(id)).unwrap_or_default();
        let (mut run, mut lib) = (None, None);
        let Some(view) = self.tabs.get_mut(self.active).map(|t| &mut t.view) else { return };
        toolstrip::body(ui, Id::new(("developer_toolstrip", kind)), accent, |b| {
            common::developer_code_group(b, shell);
            if !names.is_empty() {
                run = macro_buttons(b, &names);
            }
            if let View::Sheet(v) = view {
                lib = library_buttons(b);
                v.developer_controls(b);
            }
        });
        if let Some(name) = run {
            self.run_active_macro(name);
        }
        match lib {
            Some(Lib::Personal) => self.open_personal_workbook(),
            Some(Lib::AddIns) => self.open_macro_options(),
            None => {}
        }
    }
}

enum Lib {
    Personal,
    AddIns,
}

/// The personal macro workbook and the add-ins.
fn library_buttons(b: &mut toolstrip::Body) -> Option<Lib> {
    let a = b.accent();
    let mut lib = None;
    b.group("macros.libraries", crate::icons::Icon::Book, |ui| {
        toolstrip::column(ui, |ui| {
            if toolstrip::small(ui, Some(crate::icons::Icon::Book), tr("macros.personal"), tr("macros.personal_tip"), a, false).clicked() {
                lib = Some(Lib::Personal);
            }
            if toolstrip::small(ui, Some(crate::icons::Icon::Folder), &format!("{}…", tr("macros.addins")), tr("macros.addins_tip"), a, false).clicked() {
                lib = Some(Lib::AddIns);
            }
        });
    });
    lib
}

/// Most macros the group shows (the rest: Alt+F8).
const MAX_BUTTONS: usize = 12;

/// The group of the workbook's macros, a button each (three to a column);
/// the one clicked.
fn macro_buttons(b: &mut toolstrip::Body, names: &[String]) -> Option<String> {
    let a = b.accent();
    let mut run = None;
    b.group("macros.this_book", crate::icons::Icon::SpeakPlay, |ui| {
        for col in names[..names.len().min(MAX_BUTTONS)].chunks(3) {
            toolstrip::column(ui, |ui| {
                for n in col {
                    if toolstrip::small(ui, Some(crate::icons::Icon::SpeakPlay), n, "", a, false).clicked() {
                        run = Some(n.clone());
                    }
                }
            });
        }
    });
    run
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn after_the_view_tab_when_on() {
        let tabs = || super::super::view_tab::with_view_tab(vec![TabDef::new("home", "x"), TabDef::new("insert", "y")]);
        let ids = |t: Vec<TabDef>| t.iter().map(|t| t.id).collect::<Vec<_>>();
        assert_eq!(ids(with_developer_tab(tabs(), false)), ["home", "insert", "view"]);
        assert_eq!(ids(with_developer_tab(tabs(), true)), ["home", "insert", "view", "developer"]);
        assert!(with_developer_tab(Vec::new(), true).iter().any(|t| t.id == DEVELOPER));
    }
}
