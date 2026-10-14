//! Android: the library's entry point (`android_main`, called by the
//! NativeActivity), the on-screen keyboard, touch gestures, the system
//! clipboard and the file browser that stands for the system's file dialogs.

pub mod files;
mod thumbs;

use std::sync::OnceLock;

use eframe::egui;
use winit::platform::android::activity::AndroidApp;

static APP: OnceLock<AndroidApp> = OnceLock::new();

/// The Android package (see packaging/android/AndroidManifest.xml).
pub const PACKAGE: &str = "com.apexdesk.app";

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    let started = std::time::Instant::now();
    let _ = APP.set(app.clone());
    std::panic::set_hook(Box::new(|info| log(&format!("panic: {info}"))));
    environment(&app);
    let _ = egui_winit::clipboard::ANDROID.set((clipboard_text, |text| set_clipboard_text(text)));
    crate::diag::install();
    apex_io::picture::set_decoder(|b| crate::images::decode(b).map(|d| (d.width, d.height, d.rgba.to_vec())));
    let prefs = crate::settings::Settings::load();
    let mut options = crate::options(None, eframe::Renderer::Wgpu, &prefs, false);
    options.android_app = Some(app);
    let result = eframe::run_native("ApexDesk", options, Box::new(move |cc| Ok(Box::new(crate::app::App::new(cc, Vec::new(), None, started)))));
    if let Err(e) = result {
        log(&format!("eframe: {e}"));
    }
}

/// What the program reads from the environment on other systems: the
/// home folder (settings, recovery copies), the temporary folder and the
/// language.
fn environment(app: &AndroidApp) {
    let home = app.internal_data_path().unwrap_or_else(|| std::path::PathBuf::from(format!("/data/data/{PACKAGE}/files")));
    let tmp = home.join("tmp");
    let _ = std::fs::create_dir_all(&tmp);
    let config = app.config();
    let lang = match (config.language(), config.country()) {
        (Some(l), Some(c)) => format!("{l}_{c}.UTF-8"),
        (Some(l), None) => format!("{l}.UTF-8"),
        _ => "en_US.UTF-8".to_string(),
    };
    // SAFETY: at startup, before the program starts other threads.
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("TMPDIR", &tmp);
        std::env::set_var("LANG", lang);
    }
}

/// The device is a phone: its smallest side under 600 dp (Android's
/// line between phones and tablets; the keyboard doesn't change it).
pub fn small_screen() -> bool {
    crate::app::phone_width(APP.get().and_then(|a| a.config().smallest_screen_width_dp()))
}

/// A line in the system log (`adb logcat -s ApexDesk`).
pub fn log(text: &str) {
    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(prio: i32, tag: *const std::ffi::c_char, text: *const std::ffi::c_char) -> i32;
    }
    let text = std::ffi::CString::new(text.replace('\0', " ")).unwrap_or_default();
    // SAFETY: both strings end in a NUL; 6 is ANDROID_LOG_ERROR.
    unsafe { __android_log_write(6, c"ApexDesk".as_ptr(), text.as_ptr()) };
}

/// Each frame: the file browser is drawn, and the keyboard is looked
/// after once the frame is laid out.
pub fn frame(ctx: &egui::Context) {
    static HOOKED: std::sync::Once = std::sync::Once::new();
    HOOKED.call_once(|| ctx.on_end_pass("android-keyboard", std::sync::Arc::new(|ui: &mut egui::Ui| keyboard(ui.ctx()))));
    files::show(ctx);
}

/// The keyboard opens on a tap on a text field, not whenever one has the
/// focus (a document opens with its cursor in the text): the window asks
/// for it (eframe shows it) only after such a tap, until the focus leaves.
fn keyboard(ctx: &egui::Context) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static WANTED: AtomicBool = AtomicBool::new(false);
    // Asked for last frame: a tap now is on the text (or the dialog over it).
    static ASKED: AtomicBool = AtomicBool::new(false);
    let asks = ctx.output(|o| o.ime.is_some());
    let asked = ASKED.swap(asks, Ordering::Relaxed);
    // Not a long press (its right click ends a press too).
    let tap = ctx.input(|i| i.pointer.button_clicked(egui::PointerButton::Primary) && !i.pointer.button_clicked(egui::PointerButton::Secondary));
    // A text field the tap gave the focus to, or text that had it already.
    let on_field = || asked || ctx.memory(|m| m.focused()).and_then(|id| ctx.read_response(id)).is_some_and(|r| r.clicked());
    if asks && tap && on_field() {
        // Again after the user closed it, if it was asked for already.
        if WANTED.swap(true, Ordering::Relaxed)
            && let Some(app) = APP.get()
        {
            app.show_soft_input(true);
        }
    } else if !asks {
        WANTED.store(false, Ordering::Relaxed);
    }
    if !WANTED.load(Ordering::Relaxed) {
        ctx.output_mut(|o| o.ime = None);
    }
}

