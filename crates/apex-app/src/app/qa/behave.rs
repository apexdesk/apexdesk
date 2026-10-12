//! Behaviour, as a person testing the window sees it: in each view, in a
//! window of a laptop's size, every control (and what it shows, as the
//! invariants click) is clicked from a fresh window, and then:
//! - **drift**: left alone, the window keeps moving (a panel a little
//!   wider every frame, a dialog creeping);
//! - **inert**: a tool picked (what dragging on the page does) does
//!   nothing when dragged on the page;
//! - **jump**: using a tool on the page moves the whole view (the scroll
//!   or the zoom jumps);
//! - **stuck**: a tool its button, clicked again, doesn't let go (nor
//!   Escape), or doesn't once text was selected while it was on;
//! - **cut**: a dialog it opens doesn't fit in the window;
//! - **esc**: a dialog it opens doesn't close with Escape;
//! - **error**: an error reported.
//!
//! The results of tasks done step by step are checked in `stories`.

use std::collections::HashMap;
use std::time::Duration;

use eframe::egui::{self, Color32, ColorImage, Event, Key, Modifiers, PointerButton, Pos2, Rect, Vec2, pos2, vec2};

use super::super::View;
use super::common::{self, Sink};
use super::invariants::{actions, describe};
use super::model::{self, Qv};
use super::raster;
use crate::app::audit::{self, Control, Reach, Rig};

/// A laptop's window: panels that grow find its edge sooner.
pub(super) const SCREEN: Vec2 = vec2(1280.0, 800.0);

/// Frames left alone to tell whether the window keeps moving.
const IDLE: usize = 30;

pub(super) fn open(qv: Qv, n: u64) -> Rig {
    let mut rig = Rig::open_sized(false, SCREEN, |app| qv.setup(app, n));
    ready(&mut rig);
    rig
}

/// Waits for what the view reads in the background (a PDF's text).
pub(super) fn ready(rig: &mut Rig) {
    for _ in 0..250 {
        let done = match rig.app.tabs.get(rig.app.active).map(|t| &t.view) {
            Some(View::Pdf(v)) => v.audit_text_ready(),
            _ => true,
        };
        if done {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
        rig.frame(Vec::new());
    }
    rig.settle();
}

/// The tool armed in the view shown ("" where there are none).
pub(super) fn tool(rig: &Rig) -> String {
    match rig.app.tabs.get(rig.app.active).map(|t| &t.view) {
        Some(View::Doc(v)) => v.audit_tool(),
        Some(View::Sheet(v)) => v.audit_tool(),
        Some(View::Slides(v)) => v.audit_tool(),
        Some(View::Pdf(v)) => v.audit_tool(),
        _ => String::new(),
    }
}

/// The place of the page (the slide, the cells): the largest of what
/// takes clicks that isn't a control.
pub(super) fn content(rig: &Rig) -> Rect {
    let screen = Rect::from_min_size(Pos2::ZERO, SCREEN);
    rig.areas
        .iter()
        .map(|a| a.rect.intersect(screen))
        .filter(|r| r.is_positive() && r.area() > 0.12 * screen.area())
        .max_by(|a, b| a.area().total_cmp(&b.area()))
        .unwrap_or(Rect::from_min_max(pos2(240.0, 170.0), pos2(SCREEN.x - 40.0, SCREEN.y - 40.0)))
}

// --- what a person does -----------------------------------------------------------------

fn button(pos: Pos2, pressed: bool, modifiers: Modifiers) -> Event {
    Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers }
}

/// A press at `from`, the pointer moved to `to` in a few steps, and the
/// release there.
pub(super) fn drag(rig: &mut Rig, from: Pos2, to: Pos2) {
    drag_with(rig, from, to, Modifiers::NONE);
}

