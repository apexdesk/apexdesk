//! Plan tables in a workbook: dates follow edits in one undo step, typed
//! starts and finishes, outline, links, renumbering, baseline, leveling,
//! resources and costs. Columns of a new plan: A WBS, B name, C duration,
//! D start, E finish, F predecessors, G resources, H % complete, I notes.

use super::{NewTask, Workbook};
use crate::addr::{CellRef, Range};
use crate::format::date_to_serial;
use crate::plan::{Field, Warning};
use crate::value::Value;
use crate::Locale;

fn a(s: &str) -> CellRef {
    CellRef::parse(s).unwrap()
}

fn d(m: i64, dd: i64) -> f64 {
    date_to_serial(2026, m, dd)
}

fn val(wb: &Workbook, s: &str) -> Value {
    wb.sheets[0].value(a(s)).clone()
}

fn n(wb: &Workbook, s: &str) -> f64 {
    match val(wb, s) {
        Value::Number(x) => x,
        v => panic!("{s}: {v:?}"),
    }
}

fn txt(wb: &Workbook, s: &str) -> String {
    match val(wb, s) {
        Value::Text(t) => t.to_string(),
        Value::Empty => String::new(),
        v => panic!("{s}: {v:?}"),
    }
}

/// Design (3d), Build (5d, after 1), Launch (milestone, after 2).
fn book() -> Workbook {
    let mut wb = Workbook::new();
    wb.locale = Locale::EN;
    let tasks = [NewTask::new("Design", 0, 3.0, ""), NewTask::new("Build", 0, 5.0, "1"), NewTask::new("Launch", 0, 0.0, "2")];
    wb.insert_plan(0, a("A1"), d(9, 28), &tasks).unwrap();
    wb
}

#[test]
fn insert_schedules() {
    let wb = book();
    let t = &wb.sheets[0].tables[0];
    assert_eq!(t.range, Range::parse("A1:I4").unwrap());
    assert_eq!(t.columns[1].field, Some(Field::Name));
    assert_eq!(n(&wb, "D2"), d(9, 28));
    assert_eq!(n(&wb, "E2"), d(9, 30));
    assert_eq!(n(&wb, "D3"), d(10, 1));
    assert_eq!(n(&wb, "E3"), d(10, 7));
    assert_eq!(n(&wb, "D4"), d(10, 7));
    assert_eq!(txt(&wb, "A3"), "2");
    // Dates look like dates.
    let st = wb.sheets[0].get(a("D2")).unwrap().style;
    assert!(wb.styles.format(st).is_date());
    // One undo step removes it all.
    let mut wb = wb;
    assert_eq!(wb.undo_len(), 1);
    wb.undo();
    assert!(wb.sheets[0].tables.is_empty());
    assert_eq!(val(&wb, "D2"), Value::Empty);
}

#[test]
fn edits_move_what_depends_in_one_step() {
    let mut wb = book();
    let steps = wb.undo_len();
    wb.set_input(0, a("C2"), "5");
    assert_eq!(n(&wb, "E2"), d(10, 2));
    assert_eq!(n(&wb, "D3"), d(10, 5));
    assert_eq!(n(&wb, "D4"), d(10, 9));
    assert_eq!(wb.undo_len(), steps + 1);
    wb.undo();
    assert_eq!(n(&wb, "C2"), 3.0);
    assert_eq!(n(&wb, "D3"), d(10, 1));
    wb.redo();
    assert_eq!(n(&wb, "D3"), d(10, 5));
    // Durations as text.
    wb.set_input(0, a("C3"), "1w");
    assert_eq!(n(&wb, "C3"), 5.0);
    wb.set_input(0, a("C3"), "4h");
    assert_eq!(n(&wb, "C3"), 0.5);
    assert_eq!(n(&wb, "E3"), d(10, 5));
}

