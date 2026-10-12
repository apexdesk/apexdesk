//! Restrict Editing: the dialog that starts and stops the protection, the
//! bar shown while it applies, and the guard that takes back edits it
//! doesn't allow (whatever command made them).

use apex_doc::{EditKind, EditLimit, Protection, ProtectionHash};

use super::*;
use crate::widgets::DialogResult;

const LIMITS: [(EditLimit, &str); 4] = [
    (EditLimit::ReadOnly, "protect.read_only"),
    (EditLimit::Comments, "protect.comments"),
    (EditLimit::TrackedChanges, "protect.tracked"),
    (EditLimit::Forms, "protect.forms"),
];

fn limit_label(l: EditLimit) -> &'static str {
    LIMITS.iter().find(|(x, _)| *x == l).map_or("", |(_, k)| tr(k))
}

pub struct RestrictDlg {
    limit: EditLimit,
    pw: String,
    confirm: String,
    error: Option<&'static str>,
}

#[derive(Default)]
pub struct ProtectState {
    /// Undo steps the protection has seen (and allowed).
    pub(super) guard: usize,
    pub dlg: Option<RestrictDlg>,
    /// When the last "protected" notice showed (one every few seconds).
    noticed: Option<std::time::Instant>,
    /// The editable ranges while read-only protection applies, and the
    /// revision they were found at.
    pub(super) ranges: Vec<(Pos, Pos)>,
    ranges_rev: Option<u64>,
}

/// The shading of editable ranges (Word's light yellow).
const RANGE_SHADE: Color32 = Color32::from_rgba_unmultiplied_const(255, 240, 110, 110);

/// Whether a protected document allows an edit of this kind.
fn allowed(limit: EditLimit, kind: EditKind) -> bool {
    match limit {
        EditLimit::Comments => kind == EditKind::Comment,
        // Accepting or rejecting changes would untrack them.
        EditLimit::TrackedChanges => kind != EditKind::Revision,
        EditLimit::ReadOnly | EditLimit::Forms => false,
    }
}

/// Whether `pw` stops the protection. Older Word hashes can't be checked
/// here, so any password does.
fn password_ok(p: &Protection, pw: &str) -> bool {
    match &p.password {
        Some(h) if !h.algorithm.is_empty() => {
            let stored = apex_sheet::Password::Salted { algorithm: h.algorithm.clone(), hash: h.hash.clone(), salt: h.salt.clone(), spin: h.spin };
            stored.check(pw).unwrap_or(true)
        }
        _ => true,
    }
}

impl DocView {
    pub(super) fn open_restrict_dialog(&mut self) {
        let limit = self.doc.protection.as_ref().map_or(EditLimit::ReadOnly, |p| p.limit);
        self.protect.dlg = Some(RestrictDlg { limit, pw: String::new(), confirm: String::new(), error: None });
    }

    /// Finds the editable ranges again if the document changed.
    fn refresh_ranges(&mut self) {
        let rev = Some(self.doc.revision).filter(|_| self.doc.edit_limit() == Some(EditLimit::ReadOnly));
        if self.protect.ranges_rev != rev {
            self.protect.ranges = if rev.is_some() { self.doc.editable_ranges() } else { Vec::new() };
            self.protect.ranges_rev = rev;
        }
    }

