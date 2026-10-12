//! Shared pieces: the one-line editor, dialogs over the view (questions,
//! menus and the command palette, the open menu bar, help, the quit
//! confirmation) and text drawing helpers.

use apex_i18n::tr;
use apex_io::Kind;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, BorderType, Clear, Widget};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::cmd::{self, Cmd, Menu};

pub const ACCENT: Color = Color::Rgb(0x4F, 0x9D, 0xE8);

/// What a one-line question is for.
#[derive(Clone, Debug, PartialEq)]
pub enum Ask {
    GoTo,
    Find,
    ReplaceFind,
    /// The text to replace with; holds the text to find.
    ReplaceWith(String),
    SaveAs,
    ExportPdf,
    RenameSheet,
    NumFormat,
    ColWidth,
    TextColor,
    FillColor,
}

/// A one-line text editor (formula bar, questions, the palette's filter).
#[derive(Clone, Debug, Default)]
pub struct LineEdit {
    pub text: String,
    /// Caret, a byte offset.
    pub caret: usize,
}

impl LineEdit {
    pub fn new(s: &str) -> LineEdit {
        LineEdit { text: s.to_string(), caret: s.len() }
    }

    pub fn insert(&mut self, s: &str) {
        self.text.insert_str(self.caret, s);
        self.caret += s.len();
    }

    fn prev(&self, i: usize) -> usize {
        self.text[..i].char_indices().next_back().map_or(0, |(k, _)| k)
    }

    fn next(&self, i: usize) -> usize {
        self.text[i..].chars().next().map_or(i, |c| i + c.len_utf8())
    }

    fn word_left(&self) -> usize {
        let t = &self.text[..self.caret];
        let trimmed = t.trim_end();
        trimmed.rfind(|c: char| !c.is_alphanumeric()).map_or(0, |k| k + trimmed[k..].chars().next().map_or(1, char::len_utf8))
    }

    fn word_right(&self) -> usize {
        let t = &self.text[self.caret..];
        let skip = t.find(|c: char| c.is_alphanumeric()).unwrap_or(t.len());
        let rest = &t[skip..];
        self.caret + skip + rest.find(|c: char| !c.is_alphanumeric()).unwrap_or(rest.len())
    }

    /// Editing and caret keys; false for keys it doesn't use.
    pub fn key(&mut self, k: &KeyEvent) -> bool {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Char(c) if !ctrl && !k.modifiers.contains(KeyModifiers::ALT) => {
                let mut b = [0; 4];
                self.insert(c.encode_utf8(&mut b));
            }
            KeyCode::Backspace if self.caret > 0 => {
                let p = if ctrl { self.word_left() } else { self.prev(self.caret) };
                self.text.replace_range(p..self.caret, "");
                self.caret = p;
            }
            KeyCode::Backspace => {}
            KeyCode::Delete if self.caret < self.text.len() => {
                let n = if ctrl { self.word_right() } else { self.next(self.caret) };
                self.text.replace_range(self.caret..n, "");
            }
            KeyCode::Delete => {}
            KeyCode::Left => self.caret = if ctrl { self.word_left() } else { self.prev(self.caret) },
            KeyCode::Right => self.caret = if ctrl { self.word_right() } else { self.next(self.caret) },
            KeyCode::Home => self.caret = 0,
            KeyCode::End => self.caret = self.text.len(),
            KeyCode::Char('u') if ctrl => {
                self.text.replace_range(..self.caret, "");
                self.caret = 0;
            }
            _ => return false,
        }
        true
    }

    /// Draws the text in one row, scrolled so that the caret shows; returns
    /// the caret's screen position.
    pub fn render(&self, buf: &mut Buffer, area: Rect, style: Style) -> (u16, u16) {
        fill(buf, area, style);
        let w = area.width as usize;
        let before = self.text[..self.caret].width();
        // Scroll so that the caret stays inside.
        let skip = (before + 1).saturating_sub(w);
        let mut x = 0usize;
        let mut col = 0usize;
        for c in self.text.chars() {
            let cw = c.width().unwrap_or(0);
            if col >= skip {
                if x + cw > w {
                    break;
                }
                put(buf, area.x + x as u16, area.y, c, style);
                x += cw;
            }
            col += cw;
        }
        (area.x + (before - skip).min(w.saturating_sub(1)) as u16, area.y)
    }
}

