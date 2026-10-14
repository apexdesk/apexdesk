//! The command line: `apexdesk <command> ...` converts, prints, computes
//! and merges files without a window, and `apexdesk tui` edits one in the
//! terminal. Files are read and written by apex-io like in the window, PDF
//! pages are painted like the print dialog's (see [`crate::headless`]).
//!
//! Exit status: 0 success, 1 an error (a file failed), 2 wrong usage.

#[cfg_attr(not(feature = "empresa"), path = "cli/api_free.rs")]
mod api;
mod args;
mod console;
mod glob;
mod inspect;
#[cfg_attr(not(feature = "empresa"), path = "cli/inventory_free.rs")]
mod inventory;
mod policy;
mod secret;
mod show;
mod sign;
mod sign_key;

use std::io::Write;
use std::path::{Path, PathBuf};

use apex_doc::Document;
use apex_io::{Format, Kind, Loaded};
use apex_sheet::{CellRef, Locale, Range, Workbook};
use apex_tui::Book;

use args::{Parsed, Spec};
use show::Shown;

const COMMANDS: &[&str] = &["convert", "cat", "eval", "get", "set", "recalc", "run-macro", "merge", "info", "sign", "verify", "extend", "inventory", "inspect", "policy", "api", "mcp", "tui", "help"];

/// The command the arguments start, if any.
fn command(argv: &[String]) -> Option<&'static str> {
    match argv.first()?.as_str() {
        "--tui" => Some("tui"),
        "-h" | "--help" => Some("help"),
        "-V" | "--version" => Some("version"),
        c => COMMANDS.iter().find(|k| **k == c).copied(),
    }
}

/// Runs the command line when the first argument is a command (or
/// `--help`, `--version`, `--tui`); `None` starts the window.
pub fn run(argv: &[String]) -> Option<i32> {
    let cmd = command(argv)?;
    console::attach(cmd == "tui");
    if !matches!(cmd, "help" | "version" | "policy") {
        // Shared templates, dictionaries and roots, Get Data turned off...
        crate::settings::Settings::load().apply_globals();
    }
    let rest = &argv[1..];
    let code = match dispatch(cmd, rest) {
        Ok(code) => code,
        Err(Fail::Usage(m)) => {
            say(&format!("apexdesk {cmd}: {m}"));
            say(&format!("Try 'apexdesk {cmd} --help'."));
            2
        }
        Err(Fail::Error(m)) => {
            say(&format!("apexdesk: {m}"));
            1
        }
    };
    let _ = std::io::stdout().flush();
    Some(code)
}

enum Fail {
    /// Wrong arguments (exit status 2).
    Usage(String),
    /// The command failed (exit status 1).
    Error(String),
}

type Res<T> = Result<T, Fail>;

fn usage<T>(m: impl Into<String>) -> Res<T> {
    Err(Fail::Usage(m.into()))
}

fn error<T>(m: impl Into<String>) -> Res<T> {
    Err(Fail::Error(m.into()))
}

/// Writes to standard output; a closed pipe (`| head`) is not an error.
/// On a terminal, control characters from a document show as U+FFFD:
/// escape sequences in a cell could otherwise set the clipboard or the
/// title, or make the terminal answer. Escapes go everywhere (a pipe's
/// end, `| head` or `| tee log`, is often a terminal too).
fn emit(s: &str) {
    use std::io::IsTerminal;
    let out = std::io::stdout();
    let tty = out.is_terminal();
    let odd = |c: char| c.is_control() && c != '\n' && c != '\t' && (tty || c == '\u{1B}' || ('\u{80}'..='\u{9F}').contains(&c));
    if s.chars().any(odd) {
        let clean: String = s.chars().map(|c| if odd(c) { '\u{FFFD}' } else { c }).collect();
        let _ = out.lock().write_all(clean.as_bytes());
        return;
    }
    let _ = out.lock().write_all(s.as_bytes());
}

/// Writes a message to standard error, its control characters (from a
/// document's names, as in [`emit`]) shown as U+FFFD.
fn say(s: &str) {
    let clean: String = s.chars().map(|c| if c.is_control() && c != '\n' && c != '\t' { '\u{FFFD}' } else { c }).collect();
    eprintln!("{clean}");
}

fn dispatch(cmd: &str, rest: &[String]) -> Res<i32> {
    let (spec, help): (Spec, &str) = match cmd {
        "version" => {
            emit(&format!("apexdesk {}\n", env!("CARGO_PKG_VERSION")));
            return Ok(0);
        }
        "help" => {
            let topic = rest.first().map(String::as_str).unwrap_or("");
            match COMMANDS.iter().find(|c| **c == topic && topic != "help") {
                Some(c) => emit(help_of(c)),
                None if topic.is_empty() => emit(&main_help()),
                None => return usage(format!("unknown command '{topic}'")),
            }
            return Ok(0);
        }
        "convert" => (Spec { values: &["to", "out", "output", "sheet", "password", "encoding", "from-encoding", "handouts", "width"], flags: &["quiet", "pdfa", "notes"] }, CONVERT_HELP),
        "cat" => (Spec { values: &["sheet", "range", "format", "width", "password"], flags: &["formulas", "raw"] }, CAT_HELP),
        "eval" => (Spec { values: &["sheet", "password"], flags: &[] }, EVAL_HELP),
        "get" => (Spec { values: &["sheet", "format", "password"], flags: &["raw", "formulas"] }, GET_HELP),
        "set" => (Spec { values: &["sheet", "output", "password"], flags: &[] }, SET_HELP),
        "recalc" => (Spec { values: &["output", "password"], flags: &[] }, RECALC_HELP),
        "run-macro" => (Spec { values: &["output", "password"], flags: &["trust", "save", "allow-writes", "allow-suspicious"] }, RUN_MACRO_HELP),
        "merge" => (Spec { values: &["out", "sheet", "name", "query", "sql", "password-env"], flags: &["pdf", "single", "quiet", "allow-remote"] }, MERGE_HELP),
        "info" => (Spec { values: &["password"], flags: &["json", "page-starts"] }, INFO_HELP),
        "sign" => (Spec { values: &["cert", "password-file", "password-env", "reason", "tsa", "level", "output", "pkcs11", "slot", "cert-label", "cert-subject", "cert-sha1", "pin-env"], flags: &["quiet", "store", "list"] }, sign::SIGN_HELP),
        "verify" => (Spec { values: &["password"], flags: &["json"] }, sign::VERIFY_HELP),
        "extend" => (Spec { values: &["level", "tsa", "output"], flags: &["quiet"] }, sign::EXTEND_HELP),
        "inventory" => (Spec { values: &["output"], flags: &["quiet", "no-recurse"] }, inventory::INVENTORY_HELP),
        "inspect" => (Spec { values: &["remove", "output", "password"], flags: &["json"] }, inspect::INSPECT_HELP),
        "policy" => (Spec { values: &["templates"], flags: &["list"] }, policy::POLICY_HELP),
        "api" => (Spec { values: &[], flags: &[] }, api::API_HELP),
        "mcp" => (Spec { values: &[], flags: &[] }, crate::mcp::server::MCP_HELP),
        "tui" => (Spec { values: &[], flags: &[] }, TUI_HELP),
        _ => unreachable!(),
    };
    let a = args::parse(rest, &spec).map_err(Fail::Usage)?;
    if a.flag("help") {
        emit(help);
        return Ok(0);
    }
    match cmd {
        "convert" => convert(&a),
        "cat" => cat(&a),
        "eval" => eval(&a),
        "get" => get(&a),
        "set" => set(&a),
        "recalc" => recalc(&a),
        "run-macro" => run_macro(&a),
        "merge" => merge(&a),
        "info" => info(&a),
        "sign" => sign::sign(&a),
        "verify" => sign::verify(&a),
        "extend" => sign::extend(&a),
        "inventory" if !crate::license::has("management") => error("inventory needs ApexDesk Empresa and a business license"),
        "inventory" => inventory::inventory(&a),
        "inspect" => inspect::inspect(&a),
        "policy" => policy::policy(&a),
        "api" if !crate::license::has("management") => error("api needs ApexDesk Empresa and a business license"),
        "api" if !crate::settings::Settings::load().automation_api => error("the automation API is off: turn it on in File > AI Assistants (MCP) (or with the AllowAutomationAPI policy)"),
        "api" => api::run(),
        "mcp" => Ok(crate::mcp::server::run()),
        "tui" => tui(&a),
        _ => unreachable!(),
    }
}

