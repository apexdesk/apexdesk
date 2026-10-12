//! Parts of xlsx and docx packages kept as they were (see
//! [`apex_doc::kept`]): reading them with everything their relationships
//! reach, and writing back the ones something still points at, with their
//! content types. Custom XML and the sensitivity label list live here;
//! the workbook's own (connections, pivot tables…) in `xlsx_keep`.

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{KeptLink, KeptPart, KeptParts, SensitivityLabel};

use crate::xml::{Ev, HEADER, Reader, esc};
use crate::zip::{ZipReader, ZipWriter};

pub(crate) const REL_CUSTOM_XML: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXml";
const REL_CUSTOM_XML_PROPS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXmlProps";
const CT_CUSTOM_XML_PROPS: &str = "application/vnd.openxmlformats-officedocument.customXmlProperties+xml";
/// Sensitivity labels of newer Office (`docMetadata/LabelInfo.xml`).
const REL_LABELS: &str = "http://schemas.microsoft.com/office/2020/02/relationships/classificationlabels";

/// `[Content_Types].xml`: content types by extension and by part.
pub(crate) struct ContentTypes {
    defaults: HashMap<String, String>,
    overrides: HashMap<String, String>,
}

impl ContentTypes {
    pub(crate) fn read(z: &ZipReader) -> Self {
        let mut ct = ContentTypes { defaults: HashMap::new(), overrides: HashMap::new() };
        let Ok(xml) = z.read_string("[Content_Types].xml") else { return ct };
        let mut r = Reader::new(&xml);
        loop {
            match r.next() {
                Ev::Eof => break,
                Ev::Start(t) | Ev::Empty(t) => {
                    let ty = t.attr("ContentType").unwrap_or_default().into_owned();
                    match t.local() {
                        "Default" => {
                            ct.defaults.insert(t.attr("Extension").unwrap_or_default().to_ascii_lowercase(), ty);
                        }
                        "Override" => {
                            ct.overrides.insert(t.attr("PartName").unwrap_or_default().trim_start_matches('/').to_ascii_lowercase(), ty);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        ct
    }

    pub(crate) fn get(&self, path: &str) -> String {
        if let Some(t) = self.overrides.get(&path.to_ascii_lowercase()) {
            return t.clone();
        }
        let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
        self.defaults.get(&ext).cloned().unwrap_or_else(|| "application/octet-stream".into())
    }
}

/// The relationships part of a part.
pub(crate) fn rels_path(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
        None => format!("_rels/{path}.rels"),
    }
}

fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

/// The relationships of a part: (id, type, target in the package or as
/// written when external, external).
pub(crate) fn read_rels(z: &ZipReader, path: &str) -> Vec<(String, String, String, bool)> {
    let Ok(xml) = z.read_string(&rels_path(path)) else { return Vec::new() };
    parse_rels(&xml, dir_of(path))
}

pub(crate) fn parse_rels(xml: &str, dir: &str) -> Vec<(String, String, String, bool)> {
    let mut out = Vec::new();
    let mut r = Reader::new(xml);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "Relationship" => {
                let (Some(id), Some(target)) = (t.attr("Id"), t.attr("Target").filter(|t| t.len() <= crate::docx::MAX_REL_TARGET)) else { continue };
                let external = t.attr("TargetMode").is_some_and(|v| v == "External");
                let target = if external { target.into_owned() } else { crate::xlsx::resolve_target(dir, &target) };
                out.push((id.into_owned(), t.attr("Type").unwrap_or_default().into_owned(), target, external));
            }
            _ => {}
        }
    }
    out
}

/// Parts the writers make themselves, which are never kept as found.
fn written_anew(path: &str) -> bool {
    let p = path.trim_start_matches('/').to_lowercase();
    matches!(
        p.as_str(),
        "[content_types].xml" | "_rels/.rels" | "docprops/core.xml" | "docprops/app.xml" | "docprops/custom.xml" | "word/document.xml" | "word/styles.xml" | "word/numbering.xml" | "word/settings.xml" | "word/fonttable.xml" | "xl/workbook.xml" | "xl/styles.xml" | "xl/sharedstrings.xml"
    ) || p.starts_with("word/_rels/document")
        || p.starts_with("xl/_rels/workbook")
        || p.starts_with("xl/worksheets/")
        || p.starts_with("word/theme/")
        || p.starts_with("xl/theme/")
        || p.contains("vbaproject")
        || p.contains("vbadata")
        || p.starts_with("customui/")
}

/// Keeps the part at `path` and what its relationships reach; false if
/// the package has no such part.
pub(crate) fn collect(z: &ZipReader, ct: &ContentTypes, path: &str, kept: &mut KeptParts) -> bool {
    collect_once(z, ct, path, kept, &mut std::collections::HashSet::new())
}

/// [`collect`], each part once however its name is written (`/x.xml`,
/// `X.xml`... are the same part to the package).
fn collect_once(z: &ZipReader, ct: &ContentTypes, path: &str, kept: &mut KeptParts, seen: &mut std::collections::HashSet<String>) -> bool {
    if kept.part(path).is_some() {
        return true;
    }
    // A relationship may lead anywhere: never to what the writers make
    // themselves (the old document or its macros would come back).
    if written_anew(path) {
        return false;
    }
    let Some(stored) = z.entry_name(path) else { return false };
    if !seen.insert(stored.to_string()) {
        return true;
    }
    let Ok(data) = z.read(path) else { return false };
    kept.add(KeptPart { path: path.to_string(), content_type: ct.get(path), data: Arc::from(data) });
    let rp = rels_path(path);
    if let Ok(rels) = z.read(&rp) {
        kept.add(KeptPart { path: rp, content_type: String::new(), data: Arc::from(rels) });
        for (_, _, target, external) in read_rels(z, path) {
            if !external {
                collect_once(z, ct, &target, kept, seen);
            }
        }
    }
    true
}

/// Keeps a custom XML part (and its properties) under a new name, so that
/// parts ApexDesk writes itself (the bibliography's `item1.xml`) never
/// clash with it. Returns the relationship to it.
pub(crate) fn custom_xml(z: &ZipReader, ct: &ContentTypes, path: &str, kept: &mut KeptParts) -> Option<KeptLink> {
    let data = z.read(path).ok()?;
    let n = (2..).find(|n| kept.part(&format!("customXml/item{n}.xml")).is_none()).unwrap_or(2);
    let item = format!("customXml/item{n}.xml");
    kept.add(KeptPart { path: item.clone(), content_type: ct.get(path), data: Arc::from(data) });
    let props = read_rels(z, path).into_iter().find(|(_, ty, _, ext)| ty == REL_CUSTOM_XML_PROPS && !ext).and_then(|(_, _, t, _)| z.read(&t).ok());
    if let Some(p) = props {
        kept.add(KeptPart { path: format!("customXml/itemProps{n}.xml"), content_type: CT_CUSTOM_XML_PROPS.into(), data: Arc::from(p) });
        let rels = format!(r#"{HEADER}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL_CUSTOM_XML_PROPS}" Target="itemProps{n}.xml"/></Relationships>"#);
        kept.add(KeptPart { path: rels_path(&item), content_type: String::new(), data: Arc::from(rels.into_bytes()) });
    }
    Some(KeptLink { rel_type: REL_CUSTOM_XML.into(), target: item, id: String::new() })
}

/// The custom XML parts related to `main`, but those `skip` says ApexDesk
/// writes itself.
pub(crate) fn read_custom_xml(z: &ZipReader, ct: &ContentTypes, main: &str, kept: &mut KeptParts, skip: &dyn Fn(&str) -> bool) {
    // Each part once, and a few hundred at most: each is copied and named
    // anew.
    let mut seen = std::collections::HashSet::new();
    for (id, ty, target, external) in read_rels(z, main) {
        if ty != REL_CUSTOM_XML || external || seen.len() >= 256 || !seen.insert(target.clone()) || z.read_string(&target).is_ok_and(|x| skip(&x)) {
            continue;
        }
        if let Some(mut l) = custom_xml(z, ct, &target, kept) {
            l.id = id;
            kept.links.push(l);
        }
    }
}

/// Parts related to the package itself: the sensitivity label list.
pub(crate) fn read_package(z: &ZipReader, ct: &ContentTypes, kept: &mut KeptParts) {
    let Ok(xml) = z.read_string("_rels/.rels") else { return };
    for (id, ty, target, external) in parse_rels(&xml, "") {
        if ty != REL_LABELS || external || !collect(z, ct, &target, kept) {
            continue;
        }
        if let Ok(x) = z.read_string(&target) {
            kept.labels = read_labels(&x);
        }
        kept.package_links.push(KeptLink { rel_type: ty, target, id });
    }
}

/// `clbl:labelList`: the labels on the file (not the removed ones).
fn read_labels(xml: &str) -> Vec<SensitivityLabel> {
    let mut out = Vec::new();
    let mut r = Reader::new(xml);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "label" => {
                let on = |k: &str, d: bool| t.attr(k).map_or(d, |v| v == "1" || v == "true");
                if on("removed", false) {
                    continue;
                }
                let s = |k: &str| t.attr(k).unwrap_or_default().into_owned();
                out.push(SensitivityLabel { id: s("id"), name: String::new(), site: s("siteId"), method: s("method"), enabled: on("enabled", true) });
            }
            _ => {}
        }
    }
    out
}

/// How an XML part is encoded (custom XML parts are often UTF-16).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum XmlEncoding {
    Utf8 { bom: bool },
    Utf16Le,
    Utf16Be,
}

