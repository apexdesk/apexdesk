//! FFmpeg, an optional component loaded while running: the libraries of
//! a trimmed LGPL build (`tools/ffmpeg-build.sh`) in `components/ffmpeg`
//! next to the program, in `APEXDESK_FFMPEG` (their folder, or "off"),
//! or on Linux the system's. Only the few functions used are declared
//! here, and the few fields read are at their offsets in FFmpeg 7
//! (avformat 61, avcodec 61, avutil 59, swscale 8, swresample 5) on 64-bit
//! systems: other versions count as missing. Built only with the `ffmpeg`
//! feature (off by default: the system's decoders play media), and then
//! tried before them.
//!
//! A [`Decoder`] reads a file (in memory or on disk) and gives its video
//! frames as RGBA pictures and its sound as interleaved stereo samples.

use apex_fs::PathExt;
use std::ffi::{CString, c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use libloading::Library;

use super::decode::{Backend, Info, Out, Source};

type Ptr = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct Rational {
    num: c_int,
    den: c_int,
}

/// Field offsets in FFmpeg 7's structures (64-bit).
mod off {
    pub const FMT_NB_STREAMS: usize = 44;
    pub const FMT_STREAMS: usize = 48;
    pub const FMT_DURATION: usize = 104;
    pub const STREAM_CODECPAR: usize = 16;
    pub const STREAM_TIME_BASE: usize = 32;
    pub const STREAM_START: usize = 40;
    pub const STREAM_DISPOSITION: usize = 64;
    pub const PAR_WIDTH: usize = 72;
    pub const PAR_HEIGHT: usize = 76;
    pub const FRAME_DATA: usize = 0;
    pub const FRAME_LINESIZE: usize = 64;
    pub const FRAME_EXTENDED: usize = 96;
    pub const FRAME_WIDTH: usize = 104;
    pub const FRAME_HEIGHT: usize = 108;
    pub const FRAME_NB_SAMPLES: usize = 112;
    pub const FRAME_FORMAT: usize = 116;
    pub const FRAME_PTS: usize = 136;
    pub const FRAME_SAMPLE_RATE: usize = 192;
    pub const FRAME_BEST_TS: usize = 320;
    pub const FRAME_CH_LAYOUT: usize = 408;
    pub const PKT_STREAM_INDEX: usize = 36;
    pub const IO_BUFFER: usize = 8;
}

const MEDIA_VIDEO: c_int = 0;
const MEDIA_AUDIO: c_int = 1;
const PIX_RGBA: c_int = 26;
const SAMPLE_FLT: c_int = 3;
const SWS_BILINEAR: c_int = 2;
const SEEK_BACKWARD: c_int = 1;
const AVSEEK_SIZE: c_int = 0x10000;
const NOPTS: i64 = i64::MIN;
const DISPOSITION_ATTACHED_PIC: c_int = 1024;
const ERROR_EOF: c_int = -0x2046_4F45; // FFERRTAG('E','O','F',' ')
#[cfg(target_os = "macos")]
const EAGAIN: c_int = -35;
#[cfg(not(target_os = "macos"))]
const EAGAIN: c_int = -11;
/// Size of an `AVChannelLayout`.
const CH_LAYOUT_SIZE: usize = 24;
/// Bytes of the reading buffer of files in memory.
const IO_SIZE: usize = 64 * 1024;

type ReadFn = unsafe extern "C" fn(Ptr, *mut u8, c_int) -> c_int;
type SeekFn = unsafe extern "C" fn(Ptr, i64, c_int) -> i64;

/// The functions of the libraries.
#[allow(non_snake_case)]
struct Api {
    _libs: Vec<Library>,
    av_frame_alloc: unsafe extern "C" fn() -> Ptr,
    av_frame_free: unsafe extern "C" fn(*mut Ptr),
    av_malloc: unsafe extern "C" fn(usize) -> Ptr,
    av_dict_set: unsafe extern "C" fn(*mut Ptr, *const c_char, *const c_char, c_int) -> c_int,
    av_dict_free: unsafe extern "C" fn(*mut Ptr),
    av_freep: unsafe extern "C" fn(Ptr),
    av_opt_set_int: unsafe extern "C" fn(Ptr, *const c_char, i64, c_int) -> c_int,
    av_channel_layout_default: unsafe extern "C" fn(Ptr, c_int),
    av_log_set_level: unsafe extern "C" fn(c_int),
    avcodec_find_decoder: unsafe extern "C" fn(c_int) -> Ptr,
    avcodec_alloc_context3: unsafe extern "C" fn(Ptr) -> Ptr,
    avcodec_parameters_to_context: unsafe extern "C" fn(Ptr, Ptr) -> c_int,
    avcodec_open2: unsafe extern "C" fn(Ptr, Ptr, *mut Ptr) -> c_int,
    avcodec_send_packet: unsafe extern "C" fn(Ptr, Ptr) -> c_int,
    avcodec_receive_frame: unsafe extern "C" fn(Ptr, Ptr) -> c_int,
    avcodec_flush_buffers: unsafe extern "C" fn(Ptr),
    avcodec_free_context: unsafe extern "C" fn(*mut Ptr),
    av_packet_alloc: unsafe extern "C" fn() -> Ptr,
    av_packet_free: unsafe extern "C" fn(*mut Ptr),
    av_packet_unref: unsafe extern "C" fn(Ptr),
    avformat_alloc_context: unsafe extern "C" fn() -> Ptr,
    avformat_open_input: unsafe extern "C" fn(*mut Ptr, *const c_char, Ptr, *mut Ptr) -> c_int,
    avformat_find_stream_info: unsafe extern "C" fn(Ptr, *mut Ptr) -> c_int,
    av_read_frame: unsafe extern "C" fn(Ptr, Ptr) -> c_int,
    av_seek_frame: unsafe extern "C" fn(Ptr, c_int, i64, c_int) -> c_int,
    avformat_close_input: unsafe extern "C" fn(*mut Ptr),
    avio_alloc_context: unsafe extern "C" fn(*mut u8, c_int, c_int, Ptr, Option<ReadFn>, Ptr, Option<SeekFn>) -> Ptr,
    avio_context_free: unsafe extern "C" fn(*mut Ptr),
    sws_getCachedContext: unsafe extern "C" fn(Ptr, c_int, c_int, c_int, c_int, c_int, c_int, c_int, Ptr, Ptr, *const f64) -> Ptr,
    sws_scale: unsafe extern "C" fn(Ptr, *const *const u8, *const c_int, c_int, c_int, *const *mut u8, *const c_int) -> c_int,
    sws_freeContext: unsafe extern "C" fn(Ptr),
    swr_alloc_set_opts2: unsafe extern "C" fn(*mut Ptr, *const c_void, c_int, c_int, *const c_void, c_int, c_int, c_int, Ptr) -> c_int,
    swr_init: unsafe extern "C" fn(Ptr) -> c_int,
    swr_convert: unsafe extern "C" fn(Ptr, *const *mut u8, c_int, *const *const u8, c_int) -> c_int,
    swr_free: unsafe extern "C" fn(*mut Ptr),
}

// The functions are thread-safe on distinct objects.
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

/// The libraries and the major versions they must have, in the order
/// they load (each needs the ones before).
const LIBS: [(&str, u32); 5] = [("avutil", 59), ("swresample", 5), ("swscale", 8), ("avcodec", 61), ("avformat", 61)];

/// A library's file name here.
fn file_name(lib: &str, major: u32) -> String {
    if cfg!(windows) {
        format!("{lib}-{major}.dll")
    } else if cfg!(target_os = "macos") {
        format!("lib{lib}.{major}.dylib")
    } else {
        format!("lib{lib}.so.{major}")
    }
}

/// Folders with the libraries, most specific first.
fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os("APEXDESK_FFMPEG").map(PathBuf::from) {
        dirs.push(p.join("lib"));
        dirs.push(p.join("bin"));
        dirs.push(p);
    }
    for d in crate::portable::install_dirs() {
        for sub in ["components/ffmpeg/lib", "components/ffmpeg/bin", "components/ffmpeg"] {
            dirs.push(d.join(sub));
        }
    }
    if cfg!(target_os = "linux") {
        dirs.push(PathBuf::from("/usr/lib/apexdesk/components/ffmpeg/lib"));
    }
    // A build's own copy (target/ffmpeg/out/inst/lib beside target/debug);
    // installed, the folders above the program's may be anyone's (C:\).
    if cfg!(debug_assertions)
        && let Some(d) = crate::portable::program_dir()
    {
        dirs.extend(d.ancestors().skip(1).take(2).map(|p| p.join("ffmpeg/out/inst/lib")));
    }
    dirs
}

