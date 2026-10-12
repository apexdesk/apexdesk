//! The signatures of ZIP packages: OOXML keeps each in a part under
//! `_xmlsignatures/`, ODF all of them in `META-INF/documentsignatures.xml`.

use apex_io::zip::ZipReader;

use crate::c14n::{self, Elem};
use crate::xmldsig;
use crate::{Check, Kind, Signature};

pub const ODF_SIGNATURES: &str = "META-INF/documentsignatures.xml";

/// The parts holding OOXML signatures.
pub fn ooxml_parts(z: &ZipReader) -> Vec<String> {
    z.names().filter(|n| n.starts_with("_xmlsignatures/") && !n.contains("/_rels/") && n.ends_with(".xml")).map(str::to_string).collect()
}

/// Whether the package has document signatures (without checking them).
pub fn is_signed(data: &[u8]) -> bool {
    ZipReader::new(data).is_ok_and(|z| z.contains(ODF_SIGNATURES) || !ooxml_parts(&z).is_empty())
}

/// Parts an ODF signature must cover: the document's streams (LibreOffice
/// leaves the mimetype, the manifest and the signatures out, older
/// versions signed them). Anything else kept in META-INF counts: readers
/// may take pictures from there.
pub fn odf_needs_signing(name: &str) -> bool {
    !(name == "mimetype" || name.ends_with('/') || matches!(name, "META-INF/manifest.xml" | "META-INF/documentsignatures.xml" | "META-INF/macrosignatures.xml"))
}

/// The parts of an OOXML package a signature covering `parts` leaves out,
/// beyond those Office and LibreOffice never sign (document properties,
/// content types, the signatures). Relationship parts count only when
/// they point outside the package (a link's address could be changed):
/// LibreOffice leaves them out, and one pointing at a part added later
/// shows that part here. Nor may one lead to two signed parts of a kind
/// (two sheets, two pictures): which is which could then be swapped.
pub(crate) fn ooxml_uncovered(z: &ZipReader, parts: &[String]) -> Vec<String> {
    use crate::xml_sign::ooxml_unsigned_part;
    // Exactly: a part named like a signed one but in other capitals is not it.
    let signed: std::collections::HashSet<&str> = parts.iter().map(String::as_str).collect();
    let covered = |n: &str| signed.contains(n);
    // Links count too, with or without their TargetMode (readers take the
    // target of a hyperlink as its address either way).
    let outside = |n: &str| z.read(n).is_ok_and(|b| b.windows(10).any(|w| w == b"TargetMode") || b.windows(10).any(|w| w == b"/hyperlink") || points_unsigned(z, n, &b, parts));
    let mut out: Vec<String> = z.names().filter(|n| !ooxml_unsigned_part(n) && !covered(n) && (!n.ends_with(".rels") || outside(n))).map(str::to_string).collect();
    // How many relationships lead to each part, wherever they are in a
    // relationships part (readers take them at any depth). A signed part
    // none leads to any more was taken out of the document; one two lead
    // to, through relationships not signed, may stand in for another.
    let mut led: std::collections::HashMap<String, Vec<&str>> = std::collections::HashMap::new();
    for n in z.names().filter(|n| n.ends_with(".rels") && !ooxml_unsigned_part(n)) {
        let Ok(b) = z.read(n) else { continue };
        for t in all_targets(n, &b) {
            led.entry(t).or_default().push(n);
        }
    }
    // Those already shown as unsigned don't count twice: they are the
    // ones that don't hold.
    let flagged: std::collections::HashSet<String> = out.iter().cloned().collect();
    let mut shown = flagged.clone();
    for p in parts.iter().filter(|p| !p.ends_with(".rels") && !ooxml_unsigned_part(p) && z.contains(p)) {
        match led.get(p.as_str()) {
            None => {
                if shown.insert(p.clone()) {
                    out.push(p.clone());
                }
            }
            Some(from) if from.iter().filter(|f| !flagged.contains(**f)).count() > 1 => {
                for f in from.iter().filter(|f| !covered(f)) {
                    if shown.insert(f.to_string()) {
                        out.push(f.to_string());
                    }
                }
            }
            Some(_) => {}
        }
    }
    // Parts never signed that relationships use for something else (a
    // sheet or a picture kept among the signatures or as properties)
    // could change unseen.
    for n in z.names().filter(|n| n.ends_with(".rels") && covered(n)) {
        let Ok(b) = z.read(n) else { continue };
        for t in rel_targets(n, &b) {
            if ooxml_unsigned_part(&t) && !covered(&t) && !out.contains(&t) {
                out.push(t);
            }
        }
    }
    out
}

