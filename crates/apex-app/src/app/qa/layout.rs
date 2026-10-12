//! Layout: every screen (`scenes`) in each language (quick: es, en, de;
//! full: all), in a window of 1280×800 and in the smallest one the app
//! allows, measured from what egui lays out (the widgets' rects in the
//! accessibility tree, the texts it draws and what cuts them), not from
//! pixels:
//! - **cut**: a text of the interface cut on its left or right by what
//!   holds it, or shortened with "…";
//! - **text-overlap**: two texts of the same area drawn over each other;
//! - **overlap**: two controls of the same place over each other;
//! - **outside**: a control partly out of the window.
//!
//! What the document shows (its paragraphs, cells, slides) isn't the
//! interface: a cell rightly cuts its text.

use eframe::egui::{LayerId, Pos2, Rect, Vec2};

use super::common::{self, Sink};
use super::scenes::{self, NARROW, Scene, WIDE};
use crate::app::View;
use crate::app::audit::{self, Control, Rig};

/// Pixels past its edge that count as cut (antialiasing, rounding).
const SLACK: f32 = 1.5;

fn langs() -> Vec<String> {
    if common::full() { crate::i18n::available().map(|l| l.code.to_string()).collect() } else { ["es", "en", "de"].map(String::from).to_vec() }
}

/// The texts of the document the screen shows.
fn content(rig: &Rig) -> Vec<String> {
    let mut v = Vec::new();
    for t in &rig.app.tabs {
        match &t.view {
            View::Doc(d) => v.extend(d.doc.paragraphs.iter().map(|p| p.text.clone())),
            View::Sheet(s) => {
                for (i, sh) in s.wb.sheets.iter().enumerate() {
                    v.push(sh.name.clone());
                    v.extend(sh.iter().map(|(at, _)| s.wb.display(i, at).0));
                }
            }
            View::Slides(s) => {
                for sl in &s.pres.deck.slides {
                    for o in &sl.objects {
                        if let apex_slides::object::ObjKind::Shape { text: Some(t), .. } = &o.kind {
                            v.push(t.text());
                        }
                    }
                }
            }
            View::Pdf(_) | View::Db(_) | View::Note(_) => {}
        }
    }
    v.retain(|s| !s.trim().is_empty());
    v
}

/// Whether `role` is a control whose place in a list or grid rightly
/// runs past the window (a cell, a row of a list).
fn listed(c: &Control) -> bool {
    matches!(c.key.0.as_str(), "Cell" | "ListBoxOption" | "TreeItem" | "Row")
}

