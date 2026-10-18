//! Workbooks: the cell cursor and selection, editing in the formula bar
//! (with function autocomplete), and the commands that change the book.
//! Every change goes through the workbook's undoable operations.

use std::sync::Arc;

use apex_i18n::{functions, lookup, tr, trf};
use apex_sheet::data::{AutoFilter, ColumnFilter, FilterKind};
use apex_sheet::formula_text;
use apex_sheet::{CellRef, Clip, MAX_COLS, MAX_ROWS, Op, Range, Sheet, Value, Workbook};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use crate::cmd::{Cmd, Menu};
use crate::ui::{Ask, Choice, Item, LineEdit, Purpose};

const LAST: CellRef = CellRef { row: MAX_ROWS - 1, col: MAX_COLS - 1 };

/// Width of a column in terminal cells, from its width in pixels (Excel's
/// character count, plus one for the gap).
pub fn col_chars(px: f32) -> u16 {
    if px <= 0.0 {
        return 0;
    }
    let chars = (px / (88.0 / 64.0) - 5.0) / 7.0;
    (chars.round().max(1.0) as u16 + 1).min(80)
}

/// Pixels for a column `chars` terminal cells wide.
pub fn col_px(chars: u16) -> f32 {
    ((chars.saturating_sub(1) as f32 * 7.0 + 5.0).round() * (88.0 / 64.0)).max(0.0)
}

/// A cell being typed.
pub struct CellEdit {
    pub line: LineEdit,
    /// Started by typing over the cell: arrow keys enter it and move.
    pub typed: bool,
    /// Function names offered for the word being typed, and the one picked.
    pub ac: Vec<&'static str>,
    pub ac_sel: usize,
}

/// Where things were drawn, for the mouse and paging.
#[derive(Default)]
pub struct Geo {
    pub grid: Rect,
    /// Visible rows and their screen y.
    pub rows: Vec<(u32, u16)>,
    /// Visible columns: index, screen x and width.
    pub cols: Vec<(u32, u16, u16)>,
    pub header_y: u16,
    pub row_head_w: u16,
    pub formula_y: u16,
    /// Where the formula bar's text starts.
    pub formula_x: u16,
    /// Sheet tabs: sheet, x from, x to; and the "+" tab.
    pub tabs: Vec<(usize, u16, u16)>,
    pub add_tab: Option<(u16, u16)>,
    pub tabs_y: u16,
}

pub struct SheetUi {
    pub wb: Workbook,
    pub sheet: usize,
    pub cursor: CellRef,
    pub anchor: CellRef,
    /// First scrolled row and column (below and right of frozen panes).
    pub top: u32,
    pub left: u32,
    pub edit: Option<CellEdit>,
    /// Copied cells, their sheet, and whether they were cut.
    clip: Option<(Clip, usize, bool)>,
    pub(crate) last_find: Option<String>,
    pub show_formulas: bool,
    pub geo: Geo,
    /// Screen position of the caret while typing.
    pub caret: (u16, u16),
    drag: bool,
    pub message: Option<(String, bool)>,
    pub clipboard_out: Option<String>,
}

impl SheetUi {
    pub fn new(mut wb: Workbook) -> SheetUi {
        wb.enforce_protection = true;
        if wb.needs_calc() {
            wb.recalc_all();
        }
        let sheet = wb.sheets.iter().position(|s| !s.hidden).unwrap_or(0);
        let mut s = SheetUi {
            wb,
            sheet,
            cursor: CellRef::new(0, 0),
            anchor: CellRef::new(0, 0),
            top: 0,
            left: 0,
            edit: None,
            clip: None,
            last_find: None,
            show_formulas: false,
            geo: Geo::default(),
            caret: (0, 0),
            drag: false,
            message: None,
            clipboard_out: None,
        };
        s.select_sheet(sheet);
        s
    }

    pub fn sh(&self) -> &Sheet {
        &self.wb.sheets[self.sheet]
    }

    pub fn sel(&self) -> Range {
        Range::new(self.anchor, self.cursor)
    }

    fn info(&mut self, s: impl Into<String>) {
        self.message = Some((s.into(), false));
    }

    fn error(&mut self, s: impl Into<String>) {
        self.message = Some((s.into(), true));
    }

    /// Reports a change the sheet protection refused.
    fn check_blocked(&mut self) {
        if self.wb.take_blocked() {
            self.error(tr("tui.protected"));
        }
    }

    pub fn row_hidden(&self, r: u32) -> bool {
        self.sh().row_height(r) <= 0.0
    }

    pub fn col_width(&self, c: u32) -> u16 {
        col_chars(self.sh().col_width(c))
    }

    fn merge_at(&self, c: CellRef) -> Option<Range> {
        self.sh().merges.iter().find(|m| m.contains(c)).copied()
    }

    fn select_sheet(&mut self, i: usize) {
        self.commit_edit();
        self.sheet = i.min(self.wb.sheets.len() - 1);
        let (fr, fc) = (self.sh().frozen_rows, self.sh().frozen_cols);
        self.top = fr;
        self.left = fc;
        self.cursor = CellRef::new(fr, fc);
        self.anchor = self.cursor;
        self.cursor = self.cursor_visible(self.cursor);
        self.anchor = self.cursor;
    }

