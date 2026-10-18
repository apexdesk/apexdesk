//! The rulers' room beside the pages and the vertical ruler: the page of
//! the caret (or the one in view) in the ruler's unit, its top and bottom
//! margins to drag, the header and footer zones and, in a table, the edges
//! of its rows to drag.

use super::dialogs::ruler_unit;
use super::*;

/// Width of the vertical ruler and height of the horizontal one.
const RULER_W: f32 = 20.0;

/// Where the rulers go this frame (screen rects), kept before the pages
/// are laid out and drawn after them.
#[derive(Default)]
pub(super) struct RulerRects {
    h: Option<Rect>,
    v: Option<Rect>,
}

/// What a drag on the vertical ruler moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum VPart {
    MarginTop,
    MarginBottom,
    /// The bottom edge of a row: its table (layout index) and row.
    Row(u32, u32),
}

#[derive(Clone, Copy, Debug)]
pub(super) struct VDrag {
    part: VPart,
    /// Pointer y at the start of the drag.
    start: f32,
    /// Current offset in points.
    delta: f32,
}

/// The vertical ruler's page, in canvas y.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct VRulerGeo {
    pub page: usize,
    /// Top and bottom edges of the page.
    pub top: f32,
    pub bottom: f32,
    /// Where the text starts and ends (the margins' inner edges).
    pub text_top: f32,
    pub text_bottom: f32,
    /// The header's and the footer's distance from the page's edge.
    pub header: f32,
    pub footer: f32,
    /// Rows of the caret's table on the page: table, row, top and bottom;
    /// only rows whole on the page can be dragged.
    pub rows: Vec<(u32, u32, f32, f32, bool)>,
}

impl DocView {
    /// Whether the vertical ruler shows: with the ruler in print layout,
    /// and in the draft view when Options says so.
    pub(super) fn vruler_on(&self) -> bool {
        self.show_ruler
            && crate::settings::vruler()
            && (self.views.mode == ViewMode::Print || (self.views.mode == ViewMode::Draft && crate::settings::draft_vruler()))
            && self.hf_edit.is_none()
    }

    /// Keeps the rulers' room: the horizontal one over the pages, the
    /// vertical one on their left.
    pub(super) fn ruler_panels(&mut self, ui: &mut egui::Ui) -> RulerRects {
        let mut out = RulerRects::default();
        if !self.show_ruler || !matches!(self.views.mode, ViewMode::Print | ViewMode::Draft) {
            return out;
        }
        let bg = theme::workspace_bg(ui.visuals());
        let panel = |p: egui::Panel| p.exact_size(RULER_W).resizable(false).show_separator_line(false).frame(egui::Frame::new().fill(bg));
        out.h = Some(panel(egui::Panel::top(Id::new(("doc_ruler", self.uid)))).show(ui, |ui| ui.max_rect()).inner);
        if self.vruler_on() {
            out.v = Some(panel(egui::Panel::left(Id::new(("doc_vruler", self.uid)))).show(ui, |ui| ui.max_rect()).inner);
        }
        out
    }