/// Every part a relationships part leads to inside the package, from
/// relationships at any depth (as readers find them).
fn all_targets(rels_name: &str, rels: &[u8]) -> Vec<String> {
    let Ok(doc) = c14n::parse(rels) else { return Vec::new() };
    let base = rels_name.rsplit_once("_rels/").map_or("", |(b, _)| b);
    let mut out = Vec::new();
    let mut todo: Vec<&c14n::Elem> = doc.nodes.iter().filter_map(|n| if let c14n::Node::Elem(e) = n { Some(e) } else { None }).collect();
    while let Some(e) = todo.pop() {
        if e.local() == "Relationship" && e.attr("TargetMode").is_none_or(|m| m == "Internal") {
            out.push(resolve(base, e.attr("Target").unwrap_or("")));
        }
        todo.extend(e.elems());
    }
    out
}

/// A target readers resolve as the verifier does: no escapes,
/// backslashes, empty or "." segments, nor ".." past the root.
fn plain_target(base: &str, target: &str) -> bool {
    let (mut depth, rest) = match target.strip_prefix('/') {
        Some(abs) => (0usize, abs),
        None => (base.split('/').filter(|s| !s.is_empty()).count(), target),
    };
    if rest.contains(['%', '\\', '?', '#']) {
        return false;
    }
    for seg in rest.split('/') {
        match seg {
            "" | "." => return false,
            ".." => match depth.checked_sub(1) {
                Some(d) => depth = d,
                None => return false,
            },
            _ => depth += 1,
        }
    }
    true
}

/// The part `target` names from a relationships part of folder `base`.
fn resolve(base: &str, target: &str) -> String {
    let joined = if let Some(abs) = target.strip_prefix('/') { abs.to_string() } else { format!("{base}{target}") };
    let mut path: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                path.pop();
            }
            s => path.push(s),
        }
    }
    crate::xmldsig::part_name(&path.join("/"))
}

/// What a relationships part points at in the package, but for the
/// properties' and signatures' own kinds of relationship.
fn rel_targets(rels_name: &str, rels: &[u8]) -> Vec<String> {
    let Ok(doc) = c14n::parse(rels) else { return Vec::new() };
    let Some(root) = doc.root() else { return Vec::new() };
    let base = rels_name.rsplit_once("_rels/").map_or("", |(b, _)| b);
    root.elems()
        .filter(|r| r.local() == "Relationship" && r.attr("TargetMode").is_none_or(|m| m == "Internal"))
        .filter(|r| {
            let ty = r.attr("Type").unwrap_or("");
            !["/extended-properties", "/core-properties", "/digital-signature/origin", "/digital-signature/signature"].iter().any(|k| ty.ends_with(k))
        })
        .map(|r| resolve(base, r.attr("Target").unwrap_or("")))
        .collect()
}

/// Whether a relationships part the signature doesn't cover could make
/// the document show what wasn't signed. Readers find a part by its id
/// and take neither the type nor the content type into account, so one
/// is uncovered when it leads to a part left out (but for the package's
/// own properties, thumbnail and signatures), names an id twice, or
/// leads to two signed parts of the same kind (two sheets, two pictures:
/// their targets could be swapped; a sheet in the place of the styles
/// shows nothing).
fn points_unsigned(z: &ZipReader, rels_name: &str, rels: &[u8], parts: &[String]) -> bool {
    let Ok(doc) = c14n::parse(rels) else { return true };
    let Some(root) = doc.root() else { return false };
    let Some((base, file)) = rels_name.rsplit_once("_rels/") else { return true };
    // Only what the format has: one root, its relationships, nothing in
    // them (readers also take relationships nested anywhere).
    let roots = doc.nodes.iter().filter(|n| matches!(n, c14n::Node::Elem(_))).count();
    if roots != 1 || root.local() != "Relationships" || root.elems().any(|r| r.local() != "Relationship" || r.elems().next().is_some()) {
        return true;
    }
    // The relationships of a part the signature doesn't cover (but the
    // package's own) lead wherever whoever wrote that part chose.
    let source = format!("{base}{}", file.strip_suffix(".rels").unwrap_or(file));
    if !base.is_empty() && !parts.iter().any(|p| p == &source) && root.elems().next().is_some() {
        return true;
    }
    let mut ids = std::collections::HashSet::new();
    let mut kinds = std::collections::HashSet::new();
    for r in root.elems().filter(|r| r.local() == "Relationship") {
        if !ids.insert(r.attr("Id").unwrap_or("")) {
            return true;
        }
        if r.attr("TargetMode").is_some_and(|m| m != "Internal") {
            continue;
        }
        let target = r.attr("Target").unwrap_or("");
        if !plain_target(base, target) {
            return true;
        }
        let name = resolve(base, target);
        if parts.iter().any(|p| p == &name) {
            if !kinds.insert(kind_class(part_kind(z, &name))) {
                return true;
            }
            continue;
        }
        // Only the package's own relationships lead to the parts never
        // signed, and only as what those parts are.
        let ty = r.attr("Type").unwrap_or("");
        let own = base.is_empty()
            && crate::xml_sign::ooxml_unsigned_part(&name)
            && !name.eq_ignore_ascii_case("[Content_Types].xml")
            && ["/extended-properties", "/core-properties", "/custom-properties", "/thumbnail", "/digital-signature/origin"].iter().any(|k| ty.ends_with(k));
        let thumbnail = base.is_empty() && ty.ends_with("/thumbnail") && name.to_ascii_lowercase().starts_with("docprops/");
        if !own && !thumbnail {
            return true;
        }
    }
    false
}

