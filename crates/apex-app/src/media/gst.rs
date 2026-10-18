//! GStreamer, the system's media framework on Linux and the BSDs, loaded
//! while running (no linking): only the few functions used are declared
//! here. A file goes through `decodebin` (whatever plugins the system has)
//! to two app sinks: RGBA pictures at the size asked for and interleaved
//! stereo float sound at the rate asked for. They don't keep time: the
//! player pulls them in order and times them itself, as with FFmpeg.

use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicPtr, Ordering};
use web_time::{Duration, Instant};

use libloading::Library;

use super::decode::{Backend, Info, Out, Source};

type Ptr = *mut c_void;
type Gboolean = c_int;

const STATE_NULL: c_int = 1;
const STATE_PAUSED: c_int = 3;
const STATE_PLAYING: c_int = 4;
const CHANGE_FAILURE: c_int = 0;
const CHANGE_ASYNC: c_int = 2;
const FORMAT_TIME: c_int = 3;
const SEEK_FLUSH: c_int = 1;
const SEEK_ACCURATE: c_int = 2;
const MESSAGE_ERROR: c_int = 1 << 1;
const CLOCK_NONE: u64 = u64::MAX;
const SECOND: f64 = 1e9;

#[repr(C)]
struct GError {
    domain: u32,
    code: c_int,
    message: *mut c_char,
}

/// The start of a GstBuffer (GstMiniObject, then its own fields).
#[repr(C)]
struct GstBuffer {
    gtype: usize,
    refcount: c_int,
    lockstate: c_int,
    flags: c_uint,
    copy: Ptr,
    dispose: Ptr,
    free: Ptr,
    priv_uint: c_uint,
    priv_pointer: Ptr,
    pool: Ptr,
    pts: u64,
}

struct Api {
    gst_init_check: unsafe extern "C" fn(*mut c_int, *mut Ptr, *mut *mut GError) -> Gboolean,
    gst_pipeline_new: unsafe extern "C" fn(*const c_char) -> Ptr,
    gst_element_factory_make: unsafe extern "C" fn(*const c_char, *const c_char) -> Ptr,
    gst_element_factory_find: unsafe extern "C" fn(*const c_char) -> Ptr,
    gst_parse_bin_from_description: unsafe extern "C" fn(*const c_char, Gboolean, *mut *mut GError) -> Ptr,
    gst_bin_add: unsafe extern "C" fn(Ptr, Ptr) -> Gboolean,
    gst_bin_get_by_name: unsafe extern "C" fn(Ptr, *const c_char) -> Ptr,
    gst_element_link: unsafe extern "C" fn(Ptr, Ptr) -> Gboolean,
    gst_element_set_state: unsafe extern "C" fn(Ptr, c_int) -> c_int,
    gst_element_get_state: unsafe extern "C" fn(Ptr, *mut c_int, *mut c_int, u64) -> c_int,
    gst_element_sync_state_with_parent: unsafe extern "C" fn(Ptr) -> Gboolean,
    gst_element_get_static_pad: unsafe extern "C" fn(Ptr, *const c_char) -> Ptr,
    gst_element_query_duration: unsafe extern "C" fn(Ptr, c_int, *mut i64) -> Gboolean,
    gst_element_seek_simple: unsafe extern "C" fn(Ptr, c_int, c_int, i64) -> Gboolean,
    gst_element_get_bus: unsafe extern "C" fn(Ptr) -> Ptr,
    gst_bus_timed_pop_filtered: unsafe extern "C" fn(Ptr, u64, c_int) -> Ptr,
    gst_message_parse_error: unsafe extern "C" fn(Ptr, *mut *mut GError, *mut *mut c_char),
    gst_pad_link: unsafe extern "C" fn(Ptr, Ptr) -> c_int,
    gst_pad_is_linked: unsafe extern "C" fn(Ptr) -> Gboolean,
    gst_pad_get_current_caps: unsafe extern "C" fn(Ptr) -> Ptr,
    gst_pad_query_caps: unsafe extern "C" fn(Ptr, Ptr) -> Ptr,
    gst_caps_get_structure: unsafe extern "C" fn(Ptr, c_uint) -> Ptr,
    gst_structure_get_name: unsafe extern "C" fn(Ptr) -> *const c_char,
    gst_structure_get_int: unsafe extern "C" fn(Ptr, *const c_char, *mut c_int) -> Gboolean,
    gst_sample_get_buffer: unsafe extern "C" fn(Ptr) -> *mut GstBuffer,
    gst_sample_get_caps: unsafe extern "C" fn(Ptr) -> Ptr,
    gst_sample_get_segment: unsafe extern "C" fn(Ptr) -> Ptr,
    gst_segment_to_stream_time: unsafe extern "C" fn(Ptr, c_int, u64) -> u64,
    gst_buffer_get_size: unsafe extern "C" fn(*mut GstBuffer) -> usize,
    gst_buffer_extract: unsafe extern "C" fn(*mut GstBuffer, usize, Ptr, usize) -> usize,
    gst_util_set_object_arg: unsafe extern "C" fn(Ptr, *const c_char, *const c_char),
    gst_mini_object_unref: unsafe extern "C" fn(Ptr),
    gst_object_unref: unsafe extern "C" fn(Ptr),
    gst_element_factory_get_metadata: unsafe extern "C" fn(Ptr, *const c_char) -> *const c_char,
    gst_object_get_name: unsafe extern "C" fn(Ptr) -> *mut c_char,
    gst_app_sink_try_pull_sample: unsafe extern "C" fn(Ptr, u64) -> Ptr,
    gst_app_sink_is_eos: unsafe extern "C" fn(Ptr) -> Gboolean,
    g_object_set: unsafe extern "C" fn(Ptr, *const c_char, ...),
    g_object_unref: unsafe extern "C" fn(Ptr),
    g_signal_connect_data: unsafe extern "C" fn(Ptr, *const c_char, Ptr, Ptr, Ptr, c_int) -> std::ffi::c_ulong,
    g_memory_input_stream_new_from_data: unsafe extern "C" fn(*const c_void, isize, Ptr) -> Ptr,
    g_error_free: unsafe extern "C" fn(*mut GError),
    g_free: unsafe extern "C" fn(Ptr),
    _libs: Vec<Library>,
}

