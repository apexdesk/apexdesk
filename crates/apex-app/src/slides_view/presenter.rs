//! Running the show from the editor: fullscreen in the main window, or
//! the presenter view (current and next slide, notes, clock and time
//! running) with the slides in a second fullscreen window. Also the
//! toolstrip's Transitions, Animations and Slide Show tabs and the commands
//! they edit the slides with.

use apex_slides::anim::{Anim, Class, Dir, Effect, TransKind, Trigger};
use eframe::egui::{self, Color32, Id, Rect, RichText, Sense, Stroke, vec2};

use super::show::{self, Show};
use super::show_ink;
use super::{SlidesView, render};
use crate::common::{self, Shell};
use crate::i18n::{tr, trf};
use crate::icons::Icon;
use crate::toolstrip::{self, Body};
use crate::theme;

/// Seconds the mouse rests before its arrow hides in the show.
const MOUSE_REST: f64 = 3.0;

/// Undo steps dragging a duration joins.
const DUR_KEY: u64 = 0x7_2A45;

/// Transitions in the toolstrip (all of them in its menu).
const TRANSITIONS_SHOWN: [TransKind; 8] = [TransKind::None, TransKind::Fade, TransKind::Push, TransKind::Wipe, TransKind::Split, TransKind::Cube, TransKind::Flip, TransKind::Morph];

/// Effects offered per class, in the gallery's order.
pub const ENTRANCE: [Effect; 10] = Effect::ENTRANCE;
pub const EMPHASIS: [Effect; 6] = Effect::EMPHASIS;

