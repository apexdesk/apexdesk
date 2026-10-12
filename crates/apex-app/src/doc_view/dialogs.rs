//! Word processor find bar, dialogs (paragraph, go to page, hyperlink,
//! bookmark) and ruler.

use super::*;
use crate::widgets::DialogResult;

#[derive(Default)]
pub struct FindBar {
    pub query: String,
    pub replace: String,
    pub show_replace: bool,
    pub case: bool,
    pub whole: bool,
    pub focus: bool,
    pub hits: Vec<(Pos, Pos)>,
    pub hits_key: Option<(u64, String, apex_doc::FindOpts)>,
    /// Opened by the navigation pane's search box (the bar stays hidden).
    pub nav: bool,
    /// The "More" options are shown.
    pub more: bool,
    pub mode: apex_doc::FindMode,
    /// Formatting to find and to give the replacement.
    pub format: apex_doc::FindFormat,
    pub rformat: apex_doc::FindFormat,
    /// The format row shown: 1 find, 2 replace.
    pub fmt_open: u8,
    /// Why the query is invalid.
    pub error: Option<String>,
}

impl FindBar {
    pub fn opts(&self) -> apex_doc::FindOpts {
        let more = self.more;
        apex_doc::FindOpts { case: self.case, whole: self.whole, mode: if more { self.mode } else { Default::default() }, format: if more { self.format.clone() } else { Default::default() } }
    }

    /// Something to look for.
    pub fn active(&self) -> bool {
        !self.query.is_empty() || (self.more && !self.format.is_empty())
    }
}

pub enum DocDialog {
    Paragraph(Box<ParaDlg>),
    GoTo(Box<find_ui::GoToDlg>),
    Link(Box<LinkDlg>),
    Bookmark(Box<BookmarkDlg>),
}

/// Insert / edit hyperlink.
pub struct LinkDlg {
    /// Text to display; `None` when the selection spans paragraphs and is
    /// linked as it is.
    text: Option<String>,
    orig_text: String,
    address: String,
    tip: String,
    /// The existing link or the selection.
    range: Selection,
    editing: bool,
    remove: bool,
    focus: bool,
}

pub struct BookmarkDlg {
    name: String,
    by_location: bool,
    error: bool,
    focus: bool,
}

/// What the user typed as a link address, as a link target: e-mail
/// addresses get "mailto:", "www." gets "http://"; "#name" is a bookmark.
fn normalize_address(s: &str) -> String {
    let s = s.trim();
    if s.starts_with('#') {
        return s.to_string();
    }
    apex_doc::autolink_target(s).unwrap_or_else(|| s.to_string())
}

/// Text shown for a link inserted without display text.
fn link_display(target: &str) -> &str {
    target.strip_prefix("mailto:").or_else(|| target.strip_prefix('#')).unwrap_or(target)
}

/// Editable paragraph settings; indents in points.
#[derive(Clone, Copy, PartialEq)]
pub struct ParaVals {
    align: Align,
    left: f32,
    right: f32,
    /// 0 none, 1 first line, 2 hanging.
    special: u8,
    by: f32,
    before: f32,
    after: f32,
    /// 0 single, 1 1.5 lines, 2 double, 3 at least, 4 exactly, 5 multiple.
    line: u8,
    at: f32,
    keep_next: bool,
    page_break: bool,
    keep_lines: bool,
    widows: bool,
    no_line_numbers: bool,
}

pub struct ParaDlg {
    orig: ParaVals,
    cur: ParaVals,
    page: usize,
}

impl ParaVals {
    fn from_style(st: &ParaStyle) -> Self {
        let (before, after) = st.spacing();
        let (line, at) = match st.line_rule {
            LineRule::AtLeast => (3, st.line_spacing),
            LineRule::Exact => (4, st.line_spacing),
            LineRule::Multiple => match st.line_spacing {
                v if v <= 0.0 || (v - 1.0).abs() < 0.01 => (0, 1.0),
                v if (v - 1.5).abs() < 0.01 => (1, 1.5),
                v if (v - 2.0).abs() < 0.01 => (2, 2.0),
                v => (5, v),
            },
        };
        ParaVals {
            align: st.align,
            left: st.indent_left,
            right: st.indent_right,
            special: if st.first_line > 0.0 { 1 } else if st.first_line < 0.0 { 2 } else { 0 },
            by: st.first_line.abs(),
            before,
            after,
            line,
            at,
            keep_next: st.keep_with_next,
            page_break: st.page_break_before,
            keep_lines: st.keep_lines,
            widows: !st.no_widow_control,
            no_line_numbers: st.no_line_numbers,
        }
    }
}

