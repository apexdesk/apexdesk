//! View > New Window (a second tab on the same workbook or document) and View Side by
//! Side (two sheet tabs shown together, optionally scrolling together).
//!
//! The windows of a workbook share it by handing it over: the tab being
//! shown holds the workbook and the others keep an empty placeholder.

use eframe::egui::{self, Rect};

use super::{App, Tab, View};
use crate::common::Shell;
use crate::sheet_view::SheetView;

/// Side by side state: the other tab, synchronous scrolling and the top
/// left cell last seen of each window (tab id, row and column).
pub(super) struct SideBySide {
    pub other: u64,
    pub sync: bool,
    pub last: Vec<(u64, (u32, u32))>,
}

impl Tab {
    /// The workbook group of a window (None for a lone window).
    pub(super) fn group(&self) -> Option<u64> {
        self.link
    }
}

impl App {
    fn sheet_view(&mut self, idx: usize) -> Option<&mut SheetView> {
        match self.tabs.get_mut(idx).map(|t| &mut t.view) {
            Some(View::Sheet(v)) => Some(v),
            _ => None,
        }
    }

    /// Makes tab `idx` the holder of its workbook.
    pub(super) fn hold(&mut self, idx: usize) {
        let Some(group) = self.tabs.get(idx).and_then(Tab::group) else { return };
        if self.tabs[idx].holds {
            return;
        }
        let Some(h) = self.tabs.iter().position(|t| t.group() == Some(group) && t.holds) else { return };
        let (a, b) = if h < idx { (h, idx) } else { (idx, h) };
        let (left, right) = self.tabs.split_at_mut(b);
        let swapped = match (&mut left[a].view, &mut right[0].view) {
            (View::Sheet(x), View::Sheet(y)) => {
                x.swap_workbook(y);
                true
            }
            (View::Doc(x), View::Doc(y)) => {
                x.swap_document(y);
                true
            }
            _ => false,
        };
        if swapped {
            self.tabs[h].holds = false;
            self.tabs[idx].holds = true;
        }
    }

    /// Whether the workbook shown by tab `i` has unsaved changes.
    pub(super) fn tab_modified(&self, i: usize) -> bool {
        let Some(t) = self.tabs.get(i) else { return false };
        match t.group() {
            Some(g) => self.tabs.iter().any(|x| x.group() == Some(g) && x.view.modified()),
            None => t.view.modified(),
        }
    }

    /// Whether closing tab `i` leaves its workbook open in another window.
    pub(super) fn has_other_window(&self, i: usize) -> bool {
        let Some(g) = self.tabs.get(i).and_then(Tab::group) else { return false };
        self.tabs.iter().enumerate().any(|(j, t)| j != i && t.group() == Some(g))
    }

    /// Before removing tab `i`: another window of the workbook takes it.
    pub(super) fn release_window(&mut self, i: usize) {
        let Some(g) = self.tabs.get(i).and_then(Tab::group) else { return };
        if self.tabs[i].holds {
            if let Some(j) = self.tabs.iter().enumerate().position(|(j, t)| j != i && t.group() == Some(g)) {
                self.hold(j);
            }
        }
    }

    /// After a removal: a workbook left with one window is a plain tab.
    pub(super) fn tidy_windows(&mut self) {
        let groups: Vec<u64> = self.tabs.iter().filter_map(Tab::group).collect();
        for t in &mut self.tabs {
            if let Some(g) = t.group() {
                if groups.iter().filter(|x| **x == g).count() < 2 {
                    t.link = None;
                    t.window_no = 0;
                    t.holds = false;
                }
            }
        }
        if let Some(s) = &self.side {
            let other = self.tabs.iter().position(|t| t.id == s.other);
            if other.is_none() || other == Some(self.active) {
                self.side = None;
            }
        }
    }

