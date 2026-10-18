//! Embedded objects in odt and ods: sub-documents (`draw:object`, a
//! folder of the package) and OLE objects (`draw:object-ole`, a compound
//! file), each with its replacement picture in `ObjectReplacements/`.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{EmbedKind, Embedded, OleObject};

use crate::drawingml::read_fragment;
use crate::xml::esc;
use crate::zip::{ZipReader, ZipWriter};

/// Prefix of the objects' keys among a document's files (see [`load`]).
pub(crate) const KEY: &str = "\u{1}object:";

const SUBDOCS: [&str; 5] = ["spreadsheet", "text", "presentation", "graphics", "chart"];

/// Manifest entries: path and media type.
fn manifest(z: &ZipReader) -> Vec<(String, String)> {
    let Ok(xml) = z.read_string("META-INF/manifest.xml") else { return Vec::new() };
    let mut out = Vec::new();
    let mut entries = Vec::new();
    let root = read_fragment(&xml);
    root.find_all("file-entry", &mut entries);
    for e in entries {
        if let Some(p) = e.attr("manifest:full-path") {
            out.push((p.to_string(), e.attr("manifest:media-type").unwrap_or_default().to_string()));
        }
    }
    out
}

/// The embedded objects of an ODF package (charts too, for what the
/// chart reader doesn't take), as files to open: a package for each
/// sub-document, the compound file of each OLE object. Keyed by [`KEY`] and their name ("Object 1").
pub(crate) fn load(z: &ZipReader, out: &mut HashMap<String, Arc<[u8]>>) {
    let entries = manifest(z);
    for (path, mime) in &entries {
        if let Some(dir) = path.strip_suffix('/').filter(|d| !d.is_empty() && !d.contains('/')) {
            let Some(kind) = mime.strip_prefix("application/vnd.oasis.opendocument.") else { continue };
            // Each once, and no more than a document has: each is a
            // package made again from the whole manifest.
            if !SUBDOCS.contains(&kind) || out.contains_key(&format!("{KEY}{dir}")) || out.len() >= 1000 {
                continue;
            }
            let prefix = format!("{dir}/");
            let mut w = ZipWriter::new();
            w.add("mimetype", mime.as_bytes(), false);
            let mut man = format!(r#"<?xml version="1.0" encoding="UTF-8"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3"><manifest:file-entry manifest:full-path="/" manifest:version="1.3" manifest:media-type="{}"/>"#, esc(mime));
            for (p, m) in &entries {
                if let Some(rest) = p.strip_prefix(&prefix).filter(|r| !r.is_empty()) {
                    let _ = write!(man, r#"<manifest:file-entry manifest:full-path="{}" manifest:media-type="{}"/>"#, esc(rest), esc(m));
                }
            }
            man.push_str("</manifest:manifest>");
            w.add("META-INF/manifest.xml", man.as_bytes(), true);
            let names: Vec<String> = z.names().filter(|n| n.starts_with(&prefix) && n.len() > prefix.len() && !n.ends_with('/')).map(str::to_string).collect();
            for n in names {
                if let Ok(d) = z.read(&n) {
                    w.add(&n[prefix.len()..], &d, true);
                }
            }
            out.insert(format!("{KEY}{dir}"), Arc::from(w.finish()));
        } else if mime == "application/vnd.sun.star.oleobject" && !path.contains('/') {
            if let Ok(d) = z.read(path) {
                out.insert(format!("{KEY}{path}"), Arc::from(d));
            }
        }
    }
    let replacements: Vec<String> = z.names().filter(|n| n.starts_with("ObjectReplacements/")).map(str::to_string).collect();
    for n in replacements {
        if let Ok(d) = z.read(&n) {
            out.insert(n, Arc::from(d));
        }
    }
}

/// The object an `xlink:href` of `draw:object` or `draw:object-ole`
/// names, from the files [`load`] found.
pub(crate) fn object(files: &HashMap<String, Arc<[u8]>>, href: &str) -> Option<Embedded> {
    let name = href.trim_start_matches("./").trim_end_matches('/');
    let data = files.get(&format!("{KEY}{name}"))?.clone();
    Some(Embedded::new(EmbedKind::Ole(OleObject { prog_id: prog_id(&data), data, ..Default::default() })))
}

/// The object of a text document's `draw:object`: a chart becomes a
/// chart object with its local table's data; anything else as [`object`].
pub(crate) fn doc_object(files: &HashMap<String, Arc<[u8]>>, href: &str) -> Option<Embedded> {
    let name = href.trim_start_matches("./").trim_end_matches('/');
    let data = files.get(&format!("{KEY}{name}"))?;
    // (One without series stays as it came.)
    if let Some(chart) = crate::ods::chart::read_package(data).filter(|c| !c.series.is_empty()) {
        let book = crate::chart_object::book(&crate::chart_object::from_caches(chart));
        return Some(Embedded::new(EmbedKind::Chart(apex_doc::ChartObject { book })));
    }
    object(files, href)
}

/// Writes chart object `c` as object number `n` of a text document (its
/// data in its local table); returns its `draw:object`.
pub(crate) fn write_chart(c: &apex_doc::ChartObject, n: usize, files: &mut Vec<(String, Option<Arc<[u8]>>, String)>) -> Option<String> {
    let (wb, si, ci) = crate::chart_object::open(&c.book)?;
    let (content, styles) = crate::ods::chart::object_in_document(&wb, si, &wb.sheets[si].charts[ci]);
    let name = format!("Object {n}");
    files.push((format!("{name}/"), None, "application/vnd.oasis.opendocument.chart".into()));
    files.push((format!("{name}/content.xml"), Some(Arc::from(content.into_bytes())), "text/xml".into()));
    files.push((format!("{name}/styles.xml"), Some(Arc::from(styles.into_bytes())), "text/xml".into()));
    Some(format!(r#"<draw:object xlink:href="./{}" xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad"/>"#, esc(&name)))
}

/// The ProgID of an object's data: an ODF package's class, or the one
/// a compound file names.
fn prog_id(data: &[u8]) -> String {
    if let Ok(z) = ZipReader::new(data)
        && let Ok(m) = z.read_string("mimetype")
    {
        return crate::embed::odf_class(m.trim()).to_string();
    }
    crate::embed::compobj_prog_id(data).unwrap_or_else(|| "Package".into())
}

/// Media type of a replacement picture.
fn picture_type(data: &[u8]) -> &'static str {
    if data.starts_with(b"VCLMTF") {
        r#"application/x-openoffice-gdimetafile;windows_formatname="GDIMetaFile""#
    } else {
        crate::docx_img::image_type(data).1
    }
}

/// Writes object `o` as object number `n` of the package: the files go to
/// `files` (path, bytes, media type; folders have no bytes) and the inside
/// of its `draw:frame` is returned. `preview` is its picture.
pub(crate) fn write(o: &OleObject, preview: &Arc<[u8]>, n: usize, files: &mut Vec<(String, Option<Arc<[u8]>>, String)>) -> String {
    let name = format!("Embedded Object {n}");
    let converted = crate::embed::for_family(o, true);
    let o = converted.as_ref().unwrap_or(o);
    let tag = match crate::embed::odf_ole(o) {
        Some(ole) => {
            files.push((name.clone(), Some(Arc::from(ole)), "application/vnd.sun.star.oleobject".into()));
            "draw:object-ole"
        }
        None => {
            let z = ZipReader::new(&o.data).ok();
            let mime = z.as_ref().and_then(|z| z.read_string("mimetype").ok()).unwrap_or_default();
            let types: HashMap<String, String> = z.as_ref().map(|z| manifest(z).into_iter().collect()).unwrap_or_default();
            files.push((format!("{name}/"), None, mime.trim().to_string()));
            if let Some(z) = &z {
                let names: Vec<String> = z.names().filter(|p| *p != "mimetype" && !p.starts_with("META-INF/") && !p.ends_with('/')).map(str::to_string).collect();
                for p in names {
                    if let Ok(d) = z.read(&p) {
                        let ty = types.get(&p).cloned().unwrap_or_else(|| if p.ends_with(".xml") { "text/xml".into() } else { String::new() });
                        files.push((format!("{name}/{p}"), Some(Arc::from(d)), ty));
                    }
                }
            }
            "draw:object"
        }
    };
    let object = format!(r#"<{tag} xlink:href="./{}" xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad"/>"#, esc(&name));
    // Without a picture (LibreOffice draws the object's own).
    if preview.is_empty() {
        return object;
    }
    let rep = format!("ObjectReplacements/{name}");
    files.push((rep.clone(), Some(preview.clone()), picture_type(preview).to_string()));
    format!(r#"{object}<draw:image xlink:href="./{}" xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad"/>"#, esc(&rep))
}

/// A SmartArt as a group of its drawn shapes (ODF has no diagrams), at
/// `(x, y)` with size `size`; `anchor` holds the group's anchor attributes.
pub(crate) fn write_diagram(shapes: &mut crate::odf_shape::Writer, out: &mut String, d: &apex_doc::Diagram, at: (f32, f32), size: (f32, f32), anchor: &str, z: i64) {
    let (sx, sy) = (size.0 / d.size.0.max(1.0), size.1 / d.size.1.max(1.0));
    let _ = write!(out, r#"<draw:g {anchor} draw:z-index="{z}">"#);
    for s in &d.shapes {
        let place = crate::odf_shape::Place { x: at.0 + s.x * sx, y: at.1 + s.y * sy, w: s.w * sx, h: s.h * sy, attrs: "", graphic: "", z };
        shapes.write(out, &s.shape, &place);
    }
    out.push_str("</draw:g>");
}

/// Manifest entries of files written by [`write`].
pub(crate) fn manifest_entries(files: &[(String, Option<Arc<[u8]>>, String)]) -> String {
    let mut s = String::new();
    for (p, _, ty) in files {
        let _ = write!(s, r#"<manifest:file-entry manifest:full-path="{}" manifest:media-type="{}"/>"#, esc(p), esc(ty));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sub_documents_and_ole_round_trip() {
        let ods = crate::ods::write(&apex_sheet::Workbook::new());
        let sub = OleObject { prog_id: "x".into(), data: Arc::from(ods.clone()), ..Default::default() };
        let ole = crate::embed::from_file("a.pdf", b"%PDF-1.4".to_vec());
        let mut files = Vec::new();
        let x1 = write(&sub, &Arc::from(&[][..]), 1, &mut files);
        let x2 = write(&ole, &Arc::from(&b"\x89PNG"[..]), 2, &mut files);
        assert!(x1.starts_with(r#"<draw:object xlink:href="./Embedded Object 1""#));
        assert!(x2.starts_with(r#"<draw:object-ole xlink:href="./Embedded Object 2""#));
        // No made-up picture for an object without one.
        assert!(!x1.contains("draw:image") && !files.iter().any(|f| f.0 == "ObjectReplacements/Embedded Object 1"));
        let mut w = ZipWriter::new();
        w.add("mimetype", b"application/vnd.oasis.opendocument.text", false);
        let man = format!(r#"<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0">{}</manifest:manifest>"#, manifest_entries(&files));
        w.add("META-INF/manifest.xml", man.as_bytes(), true);
        for (p, d, _) in &files {
            if let Some(d) = d {
                w.add(p, d, true);
            }
        }
        let pkg = w.finish();
        let z = ZipReader::new(&pkg).unwrap();
        let mut found = HashMap::new();
        load(&z, &mut found);
        let a = object(&found, "./Embedded Object 1").unwrap();
        let a = a.ole().unwrap();
        assert_eq!(a.prog_id, "opendocument.CalcDocument.1");
        // The sub-document opens as a workbook.
        assert!(matches!(crate::embed::openable(a), Some((_, crate::Format::Ods))));
        let b = object(&found, "Embedded Object 2").unwrap();
        assert_eq!(b.ole().unwrap().prog_id, "Package");
        assert_eq!(crate::embed::contents(b.ole().unwrap()).1, b"%PDF-1.4");
        assert!(found.contains_key("ObjectReplacements/Embedded Object 2"));
    }
}