/// `h:mm:ss`.
pub fn clock(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

impl SlidesView {
    // --- the show -----------------------------------------------------------------------

    pub fn showing(&self) -> bool {
        self.show.is_some()
    }

    /// Starts the show from slide `from`; `presenter`: with the presenter
    /// view here and the slides in a window of their own.
    pub fn start_show(&mut self, ctx: &egui::Context, from: usize, presenter: bool, fullscreen: bool) {
        self.commit_edit();
        if self.pres.slides.is_empty() {
            return;
        }
        let now = ctx.input(|i| i.time);
        let mut s = Show::new(&self.pres.deck, from.min(self.pres.slides.len() - 1), now);
        s.presenter = presenter;
        s.was_full = fullscreen;
        s.ink.pointer = self.pointer;
        self.stop_media();
        if !presenter && !fullscreen {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
        }
        self.show = Some(s);
    }

    /// Back to editing, on the slide the show was on.
    fn end_show(&mut self, ctx: &egui::Context, s: &Show) {
        if !s.presenter && !s.was_full {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        if let Some(i) = s.slide().or(s.order.last().copied()) {
            self.go(i);
        }
        if !s.ink.is_empty() && !self.read_only {
            self.ink_keep = Some(s.ink.marks.clone());
        }
        self.media.end_show();
        self.show = None;
    }

    /// After a show with pen marks: keep them on their slides, or not.
    pub(super) fn ink_dialog(&mut self, ctx: &egui::Context) {
        if self.ink_keep.is_none() {
            return;
        }
        let res = crate::widgets::dialog(ctx, "slides_ink_keep", tr("slides.ink_title"), 360.0, |ui| {
            ui.label(tr("slides.ink_keep"));
        });
        match res {
            crate::widgets::DialogResult::Ok => {
                let marks = self.ink_keep.take().unwrap_or_default();
                let deck = self.pres.edit("slides.undo.ink");
                let mut slides: Vec<_> = marks.into_iter().filter(|(i, m)| *i < deck.slides.len() && !m.is_empty()).collect();
                slides.sort_by_key(|(i, _)| *i);
                for (i, marks) in slides {
                    let s = deck.slide_mut(i);
                    for m in &marks {
                        let id = s.next_id();
                        if let Some(o) = show_ink::mark_object(id, m) {
                            s.objects.push(o);
                        }
                    }
                }
            }
            crate::widgets::DialogResult::Cancel => self.ink_keep = None,
            crate::widgets::DialogResult::None => {}
        }
    }

    /// The show's screen, or the presenter view with the slides in their
    /// own window.
    pub(super) fn show_ui(&mut self, ui: &mut egui::Ui) {
        let Some(mut s) = self.show.take() else { return };
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let deck = &self.pres.deck;
        let end = tr("slides.show_end");
        let playing = &mut self.media;
        let mut screen = |ui: &mut egui::Ui, s: &mut Show| {
            let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
            // A click on a video plays or pauses it.
            s.media_click = playing.show_click(deck, s, &resp);
            s.input(ui.ctx(), &resp, deck);
            let now = ui.ctx().input(|i| i.time);
            show::paint_show(ui.painter(), deck, s, rect, now, end);
            playing.show_frame(ui.painter(), deck, s, rect, now);
            let bar = !s.kiosk && s.blank.is_none() && s.slide().is_some() && show_ink::bar(ui, rect, &mut s.ink);
            if bar {
                // The mouse is on the pointer bar: the arrow shows.
            } else if matches!(s.ink.pointer, show_ink::Pointer::Pen | show_ink::Pointer::Eraser) && !s.kiosk {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            } else if s.kiosk || s.blank.is_none() && s.slide().is_some() {
                // The arrow shows while the mouse moves and hides when it
                // rests; on a video or sound it's a hand (a click plays it).
                let (moved, hover) = ui.input(|i| (i.pointer.delta() != egui::Vec2::ZERO || i.pointer.any_pressed(), i.pointer.hover_pos()));
                if moved {
                    s.moved_t = now;
                    ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(MOUSE_REST + 0.05));
                }
                let icon = if s.kiosk || now - s.moved_t > MOUSE_REST {
                    egui::CursorIcon::None
                } else if matches!(s.ink.pointer, show_ink::Pointer::Arrow) && hover.is_some_and(|q| super::media::show_media_at(deck, s, rect, q)) {
                    egui::CursorIcon::PointingHand
                } else {
                    egui::CursorIcon::Default
                };
                ui.ctx().set_cursor_icon(icon);
            }
        };
        if s.presenter {
            // The slides go right of this window's screen (the second
            // monitor, most often), fullscreen there.
            let at = ctx.input(|i| i.viewport().monitor_size).map_or(egui::pos2(40.0, 40.0), |m| egui::pos2(m.x + 20.0, 20.0));
            let builder = egui::ViewportBuilder::default().with_title(tr("slides.show")).with_position(at).with_inner_size(vec2(960.0, 540.0)).with_fullscreen(true);
            ctx.show_viewport_immediate(egui::ViewportId::from_hash_of(("slideshow", self.uid)), builder, |ui, _| {
                egui::CentralPanel::default_margins().frame(egui::Frame::new().fill(Color32::BLACK)).show(ui, |ui| screen(ui, &mut s));
                if ui.ctx().input(|i| i.viewport().close_requested()) {
                    s.done = true;
                }
            });
            egui::CentralPanel::default_margins().frame(egui::Frame::new().fill(Color32::from_gray(24)).inner_margin(12)).show(ui, |ui| presenter_ui(ui, deck, &mut s, now));
        } else {
            egui::CentralPanel::default_margins().frame(egui::Frame::new().fill(Color32::BLACK)).show(ui, |ui| screen(ui, &mut s));
        }
        if s.done { self.end_show(&ctx, &s) } else { self.show = Some(s) }
    }

    // --- transitions --------------------------------------------------------------------

    fn edit_transition(&mut self, key: Option<u64>, f: impl Fn(&mut apex_slides::Transition)) {
        if self.read_only || self.pres.slides.is_empty() {
            return;
        }
        let cur = self.cur;
        let deck = match key {
            Some(k) => self.pres.edit_merge("slides.undo.transition", k ^ self.pres.slides[cur].id as u64),
            None => self.pres.edit("slides.undo.transition"),
        };
        let t = &mut deck.slide_mut(cur).transition;
        f(t);
        t.raw = None;
    }

    pub fn set_transition(&mut self, kind: TransKind) {
        self.edit_transition(None, |t| {
            t.kind = kind;
            if !kind.dirs().is_empty() && !kind.dirs().contains(&t.dir) {
                t.dir = kind.dirs()[0];
            }
            if t.dur <= 0.0 {
                t.dur = 0.7;
            }
        });
    }

    pub fn set_transition_dir(&mut self, dir: Dir) {
        self.edit_transition(None, |t| t.dir = dir);
    }

    pub fn set_transition_dur(&mut self, secs: f32) {
        self.edit_transition(Some(DUR_KEY), |t| t.dur = secs.clamp(0.01, 60.0));
    }

    /// How the slide advances: on a click, and/or after some seconds.
    pub fn set_advance(&mut self, on_click: bool, after: Option<f32>) {
        self.edit_transition(Some(DUR_KEY ^ 1), |t| {
            t.on_click = on_click;
            t.after = after.map(|a| a.max(0.0));
        });
    }

    /// This slide's transition on every slide.
    pub fn transition_to_all(&mut self) {
        if self.read_only || self.pres.slides.is_empty() {
            return;
        }
        let t = self.pres.slides[self.cur].transition.clone();
        let deck = self.pres.edit("slides.undo.transition");
        for i in 0..deck.slides.len() {
            deck.slide_mut(i).transition = apex_slides::Transition { raw: None, ..t.clone() };
        }
    }

    // --- animations ---------------------------------------------------------------------

    fn edit_timing(&mut self, key: Option<u64>, f: impl FnOnce(&mut apex_slides::Timing)) {
        if self.read_only || self.pres.slides.is_empty() {
            return;
        }
        let cur = self.cur;
        let deck = match key {
            Some(k) => self.pres.edit_merge("slides.undo.animation", k ^ ((self.pres.slides[cur].id as u64) << 8)),
            None => self.pres.edit("slides.undo.animation"),
        };
        let t = &mut deck.slide_mut(cur).timing;
        f(t);
        t.raw = None;
    }

    /// The animations of the current slide.
    pub fn animations(&self) -> &[Anim] {
        self.pres.slides.get(self.cur).map_or(&[], |s| &s.timing.anims)
    }

    /// Adds an animation to each selected object; it becomes the one picked.
    pub fn add_animation(&mut self, class: Class, effect: Effect) {
        let ids = self.selection();
        if ids.is_empty() {
            return;
        }
        self.edit_timing(None, |t| {
            for id in ids {
                t.anims.push(Anim::new(id, class, effect));
            }
        });
        self.anim_sel = self.animations().len().checked_sub(1);
    }

    /// Changes the picked animation.
    pub fn change_animation(&mut self, key: Option<u64>, f: impl FnOnce(&mut Anim)) {
        let Some(i) = self.anim_sel.filter(|&i| i < self.animations().len()) else { return };
        self.edit_timing(key.map(|k| k ^ i as u64), |t| {
            let a = &mut t.anims[i];
            f(a);
            a.raw = None;
        });
    }

    /// Another effect for the picked animation (its preset follows).
    pub fn set_effect(&mut self, class: Class, effect: Effect) {
        self.change_animation(None, |a| {
            a.class = class;
            a.effect = effect;
            a.preset = effect.preset(class);
            a.subtype = 0;
            if effect == Effect::Appear {
                a.dur = 0.0;
            } else if a.dur <= 0.0 {
                a.dur = 0.5;
            }
        });
    }

    /// An effect from the gallery: it replaces the picked animation of
    /// the selected object, else it is added (as PowerPoint does).
    pub fn pick_effect(&mut self, class: Class, effect: Effect) {
        let sel = self.selection();
        match self.anim_sel.and_then(|i| self.animations().get(i)) {
            Some(a) if sel.len() == 1 && sel[0] == a.shape => self.set_effect(class, effect),
            _ => self.add_animation(class, effect),
        }
    }

    pub fn remove_animation(&mut self) {
        let Some(i) = self.anim_sel.filter(|&i| i < self.animations().len()) else { return };
        self.edit_timing(None, |t| {
            t.anims.remove(i);
        });
        self.anim_sel = if self.animations().is_empty() { None } else { Some(i.min(self.animations().len() - 1)) };
    }

    /// What the picked animation can do: move earlier, later, be removed.
    fn anim_moves(&self) -> (bool, bool, bool) {
        let n = self.animations().len();
        match self.anim_sel.filter(|&i| i < n) {
            Some(i) => (i > 0, i + 1 < n, true),
            None => (false, false, false),
        }
    }

    /// Moves the picked animation earlier (`up`) or later.
    pub fn move_animation(&mut self, up: bool) {
        let n = self.animations().len();
        let Some(i) = self.anim_sel.filter(|&i| i < n) else { return };
        let j = if up { i.checked_sub(1) } else { Some(i + 1).filter(|&j| j < n) };
        let Some(j) = j else { return };
        self.edit_timing(None, |t| t.anims.swap(i, j));
        self.anim_sel = Some(j);
    }

    /// Plays the current slide's show from it (Preview).
    pub fn preview(&mut self, ctx: &egui::Context, fullscreen: bool) {
        let cur = self.cur;
        self.start_show(ctx, cur, false, fullscreen);
    }

    // --- tabs ---------------------------------------------------------------------------

    pub(super) fn transitions_tab(&mut self, b: &mut Body, shell: &mut Shell) {
        let acc = b.accent();
        let Some(t) = self.pres.slides.get(self.cur).map(|s| s.transition.clone()) else { return };
        let full = shell.fullscreen;
        b.group("slides.g.preview", Icon::SpeakPlay, |ui| {
            if toolstrip::large(ui, Icon::SpeakPlay, tr("slides.preview"), tr("slides.preview_tip"), acc, false).clicked() {
                self.preview(ui.ctx(), full);
            }
        });
        b.group("slides.g.transition", Icon::Slides, |ui| {
            ui.add_enabled_ui(!self.read_only, |ui| {
                for kind in TRANSITIONS_SHOWN {
                    if transition_tile(ui, kind, t.kind == kind).clicked() {
                        self.set_transition(kind);
                    }
                }
                toolstrip::large_menu(ui, Icon::Slides, tr("slides.more_transitions"), tr("slides.more_transitions_tip"), acc, |ui| {
                    for row in TransKind::ALL.chunks(6) {
                        ui.horizontal(|ui| {
                            for &kind in row {
                                if transition_tile(ui, kind, t.kind == kind).clicked() {
                                    self.set_transition(kind);
                                    ui.close();
                                }
                            }
                        });
                    }
                });
                toolstrip::large_menu(ui, Icon::Options, tr("slides.effect_options"), tr("slides.effect_options_tip"), acc, |ui| {
                    for &d in t.kind.dirs() {
                        if ui.add(egui::Button::selectable(t.dir == d, tr(d.key()))).clicked() {
                            self.set_transition_dir(d);
                            ui.close();
                        }
                    }
                    if t.kind.dirs().is_empty() {
                        ui.label(tr("slides.no_options"));
                    }
                });
            });
        });
        b.group("slides.g.timing", Icon::DateTime, |ui| {
            ui.add_enabled_ui(!self.read_only, |ui| {
                toolstrip::column(ui, |ui| {
                    let mut d = t.dur;
                    if toolstrip::number(ui, Icon::DateTime, tr("slides.duration"), &mut d, 0.01..=60.0, " s") {
                        self.set_transition_dur(d);
                    }
                    if toolstrip::small(ui, Some(Icon::Copy), tr("slides.apply_all"), tr("slides.apply_all_tip"), acc, false).clicked() {
                        self.transition_to_all();
                    }
                });
                toolstrip::column(ui, |ui| {
                    if toolstrip::check(ui, t.on_click, Icon::Select, tr("slides.on_click")) {
                        self.set_advance(!t.on_click, t.after);
                    }
                    toolstrip::row(ui, |ui| {
                        let on = t.after.is_some();
                        if toolstrip::check(ui, on, Icon::SpeakNext, tr("slides.after")) {
                            self.set_advance(t.on_click, (!on).then_some(t.after.unwrap_or(5.0)));
                        }
                        let mut a = t.after.unwrap_or(0.0);
                        if ui.add_enabled_ui(on, |ui| toolstrip::number_field(ui, &mut a, 0.0..=3600.0, " s")).inner.changed() {
                            self.set_advance(t.on_click, Some(a));
                        }
                    });
                });
            });
        });
    }

    pub(super) fn animations_tab(&mut self, b: &mut Body, shell: &mut Shell) {
        let acc = b.accent();
        let full = shell.fullscreen;
        let some = !self.selection().is_empty() && !self.read_only;
        b.group("slides.g.preview", Icon::SpeakPlay, |ui| {
            if toolstrip::large(ui, Icon::SpeakPlay, tr("slides.preview"), tr("slides.preview_tip"), acc, false).clicked() {
                self.preview(ui.ctx(), full);
            }
        });
        b.group("slides.g.animation", Icon::Brightness, |ui| {
            ui.add_enabled_ui(some, |ui| {
                for e in &ENTRANCE[..6] {
                    let e = *e;
                    if effect_tile(ui, Class::Entrance, e).clicked() {
                        self.pick_effect(Class::Entrance, e);
                    }
                }
                toolstrip::large_menu(ui, Icon::Brightness, tr("slides.more_effects"), tr("slides.more_effects_tip"), acc, |ui| {
                    for (class, key, list) in [(Class::Entrance, "slides.entrance", &ENTRANCE[..]), (Class::Emphasis, "slides.emphasis", &EMPHASIS[..]), (Class::Exit, "slides.exit", &ENTRANCE[..])] {
                        ui.label(RichText::new(tr(key)).strong());
                        ui.horizontal(|ui| {
                            for &e in list {
                                if effect_tile(ui, class, e).clicked() {
                                    self.pick_effect(class, e);
                                    ui.close();
                                }
                            }
                        });
                        ui.add_space(4.0);
                    }
                });
            });
        });
        let picked = self.anim_sel.and_then(|i| self.animations().get(i).cloned());
        b.group("slides.g.anim_timing", Icon::DateTime, |ui| {
            ui.add_enabled_ui(picked.is_some() && !self.read_only, |ui| {
                let a = picked.clone().unwrap_or_else(|| Anim::new(0, Class::Entrance, Effect::Fade));
                // Off: no animation picked (or a file open read-only).
                let ro = self.read_only;
                let why = if picked.is_none() { tr("slides.pick_anim") } else { tr("lock.blocked") };
                toolstrip::column(ui, |ui| {
                    let start = format!("{}: {}", tr("slides.start"), tr(a.trigger.key()));
                    toolstrip::small_menu(ui, Some(Icon::SpeakPlay), &start, tr("slides.start"), acc, |ui| {
                        for tg in [Trigger::OnClick, Trigger::WithPrev, Trigger::AfterPrev] {
                            if ui.selectable_label(a.trigger == tg, tr(tg.key())).clicked() {
                                self.change_animation(None, |x| x.trigger = tg);
                                ui.close();
                            }
                        }
                    })
                    .on_disabled_hover_text(why);
                    let mut d = a.dur;
                    if toolstrip::number(ui, Icon::DateTime, tr("slides.duration"), &mut d, 0.0..=60.0, " s") {
                        self.change_animation(Some(DUR_KEY ^ 2), |x| x.dur = d);
                    }
                    let mut d = a.delay;
                    if toolstrip::number(ui, Icon::SpeakPause, tr("slides.delay"), &mut d, 0.0..=60.0, " s") {
                        self.change_animation(Some(DUR_KEY ^ 3), |x| x.delay = d);
                    }
                });
                toolstrip::column(ui, |ui| {
                    let dirs = [Dir::Left, Dir::Right, Dir::Up, Dir::Down];
                    ui.add_enabled_ui(matches!(a.effect, Effect::Fly | Effect::Wipe), |ui| {
                        let r = toolstrip::small_menu(ui, Some(Icon::Options), tr("slides.effect_options"), tr("slides.effect_options_tip"), acc, |ui| {
                            for d in dirs {
                                if ui.add(egui::Button::selectable(a.dir == d, tr(d.key()))).clicked() {
                                    self.change_animation(None, |x| {
                                        x.dir = d;
                                        x.subtype = match d {
                                            Dir::Left => 8,
                                            Dir::Right => 2,
                                            Dir::Up => 1,
                                            _ => 4,
                                        };
                                    });
                                    ui.close();
                                }
                            }
                        });
                        r.on_disabled_hover_text(if picked.is_none() || ro { why } else { tr("slides.no_options") });
                    });
                    let (up, down, _) = self.anim_moves();
                    let first_or_last = |at_end: bool| if picked.is_none() || ro { why } else if at_end { tr("slides.anim_first") } else { tr("slides.anim_last") };
                    if ui.add_enabled_ui(up, |ui| toolstrip::small(ui, Some(Icon::ChevronUp), tr("slides.move_earlier"), tr("slides.move_earlier"), acc, false)).inner.on_disabled_hover_text(first_or_last(true)).clicked() {
                        self.move_animation(true);
                    }
                    if ui.add_enabled_ui(down, |ui| toolstrip::small(ui, Some(Icon::ChevronDown), tr("slides.move_later"), tr("slides.move_later"), acc, false)).inner.on_disabled_hover_text(first_or_last(false)).clicked() {
                        self.move_animation(false);
                    }
                });
                let remove = self.anim_moves().2;
                if ui.add_enabled_ui(remove, |ui| toolstrip::large(ui, Icon::Eraser, tr("slides.remove_anim"), tr("slides.remove_anim"), acc, false)).inner.on_disabled_hover_text(why).clicked() {
                    self.remove_animation();
                }
            });
        });
        b.group("slides.g.anim_pane", Icon::Outline, |ui| {
            if toolstrip::large(ui, Icon::Outline, tr("slides.anim_pane"), tr("slides.anim_pane_tip"), acc, self.anim_pane).clicked() {
                self.anim_pane = !self.anim_pane;
            }
        });
    }

    /// The list of the slide's animations, to pick, order and remove them.
    pub(super) fn anim_pane_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new(tr("slides.anim_pane")).strong());
        ui.separator();
        let names: Vec<(String, u32)> = self
            .animations()
            .iter()
            .map(|a| {
                let who = self.cur_objects().iter().find(|o| o.id == a.shape).map(|o| if o.name.is_empty() { format!("#{}", o.id) } else { o.name.clone() }).unwrap_or_default();
                let class = match a.class {
                    Class::Entrance => "slides.entrance",
                    Class::Exit => "slides.exit",
                    Class::Media => "slides.media.media",
                    _ => "slides.emphasis",
                };
                let effect = if a.class == Class::Media { "slides.media.play" } else { a.effect.key() };
                let mark = match a.trigger {
                    Trigger::OnClick => "🖱",
                    Trigger::WithPrev => " ",
                    Trigger::AfterPrev => "⏱",
                };
                (format!("{mark} {} · {} ({})", who, tr(effect), tr(class)), a.shape)
            })
            .collect();
        if names.is_empty() {
            ui.weak(tr("slides.no_anims"));
        }
        let mut click = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let mut n = 0;
            for (i, (name, id)) in names.iter().enumerate() {
                if self.animations()[i].trigger == Trigger::OnClick {
                    n += 1;
                }
                let r = ui.add(egui::Button::selectable(self.anim_sel == Some(i), format!("{n}  {name}")));
                if r.clicked() {
                    click = Some((i, *id));
                }
            }
        });
        if let Some((i, id)) = click {
            self.anim_sel = Some(i);
            self.sel = vec![id];
        }
        let (up, down, remove) = self.anim_moves();
        ui.horizontal(|ui| {
            if ui.add_enabled(up, egui::Button::new("▲")).on_hover_text(tr("slides.move_earlier")).clicked() {
                self.move_animation(true);
            }
            if ui.add_enabled(down, egui::Button::new("▼")).on_hover_text(tr("slides.move_later")).clicked() {
                self.move_animation(false);
            }
            if ui.add_enabled(remove, egui::Button::new(tr("slides.remove_anim"))).clicked() {
                self.remove_animation();
            }
        });
    }

    pub(super) fn show_tab(&mut self, b: &mut Body, shell: &mut Shell) {
        let acc = b.accent();
        let full = shell.fullscreen;
        b.group("slides.g.start_show", Icon::SpeakPlay, |ui| {
            if toolstrip::large(ui, Icon::SpeakPlay, tr("slides.from_start"), &format!("{} (F5)", tr("slides.from_start")), acc, false).clicked() {
                self.start_show(ui.ctx(), 0, false, full);
            }
            if toolstrip::large(ui, Icon::Slides, tr("slides.from_current"), &format!("{} (Shift+F5)", tr("slides.from_current")), acc, false).clicked() {
                let cur = self.cur;
                self.start_show(ui.ctx(), cur, false, full);
            }
            if toolstrip::large(ui, Icon::SideBySide, tr("slides.presenter"), &format!("{} (Alt+F5)", tr("slides.presenter_tip")), acc, false).clicked() {
                let cur = self.cur;
                self.start_show(ui.ctx(), cur, true, full);
            }
        });
        b.group("slides.g.pointer", Icon::Pen, |ui| {
            toolstrip::large_menu(ui, Icon::Pen, tr("slides.pointer_options"), tr("slides.pointer_options_tip"), acc, |ui| {
                // No keys here: as in PowerPoint, Ctrl+A/L/P/E pick the pointer
                // only during the show (the tip says so); while editing they
                // are Select All, Print...
                let tools = [
                    (show_ink::Pointer::Arrow, Icon::Select, "slides.pointer_arrow"),
                    (show_ink::Pointer::Laser, Icon::Highlight, "slides.pointer_laser"),
                    (show_ink::Pointer::Pen, Icon::Pen, "slides.pointer_pen"),
                    (show_ink::Pointer::Eraser, Icon::Eraser, "slides.pointer_eraser"),
                ];
                for (tool, icon, key) in tools {
                    let label = tr(key);
                    if toolstrip::menu_item_on(ui, icon, label, "", self.pointer == tool).clicked() {
                        self.pointer = tool;
                        ui.close();
                    }
                }
            });
        });
        b.group("slides.g.setup", Icon::Options, |ui| {
            ui.add_enabled_ui(!self.read_only, |ui| {
                toolstrip::column(ui, |ui| {
                    let s = self.pres.show.clone();
                    if toolstrip::small(ui, Some(Icon::Redo), tr("slides.loop"), tr("slides.loop_tip"), acc, s.looped).clicked() {
                        self.set_show(|x| x.looped = !s.looped);
                    }
                    if toolstrip::check(ui, s.use_timings, Icon::DateTime, tr("slides.use_timings")) {
                        self.set_show(|x| x.use_timings = !s.use_timings);
                    }
                    if toolstrip::small(ui, Some(Icon::Fullscreen), tr("slides.kiosk"), tr("slides.kiosk_tip"), acc, s.kiosk).clicked() {
                        self.set_show(|x| x.kiosk = !s.kiosk);
                    }
                });
            });
        });
    }

    /// Changes how the show runs.
    pub fn set_show(&mut self, f: impl FnOnce(&mut apex_slides::ShowSettings)) {
        if self.read_only {
            return;
        }
        f(&mut self.pres.edit("slides.undo.show").show);
    }
}

