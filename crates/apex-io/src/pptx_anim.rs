//! Slide transitions (`p:transition`) and animations (`p:timing`): the
//! common ones read into `apex_slides::anim` and written back; the whole
//! elements are kept as read while unchanged.

use std::fmt::Write as _;
use std::sync::Arc;

use apex_slides::anim::{Anim, Class, Dir, Effect, Timing, TransKind, Transition, Trigger};
use apex_slides::object::{Raw, Src};

use crate::drawingml::{Node, TEXT};
use crate::pptx_shape::node_xml;

const NS_P14: &str = "http://schemas.microsoft.com/office/powerpoint/2010/main";
const NS_P159: &str = "http://schemas.microsoft.com/office/powerpoint/2015/09/main";
const NS_P15: &str = "http://schemas.microsoft.com/office/powerpoint/2012/main";
const NS_MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";

fn dir_of(s: Option<&str>) -> Dir {
    match s {
        Some("r") => Dir::Right,
        Some("u") => Dir::Up,
        Some("d") => Dir::Down,
        Some("in") => Dir::In,
        Some("out") => Dir::Out,
        _ => Dir::Left,
    }
}

fn dir_name(d: Dir) -> &'static str {
    match d {
        Dir::Right => "r",
        Dir::Up => "u",
        Dir::Down => "d",
        Dir::In | Dir::HorzIn | Dir::VertIn => "in",
        Dir::Out | Dir::HorzOut | Dir::VertOut => "out",
        Dir::Left => "l",
        Dir::Horz => "horz",
        Dir::Vert => "vert",
    }
}

/// A `p:transition` (the one PowerPoint's fallback holds, or its choice
/// when that is a morph).
pub(crate) fn read_transition(n: &Node, raw: &str) -> Transition {
    let mut t = Transition { raw: Some(Raw { src: Src::Pptx, xml: Arc::from(raw) }), ..Default::default() };
    let tr = if n.local() == "AlternateContent" {
        let choice = n.child("Choice").and_then(|c| c.child("transition"));
        // Choices we play (PowerPoint 2010 and later ones); else the fallback.
        let ours = ["morph", "doors", "prism", "flip", "switch", "prstTrans"];
        if choice.is_some_and(|c| c.kids.iter().any(|k| ours.contains(&k.local()))) { choice } else { n.child("Fallback").and_then(|c| c.child("transition")).or(choice) }
    } else {
        Some(n)
    };
    let Some(tr) = tr else { return t };
    let choice_dur = if n.local() == "AlternateContent" { n.child("Choice").and_then(|c| c.child("transition")).and_then(|c| c.num("dur")) } else { None };
    t.dur = match choice_dur.or_else(|| tr.num("dur")) {
        Some(ms) => (ms / 1000.0) as f32,
        None => match tr.attr("spd") {
            Some("slow") => 1.0,
            Some("med") => 0.75,
            _ => 0.5,
        },
    };
    t.on_click = tr.attr("advClick").is_none_or(|v| v != "0" && v != "false");
    t.after = tr.num("advTm").map(|ms| (ms / 1000.0) as f32);
    let Some(k) = tr.kids.iter().find(|k| k.name != TEXT && k.local() != "sndAc" && k.local() != "extLst") else {
        t.kind = TransKind::None;
        return t;
    };
    let dir = k.attr("dir");
    (t.kind, t.dir) = match k.local() {
        "fade" => (TransKind::Fade, Dir::Left),
        "push" => (TransKind::Push, dir_of(dir)),
        "wipe" => (TransKind::Wipe, dir_of(dir)),
        "cover" => (TransKind::Cover, dir_of(dir)),
        "pull" => (TransKind::Reveal, dir_of(dir)),
        "zoom" => (TransKind::Zoom, dir_of(dir.or(Some("out")))),
        "split" => {
            let horz = k.attr("orient").is_none_or(|o| o == "horz");
            let out = dir.is_none_or(|d| d == "out");
            (TransKind::Split, match (horz, out) {
                (true, true) => Dir::HorzOut,
                (true, false) => Dir::HorzIn,
                (false, true) => Dir::VertOut,
                (false, false) => Dir::VertIn,
            })
        }
        "dissolve" => (TransKind::Dissolve, Dir::Left),
        "blinds" => (TransKind::Blinds, if dir == Some("vert") { Dir::Vert } else { Dir::Horz }),
        "checker" => (TransKind::Checker, if dir == Some("vert") { Dir::Vert } else { Dir::Horz }),
        "doors" => (TransKind::Doors, if dir == Some("horz") { Dir::Horz } else { Dir::Vert }),
        "prism" => (TransKind::Cube, dir_of(dir)),
        "flip" => (TransKind::Flip, if dir == Some("r") { Dir::Right } else { Dir::Left }),
        "switch" => (TransKind::Switch, if dir == Some("l") { Dir::Left } else { Dir::Right }),
        "prstTrans" if k.attr("prst") == Some("fallOver") => (TransKind::FallOver, if k.attr("invX").is_some_and(|v| v == "1" || v == "true") { Dir::Right } else { Dir::Left }),
        "morph" => (TransKind::Morph, Dir::Left),
        _ => (TransKind::Other, Dir::Left),
    };
    t
}

