//! The code view: the document as text, edited with its own undo. Text
//! files (txt, md, source code) show their file as it is; other documents
//! show the Markdown they would be saved as. Going back to the formatted
//! view reads the text again with the reader of its format, and only when
//! it changed, as one undo step of the document; what wasn't edited keeps
//! the formatting the text can't show (see `apex_doc::merge_code`). Saving to the file's own
//! format from here writes the text exactly as it is. Source code files
//! only show as code.

use std::path::Path;

use eframe::egui::text::CCursorRange;
use eframe::egui::text_edit::TextEditState;

use super::code_gutter;
use super::code_syntax::{self, Lang};
use super::md_syntax;
use super::*;
use crate::toolstrip::{self, Body};

/// Undo steps kept in the source.
const MAX_UNDO: usize = 500;

/// What the code view shows, and reads back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Source {
    /// The document as Markdown (Markdown files, other documents, new ones).
    #[default]
    Markdown,
    /// A plain text file.
    Plain,
    /// A source code file: only shown as code.
    Code(Lang),
}

impl Source {
    /// What a file at `path` shows as code.
    pub(super) fn of(path: Option<&Path>) -> Source {
        let ext = path.and_then(|p| p.extension()).and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
        match ext.as_str() {
            "txt" | "text" | "log" => Source::Plain,
            e if apex_io::CODE_EXTS.contains(&e) => Source::Code(Lang::of(e)),
            _ => Source::Markdown,
        }
    }

    /// The format that saves the text as it is.
    fn format(self) -> apex_io::Format {
        match self {
            Source::Markdown => apex_io::Format::Markdown,
            _ => apex_io::Format::Text,
        }
    }

    /// The document as this text.
    fn write(self, doc: &Document) -> String {
        match self {
            Source::Markdown => apex_io::markdown::write(doc),
            _ => doc.to_plain_text(),
        }
    }

    /// The text read back as a document.
    fn read(self, text: &str) -> Document {
        match self {
            Source::Markdown => apex_io::markdown::read(text),
            _ => Document::from_plain_text(text),
        }
    }
}

/// The code view's settings, and the code while it shows.
pub(super) struct MdState {
    pub code: Option<Box<MdCode>>,
    /// Line numbers beside the code.
    pub lines: bool,
    /// Long lines wrap at the edge (else they run on, scrolled sideways).
    pub wrap: bool,
    /// The warning that the code loses things was given.
    warned: bool,
    pub(super) source: Source,
    /// The text of the file with the document revision it is (line breaks
    /// as LF): shown while the document is still that.
    raw: Option<(String, u64)>,
    /// The file's lines end in CRLF: saved text gets them back.
    crlf: bool,
}

impl Default for MdState {
    fn default() -> Self {
        MdState { code: None, lines: true, wrap: false, warned: false, source: Source::Markdown, raw: None, crlf: false }
    }
}

/// A text to go back to, and the caret in it.
struct Step {
    text: String,
    cursor: usize,
}

/// Find and replace in the source.
#[derive(Default)]
struct CodeFind {
    query: String,
    with: String,
    replace: bool,
    case: bool,
    focus: bool,
    not_found: bool,
}

/// The Markdown source of the document being edited.
pub(super) struct MdCode {
    pub text: String,
    /// The text as the document was written (it is read back only when
    /// the text is no longer this).
    base: String,
    /// The text as last saved from here, or else `base`: what "modified"
    /// compares with.
    saved: String,
    /// The text was saved to a file from here.
    wrote: bool,
    /// The document's revision the text follows.
    rev: u64,
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// Where the last character typed went (a char index), to make one
    /// undo step of a run of typing.
    typing_at: Option<usize>,
    /// A selection (char indices) for the editor to take next frame.
    select: Option<(usize, usize)>,
    /// A line to scroll to next frame.
    scroll_row: Option<usize>,
    find: Option<CodeFind>,
    focus: bool,
}

impl MdCode {
    fn new(text: String, rev: u64) -> Self {
        MdCode { base: text.clone(), saved: text.clone(), text, wrote: false, rev, undo: Vec::new(), redo: Vec::new(), typing_at: None, select: None, scroll_row: None, find: None, focus: true }
    }

    /// The text differs from what was last saved.
    pub(super) fn dirty(&self) -> bool {
        self.text != self.saved
    }

    /// The text is no longer what the document was written as.
    fn changed(&self) -> bool {
        self.text != self.base
    }

