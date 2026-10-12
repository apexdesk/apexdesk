//! The PDF tab's toolstrip.

use eframe::egui::{self, Id};

use super::{Layout, Pane, PdfView, Request, Tool};
use crate::common::{self, Fit, Shell};
use crate::i18n::tr;
use crate::icons::Icon;
use crate::toolstrip::{self, Body, TabDef};
use crate::theme;

impl PdfView {
    pub fn toolstrip_tabs(&self) -> Vec<TabDef> {
        vec![TabDef::new("home", "toolstrip.home"), TabDef::new("comment", "pdf.tab_comment"), TabDef::new("fill", "pdf.tab_fill_sign"), TabDef::new("pages", "pdf.tab_pages"), TabDef::new("redact", "pdf.tab_redact"), TabDef::new("tools", "pdf.tab_tools"), TabDef::new("view", "menu.view")]
    }

    pub fn toolstrip(&mut self, ui: &mut egui::Ui, tab: &str, shell: &mut Shell) {
        let id = Id::new(("pdf_toolstrip", tab));
        toolstrip::body(ui, id, theme::PDF_ACCENT, |b| match tab {
            "comment" => self.comment_tab(b),
            "fill" => self.fill_tab(b),
            "pages" => self.pages_tab(b),
            "redact" => self.redact_tab(b),
            "tools" => self.tools_tab(b),
            _ => self.home_tab(b, shell),
        });
    }

    fn home_tab(&mut self, b: &mut Body, _shell: &mut Shell) {
        let acc = b.accent();
        b.group("toolstrip.g.undo", Icon::Undo, |ui| {
            toolstrip::column(ui, |ui| {
                for _ in 0..common::history_button(ui, false, &self.doc.undo_names()) {
                    self.undo();
                }
                for _ in 0..common::history_button(ui, true, &self.doc.redo_names()) {
                    self.redo();
                }
            });
        });
        b.group("toolstrip.g.clipboard", Icon::Copy, |ui| {
            toolstrip::column(ui, |ui| {
                ui.add_enabled_ui(self.sel.is_some(), |ui| {
                    if toolstrip::small(ui, Some(Icon::Copy), tr("edit.copy"), &format!("{} (Ctrl+C)", tr("edit.copy")), acc, false).clicked()
                        && let Some(t) = self.selected_text()
                    {
                        ui.ctx().copy_text(t);
                    }
                });
                if toolstrip::small(ui, Some(Icon::Select), tr("pdf.select_page_text"), &format!("{} (Ctrl+A)", tr("pdf.select_page_text")), acc, false).clicked() {
                    self.select_page(self.cur);
                }
            });
        });
        b.group("pdf.g.tools", Icon::Select, |ui| {
            if toolstrip::large(ui, Icon::Select, tr("pdf.tool_select"), tr("pdf.tool_select_tip"), acc, self.tool == Tool::Select).clicked() {
                self.set_tool(Tool::Select);
            }
            if toolstrip::large(ui, Icon::Brush, tr("pdf.tool_hand"), tr("pdf.tool_hand_tip"), acc, self.tool == Tool::Hand).clicked() {
                self.set_tool(Tool::Hand);
            }
        });
        b.group("pdf.g.find", Icon::Search, |ui| {
            if toolstrip::large(ui, Icon::Search, tr("pdf.find"), &format!("{} (Ctrl+F)", tr("pdf.find_tip")), acc, self.find.open).clicked() {
                self.find.open = !self.find.open;
                self.find.focus = self.find.open;
            }
        });
        b.group("pdf.g.show", Icon::Slides, |ui| {
            if toolstrip::large(ui, Icon::Slides, tr("pdf.present"), &format!("{} (F5)", tr("pdf.present_tip")), acc, false).clicked() {
                self.start_show(ui.ctx(), self.cur);
            }
        });
        b.group("pdf.g.convert", Icon::Doc, |ui| {
            if toolstrip::large(ui, Icon::Doc, tr("pdf.convert"), tr("pdf.convert_tip"), acc, false).clicked() {
                self.requests.push(Request::Convert);
            }
            if toolstrip::large(ui, Icon::Print, tr("print.print"), &format!("{} (Ctrl+P)", tr("pdf.print_tip")), acc, false).clicked() {
                self.open_print();
            }
        });
    }

    pub(crate) fn view_tab(&mut self, b: &mut Body) {
        let acc = b.accent();
        b.group("pdf.g.layout", Icon::MultiPage, |ui| {
            for (l, icon, key, tip) in [
                (Layout::Single, Icon::PrintLayout, "pdf.layout_single", "pdf.layout_single_tip"),
                (Layout::Continuous, Icon::Draft, "pdf.layout_continuous", "pdf.layout_continuous_tip"),
                (Layout::Two, Icon::MultiPage, "pdf.layout_two", "pdf.layout_two_tip"),
            ] {
                if toolstrip::large(ui, icon, tr(key), tr(tip), acc, self.layout == l).clicked() {
                    self.layout = l;
                    self.go_to(self.cur, 0.0);
                }
            }
        });
        b.group("pdf.g.rotate", Icon::Orientation, |ui| {
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::Redo), tr("pdf.rotate_view_right"), tr("pdf.rotate_view_tip"), acc, false).clicked() {
                    self.turn = (self.turn + 1) % 4;
                }
                if toolstrip::small(ui, Some(Icon::Undo), tr("pdf.rotate_view_left"), tr("pdf.rotate_view_tip"), acc, false).clicked() {
                    self.turn = (self.turn + 3) % 4;
                }
            });
        });
        b.group("pdf.g.pane", Icon::NavPane, |ui| {
            for (p, icon, key) in [(Pane::Thumbnails, Icon::MultiPage, "pdf.thumbnails"), (Pane::Bookmarks, Icon::Bookmark, "pdf.bookmarks"), (Pane::Comments, Icon::Comment, "pdf.comments")] {
                if toolstrip::large(ui, icon, tr(key), tr("pdf.pane_tip"), acc, self.pane == p).clicked() {
                    self.pane = if self.pane == p { Pane::None } else { p };
                }
            }
        });
        b.group("toolstrip.g.zoom", Icon::Zoom100, |ui| {
            let before = self.zoom;
            common::zoom_group(ui, &mut self.zoom, acc);
            if self.zoom != before {
                self.fit = None;
            }
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::Fullscreen), tr("zoom.page_width"), tr("zoom.page_width"), acc, self.fit == Some(Fit::PageWidth)).clicked() {
                    self.fit = Some(Fit::PageWidth);
                }
                if toolstrip::small(ui, Some(Icon::PrintLayout), tr("zoom.whole_page"), tr("zoom.whole_page"), acc, self.fit == Some(Fit::WholePage)).clicked() {
                    self.fit = Some(Fit::WholePage);
                }
            });
        });
    }
}
