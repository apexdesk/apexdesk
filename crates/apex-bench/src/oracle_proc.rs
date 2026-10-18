//! Child processes with a time and memory limit, and a small pool that
//! runs jobs on a few threads. Every file is checked in a child process of
//! its own: a crash (release builds abort on panic), a hang or runaway
//! memory then costs one file, not the run.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct Output {
    pub stdout: String,
    pub stderr: String,
    /// Exit code; None when killed by a signal or by the timeout.
    pub code: Option<i32>,
    pub timed_out: bool,
    pub secs: f64,
}

/// Limits the address space of the child (Linux and other Unix systems;
/// elsewhere only the timeout applies).
#[cfg(unix)]
fn limit_memory(cmd: &mut Command, bytes: u64) {
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(move || {
            let lim = libc::rlimit { rlim_cur: bytes as libc::rlim_t, rlim_max: bytes as libc::rlim_t };
            libc::setrlimit(libc::RLIMIT_AS, &lim);
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn limit_memory(_cmd: &mut Command, _bytes: u64) {}

/// A process group of its own, so that a timeout kills its children too.
#[cfg(unix)]
fn own_group(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

#[cfg(not(unix))]
fn own_group(_cmd: &mut Command) {}

#[cfg(unix)]
fn kill(child: &mut std::process::Child) {
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.kill();
}

#[cfg(not(unix))]
fn kill(child: &mut std::process::Child) {
    let _ = child.kill();
}

/// Runs `cmd` to its end or until `timeout`, then kills it.
pub fn run(mut cmd: Command, timeout: Duration, mem: Option<u64>) -> Output {
    if let Some(m) = mem {
        limit_memory(&mut cmd, m);
    }
    own_group(&mut cmd);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let start = Instant::now();
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Output { stdout: String::new(), stderr: format!("spawn: {e}"), code: None, timed_out: false, secs: 0.0 },
    };
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    // Read both pipes while waiting, so that a big output never blocks.
    let t_out = std::thread::spawn(move || {
        let mut s = Vec::new();
        if let Some(o) = out.as_mut() {
            let _ = o.read_to_end(&mut s);
        }
        s
    });
    let t_err = std::thread::spawn(move || {
        let mut s = Vec::new();
        if let Some(e) = err.as_mut() {
            let _ = e.take(1 << 20).read_to_end(&mut s);
        }
        s
    });
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if start.elapsed() > timeout => {
                kill(&mut child);
                timed_out = true;
                break child.wait().ok();
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => break None,
        }
    };
    let stdout = String::from_utf8_lossy(&t_out.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&t_err.join().unwrap_or_default()).into_owned();
    Output { stdout, stderr, code: status.and_then(|s| s.code()), timed_out, secs: start.elapsed().as_secs_f64() }
}

/// Runs `f` on every item with `jobs` threads; results keep the order of
/// the items. `progress` is called after each one (done, total).
pub fn pool<T: Sync, R: Send>(items: &[T], jobs: usize, f: impl Fn(&T) -> R + Sync, progress: impl Fn(usize, usize) + Sync) -> Vec<R> {
    let next = Mutex::new(0usize);
    let done = Mutex::new(0usize);
    let results: Mutex<Vec<Option<R>>> = Mutex::new((0..items.len()).map(|_| None).collect());
    std::thread::scope(|s| {
        for _ in 0..jobs.max(1) {
            s.spawn(|| {
                loop {
                    let i = {
                        let mut n = next.lock().unwrap();
                        let i = *n;
                        *n += 1;
                        i
                    };
                    let Some(item) = items.get(i) else { break };
                    let r = f(item);
                    results.lock().unwrap()[i] = Some(r);
                    let d = {
                        let mut d = done.lock().unwrap();
                        *d += 1;
                        *d
                    };
                    progress(d, items.len());
                }
            });
        }
    });
    results.into_inner().unwrap().into_iter().map(|r| r.expect("every item runs")).collect()
}

/// Files under `paths` (directories walked recursively) whose extension is
/// one of `exts` (lower case, without the dot), sorted.
pub fn files(paths: &[String], exts: &[&str]) -> Vec<std::path::PathBuf> {
    fn walk(p: &std::path::Path, exts: &[&str], out: &mut Vec<std::path::PathBuf>) {
        if p.is_dir() {
            let Ok(rd) = std::fs::read_dir(p) else { return };
            for e in rd.flatten() {
                walk(&e.path(), exts, out);
            }
        } else if p.extension().and_then(|e| e.to_str()).is_some_and(|e| exts.contains(&e.to_ascii_lowercase().as_str())) {
            out.push(p.to_path_buf());
        }
    }
    let mut out = Vec::new();
    for p in paths {
        // @LIST: a file with one path per line (a subset of an earlier run).
        match p.strip_prefix('@') {
            Some(list) => {
                for l in std::fs::read_to_string(list).unwrap_or_default().lines().map(str::trim).filter(|l| !l.is_empty()) {
                    walk(std::path::Path::new(l), exts, &mut out);
                }
            }
            None => walk(std::path::Path::new(p), exts, &mut out),
        }
    }
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_keeps_order() {
        let v: Vec<u32> = (0..50).collect();
        let r = pool(&v, 4, |x| x * 2, |_, _| {});
        assert_eq!(r, (0..50).map(|x| x * 2).collect::<Vec<_>>());
    }

    #[cfg(unix)]
    #[test]
    fn run_times_out() {
        let mut c = Command::new("sh");
        c.args(["-c", "echo hi; sleep 5"]);
        let o = run(c, Duration::from_millis(300), None);
        assert!(o.timed_out);
        assert_eq!(o.stdout.trim(), "hi");
    }
}
