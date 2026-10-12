//! Application shell: tabs, menus, file dialogs, notifications.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use apex_doc::Document;
use apex_io::{Format, Kind, Loaded};
use apex_sheet::Workbook;
use eframe::egui::{self, Align2, Color32, Key, KeyboardShortcut, Modifiers, Rect, RichText};

use crate::doc_view::DocView;
use crate::i18n::{self, tr, trf};
use crate::settings::Settings;
use crate::common::{self, AppCmd, Shell};
use crate::icons::{self, Icon};
use crate::sheet_view::SheetView;
use crate::slides_view::SlidesView;
use crate::{fonts, theme, widgets};

#[cfg(test)]
mod audit;
#[cfg(test)]
mod qa;
#[cfg_attr(not(feature = "empresa"), path = "app/batch_ui_free.rs")]
mod batch_ui;
mod dossier_ui;
#[cfg(feature = "empresa")]
pub(crate) use dossier_ui::make as make_dossier;
mod convert_ui;
mod encoding_ui;
mod file_menu;
mod fullscreen;
mod inspector_ui;
#[cfg_attr(not(feature = "empresa"), path = "app/check_ui_free.rs")]
mod check_ui;
#[cfg_attr(not(feature = "empresa"), path = "app/license_ui_free.rs")]
mod license_ui;
#[cfg_attr(not(feature = "empresa"), path = "app/control_ui_free.rs")]
mod control_ui;
mod mcp_ui;
mod policy_ui;
mod diag_ui;
mod customize;
mod mail_ui;
mod macro_run;
mod macros_ui;
mod protected;
mod locks;
mod objects;
mod options;
mod passwords;
mod pdf_tab;
mod db_tab;
mod note_tab;
mod pdf_task;
mod properties;
mod recovery;
mod toolstrip_bar;
mod signatures_ui;
mod vba_code;
mod view_tab;
mod windows;

pub(crate) use options::{decode as decode_list, encode as encode_list};
pub(crate) use protected::from_internet;

/// Third-party license notices, shown from the About dialog (see
/// `THIRD-PARTY-NOTICES.md` and `scripts/third_party_notices.py` at the
/// repository root).
const THIRD_PARTY_NOTICES: &str = include_str!("../../../THIRD-PARTY-NOTICES.md");
#[cfg(feature = "empresa")]
/// The license agreement, in English and in Spanish (the one that prevails).
#[cfg(feature = "empresa")]
const EULA: &str = include_str!("../../../EULA.md");
#[cfg(feature = "empresa")]
const EULA_ES: &str = include_str!("../../../EULA.es.md");
/// The public build's license, the GNU GPL, whatever the language.
#[cfg(not(feature = "empresa"))]
const EULA: &str = include_str!("../../../LICENSE");
#[cfg(not(feature = "empresa"))]
const EULA_ES: &str = EULA;

/// A message the views want to show to the user.
#[derive(Clone, Debug)]
pub struct Notice {
    pub text: String,
    pub error: bool,
}

impl Notice {
    pub fn info(text: impl Into<String>) -> Self {
        Notice { text: text.into(), error: false }
    }

    pub fn error(text: impl Into<String>) -> Self {
        let text = text.into();
        crate::diag::error(&text);
        Notice { text, error: true }
    }

    /// What was asked can't be done with what was given (a formula
    /// mistyped, no data chosen): shown as an error, but not one of the
    /// program's (not logged).
    pub fn refused(text: impl Into<String>) -> Self {
        Notice { text: text.into(), error: true }
    }
}

pub enum View {
    Sheet(Box<SheetView>),
    Doc(Box<DocView>),
    Slides(Box<SlidesView>),
    Pdf(Box<crate::pdf_view::PdfView>),
    Db(Box<crate::db_view::DbView>),
    Note(Box<crate::note_view::NoteView>),
}

impl View {
    fn modified(&self) -> bool {
        match self {
            View::Sheet(v) => v.wb.modified,
            View::Doc(v) => v.is_modified(),
            View::Slides(v) => v.is_modified(),
            View::Pdf(v) => v.is_modified(),
            View::Db(v) => v.is_modified(),
            View::Note(v) => v.is_modified(),
        }
    }

    /// What it shows differs from what its file keeps and other programs
    /// show: formulas worked out to other values, fields to other results.
    fn shown_otherwise(&self) -> bool {
        match self {
            View::Sheet(v) => v.wb.loaded_stale,
            View::Doc(v) => v.fields_stale(),
            _ => false,
        }
    }

    fn kind(&self) -> Kind {
        match self {
            View::Sheet(_) => Kind::Sheet,
            View::Doc(_) => Kind::Doc,
            View::Slides(_) => Kind::Slides,
            // Kept apart where it matters (saving, the toolstrip).
            View::Pdf(_) | View::Db(_) | View::Note(_) => Kind::Doc,
        }
    }

    /// The icon and colour of its tab.
    fn badge(&self) -> (Icon, Color32) {
        match self {
            View::Sheet(_) => (Icon::Sheet, theme::SHEET_ACCENT),
            View::Doc(_) => (Icon::Doc, theme::DOC_ACCENT),
            View::Slides(_) => (Icon::Slides, theme::SLIDES_ACCENT),
            View::Pdf(_) => (Icon::Pdf, theme::PDF_ACCENT),
            View::Db(_) => (Icon::Table, theme::DB_ACCENT),
            View::Note(_) => (Icon::Notebook, theme::NOTE_ACCENT),
        }
    }
}

pub struct Tab {
    id: u64,
    path: Option<PathBuf>,
    format: Option<Format>,
    /// Display name for untitled tabs ("Document 2").
    untitled: String,
    view: View,
    /// Windows of one workbook (View > New Window) share this group id;
    /// the one shown holds the workbook, the others a placeholder.
    link: Option<u64>,
    window_no: u32,
    holds: bool,
    /// The password its file is encrypted with.
    password: Option<String>,
    /// The embedded object it edits.
    embed: Option<objects::EmbedLink>,
    /// The encoding a text file (csv, txt, md) is read and saved in.
    encoding: Option<apex_io::Encoding>,
    /// Made from a file that came from the Internet (a template, a copy,
    /// an object in one): what is saved from it is marked so too.
    downloaded: bool,
}

impl Tab {
    /// Whether it came from the Internet, by its file or what it was made from.
    fn downloaded(&self) -> bool {
        self.downloaded || self.path.as_deref().is_some_and(protected::from_internet)
    }

    fn name(&self) -> String {
        let name = match &self.path {
            Some(p) => p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            None => self.untitled.clone(),
        };
        fonts::note_text(&name);
        if self.window_no > 0 { format!("{name}:{}", self.window_no) } else { name }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Closing {
    Tab(u64),
    /// Closing the window: tabs are checked one by one.
    App,
}

struct Toast {
    notice: Notice,
    at: Instant,
}

pub struct App {
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
    untitled_count: [u32; 3],
    labels_count: u32,
    settings: Settings,
    toasts: Vec<Toast>,
    closing: Option<Closing>,
    /// The user confirmed quitting; let the next close request through.
    quit_confirmed: bool,
    /// Frames left to ask again for the maximized window the last session
    /// ended with: X11 window managers (XWayland too) ignore it at creation.
    restore_maximized: u8,
    show_shortcuts: bool,
    shortcut_query: String,
    shortcut_page: usize,
    focus_shortcuts: bool,
    show_about: bool,
    show_licenses: bool,
    show_eula: bool,
    /// The business license, its dialog and seats (see `license_ui`).
    lic: license_ui::LicenseState,
    title: String,
    fullscreen: fullscreen::FullScreen,
    /// Scripted input for end-to-end checks (see `script.rs`).
    script: Option<crate::script::Script>,
    /// Process start, to report the time to the first frame
    /// (`APEXDESK_STARTUP_EXIT=1` prints it and quits).
    started: Option<Instant>,
    shot_pending: Option<PathBuf>,
    /// File dragged over the window by the script.
    script_hover: Option<PathBuf>,
    /// Files sent by other ApexDesk processes (see `instance`).
    incoming: Option<std::sync::mpsc::Receiver<PathBuf>>,
    /// Tool calls from `apexdesk mcp` (see `mcp_ui`).
    mcp: Option<std::sync::mpsc::Receiver<crate::mcp::link::Request>>,
    mcp_dlg: bool,
    /// Frame CPU times collected by a script's `perf start`.
    perf: Option<Vec<f32>>,
    /// Time spent in our own `ui` code per frame (same collection window).
    perf_ui: Vec<f32>,
    /// Wall time between frames (includes painting and presenting).
    perf_wall: Vec<f32>,
    perf_last: Option<Instant>,
    print: Option<crate::print::PrintDialog>,
    /// Print or export requested by a view (handled with the context).
    pending_print: Option<AppCmd>,
    script_pdf: Option<PathBuf>,
    /// Save as > PDF: the tab to export there (it stays as it was).
    save_pdf_to: Option<(u64, PathBuf)>,
    /// A PDF being written (exported or printed), a few pages per frame.
    pdf_task: Option<pdf_task::PdfTask>,
    template_gallery: Option<crate::templates::Gallery>,
    save_template: Option<(u64, crate::templates::SaveDialog)>,
    side: Option<windows::SideBySide>,
    options: Option<options::OptionsDlg>,
    /// Selected toolstrip tab of spreadsheets and of documents.
    toolstrip_tab: [&'static str; 6],
    palette: Option<crate::palette::Palette>,
    /// The window catching an idea for a notebook's inbox.
    capture: Option<crate::note_view::inbox::Capture>,
    /// Customize the Quick Access Toolbar and shortcuts.
    customize: Option<customize::CustomizeDlg>,
    /// Every command, for Customize (collected when `want_commands`).
    commands: Option<Vec<crate::palette::Command>>,
    want_commands: bool,
    /// A command chosen in the search, run on the next frame.
    pending_cmd: Option<crate::palette::Chosen>,
    /// Frames the toolstrip has shown in the classic interface, for a menu
    /// chosen in the search (0: not shown).
    toolstrip_peek: u8,
    recovery: recovery::Recovery,
    password_prompt: Option<passwords::Prompt>,
    encrypt_dlg: Option<passwords::EncryptDlg>,
    /// A file opened read-only as its author recommends: edit it after all?
    read_only_ask: Option<PathBuf>,
    props_dlg: Option<properties::PropsDlg>,
    control_dlg: Option<control_ui::ControlDlg>,
    inspector: Option<inspector_ui::InspectorDlg>,
    check: Option<check_ui::CheckDlg>,
    /// The tab the check before sharing let through, for the next share.
    check_passed: Option<u64>,
    /// File > Convert Files.
    convert_dlg: Option<convert_ui::ConvertDlg>,
    /// File > Batch Processing.
    batch_dlg: Option<batch_ui::BatchDlg>,
    /// File > PDF Dossier.
    dossier_dlg: Option<dossier_ui::DossierDlg>,
    /// File > Open or Save with Encoding.
    encoding_dlg: Option<encoding_ui::EncodingDlg>,
    /// File > Organization Policies is open.
    show_policy: bool,
    /// File > Diagnostics, and whether it opened for a crash.
    show_diag: bool,
    diag_crashed: bool,
    /// A message being composed by the mail program (File > Send by E-mail).
    mail_rx: Option<std::sync::mpsc::Receiver<mail_ui::MailResult>>,
    /// Macro bars, the macro editor and the macro-free save question.
    macros: macros_ui::MacroUi,
    /// Running macros, their dialogs and events.
    macro_run: macro_run::MacroRun,
    /// Signed files, their bar and details, and signing PDF exports.
    sig: signatures_ui::SigUi,
    /// Lock files, read-only files and files changed on disk.
    locks: locks::Locks,
    /// Tabs of downloaded files in Protected View.
    protected: protected::Protected,
    /// The context, for work outside the frame's code (previews).
    ctx: egui::Context,
}

const SHORTCUT_NEW_DOC: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::N);
const SHORTCUT_NEW_SHEET: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::N);
const SHORTCUT_OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
const SHORTCUT_SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
const SHORTCUT_SAVE_AS: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::S);
const SHORTCUT_PRINT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::P);
const SHORTCUT_CLOSE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::W);
const SHORTCUT_QUIT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Q);
const SHORTCUT_NEXT_TAB: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::Tab);
const SHORTCUT_PREV_TAB: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL.plus(Modifiers::SHIFT), Key::Tab);
const SHORTCUT_ZOOM_IN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Plus);
const SHORTCUT_ZOOM_IN2: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Equals);
const SHORTCUT_ZOOM_OUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Minus);
const SHORTCUT_ZOOM_RESET: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Num0);
const SHORTCUT_FULLSCREEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::F11);
const SHORTCUT_PALETTE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::ALT, Key::Q);
const SHORTCUT_CAPTURE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::I);
const SHORTCUT_FOLD_TOOLSTRIP: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::F1);

