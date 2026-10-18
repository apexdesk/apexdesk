//! Macros: the message bar of files that have them, the warning when a
//! macro-free format would drop them, and the macro editor (View > Macros):
//! the modules of the VBA project with their code (colored, with line
//! numbers and find); standard modules can be added, renamed and removed.
//! The code is kept and saved with the file; running it is `macro_run`'s.

use apex_fs::PathExt;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use apex_io::Format;
use apex_io::vba::{ModuleKind, NameError, VbaModule, VbaProject};
use apex_vba::suspicious::Signal;
use eframe::egui::{self, FontId, RichText, text::CCursor, text::CCursorRange};

use super::macro_run::{immediate_pane, watch_list};
use super::{App, Notice, View};
use crate::i18n::{tr, trf};
use crate::icons::{self, Icon};
use crate::widgets::FitWindow as _;

/// What the editor shows.
enum Source {
    /// The VBA project; read only when locked or signed (the reason's key).
    Vba { project: Box<VbaProject>, read_only: Option<&'static str> },
    /// LibreOffice Basic modules (name, code), read only.
    Basic(Vec<(String, String)>),
    /// A project that couldn't be read.
    Broken(String),
    None,
}

pub(super) struct MacroEditor {
    pub(super) tab: u64,
    source: Source,
    current: usize,
    find: super::vba_edit::Find,
    not_found: bool,
    /// Replace with, and whether that row shows.
    replace: String,
    show_replace: bool,
    /// Find in every module, not only the one shown.
    find_project: bool,
    /// What Replace All did, or the expression's error.
    find_msg: Option<(String, bool)>,
    /// Find All: (module, row, line).
    results: Vec<(usize, usize, String)>,
    /// Characters to select in the module shown (a match).
    select_range: Option<(usize, usize)>,
    /// Row to scroll to after a find.
    scroll_row: Option<usize>,
    /// New name being typed for the current module.
    rename: Option<String>,
    focus_rename: bool,
    confirm_delete: bool,
    /// Changes not yet written into the workbook or document.
    dirty: bool,
    /// Run was pressed: the macro at the cursor (None: ask which).
    run: Option<Option<String>>,
    /// Of a spreadsheet.
    sheet: bool,
    /// Its macros run: a spreadsheet's or a document's.
    runs: bool,
    /// Row to select (a macro's error).
    select_row: Option<usize>,
    /// The syntax error of the current module, for its name and code (a
    /// hash of them).
    syntax: Option<(u64, Option<apex_vba::Error>)>,
    /// Run was F8: paused at the macro's first statement.
    step: bool,
    /// The pane below the code.
    bottom: Option<Bottom>,
    /// A line entered in the Immediate window, to run.
    immediate: Option<String>,
    imm_input: String,
    imm_focus: bool,
    hist_at: Option<usize>,
    new_watch: String,
    watch_kind: usize,
    /// Depuración > Compilar: the first error (module index), or none;
    /// cleared when the code changes.
    compiled: Option<Option<(usize, apex_vba::Error)>>,
    /// Help while typing: the list of members, case, indent.
    assist: super::macro_assist::Assist,
    /// The object picked in the list above the code (None: General).
    object: Option<String>,
    /// Where Mayús+F2 jumped from: (module, row), to go back.
    back: Vec<(usize, usize)>,
    /// Give the code the keys (after a jump from a list).
    focus_code: bool,
    /// Undo and redo of each module, by name.
    history: std::collections::HashMap<String, super::macro_assist::History>,
    /// The Object Browser (F2), open.
    browser: Option<super::macro_browser::Browser>,
    /// A file to import or export, asked of the App (it has the dialogs).
    file_req: Option<FileReq>,
    /// Tools > Options: `Option Explicit` in new modules, the code's size;
    /// `prefs_changed` to save them.
    explicit: bool,
    font_size: f32,
    /// Syntax colors chosen (Options > Code colors).
    colors: super::vba_code::Colors,
    /// File > Compare with another file.
    compare: Option<super::macro_compare::Compare>,
    /// The Guide window, at this topic.
    guide: Option<usize>,
    /// In a window of its own (not on Android: one window there).
    own_window: bool,
    prefs_changed: bool,
    /// Sign… was pressed; Edit was pressed on a signed project.
    sign: bool,
    unlock: bool,
    /// Project Properties, open; the password being typed to open a
    /// locked project (and whether the last one was wrong).
    props: Option<super::macro_props::Props>,
    password: Option<(String, bool)>,
    /// Tools > References, open.
    refs: bool,
    /// The locked project was just opened with its password.
    opened: bool,
    /// The designer of the form module shown, and whether its form (not
    /// its code) is shown.
    designer: Option<super::macro_form_design::Designer>,
    form_view: bool,
    /// The other open workbooks (tab, name) a module can be copied to,
    /// and the copy asked for (module, tab).
    targets: Vec<(u64, String)>,
    copy: Option<(usize, u64)>,
}

#[derive(Clone, Copy)]
enum FileReq {
    Import,
    /// A folder of `.bas`/`.cls` files over the project (edited outside).
    ImportAll,
    Export(usize),
    ExportAll,
    /// The modules of another version of the file, side by side.
    Compare,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Bottom {
    Immediate,
    Watches,
    /// Find All's matches.
    Results,
}

/// A save that would drop the VBA project: tab, path and format asked for.
pub(super) struct SavePrompt {
    tab: u64,
    path: PathBuf,
    fmt: Format,
}

#[derive(Default)]
pub(super) struct MacroUi {
    /// Tabs showing the "this file has macros" bar.
    bars: HashSet<u64>,
    pub(super) editor: Option<MacroEditor>,
    prompt: Option<SavePrompt>,
    /// What the code of each tab with a bar has that malware does (looked
    /// at once).
    signals: std::collections::HashMap<u64, Vec<Signal>>,
    /// The bar of this tab asks "Enable anyway?".
    confirm: Option<u64>,
    /// Tabs whose locked project was opened with its password (or locked
    /// here): their code can be changed until they close.
    opened: HashSet<u64>,
    /// The publisher of each tab's signed project the user may choose to
    /// trust: its name, full name and SHA-256 (looked at once).
    publishers: std::collections::HashMap<u64, Option<(String, String, String, bool)>>,
    /// "Trust this publisher?" for a tab: name, full name and SHA-256.
    trusting: Option<(u64, String, String, String, bool)>,
}

impl MacroUi {
    /// Tab `tab`'s code changed outside the editor (`bin`): the editor
    /// shows it as it is now.
    pub(super) fn reload(&mut self, tab: u64, bin: &[u8]) {
        let opened = self.opened.contains(&tab);
        let Some(e) = self.editor.as_mut().filter(|e| e.tab == tab) else { return };
        if let Source::Vba { project, .. } = &mut e.source
            && let Ok(p) = read_project(bin, opened)
        {
            **project = p;
            e.designer = None;
        }
    }
}

/// Tab's VBA project; one locked with a password the user typed
/// (`opened`) can be changed.
fn read_project(bin: &[u8], opened: bool) -> Result<VbaProject, apex_io::Error> {
    let mut p = apex_io::vba::read(bin)?;
    if opened {
        p.protected = false;
    }
    Ok(p)
}

/// The macros of a view's workbook or document.
pub(in crate::app) fn macros_of(view: &mut View) -> &mut apex_doc::Macros {
    match view {
        View::Sheet(v) => &mut v.wb.macros,
        View::Doc(v) => &mut v.doc.macros,
        View::Slides(v) => &mut v.pres.deck.macros,
        View::Pdf(v) => &mut v.macros,
        View::Db(v) => &mut v.macros,
        View::Note(v) => &mut v.macros,
    }
}

pub(super) fn mark_modified(view: &mut View, revision: bool) {
    match view {
        View::Sheet(v) => {
            v.wb.modified = true;
            if revision {
                v.wb.revision += 1;
            }
        }
        View::Doc(v) => {
            v.doc.modified = true;
            if revision {
                v.doc.revision += 1;
            }
        }
        View::Slides(v) => {
            v.pres.modified = true;
            if revision {
                v.pres.revision += 1;
            }
        }
        // A database keeps its project in the file at once.
        View::Db(v) => v.store_macros(),
        View::Pdf(_) | View::Note(_) => {}
    }
}

/// A workbook's code names: its own and its sheets' (None: not a workbook).
fn code_names(view: &View) -> Option<(Option<String>, Vec<Option<String>>)> {
    let View::Sheet(v) = view else { return None };
    Some((v.wb.code_name.clone(), v.wb.sheets.iter().map(|s| s.code_name.clone()).collect()))
}

/// A new project for a workbook with no macros: its module keeps the
/// workbook's code name when it can be one.
fn new_project(book: Option<&str>) -> Source {
    let book = book.filter(|b| apex_io::vba::is_identifier(b)).unwrap_or("ThisWorkbook");
    let bin = apex_io::vba::new_workbook_project(book, &[]).or_else(|_| apex_io::vba::new_workbook_project("ThisWorkbook", &[]));
    match bin {
        Ok(b) => vba_source(&b),
        Err(e) => Source::Broken(e.to_string()),
    }
}

/// A new project, to edit.
fn vba_source(bin: &[u8]) -> Source {
    match apex_io::vba::read(bin) {
        Ok(p) => Source::Vba { project: Box::new(p), read_only: None },
        Err(e) => Source::Broken(e.to_string()),
    }
}

/// Modules of LibreOffice Basic libraries: `Basic/<library>/<module>.xml`.
fn basic_modules(m: &apex_doc::Macros) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for p in &m.odf {
        let Some(rest) = p.path.strip_prefix("Basic/") else { continue };
        let Some((lib, file)) = rest.split_once('/') else { continue };
        let Some(name) = file.strip_suffix(".xml") else { continue };
        if name.starts_with("script-") || name.starts_with("dialog-") {
            continue;
        }
        let xml = String::from_utf8_lossy(&p.data);
        let mut r = apex_io::xml::Reader::new(&xml);
        let mut code = String::new();
        loop {
            match r.next() {
                apex_io::xml::Ev::Eof => break,
                apex_io::xml::Ev::Start(t) if t.local() == "module" => code = r.text(),
                _ => {}
            }
        }
        out.push((format!("{lib}.{name}"), code));
    }
    out
}

fn kind_key(k: ModuleKind) -> &'static str {
    match k {
        ModuleKind::Standard => "macros.k_standard",
        ModuleKind::Class => "macros.k_class",
        ModuleKind::Document => "macros.k_document",
        ModuleKind::Form => "macros.k_form",
    }
}

fn name_error(e: NameError) -> &'static str {
    match e {
        NameError::Empty => "macros.name_empty",
        NameError::Invalid => "macros.name_invalid",
        NameError::Taken => "macros.name_taken",
    }
}

/// The macro-enabled extension for a file of `kind` saved at `path`
/// (templates stay templates).
fn macro_ext(kind: apex_io::Kind, path: &std::path::Path) -> &'static str {
    let template = apex_io::Flavor::from_path(path).template;
    match (kind, template) {
        (apex_io::Kind::Sheet, false) => "xlsm",
        (apex_io::Kind::Sheet, true) => "xltm",
        (apex_io::Kind::Doc, false) => "docm",
        (apex_io::Kind::Doc, true) => "dotm",
        (apex_io::Kind::Slides, false) => "pptm",
        (apex_io::Kind::Slides, true) => "potm",
    }
}

impl MacroUi {
    /// A file with macros was opened in tab `id`: it shows the bar.
    pub(super) fn note(&mut self, id: u64) {
        self.bars.insert(id);
    }
}

/// What in the VBA code of `m` is typical of malware.
fn scan_macros(m: &apex_doc::Macros) -> Vec<Signal> {
    let Some(project) = m.project.as_ref().and_then(|b| apex_io::vba::read(b).ok()) else { return Vec::new() };
    apex_vba::suspicious::scan(project.modules.iter().map(|m| (m.name.as_str(), m.code.as_str())))
}

/// "This file's macros look like malware (they run programs, …)", or None
/// when nothing serious was found.
pub(super) fn danger_text(signals: &[Signal]) -> Option<String> {
    let reasons: Vec<&str> = signals
        .iter()
        .filter(|s| s.serious())
        .map(|s| match s {
            Signal::AutoRun => tr("macros.sig_autorun"),
            Signal::RunsPrograms => tr("macros.sig_programs"),
            Signal::Downloads => tr("macros.sig_downloads"),
            Signal::WindowsApi => tr("macros.sig_api"),
            Signal::Obfuscated => tr("macros.sig_hidden"),
        })
        .collect();
    if reasons.is_empty() {
        return None;
    }
    let mut list = reasons.join(", ");
    if signals.contains(&Signal::AutoRun) {
        list = format!("{list}, {}", tr("macros.sig_autorun"));
    }
    Some(trf("macros.bar_danger", &[("why", &list)]))
}

