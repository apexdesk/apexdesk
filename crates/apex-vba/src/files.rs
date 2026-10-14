//! The files a macro reads and writes: `Open`, `Print #`, `Line Input #`,
//! `Close`, Dir, Kill, MkDir, FileCopy, `Name`...
//!
//! A macro reaches the files in its workbook's folder and below (only those
//! right in it when the folder is the home folder or a top one like `/tmp`)
//! and the ones the user picks in a file dialog; a workbook in a trusted
//! location reaches any. Changing files (writing, deleting, renaming, making
//! folders) asks the user once per run. Hidden files and folders (`.ssh`,
//! `.bashrc`) are never reached; programs, scripts, shortcuts and
//! documents that can hold macros are never written, nor files that can be
//! run; a function called from a cell reaches no file.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use apex_sheet::format::serial_to_date;

use crate::ast::{Expr, FileMode, FileStmt, PrintItem};
use crate::error::{Error, R};
use crate::interp::{Book, Interp};
use crate::value::{self, Array, VType, Value, number_text};

mod fso;

/// What Workbook.SaveAs and SaveCopyAs write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BookFormat {
    /// Excel's (.xlsx, .xlsm with its macros, templates).
    Xlsx,
    Ods,
    /// Flat OpenDocument.
    Fods,
    /// The first sheet as comma-separated text.
    Csv,
    /// The first sheet as tab-separated text.
    Tsv,
}

/// What Document.SaveAs writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocFormat {
    /// Word's (.docx, .docm with its macros, templates).
    Docx,
    Odt,
    /// Flat OpenDocument text.
    Fodt,
    Rtf,
    /// Plain text.
    Text,
    Markdown,
}

/// What a run's macros may do with files beyond the workbook's folder.
#[derive(Clone, Debug, Default)]
pub struct FileAccess {
    /// The workbook is in a trusted location: files anywhere (but hidden ones).
    pub trusted: bool,
    /// Whether macros may change files without asking (None: ask once).
    pub write: Option<bool>,
}

/// What a macro asks the user to pick (GetOpenFilename, FileDialog...).
#[derive(Clone, Debug, PartialEq)]
pub struct FilePick {
    pub kind: PickKind,
    /// More than one file.
    pub multi: bool,
    pub title: String,
    /// (description, patterns like `*.txt`).
    pub filters: Vec<(String, Vec<String>)>,
    /// The file name offered first.
    pub initial: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickKind {
    Open,
    Save,
    Folder,
}

const MAX_OPEN: usize = 64;
/// Bytes a run may write.
const MAX_WRITTEN: u64 = 1 << 30;
/// A file opened For Input is read whole: bigger ones are refused.
const MAX_READ: u64 = 256 << 20;
const MAX_LISTED: usize = 100_000;

/// Never written: what runs (programs, scripts, shortcuts, launchers).
const RUNNABLE: &[&str] = &[
    "app", "appimage", "appinstaller", "application", "appref-ms", "appx", "bash", "bat", "chm", "cjs", "cmd", "com", "command", "cpl", "csh", "desktop", "dll", "dylib", "exe", "fish", "gadget", "hta", "inf", "jar", "jnlp", "js", "jse", "ksh", "library-ms", "lnk", "mjs", "msc", "msi", "msix", "msp", "pif", "pl", "ps1", "ps1xml", "psd1", "psm1", "py", "pyw", "rb", "reg", "run", "scf", "scr", "service", "settingcontent-ms", "sh", "so", "url", "vb", "vbe", "vbs", "website", "wll", "ws", "wsc", "wsf", "wsh", "xbap", "xll", "zsh",
];
/// Never written raw: documents that can hold macros.
const MACRO_DOCS: &[&str] = &[
    "accdb", "accde", "doc", "docm", "dot", "dotm", "fodp", "fods", "fodt", "iqy", "mdb", "mde", "odb", "odp", "ods", "odt", "otp", "ots", "ott", "pot", "potm", "ppa", "ppam", "pps", "ppsm", "ppt", "pptm", "slk", "xla", "xlam", "xls", "xlsb", "xlsm", "xlt", "xltm",
];

/// Windows-1252's characters 0x80 to 0x9F (others as Latin-1).
const CP1252: [char; 32] = [
    '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž', '\u{8F}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9D}', 'ž', 'Ÿ',
];

fn err(n: i32) -> Error {
    Error::new(
        n,
        match n {
            52 => "Bad file name or number",
            53 => "File not found",
            54 => "Bad file mode",
            55 => "File already open",
            58 => "File already exists",
            59 => "Bad record length",
            61 => "Disk full",
            62 => "Input past end of file",
            63 => "Bad record number",
            67 => "Too many files",
            75 => "Path/File access error",
            76 => "Path not found",
            _ => "Device I/O error",
        },
    )
}

fn denied(why: &str) -> Error {
    Error::new(70, format!("Permission denied: {why}"))
}

fn io(e: std::io::Error) -> Error {
    match e.kind() {
        std::io::ErrorKind::NotFound => err(53),
        std::io::ErrorKind::PermissionDenied => Error::new(70, "Permission denied"),
        std::io::ErrorKind::AlreadyExists => err(58),
        _ => err(75),
    }
}

/// Text read from a file: UTF-8, else Windows-1252 (UTF-16 with its mark).
fn decode(bytes: Vec<u8>) -> String {
    let utf16 = |b: &[u8], little: bool| -> String {
        let units = b.chunks_exact(2).map(|c| if little { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) });
        char::decode_utf16(units).map(|c| c.unwrap_or('\u{FFFD}')).collect()
    };
    if let Some(b) = bytes.strip_prefix(b"\xFF\xFE") {
        return utf16(b, true);
    }
    if let Some(b) = bytes.strip_prefix(b"\xFE\xFF") {
        return utf16(b, false);
    }
    let bytes = match bytes.strip_prefix(b"\xEF\xBB\xBF") {
        Some(b) => b.to_vec(),
        None => bytes,
    };
    match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => e.into_bytes().iter().map(|&b| byte_char(b)).collect(),
    }
}

fn byte_char(b: u8) -> char {
    if (0x80..0xA0).contains(&b) { CP1252[usize::from(b - 0x80)] } else { char::from(b) }
}

/// A character as Windows-1252 writes it (`?` when it can't).
fn char_byte(c: char) -> u8 {
    match u32::from(c) {
        x @ (0..0x80 | 0xA0..0x100) => x as u8,
        _ => CP1252.iter().position(|&x| x == c).map_or(b'?', |i| 0x80 + i as u8),
    }
}

/// `*` and `?` as Dir and Kill match them, ignoring case.
fn wild(pat: &[char], name: &[char]) -> bool {
    let (mut p, mut n) = (0, 0);
    let mut back: Option<(usize, usize)> = None;
    while n < name.len() {
        if p < pat.len() && (pat[p] == '?' || pat[p] == name[n]) {
            p += 1;
            n += 1;
        } else if p < pat.len() && pat[p] == '*' {
            back = Some((p, n));
            p += 1;
        } else if let Some((bp, bn)) = back {
            p = bp + 1;
            n = bn + 1;
            back = Some((bp, bn + 1));
        } else {
            return false;
        }
    }
    pat[p..].iter().all(|&c| c == '*')
}

fn lower_chars(s: &str) -> Vec<char> {
    s.to_lowercase().chars().collect()
}

/// `folder/pattern` split where the last name starts.
fn split_pattern(text: &str) -> (String, String) {
    let t = text.replace('\\', "/");
    match t.rfind('/') {
        Some(0) => ("/".into(), t[1..].into()),
        Some(i) => (t[..i].into(), t[i + 1..].into()),
        None => (".".into(), t),
    }
}

