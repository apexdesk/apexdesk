//! The system's voices, for Read Aloud. Nothing is linked: the program
//! starts whether or not the computer can speak, and the engine is looked
//! for the first time it's needed.
//!
//! - Linux and BSD: Speech Dispatcher's library, loaded at run time (with
//!   a mark before each word, for highlighting), else `espeak-ng` or
//!   `espeak` as programs.
//! - macOS: the `say` program.
//! - Windows: System.Speech (SAPI) through a small PowerShell helper that
//!   reports each word.

use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Mutex, OnceLock};

use eframe::egui;

/// A voice of the system.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Voice {
    /// What the engine calls it.
    pub id: String,
    pub name: String,
    /// "es-ES" or "es" (may be empty).
    pub lang: String,
}

/// What the engine reports about the utterance `utt`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Speaking the word at byte `start` of the text, `len` bytes long.
    Word { utt: u64, start: usize, len: usize },
    /// Finished (not after `stop`).
    End { utt: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    NotStarted,
    Loading,
    Ready,
    /// No engine: why, for the user.
    Missing(String),
}

trait Backend: Send {
    fn voices(&self) -> Vec<Voice>;
    /// Speaks `text` (stopping what was being said); `rate` 1.0 is normal.
    fn speak(&mut self, utt: u64, text: &str, voice: Option<&Voice>, lang: &str, rate: f32) -> bool;
    fn stop(&mut self);
    fn pause(&mut self);
    fn resume(&mut self);
}

/// Says nothing and is done at once (for tests).
struct Silent;

impl Backend for Silent {
    fn voices(&self) -> Vec<Voice> {
        Vec::new()
    }
    fn speak(&mut self, utt: u64, _: &str, _: Option<&Voice>, _: &str, _: f32) -> bool {
        send(Event::End { utt });
        true
    }
    fn stop(&mut self) {}
    fn pause(&mut self) {}
    fn resume(&mut self) {}
}

enum Slot {
    NotStarted,
    Loading,
    Ready(Box<dyn Backend>),
    Missing(String),
}

static SLOT: Mutex<Slot> = Mutex::new(Slot::NotStarted);
static CHANNEL: OnceLock<(Sender<Event>, Mutex<Receiver<Event>>)> = OnceLock::new();
static CTX: OnceLock<egui::Context> = OnceLock::new();

fn channel() -> &'static (Sender<Event>, Mutex<Receiver<Event>>) {
    CHANNEL.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        (tx, Mutex::new(rx))
    })
}

/// Reports an event and wakes the interface.
fn send(e: Event) {
    let _ = channel().0.send(e);
    if let Some(c) = CTX.get() {
        c.request_repaint();
    }
}

/// Tests and scripted checks speak without sound.
fn muted() -> bool {
    std::env::var_os("APEXDESK_SPEECH_MUTE").is_some()
}

/// Looks for an engine in the background (once).
pub fn start(ctx: &egui::Context) {
    let _ = CTX.set(ctx.clone());
    let Ok(mut s) = SLOT.lock() else { return };
    if !matches!(*s, Slot::NotStarted) {
        return;
    }
    *s = Slot::Loading;
    drop(s);
    apex_fs::thread::spawn(|| {
        // Tests start no program: a silent engine stands in.
        let found = if cfg!(test) { Ok(Box::new(Silent) as Box<dyn Backend>) } else { find_backend() };
        if let Ok(mut s) = SLOT.lock() {
            *s = match found {
                Ok(b) => Slot::Ready(b),
                Err(why) => Slot::Missing(why),
            };
        }
        if let Some(c) = CTX.get() {
            c.request_repaint();
        }
    });
}

pub fn status() -> Status {
    match SLOT.lock().as_deref() {
        Ok(Slot::NotStarted) => Status::NotStarted,
        Ok(Slot::Loading) => Status::Loading,
        Ok(Slot::Ready(_)) => Status::Ready,
        Ok(Slot::Missing(w)) => Status::Missing(w.clone()),
        Err(_) => Status::Missing(String::new()),
    }
}

fn with<R>(f: impl FnOnce(&mut dyn Backend) -> R) -> Option<R> {
    let mut s = SLOT.lock().ok()?;
    match &mut *s {
        Slot::Ready(b) => Some(f(b.as_mut())),
        _ => None,
    }
}

