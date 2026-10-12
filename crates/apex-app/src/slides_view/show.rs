//! The slide show: slides fill the screen, advanced by clicks, keys and
//! the wheel, with their transitions and animations played as they come.
//! Where each object is at a moment (its [`Look`]) and how a transition
//! paints two slides are pure functions of the time, so they are tested
//! without a screen. The presenter view is in [`super::presenter`].

use apex_slides::anim::{Class, Dir, Effect, TransKind};
use apex_slides::{Chain, Deck, ObjKind, Object, Slide};
use eframe::egui::{self, Color32, Key, Modifiers, Painter, Rect, pos2, vec2};

use super::render::{self, Opts, Xf};
use super::show_ink::Ink;
use super::warp;

/// Seconds of the end screen before a looped show starts again.
const LOOP_PAUSE: f64 = 0.0;

/// A running show.
#[derive(Clone, Debug)]
pub struct Show {
    /// The slides shown (deck indices; hidden ones left out).
    pub order: Vec<usize>,
    /// Place in `order`; `order.len()` is the end screen.
    pub at: usize,
    /// Animation steps started on this slide.
    pub step: usize,
    /// When the last step started (seconds, egui time).
    pub step_t: f64,
    /// The slide shown before (deck index) while the transition runs, and
    /// when the current slide came.
    pub from: Option<usize>,
    pub came: f64,
    /// A black or white screen instead of the slide.
    pub blank: Option<Color32>,
    /// When the show started.
    pub start: f64,
    /// Digits typed to go to a slide by number.
    pub typed: String,
    /// The presenter view is in the main window, the slides in another.
    pub presenter: bool,
    /// The window was fullscreen before the show.
    pub was_full: bool,
    /// Starts over after the last slide.
    pub looped: bool,
    /// Advances by the slides' timings.
    pub timings: bool,
    /// Only timings advance; clicks and keys other than Esc do nothing.
    pub kiosk: bool,
    /// Asked to end.
    pub done: bool,
    /// The pen and laser pointer, and the marks made.
    pub ink: Ink,
    /// This frame's click went to a video or sound (see `super::media`).
    pub media_click: bool,
    /// When the mouse last moved (its arrow hides a while after).
    pub moved_t: f64,
}

/// How an object looks at a moment of the show.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub visible: bool,
    pub alpha: f32,
    /// Offset, points.
    pub dx: f32,
    pub dy: f32,
    /// Scale around the center.
    pub scale: f32,
    /// Extra turn, degrees.
    pub rot: f32,
    /// Width kept (turning about its upright axis), 0..1.
    pub sx: f32,
    /// Part shown from an edge: the edge it grows from and the fraction.
    pub clip: Option<(Dir, f32)>,
    /// Paragraphs not shown yet (or gone).
    pub hidden_paras: Vec<(u32, u32)>,
}

impl Default for Look {
    fn default() -> Self {
        Look { visible: true, alpha: 1.0, dx: 0.0, dy: 0.0, scale: 1.0, rot: 0.0, sx: 1.0, clip: None, hidden_paras: Vec::new() }
    }
}

