//! Review > Check Accessibility (a side window listing the problems, with
//! quick fixes), Review > Allow Edit Ranges (and the password asked for
//! one of them), and Data > Form (the classic one-record-at-a-time box).

use apex_sheet::access::{self, Category, Issue, Target};
use apex_sheet::protect::EditRange;
use apex_sheet::{CellRef, FormField, Range, Workbook};
use eframe::egui::{self, Id, RichText};

use super::SheetView;
use crate::app::Notice;
use crate::i18n::{tr, trf};
use crate::widgets::{self, DialogResult};

#[derive(Default)]
pub(super) struct ReviewState {
    /// The accessibility window is open.
    pub access_open: bool,
    /// Issues found and the revision they were found at.
    access_cache: Option<(u64, Vec<Issue>)>,
    /// Selected issue (its index in the cache) for the explanation.
    access_sel: Option<usize>,
    alt: Option<AltDlg>,
    ranges: Option<RangesDlg>,
    range_edit: Option<RangeEdit>,
    /// Sheet and index of the edit range whose password is asked for.
    unlock: Option<(usize, usize, String)>,
    form: Option<FormState>,
}

/// Quick fix: alternative text for a picture, chart or shape.
struct AltDlg {
    target: Target,
    text: String,
}

/// Review > Allow Edit Ranges: the ranges being edited, saved together.
struct RangesDlg {
    sheet: usize,
    list: Vec<EditRange>,
    sel: Option<usize>,
}

/// New or modified range (`index` None: new).
struct RangeEdit {
    index: Option<usize>,
    title: String,
    refs: String,
    pw: String,
    /// The password is kept as it was (modifying a range with one).
    keep_pw: bool,
    bad_refs: bool,
}

/// Data > Form.
struct FormState {
    sheet: usize,
    list: Range,
    fields: Vec<String>,
    /// Record shown; the record count means a new record.
    index: usize,
    values: Vec<FormField>,
    dirty: bool,
    criteria: Option<Vec<String>>,
    /// Criteria used by Find Prev/Next.
    find: Vec<String>,
    revision: u64,
}

fn category_key(c: Category) -> &'static str {
    match c {
        Category::MissingAlt => "access.cat_alt",
        Category::NoHeader => "access.cat_header",
        Category::DefaultSheetName => "access.cat_sheet_name",
        Category::Contrast => "access.cat_contrast",
        Category::MergedCells => "access.cat_merged",
        Category::HiddenData => "access.cat_hidden",
    }
}

fn why_key(c: Category) -> &'static str {
    match c {
        Category::MissingAlt => "access.why_alt",
        Category::NoHeader => "access.why_header",
        Category::DefaultSheetName => "access.why_sheet_name",
        Category::Contrast => "access.why_contrast",
        Category::MergedCells => "access.why_merged",
        Category::HiddenData => "access.why_hidden",
    }
}

/// The quick fix offered for an issue.
fn fix_key(i: &Issue) -> Option<&'static str> {
    Some(match i.target {
        Target::Image { .. } | Target::Chart { .. } | Target::Shape { .. } => "access.fix_alt",
        Target::Sheet(_) => "access.fix_rename",
        Target::Cells { .. } if i.category == Category::Contrast => "access.fix_color",
        Target::Cells { .. } => "access.fix_unmerge",
        Target::Hidden { .. } => "access.fix_unhide",
        Target::Table { .. } => return None,
    })
}

impl SheetView {
    // --- Check Accessibility ------------------------------------------------------------

    pub(super) fn toggle_accessibility(&mut self) {
        self.settle_edit();
        self.review.access_open = !self.review.access_open;
    }

    fn access_issues(&mut self) -> &[Issue] {
        let rev = self.wb.revision;
        if self.review.access_cache.as_ref().is_none_or(|(r, _)| *r != rev) {
            self.review.access_cache = Some((rev, self.wb.check_accessibility()));
        }
        &self.review.access_cache.as_ref().unwrap().1
    }

