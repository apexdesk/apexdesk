//! A file system in memory with `std::fs`'s calls, for the browser. Paths
//! are absolute from `/` (a relative one starts there); writing a file
//! makes its folders. One thread, as in the browser.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

pub use web_time::SystemTime;

#[derive(Debug)]
struct Data {
    bytes: Vec<u8>,
    modified: SystemTime,
}

#[derive(Clone)]
enum Node {
    Dir(SystemTime),
    File(Rc<RefCell<Data>>),
}

thread_local! {
    static TREE: RefCell<BTreeMap<PathBuf, Node>> = RefCell::new(BTreeMap::new());
}

/// `p` as an absolute path without `.` and `..`.
fn norm(p: &Path) -> PathBuf {
    let mut out = PathBuf::from("/");
    for c in p.components() {
        match c {
            Component::RootDir | Component::Prefix(_) => out = PathBuf::from("/"),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(s) => out.push(s),
        }
    }
    out
}

fn not_found() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "no such file or folder")
}

fn get(p: &Path) -> Option<Node> {
    let p = norm(p);
    if p == Path::new("/") {
        return Some(Node::Dir(SystemTime::UNIX_EPOCH));
    }
    TREE.with(|t| t.borrow().get(&p).cloned())
}

/// Makes `p`'s folders (not `p`).
fn make_parents(t: &mut BTreeMap<PathBuf, Node>, p: &Path) -> io::Result<()> {
    let mut cur = p.parent();
    let mut todo = Vec::new();
    while let Some(d) = cur {
        if d == Path::new("/") {
            break;
        }
        match t.get(d) {
            Some(Node::Dir(_)) => break,
            Some(Node::File(_)) => return Err(io::Error::other("a file is in the way")),
            None => todo.push(d.to_path_buf()),
        }
        cur = d.parent();
    }
    for d in todo {
        t.insert(d, Node::Dir(SystemTime::now()));
    }
    Ok(())
}

/// What is under folder `p` (its descendants), `p` normalized.
fn under(t: &BTreeMap<PathBuf, Node>, p: &Path) -> Vec<PathBuf> {
    t.keys().filter(|k| k.starts_with(p) && k.as_path() != p).cloned().collect()
}

fn file_data(p: &Path, create: bool, truncate: bool, new: bool) -> io::Result<Rc<RefCell<Data>>> {
    let p = norm(p);
    TREE.with(|t| {
        let mut t = t.borrow_mut();
        match t.get(&p) {
            Some(Node::File(_)) if new => Err(io::Error::new(io::ErrorKind::AlreadyExists, "the file exists")),
            Some(Node::File(d)) => {
                if truncate {
                    let mut d = d.borrow_mut();
                    d.bytes.clear();
                    d.modified = SystemTime::now();
                }
                Ok(d.clone())
            }
            Some(Node::Dir(_)) => Err(io::Error::new(io::ErrorKind::IsADirectory, "it is a folder")),
            None if create => {
                make_parents(&mut t, &p)?;
                let d = Rc::new(RefCell::new(Data { bytes: Vec::new(), modified: SystemTime::now() }));
                t.insert(p, Node::File(d.clone()));
                Ok(d)
            }
            None => Err(not_found()),
        }
    })
}

pub fn exists(p: impl AsRef<Path>) -> bool {
    get(p.as_ref()).is_some()
}

pub fn is_file(p: impl AsRef<Path>) -> bool {
    matches!(get(p.as_ref()), Some(Node::File(_)))
}

pub fn is_dir(p: impl AsRef<Path>) -> bool {
    matches!(get(p.as_ref()), Some(Node::Dir(_)))
}

pub fn read(p: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    match get(p.as_ref()) {
        Some(Node::File(d)) => Ok(d.borrow().bytes.clone()),
        Some(Node::Dir(_)) => Err(io::Error::new(io::ErrorKind::IsADirectory, "it is a folder")),
        None => Err(not_found()),
    }
}