    /// View > New Window.
    pub(super) fn new_window(&mut self) {
        let idx = self.active;
        let view = match self.tabs.get_mut(idx).map(|t| &mut t.view) {
            Some(View::Sheet(v)) => View::Sheet(Box::new(v.linked_window())),
            Some(View::Doc(v)) => View::Doc(Box::new(v.linked_window())),
            Some(View::Slides(_)) | None => return,
            Some(View::Pdf(_)) | Some(View::Db(_)) | Some(View::Note(_)) => return,
        };
        let group = {
            let t = &mut self.tabs[idx];
            if t.link.is_none() {
                t.link = Some(t.id);
                t.window_no = 1;
                t.holds = true;
            }
            t.link
        };
        let n = self.tabs.iter().filter(|t| t.link == group).map(|t| t.window_no).max().unwrap_or(1) + 1;
        let (path, format, untitled, password, encoding, downloaded) = {
            let t = &self.tabs[idx];
            (t.path.clone(), t.format, t.untitled.clone(), t.password.clone(), t.encoding, t.downloaded)
        };
        let id = self.next_id;
        self.next_id += 1;
        self.tabs.insert(idx + 1, Tab { id, path, format, untitled, view, link: group, window_no: n, holds: false, password, embed: None, encoding, downloaded });
        self.active = idx + 1;
        self.hold(self.active);
    }

    /// After saving tab `i`: its other windows show the new file name.
    pub(super) fn share_path(&mut self, i: usize) {
        let Some(g) = self.tabs.get(i).and_then(Tab::group) else { return };
        let (path, format) = (self.tabs[i].path.clone(), self.tabs[i].format);
        for t in self.tabs.iter_mut().filter(|t| t.group() == Some(g)) {
            t.path = path.clone();
            t.format = format;
        }
    }

