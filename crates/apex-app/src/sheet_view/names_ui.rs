//! Formulas > Defined Names: the Name Manager, Define Name, Create from
//! Selection and Paste Name, plus names in the Name Box.

use apex_sheet::names::{self, DefinedName};
use apex_sheet::NumFormat;

use super::f10::{self, F10Dlg, plain_window, ref_edit, tool_window};
use super::*;
use crate::widgets::DialogResult;

pub(super) enum NamesDlg {
    Manager { selected: usize },
    Edit(Box<NameEdit>),
    Create { top: bool, left: bool, bottom: bool, right: bool },
    /// Paste Name (F3): a name into the formula being written.
    Paste { selected: usize },
}

pub(super) struct NameEdit {
    /// Index in `Workbook::names` of the name being edited (None: new).
    orig: Option<usize>,
    name: String,
    /// 0: workbook, `i + 1`: sheet `i`.
    scope: usize,
    comment: String,
    refers: String,
    err: Option<String>,
    /// Go back to the Name Manager when done.
    manager: bool,
}

/// A value for the manager's list: numbers in the locale, arrays as
/// `{1;2;3}` (shortened).
fn value_text(v: &Value, loc: &apex_sheet::Locale) -> String {
    match v {
        Value::Array(m) => {
            let sep = if loc.decimal == ',' { ';' } else { ',' };
            let mut s = String::from("{");
            for (i, x) in m.data.iter().take(12).enumerate() {
                if i > 0 {
                    s.push(if m.cols > 1 && i % m.cols == 0 { ';' } else { sep });
                }
                s.push_str(&value_text(x, loc));
            }
            if m.data.len() > 12 {
                s.push('…');
            }
            s.push('}');
            s
        }
        Value::Text(t) => format!("\"{t}\""),
        Value::Lambda(_) => "LAMBDA".into(),
        v => NumFormat::general().format(v, loc).0,
    }
}

impl SheetView {
    pub(super) fn open_name_manager(&mut self) {
        self.open_f10(F10Dlg::Names(NamesDlg::Manager { selected: 0 }), None);
    }

    /// Define Name, for the selection.
    pub(super) fn open_define_name(&mut self) {
        let e = self.new_name_edit(false);
        self.open_f10(F10Dlg::Names(NamesDlg::Edit(Box::new(e))), Some(0));
        self.f10.pick.focus = false;
    }

    pub(super) fn open_create_names(&mut self) {
        let sel = self.sel();
        // Like Excel: guess where the labels are from the text in the edges.
        let is_text = |c: CellRef| matches!(self.sh().value(c), Value::Text(_));
        let top = (sel.start.col..=sel.end.col).any(|c| is_text(CellRef::new(sel.start.row, c))) && sel.rows() > 1;
        let left = (sel.start.row..=sel.end.row).any(|r| is_text(CellRef::new(r, sel.start.col))) && sel.cols() > 1;
        self.open_f10(F10Dlg::Names(NamesDlg::Create { top, left: left || !top, bottom: false, right: false }), None);
    }

    pub(super) fn open_paste_name(&mut self) {
        if self.wb.names.iter().any(|d| !d.hidden) {
            self.f10.dlg = Some(F10Dlg::Names(NamesDlg::Paste { selected: 0 }));
        }
    }

    fn new_name_edit(&self, manager: bool) -> NameEdit {
        let sel = self.sel();
        // The label next to the selection is a good name.
        let guess = [sel.start.row.checked_sub(1).map(|r| CellRef::new(r, sel.start.col)), sel.start.col.checked_sub(1).map(|c| CellRef::new(sel.start.row, c))]
            .into_iter()
            .flatten()
            .find_map(|c| match self.sh().value(c) {
                Value::Text(t) => names::name_from_label(t),
                _ => None,
            })
            .filter(|n| self.wb.find_name(self.sheet, n).is_none())
            .unwrap_or_default();
        let refers = format!("={}!{}", f10::quoted_sheet(&self.sh().name), f10::abs_range(&sel));
        NameEdit { orig: None, name: guess, scope: 0, comment: String::new(), refers, err: None, manager }
    }

    pub(super) fn names_dialog(&mut self, ctx: &egui::Context, d: NamesDlg) -> Option<NamesDlg> {
        match d {
            NamesDlg::Manager { selected } => self.manager_ui(ctx, selected),
            NamesDlg::Edit(e) => self.edit_name_ui(ctx, *e),
            NamesDlg::Create { top, left, bottom, right } => self.create_names_ui(ctx, [top, left, bottom, right]),
            NamesDlg::Paste { selected } => self.paste_name_ui(ctx, selected),
        }
    }