pub(super) fn drag_with(rig: &mut Rig, from: Pos2, to: Pos2, m: Modifiers) {
    rig.frame(vec![Event::PointerMoved(from)]);
    rig.frame(vec![button(from, true, m)]);
    for i in 1..=6 {
        rig.frame(vec![Event::PointerMoved(from + (to - from) * (i as f32 / 6.0))]);
    }
    rig.frame(vec![button(to, false, m)]);
    rig.frame(vec![Event::PointerGone]);
    rig.settle();
}

/// A click at `at`, `n` times (two: a double click).
pub(super) fn click_at(rig: &mut Rig, at: Pos2, n: usize) {
    rig.frame(vec![Event::PointerMoved(at)]);
    for _ in 0..n {
        rig.frame(vec![button(at, true, Modifiers::NONE)]);
        rig.frame(vec![button(at, false, Modifiers::NONE)]);
    }
    rig.frame(vec![Event::PointerGone]);
    rig.settle();
}

pub(super) fn key(rig: &mut Rig, k: Key, m: Modifiers) {
    let ev = |pressed| Event::Key { key: k, physical_key: None, pressed, repeat: false, modifiers: m };
    rig.frame(vec![ev(true)]);
    rig.frame(vec![ev(false)]);
    rig.settle();
}

pub(super) fn type_text(rig: &mut Rig, s: &str) {
    rig.frame(vec![Event::Text(s.to_string())]);
    rig.settle();
}

// --- what a person sees -----------------------------------------------------------------

/// The window as drawn by the last frame.
pub(super) fn picture(rig: &Rig) -> ColorImage {
    raster::render(rig, SCREEN)
}

/// Saves `img` as `<name>.png` in `$APEXDESK_QA_SHOTS`, when set (to see
/// what a check saw).
pub(super) fn shot(img: &ColorImage, name: &str) {
    if let Some(d) = std::env::var_os("APEXDESK_QA_SHOTS")
        && let Some(bytes) = crate::charts::encode_png(img)
    {
        let _ = std::fs::write(std::path::Path::new(&d).join(format!("{name}.png")), bytes);
    }
}

fn px(img: &ColorImage, x: usize, y: usize) -> Color32 {
    img.pixels[y * img.size[0] + x]
}

fn differs(a: Color32, b: Color32) -> bool {
    a.r().abs_diff(b.r()).max(a.g().abs_diff(b.g())).max(a.b().abs_diff(b.b())) > 24
}

/// Dark and grey: the ink of text (not a red squiggle, a blue link).
fn dark(c: Color32) -> bool {
    let (hi, lo) = (c.r().max(c.g()).max(c.b()), c.r().min(c.g()).min(c.b()));
    (c.r() as u32 * 3 + c.g() as u32 * 6 + c.b() as u32) / 10 < 110 && hi - lo < 60
}

/// The pixels of `r` (within the picture) as a range.
fn span(img: &ColorImage, r: Rect) -> (std::ops::Range<usize>, std::ops::Range<usize>) {
    let (w, h) = (img.size[0] as f32, img.size[1] as f32);
    let x = r.min.x.clamp(0.0, w) as usize..r.max.x.clamp(0.0, w) as usize;
    let y = r.min.y.clamp(0.0, h) as usize..r.max.y.clamp(0.0, h) as usize;
    (x, y)
}

/// The share of the pixels of `r`, out of `except`, that differ.
pub(super) fn changed(a: &ColorImage, b: &ColorImage, r: Rect, except: Option<Rect>) -> f32 {
    let (xs, ys) = span(a, r);
    let (mut n, mut d) = (0usize, 0usize);
    for y in ys {
        for x in xs.clone() {
            if except.is_some_and(|e| e.contains(pos2(x as f32 + 0.5, y as f32 + 0.5))) {
                continue;
            }
            n += 1;
            d += usize::from(differs(px(a, x, y), px(b, x, y)));
        }
    }
    if n == 0 { 0.0 } else { d as f32 / n as f32 }
}

