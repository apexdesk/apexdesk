//! Drawing a workbook: formula bar, column and row headers, the visible
//! cells (only those: big sheets cost what fits on screen), sheet tabs and
//! the status bar with the selection's sum, average and count.

use apex_i18n::{functions, tr};
use apex_sheet::formula_text;
use apex_sheet::{CellRef, HAlign, MAX_COLS, MAX_ROWS, Range, Value};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use unicode_width::UnicodeWidthStr;

use crate::sheet::SheetUi;
use crate::term::{Colors, luminance};
use crate::ui::{self, fill, fit, text};

const HEAD_BG: u32 = 0x333333;
const HEAD_FG: u32 = 0xC8C8C8;
const HEAD_SEL_BG: u32 = 0x264F78;
const SEL_BG: u32 = 0x264F78;
const BAR_BG: u32 = 0x252526;
const NAME_BG: u32 = 0x3C3C3C;
const LIGHT: u32 = 0xF0F0F0;
const DARK: u32 = 0x101010;

#[derive(Clone, Copy, PartialEq)]
enum Side {
    Left,
    Center,
    Right,
}

/// How a cell looks.
struct Look {
    text: String,
    fg: Option<u32>,
    bg: Option<u32>,
    mods: Modifier,
    side: Side,
    number: bool,
    empty: bool,
}

/// Column letters of a column index.
fn col_name(c: u32) -> String {
    let a1 = CellRef::new(0, c).to_a1();
    a1.trim_end_matches('1').to_string()
}

impl SheetUi {
    fn look(&self, c: CellRef) -> Look {
        let sh = self.sh();
        let cell = sh.get(c);
        let st = self.wb.styles.get(cell.map_or(0, |x| x.style));
        let empty = cell.is_none_or(|x| x.is_blank());
        let (text, color) = if empty {
            (String::new(), None)
        } else if self.show_formulas && cell.is_some_and(|x| x.formula().is_some()) {
            (self.wb.edit_text(self.sheet, c), None)
        } else {
            self.wb.display(self.sheet, c)
        };
        let value = sh.value(c);
        let number = matches!(value, Value::Number(_)) && !(self.show_formulas && cell.is_some_and(|x| x.formula().is_some()));
        let side = match st.halign {
            HAlign::Left | HAlign::Justify => Side::Left,
            HAlign::Center => Side::Center,
            HAlign::Right => Side::Right,
            HAlign::General => match value {
                Value::Number(_) => Side::Right,
                Value::Bool(_) | Value::Error(_) => Side::Center,
                _ => Side::Left,
            },
        };
        let mut fg = color.or(st.color);
        let bg = st.fill;
        match bg {
            // Text on a fill without a color of its own: black or white.
            Some(b) if fg.is_none() => fg = Some(if luminance(b) > 0.5 { DARK } else { LIGHT }),
            // Colors too close to the terminal's usual backgrounds are left
            // out when there is no fill.
            None if fg.is_some_and(|f| !(0.2..=0.85).contains(&luminance(f))) => fg = None,
            _ => {}
        }
        let mut mods = Modifier::empty();
        if st.bold {
            mods |= Modifier::BOLD;
        }
        if st.italic {
            mods |= Modifier::ITALIC;
        }
        if st.underline {
            mods |= Modifier::UNDERLINED;
        }
        if st.strike {
            mods |= Modifier::CROSSED_OUT;
        }
        let text = text.replace(['\n', '\r', '\t'], " ");
        Look { text, fg, bg, mods, side, number, empty }
    }

