//! Spreadsheet commands used by the toolbar, menus and shortcuts, plus the
//! panels around the grid (status bar, sheet tabs).

use std::sync::atomic::{AtomicU8, Ordering};

use super::*;

/// Copied formatting waiting to be painted over cells (format painter).
pub(super) struct Brush {
    pub styles: Vec<u32>,
    pub rows: u32,
    pub cols: u32,
    /// Double-clicked: stays active until Escape or a new click.
    pub sticky: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ClearKind {
    All,
    Formats,
    Contents,
    Notes,
    Links,
}

/// An action F4 and Ctrl+Y repeat on the selection.
#[derive(Clone)]
pub(super) enum Repeat {
    Style(std::rc::Rc<dyn Fn(&mut CellStyle)>),
    Borders(BorderPreset),
    Clear(ClearKind),
    InsertRows(bool),
    InsertCols(bool),
    Merge(u8),
}

pub(super) enum TabAction {
    Add,
    Duplicate(usize),
    MoveCopy(usize),
    Rename(usize),
    Delete(usize),
    Color(usize, Option<u32>),
    Hide(usize),
    Unhide(usize),
}

/// Status bar statistics shown (bits: sum, average, count, numeric count,
/// minimum, maximum).
static STATS: AtomicU8 = AtomicU8::new(0b000111);
const STAT_KEYS: [&str; 6] = ["sheet.stat_sum", "sheet.stat_avg", "sheet.stat_count", "sheet.stat_numbers", "sheet.stat_min", "sheet.stat_max"];
const STAT_NAMES: [&str; 6] = ["stats.sum", "stats.average", "stats.count", "stats.numbers", "stats.min", "stats.max"];

pub fn status_stats() -> u8 {
    STATS.load(Ordering::Relaxed)
}

pub fn set_status_stats(v: u8) {
    STATS.store(v & 0b111111, Ordering::Relaxed);
}

impl SheetView {
    // --- clipboard -----------------------------------------------------------------

    pub(super) fn request_paste(&mut self, ctx: &egui::Context) {
        self.paste_expected = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
        // Nothing comes back when the clipboard holds only a picture.
        self.paste_clipboard_image(ctx);
    }

    /// Paste special from our own clipboard; external text is pasted as values.
    pub(super) fn paste_special_now(&mut self, o: PasteSpecial) {
        self.settle_edit();
        self.take_foreign_clip(None);
        let Some((_, clip, ..)) = self.clip.clone() else {
            self.notices.push(Notice::info(tr("paste.nothing")));
            return;
        };
        let at = self.sel().start;
        // The clip carries its rules and widths: it may be of another workbook.
        let r = match o.what {
            PasteWhat::Validation => self.wb.paste_clip_validation(self.sheet, at, &clip),
            PasteWhat::ColumnWidths => {
                self.wb.paste_clip_col_widths(self.sheet, at.col, &clip);
                Range::single(at)
            }
            _ => self.wb.paste_special(self.sheet, at, &clip, o),
        };
        self.anchor = r.start;
        self.cursor = CellRef::new(r.end.row.min(LAST.row), r.end.col.min(LAST.col));
        if let Some(c) = &mut self.clip {
            c.3 = false;
        }
    }

    /// Paste Link: formulas pointing at the copied cells.
    pub(super) fn paste_link(&mut self) {
        self.settle_edit();
        self.take_foreign_clip(None);
        let Some((_, clip, src_sheet, _)) = self.clip.clone() else {
            self.notices.push(Notice::info(tr("paste.nothing")));
            return;
        };
        let at = self.sel().start;
        let r = match src_sheet {
            Some(s) => self.wb.paste_link(s, &clip_range(&clip), self.sheet, at),
            // Copied in another workbook: a link to its file.
            None => {
                let target = clip.source.as_ref().map(|s| super::links_ui::target_for(std::path::Path::new(&s.file), self.file_dir.as_deref()));
                let Some(r) = target.and_then(|t| self.wb.paste_link_external(self.sheet, at, &clip, &t)) else {
                    self.notices.push(Notice::info(tr("paste.link_unsaved")));
                    return;
                };
                r
            }
        };
        self.anchor = r.end;
        self.cursor = r.start;
    }