/// Kinds of part that can stand in for one another: any sheet for any
/// sheet, a footer for a header, any picture or other binary part.
fn kind_class(kind: String) -> String {
    match kind.as_str() {
        "<worksheet" | "<chartsheet" | "<dialogsheet" | "<macrosheet" => "sheet".into(),
        "<hdr" | "<ftr" => "hdrftr".into(),
        k if !k.starts_with('<') => "binary".into(),
        _ => kind,
    }
}

/// What a signed part is, from its own bytes: the name of an XML part's
/// root element, else its extension.
fn part_kind(z: &ZipReader, name: &str) -> String {
    let ext = || name.rsplit_once('.').map_or(String::new(), |(_, e)| e.to_ascii_lowercase());
    let Ok(b) = z.read(name) else { return ext() };
    // Pictures and other binary parts may hold a '<' anywhere.
    let text = b.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&b);
    if text.iter().find(|c| !c.is_ascii_whitespace()) != Some(&b'<') {
        return ext();
    }
    let mut i = 0;
    while let Some(k) = b[i..].iter().position(|&c| c == b'<') {
        let at = i + k + 1;
        match b.get(at) {
            Some(b'?' | b'!') => i = at,
            Some(_) => {
                let end = b[at..].iter().position(|c| c.is_ascii_whitespace() || matches!(c, b'>' | b'/')).map_or(b.len(), |e| at + e);
                let tag = String::from_utf8_lossy(&b[at..end]);
                return format!("<{}", tag.rsplit(':').next().unwrap_or(""));
            }
            None => break,
        }
        if i > 4096 {
            break;
        }
    }
    ext()
}

/// Whether a Word document shows its properties in its text: fields such
/// as TITLE or DOCPROPERTY, or content controls bound to them. Their
/// values then are part of what was signed.
pub(crate) fn shows_properties(z: &ZipReader) -> bool {
    const FIELDS: [&str; 15] = ["TITLE", "AUTHOR", "SUBJECT", "KEYWORDS", "COMMENTS", "DOCPROPERTY", "LASTSAVEDBY", "INFO", "TEMPLATE", "CREATEDATE", "SAVEDATE", "PRINTDATE", "EDITTIME", "REVNUM", "USERNAME"];
    // The stores of the core and extended properties.
    const STORES: [&str; 2] = ["6c3c8bc8-f283-45ae-878a-bab7291924a1", "6668398d-a668-4e3e-a5eb-62b293d839f1"];
    // Every XML part but the properties themselves, relationships and
    // signatures, whatever its name: headers and footers are found through
    // relationships, by any name, and parts by names in any case.
    let skip = |n: &str| {
        let l = n.to_ascii_lowercase();
        l.ends_with(".rels") || l.starts_with("docprops/") || l.starts_with("_xmlsignatures/") || l.starts_with("customxml/") || l == "[content_types].xml"
    };
    let names: Vec<String> = z.names().filter(|n| !skip(n)).map(str::to_string).collect();
    names.iter().any(|n| {
        let Ok(b) = z.read(n) else { return false };
        let utf16 = |b: &[u8], le: bool| String::from_utf16_lossy(&b.chunks_exact(2).map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) }).collect::<Vec<_>>());
        let text = match b.as_slice() {
            [0xFF, 0xFE, rest @ ..] => utf16(rest, true),
            [0xFE, 0xFF, rest @ ..] => utf16(rest, false),
            [0x3C, 0x00, ..] => utf16(&b, true),
            [0x00, 0x3C, ..] => utf16(&b, false),
            _ => String::from_utf8_lossy(&b).into_owned(),
        };
        if !text.trim_start_matches('\u{feff}').trim_start().starts_with('<') {
            return false;
        }
        let x = apex_io::xml::unescape(&text).to_ascii_lowercase();
        if x.contains("databinding") && (STORES.iter().any(|s| x.contains(s)) || x.contains("coreproperties") || x.contains(":properties[")) {
            return true;
        }
        // The fields' instructions, all together (Word may split one), in
        // elements and attributes of any prefix.
        let mut instr = String::new();
        for tag in x.split('<').skip(1) {
            let (head, after) = tag.split_once('>').unwrap_or((tag, ""));
            let name = head.split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("");
            if name.rsplit(':').next() == Some("instrtext") {
                instr.push_str(after);
            }
            let mut rest = head;
            while let Some(i) = rest.find("instr") {
                let before = rest[..i].chars().next_back();
                rest = &rest[i + 5..];
                if !before.is_some_and(|c| c == ':' || c.is_whitespace()) {
                    continue;
                }
                let Some(v) = rest.trim_start().strip_prefix('=') else { continue };
                let v = v.trim_start();
                let Some(q) = v.chars().next().filter(|&q| q == '"' || q == '\'') else { continue };
                let v = &v[1..];
                let end = v.find(q).unwrap_or(v.len());
                instr.push_str(&v[..end]);
                instr.push(' ');
            }
        }
        instr.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| FIELDS.iter().any(|f| f.eq_ignore_ascii_case(w)))
    })
}

