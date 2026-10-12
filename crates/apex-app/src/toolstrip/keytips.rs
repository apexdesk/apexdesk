//! KeyTips: pressing and releasing Alt shows a letter over each tab of the
//! toolstrip (F for the File menu); typing one opens that tab and shows letters
//! over its buttons, and typing those clicks the button, as in Office. A
//! menu opened that way shows letters over its items. Escape goes back a
//! level; a click, or Alt again, hides the letters.

use std::cell::RefCell;

use eframe::egui::{self, Color32, CornerRadius, Event, FontId, Id, Key, LayerId, Order, Pos2, Rect, Stroke, pos2, vec2};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Level {
    #[default]
    Off,
    Tabs,
    Buttons,
}

/// A button drawn while the letters show.
#[derive(Clone, Debug)]
struct Seen {
    /// Its command key (see `register`), or a folded group's title.
    key: String,
    name: String,
    rect: Rect,
    /// Opens a menu: the letters go on inside it.
    menu: bool,
    /// Drawn in a popup (a menu or a folded group).
    popup: bool,
    group: bool,
}

#[derive(Default)]
struct Tips {
    level: Level,
    typed: String,
    /// Alt is down: true while nothing else was pressed with it.
    alt: Option<bool>,
    seen: Vec<Seen>,
    /// The buttons' letters, from the last frame: (tip, button).
    buttons: Vec<(String, Seen)>,
    /// The tabs' letters: (tip, tab id), "" for File.
    tabs: Vec<(String, &'static str)>,
    /// A tab chosen by its letter, for the strip to select.
    goto: Option<&'static str>,
    open_file: bool,
    /// A folded group chosen by its letter, to open.
    open_group: Option<String>,
}

thread_local! {
    static TIPS: RefCell<Tips> = RefCell::default();
}

pub fn level() -> Level {
    TIPS.with_borrow(|t| t.level)
}

/// Whether the buttons drawn now get letters.
pub(super) fn recording() -> bool {
    TIPS.with_borrow(|t| t.level == Level::Buttons)
}

/// A button drawn at `at` (made exact by [`place`]).
pub(super) fn seen(ui: &egui::Ui, key: String, name: &str, menu: bool) {
    let popup = matches!(ui.layer_id().order, Order::Foreground | Order::Tooltip);
    let rect = Rect::from_min_size(ui.cursor().min, vec2(24.0, super::ROW_H));
    TIPS.with_borrow_mut(|t| t.seen.push(Seen { key, name: name.to_string(), rect, menu, popup, group: false }));
}

/// The rect of the button [`seen`] last.
pub(super) fn place(rect: Rect) {
    TIPS.with_borrow_mut(|t| {
        if let Some(s) = t.seen.last_mut() {
            s.rect = rect;
        }
    });
}

/// A folded group's button; true when it is to open.
pub(super) fn group(ui: &egui::Ui, title: &str, rect: Rect) -> bool {
    TIPS.with_borrow_mut(|t| {
        if t.level == Level::Buttons {
            let popup = matches!(ui.layer_id().order, Order::Foreground | Order::Tooltip);
            t.seen.push(Seen { key: title.to_string(), name: title.to_string(), rect, menu: true, popup, group: true });
        }
        if t.open_group.as_deref() == Some(title) {
            t.open_group = None;
            return true;
        }
        false
    })
}

/// The first letter or digit of a name, as typed.
fn initial(name: &str) -> Option<char> {
    name.chars().find(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase())
}

/// Letters for names, in order: the initial when no other name has it,
/// else the initial and a second character (other letters of the name,
/// then digits and letters). Names without Latin letters share "Y". No
/// tip is the start of another.
pub(crate) fn assign(names: &[&str]) -> Vec<String> {
    let firsts: Vec<char> = names.iter().map(|n| initial(n).unwrap_or('Y')).collect();
    let mut used: std::collections::HashSet<String> = Default::default();
    let mut out = Vec::with_capacity(names.len());
    for (i, name) in names.iter().enumerate() {
        let f = firsts[i];
        if firsts.iter().filter(|&&c| c == f).count() == 1 {
            out.push(f.to_string());
            continue;
        }
        let rest = name.chars().filter(|c| c.is_ascii_alphanumeric()).skip(1).map(|c| c.to_ascii_uppercase());
        let pool = rest.chain("123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ0".chars());
        let tip = pool.map(|c| format!("{f}{c}")).find(|t| !used.contains(t)).unwrap_or_else(|| format!("{f}{i}"));
        used.insert(tip.clone());
        out.push(tip);
    }
    out
}

/// Letters of the toolstrip's tabs: Office's for the usual ones, else a
/// letter of the name not taken yet. File is F.
pub(crate) fn tab_tips(tabs: &[(&'static str, &str)]) -> Vec<String> {
    let preferred = |id: &str| match id {
        "home" => Some('H'),
        "insert" => Some('N'),
        "layout" | "page" | "page_layout" => Some('P'),
        "formulas" => Some('M'),
        "data" => Some('A'),
        "review" => Some('R'),
        "view" => Some('W'),
        "references" => Some('S'),
        "mailings" => Some('M'),
        "draw" => Some('J'),
        "help" => Some('Y'),
        "design" => Some('G'),
        _ => None,
    };
    let mut used: Vec<char> = vec!['F'];
    let mut out = vec![String::new(); tabs.len()];
    for (i, (id, _)) in tabs.iter().enumerate() {
        if let Some(c) = preferred(id).filter(|c| !used.contains(c)) {
            used.push(c);
            out[i] = c.to_string();
        }
    }
    for (i, (_, name)) in tabs.iter().enumerate() {
        if out[i].is_empty() {
            let c = name.chars().map(|c| c.to_ascii_uppercase()).filter(|c| c.is_ascii_alphabetic()).chain('A'..='Z').chain('1'..='9').find(|c| !used.contains(c)).unwrap_or('0');
            used.push(c);
            out[i] = c.to_string();
        }
    }
    out
}

/// What a frame's input did.
#[derive(Debug, Default, PartialEq)]
struct Step {
    /// Keys and text are the letters': take them from the frame.
    consume: bool,
    /// A button to click (its command key).
    click: Option<String>,
}

/// Follows Alt and the letters typed.
fn step(t: &mut Tips, alt: bool, events: &[Event], allowed: bool) -> Step {
    let mut out = Step::default();
    if !allowed {
        t.level = Level::Off;
        t.alt = None;
        return out;
    }
    let pressed = events.iter().any(|e| matches!(e, Event::Key { pressed: true, .. } | Event::PointerButton { pressed: true, .. }));
    if alt {
        match &mut t.alt {
            None => t.alt = Some(!pressed),
            Some(clean) => *clean &= !pressed,
        }
    } else if t.alt.take() == Some(true) && !pressed {
        t.level = if t.level == Level::Off { Level::Tabs } else { Level::Off };
        t.typed.clear();
        return out;
    }
    if t.level == Level::Off || alt {
        return out;
    }
    if events.iter().any(|e| matches!(e, Event::PointerButton { pressed: true, .. })) {
        t.level = Level::Off;
        return out;
    }
    out.consume = true;
    for e in events {
        match e {
            Event::Key { key: Key::Escape, pressed: true, .. } => {
                t.typed.clear();
                t.level = if t.level == Level::Buttons { Level::Tabs } else { Level::Off };
            }
            Event::Key { key: Key::Backspace, pressed: true, .. } => {
                t.typed.pop();
            }
            Event::Text(s) => {
                for c in s.chars() {
                    t.typed.push(c.to_ascii_uppercase());
                    match t.level {
                        Level::Tabs => {
                            if let Some((_, id)) = t.tabs.iter().find(|(tip, _)| *tip == t.typed) {
                                if id.is_empty() {
                                    t.open_file = true;
                                    t.level = Level::Off;
                                } else {
                                    t.goto = Some(id);
                                    t.level = Level::Buttons;
                                }
                                t.typed.clear();
                            } else if !t.tabs.iter().any(|(tip, _)| tip.starts_with(&t.typed)) {
                                t.typed.clear();
                            }
                        }
                        Level::Buttons => {
                            if let Some((_, b)) = t.buttons.iter().find(|(tip, _)| *tip == t.typed).cloned() {
                                t.typed.clear();
                                if b.group {
                                    t.open_group = Some(b.key);
                                } else {
                                    if !b.menu {
                                        t.level = Level::Off;
                                    }
                                    out.click = Some(b.key);
                                }
                            } else if !t.buttons.iter().any(|(tip, _)| tip.starts_with(&t.typed)) {
                                t.typed.clear();
                            }
                        }
                        Level::Off => {}
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Reads the frame's input before anything else sees it; `allowed`: the
/// toolstrip is on screen and nothing else takes the keyboard.
pub fn input(ctx: &egui::Context, allowed: bool) {
    let (alt, events) = ctx.input(|i| (i.modifiers.alt, i.events.clone()));
    let s = TIPS.with_borrow_mut(|t| step(t, alt, &events, allowed));
    if s.consume {
        ctx.input_mut(|i| i.events.retain(|e| !matches!(e, Event::Key { .. } | Event::Text(_))));
    }
    if let Some(key) = s.click {
        super::open_menu(key);
    }
    if level() != Level::Off || s.consume {
        ctx.request_repaint();
    }
}

/// The tab strip: its letters, and the tab or File menu chosen by one.
pub(super) fn strip(ui: &egui::Ui, tabs: &[(&'static str, &str, Rect)], file: Rect) -> (Option<&'static str>, bool) {
    let list: Vec<(&'static str, &str)> = tabs.iter().map(|(id, name, _)| (*id, *name)).collect();
    let tips = tab_tips(&list);
    TIPS.with_borrow_mut(|t| {
        t.tabs = std::iter::once((String::from("F"), "")).chain(tips.iter().cloned().zip(list.iter().map(|x| x.0))).collect();
        if t.level == Level::Tabs {
            let p = painter(ui.ctx());
            badge(&p, pos2(file.center().x, file.bottom()), "F", &t.typed);
            for ((_, _, r), tip) in tabs.iter().zip(&tips) {
                badge(&p, pos2(r.center().x, r.bottom()), tip, &t.typed);
            }
        }
        (t.goto.take(), std::mem::take(&mut t.open_file))
    })
}

/// Paints the buttons' letters, after the toolstrip (and its menus) is drawn.
pub fn paint(ctx: &egui::Context) {
    TIPS.with_borrow_mut(|t| {
        let mut seen = std::mem::take(&mut t.seen);
        if t.level != Level::Buttons {
            t.buttons.clear();
            return;
        }
        // An open menu has the letters to itself.
        if seen.iter().any(|s| s.popup) {
            seen.retain(|s| s.popup);
        }
        let names: Vec<&str> = seen.iter().map(|s| s.name.as_str()).collect();
        let tips = assign(&names);
        let p = painter(ctx);
        for (s, tip) in seen.iter().zip(&tips) {
            let at = if s.rect.height() > 40.0 { pos2(s.rect.center().x, s.rect.bottom() - 6.0) } else { pos2(s.rect.left() + 12.0, s.rect.bottom() - 2.0) };
            badge(&p, at, tip, &t.typed);
        }
        t.buttons = tips.into_iter().zip(seen).collect();
    });
}

fn painter(ctx: &egui::Context) -> egui::Painter {
    ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("toolstrip_keytips")))
}

/// A letter in a small dark box centered on `at`; dimmed when what was
/// typed already rules it out.
fn badge(p: &egui::Painter, at: Pos2, tip: &str, typed: &str) {
    if !tip.starts_with(typed) {
        return;
    }
    let g = p.layout_no_wrap(tip.to_string(), FontId::proportional(11.0), Color32::WHITE);
    let rect = Rect::from_center_size(at, g.size() + vec2(8.0, 3.0));
    p.rect(rect, CornerRadius::same(3), Color32::from_rgb(0x30, 0x30, 0x30), Stroke::new(1.0, Color32::from_gray(200)), egui::StrokeKind::Outside);
    p.galley(rect.center() - g.size() / 2.0, g, Color32::WHITE);
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Modifiers;

    fn key(k: Key) -> Event {
        Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }
    }

    #[test]
    fn tips() {
        assert_eq!(assign(&["Bold", "Italic", "Borders", "Cut", "粘贴", "复制"]), ["BO", "I", "BR", "C", "Y1", "Y2"]);
        assert_eq!(assign(&["Paste", "Paste"]), ["PA", "PS"]);
        let t = tab_tips(&[("home", "Home"), ("insert", "Insert"), ("formulas", "Formulas"), ("chart", "Chart Design"), ("table", "Table")]);
        assert_eq!(t, ["H", "N", "M", "C", "T"]);
    }

    #[test]
    fn alt_then_letters() {
        let mut t = Tips::default();
        // Alt with Q (the search) doesn't show the letters.
        step(&mut t, true, &[], true);
        step(&mut t, true, &[key(Key::Q)], true);
        step(&mut t, false, &[], true);
        assert_eq!(t.level, Level::Off);
        // Alt alone does.
        step(&mut t, true, &[], true);
        step(&mut t, false, &[], true);
        assert_eq!(t.level, Level::Tabs);
        t.tabs = vec![("F".into(), ""), ("H".into(), "home"), ("N".into(), "insert")];
        let s = step(&mut t, false, &[key(Key::N), Event::Text("n".into())], true);
        assert!(s.consume);
        assert_eq!((t.level, t.goto), (Level::Buttons, Some("insert")));
        let b = |name: &str, menu: bool| Seen { key: format!("Insert/{name}"), name: name.into(), rect: Rect::NOTHING, menu, popup: false, group: false };
        t.buttons = vec![("PI".into(), b("Picture", true)), ("PT".into(), b("PivotTable", false))];
        assert_eq!(step(&mut t, false, &[Event::Text("p".into())], true).click, None);
        let s = step(&mut t, false, &[Event::Text("t".into())], true);
        assert_eq!((s.click.as_deref(), t.level), (Some("Insert/PivotTable"), Level::Off));
        // Escape goes back; a menu keeps the letters.
        t.level = Level::Buttons;
        assert_eq!(step(&mut t, false, &[Event::Text("PI".into())], true).click.as_deref(), Some("Insert/Picture"));
        assert_eq!(t.level, Level::Buttons);
        step(&mut t, false, &[key(Key::Escape)], true);
        assert_eq!(t.level, Level::Tabs);
        step(&mut t, false, &[key(Key::Escape)], true);
        assert_eq!(t.level, Level::Off);
        // F opens the File menu.
        t.level = Level::Tabs;
        step(&mut t, false, &[Event::Text("f".into())], true);
        assert!(t.open_file && t.level == Level::Off);
    }
}