    fn manager_ui(&mut self, ctx: &egui::Context, mut selected: usize) -> Option<NamesDlg> {
        enum Act {
            New,
            Edit,
            Delete,
        }
        let mut act = None;
        let mut closed = false;
        let loc = self.wb.locale;
        let rows: Vec<(String, String, String, String, String)> = self
            .wb
            .names
            .iter()
            .map(|d| {
                let home = d.scope.as_deref().and_then(|s| self.wb.sheet_index(s)).unwrap_or(self.sheet);
                let text = match &d.scope {
                    Some(s) => format!("{}!{}", f10::quoted_sheet(s), d.name),
                    None => d.name.clone(),
                };
                // The names' formulas share one budget, as one formula.
                let v = apex_sheet::functions::shared_budget(1, 1 << 26, || self.wb.eval_formula(home, &format!("={text}")));
                let scope = d.scope.clone().unwrap_or_else(|| tr("names.workbook").to_string());
                (d.name.clone(), value_text(&v, &loc), d.expr.to_display(&loc), scope, d.comment.clone())
            })
            .collect();
        selected = selected.min(rows.len().saturating_sub(1));
        let open = plain_window(ctx, "names", tr("names.manager"), 720.0, |ui| {
            ui.horizontal(|ui| {
                if ui.button(format!("{}…", tr("names.new"))).clicked() {
                    act = Some(Act::New);
                }
                if ui.add_enabled(!rows.is_empty(), egui::Button::new(format!("{}…", tr("names.edit")))).clicked() {
                    act = Some(Act::Edit);
                }
                if ui.add_enabled(!rows.is_empty(), egui::Button::new(tr("names.delete"))).clicked() {
                    act = Some(Act::Delete);
                }
            });
            ui.add_space(4.0);
            egui::Frame::group(ui.style()).inner_margin(0.0).show(ui, |ui| {
                ui.set_min_size(vec2(ui.available_width(), 220.0));
                // The name column fits the longest name, within limits.
                let font = egui::TextStyle::Body.resolve(ui.style());
                let longest = rows.iter().map(|r| ui.fonts_mut(|f| f.layout_no_wrap(r.0.clone(), font.clone(), Color32::WHITE).size().x)).fold(0.0, f32::max);
                let widths = [(longest + 16.0).clamp(120.0, 240.0), 100.0, 200.0, 90.0];
                let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
                let mut x = head.min.x + 6.0;
                for (text, w) in [(tr("names.name"), widths[0]), (tr("names.value"), widths[1]), (tr("names.refers"), widths[2]), (tr("names.scope"), widths[3]), (tr("names.comment"), 200.0)] {
                    let clip = Rect::from_min_max(pos2(x, head.min.y), pos2((x + w - 6.0).min(head.max.x), head.max.y));
                    ui.painter().with_clip_rect(clip).text(pos2(x, head.center().y), egui::Align2::LEFT_CENTER, text, font.clone(), ui.visuals().strong_text_color());
                    x += w;
                }
                ui.separator();
                egui::ScrollArea::vertical().max_height(200.0).auto_shrink([false, false]).show(ui, |ui| {
                    if rows.is_empty() {
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.add_space(8.0);
                            ui.label(RichText::new(tr("names.none")).weak());
                        });
                    }
                    for (i, (name, value, refers, scope, comment)) in rows.iter().enumerate() {
                        let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
                        crate::widgets::name(&r, egui::WidgetType::SelectableLabel, name, Some(i == selected));
                        if i == selected {
                            ui.painter().rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
                        } else if r.hovered() {
                            ui.painter().rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
                        }
                        let color = if i == selected { ui.visuals().selection.stroke.color } else { ui.visuals().text_color() };
                        let mut x = rect.min.x + 6.0;
                        for (text, w) in [(name, widths[0]), (value, widths[1]), (refers, widths[2]), (scope, widths[3]), (comment, 200.0)] {
                            let clip = Rect::from_min_max(pos2(x, rect.min.y), pos2((x + w - 6.0).min(rect.max.x), rect.max.y));
                            ui.painter().with_clip_rect(clip).text(pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, text, egui::TextStyle::Body.resolve(ui.style()), color);
                            x += w;
                        }
                        if r.clicked() {
                            selected = i;
                        }
                        if r.double_clicked() {
                            selected = i;
                            act = Some(Act::Edit);
                        }
                    }
                });
            });
            if let Some(d) = rows.get(selected) {
                ui.add_space(6.0);
                ui.label(tr("names.refers_to"));
                ui.add(egui::Label::new(RichText::new(&d.2).monospace()).truncate());
            }
            ui.add_space(8.0);
            ui.allocate_ui_with_layout(vec2(ui.available_width(), 24.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("dialog.close")).clicked() {
                    closed = true;
                }
            });
        });
        if !open || closed {
            return None;
        }
        match act {
            Some(Act::New) => {
                let e = self.new_name_edit(true);
                self.f10.pick = f10::RefPick::new(self.sheet, Some(0));
                self.f10.pick.focus = false;
                Some(NamesDlg::Edit(Box::new(e)))
            }
            Some(Act::Edit) => {
                let d = self.wb.names.get(selected)?.clone();
                let scope = d.scope.as_deref().and_then(|s| self.wb.sheet_index(s)).map_or(0, |i| i + 1);
                self.f10.pick = f10::RefPick::new(self.sheet, Some(0));
                self.f10.pick.focus = false;
                Some(NamesDlg::Edit(Box::new(NameEdit { orig: Some(selected), name: d.name, scope, comment: d.comment, refers: d.expr.to_display(&self.wb.locale), err: None, manager: true })))
            }
            Some(Act::Delete) => {
                let mut list = self.wb.names.clone();
                if selected < list.len() {
                    list.remove(selected);
                    self.wb.set_names(list);
                }
                Some(NamesDlg::Manager { selected })
            }
            None => Some(NamesDlg::Manager { selected }),
        }
    }

    fn edit_name_ui(&mut self, ctx: &egui::Context, mut e: NameEdit) -> Option<NamesDlg> {
        let grid = self.grid_has_focus(ctx);
        let sheets: Vec<String> = self.wb.sheets.iter().map(|s| s.name.clone()).collect();
        let title = if e.orig.is_some() { tr("names.edit_title") } else { tr("names.new_title") };
        let res = tool_window(ctx, "name_edit", title, 420.0, grid, |ui| {
            widgets::form(ui, "name_edit_form", |ui| {
                widgets::form_row(ui, tr("names.name"), |ui| ui.add(egui::TextEdit::singleline(&mut e.name).desired_width(240.0)));
                widgets::form_row(ui, tr("names.scope"), |ui| {
                    let label = |i: usize| if i == 0 { tr("names.workbook").to_string() } else { sheets[i - 1].clone() };
                    // A name keeps its scope once made (as in Excel).
                    ui.add_enabled_ui(e.orig.is_none(), |ui| {
                        egui::ComboBox::from_id_salt("name_scope").selected_text(label(e.scope)).width(240.0).show_ui(ui, |ui| {
                            for i in 0..=sheets.len() {
                                ui.selectable_value(&mut e.scope, i, label(i));
                            }
                        });
                    })
                    .response
                });
                widgets::form_row(ui, tr("names.comment"), |ui| {
                    let id = ui.add(egui::TextEdit::multiline(&mut e.comment).desired_rows(3).desired_width(240.0)).id;
                    ui.data_mut(|m| m.insert_temp(id.with("multiline"), true));
                    ui.label("")
                });
                widgets::form_row(ui, tr("names.refers_to"), |ui| ref_edit(ui, &mut e.refers, 0, &mut self.f10.pick, 240.0));
            });
            if let Some(err) = &e.err {
                ui.add_space(4.0);
                ui.colored_label(ui.visuals().error_fg_color, err);
            }
        });
        let back = |manager: bool, i: usize| if manager { Some(NamesDlg::Manager { selected: i }) } else { None };
        match res {
            DialogResult::None => Some(NamesDlg::Edit(Box::new(e))),
            DialogResult::Cancel => back(e.manager, e.orig.unwrap_or(0)),
            DialogResult::Ok => match self.save_name(&e) {
                Ok(i) => back(e.manager, i),
                Err(msg) => {
                    e.err = Some(msg);
                    Some(NamesDlg::Edit(Box::new(e)))
                }
            },
        }
    }

    /// Checks and stores the edited name; its index in the list.
    fn save_name(&mut self, e: &NameEdit) -> Result<usize, String> {
        let name = e.name.trim();
        names::check_name(name).map_err(|k| tr(k).to_string())?;
        let scope = e.scope.checked_sub(1).and_then(|i| self.wb.sheets.get(i)).map(|s| s.name.clone());
        if self.wb.names.iter().enumerate().any(|(i, d)| Some(i) != e.orig && d.is(name) && d.scope == scope) {
            return Err(tr("names.err_taken").to_string());
        }
        let text = e.refers.trim();
        let text = if text.starts_with('=') { text.to_string() } else { format!("={text}") };
        let expr = apex_sheet::formula::parse(&text, &self.wb.locale).map_err(|_| tr("names.err_formula").to_string())?;
        let mut d = DefinedName::new(name, scope, expr);
        d.comment = e.comment.trim().to_string();
        let mut list = self.wb.names.clone();
        let mut renames = Vec::new();
        let i = match e.orig.filter(|&i| i < list.len()) {
            Some(i) => {
                d.hidden = list[i].hidden;
                if !list[i].is(name) {
                    renames.push((list[i].name.clone(), name.to_string()));
                }
                list[i] = d;
                i
            }
            None => {
                list.push(d);
                list.len() - 1
            }
        };
        self.wb.replace_names(list, &renames);
        Ok(i)
    }

    fn create_names_ui(&mut self, ctx: &egui::Context, mut s: [bool; 4]) -> Option<NamesDlg> {
        let grid = self.grid_has_focus(ctx);
        let res = tool_window(ctx, "names_create", tr("names.create_title"), 300.0, grid, |ui| {
            ui.label(tr("names.create_from"));
            ui.add_space(4.0);
            ui.indent("names_create", |ui| {
                for (i, key) in ["names.top_row", "names.left_col", "names.bottom_row", "names.right_col"].into_iter().enumerate() {
                    ui.checkbox(&mut s[i], tr(key));
                }
            });
        });
        match res {
            DialogResult::None => Some(NamesDlg::Create { top: s[0], left: s[1], bottom: s[2], right: s[3] }),
            DialogResult::Cancel => None,
            DialogResult::Ok => {
                let n = self.wb.names_from_selection(self.sheet, self.sel(), s[0], s[1], s[2], s[3]);
                if n == 0 {
                    self.notices.push(Notice::info(tr("names.none_made")));
                }
                None
            }
        }
    }

    fn paste_name_ui(&mut self, ctx: &egui::Context, mut selected: usize) -> Option<NamesDlg> {
        let list: Vec<String> = self.wb.names.iter().filter(|d| !d.hidden && d.scope.as_deref().is_none_or(|s| s == self.sh().name)).map(|d| d.name.clone()).collect();
        let mut chosen = None;
        let res = tool_window(ctx, "names_paste", tr("names.paste_title"), 280.0, false, |ui| {
            ui.label(tr("names.paste_label"));
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_min_size(vec2(ui.available_width(), 160.0));
                egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                    for (i, n) in list.iter().enumerate() {
                        let r = ui.add(egui::Button::selectable(i == selected, n).min_size(vec2(ui.available_width(), 0.0)));
                        if r.clicked() {
                            selected = i;
                        }
                        if r.double_clicked() {
                            chosen = Some(n.clone());
                        }
                    }
                });
            });
        });
        if res == DialogResult::Ok {
            chosen = list.get(selected).cloned();
        }
        match (chosen, res) {
            (Some(n), _) => {
                self.insert_in_formula(&n);
                None
            }
            (None, DialogResult::None) => Some(NamesDlg::Paste { selected }),
            _ => None,
        }
    }

    /// Types `text` into the formula being written (or starts one).
    pub(super) fn insert_in_formula(&mut self, text: &str) {
        if self.edit.is_none() {
            let cur = self.wb.edit_text(self.sheet, self.cursor);
            let t = if cur.starts_with('=') { format!("{cur}{text}") } else { format!("={text}") };
            if let Some(ctx) = self.ctx.clone() {
                self.begin_edit(&ctx, EditOrigin::Bar, false, Some(t));
            }
        } else {
            if !self.buf.starts_with('=') {
                self.buf.insert(0, '=');
            }
            self.buf.push_str(text);
            self.refocus = true;
        }
    }

    /// Use in Formula: the names, to insert one.
    pub(super) fn use_in_formula_menu(&mut self, ui: &mut egui::Ui) {
        let list: Vec<String> = self.wb.names.iter().filter(|d| !d.hidden && d.scope.as_deref().is_none_or(|s| s == self.sh().name)).map(|d| d.name.clone()).collect();
        if list.is_empty() {
            ui.label(RichText::new(tr("names.none")).weak());
        }
        for n in list {
            if ui.button(&n).clicked() {
                self.insert_in_formula(&n);
                ui.close();
            }
        }
        ui.separator();
        if ui.button(format!("{}…", tr("names.paste_title"))).clicked() {
            self.open_paste_name();
            ui.close();
        }
    }

    /// Name Box: a name selects its cells; a new valid name names the
    /// selection. True if `text` was handled as a name.
    pub(super) fn name_box_name(&mut self, text: &str) -> bool {
        let t = text.trim();
        if let Some((si, r)) = self.wb.name_target(self.sheet, t) {
            if si != self.sheet {
                self.select_sheet(si);
            }
            self.anchor = r.start;
            self.cursor = r.end;
            self.ensure_visible = true;
            return true;
        }
        if names::check_name(t).is_ok() && self.wb.find_table(t).is_none() && self.wb.find_name(self.sheet, t).is_none() {
            let sel = self.sel();
            self.wb.define_name(DefinedName::new(t, None, names::absolute_ref(&self.sh().name, sel)));
            return true;
        }
        false
    }

    /// Names that complete the word being typed in a formula.
    pub(super) fn name_completions(&self, word: &str) -> Vec<String> {
        let w = word.to_lowercase();
        self.wb.names.iter().filter(|d| !d.hidden && d.name.to_lowercase().starts_with(&w) && d.name.len() > w.len()).map(|d| d.name.clone()).collect()
    }
}
