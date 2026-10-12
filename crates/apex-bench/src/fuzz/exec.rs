//! Runs one test case in a child process with a time limit, and tells
//! what happened: a clean open or rejection, a panic, a hang, too much
//! memory, a stack overflow, another signal, or a file of ours that
//! doesn't open again. A failure gets a signature (the place in the code)
//! so that the same bug found twice is reported once.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::child;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Ok,
    Rejected,
    Panic,
    Hang,
    Memory,
    Stack,
    Signal,
    /// A file ApexDesk saved doesn't open again.
    Reopen,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Ok => "ok",
            Kind::Rejected => "rejected",
            Kind::Panic => "panic",
            Kind::Hang => "hang",
            Kind::Memory => "memory",
            Kind::Stack => "stack",
            Kind::Signal => "signal",
            Kind::Reopen => "reopen",
        }
    }

    pub fn is_failure(self) -> bool {
        !matches!(self, Kind::Ok | Kind::Rejected)
    }
}

pub struct Outcome {
    pub kind: Kind,
    /// The step the child was in (open, recalc, save-xlsx...).
    pub stage: String,
    /// Where in the code (file:line or function); empty for no failure.
    pub signature: String,
    /// The first line of the panic message, or of the error.
    pub message: String,
    /// What the child printed (the backtrace), or the debugger's view of
    /// a hung child.
    pub trace: String,
    pub secs: f64,
    /// Peak resident memory in KiB (0 where the system doesn't say).
    pub max_rss_kb: u64,
}

pub struct Limits {
    pub timeout: Duration,
    pub memory: usize,
}

/// Runs `apex-bench fuzz --child FILE WORK`.
pub fn run(exe: &Path, file: &Path, work: &Path, limits: &Limits, debugger: bool) -> Outcome {
    let start = Instant::now();
    let mut cmd = Command::new(exe);
    cmd.arg("fuzz").arg("--child").arg(file).arg(work);
    cmd.env("APEX_BENCH_MEM_LIMIT", limits.memory.to_string()).env("RUST_BACKTRACE", "0").env("RUST_LIB_BACKTRACE", "0");
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    let mut ch = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return outcome(Kind::Signal, String::new(), format!("spawn failed: {e}"), String::new(), 0.0, 0),
    };
    let mut err = ch.stderr.take().expect("piped stderr");
    let reader = std::thread::spawn(move || {
        let mut keep = Vec::new();
        let mut buf = [0u8; 8192];
        while let Ok(n) = err.read(&mut buf) {
            if n == 0 {
                break;
            }
            if keep.len() < 256 << 10 {
                keep.extend_from_slice(&buf[..n]);
            }
        }
        String::from_utf8_lossy(&keep).into_owned()
    });
    let (status, rss, timed_out, stack) = wait(&mut ch, limits.timeout, debugger);
    let text = reader.join().unwrap_or_default();
    let secs = start.elapsed().as_secs_f64();
    classify(&text, status, timed_out, stack, secs, rss)
}

/// How the child ended: exit code or signal.
#[derive(Clone, Copy, Debug)]
pub enum Status {
    Code(i32),
    Signal(i32),
    Unknown,
}

#[cfg(target_os = "linux")]
fn wait(ch: &mut std::process::Child, timeout: Duration, debugger: bool) -> (Status, u64, bool, String) {
    let pid = ch.id() as libc::pid_t;
    let start = Instant::now();
    let mut pause = Duration::from_millis(1);
    let mut stack = String::new();
    let mut timed_out = false;
    loop {
        let mut st: libc::c_int = 0;
        let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
        let flags = if timed_out { 0 } else { libc::WNOHANG };
        let r = unsafe { libc::wait4(pid, &mut st, flags, &mut ru) };
        if r == pid {
            let status = if libc::WIFEXITED(st) {
                Status::Code(libc::WEXITSTATUS(st))
            } else if libc::WIFSIGNALED(st) {
                Status::Signal(libc::WTERMSIG(st))
            } else {
                Status::Unknown
            };
            return (status, ru.ru_maxrss as u64, timed_out, stack);
        }
        if r < 0 {
            return (Status::Unknown, 0, timed_out, stack);
        }
        if !timed_out && start.elapsed() > timeout {
            timed_out = true;
            if debugger {
                stack = gdb_stack(pid as u32);
            }
            unsafe { libc::kill(pid, libc::SIGKILL) };
            continue;
        }
        std::thread::sleep(pause);
        pause = (pause * 2).min(Duration::from_millis(20));
    }
}

