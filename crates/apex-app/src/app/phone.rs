//! The interface on a phone: one bar with the menu, the file's name,
//! Undo, Redo and Save; one row of the toolstrip tab chosen, its groups
//! folded to icons; the document under them, without the status bar. The
//! menu (in the document's place while open) has the open files and the File menu's
//! basics. Tablets keep the whole interface, and so may a phone (the
//! menu's Full interface; View > Phone interface goes back).

use eframe::egui::{self, Color32, RichText, vec2};

use super::{App, View};
use crate::common::{AppCmd, Shell};
use crate::i18n::tr;
use crate::icons::{self, Icon};
use crate::{theme, toolstrip};

/// Height of the bar's buttons: a finger's.
const BAR_H: f32 = 40.0;

/// What the phone interface keeps between frames.
#[derive(Default)]
pub(super) struct Phone {
    /// The menu (drawer) is open.
    drawer: bool,
    /// Documents already fitted to the screen's width (tab ids).
    fitted: Vec<u64>,
}

/// Whether the device is a phone: Android's smallest width under 600 dp
/// (tablets have more); elsewhere when `APEXDESK_PHONE` is set (to try
/// it in a narrow window).
pub(crate) fn phone_device() -> bool {
    #[cfg(test)]
    if let Some(on) = test::FORCE.get() {
        return on;
    }
    #[cfg(target_os = "android")]
    {
        crate::android::small_screen()
    }
    #[cfg(not(target_os = "android"))]
    {
        static SET: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *SET.get_or_init(|| std::env::var_os("APEXDESK_PHONE").is_some_and(|v| v != "0"))
    }
}

/// A screen whose smallest side is `dp` wide (Android's
/// smallestScreenWidthDp; None or 0 when unknown) is a phone's: under
/// 600 dp, Android's line between phones and tablets.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn phone_width(dp: Option<i32>) -> bool {
    dp.is_some_and(|w| w > 0 && w < 600)
}

#[cfg(test)]
pub(super) mod test {
    thread_local! {
        /// This thread's windows are a phone's (Some(true)) or a
        /// tablet's (Some(false)), whatever the process says.
        pub(in crate::app) static FORCE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
    }

    #[test]
    fn phones_and_tablets_by_their_width() {
        // Phones: Galaxy S (360 dp), Pixel (411), a large phone (430), a
        // foldable folded (390).
        for dp in [320, 360, 390, 411, 430, 599] {
            assert!(super::phone_width(Some(dp)), "{dp} dp");
        }
        // Tablets: a 7" one (600), Galaxy Tab (800), a foldable open (673),
        // and a screen that doesn't say.
        for dp in [Some(600), Some(673), Some(800), Some(1200), Some(0), None] {
            assert!(!super::phone_width(dp), "{dp:?}");
        }
    }
}

impl App {
    /// The phone interface shows.
    pub(super) fn phone(&self) -> bool {
        self.settings.phone_ui && phone_device()
    }

