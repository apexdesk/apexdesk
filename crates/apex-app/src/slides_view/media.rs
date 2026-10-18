//! Videos and sounds on slides: inserting them, the Playback tab (play,
//! when they start in the show, looping, volume, trimming) and playing
//! them on the canvas and in the show, drawn over their poster picture.
//! Decoding is [`crate::media`]'s; without FFmpeg they keep their poster.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use apex_slides::anim::Trigger;
use apex_slides::object::{Media, Picture};
use apex_slides::{Chain, Deck, Geometry, ObjKind, Object};
use eframe::egui::{self, Color32, Painter, Rect, Stroke, pos2, vec2};

use super::render::Xf;
use super::show::{self, Show};
use super::{SlidesView, objects::Target};
use crate::app::Notice;
use crate::i18n::{tr, trf};
use crate::icons::Icon;
use crate::media::{self, Player, decode};
use crate::toolstrip::{self, Body, TabDef};

/// Color of the Playback tab.
pub const MEDIA_TAB: Color32 = Color32::from_rgb(0xB0, 0x5A, 0xC8);

/// Pixels of the pictures played, on their longer side.
const PLAY_PX: usize = 1280;
/// Videos and sounds playing at once in the show: each holds a decoder,
/// a thread and its frames.
const MAX_PLAYERS: usize = 8;

/// When a video or sound plays in the show.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Start {
    /// When the slide shows (after what comes before).
    Auto,
    /// At its turn among the slide's clicks.
    ClickSequence,
    /// When it's clicked.
    Clicked,
}

pub const STARTS: [(Start, &str); 3] = [(Start::Auto, "slides.media.start_auto"), (Start::ClickSequence, "slides.media.start_click"), (Start::Clicked, "slides.media.start_clicked")];

/// Volume steps of the Volume menu (PowerPoint's).
pub const VOLUMES: [(f32, &str); 4] = [(0.25, "slides.media.vol_low"), (0.5, "slides.media.vol_medium"), (1.0, "slides.media.vol_high"), (0.0, "slides.media.vol_mute")];

/// The trim dialog: the object, the amounts cut (seconds) and the length.
#[derive(Clone, Debug)]
pub struct TrimDlg {
    pub id: u32,
    pub trim: (f32, f32),
    pub duration: f64,
}

/// What plays.
#[derive(Default)]
pub struct Playing {
    /// On the canvas: by slide id and object id.
    players: HashMap<(u32, u32), Player>,
    /// In the show: the slide they're on and its players.
    show_slide: Option<usize>,
    show_players: HashMap<u32, Player>,
    /// Started on this slide of the show (not again).
    show_started: HashSet<u32>,
    pub trim: Option<TrimDlg>,
    /// Asking before playing a media linked on another computer: the
    /// slide id, the object id and the media.
    network: Option<(u32, u32, Media)>,
    /// The notice that FFmpeg is missing was given.
    warned: bool,
}

/// The media of a picture object.
fn media_of(o: &Object) -> Option<&Media> {
    match &o.kind {
        ObjKind::Picture(p) => p.media.as_ref(),
        _ => None,
    }
}

/// A plain PNG of `w` by `h` pixels, of color `rgb`, for posters.
fn plain_png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    if let Ok(mut wr) = enc.write_header() {
        let _ = wr.write_image_data(&rgb.repeat((w * h) as usize));
    }
    out
}

impl SlidesView {
    /// Insert > Video or Audio: asks for a file and puts it on the slide.
    pub fn pick_media(&mut self, video: bool) {
        let (title, files, exts) = if video { ("slides.media.video", "slides.media.files_video", &media::VIDEO_EXTS) } else { ("slides.media.audio", "slides.media.files_audio", &media::AUDIO_EXTS) };
        crate::file_dialog::FileDialog::new().set_title(tr(title)).add_filter(tr(files), exts).pick_file(crate::file_dialog::in_view(|v: &mut SlidesView, path: std::path::PathBuf| v.insert_media_file(&path)));
    }