#[test]
fn typed_start_and_finish() {
    let mut wb = book();
    // A start typed on an automatic task: a constraint (columns added).
    wb.set_input(0, a("D2"), "10/5/2026");
    let t = &wb.sheets[0].tables[0];
    assert_eq!(t.range.end.col, 10);
    assert_eq!(t.columns[9].field, Some(Field::Constraint));
    assert_eq!(txt(&wb, "J2"), "SNET");
    assert_eq!(n(&wb, "K2"), d(10, 5));
    assert_eq!(n(&wb, "D2"), d(10, 5));
    assert_eq!(n(&wb, "D3"), d(10, 8));
    wb.undo();
    assert_eq!(wb.sheets[0].tables[0].range.end.col, 8);
    assert_eq!(n(&wb, "D2"), d(9, 28));
    // A finish typed: the duration.
    wb.set_input(0, a("E3"), "10/9/2026");
    assert_eq!(n(&wb, "C3"), 7.0);
    assert_eq!(n(&wb, "D4"), d(10, 9));
}

#[test]
fn outline_and_summaries() {
    let mut wb = book();
    // Build becomes a phase with two steps.
    wb.plan_insert_tasks((0, 0), 3, 2);
    wb.set_input(0, a("B4"), "Frame");
    wb.set_input(0, a("C4"), "2");
    wb.set_input(0, a("B5"), "Roof");
    wb.set_input(0, a("C5"), "3");
    wb.set_input(0, a("F5"), "3");
    wb.plan_indent((0, 0), &[3, 4], 1);
    // Launch (now row 6) still follows Build (id 2).
    assert_eq!(txt(&wb, "F6"), "2");
    assert_eq!(txt(&wb, "A4"), "2.1");
    assert_eq!(txt(&wb, "A6"), "3");
    // Build: from Frame to Roof, 5 days.
    assert_eq!(n(&wb, "D3"), d(10, 1));
    assert_eq!(n(&wb, "E3"), d(10, 7));
    assert_eq!(n(&wb, "C3"), 5.0);
    assert!(wb.styles.get(wb.sheets[0].get(a("B3")).unwrap().style).bold);
    assert_eq!(n(&wb, "D6"), d(10, 7));
    // Progress rolls up.
    wb.set_input(0, a("H4"), "100%");
    assert!((n(&wb, "H3") - 0.4).abs() < 1e-9);
    // Outdent: Build is a plain task again.
    wb.plan_indent((0, 0), &[3, 4], -1);
    assert!(!wb.styles.get(wb.sheets[0].get(a("B3")).unwrap().style).bold);
    // A plain task made bold stays so when the dates change.
    wb.modify_style(0, &Range::single(a("B4")), |s| s.bold = true);
    wb.set_input(0, a("C4"), "4");
    assert!(wb.styles.get(wb.sheets[0].get(a("B4")).unwrap().style).bold);
}

#[test]
fn links_and_renumbering() {
    let mut wb = book();
    wb.plan_link((0, 0), &[1, 2, 3], None);
    assert_eq!(val(&wb, "F3"), Value::Empty);
    assert_eq!(n(&wb, "D3"), d(9, 28));
    wb.plan_link((0, 0), &[1, 3], Some(crate::plan::DepType::SS));
    assert_eq!(txt(&wb, "F4"), "1SS");
    assert_eq!(n(&wb, "D4"), d(9, 28));
    wb.plan_add_link((0, 0), 2, 3, crate::plan::DepType::FS);
    assert_eq!(txt(&wb, "F4"), "1SS;2");
    // Deleting Design drops the links to it; Build is now 1.
    wb.plan_delete_tasks((0, 0), &[1]);
    assert_eq!(txt(&wb, "B2"), "Build");
    assert_eq!(txt(&wb, "F3"), "1");
    // A cycle is reported, not a hang.
    wb.set_input(0, a("F2"), "2");
    let v = wb.plan_view((0, 0)).unwrap();
    assert!(v.warnings.iter().any(|w| matches!(w, Warning::Cycle(_))));
    // A link to a task that does not exist.
    wb.set_input(0, a("F2"), "9");
    let v = wb.plan_view((0, 0)).unwrap();
    assert!(v.warnings.contains(&Warning::BadLink { task: 0, pred: 8 }));
}