impl App {
    /// What the macros of tab `id` have that malware does (looked at once
    /// per tab while the warning can show).
    pub(super) fn macro_signals(&mut self, id: u64) -> Vec<Signal> {
        if let Some(s) = self.macros.signals.get(&id) {
            return s.clone();
        }
        let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) else { return Vec::new() };
        let found = scan_macros(macros_of(&mut t.view));
        self.macros.signals.insert(id, found.clone());
        found
    }

    /// The message bar of the active tab, under the toolstrip.
    pub(super) fn macro_bar(&mut self, ui: &mut egui::Ui) {
        // Protected View's bar goes first; this one waits for it.
        let Some(id) = self.tabs.get(self.active).map(|t| t.id).filter(|id| self.macros.bars.contains(id) && !self.is_protected(*id)) else { return };
        if self.macro_run.is_enabled(id) {
            self.macros.bars.remove(&id);
            return;
        }
        let (doc, vba) = match &self.tabs[self.active].view {
            View::Sheet(v) => (false, v.wb.macros.project.is_some()),
            View::Doc(v) => (false, v.doc.macros.project.is_some()),
            View::Slides(_) => (true, false),
            View::Db(v) => (false, v.macros.project.is_some()),
            View::Pdf(_) | View::Note(_) => (true, false),
        };
        let allowed = super::macro_run::allowed(&self.settings, &self.tabs, id);
        let text = if doc {
            "macros.bar_doc"
        } else if allowed && vba {
            "macros.bar_ask"
        } else {
            "macros.bar"
        };
        let danger = danger_text(&self.macro_signals(id));
        let confirming = self.macros.confirm == Some(id);
        let (mut enable, mut ask, mut cancel, mut trust) = (false, false, false, false);
        // Under "signed only", a sound signature of someone not trusted yet:
        // the user may trust them (never for code that looks like malware).
        let publisher = if vba && !allowed && self.settings.macros_enabled && self.settings.macros_signed_only && danger.is_none() {
            let (settings, tabs) = (&self.settings, &self.tabs);
            self.macros.publishers.entry(id).or_insert_with(|| super::macro_run::untrusted_publisher(settings, tabs, id).map(|c| (c.subject.display(), c.subject.full(), c.fingerprint(), c.self_issued()))).clone()
        } else {
            None
        };
        let dark = ui.visuals().dark_mode;
        let (fill, edge) = match (dark, danger.is_some()) {
            (true, false) => (egui::Color32::from_rgb(0x4A, 0x40, 0x1C), egui::Color32::from_rgb(0x8A, 0x74, 0x2A)),
            (false, false) => (egui::Color32::from_rgb(0xFF, 0xF4, 0xCE), egui::Color32::from_rgb(0xE8, 0xD0, 0x7A)),
            (true, true) => (egui::Color32::from_rgb(0x5A, 0x1E, 0x1E), egui::Color32::from_rgb(0xA0, 0x3A, 0x3A)),
            (false, true) => (egui::Color32::from_rgb(0xFD, 0xE2, 0xE1), egui::Color32::from_rgb(0xE0, 0x8A, 0x86)),
        };
        let (mut open, mut close) = (false, false);
        egui::Panel::top("macro_bar").frame(egui::Frame::new().fill(fill).stroke(egui::Stroke::new(1.0, edge)).inner_margin(egui::Margin::symmetric(10, 4))).show(ui, |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                icons::paint(ui.painter(), r, Icon::Field, ui.visuals().text_color());
                ui.label(RichText::new(tr("macros.title")).strong());
                match (&danger, &publisher) {
                    (Some(d), _) => ui.label(RichText::new(d).strong()),
                    (None, Some((name, full, sha, _))) => ui.label(trf("macros.bar_publisher", &[("name", name)])).on_hover_text(format!("{full}\nSHA-256: {sha}")),
                    (None, None) => ui.label(tr(text)),
                };
                if publisher.is_some() {
                    trust = ui.button(format!("{}…", tr("macros.trust_publisher"))).clicked();
                }
                if allowed && vba {
                    if danger.is_none() {
                        enable = ui.button(RichText::new(tr("macros.enable")).strong()).clicked();
                    } else if confirming {
                        ui.label(tr("macros.enable_sure"));
                        enable = ui.button(tr("macros.enable")).clicked();
                        cancel = ui.button(tr("dialog.cancel")).clicked();
                    } else {
                        ask = ui.button(format!("{}…", tr("macros.enable_anyway"))).clicked();
                    }
                }
                open = ui.button(tr("macros.view")).clicked();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (r, resp) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
                    let color = if resp.hovered() { ui.visuals().strong_text_color() } else { ui.visuals().weak_text_color() };
                    icons::paint(ui.painter(), r.shrink(3.0), Icon::Close, color);
                    close = resp.on_hover_text(tr("dialog.close")).clicked();
                });
            });
        });
        if open {
            self.open_macros();
        }
        if enable {
            self.enable_macros(id);
        }
        if trust && let Some((name, full, sha, own)) = publisher {
            self.macros.trusting = Some((id, name, full, sha, own));
        }
        if ask {
            self.macros.confirm = Some(id);
        }
        if cancel || close || open || enable {
            self.macros.confirm = None;
        }
        if close || open || enable {
            self.macros.bars.remove(&id);
            self.macros.signals.remove(&id);
            self.macros.publishers.remove(&id);
        }
    }

    /// View > Macros: the editor for the active tab.
    pub(super) fn open_macros(&mut self) {
        if self.settings.macro_editor_off {
            self.notify(Notice::info(tr("macros.editor_off")));
            return;
        }
        let active = self.active;
        self.hold(active);
        if let Some(e) = &self.macros.editor
            && e.tab != self.tabs.get(active).map_or(0, |t| t.id)
        {
            let tab = e.tab;
            self.flush_macros(tab);
        }
        let Some(t) = self.tabs.get_mut(active) else { return };
        let sheet = matches!(t.view, View::Sheet(_));
        let runs = sheet || matches!(t.view, View::Doc(_) | View::Db(_));
        let names = code_names(&t.view);
        let opened = self.macros.opened.contains(&t.id);
        let m = macros_of(&mut t.view);
        let mut source = match &m.project {
            Some(bin) => match read_project(bin, opened) {
                Ok(p) => {
                    let read_only = if p.protected { Some("macros.locked") } else if m.signed { Some("macros.signed") } else { None };
                    Source::Vba { project: Box::new(p), read_only }
                }
                Err(e) => Source::Broken(e.to_string()),
            },
            None => {
                let basic = basic_modules(m);
                match (&names, &t.view) {
                    _ if !basic.is_empty() => Source::Basic(basic),
                    // A file without macros: a new project, written into it
                    // with the first change.
                    (Some((book, _)), _) => new_project(book.as_deref()),
                    (_, View::Doc(_)) => vba_source(&apex_io::vba::new_document_project()),
                    // A database's project: standard modules only.
                    (_, View::Slides(_) | View::Db(_)) => vba_source(&apex_io::vba::new_presentation_project()),
                    _ => Source::None,
                }
            }
        };
        // Modules for the sheets that have none yet (only shown until then).
        if let (Source::Vba { project, read_only: None }, Some((mut book, mut sheets))) = (&mut source, names) {
            project.match_book(&mut book, &mut sheets);
        }
        // The first module with code, like the VBA editor.
        let current = match &source {
            Source::Vba { project, .. } => project.modules.iter().position(|m| !m.code.trim().is_empty()).unwrap_or(0),
            _ => 0,
        };
        self.macros.editor = Some(MacroEditor { tab: t.id, source, current, find: Default::default(), not_found: false, replace: String::new(), show_replace: false, find_project: false, find_msg: None, results: Vec::new(), select_range: None, scroll_row: None, rename: None, focus_rename: false, confirm_delete: false, dirty: false, run: None, sheet, runs, select_row: None, syntax: None, step: false, bottom: None, immediate: None, imm_input: String::new(), imm_focus: false, hist_at: None, new_watch: String::new(), watch_kind: 0, compiled: None, assist: Default::default(), object: None, back: Vec::new(), focus_code: false, history: Default::default(), browser: None, file_req: None, explicit: self.settings.vba_explicit, font_size: self.settings.vba_font_size, colors: super::vba_code::parse_colors(&self.settings.vba_colors), compare: None, guide: None, own_window: self.settings.vba_window, prefs_changed: false, sign: false, unlock: false, props: None, password: None, refs: false, opened: false, designer: None, form_view: true, targets: Vec::new(), copy: None });
    }

    /// The code of `module` in the editor, at `line` (from a macro's error).
    pub(super) fn show_macro_code(&mut self, module: &str, line: u32) {
        self.open_macros();
        let Some(e) = &mut self.macros.editor else { return };
        let Source::Vba { project, .. } = &e.source else { return };
        let Some(i) = project.modules.iter().position(|m| m.name.eq_ignore_ascii_case(module)) else { return };
        e.current = i;
        e.scroll_row = Some(line.saturating_sub(1) as usize);
        e.select_row = e.scroll_row;
    }

    /// Writes the editor's changes into the tab's workbook or document.
    pub(super) fn flush_macros(&mut self, tab: u64) {
        self.macros.signals.remove(&tab);
        if !self.macros.editor.as_ref().is_some_and(|e| e.tab == tab && e.dirty && matches!(e.source, Source::Vba { .. })) {
            return;
        }
        let Some(idx) = self.tabs.iter().position(|t| t.id == tab) else { return };
        self.hold(idx);
        let Some(e) = self.macros.editor.as_mut() else { return };
        let Source::Vba { project, .. } = &mut e.source else { return };
        e.dirty = false;
        // A workbook's sheets and their modules, matched as they are now
        // (sheets may have come and gone while the editor was open).
        let view = &mut self.tabs[idx].view;
        let names = code_names(view);
        if let (View::Sheet(v), Some((mut book, mut sheets))) = (&mut *view, names) {
            project.match_book(&mut book, &mut sheets);
            v.wb.code_name = book;
            for (s, n) in v.wb.sheets.iter_mut().zip(sheets) {
                s.code_name = n;
            }
        }
        match project.write() {
            Ok(bin) => {
                macros_of(view).set_project(bin);
                mark_modified(view, true);
            }
            Err(err) => self.notify(Notice::error(trf("macros.bad_project", &[("error", &err)]))),
        }
    }

    /// The modules of tab `tab`'s VBA project (name, kind, code), with the
    /// editor's changes.
    pub(super) fn read_modules(&mut self, tab: u64) -> Result<Vec<(String, &'static str, String)>, String> {
        self.flush_macros(tab);
        let t = self.tabs.iter_mut().find(|t| t.id == tab).ok_or("no such tab")?;
        let opened = self.macros.opened.contains(&tab);
        let Some(bin) = &macros_of(&mut t.view).project else { return Ok(Vec::new()) };
        let p = read_project(bin, opened).map_err(|e| e.to_string())?;
        if p.protected {
            return Err("the macro project is locked with a password".into());
        }
        Ok(p.modules.into_iter().map(|m| {
            let kind = match m.kind {
                ModuleKind::Standard => "module",
                ModuleKind::Class => "class",
                ModuleKind::Document => "document",
                ModuleKind::Form => "form",
            };
            (m.name, kind, m.code)
        }).collect())
    }

    /// Writes `code` as module `name` of tab `tab`'s project (a new
    /// standard or class module when there is none by that name), as one
    /// undo step; true when the module is new. A signed project loses its
    /// signature. The file's macros are disabled until the user enables
    /// them again: code an assistant wrote never runs by itself.
    pub(super) fn write_module(&mut self, tab: u64, name: &str, code: &str, class: bool) -> Result<bool, String> {
        if self.settings.macro_editor_off {
            return Err("the macro editor is turned off by your organization".into());
        }
        let idx = self.tabs.iter().position(|t| t.id == tab).ok_or("no such tab")?;
        if !matches!(self.tabs[idx].view, View::Sheet(_)) {
            return Err("macros can be written only in spreadsheets".into());
        }
        let code = code.replace("\r\n", "\n").replace('\r', "\n");
        let edit = |project: &mut VbaProject| -> Result<bool, String> {
            if project.protected {
                return Err("the macro project is locked with a password".into());
            }
            if let Some(m) = project.modules.iter_mut().find(|m| m.name.eq_ignore_ascii_case(name)) {
                if m.kind == ModuleKind::Form {
                    return Err("forms can't be written".into());
                }
                m.code = code.clone();
                return Ok(false);
            }
            project.check_name(name, None).map_err(|_| format!("\"{name}\" can't be the name of a module"))?;
            let mut m = if class { VbaModule::class(name.trim()) } else { VbaModule::standard(name.trim()) };
            m.code = code.clone();
            project.modules.push(m);
            Ok(true)
        };
        self.edit_tab_project(idx, edit)
    }

    /// Module `module` of the editor's project copied into the workbook of
    /// tab `to` (the project explorer's "Copy to"), under a free name. Its
    /// macros are disabled until the user enables them again.
    pub(super) fn copy_module_to(&mut self, module: usize, to: u64) -> Result<String, String> {
        if self.settings.macro_editor_off {
            return Err("the macro editor is turned off by your organization".into());
        }
        let Some(e) = &self.macros.editor else { return Err("no editor".into()) };
        let Source::Vba { project, .. } = &e.source else { return Err("no project".into()) };
        let m = project.modules.get(module).filter(|m| m.kind != ModuleKind::Document).cloned().ok_or("no such module")?;
        let idx = self.tabs.iter().position(|t| t.id == to && t.id != e.tab).ok_or("no such tab")?;
        if !matches!(self.tabs[idx].view, View::Sheet(_)) {
            return Err("macros can be written only in spreadsheets".into());
        }
        let mut name = m.name.clone();
        self.edit_tab_project(idx, |project| {
            if project.protected {
                return Err(tr("macros.copy_locked").to_string());
            }
            let mut m = m;
            if project.check_name(&m.name, None).is_err() {
                m.name = project.next_name(&m.name);
            }
            name = m.name.clone();
            project.modules.push(m);
            Ok(true)
        })?;
        Ok(name)
    }

    /// Changes the project of tab `idx` with `edit`, as one undo step;
    /// what `edit` returns. A signed project loses its signature, and the
    /// file's macros are disabled until the user enables them again.
    fn edit_tab_project(&mut self, idx: usize, edit: impl FnOnce(&mut VbaProject) -> Result<bool, String>) -> Result<bool, String> {
        let tab = self.tabs[idx].id;
        self.flush_macros(tab);
        let names = code_names(&self.tabs[idx].view);
        let mut source = match &macros_of(&mut self.tabs[idx].view).project {
            Some(bin) => vba_source(bin),
            None => new_project(names.as_ref().and_then(|n| n.0.as_deref())),
        };
        let Source::Vba { project, .. } = &mut source else { return Err("this file's macros can't be read".into()) };
        let created = edit(project)?;
        self.macro_run.forget(tab);
        self.hold(idx);
        let View::Sheet(v) = &mut self.tabs[idx].view else { return Err("macros can be written only in spreadsheets".into()) };
        if let Some((mut book, mut sheets)) = names {
            project.match_book(&mut book, &mut sheets);
            v.wb.code_name = book;
            for (s, n) in v.wb.sheets.iter_mut().zip(sheets) {
                s.code_name = n;
            }
        }
        let bin = project.write().map_err(|e| e.to_string())?;
        // One step the user can undo.
        let mut parts = apex_sheet::FileParts { meta: v.wb.meta.clone(), macros: v.wb.macros.clone(), kept: v.wb.kept.clone(), sheet_kept: v.wb.sheets.iter().map(|s| s.kept.clone()).collect() };
        parts.macros.set_project(bin);
        v.wb.execute_as(apex_sheet::Op::FileParts(Box::new(parts)), "undo.macro");
        mark_modified(&mut self.tabs[idx].view, true);
        self.reload_macro_source(tab);
        Ok(created)
    }

    /// The editor showing tab `tab`'s project as it is now (just signed).
    pub(super) fn reload_macro_source(&mut self, tab: u64) {
        self.macros.signals.remove(&tab);
        let opened = self.macros.opened.contains(&tab);
        let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab) else { return };
        let m = macros_of(&mut t.view);
        let Some(e) = self.macros.editor.as_mut().filter(|e| e.tab == tab) else { return };
        let Some(bin) = &m.project else { return };
        e.source = match read_project(bin, opened) {
            Ok(p) => {
                let read_only = if p.protected { Some("macros.locked") } else if m.signed { Some("macros.signed") } else { None };
                Source::Vba { project: Box::new(p), read_only }
            }
            Err(err) => Source::Broken(err.to_string()),
        };
        e.dirty = false;
    }

    /// Opens tab `tab`'s signed project, unlocks it and changes its last
    /// module's code as typing would; whether it was signed and locked.
    #[cfg(test)]
    pub(super) fn edit_signed_macros_for_test(&mut self, tab: u64, code: &str) -> bool {
        self.open_macros();
        let Some(e) = self.macros.editor.as_mut().filter(|e| e.tab == tab) else { return false };
        let locked = matches!(e.source, Source::Vba { read_only: Some("macros.signed"), .. });
        e.unlock_signed();
        let Source::Vba { project, read_only: None } = &mut e.source else { return false };
        let Some(m) = project.modules.last_mut() else { return false };
        m.code = code.to_string();
        e.dirty = true;
        self.flush_macros(tab);
        locked
    }

    /// Whether tab `idx` can be saved at `path` as is; otherwise asks
    /// about the macros a macro-free format would drop.
    pub(super) fn macros_fit(&mut self, idx: usize, path: &std::path::Path, fmt: Format) -> bool {
        // The editor's changes first: they may be the project.
        if let Some(id) = self.tabs.get(idx).map(|t| t.id) {
            self.flush_macros(id);
        }
        let Some(t) = self.tabs.get_mut(idx) else { return true };
        let has_project = macros_of(&mut t.view).project.is_some();
        if !has_project || !matches!(fmt, Format::Xlsx | Format::Docx | Format::Pptx | Format::Ods | Format::Odt | Format::Odp | Format::Fods | Format::Fodt) || apex_io::Flavor::from_path(path).macros {
            return true;
        }
        self.macros.prompt = Some(SavePrompt { tab: t.id, path: path.to_path_buf(), fmt });
        false
    }

    /// The "save without macros?" question.
    /// "Trust every macro signed by …?", with who exactly that is.
    pub(super) fn trust_publisher_dialog(&mut self, ctx: &egui::Context) {
        let Some((id, name, full, sha, self_signed)) = self.macros.trusting.clone() else { return };
        let mut choice = None;
        let m = egui::Modal::new(egui::Id::new("macro_trust_publisher")).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.heading(tr("macros.trust_publisher_title"));
            ui.add_space(8.0);
            ui.label(trf("macros.trust_publisher_q", &[("name", &name)]));
            ui.add_space(8.0);
            egui::Grid::new("macro_trust_cert").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                ui.label(RichText::new(tr("macros.trust_subject")).weak());
                ui.label(&full);
                ui.end_row();
                ui.label(RichText::new("SHA-256").weak());
                ui.label(RichText::new(&sha).monospace().small());
                ui.end_row();
            });
            ui.add_space(8.0);
            ui.label(RichText::new(tr("macros.trust_publisher_hint")).weak());
            if self_signed {
                // Nobody vouches for who made a certificate signed by itself.
                ui.add_space(8.0);
                ui.label(RichText::new(tr("macros.trust_self_signed")).strong().color(ui.visuals().warn_fg_color));
            }
            ui.add_space(12.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let cancel = ui.button(if self_signed { RichText::new(tr("dialog.cancel")).strong() } else { RichText::new(tr("dialog.cancel")) });
                if self_signed && ui.memory(|m| m.focused().is_none()) {
                    cancel.request_focus();
                }
                if cancel.clicked() {
                    choice = Some(false);
                }
                let label = RichText::new(tr("macros.trust_publisher"));
                if ui.button(if self_signed { label } else { label.strong() }).clicked() {
                    choice = Some(true);
                }
            });
        });
        if choice.is_none() && m.is_top_modal && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            choice = Some(false);
        }
        let Some(yes) = choice else { return };
        self.macros.trusting = None;
        if !yes {
            return;
        }
        self.trust_publisher(id, sha, &name);
    }

    /// Trusts the publisher `sha` (SHA-256), called `name`, and enables tab
    /// `id`'s macros.
    pub(super) fn trust_publisher(&mut self, id: u64, sha: String, name: &str) {
        if crate::policy::locked("TrustedPublishers") {
            return;
        }
        if !self.settings.trusted_publishers.iter().any(|p| p.split_whitespace().next() == Some(sha.as_str())) {
            self.settings.trusted_publishers.push(format!("{sha} {name}"));
            self.settings.save();
        }
        self.macros.bars.remove(&id);
        self.macros.publishers.remove(&id);
        if super::macro_run::allowed(&self.settings, &self.tabs, id) {
            self.enable_macros(id);
        }
    }

    pub(super) fn macro_save_dialog(&mut self, ctx: &egui::Context) {
        let Some(p) = &self.macros.prompt else { return };
        let Some(idx) = self.tabs.iter().position(|t| t.id == p.tab) else {
            self.macros.prompt = None;
            return;
        };
        let kind = self.tabs[idx].view.kind();
        let ext = p.path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let macro_ext = macro_ext(kind, &p.path);
        let mut choice = None;
        let m = egui::Modal::new(egui::Id::new("macro_save")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.heading(tr("macros.save_q_title"));
            ui.add_space(8.0);
            ui.label(trf("macros.save_q", &[("format", &format!(".{ext}")), ("macro", &format!(".{macro_ext}"))]));
            ui.add_space(12.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("dialog.cancel")).clicked() {
                    choice = Some(0);
                }
                if ui.button(tr("macros.save_plain")).clicked() {
                    choice = Some(1);
                }
                if ui.button(RichText::new(trf("macros.save_macro", &[("macro", &format!(".{macro_ext}"))])).strong()).clicked() {
                    choice = Some(2);
                }
            });
        });
        if choice.is_none() && m.is_top_modal && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            choice = Some(0);
        }
        let Some(choice) = choice else { return };
        let Some(p) = self.macros.prompt.take() else { return };
        match choice {
            1 => {
                self.save_to(idx, p.path, p.fmt);
            }
            2 => {
                self.ask_save_path_as(idx, Some(macro_ext), |app, target| {
                    if let Some((idx, path, fmt)) = target
                        && app.macros_fit(idx, &path, fmt)
                    {
                        app.save_to(idx, path, fmt);
                    }
                });
            }
            _ => {}
        }
    }

    /// The macro editor window.
    pub(super) fn macro_editor(&mut self, ctx: &egui::Context) {
        let Some(mut e) = self.macros.editor.take() else { return };
        let Some(idx) = self.tabs.iter().position(|t| t.id == e.tab) else { return };
        let title = format!("{} — {}", tr("macros.title"), self.tabs[idx].name());
        let mut open = true;
        let mut changed = false;
        e.targets = self.tabs.iter().filter(|t| t.id != e.tab && matches!(t.view, View::Sheet(_))).map(|t| (t.id, t.name())).collect();
        let conf = self.macro_run.debug.entry(e.tab).or_default();
        if e.own_window && cfg!(not(target_os = "android")) {
            let builder = egui::ViewportBuilder::default().with_title(title).with_inner_size([1150.0, 680.0]).with_min_inner_size([480.0, 260.0]);
            ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("macro_editor"), builder, |ui, _| {
                egui::CentralPanel::default_margins().show(ui, |ui| changed = e.ui(ui, conf));
                if ui.ctx().input(|i| i.viewport().close_requested()) {
                    open = false;
                }
            });
        } else {
            egui::Window::new(title)
                .id(egui::Id::new("macro_editor"))
                .open(&mut open)
                .default_size([820.0, 520.0])
                .min_size([480.0, 260.0])
                .resizable(true)
                .collapsible(false)
                .show_fit(ctx, |ui| changed = e.ui(ui, conf));
            // Escape closes it when it is in front and no field takes the
            // key (the first Escape only leaves the code).
            let front = ctx.top_layer_id().is_some_and(|l| l.id == egui::Id::new("macro_editor"));
            if front && !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                open = false;
            }
        }
        if changed {
            e.dirty = true;
            self.hold(idx);
            mark_modified(&mut self.tabs[idx].view, false);
        }
        if std::mem::take(&mut e.prefs_changed) {
            self.settings.vba_explicit = e.explicit;
            self.settings.vba_font_size = e.font_size;
            self.settings.vba_window = e.own_window;
            self.settings.vba_colors = super::vba_code::format_colors(&e.colors);
            self.settings.save();
        }
        let file_req = e.file_req.take();
        let copy = e.copy.take();
        let tab = e.tab;
        let run = e.run.take();
        let step = std::mem::take(&mut e.step);
        let immediate = e.immediate.take();
        let sign = std::mem::take(&mut e.sign);
        if std::mem::take(&mut e.unlock) {
            e.unlock_signed();
        }
        if std::mem::take(&mut e.opened) {
            self.macros.opened.insert(tab);
            if let Source::Vba { project, read_only } = &mut e.source {
                project.protected = false;
                *read_only = None;
            }
        }
        self.macros.editor = Some(e);
        if sign {
            self.flush_macros(tab);
            self.open_sign_macros(tab);
        }
        if let Some(line) = immediate {
            self.flush_macros(tab);
            self.run_immediate(tab, line);
        }
        match run {
            Some(Some(name)) => {
                self.flush_macros(tab);
                if step {
                    self.step_macro_named(tab, name);
                } else {
                    self.run_macro_named(tab, name);
                }
            }
            Some(None) if step => self.notify(Notice::info(tr("debug.no_macro_here"))),
            Some(None) => {
                self.active = idx;
                self.open_run_macro();
            }
            None => {}
        }
        if let Some(req) = file_req {
            self.macro_file(req);
        }
        if let Some((module, to)) = copy {
            let book = self.tabs.iter().find(|t| t.id == to).map(|t| t.name()).unwrap_or_default();
            match self.copy_module_to(module, to) {
                Ok(name) => self.notify(Notice::info(trf("macros.copied_to", &[("module", &name), ("book", &book)]))),
                Err(err) => self.notify(Notice::error(err)),
            }
        }
        if !open {
            self.flush_macros(tab);
            self.macros.editor = None;
        }
    }

    /// File > Import, Export and Export Project of the macro editor: the
    /// dialog, and the files read or written when it answers.
    fn macro_file(&mut self, req: FileReq) {
        let Some(e) = &self.macros.editor else { return };
        let Source::Vba { project, .. } = &e.source else { return };
        let cp = project.code_page;
        let dir = self.tabs.iter().find(|t| t.id == e.tab).and_then(|t| t.path.as_ref()).and_then(|p| p.parent()).map(|p| p.to_path_buf());
        let mut dlg = crate::file_dialog::FileDialog::new();
        if let Some(dir) = &dir {
            dlg = dlg.set_directory(dir);
        }
        match req {
            FileReq::Import => {
                dlg.set_title(tr("macros.import")).add_filter("VBA", &["bas", "cls", "frm"]).pick_files(move |app, paths| app.import_modules(paths, cp));
            }
            FileReq::ImportAll => {
                dlg.set_title(tr("macros.import_all")).pick_folder(move |app, dir| app.import_folder(&dir, cp));
            }
            FileReq::Compare => {
                dlg.set_title(tr("macros.compare")).add_filter(tr("macros.compare_files"), &["xlsm", "xlsb", "xls", "xltm", "xlam"]).pick_file(move |app, path| match super::macro_compare::load(&path) {
                    Ok(c) => {
                        if let Some(e) = &mut app.macros.editor {
                            e.compare = Some(c);
                        }
                    }
                    Err(err) => app.notify(Notice::error(err)),
                });
            }
            FileReq::Export(i) if project.modules.get(i).is_some_and(|m| m.kind == ModuleKind::Form) => {
                let name = project.modules[i].name.clone();
                dlg.set_title(tr("macros.export")).set_file_name(format!("{name}.frm")).add_filter("VBA", &["frm"]).save_file(move |app, path| {
                    if let Err(err) = app.export_form(i, &path, cp) {
                        app.notify(Notice::error(trf("error.save", &[("error", &err)])));
                    }
                });
            }
            FileReq::Export(i) => {
                let Some(m) = project.modules.get(i) else { return };
                let bytes = m.export(cp);
                dlg.set_title(tr("macros.export")).set_file_name(format!("{}.{}", m.name, m.file_ext())).add_filter("VBA", &[m.file_ext()]).save_file(move |app, path| {
                    if let Err(err) = crate::common::write_out(&path, &bytes) {
                        app.notify(Notice::error(trf("error.save", &[("error", &err.to_string())])));
                    }
                });
            }
            FileReq::ExportAll => {
                // A form: its .frm and its .frx.
                let mut files: Vec<(String, Vec<u8>)> = Vec::new();
                let mut failed = None;
                for (i, m) in project.modules.iter().enumerate() {
                    if m.kind == ModuleKind::Form {
                        let frx = format!("{}.frx", m.name);
                        match project.export_form(i, &frx, cp) {
                            Ok((frm, design)) => files.extend([(format!("{}.frm", m.name), frm), (frx, design)]),
                            Err(err) => failed = Some(err.to_string()),
                        }
                    } else {
                        files.push((format!("{}.{}", m.name, m.file_ext()), m.export(cp)));
                    }
                }
                if let Some(err) = failed {
                    self.notify(Notice::error(trf("error.save", &[("error", &err)])));
                    return;
                }
                dlg.set_title(tr("macros.export_all")).pick_folder(move |app, dir| {
                    for (name, bytes) in &files {
                        if let Err(err) = crate::common::write_out(dir.join(name), bytes) {
                            app.notify(Notice::error(trf("error.save", &[("error", &err.to_string())])));
                            return;
                        }
                    }
                    app.notify(Notice::info(trf("macros.exported", &[("n", &files.len()), ("dir", &dir.display())])));
                });
            }
        }
    }

    /// Writes form module `i` as `path` (.frm) and its design beside it (.frx).
    fn export_form(&mut self, i: usize, path: &Path, cp: u16) -> Result<(), String> {
        let Some(e) = &self.macros.editor else { return Ok(()) };
        let Source::Vba { project, .. } = &e.source else { return Ok(()) };
        let frx_path = path.with_extension("frx");
        let frx = frx_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let (frm, design) = project.export_form(i, &frx, cp).map_err(|e| e.to_string())?;
        crate::common::write_out(path, &frm).map_err(|e| e.to_string())?;
        crate::common::write_out(&frx_path, &design).map_err(|e| e.to_string())
    }

    /// File > Import folder: each `.bas`/`.cls`/`.frm` of `dir` replaces the code
    /// of the module of its name (one exported and edited outside comes
    /// back, the workbook's and sheets' modules too), or is added.
    pub(super) fn import_folder(&mut self, dir: &Path, cp: u16) {
        let Some(e) = &mut self.macros.editor else { return };
        let Source::Vba { project, read_only: None } = &mut e.source else { return };
        let mut paths: Vec<PathBuf> = match apex_fs::read_dir(dir) {
            Ok(entries) => entries.filter_map(Result::ok).map(|d| d.path()).filter(|p| p.fs_is_file() && p.extension().is_some_and(|x| ["bas", "cls", "frm"].iter().any(|e| x.eq_ignore_ascii_case(e)))).take(1000).collect(),
            Err(err) => {
                let file = dir.display().to_string();
                self.notify(Notice::error(trf("error.open", &[("file", &file), ("error", &err.to_string())])));
                return;
            }
        };
        paths.sort();
        let (mut replaced, mut added) = (0, 0);
        let mut errors = Vec::new();
        for path in paths {
            let bytes = match apex_fs::metadata(&path).map(|m| m.len()) {
                Ok(n) if n <= 8 << 20 => apex_fs::read(&path),
                Ok(_) => Err(std::io::Error::other("too large")),
                Err(err) => Err(err),
            };
            let bytes = match bytes {
                Ok(b) => b,
                Err(err) => {
                    let file = path.display().to_string();
                    errors.push(trf("error.open", &[("file", &file), ("error", &err.to_string())]));
                    continue;
                }
            };
            let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).filter(|s| apex_io::vba::is_identifier(s)).unwrap_or_else(|| project.next_name("Module"));
            let m = match module_from(&path, &bytes, &stem, cp) {
                Ok(m) => m,
                Err(err) => {
                    let file = path.display().to_string();
                    errors.push(trf("error.open", &[("file", &file), ("error", &err)]));
                    continue;
                }
            };
            match project.modules.iter().position(|x| x.name.eq_ignore_ascii_case(&m.name)) {
                // A form's code and design come back to the form of its name.
                Some(i) if project.modules[i].kind == ModuleKind::Form || m.kind == ModuleKind::Form => {
                    let x = &mut project.modules[i];
                    if x.kind == m.kind && (x.code != m.code || m.form.is_some()) {
                        let old = std::mem::replace(&mut x.code, m.code);
                        e.history.entry(x.name.clone()).or_default().record(old, 0, &x.code, f64::INFINITY);
                        x.form = m.form;
                        e.designer = None;
                        replaced += 1;
                    }
                }
                // Its kind stays: a sheet's module is still the sheet's.
                Some(i) if project.modules[i].code != m.code => {
                    let x = &mut project.modules[i];
                    let old = std::mem::replace(&mut x.code, m.code);
                    e.history.entry(x.name.clone()).or_default().record(old, 0, &x.code, f64::INFINITY);
                    replaced += 1;
                }
                Some(_) => {}
                None if project.check_name(&m.name, None).is_ok() => {
                    project.modules.push(m);
                    added += 1;
                }
                None => {}
            }
        }
        if replaced + added > 0 {
            e.dirty = true;
            e.compiled = None;
            let tab = e.tab;
            if let Some(idx) = self.tabs.iter().position(|t| t.id == tab) {
                self.hold(idx);
                mark_modified(&mut self.tabs[idx].view, false);
            }
        }
        for err in errors {
            self.notify(Notice::error(err));
        }
        self.notify(Notice::info(trf("macros.imported_folder", &[("changed", &replaced), ("added", &added)])));
    }

    /// Modules from `.bas`/`.cls`/`.frm` files into the project being edited; a
    /// name already taken gets a number, as in the VBA editor.
    pub(super) fn import_modules(&mut self, paths: Vec<PathBuf>, cp: u16) {
        let Some(e) = &mut self.macros.editor else { return };
        let Source::Vba { project, read_only: None } = &mut e.source else { return };
        let mut added = 0;
        let mut errors = Vec::new();
        for path in paths {
            // Module files are small; a huge one isn't one.
            let bytes = match apex_fs::metadata(&path).map(|m| m.len()) {
                Ok(n) if n <= 8 << 20 => apex_fs::read(&path),
                Ok(_) => Err(std::io::Error::other("too large")),
                Err(err) => Err(err),
            };
            let bytes = match bytes {
                Ok(b) => b,
                Err(err) => {
                    let file = path.display().to_string();
                    errors.push(trf("error.open", &[("file", &file), ("error", &err.to_string())]));
                    continue;
                }
            };
            let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).filter(|s| apex_io::vba::is_identifier(s)).unwrap_or_else(|| project.next_name("Module"));
            let mut m = match module_from(&path, &bytes, &stem, cp) {
                Ok(m) => m,
                Err(err) => {
                    let file = path.display().to_string();
                    errors.push(trf("error.open", &[("file", &file), ("error", &err)]));
                    continue;
                }
            };
            if project.check_name(&m.name, None).is_err() {
                m.name = project.next_name(&m.name);
            }
            project.modules.push(m);
            e.current = project.modules.len() - 1;
            added += 1;
        }
        if added > 0 {
            e.dirty = true;
            e.compiled = None;
            let tab = e.tab;
            if let Some(idx) = self.tabs.iter().position(|t| t.id == tab) {
                self.hold(idx);
                mark_modified(&mut self.tabs[idx].view, false);
            }
        }
        for err in errors {
            self.notify(Notice::error(err));
        }
    }
}

