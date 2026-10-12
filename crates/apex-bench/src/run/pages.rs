//! Our page count, from `apexdesk info --json`: the window's views lay the
//! pages out exactly as for printing (apex-app's `headless::pages`), so
//! apex-bench asks the real program instead of copying its layout.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use super::child;

/// The apexdesk binary: the one given, or the one next to apex-bench
/// (both are built into target/<profile>/).
pub fn find(given: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = given {
        return Some(PathBuf::from(p));
    }
    let exe = std::env::current_exe().ok()?;
    let p = exe.parent()?.join(format!("apexdesk{}", std::env::consts::EXE_SUFFIX));
    p.exists().then_some(p)
}

/// Pages of each file (missing: apexdesk couldn't open it).
pub fn count(apexdesk: &Path, files: &[PathBuf], timeout: Duration, mem_mb: u64) -> Result<HashMap<PathBuf, u64>, String> {
    let mut cmd = Command::new(apexdesk);
    cmd.arg("info").arg("--json").args(files);
    let exit = child::run(cmd, timeout, mem_mb).map_err(|e| e.to_string())?;
    match exit.killed {
        Some(child::Killed::Timeout) => return Err("timeout".into()),
        Some(child::Killed::Memory) => return Err("memory limit".into()),
        None => {}
    }
    if let Some(p) = exit.panic() {
        return Err(format!("panic: {p}"));
    }
    let json: serde_json::Value = serde_json::from_str(&exit.stdout).map_err(|_| format!("no output: {}", exit.stderr_tail()))?;
    let items = match json {
        serde_json::Value::Array(a) => a,
        v => vec![v],
    };
    let mut out = HashMap::new();
    for it in items {
        if let (Some(f), Some(p)) = (it["file"].as_str(), it["pages"].as_u64()) {
            out.insert(PathBuf::from(f), p);
        }
    }
    Ok(out)
}