    /// Puts the video or sound file `path` on the slide (a notice if not).
    pub fn insert_media_file(&mut self, path: &std::path::Path) {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        match apex_fs::read(path) {
            Ok(data) => {
                if let Err(e) = self.insert_media(data, &name)
                    && !e.is_empty()
                {
                    self.notices.push(Notice::error(e));
                }
            }
            Err(e) => self.notices.push(Notice::error(e.to_string())),
        }
    }

    /// Puts the video or sound `data` (file `name`) on the slide, in the
    /// presentation, with a poster picture: the video's first, or a plain
    /// one. Its id.
    pub fn insert_media(&mut self, data: Vec<u8>, name: &str) -> Result<u32, String> {
        if self.read_only || self.mode == super::Mode::Master {
            return Err(String::new());
        }
        let bad = || trf("slides.media.bad_file", &[("file", &name)]);
        let (mime, video) = media::mime_of(name).ok_or_else(bad)?;
        let data: Arc<[u8]> = Arc::from(data);
        let src = decode::Source::Bytes(data.clone());
        let mut poster = None;
        let mut video = video;
        if decode::available() {
            match decode::probe(&src) {
                Ok(info) => {
                    video = info.size.is_some();
                    if video {
                        poster = media::player::poster_png(&src, 0.0);
                    }
                }
                // A format the system can't decode (a codec missing): it
                // goes in all the same, with a plain poster.
                Err(e) => {
                    crate::diag::warning(&format!("{name}: {e}"));
                    self.notices.push(Notice::info(trf("slides.media.no_codec", &[("file", &name)])));
                }
            }
        } else {
            self.warn_no_media();
        }
        let (png, w, h) = match poster {
            Some((png, w, h)) => (png, w as f32 * 0.75, h as f32 * 0.75),
            None if video => (plain_png(16, 9, [0x20, 0x20, 0x20]), 480.0, 270.0),
            None => (plain_png(8, 8, [0xE8, 0xE8, 0xE8]), 48.0, 48.0),
        };
        let k = (self.pres.width * 0.6 / w).min(self.pres.height * 0.6 / h).min(1.0);
        let mut frame = self.centered(w * k, h * k);
        if !video {
            // Sounds go to the bottom right, as PowerPoint puts them.
            frame.x = self.pres.width - frame.w - 24.0;
            frame.y = self.pres.height - frame.h - 24.0;
        }
        let image = apex_doc::Image::new(Arc::from(png), frame.w, frame.h);
        let m = Media::embedded(data, &if video { mime.to_string() } else { mime.replace("video/", "audio/") }, video);
        let stem = if video { "Video" } else { "Audio" };
        let mut o = Object::new(0, format!("{stem} 0"), ObjKind::Picture(Box::new(Picture { image, media: Some(m) })));
        o.sp.frame = Some(frame);
        o.sp.geom = Some(Geometry::preset("rect"));
        self.insert_object("slides.undo.insert_media", o).ok_or_else(String::new)
    }

    /// The selected video or sound on the slide: its id and media.
    pub fn selected_media(&self) -> Option<(u32, &Media)> {
        if !matches!(self.target(), Target::Slide(_)) {
            return None;
        }
        let o = self.selected_object()?;
        media_of(o).map(|m| (o.id, m))
    }

    /// Changes the selected media (typing in a field joins one step).
    pub fn edit_media(&mut self, key: Option<u64>, f: impl FnOnce(&mut Media)) {
        let Some((id, _)) = self.selected_media() else { return };
        if self.read_only {
            return;
        }
        let cur = self.cur;
        let deck = match key {
            Some(k) => self.pres.edit_merge("slides.undo.media", k ^ ((id as u64) << 8)),
            None => self.pres.edit("slides.undo.media"),
        };
        if let Some(o) = deck.object_mut(cur, id) {
            if let ObjKind::Picture(p) = &mut o.kind
                && let Some(m) = &mut p.media
            {
                f(m);
            }
            o.touch();
        }
        // What plays follows.
        if let Some(sid) = self.pres.slides.get(cur).map(|s| s.id)
            && let Some(p) = self.media.players.get(&(sid, id))
            && let Some((_, m)) = self.selected_media()
        {
            p.set_volume(if m.mute { 0.0 } else { m.volume });
        }
    }

