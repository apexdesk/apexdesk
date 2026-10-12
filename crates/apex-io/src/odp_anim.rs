//! Slide transitions, animations, show settings and themes of
//! OpenDocument presentations. They are written as ODF has them (SMIL
//! transitions in the page style, `anim:par` effects, the
//! `presentation:settings` of the show, LibreOffice's `loext:theme`),
//! and what ODF can't say (3D transitions, our effects and their
//! options, kiosk shows, theme fonts) goes in `apx:` attributes beside,
//! so that it comes back as it was.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::Theme;
use apex_doc::theme::{ColorScheme, FontScheme};
use apex_slides::ShowSettings;
use apex_slides::anim::{Anim, Class, Dir, Effect, Timing, TransKind, Transition, Trigger};
use apex_slides::object::{Raw, Src};

use crate::drawingml::Node;
use crate::xml::esc;

pub(crate) const NS_APX: &str = "urn:apexdesk:xmlns:presentation:1.0";
pub(crate) const NS_LOEXT: &str = "urn:org:documentfoundation:names:experimental:office:xmlns:loext:1.0";

/// An attribute by its exact qualified name (`Node::attr` also matches
/// local names: "apx:dur" would find "smil:dur").
pub(crate) fn exact<'a>(n: &'a Node, key: &str) -> Option<&'a str> {
    n.attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// Seconds of a SMIL clock value ("1.5s", "500ms") or an ISO duration
/// ("PT5S", "PT00H01M05S").
pub(crate) fn secs(v: &str) -> Option<f32> {
    let v = v.trim();
    if let Some(r) = v.strip_prefix("PT") {
        let (mut total, mut num) = (0.0f32, String::new());
        for c in r.chars() {
            let k = match c {
                'H' => 3600.0,
                'M' => 60.0,
                'S' => 1.0,
                _ => {
                    num.push(c);
                    continue;
                }
            };
            total += num.parse::<f32>().ok()? * k;
            num.clear();
        }
        return Some(total).filter(|t| t.is_finite());
    }
    let (n, k) = match v.strip_suffix("ms") {
        Some(n) => (n, 0.001),
        None => (v.strip_suffix('s').unwrap_or(v), 1.0),
    };
    n.trim().parse::<f32>().ok().map(|x| x * k).filter(|x| x.is_finite())
}

// --- names ------------------------------------------------------------------------------

const KINDS: [(TransKind, &str); 18] = [
    (TransKind::None, "none"),
    (TransKind::Fade, "fade"),
    (TransKind::Push, "push"),
    (TransKind::Wipe, "wipe"),
    (TransKind::Split, "split"),
    (TransKind::Reveal, "reveal"),
    (TransKind::Cover, "cover"),
    (TransKind::Zoom, "zoom"),
    (TransKind::Dissolve, "dissolve"),
    (TransKind::Blinds, "blinds"),
    (TransKind::Checker, "checker"),
    (TransKind::Doors, "doors"),
    (TransKind::Cube, "cube"),
    (TransKind::Flip, "flip"),
    (TransKind::Switch, "switch"),
    (TransKind::FallOver, "fall-over"),
    (TransKind::Morph, "morph"),
    (TransKind::Other, "other"),
];

const DIRS: [(Dir, &str); 12] = [
    (Dir::Left, "left"),
    (Dir::Right, "right"),
    (Dir::Up, "up"),
    (Dir::Down, "down"),
    (Dir::HorzIn, "horizontal-in"),
    (Dir::HorzOut, "horizontal-out"),
    (Dir::VertIn, "vertical-in"),
    (Dir::VertOut, "vertical-out"),
    (Dir::In, "in"),
    (Dir::Out, "out"),
    (Dir::Horz, "horizontal"),
    (Dir::Vert, "vertical"),
];

const EFFECTS: [(Effect, &str); 17] = [
    (Effect::Appear, "appear"),
    (Effect::Fade, "fade"),
    (Effect::Fly, "fly"),
    (Effect::Zoom, "zoom"),
    (Effect::Wipe, "wipe"),
    (Effect::Spin, "spin"),
    (Effect::Grow, "grow"),
    (Effect::Float, "float"),
    (Effect::Split, "split"),
    (Effect::Bounce, "bounce"),
    (Effect::GrowTurn, "grow-turn"),
    (Effect::Swivel, "swivel"),
    (Effect::Pulse, "pulse"),
    (Effect::Teeter, "teeter"),
    (Effect::Blink, "blink"),
    (Effect::Transparency, "transparency"),
    (Effect::Other, "other"),
];

const CLASSES: [(Class, &str); 5] = [(Class::Entrance, "entrance"), (Class::Exit, "exit"), (Class::Emphasis, "emphasis"), (Class::Path, "motion-path"), (Class::Media, "media-call")];

const TRIGGERS: [(Trigger, &str); 3] = [(Trigger::OnClick, "on-click"), (Trigger::WithPrev, "with-previous"), (Trigger::AfterPrev, "after-previous")];

