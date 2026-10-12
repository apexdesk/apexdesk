//! Accessibility, on every screen (`scenes`) in English:
//! - **name**: every control has a name a screen reader can say;
//! - **tooltip**: every button and drop-down of the window (not of a
//!   dialog) that doesn't show its name in full (an icon, a cut label)
//!   says what it does when the pointer rests on it;
//! - **shortcut**: no two commands of a view show the same shortcut;
//! - **disabled**: with something selected (a target for them), a
//!   control that is off says why when the pointer rests on it (the
//!   audit checks the other way: off when there is nothing to do).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use eframe::egui::{Event, Order};

use super::common::{self, Sink};
use super::model::Qv;
use super::scenes::{self, Scene, WIDE};
use crate::app::audit::{self, Control, Rig};

/// The jobs: a view each, and the start screen.
const JOBS: [&str; 9] = ["Start", "Doc", "DocSel", "Sheet", "SheetSel", "Slides", "SlidesSel", "Pdf", "Db"];

/// Controls a pointer rests on (not a list's rows, cells, or the tabs).
fn pointed(c: &Control) -> bool {
    matches!(c.key.0.as_str(), "Button" | "ComboBox" | "Switch" | "DefaultButton")
}

/// Tooltips at once: the pointer is left on each control for a frame or two.
fn quick_tooltips(rig: &Rig) {
    rig.ctx.all_styles_mut(|s| {
        s.interaction.tooltip_delay = 0.0;
        s.interaction.tooltip_grace_time = 0.0;
        s.interaction.show_tooltips_only_when_still = false;
    });
}

/// Whether resting the pointer on `c` shows a tooltip; none when the
/// pointer can't reach it.
fn tooltip(rig: &mut Rig, c: &Control) -> Option<bool> {
    let shown = |rig: &Rig| rig.ctx.memory(|m| m.areas().visible_layer_ids().iter().any(|l| l.order == Order::Tooltip));
    rig.frame(vec![Event::PointerGone]);
    rig.frame(Vec::new());
    let at = c.rect.center();
    if !c.rect.is_positive() {
        return None;
    }
    let mut seen = false;
    for i in 0..4 {
        rig.frame(if i == 0 { vec![Event::PointerMoved(at)] } else { Vec::new() });
        seen |= shown(rig);
    }
    rig.frame(vec![Event::PointerGone]);
    Some(seen)
}

/// The shortcuts every command of the view shows, and the commands
/// showing each.
fn shortcuts(rig: &mut Rig) -> BTreeMap<String, BTreeSet<String>> {
    rig.app.want_commands = true;
    rig.frame(Vec::new());
    let mut by_keys: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for c in rig.app.commands.take().unwrap_or_default() {
        let keys = c.entry.keys.replace(' ', "").to_lowercase();
        if keys.is_empty() {
            continue;
        }
        let label = c.entry.label.trim().trim_end_matches('…').trim().to_lowercase();
        by_keys.entry(keys).or_default().insert(label);
    }
    by_keys
}

/// Whether `c` shows its whole name as text: a label says it all, no
/// tooltip is needed.
fn labelled(c: &Control, texts: &[scenes::Text]) -> bool {
    let name = c.key.1.trim().trim_end_matches('…').trim();
    let room = c.rect.expand(1.0);
    !name.is_empty() && texts.iter().any(|t| !t.elided && room.contains(t.rect.center()) && t.text.trim().trim_end_matches('…').trim() == name)
}