/// Eased progress: slow at both ends.
fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Show {
    /// A show of `deck` from slide `from` (a deck index).
    pub fn new(deck: &Deck, from: usize, now: f64) -> Show {
        let order: Vec<usize> = (0..deck.slides.len()).filter(|&i| !deck.slides[i].hidden || i == from).collect();
        let at = order.iter().position(|&i| i >= from).unwrap_or(0);
        let mut s = Show {
            order,
            at,
            step: 0,
            step_t: now,
            from: None,
            came: now,
            blank: None,
            start: now,
            typed: String::new(),
            presenter: false,
            was_full: false,
            looped: deck.show.looped || deck.show.kiosk,
            timings: deck.show.use_timings || deck.show.kiosk,
            kiosk: deck.show.kiosk,
            done: false,
            ink: Ink::new(),
            media_click: false,
            moved_t: now,
        };
        s.arrive(deck, None, now);
        s
    }

    /// The deck index of the slide shown (none: the end screen).
    pub fn slide(&self) -> Option<usize> {
        self.order.get(self.at).copied()
    }

    /// The slide after this one, for the presenter.
    pub fn next_slide(&self) -> Option<usize> {
        self.order.get(self.at + 1).copied().or(if self.looped { self.order.first().copied() } else { None })
    }

    fn steps(deck: &Deck, i: usize) -> Vec<Vec<usize>> {
        deck.slides[i].timing.steps()
    }

    /// Transition seconds into the current slide (none when it has none).
    fn trans_dur(deck: &Deck, i: usize) -> f64 {
        let t = &deck.slides[i].transition;
        if t.kind == TransKind::None { 0.0 } else { t.dur.max(0.0) as f64 }
    }

    /// Lands on the current slide, coming from slide `from`.
    fn arrive(&mut self, deck: &Deck, from: Option<usize>, now: f64) {
        self.from = from;
        self.came = now;
        self.step = 0;
        self.step_t = now;
        if let Some(i) = self.slide() {
            if deck.slides[i].timing.auto_start() {
                self.step = 1;
                self.step_t = now + if from.is_some() { Self::trans_dur(deck, i) } else { 0.0 };
            }
            if from.is_none() || Self::trans_dur(deck, i) == 0.0 {
                self.from = None;
            }
        }
    }

    /// Whether the transition into the current slide still runs.
    pub fn in_transition(&self, deck: &Deck, now: f64) -> bool {
        match (self.from, self.slide()) {
            (Some(_), Some(i)) => now - self.came < Self::trans_dur(deck, i),
            _ => false,
        }
    }

    /// When the running step ends (seconds, egui time).
    fn step_end(&self, deck: &Deck) -> f64 {
        let Some(i) = self.slide() else { return self.step_t };
        let t = &deck.slides[i].timing;
        let steps = t.steps();
        match self.step.checked_sub(1).and_then(|k| steps.get(k)) {
            Some(st) => self.step_t + t.schedule(st).iter().map(|x| x.2).fold(0.0f32, f32::max) as f64,
            None => self.came + Self::trans_dur(deck, i),
        }
    }

    /// A click: finishes what moves, plays the next animation, or goes on.
    pub fn next(&mut self, deck: &Deck, now: f64) {
        self.blank = None;
        if self.in_transition(deck, now) {
            self.from = None;
            self.came = now - 1000.0;
            return;
        }
        let Some(i) = self.slide() else {
            if self.looped && !self.order.is_empty() {
                self.at = 0;
                self.arrive(deck, None, now);
            } else {
                self.done = true;
            }
            return;
        };
        let steps = Self::steps(deck, i);
        if now < self.step_end(deck) {
            // The running step jumps to its end.
            self.step_t = now - 1000.0;
            return;
        }
        if self.step < steps.len() {
            self.step += 1;
            self.step_t = now;
            return;
        }
        self.at += 1;
        if self.at >= self.order.len() && self.looped {
            self.at = 0;
        }
        if self.slide().is_some() {
            self.arrive(deck, Some(i), now);
        }
    }

    /// Back one animation step, or to the previous slide with all shown.
    /// Whether [`Self::prev`] has anything to go back to (not at the
    /// first step of the first slide).
    pub fn can_prev(&self, deck: &Deck) -> bool {
        let auto = self.slide().is_some_and(|i| deck.slides[i].timing.auto_start()) as usize;
        self.blank.is_some() || self.slide().is_some() && self.step > auto || self.at > 0 || self.looped
    }

    pub fn prev(&mut self, deck: &Deck, now: f64) {
        self.blank = None;
        let auto = self.slide().is_some_and(|i| deck.slides[i].timing.auto_start()) as usize;
        if self.slide().is_some() && self.step > auto {
            self.step -= 1;
            self.step_t = now - 1000.0;
            return;
        }
        if self.at == 0 {
            if !self.looped {
                return;
            }
            self.at = self.order.len();
        }
        self.at -= 1;
        self.from = None;
        self.came = now - 1000.0;
        self.step = self.slide().map_or(0, |i| Self::steps(deck, i).len());
        self.step_t = now - 1000.0;
    }

    /// Goes to the `n`th shown slide (from 0), its animations to play.
    pub fn go(&mut self, deck: &Deck, n: usize, now: f64) {
        if n < self.order.len() {
            let from = self.slide();
            self.at = n;
            self.arrive(deck, from.filter(|&f| Some(f) != self.slide()), now);
        }
    }

    /// Advances by itself: steps "after previous" and the slide's timing.
    pub fn tick(&mut self, deck: &Deck, now: f64) {
        if !self.timings || self.blank.is_some() || self.done {
            return;
        }
        let Some(i) = self.slide() else {
            if self.looped && now - self.came > LOOP_PAUSE {
                self.next(deck, now);
            }
            return;
        };
        let t = &deck.slides[i].transition;
        let Some(after) = t.after else { return };
        let steps = Self::steps(deck, i).len();
        let end = self.step_end(deck);
        if now >= end + after as f64 && now >= self.came + after as f64 {
            if self.step < steps {
                self.step += 1;
                self.step_t = now;
            } else {
                self.next(deck, now);
            }
        }
    }

    /// Seconds until something moves by itself (for repainting).
    pub fn busy(&self, deck: &Deck, now: f64) -> bool {
        self.in_transition(deck, now) || now < self.step_end(deck) || (self.timings && self.slide().is_some_and(|i| deck.slides[i].transition.after.is_some()))
    }

    /// A key during the show; true if it was used.
    pub fn key(&mut self, deck: &Deck, key: Key, m: Modifiers, now: f64) -> bool {
        if !self.kiosk && self.blank.is_none() && self.ink.key(key, m, self.slide()) {
            return true;
        }
        if m.ctrl || m.command || m.alt {
            return false;
        }
        match key {
            Key::Escape | Key::Minus => self.done = true,
            _ if self.kiosk => return false,
            Key::Num0 | Key::Num1 | Key::Num2 | Key::Num3 | Key::Num4 | Key::Num5 | Key::Num6 | Key::Num7 | Key::Num8 | Key::Num9 => {
                self.typed.push_str(key.name());
            }
            Key::Enter if !self.typed.is_empty() => {
                let n: usize = self.typed.parse().unwrap_or(0);
                self.typed.clear();
                // The number is the slide's in the deck.
                if let Some(k) = self.order.iter().position(|&i| i + 1 == n) {
                    self.go(deck, k, now);
                }
            }
            Key::B | Key::Period => self.blank = if self.blank == Some(Color32::BLACK) { None } else { Some(Color32::BLACK) },
            Key::W | Key::Comma => self.blank = if self.blank == Some(Color32::WHITE) { None } else { Some(Color32::WHITE) },
            Key::ArrowRight | Key::ArrowDown | Key::PageDown | Key::Space | Key::Enter | Key::N => self.next(deck, now),
            Key::ArrowLeft | Key::ArrowUp | Key::PageUp | Key::Backspace | Key::P => self.prev(deck, now),
            Key::Home => self.go(deck, 0, now),
            Key::End => self.go(deck, self.order.len().saturating_sub(1), now),
            _ => return false,
        }
        if !matches!(key, Key::Num0 | Key::Num1 | Key::Num2 | Key::Num3 | Key::Num4 | Key::Num5 | Key::Num6 | Key::Num7 | Key::Num8 | Key::Num9 | Key::Enter) {
            self.typed.clear();
        }
        true
    }

    /// Clicks, the wheel and keys on the show's screen.
    pub fn input(&mut self, ctx: &egui::Context, resp: &egui::Response, deck: &Deck) {
        let now = ctx.input(|i| i.time);
        // The pen and the eraser take the presses.
        let inked = match self.slide().filter(|_| !self.kiosk && self.blank.is_none()) {
            Some(i) => self.ink.input(resp, Xf::for_slide(deck, slide_rect(deck, resp.rect)), i),
            None => false,
        };
        if !self.kiosk && !inked {
            if resp.clicked() && !std::mem::take(&mut self.media_click) {
                self.next(deck, now);
            } else if resp.secondary_clicked() {
                self.prev(deck, now);
            }
            let wheel: f32 = ctx.input(|i| i.events.iter().map(|e| if let egui::Event::MouseWheel { delta, .. } = e { delta.y } else { 0.0 }).sum());
            if resp.hovered() && wheel.abs() > 0.5 {
                if wheel < 0.0 { self.next(deck, now) } else { self.prev(deck, now) }
            }
        }
        let keys: Vec<(Key, Modifiers)> = ctx.input(|i| i.events.iter().filter_map(|e| match e {
            egui::Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
            _ => None,
        }).collect());
        for (k, m) in keys {
            if self.key(deck, k, m, now) {
                ctx.input_mut(|i| i.consume_key(m, k));
            }
        }
        self.tick(deck, now);
        if self.busy(deck, now) {
            ctx.request_repaint();
        }
    }

    /// Videos and sounds of the current slide whose play animation has
    /// started by `now`.
    pub fn media_due(&self, deck: &Deck, now: f64) -> Vec<u32> {
        let Some(i) = self.slide() else { return Vec::new() };
        let t = &deck.slides[i].timing;
        let mut out = Vec::new();
        for (k, st) in t.steps().iter().enumerate().take(self.step) {
            for (a, start, _) in t.schedule(st) {
                if t.anims[a].class == Class::Media && (k + 1 < self.step || now - self.step_t >= start as f64) {
                    out.push(t.anims[a].shape);
                }
            }
        }
        out
    }

    /// How each object of slide `i` looks at `now`: by its animations
    /// played (or not yet) on this slide.
    pub fn looks(&self, deck: &Deck, i: usize, now: f64) -> Vec<(u32, Look)> {
        let slide = &deck.slides[i];
        let t = &slide.timing;
        let current = self.slide() == Some(i);
        // Progress of each animation: none (not reached), or 0..1.
        let mut prog: Vec<Option<f32>> = vec![None; t.anims.len()];
        for (k, st) in t.steps().iter().enumerate() {
            let done = if current { k + 1 < self.step || (k + 1 == self.step && now >= self.step_end(deck)) } else { self.slide().is_none_or(|c| c > i) };
            let running = current && k + 1 == self.step && !done;
            for (a, start, end) in t.schedule(st) {
                prog[a] = if done {
                    Some(1.0)
                } else if running {
                    let e = now - self.step_t;
                    if e < start as f64 { None } else if end <= start { Some(1.0) } else { Some(((e - start as f64) / (end - start) as f64).clamp(0.0, 1.0) as f32) }
                } else {
                    None
                };
            }
        }
        let mut out: Vec<(u32, Look)> = Vec::new();
        for (a, anim) in t.anims.iter().enumerate() {
            let at = match out.iter().position(|(id, _)| *id == anim.shape) {
                Some(k) => k,
                None => {
                    let mut l = Look::default();
                    // What comes in isn't there before.
                    if anim.class == Class::Entrance {
                        match anim.paras {
                            Some(r) => l.hidden_paras.push(r),
                            None => l.visible = false,
                        }
                    }
                    out.push((anim.shape, l));
                    out.len() - 1
                }
            };
            let l = &mut out[at].1;
            if anim.class == Class::Entrance && anim.paras.is_some() && !l.hidden_paras.contains(&anim.paras.unwrap()) && prog[a].is_none() {
                l.hidden_paras.push(anim.paras.unwrap());
            }
            let Some(p) = prog[a] else { continue };
            let f = frame_of(deck, slide, anim.shape);
            apply(l, anim.class, anim.effect, anim.dir, anim.paras, p, f, (deck.width, deck.height));
        }
        out
    }
}

