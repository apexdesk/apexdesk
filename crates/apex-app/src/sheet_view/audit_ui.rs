//! Formula auditing: trace arrows, the spill range border, error indicators
//! with their action menu (the review's warnings among them: a formula
//! out of its column's pattern, a number typed among formulas, a range
//! that stops short, a total that doesn't add up), and the error checking
//! dialog.

use std::cell::RefCell;
use std::collections::HashSet;

use apex_sheet::{CellRef, Content, Range, Review, Value};
use eframe::egui::{self, Color32, Id, Pos2, Rect, RichText, Shape, Stroke, StrokeKind, pos2, vec2};

use super::{EditOrigin, Geo, SheetView};
use crate::i18n::tr;
use crate::icons::Icon;

const ARROW: Color32 = Color32::from_rgb(0x1E, 0x5A, 0xC8);
const ARROW_ERR: Color32 = Color32::from_rgb(0xD0, 0x20, 0x20);
const FLAG: Color32 = Color32::from_rgb(0x10, 0x8A, 0x3C);
/// Precedent ranges larger than this are not followed to the next level.
const MAX_FOLLOW: usize = 20_000;

#[derive(Default)]
pub(super) struct AuditState {
    arrows: Vec<Arrow>,
    /// Workbook revision the arrows were traced at (edits clear them).
    rev: u64,
    /// How many levels each traced cell has, per direction.
    levels: Vec<((usize, CellRef, bool), u32)>,
    ignored: HashSet<(usize, CellRef)>,
    /// Kinds of warning ignored everywhere ("Ignore All", as in spelling).
    ignored_kinds: HashSet<&'static str>,
    /// The action menu of the error button is open.
    menu: bool,
    /// The error checking dialog is open.
    pub checker: bool,
    pub watch: super::watch_ui::WatchState,
    flags: RefCell<Option<(FlagKey, Vec<(CellRef, Issue)>)>>,
    pub tests: super::tests_ui::TestsState,
    pub origin: super::origin_ui::OriginState,
}

type FlagKey = (usize, u64, usize, usize, [Range; 2]);

#[derive(Clone, Copy, PartialEq, Debug)]
enum End {
    Area(Range),
    /// A cell on another sheet.
    Other,
}

#[derive(Clone, Copy, PartialEq, Debug)]
struct Arrow {
    sheet: usize,
    from: End,
    to: End,
    error: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Issue {
    ErrorValue,
    NumberAsText,
    Inconsistent,
    /// A number typed among cells with the same formula.
    Constant,
    /// The formula's range `from` leaves out numbers next to it (`to`
    /// takes them in).
    ShortRange { from: Range, to: Range },
    /// A total that isn't the sum of the numbers above it (`range`).
    BadTotal { sum: f64, range: Range },
}

impl Issue {
    fn key(self) -> &'static str {
        match self {
            Issue::ErrorValue => "audit.err_value",
            Issue::NumberAsText => "audit.err_text",
            Issue::Inconsistent => "audit.err_inconsistent",
            Issue::Constant => "audit.err_constant",
            Issue::ShortRange { .. } => "audit.err_short",
            Issue::BadTotal { .. } => "audit.err_total",
        }
    }

    /// The warning's text, with its details.
    fn text(self, wb: &apex_sheet::Workbook) -> String {
        match self {
            Issue::ShortRange { from, to } => crate::i18n::trf("audit.err_short", &[("a", &from.to_a1()), ("b", &to.to_a1())]),
            Issue::BadTotal { sum, .. } => crate::i18n::trf("audit.err_total", &[("v", &wb.format_value(0, &Value::Number(sum)).0)]),
            _ => tr(self.key()).to_string(),
        }
    }
}

fn looks_numeric(s: &str, decimal: char) -> bool {
    let t = s.trim();
    t.chars().any(|c| c.is_ascii_digit())
        && t.chars().all(|c| c.is_ascii_digit() || c == decimal || matches!(c, '+' | '-' | 'e' | 'E'))
        && t.replace(decimal, ".").parse::<f64>().is_ok()
}

impl SheetView {
    // --- analysis -------------------------------------------------------------