    /// Notes that the text changed from `old` (the caret at `cursor`) as
    /// one undo step; characters typed one after another make one step.
    fn record(&mut self, old: String, cursor: usize) {
        let typed = typed_at(&old, &self.text);
        let merge = typed.is_some() && typed == self.typing_at && !self.undo.is_empty();
        self.typing_at = typed.map(|p| p + 1);
        if !merge {
            self.undo.push(Step { text: old, cursor });
            if self.undo.len() > MAX_UNDO {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
    }

    /// Puts `text` in place of the source as one undo step, selecting `sel`.
    fn set_text(&mut self, text: String, cursor: usize, sel: (usize, usize)) {
        let old = std::mem::replace(&mut self.text, text);
        self.typing_at = None;
        self.undo.push(Step { text: old, cursor });
        self.redo.clear();
        self.select = Some(sel);
    }

    fn undo(&mut self, cursor: usize) -> bool {
        let Some(s) = self.undo.pop() else { return false };
        self.redo.push(Step { text: std::mem::replace(&mut self.text, s.text), cursor });
        self.select = Some((s.cursor, s.cursor));
        self.typing_at = None;
        true
    }

    fn redo(&mut self, cursor: usize) -> bool {
        let Some(s) = self.redo.pop() else { return false };
        self.undo.push(Step { text: std::mem::replace(&mut self.text, s.text), cursor });
        self.select = Some((s.cursor, s.cursor));
        self.typing_at = None;
        true
    }

    /// Selects the next match of the search after char `from` (going
    /// round to the start); false if there is none.
    fn find_next(&mut self, from: usize) -> bool {
        let Some(f) = &mut self.find else { return false };
        let hit = find_in(&self.text, &f.query, from, f.case).or_else(|| find_in(&self.text, &f.query, 0, f.case));
        f.not_found = hit.is_none() && !f.query.is_empty();
        if let Some(h) = hit {
            self.select = Some(h);
        }
        hit.is_some()
    }

    /// Replaces the selected match (`sel`) and goes to the next one.
    fn replace_one(&mut self, sel: (usize, usize)) {
        let Some(f) = &self.find else { return };
        let (a, b) = (sel.0.min(sel.1), sel.0.max(sel.1));
        if find_in(&self.text, &f.query, a, f.case) == Some((a, b)) && a < b {
            let with = f.with.clone();
            let mut t = self.text.clone();
            t.replace_range(byte_at(&t, a)..byte_at(&t, b), &with);
            let end = a + with.chars().count();
            self.set_text(t, sel.1, (end, end));
            self.find_next(end);
        } else {
            self.find_next(b);
        }
    }

    /// Replaces every match as one undo step; returns how many.
    fn replace_all(&mut self, cursor: usize) -> usize {
        let Some(f) = &self.find else { return 0 };
        let (query, with, case) = (f.query.clone(), f.with.clone(), f.case);
        let mut t = self.text.clone();
        let mut n = 0;
        let mut from = 0;
        while let Some((a, b)) = find_in(&t, &query, from, case) {
            t.replace_range(byte_at(&t, a)..byte_at(&t, b), &with);
            from = a + with.chars().count();
            n += 1;
        }
        if n > 0 {
            self.set_text(t, cursor, (cursor, cursor));
        }
        n
    }
}

/// Where one character (not a line break) was typed into `old` to make
/// `new`: its char index.
fn typed_at(old: &str, new: &str) -> Option<usize> {
    if new.len() <= old.len() {
        return None;
    }
    let pre = old.chars().zip(new.chars()).take_while(|(a, b)| a == b).count();
    let old_rest: String = old.chars().skip(pre).collect();
    let mut new_rest = new.chars().skip(pre);
    let c = new_rest.next()?;
    (c != '\n' && new_rest.eq(old_rest.chars())).then_some(pre)
}

/// Byte offset of char index `i` of `s`.
fn byte_at(s: &str, i: usize) -> usize {
    s.char_indices().nth(i).map_or(s.len(), |(b, _)| b)
}

/// The first match of `query` in `text` at char `from` or after, as a
/// char range.
fn find_in(text: &str, query: &str, from: usize, case: bool) -> Option<(usize, usize)> {
    if query.is_empty() {
        return None;
    }
    let fold = |c: char| if case { c } else { c.to_lowercase().next().unwrap_or(c) };
    let t: Vec<char> = text.chars().map(fold).collect();
    let q: Vec<char> = query.chars().map(fold).collect();
    (from..(t.len() + 1).saturating_sub(q.len())).find(|&i| t[i..i + q.len()] == q[..]).map(|i| (i, i + q.len()))
}

/// Whether the text `code` written from `doc` as `src` leaves something
/// out (formatting, headers, comments...): read back, it isn't the same.
pub(super) fn md_loses(src: Source, doc: &Document, code: &str) -> bool {
    let back = src.read(code);
    back.paragraphs != doc.paragraphs || !doc.comments.is_empty() || doc.headers.parts.iter().any(Option::is_some)
}

impl DocView {
    /// Whether the code view shows (instead of the pages).
    pub fn md_source(&self) -> bool {
        self.md.code.is_some()
    }

    /// Whether the document only shows as code (a source code file).
    pub fn code_only(&self) -> bool {
        matches!(self.md.source, Source::Code(_))
    }

    /// Switches between the formatted document and its code.
    pub fn toggle_md_source(&mut self) {
        if self.md.code.is_some() {
            self.leave_md_source();
        } else {
            self.enter_md_source();
        }
    }

    /// Tells the view the file it shows (`path`) and, for a text file, its
    /// text as read (`text`): what the code view shows. A source code file
    /// goes to the code view at once.
    pub fn set_source(&mut self, path: Option<&Path>, text: Option<String>) {
        self.md.source = Source::of(path);
        self.md.crlf = false;
        self.md.raw = text.map(|t| {
            // CRLF throughout is edited as LF and saved back as CRLF.
            let n = t.matches('\n').count();
            self.md.crlf = n > 0 && t.matches("\r\n").count() == n;
            (if self.md.crlf { t.replace("\r\n", "\n") } else { t }, self.doc.revision)
        });
        if self.code_only() {
            self.enter_md_source();
        }
    }

    /// The document was saved as `path`: the code view follows its format
    /// (a new document saved as .txt shows its text, not Markdown).
    pub fn source_saved_as(&mut self, path: &Path) {
        let src = Source::of(Some(path));
        let old = self.md.source;
        if src == old {
            return;
        }
        self.md.source = src;
        if src.format() != old.format() {
            self.md.raw = None;
            self.md.crlf = false;
            if self.md.code.is_some() {
                // The document has the code (saving took it): shown anew.
                self.md_apply();
                let text = src.write(&self.doc);
                self.md.code = Some(Box::new(MdCode::new(text, self.doc.revision)));
            }
        }
        if self.code_only() {
            self.enter_md_source();
        }
    }

    /// Shows the code: the file's text while the document is still what
    /// it has, else the document written as its format.
    pub(super) fn enter_md_source(&mut self) {
        if self.md.code.is_some() {
            return;
        }
        self.end_hf_edit();
        if self.immersive() {
            self.set_view(ViewMode::Print);
        }
        let src = self.md.source;
        let text = match self.md.raw.take() {
            Some((t, rev)) if rev == self.doc.revision => t,
            _ => {
                let text = src.write(&self.doc);
                if !self.md.warned && md_loses(src, &self.doc, &text) {
                    self.md.warned = true;
                    self.notices.push(Notice::info(tr("code.lossy")));
                }
                text
            }
        };
        self.md.code = Some(Box::new(MdCode::new(text, self.doc.revision)));
    }

    /// Back to the formatted document, reading the code again if it
    /// changed (a source code file stays code).
    pub(super) fn leave_md_source(&mut self) {
        if self.code_only() {
            return;
        }
        self.md_apply();
        let Some(c) = self.md.code.take() else { return };
        if c.wrote {
            // Saved from the code: the document is what the file has
            // unless the code changed since.
            self.doc.modified = c.dirty();
        }
        // Showing the code again, while the document stays this, shows
        // this same text.
        self.md.raw = Some((c.base, c.rev));
        self.focus = true;
    }

    /// A new window of `other`'s document shows the code as it does.
    pub(super) fn copy_code_view(&mut self, other: &DocView) {
        let m = &other.md;
        (self.md.lines, self.md.wrap, self.md.warned, self.md.source, self.md.crlf) = (m.lines, m.wrap, m.warned, m.source, m.crlf);
        if let Some(c) = &m.code {
            let mut n = MdCode::new(c.text.clone(), c.rev);
            (n.saved, n.wrote) = (c.saved.clone(), c.wrote);
            self.md.code = Some(Box::new(n));
        }
    }

    /// Reads changed code into the document (one undo step).
    pub(super) fn md_apply(&mut self) {
        let src = self.md.source;
        let Some(c) = self.md.code.as_mut() else { return };
        if !c.changed() {
            return;
        }
        // Only what was edited takes the code's look: the rest stays as
        // the document has it.
        let paras = apex_doc::merge_code(&self.doc.paragraphs, &src.read(&c.base).paragraphs, src.read(&c.text).paragraphs);
        self.doc.replace_body_keeping(paras);
        c.base = c.text.clone();
        c.rev = self.doc.revision;
        self.set_sel(Selection::caret(Pos::new(0, 0)));
        self.layout.key.0 = u64::MAX;
    }

    /// Whether saving in `fmt` writes the code as it is.
    pub fn saves_as_typed(&self, fmt: apex_io::Format) -> bool {
        self.md.code.is_some() && fmt == self.md.source.format()
    }

    /// The text to save in format `fmt` while the code shows: the code
    /// itself for its own format. For another format the document takes
    /// the code first (and `None`: the document is saved).
    pub fn md_source_for(&mut self, fmt: apex_io::Format) -> Option<String> {
        let c = self.md.code.as_ref()?;
        if fmt == self.md.source.format() {
            return Some(if self.md.crlf { c.text.replace('\n', "\r\n") } else { c.text.clone() });
        }
        self.md_apply();
        None
    }

    /// The document was saved: the code as it is now is what the file has.
    pub fn md_saved(&mut self, fmt: apex_io::Format) {
        let own = self.md.source.format();
        if let Some(c) = self.md.code.as_mut() {
            c.saved = c.text.clone();
            c.wrote |= fmt == own;
        }
    }

    /// The document with changed code read into it, for a backup copy.
    pub(super) fn code_backup(&self) -> Option<Document> {
        let c = self.md.code.as_ref().filter(|c| c.changed())?;
        let mut doc = self.doc.clone();
        let src = self.md.source;
        doc.replace_body_keeping(apex_doc::merge_code(&self.doc.paragraphs, &src.read(&c.base).paragraphs, src.read(&c.text).paragraphs));
        Some(doc)
    }

    /// A number that changes with changed code, for backups (0 while the
    /// code is the document's).
    pub(super) fn code_revision(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let Some(c) = self.md.code.as_ref().filter(|c| c.changed()) else { return 0 };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        c.text.hash(&mut h);
        h.finish()
    }

    /// Undo in the source; when it has nothing to undo and wasn't
    /// changed, the document's undo (the source follows it at once, not
    /// only when it is next drawn: another tab may be showing).
    pub(super) fn md_undo(&mut self, redo: bool) {
        let cursor = self.md_cursor().0;
        let Some(c) = self.md.code.as_mut() else { return };
        let done = if redo { c.redo(cursor) } else { c.undo(cursor) };
        if !done && !c.changed() {
            let _ = if redo { self.doc.redo() } else { self.doc.undo() };
            self.md_follow();
        }
    }

    /// Opens the source's find bar (with replace).
    pub(super) fn md_open_find(&mut self, replace: bool) {
        let (a, b) = self.md_cursor();
        let Some(c) = self.md.code.as_mut() else { return };
        let picked: String = if a != b { c.text.chars().skip(a.min(b)).take(a.abs_diff(b)).collect() } else { String::new() };
        let f = c.find.get_or_insert_with(CodeFind::default);
        f.replace |= replace;
        f.focus = true;
        if !picked.is_empty() && !picked.contains('\n') {
            f.query = picked;
        }
    }

    fn md_id(&self) -> Id {
        Id::new(("md_source", self.uid))
    }

    /// The editor's selection (cursor, anchor) as char indices.
    fn md_cursor(&self) -> (usize, usize) {
        let ctx = self.refs.ctx.as_ref();
        ctx.and_then(|ctx| TextEditState::load(ctx, self.md_id()))
            .and_then(|s| s.cursor.char_range())
            .map_or((0, 0), |r| (r.primary.index.into(), r.secondary.index.into()))
    }

    /// The code follows changes made to the document meanwhile (from the
    /// toolstrip, say) while it has none of its own.
    fn md_follow(&mut self) {
        let rev = self.doc.revision;
        let src = self.md.source;
        let Some(c) = self.md.code.as_mut() else { return };
        if c.rev == rev {
            return;
        }
        c.rev = rev;
        if !c.changed() {
            let text = src.write(&self.doc);
            if c.saved == c.base {
                c.saved = text.clone();
            }
            c.base = text.clone();
            c.text = text;
        }
    }

    fn md_keys(&mut self, ctx: &egui::Context) {
        if ctx.memory(|m| m.top_modal_layer().is_some()) {
            return;
        }
        let cmd = Modifiers::COMMAND;
        let key = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        if key(cmd | Modifiers::SHIFT, Key::Z) || key(cmd, Key::Y) {
            self.md_undo(true);
        } else if key(cmd, Key::Z) {
            self.md_undo(false);
        } else if key(cmd, Key::F) {
            self.md_open_find(false);
        } else if key(cmd, Key::H) {
            self.md_open_find(true);
        } else if key(Modifiers::NONE, Key::F3) {
            let from = self.md_cursor().0.max(self.md_cursor().1);
            if let Some(c) = self.md.code.as_mut() {
                c.find_next(from);
            }
        } else if self.md.code.as_ref().is_some_and(|c| c.find.is_some()) && key(Modifiers::NONE, Key::Escape) {
            if let Some(c) = self.md.code.as_mut() {
                c.find = None;
                c.focus = true;
            }
        }
    }

    /// The source view: the find bar (when open) and the editor.
    pub(super) fn md_source_ui(&mut self, ui: &mut egui::Ui) {
        self.md_follow();
        self.md_keys(ui.ctx());
        let (cursor, anchor) = self.md_cursor();
        let id = self.md_id();
        let editable = !self.read_only && self.doc.edit_limit().is_none();
        let size = 13.0 * self.zoom;
        let lines = self.md.lines;
        let wrap = self.md.wrap;
        let src = self.md.source;
        let uid = self.uid;
        let Some(c) = self.md.code.as_deref_mut() else { return };
        if c.find.is_some() {
            egui::Panel::top(Id::new(("md_find", uid)))
                .frame(egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(8, 4)))
                .show(ui, |ui| code_find_bar(ui, c, (cursor, anchor), editable));
        }
        egui::CentralPanel::default_margins().frame(egui::Frame::new().fill(ui.visuals().extreme_bg_color)).show(ui, |ui| {
            if let Some((a, b)) = c.select.take() {
                let mut st = TextEditState::load(ui.ctx(), id).unwrap_or_default();
                st.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(b), CCursor::new(a))));
                st.store(ui.ctx(), id);
                c.focus = true;
                let row = c.text.chars().take(a).filter(|&ch| ch == '\n').count();
                c.scroll_row = Some(row);
            }
            let old = editable.then(|| c.text.clone());
            let changed = source_area(ui, id, c, (size, src), (lines, wrap), editable);
            if let Some(old) = old.filter(|_| changed) {
                c.record(old, cursor);
            }
        });
    }

    /// The View tab's Code group: the code view, its line numbers and wrapping.
    pub(super) fn md_group(&mut self, b: &mut Body) {
        let a = b.accent();
        b.group("code.group", Icon::Field, |ui| {
            let only = self.code_only();
            let r = ui.add_enabled_ui(!only, |ui| toolstrip::large(ui, Icon::Field, tr("code.view"), tr("code.view_tip"), a, self.md_source())).inner;
            if r.on_disabled_hover_text(tr("code.only_code")).clicked() {
                self.toggle_md_source();
            }
            toolstrip::column(ui, |ui| {
                if toolstrip::check(ui, self.md.lines, Icon::Numbering, tr("code.line_numbers")) {
                    self.md.lines = !self.md.lines;
                }
                if toolstrip::check(ui, self.md.wrap, Icon::Wrap, tr("code.wrap")) {
                    self.md.wrap = !self.md.wrap;
                }
            });
        });
    }
}

