//! Signatures end to end: certificate files, OOXML and ODF packages
//! signed by LibreOffice with a test certificate, and PDF files signed
//! here (with a time-stamp from a local authority).
//!
//! The test PKI (tests/data): "ApexDesk Test Root CA" (RSA) issued Alice
//! (RSA, alice-*.p12, password "secreto") and Bob (P-256, bob-empty.p12
//! without password, bob-utf8.p12 with "ñandú"), made with OpenSSL; Carol
//! (P-256, carol-code.p12 without password) signs code, self-signed.

use std::io::{Read, Write};

use apex_sign::asn1;
use apex_sign::crypto::{Hash, PrivKey};
use apex_sign::{Cert, Check, Identity, Kind, P12Error, SignOptions, Status, Trust};

fn data(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// Trusts the test root for this process.
fn trust_test_root() -> Cert {
    let ca = Cert::from_der(&data("ca.der")).unwrap();
    apex_sign::trust::add_root(ca.clone());
    ca
}

fn alice() -> Identity {
    apex_sign::p12::read(&data("alice-aes.p12"), "secreto").unwrap()
}

fn bob() -> Identity {
    apex_sign::p12::read(&data("bob-empty.p12"), "").unwrap()
}

/// A package with one part replaced (or added).
fn with_part(pkg: &[u8], name: &str, content: &[u8]) -> Vec<u8> {
    let z = apex_io::zip::ZipReader::new(pkg).unwrap();
    let mut w = apex_io::zip::ZipWriter::new();
    let mut done = false;
    for n in z.names().map(str::to_string).collect::<Vec<_>>() {
        if n == name {
            w.add(&n, content, true);
            done = true;
        } else {
            w.add(&n, &z.read(&n).unwrap(), n != "mimetype");
        }
    }
    if !done {
        w.add(name, content, true);
    }
    w.finish()
}

#[test]
fn certificate_files() {
    let a = alice();
    assert!(matches!(a.key, PrivKey::Rsa(_)));
    assert_eq!(a.cert.subject.display(), "Alice Tester");
    assert_eq!(a.cert.subject.get("1.2.840.113549.1.9.1"), Some("alice@example.org"));
    assert_eq!(a.chain.len(), 1);
    assert!(a.chain[0].ca);
    // OpenSSL's -legacy: RC2-40 for the certificates, 3DES for the key.
    let legacy = apex_sign::p12::read(&data("alice-legacy.p12"), "secreto").unwrap();
    assert_eq!(legacy.cert.der, a.cert.der);
    assert!(legacy.key.matches(&a.cert.public_key().unwrap()));
    let b = bob();
    assert!(matches!(b.key, PrivKey::P256(_)));
    assert_eq!(b.cert.subject.display(), "Bob Tester");
    let u = apex_sign::p12::read(&data("bob-utf8.p12"), "ñandú").unwrap();
    assert_eq!(u.cert.der, b.cert.der);
    assert_eq!(apex_sign::p12::read(&data("alice-aes.p12"), "wrong").err(), Some(P12Error::WrongPassword));
    assert_eq!(apex_sign::p12::read(&data("alice-legacy.p12"), "").err(), Some(P12Error::WrongPassword));
    assert!(apex_sign::p12::read(b"not a p12", "x").is_err());
}

#[test]
fn packages_signed_by_libreoffice() {
    let ca = trust_test_root();
    for (name, kind) in [("signed.odt", Kind::Odf), ("signed.ods", Kind::Odf), ("signed.docx", Kind::Ooxml), ("signed.xlsx", Kind::Ooxml)] {
        let pkg = data(name);
        assert!(apex_sign::is_signed(&pkg), "{name}");
        let sigs = apex_sign::verify(&pkg);
        assert_eq!(sigs.len(), 1, "{name}");
        let s = &sigs[0];
        assert_eq!(s.kind, kind);
        assert_eq!(s.format, "XAdES");
        assert_eq!(s.signer(), "Alice Tester");
        assert_eq!((s.intact, s.valid), (Check::Ok, Check::Ok), "{name}: {:?}", s.note);
        assert_eq!(s.trust, Trust::Trusted("ApexDesk Test Root CA".into()));
        assert_eq!(s.chain.last().map(|c| &c.der), Some(&ca.der));
        assert!(s.time.is_some_and(|t| t > 1_790_000_000));
        assert_eq!(s.status(), Status::Valid, "{name}");
    }
}

#[test]
fn changed_packages() {
    trust_test_root();
    // Content changed after signing.
    let odt = data("signed.odt");
    let z = apex_io::zip::ZipReader::new(&odt).unwrap();
    let content = z.read_string("content.xml").unwrap().replace("Hola", "Adiós");
    let changed = with_part(&odt, "content.xml", content.as_bytes());
    let s = &apex_sign::verify(&changed)[0];
    assert_eq!(s.status(), Status::Invalid);
    assert_eq!(s.intact, Check::Failed);
    assert_eq!(s.valid, Check::Ok);
    assert_eq!(s.changed_parts, ["content.xml"]);
    // A stream added after signing.
    let added = with_part(&odt, "Pictures/extra.png", b"\x89PNG");
    let s = &apex_sign::verify(&added)[0];
    assert_eq!(s.status(), Status::ChangedAfter);
    assert_eq!(s.unsigned_parts, ["Pictures/extra.png"]);
    // A signed part of a workbook changed.
    let docx = data("signed.docx");
    let z = apex_io::zip::ZipReader::new(&docx).unwrap();
    let xlsx = data("signed.xlsx");
    let zx = apex_io::zip::ZipReader::new(&xlsx).unwrap();
    let sheet = zx.read_string("xl/worksheets/sheet1.xml").unwrap().replace("<v>2</v>", "<v>3</v>");
    let s = &apex_sign::verify(&with_part(&xlsx, "xl/worksheets/sheet1.xml", sheet.as_bytes()))[0];
    assert_eq!(s.status(), Status::Invalid);
    assert_eq!(s.changed_parts, ["xl/worksheets/sheet1.xml"]);
    // A part an OOXML signature doesn't cover (added after signing, or
    // left out by a signature over the properties alone) is reported.
    let s = &apex_sign::verify(&with_part(&docx, "word/media/extra.png", b"\x89PNG"))[0];
    assert_eq!(s.status(), Status::ChangedAfter);
    assert_eq!(s.unsigned_parts, ["word/media/extra.png"]);
    // Unsigned relationships pointed at a part no signature covers (the
    // properties, which anyone can then rewrite), or of a part added later.
    let rels = z.read_string("word/_rels/document.xml.rels").unwrap().replace("</Relationships>", "<Relationship Id=\"rIdX\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"../[Content_Types].xml\"/></Relationships>");
    let s = &apex_sign::verify(&with_part(&docx, "word/_rels/document.xml.rels", rels.as_bytes()))[0];
    assert_eq!(s.status(), Status::ChangedAfter);
    assert_eq!(s.unsigned_parts, ["word/_rels/document.xml.rels"]);
    let rels = "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"r\" Type=\"t\" Target=\"document.xml\"/></Relationships>";
    let s = &apex_sign::verify(&with_part(&docx, "word/_rels/extra.xml.rels", rels.as_bytes()))[0];
    assert_eq!(s.unsigned_parts, ["word/_rels/extra.xml.rels"]);
    // Relationships nested in another (readers take them) or a second
    // root after the first.
    let doc_rels = z.read_string("word/_rels/document.xml.rels").unwrap();
    for rels in [
        doc_rels.replacen("/>", "><Relationship Id=\"rIdN\" Type=\"t\" Target=\"styles.xml\"/></Relationship>", 1),
        doc_rels.clone() + "<Relationships/>",
    ] {
        let s = &apex_sign::verify(&with_part(&docx, "word/_rels/document.xml.rels", rels.as_bytes()))[0];
        assert_eq!(s.status(), Status::ChangedAfter, "{rels}");
    }
    // The signature value itself tampered with.
    let sig = z.read_string("_xmlsignatures/sig1.xml").unwrap();
    let at = sig.find("<SignatureValue").unwrap();
    let at = at + sig[at..].find('>').unwrap() + 5;
    let mut bad = sig.clone();
    let c = if &sig[at..at + 1] == "A" { "B" } else { "A" };
    bad.replace_range(at..at + 1, c);
    let s = &apex_sign::verify(&with_part(&docx, "_xmlsignatures/sig1.xml", bad.as_bytes()))[0];
    assert_eq!((s.intact, s.valid, s.status()), (Check::Ok, Check::Failed, Status::Invalid));
    // A signature file that can't be read can't be checked.
    let broken = with_part(&odt, "META-INF/documentsignatures.xml", b"<document-signatures");
    assert_eq!(apex_sign::verify(&broken)[0].status(), Status::Unknown);
    // Unsigned files have no signatures.
    assert!(apex_sign::verify(&data("ca.der")).is_empty());
    assert!(!apex_sign::is_signed(&pdf(1)));
    assert!(apex_sign::verify(&pdf(1)).is_empty());
}

/// A small PDF like the ones ApexDesk exports.
fn pdf(pages: usize) -> Vec<u8> {
    let mut p = apex_pdf::Pdf::new();
    p.set_info("Prueba", "ApexDesk");
    for i in 0..pages {
        p.begin_page(595.0, 842.0);
        p.fill_rect(50.0, 50.0 + i as f32 * 10.0, 200.0, 100.0, [200, 30, 30]);
    }
    p.finish()
}

/// A time when the test certificates are valid.
const SIGNED_AT: i64 = 1_800_000_000;

fn sign(pdf: &[u8], id: &Identity) -> Vec<u8> {
    let opts = SignOptions { reason: Some("Aprobado".into()), location: Some("Sevilla".into()), time: Some(SIGNED_AT), ..Default::default() };
    apex_sign::sign_pdf(pdf, id, &opts).unwrap()
}

#[test]
fn pdf_signing_rsa_and_ecdsa() {
    trust_test_root();
    for id in [alice(), bob()] {
        let signed = sign(&pdf(2), &id);
        assert!(apex_sign::is_signed(&signed));
        let sigs = apex_sign::verify(&signed);
        assert_eq!(sigs.len(), 1);
        let s = &sigs[0];
        assert_eq!(s.kind, Kind::Pdf);
        assert_eq!(s.format, "PAdES (CAdES)");
        assert_eq!(s.signer(), id.cert.subject.display());
        assert_eq!(s.reason.as_deref(), Some("Aprobado"));
        assert_eq!(s.location.as_deref(), Some("Sevilla"));
        assert_eq!(s.time, Some(SIGNED_AT));
        assert_eq!(s.status(), Status::Valid, "{:?}", s.note);
        // The update keeps the file readable: the new trailer points back.
        let text = String::from_utf8_lossy(&signed);
        assert!(text.contains("/Prev ") && text.contains("/SigFlags 3") && text.contains("/FT /Sig"));
        assert!(text.ends_with("%%EOF\n"));
    }
}

#[test]
fn pdf_changes_after_signing() {
    trust_test_root();
    let signed = sign(&pdf(1), &alice());
    // A byte of the first revision changed.
    let mut bad = signed.clone();
    bad[200] ^= 1;
    let s = &apex_sign::verify(&bad)[0];
    assert_eq!((s.intact, s.status()), (Check::Failed, Status::Invalid));
    // A later revision: still a right signature, of an older version.
    let mut later = signed.clone();
    later.extend_from_slice(b"99 0 obj\n<< /Note (added) >>\nendobj\n%%EOF\n");
    let s = &apex_sign::verify(&later)[0];
    assert!(s.changed_after);
    assert_eq!(s.status(), Status::ChangedAfter);
    // Signed twice: the first signature covers the first revision only,
    // and the second (an invisible field) changes nothing it showed.
    let twice = sign(&signed, &bob());
    let sigs = apex_sign::verify(&twice);
    assert_eq!(sigs.len(), 2);
    assert_eq!(sigs[0].status(), Status::Valid);
    assert_eq!(sigs[1].status(), Status::Valid);
    assert_eq!(sigs[1].signer(), "Bob Tester");
    assert!(String::from_utf8_lossy(&twice).contains("(Signature2)"));
}

/// `base` with an update whose table frees object `freed` (none: frees
/// nothing) and whose trailer adds `extra`.
fn updated(base: &[u8], freed: Option<u32>, extra: &str) -> Vec<u8> {
    let text = String::from_utf8_lossy(base);
    let prev: usize = text[text.rfind("startxref").unwrap() + 9..].split_whitespace().next().unwrap().parse().unwrap();
    let root = &text[text.rfind("/Root ").unwrap()..];
    let root = &root[..root.find(" R").unwrap() + 2];
    let size = &text[text.rfind("/Size ").unwrap()..];
    let size = &size[..size[6..].find(|c: char| !c.is_ascii_digit()).unwrap() + 6];
    let mut out = base.to_vec();
    let at = out.len();
    let mut table = String::from("xref\n0 1\n0000000000 65535 f \n");
    if let Some(n) = freed {
        table += &format!("{n} 1\n0000000000 00001 f \n");
    }
    out.extend_from_slice(format!("{table}trailer\n<< {size} {root} /Prev {prev}{extra} >>\nstartxref\n{at}\n%%EOF\n").as_bytes());
    out
}

#[test]
fn pdf_signing_refuses_what_readers_see_otherwise() {
    let base = pdf(1);
    let id = alice();
    let opts = SignOptions::default();
    if let Err(e) = apex_sign::sign_pdf(&updated(&base, None, ""), &id, &opts) {
        panic!("{e}");
    }
    // A newer table frees an object an older one defines: readers see
    // none, and rewriting it would bring it back.
    let e = apex_sign::sign_pdf(&updated(&base, Some(3), ""), &id, &opts).unwrap_err();
    assert!(e.to_string().contains("freed objects"), "{e}");
    // A cross-reference stream besides the tables, which readers prefer.
    let e = apex_sign::sign_pdf(&updated(&base, None, " /XRef#53tm 0"), &id, &opts).unwrap_err();
    assert!(e.to_string().contains("hybrid"), "{e}");
    // The newest table a byte after where the file says.
    let good = updated(&base, None, "");
    let at = base.len();
    let mut off = good.clone();
    let tail = format!("startxref\n{at}\n%%EOF\n");
    assert!(off.ends_with(tail.as_bytes()));
    off.truncate(off.len() - tail.len());
    off.extend_from_slice(format!("startxref\n{}\n%%EOF\n", at - 1).as_bytes());
    let e = apex_sign::sign_pdf(&off, &id, &opts).unwrap_err();
    assert!(e.to_string().contains("repaired"), "{e}");
    assert!(!apex_sign::pdf_can_add_to(&off));
    // A field named with periods: another field's full name.
    let opts = SignOptions { field: Some("a.b".into()), ..Default::default() };
    assert!(apex_sign::sign_pdf(&good, &id, &opts).is_err());
}

/// `signed` with a revision writing its catalog again, unchanged, as
/// generation `g` (listed `listed` times in the new table).
fn catalog_again(signed: &[u8], g: u16, listed: usize) -> Vec<u8> {
    let text = String::from_utf8_lossy(signed).into_owned();
    let start = text.rfind("\n1 0 obj").unwrap() + 1;
    let body = &text[start + 7..start + text[start..].find("endobj").unwrap()];
    let prev: usize = text[text.rfind("startxref").unwrap() + 9..].split_whitespace().next().unwrap().parse().unwrap();
    let t = text.rfind("trailer").unwrap();
    let dict = &text[t + 7..t + text[t..].find("startxref").unwrap()];
    let old_prev = &dict[dict.find("/Prev ").unwrap()..];
    let old_prev = &old_prev[..6 + old_prev[6..].find(|c: char| !c.is_ascii_digit()).unwrap()];
    let dict = dict.replace(old_prev, &format!("/Prev {prev}"));
    let mut out = signed.to_vec();
    let at = out.len();
    out.extend_from_slice(format!("1 {g} obj{body}endobj\n").as_bytes());
    let x = out.len();
    let mut table = String::from("xref\n0 1\n0000000000 65535 f \n");
    for _ in 0..listed {
        table += &format!("1 1\n{at:010} {g:05} n \n");
    }
    out.extend_from_slice(format!("{table}trailer{dict}startxref\n{x}\n%%EOF\n").as_bytes());
    out
}

#[test]
fn pdf_revisions_readers_take_otherwise() {
    trust_test_root();
    let signed = sign(&pdf(1), &alice());
    // The catalog written again as it was: nothing changed.
    assert_eq!(apex_sign::verify(&catalog_again(&signed, 0, 1))[0].status(), Status::Valid);
    // As another generation, references to it find nothing elsewhere.
    assert_eq!(apex_sign::verify(&catalog_again(&signed, 1, 1))[0].status(), Status::ChangedAfter);
    // Listed twice, readers take either entry.
    assert_eq!(apex_sign::verify(&catalog_again(&signed, 0, 2))[0].status(), Status::ChangedAfter);
}

/// A self-signed certificate made here, with a P-256 key.
fn self_signed(name: &str) -> Identity {
    let secret = p256::SecretKey::from_slice(&[7u8; 32]).unwrap();
    let key = PrivKey::P256(secret.clone().into());
    use p256::pkcs8::EncodePublicKey;
    let spki = secret.public_key().to_public_key_der().unwrap();
    let dn = asn1::seq(&[&asn1::set_of(&[&asn1::seq(&[&asn1::oid("2.5.4.3"), &asn1::utf8(name)])])]);
    let validity = asn1::seq(&[&asn1::utc_time(1_700_000_000), &asn1::utc_time(2_000_000_000)]);
    let tbs = asn1::seq(&[&asn1::ctx(0, &[&asn1::uint(2)]), &asn1::uint(4242), &key.alg(), &dn, &validity, &dn, spki.as_bytes()]);
    let der = apex_sign::cms::sign_x509(&key, &tbs).unwrap();
    Identity { key, cert: Cert::from_der(&der).unwrap(), chain: Vec::new() }
}

#[test]
fn self_signed_is_not_trusted() {
    let id = self_signed("Mallory");
    assert!(id.cert.self_issued() && id.cert.signed(&id.cert));
    let s = &apex_sign::verify(&sign(&pdf(1), &id))[0];
    assert_eq!((s.intact, s.valid), (Check::Ok, Check::Ok));
    assert_eq!(s.trust, Trust::SelfSigned);
    assert_eq!(s.status(), Status::Untrusted);
}

/// A time-stamp authority on a local port, answering one request with a
/// token signed by `tsa`.
fn local_tsa(tsa: Identity, time: i64) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        let body = loop {
            let n = s.read(&mut chunk).unwrap();
            buf.extend_from_slice(&chunk[..n]);
            if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase();
                let len: usize = head.lines().find_map(|l| l.strip_prefix("content-length:")).unwrap().trim().parse().unwrap();
                if buf.len() >= end + 4 + len {
                    break buf[end + 4..end + 4 + len].to_vec();
                }
            }
        };
        // The request's imprint and nonce go into the TSTInfo.
        let req = asn1::parse(&body).unwrap();
        let mut r = req.children();
        r.next();
        let imprint = r.next().unwrap().raw.to_vec();
        let nonce = r.next().unwrap().raw.to_vec();
        let info = asn1::seq(&[&asn1::uint(1), &asn1::oid("1.2.3.4"), &imprint, &asn1::uint(77), &asn1::generalized_time(time), &nonce]);
        let attr = |oid: &str, v: &[u8]| asn1::seq(&[&asn1::oid(oid), &asn1::set_of(&[v])]);
        let signed = asn1::set_of(&[&attr("1.2.840.113549.1.9.3", &asn1::oid("1.2.840.113549.1.9.16.1.4")), &attr("1.2.840.113549.1.9.4", &asn1::octets(&Hash::Sha256.digest(&info)))]);
        let sig = tsa.key.sign(&Hash::Sha256.digest(&signed)).unwrap();
        let mut signed_attrs = signed.clone();
        signed_attrs[0] = 0xA0;
        let sid = asn1::seq(&[tsa.cert.issuer_der(), &asn1::tlv(asn1::INTEGER, &tsa.cert.serial)]);
        let digest_alg = asn1::alg(Hash::Sha256.oid(), false);
        let si = asn1::seq(&[&asn1::uint(1), &sid, &digest_alg, &signed_attrs, &tsa.key.alg(), &asn1::octets(&sig)]);
        let encap = asn1::seq(&[&asn1::oid("1.2.840.113549.1.9.16.1.4"), &asn1::ctx(0, &[&asn1::octets(&info)])]);
        let sd = asn1::seq(&[&asn1::uint(3), &asn1::set_of(&[&digest_alg]), &encap, &asn1::tlv(0xA0, &tsa.cert.der), &asn1::set_of(&[&si])]);
        let token = asn1::seq(&[&asn1::oid("1.2.840.113549.1.7.2"), &asn1::ctx(0, &[&sd])]);
        let resp = asn1::seq(&[&asn1::seq(&[&asn1::uint(0)]), &token]);
        // Chunked, like many servers answer.
        let mut out = b"HTTP/1.1 200 OK\r\nContent-Type: application/timestamp-reply\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        for part in resp.chunks(100) {
            out.extend_from_slice(format!("{:x}\r\n", part.len()).as_bytes());
            out.extend_from_slice(part);
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"0\r\n\r\n");
        s.write_all(&out).unwrap();
    });
    format!("http://127.0.0.1:{port}/tsr")
}