/// Fills an area with spaces in a style.
pub fn fill(buf: &mut Buffer, area: Rect, style: Style) {
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(c) = buf.cell_mut((x, y)) {
                c.reset();
                c.set_style(style);
            }
        }
    }
}

fn put(buf: &mut Buffer, x: u16, y: u16, ch: char, style: Style) {
    if let Some(c) = buf.cell_mut((x, y)) {
        // A control character would reach the terminal as it is.
        c.set_char(if ch.is_control() { ' ' } else { ch }).set_style(style);
    }
}

/// Writes text clipped to `max` columns; returns the columns used.
pub fn text(buf: &mut Buffer, x: u16, y: u16, s: &str, max: u16, style: Style) -> u16 {
    if max == 0 || y >= buf.area.bottom() {
        return 0;
    }
    let (end, _) = buf.set_stringn(x, y, s, max as usize, style);
    end.saturating_sub(x)
}

/// Cuts text to a width, with "…" when cut.
pub fn fit(s: &str, w: usize) -> String {
    if s.width() <= w {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if used + cw + 1 > w {
            break;
        }
        out.push(c);
        used += cw;
    }
    if w > 0 {
        out.push('…');
    }
    out
}

/// A centered box of the given size, cleared, with a border and a title;
/// returns its inside.
pub fn dialog(f: &mut Frame, w: u16, h: u16, title: &str) -> Rect {
    let a = f.area();
    let w = w.min(a.width);
    let h = h.min(a.height);
    let r = Rect::new(a.x + (a.width - w) / 2, a.y + (a.height - h) / 3, w, h);
    Clear.render(r, f.buffer_mut());
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(ACCENT)).title(format!(" {title} "));
    let inner = block.inner(r);
    block.render(r, f.buffer_mut());
    inner
}

// --- dialogs -------------------------------------------------------------------------------

pub struct Prompt {
    pub ask: Ask,
    pub title: String,
    pub edit: LineEdit,
}

/// One entry of a list dialog.
pub struct Item {
    pub label: String,
    pub keys: String,
    pub cmd: Option<Cmd>,
    /// Check box of the filter menu.
    pub checked: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Purpose {
    /// Picking runs the entry's command.
    Commands,
    /// The value list of an autofilter column: Enter applies the checks.
    Filter { col: u32 },
}

/// A list to pick from, narrowed by typing: the command palette, submenus
/// and the autofilter's values.
pub struct Choice {
    pub title: String,
    pub all: Vec<Item>,
    shown: Vec<usize>,
    sel: usize,
    pub query: LineEdit,
    pub purpose: Purpose,
}

impl Choice {
    pub fn new(title: String, all: Vec<Item>, purpose: Purpose) -> Choice {
        let shown = (0..all.len()).collect();
        Choice { title, all, shown, sel: 0, query: LineEdit::default(), purpose }
    }

    fn from_entries(title: &str, entries: Vec<cmd::Entry>) -> Choice {
        let items = entries.into_iter().map(|e| Item { label: tr(e.label).to_string(), keys: e.keys.to_string(), cmd: Some(e.cmd), checked: None }).collect();
        Choice::new(title.to_string(), items, Purpose::Commands)
    }

    pub fn palette(kind: Kind) -> Choice {
        Choice::from_entries(tr("tui.palette"), cmd::palette(kind))
    }

    pub fn menu(m: Menu) -> Choice {
        let (title, entries) = cmd::menu(m);
        Choice::from_entries(tr(title), entries)
    }