/// What a ruler drag moves.
#[derive(Clone, Copy, PartialEq)]
pub enum RulerPart {
    FirstLine,
    Left,
    Right,
    MarginLeft,
    MarginRight,
}

#[derive(Clone, Copy)]
pub struct RulerDrag {
    part: RulerPart,
    /// Pointer x at the start of the drag.
    start: f32,
    /// Current offset in points.
    delta: f32,
}

/// Points per ruler unit and the unit's subdivisions.
pub(super) fn ruler_unit() -> (f32, u32, &'static str) {
    if tr("ruler.unit") == "cm" { (28.3465, 2, "cm") } else { (72.0, 8, "in") }
}

impl DocView {
    pub(super) fn find_bar(&mut self, ui: &mut egui::Ui) {
        let mut action = 0u8;
        let counter = self.find_counter();
        let styles = if self.find.as_ref().is_some_and(|f| f.more) { self.style_list() } else { Vec::new() };
        // In a narrow window the row goes on to a second line.
        ui.horizontal_wrapped(|ui| {
            let Some(f) = &mut self.find else { return };
            let q = ui.add(
                egui::TextEdit::singleline(&mut f.query)
                    .hint_text(tr("find.hint"))
                    .desired_width(200.0)
                    .event_filter(EventFilter { escape: true, ..Default::default() }),
            );
            if f.focus {
                q.request_focus();
                f.focus = false;
            }
            if q.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                action = if ui.input(|i| i.modifiers.shift) { 2 } else { 1 };
            }
            if q.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
                action = 5;
            }
            if let Some(e) = &f.error {
                ui.label(RichText::new(tr("find.bad_pattern")).color(ui.visuals().error_fg_color)).on_hover_text(e);
            } else if let Some(c) = &counter {
                ui.label(RichText::new(c).weak());
            }
            // Nothing to look for: nothing to step through or replace.
            let query = !f.query.is_empty();
            ui.add_enabled_ui(query, |ui| {
                if widgets::tool_button(ui, widgets::Face::Icon(Icon::ChevronUp), false, &format!("{} (Shift+Enter)", tr("find.prev"))).clicked() {
                    action = 2;
                }
                if widgets::tool_button(ui, widgets::Face::Icon(Icon::ChevronDown), false, &format!("{} (Enter, F3)", tr("find.next"))).clicked() {
                    action = 1;
                }
            });
            ui.checkbox(&mut f.case, tr("find.case"));
            ui.checkbox(&mut f.whole, tr("find.whole_word"));
            let more = if f.more { format!("{} <<", tr("find.less")) } else { format!("{} >>", tr("find.more")) };
            if ui.selectable_label(f.more, more).clicked() {
                f.more = !f.more;
            }
            if f.show_replace {
                ui.separator();
                ui.add(egui::TextEdit::singleline(&mut f.replace).hint_text(tr("find.replace_hint")).desired_width(180.0));
                ui.add_enabled_ui(query, |ui| {
                    if ui.button(tr("find.replace")).clicked() {
                        action = 3;
                    }
                    if ui.button(tr("find.replace_all")).clicked() {
                        action = 4;
                    }
                });
            } else if ui.button(tr("find.show_replace")).clicked() {
                f.show_replace = true;
            }
            if ui.button("×").on_hover_text("Esc").clicked() {
                action = 5;
            }
        });
        if let Some(f) = self.find.as_mut().filter(|f| f.more) {
            find_ui::find_more(ui, f, &styles);
        }
        match action {
            1 | 2 => {
                self.find_next(action == 2);
                if let Some(f) = &mut self.find {
                    f.focus = true;
                }
            }
            3 => self.replace_one(),
            4 => self.replace_all(),
            5 => {
                self.find = None;
                self.focus = true;
            }
            _ => {}
        }
    }

    /// "3 of 12" when the selection is a match, "12 matches" otherwise.
    fn find_counter(&mut self) -> Option<String> {
        if self.find.as_ref().is_none_or(|f| !f.active()) {
            return None;
        }
        let cur = (self.sel.start(), self.sel.end());
        let hits = self.find_hits();
        let n = hits.len();
        Some(match hits.iter().position(|h| *h == cur) {
            Some(i) => trf("find.count", &[("i", &(i + 1)), ("n", &n)]),
            None if n == 1 => tr("find.one_match").to_string(),
            None => trf("find.matches", &[("n", &n)]),
        })
    }

    pub(super) fn open_paragraph_dialog(&mut self) {
        let v = ParaVals::from_style(&self.cur_para().style);
        self.dialog = Some(DocDialog::Paragraph(Box::new(ParaDlg { orig: v, cur: v, page: 0 })));
    }

    /// Ctrl+K: edits the link at the caret (or `hit`), or links the selection.
    pub(super) fn open_link_dialog(&mut self, hit: Option<LinkHit>) {
        let c = self.sel.start();
        let hit = hit.or_else(|| self.doc.link_at(c).filter(|(r, _, _)| r.start() <= c && c < r.end() && self.sel.end() <= r.end()));
        let range = hit.as_ref().map_or(self.sel, |h| h.0);
        let (a, b) = (range.start(), range.end());
        let orig_text = self.doc.text_range(a, b);
        let text = (a.para == b.para).then(|| orig_text.clone());
        let (address, tip) = hit.map_or((String::new(), String::new()), |(_, l, t)| (l.to_string(), t.as_deref().unwrap_or_default().to_string()));
        let editing = !address.is_empty();
        self.dialog = Some(DocDialog::Link(Box::new(LinkDlg { text, orig_text, address, tip, range, editing, remove: false, focus: true })));
    }

    pub(super) fn open_bookmark_dialog(&mut self) {
        let c = self.sel.cursor;
        let name = self.doc.bookmarks().into_iter().filter(|b| !b.is_hidden() && b.start <= c && c <= b.end).last().map_or(String::new(), |b| b.name.to_string());
        self.dialog = Some(DocDialog::Bookmark(Box::new(BookmarkDlg { name, by_location: false, error: false, focus: true })));
    }

    fn apply_link(&mut self, d: LinkDlg) {
        if d.remove {
            self.doc.set_link(d.range, None, None);
            return;
        }
        let addr = normalize_address(&d.address);
        if addr.is_empty() {
            // Clearing the address removes the link.
            if d.editing {
                self.doc.set_link(d.range, None, None);
            }
            return;
        }
        let tip = Some(d.tip.trim()).filter(|t| !t.is_empty()).map(Arc::from);
        let link: Arc<str> = Arc::from(addr.as_str());
        match d.text {
            Some(t) if t != d.orig_text || d.range.is_empty() => {
                let t = if t.trim().is_empty() { link_display(&addr).to_string() } else { t };
                let s = self.doc.insert_link(d.range, &t, link, tip);
                self.sel = Selection::caret(s.end());
            }
            _ => {
                self.doc.set_link(d.range, Some(link), tip);
                self.sel = Selection::caret(d.range.end());
            }
        }
        self.scroll_to_caret = true;
    }

    /// The bookmark dialog: Add, Delete and Go To act at once; Add closes it.
    fn bookmark_dialog(&mut self, ctx: &egui::Context, b: &mut BookmarkDlg) -> DialogResult {
        let mut res = DialogResult::None;
        let mut list: Vec<apex_doc::Bookmark> = self.doc.bookmarks().into_iter().filter(|b| !b.is_hidden()).collect();
        if !b.by_location {
            list.sort_by_key(|b| b.name.to_lowercase());
        }
        let key = b.name.trim().to_lowercase();
        let existing = list.iter().find(|x| x.name.to_lowercase() == key).cloned();
        let (mut add, mut delete, mut goto) = (false, false, false);
        let m = egui::Modal::new(Id::new(("dialog", "doc_bookmark"))).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.heading(tr("bookmark.title"));
            ui.add_space(8.0);
            ui.label(tr("bookmark.name"));
            let r = ui.add(egui::TextEdit::singleline(&mut b.name).desired_width(f32::INFINITY));
            if std::mem::take(&mut b.focus) {
                r.request_focus();
            }
            if r.changed() {
                b.error = false;
            }
            ui.add_space(4.0);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::ScrollArea::vertical().max_height(150.0).min_scrolled_height(150.0).auto_shrink([false, false]).show(ui, |ui| {
                    if list.is_empty() {
                        ui.label(RichText::new(tr("bookmark.none")).weak());
                    }
                    for bm in &list {
                        let on = existing.as_ref().is_some_and(|e| e.name == bm.name);
                        let r = ui.selectable_label(on, bm.name.as_ref());
                        if r.clicked() {
                            b.name = bm.name.to_string();
                            b.error = false;
                        }
                        if r.double_clicked() {
                            goto = true;
                        }
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label(tr("bookmark.sort"));
                ui.radio_value(&mut b.by_location, false, tr("bookmark.sort_name"));
                ui.radio_value(&mut b.by_location, true, tr("bookmark.sort_location"));
            });
            if b.error {
                ui.colored_label(ui.visuals().error_fg_color, tr("bookmark.invalid"));
            }
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("bookmark.close")).clicked() {
                    res = DialogResult::Cancel;
                }
                if ui.add_enabled(existing.is_some(), egui::Button::new(tr("bookmark.goto"))).clicked() {
                    goto = true;
                }
                if ui.add_enabled(existing.is_some(), egui::Button::new(tr("bookmark.delete"))).clicked() {
                    delete = true;
                }
                if ui.add_enabled(!b.name.trim().is_empty(), egui::Button::new(RichText::new(tr("bookmark.add")).strong())).clicked() {
                    add = true;
                }
            });
        });
        if res == DialogResult::None {
            if ctx.input(|i| i.key_pressed(Key::Escape)) || m.should_close() && ctx.input(|i| i.pointer.any_click()) {
                res = DialogResult::Cancel;
            } else if ctx.input(|i| i.key_pressed(Key::Enter)) && !b.name.trim().is_empty() {
                add = true;
            }
        }
        if add {
            let name = b.name.trim().to_string();
            if apex_doc::valid_bookmark_name(&name) {
                // An existing name is redefined here, as in other editors.
                self.doc.add_bookmark(&name, self.sel);
                res = DialogResult::Ok;
            } else {
                b.error = true;
            }
        } else if delete {
            if let Some(e) = &existing {
                self.doc.remove_bookmark(&e.name, self.sel);
                b.name.clear();
            }
        } else if goto {
            if let Some(e) = existing.or_else(|| self.doc.bookmark(b.name.trim())) {
                self.go_to_bookmark(&e);
            }
        }
        res
    }

    pub(super) fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.dialog.take() else { return };
        let bookmarks: Vec<Arc<str>> = if matches!(d, DocDialog::Link(_)) {
            let mut v: Vec<Arc<str>> = self.doc.bookmarks().into_iter().filter(|b| !b.is_hidden()).map(|b| b.name).collect();
            v.sort_by_key(|n| n.to_lowercase());
            v
        } else {
            Vec::new()
        };
        let res = match &mut d {
            DocDialog::Bookmark(b) => self.bookmark_dialog(ctx, b),
            DocDialog::Link(l) => {
                let title = if l.editing { tr("link.edit_title") } else { tr("link.insert_title") };
                let r = widgets::dialog(ctx, "doc_link", title, 420.0, |ui| link_page(ui, l, &bookmarks));
                if l.remove { DialogResult::Ok } else { r }
            }
            DocDialog::GoTo(g) => self.goto_dialog(ctx, g),
            DocDialog::Paragraph(p) => {
                let pages = [(Icon::Indent, "para.tab_indents"), (Icon::PageBreak, "para.tab_breaks")];
                let ParaDlg { cur, page, .. } = &mut **p;
                let preview = *cur;
                widgets::nav_dialog_aside(ctx, "doc_para", tr("doc.paragraph"), &pages, page, vec2(390.0, 330.0), 170.0, |ui, page| para_page(ui, cur, page), |ui| {
                    widgets::section(ui, tr("para.preview"));
                    para_preview(ui, &preview);
                })
            }
        };
        match res {
            DialogResult::None => self.dialog = Some(d),
            DialogResult::Cancel => self.focus = true,
            DialogResult::Ok => {
                match d {
                    DocDialog::GoTo(_) => {}
                    DocDialog::Paragraph(p) => self.apply_para(p.orig, p.cur),
                    DocDialog::Link(l) => self.apply_link(*l),
                    DocDialog::Bookmark(_) => {}
                }
                self.focus = true;
            }
        }
    }

    /// Applies the fields the user changed (the others keep each paragraph's
    /// own value when several are selected).
    fn apply_para(&mut self, o: ParaVals, c: ParaVals) {
        if o == c {
            return;
        }
        self.para_style(move |s| {
            if c.align != o.align {
                s.align = c.align;
            }
            if c.left != o.left {
                s.indent_left = c.left;
            }
            if c.right != o.right {
                s.indent_right = c.right;
            }
            if (c.special, c.by) != (o.special, o.by) {
                s.first_line = match c.special {
                    1 => c.by,
                    2 => -c.by,
                    _ => 0.0,
                };
            }
            if c.before != o.before {
                s.space_before = Some(c.before);
            }
            if c.after != o.after {
                s.space_after = Some(c.after);
            }
            if (c.line, c.at) != (o.line, o.at) {
                (s.line_rule, s.line_spacing) = match c.line {
                    0 => (LineRule::Multiple, 0.0),
                    1 => (LineRule::Multiple, 1.5),
                    2 => (LineRule::Multiple, 2.0),
                    3 => (LineRule::AtLeast, c.at),
                    4 => (LineRule::Exact, c.at),
                    _ => (LineRule::Multiple, c.at),
                };
            }
            if c.keep_next != o.keep_next {
                s.keep_with_next = c.keep_next;
            }
            if c.page_break != o.page_break {
                s.page_break_before = c.page_break;
            }
            if c.keep_lines != o.keep_lines {
                s.keep_lines = c.keep_lines;
            }
            if c.widows != o.widows {
                s.no_widow_control = !c.widows;
            }
            if c.no_line_numbers != o.no_line_numbers {
                s.no_line_numbers = c.no_line_numbers;
            }
        });
    }

    // --- ruler ------------------------------------------------------------------------------

    pub(super) fn ruler(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let v = ui.visuals().clone();
        let p = ui.painter().with_clip_rect(rect);
        let pt = PT * self.zoom;
        let (page, page_l, col) = self.ruler_frame();
        let page_l = self.canvas_origin.x + page_l;
        let (unit, subdiv, _) = ruler_unit();
        let strip = Rect::from_min_max(pos2(page_l, rect.min.y + 3.0), pos2(page_l + page.width * pt, rect.max.y - 3.0));
        let margin = if v.dark_mode { Color32::from_gray(70) } else { Color32::from_gray(205) };
        let paper = if v.dark_mode { Color32::from_gray(40) } else { Color32::WHITE };
        let ink = v.weak_text_color();
        let drag = self.ruler_drag;
        let d = |part: RulerPart| drag.filter(|d| d.part == part).map_or(0.0, |d| d.delta);
        // Page margins (content edges) in screen x.
        let ml = page_l + (page.margin_left + d(RulerPart::MarginLeft)) * pt;
        let mr = page_l + (page.width - page.margin_right - d(RulerPart::MarginRight)) * pt;
        p.rect_filled(strip, 0.0, margin);
        p.rect_filled(Rect::from_x_y_ranges(ml..=mr, strip.y_range()), 0.0, paper);
        // With columns, indents and tabs are the caret column's.
        let (cl, cr) = match col {
            Some((x, w)) => {
                let x = page_l + x;
                p.rect_filled(Rect::from_x_y_ranges(ml..=x, strip.y_range()), 0.0, margin);
                p.rect_filled(Rect::from_x_y_ranges(x + w..=mr, strip.y_range()), 0.0, margin);
                (x, x + w)
            }
            None => (ml, mr),
        };
        // Ticks, numbered from the left margin like other editors.
        let step = unit * pt / subdiv as f32;
        let mid = strip.center().y;
        let font = egui::FontId::proportional(9.0);
        for dir in [-1.0f32, 1.0] {
            let mut i = if dir > 0.0 { 0 } else { 1 };
            loop {
                let x = cl + dir * i as f32 * step;
                if x < strip.min.x - 0.5 || x > strip.max.x + 0.5 {
                    break;
                }
                if i % subdiv == 0 {
                    if i > 0 {
                        p.text(pos2(x, mid), egui::Align2::CENTER_CENTER, (i / subdiv).to_string(), font.clone(), ink);
                    }
                } else {
                    let h = if subdiv % 2 == 0 && i % (subdiv / 2) == 0 { 3.0 } else { 1.5 };
                    p.line_segment([pos2(x, mid - h), pos2(x, mid + h)], Stroke::new(1.0, ink));
                }
                i += 1;
            }
        }
        // Indent markers of the caret paragraph.
        let st = self.cur_para().style;
        let base = self.indent_pt(&st) - st.indent_left;
        let left = cl + (base + st.indent_left + d(RulerPart::Left)) * pt;
        let first = left + (st.first_line + d(RulerPart::FirstLine)) * pt;
        let right = cr - (st.indent_right + d(RulerPart::Right)) * pt;
        let fill = if v.dark_mode { Color32::from_gray(190) } else { Color32::from_gray(90) };
        let tri = |x: f32, down: bool| {
            let (y0, y1) = if down { (rect.min.y, rect.min.y + 6.0) } else { (rect.max.y, rect.max.y - 6.0) };
            p.add(egui::Shape::convex_polygon(vec![pos2(x - 5.0, y0), pos2(x + 5.0, y0), pos2(x, y1)], fill, Stroke::NONE));
        };
        tri(first, true);
        tri(left, false);
        tri(right, false);
        if let Some(dr) = drag {
            let x = match dr.part {
                RulerPart::FirstLine => first,
                RulerPart::Left => left,
                RulerPart::Right => right,
                RulerPart::MarginLeft => ml,
                RulerPart::MarginRight => mr,
            };
            ui.ctx().debug_painter().vline(x, rect.max.y..=rect.max.y + self.viewport_h, Stroke::new(1.0, Color32::from_gray(128)));
        }

        // Interaction.
        let resp = ui.interact(rect, Id::new(("ruler", self.uid)), Sense::click_and_drag());
        // A drag starts only once the pointer has moved a few pixels: what
        // it grabs is what was under the press.
        let here = ui.input(|i| i.pointer.press_origin()).filter(|_| resp.is_pointer_button_down_on()).or(resp.hover_pos());
        let hit = |h: Pos2| {
            let (x, y) = (h.x, h.y);
            let at = |mx: f32, top: bool| (x - mx).abs() <= 6.0 && (y < mid) == top;
            // With no indent the markers stand on the margins' edges: the
            // band between them grabs the margin, the markers the indents.
            let between = y > rect.min.y + 7.0 && y < rect.max.y - 7.0;
            if between && (x - ml).abs() <= 4.0 && col.is_none() {
                Some(RulerPart::MarginLeft)
            } else if between && (x - mr).abs() <= 4.0 && col.is_none() {
                Some(RulerPart::MarginRight)
            } else if at(first, true) {
                Some(RulerPart::FirstLine)
            } else if at(left, false) {
                Some(RulerPart::Left)
            } else if at(right, false) {
                Some(RulerPart::Right)
            } else if (x - ml).abs() <= 4.0 && col.is_none() {
                Some(RulerPart::MarginLeft)
            } else if (x - mr).abs() <= 4.0 && col.is_none() {
                Some(RulerPart::MarginRight)
            } else {
                None
            }
        };
        let free = drag.is_none() && here.is_some_and(|h| hit(h).is_none());
        self.ruler_tabs(ui, &resp, &p, rect, cl, cr, pt, free);
        if let Some(h) = resp.hover_pos()
            && drag.is_none()
            && let Some(part) = hit(h)
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            let key = match part {
                RulerPart::FirstLine => "ruler.first_line",
                RulerPart::Left => "ruler.left_indent",
                RulerPart::Right => "ruler.right_indent",
                _ => "ruler.margin",
            };
            resp.clone().on_hover_text_at_pointer(tr(key));
        }
        if resp.drag_started()
            && let Some(h) = here
            && let Some(part) = hit(h)
        {
            self.ruler_drag = Some(RulerDrag { part, start: h.x, delta: 0.0 });
        }
        if let Some(dr) = &mut self.ruler_drag
            && let Some(h) = resp.interact_pointer_pos()
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            let raw = (h.x - dr.start) / pt;
            // Snap to the ruler subdivisions unless Alt is held.
            let snap = unit / subdiv as f32;
            let raw = if ui.input(|i| i.modifiers.alt) { raw } else { (raw / snap).round() * snap };
            dr.delta = match dr.part {
                RulerPart::Right | RulerPart::MarginRight => -raw,
                _ => raw,
            };
        }
        if resp.drag_stopped()
            && let Some(dr) = self.ruler_drag.take()
        {
            self.commit_ruler(dr);
        }
    }

    fn commit_ruler(&mut self, dr: RulerDrag) {
        if dr.delta == 0.0 {
            return;
        }
        let dv = dr.delta;
        match dr.part {
            RulerPart::FirstLine => self.para_style(move |s| s.first_line += dv),
            // The left indent carries the first line with it.
            RulerPart::Left => self.para_style(move |s| s.indent_left += dv),
            RulerPart::Right => self.para_style(move |s| s.indent_right += dv),
            RulerPart::MarginLeft | RulerPart::MarginRight => self.ruler_margin(dr.part == RulerPart::MarginLeft, dv),
        }
    }
}

