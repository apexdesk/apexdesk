//! XAdES-T, -XL and -A in packages: the unsigned properties added to a
//! signature after it was made (a time-stamp of the signature value, the
//! references to and the values of the chain and the revocation answers,
//! and archive time-stamps over everything), and reading them back.
//!
//! The file is changed as text: each property goes in before the end of
//! `<UnsignedSignatureProperties>` with its own namespace declarations, so
//! nothing else in it moves and the signatures stay valid.

use apex_io::zip::ZipReader;

use crate::c14n::{self, Doc, Elem, Method};
use crate::crypto::Hash;
use crate::ltv::{self, Level, Validation};
use crate::net::Net;
use crate::package::{ODF_SIGNATURES, ooxml_parts, signature_paths};
use crate::revocation::{self, Responder};
use crate::x509::{Cert, Name};
use crate::{Archive, Check, Error, base64, base64_encode, cms, xmldsig};

const XADES_NS: &str = "http://uri.etsi.org/01903/v1.3.2#";
const XADES141_NS: &str = "http://uri.etsi.org/01903/v1.4.1#";
const DSIG_NS: &str = "http://www.w3.org/2000/09/xmldsig#";
const C14N: &str = "http://www.w3.org/TR/2001/REC-xml-c14n-20010315";
const SHA256: &str = "http://www.w3.org/2001/04/xmlenc#sha256";

// --- the text of the file ------------------------------------------------------------

/// An element of the file as text: where its tags are.
struct Span {
    local: String,
    qname: String,
    parent: Option<usize>,
    /// Just past the start tag.
    open_end: usize,
    /// Where the end tag starts (`None`: `<a/>`).
    close: Option<usize>,
}

/// The elements of `xml`, in document order.
fn spans(xml: &str) -> Result<Vec<Span>, Error> {
    let bad = || Error::new("malformed signature XML");
    let b = xml.as_bytes();
    let mut out: Vec<Span> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut i = 0;
    while let Some(off) = xml[i..].find('<') {
        let s = i + off;
        let rest = &xml[s..];
        let skip = |end: &str| rest.find(end).map(|p| s + p + end.len()).ok_or_else(bad);
        if rest.starts_with("<!--") {
            i = skip("-->")?;
            continue;
        }
        if rest.starts_with("<![CDATA[") {
            i = skip("]]>")?;
            continue;
        }
        if rest.starts_with("<?") {
            i = skip("?>")?;
            continue;
        }
        if rest.starts_with("<!") {
            i = skip(">")?;
            continue;
        }
        // The end of the tag, past quoted values.
        let mut e = s + 1;
        let mut quote = 0u8;
        while e < b.len() {
            let c = b[e];
            if quote != 0 {
                if c == quote {
                    quote = 0;
                }
            } else if c == b'"' || c == b'\'' {
                quote = c;
            } else if c == b'>' {
                break;
            }
            e += 1;
        }
        if e >= b.len() {
            return Err(bad());
        }
        if rest.starts_with("</") {
            let top = stack.pop().ok_or_else(bad)?;
            out[top].close = Some(s);
        } else {
            let name_end = xml[s + 1..e].find(|c: char| c.is_whitespace() || c == '/').map_or(e, |p| s + 1 + p);
            let qname = xml[s + 1..name_end].to_string();
            let local = qname.rsplit(':').next().unwrap_or("").to_string();
            let empty = b[e - 1] == b'/';
            out.push(Span { local, qname, parent: stack.last().copied(), open_end: e + 1, close: None });
            if !empty {
                stack.push(out.len() - 1);
            }
        }
        i = e + 1;
    }
    if !stack.is_empty() {
        return Err(bad());
    }
    Ok(out)
}

fn children(sp: &[Span], of: usize) -> impl Iterator<Item = usize> + '_ {
    (of + 1..sp.len()).filter(move |&i| sp[i].parent == Some(of))
}

fn child(sp: &[Span], of: usize, local: &str) -> Option<usize> {
    children(sp, of).find(|&i| sp[i].local == local)
}

/// The signatures as [`signature_paths`] finds them.
fn signature_spans(sp: &[Span]) -> Vec<usize> {
    let Some(root) = sp.iter().position(|s| s.parent.is_none()) else { return Vec::new() };
    if sp[root].local == "Signature" {
        return vec![root];
    }
    children(sp, root).filter(|&i| sp[i].local == "Signature").collect()
}