/// Bands of rows of `r` where `hit` holds for some pixels: their boxes,
/// top to bottom (rows one apart join).
fn bands(img: &ColorImage, r: Rect, min: usize, hit: impl Fn(usize, usize) -> bool) -> Vec<Rect> {
    let (xs, ys) = span(img, r);
    let mut out: Vec<Rect> = Vec::new();
    let mut last = None;
    for y in ys {
        let hits: Vec<usize> = xs.clone().filter(|&x| hit(x, y)).collect();
        if hits.len() < min {
            continue;
        }
        let row = Rect::from_min_max(pos2(hits[0] as f32, y as f32), pos2(*hits.last().unwrap() as f32 + 1.0, y as f32 + 1.0));
        match (last, out.last_mut()) {
            (Some(l), Some(b)) if y <= l + 2 => *b = b.union(row),
            _ => out.push(row),
        }
        last = Some(y);
    }
    out
}

fn light(c: Color32) -> bool {
    c.r() > 215 && c.g() > 215 && c.b() > 215
}

/// The longest run of `ok` among `0..n`.
fn longest(n: usize, ok: impl Fn(usize) -> bool) -> Option<(usize, usize)> {
    let (mut best, mut start): (Option<(usize, usize)>, Option<usize>) = (None, None);
    for i in 0..=n {
        match (i < n && ok(i), start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                if best.is_none_or(|(a, b)| i - s > b - a) {
                    best = Some((s, i));
                }
                start = None;
            }
            _ => {}
        }
    }
    best
}

/// The paper shown in `r` (a page, a slide): the widest run of columns
/// mostly light, as tall as its rows are mostly light.
pub(super) fn paper(img: &ColorImage, r: Rect) -> Option<Rect> {
    let (xs, ys) = span(img, r);
    let col = |x: usize| ys.clone().filter(|&y| light(px(img, x, y))).count() * 2 > ys.len();
    let (x0, x1) = longest(xs.len(), |i| col(xs.start + i))?;
    let (x0, x1) = (xs.start + x0, xs.start + x1);
    if x1 - x0 < 100 {
        return None;
    }
    let row = |y: usize| (x0..x1).filter(|&x| light(px(img, x, y))).count() * 2 > x1 - x0;
    // From the first row mostly light to the last (a line of text may
    // be darker than light).
    let y0 = ys.clone().position(row)?;
    let y1 = ys.len() - ys.clone().rev().position(row)?;
    Some(Rect::from_min_max(pos2(x0 as f32, (ys.start + y0) as f32), pos2(x1 as f32, (ys.start + y1) as f32)))
}

/// The lines of text of `r` as they show: bands of dark ink.
pub(super) fn ink_lines(img: &ColorImage, r: Rect) -> Vec<Rect> {
    bands(img, r, 2, |x, y| dark(px(img, x, y)))
}

/// The bands of `r` where `a` and `b` differ.
pub(super) fn changed_bands(a: &ColorImage, b: &ColorImage, r: Rect) -> Vec<Rect> {
    changed_bands_except(a, b, r, &[])
}

/// [`changed_bands`] leaving out the pixels of `except` (what came over
/// the page: a floating toolbar).
pub(super) fn changed_bands_except(a: &ColorImage, b: &ColorImage, r: Rect, except: &[Rect]) -> Vec<Rect> {
    bands(a, r, 3, |x, y| differs(px(a, x, y), px(b, x, y)) && !except.iter().any(|e| e.contains(pos2(x as f32 + 0.5, y as f32 + 0.5))))
}

/// Where controls show now that didn't before (a floating toolbar, a
/// popup), with room for their frame and shadow.
pub(super) fn new_controls(rig: &Rig, before: &std::collections::HashSet<(String, String, usize)>) -> Vec<Rect> {
    rig.controls.iter().filter(|c| !before.contains(&c.key)).map(|c| c.rect.expand(24.0)).collect()
}