fn para_page(ui: &mut egui::Ui, v: &mut ParaVals, page: usize) {
    if page == 1 {
        widgets::section(ui, tr("para.pagination"));
        ui.checkbox(&mut v.widows, tr("para.widows"));
        ui.checkbox(&mut v.keep_next, tr("para.keep_next"));
        ui.checkbox(&mut v.keep_lines, tr("para.keep_lines"));
        ui.checkbox(&mut v.page_break, tr("para.page_break"));
        widgets::section(ui, tr("para.exceptions"));
        ui.checkbox(&mut v.no_line_numbers, tr("linenum.suppress"));
        return;
    }
    let (unit, _, suffix) = ruler_unit();
    let suffix = format!(" {suffix}");
    let len = |ui: &mut egui::Ui, pts: &mut f32, min: f32| {
        let mut u = *pts / unit;
        if ui.add(egui::DragValue::new(&mut u).range(min / unit..=20.0).speed(0.01).suffix(suffix.as_str()).max_decimals(2)).changed() {
            *pts = (u * unit * 100.0).round() / 100.0;
        }
    };
    widgets::section(ui, tr("para.general"));
    widgets::form(ui, "para_general", |ui| {
        widgets::form_row(ui, tr("para.alignment"), |ui| {
            widgets::combo_name(egui::ComboBox::from_id_salt("para_align").width(150.0).selected_text(align_label(v.align)).show_ui(ui, |ui| {
                for a in [Align::Left, Align::Center, Align::Right, Align::Justify] {
                    ui.selectable_value(&mut v.align, a, align_label(a));
                }
            }), tr("para.alignment"));
        });
    });
    widgets::section(ui, tr("para.indentation"));
    widgets::form(ui, "para_indent", |ui| {
        widgets::form_row(ui, tr("para.left"), |ui| len(ui, &mut v.left, -72.0));
        widgets::form_row(ui, tr("para.right"), |ui| len(ui, &mut v.right, -72.0));
        widgets::form_row(ui, tr("para.special"), |ui| {
            let names = [tr("para.special_none"), tr("para.first_line"), tr("para.hanging")];
            widgets::combo_name(egui::ComboBox::from_id_salt("para_special").width(110.0).selected_text(names[v.special as usize]).show_ui(ui, |ui| {
                for (i, n) in names.iter().enumerate() {
                    ui.selectable_value(&mut v.special, i as u8, *n);
                }
            }), tr("para.special"));
            if v.special != 0 {
                ui.label(tr("para.by"));
                len(ui, &mut v.by, 0.0);
                if v.by == 0.0 {
                    v.by = unit / 2.0;
                }
            }
        });
    });
    widgets::section(ui, tr("para.spacing"));
    widgets::form(ui, "para_spacing", |ui| {
        let pt = |ui: &mut egui::Ui, x: &mut f32| {
            ui.add(egui::DragValue::new(x).range(0.0..=400.0).speed(0.5).suffix(" pt").max_decimals(2));
        };
        widgets::form_row(ui, tr("para.before"), |ui| pt(ui, &mut v.before));
        widgets::form_row(ui, tr("para.after"), |ui| pt(ui, &mut v.after));
        widgets::form_row(ui, tr("para.line_spacing"), |ui| {
            let names = [tr("para.single"), "1.5", tr("para.double"), tr("para.at_least"), tr("para.exactly"), tr("para.multiple")];
            let before = v.line;
            widgets::combo_name(egui::ComboBox::from_id_salt("para_line").width(110.0).selected_text(names[v.line as usize]).show_ui(ui, |ui| {
                for (i, n) in names.iter().enumerate() {
                    ui.selectable_value(&mut v.line, i as u8, *n);
                }
            }), tr("para.line_spacing"));
            if v.line != before {
                v.at = match v.line {
                    3 | 4 => 12.0,
                    5 => 1.15,
                    _ => v.at,
                };
            }
            match v.line {
                3 | 4 => {
                    ui.label(tr("para.at"));
                    ui.add(egui::DragValue::new(&mut v.at).range(1.0..=400.0).speed(0.5).suffix(" pt").max_decimals(1));
                }
                5 => {
                    ui.label(tr("para.at"));
                    ui.add(egui::DragValue::new(&mut v.at).range(0.5..=10.0).speed(0.01).max_decimals(2));
                }
                _ => {}
            }
        });
    });
}

