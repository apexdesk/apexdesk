//! ApexDesk in the terminal (`apexdesk --tui`): spreadsheets and documents
//! drawn with ratatui. The terminal only limits what can be done: files
//! are opened and saved with apex-io like in the window, so everything
//! they hold is kept.
//!
//! [`App`] holds one file and reacts to terminal events; [`run`] drives it
//! on the real terminal, tests drive it on ratatui's `TestBackend`.

mod cmd;
mod doc;
mod doc_layout;
mod menubar;
mod sheet;
mod sheet_draw;
mod term;
mod ui;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use apex_doc::Document;
use apex_i18n::{tr, trf};
use apex_io::{Format, Kind, Loaded};
use apex_sheet::Workbook;
use ratatui::Frame;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

pub use cmd::Cmd;
use doc::DocUi;
use sheet::SheetUi;
use ui::{Choice, LineEdit, Overlay, Prompt};

/// What the terminal interface asks of the program that starts it.
#[derive(Default, Clone, Copy)]
pub struct Hooks {
    /// Writes a PDF of the file: pages are laid out with the window
    /// program's fonts, so it provides this.
    pub pdf: Option<fn(Book<'_>, &Path) -> Result<(), String>>,
    /// The menu bar starts hidden (the user turned it off).
    pub hide_menu_bar: bool,
    /// Remembers whether the menu bar shows.
    pub save_menu_bar: Option<fn(bool)>,
}

/// The open file, for [`Hooks`].
#[derive(Clone, Copy)]
pub enum Book<'a> {
    Sheet(&'a Workbook),
    Doc(&'a Document),
}

pub(crate) enum View {
    Sheet(Box<SheetUi>),
    Doc(Box<DocUi>),
}

/// One open file and the state of the interface.
pub struct App {
    pub(crate) path: Option<PathBuf>,
    pub(crate) format: Option<Format>,
    pub(crate) view: View,
    pub(crate) overlay: Option<Overlay>,
    /// Message of the status bar, and whether it is an error.
    pub(crate) message: Option<(String, bool)>,
    pub(crate) quit: bool,
    pub(crate) hooks: Hooks,
    pub(crate) colors: term::Colors,
    /// Text to put on the system clipboard (OSC 52) after the next draw.
    pub(crate) clipboard_out: Option<String>,
    /// Size of the last frame.
    pub(crate) area: Rect,
    /// The menu bar shows on the top row.
    pub(crate) menu_bar: bool,
}

impl App {
    /// Opens `path` (a missing file starts a new one of its kind; no path
    /// starts a new workbook).
    pub fn open(path: Option<&Path>, hooks: Hooks) -> Result<App, String> {
        let (view, format, path) = match path {
            Some(p) if p.exists() => {
                let (loaded, fmt) = apex_io::open(p).map_err(|e| trf("tui.open_failed", &[("file", &p.display()), ("error", &e)]))?;
                let view = match loaded {
                    Loaded::Sheet(wb) => View::Sheet(Box::new(SheetUi::new(*wb))),
                    Loaded::Doc(doc) => View::Doc(Box::new(DocUi::new(*doc))),
                    Loaded::Slides(_) => return Err(trf("tui.no_slides", &[("file", &p.display())])),
                };
                // Templates open as new files, like in the window.
                if apex_io::is_template(p) { (view, None, None) } else { (view, Some(fmt), Some(p.to_path_buf())) }
            }
            Some(p) => {
                let fmt = Format::from_path(p).filter(|f| f.can_save()).ok_or_else(|| trf("tui.unknown_format", &[("file", &p.display())]))?;
                let view = match fmt.kind() {
                    Kind::Sheet => View::Sheet(Box::new(SheetUi::new(Workbook::new()))),
                    Kind::Doc => View::Doc(Box::new(DocUi::new(Document::new()))),
                    Kind::Slides => return Err(trf("tui.no_slides", &[("file", &p.display())])),
                };
                (view, Some(fmt), Some(p.to_path_buf()))
            }
            None => (View::Sheet(Box::new(SheetUi::new(Workbook::new()))), None, None),
        };
        Ok(App { path, format, view, overlay: None, message: None, quit: false, hooks, colors: term::Colors::detect(), clipboard_out: None, area: Rect::new(0, 0, 80, 24), menu_bar: !hooks.hide_menu_bar })
    }

    pub fn from_workbook(wb: Workbook) -> App {
        App::with_view(View::Sheet(Box::new(SheetUi::new(wb))))
    }

    pub fn from_document(doc: Document) -> App {
        App::with_view(View::Doc(Box::new(DocUi::new(doc))))
    }

    fn with_view(view: View) -> App {
        App { path: None, format: None, view, overlay: None, message: None, quit: false, hooks: Hooks::default(), colors: term::Colors::Rgb, clipboard_out: None, area: Rect::new(0, 0, 80, 24), menu_bar: true }
    }

    pub fn modified(&self) -> bool {
        match &self.view {
            View::Sheet(s) => s.wb.modified,
            View::Doc(d) => d.doc.modified,
        }
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    pub(crate) fn file_name(&self) -> String {
        match &self.path {
            Some(p) => p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            None => tr("tui.untitled").to_string(),
        }
    }

    pub(crate) fn info(&mut self, s: impl Into<String>) {
        self.message = Some((s.into(), false));
    }

    pub(crate) fn error(&mut self, s: impl Into<String>) {
        self.message = Some((s.into(), true));
    }

    // --- drawing -------------------------------------------------------------------------

    /// The menu bar has a row of its own in sheets (the formula bar needs
    /// the width) and shares the title row in documents.
    pub fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        self.area = area;
        let colors = self.colors;
        let name = self.file_name();
        let modified = self.modified();
        let message = self.message.clone();
        let overlay_open = self.overlay.is_some();
        let kind = self.kind();
        let bar_w = menubar::bar_width(kind);
        match &mut self.view {
            View::Sheet(s) => {
                let r = if self.menu_bar { Rect { y: area.y + 1, height: area.height.saturating_sub(1), ..area } } else { area };
                s.draw(f, r, colors, &name, modified, message.as_ref(), overlay_open);
            }
            View::Doc(d) => d.draw(f, if self.menu_bar { bar_w } else { 0 }, colors, &name, modified, message.as_ref(), overlay_open),
        }
        let open = match &self.overlay {
            Some(Overlay::Bar(b)) => Some(b.menu),
            _ => None,
        };
        if self.menu_bar || open.is_some() {
            let w = if kind == Kind::Doc { bar_w.min(area.width) } else { area.width };
            menubar::draw_bar(f.buffer_mut(), Rect::new(area.x, area.y, w, 1), kind, open);
        }
        if let Some(o) = &self.overlay {
            o.draw(f);
        }
    }

    // --- events --------------------------------------------------------------------------

    pub fn handle(&mut self, ev: Event) {
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => self.key(k),
            Event::Mouse(m) if self.overlay.is_none() => self.mouse(m),
            Event::Mouse(m) => {
                if let Some(Overlay::Bar(b)) = &mut self.overlay {
                    match b.mouse(&m, self.area) {
                        ui::Outcome::Keep => {}
                        ui::Outcome::Run(cmd) => {
                            self.overlay = None;
                            self.run(cmd);
                        }
                        _ => self.overlay = None,
                    }
                }
            }
            Event::Paste(text) => self.paste_text(&text),
            _ => {}
        }
    }

    fn key(&mut self, k: KeyEvent) {
        if self.overlay.is_some() {
            self.overlay_key(k);
            return;
        }
        // A key clears the last message.
        self.message = None;
        let editing = matches!(&self.view, View::Sheet(s) if s.edit.is_some());
        // Alt+the underlined letter of a menu, unless Alt+it is a shortcut.
        if let KeyCode::Char(ch) = k.code
            && k.modifiers.contains(KeyModifiers::ALT)
            && !k.modifiers.contains(KeyModifiers::CONTROL)
            && !cmd::alt_taken(self.kind()).contains(ch.to_ascii_lowercase())
            && let Some(i) = menubar::by_mnemonic(&menubar::titles(self.kind()), ch)
        {
            self.open_bar(i);
            return;
        }
        if let Some(cmd) = cmd::global_key(&k, self.kind(), editing) {
            self.run(cmd);
            return;
        }
        let out = match &mut self.view {
            View::Sheet(s) => s.key(k),
            View::Doc(d) => d.key(k),
        };
        if let Some(cmd) = out {
            self.run(cmd);
        }
        self.take_view_messages();
    }

    fn mouse(&mut self, m: MouseEvent) {
        // A click on a title of the menu bar opens its menu.
        if self.menu_bar
            && m.kind == MouseEventKind::Down(MouseButton::Left)
            && m.row == self.area.y
            && let Some(i) = menubar::title_at(self.kind(), self.area.x, m.column)
        {
            self.open_bar(i);
            return;
        }
        let out = match &mut self.view {
            View::Sheet(s) => s.mouse(m),
            View::Doc(d) => d.mouse(m),
        };
        if let Some(cmd) = out {
            self.run(cmd);
        }
        self.take_view_messages();
    }

    fn paste_text(&mut self, text: &str) {
        if let Some(o) = &mut self.overlay {
            o.paste(text);
            return;
        }
        match &mut self.view {
            View::Sheet(s) => s.paste_text(text),
            View::Doc(d) => d.paste_text(text),
        }
        self.take_view_messages();
    }

    fn take_view_messages(&mut self) {
        let (msg, clip) = match &mut self.view {
            View::Sheet(s) => (s.message.take(), s.clipboard_out.take()),
            View::Doc(d) => (d.message.take(), d.clipboard_out.take()),
        };
        if let Some(m) = msg {
            self.message = Some(m);
        }
        if let Some(c) = clip {
            self.clipboard_out = Some(c);
        }
    }

    pub(crate) fn kind(&self) -> Kind {
        match self.view {
            View::Sheet(_) => Kind::Sheet,
            View::Doc(_) => Kind::Doc,
        }
    }

    /// Pulls down a menu of the bar.
    pub(crate) fn open_bar(&mut self, menu: usize) {
        let open = menubar::Open::new(self.kind(), menu, &|c| self.cmd_state(c));
        self.overlay = Some(Overlay::Bar(open));
    }

    /// Whether a command applies now, and the on/off state of switches,
    /// for the menu bar.
    pub(crate) fn cmd_state(&self, c: &Cmd) -> (bool, Option<bool>) {
        match (c, &self.view) {
            (Cmd::ExportPdf, _) => (self.hooks.pdf.is_some(), None),
            (Cmd::ToggleMenuBar, _) => (true, Some(self.menu_bar)),
            (Cmd::Undo, View::Sheet(s)) => (s.wb.can_undo(), None),
            (Cmd::Redo, View::Sheet(s)) => (s.wb.can_redo(), None),
            (Cmd::FindNext, View::Sheet(s)) => (s.last_find.is_some(), None),
            (Cmd::DeleteSheet | Cmd::NextSheet | Cmd::PrevSheet, View::Sheet(s)) => (s.wb.sheets.iter().filter(|x| !x.hidden).count() > 1, None),
            (Cmd::FilterMenu, View::Sheet(s)) => (s.sh().filter.is_some(), None),
            (Cmd::AutoFilter, View::Sheet(s)) => (true, Some(s.sh().filter.is_some())),
            (Cmd::ShowFormulas, View::Sheet(s)) => (true, Some(s.show_formulas)),
            (Cmd::Freeze, View::Sheet(s)) => (true, Some(s.sh().frozen_rows > 0 || s.sh().frozen_cols > 0)),
            (Cmd::Undo, View::Doc(d)) => (d.doc.can_undo(), None),
            (Cmd::Redo, View::Doc(d)) => (d.doc.can_redo(), None),
            (Cmd::Copy | Cmd::Cut, View::Doc(d)) => (!d.sel.is_empty(), None),
            (Cmd::FindNext, View::Doc(d)) => (d.last_find.is_some(), None),
            (Cmd::Outline, View::Doc(d)) => (true, Some(d.outline)),
            _ => (true, None),
        }
    }

    fn overlay_key(&mut self, k: KeyEvent) {
        let Some(mut o) = self.overlay.take() else { return };
        match o.key(k) {
            ui::Outcome::Keep => self.overlay = Some(o),
            ui::Outcome::Close => {}
            ui::Outcome::Run(cmd) => self.run(cmd),
            ui::Outcome::Answer(ask, text) => self.answer(ask, text),
            ui::Outcome::Picked(purpose, picked) => {
                if let View::Sheet(s) = &mut self.view {
                    s.filter_picked(purpose, picked);
                }
                self.take_view_messages();
            }
        }
    }

    /// Opens a one-line question.
    pub(crate) fn ask(&mut self, ask: ui::Ask, title: &str, initial: &str) {
        self.overlay = Some(Overlay::Prompt(Prompt { ask, title: title.to_string(), edit: LineEdit::new(initial) }));
    }

    fn answer(&mut self, ask: ui::Ask, text: String) {
        use ui::Ask;
        match ask {
            Ask::SaveAs => {
                if !text.trim().is_empty() {
                    self.save_as(Path::new(text.trim()));
                }
            }
            Ask::ExportPdf => {
                if !text.trim().is_empty() {
                    self.export_pdf(Path::new(text.trim()));
                }
            }
            Ask::ReplaceFind => {
                if !text.is_empty() {
                    self.ask(Ask::ReplaceWith(text.clone()), &trf("tui.replace_with", &[("text", &text)]), "");
                }
            }
            _ => {
                match &mut self.view {
                    View::Sheet(s) => s.answer(ask, text),
                    View::Doc(d) => d.answer(ask, text),
                }
                self.take_view_messages();
            }
        }
    }

    // --- commands ------------------------------------------------------------------------

    pub fn run(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Quit => {
                if self.modified() {
                    self.overlay = Some(Overlay::ConfirmQuit);
                } else {
                    self.quit = true;
                }
            }
            Cmd::QuitNow => self.quit = true,
            Cmd::SaveAndQuit => {
                if self.save() {
                    self.quit = true;
                }
            }
            Cmd::Save => {
                self.save();
            }
            Cmd::SaveAs => {
                let initial = self.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| self.default_name());
                self.ask(ui::Ask::SaveAs, tr("tui.save_as"), &initial);
            }
            Cmd::ExportPdf => {
                if self.hooks.pdf.is_none() {
                    self.error(tr("tui.no_pdf"));
                    return;
                }
                let base = self.path.clone().unwrap_or_else(|| PathBuf::from(self.default_name()));
                self.ask(ui::Ask::ExportPdf, tr("tui.export_pdf"), &base.with_extension("pdf").display().to_string());
            }
            Cmd::MenuBar => self.open_bar(0),
            Cmd::ToggleMenuBar => {
                self.menu_bar = !self.menu_bar;
                if let Some(save) = self.hooks.save_menu_bar {
                    save(self.menu_bar);
                }
            }
            Cmd::Palette => self.overlay = Some(Overlay::Choice(Choice::palette(self.kind()))),
            Cmd::Help => self.overlay = Some(Overlay::Help(0)),
            Cmd::Menu(m) => self.overlay = Some(Overlay::Choice(Choice::menu(m))),
            Cmd::Ask(ask) => {
                let (title, initial) = match &mut self.view {
                    View::Sheet(s) => s.ask_text(&ask),
                    View::Doc(d) => d.ask_text(&ask),
                };
                self.ask(ask, &title, &initial);
            }
            Cmd::FilterMenu => {
                if let View::Sheet(s) = &mut self.view
                    && let Some(c) = s.filter_menu()
                {
                    self.overlay = Some(Overlay::Choice(c));
                }
                self.take_view_messages();
            }
            other => {
                match &mut self.view {
                    View::Sheet(s) => s.run(other),
                    View::Doc(d) => d.run(other),
                }
                self.take_view_messages();
            }
        }
    }

    fn default_name(&self) -> String {
        let ext = match self.kind() {
            Kind::Sheet => "xlsx",
            Kind::Doc | Kind::Slides => "docx",
        };
        format!("{}.{ext}", tr("tui.untitled"))
    }

    /// Saves to the file's path and format; asks for a name when there is
    /// none or the format is read only.
    pub fn save(&mut self) -> bool {
        match (self.path.clone(), self.format) {
            (Some(p), Some(f)) if f.can_save() => self.write(&p, f),
            _ => {
                self.run(Cmd::SaveAs);
                false
            }
        }
    }

    fn save_as(&mut self, path: &Path) {
        let mut path = path.to_path_buf();
        if path.extension().is_none() {
            path.set_extension(match self.kind() {
                Kind::Sheet => "xlsx",
                Kind::Doc | Kind::Slides => "docx",
            });
        }
        let Some(fmt) = Format::from_path(&path).filter(|f| f.can_save() && f.kind() == self.kind()) else {
            self.error(trf("tui.cannot_save_as", &[("ext", &path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default())]));
            return;
        };
        if self.write(&path, fmt) {
            self.path = Some(path);
            self.format = Some(fmt);
        }
    }

    fn write(&mut self, path: &Path, fmt: Format) -> bool {
        let res = match &mut self.view {
            View::Sheet(s) => {
                s.commit_edit();
                apex_io::save_sheet(&s.wb, path, fmt).map(|_| s.wb.modified = false)
            }
            View::Doc(d) => apex_io::save_doc(&d.doc, path, fmt).map(|_| d.doc.modified = false),
        };
        match res {
            Ok(()) => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if fmt.is_rich() {
                    self.info(trf("tui.saved", &[("file", &name)]));
                } else {
                    self.info(trf("tui.saved_lossy", &[("file", &name), ("format", &fmt.extension().to_uppercase())]));
                }
                true
            }
            Err(e) => {
                self.error(trf("tui.save_failed", &[("error", &e)]));
                false
            }
        }
    }

    fn export_pdf(&mut self, path: &Path) {
        let Some(pdf) = self.hooks.pdf else { return };
        let res = match &mut self.view {
            View::Sheet(s) => {
                s.commit_edit();
                pdf(Book::Sheet(&s.wb), path)
            }
            View::Doc(d) => pdf(Book::Doc(&d.doc), path),
        };
        match res {
            Ok(()) => self.info(trf("tui.exported", &[("file", &path.display())])),
            Err(e) => self.error(trf("tui.save_failed", &[("error", &e)])),
        }
    }

    /// Text for the system clipboard, taken by the terminal loop.
    pub fn take_clipboard(&mut self) -> Option<String> {
        self.clipboard_out.take()
    }
}

/// The document as plain text laid out like the reading view: headings
/// underlined, list markers, box-drawn tables, notes at the end and
/// `[image: ...]` marks, wrapped at `width` columns.
pub fn doc_text(doc: &Document, width: u16) -> String {
    let layout = doc_layout::build(doc, width.max(20), term::Colors::Rgb);
    let mut out = String::new();
    for line in &layout.lines {
        let mut s = String::new();
        let mut col = 0u16;
        for seg in &line.segs {
            while col < seg.x {
                s.push(' ');
                col += 1;
            }
            s.push_str(&seg.text);
            col += seg.width();
        }
        out.push_str(s.trim_end());
        out.push('\n');
    }
    // No trailing blank lines.
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

/// Runs the interface on the terminal until the user quits.
pub fn run(path: Option<&Path>, hooks: Hooks) -> Result<(), String> {
    let mut app = App::open(path, hooks)?;
    term::run(&mut app).map_err(|e| e.to_string())
}