/// `seen`: the controls already pointed at in this job (the tabs and
/// the title bar repeat on every screen).
fn check_scene(job: &str, scene: &Scene, sel: bool, seen: &mut HashSet<String>, sink: &mut Sink) -> Result<(), String> {
    let mut rig = scenes::open(scene, WIDE)?;
    quick_tooltips(&rig);
    let controls = rig.controls.clone();
    let layers = scenes::widgets(&rig);
    let texts = scenes::texts(&rig);
    let repro = format!("the {job} sample, {}", scene.name);
    for c in &controls {
        let at = if c.place.is_empty() { String::new() } else { format!(" in {}", c.place) };
        if !sel && seen.insert(format!("name {} {} {}", c.key.0, c.place, c.key.2)) {
            sink.check();
            if c.key.1.trim().is_empty() {
                sink.fail(common::id(&[job, &scene.name, "name", &c.key.0, &c.place, &c.key.2.to_string()]), format!("[name] {job} {}: {} #{}{at} has no accessible name. Repro: {repro}", scene.name, c.key.0, c.key.2));
            }
        }
        // Tooltips only on the window's panels, not in dialogs and menus
        // (nor under a dialog, which the pointer can't reach).
        let on_window = scene.dialog.is_none() && layers.get(&c.node).is_some_and(|(l, _)| l.order == Order::Background);
        if !pointed(c) || !on_window || (sel && c.enabled) || (!sel && !c.enabled) {
            continue;
        }
        if (!sel && labelled(c, &texts)) || !seen.insert(c.describe()) {
            continue;
        }
        let Some(has) = tooltip(&mut rig, c) else { continue };
        sink.check();
        if !has {
            let (rule, why) = if sel { ("disabled", "is off with a target selected and doesn't say why") } else { ("tooltip", "shows no tooltip") };
            sink.fail(common::id(&[job, &scene.name, rule, &c.describe()]), format!("[{rule}] {job} {}: {} {why}. Repro: {repro}, rest the pointer on it", scene.name, c.describe()));
        }
    }
    Ok(())
}

fn run_job(job: &str, sink: &mut Sink) {
    audit::panic_hook();
    let all = scenes::all();
    let (base, sel) = match job {
        "Start" => (None, false),
        _ => {
            let Some(qv) = Qv::parse(job) else { return };
            let base = Qv::parse(job.trim_end_matches("Sel"));
            (base, qv.name() != base.map(Qv::name).unwrap_or_default())
        }
    };
    let mut missed = Vec::new();
    let mut seen = HashSet::new();
    for scene in all.iter().filter(|s| s.view == base && !(sel && s.dialog.is_some())) {
        let scene = Scene { view: Qv::parse(job).or(scene.view), ..scene.clone() };
        audit::watch(Some(format!("a11y {job} {}", scene.name)));
        match audit::quietly(|| check_scene(job, &scene, sel, &mut seen, sink)) {
            Ok(Ok(())) => {}
            Ok(Err(why)) => missed.push(format!("{} ({why})", scene.name)),
            Err(e) => {
                audit::drop_context();
                crate::toolstrip::test::reset();
                sink.check();
                sink.fail(common::id(&[job, &scene.name, "panic"]), format!("[panic] {job} {}: {}", scene.name, audit::panic_text(&e)));
            }
        }
    }
    // The shortcuts of the view, once.
    if let Some(qv) = base.filter(|_| !sel) {
        let mut rig = Rig::open(false, |app| qv.setup(app, 0));
        for (keys, labels) in shortcuts(&mut rig) {
            sink.check();
            if labels.len() > 1 {
                let l: Vec<&String> = labels.iter().collect();
                sink.fail(common::id(&[job, "shortcut", &keys]), format!("[shortcut] {job}: {keys} is shown for {} commands: {l:?}. Repro: the {job} sample, the tooltips of those commands", labels.len()));
            }
        }
    }
    audit::watch(None);
    if !missed.is_empty() {
        sink.note(format!("not reached: {}", missed.join("; ")));
    }
}

#[test]
#[ignore]
fn qa_a11y() {
    common::run("a11y", "app::qa::a11y::qa_a11y", &JOBS.map(String::from), run_job);
}

/// With a target selected, every control of the toolstrips that is off
/// says why (Undo and Redo aside: nothing has been done in the samples).
#[test]
fn disabled_controls_say_why() {
    audit::isolated_at("app::qa::a11y::disabled_controls_say_why", || {
        let mut silent = Vec::new();
        for scene in scenes::all().into_iter().filter(|s| s.dialog.is_none()) {
            let Some(qv) = scene.view.and_then(|v| Qv::parse(&format!("{}Sel", v.name()))) else { continue };
            let scene = Scene { view: Some(qv), ..scene };
            let Ok(mut rig) = scenes::open(&scene, WIDE) else { continue };
            quick_tooltips(&rig);
            let layers = scenes::widgets(&rig);
            for c in rig.controls.clone() {
                let on_window = layers.get(&c.node).is_some_and(|(l, _)| l.order == Order::Background);
                if pointed(&c) && on_window && !c.enabled && !["Undo", "Redo"].contains(&c.key.1.as_str()) && tooltip(&mut rig, &c) == Some(false) {
                    silent.push(format!("{} {}", scene.name, c.describe()));
                }
            }
        }
        assert!(silent.is_empty(), "off without saying why: {silent:?}");
    });
}
