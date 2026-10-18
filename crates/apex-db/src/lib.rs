//! Databases for ApexDesk, without any interface:
//!
//! - readers of other programs' files: Access (`.mdb` of Jet 3 and Jet 4,
//!   `.accdb` of ACE, see [`access`]) and dBase (`.dbf` with its memo file,
//!   see [`dbase`]), which give a [`model::Database`];
//! - the database tab's own files, SQLite (see [`sqlite`]): tables and their
//!   design, rows by pages, relations, and the forms, reports and queries
//!   saved inside ([`objects`]);
//! - building SQL for the visual query designer ([`query`]) and the reports'
//!   grouping ([`report`]).

pub mod access;
pub mod codepage;
pub mod dbase;
pub mod model;
pub mod objects;
pub mod query;
pub mod report;
pub mod forms;
#[cfg(feature = "sqlite")]
pub mod sqlite;
#[cfg(feature = "sqlite")]
pub mod import;

use std::fmt;
use std::path::Path;

pub use model::{ColType, Column, Database, Index, LeftOut, Relation, SavedQuery, Table, Value};

/// Why a database could not be read.
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    Io(String),
    /// Not a file of this kind, or damaged.
    Format(String),
    /// Encrypted with a password in a way this reader can't open.
    Encrypted,
    /// The database engine refused something (SQL errors and the like).
    Db(String),
    NoTable(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Format(e) => write!(f, "{e}"),
            Error::Encrypted => write!(f, "the database is encrypted with a password"),
            Error::Db(e) => write!(f, "{e}"),
            Error::NoTable(t) => write!(f, "no table named {t}"),
        }
    }
}

impl std::error::Error for Error {}

/// The kinds of files [`open`] reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Access,
    Dbase,
}

/// Extensions of the files [`open`] reads.
pub const ACCESS_EXTS: &[&str] = &["mdb", "accdb", "mde", "accde"];
pub const DBASE_EXTS: &[&str] = &["dbf"];

/// What a file is, by its extension.
pub fn kind_of(path: &Path) -> Option<FileKind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if ACCESS_EXTS.contains(&ext.as_str()) {
        Some(FileKind::Access)
    } else if DBASE_EXTS.contains(&ext.as_str()) {
        Some(FileKind::Dbase)
    } else {
        None
    }
}

/// Reads a whole Access or dBase file: its tables with their rows, and
/// what else it knows (relations, queries).
pub fn open(path: &Path) -> Result<Database, Error> {
    match kind_of(path) {
        Some(FileKind::Access) => access::AccessFile::open(path)?.read_all(),
        Some(FileKind::Dbase) => dbase::read_path(path),
        None => Err(Error::Format(format!("{}: not an Access or dBase file", path.display()))),
    }
}

/// The table names of an Access or dBase file (without reading rows).
pub fn table_names(path: &Path) -> Result<Vec<String>, Error> {
    match kind_of(path) {
        Some(FileKind::Access) => Ok(access::AccessFile::open(path)?.tables().into_iter().map(|t| t.name).collect()),
        Some(FileKind::Dbase) => Ok(vec![dbase::table_name(path)]),
        None => Err(Error::Format(format!("{}: not an Access or dBase file", path.display()))),
    }
}

/// One table of an Access or dBase file, with its rows.
pub fn read_table(path: &Path, name: &str) -> Result<Table, Error> {
    match kind_of(path) {
        Some(FileKind::Access) => {
            let f = access::AccessFile::open(path)?;
            let t = f.tables().into_iter().find(|t| t.name.eq_ignore_ascii_case(name)).ok_or_else(|| Error::NoTable(name.to_string()))?;
            f.read_table(&t)
        }
        Some(FileKind::Dbase) => Ok(dbase::read_path(path)?.tables.remove(0)),
        None => Err(Error::Format(format!("{}: not an Access or dBase file", path.display()))),
    }
}

/// A database file's bytes: a regular file of at most 4 GB (not a pipe
/// or a device named like one).
pub(crate) fn read_file(path: &Path) -> Result<Vec<u8>, Error> {
    let meta = apex_fs::metadata(path).map_err(|e| io_err(path, e))?;
    if !meta.is_file() || meta.len() > 4 << 30 {
        return Err(Error::Io(format!("{}: not a database file", path.display())));
    }
    apex_fs::read(path).map_err(|e| io_err(path, e))
}

pub(crate) fn io_err(path: &Path, e: std::io::Error) -> Error {
    Error::Io(format!("{}: {e}", path.display()))
}
