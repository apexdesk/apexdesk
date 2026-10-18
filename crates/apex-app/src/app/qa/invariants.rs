//! Invariants of every control: in each view, on a sample of medium size,
//! every control of every toolstrip tab (and, in the full run, what they
//! open) is clicked from a fresh window, and then:
//! - undo brings the model back to its print before the click, and redo
//!   to the one after it (controls that change no model are left out);
//! - a toggle clicked twice leaves the window as it was (or, when it
//!   edits, the model);
//! - a format applied to a selection of several paragraphs, cells or
//!   objects reaches all of them;
//! - the click takes less than [`SLOW`];
//! - (full run) the change survives saving in each native format of the
//!   view and reading the file back.

use std::collections::HashSet;
use web_time::{Duration, Instant};

use eframe::egui::{Event, Modifiers, PointerButton};

use super::common::{self, Sink};
use super::model::{self, Model, Qv, Snapshot};
use crate::app::audit::{self, Control, Reach, Rig};

/// An action slower than this is reported (the frames of the click and
/// the one after it, in the test build).
const SLOW: Duration = Duration::from_millis(250);

fn open(qv: Qv, n: u64) -> Rig {
    Rig::open(false, |app| qv.setup(app, n))
}

/// Whether the invariants check `c` at all.
pub(super) fn wanted(c: &Control) -> bool {
    // The tab's close cross ends the view; the language changes the process.
    c.enabled && !audit::skipped(c) && c.key.1 != "×"
}

/// What to click in `qv`: each control shown, and what each of them
/// shows (a toolstrip tab's buttons, a menu's entries), down to two
/// clicks (three in the full run).
pub(super) fn actions(qv: Qv, open: fn(Qv, u64) -> Rig) -> Vec<Vec<Control>> {
    let depth = if common::full() { 3 } else { 2 };
    let rig = open(qv, 0);
    let base = rig.controls.clone();
    drop(rig);
    let mut seen: HashSet<_> = base.iter().map(Control::id).collect();
    let mut all: Vec<Vec<Control>> = Vec::new();
    let mut level: Vec<Vec<Control>> = base.iter().filter(|c| wanted(c)).map(|c| vec![c.clone()]).collect();
    for d in 1..=depth {
        all.extend(level.iter().cloned());
        if d == depth {
            break;
        }
        let mut next = Vec::new();
        for p in &level {
            audit::watch(Some(format!("{qv:?}: finding the controls under {}", describe(p))));
            let got = common_quietly(|| {
                let mut rig = open(qv, 0);
                for step in p {
                    let c = rig.find(&step.key)?;
                    if rig.reach(&c) == Reach::Missed {
                        return None;
                    }
                }
                Some(rig.controls.clone())
            });
            let fresh: Vec<Control> = got.flatten().unwrap_or_default().into_iter().filter(|c| wanted(c) && seen.insert(c.id())).collect();
            // A drop-down's choices are alike (fonts, sizes): the first
            // ones and the last.
            let n = fresh.len();
            let combo = p.last().is_some_and(|l| l.key.0 == "ComboBox");
            for (i, c) in fresh.into_iter().enumerate() {
                if combo && i >= 3 && i + 1 < n {
                    continue;
                }
                let mut q = p.clone();
                q.push(c);
                next.push(q);
            }
        }
        level = next;
    }
    all
}

/// Borders go on the edges of a selection (the bottom border on its last
/// row): they rightly change only some of its cells.
fn edges(c: &Control) -> bool {
    c.key.1.to_lowercase().contains("border")
}

/// Runs `f`, a panic in it caught (and the harness left clean).
pub(super) fn common_quietly<R>(f: impl FnOnce() -> R) -> Option<R> {
    match audit::quietly(f) {
        Ok(r) => Some(r),
        Err(_) => {
            audit::drop_context();
            crate::toolstrip::test::reset();
            None
        }
    }
}

/// Clicks `c` like the audit does, timing the frames of the click and the
/// one after it; none when it had to be clicked another way.
fn timed_click(rig: &mut Rig, c: &Control) -> Option<Duration> {
    let Some(at) = rig.aim(c) else {
        rig.reach(c);
        return None;
    };
    let press = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    let t = Instant::now();
    rig.frame(vec![press(true)]);
    rig.frame(vec![press(false)]);
    rig.frame(vec![Event::PointerGone]);
    let took = t.elapsed();
    rig.settle();
    Some(took)
}

/// Where two texts part, a little of each from there.
fn first_difference(a: &str, b: &str) -> String {
    let at = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count();
    let from = at.saturating_sub(20);
    let piece = |s: &str| s.chars().skip(from).take(50).collect::<String>();
    format!("was …{}…, now …{}…", piece(a), piece(b))
}

