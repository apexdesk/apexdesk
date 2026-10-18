//! The classic menu bar: File, Edit, View… on the top row, opened with F9,
//! Alt+the underlined letter or a click. Its menus hold every command of
//! the palette with its keys; submenus are the palette's submenus.
//!
//! Menu titles carry their mnemonic in the translation ("&File",
//! "Arc&hivo", "文件(&F)"); entries get one on the fly, the first free
//! letter of their words, so they never collide in any language.

use apex_doc::ParaKind;
use apex_i18n::tr;
use apex_io::Kind;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Clear, Widget};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::cmd::{self, Cmd, Entry, Menu, e};
use crate::ui::{Ask, Outcome, fill, fit, text};

const BAR_BG: Color = Color::Rgb(0x2D, 0x2D, 0x30);
const BAR_FG: Color = Color::Rgb(0xE0, 0xE0, 0xE0);
const OFF_FG: Color = Color::Rgb(0x70, 0x70, 0x70);
const SEL_BG: Color = crate::ui::ACCENT;

/// One line of a menu.
pub enum Item {
    Cmd(Entry),
    /// A submenu, with the keys that open it from anywhere.
    Sub(Menu, &'static str),
    Sep,
}

/// One menu of the bar: its title's key and its items.
pub struct BarMenu {
    pub title: &'static str,
    pub items: Vec<Item>,
}

fn c(cmd: Cmd, label: &'static str, keys: &'static str) -> Item {
    Item::Cmd(e(cmd, label, keys))
}

/// The menus of the bar for a kind of file.
pub fn menus(kind: Kind) -> Vec<BarMenu> {
    use Item::{Sep, Sub};
    let file = BarMenu {
        title: "tui.menu_file",
        items: vec![c(Cmd::Save, "tui.cmd_save", "Ctrl+S"), c(Cmd::SaveAs, "tui.cmd_save_as", "F12"), c(Cmd::ExportPdf, "tui.cmd_export_pdf", ""), Sep, c(Cmd::Quit, "tui.cmd_quit", "Ctrl+Q")],
    };
    let mut edit = vec![c(Cmd::Undo, "tui.cmd_undo", "Ctrl+Z"), c(Cmd::Redo, "tui.cmd_redo", "Ctrl+Y"), Sep, c(Cmd::Cut, "tui.cmd_cut", "Ctrl+X"), c(Cmd::Copy, "tui.cmd_copy", "Ctrl+C"), c(Cmd::Paste, "tui.cmd_paste", "Ctrl+V"), Sep];
    if kind == Kind::Sheet {
        edit.extend([c(Cmd::EditCell, "tui.cmd_edit_cell", "F2"), c(Cmd::ClearContents, "tui.cmd_clear", "Delete")]);
    }
    edit.extend([c(Cmd::SelectAll, "tui.cmd_select_all", "Ctrl+A"), Sep, c(Cmd::Ask(Ask::Find), "tui.cmd_find", "Ctrl+F"), c(Cmd::FindNext, "tui.cmd_find_next", "F3"), c(Cmd::Ask(Ask::ReplaceFind), "tui.cmd_replace", "Ctrl+H")]);
    if kind == Kind::Sheet {
        edit.push(c(Cmd::Ask(Ask::GoTo), "tui.cmd_goto", "Ctrl+G"));
    }
    let edit = BarMenu { title: "tui.menu_edit", items: edit };
    let mut view = match kind {
        Kind::Sheet => vec![c(Cmd::ShowFormulas, "tui.cmd_show_formulas", "Ctrl+`"), c(Cmd::Freeze, "tui.cmd_freeze", "")],
        Kind::Doc | Kind::Slides => vec![c(Cmd::Outline, "tui.cmd_outline", "F6")],
    };
    view.extend([Sep, c(Cmd::ToggleMenuBar, "tui.cmd_menu_bar", ""), c(Cmd::Palette, "tui.palette", "F10")]);
    let view = BarMenu { title: "tui.menu_view", items: view };
    let emphasis = [c(Cmd::Bold, "tui.cmd_bold", "Ctrl+B"), c(Cmd::Italic, "tui.cmd_italic", "Ctrl+I"), c(Cmd::Underline, "tui.cmd_underline", "Ctrl+U"), c(Cmd::Strike, "tui.cmd_strike", "Alt+S")];
    let help = BarMenu { title: "tui.menu_help", items: vec![c(Cmd::Help, "tui.cmd_help", "F1"), c(Cmd::Palette, "tui.palette", "F10 / Ctrl+P")] };
    match kind {
        Kind::Sheet => {
            let mut format: Vec<Item> = emphasis.into();
            format.extend([
                Sep,
                Sub(Menu::NumFormat, "Alt+N"),
                c(Cmd::Ask(Ask::NumFormat), "tui.cmd_custom_format", ""),
                Sub(Menu::Align, "Alt+A"),
                Sub(Menu::TextColor, ""),
                Sub(Menu::FillColor, ""),
                Sep,
                c(Cmd::ColWider, "tui.cmd_col_wider", "Alt+→"),
                c(Cmd::ColNarrower, "tui.cmd_col_narrower", "Alt+←"),
                c(Cmd::Ask(Ask::ColWidth), "tui.cmd_col_width", ""),
            ]);
            vec![
                file,
                edit,
                view,
                BarMenu { title: "tui.menu_insert", items: vec![c(Cmd::InsertRows, "tui.cmd_insert_rows", "Alt+R"), c(Cmd::InsertCols, "tui.cmd_insert_cols", "Alt+C"), Sep, c(Cmd::AddSheet, "tui.cmd_add_sheet", "Shift+F11")] },
                BarMenu { title: "tui.menu_format", items: format },
                BarMenu {
                    title: "tui.menu_data",
                    items: vec![c(Cmd::SortAsc, "tui.cmd_sort_asc", ""), c(Cmd::SortDesc, "tui.cmd_sort_desc", ""), Sep, c(Cmd::AutoFilter, "tui.cmd_autofilter", "Alt+L"), c(Cmd::FilterMenu, "tui.cmd_filter_menu", "Alt+↓"), Sep, c(Cmd::Recalc, "tui.cmd_recalc", "Shift+F9")],
                },
                BarMenu {
                    title: "tui.menu_sheet",
                    items: vec![
                        c(Cmd::Ask(Ask::RenameSheet), "tui.cmd_rename_sheet", ""),
                        c(Cmd::DeleteSheet, "tui.cmd_delete_sheet", ""),
                        Sep,
                        c(Cmd::NextSheet, "tui.cmd_next_sheet", "Ctrl+PgDn"),
                        c(Cmd::PrevSheet, "tui.cmd_prev_sheet", "Ctrl+PgUp"),
                        Sep,
                        c(Cmd::DeleteRows, "tui.cmd_delete_rows", "Alt+Shift+R"),
                        c(Cmd::DeleteCols, "tui.cmd_delete_cols", "Alt+Shift+C"),
                    ],
                },
                help,
            ]
        }
        Kind::Doc | Kind::Slides => vec![
            file,
            edit,
            view,
            BarMenu { title: "tui.menu_format", items: emphasis.into() },
            BarMenu {
                title: "tui.menu_paragraph",
                items: vec![
                    Sub(Menu::ParaStyle, ""),
                    Sub(Menu::ParaAlign, ""),
                    Sep,
                    c(Cmd::ParaKind(ParaKind::Normal), "tui.style_normal", "Alt+0"),
                    c(Cmd::ParaKind(ParaKind::Heading(1)), "tui.style_h1", "Alt+1"),
                    c(Cmd::ParaKind(ParaKind::Heading(2)), "tui.style_h2", "Alt+2"),
                    c(Cmd::ParaKind(ParaKind::Heading(3)), "tui.style_h3", "Alt+3"),
                    c(Cmd::ParaKind(ParaKind::Bullet), "tui.style_bullet", "Alt+B"),
                    c(Cmd::ParaKind(ParaKind::Numbered), "tui.style_numbered", "Alt+N"),
                ],
            },
            help,
        ],
    }
}

// --- titles ----------------------------------------------------------------------------------

/// A menu title: its text and the mnemonic ("&" in the translation).
pub struct Title {
    pub text: String,
    /// The mnemonic, lowercase, and its char index in `text`.
    pub mn: Option<(char, usize)>,
}

pub fn title(label: &str) -> Title {
    let mut text = String::new();
    let mut mn = None;
    let mut amp = false;
    for ch in label.chars() {
        if ch == '&' && !amp {
            amp = true;
            continue;
        }
        if amp && mn.is_none() {
            mn = Some((ch.to_lowercase().next().unwrap_or(ch), text.chars().count()));
        }
        amp = false;
        text.push(ch);
    }
    Title { text, mn }
}

pub fn titles(kind: Kind) -> Vec<Title> {
    menus(kind).iter().map(|m| title(tr(m.title))).collect()
}

/// Columns of each title on the bar row: from, to.
fn spans(titles: &[Title], x0: u16) -> Vec<(u16, u16)> {
    let mut x = x0 + 1;
    titles
        .iter()
        .map(|t| {
            let w = t.text.width() as u16 + 2;
            let s = (x, x + w);
            x += w;
            s
        })
        .collect()
}

/// Columns the bar's titles take.
pub fn bar_width(kind: Kind) -> u16 {
    spans(&titles(kind), 0).last().map_or(0, |s| s.1) + 1
}

/// The title under a column of the bar row.
pub fn title_at(kind: Kind, x0: u16, x: u16) -> Option<usize> {
    spans(&titles(kind), x0).iter().position(|s| x >= s.0 && x < s.1)
}

/// The menu whose mnemonic is `c` (case aside).
pub fn by_mnemonic(titles: &[Title], c: char) -> Option<usize> {
    let c = c.to_lowercase().next()?;
    titles.iter().position(|t| t.mn.is_some_and(|m| m.0 == c))
}

/// Writes a label with its mnemonic underlined.
fn label(buf: &mut Buffer, x: u16, y: u16, s: &str, mn: Option<usize>, max: u16, style: Style) {
    let mut cx = x;
    for (i, ch) in s.chars().enumerate() {
        let w = ch.width().unwrap_or(0) as u16;
        if cx + w > x + max {
            break;
        }
        let st = if mn == Some(i) { style.add_modifier(Modifier::UNDERLINED | Modifier::BOLD) } else { style };
        let mut b = [0; 4];
        text(buf, cx, y, ch.encode_utf8(&mut b), w, st);
        cx += w;
    }
}

/// Draws the bar on `row` (filled over its whole width), `open` shown
/// pressed.
pub fn draw_bar(buf: &mut Buffer, row: Rect, kind: Kind, open: Option<usize>) {
    let base = Style::new().bg(BAR_BG).fg(BAR_FG);
    fill(buf, row, base);
    let ts = titles(kind);
    for (i, (t, s)) in ts.iter().zip(spans(&ts, row.x)).enumerate() {
        if s.1 > row.right() {
            break;
        }
        let st = if open == Some(i) { Style::new().bg(SEL_BG).fg(Color::Black) } else { base };
        fill(buf, Rect::new(s.0, row.y, s.1 - s.0, 1), st);
        label(buf, s.0 + 1, row.y, &t.text, t.mn.map(|m| m.1), s.1 - s.0 - 1, st);
    }
}

// --- an open menu ----------------------------------------------------------------------------

enum Act {
    Run(Cmd),
    /// Index into `Open::subs`.
    Sub(usize),
    Sep,
}

struct Row {
    label: String,
    keys: String,
    act: Act,
    enabled: bool,
    check: Option<bool>,
    /// Char index of the mnemonic in `label`.
    mn: Option<usize>,
}

impl Row {
    fn usable(&self) -> bool {
        self.enabled && !matches!(self.act, Act::Sep)
    }