/// Whether macros only reach the files right in folder `r` (not below):
/// the home folder or a top one.
fn shallow(r: &Path) -> bool {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).and_then(|h| fs::canonicalize(h).ok());
    r.components().filter(|c| matches!(c, Component::Normal(_))).count() <= 2 || home.as_deref() == Some(r)
}

/// One of Application.FileDialog's dialogs, as the macro set it up.
#[derive(Clone, Debug)]
pub(crate) struct Dialog {
    /// msoFileDialogOpen (1), SaveAs (2), FilePicker (3), FolderPicker (4).
    pub kind: u8,
    pub title: String,
    pub multi: bool,
    pub initial: String,
    pub button: String,
    /// (description, extensions like `*.txt; *.csv`).
    pub filters: Vec<(String, String)>,
    pub filter_index: i64,
    pub selected: Vec<String>,
}

impl Dialog {
    fn new(kind: u8) -> Dialog {
        let filters = if kind == 4 { Vec::new() } else { vec![("All Files".into(), "*.*".into())] };
        Dialog { kind, title: String::new(), multi: false, initial: String::new(), button: String::new(), filters, filter_index: 1, selected: Vec::new() }
    }

    pub fn pick_kind(&self) -> PickKind {
        match self.kind {
            2 => PickKind::Save,
            4 => PickKind::Folder,
            _ => PickKind::Open,
        }
    }
}

/// A file open with `Open`.
struct Handle {
    path: PathBuf,
    mode: FileMode,
    body: Body,
    /// The line reading or writing is on (a TextStream's Line).
    line: u64,
}

enum Body {
    /// For Input: the text, read whole, and where reading is (a byte of it).
    Text { text: String, pos: usize, size: u64 },
    /// For Output and Append: the column the next character goes to, and
    /// whether a `Write #` ended with a separator.
    Out { w: BufWriter<File>, col: usize, comma: bool },
    /// Binary and Random: the record length (1 for Binary).
    Bin { f: File, len: u64 },
}

/// The files of a run.
pub(crate) struct Files {
    /// The workbook's folder.
    root: Option<PathBuf>,
    /// Only the files right in `root` (the home folder, `/tmp`...).
    shallow: bool,
    trusted: bool,
    write: Option<bool>,
    /// The workbook's own file.
    book: Option<PathBuf>,
    /// Picked in a dialog: whether to save.
    picked: Vec<(PathBuf, bool)>,
    /// Folders picked in a dialog, like `root`.
    folders: Vec<(PathBuf, bool)>,
    /// Application.FileDialog's dialogs (msoFileDialogOpen to FolderPicker).
    pub(crate) dialogs: [Dialog; 4],
    cwd: Option<PathBuf>,
    open: BTreeMap<i32, Handle>,
    /// The number of the next FileSystemObject text stream (past Open's).
    next_stream: i32,
    /// What `Dir()` returns next.
    listing: Option<std::vec::IntoIter<String>>,
    written: u64,
}

impl Files {
    pub(crate) fn new(file_path: &str, file_name: &str, access: &FileAccess) -> Files {
        let root = Some(file_path).filter(|p| !p.is_empty()).and_then(|p| fs::canonicalize(p).ok()).filter(|p| p.is_dir());
        let shallow = root.as_deref().is_some_and(shallow);
        let book = root.as_ref().filter(|_| !file_name.is_empty()).map(|r| r.join(file_name));
        let dialogs = std::array::from_fn(|k| Dialog::new(k as u8 + 1));
        Files { root, shallow, trusted: access.trusted, write: access.write, book, picked: Vec::new(), folders: Vec::new(), dialogs, cwd: None, open: BTreeMap::new(), next_stream: 1000, listing: None, written: 0 }
    }

    /// The workbook's own file (swapped with another workbook's).
    pub(crate) fn book_mut(&mut self) -> &mut Option<PathBuf> {
        &mut self.book
    }

    /// What another workbook's code running on this one may do.
    pub(crate) fn access(&self) -> FileAccess {
        FileAccess { trusted: self.trusted, write: self.write }
    }

    /// The user picked `path` in a dialog: a file to open or to save in,
    /// or a folder.
    pub(crate) fn picked(&mut self, path: &str, kind: PickKind) {
        let Ok(p) = self.resolve(path) else { return };
        if self.picked.len() + self.folders.len() >= 10_000 {
            return;
        }
        match kind {
            PickKind::Folder => {
                let sh = shallow(&p);
                self.folders.push((p, sh));
            }
            k => self.picked.push((p, k == PickKind::Save)),
        }
    }

    /// The path `text` names, absolute (its links not followed).
    fn absolute(&self, text: &str) -> R<PathBuf> {
        if text.trim().is_empty() || text.contains('\0') {
            return Err(err(52));
        }
        let t = text.replace('\\', "/");
        let p = Path::new(&t);
        let mut out = if p.is_absolute() {
            PathBuf::new()
        } else {
            self.cwd.clone().or_else(|| self.root.clone()).ok_or_else(|| denied("the workbook isn't saved in a folder"))?
        };
        for c in p.components() {
            match c {
                Component::ParentDir => {
                    out.pop();
                }
                Component::CurDir => {}
                c => out.push(c),
            }
        }
        Ok(out)
    }

    /// The path `text` names, absolute, its links followed.
    fn resolve(&self, text: &str) -> R<PathBuf> {
        let out = self.absolute(text)?;
        // The part that exists, its links followed; a broken link is refused
        // (writing through it would make its target).
        let mut rest: Vec<OsString> = Vec::new();
        let mut head = out;
        loop {
            if let Ok(mut real) = fs::canonicalize(&head) {
                real.extend(rest.iter().rev());
                return Ok(real);
            }
            if fs::symlink_metadata(&head).is_ok() {
                return Err(err(75));
            }
            match head.file_name() {
                Some(n) => {
                    rest.push(n.to_owned());
                    head.pop();
                }
                None => return Err(err(76)),
            }
        }
    }

    /// Whether macros may reach `p` (resolved).
    fn reach(&self, p: &Path, write: bool) -> R<()> {
        if self.picked.iter().any(|(f, save)| f == p && (*save || !write)) {
            return Ok(());
        }
        let roots = self.root.iter().map(|r| (r, self.shallow)).chain(self.folders.iter().map(|(r, sh)| (r, *sh)));
        let mut inside = None;
        for (r, shallow) in roots {
            if let Ok(b) = p.strip_prefix(r)
                && (self.trusted || !shallow || b.components().count() <= 1)
            {
                inside = Some(b);
                break;
            }
        }
        let below = match inside {
            Some(b) => b,
            None if self.trusted => p,
            None => return Err(denied("macros only reach the files in the workbook's folder (right in it, for a home or top folder) and the ones picked in a dialog")),
        };
        let hidden = |q: &Path| q.components().any(|c| matches!(c, Component::Normal(x) if x.to_string_lossy().starts_with('.')));
        if hidden(below) || write && hidden(p) {
            return Err(denied("macros don't reach hidden files"));
        }
        Ok(())
    }

    /// Whether macros may write `p` as a file.
    fn writable(&self, p: &Path) -> R<()> {
        self.writable_as(p, false)
    }