/// The folder holding the libraries, if any ("" for the system's).
pub fn library_dir() -> Option<PathBuf> {
    if std::env::var_os("APEXDESK_FFMPEG").is_some_and(|v| v == "off") {
        return None;
    }
    let (lib, major) = LIBS[4];
    let name = file_name(lib, major);
    candidate_dirs().into_iter().find(|d| d.join(&name).fs_is_file())
}

fn load_lib(dir: Option<&Path>, lib: &str, major: u32) -> Result<Library, String> {
    let name = file_name(lib, major);
    let path = dir.map_or_else(|| PathBuf::from(&name), |d| d.join(&name));
    #[cfg(windows)]
    let r = {
        use libloading::os::windows::{LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR};
        // Its own folder for what it needs, then the system's: never the
        // working folder (a document's, where anything can be).
        let flags = if dir.is_some() { LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS } else { LOAD_LIBRARY_SEARCH_DEFAULT_DIRS };
        unsafe { libloading::os::windows::Library::load_with_flags(&path, flags).map(Library::from) }
    };
    #[cfg(not(windows))]
    let r = unsafe { Library::new(&path) };
    r.map_err(|e| format!("{}: {e}", path.display()))
}

/// Loads the libraries from `dir` (none: the system's search path).
fn load_from(dir: Option<&Path>) -> Result<Api, String> {
    if !cfg!(target_pointer_width = "64") {
        return Err("FFmpeg: only on 64-bit systems".into());
    }
    let mut libs = Vec::new();
    for (lib, major) in LIBS {
        let l = load_lib(dir, lib, major)?;
        // The major version the offsets are for.
        let v: libloading::Symbol<unsafe extern "C" fn() -> u32> = unsafe { l.get(format!("{lib}_version\0").as_bytes()) }.map_err(|e| e.to_string())?;
        let got = unsafe { v() } >> 16;
        if got != major {
            return Err(format!("FFmpeg: {lib} {got}, {major} needed"));
        }
        libs.push(l);
    }
    macro_rules! sym {
        ($i:expr, $name:ident) => {{
            let s: libloading::Symbol<_> = unsafe { libs[$i].get(concat!(stringify!($name), "\0").as_bytes()) }.map_err(|e| format!("FFmpeg: {e}"))?;
            *s
        }};
    }
    let api = Api {
        av_frame_alloc: sym!(0, av_frame_alloc),
        av_frame_free: sym!(0, av_frame_free),
        av_malloc: sym!(0, av_malloc),
        av_dict_set: sym!(0, av_dict_set),
        av_dict_free: sym!(0, av_dict_free),
        av_freep: sym!(0, av_freep),
        av_opt_set_int: sym!(0, av_opt_set_int),
        av_channel_layout_default: sym!(0, av_channel_layout_default),
        av_log_set_level: sym!(0, av_log_set_level),
        swr_alloc_set_opts2: sym!(1, swr_alloc_set_opts2),
        swr_init: sym!(1, swr_init),
        swr_convert: sym!(1, swr_convert),
        swr_free: sym!(1, swr_free),
        sws_getCachedContext: sym!(2, sws_getCachedContext),
        sws_scale: sym!(2, sws_scale),
        sws_freeContext: sym!(2, sws_freeContext),
        avcodec_find_decoder: sym!(3, avcodec_find_decoder),
        avcodec_alloc_context3: sym!(3, avcodec_alloc_context3),
        avcodec_parameters_to_context: sym!(3, avcodec_parameters_to_context),
        avcodec_open2: sym!(3, avcodec_open2),
        avcodec_send_packet: sym!(3, avcodec_send_packet),
        avcodec_receive_frame: sym!(3, avcodec_receive_frame),
        avcodec_flush_buffers: sym!(3, avcodec_flush_buffers),
        avcodec_free_context: sym!(3, avcodec_free_context),
        av_packet_alloc: sym!(3, av_packet_alloc),
        av_packet_free: sym!(3, av_packet_free),
        av_packet_unref: sym!(3, av_packet_unref),
        avformat_alloc_context: sym!(4, avformat_alloc_context),
        avformat_open_input: sym!(4, avformat_open_input),
        avformat_find_stream_info: sym!(4, avformat_find_stream_info),
        av_read_frame: sym!(4, av_read_frame),
        av_seek_frame: sym!(4, av_seek_frame),
        avformat_close_input: sym!(4, avformat_close_input),
        avio_alloc_context: sym!(4, avio_alloc_context),
        avio_context_free: sym!(4, avio_context_free),
        _libs: Vec::new(),
    };
    // Quiet: errors of damaged files are reported by what they return.
    unsafe { (api.av_log_set_level)(-8) };
    Ok(Api { _libs: libs, ..api })
}

