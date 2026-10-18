//! XML Signature (XMLDSig, with XAdES when present) as OOXML packages
//! (`_xmlsignatures/`) and ODF packages (`META-INF/documentsignatures.xml`)
//! use it: the signed parts' digests, the signature value and the signing
//! time.

use crate::c14n::{self, Doc, Elem, Method};
use crate::crypto::{self, Hash};
use crate::x509::Cert;
use crate::{Check, base64};

const RELS_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";

/// What checking one `<Signature>` found.
pub struct Checked {
    pub cert: Option<Cert>,
    pub certs: Vec<Cert>,
    pub time: Option<i64>,
    /// The time came without a zone (the signer's local time).
    pub local_time: bool,
    /// The signed parts and elements are unchanged.
    pub intact: Check,
    /// The signature value is right for the certificate's key.
    pub valid: Check,
    pub xades: bool,
    /// Package parts the signature covers (without a leading slash).
    pub parts: Vec<String>,
    /// The parts or elements whose digest no longer matches.
    pub changed: Vec<String>,
    /// Relationships ("part#Id") of signed relationships parts that the
    /// signature's transforms leave out (added later: readers find parts
    /// by type, not by Id), but for the kinds never signed.
    pub unselected: Vec<String>,
    /// The signer's comment (OOXML "purpose").
    pub comment: Option<String>,
    /// Why something couldn't be checked.
    pub note: Option<String>,
}

/// A value going through a reference's transforms.
enum Data<'a> {
    Octets(Vec<u8>),
    Subtree(Vec<&'a Elem>),
}

