//! Fonts embedded in documents. When a document is opened, its fonts
//! that are not installed become families like the installed ones (for
//! the screen and for PDF), held in memory. When saving with "Embed fonts"
//! on, the installed files of the families in use are put in the document.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use apex_doc::{Document, EmbeddedFont, EmbeddedFonts};

use super::{Db, Family, GENERATION, db, slot};

/// Fonts bigger than this are not embedded (large CJK fonts).
const MAX_SIZE: u64 = 24 << 20;

/// Makes the fonts of a document usable where the family is not installed;
/// gives the faces it holds, for [`release`] when the document closes.
/// A family the document uses without embedding it is not drawn with
/// another document's embedded faces.
pub fn register(fonts: &EmbeddedFonts, used: &[Arc<str>]) -> Vec<PathBuf> {
    let Ok(mut d) = db().lock() else { return Vec::new() };
    let mut changed = false;
    for name in used {
        let key = name.to_lowercase();
        if d.embedded.contains_key(&key) && !fonts.faces.iter().any(|f| f.family.to_lowercase() == key) {
            drop_family(&mut d, &key);
            changed = true;
        }
    }
    if fonts.faces.is_empty() {
        if changed {
            GENERATION.fetch_add(1, Ordering::Relaxed);
        }
        return Vec::new();
    }
    // A family another document embedded differently (or only some of
    // its faces) is dropped for both: the embedded faces are shared by
    // name, and one document mustn't draw another's text with its glyphs.
    let path_of = |f: &EmbeddedFont| PathBuf::from(format!("embedded:{}:{}", f.family.to_lowercase(), slot(f.bold, f.italic)));
    for f in &fonts.faces {
        let key = f.family.to_lowercase();
        if d.embedded.contains_key(&key) && d.mem.get(&path_of(f)).is_none_or(|m| **m != *f.data) {
            drop_family(&mut d, &key);
        }
    }
    let mut held = Vec::new();
    for f in &fonts.faces {
        let key = f.family.to_lowercase();
        let path = path_of(f);
        if d.embedded_clash.contains(&key) || held.contains(&path) {
            continue;
        }
        if d.mem.contains_key(&path) {
            *d.embedded_users.entry(path.clone()).or_default() += 1;
            held.push(path);
            continue;
        }
        // Bytes that aren't a font: the text library stops the program on
        // them. Nor outlines that would take for ever to draw.
        if skrifa::FontRef::from_index(&f.data, 0).is_err() || !apex_io::font_check::outlines_bounded(&f.data) {
            continue;
        }
        *d.embedded_users.entry(path.clone()).or_default() += 1;
        held.push(path.clone());
        let fam = d.embedded.entry(key).or_insert_with(|| Family { name: f.family.to_string(), ..Default::default() });
        fam.faces[slot(f.bold, f.italic)] = Some((path.clone(), 0, if f.bold { 700 } else { 400 }, f.data.len() as u64));
        d.mem.insert(path, f.data.clone());
    }
    merge(&mut d);
    GENERATION.fetch_add(1, Ordering::Relaxed);
    held
}