    fn access_window(&mut self, ctx: &egui::Context) {
        if !self.review.access_open {
            return;
        }
        let issues = self.access_issues().to_vec();
        let mut open = true;
        let mut go: Option<usize> = None;
        let mut fix: Option<usize> = None;
        let sel = self.review.access_sel.filter(|i| *i < issues.len());
        let screen = ctx.content_rect();
        let at = egui::pos2(screen.max.x - 380.0, screen.min.y + 140.0);
        crate::widgets::esc_closes(ctx, Id::new(("access_window", self.uid)), &mut open);
        egui::Window::new(tr("access.title")).id(Id::new(("access_window", self.uid))).default_pos(at).open(&mut open).collapsible(false).resizable(true).default_width(340.0).default_height(460.0).show(ctx, |ui| {
            if issues.is_empty() {
                ui.label(RichText::new(tr("access.none")).strong());
                ui.label(RichText::new(tr("access.none_hint")).weak());
                return;
            }
            egui::ScrollArea::vertical().max_height((ui.available_height() - 110.0).max(160.0)).auto_shrink([false, true]).show(ui, |ui| {
                for errors in [true, false] {
                    let n = issues.iter().filter(|i| i.category.is_error() == errors).count();
                    if n == 0 {
                        continue;
                    }
                    let head = format!("{} ({n})", tr(if errors { "access.errors" } else { "access.warnings" }));
                    let color = if errors { ui.visuals().error_fg_color } else { ui.visuals().warn_fg_color };
                    egui::CollapsingHeader::new(RichText::new(head).strong().color(color)).id_salt(("access_level", errors)).default_open(true).show(ui, |ui| {
                        for cat in Category::ALL.into_iter().filter(|c| c.is_error() == errors) {
                            let items: Vec<usize> = (0..issues.len()).filter(|&k| issues[k].category == cat).collect();
                            if items.is_empty() {
                                continue;
                            }
                            egui::CollapsingHeader::new(format!("{} ({})", tr(category_key(cat)), items.len())).id_salt(("access_cat", cat)).default_open(true).show(ui, |ui| {
                                for k in items {
                                    let i = &issues[k];
                                    ui.horizontal(|ui| {
                                        let text = match i.ratio {
                                            Some(r) => format!("{}  ({r:.1}:1)", i.label),
                                            None => i.label.clone(),
                                        };
                                        let r = ui.add(egui::Button::selectable(sel == Some(k), text).truncate());
                                        if r.clicked() {
                                            go = Some(k);
                                        }
                                        if let Some(key) = fix_key(i) {
                                            if ui.small_button(tr(key)).on_hover_text(tr("access.fix_tip")).clicked() {
                                                fix = Some(k);
                                            }
                                        }
                                    });
                                }
                            });
                        }
                    });
                }
            });
            ui.separator();
            match sel.map(|k| issues[k].category) {
                Some(cat) => {
                    ui.label(RichText::new(tr("access.why")).strong());
                    ui.label(tr(why_key(cat)));
                }
                None => {
                    ui.label(RichText::new(tr("access.hint")).weak());
                }
            }
        });
        if !open {
            self.review.access_open = false;
        }
        if let Some(k) = go {
            self.review.access_sel = Some(k);
            self.go_to_issue(&issues[k].target);
        }
        if let Some(k) = fix {
            self.review.access_sel = Some(k);
            self.go_to_issue(&issues[k].target);
            self.fix_issue(&issues[k]);
        }
    }

    /// Shows the object or the cells an issue is about.
    fn go_to_issue(&mut self, t: &Target) {
        self.settle_edit();
        let s = t.sheet();
        if s >= self.wb.sheets.len() {
            return;
        }
        if s != self.sheet {
            self.select_sheet(s);
        }
        let sh = &self.wb.sheets[s];
        let place = |c: CellRef| Range::single(c);
        let area = match *t {
            Target::Image { id, .. } => sh.image(id).map(|x| place(x.from)),
            Target::Chart { id, .. } => sh.charts.iter().find(|x| x.id == id).map(|x| place(x.from)),
            Target::Shape { id, .. } => sh.shapes.iter().find(|x| x.id == id).map(|x| place(x.from)),
            Target::Table { index, .. } => sh.tables.get(index).map(|x| x.range),
            Target::Sheet(_) => None,
            Target::Cells { range, .. } => Some(range),
            Target::Hidden { rows, from, .. } => Some(place(if rows { CellRef::new(from, self.cursor.col) } else { CellRef::new(self.cursor.row, from) })),
        };
        if let Some(r) = area {
            self.select_areas(&[r]);
        }
        match *t {
            Target::Image { id, .. } => {
                self.images.selected = Some((s, id));
                self.images.charts.deselect();
                self.images.shapes.selected = None;
            }
            Target::Chart { id, .. } => {
                self.images.deselect_picture();
                self.images.shapes.selected = None;
                self.images.charts.selected = Some((s, id));
            }
            Target::Shape { id, .. } => self.select_shape(id),
            _ => {
                self.images.deselect_picture();
                self.images.charts.deselect();
                self.images.shapes.selected = None;
            }
        }
        self.ensure_visible = true;
    }

