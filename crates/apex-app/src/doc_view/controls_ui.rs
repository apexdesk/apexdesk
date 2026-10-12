//! Content controls: the Insert tab menu, clicks on check boxes, the list
//! and date pickers, the box around the control at the caret, the
//! properties dialog and the editing rules (placeholders, locks).

use apex_doc::{CHECKED, ContentControl, ControlAt, ControlKind, ListItem, UNCHECKED};

use super::*;
use crate::widgets::DialogResult;

pub(super) struct ControlDlg {
    para: usize,
    id: u32,
    c: ContentControl,
    pick: Option<usize>,
    new_text: String,
    new_value: String,
}

/// The list or date picker open under a control.
pub(super) struct ControlPopup {
    para: usize,
    id: u32,
    /// The month shown by the date picker.
    year: i64,
    month: u32,
    /// Opened this frame (the click that opened it isn't outside it).
    fresh: bool,
}

/// The placeholder of a control (en spaces for a legacy text field).
fn placeholder_of(c: &ContentControl) -> &'static str {
    if c.is_form() { apex_doc::FORM_BLANK } else { placeholder(&c.kind) }
}

fn placeholder(kind: &ControlKind) -> &'static str {
    match kind {
        ControlKind::List { .. } => tr("ctl.ph_list"),
        ControlKind::Date { .. } => tr("ctl.ph_date"),
        _ => tr("ctl.ph_text"),
    }
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in(y: i64, m: u32) -> u32 {
    match m {
        2 if is_leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Day of the week, Monday = 0.
fn weekday(y: i64, m: u32, d: u32) -> u32 {
    // Sakamoto's method (Sunday = 0).
    const T: [i64; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if m < 3 { y - 1 } else { y };
    let w = (y + y.div_euclid(4) - y.div_euclid(100) + y.div_euclid(400) + T[(m - 1) as usize] + d as i64).rem_euclid(7);
    ((w + 6) % 7) as u32
}

impl DocView {
    fn find_control(&self, para: usize, id: u32) -> Option<ControlAt> {
        let p = self.doc.paragraphs.get(para)?;
        apex_doc::para_controls(p, para).into_iter().find(|c| c.control.id == id)
    }

    /// The control the caret is in (its ends included).
    pub(super) fn caret_control(&self) -> Option<ControlAt> {
        let p = self.sel.cursor;
        self.doc.control_at(p).filter(|c| c.para == p.para && c.range.start <= p.offset && p.offset <= c.range.end)
    }

    /// The Controls group of the Insert tab.
    pub(super) fn controls_group(&mut self, b: &mut crate::toolstrip::Body) {
        use crate::toolstrip;
        let a = b.accent();
        b.group("toolstrip.g.controls", Icon::Control, |ui| {
            toolstrip::large_menu(ui, Icon::Control, tr("ctl.menu"), "", a, |ui| {
                let kinds: [(&str, ControlKind); 6] = [
                    ("ctl.rich", ControlKind::RichText),
                    ("ctl.plain", ControlKind::PlainText),
                    ("ctl.checkbox", ControlKind::CheckBox { checked: false }),
                    ("ctl.combo", ControlKind::List { items: Vec::new(), editable: true }),
                    ("ctl.dropdown", ControlKind::List { items: Vec::new(), editable: false }),
                    ("ctl.date", ControlKind::Date { date: None, format: "dd/MM/yyyy".into() }),
                ];
                for (key, kind) in kinds {
                    if ui.button(tr(key)).clicked() {
                        self.insert_content_control(kind);
                        ui.close();
                    }
                }
                ui.separator();
                let at = self.caret_control();
                if ui.add_enabled(at.is_some(), egui::Button::new(format!("{}…", tr("ctl.properties")))).clicked() {
                    if let Some(at) = &at {
                        self.open_control_props(at);
                    }
                    ui.close();
                }
                if ui.add_enabled(at.as_ref().is_some_and(|c| !c.control.lock_delete), egui::Button::new(tr("ctl.remove"))).clicked() {
                    if let Some(at) = &at {
                        let p = self.doc.remove_control(at, self.sel);
                        self.set_sel(Selection::caret(p));
                    }
                    ui.close();
                }
            });
            self.forms_menu(ui, a);
        });
    }

    fn insert_content_control(&mut self, kind: ControlKind) {
        // Inside another (at its ends is beside it).
        if self.doc.control_of(self.sel).is_some_and(|c| c.range.start < self.sel.start().offset && self.sel.end().offset < c.range.end) {
            self.notices.push(Notice::info(tr("ctl.nested")));
            return;
        }
        let id = self.doc.next_control_id();
        let (text, ph) = match &kind {
            ControlKind::CheckBox { .. } => (UNCHECKED.to_string(), false),
            k => (placeholder(k).to_string(), true),
        };
        let list = matches!(kind, ControlKind::List { .. });
        let c = ContentControl { placeholder: ph, ..ContentControl::new(id, kind) };
        let s = self.doc.insert_control(self.sel, c, &text);
        self.set_sel(Selection::caret(s.start()));
        self.focus = true;
        // A new list needs its entries.
        if list && let Some(at) = self.doc.control_at(s.start()) {
            self.open_control_props(&at);
        }
    }

    /// After an edit: a bound control's new value goes to its custom XML
    /// node and to the other controls bound to it.
    pub(super) fn sync_bindings(&mut self) {
        if self.bind_rev == self.doc.revision {
            return;
        }
        if apex_io::docx_bind::has_bindings(&self.doc) {
            apex_io::docx_bind::sync(&mut self.doc, &|k| placeholder(k).to_string());
        }
        self.bind_rev = self.doc.revision;
    }

    fn open_control_props(&mut self, at: &ControlAt) {
        if at.control.is_form() {
            self.open_form_props(at);
            return;
        }
        self.control_dlg = Some(ControlDlg { para: at.para, id: at.control.id, c: (*at.control).clone(), pick: None, new_text: String::new(), new_value: String::new() });
    }

    /// Sets a control's state and text.
    fn set_control_value(&mut self, at: &ControlAt, c: ContentControl, text: &str) {
        let p = self.doc.set_control(at, c, text, self.sel);
        self.set_sel(Selection::caret(p));
    }

    /// A click on a control: check boxes toggle, lists and dates open
    /// their picker. True when the click was taken.
    pub(super) fn control_click(&mut self, local: Pos2) -> bool {
        let pos = self.pos_at(local);
        // The control under the point (next to the caret position, on
        // either side).
        let on_row = |r: Rect| local.y >= r.min.y && local.y <= r.max.y;
        let under = |at: &ControlAt| {
            let (r0, r1) = (self.caret_rect(Pos::new(at.para, at.range.start)), self.caret_rect(Pos::new(at.para, at.range.end)));
            if (r0.min.y - r1.min.y).abs() < 1.0 { on_row(r0) && local.x >= r0.min.x && local.x <= r1.max.x } else { (on_row(r0) && local.x >= r0.min.x) || (on_row(r1) && local.x <= r1.max.x) || (local.y > r0.max.y && local.y < r1.min.y) }
        };
        let Some(at) = apex_doc::para_controls(&self.doc.paragraphs[pos.para], pos.para).into_iter().find(|c| c.range.start <= pos.offset && pos.offset <= c.range.end && under(c)) else { return false };
        match &at.control.kind {
            ControlKind::CheckBox { checked } if !at.control.fixed() => {
                let checked = !*checked;
                let c = ContentControl { kind: ControlKind::CheckBox { checked }, ..(*at.control).clone() };
                self.set_control_value(&at, c, &(if checked { CHECKED } else { UNCHECKED }).to_string());
                true
            }
            ControlKind::List { .. } | ControlKind::Date { .. } if !at.control.fixed() => {
                let (y, m, _, _, _) = crate::common::local_now();
                let (mut year, mut month) = (y, m);
                if let ControlKind::Date { date: Some(d), .. } = &at.control.kind {
                    let mut it = d.split('-');
                    year = it.next().and_then(|v| v.parse().ok()).unwrap_or(y);
                    month = it.next().and_then(|v| v.parse().ok()).filter(|m| (1..=12).contains(m)).unwrap_or(m);
                }
                self.control_popup = Some(ControlPopup { para: at.para, id: at.control.id, year, month, fresh: true });
                // Placeholders are picked whole; typed lists get the caret.
                if at.control.placeholder || !at.control.kind.typed() {
                    self.set_sel(at.sel());
                } else {
                    self.caret(pos, false);
                }
                true
            }
            _ => false,
        }
    }

    /// Before typing `text`: fills a placeholder, keeps text typed over a
    /// whole control in it, or refuses edits to locked controls. True when
    /// the typing was handled here.
    pub(super) fn control_typing(&mut self, text: &str) -> bool {
        let (a, b) = (self.sel.start(), self.sel.end());
        let Some(at) = self.doc.control_at(a).or_else(|| self.doc.control_at(b)) else { return false };
        let (s, e) = (Pos::new(at.para, at.range.start), Pos::new(at.para, at.range.end));
        let caret_in = if self.sel.is_empty() { (s < a && a < e) || (at.control.editable() && a == e) || (at.control.placeholder && a == s) } else { a < e && b > s };
        if !caret_in {
            return false;
        }
        let whole = a <= s && b >= e;
        if at.control.placeholder && self.sel.is_empty() || (at.control.placeholder && whole && a == s && b == e) {
            if at.control.editable() && !text.contains('\n') {
                let c = ContentControl { placeholder: false, ..(*at.control).clone() };
                self.set_control_value(&at, c, text);
                self.scroll_to_caret = true;
            }
            return true;
        }
        if !whole && !at.control.editable() {
            return true;
        }
        if whole && a == s && b == e && at.control.editable() {
            // Typed over all of it: the text stays in the control.
            let mut attrs = self.doc.paragraphs[at.para].attrs_at(at.range.start + 1);
            attrs.control = Some(at.control.clone());
            self.pending = Some(attrs);
        }
        false
    }

    /// Before deleting (`forward`: the Delete key): whether a lock refuses
    /// it, and the control the deletion would empty (it gets its
    /// placeholder back).
    pub(super) fn control_deleting(&self, forward: bool) -> (bool, Option<ContentControl>) {
        let (mut a, mut b) = (self.sel.start(), self.sel.end());
        if self.sel.is_empty() {
            let text = &self.doc.paragraphs[a.para].text;
            if forward {
                b.offset += text[a.offset..].chars().next().map_or(0, char::len_utf8);
            } else {
                a.offset -= text[..a.offset].chars().next_back().map_or(0, char::len_utf8);
            }
        }
        if a.para != b.para || a == b {
            return (false, None);
        }
        for at in apex_doc::para_controls(&self.doc.paragraphs[a.para], a.para) {
            let (s, e) = (at.range.start, at.range.end);
            if a.offset >= e || b.offset <= s {
                continue;
            }
            let whole = a.offset <= s && b.offset >= e;
            if whole {
                if at.control.lock_delete {
                    return (true, None);
                }
                // Emptied, it gets its placeholder back (legacy check boxes
                // and drop-downs just go).
                let refill = !matches!(at.control.kind, ControlKind::CheckBox { .. }) && (!at.control.is_form() || at.control.kind == ControlKind::PlainText);
                if a.offset == s && b.offset == e && !at.control.placeholder && refill {
                    return (false, Some((*at.control).clone()));
                }
            } else if !at.control.editable() || at.control.placeholder {
                return (true, None);
            }
        }
        (false, None)
    }

    /// After a deletion that emptied a control.
    pub(super) fn refill_control(&mut self, c: ContentControl) {
        let p = self.sel.cursor;
        let s = self.doc.refill_control(p, &c, placeholder_of(&c));
        self.set_sel(Selection::caret(s.start()));
    }

    /// Check box controls, drawn in the em spaces they lay out as.
    pub(super) fn paint_check_boxes(&self, p: &egui::Painter, para: &Paragraph, pl: &ParaLayout, seg: &Segment, galley_pos: Pos2) {
        let mut boxes: Vec<(usize, bool)> = Vec::new();
        let mut ci = 0usize;
        let mut last = 0usize;
        for (s, e, a) in para.spans() {
            if !a.control.as_ref().is_some_and(|c| matches!(c.kind, ControlKind::CheckBox { .. })) {
                continue;
            }
            for (o, ch) in para.text[s..e].char_indices() {
                if ch == CHECKED || ch == UNCHECKED {
                    ci += para.text[last..s + o].chars().count();
                    last = s + o;
                    boxes.push((ci, ch == CHECKED));
                }
            }
        }
        if boxes.is_empty() {
            return;
        }
        let mut start = 0usize;
        for row in &pl.galley.rows {
            let n = row.row.glyphs.len() + usize::from(row.ends_with_newline);
            let visible = row.pos.y + row.row.size.y > seg.gy0 && row.pos.y < seg.gy1;
            for &(i, checked) in boxes.iter().filter(|b| b.0 >= start && b.0 < start + n) {
                let Some(g) = row.row.glyphs.get(i - start).filter(|_| visible) else { continue };
                let size = g.font_height * 0.62;
                let o = galley_pos + row.pos.to_vec2();
                let c = pos2(o.x + g.pos.x + g.advance_width / 2.0, o.y + g.pos.y - g.font_ascent * 0.36);
                let r = Rect::from_center_size(c, vec2(size, size));
                let stroke = Stroke::new((size / 12.0).max(1.0), Paper::TEXT);
                p.rect_stroke(r, 0.0, stroke, egui::StrokeKind::Inside);
                if checked {
                    let k = r.shrink(size * 0.2);
                    p.line_segment([k.left_top(), k.right_bottom()], stroke);
                    p.line_segment([k.right_top(), k.left_bottom()], stroke);
                }
            }
            start += n;
        }
    }

    /// The box around the control at the caret, with its title.
    pub(super) fn paint_control_box(&self, p: &egui::Painter, off: Vec2) {
        let Some(at) = self.caret_control() else { return };
        let r0 = self.caret_rect(Pos::new(at.para, at.range.start)).translate(off);
        let r1 = self.caret_rect(Pos::new(at.para, at.range.end)).translate(off);
        let color = Color32::from_rgb(0x6B, 0x8E, 0xC2);
        let stroke = Stroke::new(1.0, color);
        let pad = 2.0 * self.zoom.max(1.0);
        if (r0.min.y - r1.min.y).abs() < 1.0 {
            p.rect_stroke(Rect::from_min_max(pos2(r0.min.x - pad, r0.min.y), pos2(r1.max.x + pad, r1.max.y)), 2.0, stroke, egui::StrokeKind::Outside);
        } else {
            p.vline(r0.min.x - pad, r0.y_range(), stroke);
            p.vline(r1.max.x + pad, r1.y_range(), stroke);
        }
        if !at.control.title.is_empty() {
            let font = egui::FontId::proportional(11.0);
            let g = p.layout_no_wrap(at.control.title.clone(), font, Color32::WHITE);
            let tab = Rect::from_min_size(pos2(r0.min.x - pad, r0.min.y - g.size().y - 3.0), g.size() + vec2(8.0, 3.0));
            p.rect_filled(tab, egui::CornerRadius { nw: 2, ne: 2, sw: 0, se: 0 }, color);
            p.galley(tab.min + vec2(4.0, 1.5), g, Color32::WHITE);
        }
        if matches!(at.control.kind, ControlKind::List { .. } | ControlKind::Date { .. }) {
            let h = r1.height();
            let b = Rect::from_min_size(pos2(r1.max.x + pad, r1.min.y), vec2(h * 0.8, h));
            p.rect_filled(b, 0.0, Color32::from_rgb(0xE4, 0xEC, 0xF7));
            let c = b.center();
            let s = h * 0.15;
            p.add(egui::Shape::convex_polygon(vec![pos2(c.x - s, c.y - s * 0.5), pos2(c.x + s, c.y - s * 0.5), pos2(c.x, c.y + s * 0.7)], color, Stroke::NONE));
        }
    }

    /// The picker under a list or date control, and the properties dialog.
    pub(super) fn controls_ui(&mut self, ctx: &egui::Context) {
        self.control_props_dialog(ctx);
        let Some(pop) = self.control_popup.take() else { return };
        let Some(at) = self.find_control(pop.para, pop.id) else { return };
        let r = self.caret_rect(Pos::new(at.para, at.range.start)).translate(self.canvas_origin.to_vec2());
        let mut keep = true;
        let mut pick: Option<(ContentControl, String)> = None;
        let mut pop = pop;
        let area = egui::Area::new(Id::new(("ctl_popup", self.uid))).fixed_pos(pos2(r.min.x, r.max.y + 2.0)).order(egui::Order::Foreground).show(ctx, |ui| {
            egui::Frame::menu(ui.style()).show(ui, |ui| match &at.control.kind {
                ControlKind::List { items, .. } => {
                    ui.set_min_width(160.0);
                    if items.is_empty() {
                        ui.label(egui::RichText::new(tr("ctl.no_items")).weak());
                    }
                    for it in items {
                        if ui.selectable_label(false, &it.text).clicked() {
                            pick = Some((ContentControl { placeholder: false, ..(*at.control).clone() }, it.text.clone()));
                        }
                    }
                }
                ControlKind::Date { date, format } => {
                    let months: Vec<&str> = tr("field.months").split('|').collect();
                    let days: Vec<&str> = tr("field.days").split('|').collect();
                    ui.horizontal(|ui| {
                        if ui.small_button("◀").clicked() {
                            (pop.year, pop.month) = if pop.month == 1 { (pop.year - 1, 12) } else { (pop.year, pop.month - 1) };
                        }
                        let name = months.get(pop.month as usize - 1).copied().unwrap_or("");
                        ui.add_sized([120.0, 18.0], egui::Label::new(format!("{name} {}", pop.year)));
                        if ui.small_button("▶").clicked() {
                            (pop.year, pop.month) = if pop.month == 12 { (pop.year + 1, 1) } else { (pop.year, pop.month + 1) };
                        }
                    });
                    let chosen = date.as_deref().unwrap_or("");
                    egui::Grid::new("ctl_cal").spacing([2.0, 2.0]).show(ui, |ui| {
                        // Monday first: field.days starts on Sunday.
                        for k in 0..7 {
                            let d = days.get((k + 1) % 7).copied().unwrap_or("");
                            ui.label(egui::RichText::new(d.chars().take(2).collect::<String>()).small().weak());
                        }
                        ui.end_row();
                        let first = weekday(pop.year, pop.month, 1);
                        for _ in 0..first {
                            ui.label("");
                        }
                        for d in 1..=days_in(pop.year, pop.month) {
                            let iso = format!("{:04}-{:02}-{:02}", pop.year, pop.month, d);
                            if ui.add_sized([24.0, 20.0], egui::Button::selectable(iso == chosen, d.to_string())).clicked() {
                                let kind = ControlKind::Date { date: Some(iso.clone()), format: format.clone() };
                                let text = apex_doc::control_date_text(&iso, format, &months, &days);
                                pick = Some((ContentControl { placeholder: false, kind, ..(*at.control).clone() }, text));
                            }
                            if (first + d) % 7 == 0 {
                                ui.end_row();
                            }
                        }
                    });
                    if ui.button(tr("ctl.today")).clicked() {
                        let (y, m, d, _, _) = crate::common::local_now();
                        let iso = format!("{y:04}-{m:02}-{d:02}");
                        let kind = ControlKind::Date { date: Some(iso.clone()), format: format.clone() };
                        let text = apex_doc::control_date_text(&iso, format, &months, &days);
                        pick = Some((ContentControl { placeholder: false, kind, ..(*at.control).clone() }, text));
                    }
                }
                _ => keep = false,
            });
        });
        if let Some((c, text)) = pick {
            self.set_control_value(&at, c, &text);
            self.focus = true;
            return;
        }
        let outside = ctx.input(|i| i.pointer.primary_pressed() && i.pointer.interact_pos().is_some_and(|p| !area.response.rect.contains(p)));
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) || (outside && !pop.fresh) {
            keep = false;
        }
        pop.fresh = false;
        // Gone when the caret leaves the control.
        if self.caret_control().is_none_or(|c| c.control.id != pop.id) {
            keep = false;
        }
        if keep {
            self.control_popup = Some(pop);
        }
    }

    fn control_props_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.control_dlg.take() else { return };
        let res = widgets::dialog(ctx, "doc_control_props", tr("ctl.props_title"), 420.0, |ui| {
            egui::Grid::new("ctl_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label(tr("ctl.title"));
                ui.add(egui::TextEdit::singleline(&mut d.c.title).desired_width(f32::INFINITY));
                ui.end_row();
                ui.label(tr("ctl.tag"));
                ui.add(egui::TextEdit::singleline(&mut d.c.tag).desired_width(f32::INFINITY));
                ui.end_row();
            });
            ui.add_space(4.0);
            ui.checkbox(&mut d.c.lock_delete, tr("ctl.lock_delete"));
            ui.checkbox(&mut d.c.lock_content, tr("ctl.lock_content"));
            match &mut d.c.kind {
                ControlKind::List { items, .. } => {
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(tr("ctl.items")).strong());
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.set_min_size(vec2(ui.available_width(), 120.0));
                        egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                            for (i, it) in items.iter().enumerate() {
                                let line = if it.value.is_empty() || it.value == it.text { it.text.clone() } else { format!("{}  ({})", it.text, it.value) };
                                if ui.selectable_label(d.pick == Some(i), line).clicked() {
                                    d.pick = Some(i);
                                }
                            }
                        });
                    });
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut d.new_text).hint_text(tr("ctl.item_text")).desired_width(150.0));
                        ui.add(egui::TextEdit::singleline(&mut d.new_value).hint_text(tr("ctl.item_value")).desired_width(90.0));
                        if ui.add_enabled(!d.new_text.trim().is_empty(), egui::Button::new(tr("ctl.add"))).clicked() {
                            let text = d.new_text.trim().to_string();
                            let value = if d.new_value.trim().is_empty() { text.clone() } else { d.new_value.trim().to_string() };
                            items.push(ListItem { text, value });
                            d.new_text.clear();
                            d.new_value.clear();
                        }
                    });
                    ui.horizontal(|ui| {
                        let picked = d.pick.filter(|&i| i < items.len());
                        if ui.add_enabled(picked.is_some(), egui::Button::new(tr("ctl.item_remove"))).clicked()
                            && let Some(i) = picked
                        {
                            items.remove(i);
                            d.pick = None;
                        }
                        if ui.add_enabled(picked.is_some_and(|i| i > 0), egui::Button::new(tr("ctl.up"))).clicked()
                            && let Some(i) = picked
                        {
                            items.swap(i, i - 1);
                            d.pick = Some(i - 1);
                        }
                        if ui.add_enabled(picked.is_some_and(|i| i + 1 < items.len()), egui::Button::new(tr("ctl.down"))).clicked()
                            && let Some(i) = picked
                        {
                            items.swap(i, i + 1);
                            d.pick = Some(i + 1);
                        }
                    });
                }
                ControlKind::Date { format, .. } => {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(tr("ctl.date_format"));
                        egui::ComboBox::from_id_salt("ctl_date_fmt").selected_text(format.as_str()).width(200.0).show_ui(ui, |ui| {
                            for f in ["dd/MM/yyyy", "d/M/yyyy", "dd-MM-yy", "yyyy-MM-dd", "d 'de' MMMM 'de' yyyy", "dddd, d 'de' MMMM 'de' yyyy", "MMMM yyyy"] {
                                ui.selectable_value(format, f.to_string(), f);
                            }
                        });
                    });
                }
                _ => {}
            }
        });
        match res {
            DialogResult::None => self.control_dlg = Some(d),
            DialogResult::Cancel => self.focus = true,
            DialogResult::Ok => {
                if let Some(at) = self.find_control(d.para, d.id) {
                    // A date shows in its new format.
                    let text = match &d.c.kind {
                        ControlKind::Date { date: Some(iso), format } if !d.c.placeholder => {
                            let months: Vec<&str> = tr("field.months").split('|').collect();
                            let days: Vec<&str> = tr("field.days").split('|').collect();
                            apex_doc::control_date_text(iso, format, &months, &days)
                        }
                        _ => self.doc.paragraphs[at.para].text[at.range.clone()].to_string(),
                    };
                    if d.c != *at.control || text != self.doc.paragraphs[at.para].text[at.range.clone()] {
                        self.set_control_value(&at, d.c, &text);
                    }
                }
                self.focus = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar() {
        assert_eq!(weekday(2026, 9, 25), 4); // Friday
        assert_eq!(weekday(2024, 2, 29), 3); // Thursday
        assert_eq!(days_in(2024, 2), 29);
        assert_eq!(days_in(2100, 2), 28);
    }
}
