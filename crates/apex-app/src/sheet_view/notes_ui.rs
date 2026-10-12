//! Notes (red triangle, hover box, editor), hyperlinks (Ctrl+K dialog,
//! Ctrl+click), sheet and workbook protection dialogs, and the Insert
//! Symbol dialog.

use apex_sheet::protect::allow;
use apex_sheet::{CellRef, Link, Note, Protection, Range};
use eframe::egui::{self, Color32, FontId, Id, Rect, RichText, Shape, Stroke, StrokeKind, pos2, vec2};

use super::{EditOrigin, Geo, SheetView};
use crate::app::Notice;
use crate::common;
use crate::i18n::tr;
use crate::symbols::SymbolDialog;
use crate::widgets::{self, DialogResult};

const MARK: Color32 = Color32::from_rgb(0xD0, 0x10, 0x10);
const NOTE_BG: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xE1);
const NOTE_BORDER: Color32 = Color32::from_rgb(0x80, 0x80, 0x60);
const NOTE_W: f32 = 180.0;
/// Threaded comments: marker and card.
const THREAD_MARK: Color32 = Color32::from_rgb(0x70, 0x30, 0xA0);
const THREAD_BG: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
const THREAD_W: f32 = 240.0;
/// Excel's Hyperlink cell style.
pub(super) const LINK_COLOR: u32 = 0x0563C1;

#[derive(Default)]
pub(super) struct NotesState {
    edit: Option<NoteEdit>,
    link: Option<LinkDlg>,
    protect: Option<ProtectDlg>,
    unprotect: Option<(Option<usize>, String)>,
    pub symbols: SymbolDialog,
}

struct NoteEdit {
    sheet: usize,
    cell: CellRef,
    text: String,
    author: String,
    focus: bool,
    /// A threaded comment: shows its replies and takes a new one.
    threaded: bool,
    resolved: bool,
    reply: String,
}

/// `2024-05-01T10:30:00` as `2024-05-01 10:30`.
fn short_date(d: &str) -> String {
    let d = d.trim_end_matches('Z');
    match d.split_once('T') {
        Some((day, time)) => format!("{day} {}", time.get(..5).unwrap_or(time)),
        None => d.to_string(),
    }
}

struct LinkDlg {
    cell: CellRef,
    /// Text shown in the cell; `None` when the cell holds a formula or number.
    text: Option<String>,
    /// 0 = web page or file, 1 = place in this workbook, 2 = e-mail.
    kind: u8,
    url: String,
    sheet: usize,
    place: String,
    email: String,
    subject: String,
    tip: String,
    editing: bool,
}

struct ProtectDlg {
    /// `None` protects the workbook's structure.
    sheet: Option<usize>,
    pw: String,
    confirm: String,
    allow: u32,
    mismatch: bool,
}

/// The allow flags offered in the Protect Sheet dialog, in Excel's order.
const ALLOW_UI: [(u32, &str); 15] = [
    (allow::SELECT_LOCKED, "protect.allow_select_locked"),
    (allow::SELECT_UNLOCKED, "protect.allow_select_unlocked"),
    (allow::FORMAT_CELLS, "protect.allow_format_cells"),
    (allow::FORMAT_COLUMNS, "protect.allow_format_cols"),
    (allow::FORMAT_ROWS, "protect.allow_format_rows"),
    (allow::INSERT_COLUMNS, "protect.allow_insert_cols"),
    (allow::INSERT_ROWS, "protect.allow_insert_rows"),
    (allow::INSERT_LINKS, "protect.allow_insert_links"),
    (allow::DELETE_COLUMNS, "protect.allow_delete_cols"),
    (allow::DELETE_ROWS, "protect.allow_delete_rows"),
    (allow::SORT, "protect.allow_sort"),
    (allow::AUTOFILTER, "protect.allow_autofilter"),
    (allow::PIVOT_TABLES, "protect.allow_pivot"),
    (allow::OBJECTS, "protect.allow_objects"),
    (allow::SCENARIOS, "protect.allow_scenarios"),
];