    /// Paste as Picture: an image of the copied cells over the sheet.
    pub(super) fn paste_picture(&mut self) {
        self.settle_edit();
        self.take_foreign_clip(None);
        let Some((_, clip, src_sheet, _)) = self.clip.clone() else {
            self.notices.push(Notice::info(tr("paste.nothing")));
            return;
        };
        let Some(ctx) = self.ctx.clone() else { return };
        let src = clip_range(&clip);
        // Cells copied in another workbook are drawn from a sheet made of
        // them: their values, formats and sizes where they were.
        let other = src_sheet.is_none().then(|| {
            let mut wb = Workbook::new();
            let clip = wb.adopt_clip(&clip);
            for what in [PasteWhat::Values, PasteWhat::Formats] {
                wb.paste_special(0, clip.origin, &clip, PasteSpecial { what, ..Default::default() });
            }
            let sh = &mut wb.sheets[0];
            for (i, w) in &clip.col_widths {
                sh.col_widths.insert(clip.origin.col + i, *w);
            }
            for (i, h) in &clip.row_heights {
                sh.row_heights.insert(clip.origin.row + i, *h);
            }
            wb
        });
        let (wb, src_sheet) = (other.as_ref().unwrap_or(&self.wb), src_sheet.unwrap_or(0));
        let Some(png) = super::picture::range_picture(wb, &ctx, src_sheet, &src, 2.0).and_then(|img| crate::charts::encode_png(&img)) else {
            self.notices.push(Notice::error(tr("paste.picture_big")));
            return;
        };
        let sh = &wb.sheets[src_sheet];
        let w: f32 = (src.start.col..=src.end.col).map(|c| sh.col_width(c)).sum();
        let h: f32 = (src.start.row..=src.end.row).map(|r| sh.row_height(r)).sum();
        let mut img = apex_sheet::image::SheetImage::new(Arc::from(png), apex_sheet::image::ImageFormat::Png, self.sel().start, w, h);
        img.name = tr("paste.picture").to_string();
        let id = self.wb.add_image(self.sheet, img);
        if self.sh().image(id).is_some() {
            self.images.selected = Some((self.sheet, id));
        }
    }

    pub(super) fn start_brush(&mut self, sticky: bool) {
        let s = self.sel();
        let (rows, cols) = (s.rows().min(200), s.cols().min(200));
        let mut styles = Vec::with_capacity((rows * cols) as usize);
        for r in 0..rows {
            for c in 0..cols {
                styles.push(self.wb.cell(self.sheet, CellRef::new(s.start.row + r, s.start.col + c)).map_or(0, |x| x.style));
            }
        }
        let formats = styles.iter().map(|id| self.wb.styles.get(*id).clone()).collect();
        self.brush = Some(Brush { styles, rows, cols, sticky });
        self.brush_serial = crate::painter::hold(crate::painter::Paint::Cells { styles: formats, rows, cols }, sticky);
    }

    /// Takes the formats picked up in another workbook, and lets go of
    /// the painter turned off there (see `painter::sync`).
    pub(super) fn sync_brush(&mut self) {
        use crate::painter::{Paint, Sync};
        match crate::painter::sync(self.brush.is_some(), &mut self.brush_serial) {
            Sync::Keep => {}
            Sync::Drop => self.brush = None,
            Sync::Take(h) => {
                self.brush = None;
                let Paint::Cells { styles, rows, cols } = &*h.paint else { return };
                // Formats are numbers in a workbook's own list.
                let styles = styles.iter().map(|s| self.wb.styles.intern(s.clone())).collect();
                self.brush = Some(Brush { styles, rows: *rows, cols: *cols, sticky: h.sticky });
            }
        }
    }

    /// Paints the copied formatting over the selection (tiled).
    pub(super) fn apply_brush(&mut self) {
        let Some(b) = &self.brush else { return };
        let s = self.sel();
        let target = if s.is_single() { Range::new(s.start, CellRef::new((s.start.row + b.rows - 1).min(LAST.row), (s.start.col + b.cols - 1).min(LAST.col))) } else { s };
        let target = if target.cell_count() > 1_000_000 { self.wb.clamp_to_used(self.sheet, &target).unwrap_or(target) } else { target };
        if target.cell_count() > 1_000_000 {
            return;
        }
        let mut cells = Vec::new();
        for row in target.start.row..=target.end.row {
            for col in target.start.col..=target.end.col {
                let k = ((row - target.start.row) % b.rows * b.cols + (col - target.start.col) % b.cols) as usize;
                let style = b.styles[k];
                let c = CellRef::new(row, col);
                let old = self.wb.cell(self.sheet, c).cloned();
                if old.as_ref().map_or(0, |x| x.style) != style {
                    cells.push((c, Cell::with_style(old.as_ref(), style)));
                }
            }
        }
        if !cells.is_empty() {
            self.wb.execute_as(Op::SetCells { sheet: self.sheet, cells }, "undo.format");
        }
        self.anchor = target.start;
        self.cursor = target.end;
        if !b.sticky {
            self.brush = None;
        }
    }

    /// Repeats the last formatting or insertion on the selection (F4).
    pub(super) fn repeat_last(&mut self) {
        let Some(r) = self.repeat.clone() else { return };
        match r {
            Repeat::Style(f) => self.set_style(move |s| f(s)),
            Repeat::Borders(p) => self.apply_borders(p),
            Repeat::Clear(k) => self.clear(k),
            Repeat::InsertRows(b) => self.insert_rows(b),
            Repeat::InsertCols(b) => self.insert_cols(b),
            Repeat::Merge(k) => self.merge_cmd(k, false),
        }
    }