pub(super) fn describe(path: &[Control]) -> String {
    path.iter().map(Control::describe).collect::<Vec<_>>().join(" → ")
}

/// What one path found: (rule, detail) failures, checks made, the time.
#[derive(Default)]
struct Found {
    fails: Vec<(&'static str, String)>,
    checks: usize,
    took: Option<Duration>,
    /// The model changed.
    edits: bool,
}

fn check_path(qv: Qv, path: &[Control], n: u64, tmp: &std::path::Path) -> Result<Option<Found>, String> {
    let r = audit::quietly(|| {
        let mut rig = open(qv, n);
        let id = rig.app.tabs[rig.app.active].id;
        let (last, before) = path.split_last().unwrap();
        for step in before {
            let Some(c) = rig.find(&step.key) else { return None };
            if rig.reach(&c) == Reach::Missed {
                return None;
            }
        }
        let c = rig.find(&last.key)?;
        let mut f = Found::default();
        let m0 = model::model(&rig.app, id)?;
        let e0 = model::elements(&rig.app, id, qv);
        let snap0 = if common::full() { Snapshot::take(&rig.app, id) } else { None };
        let c = if c.toggle {
            // What the window was doing (looking for printers, searching)
            // done first: it isn't the toggle's. So is scrolling to it (a
            // check box low in a dialog): the second click finds it shown.
            apex_fs::thread::sleep(Duration::from_millis(300));
            rig.settle();
            rig.shown(&c)
        } else {
            c
        };
        let u0 = rig.print();
        f.took = timed_click(&mut rig, &c);
        f.checks += 1;
        if f.took.is_some_and(|t| t > SLOW) {
            f.fails.push(("time", format!("took {} ms (limit {} ms)", f.took.unwrap().as_millis(), SLOW.as_millis())));
        }
        let Some(m1) = model::model(&rig.app, id) else { return Some(f) };
        let e1 = model::elements(&rig.app, id, qv);
        let snap1 = if common::full() { Snapshot::take(&rig.app, id) } else { None };
        // Leaving the code view (for another view) edits nothing: the
        // code is no longer there to compare.
        let m0 = if m0.0.contains_key("code") && !m1.0.contains_key("code") { m0.only(|k| k != "code") } else { m0 };
        f.edits = m1 != m0;
        if f.edits {
            // Undo and redo.
            f.checks += 2;
            model::undo(&mut rig.app, id, false);
            rig.settle();
            match model::model(&rig.app, id) {
                Some(m) if m != m0 => f.fails.push(("undo", format!("undo leaves {} changed", model::brief(&m0.diff(&m))))),
                _ => {}
            }
            model::undo(&mut rig.app, id, true);
            rig.settle();
            match model::model(&rig.app, id) {
                Some(m) if m != m1 => f.fails.push(("redo", format!("redo gives {} different from after the click", model::brief(&m1.diff(&m))))),
                _ => {}
            }
            // A format reaches all the selection, or none of it: an action
            // that changed only selected elements, and none's text.
            if let (Some(a), Some(b)) = (e0, e1) {
                let same_text = a.sel.iter().zip(&b.sel).all(|(x, y)| x.0 == y.0);
                let changed = a.sel.iter().zip(&b.sel).filter(|(x, y)| x.1 != y.1).count();
                if a.sel.len() == b.sel.len() && a.rest == b.rest && a.sel.len() > 1 && same_text && !edges(&c) {
                    f.checks += 1;
                    if changed > 0 && changed < a.sel.len() {
                        f.fails.push(("selection", format!("changed {changed} of the {} selected elements", a.sel.len())));
                    }
                }
            }
            // Saved and read back.
            if let (Some(s0), Some(s1)) = (&snap0, &snap1) {
                persist(s0, s1, &m0, &m1, tmp, &mut f);
            }
        }
        // A toggle clicked again goes back; one that stays on (a tab, a
        // choice among several) isn't a toggle.
        if c.toggle && model::model(&rig.app, id).as_ref() == Some(&m1) {
            if let Some(again) = rig.find(&last.key).filter(|a| a.enabled) {
                rig.reach(&again);
                let still_on = again.on && rig.find(&last.key).is_some_and(|a| a.on);
                let u2 = rig.print();
                match model::model(&rig.app, id) {
                    _ if still_on => {}
                    // Clicked again, it changed nothing more: a choice.
                    Some(m2) if f.edits && m2 == m1 => {}
                    Some(m2) if f.edits && m2 != m0 => {
                        f.checks += 1;
                        f.fails.push(("toggle", format!("clicked twice, the model isn't back: {}", model::brief(&m0.diff(&m2)))));
                    }
                    Some(_) if !f.edits => {
                        f.checks += 1;
                        // A clock (the presenter's) keeps going by itself.
                        let clock = regex::Regex::new(r"\d+:\d\d(:\d\d)?").unwrap();
                        let still = |t: &std::collections::BTreeSet<String>| t.iter().map(|s| clock.replace_all(s, "#").into_owned()).collect::<std::collections::BTreeSet<_>>();
                        let (t0, t2) = (still(&u0.tree), still(&u2.tree));
                        if !(t0 == t2 && u0.settings == u2.settings && u0.app == u2.app) {
                            let gone: Vec<String> = t0.difference(&t2).take(2).cloned().collect();
                            let came: Vec<String> = t2.difference(&t0).take(2).cloned().collect();
                            let what = if t0 != t2 {
                                format!("controls: was {gone:?}, now {came:?}")
                            } else if u0.settings != u2.settings {
                                "the settings".to_string()
                            } else {
                                format!("the window's state: {}", first_difference(&u0.app, &u2.app))
                            };
                            f.fails.push(("toggle", format!("clicked twice, the window isn't back ({what})")));
                        }
                    }
                    _ => f.checks += 1,
                }
            }
        }
        Some(f)
    });
    r.map_err(|e| {
        audit::drop_context();
        crate::toolstrip::test::reset();
        audit::panic_text(&e)
    })
}

/// The change from `s0` to `s1` is still there once saved in each native
/// format and read back (compared with `s0` saved and read back), in the
/// parts the format can hold.
fn persist(s0: &Snapshot, s1: &Snapshot, m0: &Model, m1: &Model, tmp: &std::path::Path, f: &mut Found) {
    for (fmt, holds) in s1.formats() {
        let changed = m0.only(holds).diff(&m1.only(holds));
        if changed.is_empty() {
            continue;
        }
        f.checks += 1;
        let (b, a) = (s0.round_trip(fmt, tmp, "before"), s1.round_trip(fmt, tmp, "after"));
        match (b, a) {
            (Ok(b), Ok(a)) => {
                let lost = !changed.iter().any(|k| b.0.get(k) != a.0.get(k)) && b.only(holds) == a.only(holds);
                if lost {
                    f.fails.push(("persist", format!("{fmt:?} loses the change ({})", model::brief(&changed))));
                }
            }
            (_, Err(e)) | (Err(e), _) => f.fails.push(("persist", format!("{fmt:?}: {e}"))),
        }
    }
}

fn run_view(job: &str, sink: &mut Sink) {
    audit::panic_hook();
    let Some(qv) = Qv::parse(job) else { return };
    let tmp = apex_fs::temp_dir().join("qa-invariants");
    let _ = apex_fs::create_dir_all(&tmp);
    audit::watch(Some(format!("{job}: finding the controls")));
    let paths = actions(qv, open);
    let mut slow: Vec<(Duration, String)> = Vec::new();
    let mut edits = 0;
    for (i, path) in paths.iter().enumerate() {
        let what = describe(path);
        audit::watch(Some(format!("{job}: {what}")));
        let repro = format!("open the {job} sample, click {what}");
        match check_path(qv, path, 100 + i as u64, &tmp) {
            Err(e) => {
                sink.check();
                sink.fail(common::id(&[job, "panic", &what]), format!("[panic] {job}: {what} — {e}. Repro: {repro}"));
            }
            Ok(None) => {}
            Ok(Some(f)) => {
                for _ in 0..f.checks {
                    sink.check();
                }
                edits += usize::from(f.edits);
                if let Some(t) = f.took {
                    slow.push((t, what.clone()));
                }
                for (rule, detail) in f.fails {
                    let how = match rule {
                        "undo" => ", then Undo",
                        "redo" => ", then Undo and Redo",
                        "toggle" => " twice",
                        "persist" => ", save and open again",
                        _ => "",
                    };
                    sink.fail(common::id(&[job, rule, &what]), format!("[{rule}] {job}: {what} — {detail}. Repro: {repro}{how}"));
                }
            }
        }
    }
    audit::watch(None);
    slow.sort_by(|a, b| b.0.cmp(&a.0));
    let top: Vec<String> = slow.iter().take(3).map(|(t, w)| format!("{} ms {w}", t.as_millis())).collect();
    sink.note(format!("{} controls clicked, {edits} changed the model; slowest: {}", paths.len(), top.join("; ")));
    let _ = apex_fs::remove_dir_all(&tmp);
}

#[test]
#[ignore]
fn qa_invariants() {
    let jobs: Vec<String> = model::ALL.iter().map(|q| q.name()).collect();
    common::run("invariants", "app::qa::invariants::qa_invariants", &jobs, run_view);
}