fn load_api() -> Result<Api, String> {
    let open = |name: &str| unsafe { Library::new(name) }.map_err(|e| format!("{name}: {e}"));
    let libs = vec![open("libgstreamer-1.0.so.0")?, open("libgstapp-1.0.so.0")?, open("libgobject-2.0.so.0")?, open("libgio-2.0.so.0")?, open("libglib-2.0.so.0")?];
    macro_rules! sym {
        ($lib:expr, $name:ident) => {
            *unsafe { libs[$lib].get(concat!(stringify!($name), "\0").as_bytes()) }.map_err(|e| format!("{}: {e}", stringify!($name)))?
        };
    }
    Ok(Api {
        gst_init_check: sym!(0, gst_init_check),
        gst_pipeline_new: sym!(0, gst_pipeline_new),
        gst_element_factory_make: sym!(0, gst_element_factory_make),
        gst_element_factory_find: sym!(0, gst_element_factory_find),
        gst_parse_bin_from_description: sym!(0, gst_parse_bin_from_description),
        gst_bin_add: sym!(0, gst_bin_add),
        gst_bin_get_by_name: sym!(0, gst_bin_get_by_name),
        gst_element_link: sym!(0, gst_element_link),
        gst_element_set_state: sym!(0, gst_element_set_state),
        gst_element_get_state: sym!(0, gst_element_get_state),
        gst_element_sync_state_with_parent: sym!(0, gst_element_sync_state_with_parent),
        gst_element_get_static_pad: sym!(0, gst_element_get_static_pad),
        gst_element_query_duration: sym!(0, gst_element_query_duration),
        gst_element_seek_simple: sym!(0, gst_element_seek_simple),
        gst_element_get_bus: sym!(0, gst_element_get_bus),
        gst_bus_timed_pop_filtered: sym!(0, gst_bus_timed_pop_filtered),
        gst_message_parse_error: sym!(0, gst_message_parse_error),
        gst_pad_link: sym!(0, gst_pad_link),
        gst_pad_is_linked: sym!(0, gst_pad_is_linked),
        gst_pad_get_current_caps: sym!(0, gst_pad_get_current_caps),
        gst_pad_query_caps: sym!(0, gst_pad_query_caps),
        gst_caps_get_structure: sym!(0, gst_caps_get_structure),
        gst_structure_get_name: sym!(0, gst_structure_get_name),
        gst_structure_get_int: sym!(0, gst_structure_get_int),
        gst_sample_get_buffer: sym!(0, gst_sample_get_buffer),
        gst_sample_get_caps: sym!(0, gst_sample_get_caps),
        gst_sample_get_segment: sym!(0, gst_sample_get_segment),
        gst_segment_to_stream_time: sym!(0, gst_segment_to_stream_time),
        gst_buffer_get_size: sym!(0, gst_buffer_get_size),
        gst_buffer_extract: sym!(0, gst_buffer_extract),
        gst_util_set_object_arg: sym!(0, gst_util_set_object_arg),
        gst_mini_object_unref: sym!(0, gst_mini_object_unref),
        gst_object_unref: sym!(0, gst_object_unref),
        gst_element_factory_get_metadata: sym!(0, gst_element_factory_get_metadata),
        gst_object_get_name: sym!(0, gst_object_get_name),
        gst_app_sink_try_pull_sample: sym!(1, gst_app_sink_try_pull_sample),
        gst_app_sink_is_eos: sym!(1, gst_app_sink_is_eos),
        g_object_set: sym!(2, g_object_set),
        g_object_unref: sym!(2, g_object_unref),
        g_signal_connect_data: sym!(2, g_signal_connect_data),
        g_memory_input_stream_new_from_data: sym!(3, g_memory_input_stream_new_from_data),
        g_error_free: sym!(4, g_error_free),
        g_free: sym!(4, g_free),
        _libs: Vec::new(),
    })
    .map(|api| Api { _libs: libs, ..api })
}