/// The presenter view: current slide, next slide, notes, times, controls.
fn presenter_ui(ui: &mut egui::Ui, deck: &apex_slides::Deck, s: &mut Show, now: f64) {
    let (_, _, _, h, mi) = common::local_now();
    let n = s.order.len();
    ui.horizontal(|ui| {
        ui.label(RichText::new(clock(now - s.start)).size(22.0).color(Color32::WHITE)).on_hover_text(tr("slides.elapsed"));
        ui.add_space(24.0);
        ui.label(RichText::new(format!("{h:02}:{mi:02}")).size(22.0).color(Color32::from_gray(200))).on_hover_text(tr("slides.clock"));
        ui.add_space(24.0);
        let pos = if s.slide().is_some() { trf("slides.slide_of", &[("n", &(s.at + 1)), ("total", &n)]) } else { tr("slides.show_end").to_string() };
        ui.label(RichText::new(pos).size(16.0).color(Color32::from_gray(220)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(tr("slides.end_show")).clicked() {
                s.done = true;
            }
            let black = s.blank == Some(Color32::BLACK);
            if ui.add(egui::Button::new(tr("slides.black")).selected(black)).on_hover_text("B").clicked() {
                s.blank = if black { None } else { Some(Color32::BLACK) };
            }
            if ui.button("▶").on_hover_text(tr("slides.next")).clicked() {
                s.next(deck, now);
            }
            if ui.add_enabled(s.can_prev(deck), egui::Button::new("◀")).on_hover_text(tr("slides.previous")).clicked() {
                s.prev(deck, now);
            }
        });
    });
    ui.separator();
    let avail = ui.available_rect_before_wrap();
    let left = Rect::from_min_size(avail.min, vec2(avail.width() * 0.62, avail.height()));
    let right = Rect::from_min_max(egui::pos2(left.max.x + 16.0, avail.min.y), avail.max);
    let resp = ui.allocate_rect(avail, Sense::click());
    // The keys and clicks drive the show here too.
    let p = ui.painter();
    let fit = |r: Rect| {
        let k = (r.width() / deck.width.max(1.0)).min(r.height() / deck.height.max(1.0));
        Rect::from_min_size(r.min, vec2(deck.width, deck.height) * k)
    };
    show::paint_show(p, deck, s, fit(left), now, tr("slides.show_end"));
    let next_r = fit(Rect::from_min_size(right.min + vec2(0.0, 20.0), vec2(right.width(), right.height() * 0.4)));
    p.text(right.min, egui::Align2::LEFT_TOP, tr("slides.next"), egui::FontId::proportional(13.0), Color32::from_gray(180));
    match s.next_slide() {
        Some(i) => render::paint_slide(p, deck, &deck.slides[i], next_r, render::Opts::default()),
        None => {
            p.rect_filled(next_r, 0.0, Color32::BLACK);
        }
    }
    p.rect_stroke(next_r, 0.0, Stroke::new(1.0, Color32::from_gray(90)), egui::StrokeKind::Outside);
    let notes = s.slide().map(|i| deck.slides[i].notes_text()).unwrap_or_default();
    let notes_r = Rect::from_min_max(egui::pos2(right.min.x, next_r.max.y + 16.0), right.max);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(notes_r).id_salt(Id::new("presenter_notes")));
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(&mut child, |ui| {
        ui.label(RichText::new(if notes.is_empty() { tr("slides.no_notes").to_string() } else { notes }).size(18.0).color(Color32::WHITE));
    });
    s.input(ui.ctx(), &resp, deck);
    // The clocks tick.
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
}