    /// The first visible cell at or after `c`.
    fn cursor_visible(&self, mut c: CellRef) -> CellRef {
        while self.row_hidden(c.row) && c.row < LAST.row {
            c.row += 1;
        }
        while self.col_width(c.col) == 0 && c.col < LAST.col {
            c.col += 1;
        }
        c
    }

    // --- moving ------------------------------------------------------------------------

    pub fn move_to(&mut self, c: CellRef, extend: bool) {
        self.cursor = CellRef::new(c.row.min(LAST.row), c.col.min(LAST.col));
        if !extend {
            self.anchor = self.cursor;
        }
        self.ensure_visible();
    }

    /// One visible row or column in a direction, over merged areas.
    fn step(&self, from: CellRef, dr: i32, dc: i32) -> CellRef {
        let mut c = from;
        if let Some(m) = self.merge_at(c) {
            if dc > 0 {
                c.col = m.end.col;
            }
            if dr > 0 {
                c.row = m.end.row;
            }
            if dc < 0 {
                c.col = m.start.col;
            }
            if dr < 0 {
                c.row = m.start.row;
            }
        }
        let start = c;
        loop {
            let r = c.row as i64 + dr as i64;
            let k = c.col as i64 + dc as i64;
            if r < 0 || k < 0 || r > LAST.row as i64 || k > LAST.col as i64 {
                return if c == start { from } else { c };
            }
            c = CellRef::new(r as u32, k as u32);
            if (dr == 0 || !self.row_hidden(c.row)) && (dc == 0 || self.col_width(c.col) > 0) {
                break;
            }
        }
        match self.merge_at(c) {
            Some(m) if !m.contains(from) => m.start,
            _ => c,
        }
    }

    fn arrow(&mut self, dr: i32, dc: i32, extend: bool, jump: bool) {
        let to = if jump { self.sh().jump(self.cursor, dr, dc, LAST) } else { self.step(self.cursor, dr, dc) };
        self.move_to(to, extend);
    }

    /// Rows that fit in the grid.
    fn page_rows(&self) -> u32 {
        (self.geo.grid.height as u32).max(2) - 1
    }

    fn page(&mut self, down: bool, extend: bool) {
        let n = self.page_rows();
        let mut c = self.cursor;
        for _ in 0..n {
            c = self.step(c, if down { 1 } else { -1 }, 0);
        }
        let mut t = self.top;
        for _ in 0..n {
            t = if down { self.next_row(t) } else { self.prev_row(t) };
        }
        self.top = t.max(self.sh().frozen_rows);
        self.move_to(c, extend);
    }

    fn next_row(&self, r: u32) -> u32 {
        let mut n = r + 1;
        while n < LAST.row && self.row_hidden(n) {
            n += 1;
        }
        n.min(LAST.row)
    }

    fn prev_row(&self, r: u32) -> u32 {
        let mut n = r.saturating_sub(1);
        while n > 0 && self.row_hidden(n) {
            n -= 1;
        }
        n
    }

    /// Scrolls so that the cursor shows.
    pub fn ensure_visible(&mut self) {
        if self.geo.grid.height == 0 {
            // Not drawn yet: the first frame scrolls.
            return;
        }
        let sh = self.sh();
        let (fr, fc) = (sh.frozen_rows, sh.frozen_cols);
        let c = self.cursor;
        if c.row >= fr {
            if c.row < self.top {
                self.top = c.row;
            } else {
                // Rows of the scrolled part that fit.
                let frozen_h = (0..fr).filter(|&r| !self.row_hidden(r)).count() as u32;
                let room = (self.geo.grid.height as u32).saturating_sub(frozen_h).max(1);
                let mut n = 0;
                let mut r = c.row;
                let mut first = c.row;
                while r >= self.top {
                    if !self.row_hidden(r) {
                        n += 1;
                        if n > room {
                            break;
                        }
                        first = r;
                    }
                    if r == 0 {
                        break;
                    }
                    r -= 1;
                }
                if n > room {
                    self.top = first;
                }
            }
        }
        if c.col >= fc {
            if c.col < self.left {
                self.left = c.col;
            } else {
                let frozen_w: u32 = (0..fc).map(|k| self.col_width(k) as u32).sum();
                let room = (self.geo.grid.width as u32).saturating_sub(self.geo.row_head_w as u32 + frozen_w).max(1);
                let mut used = 0;
                let mut k = c.col;
                let mut first = c.col;
                loop {
                    used += self.col_width(k) as u32;
                    if used > room && k != c.col {
                        break;
                    }
                    first = k;
                    if k == self.left || k == 0 {
                        break;
                    }
                    k -= 1;
                }
                if first > self.left {
                    self.left = first;
                }
            }
        }
    }

    fn last_used(&self) -> CellRef {
        self.sh().extent().unwrap_or_default()
    }

    // --- keys --------------------------------------------------------------------------