#[test]
fn deleting_every_task() {
    let mut wb = book();
    wb.plan_delete_tasks((0, 0), &[1, 2, 3]);
    assert_eq!(wb.sheets[0].tables[0].range, Range::parse("A1:I2").unwrap());
    assert_eq!(val(&wb, "B2"), Value::Empty);
    assert_eq!(val(&wb, "A3"), Value::Empty);
    wb.undo();
    assert_eq!(txt(&wb, "B4"), "Launch");
}

#[test]
fn manual_tasks_and_moving() {
    let mut wb = book();
    wb.plan_set_mode((0, 0), &[2], true, "Manual", "Auto");
    assert_eq!(txt(&wb, "J3"), "Manual");
    wb.set_input(0, a("C2"), "10");
    // Build does not move.
    assert_eq!(n(&wb, "D3"), d(10, 1));
    let v = wb.plan_view((0, 0)).unwrap();
    assert!(v.warnings.contains(&Warning::LinkConflict { task: 1, pred: 0 }));
    // Moved in the Gantt: it keeps its length.
    wb.plan_move_task((0, 0), 2, d(10, 12));
    assert_eq!(n(&wb, "D3"), d(10, 12));
    assert_eq!(n(&wb, "E3"), d(10, 16));
    // An automatic task moved gets a constraint.
    wb.plan_move_task((0, 0), 1, d(10, 5));
    assert_eq!(n(&wb, "D2"), d(10, 5));
    // Stretched.
    wb.plan_set_duration((0, 0), 1, 2.0);
    assert_eq!(n(&wb, "E2"), d(10, 6));
}

#[test]
fn resources_costs_and_leveling() {
    let mut wb = book();
    wb.plan_link((0, 0), &[2], None);
    wb.set_input(0, a("G2"), "Ana");
    wb.set_input(0, a("G3"), "Ana;Cement[20]");
    let rid = wb.plan_resource_sheet((0, 0), "Resources").unwrap();
    assert_eq!(wb.sheets.len(), 2);
    let rs = &wb.sheets[rid.0];
    assert_eq!(rs.value(a("A2")), &Value::text("Ana"));
    // Ana costs 10 an hour; cement is a material at 5 each.
    wb.set_input(1, a("E2"), "10");
    wb.set_input(1, a("B3"), "material");
    wb.set_input(1, a("E3"), "5");
    let v = wb.plan_view((0, 0)).unwrap();
    assert_eq!(v.resources[1].kind, crate::plan::ResKind::Material);
    // Design: 3 days * 8 h * 10; Build: 5 * 8 * 10 + 20 * 5.
    assert!((v.cost[0] - 240.0).abs() < 1e-9);
    assert!((v.cost[1] - 500.0).abs() < 1e-9);
    assert!((v.work[1] - 40.0).abs() < 1e-9);
    assert!((v.total_cost() - 740.0).abs() < 1e-9);
    // Ana does both at once: leveling puts Design (which has slack) after
    // Build.
    assert_eq!(n(&wb, "D3"), d(9, 28));
    let before = wb.undo_len();
    assert!(wb.plan_level((0, 0)));
    assert_eq!(wb.undo_len(), before + 1);
    assert_eq!(n(&wb, "D3"), d(9, 28));
    assert_eq!(n(&wb, "D2"), d(10, 5));
    assert_eq!(wb.sheets[0].tables[0].columns.last().unwrap().field, Some(Field::LevelDelay));
    wb.plan_clear_leveling((0, 0));
    assert_eq!(n(&wb, "D2"), d(9, 28));
    wb.undo();
    assert_eq!(n(&wb, "D2"), d(10, 5));
    wb.undo();
    assert_eq!(n(&wb, "D2"), d(9, 28));
    assert_eq!(wb.sheets[0].tables[0].range.end.col, 8);
}

