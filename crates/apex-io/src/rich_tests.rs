//! Mixed formatting inside text cells through xlsx and ods.

use std::sync::Arc;

use apex_sheet::rich::{RichRun, RunFont, Script};
use apex_sheet::{CellRef, Value, Workbook};

fn book() -> Workbook {
    let mut wb = Workbook::new();
    wb.set_input(0, CellRef::new(0, 0), "Hello big world H2O");
    let runs = vec![
        RichRun { len: 6, font: RunFont::default() },
        RichRun { len: 3, font: RunFont { bold: Some(true), color: Some(0xFF0000), ..Default::default() } },
        RichRun { len: 7, font: RunFont { italic: Some(true), size: Some(14.0), font: Some(Arc::from("Georgia")), underline: Some(true), ..Default::default() } },
        RichRun { len: 1, font: RunFont::default() },
        RichRun { len: 1, font: RunFont { script: Script::Sub, strike: Some(true), ..Default::default() } },
        RichRun { len: 1, font: RunFont::default() },
    ];
    wb.sheets[0].edit_extra(CellRef::new(0, 0), |x| x.rich = Some(Arc::new(runs)));
    wb
}

fn check(wb: &Workbook, what: &str) {
    let cell = wb.sheets[0].get(CellRef::new(0, 0)).unwrap();
    assert_eq!(cell.value(), &Value::text("Hello big world H2O"), "{what}");
    let runs = cell.rich().unwrap_or_else(|| panic!("{what}: no runs"));
    let text = "Hello big world H2O";
    let mut at = 0;
    let mut found = Vec::new();
    for r in runs {
        found.push((&text[at..at + r.len], r.font.clone()));
        at += r.len;
    }
    let get = |s: &str| found.iter().find(|(t, _)| *t == s).map(|x| x.1.clone()).unwrap_or_else(|| panic!("{what}: no run {s:?} in {found:?}"));
    let big = get("big");
    assert_eq!((big.bold, big.color), (Some(true), Some(0xFF0000)), "{what}");
    let world = get(" world ");
    assert_eq!((world.italic, world.size, world.font.as_deref(), world.underline), (Some(true), Some(14.0), Some("Georgia"), Some(true)), "{what}");
    let two = get("2");
    assert_eq!((two.script, two.strike), (Script::Sub, Some(true)), "{what}");
    assert!(!get("Hello ").bold.unwrap_or(false), "{what}");
}

#[test]
fn rich_text_xlsx() {
    let wb = book();
    check(&wb, "model");
    let back = crate::xlsx::read(&crate::xlsx::write(&wb)).unwrap();
    check(&back, "xlsx");
    // Editing the text drops the runs.
    let mut back = back;
    back.set_input(0, CellRef::new(0, 0), "Plain");
    assert!(back.sheets[0].get(CellRef::new(0, 0)).unwrap().rich().is_none());
}

#[test]
fn rich_text_ods() {
    let back = crate::ods::read(&crate::ods::write(&book())).unwrap();
    check(&back, "ods");
}
