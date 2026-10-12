//! Views of a document: print layout (one or several pages to a row),
//! draft, outline, read mode and focus; and the window split in two panes.

use super::*;

/// How the document is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ViewMode {
    #[default]
    Print,
    /// The text without the page around it: page breaks are dotted lines.
    Draft,
    Outline,
    /// Pages to read, without the toolstrip; the document can't be changed.
    Read,
    /// Only the page, without the toolstrip or the status bar.
    Focus,
}

pub(super) struct ViewState {
    pub mode: ViewMode,
    /// Pages to a row in print layout and read mode.
    pub cols: usize,
    /// The split: height of the upper pane as a fraction of the view.
    pub split: Option<f32>,
    /// The pane the caret belongs to (0 upper, 1 lower).
    pub pane: usize,
    /// The pane being laid out and drawn.
    pub drawing: usize,
    pub outline: outline_ui::OutlineState,
    /// A toolstrip tab to show (Outlining, when that view starts).
    pub wanted_tab: Option<&'static str>,
    /// The page at the top of the view, and a page to scroll to the top.
    pub top_page: usize,
    pub scroll_to_page: Option<usize>,
    /// The zoom and pages to a row before read mode.
    saved: Option<(f32, usize)>,
    /// Frames until the zoom fits the pages again (the toolstrip goes away
    /// a frame after read mode starts).
    refit: u8,
}

impl Default for ViewState {
    fn default() -> Self {
        ViewState { mode: ViewMode::Print, cols: 1, split: None, pane: 0, drawing: 0, outline: Default::default(), wanted_tab: None, top_page: 0, scroll_to_page: None, saved: None, refit: 0 }
    }
}

/// Space between the text and the edge of a draft page.
const DRAFT_PAD: f32 = 14.0;

impl Layout {
    /// Places the pages `cols` to a row.
    pub(super) fn arrange_rows(&mut self, cols: usize) {
        let cols = cols.clamp(1, self.pages.max(1));
        self.cols = cols;
        self.gap = PAGE_GAP;
        let w = self.page.x;
        self.row_w = cols as f32 * w + (cols - 1) as f32 * PAGE_GAP;
        if cols == 1 {
            return;
        }
        let mut top = PAGE_GAP;
        for row in self.geo.chunks_mut(cols) {
            let h = row.iter().map(|g| g.size.y).fold(0.0, f32::max);
            for (c, g) in row.iter_mut().enumerate() {
                g.top = top;
                g.dx = c as f32 * (w + PAGE_GAP) + (w - g.size.x) / 2.0;
            }
            top += h + PAGE_GAP;
        }
    }

    /// Draft: only the text of each page, one page right below the other.
    pub(super) fn arrange_draft(&mut self) {
        self.cols = 1;
        self.gap = 0.0;
        let used: Vec<f32> = (0..self.geo.len())
            .map(|pg| {
                let segs = self.page_segs(pg, pg).chain(self.ghosts.iter().filter(|s| s.page == pg));
                segs.map(|s| s.y + s.gy1 - s.gy0).fold(0.0, f32::max)
            })
            .collect();
        let w = self.geo.iter().map(|g| g.content.x).fold(self.content.x, f32::max) + 2.0 * DRAFT_PAD;
        let mut top = PAGE_GAP;
        for (g, used) in self.geo.iter_mut().zip(used) {
            g.margin = vec2(DRAFT_PAD, DRAFT_PAD / 2.0);
            g.size = vec2(g.content.x + 2.0 * DRAFT_PAD, used + DRAFT_PAD);
            g.dx = (w - g.size.x) / 2.0;
            g.top = top;
            top += g.size.y;
        }
        self.page = vec2(w, self.geo.iter().map(|g| g.size.y).fold(0.0, f32::max));
        self.row_w = w;
    }

    /// The page at canvas point (`x` from the left of the pages, `y`).
    pub(super) fn page_at(&self, x: f32, y: f32) -> usize {
        let cols = self.cols.max(1);
        if cols == 1 {
            return self.page_at_y(y);
        }
        let row = self.row_at(y);
        let col = ((x + PAGE_GAP / 2.0) / (self.page.x + PAGE_GAP)).floor().clamp(0.0, (cols - 1) as f32) as usize;
        (row * cols + col).min(self.pages.saturating_sub(1))
    }

    fn row_at(&self, y: f32) -> usize {
        let cols = self.cols.max(1);
        let rows = self.geo.len().div_ceil(cols);
        (0..rows)
            .position(|r| {
                let row = &self.geo[r * cols..((r + 1) * cols).min(self.geo.len())];
                let bottom = row[0].top + row.iter().map(|g| g.size.y).fold(0.0, f32::max);
                bottom + self.gap / 2.0 > y
            })
            .unwrap_or(rows.saturating_sub(1))
    }

