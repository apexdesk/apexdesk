//! Legacy form fields (Word's text, check box and drop-down fields): the
//! Legacy Forms menu of the Insert tab, their options dialog, what typing
//! in a text field may add, Tab between fields and the "filling in forms"
//! protection, which lets only form fields change.

use apex_doc::{ContentControl, ControlAt, ControlKind, EditLimit, FORM_BLANK, FormField, FormTextType, ListItem};

use super::*;
use crate::widgets::DialogResult;

/// The shading behind form fields (Word's light grey).
const SHADE: Color32 = Color32::from_rgb(0xDD, 0xDD, 0xDD);

pub(super) struct FormDlg {
    para: usize,
    id: u32,
    c: ContentControl,
    f: FormField,
    new_item: String,
    pick: Option<usize>,
}

#[derive(Default)]
pub(super) struct FormState {
    pub dlg: Option<FormDlg>,
    /// Form field shading is off.
    pub no_shading: bool,
    /// The revision and fingerprint the forms protection last allowed.
    allowed: Option<(u64, u64)>,
}

fn type_label(t: FormTextType) -> &'static str {
    tr(match t {
        FormTextType::Regular => "ffld.t_regular",
        FormTextType::Number => "ffld.t_number",
        FormTextType::Date => "ffld.t_date",
        FormTextType::CurrentDate => "ffld.t_cur_date",
        FormTextType::CurrentTime => "ffld.t_cur_time",
        FormTextType::Calculated => "ffld.t_calc",
    })
}

/// Formats offered for each kind of text field.
fn formats(t: FormTextType) -> &'static [&'static str] {
    match t {
        FormTextType::Regular => &["", "UPPERCASE", "lowercase", "First capital", "Title case"],
        FormTextType::Number => &["", "0", "0.00", "#,##0", "#,##0.00", "0%", "0.00%"],
        FormTextType::Date | FormTextType::CurrentDate => &["", "dd/MM/yyyy", "d/M/yyyy", "yyyy-MM-dd", "d MMMM yyyy", "dddd, d MMMM yyyy"],
        FormTextType::CurrentTime => &["", "HH:mm", "HH:mm:ss", "h:mm AM/PM"],
        FormTextType::Calculated => &["", "0", "0.00", "#,##0.00"],
    }
}

impl DocView {
    /// The Legacy Forms menu of the Insert tab's Controls group.
    pub(super) fn forms_menu(&mut self, ui: &mut egui::Ui, a: Color32) {
        use crate::toolstrip;
        toolstrip::large_menu(ui, Icon::TextBox, tr("ffld.menu"), "", a, |ui| {
            let kinds: [(Icon, &str, ControlKind); 3] = [
                (Icon::TextBox, "ffld.text", ControlKind::PlainText),
                (Icon::Checkbox, "ffld.checkbox", ControlKind::CheckBox { checked: false }),
                (Icon::Validation, "ffld.dropdown", ControlKind::List { items: Vec::new(), editable: false }),
            ];
            for (icon, key, kind) in kinds {
                if toolstrip::menu_item(ui, icon, tr(key)).clicked() {
                    self.insert_form_field(kind);
                    ui.close();
                }
            }
            ui.separator();
            let at = self.caret_control().filter(|c| c.control.is_form());
            let r = ui.add_enabled_ui(at.is_some(), |ui| toolstrip::menu_item(ui, Icon::Options, &format!("{}…", tr("ffld.properties")))).inner;
            if r.clicked() {
                if let Some(at) = &at {
                    self.open_form_props(at);
                }
                ui.close();
            }
            if toolstrip::check(ui, !self.forms.no_shading, Icon::Shading, tr("ffld.shading")) {
                self.forms.no_shading = !self.forms.no_shading;
            }
        });
    }