    /// Keeps the entries whose label has every typed word.
    fn refilter(&mut self) {
        let q = self.query.text.to_lowercase();
        let words: Vec<&str> = q.split_whitespace().collect();
        self.shown = (0..self.all.len())
            .filter(|&i| {
                let l = format!("{} {}", self.all[i].label.to_lowercase(), self.all[i].keys.to_lowercase());
                words.iter().all(|w| l.contains(w))
            })
            .collect();
        self.sel = 0;
    }

    fn toggle(&mut self) {
        let Some(&i) = self.shown.get(self.sel) else { return };
        let Some(on) = self.all[i].checked else { return };
        if i == 0 {
            // "(Select all)".
            for it in &mut self.all {
                if it.checked.is_some() {
                    it.checked = Some(!on);
                }
            }
        } else {
            self.all[i].checked = Some(!on);
            let rest = self.all[1..].iter().filter_map(|x| x.checked).all(|c| c);
            self.all[0].checked = Some(rest);
        }
    }

    fn key(&mut self, k: &KeyEvent) -> Outcome {
        let n = self.shown.len();
        match k.code {
            KeyCode::Esc => return Outcome::Close,
            KeyCode::Up => self.sel = self.sel.checked_sub(1).unwrap_or(n.saturating_sub(1)),
            KeyCode::Down => self.sel = if self.sel + 1 < n { self.sel + 1 } else { 0 },
            KeyCode::PageUp => self.sel = self.sel.saturating_sub(10),
            KeyCode::PageDown => self.sel = (self.sel + 10).min(n.saturating_sub(1)),
            KeyCode::Char(' ') if matches!(self.purpose, Purpose::Filter { .. }) => self.toggle(),
            KeyCode::Enter => {
                let item = self.shown.get(self.sel).map(|&i| &self.all[i]);
                if let Some(c) = item.and_then(|i| i.cmd.clone()) {
                    return Outcome::Run(c);
                }
                if let Purpose::Filter { .. } = self.purpose {
                    let picked = self.all.iter().skip(1).filter_map(|i| Some((i.label.clone(), i.checked?))).collect();
                    return Outcome::Picked(self.purpose, picked);
                }
                return Outcome::Close;
            }
            _ => {
                if self.query.key(k) {
                    self.refilter();
                }
            }
        }
        Outcome::Keep
    }