/// The failures of one screen: (rule, subject, detail).
pub(super) fn check(rig: &Rig, size: Vec2) -> (usize, Vec<(&'static str, String, String)>) {
    let mut fails = Vec::new();
    let mut checks = 0;
    let doc = content(rig);
    let is_content = |s: &str| {
        let s = s.trim();
        s.is_empty() || doc.iter().any(|d| d.contains(s) || s.contains(d.as_str()))
    };
    let texts: Vec<_> = scenes::texts(rig).into_iter().filter(|t| !is_content(&t.text)).collect();
    for t in &texts {
        checks += 1;
        let shown = t.rect.intersect(t.clip);
        if shown.width() < 2.0 || shown.height() < 2.0 {
            // Scrolled out of its area.
            continue;
        }
        let (left, right) = (t.clip.min.x - t.rect.min.x, t.rect.max.x - t.clip.max.x);
        if left > SLACK || right > SLACK {
            let side = if right > SLACK { format!("{right:.0} px on the right") } else { format!("{left:.0} px on the left") };
            fails.push(("cut", scenes::short(&t.text), format!("cut {side} (text {:.0} px wide, {:.0} px shown)", t.rect.width(), shown.width())));
        } else if t.elided {
            fails.push(("cut", scenes::short(&t.text), "shortened with … to fit".into()));
        }
    }
    // Texts over each other in one area (not a text and its own shadow).
    for (i, a) in texts.iter().enumerate() {
        for b in &texts[i + 1..] {
            if a.clip != b.clip || a.text == b.text {
                continue;
            }
            let o = a.rect.intersect(b.rect).intersect(a.clip);
            if o.width() > 2.0 && o.height() > 0.4 * a.rect.height().min(b.rect.height()) {
                checks += 1;
                fails.push(("text-overlap", format!("{} ⟷ {}", scenes::short(&a.text), scenes::short(&b.text)), format!("drawn over each other ({:.0}×{:.0} px)", o.width(), o.height())));
            }
        }
    }
    let screen = Rect::from_min_size(Pos2::ZERO, size);
    let shown = scenes::widgets(rig);
    let controls: Vec<(&Control, LayerId, Rect)> = rig
        .controls
        .iter()
        .filter(|c| !listed(c) && c.bounds.width() >= 1.0 && c.bounds.height() >= 1.0)
        .filter_map(|c| shown.get(&c.node).map(|(l, r)| (c, *l, *r)))
        .collect();
    for (c, _, vis) in &controls {
        checks += 1;
        let b = c.bounds;
        if vis.width() < 1.0 || vis.height() < 1.0 {
            continue;
        }
        // Past an edge of the window, and shown up to it (not cut before
        // by a list it scrolls in).
        let near = |a: f32, b: f32| (a - b).abs() <= SLACK;
        let past = [
            (screen.min.x - b.min.x, near(vis.min.x, screen.min.x)),
            (b.max.x - screen.max.x, near(vis.max.x, screen.max.x)),
            (screen.min.y - b.min.y, near(vis.min.y, screen.min.y)),
            (b.max.y - screen.max.y, near(vis.max.y, screen.max.y)),
        ]
        .into_iter()
        .filter(|(d, at_edge)| *d > SLACK && *at_edge)
        .fold(0.0f32, |m, (d, _)| m.max(d));
        if past > 0.0 {
            fails.push(("outside", c.describe(), format!("{past:.0} px out of the window")));
        }
    }
    // Controls of one layer over each other, in what shows of them.
    for (i, (a, la, va)) in controls.iter().enumerate() {
        for (b, lb, vb) in &controls[i + 1..] {
            if la != lb || a.bounds.contains_rect(b.bounds) || b.bounds.contains_rect(a.bounds) {
                continue;
            }
            let o = va.intersect(*vb);
            if o.width() > 2.0 && o.height() > 2.0 {
                checks += 1;
                fails.push(("overlap", format!("{} ⟷ {}", a.describe(), b.describe()), format!("over each other ({:.0}×{:.0} px)", o.width(), o.height())));
            }
        }
    }
    (checks, fails)
}

fn run_lang(job: &str, sink: &mut Sink) {
    audit::panic_hook();
    crate::i18n::set(job);
    let all = scenes::all();
    let mut missed = Vec::new();
    for scene in &all {
        for size in [WIDE, NARROW] {
            let dims = format!("{}x{}", size.x, size.y);
            audit::watch(Some(format!("layout {job} {dims} {}", scene.name)));
            let r = audit::quietly(|| scenes::open(scene, size).map(|rig| check(&rig, size)));
            match r {
                Err(e) => {
                    audit::drop_context();
                    crate::toolstrip::test::reset();
                    sink.check();
                    sink.fail(common::id(&[job, &dims, &scene.name, "panic"]), format!("[panic] {job} {dims} {}: opening it panics: {}. Repro: {}", scene.name, audit::panic_text(&e), repro(job, scene)));
                }
                Ok(Err(why)) => missed.push(format!("{} {dims} ({why})", scene.name)),
                Ok(Ok((checks, fails))) => {
                    for _ in 0..checks {
                        sink.check();
                    }
                    for (rule, what, detail) in fails {
                        sink.fail(common::id(&[job, &dims, &scene.name, rule, &what]), format!("[{rule}] {job} {dims} {}: {what} — {detail}. Repro: {}", scene.name, repro(job, scene)));
                    }
                }
            }
        }
    }
    audit::watch(None);
    sink.note(format!("{} screens at 1280x800 and 640x400", all.len()));
    if !missed.is_empty() {
        sink.note(format!("not reached: {}", missed.join("; ")));
    }
}

fn repro(lang: &str, scene: &Scene) -> String {
    let how = match scene.dialog {
        Some(d) => format!(", open the {d:?} dialog"),
        None if scene.tab.is_empty() => String::new(),
        None => format!(", toolstrip tab {}", scene.tab),
    };
    format!("language {lang}, {} sample{how}", scene.view.map_or("no".to_string(), |v| format!("{v:?}")))
}

#[test]
#[ignore]
fn qa_layout() {
    common::run("layout", "app::qa::layout::qa_layout", &langs(), run_lang);
}
