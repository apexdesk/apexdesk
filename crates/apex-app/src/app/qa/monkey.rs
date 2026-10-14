//! Monkey testing: in each view, sequences of clicks on the controls
//! shown, clicks in the page (cells, slide), keys and typed text, drawn
//! from a fixed seed. None may panic or report an error; undoing
//! everything afterwards brings the first model back; memory must not
//! grow past [`MEMORY`]. A failing sequence is cut down (delta debugging)
//! to the fewest steps that still fail, reported with its seed.

use std::cell::Cell;

use eframe::egui::{self, Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2};

use super::common::{self, Rng, Sink};
use super::model::{self, Model, Qv};
use crate::app::audit::{self, Control, Rig};
use crate::i18n::tr;

/// Memory a sequence may add to the process.
const MEMORY: u64 = 512 << 20;
/// Memory past which a sequence stops at once: something grows without
/// end, and going on would take the machine's memory.
const RUNAWAY: u64 = 2 << 30;
/// Undo steps tried at most when undoing everything.
const MAX_UNDO: usize = 3000;

const VIEWS: [Qv; 8] = [Qv::Doc, Qv::Sheet, Qv::Slides, Qv::Pdf, Qv::Db, Qv::Code, Qv::Note, Qv::Macros];

fn seeds() -> (u64, usize) {
    if common::full() { (20, 500) } else { (3, 150) }
}

/// Replays tried at most to cut a failing sequence down.
fn budget() -> usize {
    if common::full() { 120 } else { 30 }
}

/// Keys pressed, with their modifiers.
const KEYS: &[(Key, Modifiers)] = &[
    (Key::ArrowLeft, Modifiers::NONE),
    (Key::ArrowRight, Modifiers::NONE),
    (Key::ArrowUp, Modifiers::NONE),
    (Key::ArrowDown, Modifiers::NONE),
    (Key::ArrowRight, Modifiers::SHIFT),
    (Key::ArrowDown, Modifiers::SHIFT),
    (Key::Home, Modifiers::NONE),
    (Key::End, Modifiers::SHIFT),
    (Key::PageDown, Modifiers::NONE),
    (Key::Enter, Modifiers::NONE),
    (Key::Tab, Modifiers::NONE),
    (Key::Backspace, Modifiers::NONE),
    (Key::Delete, Modifiers::NONE),
    (Key::Escape, Modifiers::NONE),
    (Key::Z, Modifiers::COMMAND),
    (Key::Y, Modifiers::COMMAND),
    (Key::A, Modifiers::COMMAND),
    (Key::B, Modifiers::COMMAND),
    (Key::I, Modifiers::COMMAND),
    (Key::C, Modifiers::COMMAND),
    (Key::X, Modifiers::COMMAND),
    (Key::V, Modifiers::COMMAND),
    (Key::F2, Modifiers::NONE),
];

const WORDS: &[&str] = &["a", "Hello", "42", "=1+2", "3.5", "x y", "ñandú", "-", "=SUM(B2:B5)", "  ", "Ω"];

/// One thing the monkey does, replayable in a fresh window.
#[derive(Clone, Debug)]
enum Step {
    Click { key: (String, String, usize), what: String },
    /// A click at a point of a larger widget (a page, the cells), as
    /// fractions of it.
    Area { key: (String, String, usize), fx: f32, fy: f32, right: bool },
    Key(Key, Modifiers),
    Text(&'static str),
}

impl Step {
    fn describe(&self) -> String {
        match self {
            Step::Click { what, .. } => format!("click {what}"),
            Step::Area { key, fx, fy, right } => format!("{} {} at {:.2},{:.2}", if *right { "right-click" } else { "click" }, if key.1.is_empty() { &key.0 } else { &key.1 }, fx, fy),
            Step::Key(k, m) => format!("key {}{k:?}", if m.command { "Ctrl+" } else if m.shift { "Shift+" } else { "" }),
            Step::Text(t) => format!("type {t:?}"),
        }
    }
}

/// Controls the monkey leaves alone: they end the view or the window,
/// or change the process (the language).
fn wanted(c: &Control) -> bool {
    c.enabled && !audit::skipped(c) && c.key.1 != "×" && !c.key.1.starts_with(tr("tab.close")) && !c.key.1.starts_with(tr("menu.quit"))
}

/// With APEXDESK_QA_TRACE set to a file, each step goes there before it
/// is done: what a killed process was doing last.
fn trace(line: &str) {
    use std::io::Write;
    let Some(p) = std::env::var_os("APEXDESK_QA_TRACE") else { return };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
        let _ = writeln!(f, "{line}");
    }
}