    /// Draws the sheet in `area` (the screen but the menu bar's row).
    pub fn draw(&mut self, f: &mut Frame, area: Rect, colors: Colors, name: &str, modified: bool, message: Option<&(String, bool)>, overlay_open: bool) {
        if area.height < 6 || area.width < 20 {
            return;
        }
        let c = |rgb: u32| colors.color(rgb);
        let formula_y = area.y;
        let header_y = area.y + 1;
        let tabs_y = area.bottom() - 2;
        let status_y = area.bottom() - 1;
        let grid = Rect::new(area.x, area.y + 2, area.width, tabs_y - (area.y + 2));
        // A new size (or the first frame) scrolls to the cursor.
        if self.geo.grid != grid {
            self.geo.grid = grid;
            self.geo.row_head_w = 4;
            self.ensure_visible();
        }
        let (fr, fc) = (self.sh().frozen_rows, self.sh().frozen_cols);
        self.top = self.top.max(fr);
        self.left = self.left.max(fc);

        // Visible rows: the frozen ones, then from `top`.
        let mut rows: Vec<(u32, u16)> = Vec::new();
        let mut y = grid.y;
        let mut r = 0;
        while y < grid.bottom() && r < MAX_ROWS {
            if r == fr.min(self.top) && r < self.top {
                r = self.top;
                continue;
            }
            if !self.row_hidden(r) {
                rows.push((r, y));
                y += 1;
            }
            r += 1;
        }
        let last_row = rows.last().map_or(0, |x| x.0);
        let head_w = ((last_row + 1).to_string().len() as u16).max(3) + 1;
        // Visible columns, the last one maybe cut.
        let mut cols: Vec<(u32, u16, u16)> = Vec::new();
        let mut x = grid.x + head_w;
        let mut k = 0;
        while x < grid.right() && k < MAX_COLS {
            if k == fc.min(self.left) && k < self.left {
                k = self.left;
                continue;
            }
            let w = self.col_width(k);
            if w > 0 {
                let w = w.min(grid.right() - x);
                cols.push((k, x, w));
                x += w;
            }
            k += 1;
        }
        self.geo.rows = rows.clone();
        self.geo.cols = cols.clone();
        self.geo.header_y = header_y;
        self.geo.row_head_w = head_w;
        self.geo.formula_y = formula_y;
        self.geo.tabs_y = tabs_y;

        let sel = self.sel();
        let buf = f.buffer_mut();
        // Headers.
        let head = Style::new().bg(c(HEAD_BG)).fg(c(HEAD_FG));
        let head_sel = Style::new().bg(c(HEAD_SEL_BG)).fg(c(LIGHT)).add_modifier(Modifier::BOLD);
        fill(buf, Rect::new(grid.x, header_y, grid.width, 1), head);
        for &(k, x, w) in &cols {
            let st = if (sel.start.col..=sel.end.col).contains(&k) { head_sel } else { head };
            fill(buf, Rect::new(x, header_y, w, 1), st);
            let n = col_name(k);
            let nw = n.width() as u16;
            let off = w.saturating_sub(nw) / 2;
            text(buf, x + off, header_y, &n, w.saturating_sub(off), st);
        }
        for &(r, y) in &rows {
            let st = if (sel.start.row..=sel.end.row).contains(&r) { head_sel } else { head };
            fill(buf, Rect::new(grid.x, y, head_w, 1), st);
            let n = (r + 1).to_string();
            text(buf, grid.x + head_w - 1 - n.len() as u16, y, &n, n.len() as u16, st);
        }
        self.draw_cells(buf, colors, &rows, &cols, grid);
        self.draw_formula_bar(buf, colors, formula_y, area);
        self.draw_tabs(buf, colors, tabs_y, area);

        // Status bar: the argument hint while typing a formula, else the
        // message or hint, and the selection's numbers.
        let status = Rect::new(area.x, status_y, area.width, 1);
        let right = self.stats_text();
        let hint = if modified { format!("{name} ●  {}", tr("tui.hint")) } else { format!("{name}  {}", tr("tui.hint")) };
        ui::status_bar(buf, status, message, &hint, &right);
        if message.is_none() {
            self.draw_arg_hint(buf, colors, status);
        }
        if let Some(e) = &self.edit {
            if !e.ac.is_empty() {
                self.draw_ac(buf, colors, area);
            }
            if !overlay_open {
                let (cx, cy) = self.caret;
                f.set_cursor_position((cx, cy));
            }
        }
    }