/// Puts `text` at the end (`last`) or the start of element `e`'s content.
fn insert(xml: &str, e: &Span, text: &str, last: bool) -> String {
    match e.close {
        Some(close) => {
            let at = if last { close } else { e.open_end };
            format!("{}{text}{}", &xml[..at], &xml[at..])
        }
        None => {
            // `<a/>` becomes `<a>text</a>`.
            let slash = xml[..e.open_end].rfind('/').unwrap_or(e.open_end - 1);
            format!("{}>{text}</{}>{}", &xml[..slash], e.qname, &xml[e.open_end..])
        }
    }
}

/// The `QualifyingProperties` of the `k`-th signature (as a span).
fn qp_span(sp: &[Span], k: usize) -> Option<usize> {
    let sig = *signature_spans(sp).get(k)?;
    children(sp, sig).filter(|&o| sp[o].local == "Object").find_map(|o| child(sp, o, "QualifyingProperties"))
}

/// Makes sure the `k`-th signature has `UnsignedSignatureProperties`.
fn ensure_usp(xml: String, k: usize) -> Result<String, Error> {
    let sp = spans(&xml)?;
    let qp = qp_span(&sp, k).ok_or_else(|| Error::new("only XAdES signatures can be extended"))?;
    match child(&sp, qp, "UnsignedProperties") {
        None => Ok(insert(&xml, &sp[qp], &format!("<xd:UnsignedProperties xmlns:xd=\"{XADES_NS}\"><xd:UnsignedSignatureProperties></xd:UnsignedSignatureProperties></xd:UnsignedProperties>"), true)),
        Some(up) if child(&sp, up, "UnsignedSignatureProperties").is_none() => Ok(insert(&xml, &sp[up], &format!("<xd:UnsignedSignatureProperties xmlns:xd=\"{XADES_NS}\"></xd:UnsignedSignatureProperties>"), false)),
        Some(_) => Ok(xml),
    }
}

/// Adds `text` at the end of the `k`-th signature's unsigned signature
/// properties, or of its child `inside` when given and present.
fn add_usp(xml: &str, k: usize, inside: Option<&str>, text: &str) -> Result<String, Error> {
    let sp = spans(xml)?;
    let usp = qp_span(&sp, k).and_then(|qp| child(&sp, qp, "UnsignedProperties")).and_then(|up| child(&sp, up, "UnsignedSignatureProperties")).ok_or_else(|| Error::new("no unsigned properties"))?;
    let at = inside.and_then(|n| child(&sp, usp, n)).unwrap_or(usp);
    Ok(insert(xml, &sp[at], text, true))
}

// --- the parsed file -----------------------------------------------------------------

/// The path down to the unsigned signature properties of a signature.
fn usp_path<'a>(sig: &[&'a Elem]) -> Option<Vec<&'a Elem>> {
    let s = *sig.last()?;
    let (o, qp) = s.elems().filter(|o| o.local() == "Object").find_map(|o| o.child("QualifyingProperties").map(|q| (o, q)))?;
    let up = qp.child("UnsignedProperties")?;
    let usp = up.child("UnsignedSignatureProperties")?;
    let mut p = sig.to_vec();
    p.extend([o, qp, up, usp]);
    Some(p)
}

fn with<'a>(path: &[&'a Elem], e: &'a Elem) -> Vec<&'a Elem> {
    let mut p = path.to_vec();
    p.push(e);
    p
}

/// The canonical form of `e` (a child of the end of `path`).
fn canon(path: &[&Elem], e: &Elem, m: &Method) -> Vec<u8> {
    c14n::subtree(&with(path, e), m)
}

/// The canonicalization a time-stamp element names (inclusive C14N by
/// default).
fn method_of(ts: &Elem) -> Method {
    ts.child("CanonicalizationMethod").and_then(|c| c.attr("Algorithm")).and_then(Method::from_uri).unwrap_or_default()
}

/// The token of a time-stamp element.
fn token(ts: &Elem) -> Option<Vec<u8>> {
    ts.child("EncapsulatedTimeStamp").and_then(|e| base64(&e.text()))
}