impl App {
    pub fn new(cc: &eframe::CreationContext, files: Vec<PathBuf>, incoming: Option<std::sync::mpsc::Receiver<PathBuf>>, started: Instant) -> App {
        let ctx = &cc.egui_ctx;
        let settings = Settings::load();
        init(ctx, &settings, started);
        let mut app = App::blank(ctx, settings, incoming, started);
        for f in files {
            app.open_arg(&f);
        }
        app
    }

    /// The window's state with `settings`, before any file is opened (the
    /// context and the process set up by [`init`]).
    fn blank(ctx: &egui::Context, settings: Settings, incoming: Option<std::sync::mpsc::Receiver<PathBuf>>, started: Instant) -> App {
        // A crash report saved since the last start: say so once.
        let crashed = std::env::var_os("APEXDESK_SCRIPT").is_none() && crate::diag::new_crash();
        let settings_maximized = settings.window_maximized && std::env::var_os("APEXDESK_SCRIPT").is_none();
        App {
            tabs: Vec::new(),
            active: 0,
            next_id: 1,
            untitled_count: [0, 0, 0],
            labels_count: 0,
            settings,
            toasts: Vec::new(),
            closing: None,
            quit_confirmed: false,
            restore_maximized: if settings_maximized { 120 } else { 0 },
            show_shortcuts: false,
            shortcut_query: String::new(),
            shortcut_page: 0,
            focus_shortcuts: false,
            show_about: false,
            show_licenses: false,
            show_eula: false,
            lic: license_ui::LicenseState::new(),
            title: String::new(),
            fullscreen: fullscreen::FullScreen::default(),
            script: crate::script::Script::from_env(),
            perf: None,
            perf_ui: Vec::new(),
            perf_wall: Vec::new(),
            perf_last: None,
            print: None,
            pending_print: None,
            script_pdf: None,
            save_pdf_to: None,
            pdf_task: None,
            template_gallery: None,
            save_template: None,
            shot_pending: None,
            script_hover: None,
            incoming,
            mcp: if cfg!(test) { None } else { crate::mcp::link::listen(ctx.clone(), std::env::var_os("APEXDESK_SCRIPT").is_some()) },
            mcp_dlg: false,
            started: Some(started),
            side: None,
            options: None,
            toolstrip_tab: ["home"; 6],
            palette: None,
            capture: None,
            customize: None,
            commands: None,
            want_commands: false,
            pending_cmd: None,
            toolstrip_peek: 0,
            recovery: recovery::Recovery::new(),
            password_prompt: None,
            encrypt_dlg: None,
            read_only_ask: None,
            props_dlg: None,
            control_dlg: None,
            inspector: None,
            check: None,
            check_passed: None,
            convert_dlg: None,
            batch_dlg: None,
            dossier_dlg: None,
            encoding_dlg: None,
            show_policy: false,
            show_diag: crashed,
            diag_crashed: crashed,
            mail_rx: None,
            macros: macros_ui::MacroUi::default(),
            macro_run: macro_run::MacroRun::default(),
            sig: Default::default(),
            locks: Default::default(),
            protected: Default::default(),
            ctx: ctx.clone(),
        }
    }

    /// A command line argument: a file, or `--new-doc` / `--new-sheet` (the
    /// launcher's actions).
    fn open_arg(&mut self, arg: &Path) {
        match arg.to_str() {
            Some("--new-doc") => self.new_doc(),
            Some("--new-sheet") => self.new_sheet(),
            Some("--new-slides") => self.new_slides(),
            _ => self.open_path(arg),
        }
    }

    pub(crate) fn notify(&mut self, n: Notice) {
        self.toasts.push(Toast { notice: n, at: Instant::now() });
    }

    fn push_tab(&mut self, view: View, path: Option<PathBuf>, format: Option<Format>) {
        let untitled = if path.is_none() {
            let (i, key) = match view.kind() {
                Kind::Sheet => (0, "tab.untitled_sheet"),
                Kind::Doc => (1, "tab.untitled_doc"),
                Kind::Slides => (2, "tab.untitled_slides"),
            };
            self.untitled_count[i] += 1;
            trf(key, &[("n", &self.untitled_count[i])])
        } else {
            String::new()
        };
        self.tabs.push(Tab { id: self.next_id, path, format, untitled, view, link: None, window_no: 0, holds: false, password: None, embed: None, encoding: None, downloaded: false });
        self.next_id += 1;
        self.active = self.tabs.len() - 1;
    }

    fn new_sheet(&mut self) {
        if let Some(Loaded::Sheet(wb)) = crate::templates::normal(Kind::Sheet) {
            self.push_tab(View::Sheet(Box::new(SheetView::new(*wb))), None, None);
            return;
        }
        let mut wb = Workbook::empty();
        wb.sheets.push(apex_sheet::Sheet::new(trf("sheet.default_name", &[("n", &1)])));
        self.push_tab(View::Sheet(Box::new(SheetView::new(wb))), None, None);
    }

    fn new_doc(&mut self) {
        if let Some(Loaded::Doc(doc)) = crate::templates::normal(Kind::Doc) {
            self.push_tab(View::Doc(Box::new(DocView::new(*doc))), None, None);
            return;
        }
        self.push_tab(View::Doc(Box::new(DocView::new(Document::new()))), None, None);
    }

    fn new_slides(&mut self) {
        self.push_tab(View::Slides(Box::new(SlidesView::new(apex_slides::build::new_deck(0)))), None, None);
    }

    /// The view of tab `id`, while it's open.
    pub(crate) fn tab_view_mut(&mut self, id: u64) -> Option<&mut View> {
        self.tabs.iter_mut().find(|t| t.id == id).map(|t| &mut t.view)
    }

    /// Where tab `id` is, while it's open.
    fn tab_index(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    fn open_dialog(&mut self) {
        let all: Vec<&str> = apex_io::SHEET_EXTS.iter().chain(apex_io::DOC_EXTS).chain(apex_io::CODE_EXTS).chain(apex_io::SLIDES_EXTS).chain(apex_io::DB_EXTS).chain(crate::db_view::EXTS).chain(crate::note_view::EXTS).chain(&["pdf"]).copied().collect();
        let dbs: Vec<&str> = crate::db_view::EXTS.iter().chain(apex_io::DB_EXTS).copied().collect();
        let docs: Vec<&str> = apex_io::DOC_EXTS.iter().chain(apex_io::CODE_EXTS).copied().collect();
        crate::file_dialog::FileDialog::new()
            .set_title(tr("dialog.open_title"))
            .add_filter(tr("dialog.filter_all"), &all)
            .add_filter(tr("dialog.filter_sheets"), apex_io::SHEET_EXTS)
            .add_filter(tr("dialog.filter_docs"), &docs)
            .add_filter(tr("dialog.filter_slides"), apex_io::SLIDES_EXTS)
            .add_filter(tr("dialog.filter_databases"), &dbs)
            .add_filter("PDF", &["pdf"])
            .add_filter(tr("nb.filter"), crate::note_view::EXTS)
            .pick_files(|app, picked| {
                for p in picked {
                    // Access and dBase files go into a database, as File >
                    // Open database did; PDF files open in a PDF tab.
                    if Self::imported_db(&p) {
                        app.open_database_file(p);
                    } else {
                        app.open_path(&p);
                    }
                }
            });
    }

    /// Whether `p` is an Access or dBase file (imported into a database).
    fn imported_db(p: &Path) -> bool {
        p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).is_some_and(|e| apex_db::ACCESS_EXTS.iter().chain(apex_db::DBASE_EXTS).any(|x| *x == e))
    }

    pub fn open_path(&mut self, path: &Path) {
        self.open_path_with(path, None);
    }

