//! Decoding videos and sounds with what the system has: GStreamer on Linux
//! and the BSDs ([`super::gst`]), Media Foundation on Windows
//! ([`super::mf`]), AVFoundation on macOS (`avf`, not wired up yet). Built with the
//! `ffmpeg` feature, an FFmpeg component is tried first. `APEXDESK_MEDIA=off`
//! turns them all off.
//!
//! A [`Decoder`] reads a file (in memory or on disk) and gives its video
//! frames as RGBA pictures and its sound as interleaved stereo samples, in
//! the order they come; the player times them.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

/// Where a file to play is.
#[derive(Clone, Debug)]
pub enum Source {
    Bytes(Arc<[u8]>),
    Path(PathBuf),
}

/// What a decoder gives.
pub enum Out {
    /// A picture: its time (seconds), size and RGBA pixels.
    Video { t: f64, w: usize, h: usize, rgba: Vec<u8> },
    /// Interleaved stereo samples at the rate asked for, from time `t`.
    Audio { t: f64, samples: Vec<f32> },
}

/// What a file holds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Info {
    /// Seconds (0 when unknown).
    pub duration: f64,
    /// The video's size, if it has pictures.
    pub size: Option<(usize, usize)>,
    pub audio: bool,
}

/// A way of decoding (FFmpeg or the system's).
pub trait Backend: Send {
    fn info(&self) -> Info;
    /// The size pictures are given at (none: their own) and the sample
    /// rate of sound; set before the first [`Backend::seek`] or
    /// [`Backend::next`], and again when they change.
    fn set_output(&mut self, size: Option<(usize, usize)>, rate: u32);
    /// Goes to `t` seconds (at or before it: what comes before `t` is the
    /// caller's to skip).
    fn seek(&mut self, t: f64);
    /// The next picture or sound; none at the end.
    fn next(&mut self) -> Option<Out>;
}

type Open = fn(&Source, bool) -> Result<Box<dyn Backend>, String>;