/// The text of an XML part and its encoding.
pub(crate) fn decode_xml(b: &[u8]) -> (String, XmlEncoding) {
    let utf16 = |b: &[u8], le: bool| {
        let units: Vec<u16> = b.chunks_exact(2).map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) }).collect();
        String::from_utf16_lossy(&units)
    };
    match b {
        [0xFF, 0xFE, rest @ ..] => (utf16(rest, true), XmlEncoding::Utf16Le),
        [0xFE, 0xFF, rest @ ..] => (utf16(rest, false), XmlEncoding::Utf16Be),
        [0x3C, 0x00, ..] => (utf16(b, true), XmlEncoding::Utf16Le),
        [0x00, 0x3C, ..] => (utf16(b, false), XmlEncoding::Utf16Be),
        [0xEF, 0xBB, 0xBF, rest @ ..] => (String::from_utf8_lossy(rest).into_owned(), XmlEncoding::Utf8 { bom: true }),
        _ => (String::from_utf8_lossy(b).into_owned(), XmlEncoding::Utf8 { bom: false }),
    }
}

/// XML text encoded as its part was (UTF-16 with a byte order mark).
pub(crate) fn encode_xml(s: &str, e: XmlEncoding) -> Vec<u8> {
    match e {
        XmlEncoding::Utf8 { bom } => [if bom { &[0xEF, 0xBB, 0xBF][..] } else { &[] }, s.as_bytes()].concat(),
        XmlEncoding::Utf16Le => [0xFF, 0xFE].into_iter().chain(s.encode_utf16().flat_map(u16::to_le_bytes)).collect(),
        XmlEncoding::Utf16Be => [0xFE, 0xFF].into_iter().chain(s.encode_utf16().flat_map(u16::to_be_bytes)).collect(),
    }
}