    /// Whether macros may write `p`; `book`: the workbook itself saved
    /// there (SaveAs), which may hold its macros.
    fn writable_as(&self, p: &Path, book: bool) -> R<()> {
        if !book && self.book.as_deref() == Some(p) {
            return Err(denied("a macro can't write over its own workbook"));
        }
        let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        // Windows drops trailing dots and spaces (`x.exe.`); `x:y` is a stream.
        let name = name.trim_end_matches(['.', ' ']);
        if name.contains(':') {
            return Err(err(52));
        }
        let ext = name.rsplit_once('.').map_or("", |(_, e)| e);
        // The workbook saved as itself (the formats SaveAs writes).
        let own_kind = book && matches!(ext, "xlsm" | "xltm" | "ods" | "ots" | "fods" | "docm" | "dotm" | "odt" | "ott");
        if RUNNABLE.contains(&ext) || MACRO_DOCS.contains(&ext) && !own_kind {
            return Err(denied(&format!("macros don't write .{ext} files")));
        }
        if let Ok(m) = fs::symlink_metadata(p) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if m.is_file() && m.permissions().mode() & 0o111 != 0 {
                    return Err(denied("macros don't change files that can be run"));
                }
            }
            if !m.is_file() && !m.is_dir() {
                return Err(err(75));
            }
        }
        Ok(())
    }

    fn handle(&mut self, n: i32) -> R<&mut Handle> {
        self.open.get_mut(&n).ok_or_else(|| err(52))
    }

    fn is_open(&self, p: &Path) -> bool {
        self.open.values().any(|h| h.path == p)
    }

    /// Room for `n` more bytes written this run.
    fn count(&mut self, n: u64) -> R<()> {
        if self.written.saturating_add(n) > MAX_WRITTEN {
            return Err(Error::new(61, "Disk full: macros write up to 1 GB per run"));
        }
        self.written += n;
        Ok(())
    }
}