    /// Today's date or the time now, as typed in a cell.
    pub(super) fn now_text(&self, time: bool) -> String {
        let (y, mo, d, h, mi) = common::local_now();
        if time {
            return format!("{h:02}:{mi:02}");
        }
        let loc = &self.wb.locale;
        let s = loc.date_sep;
        match loc.date_order {
            apex_sheet::format::DateOrder::Dmy => format!("{d:02}{s}{mo:02}{s}{y}"),
            apex_sheet::format::DateOrder::Mdy => format!("{mo:02}{s}{d:02}{s}{y}"),
            apex_sheet::format::DateOrder::Ymd => format!("{y}{s}{mo:02}{s}{d:02}"),
        }
    }

    /// The focused editor: the formula bar or the one in the cell.
    pub(super) fn editor_id(&self, ctx: &egui::Context) -> Id {
        if ctx.memory(|m| m.has_focus(self.bar_id())) { self.bar_id() } else { self.cell_edit_id() }
    }

    /// Types text at the editor's caret (over its selection).
    pub(super) fn insert_in_editor(&mut self, ctx: &egui::Context, text: &str) {
        let id = self.editor_id(ctx);
        let n = self.buf.chars().count();
        let (a, b) = egui::text_edit::TextEditState::load(ctx, id).and_then(|s| s.cursor.char_range()).map_or((n, n), |r| {
            let (x, y) = (r.primary.index.0, r.secondary.index.0);
            (x.min(y).min(n), x.max(y).min(n))
        });
        let byte = |c: usize| self.buf.char_indices().nth(c).map_or(self.buf.len(), |(i, _)| i);
        let (ba, bb) = (byte(a), byte(b));
        self.buf.replace_range(ba..bb, text);
        super::set_caret(ctx, id, a + text.chars().count());
    }

    // --- formatting ------------------------------------------------------------------

    pub(super) fn set_size(&mut self, pt: f32) {
        let v = if (pt - DEFAULT_PT).abs() < 0.01 { 0 } else { (pt * 10.0).round().clamp(10.0, 4000.0) as u16 };
        self.set_style(move |s| s.font_size = v);
    }

    pub(super) fn grow_font(&mut self, up: bool) {
        let cur = style_pt(&self.style_at_cursor());
        self.set_size(common::step_size(cur, up));
    }

    pub(super) fn set_num_fmt(&mut self, code: &str) {
        let code: Arc<str> = Arc::from(code);
        self.set_style(move |s| s.num_fmt = code.clone());
    }

    pub(super) fn apply_borders(&mut self, p: BorderPreset) {
        self.repeat = Some(Repeat::Borders(p));
        self.settle_edit();
        let line = self.border_line;
        self.each_area("undo.borders", |v, r| v.wb.apply_borders(v.sheet, r, p, line));
    }

    /// 0 merge and center, 1 merge, 2 merge across, 3 unmerge.
    pub(super) fn merge_cmd(&mut self, kind: u8, confirmed: bool) {
        self.repeat = Some(Repeat::Merge(kind));
        self.settle_edit();
        let s = self.sel();
        if kind == 3 {
            self.wb.unmerge(self.sheet, &s);
            return;
        }
        if s.is_single() || s.cell_count() > 10_000_000 {
            return;
        }
        let across = kind == 2;
        if !confirmed && self.wb.merge_loses_data(self.sheet, &s, across) {
            self.dialog = Some(Dialog::MergeWarn(kind));
            return;
        }
        self.wb.merge(self.sheet, &s, across, kind == 0);
    }

    pub(super) fn clear(&mut self, k: ClearKind) {
        self.repeat = Some(Repeat::Clear(k));
        self.settle_edit();
        self.each_area("undo.clear", |v, r| match k {
            ClearKind::All => {
                v.wb.clear_all(v.sheet, r);
                v.wb.unmerge(v.sheet, r);
            }
            ClearKind::Formats => v.wb.clear_formats(v.sheet, r),
            ClearKind::Contents => v.wb.clear_contents(v.sheet, r),
            ClearKind::Notes => v.wb.clear_extras(v.sheet, r, false),
            ClearKind::Links => v.wb.clear_extras(v.sheet, r, true),
        });
    }

    // --- rows, columns, sheets ------------------------------------------------------------

    pub(super) fn insert_rows(&mut self, below: bool) {
        self.repeat = Some(Repeat::InsertRows(below));
        self.settle_edit();
        let s = self.sel();
        let count = if self.whole_cols() { 1 } else { s.rows() };
        let at = if below { (s.end.row + 1).min(LAST.row) } else { s.start.row };
        self.wb.execute(Op::InsertRows { sheet: self.sheet, at, count });
    }

    pub(super) fn insert_cols(&mut self, right: bool) {
        self.repeat = Some(Repeat::InsertCols(right));
        self.settle_edit();
        let s = self.sel();
        let count = if self.whole_rows() { 1 } else { s.cols() };
        let at = if right { (s.end.col + 1).min(LAST.col) } else { s.start.col };
        self.wb.execute(Op::InsertCols { sheet: self.sheet, at, count });
    }