// --- help ----------------------------------------------------------------------------

fn main_help() -> String {
    format!(
        "ApexDesk {}: spreadsheets and documents.

Usage:
  apexdesk [FILE...]          open files in the window
  apexdesk tui [FILE]         edit a file in the terminal (also: --tui)
  apexdesk <command> [ARGS]   work on files without a window

Commands:
  convert   convert files to another format (pdf, xlsx, docx, csv, md...)
  cat       print the cells of a spreadsheet or the text of a document
  eval      compute a formula
  get       print cell values
  set       change cells and save
  recalc    recalculate every formula and save
  run-macro run a macro of a workbook (.xlsm) and save
  merge     mail merge a document with a spreadsheet, CSV file or database
  info      describe files (type, sheets, pages, author, macros...)
  sign      sign a document with a certificate file (.p12, .pfx)
  verify    check the digital signatures of files
  extend    make signatures verifiable in the long term (B-LT, B-LTA)
  inventory list office files with macros, links, signatures, passwords
  inspect   report what a file carries that doesn't show; remove it
  policy    show the organization's policies; write their templates
  api       automation for scripts: JSON requests on stdin (business)
  mcp       let an AI assistant edit the open files (MCP server)
  help      show the help of a command

Run 'apexdesk <command> --help' for its options.
Exit status: 0 success, 1 error, 2 wrong usage.
",
        env!("CARGO_PKG_VERSION")
    )
}

fn help_of(cmd: &str) -> &'static str {
    match cmd {
        "convert" => CONVERT_HELP,
        "cat" => CAT_HELP,
        "eval" => EVAL_HELP,
        "get" => GET_HELP,
        "set" => SET_HELP,
        "recalc" => RECALC_HELP,
        "run-macro" => RUN_MACRO_HELP,
        "merge" => MERGE_HELP,
        "info" => INFO_HELP,
        "sign" => sign::SIGN_HELP,
        "verify" => sign::VERIFY_HELP,
        "extend" => sign::EXTEND_HELP,
        "inventory" => inventory::INVENTORY_HELP,
        "inspect" => inspect::INSPECT_HELP,
        "policy" => policy::POLICY_HELP,
        "api" => api::API_HELP,
        "mcp" => crate::mcp::server::MCP_HELP,
        _ => TUI_HELP,

    }
}

const CONVERT_HELP: &str = "Usage: apexdesk convert FILE... --to FORMAT [options]

Converts each file (wildcards like *.xlsx work on every system).

Formats:
  spreadsheets  xlsx ods fods csv tsv pdf html md txt
  documents     docx odt fodt rtf md txt pdf html epub
  presentations pptx pdf png html md txt
Input may also be xls, xlsb, doc, and WPS Office et and wps.

Options:
  -t, --to FORMAT     output format (required)
  -d, --out DIR       folder for the new files (default: next to each file)
  -o, --output FILE   output file name (one input file only)
  -s, --sheet SHEET   sheet for csv, tsv, md and txt: number (1 = first),
                      name, or 'all' for one file per sheet
  -p, --password PW   password of encrypted files (kept in xlsx, ods, docx
                      and odt)
      --password-env VAR
                      the password is in environment variable VAR
  -q, --quiet         do not list the files written
      --pdfa          pdf: write PDF/A-2b, for archiving (documents are
                      always tagged, for screen readers)
      --encoding ENC  csv, tsv, txt and md: encoding to write (default
                      utf-8): utf-8, utf-8-bom, utf-16le, utf-16be,
                      windows-1252, cp850, iso-8859-1, iso-8859-15
      --from-encoding ENC
                      csv, tsv, txt and md input: its encoding (default:
                      detected)
      --notes         presentations to pdf: notes pages
      --handouts N    presentations to pdf: handouts of N slides a page
                      (1, 2, 3, 4, 6 or 9)
      --width PX      presentations to png: picture width (default 1920);
                      one file per slide, NAME-1.png, NAME-2.png...

A file is never written over its input.
";

const CAT_HELP: &str = "Usage: apexdesk cat FILE [options]

Prints the cells of a spreadsheet, or the text of a document.

Options:
  -s, --sheet SHEET   sheet number (1 = first) or name
  -r, --range RANGE   cells to print, like A1:D20 (default: the used area)
  -f, --format FMT    spreadsheets: table (default), csv, tsv, json, md
                      documents: text (default), md, plain, html
      --raw           values without number formats
      --formulas      formulas instead of their values
  -w, --width N       documents: wrap text at N columns (default 80)
  -p, --password PW   password of an encrypted file
      --password-env VAR
                      the password is in environment variable VAR
";

const EVAL_HELP: &str = "Usage: apexdesk eval [FILE] FORMULA [options]

Computes a formula on the cells of FILE (or on an empty workbook) and
prints the result. Formulas use English function names and ',' between
arguments: 'SUM(B2:B10)/2', '=AVERAGE(Sheet2!A:A)'. Nothing is saved.

Options:
  -s, --sheet SHEET   sheet the formula is on (default: the first)
  -p, --password PW   password of an encrypted file
      --password-env VAR
                      the password is in environment variable VAR
";

const GET_HELP: &str = "Usage: apexdesk get FILE CELL... [options]

Prints the value of cells or ranges: B3, A1:C10, Sheet2!D4 or a defined
name. A range prints one row per line.

Options:
  -s, --sheet SHEET   sheet number (1 = first) or name
  -f, --format FMT    ranges: tsv (default), csv, json, table, md
      --raw           values without number formats
      --formulas      formulas instead of their values
  -p, --password PW   password of an encrypted file
      --password-env VAR
                      the password is in environment variable VAR
";

const SET_HELP: &str = "Usage: apexdesk set FILE CELL VALUE [CELL VALUE...] [options]

Changes cells as if typed and saves the file (formulas recalculate).
Values use English syntax: 3.5, TRUE, 2024-05-31, =SUM(A1:A3).
Protected cells are not changed.

Options:
  -s, --sheet SHEET   sheet number (1 = first) or name
  -o, --output FILE   save to another file (format by its extension)
  -p, --password PW   password of an encrypted file (kept when saving)
      --password-env VAR
                      the password is in environment variable VAR
";

const RECALC_HELP: &str = "Usage: apexdesk recalc FILE [options]

Recalculates every formula of a spreadsheet and saves it.

Options:
  -o, --output FILE   save to another file (format by its extension)
  -p, --password PW   password of an encrypted file (kept when saving)
      --password-env VAR
                      the password is in environment variable VAR
";

const RUN_MACRO_HELP: &str = "Usage: apexdesk run-macro FILE MACRO [ARG...] --trust [options]

Runs a VBA macro of a workbook without a window: a Sub by name
(Module1.Main also works), with ARGs for its parameters as typed in Run
Macro: a number, TRUE or FALSE, else text (no quotes). Debug.Print goes
to standard output; MsgBox to standard error, answered with its default
button; InputBox is cancelled.