/// The window's area the system leaves to the program: what is drawn
/// below the keyboard, or under bars and a notch the system keeps on screen
/// although the activity asks for them hidden (some tablets), can't be seen
/// or touched. Full screen the window doesn't shrink for either (the
/// content rect stays whole): the activity tells their sizes
/// (packaging/android/java/.../MainActivity.java).
pub fn visible(raw: &mut egui::RawInput, pixels_per_point: f32) {
    let Some(app) = APP.get() else { return };
    let Some(screen) = raw.screen_rect.as_mut() else { return };
    let ppp = pixels_per_point.max(0.1);
    let r = app.content_rect();
    let [keyboard, left, top, right, bottom] = insets().map(|v| v.max(0) as f32 / ppp);
    let mut area = *screen;
    if r.bottom > r.top {
        area.min.y = area.min.y.max(r.top as f32 / ppp);
        area.max.y = area.max.y.min(r.bottom as f32 / ppp);
        area.min.x = area.min.x.max(r.left as f32 / ppp);
        area.max.x = area.max.x.min(r.right as f32 / ppp);
    }
    area.min.x = area.min.x.max(screen.min.x + left);
    area.min.y = area.min.y.max(screen.min.y + top);
    area.max.x = area.max.x.min(screen.max.x - right);
    area.max.y = area.max.y.min(screen.max.y - if keyboard > 0.0 { keyboard.max(bottom) } else { bottom });
    // Never less than a usable window (a wrong report shouldn't hide it).
    if area.height() > 100.0 && area.width() > 100.0 {
        *screen = area;
    }
}

/// The keyboard's height and the system's insets (left, top, right,
/// bottom) in pixels, as the activity last saw them.
fn insets() -> [i32; 5] {
    use jni::{jni_sig, jni_str};
    with_activity(|env, activity| {
        let class = env.get_object_class(activity)?;
        let mut get = |name| env.get_static_field(&class, name, jni_sig!("I")).and_then(|v| v.i());
        Ok([get(jni_str!("keyboardHeight"))?, get(jni_str!("insetLeft"))?, get(jni_str!("insetTop"))?, get(jni_str!("insetRight"))?, get(jni_str!("insetBottom"))?])
    })
    .unwrap_or([0; 5])
}

/// Files other apps opened with ApexDesk since the last call (the
/// activity takes them from the intent: their path, or a copy).
pub fn opened() -> Vec<std::path::PathBuf> {
    use jni::objects::JString;
    use jni::{jni_sig, jni_str};
    with_activity(|env, activity| {
        let class = env.get_object_class(activity)?;
        let list = env.call_static_method(&class, jni_str!("takeOpened"), jni_sig!("()Ljava/lang/String;"), &[])?.l()?;
        if list.is_null() {
            return Ok(Vec::new());
        }
        let list = env.cast_local::<JString>(list)?.try_to_string(env)?;
        Ok(list.lines().filter(|l| !l.is_empty()).map(std::path::PathBuf::from).collect())
    })
    .unwrap_or_default()
}

/// A file was saved: a copy of another app's file goes back to it. False
/// when it couldn't (the changes are only in the copy).
pub fn saved(path: &std::path::Path) -> bool {
    use jni::objects::JValue;
    use jni::{jni_sig, jni_str};
    with_activity(|env, activity| {
        let p = env.new_string(path.to_string_lossy())?;
        env.call_method(activity, jni_str!("saved"), jni_sig!("(Ljava/lang/String;)Z"), &[JValue::Object(&p)])?.z()
    })
    .unwrap_or(true)
}

