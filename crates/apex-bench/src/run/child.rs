//! Child processes with a time and a memory limit. The memory is watched
//! through /proc on Linux (elsewhere only the time limit applies).

use std::io::Read;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Killed {
    Timeout,
    Memory,
}

pub struct Exit {
    pub status: Option<ExitStatus>,
    pub killed: Option<Killed>,
    pub stdout: String,
    pub stderr: String,
    /// Peak resident memory seen, in kB (0 where unknown).
    pub peak_kb: u64,
}

impl Exit {
    pub fn success(&self) -> bool {
        self.killed.is_none() && self.status.is_some_and(|s| s.success())
    }

    /// The panic message on stderr ("message (at src/x.rs:1:2)").
    pub fn panic(&self) -> Option<String> {
        let lines: Vec<&str> = self.stderr.lines().collect();
        let i = lines.iter().position(|l| l.contains("panicked at"))?;
        let at = lines[i].split("panicked at").nth(1).unwrap_or("").trim().trim_end_matches(':').to_string();
        // Since Rust 1.73 the message follows on its own line.
        let msg = lines.get(i + 1).filter(|l| !l.starts_with("note:")).map(|l| l.trim()).unwrap_or("");
        Some(if msg.is_empty() { format!("panic at {at}") } else { format!("{msg} (at {at})") })
    }

    /// The last lines of stderr, for crashes without a panic message.
    pub fn stderr_tail(&self) -> String {
        let lines: Vec<&str> = self.stderr.lines().filter(|l| !l.trim().is_empty()).collect();
        lines[lines.len().saturating_sub(2)..].join(" / ")
    }
}

/// Runs `cmd` to the end, killing it after `timeout` or past `mem_mb` of
/// resident memory.
pub fn run(mut cmd: Command, timeout: Duration, mem_mb: u64) -> std::io::Result<Exit> {
    let start = Instant::now();
    #[cfg(unix)]
    {
        // Its own process group, so that helpers it starts die with it.
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let reader = |mut r: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = r.read_to_end(&mut buf);
            String::from_utf8_lossy(&buf).into_owned()
        })
    };
    let out = reader(Box::new(child.stdout.take().expect("piped")));
    let err = reader(Box::new(child.stderr.take().expect("piped")));
    let mut killed = None;
    let mut peak_kb = 0;
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break Some(s);
        }
        let rss = rss_kb(child.id());
        peak_kb = peak_kb.max(rss);
        if start.elapsed() > timeout {
            killed = Some(Killed::Timeout);
        } else if mem_mb > 0 && rss > mem_mb * 1024 {
            killed = Some(Killed::Memory);
        }
        if killed.is_some() {
            kill_group(child.id());
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    Ok(Exit { status, killed, stdout: out.join().unwrap_or_default(), stderr: err.join().unwrap_or_default(), peak_kb })
}

#[cfg(unix)]
fn kill_group(pid: u32) {
    let _ = Command::new("kill").args(["-KILL", "--", &format!("-{pid}")]).stderr(Stdio::null()).status();
}

#[cfg(not(unix))]
fn kill_group(_pid: u32) {}

/// A field of /proc/PID/status in kB.
#[cfg(target_os = "linux")]
fn status_kb(pid: &str, field: &str) -> u64 {
    let s = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
    s.lines().find_map(|l| l.strip_prefix(field)).and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok()).unwrap_or(0)
}

#[cfg(target_os = "linux")]
fn rss_kb(pid: u32) -> u64 {
    status_kb(&pid.to_string(), "VmRSS:")
}

#[cfg(not(target_os = "linux"))]
fn rss_kb(_pid: u32) -> u64 {
    0
}

/// Peak resident memory of this process in kB (0 where unknown).
#[cfg(target_os = "linux")]
pub fn own_peak_kb() -> u64 {
    status_kb("self", "VmHWM:")
}

#[cfg(not(target_os = "linux"))]
pub fn own_peak_kb() -> u64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panic_message() {
        let e = Exit {
            status: None,
            killed: None,
            stdout: String::new(),
            stderr: "\nthread 'main' panicked at crates/x/src/a.rs:10:5:\nindex out of bounds\nnote: run with `RUST_BACKTRACE=1`\n".into(),
            peak_kb: 0,
        };
        assert_eq!(e.panic().unwrap(), "index out of bounds (at crates/x/src/a.rs:10:5)");
    }

    #[cfg(unix)]
    #[test]
    fn timeout_kills() {
        let mut c = Command::new("sleep");
        c.arg("5");
        let t = Instant::now();
        let e = run(c, Duration::from_millis(200), 0).unwrap();
        assert_eq!(e.killed, Some(Killed::Timeout));
        assert!(t.elapsed() < Duration::from_secs(3));
    }
}
