//! Documents: reading (wrapped to the terminal, with a panel of headings)
//! and editing the text through the document model, which keeps all the
//! formatting the terminal can't show; undo is the model's.

use apex_doc::{Attrs, Document, LINE_BREAK, ParaKind, Paragraph, Pos, Selection};
use apex_i18n::{tr, trf};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use unicode_width::UnicodeWidthStr;

use crate::cmd::Cmd;
use crate::doc_layout::{self, ACCENT, Layout};
use crate::term::Colors;
use crate::ui::{self, Ask, fill, fit, text};

const OUTLINE_W: u16 = 30;
const MAX_TEXT_W: u16 = 100;

pub struct DocUi {
    pub doc: Document,
    pub sel: Selection,
    layout: Layout,
    /// What the layout was built for: revision, width, colors.
    layout_key: Option<(u64, u16, Colors)>,
    /// First line shown.
    pub scroll: usize,
    /// Column kept while moving up and down.
    want_x: Option<u16>,
    pub outline: bool,
    outline_focus: bool,
    outline_sel: usize,
    pub(crate) last_find: Option<String>,
    /// Copied paragraphs (with their formatting).
    clip: Option<Vec<Paragraph>>,
    /// The last frame's size.
    area: Rect,
    text_area: Rect,
    outline_area: Rect,
    /// Screen x of the text column.
    text_x: u16,
    drag: bool,
    words: Option<(u64, usize)>,
    pub message: Option<(String, bool)>,
    pub clipboard_out: Option<String>,
}

impl DocUi {
    pub fn new(doc: Document) -> DocUi {
        DocUi {
            doc,
            sel: Selection::caret(Pos::new(0, 0)),
            layout: Layout::default(),
            layout_key: None,
            scroll: 0,
            want_x: None,
            outline: false,
            outline_focus: false,
            outline_sel: 0,
            last_find: None,
            clip: None,
            area: Rect::new(0, 0, 80, 24),
            text_area: Rect::new(0, 1, 80, 22),
            outline_area: Rect::default(),
            text_x: 0,
            drag: false,
            words: None,
            message: None,
            clipboard_out: None,
        }
    }

    fn info(&mut self, s: impl Into<String>) {
        self.message = Some((s.into(), false));
    }

    fn error(&mut self, s: impl Into<String>) {
        self.message = Some((s.into(), true));
    }

    /// Width of the text column for a screen area.
    fn column(&self, area: Rect) -> (u16, u16) {
        let left = if self.outline { OUTLINE_W.min(area.width / 3) } else { 0 };
        let avail = area.width.saturating_sub(left + 2);
        let w = avail.min(MAX_TEXT_W);
        (area.x + left + 1 + (avail - w) / 2, w)
    }

    /// Lays the document out again if it changed.
    fn relayout(&mut self, width: u16, colors: Colors) {
        let key = (self.doc.revision, width, colors);
        if self.layout_key != Some(key) {
            self.layout = doc_layout::build(&self.doc, width, colors);
            self.layout_key = Some(key);
        }
    }

    fn ensure_layout(&mut self) {
        let (_, w) = self.column(self.area);
        let colors = self.layout_key.map_or(Colors::Rgb, |k| k.2);
        self.relayout(w, colors);
    }

    fn caret_line(&self) -> Option<(usize, u16)> {
        let c = self.sel.cursor;
        self.layout.locate(c.para, c.offset)
    }

    /// Scrolls so that the caret shows.
    fn ensure_visible(&mut self) {
        let h = self.text_area.height.max(1) as usize;
        if let Some((ln, _)) = self.caret_line() {
            if ln < self.scroll {
                self.scroll = ln;
            } else if ln >= self.scroll + h {
                self.scroll = ln + 1 - h;
            }
        }
    }

    fn set_caret(&mut self, p: Pos, extend: bool) {
        let p = self.doc.clamp(p);
        self.sel = if extend { Selection { anchor: self.sel.anchor, cursor: p } } else { Selection::caret(p) };
    }