/// The validation data among `elems` (certificate and revocation values).
fn kept_in<'a>(elems: impl Iterator<Item = &'a Elem>) -> Validation {
    let mut v = Validation::default();
    for e in elems {
        let mut found = Vec::new();
        e.find_all("EncapsulatedX509Certificate", &mut found);
        for c in found.iter().filter_map(|c| base64(&c.text())).filter_map(|d| Cert::from_der(&d).ok()) {
            v.add_cert(&c);
        }
        let mut found = Vec::new();
        e.find_all("EncapsulatedCRLValue", &mut found);
        for d in found.iter().filter_map(|c| base64(&c.text())) {
            v.add_crl(d);
        }
        let mut found = Vec::new();
        e.find_all("EncapsulatedOCSPValue", &mut found);
        for d in found.iter().filter_map(|c| base64(&c.text())) {
            v.add_ocsp(d);
        }
    }
    v
}

fn is_timestamp(local: &str) -> bool {
    matches!(local, "SignatureTimeStamp" | "SigAndRefsTimeStamp" | "RefsOnlyTimeStamp" | "ArchiveTimeStamp")
}

/// What an archive time-stamp placed after the first `before` unsigned
/// properties covers: the referenced data, SignedInfo, SignatureValue and
/// KeyInfo, those properties and the other objects.
fn archive_input(doc: &Doc, sig: &[&Elem], usp: &[&Elem], before: usize, m: &Method, resolve: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<Vec<u8>, Error> {
    let s = *sig.last().ok_or_else(|| Error::new("no signature"))?;
    let si = s.child("SignedInfo").ok_or_else(|| Error::new("no SignedInfo"))?;
    let mut out = Vec::new();
    for r in si.elems().filter(|e| e.local() == "Reference") {
        // As the signature was made (relationships the xmlsec way or not).
        let mut data = None;
        for xmlsec in [false, true] {
            let (hash, bytes, want) = xmldsig::reference_data(r, doc, resolve, &mut Vec::new(), xmlsec).map_err(Error::new)?;
            if hash.digest(&bytes) == want {
                data = Some(bytes);
                break;
            }
        }
        out.extend(data.ok_or_else(|| Error::new("the signed content changed"))?);
    }
    for name in ["SignedInfo", "SignatureValue", "KeyInfo"] {
        if let Some(e) = s.child(name) {
            out.extend(canon(sig, e, m));
        }
    }
    let props = usp.last().ok_or_else(|| Error::new("no unsigned properties"))?;
    for e in props.elems().take(before) {
        out.extend(canon(usp, e, m));
    }
    for o in s.elems().filter(|o| o.local() == "Object" && o.child("QualifyingProperties").is_none()) {
        out.extend(canon(sig, o, m));
    }
    Ok(out)
}

/// What a signature's unsigned properties say, for its check.
pub(crate) struct Info {
    /// The signature time-stamp that holds: its time, authority and the
    /// authority's chain.
    pub stamp: Option<(i64, Option<Cert>, Vec<Cert>)>,
    pub kept: Validation,
    pub archive: Option<Archive>,
}

/// Whether a time-stamp's authority is trusted to time-stamp.
fn trusted_stamp(st: &cms::Stamp, kept: &Validation) -> bool {
    let Some(tsa) = &st.tsa else { return false };
    let mut pool = st.certs.clone();
    pool.extend(kept.certs.iter().cloned());
    crate::trust::fit_for_timestamping(tsa) && crate::trust::check(tsa, &pool, st.time).1.is_trusted()
}

/// Reads the unsigned properties of the signature at the end of `sig`.
pub(crate) fn info(doc: &Doc, sig: &[&Elem], resolve: &dyn Fn(&str) -> Option<Vec<u8>>) -> Info {
    let mut info = Info { stamp: None, kept: Validation::default(), archive: None };
    let Some(usp) = usp_path(sig) else { return info };
    let props = usp[usp.len() - 1];
    info.kept = kept_in(props.elems());
    let s = sig[sig.len() - 1];
    let chain_of = |st: &cms::Stamp, kept: &Validation| -> Vec<Cert> {
        let Some(tsa) = &st.tsa else { return Vec::new() };
        let mut pool = st.certs.clone();
        pool.extend(kept.certs.iter().cloned());
        crate::trust::check(tsa, &pool, st.time).0
    };
    // Each archive stamp covers all before it: a few dozen stamps are more
    // than any signature gathers, many would take quadratic time.
    let mut stamps = 0;
    for (i, e) in props.elems().enumerate() {
        if is_timestamp(e.local()) {
            stamps += 1;
            if stamps > 32 {
                break;
            }
        }
        let Some(st) = token(e).and_then(|t| cms::stamp(&t).ok()).filter(|st| st.valid) else { continue };
        let m = method_of(e);
        match e.local() {
            "SignatureTimeStamp" if info.stamp.is_none() => {
                let Some(value) = s.child("SignatureValue") else { continue };
                // Only an authority trusted to time-stamp gives the time.
                if st.imprint == st.hash.digest(&canon(sig, value, &m))
                    && let Some(tsa) = &st.tsa
                    && crate::trust::fit_for_timestamping(tsa)
                {
                    let mut pool = st.certs.clone();
                    pool.extend(info.kept.certs.iter().cloned());
                    let (chain, trust) = crate::trust::check(tsa, &pool, st.time);
                    if trust.is_trusted() {
                        info.stamp = Some((st.time, st.tsa.clone(), chain));
                    }
                }
            }
            "ArchiveTimeStamp" => {
                if archive_input(doc, sig, &usp, i, &m, resolve).is_ok_and(|d| st.imprint == st.hash.digest(&d)) && trusted_stamp(&st, &info.kept) {
                    let chain = chain_of(&st, &info.kept);
                    info.archive = Some(Archive { time: st.time, until: ltv::chain_until(&chain), kept: kept_in(props.elems().take(i)) });
                }
            }
            _ => {}
        }
    }
    info
}

// --- extending -----------------------------------------------------------------------

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn iso(t: i64) -> String {
    let (y, mo, d, h, mi, s) = crate::time::civil(t);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// A big-endian unsigned number in decimal.
fn decimal(bytes: &[u8]) -> String {
    let mut n: Vec<u8> = bytes.iter().copied().skip_while(|&b| b == 0).collect();
    let mut digits = Vec::new();
    while !n.is_empty() {
        let mut rem = 0u32;
        for b in n.iter_mut() {
            let v = rem * 256 + *b as u32;
            *b = (v / 10) as u8;
            rem = v % 10;
        }
        digits.push(b'0' + rem as u8);
        let lead = n.iter().take_while(|&&b| b == 0).count();
        n.drain(..lead);
    }
    if digits.is_empty() {
        return "0".into();
    }
    digits.reverse();
    String::from_utf8(digits).unwrap_or_default()
}

fn digest_alg_and_value(data: &[u8]) -> String {
    format!("<ds:DigestMethod Algorithm=\"{SHA256}\"/><ds:DigestValue>{}</ds:DigestValue>", base64_encode(&Hash::Sha256.digest(data)))
}

/// A time-stamp element (`xd:` 1.3.2, `xa:` 1.4.1) of `data`.
fn stamp_element(qname: &str, data: &[u8], tsa: &str, net: &dyn Net) -> Result<String, Error> {
    let tok = crate::tsa::request_with(net, tsa, Hash::Sha256, &Hash::Sha256.digest(data))?;
    Ok(format!(
        "<{qname} xmlns:xd=\"{XADES_NS}\" xmlns:xa=\"{XADES141_NS}\" xmlns:ds=\"{DSIG_NS}\"><ds:CanonicalizationMethod Algorithm=\"{C14N}\"/><xd:EncapsulatedTimeStamp>{}</xd:EncapsulatedTimeStamp></{qname}>",
        base64_encode(&tok)
    ))
}

fn cert_values(certs: &[Cert]) -> String {
    certs.iter().map(|c| format!("<xd:EncapsulatedX509Certificate>{}</xd:EncapsulatedX509Certificate>", base64_encode(&c.der))).collect()
}

fn revocation_values(v: &Validation) -> String {
    let mut out = format!("<xd:RevocationValues xmlns:xd=\"{XADES_NS}\">");
    if !v.crls.is_empty() {
        out.push_str("<xd:CRLValues>");
        for d in &v.crls {
            out.push_str(&format!("<xd:EncapsulatedCRLValue>{}</xd:EncapsulatedCRLValue>", base64_encode(d)));
        }
        out.push_str("</xd:CRLValues>");
    }
    if !v.ocsps.is_empty() {
        out.push_str("<xd:OCSPValues>");
        for d in &v.ocsps {
            out.push_str(&format!("<xd:EncapsulatedOCSPValue>{}</xd:EncapsulatedOCSPValue>", base64_encode(d)));
        }
        out.push_str("</xd:OCSPValues>");
    }
    out.push_str("</xd:RevocationValues>");
    out
}

/// CompleteCertificateRefs and CompleteRevocationRefs of `v` (the signer's
/// certificate left out: the signed properties name it).
fn refs(v: &Validation, signer: &Cert) -> (String, String) {
    let mut certs = format!("<xd:CompleteCertificateRefs xmlns:xd=\"{XADES_NS}\" xmlns:ds=\"{DSIG_NS}\"><xd:CertRefs>");
    for c in v.certs.iter().filter(|c| c.der != signer.der) {
        certs.push_str(&format!(
            "<xd:Cert><xd:CertDigest>{}</xd:CertDigest><xd:IssuerSerial><ds:X509IssuerName>{}</ds:X509IssuerName><ds:X509SerialNumber>{}</ds:X509SerialNumber></xd:IssuerSerial></xd:Cert>",
            digest_alg_and_value(&c.der),
            esc(&c.issuer.full()),
            decimal(&c.serial)
        ));
    }
    certs.push_str("</xd:CertRefs></xd:CompleteCertificateRefs>");
    let mut rev = format!("<xd:CompleteRevocationRefs xmlns:xd=\"{XADES_NS}\" xmlns:ds=\"{DSIG_NS}\">");
    let crls: Vec<(&Vec<u8>, revocation::Crl)> = v.crls.iter().filter_map(|d| revocation::parse_crl(d).map(|c| (d, c))).collect();
    if !crls.is_empty() {
        rev.push_str("<xd:CRLRefs>");
        for (d, c) in crls {
            rev.push_str(&format!(
                "<xd:CRLRef><xd:DigestAlgAndValue>{}</xd:DigestAlgAndValue><xd:CRLIdentifier><xd:Issuer>{}</xd:Issuer><xd:IssueTime>{}</xd:IssueTime></xd:CRLIdentifier></xd:CRLRef>",
                digest_alg_and_value(d),
                esc(&c.issuer.full()),
                iso(c.this_update)
            ));
        }
        rev.push_str("</xd:CRLRefs>");
    }
    let ocsps: Vec<(&Vec<u8>, revocation::Ocsp)> = v.ocsps.iter().filter_map(|d| revocation::parse_ocsp(d).map(|o| (d, o))).collect();
    if !ocsps.is_empty() {
        rev.push_str("<xd:OCSPRefs>");
        for (d, o) in ocsps {
            let responder = match &o.responder {
                Responder::ByName(der) => {
                    let name = crate::asn1::parse(der).map(|t| Name::parse(&t).full()).unwrap_or_default();
                    format!("<xd:ByName>{}</xd:ByName>", esc(&name))
                }
                Responder::ByKey(k) => format!("<xd:ByKey>{}</xd:ByKey>", base64_encode(k)),
            };
            rev.push_str(&format!(
                "<xd:OCSPRef><xd:OCSPIdentifier><xd:ResponderID>{responder}</xd:ResponderID><xd:ProducedAt>{}</xd:ProducedAt></xd:OCSPIdentifier><xd:DigestAlgAndValue>{}</xd:DigestAlgAndValue></xd:OCSPRef>",
                iso(o.produced),
                digest_alg_and_value(d)
            ));
        }
        rev.push_str("</xd:OCSPRefs>");
    }
    rev.push_str("</xd:CompleteRevocationRefs>");
    (certs, rev)
}

/// The authorities of the time-stamps among `props`' children.
fn stamp_leaves(props: &Elem) -> Vec<(Cert, Vec<Cert>)> {
    props
        .elems()
        .filter(|e| is_timestamp(e.local()))
        .filter_map(|e| token(e).and_then(|t| cms::stamp(&t).ok()))
        .filter_map(|st| st.tsa.map(|t| (t, st.certs)))
        .collect()
}

/// Extends the `k`-th signature of the signatures file `xml` to `level`.
fn extend_one(xml: String, k: usize, level: Level, tsa: &str, net: &dyn Net, resolve: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<String, Error> {
    // Only a sound signature is worth keeping.
    let (signer, certs) = {
        let doc = c14n::parse(xml.as_bytes())?;
        let paths = signature_paths(&doc);
        let sig = paths.get(k).ok_or_else(|| Error::new("signature not found"))?;
        let c = xmldsig::check(&doc, sig, resolve);
        if c.valid != Check::Ok || c.intact != Check::Ok {
            return Err(Error::new("a signature of the file isn't valid, so it can't be extended"));
        }
        (c.cert.ok_or_else(|| Error::new("no signer certificate"))?, c.certs)
    };
    let mut xml = ensure_usp(xml, k)?;
    // XAdES-T: a time-stamp of the signature value.
    let has = |xml: &str, name: &str| -> Result<bool, Error> {
        let doc = c14n::parse(xml.as_bytes())?;
        let paths = signature_paths(&doc);
        Ok(paths.get(k).and_then(|p| usp_path(p)).is_some_and(|u| u[u.len() - 1].child(name).is_some()))
    };
    if !has(&xml, "SignatureTimeStamp")? {
        let data = {
            let doc = c14n::parse(xml.as_bytes())?;
            let paths = signature_paths(&doc);
            let sig = &paths[k];
            let value = sig[sig.len() - 1].child("SignatureValue").ok_or_else(|| Error::new("no SignatureValue"))?;
            canon(sig, value, &Method::default())
        };
        let el = stamp_element("xd:SignatureTimeStamp", &data, tsa, net)?;
        xml = add_usp(&xml, k, None, &el)?;
    }
    if level < Level::Lt {
        return Ok(xml);
    }
    // XAdES-XL: the chains and the revocation answers, referenced and kept.
    let (kept, leaves, has_refs, has_values, has_archive) = {
        let doc = c14n::parse(xml.as_bytes())?;
        let paths = signature_paths(&doc);
        let usp = usp_path(&paths[k]).ok_or_else(|| Error::new("no unsigned properties"))?;
        let props = usp[usp.len() - 1];
        let mut leaves = vec![(signer.clone(), certs.clone())];
        leaves.extend(stamp_leaves(props));
        (kept_in(props.elems()), leaves, props.child("CompleteCertificateRefs").is_some(), props.child("RevocationValues").is_some(), props.child("ArchiveTimeStamp").is_some())
    };
    let mut new = ltv::gather(net, &leaves, &kept)?;
    if !has_refs {
        let mut all = kept.clone();
        all.merge(&new);
        let (cert_refs, rev_refs) = refs(&all, &signer);
        xml = add_usp(&xml, k, None, &format!("{cert_refs}{rev_refs}"))?;
        // SigAndRefsTimeStamp: the signature value, its time-stamps and
        // the references.
        let data = {
            let doc = c14n::parse(xml.as_bytes())?;
            let paths = signature_paths(&doc);
            let sig = &paths[k];
            let usp = usp_path(sig).ok_or_else(|| Error::new("no unsigned properties"))?;
            let props = usp[usp.len() - 1];
            let m = Method::default();
            let mut d = sig[sig.len() - 1].child("SignatureValue").map(|v| canon(sig, v, &m)).unwrap_or_default();
            for name in ["SignatureTimeStamp", "CompleteCertificateRefs", "CompleteRevocationRefs"] {
                for e in props.elems().filter(|e| e.local() == name) {
                    d.extend(canon(&usp, e, &m));
                }
            }
            d
        };
        let el = stamp_element("xd:SigAndRefsTimeStamp", &data, tsa, net)?;
        xml = add_usp(&xml, k, None, &el)?;
        // Its authority is checked too.
        let doc = c14n::parse(xml.as_bytes())?;
        let paths = signature_paths(&doc);
        if let Some(usp) = usp_path(&paths[k])
            && let Some(ts) = usp[usp.len() - 1].elems().filter(|e| e.local() == "SigAndRefsTimeStamp").last()
            && let Some(st) = token(ts).and_then(|t| cms::stamp(&t).ok())
            && let Some(t) = st.tsa
        {
            let mut have = kept.clone();
            have.merge(&new);
            new.merge(&ltv::gather(net, &[(t, st.certs)], &have)?);
        }
    }
    if !new.is_empty() {
        if !has_values && !has_archive {
            // Among the certificate values already there (XAdES-BES keeps
            // the chain it had), then the revocation values.
            let certs = cert_values(&new.certs);
            let doc_has_cv = has(&xml, "CertificateValues")?;
            xml = if doc_has_cv {
                add_usp(&xml, k, Some("CertificateValues"), &certs)?
            } else {
                add_usp(&xml, k, None, &format!("<xd:CertificateValues xmlns:xd=\"{XADES_NS}\">{certs}</xd:CertificateValues>"))?
            };
            xml = add_usp(&xml, k, None, &revocation_values(&new))?;
        } else {
            // Later additions (for the time-stamps made since).
            let el = format!(
                "<xa:TimeStampValidationData xmlns:xa=\"{XADES141_NS}\" xmlns:xd=\"{XADES_NS}\"><xd:CertificateValues>{}</xd:CertificateValues>{}</xa:TimeStampValidationData>",
                cert_values(&new.certs),
                revocation_values(&new)
            );
            xml = add_usp(&xml, k, None, &el)?;
        }
    }
    if level < Level::Lta {
        return Ok(xml);
    }
    // XAdES-A: an archive time-stamp over all of it.
    let data = {
        let doc = c14n::parse(xml.as_bytes())?;
        let paths = signature_paths(&doc);
        let sig = &paths[k];
        let usp = usp_path(sig).ok_or_else(|| Error::new("no unsigned properties"))?;
        let n = usp[usp.len() - 1].elems().count();
        archive_input(&doc, sig, &usp, n, &Method::default(), resolve)?
    };
    let el = stamp_element("xa:ArchiveTimeStamp", &data, tsa, net)?;
    add_usp(&xml, k, None, &el)
}

/// Extends the signatures of an OOXML or ODF package to `level` (every
/// one, or only the newest).
pub(crate) fn extend_some(data: &[u8], level: Level, tsa: Option<&str>, net: &dyn Net, newest_only: bool) -> Result<Vec<u8>, Error> {
    if level == Level::B {
        return Ok(data.to_vec());
    }
    let tsa = tsa.ok_or_else(|| Error::new("a time-stamp server is needed for this level"))?;
    let z = ZipReader::new(data).map_err(|_| Error::new("only unencrypted OOXML and ODF files can be extended"))?;
    let mut files: Vec<String> = Vec::new();
    if z.contains(ODF_SIGNATURES) {
        files.push(ODF_SIGNATURES.into());
    }
    files.extend(ooxml_parts(&z));
    let resolve = |name: &str| z.read(name).ok();
    let mut texts: Vec<(String, String, usize)> = Vec::new();
    for f in files {
        let xml = z.read_string(&f)?;
        let n = signature_spans(&spans(&xml)?).len();
        if n > 0 {
            texts.push((f, xml, n));
        }
    }
    if texts.is_empty() {
        return Err(Error::new("the file isn't signed"));
    }
    let last = texts.len() - 1;
    let mut changes = Vec::new();
    for (i, (f, mut xml, n)) in texts.into_iter().enumerate() {
        if newest_only && i != last {
            continue;
        }
        let from = if newest_only { n - 1 } else { 0 };
        for k in from..n {
            xml = extend_one(xml, k, level, tsa, net, &resolve)?;
        }
        changes.push((f, xml.into_bytes()));
    }
    crate::xml_sign::repack(&z, changes)
}

pub(crate) fn extend(data: &[u8], level: Level, tsa: Option<&str>, net: &dyn Net) -> Result<Vec<u8>, Error> {
    extend_some(data, level, tsa, net, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_spans() {
        assert_eq!(decimal(&[0x01, 0x00]), "256");
        assert_eq!(decimal(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]), "18446744073709551615");
        assert_eq!(decimal(&[]), "0");
        let xml = "<?xml version=\"1.0\"?><!-- <x> --><r><a t=\"x>y\"/><b><c/></b></r>";
        let sp = spans(xml).unwrap();
        assert_eq!(sp.iter().map(|s| s.local.as_str()).collect::<Vec<_>>(), ["r", "a", "b", "c"]);
        assert_eq!(sp[3].parent, Some(2));
        assert_eq!(insert(xml, &sp[1], "<i/>", true), "<?xml version=\"1.0\"?><!-- <x> --><r><a t=\"x>y\"><i/></a><b><c/></b></r>");
        assert_eq!(insert(xml, &sp[2], "<i/>", true), "<?xml version=\"1.0\"?><!-- <x> --><r><a t=\"x>y\"/><b><c/><i/></b></r>");
        assert!(spans("<a><b></a>").is_err());
    }
}
