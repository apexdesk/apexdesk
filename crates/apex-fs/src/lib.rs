//! Files for ApexDesk. On the desktop this is `std::fs` itself; in the
//! browser, where std has no file system, the same calls work on one kept
//! in memory (`mem`), which the web version fills with the files it opens
//! and reads the ones saved from.
//!
//! The rest of the program uses `apex_fs::` where it would use `std::fs::`,
//! and [`PathExt`] where it would ask a `Path` (which asks std's).
//!
//! The browser has no threads either: [`thread`] is `std::thread`, or
//! there work given to a thread done on the spot.

#[cfg(not(target_arch = "wasm32"))]
pub use std::fs::*;

/// Whether something is at `p` (`Path::exists`).
#[cfg(not(target_arch = "wasm32"))]
pub fn exists(p: impl AsRef<std::path::Path>) -> bool {
    p.as_ref().exists()
}

/// Whether `p` is a file (`Path::is_file`).
#[cfg(not(target_arch = "wasm32"))]
pub fn is_file(p: impl AsRef<std::path::Path>) -> bool {
    p.as_ref().is_file()
}

/// Whether `p` is a folder (`Path::is_dir`).
#[cfg(not(target_arch = "wasm32"))]
pub fn is_dir(p: impl AsRef<std::path::Path>) -> bool {
    p.as_ref().is_dir()
}

/// `Path`'s questions about the file system, asked of this one.
pub trait PathExt {
    /// `Path::exists`.
    fn fs_exists(&self) -> bool;
    /// `Path::is_file`.
    fn fs_is_file(&self) -> bool;
    /// `Path::is_dir`.
    fn fs_is_dir(&self) -> bool;
    /// `Path::metadata`.
    fn fs_metadata(&self) -> std::io::Result<Metadata>;
    /// `Path::read_dir`.
    fn fs_read_dir(&self) -> std::io::Result<ReadDir>;
    /// `Path::canonicalize`.
    fn fs_canonicalize(&self) -> std::io::Result<std::path::PathBuf>;
}

impl PathExt for std::path::Path {
    fn fs_exists(&self) -> bool {
        exists(self)
    }

    fn fs_is_file(&self) -> bool {
        is_file(self)
    }

    fn fs_is_dir(&self) -> bool {
        is_dir(self)
    }

    fn fs_metadata(&self) -> std::io::Result<Metadata> {
        metadata(self)
    }

    fn fs_read_dir(&self) -> std::io::Result<ReadDir> {
        read_dir(self)
    }

    fn fs_canonicalize(&self) -> std::io::Result<std::path::PathBuf> {
        canonicalize(self)
    }
}

/// The folder for temporary files (`std::env::temp_dir`).
pub fn temp_dir() -> std::path::PathBuf {
    #[cfg(not(target_arch = "wasm32"))]
    return std::env::temp_dir();
    #[cfg(target_arch = "wasm32")]
    return std::path::PathBuf::from("/tmp");
}

/// This process's id (`std::process::id`); the browser's page is one.
pub fn process_id() -> u32 {
    #[cfg(not(target_arch = "wasm32"))]
    return std::process::id();
    #[cfg(target_arch = "wasm32")]
    return 1;
}

#[cfg(not(target_arch = "wasm32"))]
pub use std::thread;
#[cfg(target_arch = "wasm32")]
pub mod thread;

#[cfg(target_arch = "wasm32")]
mod mem;
#[cfg(target_arch = "wasm32")]
pub use mem::*;

#[cfg(test)]
mod tests {
    use std::io::{Read, Seek, SeekFrom, Write};

    #[test]
    fn files_and_folders() {
        let dir = std::env::temp_dir().join(format!("apex-fs-{}", std::process::id()));
        let _ = super::remove_dir_all(&dir);
        super::create_dir_all(dir.join("a/b")).unwrap();
        super::write(dir.join("a/b/x.txt"), "hola").unwrap();
        assert_eq!(super::read_to_string(dir.join("a/b/x.txt")).unwrap(), "hola");
        assert!(super::exists(dir.join("a/b/x.txt")) && super::is_dir(dir.join("a")) && super::is_file(dir.join("a/b/x.txt")));
        let mut f = super::OpenOptions::new().append(true).open(dir.join("a/b/x.txt")).unwrap();
        f.write_all(b" mundo").unwrap();
        let mut f = super::File::open(dir.join("a/b/x.txt")).unwrap();
        f.seek(SeekFrom::Start(5)).unwrap();
        let mut s = String::new();
        f.read_to_string(&mut s).unwrap();
        assert_eq!(s, "mundo");
        super::rename(dir.join("a/b"), dir.join("c")).unwrap();
        let names: Vec<_> = super::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
        assert_eq!(names, ["a", "c"]);
        assert_eq!(super::metadata(dir.join("c/x.txt")).unwrap().len(), 10);
        super::remove_dir_all(&dir).unwrap();
        assert!(!super::exists(&dir));
    }
}