/// The module in file `path` (its bytes, `bytes`; `stem` names it if it
/// doesn't): a `.frm` with its design from the `.frx` beside it.
fn module_from(path: &Path, bytes: &[u8], stem: &str, cp: u16) -> Result<VbaModule, String> {
    if !path.extension().is_some_and(|x| x.eq_ignore_ascii_case("frm")) {
        return Ok(VbaModule::import(bytes, stem, cp));
    }
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let frx = move |name: &str| {
        // Only a .frx beside it, by its file name.
        let file = Path::new(name).file_name()?;
        if !Path::new(file).extension().is_some_and(|x| x.eq_ignore_ascii_case("frx")) {
            return None;
        }
        let p = dir.join(file);
        if apex_fs::metadata(&p).ok()?.len() > 32 << 20 {
            return None;
        }
        apex_fs::read(p).ok()
    };
    VbaModule::import_form(bytes, &frx, stem, cp).map_err(|e| e.to_string())
}

/// The row of the caret in the code editor `id`.
pub(super) fn cursor_row(ctx: &egui::Context, id: egui::Id, code: &str) -> usize {
    let state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    let at: usize = state.cursor.char_range().map_or(0, |r| r.primary.index.into());
    code.chars().take(at).filter(|&c| c == '\n').count()
}