    /// What error checking reports for `c`, if anything.
    pub(super) fn issue_at(&self, c: CellRef) -> Option<Issue> {
        if self.audit.ignored.contains(&(self.sheet, c)) {
            return None;
        }
        let cell = self.sh().get(c)?;
        let issue = match &cell.content {
            Content::Formula { value: Value::Error(_), .. } => Some(Issue::ErrorValue),
            Content::Value(Value::Text(s)) if looks_numeric(s, self.wb.locale.decimal) => Some(Issue::NumberAsText),
            _ => self.wb.review(self.sheet, c).map(|r| match r {
                Review::Inconsistent => Issue::Inconsistent,
                Review::Constant => Issue::Constant,
                Review::ShortRange(to) => {
                    let from = self.wb.precedents(self.sheet, c).into_iter().find(|(s, r)| *s == self.sheet && (r.start == to.start || r.end == to.end)).map_or(to, |(_, r)| r);
                    Issue::ShortRange { from, to }
                }
                Review::BadTotal(sum) => Issue::BadTotal { sum, range: self.block_above(c) },
            }),
        };
        issue.filter(|i| !self.audit.ignored_kinds.contains(i.key()))
    }

    /// The numbers right above `c`.
    fn block_above(&self, c: CellRef) -> Range {
        let mut top = c.row;
        while top > 0 && matches!(self.sh().value(CellRef::new(top - 1, c.col)), Value::Number(_)) {
            top -= 1;
        }
        Range::new(CellRef::new(top, c.col), CellRef::new(c.row.saturating_sub(1).max(top), c.col))
    }

    /// Flagged cells in the visible area (cached per revision and scroll).
    fn visible_issues(&self, g: &Geo) -> Vec<(CellRef, Issue)> {
        let sh = self.sh();
        let (rows, cols) = (g.scroll_rows(sh), g.scroll_cols(sh));
        let fr = if g.fr > 0 { Range::new(CellRef::new(g.top.row, 0), CellRef::new(g.fr - 1, *cols.end())) } else { Range::default() };
        let main = Range::new(CellRef::new(*rows.start(), 0), CellRef::new(*rows.end(), *cols.end()));
        let key = (self.sheet, self.wb.revision, self.audit.ignored.len(), self.audit.ignored_kinds.len(), [fr, main]);
        let mut cache = self.audit.flags.borrow_mut();
        if cache.as_ref().is_none_or(|(k, _)| *k != key) {
            let mut out = Vec::new();
            let mut cells = Vec::new();
            for (a, b) in super::runs(&g.shown_rows(sh)) {
                sh.for_each_in(&Range::new(CellRef::new(a, 0), CellRef::new(b, *cols.end())), |c, _| cells.push(c));
            }
            if g.fr > 0 {
                sh.for_each_in(&fr, |c, _| cells.push(c));
            }
            for c in cells {
                if let Some(i) = self.issue_at(c) {
                    out.push((c, i));
                }
            }
            *cache = Some((key, out));
        }
        cache.as_ref().unwrap().1.clone()
    }

    /// Every flagged cell of the sheet in reading order.
    fn all_issues(&self) -> Vec<CellRef> {
        let mut v: Vec<CellRef> = self.sh().iter().map(|(c, _)| c).filter(|c| self.issue_at(*c).is_some()).collect();
        v.sort_unstable_by_key(|c| (c.row, c.col));
        v
    }

    // --- tracing ---------------------------------------------------------------

    fn fresh_arrows(&mut self) {
        if self.audit.rev != self.wb.revision {
            self.audit.arrows.clear();
            self.audit.levels.clear();
            self.audit.rev = self.wb.revision;
        }
    }