    pub(super) fn hide(&mut self, rows: bool, hide: bool, s: &Range) {
        self.settle_edit();
        let (a, b) = if rows { (s.start.row, s.end.row) } else { (s.start.col, s.end.col) };
        // Unhiding a single row/column also reveals hidden ones next to it.
        let (a, b) = if !hide && a == b {
            let sh = self.sh();
            let size = |i: u32| if rows { sh.row_height(i) } else { sh.col_width(i) };
            let mut lo = a;
            while lo > 0 && size(lo - 1) == 0.0 {
                lo -= 1;
            }
            let mut hi = b;
            let max = if rows { LAST.row } else { LAST.col };
            while hi < max && size(hi + 1) == 0.0 {
                hi += 1;
            }
            (lo, hi)
        } else {
            (a, b)
        };
        let b = b.min(a + 100_000);
        self.wb.set_hidden(self.sheet, rows, a, b, hide);
    }

    pub(super) fn frozen(&self) -> bool {
        let sh = self.sh();
        sh.frozen_rows > 0 || sh.frozen_cols > 0
    }

    /// Freeze Panes: the rows above and the columns left of the active
    /// cell stay in view, from the top left cell shown on; with that cell
    /// active, the view is frozen in its middle, as in Excel. Unfreezes
    /// when frozen.
    pub(super) fn toggle_freeze(&mut self) {
        if self.frozen() {
            self.freeze(0, 0);
            return;
        }
        self.remove_split();
        let top = self.top_left_shown();
        let c = self.cursor;
        let (rows, cols) = if c == top {
            let sh = self.sh();
            let (w, h) = self.view_size;
            (sh.row_at(sh.row_y(top.row) + h / 2.0).max(top.row + 1), sh.col_at(sh.col_x(top.col) + w / 2.0).max(top.col + 1))
        } else {
            (if c.row > top.row { c.row } else { 0 }, if c.col > top.col { c.col } else { 0 })
        };
        self.freeze(rows, cols);
    }

    /// The top left cell in view (unfrozen).
    fn top_left_shown(&self) -> CellRef {
        let sh = self.sh();
        CellRef::new(sh.row_at(self.scroll.y), sh.col_at(self.scroll.x))
    }

    /// Freezes the rows above `rows` and the columns left of `cols` (none:
    /// unfreezes). The frozen panes show from the top left cell in view,
    /// so the view doesn't move.
    pub(super) fn freeze(&mut self, rows: u32, cols: u32) {
        if rows > 0 || cols > 0 {
            self.remove_split();
        }
        if self.frozen() {
            // Back to one pane showing what the frozen ones did.
            let sh = self.sh();
            let t = sh.frozen_top;
            let (sx, sy) = (if sh.frozen_cols > 0 { sh.col_x(t.col) } else { self.scroll.x }, if sh.frozen_rows > 0 { sh.row_y(t.row) } else { self.scroll.y });
            self.scroll = vec2(sx, sy);
        }
        let top = self.top_left_shown();
        let top = CellRef::new(if top.row < rows { top.row } else { 0 }, if top.col < cols { top.col } else { 0 });
        self.wb.execute(Op::Freeze { sheet: self.sheet, rows, cols, top });
        let sh = &self.wb.sheets[self.sheet];
        if rows > 0 {
            self.scroll.y = sh.row_y(top.row);
        }
        if cols > 0 {
            self.scroll.x = sh.col_x(top.col);
        }
    }

    /// Freeze Top Row / First Column / both: the top row and the first
    /// column in view, as in Excel.
    pub(super) fn freeze_top(&mut self, row: bool, col: bool) {
        if self.frozen() {
            self.freeze(0, 0);
        }
        let top = self.top_left_shown();
        self.freeze(if row { top.row + 1 } else { 0 }, if col { top.col + 1 } else { 0 });
    }

    pub(super) fn visible_sheets(&self) -> usize {
        self.wb.sheets.iter().filter(|s| !s.hidden).count()
    }

    pub(super) fn add_sheet(&mut self) {
        let name = unique_name(&self.wb, &tr("sheet.default_name").replace("{n}", ""));
        let index = self.sheet + 1;
        self.wb.execute(Op::AddSheet { index, sheet: Box::new(Sheet::new(name)) });
        self.select_sheet(index);
    }

    /// Deletes sheet `i`, asking first when other sheets' formulas read it.
    pub(super) fn remove_sheet(&mut self, i: usize) {
        if (self.visible_sheets() > 1 || self.wb.sheets[i].hidden) && !self.ask_before_delete(super::origin_ui::Deletion::Sheet(i)) {
            self.remove_sheet_now(i);
        }
    }

    pub(super) fn remove_sheet_now(&mut self, i: usize) {
        if self.visible_sheets() > 1 || self.wb.sheets[i].hidden {
            self.wb.execute(Op::RemoveSheet { index: i });
            self.clamp_sheet();
            if self.sheet >= i && self.sheet > 0 {
                self.select_sheet(self.sheet - 1);
            }
            self.skip_hidden_sheet();
        }
    }

    pub(super) fn hide_sheet(&mut self, i: usize) {
        if self.visible_sheets() > 1 {
            let tab_color = self.wb.sheets[i].tab_color;
            self.wb.execute(Op::SheetProps { index: i, hidden: true, tab_color });
            self.skip_hidden_sheet();
        }
    }