Macros run only with --trust, saying you trust this file's code. Even
then they don't run when your organization turned macros off, for files
from the Internet, when only signed macros may run and these aren't, or
when the code looks like malware. The macro reaches the files in the
workbook's folder (anywhere in a trusted location) and changes none
without --allow-writes.

Options:
      --trust             run the file's macros (required)
  -o, --output FILE       save the workbook to FILE after the macro
      --save              save the workbook over FILE after the macro
                          (also saved when the macro calls Save)
      --allow-writes      let the macro write, delete or rename files
      --allow-suspicious  run code that looks like malware anyway
  -p, --password PW       password of an encrypted file (kept when saving)
      --password-env VAR
                          the password is in environment variable VAR

Exit status: 0 the macro ended, 1 it failed (its error and line on
standard error) or didn't run, 2 wrong usage.
";

const MERGE_HELP: &str = "Usage: apexdesk merge TEMPLATE DATA [options]

Mail merge: the template is a document with fields written as \u{ab}Name\u{bb}
(as the Mailings tab inserts them); DATA is a spreadsheet or CSV file
whose first row names the fields. Writes one document per record, in the
template's format, keeping all its formatting.

Options:
  -d, --out DIR       folder for the documents (default: current folder)
      --pdf           write PDF files instead
      --single        one document with every record, each on a new page
      --name PATTERN  file names: {n} is the record number, {Field} a
                      field's value (default: TEMPLATE-{n})
  -s, --sheet SHEET   sheet of DATA (default: the first)
      --query NAME    DATA is a workbook: use what its Get Data query NAME
                      gives (run now)
      --allow-remote  let that query connect to other computers, the web
                      or a database server, or read files (the workbook's
                      queries may name any)
      --sql SQL       DATA is a database: a SQLite file, or a server as
                      postgres://USER@HOST[:PORT]/DB (also mysql://,
                      sqlserver://); SQL is a SELECT or a table name
      --password-env VAR
                      the database password is in environment variable VAR
                      (on Windows, sqlserver:// without a user signs in
                      with the Windows session)
  -q, --quiet         do not list the files written
";

const INFO_HELP: &str = "Usage: apexdesk info FILE... [options]

Describes files: format, size, sheets and their used area, pages,
words, properties (title, author...), macros and encryption.

Options:
      --json          print JSON
      --page-starts   documents: where each page starts (paragraph number,
                      from 0, and whether it goes on from the page before)
  -p, --password PW   password of encrypted files
      --password-env VAR
                      the password is in environment variable VAR
";

const TUI_HELP: &str = "Usage: apexdesk tui [FILE]   (or: apexdesk --tui [FILE])

Opens a spreadsheet or document in the terminal, or a new workbook.
A FILE that does not exist is created on saving (its extension says
whether it is a spreadsheet or a document).
F1 shows the keys, F10 or Ctrl+P every command.
";

// --- common --------------------------------------------------------------------------

/// Opens a file; `--password` for encrypted ones.
fn open(path: &Path, a: &Parsed) -> Res<(Loaded, Format)> {
    apex_io::open_with(path, a.value("password")).map_err(|e| {
        let hint = if e.needs_password() { " (use --password)" } else { "" };
        Fail::Error(format!("{}: {e}{hint}", path.display()))
    })
}

fn open_sheet(path: &Path, a: &Parsed) -> Res<(Workbook, Format)> {
    match open(path, a)? {
        (Loaded::Sheet(wb), f) => Ok((*wb, f)),
        (Loaded::Doc(_) | Loaded::Slides(_), _) => error(format!("{}: not a spreadsheet", path.display())),
    }
}

/// The sheet `--sheet` names: a number from 1, or a name.
fn pick_sheet(wb: &Workbook, spec: Option<&str>) -> Res<usize> {
    let Some(s) = spec else { return Ok(wb.sheets.iter().position(|s| !s.hidden).unwrap_or(0)) };
    if let Some(i) = wb.sheet_index(s) {
        return Ok(i);
    }
    match s.parse::<usize>() {
        Ok(n) if n >= 1 && n <= wb.sheets.len() => Ok(n - 1),
        _ => error(format!("no sheet '{s}' (sheets: {})", wb.sheets.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", "))),
    }
}

/// A cell, range or defined name, maybe with a sheet ("Data!A1:B4").
fn target(wb: &Workbook, sheet: usize, text: &str) -> Res<(usize, Range)> {
    let (sheet, r) = match text.rsplit_once('!') {
        Some((s, r)) => {
            let name = s.trim_matches('\'').replace("''", "'");
            (wb.sheet_index(&name).ok_or_else(|| Fail::Error(format!("no sheet '{name}'")))?, r)
        }
        None => (sheet, text),
    };
    let upper = r.replace('$', "").to_ascii_uppercase();
    if let Some(range) = Range::parse(&upper) {
        return Ok((sheet, range));
    }
    // Whole columns and rows ("A:C", "2:5") within the used area.
    if let Some((x, y)) = upper.split_once(':') {
        let used = wb.sheets[sheet].extent().unwrap_or_default();
        if let (Some(c0), Some(c1)) = (apex_sheet::addr::parse_col(x), apex_sheet::addr::parse_col(y)) {
            return Ok((sheet, Range::new(CellRef::new(0, c0), CellRef::new(used.row, c1))));
        }
        if let (Ok(r0), Ok(r1)) = (x.parse::<u32>(), y.parse::<u32>())
            && r0 >= 1
            && r1 >= 1
        {
            return Ok((sheet, Range::new(CellRef::new(r0 - 1, 0), CellRef::new(r1 - 1, used.col))));
        }
    }
    wb.name_target(sheet, r).ok_or_else(|| Fail::Error(format!("'{text}' is not a cell, range or name")))
}

/// The used area of a sheet (from A1), if it has cells, cut to the rows
/// a grid of text can hold: a lone cell far away would make billions.
fn used(wb: &Workbook, sheet: usize) -> Option<Range> {
    const MAX_CELLS: u32 = 10_000_000;
    wb.sheets[sheet].extent().map(|e| Range::new(CellRef::new(0, 0), CellRef::new(e.row.min(MAX_CELLS / (e.col + 1) - 1), e.col)))
}

fn stem(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "out".into())
}

/// Whether two paths are the same file.
fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// Saves a workbook in the format of `path` (or `fmt`).
fn save_sheet(wb: &Workbook, path: &Path, fmt: Format, a: &Parsed) -> Res<()> {
    if !fmt.can_save() || fmt.kind() != Kind::Sheet {
        return error(format!("{}: can't save as .{}; use --output with another extension", path.display(), fmt.extension()));
    }
    apex_io::save_sheet_with(wb, path, fmt, a.value("password").filter(|_| apex_io::can_encrypt(fmt))).map_err(|e| Fail::Error(format!("{}: {e}", path.display())))
}

/// Where `set` and `recalc` save: `--output`, or the file itself.
fn output_of(path: &Path, fmt: Format, a: &Parsed) -> Res<(PathBuf, Format)> {
    match a.value("output") {
        Some(o) => {
            let o = PathBuf::from(o);
            let f = Format::from_path(&o).ok_or_else(|| Fail::Usage(format!("unknown format of '{}'", o.display())))?;
            Ok((o, f))
        }
        None => Ok((path.to_path_buf(), fmt)),
    }
}

// --- convert -------------------------------------------------------------------------

/// What `convert` writes.
#[derive(Clone, Copy, PartialEq)]
enum Target {
    File(Format),
    Pdf,
    Html,
    Epub,
    Png,
}