    /// Opens a file, encrypted ones with `password` (asking for it when
    /// it's missing or wrong).
    fn open_path_with(&mut self, path: &Path, password: Option<String>) {
        let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if let Some(i) = self.tabs.iter().position(|t| t.path.as_deref() == Some(canon.as_path())) {
            self.active = i;
            return;
        }
        // SQLite keeps several users of a database apart itself.
        if crate::db_view::is_db_file(&canon) {
            self.open_db_tab(&canon);
            return;
        }
        // A notebook holds a lock on its file itself.
        if crate::note_view::is_note_file(&canon) {
            self.open_note_tab(&canon);
            return;
        }
        // Someone else may have it open: ask first.
        let Some(mode) = self.lock_gate(&canon, &password) else { return };
        if Format::from_path(&canon) == Some(Format::Pdf) {
            self.open_pdf_tab(&canon, password, mode);
            return;
        }
        let t0 = Instant::now();
        match apex_io::open_with(&canon, password.as_deref()) {
            Ok((loaded, fmt)) => {
                let (macros, recommended) = match &loaded {
                    Loaded::Sheet(wb) => (wb.macros.has_code(), wb.meta.read_only_recommended),
                    Loaded::Doc(doc) => (doc.macros.has_code(), doc.meta.read_only_recommended),
                    Loaded::Slides(d) => (d.macros.has_code(), d.meta.read_only_recommended),
                };
                let truncated = matches!(&loaded, Loaded::Doc(d) if d.truncated);
                let view = locks::view_of(loaded);
                let signed = signatures_ui::read_signed(&canon, password.as_deref(), fmt);
                // Replace a pristine untitled tab instead of piling up tabs.
                if let Some(t) = self.tabs.get(self.active) {
                    if t.path.is_none() && !t.view.modified() && t.view.kind() == fmt.kind() && self.tabs.len() == 1 {
                        self.tabs.clear();
                    }
                }
                // A template opens as a new file, like in Excel and Word.
                if apex_io::is_template(&canon) {
                    self.push_tab(view, None, None);
                    self.made_from(&canon);
                } else if mode == locks::OpenMode::Copy {
                    self.push_tab(view, None, None);
                    self.made_from(&canon);
                    self.opened_copy(&canon);
                } else {
                    self.push_tab(view, Some(canon.clone()), Some(fmt));
                    // A text file is saved again in the encoding it has.
                    let encoding = apex_io::is_text_format(fmt).then(|| apex_io::detect_file_encoding(&canon).ok()).flatten();
                    if let Some(t) = self.tabs.last_mut() {
                        t.password = password;
                        t.encoding = encoding;
                    }
                    // The author recommends read-only: so it opens, asking.
                    let mode = if recommended && mode == locks::OpenMode::Normal {
                        self.read_only_ask = Some(canon.clone());
                        locks::OpenMode::ReadOnly
                    } else {
                        mode
                    };
                    self.opened(&canon, mode);
                }
                self.note_source(self.tabs.len() - 1, &canon, true);
                // Downloaded: read-only and macros off until the user says.
                let protect = self.should_protect(&canon);
                if protect {
                    self.protect_last();
                }
                if macros && let Some(id) = self.tabs.last().map(|t| t.id) {
                    self.macros.note(id);
                    if !protect {
                        self.macros_opened(id, &canon);
                    }
                }
                if let (Some(s), Some(id)) = (signed, self.tabs.last().map(|t| t.id)) {
                    self.sig.note(id, s);
                }
                self.settings.add_recent(canon.clone());
                self.settings.save();
                let ms = t0.elapsed().as_secs_f32() * 1000.0;
                if truncated {
                    self.opened_in_part(&canon);
                } else if fmt == apex_io::Format::Pdf {
                    // Rebuilt from how the pages are drawn: say so.
                    self.notify(Notice::info(tr("notice.pdf_opened")));
                } else {
                    self.notify(Notice::info(trf("notice.opened", &[("ms", &format!("{ms:.0}"))])));
                }
            }
            Err(e) if e.needs_password() || e.wrong_password() => {
                self.keep_open_mode(&canon, mode);
                self.ask_password(canon, None, e.wrong_password());
            }
            Err(e) if e.pdf_scanned() => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.notify(Notice::error(trf("error.pdf_scanned", &[("file", &name)])));
            }
            Err(e) if e.rights_managed() => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.notify(Notice::error(trf("error.rights_managed", &[("file", &name)])));
            }
            Err(e) => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.notify(Notice::error(trf("error.open", &[("file", &name), ("error", &e)])));
            }
        }
    }

    /// Tells document tab `idx` the file it shows (at `path`) and, with
    /// `read`, a text file's text: what its code view shows.
    fn note_source(&mut self, idx: usize, path: &Path, read: bool) {
        let Some(tab) = self.tabs.get_mut(idx) else { return };
        let View::Doc(v) = &mut tab.view else { return };
        let fmt = tab.format.or_else(|| Format::from_path(path));
        let text = if read && fmt.is_some_and(apex_io::is_text_format) {
            std::fs::read(path).ok().map(|data| tab.encoding.unwrap_or_else(|| apex_io::encoding::detect(&data)).decode(&data))
        } else {
            None
        };
        v.set_source(Some(path), text);
    }

    /// Saves the tab.
    fn save(&mut self, idx: usize, save_as: bool) {
        self.save_then(idx, save_as, |_, _| {});
    }

    /// Saves the tab; `then` learns whether it was (false: the user
    /// cancelled or saving failed), in a later frame if a dialog asks
    /// where.
    fn save_then(&mut self, idx: usize, save_as: bool, then: impl FnOnce(&mut App, bool) + 'static) {
        // An embedded object goes back into its container.
        if !save_as && self.edits_object(idx) {
            let ok = self.save_object(idx);
            return then(self, ok);
        }
        if matches!(self.tabs[idx].view, View::Pdf(_)) {
            return self.save_pdf(idx, save_as, then);
        }
        if matches!(self.tabs[idx].view, View::Db(_)) {
            let ok = self.save_db(idx, save_as);
            return then(self, ok);
        }
        if matches!(self.tabs[idx].view, View::Note(_)) {
            let ok = self.save_note(idx, save_as);
            return then(self, ok);
        }
        self.hold(idx);
        let tab = &self.tabs[idx];
        let read_only = tab.view.read_only();
        match (&tab.path, tab.format, save_as) {
            // Read-only formats (.xls, .xlsb, .doc) and files opened
            // read-only are saved as another one.
            (Some(p), Some(f), false) if f.can_save() && !read_only => {
                let ok = self.save_checked(idx, p.clone(), f);
                then(self, ok);
            }
            _ => self.ask_save_path(idx, move |app, target| {
                let ok = target.is_some_and(|(idx, path, fmt)| app.save_checked(idx, path, fmt));
                then(app, ok);
            }),
        }
    }

    /// Saves tab `idx` at `path` unless a question comes first.
    fn save_checked(&mut self, idx: usize, path: PathBuf, fmt: Format) -> bool {
        self.hold(idx);
        // Saving changes drops a signature: ask first.
        if !self.signature_fits(idx, &path, fmt) {
            return false;
        }
        // A macro-free format would drop the VBA project: ask first.
        if !self.macros_fit(idx, &path, fmt) {
            return false;
        }
        // The properties a policy requires: ask for them.
        if !self.props_fit(idx, &path, fmt) {
            return false;
        }
        self.save_to(idx, path, fmt)
    }

    /// Saves tab `idx` at `path` in `fmt`.
    /// The last tab was made from the file at `src`: if that came from
    /// the Internet, so did it.
    fn made_from(&mut self, src: &std::path::Path) {
        let downloaded = protected::from_internet(src);
        if let Some(t) = self.tabs.last_mut() {
            t.downloaded = downloaded;
        }
    }

    fn save_to(&mut self, idx: usize, path: PathBuf, fmt: Format) -> bool {
        // Someone changed the file since: ask before writing over it.
        if !self.may_overwrite(idx, &path) {
            return false;
        }
        self.hold(idx);
        if let Some(id) = self.tabs.get(idx).map(|t| t.id) {
            self.flush_macros(id);
        }
        // A signed file saved unchanged is written as it was read, which
        // keeps its signature.
        let original = self.signed_original(idx, fmt);
        let kept = original.is_some();
        // Text in an encoding other than plain UTF-8.
        let encoding = self.tabs[idx].encoding.filter(|e| *e != apex_io::Encoding::Utf8 && apex_io::is_text_format(fmt) && original.is_none());
        let pw = self.tabs[idx].password.clone();
        // A document that came from the Internet stays marked as such in
        // every copy saved from it (the mark is lost when the file is
        // replaced, and a new name never had it).
        let downloaded = self.tabs[idx].downloaded();
        let res = if let Some(enc) = encoding {
            self.save_text_encoded(idx, &path, fmt, enc)
        } else {
            self.write_tab(idx, &path, fmt, original, pw.as_deref())
        };
        if res.is_ok() && downloaded {
            apex_io::safe_path::mark_untrusted(&path);
        }
        if res.is_ok() && !kept {
            let id = self.tabs[idx].id;
            self.signature_dropped(id);
        }
        self.after_save(idx, path, fmt, res, pw.is_some())
    }

    /// Writes tab `idx` (or the signed bytes it was read from).
    fn write_tab(&mut self, idx: usize, path: &Path, fmt: Format, original: Option<Vec<u8>>, pw: Option<&str>) -> Result<(), String> {
        let path = path.to_path_buf();
        match (&mut self.tabs[idx].view, original) {
            (_, Some(bytes)) => apex_io::write_atomic(&path, &bytes).map_err(|e| e.to_string()),
            (View::Sheet(v), None) => {
                v.commit_edit();
                apex_io::save_sheet_with(&v.wb, &path, fmt, pw)
                    .map(|_| {
                        v.wb.modified = false;
                        v.wb.loaded_stale = false;
                    })
                    .map_err(|e| e.to_string())
            }
            (View::Doc(v), None) => {
                // The Markdown source, when it shows, is saved as it is.
                if let Some(text) = v.md_source_for(fmt) {
                    let res = apex_io::save_text(apex_io::TextSource::Raw(&text), &path, fmt, apex_io::Encoding::Utf8).map_err(|e| e.to_string());
                    if res.is_ok() {
                        v.doc.modified = false;
                        v.md_saved(fmt);
                    }
                    return res.map(|_| ());
                }
                // Tables of contents and fields are updated, like other editors.
                v.end_hf_edit();
                v.update_for_output();
                v.doc.fonts = crate::fonts::embedded::for_saving(&v.doc);
                apex_io::save_doc_with(&v.doc, &path, fmt, pw)
                    .map(|_| {
                        v.doc.modified = false;
                        v.md_saved(fmt);
                    })
                    .map_err(|e| e.to_string())
            }
            (View::Slides(v), None) => {
                v.commit_edit();
                apex_io::save_slides_with(&v.pres.deck, &path, fmt, pw).map(|_| v.pres.modified = false).map_err(|e| e.to_string())
            }
            (View::Pdf(v), None) => v.doc.save_to(|b| apex_io::write_atomic(&path, b)).map_err(|e| e.to_string()),
            (View::Db(v), None) => v.db.backup_to(&path).map_err(|e| e.to_string()),
            (View::Note(v), None) => v.copy_to(&path),
        }
    }

    /// Notes a save of tab `idx` done (or failed); returns whether it was.
    fn after_save(&mut self, idx: usize, path: PathBuf, fmt: Format, res: Result<(), String>, encrypted: bool) -> bool {
        if res.is_ok() && encrypted && !apex_io::can_encrypt(fmt) {
            self.notify(Notice::info(tr("password.not_saved_formats")));
        }
        let tab = &mut self.tabs[idx];
        match res {
            Ok(()) => {
                // Code saved as it is loses nothing.
                let lossy = !fmt.is_rich() && !matches!(&tab.view, View::Doc(v) if v.saves_as_typed(fmt));
                if let View::Doc(v) = &mut tab.view {
                    v.source_saved_as(&path);
                }
                let multi_sheet = matches!(&tab.view, View::Sheet(v) if v.wb.sheets.len() > 1);
                tab.path = Some(path.clone());
                tab.format = Some(fmt);
                self.share_path(idx);
                self.saved_file(idx, &path);
                self.settings.add_recent(path);
                self.settings.save();
                if lossy {
                    let key = match (fmt.kind(), multi_sheet) {
                        (Kind::Sheet, true) => "notice.saved_lossy_sheets",
                        (Kind::Sheet, false) => "notice.saved_lossy_sheet",
                        (Kind::Doc, _) => "notice.saved_lossy_doc",
                        (Kind::Slides, _) => "notice.saved_lossy_slides",
                    };
                    self.notify(Notice::info(trf(key, &[("format", &fmt.extension().to_uppercase())])));
                } else {
                    self.notify(Notice::info(tr("notice.saved")));
                }
                true
            }
            Err(e) => {
                self.notify(Notice::error(trf("error.save", &[("error", &e)])));
                false
            }
        }
    }

    fn ask_save_path(&self, idx: usize, then: impl FnOnce(&mut App, Option<(usize, PathBuf, Format)>) + 'static) {
        self.ask_save_path_as(idx, None, then);
    }

    /// Asks where to save tab `idx`, offering the extension `prefer` first
    /// (or else the current one); `then` gets where the tab is now, the
    /// path and the format (None: cancelled, or the tab has closed).
    fn ask_save_path_as(&self, idx: usize, prefer: Option<&str>, then: impl FnOnce(&mut App, Option<(usize, PathBuf, Format)>) + 'static) {
        let tab = &self.tabs[idx];
        let kind = tab.view.kind();
        let formats: &[(&str, &str)] = match kind {
            Kind::Sheet => &[
                ("xlsx", "dialog.fmt_xlsx"),
                ("xlsm", "dialog.fmt_xlsm"),
                ("xltm", "dialog.fmt_xltm"),
                ("ods", "dialog.fmt_ods"),
                ("fods", "dialog.fmt_fods"),
                ("csv", "dialog.fmt_csv"),
                ("tsv", "dialog.fmt_tsv"),
            ],
            Kind::Doc => &[
                ("docx", "dialog.fmt_docx"),
                ("docm", "dialog.fmt_docm"),
                ("dotm", "dialog.fmt_dotm"),
                ("odt", "dialog.fmt_odt"),
                ("fodt", "dialog.fmt_fodt"),
                ("rtf", "dialog.fmt_rtf"),
                ("md", "dialog.fmt_md"),
                ("txt", "dialog.fmt_txt"),
            ],
            Kind::Slides => &[
                ("pptx", "dialog.fmt_pptx"),
                ("pptm", "dialog.fmt_pptm"),
                ("potx", "dialog.fmt_potx"),
                ("ppsx", "dialog.fmt_ppsx"),
                ("odp", "dialog.fmt_odp"),
                ("otp", "dialog.fmt_otp"),
            ],
        };
        // Offer the current format first so "Save as" keeps it by default
        // (macro-enabled if the file came with macros).
        let current = tab.path.as_ref().and_then(|p| p.extension()).and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
        let has_macros = match &tab.view {
            View::Sheet(v) => v.wb.macros.project.is_some(),
            View::Doc(v) => v.doc.macros.project.is_some(),
            View::Slides(v) => v.pres.macros.project.is_some(),
            View::Pdf(_) | View::Db(_) | View::Note(_) => false,
        };
        // New files: the default format of the settings (or the policy).
        let own = match kind {
            Kind::Sheet => self.settings.save_sheet_format.as_str(),
            Kind::Doc => self.settings.save_doc_format.as_str(),
            Kind::Slides => "pptx",
        };
        let plain = formats.iter().find(|f| f.0 == own).map_or(formats[0].0, |f| f.0);
        let first = prefer.map(str::to_string).or(current.clone().filter(|e| formats.iter().any(|f| f.0 == e))).or_else(|| {
            let fallback = if has_macros { formats[1].0 } else { plain };
            Some(tab.format.filter(|f| f.can_save() && !has_macros).map_or(fallback, |f| f.extension()).to_string())
        });
        let mut order: Vec<(&str, &str)> = formats.to_vec();
        // A source code file is offered its own extension first.
        let code = current.as_deref().and_then(|e| apex_io::CODE_EXTS.iter().find(|c| **c == e)).filter(|_| kind == Kind::Doc && prefer.is_none());
        if let Some(ext) = code {
            order.insert(0, (*ext, "dialog.fmt_code"));
        } else if let Some(i) = order.iter().position(|(e, _)| Some(*e) == first.as_deref()) {
            let x = order.remove(i);
            order.insert(0, x);
        }
        let default_ext = order[0].0;
        let default_fmt = Format::from_path(Path::new(&format!("x.{default_ext}"))).unwrap_or(match kind {
            Kind::Sheet => Format::Xlsx,
            Kind::Doc => Format::Docx,
            Kind::Slides => Format::Pptx,
        });
        let stem = match &tab.path {
            Some(p) => p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            None => tab.untitled.clone(),
        };
        let mut dlg = crate::file_dialog::FileDialog::new()
            .set_title(tr("dialog.save_title"))
            .set_file_name(format!("{stem}.{default_ext}"));
        if let Some(dir) = tab.path.as_ref().and_then(|p| p.parent()) {
            dlg = dlg.set_directory(dir);
        }
        for (ext, key) in &order {
            dlg = dlg.add_filter(tr(key), &[*ext]);
        }
        // PDF too, as in Office: the file is exported, and the tab stays
        // the file it was (not a macro format asked for).
        let pdf = prefer.is_none();
        if pdf {
            dlg = dlg.add_filter(tr("dialog.fmt_pdf"), &["pdf"]);
        }
        let id = tab.id;
        dlg.save_file_or_cancel(move |app, path| {
            if pdf && let Some(p) = path.as_ref().filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))) {
                app.save_pdf_to = Some((id, p.clone()));
                return then(app, None);
            }
            let target = path.and_then(|mut path| {
                let fmt = match Format::from_path(&path) {
                    Some(f) if f.kind() == kind && f.can_save() => f,
                    _ => {
                        let mut s = path.into_os_string();
                        s.push(".");
                        s.push(default_ext);
                        path = PathBuf::from(s);
                        default_fmt
                    }
                };
                Some((app.tab_index(id)?, path, fmt))
            });
            then(app, target);
        });
    }

    fn request_close_tab(&mut self, idx: usize) {
        if self.edits_object(idx) && self.tab_modified(idx) {
            // Closing an edited object puts it back, like Office.
            if self.save_object(idx) {
                self.remove_tab(idx);
            }
            return;
        }
        if self.tab_modified(idx) && !self.has_other_window(idx) {
            self.active = idx;
            self.closing = Some(Closing::Tab(self.tabs[idx].id));
        } else {
            self.remove_tab(idx);
        }
    }

    fn remove_tab(&mut self, idx: usize) {
        self.release_window(idx);
        self.tabs.remove(idx);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        } else if self.active > idx {
            self.active -= 1;
        }
        self.tidy_windows();
    }

    fn zoom(&mut self, factor: Option<f32>) {
        let Some(t) = self.tabs.get_mut(self.active) else { return };
        let z = match &mut t.view {
            View::Sheet(v) => &mut v.zoom,
            View::Doc(v) => &mut v.zoom,
            View::Slides(v) => {
                v.fit = false;
                &mut v.zoom
            }
            View::Pdf(v) => {
                v.fit = None;
                &mut v.zoom
            }
            View::Db(v) => &mut v.zoom,
            View::Note(v) => match &mut v.editor {
                Some(ed) => &mut ed.zoom,
                None => return,
            },
        };
        *z = match factor {
            Some(f) => (*z * f).clamp(0.25, 4.0),
            None => 1.0,
        };
    }

    fn toggle_fullscreen(&mut self, ctx: &egui::Context) {
        let (seen, now) = ctx.input(|i| (fullscreen::Seen::of(i.viewport()), i.time));
        for c in self.fullscreen.toggle(&seen, now) {
            ctx.send_viewport_cmd(c);
        }
        ctx.request_repaint_after(Duration::from_secs_f64(fullscreen::CHECK));
    }

    /// Watches a full-screen leave and reports the window's real state.
    fn fullscreen_frame(&mut self, ctx: &egui::Context) -> bool {
        let (seen, now) = ctx.input(|i| (fullscreen::Seen::of(i.viewport()), i.time));
        for c in self.fullscreen.tick(&seen, now) {
            ctx.send_viewport_cmd(c);
        }
        if self.fullscreen.pending() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        self.fullscreen.is_full(&seen)
    }

    fn run_cmd(&mut self, ctx: &egui::Context, cmd: AppCmd) {
        match cmd {
            AppCmd::FormulaBar => {
                self.settings.show_formula_bar = !self.settings.show_formula_bar;
                self.settings.save();
            }
            AppCmd::Fullscreen => self.toggle_fullscreen(ctx),
            AppCmd::Theme(dark) => {
                self.settings.dark = dark;
                apply_theme(ctx, dark);
                self.settings.save();
            }
            AppCmd::DarkCanvas => {
                self.settings.dark_canvas = !self.settings.dark_canvas;
                // It shows with the dark theme: turning it on switches to it.
                if self.settings.dark_canvas && !ctx.global_style().visuals.dark_mode {
                    self.settings.dark = Some(true);
                    apply_theme(ctx, Some(true));
                }
                self.settings.save();
            }
            AppCmd::NewWindow => self.new_window(),
            // Without another workbook window the button says what it needs.
            AppCmd::SideBySide(None) if self.side.is_none() => self.notify(Notice::info(tr("window.side_by_side_need"))),
            AppCmd::SideBySide(other) => self.side = other.map(|other| windows::SideBySide { other, sync: true, last: Vec::new() }),
            AppCmd::SheetToBook { sheet, target, before, copy } => self.sheet_to_book(sheet, target, before, copy),
            AppCmd::SyncScroll => match &mut self.side {
                Some(s) => {
                    s.sync = !s.sync;
                    s.last.clear();
                }
                None => self.notify(Notice::info(tr("window.sync_scroll_need"))),
            },
            AppCmd::Macros => self.open_macros(),
            AppCmd::RunMacro => self.open_run_macro(),
            AppCmd::NewDoc => self.new_doc(),
            AppCmd::NewSheet => self.new_sheet(),
            AppCmd::NewSlides => self.new_slides(),
            AppCmd::Open => self.open_dialog(),
            AppCmd::OpenPath(p) => self.open_path(&p),
            AppCmd::OpenNoteBeside(p) => {
                let here = self.tabs.get(self.active).map(|t| t.id);
                self.open_path(&p);
                if let Some(i) = here.and_then(|id| self.tabs.iter().position(|t| t.id == id)) {
                    self.active = i;
                }
            }
            AppCmd::ShowNotePage(uid, page) => {
                if let Some(i) = self.tabs.iter().position(|t| matches!(&t.view, View::Note(v) if v.uid() == uid)) {
                    self.active = i;
                    if let View::Note(v) = &mut self.tabs[i].view {
                        v.go_to_page(page);
                    }
                }
            }
            AppCmd::Save => {
                if !self.tabs.is_empty() {
                    self.save(self.active, false);
                }
            }
            AppCmd::Print | AppCmd::ExportPdf | AppCmd::ExportPdfDialog => self.pending_print = Some(cmd),
            AppCmd::OpenDoc(d, pdf) => {
                let doc = std::sync::Arc::try_unwrap(d.0).unwrap_or_else(|a| (*a).clone());
                self.push_tab(View::Doc(Box::new(DocView::new(doc))), None, None);
                if pdf {
                    self.pending_print = Some(AppCmd::ExportPdf);
                }
            }
            AppCmd::OpenLabels(d) => {
                let doc = std::sync::Arc::try_unwrap(d.0).unwrap_or_else(|a| (*a).clone());
                self.push_tab(View::Doc(Box::new(DocView::new(doc))), None, None);
                // Its own name, so it doesn't pass for the document it came from.
                self.untitled_count[1] -= 1;
                self.labels_count += 1;
                self.tabs[self.active].untitled = trf("tab.untitled_labels", &[("n", &self.labels_count)]);
            }
        }
    }

    // --- printing ----------------------------------------------------------------------

    fn print_job(&mut self, ctx: &egui::Context, idx: usize, scope: crate::print::Scope) -> Option<crate::print::Job> {
        self.hold(idx);
        let tab = self.tabs.get_mut(idx)?;
        let name = tab.name();
        Some(match &mut tab.view {
            View::Sheet(v) => {
                v.commit_edit();
                v.print_job(&name, scope)
            }
            View::Doc(v) => {
                v.update_for_output();
                v.print_job(ctx, &name)
            }
            View::Slides(v) => v.print_job_as(ctx, &name, scope),
            View::Pdf(_) => return None,
            View::Db(v) => crate::doc_view::DocView::new(v.print_doc()).print_job(ctx, &name),
            View::Note(v) => crate::doc_view::DocView::new(v.print_doc()).print_job(ctx, &name),
        })
    }

    /// The print dialog; `export`: File > Export > PDF's (no printer).
    fn open_print(&mut self, ctx: &egui::Context, export: bool) {
        if let Some(View::Pdf(v)) = self.tabs.get_mut(self.active).map(|t| &mut t.view) {
            v.open_print();
            return;
        }
        let Some(job) = self.print_job(ctx, self.active, crate::print::Scope::ActiveSheet) else { return };
        let tab = &self.tabs[self.active];
        let mut dlg = crate::print::PrintDialog::new(tab.id, job, tab.view.kind() == Kind::Sheet);
        dlg.slides = tab.view.kind() == Kind::Slides;
        dlg.export = export;
        self.print = Some(dlg);
    }

    /// Asks where to save a PDF of `pages` of a job and writes it.
    fn export_pdf(&mut self, idx: usize, job: crate::print::Job, pages: Vec<usize>) {
        self.pdf_path(idx, move |app, idx, path| app.start_pdf(idx, job, pages, pdf_task::PdfDest::File(path)));
    }

    /// Asks where to save a PDF of tab `idx`; `then` gets where the tab is
    /// now and the path.
    fn pdf_path(&self, idx: usize, then: impl FnOnce(&mut App, usize, PathBuf) + 'static) {
        let Some(tab) = self.tabs.get(idx) else { return };
        let stem = match &tab.path {
            Some(p) => p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            None => tab.untitled.clone(),
        };
        let mut dlg = crate::file_dialog::FileDialog::new().set_title(tr("print.export_pdf")).set_file_name(format!("{stem}.pdf")).add_filter("PDF", &["pdf"]);
        if let Some(dir) = tab.path.as_ref().and_then(|p| p.parent()) {
            dlg = dlg.set_directory(dir);
        }
        let id = tab.id;
        dlg.save_file(move |app, mut path| {
            if path.extension().is_none_or(|e| !e.eq_ignore_ascii_case("pdf")) {
                path.as_mut_os_string().push(".pdf");
            }
            if let Some(idx) = app.tab_index(id) {
                then(app, idx, path);
            }
        });
    }

    fn template_dialogs(&mut self, ctx: &egui::Context) {
        if let Some(g) = &mut self.template_gallery
            && let Some(r) = g.ui(ctx)
        {
            self.template_gallery = None;
            match r {
                Ok(Loaded::Sheet(wb)) => self.push_tab(View::Sheet(Box::new(SheetView::new(*wb))), None, None),
                Ok(Loaded::Doc(doc)) => self.push_tab(View::Doc(Box::new(DocView::new(*doc))), None, None),
                Ok(Loaded::Slides(d)) => self.push_tab(View::Slides(Box::new(SlidesView::new(*d))), None, None),
                Err(e) if !e.is_empty() => self.notify(Notice::error(trf("error.open", &[("file", &tr("tpl.template")), ("error", &e)]))),
                Err(_) => {}
            }
        }
        if let Some((id, d)) = &mut self.save_template
            && let Some(r) = d.ui(ctx)
        {
            let id = *id;
            self.save_template = None;
            let Some(path) = r else { return };
            let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) else { return };
            let res = match &mut tab.view {
                View::Doc(v) => {
                    v.end_hf_edit();
                    crate::templates::save(&path, Some(&v.doc), None, None)
                }
                View::Sheet(v) => {
                    v.commit_edit();
                    crate::templates::save(&path, None, Some(&v.wb), None)
                }
                View::Slides(v) => {
                    v.commit_edit();
                    crate::templates::save(&path, None, None, Some(&v.pres.deck))
                }
                View::Pdf(_) | View::Db(_) | View::Note(_) => return,
            };
            match res {
                Ok(()) => self.notify(Notice::info(trf("tpl.saved", &[("name", &path.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default())]))),
                Err(e) => self.notify(Notice::error(trf("error.save", &[("error", &e)]))),
            }
        }
    }

    /// Asks for a folder and writes every slide there as a PNG picture
    /// (`name-1.png`, ...), 1920 pixels wide.
    fn export_slide_pictures(&mut self, ctx: &egui::Context, idx: usize) {
        let Some(tab) = self.tabs.get(idx) else { return };
        let View::Slides(_) = &tab.view else { return };
        let stem = match &tab.path {
            Some(p) => p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            None => tab.untitled.clone(),
        };
        let mut dlg = crate::file_dialog::FileDialog::new().set_title(tr("slides.export_png"));
        if let Some(dir) = tab.path.as_ref().and_then(|p| p.parent()) {
            dlg = dlg.set_directory(dir);
        }
        let (id, ctx) = (tab.id, ctx.clone());
        dlg.pick_folder(move |app, dir| {
            let Some(View::Slides(v)) = app.tab_view_mut(id) else { return };
            let deck = &v.pres.deck;
            let mut written = 0;
            let mut failed = None;
            for i in 0..deck.slides.len() {
                let Some(png) = crate::slides_view::handouts::slide_png(&ctx, deck, i, 1920) else { continue };
                if let Err(e) = crate::common::write_out(dir.join(format!("{stem}-{}.png", i + 1)), png) {
                    failed = Some(e);
                    break;
                }
                written += 1;
            }
            match failed {
                Some(e) => app.notify(Notice::error(trf("error.save", &[("error", &e.to_string())]))),
                None => app.notify(Notice::info(trf("slides.exported_png", &[("n", &written.to_string()), ("dir", &dir.display().to_string())]))),
            }
        });
    }

    /// Asks where to save the tab as HTML (or an EPUB book) and writes it.
    fn export_web(&mut self, idx: usize, epub: bool) {
        if let Some(View::Doc(v)) = self.tabs.get_mut(idx).map(|t| &mut t.view) {
            v.end_hf_edit();
        }
        let Some(tab) = self.tabs.get(idx) else { return };
        let stem = match &tab.path {
            Some(p) => p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            None => tab.untitled.clone(),
        };
        let (ext, filter) = if epub { ("epub", "EPUB") } else { ("html", "HTML") };
        let mut dlg = crate::file_dialog::FileDialog::new().set_title(tr(if epub { "export.epub" } else { "export.html" })).set_file_name(format!("{stem}.{ext}")).add_filter(filter, &[ext]);
        if let Some(dir) = tab.path.as_ref().and_then(|p| p.parent()) {
            dlg = dlg.set_directory(dir);
        }
        let id = tab.id;
        dlg.save_file(move |app, mut path| {
            if path.extension().is_none_or(|e| !e.eq_ignore_ascii_case(ext) && !(ext == "html" && e.eq_ignore_ascii_case("htm"))) {
                path.as_mut_os_string().push(format!(".{ext}"));
            }
            if let Some(idx) = app.tab_index(id) {
                app.write_web(idx, &path);
            }
        });
    }

    /// Writes HTML or EPUB (by the extension) of a tab.
    fn write_web(&mut self, idx: usize, path: &Path) {
        let Some(tab) = self.tabs.get(idx) else { return };
        let epub = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("epub"));
        let title = match &tab.path {
            Some(p) => p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            None => tab.untitled.clone(),
        };
        let bytes = match &tab.view {
            View::Doc(v) if epub => apex_io::html::doc_epub(&v.doc),
            View::Doc(v) => apex_io::html::doc_html(&v.doc).into_bytes(),
            View::Sheet(v) => apex_io::html::sheet_html(&v.wb, &title).into_bytes(),
            View::Slides(v) => crate::slides_view::outline_html(&v.pres.deck, &title).into_bytes(),
            View::Pdf(_) | View::Db(_) | View::Note(_) => return,
        };
        match apex_io::write_atomic(path, &bytes) {
            Ok(()) => self.notify(Notice::info(trf("print.exported", &[("file", &path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default())]))),
            Err(e) => self.notify(Notice::error(trf("error.save", &[("error", &e.to_string())]))),
        }
    }

    fn print_dialogs(&mut self, ctx: &egui::Context) {
        if let Some((id, path)) = self.save_pdf_to.take()
            && let Some(idx) = self.tab_index(id)
            // A policy's check before sharing comes first (and then the
            // export dialog).
            && self.share_checked(idx, check_ui::Then::ExportPdf { dialog: true })
            && let Some(job) = self.print_job(ctx, idx, crate::print::Scope::Workbook)
        {
            let all: Vec<usize> = (0..job.len()).collect();
            self.start_pdf(idx, job, all, pdf_task::PdfDest::File(path));
        }
        if let Some(path) = self.script_pdf.take() {
            if let Some(job) = self.print_job(ctx, self.active, crate::print::Scope::Workbook) {
                let all: Vec<usize> = (0..job.len()).collect();
                self.start_pdf(self.active, job, all, pdf_task::PdfDest::File(path));
            }
        }
        self.pdf_task(ctx);
        if self.pdf_task.is_some() {
            return;
        }
        match self.pending_print.take() {
            Some(AppCmd::Print) => self.open_print(ctx, false),
            Some(AppCmd::ExportPdf | AppCmd::ExportPdfDialog) if matches!(self.tabs.get(self.active).map(|t| &t.view), Some(View::Pdf(_))) => self.export_pdf_copy(self.active),
            // The check before sharing, when a policy wants it.
            Some(cmd @ (AppCmd::ExportPdf | AppCmd::ExportPdfDialog)) if !self.share_checked(self.active, check_ui::Then::ExportPdf { dialog: matches!(cmd, AppCmd::ExportPdfDialog) }) => {}
            Some(AppCmd::ExportPdfDialog) => self.open_print(ctx, true),
            Some(AppCmd::ExportPdf) => {
                if let Some(job) = self.print_job(ctx, self.active, crate::print::Scope::ActiveSheet) {
                    let all: Vec<usize> = (0..job.len()).collect();
                    self.export_pdf(self.active, job, all);
                }
            }
            _ => {}
        }
        let Some(dlg) = &mut self.print else { return };
        let Some(idx) = self.tabs.iter().position(|t| t.id == dlg.tab) else {
            self.print = None;
            return;
        };
        use crate::print::PrintAction;
        let tabs = &mut self.tabs;
        let action = dlg.ui(ctx, &mut |job, n| Self::paint_page(tabs, ctx, idx, job, n));
        match action {
            None => {}
            Some(PrintAction::Rerender) => {
                let scope = dlg.scope;
                if let Some(job) = self.print_job(ctx, idx, scope) {
                    if let Some(d) = &mut self.print {
                        d.set_job(job);
                    }
                }
            }
            Some(PrintAction::Print { printer, copies, pages, layout }) => {
                if let Some(d) = self.print.take() {
                    self.start_layout(ctx, idx, d.job, pages, layout, pdf_task::PdfDest::Printer { printer, copies, sides: layout.sides });
                }
            }
            Some(PrintAction::ExportPdf { pages, layout }) => {
                if let Some(d) = self.print.take() {
                    self.pdf_path(idx, move |app, idx, path| {
                        let ctx = app.ctx.clone();
                        app.start_layout(&ctx, idx, d.job, pages, layout, pdf_task::PdfDest::File(path));
                    });
                }
            }
            Some(PrintAction::PageSetup) => {
                self.print = None;
                self.active = idx;
                match &mut self.tabs[idx].view {
                    View::Sheet(v) => v.open_page_setup(0),
                    View::Doc(v) => v.open_page_setup(0),
                    View::Slides(_) => {}
                    View::Pdf(_) | View::Db(_) | View::Note(_) => {}
                }
            }
            Some(PrintAction::Close) => self.print = None,
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        // Keys pressed for a new shortcut do nothing else.
        if self.custom_shortcuts(ctx) {
            return;
        }
        let act = |s: &KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(s));
        // Longer shortcuts first: consume_shortcut ignores extra modifiers.
        if act(&SHORTCUT_NEW_SHEET) {
            self.new_sheet();
        } else if act(&SHORTCUT_NEW_DOC) {
            self.new_doc();
        }
        if act(&SHORTCUT_OPEN) {
            self.open_dialog();
        }
        // Macros (Alt+F8) and the VBA editor (Alt+F11), like Excel.
        if ctx.input_mut(|i| i.consume_key(Modifiers::ALT, Key::F8)) {
            self.open_run_macro();
        } else if ctx.input_mut(|i| i.consume_key(Modifiers::ALT, Key::F11)) {
            self.open_macros();
        }
        // Ctrl+Shift+Alt+S (the Styles pane) is left to the view.
        if ctx.input(|i| i.modifiers.alt) {
        } else if act(&SHORTCUT_SAVE_AS) {
            if !self.tabs.is_empty() {
                self.save(self.active, true);
            }
        } else if act(&SHORTCUT_SAVE) && !self.tabs.is_empty() {
            self.save(self.active, false);
        }
        // Exact: Ctrl+Shift+P is superscript in documents.
        // In a book, Ctrl+P is Go to; in a notebook, Go to page.
        let book = matches!(self.tabs.get(self.active).map(|t| &t.view), Some(View::Doc(v)) if v.is_book()) || matches!(self.tabs.get(self.active).map(|t| &t.view), Some(View::Note(_)));
        if !self.tabs.is_empty() && !book && ctx.input_mut(|i| i.modifiers.matches_exact(SHORTCUT_PRINT.modifiers) && i.consume_shortcut(&SHORTCUT_PRINT)) {
            self.pending_print = Some(AppCmd::Print);
        }
        if act(&SHORTCUT_CLOSE) && !self.tabs.is_empty() {
            self.request_close_tab(self.active);
        }
        if act(&SHORTCUT_QUIT) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if act(&SHORTCUT_PREV_TAB) {
            if !self.tabs.is_empty() {
                self.active = (self.active + self.tabs.len() - 1) % self.tabs.len();
            }
        } else if act(&SHORTCUT_NEXT_TAB) && !self.tabs.is_empty() {
            self.active = (self.active + 1) % self.tabs.len();
        }
        // Zoom needs the exact modifiers: the spreadsheet uses Ctrl+Shift+Plus
        // (insert cells) and Ctrl+Shift+0 (unhide columns), and Ctrl+Minus
        // deletes cells there, as in other spreadsheets.
        let sheet = self.tabs.get(self.active).is_some_and(|t| t.view.kind() == Kind::Sheet);
        let exact = |s: &KeyboardShortcut| ctx.input_mut(|i| i.modifiers.matches_exact(s.modifiers) && i.consume_shortcut(s));
        if exact(&SHORTCUT_ZOOM_IN) || exact(&SHORTCUT_ZOOM_IN2) {
            self.zoom(Some(1.1));
        }
        if !sheet && exact(&SHORTCUT_ZOOM_OUT) {
            self.zoom(Some(1.0 / 1.1));
        }
        if exact(&SHORTCUT_ZOOM_RESET) {
            self.zoom(None);
        }
        if act(&SHORTCUT_FULLSCREEN) {
            self.toggle_fullscreen(ctx);
        }
        if ctx.input_mut(|i| i.modifiers.matches_exact(SHORTCUT_PALETTE.modifiers) && i.consume_shortcut(&SHORTCUT_PALETTE)) {
            self.palette = if self.palette.is_some() { None } else { Some(crate::palette::Palette::new()) };
        }
        if ctx.input_mut(|i| i.modifiers.matches_exact(SHORTCUT_CAPTURE.modifiers) && i.consume_shortcut(&SHORTCUT_CAPTURE)) {
            self.open_capture();
        }
        if act(&SHORTCUT_FOLD_TOOLSTRIP) {
            self.settings.show_toolbar = !self.settings.show_toolbar;
            self.settings.save();
        }
    }

    fn tab_bar(&mut self, ui: &mut egui::Ui) {
        let mut close = None;
        let mut new_window = None;
        // The tab being dragged (by id), and where each tab is.
        let drag_id = egui::Id::new("tab_drag");
        let mut dragged: Option<u64> = ui.ctx().data(|d| d.get_temp(drag_id));
        let mut rects: Vec<egui::Rect> = Vec::with_capacity(self.tabs.len());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (i, t) in self.tabs.iter().enumerate() {
                let selected = i == self.active;
                let (badge, color) = t.view.badge();
                let frame = egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(8, 4))
                    .corner_radius(egui::CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 })
                    .fill(if selected { ui.visuals().extreme_bg_color } else { Color32::TRANSPARENT });
                let mut cross = egui::Rect::NOTHING;
                let resp = frame
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                            icons::paint(ui.painter(), r, badge, color);
                            let mut name = t.name();
                            if self.tab_modified(i) {
                                name.push_str(" •");
                            }
                            ui.add(egui::Label::new(if selected { RichText::new(name).strong() } else { RichText::new(name) }).selectable(false));
                            // The tab's own click area is registered on top of this
                            // button, so the click is resolved below by position.
                            cross = ui.add(egui::Button::new("×").frame(false).small()).on_hover_text(tr("tab.close")).rect;
                        });
                    })
                    .response;
                let resp = ui.interact(resp.rect, ui.id().with(("tab", t.id)), egui::Sense::click_and_drag());
                rects.push(resp.rect);
                crate::widgets::name(&resp, egui::WidgetType::SelectableLabel, &t.name(), Some(selected));
                if resp.drag_started() {
                    dragged = Some(t.id);
                }
                let movable = t.path.is_some() && !self.tab_modified(i) && !self.edits_object(i);
                resp.context_menu(|ui| {
                    let r = ui.add_enabled(movable, egui::Button::new(tr("tab.new_window")));
                    let r = if movable { r } else { r.on_disabled_hover_text(tr("tab.new_window_saved")) };
                    if r.clicked() {
                        new_window = Some(i);
                        ui.close();
                    }
                    if ui.button(tr("tab.close")).clicked() {
                        close = Some(i);
                        ui.close();
                    }
                });
                let on_cross = resp.interact_pointer_pos().is_some_and(|p| cross.contains(p));
                if resp.clicked() && on_cross {
                    close = Some(i);
                } else if resp.clicked() {
                    self.active = i;
                }
                if resp.middle_clicked() {
                    close = Some(i);
                }
                if let Some(p) = &t.path {
                    resp.on_hover_text(p.to_string_lossy());
                }
            }
            crate::widgets::menu_button(ui, "+", |ui| {
                if ui.button(tr("menu.new_doc")).clicked() {
                    self.new_doc();
                    ui.close();
                }
                if ui.button(tr("menu.new_sheet")).clicked() {
                    self.new_sheet();
                    ui.close();
                }
                if ui.button(tr("menu.new_slides")).clicked() {
                    self.new_slides();
                    ui.close();
                }
                if ui.button(format!("{}…", tr("menu.new_database"))).on_hover_text(tr("db.new_database_tip")).clicked() {
                    ui.close();
                    self.new_database();
                }
                if ui.button(format!("{}…", tr("menu.new_notebook"))).on_hover_text(tr("nb.new_notebook_tip")).clicked() {
                    ui.close();
                    self.new_notebook();
                }
            });
        });
        // Dragged: a line where it would go; dropped: it goes there.
        if let Some(id) = dragged
            && let Some(from) = self.tabs.iter().position(|t| t.id == id)
        {
            let (x, down) = ui.input(|i| (i.pointer.interact_pos().map(|p| p.x), i.pointer.primary_down()));
            let to = x.map_or(from, |x| rects.iter().filter(|r| r.center().x < x).count());
            if down {
                if to != from && to != from + 1 {
                    let lx = rects.get(to).map_or_else(|| rects.last().map_or(0.0, |r| r.max.x + 1.0), |r| r.min.x - 1.0);
                    let y = rects.first().map_or(egui::Rangef::new(0.0, 0.0), |r| r.y_range());
                    ui.painter().vline(lx, y, egui::Stroke::new(2.0, ui.visuals().selection.stroke.color));
                }
                ui.ctx().data_mut(|d| d.insert_temp(drag_id, id));
            } else {
                ui.ctx().data_mut(|d| d.remove::<u64>(drag_id));
                self.move_tab(from, to);
            }
        }
        if let Some(i) = close {
            self.request_close_tab(i);
        } else if let Some(i) = new_window {
            self.open_in_new_window(i);
        }
    }

    /// Moves tab `from` to before the tab at `to` (`to` counted with it
    /// still in place); the active tab stays the same tab.
    fn move_tab(&mut self, from: usize, to: usize) {
        let to = if to > from { to - 1 } else { to };
        if from == to || from >= self.tabs.len() || to >= self.tabs.len() {
            return;
        }
        let active = self.tabs.get(self.active).map(|t| t.id);
        let t = self.tabs.remove(from);
        self.tabs.insert(to, t);
        if let Some(a) = active.and_then(|a| self.tabs.iter().position(|t| t.id == a)) {
            self.active = a;
        }
    }

    /// Takes a saved tab to a window of its own: closes it here and opens
    /// its file in a new ApexDesk (which doesn't hand it back to this one).
    fn open_in_new_window(&mut self, idx: usize) {
        let Some(path) = self.tabs.get(idx).and_then(|t| t.path.clone()) else { return };
        if self.tab_modified(idx) {
            return;
        }
        // Closed first: a notebook is open in one place only.
        self.remove_tab(idx);
        let spawned = std::env::current_exe().and_then(|exe| std::process::Command::new(exe).arg(&path).env(crate::instance::NEW_WINDOW, "1").spawn());
        if let Err(e) = spawned {
            self.notify(Notice::error(format!("{}: {e}", tr("tab.new_window"))));
            self.open_path(&path);
        }
    }

    fn start_screen(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.18).max(20.0));
            ui.label(RichText::new("ApexDesk").size(34.0).strong());
            ui.label(RichText::new(tr("start.tagline")).weak());
            ui.add_space(4.0);
            self.lic.badge(ui);
            ui.add_space(28.0);
            ui.horizontal(|ui| {
                // Narrower buttons in a small window (their names wrap).
                let gap = ui.spacing().item_spacing.x;
                let w = ((ui.available_width() - gap * 5.0) / 6.0).clamp(90.0, 190.0);
                let total = w * 6.0 + gap * 5.0;
                ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
                if big_button(ui, Icon::Doc, tr("start.new_doc"), theme::DOC_ACCENT, w).clicked() {
                    self.new_doc();
                }
                if big_button(ui, Icon::Sheet, tr("start.new_sheet"), theme::SHEET_ACCENT, w).clicked() {
                    self.new_sheet();
                }
                if big_button(ui, Icon::Slides, tr("start.new_slides"), theme::SLIDES_ACCENT, w).clicked() {
                    self.new_slides();
                }
                if big_button(ui, Icon::Table, tr("start.new_database"), theme::DB_ACCENT, w).on_hover_text(tr("db.new_database_tip")).clicked() {
                    self.new_database();
                }
                if big_button(ui, Icon::Notebook, tr("start.new_notebook"), theme::NOTE_ACCENT, w).on_hover_text(tr("nb.new_notebook_tip")).clicked() {
                    self.new_notebook();
                }
                if big_button(ui, Icon::Folder, tr("start.open"), theme::ACCENT, w).clicked() {
                    self.open_dialog();
                }
            });
            ui.add_space(8.0);
            // The other ways in of the File menu, as links.
            ui.horizontal(|ui| {
                let labels = [format!("{}…", tr("tpl.new_menu")), format!("{}…", tr("pdf.open_title")), format!("{}…", tr("db.open_database"))];
                let gap = ui.spacing().item_spacing.x * 4.0;
                let font = egui::TextStyle::Body.resolve(ui.style());
                let total: f32 = labels.iter().map(|l| ui.ctx().fonts_mut(|f| f.layout_no_wrap(l.clone(), font.clone(), Color32::WHITE).size().x) + gap).sum::<f32>() - gap;
                // One under another when they don't fit side by side.
                let mut links = |ui: &mut egui::Ui| {
                    if ui.link(&labels[0]).clicked() {
                        self.template_gallery = Some(crate::templates::Gallery::new(None));
                    }
                    if ui.link(&labels[1]).on_hover_text(tr("pdf.open_tip")).clicked() {
                        self.open_pdf_view();
                    }
                    if ui.link(&labels[2]).on_hover_text(tr("db.open_database_tip")).clicked() {
                        self.open_database();
                    }
                };
                if total > ui.available_width() {
                    ui.vertical_centered(&mut links);
                    return;
                }
                ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
                ui.spacing_mut().item_spacing.x = gap;
                links(ui);
            });
            if !self.settings.recent.is_empty() {
                ui.add_space(30.0);
                ui.label(RichText::new(tr("start.recent")).strong());
                ui.add_space(6.0);
                let mut open = None;
                for p in self.settings.recent.iter().take(8) {
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let dir = p.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
                    let text = RichText::new(format!("{name}   ")).strong();
                    let resp = ui.add(egui::Button::new(text).right_text(RichText::new(dir).weak()).frame(false).min_size(egui::vec2(420.0, 0.0)));
                    if resp.clicked() {
                        open = Some(p.clone());
                    }
                }
                if let Some(p) = open {
                    self.open_path(&p);
                }
            }
            ui.add_space(24.0);
            ui.label(RichText::new(tr("start.drop_hint")).weak().small());
        });
    }

    fn close_dialog(&mut self, ctx: &egui::Context) {
        let Some(closing) = self.closing else { return };
        // Saving asks where: the answer goes on (or stops) closing.
        if crate::file_dialog::busy() {
            return;
        }
        let idx = match closing {
            Closing::Tab(id) => self.tabs.iter().position(|t| t.id == id),
            Closing::App => self.tabs.iter().position(|t| t.view.modified()),
        };
        let Some(idx) = idx else {
            // Nothing (left) to ask about.
            self.closing = None;
            if closing == Closing::App {
                self.quit_confirmed = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            return;
        };
        self.active = idx;
        let name = self.tabs[idx].name();
        let mut choice = None;
        egui::Modal::new(egui::Id::new("unsaved")).show(ctx, |ui| {
            ui.set_max_width(380.0);
            ui.heading(tr("unsaved.title"));
            ui.add_space(6.0);
            ui.label(trf("unsaved.body", &[("file", &name)]));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new(tr("unsaved.save")).strong()).clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                    choice = Some(0);
                }
                if ui.button(tr("unsaved.discard")).clicked() {
                    choice = Some(1);
                }
                if ui.button(tr("unsaved.cancel")).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    choice = Some(2);
                }
            });
        });
        match choice {
            Some(0) => {
                let id = self.tabs[idx].id;
                self.save_then(idx, false, move |app, saved| {
                    if !saved {
                        app.closing = None;
                    } else if matches!(closing, Closing::Tab(_)) {
                        if let Some(idx) = app.tab_index(id) {
                            app.remove_tab(idx);
                        }
                        app.closing = None;
                    }
                });
            }
            Some(1) => {
                match closing {
                    Closing::Tab(_) => self.closing = None,
                    // Other windows of the workbook must not ask again.
                    Closing::App => {
                        if let View::Sheet(v) = &mut self.tabs[idx].view {
                            v.wb.modified = false;
                        }
                    }
                }
                self.remove_tab(idx);
            }
            Some(_) => self.closing = None,
            None => {}
        }
    }

    fn toasts(&mut self, ctx: &egui::Context) {
        const LIFE: f32 = 4.0;
        self.toasts.retain(|t| t.at.elapsed().as_secs_f32() < LIFE + if t.notice.error { 4.0 } else { 0.0 });
        if self.toasts.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -36.0))
            .interactable(false)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                for t in self.toasts.iter().rev().take(4).collect::<Vec<_>>().into_iter().rev() {
                    let (bg, fg) = if t.notice.error {
                        (Color32::from_rgb(0xB3, 0x26, 0x1E), Color32::WHITE)
                    } else {
                        (Color32::from_rgb(0x32, 0x36, 0x3B), Color32::WHITE)
                    };
                    egui::Frame::new().fill(bg).corner_radius(6).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                        ui.set_max_width(420.0);
                        ui.label(RichText::new(&t.notice.text).color(fg));
                    });
                    ui.add_space(4.0);
                }
            });
        ctx.request_repaint_after(Duration::from_millis(250));
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if self.show_shortcuts {
            // "## Title" lines start a section; the rest are "keys | action".
            let text = tr("help.shortcuts");
            let sections: Vec<(&'static str, &'static str)> = text.split("## ").filter(|s| !s.trim().is_empty()).map(|s| s.split_once('\n').map_or((s.trim(), ""), |(t, b)| (t.trim(), b))).collect();
            let icons = [Icon::Keyboard, Icon::Sheet, Icon::Doc];
            let pages: Vec<(Icon, &'static str)> = sections.iter().enumerate().map(|(i, (t, _))| (icons.get(i).copied().unwrap_or(Icon::Keyboard), *t)).collect();
            let (query, focus, page) = (&mut self.shortcut_query, &mut self.focus_shortcuts, &mut self.shortcut_page);
            let mut customize = false;
            let res = widgets::nav_dialog(ctx, "shortcuts", tr("menu.shortcuts"), &pages, page, egui::vec2(560.0, 440.0), |ui, p| {
                ui.horizontal(|ui| {
                    let r = ui.add(egui::TextEdit::singleline(query).hint_text(tr("help.search")).desired_width(300.0));
                    if std::mem::take(focus) {
                        r.request_focus();
                    }
                    customize = ui.button(tr("shortcuts.customize")).clicked();
                });
                let q = query.trim().to_lowercase();
                // A search looks in every section.
                let mut any = false;
                for (i, (title, body)) in sections.iter().enumerate() {
                    if q.is_empty() && i != p {
                        continue;
                    }
                    let rows: Vec<(&str, &str)> = body
                        .lines()
                        .filter(|l| !l.trim().is_empty())
                        .map(|l| l.split_once('|').map_or((l.trim(), ""), |(k, d)| (k.trim(), d.trim())))
                        .filter(|(k, d)| q.is_empty() || k.to_lowercase().contains(&q) || d.to_lowercase().contains(&q))
                        .collect();
                    if rows.is_empty() {
                        continue;
                    }
                    any = true;
                    widgets::section(ui, title);
                    egui::Grid::new(("shortcuts", i)).num_columns(2).striped(true).min_col_width(200.0).spacing([12.0, 6.0]).show(ui, |ui| {
                        for (k, d) in rows {
                            ui.label(RichText::new(k).monospace());
                            ui.label(d);
                            ui.end_row();
                        }
                    });
                }
                if !any {
                    ui.add_space(12.0);
                    ui.label(RichText::new(tr("shortcuts.none")).weak());
                }
            });
            if res != widgets::DialogResult::None || customize {
                self.show_shortcuts = false;
            }
            if customize {
                self.open_customize(1);
            }
        }
        if self.show_about {
            let m = egui::Modal::new(egui::Id::new("about")).show(ctx, |ui| {
                ui.set_width(420.0);
                ui.label(RichText::new("ApexDesk").size(22.0).strong());
                ui.label(RichText::new(format!("{} {}", tr("about.version"), env!("CARGO_PKG_VERSION"))).weak());
                ui.add_space(8.0);
                ui.label(tr("about.body"));
                ui.add_space(8.0);
                self.license_about(ui);
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    let close = ui.button(tr("dialog.close")).clicked();
                    if ui.button(tr("about.licenses")).clicked() {
                        self.show_licenses = true;
                    }
                    if ui.button(tr("about.eula")).clicked() {
                        self.show_eula = true;
                    }
                    self.license_button(ui);
                    close
                })
                .inner
            });
            if m.inner || m.should_close() {
                self.show_about = false;
            }
        }
        if self.show_licenses {
            // Never taller than the window: it would settle a frame later.
            let h = 460.0f32.min(widgets::dialog_room(ctx).y - 4.0);
            let close = widgets::info_dialog(ctx, "licenses", &tr("about.licenses"), 640.0, |ui| {
                egui::ScrollArea::vertical().max_height(h).auto_shrink([false, false]).show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(THIRD_PARTY_NOTICES).monospace()).wrap());
                });
            });
            if close {
                self.show_licenses = false;
            }
        }
        if self.show_eula {
            let h = 460.0f32.min(widgets::dialog_room(ctx).y - 4.0);
            let text = if crate::i18n::current() == "es" { EULA_ES } else { EULA };
            let close = widgets::info_dialog(ctx, "eula", &tr("about.eula"), 640.0, |ui| {
                egui::ScrollArea::vertical().max_height(h).auto_shrink([false, false]).show(ui, |ui| {
                    ui.add(egui::Label::new(text).wrap());
                });
            });
            if close {
                self.show_eula = false;
            }
        }
        self.license_dialog_ui(ctx);
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let title = match self.tabs.get(self.active) {
            Some(t) => {
                let ro = if t.view.read_only() { format!(" [{}]", tr("lock.read_only")) } else { String::new() };
                format!("{}{ro}{} — ApexDesk", t.name(), if self.tab_modified(self.active) { " •" } else { "" })
            }
            None => "ApexDesk".to_string(),
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }
}