/// Whether each entry of the ZIP's directory has the same name in its
/// local header, and no name is there twice: readers that go by one or
/// the other would otherwise see different content under one signature.
fn zip_consistent(d: &[u8]) -> bool {
    let u16_at = |i: usize| d.get(i..i + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize);
    let u32_at = |i: usize| d.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    let Some(eocd) = d.len().checked_sub(22).and_then(|last| (last.saturating_sub(65_535)..=last).rev().find(|&i| d[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])) else { return false };
    let (Some(mut count), Some(mut p)) = (u16_at(eocd + 10), u32_at(eocd + 16)) else { return false };
    // With Zip64 end records, their count and offset are the ones readers
    // go by (the short ones may say less).
    let u64_at = |i: usize| d.get(i..i + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap_or_default()));
    if eocd >= 20 && u32_at(eocd - 20) == Some(0x0706_4b50) {
        let Some(z) = u64_at(eocd - 12).and_then(|z| usize::try_from(z).ok()) else { return false };
        if u32_at(z) == Some(0x0606_4b50) {
            let (Some(c), Some(o)) = (u64_at(z + 32), u64_at(z + 48)) else { return false };
            let (Ok(c), Ok(o)) = (usize::try_from(c), usize::try_from(o)) else { return false };
            (count, p) = (c.min(d.len() / 46), o);
        }
    }
    let mut names = std::collections::HashSet::new();
    for _ in 0..count {
        let (Some(nl), Some(xl), Some(cl), Some(off)) = (u16_at(p + 28), u16_at(p + 30), u16_at(p + 32), u32_at(p + 42)) else { return false };
        let Some(name) = d.get(p + 46..p + 46 + nl) else { return false };
        let local = u16_at(off + 26).and_then(|ln| d.get(off + 30..off + 30 + ln));
        if u32_at(off) != Some(0x0403_4b50) || local != Some(name) || !names.insert(name) {
            return false;
        }
        p += 46 + nl + xl + cl;
    }
    true
}

/// The signatures of a signatures file, each with the path down to it:
/// the root, or the root's children.
pub(crate) fn signature_paths(doc: &c14n::Doc) -> Vec<Vec<&Elem>> {
    let Some(root) = doc.root() else { return Vec::new() };
    if root.local() == "Signature" {
        return vec![vec![root]];
    }
    // More than a document is ever given would only cost time to check.
    root.elems().filter(|e| e.local() == "Signature").take(64).map(|e| vec![root, e]).collect()
}