/// The frame of object `id` of a slide (points).
fn frame_of(deck: &Deck, s: &Slide, id: u32) -> apex_slides::Frame {
    let ch = Chain::for_slide(deck, s);
    s.objects.iter().find(|o| o.id == id).map(|o| ch.frame(o)).unwrap_or_default()
}

/// Plays one animation at progress `p` on a look.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply(l: &mut Look, class: Class, effect: Effect, dir: Dir, paras: Option<(u32, u32)>, p: f32, f: apex_slides::Frame, size: (f32, f32)) {
    let e = ease(p);
    match class {
        // Videos and sounds play (see `super::media`); they don't move.
        Class::Media => {}
        Class::Emphasis => match effect {
            Effect::Spin => l.rot += 360.0 * e,
            Effect::Grow => l.scale *= 1.0 + 0.5 * e,
            Effect::Pulse => l.scale *= 1.0 + 0.08 * (std::f32::consts::PI * p).sin(),
            Effect::Teeter => l.rot += 8.0 * (4.0 * std::f32::consts::PI * p).sin() * (1.0 - p),
            Effect::Blink => {
                if p < 1.0 && (p * 4.0) as u32 % 2 == 0 {
                    l.alpha = 0.0;
                }
            }
            Effect::Transparency => l.alpha *= 1.0 - 0.5 * e,
            _ => l.alpha *= 1.0 - 0.5 * (std::f32::consts::PI * p).sin(),
        },
        Class::Entrance | Class::Exit | Class::Path => {
            let entering = class != Class::Exit;
            // How much is there: 0 gone, 1 fully in.
            let k = if entering { e } else { 1.0 - e };
            if let Some(r) = paras {
                // A list shown by paragraph: the rest of the object stays.
                l.hidden_paras.retain(|x| *x != r);
                if (entering && p <= 0.0) || (!entering && p >= 1.0) {
                    l.hidden_paras.push(r);
                }
                return;
            }
            if class == Class::Path {
                return;
            }
            if entering {
                l.visible = true;
            } else if p >= 1.0 {
                l.visible = false;
                return;
            }
            match effect {
                Effect::Appear => {}
                Effect::Fly => {
                    let (w, h) = size;
                    let away = 1.0 - k;
                    match dir {
                        Dir::Right => l.dx += (w - f.x) * away,
                        Dir::Up => l.dy -= (f.y + f.h) * away,
                        Dir::Down => l.dy += (h - f.y) * away,
                        _ => l.dx -= (f.x + f.w) * away,
                    }
                }
                Effect::Zoom => {
                    l.scale *= k.max(0.01);
                    l.alpha *= k;
                }
                Effect::Wipe => l.clip = Some((dir, k)),
                Effect::Split => l.clip = Some((Dir::VertOut, k)),
                Effect::Float => {
                    l.alpha *= k;
                    let way = if entering { 1.0 } else { -1.0 };
                    l.dy += way * 0.1 * size.1 * (1.0 - k);
                }
                Effect::Bounce => {
                    // Drops from above and bounces twice (out: hops and falls).
                    let t = p.clamp(0.0, 1.0);
                    // Below its place (negative: above), in drops.
                    let down = if entering { bounce(t) - 1.0 } else { hop(t) };
                    l.dy += down * 0.3 * size.1;
                    l.alpha *= if entering { (t * 4.0).min(1.0) } else { ((1.0 - t) * 4.0).min(1.0) };
                }
                Effect::GrowTurn => {
                    l.scale *= k.max(0.01);
                    l.rot += if entering { -90.0 } else { 90.0 } * (1.0 - k);
                    l.alpha *= k;
                }
                Effect::Swivel => {
                    l.sx *= (2.5 * std::f32::consts::PI * k).sin().abs();
                    l.alpha *= (k * 2.0).min(1.0);
                }
                _ => l.alpha *= k,
            }
        }
    }
}