/// Selects row `row` of the code editor `id`.
fn select_row(ctx: &egui::Context, id: egui::Id, code: &str, row: usize) {
    let mut start = 0;
    for (i, l) in code.split('\n').enumerate() {
        let len = l.trim_end_matches('\r').chars().count();
        if i == row {
            let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
            // A blank line (an event's body): the cursor at its end, to type in.
            let from = if l.trim().is_empty() { start + len } else { start };
            state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(from), CCursor::new(start + len))));
            state.store(ctx, id);
            ctx.memory_mut(|m| m.request_focus(id));
            return;
        }
        start += l.chars().count() + 1;
    }
}

/// The macro (public Sub without arguments) the caret at `row` is in.
fn macro_at(m: &VbaModule, row: usize) -> Option<String> {
    let kind = match m.kind {
        ModuleKind::Standard => apex_vba::ModuleKind::Standard,
        ModuleKind::Document => apex_vba::ModuleKind::Document,
        _ => return None,
    };
    let p = apex_vba::Project::parse(&[apex_vba::Source { name: m.name.clone(), kind, code: m.code.clone() }]).ok()?;
    let module = p.modules.first()?;
    let proc = module.procs.iter().filter(|p| p.line as usize <= row + 1).max_by_key(|p| p.line)?;
    let name = if kind == apex_vba::ModuleKind::Standard { proc.name.clone() } else { format!("{}.{}", m.name, proc.name) };
    p.macros().contains(&name).then_some(name)
}

/// Depuración > Compilar: the project's first error, syntax or not, and
/// the index of its module.
fn compile(modules: &[VbaModule]) -> Option<(usize, apex_vba::Error)> {
    let sources: Vec<apex_vba::Source> = modules
        .iter()
        .filter_map(|m| {
            let kind = match m.kind {
                ModuleKind::Standard => apex_vba::ModuleKind::Standard,
                ModuleKind::Document => apex_vba::ModuleKind::Document,
                ModuleKind::Class => apex_vba::ModuleKind::Class,
                ModuleKind::Form => return None,
            };
            Some(apex_vba::Source { name: m.name.clone(), kind, code: m.code.clone() })
        })
        .collect();
    let e = match apex_vba::Project::parse(&sources) {
        Err(e) => e,
        Ok(p) => p.compile_errors().into_iter().next()?,
    };
    let i = modules.iter().position(|m| m.name.eq_ignore_ascii_case(&e.module)).unwrap_or(0);
    Some((i, e))
}

/// The first syntax error of a module's code, if any (forms aren't read).
fn syntax_error(m: &VbaModule) -> Option<apex_vba::Error> {
    let kind = match m.kind {
        ModuleKind::Standard => apex_vba::ModuleKind::Standard,
        ModuleKind::Document => apex_vba::ModuleKind::Document,
        ModuleKind::Class => apex_vba::ModuleKind::Class,
        ModuleKind::Form => return None,
    };
    apex_vba::Project::parse(&[apex_vba::Source { name: m.name.clone(), kind, code: m.code.clone() }]).err().filter(|e| e.line > 0)
}

/// The next match from `from` in module `current`, then (in the project)
/// in the modules after it, wrapping round: (module, start, end).
fn find_from(codes: &[&str], current: usize, from: usize, f: &super::vba_edit::Find, project: bool) -> Option<(usize, usize, usize)> {
    let all = f.matches(codes.get(current)?).ok()?;
    if let Some(m) = all.iter().find(|m| m.0 >= from) {
        return Some((current, m.0, m.1));
    }
    if project {
        for k in 1..codes.len() {
            let i = (current + k) % codes.len();
            if let Some(m) = f.matches(codes[i]).ok()?.first() {
                return Some((i, m.0, m.1));
            }
        }
    }
    all.first().map(|m| (current, m.0, m.1))
}

impl MacroEditor {
    /// Editing a signed project: the first change drops the signature.
    fn unlock_signed(&mut self) {
        if let Source::Vba { read_only, .. } = &mut self.source
            && *read_only == Some("macros.signed")
        {
            *read_only = None;
        }
    }