    fn mnemonic(&self) -> Option<char> {
        let i = self.mn?;
        self.label.chars().nth(i).and_then(|c| c.to_lowercase().next())
    }
}

/// Gives each row a letter of its label no row before it has: word starts
/// first, then any letter or digit (only ASCII ones, which every keyboard
/// types).
fn assign_mnemonics(rows: &mut [Row]) {
    let mut used: Vec<char> = Vec::new();
    for r in rows.iter_mut().filter(|r| !matches!(r.act, Act::Sep)) {
        let chars: Vec<char> = r.label.chars().collect();
        let starts = (0..chars.len()).filter(|&i| i == 0 || !chars[i - 1].is_alphanumeric());
        let pick = starts.chain(0..chars.len()).find(|&i| chars[i].is_ascii_alphanumeric() && !used.contains(&chars[i].to_ascii_lowercase()));
        if let Some(i) = pick {
            used.push(chars[i].to_ascii_lowercase());
            r.mn = Some(i);
        }
    }
}

/// What the app says of a command: whether it applies now, and its
/// on/off state for those that are switches.
pub type State<'a> = &'a dyn Fn(&Cmd) -> (bool, Option<bool>);

fn entry_row(en: Entry, state: State) -> Row {
    let (enabled, check) = state(&en.cmd);
    Row { label: tr(en.label).to_string(), keys: en.keys.to_string(), act: Act::Run(en.cmd), enabled, check, mn: None }
}

/// The bar with one of its menus pulled down.
pub struct Open {
    titles: Vec<Title>,
    lists: Vec<Vec<Row>>,
    subs: Vec<Vec<Row>>,
    pub menu: usize,
    sel: Option<usize>,
    /// The open submenu: its index in `subs`, the row it hangs from and
    /// the selected row.
    sub: Option<(usize, usize, Option<usize>)>,
}

/// Where the open menus are drawn.
struct Geo {
    drop: Rect,
    /// First row shown (long menus scroll).
    top: usize,
    sub: Option<(Rect, usize)>,
}

impl Open {
    pub fn new(kind: Kind, menu: usize, state: State) -> Open {
        let mut subs = Vec::new();
        let mut lists = Vec::new();
        for m in menus(kind) {
            let mut rows = Vec::new();
            for it in m.items {
                rows.push(match it {
                    Item::Cmd(en) => entry_row(en, state),
                    Item::Sep => Row { label: String::new(), keys: String::new(), act: Act::Sep, enabled: false, check: None, mn: None },
                    Item::Sub(sm, keys) => {
                        let (t, entries) = cmd::menu(sm);
                        let mut sr: Vec<Row> = entries.into_iter().map(|en| entry_row(en, state)).collect();
                        assign_mnemonics(&mut sr);
                        subs.push(sr);
                        Row { label: tr(t).to_string(), keys: keys.to_string(), act: Act::Sub(subs.len() - 1), enabled: true, check: None, mn: None }
                    }
                });
            }
            assign_mnemonics(&mut rows);
            lists.push(rows);
        }
        let mut o = Open { titles: titles(kind), lists, subs, menu: 0, sel: None, sub: None };
        o.switch(menu);
        o
    }