/// A target relative to the directory of the part pointing at it.
pub(crate) fn relative(from_dir: &str, target: &str) -> String {
    let from: Vec<&str> = from_dir.split('/').filter(|s| !s.is_empty()).collect();
    let to: Vec<&str> = target.split('/').collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut out: Vec<&str> = vec![".."; from.len() - common];
    out.extend(&to[common..]);
    out.join("/")
}

/// The kept parts that go into a file being written: those reached from
/// the relationships the writer keeps, followed through the kept
/// relationship parts.
pub(crate) struct Reach<'a> {
    kept: &'a KeptParts,
    pub(crate) parts: BTreeSet<&'a str>,
}

impl<'a> Reach<'a> {
    pub(crate) fn new(kept: &'a KeptParts) -> Self {
        Reach { kept, parts: BTreeSet::new() }
    }

    /// Takes in a part and all it reaches; false if it isn't kept.
    pub(crate) fn add(&mut self, path: &str) -> bool {
        let Some(p) = self.kept.part(path) else { return false };
        if !self.parts.insert(p.path.as_str()) {
            return true;
        }
        if let Some(r) = self.kept.part(&rels_path(path)) {
            self.parts.insert(r.path.as_str());
            for (_, _, target, external) in parse_rels(&String::from_utf8_lossy(&r.data), dir_of(path)) {
                if !external {
                    self.add(&target);
                }
            }
        }
        true
    }