#[test]
fn baseline_and_variance() {
    let mut wb = book();
    wb.plan_save_baseline((0, 0), d(9, 27));
    let t = &wb.sheets[0].tables[0];
    assert_eq!(t.plan.as_ref().unwrap().baseline, Some(d(9, 27)));
    assert_eq!(t.columns.len(), 15);
    assert_eq!(n(&wb, "J3"), d(10, 1));
    assert_eq!(n(&wb, "M3"), 0.0);
    wb.set_input(0, a("C2"), "5");
    // Build starts two days late.
    assert_eq!(n(&wb, "M3"), 2.0);
    assert_eq!(n(&wb, "N3"), 2.0);
    assert_eq!(wb.plan_view((0, 0)).unwrap().baseline[1], Some((d(10, 1) as i64, d(10, 7) as i64)));
}

#[test]
fn convert_a_table() {
    let mut wb = Workbook::new();
    wb.locale = Locale::EN;
    wb.set_inputs(0, vec![(a("A1"), "Task".into()), (a("B1"), "Duration".into()), (a("A2"), "One".into()), (a("B2"), "2".into()), (a("A3"), "Two".into()), (a("B3"), "3".into())]);
    let id = wb.create_table(0, Range::parse("A1:B3").unwrap(), true, "TableStyleMedium2").unwrap();
    let before = wb.undo_len();
    wb.convert_to_plan(id, d(9, 28)).unwrap();
    assert_eq!(wb.undo_len(), before + 1);
    let t = &wb.sheets[0].tables[0];
    assert_eq!(t.columns[0].field, Some(Field::Name));
    assert_eq!(t.columns[1].field, Some(Field::Duration));
    assert_eq!(t.columns.len(), 5);
    assert_eq!(n(&wb, "C3"), d(9, 28));
    wb.set_input(0, a("E3"), "1");
    assert_eq!(n(&wb, "C3"), d(9, 30));
    wb.undo();
    wb.undo();
    assert!(wb.sheets[0].tables[0].plan.is_none());
}

#[test]
fn new_rows_become_tasks() {
    let mut wb = book();
    // Typing below the table adds a task.
    wb.set_input(0, a("B5"), "Party");
    wb.table_after_input(0, a("B5"));
    assert_eq!(wb.sheets[0].tables[0].range.end.row, 4);
    assert_eq!(n(&wb, "C5"), 1.0);
    assert_eq!(n(&wb, "D5"), d(9, 28));
    // Blank rows are no tasks.
    wb.set_input(0, a("B5"), "");
    wb.set_input(0, a("C5"), "");
    assert_eq!(wb.plan_view((0, 0)).unwrap().rows.len(), 3);
    assert_eq!(val(&wb, "D5"), Value::Empty);
}

#[test]
fn calendar_settings() {
    let mut wb = book();
    let mut p = wb.sheets[0].tables[0].plan.as_deref().unwrap().clone();
    p.calendars[0].exceptions.push(crate::plan::Exception::holiday("Holiday", d(9, 29) as i64));
    wb.set_plan((0, 0), p);
    assert_eq!(n(&wb, "E2"), d(10, 1));
    wb.undo();
    assert_eq!(n(&wb, "E2"), d(9, 30));
}

