//! Long-term signatures (PAdES B-LT and B-LTA, XAdES-T, -XL and -A)
//! against a test PKI made here: a root that answers OCSP and publishes a
//! CRL, a signer and a time-stamp authority, all reached through a fake
//! network. `APEXDESK_NET_TESTS=1` adds a test against a free public
//! time-stamp authority.

use std::sync::Mutex;

use apex_sign::asn1;
use apex_sign::crypto::{Hash, PrivKey};
use apex_sign::net::Net;
use apex_sign::{Cert, Error, Identity, Kind, Level, Revocation, SignOptions, Status, Trust};

const OCSP_URL: &str = "http://ocsp.test/";
const CRL_URL: &str = "http://crl.test/root.crl";
const TSA_URL: &str = "http://tsa.test/";
/// When the fake network lives (the certificates last till 2033).
const START: i64 = 1_790_000_000;
/// The revoked signer's revocation time.
const REVOKED_AT: i64 = 1_750_000_000;

fn key(seed: u8) -> PrivKey {
    PrivKey::P256(p256::SecretKey::from_slice(&[seed; 32]).unwrap().into())
}

fn name(cn: &str) -> Vec<u8> {
    asn1::seq(&[&asn1::set_of(&[&asn1::seq(&[&asn1::oid("2.5.4.3"), &asn1::utf8(cn)])])])
}

fn ext(oid: &str, value: &[u8]) -> Vec<u8> {
    asn1::seq(&[&asn1::oid(oid), &asn1::octets(value)])
}

/// A certificate for `seed`'s key, signed by `issuer` (itself if `None`).
fn cert(seed: u8, cn: &str, serial: u64, issuer: Option<(&PrivKey, &Cert)>, ca: bool, extra: &[Vec<u8>]) -> Identity {
    let k = key(seed);
    use p256::pkcs8::EncodePublicKey;
    let spki = p256::SecretKey::from_slice(&[seed; 32]).unwrap().public_key().to_public_key_der().unwrap();
    let subject = name(cn);
    let issuer_dn = issuer.map_or(subject.clone(), |(_, c)| c.subject_der().to_vec());
    let mut exts = vec![ext("2.5.29.19", &asn1::seq(&if ca { vec![asn1::boolean(true)] } else { Vec::new() }.iter().map(Vec::as_slice).collect::<Vec<_>>()))];
    exts.extend(extra.iter().cloned());
    let ext_refs: Vec<&[u8]> = exts.iter().map(Vec::as_slice).collect();
    let validity = asn1::seq(&[&asn1::utc_time(1_700_000_000), &asn1::utc_time(2_000_000_000)]);
    let signing = issuer.map_or(&k, |(ik, _)| ik);
    let tbs = asn1::seq(&[&asn1::ctx(0, &[&asn1::uint(2)]), &asn1::uint(serial), &signing.alg(), &issuer_dn, &validity, &subject, spki.as_bytes(), &asn1::ctx(3, &[&asn1::seq(&ext_refs)])]);
    let der = apex_sign::cms::sign_x509(signing, &tbs).unwrap();
    let chain = issuer.map(|(_, c)| vec![c.clone()]).unwrap_or_default();
    Identity { key: k, cert: Cert::from_der(&der).unwrap(), chain }
}

/// Where to check an end certificate: the root's OCSP responder and CRL.
fn checked_at() -> Vec<Vec<u8>> {
    let aia = asn1::seq(&[&asn1::seq(&[&asn1::oid("1.3.6.1.5.5.7.48.1"), &asn1::tlv(0x86, OCSP_URL.as_bytes())])]);
    let dp = asn1::seq(&[&asn1::seq(&[&asn1::ctx(0, &[&asn1::ctx(0, &[&asn1::tlv(0x86, CRL_URL.as_bytes())])])])]);
    vec![ext("1.3.6.1.5.5.7.1.1", &aia), ext("2.5.29.31", &dp)]
}