    fn switch(&mut self, menu: usize) {
        self.menu = menu.min(self.lists.len() - 1);
        self.sub = None;
        self.sel = step(&self.lists[self.menu], None, 1);
    }

    /// The list keys act on: the submenu when one is open.
    fn current(&self) -> (&Vec<Row>, Option<usize>) {
        match self.sub {
            Some((i, _, sel)) => (&self.subs[i], sel),
            None => (&self.lists[self.menu], self.sel),
        }
    }

    fn set_sel(&mut self, v: Option<usize>) {
        match &mut self.sub {
            Some((.., sel)) => *sel = v,
            None => self.sel = v,
        }
    }

    /// Runs a row of the current list, or opens its submenu.
    fn activate(&mut self, i: usize) -> Outcome {
        let (rows, _) = self.current();
        let Some(r) = rows.get(i) else { return Outcome::Keep };
        if !r.usable() {
            return Outcome::Keep;
        }
        match &r.act {
            Act::Run(c) => Outcome::Run(c.clone()),
            Act::Sub(s) => {
                let s = *s;
                if self.sub.is_none() {
                    self.sel = Some(i);
                    self.sub = Some((s, i, step(&self.subs[s], None, 1)));
                }
                Outcome::Keep
            }
            Act::Sep => Outcome::Keep,
        }
    }