    /// When the selected media plays in the show.
    pub fn media_start(&self) -> Option<Start> {
        let (id, _) = self.selected_media()?;
        Some(match self.pres.slides[self.cur].timing.media_start(id) {
            None => Start::Clicked,
            Some(Trigger::OnClick) => Start::ClickSequence,
            Some(_) => Start::Auto,
        })
    }

    pub fn set_media_start(&mut self, start: Start) {
        let Some((id, _)) = self.selected_media() else { return };
        if self.read_only {
            return;
        }
        let cur = self.cur;
        let deck = self.pres.edit("slides.undo.media");
        let s = deck.slide_mut(cur);
        let trigger = match start {
            Start::Auto => Some(Trigger::AfterPrev),
            Start::ClickSequence => Some(Trigger::OnClick),
            Start::Clicked => None,
        };
        s.timing.set_media_start(id, trigger, 0.0);
        if let Some(o) = apex_slides::edit::find_mut(&mut s.objects, id) {
            if let ObjKind::Picture(p) = &mut o.kind
                && let Some(m) = &mut p.media
            {
                m.autoplay = start == Start::Auto;
            }
            o.touch();
        }
    }

    /// Plays or pauses the selected media on the canvas.
    pub fn toggle_media(&mut self) {
        let Some((id, m)) = self.selected_media().map(|(id, m)| (id, m.clone())) else { return };
        let Some(sid) = self.pres.slides.get(self.cur).map(|s| s.id) else { return };
        if let Some(p) = self.media.players.get_mut(&(sid, id))
            && !p.ended()
        {
            p.toggle();
            return;
        }
        // On another computer: the user says first (the show never asks:
        // there it doesn't play until allowed here).
        if media::player::network_link(&m).is_some() {
            self.media.network = Some((sid, id, m));
            return;
        }
        match start_player(&m) {
            Ok(p) => {
                self.media.players.insert((sid, id), p);
            }
            Err(e) => self.media_error(e),
        }
    }

    /// Whether the selected media plays on the canvas.
    pub fn media_playing(&self) -> bool {
        let Some((id, _)) = self.selected_media() else { return false };
        let sid = self.pres.slides.get(self.cur).map_or(0, |s| s.id);
        self.media.players.get(&(sid, id)).is_some_and(|p| p.playing())
    }

    fn media_error(&mut self, e: String) {
        if !decode::available() {
            self.warn_no_media();
        } else {
            self.notices.push(Notice::error(e));
        }
    }

    /// Says once that the system has no media support (videos show their
    /// poster).
    fn warn_no_media(&mut self) {
        if !self.media.warned {
            self.media.warned = true;
            self.notices.push(Notice::info(tr("slides.media.no_support")));
        }
    }

    /// Stops what plays on the canvas (leaving the slide, a show).
    pub fn stop_media(&mut self) {
        self.media.players.clear();
    }

    /// Paints what plays on the canvas's slide over the objects.
    pub(super) fn paint_canvas_media(&mut self, p: &Painter, xf: Xf) {
        if self.media.players.is_empty() {
            return;
        }
        let Some(s) = self.pres.slides.get(self.cur).filter(|_| matches!(self.target(), Target::Slide(_))) else {
            self.media.players.clear();
            return;
        };
        let sid = s.id;
        // Players of other slides, gone objects or finished stop.
        self.media.players.retain(|(ps, id), pl| *ps == sid && s.objects.iter().any(|o| o.id == *id) && !pl.ended());
        let ch = Chain::for_slide(&self.pres.deck, s);
        for o in &s.objects {
            if let Some(pl) = self.media.players.get_mut(&(sid, o.id)) {
                paint_player(p, pl, xf.rect(&ch.frame(o)), true);
            }
        }
        if !self.media.players.is_empty() {
            p.ctx().request_repaint_after(web_time::Duration::from_millis(15));
        }
    }