#[test]
fn pdf_with_time_stamp() {
    trust_test_root();
    let stamp_time = SIGNED_AT + 123;
    let url = local_tsa(bob(), stamp_time);
    let opts = SignOptions { tsa_url: Some(url), time: Some(SIGNED_AT), ..Default::default() };
    let signed = apex_sign::sign_pdf(&pdf(1), &alice(), &opts).unwrap();
    let s = &apex_sign::verify(&signed)[0];
    assert_eq!(s.status(), Status::Valid, "{:?}", s.note);
    // Bob's certificate isn't one for time-stamping (and anyone can make
    // a time-stamp): it doesn't vouch for the time. Those of a real
    // authority are in ltv.rs.
    assert!(!s.timestamped);
    assert_eq!(s.time, Some(SIGNED_AT));
    let _ = stamp_time;
    // A server that isn't there fails the signing instead of leaving the
    // time-stamp out quietly.
    let opts = SignOptions { tsa_url: Some("http://127.0.0.1:9/".into()), ..Default::default() };
    assert!(apex_sign::sign_pdf(&pdf(1), &alice(), &opts).is_err());
}

/// A package LibreOffice signed with its signatures taken out.
fn unsigned(name: &str) -> Vec<u8> {
    let pkg = data(name);
    let z = apex_io::zip::ZipReader::new(&pkg).unwrap();
    let mut w = apex_io::zip::ZipWriter::new();
    for n in z.names() {
        if n.starts_with("_xmlsignatures/") || n == "META-INF/documentsignatures.xml" {
            continue;
        }
        let mut d = z.read(n).unwrap();
        if n == "_rels/.rels" {
            let s = String::from_utf8(d).unwrap();
            let at = s.find("digital-signature/origin").unwrap();
            let (a, b) = (s[..at].rfind("<Relationship ").unwrap(), at + s[at..].find("/>").unwrap() + 2);
            d = format!("{}{}", &s[..a], &s[b..]).into_bytes();
        }
        w.add(n, &d, n != "mimetype");
    }
    w.finish()
}