pub fn voices() -> Vec<Voice> {
    with(|b| b.voices()).unwrap_or_default()
}

/// Speaks `text` as utterance `utt` with the voice `voice_id` (empty: one
/// for `lang`).
pub fn speak(utt: u64, text: &str, lang: &str, voice_id: &str, rate: f32) -> bool {
    with(|b| {
        let voices = b.voices();
        let voice = if voice_id.is_empty() { pick(&voices, lang) } else { voices.iter().find(|v| v.id == voice_id) };
        b.speak(utt, text, voice, lang, rate.clamp(0.5, 2.0))
    })
    .unwrap_or(false)
}

pub fn stop() {
    with(|b| b.stop());
}

pub fn pause() {
    with(|b| b.pause());
}

pub fn resume() {
    with(|b| b.resume());
}

/// Events since the last call.
pub fn events() -> Vec<Event> {
    channel().1.lock().map(|rx| rx.try_iter().collect()).unwrap_or_default()
}

/// "es_ES" or "es-es" as "es-ES".
/// A document's language without its region, for a speech program or
/// protocol line: letters only (else none), as the document says anything.
pub(crate) fn primary_lang(lang: &str) -> String {
    let p = lang.split(['-', '_']).next().unwrap_or("").to_lowercase();
    if p.len() <= 8 && p.bytes().all(|b| b.is_ascii_alphabetic()) { p } else { String::new() }
}

pub fn norm_lang(l: &str) -> String {
    let l = l.trim().replace('_', "-");
    match l.split_once('-') {
        Some((a, b)) => format!("{}-{}", a.to_lowercase(), b.to_uppercase()),
        None => l.to_lowercase(),
    }
}

/// The voice for a language: the same region first, then the same language.
fn pick<'a>(voices: &'a [Voice], lang: &str) -> Option<&'a Voice> {
    let lang = norm_lang(lang);
    let primary = lang.split('-').next().unwrap_or("");
    if primary.is_empty() {
        return None;
    }
    voices.iter().find(|v| norm_lang(&v.lang) == lang).or_else(|| voices.iter().find(|v| norm_lang(&v.lang).split('-').next() == Some(primary)))
}

/// Byte offsets where words start (after whitespace).
#[cfg(any(all(unix, not(target_os = "macos")), test))]
pub fn word_starts(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut prev_space = true;
    for (i, c) in text.char_indices() {
        let space = c.is_whitespace();
        if !space && prev_space {
            out.push(i);
        }
        prev_space = space;
    }
    out
}

/// Length in bytes of the word at `start`.
#[cfg(any(all(unix, not(target_os = "macos")), test))]
pub fn word_len(text: &str, start: usize) -> usize {
    text.get(start..).map_or(0, |t| t.find(char::is_whitespace).unwrap_or(t.len()))
}

// --- process engines (espeak-ng, espeak, say) -------------------------------------------

#[cfg(unix)]
mod process {
    use std::io::Write;
    use std::process::{Child, Command, Stdio};
    use std::sync::{Arc, Mutex};

    use super::{Backend, Event, Voice, send};

    /// Speaks with a program per utterance, fed the text on its input.
    pub struct Proc {
        pub program: &'static str,
        pub voices: Vec<Voice>,
        child: Arc<Mutex<Option<(u64, Child)>>>,
    }

    impl Proc {
        pub fn new(program: &'static str, voices: Vec<Voice>) -> Proc {
            Proc { program, voices, child: Arc::new(Mutex::new(None)) }
        }

        fn signal(&self, sig: i32) {
            if let Ok(g) = self.child.lock()
                && let Some((_, c)) = g.as_ref()
            {
                // SAFETY: signals the child we started (not yet reaped).
                unsafe {
                    libc::kill(c.id() as libc::pid_t, sig);
                }
            }
        }
    }

    impl Backend for Proc {
        fn voices(&self) -> Vec<Voice> {
            self.voices.clone()
        }