    fn draw(&self, f: &mut Frame) {
        let a = f.area();
        let h = (self.all.len() as u16 + 4).min(a.height.saturating_sub(2)).max(6);
        let inner = dialog(f, 64, h, &self.title);
        if inner.height < 2 {
            return;
        }
        let buf = f.buffer_mut();
        let (cx, cy) = self.query.render(buf, Rect { height: 1, ..inner }, Style::new().add_modifier(Modifier::UNDERLINED));
        let rows = inner.height as usize - 1;
        let top = self.sel.saturating_sub(rows.saturating_sub(1));
        for (k, &i) in self.shown.iter().enumerate().skip(top).take(rows) {
            let y = inner.y + 1 + (k - top) as u16;
            let it = &self.all[i];
            let style = if k == self.sel { Style::new().bg(ACCENT).fg(Color::Black) } else { Style::new() };
            fill(buf, Rect::new(inner.x, y, inner.width, 1), style);
            let mut label = String::new();
            if let Some(c) = it.checked {
                label.push_str(if c { "[x] " } else { "[ ] " });
            }
            label.push_str(&it.label);
            let kw = it.keys.width() as u16;
            let lw = inner.width.saturating_sub(kw + 2);
            text(buf, inner.x + 1, y, &fit(&label, lw as usize), lw, style);
            if kw > 0 && kw + 2 < inner.width {
                text(buf, inner.right() - kw - 1, y, &it.keys, kw, style.add_modifier(Modifier::DIM));
            }
        }
        f.set_cursor_position((cx, cy));
    }
}

pub enum Overlay {
    Prompt(Prompt),
    Choice(Choice),
    /// Key help, scrolled by this many lines.
    Help(u16),
    ConfirmQuit,
    /// The menu bar with a menu pulled down.
    Bar(crate::menubar::Open),
}

pub enum Outcome {
    Keep,
    Close,
    Run(Cmd),
    Answer(Ask, String),
    /// The checks of a filter list: value and whether it is checked.
    Picked(Purpose, Vec<(String, bool)>),
}

/// Keys and what they do, for the help (label keys; "" starts a section
/// whose title is the key text).
const HELP: &[(&str, &str)] = &[
    ("", "tui.help_general"),
    ("F1", "tui.cmd_help"),
    ("F9", "tui.help_menubar"),
    ("Alt+…", "tui.help_menu_alt"),
    ("F10 / Ctrl+P", "tui.palette"),
    ("Ctrl+S", "tui.cmd_save"),
    ("F12", "tui.cmd_save_as"),
    ("Ctrl+Q", "tui.cmd_quit"),
    ("Ctrl+Z / Ctrl+Y", "tui.help_undo"),
    ("Ctrl+C / Ctrl+X / Ctrl+V", "tui.help_clipboard"),
    ("Ctrl+F / F3", "tui.cmd_find"),
    ("Ctrl+H / Ctrl+R", "tui.cmd_replace"),
    ("Ctrl+B / Ctrl+I / Ctrl+U", "tui.help_emphasis"),
    ("", "tui.help_sheets"),
    ("↑ ↓ ← → PgUp PgDn", "tui.help_move"),
    ("Ctrl+↑↓←→ / Home / Ctrl+Home / Ctrl+End", "tui.help_jump"),
    ("Shift+…", "tui.help_select"),
    ("Enter / F2", "tui.cmd_edit_cell"),
    ("Tab (=…)", "tui.help_complete"),
    ("Delete", "tui.cmd_clear"),
    ("Ctrl+G / F5", "tui.cmd_goto"),
    ("Ctrl+PgUp / Ctrl+PgDn", "tui.help_sheets_switch"),
    ("Alt+R / Alt+C", "tui.help_insert"),
    ("Alt+Shift+R / Alt+Shift+C", "tui.help_delete"),
    ("Alt+L / Alt+Down", "tui.cmd_autofilter"),
    ("Alt+N / Alt+A", "tui.help_format"),
    ("Alt+← / Alt+→", "tui.help_width"),
    ("Shift+F9", "tui.cmd_recalc"),
    ("", "tui.help_docs"),
    ("↑ ↓ ← → PgUp PgDn Home End", "tui.help_move"),
    ("Enter / Backspace / Delete", "tui.help_edit_text"),
    ("Alt+0 / Alt+1..3 / Alt+B / Alt+N", "tui.cmd_para_style"),
    ("F6 / Alt+O", "tui.cmd_outline"),
    ("", "tui.help_mouse"),
];

impl Overlay {
    pub fn key(&mut self, k: KeyEvent) -> Outcome {
        match self {
            Overlay::Prompt(p) => match k.code {
                KeyCode::Esc => Outcome::Close,
                KeyCode::Enter => Outcome::Answer(p.ask.clone(), std::mem::take(&mut p.edit.text)),
                _ => {
                    p.edit.key(&k);
                    Outcome::Keep
                }
            },
            Overlay::Choice(c) => c.key(&k),
            Overlay::Bar(b) => b.key(&k),
            Overlay::Help(s) => match k.code {
                KeyCode::Down => {
                    *s = s.saturating_add(1);
                    Outcome::Keep
                }
                KeyCode::Up => {
                    *s = s.saturating_sub(1);
                    Outcome::Keep
                }
                KeyCode::PageDown => {
                    *s = s.saturating_add(10);
                    Outcome::Keep
                }
                KeyCode::PageUp => {
                    *s = s.saturating_sub(10);
                    Outcome::Keep
                }
                _ => Outcome::Close,
            },
            Overlay::ConfirmQuit => match k.code {
                // Yes in the interface languages: yes, sí, oui, ja, sì, sim.
                KeyCode::Char('y' | 'Y' | 's' | 'S' | 'o' | 'O' | 'j' | 'J') | KeyCode::Enter => Outcome::Run(Cmd::SaveAndQuit),
                KeyCode::Char('n' | 'N') => Outcome::Run(Cmd::QuitNow),
                _ => Outcome::Close,
            },
        }
    }