static API: OnceLock<Result<Api, String>> = OnceLock::new();

fn api() -> Result<&'static Api, String> {
    API.get_or_init(|| {
        if std::env::var_os("APEXDESK_FFMPEG").is_some_and(|v| v == "off") {
            return Err("FFmpeg: off".into());
        }
        match library_dir() {
            Some(d) => load_from(Some(&d)),
            // The system's (a distribution's package).
            None if cfg!(target_os = "linux") => load_from(None),
            None => Err("FFmpeg: not installed".into()),
        }
        .inspect_err(|e| crate::diag::warning(e))
    })
    .as_ref()
    .map_err(Clone::clone)
}

/// Loads FFmpeg, if it's there.
pub fn load() -> Result<(), String> {
    api().map(|_| ())
}

/// Opens `src` with FFmpeg.
pub fn open(src: &Source, want_video: bool) -> Result<Box<dyn Backend>, String> {
    Decoder::open(src, want_video).map(|d| Box::new(d) as Box<dyn Backend>)
}

impl Backend for Decoder {
    fn info(&self) -> Info {
        self.info
    }

    fn set_output(&mut self, size: Option<(usize, usize)>, rate: u32) {
        self.out_size = size;
        self.out_rate = rate;
    }

    fn seek(&mut self, t: f64) {
        Decoder::seek(self, t)
    }