/// Writes a transition (nothing when there is none and it advances on
/// clicks only).
pub(crate) fn write_transition(out: &mut String, t: &Transition) {
    if let Some(r) = t.raw.as_ref().filter(|r| r.src == Src::Pptx) {
        out.push_str(&r.xml);
        return;
    }
    if t.kind == TransKind::None && t.on_click && t.after.is_none() {
        return;
    }
    let spd = if t.dur >= 0.9 {
        "slow"
    } else if t.dur >= 0.6 {
        "med"
    } else {
        "fast"
    };
    let ms = (t.dur * 1000.0).round() as i64;
    let mut attrs = format!(r#" spd="{spd}""#);
    if !t.on_click {
        attrs.push_str(r#" advClick="0""#);
    }
    if let Some(a) = t.after {
        let _ = write!(attrs, r#" advTm="{}""#, (a * 1000.0).round() as i64);
    }
    let body = |kind: TransKind| -> String {
        match kind {
            TransKind::None => String::new(),
            TransKind::Push => format!(r#"<p:push dir="{}"/>"#, dir_name(t.dir)),
            TransKind::Wipe => format!(r#"<p:wipe dir="{}"/>"#, dir_name(t.dir)),
            TransKind::Cover => format!(r#"<p:cover dir="{}"/>"#, dir_name(t.dir)),
            TransKind::Reveal => format!(r#"<p:pull dir="{}"/>"#, dir_name(t.dir)),
            TransKind::Zoom => format!(r#"<p:zoom dir="{}"/>"#, if t.dir == Dir::In { "in" } else { "out" }),
            TransKind::Split => {
                let (o, d) = match t.dir {
                    Dir::HorzIn => ("horz", "in"),
                    Dir::VertIn => ("vert", "in"),
                    Dir::VertOut => ("vert", "out"),
                    _ => ("horz", "out"),
                };
                format!(r#"<p:split orient="{o}" dir="{d}"/>"#)
            }
            TransKind::Dissolve => "<p:dissolve/>".into(),
            TransKind::Blinds => format!(r#"<p:blinds dir="{}"/>"#, if t.dir == Dir::Vert { "vert" } else { "horz" }),
            TransKind::Checker => format!(r#"<p:checker dir="{}"/>"#, if t.dir == Dir::Vert { "vert" } else { "horz" }),
            _ => "<p:fade/>".into(),
        }
    };
    // PowerPoint 2010 and 2013 transitions: a fade for older readers.
    let newer = match t.kind {
        TransKind::Doors => Some(("p14", format!(r#"<p14:doors dir="{}"/>"#, if t.dir == Dir::Horz { "horz" } else { "vert" }))),
        TransKind::Cube => Some(("p14", format!(r#"<p14:prism dir="{}"/>"#, dir_name(t.dir)))),
        TransKind::Flip => Some(("p14", format!(r#"<p14:flip dir="{}"/>"#, if t.dir == Dir::Right { "r" } else { "l" }))),
        TransKind::Switch => Some(("p14", format!(r#"<p14:switch dir="{}"/>"#, if t.dir == Dir::Left { "l" } else { "r" }))),
        TransKind::FallOver => Some(("p15", format!(r#"<p15:prstTrans prst="fallOver"{}/>"#, if t.dir == Dir::Right { r#" invX="1""# } else { "" }))),
        _ => None,
    };
    if let Some((req, body)) = newer {
        let _ = write!(
            out,
            r#"<mc:AlternateContent xmlns:mc="{NS_MC}"><mc:Choice xmlns:p14="{NS_P14}" xmlns:p15="{NS_P15}" Requires="{req}"><p:transition{attrs} p14:dur="{ms}">{body}</p:transition></mc:Choice><mc:Fallback><p:transition{attrs}><p:fade/></p:transition></mc:Fallback></mc:AlternateContent>"#
        );
    } else if t.kind == TransKind::Morph {
        let _ = write!(
            out,
            r#"<mc:AlternateContent xmlns:mc="{NS_MC}"><mc:Choice xmlns:p159="{NS_P159}" xmlns:p14="{NS_P14}" Requires="p159"><p:transition{attrs} p14:dur="{ms}"><p159:morph option="byObject"/></p:transition></mc:Choice><mc:Fallback><p:transition{attrs}><p:fade/></p:transition></mc:Fallback></mc:AlternateContent>"#
        );
    } else {
        let _ = write!(
            out,
            r#"<mc:AlternateContent xmlns:mc="{NS_MC}"><mc:Choice xmlns:p14="{NS_P14}" Requires="p14"><p:transition{attrs} p14:dur="{ms}">{}</p:transition></mc:Choice><mc:Fallback><p:transition{attrs}>{}</p:transition></mc:Fallback></mc:AlternateContent>"#,
            body(t.kind),
            body(t.kind)
        );
    }
}

/// The animations of a `p:timing`.
pub(crate) fn read_timing(n: &Node, raw: &str) -> Timing {
    let mut t = Timing { raw: Some(Raw { src: Src::Pptx, xml: Arc::from(raw) }), ..Default::default() };
    let Some(main) = find_main_seq(n) else { return t };
    let mut effects = Vec::new();
    collect_effects(main, &mut effects);
    for (par, ctn) in effects {
        let class = Class::parse(ctn.attr("presetClass").unwrap_or("entr"));
        let preset = ctn.num("presetID").unwrap_or(0.0) as u32;
        let subtype = ctn.num("presetSubtype").unwrap_or(0.0) as u32;
        let mut tgts = Vec::new();
        ctn.find_all("spTgt", &mut tgts);
        let Some(tgt) = tgts.first() else { continue };
        let shape = tgt.num("spid").unwrap_or(0.0) as u32;
        let paras = tgt.find("pRg").map(|r| (r.num("st").unwrap_or(0.0) as u32, r.num("end").unwrap_or(0.0) as u32));
        let mut a = Anim::new(shape, class, Effect::from_preset(class, preset));
        a.preset = preset;
        a.subtype = subtype;
        a.paras = paras;
        a.trigger = Trigger::parse(ctn.attr("nodeType").unwrap_or("clickEffect"));
        a.dir = match subtype {
            1 => Dir::Up,
            2 => Dir::Right,
            4 => Dir::Down,
            16 => Dir::In,
            32 => Dir::Out,
            _ => Dir::Left,
        };
        a.delay = ctn.child("stCondLst").and_then(|s| s.child("cond")).and_then(|c| c.num("delay")).map_or(0.0, |d| (d / 1000.0) as f32);
        a.dur = longest_dur(ctn).map_or(if a.effect == Effect::Appear { 0.0 } else { 0.5 }, |ms| ms / 1000.0);
        let mut s = String::new();
        node_xml(&mut s, par);
        a.raw = Some(Raw { src: Src::Pptx, xml: Arc::from(s) });
        t.anims.push(a);
    }
    t
}

fn find_main_seq(n: &Node) -> Option<&Node> {
    let mut seqs = Vec::new();
    n.find_all("cTn", &mut seqs);
    for c in seqs {
        if c.attr("nodeType") == Some("mainSeq") {
            return Some(c);
        }
        if let Some(m) = find_main_seq(c) {
            return Some(m);
        }
    }
    None
}

/// Effect `p:par`s with their `p:cTn` (those with a preset class).
fn collect_effects<'a>(n: &'a Node, out: &mut Vec<(&'a Node, &'a Node)>) {
    for k in &n.kids {
        if k.local() == "par" {
            if let Some(c) = k.child("cTn") {
                if c.attr("presetClass").is_some() {
                    out.push((k, c));
                    continue;
                }
            }
        }
        collect_effects(k, out);
    }
}

/// The longest duration (ms) of the behaviors of an effect.
fn longest_dur(ctn: &Node) -> Option<f32> {
    let mut all = Vec::new();
    ctn.find_all("cTn", &mut all);
    let mut best: Option<f32> = None;
    let mut look = |c: &Node| {
        if let Some(d) = c.num("dur") {
            best = Some(best.map_or(d as f32, |b: f32| b.max(d as f32)));
        }
    };
    for c in all {
        look(c);
        let mut deeper = Vec::new();
        c.find_all("cTn", &mut deeper);
        for d in deeper {
            look(d);
        }
    }
    best
}

/// How a video or sound plays, from its `p:cMediaNode` in the timing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MediaNode {
    pub shape: u32,
    pub video: bool,
    pub volume: f32,
    pub mute: bool,
    pub looped: bool,
}

/// The media nodes of a `p:timing`.
pub(crate) fn read_media_nodes(n: &Node) -> Vec<MediaNode> {
    let mut out = Vec::new();
    let mut nodes = Vec::new();
    n.find_all("cMediaNode", &mut nodes);
    for m in nodes {
        let mut tgt = Vec::new();
        m.find_all("spTgt", &mut tgt);
        let Some(shape) = tgt.first().and_then(|t| t.num("spid")) else { continue };
        let looped = m.child("cTn").is_some_and(|c| c.attr("repeatCount") == Some("indefinite"));
        let volume = m.num("vol").map_or(1.0, |v| (v / 100_000.0).clamp(0.0, 1.0) as f32);
        let mute = m.attr("mute").is_some_and(|v| v == "1" || v == "true");
        out.push(MediaNode { shape: shape as u32, video: true, volume, mute, looped });
    }
    out
}

/// Writes the timing: as read while unchanged, else regenerated with the
/// slide's videos and sounds (`media`).
pub(crate) fn write_timing(out: &mut String, t: &Timing, media: &[MediaNode]) {
    if let Some(r) = t.raw.as_ref().filter(|r| r.src == Src::Pptx) {
        out.push_str(&r.xml);
        return;
    }
    if t.anims.is_empty() && media.is_empty() {
        return;
    }
    let mut id = 2u32;
    let mut next = || {
        id += 1;
        id
    };
    let mut s = String::from(r#"<p:timing><p:tnLst><p:par><p:cTn id="1" dur="indefinite" restart="never" nodeType="tmRoot"><p:childTnLst>"#);
    if t.anims.is_empty() {
        write_media_nodes(&mut s, media, &mut next);
        s.push_str("</p:childTnLst></p:cTn></p:par></p:tnLst></p:timing>");
        out.push_str(&s);
        return;
    }
    s.push_str(r#"<p:seq concurrent="1" nextAc="seek"><p:cTn id="2" dur="indefinite" nodeType="mainSeq"><p:childTnLst>"#);
    for (k, step) in t.steps().iter().enumerate() {
        let auto = k == 0 && t.auto_start();
        let _ = write!(s, r#"<p:par><p:cTn id="{}" fill="hold"><p:stCondLst><p:cond delay="indefinite"/>"#, next());
        if auto {
            s.push_str(r#"<p:cond evt="onBegin" delay="0"><p:tn val="2"/></p:cond>"#);
        }
        s.push_str("</p:stCondLst><p:childTnLst>");
        // Sub-groups: an after-previous animation starts a new one.
        let sched = t.schedule(step);
        let mut groups: Vec<Vec<(usize, f32)>> = Vec::new();
        for (n, &(i, start, _)) in sched.iter().enumerate() {
            if n == 0 || t.anims[i].trigger == Trigger::AfterPrev {
                groups.push(Vec::new());
            }
            groups.last_mut().unwrap().push((i, start));
        }
        for g in groups {
            let base = g.iter().map(|x| x.1 - t.anims[x.0].delay.max(0.0)).fold(f32::MAX, f32::min).max(0.0);
            let _ = write!(s, r#"<p:par><p:cTn id="{}" fill="hold"><p:stCondLst><p:cond delay="{}"/></p:stCondLst><p:childTnLst>"#, next(), (base * 1000.0).round() as i64);
            for (i, _) in g {
                write_effect(&mut s, &t.anims[i], &mut next);
            }
            s.push_str("</p:childTnLst></p:cTn></p:par>");
        }
        s.push_str("</p:childTnLst></p:cTn></p:par>");
    }
    s.push_str(r#"</p:childTnLst></p:cTn><p:prevCondLst><p:cond evt="onPrev" delay="0"><p:tgtEl><p:sldTgt/></p:tgtEl></p:cond></p:prevCondLst><p:nextCondLst><p:cond evt="onNext" delay="0"><p:tgtEl><p:sldTgt/></p:tgtEl></p:cond></p:nextCondLst></p:seq>"#);
    write_media_nodes(&mut s, media, &mut next);
    s.push_str("</p:childTnLst></p:cTn></p:par></p:tnLst>");
    let paras: Vec<u32> = {
        let mut v: Vec<u32> = t.anims.iter().filter(|a| a.paras.is_some()).map(|a| a.shape).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    if !paras.is_empty() {
        s.push_str("<p:bldLst>");
        for p in paras {
            let _ = write!(s, r#"<p:bldP spid="{p}" grpId="0" build="p"/>"#);
        }
        s.push_str("</p:bldLst>");
    }
    s.push_str("</p:timing>");
    out.push_str(&s);
}

/// The `p:video` and `p:audio` nodes: volume, muting and looping.
fn write_media_nodes(s: &mut String, media: &[MediaNode], next: &mut dyn FnMut() -> u32) {
    for m in media {
        let tag = if m.video { "video" } else { "audio" };
        let _ = write!(s, r#"<p:{tag}><p:cMediaNode vol="{}""#, (m.volume.clamp(0.0, 1.0) * 100_000.0).round() as i64);
        if m.mute {
            s.push_str(r#" mute="1""#);
        }
        let _ = write!(s, r#"><p:cTn id="{}""#, next());
        if m.looped {
            s.push_str(r#" repeatCount="indefinite""#);
        }
        let _ = write!(s, r#" fill="hold" display="0"><p:stCondLst><p:cond delay="indefinite"/></p:stCondLst></p:cTn><p:tgtEl><p:spTgt spid="{}"/></p:tgtEl></p:cMediaNode></p:{tag}>"#, m.shape);
    }
}

/// One effect's `p:par`.
fn write_effect(s: &mut String, a: &Anim, next: &mut dyn FnMut() -> u32) {
    let tgt = match a.paras {
        Some((st, end)) => format!(r#"<p:spTgt spid="{}"><p:txEl><p:pRg st="{st}" end="{end}"/></p:txEl></p:spTgt>"#, a.shape),
        None => format!(r#"<p:spTgt spid="{}"/>"#, a.shape),
    };
    let ms = (a.dur.max(0.0) * 1000.0).round().max(1.0) as i64;
    let subtype = match (a.effect, a.dir) {
        (Effect::Fly | Effect::Wipe, Dir::Up) => 1,
        (Effect::Fly | Effect::Wipe, Dir::Right) => 2,
        (Effect::Fly | Effect::Wipe, Dir::Down) => 4,
        (Effect::Fly | Effect::Wipe, _) => 8,
        (Effect::Zoom, _) => 16,
        _ => 0,
    };
    let (preset, subtype) = if a.effect == Effect::Other { (a.preset, a.subtype) } else { (a.effect.preset(a.class), subtype) };
    let _ = write!(
        s,
        r#"<p:par><p:cTn id="{}" presetID="{preset}" presetClass="{}" presetSubtype="{subtype}" fill="hold" grpId="0" nodeType="{}"><p:stCondLst><p:cond delay="{}"/></p:stCondLst><p:childTnLst>"#,
        next(),
        a.class.name(),
        a.trigger.name(),
        (a.delay.max(0.0) * 1000.0).round() as i64
    );
    let set = |s: &mut String, next: &mut dyn FnMut() -> u32, val: &str, delay: i64| {
        let _ = write!(
            s,
            r#"<p:set><p:cBhvr><p:cTn id="{}" dur="1" fill="hold"><p:stCondLst><p:cond delay="{delay}"/></p:stCondLst></p:cTn><p:tgtEl>{tgt}</p:tgtEl><p:attrNameLst><p:attrName>style.visibility</p:attrName></p:attrNameLst></p:cBhvr><p:to><p:strVal val="{val}"/></p:to></p:set>"#,
            next()
        );
    };
    let effect = |s: &mut String, next: &mut dyn FnMut() -> u32, filter: &str, dir: &str| {
        let _ = write!(s, r#"<p:animEffect transition="{dir}" filter="{filter}"><p:cBhvr><p:cTn id="{}" dur="{ms}"/><p:tgtEl>{tgt}</p:tgtEl></p:cBhvr></p:animEffect>"#, next());
    };
    let anim = |s: &mut String, next: &mut dyn FnMut() -> u32, attr: &str, from: &str, to: &str| {
        let _ = write!(
            s,
            r#"<p:anim calcmode="lin" valueType="num"><p:cBhvr additive="base"><p:cTn id="{}" dur="{ms}" fill="hold"/><p:tgtEl>{tgt}</p:tgtEl><p:attrNameLst><p:attrName>{attr}</p:attrName></p:attrNameLst></p:cBhvr><p:tavLst><p:tav tm="0"><p:val><p:strVal val="{from}"/></p:val></p:tav><p:tav tm="100000"><p:val><p:strVal val="{to}"/></p:val></p:tav></p:tavLst></p:anim>"#,
            next()
        );
    };
    // An attribute through times (`tm` in thousandths of a percent).
    let keys = |s: &mut String, next: &mut dyn FnMut() -> u32, attr: &str, vals: &[(u32, &str)]| {
        let tavs: String = vals.iter().map(|(tm, v)| format!(r#"<p:tav tm="{tm}"><p:val><p:strVal val="{v}"/></p:val></p:tav>"#)).collect();
        let _ = write!(
            s,
            r#"<p:anim calcmode="lin" valueType="num"><p:cBhvr additive="base"><p:cTn id="{}" dur="{ms}" fill="hold"/><p:tgtEl>{tgt}</p:tgtEl><p:attrNameLst><p:attrName>{attr}</p:attrName></p:attrNameLst></p:cBhvr><p:tavLst>{tavs}</p:tavLst></p:anim>"#,
            next()
        );
    };
    let wipe_dir = match a.dir {
        Dir::Up => "up",
        Dir::Right => "right",
        Dir::Down => "down",
        _ => "left",
    };
    match (a.class, a.effect) {
        (Class::Entrance, Effect::Appear) => set(s, next, "visible", 0),
        (Class::Entrance, Effect::Fade) => {
            set(s, next, "visible", 0);
            effect(s, next, "fade", "in");
        }
        (Class::Entrance, Effect::Wipe) => {
            set(s, next, "visible", 0);
            effect(s, next, &format!("wipe({wipe_dir})"), "in");
        }
        (Class::Entrance, Effect::Fly) => {
            set(s, next, "visible", 0);
            let (fx, fy) = match a.dir {
                Dir::Up => ("#ppt_x", "0-#ppt_h/2"),
                Dir::Right => ("1+#ppt_w/2", "#ppt_y"),
                Dir::Down => ("#ppt_x", "1+#ppt_h/2"),
                _ => ("0-#ppt_w/2", "#ppt_y"),
            };
            anim(s, next, "ppt_x", fx, "#ppt_x");
            anim(s, next, "ppt_y", fy, "#ppt_y");
        }
        (Class::Entrance, Effect::Zoom) => {
            set(s, next, "visible", 0);
            anim(s, next, "ppt_w", "0", "#ppt_w");
            anim(s, next, "ppt_h", "0", "#ppt_h");
            effect(s, next, "fade", "in");
        }
        (Class::Entrance, Effect::Float) => {
            set(s, next, "visible", 0);
            effect(s, next, "fade", "in");
            anim(s, next, "ppt_y", "#ppt_y+.1", "#ppt_y");
        }
        (Class::Entrance, Effect::Split) => {
            set(s, next, "visible", 0);
            effect(s, next, "barn(outVertical)", "in");
        }
        (Class::Entrance, Effect::Bounce) => {
            set(s, next, "visible", 0);
            effect(s, next, "fade", "in");
            keys(s, next, "ppt_y", &[(0, "#ppt_y-.3"), (40000, "#ppt_y"), (60000, "#ppt_y-.06"), (80000, "#ppt_y"), (90000, "#ppt_y-.015"), (100000, "#ppt_y")]);
        }
        (Class::Entrance, Effect::GrowTurn) => {
            set(s, next, "visible", 0);
            anim(s, next, "ppt_w", "0", "#ppt_w");
            anim(s, next, "ppt_h", "0", "#ppt_h");
            anim(s, next, "style.rotation", "-90", "0");
            effect(s, next, "fade", "in");
        }
        (Class::Entrance, Effect::Swivel) => {
            set(s, next, "visible", 0);
            effect(s, next, "fade", "in");
            keys(s, next, "ppt_w", &[(0, "0"), (33333, "#ppt_w"), (66666, "0"), (100000, "#ppt_w")]);
        }
        (Class::Exit, e) if e != Effect::Other => {
            match e {
                Effect::Fly => {
                    let (tx, ty) = match a.dir {
                        Dir::Up => ("#ppt_x", "0-#ppt_h/2"),
                        Dir::Right => ("1+#ppt_w/2", "#ppt_y"),
                        Dir::Down => ("#ppt_x", "1+#ppt_h/2"),
                        _ => ("0-#ppt_w/2", "#ppt_y"),
                    };
                    anim(s, next, "ppt_x", "#ppt_x", tx);
                    anim(s, next, "ppt_y", "#ppt_y", ty);
                }
                Effect::Wipe => effect(s, next, &format!("wipe({wipe_dir})"), "out"),
                Effect::Zoom => {
                    anim(s, next, "ppt_w", "#ppt_w", "0");
                    anim(s, next, "ppt_h", "#ppt_h", "0");
                    effect(s, next, "fade", "out");
                }
                Effect::Appear => {}
                Effect::Float => {
                    effect(s, next, "fade", "out");
                    anim(s, next, "ppt_y", "#ppt_y", "#ppt_y-.1");
                }
                Effect::Split => effect(s, next, "barn(inVertical)", "out"),
                Effect::Bounce => {
                    effect(s, next, "fade", "out");
                    keys(s, next, "ppt_y", &[(0, "#ppt_y"), (30000, "#ppt_y-.06"), (100000, "#ppt_y+.3")]);
                }
                Effect::GrowTurn => {
                    anim(s, next, "ppt_w", "#ppt_w", "0");
                    anim(s, next, "ppt_h", "#ppt_h", "0");
                    anim(s, next, "style.rotation", "0", "90");
                    effect(s, next, "fade", "out");
                }
                Effect::Swivel => {
                    effect(s, next, "fade", "out");
                    keys(s, next, "ppt_w", &[(0, "#ppt_w"), (33333, "0"), (66666, "#ppt_w"), (100000, "0")]);
                }
                _ => effect(s, next, "fade", "out"),
            }
            set(s, next, "hidden", (ms - 1).max(0));
        }
        (Class::Emphasis, Effect::Spin) => {
            let _ = write!(s, r#"<p:animRot by="21600000"><p:cBhvr><p:cTn id="{}" dur="{ms}" fill="hold"/><p:tgtEl>{tgt}</p:tgtEl><p:attrNameLst><p:attrName>r</p:attrName></p:attrNameLst></p:cBhvr></p:animRot>"#, next());
        }
        (Class::Emphasis, Effect::Pulse) => {
            let _ = write!(s, r#"<p:animScale><p:cBhvr><p:cTn id="{}" dur="{}" autoRev="1" fill="hold"/><p:tgtEl>{tgt}</p:tgtEl></p:cBhvr><p:by x="105000" y="105000"/></p:animScale>"#, next(), (ms / 2).max(1));
        }
        (Class::Emphasis, Effect::Teeter) => {
            // Four swings: right, left, right and back.
            for (k, by) in [240000, -480000, 480000, -240000].into_iter().enumerate() {
                let _ = write!(
                    s,
                    r#"<p:animRot by="{by}"><p:cBhvr><p:cTn id="{}" dur="{}" fill="hold"><p:stCondLst><p:cond delay="{}"/></p:stCondLst></p:cTn><p:tgtEl>{tgt}</p:tgtEl><p:attrNameLst><p:attrName>r</p:attrName></p:attrNameLst></p:cBhvr></p:animRot>"#,
                    next(),
                    (ms / 4).max(1),
                    ms * k as i64 / 4
                );
            }
        }
        (Class::Emphasis, Effect::Blink) => {
            for (k, v) in ["hidden", "visible", "hidden", "visible"].into_iter().enumerate() {
                set(s, next, v, ms * k as i64 / 4);
            }
        }
        (Class::Emphasis, Effect::Transparency) => {
            let _ = write!(s, r#"<p:animEffect transition="out" filter="image" prLst="opacity: 0.5"><p:cBhvr rctx="IE"><p:cTn id="{}" dur="{ms}" fill="hold"/><p:tgtEl>{tgt}</p:tgtEl></p:cBhvr></p:animEffect>"#, next());
        }
        (Class::Media, _) => {
            let _ = write!(s, r#"<p:cmd type="call" cmd="playFrom(0.0)"><p:cBhvr><p:cTn id="{}" dur="{ms}" fill="hold"/><p:tgtEl>{tgt}</p:tgtEl></p:cBhvr></p:cmd>"#, next());
        }
        (Class::Emphasis, Effect::Grow) => {
            let _ = write!(s, r#"<p:animScale><p:cBhvr><p:cTn id="{}" dur="{ms}" fill="hold"/><p:tgtEl>{tgt}</p:tgtEl></p:cBhvr><p:by x="150000" y="150000"/></p:animScale>"#, next());
        }
        _ => match &a.raw {
            // Its behaviors as read, with fresh ids.
            Some(r) => {
                let inner = r.xml.find("<p:childTnLst>").and_then(|i| r.xml.rfind("</p:childTnLst>").map(|j| &r.xml[i + 14..j])).unwrap_or("");
                s.push_str(&renumber(inner, next));
            }
            None => {
                set(s, next, "visible", 0);
                effect(s, next, "fade", "in");
            }
        },
    }
    s.push_str("</p:childTnLst></p:cTn></p:par>");
}

/// `p:cTn` ids replaced by fresh ones.
fn renumber(xml: &str, next: &mut dyn FnMut() -> u32) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(i) = rest.find("<p:cTn id=\"") {
        let at = i + 11;
        out.push_str(&rest[..at]);
        let end = rest[at..].find('"').map_or(at, |e| at + e);
        let _ = write!(out, "{}", next());
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawingml::read_fragment;

    #[test]
    fn timing_round_trip() {
        let mut t = Timing::default();
        let mut a = Anim::new(4, Class::Entrance, Effect::Fly);
        a.dir = Dir::Down;
        t.anims.push(a.clone());
        a.trigger = Trigger::AfterPrev;
        a.effect = Effect::Fade;
        a.delay = 0.5;
        t.anims.push(a.clone());
        a.class = Class::Exit;
        a.trigger = Trigger::OnClick;
        a.paras = Some((0, 0));
        t.anims.push(a);
        let mut s = String::new();
        write_timing(&mut s, &t, &[]);
        let n = read_fragment(&s);
        let back = read_timing(&n.kids[0], &s);
        assert_eq!(back.anims.len(), 3);
        assert_eq!(back.anims[0].effect, Effect::Fly);
        assert_eq!(back.anims[0].dir, Dir::Down);
        assert_eq!(back.anims[1].trigger, Trigger::AfterPrev);
        assert_eq!(back.anims[1].delay, 0.5);
        assert_eq!(back.anims[2].class, Class::Exit);
        assert_eq!(back.anims[2].paras, Some((0, 0)));
        assert_eq!(back.steps(), t.steps());
        assert!(s.contains("<p:bldP spid=\"4\""));
    }

    #[test]
    fn transitions_round_trip() {
        let t = Transition { kind: TransKind::Push, dir: Dir::Up, dur: 1.0, on_click: true, after: Some(3.0), raw: None };
        let mut s = String::new();
        write_transition(&mut s, &t);
        let n = read_fragment(&s);
        let back = read_transition(&n.kids[0], &s);
        assert_eq!((back.kind, back.dir, back.dur, back.after), (TransKind::Push, Dir::Up, 1.0, Some(3.0)));
    }

    #[test]
    fn every_transition_and_effect_comes_back() {
        for kind in TransKind::ALL.into_iter().filter(|k| !matches!(k, TransKind::Morph | TransKind::None)) {
            for &dir in kind.dirs().iter().chain(kind.dirs().is_empty().then_some(&Dir::Left)) {
                let t = Transition { kind, dir, dur: 1.5, on_click: true, after: None, raw: None };
                let mut s = String::new();
                write_transition(&mut s, &t);
                let back = read_transition(&read_fragment(&s).kids[0], &s);
                assert_eq!(back.kind, kind, "{s}");
                if !kind.dirs().is_empty() {
                    assert_eq!(back.dir, dir, "{s}");
                }
            }
        }
        for (class, list) in [(Class::Entrance, &Effect::ENTRANCE[..]), (Class::Exit, &Effect::ENTRANCE[..]), (Class::Emphasis, &Effect::EMPHASIS[..])] {
            for &e in list {
                let mut t = Timing::default();
                t.anims.push(Anim::new(4, class, e));
                let mut s = String::new();
                write_timing(&mut s, &t, &[]);
                let back = read_timing(&read_fragment(&s).kids[0], &s);
                assert_eq!((back.anims[0].class, back.anims[0].effect), (class, e), "{s}");
            }
        }
    }
}