/// A step drawn for the window as it is now.
fn draw(rig: &Rig, rng: &mut Rng) -> Step {
    let r = rng.below(100);
    let controls: Vec<&Control> = rig.controls.iter().filter(|c| wanted(c)).collect();
    if r < 45 && !controls.is_empty() {
        let c = controls[rng.below(controls.len())];
        return Step::Click { key: c.key.clone(), what: c.describe() };
    }
    if r < 62 && !rig.areas.is_empty() {
        let a = &rig.areas[rng.below(rig.areas.len())];
        return Step::Area { key: a.key.clone(), fx: rng.unit(), fy: rng.unit(), right: rng.below(8) == 0 };
    }
    if r < 85 {
        let (k, m) = KEYS[rng.below(KEYS.len())];
        return Step::Key(k, m);
    }
    Step::Text(WORDS[rng.below(WORDS.len())])
}

/// Presses at `at`, then lets the window settle a little.
fn press(rig: &mut Rig, at: Pos2, button: PointerButton) {
    let ev = |pressed| Event::PointerButton { pos: at, button, pressed, modifiers: Modifiers::NONE };
    rig.frame(vec![Event::PointerMoved(at)]);
    rig.frame(vec![ev(true)]);
    rig.frame(vec![ev(false)]);
    rig.frame(vec![Event::PointerGone]);
    after(rig);
}

/// A few frames for what a step started (a popup, a relayout).
fn after(rig: &mut Rig) {
    for _ in 0..3 {
        if !rig.frame(Vec::new()) {
            break;
        }
    }
}

fn screen() -> Rect {
    Rect::from_min_size(Pos2::ZERO, egui::vec2(1600.0, 1400.0))
}

/// Does `step`; false when what it acts on isn't there.
fn act(rig: &mut Rig, step: &Step) -> bool {
    match step {
        Step::Click { key, .. } => {
            let Some(c) = rig.find(key) else { return false };
            let at = c.rect.center();
            if screen().contains(at) {
                press(rig, at, PointerButton::Primary);
            } else {
                rig.reach(&c);
            }
        }
        Step::Area { key, fx, fy, right } => {
            let Some(a) = rig.areas.iter().find(|a| &a.key == key).cloned() else { return false };
            let at = a.rect.intersect(screen()).lerp_inside(egui::vec2(*fx, *fy));
            press(rig, pos2(at.x, at.y), if *right { PointerButton::Secondary } else { PointerButton::Primary });
        }
        Step::Key(k, m) => {
            let ev = |pressed| Event::Key { key: *k, physical_key: None, pressed, repeat: false, modifiers: *m };
            rig.frame(vec![ev(true)]);
            rig.frame(vec![ev(false)]);
            after(rig);
        }
        Step::Text(t) => {
            rig.frame(vec![Event::Text(t.to_string())]);
            after(rig);
        }
    }
    true
}

/// How a sequence failed.
#[derive(Clone, Debug, PartialEq)]
enum Failure {
    Panic(String),
    Error(String),
    /// Undoing everything leaves these parts changed.
    UndoAll(Vec<String>),
    Memory(u64),
}

impl Failure {
    /// What tells this failure from others, the same from run to run.
    fn signature(&self) -> String {
        match self {
            // The place, without the message (it may name values).
            Failure::Panic(p) => format!("panic at {}", p.rsplit(" at ").next().unwrap_or(p)),
            Failure::Error(e) => format!("error \"{}\"", e.chars().filter(|c| !c.is_ascii_digit()).take(80).collect::<String>()),
            // The kind of part (a paragraph's format, not which paragraph).
            Failure::UndoAll(parts) => {
                let mut kinds: Vec<String> = parts.iter().map(|p| p.chars().filter(|c| !c.is_ascii_digit()).collect()).collect();
                kinds.dedup();
                format!("undo-all leaves {}", kinds.join(", "))
            }
            Failure::Memory(_) => "memory".into(),
        }
    }