static API: OnceLock<Result<Api, String>> = OnceLock::new();

fn api() -> Result<&'static Api, String> {
    API.get_or_init(|| {
        if !cfg!(target_pointer_width = "64") {
            return Err("64-bit systems only".into());
        }
        let a = load_api()?;
        unsafe {
            let mut err = std::ptr::null_mut();
            if (a.gst_init_check)(std::ptr::null_mut(), std::ptr::null_mut(), &mut err) == 0 {
                return Err(take_error(&a, err));
            }
            // The elements used, from the base plugins.
            for name in ["decodebin", "appsink", "videoconvert", "videoscale", "audioconvert", "audioresample", "giostreamsrc"] {
                let f = (a.gst_element_factory_find)(cs(name).as_ptr());
                if f.is_null() {
                    return Err(format!("no {name} element (GStreamer's base plugins)"));
                }
                (a.gst_object_unref)(f);
            }
        }
        Ok(a)
    })
    .as_ref()
    .map_err(Clone::clone)
}

/// Loads GStreamer and checks it has what's needed.
pub fn load() -> Result<(), String> {
    api().map(|_| ())
}

fn cs(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
}

/// A GError's text, freeing it.
unsafe fn take_error(a: &Api, err: *mut GError) -> String {
    if err.is_null() {
        return "unknown error".into();
    }
    unsafe {
        let text = if (*err).message.is_null() { String::new() } else { CStr::from_ptr((*err).message).to_string_lossy().into_owned() };
        (a.g_error_free)(err);
        text
    }
}

/// The name of a caps' first structure ("video/x-raw") and its size.
unsafe fn caps_kind(a: &Api, caps: Ptr) -> (String, Option<(usize, usize)>) {
    unsafe {
        let s = (a.gst_caps_get_structure)(caps, 0);
        if s.is_null() {
            return (String::new(), None);
        }
        let name = (a.gst_structure_get_name)(s);
        let name = if name.is_null() { String::new() } else { CStr::from_ptr(name).to_string_lossy().into_owned() };
        let (mut w, mut h) = (0, 0);
        let size = ((a.gst_structure_get_int)(s, c"width".as_ptr(), &mut w) != 0 && (a.gst_structure_get_int)(s, c"height".as_ptr(), &mut h) != 0).then_some((w.max(1) as usize, h.max(1) as usize));
        (name, size)
    }
}