    fn fix_issue(&mut self, i: &Issue) {
        match i.target {
            Target::Image { sheet, id } => {
                let text = self.wb.sheets[sheet].image(id).map(|x| x.alt.clone()).unwrap_or_default();
                self.review.alt = Some(AltDlg { target: i.target.clone(), text });
            }
            Target::Chart { .. } | Target::Shape { .. } => self.review.alt = Some(AltDlg { target: i.target.clone(), text: String::new() }),
            Target::Sheet(s) => {
                self.renaming = Some((s, self.wb.sheets[s].name.clone()));
                self.rename_focus = true;
            }
            Target::Cells { sheet, range } if i.category == Category::Contrast => {
                let fill = self.wb.cell(sheet, range.start).map_or(0xFFFFFF, |c| self.wb.styles.get(c.style).fill.unwrap_or(0xFFFFFF));
                let color = access::readable_on(fill);
                self.wb.modify_style(sheet, &range, |s| {
                    s.color = Some(color);
                    s.theme.color = None;
                });
            }
            Target::Cells { sheet, range } => self.wb.unmerge(sheet, &range),
            Target::Hidden { sheet, rows, from, to } => self.wb.set_hidden(sheet, rows, from, to, false),
            Target::Table { .. } => {}
        }
    }

    fn alt_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.review.alt.take() else { return };
        let res = widgets::dialog(ctx, "access_alt", tr("access.alt_title"), 380.0, |ui| {
            ui.label(tr("access.alt_prompt"));
            let r = ui.add(egui::TextEdit::multiline(&mut d.text).desired_rows(3).desired_width(f32::INFINITY));
            if ui.memory(|m| m.focused().is_none()) {
                r.request_focus();
            }
            ui.label(RichText::new(tr("access.alt_hint")).weak().small());
        });
        match res {
            DialogResult::None => self.review.alt = Some(d),
            DialogResult::Cancel => self.focus_grid = true,
            DialogResult::Ok => {
                self.focus_grid = true;
                let text = d.text.trim().to_string();
                match d.target {
                    Target::Image { sheet, id } => {
                        if let Some(mut img) = self.wb.sheets[sheet].image(id).cloned() {
                            img.alt = text;
                            self.wb.set_image(sheet, img);
                        }
                    }
                    Target::Chart { sheet, id } => {
                        if let Some(mut c) = self.wb.sheets[sheet].charts.iter().find(|c| c.id == id).cloned() {
                            c.alt = text;
                            self.wb.set_chart(sheet, c);
                        }
                    }
                    Target::Shape { sheet, id } => {
                        if let Some(mut s) = self.wb.sheets[sheet].shapes.iter().find(|s| s.id == id).cloned() {
                            s.shape.alt = text;
                            self.wb.set_shape(sheet, s);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // --- Allow Edit Ranges ----------------------------------------------------------------

    pub(super) fn open_edit_ranges(&mut self) {
        self.settle_edit();
        let sheet = self.sheet;
        self.review.ranges = Some(RangesDlg { sheet, list: self.wb.sheets[sheet].edit_ranges.clone(), sel: None });
    }

    fn edit_ranges_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.review.ranges.take() else { return };
        if d.sheet >= self.wb.sheets.len() {
            return;
        }
        let protected = self.wb.is_protected(d.sheet);
        let mut open = true;
        let (mut ok, mut cancel, mut protect, mut new, mut modify, mut delete) = (false, false, false, false, false, false);
        crate::widgets::esc_closes(ctx, Id::new(("edit_ranges", self.uid)), &mut open);
        egui::Window::new(tr("ranges.title")).id(Id::new(("edit_ranges", self.uid))).pivot(egui::Align2::CENTER_CENTER).default_pos(ctx.content_rect().center()).open(&mut open).collapsible(false).resizable(false).default_width(470.0).show(ctx, |ui| {
            ui.label(tr("ranges.list"));
            egui::Frame::new().fill(ui.visuals().extreme_bg_color).inner_margin(4).show(ui, |ui| {
                ui.set_min_size(egui::vec2(440.0, 130.0));
                egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                    egui::Grid::new("ranges_grid").num_columns(3).striped(true).min_col_width(60.0).show(ui, |ui| {
                        for key in ["ranges.col_title", "ranges.col_refs", "ranges.col_password"] {
                            ui.label(RichText::new(tr(key)).strong());
                        }
                        ui.end_row();
                        for (k, e) in d.list.iter().enumerate() {
                            let on = d.sel == Some(k);
                            if ui.add(egui::Button::selectable(on, &e.title).truncate().min_size(egui::vec2(140.0, 18.0))).clicked() {
                                d.sel = Some(k);
                            }
                            if ui.add(egui::Button::selectable(on, e.refers_to()).truncate().min_size(egui::vec2(180.0, 18.0))).double_clicked() {
                                d.sel = Some(k);
                                modify = true;
                            }
                            ui.label(if e.has_password() { "••••" } else { "" });
                            ui.end_row();
                        }
                    });
                });
            });
            ui.add_enabled_ui(!protected, |ui| {
                ui.horizontal(|ui| {
                    if ui.button(format!("{}…", tr("ranges.new"))).clicked() {
                        new = true;
                    }
                    if ui.add_enabled(d.sel.is_some(), egui::Button::new(format!("{}…", tr("ranges.modify")))).clicked() {
                        modify = true;
                    }
                    if ui.add_enabled(d.sel.is_some(), egui::Button::new(tr("ranges.delete"))).clicked() {
                        delete = true;
                    }
                });
            });
            if protected {
                ui.colored_label(ui.visuals().warn_fg_color, tr("ranges.protected"));
            }
            ui.add_space(4.0);
            ui.label(RichText::new(tr("ranges.hint")).weak().small());
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let key = if protected { "protect.unprotect_sheet" } else { "protect.sheet_title" };
                if ui.button(format!("{}…", tr(key))).clicked() {
                    protect = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tr("unsaved.cancel")).clicked() {
                        cancel = true;
                    }
                    if ui.button(RichText::new(tr("dialog.ok")).strong()).clicked() {
                        ok = true;
                    }
                });
            });
        });
        if new {
            let refs = self.areas().iter().map(|r| r.to_a1()).collect::<Vec<_>>().join(",");
            let title = (1..).map(|n| trf("ranges.default_title", &[("n", &n)])).find(|t| !d.list.iter().any(|e| &e.title == t)).unwrap_or_default();
            self.review.range_edit = Some(RangeEdit { index: None, title, refs, pw: String::new(), keep_pw: false, bad_refs: false });
        }
        if let Some(k) = d.sel.filter(|_| modify) {
            let e = &d.list[k];
            self.review.range_edit = Some(RangeEdit { index: Some(k), title: e.title.clone(), refs: e.refers_to(), pw: String::new(), keep_pw: e.has_password(), bad_refs: false });
        }
        if let Some(k) = d.sel.filter(|_| delete) {
            d.list.remove(k);
            d.sel = None;
        }
        if ok || protect {
            let (sheet, list) = (d.sheet, d.list.clone());
            if self.wb.sheets[sheet].edit_ranges != list {
                self.wb.set_edit_ranges(sheet, list);
            }
        }
        if protect {
            self.protect_cmd(Some(d.sheet));
        }
        if open && !ok && !cancel && !protect {
            self.review.ranges = Some(d);
        } else {
            self.focus_grid = true;
        }
    }

    fn range_edit_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut e) = self.review.range_edit.take() else { return };
        let title = if e.index.is_some() { tr("ranges.modify_title") } else { tr("ranges.new_title") };
        let res = widgets::dialog(ctx, "range_edit", title, 360.0, |ui| {
            egui::Grid::new("range_edit_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label(tr("ranges.field_title"));
                ui.add(egui::TextEdit::singleline(&mut e.title).desired_width(200.0));
                ui.end_row();
                ui.label(tr("ranges.field_refs"));
                ui.add(egui::TextEdit::singleline(&mut e.refs).desired_width(200.0));
                ui.end_row();
                ui.label(tr("ranges.field_password"));
                let hint = if e.keep_pw { tr("ranges.password_kept") } else { "" };
                if ui.add(egui::TextEdit::singleline(&mut e.pw).password(true).hint_text(hint).desired_width(200.0)).changed() {
                    e.keep_pw = false;
                }
                ui.end_row();
            });
            if e.bad_refs {
                ui.colored_label(ui.visuals().error_fg_color, tr("ranges.bad_refs"));
            }
        });
        match res {
            DialogResult::None => self.review.range_edit = Some(e),
            DialogResult::Cancel => {}
            DialogResult::Ok => {
                let Some(ranges) = EditRange::parse_refs(&e.refs) else {
                    e.bad_refs = true;
                    self.review.range_edit = Some(e);
                    return;
                };
                let Some(d) = self.review.ranges.as_mut() else { return };
                let title = if e.title.trim().is_empty() { e.refs.trim().to_string() } else { e.title.trim().to_string() };
                let mut r = EditRange::new(title, ranges, &e.pw);
                match e.index.and_then(|k| d.list.get_mut(k)) {
                    Some(old) => {
                        if e.keep_pw {
                            r.passwords = old.passwords.clone();
                        }
                        *old = r;
                    }
                    None => {
                        d.list.push(r);
                        d.sel = Some(d.list.len() - 1);
                    }
                }
            }
        }
    }

    /// Asks for the password of the edit range holding `c` when it needs
    /// one; true when it was asked.
    pub(super) fn ask_range_password(&mut self, c: CellRef) -> bool {
        match self.wb.edit_range_asking(self.sheet, c) {
            Some(i) => {
                if self.review.unlock.is_none() {
                    self.review.unlock = Some((self.sheet, i, String::new()));
                }
                true
            }
            None => false,
        }
    }

    fn unlock_dialog(&mut self, ctx: &egui::Context) {
        let Some((sheet, index, mut pw)) = self.review.unlock.take() else { return };
        let res = widgets::dialog(ctx, "unlock_range", tr("ranges.unlock_title"), 340.0, |ui| {
            ui.label(tr("ranges.unlock_prompt"));
            let r = ui.add(egui::TextEdit::singleline(&mut pw).password(true).desired_width(f32::INFINITY));
            if ui.memory(|m| m.focused().is_none()) {
                r.request_focus();
            }
        });
        match res {
            DialogResult::None => self.review.unlock = Some((sheet, index, pw)),
            DialogResult::Cancel => self.focus_grid = true,
            DialogResult::Ok => {
                self.focus_grid = true;
                if !self.wb.unlock_edit_range(sheet, index, &pw) {
                    self.notices.push(Notice::error(tr("protect.wrong_password")));
                }
            }
        }
    }

    // --- Data Form --------------------------------------------------------------------------

    pub(super) fn open_data_form(&mut self) {
        self.settle_edit();
        let Some(list) = self.wb.form_list(self.sheet, self.cursor) else {
            self.notices.push(Notice::info(tr("form.no_list")));
            return;
        };
        let fields = self.wb.form_fields(self.sheet, &list);
        let mut f = FormState { sheet: self.sheet, list, fields, index: 0, values: Vec::new(), dirty: false, criteria: None, find: Vec::new(), revision: 0 };
        // Start at the record of the active cell.
        if self.cursor.row > list.start.row && self.cursor.row <= list.end.row {
            f.index = (self.cursor.row - list.start.row - 1) as usize;
        }
        self.load_record(&mut f);
        self.review.form = Some(f);
    }

    fn load_record(&self, f: &mut FormState) {
        let n = Workbook::form_count(&f.list);
        f.index = f.index.min(n);
        f.values = if f.index < n {
            self.wb.form_record(f.sheet, &f.list, f.index)
        } else {
            // A new record: computed fields show nothing yet.
            let last = if n > 0 { self.wb.form_record(f.sheet, &f.list, n - 1) } else { Vec::new() };
            (0..f.fields.len()).map(|k| FormField { text: String::new(), computed: last.get(k).is_some_and(|x| x.computed) }).collect()
        };
        f.dirty = false;
        f.revision = self.wb.revision;
    }

    /// Saves the changed fields of the record shown.
    fn commit_record(&mut self, f: &mut FormState) {
        if !f.dirty {
            return;
        }
        let values: Vec<String> = f.values.iter().map(|v| v.text.clone()).collect();
        let n = Workbook::form_count(&f.list);
        if f.index < n {
            self.wb.form_update(f.sheet, &f.list, f.index, &values);
        } else {
            f.list = self.wb.form_add(f.sheet, &f.list, &values);
        }
        f.dirty = false;
    }

    fn data_form_window(&mut self, ctx: &egui::Context) {
        let Some(mut f) = self.review.form.take() else { return };
        if f.sheet >= self.wb.sheets.len() {
            return;
        }
        // Changed elsewhere (undo): read the record again.
        if !f.dirty && f.criteria.is_none() && f.revision != self.wb.revision {
            if let Some(list) = self.wb.form_list(f.sheet, f.list.start) {
                if list.start == f.list.start && list.cols() == f.list.cols() {
                    f.list = list;
                }
            }
            self.load_record(&mut f);
        }
        let n = Workbook::form_count(&f.list);
        let mut open = true;
        #[derive(PartialEq)]
        enum Act {
            None,
            New,
            Delete,
            Restore,
            Prev,
            Next,
            Criteria,
            Close,
            Enter,
        }
        let mut act = Act::None;
        let title = self.wb.sheets[f.sheet].name.clone();
        let at = ctx.content_rect().center() - egui::vec2(230.0, 200.0);
        crate::widgets::esc_closes(ctx, Id::new(("data_form", self.uid)), &mut open);
        egui::Window::new(title).id(Id::new(("data_form", self.uid))).default_pos(at).open(&mut open).collapsible(false).resizable(false).min_width(440.0).show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(300.0);
                    egui::ScrollArea::vertical().max_height(360.0).auto_shrink([false, true]).show(ui, |ui| {
                        egui::Grid::new("form_grid").num_columns(2).spacing([8.0, 5.0]).show(ui, |ui| {
                            for k in 0..f.fields.len() {
                                ui.label(format!("{}:", f.fields[k]));
                                match &mut f.criteria {
                                    Some(c) => {
                                        ui.add(egui::TextEdit::singleline(&mut c[k]).desired_width(190.0));
                                    }
                                    None if f.values[k].computed => {
                                        ui.add_sized([190.0, 18.0], egui::Label::new(f.values[k].text.clone()).truncate());
                                    }
                                    None => {
                                        let r = ui.add(egui::TextEdit::singleline(&mut f.values[k].text).desired_width(190.0));
                                        if r.changed() {
                                            f.dirty = true;
                                        }
                                        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                            act = Act::Enter;
                                        }
                                    }
                                }
                                ui.end_row();
                            }
                        });
                    });
                });
                ui.separator();
                ui.vertical(|ui| {
                    let status = if f.criteria.is_some() {
                        tr("form.criteria").to_string()
                    } else if f.index >= n {
                        tr("form.new_record").to_string()
                    } else {
                        trf("form.record_of", &[("n", &(f.index + 1)), ("m", &n)])
                    };
                    ui.label(RichText::new(status).strong());
                    ui.add_space(6.0);
                    let w = egui::vec2(120.0, 22.0);
                    let crit = f.criteria.is_some();
                    let b = |ui: &mut egui::Ui, on: bool, key: &'static str, tip: &'static str| ui.add_enabled(on, egui::Button::new(tr(key)).min_size(w)).on_hover_text(tr(tip)).clicked();
                    // Criteria: Clear with some typed, Restore when they
                    // differ from the ones last used.
                    let (clear, restore) = match &f.criteria {
                        Some(c) => (c.iter().any(|x| !x.is_empty()), *c != f.find),
                        None => (true, f.dirty),
                    };
                    if b(ui, clear, if crit { "form.clear" } else { "form.new" }, if crit { "form.clear_tip" } else { "form.new_tip" }) {
                        act = Act::New;
                    }
                    if b(ui, !crit && f.index < n, "form.delete", "form.delete_tip") {
                        act = Act::Delete;
                    }
                    if b(ui, restore, "form.restore", "form.restore_tip") {
                        act = Act::Restore;
                    }
                    ui.add_space(6.0);
                    if b(ui, true, "form.find_prev", "form.find_prev_tip") {
                        act = Act::Prev;
                    }
                    if b(ui, true, "form.find_next", "form.find_next_tip") {
                        act = Act::Next;
                    }
                    if b(ui, true, if crit { "form.form" } else { "form.criteria" }, "form.criteria_tip") {
                        act = Act::Criteria;
                    }
                    ui.add_space(6.0);
                    if b(ui, true, "form.close", "form.close_tip") {
                        act = Act::Close;
                    }
                });
            });
        });
        if !open {
            act = Act::Close;
        }
        let find = |v: &SheetView, f: &FormState, forward: bool| v.wb.form_find(f.sheet, &f.list, Some(f.index).filter(|i| *i < n), forward, &f.find);
        match act {
            Act::None => {}
            Act::New if f.criteria.is_some() => f.criteria = Some(vec![String::new(); f.fields.len()]),
            Act::New => {
                self.commit_record(&mut f);
                f.index = Workbook::form_count(&f.list);
                self.load_record(&mut f);
            }
            Act::Delete => {
                f.list = self.wb.form_delete(f.sheet, &f.list, f.index);
                self.load_record(&mut f);
            }
            Act::Restore if f.criteria.is_some() => f.criteria = Some(f.find.clone()),
            Act::Restore => self.load_record(&mut f),
            Act::Prev | Act::Next => {
                if let Some(c) = f.criteria.take() {
                    f.find = c;
                }
                self.commit_record(&mut f);
                let n = Workbook::form_count(&f.list);
                match find(self, &f, act == Act::Next) {
                    Some(i) => f.index = i,
                    None if act == Act::Next && f.find.iter().all(|c| c.trim().is_empty()) => f.index = n,
                    None => {}
                }
                self.load_record(&mut f);
                self.show_record(&f);
            }
            Act::Criteria => match f.criteria.take() {
                Some(c) => {
                    f.find = c;
                    self.load_record(&mut f);
                }
                None => {
                    self.commit_record(&mut f);
                    let len = f.fields.len();
                    f.find.resize(len, String::new());
                    f.criteria = Some(f.find.clone());
                }
            },
            Act::Enter => {
                // Enter saves and moves on, to a new record after the last.
                let was_new = f.index >= Workbook::form_count(&f.list);
                self.commit_record(&mut f);
                f.index = if was_new { Workbook::form_count(&f.list) } else { f.index + 1 };
                self.load_record(&mut f);
                self.show_record(&f);
            }
            Act::Close => {
                if f.criteria.is_none() {
                    self.commit_record(&mut f);
                }
                self.focus_grid = true;
                return;
            }
        }
        self.review.form = Some(f);
    }

    /// Moves the cursor to the record shown.
    fn show_record(&mut self, f: &FormState) {
        if f.sheet != self.sheet || f.index >= Workbook::form_count(&f.list) {
            return;
        }
        let c = CellRef::new(f.list.start.row + 1 + f.index as u32, f.list.start.col);
        self.anchor = c;
        self.cursor = c;
        self.ensure_visible = true;
    }

    pub(super) fn review_popups(&mut self, ctx: &egui::Context) {
        self.access_window(ctx);
        self.alt_dialog(ctx);
        self.edit_ranges_window(ctx);
        self.range_edit_dialog(ctx);
        self.unlock_dialog(ctx);
        self.data_form_window(ctx);
    }
}
