//! The child process of the fuzzer (`apex-bench fuzz --child FILE WORKDIR`):
//! opens one file like the window does, recalculates it, saves it in every
//! format ApexDesk writes and opens those again. Each step is announced on
//! stderr (`APEX-STAGE name`); a panic or an allocation past the limit is
//! printed with its backtrace (`APEX-CRASH ...`) and ends the process with
//! a status of its own, so the parent can tell them apart from a clean
//! rejection of a broken file (status 0, `APEX-REJECTED message`).

use std::path::Path;
use std::sync::Mutex;

use apex_io::{Format, Loaded};

pub const EXIT_PANIC: i32 = 101;
pub const EXIT_MEMORY: i32 = 102;
/// A file ApexDesk wrote can't be saved or opened again.
pub const EXIT_ERROR: i32 = 103;

/// Password of the encrypted seeds made here (and of the repository's
/// samples); then the usual ones of other test suites and Excel's default.
pub const PASSWORD: &str = "secreto";
const PASSWORDS: &[&str] = &[PASSWORD, "VelvetSweatshop", "password", "pass", "test", "1234", "abc"];

static STAGE: Mutex<String> = Mutex::new(String::new());

fn stage(name: &str) {
    eprintln!("APEX-STAGE {name}");
    if let Ok(mut s) = STAGE.lock() {
        name.clone_into(&mut s);
    }
}

pub fn main(args: &[String]) -> Result<(), String> {
    let [file, work, ..] = args else { return Err("usage: fuzz --child FILE WORKDIR".into()) };
    limits();
    let limit = std::env::var("APEX_BENCH_MEM_LIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(2usize << 30);
    super::alloc::set_limit(limit);
    std::panic::set_hook(Box::new(|info| {
        let stage = STAGE.try_lock().map(|s| s.clone()).unwrap_or_default();
        let bt = std::backtrace::Backtrace::force_capture();
        eprintln!("APEX-CRASH panic stage={stage}\n{info}\n{bt}");
        std::process::exit(EXIT_PANIC);
    }));
    let (file, work) = (file.clone(), work.clone());
    // The size of a main thread's stack on Linux; set here so that the
    // result doesn't depend on the platform the fuzzer runs on.
    let t = std::thread::Builder::new().stack_size(8 << 20).spawn(move || exercise(Path::new(&file), Path::new(&work))).map_err(|e| e.to_string())?;
    let result = t.join().map_err(|_| "the worker thread panicked".to_string())?;
    eprintln!("APEX-PEAK heap {} MB, resident {} MB", super::alloc::peak() >> 20, resident_peak_kb() >> 10);
    if let Err(e) = result {
        eprintln!("APEX-ERROR {e}");
        std::process::exit(EXIT_ERROR);
    }
    Ok(())
}

/// Peak resident memory of this process in KiB (Linux; 0 elsewhere).
/// The parent can't use what wait4 says: after a fork and exec the child
/// starts with the parent's peak.
pub fn resident_peak_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find_map(|l| l.strip_prefix("VmHWM:").map(|v| v.trim().trim_end_matches("kB").trim().parse().unwrap_or(0))))
        .unwrap_or(0)
}

/// The work on one file; errors of the file itself are expected.
pub fn exercise(file: &Path, work: &Path) -> Result<(), String> {
    std::fs::create_dir_all(work).map_err(|e| e.to_string())?;
    let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if ext == "bin" {
        return vba(file);
    }
    stage("open");
    let encrypted = apex_io::is_encrypted(&std::fs::read(file).map_err(|e| e.to_string())?);
    let mut opened = apex_io::open_with(file, None);
    if encrypted {
        for pw in PASSWORDS {
            opened = apex_io::open_with(file, Some(pw));
            if !opened.as_ref().is_err_and(|e| e.wrong_password()) {
                break;
            }
        }
    }
    let (loaded, _) = match opened {
        Ok(l) => l,
        Err(e) => {
            eprintln!("APEX-REJECTED {e}");
            return Ok(());
        }
    };
    match loaded {
        Loaded::Sheet(mut wb) => {
            stage("recalc");
            wb.recalc_all();
            for fmt in [Format::Xlsx, Format::Ods, Format::Csv] {
                let out = work.join(format!("saved.{}", fmt.extension()));
                stage(&format!("save-{}", fmt.extension()));
                apex_io::save_sheet(&wb, &out, fmt).map_err(|e| format!("save {}: {e}", fmt.extension()))?;
                if fmt != Format::Csv {
                    stage(&format!("reopen-{}", fmt.extension()));
                    apex_io::open(&out).map_err(|e| format!("reopen {}: {e}", fmt.extension()))?;
                }
            }
        }
        Loaded::Doc(doc) => {
            for fmt in [Format::Docx, Format::Odt, Format::Rtf, Format::Markdown, Format::Text] {
                let out = work.join(format!("saved.{}", fmt.extension()));
                stage(&format!("save-{}", fmt.extension()));
                apex_io::save_doc(&doc, &out, fmt).map_err(|e| format!("save {}: {e}", fmt.extension()))?;
                if fmt.is_rich() || fmt == Format::Rtf {
                    stage(&format!("reopen-{}", fmt.extension()));
                    apex_io::open(&out).map_err(|e| format!("reopen {}: {e}", fmt.extension()))?;
                }
            }
        }
        Loaded::Slides(deck) => {
            let out = work.join("saved.pptx");
            stage("save-pptx");
            apex_io::save_slides(&deck, &out, Format::Pptx).map_err(|e| format!("save pptx: {e}"))?;
            stage("reopen-pptx");
            apex_io::open(&out).map_err(|e| format!("reopen pptx: {e}"))?;
        }
    }
    stage("done");
    Ok(())
}

/// A vbaProject.bin: read, change a module (so the project is written
/// again, not copied) and read the result.
fn vba(file: &Path) -> Result<(), String> {
    stage("open");
    let data = std::fs::read(file).map_err(|e| e.to_string())?;
    let mut project = match apex_io::vba::read(&data) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("APEX-REJECTED {e}");
            return Ok(());
        }
    };
    stage("save-vba");
    match project.modules.first_mut() {
        Some(m) => m.code.push_str("\nSub Fuzz()\nEnd Sub\n"),
        None => project.modules.push(apex_io::vba::VbaModule::standard("Fuzz")),
    }
    let bin = project.write().map_err(|e| format!("save vba: {e}"))?;
    stage("reopen-vba");
    apex_io::vba::read(&bin).map_err(|e| format!("reopen vba: {e}"))?;
    stage("done");
    Ok(())
}

/// No core dumps; on Linux, a cap on the address space as a backstop of
/// the allocator's limit, and a debugger may attach to print where a hung
/// child is.
#[cfg(target_os = "linux")]
fn limits() {
    unsafe {
        let none = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        libc::setrlimit(libc::RLIMIT_CORE, &none);
        let space = libc::rlimit { rlim_cur: 8 << 30, rlim_max: 8 << 30 };
        libc::setrlimit(libc::RLIMIT_AS, &space);
        libc::prctl(libc::PR_SET_PTRACER, libc::PR_SET_PTRACER_ANY, 0, 0, 0);
    }
}

#[cfg(not(target_os = "linux"))]
fn limits() {}