const VIDEO_CAPS: &str = "video/x-raw,format=RGBA,pixel-aspect-ratio=1/1";
#[cfg(target_endian = "little")]
const AUDIO_CAPS: &str = "audio/x-raw,format=F32LE,layout=interleaved,channels=2";
#[cfg(target_endian = "big")]
const AUDIO_CAPS: &str = "audio/x-raw,format=F32BE,layout=interleaved,channels=2";

/// What decodebin's new pads are linked to, shared with its callback.
struct Links {
    api: &'static Api,
    pipeline: Ptr,
    want_video: bool,
    /// The app sinks and their caps filters, once linked.
    video: AtomicPtr<c_void>,
    video_caps: AtomicPtr<c_void>,
    audio: AtomicPtr<c_void>,
    audio_caps: AtomicPtr<c_void>,
}

/// Keeps decodebin off streaming playlists (HLS, DASH, Smooth Streaming)
/// and sources: their elements fetch whatever addresses the media names,
/// and a document's media mustn't reach the network.
unsafe extern "C" fn autoplug_select(_bin: Ptr, _pad: Ptr, _caps: Ptr, factory: Ptr, data: Ptr) -> c_int {
    const TRY: c_int = 0;
    const SKIP: c_int = 2;
    let l = unsafe { &*(data as *const Links) };
    let text = |p: *const c_char| if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().to_ascii_lowercase() };
    let klass = text(unsafe { (l.api.gst_element_factory_get_metadata)(factory, c"klass".as_ptr()) });
    // (`gst_plugin_feature_get_name` is a macro, not in the library.)
    let name = unsafe { (l.api.gst_object_get_name)(factory) };
    let name = {
        let t = text(name);
        if !name.is_null() {
            unsafe { (l.api.g_free)(name as Ptr) };
        }
        t
    };
    let net = klass.contains("adaptive") || klass.contains("source") || klass.contains("network") || ["hls", "dash", "mss", "playlist", "souphttp", "curl", "rtsp", "rtmp", "srt", "udp", "tcp"].iter().any(|n| name.contains(n));
    if net { SKIP } else { TRY }
}

/// A branch for a new pad of decodebin: into the pipeline, playing along.
unsafe fn add_branch(l: &Links, pad: Ptr, desc: &str) -> Option<(Ptr, Ptr)> {
    let a = l.api;
    unsafe {
        let mut err = std::ptr::null_mut();
        let bin = (a.gst_parse_bin_from_description)(cs(desc).as_ptr(), 1, &mut err);
        if bin.is_null() {
            crate::diag::warning(&format!("GStreamer: {}", take_error(a, err)));
            return None;
        }
        (a.gst_bin_add)(l.pipeline, bin);
        (a.gst_element_sync_state_with_parent)(bin);
        let sink_pad = (a.gst_element_get_static_pad)(bin, c"sink".as_ptr());
        if sink_pad.is_null() {
            return None;
        }
        let r = (a.gst_pad_link)(pad, sink_pad);
        (a.gst_object_unref)(sink_pad);
        if r != 0 {
            return None;
        }
        let sink = (a.gst_bin_get_by_name)(bin, c"sink".as_ptr());
        let caps = (a.gst_bin_get_by_name)(bin, c"caps".as_ptr());
        Some((sink, caps))
    }
}