    fn draw_cells(&self, buf: &mut Buffer, colors: Colors, rows: &[(u32, u16)], cols: &[(u32, u16, u16)], grid: Rect) {
        let sh = self.sh();
        let sel = self.sel();
        let (Some(first_row), Some(last_row), Some(first_col), Some(last_col)) = (rows.iter().map(|r| r.0).min(), rows.iter().map(|r| r.0).max(), cols.iter().map(|k| k.0).min(), cols.iter().map(|k| k.0).max()) else { return };
        let view = Range::new(CellRef::new(first_row, first_col), CellRef::new(last_row, last_col));
        let merges: Vec<Range> = sh.merges.iter().filter(|m| m.intersect(&view).is_some()).copied().collect();
        let filter = sh.filter.as_ref();
        let conv = |rgb: u32| colors.color(rgb);
        for &(r, y) in rows {
            // Overflowing text: the x up to which the row is already written.
            let mut written_to = 0u16;
            for (i, &(k, x, w)) in cols.iter().enumerate() {
                let cell = CellRef::new(r, k);
                let merge = merges.iter().find(|m| m.contains(cell));
                let origin = merge.map_or(cell, |m| m.start);
                let look = self.look(origin);
                let selected = sel.contains(cell) && !sel.is_single();
                let is_cursor = merge.map_or(cell == self.cursor, |m| m.contains(self.cursor));
                // Background.
                let mut bg = look.bg;
                let mut fg = look.fg;
                if selected {
                    bg = Some(SEL_BG);
                    if fg.is_some_and(|f| luminance(f) < 0.5) {
                        fg = Some(LIGHT);
                    }
                }
                let mut base = Style::new();
                if let Some(b) = bg {
                    base = base.bg(conv(b));
                }
                if x >= written_to {
                    fill(buf, Rect::new(x, y, w, 1), base);
                } else {
                    // Keep the text that overflowed here; paint the fill.
                    for xx in x..x + w {
                        if let Some(cl) = buf.cell_mut((xx, y)) {
                            if let Some(b) = bg {
                                cl.set_bg(conv(b));
                            }
                        }
                    }
                }
                // Text: in a merged area, once, over its visible width.
                let draw_text = match merge {
                    Some(m) => {
                        let first_vis_row = rows.iter().map(|q| q.0).find(|&q| m.start.row <= q && q <= m.end.row);
                        let first_vis_col = cols.iter().map(|q| q.0).find(|&q| m.start.col <= q && q <= m.end.col);
                        first_vis_row == Some(r) && first_vis_col == Some(k)
                    }
                    None => true,
                };
                let span_w: u16 = match merge {
                    Some(m) => cols[i..].iter().take_while(|q| q.0 <= m.end.col).map(|q| q.2).sum(),
                    None => w,
                };
                let editing = is_cursor && self.edit.is_some();
                if draw_text && (!look.empty || editing) && x >= written_to {
                    let mut t = look.text.clone();
                    if editing {
                        t = self.edit.as_ref().map(|e| e.line.text.clone()).unwrap_or_default();
                    }
                    let avail = if span_w >= 3 { span_w - 1 } else { span_w };
                    let tw = t.width() as u16;
                    let mut style = Style::new().add_modifier(look.mods);
                    if let Some(f) = fg {
                        style = style.fg(conv(f));
                    }
                    let side = if editing { Side::Left } else { look.side };
                    if tw > avail && look.number && !editing {
                        text(buf, x, y, &"#".repeat(avail as usize), avail, style);
                    } else if tw > avail && side == Side::Left && merge.is_none() && !editing {
                        // Spill into the empty cells on the right.
                        let mut room = avail;
                        for &(k2, _, w2) in &cols[i + 1..] {
                            let c2 = CellRef::new(r, k2);
                            if room >= tw || sh.get(c2).is_some_and(|q| !q.is_blank()) || merges.iter().any(|m| m.contains(c2)) {
                                break;
                            }
                            room += w2;
                        }
                        let used = text(buf, x, y, &t, room.min(grid.right() - x), style);
                        written_to = x + used.max(w);
                    } else {
                        let shown = if editing && tw > avail {
                            // The end of the text being typed.
                            let skip = t.chars().count().saturating_sub(avail as usize);
                            t.chars().skip(skip).collect()
                        } else {
                            fit(&t, avail as usize)
                        };
                        let sw = shown.width() as u16;
                        let off = match side {
                            Side::Left => 0,
                            Side::Center => avail.saturating_sub(sw) / 2,
                            Side::Right => avail.saturating_sub(sw),
                        };
                        text(buf, x + off, y, &shown, avail - off.min(avail), style);
                    }
                }
                if is_cursor {
                    // One colour over the whole cell: reversing each character
                    // split a filled cell into its text and its padding.
                    let cw = if draw_text { span_w } else { w };
                    for xx in x..(x + cw).min(grid.right()) {
                        if let Some(cl) = buf.cell_mut((xx, y)) {
                            cl.set_bg(conv(LIGHT));
                            cl.set_fg(conv(DARK));
                        }
                    }
                }
                // Autofilter buttons at the right end of the header cells.
                if let Some(fl) = filter
                    && r == fl.range.start.row
                    && (fl.range.start.col..=fl.range.end.col).contains(&k)
                    && w >= 2
                {
                    let on = fl.column(k).is_some();
                    if let Some(cl) = buf.cell_mut((x + w - 1, y)) {
                        cl.set_char(if on { '▼' } else { '▾' });
                        cl.set_fg(conv(if on { 0xFFC000 } else { 0x4F9DE8 }));
                    }
                }
            }
        }
    }

