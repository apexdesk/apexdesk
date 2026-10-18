//! Scheduler tests. Dates are in September-October 2026; 28 September is a
//! Monday.

use super::calendar::Exception;
use super::engine::{outline, wbs};
use super::level::{Use, level, loads, overloaded};
use super::*;
use crate::format::date_to_serial;

fn d(m: i64, dd: i64) -> i64 {
    date_to_serial(2026, m, dd) as i64
}

fn mon() -> f64 {
    d(9, 28) as f64
}

fn task(dur: f64) -> TaskIn {
    TaskIn { duration: dur, ..Default::default() }
}

fn after(dur: f64, list: &str) -> TaskIn {
    TaskIn { duration: dur, links: parse::links(list, 8.0).0, ..Default::default() }
}

fn run(tasks: &[TaskIn]) -> Schedule {
    schedule(tasks, &[Calendar::standard("S")], mon())
}

/// (start day, finish day) of each task.
fn days(s: &Schedule) -> Vec<(i64, i64)> {
    s.tasks.iter().map(|t| (t.start_day, t.finish_day)).collect()
}

#[test]
fn finish_to_start_chain() {
    let s = run(&[task(3.0), after(2.0, "1"), after(0.0, "2")]);
    assert_eq!(days(&s), vec![(d(9, 28), d(9, 30)), (d(10, 1), d(10, 2)), (d(10, 2), d(10, 2))]);
    assert!(s.warnings.is_empty());
    assert!(s.tasks.iter().all(|t| t.critical));
    assert_eq!(s.tasks[2].slack, 0.0);
    // Project finish: end of Friday 2 October.
    assert_eq!(s.finish.floor() as i64, d(10, 2));
}

#[test]
fn link_types_and_lags() {
    let s = run(&[
        task(5.0),              // 1: Mon 28 - Fri 2
        after(2.0, "1SS+2d"),   // 2: Wed 30 - Thu 1
        after(2.0, "1FF"),      // 3: Thu 1 - Fri 2
        after(1.0, "1SF"),      // 4: finish >= start of 1: starts Mon 28? no earlier than project start
        after(2.0, "1FS+2d"),   // 5: Wed 7 - Thu 8
        after(1.0, "1FS-2d"),   // 6: Thu 1
        after(2.0, "1SS+40%"),  // 7: 40% of 5d = 2d: Wed 30 - Thu 1
        after(1.0, "1FS+3ed"),  // 8: 3 calendar days after the end of Fri 2 is the end of Mon 5
    ]);
    let v = days(&s);
    assert_eq!(v[1], (d(9, 30), d(10, 1)));
    assert_eq!(v[2], (d(10, 1), d(10, 2)));
    assert_eq!(v[3], (d(9, 28), d(9, 28)));
    assert_eq!(v[4], (d(10, 7), d(10, 8)));
    assert_eq!(v[5], (d(10, 1), d(10, 1)));
    assert_eq!(v[6], (d(9, 30), d(10, 1)));
    assert_eq!(v[7], (d(10, 6), d(10, 6)));
}

#[test]
fn slack_and_critical_path() {
    // 1 (5d) and 2 (2d) in parallel, both before 3.
    let s = run(&[task(5.0), task(2.0), after(1.0, "1;2")]);
    assert!(s.tasks[0].critical && s.tasks[2].critical);
    assert!(!s.tasks[1].critical);
    assert!((s.tasks[1].slack - 3.0).abs() < 1e-6);
    assert!((s.tasks[1].free_slack - 3.0).abs() < 1e-6);
    // Late dates of 2: Thu 1 - Fri 2.
    let cal = Calendar::standard("S");
    let w = WorkTime::new(&cal);
    assert_eq!(w.start_day(s.tasks[1].late_start), d(10, 1));
    assert_eq!(w.finish_day(s.tasks[1].late_finish), d(10, 2));
    // Free slack is smaller than total slack when a successor has slack.
    let s = run(&[task(5.0), task(1.0), after(1.0, "2"), after(1.0, "1;3")]);
    assert!((s.tasks[1].slack - 3.0).abs() < 1e-6);
    assert!(s.tasks[1].free_slack.abs() < 1e-6);
    assert!((s.tasks[2].free_slack - 3.0).abs() < 1e-6);
}