impl Interp<'_> {
    pub(crate) fn no_udf(&self) -> R<()> {
        match self.wb {
            Book::Ref(_) => Err(denied("a function called from a cell can't use files")),
            Book::Mut(_) | Book::Owned(_) => Ok(()),
        }
    }

    /// The path `text` names, if macros may read it.
    pub(crate) fn path_to_read(&mut self, text: &str) -> R<PathBuf> {
        self.no_udf()?;
        let p = self.files.resolve(text)?;
        self.files.reach(&p, false)?;
        Ok(p)
    }

    /// The path `text` names, if macros may change it (asks the first time).
    fn path_to_change(&mut self, text: &str) -> R<PathBuf> {
        self.changing(text, false)
    }

    fn changing(&mut self, text: &str, book: bool) -> R<PathBuf> {
        self.no_udf()?;
        let p = self.files.resolve(text)?;
        self.files.reach(&p, true)?;
        self.files.writable_as(&p, book)?;
        if self.files.picked.iter().any(|(f, save)| *f == p && *save) {
            return Ok(p);
        }
        let allowed = match self.files.write {
            Some(a) => a,
            None => {
                let folder = self.files.root.as_deref().or(p.parent()).map(|f| f.display().to_string()).unwrap_or_default();
                let a = self.paused(|ui| ui.allow_file_changes(&folder));
                self.files.write = Some(a);
                a
            }
        };
        if !allowed {
            return Err(denied("the macro may not change files"));
        }
        Ok(p)
    }

    fn file_number(&mut self, e: &Expr) -> R<i32> {
        let n = self.eval_scalar(e)?.to_i64()?;
        if !(1..=511).contains(&n) {
            return Err(err(52));
        }
        Ok(n as i32)
    }

    pub(crate) fn file_stmt(&mut self, s: &FileStmt) -> R<()> {
        match s {
            FileStmt::Open { path, mode, read, write, num, len } => {
                let text = self.eval_scalar(path)?.to_str()?;
                let n = self.file_number(num)?;
                let len = match len {
                    Some(e) => {
                        let l = self.eval_scalar(e)?.to_i64()?;
                        if !(1..=32_767).contains(&l) {
                            return Err(err(59));
                        }
                        l as u64
                    }
                    None if *mode == FileMode::Random => 128,
                    None => 1,
                };
                self.open_file(&text, *mode, *read && !*write, n, len)
            }
            FileStmt::Close(nums) => {
                let mut list = Vec::new();
                for e in nums {
                    list.push(self.file_number(e)?);
                }
                if nums.is_empty() {
                    // Not FileSystemObject's text streams.
                    list = self.files.open.keys().copied().filter(|n| *n <= 511).collect();
                }
                let mut out = Ok(());
                for n in list {
                    if let Some(Handle { body: Body::Out { mut w, .. }, .. }) = self.files.open.remove(&n) {
                        out = out.and(w.flush().map_err(io));
                    }
                }
                out
            }
            FileStmt::Print { num, items } => {
                let n = self.file_number(num)?;
                let col = match &self.files.handle(n)?.body {
                    Body::Out { col, .. } => *col,
                    _ => return Err(err(54)),
                };
                let (mut out, mut col, newline) = self.print_line(items, col)?;
                if newline {
                    out.push_str("\r\n");
                    col = 0;
                }
                self.emit(n, &out, col, None)
            }
            FileStmt::Write { num, items, open } => {
                let n = self.file_number(num)?;
                let mut comma = match &self.files.handle(n)?.body {
                    Body::Out { comma, .. } => *comma,
                    _ => return Err(err(54)),
                };
                let mut out = String::new();
                for e in items {
                    let v = self.eval_scalar(e)?;
                    if comma {
                        out.push(',');
                    }
                    out.push_str(&write_text(&v)?);
                    comma = true;
                    value::string_room(out.len())?;
                }
                if !*open {
                    out.push_str("\r\n");
                    comma = false;
                }
                self.emit(n, &out, 0, Some(comma))
            }
            FileStmt::Input { num, vars } => {
                let n = self.file_number(num)?;
                for var in vars {
                    let v = match &mut self.files.handle(n)?.body {
                        Body::Text { text, pos, .. } => next_field(text, pos).ok_or_else(|| err(62))?,
                        _ => return Err(err(54)),
                    };
                    self.assign(var, v, false)?;
                }
                Ok(())
            }
            FileStmt::LineInput { num, var } => {
                let n = self.file_number(num)?;
                let line = match &mut self.files.handle(n)?.body {
                    Body::Text { text, pos, .. } => {
                        if *pos >= text.len() {
                            return Err(err(62));
                        }
                        let rest = &text[*pos..];
                        let end = rest.find(['\r', '\n']).unwrap_or(rest.len());
                        let line = rest[..end].to_string();
                        let mut skip = end;
                        if rest[end..].starts_with("\r\n") {
                            skip += 2;
                        } else if end < rest.len() {
                            skip += 1;
                        }
                        *pos += skip;
                        line
                    }
                    _ => return Err(err(54)),
                };
                self.assign(var, Value::str(line), false)
            }
            FileStmt::Get { num, pos, var, put } => {
                let n = self.file_number(num)?;
                let at = match pos {
                    Some(e) => {
                        let p = self.eval_scalar(e)?.to_i64()?;
                        if p < 1 {
                            return Err(err(63));
                        }
                        Some(p as u64)
                    }
                    None => None,
                };
                if *put { self.put_record(n, at, var) } else { self.get_record(n, at, var) }
            }
            FileStmt::Seek { num, pos } => {
                let n = self.file_number(num)?;
                let p = self.eval_scalar(pos)?.to_i64()?;
                if p < 1 {
                    return Err(err(63));
                }
                let p = p as u64 - 1;
                match &mut self.files.handle(n)?.body {
                    Body::Text { text, pos, .. } => {
                        let mut at = (p.min(text.len() as u64)) as usize;
                        while !text.is_char_boundary(at) {
                            at -= 1;
                        }
                        *pos = at;
                    }
                    Body::Out { w, .. } => {
                        w.seek(SeekFrom::Start(p)).map_err(io)?;
                    }
                    Body::Bin { f, len } => {
                        f.seek(SeekFrom::Start(p.saturating_mul(*len))).map_err(io)?;
                    }
                }
                Ok(())
            }
            FileStmt::Name { from, to } => {
                let from = self.eval_scalar(from)?.to_str()?;
                let to = self.eval_scalar(to)?.to_str()?;
                let from = self.path_to_change(&from)?;
                let to = self.path_to_change(&to)?;
                if fs::symlink_metadata(&from).is_err() {
                    return Err(err(53));
                }
                if fs::symlink_metadata(&to).is_ok() {
                    return Err(err(58));
                }
                if self.files.is_open(&from) || self.files.holds_in_use(&from) {
                    return Err(err(75));
                }
                if to.parent().is_some_and(|d| !d.is_dir()) {
                    return Err(err(76));
                }
                // A folder moves with all it holds: all of it must be the
                // macro's to change, as for FileSystemObject.MoveFolder.
                if fs::symlink_metadata(&from).is_ok_and(|m| m.is_dir()) {
                    if to.starts_with(&from) {
                        return Err(err(75));
                    }
                    self.files.tree(&from, true, false)?;
                }
                fs::rename(&from, &to).map_err(io)
            }
            FileStmt::Nothing => Ok(()),
        }
    }

    /// The text of `Print #` or `Debug.Print` items from column `col`, the
    /// column after it, and whether a new line follows (no `;` or `,` at
    /// the end).
    pub(crate) fn print_line(&mut self, items: &[PrintItem], mut col: usize) -> R<(String, usize, bool)> {
        let mut out = String::new();
        let mut newline = true;
        let pad = |out: &mut String, col: &mut usize, to: usize| {
            while *col < to {
                out.push(' ');
                *col += 1;
            }
        };
        for item in items {
            newline = true;
            match item {
                PrintItem::Expr(e) => {
                    let v = self.eval_scalar(e)?;
                    let t = print_text(&v)?;
                    col = match t.rfind('\n') {
                        Some(i) => t[i + 1..].chars().count(),
                        None => col + t.chars().count(),
                    };
                    out.push_str(&t);
                }
                PrintItem::Semi => newline = false,
                PrintItem::Comma | PrintItem::Tab(None) => {
                    let to = (col / 14 + 1) * 14;
                    pad(&mut out, &mut col, to);
                    newline = false;
                }
                PrintItem::Spc(e) => {
                    let k = self.eval_scalar(e)?.to_i64()?.clamp(0, 32_767) as usize;
                    let to = col + k;
                    pad(&mut out, &mut col, to);
                }
                PrintItem::Tab(Some(e)) => {
                    let to = self.eval_scalar(e)?.to_i64()?.clamp(1, 32_767) as usize - 1;
                    if col > to {
                        out.push_str("\r\n");
                        col = 0;
                    }
                    pad(&mut out, &mut col, to);
                }
            }
            value::string_room(out.len())?;
        }
        Ok((out, col, newline))
    }

    fn open_file(&mut self, text: &str, mode: FileMode, read_only: bool, n: i32, len: u64) -> R<()> {
        if self.files.open.contains_key(&n) {
            return Err(err(55));
        }
        if self.files.open.len() >= MAX_OPEN {
            return Err(err(67));
        }
        let changes = match mode {
            FileMode::Input => false,
            FileMode::Output | FileMode::Append => true,
            FileMode::Binary | FileMode::Random => !read_only,
        };
        let p = if changes { self.path_to_change(text)? } else { self.path_to_read(text)? };
        let writes = |m: FileMode| m != FileMode::Input;
        if self.files.open.values().any(|h| h.path == p && (changes || writes(h.mode))) {
            return Err(err(55));
        }
        match fs::metadata(&p) {
            // Not a folder, a device or a pipe (reading one could wait forever).
            Ok(m) if !m.is_file() => return Err(err(75)),
            Err(_) if !changes => return Err(err(53)),
            Err(_) if p.parent().is_some_and(|d| !d.is_dir()) => return Err(err(76)),
            _ => {}
        }
        let body = match mode {
            FileMode::Input => {
                let f = File::open(&p).map_err(io)?;
                let size = f.metadata().map_err(io)?.len();
                if size > MAX_READ {
                    return Err(Error::new(7, "Out of memory: files over 256 MB can't be opened For Input"));
                }
                let mut bytes = Vec::new();
                f.take(MAX_READ).read_to_end(&mut bytes).map_err(io)?;
                Body::Text { text: decode(bytes), pos: 0, size }
            }
            FileMode::Output | FileMode::Append => {
                let append = mode == FileMode::Append;
                let f = OpenOptions::new().write(true).create(true).truncate(!append).append(append).open(&p).map_err(io)?;
                Body::Out { w: BufWriter::new(f), col: 0, comma: false }
            }
            FileMode::Binary | FileMode::Random => {
                let f = OpenOptions::new().read(true).write(changes).create(changes).truncate(false).open(&p).map_err(io)?;
                Body::Bin { f, len }
            }
        };
        self.files.open.insert(n, Handle { path: p, mode, body, line: 1 });
        Ok(())
    }

    /// Writes `text` to file `n`, which then is at column `col`.
    fn emit(&mut self, n: i32, text: &str, col: usize, comma: Option<bool>) -> R<()> {
        self.files.count(text.len() as u64)?;
        let h = self.files.handle(n)?;
        match &mut h.body {
            Body::Out { w, col: c, comma: k } => {
                w.write_all(text.as_bytes()).map_err(io)?;
                h.line += text.matches('\n').count() as u64;
                *c = col;
                if let Some(x) = comma {
                    *k = x;
                }
                Ok(())
            }
            _ => Err(err(54)),
        }
    }

    /// The type a `Get` reads into `var`: what it's declared as.
    fn declared(&mut self, var: &Expr) -> R<(Value, VType, Option<u32>)> {
        let v = self.eval(var)?;
        if let Expr::Name(name) = var
            && let Some(s) = self.lookup(name)
        {
            let s = s.borrow();
            if !s.array {
                return Ok((v, s.ty, s.fixed));
            }
        }
        let ty = match &v {
            Value::Array(a) => a.ty,
            Value::Bool(_) => VType::Boolean,
            Value::Int(_) => VType::Integer,
            Value::Long(_) => VType::Long,
            Value::Double(_) => VType::Double,
            Value::Date(_) => VType::Date,
            Value::Str(_) => VType::String,
            _ => VType::Variant,
        };
        Ok((v, ty, None))
    }

    fn put_record(&mut self, n: i32, at: Option<u64>, var: &Expr) -> R<()> {
        let (v, ty, fixed) = self.declared(var)?;
        let random = self.files.handle(n)?.mode == FileMode::Random;
        let mut bytes = Vec::new();
        match &v {
            Value::Array(a) => {
                for x in &a.data {
                    encode(x, a.ty, None, false, &mut bytes)?;
                }
            }
            v => encode(v, ty, fixed, random, &mut bytes)?,
        }
        self.files.count(bytes.len() as u64)?;
        let Body::Bin { f, len } = &mut self.files.handle(n)?.body else { return Err(err(54)) };
        if random && bytes.len() as u64 > *len {
            return Err(err(59));
        }
        if let Some(p) = at {
            f.seek(SeekFrom::Start((p - 1).saturating_mul(*len))).map_err(io)?;
        }
        let start = f.stream_position().map_err(io)?;
        f.write_all(&bytes).map_err(io)?;
        next_record(f, start, *len, random)
    }

    fn get_record(&mut self, n: i32, at: Option<u64>, var: &Expr) -> R<()> {
        let (v, ty, fixed) = self.declared(var)?;
        let random = self.files.handle(n)?.mode == FileMode::Random;
        let Body::Bin { f, len } = &mut self.files.handle(n)?.body else { return Err(err(54)) };
        if let Some(p) = at {
            f.seek(SeekFrom::Start((p - 1).saturating_mul(*len))).map_err(io)?;
        }
        let start = f.stream_position().map_err(io)?;
        let new = match v {
            Value::Array(a) => {
                let mut a = Array::clone(&a);
                for x in &mut a.data {
                    *x = decode_value(f, a.ty, x, None, false)?;
                }
                Value::Array(Rc::new(a))
            }
            v => decode_value(f, ty, &v, fixed, random)?,
        };
        next_record(f, start, *len, random)?;
        self.assign(var, new, false)
    }

    /// The file functions: Dir, Kill, EOF...; None for other names.
    pub(crate) fn file_fn(&mut self, n: &str, a: &[Value]) -> Option<R<Value>> {
        const NAMES: &[&str] = &[
            "chdir", "chdrive", "curdir", "dir", "eof", "fileattr", "filecopy", "filedatetime", "filelen", "freefile", "getattr", "input", "kill", "loc", "lof", "mkdir", "reset", "rmdir", "seek", "setattr",
        ];
        if !NAMES.contains(&n) {
            return None;
        }
        Some(self.file_call(n, a))
    }

    fn file_call(&mut self, n: &str, a: &[Value]) -> R<Value> {
        self.no_udf()?;
        let arg = |i: usize| a.get(i).filter(|v| !matches!(v, Value::Missing));
        let need = |i: usize| arg(i).ok_or_else(|| Error::new(449, "Argument not optional"));
        let s = |i: usize| need(i)?.to_str();
        let num = |i: usize| -> R<i32> {
            let n = need(i)?.to_i64()?;
            if !(1..=511).contains(&n) {
                return Err(err(52));
            }
            Ok(n as i32)
        };
        Ok(match n {
            "freefile" => {
                let range = match arg(0) {
                    Some(v) => v.to_i64()?,
                    None => 0,
                };
                let span = if range == 1 { 256..=511 } else { 1..=255 };
                Value::Int(span.into_iter().find(|k| !self.files.open.contains_key(k)).ok_or_else(|| err(67))? as i16)
            }
            "reset" => {
                self.file_stmt(&FileStmt::Close(Vec::new()))?;
                Value::Empty
            }
            "eof" => {
                let n = num(0)?;
                Value::Bool(match &mut self.files.handle(n)?.body {
                    Body::Text { text, pos, .. } => *pos >= text.len(),
                    Body::Out { .. } => true,
                    Body::Bin { f, .. } => {
                        let at = f.stream_position().map_err(io)?;
                        at >= f.metadata().map_err(io)?.len()
                    }
                })
            }
            "lof" => {
                let n = num(0)?;
                let size = match &mut self.files.handle(n)?.body {
                    Body::Text { size, .. } => *size,
                    Body::Out { w, .. } => {
                        w.flush().map_err(io)?;
                        w.get_ref().metadata().map_err(io)?.len()
                    }
                    Body::Bin { f, .. } => f.metadata().map_err(io)?.len(),
                };
                Value::Double(size as f64)
            }
            "loc" | "seek" => {
                let seek = n == "seek";
                let n = num(0)?;
                let at = match &mut self.files.handle(n)?.body {
                    Body::Text { pos, .. } => {
                        if seek { *pos as u64 + 1 } else { (*pos as u64).div_ceil(128) }
                    }
                    Body::Out { w, .. } => {
                        let p = w.stream_position().map_err(io)?;
                        if seek { p + 1 } else { p.div_ceil(128) }
                    }
                    Body::Bin { f, len } => {
                        let p = f.stream_position().map_err(io)?;
                        if seek { p / *len + 1 } else if *len == 1 { p } else { p / *len }
                    }
                };
                Value::Double(at as f64)
            }
            "fileattr" => {
                let n = num(0)?;
                Value::Long(match self.files.handle(n)?.mode {
                    FileMode::Input => 1,
                    FileMode::Output => 2,
                    FileMode::Random => 4,
                    FileMode::Append => 8,
                    FileMode::Binary => 32,
                })
            }
            "input" => {
                let k = need(0)?.to_i64()?;
                if k < 0 {
                    return Err(Error::invalid_call());
                }
                let k = k as usize;
                value::string_room(k)?;
                let n = num(1)?;
                let text = match &mut self.files.handle(n)?.body {
                    Body::Text { text, pos, .. } => {
                        let rest = &text[*pos..];
                        let end = rest.char_indices().nth(k).map_or(rest.len(), |(i, _)| i);
                        if rest[..end].chars().count() < k {
                            return Err(err(62));
                        }
                        *pos += end;
                        rest[..end].to_string()
                    }
                    Body::Bin { f, .. } => {
                        let mut buf = Vec::new();
                        Read::by_ref(f).take(k as u64).read_to_end(&mut buf).map_err(io)?;
                        if buf.len() < k {
                            return Err(err(62));
                        }
                        buf.iter().map(|&b| byte_char(b)).collect()
                    }
                    Body::Out { .. } => return Err(err(54)),
                };
                Value::str(text)
            }
            "dir" => self.dir(arg(0).cloned(), arg(1).cloned())?,
            "kill" => {
                let text = s(0)?;
                let (folder, pat) = split_pattern(&text);
                if pat.contains(['*', '?']) {
                    let dir = self.path_to_read(&folder)?;
                    let names = self.list(&dir, &pat, false)?;
                    if names.is_empty() {
                        return Err(err(53));
                    }
                    for name in names {
                        self.kill(&dir.join(name).to_string_lossy())?;
                    }
                } else {
                    self.kill(&text)?;
                }
                Value::Empty
            }
            "mkdir" => {
                let p = self.path_to_change(&s(0)?)?;
                if fs::symlink_metadata(&p).is_ok() {
                    return Err(err(75));
                }
                if p.parent().is_some_and(|d| !d.is_dir()) {
                    return Err(err(76));
                }
                fs::create_dir(&p).map_err(io)?;
                Value::Empty
            }
            "rmdir" => {
                let p = self.path_to_change(&s(0)?)?;
                if !p.is_dir() {
                    return Err(err(76));
                }
                if Some(&p) == self.files.root.as_ref() || fs::read_dir(&p).map_err(io)?.next().is_some() {
                    return Err(err(75));
                }
                fs::remove_dir(&p).map_err(io)?;
                Value::Empty
            }
            "chdir" => {
                let p = self.path_to_read(&s(0)?)?;
                if !p.is_dir() {
                    return Err(err(76));
                }
                self.files.reach(&p.join("x"), false)?;
                self.files.cwd = Some(p);
                Value::Empty
            }
            "chdrive" => Value::Empty,
            "curdir" => Value::str(self.files.cwd.as_ref().or(self.files.root.as_ref()).map(|p| p.display().to_string()).unwrap_or_default()),
            "filecopy" => {
                let from = self.path_to_read(&s(0)?)?;
                let to = self.path_to_change(&s(1)?)?;
                self.copy_file(&from, &to)?;
                Value::Empty
            }
            "filelen" => {
                let p = self.path_to_read(&s(0)?)?;
                let m = fs::metadata(&p).map_err(io)?;
                if !m.is_file() {
                    return Err(err(53));
                }
                Value::Double(m.len() as f64)
            }
            "filedatetime" => {
                let p = self.path_to_read(&s(0)?)?;
                self.file_date(fs::metadata(&p).map_err(io)?.modified())?
            }
            "getattr" => {
                let p = self.path_to_read(&s(0)?)?;
                let m = fs::metadata(&p).map_err(io)?;
                Value::Int(if m.is_dir() { 16 } else if m.permissions().readonly() { 1 } else { 0 })
            }
            "setattr" => {
                let p = self.path_to_change(&s(0)?)?;
                let attrs = need(1)?.to_i64()?;
                let m = fs::metadata(&p).map_err(io)?;
                if !m.is_file() {
                    return Err(err(53));
                }
                let mut perm = m.permissions();
                if perm.readonly() != (attrs & 1 != 0) {
                    #[allow(clippy::permissions_set_readonly_false)]
                    perm.set_readonly(attrs & 1 != 0);
                    fs::set_permissions(&p, perm).map_err(io)?;
                }
                Value::Empty
            }
            _ => return Err(err(5)),
        })
    }

    /// Copies file `from` to `to`, both reachable.
    fn copy_file(&mut self, from: &Path, to: &Path) -> R<()> {
        let m = fs::metadata(from).map_err(io)?;
        if !m.is_file() {
            return Err(err(53));
        }
        if to == from || self.files.is_open(to) || self.files.is_open(from) || to.is_dir() {
            return Err(err(75));
        }
        self.files.count(m.len())?;
        // A new file of its own (not a copy of `from`'s permissions).
        let mut src = File::open(from).map_err(io)?;
        let mut dst = OpenOptions::new().write(true).create(true).truncate(true).open(to).map_err(io)?;
        std::io::copy(&mut Read::by_ref(&mut src).take(m.len()), &mut dst).map_err(io)?;
        Ok(())
    }

    /// Workbook.SaveAs (`keep`: the workbook is that file from now on) and
    /// SaveCopyAs: the workbook as it is now written at `text`, in
    /// `format` (xlFileFormat) or the one its extension says.
    pub(crate) fn save_book(&mut self, text: &str, format: Option<i64>, keep: bool) -> R<()> {
        self.no_udf()?;
        let bad = || Error::new(1004, "This extension can't be used with the selected file type");
        let ext_of = |t: &str| split_pattern(t).1.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
        let mut text = text.trim().to_string();
        let current = ext_of(&self.file_name).unwrap_or_else(|| "xlsx".into());
        let (fmt, exts): (BookFormat, &[&str]) = match format {
            None | Some(-4143) => {
                let ext = ext_of(&text).unwrap_or_else(|| current.clone());
                match ext.as_str() {
                    "xlsx" | "xlsm" | "xltx" | "xltm" => (BookFormat::Xlsx, &["xlsx", "xlsm", "xltx", "xltm"]),
                    "ods" | "ots" => (BookFormat::Ods, &["ods", "ots"]),
                    "fods" => (BookFormat::Fods, &["fods"]),
                    "csv" => (BookFormat::Csv, &["csv"]),
                    "tsv" | "tab" => (BookFormat::Tsv, &["tsv", "tab"]),
                    _ => return Err(Error::new(1004, format!("Workbooks can't be saved as .{ext} files"))),
                }
            }
            Some(51) => (BookFormat::Xlsx, &["xlsx"]),
            Some(52) => (BookFormat::Xlsx, &["xlsm"]),
            Some(53) => (BookFormat::Xlsx, &["xltm"]),
            Some(54) => (BookFormat::Xlsx, &["xltx"]),
            Some(6 | 22 | 23 | 24 | 62) => (BookFormat::Csv, &["csv"]),
            Some(-4158 | 20 | 21 | 42) => (BookFormat::Tsv, &["txt", "tab", "tsv"]),
            Some(60) => (BookFormat::Ods, &["ods"]),
            Some(f) => return Err(Error::new(1004, format!("File format {f} can't be saved"))),
        };
        match ext_of(&text) {
            Some(e) if !exts.contains(&e.as_str()) => return Err(bad()),
            Some(_) => {}
            None => {
                let ext = if exts.contains(&current.as_str()) { current.as_str() } else { exts[0] };
                text = format!("{text}.{ext}");
            }
        }
        let p = self.changing(&text, true)?;
        // Another workbook's macros don't go to new files (as FileCopy).
        if matches!(self.wb, Book::Owned(_)) && self.files.book.as_ref() != Some(&p) && self.wb.macros.has_code() && !matches!(ext_of(&text).as_deref(), Some("xlsx" | "xltx" | "csv" | "tsv" | "tab" | "txt")) {
            return Err(denied("macros don't copy another workbook's macros to a new file (save it as .xlsx)"));
        }
        if self.other_book_files().contains(&p) {
            return Err(Error::new(1004, "Another workbook open in the macro is that file"));
        }
        if self.files.is_open(&p) || p.is_dir() {
            return Err(err(75));
        }
        if p.parent().is_some_and(|d| !d.is_dir()) {
            return Err(err(76));
        }
        let path = p.display().to_string();
        let t = std::time::Instant::now();
        let res = match &self.wb {
            Book::Mut(wb) => self.ui.save_workbook(wb, &path, fmt, self.active),
            Book::Owned(wb) => self.ui.save_opened(wb, &path, fmt, self.active),
            Book::Ref(_) => return Err(denied("a function called from a cell can't save")),
        };
        self.paused += t.elapsed();
        res.map_err(|e| Error::new(1004, format!("The workbook couldn't be saved: {e}")))?;
        if keep {
            self.file_name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            self.file_path = p.parent().map(|d| d.display().to_string()).unwrap_or_default();
            self.files.book = Some(p);
            self.wb_mut()?.modified = false;
            self.saved_as = Some((path, self.undo.len()));
        }
        Ok(())
    }

    /// Document.SaveAs (`keep`: the document is that file from now on):
    /// the document as it is now written at `text`, in `format`
    /// (WdSaveFormat) or the one its extension says.
    pub(crate) fn save_doc(&mut self, text: &str, format: Option<i64>, keep: bool) -> R<()> {
        let bad = || Error::new(5152, "This extension can't be used with the selected file type");
        let ext_of = |t: &str| split_pattern(t).1.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
        let mut text = text.trim().to_string();
        let current = ext_of(&self.file_name).unwrap_or_else(|| "docx".into());
        let (fmt, exts): (DocFormat, &[&str]) = match format {
            None => {
                let ext = ext_of(&text).unwrap_or_else(|| current.clone());
                match ext.as_str() {
                    "docx" | "docm" | "dotx" | "dotm" => (DocFormat::Docx, &["docx", "docm", "dotx", "dotm"]),
                    "odt" | "ott" => (DocFormat::Odt, &["odt", "ott"]),
                    "fodt" => (DocFormat::Fodt, &["fodt"]),
                    "rtf" => (DocFormat::Rtf, &["rtf"]),
                    "txt" => (DocFormat::Text, &["txt"]),
                    "md" | "markdown" => (DocFormat::Markdown, &["md", "markdown"]),
                    _ => return Err(Error::new(5152, format!("Documents can't be saved as .{ext} files"))),
                }
            }
            Some(12 | 16) => (DocFormat::Docx, &["docx"]),
            Some(13) => (DocFormat::Docx, &["docm"]),
            Some(14) => (DocFormat::Docx, &["dotx"]),
            Some(15) => (DocFormat::Docx, &["dotm"]),
            Some(23) => (DocFormat::Odt, &["odt"]),
            Some(6) => (DocFormat::Rtf, &["rtf"]),
            Some(2 | 3 | 4 | 5 | 7) => (DocFormat::Text, &["txt"]),
            Some(f) => return Err(Error::new(5152, format!("File format {f} can't be saved"))),
        };
        match ext_of(&text) {
            Some(e) if !exts.contains(&e.as_str()) => return Err(bad()),
            Some(_) => {}
            None => {
                let ext = if exts.contains(&current.as_str()) { current.as_str() } else { exts[0] };
                text = format!("{text}.{ext}");
            }
        }
        let p = self.changing(&text, true)?;
        let Some(w) = self.word.as_ref() else { return Err(Error::no_member("SaveAs")) };
        // Another document's macros don't go to new files (as FileCopy).
        if matches!(w.doc, crate::word::DocRef::Owned(_)) && self.files.book.as_ref() != Some(&p) && w.doc.macros.has_code() && !matches!(ext_of(&text).as_deref(), Some("docx" | "dotx" | "odt" | "ott" | "fodt" | "rtf" | "txt" | "md" | "markdown")) {
            return Err(denied("macros don't copy another document's macros to a new file (save it as .docx)"));
        }
        if self.wb.read_only && self.files.book.as_ref() == Some(&p) {
            return Err(Error::new(5155, format!("'{}' is read-only: use SaveAs with another name", self.file_name)));
        }
        if self.other_book_files().contains(&p) {
            return Err(Error::new(5152, "Another document open in the macro is that file"));
        }
        if self.files.is_open(&p) || p.is_dir() {
            return Err(err(75));
        }
        if p.parent().is_some_and(|d| !d.is_dir()) {
            return Err(err(76));
        }
        let path = p.display().to_string();
        let t = std::time::Instant::now();
        let Some(w) = self.word.as_ref() else { return Err(Error::no_member("SaveAs")) };
        let res = self.ui.save_document(&w.doc, &path, fmt);
        self.paused += t.elapsed();
        res.map_err(|e| Error::new(5152, format!("The document couldn't be saved: {e}")))?;
        if keep {
            self.file_name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            self.file_path = p.parent().map(|d| d.display().to_string()).unwrap_or_default();
            self.files.book = Some(p);
            let rev = self.w().doc.revision;
            self.w().doc.modified = false;
            self.saved_as = Some((path, rev as usize));
        }
        Ok(())
    }

    /// A file's time as a date, local time.
    fn file_date(&self, t: std::io::Result<std::time::SystemTime>) -> R<Value> {
        let t = t.map_err(io)?;
        let secs = match t.duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_secs_f64(),
            Err(e) => -e.duration().as_secs_f64(),
        };
        let serial = 25_569.0 + (secs + self.utc_offset as f64) / 86_400.0;
        Ok(Value::Date((serial * 86_400.0).round() / 86_400.0))
    }

    fn kill(&mut self, text: &str) -> R<()> {
        let p = self.path_to_change(text)?;
        match fs::symlink_metadata(&p) {
            Ok(m) if m.is_file() => {}
            _ => return Err(err(53)),
        }
        if self.files.is_open(&p) {
            return Err(err(75));
        }
        fs::remove_file(&p).map_err(io)
    }

    /// The names in folder `dir` that match `pat` (folders too with `dirs`).
    fn list(&mut self, dir: &Path, pat: &str, dirs: bool) -> R<Vec<String>> {
        self.files.reach(&dir.join("x"), false)?;
        let pat = lower_chars(if pat.is_empty() { "*" } else { pat });
        let mut out = Vec::new();
        for e in fs::read_dir(dir).map_err(|_| err(76))? {
            let Ok(e) = e else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || !wild(&pat, &lower_chars(&name)) {
                continue;
            }
            let Ok(t) = fs::metadata(e.path()) else { continue };
            if t.is_file() || dirs && t.is_dir() {
                out.push(name);
            }
            if out.len() >= MAX_LISTED {
                break;
            }
        }
        out.sort();
        Ok(out)
    }

    /// Dir(pattern, attributes): the first name that matches; Dir(): the next.
    fn dir(&mut self, pattern: Option<Value>, attrs: Option<Value>) -> R<Value> {
        let Some(pattern) = pattern else {
            let it = self.files.listing.as_mut().ok_or_else(Error::invalid_call)?;
            return Ok(Value::str(it.next().unwrap_or_default()));
        };
        let text = pattern.to_str()?;
        let dirs = match attrs {
            Some(v) => v.to_i64()? & 16 != 0,
            None => false,
        };
        self.files.listing = None;
        if text.is_empty() {
            return Ok(Value::str(""));
        }
        let (folder, pat) = split_pattern(&text);
        // A file picked in a dialog, outside the folder.
        if !pat.contains(['*', '?'])
            && let Ok(p) = self.files.resolve(&text)
            && self.files.picked.iter().any(|(f, _)| *f == p)
        {
            return Ok(Value::str(if p.is_file() { pat } else { String::new() }));
        }
        let dir = self.path_to_read(&folder)?;
        if !dir.is_dir() {
            return Ok(Value::str(""));
        }
        let names = self.list(&dir, &pat, dirs)?;
        let mut it = names.into_iter();
        let first = it.next().unwrap_or_default();
        self.files.listing = Some(it);
        Ok(Value::str(first))
    }
}

