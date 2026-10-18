//! Slide transitions and object animations: the common ones are
//! understood (and played); the others are kept as the file wrote them
//! and play as a fade.

use std::sync::Arc;

use crate::object::Raw;

/// Transitions we play.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TransKind {
    None,
    Fade,
    Push,
    Wipe,
    Split,
    Reveal,
    Cover,
    Zoom,
    /// Small squares of the new slide appear at random.
    Dissolve,
    /// Stripes of the new slide widen.
    Blinds,
    Checker,
    /// The old slide opens as two doors.
    Doors,
    /// The slides are two faces of a turning cube.
    Cube,
    /// The slide turns over like a card.
    Flip,
    /// The old slide swings aside and the new one comes forward.
    Switch,
    /// The old slide falls back like a board.
    FallOver,
    /// Objects of the same name move from one slide to the next.
    Morph,
    /// Any other (played as a fade, kept as written).
    Other,
}

impl TransKind {
    pub const ALL: [TransKind; 17] = [
        TransKind::None,
        TransKind::Fade,
        TransKind::Push,
        TransKind::Wipe,
        TransKind::Split,
        TransKind::Reveal,
        TransKind::Cover,
        TransKind::Zoom,
        TransKind::Dissolve,
        TransKind::Blinds,
        TransKind::Checker,
        TransKind::Doors,
        TransKind::Cube,
        TransKind::Flip,
        TransKind::Switch,
        TransKind::FallOver,
        TransKind::Morph,
    ];

    pub fn key(self) -> &'static str {
        match self {
            TransKind::None => "slides.tr.none",
            TransKind::Fade => "slides.tr.fade",
            TransKind::Push => "slides.tr.push",
            TransKind::Wipe => "slides.tr.wipe",
            TransKind::Split => "slides.tr.split",
            TransKind::Reveal => "slides.tr.reveal",
            TransKind::Cover => "slides.tr.cover",
            TransKind::Zoom => "slides.tr.zoom",
            TransKind::Dissolve => "slides.tr.dissolve",
            TransKind::Blinds => "slides.tr.blinds",
            TransKind::Checker => "slides.tr.checker",
            TransKind::Doors => "slides.tr.doors",
            TransKind::Cube => "slides.tr.cube",
            TransKind::Flip => "slides.tr.flip",
            TransKind::Switch => "slides.tr.switch",
            TransKind::FallOver => "slides.tr.fall_over",
            TransKind::Morph => "slides.tr.morph",
            TransKind::Other => "slides.tr.other",
        }
    }

    /// Directions the transition takes.
    pub fn dirs(self) -> &'static [Dir] {
        match self {
            TransKind::Push | TransKind::Wipe | TransKind::Cover | TransKind::Reveal | TransKind::Cube => &[Dir::Left, Dir::Right, Dir::Up, Dir::Down],
            TransKind::Flip | TransKind::Switch | TransKind::FallOver => &[Dir::Left, Dir::Right],
            TransKind::Blinds | TransKind::Checker | TransKind::Doors => &[Dir::Vert, Dir::Horz],
            TransKind::Split => &[Dir::VertOut, Dir::VertIn, Dir::HorzOut, Dir::HorzIn],
            TransKind::Zoom => &[Dir::In, Dir::Out],
            _ => &[],
        }
    }
}

/// Where a transition or an animation comes from or goes to. `Left`:
/// from the left edge toward the right for animations; for transitions
/// the new slide comes in moving left (DrawingML "l").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Dir {
    #[default]
    Left,
    Right,
    Up,
    Down,
    HorzIn,
    HorzOut,
    VertIn,
    VertOut,
    In,
    Out,
    /// Across the width (blinds, doors).
    Horz,
    /// Down the height.
    Vert,
}