struct Pki {
    root: Identity,
    signer: Identity,
    revoked: Identity,
    tsa: Identity,
}

fn pki() -> Pki {
    let root = cert(11, "LTV Test Root", 1, None, true, &[]);
    let signer = cert(12, "Carmen Firmante", 2, Some((&root.key, &root.cert)), false, &checked_at());
    let revoked = cert(13, "Revocada", 3, Some((&root.key, &root.cert)), false, &checked_at());
    // The authority is checked through the CRL only.
    let dp = asn1::seq(&[&asn1::seq(&[&asn1::ctx(0, &[&asn1::ctx(0, &[&asn1::tlv(0x86, CRL_URL.as_bytes())])])])]);
    let eku = asn1::seq(&[&asn1::oid("1.3.6.1.5.5.7.3.8")]);
    let tsa = cert(14, "LTV Test TSA", 4, Some((&root.key, &root.cert)), false, &[ext("2.5.29.31", &dp), ext("2.5.29.37", &eku)]);
    apex_sign::trust::add_root(root.cert.clone());
    Pki { root, signer, revoked, tsa }
}

/// The network of the test PKI: every answer made on the spot, the clock
/// moving a minute with each.
struct Fake<'a> {
    pki: &'a Pki,
    clock: Mutex<i64>,
    asked: Mutex<Vec<String>>,
}

impl Fake<'_> {
    fn new(pki: &Pki) -> Fake<'_> {
        Fake { pki, clock: Mutex::new(START), asked: Mutex::new(Vec::new()) }
    }

    fn tick(&self) -> i64 {
        let mut c = self.clock.lock().unwrap();
        *c += 60;
        *c
    }

    fn revoked_serial(&self) -> Vec<u8> {
        self.pki.revoked.cert.serial.iter().copied().skip_while(|&b| b == 0).collect()
    }

    fn ocsp(&self, req: &[u8]) -> Vec<u8> {
        let now = self.tick();
        let r = asn1::parse(req).unwrap();
        let cert_id = r.children().next().unwrap().children().next().unwrap().children().next().unwrap().children().next().unwrap().raw.to_vec();
        let serial = {
            let id = asn1::parse(&cert_id).unwrap();
            let mut c = id.children();
            c.next();
            c.next();
            c.next();
            c.next().unwrap().body.to_vec()
        };
        let status = if serial == self.revoked_serial() { asn1::ctx(1, &[&asn1::generalized_time(REVOKED_AT)]) } else { vec![0x80, 0x00] };
        let single = asn1::seq(&[&cert_id, &status, &asn1::generalized_time(now), &asn1::ctx(0, &[&asn1::generalized_time(now + 86_400)])]);
        let data = asn1::seq(&[&asn1::ctx(1, &[self.pki.root.cert.subject_der()]), &asn1::generalized_time(now), &asn1::seq(&[&single])]);
        let basic = apex_sign::cms::sign_x509(&self.pki.root.key, &data).unwrap();
        let bytes = asn1::seq(&[&asn1::oid("1.3.6.1.5.5.7.48.1.1"), &asn1::octets(&basic)]);
        asn1::seq(&[&asn1::tlv(0x0A, &[0]), &asn1::ctx(0, &[&bytes])])
    }

    fn crl(&self) -> Vec<u8> {
        let now = self.tick();
        let entry = asn1::seq(&[&asn1::uint_bytes(&self.revoked_serial()), &asn1::utc_time(REVOKED_AT)]);
        let tbs = asn1::seq(&[&asn1::uint(1), &self.pki.root.key.alg(), self.pki.root.cert.subject_der(), &asn1::utc_time(now), &asn1::utc_time(now + 86_400 * 7), &asn1::seq(&[&entry])]);
        apex_sign::cms::sign_x509(&self.pki.root.key, &tbs).unwrap()
    }

    fn stamp(&self, req: &[u8]) -> Vec<u8> {
        let time = self.tick();
        let tsa = &self.pki.tsa;
        let req = asn1::parse(req).unwrap();
        let mut r = req.children();
        r.next();
        let imprint = r.next().unwrap().raw.to_vec();
        let nonce = r.next().unwrap().raw.to_vec();
        let info = asn1::seq(&[&asn1::uint(1), &asn1::oid("1.2.3.4"), &imprint, &asn1::uint(time as u64), &asn1::generalized_time(time), &nonce]);
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
        asn1::seq(&[&asn1::seq(&[&asn1::uint(0)]), &token])
    }

    fn asked(&self, what: &str) -> usize {
        self.asked.lock().unwrap().iter().filter(|a| a.starts_with(what)).count()
    }
}