/// After a record of a Random file, the next one starts.
fn next_record(f: &mut File, start: u64, len: u64, random: bool) -> R<()> {
    if random {
        f.seek(SeekFrom::Start((start / len + 1).saturating_mul(len))).map_err(io)?;
    }
    Ok(())
}

/// A value as `Print #` writes it: numbers with a space before (for the
/// sign) and after.
fn print_text(v: &Value) -> R<String> {
    Ok(match v {
        Value::Empty | Value::Missing => String::new(),
        Value::Null => "Null".into(),
        Value::Error(n) => format!("Error {n}"),
        Value::Bool(b) => if *b { "True" } else { "False" }.into(),
        Value::Int(_) | Value::Long(_) | Value::Double(_) => {
            let t = v.to_str()?;
            if t.starts_with('-') { format!("{t} ") } else { format!(" {t} ") }
        }
        v => v.to_str()?,
    })
}

/// A value as `Write #` writes it: strings quoted, `#TRUE#`, `#2024-01-31#`.
fn write_text(v: &Value) -> R<String> {
    Ok(match v {
        Value::Empty | Value::Missing => String::new(),
        Value::Null => "#NULL#".into(),
        Value::Error(n) => format!("#ERROR {n}#"),
        Value::Bool(b) => if *b { "#TRUE#" } else { "#FALSE#" }.into(),
        Value::Int(i) => i.to_string(),
        Value::Long(i) => i.to_string(),
        Value::Double(x) => number_text(*x),
        Value::Date(d) => {
            let (y, m, day) = serial_to_date(*d);
            let secs = ((d - d.floor()) * 86_400.0).round() as i64 % 86_400;
            let time = format!("{:02}:{:02}:{:02}", secs / 3600, secs % 3600 / 60, secs % 60);
            if d.floor() == 0.0 && secs != 0 {
                format!("#{time}#")
            } else if secs == 0 {
                format!("#{y:04}-{m:02}-{day:02}#")
            } else {
                format!("#{y:04}-{m:02}-{day:02} {time}#")
            }
        }
        v => format!("\"{}\"", v.to_str()?),
    })
}