    /// The window's content; returns whether the code changed.
    fn ui(&mut self, ui: &mut egui::Ui, conf: &mut super::macro_run::DebugConf) -> bool {
        let (project, read_only) = match &mut self.source {
            Source::Vba { project, read_only } => (project, *read_only),
            Source::Basic(modules) => {
                ui.label(RichText::new(tr("macros.basic_ro")).weak());
                ui.separator();
                let names: Vec<String> = modules.iter().map(|m| m.0.clone()).collect();
                self.current = self.current.min(modules.len().saturating_sub(1));
                let mut code = modules.get(self.current).map(|m| m.1.as_str()).unwrap_or("");
                let mut pick = None;
                ui.horizontal_top(|ui| {
                    module_list(ui, names.iter().map(|n| (n.as_str(), "")), self.current, &mut pick);
                    ui.separator();
                    code_area(ui, egui::Id::new(("basic_code", self.current)), &mut code, &mut self.scroll_row, &mut Marks::default());
                });
                if let Some(i) = pick {
                    self.current = i;
                }
                return false;
            }
            Source::Broken(err) => {
                ui.label(RichText::new(trf("macros.bad_project", &[("error", err)])).color(ui.visuals().error_fg_color));
                return false;
            }
            Source::None => {
                ui.add_space(8.0);
                ui.label(tr("macros.none"));
                ui.label(RichText::new(tr("macros.not_run")).weak());
                return false;
            }
        };
        let mut changed = false;
        self.current = self.current.min(project.modules.len().saturating_sub(1));
        // The code as it was, for undo: whatever changes it below is a step.
        let code_id0 = egui::Id::new(("vba_code", self.current, project.modules.get(self.current).map(|m| m.name.clone()).unwrap_or_default()));
        let before = project.modules.get(self.current).map(|m| (self.current, m.name.clone(), m.code.clone(), super::macro_assist::cursor(ui.ctx(), code_id0)));
        match read_only {
            Some(key) => {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(tr(key)).color(ui.visuals().warn_fg_color));
                    if key == "macros.signed" && ui.button(tr("macros.edit_signed")).on_hover_text(tr("macros.edit_signed_tip")).clicked() {
                        self.unlock = true;
                    }
                    if key == "macros.locked" && super::macro_props::unlock_row(ui, &mut self.password, project) {
                        self.opened = true;
                    }
                });
            }
            None => {
                ui.label(RichText::new(tr("macros.not_run")).weak());
            }
        }
        // Toolbar: modules, then find.
        let editable = read_only.is_none();
        let standard = project.modules.get(self.current).is_some_and(|m| matches!(m.kind, ModuleKind::Standard | ModuleKind::Class));
        let code_id = egui::Id::new(("vba_code", self.current, project.modules.get(self.current).map(|m| m.name.clone()).unwrap_or_default()));
        let runnable = self.runs && project.modules.get(self.current).is_some_and(|m| matches!(m.kind, ModuleKind::Standard | ModuleKind::Document));
        ui.horizontal(|ui| {
            if ui.add_enabled(runnable, egui::Button::new(format!("▶ {}", tr("macros.run")))).on_hover_text(tr("macros.run_here_tip")).clicked()
                && let Some(m) = project.modules.get(self.current)
            {
                let row = cursor_row(ui.ctx(), code_id, &m.code);
                self.run = Some(macro_at(m, row));
            }
            if ui.add_enabled(runnable, egui::Button::new(tr("debug.step_into"))).on_hover_text(tr("debug.step_here_tip")).clicked()
                && let Some(m) = project.modules.get(self.current)
            {
                let row = cursor_row(ui.ctx(), code_id, &m.code);
                self.run = Some(macro_at(m, row));
                self.step = true;
            }
            if ui.button(tr("macros.browser")).on_hover_text("F2").clicked() {
                self.browser = Some(self.browser.take().unwrap_or_default());
            }
            if ui.button(tr("macros.guide")).on_hover_text(tr("macros.guide_tip")).clicked() {
                self.guide = Some(self.guide.unwrap_or(0));
            }
            if ui.button(tr("debug.compile")).on_hover_text(tr("debug.compile_tip")).clicked() {
                self.compiled = Some(compile(&project.modules));
            }
            if self.runs && ui.add_enabled(read_only != Some("macros.locked"), egui::Button::new(tr("macros.sign"))).on_hover_text(tr("macros.sign_tip")).clicked() {
                self.sign = true;
            }
            ui.add_enabled_ui(editable, |ui| {
                for (on, key) in [(true, "macros.comment"), (false, "macros.uncomment")] {
                    let text = if on { "'+" } else { "'-" };
                    if ui.button(text).on_hover_text(tr(key)).clicked()
                        && let Some(m) = project.modules.get_mut(self.current)
                        && super::macro_assist::comment(ui.ctx(), code_id, &mut m.code, on)
                    {
                        changed = true;
                    }
                }
            });
            ui.separator();
            ui.add_enabled_ui(editable, |ui| {
                ui.menu_button(format!("+ {}", tr("macros.insert")), |ui| {
                    if ui.button(tr("macros.add_form")).clicked() {
                        let name = project.next_name("UserForm");
                        let design = apex_vba::forms::Design { caption: name.clone(), ..Default::default() };
                        let mut m = VbaModule::form(&name, design.write(&name));
                        if self.explicit {
                            m.code = "Option Explicit\n\n".into();
                        }
                        project.modules.push(m);
                        self.current = project.modules.len() - 1;
                        self.form_view = true;
                        self.rename = None;
                        self.confirm_delete = false;
                        changed = true;
                    }
                    for (class, key) in [(false, "macros.add"), (true, "macros.add_class")] {
                        if ui.button(tr(key)).clicked() {
                            let name = project.next_name(if class { "Class" } else { "Module" });
                            let mut m = if class { VbaModule::class(&name) } else { VbaModule::standard(&name) };
                            if self.explicit {
                                m.code = "Option Explicit\n\n".into();
                            }
                            project.modules.push(m);
                            self.current = project.modules.len() - 1;
                            self.rename = None;
                            self.confirm_delete = false;
                            changed = true;
                        }
                    }
                });
                ui.menu_button(tr("macros.file"), |ui| {
                    if ui.button(tr("macros.import")).clicked() {
                        self.file_req = Some(FileReq::Import);
                    }
                    if ui.button(tr("macros.import_all")).on_hover_text(tr("macros.import_all_tip")).clicked() {
                        self.file_req = Some(FileReq::ImportAll);
                    }
                    if ui.add_enabled(!project.modules.is_empty(), egui::Button::new(tr("macros.export"))).clicked() {
                        self.file_req = Some(FileReq::Export(self.current));
                    }
                    if ui.add_enabled(!project.modules.is_empty(), egui::Button::new(tr("macros.export_all"))).clicked() {
                        self.file_req = Some(FileReq::ExportAll);
                    }
                    ui.separator();
                    if ui.button(tr("macros.compare")).on_hover_text(tr("macros.compare_tip")).clicked() {
                        self.file_req = Some(FileReq::Compare);
                    }
                });
                ui.menu_button(tr("macros.options"), |ui| {
                    if ui.button(tr("macros.properties")).on_hover_text(tr("macros.properties_tip")).clicked() {
                        self.props = Some(super::macro_props::Props::of(project));
                        ui.close();
                    }
                    if ui.button(tr("macros.references")).on_hover_text(tr("macros.references_tip")).clicked() {
                        self.refs = true;
                        ui.close();
                    }
                    ui.separator();
                    if ui.checkbox(&mut self.explicit, tr("macros.require_decl")).on_hover_text(tr("macros.require_decl_tip")).changed() {
                        self.prefs_changed = true;
                    }
                    if cfg!(not(target_os = "android")) && ui.checkbox(&mut self.own_window, tr("macros.own_window")).on_hover_text(tr("macros.own_window_tip")).changed() {
                        self.prefs_changed = true;
                    }
                    ui.horizontal(|ui| {
                        ui.label(tr("macros.font_size"));
                        if ui.add(egui::DragValue::new(&mut self.font_size).range(8.0..=32.0).speed(0.25)).changed() {
                            self.prefs_changed = true;
                        }
                    });
                    ui.menu_button(tr("macros.colors"), |ui| {
                        let (dark, fg) = (ui.visuals().dark_mode, ui.visuals().text_color());
                        egui::Grid::new("vba_colors").num_columns(2).show(ui, |ui| {
                            let names = ["macros.colors_keyword", "macros.colors_string", "macros.colors_comment", "macros.colors_number"];
                            for (i, ((tok, _), name)) in super::vba_code::CHOSEN.iter().zip(names).enumerate() {
                                ui.label(tr(name));
                                let mut c = super::vba_code::color(*tok, dark, fg, &self.colors);
                                if ui.color_edit_button_srgba(&mut c).changed() {
                                    self.colors[i] = Some(c);
                                    self.prefs_changed = true;
                                }
                                ui.end_row();
                            }
                        });
                        if ui.add_enabled(self.colors.iter().any(Option::is_some), egui::Button::new(tr("macros.colors_reset"))).clicked() {
                            self.colors = Default::default();
                            self.prefs_changed = true;
                        }
                    });
                });
                ui.add_enabled_ui(standard, |ui| {
                    if ui.button(tr("macros.rename")).clicked() {
                        self.rename = project.modules.get(self.current).map(|m| m.name.clone());
                        self.focus_rename = true;
                        self.confirm_delete = false;
                    }
                    if ui.button(tr("macros.delete")).clicked() {
                        self.confirm_delete = true;
                        self.rename = None;
                    }
                });
            });
            ui.separator();
            ui.separator();
            let mut panes = vec![(Bottom::Immediate, "debug.immediate", "Ctrl+G"), (Bottom::Watches, "debug.watches", "")];
            if !self.results.is_empty() {
                panes.push((Bottom::Results, "macros.results", ""));
            }
            for (b, key, tip) in panes {
                let on = self.bottom == Some(b);
                let r = ui.selectable_label(on, tr(key));
                let r = if tip.is_empty() { r } else { r.on_hover_text(tip) };
                if r.clicked() {
                    self.bottom = if on { None } else { Some(b) };
                    self.imm_focus = b == Bottom::Immediate;
                }
            }
        });
        // Find and replace (Ctrl+F, Ctrl+H), in the module or the project.
        let on_top = ui.ctx().memory(|m| m.top_modal_layer()).is_none();
        let find_id = egui::Id::new("macro_find");
        if on_top && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
            ui.memory_mut(|m| m.request_focus(find_id));
        }
        if on_top && editable && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::H)) {
            self.show_replace = true;
            ui.memory_mut(|m| m.request_focus(find_id));
        }
        let mut find_next = false;
        let mut pick_find = None;
        let mut action = None;
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut self.find.text).id(find_id).hint_text(tr("macros.find")).desired_width(180.0));
            if r.changed() {
                self.not_found = false;
                self.find_msg = None;
            }
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            for (on, label, key) in [(&mut self.find.case, "Aa", "macros.match_case"), (&mut self.find.word, "ab|", "macros.whole_word"), (&mut self.find.regex, ".*", "macros.use_regex")] {
                if ui.selectable_label(*on, RichText::new(label).monospace()).on_hover_text(tr(key)).clicked() {
                    *on = !*on;
                    self.not_found = false;
                }
            }
            egui::ComboBox::from_id_salt("macro_find_scope").selected_text(tr(if self.find_project { "macros.scope_project" } else { "macros.scope_module" })).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.find_project, false, tr("macros.scope_module"));
                ui.selectable_value(&mut self.find_project, true, tr("macros.scope_project"));
            });
            if (ui.button(tr("macros.find_next")).clicked() || enter) && !self.find.text.is_empty() {
                find_next = true;
                if enter {
                    r.request_focus();
                }
            }
            if ui.button(tr("macros.find_all")).clicked() {
                action = Some(0);
            }
            if editable && ui.selectable_label(self.show_replace, tr("macros.replace")).on_hover_text("Ctrl+H").clicked() {
                self.show_replace = !self.show_replace;
            }
            if self.not_found {
                ui.label(RichText::new(tr("macros.not_found")).color(ui.visuals().warn_fg_color));
            }
            if let Some((msg, error)) = &self.find_msg {
                let color = if *error { ui.visuals().error_fg_color } else { ui.visuals().text_color() };
                ui.label(RichText::new(msg).color(color));
            }
        });
        if self.show_replace && editable {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.replace).hint_text(tr("macros.replace_with")).desired_width(180.0));
                if ui.button(tr("macros.replace")).clicked() {
                    action = Some(1);
                }
                if ui.button(tr("macros.replace_all")).clicked() {
                    action = Some(2);
                }
            });
        }
        // An expression that doesn't compile: its error, nothing done.
        if let Err(e) = self.find.matches("") {
            self.find_msg = Some((e, true));
            find_next = false;
            action = None;
        }
        let at = super::macro_assist::cursor_range(ui.ctx(), code_id);
        // Replace: the match selected becomes the new text, then on to the next.
        if action == Some(1)
            && let Some(m) = project.modules.get_mut(self.current)
        {
            let (lo, hi) = (at.0.min(at.1), at.0.max(at.1));
            if self.find.matches(&m.code).is_ok_and(|all| all.contains(&(lo, hi))) {
                let a = m.code.char_indices().nth(lo).map_or(m.code.len(), |(i, _)| i);
                let b = m.code.char_indices().nth(hi).map_or(m.code.len(), |(i, _)| i);
                let new = self.find.replacement(&m.code[a..b], &self.replace);
                m.code.replace_range(a..b, &new);
                changed = true;
                self.select_range = Some((lo + new.chars().count(), lo + new.chars().count()));
            }
            find_next = true;
        }
        if action == Some(2) {
            let mut total = 0;
            for (i, m) in project.modules.iter_mut().enumerate() {
                if !self.find_project && i != self.current {
                    continue;
                }
                if let Ok((code, n)) = self.find.replace_all(&m.code, &self.replace)
                    && n > 0
                {
                    m.code = code;
                    total += n;
                    changed = true;
                }
            }
            self.find_msg = Some((trf("macros.replaced", &[("n", &total)]), false));
            self.results.clear();
            if self.bottom == Some(Bottom::Results) {
                self.bottom = None;
            }
        }
        if action == Some(0) {
            self.results.clear();
            for (i, m) in project.modules.iter().enumerate() {
                if !self.find_project && i != self.current {
                    continue;
                }
                let lines: Vec<&str> = m.code.split('\n').collect();
                let mut last = None;
                for (a, _) in self.find.matches(&m.code).unwrap_or_default() {
                    let row = m.code.chars().take(a).filter(|c| *c == '\n').count();
                    if last != Some(row) {
                        self.results.push((i, row, lines.get(row).map_or("", |l| l.trim()).to_string()));
                        last = Some(row);
                    }
                }
            }
            self.not_found = self.results.is_empty();
            if !self.not_found {
                self.bottom = Some(Bottom::Results);
            }
        }
        if find_next {
            let from = self.select_range.map_or(at.0.max(at.1), |r| r.1);
            let codes: Vec<&str> = project.modules.iter().map(|m| m.code.as_str()).collect();
            match find_from(&codes, self.current, from, &self.find, self.find_project) {
                Some((i, a, b)) => {
                    if i != self.current {
                        pick_find = Some(i);
                    }
                    self.select_range = Some((a, b));
                    self.not_found = false;
                }
                None => self.not_found = true,
            }
        }
        // Rename or delete, asked in place.
        if let Some(name) = &mut self.rename {
            let check = project.check_name(name, Some(self.current));
            let mut done = None;
            ui.horizontal(|ui| {
                ui.label(tr("macros.rename"));
                let r = ui.add(egui::TextEdit::singleline(name).desired_width(200.0));
                if std::mem::take(&mut self.focus_rename) {
                    r.request_focus();
                }
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.add_enabled(check.is_ok(), egui::Button::new(tr("dialog.ok"))).clicked() || (enter && check.is_ok()) {
                    done = Some(true);
                }
                if ui.button(tr("dialog.cancel")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    done = Some(false);
                }
                if let Err(err) = check {
                    ui.label(RichText::new(tr(name_error(err))).color(ui.visuals().error_fg_color));
                }
            });
            match done {
                Some(true) => {
                    let name = name.trim().to_string();
                    if let Some(m) = project.modules.get_mut(self.current)
                        && m.name != name
                    {
                        conf.rename(&m.name, &name);
                        m.name = name;
                        changed = true;
                    }
                    self.rename = None;
                }
                Some(false) => self.rename = None,
                None => {}
            }
        }
        if self.confirm_delete {
            let name = project.modules.get(self.current).map(|m| m.name.clone()).unwrap_or_default();
            ui.horizontal(|ui| {
                ui.label(trf("macros.delete_q", &[("name", &name)]));
                if ui.button(tr("macros.delete")).clicked() {
                    project.modules.remove(self.current);
                    self.current = self.current.saturating_sub(1);
                    self.confirm_delete = false;
                    changed = true;
                }
                if ui.button(tr("dialog.cancel")).clicked() {
                    self.confirm_delete = false;
                }
            });
        }
        // Syntax errors, as the VBA editor shows them: off the line being typed.
        if let Some(m) = project.modules.get(self.current) {
            let key = {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                (&m.name, &m.code).hash(&mut h);
                h.finish()
            };
            if self.syntax.as_ref().is_none_or(|s| s.0 != key) {
                self.syntax = Some((key, syntax_error(m)));
            }
            if let Some((_, Some(err))) = &self.syntax {
                let typing = ui.memory(|mem| mem.has_focus(code_id)) && cursor_row(ui.ctx(), code_id, &m.code) + 1 == err.line as usize;
                if !typing {
                    ui.horizontal(|ui| {
                        let at = trf("macros.syntax_at", &[("line", &err.line)]);
                        if ui.link(RichText::new(at).color(ui.visuals().error_fg_color).strong()).clicked() {
                            self.select_row = Some(err.line.saturating_sub(1) as usize);
                            self.scroll_row = self.select_row;
                        }
                        ui.label(RichText::new(&err.message).color(ui.visuals().error_fg_color));
                    });
                }
            }
        }
        match &self.compiled {
            Some(None) => {
                ui.label(RichText::new(tr("debug.compiled_ok")).weak());
            }
            Some(Some((i, err))) => {
                let (i, line, module) = (*i, err.line.max(1), err.module.clone());
                ui.horizontal(|ui| {
                    let at = trf("debug.compile_at", &[("module", &module), ("line", &line)]);
                    if ui.link(RichText::new(at).color(ui.visuals().error_fg_color).strong()).clicked() {
                        self.current = i;
                        self.select_row = Some(line as usize - 1);
                        self.scroll_row = self.select_row;
                    }
                    ui.label(RichText::new(&err.message).color(ui.visuals().error_fg_color));
                });
            }
            None => {}
        }
        ui.separator();
        // Ctrl+G: the Immediate window, as in the VBA editor (not while the
        // debugger, above, has the keys).
        if on_top && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::G)) {
            self.bottom = Some(Bottom::Immediate);
            self.imm_focus = true;
        }
        // A form module: its designer, kept while it is the one shown.
        let is_form = project.modules.get(self.current).is_some_and(|m| m.kind == ModuleKind::Form);
        if is_form {
            let name = project.modules[self.current].name.clone();
            if !self.designer.as_ref().is_some_and(|d| d.is(self.current, &name)) {
                self.designer = Some(super::macro_form_design::Designer::open(self.current, &name, project.form_streams(self.current).as_ref()));
            }
            // F7: its code; Mayús+F7: its form.
            if on_top && ui.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::F7)) {
                self.form_view = true;
            } else if on_top && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F7)) {
                self.form_view = false;
            }
        }
        // Sent to a line of it: its code.
        if self.select_row.is_some() || self.select_range.is_some() {
            self.form_view = false;
        }
        let form_view = is_form && self.form_view;
        // The object and procedure lists above the code, as in the VBA editor.
        if let Some(m) = project.modules.get_mut(self.current) {
            // A form's objects are it and its controls, with their events.
            let form_objects = is_form.then(|| self.designer.as_ref().map(|d| d.objects()).unwrap_or_default());
            let objects: Vec<String> = match &form_objects {
                Some(o) => std::iter::once("UserForm".to_string()).chain(o.iter().map(|o| o.0.clone())).collect(),
                None => module_object(m, self.sheet).map(|o| o.to_string()).into_iter().collect(),
            };
            if self.object.as_ref().is_some_and(|o| !objects.contains(o)) {
                self.object = None;
            }
            let row = cursor_row(ui.ctx(), code_id, &m.code);
            let procs = super::vba_edit::proc_rows(&m.code);
            ui.horizontal(|ui| {
                // Over the code, past the modules.
                ui.add_space(170.0 + ui.spacing().item_spacing.x * 2.0 + 6.0);
                if is_form {
                    if ui.selectable_label(self.form_view, tr("macros.view_object")).on_hover_text("Mayús+F7").clicked() {
                        self.form_view = true;
                    }
                    if ui.selectable_label(!self.form_view, tr("macros.view_code")).on_hover_text("F7").clicked() {
                        self.form_view = false;
                    }
                    ui.separator();
                }
                if form_view {
                    return;
                }
                let general = tr("macros.general");
                egui::ComboBox::from_id_salt("macro_object").width(200.0).selected_text(self.object.as_deref().unwrap_or(general)).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.object, None, general);
                    for o in &objects {
                        ui.selectable_value(&mut self.object, Some(o.clone()), o);
                    }
                });
                let at = procs.iter().rev().find(|p| p.1 <= row).map(|p| p.0.clone());
                match self.object.clone() {
                    None => {
                        let shown = at.clone().unwrap_or_else(|| tr("macros.declarations").to_string());
                        egui::ComboBox::from_id_salt("macro_proc").width(260.0).selected_text(shown).show_ui(ui, |ui| {
                            if ui.selectable_label(at.is_none(), tr("macros.declarations")).clicked() {
                                self.select_row = Some(0);
                                self.scroll_row = Some(0);
                            }
                            for (name, r) in &procs {
                                if ui.selectable_label(at.as_ref() == Some(name), name).clicked() {
                                    self.select_row = Some(*r);
                                    self.scroll_row = Some(*r);
                                }
                            }
                        });
                    }
                    Some(o) => {
                        let prefix = format!("{}_", o.to_lowercase());
                        let shown = at.as_ref().and_then(|a| a.to_lowercase().strip_prefix(&prefix).and_then(|_| a.get(prefix.len()..)).map(str::to_string)).unwrap_or_default();
                        // (name, parameters, help)
                        let events: Vec<(&str, &str, &str)> = match &form_objects {
                            Some(objs) => {
                                let kind = objs.iter().find(|x| x.0 == o).map(|x| x.1);
                                super::macro_form_design::events(kind).into_iter().map(|(n, p)| (n, p, "")).collect()
                            }
                            None => apex_vba::catalog::class(&o).map_or(&[][..], |c| &c.members).iter().filter(|e| e.kind == apex_vba::catalog::Kind::Event).map(|e| (e.name, e.params, e.help)).collect(),
                        };
                        egui::ComboBox::from_id_salt("macro_event").width(260.0).selected_text(shown).show_ui(ui, |ui| {
                            for (name, params, help) in events {
                                let has = procs.iter().any(|p| p.0.eq_ignore_ascii_case(&format!("{o}_{name}")));
                                let text = if has { RichText::new(name).strong() } else { RichText::new(name) };
                                let r = ui.selectable_label(false, text);
                                let r = if help.is_empty() { r } else { r.on_hover_text(help) };
                                if r.clicked() {
                                    match super::vba_edit::event_proc(&m.code, &o, name, params) {
                                        Ok(r) => self.select_row = Some(r),
                                        Err((code, r)) if editable => {
                                            m.code = code;
                                            changed = true;
                                            self.select_row = Some(r);
                                        }
                                        Err(_) => {}
                                    }
                                    self.scroll_row = self.select_row;
                                }
                            }
                        });
                    }
                }
            });
        }
        // The Immediate window or the watches go under the code.
        // Fixed heights that add up to less than the room: the window grows
        // to fit its contents, so a fraction of it would grow every frame.
        let bottom_h = if self.bottom.is_some() { 170.0 } else { 0.0 };
        let gap = if self.bottom.is_some() { 6.0 + 3.0 * ui.spacing().item_spacing.y } else { 0.0 };
        let code_h = (ui.available_height() - bottom_h - gap).max(120.0);
        let mut pick = pick_find;
        let mut undone = false;
        let mut help_word = None;
        let (mut goto_def, mut go_back) = (None, false);
        let width = ui.available_width();
        let focused = editable && ui.memory(|mem| mem.has_focus(code_id));
        let info = focused.then(|| assist_info(project, self.current, self.sheet, self.runs && !self.sheet));
        ui.allocate_ui(egui::vec2(width, code_h), |ui| {
            ui.set_max_height(code_h);
            ui.horizontal_top(|ui| {
                project_tree(ui, project, self.current, &mut pick, &self.targets, &mut self.copy);
                ui.separator();
                if form_view
                    && let (Some(m), Some(d)) = (project.modules.get_mut(self.current), self.designer.as_mut())
                {
                    ui.vertical(|ui| {
                        let out = d.ui(ui, editable, &m.name);
                        if out.changed {
                            m.form = Some(d.design.write(&m.name));
                            changed = true;
                        }
                        // A double click: to the code of the event.
                        if let Some((object, event, params)) = out.code {
                            match super::vba_edit::event_proc(&m.code, &object, event, params) {
                                Ok(r) => self.select_row = Some(r),
                                Err((code, r)) if editable => {
                                    m.code = code;
                                    changed = true;
                                    self.select_row = Some(r);
                                }
                                Err(_) => {}
                            }
                            self.scroll_row = self.select_row;
                            self.form_view = false;
                            self.object = Some(object);
                        }
                    });
                } else if let Some(m) = project.modules.get_mut(self.current) {
                    if let Some(row) = self.select_row.take() {
                        select_row(ui.ctx(), code_id, &m.code, row);
                        self.focus_code = true;
                    }
                    if let Some((a, b)) = self.select_range.take() {
                        super::macro_assist::select(ui.ctx(), code_id, a, b);
                        self.scroll_row = Some(m.code.chars().take(a).filter(|c| *c == '\n').count());
                        self.focus_code = true;
                    }
                    // Not in the frame of a click: the code would let go of
                    // the focus, clicked elsewhere.
                    if self.focus_code && !ui.input(|i| i.pointer.any_click()) {
                        ui.memory_mut(|mem| mem.request_focus(code_id));
                        self.focus_code = false;
                    }
                    let lines = conf.lines(&m.name);
                    let separators: Vec<u32> = super::vba_edit::proc_rows(&m.code).iter().filter(|p| p.1 > 0).map(|p| p.1 as u32 + 1).collect();
                    let mut marks = Marks { breakpoints: &lines, separators: &separators, font_size: self.font_size, colors: self.colors, ..Default::default() };
                    let before = (m.code.split('\n').count(), cursor_row(ui.ctx(), code_id, &m.code));
                    if editable {
                        if focused && super::macro_assist::undo_keys(ui, code_id, &mut m.code, self.history.entry(m.name.clone()).or_default()) {
                            changed = true;
                            undone = true;
                        }
                        if let Some(info) = &info {
                            changed |= super::macro_assist::before(ui, code_id, &mut m.code, &mut self.assist, info);
                        }
                        let edited = code_area(ui, code_id, &mut m.code, &mut self.scroll_row, &mut marks);
                        if let Some(info) = &info {
                            changed |= super::macro_assist::after(ui, code_id, &mut m.code, &mut self.assist, info, edited, marks.cursor);
                        }
                        if edited {
                            changed = true;
                            // Breakpoints below the lines added or removed move with their code.
                            let delta = m.code.split('\n').count() as i64 - before.0 as i64;
                            conf.shift(&m.name, before.1 as u32 + 1, delta);
                        }
                    } else {
                        code_area(ui, code_id, &mut m.code.as_str(), &mut self.scroll_row, &mut marks);
                    }
                    if let Some(line) = marks.clicked {
                        conf.toggle(&m.name, line);
                    }
                    // The VBA editor's keys in the code: F5 runs the macro at the
                    // cursor, F8 steps through it, F9 sets a breakpoint.
                    if ui.memory(|mem| mem.has_focus(code_id)) {
                        let row = cursor_row(ui.ctx(), code_id, &m.code);
                        if runnable && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F5)) {
                            self.run = Some(macro_at(m, row));
                        }
                        if runnable && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F8)) {
                            self.run = Some(macro_at(m, row));
                            self.step = true;
                        }
                        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F9)) {
                            conf.toggle(&m.name, row as u32 + 1);
                        }
                        // F2: the Object Browser; F1: it, at the word under the cursor.
                        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F2)) {
                            self.browser = Some(self.browser.take().unwrap_or_default());
                        }
                        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F1)) {
                            let at = super::macro_assist::cursor(ui.ctx(), code_id);
                            help_word = word_at(&m.code, at).or_else(|| word_at(&m.code, at.saturating_sub(1)));
                            self.browser = Some(self.browser.take().unwrap_or_default());
                        }
                        // Mayús+F2: to where the name at the cursor is declared;
                        // Ctrl+Mayús+F2: back.
                        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::F2)) {
                            go_back = true;
                        } else if ui.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::F2)) {
                            let at = super::macro_assist::cursor(ui.ctx(), code_id);
                            goto_def = word_at(&m.code, at).or_else(|| word_at(&m.code, at.saturating_sub(1))).map(|w| (w, row));
                        }
                    }
                }
            });
        });
        if let Some(b) = self.bottom {
            ui.separator();
            ui.allocate_ui(egui::vec2(width, bottom_h), |ui| {
                ui.set_max_height(bottom_h);
                match b {
                    Bottom::Immediate => {
                        if !self.runs {
                            ui.label(RichText::new(tr("macros.sheets_only")).weak());
                        }
                        if let Some(line) = immediate_pane(ui, conf, &mut self.imm_input, &mut self.imm_focus, &mut self.hist_at, self.runs) {
                            self.immediate = Some(line);
                        }
                    }
                    Bottom::Watches => {
                        watch_list(ui, &mut conf.watches, None, &mut self.new_watch, &mut self.watch_kind);
                    }
                    Bottom::Results => {
                        egui::ScrollArea::vertical().id_salt("macro_results").auto_shrink([false, false]).show(ui, |ui| {
                            for (i, row, line) in &self.results {
                                let name = project.modules.get(*i).map_or("", |m| m.name.as_str());
                                if ui.selectable_label(false, RichText::new(format!("{name} ({}): {line}", row + 1)).monospace()).clicked() {
                                    if *i != self.current {
                                        pick = Some(*i);
                                    }
                                    self.select_row = Some(*row);
                                    self.scroll_row = Some(*row);
                                }
                            }
                        });
                    }
                }
            });
        }
        if let Some((word, row)) = goto_def {
            let codes: Vec<&str> = project.modules.iter().map(|m| m.code.as_str()).collect();
            if let Some((i, r)) = super::vba_edit::definition(&codes, self.current, row, &word) {
                self.back.push((self.current, row));
                if i != self.current {
                    pick = Some(i);
                }
                self.select_row = Some(r);
                self.scroll_row = Some(r);
            }
        }
        super::macro_props::references(ui.ctx(), &mut self.refs, project);
        super::macro_compare::show(ui.ctx(), &mut self.compare, project);
        super::macro_guide::show(ui.ctx(), &mut self.guide);
        if super::macro_props::show(ui.ctx(), &mut self.props, project) {
            // Locked here: still open for whoever locked it.
            self.opened = true;
            changed = true;
        }
        if self.browser.is_some() {
            let modules: Vec<(String, String)> = project.modules.iter().map(|m| (m.name.clone(), m.code.clone())).collect();
            if let Some(w) = help_word
                && let Some(b) = super::macro_browser::lookup(&w, &modules, self.runs && !self.sheet)
            {
                self.browser = Some(b);
            }
            if let Some((i, r)) = super::macro_browser::show(ui.ctx(), &mut self.browser, &modules, self.runs && !self.sheet) {
                if i != self.current {
                    pick = Some(i);
                }
                self.select_row = Some(r);
                self.scroll_row = Some(r);
            }
        }
        if go_back && let Some((i, r)) = self.back.pop() {
            if i != self.current && i < project.modules.len() {
                pick = Some(i);
            }
            self.select_row = Some(r);
            self.scroll_row = Some(r);
        }
        if let Some(i) = pick {
            self.current = i;
            self.object = None;
            self.form_view = true;
            self.rename = None;
            self.confirm_delete = false;
            self.not_found = false;
        }
        if let Some((i, name, old, at)) = before
            && !undone
            && let Some(m) = project.modules.get(i).filter(|m| m.name == name && m.code != old)
        {
            let now = ui.input(|i| i.time);
            self.history.entry(name).or_default().record(old, at, &m.code, now);
        }
        if changed {
            self.compiled = None;
        }
        changed
    }
}