/// A page with the paragraph among grey ones, like Word's preview: its
/// indents, alignment and spacing to scale.
fn para_preview(ui: &mut egui::Ui, v: &ParaVals) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 170.0), egui::Sense::hover());
    let p = ui.painter_at(rect);
    let vis = ui.visuals();
    p.rect_filled(rect, 2.0, Color32::WHITE);
    p.rect_stroke(rect, 2.0, vis.widgets.noninteractive.bg_stroke, egui::StrokeKind::Inside);
    // A 6.5-inch text column drawn 150 px wide.
    let scale = (rect.width() - 20.0) / 468.0;
    let (x0, x1) = (rect.min.x + 10.0, rect.max.x - 10.0);
    let grey = Color32::from_gray(205);
    let dark = Color32::from_gray(90);
    let bar = 3.0;
    let mut y = rect.min.y + 10.0;
    let plain = |y: &mut f32, n: usize| {
        for i in 0..n {
            let end = if i + 1 == n { x0 + (x1 - x0) * 0.6 } else { x1 };
            p.line_segment([pos2(x0, *y), pos2(end, *y)], egui::Stroke::new(bar, grey));
            *y += 6.0;
        }
    };
    plain(&mut y, 3);
    y += v.before.min(60.0) * scale;
    let gap = match v.line {
        1 => 9.0,
        2 => 12.0,
        3 => (v.at * scale * 2.0).max(6.0),
        4 => (v.at * scale * 2.0).clamp(3.0, 20.0),
        5 => (6.0 * v.at).clamp(3.0, 20.0),
        _ => 6.0,
    };
    let left = x0 + v.left.clamp(-36.0, 200.0) * scale;
    let right = x1 - v.right.clamp(-36.0, 200.0) * scale;
    let first = match v.special {
        1 => v.by,
        2 => -v.by,
        _ => 0.0,
    } * scale;
    let widths = [1.0, 0.93, 0.97, 0.9, 0.55];
    for (i, w) in widths.iter().enumerate() {
        let a = if i == 0 { left + first } else { left };
        let full = (right - a).max(4.0);
        let len = if matches!(v.align, Align::Justify) && i + 1 < widths.len() { full } else { full * w };
        let start = match v.align {
            Align::Center => a + (full - len) / 2.0,
            Align::Right => right - len,
            _ => a,
        };
        if y > rect.max.y - 4.0 {
            break;
        }
        p.line_segment([pos2(start, y), pos2(start + len, y)], egui::Stroke::new(bar, dark));
        y += gap;
    }
    y += v.after.min(60.0) * scale;
    plain(&mut y, 6);
}