/// Earned value at 1 October: Design (3 days, 300) is done, Build (5 days,
/// 1000 in the baseline, 2000 now) is 10 % done after its first day.
#[test]
fn earned_value() {
    let mut wb = book();
    wb.plan_add_field((0, 0), Field::FixedCost);
    wb.set_input(0, a("J2"), "300");
    wb.set_input(0, a("J3"), "1000");
    wb.plan_save_baseline((0, 0), d(9, 27));
    let mut p = wb.sheets[0].tables[0].plan.as_deref().unwrap().clone();
    p.status = Some(d(10, 1));
    wb.set_plan((0, 0), p);
    wb.set_input(0, a("H2"), "100");
    wb.set_input(0, a("H3"), "10");
    wb.set_input(0, a("J3"), "2000");
    let v = wb.plan_view((0, 0)).unwrap();
    assert_eq!(v.status, d(10, 1) as i64);
    let close = |a: f64, b: f64| assert!((a - b).abs() < 1e-6, "{a} != {b}");
    close(v.ev[1].pv, 200.0);
    close(v.ev[1].ev, 100.0);
    close(v.ev[1].ac, 200.0);
    let t = v.total_ev();
    close(t.bac, 1300.0);
    close(t.pv, 500.0);
    close(t.ev, 400.0);
    close(t.ac, 500.0);
    close(t.sv(), -100.0);
    close(t.cv(), -100.0);
    close(t.spi().unwrap(), 0.8);
    close(t.cpi().unwrap(), 0.8);
    close(t.eac(), 1625.0);
    close(t.vac(), -325.0);
    // As columns: filled in, and removed with an undo.
    let cols = wb.sheets[0].tables[0].columns.len() as u32;
    wb.plan_add_field((0, 0), Field::Cpi);
    assert_eq!(wb.sheets[0].value(CellRef::new(2, cols)), &Value::Number(0.5));
    wb.undo();
    assert_eq!(wb.sheets[0].tables[0].columns.len() as u32, cols);
    // The curves: planned value grows to the budget, earned value and
    // actual cost stop at the status date.
    let curves = v.ev_curves(1);
    let last = curves.last().unwrap();
    close(last.1, 1300.0);
    assert!(last.2.is_none());
    let at_status = curves.iter().find(|c| c.0 == d(10, 1) as i64).unwrap();
    close(at_status.1, 500.0);
    close(at_status.2.unwrap(), 400.0);
    close(at_status.3.unwrap(), 500.0);
    assert!(curves.windows(2).all(|w| w[1].1 >= w[0].1 - 1e-9));
}

/// Every report makes a sheet after the plan's, in one undo step.
#[test]
fn reports() {
    use super::PlanReport;
    let mut wb = book();
    wb.set_input(0, a("G2"), "Ana");
    wb.set_input(0, a("G3"), "Ana");
    wb.plan_link((0, 0), &[2], None);
    wb.plan_save_baseline((0, 0), d(9, 27));
    let mut p = wb.sheets[0].tables[0].plan.as_deref().unwrap().clone();
    p.status = Some(d(10, 20));
    wb.set_plan((0, 0), p);
    let t = |k: &str| k.trim_start_matches("plan.rep.").to_string();
    for kind in PlanReport::ALL {
        let before = wb.undo_len();
        let s = wb.plan_report((0, 0), kind, &t).unwrap();
        assert_eq!(s, 1);
        assert_eq!(wb.undo_len(), before + 1);
        assert!(wb.sheets[1].name.starts_with(&t(kind.key())), "{}", wb.sheets[1].name);
        assert!(matches!(wb.sheets[1].value(a("B2")), Value::Number(x) if *x == d(10, 20)));
        wb.undo();
        assert_eq!(wb.sheets.len(), 1);
    }
    // Late: nothing is done by 20 October.
    wb.plan_report((0, 0), PlanReport::LateTasks, &t);
    assert_eq!(wb.sheets[1].value(a("B5")), &Value::text("Design"));
    assert_eq!(wb.sheets[1].value(a("B6")), &Value::text("Build"));
    assert_eq!(wb.sheets[1].charts.len(), 1);
    // Overload: Ana does Design and Build at once for three days.
    wb.plan_report((0, 0), PlanReport::Overload, &t);
    assert_eq!(wb.sheets[1].value(a("A5")), &Value::text("Ana"));
    assert_eq!(wb.sheets[1].value(a("D5")), &Value::Number(3.0));
    // Earned value: the three curves.
    wb.plan_report((0, 0), PlanReport::EarnedValue, &t);
    assert_eq!(wb.sheets[1].charts[0].series.len(), 3);
    assert_eq!(wb.sheets.len(), 4);
}