/// The object of a module for its events: its class in the catalog.
fn module_object(m: &VbaModule, sheet: bool) -> Option<&'static str> {
    match m.kind {
        ModuleKind::Document if !sheet => Some("Document"),
        ModuleKind::Document if m.attributes.contains("00020819-0000-0000-C000-000000000046") => Some("Workbook"),
        ModuleKind::Document => Some("Worksheet"),
        ModuleKind::Class => Some("Class"),
        _ => None,
    }
}

/// What completion sees of the project from module `current`.
/// `word`: a document's macros, which see Word's object model.
fn assist_info(project: &VbaProject, current: usize, sheet: bool, word: bool) -> super::macro_assist::Info {
    // Excel's document modules: the workbook's and the sheets'.
    let class = |m: &VbaModule| if m.attributes.contains("00020819-0000-0000-C000-000000000046") { "Workbook" } else { "Worksheet" };
    let objects = if sheet {
        project.modules.iter().filter(|m| m.kind == ModuleKind::Document).map(|m| (m.name.clone(), class(m))).collect()
    } else if word {
        project.modules.iter().filter(|m| m.kind == ModuleKind::Document).map(|m| (m.name.clone(), "Document")).collect()
    } else {
        Vec::new()
    };
    let me = project.modules.get(current).filter(|m| (sheet || word) && m.kind == ModuleKind::Document).map(|m| if word { "Document" } else { class(m) });
    let mut procs = Vec::new();
    for (i, m) in project.modules.iter().enumerate() {
        if i == current || m.kind == ModuleKind::Standard {
            procs.extend(super::vba_edit::procedures(&m.code));
        }
    }
    let classes = project.modules.iter().filter(|m| m.kind == ModuleKind::Class).map(|m| m.name.clone()).collect();
    let sources = project.modules.iter().map(|m| m.code.clone()).collect();
    super::macro_assist::Info { word, me, objects, procs, classes, sources }
}