fn link_page(ui: &mut egui::Ui, d: &mut LinkDlg, bookmarks: &[Arc<str>]) {
    egui::Grid::new("link_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label(tr("link.text"));
        match &mut d.text {
            Some(t) => {
                ui.add(egui::TextEdit::singleline(t).desired_width(280.0));
            }
            None => {
                ui.add_enabled(false, egui::TextEdit::singleline(&mut tr("link.selection").to_string()).desired_width(280.0));
            }
        }
        ui.end_row();
        ui.label(tr("link.address"));
        let r = ui.add(egui::TextEdit::singleline(&mut d.address).hint_text("https://…").desired_width(280.0));
        if std::mem::take(&mut d.focus) {
            r.request_focus();
        }
        ui.end_row();
        ui.label(tr("link.place"));
        let cur = d.address.strip_prefix('#').map(str::to_string);
        let shown = cur.clone().filter(|c| bookmarks.iter().any(|b| b.eq_ignore_ascii_case(c))).unwrap_or_else(|| tr("link.place_none").to_string());
        ui.add_enabled_ui(!bookmarks.is_empty(), |ui| {
            egui::ComboBox::from_id_salt("link_place").width(280.0).selected_text(shown).show_ui(ui, |ui| {
                for b in bookmarks {
                    let on = cur.as_deref().is_some_and(|c| b.eq_ignore_ascii_case(c));
                    if ui.selectable_label(on, b.as_ref()).clicked() {
                        d.address = format!("#{b}");
                    }
                }
            });
        });
        ui.end_row();
        ui.label(tr("link.tip"));
        ui.add(egui::TextEdit::singleline(&mut d.tip).desired_width(280.0));
        ui.end_row();
    });
    if d.editing {
        ui.add_space(6.0);
        if ui.button(tr("link.remove")).clicked() {
            d.remove = true;
        }
    }
}

fn align_label(a: Align) -> &'static str {
    tr(match a {
        Align::Left => "format.align_left",
        Align::Center => "format.align_center",
        Align::Right => "format.align_right",
        Align::Justify => "format.align_justify",
    })
}
