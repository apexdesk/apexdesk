//! The File menu: file commands on the left (Open first, then New with
//! every kind of file, as in Office), the less used ones in submenus
//! (Export, Share, Protect and sign, Info, Tools), and recent files on
//! the right. Commands that don't apply to the active tab are
//! hidden, and so is a submenu left empty.

use eframe::egui::{self, Color32, RichText, vec2};

use super::{App, SHORTCUT_CLOSE, SHORTCUT_NEW_DOC, SHORTCUT_NEW_SHEET, SHORTCUT_OPEN, SHORTCUT_PRINT, SHORTCUT_QUIT, SHORTCUT_SAVE, SHORTCUT_SAVE_AS, View};
use crate::common::AppCmd;
use crate::i18n::tr;
use crate::icons::Icon;
use crate::toolstrip;

/// What the active tab is (all false without tabs).
#[derive(Clone, Copy, Default)]
struct Active {
    has: bool,
    doc: bool,
    sheet: bool,
    slides: bool,
    db: bool,
}

impl Active {
    fn of(app: &App) -> Active {
        let Some(t) = app.tabs.get(app.active) else { return Active::default() };
        let v = &t.view;
        Active { has: true, doc: matches!(v, View::Doc(_)), sheet: matches!(v, View::Sheet(_)), slides: matches!(v, View::Slides(_)), db: matches!(v, View::Db(_)) }
    }

    /// A spreadsheet, document or presentation.
    fn plain(self) -> bool {
        self.doc || self.sheet || self.slides
    }
}

/// A command of the menu (closes it when clicked); `keys`: its shortcut.
fn item(ui: &mut egui::Ui, a: Color32, icon: Icon, label: &str, keys: &str) -> bool {
    let hover = if keys.is_empty() { String::new() } else { format!("{label} ({keys})") };
    let r = toolstrip::small(ui, Some(icon), label, &hover, a, false);
    if r.clicked() {
        ui.close();
    }
    r.clicked()
}

/// [`item`] with a tip.
fn tip_item(ui: &mut egui::Ui, a: Color32, icon: Icon, label: &str, tip: &str) -> bool {
    let r = toolstrip::small(ui, Some(icon), label, tip, a, false);
    if r.clicked() {
        ui.close();
    }
    r.clicked()
}

fn dots(key: &'static str) -> String {
    format!("{}…", tr(key))
}

impl App {
    /// The File menu, on the toolstrip, the classic menu bar and the start
    /// screen.
    pub(super) fn file_menu(&mut self, ui: &mut egui::Ui) {
        let mut cmd: Option<AppCmd> = None;
        ui.horizontal_top(|ui| {
            toolstrip::column(ui, |ui| {
                ui.set_min_width(230.0);
                ui.set_max_width(260.0);
                self.file_commands(ui, &mut cmd);
            });
            ui.separator();
            if let Some(p) = self.recent_files(ui) {
                cmd = Some(AppCmd::OpenPath(p));
            }
        });
        if let Some(c) = cmd {
            self.run_cmd(ui.ctx(), c);
        }
    }

