//! `apexdesk run-macro`: a workbook's macro run without a window, for
//! batch work. The same security as the window, but for the question
//! (the command's `--trust` is the answer): never with macros turned off
//! by the organization, nor for files from the Internet, only signed ones
//! under "signed only", none whose code looks like malware unless asked,
//! and no file changes outside what the macro is allowed unless asked.

use std::path::Path;
use std::sync::Arc;

use apex_sheet::Workbook;
use apex_vba::{Options, Outcome, Project};

use super::{signed_book, sources, trusted};

/// What the command allows beyond running the macro.
pub(crate) struct Headless<'a> {
    /// The macro may write, delete or rename files where it reaches.
    pub allow_writes: bool,
    /// The workbook's password, kept by SaveAs.
    pub password: Option<String>,
    /// Where Debug.Print and the rest (MsgBox, what isn't allowed) go.
    pub out: Box<dyn FnMut(&str) + Send + 'a>,
    pub err: Box<dyn FnMut(&str) + Send + 'a>,
}

struct CliUi<'a> {
    allow_writes: bool,
    password: Option<String>,
    out: Box<dyn FnMut(&str) + Send + 'a>,
    err: Box<dyn FnMut(&str) + Send + 'a>,
}

impl apex_vba::Ui for CliUi<'_> {
    /// Shown on stderr; answered with the box's default button.
    fn msg_box(&mut self, prompt: &str, buttons: i32, title: &str) -> i32 {
        (self.err)(&if title.is_empty() { prompt.to_string() } else { format!("{title}: {prompt}") });
        let set: &[i32] = match buttons & 0xF {
            1 => &[1, 2],
            2 => &[3, 4, 5],
            3 => &[6, 7, 2],
            4 => &[6, 7],
            5 => &[4, 2],
            _ => &[1],
        };
        let default = ((buttons >> 8) & 3) as usize;
        set.get(default).copied().unwrap_or(set[0])
    }

    /// Nobody to type: cancelled.
    fn input_box(&mut self, prompt: &str, title: &str, _default: &str) -> Option<String> {
        (self.err)(&format!("{}: {prompt} (cancelled: no input in run-macro)", if title.is_empty() { "InputBox" } else { title }));
        None
    }

    fn debug_print(&mut self, text: &str) {
        (self.out)(&format!("{text}\n"));
    }

    fn allow_file_changes(&mut self, folder: &str) -> bool {
        if !self.allow_writes {
            (self.err)(&format!("the macro wants to change files in {folder}: not allowed (use --allow-writes)"));
        }
        self.allow_writes
    }

    fn save_workbook(&mut self, wb: &Workbook, path: &str, format: apex_vba::files::BookFormat, sheet: usize) -> Result<(), String> {
        super::write_book(wb, path, format, sheet, self.password.as_deref(), false)
    }

    fn save_opened(&mut self, wb: &Workbook, path: &str, format: apex_vba::files::BookFormat, sheet: usize) -> Result<(), String> {
        super::write_book(wb, path, format, sheet, None, false)
    }

    fn open_workbook(&mut self, path: &str, password: Option<&str>) -> Result<Workbook, String> {
        super::read_book(path, password)
    }
}

/// Why the macros of `wb` (read from `path`) may not run here, if not.
pub(crate) fn refused(path: &Path, wb: &Workbook, allow_suspicious: bool) -> Option<String> {
    let settings = crate::settings::Settings::load();
    if crate::policy::locked("MacroSecurity") && !settings.macros_enabled {
        return Some("macros are turned off by your organization".into());
    }
    if crate::app::from_internet(path) {
        return Some("the file came from the Internet: its macros don't run (save a copy you trust, or unblock it)".into());
    }
    let Some(bin) = wb.macros.project.as_ref() else { return Some("the file has no macros".into()) };
    if settings.macros_signed_only && !signed_book(wb, &settings.trusted_publishers) {
        return Some("only signed macros may run, and this file's aren't signed by a trusted publisher".into());
    }
    let srcs = sources(bin).ok()?;
    let signals = apex_vba::suspicious::scan(srcs.iter().map(|s| (s.name.as_str(), s.code.as_str())));
    if !allow_suspicious && signals.iter().any(|s| s.serious()) {
        let what: Vec<String> = signals.iter().map(|s| format!("{s:?}")).collect();
        return Some(format!("the code looks like malware ({}): not run (use --allow-suspicious if you know it)", what.join(", ")));
    }
    None
}

/// Runs macro `name` (with `args`, as typed) of `wb`, read from `path`,
/// once [`refused`] said nothing. Err: why it couldn't start.
pub(crate) fn run(path: &Path, wb: &mut Workbook, name: &str, args: &[String], h: Headless<'_>) -> Result<Outcome, String> {
    let settings = crate::settings::Settings::load();
    let bin = wb.macros.project.clone().ok_or("the file has no macros")?;
    let srcs = Arc::new(sources(&bin).map_err(|e| e.to_string())?);
    let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let file_path = apex_fs::canonicalize(path).ok().and_then(|p| p.parent().map(|d| d.to_string_lossy().into_owned())).unwrap_or_default();
    let opts = Options {
        active_sheet: wb.sheets.iter().position(|s| !s.hidden).unwrap_or(0),
        file_name,
        file_path,
        user_name: crate::doc_view::user_name(),
        utc_offset: crate::common::local_utc_offset(),
        max_stack: 48 << 20,
        // Batch work may take long; Ctrl+C ends it.
        max_time: web_time::Duration::from_secs(24 * 3600),
        max_steps: u64::MAX,
        files: apex_vba::files::FileAccess { trusted: trusted(path, &settings.trusted_locations), write: None },
        ..Options::default()
    };
    // Its functions in cells, as when its macros are enabled.
    if let Some(f) = apex_vba::udf::Functions::with_libraries(Some(srcs.clone()), &[], &opts) {
        wb.set_udf(Some(Arc::new(f) as Arc<dyn apex_sheet::udf::Udf>));
    }
    let mut ui = CliUi { allow_writes: h.allow_writes, password: h.password, out: h.out, err: h.err };
    apex_fs::thread::scope(|s| {
        apex_fs::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn_scoped(s, || {
                let project = Project::parse(&srcs).map_err(|e| e.to_string())?;
                Ok(if args.is_empty() { apex_vba::run_macro(&project, wb, &mut ui, &opts, name) } else { apex_vba::call_macro(&project, wb, &mut ui, &opts, name, args) })
            })
            .map_err(|e| e.to_string())?
            .join()
            .map_err(|_| "the macro crashed".to_string())?
    })
}