    fn insert_form_field(&mut self, kind: ControlKind) {
        if self.doc.control_of(self.sel).is_some_and(|c| c.range.start < self.sel.start().offset && self.sel.end().offset < c.range.end) {
            self.notices.push(Notice::info(tr("ctl.nested")));
            return;
        }
        let id = self.doc.next_control_id();
        // Word's names: Text1, Check1, Dropdown1...
        let stem = match kind {
            ControlKind::CheckBox { .. } => "Check",
            ControlKind::List { .. } => "Dropdown",
            _ => "Text",
        };
        let taken: Vec<String> = self.doc.controls().iter().filter_map(|c| c.control.legacy.as_ref().map(|f| f.name.to_lowercase())).chain(self.doc.bookmarks().iter().map(|b| b.name.to_lowercase())).collect();
        let name = (1..).map(|n| format!("{stem}{n}")).find(|n| !taken.contains(&n.to_lowercase())).unwrap_or_default();
        let list = matches!(kind, ControlKind::List { .. });
        let text = match kind {
            ControlKind::CheckBox { .. } => apex_doc::UNCHECKED.to_string(),
            _ => FORM_BLANK.to_string(),
        };
        let blank = matches!(kind, ControlKind::PlainText);
        let c = ContentControl { placeholder: blank, ..ContentControl::form(id, kind, FormField { name, ..Default::default() }) };
        // At the start of the selection (which stays).
        let at = self.sel.start();
        let s = self.doc.insert_control(Selection::caret(at), c, &text);
        self.set_sel(Selection::caret(s.end()));
        self.focus = true;
        // A new drop-down needs its items.
        if list && let Some(at) = self.doc.control_at(s.start()) {
            self.open_form_props(&at);
        }
    }

    pub(super) fn open_form_props(&mut self, at: &ControlAt) {
        let Some(f) = at.control.legacy.as_deref() else { return };
        self.forms.dlg = Some(FormDlg { para: at.para, id: at.control.id, c: (*at.control).clone(), f: f.clone(), new_item: String::new(), pick: None });
    }

    /// Text typed into a legacy text field as it may go in (cased, only
    /// numbers, within the maximum length); `None` outside one.
    pub(super) fn form_typed(&mut self, text: &str) -> Option<String> {
        let (a, b) = (self.sel.start(), self.sel.end());
        let at = self.doc.control_at(a).filter(|c| c.para == a.para && c.range.start <= a.offset && a.offset <= c.range.end)?;
        let f = at.control.legacy.as_deref().filter(|_| at.control.kind == ControlKind::PlainText)?;
        if b.para != a.para || b.offset > at.range.end {
            return None;
        }
        let field = &self.doc.paragraphs[at.para].text[at.range.clone()];
        let len = if at.control.placeholder { 0 } else { field.chars().count() - self.doc.paragraphs[a.para].text[a.offset..b.offset].chars().count() };
        let fitted = f.fit_typed(text, len);
        if fitted.chars().count() < text.chars().count() && at.control.editable() {
            let why = if f.max_len > 0 && len + fitted.chars().count() >= f.max_len as usize { trf("ffld.full", &[("n", &f.max_len)]) } else { tr("ffld.only_numbers").to_string() };
            self.notices.push(Notice::info(why));
        }
        Some(fitted)
    }

    /// Tab in a document protected for forms: to the next (or previous)
    /// field. True when there was one.
    pub(super) fn tab_to_field(&mut self, back: bool) -> bool {
        let all: Vec<ControlAt> = self.doc.controls().into_iter().filter(|c| c.control.legacy.as_ref().is_none_or(|f| f.enabled) && !c.control.lock_content).collect();
        if all.is_empty() {
            return false;
        }
        let here = self.sel.start();
        let cur = all.iter().position(|c| c.para == here.para && c.range.start <= here.offset && here.offset <= c.range.end);
        let next = match (cur, back) {
            (Some(i), false) => (i + 1) % all.len(),
            (Some(i), true) => (i + all.len() - 1) % all.len(),
            (None, false) => all.iter().position(|c| Pos::new(c.para, c.range.start) >= here).unwrap_or(0),
            (None, true) => all.iter().rposition(|c| Pos::new(c.para, c.range.end) <= here).unwrap_or(all.len() - 1),
        };
        let c = &all[next];
        // Text is selected to be typed over; boxes and lists get the caret.
        if c.control.kind.typed() {
            self.set_sel(c.sel());
        } else {
            self.set_sel(Selection::caret(Pos::new(c.para, c.range.start)));
        }
        self.scroll_to_caret = true;
        true
    }

