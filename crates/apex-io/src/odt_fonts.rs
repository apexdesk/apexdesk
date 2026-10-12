//! Embedded fonts in OpenDocument files, as LibreOffice writes them: a
//! font declaration (`style:font-face`) points at a font file of the
//! package (`svg:font-face-uri`, `Fonts/...`), with its style and weight;
//! settings say whether saving embeds fonts (`EmbedFonts`).

use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{EmbeddedFont, EmbeddedFonts};

use crate::xml::{Ev, Reader, esc};
use crate::zip::{ZipReader, ZipWriter};

pub(crate) const SETTING: &str = "EmbedFonts";

/// Whether settings.xml turns on embedding.
pub(crate) fn embed_setting(settings: &str) -> bool {
    let mut r = Reader::new(settings);
    loop {
        match r.next() {
            Ev::Eof => return false,
            Ev::Start(t) if t.name == "config:config-item" && t.attr("config:name").as_deref() == Some(SETTING) => return r.text().trim() == "true",
            _ => {}
        }
    }
}

/// Reads the fonts declared in `xml` (styles.xml or content.xml) with a
/// file of the package, skipping faces already in `out`.
pub(crate) fn read(z: &ZipReader, xml: &str, out: &mut EmbeddedFonts) {
    let mut r = Reader::new(xml);
    let mut family: Option<String> = None;
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) if t.name == "style:font-face" => {
                family = t.attr("svg:font-family").map(|f| f.trim_matches(|c| c == '\'' || c == '"').to_string()).or_else(|| t.attr("style:name").map(|n| n.into_owned()));
            }
            Ev::End("style:font-face") => family = None,
            Ev::Start(t) | Ev::Empty(t) if t.name == "svg:font-face-uri" => {
                let (Some(fam), Some(href)) = (&family, t.attr("xlink:href")) else { continue };
                if href.contains(':') {
                    continue;
                }
                let bold = t.attr("loext:font-weight").is_some_and(|w| w == "bold" || w.parse::<u16>().is_ok_and(|n| n >= 600));
                let italic = t.attr("loext:font-style").is_some_and(|s| s == "italic" || s == "oblique");
                if out.faces.iter().any(|f| f.family.eq_ignore_ascii_case(fam) && f.bold == bold && f.italic == italic) {
                    continue;
                }
                if let Ok(data) = z.read(href.trim_start_matches("./")) {
                    out.faces.push(EmbeddedFont { family: fam.as_str().into(), bold, italic, data: Arc::from(data) });
                }
            }
            Ev::Start(t) if matches!(t.name, "office:styles" | "office:automatic-styles" | "office:body" | "office:master-styles") => break,
            _ => {}
        }
    }
}

/// A file name for a face: the family with safe characters and its style.
fn file_name(f: &EmbeddedFont) -> String {
    let fam: String = f.family.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    let style = match (f.bold, f.italic) {
        (false, false) => "Regular",
        (true, false) => "Bold",
        (false, true) => "Italic",
        (true, true) => "BoldItalic",
    };
    format!("Fonts/{fam}-{style}.ttf")
}

/// Writes the font files; returns the font declarations for styles.xml
/// and the manifest entries.
pub(crate) fn write(z: &mut ZipWriter, fonts: &EmbeddedFonts) -> (String, String) {
    let mut decls = String::new();
    let mut manifest = String::new();
    for fam in fonts.families() {
        let _ = write!(decls, r#"<style:font-face style:name="{0}" svg:font-family="{0}"><svg:font-face-src>"#, esc(&fam));
        for f in fonts.faces.iter().filter(|f| f.family.eq_ignore_ascii_case(&fam)) {
            let path = file_name(f);
            z.add(&path, &f.data, true);
            let _ = write!(manifest, r#"<manifest:file-entry manifest:full-path="{}" manifest:media-type="application/x-font-ttf"/>"#, esc(&path));
            let _ = write!(
                decls,
                r#"<svg:font-face-uri xlink:href="{}" xlink:type="simple" loext:font-style="{}" loext:font-weight="{}"><svg:font-face-format svg:string="truetype"/></svg:font-face-uri>"#,
                esc(&path),
                if f.italic { "italic" } else { "normal" },
                if f.bold { "bold" } else { "normal" }
            );
        }
        decls.push_str("</svg:font-face-src></style:font-face>");
    }
    if !decls.is_empty() {
        decls = format!("<office:font-face-decls>{decls}</office:font-face-decls>");
    }
    (decls, manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut doc = apex_doc::Document::from_paragraphs(vec![apex_doc::Paragraph::new("x")]);
        let data: Arc<[u8]> = Arc::from((0..100u8).collect::<Vec<_>>());
        let face = EmbeddedFont { family: "Gelasio".into(), bold: false, italic: false, data };
        doc.fonts = EmbeddedFonts { embed: true, faces: vec![face.clone(), EmbeddedFont { bold: true, italic: true, ..face }] };
        let bytes = crate::odt::write(&doc);
        let z = ZipReader::new(&bytes).unwrap();
        assert!(z.read_string("META-INF/manifest.xml").unwrap().contains("Fonts/Gelasio-BoldItalic.ttf"));
        let back = crate::odt::read(&bytes).unwrap();
        assert_eq!(back.fonts, doc.fonts);
    }
}
