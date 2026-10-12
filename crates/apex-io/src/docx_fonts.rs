//! Embedded fonts in .docx files: `word/fontTable.xml` names each face
//! (`w:embedRegular`, `w:embedBold`...) with a relationship to an
//! obfuscated font (`word/fonts/fontN.odttf`) and its key; settings say
//! whether saving embeds fonts (`w:embedTrueTypeFonts`).
//!
//! Obfuscation (ISO/IEC 29500-1, 17.8.1): the first 32 bytes of the font
//! are XORed with the 16 bytes of the key GUID, taken in reverse order.

use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{EmbeddedFont, EmbeddedFonts};

use crate::xml::{Ev, HEADER, Reader, esc};
use crate::zip::{ZipReader, ZipWriter};

const SLOTS: [(&str, bool, bool); 4] = [("embedRegular", false, false), ("embedBold", true, false), ("embedItalic", false, true), ("embedBoldItalic", true, true)];
pub(crate) const REL: &str = r#"<Relationship Id="rIdFT" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/fontTable" Target="fontTable.xml"/>"#;
pub(crate) const CONTENT_TYPES: &str = r#"<Default Extension="odttf" ContentType="application/vnd.openxmlformats-officedocument.obfuscatedFont"/><Override PartName="/word/fontTable.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.fontTable+xml"/>"#;
const FONT_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/font";

/// The 16 key bytes of a GUID such as `{0A1B...}`, reversed.
fn key_bytes(guid: &str) -> Option<[u8; 16]> {
    let hex: Vec<u8> = guid.bytes().filter(u8::is_ascii_hexdigit).collect();
    if hex.len() != 32 {
        return None;
    }
    let mut k = [0u8; 16];
    for (i, pair) in hex.chunks(2).enumerate() {
        let v = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
        k[15 - i] = v;
    }
    Some(k)
}

/// Obfuscates or restores a font (the operation is its own inverse).
pub(crate) fn xor_font(data: &mut [u8], guid: &str) -> bool {
    let Some(k) = key_bytes(guid) else { return false };
    for (i, b) in data.iter_mut().take(32).enumerate() {
        *b ^= k[i % 16];
    }
    true
}

/// A fresh GUID, from the process's random hasher keys and the clock.
fn new_guid(n: usize) -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_usize(n);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    h.write_u128(now);
    let a = h.finish();
    h.write_u64(a);
    let b = h.finish();
    let x = (a as u128) << 64 | b as u128;
    let s = format!("{x:032X}");
    format!("{{{}-{}-{}-{}-{}}}", &s[..8], &s[8..12], &s[12..16], &s[16..20], &s[20..])
}

/// Reads the embedded fonts and the embed setting.
pub(crate) fn read(z: &ZipReader) -> EmbeddedFonts {
    let embed = z.read_string("word/settings.xml").is_ok_and(|s| crate::docx::on_flag(&s, "embedTrueTypeFonts"));
    let mut out = EmbeddedFonts { embed, faces: Vec::new() };
    let Ok(xml) = z.read_string("word/fontTable.xml") else { return out };
    let rels = z.read_string("word/_rels/fontTable.xml.rels").map(|s| crate::docx::read_rels(&s)).unwrap_or_default();
    let mut r = Reader::new(&xml);
    let mut family: Option<String> = None;
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) if t.local() == "font" => family = t.attr("w:name").map(|n| n.into_owned()),
            Ev::End(n) if crate::xml::local(n) == "font" => family = None,
            // Each face is checked and drawn from: a few families'
            // worth, however many the table names.
            _ if out.faces.len() >= 256 => break,
            Ev::Start(t) | Ev::Empty(t) => {
                let Some(&(_, bold, italic)) = SLOTS.iter().find(|s| s.0 == t.local()) else { continue };
                let (Some(fam), Some(id), Some(key)) = (&family, t.attr("r:id"), t.attr("w:fontKey")) else { continue };
                let Some(target) = rels.get(&*id) else { continue };
                let path = crate::xlsx::resolve_target("word", target);
                let Ok(mut data) = z.read(&path) else { continue };
                if xor_font(&mut data, &key) {
                    out.faces.push(EmbeddedFont { family: fam.as_str().into(), bold, italic, data: Arc::from(data) });
                }
            }
            _ => {}
        }
    }
    out
}

/// The settings element turning embedding on.
pub(crate) fn settings(f: &EmbeddedFonts) -> &'static str {
    if f.embed { "<w:embedTrueTypeFonts/>" } else { "" }
}

/// Writes the font table, its fonts and relationships; false when there
/// is nothing to write.
pub(crate) fn write(z: &mut ZipWriter, f: &EmbeddedFonts) -> bool {
    if f.faces.is_empty() {
        return false;
    }
    let mut table = format!(r#"{HEADER}<w:fonts xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">"#);
    let mut rels = format!(r#"{HEADER}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#);
    let mut n = 0;
    for fam in f.families() {
        let _ = write!(table, r#"<w:font w:name="{}">"#, esc(&fam));
        for (el, bold, italic) in SLOTS {
            let Some(face) = f.faces.iter().find(|x| x.family.eq_ignore_ascii_case(&fam) && x.bold == bold && x.italic == italic) else { continue };
            n += 1;
            let guid = new_guid(n);
            let mut data = face.data.to_vec();
            xor_font(&mut data, &guid);
            z.add(&format!("word/fonts/font{n}.odttf"), &data, true);
            let _ = write!(rels, r#"<Relationship Id="rId{n}" Type="{FONT_REL}" Target="fonts/font{n}.odttf"/>"#);
            let _ = write!(table, r#"<w:{el} r:id="rId{n}" w:fontKey="{guid}"/>"#);
        }
        table.push_str("</w:font>");
    }
    table.push_str("</w:fonts>");
    rels.push_str("</Relationships>");
    z.add("word/fontTable.xml", table.as_bytes(), true);
    z.add("word/_rels/fontTable.xml.rels", rels.as_bytes(), true);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_and_roundtrip() {
        let k = key_bytes("{00112233-4455-6677-8899-AABBCCDDEEFF}").unwrap();
        assert_eq!(k[0], 0xFF);
        assert_eq!(k[15], 0x00);
        let orig: Vec<u8> = (0..40).collect();
        let mut d = orig.clone();
        assert!(xor_font(&mut d, "{00112233-4455-6677-8899-AABBCCDDEEFF}"));
        assert_eq!(d[0], 0xFF);
        assert_eq!(d[16], 16 ^ 0xFF);
        assert_eq!(d[32..], orig[32..]);
        xor_font(&mut d, "{00112233-4455-6677-8899-AABBCCDDEEFF}");
        assert_eq!(d, orig);
        assert_ne!(new_guid(1), new_guid(2));

        let mut doc = apex_doc::Document::from_paragraphs(vec![apex_doc::Paragraph::new("x")]);
        let data: Arc<[u8]> = Arc::from((0..100u8).collect::<Vec<_>>());
        let face = EmbeddedFont { family: "Gelasio".into(), bold: false, italic: false, data };
        doc.fonts = EmbeddedFonts { embed: true, faces: vec![face.clone(), EmbeddedFont { italic: true, ..face }] };
        let bytes = crate::docx::write(&doc);
        let z = ZipReader::new(&bytes).unwrap();
        assert!(z.read("word/fonts/font1.odttf").unwrap() != doc.fonts.faces[0].data.to_vec());
        let back = crate::docx::read(&bytes).unwrap();
        assert_eq!(back.fonts, doc.fonts);
    }
}