impl eframe::App for App {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let start = std::time::Instant::now();
        // Only the window's state: another window may have changed other
        // settings since this one read them.
        if std::env::var_os("APEXDESK_SCRIPT").is_none() {
            let mut saved = Settings::load();
            saved.window_maximized = self.settings.window_maximized;
            saved.window_size = self.settings.window_size;
            saved.save();
        }
        self.recovery.clear();
        self.release_locks();
        crate::objects::clean_temp();
        // The tabs write what they keep on closing (a notebook's pages).
        self.tabs.clear();
        crate::instance::cleanup();
        crate::mcp::link::cleanup();
        let took = start.elapsed();
        if took > std::time::Duration::from_secs(1) {
            crate::diag::warning(&format!("closing took {} ms", took.as_millis()));
        }
        // Everything is saved: the process ends now. Taking down the
        // graphics device and the display connection can take seconds with
        // some drivers, and a window opened meanwhile would hand its files
        // to this closing one. Scripted runs end as before.
        if self.script.is_none() && !cfg!(test) {
            use std::io::Write;
            let _ = std::io::stdout().flush();
            std::process::exit(0);
        }
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        use crate::script::Step;
        // A file dialog is open: the window paints but, like under a modal
        // dialog, takes no clicks or keys until it's answered.
        if crate::file_dialog::busy() {
            // Releases still count, or a key or button would stay down.
            raw.events.retain(|e| {
                matches!(e, egui::Event::Screenshot { .. } | egui::Event::WindowFocused(_) | egui::Event::PointerGone | egui::Event::Key { pressed: false, .. } | egui::Event::PointerButton { pressed: false, .. })
            });
            raw.dropped_files.clear();
        }
        let Some(script) = &mut self.script else { return };
        ctx.request_repaint();
        // Only scripted input counts: the real keyboard and mouse (the test
        // window may take focus while someone types elsewhere) would make
        // runs unreproducible.
        raw.events.retain(|e| matches!(e, egui::Event::Screenshot { .. }));
        raw.hovered_files.clear();
        raw.dropped_files.clear();
        // Scripted keys reach the window whether or not it has the system's
        // focus (a display without a window manager never gives it).
        raw.focused = true;
        if let Some(p) = &self.script_hover {
            raw.hovered_files.push(egui::HoveredFile { path: Some(p.clone()), ..Default::default() });
        }
        if self.shot_pending.is_some() {
            return;
        }
        match script.next() {
            Some(Step::Input(events, mods)) => {
                // The modifiers stay held until the next scripted input, as
                // a person's would during the frames a shortcut takes.
                raw.events.push(egui::Event::ModifiersChanged(mods));
                raw.events.extend(events);
            }
            Some(Step::Shot(p)) => {
                self.shot_pending = Some(p);
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            }
            Some(Step::Pdf(p)) => self.script_pdf = Some(p),
            Some(Step::Export(p)) => self.write_web(self.active, &p),
            Some(Step::Recipients(p)) => {
                if let Some(View::Doc(v)) = self.tabs.get_mut(self.active).map(|t| &mut t.view) {
                    v.load_recipients_from(&p);
                }
            }
            Some(Step::Hover(p)) => {
                raw.hovered_files.push(egui::HoveredFile { path: Some(p.clone()), ..Default::default() });
                self.script_hover = Some(p);
            }
            Some(Step::Drop(p)) => {
                self.script_hover = None;
                raw.hovered_files.clear();
                raw.dropped_files.push(std::sync::Arc::new(crate::script::ScriptFile(p)));
            }
            Some(Step::Image(p)) => {
                match self.tabs.get_mut(self.active).map(|t| &mut t.view) {
                    Some(View::Sheet(v)) => v.insert_image_file(&p),
                    Some(View::Doc(v)) => v.insert_picture_file(&p),
                    _ => {}
                }
            }
            Some(Step::ViewPdf(p)) => self.open_path(&p),
            Some(Step::Query(p)) => {
                if let Some(View::Sheet(v)) = self.tabs.get_mut(self.active).map(|t| &mut t.view) {
                    v.new_query_file(&p);
                }
            }
            Some(Step::Quit) => {
                self.quit_confirmed = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Some(Step::PerfStart) => {
                self.perf = Some(Vec::new());
                self.perf_ui.clear();
                self.perf_wall.clear();
                self.perf_last = None;
            }
            Some(Step::PerfReport(name)) => {
                if let Some(mut v) = self.perf.take() {
                    v.sort_by(f32::total_cmp);
                    if !v.is_empty() {
                        let avg = v.iter().sum::<f32>() / v.len() as f32;
                        let p95 = v[(v.len() * 95 / 100).min(v.len() - 1)];
                        eprintln!("perf {name}: {} frames, avg {:.2} ms, p95 {:.2} ms, max {:.2} ms", v.len(), avg * 1e3, p95 * 1e3, v[v.len() - 1] * 1e3);
                        let mut u = std::mem::take(&mut self.perf_ui);
                        u.sort_by(f32::total_cmp);
                        if !u.is_empty() {
                            let avg = u.iter().sum::<f32>() / u.len() as f32;
                            eprintln!("  ui code: avg {:.2} ms, max {:.2} ms", avg * 1e3, u[u.len() - 1] * 1e3);
                        }
                        let mut w = std::mem::take(&mut self.perf_wall);
                        w.sort_by(f32::total_cmp);
                        if !w.is_empty() {
                            let avg = w.iter().sum::<f32>() / w.len() as f32;
                            let p95 = w[(w.len() * 95 / 100).min(w.len() - 1)];
                            eprintln!("  wall: avg {:.2} ms, p95 {:.2} ms, max {:.2} ms", avg * 1e3, p95 * 1e3, w[w.len() - 1] * 1e3);
                        }
                    }
                }
            }
            Some(Step::Idle) => {}
            Some(Step::Sleep(ms)) => {
                std::thread::sleep(std::time::Duration::from_millis(ms));
                ctx.request_repaint();
            }
            None => self.script = None,
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        fonts::apply(&ctx);
        if let (Some(v), Some(t)) = (&mut self.perf, frame.info().cpu_usage) {
            v.push(t);
            let now = Instant::now();
            if let Some(last) = self.perf_last.replace(now) {
                self.perf_wall.push((now - last).as_secs_f32());
            }
        }
        let ui_start = Instant::now();
        // A file dialog answered since the last frame: do what it was for.
        crate::file_dialog::frame(&ctx, frame);
        crate::file_dialog::poll(self);
        if let Some(t) = self.tabs.get(self.active) {
            crate::file_dialog::set_owner(t.id);
        }

        // Remember the window for the next start: maximized or not, and the
        // last normal size (restoring goes back to it).
        if std::env::var_os("APEXDESK_SCRIPT").is_none() {
            let (maximized, fullscreen, inner) = ctx.input(|i| (i.viewport().maximized.unwrap_or(false), i.viewport().fullscreen.unwrap_or(false), i.viewport().inner_rect));
            if self.restore_maximized > 0 {
                // Asked until the window says it is (then the user decides).
                if maximized {
                    self.restore_maximized = 0;
                } else {
                    self.restore_maximized -= 1;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
                    ctx.request_repaint_after(std::time::Duration::from_millis(16));
                }
            }
            self.settings.window_maximized = maximized || self.restore_maximized > 0;
            if !maximized && !fullscreen && let Some(r) = inner {
                let size = [r.width().round(), r.height().round()];
                if size[0] >= 640.0 && size[1] >= 400.0 {
                    self.settings.window_size = Some(size);
                }
            }
        }

        // Window close: ask about unsaved tabs first.
        if ctx.input(|i| i.viewport().close_requested()) && !self.quit_confirmed && self.tabs.iter().any(|t| t.view.modified()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.closing = Some(Closing::App);
        }

        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        let sent: Vec<PathBuf> = self.incoming.as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default();
        for p in dropped {
            // A picture dropped on a sheet goes into it.
            let picture = p.extension().is_some_and(|e| crate::sheet_view::PICTURE_EXTENSIONS.contains(&e.to_string_lossy().to_lowercase().as_str()));
            match self.tabs.get_mut(self.active).map(|t| &mut t.view) {
                Some(View::Sheet(v)) if picture => v.insert_image_file(&p),
                Some(View::Note(v)) if picture && v.board_open() => v.add_picture_file(&p),
                // A folder dropped on a notebook is an Obsidian vault to bring in.
                Some(View::Note(v)) if p.is_dir() => {
                    v.import_vault(&p);
                }
                // Anything else dropped on a page is kept in the notebook.
                Some(View::Note(v)) if v.page_editing() && !crate::note_view::is_note_file(&p) => v.drop_file(&p),
                // A video or sound dropped on a presentation goes on the slide.
                Some(View::Slides(v)) if crate::media::mime_of(&p.to_string_lossy()).is_some() => v.insert_media_file(&p),
                _ => self.open_path(&p),
            }
        }
        for p in sent {
            self.open_arg(&p);
        }
        // Documents the Finder opened (macOS), as files sent by other
        // processes are.
        for p in crate::opendoc::take() {
            self.open_arg(&p);
        }
        self.mcp_requests(&ctx);
        drop_overlay(&ctx);

        // KeyTips (Alt, then letters) take their keys before the shortcuts.
        let immersive = match self.tabs.get(self.active).map(|t| &t.view) {
            Some(View::Doc(v)) => v.immersive(),
            // The slide show has the whole window.
            Some(View::Slides(v)) => v.showing(),
            Some(View::Pdf(v)) => v.showing(),
            _ => false,
        };
        crate::toolstrip::keytips::input(&ctx, !self.settings.classic && !immersive && self.closing.is_none() && self.palette.is_none());
        let showing = match self.tabs.get(self.active).map(|t| &t.view) {
            Some(View::Slides(v)) => v.showing(),
            Some(View::Pdf(v)) => v.showing(),
            _ => false,
        };
        if self.closing.is_none() && !showing {
            self.shortcuts(&ctx);
        }

        self.palette_ui(&ctx);
        self.capture_ui(&ctx);

        let mut notices = Vec::new();
        let file_name = self.tabs.get(self.active).map(Tab::name).unwrap_or_default();
        let others: Vec<(u64, String)> = if matches!(self.tabs.get(self.active).map(|t| &t.view), Some(View::Sheet(_))) {
            self.tabs.iter().enumerate().filter(|(i, t)| *i != self.active && matches!(t.view, View::Sheet(_))).map(|(_, t)| (t.id, t.name())).collect()
        } else {
            Vec::new()
        };
        theme::set_dark_canvas(self.settings.dark_canvas, ui.visuals().dark_mode);
        let mut shell = Shell {
            cmd: None,
            show_formula_bar: self.settings.show_formula_bar,
            file_name,
            fullscreen: self.fullscreen_frame(&ctx),
            dark: self.settings.dark,
            others,
            books: self.books_for_move(),
            side: self.side.as_ref().map(|s| s.sync),
        };
        // Read mode and focus show only the document.
        if !immersive {
            egui::Panel::top("tabs").frame(egui::Frame::new().fill(theme::workspace_bg(ui.visuals())).inner_margin(egui::Margin { left: 6, right: 6, top: 4, bottom: 0 })).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let right = 260.0_f32.min(ui.available_width() * 0.4) + 8.0 + self.qat_width();
                    ui.allocate_ui(egui::vec2((ui.available_width() - right).max(0.0), 28.0), |ui| self.tab_bar(ui));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        self.search_box(ui);
                        self.qat_bar(ui);
                    });
                });
            });
            egui::Panel::top("toolstrip").frame(egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin { left: 6, right: 6, top: 2, bottom: 4 })).show(ui, |ui| {
                if self.settings.classic {
                    self.classic_ui(ui, &mut shell);
                } else {
                    self.toolstrip_ui(ui, &mut shell);
                }
            });
            crate::toolstrip::keytips::paint(&ctx);
            self.protected_bar(ui);
            self.macro_bar(ui);
            self.signature_bar(ui);
        }
        let has_tab = self.tabs.get(self.active).is_some();
        self.hold(self.active);
        let side = self.side.is_some();
        if let Some(tab) = self.tabs.get_mut(self.active) {
            if matches!(tab.view, View::Sheet(_)) {
                let name = tab.name();
                if let View::Sheet(v) = &mut tab.view {
                    v.set_file_dir(tab.path.as_deref().and_then(std::path::Path::parent));
                    v.set_file_name(name);
                }
            }
            if !immersive {
                egui::Panel::bottom("status").show(ui, |ui| {
                    ui.horizontal(|ui| {
                        match &mut tab.view {
                            View::Sheet(v) => v.status(ui),
                            View::Doc(v) => v.status(ui),
                            View::Slides(v) => v.status(ui),
                            View::Pdf(v) => v.status(ui),
                            View::Db(v) => v.status(ui),
                            View::Note(v) => v.status(ui),
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            match &mut tab.view {
                                View::Sheet(v) => {
                                    common::zoom_control(ui, &mut v.zoom, false);
                                }
                                View::Doc(v) => {
                                    if let Some(fit) = common::zoom_control(ui, &mut v.zoom, true) {
                                        v.fit = Some(fit);
                                    }
                                }
                                View::Slides(v) => v.zoom_status(ui),
                                View::Pdf(v) => v.zoom_status(ui),
                                View::Db(v) => v.zoom_status(ui),
                                View::Note(v) => {
                                    if let Some(ed) = &mut v.editor {
                                        common::zoom_control(ui, &mut ed.zoom, false);
                                    }
                                }
                            }
                            if let Some(f) = tab.format {
                                // The file's own extension tells .xlsm and .docm apart.
                                let ext = tab.path.as_ref().and_then(|p| p.extension()).and_then(|e| e.to_str()).filter(|e| Format::from_path(Path::new(&format!("x.{e}"))) == Some(f)).unwrap_or(f.extension());
                                ui.separator();
                                ui.label(RichText::new(ext.to_uppercase()).weak());
                            }
                            let (meta, kept) = match &tab.view {
                                View::Sheet(v) => (&v.wb.meta, &v.wb.kept),
                                View::Doc(v) => (&v.doc.meta, &v.doc.kept),
                                View::Slides(v) => (&v.pres.deck.meta, crate::slides_view::no_kept()),
                                View::Pdf(_) | View::Db(_) | View::Note(_) => (crate::pdf_view::no_meta(), crate::slides_view::no_kept()),
                            };
                            crate::labels_ui::status(ui, meta, kept);
                            if tab.view.read_only() {
                                App::read_only_badge(ui);
                            }
                            ui.separator();
                            self.lic.badge(ui);
                        });
                    });
                });
            }
            if !side {
                egui::CentralPanel::default_margins().frame(egui::Frame::new().fill(theme::workspace_bg(ui.visuals()))).show(ui, |ui| match &mut tab.view {
                    View::Sheet(v) => {
                        v.ui(ui, &mut shell);
                        notices.append(&mut v.notices);
                    }
                    View::Doc(v) => {
                        v.ui(ui, &mut shell);
                        notices.append(&mut v.notices);
                    }
                    View::Slides(v) => {
                        v.ui(ui, &mut shell);
                        notices.append(&mut v.notices);
                    }
                    View::Pdf(v) => {
                        v.ui(ui, &mut shell);
                        notices.append(&mut v.notices);
                    }
                    View::Db(v) => {
                        v.ui(ui, &mut shell);
                        notices.append(&mut v.notices);
                    }
                    View::Note(v) => {
                        v.ui(ui, &mut shell);
                        notices.append(&mut v.notices);
                    }
                });
            }
        }
        if side && has_tab {
            egui::CentralPanel::default_margins().frame(egui::Frame::new().fill(theme::workspace_bg(ui.visuals()))).show(ui, |ui| {
                if !self.side_by_side_ui(ui, &mut shell, &mut notices) {
                    // Nothing to compare with any more: the active tab alone.
                    if let Some(tab) = self.tabs.get_mut(self.active) {
                        match &mut tab.view {
                            View::Sheet(v) => {
                                v.ui(ui, &mut shell);
                                notices.append(&mut v.notices);
                            }
                            View::Doc(v) => {
                                v.ui(ui, &mut shell);
                                notices.append(&mut v.notices);
                            }
                            View::Slides(v) => {
                                v.ui(ui, &mut shell);
                                notices.append(&mut v.notices);
                            }
                            View::Pdf(v) => {
                                v.ui(ui, &mut shell);
                                notices.append(&mut v.notices);
                            }
                            View::Db(v) => {
                                v.ui(ui, &mut shell);
                                notices.append(&mut v.notices);
                            }
                            View::Note(v) => {
                                v.ui(ui, &mut shell);
                                notices.append(&mut v.notices);
                            }
                        }
                    }
                }
            });
        }
        let cmd = shell.cmd.take();
        self.object_requests();
        self.pdf_requests();
        self.db_requests();
        self.flush_notes();
        if !has_tab {
            egui::CentralPanel::default_margins().show(ui, |ui| self.start_screen(ui));
        }
        for n in notices {
            self.notify(n);
        }
        if let Some(c) = cmd {
            self.run_cmd(&ctx, c);
        }
        if common::take_prefs_dirty() {
            self.settings.recent_colors = widgets::recent_colors();
            self.settings.recent_fonts = common::recent_fonts();
            self.settings.recent_symbols = crate::symbols::recent();
            self.settings.status_stats = crate::sheet_view::status_stats();
            self.settings.save();
        }
        if crate::doc_view::take_user_changed() {
            self.settings.user_name = crate::doc_view::user_setting();
            self.settings.save();
        }
        if crate::read_aloud::take_changed() {
            (self.settings.speech_voice, self.settings.speech_rate) = crate::read_aloud::prefs();
            self.settings.save();
        }
        let (lang, auto) = crate::spell::take_changed();
        if lang {
            self.settings.spell_lang = crate::spell::language();
        }
        if auto {
            self.settings.spell_auto = crate::spell::auto();
        }
        if lang || auto {
            self.settings.save();
        }

        self.sync_locks();
        self.lock_dialogs(&ctx);
        self.template_dialogs(&ctx);
        self.close_dialog(&ctx);
        self.dialogs(&ctx);
        self.options_dialog(&ctx);
        self.customize_dialog(&ctx);
        self.recovery_dialog(&ctx);
        self.password_dialogs(&ctx);
        self.properties_dialog(&ctx);
        self.control_dialog(&ctx);
        self.inspector_dialog(&ctx);
        self.check_dialog(&ctx);
        self.mcp_dialog(&ctx);
        self.convert_dialog(&ctx);
        self.batch_dialog(&ctx);
        self.dossier_dialog(&ctx);
        crate::links::dialog(&ctx);
        self.encoding_dialog(&ctx);
        self.policy_dialog(&ctx);
        self.diag_dialog(&ctx);

        self.poll_mail();
        self.macro_editor(&ctx);
        self.macro_save_dialog(&ctx);
        self.macro_frame(&ctx);
        self.signature_dialogs(&ctx);
        self.autosave(&ctx);
        // Opened from Options: on top of it.
        crate::autocorrect::dialog_ui(&ctx);
        self.print_dialogs(&ctx);
        self.toasts(&ctx);
        self.update_title(&ctx);
        self.debug_capture(&ctx);
        if self.perf.is_some() {
            self.perf_ui.push(ui_start.elapsed().as_secs_f32());
        }
        if let Some(t) = self.started.take() {
            if std::env::var_os("APEXDESK_STARTUP_EXIT").is_some() {
                eprintln!("first frame: {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
                self.quit_confirmed = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }
}

impl App {
    fn debug_capture(&mut self, ctx: &egui::Context) {
        if self.shot_pending.is_none() {
            return;
        }
        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let (Some(img), Some(path)) = (shot, &self.shot_pending) {
            crate::script::save_ppm(path, &img);
            self.shot_pending = None;
        }
        ctx.request_repaint();
    }
}

/// Sets up the process and the window's context for `settings`: language,
/// fonts, theme and the preferences kept outside the app.
fn init(ctx: &egui::Context, settings: &Settings, started: Instant) {
    settings.apply_globals();
    crate::seats::record_at_start();
    // No language chosen yet: the one of the system, if translated.
    if settings.lang.is_empty() {
        i18n::set_from_env();
    } else {
        i18n::set(&settings.lang);
    }
    let t_fonts = Instant::now();
    fonts::install(ctx);
    widgets::set_recent_colors(settings.recent_colors.clone());
    common::set_recent_fonts(settings.recent_fonts.clone());
    crate::symbols::set_recent(settings.recent_symbols.clone());
    crate::sheet_view::set_status_stats(settings.status_stats);
    crate::sheet_view::set_custom_lists(settings.custom_lists.clone());
    if std::env::var_os("APEXDESK_STARTUP_EXIT").is_some() {
        eprintln!("fonts: {:.1} ms, app created at {:.1} ms", t_fonts.elapsed().as_secs_f64() * 1000.0, started.elapsed().as_secs_f64() * 1000.0);
    }
    crate::spell::init(ctx, &settings.spell_lang, settings.spell_auto);
    crate::settings::set_mini_toolbar(settings.mini_toolbar);
    crate::settings::set_vruler(settings.vruler);
    crate::settings::set_draft_vruler(settings.draft_vruler);
    crate::doc_view::set_user_name(&settings.user_name, false);
    crate::read_aloud::set_prefs(&settings.speech_voice, settings.speech_rate);
    theme::setup(ctx, settings.contrast, settings.sharp_text);
    apply_theme(ctx, settings.dark);
    // Zoom shortcuts act on the document, not on the whole interface.
    // A right click held a moment still opens its menu: past 0.8 s egui
    // took it for no click at all, and the menu never came.
    ctx.options_mut(|o| {
        o.zoom_with_keyboard = false;
        o.input_options.max_click_duration = 5.0;
    });
}

fn apply_theme(ctx: &egui::Context, dark: Option<bool>) {
    ctx.set_theme(match dark {
        None => egui::ThemePreference::System,
        Some(true) => egui::ThemePreference::Dark,
        Some(false) => egui::ThemePreference::Light,
    });
}

fn big_button(ui: &mut egui::Ui, icon: Icon, label: &str, color: Color32, width: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 110.0), egui::Sense::click());
    crate::widgets::name(&resp, egui::WidgetType::Button, label, None);
    let v = ui.visuals();
    let bg = if resp.hovered() { v.widgets.hovered.bg_fill } else { v.extreme_bg_color };
    ui.painter().rect_filled(rect, 10.0, bg);
    ui.painter().rect_stroke(rect, 10.0, egui::Stroke::new(if resp.hovered() { 2.0 } else { 1.0 }, color), egui::StrokeKind::Inside);
    icons::paint(ui.painter(), Rect::from_center_size(rect.center() - egui::vec2(0.0, 14.0), egui::vec2(34.0, 34.0)), icon, color);
    // A name wider than the button goes on two lines, smaller if it
    // takes more (or a word doesn't fit): it stays within the button.
    let room = width - 12.0;
    let lay = |size: f32| {
        let mut job = egui::text::LayoutJob::simple(label.to_string(), egui::FontId::proportional(size), v.text_color(), room);
        job.halign = egui::Align::Center;
        ui.painter().layout_job(job)
    };
    let fits = |size: f32| label.split_whitespace().all(|w| ui.painter().layout_no_wrap(w.to_string(), egui::FontId::proportional(size), v.text_color()).size().x <= room) && lay(size).rows.len() <= 2;
    let g = lay([14.0, 13.0, 12.0, 11.0].into_iter().find(|&s| fits(s)).unwrap_or(10.0));
    let top = rect.center().y + 28.0 - g.rows.first().map_or(0.0, |r| r.height()) / 2.0;
    ui.painter().galley(egui::pos2(rect.center().x, top), g, v.text_color());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// Dims the window and shows a hint while files are dragged over it.
fn drop_overlay(ctx: &egui::Context) {
    let names: Vec<String> = ctx.input(|i| {
        i.raw.hovered_files.iter().filter_map(|f| f.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned())).collect()
    });
    if ctx.input(|i| i.raw.hovered_files.is_empty()) {
        return;
    }
    let screen = ctx.content_rect();
    let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop_overlay")));
    p.rect_filled(screen, 0.0, Color32::from_black_alpha(150));
    let frame = screen.shrink(24.0);
    p.rect_stroke(frame, 12.0, egui::Stroke::new(3.0, theme::SHEET_ACCENT), egui::StrokeKind::Inside);
    p.text(frame.center() - egui::vec2(0.0, 14.0), egui::Align2::CENTER_CENTER, tr("app.drop_hint"), egui::FontId::proportional(26.0), Color32::WHITE);
    if !names.is_empty() {
        let list = if names.len() > 3 { format!("{}, … (+{})", names[..3].join(", "), names.len() - 3) } else { names.join(", ") };
        p.text(frame.center() + egui::vec2(0.0, 20.0), egui::Align2::CENTER_CENTER, list, egui::FontId::proportional(15.0), Color32::from_gray(210));
    }
}