/// The backends of this build, in the order they're tried.
fn backends() -> Vec<(&'static str, fn() -> Result<(), String>, Open)> {
    let mut v: Vec<(&'static str, fn() -> Result<(), String>, Open)> = Vec::new();
    #[cfg(feature = "ffmpeg")]
    v.push(("FFmpeg", super::ffmpeg::load, super::ffmpeg::open));
    #[cfg(all(unix, not(target_os = "macos")))]
    v.push(("GStreamer", super::gst::load, super::gst::open));
    #[cfg(windows)]
    v.push(("Media Foundation", super::mf::load, super::mf::open));
    #[cfg(target_os = "macos")]
    v.push(("AVFoundation", super::avf::load, super::avf::open));
    v
}

/// The backends that loaded, once.
fn loaded() -> &'static [(&'static str, Open)] {
    static LOADED: OnceLock<Vec<(&'static str, Open)>> = OnceLock::new();
    LOADED.get_or_init(|| {
        if std::env::var_os("APEXDESK_MEDIA").is_some_and(|v| v == "off") {
            return Vec::new();
        }
        let mut ok = Vec::new();
        for (name, load, open) in backends() {
            match load() {
                Ok(()) => ok.push((name, open)),
                Err(e) => crate::diag::warning(&format!("{name}: {e}")),
            }
        }
        ok
    })
}

/// Whether videos and sounds can be played here.
pub fn available() -> bool {
    !loaded().is_empty()
}

/// The system's media framework, by name.
#[cfg(test)]
pub const SYSTEM: &str = if cfg!(windows) { "Media Foundation" } else if cfg!(target_os = "macos") { "AVFoundation" } else { "GStreamer" };

/// A file being decoded.
pub struct Decoder {
    b: Box<dyn Backend>,
    /// The size pictures are given at (none: their own).
    pub out_size: Option<(usize, usize)>,
    /// The sample rate sound is given at.
    pub out_rate: u32,
    /// What the backend was last told.
    applied: Option<(Option<(usize, usize)>, u32)>,
}

impl Decoder {
    /// Opens a file and its video and sound streams (`want_video`: false
    /// to decode only the sound), with the first backend that can.
    pub fn open(src: &Source, want_video: bool) -> Result<Decoder, String> {
        if is_manifest(src) {
            return Err("a streaming playlist, not a media file".into());
        }
        let mut err = String::from("no media support");
        for (name, open) in loaded() {
            match open(src, want_video) {
                // Neither sound nor pictures: nothing this way can decode.
                Ok(b) if b.info().size.is_none() && !b.info().audio => err = format!("{name}: no sound or pictures"),
                Ok(b) => return Ok(Decoder { b, out_size: None, out_rate: 48_000, applied: None }),
                Err(e) => err = format!("{name}: {e}"),
            }
        }
        Err(err)
    }

    pub fn info(&self) -> Info {
        self.b.info()
    }

    fn apply(&mut self) {
        let want = (self.out_size, self.out_rate);
        if self.applied != Some(want) {
            self.b.set_output(want.0, want.1);
            self.applied = Some(want);
        }
    }

    /// Goes to `t` seconds (at or before it: what comes before `t` is the
    /// caller's to skip).
    pub fn seek(&mut self, t: f64) {
        self.apply();
        self.b.seek(t.max(0.0));
    }

    pub fn next(&mut self) -> Option<Out> {
        self.apply();
        self.b.next()
    }
}

/// What a file holds, without playing it.
pub fn probe(src: &Source) -> Result<Info, String> {
    Decoder::open(src, true).map(|d| d.info())
}

/// The picture at `t` seconds (or the first one) of a video, at most
/// `max` pixels on its longer side: width, height and RGBA pixels.
pub fn picture_at(src: &Source, t: f64, max: usize) -> Option<(usize, usize, Vec<u8>)> {
    let mut d = Decoder::open(src, true).ok()?;
    let (w, h) = d.info().size?;
    d.out_size = Some(fit((w, h), max));
    if t > 0.0 {
        d.seek(t);
    }
    let mut first = None;
    // Frames before `t` after seeking to the key frame before it.
    for _ in 0..2000 {
        match d.next()? {
            Out::Video { t: ft, w, h, rgba } => {
                if ft + 0.001 >= t {
                    return Some((w, h, rgba));
                }
                first = Some((w, h, rgba));
            }
            Out::Audio { .. } => {}
        }
    }
    first
}

/// A size fitted in `max` pixels on its longer side, even.
pub fn fit((w, h): (usize, usize), max: usize) -> (usize, usize) {
    let k = (max as f64 / w.max(h).max(1) as f64).min(1.0);
    let even = |v: f64| ((v.round() as usize).max(2) + 1) & !1;
    (even(w as f64 * k), even(h as f64 * k))
}

/// A picture of `sw`×`sh` pixels, 4 bytes each, rows `stride` bytes apart
/// and in B, G, R order when `bgr`, as RGBA of `dw`×`dh` (the nearest
/// pixel; alpha opaque). For systems that don't scale or convert.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub fn to_rgba(src: &[u8], sw: usize, sh: usize, stride: usize, bgr: bool, (dw, dh): (usize, usize)) -> Vec<u8> {
    let mut out = vec![0u8; dw * dh * 4];
    if sw == 0 || sh == 0 {
        return out;
    }
    for y in 0..dh {
        let row = (y * sh / dh.max(1)).min(sh - 1) * stride;
        for x in 0..dw {
            let at = row + (x * sw / dw.max(1)).min(sw - 1) * 4;
            let Some(p) = src.get(at..at + 4) else { continue };
            let o = &mut out[(y * dw + x) * 4..][..4];
            if bgr {
                o.copy_from_slice(&[p[2], p[1], p[0], 255]);
            } else {
                o.copy_from_slice(&[p[0], p[1], p[2], 255]);
            }
        }
    }
    out
}

/// Sound of `channels` interleaved at `from` samples per second as
/// interleaved stereo at `to` (linear), carrying `pos` (the fraction
/// between calls) and `last` (the frame before) from one piece to the next.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub struct Resampler {
    pub from: u32,
    pub to: u32,
    pub channels: usize,
    pos: f64,
    last: Option<[f32; 2]>,
}

#[cfg_attr(not(any(windows, test)), allow(dead_code))]
impl Resampler {
    pub fn new(from: u32, to: u32, channels: usize) -> Resampler {
        Resampler { from: from.max(1), to: to.max(1), channels: channels.max(1), pos: 0.0, last: None }
    }

    /// Starts afresh (after seeking).
    pub fn reset(&mut self) {
        self.pos = 0.0;
        self.last = None;
    }

    pub fn run(&mut self, input: &[f32]) -> Vec<f32> {
        let c = self.channels;
        let stereo = |f: &[f32]| if c == 1 { [f[0], f[0]] } else { [f[0], f[1]] };
        let mut frames: Vec<[f32; 2]> = Vec::with_capacity(input.len() / c + 1);
        frames.extend(self.last);
        frames.extend(input.chunks_exact(c).map(stereo));
        if frames.len() < 2 {
            self.last = frames.last().copied();
            return Vec::new();
        }
        let step = self.from as f64 / self.to as f64;
        let mut out = Vec::with_capacity(((frames.len() as f64 / step) as usize + 1) * 2);
        while self.pos + 1.0 < frames.len() as f64 {
            let i = self.pos as usize;
            let k = (self.pos - i as f64) as f32;
            let (a, b) = (frames[i], frames[i + 1]);
            out.push(a[0] + (b[0] - a[0]) * k);
            out.push(a[1] + (b[1] - a[1]) * k);
            self.pos += step;
        }
        // Keep the last frame; the position counts from it.
        self.pos -= (frames.len() - 1) as f64;
        self.last = frames.last().copied();
        out
    }
}