    fn same(&self, other: &Failure) -> bool {
        self.signature() == other.signature()
    }
}

/// What a run found: the steps done, each failure with the step it came
/// at (the number of steps for what is checked after them all), and
/// whether the model had changed before undoing everything.
struct Run {
    steps: Vec<Step>,
    failures: Vec<(usize, Failure)>,
    edited: bool,
}

/// Runs a sequence in a fresh window of `qv`: `given` steps, or else
/// `len` steps drawn from `seed` as it goes. An error doesn't stop it;
/// a panic does.
fn run(qv: Qv, seed: u64, given: Option<&[Step]>, len: usize, memory: bool) -> Run {
    let mut rng = Rng::new(seed);
    let steps = std::cell::RefCell::new(Vec::new());
    let failures = std::cell::RefCell::new(Vec::new());
    let edited = Cell::new(false);
    let r = audit::quietly(|| {
        let mut rig = Rig::open(false, |app| qv.setup(app, seed));
        let id = rig.app.tabs[rig.app.active].id;
        let first = model::model(&rig.app, id).unwrap_or_default();
        common::take_errors();
        let rss = common::rss();
        let len = given.map_or(len, <[Step]>::len);
        for i in 0..len {
            let s = match given {
                Some(g) => g[i].clone(),
                None => draw(&rig, &mut rng),
            };
            steps.borrow_mut().push(s.clone());
            audit::watch(Some(format!("{qv:?} seed {seed}: step {i} {}", s.describe())));
            trace(&format!("{qv:?} seed {seed} step {i} rss {} MB: {}", common::rss() >> 20, s.describe()));
            act(&mut rig, &s);
            for e in common::take_errors() {
                failures.borrow_mut().push((i, Failure::Error(e)));
            }
            let grew = common::rss().saturating_sub(rss);
            if grew > RUNAWAY {
                failures.borrow_mut().push((i, Failure::Memory(grew)));
                return;
            }
        }
        let grew = common::rss().saturating_sub(rss);
        edited.set(model::model(&rig.app, id).is_some_and(|m| m != first));
        // What was deleted for good can't come back, and the app says so.
        if !model::irreversible(&rig.app, id)
            && let Some(parts) = undo_all(&mut rig, id, &first)
        {
            failures.borrow_mut().push((len, Failure::UndoAll(parts)));
        }
        if memory && grew > MEMORY {
            failures.borrow_mut().push((len, Failure::Memory(grew)));
        }
    });
    let steps = steps.into_inner();
    let mut failures = failures.into_inner();
    if let Err(e) = r {
        audit::drop_context();
        crate::toolstrip::test::reset();
        failures.push((steps.len().saturating_sub(1), Failure::Panic(audit::panic_text(&e))));
    }
    Run { steps, failures, edited: edited.get() }
}