unsafe extern "C" fn pad_added(_dec: Ptr, pad: Ptr, data: Ptr) {
    let l = unsafe { &*(data as *const Links) };
    let a = l.api;
    unsafe {
        let mut caps = (a.gst_pad_get_current_caps)(pad);
        if caps.is_null() {
            caps = (a.gst_pad_query_caps)(pad, std::ptr::null_mut());
        }
        if caps.is_null() {
            return;
        }
        let (kind, _) = caps_kind(a, caps);
        (a.gst_mini_object_unref)(caps);
        let (slot, caps_slot, desc) = if kind.starts_with("video/") && l.want_video && l.video.load(Ordering::Acquire).is_null() {
            (&l.video, &l.video_caps, format!("videoconvert ! videoscale ! capsfilter name=caps caps=\"{VIDEO_CAPS}\" ! appsink name=sink sync=false max-buffers=4 enable-last-sample=false"))
        } else if kind.starts_with("audio/") && l.audio.load(Ordering::Acquire).is_null() {
            (&l.audio, &l.audio_caps, format!("audioconvert ! audioresample ! capsfilter name=caps caps=\"{AUDIO_CAPS}\" ! appsink name=sink sync=false max-buffers=64 enable-last-sample=false"))
        } else {
            // Not wanted: taken and dropped, so the rest flows.
            if (a.gst_pad_is_linked)(pad) == 0 {
                add_branch(l, pad, "fakesink sync=false");
            }
            return;
        };
        if let Some((sink, caps)) = add_branch(l, pad, &desc) {
            caps_slot.store(caps, Ordering::Release);
            slot.store(sink, Ordering::Release);
        }
    }
}

/// A file being decoded by a GStreamer pipeline.
struct Gst {
    api: &'static Api,
    pipeline: Ptr,
    links: Box<Links>,
    /// Kept while the pipeline reads it.
    _data: Option<std::sync::Arc<[u8]>>,
    info: Info,
    /// Playing since the first seek.
    started: bool,
    /// Pulled and not yet given, and whether each sink ended.
    video: Option<Out>,
    audio: Option<Out>,
    video_done: bool,
    audio_done: bool,
}

// Used by one thread at a time; GStreamer's objects are thread-safe.
unsafe impl Send for Gst {}

/// Opens `src` with GStreamer.
pub fn open(src: &Source, want_video: bool) -> Result<Box<dyn Backend>, String> {
    let a = api()?;
    unsafe {
        let pipeline = (a.gst_pipeline_new)(std::ptr::null());
        if pipeline.is_null() {
            return Err("can't make a pipeline".into());
        }
        let links = Box::new(Links { api: a, pipeline, want_video, video: AtomicPtr::default(), video_caps: AtomicPtr::default(), audio: AtomicPtr::default(), audio_caps: AtomicPtr::default() });
        let mut g = Gst { api: a, pipeline, links, _data: None, info: Info::default(), started: false, video: None, audio: None, video_done: false, audio_done: false };
        let source = match src {
            Source::Path(p) => {
                let e = (a.gst_element_factory_make)(c"filesrc".as_ptr(), std::ptr::null());
                if !e.is_null() {
                    let path = CString::new(p.as_os_str().as_encoded_bytes()).map_err(|e| e.to_string())?;
                    (a.g_object_set)(e, c"location".as_ptr(), path.as_ptr(), std::ptr::null::<c_char>());
                }
                e
            }
            Source::Bytes(data) => {
                // A seekable stream over the bytes, which stay alive in `g`.
                let e = (a.gst_element_factory_make)(c"giostreamsrc".as_ptr(), std::ptr::null());
                if !e.is_null() {
                    let stream = (a.g_memory_input_stream_new_from_data)(data.as_ptr() as *const c_void, data.len() as isize, std::ptr::null_mut());
                    (a.g_object_set)(e, c"stream".as_ptr(), stream, std::ptr::null::<c_char>());
                    (a.g_object_unref)(stream);
                }
                g._data = Some(data.clone());
                e
            }
        };
        let decode = (a.gst_element_factory_make)(c"decodebin".as_ptr(), std::ptr::null());
        if source.is_null() || decode.is_null() {
            return Err("missing elements".into());
        }
        (a.gst_bin_add)(pipeline, source);
        (a.gst_bin_add)(pipeline, decode);
        if (a.gst_element_link)(source, decode) == 0 {
            return Err("can't link the source".into());
        }
        let cb: unsafe extern "C" fn(Ptr, Ptr, Ptr) = pad_added;
        let select: unsafe extern "C" fn(Ptr, Ptr, Ptr, Ptr, Ptr) -> c_int = autoplug_select;
        (a.g_signal_connect_data)(decode, c"autoplug-select".as_ptr(), select as Ptr, &*g.links as *const Links as Ptr, std::ptr::null_mut(), 0);
        (a.g_signal_connect_data)(decode, c"pad-added".as_ptr(), cb as Ptr, &*g.links as *const Links as Ptr, std::ptr::null_mut(), 0);
        // Ready (paused on the first picture and sound): what it holds.
        g.wait_state(STATE_PAUSED)?;
        let vsink = g.links.video.load(Ordering::Acquire);
        let asink = g.links.audio.load(Ordering::Acquire);
        if vsink.is_null() && asink.is_null() {
            return Err("nothing to play".into());
        }
        if !vsink.is_null() {
            let pad = (a.gst_element_get_static_pad)(vsink, c"sink".as_ptr());
            if !pad.is_null() {
                let caps = (a.gst_pad_get_current_caps)(pad);
                if !caps.is_null() {
                    g.info.size = caps_kind(a, caps).1;
                    (a.gst_mini_object_unref)(caps);
                }
                (a.gst_object_unref)(pad);
            }
            g.info.size = g.info.size.or(Some((16, 16)));
        }
        g.info.audio = !asink.is_null();
        let mut dur = 0i64;
        if (a.gst_element_query_duration)(pipeline, FORMAT_TIME, &mut dur) != 0 && dur > 0 {
            g.info.duration = dur as f64 / SECOND;
        }
        Ok(Box::new(g))
    }
}

