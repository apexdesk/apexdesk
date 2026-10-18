//! Formulas > Tests: formulas saved with the workbook that must stay true
//! (totals that add up, columns without errors...). They run again after
//! every recalculation; one that stops passing is announced, and the
//! status bar counts them. Proposals come from the patterns the sheet
//! shows (see `Workbook::propose_checks`).

use apex_sheet::{Check, Outcome};
use eframe::egui::{self, Color32, Id, RichText};

use super::SheetView;
use crate::i18n::{tr, trf};
use crate::widgets::FitWindow as _;

const PASS: Color32 = Color32::from_rgb(0x1E, 0x8E, 0x4E);
const FAIL: Color32 = Color32::from_rgb(0xC4, 0x2B, 0x1C);

#[derive(Default)]
pub(crate) struct TestsState {
    pub open: bool,
    /// The last results and what they were computed at (revision, tests).
    results: Vec<Outcome>,
    at: Option<(u64, usize)>,
    /// A new test being typed.
    name: String,
    formula: String,
    /// The proposals shown (None: not asked for).
    proposals: Option<Vec<Check>>,
    error: Option<String>,
}

fn mark(o: Outcome) -> (&'static str, Color32) {
    match o {
        Outcome::Pass => ("✔", PASS),
        Outcome::Fail => ("✖", FAIL),
        Outcome::Error => ("⚠", Color32::from_rgb(0xC8, 0x8A, 0x10)),
    }
}

impl SheetView {
    /// A test's formula as typed in this locale, with `=`.
    fn check_text(&self, c: &Check) -> String {
        match apex_sheet::formula::parse(&c.formula, &apex_sheet::format::Locale::default()) {
            Ok(e) => format!("={}", e.to_display(&self.wb.locale).trim_start_matches('=')),
            Err(_) => format!("={}", c.formula),
        }
    }

    fn check_label(&self, c: &Check) -> String {
        if c.name.is_empty() { self.check_text(c) } else { c.name.clone() }
    }

    /// Runs the tests again when the workbook changed, and announces the
    /// ones that stopped passing.
    pub(super) fn run_tests(&mut self) {
        let at = (self.wb.revision, self.wb.checks.len());
        if self.audit.tests.at == Some(at) {
            return;
        }
        let new = self.wb.run_checks();
        let before = std::mem::take(&mut self.audit.tests.results);
        if before.len() == new.len() {
            for (i, (a, b)) in before.iter().zip(&new).enumerate() {
                if *a == Outcome::Pass && *b != Outcome::Pass {
                    let name = self.check_label(&self.wb.checks[i]);
                    self.notices.push(crate::app::Notice::error(trf("tests.now_failing", &[("name", &name)])));
                }
            }
        }
        self.audit.tests.results = new;
        self.audit.tests.at = Some(at);
    }

    /// (passing, total) for the status bar.
    pub(super) fn tests_count(&self) -> Option<(usize, usize)> {
        let r = &self.audit.tests.results;
        (!self.wb.checks.is_empty() && r.len() == self.wb.checks.len()).then(|| (r.iter().filter(|o| **o == Outcome::Pass).count(), r.len()))
    }

    pub(super) fn open_tests(&mut self) {
        self.settle_edit();
        self.audit.tests.open = true;
        self.audit.tests.error = None;
    }

    /// Adds the test typed in the dialog.
    fn add_typed_test(&mut self) {
        let t = &self.audit.tests;
        let src = t.formula.trim().trim_start_matches('=').to_string();
        match apex_sheet::formula::parse(&src, &self.wb.locale) {
            Ok(e) if !src.is_empty() => {
                let mut list = self.wb.checks.clone();
                list.push(Check { name: t.name.trim().to_string(), sheet: self.sh().name.clone(), formula: e.to_canonical() });
                self.wb.set_checks(list);
                self.audit.tests.name.clear();
                self.audit.tests.formula.clear();
                self.audit.tests.error = None;
            }
            _ => self.audit.tests.error = Some(tr("tests.bad_formula").to_string()),
        }
    }

