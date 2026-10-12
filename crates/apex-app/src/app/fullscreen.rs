//! View > Full screen (F11) from the window's real state: the toggle reads
//! what the window reports, not what was last asked, so a slide show or a
//! PDF presentation that changed it in between cannot leave the button
//! out of step. Leaving is checked after a moment and retried once, and
//! the size, place and maximized state from before come back.
//!
//! winit's X11 backend (the default, XWayland under GNOME) reports the
//! state it last asked for, not the window manager's, and drops a request
//! equal to it (winit 0.30 `x11/window.rs` `set_fullscreen_inner`), so a
//! window the manager kept in full screen is also told by its size: it
//! still covers the whole monitor though it did not before.

use eframe::egui::{self, Rect, Vec2, ViewportCommand};

/// How long the window gets to leave full screen before a retry, seconds.
pub const CHECK: f64 = 0.3;

/// What the window reports about itself, in points.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Seen {
    pub full: Option<bool>,
    pub maximized: bool,
    pub inner: Option<Rect>,
    pub outer: Option<Rect>,
    pub monitor: Option<Vec2>,
}

impl Seen {
    pub fn of(info: &egui::ViewportInfo) -> Self {
        Seen { full: info.fullscreen, maximized: info.maximized.unwrap_or(false), inner: info.inner_rect, outer: info.outer_rect, monitor: info.monitor_size }
    }

    /// The window fills its whole monitor, as only full screen does.
    fn covers(&self) -> bool {
        match (self.inner, self.monitor) {
            (Some(r), Some(m)) => m.x > 0.0 && (r.width() - m.x).abs() <= 1.0 && (r.height() - m.y).abs() <= 1.0,
            _ => false,
        }
    }
}

struct Leave {
    at: f64,
    retried: bool,
}

#[derive(Default)]
pub struct FullScreen {
    /// What was last asked, for when the window does not say.
    asked: bool,
    /// The window just before entering full screen.
    before: Option<Seen>,
    leave: Option<Leave>,
}

impl FullScreen {
    #[cfg(test)]
    pub fn asked(&self) -> bool {
        self.asked
    }

    /// Whether the window is in full screen now.
    pub fn is_full(&self, seen: &Seen) -> bool {
        seen.full.unwrap_or(self.asked)
    }

    /// A leave is being watched: keep frames coming.
    pub fn pending(&self) -> bool {
        self.leave.is_some()
    }

    /// F11 or the button: the commands that flip the real state.
    pub fn toggle(&mut self, seen: &Seen, now: f64) -> Vec<ViewportCommand> {
        if self.is_full(seen) {
            self.asked = false;
            self.leave = Some(Leave { at: now, retried: false });
            vec![ViewportCommand::Fullscreen(false)]
        } else {
            self.asked = true;
            self.leave = None;
            self.before = Some(*seen);
            vec![ViewportCommand::Fullscreen(true)]
        }
    }

    fn stuck(&self, seen: &Seen) -> bool {
        seen.full == Some(true) || (seen.covers() && !self.before.is_some_and(|b| b.covers()))
    }

    /// Each frame: once a leave has had its moment, retry it if the
    /// window stayed in full screen, else put back what it was before.
    pub fn tick(&mut self, seen: &Seen, now: f64) -> Vec<ViewportCommand> {
        let stuck = self.stuck(seen);
        let Some(l) = &mut self.leave else { return Vec::new() };
        if now - l.at < CHECK {
            return Vec::new();
        }
        if stuck {
            if l.retried {
                self.leave = None;
                return Vec::new();
            }
            l.retried = true;
            l.at = now;
            // winit already believes the window left: a second "leave"
            // alone would be dropped, so enter and leave again.
            return if seen.full == Some(true) {
                vec![ViewportCommand::Fullscreen(false)]
            } else {
                vec![ViewportCommand::Fullscreen(true), ViewportCommand::Fullscreen(false)]
            };
        }
        self.leave = None;
        self.restore(seen)
    }