    pub fn key(&mut self, k: KeyEvent) -> Option<Cmd> {
        if self.edit.is_some() {
            return self.edit_key(k);
        }
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        match k.code {
            KeyCode::Down if alt => return Some(Cmd::FilterMenu),
            KeyCode::Left if alt => return Some(Cmd::ColNarrower),
            KeyCode::Right if alt => return Some(Cmd::ColWider),
            KeyCode::Up => self.arrow(-1, 0, shift, ctrl),
            KeyCode::Down => self.arrow(1, 0, shift, ctrl),
            KeyCode::Left => self.arrow(0, -1, shift, ctrl),
            KeyCode::Right => self.arrow(0, 1, shift, ctrl),
            KeyCode::Tab => self.arrow(0, 1, false, false),
            KeyCode::BackTab => self.arrow(0, -1, false, false),
            KeyCode::PageDown if ctrl => return Some(Cmd::NextSheet),
            KeyCode::PageUp if ctrl => return Some(Cmd::PrevSheet),
            KeyCode::PageDown => self.page(true, shift),
            KeyCode::PageUp => self.page(false, shift),
            KeyCode::Home if ctrl => self.move_to(self.cursor_visible(CellRef::new(0, 0)), shift),
            KeyCode::End if ctrl => self.move_to(self.last_used(), shift),
            KeyCode::Home => self.move_to(CellRef::new(self.cursor.row, 0), shift),
            KeyCode::End => {
                let last = self.last_used();
                self.move_to(CellRef::new(self.cursor.row, last.col), shift)
            }
            KeyCode::Enter => {
                if shift {
                    self.arrow(-1, 0, false, false)
                } else {
                    return Some(Cmd::EditCell);
                }
            }
            KeyCode::F(2) => return Some(Cmd::EditCell),
            KeyCode::F(5) => return Some(Cmd::Ask(Ask::GoTo)),
            KeyCode::F(9) if shift => return Some(Cmd::Recalc),
            KeyCode::F(11) if shift => return Some(Cmd::AddSheet),
            KeyCode::Delete => return Some(Cmd::ClearContents),
            KeyCode::Backspace => {
                self.run(Cmd::ClearContents);
                self.start_edit(String::new(), true);
            }
            KeyCode::Char(c) if ctrl => {
                return match c.to_ascii_lowercase() {
                    'c' => Some(Cmd::Copy),
                    'x' => Some(Cmd::Cut),
                    'v' => Some(Cmd::Paste),
                    'a' => Some(Cmd::SelectAll),
                    'b' => Some(Cmd::Bold),
                    'i' => Some(Cmd::Italic),
                    'u' => Some(Cmd::Underline),
                    'g' => Some(Cmd::Ask(Ask::GoTo)),
                    '`' => Some(Cmd::ShowFormulas),
                    '+' => Some(Cmd::Menu(Menu::Insert)),
                    '-' => Some(Cmd::Menu(Menu::Delete)),
                    _ => None,
                };
            }
            KeyCode::Char(c) if alt => {
                return match c {
                    'r' => Some(Cmd::InsertRows),
                    'R' => Some(Cmd::DeleteRows),
                    'c' => Some(Cmd::InsertCols),
                    'C' => Some(Cmd::DeleteCols),
                    'l' | 'L' => Some(Cmd::AutoFilter),
                    'i' | 'I' => Some(Cmd::Italic),
                    's' | 'S' => Some(Cmd::Strike),
                    _ => None,
                };
            }
            KeyCode::Char(c) => {
                let mut b = [0; 4];
                self.start_edit(c.encode_utf8(&mut b).to_string(), true);
            }
            _ => {}
        }
        None
    }

    fn start_edit(&mut self, text: String, typed: bool) {
        self.anchor = self.cursor;
        self.edit = Some(CellEdit { line: LineEdit::new(&text), typed, ac: Vec::new(), ac_sel: 0 });
        self.update_ac();
        self.ensure_visible();
    }

    /// Autocomplete for the function name before the caret.
    fn update_ac(&mut self) {
        let Some(e) = &mut self.edit else { return };
        let before = &e.line.text[..e.line.caret];
        let after_ok = !e.line.text[e.line.caret..].starts_with(|c: char| c.is_alphanumeric() || c == '(');
        e.ac = match formula_text::word_at_end(before) {
            Some(w) if after_ok => functions::complete(w).into_iter().map(|(n, _)| n).take(8).collect(),
            _ => Vec::new(),
        };
        e.ac_sel = 0;
    }

    fn accept_ac(&mut self) {
        let Some(e) = &mut self.edit else { return };
        let Some(&name) = e.ac.get(e.ac_sel) else { return };
        let before = &e.line.text[..e.line.caret];
        let Some(w) = formula_text::word_at_end(before) else { return };
        let start = e.line.caret - w.len();
        e.line.text.replace_range(start..e.line.caret, &format!("{name}("));
        e.line.caret = start + name.len() + 1;
        e.ac.clear();
    }

    fn edit_key(&mut self, k: KeyEvent) -> Option<Cmd> {
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let e = self.edit.as_mut()?;
        let ac_open = !e.ac.is_empty();
        match k.code {
            KeyCode::Esc if ac_open => e.ac.clear(),
            KeyCode::Esc => self.edit = None,
            KeyCode::Tab if ac_open => self.accept_ac(),
            KeyCode::Up if ac_open => e.ac_sel = e.ac_sel.checked_sub(1).unwrap_or(e.ac.len() - 1),
            KeyCode::Down if ac_open => e.ac_sel = (e.ac_sel + 1) % e.ac.len(),
            KeyCode::Enter => {
                if self.try_commit() {
                    self.arrow(if shift { -1 } else { 1 }, 0, false, false);
                }
            }
            KeyCode::Tab | KeyCode::BackTab => {
                if self.try_commit() {
                    self.arrow(0, if shift || k.code == KeyCode::BackTab { -1 } else { 1 }, false, false);
                }
            }
            KeyCode::Up | KeyCode::Down if e.typed || !e.line.text.starts_with('=') => {
                if self.try_commit() {
                    self.arrow(if k.code == KeyCode::Up { -1 } else { 1 }, 0, false, false);
                }
            }
            KeyCode::Left | KeyCode::Right if e.typed && !e.line.text.starts_with('=') => {
                if self.try_commit() {
                    self.arrow(0, if k.code == KeyCode::Left { -1 } else { 1 }, false, false);
                }
            }
            KeyCode::F(2) => e.typed = false,
            _ => {
                if e.line.key(&k) {
                    self.update_ac();
                }
            }
        }
        None
    }