/// A transition in the gallery: a small picture of its motion.
fn transition_tile(ui: &mut egui::Ui, kind: TransKind, on: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(58.0, 62.0), Sense::click());
    crate::widgets::name(&resp, egui::WidgetType::Button, tr(kind.key()), Some(on));
    let p = ui.painter();
    let v = ui.visuals();
    if on || resp.hovered() {
        p.rect_filled(rect, 3.0, if on { v.selection.bg_fill } else { v.widgets.hovered.weak_bg_fill });
    }
    let r = Rect::from_center_size(rect.center_top() + vec2(0.0, 20.0), vec2(38.0, 26.0));
    let old = |p: &egui::Painter, r: Rect| {
        p.rect_filled(r, 1.0, Color32::from_rgb(0x2F, 0x55, 0x97));
    };
    let new = |p: &egui::Painter, r: Rect| {
        p.rect_filled(r, 1.0, theme::SLIDES_ACCENT);
    };
    let dir = kind.dirs().first().copied().unwrap_or(Dir::Left);
    // Hovered, it plays; else a moment that shows it (a flip is edge on
    // half way).
    let k = if resp.hovered() {
        ui.ctx().request_repaint();
        let t = ui.input(|i| i.time) as f32 % 1.6;
        (t / 1.2).min(1.0)
    } else if matches!(kind, TransKind::Flip | TransKind::Switch) {
        0.3
    } else {
        0.5
    };
    show::paint_transition(&p.with_clip_rect(r.expand(3.0).intersect(p.clip_rect())), r, if kind == TransKind::Morph { TransKind::Fade } else { kind }, dir, k, &old, &new);
    p.rect_stroke(r, 1.0, Stroke::new(1.0, Color32::from_gray(140)), egui::StrokeKind::Outside);
    tile_label(ui, rect, tr(kind.key()));
    resp.on_hover_text(tr(kind.key()))
}