impl Gst {
    /// Goes to `state`, waiting until it's there; errors say why not.
    unsafe fn wait_state(&self, state: c_int) -> Result<(), String> {
        let a = self.api;
        unsafe {
            let mut r = (a.gst_element_set_state)(self.pipeline, state);
            if r == CHANGE_ASYNC {
                r = (a.gst_element_get_state)(self.pipeline, std::ptr::null_mut(), std::ptr::null_mut(), 10 * SECOND as u64);
            }
            match r {
                CHANGE_FAILURE => Err(self.error().unwrap_or_else(|| "can't play".into())),
                CHANGE_ASYNC => Err("timed out".into()),
                _ => Ok(()),
            }
        }
    }

    /// The pipeline's first error, if any.
    fn error(&self) -> Option<String> {
        let a = self.api;
        unsafe {
            let bus = (a.gst_element_get_bus)(self.pipeline);
            if bus.is_null() {
                return None;
            }
            let msg = (a.gst_bus_timed_pop_filtered)(bus, 0, MESSAGE_ERROR);
            (a.gst_object_unref)(bus);
            if msg.is_null() {
                return None;
            }
            let (mut err, mut debug) = (std::ptr::null_mut(), std::ptr::null_mut());
            (a.gst_message_parse_error)(msg, &mut err, &mut debug);
            (a.g_free)(debug as Ptr);
            (a.gst_mini_object_unref)(msg);
            Some(take_error(a, err))
        }
    }

    /// The next sample of `sink` as what it holds; none at its end.
    fn pull(&self, sink: Ptr, video: bool) -> Option<Out> {
        let a = self.api;
        let since = Instant::now();
        unsafe {
            loop {
                let sample = (a.gst_app_sink_try_pull_sample)(sink, 50_000_000);
                if sample.is_null() {
                    // Ended, failed or stuck.
                    if (a.gst_app_sink_is_eos)(sink) != 0 || since.elapsed() > Duration::from_secs(5) {
                        return None;
                    }
                    if let Some(e) = self.error() {
                        crate::diag::warning(&format!("GStreamer: {e}"));
                        return None;
                    }
                    continue;
                }
                let out = self.sample(sample, video);
                (a.gst_mini_object_unref)(sample);
                if out.is_some() {
                    return out;
                }
            }
        }
    }