/// Fingers on the screen, as on a phone: a tap clicks, a swipe scrolls
/// and, let go while moving, goes on and slows down; holding still and
/// lifting is a right click (context menus), holding still and then moving
/// drags (selects text or cells), and two fingers pan and zoom (the zoom
/// is egui's, from the touches: see `pinch`). On the selected picture,
/// chart or shape (and its handles, `crate::touch`) a finger takes hold at
/// once: it moves or resizes it, held still it opens its menu; two fingers
/// spread over it scale it, dragging its corner handle for them.
pub fn touch(ctx: &egui::Context, raw: &mut egui::RawInput) {
    use egui::{Event, PointerButton, TouchPhase, Vec2};
    use std::sync::Mutex;
    #[derive(Clone, Copy)]
    enum Finger {
        /// Down for less than the hold time, not moved: the press waits.
        /// `stop`: it stopped a fling, and lifted it doesn't click.
        Down { at: egui::Pos2, since: f64, stop: bool },
        /// Moved before the hold time: each move scrolls. `v` is the speed
        /// in points a second, measured until `when` (with `acc` moved since).
        Swipe { last: egui::Pos2, v: Vec2, when: f64, acc: Vec2 },
        /// Let go at `at` while swiping: scrolls on at `v`, slowing down
        /// (what is under `at`: the pointer stays there meanwhile).
        Fling { at: egui::Pos2, v: Vec2, when: f64 },
        /// Held still: the press went through.
        Held { at: egui::Pos2 },
        /// Held, then moved: an ordinary drag.
        Drag,
        /// Two fingers or more, until all are up; `center` is theirs.
        Pinch { center: Option<egui::Pos2> },
        /// Put down on a selected object (`grip`) at `at`: pressed at once.
        /// `moved`: beyond the slop (no menu when held).
        Grab { at: egui::Pos2, since: f64, moved: bool, grip: crate::touch::Grip },
        /// Two fingers scaling an object: its handle `corner` is dragged
        /// from `anchor` as far as the fingers spread from `d0` apart. A press
        /// already at `undo` (a grab) is taken back first, a frame a `step`.
        Scale { anchor: egui::Pos2, corner: egui::Pos2, d0: f32, s: f32, step: u8, undo: Option<egui::Pos2> },
    }
    struct State {
        finger: Option<Finger>,
        touches: Vec<(egui::TouchId, egui::Pos2)>,
        /// The selected objects the last frame drew.
        grips: Vec<crate::touch::Grip>,
        /// The finger left last frame: the pointer goes now.
        gone: bool,
    }
    static STATE: Mutex<State> = Mutex::new(State { finger: None, touches: Vec::new(), grips: Vec::new(), gone: false });
    const HOLD: f64 = 0.5;
    const SLOP: f32 = 12.0;
    /// A fling slows down by e (2.7 times) in 1 / FRICTION seconds.
    const FRICTION: f32 = 2.5;
    /// Slower than this (points a second), a fling stops.
    const MIN_SPEED: f32 = 40.0;
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let State { finger, touches, grips, gone } = &mut *state;
    *grips = crate::touch::take();
    let now = raw.time.unwrap_or(0.0);
    let press = |pos, button, pressed, modifiers| Event::PointerButton { pos, button, pressed, modifiers };
    let wheel = |delta: Vec2| Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta, phase: TouchPhase::Move, modifiers: egui::Modifiers::NONE };
    // The speed so far, with the moves since it was last measured.
    let measure = |v: Vec2, when: f64, acc: Vec2| if now - when >= 0.008 { (v * 0.2 + acc / (now - when) as f32 * 0.8, now, Vec2::ZERO) } else { (v, when, acc) };
    let mut out = Vec::with_capacity(raw.events.len());
    // After a right click the pointer stays, for the menu to open there,
    // and after a fling, for what scrolls to stay under it.
    let mut keep_pointer = false;
    // A finger lifted leaves the pointer where it was for a frame: what
    // reads the pointer on a click or at the end of a drag finds it.
    if std::mem::take(gone) {
        out.push(Event::PointerGone);
    }
    for e in std::mem::take(&mut raw.events) {
        if let Event::Touch { id, phase, pos, .. } = e {
            // The first finger down: touches whose end went elsewhere (to the
            // screen saver, the system's bars) are gone.
            if phase == TouchPhase::Start && matches!(finger, None | Some(Finger::Fling { .. })) {
                touches.clear();
            }
            touches.retain(|(t, _)| *t != id);
            if matches!(phase, TouchPhase::Start | TouchPhase::Move) {
                touches.push((id, pos));
            }
            if touches.len() >= 2 && !matches!(finger, Some(Finger::Pinch { .. } | Finger::Scale { .. })) {
                let (a, b) = (touches[0].1, touches[1].1);
                let center = a + (b - a) / 2.0;
                let (grip, undo) = match *finger {
                    Some(Finger::Grab { at, grip, .. }) => (Some(grip), Some(at)),
                    _ => (grips.iter().find(|g| g.scale.is_some() && g.zone.contains(center)).copied(), None),
                };
                if let Some((corner, anchor)) = grip.and_then(|g| g.scale) {
                    *finger = Some(Finger::Scale { anchor, corner, d0: a.distance(b).max(1.0), s: 1.0, step: 0, undo });
                } else {
                    // A press that went through ends here.
                    if matches!(finger, Some(Finger::Held { .. } | Finger::Drag | Finger::Grab { .. })) {
                        out.push(press(touches[0].1, PointerButton::Primary, false, egui::Modifiers::NONE));
                    }
                    *finger = Some(Finger::Pinch { center: None });
                }
            }
            // Scaling, egui doesn't see the fingers (it would zoom the view).
            if matches!(finger, Some(Finger::Scale { .. })) && matches!(phase, TouchPhase::Start | TouchPhase::Move) {
                continue;
            }
            out.push(e);
            continue;
        }
        match (e, *finger) {
            (Event::PointerGone, _) => {
                if !keep_pointer {
                    *gone = true;
                    ctx.request_repaint();
                }
            }
            // Two fingers: no clicks, no drags, no scrolling by the first.
            (Event::PointerMoved(_) | Event::PointerButton { .. }, Some(Finger::Pinch { .. } | Finger::Scale { .. })) => {}
            (Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers }, state) => {
                let grip = grips.iter().find(|g| g.zone.contains(pos)).copied().or_else(|| drags_at(ctx, pos).then_some(crate::touch::Grip { zone: egui::Rect::NOTHING, scale: None }));
                if let Some(grip) = grip {
                    // On a selected object: it is taken at once.
                    out.push(press(pos, PointerButton::Primary, true, modifiers));
                    *finger = Some(Finger::Grab { at: pos, since: now, moved: false, grip });
                } else {
                    *finger = Some(Finger::Down { at: pos, since: now, stop: matches!(state, Some(Finger::Fling { .. })) });
                }
            }
            (Event::PointerMoved(p), Some(Finger::Grab { at, since, moved, grip })) => {
                *finger = Some(Finger::Grab { at, since, moved: moved || at.distance(p) > SLOP, grip });
                out.push(Event::PointerMoved(p));
            }
            (Event::PointerMoved(p), Some(Finger::Down { at, since, .. })) => {
                if at.distance(p) > SLOP {
                    out.push(wheel(p - at));
                    *finger = Some(Finger::Swipe { last: p, v: Vec2::ZERO, when: since, acc: p - at });
                }
            }
            (Event::PointerMoved(p), Some(Finger::Swipe { last, v, when, acc })) => {
                out.push(wheel(p - last));
                let (v, when, acc) = measure(v, when, acc + (p - last));
                *finger = Some(Finger::Swipe { last: p, v, when, acc });
            }
            (Event::PointerMoved(p), Some(Finger::Held { at })) => {
                if at.distance(p) > SLOP {
                    *finger = Some(Finger::Drag);
                }
                out.push(Event::PointerMoved(p));
            }
            (Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers }, state) => {
                *finger = None;
                match state {
                    Some(Finger::Down { stop: true, .. }) => {}
                    Some(Finger::Down { at, .. }) => {
                        out.push(press(at, PointerButton::Primary, true, modifiers));
                        out.push(press(at, PointerButton::Primary, false, modifiers));
                    }
                    Some(Finger::Swipe { last, v, when, .. }) => {
                        // Still moving when let go (not stopped first).
                        if now - when < 0.1 && v.length() > 4.0 * MIN_SPEED {
                            *finger = Some(Finger::Fling { at: last, v, when: now });
                            keep_pointer = true;
                        }
                    }
                    Some(Finger::Held { at }) => {
                        out.push(press(at, PointerButton::Primary, false, modifiers));
                        out.push(press(at, PointerButton::Secondary, true, modifiers));
                        out.push(press(at, PointerButton::Secondary, false, modifiers));
                        keep_pointer = true;
                    }
                    Some(Finger::Drag | Finger::Grab { .. } | Finger::Fling { .. } | Finger::Pinch { .. } | Finger::Scale { .. }) | None => out.push(press(pos, PointerButton::Primary, false, modifiers)),
                }
            }
            (e, _) => out.push(e),
        }
    }
    raw.events = out;
    match *finger {
        Some(Finger::Down { at, since, stop }) => {
            if now - since >= HOLD {
                raw.events.push(press(at, PointerButton::Primary, true, egui::Modifiers::NONE));
                *finger = Some(Finger::Held { at });
            } else if !stop {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(HOLD - (now - since)));
            }
        }
        Some(Finger::Fling { at, v, when }) => {
            let dt = (now - when) as f32;
            raw.events.push(Event::PointerMoved(at));
            raw.events.push(wheel(v * dt));
            let v = v * (-dt * FRICTION).exp();
            if v.length() > MIN_SPEED {
                *finger = Some(Finger::Fling { at, v, when: now });
                ctx.request_repaint();
            } else {
                *finger = None;
                raw.events.push(Event::PointerGone);
            }
        }
        Some(Finger::Grab { at, since, moved: false, .. }) => {
            // Held still: its menu, as a right click. The finger does
            // nothing more until it is up.
            if now - since >= HOLD {
                raw.events.push(press(at, PointerButton::Primary, false, egui::Modifiers::NONE));
                raw.events.push(press(at, PointerButton::Secondary, true, egui::Modifiers::NONE));
                raw.events.push(press(at, PointerButton::Secondary, false, egui::Modifiers::NONE));
                *finger = Some(Finger::Pinch { center: None });
            } else {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(HOLD - (now - since)));
            }
        }
        Some(Finger::Scale { anchor, corner, d0, s: last, step, undo }) => {
            let none = egui::Modifiers::NONE;
            let to = |s: f32| anchor + (corner - anchor) * s;
            let spread = (touches.len() >= 2).then(|| (touches[0].1.distance(touches[1].1) / d0).clamp(0.05, 20.0));
            match (spread, step, undo) {
                // The fingers are up: the drag ends where they left it.
                (None, 0 | 1, Some(at)) => {
                    raw.events.push(Event::PointerMoved(at));
                    raw.events.push(press(at, PointerButton::Primary, false, none));
                }
                (None, 3.., _) => {
                    raw.events.push(Event::PointerMoved(to(last)));
                    raw.events.push(press(to(last), PointerButton::Primary, false, none));
                    *gone = true;
                }
                (None, ..) => {}
                // The grab goes back to where it started (a picture moved a
                // little stays), then lets go, then the corner is taken.
                (Some(_), 0, Some(at)) => raw.events.push(Event::PointerMoved(at)),
                (Some(_), 1, Some(at)) => raw.events.push(press(at, PointerButton::Primary, false, none)),
                (Some(_), 0..=2, _) => {
                    raw.events.push(Event::PointerMoved(corner));
                    raw.events.push(press(corner, PointerButton::Primary, true, none));
                }
                (Some(s), ..) => raw.events.push(Event::PointerMoved(to(s))),
            }
            if spread.is_none() {
                *finger = (!touches.is_empty()).then_some(Finger::Pinch { center: None });
            } else {
                let step = match (step, undo) {
                    (0, Some(_)) => 1,
                    (1, _) => 2,
                    _ => 3,
                };
                let s = if step == 3 { spread.unwrap_or(last) } else { 1.0 };
                *finger = Some(Finger::Scale { anchor, corner, d0, s, step, undo });
                ctx.request_repaint();
            }
        }
        Some(Finger::Pinch { center }) => {
            if touches.is_empty() {
                *finger = None;
            } else if touches.len() >= 2 {
                let c = touches.iter().fold(Vec2::ZERO, |s, (_, p)| s + p.to_vec2()) / touches.len() as f32;
                if let Some(old) = center {
                    raw.events.push(wheel(c - old.to_vec2()));
                }
                *finger = Some(Finger::Pinch { center: Some(c.to_pos2()) });
            } else {
                // One finger left: it pans from where it is now.
                *finger = Some(Finger::Pinch { center: None });
            }
        }
        _ => {}
    }
}