#[test]
fn package_signing() {
    trust_test_root();
    for name in ["signed.docx", "signed.xlsx", "signed.odt", "signed.ods"] {
        let pkg = unsigned(name);
        assert!(!apex_sign::is_signed(&pkg) && apex_sign::can_sign_package(&pkg), "{name}");
        // Alice (RSA) with a comment, then Bob (ECDSA): both signatures hold.
        let opts = SignOptions { reason: Some("Aprobado <sin> cambios & más".into()), time: Some(SIGNED_AT), ..Default::default() };
        let once = apex_sign::sign_package(&pkg, &alice(), &opts).unwrap();
        let sigs = apex_sign::verify(&once);
        assert_eq!(sigs.len(), 1, "{name}");
        let s = &sigs[0];
        assert_eq!(s.status(), Status::Valid, "{name}: {:?}", s.note);
        assert_eq!((s.signer().as_str(), s.format.as_str(), s.time), ("Alice Tester", "XAdES", Some(SIGNED_AT)));
        assert_eq!(s.reason.as_deref(), Some("Aprobado <sin> cambios & más"));
        assert_eq!(s.kind, if name.ends_with('x') { Kind::Ooxml } else { Kind::Odf });
        let twice = apex_sign::sign_package(&once, &bob(), &SignOptions::default()).unwrap();
        let sigs = apex_sign::verify(&twice);
        let got: Vec<(String, Status)> = sigs.iter().map(|s| (s.signer(), s.status())).collect();
        assert_eq!(got, [("Alice Tester".to_string(), Status::Valid), ("Bob Tester".to_string(), Status::Valid)], "{name}");
        assert!(sigs[1].reason.is_none());
        // The document's parts are as they were.
        let (z0, z2) = (apex_io::zip::ZipReader::new(&pkg).unwrap(), apex_io::zip::ZipReader::new(&twice).unwrap());
        for n in z0.names().filter(|n| !n.ends_with(".rels") && !n.starts_with("[Content_Types]")) {
            assert_eq!(z0.read(n).unwrap(), z2.read(n).unwrap(), "{name}: {n}");
        }
        // A changed part breaks both.
        let part = z2.names().find(|n| n.ends_with("document.xml") || n.ends_with("sheet1.xml") || *n == "content.xml").unwrap().to_string();
        let mut xml = z2.read_string(&part).unwrap();
        xml.insert(xml.rfind("</").unwrap(), 'x');
        let changed = with_part(&twice, &part, xml.as_bytes());
        assert!(apex_sign::verify(&changed).iter().all(|s| s.status() == Status::Invalid), "{name}");
    }
    // Next to LibreOffice's own signature.
    for name in ["signed.docx", "signed.odt"] {
        let both = apex_sign::sign_package(&data(name), &bob(), &SignOptions::default()).unwrap();
        let sigs = apex_sign::verify(&both);
        assert_eq!(sigs.len(), 2);
        assert!(sigs.iter().all(|s| s.status() == Status::Valid), "{name}");
    }
    // What can't be signed.
    assert!(!apex_sign::can_sign_package(&pdf(1)));
    assert!(apex_sign::sign_package(b"a,b\n1,2\n", &alice(), &SignOptions::default()).is_err());
}