#[cfg(not(target_os = "linux"))]
fn wait(ch: &mut std::process::Child, timeout: Duration, _debugger: bool) -> (Status, u64, bool, String) {
    let start = Instant::now();
    loop {
        match ch.try_wait() {
            Ok(Some(s)) => return (s.code().map(Status::Code).unwrap_or(Status::Unknown), 0, false, String::new()),
            Ok(None) if start.elapsed() > timeout => {
                let _ = ch.kill();
                let _ = ch.wait();
                return (Status::Unknown, 0, true, String::new());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(_) => return (Status::Unknown, 0, false, String::new()),
        }
    }
}

/// Caps the address space of a helper process (Linux; nothing elsewhere):
/// a big file must not take the machine's memory.
pub fn limit_memory(cmd: &mut Command, bytes: u64) {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setrlimit is async-signal-safe; nothing else runs
        // between fork and exec.
        unsafe {
            cmd.pre_exec(move || {
                let l = libc::rlimit { rlim_cur: bytes, rlim_max: bytes };
                libc::setrlimit(libc::RLIMIT_AS, &l);
                Ok(())
            });
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (cmd, bytes);
}

/// Waits for a helper process (LibreOffice, the command line) at most
/// `limit`; kills it after that. Returns whether it ended well, and its
/// peak memory in KiB with the processes it waited for.
pub fn wait_limited(ch: &mut std::process::Child, limit: Duration) -> (bool, u64) {
    let (status, rss, timed_out, _) = wait(ch, limit, false);
    (!timed_out && matches!(status, Status::Code(0)), rss)
}

/// The stacks of a hung child, from gdb when it is installed.
#[cfg(target_os = "linux")]
fn gdb_stack(pid: u32) -> String {
    let child = Command::new("gdb")
        .args(["-p", &pid.to_string(), "-batch", "-nx", "-ex", "thread apply all bt 60"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else { return String::new() };
    let mut out = child.stdout.take().expect("piped stdout");
    let t = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(60) {
        if let Ok(Some(_)) = child.try_wait() {
            return t.join().unwrap_or_default();
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    t.join().unwrap_or_default()
}

fn outcome(kind: Kind, stage: String, message: String, trace: String, secs: f64, rss: u64) -> Outcome {
    Outcome { kind, stage, signature: String::new(), message, trace, secs, max_rss_kb: rss }
}

/// Frames of a Rust backtrace or of gdb: (function, file:line).
pub fn frames(trace: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let lines: Vec<&str> = trace.lines().collect();
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim_start();
        if let Some(rest) = t.strip_prefix('#') {
            // gdb: "#3  0x... in apex_io::x::y (...) at crates/apex-io/src/x.rs:12"
            let body = rest.split_once(' ').map(|x| x.1).unwrap_or("").trim_start();
            let body = body.split_once(" in ").map(|x| x.1).unwrap_or(body);
            let func = body.split(" (").next().unwrap_or("").trim().to_string();
            let loc = body.rsplit_once(" at ").map(|x| x.1.trim().to_string()).unwrap_or_default();
            out.push((func, loc));
        } else if let Some((n, f)) = t.split_once(": ")
            && n.chars().all(|c| c.is_ascii_digit())
            && !n.is_empty()
        {
            // std: "  12: apex_io::xlsx::read\n             at ./crates/apex-io/src/xlsx.rs:1:2"
            let loc = lines.get(i + 1).and_then(|n| n.trim_start().strip_prefix("at ")).map(|s| s.trim().to_string()).unwrap_or_default();
            out.push((f.trim().to_string(), loc));
        }
    }
    out
}

/// The first frame in ApexDesk's own code, not in the bench or std.
fn own_frame(frames: &[(String, String)]) -> Option<&(String, String)> {
    frames.iter().find(|(f, loc)| {
        let ours = loc.contains("crates/apex-") || f.starts_with("apex_");
        ours && !loc.contains("apex-bench") && !f.starts_with("apex_bench")
    })
}

/// Where a hung child was: the first frame in a reader or writer
/// (apex-io), else in any crate of ApexDesk; as file and function, not
/// line (a loop is at a different line each time). gdb names inlined
/// functions without their module, the file tells it.
fn hang_frame(frames: &[(String, String)]) -> Option<String> {
    let ours = |(f, l): &&(String, String)| l.contains("crates/apex-") && !l.contains("apex-bench") && !f.is_empty();
    let pick = frames.iter().filter(ours).find(|(_, l)| l.contains("crates/apex-io/")).or_else(|| frames.iter().find(|x| ours(x)))?;
    let file = short_loc(&pick.1);
    let file = file.rsplit_once(':').map(|x| x.0).unwrap_or(&file).trim_start_matches("crates/").to_string();
    let func = pick.0.split("::{{closure}}").next().unwrap_or(&pick.0);
    let func = func.split('<').next().unwrap_or(func).rsplit("::").next().unwrap_or(func);
    Some(format!("{file} {func}"))
}

/// `crates/apex-io/src/x.rs:12` from any path of the file and a column.
fn short_loc(loc: &str) -> String {
    let loc = loc.find("crates/").map(|i| &loc[i..]).unwrap_or(loc);
    let mut parts = loc.rsplitn(3, ':').collect::<Vec<_>>();
    if parts.len() == 3 && parts[0].chars().all(|c| c.is_ascii_digit()) {
        parts.remove(0);
        return format!("{}:{}", parts[1], parts[0]);
    }
    loc.to_string()
}

pub fn classify(text: &str, status: Status, timed_out: bool, stack: String, secs: f64, _wait_rss: u64) -> Outcome {
    // "APEX-PEAK heap 12 MB, resident 40 MB"
    let rss = text
        .lines()
        .find_map(|l| l.strip_prefix("APEX-PEAK "))
        .and_then(|l| l.split("resident ").nth(1))
        .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
        .map(|mb| mb << 10)
        .unwrap_or(0);
    let stage = text.lines().filter_map(|l| l.strip_prefix("APEX-STAGE ")).last().unwrap_or("start").to_string();
    let crash_stage = text.lines().find_map(|l| l.strip_prefix("APEX-CRASH panic stage=")).map(str::to_string);
    let stage = crash_stage.unwrap_or(stage);
    if timed_out {
        let fr = frames(&stack);
        let mut o = outcome(Kind::Hang, stage.clone(), format!("no answer in {secs:.0} s"), stack.clone(), secs, rss);
        o.signature = hang_frame(&fr).unwrap_or_else(|| format!("stage {stage}"));
        return o;
    }
    if let Some(i) = text.find("APEX-CRASH panic") {
        let body = &text[i..];
        let mut lines = body.lines().skip(1);
        let head = lines.next().unwrap_or("");
        // "panicked at crates/apex-io/src/x.rs:1:2:" then the message.
        let loc = head.strip_prefix("panicked at ").unwrap_or(head).trim_end_matches(':').to_string();
        let message = lines.next().unwrap_or("").chars().take(300).collect::<String>();
        let fr = frames(body);
        let sig = if loc.contains("crates/apex-") && !loc.contains("apex-bench") {
            short_loc(&loc)
        } else {
            match own_frame(&fr) {
                Some((f, l)) if !l.is_empty() => format!("{} (in {})", short_loc(l), short_loc(&loc)),
                Some((f, _)) => format!("{f} (in {})", short_loc(&loc)),
                None => short_loc(&loc),
            }
        };
        let mut o = outcome(Kind::Panic, stage, message, body.chars().take(64 << 10).collect(), secs, rss);
        o.signature = sig;
        return o;
    }
    if let Some(i) = text.find("APEX-CRASH memory") {
        let body = &text[i..];
        let fr = frames(body);
        let sig = own_frame(&fr).map(|(f, l)| if l.is_empty() { f.clone() } else { short_loc(l) }).unwrap_or_else(|| format!("stage {stage}"));
        let message = body.lines().next().unwrap_or("").trim_start_matches("APEX-CRASH memory: ").to_string();
        let mut o = outcome(Kind::Memory, stage, message, body.chars().take(64 << 10).collect(), secs, rss);
        o.signature = sig;
        return o;
    }
    if text.contains("has overflowed its stack") {
        let mut o = outcome(Kind::Stack, stage.clone(), "stack overflow".into(), text.chars().take(8 << 10).collect(), secs, rss);
        o.signature = format!("stage {stage}");
        return o;
    }
    if let Some(i) = text.find("APEX-ERROR ") {
        let message = text[i + 11..].lines().next().unwrap_or("").to_string();
        let mut o = outcome(Kind::Reopen, stage.clone(), message.clone(), String::new(), secs, rss);
        // The message without the specifics of the file (numbers, names).
        let generic: String = message.chars().map(|c| if c.is_ascii_digit() { '#' } else { c }).collect();
        o.signature = format!("{stage}: {}", generic.split('\'').next().unwrap_or(&generic).trim());
        return o;
    }
    match status {
        Status::Code(0) => {
            let rejected = text.contains("APEX-REJECTED");
            let msg = text.lines().find_map(|l| l.strip_prefix("APEX-REJECTED ")).unwrap_or("").to_string();
            let mut o = outcome(if rejected { Kind::Rejected } else { Kind::Ok }, stage, msg, String::new(), secs, rss);
            if rss > 2 << 20 {
                o.kind = Kind::Memory;
                o.message = text.lines().find_map(|l| l.strip_prefix("APEX-PEAK ")).unwrap_or("").to_string();
                o.signature = format!("resident over 2 GB, stage {}", o.stage);
            }
            o
        }
        Status::Code(c) if c == child::EXIT_PANIC || c == child::EXIT_MEMORY => {
            // The report was cut: still a failure.
            let mut o = outcome(Kind::Signal, stage.clone(), format!("exit {c} without a report"), text.chars().take(8 << 10).collect(), secs, rss);
            o.signature = format!("exit {c} at {stage}");
            o
        }
        Status::Signal(s) => {
            let mut o = outcome(Kind::Signal, stage.clone(), format!("signal {s}"), text.chars().take(8 << 10).collect(), secs, rss);
            o.signature = format!("signal {s} at {stage}");
            o
        }
        Status::Code(c) => {
            let mut o = outcome(Kind::Signal, stage.clone(), format!("exit {c}"), text.chars().take(8 << 10).collect(), secs, rss);
            o.signature = format!("exit {c} at {stage}");
            o
        }
        Status::Unknown => outcome(Kind::Ok, stage, String::new(), String::new(), secs, rss),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panics_are_located() {
        let text = "APEX-STAGE open\nAPEX-CRASH panic stage=open\npanicked at crates/apex-io/src/xlsx.rs:120:9:\nindex out of bounds\nstack backtrace:\n   0: std::x\n             at /rustc/abc/library/std/src/x.rs:1:1\n   1: apex_io::xlsx::read\n             at ./crates/apex-io/src/xlsx.rs:120:9\n";
        let o = classify(text, Status::Code(101), false, String::new(), 0.1, 0);
        assert_eq!(o.kind, Kind::Panic);
        assert_eq!(o.signature, "crates/apex-io/src/xlsx.rs:120");
        assert_eq!(o.message, "index out of bounds");
        assert_eq!(o.stage, "open");
        let text = "APEX-CRASH panic stage=save-xlsx\npanicked at /rustc/abc/library/alloc/src/raw_vec.rs:24:5:\ncapacity overflow\n   3: apex_io::xlsx::write\n             at ./crates/apex-io/src/xlsx.rs:77:3\n";
        let o = classify(text, Status::Code(101), false, String::new(), 0.1, 0);
        assert_eq!(o.signature, "crates/apex-io/src/xlsx.rs:77 (in /rustc/abc/library/alloc/src/raw_vec.rs:24)");
        assert_eq!(o.stage, "save-xlsx");
    }

    #[test]
    fn hangs_by_function() {
        let gdb = "Thread 2 (Thread 0x7f):\n#0  0x0000 in core::str::x () at /rustc/a/core.rs:1\n#1  get<u32> () at crates/apex-sheet/src/sheet.rs:649\n#2  0x0001 in apex_io::rtf::parse::{{closure}} (p=...) at crates/apex-io/src/rtf.rs:88\n";
        let o = classify("APEX-STAGE open\n", Status::Signal(9), true, gdb.into(), 10.0, 0);
        assert_eq!(o.kind, Kind::Hang);
        assert_eq!(o.signature, "apex-io/src/rtf.rs parse");
    }

    #[test]
    fn clean_results() {
        assert_eq!(classify("APEX-STAGE open\nAPEX-REJECTED bad zip\n", Status::Code(0), false, String::new(), 0.0, 0).kind, Kind::Rejected);
        assert_eq!(classify("APEX-STAGE done\n", Status::Code(0), false, String::new(), 0.0, 0).kind, Kind::Ok);
        assert_eq!(classify("thread 'x' has overflowed its stack\n", Status::Signal(6), false, String::new(), 0.0, 0).kind, Kind::Stack);
        let o = classify("APEX-STAGE reopen-xlsx\nAPEX-ERROR reopen xlsx: missing part 'xl/a1.xml'\n", Status::Code(103), false, String::new(), 0.0, 0);
        assert_eq!((o.kind, o.signature.as_str()), (Kind::Reopen, "reopen-xlsx: reopen xlsx: missing part"));
    }
}
