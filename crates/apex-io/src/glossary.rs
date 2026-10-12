//! Building blocks (AutoText, Quick Parts) in the glossary document of a
//! docx or dotx (`word/glossary/document.xml`), as Word keeps them in
//! Normal.dotm and "Building Blocks.dotx". Listing is cheap (names,
//! categories and a text preview); a block's content is read on demand
//! through the ordinary docx reader, from a copy of the package whose
//! main part is the block. A package of blocks is written the same way
//! back: each block through the docx writer, its body moved into a
//! `w:docPart`. Styles and numbering of the package come from the first
//! block.

use apex_doc::{Document, Paragraph};

use crate::xml::{esc, unescape};
use crate::zip::{ZipReader, ZipWriter};

const GLOSSARY: &str = "word/glossary/document.xml";
const GLOSSARY_RELS: &str = "word/glossary/_rels/document.xml.rels";
const GLOSSARY_TYPE: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document.glossary+xml";
const GLOSSARY_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/glossaryDocument";
const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// A building block as listed.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub category: String,
    /// Word's gallery (`autoTxt`, `docParts`, `hdrs`...).
    pub gallery: String,
    /// The start of its text.
    pub preview: String,
    /// Its place in the glossary, for [`load`].
    pub index: usize,
}

/// A block to write.
pub struct Block<'a> {
    pub name: &'a str,
    pub category: &'a str,
    pub gallery: &'a str,
    pub doc: &'a Document,
}

/// The `w:docPart` elements of a glossary: (whole element, body inside).
fn parts(xml: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(a) = rest.find("<w:docPart>").or_else(|| rest.find("<w:docPart ")) {
        let Some(len) = rest[a..].find("</w:docPart>") else { break };
        let part = &rest[a..a + len + "</w:docPart>".len()];
        let body = match (part.find("<w:docPartBody>"), part.find("</w:docPartBody>")) {
            (Some(s), Some(e)) if e > s => &part[s + "<w:docPartBody>".len()..e],
            _ => "",
        };
        out.push((part, body));
        rest = &rest[a + part.len()..];
    }
    out
}

/// The `w:val` of the first `<w:{tag} .../>` in `xml`.
fn val<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let a = xml.find(&format!("<w:{tag} "))?;
    let el = &xml[a..a + xml[a..].find('>')?];
    let v = el.find("w:val=\"")? + "w:val=\"".len();
    Some(&el[v..v + el[v..].find('"')?])
}