    unsafe fn sample(&self, sample: Ptr, video: bool) -> Option<Out> {
        let a = self.api;
        unsafe {
            let buf = (a.gst_sample_get_buffer)(sample);
            if buf.is_null() {
                return None;
            }
            let pts = (*buf).pts;
            let seg = (a.gst_sample_get_segment)(sample);
            let st = if pts == CLOCK_NONE || seg.is_null() { pts } else { (a.gst_segment_to_stream_time)(seg, FORMAT_TIME, pts) };
            let t = if st == CLOCK_NONE { if pts == CLOCK_NONE { 0.0 } else { pts as f64 / SECOND } } else { st as f64 / SECOND };
            let n = (a.gst_buffer_get_size)(buf);
            let mut bytes = vec![0u8; n];
            (a.gst_buffer_extract)(buf, 0, bytes.as_mut_ptr() as Ptr, n);
            if video {
                let caps = (a.gst_sample_get_caps)(sample);
                let (w, h) = (if caps.is_null() { None } else { caps_kind(a, caps).1 })?;
                if bytes.len() < w * h * 4 {
                    return None;
                }
                // Rows are 4-byte aligned already: RGBA packs.
                bytes.truncate(w * h * 4);
                Some(Out::Video { t, w, h, rgba: bytes })
            } else {
                let samples = bytes.chunks_exact(4).map(|b| f32::from_ne_bytes([b[0], b[1], b[2], b[3]])).collect();
                Some(Out::Audio { t, samples })
            }
        }
    }
}

impl Backend for Gst {
    fn info(&self) -> Info {
        self.info
    }

    fn set_output(&mut self, size: Option<(usize, usize)>, rate: u32) {
        let a = self.api;
        let set = |filter: Ptr, caps: String| {
            if !filter.is_null() {
                unsafe { (a.gst_util_set_object_arg)(filter, c"caps".as_ptr(), cs(&caps).as_ptr()) };
            }
        };
        let size = size.map_or(String::new(), |(w, h)| format!(",width={w},height={h}"));
        set(self.links.video_caps.load(Ordering::Acquire), format!("{VIDEO_CAPS}{size}"));
        set(self.links.audio_caps.load(Ordering::Acquire), format!("{AUDIO_CAPS},rate={}", rate.max(1)));
    }

    fn seek(&mut self, t: f64) {
        let a = self.api;
        self.video = None;
        self.audio = None;
        self.video_done = false;
        self.audio_done = false;
        unsafe {
            // Flushing: what was decoded before goes, the new sizes apply.
            (a.gst_element_seek_simple)(self.pipeline, FORMAT_TIME, SEEK_FLUSH | SEEK_ACCURATE, (t * SECOND) as i64);
            if !self.started {
                self.started = true;
                if let Err(e) = self.wait_state(STATE_PLAYING) {
                    crate::diag::warning(&format!("GStreamer: {e}"));
                }
            }
        }
    }

    fn next(&mut self) -> Option<Out> {
        if !self.started {
            self.seek(0.0);
        }
        let vsink = self.links.video.load(Ordering::Acquire);
        let asink = self.links.audio.load(Ordering::Acquire);
        if self.video.is_none() && !self.video_done && !vsink.is_null() {
            self.video = self.pull(vsink, true);
            self.video_done = self.video.is_none();
        }
        if self.audio.is_none() && !self.audio_done && !asink.is_null() {
            self.audio = self.pull(asink, false);
            self.audio_done = self.audio.is_none();
        }
        let time = |o: &Option<Out>| match o {
            Some(Out::Video { t, .. }) | Some(Out::Audio { t, .. }) => *t,
            None => f64::INFINITY,
        };
        // The earlier first.
        if time(&self.video) <= time(&self.audio) { self.video.take() } else { self.audio.take() }
    }
}

impl Drop for Gst {
    fn drop(&mut self) {
        let a = self.api;
        unsafe {
            // Stops its threads before the callback's data goes.
            (a.gst_element_set_state)(self.pipeline, STATE_NULL);
            for p in [&self.links.video, &self.links.video_caps, &self.links.audio, &self.links.audio_caps] {
                let p = p.swap(std::ptr::null_mut(), Ordering::AcqRel);
                if !p.is_null() {
                    (a.gst_object_unref)(p);
                }
            }
            (a.gst_object_unref)(self.pipeline);
        }
    }
}