    /// The left column.
    fn file_commands(&mut self, ui: &mut egui::Ui, cmd: &mut Option<AppCmd>) {
        let a = self.accent();
        let on = Active::of(self);
        let keys = |s: &egui::KeyboardShortcut| ui.ctx().format_shortcut(s);
        let (k_doc, k_sheet, k_open, k_save, k_save_as, k_print, k_close, k_quit) =
            (keys(&SHORTCUT_NEW_DOC), keys(&SHORTCUT_NEW_SHEET), keys(&SHORTCUT_OPEN), keys(&SHORTCUT_SAVE), keys(&SHORTCUT_SAVE_AS), keys(&SHORTCUT_PRINT), keys(&SHORTCUT_CLOSE), keys(&SHORTCUT_QUIT));
        // Open (also PDF files and databases).
        if item(ui, a, Icon::Folder, tr("menu.open"), &k_open) {
            *cmd = Some(AppCmd::Open);
        }
        toolstrip::small_sub_menu(ui, Some(Icon::New), tr("menu.new"), a, |ui| {
            if item(ui, a, Icon::Doc, tr("menu.new_doc"), &k_doc) {
                *cmd = Some(AppCmd::NewDoc);
            }
            if item(ui, a, Icon::Sheet, tr("menu.new_sheet"), &k_sheet) {
                *cmd = Some(AppCmd::NewSheet);
            }
            if item(ui, a, Icon::Slides, tr("menu.new_slides"), "") {
                *cmd = Some(AppCmd::NewSlides);
            }
            // Android leaves the database tab out.
            #[cfg(not(target_os = "android"))]
            if tip_item(ui, a, Icon::Table, &dots("db.new_database"), tr("db.new_database_tip")) {
                self.new_database();
            }
            if tip_item(ui, a, Icon::Notebook, &dots("nb.new_notebook"), tr("nb.new_notebook_tip")) {
                self.new_notebook();
            }
            if item(ui, a, Icon::Template, &dots("tpl.new_menu"), "") {
                self.template_gallery = Some(crate::templates::Gallery::new(None));
            }
        });
        if on.has {
            ui.separator();
            if item(ui, a, Icon::Save, tr("menu.save"), &k_save) {
                *cmd = Some(AppCmd::Save);
            }
            if item(ui, a, Icon::Save, tr("menu.save_as"), &k_save_as) {
                self.save(self.active, true);
            }
            if item(ui, a, Icon::Save, &dots("enc.save"), "") {
                self.save_with_encoding();
            }
            if crate::templates::can_save()
                && item(ui, a, Icon::Template, &dots("tpl.save_menu"), "")
                && let Some(t) = self.tabs.get(self.active)
            {
                let name = t.path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                self.save_template = Some((t.id, crate::templates::SaveDialog::new(t.view.kind(), name)));
            }
            ui.separator();
            if item(ui, a, Icon::Pdf, &dots("print.export_pdf"), "") {
                *cmd = Some(AppCmd::ExportPdfDialog);
            }
            toolstrip::small_sub_menu(ui, Some(Icon::Export), tr("file.export"), a, |ui| {
                self.export_menu(ui, on);
            });
            if item(ui, a, Icon::Print, &dots("print.print"), &k_print) {
                *cmd = Some(AppCmd::Print);
            }
        } else {
            ui.separator();
        }
        toolstrip::small_sub_menu(ui, Some(Icon::Control), tr("file.tools"), a, |ui| {
            self.tools_menu(ui, on);
        });
        if on.has {
            toolstrip::small_sub_menu(ui, Some(Icon::Envelope), tr("file.share"), a, |ui| {
                if item(ui, a, Icon::Envelope, tr("mail.send"), "") {
                    self.send_by_mail(ui.ctx(), self.active, false);
                }
                if item(ui, a, Icon::Envelope, tr("mail.send_pdf"), "") {
                    self.send_by_mail(ui.ctx(), self.active, true);
                }
            });
            toolstrip::small_sub_menu(ui, Some(Icon::Protect), tr("file.protect"), a, |ui| {
                self.protect_menu(ui);
            });
            // A database has no properties, fonts or hidden data to inspect.
            if !on.db {
                toolstrip::small_sub_menu(ui, Some(Icon::Note), tr("file.info"), a, |ui| {
                        self.info_menu(ui, on);
                });
            }
        }
        ui.separator();
        // Only sheets and documents have a page setup; slides set their
        // size in the Design tab.
        if (on.doc || on.sheet) && item(ui, a, Icon::PageSize, &dots("page.setup"), "") {
            match &mut self.tabs[self.active].view {
                View::Sheet(v) => v.open_page_setup(0),
                View::Doc(v) => v.open_page_setup(0),
                View::Slides(_) | View::Pdf(_) | View::Db(_) | View::Note(_) => {}
            }
        }
        // The language is chosen in Options.
        if item(ui, a, Icon::Options, &dots("options.title"), "") {
            self.open_options();
        }
        // Also on the start screen, which has no View tab.
        let dark = self.settings.dark;
        toolstrip::small_sub_menu(ui, Some(Icon::Theme), tr("menu.theme"), a, |ui| {
            if let Some(v) = crate::common::theme_choices(ui, dark) {
                *cmd = Some(AppCmd::Theme(v));
            }
        });
        if item(ui, a, Icon::Keyboard, tr("menu.shortcuts"), "") {
            self.show_shortcuts = true;
            self.focus_shortcuts = true;
        }
        if item(ui, a, Icon::Help, tr("menu.about"), "") {
            self.show_about = true;
        }
        ui.separator();
        if on.has && item(ui, a, Icon::Close, tr("menu.close_tab"), &k_close) {
            self.request_close_tab(self.active);
        }
        if item(ui, a, Icon::Close, tr("menu.quit"), &k_quit) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// File > Export (Export to PDF is beside it, in the menu itself).
    fn export_menu(&mut self, ui: &mut egui::Ui, on: Active) {
        let a = self.accent();
        if item(ui, a, Icon::Pdf, &dots("sign.export"), "") {
            self.open_sign_pdf();
        }
        if item(ui, a, Icon::Export, &dots("export.html"), "") {
            self.export_web(self.active, false);
        }
        if on.doc && item(ui, a, Icon::Export, &dots("export.epub"), "") {
            self.export_web(self.active, true);
        }
        if on.doc && item(ui, a, Icon::Export, &dots("export.tex"), "") {
            self.export_tex(self.active);
        }
        if on.sheet {
            if item(ui, a, Icon::Export, &dots("export.markdown"), "") {
                self.export_sheet_text(self.active, "md");
            }
            if item(ui, a, Icon::Export, &dots("export.text"), "") {
                self.export_sheet_text(self.active, "txt");
            }
            if item(ui, a, Icon::Export, &dots("export.all_csv"), "") {
                self.export_sheets_csv(self.active);
            }
        }
        if on.slides && item(ui, a, Icon::Export, &dots("slides.export_png"), "") {
            self.export_slide_pictures(ui.ctx(), self.active);
        }
    }

    /// File > Protect and sign.
    fn protect_menu(&mut self, ui: &mut egui::Ui) {
        let a = self.accent();
        if item(ui, a, Icon::Protect, &dots("password.encrypt_title"), "") {
            self.open_encrypt_dialog();
        }
        if item(ui, a, Icon::Protect, &dots("sign.title"), "") {
            self.open_signatures();
        }
        // Disabled, not hidden: its tip says why it can't be signed.
        let blocked = self.sign_doc_blocked();
        ui.add_enabled_ui(blocked.is_none(), |ui| {
            let r = toolstrip::small(ui, Some(Icon::Protect), &dots("sign.doc"), "", a, false).on_disabled_hover_text(blocked.unwrap_or_default());
            if r.clicked() {
                ui.close();
                self.open_sign_doc();
            }
        });
        if item(ui, a, Icon::Protect, &dots("sign.extend"), "") {
            self.open_extend_signatures();
        }
    }

    /// File > Info (not for a database).
    fn info_menu(&mut self, ui: &mut egui::Ui, on: Active) {
        let a = self.accent();
        if item(ui, a, Icon::Options, &dots("props.title"), "") {
            self.open_properties();
        }
        if on.plain() && tip_item(ui, a, Icon::ErrorCheck, &dots("fi.title"), tr("fi.tip")) {
            self.open_inspector();
        }
        if on.plain() && crate::license::has("management") && tip_item(ui, a, Icon::Accept, &dots("check.title"), tr("check.tip")) {
            self.open_check();
        }
        if on.plain() && crate::license::has("management") && tip_item(ui, a, Icon::TrackChanges, &dots("control.title"), tr("control.tip")) {
            self.open_control();
        }
        if on.doc {
            // Word's "Embed fonts in the file": the fonts in use go into the
            // file on the next save.
            let embed = matches!(self.tabs.get(self.active).map(|t| &t.view), Some(View::Doc(v)) if v.doc.fonts.embed);
            let r = toolstrip::small(ui, Some(Icon::Save), tr("fonts.embed"), tr("fonts.embed_tip"), a, embed);
            if r.clicked()
                && let View::Doc(v) = &mut self.tabs[self.active].view
            {
                v.toggle_embed_fonts();
            }
            // Also in View, in a group of its own.
            let book = matches!(self.tabs.get(self.active).map(|t| &t.view), Some(View::Doc(v)) if v.is_book());
            let label = if book { tr("book.already").to_string() } else { dots("book.confirm_title") };
            let r = ui.add_enabled_ui(!book, |ui| tip_item(ui, a, Icon::Book, &label, tr("view.book_mode_tip"))).inner;
            if r && let View::Doc(v) = &mut self.tabs[self.active].view {
                v.ask_book_mode();
            }
        }
    }

    /// File > Tools: also on the start screen (but Customize, which needs a
    /// tab).
    fn tools_menu(&mut self, ui: &mut egui::Ui, on: Active) {
        let a = self.accent();
        if item(ui, a, Icon::Export, &dots("convert.title"), "") {
            self.open_convert();
        }
        if crate::license::has("management") && tip_item(ui, a, Icon::Export, &dots("batch.title"), tr("batch.tip")) {
            self.open_batch();
        }
        if tip_item(ui, a, Icon::Pdf, &dots("dossier.title"), tr("dossier.tip")) {
            self.open_dossier();
        }
        if on.has && item(ui, a, Icon::Keyboard, &dots("customize.title"), "") {
            self.open_customize(0);
        }
        if tip_item(ui, a, Icon::Link, &dots("mcp.title"), tr("mcp.tip")) {
            self.open_mcp();
        }
        if item(ui, a, Icon::Protect, &dots("policy.title"), "") {
            self.show_policy = true;
        }
        if item(ui, a, Icon::ErrorCheck, &dots("diag.title"), "") {
            self.show_diag = true;
        }
        if item(ui, a, Icon::Folder, &dots("enc.open"), "") {
            self.open_with_encoding();
        }
    }

    /// The right column: a recent file clicked.
    fn recent_files(&mut self, ui: &mut egui::Ui) -> Option<std::path::PathBuf> {
        let mut open = None;
        ui.vertical(|ui| {
            ui.set_min_width(260.0);
            ui.set_max_width(340.0);
            ui.label(RichText::new(tr("start.recent")).strong());
            ui.add_space(4.0);
            if self.settings.recent.is_empty() {
                ui.label(RichText::new(tr("toolbar.no_recent")).weak());
            }
            for p in self.settings.recent.iter().take(12) {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let dir = p.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
                let r = ui.add(egui::Button::new(RichText::new(name).strong()).right_text(RichText::new(short_dir(&dir)).weak().small()).frame(false).min_size(vec2(260.0, 22.0)));
                if r.on_hover_text(p.to_string_lossy()).clicked() {
                    open = Some(p.clone());
                    ui.close();
                }
            }
        });
        open
    }
}

/// The end of a long folder path.
fn short_dir(dir: &str) -> String {
    let n = dir.chars().count();
    if n <= 34 { dir.to_string() } else { format!("…{}", dir.chars().skip(n - 33).collect::<String>()) }
}