impl Dir {
    pub fn key(self) -> &'static str {
        match self {
            Dir::Left => "slides.dir.left",
            Dir::Right => "slides.dir.right",
            Dir::Up => "slides.dir.up",
            Dir::Down => "slides.dir.down",
            Dir::HorzIn => "slides.dir.horz_in",
            Dir::HorzOut => "slides.dir.horz_out",
            Dir::VertIn => "slides.dir.vert_in",
            Dir::VertOut => "slides.dir.vert_out",
            Dir::In => "slides.dir.in",
            Dir::Out => "slides.dir.out",
            Dir::Horz => "slides.dir.horz",
            Dir::Vert => "slides.dir.vert",
        }
    }
}

/// How a slide comes in.
#[derive(Clone, Debug, PartialEq)]
pub struct Transition {
    pub kind: TransKind,
    pub dir: Dir,
    /// Seconds.
    pub dur: f32,
    /// A click advances to the next slide.
    pub on_click: bool,
    /// Advances by itself after this many seconds.
    pub after: Option<f32>,
    /// As read (an unknown kind, sounds, extensions); dropped on change.
    pub raw: Option<Raw>,
}

impl Default for Transition {
    fn default() -> Self {
        Transition { kind: TransKind::None, dir: Dir::Left, dur: 0.7, on_click: true, after: None, raw: None }
    }
}

/// Animation classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Class {
    Entrance,
    Exit,
    Emphasis,
    /// Motion paths (kept, not played).
    Path,
    /// Playing a video or sound (PowerPoint's "mediacall").
    Media,
}

impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Class::Entrance => "entr",
            Class::Exit => "exit",
            Class::Emphasis => "emph",
            Class::Path => "path",
            Class::Media => "mediacall",
        }
    }

    pub fn parse(s: &str) -> Class {
        match s {
            "exit" => Class::Exit,
            "emph" => Class::Emphasis,
            "path" => Class::Path,
            "mediacall" => Class::Media,
            _ => Class::Entrance,
        }
    }
}

/// Effects we play.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Effect {
    Appear,
    Fade,
    Fly,
    Zoom,
    Wipe,
    Spin,
    Grow,
    /// Fades in rising (out: sinking).
    Float,
    /// Opens from the middle.
    Split,
    /// Drops and bounces.
    Bounce,
    /// Grows from nothing turning.
    GrowTurn,
    /// Turns about its upright axis.
    Swivel,
    /// Swells a little and back.
    Pulse,
    /// Rocks from side to side.
    Teeter,
    /// Goes off and on.
    Blink,
    /// Becomes half see-through.
    Transparency,
    /// Another (a fade when played; its XML is kept).
    Other,
}

impl Effect {
    pub const ENTRANCE: [Effect; 10] = [Effect::Appear, Effect::Fade, Effect::Fly, Effect::Float, Effect::Wipe, Effect::Split, Effect::Zoom, Effect::GrowTurn, Effect::Swivel, Effect::Bounce];
    pub const EMPHASIS: [Effect; 6] = [Effect::Pulse, Effect::Spin, Effect::Grow, Effect::Teeter, Effect::Blink, Effect::Transparency];

    /// PowerPoint's preset id of the effect in a class.
    pub fn preset(self, class: Class) -> u32 {
        match (self, class) {
            (Effect::Appear, _) => 1,
            (Effect::Fly, _) => 2,
            (Effect::Wipe, _) => 22,
            (Effect::Zoom, _) => 53,
            (Effect::Fade, _) => 10,
            (Effect::Spin, _) => 8,
            (Effect::Grow, _) => 6,
            (Effect::Float, _) => 42,
            (Effect::Split, _) => 16,
            (Effect::Bounce, _) => 26,
            (Effect::GrowTurn, _) => 31,
            (Effect::Swivel, _) => 19,
            (Effect::Pulse, _) => 26,
            (Effect::Teeter, _) => 32,
            (Effect::Blink, _) => 35,
            (Effect::Transparency, _) => 9,
            (Effect::Other, _) => 0,
        }
    }

