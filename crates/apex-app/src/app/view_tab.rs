//! The View tab every kind of tab has. The view draws its own groups
//! (views, show, zoom); the ones that don't depend on the kind of file
//! follow, drawn here once for all: Window (the view's own window buttons,
//! for documents and sheets) and Screen (full screen, the interface's
//! theme).
//! A new kind of tab gets the tab and those groups without doing anything,
//! and they show the same, in the same place, in the command search and in
//! the classic interface's View menu too.

use eframe::egui::{self, Id};

use super::{App, View};
use crate::common::{self, Shell};
use crate::icons::Icon;
use crate::toolstrip::{self, TabDef};

/// Id of the View tab.
pub(super) const VIEW: &str = "view";

/// `tabs` with the View tab: where the view puts it, or else after its
/// last own tab (before the contextual ones).
pub(super) fn with_view_tab(mut tabs: Vec<TabDef>) -> Vec<TabDef> {
    if !tabs.is_empty() && !tabs.iter().any(|t| t.id == VIEW) {
        let at = tabs.iter().rposition(|t| t.context.is_none()).map_or(tabs.len(), |i| i + 1);
        tabs.insert(at, TabDef::new(VIEW, "menu.view"));
    }
    tabs
}

impl App {
    /// The View tab's body.
    pub(super) fn view_tab(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        let accent = self.accent();
        let kind = self.toolstrip_kind();
        let Some(view) = self.tabs.get_mut(self.active).map(|t| &mut t.view) else { return };
        // The dark workspace darkens pages, cells, slides and notebook
        // pages; PDFs and databases have none of those.
        let workspace = matches!(view, View::Doc(_) | View::Sheet(_) | View::Slides(_) | View::Note(_));
        let macros = matches!(view, View::Doc(_) | View::Sheet(_));
        toolstrip::body(ui, Id::new(("view_toolstrip", kind)), accent, |b| {
            match &mut *view {
                View::Sheet(v) => v.view_tab(b, shell),
                View::Doc(v) => v.view_tab(b),
                View::Slides(v) => v.view_tab(b),
                View::Pdf(v) => v.view_tab(b),
                View::Db(v) => v.view_tab(b),
                View::Note(v) => v.view_tab(b),
            }
            let a = b.accent();
            // Its own windows only: the tabs are always in sight to switch.
            if matches!(view, View::Sheet(_) | View::Doc(_)) {
                b.group("toolstrip.g.window", Icon::Window, |ui| match &mut *view {
                    View::Sheet(v) => v.window_buttons(ui, shell, a),
                    View::Doc(v) => v.window_buttons(ui, shell, a),
                    _ => {}
                });
            }
            common::screen_group(b, shell, workspace);
            if macros {
                common::macros_group(b, shell);
            }
        });
    }
}