    /// Takes back edits the protection doesn't allow; call once a frame.
    pub(super) fn enforce_protection(&mut self) {
        self.refresh_ranges();
        // Read mode shows the document without changing it.
        let read = self.views.mode == ViewMode::Read;
        let Some(limit) = (if read || self.read_only { Some(EditLimit::ReadOnly) } else { self.doc.edit_limit() }) else {
            self.protect.guard = self.doc.undo_len();
            self.forms_unprotected();
            return;
        };
        // Filling in forms, or read-only with editable ranges: whatever the
        // fields or the ranges don't hold must stay.
        let ranges = limit == EditLimit::ReadOnly && !read && !self.read_only && !self.protect.ranges.is_empty();
        if limit == EditLimit::Forms || ranges {
            let blocked = self.enforce_forms(if ranges { Document::locked_skeleton } else { Document::form_skeleton });
            self.doc.seal_undo();
            self.refresh_ranges();
            if blocked && self.protect.noticed.is_none_or(|t| t.elapsed().as_secs() >= 4) {
                self.protect.noticed = Some(std::time::Instant::now());
                self.notices.push(Notice::info(trf("protect.doc_blocked", &[("limit", &limit_label(limit))])));
            }
            return;
        }
        self.forms_unprotected();
        if limit == EditLimit::TrackedChanges {
            self.doc.track_changes = true;
        }
        let mut blocked = false;
        // Undo past the protection's start: redo what it may not undo.
        while self.doc.undo_len() < self.protect.guard {
            match self.doc.redo_kind() {
                Some(k) if !allowed(limit, k) => {
                    if self.doc.redo().is_none() {
                        break;
                    }
                    blocked = true;
                }
                _ => {
                    self.protect.guard = self.doc.undo_len();
                }
            }
        }
        let n = self.doc.undo_len();
        if n > self.protect.guard {
            if (self.protect.guard..n).all(|i| self.doc.undo_kind(i).is_some_and(|k| allowed(limit, k))) {
                self.protect.guard = n;
            } else {
                let mut sel = None;
                for _ in self.protect.guard..n {
                    sel = self.doc.undo().or(sel);
                }
                if let Some(s) = sel {
                    self.sel = Selection::caret(self.doc.clamp(s.cursor));
                }
                blocked = true;
            }
        }
        // Typing never merges into a step from before.
        self.doc.seal_undo();
        // Nothing of a read-only file changes: nothing to save.
        if self.read_only && blocked {
            self.doc.modified = false;
        }
        if blocked && !read && self.protect.noticed.is_none_or(|t| t.elapsed().as_secs() >= 4) {
            self.protect.noticed = Some(std::time::Instant::now());
            let text = if self.read_only { tr("lock.blocked").to_string() } else { trf("protect.doc_blocked", &[("limit", &limit_label(limit))]) };
            self.notices.push(Notice::info(text));
        }
    }

