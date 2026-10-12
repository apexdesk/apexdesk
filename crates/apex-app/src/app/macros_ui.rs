//! Macros: the message bar of files that have them, the warning when a
//! macro-free format would drop them, and the macro editor (View > Macros):
//! the modules of the VBA project with their code (colored, with line
//! numbers and find); standard modules can be added, renamed and removed.
//! The code is kept and saved with the file; running it is `macro_run`'s.

use std::collections::HashSet;
use std::path::PathBuf;

use apex_io::Format;
use apex_io::vba::{ModuleKind, NameError, VbaModule, VbaProject};
use eframe::egui::{self, FontId, RichText, text::CCursor, text::CCursorRange};

use super::{App, Notice, View};
use crate::i18n::{tr, trf};
use crate::icons::{self, Icon};

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
    find: String,
    not_found: bool,
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
    /// Of a spreadsheet (macros run only there).
    sheet: bool,
    /// Row to select (a macro's error).
    select_row: Option<usize>,
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
}

/// The macros of a view's workbook or document.
fn macros_of(view: &mut View) -> &mut apex_doc::Macros {
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
        View::Pdf(_) | View::Db(_) | View::Note(_) => {}
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

impl App {
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
            View::Doc(_) | View::Slides(_) => (true, false),
            View::Pdf(_) | View::Db(_) | View::Note(_) => (true, false),
        };
        let allowed = super::macro_run::allowed(&self.settings, &self.tabs, id);
        let text = if doc {
            "macros.bar_doc"
        } else if allowed && vba {
            "macros.bar_ask"
        } else {
            "macros.bar"
        };
        let mut enable = false;
        let dark = ui.visuals().dark_mode;
        let (fill, edge) = if dark { (egui::Color32::from_rgb(0x4A, 0x40, 0x1C), egui::Color32::from_rgb(0x8A, 0x74, 0x2A)) } else { (egui::Color32::from_rgb(0xFF, 0xF4, 0xCE), egui::Color32::from_rgb(0xE8, 0xD0, 0x7A)) };
        let (mut open, mut close) = (false, false);
        egui::Panel::top("macro_bar").frame(egui::Frame::new().fill(fill).stroke(egui::Stroke::new(1.0, edge)).inner_margin(egui::Margin::symmetric(10, 4))).show(ui, |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                icons::paint(ui.painter(), r, Icon::Field, ui.visuals().text_color());
                ui.label(RichText::new(tr("macros.title")).strong());
                ui.label(tr(text));
                if allowed && vba {
                    enable = ui.button(RichText::new(tr("macros.enable")).strong()).clicked();
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
        if close || open || enable {
            self.macros.bars.remove(&id);
        }
    }

    /// View > Macros: the editor for the active tab.
    pub(super) fn open_macros(&mut self) {
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
        let m = macros_of(&mut t.view);
        let source = match &m.project {
            Some(bin) => match apex_io::vba::read(bin) {
                Ok(p) => {
                    let read_only = if p.protected { Some("macros.locked") } else if m.signed { Some("macros.signed") } else { None };
                    Source::Vba { project: Box::new(p), read_only }
                }
                Err(e) => Source::Broken(e.to_string()),
            },
            None => {
                let basic = basic_modules(m);
                if basic.is_empty() { Source::None } else { Source::Basic(basic) }
            }
        };
        // The first module with code, like the VBA editor.
        let current = match &source {
            Source::Vba { project, .. } => project.modules.iter().position(|m| !m.code.trim().is_empty()).unwrap_or(0),
            _ => 0,
        };
        self.macros.editor = Some(MacroEditor { tab: t.id, source, current, find: String::new(), not_found: false, scroll_row: None, rename: None, focus_rename: false, confirm_delete: false, dirty: false, run: None, sheet, select_row: None });
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
        let Some(e) = self.macros.editor.as_mut().filter(|e| e.tab == tab && e.dirty) else { return };
        let Source::Vba { project, .. } = &e.source else { return };
        let bin = project.write();
        e.dirty = false;
        let Some(idx) = self.tabs.iter().position(|t| t.id == tab) else { return };
        self.hold(idx);
        match bin {
            Ok(bin) => {
                let view = &mut self.tabs[idx].view;
                macros_of(view).set_project(bin);
                mark_modified(view, true);
            }
            Err(err) => self.notify(Notice::error(trf("macros.bad_project", &[("error", &err)]))),
        }
    }

    /// Whether tab `idx` can be saved at `path` as is; otherwise asks
    /// about the macros a macro-free format would drop.
    pub(super) fn macros_fit(&mut self, idx: usize, path: &std::path::Path, fmt: Format) -> bool {
        let Some(t) = self.tabs.get_mut(idx) else { return true };
        let has_project = macros_of(&mut t.view).project.is_some();
        if !has_project || !matches!(fmt, Format::Xlsx | Format::Docx) || apex_io::Flavor::from_path(path).macros {
            return true;
        }
        self.macros.prompt = Some(SavePrompt { tab: t.id, path: path.to_path_buf(), fmt });
        false
    }

    /// The "save without macros?" question.
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
        egui::Window::new(title)
            .id(egui::Id::new("macro_editor"))
            .open(&mut open)
            .default_size([820.0, 520.0])
            .min_size([480.0, 260.0])
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| changed = e.ui(ui));
        if changed {
            e.dirty = true;
            self.hold(idx);
            mark_modified(&mut self.tabs[idx].view, false);
        }
        let tab = e.tab;
        let run = e.run.take();
        self.macros.editor = Some(e);
        match run {
            Some(Some(name)) => {
                self.flush_macros(tab);
                self.run_macro_named(tab, name);
            }
            Some(None) => {
                self.active = idx;
                self.open_run_macro();
            }
            None => {}
        }
        if !open {
            self.flush_macros(tab);
            self.macros.editor = None;
        }
    }
}