    /// Enters the cell being typed; false (with a message) for a formula
    /// that doesn't parse.
    fn try_commit(&mut self) -> bool {
        let Some(e) = &self.edit else { return true };
        if let Err(msg) = self.wb.check_formula(&e.line.text) {
            self.error(trf("tui.bad_formula", &[("error", &msg)]));
            return false;
        }
        self.commit_edit();
        true
    }

    /// Enters the cell being typed as it is.
    pub fn commit_edit(&mut self) {
        let Some(e) = self.edit.take() else { return };
        if e.line.text != self.wb.edit_text(self.sheet, self.cursor) {
            self.wb.set_input(self.sheet, self.cursor, &e.line.text);
            self.check_blocked();
        }
    }

    // --- mouse -------------------------------------------------------------------------

    fn cell_at(&self, x: u16, y: u16) -> Option<CellRef> {
        let row = self.geo.rows.iter().find(|r| r.1 == y)?.0;
        let col = self.geo.cols.iter().find(|c| x >= c.1 && x < c.1 + c.2)?.0;
        Some(CellRef::new(row, col))
    }

    pub fn mouse(&mut self, m: MouseEvent) -> Option<Cmd> {
        let (x, y) = (m.column, m.row);
        let shift = m.modifiers.contains(KeyModifiers::SHIFT);
        match m.kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp if shift => {
                let fc = self.sh().frozen_cols;
                self.left = if m.kind == MouseEventKind::ScrollDown { (self.left + 3).min(LAST.col) } else { self.left.saturating_sub(3).max(fc) };
            }
            MouseEventKind::ScrollDown => {
                for _ in 0..3 {
                    self.top = self.next_row(self.top);
                }
            }
            MouseEventKind::ScrollUp => {
                let fr = self.sh().frozen_rows;
                for _ in 0..3 {
                    self.top = self.prev_row(self.top).max(fr);
                }
            }
            MouseEventKind::ScrollRight => self.left = (self.left + 3).min(LAST.col),
            MouseEventKind::ScrollLeft => self.left = self.left.saturating_sub(3).max(self.sh().frozen_cols),
            MouseEventKind::Down(MouseButton::Left) => {
                if y == self.geo.tabs_y {
                    if let Some(&(i, ..)) = self.geo.tabs.iter().find(|t| x >= t.1 && x < t.2) {
                        self.select_sheet(i);
                    } else if self.geo.add_tab.is_some_and(|(a, b)| x >= a && x < b) {
                        return Some(Cmd::AddSheet);
                    }
                    return None;
                }
                if y == self.geo.formula_y {
                    if self.edit.is_none() {
                        return Some(Cmd::EditCell);
                    }
                    return None;
                }
                self.commit_edit();
                if y == self.geo.header_y {
                    // A column header selects the column.
                    if let Some(&(c, ..)) = self.geo.cols.iter().find(|c| x >= c.1 && x < c.1 + c.2) {
                        self.anchor = CellRef::new(0, c);
                        self.cursor = CellRef::new(LAST.row, c);
                    }
                    return None;
                }
                if x < self.geo.grid.x + self.geo.row_head_w {
                    if let Some(&(r, _)) = self.geo.rows.iter().find(|r| r.1 == y) {
                        self.anchor = CellRef::new(r, 0);
                        self.cursor = CellRef::new(r, LAST.col);
                    }
                    return None;
                }
                let c = self.cell_at(x, y)?;
                self.move_to(c, shift);
                self.drag = true;
                // The filter button at the right end of a header cell.
                if let Some(f) = &self.sh().filter
                    && c.row == f.range.start.row
                    && (f.range.start.col..=f.range.end.col).contains(&c.col)
                    && self.geo.cols.iter().any(|k| k.0 == c.col && x + 1 == k.1 + k.2)
                {
                    return Some(Cmd::FilterMenu);
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if self.drag => {
                if let Some(c) = self.cell_at(x, y) {
                    self.cursor = c;
                }
            }
            MouseEventKind::Up(_) => self.drag = false,
            _ => {}
        }
        None
    }

    // --- commands ----------------------------------------------------------------------

    pub fn run(&mut self, cmd: Cmd) {
        if !matches!(cmd, Cmd::Undo | Cmd::Redo) {
            self.commit_edit();
        }
        let sheet = self.sheet;
        let sel = self.sel();
        match cmd {
            Cmd::EditCell => {
                let t = self.wb.edit_text(sheet, self.cursor);
                self.start_edit(t, false);
            }
            Cmd::Undo => {
                self.edit = None;
                let label = self.wb.undo_history().last().copied();
                if self.wb.undo() {
                    self.info(trf("tui.undone", &[("what", &label.and_then(lookup).unwrap_or(""))]));
                }
                self.clamp_sheet();
            }
            Cmd::Redo => {
                self.edit = None;
                let label = self.wb.redo_history().last().copied();
                if self.wb.redo() {
                    self.info(trf("tui.redone", &[("what", &label.and_then(lookup).unwrap_or(""))]));
                }
                self.clamp_sheet();
            }
            Cmd::Copy | Cmd::Cut => self.copy(cmd == Cmd::Cut),
            Cmd::Paste => self.paste(),
            Cmd::SelectAll => {
                self.anchor = CellRef::new(0, 0);
                self.cursor = self.last_used();
            }
            Cmd::ClearContents => self.wb.clear_contents(sheet, &sel),
            Cmd::Bold | Cmd::Italic | Cmd::Underline | Cmd::Strike => {
                let st = self.wb.styles.get(self.wb.cell(sheet, self.cursor).map_or(0, |c| c.style)).clone();
                match cmd {
                    Cmd::Bold => self.wb.modify_style(sheet, &sel, |s| s.bold = !st.bold),
                    Cmd::Italic => self.wb.modify_style(sheet, &sel, |s| s.italic = !st.italic),
                    Cmd::Underline => self.wb.modify_style(sheet, &sel, |s| s.underline = !st.underline),
                    _ => self.wb.modify_style(sheet, &sel, |s| s.strike = !st.strike),
                }
            }
            Cmd::NumFormat(f) => self.wb.modify_style(sheet, &sel, |s| s.num_fmt = Arc::from(f)),
            Cmd::Align(a) => self.wb.modify_style(sheet, &sel, |s| s.halign = a),
            Cmd::TextColor(c) => self.wb.modify_style(sheet, &sel, |s| {
                s.color = c;
                s.theme.color = None;
            }),
            Cmd::FillColor(c) => self.wb.modify_style(sheet, &sel, |s| {
                s.fill = c;
                s.theme.fill = None;
            }),
            Cmd::InsertRows => self.wb.execute(Op::InsertRows { sheet, at: sel.start.row, count: sel.rows().min(10_000) }),
            Cmd::DeleteRows => self.wb.execute(Op::DeleteRows { sheet, at: sel.start.row, count: sel.rows() }),
            Cmd::InsertCols => self.wb.execute(Op::InsertCols { sheet, at: sel.start.col, count: sel.cols().min(1_000) }),
            Cmd::DeleteCols => self.wb.execute(Op::DeleteCols { sheet, at: sel.start.col, count: sel.cols() }),
            Cmd::SortAsc | Cmd::SortDesc => self.sort(cmd == Cmd::SortAsc),
            Cmd::AutoFilter => {
                if self.sh().filter.is_some() {
                    self.wb.set_filter(sheet, None, "undo.filter");
                } else {
                    let r = if sel.is_single() { self.wb.current_region(sheet, self.cursor) } else { sel };
                    self.wb.set_filter(sheet, Some(AutoFilter::new(r)), "undo.filter");
                }
            }
            Cmd::ColWider | Cmd::ColNarrower => {
                let sizes = (sel.start.col..=sel.end.col.min(sel.start.col + 1000))
                    .map(|c| {
                        let w = self.col_width(c) as i32 + if cmd == Cmd::ColWider { 1 } else { -1 };
                        (c, Some(col_px(w.clamp(2, 80) as u16)))
                    })
                    .collect();
                self.wb.execute(Op::ColWidths { sheet, sizes });
            }
            Cmd::AddSheet => {
                let name = self.wb.unique_sheet_name(&tr("sheet.default_name").replace("{n}", ""));
                let index = self.sheet + 1;
                self.wb.execute(Op::AddSheet { index, sheet: Box::new(Sheet::new(name)) });
                if self.wb.sheets.len() > index {
                    self.select_sheet(index);
                }
            }
            Cmd::DeleteSheet => {
                if self.wb.sheets.iter().filter(|s| !s.hidden).count() > 1 {
                    self.wb.execute(Op::RemoveSheet { index: sheet });
                    let i = self.sheet.min(self.wb.sheets.len() - 1);
                    self.select_sheet(i);
                    self.skip_hidden(-1);
                } else {
                    self.error(tr("tui.last_sheet"));
                }
            }
            Cmd::NextSheet => self.cycle_sheet(1),
            Cmd::PrevSheet => self.cycle_sheet(-1),
            Cmd::Recalc => {
                self.wb.recalc_all();
                self.info(tr("tui.recalculated"));
            }
            Cmd::Freeze => {
                let sh = self.sh();
                let (rows, cols) = if sh.frozen_rows > 0 || sh.frozen_cols > 0 { (0, 0) } else { (self.cursor.row, self.cursor.col) };
                self.wb.execute(Op::Freeze { sheet, rows, cols, top: CellRef::new(0, 0) });
                self.top = rows;
                self.left = cols;
            }
            Cmd::ShowFormulas => self.show_formulas = !self.show_formulas,
            Cmd::FindNext => match self.last_find.clone() {
                Some(t) => self.find(&t),
                None => {}
            },
            _ => {}
        }
        self.check_blocked();
        self.clamp_sheet();
        self.ensure_visible();
    }

    fn clamp_sheet(&mut self) {
        if self.sheet >= self.wb.sheets.len() {
            self.sheet = self.wb.sheets.len() - 1;
        }
    }

    fn skip_hidden(&mut self, dir: i32) {
        let n = self.wb.sheets.len() as i32;
        let mut i = self.sheet as i32;
        for _ in 0..n {
            if !self.wb.sheets[i as usize].hidden {
                break;
            }
            i = (i + dir).rem_euclid(n);
        }
        if i as usize != self.sheet {
            self.select_sheet(i as usize);
        }
    }

    fn cycle_sheet(&mut self, dir: i32) {
        let n = self.wb.sheets.len() as i32;
        let mut i = self.sheet as i32;
        for _ in 0..n {
            i = (i + dir).rem_euclid(n);
            if !self.wb.sheets[i as usize].hidden {
                break;
            }
        }
        self.select_sheet(i as usize);
    }

    fn copy(&mut self, cut: bool) {
        let sel = self.sel();
        let Some(used) = self.wb.clamp_to_used(self.sheet, &sel) else {
            self.clip = Some((self.wb.copy(self.sheet, &Range::single(sel.start)), self.sheet, cut));
            return;
        };
        let r = Range::new(sel.start, used.end);
        if r.cell_count() > 2_000_000 {
            self.error(tr("tui.too_big"));
            return;
        }
        let mut text = String::new();
        for row in r.start.row..=r.end.row {
            for col in r.start.col..=r.end.col {
                if col > r.start.col {
                    text.push('\t');
                }
                let (s, _) = self.wb.display(self.sheet, CellRef::new(row, col));
                if s.contains(['\t', '\n', '"']) {
                    text.push('"');
                    text.push_str(&s.replace('"', "\"\""));
                    text.push('"');
                } else {
                    text.push_str(&s);
                }
            }
            text.push('\n');
        }
        self.clip = Some((self.wb.copy(self.sheet, &r), self.sheet, cut));
        self.clipboard_out = Some(text);
        self.info(trf(if cut { "tui.cut_cells" } else { "tui.copied_cells" }, &[("n", &r.cell_count())]));
    }

    fn paste(&mut self) {
        let Some((clip, src_sheet, cut)) = self.clip.clone() else {
            self.info(tr("tui.paste_hint"));
            return;
        };
        let target = self.sel();
        let before = self.wb.undo_len();
        self.wb.paste(self.sheet, &target, &clip);
        if cut && clip.rows > 0 && clip.cols > 0 {
            // Clear the source cells that were not overwritten.
            let src = Range::new(clip.origin, CellRef::new(clip.origin.row + clip.rows - 1, clip.origin.col + clip.cols - 1));
            let dst = Range::new(target.start, CellRef::new(target.start.row + clip.rows - 1, target.start.col + clip.cols - 1));
            let mut cells = Vec::new();
            for row in src.start.row..=src.end.row {
                for col in src.start.col..=src.end.col {
                    let c = CellRef::new(row, col);
                    if (src_sheet != self.sheet || !dst.contains(c)) && self.wb.cell(src_sheet, c).is_some() {
                        cells.push((c, None));
                    }
                }
            }
            if !cells.is_empty() {
                self.wb.execute(Op::SetCells { sheet: src_sheet, cells });
            }
            self.clip = None;
        }
        let n = self.wb.undo_len() - before;
        if n > 1 {
            self.wb.merge_undo(n, "undo.paste");
        }
        let end = CellRef::new((target.start.row + clip.rows.max(1) - 1).max(target.end.row), (target.start.col + clip.cols.max(1) - 1).max(target.end.col));
        self.anchor = target.start;
        self.cursor = CellRef::new(end.row.min(LAST.row), end.col.min(LAST.col));
    }

    /// Text pasted from the terminal (the system clipboard): into the cell
    /// being typed, else tab-separated values from the cursor.
    pub fn paste_text(&mut self, text: &str) {
        if let Some(e) = &mut self.edit {
            e.line.insert(text.lines().next().unwrap_or(""));
            self.update_ac();
            return;
        }
        let start = self.sel().start;
        let mut inputs = Vec::new();
        for (dr, line) in text.lines().enumerate() {
            for (dc, v) in line.split('\t').enumerate() {
                let c = CellRef::new(start.row + dr as u32, start.col + dc as u32);
                if c.row <= LAST.row && c.col <= LAST.col && !(v.is_empty() && self.wb.cell(self.sheet, c).is_none()) {
                    inputs.push((c, v.trim_end_matches('\r').to_string()));
                }
            }
        }
        if let (Some(last_row), Some(last_col)) = (inputs.iter().map(|x| x.0.row).max(), inputs.iter().map(|x| x.0.col).max()) {
            self.wb.set_inputs(self.sheet, inputs);
            self.check_blocked();
            self.anchor = start;
            self.cursor = CellRef::new(last_row, last_col);
        }
    }

    /// Sorts the selection, or the region around the cursor, by the
    /// cursor's column; a filtered list sorts from its filter.
    fn sort(&mut self, asc: bool) {
        let sheet = self.sheet;
        let col = self.cursor.col;
        if let Some(f) = &self.sh().filter
            && f.range.contains(self.cursor)
        {
            self.wb.filter_sort(sheet, col, asc);
            return;
        }
        let sel = self.sel();
        let mut r = if sel.is_single() { self.wb.current_region(sheet, self.cursor) } else { sel };
        if let Some(u) = self.wb.clamp_to_used(sheet, &r) {
            r = u;
        }
        // A first row of labels over other kinds of values is a header.
        if r.rows() > 1 {
            let text_at = |row: u32, c: u32| matches!(self.sh().value(CellRef::new(row, c)), Value::Text(_));
            let labels = (r.start.col..=r.end.col).all(|c| text_at(r.start.row, c) || self.sh().value(CellRef::new(r.start.row, c)).is_empty());
            let values_below = (r.start.col..=r.end.col).any(|c| !text_at(r.start.row + 1, c) && !self.sh().value(CellRef::new(r.start.row + 1, c)).is_empty());
            if labels && values_below {
                r = Range::new(CellRef::new(r.start.row + 1, r.start.col), r.end);
            }
        }
        self.wb.sort(sheet, &r, col, asc);
        self.info(trf("tui.sorted", &[("range", &r.to_a1())]));
    }

    /// The next cell (after the cursor, row by row, wrapping) whose value
    /// or formula has `needle`.
    fn find(&mut self, needle: &str) {
        let n = needle.to_lowercase();
        let mut hits: Vec<CellRef> = Vec::new();
        for (c, cell) in self.sh().iter() {
            if cell.is_blank() {
                continue;
            }
            let shown = self.wb.display(self.sheet, c).0;
            if shown.to_lowercase().contains(&n) || (cell.formula().is_some() && self.wb.edit_text(self.sheet, c).to_lowercase().contains(&n)) {
                hits.push(c);
            }
        }
        hits.sort_by_key(|c| (c.row, c.col));
        let cur = (self.cursor.row, self.cursor.col);
        match hits.iter().find(|c| (c.row, c.col) > cur).or(hits.first()) {
            Some(&c) => {
                self.move_to(c, false);
                self.info(trf("tui.found_n", &[("n", &hits.len())]));
            }
            None => self.error(trf("tui.not_found", &[("text", &needle)])),
        }
    }

    fn replace(&mut self, needle: &str, with: &str) {
        let sel = self.sel();
        let n = needle.to_lowercase();
        let mut inputs = Vec::new();
        for (c, cell) in self.sh().iter() {
            if cell.is_blank() || (!sel.is_single() && !sel.contains(c)) {
                continue;
            }
            let t = self.wb.edit_text(self.sheet, c);
            if t.to_lowercase().contains(&n) {
                inputs.push((c, replace_ci(&t, needle, with)));
            }
        }
        let count = inputs.len();
        if count > 0 {
            self.wb.set_inputs(self.sheet, inputs);
            self.check_blocked();
        }
        self.info(trf("tui.replaced_n", &[("n", &count)]));
    }

    // --- questions ---------------------------------------------------------------------

    pub fn ask_text(&mut self, ask: &Ask) -> (String, String) {
        self.commit_edit();
        match ask {
            Ask::GoTo => (tr("tui.goto").to_string(), String::new()),
            Ask::Find => (tr("tui.find").to_string(), self.last_find.clone().unwrap_or_default()),
            Ask::ReplaceFind => (tr("tui.replace").to_string(), self.last_find.clone().unwrap_or_default()),
            Ask::RenameSheet => (tr("tui.rename_sheet").to_string(), self.sh().name.clone()),
            Ask::NumFormat => {
                let st = self.wb.cell(self.sheet, self.cursor).map_or(0, |c| c.style);
                (tr("tui.custom_format").to_string(), self.wb.styles.get(st).num_fmt.to_string())
            }
            Ask::ColWidth => (tr("tui.col_width").to_string(), self.col_width(self.cursor.col).saturating_sub(1).to_string()),
            Ask::TextColor | Ask::FillColor => (tr("tui.color_custom").to_string(), "#".to_string()),
            _ => (String::new(), String::new()),
        }
    }

    pub fn answer(&mut self, ask: Ask, text: String) {
        let sheet = self.sheet;
        let sel = self.sel();
        match ask {
            Ask::GoTo => match self.goto_target(text.trim()) {
                Some((s, r)) => {
                    if s != self.sheet {
                        self.select_sheet(s);
                    }
                    self.anchor = r.start;
                    self.cursor = r.end;
                    let c = self.cursor;
                    self.move_to(r.start, false);
                    self.cursor = c;
                    self.anchor = r.start;
                    self.ensure_visible();
                }
                None => self.error(trf("tui.bad_ref", &[("text", &text)])),
            },
            Ask::Find => {
                if !text.is_empty() {
                    self.find(&text);
                    self.last_find = Some(text);
                }
            }
            Ask::ReplaceWith(find) => {
                self.replace(&find, &text);
                self.last_find = Some(find);
            }
            Ask::RenameSheet => {
                let name = text.trim().to_string();
                if name.is_empty() || self.wb.sheet_index(&name).is_some_and(|i| i != sheet) || name.contains(['[', ']', '*', '?', '/', '\\', ':']) {
                    self.error(tr("tui.bad_sheet_name"));
                } else {
                    self.wb.execute(Op::RenameSheet { index: sheet, name });
                }
            }
            Ask::NumFormat => {
                if !text.trim().is_empty() {
                    let f: Arc<str> = Arc::from(text.trim());
                    self.wb.modify_style(sheet, &sel, |s| s.num_fmt = f.clone());
                }
            }
            Ask::ColWidth => match text.trim().replace(',', ".").parse::<f32>() {
                Ok(w) if (0.0..=255.0).contains(&w) => {
                    let px = if w == 0.0 { 0.0 } else { ((w * 7.0 + 5.0).round() * (88.0 / 64.0)).max(0.0) };
                    let sizes = (sel.start.col..=sel.end.col.min(sel.start.col + 1000)).map(|c| (c, Some(px))).collect();
                    self.wb.execute(Op::ColWidths { sheet, sizes });
                }
                _ => self.error(tr("tui.bad_number")),
            },
            Ask::TextColor | Ask::FillColor => match parse_color(&text) {
                Some(c) => self.run(if ask == Ask::TextColor { Cmd::TextColor(Some(c)) } else { Cmd::FillColor(Some(c)) }),
                None => self.error(tr("tui.bad_color")),
            },
            _ => {}
        }
        self.check_blocked();
    }

    /// A cell, range, `Sheet!A1` reference or defined name.
    fn goto_target(&self, s: &str) -> Option<(usize, Range)> {
        if s.is_empty() {
            return None;
        }
        if let Some(t) = self.wb.name_target(self.sheet, s) {
            return Some(t);
        }
        let (sheet, r) = match s.rsplit_once('!') {
            Some((name, r)) => (self.wb.sheet_index(name.trim_matches('\''))?, r),
            None => (self.sheet, s),
        };
        let r = r.replace('$', "");
        let range = Range::parse(&r).or_else(|| CellRef::parse(&r).map(Range::single))?;
        Some((sheet, range))
    }

    // --- autofilter --------------------------------------------------------------------

    /// The value list of the filter column at the cursor.
    pub fn filter_menu(&mut self) -> Option<Choice> {
        self.commit_edit();
        let Some(f) = self.sh().filter.clone() else {
            self.error(tr("tui.no_filter"));
            return None;
        };
        let col = self.cursor.col;
        if !(f.range.start.col..=f.range.end.col).contains(&col) {
            self.error(tr("tui.no_filter"));
            return None;
        }
        // Every value of the column (with the other columns' filters on).
        let (values, blanks) = self.wb.filter_values(self.sheet, &f, col);
        let checked = |v: &str| match f.column(col).map(|c| &c.kind) {
            Some(FilterKind::Values { items, .. }) => items.iter().any(|i| i.eq_ignore_ascii_case(v)),
            Some(_) => true,
            None => true,
        };
        let blank_on = match f.column(col).map(|c| &c.kind) {
            Some(FilterKind::Values { blanks, .. }) => *blanks,
            _ => true,
        };
        let mut items = vec![Item { label: tr("tui.select_all").to_string(), keys: String::new(), cmd: None, checked: Some(true) }];
        for v in values {
            let on = checked(&v);
            items.push(Item { label: v, keys: String::new(), cmd: None, checked: Some(on) });
        }
        if blanks {
            items.push(Item { label: tr("tui.blanks").to_string(), keys: String::new(), cmd: None, checked: Some(blank_on) });
        }
        items[0].checked = Some(items[1..].iter().all(|i| i.checked == Some(true)));
        items.push(Item { label: tr("tui.cmd_sort_asc").to_string(), keys: String::new(), cmd: Some(Cmd::SortAsc), checked: None });
        items.push(Item { label: tr("tui.cmd_sort_desc").to_string(), keys: String::new(), cmd: Some(Cmd::SortDesc), checked: None });
        let title = trf("tui.filter_title", &[("col", &CellRef::new(0, col).to_a1().trim_end_matches('1'))]);
        Some(Choice::new(title, items, Purpose::Filter { col }))
    }

    /// Applies the checks of a filter list.
    pub fn filter_picked(&mut self, purpose: Purpose, picked: Vec<(String, bool)>) {
        let Purpose::Filter { col } = purpose else { return };
        let Some(mut f) = self.sh().filter.clone() else { return };
        let blanks_label = tr("tui.blanks");
        let blanks = picked.iter().any(|(l, on)| l == blanks_label && *on);
        let items: Vec<String> = picked.iter().filter(|(l, on)| *on && l != blanks_label).map(|(l, _)| l.clone()).collect();
        f.cols.retain(|c| c.col != col);
        if picked.iter().any(|(_, on)| !on) {
            f.cols.push(ColumnFilter { col, kind: FilterKind::Values { items, blanks } });
        }
        self.wb.set_filter(self.sheet, Some(f), "undo.filter");
        self.check_blocked();
        self.cursor = self.cursor_visible(self.cursor);
        self.anchor = self.cursor;
    }
}

/// Replaces every match of `needle` ignoring case.
fn replace_ci(text: &str, needle: &str, with: &str) -> String {
    let (lt, ln) = (text.to_lowercase(), needle.to_lowercase());
    // Offsets in the lowercase text are the original's only when every
    // character keeps its length.
    if needle.is_empty() || !text.chars().all(|c| c.to_lowercase().map(char::len_utf8).sum::<usize>() == c.len_utf8()) {
        return text.replace(needle, with);
    }
    let mut out = String::new();
    let mut i = 0;
    while let Some(k) = lt[i..].find(&ln) {
        out.push_str(&text[i..i + k]);
        out.push_str(with);
        i += k + ln.len();
    }
    out.push_str(&text[i..]);
    out
}

/// "#RRGGBB" or "RRGGBB".
pub fn parse_color(s: &str) -> Option<u32> {
    let h = s.trim().trim_start_matches('#');
    (h.len() == 6).then(|| u32::from_str_radix(h, 16).ok()).flatten()
}