/// Splits `Sheet!A1` or `'My sheet'!A1:B2` into a sheet index and range.
pub(super) fn parse_place(wb: &apex_sheet::Workbook, place: &str, current: usize) -> Option<(usize, Range)> {
    let (sheet, cell) = match place.rsplit_once('!') {
        Some((s, c)) => {
            let name = s.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')).map_or(s.to_string(), |x| x.replace("''", "'"));
            (wb.sheets.iter().position(|sh| sh.name.eq_ignore_ascii_case(&name))?, c)
        }
        None => (current, place),
    };
    let cell = cell.replace('$', "");
    let r = Range::parse(&cell).or_else(|| CellRef::parse(&cell).map(Range::single))?;
    Some((sheet, r))
}

fn quote_sheet(name: &str) -> String {
    if name.chars().all(|c| c.is_alphanumeric() || c == '_') && !name.starts_with(|c: char| c.is_ascii_digit()) {
        name.to_string()
    } else {
        format!("'{}'", name.replace('\'', "''"))
    }
}

impl SheetView {
    // --- notes ------------------------------------------------------------------------

    pub(super) fn note_at(&self, c: CellRef) -> Option<&Note> {
        self.wb.cell(self.sheet, c)?.note()
    }

    /// Opens the note editor on the cursor cell (new or existing note).
    pub(super) fn edit_note(&mut self) {
        self.settle_edit();
        let c = self.cursor;
        let (text, author, threaded, resolved) = match self.note_at(c) {
            Some(n) => (n.text.clone(), n.author.clone(), n.threaded, n.resolved),
            None => (String::new(), common::user_name(), false, false),
        };
        self.notes.edit = Some(NoteEdit { sheet: self.sheet, cell: c, text, author, focus: true, threaded, resolved, reply: String::new() });
    }

    /// A new threaded comment on the cursor cell, or a reply to its thread.
    pub(super) fn new_comment(&mut self) {
        let fresh = self.note_at(self.cursor).is_none();
        self.edit_note();
        if let Some(e) = self.notes.edit.as_mut().filter(|_| fresh) {
            e.threaded = true;
        }
    }

    pub(super) fn delete_notes(&mut self) {
        let r = self.sel();
        self.wb.clear_extras(self.sheet, &r, false);
    }

    pub(super) fn toggle_note_shown(&mut self) {
        let shown = self.note_at(self.cursor).is_some_and(|n| n.visible);
        let r = self.sel();
        self.wb.show_notes(self.sheet, Some(&r), !shown);
    }

    pub(super) fn any_note_hidden(&self) -> bool {
        self.wb.notes(self.sheet).iter().any(|(_, n)| !n.visible)
    }

    pub(super) fn show_all_notes(&mut self) {
        let show = self.any_note_hidden();
        self.wb.show_notes(self.sheet, None, show);
    }

    /// Moves the cursor to the next (or previous) note in reading order.
    pub(super) fn step_note(&mut self, back: bool) {
        let mut cells: Vec<CellRef> = self.wb.notes(self.sheet).into_iter().map(|(c, _)| c).collect();
        cells.sort_unstable_by_key(|c| (c.row, c.col));
        let key = (self.cursor.row, self.cursor.col);
        let next = if back { cells.iter().rev().find(|c| (c.row, c.col) < key).or(cells.last()) } else { cells.iter().find(|c| (c.row, c.col) > key).or(cells.first()) };
        if let Some(&c) = next {
            self.select(c, false);
        }
    }