impl Target {
    fn parse(s: &str) -> Option<Target> {
        let s = s.trim_start_matches('.').to_ascii_lowercase();
        Some(match s.as_str() {
            "pdf" => Target::Pdf,
            "html" | "htm" => Target::Html,
            "epub" => Target::Epub,
            "png" => Target::Png,
            "text" | "plain" => Target::File(Format::Text),
            _ if apex_io::CODE_EXTS.contains(&s.as_str()) => return None,
            _ => Target::File(Format::from_path(Path::new(&format!("x.{s}"))).filter(|f| f.can_save())?),
        })
    }

    fn ext(self) -> &'static str {
        match self {
            Target::File(f) => f.extension(),
            Target::Pdf => "pdf",
            Target::Html => "html",
            Target::Epub => "epub",
            Target::Png => "png",
        }
    }
}

fn convert(a: &Parsed) -> Res<i32> {
    let Some(to) = a.value("to") else { return usage("missing --to FORMAT") };
    let target = Target::parse(to).ok_or_else(|| Fail::Usage(format!("unknown format '{to}'")))?;
    crate::headless::set_pdfa(a.flag("pdfa"));
    if a.pos.is_empty() {
        return usage("no files given");
    }
    let mut files = Vec::new();
    for p in &a.pos {
        match glob::expand(p) {
            Ok(v) => files.extend(v),
            Err(e) => return error(e),
        }
    }
    if a.value("output").is_some() && files.len() > 1 {
        return usage("--output needs a single input file; use --out DIR");
    }
    if let Some(dir) = a.value("out") {
        std::fs::create_dir_all(dir).map_err(|e| Fail::Error(format!("{dir}: {e}")))?;
    }
    let mut failed = 0;
    for f in &files {
        if let Err(Fail::Error(e) | Fail::Usage(e)) = convert_one(f, target, a) {
            say(&format!("apexdesk: {e}"));
            failed += 1;
        }
    }
    Ok(i32::from(failed > 0))
}

/// The encoding named by option `name`, if given.
fn encoding_of(a: &Parsed, name: &str) -> Res<Option<apex_io::Encoding>> {
    match a.value(name) {
        None => Ok(None),
        Some(s) => apex_io::Encoding::from_key(s).map(Some).ok_or_else(|| Fail::Usage(format!("unknown encoding '{s}' (see apexdesk help convert)"))),
    }
}