    /// Back to the size, place and maximized state from before entering,
    /// only where the window differs from it.
    fn restore(&mut self, seen: &Seen) -> Vec<ViewportCommand> {
        let Some(b) = self.before.take() else { return Vec::new() };
        let mut out = Vec::new();
        if b.maximized {
            if !seen.maximized {
                out.push(ViewportCommand::Maximized(true));
            }
            return out;
        }
        if seen.maximized {
            out.push(ViewportCommand::Maximized(false));
        }
        if let (Some(was), Some(is)) = (b.inner, seen.inner)
            && (seen.maximized || (was.size() - is.size()).length() > 2.0)
        {
            out.push(ViewportCommand::InnerSize(was.size()));
        }
        if let (Some(was), Some(is)) = (b.outer, seen.outer)
            && (seen.maximized || (was.min - is.min).length() > 2.0)
        {
            out.push(ViewportCommand::OuterPosition(was.min));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Pos2, pos2, vec2};

    const MON: Vec2 = Vec2::new(1920.0, 1080.0);

    fn window(full: Option<bool>) -> Seen {
        Seen { full, maximized: false, inner: Some(Rect::from_min_size(pos2(100.0, 80.0), vec2(1280.0, 800.0))), outer: Some(Rect::from_min_size(pos2(100.0, 50.0), vec2(1280.0, 830.0))), monitor: Some(MON) }
    }

    fn screen(full: Option<bool>) -> Seen {
        let r = Rect::from_min_size(Pos2::ZERO, MON);
        Seen { full, maximized: false, inner: Some(r), outer: Some(r), monitor: Some(MON) }
    }

    fn is_cmd(c: &[ViewportCommand], want: &[ViewportCommand]) -> bool {
        c.len() == want.len() && c.iter().zip(want).all(|(a, b)| a == b)
    }

    #[test]
    fn toggle_follows_the_real_state_not_the_remembered_one() {
        let mut f = FullScreen::default();
        // A slide show put the window in full screen behind our back:
        // the toggle leaves it instead of asking to enter again.
        assert!(is_cmd(&f.toggle(&screen(Some(true)), 0.0), &[ViewportCommand::Fullscreen(false)]));
        // A PDF show left it: the toggle enters though we last left.
        assert!(is_cmd(&f.toggle(&window(Some(false)), 1.0), &[ViewportCommand::Fullscreen(true)]));
        assert!(f.asked());
        // No report from the window: what was asked stands.
        assert!(f.is_full(&Seen::default()));
        assert!(is_cmd(&f.toggle(&Seen::default(), 2.0), &[ViewportCommand::Fullscreen(false)]));
    }

    #[test]
    fn a_clean_leave_restores_nothing_that_is_already_right() {
        let mut f = FullScreen::default();
        f.toggle(&window(Some(false)), 0.0);
        f.toggle(&screen(Some(true)), 1.0);
        assert!(f.pending());
        assert!(f.tick(&window(Some(false)), 1.1).is_empty());
        assert!(f.tick(&window(Some(false)), 1.4).is_empty());
        assert!(!f.pending());
    }

    #[test]
    fn a_leave_the_window_ignored_is_retried_once() {
        let mut f = FullScreen::default();
        f.toggle(&window(Some(false)), 0.0);
        f.toggle(&screen(Some(true)), 1.0);
        // Wayland reports it is still full: plain leave again.
        assert!(is_cmd(&f.tick(&screen(Some(true)), 1.4), &[ViewportCommand::Fullscreen(false)]));
        assert!(f.pending());
        // Still stuck after the retry: give up, no loop.
        assert!(f.tick(&screen(Some(true)), 1.8).is_empty());
        assert!(!f.pending());
    }

    #[test]
    fn x11_stale_state_is_seen_by_size_and_forced() {
        let mut f = FullScreen::default();
        f.toggle(&window(Some(false)), 0.0);
        f.toggle(&screen(Some(true)), 1.0);
        // winit says "not full" but the window still covers the monitor.
        assert!(is_cmd(&f.tick(&screen(Some(false)), 1.4), &[ViewportCommand::Fullscreen(true), ViewportCommand::Fullscreen(false)]));
        // The retry worked: the old size and place come back.
        let mut moved = window(Some(false));
        moved.inner = Some(Rect::from_min_size(pos2(0.0, 30.0), vec2(1000.0, 700.0)));
        moved.outer = Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 730.0)));
        let c = f.tick(&moved, 1.8);
        assert!(is_cmd(&c, &[ViewportCommand::InnerSize(vec2(1280.0, 800.0)), ViewportCommand::OuterPosition(pos2(100.0, 50.0))]));
    }

    #[test]
    fn maximized_before_is_maximized_after() {
        let mut f = FullScreen::default();
        let mut max = window(Some(false));
        max.maximized = true;
        f.toggle(&max, 0.0);
        f.toggle(&screen(Some(true)), 1.0);
        assert!(is_cmd(&f.tick(&window(Some(false)), 1.4), &[ViewportCommand::Maximized(true)]));
    }

    #[test]
    fn a_window_that_always_filled_the_monitor_is_not_stuck() {
        let mut f = FullScreen::default();
        f.toggle(&screen(Some(false)), 0.0);
        f.toggle(&screen(Some(true)), 1.0);
        assert!(f.tick(&screen(Some(false)), 1.4).is_empty());
        assert!(!f.pending());
    }
}