/// An animation effect in the gallery: its picture, colored by its class;
/// hovered, it plays on a small box.
fn effect_tile(ui: &mut egui::Ui, class: Class, e: Effect) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(56.0, 62.0), Sense::click());
    crate::widgets::name(&resp, egui::WidgetType::Button, tr(e.key()), None);
    let p = ui.painter();
    let v = ui.visuals();
    if resp.hovered() {
        p.rect_filled(rect, 3.0, v.widgets.hovered.weak_bg_fill);
    }
    let color = match class {
        Class::Entrance => Color32::from_rgb(0x4C, 0xAF, 0x50),
        Class::Exit => Color32::from_rgb(0xE5, 0x39, 0x35),
        _ => Color32::from_rgb(0xFB, 0xC0, 0x2D),
    };
    let area = Rect::from_center_size(rect.center_top() + vec2(0.0, 20.0), vec2(44.0, 32.0));
    if resp.hovered() {
        ui.ctx().request_repaint();
        let t = ui.input(|i| i.time) as f32 % 1.8;
        effect_play(&p.with_clip_rect(area.intersect(p.clip_rect())), area, class, e, (t / 1.4).min(1.0), color);
    } else {
        effect_icon(p, area, class, e, color);
    }
    tile_label(ui, rect, tr(e.key()));
    resp.on_hover_text(tr(e.key()))
}