    /// The bar, the row of tools and the drawer (instead of the document
    /// tabs and the toolstrip).
    pub(super) fn phone_top(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        let ctx = ui.ctx().clone();
        let bg = theme::workspace_bg(ui.visuals());
        egui::Panel::top("phone_bar").frame(egui::Frame::new().fill(bg).inner_margin(egui::Margin::symmetric(4, 2))).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(BAR_H);
                if bar_button(ui, None, "☰", tr("phone.menu"), self.phone.drawer).clicked() {
                    self.phone.drawer = !self.phone.drawer;
                }
                let has = self.tabs.get(self.active).is_some();
                // Right to left: Save, Redo, Undo, Search; the name takes
                // what is left.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if has && bar_button(ui, Some(Icon::Save), "", tr("menu.save"), false).clicked() {
                        shell.cmd = Some(AppCmd::Save);
                    }
                    if has && bar_button(ui, Some(Icon::Redo), "", tr("edit.redo"), false).clicked() {
                        press(&ctx, egui::Key::Y);
                    }
                    if has && bar_button(ui, Some(Icon::Undo), "", tr("edit.undo"), false).clicked() {
                        press(&ctx, egui::Key::Z);
                    }
                    if bar_button(ui, Some(Icon::Search), "", tr("palette.search"), false).clicked() {
                        self.palette = Some(crate::palette::Palette::new());
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        let Some(t) = self.tabs.get(self.active) else { return };
                        let (badge, color) = t.view.badge();
                        let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), egui::Sense::hover());
                        icons::paint(ui.painter(), r, badge, color);
                        let mut name = t.name();
                        if self.tab_modified(self.active) {
                            name.push_str(" •");
                        }
                        // Tapping the name opens the menu with the open files.
                        let r = ui.add(egui::Label::new(RichText::new(name).strong()).truncate().sense(egui::Sense::click()));
                        if r.clicked() {
                            self.phone.drawer = true;
                        }
                    });
                });
            });
            // What the command search runs and lists.
            let tabs = self.toolstrip_tabs();
            self.toolstrip_tasks(ui, &tabs, shell);
        });
        if self.tabs.get(self.active).is_some() && self.settings.show_toolbar {
            egui::Panel::top("phone_tools").frame(egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(4, 2))).show(ui, |ui| {
                self.phone_tools(ui, shell);
            });
        }
        // A document fits the screen's width when it first shows, without
        // the rulers.
        if let Some(t) = self.tabs.get_mut(self.active)
            && let View::Doc(v) = &mut t.view
            && !self.phone.fitted.contains(&t.id)
        {
            self.phone.fitted.push(t.id);
            v.fit = Some(crate::common::Fit::PageWidth);
            v.set_ruler(false);
        }
        // A PDF keeps the whole width for its pages: no side pane, ever.
        if let Some(t) = self.tabs.get_mut(self.active)
            && let View::Pdf(v) = &mut t.view
        {
            v.pane = crate::pdf_view::Pane::None;
        }
    }

    /// The toolstrip tab chosen (a menu of them) and its groups in one row.
    fn phone_tools(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        let kind = self.toolstrip_kind();
        let tabs = self.toolstrip_tabs();
        let mut cur = self.toolstrip_tab[kind];
        if !tabs.iter().any(|t| t.id == cur) {
            cur = tabs.iter().find(|t| t.id == "home").or(tabs.first()).map_or("home", |t| t.id);
        }
        ui.horizontal(|ui| {
            let label = tabs.iter().find(|t| t.id == cur).map_or("", |t| tr(t.label));
            egui::ComboBox::from_id_salt("phone_tab").selected_text(label).width(110.0).height(f32::INFINITY).show_ui(ui, |ui| {
                for t in &tabs {
                    let mut text = RichText::new(tr(t.label));
                    if let Some(c) = t.context {
                        text = text.color(c);
                    }
                    ui.selectable_value(&mut cur, t.id, text);
                }
            });
            // The rest of the row: the groups fold to what fits.
            toolstrip::with_style(toolstrip::Style::Compact, || self.draw_tab(ui, &tabs, cur, shell));
        });
        self.toolstrip_tab[kind] = cur;
    }

    /// The menu is open: it takes the place of the document.
    pub(super) fn phone_drawer_open(&self) -> bool {
        self.phone.drawer
    }

    /// The menu, where the document shows: the open files, then making,
    /// opening and saving files.
    pub(super) fn phone_drawer(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        egui::ScrollArea::vertical().show(ui, |ui| self.phone_menu(ui, shell));
    }

    fn phone_menu(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        ui.add_space(6.0);
        ui.label(RichText::new("ApexDesk").size(20.0).strong());
        ui.add_space(6.0);
        let mut close = false;
        if !self.tabs.is_empty() {
            ui.label(RichText::new(tr("phone.files")).weak());
            let mut pick = None;
            let mut shut = None;
            for (i, t) in self.tabs.iter().enumerate() {
                ui.horizontal(|ui| {
                    let (badge, color) = t.view.badge();
                    let mut name = t.name();
                    if self.tab_modified(i) {
                        name.push_str(" •");
                    }
                    let w = ui.available_width() - BAR_H - 4.0;
                    let text = if i == self.active { RichText::new(name).strong() } else { RichText::new(name) };
                    if row(ui, Some((badge, color)), text, w, i == self.active).clicked() {
                        pick = Some(i);
                    }
                    if bar_button(ui, None, "×", tr("tab.close"), false).clicked() {
                        shut = Some(i);
                    }
                });
            }
            if let Some(i) = pick {
                self.active = i;
                close = true;
            }
            if let Some(i) = shut {
                self.close_tab_checked(i);
            }
            ui.separator();
        }
        let w = ui.available_width();
        let a = theme::ACCENT;
        let mut cmd = None;
        if row(ui, Some((Icon::Doc, theme::DOC_ACCENT)), RichText::new(tr("menu.new_doc")), w, false).clicked() {
            cmd = Some(AppCmd::NewDoc);
        }
        if row(ui, Some((Icon::Sheet, theme::SHEET_ACCENT)), RichText::new(tr("menu.new_sheet")), w, false).clicked() {
            cmd = Some(AppCmd::NewSheet);
        }
        if row(ui, Some((Icon::Folder, a)), RichText::new(tr("menu.open")), w, false).clicked() {
            cmd = Some(AppCmd::Open);
        }
        if self.tabs.get(self.active).is_some() {
            if row(ui, Some((Icon::Save, a)), RichText::new(tr("menu.save_as")), w, false).clicked() {
                self.save(self.active, true);
                close = true;
            }
            if row(ui, Some((Icon::Pdf, a)), RichText::new(tr("print.export_pdf")), w, false).clicked() {
                // Straight to the file: the dialog with the options is
                // wider than a phone.
                cmd = Some(AppCmd::ExportPdf);
            }
        }
        ui.separator();
        if row(ui, Some((Icon::Options, a)), RichText::new(format!("{}…", tr("options.title"))), w, false).clicked() {
            self.open_options();
            close = true;
        }
        if row(ui, Some((Icon::Fullscreen, a)), RichText::new(tr("phone.full")), w, false).on_hover_text(tr("phone.full_tip")).clicked() {
            self.settings.phone_ui = false;
            self.settings.save();
            close = true;
        }
        if let Some(c) = cmd {
            shell.cmd = Some(c);
            close = true;
        }
        if close {
            self.phone.drawer = false;
        }
    }

    /// The start screen on a phone: new files, Open and the recent files,
    /// one under another.
    pub(super) fn phone_start(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(24.0);
                ui.label(RichText::new("ApexDesk").size(30.0).strong());
                ui.label(RichText::new(tr("start.tagline")).weak());
                ui.add_space(4.0);
                self.lic.badge(ui);
                ui.add_space(20.0);
            });
            let w = ui.available_width();
            if row(ui, Some((Icon::Doc, theme::DOC_ACCENT)), RichText::new(tr("start.new_doc")).strong(), w, false).clicked() {
                self.new_doc();
            }
            if row(ui, Some((Icon::Sheet, theme::SHEET_ACCENT)), RichText::new(tr("start.new_sheet")).strong(), w, false).clicked() {
                self.new_sheet();
            }
            if row(ui, Some((Icon::Notebook, theme::NOTE_ACCENT)), RichText::new(tr("start.new_notebook")).strong(), w, false).clicked() {
                self.new_notebook();
            }
            if row(ui, Some((Icon::Folder, theme::ACCENT)), RichText::new(tr("start.open")).strong(), w, false).clicked() {
                self.open_dialog();
            }
            if !self.settings.recent.is_empty() {
                ui.add_space(16.0);
                ui.label(RichText::new(tr("start.recent")).strong());
                ui.add_space(4.0);
                let mut open = None;
                for p in self.settings.recent.iter().take(12) {
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let icon = match apex_io::Format::from_path(p).map(apex_io::Format::kind) {
                        Some(apex_io::Kind::Sheet) => (Icon::Sheet, theme::SHEET_ACCENT),
                        Some(apex_io::Kind::Slides) => (Icon::Slides, theme::SLIDES_ACCENT),
                        _ => (Icon::Doc, theme::DOC_ACCENT),
                    };
                    if row(ui, Some(icon), RichText::new(name), w, false).on_hover_text(p.display().to_string()).clicked() {
                        open = Some(p.clone());
                    }
                }
                if let Some(p) = open {
                    self.open_path(&p);
                }
            }
        });
    }
}

