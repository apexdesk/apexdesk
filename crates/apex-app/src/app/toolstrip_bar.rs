//! The top of the window: document tabs with the command search, the
//! toolstrip's tab strip with the File menu, and the toolstrip itself.

use eframe::egui::{self, Color32, RichText, vec2};

use super::{App, View};
use crate::common::Shell;
use crate::i18n::tr;
use crate::icons::Icon;
use crate::palette::{Command, Palette};
use crate::toolstrip::{self, TabDef};
use crate::{icons, theme};

/// Tab id of the File menu's commands in the search.
pub(super) const FILE: &str = "file";

impl App {
    pub(super) fn toolstrip_kind(&self) -> usize {
        match self.tabs.get(self.active).map(|t| &t.view) {
            Some(View::Doc(_)) => 1,
            Some(View::Slides(_)) => 2,
            Some(View::Pdf(_)) => 3,
            Some(View::Db(_)) => 4,
            Some(View::Note(_)) => 5,
            _ => 0,
        }
    }

    /// The view's tabs, and the View tab every view has.
    pub(super) fn toolstrip_tabs(&self) -> Vec<TabDef> {
        let tabs = match self.tabs.get(self.active).map(|t| &t.view) {
            Some(View::Sheet(v)) => v.toolstrip_tabs(),
            Some(View::Doc(v)) => v.toolstrip_tabs(),
            Some(View::Slides(v)) => v.toolstrip_tabs(),
            None => Vec::new(),
            Some(View::Pdf(v)) => v.toolstrip_tabs(),
            Some(View::Db(v)) => v.toolstrip_tabs(),
            Some(View::Note(v)) => v.toolstrip_tabs(),
        };
        let developer = self.settings.developer_tab && matches!(self.tabs.get(self.active).map(|t| &t.view), Some(View::Sheet(_) | View::Doc(_)));
        super::developer_tab::with_developer_tab(super::view_tab::with_view_tab(tabs), developer)
    }

    pub(super) fn accent(&self) -> Color32 {
        match self.tabs.get(self.active).map(|t| &t.view) {
            Some(View::Sheet(_)) => theme::SHEET_ACCENT,
            Some(View::Doc(_)) => theme::DOC_ACCENT,
            Some(View::Slides(_)) => theme::SLIDES_ACCENT,
            None => theme::ACCENT,
            Some(View::Pdf(_)) => theme::PDF_ACCENT,
            Some(View::Db(_)) => theme::DB_ACCENT,
            Some(View::Note(_)) => theme::NOTE_ACCENT,
        }
    }

    /// The search box at the right of the document tabs.
    pub(super) fn search_box(&mut self, ui: &mut egui::Ui) {
        let w = 250.0_f32.min(ui.available_width() * 0.4);
        let (rect, resp) = ui.allocate_exact_size(vec2(w, 24.0), egui::Sense::click());
        crate::widgets::name(&resp, egui::WidgetType::Button, &format!("{} (Alt+Q)", tr("palette.search")), None);
        let v = ui.visuals();
        let stroke = if resp.hovered() { v.widgets.hovered.bg_stroke } else { v.widgets.inactive.bg_stroke };
        ui.painter().rect(rect, 12.0, v.extreme_bg_color, stroke, egui::StrokeKind::Inside);
        let ir = egui::Rect::from_center_size(egui::pos2(rect.min.x + 16.0, rect.center().y), vec2(14.0, 14.0));
        icons::paint(ui.painter(), ir, Icon::Search, v.weak_text_color());
        // A narrow box leaves the key out, then all but the first word,
        // then the text (the tooltip and the name keep it).
        let font = egui::FontId::proportional(12.5);
        let room = rect.width() - 34.0;
        let base = tr("palette.search");
        let fits = |t: &String| ui.painter().layout_no_wrap(t.clone(), font.clone(), Color32::PLACEHOLDER).size().x <= room;
        let text = [format!("{base} (Alt+Q)"), base.to_string(), base.split_whitespace().next().unwrap_or_default().to_string()].into_iter().find(fits).unwrap_or_default();
        ui.painter().with_clip_rect(rect.shrink(4.0)).text(egui::pos2(rect.min.x + 30.0, rect.center().y), egui::Align2::LEFT_CENTER, text, font, v.weak_text_color());
        if resp.on_hover_text(tr("palette.hint")).clicked() {
            self.palette = Some(Palette::new());
        }
    }