thread_local! {
    /// The files a conversion names as its output.
    static OUTPUTS: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Converts one file; what comes from a downloaded one is marked as such.
fn convert_one(path: &Path, target: Target, a: &Parsed) -> Res<()> {
    OUTPUTS.with(|o| o.borrow_mut().clear());
    let res = convert_file_to(path, target, a);
    let outputs = OUTPUTS.with(|o| std::mem::take(&mut *o.borrow_mut()));
    if crate::app::from_internet(path) {
        for out in outputs.iter().filter(|o| o.is_file()) {
            apex_io::safe_path::mark_untrusted(out);
        }
    }
    res
}

fn convert_file_to(path: &Path, target: Target, a: &Parsed) -> Res<()> {
    let to_enc = encoding_of(a, "encoding")?;
    let loaded = match (encoding_of(a, "from-encoding")?, Format::from_path(path)) {
        (Some(enc), Some(f)) if apex_io::is_text_format(f) => apex_io::open_text(path, f, Some(enc), None).map_err(|e| Fail::Error(format!("{}: {e}", path.display())))?.0,
        _ => open(path, a)?.0,
    };
    let text_out = |s: String| match to_enc {
        Some(e) => e.encode(&s).0,
        None => s.into_bytes(),
    };
    let dir = match a.value("out") {
        Some(d) => PathBuf::from(d),
        None => path.parent().map(Path::to_path_buf).unwrap_or_default(),
    };
    let name = stem(path);
    let dest = |suffix: &str| -> PathBuf {
        let p = match a.value("output") {
            Some(o) if suffix.is_empty() => PathBuf::from(o),
            _ => dir.join(format!("{name}{suffix}.{}", target.ext())),
        };
        OUTPUTS.with(|o| o.borrow_mut().push(p.clone()));
        p
    };
    let mut written: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    // A workbook goes to PDF without a copy (a view takes it).
    let loaded = match (target, loaded) {
        (Target::Pdf, Loaded::Sheet(wb)) => {
            let out = dest("");
            check_not_input(path, &out)?;
            let bytes = crate::headless::sheet_pdf(crate::sheet_view::SheetView::new(*wb), &name);
            apex_io::write_atomic(&out, &bytes).map_err(|e| Fail::Error(format!("{}: {e}", out.display())))?;
            report(path, &out, a);
            return Ok(());
        }
        (_, l) => l,
    };
    match &loaded {
        Loaded::Sheet(wb) => {
            let per_sheet = matches!(target, Target::File(Format::Csv | Format::Tsv | Format::Markdown | Format::Text));
            let sheets: Vec<usize> = match a.value("sheet") {
                Some("all") if per_sheet => (0..wb.sheets.len()).collect(),
                s => vec![pick_sheet(wb, s)?],
            };
            for &i in &sheets {
                let bytes = match target {
                    Target::File(Format::Csv) => text_out(apex_io::csv::write(wb, i, None)),
                    Target::File(Format::Tsv) => text_out(apex_io::csv::write(wb, i, Some('\t'))),
                    Target::File(Format::Markdown) => text_out(used(wb, i).map(|r| show::markdown(&show::grid(wb, i, r, Shown::Display))).unwrap_or_default()),
                    Target::File(Format::Text) => text_out(used(wb, i).map(|r| show::table(wb, i, r, &show::grid(wb, i, r, Shown::Display))).unwrap_or_default()),
                    Target::File(f @ (Format::Xlsx | Format::Ods | Format::Fods)) => {
                        let out = dest("");
                        check_not_input(path, &out)?;
                        save_sheet(wb, &out, f, a)?;
                        report(path, &out, a);
                        return Ok(());
                    }
                    Target::Pdf => crate::headless::pdf(Book::Sheet(wb), &name),
                    Target::Html => apex_io::html::sheet_html(wb, &name).into_bytes(),
                    _ => return error(format!("{}: a spreadsheet can't be converted to {}", path.display(), target.ext())),
                };
                let suffix = if sheets.len() > 1 { format!("-{}", safe_name(&wb.sheets[i].name)) } else { String::new() };
                written.push((dest(&suffix), bytes));
                if !per_sheet {
                    break;
                }
            }
        }
        Loaded::Doc(doc) => {
            let bytes = match target {
                Target::File(f @ (Format::Markdown | Format::Text)) if to_enc.is_some() => {
                    let out = dest("");
                    check_not_input(path, &out)?;
                    let lost = apex_io::save_text(apex_io::TextSource::Doc(doc), &out, f, to_enc.unwrap_or(apex_io::Encoding::Utf8)).map_err(|e| Fail::Error(format!("{}: {e}", out.display())))?;
                    if lost > 0 {
                        say(&format!("apexdesk: {}: {lost} characters not in the encoding, written as '?'", out.display()));
                    }
                    report(path, &out, a);
                    return Ok(());
                }
                Target::File(f @ (Format::Docx | Format::Odt | Format::Fodt | Format::Markdown | Format::Rtf | Format::Text)) => {

                    let out = dest("");
                    check_not_input(path, &out)?;
                    apex_io::save_doc_with(doc, &out, f, a.value("password").filter(|_| apex_io::can_encrypt(f))).map_err(|e| Fail::Error(format!("{}: {e}", out.display())))?;
                    report(path, &out, a);
                    return Ok(());
                }
                Target::Pdf => crate::headless::pdf(Book::Doc(doc), &name),
                Target::Html => apex_io::html::doc_html(doc).into_bytes(),
                Target::Epub => apex_io::html::doc_epub(doc),
                _ => return error(format!("{}: a document can't be converted to {}", path.display(), target.ext())),
            };
            written.push((dest(""), bytes));
        }
        Loaded::Slides(deck) => {
            let bytes = match target {
                Target::File(f @ (Format::Pptx | Format::Odp)) => {
                    let out = dest("");
                    check_not_input(path, &out)?;
                    apex_io::save_slides_with(deck, &out, f, a.value("password").filter(|_| apex_io::can_encrypt(f))).map_err(|e| Fail::Error(format!("{}: {e}", out.display())))?;
                    report(path, &out, a);
                    return Ok(());
                }
                Target::Pdf => {
                    let what = match a.value("handouts") {
                        Some(n) => match n.parse::<u8>() {
                            Ok(n) if crate::slides_view::handouts::HANDOUTS.contains(&n) => crate::print::Scope::Handouts(n),
                            _ => return usage("--handouts takes 1, 2, 3, 4, 6 or 9"),
                        },
                        None if a.flag("notes") => crate::print::Scope::Notes,
                        None => crate::print::Scope::ActiveSheet,
                    };
                    crate::headless::slides_pdf(deck, &name, what)
                }
                Target::Png => {
                    let width = match a.value("width").map(str::parse::<u32>) {
                        None => 1920,
                        Some(Ok(w)) if (16..=10000).contains(&w) => w,
                        _ => return usage("--width takes a number of pixels"),
                    };
                    for (i, png) in crate::headless::slides_png(deck, width).into_iter().enumerate() {
                        written.push((dir.join(format!("{name}-{}.png", i + 1)), png));
                    }
                    Vec::new()
                }
                Target::Html => crate::slides_view::outline_html(deck, &name).into_bytes(),
                Target::File(Format::Text) => crate::slides_view::outline_text(deck, false).into_bytes(),
                Target::File(Format::Markdown) => crate::slides_view::outline_text(deck, true).into_bytes(),
                _ => return error(format!("{}: a presentation can't be converted to {}", path.display(), target.ext())),
            };
            if target != Target::Png {
                written.push((dest(""), bytes));
            }
        }
    }
    for (out, bytes) in written {
        check_not_input(path, &out)?;
        apex_io::write_atomic(&out, &bytes).map_err(|e| Fail::Error(format!("{}: {e}", out.display())))?;
        report(path, &out, a);
    }
    Ok(())
}

fn check_not_input(input: &Path, out: &Path) -> Res<()> {
    if same_file(input, out) {
        return error(format!("{}: the output would replace the input; use --out DIR", input.display()));
    }
    Ok(())
}

fn report(from: &Path, to: &Path, a: &Parsed) {
    if !a.flag("quiet") {
        emit(&format!("{} -> {}\n", from.display(), to.display()));
    }
}

/// `s` without characters file names can't have.
fn safe_name(s: &str) -> String {
    let t: String = s.chars().map(|c| if "/\\:*?\"<>|".contains(c) || c.is_control() { '_' } else { c }).collect();
    // Nor Windows' device names (CON, NUL, COM1...).
    apex_io::safe_path::safe_file_name(t.trim().trim_matches('.'), "")
}

// --- for the window ------------------------------------------------------------------

/// Formats `convert` writes: for spreadsheets and for documents.
pub(crate) const SHEET_TARGETS: &[&str] = &["xlsx", "ods", "fods", "csv", "tsv", "pdf", "html", "md", "txt"];
pub(crate) const DOC_TARGETS: &[&str] = &["docx", "odt", "fodt", "rtf", "md", "txt", "pdf", "html", "epub"];
pub(crate) const SLIDES_TARGETS: &[&str] = &["pptx", "odp", "pdf", "png", "html", "md", "txt"];

/// Converts a file as `apexdesk convert FILE --to TO` does (File >
/// Convert Files), into `out` or next to it; `all_sheets`: one file per
/// sheet for the text formats.
pub(crate) fn convert_file(path: &Path, to: &str, out: Option<&Path>, all_sheets: bool) -> Result<(), String> {
    let mut argv: Vec<String> = vec!["--to".into(), to.into(), "--quiet".into()];
    if let Some(d) = out {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        argv.extend(["--out".into(), d.display().to_string()]);
    }
    if all_sheets {
        argv.extend(["--sheet".into(), "all".into()]);
    }
    let spec = Spec { values: &["to", "out", "output", "sheet", "password"], flags: &["quiet"] };
    let a = args::parse(&argv, &spec)?;
    let target = Target::parse(to).ok_or_else(|| format!("unknown format '{to}'"))?;
    match convert_one(path, target, &a) {
        Ok(()) => Ok(()),
        Err(Fail::Error(e) | Fail::Usage(e)) => Err(e),
    }
}

/// A sheet as text: "csv", "tsv", "md" (a Markdown table) or "txt" (a
/// table in columns), as `convert` writes them.
pub(crate) fn sheet_text(wb: &Workbook, sheet: usize, to: &str) -> String {
    match to {
        "csv" => apex_io::csv::write(wb, sheet, None),
        "tsv" => apex_io::csv::write(wb, sheet, Some('\t')),
        "md" => used(wb, sheet).map(|r| show::markdown(&show::grid(wb, sheet, r, Shown::Display))).unwrap_or_default(),
        _ => used(wb, sheet).map(|r| show::table(wb, sheet, r, &show::grid(wb, sheet, r, Shown::Display))).unwrap_or_default(),
    }
}

/// A file name without characters file systems refuse.
pub(crate) fn file_safe(s: &str) -> String {
    safe_name(s)
}

/// A file name from a pattern (see `merge --name`).
pub(crate) fn merge_name(pat: &str, n: usize, digits: usize, fields: &[String], rec: &[String]) -> String {
    safe_name(&fill_pattern(pat, n, digits, fields, rec))
}

// --- cat, get, eval ------------------------------------------------------------------

fn one_file(a: &Parsed) -> Res<PathBuf> {
    match a.pos.as_slice() {
        [f] => Ok(PathBuf::from(f)),
        [] => usage("no file given"),
        _ => usage(format!("unexpected argument '{}'", a.pos[1])),
    }
}

fn shown(a: &Parsed) -> Shown {
    if a.flag("formulas") {
        Shown::Formula
    } else if a.flag("raw") {
        Shown::Raw
    } else {
        Shown::Display
    }
}

/// Cells in one of the formats of `cat` and `get`.
fn cells_as(wb: &Workbook, sheet: usize, r: Range, format: &str, shown: Shown) -> Res<String> {
    Ok(match format {
        "json" => show::json(wb, sheet, r),
        f => {
            let rows = show::grid(wb, sheet, r, shown);
            match f {
                "table" => show::table(wb, sheet, r, &rows),
                "csv" => show::delimited(&rows, ','),
                "tsv" => show::delimited(&rows, '\t'),
                "md" | "markdown" => show::markdown(&rows),
                _ => return usage(format!("unknown format '{format}' (table, csv, tsv, json, md)")),
            }
        }
    })
}

fn cat(a: &Parsed) -> Res<i32> {
    let path = one_file(a)?;
    match open(&path, a)?.0 {
        Loaded::Sheet(wb) => {
            let sheet = pick_sheet(&wb, a.value("sheet"))?;
            let r = match a.value("range") {
                Some(t) => {
                    let (s, r) = target(&wb, sheet, t)?;
                    if s != sheet {
                        return print_cells(&wb, s, r, a);
                    }
                    Some(r)
                }
                None => used(&wb, sheet),
            };
            match r {
                Some(r) => print_cells(&wb, sheet, r, a),
                None => Ok(0),
            }
        }
        Loaded::Doc(doc) => {
            let width = match a.value("width") {
                Some(w) => w.parse::<u16>().map_err(|_| Fail::Usage(format!("bad width '{w}'")))?,
                None => 80,
            };
            emit(&doc_as(&doc, a.value("format").unwrap_or("text"), width)?);
            Ok(0)
        }
        Loaded::Slides(deck) => {
            let md = matches!(a.value("format"), Some("md" | "markdown"));
            emit(&crate::slides_view::outline_text(&deck, md));
            Ok(0)
        }
    }
}

fn print_cells(wb: &Workbook, sheet: usize, r: Range, a: &Parsed) -> Res<i32> {
    emit(&cells_as(wb, sheet, r, a.value("format").unwrap_or("table"), shown(a))?);
    Ok(0)
}

fn doc_as(doc: &Document, format: &str, width: u16) -> Res<String> {
    Ok(match format {
        "text" | "txt" => apex_tui::doc_text(doc, width),
        "plain" => {
            let mut t = doc.shared().to_plain_text();
            if !t.ends_with('\n') {
                t.push('\n');
            }
            t
        }
        "md" | "markdown" => apex_io::markdown::write(&doc.shared()),
        "html" => apex_io::html::doc_html(doc),
        _ => return usage(format!("unknown format '{format}' for a document (text, md, plain, html)")),
    })
}

fn get(a: &Parsed) -> Res<i32> {
    let [file, cells @ ..] = a.pos.as_slice() else { return usage("no file given") };
    if cells.is_empty() {
        return usage("no cell given");
    }
    let (wb, _) = open_sheet(Path::new(file), a)?;
    let sheet = pick_sheet(&wb, a.value("sheet"))?;
    for t in cells {
        let (s, r) = target(&wb, sheet, t)?;
        if r.is_single() && a.value("format").is_none() {
            let rows = show::grid(&wb, s, r, shown(a));
            emit(&format!("{}\n", rows[0][0]));
        } else {
            emit(&cells_as(&wb, s, r, a.value("format").unwrap_or("tsv"), shown(a))?);
        }
    }
    Ok(0)
}

fn eval(a: &Parsed) -> Res<i32> {
    let (mut wb, formula) = match a.pos.as_slice() {
        [f] => (Workbook::new(), f.clone()),
        [file, f] => (open_sheet(Path::new(file), a)?.0, f.clone()),
        [] => return usage("no formula given"),
        _ => return usage(format!("unexpected argument '{}'", a.pos[2])),
    };
    let sheet = pick_sheet(&wb, a.value("sheet"))?;
    wb.locale = Locale::EN;
    let formula = if formula.starts_with('=') { formula } else { format!("={formula}") };
    wb.check_formula(&formula).map_err(|e| Fail::Error(format!("{formula}: {e}")))?;
    let v = wb.eval_formula(sheet, &formula);
    let text = match &v {
        apex_sheet::Value::Array(m) => {
            let rows: Vec<Vec<String>> = (0..m.rows).map(|r| (0..m.cols).map(|c| show::raw(&m.data[r * m.cols + c])).collect()).collect();
            show::delimited(&rows, '\t')
        }
        v => format!("{}\n", show::raw(v)),
    };
    emit(&text);
    Ok(0)
}

// --- set, recalc ---------------------------------------------------------------------

fn set(a: &Parsed) -> Res<i32> {
    let [file, pairs @ ..] = a.pos.as_slice() else { return usage("no file given") };
    if pairs.is_empty() || pairs.len() % 2 != 0 {
        return usage("give pairs of CELL VALUE");
    }
    let path = PathBuf::from(file);
    let (mut wb, fmt) = open_sheet(&path, a)?;
    let (out, out_fmt) = output_of(&path, fmt, a)?;
    let sheet = pick_sheet(&wb, a.value("sheet"))?;
    // Typed values and formulas in English syntax, whatever the system's.
    let locale = std::mem::replace(&mut wb.locale, Locale::EN);
    wb.enforce_protection = true;
    for pair in pairs.chunks(2) {
        let (s, r) = target(&wb, sheet, &pair[0])?;
        let text = &pair[1];
        wb.check_formula(text).map_err(|e| Fail::Error(format!("{text}: {e}")))?;
        // A range gets the value in each cell.
        let inputs: Vec<(CellRef, String)> = (r.start.row..=r.end.row).flat_map(|row| (r.start.col..=r.end.col).map(move |col| (CellRef::new(row, col), text.clone()))).collect();
        wb.set_inputs(s, inputs);
        if wb.take_blocked() {
            return error(format!("{}: {} is protected", path.display(), pair[0]));
        }
    }
    wb.locale = locale;
    save_sheet(&wb, &out, out_fmt, a)?;
    Ok(0)
}

fn recalc(a: &Parsed) -> Res<i32> {
    let path = one_file(a)?;
    let (mut wb, fmt) = open_sheet(&path, a)?;
    let (out, out_fmt) = output_of(&path, fmt, a)?;
    wb.recalc_all();
    save_sheet(&wb, &out, out_fmt, a)?;
    Ok(0)
}

// --- run-macro -----------------------------------------------------------------------

fn run_macro(a: &Parsed) -> Res<i32> {
    run_macro_with(a, Box::new(emit), Box::new(say))?;
    Ok(0)
}

/// `run-macro`, printing with `out` and `err`: where it saved, if it did.
fn run_macro_with<'a>(a: &Parsed, out: Box<dyn FnMut(&str) + Send + 'a>, err: Box<dyn FnMut(&str) + Send + 'a>) -> Res<Option<PathBuf>> {
    let [file, name, args @ ..] = a.pos.as_slice() else { return usage("give a FILE and a MACRO") };
    if !a.flag("trust") {
        return usage("macros run only with --trust (you trust this file's code)");
    }
    let path = PathBuf::from(file);
    let (mut wb, fmt) = open_sheet(&path, a)?;
    if let Some(why) = crate::app::macro_headless::refused(&path, &wb, a.flag("allow-suspicious")) {
        return error(format!("{file}: {why}"));
    }
    let h = crate::app::macro_headless::Headless { allow_writes: a.flag("allow-writes"), password: a.value("password").map(str::to_string), out, err };
    let ran = crate::app::macro_headless::run(&path, &mut wb, name, args, h).map_err(|e| Fail::Error(format!("{file}: {e}")))?;
    if let Some(e) = &ran.error {
        return error(format!("{file}: {name}: {} (line {} of {})", e, e.line, e.module));
    }
    if a.value("output").is_some() || a.flag("save") || ran.save {
        let (target, target_fmt) = output_of(&path, fmt, a)?;
        save_sheet(&wb, &target, target_fmt, a)?;
        return Ok(Some(target));
    }
    Ok(None)
}