    // --- the Playback tab ----------------------------------------------------------------

    pub(super) fn playback_tab(&mut self, b: &mut Body) {
        let acc = b.accent();
        let Some((_, m)) = self.selected_media() else { return };
        let (looped, volume, mute) = (m.looped, m.volume, m.mute);
        let playing = self.media_playing();
        b.group("slides.media.g.preview", Icon::SpeakPlay, |ui| {
            let (icon, label) = if playing { (Icon::SpeakPause, "slides.media.pause") } else { (Icon::SpeakPlay, "slides.media.play") };
            if toolstrip::large(ui, icon, tr(label), tr("slides.media.play_tip"), acc, false).clicked() {
                self.toggle_media();
            }
        });
        b.group("slides.media.g.editing", Icon::Video, |ui| {
            ui.add_enabled_ui(!self.read_only, |ui| {
                if toolstrip::large(ui, Icon::Video, tr("slides.media.trim"), tr("slides.media.trim_tip"), acc, false).clicked() {
                    self.open_trim();
                }
            });
        });
        b.group("slides.media.g.options", Icon::Audio, |ui| {
            ui.add_enabled_ui(!self.read_only, |ui| {
                ui.vertical(|ui| {
                    let mut vol = None;
                    toolstrip::small_menu(ui, Some(Icon::Audio), tr("slides.media.volume"), tr("slides.media.volume_tip"), acc, |ui| {
                        for (v, key) in VOLUMES {
                            let on = if v == 0.0 { mute } else { !mute && (volume - v).abs() < 0.01 };
                            if toolstrip::small(ui, None, tr(key), "", acc, on).clicked() {
                                vol = Some(v);
                                ui.close();
                            }
                        }
                    });
                    if let Some(v) = vol {
                        self.edit_media(None, |m| {
                            m.mute = v == 0.0;
                            if v > 0.0 {
                                m.volume = v;
                            }
                        });
                    }
                    let now = self.media_start();
                    let mut start = None;
                    toolstrip::small_menu(ui, None, tr("slides.media.start"), tr("slides.media.start_tip"), acc, |ui| {
                        for (s, key) in STARTS {
                            if toolstrip::small(ui, None, tr(key), "", acc, now == Some(s)).clicked() {
                                start = Some(s);
                                ui.close();
                            }
                        }
                    });
                    if let Some(s) = start {
                        self.set_media_start(s);
                    }
                    if toolstrip::check(ui, looped, Icon::Redo, tr("slides.media.loop")) {
                        self.edit_media(None, |m| m.looped = !m.looped);
                    }
                });
            });
        });
    }

    /// Opens the trim dialog for the selected media.
    pub fn open_trim(&mut self) {
        let Some((id, m)) = self.selected_media() else { return };
        let duration = media::player::source(m).and_then(|s| decode::probe(&s).ok()).map_or(0.0, |i| i.duration);
        self.media.trim = Some(TrimDlg { id, trim: m.trim, duration });
    }

    /// Cuts `trim` (seconds from the start and the end) off media `id`.
    pub fn set_trim(&mut self, id: u32, trim: (f32, f32)) {
        if self.selected_media().map(|(i, _)| i) != Some(id) {
            self.sel = vec![id];
        }
        self.edit_media(None, |m| m.trim = (trim.0.max(0.0), trim.1.max(0.0)));
    }

    /// The question before playing a media linked on another computer.
    pub(super) fn network_media_dialog(&mut self, ctx: &egui::Context) {
        let Some((sid, id, m)) = self.media.network.take() else { return };
        let link = media::player::network_link(&m).unwrap_or_default().to_string();
        let res = crate::widgets::dialog(ctx, "slides_media_network", tr("slides.media.network_title"), 420.0, |ui| {
            ui.label(trf("slides.media.network_ask", &[("x", &link)]));
        });
        match res {
            crate::widgets::DialogResult::Ok => {
                media::player::allow_network(&link);
                match start_player(&m) {
                    Ok(p) => {
                        self.media.players.insert((sid, id), p);
                    }
                    Err(e) => self.media_error(e),
                }
            }
            crate::widgets::DialogResult::Cancel => {}
            crate::widgets::DialogResult::None => self.media.network = Some((sid, id, m)),
        }
    }