/// The project on the left as the VBA editor shows it: objects, forms,
/// modules and class modules, and the properties of the one picked.
/// Right-clicking a module copies it to another open workbook.
fn project_tree(ui: &mut egui::Ui, project: &VbaProject, current: usize, pick: &mut Option<usize>, targets: &[(u64, String)], copy: &mut Option<(usize, u64)>) {
    ui.vertical(|ui| {
        ui.set_width(170.0);
        let props_h = 64.0;
        let h = (ui.available_height() - props_h).max(60.0);
        egui::ScrollArea::vertical().id_salt("macro_modules").auto_shrink([false, false]).max_height(h).show(ui, |ui| {
            for (kind, key) in [(ModuleKind::Document, "macros.group_objects"), (ModuleKind::Form, "macros.group_forms"), (ModuleKind::Standard, "macros.group_modules"), (ModuleKind::Class, "macros.group_classes")] {
                let members: Vec<(usize, &VbaModule)> = project.modules.iter().enumerate().filter(|m| m.1.kind == kind).collect();
                if members.is_empty() {
                    continue;
                }
                egui::CollapsingHeader::new(tr(key)).id_salt(("macro_group", key)).default_open(true).show(ui, |ui| {
                    for (i, m) in members {
                        let r = ui.selectable_label(i == current, &m.name);
                        if r.clicked() {
                            *pick = Some(i);
                        }
                        if kind != ModuleKind::Document && !targets.is_empty() {
                            r.context_menu(|ui| {
                                ui.label(RichText::new(tr("macros.copy_to")).weak());
                                for (tab, name) in targets {
                                    if ui.button(name).clicked() {
                                        *copy = Some((i, *tab));
                                        ui.close();
                                    }
                                }
                            });
                        }
                    }
                });
            }
        });
        ui.separator();
        if let Some(m) = project.modules.get(current) {
            egui::Grid::new("macro_props").num_columns(2).spacing([8.0, 2.0]).show(ui, |ui| {
                ui.label(RichText::new("(Name)").small().weak());
                ui.label(RichText::new(&m.name).small());
                ui.end_row();
                ui.label(RichText::new(tr("macros.kind")).small().weak());
                ui.label(RichText::new(tr(kind_key(m.kind))).small());
                ui.end_row();
                if m.kind == ModuleKind::Class {
                    let on = |k: &str| m.attributes.lines().any(|l| l.starts_with(&format!("Attribute {k} = True")));
                    ui.label(RichText::new("PredeclaredId").small().weak());
                    ui.label(RichText::new(if on("VB_PredeclaredId") { "True" } else { "False" }).small());
                    ui.end_row();
                }
            });
        }
    });
}

/// The modules on the left: name and kind; sets `pick` when one is clicked.
fn module_list<'a>(ui: &mut egui::Ui, entries: impl Iterator<Item = (&'a str, &'a str)>, current: usize, pick: &mut Option<usize>) {
    ui.vertical(|ui| {
        ui.set_width(170.0);
        egui::ScrollArea::vertical().id_salt("macro_modules").auto_shrink([false, false]).max_height(ui.available_height()).show(ui, |ui| {
            for (i, (name, kind)) in entries.enumerate() {
                let r = ui.horizontal(|ui| {
                    let r = ui.selectable_label(i == current, name);
                    if !kind.is_empty() {
                        ui.label(RichText::new(kind).small().weak());
                    }
                    r
                }).inner;
                if r.clicked() {
                    *pick = Some(i);
                }
            }
        });
    });
}

/// Debugging marks of the code shown, and what the user did with them.
#[derive(Default)]
pub(super) struct Marks<'a> {
    /// Lines (1-based) with a breakpoint.
    pub breakpoints: &'a [u32],
    /// The next statement to run (highlighted yellow, like VBA).
    pub current: Option<u32>,
    /// A caller's line picked in the call stack (green).
    pub frame: Option<u32>,
    /// Out: the line whose number was clicked (toggles its breakpoint).
    pub clicked: Option<u32>,
    /// A variable's value (by its name, lower case), shown when the
    /// pointer is on it.
    pub values: Option<&'a dyn Fn(&str) -> Option<String>>,
    /// The code's size (0: the usual 13).
    pub font_size: f32,
    /// Syntax colors chosen.
    pub colors: super::vba_code::Colors,
    /// Lines (1-based) a procedure starts at: a line above them.
    pub separators: &'a [u32],
    /// Out: where the cursor is on screen.
    pub cursor: Option<egui::Rect>,
}

/// The identifier at char `at` of `text`, lower case.
fn word_at(text: &str, at: usize) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    if !chars.get(at).copied().is_some_and(is_word) {
        return None;
    }
    let mut a = at;
    while a > 0 && is_word(chars[a - 1]) {
        a -= 1;
    }
    let mut b = at;
    while b < chars.len() && is_word(chars[b]) {
        b += 1;
    }
    let w: String = chars[a..b].iter().collect();
    w.starts_with(|c: char| c.is_alphabetic()).then(|| w.to_lowercase())
}

