//! Playing a video or sound: a thread decodes ahead (a few pictures, half
//! a second of sound) from the trimmed start; the pictures are shown by
//! the wall clock and the sound goes to the system's output as it comes,
//! both starting together. At the trimmed end it stops, or starts again
//! when looped.

use apex_fs::PathExt;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use apex_fs::thread::JoinHandle;
use web_time::{Duration, Instant};

use apex_slides::object::Media;
use eframe::egui;

use super::audio_out::{self, Ring};
use super::decode::{self, Decoder, Info, Out, Source};

/// Pictures decoded ahead.
const AHEAD: usize = 6;
/// Seconds of sound decoded ahead.
const SOUND_AHEAD: f64 = 0.5;

/// A picture and when it shows (seconds from the start of playing).
struct Picture {
    t: f64,
    image: egui::ColorImage,
}

#[derive(Default)]
struct Shared {
    pictures: Mutex<VecDeque<Picture>>,
    stop: AtomicBool,
    /// The decoder reached the end (and doesn't loop).
    ended: AtomicBool,
}

/// The time played, stopping while paused.
#[derive(Clone, Copy, Debug)]
struct Clock {
    /// Seconds before the last resume.
    before: f64,
    /// Since when it runs.
    since: Option<Instant>,
}

impl Clock {
    fn now(&self) -> f64 {
        self.before + self.since.map_or(0.0, |s| s.elapsed().as_secs_f64())
    }
}

/// Network links (`\\server\share`, `file://server/…`) the user allowed
/// to play in this session.
static ALLOWED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The user allowed `link`, a network link, to play.
pub fn allow_network(link: &str) {
    if let Ok(mut a) = ALLOWED.lock() {
        a.push(link.to_string());
    }
}

/// The link of a media linked to a file on another computer that the
/// user hasn't allowed yet: even looking for it connects to that computer
/// (on Windows, signing in to it as the user), so nothing touches it
/// before the user says so. The same rule as Get Data's network paths.
pub fn network_link(m: &Media) -> Option<&str> {
    if m.data.is_some() {
        return None;
    }
    let link = m.link.as_deref()?;
    let allowed = ALLOWED.lock().is_ok_and(|a| a.iter().any(|l| l == link));
    let remote = apex_sheet::query::is_network_path(link) || apex_sheet::query::is_network_path(&local_path(link).to_string_lossy());
    (remote && !allowed).then_some(link)
}

/// The path a media's link names.
fn local_path(link: &str) -> std::path::PathBuf {
    let path = link.strip_prefix("file://").map(|p| p.replace("%20", " ")).unwrap_or_else(|| link.to_string());
    // file:///C:/… on Windows.
    let path = if cfg!(windows) { path.trim_start_matches('/').to_string() } else { path };
    std::path::PathBuf::from(path)
}

/// Where a media's file is: in the presentation, or a path or file URL.
/// None for a network link not allowed yet ([`network_link`]).
pub fn source(m: &Media) -> Option<Source> {
    if let Some(d) = &m.data {
        return Some(Source::Bytes(d.clone()));
    }
    if network_link(m).is_some() {
        return None;
    }
    let p = local_path(m.link.as_deref()?);
    p.fs_is_file().then_some(Source::Path(p))
}

/// A video or sound playing.
pub struct Player {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    ring: Option<Arc<Ring>>,
    _out: Option<audio_out::Output>,
    clock: Clock,
    /// The picture shown.
    tex: Option<egui::TextureHandle>,
    pub info: Info,
    /// Seconds it plays (the trimmed length; 0 when unknown).
    pub length: f64,
    looped: bool,
}

impl Player {
    /// Starts playing `m` at its trimmed start; pictures at most `max`
    /// pixels on their longer side.
    pub fn start(m: &Media, max: usize) -> Result<Player, String> {
        let src = source(m).ok_or("file not found")?;
        let mut dec = Decoder::open(&src, m.video)?;
        let info = dec.info();
        let (from, to) = trimmed(m, info.duration);
        if let Some(s) = info.size {
            dec.out_size = Some(decode::fit(s, max));
        }
        let (ring, out) = if info.audio && !m.mute {
            let ring = Ring::new(m.volume);
            match audio_out::open(ring.clone()) {
                Ok(o) => {
                    dec.out_rate = o.rate;
                    (Some(ring), Some(o))
                }
                Err(e) => {
                    crate::diag::warning(&format!("sound: {e}"));
                    (None, None)
                }
            }
        } else {
            (None, None)
        };
        let shared = Arc::new(Shared::default());
        let job = Job { dec, from, to, looped: m.looped, shared: shared.clone(), ring: ring.clone(), video: info.size.is_some() };
        let thread = apex_fs::thread::Builder::new().name("media".into()).spawn(move || job.run()).map_err(|e| e.to_string())?;
        Ok(Player { shared, thread: Some(thread), ring, _out: out, clock: Clock { before: 0.0, since: Some(Instant::now()) }, tex: None, info, length: (to - from).max(0.0), looped: m.looped })
    }