    /// The first and last page shown between canvas y `y0` and `y1`.
    pub(super) fn pages_between(&self, y0: f32, y1: f32) -> (usize, usize) {
        let cols = self.cols.max(1);
        if cols == 1 {
            return (self.page_at_y(y0), self.page_at_y(y1));
        }
        let last = self.pages.saturating_sub(1);
        ((self.row_at(y0) * cols).min(last), (self.row_at(y1) * cols + cols - 1).min(last))
    }
}

/// Whether read mode lets an input event through (nothing that edits).
pub(super) fn read_mode_event(e: &egui::Event) -> bool {
    match e {
        egui::Event::Copy => true,
        egui::Event::Key { key, modifiers, .. } => {
            let nav = matches!(key, Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight | Key::Home | Key::End | Key::PageUp | Key::PageDown);
            nav || modifiers.command && matches!(key, Key::C | Key::A | Key::F)
        }
        _ => false,
    }
}

impl DocView {
    /// Read mode and focus hide the toolstrip, tabs and status bar.
    pub fn immersive(&self) -> bool {
        matches!(self.views.mode, ViewMode::Read | ViewMode::Focus)
    }

    /// The toolstrip tab the view asks for, once.
    pub fn take_wanted_tab(&mut self) -> Option<&'static str> {
        self.views.wanted_tab.take()
    }

    pub(super) fn set_view(&mut self, mode: ViewMode) {
        let old = self.views.mode;
        if old == mode {
            return;
        }
        self.views.mode = mode;
        match mode {
            ViewMode::Read => {
                self.views.saved = Some((self.zoom, self.views.cols));
                self.views.cols = if self.viewport_w > 2.2 * self.doc.page.width * PT * 0.6 { 2 } else { 1 };
                self.fit = Some(common::Fit::WholePage);
                self.views.refit = 2;
            }
            ViewMode::Outline => self.views.wanted_tab = Some("outline"),
            _ => {}
        }
        if old == ViewMode::Read
            && let Some((zoom, cols)) = self.views.saved.take()
        {
            self.zoom = zoom;
            self.views.cols = cols;
        }
        if old == ViewMode::Outline {
            self.views.wanted_tab = Some("view");
        }
        self.scroll_to_caret = true;
        self.focus = true;
    }

    /// Pages to a row the layout uses.
    pub(super) fn page_cols(&self) -> usize {
        match self.views.mode {
            ViewMode::Print | ViewMode::Read | ViewMode::Focus => self.views.cols.max(1),
            _ => 1,
        }
    }

    /// Places the pages as the view shows them (after laying out).
    pub(super) fn arrange_pages(&mut self) {
        if self.views.mode == ViewMode::Draft {
            self.layout.arrange_draft();
        } else {
            self.layout.arrange_rows(self.page_cols());
        }
    }

    /// The zoom that fits `fit` in a view of `avail` pixels.
    pub(super) fn fit_zoom(&self, fit: common::Fit, avail: Vec2) -> f32 {
        let cols = self.page_cols() as f32;
        let pw = self.doc.page.width * PT;
        let ph = self.doc.page.height * PT;
        let w = (avail.x - (cols + 1.0) * PAGE_GAP - 16.0) / (cols * pw);
        match fit {
            common::Fit::PageWidth => w,
            common::Fit::WholePage => w.min((avail.y - 2.0 * PAGE_GAP) / ph),
        }
        .clamp(0.1, 4.0)
    }

    /// Shows `n` pages to a row, zoomed to fit.
    pub(super) fn show_pages(&mut self, n: usize) {
        if !matches!(self.views.mode, ViewMode::Print | ViewMode::Read | ViewMode::Focus) {
            self.views.mode = ViewMode::Print;
        }
        self.views.cols = n.max(1);
        self.fit = Some(common::Fit::WholePage);
    }

    pub(super) fn toggle_split(&mut self) {
        self.views.split = match self.views.split {
            Some(_) => None,
            None => Some(0.5),
        };
        self.views.pane = 0;
        self.focus = true;
    }

    /// Keys of read mode and focus: Esc leaves them; read mode turns the
    /// pages with the arrows.
    pub(super) fn view_keys(&mut self, ctx: &egui::Context) {
        if !self.immersive() || ctx.memory(|m| m.top_modal_layer().is_some()) || self.find.is_some() {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.set_view(ViewMode::Print);
            return;
        }
        if self.views.mode != ViewMode::Read {
            return;
        }
        let step = |i: &mut egui::InputState, keys: &[Key]| keys.iter().any(|k| i.consume_key(Modifiers::NONE, *k));
        if ctx.input_mut(|i| step(i, &[Key::ArrowRight, Key::PageDown, Key::Space])) {
            self.turn_page(true);
        } else if ctx.input_mut(|i| step(i, &[Key::ArrowLeft, Key::PageUp, Key::Backspace])) {
            self.turn_page(false);
        }
    }

    /// Read mode: the next or previous row of pages.
    fn turn_page(&mut self, forward: bool) {
        let cols = self.layout.cols.max(1);
        let row = self.views.top_page / cols;
        let target = if forward { (row + 1) * cols } else { row.saturating_sub(1) * cols };
        if target < self.layout.pages {
            self.go_to_page(target);
            self.views.top_page = target;
            self.views.scroll_to_page = Some(target);
        }
    }

    /// The bar above the pages in read mode.
    pub(super) fn read_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
            crate::icons::paint(ui.painter(), r, Icon::ReadMode, ui.visuals().text_color());
            ui.label(RichText::new(tr("view.read_mode")).strong());
            ui.add_space(16.0);
            let page = self.views.top_page + 1;
            // Off at the first and the last row of pages.
            let (cols, row) = (self.layout.cols.max(1), self.views.top_page / self.layout.cols.max(1));
            if ui.add_enabled(row > 0, egui::Button::new("◀")).on_hover_text(tr("view.prev_screen")).clicked() {
                self.turn_page(false);
            }
            ui.label(trf("doc.page_of", &[("n", &page), ("total", &self.layout.pages)]));
            if ui.add_enabled((row + 1) * cols < self.layout.pages, egui::Button::new("▶")).on_hover_text(tr("view.next_screen")).clicked() {
                self.turn_page(true);
            }
            ui.add_space(16.0);
            for n in [1, 2] {
                let key = if n == 1 { "view.one_page" } else { "view.two_pages" };
                if ui.selectable_label(self.views.cols == n, tr(key)).clicked() {
                    self.show_pages(n);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(format!("{} (Esc)", tr("view.exit_read"))).clicked() {
                    self.set_view(ViewMode::Print);
                }
            });
        });
    }

    /// Focus: a quiet button to leave it, top right.
    pub(super) fn focus_exit(&mut self, ctx: &egui::Context) {
        let screen = ctx.content_rect();
        egui::Area::new(Id::new(("focus_exit", self.uid))).fixed_pos(pos2(screen.max.x - 230.0, screen.min.y + 8.0)).order(egui::Order::Foreground).show(ctx, |ui| {
            let near = ctx.pointer_hover_pos().is_some_and(|p| p.y < screen.min.y + 80.0);
            ui.set_opacity(if near { 1.0 } else { 0.6 });
            if ui.button(format!("{} (Esc)", tr("view.exit_focus"))).clicked() {
                self.set_view(ViewMode::Print);
            }
        });
    }

    /// The document area: one pane, or two with a bar between them.
    pub(super) fn panes(&mut self, ui: &mut egui::Ui) {
        if self.views.refit > 0 {
            self.views.refit -= 1;
            if self.views.refit == 0 {
                self.fit = Some(common::Fit::WholePage);
            }
        }
        let Some(frac) = self.views.split else {
            self.views.drawing = 0;
            self.canvas(ui);
            return;
        };
        let full = ui.available_rect_before_wrap();
        let bar = 6.0;
        let top_h = ((full.height() - bar) * frac).clamp(40.0, (full.height() - bar - 40.0).max(40.0));
        let rects = [Rect::from_min_size(full.min, vec2(full.width(), top_h)), Rect::from_min_max(pos2(full.min.x, full.min.y + top_h + bar), full.max)];
        let bar_rect = Rect::from_min_size(pos2(full.min.x, full.min.y + top_h), vec2(full.width(), bar));
        // A press in a pane makes it the one the caret scrolls in.
        if let Some(p) = ui.input(|i| if i.pointer.any_pressed() { i.pointer.interact_pos() } else { None })
            && ui.memory(|m| m.allows_interaction(ui.layer_id()))
            && ui.ctx().layer_id_at(p).is_none_or(|l| l.order == egui::Order::Background)
        {
            if let Some(k) = rects.iter().position(|r| r.contains(p)) {
                self.views.pane = k;
            }
        }
        for (k, r) in rects.into_iter().enumerate() {
            self.views.drawing = k;
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).id_salt(("doc_pane", k)));
            child.set_clip_rect(r);
            self.canvas(&mut child);
        }
        self.views.drawing = self.views.pane;
        let resp = ui.interact(bar_rect, Id::new(("doc_split", self.uid)), Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::ResizeVertical);
        let v = ui.visuals();
        ui.painter().rect_filled(bar_rect, 0.0, v.widgets.noninteractive.bg_stroke.color);
        if resp.dragged()
            && let Some(p) = resp.interact_pointer_pos()
        {
            self.views.split = Some(((p.y - full.min.y - bar / 2.0) / (full.height() - bar)).clamp(0.05, 0.95));
        }
        if resp.double_clicked() {
            self.views.split = None;
        }
    }

    /// The View tab's Views group.
    pub(super) fn views_group(&mut self, b: &mut crate::toolstrip::Body) {
        use crate::toolstrip;
        let a = b.accent();
        b.group("toolstrip.g.views", Icon::PrintLayout, |ui| {
            let m = self.views.mode;
            for (mode, icon, key, tip) in [
                (ViewMode::Read, Icon::ReadMode, "view.read_mode", "view.read_mode_tip"),
                (ViewMode::Print, Icon::PrintLayout, "view.print_layout", "view.print_layout_tip"),
                (ViewMode::Outline, Icon::Outline, "view.outline", "view.outline_tip"),
                (ViewMode::Draft, Icon::Draft, "view.draft", "view.draft_tip"),
                (ViewMode::Focus, Icon::Focus, "view.focus", "view.focus_tip"),
            ] {
                // A view of the pages leaves the Markdown source.
                if toolstrip::large(ui, icon, tr(key), tr(tip), a, m == mode && !self.md_source()).clicked() {
                    self.leave_md_source();
                    self.set_view(mode);
                }
            }
        });
        // Not a view: the document becomes a book for good.
        b.group("view.g_mode", Icon::Book, |ui| self.book_mode_button(ui, a));
    }

    /// The View tab's Zoom group's page buttons.
    pub(super) fn page_zoom_buttons(&mut self, ui: &mut egui::Ui, a: Color32) {
        use crate::toolstrip;
        toolstrip::column(ui, |ui| {
            if toolstrip::small(ui, Some(Icon::PageSize), tr("view.one_page"), "", a, false).clicked() {
                self.show_pages(1);
            }
            if toolstrip::small(ui, Some(Icon::MultiPage), tr("view.multi_page"), "", a, self.page_cols() > 1).clicked() {
                self.show_pages(2);
            }
            if toolstrip::small(ui, Some(Icon::Margins), tr("zoom.page_width"), "", a, false).clicked() {
                if self.views.mode == ViewMode::Print {
                    self.views.cols = 1;
                }
                self.fit = Some(common::Fit::PageWidth);
            }
        });
    }

    /// The document's buttons of the View tab's Window group.
    pub(crate) fn window_buttons(&mut self, ui: &mut egui::Ui, shell: &mut Shell, a: egui::Color32) {
        use crate::toolstrip;
        if toolstrip::large(ui, Icon::Window, tr("window.new"), tr("view.new_window_tip"), a, false).clicked() {
            shell.cmd = Some(common::AppCmd::NewWindow);
        }
        let on = self.views.split.is_some();
        let label = if on { tr("view.remove_split") } else { tr("window.split") };
        if toolstrip::large(ui, Icon::Split, label, tr("view.split_tip"), a, on).clicked() {
            self.toggle_split();
        }
    }

    /// A new window of the same document (View > New Window): the same
    /// place, zoom and view; the document itself is handed over.
    pub fn linked_window(&mut self) -> DocView {
        // Code typed here is the document's before it is shared.
        self.md_apply();
        let mut v = DocView::new(Document::default());
        v.copy_code_view(self);
        v.zoom = self.zoom;
        v.sel = self.sel;
        v.show_ruler = self.show_ruler;
        v.show_marks = self.show_marks;
        v.views.mode = if self.immersive() { ViewMode::Print } else { self.views.mode };
        v.views.cols = self.views.cols;
        v.scroll_to_caret = true;
        v
    }

    /// Hands the document to another window of it; each keeps its own
    /// selection, zoom and view.
    pub fn swap_document(&mut self, other: &mut DocView) {
        self.md_apply();
        other.md_apply();
        std::mem::swap(&mut self.doc, &mut other.doc);
        for v in [self, other] {
            v.layout.key = Default::default();
            v.sel = Selection { anchor: v.doc.clamp(v.sel.anchor), cursor: v.doc.clamp(v.sel.cursor) };
        }
    }
}
