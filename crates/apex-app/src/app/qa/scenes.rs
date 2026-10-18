//! The screens the layout, screenshot, accessibility and translation
//! sections look at: the start screen, each toolstrip tab of each view and
//! the main dialogs, each opened in a fresh window of a given size; and
//! what a screen draws (its texts, where they are and what cuts them).

use eframe::egui::{self, Event, Key, Modifiers, Rect, Vec2, vec2};

use super::model::Qv;
use crate::app::audit::{Control, Reach, Rig};
use crate::i18n::tr;

/// The window of every day.
pub(super) const WIDE: Vec2 = vec2(1280.0, 800.0);
/// The smallest window the app allows (`main.rs`).
pub(super) const NARROW: Vec2 = vec2(640.0, 400.0);

/// The views whose toolstrip tabs are screens.
const VIEWS: [Qv; 5] = [Qv::Doc, Qv::Sheet, Qv::Slides, Qv::Pdf, Qv::Db];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Dialog {
    Options,
    Print,
    Find,
    Font,
    Paragraph,
    Table,
}

const DIALOGS: [Dialog; 6] = [Dialog::Options, Dialog::Print, Dialog::Find, Dialog::Font, Dialog::Paragraph, Dialog::Table];

/// A screen: a view (none: the start screen), its toolstrip tab, and a
/// dialog open over it.
#[derive(Clone, Debug)]
pub(super) struct Scene {
    /// "Start", "Doc/insert", "Doc/Font dialog": stable, for ids and files.
    pub name: String,
    pub view: Option<Qv>,
    pub tab: &'static str,
    pub dialog: Option<Dialog>,
}

/// Every screen: the start screen, the toolstrip tabs of each view (not
/// the contextual ones) and the dialogs (over a document).
pub(super) fn all() -> Vec<Scene> {
    let mut v = vec![Scene { name: "Start".into(), view: None, tab: "", dialog: None }];
    for qv in VIEWS {
        let rig = Rig::open(false, |app| qv.setup(app, 0));
        for t in rig.app.toolstrip_tabs().into_iter().filter(|t| t.context.is_none()) {
            v.push(Scene { name: format!("{qv:?}/{}", t.id), view: Some(qv), tab: t.id, dialog: None });
        }
    }
    for d in DIALOGS {
        let tab = if d == Dialog::Table { "insert" } else { "home" };
        v.push(Scene { name: format!("Doc/{d:?} dialog"), view: Some(Qv::Doc), tab, dialog: Some(d) });
    }
    v
}

fn key(rig: &mut Rig, key: Key) {
    let ev = |pressed| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::COMMAND };
    rig.frame(vec![ev(true)]);
    rig.frame(vec![ev(false)]);
    rig.settle();
}

/// The control named `name` (or whose name starts with it: a shortcut
/// may follow; or that name without its "…": a button's name).
pub(super) fn named(rig: &Rig, name: &str) -> Option<Control> {
    let bare = name.trim_end_matches('…');
    let c = &rig.controls;
    c.iter().find(|c| c.key.1 == name).or_else(|| c.iter().find(|c| c.key.1.starts_with(name))).or_else(|| c.iter().find(|c| c.key.0 == "Button" && c.key.1 == bare)).cloned()
}

/// Clicks the control `name`; when it isn't there, first opens the
/// toolstrip group `group` folded into a button (a small window).
pub(super) fn click(rig: &mut Rig, name: &str, group: &'static str) -> Result<(), String> {
    if !group.is_empty()
        && named(rig, name).is_none()
        && let Some(g) = rig.controls.iter().find(|c| c.key.0 == "ComboBox" && c.key.1 == tr(group)).cloned()
    {
        rig.reach(&g);
    }
    let Some(c) = named(rig, name) else {
        let word = name.split_whitespace().next().unwrap_or(name).trim_end_matches('…');
        let like: Vec<String> = rig.controls.iter().filter(|c| c.key.1.contains(word)).take(4).map(Control::describe).collect();
        return Err(format!("no control \"{name}\"; like it: {like:?}"));
    };
    if rig.reach(&c) == Reach::Missed {
        return Err(format!("\"{name}\" can't be clicked"));
    }
    Ok(())
}

/// Opens `scene` in a fresh window of `size`; why not, when a step of it
/// isn't there (a button folded away in a small window).
pub(super) fn open(scene: &Scene, size: Vec2) -> Result<Rig, String> {
    let mut rig = Rig::open_sized(false, size, |app| {
        if let Some(qv) = scene.view {
            qv.setup(app, 0);
        }
    });
    if !scene.tab.is_empty() {
        let k = rig.app.toolstrip_kind();
        rig.app.toolstrip_tab[k] = scene.tab;
        rig.settle();
    }
    let before: std::collections::HashSet<_> = rig.controls.iter().map(Control::id).collect();
    match scene.dialog {
        None => {}
        Some(Dialog::Options) => {
            rig.app.open_options();
            rig.settle();
        }
        Some(Dialog::Print) => {
            let ctx = rig.ctx.clone();
            rig.app.open_print(&ctx, false);
            // The printers are looked for in the background.
            apex_fs::thread::sleep(web_time::Duration::from_millis(300));
            rig.settle();
        }
        Some(Dialog::Find) => key(&mut rig, Key::F),
        Some(Dialog::Font) => key(&mut rig, Key::D),
        Some(Dialog::Paragraph) => click(&mut rig, &format!("{}…", tr("doc.paragraph")), "toolstrip.g.styles")?,
        Some(Dialog::Table) => {
            click(&mut rig, tr("table.table"), "toolstrip.g.tables")?;
            click(&mut rig, &format!("{}…", tr("table.insert_table")), "")?;
        }
    }
    // The pointer away, nothing hovered.
    rig.frame(vec![Event::PointerGone]);
    rig.settle();
    if scene.dialog.is_some() && rig.controls.iter().all(|c| before.contains(&c.id())) {
        return Err("the dialog didn't open".into());
    }
    Ok(rig)
}

/// A text drawn: its string, where it is, what cuts it, and whether it
/// was shortened with "…" to fit.
pub(super) struct Text {
    pub text: String,
    pub rect: Rect,
    pub clip: Rect,
    pub elided: bool,
}

/// The texts the last frame drew (not turned ones).
pub(super) fn texts(rig: &Rig) -> Vec<Text> {
    fn walk(s: &egui::Shape, clip: Rect, out: &mut Vec<Text>) {
        match s {
            egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, clip, out)),
            egui::Shape::Text(t) if t.angle == 0.0 && !t.galley.is_empty() => {
                out.push(Text { text: t.galley.text().to_string(), rect: t.galley.rect.translate(t.pos.to_vec2()), clip, elided: t.galley.elided });
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for s in &rig.shapes {
        walk(&s.shape, s.clip_rect, &mut out);
    }
    out
}

/// Each widget of the last frame, by node: its layer (the window's
/// panels, a dialog, a popup) and the part of it shown (cut by what
/// holds it).
pub(super) fn widgets(rig: &Rig) -> std::collections::HashMap<u64, (egui::LayerId, Rect)> {
    rig.ctx.viewport(|vp| vp.prev_pass.widgets.layers().flat_map(|(_, ws)| ws.iter().map(|w| (w.id.value(), (w.layer_id, w.interact_rect)))).collect())
}

/// A short form of a text for a report line.
pub(super) fn short(s: &str) -> String {
    let one = s.replace('\n', " ⏎ ");
    if one.chars().count() > 40 { format!("{}…", one.chars().take(40).collect::<String>()) } else { one }
}