pub fn verify(data: &[u8]) -> Vec<Signature> {
    let Ok(z) = ZipReader::new(data) else { return Vec::new() };
    let consistent = zip_consistent(data);
    let resolve = |name: &str| z.read(name).ok();
    let mut out = Vec::new();
    let mut sources: Vec<(Kind, Vec<u8>)> = Vec::new();
    if let Ok(x) = z.read(ODF_SIGNATURES) {
        sources.push((Kind::Odf, x));
    }
    for p in ooxml_parts(&z).into_iter().take(64) {
        if let Ok(x) = z.read(&p) {
            sources.push((Kind::Ooxml, x));
        }
    }
    for (kind, xml) in sources {
        let doc = match c14n::parse(&xml) {
            Ok(d) => d,
            Err(e) => {
                let mut s = Signature::new(kind);
                s.note = Some(e.to_string());
                out.push(s);
                continue;
            }
        };
        for path in signature_paths(&doc) {
            let c = xmldsig::check(&doc, &path, &resolve);
            let mut s = Signature::new(kind);
            s.format = if c.xades { "XAdES".into() } else { "XML-DSig".into() };
            s.cert = c.cert;
            s.certs = c.certs;
            s.time = c.time;
            s.local_time = c.local_time;
            s.intact = c.intact;
            s.valid = c.valid;
            s.reason = c.comment;
            s.note = c.note;
            s.changed_parts = c.changed;
            if !consistent {
                s.intact = Check::Failed;
                s.note = Some("the package's directory doesn't match its entries".into());
            }
            if kind == Kind::Odf && s.intact == Check::Ok {
                // Streams added after signing.
                s.unsigned_parts = z.names().filter(|n| odf_needs_signing(n) && !c.parts.iter().any(|p| p == n)).map(str::to_string).collect();
                s.changed_after = !s.unsigned_parts.is_empty();
            }
            if kind == Kind::Ooxml && s.intact == Check::Ok {
                // Parts the signature leaves out: a signature over the
                // properties alone must not vouch for the document.
                s.unsigned_parts = ooxml_uncovered(&z, &c.parts);
                s.unsigned_parts.extend(c.unselected.iter().cloned());
                // The properties go unsigned, unless the body shows them.
                if shows_properties(&z) {
                    s.unsigned_parts.extend(["docProps/core.xml", "docProps/app.xml"].into_iter().filter(|n| z.contains(n) && !c.parts.iter().any(|p| p == n)).map(str::to_string));
                }
                s.changed_after = !s.unsigned_parts.is_empty();
            }
            // XAdES-T, -XL and -A.
            let info = crate::xades::info(&doc, &path, &resolve);
            if let Some((time, tsa, chain)) = info.stamp {
                s.time = Some(time);
                s.local_time = false;
                s.timestamped = true;
                s.tsa = tsa;
                s.tsa_chain = chain;
            }
            let mut s = s.finish();
            s.judge(&info.kept, info.archive.as_ref());
            out.push(s);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn properties_shown_in_the_text_are_found() {
        let zip = |name: &str, body: &str| {
            let mut w = apex_io::zip::ZipWriter::new();
            w.add(name, body.as_bytes(), true);
            w.finish()
        };
        let shows_in = |name: &str, body: &str| shows_properties(&ZipReader::new(&zip(name, body)).unwrap());
        let shows = |body: &str| shows_in("word/document.xml", body);
        // Any prefix, quote or escape, and parts by any name.
        assert!(shows(r#"<x:r><x:instrText>&#84;ITLE</x:instrText></x:r>"#));
        assert!(shows(r#"<w:fldSimple w:instr=' TITLE '/>"#));
        assert!(shows_in("hdr/h.bin", r#"<w:hdr><w:fldSimple w:instr="AUTHOR"/></w:hdr>"#));
        assert!(shows_in("Word/document.xml", r#"<w:fldSimple w:instr="AUTHOR"/>"#));
        assert!(!shows_in("word/media/a.png", "PNG TITLE"));
        assert!(shows(r#"<w:r><w:instrText xml:space="preserve"> DOC</w:instrText></w:r><w:r><w:instrText>PROPERTY Company </w:instrText></w:r>"#));
        assert!(shows(r#"<w:fldSimple w:instr=" TITLE "/>"#));
        assert!(shows(r#"<w:dataBinding w:xpath="/ns1:coreProperties[1]/ns0:title[1]" w:storeItemID="{6C3C8BC8-F283-45AE-878A-BAB7291924A1}"/>"#));
        assert!(!shows(r#"<w:t>TITLE</w:t><w:fldSimple w:instr=" PAGE "/>"#));
    }

    #[test]
    fn zip64_directories_are_checked_in_full() {
        let mut w = apex_io::zip::ZipWriter::new();
        for i in 0..70_000 {
            w.add(&format!("f{i}"), b"x", false);
        }
        let mut bytes = w.finish();
        assert!(zip_consistent(&bytes));
        // A name twice past the short count is still seen.
        let last = bytes.windows(6).rposition(|w| w == b"f69999").unwrap();
        bytes[last..last + 6].copy_from_slice(b"f69998");
        assert!(!zip_consistent(&bytes));
    }
}