pub(super) fn control_keys(rig: &Rig) -> std::collections::HashSet<(String, String, usize)> {
    rig.controls.iter().map(|c| c.key.clone()).collect()
}

// --- the checks ---------------------------------------------------------------------------

fn layout(rig: &Rig) -> HashMap<(String, String, usize), Rect> {
    rig.controls.iter().map(|c| (c.key.clone(), c.bounds)).collect()
}

/// Whether the window, left alone, keeps moving: what moved.
pub(super) fn drift(rig: &mut Rig) -> Option<String> {
    let a = layout(rig);
    for _ in 0..IDLE {
        rig.frame(Vec::new());
    }
    let b = layout(rig);
    let mut moved: Vec<String> = a
        .iter()
        .filter_map(|(k, r)| {
            let q = b.get(k)?;
            let d = (r.min - q.min).length().max((r.max - q.max).length());
            (d > 2.0).then(|| format!("{} \"{}\" from {:.0},{:.0} {:.0}×{:.0} to {:.0},{:.0} {:.0}×{:.0}", k.0, k.1, r.min.x, r.min.y, r.width(), r.height(), q.min.x, q.min.y, q.width(), q.height()))
        })
        .collect();
    moved.sort();
    (!moved.is_empty()).then(|| format!("{} controls moved in {IDLE} frames with no input, as {}", moved.len(), moved[..moved.len().min(2)].join("; ")))
}

/// The dialog in front (the app's dialogs are modal): its layer and place.
pub(super) fn modal(rig: &Rig) -> Option<(egui::LayerId, Rect)> {
    rig.ctx.memory(|m| m.top_modal_layer().and_then(|l| m.area_rect(l.id).map(|r| (l, r))))
}

/// Tools whose drag is meant to move the view.
fn moves_view(tool: &str) -> bool {
    tool.contains("Hand") || tool.contains("Pan")
}