/// Whether `src` is a playlist or streaming manifest (HLS, DASH, Smooth
/// Streaming, SDP, M3U, PLS, ASX, XSPF) rather than media: decoders would
/// fetch the addresses it lists, which come from whoever made the document.
pub fn is_manifest(src: &Source) -> bool {
    let head: Vec<u8> = match src {
        Source::Bytes(b) => b.iter().take(4096).copied().collect(),
        Source::Path(p) => {
            use std::io::Read;
            let mut h = Vec::new();
            if let Ok(f) = std::fs::File::open(p) {
                let _ = f.take(4096).read_to_end(&mut h);
            }
            h
        }
    };
    // Media is binary from its first bytes; text anything decoders find
    // in it (a manifest behind a long comment) is no media of ours.
    if !head.is_empty() && std::str::from_utf8(&head).is_ok_and(|t| t.chars().all(|c| !c.is_control() || matches!(c, '\t' | '\r' | '\n'))) {
        return true;
    }
    // A text cut in the middle of a character.
    if head.len() == 4096 && head.iter().all(|&b| b >= 0x20 || matches!(b, b'\t' | b'\r' | b'\n')) {
        return true;
    }
    let text = String::from_utf8_lossy(&head).trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n']).to_ascii_lowercase();
    ["#extm3u", "#ext-x-", "v=0", "[playlist]", "<asx", "rtsp://", "http://", "https://"].iter().any(|m| text.starts_with(m))
        || (text.starts_with('<') && ["<mpd", "<smoothstreamingmedia", "<playlist", "<smil"].iter().any(|m| text.contains(m)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playlists_are_not_played() {
        let b = |t: &str| Source::Bytes(t.as_bytes().into());
        assert!(is_manifest(&b("\u{feff}#EXTM3U\n#EXT-X-STREAM-INF:\nhttp://x/a.m3u8")));
        assert!(is_manifest(&b("<?xml version=\"1.0\"?><MPD xmlns=\"urn:mpeg:dash\">")));
        assert!(!is_manifest(&Source::Bytes(vec![0, 0, 0, 0x20, b'f', b't', b'y', b'p'].into())));
        assert!(is_manifest(&b(&format!("<!--{}--><MPD xmlns=\"urn:mpeg:dash\">", "x".repeat(5000)))));
    }

    #[test]
    fn sizes_fit_and_are_even() {
        assert_eq!(fit((1920, 1080), 640), (640, 360));
        assert_eq!(fit((101, 51), 1000), (102, 52));
        assert_eq!(fit((0, 0), 10), (2, 2));
    }

    #[test]
    fn pictures_convert_and_scale() {
        // 2x1 BGRX with padded rows, to 1x1 and 4x2.
        let src = [1, 2, 3, 0, 4, 5, 6, 0, 9, 9];
        assert_eq!(to_rgba(&src, 2, 1, 10, true, (1, 1)), vec![3, 2, 1, 255]);
        let big = to_rgba(&src, 2, 1, 10, false, (4, 2));
        assert_eq!(&big[..16], &[1, 2, 3, 255, 1, 2, 3, 255, 4, 5, 6, 255, 4, 5, 6, 255]);
        assert_eq!(&big[16..20], &[1, 2, 3, 255]);
    }

    #[test]
    fn sound_resamples_to_stereo() {
        let mut r = Resampler::new(100, 200, 1);
        let a = r.run(&[0.0, 1.0]);
        assert_eq!(a, vec![0.0, 0.0, 0.5, 0.5]);
        // Goes on from the frame before.
        let b = r.run(&[0.0]);
        assert_eq!(b, vec![1.0, 1.0, 0.5, 0.5]);
        // The same rate passes through, in pieces.
        let mut r = Resampler::new(48_000, 48_000, 2);
        let mut all = r.run(&[1.0, 2.0, 3.0, 4.0]);
        all.extend(r.run(&[5.0, 6.0]));
        assert_eq!(all, vec![1.0, 2.0, 3.0, 4.0]);
        r.reset();
        assert!(r.run(&[7.0, 8.0]).is_empty());
    }
}