/// Whether a finger put down at `p` is on something dragged rather than
/// scrolled (a slider, a number dragged, a scroll bar, a splitter, a
/// dialog's title bar): a thin widget that drags, as the last frame laid
/// them out. Pages, lists and the grid scroll under a swipe.
fn drags_at(ctx: &egui::Context, p: egui::Pos2) -> bool {
    let Some(layer) = ctx.layer_id_at(p) else { return false };
    let to_global = ctx.layer_transform_to_global(layer);
    ctx.viewport(|v| {
        v.prev_pass.widgets.get_layer(layer).any(|w| {
            let r = to_global.map_or(w.interact_rect, |t| t * w.interact_rect);
            w.enabled && w.sense.senses_drag() && r.contains(p) && r.width().min(r.height()) <= 32.0
        })
    })
}

/// The zoom of two fingers this frame, if they moved apart or together.
pub fn pinch(ctx: &egui::Context) -> Option<f32> {
    ctx.input(|i| i.multi_touch()).map(|t| t.zoom_delta).filter(|z| (z - 1.0).abs() > 1e-4)
}

/// The system clipboard's text (what other apps copied too).
pub fn clipboard_text() -> Option<String> {
    use jni::objects::{JString, JValue};
    use jni::{jni_sig, jni_str};
    with_activity(|env, activity| {
        let manager = clipboard_manager(env, activity)?;
        let clip = env.call_method(&manager, jni_str!("getPrimaryClip"), jni_sig!("()Landroid/content/ClipData;"), &[])?.l()?;
        if clip.is_null() || env.call_method(&clip, jni_str!("getItemCount"), jni_sig!("()I"), &[])?.i()? < 1 {
            return Ok(None);
        }
        let item = env.call_method(&clip, jni_str!("getItemAt"), jni_sig!("(I)Landroid/content/ClipData$Item;"), &[JValue::Int(0)])?.l()?;
        let text = env.call_method(&item, jni_str!("coerceToText"), jni_sig!("(Landroid/content/Context;)Ljava/lang/CharSequence;"), &[JValue::Object(activity)])?.l()?;
        if text.is_null() {
            return Ok(None);
        }
        let text = env.call_method(&text, jni_str!("toString"), jni_sig!("()Ljava/lang/String;"), &[])?.l()?;
        Ok(Some(env.cast_local::<JString>(text)?.try_to_string(env)?))
    })
    .unwrap_or_else(|e| {
        log(&format!("clipboard: {e}"));
        None
    })
}