    fn next(&mut self) -> Option<Out> {
        Decoder::next(self)
    }
}

unsafe fn rd<T: Copy>(p: *const c_void, at: usize) -> T {
    unsafe { std::ptr::read_unaligned((p as *const u8).add(at) as *const T) }
}

/// A file in memory, read through FFmpeg's callbacks.
struct MemFile {
    data: Arc<[u8]>,
    pos: usize,
}

unsafe extern "C" fn mem_read(opaque: Ptr, buf: *mut u8, size: c_int) -> c_int {
    let f = unsafe { &mut *(opaque as *mut MemFile) };
    let n = (size.max(0) as usize).min(f.data.len().saturating_sub(f.pos));
    if n == 0 {
        return ERROR_EOF;
    }
    unsafe { std::ptr::copy_nonoverlapping(f.data.as_ptr().add(f.pos), buf, n) };
    f.pos += n;
    n as c_int
}

unsafe extern "C" fn mem_seek(opaque: Ptr, offset: i64, whence: c_int) -> i64 {
    let f = unsafe { &mut *(opaque as *mut MemFile) };
    let len = f.data.len() as i64;
    if whence & AVSEEK_SIZE != 0 {
        return len;
    }
    let to = match whence & 3 {
        0 => offset,
        1 => f.pos as i64 + offset,
        2 => len + offset,
        _ => return -1,
    };
    if !(0..=len).contains(&to) {
        return -1;
    }
    f.pos = to as usize;
    to
}