    /// The shading behind a form field's text, if shown (on screen only).
    pub(super) fn form_shade(&self, a: &Attrs) -> Option<Color32> {
        (!self.forms.no_shading && !self.printing.get() && a.control.as_ref().is_some_and(|c| c.is_form())).then_some(SHADE)
    }

    /// "Filling in forms": takes back whatever changed outside form fields
    /// and content controls (undoing or redoing to where it was allowed);
    /// `skeleton` fingerprints what must stay (read-only protection with
    /// editable ranges uses it too). True when something was taken back.
    pub(super) fn enforce_forms(&mut self, skeleton: fn(&apex_doc::Document) -> u64) -> bool {
        let rev = self.doc.revision;
        if self.forms.allowed.is_some_and(|(r, _)| r == rev) {
            return false;
        }
        let now = skeleton(&self.doc);
        let Some((_, good)) = self.forms.allowed.filter(|(_, s)| *s != now) else {
            self.forms.allowed = Some((rev, now));
            self.protect.guard = self.doc.undo_len();
            return false;
        };
        let mut sel = None;
        for _ in 0..1000 {
            if skeleton(&self.doc) == good {
                break;
            }
            let s = if self.doc.undo_len() > self.protect.guard { self.doc.undo() } else if self.doc.redo_kind().is_some() { self.doc.redo() } else { None };
            match s {
                Some(s) => sel = Some(s),
                None => break,
            }
        }
        if let Some(s) = sel {
            self.sel = Selection::caret(self.doc.clamp(s.cursor));
        }
        // Whatever it came to is where the protection stands now.
        self.forms.allowed = Some((self.doc.revision, skeleton(&self.doc)));
        self.protect.guard = self.doc.undo_len();
        true
    }

    /// Forgets what the forms protection allowed (it isn't on).
    pub(super) fn forms_unprotected(&mut self) {
        self.forms.allowed = None;
    }