fn preview(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while out.chars().count() < 200 {
        let (Some(t), Some(p)) = (rest.find("<w:t"), rest.find("</w:p>").or(Some(usize::MAX))) else { break };
        if p < t {
            out.push(' ');
            rest = &rest[p + 6..];
            continue;
        }
        let tail = &rest[t + 4..];
        // `<w:t>` or `<w:t xml:space=...>`, not `<w:tab/>` or `<w:tbl>`.
        if !tail.starts_with('>') && !tail.starts_with(' ') {
            rest = tail;
            continue;
        }
        let Some(open) = tail.find('>') else { break };
        let Some(close) = tail.find("</w:t>") else { break };
        out.push_str(&unescape(&tail[open + 1..close]));
        rest = &tail[close + 6..];
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(200).collect()
}

/// The building blocks of a docx/dotx package (none without a glossary).
pub fn list(data: &[u8]) -> Vec<Entry> {
    let Ok(z) = ZipReader::new(data) else { return Vec::new() };
    let Ok(xml) = z.read_string(GLOSSARY) else { return Vec::new() };
    parts(&xml)
        .into_iter()
        .enumerate()
        .filter_map(|(index, (part, body))| {
            let pr = part.split("<w:docPartBody").next().unwrap_or(part);
            let name = unescape(val(pr, "name")?).into_owned();
            let cat_xml = pr.find("<w:category>").map_or("", |c| &pr[c..]);
            let category = val(cat_xml, "name").map(|c| unescape(c).into_owned()).unwrap_or_default();
            let gallery = val(cat_xml, "gallery").unwrap_or("autoTxt").to_string();
            Some(Entry { name, category, gallery, preview: preview(body), index })
        })
        .collect()
}

/// A relationships part with relative targets moved from `word/glossary/`
/// to `word/`.
fn moved_rels(rels: &str) -> String {
    let mut out = String::with_capacity(rels.len());
    let mut rest = rels;
    while let Some(a) = rest.find("Target=\"") {
        let v = a + "Target=\"".len();
        out.push_str(&rest[..v]);
        rest = &rest[v..];
        let external = rest.starts_with("http:") || rest.starts_with("https:") || rest.starts_with("mailto:") || rest.starts_with('/') || rest.starts_with("file:");
        if !external {
            out.push_str("glossary/");
        }
    }
    out.push_str(rest);
    out
}

/// The content of block `index` as a document.
pub fn load(data: &[u8], index: usize) -> Option<Document> {
    let z = ZipReader::new(data).ok()?;
    let xml = z.read_string(GLOSSARY).ok()?;
    let (_, body) = *parts(&xml).get(index)?;
    let root = xml.find("<w:glossaryDocument")?;
    let decl_end = root + xml[root..].find('>')?;
    let decls = xml[root + "<w:glossaryDocument".len()..decl_end].trim_end_matches('/');
    let doc_xml = format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:document{decls}><w:body>{body}</w:body></w:document>");
    let rels = z.read_string(GLOSSARY_RELS).unwrap_or_else(|_| r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"/>"#.to_string());
    let mut w = ZipWriter::new();
    for name in z.names().map(str::to_string).collect::<Vec<_>>() {
        let swap = match name.as_str() {
            "word/document.xml" => Some(doc_xml.as_bytes().to_vec()),
            "word/_rels/document.xml.rels" => Some(moved_rels(&rels).into_bytes()),
            "word/styles.xml" | "word/numbering.xml" => z.read(&format!("word/glossary/{}", &name[5..])).ok(),
            _ => None,
        };
        let data = match swap {
            Some(d) => d,
            None => z.read(&name).ok()?,
        };
        w.add(&name, &data, true);
    }
    let mut doc = crate::docx::read(&w.finish()).ok()?;
    doc.meta = Default::default();
    // A block is a fragment: a trailing empty paragraph adds nothing.
    if doc.paragraphs.len() > 1 && doc.paragraphs.last().is_some_and(|p| p.text.is_empty()) {
        doc.paragraphs.pop();
    }
    Some(doc)
}

/// The body of a written document without its section properties.
fn body_of(docx: &[u8]) -> Option<(String, ZipReader<'_>)> {
    let z = ZipReader::new(docx).ok()?;
    let xml = z.read_string("word/document.xml").ok()?;
    let a = xml.find("<w:body>")? + "<w:body>".len();
    let e = xml.rfind("</w:body>")?;
    let mut body = xml[a..e].to_string();
    // The last sectPr is the section's, not the block's.
    if let Some(s) = body.rfind("<w:sectPr") {
        body.truncate(s);
    }
    Some((body, z))
}

/// Relationships of `rels` that a block's content uses (pictures, links,
/// objects), as (id, type, target, external).
fn content_rels(rels: &str) -> Vec<(String, String, String, bool)> {
    let mut out = Vec::new();
    for el in rels.split("<Relationship ").skip(1) {
        let attr = |n: &str| el.find(&format!("{n}=\"")).map(|a| a + n.len() + 2).and_then(|v| el[v..].find('"').map(|e| el[v..v + e].to_string()));
        let (Some(id), Some(ty), Some(target)) = (attr("Id"), attr("Type"), attr("Target")) else { continue };
        let kind = ty.rsplit('/').next().unwrap_or("");
        if matches!(kind, "image" | "hyperlink" | "oleObject" | "package") {
            out.push((id, ty, target, attr("TargetMode").is_some_and(|m| m == "External")));
        }
    }
    out
}

/// A docx package holding `blocks` in its glossary (the main document
/// says what the file is).
pub fn write(blocks: &[Block]) -> Vec<u8> {
    let mut main = Document::from_paragraphs(vec![Paragraph::new("ApexDesk building blocks")]);
    main.meta.title = "Building Blocks".into();
    let base = crate::docx::write(&main);
    let mut xml = format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:glossaryDocument xmlns:w=\"{W}\" xmlns:r=\"{R}\" xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\" xmlns:wps=\"http://schemas.microsoft.com/office/word/2010/wordprocessingShape\" xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" xmlns:w14=\"http://schemas.microsoft.com/office/word/2010/wordml\" xmlns:v=\"urn:schemas-microsoft-com:vml\" xmlns:o=\"urn:schemas-microsoft-com:office:office\"><w:docParts>");
    let mut rels = String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#);
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut shared: Vec<(&str, Vec<u8>)> = Vec::new();
    for (k, b) in blocks.iter().enumerate() {
        let one = crate::docx::write(b.doc);
        let Some((mut body, z)) = body_of(&one) else { continue };
        if k == 0 {
            for part in ["styles.xml", "numbering.xml"] {
                if let Ok(d) = z.read(&format!("word/{part}")) {
                    shared.push((part, d));
                }
            }
        }
        let own = z.read_string("word/_rels/document.xml.rels").unwrap_or_default();
        for (id, ty, target, external) in content_rels(&own) {
            let new_id = format!("b{k}{id}");
            body = body.replace(&format!("\"{id}\""), &format!("\"{new_id}\""));
            let target = if external {
                rels.push_str(&format!(r#"<Relationship Id="{new_id}" Type="{ty}" Target="{}" TargetMode="External"/>"#, esc(&target)));
                continue;
            } else {
                let file = format!("media/b{k}_{}", target.rsplit('/').next().unwrap_or(&target));
                if let Ok(d) = z.read(&format!("word/{}", target.trim_start_matches('/').trim_start_matches("word/"))) {
                    files.push((format!("word/glossary/{file}"), d));
                }
                file
            };
            rels.push_str(&format!(r#"<Relationship Id="{new_id}" Type="{ty}" Target="{target}"/>"#));
        }
        xml.push_str(&format!(
            "<w:docPart><w:docPartPr><w:name w:val=\"{}\"/><w:category><w:name w:val=\"{}\"/><w:gallery w:val=\"{}\"/></w:category><w:behaviors><w:behavior w:val=\"content\"/></w:behaviors><w:guid w:val=\"{{{}}}\"/></w:docPartPr><w:docPartBody>{body}</w:docPartBody></w:docPart>",
            esc(b.name),
            esc(if b.category.is_empty() { "General" } else { b.category }),
            esc(if b.gallery.is_empty() { "autoTxt" } else { b.gallery }),
            guid(k, b.name),
        ));
    }
    xml.push_str("</w:docParts></w:glossaryDocument>");
    for (part, _) in &shared {
        let ty = if *part == "styles.xml" { "styles" } else { "numbering" };
        rels.push_str(&format!(r#"<Relationship Id="g{ty}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/{ty}" Target="{part}"/>"#));
    }
    rels.push_str("</Relationships>");
    let Ok(z) = ZipReader::new(&base) else { return base };
    let mut w = ZipWriter::new();
    for name in z.names().map(str::to_string).collect::<Vec<_>>() {
        let Ok(mut data) = z.read(&name) else { continue };
        if name == "[Content_Types].xml" {
            let mut s = String::from_utf8_lossy(&data).into_owned();
            let mut add = format!(r#"<Override PartName="/{GLOSSARY}" ContentType="{GLOSSARY_TYPE}"/>"#);
            for (part, _) in &shared {
                let ty = if *part == "styles.xml" { "styles" } else { "numbering" };
                add.push_str(&format!(r#"<Override PartName="/word/glossary/{part}" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.{ty}+xml"/>"#));
            }
            for ext in ["png", "jpeg", "jpg", "gif", "bmp", "emf", "wmf", "svg", "tiff", "bin"] {
                if files.iter().any(|(f, _)| f.to_ascii_lowercase().ends_with(&format!(".{ext}"))) && !s.contains(&format!("Extension=\"{ext}\"")) {
                    let ct = match ext {
                        "jpg" | "jpeg" => "image/jpeg".to_string(),
                        "svg" => "image/svg+xml".to_string(),
                        "emf" => "image/x-emf".to_string(),
                        "wmf" => "image/x-wmf".to_string(),
                        "bin" => "application/vnd.openxmlformats-officedocument.oleObject".to_string(),
                        e => format!("image/{e}"),
                    };
                    add.push_str(&format!(r#"<Default Extension="{ext}" ContentType="{ct}"/>"#));
                }
            }
            s = s.replacen("</Types>", &format!("{add}</Types>"), 1);
            data = s.into_bytes();
        } else if name == "word/_rels/document.xml.rels" {
            let s = String::from_utf8_lossy(&data).into_owned();
            data = s.replacen("</Relationships>", &format!(r#"<Relationship Id="rIdGlossary" Type="{GLOSSARY_REL}" Target="glossary/document.xml"/></Relationships>"#), 1).into_bytes();
        }
        w.add(&name, &data, true);
    }
    w.add(GLOSSARY, xml.as_bytes(), true);
    w.add(GLOSSARY_RELS, rels.as_bytes(), true);
    for (part, d) in &shared {
        w.add(&format!("word/glossary/{part}"), d, true);
    }
    for (name, d) in &files {
        w.add(name, d, false);
    }
    w.finish()
}

/// A stable GUID-shaped id for a block.
fn guid(k: usize, name: &str) -> String {
    let h = crate::zip::crc32(format!("{k}:{name}").as_bytes());
    let h2 = crate::zip::crc32(name.as_bytes());
    format!("{h:08X}-{:04X}-4{:03X}-8{:03X}-{h2:08X}{k:04X}", (h >> 16) & 0xFFFF, h2 & 0xFFF, (h2 >> 12) & 0xFFF)
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_doc::{Pos, Selection};

    #[test]
    fn blocks_round_trip() {
        let mut sig = Document::from_paragraphs(vec![Paragraph::new("Kind regards,"), Paragraph::new("Ana & Co")]);
        sig.format_range(Selection { anchor: Pos::new(1, 0), cursor: Pos::new(1, 3) }, &|a| a.bold = true);
        let addr = Document::from_paragraphs(vec![Paragraph::new("Main St 1 <HQ>")]);
        let data = write(&[Block { name: "sig", category: "Letters", gallery: "", doc: &sig }, Block { name: "Address & more", category: "", gallery: "docParts", doc: &addr }]);
        let l = list(&data);
        assert_eq!(l.len(), 2);
        assert_eq!((l[0].name.as_str(), l[0].category.as_str(), l[0].gallery.as_str(), l[0].preview.as_str()), ("sig", "Letters", "autoTxt", "Kind regards, Ana & Co"));
        assert_eq!((l[1].name.as_str(), l[1].category.as_str(), l[1].gallery.as_str()), ("Address & more", "General", "docParts"));
        let back = load(&data, 0).unwrap();
        let texts: Vec<&str> = back.paragraphs.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(texts, ["Kind regards,", "Ana & Co"]);
        assert!(back.paragraphs[1].attrs_at(1).bold && !back.paragraphs[1].attrs_at(5).bold);
        assert_eq!(load(&data, 1).unwrap().paragraphs[0].text, "Main St 1 <HQ>");
        assert!(load(&data, 2).is_none());
        // The package is a document of its own.
        assert_eq!(crate::docx::read(&data).unwrap().paragraphs[0].text, "ApexDesk building blocks");
        assert!(list(&crate::docx::write(&sig)).is_empty());
    }

    #[test]
    fn word_glossary() {
        let xml = r#"<w:glossaryDocument xmlns:w="x"><w:docParts><w:docPart><w:docPartPr><w:name w:val="Confidential"/><w:category><w:name w:val="Disclaimers"/><w:gallery w:val="autoTxt"/></w:category></w:docPartPr><w:docPartBody><w:p><w:r><w:t xml:space="preserve">Do not </w:t></w:r><w:r><w:tab/><w:t>share</w:t></w:r></w:p><w:tbl></w:tbl></w:docPartBody></w:docPart><w:docPart><w:docPartPr><w:category><w:name w:val="x"/></w:category></w:docPartPr><w:docPartBody/></w:docPart></w:docParts></w:glossaryDocument>"#;
        let p = parts(xml);
        assert_eq!(p.len(), 2);
        assert_eq!(preview(p[0].1), "Do not share");
        assert_eq!(val(p[0].0, "name"), Some("Confidential"));
        assert_eq!(moved_rels(r#"<Relationship Target="media/a.png"/><Relationship Target="https://x" TargetMode="External"/>"#), r#"<Relationship Target="glossary/media/a.png"/><Relationship Target="https://x" TargetMode="External"/>"#);
    }
}