#[test]
fn constraints() {
    let mut a = task(2.0);
    a.constraint = Constraint::Snet(d(10, 7) as f64);
    let mut b = after(1.0, "1");
    b.constraint = Constraint::Mso(d(10, 1) as f64);
    let mut c = task(3.0);
    c.constraint = Constraint::Fnlt(d(9, 29) as f64);
    let mut e = task(2.0);
    e.constraint = Constraint::Mfo(d(10, 9) as f64);
    let mut f = task(1.0);
    f.constraint = Constraint::Fnet(d(10, 2) as f64);
    let s = run(&[a, b, c, e, f]);
    let v = days(&s);
    assert_eq!(v[0], (d(10, 7), d(10, 8)));
    // Must start on wins over the link, with a warning.
    assert_eq!(v[1], (d(10, 1), d(10, 1)));
    assert!(s.warnings.contains(&Warning::LinkConflict { task: 1, pred: 0 }));
    // Constraints win over the project start too.
    assert_eq!(v[2], (d(9, 25), d(9, 29)));
    assert_eq!(v[3], (d(10, 8), d(10, 9)));
    assert_eq!(v[4], (d(10, 2), d(10, 2)));
    // Start no later than: slack up to the date.
    let mut g = task(1.0);
    g.constraint = Constraint::Snlt(d(9, 30) as f64);
    let s = run(&[task(10.0), g]);
    assert!((s.tasks[1].slack - 2.0).abs() < 1e-6);
    let mut late = after(1.0, "1");
    late.constraint = Constraint::Snlt(d(9, 30) as f64);
    let s = run(&[task(5.0), late]);
    assert_eq!(s.tasks[1].start_day, d(9, 30));
    assert!(s.warnings.contains(&Warning::LinkConflict { task: 1, pred: 0 }));
}

#[test]
fn summaries_roll_up() {
    // 1 Phase A
    //   2 a1 (2d)
    //   3 a2 (3d, after 2), 50% done
    // 4 Phase B (after 1)
    //   5 b1 (1d)
    //   6 b2 (1d)
    // 7 End (after 4)
    let mut t = vec![task(0.0), task(2.0), after(3.0, "2"), after(0.0, "1"), task(1.0), task(1.0), after(0.0, "4")];
    t[1].level = 1;
    t[2].level = 1;
    t[2].percent = 0.5;
    t[4].level = 1;
    t[5].level = 1;
    let s = run(&t);
    assert!(s.warnings.is_empty(), "{:?}", s.warnings);
    assert!(s.tasks[0].summary && s.tasks[3].summary && !s.tasks[6].summary);
    let v = days(&s);
    assert_eq!(v[0], (d(9, 28), d(10, 2)));
    assert!((s.tasks[0].duration - 5.0).abs() < 1e-6);
    // Weighted by duration: (0*2 + 0.5*3) / 5.
    assert!((s.tasks[0].percent - 0.3).abs() < 1e-6);
    // The link into phase B moves its subtasks.
    assert_eq!(v[4], (d(10, 5), d(10, 5)));
    assert_eq!(v[3], (d(10, 5), d(10, 5)));
    assert_eq!(v[6], (d(10, 5), d(10, 5)));
    let w: Vec<&str> = s.tasks.iter().map(|t| t.wbs.as_str()).collect();
    assert_eq!(w, vec!["1", "1.1", "1.2", "2", "2.1", "2.2", "3"]);
    assert_eq!(s.tasks[2].parent, Some(0));
    assert!(s.tasks[0].critical);
}

#[test]
fn summary_links_to_own_subtasks_are_ignored() {
    let mut t = vec![task(0.0), after(2.0, "1"), after(1.0, "1FF")];
    t[1].level = 1;
    t[2].level = 0;
    // 3 FF into... 3 is not a summary: fine. 2 links to its own summary.
    let s = run(&t);
    assert!(s.warnings.contains(&Warning::SummaryLink { task: 1, pred: 0 }));
    // FF into a summary.
    let mut t = vec![task(1.0), after(0.0, "1FF"), task(2.0)];
    t[2].level = 1;
    let s = run(&t);
    assert!(s.warnings.contains(&Warning::SummaryLink { task: 1, pred: 0 }));
}

#[test]
fn outline_levels_and_numbers() {
    let (l, p) = outline(&[3, 1, 2, 5, 0, 1]);
    assert_eq!(l, vec![0, 1, 2, 3, 0, 1]);
    assert_eq!(p, vec![None, Some(0), Some(1), Some(2), None, Some(4)]);
    assert_eq!(wbs(&l), vec!["1", "1.1", "1.1.1", "1.1.1.1", "2", "2.1"]);
}

#[test]
fn manual_tasks_stay() {
    let mut m = after(2.0, "1");
    m.manual = true;
    m.start = Some(d(9, 29) as f64);
    m.finish = Some(d(10, 6) as f64);
    let s = run(&[task(3.0), m, after(1.0, "2")]);
    let v = days(&s);
    assert_eq!(v[1], (d(9, 29), d(10, 6)));
    assert!(s.warnings.contains(&Warning::LinkConflict { task: 1, pred: 0 }));
    // Its successor follows its dates.
    assert_eq!(v[2], (d(10, 7), d(10, 7)));
}

#[test]
fn cycles_are_reported() {
    let s = run(&[after(1.0, "3"), after(1.0, "1"), after(1.0, "2"), after(1.0, "3")]);
    assert!(s.warnings.iter().any(|w| matches!(w, Warning::Cycle(v) if *v == vec![0, 1, 2])));
    // Still scheduled.
    assert_eq!(s.tasks.len(), 4);
    assert_eq!(s.tasks[0].start_day, d(9, 28));
    assert_eq!(s.tasks[3].start_day, d(9, 29));
    // Bad and self references.
    let s = run(&[after(1.0, "1;9")]);
    assert_eq!(s.warnings.len(), 2);
}