pub fn read_to_string(p: impl AsRef<Path>) -> io::Result<String> {
    String::from_utf8(read(p)?).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not UTF-8"))
}

pub fn write(p: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
    let d = file_data(p.as_ref(), true, true, false)?;
    let mut d = d.borrow_mut();
    d.bytes = contents.as_ref().to_vec();
    d.modified = SystemTime::now();
    Ok(())
}

pub fn create_dir(p: impl AsRef<Path>) -> io::Result<()> {
    let p = norm(p.as_ref());
    if get(&p).is_some() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "it exists"));
    }
    if !p.parent().is_none_or(is_dir) {
        return Err(not_found());
    }
    TREE.with(|t| t.borrow_mut().insert(p, Node::Dir(SystemTime::now())));
    Ok(())
}

pub fn create_dir_all(p: impl AsRef<Path>) -> io::Result<()> {
    let p = norm(p.as_ref());
    match get(&p) {
        Some(Node::Dir(_)) => Ok(()),
        Some(Node::File(_)) => Err(io::Error::new(io::ErrorKind::AlreadyExists, "a file is there")),
        None => TREE.with(|t| {
            let mut t = t.borrow_mut();
            make_parents(&mut t, &p)?;
            t.insert(p, Node::Dir(SystemTime::now()));
            Ok(())
        }),
    }
}

pub fn remove_file(p: impl AsRef<Path>) -> io::Result<()> {
    let p = norm(p.as_ref());
    TREE.with(|t| {
        let mut t = t.borrow_mut();
        match t.get(&p) {
            Some(Node::File(_)) => {
                t.remove(&p);
                Ok(())
            }
            Some(Node::Dir(_)) => Err(io::Error::new(io::ErrorKind::IsADirectory, "it is a folder")),
            None => Err(not_found()),
        }
    })
}

pub fn remove_dir(p: impl AsRef<Path>) -> io::Result<()> {
    let p = norm(p.as_ref());
    TREE.with(|t| {
        let mut t = t.borrow_mut();
        if !matches!(t.get(&p), Some(Node::Dir(_))) {
            return Err(not_found());
        }
        if !under(&t, &p).is_empty() {
            return Err(io::Error::new(io::ErrorKind::DirectoryNotEmpty, "the folder isn't empty"));
        }
        t.remove(&p);
        Ok(())
    })
}

pub fn remove_dir_all(p: impl AsRef<Path>) -> io::Result<()> {
    let p = norm(p.as_ref());
    TREE.with(|t| {
        let mut t = t.borrow_mut();
        if !matches!(t.get(&p), Some(Node::Dir(_))) {
            return Err(not_found());
        }
        for k in under(&t, &p) {
            t.remove(&k);
        }
        t.remove(&p);
        Ok(())
    })
}

pub fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    let (from, to) = (norm(from.as_ref()), norm(to.as_ref()));
    if from == to {
        return Ok(());
    }
    TREE.with(|t| {
        let mut t = t.borrow_mut();
        let node = t.get(&from).cloned().ok_or_else(not_found)?;
        if matches!(node, Node::Dir(_)) && to.starts_with(&from) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "into itself"));
        }
        if let Some(Node::Dir(_)) = t.get(&to) {
            if !under(&t, &to).is_empty() || matches!(node, Node::File(_)) {
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, "a folder is there"));
            }
        }
        make_parents(&mut t, &to)?;
        let moved = under(&t, &from);
        t.remove(&from);
        t.insert(to.clone(), node);
        for k in moved {
            if let Some(n) = t.remove(&k) {
                let rest = k.strip_prefix(&from).unwrap_or(&k).to_path_buf();
                t.insert(to.join(rest), n);
            }
        }
        Ok(())
    })
}