    pub(super) fn unhide_sheet(&mut self, i: usize) {
        let tab_color = self.wb.sheets[i].tab_color;
        self.wb.execute(Op::SheetProps { index: i, hidden: false, tab_color });
        self.select_sheet(i);
    }

    /// Moves off a hidden sheet (after hiding or undo).
    pub(super) fn skip_hidden_sheet(&mut self) {
        if self.wb.sheets.get(self.sheet).is_some_and(|s| s.hidden) {
            if let Some(i) = (0..self.wb.sheets.len()).map(|k| (self.sheet + k) % self.wb.sheets.len()).find(|&k| !self.wb.sheets[k].hidden) {
                self.select_sheet(i);
            }
        }
    }

    // --- data ------------------------------------------------------------------------------

    /// The area to sort and whether it has a header row. A single cell
    /// sorts the used area; a text header row above numbers stays in place.
    /// The range to sort without its header, and whether it seems to have
    /// one. Left to right (`by_cols`), the header is the first column.
    pub(super) fn sort_region(&self, with_header: bool, by_cols: bool) -> (Option<Range>, bool) {
        let sel = self.sel();
        let region = if sel.is_single() {
            self.wb.clamp_to_used(self.sheet, &Range::new(CellRef::new(0, 0), LAST))
        } else {
            self.wb.clamp_to_used(self.sheet, &sel).map(|r| Range::new(sel.start, r.end))
        };
        let Some(mut r) = region else { return (None, false) };
        let sh = self.sh();
        let header = (r.start.col..=r.end.col).all(|c| matches!(sh.value(CellRef::new(r.start.row, c)), Value::Text(_) | Value::Empty))
            && (r.start.row + 1..=r.end.row.min(r.start.row + 50)).any(|row| (r.start.col..=r.end.col).any(|c| matches!(sh.value(CellRef::new(row, c)), Value::Number(_))));
        if by_cols {
            if with_header && r.cols() > 1 {
                r.start.col += 1;
            }
        } else if (with_header || (sel.is_single() && header)) && r.rows() > 1 {
            r.start.row += 1;
        }
        (Some(r), header)
    }

    pub(super) fn sort(&mut self, ascending: bool) {
        self.settle_edit();
        if let (Some(r), _) = self.sort_region(false, false) {
            self.wb.sort_by(self.sheet, &r, &[SortKey::new(self.cursor.col.clamp(r.start.col, r.end.col), ascending)], false);
        }
    }

    /// AutoSum with `func` over the numbers above (or to the left).
    pub(super) fn auto_sum(&mut self, func: &'static str) {
        self.settle_edit();
        let s = self.sel();
        let sh = self.sh();
        let is_num = |r: u32, col: u32| matches!(sh.value(CellRef::new(r, col)), Value::Number(_));
        let name = apex_sheet::functions::Func::lookup(func).map_or(func, crate::formula_help::display_name);
        // A selected block of numbers: totals go in the row below.
        if !s.is_single() && s.rows() > 1 && s.end.row < LAST.row {
            let inputs: Vec<(CellRef, String)> = (s.start.col..=s.end.col)
                .map(|c| (CellRef::new(s.end.row + 1, c), format!("={name}({})", Range::new(CellRef::new(s.start.row, c), CellRef::new(s.end.row, c)).to_a1())))
                .collect();
            self.wb.set_inputs(self.sheet, inputs);
            return;
        }
        let c = self.cursor;
        let range = if c.row > 0 && is_num(c.row - 1, c.col) {
            let mut r = c.row - 1;
            while r > 0 && is_num(r - 1, c.col) {
                r -= 1;
            }
            Some(Range::new(CellRef::new(r, c.col), CellRef::new(c.row - 1, c.col)))
        } else if c.col > 0 && is_num(c.row, c.col - 1) {
            let mut k = c.col - 1;
            while k > 0 && is_num(c.row, k - 1) {
                k -= 1;
            }
            Some(Range::new(CellRef::new(c.row, k), CellRef::new(c.row, c.col - 1)))
        } else {
            None
        };
        match range {
            Some(r) => {
                self.wb.set_input(self.sheet, c, &format!("={name}({})", r.to_a1()));
                self.move_by(1, 0, false, false);
            }
            None => {
                // Nothing to total: start the formula for the user to finish.
                if let Some(ctx) = self.ctx.clone() {
                    self.begin_edit(&ctx, EditOrigin::Cell, false, Some(format!("={name}(")));
                }
            }
        }
    }

    pub(super) fn fill(&mut self, dir: FillDir) {
        self.settle_edit();
        let s = self.sel();
        let ok = match dir {
            FillDir::Down | FillDir::Up => s.rows() > 1,
            FillDir::Right | FillDir::Left => s.cols() > 1,
        };
        if ok {
            self.wb.fill_dir(self.sheet, &s, dir);
        }
    }

    // --- autofit --------------------------------------------------------------------------

