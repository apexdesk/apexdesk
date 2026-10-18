//! The few Windows functions a `Declare` may name that ApexDesk acts out
//! itself: timing and a beep, which macros use to wait or to measure.
//! Nothing ever calls into the system; any other declared function fails
//! when called, and the scanner still warns about it.

use std::sync::OnceLock;
use web_time::Instant;

/// (library, function) acted out, lowercase.
const HARMLESS: &[(&str, &str)] = &[
    ("kernel32", "sleep"),
    ("kernel32", "gettickcount"),
    ("kernel32", "gettickcount64"),
    ("kernel32", "queryperformancecounter"),
    ("kernel32", "queryperformancefrequency"),
    ("winmm", "timegettime"),
    ("user32", "messagebeep"),
];

/// Ticks of the performance counter in a second.
pub const FREQUENCY: f64 = 1_000_000.0;

/// Whether a declared function (its `Lib` and its `Alias` or name) is one
/// ApexDesk acts out. A library named by a path is never: it isn't the
/// system's.
pub fn harmless(lib: &str, func: &str) -> bool {
    let lib = lib.to_ascii_lowercase();
    let lib = lib.strip_suffix(".dll").unwrap_or(&lib);
    HARMLESS.iter().any(|&(l, f)| l == lib && func.eq_ignore_ascii_case(f))
}

/// Time since the first call, in seconds, standing for time since start-up.
pub fn uptime() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}