    /// Draws the rulers in the room kept for them.
    pub(super) fn draw_rulers(&mut self, ui: &mut egui::Ui, r: RulerRects) {
        if let Some(rect) = r.h {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).id_salt("doc_ruler_ui"));
            child.set_clip_rect(rect);
            self.ruler(&mut child);
        }
        if let Some(rect) = r.v {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).id_salt("doc_vruler_ui"));
            child.set_clip_rect(rect);
            self.vruler(&mut child);
        }
    }

    /// The page the vertical ruler shows: the caret's while it's in view
    /// (canvas y `y0..y1`), else the one in the middle of the view.
    fn vruler_page(&self, y0: f32, y1: f32) -> usize {
        let caret = self.caret_seg().map_or(0, |s| s.page);
        let g = self.layout.geo_of(caret);
        if g.top < y1 && g.top + g.size.y > y0 {
            return caret;
        }
        self.layout.page_at_y((y0 + y1) / 2.0)
    }

    /// The vertical ruler's geometry for the view's canvas y `y0..y1`.
    pub(super) fn vruler_geo(&self, y0: f32, y1: f32) -> VRulerGeo {
        let pt = PT * self.zoom;
        let page = self.vruler_page(y0, y1);
        let g = self.layout.geo_of(page);
        let sects = self.doc.sections();
        let setup = *sects[g.sect.min(sects.len() - 1)].page;
        let o = self.content_origin(page);
        let (top, text_top, text_bottom) = if self.views.mode == ViewMode::Draft {
            // A draft page is only its text: the paper is put around it.
            let top = o.y - setup.text_top() * pt;
            (top, o.y, top + (setup.text_top() + setup.content_height()) * pt)
        } else {
            (g.top, o.y, o.y + g.content.y)
        };
        let bottom = if self.views.mode == ViewMode::Draft { top + setup.height * pt } else { g.top + g.size.y };
        // The caret's table, row by row.
        let mut rows = Vec::new();
        if let Some(&t) = self.layout.owner.get(self.sel.cursor.para)
            && t != u32::MAX
        {
            for f in self.layout.frags_on(page, page).iter().filter(|f| f.table == t && !f.ghost) {
                rows.push((f.table, f.row, o.y + f.y, o.y + f.y + f.h, f.top && f.bottom));
            }
        }
        VRulerGeo { page, top, bottom, text_top, text_bottom, header: setup.margin_header * pt, footer: setup.margin_footer * pt, rows }
    }

    pub(super) fn vruler(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let v = ui.visuals().clone();
        let p = ui.painter().with_clip_rect(rect);
        let pt = PT * self.zoom;
        let oy = self.canvas_origin.y;
        let geo = self.vruler_geo(rect.min.y - oy, rect.max.y - oy);
        let (unit, subdiv, _) = ruler_unit();
        let drag = self.vruler_drag;
        let d = |part: VPart| drag.filter(|d| d.part == part).map_or(0.0, |d| d.delta * pt);
        let y = |cy: f32| cy + oy;
        let strip = Rect::from_x_y_ranges(rect.min.x + 3.0..=rect.max.x - 3.0, y(geo.top)..=y(geo.bottom));
        let margin = if v.dark_mode { Color32::from_gray(70) } else { Color32::from_gray(205) };
        let paper = if v.dark_mode { Color32::from_gray(40) } else { Color32::WHITE };
        let zone = if v.dark_mode { Color32::from_gray(90) } else { Color32::from_gray(180) };
        let ink = v.weak_text_color();
        let mt = y(geo.text_top) + d(VPart::MarginTop);
        let mb = y(geo.text_bottom) - d(VPart::MarginBottom);
        p.rect_filled(strip, 0.0, margin);
        p.rect_filled(Rect::from_x_y_ranges(strip.x_range(), mt..=mb.max(mt)), 0.0, paper);
        // The header and footer zones: from their distance to the text.
        let band = Rect::from_x_y_ranges(strip.max.x - 4.0..=strip.max.x, strip.y_range());
        let head = strip.min.y + geo.header;
        let foot = strip.max.y - geo.footer;
        if head < mt {
            p.rect_filled(Rect::from_x_y_ranges(band.x_range(), head..=mt), 0.0, zone);
        }
        if foot > mb {
            p.rect_filled(Rect::from_x_y_ranges(band.x_range(), mb..=foot), 0.0, zone);
        }
        // Ticks, numbered from the top margin.
        let step = unit * pt / subdiv as f32;
        let mid = strip.center().x;
        let font = egui::FontId::proportional(9.0);
        for dir in [-1.0f32, 1.0] {
            let mut i = if dir > 0.0 { 0 } else { 1 };
            loop {
                let ty = mt + dir * i as f32 * step;
                if ty < strip.min.y - 0.5 || ty > strip.max.y + 0.5 {
                    break;
                }
                if ty >= rect.min.y - 10.0 && ty <= rect.max.y + 10.0 {
                    if i % subdiv == 0 {
                        if i > 0 {
                            p.text(pos2(mid, ty), egui::Align2::CENTER_CENTER, (i / subdiv).to_string(), font.clone(), ink);
                        }
                    } else {
                        let w = if subdiv % 2 == 0 && i % (subdiv / 2) == 0 { 3.0 } else { 1.5 };
                        p.line_segment([pos2(mid - w, ty), pos2(mid + w, ty)], Stroke::new(1.0, ink));
                    }
                }
                i += 1;
            }
        }
        // The table's row edges.
        let mark = if v.dark_mode { Color32::from_gray(190) } else { Color32::from_gray(90) };
        let row_y = |t: u32, r: u32, y1: f32| y(y1) + d(VPart::Row(t, r));
        for &(t, r, _, y1, whole) in &geo.rows {
            let ry = row_y(t, r, y1);
            let fill = if whole { mark } else { zone };
            p.rect_filled(Rect::from_x_y_ranges(strip.min.x + 1.0..=strip.max.x - 1.0, ry - 1.5..=ry + 1.5), 1.0, fill);
        }
        if let Some(dr) = drag {
            let at = match dr.part {
                VPart::MarginTop => Some(mt),
                VPart::MarginBottom => Some(mb),
                VPart::Row(t, r) => geo.rows.iter().find(|x| x.0 == t && x.1 == r).map(|x| row_y(t, r, x.3)),
            };
            if let Some(at) = at {
                ui.ctx().debug_painter().hline(rect.max.x..=rect.max.x + self.viewport_w, at, Stroke::new(1.0, Color32::from_gray(128)));
            }
        }

        // Interaction.
        let resp = ui.interact(rect, Id::new(("vruler", self.uid)), Sense::drag());
        let hit = |py: f32| {
            let near = |at: f32| (py - at).abs() <= 4.0;
            if let Some(&(t, r, ..)) = geo.rows.iter().find(|x| x.4 && near(y(x.3))) {
                Some(VPart::Row(t, r))
            } else if near(mt) {
                Some(VPart::MarginTop)
            } else if near(mb) {
                Some(VPart::MarginBottom)
            } else {
                None
            }
        };
        if let Some(h) = resp.hover_pos()
            && drag.is_none()
            && let Some(part) = hit(h.y)
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
            let key = match part {
                VPart::MarginTop => "vruler.margin_top",
                VPart::MarginBottom => "vruler.margin_bottom",
                VPart::Row(..) => "vruler.row",
            };
            resp.clone().on_hover_text_at_pointer(tr(key));
        }
        // A drag starts where the button went down: by the time egui calls
        // it a drag the pointer has left the 4 px around the marker.
        if resp.drag_started()
            && let Some(h) = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos())
            && let Some(part) = hit(h.y)
        {
            self.vruler_drag = Some(VDrag { part, start: h.y, delta: 0.0 });
        }
        if let Some(dr) = &mut self.vruler_drag
            && let Some(h) = resp.interact_pointer_pos()
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
            let raw = (h.y - dr.start) / pt;
            // Snap to the ruler subdivisions unless Alt is held.
            let snap = unit / subdiv as f32;
            let raw = if ui.input(|i| i.modifiers.alt) { raw } else { (raw / snap).round() * snap };
            dr.delta = if dr.part == VPart::MarginBottom { -raw } else { raw };
        }
        if resp.drag_stopped()
            && let Some(dr) = self.vruler_drag.take()
        {
            let rows = geo.rows.clone();
            self.commit_vruler(dr.part, dr.delta, &rows, pt);
        }
    }

    /// Commits a drag of `dv` points on the vertical ruler, one undo step.
    pub(super) fn commit_vruler(&mut self, part: VPart, dv: f32, rows: &[(u32, u32, f32, f32, bool)], pt: f32) {
        if dv == 0.0 {
            return;
        }
        let min_content = 72.0;
        match part {
            VPart::MarginTop | VPart::MarginBottom => {
                let top = part == VPart::MarginTop;
                self.change_page(move |pg| {
                    let (m, other) = if top { (&mut pg.margin_top, pg.margin_bottom) } else { (&mut pg.margin_bottom, pg.margin_top) };
                    *m = (*m + dv).clamp(0.0, (pg.height - other - min_content).max(0.0));
                });
            }
            VPart::Row(t, r) => {
                let Some(&(_, _, y0, y1, _)) = rows.iter().find(|x| x.0 == t && x.1 == r) else { return };
                let Some(tb) = self.layout.tables.get(t as usize) else { return };
                // A cell of that row: its table's props are the row's.
                let Some(para) = (tb.map.start..tb.map.end).find(|&i| tb.map.pos_of(&self.doc.paragraphs, i).0 == r as usize) else { return };
                let h = ((y1 - y0) / pt + dv).max(4.0);
                let r = r as usize;
                self.doc.set_table_props(Selection::caret(Pos::new(para, 0)), &move |p: &mut apex_doc::TableProps| {
                    if p.row_heights.len() <= r {
                        p.row_heights.resize(r + 1, 0.0);
                    }
                    p.row_heights[r] = h;
                });
                self.scroll_to_caret = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A document laid out at `zoom` in `mode`, the canvas at a place on
    /// the screen as the view would put it (scrolled, a pane on its left).
    fn laid_out(doc: Document, zoom: f32, mode: ViewMode) -> DocView {
        let ctx = egui::Context::default();
        fonts::install(&ctx);
        let frame = || ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        frame();
        frame();
        let mut v = DocView::new(doc);
        v.zoom = zoom;
        v.views.mode = mode;
        v.layout = v.build_layout(&ctx, zoom, (1, 0, 0, 0, 0), Layout::default());
        v.arrange_pages();
        v.canvas_origin = pos2(260.0, -900.0);
        v.page_x = 410.0;
        v
    }

    fn long_doc() -> Document {
        Document::from_paragraphs((0..120).map(|i| Paragraph::new(format!("Paragraph {i} with some words to fill the line"))).collect())
    }

    fn setup(v: &DocView) -> apex_doc::PageSetup {
        *v.doc.section_setup(0).0
    }

    /// Screen x of the ruler's zero, where it numbers from.
    fn ruler_zero(v: &DocView) -> f32 {
        let (page, page_l, col) = v.ruler_frame();
        let pt = PT * v.zoom;
        v.canvas_origin.x + page_l + col.map_or(page.margin_left * pt, |c| c.0)
    }

    #[test]
    fn the_ruler_stands_on_the_text() {
        for mode in [ViewMode::Print, ViewMode::Draft] {
            for zoom in [0.75, 1.0, 1.6] {
                let mut v = laid_out(long_doc(), zoom, mode);
                assert!(v.layout.pages > 1);
                // On the first page and a later one.
                for para in [0, 100] {
                    v.sel = Selection::caret(Pos::new(para, 0));
                    let caret = v.caret_rect(v.sel.cursor).min.x + v.canvas_origin.x;
                    assert!((ruler_zero(&v) - caret).abs() < 0.5, "{mode:?} {zoom} {para}: ruler {} text {caret}", ruler_zero(&v));
                }
            }
        }
        // Pages side by side: the caret's page.
        let mut v = laid_out(long_doc(), 1.0, ViewMode::Print);
        v.views.cols = 2;
        v.arrange_pages();
        let para = (0..120).find(|&i| {
            v.sel = Selection::caret(Pos::new(i, 0));
            v.caret_seg().unwrap().page == 1
        });
        assert!(para.is_some());
        let caret = v.caret_rect(v.sel.cursor).min.x + v.canvas_origin.x;
        assert!((ruler_zero(&v) - caret).abs() < 0.5);
    }

    #[test]
    fn the_vertical_ruler_follows_the_page() {
        let mut v = laid_out(long_doc(), 1.25, ViewMode::Print);
        let pt = PT * v.zoom;
        v.sel = Selection::caret(Pos::new(100, 0));
        let page = v.caret_seg().unwrap().page;
        assert!(page > 0);
        let g = v.layout.geo_of(page);
        // The caret's page while in view; its top margin where the text starts.
        let geo = v.vruler_geo(g.top, g.top + 500.0);
        assert_eq!(geo.page, page);
        assert!((geo.text_top - v.content_origin(page).y).abs() < 0.5);
        let s = setup(&v);
        assert!((geo.text_top - geo.top - s.margin_top * pt).abs() < 0.5);
        assert!((geo.bottom - geo.text_bottom - s.margin_bottom * pt).abs() < 1.0);
        // Scrolled away from it: the page in view.
        let g0 = v.layout.geo_of(0);
        assert_eq!(v.vruler_geo(g0.top, g0.top + 300.0).page, 0);
        // Dragging the top margin down 1 cm is one undo step.
        v.commit_vruler(VPart::MarginTop, 28.3465, &[], pt);
        assert!((setup(&v).margin_top - s.margin_top - 28.3465).abs() < 0.01);
        v.doc.undo();
        assert!((setup(&v).margin_top - s.margin_top).abs() < 0.01);
        v.commit_vruler(VPart::MarginBottom, -10.0, &[], pt);
        assert!((setup(&v).margin_bottom - s.margin_bottom + 10.0).abs() < 0.01);
        // The draft view puts the paper around the text.
        let mut d = laid_out(long_doc(), 1.0, ViewMode::Draft);
        d.sel = Selection::caret(Pos::new(0, 0));
        let geo = d.vruler_geo(0.0, 400.0);
        assert!((geo.text_top - d.content_origin(0).y).abs() < 0.5);
        assert!((geo.text_top - geo.top - s.margin_top * PT).abs() < 0.5);
    }

    #[test]
    fn rows_are_dragged_on_the_vertical_ruler() {
        let mut doc = Document::from_paragraphs(vec![Paragraph::new("Before")]);
        doc.insert_table(Selection::caret(Pos::new(0, 6)), 3, 2, 400.0);
        let mut v = laid_out(doc, 1.0, ViewMode::Print);
        let cell = (0..v.doc.paragraphs.len()).find(|&i| v.doc.table_map(i).is_some()).unwrap();
        v.sel = Selection::caret(Pos::new(cell, 0));
        let pt = PT * v.zoom;
        let geo = v.vruler_geo(0.0, 800.0);
        assert_eq!(geo.rows.len(), 3);
        let (t, r, y0, y1, whole) = geo.rows[1];
        assert!(whole && y1 > y0);
        v.commit_vruler(VPart::Row(t, r), 20.0, &geo.rows, pt);
        let want = (y1 - y0) / pt + 20.0;
        let props = |v: &DocView| v.doc.table_map(cell).unwrap().props(&v.doc.paragraphs).cloned().unwrap_or_default();
        assert!((props(&v).row_heights[1] - want).abs() < 0.01, "{:?}", props(&v).row_heights);
        v.doc.undo();
        assert!(props(&v).row_heights.get(1).is_none_or(|h| *h == 0.0));
    }

    /// The whole view drawn as the app does, the right-click on the ruler
    /// in the text's width.
    #[test]
    fn a_right_click_on_the_ruler_opens_its_tab_menu() {
        let ctx = egui::Context::default();
        fonts::install(&ctx);
        let mut v = DocView::new(Document::from_paragraphs(vec![Paragraph::new("Tabs\tare\there")]));
        let mut shell = crate::common::Shell { cmd: None, show_formula_bar: false, file_name: "n".into(), fullscreen: false, dark: None, others: Vec::new(), books: Vec::new(), side: None, recording: None };
        let mut frame = |v: &mut DocView, events: Vec<egui::Event>| {
            let input = egui::RawInput { events, screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 900.0))), ..Default::default() };
            ctx.run_ui(input, |ui| v.ui(ui, &mut shell)).textures_delta.clear();
        };
        for _ in 0..3 {
            frame(&mut v, vec![]);
        }
        let ruler = ctx.read_response(Id::new(("ruler", v.uid))).expect("the ruler shown").rect;
        let p = pos2(ruler_zero(&v) + 100.0, ruler.center().y);
        let button = |pressed| egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed, modifiers: Default::default() };
        frame(&mut v, vec![egui::Event::PointerMoved(p), button(true)]);
        frame(&mut v, vec![button(false)]);
        frame(&mut v, vec![]);
        frame(&mut v, vec![]);
        assert!(egui::Popup::is_any_open(&ctx));
    }

    /// With no indent the indent markers stand on the margins' edges: a
    /// drag in the ruler's middle moves the margin, one on its bottom edge
    /// the left indent.
    #[test]
    fn the_margins_drag_from_under_the_indent_markers() {
        let ctx = egui::Context::default();
        fonts::install(&ctx);
        let mut v = DocView::new(Document::from_paragraphs(vec![Paragraph::new("Text")]));
        let mut shell = crate::common::Shell { cmd: None, show_formula_bar: false, file_name: "n".into(), fullscreen: false, dark: None, others: Vec::new(), books: Vec::new(), side: None, recording: None };
        let mut frame = |v: &mut DocView, events: Vec<egui::Event>| {
            let input = egui::RawInput { events, screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 900.0))), ..Default::default() };
            ctx.run_ui(input, |ui| v.ui(ui, &mut shell)).textures_delta.clear();
        };
        for _ in 0..3 {
            frame(&mut v, vec![]);
        }
        let ruler = ctx.read_response(Id::new(("ruler", v.uid))).expect("the ruler shown").rect;
        let drag = |v: &mut DocView, frame: &mut dyn FnMut(&mut DocView, Vec<egui::Event>), y: f32| {
            let (a, b) = (pos2(ruler_zero(v), y), pos2(ruler_zero(v) + 60.0, y));
            let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
            frame(v, vec![egui::Event::PointerMoved(a), button(a, true)]);
            for i in 1..=6 {
                frame(v, vec![egui::Event::PointerMoved(a + vec2(10.0 * i as f32, 0.0))]);
            }
            frame(v, vec![button(b, false)]);
            frame(v, vec![]);
        };
        let (margin, indent) = (setup(&v).margin_left, v.cur_para().style.indent_left);
        drag(&mut v, &mut frame, ruler.center().y);
        assert!(setup(&v).margin_left > margin + 20.0, "{} -> {}", margin, setup(&v).margin_left);
        assert_eq!(v.cur_para().style.indent_left, indent);
        let margin = setup(&v).margin_left;
        drag(&mut v, &mut frame, ruler.max.y - 2.0);
        assert_eq!(setup(&v).margin_left, margin);
        assert!(v.cur_para().style.indent_left > indent + 20.0);
    }

    #[test]
    fn the_top_margin_drags_on_the_vertical_ruler() {
        let ctx = egui::Context::default();
        fonts::install(&ctx);
        let mut v = DocView::new(Document::from_paragraphs(vec![Paragraph::new("Text")]));
        let mut shell = crate::common::Shell { cmd: None, show_formula_bar: false, file_name: "n".into(), fullscreen: false, dark: None, others: Vec::new(), books: Vec::new(), side: None, recording: None };
        let mut frame = |v: &mut DocView, events: Vec<egui::Event>| {
            let input = egui::RawInput { events, screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 900.0))), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| v.ui(ui, &mut shell));
            out.textures_delta.clear();
            out.platform_output.cursor_icon
        };
        for _ in 0..3 {
            frame(&mut v, vec![]);
        }
        let ruler = ctx.read_response(Id::new(("vruler", v.uid))).expect("the vertical ruler shown").rect;
        // The margin's marker is where the pointer turns into a resize arrow.
        let x = ruler.center().x;
        let y = (ruler.min.y as i32..ruler.max.y as i32)
            .map(|y| y as f32)
            .find(|&y| frame(&mut v, vec![egui::Event::PointerMoved(pos2(x, y))]) == egui::CursorIcon::ResizeVertical)
            .expect("a margin marker");
        let a = pos2(x, y + 2.0);
        let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        let margin = setup(&v).margin_top;
        frame(&mut v, vec![egui::Event::PointerMoved(a), button(a, true)]);
        for i in 1..=6 {
            frame(&mut v, vec![egui::Event::PointerMoved(a + vec2(0.0, 10.0 * i as f32))]);
        }
        frame(&mut v, vec![button(a + vec2(0.0, 60.0), false)]);
        frame(&mut v, vec![]);
        assert!(setup(&v).margin_top > margin + 20.0, "{} -> {}", margin, setup(&v).margin_top);
    }
}