    /// The tab strip and, unless folded, the active tab's groups.
    pub(super) fn toolstrip_ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        let kind = self.toolstrip_kind();
        let tabs = self.toolstrip_tabs();
        let mut cur = self.toolstrip_tab[kind];
        // A view that starts with its own tab (Outline) shows it.
        if let Some(View::Doc(v)) = self.tabs.get_mut(self.active).map(|t| &mut t.view)
            && let Some(t) = v.take_wanted_tab()
        {
            cur = t;
        }
        if !tabs.iter().any(|t| t.id == cur) {
            cur = tabs.iter().find(|t| t.id == "home").or(tabs.first()).map_or("home", |t| t.id);
        }
        let folded = !self.settings.show_toolbar || self.tabs.is_empty();
        let strip = toolstrip::tab_strip(ui, &tabs, &mut cur, self.accent(), folded);
        self.toolstrip_tab[kind] = cur;
        if let Some(file) = &strip.file {
            egui::Popup::menu(file).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| self.file_menu(ui));
        }
        if strip.toggle_fold || strip.unfold {
            self.settings.show_toolbar = strip.unfold || !self.settings.show_toolbar;
            self.settings.save();
        }
        if !folded && !strip.toggle_fold {
            ui.add_space(2.0);
            self.draw_tab(ui, &tabs, cur, shell);
        }
        self.toolstrip_tasks(ui, &tabs, shell);
    }

    /// Draws the toolstrip tab `id` in the current [`toolstrip::Style`].
    pub(super) fn draw_tab(&mut self, ui: &mut egui::Ui, tabs: &[TabDef], id: &'static str, shell: &mut Shell) {
        let label = tabs.iter().find(|t| t.id == id).map_or("", |t| tr(t.label));
        if id == super::view_tab::VIEW {
            return toolstrip::within(label, || self.view_tab(ui, shell));
        }
        if id == super::developer_tab::DEVELOPER {
            return toolstrip::within(label, || self.developer_tab(ui, shell));
        }
        toolstrip::within(label, || match self.tabs.get_mut(self.active).map(|t| &mut t.view) {
            Some(View::Sheet(v)) => v.toolstrip(ui, id, shell),
            Some(View::Doc(v)) => v.toolstrip(ui, id, shell),
            Some(View::Slides(v)) => v.toolstrip(ui, id, shell),
            None => {}
            Some(View::Pdf(v)) => v.toolstrip(ui, id, shell),
            Some(View::Db(v)) => v.toolstrip(ui, id, shell),
            Some(View::Note(v)) => v.toolstrip(ui, id, shell),
        });
    }

    /// The classic interface: a menu per toolstrip tab and a toolbar with the
    /// Home tab in one row.
    pub(super) fn classic_ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        let tabs = self.toolstrip_tabs();
        egui::MenuBar::new().ui(ui, |ui| {
            crate::widgets::menu_button(ui, tr("menu.file"), |ui| self.file_menu(ui));
            for t in &tabs {
                let mut text = RichText::new(tr(t.label));
                if let Some(c) = t.context {
                    text = text.color(c);
                }
                crate::widgets::menu_button(ui, text, |ui| {
                    ui.set_min_width(220.0);
                    toolstrip::with_style(toolstrip::Style::Menu, || self.draw_tab(ui, &tabs, t.id, shell));
                });
            }
        });
        if self.settings.show_toolbar && !self.tabs.is_empty() {
            ui.add_space(2.0);
            toolstrip::with_style(toolstrip::Style::Compact, || self.draw_tab(ui, &tabs, "home", shell));
        }
        // A menu chosen in the search: its tab shows as a toolstrip until the
        // menu closes.
        // (Not when the toolbar has opened it.)
        if self.toolstrip_peek == 1 && !toolstrip::menu_pending() {
            self.toolstrip_peek = 0;
        }
        if self.toolstrip_peek > 0 {
            let kind = self.toolstrip_kind();
            let cur = self.toolstrip_tab[kind];
            ui.add_space(2.0);
            self.draw_tab(ui, &tabs, cur, shell);
            if self.toolstrip_peek > 3 && !egui::Popup::is_any_open(ui.ctx()) {
                self.toolstrip_peek = 0;
            } else {
                self.toolstrip_peek = self.toolstrip_peek.saturating_add(1);
                ui.ctx().request_repaint();
            }
        }
        self.toolstrip_tasks(ui, &tabs, shell);
    }

    /// Runs a command chosen in the search and collects the search's
    /// commands.
    pub(super) fn toolstrip_tasks(&mut self, ui: &mut egui::Ui, tabs: &[TabDef], shell: &mut Shell) {
        // A command chosen in the search: its button, clicked off screen.
        if let Some(c) = self.pending_cmd.take() {
            toolstrip::offscreen_ui(ui, |ui| {
                if c.tab == FILE {
                    toolstrip::run(&c.key, || toolstrip::within(tr("menu.file"), || self.file_menu(ui)));
                } else if let Some(t) = tabs.iter().find(|t| t.id == c.tab) {
                    toolstrip::run(&c.key, || self.draw_tab(ui, tabs, t.id, shell));
                }
            });
        }
        if self.palette.as_ref().is_some_and(|p| p.commands.is_none()) {
            let cmds = self.collect_commands(ui, tabs, shell);
            if let Some(p) = &mut self.palette {
                p.commands = Some(cmds);
            }
        }
        if self.want_commands {
            self.want_commands = false;
            self.commands = Some(self.collect_commands(ui, tabs, shell));
        }
    }

    /// Every command of the toolstrip's tabs and the File menu.
    fn collect_commands(&mut self, ui: &mut egui::Ui, tabs: &[TabDef], shell: &mut Shell) -> Vec<Command> {
        let mut out = Vec::new();
        // What a command does can't change the document here: nothing is
        // clicked while collecting.
        let cmd = shell.cmd.take();
        toolstrip::offscreen_ui(ui, |ui| {
            for t in tabs {
                let entries = toolstrip::collect(|| self.draw_tab(ui, tabs, t.id, shell));
                out.extend(entries.into_iter().map(|entry| Command { tab: t.id, entry }));
            }
            let entries = toolstrip::collect(|| toolstrip::within(tr("menu.file"), || self.file_menu(ui)));
            out.extend(entries.into_iter().map(|entry| Command { tab: FILE, entry }));
            // Contextual tabs that aren't showing: found, but not available.
            let hidden = match self.tabs.get(self.active).map(|t| &t.view) {
                Some(View::Doc(_)) => crate::doc_view::DocView::contextual_commands(ui, shell, tabs),
                Some(View::Sheet(_)) => crate::sheet_view::SheetView::contextual_commands(ui, shell, tabs),
                Some(View::Slides(_)) => crate::slides_view::SlidesView::contextual_commands(ui, shell, tabs),
                None => Vec::new(),
                Some(View::Pdf(_)) | Some(View::Db(_)) | Some(View::Note(_)) => Vec::new(),
            };
            for (t, entries) in hidden {
                out.extend(entries.into_iter().map(|entry| Command { tab: t.id, entry: toolstrip::Entry { enabled: false, ..entry } }));
            }
        });
        shell.cmd = cmd;
        out
    }

    /// Opens the command search's choice: a menu opens on the toolstrip, the
    /// rest runs on the next frame.
    pub(super) fn palette_ui(&mut self, ctx: &egui::Context) {
        let Some(p) = &mut self.palette else { return };
        let Some(res) = p.ui(ctx) else {
            // The views don't see the keys typed into the search.
            ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Key { .. } | egui::Event::Text(_) | egui::Event::Paste(_) | egui::Event::Copy | egui::Event::Cut)));
            return;
        };
        self.palette = None;
        if let Some(c) = res {
            self.choose(ctx, c);
        }
    }
}