/// The next field `Input #` reads from `text` at `pos`: quoted text,
/// `#TRUE#`, a number or text up to the next comma or line end.
fn next_field(text: &str, pos: &mut usize) -> Option<Value> {
    let b = text.as_bytes();
    let mut i = *pos;
    while i < b.len() && matches!(b[i], b' ' | b'\t') {
        i += 1;
    }
    if i >= b.len() {
        return None;
    }
    let v = if b[i] == b'"' {
        let start = i + 1;
        let end = text[start..].find('"').map_or(b.len(), |k| start + k);
        let v = Value::str(&text[start..end]);
        i = (end + 1).min(b.len());
        while i < b.len() && !matches!(b[i], b',' | b'\r' | b'\n') {
            i += 1;
        }
        v
    } else {
        let start = i;
        while i < b.len() && !matches!(b[i], b',' | b'\r' | b'\n') {
            i += 1;
        }
        field_value(text[start..i].trim())
    };
    if i < b.len() {
        i += if b[i] == b'\r' && b.get(i + 1) == Some(&b'\n') { 2 } else { 1 };
    }
    *pos = i;
    Some(v)
}

fn field_value(raw: &str) -> Value {
    if raw.is_empty() {
        return Value::Empty;
    }
    if let Some(inner) = raw.strip_prefix('#').and_then(|r| r.strip_suffix('#')) {
        let up = inner.to_ascii_uppercase();
        match up.as_str() {
            "TRUE" => return Value::Bool(true),
            "FALSE" => return Value::Bool(false),
            "NULL" => return Value::Null,
            _ => {}
        }
        if let Some(n) = up.strip_prefix("ERROR ").and_then(|n| n.trim().parse().ok()) {
            return Value::Error(n);
        }
        if let Some(d) = value::parse_date(inner) {
            return Value::Date(d);
        }
    }
    match raw.parse::<f64>() {
        Ok(x) if x.is_finite() => Value::Double(x),
        _ => Value::str(raw),
    }
}