/// A document that held `faces` closed: the faces no open document holds
/// any more are dropped.
pub fn release(faces: &[PathBuf]) {
    if faces.is_empty() {
        return;
    }
    let Ok(mut d) = db().lock() else { return };
    for path in faces {
        let Some(n) = d.embedded_users.get_mut(path) else { continue };
        *n -= 1;
        if *n > 0 {
            continue;
        }
        d.embedded_users.remove(path);
        d.mem.remove(path);
        let keys: Vec<String> = d.embedded.iter().filter(|(_, f)| f.faces.iter().flatten().any(|x| &x.0 == path)).map(|(k, _)| k.clone()).collect();
        for key in keys {
            drop_family(&mut d, &key);
            // Not a clash: the next document may embed it again.
            d.embedded_clash.remove(&key);
        }
    }
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// Stops using family `key`'s embedded faces (for every document).
fn drop_family(d: &mut Db, key: &str) {
    d.embedded.remove(key);
    if d.families.get(key).is_some_and(|fam| fam.faces.iter().flatten().all(|x| x.0.to_string_lossy().starts_with("embedded:"))) {
        d.families.remove(key);
    }
    d.embedded_clash.insert(key.to_string());
}

/// Adds the embedded families that are not installed to the usable ones.
pub(super) fn merge(d: &mut Db) {
    let add: Vec<(String, Family)> = d
        .embedded
        .iter()
        // Nor in place of a metric-compatible one that is: a document's
        // "Calibri" would otherwise draw every later document's text.
        .filter(|(k, _)| !d.families.contains_key(*k) && super::resolve(d, k).is_none())
        .map(|(k, f)| (k.clone(), Family { name: f.name.clone(), faces: f.faces.clone() }))
        .collect();
    for (k, f) in add {
        d.families.insert(k, f);
    }
}

/// Whether a font's license allows embedding it (OS/2 `fsType` is not
/// "restricted license").
pub(crate) fn embeddable(font: &[u8]) -> bool {
    if font.get(..4) == Some(b"ttcf") {
        return false;
    }
    let be16 = |at: usize| font.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    let be32 = |at: usize| font.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    let n = be16(4).unwrap_or(0) as usize;
    for i in 0..n {
        let r = 12 + i * 16;
        if font.get(r..r + 4) == Some(b"OS/2") {
            let Some(off) = be32(r + 8) else { return false };
            return be16(off as usize + 8).is_some_and(|fs| fs & 0x000F != 0x0002);
        }
    }
    true
}

/// The fonts to save in `doc`: with embedding on, the faces of each family
/// in use (installed ones read from disk, others kept from the file);
/// with it off, none.
pub fn for_saving(doc: &Document) -> EmbeddedFonts {
    if !doc.fonts.embed {
        return EmbeddedFonts::default();
    }
    let mut faces = Vec::new();
    let used = doc.fonts_used();
    let Ok(d) = db().lock() else { return doc.fonts.clone() };
    for name in used {
        let key = name.to_lowercase();
        let mut found = false;
        if let Some(fam) = d.families.get(&key) {
            for (s, face) in fam.faces.iter().enumerate() {
                let Some((path, index, _, size)) = face else { continue };
                if *index != 0 || *size > MAX_SIZE {
                    continue;
                }
                let data: Option<Arc<[u8]>> = match d.mem.get(path) {
                    // Another document's font is never put in this one.
                    Some(b) => doc.fonts.faces.iter().find(|f| f.data == *b).map(|f| f.data.clone()),
                    None => std::fs::read(path).ok().filter(|b| embeddable(b)).map(Arc::from),
                };
                if let Some(data) = data {
                    faces.push(EmbeddedFont { family: fam.name.as_str().into(), bold: s & 1 == 1, italic: s & 2 == 2, data });
                    found = true;
                }
            }
        }
        if !found {
            // Not installed here (or not scanned yet): what the file had.
            faces.extend(doc.fonts.faces.iter().filter(|f| f.family.eq_ignore_ascii_case(&name)).cloned());
        }
    }
    EmbeddedFonts { embed: true, faces }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn license_and_register() {
        // A table directory with an OS/2 table whose fsType is 2 (restricted).
        let mut f = vec![0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        f.extend_from_slice(b"OS/2");
        f.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 28, 0, 0, 0, 10]);
        f.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 2]);
        assert!(!embeddable(&f));
        f[37] = 8;
        assert!(embeddable(&f));
        assert!(!embeddable(b"ttcf...."));

        // Only a font's table directory is read when it's registered.
        let data: Arc<[u8]> = Arc::from(&f[..]);
        let fonts = EmbeddedFonts { embed: true, faces: vec![EmbeddedFont { family: "Apex Test Embedded".into(), bold: false, italic: false, data }] };
        let held = register(&fonts, &[]);
        let path = {
            let d = db().lock().unwrap();
            let fam = &d.families["apex test embedded"];
            let path = fam.faces[0].as_ref().unwrap().0.clone();
            assert!(d.mem.contains_key(&path));
            path
        };
        // Held twice, then released by each document.
        let again = register(&fonts, &[]);
        release(&held);
        assert!(db().lock().unwrap().mem.contains_key(&path));
        release(&again);
        let d = db().lock().unwrap();
        assert!(!d.mem.contains_key(&path) && !d.families.contains_key("apex test embedded"));
    }

    #[test]
    fn another_documents_font_is_not_used() {
        let mut f = vec![0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        f.resize(64, 0);
        let data: Arc<[u8]> = Arc::from(&f[..]);
        let fonts = EmbeddedFonts { embed: true, faces: vec![EmbeddedFont { family: "Apex Test Borrowed".into(), bold: false, italic: false, data }] };
        let held = register(&fonts, &[]);
        assert!(db().lock().unwrap().families.contains_key("apex test borrowed"));
        // A document that uses the family without embedding it.
        register(&EmbeddedFonts::default(), &[Arc::from("Apex Test Borrowed")]);
        assert!(!db().lock().unwrap().families.contains_key("apex test borrowed"));
        release(&held);
    }
}