/// The modules of a workbook's VBA project: name, kind and code.
fn macro_modules(wb: &Workbook) -> Res<Vec<(String, &'static str, String)>> {
    let Some(bin) = &wb.macros.project else { return Ok(Vec::new()) };
    let p = apex_io::vba::read(bin).map_err(|e| Fail::Error(e.to_string()))?;
    if p.protected {
        return error("the macro project is locked with a password");
    }
    Ok(p.modules.into_iter().map(|m| {
        let kind = match m.kind {
            apex_io::vba::ModuleKind::Standard => "module",
            apex_io::vba::ModuleKind::Class => "class",
            apex_io::vba::ModuleKind::Document => "document",
            apex_io::vba::ModuleKind::Form => "form",
        };
        (m.name, kind, m.code)
    }).collect())
}

/// Writes `code` as module `name` of a workbook's project (a new one
/// when there is none by that name): true when the module is new. A
/// signed project loses its signature.
fn write_module(wb: &mut Workbook, name: &str, code: &str, class: bool) -> Res<bool> {
    use apex_io::vba::{ModuleKind, VbaModule};
    let bin = match &wb.macros.project {
        Some(b) => b.to_vec(),
        None => apex_io::vba::new_workbook_project(wb.code_name.as_deref().filter(|b| apex_io::vba::is_identifier(b)).unwrap_or("ThisWorkbook"), &[]).map_err(|e| Fail::Error(e.to_string()))?,
    };
    let mut p = apex_io::vba::read(&bin).map_err(|e| Fail::Error(e.to_string()))?;
    if p.protected {
        return error("the macro project is locked with a password");
    }
    let code = code.replace("\r\n", "\n").replace('\r', "\n");
    let created = match p.modules.iter_mut().find(|m| m.name.eq_ignore_ascii_case(name)) {
        Some(m) if m.kind == ModuleKind::Form => return error("forms can't be written"),
        Some(m) => {
            m.code = code;
            false
        }
        None => {
            p.check_name(name, None).map_err(|_| Fail::Usage(format!("'{name}' can't be the name of a module")))?;
            let mut m = if class { VbaModule::class(name.trim()) } else { VbaModule::standard(name.trim()) };
            m.code = code;
            p.modules.push(m);
            true
        }
    };
    let mut book = wb.code_name.clone();
    let mut sheets: Vec<Option<String>> = wb.sheets.iter().map(|s| s.code_name.clone()).collect();
    p.match_book(&mut book, &mut sheets);
    wb.code_name = book;
    for (s, n) in wb.sheets.iter_mut().zip(sheets) {
        s.code_name = n;
    }
    wb.macros.set_project(p.write().map_err(|e| Fail::Error(e.to_string()))?);
    Ok(created)
}