    /// Shows the active tab, and the side by side one next to it.
    /// Returns false when there is no side by side view to show.
    pub(super) fn side_by_side_ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell, notices: &mut Vec<crate::app::Notice>) -> bool {
        let Some(s) = &self.side else { return false };
        let Some(other) = self.tabs.iter().position(|t| t.id == s.other) else {
            self.side = None;
            return false;
        };
        if other == self.active || !matches!(self.tabs[self.active].view, View::Sheet(_)) || !matches!(self.tabs[other].view, View::Sheet(_)) {
            self.side = None;
            return false;
        }
        let full = ui.available_rect_before_wrap();
        let gap = 4.0;
        let half = (full.width() - gap) / 2.0;
        // The active window on the left, like the other window keeps its place.
        let (first, second) = if self.active < other { (self.active, other) } else { (other, self.active) };
        let rects = [Rect::from_min_size(full.min, egui::vec2(half, full.height())), Rect::from_min_max(egui::pos2(full.min.x + half + gap, full.min.y), full.max)];
        // A press in the other window activates it.
        let press = ui.input(|i| if i.pointer.any_pressed() { i.pointer.interact_pos() } else { None });
        if let Some(p) = press {
            let over_popup = ui.ctx().layer_id_at(p).is_some_and(|l| l.order != egui::Order::Background);
            let other_rect = if other == first { rects[0] } else { rects[1] };
            if other_rect.contains(p) && !over_popup {
                let old = self.active;
                self.active = other;
                let old_id = self.tabs[old].id;
                if let Some(s) = &mut self.side {
                    s.other = old_id;
                    s.last.clear();
                }
            }
        }
        let mut seen = Vec::new();
        for (k, idx) in [first, second].into_iter().enumerate() {
            self.hold(idx);
            let r = rects[k];
            let active = idx == self.active;
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).id_salt(("side", k)));
            child.set_clip_rect(r);
            crate::file_dialog::set_owner(self.tabs[idx].id);
            if let Some(v) = self.sheet_view(idx) {
                v.ui(&mut child, shell);
                notices.append(&mut v.notices);
                seen.push((idx, v.top_left_cell()));
            }
            if active {
                ui.painter().rect_stroke(r, 0.0, egui::Stroke::new(1.5, crate::theme::SHEET_ACCENT), egui::StrokeKind::Inside);
            }
        }
        self.hold(self.active);
        crate::file_dialog::set_owner(self.tabs[self.active].id);
        if self.sync_windows(seen) {
            ui.ctx().request_repaint();
        }
        true
    }

    /// Synchronous scrolling: when a window (the active one first) moved
    /// by some rows or columns, the other one moves by as many, whatever
    /// the heights and widths of their rows and columns. `seen` has the
    /// top left cell of each window as drawn.
    /// Returns whether it moved one.
    fn sync_windows(&mut self, seen: Vec<(usize, (u32, u32))>) -> bool {
        let Some(s) = &self.side else { return false };
        let mut now: Vec<(usize, u64, (u32, u32))> = seen.iter().map(|&(i, cell)| (i, self.tabs[i].id, cell)).collect();
        now.sort_by_key(|n| n.0 != self.active);
        let moved = |k: usize| s.last.iter().find(|l| l.0 == now[k].1).map(|l| l.1).filter(|l| *l != now[k].2);
        let step = if s.sync && now.len() == 2 { (0..2).find_map(|k| moved(k).map(|l| (k, l))) } else { None };
        if let Some((k, (row, col))) = step {
            let o = 1 - k;
            let by = |to: u32, from: u32, at: u32| (at as i64 + to as i64 - from as i64).max(0) as u32;
            let to = (by(now[k].2.0, row, now[o].2.0), by(now[k].2.1, col, now[o].2.1));
            let idx = now[o].0;
            self.hold(idx);
            if let Some(v) = self.sheet_view(idx) {
                v.set_top_left_cell(to.0, to.1);
                now[o].2 = v.top_left_cell();
            }
            self.hold(self.active);
        }
        if let Some(s) = &mut self.side {
            s.last = now.iter().map(|n| (n.1, n.2)).collect();
        }
        step.is_some()
    }

    /// The other workbooks and their sheets while the active one has Move
    /// or Copy open (one entry per workbook, named by its shown window).
    pub(super) fn books_for_move(&self) -> Vec<(u64, String, Vec<String>)> {
        let Some(View::Sheet(v)) = self.tabs.get(self.active).map(|t| &t.view) else { return Vec::new() };
        if !v.wants_books() {
            return Vec::new();
        }
        let mine = self.tabs[self.active].group();
        let holder = |t: &Tab| t.group().is_none() || t.holds;
        self.tabs
            .iter()
            .enumerate()
            .filter(|(i, t)| *i != self.active && (mine.is_none() || t.group() != mine) && holder(t))
            .filter_map(|(_, t)| match &t.view {
                View::Sheet(v) => Some((t.id, t.name(), v.wb.sheets.iter().map(|s| s.name.clone()).collect())),
                View::Doc(_) | View::Slides(_) => None,
                View::Pdf(_) | View::Db(_) | View::Note(_) => None,
            })
            .collect()
    }

    /// Move or Copy to another workbook (tab id) or to a new one.
    pub(super) fn sheet_to_book(&mut self, sheet: usize, target: Option<u64>, before: usize, copy: bool) {
        let src = self.active;
        let dst = match target {
            Some(id) => match self.tabs.iter().position(|t| t.id == id) {
                Some(d) if d != src => d,
                _ => return,
            },
            None => {
                let Some(View::Sheet(v)) = self.tabs.get(src).map(|t| &t.view) else { return };
                let wb = apex_sheet::Workbook::book_from_sheet(&v.wb, sheet);
                self.push_tab(View::Sheet(Box::new(SheetView::new(wb))), None, None);
                if !copy {
                    if let Some(View::Sheet(v)) = self.tabs.get_mut(src).map(|t| &mut t.view) {
                        v.sheet_moved_out(sheet);
                    }
                }
                return;
            }
        };
        let (a, b) = if src < dst { (src, dst) } else { (dst, src) };
        let (left, right) = self.tabs.split_at_mut(b);
        let (from, to) = if src < dst { (&mut left[a].view, &mut right[0].view) } else { (&mut right[0].view, &mut left[a].view) };
        let (View::Sheet(from), View::Sheet(to)) = (from, to) else { return };
        let n = to.wb.sheets.len();
        to.adopt_sheet(&from.wb, sheet, before);
        if to.wb.sheets.len() == n {
            return;
        }
        if !copy {
            from.sheet_moved_out(sheet);
        }
        self.active = dst;
        self.hold(dst);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use eframe::egui::{self, Pos2, Rect, vec2};

    use super::super::{App, View};
    use super::SideBySide;
    use crate::common::{AppCmd, Shell};
    use crate::i18n::tr;
    use crate::sheet_view::SheetView;

    fn book(row_height: f32) -> SheetView {
        let mut wb = apex_sheet::Workbook::new();
        wb.set_input(0, apex_sheet::CellRef::new(0, 0), "x");
        wb.sheets[0].default_row_height = row_height;
        SheetView::new(wb)
    }

    fn frame(ctx: &egui::Context, app: &mut App) {
        let mut shell = Shell { cmd: None, show_formula_bar: true, file_name: "w".into(), fullscreen: false, dark: None, others: Vec::new(), books: Vec::new(), side: None };
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))), ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| {
            assert!(app.side_by_side_ui(ui, &mut shell, &mut Vec::new()));
        });
        out.textures_delta.clear();
    }

    fn top_left(app: &App, i: usize) -> (u32, u32) {
        match &app.tabs[i].view {
            View::Sheet(v) => v.top_left_cell(),
            _ => unreachable!(),
        }
    }

    fn scroll_to(app: &mut App, i: usize, row: u32, col: u32) {
        if let View::Sheet(v) = &mut app.tabs[i].view {
            v.set_top_left_cell(row, col);
        }
    }

    /// Synchronous scrolling moves both windows by the same rows and
    /// columns (rows of different heights too), whichever one scrolls.
    #[test]
    fn side_by_side_scrolls_rows_together() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let mut app = App::blank(&ctx, crate::settings::Settings::default(), None, Instant::now());
        app.push_tab(View::Sheet(Box::new(book(15.0))), None, None);
        app.push_tab(View::Sheet(Box::new(book(40.0))), None, None);
        app.active = 0;
        app.side = Some(SideBySide { other: app.tabs[1].id, sync: true, last: Vec::new() });
        frame(&ctx, &mut app);
        // The active window scrolls: the other follows by as many rows.
        scroll_to(&mut app, 0, 30, 4);
        frame(&ctx, &mut app);
        assert_eq!(top_left(&app, 1), (30, 4));
        // The other one scrolls: the active one follows.
        scroll_to(&mut app, 1, 50, 6);
        frame(&ctx, &mut app);
        assert_eq!(top_left(&app, 0), (50, 6));
        // Not in sync: each on its own.
        app.run_cmd(&ctx, AppCmd::SyncScroll);
        scroll_to(&mut app, 0, 10, 0);
        frame(&ctx, &mut app);
        frame(&ctx, &mut app);
        assert_eq!(top_left(&app, 1), (50, 6));
    }

    /// Without side by side, the buttons say what they need.
    #[test]
    fn side_by_side_says_what_it_needs() {
        let ctx = egui::Context::default();
        let mut app = App::blank(&ctx, crate::settings::Settings::default(), None, Instant::now());
        app.run_cmd(&ctx, AppCmd::SideBySide(None));
        app.run_cmd(&ctx, AppCmd::SyncScroll);
        let said: Vec<&str> = app.toasts.iter().map(|t| t.notice.text.as_str()).collect();
        assert_eq!(said, [tr("window.side_by_side_need"), tr("window.sync_scroll_need")]);
    }
}