/// `v` as `Put` writes a value of type `ty` (Random files: a string's
/// length first, unless it's `String * n`).
fn encode(v: &Value, ty: VType, fixed: Option<u32>, random: bool, out: &mut Vec<u8>) -> R<()> {
    match ty {
        VType::Byte => out.push(v.to_i64()? as u8),
        VType::Boolean => out.extend_from_slice(&(if v.to_bool()? { -1i16 } else { 0 }).to_le_bytes()),
        VType::Integer => out.extend_from_slice(&(v.to_i64()? as i16).to_le_bytes()),
        VType::Long => out.extend_from_slice(&v.to_long()?.to_le_bytes()),
        VType::LongLong => out.extend_from_slice(&v.to_i64()?.to_le_bytes()),
        VType::Double => out.extend_from_slice(&v.to_f64()?.to_le_bytes()),
        VType::Currency => out.extend_from_slice(&((v.to_f64()? * 10_000.0).round() as i64).to_le_bytes()),
        VType::Date => out.extend_from_slice(&v.to_date()?.to_le_bytes()),
        VType::String => {
            let s: Vec<u8> = v.to_str()?.chars().map(char_byte).collect();
            match fixed {
                Some(n) => out.extend((0..n as usize).map(|i| s.get(i).copied().unwrap_or(b' '))),
                None => {
                    if random {
                        let n = u16::try_from(s.len()).map_err(|_| err(59))?;
                        out.extend_from_slice(&n.to_le_bytes());
                    }
                    out.extend_from_slice(&s);
                }
            }
        }
        VType::Variant => {
            // Put of a Variant also writes its type: only typed values here.
            let ty = match v {
                Value::Bool(_) => VType::Boolean,
                Value::Int(_) => VType::Integer,
                Value::Long(_) => VType::Long,
                Value::Double(_) => VType::Double,
                Value::Date(_) => VType::Date,
                Value::Str(_) => VType::String,
                _ => return Err(Error::unsupported("Put of a Variant that holds no value (declare the variable's type)")),
            };
            encode(v, ty, None, random, out)?;
        }
        VType::Object => return Err(Error::type_mismatch()),
    }
    Ok(())
}