/// The find (and replace) bar of the source.
fn code_find_bar(ui: &mut egui::Ui, c: &mut MdCode, (cursor, anchor): (usize, usize), editable: bool) {
    let Some(f) = c.find.as_mut() else { return };
    let mut next = false;
    let mut replace = false;
    let mut all = false;
    let mut close = false;
    ui.horizontal(|ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut f.query).hint_text(tr("find.hint")).desired_width(200.0));
        if std::mem::take(&mut f.focus) {
            r.request_focus();
        }
        if r.changed() {
            f.not_found = false;
        }
        next |= r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        next |= ui.button(tr("find.next")).clicked();
        ui.checkbox(&mut f.case, tr("find.case"));
        if editable {
            ui.toggle_value(&mut f.replace, tr("find.show_replace"));
        }
        if f.not_found {
            ui.label(RichText::new(tr("find.not_found")).color(ui.visuals().warn_fg_color));
        }
        close = ui.button("✕").on_hover_text(tr("dialog.close")).clicked();
    });
    if f.replace && editable {
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut f.with).hint_text(tr("find.replace_hint")).desired_width(200.0));
            replace = ui.button(tr("find.replace")).clicked();
            all = ui.button(tr("find.replace_all")).clicked();
        });
    }
    if close {
        c.find = None;
        c.focus = true;
    } else if next {
        c.find_next(cursor.max(anchor));
    } else if replace {
        c.replace_one((anchor, cursor));
    } else if all {
        c.replace_all(cursor);
    }
}