fn name_of<T: PartialEq + Copy>(table: &[(T, &'static str)], v: T) -> &'static str {
    table.iter().find(|(k, _)| *k == v).map_or("", |(_, n)| n)
}

fn value_of<T: Copy>(table: &[(T, &'static str)], s: &str) -> Option<T> {
    table.iter().find(|(_, n)| *n == s).map(|(k, _)| *k)
}

// --- transitions ------------------------------------------------------------------------

/// The nearest SMIL transition: type, subtype and whether it runs in
/// reverse.
fn smil_of(t: &Transition) -> Option<(&'static str, &'static str, bool)> {
    let from = match t.dir {
        Dir::Right => "fromLeft",
        Dir::Up => "fromBottom",
        Dir::Down => "fromTop",
        _ => "fromRight",
    };
    Some(match t.kind {
        TransKind::None => return None,
        TransKind::Push | TransKind::Cube => ("pushWipe", from, false),
        TransKind::Cover => ("slideWipe", from, false),
        TransKind::Reveal => ("slideWipe", from, true),
        TransKind::Wipe => match t.dir {
            Dir::Left => ("barWipe", "leftToRight", true),
            Dir::Up => ("barWipe", "topToBottom", true),
            Dir::Down => ("barWipe", "topToBottom", false),
            _ => ("barWipe", "leftToRight", false),
        },
        TransKind::Split => ("barnDoorWipe", if matches!(t.dir, Dir::HorzIn | Dir::HorzOut) { "horizontal" } else { "vertical" }, matches!(t.dir, Dir::HorzIn | Dir::VertIn)),
        TransKind::Doors => ("barnDoorWipe", if t.dir == Dir::Horz { "horizontal" } else { "vertical" }, false),
        TransKind::Zoom => ("irisWipe", "rectangle", t.dir == Dir::In),
        TransKind::Dissolve => ("dissolve", "defaultSubtype", false),
        TransKind::Blinds => ("blindsWipe", if t.dir == Dir::Horz { "horizontal" } else { "vertical" }, false),
        TransKind::Checker => ("checkerBoardWipe", if t.dir == Dir::Vert { "down" } else { "across" }, false),
        _ => ("fade", "crossfade", false),
    })
}

/// The transition of a SMIL type and subtype.
fn from_smil(ty: &str, sub: &str, rev: bool) -> (TransKind, Dir) {
    let side = match sub {
        "fromLeft" => Dir::Right,
        "fromBottom" => Dir::Up,
        "fromTop" => Dir::Down,
        _ => Dir::Left,
    };
    match ty {
        "fade" => (TransKind::Fade, Dir::Left),
        "pushWipe" => (TransKind::Push, side),
        "slideWipe" => (if rev { TransKind::Reveal } else { TransKind::Cover }, side),
        "barWipe" => (TransKind::Wipe, match (sub == "topToBottom", rev) {
            (false, true) => Dir::Left,
            (false, false) => Dir::Right,
            (true, true) => Dir::Up,
            (true, false) => Dir::Down,
        }),
        "barnDoorWipe" => (TransKind::Split, match (sub == "horizontal", rev) {
            (false, false) => Dir::VertOut,
            (false, true) => Dir::VertIn,
            (true, false) => Dir::HorzOut,
            (true, true) => Dir::HorzIn,
        }),
        "irisWipe" => (TransKind::Zoom, if rev { Dir::In } else { Dir::Out }),
        "dissolve" => (TransKind::Dissolve, Dir::Left),
        "blindsWipe" => (TransKind::Blinds, if sub == "horizontal" { Dir::Horz } else { Dir::Vert }),
        "checkerBoardWipe" => (TransKind::Checker, if sub == "down" { Dir::Vert } else { Dir::Horz }),
        _ => (TransKind::Other, Dir::Left),
    }
}

/// The page style properties of a transition and of how the slide
/// advances.
pub(crate) fn transition_props(t: &Transition) -> String {
    let mut s = String::new();
    if let Some((ty, sub, rev)) = smil_of(t) {
        let _ = write!(s, r#" smil:type="{ty}" smil:subtype="{sub}""#);
        if rev {
            s.push_str(r#" smil:direction="reverse""#);
        }
        let speed = if t.dur < 0.75 {
            "fast"
        } else if t.dur < 1.5 {
            "medium"
        } else {
            "slow"
        };
        let _ = write!(s, r#" presentation:transition-speed="{speed}""#);
    }
    match t.after {
        Some(a) => {
            let _ = write!(s, r#" presentation:transition-type="automatic" presentation:duration="PT{}S""#, a.max(0.0));
        }
        None if t.kind != TransKind::None => s.push_str(r#" presentation:transition-type="manual""#),
        None => {}
    }
    if (Transition { raw: None, ..t.clone() }) != Transition::default() {
        let _ = write!(s, r#" apx:transition="{}" apx:transition-direction="{}" apx:transition-duration="{}""#, name_of(&KINDS, t.kind), name_of(&DIRS, t.dir), t.dur);
        if !t.on_click {
            s.push_str(r#" apx:advance-on-click="false""#);
        }
    }
    s
}

/// Whether a page style property is one of the transition's.
pub(crate) fn is_transition_prop(k: &str) -> bool {
    k.starts_with("smil:") || k.starts_with("presentation:transition-") || k == "presentation:duration" || k.starts_with("apx:")
}

/// The transition of a page style (`get` looks a property up).
pub(crate) fn read_transition(get: &dyn Fn(&str) -> Option<String>) -> Transition {
    let mut t = Transition::default();
    if let Some(ty) = get("smil:type") {
        let sub = get("smil:subtype").unwrap_or_default();
        (t.kind, t.dir) = from_smil(&ty, &sub, get("smil:direction").as_deref() == Some("reverse"));
        t.dur = match get("presentation:transition-speed").as_deref() {
            Some("slow") => 2.0,
            Some("medium") => 1.0,
            Some("fast") => 0.5,
            _ => t.dur,
        };
    }
    if get("presentation:transition-type").as_deref() == Some("automatic") {
        t.after = Some(get("presentation:duration").and_then(|d| secs(&d)).unwrap_or(0.0).max(0.0));
    }
    if let Some(k) = get("apx:transition").and_then(|v| value_of(&KINDS, &v)) {
        t.kind = k;
    }
    if let Some(d) = get("apx:transition-direction").and_then(|v| value_of(&DIRS, &v)) {
        t.dir = d;
    }
    if let Some(d) = get("apx:transition-duration").and_then(|v| v.parse::<f32>().ok()).filter(|d| d.is_finite()) {
        t.dur = d;
    }
    t.on_click = get("apx:advance-on-click").as_deref() != Some("false");
    t
}

// --- animations -------------------------------------------------------------------------

/// LibreOffice's preset id of an effect.
fn preset_id(class: Class, effect: Effect) -> Option<String> {
    let base = match class {
        Class::Entrance => "entrance",
        Class::Exit => "exit",
        Class::Emphasis => "emphasis",
        Class::Path => return Some("ooo-motionpath-right".into()),
        Class::Media => return Some("ooo-media-start".into()),
    };
    let name = match (class, effect) {
        (_, Effect::Other) => return None,
        (Class::Exit, Effect::Appear) => "disappear",
        (Class::Exit, Effect::Fade) => "fade-out",
        (_, Effect::Fade) => "fade-in",
        (Class::Exit, Effect::Fly) => "fly-out",
        (_, Effect::Fly) => "fly-in",
        (_, Effect::Grow) => "grow-and-shrink",
        (_, Effect::GrowTurn) => "turn-and-grow",
        (_, e) => name_of(&EFFECTS, e),
    };
    Some(format!("ooo-{base}-{name}"))
}

/// The effect of a LibreOffice preset id.
fn effect_of_preset(id: &str) -> Effect {
    let name = id.strip_prefix("ooo-").and_then(|r| r.split_once('-')).map_or("", |(_, n)| n);
    match name {
        "appear" | "disappear" => Effect::Appear,
        "fade-in" | "fade-out" => Effect::Fade,
        "fly-in" | "fly-out" => Effect::Fly,
        "grow-and-shrink" => Effect::Grow,
        "turn-and-grow" => Effect::GrowTurn,
        n => value_of(&EFFECTS, n).unwrap_or(Effect::Other),
    }
}

/// LibreOffice's preset subtype of a direction.
fn sub_type(d: Dir) -> &'static str {
    match d {
        Dir::Left => "from-left",
        Dir::Right => "from-right",
        Dir::Up => "from-top",
        Dir::Down => "from-bottom",
        d => name_of(&DIRS, d),
    }
}

fn dir_of_sub_type(s: &str) -> Option<Dir> {
    match s {
        "from-left" => Some(Dir::Left),
        "from-right" => Some(Dir::Right),
        "from-top" => Some(Dir::Up),
        "from-bottom" => Some(Dir::Down),
        s => value_of(&DIRS, s),
    }
}

/// The SMIL behaviors of an effect on element `x`.
fn behaviors(out: &mut String, a: &Anim, x: &str) {
    let d = format!("{}s", a.dur.max(0.001));
    let tgt = format!(r#"smil:targetElement="{}""#, esc(x));
    let set = |out: &mut String, begin: &str, attr: &str, to: &str| {
        let _ = write!(out, r#"<anim:set smil:begin="{begin}" smil:dur="0.001s" smil:fill="hold" {tgt} smil:attributeName="{attr}" smil:to="{to}"/>"#);
    };
    let filter = |out: &mut String, ty: &str, sub: &str, mode: &str| {
        let _ = write!(out, r#"<anim:transitionFilter smil:dur="{d}" {tgt} smil:type="{ty}" smil:subtype="{sub}"{mode}/>"#);
    };
    let animate = |out: &mut String, attr: &str, values: &str| {
        let _ = write!(out, r#"<anim:animate smil:dur="{d}" smil:fill="hold" {tgt} smil:attributeName="{attr}" smil:values="{values}" smil:keyTimes="0;1" presentation:additive="base"/>"#);
    };
    let transform = |out: &mut String, ty: &str, how: &str| {
        let _ = write!(out, r#"<anim:animateTransform smil:dur="{d}" smil:fill="hold" {tgt} svg:type="{ty}" {how}/>"#);
    };
    match a.class {
        Class::Entrance => {
            set(out, "0s", "visibility", "visible");
            match a.effect {
                Effect::Appear => {}
                Effect::Fly | Effect::Bounce | Effect::Float => {
                    let (attr, from) = match a.dir {
                        Dir::Left => ("x", "0-width/2"),
                        Dir::Right => ("x", "1+width/2"),
                        Dir::Up => ("y", "0-height/2"),
                        _ => ("y", "1+height/2"),
                    };
                    animate(out, attr, &format!("{from};{attr}"));
                    if a.effect == Effect::Float {
                        filter(out, "fade", "crossfade", "");
                    }
                }
                Effect::Wipe => filter(out, "barWipe", if matches!(a.dir, Dir::Up | Dir::Down) { "topToBottom" } else { "leftToRight" }, ""),
                Effect::Split => filter(out, "barnDoorWipe", if matches!(a.dir, Dir::HorzIn | Dir::HorzOut) { "horizontal" } else { "vertical" }, ""),
                Effect::Zoom | Effect::GrowTurn => {
                    animate(out, "width", "0;width");
                    animate(out, "height", "0;height");
                    if a.effect == Effect::GrowTurn {
                        transform(out, "rotate", r#"smil:by="90""#);
                    }
                }
                _ => filter(out, "fade", "crossfade", ""),
            }
        }
        Class::Exit => {
            if a.effect != Effect::Appear {
                filter(out, "fade", "crossfade", r#" smil:mode="out""#);
            }
            set(out, &d, "visibility", "hidden");
        }
        Class::Emphasis => match a.effect {
            Effect::Spin => transform(out, "rotate", r#"smil:by="360""#),
            Effect::Teeter => transform(out, "rotate", r#"smil:by="5" smil:autoReverse="true""#),
            Effect::Pulse => transform(out, "scale", r#"smil:to="1.05,1.05" smil:autoReverse="true""#),
            Effect::Blink => {
                let _ = write!(out, r#"<anim:set smil:begin="0s" smil:dur="{d}" {tgt} smil:attributeName="visibility" smil:to="hidden"/>"#);
            }
            Effect::Transparency => {
                let _ = write!(out, r#"<anim:set smil:begin="0s" smil:dur="{d}" smil:fill="hold" {tgt} smil:attributeName="opacity" smil:to="0.5"/>"#);
            }
            _ => transform(out, "scale", r#"smil:to="1.5,1.5""#),
        },
        Class::Path => {
            let _ = write!(out, r#"<anim:animateMotion smil:dur="{d}" smil:fill="hold" {tgt} svg:path="M 0 0 L 0.25 0"/>"#);
        }
        Class::Media => {
            let _ = write!(out, r#"<anim:command smil:begin="0s" {tgt} anim:command="play"/>"#);
        }
    }
}

/// The first element an effect animates.
fn first_target(n: &Node) -> Option<&str> {
    for k in &n.kids {
        if let Some(t) = exact(k, "smil:targetElement").or_else(|| first_target(k)) {
            return Some(t);
        }
    }
    None
}

/// One effect: as read (with its target's new id) or written from the model.
fn write_effect(out: &mut String, a: &Anim, x: &str, begin: f32) {
    let _ = write!(out, r#"<anim:par smil:begin="{}s">"#, begin.max(0.0));
    if let Some(r) = a.raw.as_ref().filter(|r| r.src == Src::Odp) {
        match first_target(&crate::drawingml::read_fragment(&r.xml)) {
            Some(old) => out.push_str(&r.xml.replace(&format!(r#"smil:targetElement="{}""#, esc(old)), &format!(r#"smil:targetElement="{}""#, esc(x)))),
            None => out.push_str(&r.xml),
        }
        out.push_str("</anim:par>");
        return;
    }
    let _ = write!(
        out,
        r#"<anim:par smil:begin="{}s" smil:fill="hold" presentation:node-type="{}" presentation:preset-class="{}""#,
        a.delay.max(0.0),
        name_of(&TRIGGERS, a.trigger),
        name_of(&CLASSES, a.class)
    );
    if let Some(id) = preset_id(a.class, a.effect) {
        let _ = write!(out, r#" presentation:preset-id="{id}""#);
    }
    if matches!(a.effect, Effect::Fly | Effect::Float | Effect::Bounce | Effect::Wipe | Effect::Split | Effect::Zoom) {
        let _ = write!(out, r#" presentation:preset-sub-type="{}""#, sub_type(a.dir));
    }
    let _ = write!(out, r#" apx:effect="{}" apx:preset="{}" apx:subtype="{}" apx:dir="{}" apx:dur="{}""#, name_of(&EFFECTS, a.effect), a.preset, a.subtype, name_of(&DIRS, a.dir), a.dur);
    if let Some((p0, p1)) = a.paras {
        let _ = write!(out, r#" apx:paras="{p0} {p1}""#);
    }
    out.push('>');
    behaviors(out, a, x);
    out.push_str("</anim:par></anim:par>");
}

/// Writes a page's animations: the main sequence, each click a step, and
/// `kept`, the page's other sequences as read. `target` gives an object's
/// id in the file (animations of objects not written are left out).
pub(crate) fn write_timing(out: &mut String, t: &Timing, target: &dyn Fn(u32) -> Option<String>, kept: &[String]) {
    let mut main = String::new();
    for (k, step) in t.steps().iter().enumerate() {
        let mut body = String::new();
        for (i, start, _) in t.schedule(step) {
            let a = &t.anims[i];
            if let Some(x) = target(a.shape) {
                write_effect(&mut body, a, &x, start - a.delay.max(0.0));
            }
        }
        if !body.is_empty() {
            let begin = if k == 0 && t.auto_start() { "0s" } else { "next" };
            let _ = write!(main, r#"<anim:par smil:begin="{begin}">{body}</anim:par>"#);
        }
    }
    if main.is_empty() && kept.is_empty() {
        return;
    }
    out.push_str(r#"<anim:par presentation:node-type="timing-root">"#);
    if !main.is_empty() {
        let _ = write!(out, r#"<anim:seq presentation:node-type="main-sequence">{main}</anim:seq>"#);
    }
    for k in kept {
        out.push_str(k);
    }
    out.push_str("</anim:par>");
}

/// The timing root of a page.
pub(crate) fn timing_root(page: &Node) -> Option<&Node> {
    page.children("par").find(|p| p.attr("presentation:node-type") == Some("timing-root"))
}

/// Whether a timing node is the main sequence.
pub(crate) fn is_main(n: &Node) -> bool {
    n.attr("presentation:node-type") == Some("main-sequence")
}

fn is_effect(n: &Node) -> bool {
    n.local() == "par" && (n.attr("presentation:preset-class").is_some() || (matches!(n.attr("presentation:node-type"), Some("on-click" | "with-previous" | "after-previous")) && n.child("par").is_none()))
}

fn effects<'a>(n: &'a Node, out: &mut Vec<&'a Node>) {
    for k in &n.kids {
        if is_effect(k) {
            out.push(k);
        } else {
            effects(k, out);
        }
    }
}

/// The longest duration among an effect's behaviors.
fn longest(n: &Node) -> Option<f32> {
    let mut best = exact(n, "smil:dur").and_then(secs);
    for k in &n.kids {
        if let Some(d) = longest(k) {
            best = Some(best.map_or(d, |b| b.max(d)));
        }
    }
    best
}

/// The animations of a page's main sequence; `ids` maps the ids of the
/// file's elements to our objects.
pub(crate) fn read_timing(page: &Node, ids: &HashMap<String, u32>) -> Timing {
    let mut t = Timing::default();
    let Some(main) = timing_root(page).and_then(|r| r.kids.iter().find(|k| is_main(k))) else { return t };
    let mut all = Vec::new();
    effects(main, &mut all);
    for e in all {
        let Some(&shape) = first_target(e).and_then(|x| ids.get(x)) else { continue };
        let class = exact(e, "presentation:preset-class").and_then(|c| value_of(&CLASSES, c)).unwrap_or(Class::Entrance);
        let ours = exact(e, "apx:effect");
        let effect = ours.and_then(|v| value_of(&EFFECTS, v)).unwrap_or_else(|| exact(e, "presentation:preset-id").map_or(Effect::Other, effect_of_preset));
        let mut a = Anim::new(shape, class, effect);
        a.preset = exact(e, "apx:preset").and_then(|v| v.parse().ok()).unwrap_or(a.preset);
        a.subtype = exact(e, "apx:subtype").and_then(|v| v.parse().ok()).unwrap_or(0);
        a.dir = exact(e, "apx:dir").and_then(|v| value_of(&DIRS, v)).or_else(|| exact(e, "presentation:preset-sub-type").and_then(dir_of_sub_type)).unwrap_or(Dir::Left);
        a.trigger = exact(e, "presentation:node-type").and_then(|v| value_of(&TRIGGERS, v)).unwrap_or(Trigger::OnClick);
        a.delay = exact(e, "smil:begin").and_then(secs).unwrap_or(0.0).max(0.0);
        a.dur = match exact(e, "apx:dur").and_then(|v| v.parse::<f32>().ok()).filter(|d| d.is_finite()) {
            Some(d) => d,
            None => longest(e).filter(|d| *d > 0.001).unwrap_or(if effect == Effect::Appear { 0.0 } else { 0.5 }),
        };
        a.paras = exact(e, "apx:paras").and_then(|v| v.split_once(' ')).and_then(|(p, q)| Some((p.parse().ok()?, q.parse().ok()?)));
        if ours.is_none() {
            let mut s = String::new();
            crate::pptx_shape::node_xml(&mut s, e);
            a.raw = Some(Raw { src: Src::Odp, xml: Arc::from(s) });
        }
        t.anims.push(a);
    }
    t
}

// --- show settings ----------------------------------------------------------------------

/// The attributes of `presentation:settings` for the show.
pub(crate) fn show_attrs(s: &ShowSettings) -> String {
    let mut a = format!(r#" presentation:endless="{}" presentation:force-manual="{}""#, s.looped, !s.use_timings);
    if s.kiosk {
        a.push_str(r#" apx:kiosk="true""#);
    }
    a
}

/// Whether a show needs settings written.
pub(crate) fn show_set(s: &ShowSettings) -> bool {
    s.looped || s.kiosk || !s.use_timings
}

/// The show of `presentation:settings`.
pub(crate) fn read_show(n: &Node) -> ShowSettings {
    let on = |k: &str| exact(n, k) == Some("true");
    ShowSettings::new(on("presentation:endless"), on("apx:kiosk"), !on("presentation:force-manual"))
}

/// Start tag `tag` without attribute `key`.
fn drop_attr(tag: &str, key: &str) -> String {
    let pat = format!(" {key}=\"");
    match tag.find(&pat) {
        Some(i) => {
            let at = i + pat.len();
            let end = tag[at..].find('"').map_or(tag.len(), |e| at + e + 1);
            format!("{}{}", &tag[..i], &tag[end..])
        }
        None => tag.to_string(),
    }
}

/// `xml` (what follows a presentation's pages) with the show's settings:
/// those of its `presentation:settings` replaced, or a new one.
pub(crate) fn set_show(xml: &str, s: &ShowSettings) -> String {
    let Some(i) = xml.find("<presentation:settings") else {
        return format!("<presentation:settings{}/>{xml}", show_attrs(s));
    };
    let Some(close) = xml[i..].find('>').map(|e| i + e) else { return xml.to_string() };
    let end = if xml[..close].ends_with('/') { close - 1 } else { close };
    let mut tag = xml[i..end].to_string();
    for k in ["presentation:endless", "presentation:force-manual", "apx:kiosk"] {
        tag = drop_attr(&tag, k);
    }
    format!("{}{tag}{}{}", &xml[..i], show_attrs(s), &xml[end..])
}

// --- themes -----------------------------------------------------------------------------

const SLOTS: [&str; 12] = ["dark1", "light1", "dark2", "light2", "accent1", "accent2", "accent3", "accent4", "accent5", "accent6", "hyperlink", "followed-hyperlink"];

/// A master's theme (LibreOffice's `loext:theme`, with its fonts).
pub(crate) fn theme_xml(t: &Theme) -> String {
    let f = &t.fonts;
    let mut s = format!(r#"<loext:theme loext:name="{}" apx:font-scheme="{}" apx:major-font="{}" apx:minor-font="{}"><loext:theme-colors loext:name="{}">"#, esc(&t.name), esc(&f.name), esc(&f.major), esc(&f.minor), esc(&t.colors.name));
    for (k, c) in SLOTS.iter().zip(t.colors.colors) {
        let _ = write!(s, r##"<loext:color loext:name="{k}" loext:color="#{:06x}"/>"##, c & 0xFFFFFF);
    }
    s.push_str("</loext:theme-colors>");
    for (major, script, face) in &f.scripts {
        let _ = write!(s, r#"<apx:font apx:major="{major}" apx:script="{}" apx:typeface="{}"/>"#, esc(script), esc(face));
    }
    s.push_str("</loext:theme>");
    s
}

/// The theme of a master page.
pub(crate) fn read_theme(master: &Node) -> Option<Theme> {
    let th = master.child("theme")?;
    let mut t = Theme::builtin(0);
    if let Some(n) = exact(th, "loext:name") {
        t.name = Arc::from(n);
    }
    if let Some(cs) = th.child("theme-colors") {
        let mut colors = t.colors.colors;
        for c in cs.children("color") {
            let slot = exact(c, "loext:name").and_then(|n| SLOTS.iter().position(|s| *s == n));
            let rgb = exact(c, "loext:color").and_then(|v| u32::from_str_radix(v.trim_start_matches('#'), 16).ok());
            if let (Some(i), Some(rgb)) = (slot, rgb) {
                colors[i] = rgb;
            }
        }
        t.colors = ColorScheme { name: Arc::from(exact(cs, "loext:name").unwrap_or(&t.colors.name)), colors };
    }
    let font = |k: &str, d: &Arc<str>| exact(th, k).map_or_else(|| d.clone(), Arc::from);
    let scripts = th.children("font").filter_map(|f| Some((exact(f, "apx:major") == Some("true"), Arc::from(exact(f, "apx:script")?), Arc::from(exact(f, "apx:typeface")?)))).collect();
    t.fonts = FontScheme { name: font("apx:font-scheme", &t.fonts.name), major: font("apx:major-font", &t.fonts.major), minor: font("apx:minor-font", &t.fonts.minor), scripts };
    Some(t)
}

/// `xml` with namespace declarations `ns` on its root element where
/// they are missing.
pub(crate) fn ensure_ns(xml: &str, ns: &[(&str, &str)]) -> String {
    let Some(start) = xml.match_indices('<').map(|(i, _)| i).find(|&i| xml[i + 1..].starts_with(|c: char| c.is_ascii_alphabetic())) else { return xml.to_string() };
    let Some(end) = xml[start..].find('>').map(|e| start + e) else { return xml.to_string() };
    let tag = &xml[start..end];
    let mut add = String::new();
    for (p, uri) in ns {
        if !tag.contains(&format!("xmlns:{p}=")) {
            let _ = write!(add, r#" xmlns:{p}="{uri}""#);
        }
    }
    if add.is_empty() {
        return xml.to_string();
    }
    let at = if tag.ends_with('/') { end - 1 } else { end };
    format!("{}{add}{}", &xml[..at], &xml[at..])
}

/// Replaces (or adds) the theme of master page `name` in `styles`.
pub(crate) fn set_master_theme(styles: &str, name: &str, theme: &Theme) -> String {
    let pat = format!(r#"style:name="{}""#, esc(name));
    let mut from = 0;
    while let Some(i) = styles[from..].find("<style:master-page").map(|i| from + i) {
        let Some(close) = styles[i..].find('>').map(|e| i + e) else { break };
        from = close;
        if !styles[i..close].contains(&pat) {
            continue;
        }
        let x = theme_xml(theme);
        if styles[..close].ends_with('/') {
            return format!("{}>{x}</style:master-page>{}", &styles[..close - 1], &styles[close + 1..]);
        }
        let end = styles[close..].find("</style:master-page>").map_or(styles.len(), |e| close + e);
        let inner = &styles[close + 1..end];
        let inner = match (inner.find("<loext:theme"), inner.find("</loext:theme>")) {
            (Some(a), Some(b)) if a < b => format!("{}{}", &inner[..a], &inner[b + "</loext:theme>".len()..]),
            _ => inner.to_string(),
        };
        return format!("{}{x}{inner}{}", &styles[..close + 1], &styles[end..]);
    }
    styles.to_string()
}

#[cfg(test)]
mod tests {
    use apex_slides::anim::{Anim, Class, Dir, Effect, Timing, TransKind, Transition, Trigger};
    use apex_slides::fill::{Fill, Line};
    use apex_slides::{Deck, ShowSettings};

    fn deck() -> Deck {
        let mut d = apex_slides::build::new_deck(0);
        let n = d.slides.len();
        apex_slides::edit::add_slide(&mut d, n, 0, 1);
        d
    }

    fn rt(d: &Deck) -> Deck {
        crate::odp::read(&crate::odp_write::write(d)).unwrap()
    }

    fn part(d: &Deck, name: &str) -> String {
        crate::zip::ZipReader::new(&crate::odp_write::write(d)).unwrap().read_string(name).unwrap()
    }

    #[test]
    fn transitions_come_back() {
        let mut kinds: Vec<(TransKind, Dir)> = Vec::new();
        for k in TransKind::ALL {
            let dirs = k.dirs();
            if dirs.is_empty() {
                kinds.push((k, Dir::Left));
            }
            kinds.extend(dirs.iter().map(|&d| (k, d)));
        }
        for (i, (kind, dir)) in kinds.into_iter().enumerate() {
            let mut d = deck();
            let (on_click, after) = if i % 2 == 0 { (false, Some(3.5)) } else { (true, None) };
            let t = Transition { kind, dir, dur: 1.25, on_click, after, raw: None };
            d.slide_mut(0).transition = t.clone();
            let back = rt(&d);
            assert_eq!(back.slides[0].transition, t, "{kind:?} {dir:?}");
            assert_eq!(back.slides[1].transition, Transition::default());
            // From a file of ours too.
            let mut again = back.clone();
            again.slide_mut(1).transition = t.clone();
            assert_eq!(rt(&again).slides[1].transition, t, "{kind:?} {dir:?}");
        }
        // Other suites see SMIL.
        let mut d = deck();
        d.slide_mut(0).transition = Transition { kind: TransKind::Cube, dir: Dir::Up, dur: 1.0, on_click: true, after: Some(2.0), raw: None };
        let c = part(&d, "content.xml");
        assert!(c.contains("smil:type=") && c.contains(r#"presentation:transition-type="automatic""#) && c.contains(r#"apx:transition="cube""#), "{c}");
    }

    fn anims(ids: &[u32]) -> Vec<Anim> {
        let mut out = Vec::new();
        let classes = [(Class::Entrance, &Effect::ENTRANCE[..]), (Class::Exit, &Effect::ENTRANCE[..]), (Class::Emphasis, &Effect::EMPHASIS[..])];
        let dirs = [Dir::Left, Dir::Right, Dir::Up, Dir::Down, Dir::VertIn, Dir::HorzOut, Dir::In];
        let triggers = [Trigger::OnClick, Trigger::WithPrev, Trigger::AfterPrev];
        let mut i = 0;
        for (class, effects) in classes {
            for &e in effects {
                let mut a = Anim::new(ids[i % ids.len()], class, e);
                a.dir = dirs[i % dirs.len()];
                a.trigger = triggers[i % 3];
                a.delay = (i % 4) as f32 * 0.25;
                a.dur = 0.5 + (i % 5) as f32 * 0.5;
                if i % 6 == 0 {
                    a.paras = Some((1, 2));
                }
                out.push(a);
                i += 1;
            }
        }
        out
    }

    #[test]
    fn animations_come_back() {
        let mut d = deck();
        let ids: Vec<u32> = d.slides[1].objects.iter().map(|o| o.id).collect();
        let list = anims(&ids);
        d.slide_mut(1).timing = Timing { anims: list.clone(), raw: None, extra: None };
        let back = rt(&d);
        assert_eq!(back.slides[1].timing.anims, list);
        assert!(back.slides[0].timing.anims.is_empty());
        // Objects keep their ids.
        let ids_back: Vec<u32> = back.slides[1].objects.iter().map(|o| o.id).collect();
        assert_eq!(ids_back, ids);
        // Moved earlier, removed, changed: in a file read back.
        let mut d = back;
        let mut l = list.clone();
        l.swap(0, 3);
        l.remove(5);
        l[2].dur = 3.0;
        l[2].trigger = Trigger::AfterPrev;
        d.slide_mut(1).timing = Timing { anims: l.clone(), raw: None, extra: None };
        let back = rt(&d);
        assert_eq!(back.slides[1].timing.anims, l);
        // All removed.
        let mut d = back;
        d.slide_mut(1).timing = Timing::default();
        assert!(rt(&d).slides[1].timing.anims.is_empty());
        // Other suites see the presets.
        let mut d = deck();
        d.slide_mut(1).timing = Timing { anims: vec![Anim::new(ids[0], Class::Entrance, Effect::Fly)], raw: None, extra: None };
        let c = part(&d, "content.xml");
        assert!(c.contains(r#"presentation:preset-id="ooo-entrance-fly-in""#) && c.contains("smil:targetElement=") && c.contains("main-sequence"), "{c}");
    }

    #[test]
    fn show_settings_come_back() {
        for (l, k, u) in [(true, false, true), (false, true, true), (false, false, false), (true, true, false)] {
            let mut d = deck();
            d.show.looped = l;
            d.show.kiosk = k;
            d.show.use_timings = u;
            let back = rt(&d);
            assert_eq!((back.show.looped, back.show.kiosk, back.show.use_timings), (l, k, u));
            let mut d = back;
            d.show.looped = !l;
            d.show.use_timings = !u;
            let back = rt(&d);
            assert_eq!((back.show.looped, back.show.kiosk, back.show.use_timings), (!l, k, !u));
        }
        assert_eq!(rt(&deck()).show, ShowSettings::new(false, false, true));
    }

    #[test]
    fn master_themes_come_back() {
        let gray = apex_doc::Theme::builtin(2);
        assert_eq!(&*gray.name, "Grayscale");
        let mut d = deck();
        d.master_mut(0).theme.theme = gray.clone();
        assert_eq!(rt(&d).masters[0].theme.theme, gray);
        // In a file read back, the master changed.
        let mut d = rt(&deck());
        assert_ne!(d.masters[0].theme.theme, gray);
        d.master_mut(0).theme.theme = gray.clone();
        assert_eq!(rt(&d).masters[0].theme.theme, gray);
        let styles = part(&d, "styles.xml");
        assert!(styles.contains(r#"<loext:theme loext:name="Grayscale""#) && styles.contains("xmlns:loext="), "{styles}");
    }

    /// The title placeholder of slide 0 changed by `f`, read back.
    fn changed(f: &dyn Fn(&mut apex_slides::Object)) -> apex_slides::Object {
        let mut d = deck();
        let o = d.slide_mut(0).objects.iter_mut().find(|o| o.is_title()).unwrap();
        f(o);
        let back = rt(&d);
        back.slides[0].objects.iter().find(|o| o.is_title()).unwrap().clone()
    }

    #[test]
    fn placeholder_formats_come_back() {
        let base = changed(&|_| {});
        assert!(base.sp.fill.is_none() && base.sp.line.is_none(), "{:?} {:?}", base.sp.fill, base.sp.line);
        assert_eq!(changed(&|o| o.sp.fill = Some(Fill::None)).sp.fill, Some(Fill::None));
        assert_eq!(changed(&|o| o.sp.line = Some(Line::none())).sp.line, Some(Line::none()));
        assert_eq!(changed(&|o| o.sp.line = Some(Line::default())).sp.line, Some(Line::default()));
        for w in [0.25, 0.5, 0.75, 1.0, 1.5, 2.25, 3.0, 4.5, 6.0] {
            let l = Line { width: Some(w), ..Default::default() };
            assert_eq!(changed(&|o| o.sp.line = Some(l.clone())).sp.line, Some(l), "{w}");
        }
        // Flips and an own frame (aligned where it was).
        let frame = base.sp.frame.unwrap();
        for (h, v) in [(true, false), (false, true), (true, true)] {
            let o = changed(&|o| {
                let mut f = frame;
                f.flip_h = h;
                f.flip_v = v;
                o.sp.frame = Some(f);
            });
            let f = o.sp.frame.unwrap();
            assert_eq!((f.flip_h, f.flip_v), (h, v));
        }
        let o = changed(&|o| o.sp.frame = Some(frame));
        assert_ne!(format!("{:?}", o.raw), format!("{:?}", base.raw));
        // Shapes of a file read back.
        let mut d = rt(&deck());
        let o = d.slide_mut(0).objects.iter_mut().find(|o| o.is_title()).unwrap();
        o.touch();
        o.sp.line = Some(Line { width: Some(4.5), ..Default::default() });
        let mut f = o.sp.frame.unwrap();
        f.flip_v = true;
        o.sp.frame = Some(f);
        let back = rt(&d);
        let o = back.slides[0].objects.iter().find(|o| o.is_title()).unwrap();
        assert_eq!(o.sp.line, Some(Line { width: Some(4.5), ..Default::default() }));
        assert!(o.sp.frame.unwrap().flip_v);
    }
}