    fn text_size(&self, ui: &egui::Ui, c: CellRef, style: u32, wrap: Option<f32>) -> Vec2 {
        let (text, _) = self.wb.display(self.sheet, c);
        if text.is_empty() {
            return Vec2::ZERO;
        }
        let st = self.wb.styles.get(style);
        let text = visible_part(&text, vec2(800.0, 400.0), cell_px(st, 1.0), wrap.is_some(), HAlign::Left);
        let job = cell_job(text, st, Paper::TEXT, 1.0, wrap);
        let gal = ui.fonts_mut(|f| f.layout_job(job));
        gal.size() + vec2(st.indent as f32 * INDENT_PX, 0.0)
    }

    pub(super) fn autofit_cols(&mut self, ui: &egui::Ui, c0: u32, c1: u32) {
        let Some(ext) = self.sh().extent() else { return };
        let mut sizes = Vec::new();
        for col in c0..=c1.min(ext.col) {
            let r = Range::new(CellRef::new(0, col), CellRef::new(ext.row.min(20_000), col));
            let mut cells = Vec::new();
            self.sh().for_each_in(&r, |c, cell| cells.push((c, cell.style)));
            let w = cells.into_iter().map(|(c, st)| self.text_size(ui, c, st, None).x).fold(0.0, f32::max);
            if w > 0.0 {
                sizes.push((col, Some((w + 10.0).min(800.0))));
            }
        }
        if !sizes.is_empty() {
            self.wb.execute(Op::ColWidths { sheet: self.sheet, sizes });
        }
    }

    pub(super) fn autofit_rows(&mut self, ui: &egui::Ui) {
        let s = self.sel();
        let Some(r) = self.wb.clamp_to_used(self.sheet, &s) else { return };
        let r = Range::new(CellRef::new(r.start.row, 0), CellRef::new(r.end.row.min(r.start.row + 20_000), r.end.col));
        let mut tallest: std::collections::BTreeMap<u32, f32> = Default::default();
        let mut cells = Vec::new();
        self.sh().for_each_in(&r, |c, cell| cells.push((c, cell.style)));
        for (c, style) in cells {
            let st = self.wb.styles.get(style);
            let wrap = st.wrap.then(|| self.sh().col_width(c.col) - 6.0);
            let h = self.text_size(ui, c, style, wrap).y;
            let e = tallest.entry(c.row).or_insert(0.0);
            *e = e.max(h);
        }
        let sizes: Vec<(u32, Option<f32>)> = (r.start.row..=r.end.row).map(|row| (row, tallest.get(&row).map(|h| (h + 6.0).max(apex_sheet::sheet::DEFAULT_ROW_HEIGHT)).filter(|h| *h > apex_sheet::sheet::DEFAULT_ROW_HEIGHT))).collect();
        self.wb.execute(Op::RowHeights { sheet: self.sheet, sizes });
    }

    // --- panels -------------------------------------------------------------------------------

    pub fn ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        self.ctx = Some(ui.ctx().clone());
        self.sync_brush();
        let area = ui.max_rect();
        self.audit.watch.book.clone_from(&shell.file_name);
        crate::themes_ui::set_active(&self.wb.cell_styles.theme);
        // Refresh the editor text when the selection or the data changed.
        let key = (self.sheet, self.cursor, self.wb.revision);
        if self.edit.is_none() && self.buf_key != Some(key) {
            self.buf = if self.wb.formula_hidden(self.sheet, self.cursor) { String::new() } else { self.wb.edit_text(self.sheet, self.cursor) };
            self.buf_key = Some(key);
        }
        if std::mem::take(&mut self.refocus) {
            let id = if self.edit.is_some_and(|e| e.origin == EditOrigin::Bar) { self.bar_id() } else { self.cell_edit_id() };
            ui.memory_mut(|m| m.request_focus(id));
            self.caret_to_end = true;
        }
        self.skip_hidden_sheet();
        self.images_frame();
        self.f10_frame();
        self.autocomplete_keys(ui);
        self.editor_keys(ui);