    fn draw_formula_bar(&mut self, buf: &mut Buffer, colors: Colors, y: u16, area: Rect) {
        let conv = |rgb: u32| colors.color(rgb);
        let bar = Style::new().bg(conv(BAR_BG));
        fill(buf, Rect::new(area.x, y, area.width, 1), bar);
        let sel = self.sel();
        let name = if sel.is_single() { self.cursor.to_a1() } else { format!("{}:{}", sel.start.to_a1(), sel.end.to_a1()) };
        let nw = 12.min(area.width / 3);
        let name_style = Style::new().bg(conv(NAME_BG)).fg(conv(LIGHT));
        fill(buf, Rect::new(area.x, y, nw, 1), name_style);
        text(buf, area.x + 1, y, &fit(&name, nw as usize - 1), nw - 1, name_style);
        text(buf, area.x + nw + 1, y, "fx", 2, bar.fg(conv(0x4F9DE8)).add_modifier(Modifier::ITALIC));
        let content = Rect::new(area.x + nw + 4, y, area.width.saturating_sub(nw + 4), 1);
        self.geo.formula_x = content.x;
        match &self.edit {
            Some(e) => {
                self.caret = e.line.render(buf, content, bar.fg(conv(LIGHT)));
            }
            None => {
                let t = self.wb.edit_text(self.sheet, self.cursor).replace('\n', "⏎");
                text(buf, content.x, y, &fit(&t, content.width as usize), content.width, bar.fg(conv(LIGHT)));
            }
        }
    }

    fn draw_tabs(&mut self, buf: &mut Buffer, colors: Colors, y: u16, area: Rect) {
        let conv = |rgb: u32| colors.color(rgb);
        let base = Style::new().bg(conv(BAR_BG)).fg(conv(HEAD_FG));
        fill(buf, Rect::new(area.x, y, area.width, 1), base);
        let visible: Vec<usize> = (0..self.wb.sheets.len()).filter(|&i| !self.wb.sheets[i].hidden).collect();
        let widths: Vec<u16> = visible.iter().map(|&i| self.wb.sheets[i].name.width() as u16 + 2).collect();
        // Start late enough for the active tab to show.
        let pos = visible.iter().position(|&i| i == self.sheet).unwrap_or(0);
        let mut first = 0;
        while first < pos && widths[first..=pos].iter().map(|w| w + 1).sum::<u16>() + 4 > area.width {
            first += 1;
        }
        let mut x = area.x;
        self.geo.tabs.clear();
        for (j, &i) in visible.iter().enumerate().skip(first) {
            let w = widths[j];
            if x + w + 4 > area.right() {
                break;
            }
            let sh = &self.wb.sheets[i];
            let mut st = if i == self.sheet { Style::new().bg(conv(NAME_BG)).fg(conv(LIGHT)).add_modifier(Modifier::BOLD) } else { base };
            if let Some(tc) = sh.tab_color {
                st = st.underline_color(conv(tc)).add_modifier(Modifier::UNDERLINED);
            }
            fill(buf, Rect::new(x, y, w, 1), st);
            text(buf, x + 1, y, &sh.name, w - 2, st);
            self.geo.tabs.push((i, x, x + w));
            x += w + 1;
        }
        text(buf, x, y, " + ", 3, base.fg(conv(0x4F9DE8)));
        self.geo.add_tab = Some((x, x + 3));
    }