        fn speak(&mut self, utt: u64, text: &str, voice: Option<&Voice>, lang: &str, rate: f32) -> bool {
            self.stop();
            let wpm = ((175.0 * rate) as u32).to_string();
            let mut cmd = Command::new(self.program);
            if self.program == "say" {
                if let Some(v) = voice {
                    cmd.args(["-v", &v.id]);
                }
                cmd.args(["-r", &wpm, "-f", "-"]);
            } else {
                let v = voice.map_or_else(|| crate::speech::primary_lang(lang), |v| v.id.clone());
                if !v.is_empty() {
                    cmd.args(["-v", &v]);
                }
                cmd.args(["-s", &wpm, "--stdin"]);
                if super::muted() {
                    cmd.args(["-a", "0"]);
                }
            }
            let Ok(mut child) = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else { return false };
            if let Some(mut input) = child.stdin.take() {
                let text = text.to_string();
                // Written aside: a long text must not block the interface.
                apex_fs::thread::spawn(move || {
                    let _ = input.write_all(text.as_bytes());
                });
            }
            if let Ok(mut g) = self.child.lock() {
                *g = Some((utt, child));
            }
            let slot = Arc::clone(&self.child);
            apex_fs::thread::spawn(move || {
                loop {
                    apex_fs::thread::sleep(web_time::Duration::from_millis(60));
                    let Ok(mut g) = slot.lock() else { return };
                    match g.as_mut() {
                        Some((u, c)) if *u == utt => {
                            if let Ok(Some(_)) = c.try_wait() {
                                *g = None;
                                drop(g);
                                send(Event::End { utt });
                                return;
                            }
                        }
                        // Stopped, or another utterance took over.
                        _ => return,
                    }
                }
            });
            true
        }

        fn stop(&mut self) {
            let Ok(mut g) = self.child.lock() else { return };
            if let Some((_, mut c)) = g.take() {
                // SAFETY: wakes a paused child so that it can die.
                unsafe {
                    libc::kill(c.id() as libc::pid_t, libc::SIGCONT);
                }
                let _ = c.kill();
                let _ = c.wait();
            }
        }

        fn pause(&mut self) {
            self.signal(libc::SIGSTOP);
        }

        fn resume(&mut self) {
            self.signal(libc::SIGCONT);
        }
    }

    /// Output of a program, if it runs.
    pub fn output(program: &str, args: &[&str]) -> Option<String> {
        let o = Command::new(program).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        o.status.success().then(|| String::from_utf8_lossy(&o.stdout).into_owned())
    }

    /// `espeak-ng --voices`: "Pty Language Age/Gender VoiceName File ...".
    #[cfg(any(not(target_os = "macos"), test))]
    pub fn espeak_voices(text: &str) -> Vec<Voice> {
        text.lines()
            .skip(1)
            .filter_map(|l| {
                let f: Vec<&str> = l.split_whitespace().collect();
                let (lang, name) = (f.get(1)?, f.get(3)?);
                Some(Voice { id: lang.to_string(), name: name.replace('_', " "), lang: lang.to_string() })
            })
            .collect()
    }

    /// `say -v ?`: "Name   en_US    # Sample sentence".
    #[cfg(any(target_os = "macos", test))]
    pub fn say_voices(text: &str) -> Vec<Voice> {
        text.lines()
            .filter_map(|l| {
                let left = l.split('#').next()?.trim_end();
                let (name, lang) = left.rsplit_once(char::is_whitespace)?;
                let name = name.trim();
                (!name.is_empty()).then(|| Voice { id: name.to_string(), name: name.to_string(), lang: lang.replace('_', "-") })
            })
            .collect()
    }
}

// --- Speech Dispatcher ---------------------------------------------------------------------

#[cfg(all(unix, not(target_os = "macos")))]
mod speechd {
    use std::collections::HashMap;
    use std::ffi::{CStr, CString, c_char, c_int, c_void};
    use std::sync::Mutex;

    use super::{Backend, Event, Voice, send};

    type Cb = unsafe extern "C" fn(usize, usize, c_int);
    type CbIm = unsafe extern "C" fn(usize, usize, c_int, *mut c_char);

    /// The start of `SPDConnection`: its public callbacks.
    #[repr(C)]
    struct Conn {
        begin: Option<Cb>,
        end: Option<Cb>,
        cancel: Option<Cb>,
        pause: Option<Cb>,
        resume: Option<Cb>,
        im: Option<CbIm>,
    }

    #[repr(C)]
    struct SpdVoice {
        name: *const c_char,
        language: *const c_char,
        variant: *const c_char,
    }