/// A stream being decoded.
struct Stream {
    index: c_int,
    ctx: Ptr,
    tb: Rational,
    start: i64,
    /// Drained after the end.
    done: bool,
}

/// A file being decoded.
struct Decoder {
    api: &'static Api,
    fmt: Ptr,
    io: Ptr,
    mem: *mut MemFile,
    pkt: Ptr,
    frame: Ptr,
    video: Option<Stream>,
    audio: Option<Stream>,
    sws: Ptr,
    swr: Ptr,
    /// The format, rate and channel layout `swr` was made for.
    swr_for: (c_int, c_int, [u8; 16]),
    /// The size pictures are given at (none: their own).
    pub out_size: Option<(usize, usize)>,
    /// The sample rate sound is given at.
    pub out_rate: u32,
    eof: bool,
    info: Info,
}

// Used by one thread at a time.
unsafe impl Send for Decoder {}

impl Decoder {
    /// Opens a file and its video and sound streams (`want_video`: false
    /// to decode only the sound).
    pub fn open(src: &Source, want_video: bool) -> Result<Decoder, String> {
        let api = api()?;
        let mut d = Decoder { api, fmt: std::ptr::null_mut(), io: std::ptr::null_mut(), mem: std::ptr::null_mut(), pkt: std::ptr::null_mut(), frame: std::ptr::null_mut(), video: None, audio: None, sws: std::ptr::null_mut(), swr: std::ptr::null_mut(), swr_for: (0, 0, [0; 16]), out_size: None, out_rate: 48_000, eof: false, info: Info::default() };
        unsafe { d.open_inner(src, want_video)? };
        Ok(d)
    }