// --- merge ---------------------------------------------------------------------------

fn merge(a: &Parsed) -> Res<i32> {
    let [template, data] = a.pos.as_slice() else { return usage("give a TEMPLATE and a DATA file") };
    let (tpath, dpath) = (Path::new(template), Path::new(data));
    let (Loaded::Doc(doc), tfmt) = open(tpath, a)? else { return error(format!("{template}: the template must be a document")) };
    let password = match a.value("password-env") {
        Some(var) => std::env::var(var).map_err(|_| Fail::Usage(format!("environment variable {var} is not set")))?,
        None => String::new(),
    };
    let (fields, records) = match (a.value("query"), a.value("sql")) {
        (Some(_), Some(_)) => return usage("give --query or --sql, not both"),
        (Some(q), None) => crate::merge_data::from_query(dpath, q, &password, &|_| a.flag("allow-remote")).map_err(|e| Fail::Error(format!("{data}: {e}")))?,
        (None, Some(sql)) => {
            let src = crate::merge_data::parse_database(data, sql).map_err(Fail::Usage)?;
            crate::merge_data::from_source(&src, &password).map_err(|e| Fail::Error(format!("{data}: {e}")))?
        }
        (None, None) => {
            let (wb, _) = open_sheet(dpath, a)?;
            let sheet = pick_sheet(&wb, a.value("sheet"))?;
            apex_io::records::records(&wb, sheet, |n| format!("Column {n}")).ok_or_else(|| Fail::Error(format!("{data}: no data")))?
        }
    };

    for used in apex_doc::merge::fields_used(&doc) {
        if !fields.contains(&used) {
            say(&format!("apexdesk: warning: the field \u{ab}{used}\u{bb} is not in {data}"));
        }
    }
    let dir = PathBuf::from(a.value("out").unwrap_or("."));
    std::fs::create_dir_all(&dir).map_err(|e| Fail::Error(format!("{}: {e}", dir.display())))?;
    let fmt = if tfmt.can_save() { tfmt } else { Format::Docx };
    let ext = if a.flag("pdf") { "pdf" } else { fmt.extension() };
    let base = stem(tpath);
    let write = |doc: &Document, out: &Path| -> Res<()> {
        let res = if a.flag("pdf") {
            apex_io::write_atomic(out, &crate::headless::pdf(Book::Doc(doc), &stem(out))).map_err(|e| e.to_string())
        } else {
            apex_io::save_doc(doc, out, fmt).map_err(|e| e.to_string())
        };
        res.map_err(|e| Fail::Error(format!("{}: {e}", out.display())))?;
        if !a.flag("quiet") {
            emit(&format!("{}\n", out.display()));
        }
        Ok(())
    };
    if a.flag("single") {
        let out = dir.join(format!("{base}-merged.{ext}"));
        check_not_input(tpath, &out)?;
        write(&apex_doc::merge::merge(&doc, &fields, &records), &out)?;
        return Ok(0);
    }
    let pattern = a.value("name").map(str::to_string).unwrap_or_else(|| format!("{}-{{n}}", base.replace('{', "{{")));
    let digits = records.len().to_string().len();
    let mut taken = std::collections::HashSet::new();
    for (k, rec) in records.iter().enumerate() {
        let mut name = merge_name(&pattern, k + 1, digits, &fields, rec);
        if name.is_empty() {
            name = format!("{base}-{:0digits$}", k + 1);
        }
        // Two records with the same name don't overwrite each other.
        let mut unique = name.clone();
        let mut n = 2;
        while !taken.insert(unique.to_lowercase()) {
            unique = format!("{name}-{n}");
            n += 1;
        }
        let out = dir.join(format!("{unique}.{ext}"));
        check_not_input(tpath, &out)?;
        write(&apex_doc::merge::fill(&doc, &fields, rec), &out)?;
    }
    Ok(0)
}

/// A file name from `--name`: `{n}` is the record number, `{Field}` its
/// value, `{{` a brace.
pub(crate) fn fill_pattern(pat: &str, n: usize, digits: usize, fields: &[String], rec: &[String]) -> String {
    let mut out = String::new();
    let mut rest = pat;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        if let Some(stripped) = after.strip_prefix('{') {
            out.push('{');
            rest = stripped;
            continue;
        }
        let Some(j) = after.find('}') else {
            out.push_str(&rest[i..]);
            rest = "";
            break;
        };
        let key = &after[..j];
        if key == "n" {
            out.push_str(&format!("{n:0digits$}"));
        } else if let Some(f) = fields.iter().position(|f| f.eq_ignore_ascii_case(key)) {
            out.push_str(rec.get(f).map_or("", String::as_str));
        } else {
            out.push_str(&after[..=j]);
        }
        rest = &after[j + 1..];
    }
    out.push_str(rest);
    safe_name(&out)
}

// --- info ----------------------------------------------------------------------------