    pub(crate) fn has(&self, path: &str) -> bool {
        self.parts.contains(path)
    }

    /// Adds the parts to the package; returns their content type overrides.
    pub(crate) fn write(&self, z: &mut ZipWriter) -> String {
        let mut ct = String::new();
        for p in self.kept.parts.iter().filter(|p| self.parts.contains(p.path.as_str())) {
            z.add(&p.path, &p.data, true);
            if !p.content_type.is_empty() {
                let _ = write!(ct, r#"<Override PartName="/{}" ContentType="{}"/>"#, esc(&p.path), esc(&p.content_type));
            }
        }
        ct
    }
}

/// A relationship element from a part in `from_dir`.
pub(crate) fn rel(id: &str, l: &KeptLink, from_dir: &str) -> String {
    format!(r#"<Relationship Id="{id}" Type="{}" Target="{}"/>"#, esc(&l.rel_type), esc(&relative(from_dir, &l.target)))
}

/// Kept parts of a document or workbook besides the workbook's own: its
/// custom XML (from the main part in `main_dir`) and the package's.
/// Returns (main part relationships, package relationships, content type
/// overrides).
pub(crate) fn write_common(z: &mut ZipWriter, kept: &KeptParts, main_dir: &str, extra: Option<Reach<'_>>) -> (String, String, String) {
    let mut reach = extra.unwrap_or_else(|| Reach::new(kept));
    let (mut rels, mut pkg) = (String::new(), String::new());
    for (i, l) in kept.links.iter().enumerate() {
        if l.rel_type == REL_CUSTOM_XML && reach.add(&l.target) {
            rels.push_str(&rel(&format!("rIdK{}", i + 1), l, main_dir));
        }
    }
    for (i, l) in kept.package_links.iter().enumerate() {
        if reach.add(&l.target) {
            pkg.push_str(&rel(&format!("rIdKP{}", i + 1), l, ""));
        }
    }
    (rels, pkg, reach.write(z))
}

/// What a docx keeps: custom XML but the bibliography (written from the
/// sources) and the label list.
pub(crate) fn read_docx(z: &ZipReader) -> KeptParts {
    let ct = ContentTypes::read(z);
    let mut kept = KeptParts::default();
    // Ink parts share the custom XML relationship; they are read as ink.
    read_custom_xml(z, &ct, "word/document.xml", &mut kept, &|x| x.contains(crate::docx_biblio::NS) || x.contains(crate::book_xml::NS) || x.contains("http://www.w3.org/2003/InkML"));
    read_package(z, &ct, &mut kept);
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_targets() {
        assert_eq!(relative("xl", "xl/connections.xml"), "connections.xml");
        assert_eq!(relative("word", "customXml/item2.xml"), "../customXml/item2.xml");
        assert_eq!(relative("", "docMetadata/LabelInfo.xml"), "docMetadata/LabelInfo.xml");
        assert_eq!(relative("xl/worksheets", "xl/pivotTables/pivotTable1.xml"), "../pivotTables/pivotTable1.xml");
    }

    #[test]
    fn encodings() {
        for e in [XmlEncoding::Utf8 { bom: false }, XmlEncoding::Utf8 { bom: true }, XmlEncoding::Utf16Le, XmlEncoding::Utf16Be] {
            assert_eq!(decode_xml(&encode_xml("<a>ñ€</a>", e)), ("<a>ñ€</a>".to_string(), e));
        }
    }

    #[test]
    fn label_list() {
        let x = r#"<clbl:labelList xmlns:clbl="http://schemas.microsoft.com/office/2020/mipLabelMetadata"><clbl:label id="{A}" enabled="1" method="Standard" siteId="{S}" removed="0"/><clbl:label id="{B}" enabled="1" removed="1"/></clbl:labelList>"#;
        let l = read_labels(x);
        assert_eq!(l.len(), 1);
        assert_eq!((l[0].id.as_str(), l[0].site.as_str(), l[0].enabled), ("{A}", "{S}", true));
    }
}