    pub fn paste(&mut self, s: &str) {
        let s = s.replace(['\r', '\n'], " ");
        match self {
            Overlay::Prompt(p) => p.edit.insert(&s),
            Overlay::Choice(c) => {
                c.query.insert(&s);
                c.refilter();
            }
            _ => {}
        }
    }

    pub fn draw(&self, f: &mut Frame) {
        match self {
            Overlay::Prompt(p) => {
                let inner = dialog(f, 60, 3, &p.title);
                let (x, y) = p.edit.render(f.buffer_mut(), inner, Style::new());
                f.set_cursor_position((x, y));
            }
            Overlay::Choice(c) => c.draw(f),
            Overlay::Bar(b) => b.draw(f),
            Overlay::Help(scroll) => {
                let a = f.area();
                let inner = dialog(f, 76, a.height.saturating_sub(2), tr("tui.cmd_help"));
                let buf = f.buffer_mut();
                let kw = HELP.iter().map(|h| h.0.width()).max().unwrap_or(0).min(inner.width as usize / 2) as u16;
                let max_scroll = (HELP.len() as u16).saturating_sub(inner.height);
                for (i, (keys, label)) in HELP.iter().enumerate().skip((*scroll).min(max_scroll) as usize).take(inner.height as usize) {
                    let y = inner.y + (i as u16 - (*scroll).min(max_scroll));
                    if keys.is_empty() {
                        text(buf, inner.x, y, tr(label), inner.width, Style::new().fg(ACCENT).add_modifier(Modifier::BOLD));
                    } else {
                        text(buf, inner.x + 1, y, keys, kw, Style::new().add_modifier(Modifier::BOLD));
                        text(buf, inner.x + kw + 3, y, tr(label), inner.width.saturating_sub(kw + 3), Style::new());
                    }
                }
            }
            Overlay::ConfirmQuit => {
                let inner = dialog(f, 60, 4, tr("tui.unsaved_title"));
                let buf = f.buffer_mut();
                text(buf, inner.x + 1, inner.y, tr("tui.unsaved_question"), inner.width - 1, Style::new());
                text(buf, inner.x + 1, inner.y + 1, tr("tui.unsaved_keys"), inner.width - 1, Style::new().add_modifier(Modifier::BOLD));
            }
        }
    }
}

/// The status bar: the message (or a hint) on the left, `right` on the
/// right.
pub fn status_bar(buf: &mut Buffer, area: Rect, message: Option<&(String, bool)>, left: &str, right: &str) {
    let base = Style::new().bg(Color::Rgb(0x2B, 0x2B, 0x2B)).fg(Color::Rgb(0xE0, 0xE0, 0xE0));
    fill(buf, area, base);
    let rw = right.width() as u16;
    let room = area.width.saturating_sub(rw + 2);
    match message {
        Some((m, err)) => {
            let st = if *err { base.fg(Color::Rgb(0xFF, 0x6B, 0x6B)).add_modifier(Modifier::BOLD) } else { base.fg(Color::Rgb(0x8F, 0xD1, 0x8F)) };
            text(buf, area.x + 1, area.y, &fit(m, room as usize), room, st);
        }
        None => {
            text(buf, area.x + 1, area.y, &fit(left, room as usize), room, base);
        }
    }
    if rw + 2 <= area.width {
        text(buf, area.right() - rw - 1, area.y, right, rw, base);
    }
}