    pub fn from_preset(class: Class, id: u32) -> Effect {
        match (class, id) {
            (Class::Entrance | Class::Exit, 1) => Effect::Appear,
            (Class::Entrance | Class::Exit, 2) => Effect::Fly,
            (Class::Entrance | Class::Exit, 10) => Effect::Fade,
            (Class::Entrance | Class::Exit, 22) => Effect::Wipe,
            (Class::Entrance | Class::Exit, 53 | 23) => Effect::Zoom,
            (Class::Emphasis, 8) => Effect::Spin,
            (Class::Emphasis, 6) => Effect::Grow,
            (Class::Entrance | Class::Exit, 42) => Effect::Float,
            (Class::Entrance | Class::Exit, 16) => Effect::Split,
            (Class::Entrance | Class::Exit, 26) => Effect::Bounce,
            (Class::Entrance | Class::Exit, 31) => Effect::GrowTurn,
            (Class::Entrance | Class::Exit, 19 | 45) => Effect::Swivel,
            (Class::Emphasis, 26) => Effect::Pulse,
            (Class::Emphasis, 32) => Effect::Teeter,
            (Class::Emphasis, 35) => Effect::Blink,
            (Class::Emphasis, 9) => Effect::Transparency,
            _ => Effect::Other,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Effect::Appear => "slides.fx.appear",
            Effect::Fade => "slides.fx.fade",
            Effect::Fly => "slides.fx.fly",
            Effect::Zoom => "slides.fx.zoom",
            Effect::Wipe => "slides.fx.wipe",
            Effect::Spin => "slides.fx.spin",
            Effect::Grow => "slides.fx.grow",
            Effect::Float => "slides.fx.float",
            Effect::Split => "slides.fx.split",
            Effect::Bounce => "slides.fx.bounce",
            Effect::GrowTurn => "slides.fx.grow_turn",
            Effect::Swivel => "slides.fx.swivel",
            Effect::Pulse => "slides.fx.pulse",
            Effect::Teeter => "slides.fx.teeter",
            Effect::Blink => "slides.fx.blink",
            Effect::Transparency => "slides.fx.transparency",
            Effect::Other => "slides.fx.other",
        }
    }
}

/// When an animation starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Trigger {
    #[default]
    OnClick,
    WithPrev,
    AfterPrev,
}

impl Trigger {
    pub fn name(self) -> &'static str {
        match self {
            Trigger::OnClick => "clickEffect",
            Trigger::WithPrev => "withEffect",
            Trigger::AfterPrev => "afterEffect",
        }
    }

    pub fn parse(s: &str) -> Trigger {
        match s {
            "withEffect" => Trigger::WithPrev,
            "afterEffect" => Trigger::AfterPrev,
            _ => Trigger::OnClick,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Trigger::OnClick => "slides.anim.on_click",
            Trigger::WithPrev => "slides.anim.with_prev",
            Trigger::AfterPrev => "slides.anim.after_prev",
        }
    }
}

/// One animation of an object.
#[derive(Clone, Debug, PartialEq)]
pub struct Anim {
    /// The object's id.
    pub shape: u32,
    /// Only these paragraphs (a list animated by paragraph).
    pub paras: Option<(u32, u32)>,
    pub class: Class,
    pub effect: Effect,
    /// PowerPoint's preset id and subtype (direction), kept.
    pub preset: u32,
    pub subtype: u32,
    pub dir: Dir,
    pub trigger: Trigger,
    /// Seconds.
    pub delay: f32,
    pub dur: f32,
    /// The effect's XML as read (its `p:par`), while unchanged.
    pub raw: Option<Raw>,
}

impl Anim {
    pub fn new(shape: u32, class: Class, effect: Effect) -> Anim {
        Anim { shape, paras: None, class, effect, preset: effect.preset(class), subtype: 0, dir: Dir::Left, trigger: Trigger::OnClick, delay: 0.0, dur: if effect == Effect::Appear { 0.0 } else { 0.5 }, raw: None }
    }
}