        egui::Panel::top(Id::new(("sheet_toolbar", self.uid)))
            .frame(egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(8, 4)))
            .show(ui, |ui| {
                self.getdata_bar(ui);
                if self.find.is_some() {
                    self.find_bar(ui);
                }
                if shell.show_formula_bar {
                    if self.find.is_some() {
                        ui.add_space(2.0);
                    }
                    self.formula_bar(ui);
                }
            });
        egui::Panel::bottom(Id::new(("sheet_tabs", self.uid)))
            .frame(egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(6, 3)))
            .show(ui, |ui| self.sheet_tabs(ui));
        self.f10_panels(ui);
        self.gantt_panel(ui);
        egui::CentralPanel::default_margins().frame(egui::Frame::new()).show(ui, |ui| self.grid(ui));
        self.formula_popups(ui);
        self.data_popups(ui);
        self.audit_popups(ui.ctx());
        self.notes_popups(ui.ctx());
        self.review_popups(ui.ctx());
        self.page_setup_dialog(ui.ctx());
        self.image_dialog(ui.ctx());
        self.spelling_dialog(ui.ctx());
        self.dialogs(ui.ctx());
        self.plan_dialogs(ui.ctx(), shell);
        self.clipboard_pane(ui.ctx());
        self.move_copy_dialog(ui.ctx(), shell);
        self.custom_views_dialog(ui.ctx());
        self.edit_links_dialog(ui.ctx(), shell);
        self.queries_dialog(ui.ctx());
        self.data_model_dialog(ui.ctx());
        self.getdata_frame(ui.ctx());
        self.cell_style_dialogs(ui.ctx());
        self.data_dialogs(ui.ctx());
        self.f10_ui(ui.ctx());
        self.read_aloud_frame(ui.ctx(), area);
        // Commands refused because the file was opened read-only.
        if self.wb.read_only && self.wb.take_blocked() {
            self.notices.push(Notice::info(tr("lock.blocked")));
        }
    }

    pub fn status(&mut self, ui: &mut egui::Ui) {
        let sel = self.sel();
        let label = if sel.is_single() { self.cursor.to_a1() } else { format!("{}  ({}×{})", sel_label(&sel, self.wb.calc.r1c1), sel.rows(), sel.cols()) };
        ui.label(label);
        if self.wb.group.len() > 1 {
            ui.separator();
            if ui.add(egui::Button::new(RichText::new(trf("sheet.group_status", &[("n", &self.wb.group.len())])).color(theme::ACCENT)).frame(false)).on_hover_text(tr("sheet.ungroup")).clicked() {
                self.group_all(false);
            }
        }
        if let Some(b) = &self.brush {
            ui.separator();
            ui.label(RichText::new(if b.sticky { tr("brush.active_sticky") } else { tr("brush.active") }).color(theme::ACCENT));
        }
        if self.wb.needs_calc() {
            ui.separator();
            if ui.add(egui::Button::new(RichText::new(tr("calc.calculate")).color(theme::ACCENT)).frame(false)).on_hover_text(tr("calc.pending")).clicked() {
                self.calculate_now();
            }
        }
        if let Some((ok, all)) = self.tests_count() {
            ui.separator();
            let (text, color) = if ok == all { (trf("tests.status_ok", &[("n", &all)]), theme::SHEET_ACCENT) } else { (trf("tests.status_fail", &[("n", &(all - ok)), ("m", &all)]), Color32::from_rgb(0xC4, 0x2B, 0x1C)) };
            if ui.add(egui::Button::new(RichText::new(text).color(color)).frame(false)).on_hover_text(tr("tests.title")).clicked() {
                self.open_tests();
            }
        }
        if let Some((s, c)) = self.wb.circular_cell() {
            ui.separator();
            let at = if s == self.sheet { c.to_a1() } else { format!("{}!{}", self.wb.sheets[s].name, c.to_a1()) };
            if ui.add(egui::Label::new(trf("calc.circular", &[("v", &at)])).sense(Sense::click())).clicked() {
                if s != self.sheet {
                    self.select_sheet(s);
                }
                self.select(c, false);
            }
        }
        if sel.is_single() {
            return;
        }
        let key = (self.sheet, sel, self.wb.revision);
        if self.stats.as_ref().is_none_or(|(k, _)| *k != key) {
            self.stats = Some((key, self.wb.stats(self.sheet, &sel)));
        }
        let st = self.stats.as_ref().unwrap().1.clone();
        if st.count == 0 {
            return;
        }
        ui.separator();
        let fmt = |n: f64| self.wb.format_value(0, &Value::Number(n)).0;
        let mask = status_stats();
        let values: [Option<String>; 6] = [
            (st.numbers > 0).then(|| fmt(st.sum)),
            (st.numbers > 0).then(|| fmt(st.sum / st.numbers as f64)),
            Some(st.count.to_string()),
            Some(st.numbers.to_string()),
            st.min.map(fmt),
            st.max.map(fmt),
        ];
        let mut any = false;
        for (i, v) in values.iter().enumerate() {
            if mask & (1 << i) != 0 {
                if let Some(v) = v {
                    let r = ui.add(egui::Label::new(trf(STAT_KEYS[i], &[("v", v)])).sense(Sense::click()));
                    r.context_menu(|ui| stats_chooser(ui));
                    any = true;
                }
            }
        }
        if !any {
            let r = ui.add(egui::Label::new(RichText::new("Σ").weak()).sense(Sense::click()));
            r.context_menu(|ui| stats_chooser(ui));
        }
    }

    pub(super) fn sheet_tabs(&mut self, ui: &mut egui::Ui) {
        let mut action: Option<TabAction> = None;
        let mut select = None;
        let mut rename_done: Option<(usize, bool)> = None;
        let mut drop_at: Option<(usize, usize)> = None;
        ui.horizontal(|ui| {
            if ui.button("+").on_hover_text(tr("sheet.add_sheet")).clicked() {
                action = Some(TabAction::Add);
            }
            ui.separator();
            egui::ScrollArea::horizontal().id_salt(("tabs_scroll", self.uid)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let mut rects: Vec<(usize, Rect)> = Vec::new();
                    for i in 0..self.wb.sheets.len() {
                        if self.wb.sheets[i].hidden {
                            continue;
                        }
                        if let Some((ri, name)) = &mut self.renaming {
                            if *ri == i {
                                let r = ui.add(egui::TextEdit::singleline(name).desired_width(110.0));
                                r.request_focus();
                                if r.lost_focus() {
                                    rename_done = Some((i, !ui.input(|i| i.key_pressed(Key::Escape))));
                                }
                                continue;
                            }
                        }
                        let selected = i == self.sheet;
                        let in_group = self.wb.group.len() > 1 && self.wb.group.contains(&i);
                        let name = self.wb.sheets[i].name.clone();
                        crate::fonts::note_text(&name);
                        let text = if selected { RichText::new(&name).strong() } else { RichText::new(&name) };
                        let resp = ui.add(egui::Button::selectable(selected || in_group, text).sense(Sense::click_and_drag()));
                        if let Some(c) = self.wb.sheets[i].tab_color {
                            let r = resp.rect;
                            ui.painter().rect_filled(Rect::from_min_max(pos2(r.min.x + 3.0, r.max.y - 4.0), pos2(r.max.x - 3.0, r.max.y - 1.0)), 1.0, theme::rgb(c));
                        }
                        rects.push((i, resp.rect));
                        if resp.clicked() {
                            let m = ui.input(|i| i.modifiers);
                            if m.command || m.shift {
                                self.group_tab(i, m.shift);
                            } else {
                                select = Some(i);
                            }
                        }
                        if resp.double_clicked() {
                            action = Some(TabAction::Rename(i));
                        }
                        if resp.drag_started() {
                            self.tab_drag = Some(i);
                        }
                        resp.context_menu(|ui| {
                            if let Some(a) = self.tab_menu(ui, i) {
                                action = Some(a);
                            }
                        });
                    }
                    // Dragging a tab: show where it would go.
                    if let (Some(from), Some(p)) = (self.tab_drag, ui.ctx().pointer_interact_pos()) {
                        let target = rects.iter().find(|(_, r)| p.x < r.center().x).map(|(i, _)| *i).unwrap_or(self.wb.sheets.len());
                        let x = rects.iter().find(|(i, _)| *i == target).map(|(_, r)| r.min.x - 2.0).or_else(|| rects.last().map(|(_, r)| r.max.x + 2.0));
                        if let Some(x) = x {
                            let y = rects.first().map_or(p.y, |(_, r)| r.center().y);
                            ui.painter().vline(x, (y - 10.0)..=(y + 10.0), Stroke::new(2.0, theme::SHEET_ACCENT));
                        }
                        if ui.input(|i| i.pointer.any_released()) {
                            drop_at = Some((from, target));
                            self.tab_drag = None;
                        }
                    }
                });
            });
        });
        if let Some((from, target)) = drop_at {
            let to = if target > from { target - 1 } else { target };
            if to != from && to < self.wb.sheets.len() {
                self.settle_edit();
                self.wb.execute(Op::MoveSheet { from, to });
                self.sheet = to;
                self.buf_key = None;
            }
        }
        if let Some(i) = select {
            self.select_sheet(i);
        }
        if let Some((i, ok)) = rename_done {
            if let Some((_, name)) = self.renaming.take() {
                let name = name.trim().to_string();
                if ok && !name.is_empty() && name != self.wb.sheets[i].name {
                    if self.wb.sheet_index(&name).is_some() {
                        self.notices.push(Notice::error(tr("sheet.name_taken")));
                    } else {
                        self.wb.execute(Op::RenameSheet { index: i, name });
                    }
                }
            }
        }
        match action {
            Some(TabAction::Add) => self.add_sheet(),
            Some(TabAction::Duplicate(i)) => {
                self.settle_edit();
                let k = self.wb.duplicate_sheet(i);
                self.select_sheet(k);
            }
            Some(TabAction::MoveCopy(i)) => self.open_move_copy(i),
            Some(TabAction::Rename(i)) => self.renaming = Some((i, self.wb.sheets[i].name.clone())),
            Some(TabAction::Delete(i)) => self.remove_sheet(i),
            Some(TabAction::Color(i, c)) => {
                let hidden = self.wb.sheets[i].hidden;
                self.wb.execute(Op::SheetProps { index: i, hidden, tab_color: c });
            }
            Some(TabAction::Hide(i)) => self.hide_sheet(i),
            Some(TabAction::Unhide(i)) => self.unhide_sheet(i),
            None => {}
        }
    }
}

fn stats_chooser(ui: &mut egui::Ui) {
    let mut mask = status_stats();
    for (i, key) in STAT_NAMES.iter().enumerate() {
        let mut on = mask & (1 << i) != 0;
        if ui.checkbox(&mut on, tr_key(key)).changed() {
            mask ^= 1 << i;
        }
    }
    if mask != status_stats() {
        set_status_stats(mask);
        common::mark_prefs_dirty();
    }
}

fn tr_key(key: &str) -> &'static str {
    crate::i18n::lookup(key).unwrap_or("")
}

/// The cells a clip was copied from.
fn clip_range(clip: &Clip) -> Range {
    Range::new(clip.origin, CellRef::new(clip.origin.row + clip.rows.max(1) - 1, clip.origin.col + clip.cols.max(1) - 1))
}
