//! Scripting.FileSystemObject with its TextStream, File, Folder, Files and
//! Folders: the same files as `Open` and `Kill` reach, with the same rules.

use std::fs;
use std::path::{MAIN_SEPARATOR, Path, PathBuf};

use super::{Body, MAX_LISTED, denied, err, io, split_pattern};
use crate::ast::FileMode;
use crate::error::{Error, R};
use crate::excel::Args;
use crate::interp::Interp;
use crate::object::{Obj, Part};
use crate::value::{self, Value};

/// How deep CopyFolder, MoveFolder and DeleteFolder go.
const MAX_DEPTH: usize = 64;

fn part(p: Part) -> Value {
    Value::Object(Obj::Part(Box::new(p)))
}

fn shown(p: &Path) -> String {
    p.display().to_string()
}

/// A name of a file or folder alone (not a path).
fn plain(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
}

/// `C:\a\b.txt` as `C:\a` and `b.txt` (trailing separators dropped).
fn split_name(path: &str) -> (&str, &str) {
    let t = path.trim_end_matches(['/', '\\']);
    match t.rfind(['/', '\\']) {
        Some(i) => (&t[..i], &t[i + 1..]),
        None => ("", t),
    }
}

fn ends_in_separator(text: &str) -> bool {
    text.ends_with(['/', '\\'])
}

impl super::Files {
    /// The files (or folders, with `dirs`) right in folder `dir`, by name.
    fn entries(&self, dir: &Path, dirs: bool) -> R<Vec<String>> {
        self.reach(&dir.join("x"), false)?;
        let mut out = Vec::new();
        for e in fs::read_dir(dir).map_err(|_| err(76))? {
            let Ok(e) = e else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let Ok(m) = fs::metadata(e.path()) else { continue };
            if if dirs { m.is_dir() } else { m.is_file() } {
                out.push(name);
            }
            if out.len() >= MAX_LISTED {
                break;
            }
        }
        out.sort();
        Ok(out)
    }

    /// Everything below folder `dir`, links not followed, each folder before
    /// what is in it. All of it must be reachable (`write`: to change);
    /// with `skip_hidden` hidden files are left out instead of refused.
    pub(super) fn tree(&self, dir: &Path, write: bool, skip_hidden: bool) -> R<Vec<(PathBuf, fs::Metadata)>> {
        let mut out = Vec::new();
        let mut stack = vec![(dir.to_path_buf(), 0)];
        while let Some((d, depth)) = stack.pop() {
            if depth >= MAX_DEPTH {
                return Err(Error::new(75, "Path/File access error: folders too deep"));
            }
            for e in fs::read_dir(&d).map_err(io)? {
                let e = e.map_err(io)?;
                let p = e.path();
                if skip_hidden && e.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                let m = fs::symlink_metadata(&p).map_err(io)?;
                self.reach(&p, write)?;
                if m.is_dir() {
                    stack.push((p.clone(), depth + 1));
                } else if !m.is_file() {
                    // A link, a device...: not followed nor copied.
                    return Err(err(75));
                }
                if out.len() >= MAX_LISTED {
                    return Err(Error::new(7, "Out of memory: too many files"));
                }
                out.push((p, m));
            }
        }
        Ok(out)
    }

    /// Whether `p` is or holds a folder macros reach from, the workbook or
    /// an open file: it can't be moved or deleted.
    pub(super) fn holds_in_use(&self, p: &Path) -> bool {
        let roots = self.root.iter().chain(self.folders.iter().map(|(f, _)| f)).chain(self.cwd.iter()).chain(self.book.iter());
        roots.chain(self.open.values().map(|h| &h.path)).any(|r| r.starts_with(p))
    }
}