/// Puts text on the system clipboard, for other apps too.
pub fn set_clipboard_text(text: &str) {
    use jni::objects::JValue;
    use jni::{jni_sig, jni_str};
    let r = with_activity(|env, activity| {
        let manager = clipboard_manager(env, activity)?;
        let label = env.new_string("ApexDesk")?;
        let text = env.new_string(text)?;
        let clip = env.call_static_method(jni_str!("android/content/ClipData"), jni_str!("newPlainText"), jni_sig!("(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;"), &[JValue::Object(&label), JValue::Object(&text)])?.l()?;
        env.call_method(&manager, jni_str!("setPrimaryClip"), jni_sig!("(Landroid/content/ClipData;)V"), &[JValue::Object(&clip)])?;
        Ok(())
    });
    if let Err(e) = r {
        log(&format!("clipboard: {e}"));
    }
}

fn clipboard_manager<'local>(env: &mut jni::Env<'local>, activity: &jni::objects::JObject) -> jni::errors::Result<jni::objects::JObject<'local>> {
    use jni::objects::JValue;
    use jni::{jni_sig, jni_str};
    let name = env.new_string("clipboard")?;
    env.call_method(activity, jni_str!("getSystemService"), jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"), &[JValue::Object(&name)])?.l()
}

/// Runs `f` with the Java environment and the activity.
fn with_activity<T>(f: impl FnOnce(&mut jni::Env, &jni::objects::JObject) -> jni::errors::Result<T>) -> Result<T, String> {
    let app = APP.get().ok_or("no activity")?;
    // SAFETY: the pointers are the process's Java VM and the activity,
    // alive while the program runs.
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    vm.attach_current_thread(|env| {
        let activity = unsafe { jni::objects::JObject::from_raw(env, app.activity_as_ptr().cast()) };
        f(env, &activity)
    })
    .map_err(|e: jni::errors::Error| e.to_string())
}

/// Whether the program may read the files of the shared storage
/// (MainActivity.hasFileAccess: "All files access" from Android 11, the
/// storage permission before). Without it Android lists the folders but
/// not the files in them.
pub fn all_files_access() -> bool {
    use jni::{jni_sig, jni_str};
    with_activity(|env, activity| env.call_method(activity, jni_str!("hasFileAccess"), jni_sig!("()Z"), &[])?.z()).unwrap_or(true)
}

/// Asks the user for it (MainActivity.askFileAccess): the settings page
/// where the program is let reach every file, or the permission's dialog.
pub fn ask_all_files_access() {
    use jni::{jni_sig, jni_str};
    if let Err(e) = with_activity(|env, activity| {
        env.call_method(activity, jni_str!("askFileAccess"), jni_sig!("()V"), &[])?;
        Ok(())
    }) {
        log(&format!("all files access: {e}"));
    }
}