#[test]
fn pdf_signing_a_visible_field() {
    use apex_io::pdf::edit::PdfDoc;
    use apex_io::pdf::edit::forms::{FieldKind, SigLook};
    trust_test_root();
    let mut doc = PdfDoc::open(pdf(2), None).unwrap();
    let look = SigLook { lines: vec!["Alice".into()], ..Default::default() };
    let name = doc.add_signature_field(1, [100.0, 100.0, 300.0, 160.0], &look).unwrap();
    let base = doc.save();
    let opts = SignOptions { time: Some(SIGNED_AT), field: Some(name.clone()), ..Default::default() };
    let signed = apex_sign::sign_pdf(&base, &alice(), &opts).unwrap();
    let sigs = apex_sign::verify(&signed);
    assert_eq!(sigs.len(), 1);
    assert_eq!(sigs[0].status(), Status::Valid, "{:?}", sigs[0].note);
    // No second field: the drawn one holds the signature.
    let again = PdfDoc::open(signed.clone(), None).unwrap();
    let f = again.fields();
    assert_eq!(f.len(), 1);
    assert_eq!((f[0].kind.clone(), f[0].name.as_str(), f[0].widgets[0].page), (FieldKind::Signature, name.as_str(), 1));
    // A second place, signed by another: filling it leaves the first
    // signature as it was.
    let mut doc = PdfDoc::open(base.clone(), None).unwrap();
    let second2 = doc.add_signature_field(1, [100.0, 200.0, 300.0, 260.0], &look).unwrap();
    let both = doc.save();
    let one = apex_sign::sign_pdf(&both, &alice(), &opts).unwrap();
    let two = apex_sign::sign_pdf(&one, &bob(), &SignOptions { field: Some(second2.clone()), ..opts.clone() }).unwrap();
    let sigs = apex_sign::verify(&two);
    assert_eq!(sigs.len(), 2);
    assert!(sigs.iter().all(|s| s.status() == Status::Valid), "{:?}", sigs.iter().map(|s| (s.status(), s.note.clone())).collect::<Vec<_>>());
    // A revision that changes anything else still shows.
    let mut later = two.clone();
    later.extend_from_slice(b"99 0 obj\n<< /Note (added) >>\nendobj\n%%EOF\n");
    assert!(apex_sign::verify(&later).iter().all(|s| s.status() == Status::ChangedAfter));
    // A field already signed, or missing, is refused.
    assert!(apex_sign::sign_pdf(&signed, &alice(), &opts).is_err());
    let none = SignOptions { field: Some("Nope".into()), ..opts };
    assert!(apex_sign::sign_pdf(&base, &alice(), &none).is_err());
}