/// How far down a bouncing drop is at `t`: 0 above, 1 landed.
fn bounce(t: f32) -> f32 {
    let (n, d) = (7.5625, 2.75);
    if t < 1.0 / d {
        n * t * t
    } else if t < 2.0 / d {
        let t = t - 1.5 / d;
        n * t * t + 0.75
    } else if t < 2.5 / d {
        let t = t - 2.25 / d;
        n * t * t + 0.9375
    } else {
        let t = t - 2.625 / d;
        n * t * t + 0.984375
    }
}

/// Leaving with a bounce: a hop up, then a fall below (signed, in drops).
fn hop(t: f32) -> f32 {
    if t < 0.3 { -0.2 * (t / 0.3 * std::f32::consts::FRAC_PI_2).sin() } else { -0.2 + 1.2 * ((t - 0.3) / 0.7).powi(2) }
}

/// An object as it looks: moved, scaled, turned and its hidden
/// paragraphs emptied.
fn looked(ch: &Chain, o: &Object, l: &Look) -> Object {
    let mut o = o.clone();
    let mut f = ch.frame(&o);
    let (cx, cy) = (f.x + f.w / 2.0 + l.dx, f.y + f.h / 2.0 + l.dy);
    f.w *= l.scale;
    f.h *= l.scale;
    f.x = cx - f.w / 2.0;
    f.y = cy - f.h / 2.0;
    f.rot = (f.rot + l.rot) % 360.0;
    if f != ch.frame(&o) {
        o.sp.frame = Some(f);
    }
    if !l.hidden_paras.is_empty()
        && let ObjKind::Shape { .. } = o.kind
        && let Some(t) = o.text_or_new()
    {
        for (k, p) in t.paras.iter_mut().enumerate() {
            if l.hidden_paras.iter().any(|&(a, b)| (a..=b).contains(&(k as u32))) {
                let attrs = p.runs.first().map(|r| r.attrs.clone()).unwrap_or_default();
                p.text.clear();
                p.runs = vec![apex_doc::Run { len: 0, attrs }];
            }
        }
    }
    o
}