impl Net for Fake<'_> {
    fn post(&self, url: &str, content_type: &str, body: &[u8]) -> Result<Vec<u8>, Error> {
        self.asked.lock().unwrap().push(url.to_string());
        match (url, content_type) {
            (OCSP_URL, "application/ocsp-request") => Ok(self.ocsp(body)),
            (TSA_URL, "application/timestamp-query") => Ok(self.stamp(body)),
            _ => Err(Error::new(format!("{url}: not found"))),
        }
    }

    fn get(&self, url: &str) -> Result<Vec<u8>, Error> {
        self.asked.lock().unwrap().push(url.to_string());
        if url == CRL_URL { Ok(self.crl()) } else { Err(Error::new(format!("{url}: not found"))) }
    }
}

fn pdf() -> Vec<u8> {
    let mut p = apex_pdf::Pdf::new();
    p.set_info("Prueba", "ApexDesk");
    p.begin_page(595.0, 842.0);
    p.fill_rect(50.0, 50.0, 200.0, 100.0, [200, 30, 30]);
    p.finish()
}

fn opts(level: Level) -> SignOptions {
    SignOptions { tsa_url: Some(TSA_URL.into()), level, ..Default::default() }
}

/// The signatures that aren't document time-stamps.
fn signed_by_people(data: &[u8]) -> Vec<apex_sign::Signature> {
    apex_sign::verify(data).into_iter().filter(|s| s.kind != Kind::PdfTimestamp).collect()
}

#[test]
fn pdf_levels() {
    let pki = pki();
    let net = Fake::new(&pki);
    // B-T: a time, no validation data.
    let t = apex_sign::sign_pdf_with(&pdf(), &pki.signer, &opts(Level::T), &net).unwrap();
    let s = &signed_by_people(&t)[0];
    assert_eq!(s.status(), Status::Valid, "{:?}", s.note);
    assert_eq!((s.level, s.revocation), (Level::T, Revocation::NotChecked));
    assert_eq!(s.level_name(), "PAdES B-T");
    // Extended to B-LT: the DSS answers for the signer and the authority.
    let lt = apex_sign::extend_with(&t, Level::Lt, Some(TSA_URL), &net).unwrap();
    assert!(lt.starts_with(&t), "an incremental update");
    let s = &signed_by_people(&lt)[0];
    assert_eq!(s.status(), Status::Valid, "{:?}", s.note);
    assert_eq!(s.level, Level::Lt);
    assert!(matches!(s.revocation, Revocation::Good(t) if t > START));
    assert_eq!(s.verifiable_until, Some(2_000_000_000));
    // The CRL answers for both (the authority has no OCSP responder).
    assert!(net.asked(CRL_URL) >= 1);
    // B-LTA, then renewed: a newer archive time-stamp each time.
    let lta = apex_sign::extend_with(&lt, Level::Lta, Some(TSA_URL), &net).unwrap();
    let s = &signed_by_people(&lta)[0];
    assert_eq!((s.status(), s.level), (Status::Valid, Level::Lta), "{:?}", s.note);
    let first = s.archive_time.unwrap();
    let renewed = apex_sign::extend_with(&lta, Level::Lta, Some(TSA_URL), &net).unwrap();
    let s = &signed_by_people(&renewed)[0];
    assert_eq!(s.level, Level::Lta);
    assert!(s.archive_time.unwrap() > first);
    let stamps: Vec<_> = apex_sign::verify(&renewed).into_iter().filter(|s| s.kind == Kind::PdfTimestamp).collect();
    assert_eq!(stamps.len(), 2);
    assert!(stamps.iter().all(|s| s.status() == Status::Valid), "{:?}", stamps.iter().map(|s| &s.note).collect::<Vec<_>>());
    // Signed at B-LTA at once.
    let direct = apex_sign::sign_pdf_with(&pdf(), &pki.signer, &opts(Level::Lta), &net).unwrap();
    let s = &signed_by_people(&direct)[0];
    assert_eq!((s.status(), s.level), (Status::Valid, Level::Lta), "{:?}", s.note);
    // A level from T up needs an authority.
    let none = SignOptions { level: Level::Lt, ..Default::default() };
    assert!(apex_sign::sign_pdf_with(&pdf(), &pki.signer, &none, &net).is_err());
    assert!(apex_sign::extend_with(&t, Level::Lta, None, &net).is_err());
}

