//! Handles for fingers: on a touch screen (Android) the handles of the
//! selected picture, chart or shape are drawn bigger and reach farther,
//! and each frame the views say where they are (`grip`), so that a finger
//! put down there drags at once (instead of scrolling the view) and two
//! fingers spread over the object scale it (crates/apex-app/src/android.rs).
//! With a mouse nothing changes.

use eframe::egui::{Painter, Pos2, Rect};

/// How far from a handle a press still takes it, given the mouse's.
pub const fn reach(mouse: f32) -> f32 {
    if cfg!(target_os = "android") && mouse < 18.0 { 18.0 } else { mouse }
}

/// The drawn size of a handle, given the mouse's.
pub const fn size(mouse: f32) -> f32 {
    if cfg!(target_os = "android") { mouse * 1.8 } else { mouse }
}

/// A place a finger takes at once: `zone`, and the handle that scales the
/// object (`corner`, dragged away from or towards `anchor`, the opposite one).
#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub struct Grip {
    pub zone: Rect,
    pub scale: Option<(Pos2, Pos2)>,
}

#[cfg(target_os = "android")]
static GRIPS: std::sync::Mutex<Vec<Grip>> = std::sync::Mutex::new(Vec::new());

/// A selected object (or another place to drag at once) drawn this frame
/// by `p` in `zone` (screen), taken up to `pad` farther, with its scaling
/// handle and the opposite one (`scale`: corner, anchor).
#[cfg_attr(not(target_os = "android"), allow(unused_variables))]
pub fn grip(p: &Painter, zone: Rect, pad: f32, scale: Option<(Pos2, Pos2)>) {
    #[cfg(target_os = "android")]
    {
        let zone = zone.expand(pad).intersect(p.clip_rect());
        if zone.is_positive() {
            GRIPS.lock().unwrap_or_else(|e| e.into_inner()).push(Grip { zone, scale });
        }
    }
}

/// The grips the last frame drew (taken: the next frame says its own).
#[cfg(target_os = "android")]
pub fn take() -> Vec<Grip> {
    std::mem::take(&mut *GRIPS.lock().unwrap_or_else(|e| e.into_inner()))
}

/// The handle nearest `q` within reach, of `handles`.
pub fn nearest<H: Copy>(handles: impl IntoIterator<Item = (H, Pos2)>, q: Pos2, mouse: f32) -> Option<H> {
    let r = reach(mouse);
    handles.into_iter().map(|(h, at)| (h, (at - q).length())).filter(|(_, d)| *d <= r).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(h, _)| h)
}