pub fn copy(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<u64> {
    let bytes = read(from)?;
    let n = bytes.len() as u64;
    write(to, bytes)?;
    Ok(n)
}

pub fn canonicalize(p: impl AsRef<Path>) -> io::Result<PathBuf> {
    let p = norm(p.as_ref());
    get(&p).map(|_| p).ok_or_else(not_found)
}

/// There are no links here.
pub fn read_link(_p: impl AsRef<Path>) -> io::Result<PathBuf> {
    Err(io::Error::new(io::ErrorKind::InvalidInput, "not a link"))
}

pub fn metadata(p: impl AsRef<Path>) -> io::Result<Metadata> {
    get(p.as_ref()).map(|n| Metadata::of(&n)).ok_or_else(not_found)
}

pub fn symlink_metadata(p: impl AsRef<Path>) -> io::Result<Metadata> {
    metadata(p)
}

pub fn set_permissions(p: impl AsRef<Path>, _perm: Permissions) -> io::Result<()> {
    metadata(p).map(|_| ())
}

pub fn read_dir(p: impl AsRef<Path>) -> io::Result<ReadDir> {
    let p = norm(p.as_ref());
    if !is_dir(&p) {
        return Err(not_found());
    }
    let items = TREE.with(|t| t.borrow().iter().filter(|(k, _)| k.parent() == Some(p.as_path())).map(|(k, n)| Ok(DirEntry { path: k.clone(), meta: Metadata::of(n) })).collect::<Vec<_>>());
    Ok(ReadDir(items.into_iter()))
}

pub struct ReadDir(std::vec::IntoIter<io::Result<DirEntry>>);

impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}

pub struct DirEntry {
    path: PathBuf,
    meta: Metadata,
}