/// An effect played at `t` (0..1) on a box in the middle of `area`.
fn effect_play(p: &egui::Painter, area: Rect, class: Class, e: Effect, t: f32, color: Color32) {
    let size = (area.width(), area.height());
    let f = apex_slides::Frame { x: size.0 / 2.0 - 9.0, y: size.1 / 2.0 - 7.0, w: 18.0, h: 14.0, ..Default::default() };
    let mut l = show::Look::default();
    let dir = if e == Effect::Fly { Dir::Down } else { Dir::Left };
    // Entrances start hidden.
    if class == Class::Entrance && t <= 0.0 {
        l.visible = false;
    }
    show::apply(&mut l, class, e, dir, None, t, f, size);
    if !l.visible || l.alpha <= 0.0 {
        return;
    }
    let c = area.center() + vec2(l.dx, l.dy);
    let (w, h) = (f.w * l.scale * l.sx, f.h * l.scale);
    let mut q = p.clone();
    if let Some((d, k)) = l.clip {
        let r = Rect::from_center_size(area.center(), vec2(f.w, f.h)).expand(1.0);
        let c = match d {
            Dir::VertOut => Rect::from_center_size(r.center(), vec2(r.width() * k, r.height())),
            _ => Rect::from_min_max(r.min, egui::pos2(r.min.x + r.width() * k, r.max.y)),
        };
        q = q.with_clip_rect(c.intersect(q.clip_rect()));
    }
    box_at(&q, c, vec2(w, h), l.rot.to_radians(), color.gamma_multiply(l.alpha.clamp(0.0, 1.0)));
}

/// A box of `size` centered at `c`, turned by `rot` radians.
fn box_at(p: &egui::Painter, c: egui::Pos2, size: egui::Vec2, rot: f32, fill: Color32) {
    let (s, co) = rot.sin_cos();
    let pts = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].map(|(x, y): (f32, f32)| {
        let (x, y) = (x * size.x / 2.0, y * size.y / 2.0);
        c + vec2(x * co - y * s, x * s + y * co)
    });
    p.add(egui::Shape::convex_polygon(pts.to_vec(), fill, Stroke::NONE));
}