    /// Moves a line up or down, keeping the column.
    fn vertical(&mut self, lines: i64, extend: bool) {
        self.ensure_layout();
        let Some((ln, x)) = self.caret_line() else { return };
        let want = *self.want_x.get_or_insert(x);
        let mut t = ln as i64;
        let last = self.layout.lines.len() as i64 - 1;
        let step = lines.signum();
        let mut left = lines.abs();
        while left > 0 {
            let n = t + step;
            if n < 0 || n > last {
                break;
            }
            t = n;
            // Lines with no text (borders, blank lines) don't count.
            if self.layout.lines[t as usize].segs.iter().any(|s| s.src.is_some()) {
                left -= 1;
            }
        }
        if let Some((para, off)) = self.layout.hit(t as usize, want) {
            self.set_caret(Pos::new(para, off), extend);
        }
    }

    /// Start or end of the screen line.
    fn line_edge(&mut self, end: bool, extend: bool) {
        self.ensure_layout();
        let Some((ln, _)) = self.caret_line() else { return };
        let x = if end { u16::MAX } else { 0 };
        if let Some((para, off)) = self.layout.hit(ln, x) {
            self.set_caret(Pos::new(para, off), extend);
        }
    }

    // --- keys --------------------------------------------------------------------------

    pub fn key(&mut self, k: KeyEvent) -> Option<Cmd> {
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        if self.outline && self.outline_focus {
            match k.code {
                KeyCode::Up => self.outline_sel = self.outline_sel.saturating_sub(1),
                KeyCode::Down => self.outline_sel = (self.outline_sel + 1).min(self.layout.headings.len().saturating_sub(1)),
                KeyCode::Enter => {
                    self.goto_heading(self.outline_sel);
                    self.outline_focus = false;
                }
                KeyCode::Esc | KeyCode::Tab | KeyCode::Right => self.outline_focus = false,
                KeyCode::F(6) => self.outline = false,
                _ => {}
            }
            return None;
        }
        let vertical = matches!(k.code, KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown);
        if !vertical {
            self.want_x = None;
        }
        let sel = self.sel;
        let c = sel.cursor;
        match k.code {
            KeyCode::Left if ctrl => self.set_caret(self.doc.prev_word(c), shift),
            KeyCode::Right if ctrl => self.set_caret(self.doc.next_word(c), shift),
            KeyCode::Left if !shift && !sel.is_empty() => self.set_caret(sel.start(), false),
            KeyCode::Right if !shift && !sel.is_empty() => self.set_caret(sel.end(), false),
            KeyCode::Left => self.set_caret(self.doc.prev_char(c), shift),
            KeyCode::Right => self.set_caret(self.doc.next_char(c), shift),
            KeyCode::Up => self.vertical(-1, shift),
            KeyCode::Down => self.vertical(1, shift),
            KeyCode::PageUp => self.vertical(-(self.text_area.height.max(2) as i64 - 1), shift),
            KeyCode::PageDown => self.vertical(self.text_area.height.max(2) as i64 - 1, shift),
            KeyCode::Home if ctrl => self.set_caret(Pos::new(0, 0), shift),
            KeyCode::End if ctrl => self.set_caret(self.doc.end(), shift),
            KeyCode::Home => self.line_edge(false, shift),
            KeyCode::End => self.line_edge(true, shift),
            KeyCode::Esc => self.set_caret(c, false),
            KeyCode::F(6) => {
                if self.outline {
                    self.outline_focus = !self.outline_focus;
                } else {
                    self.outline = true;
                    self.outline_focus = true;
                    self.sync_outline();
                }
            }
            KeyCode::Enter if shift => self.type_text(&LINE_BREAK.to_string()),
            KeyCode::Enter => {
                let p = self.doc.new_paragraph(sel);
                self.set_caret(p, false);
            }
            KeyCode::Backspace => {
                let p = self.doc.backspace(sel, ctrl);
                self.set_caret(p, false);
            }
            KeyCode::Delete => {
                let p = self.doc.delete_forward(sel, ctrl);
                self.set_caret(p, false);
            }
            KeyCode::Tab | KeyCode::BackTab => {
                let para = &self.doc.paragraphs[c.para];
                if para.style.kind.is_list() {
                    let out = shift || k.code == KeyCode::BackTab;
                    let (a, b) = (sel.start().para, sel.end().para);
                    self.doc.set_para_style(a, b, sel, &|s| s.level = if out { s.level.saturating_sub(1) } else { (s.level + 1).min(8) });
                } else if k.code == KeyCode::Tab {
                    self.type_text("\t");
                }
            }
            KeyCode::Char(ch) if ctrl => {
                return match ch.to_ascii_lowercase() {
                    'c' => Some(Cmd::Copy),
                    'x' => Some(Cmd::Cut),
                    'v' => Some(Cmd::Paste),
                    'a' => Some(Cmd::SelectAll),
                    'b' => Some(Cmd::Bold),
                    'i' => Some(Cmd::Italic),
                    'u' => Some(Cmd::Underline),
                    _ => None,
                };
            }
            KeyCode::Char(ch) if alt => {
                return match ch {
                    '0' => Some(Cmd::ParaKind(ParaKind::Normal)),
                    '1'..='3' => Some(Cmd::ParaKind(ParaKind::Heading(ch as u8 - b'0'))),
                    'b' | 'B' => Some(Cmd::ParaKind(ParaKind::Bullet)),
                    'n' | 'N' => Some(Cmd::ParaKind(ParaKind::Numbered)),
                    'i' | 'I' => Some(Cmd::Italic),
                    's' | 'S' => Some(Cmd::Strike),
                    'o' | 'O' => Some(Cmd::Outline),
                    _ => None,
                };
            }
            KeyCode::Char(ch) => {
                let mut b = [0; 4];
                self.type_text(ch.encode_utf8(&mut b));
            }
            _ => {}
        }
        self.after_move();
        None
    }