/// The code with its colors, (if `lines`) line numbers and (if `wrap`) long
/// lines wrapped at the edge; returns whether it changed.
fn source_area(ui: &mut egui::Ui, id: Id, c: &mut MdCode, (size, src): (f32, Source), (lines, wrap): (bool, bool), editable: bool) -> bool {
    let font = egui::FontId::monospace(size);
    let dark = ui.visuals().dark_mode;
    let fg = ui.visuals().text_color();
    let mut changed = false;
    // The numbers have a column of their own left of the scrolled code, and
    // are painted where the code's rows came out.
    let full = ui.available_rect_before_wrap();
    let gutter_w = if lines { code_gutter::width(ui, &font, c.text.split('\n').count()) } else { 0.0 };
    let (gutter, rest) = full.split_left_right_at_x(full.min.x + gutter_w);
    let mut shown = None;
    ui.scope_builder(egui::UiBuilder::new().max_rect(rest), |ui| {
        egui::ScrollArea::both().id_salt(id.with("scroll")).auto_shrink([false, false]).show(ui, |ui| {
            let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, width: f32| {
                let mut job = match src {
                    Source::Markdown => md_syntax::job(text.as_str(), font.clone(), dark, fg),
                    Source::Plain => code_syntax::job(text.as_str(), None, font.clone(), dark, fg),
                    Source::Code(lang) => code_syntax::job(text.as_str(), Some(lang), font.clone(), dark, fg),
                };
                job.wrap.max_width = if wrap { width } else { f32::INFINITY };
                ui.fonts_mut(|f| f.layout_job(job))
            };
            let out = if editable {
                source_edit(&mut c.text, id, &font).layouter(&mut layouter).show(ui)
            } else {
                source_edit(&mut c.text.as_str(), id, &font).layouter(&mut layouter).show(ui)
            };
            let r = &out.response.response;
            crate::widgets::name(r, egui::WidgetType::TextEdit, &tr("code.view"), None);
            changed = r.changed();
            if std::mem::take(&mut c.focus) {
                r.request_focus();
            }
            if let Some(row) = c.scroll_row.take() {
                let at = code_gutter::line_rect(&out.galley, row).translate(out.galley_pos.to_vec2());
                ui.scroll_to_rect(Rect::from_min_size(at.min, vec2(8.0, at.height())), Some(egui::Align::Center));
            }
            shown = Some((out.galley, out.galley_pos, out.text_clip_rect));
        });
    });
    if let Some((galley, pos, clip)) = shown.filter(|_| lines) {
        code_gutter::paint(ui, gutter, &galley, pos, clip, &font, ui.visuals().weak_text_color());
    }
    changed
}