#[test]
fn holidays_and_resource_calendars() {
    let mut c = Calendar::standard("S");
    c.exceptions.push(Exception::holiday("Fiesta", d(10, 12)));
    let mut r = Calendar::standard("Ana");
    r.exceptions.push(Exception { name: "Vacaciones".into(), from: d(10, 13), to: d(10, 14), working: false });
    let cals = vec![c, r];
    let mut t2 = after(3.0, "1");
    t2.calendars = vec![1];
    let s = schedule(&[task(10.0), t2, after(3.0, "1")], &cals, mon());
    let v = days(&s);
    // Ten days: Mon 28 to Fri 9.
    assert_eq!(v[0], (d(9, 28), d(10, 9)));
    // Ana: holiday 12, vacation 13-14: Thu 15 - Mon 19.
    assert_eq!(v[1], (d(10, 15), d(10, 19)));
    // Without Ana: 13 - 15.
    assert_eq!(v[2], (d(10, 13), d(10, 15)));
}

#[test]
fn as_late_as_possible() {
    let mut late = task(1.0);
    late.constraint = Constraint::Alap;
    let s = run(&[task(5.0), late, after(1.0, "1;2")]);
    assert_eq!(s.tasks[1].start_day, d(10, 2));
}

#[test]
fn actual_dates() {
    let mut t = after(3.0, "1");
    t.actual_start = Some(d(9, 29) as f64);
    t.actual_finish = Some(d(10, 5) as f64);
    let s = run(&[task(2.0), t, after(1.0, "2")]);
    let v = days(&s);
    assert_eq!(v[1], (d(9, 29), d(10, 5)));
    assert!((s.tasks[1].duration - 5.0).abs() < 1e-6);
    assert_eq!(v[2], (d(10, 6), d(10, 6)));
}

#[test]
fn project_start_on_weekend() {
    let s = schedule(&[task(1.0), task(0.0)], &[Calendar::standard("S")], d(10, 3) as f64);
    assert_eq!(days(&s), vec![(d(10, 5), d(10, 5)), (d(10, 5), d(10, 5))]);
}

#[test]
fn fractional_durations() {
    let s = run(&[task(0.5), after(0.5, "1"), after(1.0, "2")]);
    assert_eq!(days(&s), vec![(d(9, 28), d(9, 28)), (d(9, 28), d(9, 28)), (d(9, 29), d(9, 29))]);
}

#[test]
fn leveling() {
    // Two tasks of 3 days for the same person, at the same time.
    let tasks = vec![task(3.0), task(3.0), after(1.0, "2")];
    let cals = [Calendar::standard("S")];
    let uses = [Use { task: 0, res: 0, units: 1.0 }, Use { task: 1, res: 0, units: 1.0 }];
    let s = schedule(&tasks, &cals, mon());
    let l = loads(&s, &cals, &[vec![], vec![], vec![]], &uses, 1);
    assert_eq!(overloaded(&l[0], 1.0), vec![d(9, 28), d(9, 29), d(9, 30)]);
    // The one with less slack (2, which has a successor... both have
    // slack 1 vs 0) stays; the other waits.
    let delays = level(&tasks, &cals, mon(), &uses, &[1.0], &[500.0, 500.0, 500.0]);
    let mut t2 = tasks.clone();
    for (t, dl) in t2.iter_mut().zip(&delays) {
        t.delay = *dl;
    }
    let s = schedule(&t2, &cals, mon());
    let l = loads(&s, &cals, &[vec![], vec![], vec![]], &uses, 1);
    assert!(overloaded(&l[0], 1.0).is_empty());
    assert_eq!(s.tasks[1].start_day, d(9, 28));
    assert_eq!(s.tasks[0].start_day, d(10, 1));
    // Priority wins over slack.
    let delays = level(&tasks, &cals, mon(), &uses, &[1.0], &[900.0, 500.0, 500.0]);
    assert!(delays[0] == 0.0 && delays[1] > 0.0);
    // Half time each: no overload.
    let half = [Use { task: 0, res: 0, units: 0.5 }, Use { task: 1, res: 0, units: 0.5 }];
    assert!(level(&tasks, &cals, mon(), &half, &[1.0], &[]).iter().all(|d| *d == 0.0));
    // Manual tasks never move.
    let mut m = tasks.clone();
    m[0].manual = true;
    m[0].start = Some(mon());
    m[1].manual = true;
    m[1].start = Some(mon());
    assert!(level(&m, &cals, mon(), &uses, &[1.0], &[]).iter().all(|d| *d == 0.0));
}

#[test]
fn many_tasks_are_fast() {
    // A chain of 2000 tasks and 2000 parallel ones.
    let mut t = Vec::new();
    for i in 0..2000 {
        t.push(if i == 0 { task(1.0) } else { after(1.0, &i.to_string()) });
    }
    for _ in 0..2000 {
        t.push(after(2.0, "1"));
    }
    let now = web_time::Instant::now();
    let s = run(&t);
    assert!(now.elapsed().as_secs_f64() < 5.0);
    assert!(s.warnings.is_empty());
    assert!(s.tasks[1999].critical);
}