    /// Adds one more level of precedent (or dependent) arrows for the cursor.
    pub(super) fn trace(&mut self, dependents: bool) {
        self.settle_edit();
        self.fresh_arrows();
        let key = (self.sheet, self.cursor, dependents);
        let level = match self.audit.levels.iter_mut().find(|(k, _)| *k == key) {
            Some((_, l)) => {
                *l += 1;
                *l
            }
            None => {
                self.audit.levels.push((key, 1));
                1
            }
        };
        let before = self.audit.arrows.len();
        let mut seen: HashSet<(usize, CellRef)> = HashSet::new();
        let mut frontier = vec![(self.sheet, self.cursor)];
        seen.insert((self.sheet, self.cursor));
        for _ in 0..level {
            let mut next = Vec::new();
            for (s, c) in frontier {
                if dependents {
                    for (ds, d) in self.wb.dependents(s, c) {
                        let to = if ds == s { End::Area(Range::single(d)) } else { End::Other };
                        self.add_arrow(Arrow { sheet: s, from: End::Area(Range::single(c)), to, error: false });
                        if seen.insert((ds, d)) {
                            next.push((ds, d));
                        }
                    }
                } else {
                    for (ps, r) in self.wb.precedents(s, c) {
                        let error = (0..r.rows().min(64))
                            .flat_map(|i| (0..r.cols().min(64)).map(move |j| CellRef::new(r.start.row + i, r.start.col + j)))
                            .any(|x| matches!(self.wb.sheets[ps].value(x), Value::Error(_)));
                        let from = if ps == s { End::Area(r) } else { End::Other };
                        self.add_arrow(Arrow { sheet: s, from, to: End::Area(Range::single(c)), error });
                        if (r.rows() as usize) * (r.cols() as usize) <= MAX_FOLLOW {
                            self.wb.sheets[ps].for_each_in(&r, |x, cell| {
                                if cell.formula().is_some() && seen.insert((ps, x)) {
                                    next.push((ps, x));
                                }
                            });
                        }
                    }
                }
            }
            frontier = next;
        }
        if self.audit.arrows.len() == before {
            // Nothing new: don't count the click as a level.
            if let Some((_, l)) = self.audit.levels.iter_mut().find(|(k, _)| *k == key) {
                *l = l.saturating_sub(1);
            }
            let msg = if dependents { "audit.no_dependents" } else { "audit.no_precedents" };
            if level == 1 {
                self.notices.push(crate::app::Notice::info(tr(msg)));
            }
        }
    }

    fn add_arrow(&mut self, a: Arrow) {
        if !self.audit.arrows.contains(&a) {
            self.audit.arrows.push(a);
        }
    }

    pub(super) fn remove_arrows(&mut self) {
        self.audit.arrows.clear();
        self.audit.levels.clear();
    }

    pub(super) fn has_arrows(&self) -> bool {
        !self.audit.arrows.is_empty() && self.audit.rev == self.wb.revision
    }

    /// Recalculates every formula (F9).
    pub(super) fn calculate_now(&mut self) {
        self.settle_edit();
        self.wb.recalc_all();
        self.wb.revision += 1;
    }

    // --- painting ----------------------------------------------------------------

    /// The error triangles: marks of the cells, under the pictures and
    /// charts.
    pub(super) fn paint_issue_flags(&self, painter: &egui::Painter, g: &Geo) {
        let p = painter.with_clip_rect(g.view);
        let sh = self.sh();
        for (c, _) in self.visible_issues(g) {
            let r = g.cell_rect(sh, c);
            if r.width() > 4.0 && r.height() > 4.0 {
                let s = (5.0 * g.z).clamp(3.0, 8.0);
                p.add(Shape::convex_polygon(vec![r.min, r.min + vec2(s, 0.0), r.min + vec2(0.0, s)], FLAG, Stroke::NONE));
            }
        }
    }