    const MODE_THREADED: c_int = 1;
    const PRIORITY_TEXT: c_int = 3;
    const DATA_SSML: c_int = 1;
    const NOTIFY_END: c_int = 2;
    const NOTIFY_MARKS: c_int = 4;
    const EVENT_END: c_int = 1;
    const EVENT_MARK: c_int = 5;

    struct Api {
        open: unsafe extern "C" fn(*const c_char, *const c_char, *const c_char, c_int) -> *mut Conn,
        say: unsafe extern "C" fn(*mut Conn, c_int, *const c_char) -> c_int,
        cancel: unsafe extern "C" fn(*mut Conn) -> c_int,
        pause: unsafe extern "C" fn(*mut Conn) -> c_int,
        resume: unsafe extern "C" fn(*mut Conn) -> c_int,
        data_mode: unsafe extern "C" fn(*mut Conn, c_int) -> c_int,
        rate: unsafe extern "C" fn(*mut Conn, c_int) -> c_int,
        volume: unsafe extern "C" fn(*mut Conn, c_int) -> c_int,
        language: unsafe extern "C" fn(*mut Conn, *const c_char) -> c_int,
        voice: unsafe extern "C" fn(*mut Conn, *const c_char) -> c_int,
        voices: unsafe extern "C" fn(*mut Conn) -> *mut *mut SpdVoice,
        notify: unsafe extern "C" fn(*mut Conn, c_int) -> c_int,
    }

    /// The utterance of each message, and the text being spoken.
    struct Current {
        msgs: HashMap<usize, u64>,
        texts: HashMap<u64, String>,
        /// Events of messages not yet known (they can beat `spd_say`).
        early: Vec<(usize, c_int, Option<usize>)>,
    }

    static CURRENT: Mutex<Option<Current>> = Mutex::new(None);

    fn deliver(cur: &Current, utt: u64, kind: c_int, mark: Option<usize>) {
        match (kind, mark) {
            (EVENT_END, _) => send(Event::End { utt }),
            (EVENT_MARK, Some(start)) => {
                let len = cur.texts.get(&utt).map_or(0, |t| super::word_len(t, start));
                send(Event::Word { utt, start, len });
            }
            _ => {}
        }
    }

    fn on_event(msg: usize, kind: c_int, mark: Option<usize>) {
        let Ok(mut g) = CURRENT.lock() else { return };
        let Some(cur) = g.as_mut() else { return };
        match cur.msgs.get(&msg) {
            Some(&utt) => deliver(cur, utt, kind, mark),
            None => cur.early.push((msg, kind, mark)),
        }
    }

    unsafe extern "C" fn on_end(msg: usize, _client: usize, kind: c_int) {
        on_event(msg, kind, None);
    }

    unsafe extern "C" fn on_mark(msg: usize, _client: usize, kind: c_int, mark: *mut c_char) {
        // SAFETY: the library passes a NUL-terminated mark name.
        let name = if mark.is_null() { None } else { unsafe { CStr::from_ptr(mark) }.to_str().ok().and_then(|s| s.parse().ok()) };
        on_event(msg, kind, name);
    }

    pub struct Speechd {
        api: Api,
        conn: *mut Conn,
        voices: Vec<Voice>,
    }

    // SAFETY: the connection is opened in threaded mode (its calls lock
    // internally) and only used behind the engine's mutex.
    unsafe impl Send for Speechd {}

    unsafe fn sym<T>(lib: *mut c_void, name: &str) -> Option<T> {
        let c = CString::new(name).ok()?;
        // SAFETY: `lib` is a handle from dlopen; T is a function pointer
        // type matching the C declaration.
        unsafe {
            let p = libc::dlsym(lib, c.as_ptr());
            (!p.is_null()).then(|| std::mem::transmute_copy::<*mut c_void, T>(&p))
        }
    }

    fn cstr(p: *const c_char) -> String {
        if p.is_null() {
            return String::new();
        }
        // SAFETY: the library's strings are NUL-terminated.
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }

    /// Opens a connection to Speech Dispatcher (starting it if needed).
    pub fn open() -> Result<Speechd, String> {
        let lib = ["libspeechd.so.2", "libspeechd.so"].iter().find_map(|n| {
            let c = CString::new(*n).ok()?;
            // SAFETY: loading a library by name.
            let h = unsafe { libc::dlopen(c.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
            (!h.is_null()).then_some(h)
        });
        let Some(lib) = lib else { return Err("libspeechd".into()) };
        // SAFETY: the names and types follow libspeechd.h.
        let api = unsafe {
            (|| {
                Some(Api {
                    open: sym(lib, "spd_open")?,
                    say: sym(lib, "spd_say")?,
                    cancel: sym(lib, "spd_cancel")?,
                    pause: sym(lib, "spd_pause")?,
                    resume: sym(lib, "spd_resume")?,
                    data_mode: sym(lib, "spd_set_data_mode")?,
                    rate: sym(lib, "spd_set_voice_rate")?,
                    volume: sym(lib, "spd_set_volume")?,
                    language: sym(lib, "spd_set_language")?,
                    voice: sym(lib, "spd_set_synthesis_voice")?,
                    voices: sym(lib, "spd_list_synthesis_voices")?,
                    notify: sym(lib, "spd_set_notification_on")?,
                })
            })()
        };
        let Some(api) = api else { return Err("libspeechd".into()) };
        let name = CString::new("apexdesk").unwrap_or_default();
        let main = CString::new("main").unwrap_or_default();
        // SAFETY: valid strings; a null user takes the login name.
        let conn = unsafe { (api.open)(name.as_ptr(), main.as_ptr(), std::ptr::null(), MODE_THREADED) };
        if conn.is_null() {
            return Err("speech-dispatcher".into());
        }
        if let Ok(mut g) = CURRENT.lock() {
            *g = Some(Current { msgs: HashMap::new(), texts: HashMap::new(), early: Vec::new() });
        }
        // SAFETY: `conn` is a live connection; the callbacks are set before
        // notifications are turned on.
        let voices = unsafe {
            (*conn).end = Some(on_end);
            (*conn).im = Some(on_mark);
            (api.notify)(conn, NOTIFY_END);
            (api.notify)(conn, NOTIFY_MARKS);
            (api.data_mode)(conn, DATA_SSML);
            if super::muted() {
                (api.volume)(conn, -100);
            }
            let mut out = Vec::new();
            let list = (api.voices)(conn);
            if !list.is_null() {
                let mut i = 0;
                while !(*list.add(i)).is_null() {
                    let v = &**list.add(i);
                    let name = cstr(v.name);
                    out.push(Voice { id: name.clone(), name, lang: cstr(v.language) });
                    i += 1;
                }
                // The list is small and asked once: not freed (the
                // freeing function is missing from older versions).
            }
            out
        };
        Ok(Speechd { api, conn, voices })
    }

    /// The text as SSML with a mark (its byte offset) before each word.
    pub fn ssml(text: &str) -> String {
        let starts = super::word_starts(text);
        let mut out = String::from("<speak>");
        let mut next = starts.iter().peekable();
        for (i, c) in text.char_indices() {
            if next.peek() == Some(&&i) {
                next.next();
                out.push_str(&format!("<mark name=\"{i}\"/>"));
            }
            match c {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                c if c.is_control() => out.push(' '),
                c => out.push(c),
            }
        }
        out.push_str("</speak>");
        out
    }

    impl Backend for Speechd {
        fn voices(&self) -> Vec<Voice> {
            self.voices.clone()
        }

        fn speak(&mut self, utt: u64, text: &str, voice: Option<&Voice>, lang: &str, rate: f32) -> bool {
            let Ok(doc) = CString::new(ssml(text)) else { return false };
            // -100..100 with 0 the normal speed; 2x is the fastest.
            let r = (rate.log2() * 100.0).round().clamp(-100.0, 100.0) as c_int;
            if let Ok(mut g) = CURRENT.lock()
                && let Some(cur) = g.as_mut()
            {
                cur.texts.clear();
                cur.texts.insert(utt, text.to_string());
            }
            // SAFETY: live connection, valid strings.
            let msg = unsafe {
                (self.api.cancel)(self.conn);
                (self.api.resume)(self.conn);
                (self.api.rate)(self.conn, r);
                let primary = crate::speech::primary_lang(lang);
                if let Ok(l) = CString::new(voice.map_or(primary, |v| v.lang.clone())) {
                    if !l.as_bytes().is_empty() {
                        (self.api.language)(self.conn, l.as_ptr());
                    }
                }
                if let Some(v) = voice
                    && let Ok(n) = CString::new(v.id.clone())
                {
                    (self.api.voice)(self.conn, n.as_ptr());
                }
                (self.api.say)(self.conn, PRIORITY_TEXT, doc.as_ptr())
            };
            if msg < 0 {
                return false;
            }
            if let Ok(mut g) = CURRENT.lock()
                && let Some(cur) = g.as_mut()
            {
                let msg = msg as usize;
                cur.msgs.clear();
                cur.msgs.insert(msg, utt);
                let early: Vec<_> = cur.early.drain(..).filter(|e| e.0 == msg).collect();
                for (_, kind, mark) in early {
                    deliver(cur, utt, kind, mark);
                }
            }
            true
        }

        fn stop(&mut self) {
            if let Ok(mut g) = CURRENT.lock()
                && let Some(cur) = g.as_mut()
            {
                cur.msgs.clear();
                cur.early.clear();
            }
            // SAFETY: live connection.
            unsafe {
                (self.api.cancel)(self.conn);
                (self.api.resume)(self.conn);
            }
        }

        fn pause(&mut self) {
            // SAFETY: live connection.
            unsafe {
                (self.api.pause)(self.conn);
            }
        }

        fn resume(&mut self) {
            // SAFETY: live connection.
            unsafe {
                (self.api.resume)(self.conn);
            }
        }
    }
}

// --- Windows: System.Speech -------------------------------------------------------------------

#[cfg(windows)]
mod sapi {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Child, ChildStdin, Command, Stdio};

    use super::{Backend, Event, Voice, send};

    /// A helper that speaks what it reads on its input ("S utt rate voice
    /// text", "X" stop, "P" pause, "U" resume, base64 strings) and writes
    /// the voices ("V name lang", then "R"), each word ("W utt pos len",
    /// UTF-16 units) and each end ("E utt").
    const SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Speech
$ref = [System.Speech.Synthesis.SpeechSynthesizer].Assembly.Location
Add-Type -ReferencedAssemblies $ref -TypeDefinition @'
using System;
using System.IO;
using System.Text;
using System.Speech.Synthesis;

public static class ApexSpeech {
    static readonly object Gate = new object();
    static StreamWriter Out;
    static long Cur;
    static Prompt Current;

    static void Send(string s) { lock (Gate) { Out.WriteLine(s); Out.Flush(); } }
    static string Enc(string s) { return Convert.ToBase64String(Encoding.UTF8.GetBytes(s)); }
    static string Dec(string s) { return Encoding.UTF8.GetString(Convert.FromBase64String(s)); }

    public static void Run() {
        Out = new StreamWriter(Console.OpenStandardOutput(), new UTF8Encoding(false));
        var input = new StreamReader(Console.OpenStandardInput(), new UTF8Encoding(false));
        var s = new SpeechSynthesizer();
        s.SetOutputToDefaultAudioDevice();
        s.SpeakProgress += (o, e) => { if (e.Prompt == Current) Send("W " + Cur + " " + e.CharacterPosition + " " + e.CharacterCount); };
        s.SpeakCompleted += (o, e) => { if (e.Prompt == Current && !e.Cancelled) Send("E " + Cur); };
        foreach (var v in s.GetInstalledVoices()) {
            if (v.Enabled) Send("V " + Enc(v.VoiceInfo.Name) + " " + v.VoiceInfo.Culture.Name);
        }
        Send("R");
        string line;
        while ((line = input.ReadLine()) != null) {
            var f = line.Split(' ');
            try {
                if (f[0] == "S" && f.Length == 5) {
                    Current = null;
                    s.SpeakAsyncCancelAll();
                    if (s.State == SynthesizerState.Paused) s.Resume();
                    s.Rate = int.Parse(f[2]);
                    var voice = Dec(f[3]);
                    if (voice.Length > 0) s.SelectVoice(voice);
                    Cur = long.Parse(f[1]);
                    var p = new Prompt(Dec(f[4]));
                    Current = p;
                    s.SpeakAsync(p);
                } else if (f[0] == "X") {
                    Current = null;
                    s.SpeakAsyncCancelAll();
                    if (s.State == SynthesizerState.Paused) s.Resume();
                } else if (f[0] == "P") {
                    if (s.State == SynthesizerState.Speaking) s.Pause();
                } else if (f[0] == "U") {
                    if (s.State == SynthesizerState.Paused) s.Resume();
                } else if (f[0] == "M") {
                    s.Volume = 0;
                }
            } catch (Exception) {
                Send("E " + Cur);
            }
        }
    }
}
'@
[ApexSpeech]::Run()
"#;

    pub struct Sapi {
        child: Child,
        input: ChildStdin,
        voices: Vec<Voice>,
        /// Text of each utterance being spoken, to turn positions into bytes.
        texts: std::sync::Arc<std::sync::Mutex<Vec<(u64, String)>>>,
    }

    /// Byte offset of UTF-16 position `u` of `text`.
    fn byte_of(text: &str, u: usize) -> usize {
        let mut n = 0;
        for (i, c) in text.char_indices() {
            if n >= u {
                return i;
            }
            n += c.len_utf16();
        }
        text.len()
    }

    pub fn open() -> Result<Sapi, String> {
        use std::os::windows::process::CommandExt;
        const NO_WINDOW: u32 = 0x0800_0000;
        let utf16: Vec<u8> = SCRIPT.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut child = Command::new(apex_io::safe_path::windows_program(r"System32\WindowsPowerShell\v1.0\powershell.exe"))
            .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &super::b64(&utf16)])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(NO_WINDOW)
            .spawn()
            .map_err(|_| "PowerShell".to_string())?;
        let input = child.stdin.take().ok_or("PowerShell")?;
        let mut out = BufReader::new(child.stdout.take().ok_or("PowerShell")?);
        let mut voices = Vec::new();
        let mut line = String::new();
        loop {
            line.clear();
            if out.read_line(&mut line).unwrap_or(0) == 0 {
                let _ = child.kill();
                return Err("System.Speech".into());
            }
            let l = line.trim_end();
            if l == "R" {
                break;
            }
            if let Some(rest) = l.strip_prefix("V ") {
                let (name, lang) = rest.split_once(' ').unwrap_or((rest, ""));
                let name = super::unb64(name);
                voices.push(Voice { id: name.clone(), name, lang: lang.to_string() });
            }
        }
        let texts: std::sync::Arc<std::sync::Mutex<Vec<(u64, String)>>> = Default::default();
        let t = std::sync::Arc::clone(&texts);
        apex_fs::thread::spawn(move || {
            for line in out.lines() {
                let Ok(line) = line else { break };
                let f: Vec<&str> = line.split(' ').collect();
                let num = |i: usize| f.get(i).and_then(|x| x.parse::<u64>().ok());
                match (f.first().copied(), num(1)) {
                    (Some("W"), Some(utt)) => {
                        let (Some(pos), Some(len)) = (num(2), num(3)) else { continue };
                        let Ok(g) = t.lock() else { continue };
                        if let Some((_, text)) = g.iter().find(|(u, _)| *u == utt) {
                            let start = byte_of(text, pos as usize);
                            let end = byte_of(text, (pos + len) as usize);
                            send(Event::Word { utt, start, len: end - start });
                        }
                    }
                    (Some("E"), Some(utt)) => send(Event::End { utt }),
                    _ => {}
                }
            }
        });
        let mut s = Sapi { child, input, voices, texts };
        if super::muted() {
            s.cmd("M");
        }
        Ok(s)
    }

    impl Sapi {
        fn cmd(&mut self, c: &str) {
            let _ = writeln!(self.input, "{c}");
            let _ = self.input.flush();
        }
    }

    impl Drop for Sapi {
        fn drop(&mut self) {
            let _ = self.child.kill();
        }
    }

    impl Backend for Sapi {
        fn voices(&self) -> Vec<Voice> {
            self.voices.clone()
        }

        fn speak(&mut self, utt: u64, text: &str, voice: Option<&Voice>, _lang: &str, rate: f32) -> bool {
            if let Ok(mut g) = self.texts.lock() {
                g.clear();
                g.push((utt, text.to_string()));
            }
            // -10..10; 10 is about three times as fast.
            let r = (rate.log2() * 6.3).round().clamp(-10.0, 10.0) as i32;
            let v = super::b64(voice.map_or("", |v| v.id.as_str()).as_bytes());
            self.cmd(&format!("S {utt} {r} {v} {}", super::b64(text.as_bytes())));
            true
        }

        fn stop(&mut self) {
            self.cmd("X");
        }

        fn pause(&mut self) {
            self.cmd("P");
        }

        fn resume(&mut self) {
            self.cmd("U");
        }
    }
}