/// Paints slide `s` with its objects as `looks` say.
pub fn paint_looks(p: &Painter, deck: &Deck, s: &Slide, rect: Rect, looks: &[(u32, Look)]) {
    let p = p.with_clip_rect(rect.intersect(p.clip_rect()));
    let ch = Chain::for_slide(deck, s);
    let xf = Xf::for_slide(deck, rect);
    let o = Opts::default();
    render::paint_fill_rect(&p, &ch, &ch.background(), rect, xf.sx, false);
    for (obj, c) in ch.decoration() {
        render::paint_object(&p, &c, obj, xf, xf.sx, o);
    }
    for obj in &s.objects {
        let Some((_, l)) = looks.iter().find(|(id, _)| *id == obj.id) else {
            render::paint_object(&p, &ch, obj, xf, xf.sx, o);
            continue;
        };
        if !l.visible || l.alpha <= 0.0 {
            continue;
        }
        let shown = looked(&ch, obj, l);
        let mut q = p.clone();
        q.multiply_opacity(l.alpha.clamp(0.0, 1.0));
        if let Some((dir, k)) = l.clip {
            let r = xf.rect(&ch.frame(obj)).expand(2.0);
            let c = match dir {
                // Opening from the middle.
                Dir::VertOut => Rect::from_center_size(r.center(), vec2(r.width() * k, r.height())),
                Dir::HorzOut => Rect::from_center_size(r.center(), vec2(r.width(), r.height() * k)),
                Dir::Right => Rect::from_min_max(pos2(r.max.x - r.width() * k, r.min.y), r.max),
                Dir::Up => Rect::from_min_max(r.min, pos2(r.max.x, r.min.y + r.height() * k)),
                Dir::Down => Rect::from_min_max(pos2(r.min.x, r.max.y - r.height() * k), r.max),
                _ => Rect::from_min_max(r.min, pos2(r.min.x + r.width() * k, r.max.y)),
            };
            q.set_clip_rect(c.intersect(q.clip_rect()));
        }
        if l.sx < 1.0 {
            // Turning about its upright axis: squeezed to its middle.
            let cx = xf.rect(&ch.frame(&shown)).center().x;
            let shapes = super::warp::capture(&q, |q| render::paint_object(q, &ch, &shown, xf, xf.sx, o));
            let sx = l.sx.max(0.001);
            super::warp::warp(&q, shapes, &|v| pos2(cx + (v.x - cx) * sx, v.y), 0.6 + 0.4 * sx, 1.0);
            continue;
        }
        render::paint_object(&q, &ch, &shown, xf, xf.sx, o);
    }
}

/// Where the slide goes in the show's `rect`: as large as fits, centered.
pub fn slide_rect(deck: &Deck, rect: Rect) -> Rect {
    let k = (rect.width() / deck.width.max(1.0)).min(rect.height() / deck.height.max(1.0));
    Rect::from_center_size(rect.center(), vec2(deck.width, deck.height) * k)
}

/// Paints the show in `rect`: the slide (or the end screen, or a blank
/// screen), during a transition both slides.
pub fn paint_show(p: &Painter, deck: &Deck, show: &Show, rect: Rect, now: f64, end_text: &str) {
    p.rect_filled(rect, 0.0, Color32::BLACK);
    if let Some(c) = show.blank {
        p.rect_filled(rect, 0.0, c);
        return;
    }
    let Some(i) = show.slide() else {
        p.text(rect.center_top() + vec2(0.0, 24.0), egui::Align2::CENTER_TOP, end_text, egui::FontId::proportional(18.0), Color32::from_gray(200));
        return;
    };
    let r = slide_rect(deck, rect);
    let looks = show.looks(deck, i, now);
    let new = |p: &Painter, r: Rect| paint_looks(p, deck, &deck.slides[i], r, &looks);
    match show.from.filter(|_| show.in_transition(deck, now)) {
        Some(from) => {
            let t = &deck.slides[i].transition;
            let prog = ease(((now - show.came) / t.dur.max(0.01) as f64) as f32);
            let old = |p: &Painter, r: Rect| {
                let all = show.looks_done(deck, from);
                paint_looks(p, deck, &deck.slides[from], r, &all)
            };
            if t.kind == TransKind::Morph {
                super::morph::paint(p, deck, &deck.slides[from], &deck.slides[i], r, prog);
            } else {
                paint_transition(p, r, t.kind, t.dir, prog, &old, &new);
            }
        }
        None => new(p, r),
    }
    show.ink.paint(&p.with_clip_rect(r.intersect(p.clip_rect())), Xf::for_slide(deck, r), i);
}

impl Show {
    /// Looks of slide `i` with everything played (as it was left).
    fn looks_done(&self, deck: &Deck, i: usize) -> Vec<(u32, Look)> {
        let mut out = Vec::new();
        let t = &deck.slides[i].timing;
        for anim in &t.anims {
            if !out.iter().any(|(id, _): &(u32, Look)| *id == anim.shape) {
                out.push((anim.shape, Look::default()));
            }
            let l = &mut out.iter_mut().find(|(id, _)| *id == anim.shape).unwrap().1;
            apply(l, anim.class, anim.effect, anim.dir, anim.paras, 1.0, frame_of(deck, &deck.slides[i], anim.shape), (deck.width, deck.height));
        }
        out
    }
}