    /// Spill border, the error button and trace arrows.
    pub(super) fn paint_audit(&self, painter: &egui::Painter, g: &Geo) {
        let p = painter.with_clip_rect(g.view);
        let sh = self.sh();
        // The area spilled by the formula under the cursor.
        if let Some(a) = self.wb.spill_anchor(self.sheet, self.cursor).or(Some(self.cursor)) {
            if let Some(r) = self.wb.spill_range(self.sheet, a) {
                if r.rows() * r.cols() > 1 {
                    p.rect_stroke(g.range_rect(sh, &r), 0.0, Stroke::new(1.0, ARROW), StrokeKind::Inside);
                }
            }
        }
        if self.edit.is_none() {
            if let Some(r) = self.issue_button() {
                p.rect_filled(r, 2.0, Color32::from_rgb(0xFF, 0xF4, 0xCE));
                p.rect_stroke(r, 2.0, Stroke::new(1.0, Color32::from_rgb(0xC8, 0x9B, 0x2C)), StrokeKind::Inside);
                let c = r.center();
                let h = r.height() * 0.32;
                p.add(Shape::convex_polygon(vec![c + vec2(0.0, -h), c + vec2(h * 1.05, h * 0.8), c + vec2(-h * 1.05, h * 0.8)], Color32::from_rgb(0xE0, 0xA0, 0x10), Stroke::NONE));
                p.line_segment([c + vec2(0.0, -h * 0.35), c + vec2(0.0, h * 0.3)], Stroke::new(1.2, Color32::BLACK));
            }
        }
        if !self.has_arrows() {
            return;
        }
        for a in self.audit.arrows.iter().filter(|a| a.sheet == self.sheet) {
            let color = if a.error { ARROW_ERR } else { ARROW };
            let stroke = Stroke::new(1.3, color);
            let center = |r: &Range| g.cell_rect(sh, r.start).center();
            match (a.from, a.to) {
                (End::Area(f), End::Area(t)) => {
                    if f.rows() * f.cols() > 1 {
                        p.rect_stroke(g.range_rect(sh, &f), 0.0, stroke, StrokeKind::Inside);
                    }
                    let (s, e) = (center(&f), center(&t));
                    p.line_segment([s, e], stroke);
                    p.circle_filled(s, 2.5, color);
                    arrow_head(&p, s, e, color);
                }
                (End::Other, End::Area(t)) => {
                    let e = center(&t);
                    let s = e + vec2(-60.0, -40.0);
                    p.add(Shape::dashed_line(&[s, e], Stroke::new(1.3, Color32::BLACK), 5.0, 3.0));
                    arrow_head(&p, s, e, Color32::BLACK);
                    sheet_icon(&p, s);
                }
                (End::Area(f), End::Other) => {
                    let s = center(&f);
                    let e = s + vec2(60.0, -40.0);
                    p.add(Shape::dashed_line(&[s, e], Stroke::new(1.3, Color32::BLACK), 5.0, 3.0));
                    p.circle_filled(s, 2.5, Color32::BLACK);
                    sheet_icon(&p, e + vec2(8.0, -8.0));
                }
                _ => {}
            }
        }
    }

    /// The warning button next to a flagged cursor cell.
    fn issue_button(&self) -> Option<Rect> {
        let r = self.data.cursor_rect?;
        if !self.sel().is_single() {
            return None;
        }
        self.issue_at(self.cursor)?;
        let s = r.height().clamp(14.0, 20.0);
        Some(Rect::from_min_size(pos2(r.min.x - s - 2.0, r.min.y), vec2(s, s)))
    }

    /// Handles a click on the error button; true if it took it.
    pub(super) fn audit_click(&mut self, p: Pos2) -> bool {
        if self.issue_button().is_some_and(|r| r.contains(p)) {
            self.audit.menu = !self.audit.menu;
            return true;
        }
        false
    }

    // --- actions and popups ------------------------------------------------------------

    fn convert_to_number(&mut self, c: CellRef) {
        if let Value::Text(s) = self.sh().value(c).clone() {
            self.wb.set_input(self.sheet, c, s.trim());
        }
    }

    fn copy_from_above(&mut self, c: CellRef) {
        // The neighbour whose formula the cell should have: above, else left.
        let from = [(1i64, 0i64), (0, 1)].into_iter().find_map(|(dr, dc)| {
            let n = CellRef::new(c.row.checked_sub(dr as u32)?, c.col.checked_sub(dc as u32)?);
            self.sh().get(n).and_then(|x| x.formula()).map(|f| (f.expr().into_owned(), dr, dc))
        });
        let Some((mut e, dr, dc)) = from else { return };
        e.offset(dr, dc);
        let text = e.to_display(&self.wb.locale);
        self.wb.set_input(self.sheet, c, &text);
    }

