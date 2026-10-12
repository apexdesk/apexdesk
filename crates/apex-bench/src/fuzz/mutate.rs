//! The seeds and how a seed is turned into a new test case, depending on
//! its container: zip package, OLE compound file or text.

use std::path::{Path, PathBuf};

use super::rng::Rng;
use super::{bytes, ole, zip};

pub struct Seed {
    pub path: PathBuf,
    pub data: Vec<u8>,
    /// The reader it exercises: xlsx, docx, ods, odt, xlsb, xls, doc, csv,
    /// rtf, md, txt, vba or crypto (any encrypted file).
    pub reader: String,
    /// Extension given to the test cases (the child picks the reader by
    /// it, like the window).
    pub ext: String,
}

/// The reader of a file by its extension and content; None for files
/// the fuzzer doesn't use.
pub fn reader_of(path: &Path, data: &[u8]) -> Option<(String, String)> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let reader = match ext.as_str() {
        "bin" if data.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) => "vba",
        "bin" => return None,
        _ => {
            let fmt = apex_io::Format::from_path(path)?;
            if fmt == apex_io::Format::Tsv { "csv" } else { fmt.extension() }
        }
    };
    let encrypted = std::panic::catch_unwind(|| apex_io::is_encrypted(data)).unwrap_or(false);
    let reader = if encrypted { "crypto" } else { reader };
    Some((reader.to_string(), ext))
}

pub fn load(path: &Path) -> Option<Seed> {
    let data = std::fs::read(path).ok()?;
    let (reader, ext) = reader_of(path, &data)?;
    Some(Seed { path: path.to_path_buf(), data, reader, ext })
}

/// A new test case from `seed`; `pool` holds seeds of the same reader.
pub fn mutate(rng: &mut Rng, seed: &Seed, pool: &[&Seed]) -> Vec<u8> {
    let other = if pool.is_empty() { None } else { Some(pool[rng.below(pool.len())]) };
    let d = &seed.data;
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if d.starts_with(b"PK") {
            let mut rng2 = Rng::new(rng.next());
            // Parts of another file, to take some into this one.
            let other = other.filter(|_| rng2.one_in(4)).and_then(|o| zip::parts(&o.data));
            zip::mutate(&mut rng2, d, other.as_deref())
        } else if d.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
            let mut rng2 = Rng::new(rng.next());
            ole::mutate(&mut rng2, d)
        } else {
            let mut out = d.clone();
            let mut rng2 = Rng::new(rng.next());
            for _ in 0..rng2.small_len(4) {
                if rng2.one_in(12) {
                    // A piece of another file of the same kind.
                    if let Some(o) = other.filter(|o| !o.data.is_empty()) {
                        let a = rng2.below(o.data.len());
                        let n = rng2.small_len(o.data.len() - a);
                        let at = rng2.below(out.len() + 1);
                        out.splice(at..at, o.data[a..a + n].iter().copied());
                        continue;
                    }
                }
                bytes::text(&mut rng2, &mut out, &seed.ext);
            }
            out
        }
    }));
    caught.unwrap_or_else(|_| {
        let mut out = d.clone();
        bytes::binary(rng, &mut out);
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readers_by_extension_and_content() {
        let r = |p: &str, d: &[u8]| reader_of(Path::new(p), d).map(|x| x.0);
        assert_eq!(r("a.XLSX", b"PK").as_deref(), Some("xlsx"));
        assert_eq!(r("a.tsv", b"x").as_deref(), Some("csv"));
        assert_eq!(r("vbaProject.bin", &[0xD0, 0xCF, 0x11, 0xE0]).as_deref(), Some("vba"));
        assert_eq!(r("x.bin", b"zz"), None);
        assert_eq!(r("x.png", b"zz"), None);
    }

    #[test]
    fn text_seeds_mutate() {
        let seed = Seed { path: "a.csv".into(), data: b"a,b\n1,2\n".to_vec(), reader: "csv".into(), ext: "csv".into() };
        let mut rng = Rng::new(1);
        assert!((0..50).any(|_| mutate(&mut rng, &seed, &[]) != seed.data));
    }
}