    pub fn key(&mut self, k: &KeyEvent) -> Outcome {
        let n = self.lists.len();
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        match k.code {
            KeyCode::Esc if self.sub.is_some() => self.sub = None,
            KeyCode::Esc | KeyCode::F(9) | KeyCode::F(10) => return Outcome::Close,
            KeyCode::Left if self.sub.is_some() => self.sub = None,
            KeyCode::Left => self.switch((self.menu + n - 1) % n),
            KeyCode::Right => {
                let on_sub = self.sub.is_none() && self.sel.is_some_and(|i| matches!(self.lists[self.menu][i].act, Act::Sub(_)));
                if on_sub {
                    return self.activate(self.sel.unwrap_or(0));
                }
                self.switch((self.menu + 1) % n);
            }
            KeyCode::Up | KeyCode::Down => {
                let (rows, sel) = self.current();
                let v = step(rows, sel, if k.code == KeyCode::Up { -1 } else { 1 });
                self.set_sel(v);
            }
            KeyCode::Home | KeyCode::End => {
                let (rows, _) = self.current();
                let v = if k.code == KeyCode::Home { step(rows, None, 1) } else { step(rows, None, -1) };
                self.set_sel(v);
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                if let (_, Some(i)) = self.current() {
                    return self.activate(i);
                }
            }
            KeyCode::Char(ch) if alt => {
                if let Some(i) = by_mnemonic(&self.titles, ch) {
                    self.switch(i);
                }
            }
            KeyCode::Char(ch) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                let ch = ch.to_lowercase().next().unwrap_or(ch);
                let (rows, _) = self.current();
                if let Some(i) = rows.iter().position(|r| r.usable() && r.mnemonic() == Some(ch)) {
                    return self.activate(i);
                }
            }
            _ => {}
        }
        Outcome::Keep
    }