    fn stats_text(&self) -> String {
        let sel = self.sel();
        if sel.is_single() {
            return String::new();
        }
        let st = self.wb.stats(self.sheet, &sel);
        if st.count == 0 {
            return String::new();
        }
        let num = |v: f64| self.wb.format_value(0, &Value::Number((v * 1e10).round() / 1e10)).0;
        if st.numbers > 0 {
            format!("{}: {}   {}: {}   {}: {}", tr("tui.sum"), num(st.sum), tr("tui.average"), num(st.sum / st.numbers as f64), tr("tui.count"), st.count)
        } else {
            format!("{}: {}", tr("tui.count"), st.count)
        }
    }

    /// The function and argument being typed: "SUM(number1, [number2]…)".
    fn draw_arg_hint(&self, buf: &mut Buffer, colors: Colors, area: Rect) {
        let Some(e) = &self.edit else { return };
        let sep = self.wb.locale.arg_sep;
        let Some((func, n)) = formula_text::call_at_end(&e.line.text[..e.line.caret], sep) else { return };
        let (args, desc) = functions::help(func, sep);
        let base = Style::new().bg(colors.color(0x2B2B2B)).fg(colors.color(0xE0E0E0));
        fill(buf, area, base);
        let mut x = area.x + 1;
        let end = area.right().saturating_sub(1);
        let mut put = |s: &str, st: Style, x: &mut u16| {
            if *x < end {
                *x += text(buf, *x, area.y, s, end - *x, st);
            }
        };
        put(&format!("{}(", functions::display_name(func)), base.add_modifier(Modifier::BOLD), &mut x);
        let parts: Vec<&str> = args.split(sep).collect();
        let last = parts.len().saturating_sub(1);
        for (i, p) in parts.iter().enumerate() {
            if i > 0 {
                put(&sep.to_string(), base, &mut x);
            }
            // Past the last argument, a repeating one ("…") stays marked.
            let current = i == n || (n > last && i == last && p.contains('…'));
            let st = if current { base.fg(colors.color(0xFFC000)).add_modifier(Modifier::BOLD | Modifier::UNDERLINED) } else { base };
            put(p, st, &mut x);
        }
        put(")  ", base.add_modifier(Modifier::BOLD), &mut x);
        put(desc, base.add_modifier(Modifier::DIM), &mut x);
    }

    /// The function names offered while typing.
    fn draw_ac(&self, buf: &mut Buffer, colors: Colors, area: Rect) {
        let Some(e) = &self.edit else { return };
        let word = formula_text::word_at_end(&e.line.text[..e.line.caret]).unwrap_or("");
        let before = e.line.text[..e.line.caret - word.len()].width() as u16;
        let x = (self.geo.formula_x + before).min(area.right().saturating_sub(30));
        let w = 44.min(area.width.saturating_sub(x - area.x));
        let base = Style::new().bg(colors.color(0x2D2D30)).fg(colors.color(0xE0E0E0));
        let sep = self.wb.locale.arg_sep;
        for (i, name) in e.ac.iter().enumerate() {
            let y = self.geo.formula_y + 1 + i as u16;
            if y >= area.bottom().saturating_sub(1) {
                break;
            }
            let st = if i == e.ac_sel { Style::new().bg(colors.color(0x4F9DE8)).fg(colors.color(DARK)) } else { base };
            fill(buf, Rect::new(x, y, w, 1), st);
            let used = text(buf, x + 1, y, name, w.saturating_sub(2), st.add_modifier(Modifier::BOLD));
            if let Some(f) = apex_sheet::functions::Func::lookup(name) {
                let desc = functions::help(f, sep).1;
                let room = w.saturating_sub(used + 3);
                text(buf, x + used + 2, y, &fit(desc, room as usize), room, st);
            }
        }
    }
}