#[test]
fn pdf_revoked_signer() {
    let pki = pki();
    let net = Fake::new(&pki);
    let signed = apex_sign::sign_pdf_with(&pdf(), &pki.revoked, &opts(Level::Lt), &net).unwrap();
    let s = &signed_by_people(&signed)[0];
    assert_eq!(s.revocation, Revocation::Revoked(REVOKED_AT));
    assert_eq!(s.trust, Trust::Revoked(REVOKED_AT));
    assert_eq!(s.status(), Status::Invalid);
}

#[test]
fn pdf_without_network() {
    let pki = pki();
    struct Down;
    impl Net for Down {
        fn post(&self, url: &str, _: &str, _: &[u8]) -> Result<Vec<u8>, Error> {
            Err(Error::new(format!("{url}: unreachable")))
        }
        fn get(&self, url: &str) -> Result<Vec<u8>, Error> {
            Err(Error::new(format!("{url}: unreachable")))
        }
    }
    let net = Fake::new(&pki);
    let t = apex_sign::sign_pdf_with(&pdf(), &pki.signer, &opts(Level::T), &net).unwrap();
    // Nothing is written half: the error says why.
    let e = apex_sign::extend_with(&t, Level::Lt, Some(TSA_URL), &Down).unwrap_err();
    assert!(e.to_string().contains("unreachable"), "{e}");
}