/// Reads a value of type `ty` (`old` gives a string's length): bytes past
/// the end of the file read as zeros.
fn decode_value(f: &mut File, ty: VType, old: &Value, fixed: Option<u32>, random: bool) -> R<Value> {
    let mut take = |n: usize| -> R<Vec<u8>> {
        let mut buf = Vec::with_capacity(n);
        Read::by_ref(f).take(n as u64).read_to_end(&mut buf).map_err(io)?;
        Ok(buf)
    };
    let mut fixed_bytes = |n: usize| -> R<Vec<u8>> {
        let mut b = take(n)?;
        b.resize(n, 0);
        Ok(b)
    };
    Ok(match ty {
        VType::Byte => Value::Int(i16::from(fixed_bytes(1)?[0])),
        VType::Boolean => Value::Bool(i16::from_le_bytes(fixed_bytes(2)?.try_into().unwrap_or_default()) != 0),
        VType::Integer => Value::Int(i16::from_le_bytes(fixed_bytes(2)?.try_into().unwrap_or_default())),
        VType::Long => Value::Long(i32::from_le_bytes(fixed_bytes(4)?.try_into().unwrap_or_default())),
        VType::LongLong => Value::Double(i64::from_le_bytes(fixed_bytes(8)?.try_into().unwrap_or_default()) as f64),
        VType::Double => Value::Double(f64::from_le_bytes(fixed_bytes(8)?.try_into().unwrap_or_default())),
        VType::Currency => Value::Double(i64::from_le_bytes(fixed_bytes(8)?.try_into().unwrap_or_default()) as f64 / 10_000.0),
        VType::Date => Value::Date(f64::from_le_bytes(fixed_bytes(8)?.try_into().unwrap_or_default())),
        VType::String => {
            let n = match fixed {
                Some(n) => n as usize,
                None if random => usize::from(u16::from_le_bytes(fixed_bytes(2)?.try_into().unwrap_or_default())),
                None => old.to_str()?.chars().count(),
            };
            value::string_room(n)?;
            Value::str(take(n)?.iter().map(|&b| byte_char(b)).collect::<String>())
        }
        VType::Variant => {
            let ty = match old {
                Value::Bool(_) => VType::Boolean,
                Value::Int(_) => VType::Integer,
                Value::Long(_) => VType::Long,
                Value::Double(_) => VType::Double,
                Value::Date(_) => VType::Date,
                Value::Str(_) => VType::String,
                _ => return Err(Error::unsupported("Get into a Variant that holds no value (declare the variable's type)")),
            };
            return decode_value(f, ty, old, None, random);
        }
        VType::Object => return Err(Error::type_mismatch()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_folders_are_shallow() {
        assert!(shallow(Path::new("/tmp")));
        assert!(shallow(Path::new("/")));
        assert!(shallow(Path::new("/home/someone")));
        assert!(!shallow(Path::new("/home/someone/Documents")));
    }

    #[test]
    fn wildcards() {
        let w = |p: &str, n: &str| wild(&lower_chars(p), &lower_chars(n));
        assert!(w("*.txt", "a.TXT"));
        assert!(w("a?c*", "abcdef"));
        assert!(!w("*.txt", "a.csv"));
        assert!(w("*", ""));
        assert!(!w("a*b*c", "aXbY"));
    }

    #[test]
    fn text_codes() {
        assert_eq!(decode(b"caf\xE9 \x80".to_vec()), "café €");
        assert_eq!(decode("ñ".as_bytes().to_vec()), "ñ");
        assert_eq!(char_byte('€'), 0x80);
        assert_eq!(char_byte('é'), 0xE9);
        assert_eq!(char_byte('中'), b'?');
    }

    #[test]
    fn fields() {
        let text = "\"a, b\",12,#TRUE#,,x y\r\n#2024-01-31#";
        let mut pos = 0;
        let mut out = Vec::new();
        while let Some(v) = next_field(text, &mut pos) {
            out.push(format!("{v:?}"));
        }
        assert_eq!(out.len(), 6, "{out:?}");
        assert!(out[0].contains("a, b") && out[1].contains("12") && out[2].contains("true") && out[3] == "Empty" && out[4].contains("x y") && out[5].contains("Date"));
    }
}