    /// The first action of the warning at `c` (what fixes it).
    pub(super) fn review_fix(&mut self, c: CellRef) {
        match self.issue_at(c) {
            Some(Issue::NumberAsText) => self.convert_to_number(c),
            Some(Issue::Inconsistent | Issue::Constant) => self.copy_from_above(c),
            Some(Issue::ShortRange { from, to }) => self.extend_range(c, from, to),
            Some(Issue::BadTotal { range, .. }) => self.wb.set_input(self.sheet, c, &format!("=SUM({})", range.to_a1())),
            _ => {}
        }
    }

    /// Makes the formula's range take in the numbers it left out.
    fn extend_range(&mut self, c: CellRef, from: Range, to: Range) {
        let text = self.wb.edit_text(self.sheet, c);
        let (a, b) = (from.to_a1(), to.to_a1());
        let abs = |r: &Range| format!("${}${}:${}${}", apex_sheet::addr::col_name(r.start.col), r.start.row + 1, apex_sheet::addr::col_name(r.end.col), r.end.row + 1);
        let new = if text.contains(&a) { text.replacen(&a, &b, 1) } else { text.replacen(&abs(&from), &abs(&to), 1) };
        if new != text {
            self.wb.set_input(self.sheet, c, &new);
        }
    }

    /// Runs an action from the error menu or the checker on `c`.
    fn issue_action(&mut self, ctx: &egui::Context, c: CellRef, act: IssueAct) {
        match act {
            IssueAct::Convert | IssueAct::CopyAbove | IssueAct::Extend | IssueAct::SumAbove => self.review_fix(c),
            IssueAct::Ignore => {
                self.audit.ignored.insert((self.sheet, c));
            }
            IssueAct::IgnoreAll => {
                if let Some(i) = self.issue_at(c) {
                    self.audit.ignored_kinds.insert(i.key());
                }
            }
            IssueAct::Edit => {
                self.cursor = c;
                self.anchor = c;
                self.begin_edit(ctx, EditOrigin::Bar, false, None);
            }
            IssueAct::Trace => {
                self.cursor = c;
                self.anchor = c;
                self.trace(false);
            }
        }
        self.audit.menu = false;
    }

