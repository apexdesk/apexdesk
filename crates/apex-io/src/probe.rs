//! Facts about a file read from its bytes without loading it (the command
//! line's `info`).

/// Whether the file carries macros: a VBA project in an OOXML package
/// (xlsm/docm), Basic libraries in an ODF package, or a VBA storage in a
/// legacy .xls/.doc.
pub fn has_macros(data: &[u8]) -> bool {
    if data.starts_with(b"PK") {
        let Ok(z) = crate::zip::ZipReader::new(data) else { return false };
        return z.names().any(|n| {
            let n = n.to_ascii_lowercase();
            n.ends_with("vbaproject.bin") || (n.starts_with("basic/") && n.ends_with(".xml") && !n.ends_with("script-lc.xml") && !n.ends_with("dialog-lc.xml"))
        });
    }
    if data.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        let Ok(c) = cfb::CompoundFile::open(std::io::Cursor::new(data)) else { return false };
        return c.is_storage("/_VBA_PROJECT_CUR") || c.is_storage("/Macros");
    }
    false
}

/// What a document inventory notes of a file (`apexdesk inventory`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// A format of old programs (binary .xls/.doc/.ppt, Lotus, Works,
    /// StarOffice), best converted.
    pub old_format: bool,
    pub macros: bool,
    /// Links to other files: external workbook references, linked
    /// objects, pictures and sections, attached templates.
    pub external_links: usize,
    pub signatures: usize,
    /// Encrypted with a password to open (the other facts are then
    /// unknown).
    pub password: bool,
}

/// Extensions of old formats.
const OLD: &[&str] = &["xls", "xlt", "xla", "doc", "dot", "ppt", "pps", "pot", "wk1", "wk3", "wk4", "wks", "wps", "wpt", "wpd", "wri", "sxc", "sxw", "sxi", "sxd", "stc", "stw", "123", "et", "ett"];

const OLE_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

/// The facts of a file with extension `ext`, from its bytes.
pub fn facts(data: &[u8], ext: &str) -> Facts {
    let ext = ext.to_ascii_lowercase();
    let mut f = Facts { old_format: OLD.contains(&ext.as_str()), ..Default::default() };
    if crate::is_encrypted(data) || (data.starts_with(b"%PDF") && pdf_encrypted(data)) {
        f.password = true;
        return f;
    }
    f.macros = has_macros(data);
    if data.starts_with(b"PK") {
        if let Ok(z) = crate::zip::ZipReader::new(data) {
            zip_facts(&z, &mut f);
        }
    } else if data.starts_with(&OLE_MAGIC) {
        f.old_format = true;
        if let Ok(c) = cfb::CompoundFile::open(std::io::Cursor::new(data)) {
            let signed = ["/_xmlsignatures", "/_signatures", "/\u{5}DigitalSignature", "/MsoDigitalSignature"];
            f.signatures = signed.iter().filter(|p| c.exists(p)).count();
            crate::crypto::walk_within(&c, &mut |e| {
                f.external_links += usize::from(e.name() == "LinkInfo");
                true
            });
        }
    } else if data.starts_with(b"%PDF") {
        f.signatures = memchr::memmem::find_iter(data, b"/ByteRange").count();
    }
    f
}

/// Whether a PDF has an /Encrypt entry (in its trailer or xref stream).
fn pdf_encrypted(data: &[u8]) -> bool {
    let tail = &data[data.len().saturating_sub(1 << 16)..];
    memchr::memmem::find(tail, b"/Encrypt").is_some()
}