/// The key `key` with Ctrl (Cmd), as if typed: Undo and Redo of the view
/// that shows (each view does its own).
fn press(ctx: &egui::Context, key: egui::Key) {
    let ev = |pressed| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::COMMAND };
    ctx.input_mut(|i| {
        i.events.push(ev(true));
        i.events.push(ev(false));
    });
}

/// A button of the bar, as big as a finger: an icon or a text.
fn bar_button(ui: &mut egui::Ui, icon: Option<Icon>, text: &str, name: &str, on: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(BAR_H, BAR_H), egui::Sense::click());
    crate::widgets::name(&resp, egui::WidgetType::Button, name, None);
    let v = ui.visuals();
    if on || resp.hovered() || resp.is_pointer_button_down_on() {
        ui.painter().rect_filled(rect.shrink(2.0), 6.0, v.widgets.hovered.weak_bg_fill);
    }
    match icon {
        Some(i) => icons::paint_accent(ui.painter(), egui::Rect::from_center_size(rect.center(), vec2(20.0, 20.0)), i, v.text_color(), theme::ACCENT),
        None => {
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(20.0), v.text_color());
        }
    }
    resp.on_hover_text(name)
}

/// A row of a list, as tall as a finger: an icon and a text.
fn row(ui: &mut egui::Ui, icon: Option<(Icon, Color32)>, text: RichText, width: f32, on: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width.max(BAR_H), BAR_H), egui::Sense::click());
    let galley = egui::WidgetText::from(text.clone()).into_galley(ui, Some(egui::TextWrapMode::Truncate), rect.width() - 44.0, egui::TextStyle::Body);
    crate::widgets::name(&resp, egui::WidgetType::Button, galley.text(), None);
    let v = ui.visuals();
    if on || resp.hovered() || resp.is_pointer_button_down_on() {
        ui.painter().rect_filled(rect, 6.0, v.widgets.hovered.weak_bg_fill);
    }
    if let Some((i, c)) = icon {
        icons::paint(ui.painter(), egui::Rect::from_center_size(egui::pos2(rect.min.x + 20.0, rect.center().y), vec2(20.0, 20.0)), i, c);
    }
    ui.painter().galley(egui::pos2(rect.min.x + 40.0, rect.center().y - galley.size().y / 2.0), galley, v.text_color());
    resp
}