    fn note_editor(&mut self, ctx: &egui::Context) {
        let Some(mut e) = self.notes.edit.take() else { return };
        if e.sheet != self.sheet {
            return;
        }
        let Some(cr) = self.data.cursor_rect.filter(|_| self.cursor == e.cell).or_else(|| self.data.cursor_rect) else {
            self.notes.edit = Some(e);
            return;
        };
        let id = Id::new(("note_edit", self.uid));
        let mut done = false;
        let replies = self.wb.cell(e.sheet, e.cell).and_then(|c| c.note()).map(|n| (n.date.clone(), n.replies.clone())).unwrap_or_default();
        let area = egui::Area::new(Id::new(("note_area", self.uid))).order(egui::Order::Foreground).fixed_pos(pos2(cr.max.x + 12.0, cr.min.y)).constrain(true).show(ctx, |ui| {
            if e.threaded {
                self.thread_editor(ui, &mut e, id, &replies.0, &replies.1);
                return;
            }
            egui::Frame::new().fill(NOTE_BG).stroke(Stroke::new(1.0, NOTE_BORDER)).inner_margin(6.0).show(ui, |ui| {
                ui.set_width(NOTE_W + 20.0);
                if !e.author.is_empty() {
                    ui.label(RichText::new(format!("{}:", e.author)).strong().color(Color32::BLACK));
                }
                let r = ui.add(egui::TextEdit::multiline(&mut e.text).id(id).desired_rows(4).desired_width(NOTE_W + 8.0).frame(egui::Frame::NONE).text_color(Color32::BLACK));
                ui.data_mut(|d| d.insert_temp(id.with("multiline"), true));
                if std::mem::take(&mut e.focus) {
                    r.request_focus();
                }
            });
        });
        let outside = ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !area.response.rect.contains(p)));
        if outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            done = true;
        }
        if !done {
            self.notes.edit = Some(e);
            return;
        }
        let old = self.wb.cell(e.sheet, e.cell).and_then(|c| c.note()).cloned();
        let text = e.text.trim_end().to_string();
        let mut new = old.clone().unwrap_or_default();
        new.text = text;
        new.author = e.author;
        new.threaded = e.threaded;
        new.resolved = e.resolved;
        if e.threaded && new.date.is_empty() {
            new.date = common::local_iso();
        }
        let reply = e.reply.trim_end();
        if !reply.is_empty() {
            new.replies.push(apex_sheet::Reply { text: reply.to_string(), author: common::user_name(), date: common::local_iso() });
        }
        let new = (!new.text.is_empty() || !new.replies.is_empty()).then_some(new);
        if new != old {
            self.wb.set_note(e.sheet, e.cell, new);
        }
        self.focus_grid = true;
    }

    /// The card of a threaded comment: the first comment (editable), its
    /// replies and a box to answer.
    fn thread_editor(&self, ui: &mut egui::Ui, e: &mut NoteEdit, id: Id, date: &str, replies: &[apex_sheet::Reply]) {
        let head = |ui: &mut egui::Ui, author: &str, date: &str| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(author).strong().color(Color32::BLACK));
                if !date.is_empty() {
                    ui.label(RichText::new(short_date(date)).small().color(Color32::from_gray(110)));
                }
            });
        };
        egui::Frame::new().fill(THREAD_BG).stroke(Stroke::new(1.0, Color32::from_gray(170))).corner_radius(4.0).inner_margin(8.0).show(ui, |ui| {
            ui.set_width(THREAD_W);
            ui.visuals_mut().override_text_color = Some(Color32::BLACK);
            head(ui, &e.author, date);
            let r = ui.add(egui::TextEdit::multiline(&mut e.text).id(id).desired_rows(2).desired_width(THREAD_W).frame(egui::Frame::NONE).text_color(Color32::BLACK));
            ui.data_mut(|d| d.insert_temp(id.with("multiline"), true));
            for rp in replies {
                ui.separator();
                head(ui, &rp.author, &rp.date);
                ui.label(RichText::new(&rp.text).color(Color32::BLACK));
            }
            ui.separator();
            let rr = ui.add(
                egui::TextEdit::multiline(&mut e.reply)
                    .id(id.with("reply"))
                    .hint_text(tr("comment.reply_hint"))
                    .desired_rows(1)
                    .desired_width(THREAD_W)
                    .text_color(Color32::BLACK)
                    .background_color(Color32::from_gray(245)),
            );
            ui.data_mut(|d| d.insert_temp(id.with("reply").with("multiline"), true));
            ui.checkbox(&mut e.resolved, RichText::new(tr("comment.resolved")).color(Color32::BLACK));
            if std::mem::take(&mut e.focus) {
                if e.text.is_empty() { r.request_focus() } else { rr.request_focus() }
            }
        });
    }

    // --- links ------------------------------------------------------------------------

    pub(super) fn open_link_dialog(&mut self) {
        self.settle_edit();
        let c = self.cursor;
        let cell = self.wb.cell(self.sheet, c);
        let text = match cell.map(|x| &x.content) {
            None | Some(apex_sheet::Content::Empty) => Some(String::new()),
            Some(apex_sheet::Content::Value(apex_sheet::Value::Text(t))) => Some(t.to_string()),
            _ => None,
        };
        let link = cell.and_then(|x| x.link()).cloned();
        let mut d = LinkDlg { cell: c, text, kind: 0, url: String::new(), sheet: self.sheet, place: "A1".into(), email: String::new(), subject: String::new(), tip: String::new(), editing: link.is_some() };
        if let Some(l) = link {
            d.tip = l.tip.clone();
            if let Some(place) = l.place() {
                d.kind = 1;
                if let Some((s, r)) = parse_place(&self.wb, place, self.sheet) {
                    d.sheet = s;
                    d.place = if r.is_single() { r.start.to_a1() } else { r.to_a1() };
                }
            } else if let Some(m) = l.target.strip_prefix("mailto:") {
                d.kind = 2;
                let (addr, q) = m.split_once('?').unwrap_or((m, ""));
                d.email = addr.to_string();
                d.subject = q.strip_prefix("subject=").unwrap_or("").replace("%20", " ");
            } else {
                d.url = l.target.clone();
            }
        }
        self.notes.link = Some(d);
    }

    pub(super) fn remove_links(&mut self) {
        let r = self.sel();
        self.wb.clear_extras(self.sheet, &r, true);
    }

    pub(super) fn link_at(&self, c: CellRef) -> Option<&Link> {
        self.wb.cell(self.sheet, c)?.link()
    }

    /// Opens a link: jumps to a place in the workbook or hands the target
    /// to the system.
    pub(super) fn follow_link(&mut self, c: CellRef) {
        let Some(l) = self.link_at(c).cloned() else { return };
        match l.place() {
            Some(place) => match parse_place(&self.wb, place, self.sheet) {
                Some((s, r)) => {
                    if s != self.sheet {
                        self.select_sheet(s);
                    }
                    self.anchor = r.start;
                    self.select(r.end, true);
                    self.cursor = r.start;
                    self.anchor = r.end;
                }
                None => self.notices.push(Notice::error(tr("link.bad_place"))),
            },
            None => {
                if let Err(e) = crate::links::follow(&l.target) {
                    self.notices.push(Notice::error(e));
                }
            }
        }
    }

    /// Ctrl+click on a linked cell follows it.
    pub(super) fn notes_click(&mut self, c: CellRef, mods: egui::Modifiers) -> bool {
        if mods.command && self.link_at(c).is_some() {
            self.follow_link(c);
            return true;
        }
        false
    }

    fn link_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.notes.link.take() else { return };
        let names: Vec<String> = self.wb.sheets.iter().map(|s| s.name.clone()).collect();
        let title = if d.editing { tr("link.edit_title") } else { tr("link.insert_title") };
        let mut remove = false;
        let res = widgets::dialog(ctx, "link", title, 440.0, |ui| {
            egui::Grid::new("link_grid").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                ui.label(tr("link.text"));
                match &mut d.text {
                    Some(t) => {
                        ui.add(egui::TextEdit::singleline(t).desired_width(300.0));
                    }
                    None => {
                        ui.label(RichText::new(tr("link.text_formula")).weak());
                    }
                }
                ui.end_row();
                ui.label(tr("link.to"));
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut d.kind, 0, tr("link.kind_web"));
                    ui.selectable_value(&mut d.kind, 1, tr("link.kind_place"));
                    ui.selectable_value(&mut d.kind, 2, tr("link.kind_email"));
                });
                ui.end_row();
                match d.kind {
                    0 => {
                        ui.label(tr("link.address"));
                        let r = ui.add(egui::TextEdit::singleline(&mut d.url).hint_text("https://").desired_width(300.0));
                        if !r.has_focus() && ui.memory(|m| m.focused().is_none()) {
                            r.request_focus();
                        }
                        ui.end_row();
                    }
                    1 => {
                        ui.label(tr("link.sheet"));
                        egui::ComboBox::from_id_salt("link_sheet").width(300.0).selected_text(names.get(d.sheet).cloned().unwrap_or_default()).show_ui(ui, |ui| {
                            for (i, n) in names.iter().enumerate() {
                                ui.selectable_value(&mut d.sheet, i, n);
                            }
                        });
                        ui.end_row();
                        ui.label(tr("link.cell_ref"));
                        ui.add(egui::TextEdit::singleline(&mut d.place).desired_width(120.0));
                        ui.end_row();
                    }
                    _ => {
                        ui.label(tr("link.email"));
                        ui.add(egui::TextEdit::singleline(&mut d.email).desired_width(300.0));
                        ui.end_row();
                        ui.label(tr("link.subject"));
                        ui.add(egui::TextEdit::singleline(&mut d.subject).desired_width(300.0));
                        ui.end_row();
                    }
                }
                ui.label(tr("link.tip"));
                ui.add(egui::TextEdit::singleline(&mut d.tip).desired_width(300.0));
                ui.end_row();
            });
            if d.editing {
                ui.add_space(6.0);
                if ui.button(tr("link.remove")).clicked() {
                    remove = true;
                }
            }
        });
        if remove {
            self.wb.set_link(self.sheet, d.cell, None);
            self.focus_grid = true;
            return;
        }
        match res {
            DialogResult::None => self.notes.link = Some(d),
            DialogResult::Cancel => self.focus_grid = true,
            DialogResult::Ok => {
                self.focus_grid = true;
                self.apply_link(d);
            }
        }
    }

    fn apply_link(&mut self, d: LinkDlg) {
        let target = match d.kind {
            0 => {
                let u = d.url.trim();
                if u.is_empty() {
                    String::new()
                } else if u.starts_with("www.") {
                    format!("http://{u}")
                } else {
                    u.to_string()
                }
            }
            1 => {
                let p = d.place.trim().replace('$', "");
                if Range::parse(&p).is_none() && CellRef::parse(&p).is_none() {
                    self.notices.push(Notice::error(tr("link.bad_place")));
                    return;
                }
                format!("#{}!{}", quote_sheet(&self.wb.sheets[d.sheet.min(self.wb.sheets.len() - 1)].name), p.to_uppercase())
            }
            _ => {
                let e = d.email.trim().trim_start_matches("mailto:");
                if e.is_empty() {
                    String::new()
                } else if d.subject.trim().is_empty() {
                    format!("mailto:{e}")
                } else {
                    format!("mailto:{e}?subject={}", d.subject.trim().replace(' ', "%20"))
                }
            }
        };
        let c = d.cell;
        if target.is_empty() {
            if d.editing {
                self.wb.set_link(self.sheet, c, None);
            }
            return;
        }
        let before = self.wb.revision;
        let mut steps = 0;
        if let Some(t) = d.text {
            let shown = if t.trim().is_empty() { target.trim_start_matches('#').trim_start_matches("mailto:").to_string() } else { t };
            if self.wb.edit_text(self.sheet, c) != shown {
                // Text starting with "=" stays text.
                let input = if shown.starts_with(['=', '+', '-']) { format!("'{shown}") } else { shown };
                self.wb.set_input(self.sheet, c, &input);
                steps += 1;
            }
        }
        self.wb.set_link(self.sheet, c, Some(Link { target, tip: d.tip.trim().to_string() }));
        steps += 1;
        if !d.editing {
            let st = self.wb.styles.get(self.wb.cell(self.sheet, c).map_or(0, |x| x.style)).clone();
            if st.color.is_none() && !st.underline {
                self.wb.modify_style(self.sheet, &Range::single(c), |s| {
                    s.color = Some(LINK_COLOR);
                    s.underline = true;
                });
                steps += 1;
            }
        }
        if self.wb.revision != before && steps > 1 && !self.wb.take_blocked() {
            self.wb.merge_undo(steps, "undo.link");
        }
    }

    // --- protection -------------------------------------------------------------------

    /// Protects the sheet (or with `None` the workbook), or starts
    /// unprotecting it when it already is.
    pub(super) fn protect_cmd(&mut self, sheet: Option<usize>) {
        self.settle_edit();
        let current = match sheet {
            Some(i) => self.wb.sheets[i].protection.as_ref(),
            None => self.wb.protection.as_ref(),
        };
        match current {
            Some(p) if p.has_password() => self.notes.unprotect = Some((sheet, String::new())),
            Some(_) => self.wb.set_protection(sheet, None),
            None => self.notes.protect = Some(ProtectDlg { sheet, pw: String::new(), confirm: String::new(), allow: allow::DEFAULT, mismatch: false }),
        }
    }

    fn protect_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.notes.protect.take() else { return };
        let title = if d.sheet.is_some() { tr("protect.sheet_title") } else { tr("protect.workbook_title") };
        let res = widgets::dialog(ctx, "protect", title, 380.0, |ui| {
            egui::Grid::new("protect_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label(tr("protect.password"));
                let r = ui.add(egui::TextEdit::singleline(&mut d.pw).password(true).desired_width(200.0));
                if ui.memory(|m| m.focused().is_none()) {
                    r.request_focus();
                }
                ui.end_row();
                ui.label(tr("protect.confirm"));
                ui.add(egui::TextEdit::singleline(&mut d.confirm).password(true).desired_width(200.0));
                ui.end_row();
            });
            if d.mismatch {
                ui.colored_label(ui.visuals().error_fg_color, tr("protect.mismatch"));
            }
            ui.label(RichText::new(tr("protect.password_hint")).weak().small());
            if d.sheet.is_some() {
                ui.add_space(6.0);
                ui.label(tr("protect.allow"));
                egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                    for (flag, key) in ALLOW_UI {
                        let mut on = d.allow & flag != 0;
                        if ui.checkbox(&mut on, tr(key)).changed() {
                            d.allow ^= flag;
                        }
                    }
                });
            }
        });
        match res {
            DialogResult::None => self.notes.protect = Some(d),
            DialogResult::Cancel => self.focus_grid = true,
            DialogResult::Ok if d.pw != d.confirm => {
                d.mismatch = true;
                d.confirm.clear();
                self.notes.protect = Some(d);
            }
            DialogResult::Ok => {
                self.focus_grid = true;
                self.wb.set_protection(d.sheet, Some(Protection::with_password(&d.pw, if d.sheet.is_some() { d.allow } else { 0 })));
            }
        }
    }

    fn unprotect_dialog(&mut self, ctx: &egui::Context) {
        let Some((sheet, mut pw)) = self.notes.unprotect.take() else { return };
        let title = if sheet.is_some() { tr("protect.unprotect_sheet") } else { tr("protect.unprotect_workbook") };
        let res = widgets::dialog(ctx, "unprotect", title, 300.0, |ui| {
            ui.label(tr("protect.password"));
            let r = ui.add(egui::TextEdit::singleline(&mut pw).password(true));
            if ui.memory(|m| m.focused().is_none()) {
                r.request_focus();
            }
        });
        match res {
            DialogResult::None => self.notes.unprotect = Some((sheet, pw)),
            DialogResult::Cancel => self.focus_grid = true,
            DialogResult::Ok => {
                self.focus_grid = true;
                let p = match sheet {
                    Some(i) => self.wb.sheets[i].protection.as_ref(),
                    None => self.wb.protection.as_ref(),
                };
                if p.is_none_or(|p| p.unlocks(&pw)) {
                    self.wb.set_protection(sheet, None);
                } else {
                    self.notices.push(Notice::error(tr("protect.wrong_password")));
                }
            }
        }
    }

    /// Refuses to start editing a locked cell (asking for the password
    /// of an edit range that has one).
    pub(super) fn guard_edit(&mut self) -> bool {
        if self.wb.read_only {
            self.notices.push(Notice::info(tr("lock.blocked")));
            return false;
        }
        if self.wb.cell_locked(self.sheet, self.cursor) {
            if !self.ask_range_password(self.cursor) {
                self.notices.push(Notice::error(tr("protect.blocked")));
            }
            return false;
        }
        true
    }

    // --- symbols ----------------------------------------------------------------------

    pub(super) fn open_symbols(&mut self) {
        self.notes.symbols.open();
    }

    fn symbols_dialog(&mut self, ctx: &egui::Context) {
        let Some(s) = self.notes.symbols.show(ctx) else { return };
        if self.edit.is_some() {
            self.buf.push_str(&s);
            self.caret_to_end = true;
            self.refocus = true;
        } else if self.guard_edit() {
            let text = self.wb.edit_text(self.sheet, self.cursor) + &s;
            self.begin_edit(ctx, EditOrigin::Cell, false, Some(text));
        }
    }

    /// The view's own keys wait while one of these is open.
    pub(super) fn notes_busy(&self) -> bool {
        self.notes.edit.is_some() || self.notes.symbols.is_open()
    }

    pub(super) fn notes_popups(&mut self, ctx: &egui::Context) {
        self.note_editor(ctx);
        self.link_dialog(ctx);
        self.protect_dialog(ctx);
        self.unprotect_dialog(ctx);
        self.symbols_dialog(ctx);
        if self.wb.take_blocked() && !self.ask_range_password(self.cursor) {
            self.notices.push(Notice::error(tr("protect.blocked")));
        }
    }

    // --- painting ---------------------------------------------------------------------

    /// The cells with a note among those shown, and their notes.
    fn shown_notes(&self, g: &Geo) -> Vec<(CellRef, &Note)> {
        let sh = self.sh();
        let cols = g.scroll_cols(sh);
        let mut areas: Vec<Range> = super::runs(&g.shown_rows(sh)).into_iter().map(|(a, b)| Range::new(CellRef::new(a, 0), CellRef::new(b, *cols.end()))).collect();
        if g.fr > 0 {
            areas.push(Range::new(CellRef::new(g.top.row, 0), CellRef::new(g.fr - 1, *cols.end())));
        }
        let mut cells: Vec<CellRef> = Vec::new();
        for a in &areas {
            sh.for_each_in(a, |c, cell| {
                if cell.note().is_some() {
                    cells.push(c);
                }
            });
        }
        cells.into_iter().filter_map(|c| Some((c, sh.get(c)?.note()?))).collect()
    }

    /// The red corners of the cells with a note: marks of the cells, under
    /// the pictures and charts.
    pub(super) fn paint_note_marks(&self, painter: &egui::Painter, g: &Geo) {
        let p = painter.with_clip_rect(g.view);
        let sh = self.sh();
        for (c, n) in self.shown_notes(g) {
            let r = g.cell_rect(sh, c);
            if r.width() > 4.0 && r.height() > 4.0 {
                let s = (6.0 * g.z).clamp(4.0, 9.0);
                let color = if n.threaded { THREAD_MARK } else { MARK };
                p.add(Shape::convex_polygon(vec![r.right_top(), r.right_top() + vec2(0.0, s), r.right_top() - vec2(s, 0.0)], color, Stroke::NONE));
            }
        }
    }

    /// The notes shown all the time and the one under the pointer (over
    /// everything, pictures and charts too).
    pub(super) fn paint_notes(&self, painter: &egui::Painter, g: &Geo) {
        let p = painter.with_clip_rect(g.view);
        let sh = self.sh();
        let noted = self.shown_notes(g);
        let editing = self.notes.edit.as_ref().filter(|e| e.sheet == self.sheet).map(|e| e.cell);
        for &(c, n) in noted.iter().filter(|(c, n)| n.visible && Some(*c) != editing) {
            self.paint_note_box(&p, g, c, n);
        }
        // Hovering a cell shows its note or where its link goes.
        let Some(pos) = p.ctx().pointer_hover_pos().filter(|pp| g.view.contains(*pp) && pp.x > g.origin.x && pp.y > g.origin.y) else { return };
        if p.ctx().layer_id_at(pos).is_some_and(|l| l != p.layer_id()) || self.drag != super::Drag::None {
            return;
        }
        let c = CellRef::new(g.row_at(sh, pos.y), g.col_at(sh, pos.x));
        if let Some(n) = sh.get(c).and_then(|x| x.note()).filter(|n| !n.visible && Some(c) != editing) {
            self.paint_note_box(&p, g, c, n);
        } else if let Some(l) = sh.get(c).and_then(|x| x.link()) {
            // The document's tip, then where the link really goes.
            let to = l.target.trim_start_matches('#');
            let tip = l.tip.split_whitespace().collect::<Vec<_>>().join(" ");
            let text = if tip.is_empty() || tip == to { format!("{to}\n{}", tr("link.follow_hint")) } else { format!("{tip}\n{to}\n{}", tr("link.follow_hint")) };
            let galley = p.layout(text, FontId::proportional(12.0), Color32::BLACK, 320.0);
            let rect = Rect::from_min_size(pos + vec2(12.0, 18.0), galley.size() + vec2(10.0, 8.0));
            p.rect_filled(rect, 3.0, Color32::from_rgb(0xFA, 0xFA, 0xFA));
            p.rect_stroke(rect, 3.0, Stroke::new(1.0, Color32::from_gray(150)), StrokeKind::Inside);
            p.galley(rect.min + vec2(5.0, 4.0), galley, Color32::BLACK);
        }
    }

    fn paint_note_box(&self, p: &egui::Painter, g: &Geo, c: CellRef, n: &Note) {
        if n.threaded {
            return self.paint_thread_box(p, g, c, n);
        }
        let r = g.cell_rect(self.sh(), c);
        let mut job = egui::text::LayoutJob::default();
        if !n.author.is_empty() {
            job.append(&format!("{}:\n", n.author), 0.0, egui::TextFormat { font_id: crate::fonts::font(crate::fonts::Face::Sans, true, 12.0), color: Color32::BLACK, ..Default::default() });
        }
        job.append(&n.text, 0.0, egui::TextFormat { font_id: FontId::proportional(12.0), color: Color32::BLACK, ..Default::default() });
        job.wrap.max_width = NOTE_W;
        let galley = p.layout_job(job);
        let at = pos2(r.max.x + 14.0, r.min.y - 2.0);
        let rect = Rect::from_min_size(at, vec2(NOTE_W + 12.0, galley.size().y + 10.0));
        p.line_segment([r.right_top(), rect.left_top() + vec2(0.0, 6.0)], Stroke::new(1.0, Color32::BLACK));
        p.rect_filled(rect.translate(vec2(2.0, 2.0)), 0.0, Color32::from_black_alpha(40));
        p.rect_filled(rect, 0.0, NOTE_BG);
        p.rect_stroke(rect, 0.0, Stroke::new(1.0, NOTE_BORDER), StrokeKind::Inside);
        p.galley(rect.min + vec2(6.0, 5.0), galley, Color32::BLACK);
    }

    /// A threaded comment: each message with its author and date.
    fn paint_thread_box(&self, p: &egui::Painter, g: &Geo, c: CellRef, n: &Note) {
        let r = g.cell_rect(self.sh(), c);
        let bold = egui::TextFormat { font_id: crate::fonts::font(crate::fonts::Face::Sans, true, 12.0), color: Color32::BLACK, ..Default::default() };
        let small = egui::TextFormat { font_id: FontId::proportional(10.5), color: Color32::from_gray(110), ..Default::default() };
        let plain = egui::TextFormat { font_id: FontId::proportional(12.0), color: Color32::BLACK, ..Default::default() };
        let mut job = egui::text::LayoutJob::default();
        let msgs = std::iter::once((&n.author, &n.date, &n.text)).chain(n.replies.iter().map(|x| (&x.author, &x.date, &x.text)));
        for (i, (author, date, text)) in msgs.enumerate() {
            if i > 0 {
                job.append("\n\n", 0.0, small.clone());
            }
            job.append(author, 0.0, bold.clone());
            if !date.is_empty() {
                job.append(&format!("  {}", short_date(date)), 0.0, small.clone());
            }
            job.append(&format!("\n{text}"), 0.0, plain.clone());
        }
        if n.resolved {
            job.append(&format!("\n\n{}", tr("comment.resolved")), 0.0, small);
        }
        job.wrap.max_width = THREAD_W;
        let galley = p.layout_job(job);
        let at = pos2(r.max.x + 14.0, r.min.y - 2.0);
        let rect = Rect::from_min_size(at, vec2(THREAD_W + 16.0, galley.size().y + 14.0));
        p.rect_filled(rect.translate(vec2(2.0, 2.0)), 4.0, Color32::from_black_alpha(40));
        p.rect_filled(rect, 4.0, THREAD_BG);
        p.rect_stroke(rect, 4.0, Stroke::new(1.0, Color32::from_gray(170)), StrokeKind::Inside);
        p.rect_filled(Rect::from_min_size(rect.min, vec2(3.0, rect.height())), 0.0, THREAD_MARK);
        p.galley(rect.min + vec2(9.0, 7.0), galley, Color32::BLACK);
    }
}