/// The picture of an effect in `area`.
fn effect_icon(p: &egui::Painter, area: Rect, class: Class, e: Effect, color: Color32) {
    use std::f32::consts::PI;
    let c = area.center();
    let o = Rect::from_center_size(c, vec2(18.0, 14.0));
    let line = Stroke::new(1.5, color);
    let faint = |k: f32| color.gamma_multiply(k);
    let arc = |center: egui::Pos2, r: f32, a0: f32, a1: f32, st: Stroke| {
        let pts: Vec<egui::Pos2> = (0..=16).map(|i| center + r * egui::Vec2::angled(a0 + (a1 - a0) * i as f32 / 16.0)).collect();
        p.add(egui::Shape::line(pts, st));
    };
    // An arrow head at `tip` pointing along `dir`.
    let head = |tip: egui::Pos2, dir: egui::Vec2| {
        let d = dir.normalized() * 4.0;
        let n = vec2(-d.y, d.x);
        p.add(egui::Shape::convex_polygon(vec![tip, tip - d + n * 0.8, tip - d - n * 0.8], color, Stroke::NONE));
    };
    // Exits go away: the picture leans the other way where it moves.
    let out = class == Class::Exit;
    match e {
        Effect::Appear => {
            p.rect_filled(o, 2.0, color);
            for a in [-0.9f32, -0.3, 0.3] {
                let from = o.right_top() + vec2(2.0, -2.0);
                let v = egui::Vec2::angled(a - PI / 4.0);
                p.line_segment([from + v * 2.0, from + v * 6.0], line);
            }
        }
        Effect::Fade => {
            p.rect_filled(o.translate(vec2(-8.0, 0.0)), 2.0, faint(0.2));
            p.rect_filled(o.translate(vec2(-4.0, 0.0)), 2.0, faint(0.45));
            p.rect_filled(o.translate(vec2(4.0, 0.0)), 2.0, color);
        }
        Effect::Fly => {
            let b = o.translate(vec2(6.0, 0.0));
            p.rect_filled(b, 2.0, color);
            for (y, len) in [(-4.0, 8.0), (0.0, 12.0), (4.0, 8.0)] {
                let x = b.min.x - 3.0;
                p.line_segment([egui::pos2(x - len, c.y + y), egui::pos2(x, c.y + y)], Stroke::new(1.2, faint(0.7)));
            }
        }
        Effect::Float => {
            let dy = if out { -5.0 } else { 5.0 };
            p.rect_filled(o.translate(vec2(0.0, dy)), 2.0, faint(0.25));
            p.rect_filled(o.translate(vec2(0.0, -dy)), 2.0, color);
            let x = o.max.x + 6.0;
            p.line_segment([egui::pos2(x, c.y + 7.0), egui::pos2(x, c.y - 7.0)], line);
            head(egui::pos2(x, c.y - 8.0), vec2(0.0, -1.0));
        }
        Effect::Wipe => {
            p.rect_stroke(o, 2.0, Stroke::new(1.0, faint(0.6)), egui::StrokeKind::Inside);
            p.rect_filled(Rect::from_min_max(o.min, egui::pos2(o.min.x + o.width() * 0.6, o.max.y)), 2.0, color);
            let x = o.min.x + o.width() * 0.6 + 2.0;
            p.line_segment([egui::pos2(x, o.min.y - 3.0), egui::pos2(x, o.max.y + 3.0)], line);
        }
        Effect::Split => {
            p.rect_stroke(o, 2.0, Stroke::new(1.0, faint(0.6)), egui::StrokeKind::Inside);
            p.rect_filled(Rect::from_center_size(c, vec2(8.0, o.height())), 0.0, color);
            for s in [-1.0f32, 1.0] {
                let from = c + vec2(s * 11.0, 0.0);
                p.line_segment([from, from + vec2(s * 6.0, 0.0)], line);
                head(from + vec2(s * 8.0, 0.0), vec2(s, 0.0));
            }
        }
        Effect::Zoom => {
            p.rect_stroke(o.expand(4.0), 2.0, Stroke::new(1.0, faint(0.5)), egui::StrokeKind::Inside);
            p.rect_filled(o.shrink2(vec2(4.0, 3.0)), 2.0, color);
            for (a, b) in [(o.expand(4.0).left_top(), o.left_top()), (o.expand(4.0).right_bottom(), o.right_bottom())] {
                p.line_segment([a, b], Stroke::new(1.0, faint(0.7)));
            }
        }
        Effect::GrowTurn => {
            box_at(p, c + vec2(-6.0, 3.0), vec2(8.0, 6.0), -0.6, faint(0.35));
            box_at(p, c + vec2(3.0, -1.0), vec2(16.0, 12.0), 0.35, color);
        }
        Effect::Swivel => {
            p.rect_filled(Rect::from_center_size(c, vec2(7.0, 16.0)), 1.0, color);
            arc(c, 12.0, PI * 0.15, PI * 0.85, Stroke::new(1.3, faint(0.8)));
            head(c + 12.0 * egui::Vec2::angled(PI * 0.15), vec2(1.0, -1.0));
        }
        Effect::Bounce => {
            let b = Rect::from_center_size(c + vec2(9.0, 8.0), vec2(10.0, 8.0));
            p.rect_filled(b, 2.0, color);
            let pts: Vec<egui::Pos2> = (0..=20)
                .map(|i| {
                    let t = i as f32 / 20.0;
                    let x = c.x - 16.0 + t * 25.0;
                    let y = c.y + 4.0 - 14.0 * (1.0 - t) * (2.0 * PI * t).sin().abs();
                    egui::pos2(x, y)
                })
                .collect();
            p.add(egui::Shape::dashed_line(&pts, Stroke::new(1.2, faint(0.7)), 2.0, 2.0));
        }
        Effect::Spin => {
            p.rect_filled(o.shrink(2.0), 2.0, color);
            arc(c, 13.0, -PI * 0.9, PI * 0.4, line);
            head(c + 13.0 * egui::Vec2::angled(PI * 0.4), egui::Vec2::angled(PI * 0.9));
        }
        Effect::Grow => {
            p.rect_stroke(o.expand(5.0), 2.0, Stroke::new(1.0, faint(0.5)), egui::StrokeKind::Inside);
            p.rect_filled(o, 2.0, color);
            for corner in [o.expand(5.0).left_top(), o.expand(5.0).right_bottom()] {
                head(corner, corner - c);
            }
        }
        Effect::Pulse => {
            p.rect_filled(o, 2.0, color);
            p.rect_stroke(o.expand(3.0), 3.0, Stroke::new(1.2, faint(0.55)), egui::StrokeKind::Inside);
            p.rect_stroke(o.expand(6.5), 4.0, Stroke::new(1.0, faint(0.25)), egui::StrokeKind::Inside);
        }
        Effect::Teeter => {
            box_at(p, c, vec2(18.0, 14.0), -0.3, faint(0.3));
            box_at(p, c, vec2(18.0, 14.0), 0.3, color);
            arc(c + vec2(0.0, -2.0), 15.0, -PI * 0.7, -PI * 0.3, Stroke::new(1.2, faint(0.7)));
        }
        Effect::Blink => {
            p.rect_filled(o.translate(vec2(-7.0, 0.0)), 2.0, color);
            p.rect_stroke(o.translate(vec2(7.0, 0.0)), 2.0, Stroke::new(1.0, faint(0.6)), egui::StrokeKind::Inside);
        }
        Effect::Transparency => {
            let r = o.expand(3.0);
            for i in 0..4 {
                for j in 0..3 {
                    if (i + j) % 2 == 0 {
                        let s = vec2(r.width() / 4.0, r.height() / 3.0);
                        p.rect_filled(Rect::from_min_size(r.min + vec2(i as f32 * s.x, j as f32 * s.y), s), 0.0, Color32::from_gray(150).gamma_multiply(0.5));
                    }
                }
            }
            p.rect_filled(o.translate(vec2(4.0, 3.0)), 2.0, faint(0.55));
        }
        Effect::Other => {
            p.text(c, egui::Align2::CENTER_CENTER, "★", egui::FontId::proportional(24.0), color);
        }
    }
}