    pub(super) fn trim_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.media.trim.take() else { return };
        let res = crate::widgets::dialog(ctx, "slides_media_trim", tr("slides.media.trim_title"), 340.0, |ui| {
            let dur = d.duration as f32;
            if dur > 0.0 {
                ui.label(trf("slides.media.duration", &[("time", &media::clock_text(d.duration))]));
            }
            let max = if dur > 0.0 { dur } else { 36_000.0 };
            egui::Grid::new("media_trim").num_columns(2).show(ui, |ui| {
                ui.label(tr("slides.media.trim_start"));
                let mut start = d.trim.0;
                ui.add(egui::DragValue::new(&mut start).range(0.0..=max).speed(0.05).max_decimals(2).suffix(" s"));
                d.trim.0 = start;
                ui.end_row();
                ui.label(tr("slides.media.trim_end"));
                // The end is shown as a time, kept as the amount cut.
                let mut end = if dur > 0.0 { dur - d.trim.1 } else { d.trim.1 };
                ui.add(egui::DragValue::new(&mut end).range(0.0..=max).speed(0.05).max_decimals(2).suffix(" s"));
                d.trim.1 = if dur > 0.0 { (dur - end).max(0.0) } else { end };
                ui.end_row();
            });
            if dur > 0.0 && d.trim.0 + d.trim.1 > dur - 0.1 {
                d.trim.0 = (dur - d.trim.1 - 0.1).max(0.0);
            }
        });
        match res {
            crate::widgets::DialogResult::Ok => self.set_trim(d.id, d.trim),
            crate::widgets::DialogResult::Cancel => {}
            crate::widgets::DialogResult::None => self.media.trim = Some(d),
        }
    }

    /// The Playback tab's commands for the search, on a sample slide.
    pub fn media_commands(ui: &mut egui::Ui) -> Vec<toolstrip::Entry> {
        let mut v = SlidesView::new(apex_slides::build::new_deck(0));
        let m = Media::embedded(Arc::from(&b"sample"[..]), "video/mp4", true);
        let mut o = Object::new(0, "Video", ObjKind::Picture(Box::new(Picture { image: apex_doc::Image::new(Arc::from(crate::doc_view::SAMPLE_PNG), 20.0, 20.0), media: Some(m) })));
        o.sp.frame = Some(apex_slides::Frame::new(10.0, 10.0, 20.0, 20.0));
        v.insert_object("slides.undo.insert_media", o);
        toolstrip::collect(|| toolstrip::within(tr("slides.media.playback"), || toolstrip::body(ui, egui::Id::new("slides_media_cmds"), MEDIA_TAB, |b| v.playback_tab(b))))
    }

    pub fn playback_tab_def() -> TabDef {
        TabDef::contextual("playback", "slides.media.playback", MEDIA_TAB)
    }
}

fn start_player(m: &Media) -> Result<Player, String> {
    let mut m = m.clone();
    if m.mute {
        m.volume = 0.0;
    }
    Player::start(&m, PLAY_PX)
}