    unsafe fn open_inner(&mut self, src: &Source, want_video: bool) -> Result<(), String> {
        let a = self.api;
        unsafe {
            self.fmt = (a.avformat_alloc_context)();
            if self.fmt.is_null() {
                return Err("FFmpeg: out of memory".into());
            }
            let url = match src {
                Source::Bytes(data) => {
                    let buf = (a.av_malloc)(IO_SIZE) as *mut u8;
                    self.mem = Box::into_raw(Box::new(MemFile { data: data.clone(), pos: 0 }));
                    self.io = (a.avio_alloc_context)(buf, IO_SIZE as c_int, 0, self.mem as Ptr, Some(mem_read), std::ptr::null_mut(), Some(mem_seek));
                    if self.io.is_null() {
                        return Err("FFmpeg: out of memory".into());
                    }
                    // AVFormatContext.pb
                    std::ptr::write_unaligned((self.fmt as *mut u8).add(32) as *mut Ptr, self.io);
                    CString::default()
                }
                Source::Path(p) => CString::new(p.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?,
            };
            // Only the file itself (or the bytes given): a playlist or a
            // concat list in it may not send FFmpeg to the network or to
            // other files.
            let mut opts: Ptr = std::ptr::null_mut();
            let protocols = if matches!(src, Source::Bytes(_)) { c"" } else { c"file" };
            (a.av_dict_set)(&mut opts, c"protocol_whitelist".as_ptr(), protocols.as_ptr(), 0);
            let mut fmt = self.fmt;
            let r = (a.avformat_open_input)(&mut fmt, url.as_ptr(), std::ptr::null_mut(), &mut opts);
            (a.av_dict_free)(&mut opts);
            if r < 0 {
                // It freed the context.
                self.fmt = std::ptr::null_mut();
                return Err(format!("FFmpeg: can't open ({r})"));
            }
            self.fmt = fmt;
            if (a.avformat_find_stream_info)(self.fmt, std::ptr::null_mut()) < 0 {
                return Err("FFmpeg: no streams".into());
            }
            let n: u32 = rd(self.fmt, off::FMT_NB_STREAMS);
            let streams: *const Ptr = rd(self.fmt, off::FMT_STREAMS);
            for i in 0..n.min(64) as usize {
                let st = *streams.add(i);
                let par: Ptr = rd(st, off::STREAM_CODECPAR);
                let kind: c_int = rd(par, 0);
                let disposition: c_int = rd(st, off::STREAM_DISPOSITION);
                let slot = match kind {
                    MEDIA_VIDEO if want_video && self.video.is_none() && disposition & DISPOSITION_ATTACHED_PIC == 0 => &mut self.video,
                    MEDIA_AUDIO if self.audio.is_none() => &mut self.audio,
                    _ => continue,
                };
                let codec_id: c_int = rd(par, 4);
                let codec = (a.avcodec_find_decoder)(codec_id);
                if codec.is_null() {
                    continue;
                }
                let mut ctx = (a.avcodec_alloc_context3)(codec);
                if ctx.is_null() {
                    continue;
                }
                let threads = CString::new("threads").unwrap();
                if (a.avcodec_parameters_to_context)(ctx, par) < 0 || {
                    (a.av_opt_set_int)(ctx, threads.as_ptr(), 0, 0);
                    (a.avcodec_open2)(ctx, codec, std::ptr::null_mut()) < 0
                } {
                    (a.avcodec_free_context)(&mut ctx);
                    continue;
                }
                if kind == MEDIA_VIDEO {
                    let (w, h): (c_int, c_int) = (rd(par, off::PAR_WIDTH), rd(par, off::PAR_HEIGHT));
                    self.info.size = Some((w.max(1) as usize, h.max(1) as usize));
                } else {
                    self.info.audio = true;
                }
                *slot = Some(Stream { index: i as c_int, ctx, tb: rd(st, off::STREAM_TIME_BASE), start: rd(st, off::STREAM_START), done: false });
            }
            if self.video.is_none() && self.audio.is_none() {
                return Err("FFmpeg: nothing to play".into());
            }
            let dur: i64 = rd(self.fmt, off::FMT_DURATION);
            self.info.duration = if dur > 0 && dur != NOPTS { dur as f64 / 1_000_000.0 } else { 0.0 };
            self.pkt = (a.av_packet_alloc)();
            self.frame = (a.av_frame_alloc)();
            if self.pkt.is_null() || self.frame.is_null() {
                return Err("FFmpeg: out of memory".into());
            }
        }
        Ok(())
    }

    /// Goes to `t` seconds (the key frame before it: what comes before
    /// `t` is the caller's to skip).
    pub fn seek(&mut self, t: f64) {
        let a = self.api;
        unsafe {
            (a.av_seek_frame)(self.fmt, -1, (t.max(0.0) * 1_000_000.0) as i64, SEEK_BACKWARD);
            for s in [&mut self.video, &mut self.audio].into_iter().flatten() {
                (a.avcodec_flush_buffers)(s.ctx);
                s.done = false;
            }
        }
        self.eof = false;
    }

    /// The next picture or piece of sound; `None` at the end.
    pub fn next(&mut self) -> Option<Out> {
        let a = self.api;
        loop {
            for video in [true, false] {
                if let Some(o) = self.receive(video) {
                    return Some(o);
                }
            }
            if self.eof && [&self.video, &self.audio].into_iter().flatten().all(|s| s.done) {
                return None;
            }
            unsafe {
                let r = (a.av_read_frame)(self.fmt, self.pkt);
                if r < 0 {
                    if !self.eof {
                        // Flushes the decoders: their last frames come out.
                        self.eof = true;
                        for s in [&self.video, &self.audio].into_iter().flatten() {
                            (a.avcodec_send_packet)(s.ctx, std::ptr::null_mut());
                        }
                    } else {
                        for s in [&mut self.video, &mut self.audio].into_iter().flatten() {
                            s.done = true;
                        }
                    }
                    continue;
                }
                let idx: c_int = rd(self.pkt, off::PKT_STREAM_INDEX);
                for s in [&self.video, &self.audio].into_iter().flatten() {
                    if s.index == idx {
                        (a.avcodec_send_packet)(s.ctx, self.pkt);
                    }
                }
                (a.av_packet_unref)(self.pkt);
            }
        }
    }

    /// A frame of the video (or sound) stream, converted.
    fn receive(&mut self, video: bool) -> Option<Out> {
        let a = self.api;
        let s = if video { self.video.as_mut()? } else { self.audio.as_mut()? };
        if s.done {
            return None;
        }
        unsafe {
            let r = (a.avcodec_receive_frame)(s.ctx, self.frame);
            if r == ERROR_EOF {
                s.done = true;
                return None;
            }
            if r == EAGAIN || r < 0 {
                if r != EAGAIN && self.eof {
                    s.done = true;
                }
                return None;
            }
            let mut ts: i64 = rd(self.frame, off::FRAME_BEST_TS);
            if ts == NOPTS {
                ts = rd(self.frame, off::FRAME_PTS);
            }
            let start = if s.start == NOPTS { 0 } else { s.start };
            let t = if ts == NOPTS || s.tb.den == 0 { 0.0 } else { (ts - start) as f64 * s.tb.num as f64 / s.tb.den as f64 };
            let out = if video { self.picture(t) } else { self.sound(t) };
            // AVFrame is reused: av_frame_unref is done by receive_frame.
            out
        }
    }

    unsafe fn picture(&mut self, t: f64) -> Option<Out> {
        let a = self.api;
        unsafe {
            let (w, h, fmt): (c_int, c_int, c_int) = (rd(self.frame, off::FRAME_WIDTH), rd(self.frame, off::FRAME_HEIGHT), rd(self.frame, off::FRAME_FORMAT));
            if w <= 0 || h <= 0 {
                return None;
            }
            let (dw, dh) = self.out_size.unwrap_or((w as usize, h as usize));
            let (dw, dh) = (dw.clamp(2, 4096), dh.clamp(2, 4096));
            self.sws = (a.sws_getCachedContext)(self.sws, w, h, fmt, dw as c_int, dh as c_int, PIX_RGBA, SWS_BILINEAR, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null());
            if self.sws.is_null() {
                return None;
            }
            let mut rgba = vec![0u8; dw * dh * 4];
            let dst: [*mut u8; 4] = [rgba.as_mut_ptr(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()];
            let stride: [c_int; 4] = [(dw * 4) as c_int, 0, 0, 0];
            let data = (self.frame as *const u8).add(off::FRAME_DATA) as *const *const u8;
            let lines = (self.frame as *const u8).add(off::FRAME_LINESIZE) as *const c_int;
            (a.sws_scale)(self.sws, data, lines, 0, h, dst.as_ptr(), stride.as_ptr());
            Some(Out::Video { t, w: dw, h: dh, rgba })
        }
    }

    unsafe fn sound(&mut self, t: f64) -> Option<Out> {
        let a = self.api;
        unsafe {
            let n: c_int = rd(self.frame, off::FRAME_NB_SAMPLES);
            let rate: c_int = rd(self.frame, off::FRAME_SAMPLE_RATE);
            let fmt: c_int = rd(self.frame, off::FRAME_FORMAT);
            if n <= 0 || rate <= 0 {
                return None;
            }
            let layout = (self.frame as *const u8).add(off::FRAME_CH_LAYOUT) as *const c_void;
            // A stream may change its format, rate or channels midway: the
            // converter made for the first ones would read past the new
            // frame's buffers.
            let now_for = (fmt, rate, std::ptr::read_unaligned(layout as *const [u8; 16]));
            if !self.swr.is_null() && self.swr_for != now_for {
                (a.swr_free)(&mut self.swr);
                self.swr = std::ptr::null_mut();
            }
            if self.swr.is_null() {
                self.swr_for = now_for;
                let mut stereo = [0u8; CH_LAYOUT_SIZE];
                (a.av_channel_layout_default)(stereo.as_mut_ptr() as Ptr, 2);
                if (a.swr_alloc_set_opts2)(&mut self.swr, stereo.as_ptr() as *const c_void, SAMPLE_FLT, self.out_rate as c_int, layout, fmt, rate, 0, std::ptr::null_mut()) < 0 || (a.swr_init)(self.swr) < 0 {
                    (a.swr_free)(&mut self.swr);
                    self.swr = std::ptr::null_mut();
                    return None;
                }
            }
            let cap = (n as i64 * self.out_rate as i64 / rate as i64 + 256) as usize;
            let mut samples = vec![0f32; cap * 2];
            let out: [*mut u8; 1] = [samples.as_mut_ptr() as *mut u8];
            let input: *const *const u8 = rd(self.frame, off::FRAME_EXTENDED);
            let got = (a.swr_convert)(self.swr, out.as_ptr(), cap as c_int, input, n);
            if got <= 0 {
                return None;
            }
            samples.truncate(got as usize * 2);
            Some(Out::Audio { t, samples })
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        let a = self.api;
        unsafe {
            for s in [&mut self.video, &mut self.audio].into_iter().flatten() {
                (a.avcodec_free_context)(&mut s.ctx);
            }
            if !self.sws.is_null() {
                (a.sws_freeContext)(self.sws);
            }
            if !self.swr.is_null() {
                (a.swr_free)(&mut self.swr);
            }
            if !self.pkt.is_null() {
                (a.av_packet_free)(&mut self.pkt);
            }
            if !self.frame.is_null() {
                (a.av_frame_free)(&mut self.frame);
            }
            if !self.fmt.is_null() {
                (a.avformat_close_input)(&mut self.fmt);
            }
            if !self.io.is_null() {
                // The buffer may have been replaced: free the one it holds.
                (a.av_freep)((self.io as *mut u8).add(off::IO_BUFFER) as Ptr);
                (a.avio_context_free)(&mut self.io);
            }
            if !self.mem.is_null() {
                drop(Box::from_raw(self.mem));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_names() {
        let n = file_name("avcodec", 61);
        assert!(n.contains("avcodec") && n.contains("61"));
    }

    #[test]
    fn memory_file_reads_and_seeks() {
        let mut f = MemFile { data: Arc::from(&b"0123456789"[..]), pos: 0 };
        let p = &mut f as *mut MemFile as Ptr;
        let mut buf = [0u8; 4];
        unsafe {
            assert_eq!(mem_read(p, buf.as_mut_ptr(), 4), 4);
            assert_eq!(&buf, b"0123");
            assert_eq!(mem_seek(p, 0, AVSEEK_SIZE), 10);
            assert_eq!(mem_seek(p, -2, 2), 8);
            assert_eq!(mem_read(p, buf.as_mut_ptr(), 4), 2);
            assert_eq!(mem_read(p, buf.as_mut_ptr(), 4), ERROR_EOF);
            assert_eq!(mem_seek(p, 20, 0), -1);
        }
    }
}