    fn geometry(&self, area: Rect) -> Geo {
        let x = spans(&self.titles, area.x)[self.menu].0;
        let rows = &self.lists[self.menu];
        let drop = popup(rows, x, area.y + 1, area);
        let top = scroll(rows.len(), drop.height, self.sel);
        let sub = self.sub.map(|(s, at, sel)| {
            let sr = &self.subs[s];
            let (w, h) = size(sr, area);
            // Right of the menu, else left of it, else over it at the right.
            let mut sx = drop.right();
            if sx + w > area.right() {
                sx = if drop.x >= area.x + w { drop.x - w } else { area.right().saturating_sub(w) };
            }
            let y = (drop.y + at.saturating_sub(top) as u16).min(area.bottom().saturating_sub(h)).max(area.y + 1);
            let r = Rect::new(sx, y, w, h);
            (r, scroll(sr.len(), h, sel))
        });
        Geo { drop, top, sub }
    }

    pub fn mouse(&mut self, m: &MouseEvent, area: Rect) -> Outcome {
        let (x, y) = (m.column, m.row);
        let g = self.geometry(area);
        let inside = |r: Rect| x >= r.x && x < r.right() && y > r.y && y + 1 < r.bottom();
        let hit_title = if y == area.y { spans(&self.titles, area.x).iter().position(|s| x >= s.0 && x < s.1) } else { None };
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if y == area.y {
                    return match hit_title {
                        Some(i) if i != self.menu => {
                            self.switch(i);
                            Outcome::Keep
                        }
                        _ => Outcome::Close,
                    };
                }
                if let Some((r, top)) = g.sub
                    && inside(r)
                {
                    return self.activate(top + (y - r.y - 1) as usize);
                }
                if inside(g.drop) {
                    let i = g.top + (y - g.drop.y - 1) as usize;
                    // Another row of the menu closes the submenu first.
                    if self.sub.is_some_and(|s| s.1 != i) {
                        self.sub = None;
                    }
                    if self.sub.is_some() {
                        return Outcome::Keep;
                    }
                    return self.activate(i);
                }
                if g.sub.is_some_and(|(r, _)| x >= r.x && x < r.right() && y >= r.y && y < r.bottom()) || (x >= g.drop.x && x < g.drop.right() && y >= g.drop.y && y < g.drop.bottom()) {
                    // The borders.
                    return Outcome::Keep;
                }
                Outcome::Close
            }
            MouseEventKind::Moved => {
                if let Some(i) = hit_title
                    && i != self.menu
                {
                    self.switch(i);
                } else if let Some((r, top)) = g.sub
                    && inside(r)
                {
                    let i = top + (y - r.y - 1) as usize;
                    if self.subs[self.sub.map_or(0, |s| s.0)].get(i).is_some_and(Row::usable) {
                        self.set_sel(Some(i));
                    }
                } else if self.sub.is_none() && inside(g.drop) {
                    let i = g.top + (y - g.drop.y - 1) as usize;
                    if self.lists[self.menu].get(i).is_some_and(Row::usable) {
                        self.sel = Some(i);
                    }
                }
                Outcome::Keep
            }
            _ => Outcome::Keep,
        }
    }

    /// Draws the pulled-down menu (the bar row is drawn by the app).
    pub fn draw(&self, f: &mut Frame) {
        let area = f.area();
        let g = self.geometry(area);
        draw_list(f.buffer_mut(), g.drop, &self.lists[self.menu], g.top, self.sel, self.sub.is_none());
        if let Some((r, top)) = g.sub {
            let (s, _, sel) = self.sub.unwrap_or((0, 0, None));
            draw_list(f.buffer_mut(), r, &self.subs[s], top, sel, true);
        }
    }
}