/// One line of `info`: a name and a value (typed for JSON).
enum Fact {
    Text(String),
    Num(u64),
    Bool(bool),
    List(Vec<Vec<(&'static str, Fact)>>),
}

fn info(a: &Parsed) -> Res<i32> {
    if a.pos.is_empty() {
        return usage("no file given");
    }
    let json = a.flag("json");
    let mut all = Vec::new();
    let mut failed = false;
    for p in &a.pos {
        for f in glob::expand(p).map_err(Fail::Error)? {
            match facts(&f, a) {
                Ok(facts) => all.push(facts),
                Err(Fail::Error(e) | Fail::Usage(e)) => {
                    say(&format!("apexdesk: {e}"));
                    failed = true;
                }
            }
        }
    }
    if json {
        let depth = usize::from(all.len() > 1);
        let items: Vec<String> = all.iter().map(|f| json_object(f, depth)).collect();
        emit(&if items.len() == 1 { format!("{}\n", items[0]) } else { format!("[\n  {}\n]\n", items.join(",\n  ")) });
    } else {
        let blocks: Vec<String> = all.iter().map(|f| text_facts(f, 0)).collect();
        emit(&blocks.join("\n"));
    }
    Ok(i32::from(failed))
}

fn facts(path: &Path, a: &Parsed) -> Res<Vec<(&'static str, Fact)>> {
    let data = std::fs::read(path).map_err(|e| Fail::Error(format!("{}: {e}", path.display())))?;
    let mut out: Vec<(&'static str, Fact)> = vec![("file", Fact::Text(path.display().to_string())), ("size", Fact::Num(data.len() as u64))];
    let encrypted = apex_io::is_encrypted(&data);
    let loaded = match apex_io::open_with(path, a.value("password")) {
        Ok(l) => Some(l),
        Err(e) if e.needs_password() => None,
        Err(e) => return error(format!("{}: {e}", path.display())),
    };
    out.push(("encrypted", Fact::Bool(encrypted)));
    let Some((loaded, fmt)) = loaded else {
        out.push(("note", Fact::Text("encrypted: give --password to see more".into())));
        return Ok(out);
    };
    out.insert(1, ("format", Fact::Text(fmt.extension().to_string())));
    out.push(("macros", Fact::Bool(apex_io::probe::has_macros(&data))));
    let meta = match &loaded {
        Loaded::Sheet(wb) => {
            out.insert(2, ("type", Fact::Text("spreadsheet".into())));
            let sheets = wb
                .sheets
                .iter()
                .enumerate()
                .map(|(i, sh)| {
                    let mut f = vec![("name", Fact::Text(sh.name.clone())), ("cells", Fact::Num(sh.cell_count() as u64))];
                    f.push(("used", Fact::Text(wb.sheets[i].extent().map(|e| Range::new(CellRef::new(0, 0), e).to_a1()).unwrap_or_default())));
                    if sh.hidden {
                        f.push(("hidden", Fact::Bool(true)));
                    }
                    f
                })
                .collect();
            out.push(("sheets", Fact::List(sheets)));
            out.push(("pages", Fact::Num(crate::headless::pages(Book::Sheet(wb)) as u64)));
            wb.meta.clone()
        }
        Loaded::Doc(doc) => {
            out.insert(2, ("type", Fact::Text("document".into())));
            let text = doc.to_plain_text();
            out.push(("paragraphs", Fact::Num(doc.paragraphs.len() as u64)));
            out.push(("words", Fact::Num(doc.word_count() as u64)));
            out.push(("characters", Fact::Num(text.chars().filter(|c| *c != '\n').count() as u64)));
            let tables: std::collections::HashSet<u32> = doc.paragraphs.iter().filter_map(|p| p.cell.as_ref().map(|c| c.table)).collect();
            out.push(("tables", Fact::Num(tables.len() as u64)));
            out.push(("images", Fact::Num(doc.images().len() as u64)));
            out.push(("comments", Fact::Num(doc.comments.len() as u64)));
            if a.flag("page-starts") {
                let starts = crate::headless::page_starts(doc);
                out.push(("pages", Fact::Num(starts.len() as u64)));
                let list = starts.iter().map(|&(para, continued)| vec![("paragraph", Fact::Num(para as u64)), ("continued", Fact::Bool(continued))]).collect();
                out.push(("page_starts", Fact::List(list)));
            } else {
                out.push(("pages", Fact::Num(crate::headless::pages(Book::Doc(doc)) as u64)));
            }
            doc.meta.clone()
        }
        Loaded::Slides(deck) => {
            out.insert(2, ("type", Fact::Text("presentation".into())));
            out.push(("slides", Fact::Num(deck.slides.len() as u64)));
            out.push(("hidden", Fact::Num(deck.slides.iter().filter(|s| s.hidden).count() as u64)));
            out.push(("masters", Fact::Num(deck.masters.len() as u64)));
            out.push(("layouts", Fact::Num(deck.masters.iter().map(|m| m.layouts.len()).sum::<usize>() as u64)));
            out.push(("sections", Fact::Num(deck.sections.len() as u64)));
            out.push(("width", Fact::Text(format!("{:.0}pt", deck.width))));
            out.push(("height", Fact::Text(format!("{:.0}pt", deck.height))));
            out.push(("pictures", Fact::Num(crate::slides_view::pictures(deck).len() as u64)));
            deck.meta.clone()
        }
    };
    for (k, v) in [("title", &meta.title), ("author", &meta.author), ("subject", &meta.subject), ("keywords", &meta.keywords), ("company", &meta.company), ("created", &meta.created)] {
        if !v.trim().is_empty() {
            out.push((k, Fact::Text(v.clone())));
        }
    }
    Ok(out)
}

fn text_facts(facts: &[(&'static str, Fact)], indent: usize) -> String {
    let pad = "  ".repeat(indent);
    let mut out = String::new();
    for (k, v) in facts {
        match v {
            Fact::Text(t) => out.push_str(&format!("{pad}{k}: {}\n", one_line(t))),
            Fact::Num(n) => out.push_str(&format!("{pad}{k}: {n}\n")),
            Fact::Bool(b) => out.push_str(&format!("{pad}{k}: {}\n", if *b { "yes" } else { "no" })),
            Fact::List(items) => {
                out.push_str(&format!("{pad}{k}: {}\n", items.len()));
                for item in items {
                    // Each item on one line: "name (cells: 3, used: A1:B2)".
                    let parts: Vec<String> = item.iter().skip(1).map(|(k, v)| format!("{k}: {}", fact_text(v))).collect();
                    out.push_str(&format!("{pad}  - {} ({})\n", fact_text(&item[0].1), parts.join(", ")));
                }
            }
        }
    }
    out
}

/// A document's words on one line: a line break in them could add a line
/// that seems the tool's own.
pub(crate) fn one_line(t: &str) -> String {
    t.chars().map(|c| if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') { ' ' } else { c }).collect()
}

fn fact_text(f: &Fact) -> String {
    match f {
        Fact::Text(t) => one_line(t),
        Fact::Num(n) => n.to_string(),
        Fact::Bool(b) => if *b { "yes" } else { "no" }.to_string(),
        Fact::List(l) => l.len().to_string(),
    }
}

fn json_object(facts: &[(&'static str, Fact)], depth: usize) -> String {
    let pad = "  ".repeat(depth + 1);
    let items: Vec<String> = facts
        .iter()
        .map(|(k, v)| {
            let v = match v {
                Fact::Text(t) => show::json_str(t),
                Fact::Num(n) => n.to_string(),
                Fact::Bool(b) => b.to_string(),
                Fact::List(l) => format!("[{}]", l.iter().map(|o| json_object(o, depth + 1)).collect::<Vec<_>>().join(", ")),
            };
            format!("{pad}\"{k}\": {v}")
        })
        .collect();
    format!("{{\n{}\n{}}}", items.join(",\n"), "  ".repeat(depth))
}

// --- tui -----------------------------------------------------------------------------

fn tui(a: &Parsed) -> Res<i32> {
    if a.pos.len() > 1 {
        return usage(format!("unexpected argument '{}'", a.pos[1]));
    }
    let settings = crate::settings::Settings::load();
    if settings.lang.is_empty() {
        apex_i18n::set_from_env();
    } else {
        apex_i18n::set(&settings.lang);
    }
    let path = a.pos.first().map(PathBuf::from);
    // The menu bar's setting is saved over the settings as they are then
    // (the window may have changed them meanwhile).
    fn save_menu_bar(on: bool) {
        let mut s = crate::settings::Settings::load();
        s.tui_menu_bar = on;
        s.save();
    }
    let hooks = apex_tui::Hooks { pdf: Some(crate::headless::write_pdf), hide_menu_bar: !settings.tui_menu_bar, save_menu_bar: Some(save_menu_bar) };
    apex_tui::run(path.as_deref(), hooks).map_err(Fail::Error)?;
    Ok(0)
}