impl Interp<'_> {
    /// CreateObject("Scripting.FileSystemObject").
    pub(crate) fn new_fso(&self) -> R<Value> {
        self.no_udf()?;
        Ok(part(Part::Fso))
    }

    fn fso_str(&mut self, a: &Args, pos: usize, name: &str) -> R<Option<String>> {
        match a.get(pos, name) {
            Some(v) => Ok(Some(self.scalar(v)?.to_str()?)),
            None => Ok(None),
        }
    }

    fn fso_need(&mut self, a: &Args, pos: usize, name: &str) -> R<String> {
        self.fso_str(a, pos, name)?.ok_or_else(|| Error::new(449, "Argument not optional"))
    }

    fn fso_bool(&mut self, a: &Args, pos: usize, name: &str, default: bool) -> R<bool> {
        match a.get(pos, name) {
            Some(v) => self.scalar(v)?.to_bool(),
            None => Ok(default),
        }
    }

    fn fso_int(&mut self, a: &Args, pos: usize, name: &str, default: i64) -> R<i64> {
        match a.get(pos, name) {
            Some(v) => self.scalar(v)?.to_i64(),
            None => Ok(default),
        }
    }

    pub(crate) fn fso_member(&mut self, name: &str, a: &Args) -> R<Value> {
        Ok(match name {
            "buildpath" => {
                let p = self.fso_need(a, 0, "path")?;
                let n = self.fso_need(a, 1, "name")?;
                let sep = if p.contains('\\') && !p.contains('/') { '\\' } else { MAIN_SEPARATOR };
                Value::str(if p.is_empty() || ends_in_separator(&p) || n.starts_with(['/', '\\']) { p + &n } else { format!("{p}{sep}{n}") })
            }
            "getfilename" => Value::str(split_name(&self.fso_need(a, 0, "path")?).1),
            "getbasename" | "getextensionname" => {
                let p = self.fso_need(a, 0, "path")?;
                let file = split_name(&p).1;
                let (base, ext) = file.rsplit_once('.').unwrap_or((file, ""));
                Value::str(if name == "getbasename" { base } else { ext })
            }
            "getparentfoldername" => {
                let p = self.fso_need(a, 0, "path")?;
                let (parent, _) = split_name(&p);
                Value::str(if parent.is_empty() && p.starts_with('/') { "/" } else { parent })
            }
            "getdrivename" => Value::str(""),
            "getabsolutepathname" => {
                let p = self.fso_need(a, 0, "path")?;
                Value::str(shown(&self.files.absolute(&p)?))
            }
            "gettempname" => {
                let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
                Value::str(format!("rad{:05X}.tmp", (n ^ (n >> 12)) & 0xF_FFFF))
            }
            "fileexists" | "folderexists" => {
                let p = self.fso_need(a, 0, "filespec")?;
                self.no_udf()?;
                // What macros don't reach doesn't exist for them.
                let found = self.files.resolve(&p).ok().filter(|p| self.files.reach(p, false).is_ok());
                Value::Bool(found.is_some_and(|p| if name == "fileexists" { p.is_file() } else { p.is_dir() }))
            }
            "driveexists" => Value::Bool(false),
            "getfile" => {
                let text = self.fso_need(a, 0, "filepath")?;
                let p = self.path_to_read(&text)?;
                if !p.is_file() {
                    return Err(err(53));
                }
                part(Part::FsoFile(p))
            }
            "getfolder" => {
                let text = self.fso_need(a, 0, "folderpath")?;
                let p = self.path_to_read(&text)?;
                if !p.is_dir() {
                    return Err(err(76));
                }
                part(Part::FsoFolder(p))
            }
            "getfileversion" => Value::str(""),
            "createfolder" => {
                let text = self.fso_need(a, 0, "path")?;
                part(Part::FsoFolder(self.create_folder(&text)?))
            }
            "createtextfile" => {
                let text = self.fso_need(a, 0, "filename")?;
                let overwrite = self.fso_bool(a, 1, "overwrite", true)?;
                self.open_stream(&text, 2, true, overwrite)?
            }
            "opentextfile" => {
                let text = self.fso_need(a, 0, "filename")?;
                let mode = self.fso_int(a, 1, "iomode", 1)?;
                let create = self.fso_bool(a, 2, "create", false)?;
                self.open_stream(&text, mode, create, true)?
            }
            "copyfile" | "movefile" => {
                let from = self.fso_need(a, 0, "source")?;
                let to = self.fso_need(a, 1, "destination")?;
                let overwrite = name == "copyfile" && self.fso_bool(a, 2, "overwritefiles", true)?;
                self.copy_or_move_files(&from, &to, overwrite, name == "movefile")?;
                Value::Empty
            }
            "deletefile" => {
                let spec = self.fso_need(a, 0, "filespec")?;
                let force = self.fso_bool(a, 1, "force", false)?;
                for p in self.matching(&spec, false, true)? {
                    self.delete_file(&p, force)?;
                }
                Value::Empty
            }
            "copyfolder" => {
                let from = self.fso_need(a, 0, "source")?;
                let to = self.fso_need(a, 1, "destination")?;
                let overwrite = self.fso_bool(a, 2, "overwritefiles", true)?;
                let into = ends_in_separator(&to) || split_pattern(&from).1.contains(['*', '?']);
                for p in self.matching(&from, true, false)? {
                    self.copy_folder(&p, &to, into, overwrite)?;
                }
                Value::Empty
            }
            "movefolder" => {
                let from = self.fso_need(a, 0, "source")?;
                let to = self.fso_need(a, 1, "destination")?;
                let into = ends_in_separator(&to) || split_pattern(&from).1.contains(['*', '?']);
                for p in self.matching(&from, true, true)? {
                    self.move_folder(&p, &to, into)?;
                }
                Value::Empty
            }
            "deletefolder" => {
                let spec = self.fso_need(a, 0, "folderspec")?;
                let force = self.fso_bool(a, 1, "force", false)?;
                for p in self.matching(&spec, true, true)? {
                    self.delete_folder(&p, force)?;
                }
                Value::Empty
            }
            "getspecialfolder" | "getdrive" | "drives" | "getstandardstream" => return Err(Error::unsupported(&format!("FileSystemObject.{name}"))),
            _ => return Err(Error::no_member(name)),
        })
    }

    /// The files (or folders) `spec` names, its last part maybe with
    /// wildcards; `change`: to be changed.
    fn matching(&mut self, spec: &str, dirs: bool, change: bool) -> R<Vec<PathBuf>> {
        let (folder, pat) = split_pattern(spec);
        let missing = if dirs { 76 } else { 53 };
        if !pat.contains(['*', '?']) {
            let p = if change { self.path_to_change(spec)? } else { self.path_to_read(spec)? };
            if if dirs { !p.is_dir() } else { !p.is_file() } {
                return Err(err(missing));
            }
            return Ok(vec![p]);
        }
        let dir = self.path_to_read(&folder)?;
        let pat = super::lower_chars(&pat);
        let names: Vec<String> = self.files.entries(&dir, dirs)?.into_iter().filter(|n| super::wild(&pat, &super::lower_chars(n))).collect();
        if names.is_empty() {
            return Err(err(missing));
        }
        let mut out = Vec::new();
        for n in names {
            let text = shown(&dir.join(n));
            out.push(if change { self.path_to_change(&text)? } else { self.path_to_read(&text)? });
        }
        Ok(out)
    }

    /// Where `from` goes for destination `to`: into it when it's a folder.
    fn destination(&mut self, from: &Path, to: &str, into: bool) -> R<PathBuf> {
        let d = self.files.resolve(to)?;
        if into || d.is_dir() && !ends_in_separator(to) && from.is_file() {
            if !d.is_dir() {
                return Err(err(76));
            }
            let name = from.file_name().ok_or_else(|| err(52))?;
            return self.path_to_change(&shown(&d.join(name)));
        }
        self.path_to_change(to)
    }

    fn copy_or_move_files(&mut self, from: &str, to: &str, overwrite: bool, moving: bool) -> R<()> {
        let into = ends_in_separator(to) || split_pattern(from).1.contains(['*', '?']);
        for src in self.matching(from, false, moving)? {
            let dst = self.destination(&src, to, into)?;
            if let Ok(m) = fs::symlink_metadata(&dst) {
                if moving || !overwrite || m.is_dir() {
                    return Err(err(58));
                }
                if m.permissions().readonly() {
                    return Err(denied("the file is read-only"));
                }
            }
            if moving {
                if self.files.is_open(&src) || dst.starts_with(&src) {
                    return Err(err(75));
                }
                if dst.parent().is_some_and(|d| !d.is_dir()) {
                    return Err(err(76));
                }
                fs::rename(&src, &dst).map_err(io)?;
            } else {
                self.copy_file(&src, &dst)?;
            }
        }
        Ok(())
    }

    fn delete_file(&mut self, p: &Path, force: bool) -> R<()> {
        let m = fs::symlink_metadata(p).map_err(io)?;
        if !m.is_file() {
            return Err(err(53));
        }
        if m.permissions().readonly() && !force {
            return Err(denied("the file is read-only"));
        }
        if self.files.is_open(p) {
            return Err(err(75));
        }
        fs::remove_file(p).map_err(io)
    }

    fn create_folder(&mut self, text: &str) -> R<PathBuf> {
        let p = self.path_to_change(text)?;
        if fs::symlink_metadata(&p).is_ok() {
            return Err(err(58));
        }
        if p.parent().is_some_and(|d| !d.is_dir()) {
            return Err(err(76));
        }
        fs::create_dir(&p).map_err(io)?;
        Ok(p)
    }

    fn copy_folder(&mut self, src: &Path, to: &str, into: bool, overwrite: bool) -> R<()> {
        let dst = if into {
            let d = self.files.resolve(to)?;
            if !d.is_dir() {
                return Err(err(76));
            }
            self.path_to_change(&shown(&d.join(src.file_name().ok_or_else(|| err(52))?)))?
        } else {
            self.path_to_change(to)?
        };
        if dst.starts_with(src) {
            return Err(Error::new(75, "Path/File access error: a folder can't be copied into itself"));
        }
        let items = self.files.tree(src, false, true)?;
        match fs::symlink_metadata(&dst) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => return Err(err(58)),
            Err(_) => {
                if dst.parent().is_some_and(|d| !d.is_dir()) {
                    return Err(err(76));
                }
                fs::create_dir(&dst).map_err(io)?;
            }
        }
        for (p, m) in items {
            let rel = p.strip_prefix(src).map_err(|_| err(75))?;
            let target = self.path_to_change(&shown(&dst.join(rel)))?;
            let there = fs::symlink_metadata(&target).ok();
            if m.is_dir() {
                match there {
                    Some(t) if t.is_dir() => {}
                    Some(_) => return Err(err(58)),
                    None => fs::create_dir(&target).map_err(io)?,
                }
            } else {
                if let Some(t) = there {
                    if !overwrite || t.is_dir() {
                        return Err(err(58));
                    }
                    if t.permissions().readonly() {
                        return Err(denied("the file is read-only"));
                    }
                }
                self.copy_file(&p, &target)?;
            }
        }
        Ok(())
    }

    fn move_folder(&mut self, src: &Path, to: &str, into: bool) -> R<()> {
        let dst = if into {
            let d = self.files.resolve(to)?;
            if !d.is_dir() {
                return Err(err(76));
            }
            self.path_to_change(&shown(&d.join(src.file_name().ok_or_else(|| err(52))?)))?
        } else {
            self.path_to_change(to)?
        };
        if self.files.holds_in_use(src) || dst.starts_with(src) {
            return Err(err(75));
        }
        if fs::symlink_metadata(&dst).is_ok() {
            return Err(err(58));
        }
        if dst.parent().is_some_and(|d| !d.is_dir()) {
            return Err(err(76));
        }
        // All of it must be the macro's to change.
        self.files.tree(src, true, false)?;
        fs::rename(src, &dst).map_err(io)
    }

    fn delete_folder(&mut self, p: &Path, force: bool) -> R<()> {
        if self.files.holds_in_use(p) {
            return Err(err(75));
        }
        // Checked whole first: nothing is deleted if any of it can't be.
        let items = self.files.tree(p, true, false)?;
        for (q, m) in &items {
            if m.is_file() {
                self.files.writable(q)?;
                if m.permissions().readonly() && !force {
                    return Err(denied("a file in the folder is read-only"));
                }
            }
        }
        for (q, m) in items.iter().rev() {
            if m.is_dir() { fs::remove_dir(q) } else { fs::remove_file(q) }.map_err(io)?;
        }
        fs::remove_dir(p).map_err(io)
    }

    /// OpenTextFile, CreateTextFile, OpenAsTextStream: a TextStream.
    fn open_stream(&mut self, text: &str, iomode: i64, create: bool, overwrite: bool) -> R<Value> {
        let mode = match iomode {
            1 => FileMode::Input,
            2 => FileMode::Output,
            8 => FileMode::Append,
            _ => return Err(Error::invalid_call()),
        };
        if mode == FileMode::Input {
            // Made empty when missing, with `create`.
            if create && fs::symlink_metadata(self.path_to_read(text)?).is_err() {
                let p = self.path_to_change(text)?;
                if p.parent().is_some_and(|d| !d.is_dir()) {
                    return Err(err(76));
                }
                fs::OpenOptions::new().write(true).create_new(true).open(&p).map_err(io)?;
            }
        } else {
            let p = self.path_to_change(text)?;
            let exists = fs::symlink_metadata(&p).is_ok();
            if exists && !overwrite {
                return Err(err(58));
            }
            if !exists && !create {
                return Err(err(53));
            }
        }
        let n = self.files.next_stream;
        if n == i32::MAX {
            return Err(err(67));
        }
        self.open_file(text, mode, false, n, 1)?;
        self.files.next_stream += 1;
        Ok(part(Part::TextStream(n)))
    }

    pub(crate) fn stream_member(&mut self, n: i32, name: &str, a: &Args) -> R<Value> {
        let reading = matches!(name, "readline" | "readall" | "read" | "skip" | "skipline" | "atendofstream" | "atendofline");
        Ok(match name {
            "close" => {
                if let Some(super::Handle { body: Body::Out { mut w, .. }, .. }) = self.files.open.remove(&n) {
                    std::io::Write::flush(&mut w).map_err(io)?;
                }
                Value::Empty
            }
            "line" => Value::Long(self.files.handle(n)?.line.min(i32::MAX as u64) as i32),
            "column" => {
                let col = match &self.files.handle(n)?.body {
                    Body::Text { text, pos, .. } => text[..*pos].rsplit('\n').next().map_or(0, |l| l.chars().count()),
                    Body::Out { col, .. } => *col,
                    Body::Bin { .. } => return Err(err(54)),
                };
                Value::Long(col.min(i32::MAX as usize - 1) as i32 + 1)
            }
            "write" | "writeline" | "writeblanklines" => {
                let text = match name {
                    "write" => self.fso_need(a, 0, "text")?,
                    "writeline" => self.fso_str(a, 0, "text")?.unwrap_or_default() + "\r\n",
                    _ => {
                        let k = self.fso_int(a, 0, "lines", 0)?.max(0) as usize;
                        value::string_room(k.saturating_mul(2))?;
                        "\r\n".repeat(k)
                    }
                };
                let col = match &self.files.handle(n)?.body {
                    Body::Out { col, .. } => *col,
                    _ => return Err(err(54)),
                };
                let col = match text.rfind('\n') {
                    Some(i) => text[i + 1..].chars().count(),
                    None => col + text.chars().count(),
                };
                self.emit(n, &text, col, None)?;
                Value::Empty
            }
            _ if reading => {
                let k = match name {
                    "read" | "skip" => {
                        let k = self.fso_int(a, 0, "characters", 0)?;
                        if k < 0 {
                            return Err(Error::invalid_call());
                        }
                        value::string_room(k as usize)?;
                        k as usize
                    }
                    _ => 0,
                };
                let h = self.files.handle(n)?;
                let Body::Text { text, pos, .. } = &mut h.body else { return Err(err(54)) };
                let rest = &text[*pos..];
                match name {
                    "atendofstream" => return Ok(Value::Bool(rest.is_empty())),
                    "atendofline" => return Ok(Value::Bool(rest.is_empty() || rest.starts_with(['\r', '\n']))),
                    _ => {}
                }
                if rest.is_empty() {
                    return Err(err(62));
                }
                let (taken, skip) = match name {
                    "readline" | "skipline" => {
                        let end = rest.find(['\r', '\n']).unwrap_or(rest.len());
                        let skip = if rest[end..].starts_with("\r\n") { end + 2 } else if end < rest.len() { end + 1 } else { end };
                        (&rest[..end], skip)
                    }
                    "readall" => (rest, rest.len()),
                    _ => {
                        let end = rest.char_indices().nth(k).map_or(rest.len(), |(i, _)| i);
                        (&rest[..end], end)
                    }
                };
                let lines = if matches!(name, "readline" | "skipline") { 1 } else { taken.matches('\n').count() as u64 };
                let out = if matches!(name, "skip" | "skipline") { Value::Empty } else { Value::str(taken) };
                *pos += skip;
                h.line += lines;
                out
            }
            _ => return Err(Error::no_member(name)),
        })
    }

    pub(crate) fn fso_file_member(&mut self, p: PathBuf, name: &str, a: &Args) -> R<Value> {
        let dir = p.is_dir();
        Ok(match name {
            "name" | "shortname" => Value::str(p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| shown(&p))),
            "path" | "shortpath" | "_default" => Value::str(shown(&p)),
            "drive" => Value::str(""),
            "parentfolder" => match p.parent() {
                Some(d) => part(Part::FsoFolder(d.to_path_buf())),
                None => Value::Nothing,
            },
            "isrootfolder" => Value::Bool(p.parent().is_none()),
            "type" => Value::str(if dir {
                "File folder".to_string()
            } else {
                match p.extension() {
                    Some(e) => format!("{} File", e.to_string_lossy().to_uppercase()),
                    None => "File".to_string(),
                }
            }),
            "size" | "datecreated" | "datelastmodified" | "datelastaccessed" | "attributes" => {
                let p = self.path_to_read(&shown(&p))?;
                let m = fs::metadata(&p).map_err(io)?;
                match name {
                    "size" => {
                        let size = if m.is_dir() { self.files.tree(&p, false, true)?.iter().filter(|(_, m)| m.is_file()).map(|(_, m)| m.len()).sum() } else { m.len() };
                        if size <= i32::MAX as u64 { Value::Long(size as i32) } else { Value::Double(size as f64) }
                    }
                    "datecreated" => self.file_date(m.created().or_else(|_| m.modified()))?,
                    "datelastmodified" => self.file_date(m.modified())?,
                    "datelastaccessed" => self.file_date(m.accessed())?,
                    _ => Value::Long(if m.is_dir() { 16 } else if m.permissions().readonly() { 1 } else { 0 }),
                }
            }
            // `.SubFolders("x")`: an item.
            "files" | "subfolders" if dir && a.len() > 0 => return self.fso_list_member(p, name == "subfolders", "item", a),
            "files" if dir => part(Part::FsoFiles(p)),
            "subfolders" if dir => part(Part::FsoFolders(p)),
            "createtextfile" if dir => {
                let file = self.fso_need(a, 0, "filename")?;
                let overwrite = self.fso_bool(a, 1, "overwrite", true)?;
                self.open_stream(&shown(&p.join(file)), 2, true, overwrite)?
            }
            "openastextstream" if !dir => {
                let mode = self.fso_int(a, 0, "iomode", 1)?;
                self.open_stream(&shown(&p), mode, false, true)?
            }
            "copy" => {
                let to = self.fso_need(a, 0, "destination")?;
                let overwrite = self.fso_bool(a, 1, "overwritefiles", true)?;
                if dir {
                    self.copy_folder(&p, &to, ends_in_separator(&to), overwrite)?;
                } else {
                    self.copy_or_move_files(&shown(&p), &to, overwrite, false)?;
                }
                Value::Empty
            }
            "move" => {
                let to = self.fso_need(a, 0, "destination")?;
                if dir {
                    let src = self.path_to_change(&shown(&p))?;
                    self.move_folder(&src, &to, ends_in_separator(&to))?;
                } else {
                    self.copy_or_move_files(&shown(&p), &to, false, true)?;
                }
                Value::Empty
            }
            "delete" => {
                let force = self.fso_bool(a, 0, "force", false)?;
                let q = self.path_to_change(&shown(&p))?;
                if dir { self.delete_folder(&q, force)? } else { self.delete_file(&q, force)? }
                Value::Empty
            }
            _ => return Err(Error::no_member(name)),
        })
    }

    pub(crate) fn fso_file_set(&mut self, p: PathBuf, name: &str, v: Value) -> R<()> {
        match name {
            "name" => {
                let new = v.to_str()?;
                if !plain(&new) {
                    return Err(err(52));
                }
                let from = self.path_to_change(&shown(&p))?;
                let to = self.path_to_change(&shown(&p.with_file_name(&new)))?;
                if fs::symlink_metadata(&to).is_ok() {
                    return Err(err(58));
                }
                if self.files.is_open(&from) || from.is_dir() && self.files.holds_in_use(&from) {
                    return Err(err(75));
                }
                if from.is_dir() {
                    self.files.tree(&from, true, false)?;
                }
                fs::rename(&from, &to).map_err(io)
            }
            "attributes" => {
                let attrs = v.to_i64()?;
                let p = self.path_to_change(&shown(&p))?;
                let m = fs::metadata(&p).map_err(io)?;
                if !m.is_file() {
                    return Ok(());
                }
                let mut perm = m.permissions();
                if perm.readonly() != (attrs & 1 != 0) {
                    #[allow(clippy::permissions_set_readonly_false)]
                    perm.set_readonly(attrs & 1 != 0);
                    fs::set_permissions(&p, perm).map_err(io)?;
                }
                Ok(())
            }
            _ => Err(Error::no_member(name)),
        }
    }

    /// A folder's Files or SubFolders.
    pub(crate) fn fso_list_member(&mut self, dir: PathBuf, dirs: bool, name: &str, a: &Args) -> R<Value> {
        self.no_udf()?;
        Ok(match name {
            "count" => Value::Long(self.files.entries(&dir, dirs)?.len() as i32),
            "item" | "_default" => {
                let key = self.fso_need(a, 0, "key")?;
                if !plain(&key) || key.starts_with('.') {
                    return Err(Error::invalid_call());
                }
                self.files.reach(&dir.join(&key), false)?;
                let p = dir.join(&key);
                let found = fs::metadata(&p).is_ok_and(|m| if dirs { m.is_dir() } else { m.is_file() });
                if !found {
                    return Err(Error::invalid_call());
                }
                part(if dirs { Part::FsoFolder(p) } else { Part::FsoFile(p) })
            }
            "add" if dirs => {
                let key = self.fso_need(a, 0, "name")?;
                if !plain(&key) {
                    return Err(err(52));
                }
                part(Part::FsoFolder(self.create_folder(&shown(&dir.join(key)))?))
            }
            _ => return Err(Error::no_member(name)),
        })
    }

    /// For Each over a folder's Files or SubFolders.
    pub(crate) fn fso_items(&self, dir: &Path, dirs: bool) -> Option<Vec<Value>> {
        let names = self.files.entries(dir, dirs).ok()?;
        Some(names.into_iter().map(|n| part(if dirs { Part::FsoFolder(dir.join(n)) } else { Part::FsoFile(dir.join(n)) })).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(split_name("C:\\a\\b.txt"), ("C:\\a", "b.txt"));
        assert_eq!(split_name("/a/b/"), ("/a", "b"));
        assert_eq!(split_name("b"), ("", "b"));
        assert!(plain("a.txt") && !plain("..") && !plain("a/b") && !plain(""));
    }
}