/// A package with nothing signed.
fn package(name: &str) -> Vec<u8> {
    let pkg = std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap();
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
fn package_levels() {
    let pki = pki();
    let net = Fake::new(&pki);
    for name in ["signed.docx", "signed.odt"] {
        let pkg = package(name);
        // XAdES-T when signing.
        let t = apex_sign::sign_package_with(&pkg, &pki.signer, &opts(Level::T), &net).unwrap();
        let s = &apex_sign::verify(&t)[0];
        assert_eq!(s.status(), Status::Valid, "{name}: {:?}", s.note);
        assert!(s.timestamped && s.time.is_some_and(|t| t > START), "{name}");
        assert_eq!(s.level_name(), "XAdES-T");
        // XAdES-XL: references, values and their time-stamp.
        let xl = apex_sign::extend_with(&t, Level::Lt, Some(TSA_URL), &net).unwrap();
        let s = &apex_sign::verify(&xl)[0];
        assert_eq!((s.status(), s.level), (Status::Valid, Level::Lt), "{name}: {:?}", s.note);
        assert!(matches!(s.revocation, Revocation::Good(_)));
        assert_eq!(s.level_name(), "XAdES-XL");
        // XAdES-A, renewed.
        let a = apex_sign::extend_with(&xl, Level::Lta, Some(TSA_URL), &net).unwrap();
        let s = &apex_sign::verify(&a)[0];
        assert_eq!((s.status(), s.level), (Status::Valid, Level::Lta), "{name}: {:?}", s.note);
        let first = s.archive_time.unwrap();
        let again = apex_sign::extend_with(&a, Level::Lta, Some(TSA_URL), &net).unwrap();
        let s = &apex_sign::verify(&again)[0];
        assert_eq!((s.status(), s.level), (Status::Valid, Level::Lta), "{name}: {:?}", s.note);
        assert!(s.archive_time.unwrap() > first);
        // A changed document breaks the archive time-stamp too.
        let z = apex_io::zip::ZipReader::new(&again).unwrap();
        let part = z.names().find(|n| n.ends_with("document.xml") || *n == "content.xml").unwrap().to_string();
        let mut body = z.read_string(&part).unwrap();
        body.insert(body.rfind("</").unwrap(), 'x');
        let mut w = apex_io::zip::ZipWriter::new();
        for n in z.names() {
            let d = if n == part { body.clone().into_bytes() } else { z.read(n).unwrap() };
            w.add(n, &d, n != "mimetype");
        }
        let broken = w.finish();
        assert_eq!(apex_sign::verify(&broken)[0].status(), Status::Invalid, "{name}");
        assert!(apex_sign::extend_with(&broken, Level::Lta, Some(TSA_URL), &net).is_err());
        // Straight to XAdES-A, and a second signer keeps the first intact.
        let direct = apex_sign::sign_package_with(&pkg, &pki.signer, &opts(Level::Lta), &net).unwrap();
        let two = apex_sign::sign_package_with(&direct, &pki.signer, &opts(Level::Lt), &net).unwrap();
        let sigs = apex_sign::verify(&two);
        let got: Vec<(Status, Level)> = sigs.iter().map(|s| (s.status(), s.level)).collect();
        assert_eq!(got, [(Status::Valid, Level::Lta), (Status::Valid, Level::Lt)], "{name}");
    }
}

#[test]
fn revocation_answers() {
    use apex_sign::revocation::{self, CertStatus, Source};
    let pki = pki();
    let net = Fake::new(&pki);
    assert_eq!(pki.signer.cert.ocsp_urls, [OCSP_URL]);
    assert_eq!(pki.signer.cert.crl_urls, [CRL_URL]);
    // OCSP first.
    let (source, der, ev) = revocation::fetch(&net, &pki.signer.cert, &pki.root.cert, &[]).unwrap();
    assert!(source == Source::Ocsp && ev.status == CertStatus::Good);
    assert!(revocation::check_ocsp(&der, &pki.signer.cert, &pki.root.cert, &[]).is_some());
    // Not for another certificate, nor signed by another issuer.
    assert!(revocation::check_ocsp(&der, &pki.revoked.cert, &pki.root.cert, &[]).is_none());
    assert!(revocation::check_ocsp(&der, &pki.signer.cert, &pki.tsa.cert, &[]).is_none());
    let (_, _, ev) = revocation::fetch(&net, &pki.revoked.cert, &pki.root.cert, &[]).unwrap();
    assert_eq!(ev.status, CertStatus::Revoked(REVOKED_AT));
    // The CRL.
    let crl = net.crl();
    assert_eq!(revocation::check_crl(&crl, &pki.signer.cert, &pki.root.cert).unwrap().status, CertStatus::Good);
    assert_eq!(revocation::check_crl(&crl, &pki.revoked.cert, &pki.root.cert).unwrap().status, CertStatus::Revoked(REVOKED_AT));
    assert!(revocation::check_crl(&crl, &pki.signer.cert, &pki.tsa.cert).is_none());
}

/// Against a free public authority (FreeTSA), when asked for.
#[test]
fn public_time_stamp() {
    if std::env::var("APEXDESK_NET_TESTS").is_err() {
        return;
    }
    let pki = pki();
    let opts = SignOptions { tsa_url: Some("https://freetsa.org/tsr".into()), ..Default::default() };
    let signed = apex_sign::sign_pdf(&pdf(), &pki.signer, &opts).unwrap();
    let s = &apex_sign::verify(&signed)[0];
    assert!(s.timestamped && s.tsa.is_some());
}