/// The row of the caret in the code editor `id`.
fn cursor_row(ctx: &egui::Context, id: egui::Id, code: &str) -> usize {
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
            state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(start), CCursor::new(start + len))));
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

/// Selects the next match of `find` in `code` (case ignored, from the
/// cursor, wrapping around); returns its row.
fn find_next(ctx: &egui::Context, id: egui::Id, code: &str, find: &str) -> Option<usize> {
    let low = |s: &str| -> Vec<char> { s.chars().map(|c| c.to_lowercase().next().unwrap_or(c)).collect() };
    let (hay, needle) = (low(code), low(find));
    let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    let from: usize = state.cursor.char_range().map_or(0, |r| r.primary.index.max(r.secondary.index).into());
    let at = |i: usize| hay.get(i..i + needle.len()) == Some(&needle[..]);
    let i = (from..hay.len()).chain(0..from.min(hay.len())).find(|&i| at(i))?;
    state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(i), CCursor::new(i + needle.len()))));
    state.store(ctx, id);
    ctx.memory_mut(|m| m.request_focus(id));
    Some(hay[..i].iter().filter(|&&c| c == '\n').count())
}

impl MacroEditor {
    /// The window's content; returns whether the code changed.
    fn ui(&mut self, ui: &mut egui::Ui) -> bool {
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
                    code_area(ui, egui::Id::new(("basic_code", self.current)), &mut code, &mut self.scroll_row);
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
        match read_only {
            Some(key) => {
                ui.label(RichText::new(tr(key)).color(ui.visuals().warn_fg_color));
            }
            None => {
                ui.label(RichText::new(tr("macros.not_run")).weak());
            }
        }
        // Toolbar: modules, then find.
        let editable = read_only.is_none();
        let standard = project.modules.get(self.current).is_some_and(|m| m.kind == ModuleKind::Standard);
        let code_id = egui::Id::new(("vba_code", self.current, project.modules.get(self.current).map(|m| m.name.clone()).unwrap_or_default()));
        let runnable = self.sheet && project.modules.get(self.current).is_some_and(|m| matches!(m.kind, ModuleKind::Standard | ModuleKind::Document));
        ui.horizontal(|ui| {
            if ui.add_enabled(runnable, egui::Button::new(format!("▶ {}", tr("macros.run")))).on_hover_text(tr("macros.run_tip")).clicked()
                && let Some(m) = project.modules.get(self.current)
            {
                let row = cursor_row(ui.ctx(), code_id, &m.code);
                self.run = Some(macro_at(m, row));
            }
            ui.separator();
            ui.add_enabled_ui(editable, |ui| {
                if ui.button(format!("+ {}", tr("macros.add"))).clicked() {
                    let name = project.next_name("Module");
                    project.modules.push(VbaModule::standard(&name));
                    self.current = project.modules.len() - 1;
                    self.rename = None;
                    self.confirm_delete = false;
                    changed = true;
                }
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
            let r = ui.add(egui::TextEdit::singleline(&mut self.find).hint_text(tr("macros.find")).desired_width(180.0));
            if r.changed() {
                self.not_found = false;
            }
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button(tr("macros.find_next")).clicked() || enter) && !self.find.is_empty() {
                let code = project.modules.get(self.current).map(|m| m.code.as_str()).unwrap_or("");
                self.scroll_row = find_next(ui.ctx(), code_id, code, &self.find);
                self.not_found = self.scroll_row.is_none();
                if enter && self.not_found {
                    r.request_focus();
                }
            }
            if self.not_found {
                ui.label(RichText::new(tr("macros.not_found")).color(ui.visuals().warn_fg_color));
            }
        });
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
        ui.separator();
        let mut pick = None;
        let entries: Vec<(String, &'static str)> = project.modules.iter().map(|m| (m.name.clone(), tr(kind_key(m.kind)))).collect();
        ui.horizontal_top(|ui| {
            module_list(ui, entries.iter().map(|(n, k)| (n.as_str(), *k)), self.current, &mut pick);
            ui.separator();
            if let Some(m) = project.modules.get_mut(self.current) {
                if let Some(row) = self.select_row.take() {
                    select_row(ui.ctx(), code_id, &m.code, row);
                }
                if editable {
                    changed |= code_area(ui, code_id, &mut m.code, &mut self.scroll_row);
                } else {
                    code_area(ui, code_id, &mut m.code.as_str(), &mut self.scroll_row);
                }
            }
        });
        if let Some(i) = pick {
            self.current = i;
            self.rename = None;
            self.confirm_delete = false;
            self.not_found = false;
        }
        changed
    }
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

/// The code with line numbers and colors; returns whether it changed.
fn code_area(ui: &mut egui::Ui, id: egui::Id, code: &mut dyn egui::TextBuffer, scroll_row: &mut Option<usize>) -> bool {
    let font = FontId::monospace(13.0);
    let dark = ui.visuals().dark_mode;
    let fg = ui.visuals().text_color();
    let row_h = ui.fonts_mut(|f| f.row_height(&font));
    let rows = code.as_str().split('\n').count().max(1);
    let digits = rows.to_string().len().max(2);
    let numbers: String = (1..=rows).map(|n| format!("{n:>digits$}\n")).collect();
    let mut changed = false;
    egui::ScrollArea::both().id_salt(id.with("scroll")).auto_shrink([false, false]).show(ui, |ui| {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.vertical(|ui| {
                ui.add_space(2.0);
                ui.label(RichText::new(numbers.trim_end_matches('\n')).font(font.clone()).color(ui.visuals().weak_text_color()));
            });
            let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, _wrap: f32| ui.fonts_mut(|f| f.layout_job(super::vba_code::job(text.as_str(), font.clone(), dark, fg)));
            let r = ui.add(
                egui::TextEdit::multiline(code)
                    .id(id)
                    .font(font.clone())
                    .code_editor()
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::symmetric(4, 2))
                    .desired_width(f32::INFINITY)
                    .desired_rows(24)
                    .layouter(&mut layouter),
            );
            changed = r.changed();
            if let Some(row) = scroll_row.take() {
                let y = r.rect.min.y + 2.0 + row as f32 * row_h;
                ui.scroll_to_rect(egui::Rect::from_min_size(egui::pos2(r.rect.min.x, y), egui::vec2(8.0, row_h)), Some(egui::Align::Center));
            }
        });
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

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
