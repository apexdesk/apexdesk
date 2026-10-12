//! Plans survive xlsx and ods: settings, calendars, column roles and the
//! scheduled values, and the table still reschedules after loading.

use apex_sheet::format::date_to_serial;
use apex_sheet::plan::{Exception, Field, PlanKind};
use apex_sheet::workbook::NewTask;
use apex_sheet::{CellRef, Locale, Value, Workbook};

fn a(s: &str) -> CellRef {
    CellRef::parse(s).unwrap()
}

fn d(m: i64, dd: i64) -> f64 {
    date_to_serial(2026, m, dd)
}

fn book() -> Workbook {
    let mut wb = Workbook::new();
    wb.locale = Locale::EN;
    let tasks = [NewTask::new("Design", 0, 3.0, ""), NewTask::new("Build", 0, 5.0, "1"), NewTask::new("Launch", 0, 0.0, "2")];
    wb.insert_plan(0, a("A1"), d(9, 28), &tasks).unwrap();
    wb.set_input(0, a("G2"), "Ana");
    wb.plan_resource_sheet((0, 0), "Resources").unwrap();
    let mut p = wb.sheets[0].tables[0].plan.as_deref().unwrap().clone();
    p.calendars[0].exceptions.push(Exception::holiday("Fiesta & <co>", d(10, 12) as i64));
    p.calendars[0].exceptions.push(Exception { name: "Saturday shift".into(), from: d(10, 3) as i64, to: d(10, 3) as i64, working: true });
    p.calendars[0].hours = vec![(8.5, 13.0), (14.0, 17.5)];
    let mut night = apex_sheet::plan::Calendar::standard("Night");
    night.weekdays = [true, true, true, true, false, false, false];
    night.inherit = true;
    p.calendars.push(night);
    p.status = Some(d(10, 1));
    wb.set_plan((0, 0), p);
    wb.plan_save_baseline((0, 0), d(9, 27));
    let f = apex_sheet::plan::CustomField { column: "Risk & <x>".into(), formula: "IF([Duration]<4,1,3)".into(), rollup: apex_sheet::plan::Rollup::Max };
    let light = apex_sheet::workbook::Indicator { set: "3TrafficLights1".into(), low: 2.0, high: 3.0, reverse: false };
    wb.plan_set_custom((0, 0), f, Some(light));
    wb
}

fn check(back: &Workbook, wb: &Workbook) {
    for (s, (x, y)) in back.sheets.iter().zip(&wb.sheets).enumerate() {
        assert_eq!(x.tables.len(), y.tables.len(), "sheet {s}");
        for (t, u) in x.tables.iter().zip(&y.tables) {
            assert_eq!(t.name, u.name);
            assert_eq!(t.plan, u.plan, "{}", t.name);
            let f = |t: &apex_sheet::table::Table| t.columns.iter().map(|c| c.field).collect::<Vec<_>>();
            assert_eq!(f(t), f(u));
        }
    }
    assert!(!back.sheets[0].tables[0].plan.as_ref().unwrap().resources.is_empty());
    assert_eq!(back.sheets[1].tables[0].plan.as_ref().unwrap().kind, PlanKind::Resources);
    assert_eq!(back.sheets[0].tables[0].plan.as_ref().unwrap().custom.len(), 1);
    assert!(back.plan_indicator((0, 0), "Risk & <x>").is_some());
    assert_eq!(back.plan_indicator((0, 0), "Risk & <x>"), wb.plan_indicator((0, 0), "Risk & <x>"));
    for c in ["D2", "E2", "D3", "E3", "D4", "A3", "F3"] {
        assert_eq!(back.sheets[0].value(a(c)), wb.sheets[0].value(a(c)), "{c}");
    }
}

#[test]
fn roundtrip() {
    let wb = book();
    assert!(wb.sheets[0].tables[0].columns.iter().any(|c| c.field == Some(Field::BaselineStart)));
    for (xlsx, data) in [(true, crate::xlsx::write(&wb)), (false, crate::ods::write(&wb))] {
        let mut back = if xlsx { crate::xlsx::read(&data).unwrap() } else { crate::ods::read(&data).unwrap() };
        check(&back, &wb);
        // Written again, nothing changes.
        let again = if xlsx { crate::xlsx::read(&crate::xlsx::write(&back)).unwrap() } else { crate::ods::read(&crate::ods::write(&back)).unwrap() };
        check(&again, &wb);
        // Still a plan: a longer design moves what follows.
        let before = back.sheets[0].value(a("D3")).clone();
        back.set_input(0, a("C2"), "4");
        assert_ne!(back.sheets[0].value(a("D3")), &before, "xlsx {xlsx}");
    }
}

/// Other programs see a plain table; ours is an extension they skip.
#[test]
fn plain_for_others() {
    let data = crate::xlsx::write(&book());
    let z = crate::zip::ZipReader::new(&data).unwrap();
    let wbx = z.read_string("xl/workbook.xml").unwrap();
    assert!(wbx.contains("<apex:plans><apex:plan table=\""), "{wbx}");
    assert!(wbx.contains("Fiesta &amp; &lt;co&gt;"));
    let t1 = z.read_string("xl/tables/table1.xml").unwrap();
    assert!(!t1.contains("apex"));
    let data = crate::ods::write(&book());
    let z = crate::zip::ZipReader::new(&data).unwrap();
    assert!(z.read_string("apexdesk/model.xml").unwrap().contains("<apex:plans>"));
    assert!(z.read_string("META-INF/manifest.xml").unwrap().contains("apexdesk/model.xml"));
    let _ = Value::Empty;
}

/// With APEX_PLAN_OUT set, writes the sample plan there (plan.xlsx and
/// plan.ods) to open in other programs.
#[test]
fn sample_files() {
    let Ok(dir) = std::env::var("APEX_PLAN_OUT") else { return };
    let wb = book();
    std::fs::write(format!("{dir}/plan.xlsx"), crate::xlsx::write(&wb)).unwrap();
    std::fs::write(format!("{dir}/plan.ods"), crate::ods::write(&wb)).unwrap();
}