/// Paints a transition at progress `k` (0..1) from `old` to `new` in `r`.
pub fn paint_transition(p: &Painter, r: Rect, kind: TransKind, dir: Dir, k: f32, old: &dyn Fn(&Painter, Rect), new: &dyn Fn(&Painter, Rect)) {
    let clip = |c: Rect| p.with_clip_rect(c.intersect(p.clip_rect()));
    let (w, h) = (r.width(), r.height());
    // Where a slide coming in `dir` is at `k`: off one edge to in place.
    let incoming = |k: f32| match dir {
        Dir::Right => vec2(-w * (1.0 - k), 0.0),
        Dir::Up => vec2(0.0, h * (1.0 - k)),
        Dir::Down => vec2(0.0, -h * (1.0 - k)),
        _ => vec2(w * (1.0 - k), 0.0),
    };
    match kind {
        TransKind::None => new(p, r),
        TransKind::Push => {
            let d = incoming(k);
            let back = incoming(0.0);
            let pc = clip(r);
            old(&pc, r.translate(d - back));
            new(&pc, r.translate(d));
        }
        TransKind::Cover => {
            old(p, r);
            new(&clip(r), r.translate(incoming(k)));
        }
        TransKind::Reveal => {
            new(p, r);
            let back = incoming(0.0);
            old(&clip(r), r.translate(-back * k));
        }
        TransKind::Wipe => {
            old(p, r);
            let c = match dir {
                Dir::Right => Rect::from_min_max(r.min, pos2(r.min.x + w * k, r.max.y)),
                Dir::Up => Rect::from_min_max(pos2(r.min.x, r.max.y - h * k), r.max),
                Dir::Down => Rect::from_min_max(r.min, pos2(r.max.x, r.min.y + h * k)),
                _ => Rect::from_min_max(pos2(r.max.x - w * k, r.min.y), r.max),
            };
            new(&clip(c), r);
        }
        TransKind::Split => {
            let out = matches!(dir, Dir::HorzOut | Dir::VertOut);
            let horz = matches!(dir, Dir::HorzIn | Dir::HorzOut);
            let c = |f: f32| if horz { Rect::from_center_size(r.center(), vec2(w, h * f)) } else { Rect::from_center_size(r.center(), vec2(w * f, h)) };
            if out {
                old(p, r);
                new(&clip(c(k)), r);
            } else {
                new(p, r);
                old(&clip(c(1.0 - k)), r);
            }
        }
        TransKind::Zoom => {
            if dir == Dir::Out {
                new(p, r);
                let mut q = clip(r);
                q.multiply_opacity(1.0 - k);
                old(&q, Rect::from_center_size(r.center(), r.size() * (1.0 + k)));
            } else {
                old(p, r);
                let mut q = clip(r);
                q.multiply_opacity(k);
                new(&q, Rect::from_center_size(r.center(), r.size() * k.max(0.01)));
            }
        }
        TransKind::Dissolve => {
            // The new slide in small squares, at random.
            old(p, r);
            let shapes = warp::capture(p, |p| new(p, r));
            let cols = 32;
            let side = w / cols as f32;
            let rows = (h / side).ceil() as u32;
            for y in 0..rows {
                let mut run: Option<u32> = None;
                for x in 0..=cols {
                    let on = x < cols && warp::noise(y * 997 + x) < k;
                    match (on, run) {
                        (true, None) => run = Some(x),
                        (false, Some(x0)) => {
                            let c = Rect::from_min_max(pos2(r.min.x + x0 as f32 * side, r.min.y + y as f32 * side), pos2(r.min.x + x as f32 * side, r.min.y + (y + 1) as f32 * side));
                            warp::replay(p, &shapes, c.intersect(r));
                            run = None;
                        }
                        _ => {}
                    }
                }
            }
        }
        TransKind::Blinds | TransKind::Checker => {
            // Slats (blinds) or squares a row apart (checker) opening
            // across (`Horz`) or down.
            old(p, r);
            let shapes = warp::capture(p, |p| new(p, r));
            let across = dir != Dir::Vert;
            let (long, short) = if across { (w, h) } else { (h, w) };
            let n = if kind == TransKind::Blinds { 6 } else { 8 };
            let cell = short / n as f32;
            let at = |a: f32, b: f32, len_a: f32, len_b: f32| {
                if across { Rect::from_min_size(pos2(r.min.x + a, r.min.y + b), vec2(len_a, len_b)) } else { Rect::from_min_size(pos2(r.min.x + b, r.min.y + a), vec2(len_b, len_a)) }
            };
            for i in 0..n {
                let b = i as f32 * cell;
                if kind == TransKind::Blinds {
                    // Each slat opens from its top (or left) edge.
                    let c = if across { Rect::from_min_size(pos2(r.min.x, r.min.y + b), vec2(w, cell * k)) } else { Rect::from_min_size(pos2(r.min.x + b, r.min.y), vec2(cell * k, h)) };
                    warp::replay(p, &shapes, c);
                } else {
                    let shift = if i % 2 == 1 { cell / 2.0 } else { 0.0 };
                    let mut a = shift - cell;
                    while a < long {
                        warp::replay(p, &shapes, at(a.max(0.0), b, (a + cell * k).min(long) - a.max(0.0), cell).intersect(r));
                        a += cell;
                    }
                }
            }
        }
        TransKind::Doors => {
            // The old slide opens in two halves towards the viewer; the
            // new one comes forward behind. Vertical doors are side by side.
            let across = dir != Dir::Horz;
            let mut q = clip(r);
            q.multiply_opacity(0.4 + 0.6 * k);
            new(&q, Rect::from_center_size(r.center(), r.size() * (0.8 + 0.2 * k)));
            let a = std::f32::consts::FRAC_PI_2 * 1.1 * k;
            let halves = if across {
                [(Rect::from_min_max(r.min, pos2(r.center().x, r.max.y)), [-w / 2.0, 0.0, 0.0], 1.0), (Rect::from_min_max(pos2(r.center().x, r.min.y), r.max), [w / 2.0, 0.0, 0.0], -1.0)]
            } else {
                [(Rect::from_min_max(r.min, pos2(r.max.x, r.center().y)), [0.0, -h / 2.0, 0.0], 1.0), (Rect::from_min_max(pos2(r.min.x, r.center().y), r.max), [0.0, h / 2.0, 0.0], -1.0)]
            };
            for (half, pivot, sign) in halves {
                let t = if across { warp::Turn { pivot, yaw: -sign * a, ..Default::default() } } else { warp::Turn { pivot, pitch: sign * a, ..Default::default() } };
                let shapes = warp::capture(p, |p| old(&p.with_clip_rect(half.intersect(p.clip_rect())), r));
                warp::warp(p, shapes, &|v| t.see(r.center(), 2.0 * w, v), 0.5 + 0.5 * t.facing().max(0.0), (1.0 - k) * 1.5);
            }
        }
        TransKind::Cube | TransKind::Flip | TransKind::Switch | TransKind::FallOver => {
            let c = r.center();
            let dist = 2.0 * w;
            let pi = std::f32::consts::PI;
            // Positive: towards the right (or down).
            let sign = if matches!(dir, Dir::Left | Dir::Up) { -1.0 } else { 1.0 };
            let upright = matches!(dir, Dir::Up | Dir::Down);
            let turn = |a: f32, pivot: [f32; 3], back: f32| {
                let off = [0.0, 0.0, back];
                if upright { warp::Turn { pivot, pitch: -a, off, ..Default::default() } } else { warp::Turn { pivot, yaw: a, off, ..Default::default() } }
            };
            // The slides as turned, the farthest first.
            let mut faces: Vec<(warp::Turn, bool, f32)> = Vec::new();
            match kind {
                TransKind::Cube => {
                    let depth = if upright { h } else { w } / 2.0;
                    let back = 0.35 * w * (pi * k).sin();
                    let th = std::f32::consts::FRAC_PI_2 * k;
                    faces.push((turn(sign * th, [0.0, 0.0, depth], back), false, 1.0));
                    faces.push((turn(sign * (th - std::f32::consts::FRAC_PI_2), [0.0, 0.0, depth], back), true, 1.0));
                }
                TransKind::Flip => {
                    let back = 0.5 * w * (pi * k).sin();
                    let a = sign * pi * k;
                    if k < 0.5 {
                        faces.push((turn(a, [0.0; 3], back), false, 1.0));
                    } else {
                        faces.push((turn(a - sign * pi, [0.0; 3], back), true, 1.0));
                    }
                }
                TransKind::Switch => {
                    // They swap places, one passing behind the other.
                    let s = (pi * k).sin();
                    let mut o = turn(-sign * 0.5 * s, [0.0; 3], 0.6 * w * k);
                    o.off[0] = sign * 0.6 * w * s;
                    let mut n = turn(sign * 0.5 * s, [0.0; 3], 0.6 * w * (1.0 - k));
                    n.off[0] = -sign * 0.6 * w * s;
                    if k < 0.5 {
                        faces.push((n, true, 1.0));
                        faces.push((o, false, 1.0));
                    } else {
                        faces.push((o, false, 1.0));
                        faces.push((n, true, 1.0));
                    }
                }
                _ => {
                    // The old slide tips over backwards on its foot.
                    faces.push((warp::Turn::default(), true, 1.0));
                    let fall = std::f32::consts::FRAC_PI_2 * k * k;
                    let t = warp::Turn { pivot: [0.0, h / 2.0, 0.0], pitch: -fall, roll: -sign * 0.3 * k, ..Default::default() };
                    faces.push((t, false, (1.0 - k) * 3.0));
                }
            }
            if kind == TransKind::Cube && faces.len() == 2 && faces[0].0.facing() > faces[1].0.facing() {
                faces.swap(0, 1);
            }
            for (t, is_new, alpha) in faces {
                let face = t.facing();
                if face <= 0.0 {
                    continue;
                }
                let shapes = warp::capture(p, |p| if is_new { new(p, r) } else { old(p, r) });
                let shade = if kind == TransKind::FallOver { 1.0 } else { 0.35 + 0.65 * face };
                warp::warp(p, shapes, &|v| t.see(c, dist, v), shade, alpha);
            }
        }
        // Fade, and the ones we don't play as fades (Morph is painted by
        // `super::morph` where both slides are known).
        _ => {
            old(p, r);
            let mut q = p.clone();
            q.multiply_opacity(k);
            new(&q, r);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_slides::anim::{Anim, Trigger};

    fn deck() -> Deck {
        let mut d = apex_slides::build::new_deck(0);
        for _ in 0..3 {
            let n = d.slides.len();
            apex_slides::edit::add_slide(&mut d, n, 0, 1);
        }
        d
    }

    #[test]
    fn clicks_go_through_slides() {
        let mut d = deck();
        d.slide_mut(1).hidden = true;
        let mut s = Show::new(&d, 0, 0.0);
        assert_eq!(s.order, vec![0, 2, 3]);
        s.next(&d, 1.0);
        assert_eq!(s.slide(), Some(2));
        s.prev(&d, 2.0);
        assert_eq!(s.slide(), Some(0));
        s.prev(&d, 2.0);
        assert_eq!(s.slide(), Some(0));
        s.key(&d, Key::End, Modifiers::NONE, 3.0);
        assert_eq!(s.slide(), Some(3));
        s.next(&d, 4.0);
        assert_eq!(s.slide(), None);
        s.next(&d, 5.0);
        assert!(s.done);
        // By number: "3" Enter is the deck's third slide... hidden, so not.
        let mut s = Show::new(&d, 0, 0.0);
        s.key(&d, Key::Num4, Modifiers::NONE, 1.0);
        s.key(&d, Key::Enter, Modifiers::NONE, 1.0);
        assert_eq!(s.slide(), Some(3));
        s.key(&d, Key::B, Modifiers::NONE, 1.0);
        assert_eq!(s.blank, Some(Color32::BLACK));
        s.key(&d, Key::Escape, Modifiers::NONE, 1.0);
        assert!(s.done);
    }

    #[test]
    fn loops_and_timings() {
        let mut d = deck();
        d.show.looped = true;
        for i in 0..d.slides.len() {
            d.slide_mut(i).transition.after = Some(2.0);
        }
        let mut s = Show::new(&d, 3, 0.0);
        s.tick(&d, 1.0);
        assert_eq!(s.slide(), Some(3));
        s.tick(&d, 2.5);
        assert_eq!(s.slide(), Some(0));
        s.prev(&d, 3.0);
        assert_eq!(s.slide(), Some(3));
    }

    #[test]
    fn animations_step_by_click() {
        let mut d = deck();
        let id = d.slides[0].objects[0].id;
        let other = d.slides[0].objects[1].id;
        let t = &mut d.slide_mut(0).timing;
        t.anims.push(Anim::new(id, Class::Entrance, Effect::Fade));
        let mut a = Anim::new(other, Class::Entrance, Effect::Fly);
        a.trigger = Trigger::AfterPrev;
        t.anims.push(a);
        t.anims.push(Anim::new(id, Class::Exit, Effect::Appear));
        let mut s = Show::new(&d, 0, 0.0);
        let look = |s: &Show, now: f64, id: u32| s.looks(&d, 0, now).into_iter().find(|x| x.0 == id).unwrap().1;
        assert!(!look(&s, 0.0, id).visible);
        s.next(&d, 1.0);
        let mid = look(&s, 1.25, id);
        assert!(mid.visible && mid.alpha > 0.1 && mid.alpha < 0.9);
        // The second comes after the first, flying in from the left.
        let fly = look(&s, 1.75, other);
        assert!(fly.visible && fly.dx < 0.0);
        assert_eq!(look(&s, 3.0, other).dx, 0.0);
        // A click while it plays jumps to its end.
        let mut t = s.clone();
        t.next(&d, 1.1);
        assert_eq!(t.step, 1);
        assert_eq!(look(&t, 1.1, id).alpha, 1.0);
        s.next(&d, 3.0);
        assert!(!look(&s, 3.0, id).visible);
        s.next(&d, 4.0);
        assert_eq!(s.slide(), Some(1));
        // Back: the previous slide with everything played.
        s.prev(&d, 5.0);
        assert_eq!((s.slide(), s.step), (Some(0), 2));
        assert!(!look(&s, 5.0, id).visible);
        s.prev(&d, 5.0);
        assert!(look(&s, 5.0, id).visible);
    }

    #[test]
    fn transitions_take_their_time() {
        let mut d = deck();
        d.slide_mut(1).transition.kind = TransKind::Push;
        d.slide_mut(1).transition.dur = 1.0;
        let mut s = Show::new(&d, 0, 0.0);
        s.next(&d, 1.0);
        assert!(s.in_transition(&d, 1.5));
        assert!(!s.in_transition(&d, 2.1));
        // A click ends it.
        s.next(&d, 1.2);
        assert!(!s.in_transition(&d, 1.3));
        assert_eq!(s.slide(), Some(1));
    }

    #[test]
    fn looked_objects() {
        let d = deck();
        let s = &d.slides[1];
        let ch = Chain::for_slide(&d, s);
        let o = &s.objects[1];
        let l = Look { dx: 10.0, scale: 0.5, hidden_paras: vec![(0, 0)], ..Default::default() };
        let m = looked(&ch, o, &l);
        let (a, b) = (ch.frame(o), m.sp.frame.unwrap());
        assert!((b.w - a.w * 0.5).abs() < 0.01 && (b.x + b.w / 2.0 - (a.x + a.w / 2.0 + 10.0)).abs() < 0.01);
    }

    #[test]
    fn effects_end_in_place() {
        let f = apex_slides::Frame { x: 100.0, y: 100.0, w: 50.0, h: 40.0, ..Default::default() };
        for e in Effect::ENTRANCE {
            let mut l = Look::default();
            apply(&mut l, Class::Entrance, e, Dir::Left, None, 1.0, f, (960.0, 540.0));
            assert!(l.visible && (l.alpha - 1.0).abs() < 1e-3 && l.dx.abs() < 1e-3 && l.dy.abs() < 1e-3 && (l.scale - 1.0).abs() < 1e-3 && (l.sx - 1.0).abs() < 1e-3, "{e:?} {l:?}");
            let mut l = Look::default();
            apply(&mut l, Class::Exit, e, Dir::Left, None, 1.0, f, (960.0, 540.0));
            assert!(!l.visible, "{e:?}");
        }
        for e in [Effect::Pulse, Effect::Teeter, Effect::Blink] {
            let mut l = Look::default();
            apply(&mut l, Class::Emphasis, e, Dir::Left, None, 1.0, f, (960.0, 540.0));
            assert!((l.scale - 1.0).abs() < 1e-3 && l.rot.abs() < 1e-3 && l.alpha == 1.0, "{e:?} {l:?}");
        }
        // Half way a bounce is above its place, a swivel narrow or wide.
        let mut l = Look::default();
        apply(&mut l, Class::Entrance, Effect::Bounce, Dir::Left, None, 0.2, f, (960.0, 540.0));
        assert!(l.dy < 0.0);
    }

    #[test]
    fn warp_turns() {
        let c = pos2(100.0, 100.0);
        let t = warp::Turn::default();
        assert_eq!(t.see(c, 500.0, pos2(130.0, 80.0)), pos2(130.0, 80.0));
        // Turned a quarter about its middle it is edge on.
        let t = warp::Turn { yaw: std::f32::consts::FRAC_PI_2, ..Default::default() };
        assert!(t.facing().abs() < 1e-5);
        assert!((t.see(c, 500.0, pos2(150.0, 100.0)).x - 100.0).abs() < 1e-3);
    }
}
