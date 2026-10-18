//! Functions of a workbook's code that its cells may call (`=IVA(A1)`):
//! the `Public Function`s of standard modules. They run while the cell is
//! calculated, on the workbook as it is, which they may read but not
//! change; within limits of time, steps and stack of their own, and with
//! no dialogs. An error in one gives #VALUE!.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use web_time::Duration;

use apex_sheet::addr::{CellRef, Range};
use apex_sheet::udf::{Udf, UdfArg};
use apex_sheet::{ErrorKind, Workbook};

use crate::ast::{Arg, Expr, ModuleKind, ProcKind};
use crate::interp::{Book, Interp};
use crate::object::{Obj, RangeRef};
use crate::value::Value;
use crate::{Options, Project, Source, Ui};

/// What one call may take.
const MAX_STEPS: u64 = 2_000_000;
const MAX_TIME: Duration = Duration::from_secs(5);
const MAX_STACK: usize = 1 << 20;

/// The functions of a project, and of the libraries (personal macro
/// workbook, add-ins) after it, for [`Workbook::set_udf`].
pub struct Functions {
    parts: Vec<Part>,
    file_name: String,
    file_path: String,
    user_name: String,
    utc_offset: i64,
}

/// The functions of one project.
struct Part {
    sources: Arc<Vec<Source>>,
    /// As the code writes them, with their parameters.
    names: Vec<(String, Vec<String>)>,
    /// Those that call `Application.Volatile` (lower case).
    volatile: Vec<String>,
}

/// Parsed projects kept on one thread.
const CACHED: usize = 8;

thread_local! {
    /// The projects last parsed on this thread, the latest last.
    static PARSED: RefCell<Vec<(Arc<Vec<Source>>, Rc<Project>)>> = const { RefCell::new(Vec::new()) };
    /// A function is running on this thread (they don't call each other
    /// through cells).
    static RUNNING: Cell<bool> = const { Cell::new(false) };
}

impl Part {
    fn new(sources: Arc<Vec<Source>>) -> Option<Part> {
        let project = Project::parse(&sources).ok()?;
        let names: Vec<(String, Vec<String>)> = project
            .modules
            .iter()
            .filter(|m| m.kind == ModuleKind::Standard)
            .flat_map(|m| m.procs.iter().filter(|p| p.public && p.kind == ProcKind::Function).map(|p| (p.name.clone(), p.params.iter().map(|a| if a.optional { format!("[{}]", a.name) } else { a.name.clone() }).collect())))
            .collect();
        if names.is_empty() {
            return None;
        }
        let volatile = sources.iter().filter(|s| s.kind == ModuleKind::Standard).flat_map(|s| volatile_functions(&s.code)).collect();
        Some(Part { sources, names, volatile })
    }

    fn has(&self, name: &str) -> bool {
        self.names.iter().any(|(n, _)| n.eq_ignore_ascii_case(name))
    }
}

impl Functions {
    /// The functions of `sources` (None when there are none or the code
    /// doesn't parse). `opts` gives the file's name and the user.
    pub fn new(sources: Arc<Vec<Source>>, opts: &Options) -> Option<Functions> {
        Functions::with_libraries(Some(sources), &[], opts)
    }

    /// The functions of `sources`, if any, then those of `libs` a name
    /// not taken yet finds.
    pub fn with_libraries(sources: Option<Arc<Vec<Source>>>, libs: &[crate::Library], opts: &Options) -> Option<Functions> {
        let parts: Vec<Part> = sources.into_iter().chain(libs.iter().map(|l| l.sources.clone())).filter_map(Part::new).collect();
        if parts.is_empty() {
            return None;
        }
        Some(Functions {
            parts,
            file_name: opts.file_name.clone(),
            file_path: opts.file_path.clone(),
            user_name: opts.user_name.clone(),
            utc_offset: opts.utc_offset,
        })
    }

    /// The project that has `name`.
    fn part(&self, name: &str) -> Option<&Part> {
        self.parts.iter().find(|p| p.has(name))
    }

    fn project(sources: &Arc<Vec<Source>>) -> Option<Rc<Project>> {
        PARSED.with(|p| {
            let mut p = p.borrow_mut();
            if let Some(i) = p.iter().position(|(s, _)| Arc::ptr_eq(s, sources)) {
                let hit = p.remove(i);
                let project = hit.1.clone();
                p.push(hit);
                return Some(project);
            }
            let project = Rc::new(Project::parse(sources).ok()?);
            if p.len() >= CACHED {
                p.remove(0);
            }
            p.push((sources.clone(), project.clone()));
            Some(project)
        })
    }