#[test]
fn packages_pdf_readers_take_for_pdfs() {
    trust_test_root();
    // A part stored first whose bytes start a PDF.
    let pkg = unsigned("signed.docx");
    let z = apex_io::zip::ZipReader::new(&pkg).unwrap();
    let mut w = apex_io::zip::ZipWriter::new();
    w.add("word/media/a.bin", b"%PDF-1.7\n1 0 obj<</Type/Catalog>>endobj\n", false);
    for n in z.names() {
        w.add(n, &z.read(n).unwrap(), true);
    }
    let both = w.finish();
    let plain = apex_sign::sign_package(&pkg, &alice(), &SignOptions::default()).unwrap();
    assert_eq!(apex_sign::verify(&plain)[0].status(), Status::Valid);
    let signed = apex_sign::sign_package(&both, &alice(), &SignOptions::default()).unwrap();
    let sigs = apex_sign::verify(&signed);
    assert_eq!(sigs.len(), 1);
    assert_eq!(sigs[0].status(), Status::ChangedAfter);
    assert!(!apex_sign::stamps_only(&sigs));
}

#[test]
fn vba_projects_signed_here() {
    let bin = apex_io::vba::new_workbook_project("ThisWorkbook", &["Sheet1".to_string()]).unwrap();
    let mut p = apex_io::vba::read(&bin).unwrap();
    let mut m = apex_io::vba::VbaModule::standard("Module1");
    m.code = "Sub Hello()\n    MsgBox \"Hi\"\nEnd Sub\n".into();
    p.modules.push(m);
    // Written as usual, a module ends in a line break: the V3 readings
    // differ and it isn't signed.
    assert!(apex_sign::vba::sign(&alice(), &p.write().unwrap()).is_err());
    let signed = p.write_for_signing().unwrap();
    for id in [alice(), bob()] {
        let sig = apex_sign::vba::sign(&id, &signed).unwrap();
        let s = apex_sign::vba::check(&sig, &signed).unwrap();
        assert!(s.intact && s.code_matches && s.v3, "{}", id.cert.subject.display());
        assert_eq!(s.cert.der, id.cert.der);
        // Not for other code.
        let mut q = apex_io::vba::read(&signed).unwrap();
        q.modules.last_mut().unwrap().code = "Sub Hello()\n    Shell \"calc\"\nEnd Sub".into();
        let other = q.write_for_signing().unwrap();
        let s = apex_sign::vba::check(&sig, &other).unwrap();
        assert!(s.intact && !s.code_matches && !s.v3);
        // Nor once a byte of what it signs changes.
        let mut bad = sig.clone();
        let at = bad.windows(4).position(|w| w == [12, 0, 0, 0]).unwrap();
        bad[at + 4] ^= 2;
        assert!(!apex_sign::vba::check(&bad, &signed).unwrap().intact);
    }
    // A certificate for signing code makes a signature that may be trusted.
    let carol = apex_sign::p12::read(&data("carol-code.p12"), "").unwrap();
    let s = apex_sign::vba::check(&apex_sign::vba::sign(&carol, &signed).unwrap(), &signed).unwrap();
    assert!(s.code_signing && s.is_sound() && !s.is_trusted());
    assert!(s.is_trusted_with(&[carol.cert.fingerprint()], true));
}