/// The name at the foot of a gallery tile: on two lines when it is wider
/// than the tile, smaller when that isn't enough; a word is never broken
/// ("Desvanecer" doesn't become "Desvanece / r").
fn tile_label(ui: &egui::Ui, rect: Rect, text: &str) {
    let color = ui.visuals().text_color();
    let width = rect.width() - 2.0;
    let lay = |size: f32| {
        let mut job = egui::text::LayoutJob::simple(text.to_string(), egui::FontId::proportional(size), color, width);
        job.halign = egui::Align::Center;
        job.wrap.max_rows = 2;
        ui.painter().layout_job(job)
    };
    let fits = |size: f32| {
        let words = text.split_whitespace().all(|w| ui.painter().layout_no_wrap(w.to_string(), egui::FontId::proportional(size), color).size().x <= width);
        words && lay(size).rows.len() <= 2 && !lay(size).elided
    };
    // One word too long even small: broken in two with a hyphen
    // ("Transfor-mación"), near its middle.
    let broken = || {
        let chars: Vec<(usize, char)> = text.char_indices().collect();
        if text.contains(char::is_whitespace) || chars.len() < 6 {
            return None;
        }
        let mid = chars.len() / 2;
        let font = egui::FontId::proportional(9.0);
        (0..mid - 2).flat_map(|d| [mid - d, mid + d]).find_map(|i| {
            let at = chars.get(i)?.0;
            let (a, b) = (format!("{}-", &text[..at]), &text[at..]);
            let w = |t: &str| ui.painter().layout_no_wrap(t.to_string(), font.clone(), color).size().x;
            (w(&a) <= width && w(b) <= width).then(|| {
                let mut job = egui::text::LayoutJob::simple(format!("{a}\n{b}"), font.clone(), color, width);
                job.halign = egui::Align::Center;
                ui.painter().layout_job(job)
            })
        })
    };
    let g = match [11.0, 10.0, 9.0].into_iter().find(|&s| fits(s)) {
        Some(size) => lay(size),
        // Still too long: cut short with "…" (the tooltip has it).
        None => broken().unwrap_or_else(|| {
            let mut job = egui::text::LayoutJob::simple(text.to_string(), egui::FontId::proportional(9.0), color, width);
            job.halign = egui::Align::Center;
            job.wrap = egui::text::TextWrapping { max_rows: 1, overflow_character: Some('…'), ..job.wrap };
            ui.painter().layout_job(job)
        }),
    };
    ui.painter().galley(egui::pos2(rect.center().x, rect.max.y - 3.0 - g.size().y), g, color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_slides::Frame;

    #[test]
    fn clocks() {
        assert_eq!(clock(0.0), "0:00:00");
        assert_eq!(clock(3725.4), "1:02:05");
    }

    #[test]
    fn transitions_edit_and_undo() {
        let mut v = SlidesView::new(apex_slides::build::new_deck(0));
        v.new_slide();
        v.set_transition(TransKind::Split);
        assert_eq!(v.pres.slides[1].transition.dir, Dir::VertOut);
        v.set_transition(TransKind::Push);
        v.set_transition_dir(Dir::Up);
        v.set_transition_dur(1.5);
        v.set_transition_dur(2.0);
        v.set_advance(false, Some(3.0));
        v.transition_to_all();
        let t = &v.pres.slides[0].transition;
        assert_eq!((t.kind, t.dir, t.dur, t.on_click, t.after), (TransKind::Push, Dir::Up, 2.0, false, Some(3.0)));
        v.undo();
        assert_eq!(v.pres.slides[0].transition.kind, TransKind::None);
        v.undo();
        v.undo();
        assert_eq!(v.pres.slides[1].transition.dur, 0.7);
    }

    #[test]
    fn animations_edit_and_undo() {
        let mut v = SlidesView::new(apex_slides::build::new_deck(0));
        let a = v.insert_shape("rect", Frame::new(10.0, 10.0, 50.0, 50.0), false).unwrap();
        let b = v.insert_shape("ellipse", Frame::new(100.0, 10.0, 50.0, 50.0), false).unwrap();
        v.sel = vec![a];
        v.add_animation(Class::Entrance, Effect::Fly);
        v.sel = vec![b];
        v.add_animation(Class::Entrance, Effect::Fade);
        assert_eq!(v.anim_sel, Some(1));
        v.change_animation(None, |x| x.trigger = Trigger::AfterPrev);
        v.pick_effect(Class::Emphasis, Effect::Spin);
        assert_eq!(v.animations().len(), 2);
        v.move_animation(true);
        assert_eq!(v.animations()[0].shape, b);
        assert_eq!(v.animations()[0].effect, Effect::Spin);
        v.remove_animation();
        assert_eq!(v.animations().len(), 1);
        v.undo();
        assert_eq!(v.animations().len(), 2);
        v.set_show(|s| s.looped = true);
        assert!(v.pres.show.looped && v.pres.show.changed());
        // Turned off again, the show is as it was.
        v.set_show(|s| s.looped = false);
        assert!(!v.pres.show.changed());
        assert_eq!(v.pres.show, apex_slides::ShowSettings::default());
    }
}