/// The next usable row from `from` in a direction, wrapping.
fn step(rows: &[Row], from: Option<usize>, dir: i32) -> Option<usize> {
    let n = rows.len() as i32;
    if n == 0 {
        return None;
    }
    let mut i = from.map_or(if dir > 0 { -1 } else { n }, |f| f as i32);
    for _ in 0..n {
        i = (i + dir).rem_euclid(n);
        if rows[i as usize].usable() {
            return Some(i as usize);
        }
    }
    from
}

/// Width and height of a menu box (with its border), within the area.
fn size(rows: &[Row], area: Rect) -> (u16, u16) {
    let lw = rows.iter().map(|r| r.label.width()).max().unwrap_or(0) as u16;
    let kw = rows.iter().map(|r| r.keys.width() + if matches!(r.act, Act::Sub(_)) { 2 } else { 0 }).max().unwrap_or(0) as u16;
    let w = (lw + kw + 8).min(area.width);
    let h = (rows.len() as u16 + 2).min(area.height.saturating_sub(1));
    (w, h)
}

fn popup(rows: &[Row], x: u16, y: u16, area: Rect) -> Rect {
    let (w, h) = size(rows, area);
    let x = x.min(area.right().saturating_sub(w));
    Rect::new(x, y, w, h)
}

/// First row shown so that the selected one fits.
fn scroll(len: usize, h: u16, sel: Option<usize>) -> usize {
    let room = (h as usize).saturating_sub(2).max(1);
    sel.map_or(0, |s| (s + 1).saturating_sub(room)).min(len.saturating_sub(room))
}