    fn issue_actions(&self, ui: &mut egui::Ui, issue: Issue) -> Option<IssueAct> {
        let mut act = None;
        let mut item = |ui: &mut egui::Ui, key: &'static str, a: IssueAct| {
            if ui.button(tr(key)).clicked() {
                act = Some(a);
            }
        };
        match issue {
            Issue::NumberAsText => item(ui, "audit.convert", IssueAct::Convert),
            Issue::Inconsistent => item(ui, "audit.copy_above", IssueAct::CopyAbove),
            Issue::Constant => item(ui, "audit.use_formula", IssueAct::CopyAbove),
            Issue::ShortRange { .. } => item(ui, "audit.extend_range", IssueAct::Extend),
            Issue::BadTotal { .. } => item(ui, "audit.sum_above", IssueAct::SumAbove),
            Issue::ErrorValue => item(ui, "audit.trace_error", IssueAct::Trace),
        }
        item(ui, "audit.ignore", IssueAct::Ignore);
        if !matches!(issue, Issue::ErrorValue) {
            item(ui, "audit.ignore_all", IssueAct::IgnoreAll);
        }
        item(ui, "audit.edit_bar", IssueAct::Edit);
        act
    }

    pub(super) fn audit_popups(&mut self, ctx: &egui::Context) {
        self.issue_menu(ctx);
        self.checker(ctx);
        self.tests_popups(ctx);
        self.origin_window(ctx);
        self.watch_window(ctx);
        self.evaluate_window(ctx);
    }

    fn issue_menu(&mut self, ctx: &egui::Context) {
        if !self.audit.menu {
            return;
        }
        let (Some(b), Some(issue)) = (self.issue_button(), self.issue_at(self.cursor)) else {
            self.audit.menu = false;
            return;
        };
        let mut act = None;
        let area = egui::Area::new(Id::new(("issue_menu", self.uid))).order(egui::Order::Foreground).fixed_pos(b.left_bottom() + vec2(0.0, 2.0)).constrain(true).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(200.0);
                ui.label(RichText::new(issue.text(&self.wb)).strong());
                ui.separator();
                act = self.issue_actions(ui, issue);
            });
        });
        if let Some(a) = act {
            let c = self.cursor;
            self.issue_action(ctx, c, a);
        } else if ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !area.response.rect.contains(p) && !b.contains(p))) || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.audit.menu = false;
        }
    }

    /// Opens the error checker on the next flagged cell from the cursor.
    pub(super) fn open_checker(&mut self) {
        self.settle_edit();
        self.audit.checker = true;
        if self.issue_at(self.cursor).is_none() {
            self.step_issue(true);
        }
    }

    fn step_issue(&mut self, forward: bool) -> bool {
        let all = self.all_issues();
        let key = |c: &CellRef| (c.row, c.col);
        let cur = key(&self.cursor);
        let next = if forward {
            all.iter().find(|c| key(c) > cur).or(all.first())
        } else {
            all.iter().rev().find(|c| key(c) < cur).or(all.last())
        };
        match next.copied() {
            Some(c) => {
                self.cursor = c;
                self.anchor = c;
                self.ensure_visible = true;
                true
            }
            None => false,
        }
    }

    fn checker(&mut self, ctx: &egui::Context) {
        if !self.audit.checker {
            return;
        }
        let mut open = true;
        let issue = self.issue_at(self.cursor);
        let (mut act, mut step) = (None, None);
        crate::widgets::esc_closes(ctx, Id::new(("err_check", self.uid)), &mut open);
        egui::Window::new(tr("audit.checker")).id(Id::new(("err_check", self.uid))).pivot(egui::Align2::RIGHT_TOP).default_pos(ctx.content_rect().right_top() + egui::vec2(-40.0, 140.0)).open(&mut open).collapsible(false).resizable(false).default_width(360.0).show(ctx, |ui| {
            match issue {
                Some(i) => {
                    crate::widgets::section(ui, &format!("{} {}", tr("audit.error_in"), self.cursor.to_a1()));
                    let text = self.wb.edit_text(self.sheet, self.cursor);
                    crate::widgets::form(ui, "err_check_form", |ui| {
                        crate::widgets::form_row(ui, tr("audit.formula"), |ui| ui.label(RichText::new(text).monospace()));
                        crate::widgets::form_row(ui, tr("audit.problem"), |ui| ui.add(egui::Label::new(i.text(&self.wb)).wrap()));
                    });
                    crate::widgets::section(ui, tr("audit.actions"));
                    act = self.issue_actions(ui, i);
                }
                None => {
                    ui.add_space(8.0);
                    ui.label(tr("audit.no_errors"));
                }
            }
            ui.add_space(8.0);
            ui.separator();
            // With no errors (as it says) there is nothing to step to.
            ui.add_enabled_ui(issue.is_some() || !self.all_issues().is_empty(), |ui| {
                ui.horizontal(|ui| {
                    if ui.button(tr("audit.previous")).clicked() {
                        step = Some(false);
                    }
                    if ui.button(tr("audit.next")).clicked() {
                        step = Some(true);
                    }
                });
            });
        });
        let c = self.cursor;
        if let Some(a) = act {
            self.issue_action(ctx, c, a);
            if a != IssueAct::Edit && a != IssueAct::Trace {
                self.step_issue(true);
            }
        }
        if let Some(fwd) = step {
            self.step_issue(fwd);
        }
        if !open {
            self.audit.checker = false;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum IssueAct {
    Convert,
    CopyAbove,
    Extend,
    SumAbove,
    Ignore,
    IgnoreAll,
    Edit,
    Trace,
}

fn arrow_head(p: &egui::Painter, s: Pos2, e: Pos2, color: Color32) {
    let d = e - s;
    if d.length() < 1.0 {
        return;
    }
    let u = d.normalized();
    let n = vec2(-u.y, u.x);
    let base = e - u * 8.0;
    p.add(Shape::convex_polygon(vec![e, base + n * 3.5, base - n * 3.5], color, Stroke::NONE));
}

fn sheet_icon(p: &egui::Painter, at: Pos2) {
    let r = Rect::from_center_size(at, vec2(16.0, 16.0));
    p.rect_filled(r, 1.0, Color32::WHITE);
    crate::icons::paint(p, r, Icon::Sheet, Color32::from_gray(60));
}