    fn run(&self, wb: &Workbook, name: &str, args: Vec<UdfArg>, sheet: usize, cell: CellRef) -> Option<apex_sheet::Value> {
        let project = Functions::project(&self.part(name)?.sources)?;
        let (mi, pi) = project.find(name)?;
        let m = &project.modules[mi];
        let proc = m.procs[pi].clone();
        if m.kind != ModuleKind::Standard || !proc.public || proc.kind != ProcKind::Function || args.len() > proc.params.len() && !proc.params.last().is_some_and(|p| p.paramarray) {
            return None;
        }
        let opts = Options {
            active_sheet: sheet,
            selection: Range::single(cell),
            file_name: self.file_name.clone(),
            file_path: self.file_path.clone(),
            user_name: self.user_name.clone(),
            utc_offset: self.utc_offset,
            max_steps: MAX_STEPS,
            max_time: MAX_TIME,
            max_stack: MAX_STACK,
            ..Options::default()
        };
        let mut ui = Quiet;
        let mut it = Interp::with_book(&project, Book::Ref(wb), &mut ui, &opts).ok()?;
        it.caller = Some(RangeRef::new(sheet, Range::single(cell)));
        let mut list: Vec<Arg> = args
            .into_iter()
            .map(|a| Arg {
                name: None,
                expr: match a {
                    UdfArg::Value(v) => Some(Expr::Lit(it.from_cell_value(&v, false))),
                    UdfArg::Range { sheet, range } => Some(Expr::Lit(Value::Object(Obj::Range(RangeRef::new(sheet, range))))),
                    UdfArg::Missing => None,
                },
            })
            .collect();
        while list.last().is_some_and(|a| a.expr.is_none()) {
            list.pop();
        }
        let v = it.call_proc(mi, proc, &list).ok()?;
        it.to_cell_value(v).ok()
    }
}

impl Udf for Functions {
    fn has(&self, name: &str) -> bool {
        self.part(name).is_some()
    }

    fn is_volatile(&self, name: &str) -> bool {
        self.part(name).is_some_and(|p| p.volatile.iter().any(|n| n.eq_ignore_ascii_case(name)))
    }

    fn names(&self) -> Vec<(String, Vec<String>)> {
        let mut out: Vec<(String, Vec<String>)> = Vec::new();
        for (n, params) in self.parts.iter().flat_map(|p| &p.names) {
            if !out.iter().any(|(o, _)| o.eq_ignore_ascii_case(n)) {
                out.push((n.clone(), params.clone()));
            }
        }
        out
    }

    fn call(&self, wb: &Workbook, name: &str, args: Vec<UdfArg>, sheet: usize, cell: CellRef) -> apex_sheet::Value {
        if RUNNING.with(Cell::get) {
            return apex_sheet::Value::Error(ErrorKind::Value);
        }
        RUNNING.with(|r| r.set(true));
        // A panic in the code under it doesn't leave the flag set.
        struct Done;
        impl Drop for Done {
            fn drop(&mut self) {
                RUNNING.with(|r| r.set(false));
            }
        }
        let _done = Done;
        // The interpreter's thread-wide settings (the locale of values)
        // are the caller's again afterwards.
        let locale = crate::value::locale();
        let v = self.run(wb, name, args, sheet, cell);
        crate::value::set_locale(locale);
        v.unwrap_or(apex_sheet::Value::Error(ErrorKind::Value))
    }
}

/// No dialogs from a function in a cell: MsgBox gives OK, InputBox
/// nothing.
struct Quiet;

impl Ui for Quiet {
    fn msg_box(&mut self, _prompt: &str, _buttons: i32, _title: &str) -> i32 {
        1
    }

    fn input_box(&mut self, _prompt: &str, _title: &str, _default: &str) -> Option<String> {
        None
    }
}

/// The Functions of a module's code that call `Application.Volatile`
/// (not `Application.Volatile False`), lower case.
fn volatile_functions(code: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for line in code.lines() {
        let t = line.trim().to_ascii_lowercase();
        let words: Vec<&str> = t.split(|c: char| c.is_whitespace() || c == '(').filter(|w| !w.is_empty()).collect();
        let skip = words.iter().take_while(|w| matches!(**w, "public" | "private" | "friend" | "static")).count();
        if words.get(skip) == Some(&"function") {
            current = words.get(skip + 1).map(|n| n.to_string());
        } else if words.first() == Some(&"end") && words.get(1) == Some(&"function") {
            current = None;
        } else if let Some(f) = &current
            && let Some(rest) = t.split('\'').next().and_then(|c| c.trim().strip_prefix("application.volatile"))
            && !rest.trim().starts_with("false")
            && (rest.is_empty() || rest.starts_with(' ') || rest.starts_with('('))
            && !out.contains(f)
        {
            out.push(f.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volatile_ones() {
        let code = "Function A()\n  Application.Volatile\nEnd Function\nPublic Function B(x)\n  Application.Volatile False\nEnd Function\nFunction C()\n  ' Application.Volatile\n  Application.Volatile (True)\nEnd Function\nFunction D()\nEnd Function\n";
        assert_eq!(volatile_functions(code), ["a", "c"]);
    }
}