/// Paints a player over its object: the picture playing and, on the
/// canvas, a bar of how far it is.
fn paint_player(p: &Painter, pl: &mut Player, rect: Rect, bar: bool) {
    let has_video = pl.info.size.is_some();
    if has_video && let Some(t) = pl.texture(p.ctx()) {
        p.image(t.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
    if bar && pl.length > 0.0 && pl.length.is_finite() {
        let k = (pl.position() / pl.length).clamp(0.0, 1.0) as f32;
        let h = 4.0f32.min(rect.height() * 0.1);
        let r = Rect::from_min_size(pos2(rect.min.x, rect.max.y - h), vec2(rect.width(), h));
        p.rect_filled(r, 0.0, Color32::from_black_alpha(120));
        p.rect_filled(Rect::from_min_size(r.min, vec2(r.width() * k, h)), 0.0, crate::theme::SLIDES_ACCENT);
    }
    if !pl.playing() && !pl.ended() {
        p.rect_stroke(rect, 0.0, Stroke::new(1.0, Color32::from_gray(160)), egui::StrokeKind::Inside);
    }
}

/// The media objects of slide `i` and their frames (points).
fn media_objects(deck: &Deck, i: usize) -> Vec<(u32, Media, apex_slides::Frame)> {
    let s = &deck.slides[i];
    let ch = Chain::for_slide(deck, s);
    s.objects.iter().filter_map(|o| media_of(o).map(|m| (o.id, m.clone(), ch.frame(o)))).collect()
}

/// Whether screen point `q` is on a video or sound of the show's slide,
/// drawn in `rect`.
pub fn show_media_at(deck: &Deck, s: &Show, rect: Rect, q: egui::Pos2) -> bool {
    let Some(i) = s.slide() else { return false };
    let xf = Xf::for_slide(deck, show::slide_rect(deck, rect));
    media_objects(deck, i).iter().any(|(_, _, f)| xf.rect(f).contains(q))
}

impl Playing {
    /// In the show, before its input: a click on a video or sound plays
    /// or pauses it instead of going on. True if it took the click.
    pub fn show_click(&mut self, deck: &Deck, s: &Show, resp: &egui::Response) -> bool {
        let (Some(i), Some(q)) = (s.slide(), resp.interact_pointer_pos()) else { return false };
        if !resp.clicked() || s.blank.is_some() || s.kiosk || !matches!(s.ink.pointer, super::show_ink::Pointer::Arrow) {
            return false;
        }
        let xf = Xf::for_slide(deck, show::slide_rect(deck, resp.rect));
        let Some((id, m, _)) = media_objects(deck, i).into_iter().rev().find(|(_, _, f)| xf.rect(f).contains(q)) else { return false };
        self.show_slide = Some(i);
        match self.show_players.get_mut(&id) {
            Some(p) if !p.ended() => p.toggle(),
            _ => {
                if self.show_players.len() < MAX_PLAYERS
                    && let Ok(p) = start_player(&m)
                {
                    self.show_players.insert(id, p);
                }
                self.show_started.insert(id);
            }
        }
        true
    }

    /// In the show, after painting its slide in `rect`: starts what's due
    /// and paints what plays.
    pub fn show_frame(&mut self, p: &Painter, deck: &Deck, s: &Show, rect: Rect, now: f64) {
        let slide = s.slide();
        if slide != self.show_slide {
            self.show_players.clear();
            self.show_started.clear();
            self.show_slide = slide;
        }
        let Some(i) = slide else { return };
        if s.blank.is_some() || s.in_transition(deck, now) {
            return;
        }
        let objs = media_objects(deck, i);
        let timing = &deck.slides[i].timing;
        // Due: their play animation started, or (no such animation) set to
        // play by themselves.
        let mut due = s.media_due(deck, now);
        if objs.iter().any(|(_, m, _)| m.autoplay) {
            let animated: HashSet<u32> = timing.anims.iter().filter(|a| a.class == apex_slides::anim::Class::Media).map(|a| a.shape).collect();
            due.extend(objs.iter().filter(|(id, m, _)| m.autoplay && !animated.contains(id)).map(|(id, ..)| *id));
        }
        let at: HashMap<u32, usize> = objs.iter().enumerate().map(|(n, (id, ..))| (*id, n)).collect();
        for id in due {
            if self.show_players.len() < MAX_PLAYERS
                && self.show_started.insert(id)
                && let Some(&n) = at.get(&id)
                && let Ok(pl) = start_player(&objs[n].1)
            {
                self.show_players.insert(id, pl);
            }
        }
        if self.show_players.is_empty() {
            return;
        }
        let xf = Xf::for_slide(deck, show::slide_rect(deck, rect));
        let p = p.with_clip_rect(show::slide_rect(deck, rect).intersect(p.clip_rect()));
        for (id, _, f) in &objs {
            if let Some(pl) = self.show_players.get_mut(id) {
                paint_player(&p, pl, xf.rect(f), false);
            }
        }
        self.show_players.retain(|_, pl| !pl.ended());
        p.ctx().request_repaint_after(web_time::Duration::from_millis(15));
    }

    /// The show ended.
    pub fn end_show(&mut self) {
        self.show_players.clear();
        self.show_started.clear();
        self.show_slide = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIP: &[u8] = include_bytes!("../../tests/data/clip.mp4");

    #[test]
    fn insert_set_and_undo() {
        let mut v = SlidesView::new(apex_slides::build::new_deck(0));
        assert!(v.insert_media(b"not a video".to_vec(), "notes.txt").is_err());
        let id = v.insert_media(CLIP.to_vec(), "clip.mp4").unwrap();
        let (_, m) = v.selected_media().unwrap();
        assert!(m.video && m.data.is_some() && m.mime == "video/mp4");
        if decode::probe(&decode::Source::Bytes(Arc::from(CLIP))).is_ok() {
            // The poster is the first picture, 64x48 pixels.
            let ObjKind::Picture(p) = &v.selected_object().unwrap().kind else { panic!() };
            assert_eq!(apex_io::pixel_size(&p.image.data), Some((64, 48)));
        }
        assert_eq!(v.media_start(), Some(Start::Clicked));
        v.set_media_start(Start::Auto);
        assert_eq!(v.media_start(), Some(Start::Auto));
        assert!(v.selected_media().unwrap().1.autoplay);
        v.set_media_start(Start::ClickSequence);
        assert_eq!(v.pres.slides[0].timing.anims.len(), 1);
        v.edit_media(None, |m| m.looped = true);
        v.set_trim(id, (0.5, 0.25));
        assert_eq!(v.selected_media().unwrap().1.trim, (0.5, 0.25));
        v.undo();
        v.undo();
        assert!(!v.selected_media().unwrap().1.looped);
        v.undo();
        assert_eq!(v.media_start(), Some(Start::Auto));
        v.undo();
        v.undo();
        assert!(v.selected_media().is_none());
        // A sound goes to the corner, even one the system can't decode
        // (with a notice).
        let mut v = SlidesView::new(apex_slides::build::new_deck(0));
        let id = v.insert_media(b"ID3 sound".to_vec(), "talk.mp3").unwrap();
        let o = v.pres.slides[0].objects.iter().find(|o| o.id == id).unwrap();
        assert!(o.sp.frame.unwrap().x > v.pres.width / 2.0);
        assert!(!media_of(o).unwrap().video);
        assert_eq!(v.notices.len(), 1);
        // Without media support it's said once.
        v.notices.clear();
        v.warn_no_media();
        v.warn_no_media();
        assert_eq!(v.notices.len(), 1);
    }

    #[test]
    fn show_starts_media_by_timing() {
        let mut v = SlidesView::new(apex_slides::build::new_deck(0));
        let id = v.insert_media(CLIP.to_vec(), "clip.mp4").unwrap();
        v.set_media_start(Start::ClickSequence);
        let deck = v.pres.deck.clone();
        let mut s = Show::new(&deck, 0, 0.0);
        assert!(s.media_due(&deck, 0.0).is_empty());
        s.next(&deck, 1.0);
        assert_eq!(s.media_due(&deck, 1.0), vec![id]);
        v.set_media_start(Start::Auto);
        let deck = v.pres.deck.clone();
        let s = Show::new(&deck, 0, 0.0);
        assert_eq!(s.media_due(&deck, 0.1), vec![id]);
    }

    #[test]
    fn playback_commands_are_found() {
        let ctx = egui::Context::default();
        let mut names = Vec::new();
        ctx.run_ui(Default::default(), |ui| {
            names = SlidesView::media_commands(ui).into_iter().map(|e| e.label).collect();
        })
        .textures_delta
        .clear();
        for key in ["slides.media.play", "slides.media.trim", "slides.media.loop"] {
            assert!(names.iter().any(|n| n.contains(tr(key))), "{key}: {names:?}");
        }
    }
}