fn draw_list(buf: &mut Buffer, r: Rect, rows: &[Row], top: usize, sel: Option<usize>, focus: bool) {
    if r.width < 4 || r.height < 3 {
        return;
    }
    Clear.render(r, buf);
    let base = Style::new().bg(BAR_BG).fg(BAR_FG);
    fill(buf, r, base);
    let border = base.fg(OFF_FG);
    // Box: ┌─┐ │ │ └─┘.
    let line = |buf: &mut Buffer, y: u16, l: &str, m: &str, rr: &str| {
        text(buf, r.x, y, l, 1, border);
        for x in r.x + 1..r.right() - 1 {
            text(buf, x, y, m, 1, border);
        }
        text(buf, r.right() - 1, y, rr, 1, border);
    };
    line(buf, r.y, "┌", "─", "┐");
    line(buf, r.bottom() - 1, "└", "─", "┘");
    let inner = r.width - 2;
    for (k, row) in rows.iter().enumerate().skip(top).take(r.height as usize - 2) {
        let y = r.y + 1 + (k - top) as u16;
        if matches!(row.act, Act::Sep) {
            line(buf, y, "├", "─", "┤");
            continue;
        }
        text(buf, r.x, y, "│", 1, border);
        text(buf, r.right() - 1, y, "│", 1, border);
        let mut st = if !row.enabled { base.fg(OFF_FG) } else { base };
        if sel == Some(k) {
            st = if focus { Style::new().bg(SEL_BG).fg(Color::Black) } else { base.bg(Color::Rgb(0x3F, 0x3F, 0x46)) };
            if !row.enabled {
                st = st.fg(OFF_FG);
            }
        }
        fill(buf, Rect::new(r.x + 1, y, inner, 1), st);
        if row.check == Some(true) {
            text(buf, r.x + 2, y, "✓", 1, st);
        }
        let mut keys = row.keys.clone();
        if matches!(row.act, Act::Sub(_)) {
            keys = if keys.is_empty() { "›".into() } else { format!("{keys} ›") };
        }
        let kw = keys.width() as u16;
        let lw = inner.saturating_sub(kw + 5);
        let lab = fit(&row.label, lw as usize);
        let mn = if lab == row.label && row.enabled { row.mn } else { None };
        label(buf, r.x + 4, y, &lab, mn, lw, st);
        if kw > 0 && kw + 5 < inner {
            let kst = if row.enabled && !matches!(row.act, Act::Sub(_)) { st.add_modifier(Modifier::DIM) } else { st };
            text(buf, r.right() - 2 - kw, y, &keys, kw, kst);
        }
    }
}

#[cfg(test)]
pub(crate) fn mnemonics(kind: Kind) -> Vec<Option<char>> {
    titles(kind).iter().map(|t| t.mn.map(|m| m.0)).collect()
}

#[cfg(test)]
pub(crate) fn entry_mnemonics(o: &Open) -> Vec<Vec<char>> {
    o.lists.iter().chain(&o.subs).map(|l| l.iter().filter_map(Row::mnemonic).collect()).collect()
}

#[cfg(test)]
pub(crate) fn all_cmds(kind: Kind) -> Vec<Cmd> {
    let mut v = Vec::new();
    for m in menus(kind) {
        for it in m.items {
            match it {
                Item::Cmd(en) => v.push(en.cmd),
                Item::Sub(sm, _) => {
                    v.push(Cmd::Menu(sm));
                    v.extend(cmd::menu(sm).1.into_iter().map(|en| en.cmd));
                }
                Item::Sep => {}
            }
        }
    }
    v
}