#[derive(Default)]
struct Found {
    fails: Vec<(&'static str, String)>,
    checks: usize,
    /// A tool was picked and used.
    used: bool,
    /// Dialogs it opened.
    dialogs: usize,
}

/// Lets go of the tool picked with `last`: its button again, else Escape
/// (twice: the first may only drop what is picked). Err: what is still on.
fn release(rig: &mut Rig, last: &Control, t0: &str) -> Result<(), String> {
    if tool(rig) == t0 {
        // Let go by itself once used (a text box drawn).
        return Ok(());
    }
    let shown = rig.find(&last.key);
    if let Some(c) = &shown {
        rig.reach(c);
        if tool(rig) == t0 {
            return Ok(());
        }
    }
    let after_button = tool(rig);
    for _ in 0..2 {
        key(rig, Key::Escape, Modifiers::NONE);
        if tool(rig) == t0 {
            // Picked from a menu or the search, gone since: Escape is the way.
            return if shown.is_some() { Err(format!("its button clicked again leaves it on ({after_button}); only Escape lets go")) } else { Ok(()) };
        }
    }
    Err(format!("neither its button clicked again nor Escape lets go: {}", tool(rig)))
}

/// A tool just picked with `last`, used as a person would.
fn use_tool(rig: &mut Rig, id: u64, t0: &str, last: &Control, f: &mut Found) {
    let t1 = tool(rig);
    let area = content(rig);
    // Up and left: a view shown from its start can only scroll down.
    let (from, to) = (area.lerp_inside(vec2(0.6, 0.6)), area.lerp_inside(vec2(0.38, 0.38)));
    let near = Rect::from_two_pos(from, to).expand(60.0);
    let (p0, m0) = (picture(rig), model::model(&rig.app, id));
    drag(rig, from, to);
    let (mut p1, mut m1) = (picture(rig), model::model(&rig.app, id));
    // A dialog asking for what to put there (a note's text): what the
    // drag did shows once it's closed.
    let asked = modal(rig).is_some();
    if asked {
        // Once more only if needed: a second Escape would let go the tool.
        key(rig, Key::Escape, Modifiers::NONE);
        if modal(rig).is_some() {
            key(rig, Key::Escape, Modifiers::NONE);
        }
        if modal(rig).is_some() {
            f.fails.push(("esc", format!("with it ({t1}), the dialog a drag on the page opens doesn't close with Escape")));
            return;
        }
        (p1, m1) = (picture(rig), model::model(&rig.app, id));
    }
    f.checks += 2;
    // The eraser and the lasso act on ink: on a page without any, nothing.
    let on_ink = t1.contains("Eraser") || t1.contains("Lasso");
    if !asked && !on_ink && m1 == m0 && changed(&p0, &p1, area, None) < 0.0005 {
        f.fails.push(("inert", format!("picked ({t1}), a drag on the page changes nothing")));
    }
    let far = changed(&p0, &p1, area, Some(near));
    if far > 0.5 && !moves_view(&t1) {
        shot(&p0, &format!("jump-{t1}-0"));
        shot(&p1, &format!("jump-{t1}-1"));
        f.fails.push(("jump", format!("with it ({t1}), a drag on the page changes {:.0}% of the view away from the drag: the view scrolled or zoomed", far * 100.0)));
    }
    f.checks += 1;
    if let Some(d) = drift(rig) {
        f.fails.push(("drift", format!("after using it: {d}")));
    }
    f.checks += 1;
    if let Err(e) = release(rig, last, t0) {
        f.fails.push(("stuck", e));
        return;
    }
    // On again, text selected meanwhile, its button again.
    let Some(c) = rig.find(&last.key) else { return };
    rig.reach(&c);
    if tool(rig) == t0 {
        return;
    }
    key(rig, Key::A, Modifiers::COMMAND);
    let Some(c) = rig.find(&last.key) else { return };
    f.checks += 1;
    rig.reach(&c);
    if tool(rig) != t0 {
        f.fails.push(("stuck", format!("with everything selected (Ctrl+A), its button clicked again leaves it on ({})", tool(rig))));
    }
}

fn check_path(qv: Qv, path: &[Control], n: u64) -> Result<Option<Found>, String> {
    let r = audit::quietly(|| {
        let mut rig = open(qv, n);
        let id = rig.app.tabs[rig.app.active].id;
        let (last, before) = path.split_last().unwrap();
        for step in before {
            let c = rig.find(&step.key)?;
            if rig.reach(&c) == Reach::Missed {
                return None;
            }
        }
        let c = rig.find(&last.key)?;
        common::take_errors();
        let mut f = Found::default();
        let (t0, m0) = (tool(&rig), model::model(&rig.app, id));
        let w0: Vec<String> = rig.windows.iter().map(|w| w.0.clone()).collect();
        let d0 = modal(&rig).map(|d| d.0);
        rig.reach(&c);
        f.checks += 2;
        if let Some(e) = common::take_errors().first() {
            f.fails.push(("error", format!("reports an error: {e}")));
        }
        if let Some(d) = drift(&mut rig) {
            f.fails.push(("drift", d));
        }
        // The dialogs it opened: whole in the window, gone with Escape.
        let screen = Rect::from_min_size(Pos2::ZERO, SCREEN);
        let mut opened: Vec<(String, Rect)> = rig.windows.iter().filter(|w| !w0.contains(&w.0) && w.1.area() < 0.9 * screen.area()).cloned().collect();
        let dialog = modal(&rig).filter(|d| Some(d.0) != d0);
        if let Some((_, r)) = dialog {
            opened.push(("(dialog)".into(), r));
        }
        f.dialogs = opened.len();
        if !opened.is_empty() {
            for (name, r) in &opened {
                f.checks += 1;
                if !screen.expand(2.0).contains_rect(*r) {
                    f.fails.push(("cut", format!("the dialog {name} shows at {:.0},{:.0}–{:.0},{:.0}, past the edge of a {}×{} window", r.min.x, r.min.y, r.max.x, r.max.y, SCREEN.x, SCREEN.y)));
                }
            }
            // Twice: the first may only leave a text field.
            key(&mut rig, Key::Escape, Modifiers::NONE);
            key(&mut rig, Key::Escape, Modifiers::NONE);
            for (name, _) in &opened {
                f.checks += 1;
                let still = if name == "(dialog)" { modal(&rig).is_some_and(|d| Some(d.0) == dialog.map(|d| d.0)) } else { rig.windows.iter().any(|w| &w.0 == name) };
                if still {
                    f.fails.push(("esc", format!("the dialog {name} doesn't close with Escape")));
                }
            }
            return Some(f);
        }
        // Still this view, a tool picked and nothing edited yet.
        let same = rig.app.tabs.get(rig.app.active).is_some_and(|t| t.id == id);
        if same && tool(&rig) != t0 && model::model(&rig.app, id) == m0 {
            f.used = true;
            use_tool(&mut rig, id, &t0, last, &mut f);
            if let Some(e) = common::take_errors().first() {
                f.fails.push(("error", format!("used on the page, reports an error: {e}")));
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

fn run_view(job: &str, sink: &mut Sink) {
    audit::panic_hook();
    let Some(qv) = job_view(job) else { return };
    audit::watch(Some(format!("{job}: finding the controls")));
    let paths = actions(qv, open);
    let (mut used, mut dialogs) = (0, 0);
    for (i, path) in paths.iter().enumerate() {
        let what = describe(path);
        audit::watch(Some(format!("{job}: {what}")));
        let repro = format!("open the {job} sample in a {}×{} window, click {what}", SCREEN.x, SCREEN.y);
        match check_path(qv, path, 100 + i as u64) {
            Err(e) => {
                sink.check();
                sink.fail(common::id(&[job, "panic", &what]), format!("[panic] {job}: {what} — {e}. Repro: {repro}"));
            }
            Ok(None) => {}
            Ok(Some(f)) => {
                for _ in 0..f.checks {
                    sink.check();
                }
                used += usize::from(f.used);
                dialogs += f.dialogs;
                for (rule, detail) in f.fails {
                    let how = match rule {
                        "drift" => ", then wait",
                        "esc" => ", then press Esc",
                        "inert" | "jump" => ", then drag on the page",
                        "stuck" => ", drag on the page, click it again (and press Esc)",
                        _ => "",
                    };
                    sink.fail(common::id(&[job, rule, &what]), format!("[{rule}] {job}: {what} — {detail}. Repro: {repro}{how}"));
                }
            }
        }
    }
    audit::watch(None);
    sink.note(format!("{} controls clicked, {used} of them tools used on the page, {dialogs} dialogs opened", paths.len()));
}

/// The languages the window is checked in: the user's, and the one of
/// the longest words (a label a little longer may push a panel wider).
pub(super) const LANGS: [&str; 2] = ["es", "de"];

/// The view of a job ("Pdf@es"), the window set to its language.
pub(super) fn job_view(job: &str) -> Option<Qv> {
    let (view, lang) = job.split_once('@').unwrap_or((job, "en"));
    crate::i18n::set(lang);
    Qv::parse(view)
}

/// The views checked: the samples, the selections where a tool meets
/// them.
pub(super) const VIEWS: [Qv; 8] = [Qv::Doc, Qv::DocSel, Qv::Sheet, Qv::Slides, Qv::SlidesSel, Qv::Pdf, Qv::Db, Qv::Note];

#[test]
#[ignore]
fn qa_behave() {
    let langs = if common::full() { &LANGS[..] } else { &LANGS[..1] };
    let jobs: Vec<String> = VIEWS.iter().flat_map(|q| langs.iter().map(move |l| format!("{}@{l}", q.name()))).collect();
    common::run("behave", "app::qa::behave::qa_behave", &jobs, run_view);
}