    fn type_text(&mut self, s: &str) {
        let p = self.doc.insert_text(self.sel, s, None);
        self.set_caret(p, false);
    }

    fn after_move(&mut self) {
        self.ensure_layout();
        self.ensure_visible();
        if self.outline {
            self.sync_outline();
        }
    }

    /// Selects the heading the caret is under.
    fn sync_outline(&mut self) {
        let p = self.sel.cursor.para;
        if let Some(i) = self.layout.headings.iter().rposition(|h| h.0 <= p) {
            self.outline_sel = i;
        }
    }

    fn goto_heading(&mut self, i: usize) {
        let Some(&(para, ..)) = self.layout.headings.get(i) else { return };
        self.set_caret(Pos::new(para, 0), false);
        self.ensure_layout();
        if let Some((ln, _)) = self.caret_line() {
            self.scroll = ln;
        }
    }

    // --- mouse -------------------------------------------------------------------------

    pub fn mouse(&mut self, m: MouseEvent) -> Option<Cmd> {
        let (x, y) = (m.column, m.row);
        let inside = |r: Rect| x >= r.x && x < r.right() && y >= r.y && y < r.bottom();
        match m.kind {
            MouseEventKind::ScrollDown => self.scroll = (self.scroll + 3).min(self.layout.lines.len().saturating_sub(1)),
            MouseEventKind::ScrollUp => self.scroll = self.scroll.saturating_sub(3),
            MouseEventKind::Down(MouseButton::Left) if self.outline && inside(self.outline_area) => {
                let i = (y - self.outline_area.y) as usize + self.outline_scroll();
                if i < self.layout.headings.len() {
                    self.outline_sel = i;
                    self.goto_heading(i);
                }
            }
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left) if inside(self.text_area) || self.drag => {
                let ln = self.scroll + y.saturating_sub(self.text_area.y) as usize;
                let lx = x.saturating_sub(self.text_x);
                if let Some((para, off)) = self.layout.hit(ln.min(self.layout.lines.len().saturating_sub(1)), lx) {
                    let extend = matches!(m.kind, MouseEventKind::Drag(_)) || m.modifiers.contains(KeyModifiers::SHIFT);
                    self.set_caret(Pos::new(para, off), extend);
                    self.drag = true;
                    self.want_x = None;
                }
            }
            MouseEventKind::Up(_) => self.drag = false,
            _ => {}
        }
        None
    }

    fn outline_scroll(&self) -> usize {
        self.outline_sel.saturating_sub(self.outline_area.height.max(1) as usize - 1)
    }

    // --- commands ----------------------------------------------------------------------

    /// The selection, or the word at the caret.
    fn target(&self) -> Selection {
        if self.sel.is_empty() {
            let (a, b) = self.doc.word_at(self.sel.cursor);
            Selection { anchor: a, cursor: b }
        } else {
            self.sel
        }
    }

    pub fn run(&mut self, cmd: Cmd) {
        let sel = self.sel;
        let (a, b) = (sel.start().para, sel.end().para);
        match cmd {
            Cmd::Undo => match self.doc.undo() {
                Some(s) => self.sel = s,
                None => self.info(tr("tui.nothing_undo")),
            },
            Cmd::Redo => {
                if let Some(s) = self.doc.redo() {
                    self.sel = s;
                }
            }
            Cmd::SelectAll => self.sel = Selection { anchor: Pos::new(0, 0), cursor: self.doc.end() },
            Cmd::Copy | Cmd::Cut => {
                if sel.is_empty() {
                    return;
                }
                self.clip = Some(self.doc.slice(sel.start(), sel.end()));
                let t = self.doc.text_range(sel.start(), sel.end()).replace(LINE_BREAK, "\n");
                self.clipboard_out = Some(t);
                if cmd == Cmd::Cut {
                    let p = self.doc.delete(sel);
                    self.set_caret(p, false);
                }
            }
            Cmd::Paste => match self.clip.clone() {
                Some(paras) => {
                    let p = self.doc.insert_paragraphs(sel, &paras);
                    self.set_caret(p, false);
                }
                None => self.info(tr("tui.paste_hint")),
            },
            Cmd::Bold => self.doc.toggle(self.target(), |a: &Attrs| a.bold, |a, v| a.bold = v),
            Cmd::Italic => self.doc.toggle(self.target(), |a: &Attrs| a.italic, |a, v| a.italic = v),
            Cmd::Underline => self.doc.toggle(self.target(), |a: &Attrs| a.underline, |a, v| a.underline = v),
            Cmd::Strike => self.doc.toggle(self.target(), |a: &Attrs| a.strike, |a, v| a.strike = v),
            Cmd::ParaKind(kind) => {
                self.doc.set_para_style(a, b, sel, &|s| {
                    s.kind = kind;
                    s.custom = 0;
                    if !kind.is_list() {
                        s.list = 0;
                    }
                });
            }
            Cmd::ParaAlign(al) => self.doc.set_para_style(a, b, sel, &|s| s.align = al),
            Cmd::Outline => {
                self.outline = !self.outline;
                self.outline_focus = self.outline;
                self.sync_outline();
            }
            Cmd::FindNext => {
                if let Some(t) = self.last_find.clone() {
                    self.find(&t);
                }
            }
            _ => {}
        }
        self.after_move();
    }

    fn find(&mut self, needle: &str) {
        let from = self.sel.end();
        let hit = self.doc.find(needle, from, false).or_else(|| self.doc.find(needle, Pos::new(0, 0), false));
        match hit {
            Some((a, b)) => {
                self.sel = Selection { anchor: a, cursor: b };
                self.after_move();
            }
            None => self.error(trf("tui.not_found", &[("text", &needle)])),
        }
    }

    pub fn paste_text(&mut self, text: &str) {
        let t = text.replace("\r\n", "\n").replace('\r', "\n");
        self.type_text(&t);
        self.after_move();
    }

    pub fn ask_text(&mut self, ask: &Ask) -> (String, String) {
        match ask {
            Ask::Find => (tr("tui.find").to_string(), self.last_find.clone().unwrap_or_default()),
            Ask::ReplaceFind => (tr("tui.replace").to_string(), self.last_find.clone().unwrap_or_default()),
            _ => (String::new(), String::new()),
        }
    }

    pub fn answer(&mut self, ask: Ask, text: String) {
        match ask {
            Ask::Find if !text.is_empty() => {
                self.find(&text);
                self.last_find = Some(text);
            }
            Ask::ReplaceWith(find) => {
                let n = self.doc.replace_all(&find, &text, false);
                self.set_caret(self.sel.cursor, false);
                self.info(trf("tui.replaced_n", &[("n", &n)]));
                self.last_find = Some(find);
                self.after_move();
            }
            _ => {}
        }
    }

    // --- drawing -----------------------------------------------------------------------

    /// Draws the document; its title row starts at `title_x` (the menu
    /// bar's titles are left of it).
    pub fn draw(&mut self, f: &mut Frame, title_x: u16, colors: Colors, name: &str, modified: bool, message: Option<&(String, bool)>, overlay_open: bool) {
        let area = f.area();
        if area.height < 4 || area.width < 20 {
            return;
        }
        self.area = area;
        let (tx, tw) = self.column(area);
        self.relayout(tw, colors);
        self.text_x = tx;
        self.text_area = Rect::new(tx, area.y + 1, tw, area.height - 2);
        self.outline_area = if self.outline { Rect::new(area.x, area.y + 1, OUTLINE_W.min(area.width / 3), area.height - 2) } else { Rect::default() };
        let h = self.text_area.height as usize;
        self.scroll = self.scroll.min(self.layout.lines.len().saturating_sub(1));
        let buf = f.buffer_mut();

        // Title bar: file and the heading the caret is under.
        let bar = Style::new().bg(colors.color(0x252526)).fg(colors.color(0xE0E0E0));
        fill(buf, Rect::new(area.x, area.y, area.width, 1), bar);
        let mut title = format!(" {name}{}", if modified { " ●" } else { "" });
        let p = self.sel.cursor.para;
        if let Some(hd) = self.layout.headings.iter().rev().find(|h| h.0 <= p) {
            title.push_str(&format!("  ›  {}", hd.2));
        }
        let room = area.width.saturating_sub(title_x);
        text(buf, area.x + title_x, area.y, &fit(&title, room as usize), room, bar);

        // The text.
        let (s0, s1) = (self.sel.start(), self.sel.end());
        let sel_bg = colors.color(0x264F78);
        for (row, ln) in (self.scroll..self.layout.lines.len()).take(h).enumerate() {
            let y = self.text_area.y + row as u16;
            for s in &self.layout.lines[ln].segs {
                let x = tx + s.x;
                if x >= tx + tw + 1 {
                    continue;
                }
                let room = (tx + tw + 1).saturating_sub(x);
                match s.src {
                    Some(src) if !self.sel.is_empty() => {
                        // Character by character, for the selection.
                        let mut cx = x;
                        let mut chars: Vec<(usize, String)> = if src.atomic { vec![(src.start, s.text.clone())] } else { s.text.char_indices().map(|(k, c)| (src.start + k, c.to_string())).collect() };
                        if chars.is_empty() {
                            chars.push((src.start, String::new()));
                        }
                        for (off, t) in chars {
                            let pos = Pos::new(src.para, off);
                            let inside = pos >= s0 && pos < s1;
                            let st = if inside { s.style.bg(sel_bg) } else { s.style };
                            let w = t.width() as u16;
                            if cx + w > x + room {
                                break;
                            }
                            text(buf, cx, y, &t, w, st);
                            cx += w;
                        }
                    }
                    _ => {
                        text(buf, x, y, &s.text, room, s.style);
                    }
                }
            }
        }

        // Headings panel.
        if self.outline {
            let r = self.outline_area;
            let base = Style::new().bg(colors.color(0x1E1E1E)).fg(colors.color(0xD0D0D0));
            fill(buf, r, base);
            text(buf, r.x + 1, r.y, &fit(tr("tui.outline"), r.width as usize - 2), r.width - 2, base.fg(colors.color(ACCENT)).add_modifier(Modifier::BOLD));
            let list = Rect::new(r.x, r.y + 1, r.width, r.height.saturating_sub(1));
            let top = self.outline_sel.saturating_sub(list.height.max(1) as usize - 1);
            for (k, (_, level, t)) in self.layout.headings.iter().enumerate().skip(top).take(list.height as usize) {
                let y = list.y + (k - top) as u16;
                let st = if k == self.outline_sel { if self.outline_focus { base.bg(colors.color(ACCENT)).fg(colors.color(0x101010)) } else { base.bg(colors.color(0x3C3C3C)) } } else { base };
                fill(buf, Rect::new(list.x, y, list.width, 1), st);
                let indent = (*level as u16).saturating_sub(1).min(6);
                let w = list.width.saturating_sub(indent + 2);
                text(buf, list.x + 1 + indent, y, &fit(t, w as usize), w, st);
            }
        }

        // Status bar.
        let words = match self.words {
            Some((rev, n)) if rev == self.doc.revision => n,
            _ => {
                let n = self.doc.word_count();
                self.words = Some((self.doc.revision, n));
                n
            }
        };
        let right = trf("tui.doc_status", &[("words", &words), ("para", &(p + 1)), ("paras", &self.doc.paragraphs.len())]);
        ui::status_bar(buf, Rect::new(area.x, area.bottom() - 1, area.width, 1), message, &format!("{name}  {}", tr("tui.hint")), &right);

        if !overlay_open
            && !self.outline_focus
            && let Some((ln, x)) = self.caret_line()
            && ln >= self.scroll
            && ln < self.scroll + h
        {
            f.set_cursor_position((tx + x.min(tw), self.text_area.y + (ln - self.scroll) as u16));
        }
    }
}