/// A package part name from a reference URI: no query, no leading
/// slash, percent escapes decoded.
pub fn part_name(uri: &str) -> String {
    let path = uri.split('?').next().unwrap_or(uri).trim_start_matches('/');
    let b = path.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(v) = path.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn method_of(e: &Elem, uri: &str) -> Option<Method> {
    let mut m = Method::from_uri(uri)?;
    if let Some(list) = e.child("InclusiveNamespaces").and_then(|i| i.attr("PrefixList")) {
        m.prefixes = list.split_whitespace().map(str::to_string).collect();
    }
    Some(m)
}

/// OPC's relationship transform: the chosen relationships, sorted by Id,
/// written canonically. `xmlsec`: as the xmlsec library (LibreOffice)
/// does it, which writes each text node among the relationships (the
/// white space some writers put there) as an empty `<text>` first; Office
/// and the standard leave them out.
pub fn relationship_transform(rels: &[u8], t: &Elem, xmlsec: bool) -> Result<Vec<u8>, String> {
    let mut ids = Vec::new();
    let mut types = Vec::new();
    for e in t.elems() {
        match e.local() {
            "RelationshipReference" => ids.extend(e.attr("SourceId").map(str::to_string)),
            "RelationshipsGroupReference" => types.extend(e.attr("SourceType").map(str::to_string)),
            _ => {}
        }
    }
    // Relationships are plain: no instructions past the declaration nor
    // declarations, which readers might end elsewhere than this one.
    let body = rels.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(rels).trim_ascii_start();
    let body = if body.starts_with(b"<?xml") { body.windows(2).position(|w| w == b"?>").map_or(body, |i| &body[i + 2..]) } else { body };
    if body.windows(2).any(|w| w == b"<?") || body.windows(2).enumerate().any(|(i, w)| w == b"<!" && !body[i..].starts_with(b"<!--")) {
        return Err("the relationships hold instructions or declarations".into());
    }
    let doc = c14n::parse(rels).map_err(|e| e.to_string())?;
    let root = doc.root().ok_or("empty relationships")?;
    // Only relationships, directly under the root: readers that take one
    // at any depth would see one this transform leaves out.
    if root.elems().any(|r| r.local() != "Relationship" || r.elems().next().is_some()) {
        return Err("the relationships hold other elements".into());
    }
    // An Id twice: readers keep one or the other.
    let mut all_ids: Vec<&str> = root.elems().filter_map(|r| r.attr("Id")).collect();
    all_ids.sort_unstable();
    if all_ids.windows(2).any(|w| w[0] == w[1]) {
        return Err("a relationship Id is there twice".into());
    }
    // A part of the document among the signatures' (which no signature
    // covers) would be shown as signed.
    if root.elems().any(|r| r.attr("Target").is_some_and(|t| t.to_ascii_lowercase().contains("_xmlsignatures/")) && !r.attr("Type").is_some_and(|t| t.ends_with("/digital-signature/origin") || t.ends_with("/digital-signature/signature"))) {
        return Err("a relationship points into the signatures".into());
    }
    // A relationship saying one of these twice (once with a prefix) could
    // be read one way here and another when the document is opened.
    for r in root.elems() {
        for k in ["Id", "Target", "TargetMode", "Type"] {
            if r.attrs.iter().filter(|(n, _)| n.rsplit(':').next() == Some(k)).count() > 1 {
                return Err(format!("a relationship has {k} twice"));
            }
        }
    }
    let mut picked: Vec<&Elem> = root.elems().filter(|r| r.local() == "Relationship" && (r.attr("Id").is_some_and(|i| ids.iter().any(|x| x == i)) || r.attr("Type").is_some_and(|ty| types.iter().any(|x| x == ty)))).collect();
    picked.sort_by(|a, b| a.attr("Id").cmp(&b.attr("Id")));
    let mut out = Elem { name: "Relationships".into(), ns: vec![(String::new(), RELS_NS.into())], ..Default::default() };
    if xmlsec {
        for _ in root.children.iter().filter(|n| matches!(n, c14n::Node::Text(_))) {
            out.children.push(c14n::Node::Elem(Elem { name: "text".into(), ..Default::default() }));
        }
    }
    for r in picked {
        let mut attrs = Vec::new();
        for k in ["Id", "Target", "TargetMode", "Type"] {
            let v = r.attr(k).map(str::to_string).or_else(|| (k == "TargetMode").then(|| "Internal".to_string()));
            if let Some(v) = v {
                attrs.push((k.to_string(), v));
            }
        }
        out.children.push(c14n::Node::Elem(Elem { name: "Relationship".into(), attrs, ..Default::default() }));
    }
    Ok(c14n::doc(&Doc::new(vec![c14n::Node::Elem(out)]), &Method::default()))
}

/// The relationships a reference's relationship transform leaves out,
/// as "part#Id", but for the kinds signers leave out (properties, the
/// thumbnail, the signatures).
fn unselected(r: &Elem, resolve: &dyn Fn(&str) -> Option<Vec<u8>>) -> Vec<String> {
    let Some(t) = r.child("Transforms").and_then(|ts| ts.elems().find(|t| t.attr("Algorithm").is_some_and(|a| a.ends_with("/RelationshipTransform")))) else { return Vec::new() };
    let name = part_name(r.attr("URI").unwrap_or(""));
    let Some(Ok(doc)) = resolve(&name).map(|b| c14n::parse(&b)) else { return Vec::new() };
    let Some(root) = doc.root() else { return Vec::new() };
    let ids: Vec<&str> = t.elems().filter(|e| e.local() == "RelationshipReference").filter_map(|e| e.attr("SourceId")).collect();
    let types: Vec<&str> = t.elems().filter(|e| e.local() == "RelationshipsGroupReference").filter_map(|e| e.attr("SourceType")).collect();
    let never_signed = |ty: &str| {
        ["/extended-properties", "/core-properties", "/custom-properties", "/thumbnail", "/digital-signature/origin", "/digital-signature/signature"].iter().any(|k| ty.ends_with(k))
    };
    root.elems()
        .filter(|e| e.local() == "Relationship")
        .filter(|e| !e.attr("Id").is_some_and(|i| ids.contains(&i)) && !e.attr("Type").is_some_and(|ty| types.contains(&ty) || never_signed(ty)))
        .map(|e| format!("{name}#{}", e.attr("Id").unwrap_or("")))
        .collect()
}

/// The digest a reference has now, and the one it had when signed.
fn reference(r: &Elem, doc: &Doc, resolve: &dyn Fn(&str) -> Option<Vec<u8>>, parts: &mut Vec<String>, xmlsec: bool) -> Result<(Vec<u8>, Vec<u8>), String> {
    let (hash, bytes, expected) = reference_data(r, doc, resolve, parts, xmlsec)?;
    Ok((hash.digest(&bytes), expected))
}

/// What a reference digests (after its transforms), with the digest
/// algorithm and the digest it had when signed.
pub fn reference_data(r: &Elem, doc: &Doc, resolve: &dyn Fn(&str) -> Option<Vec<u8>>, parts: &mut Vec<String>, xmlsec: bool) -> Result<(Hash, Vec<u8>, Vec<u8>), String> {
    let uri = r.attr("URI").unwrap_or("");
    let hash = r.child("DigestMethod").and_then(|d| d.attr("Algorithm")).and_then(Hash::from_uri).ok_or("unknown digest")?;
    let expected = base64(&r.child("DigestValue").map(|d| d.text()).unwrap_or_default()).ok_or("bad digest value")?;
    let mut data = if let Some(id) = uri.strip_prefix('#') {
        Data::Subtree(doc.find_id(id).ok_or_else(|| format!("#{id} not found"))?)
    } else if uri.is_empty() {
        return Err("whole-document references are not supported".into());
    } else {
        let name = part_name(uri);
        let bytes = resolve(&name).ok_or_else(|| format!("{name} is missing"))?;
        parts.push(name);
        Data::Octets(bytes)
    };
    if let Some(ts) = r.child("Transforms") {
        for t in ts.elems().filter(|t| t.local() == "Transform") {
            let alg = t.attr("Algorithm").unwrap_or("");
            if let Some(m) = method_of(t, alg) {
                data = Data::Octets(match data {
                    Data::Octets(b) => c14n::doc(&c14n::parse(&b).map_err(|e| e.to_string())?, &m),
                    Data::Subtree(p) => c14n::subtree(&p, &m),
                });
            } else if alg.ends_with("/RelationshipTransform") {
                let Data::Octets(b) = data else { return Err("relationship transform of an element".into()) };
                data = Data::Octets(relationship_transform(&b, t, xmlsec)?);
            } else {
                return Err(format!("unsupported transform {alg}"));
            }
        }
    }
    let bytes = match data {
        Data::Octets(b) => b,
        Data::Subtree(p) => c14n::subtree(&p, &Method::default()),
    };
    Ok((hash, bytes, expected))
}

/// References a signature may make to its own document's elements, and
/// in all (one per part of a package).
const MAX_OWN_REFS: usize = 64;
const MAX_REFS: usize = 20_000;

/// Checks the `<Signature>` at the end of `path` in `doc`; `resolve`
/// reads a package part.
pub fn check(doc: &Doc, path: &[&Elem], resolve: &dyn Fn(&str) -> Option<Vec<u8>>) -> Checked {
    let sig = path[path.len() - 1];
    let mut out = Checked { cert: None, certs: Vec::new(), time: None, local_time: false, intact: Check::Unknown, valid: Check::Unknown, xades: false, parts: Vec::new(), changed: Vec::new(), unselected: Vec::new(), comment: None, note: None };
    // Certificates of the key info.
    let mut x509 = Vec::new();
    if let Some(ki) = sig.child("KeyInfo") {
        ki.find_all("X509Certificate", &mut x509);
        if x509.is_empty() && ki.find("PGPData").is_some() {
            out.note = Some("OpenPGP signatures are not supported".into());
        }
    }
    // XAdES keeps the rest of the chain among its unsigned properties.
    sig.find_all("EncapsulatedX509Certificate", &mut x509);
    for c in x509.iter().take(4 * crate::MAX_CERTS).filter_map(|c| base64(&c.text())).filter_map(|d| Cert::from_der(&d).ok()) {
        if out.certs.len() < crate::MAX_CERTS && !out.certs.iter().any(|x| x.der == c.der) {
            out.certs.push(c);
        }
    }
    let Some(si) = sig.child("SignedInfo") else {
        out.note = Some("no SignedInfo".into());
        return out;
    };
    // Each reference within the document is looked for through all of it:
    // more than any signature has would only cost time.
    let own = |r: &&Elem| r.local() == "Reference" && r.attr("URI").is_some_and(|u| u.starts_with('#'));
    if si.elems().filter(own).count() > MAX_OWN_REFS {
        out.intact = Check::Failed;
        out.note = Some("too many references".into());
        return out;
    }
    // What SignedInfo points at within the signature (its objects): only
    // there are the time, the comment and the manifests signed; anyone
    // may add more elsewhere in the signature.
    let signed: Vec<&Elem> = si
        .elems()
        .filter(|e| e.local() == "Reference")
        .filter_map(|r| r.attr("URI")?.strip_prefix('#'))
        .filter_map(|id| doc.find_id(id)?.last().copied())
        .collect();
    let find = |name: &str| signed.iter().find_map(|e| if e.local() == name { Some(*e) } else { e.find(name) });
    // Signing time: XAdES, OOXML's, or ODF's dc:date.
    out.xades = sig.find("QualifyingProperties").is_some();
    let time = find("SigningTime").map(|e| e.text())
        .or_else(|| find("SignatureTime").and_then(|t| t.child("Value")).map(|e| e.text()))
        .or_else(|| find("date").map(|e| e.text()));
    if let Some(t) = time {
        out.time = crate::time::parse_iso(&t);
        out.local_time = !crate::time::has_zone(&t);
    }
    // The signer's comment: OOXML's, or ODF's dc:description.
    out.comment = find("SignatureComments").or_else(|| find("description")).map(|e| e.text()).filter(|c| !c.trim().is_empty());
    // The references, and those of signed manifests (OOXML signs its
    // parts through one).
    let mut refs: Vec<&Elem> = si.elems().filter(|e| e.local() == "Reference").collect();
    let mut manifests = Vec::new();
    for e in &signed {
        if e.local() == "Manifest" {
            manifests.push(*e);
        }
        e.find_all("Manifest", &mut manifests);
    }
    manifests.dedup_by(|a, b| std::ptr::eq(*a, *b));
    for m in manifests {
        refs.extend(m.elems().filter(|e| e.local() == "Reference"));
    }
    if refs.iter().copied().filter(own).count() > MAX_OWN_REFS || refs.len() > MAX_REFS {
        out.intact = Check::Failed;
        out.note = Some("too many references".into());
        return out;
    }
    let mut intact = Check::Ok;
    for r in refs {
        out.unselected.extend(unselected(r, resolve));
        match reference(r, doc, resolve, &mut out.parts, false) {
            Ok((got, want)) if got == want => {}
            // Relationships digested as LibreOffice does.
            Ok(_) if reference(r, doc, resolve, &mut Vec::new(), true).is_ok_and(|(got, want)| got == want) => {}
            Ok(_) => {
                intact = Check::Failed;
                out.changed.push(r.attr("URI").map(|u| part_name(u.trim_start_matches('#'))).unwrap_or_default());
            }
            Err(e) => {
                if intact == Check::Ok {
                    intact = Check::Unknown;
                }
                out.note.get_or_insert(e);
            }
        }
    }
    out.intact = intact;
    // The signature over the canonical SignedInfo.
    let method = si.child("CanonicalizationMethod").and_then(|c| c.attr("Algorithm").and_then(|a| method_of(c, a)));
    let scheme = si.child("SignatureMethod").and_then(|s| s.attr("Algorithm")).and_then(crypto::scheme_of_uri);
    let value = sig.child("SignatureValue").and_then(|v| base64(&v.text()));
    match (method, scheme, value) {
        (Some(m), Some((scheme, hash)), Some(value)) => {
            let mut si_path: Vec<&Elem> = path.to_vec();
            si_path.push(si);
            let digest = hash.digest(&c14n::subtree(&si_path, &m));
            let signer = out.certs.iter().find(|c| c.public_key().is_ok_and(|k| k.verify(scheme, hash, &digest, &value)));
            match signer {
                Some(c) => {
                    out.cert = Some(c.clone());
                    out.valid = Check::Ok;
                }
                None => {
                    out.cert = out.certs.iter().find(|c| !c.ca).or(out.certs.first()).cloned();
                    out.valid = if out.cert.is_some() { Check::Failed } else { Check::Unknown };
                }
            }
        }
        _ => {
            out.note.get_or_insert("unsupported signature method".into());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_names() {
        assert_eq!(part_name("/word/document.xml?ContentType=application/vnd.openxmlformats"), "word/document.xml");
        assert_eq!(part_name("Pictures/a%20b.png"), "Pictures/a b.png");
    }

    #[test]
    fn relationship_transform_output() {
        let rels = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId3" Type="http://t/b" Target="b.xml"/><Relationship Id="rId1" Type="http://t/a" Target="a.xml"/><Relationship Id="rId2" Type="http://t/h" Target="http://x" TargetMode="External"/></Relationships>"#;
        let t = c14n::parse(br#"<Transform xmlns:m="urn:m"><m:RelationshipReference SourceId="rId3"/><m:RelationshipReference SourceId="rId1"/><m:RelationshipsGroupReference SourceType="http://t/h"/></Transform>"#).unwrap();
        let out = relationship_transform(rels, t.root().unwrap(), false).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Target=\"a.xml\" TargetMode=\"Internal\" Type=\"http://t/a\"></Relationship><Relationship Id=\"rId2\" Target=\"http://x\" TargetMode=\"External\" Type=\"http://t/h\"></Relationship><Relationship Id=\"rId3\" Target=\"b.xml\" TargetMode=\"Internal\" Type=\"http://t/b\"></Relationship></Relationships>");
        // xmlsec's way with white space (checked against xmlsec 1.3): a
        // `<text>` per text node, whichever relationships are chosen.
        let rels = b"<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n <Relationship Id=\"rId2\" Type=\"http://t/b\" Target=\"b.xml\"/>\n <Relationship Id=\"rId1\" Type=\"http://t/a\" Target=\"a.xml\"/>\n</Relationships>";
        let t = c14n::parse(br#"<Transform xmlns:m="urn:m"><m:RelationshipReference SourceId="rId1"/></Transform>"#).unwrap();
        let out = relationship_transform(rels, t.root().unwrap(), true).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><text></text><text></text><text></text><Relationship Id=\"rId1\" Target=\"a.xml\" TargetMode=\"Internal\" Type=\"http://t/a\"></Relationship></Relationships>");
    }
}