    pub fn playing(&self) -> bool {
        self.clock.since.is_some() && !self.ended()
    }

    /// It played to the end.
    pub fn ended(&self) -> bool {
        !self.looped && self.shared.ended.load(Ordering::Relaxed) && self.shared.pictures.lock().unwrap().is_empty() && self.ring.as_ref().is_none_or(|r| r.frames() == 0)
    }

    /// Pauses or goes on.
    pub fn toggle(&mut self) {
        self.set_paused(self.clock.since.is_some());
    }

    pub fn set_paused(&mut self, paused: bool) {
        match (paused, self.clock.since) {
            (true, Some(_)) => self.clock = Clock { before: self.clock.now(), since: None },
            (false, None) => self.clock.since = Some(Instant::now()),
            _ => {}
        }
        if let Some(r) = &self.ring {
            r.paused.store(paused, Ordering::Relaxed);
        }
    }

    /// Seconds played (looping: of this round).
    pub fn position(&self) -> f64 {
        let t = self.clock.now();
        if self.length <= 0.0 { t } else if self.looped { t % self.length } else { t.min(self.length) }
    }

    pub fn set_volume(&self, v: f32) {
        if let Some(r) = &self.ring {
            r.set_volume(v);
        }
    }

    /// The picture to show now (the latest due), uploaded to `ctx`.
    pub fn texture(&mut self, ctx: &egui::Context) -> Option<&egui::TextureHandle> {
        let now = self.clock.now();
        let due = {
            let mut q = self.shared.pictures.lock().unwrap();
            let mut due = None;
            while q.front().is_some_and(|p| p.t <= now || self.tex.is_none()) {
                due = q.pop_front();
                if self.tex.is_none() {
                    break;
                }
            }
            due
        };
        if let Some(p) = due {
            match &mut self.tex {
                Some(t) => t.set(p.image, egui::TextureOptions::LINEAR),
                None => self.tex = Some(ctx.load_texture("media-frame", p.image, egui::TextureOptions::LINEAR)),
            }
        }
        self.tex.as_ref()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The trimmed start and end, seconds.
pub fn trimmed(m: &Media, duration: f64) -> (f64, f64) {
    let from = m.trim.0.max(0.0) as f64;
    let to = if duration > 0.0 { (duration - m.trim.1.max(0.0) as f64).max(from) } else { f64::INFINITY };
    (from, to)
}

/// The decoding thread's work.
struct Job {
    dec: Decoder,
    from: f64,
    to: f64,
    looped: bool,
    shared: Arc<Shared>,
    ring: Option<Arc<Ring>>,
    video: bool,
}

impl Job {
    fn run(mut self) {
        let rate = self.dec.out_rate as f64;
        // Seconds of play before this round (looping).
        let mut base = 0.0;
        // Sound handed over, seconds from the start of playing.
        let mut sound_at = 0.0;
        if self.from > 0.0 {
            self.dec.seek(self.from);
        }
        let mut round_len: f64 = 0.0;
        while !self.shared.stop.load(Ordering::Relaxed) {
            // Enough ahead: wait.
            let pictures = self.shared.pictures.lock().unwrap().len();
            let sound = self.ring.as_ref().map_or(0.0, |r| r.frames() as f64 / rate);
            if (!self.video || pictures >= AHEAD) && (self.ring.is_none() || sound >= SOUND_AHEAD) && (self.video || self.ring.is_some()) {
                apex_fs::thread::sleep(Duration::from_millis(5));
                continue;
            }
            let out = self.dec.next();
            let end = match &out {
                None => true,
                Some(Out::Video { t, .. }) | Some(Out::Audio { t, .. }) => *t >= self.to,
            };
            if end {
                if !self.looped {
                    self.shared.ended.store(true, Ordering::Relaxed);
                    // Waits to be dropped.
                    while !self.shared.stop.load(Ordering::Relaxed) {
                        apex_fs::thread::sleep(Duration::from_millis(20));
                    }
                    return;
                }
                let len = if self.to.is_finite() { self.to - self.from } else { round_len };
                if len <= 0.0 {
                    self.shared.ended.store(true, Ordering::Relaxed);
                    return;
                }
                base += len;
                round_len = 0.0;
                self.dec.seek(self.from);
                continue;
            }
            match out {
                Some(Out::Video { t, w, h, rgba }) if t + 0.02 >= self.from => {
                    round_len = round_len.max(t - self.from);
                    let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
                    self.shared.pictures.lock().unwrap().push_back(Picture { t: base + t - self.from, image });
                }
                Some(Out::Audio { t, samples }) if t + 0.02 >= self.from => {
                    round_len = round_len.max(t - self.from);
                    if let Some(r) = &self.ring {
                        // Silence first if sound starts later than play.
                        let at = base + t - self.from;
                        if at > sound_at + 0.05 {
                            // At most a minute of it: a file's timestamps may
                            // jump anywhere.
                            r.push(&vec![0.0; ((at - sound_at).min(60.0) * rate) as usize * 2]);
                            sound_at = at;
                        }
                        r.push(&samples);
                        sound_at += samples.len() as f64 / 2.0 / rate;
                    }
                }
                _ => {}
            }
        }
    }
}

/// The poster of a video: its picture at `t` (or the first) as PNG.
pub fn poster_png(src: &Source, t: f64) -> Option<(Vec<u8>, usize, usize)> {
    let (w, h, rgba) = decode::picture_at(src, t, 1280)?;
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().ok()?;
        wr.write_image_data(&rgba).ok()?;
    }
    Some((out, w, h))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn trims_and_sources() {
        let mut m = Media::embedded(Arc::from(&b"x"[..]), "video/mp4", true);
        m.trim = (1.0, 2.0);
        assert_eq!(trimmed(&m, 10.0), (1.0, 8.0));
        assert_eq!(trimmed(&m, 0.0).1, f64::INFINITY);
        assert!(matches!(source(&m), Some(Source::Bytes(_))));
        m.data = None;
        m.link = Some("file:///no/such/file.mp4".into());
        assert!(source(&m).is_none());
        assert!(network_link(&m).is_none());
    }

    #[test]
    fn network_links_wait_for_the_user() {
        let mut m = Media::embedded(Arc::from(&b"x"[..]), "video/mp4", true);
        m.data = None;
        for link in ["\\\\fileserver\\share\\a.mp4", "//fileserver/share/b.mp4", "file://fileserver/share/c.mp4"] {
            m.link = Some(link.into());
            assert_eq!(network_link(&m), Some(link));
            assert!(source(&m).is_none());
        }
        m.link = Some("//fileserver/share/d.mp4".into());
        allow_network("//fileserver/share/d.mp4");
        assert!(network_link(&m).is_none());
    }

    /// Pictures keep coming while it plays, and it ends (with a system's
    /// decoder; else nothing to check).
    #[test]
    fn plays_through() {
        let mut m = Media::embedded(Arc::from(&include_bytes!("../../tests/data/clip.mp4")[..]), "video/mp4", true);
        m.mute = true;
        let Ok(mut p) = Player::start(&m, 64) else { return };
        let ctx = egui::Context::default();
        let mut shown = 0;
        let start = Instant::now();
        while !p.ended() && start.elapsed() < Duration::from_secs(5) {
            let before = p.shared.pictures.lock().unwrap().len();
            p.texture(&ctx);
            if p.shared.pictures.lock().unwrap().len() < before {
                shown += 1;
            }
            apex_fs::thread::sleep(Duration::from_millis(10));
        }
        assert!(p.ended(), "{}", p.position());
        assert!(shown >= 15, "{shown}");
        assert!(start.elapsed() >= Duration::from_millis(1800), "{:?}", start.elapsed());
    }

    #[test]
    fn clock_stops_while_paused() {
        let c = Clock { before: 2.0, since: None };
        assert_eq!(c.now(), 2.0);
        let c = Clock { before: 1.0, since: Some(Instant::now() - Duration::from_millis(500)) };
        assert!(c.now() >= 1.5);
    }
}