/// Undoes everything in tab `id`; the parts still not as in `first`
/// (none when the tab is closed: nothing to compare).
fn undo_all(rig: &mut Rig, id: u64, first: &Model) -> Option<Vec<String>> {
    // Out of any text field or dialog first, as a user would.
    for _ in 0..2 {
        rig.frame(vec![Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
    }
    rig.frame(Vec::new());
    let mut still = 0;
    for _ in 0..MAX_UNDO {
        let before = model::model(&rig.app, id)?;
        match model::can_undo(&rig.app, id) {
            Some(false) => break,
            Some(true) => model::undo(&mut rig.app, id, false),
            // It can't tell: until undoing changes nothing any more.
            None => {
                model::undo(&mut rig.app, id, false);
                still = if model::model(&rig.app, id).as_ref() == Some(&before) { still + 1 } else { 0 };
                if still >= 5 {
                    break;
                }
            }
        }
    }
    rig.frame(Vec::new());
    let now = model::model(&rig.app, id)?;
    // Out of the code view: the document is what is compared.
    let parts: Vec<String> = first.diff(&now).into_iter().filter(|p| p != "code" || now.0.contains_key("code")).collect();
    (!parts.is_empty()).then_some(parts)
}

/// Cuts `steps` down to fewer that still fail like `f` (ddmin, within
/// the replay budget).
fn minimize(qv: Qv, steps: Vec<Step>, f: &Failure, seed: u64) -> Vec<Step> {
    let tries = Cell::new(0);
    let fails = |s: &[Step]| -> bool {
        tries.set(tries.get() + 1);
        audit::watch(Some(format!("{qv:?} seed {seed}: cutting down ({} steps)", s.len())));
        run(qv, seed, Some(s), 0, false).failures.iter().any(|(_, g)| g.same(f))
    };
    let mut cur = steps;
    let mut n = 2;
    while cur.len() >= 2 && tries.get() < budget() {
        let chunk = cur.len().div_ceil(n);
        let mut cut = false;
        // Each piece alone, then everything but each piece.
        for i in 0..n {
            let (a, b) = (i * chunk, ((i + 1) * chunk).min(cur.len()));
            if a >= b {
                continue;
            }
            if tries.get() >= budget() {
                break;
            }
            let piece = cur[a..b].to_vec();
            if fails(&piece) {
                cur = piece;
                n = 2;
                cut = true;
                break;
            }
            let mut rest = cur[..a].to_vec();
            rest.extend_from_slice(&cur[b..]);
            if tries.get() < budget() && fails(&rest) {
                cur = rest;
                n = (n - 1).max(2);
                cut = true;
                break;
            }
        }
        if !cut {
            if n >= cur.len() {
                break;
            }
            n = (n * 2).min(cur.len());
        }
    }
    cur
}

fn run_view(job: &str, sink: &mut Sink) {
    audit::panic_hook();
    let Some(qv) = Qv::parse(job) else { return };
    let (seeds, len) = seeds();
    let rss0 = common::rss();
    let mut edits = 0;
    // APEXDESK_QA_SEED: only that sequence (to follow one up).
    let only: Option<u64> = std::env::var("APEXDESK_QA_SEED").ok().and_then(|v| v.parse().ok());
    for seed in (1..=seeds).filter(|s| only.is_none_or(|o| o == *s)) {
        let found = run(qv, seed, None, len, true);
        sink.check();
        edits += usize::from(found.edited);
        let mut reported: Vec<String> = Vec::new();
        for (at, f) in &found.failures {
            // One report for each kind of failure of the sequence.
            if reported.contains(&f.signature()) {
                continue;
            }
            reported.push(f.signature());
            // What came after the failing step plays no part in it.
            let steps = found.steps[..(*at + 1).min(found.steps.len())].to_vec();
            let total = steps.len();
            let small = if matches!(f, Failure::Memory(_)) { steps } else { minimize(qv, steps, f, seed) };
            let list: Vec<String> = small.iter().enumerate().map(|(i, s)| format!("{}. {}", i + 1, s.describe())).collect();
            let (rule, detail) = match f {
                Failure::Panic(p) => ("panic", format!("panic: {p}")),
                Failure::Error(e) => ("error", format!("error shown: {e}")),
                Failure::UndoAll(p) => ("undo-all", format!("undoing everything leaves {} changed", model::brief(p))),
                Failure::Memory(b) => ("memory", format!("memory grew {} MB (limit {} MB)", b >> 20, MEMORY >> 20)),
            };
            sink.fail(common::id(&[job, &f.signature()]), format!("[{rule}] {job}: {detail}. Repro: seed {seed}, {total} steps cut to {}: {}", small.len(), list.join("; ")));
        }
    }
    audit::watch(None);
    sink.note(format!("{seeds} seeds × {len} steps, {edits} of them edited the model; process memory {} → {} MB", rss0 >> 20, common::rss() >> 20));
}

#[test]
#[ignore]
fn qa_monkey() {
    let jobs: Vec<String> = VIEWS.iter().map(|q| q.name()).collect();
    common::run("monkey", "app::qa::monkey::qa_monkey", &jobs, run_view);
}