#[cfg(any(windows, test))]
fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            out.push(if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

#[cfg(any(windows, test))]
fn unb64(s: &str) -> String {
    let val = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for v in s.bytes().filter_map(val) {
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// --- finding an engine ------------------------------------------------------------------------

#[cfg(all(unix, not(target_os = "macos")))]
fn find_backend() -> Result<Box<dyn Backend>, String> {
    if std::env::var_os("APEXDESK_NO_SPEECHD").is_none() {
        if let Ok(s) = speechd::open() {
            return Ok(Box::new(s));
        }
    }
    for program in ["espeak-ng", "espeak"] {
        if let Some(out) = process::output(program, &["--voices"]) {
            return Ok(Box::new(process::Proc::new(program, process::espeak_voices(&out))));
        }
    }
    Err(crate::i18n::tr("speak.missing_linux").to_string())
}

#[cfg(target_os = "macos")]
fn find_backend() -> Result<Box<dyn Backend>, String> {
    match process::output("say", &["-v", "?"]) {
        Some(out) => Ok(Box::new(process::Proc::new("say", process::say_voices(&out)))),
        None => Err(crate::i18n::tr("speak.missing").to_string()),
    }
}

#[cfg(windows)]
fn find_backend() -> Result<Box<dyn Backend>, String> {
    sapi::open().map(|s| Box::new(s) as Box<dyn Backend>).map_err(|_| crate::i18n::tr("speak.missing").to_string())
}

#[cfg(not(any(unix, windows)))]
fn find_backend() -> Result<Box<dyn Backend>, String> {
    Err(crate::i18n::tr("speak.missing").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words() {
        let t = "  Hola, ¿qué tal?\tBien ";
        let s = word_starts(t);
        assert_eq!(s.iter().map(|&i| &t[i..i + word_len(t, i)]).collect::<Vec<_>>(), ["Hola,", "¿qué", "tal?", "Bien"]);
        assert_eq!(word_len("abc", 5), 0);
    }

    #[test]
    fn voices_by_language() {
        let v = |id: &str, lang: &str| Voice { id: id.into(), name: id.into(), lang: lang.into() };
        let list = [v("a", "en-US"), v("b", "es-MX"), v("c", "es_ES"), v("d", "fr")];
        assert_eq!(pick(&list, "es-ES").map(|v| v.id.as_str()), Some("c"));
        assert_eq!(pick(&list, "es_AR").map(|v| v.id.as_str()), Some("b"));
        assert_eq!(pick(&list, "fr-CA").map(|v| v.id.as_str()), Some("d"));
        assert_eq!(pick(&list, "de"), None);
        assert_eq!(pick(&list, ""), None);
        assert_eq!(norm_lang("pt_br"), "pt-BR");
    }

    #[test]
    fn base64_both_ways() {
        for s in ["", "a", "ab", "abc", "Microsoft Helena Desktop - Spanish (Spain)", "ñandú 語"] {
            assert_eq!(unb64(&b64(s.as_bytes())), s);
        }
        assert_eq!(b64(b"Man"), "TWFu");
        assert_eq!(b64(b"Ma"), "TWE=");
    }

    #[cfg(unix)]
    #[test]
    fn program_voice_lists() {
        let espeak = " Pty Language       Age/Gender VoiceName          File                 Other Languages\n 5  es              --/M      Spanish_(Spain)    roa/es\n 5  en-us           --/M      English_(America)  gmw/en-US            (en 3)\n";
        let v = process::espeak_voices(espeak);
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].id.as_str(), v[0].name.as_str(), v[1].lang.as_str()), ("es", "Spanish (Spain)", "en-us"));
        let say = "Alex                en_US    # Most people recognize me by my voice.\nBad News            en_US    # The light you see at the end of the tunnel is the headlamp of a fast approaching train.\nMónica              es_ES    # Hola, me llamo Mónica.\n";
        let v = process::say_voices(say);
        assert_eq!(v.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(), ["Alex", "Bad News", "Mónica"]);
        assert_eq!(v[2].lang, "es-ES");
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn marks_before_words() {
        assert_eq!(speechd::ssml("a <b> & c"), "<speak><mark name=\"0\"/>a <mark name=\"2\"/>&lt;b&gt; <mark name=\"6\"/>&amp; <mark name=\"8\"/>c</speak>");
    }
}