/// The code with line numbers and colors; returns whether it changed.
pub(super) fn code_area(ui: &mut egui::Ui, id: egui::Id, code: &mut dyn egui::TextBuffer, scroll_row: &mut Option<usize>, marks: &mut Marks) -> bool {
    let font = FontId::monospace(if marks.font_size > 0.0 { marks.font_size } else { 13.0 });
    let colors = marks.colors;
    let dark = ui.visuals().dark_mode;
    let fg = ui.visuals().text_color();
    let row_h = ui.fonts_mut(|f| f.row_height(&font));
    let char_w = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
    let rows = code.as_str().split('\n').count().max(1);
    let digits = rows.to_string().len().max(2);
    let mut changed = false;
    egui::ScrollArea::both().id_salt(id.with("scroll")).auto_shrink([false, false]).show(ui, |ui| {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            // The margin: line numbers, breakpoints (click to set one) and
            // the line running.
            let top = 2.0;
            let size = egui::vec2(14.0 + digits as f32 * char_w, top * 2.0 + rows as f32 * row_h);
            let (rect, gutter) = ui.allocate_exact_size(size, egui::Sense::click());
            let painter = ui.painter_at(rect);
            let visible = ui.clip_rect();
            let weak = ui.visuals().weak_text_color();
            let first = (((visible.top() - rect.top() - top) / row_h).floor().max(0.0)) as usize;
            let last = ((((visible.bottom() - rect.top() - top) / row_h).ceil()) as usize).min(rows);
            for row in first..last {
                let line = row as u32 + 1;
                let y = rect.top() + top + row as f32 * row_h;
                painter.text(egui::pos2(rect.right(), y), egui::Align2::RIGHT_TOP, line.to_string(), font.clone(), weak);
                let dot = egui::pos2(rect.left() + 6.0, y + row_h / 2.0);
                if marks.breakpoints.contains(&line) {
                    // Larger under the arrow, so both show.
                    let r = if marks.current == Some(line) { 6.5 } else { 5.0 };
                    painter.circle_filled(dot, r, egui::Color32::from_rgb(0xC0, 0x20, 0x20));
                }
                if marks.current == Some(line) {
                    painter.add(egui::Shape::convex_polygon(vec![dot + egui::vec2(-4.0, -3.5), dot + egui::vec2(3.0, 0.0), dot + egui::vec2(-4.0, 3.5)], egui::Color32::from_rgb(0xE8, 0xC0, 0x10), egui::Stroke::new(1.0, egui::Color32::BLACK)));
                }
            }
            if gutter.clicked()
                && let Some(p) = gutter.interact_pointer_pos()
            {
                let row = ((p.y - rect.top() - top) / row_h).floor();
                if row >= 0.0 && (row as usize) < rows {
                    marks.clicked = Some(row as u32 + 1);
                }
            }
            // The line running and the breakpoints' lines, behind the text.
            let back = ui.painter().add(egui::Shape::Noop);
            let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, _wrap: f32| ui.fonts_mut(|f| f.layout_job(super::vba_code::job(text.as_str(), font.clone(), dark, fg, &colors)));
            let out = egui::TextEdit::multiline(code)
                .id(id)
                .font(font.clone())
                .code_editor()
                .frame(egui::Frame::NONE)
                .margin(egui::Margin::symmetric(4, 2))
                .desired_width(f32::INFINITY)
                .desired_rows(24)
                .layouter(&mut layouter)
                .show(ui);
            let r = &out.response.response;
            changed = r.changed();
            marks.cursor = out.cursor_range.map(|c| out.galley.pos_from_cursor(c.primary).translate(out.galley_pos.to_vec2()));
            let band = |line: u32| egui::Rect::from_min_size(egui::pos2(r.rect.left(), r.rect.top() + top + (line - 1) as f32 * row_h), egui::vec2(r.rect.width(), row_h));
            let mut shapes = Vec::new();
            for &line in marks.breakpoints {
                if line >= 1 && (line as usize) <= rows {
                    shapes.push(egui::Shape::rect_filled(band(line), 0.0, egui::Color32::from_rgba_unmultiplied(0xC0, 0x20, 0x20, if dark { 70 } else { 50 })));
                }
            }
            if let Some(line) = marks.frame.filter(|l| *l >= 1 && (*l as usize) <= rows) {
                shapes.push(egui::Shape::rect_filled(band(line), 0.0, egui::Color32::from_rgba_unmultiplied(0x30, 0xA0, 0x50, if dark { 80 } else { 60 })));
            }
            if let Some(line) = marks.current.filter(|l| *l >= 1 && (*l as usize) <= rows) {
                shapes.push(egui::Shape::rect_filled(band(line), 0.0, egui::Color32::from_rgba_unmultiplied(0xF0, 0xD0, 0x20, if dark { 90 } else { 120 })));
            }
            let line_color = ui.visuals().widgets.noninteractive.bg_stroke.color;
            for &line in marks.separators {
                if line >= 1 && (line as usize) <= rows {
                    let y = band(line).top();
                    shapes.push(egui::Shape::hline(r.rect.x_range(), y, egui::Stroke::new(1.0, line_color)));
                }
            }
            ui.painter().set(back, egui::Shape::Vec(shapes));
            if let (Some(values), Some(p)) = (marks.values, r.hover_pos()) {
                let at = out.galley.cursor_from_pos(p - out.galley_pos).index;
                if let Some(text) = word_at(code.as_str(), at.into()).and_then(|w| values(&w)) {
                    r.clone().on_hover_text_at_pointer(RichText::new(text).monospace());
                }
            }
            if let Some(row) = scroll_row.take() {
                let y = r.rect.min.y + top + row as f32 * row_h;
                ui.scroll_to_rect(egui::Rect::from_min_size(egui::pos2(r.rect.min.x, y), egui::vec2(8.0, row_h)), Some(egui::Align::Center));
            }
        });
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Macros written in a workbook that had none: a new project with a
    /// module per sheet (also one added while the editor was open), kept
    /// through .xlsm and asked about before an .xlsx drops it.
    #[test]
    fn new_projects_in_documents_and_presentations() {
        let ctx = egui::Context::default();
        let mut app = App::blank(&ctx, crate::settings::Settings::default(), None, web_time::Instant::now());
        app.push_tab(View::Doc(Box::new(crate::doc_view::DocView::new(apex_doc::Document::new()))), None, None);
        app.push_tab(View::Slides(Box::new(crate::slides_view::SlidesView::new(apex_slides::build::new_deck(0)))), None, None);
        for (tab, modules) in [(0, vec!["ThisDocument"]), (1, vec![])] {
            app.active = tab;
            app.open_macros();
            let e = app.macros.editor.as_mut().unwrap();
            let Source::Vba { project, read_only: None } = &mut e.source else { panic!("a new project") };
            assert_eq!(project.modules.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), modules);
            let mut m = VbaModule::standard(&project.next_name("Module"));
            m.code = "Sub Hola()\nEnd Sub\n".into();
            project.modules.push(m);
            e.dirty = true;
            let (path, fmt) = if tab == 0 { ("/x/a.docx", Format::Docx) } else { ("/x/a.pptx", Format::Pptx) };
            // Saving without macros asks; the project is in the file now.
            assert!(!app.macros_fit(tab, std::path::Path::new(path), fmt));
            app.macros.prompt = None;
            let bin = macros_of(&mut app.tabs[tab].view).project.clone().unwrap();
            let p = apex_io::vba::read(&bin).unwrap();
            assert!(p.modules.iter().any(|m| m.code.contains("Sub Hola")));
            assert!(app.macros_fit(tab, std::path::Path::new(&path.replace('x', "m")), fmt));
        }
    }

    #[test]
    fn new_project_in_a_workbook() {
        let ctx = egui::Context::default();
        let mut app = App::blank(&ctx, crate::settings::Settings::default(), None, web_time::Instant::now());
        let mut wb = apex_sheet::Workbook::new();
        wb.sheets.push(apex_sheet::Sheet::new("Datos"));
        app.push_tab(View::Sheet(Box::new(crate::sheet_view::SheetView::new(wb))), None, None);
        app.active = 0;
        app.open_macros();
        let e = app.macros.editor.as_mut().unwrap();
        let Source::Vba { project, read_only: None } = &mut e.source else { panic!("a new project") };
        let shown: Vec<&str> = project.modules.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(shown, ["ThisWorkbook", "Sheet1", "Sheet2"]);
        // Only shown: the workbook has no macros until something changes.
        let View::Sheet(v) = &app.tabs[0].view else { unreachable!() };
        assert!(v.wb.macros.project.is_none() && v.wb.code_name.is_none() && !v.wb.modified);
        let mut m = VbaModule::standard(&project.next_name("Module"));
        m.code = "Sub Hola()\n    Range(\"A1\").Value = 5\nEnd Sub\n".into();
        project.modules.push(m);
        e.dirty = true;
        // A sheet added meanwhile gets its own module on the way in.
        let View::Sheet(v) = &mut app.tabs[0].view else { unreachable!() };
        v.wb.sheets.push(apex_sheet::Sheet::new("Más"));
        // Saving as .xlsx asks first (the editor's changes count).
        assert!(!app.macros_fit(0, std::path::Path::new("/x/libro.xlsx"), Format::Xlsx));
        let View::Sheet(v) = &app.tabs[0].view else { unreachable!() };
        assert_eq!(v.wb.code_name.as_deref(), Some("ThisWorkbook"));
        let codes: Vec<Option<&str>> = v.wb.sheets.iter().map(|s| s.code_name.as_deref()).collect();
        assert_eq!(codes, [Some("Sheet1"), Some("Sheet2"), Some("Sheet3")]);
        // Through .xlsm: every code name has its module, and the code runs
        // from what was saved.
        let bytes = apex_io::xlsx::write_as(&v.wb, apex_io::Flavor::from_path(std::path::Path::new("libro.xlsm")));
        let back = apex_io::xlsx::read(&bytes).unwrap();
        assert_eq!(back.code_name.as_deref(), Some("ThisWorkbook"));
        let p = apex_io::vba::read(back.macros.project.as_ref().unwrap()).unwrap();
        let modules: Vec<(&str, ModuleKind)> = p.modules.iter().map(|m| (m.name.as_str(), m.kind)).collect();
        assert_eq!(modules, [("ThisWorkbook", ModuleKind::Document), ("Sheet1", ModuleKind::Document), ("Sheet2", ModuleKind::Document), ("Module1", ModuleKind::Standard), ("Sheet3", ModuleKind::Document)]);
        assert!(!p.protected);
        assert!(p.modules[3].code.contains("Range(\"A1\").Value = 5"));
    }

    #[test]
    fn syntax_errors_and_the_macro_at_the_cursor() {
        let mut m = VbaModule::standard("Module1");
        m.code = "Sub Uno()\nEnd Sub\n\nSub Dos()\n    x = 1\nEnd Sub\n".into();
        assert!(syntax_error(&m).is_none());
        assert_eq!(macro_at(&m, 0).as_deref(), Some("Uno"));
        assert_eq!(macro_at(&m, 4).as_deref(), Some("Dos"));
        m.code = "Sub Uno()\n    If x Then\nEnd Sub\n".into();
        let e = syntax_error(&m).unwrap();
        assert_eq!(e.line, 3);
        assert_eq!(macro_at(&m, 0), None);
        let mut d = VbaModule::sheet("Sheet1");
        d.code = "Sub Hola()\nEnd Sub\n".into();
        assert_eq!(macro_at(&d, 0).as_deref(), Some("Sheet1.Hola"));
    }

    #[test]
    fn importing_modules() {
        let ctx = egui::Context::default();
        let mut app = App::blank(&ctx, crate::settings::Settings::default(), None, web_time::Instant::now());
        app.push_tab(View::Sheet(Box::new(crate::sheet_view::SheetView::new(apex_sheet::Workbook::new()))), None, None);
        app.active = 0;
        app.open_macros();
        let dir = apex_fs::temp_dir().join(format!("apex-vba-import-{}", apex_fs::process_id()));
        apex_fs::create_dir_all(&dir).unwrap();
        let mut c = VbaModule::class("Point");
        c.code = "Public X As Long\n".into();
        apex_fs::write(dir.join("Point.cls"), c.export(1252)).unwrap();
        apex_fs::write(dir.join("Sheet1.bas"), b"Sub A()\nEnd Sub\n").unwrap();
        app.import_modules(vec![dir.join("Point.cls"), dir.join("Sheet1.bas"), dir.join("missing.bas")], 1252);
        let _ = apex_fs::remove_dir_all(&dir);
        let e = app.macros.editor.as_ref().unwrap();
        let Source::Vba { project, .. } = &e.source else { panic!() };
        let added: Vec<(&str, ModuleKind)> = project.modules.iter().skip(2).map(|m| (m.name.as_str(), m.kind)).collect();
        // The name taken (a sheet's) gets a number.
        assert_eq!(added, [("Point", ModuleKind::Class), ("Sheet11", ModuleKind::Standard)]);
        assert!(e.dirty && e.current == 3);
        assert_eq!(app.toasts.len(), 1);
    }

    #[test]
    fn copying_a_module_to_another_workbook() {
        let ctx = egui::Context::default();
        let mut app = App::blank(&ctx, crate::settings::Settings::default(), None, web_time::Instant::now());
        for _ in 0..2 {
            app.push_tab(View::Sheet(Box::new(crate::sheet_view::SheetView::new(apex_sheet::Workbook::new()))), None, None);
        }
        let to = app.tabs[1].id;
        // The target has a Module1 already.
        assert_eq!(app.write_module(to, "Module1", "Sub Old()\nEnd Sub\n", false), Ok(true));
        app.active = 0;
        app.open_macros();
        let e = app.macros.editor.as_mut().unwrap();
        let Source::Vba { project, .. } = &mut e.source else { panic!() };
        let mut m = VbaModule::standard("Module1");
        m.code = "Sub Hola()\nEnd Sub\n".into();
        project.modules.push(m);
        let module = project.modules.len() - 1;
        // Not the sheets' modules, nor into its own workbook.
        assert!(app.copy_module_to(0, to).is_err());
        assert!(app.copy_module_to(module, app.tabs[0].id).is_err());
        assert_eq!(app.copy_module_to(module, to).as_deref(), Ok("Module11"));
        let bin = macros_of(&mut app.tabs[1].view).project.clone().unwrap();
        let p = apex_io::vba::read(&bin).unwrap();
        let m = p.modules.iter().find(|m| m.name == "Module11").unwrap();
        assert!(m.code.contains("Sub Hola") && m.kind == ModuleKind::Standard);
        assert!(p.modules.iter().any(|m| m.name == "Module1" && m.code.contains("Sub Old")));
        let View::Sheet(v) = &app.tabs[1].view else { unreachable!() };
        assert!(v.wb.modified);
    }

    #[test]
    fn forms_go_out_and_back_as_frm_and_frx() {
        let ctx = egui::Context::default();
        let mut app = App::blank(&ctx, crate::settings::Settings::default(), None, web_time::Instant::now());
        app.push_tab(View::Sheet(Box::new(crate::sheet_view::SheetView::new(apex_sheet::Workbook::new()))), None, None);
        app.active = 0;
        app.open_macros();
        let design = apex_vba::forms::Design { caption: "Datos".into(), ..Default::default() };
        let mut form = VbaModule::form("UserForm1", design.write("UserForm1"));
        form.code = "Private Sub UserForm_Click()\nEnd Sub\n".into();
        let i = {
            let Source::Vba { project, .. } = &mut app.macros.editor.as_mut().unwrap().source else { panic!() };
            project.modules.push(form);
            project.modules.len() - 1
        };
        let dir = apex_fs::temp_dir().join(format!("apex-vba-forms-{}", apex_fs::process_id()));
        apex_fs::create_dir_all(&dir).unwrap();
        app.export_form(i, &dir.join("Datos.frm"), 1252).unwrap();
        let frm = apex_fs::read_to_string(dir.join("Datos.frm")).unwrap();
        assert!(frm.contains("OleObjectBlob   =   \"Datos.frx\":0000") && dir.join("Datos.frx").fs_exists(), "{frm}");
        app.import_modules(vec![dir.join("Datos.frm")], 1252);
        let _ = apex_fs::remove_dir_all(&dir);
        let e = app.macros.editor.as_ref().unwrap();
        let Source::Vba { project, .. } = &e.source else { panic!() };
        let m = project.modules.last().unwrap();
        // The name taken gets a number; the design comes with it.
        assert_eq!((m.name.as_str(), m.kind), ("UserForm11", ModuleKind::Form));
        assert_eq!(m.code, "Private Sub UserForm_Click()\nEnd Sub\n");
        let back = apex_vba::forms::Design::read(m.form.as_ref().unwrap());
        assert_eq!(back.caption, "Datos");
        assert!(app.toasts.is_empty());
    }

    #[test]
    fn importing_a_folder() {
        let ctx = egui::Context::default();
        let mut app = App::blank(&ctx, crate::settings::Settings::default(), None, web_time::Instant::now());
        app.push_tab(View::Sheet(Box::new(crate::sheet_view::SheetView::new(apex_sheet::Workbook::new()))), None, None);
        app.active = 0;
        app.open_macros();
        let dir = apex_fs::temp_dir().join(format!("apex-vba-folder-{}", apex_fs::process_id()));
        apex_fs::create_dir_all(&dir).unwrap();
        {
            let e = app.macros.editor.as_ref().unwrap();
            let Source::Vba { project, .. } = &e.source else { panic!() };
            // Exported, then edited outside.
            for m in &project.modules {
                let mut m = m.clone();
                m.code = format!("' edited {}\n", m.name);
                apex_fs::write(dir.join(format!("{}.{}", m.name, m.file_ext())), m.export(1252)).unwrap();
            }
        }
        apex_fs::write(dir.join("Tools.bas"), b"Sub T()\nEnd Sub\n").unwrap();
        apex_fs::write(dir.join("notes.txt"), b"not code").unwrap();
        app.import_folder(&dir, 1252);
        let _ = apex_fs::remove_dir_all(&dir);
        let e = app.macros.editor.as_ref().unwrap();
        let Source::Vba { project, .. } = &e.source else { panic!() };
        let wb = &project.modules[0];
        assert_eq!((wb.kind, wb.code.as_str()), (ModuleKind::Document, "' edited ThisWorkbook\n"));
        assert!(project.modules.iter().any(|m| m.name == "Tools" && m.kind == ModuleKind::Standard));
        assert_eq!(project.modules.len(), 3);
        assert!(e.dirty);
    }

    #[test]
    fn find_in_the_project() {
        let f = super::super::vba_edit::Find { text: "x".into(), word: true, ..Default::default() };
        let codes = ["a = x", "b", "x = x + 1"];
        assert_eq!(find_from(&codes, 0, 0, &f, false), Some((0, 4, 5)));
        assert_eq!(find_from(&codes, 0, 5, &f, false), Some((0, 4, 5)));
        assert_eq!(find_from(&codes, 0, 5, &f, true), Some((2, 0, 1)));
        assert_eq!(find_from(&codes, 2, 5, &f, true), Some((0, 4, 5)));
        assert_eq!(find_from(&codes, 1, 0, &f, false), None);
    }

    #[test]
    fn compiling_the_project() {
        let mut a = VbaModule::standard("Module1");
        a.code = "Sub Uno()\n    Dos\nEnd Sub\n".into();
        let mut b = VbaModule::standard("Module2");
        b.code = "Option Explicit\nSub Dos()\n    n = 1\nEnd Sub\n".into();
        let (i, e) = compile(&[a.clone(), b.clone()]).unwrap();
        assert_eq!((i, e.line), (1, 3));
        b.code = "Sub Dos(\nEnd Sub\n".into();
        assert_eq!(compile(&[a.clone(), b]).map(|(i, e)| (i, e.line)), Some((1, 1)));
        assert!(compile(&[a]).is_none());
    }

    #[test]
    fn macro_extensions() {
        let p = |s: &str| std::path::PathBuf::from(s);
        assert_eq!(macro_ext(apex_io::Kind::Sheet, &p("a.xlsx")), "xlsm");
        assert_eq!(macro_ext(apex_io::Kind::Sheet, &p("a.xltx")), "xltm");
        assert_eq!(macro_ext(apex_io::Kind::Doc, &p("a.docx")), "docm");
    }

    #[test]
    fn basic_libraries() {
        let part = |path: &str, xml: &str| apex_doc::macros::KeptPart { path: path.into(), content_type: "text/xml".into(), rel_type: String::new(), owner: apex_doc::macros::PartOwner::Other, data: std::sync::Arc::new(xml.as_bytes().to_vec()) };
        let m = apex_doc::Macros {
            odf: vec![
                part("Basic/Standard/Module1.xml", "<?xml version=\"1.0\"?><script:module xmlns:script=\"http://openoffice.org/2000/script\" script:name=\"Module1\">Sub Main\n  Print &quot;x&quot;\nEnd Sub\n</script:module>"),
                part("Basic/Standard/script-lb.xml", "<library:library/>"),
            ],
            ..Default::default()
        };
        assert_eq!(basic_modules(&m), [("Standard.Module1".to_string(), "Sub Main\n  Print \"x\"\nEnd Sub\n".to_string())]);
    }
}