    pub(super) fn tests_popups(&mut self, ctx: &egui::Context) {
        self.run_tests();
        if !self.audit.tests.open {
            return;
        }
        let mut open = true;
        let (mut remove, mut go, mut add, mut propose, mut take) = (None, None, false, false, Vec::new());
        crate::widgets::esc_closes(ctx, Id::new(("tests", self.uid)), &mut open);
        egui::Window::new(tr("tests.title")).id(Id::new(("tests", self.uid))).pivot(egui::Align2::RIGHT_TOP).default_pos(ctx.content_rect().right_top() + egui::vec2(-40.0, 140.0)).open(&mut open).collapsible(false).default_width(460.0).show_fit(ctx, |ui| {
            ui.label(RichText::new(tr("tests.intro")).weak());
            ui.add_space(6.0);
            if self.wb.checks.is_empty() {
                ui.label(tr("tests.none"));
            }
            egui::ScrollArea::vertical().max_height(260.0).id_salt("tests_list").show(ui, |ui| {
                egui::Grid::new(("tests_grid", self.uid)).num_columns(4).spacing([8.0, 4.0]).show(ui, |ui| {
                    for (i, c) in self.wb.checks.iter().enumerate() {
                        let (m, color) = mark(self.audit.tests.results.get(i).copied().unwrap_or(Outcome::Error));
                        ui.label(RichText::new(m).color(color).strong());
                        ui.label(self.check_label(c)).on_hover_text(format!("{}\n{}", c.sheet, self.check_text(c)));
                        if ui.small_button(tr("tests.go")).on_hover_text(tr("tests.go_tip")).clicked() {
                            go = Some(i);
                        }
                        if ui.small_button("×").on_hover_text(tr("tests.remove")).clicked() {
                            remove = Some(i);
                        }
                        ui.end_row();
                    }
                });
            });
            crate::widgets::section(ui, tr("tests.new"));
            crate::widgets::form(ui, "tests_form", |ui| {
                crate::widgets::form_row(ui, tr("tests.name"), |ui| ui.add(egui::TextEdit::singleline(&mut self.audit.tests.name).desired_width(280.0)));
                crate::widgets::form_row(ui, tr("audit.formula"), |ui| ui.add(egui::TextEdit::singleline(&mut self.audit.tests.formula).hint_text("=SUM(B2:B9)=B10").font(egui::TextStyle::Monospace).desired_width(280.0)));
            });
            if let Some(e) = &self.audit.tests.error {
                ui.colored_label(FAIL, e);
            }
            ui.horizontal(|ui| {
                add = ui.button(tr("tests.add")).clicked();
                propose = ui.button(tr("tests.propose")).on_hover_text(tr("tests.propose_tip")).clicked();
            });
            if let Some(list) = &self.audit.tests.proposals {
                crate::widgets::section(ui, tr("tests.proposals"));
                if list.is_empty() {
                    ui.label(RichText::new(tr("tests.no_proposals")).weak());
                }
                for (i, c) in list.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.small_button(tr("tests.add")).clicked() {
                            take.push(i);
                        }
                        ui.label(RichText::new(self.check_text(c)).monospace());
                    });
                }
                if list.len() > 1 && ui.button(tr("tests.add_all")).clicked() {
                    take = (0..list.len()).collect();
                }
            }
        });
        if let Some(i) = remove {
            let mut list = self.wb.checks.clone();
            list.remove(i);
            self.wb.set_checks(list);
        }
        if let Some(i) = go {
            let c = self.wb.checks[i].clone();
            if let Some(s) = self.wb.sheets.iter().position(|s| s.name == c.sheet) {
                self.select_sheet(s);
            }
            // The first cell the formula reads.
            if let Some((s, r)) = self.wb.precedent_ranges(&c).into_iter().next() {
                if s != self.sheet {
                    self.select_sheet(s);
                }
                self.select(r.start, false);
                self.ensure_visible = true;
            }
        }
        if add {
            self.add_typed_test();
        }
        if propose {
            self.audit.tests.proposals = Some(self.wb.propose_checks(self.sheet));
        }
        if !take.is_empty() {
            if let Some(props) = self.audit.tests.proposals.take() {
                let mut list = self.wb.checks.clone();
                let (mut picked, rest): (Vec<_>, Vec<_>) = props.into_iter().enumerate().partition(|(i, _)| take.contains(i));
                list.extend(picked.drain(..).map(|(_, c)| c));
                self.wb.set_checks(list);
                self.audit.tests.proposals = Some(rest.into_iter().map(|(_, c)| c).collect());
            }
        }
        if !open {
            self.audit.tests.open = false;
            self.audit.tests.proposals = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::audit_ui::Issue;
    use super::*;
    use apex_sheet::{CellRef, Workbook};

    fn view() -> SheetView {
        let mut wb = Workbook::new();
        for (a, v) in [("A1", "Item"), ("B1", "Qty"), ("A2", "a"), ("B2", "=1+1"), ("A3", "b"), ("B3", "=2+2"), ("A4", "Total"), ("B4", "6")] {
            wb.set_input(0, CellRef::parse(a).unwrap(), v);
        }
        SheetView::new(wb)
    }

    #[test]
    fn tests_run_after_changes_and_warn() {
        let mut v = view();
        let props = v.wb.propose_checks(0);
        assert!(props.iter().any(|c| c.formula == "SUM(B2:B3)=B4"), "{props:?}");
        v.wb.set_checks(props.into_iter().filter(|c| c.formula.starts_with("SUM")).collect());
        v.run_tests();
        assert_eq!(v.tests_count(), Some((1, 1)));
        v.wb.set_input(0, CellRef::parse("B4").unwrap(), "7");
        v.run_tests();
        assert_eq!(v.tests_count(), Some((0, 1)));
        assert!(v.notices.iter().any(|n| n.text.contains("SUM(B2:B3)=B4")), "announced");
        // The review flags the total, and its action fixes it.
        let b4 = CellRef::parse("B4").unwrap();
        assert!(matches!(v.issue_at(b4), Some(Issue::BadTotal { sum, .. }) if sum == 6.0));
        v.review_fix(b4);
        assert_eq!(v.wb.edit_text(0, b4), "=SUM(B2:B3)");
        v.run_tests();
        assert_eq!(v.tests_count(), Some((1, 1)));
        // A test typed in the dialog.
        v.audit.tests.formula = "=B2<B3".into();
        v.add_typed_test();
        assert_eq!(v.wb.checks.last().unwrap().formula, "B2<B3");
        v.audit.tests.formula = "=SUM(".into();
        v.add_typed_test();
        assert!(v.audit.tests.error.is_some());
    }
}