impl DirEntry {
    pub fn path(&self) -> PathBuf {
        self.path.clone()
    }
    pub fn file_name(&self) -> OsString {
        self.path.file_name().map(|n| n.to_os_string()).unwrap_or_default()
    }
    pub fn metadata(&self) -> io::Result<Metadata> {
        Ok(self.meta.clone())
    }
    pub fn file_type(&self) -> io::Result<FileType> {
        Ok(self.meta.file_type())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileType {
    dir: bool,
}

impl FileType {
    pub fn is_dir(&self) -> bool {
        self.dir
    }
    pub fn is_file(&self) -> bool {
        !self.dir
    }
    pub fn is_symlink(&self) -> bool {
        false
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Permissions {
    readonly: bool,
}

impl Permissions {
    pub fn readonly(&self) -> bool {
        self.readonly
    }
    pub fn set_readonly(&mut self, readonly: bool) {
        self.readonly = readonly;
    }
}

#[derive(Clone, Debug)]
pub struct Metadata {
    len: u64,
    dir: bool,
    modified: SystemTime,
}

impl Metadata {
    fn of(n: &Node) -> Metadata {
        match n {
            Node::Dir(t) => Metadata { len: 0, dir: true, modified: *t },
            Node::File(d) => {
                let d = d.borrow();
                Metadata { len: d.bytes.len() as u64, dir: false, modified: d.modified }
            }
        }
    }
    pub fn len(&self) -> u64 {
        self.len
    }
    pub fn is_dir(&self) -> bool {
        self.dir
    }
    pub fn is_file(&self) -> bool {
        !self.dir
    }
    pub fn is_symlink(&self) -> bool {
        false
    }
    pub fn file_type(&self) -> FileType {
        FileType { dir: self.dir }
    }
    pub fn modified(&self) -> io::Result<SystemTime> {
        Ok(self.modified)
    }
    pub fn created(&self) -> io::Result<SystemTime> {
        Ok(self.modified)
    }
    pub fn accessed(&self) -> io::Result<SystemTime> {
        Ok(self.modified)
    }
    pub fn permissions(&self) -> Permissions {
        Permissions { readonly: false }
    }
}

/// An open file: reads and writes go to the file's bytes at once.
#[derive(Debug)]
pub struct File {
    data: Rc<RefCell<Data>>,
    pos: u64,
    append: bool,
}

impl File {
    pub fn open(p: impl AsRef<Path>) -> io::Result<File> {
        OpenOptions::new().read(true).open(p)
    }
    pub fn create(p: impl AsRef<Path>) -> io::Result<File> {
        OpenOptions::new().write(true).create(true).truncate(true).open(p)
    }
    pub fn create_new(p: impl AsRef<Path>) -> io::Result<File> {
        OpenOptions::new().write(true).create_new(true).open(p)
    }
    pub fn options() -> OpenOptions {
        OpenOptions::new()
    }
    pub fn metadata(&self) -> io::Result<Metadata> {
        Ok(Metadata::of(&Node::File(self.data.clone())))
    }
    pub fn set_permissions(&self, _perm: Permissions) -> io::Result<()> {
        Ok(())
    }
    pub fn set_len(&self, n: u64) -> io::Result<()> {
        self.data.borrow_mut().bytes.resize(n as usize, 0);
        Ok(())
    }
    pub fn sync_all(&self) -> io::Result<()> {
        Ok(())
    }
    pub fn sync_data(&self) -> io::Result<()> {
        Ok(())
    }
    pub fn try_clone(&self) -> io::Result<File> {
        Ok(File { data: self.data.clone(), pos: self.pos, append: self.append })
    }
}

impl Read for File {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let d = self.data.borrow();
        let at = (self.pos as usize).min(d.bytes.len());
        let n = buf.len().min(d.bytes.len() - at);
        buf[..n].copy_from_slice(&d.bytes[at..at + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Write for File {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut d = self.data.borrow_mut();
        if self.append {
            self.pos = d.bytes.len() as u64;
        }
        let at = self.pos as usize;
        if d.bytes.len() < at + buf.len() {
            d.bytes.resize(at + buf.len(), 0);
        }
        d.bytes[at..at + buf.len()].copy_from_slice(buf);
        d.modified = SystemTime::now();
        self.pos += buf.len() as u64;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for File {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let len = self.data.borrow().bytes.len() as i64;
        let p = match to {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::End(d) => len + d,
            SeekFrom::Current(d) => self.pos as i64 + d,
        };
        if p < 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "before the start"));
        }
        self.pos = p as u64;
        Ok(self.pos)
    }
}

#[derive(Clone, Debug, Default)]
pub struct OpenOptions {
    write: bool,
    append: bool,
    truncate: bool,
    create: bool,
    create_new: bool,
}

impl OpenOptions {
    pub fn new() -> OpenOptions {
        OpenOptions::default()
    }
    pub fn read(&mut self, _on: bool) -> &mut OpenOptions {
        self
    }
    pub fn write(&mut self, on: bool) -> &mut OpenOptions {
        self.write = on;
        self
    }
    pub fn append(&mut self, on: bool) -> &mut OpenOptions {
        self.append = on;
        self
    }
    pub fn truncate(&mut self, on: bool) -> &mut OpenOptions {
        self.truncate = on;
        self
    }
    pub fn create(&mut self, on: bool) -> &mut OpenOptions {
        self.create = on;
        self
    }
    pub fn create_new(&mut self, on: bool) -> &mut OpenOptions {
        self.create_new = on;
        self
    }
    pub fn open(&self, p: impl AsRef<Path>) -> io::Result<File> {
        let writes = self.write || self.append;
        let data = file_data(p.as_ref(), writes && (self.create || self.create_new), writes && self.truncate, self.create_new)?;
        Ok(File { data, pos: 0, append: self.append })
    }
}

#[derive(Debug, Default)]
pub struct DirBuilder {
    recursive: bool,
}

impl DirBuilder {
    pub fn new() -> DirBuilder {
        DirBuilder::default()
    }
    pub fn recursive(&mut self, on: bool) -> &mut DirBuilder {
        self.recursive = on;
        self
    }
    pub fn create(&self, p: impl AsRef<Path>) -> io::Result<()> {
        if self.recursive { create_dir_all(p) } else { create_dir(p) }
    }
}