fn zip_facts(z: &crate::zip::ZipReader<'_>, f: &mut Facts) {
    let names: Vec<String> = z.names().map(str::to_string).collect();
    for n in &names {
        let low = n.to_ascii_lowercase();
        if low.starts_with("_xmlsignatures/sig") && low.ends_with(".xml") {
            f.signatures += 1;
        } else if low == "meta-inf/documentsignatures.xml" || low == "meta-inf/macrosignatures.xml" {
            let xml = z.read_string(n).unwrap_or_default();
            f.signatures += xml.matches(":Signature ").count() + xml.matches("<Signature ").count();
        } else if low.ends_with(".rels") {
            // Links other than hyperlinks leave the package.
            let xml = z.read_string(n).unwrap_or_default();
            f.external_links += xml.split("<Relationship ").skip(1).filter(|r| r.contains("TargetMode=\"External\"") && !r.contains("/hyperlink\"")).count();
        } else if low == "content.xml" {
            let xml = z.read_string(n).unwrap_or_default();
            let linked = ["<table:table-source", "<text:section-source", "<table:dde-source", "<office:dde-source", "<text:dde-connection-decl "];
            f.external_links += linked.iter().map(|t| xml.matches(t).count()).sum::<usize>();
            // Pictures and objects kept outside the package.
            // The last '<' before each, looked for once over the whole part.
            let (mut seen, mut last_lt) = (0, None);
            for (i, _) in xml.match_indices("xlink:href=\"") {
                if let Some(p) = xml[seen..i].rfind('<') {
                    last_lt = Some(seen + p);
                }
                seen = i;
                let href = &xml[i + 12..];
                let inside = href.starts_with("Pictures/") || href.starts_with("./") || href.starts_with("Object") || href.starts_with('#') || href.starts_with("http") || href.starts_with("mailto:");
                let is_frame = last_lt.is_some_and(|s| xml[s..i].starts_with("<draw:image") || xml[s..i].starts_with("<draw:object"));
                if is_frame && !inside {
                    f.external_links += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_facts() {
        let mut w = crate::zip::ZipWriter::new();
        w.add("xl/workbook.xml", b"<workbook/>", true);
        w.add("xl/_rels/workbook.xml.rels", br#"<Relationships><Relationship Id="rId1" Type="http://x/externalLink" Target="externalLinks/externalLink1.xml"/></Relationships>"#, true);
        w.add("xl/externalLinks/_rels/externalLink1.xml.rels", br#"<Relationships><Relationship Id="rId1" Type="http://x/externalLinkPath" Target="file:///C:/data/prices.xlsx" TargetMode="External"/></Relationships>"#, true);
        w.add("xl/worksheets/_rels/sheet1.xml.rels", br#"<Relationships><Relationship Id="rId1" Type="http://x/hyperlink" Target="https://example.com" TargetMode="External"/></Relationships>"#, true);
        w.add("xl/vbaProject.bin", b"x", true);
        w.add("_xmlsignatures/sig1.xml", b"<Signature/>", true);
        let f = facts(&w.finish(), "xlsm");
        assert_eq!(f, Facts { old_format: false, macros: true, external_links: 1, signatures: 1, password: false });
        let mut w = crate::zip::ZipWriter::new();
        w.add("content.xml", br#"<office:document-content><text:section><text:section-source xlink:href="../clause.odt"/></text:section><draw:frame><draw:image xlink:href="../logo.png"/></draw:frame><draw:frame><draw:image xlink:href="Pictures/1.png"/></draw:frame></office:document-content>"#, true);
        w.add("META-INF/documentsignatures.xml", br#"<document-signatures><dsig:Signature Id="a"/><dsig:Signature Id="b"/></document-signatures>"#, true);
        let f = facts(&w.finish(), "odt");
        assert_eq!((f.external_links, f.signatures, f.macros), (2, 2, false));
        assert!(facts(b"%PDF-1.7\n<< /ByteRange [0 1 2 3] >>\ntrailer << /Encrypt 5 0 R >>", "pdf").password);
        assert_eq!(facts(b"%PDF-1.7\n<< /ByteRange [0 1 2 3] >>", "pdf").signatures, 1);
        assert!(facts(b"x", "wk1").old_format);
    }

    #[test]
    fn finds_vba_projects() {
        let mut w = crate::zip::ZipWriter::new();
        w.add("xl/workbook.xml", b"<workbook/>", true);
        assert!(!has_macros(&w.finish()));
        let mut w = crate::zip::ZipWriter::new();
        w.add("xl/vbaProject.bin", b"x", true);
        assert!(has_macros(&w.finish()));
        let mut w = crate::zip::ZipWriter::new();
        w.add("Basic/Standard/Module1.xml", b"x", true);
        assert!(has_macros(&w.finish()));
        assert!(!has_macros(b"plain text"));
    }
}