/// The animations of a slide, in order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Timing {
    pub anims: Vec<Anim>,
    /// The slide's whole `p:timing` as read, while no animation changed.
    pub raw: Option<Raw>,
    /// Other timing XML we don't model (triggers on shapes, media
    /// sequences) kept to write back with regenerated animations.
    pub extra: Option<Arc<str>>,
}

impl Timing {
    /// Steps of the show: the animations grouped by click (each group
    /// starts with an on-click animation, except a first group that
    /// starts by itself).
    pub fn steps(&self) -> Vec<Vec<usize>> {
        let mut out: Vec<Vec<usize>> = Vec::new();
        for (i, a) in self.anims.iter().enumerate() {
            if a.trigger == Trigger::OnClick || out.is_empty() {
                out.push(Vec::new());
            }
            out.last_mut().unwrap().push(i);
        }
        out
    }

    /// Whether the first group starts without a click.
    pub fn auto_start(&self) -> bool {
        self.anims.first().is_some_and(|a| a.trigger != Trigger::OnClick)
    }

    /// Start and end times (seconds from the step start) of each
    /// animation in a step.
    pub fn schedule(&self, step: &[usize]) -> Vec<(usize, f32, f32)> {
        let mut out = Vec::new();
        let (mut prev_start, mut prev_end) = (0.0f32, 0.0f32);
        for (k, &i) in step.iter().enumerate() {
            let a = &self.anims[i];
            let base = match a.trigger {
                Trigger::AfterPrev if k > 0 => prev_end,
                Trigger::WithPrev if k > 0 => prev_start,
                _ => 0.0,
            };
            let start = base + a.delay.max(0.0);
            let end = start + a.dur.max(0.0);
            out.push((i, start, end));
            prev_start = start;
            prev_end = prev_end.max(end);
        }
        out
    }

    /// How video or sound `shape` starts in the show: by itself (with or
    /// after the previous step) or with a click of the sequence; `None`
    /// when only a click on it plays it.
    pub fn media_start(&self, shape: u32) -> Option<Trigger> {
        self.anims.iter().find(|a| a.class == Class::Media && a.shape == shape).map(|a| a.trigger)
    }

    /// Sets how `shape` starts (see [`Timing::media_start`]): its play
    /// animation goes first, so that it starts with the slide or with
    /// the first click.
    pub fn set_media_start(&mut self, shape: u32, start: Option<Trigger>, dur: f32) {
        if self.media_start(shape) == start {
            return;
        }
        self.anims.retain(|a| !(a.class == Class::Media && a.shape == shape));
        if let Some(t) = start {
            let mut a = Anim::new(shape, Class::Media, Effect::Other);
            a.preset = 1;
            a.trigger = t;
            a.dur = dur.max(0.001);
            a.raw = None;
            self.anims.insert(0, a);
        }
        self.raw = None;
    }

    /// Drops the animations of objects that are gone.
    pub fn retain_shapes(&mut self, alive: &dyn Fn(u32) -> bool) {
        let n = self.anims.len();
        self.anims.retain(|a| alive(a.shape));
        if self.anims.len() != n {
            self.raw = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_and_schedule() {
        let mut t = Timing::default();
        let mut a = Anim::new(2, Class::Entrance, Effect::Fade);
        t.anims.push(a.clone());
        a.trigger = Trigger::WithPrev;
        t.anims.push(a.clone());
        a.trigger = Trigger::AfterPrev;
        a.delay = 0.25;
        t.anims.push(a.clone());
        a.trigger = Trigger::OnClick;
        t.anims.push(a);
        let steps = t.steps();
        assert_eq!(steps, vec![vec![0, 1, 2], vec![3]]);
        let s = t.schedule(&steps[0]);
        assert_eq!(s[1].1, 0.0);
        assert!((s[2].1 - 0.75).abs() < 1e-6);
        assert!(!t.auto_start());
    }
}