    pub(super) fn form_props_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.forms.dlg.take() else { return };
        let title = tr(match d.c.kind {
            ControlKind::CheckBox { .. } => "ffld.check_title",
            ControlKind::List { .. } => "ffld.drop_title",
            _ => "ffld.text_title",
        });
        let res = widgets::dialog(ctx, "doc_form_field", title, 440.0, |ui| {
            let f = &mut d.f;
            egui::Grid::new("ffld_grid").num_columns(2).min_col_width(180.0).spacing([12.0, 6.0]).show(ui, |ui| {
                match &mut d.c.kind {
                    ControlKind::CheckBox { .. } => {
                        ui.label(tr("ffld.size"));
                        ui.horizontal(|ui| {
                            let mut exact = f.check_size.is_some();
                            ui.radio_value(&mut exact, false, tr("ffld.size_auto"));
                            ui.radio_value(&mut exact, true, tr("ffld.size_exact"));
                            let mut pt = f.check_size.unwrap_or(20) as f32 / 2.0;
                            ui.add_enabled(exact, egui::DragValue::new(&mut pt).range(1.0..=1638.0).speed(0.5));
                            f.check_size = exact.then_some((pt * 2.0).round() as u32);
                        });
                        ui.end_row();
                        ui.label(tr("ffld.default_value"));
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut f.default_checked, false, tr("ffld.unchecked"));
                            ui.radio_value(&mut f.default_checked, true, tr("ffld.checked"));
                        });
                        ui.end_row();
                    }
                    ControlKind::List { .. } => {}
                    _ => {
                        ui.label(tr("ffld.type"));
                        egui::ComboBox::from_id_salt("ffld_type").selected_text(type_label(f.text_type)).width(200.0).show_ui(ui, |ui| {
                            for t in FormTextType::ALL {
                                ui.selectable_value(&mut f.text_type, t, type_label(t));
                            }
                        });
                        ui.end_row();
                        ui.label(tr("ffld.default_text"));
                        ui.add(egui::TextEdit::singleline(&mut f.default_text).desired_width(f32::INFINITY));
                        ui.end_row();
                        ui.label(tr("ffld.max_len"));
                        ui.add(egui::DragValue::new(&mut f.max_len).range(0..=255));
                        ui.end_row();
                        ui.label(tr("ffld.format"));
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut f.format).desired_width(170.0));
                            egui::ComboBox::from_id_salt("ffld_fmt").selected_text("").width(24.0).show_ui(ui, |ui| {
                                for x in formats(f.text_type) {
                                    ui.selectable_value(&mut f.format, x.to_string(), if x.is_empty() { "—" } else { x });
                                }
                            });
                        });
                        ui.end_row();
                    }
                }
                ui.label(tr("ffld.name"));
                ui.add(egui::TextEdit::singleline(&mut f.name).desired_width(f32::INFINITY));
                ui.end_row();
            });
            if let ControlKind::List { items, .. } = &mut d.c.kind {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut d.new_item).hint_text(tr("ffld.new_item")).desired_width(220.0));
                    if ui.add_enabled(!d.new_item.trim().is_empty(), egui::Button::new(tr("ctl.add"))).clicked() {
                        let t = d.new_item.trim().to_string();
                        items.push(ListItem { text: t.clone(), value: t });
                        d.new_item.clear();
                    }
                });
                ui.label(egui::RichText::new(tr("ffld.items")).strong());
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_min_size(vec2(ui.available_width(), 110.0));
                    egui::ScrollArea::vertical().max_height(110.0).show(ui, |ui| {
                        for (i, it) in items.iter().enumerate() {
                            let def = if i == f.default_item as usize { " ✓" } else { "" };
                            if ui.selectable_label(d.pick == Some(i), format!("{}{def}", it.text)).clicked() {
                                d.pick = Some(i);
                            }
                        }
                    });
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
                    if ui.add_enabled(picked.is_some(), egui::Button::new(tr("ffld.default_value").trim_end_matches(':'))).clicked()
                        && let Some(i) = picked
                    {
                        f.default_item = i as u32;
                    }
                });
            }
            ui.add_space(6.0);
            egui::Grid::new("ffld_more").num_columns(2).min_col_width(180.0).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label(tr("ffld.help"));
                ui.add(egui::TextEdit::singleline(&mut f.help).desired_width(f32::INFINITY));
                ui.end_row();
                ui.label(tr("ffld.status"));
                ui.add(egui::TextEdit::singleline(&mut f.status).desired_width(f32::INFINITY));
                ui.end_row();
                ui.label(tr("ffld.entry_macro"));
                ui.add(egui::TextEdit::singleline(&mut f.entry_macro).desired_width(f32::INFINITY));
                ui.end_row();
                ui.label(tr("ffld.exit_macro"));
                ui.add(egui::TextEdit::singleline(&mut f.exit_macro).desired_width(f32::INFINITY));
                ui.end_row();
            });
            ui.checkbox(&mut f.enabled, tr("ffld.enabled"));
            ui.checkbox(&mut f.calc_on_exit, tr("ffld.calc_exit"));
        });
        match res {
            DialogResult::None => self.forms.dlg = Some(d),
            DialogResult::Cancel => self.focus = true,
            DialogResult::Ok => {
                if let Some(at) = self.find_form(d.para, d.id) {
                    self.apply_form_props(&at, d.c, d.f);
                }
                self.focus = true;
            }
        }
    }

    fn find_form(&self, para: usize, id: u32) -> Option<ControlAt> {
        let p = self.doc.paragraphs.get(para)?;
        apex_doc::para_controls(p, para).into_iter().find(|c| c.control.id == id)
    }

    /// The field with its new settings; a new default shows as its value.
    fn apply_form_props(&mut self, at: &ControlAt, mut c: ContentControl, f: FormField) {
        let old = at.control.legacy.as_deref().cloned().unwrap_or_default();
        let cur = self.doc.paragraphs[at.para].text[at.range.clone()].to_string();
        let mut text = cur.clone();
        match &mut c.kind {
            ControlKind::CheckBox { checked } => {
                if f.default_checked != old.default_checked {
                    *checked = f.default_checked;
                }
                text = c.form_text(0);
            }
            ControlKind::List { items, .. } => {
                let pick = if f.default_item != old.default_item || !items.iter().any(|i| i.text == cur) { f.default_item as usize } else { apex_doc::form_item_index(items, &cur) };
                text = ContentControl { kind: ControlKind::List { items: items.clone(), editable: false }, ..c.clone() }.form_text(pick);
            }
            _ => {
                if f.default_text != old.default_text || (c.placeholder && !f.default_text.is_empty()) {
                    text = if f.default_text.is_empty() { FORM_BLANK.to_string() } else { f.default_text.clone() };
                    c.placeholder = f.default_text.is_empty();
                }
            }
        }
        c.legacy = Some(Box::new(f));
        if c != *at.control || text != cur {
            let p = self.doc.set_control(at, c, &text, self.sel);
            self.set_sel(Selection::caret(p));
        }
    }

    /// Whether the document is protected for filling in forms.
    pub(super) fn forms_only(&self) -> bool {
        self.doc.edit_limit() == Some(EditLimit::Forms) && !self.read_only
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "Name: [field] OK: [box]" with a text field of at most 4 upper-case
    /// characters.
    fn form() -> DocView {
        let mut d = Document::from_plain_text("Name:  OK: ");
        let f = FormField { name: "Nombre".into(), max_len: 4, format: "UPPERCASE".into(), ..Default::default() };
        d.insert_control(Selection::caret(Pos::new(0, 6)), ContentControl { placeholder: true, ..ContentControl::form(1, ControlKind::PlainText, f) }, FORM_BLANK);
        let end = d.paragraphs[0].text.len();
        d.insert_control(Selection::caret(Pos::new(0, end)), ContentControl::form(2, ControlKind::CheckBox { checked: false }, FormField::default()), &apex_doc::UNCHECKED.to_string());
        DocView::new(d)
    }

    #[test]
    fn typing_fits_the_field() {
        let mut v = form();
        let base = v.doc.undo_len();
        v.set_sel(Selection::caret(Pos::new(0, 6)));
        for c in ["a", "n", "a", "b", "c"] {
            v.insert(c);
        }
        let at = v.doc.controls()[0].clone();
        assert_eq!(&v.doc.paragraphs[0].text[at.range.clone()], "ANAB");
        assert!(!at.control.placeholder);
        // Undone, the blank comes back.
        while v.doc.undo_len() > base {
            v.undo();
        }
        assert!(v.doc.controls()[0].control.placeholder);
    }

    #[test]
    fn forms_protection_keeps_the_rest() {
        let mut v = form();
        v.doc.protection = Some(apex_doc::Protection { limit: EditLimit::Forms, enforced: true, password: None });
        v.enforce_protection();
        let text = v.doc.paragraphs[0].text.clone();
        // Typing outside a field is taken back.
        v.set_sel(Selection::caret(Pos::new(0, 0)));
        v.insert("x");
        v.enforce_protection();
        assert_eq!(v.doc.paragraphs[0].text, text);
        // In the field it stays.
        assert!(v.tab_to_field(false));
        assert_eq!(v.doc.control_at(v.sel.cursor).unwrap().control.id, 1);
        v.insert("z");
        v.enforce_protection();
        assert!(v.doc.paragraphs[0].text.contains("Name: Z"));
        // Tab goes on to the check box.
        assert!(v.tab_to_field(false));
        assert_eq!(v.doc.control_at(v.sel.cursor).unwrap().control.id, 2);
        // Undoing the fill is allowed too.
        v.undo();
        v.enforce_protection();
        assert!(v.doc.controls()[0].control.placeholder);
    }
}