    /// The bar above the page while the document is protected.
    pub(super) fn protect_bar(&mut self, ui: &mut egui::Ui) {
        let Some(limit) = self.doc.edit_limit() else { return };
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
            crate::icons::paint(ui.painter(), r, Icon::Protect, ui.visuals().text_color());
            ui.label(trf("protect.bar", &[("limit", &limit_label(limit))]));
            if !self.protect.ranges.is_empty() && ui.button(tr("protect.next_region")).clicked() {
                self.next_editable_range();
            }
            if ui.button(format!("{}…", tr("protect.stop"))).clicked() {
                self.open_restrict_dialog();
            }
        });
    }

    /// Selects the editable range after the selection (the first one after
    /// the last).
    fn next_editable_range(&mut self) {
        let at = self.sel.end();
        let r = &self.protect.ranges;
        let Some(&(a, b)) = r.iter().find(|(a, _)| *a >= at).or(r.first()) else { return };
        self.set_sel(Selection { anchor: a, cursor: b });
        self.scroll_to_caret = true;
        self.focus = true;
    }

    /// The shading of the editable ranges in a line segment.
    pub(super) fn paint_editable_ranges(&self, p: &egui::Painter, seg: &Segment, pl: &ParaLayout, galley_pos: Pos2) {
        if self.printing.get() {
            return;
        }
        let text = &self.doc.paragraphs[seg.para].text;
        for &(a, b) in self.protect.ranges.iter().filter(|(a, b)| a.para <= seg.para && seg.para <= b.para) {
            let from = if a.para == seg.para { char_count(text, a.offset) } else { 0 };
            let to = if b.para == seg.para { char_count(text, b.offset) } else { usize::MAX };
            self.paint_range(p, seg, pl, from, to, galley_pos, RANGE_SHADE);
        }
    }

    /// A small bracket at each end of the editable ranges.
    pub(super) fn paint_range_brackets(&self, p: &egui::Painter, off: Vec2) {
        if self.printing.get() {
            return;
        }
        let stroke = Stroke::new(self.zoom.max(1.0), Color32::from_gray(120));
        for &(a, b) in &self.protect.ranges {
            for (at, end) in [(a, false), (b, true)] {
                let r = self.caret_rect(self.doc.clamp(at)).translate(off);
                let (x, w) = (r.min.x.round() + 0.5, 3.0 * self.zoom.max(1.0));
                let dx = if end { -w } else { w };
                p.vline(x, r.y_range(), stroke);
                p.hline(x..=x + dx, r.min.y, stroke);
                p.hline(x..=x + dx, r.max.y, stroke);
            }
        }
    }

    /// Exceptions: parts of a read-only document anyone may still edit
    /// (with a business license; files that have them work for everyone).
    fn exceptions_section(&mut self, ui: &mut egui::Ui, limit: EditLimit) {
        let licensed = crate::license::has("management");
        let n = self.doc.editable_ranges().len();
        ui.add_enabled_ui(licensed && limit == EditLimit::ReadOnly, |ui| {
            ui.strong(tr("protect.exceptions"));
            ui.label(trf("protect.regions", &[("n", &n)]));
            ui.horizontal(|ui| {
                if ui.add_enabled(!self.sel.is_empty(), egui::Button::new(tr("protect.make_editable"))).clicked() {
                    self.doc.add_editable_range(self.sel);
                }
                if ui.add_enabled(n > 0, egui::Button::new(tr("protect.remove_regions"))).clicked() {
                    self.doc.remove_editable_ranges(self.sel);
                }
            });
        });
        if !licensed {
            ui.label(RichText::new(tr("protect.exceptions_business")).small().weak());
        }
    }

    pub(super) fn restrict_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.protect.dlg.take() else { return };
        let enforced = self.doc.edit_limit().is_some();
        let title = tr("protect.title");
        let res = widgets::dialog(ctx, "restrict_editing", title, 380.0, |ui| {
            if enforced {
                ui.label(trf("protect.bar", &[("limit", &limit_label(d.limit))]));
                ui.add_space(4.0);
                ui.label(tr("protect.stop_intro"));
                ui.add(egui::TextEdit::singleline(&mut d.pw).password(true).desired_width(f32::INFINITY));
            } else {
                ui.label(tr("protect.intro"));
                ui.add_space(4.0);
                for (l, key) in LIMITS {
                    ui.radio_value(&mut d.limit, l, tr(key));
                }
                ui.add_space(6.0);
                ui.label(tr("protect.password_intro"));
                egui::Grid::new("restrict_pw").num_columns(2).show(ui, |ui| {
                    ui.label(tr("password.password"));
                    ui.add(egui::TextEdit::singleline(&mut d.pw).password(true).desired_width(200.0));
                    ui.end_row();
                    ui.label(tr("password.confirm"));
                    ui.add(egui::TextEdit::singleline(&mut d.confirm).password(true).desired_width(200.0));
                    ui.end_row();
                });
                ui.add_space(6.0);
                self.exceptions_section(ui, d.limit);
            }
            if let Some(e) = d.error {
                ui.label(RichText::new(tr(e)).color(ui.visuals().error_fg_color));
            }
        });
        match res {
            DialogResult::None => self.protect.dlg = Some(d),
            DialogResult::Cancel => self.refocus = true,
            DialogResult::Ok if enforced => {
                let ok = self.doc.protection.as_ref().is_none_or(|p| password_ok(p, &d.pw));
                if ok {
                    if let Some(p) = &mut self.doc.protection {
                        p.enforced = false;
                    }
                    self.doc.modified = true;
                    self.refocus = true;
                } else {
                    d.error = Some("password.wrong");
                    d.pw.clear();
                    self.protect.dlg = Some(d);
                }
            }
            DialogResult::Ok => {
                if d.pw != d.confirm {
                    d.error = Some("password.mismatch");
                    self.protect.dlg = Some(d);
                    return;
                }
                let password = (!d.pw.is_empty()).then(|| match apex_sheet::Password::new(&d.pw).into_iter().next() {
                    Some(apex_sheet::Password::Salted { algorithm, hash, salt, spin }) => ProtectionHash { algorithm, hash, salt, spin, legacy: Vec::new() },
                    _ => ProtectionHash { algorithm: String::new(), hash: String::new(), salt: String::new(), spin: 0, legacy: Vec::new() },
                });
                self.doc.protection = Some(Protection { limit: d.limit, enforced: true, password });
                self.protect.guard = self.doc.undo_len();
                self.doc.modified = true;
                self.refocus = true;
            }
        }
    }
}