/// A custom field: a formula over the task's columns, rolled up into the
/// summaries, with a traffic light, kept after edits, in one undo step.
#[test]
fn custom_fields() {
    use super::Indicator;
    use crate::data::CfKind;
    use crate::plan::{CustomField, Rollup};
    let mut wb = book();
    wb.plan_insert_tasks((0, 0), 3, 2);
    wb.set_input(0, a("B4"), "Frame");
    wb.set_input(0, a("C4"), "2");
    wb.set_input(0, a("B5"), "Roof");
    wb.set_input(0, a("C5"), "3");
    wb.plan_indent((0, 0), &[3, 4], 1);
    let steps = wb.undo_len();
    let f = CustomField { column: "Double".into(), formula: "=[Duration]*2".into(), rollup: Rollup::Max };
    let light = Indicator { set: "3TrafficLights1".into(), low: 4.0, high: 6.0, reverse: false };
    wb.plan_set_custom((0, 0), f, Some(light.clone()));
    assert_eq!(wb.undo_len(), steps + 1);
    assert_eq!(txt(&wb, "J1"), "Double");
    assert_eq!(n(&wb, "J2"), 6.0);
    assert_eq!(n(&wb, "J4"), 4.0);
    assert_eq!(n(&wb, "J5"), 6.0);
    // Build is a summary: the largest of its subtasks, not 2 × 5.
    assert_eq!(n(&wb, "J3"), 6.0);
    assert!(matches!(wb.sheets[0].cond_formats[0].kind, CfKind::IconSet { .. }));
    assert_eq!(wb.plan_indicator((0, 0), "double"), Some(light));
    // Edits recompute it.
    wb.set_input(0, a("C5"), "4");
    assert_eq!(n(&wb, "J5"), 8.0);
    assert_eq!(n(&wb, "J3"), 8.0);
    // Sum instead, no light.
    let f = CustomField { column: "Double".into(), formula: "[Duration]*2".into(), rollup: Rollup::Sum };
    wb.plan_set_custom((0, 0), f, None);
    assert_eq!(n(&wb, "J3"), 12.0);
    assert!(wb.sheets[0].cond_formats.is_empty());
    wb.undo();
    assert_eq!(n(&wb, "J3"), 8.0);
    assert_eq!(wb.sheets[0].cond_formats.len(), 1);
    wb.undo();
    wb.undo();
    assert_eq!(wb.sheets[0].tables[0].columns.len(), 9);
    assert!(wb.sheets[0].cond_formats.is_empty());
    assert!(wb.plan_formula_error("[Duration]*").is_some());
    assert!(wb.plan_formula_error("[Duration]*2").is_none());
}

/// Team Planner: a task moves from one resource to another (keeping its
/// units), to nobody, and from nobody, one undo step each.
#[test]
fn reassign() {
    let mut wb = book();
    wb.set_input(0, a("G2"), "Ana[50%];Luis");
    let steps = wb.undo_len();
    assert!(wb.plan_reassign((0, 0), 0, Some("ana"), Some("Eva")));
    assert_eq!(txt(&wb, "G2"), "Luis;Eva[50%]");
    assert_eq!(wb.undo_len(), steps + 1);
    assert!(wb.plan_reassign((0, 0), 0, Some("Luis"), None));
    assert_eq!(txt(&wb, "G2"), "Eva[50%]");
    assert!(wb.plan_reassign((0, 0), 1, None, Some("Ana")));
    assert_eq!(txt(&wb, "G3"), "Ana");
    assert!(!wb.plan_reassign((0, 0), 1, Some("Ana"), Some("ana")));
    wb.undo();
    wb.undo();
    wb.undo();
    assert_eq!(txt(&wb, "G2"), "Ana[50%];Luis");
}