/// The source's text field.
fn source_edit<'t>(text: &'t mut dyn egui::TextBuffer, id: Id, font: &egui::FontId) -> egui::TextEdit<'t> {
    egui::TextEdit::multiline(text)
        .id(id)
        .font(font.clone())
        .code_editor()
        .frame(egui::Frame::NONE)
        .margin(egui::Margin::symmetric(6, 4))
        .desired_width(f32::INFINITY)
        .desired_rows(30)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MD: &str = "# Título\n\nUn párrafo con **negrita**, *cursiva* y `código`.\n\n- uno\n- dos\n\n1. primero\n2. segundo\n\n> cita\n\n| A | B |\n|---|---|\n| x | y |\n\n```\nlet x = 1;\n```\n\nUn [enlace](http://example.com).\n";

    fn view(md: &str) -> DocView {
        DocView::new(apex_io::markdown::read(md))
    }

    fn type_in(v: &mut DocView, text: &str) {
        let c = v.md.code.as_mut().unwrap();
        let old = c.text.clone();
        c.text.push_str(text);
        let at = old.chars().count();
        c.record(old, at);
    }

    #[test]
    fn round_trip_untouched_changes_nothing() {
        let mut v = view(MD);
        let before = v.doc.paragraphs.clone();
        let (rev, undo) = (v.doc.revision, v.doc.can_undo());
        v.toggle_md_source();
        assert!(v.md_source());
        assert!(!v.is_modified());
        assert!(v.notices.is_empty(), "a Markdown document loses nothing");
        v.toggle_md_source();
        assert!(!v.md_source());
        assert_eq!(v.doc.paragraphs, before);
        assert_eq!((v.doc.revision, v.doc.can_undo()), (rev, undo));
        assert!(!v.is_modified());
    }

    #[test]
    fn source_edit_shows_back_and_undoes_in_one_step() {
        let mut v = view(MD);
        let before = v.doc.paragraphs.clone();
        v.toggle_md_source();
        type_in(&mut v, "\n## Nuevo\n");
        assert!(v.is_modified(), "an edited source is a change");
        v.toggle_md_source();
        assert!(v.doc.modified);
        let last = v.doc.paragraphs.last().unwrap();
        assert_eq!((last.text.as_str(), last.style.kind), ("Nuevo", ParaKind::Heading(2)));
        // Going back to the formatted view was one step of the document.
        v.undo();
        assert_eq!(v.doc.paragraphs, before);
        v.redo();
        assert_eq!(v.doc.paragraphs.last().unwrap().text, "Nuevo");
    }

    // A command that edits the document shows in the code; undone, the
    // code is back at once, before the view is drawn again.
    #[test]
    fn undoing_a_command_brings_the_code_back() {
        let mut v = view(MD);
        v.toggle_md_source();
        let orig = v.code_text().unwrap().to_string();
        v.insert(&LINE_BREAK.to_string());
        v.md_follow();
        assert_ne!(v.code_text().unwrap(), orig);
        v.undo();
        assert_eq!(v.code_text().unwrap(), orig);
        v.redo();
        assert_ne!(v.code_text().unwrap(), orig);
    }

    #[test]
    fn source_undo_merges_typing() {
        let mut v = view("Hola\n");
        v.toggle_md_source();
        let base = v.md.code.as_ref().unwrap().text.clone();
        for ch in ["a", "b", "c"] {
            type_in(&mut v, ch);
        }
        type_in(&mut v, "\n");
        type_in(&mut v, "d");
        let c = v.md.code.as_mut().unwrap();
        assert_eq!(c.undo.len(), 3, "abc, the line break, d");
        c.undo(0);
        c.undo(0);
        assert_eq!(c.text, format!("{base}abc"));
        c.undo(0);
        assert_eq!(c.text, base);
        assert!(!c.undo(0));
        c.redo(0);
        assert_eq!(c.text, format!("{base}abc"));
        // Undone back to the start, the document stays as it was.
        let paras = v.doc.paragraphs.clone();
        v.md_undo(false);
        v.toggle_md_source();
        assert_eq!(v.doc.paragraphs, paras);
        assert!(!v.doc.can_undo());
    }

    #[test]
    fn saving_the_source_writes_it_as_typed() {
        let dir = apex_fs::temp_dir().join(format!("apex_mdsrc_{}", apex_fs::process_id()));
        apex_fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.md");
        let mut v = view(MD);
        v.toggle_md_source();
        // Not what the writer would make (two spaces, a star bullet).
        let typed = "#  Otro   título\n\n* suelto\n";
        {
            let c = v.md.code.as_mut().unwrap();
            let old = std::mem::replace(&mut c.text, typed.to_string());
            c.record(old, 0);
        }
        let text = v.md_source_for(apex_io::Format::Markdown).unwrap();
        apex_io::save_text(apex_io::TextSource::Raw(&text), &path, apex_io::Format::Markdown, apex_io::Encoding::Utf8).unwrap();
        v.doc.modified = false;
        v.md_saved(apex_io::Format::Markdown);
        assert_eq!(apex_fs::read_to_string(&path).unwrap(), typed);
        assert!(!v.is_modified());
        // Back to the formatted view: the document is what was saved.
        v.toggle_md_source();
        assert!(!v.is_modified());
        assert_eq!(v.doc.paragraphs[0].text, "Otro   título");
        // Saved as another format, the document takes the source first.
        v.toggle_md_source();
        type_in(&mut v, "\nfin\n");
        assert!(v.md_source_for(apex_io::Format::Docx).is_none());
        assert_eq!(v.doc.paragraphs.last().unwrap().text, "fin");
        let _ = apex_fs::remove_dir_all(&dir);
    }

    #[test]
    fn warns_once_when_markdown_loses_things() {
        let mut d = apex_io::markdown::read("Hola\n");
        d.paragraphs[0].runs[0].attrs.color = Some(0xFF0000);
        let mut v = DocView::new(d);
        assert!(md_loses(Source::Markdown, &v.doc, &apex_io::markdown::write(&v.doc)));
        v.toggle_md_source();
        assert_eq!(v.notices.len(), 1);
        v.toggle_md_source();
        v.toggle_md_source();
        assert_eq!(v.notices.len(), 1, "only once");
    }

    #[test]
    fn find_and_replace() {
        let mut c = MdCode::new("uno Dos uno\n".into(), 0);
        c.find = Some(CodeFind { query: "UNO".into(), with: "tres".into(), ..Default::default() });
        assert!(c.find_next(1));
        assert_eq!(c.select, Some((8, 11)));
        c.replace_one((8, 11));
        assert_eq!(c.text, "uno Dos tres\n");
        assert_eq!(c.replace_all(0), 1);
        assert_eq!(c.text, "tres Dos tres\n");
        assert!(c.undo(0));
        assert_eq!(c.text, "uno Dos tres\n");
        c.find.as_mut().unwrap().case = true;
        assert!(!c.find_next(0));
    }

    /// A file opened as the app opens it: the view of what it loads and
    /// the file's text in `enc`.
    fn open_file(path: &Path, enc: apex_io::Encoding) -> DocView {
        let (loaded, fmt) = apex_io::open(path).unwrap();
        let apex_io::Loaded::Doc(doc) = loaded else { panic!("not a document") };
        let mut v = DocView::new(*doc);
        let text = apex_io::is_text_format(fmt).then(|| enc.decode(&apex_fs::read(path).unwrap()));
        v.set_source(Some(path), text);
        v
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = apex_fs::temp_dir().join(format!("apex_code_{name}_{}", apex_fs::process_id()));
        apex_fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn text_file_shows_and_saves_as_it_is() {
        let dir = temp_dir("txt");
        // CRLF lines, spaces and tabs the plain text reader would keep but
        // a writer might not, in Latin-1.
        let raw = "Línea uno  \r\n\tdos\r\n\r\nfin sin salto";
        let (bytes, _) = apex_io::Encoding::Latin1.encode(raw);
        let path = dir.join("a.txt");
        apex_fs::write(&path, &bytes).unwrap();
        let mut v = open_file(&path, apex_io::Encoding::Latin1);
        assert!(!v.md_source() && !v.code_only(), "a .txt opens formatted");
        let before = v.doc.paragraphs.clone();
        // Untouched, going to the code and back changes nothing.
        v.toggle_md_source();
        assert_eq!(v.md.code.as_ref().unwrap().text, raw.replace("\r\n", "\n"));
        assert!(v.notices.is_empty() && !v.is_modified());
        v.toggle_md_source();
        assert_eq!(v.doc.paragraphs, before);
        assert!(!v.is_modified() && !v.doc.can_undo());
        // Saved from the code, the file is the same bytes.
        v.toggle_md_source();
        assert!(v.saves_as_typed(apex_io::Format::Text));
        let text = v.md_source_for(apex_io::Format::Text).unwrap();
        let out = dir.join("b.txt");
        apex_io::save_text(apex_io::TextSource::Raw(&text), &out, apex_io::Format::Text, apex_io::Encoding::Latin1).unwrap();
        assert_eq!(apex_fs::read(&out).unwrap(), bytes);
        // Edited, it reads back as plain text (no Markdown).
        type_in(&mut v, "\n# no es título");
        v.toggle_md_source();
        let last = v.doc.paragraphs.last().unwrap();
        assert_eq!((last.text.as_str(), last.style.kind), ("# no es título", ParaKind::Normal));
        let _ = apex_fs::remove_dir_all(&dir);
    }

    #[test]
    fn markdown_file_shows_its_own_text() {
        let dir = temp_dir("md");
        // Not how the writer would put it.
        let raw = "#   Título\n\n* uno\n* dos\n";
        let path = dir.join("a.md");
        apex_fs::write(&path, raw).unwrap();
        let mut v = open_file(&path, apex_io::Encoding::Utf8);
        let before = v.doc.paragraphs.clone();
        v.toggle_md_source();
        assert_eq!(v.md.code.as_ref().unwrap().text, raw);
        assert_eq!(v.md_source_for(apex_io::Format::Markdown).unwrap(), raw);
        v.toggle_md_source();
        assert_eq!(v.doc.paragraphs, before);
        assert!(!v.is_modified());
        // Changed in the formatted view, the code is written from it.
        v.doc.replace_body(apex_io::markdown::read("Otro\n").paragraphs);
        v.toggle_md_source();
        assert_eq!(v.md.code.as_ref().unwrap().text, apex_io::markdown::write(&v.doc));
        let _ = apex_fs::remove_dir_all(&dir);
    }

    #[test]
    fn rich_documents_show_markdown() {
        let dir = temp_dir("docx");
        let path = dir.join("a.docx");
        apex_io::save_doc(&apex_io::markdown::read("# Título\n\nHola\n"), &path, apex_io::Format::Docx).unwrap();
        let mut v = open_file(&path, apex_io::Encoding::Utf8);
        v.toggle_md_source();
        assert!(v.md.code.as_ref().unwrap().text.starts_with("# Título"));
        assert!(!v.saves_as_typed(apex_io::Format::Docx));
        // A new document saved as .txt shows its text from then on.
        v.toggle_md_source();
        v.source_saved_as(&dir.join("b.txt"));
        v.toggle_md_source();
        assert_eq!(v.md.code.as_ref().unwrap().text, "Título\nHola");
        let _ = apex_fs::remove_dir_all(&dir);
    }

    #[test]
    fn source_code_opens_as_code() {
        let dir = temp_dir("rs");
        let raw = "fn main() {\n    println!(\"hola\"); // ñ\n}\n";
        for name in ["main.rs", "a.json"] {
            let path = dir.join(name);
            apex_fs::write(&path, raw).unwrap();
            let mut v = open_file(&path, apex_io::Encoding::Utf8);
            assert!(v.md_source() && v.code_only());
            assert_eq!(v.md.code.as_ref().unwrap().text, raw);
            assert!(!v.is_modified());
            // It stays code.
            v.toggle_md_source();
            v.leave_md_source();
            assert!(v.md_source());
            type_in(&mut v, "// fin\n");
            assert!(v.is_modified());
            assert_eq!(v.md_source_for(apex_io::Format::Text).unwrap(), format!("{raw}// fin\n"));
            v.md_saved(apex_io::Format::Text);
            assert!(!v.is_modified());
            // Undo goes back to the file's text.
            v.undo();
            assert_eq!(v.md.code.as_ref().unwrap().text, raw);
        }
        let _ = apex_fs::remove_dir_all(&dir);
    }

    /// Each line number painted beside the code is on the row its line
    /// starts at, with emoji, CJK, tabs and empty lines, at any scale.
    #[test]
    fn line_numbers_sit_on_their_rows() {
        let text = format!("fn a() {{}}\n\n// 😀🎉 emoji\nlet s = \"中文 日本語 한국어\";\n\n\tlet t = 1;\n{}\n\nfin", "x".repeat(400));
        for (ppp, size) in [(1.0, 13.0), (1.25, 17.3), (1.5, 9.7)] {
            let mut c = MdCode::new(text.clone(), 0);
            let ctx = egui::Context::default();
            ctx.set_pixels_per_point(ppp);
            let input = || egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(700.0, 500.0))), ..Default::default() };
            let mut shapes = Vec::new();
            for _ in 0..2 {
                let mut out = ctx.run_ui(input(), |ui| {
                    source_area(ui, Id::new("code"), &mut c, (size, Source::Code(Lang::of("rs"))), (true, false), true);
                });
                out.textures_delta.clear();
                shapes = out.shapes;
            }
            let texts: Vec<_> = shapes.iter().filter_map(|s| if let egui::Shape::Text(t) = &s.shape { Some(t) } else { None }).collect();
            let code = texts.iter().find(|t| t.galley.text() == text).expect("the code is shown");
            let starts: Vec<f32> = code_gutter::line_rows(&code.galley).into_iter().map(|(_, y)| code.pos.y + y).collect();
            assert_eq!(starts.len(), 9);
            // Every row of a text of digits is a number: where it is.
            let mut numbers = Vec::new();
            for t in texts.iter().filter(|t| !t.galley.text().trim().is_empty() && t.galley.text().chars().all(|ch| ch.is_ascii_digit() || ch.is_whitespace())) {
                for row in &t.galley.rows {
                    if let Ok(n) = row.text().trim().parse::<usize>() {
                        numbers.push((n, t.pos.y + row.rect().min.y));
                    }
                }
            }
            assert_eq!(numbers.iter().map(|n| n.0).collect::<Vec<_>>(), (1..=9).collect::<Vec<_>>(), "at {ppp}");
            for (n, y) in numbers {
                assert!((y - starts[n - 1]).abs() <= 0.5, "line {n} at {ppp}: number at {y}, row at {}", starts[n - 1]);
            }
        }
    }

    #[test]
    fn backups_have_changed_code() {
        let mut v = view("Hola\n");
        v.toggle_md_source();
        let rev = v.backup_revision();
        type_in(&mut v, "\nAdiós\n");
        assert_ne!(v.backup_revision(), rev);
        assert_eq!(v.backup_doc().paragraphs.last().unwrap().text, "Adiós");
    }

    /// A new window of the code shows it too, with what was typed; going
    /// back and undoing undoes only that.
    #[test]
    fn a_new_window_shares_the_code() {
        let mut v = view(MD);
        v.toggle_md_source();
        let text = v.md.code.as_ref().unwrap().text.clone();
        type_in(&mut v, "\nAdiós\n");
        let mut w = v.linked_window();
        assert!(w.md_source());
        w.swap_document(&mut v);
        assert_eq!(w.doc.paragraphs.last().unwrap().text, "Adiós");
        v.swap_document(&mut w);
        v.md_follow();
        v.undo();
        v.md_follow();
        assert_eq!(v.md.code.as_ref().unwrap().text, text);
        v.toggle_md_source();
        assert!(v.doc.paragraphs.iter().all(|p| p.text != "Adiós"));
    }

    /// Editing a line of the code keeps the look of the rest (and of the
    /// edited line's own text) and the comments; undo brings all back.
    #[test]
    fn editing_the_code_keeps_the_formatting() {
        let mut doc = apex_io::markdown::read(MD);
        for p in &mut doc.paragraphs {
            for r in &mut p.runs {
                r.attrs.color = Some(0x2255aa);
                r.attrs.font = Some(std::sync::Arc::from("Georgia"));
            }
        }
        let before = doc.paragraphs.clone();
        let mut v = DocView::new(doc);
        v.toggle_md_source();
        let c = v.md.code.as_mut().unwrap();
        let old = c.text.clone();
        c.text = c.text.replace("Un párrafo con", "Un párrafo nuevo con");
        c.record(old, 0);
        v.toggle_md_source();
        let ps = &v.doc.paragraphs;
        assert_eq!(ps.len(), before.len());
        for (a, b) in ps.iter().zip(&before) {
            if a.text.contains("nuevo") {
                assert!(a.runs.iter().all(|r| r.attrs.color == Some(0x2255aa) && r.attrs.font.as_deref() == Some("Georgia")), "{:?}", a.runs);
            } else {
                assert_eq!(a, b);
            }
        }
        v.undo();
        assert_eq!(v.doc.paragraphs, before);
    }
}
