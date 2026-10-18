//! Signing with keys outside the process: a signer that only gets
//! digests (as a card or a store does), and a real PKCS #11 module, NSS's
//! softokn, when the system has it (with a database made for the test).
//!
//! With APEX_SIGN_OUT set to a directory, the signed files are written
//! there too, to check them with other programs (pdfsig, LibreOffice).

use rsa::traits::PublicKeyParts;

use apex_sign::crypto::{Hash, PrivKey};
use apex_sign::signer::{self, KeyKind};
use apex_sign::{Cert, Identity, SignOptions, Signer, Status};

fn data(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn alice() -> Identity {
    apex_sign::p12::read(&data("alice-aes.p12"), "secreto").unwrap()
}

fn bob() -> Identity {
    apex_sign::p12::read(&data("bob-empty.p12"), "").unwrap()
}

fn trust_test_root() {
    apex_sign::trust::add_root(Cert::from_der(&data("ca.der")).unwrap());
}

const SIGNED_AT: i64 = 1_800_000_000;

fn pdf() -> Vec<u8> {
    let mut p = apex_pdf::Pdf::new();
    p.set_info("Prueba", "ApexDesk");
    p.begin_page(595.0, 842.0);
    p.fill_rect(50.0, 50.0, 200.0, 100.0, [200, 30, 30]);
    p.finish()
}

/// A docx without signatures.
fn docx() -> Vec<u8> {
    let mut w = apex_io::zip::ZipWriter::new();
    w.add("[Content_Types].xml", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#, true);
    w.add("_rels/.rels", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#, true);
    w.add("word/document.xml", br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Firmado con una tarjeta</w:t></w:r></w:p></w:body></w:document>"#, true);
    w.finish()
}

/// Writes a signed file for other programs to check, if asked to.
fn keep(name: &str, bytes: &[u8]) {
    if let Some(dir) = std::env::var_os("APEX_SIGN_OUT") {
        std::fs::write(std::path::Path::new(&dir).join(name), bytes).unwrap();
    }
}

/// Signs as a card does: RSA over the DigestInfo it's given whole
/// (CKM_RSA_PKCS), ECDSA as r and s side by side.
struct Card {
    id: Identity,
    calls: std::sync::atomic::AtomicUsize,
}

impl Signer for Card {
    fn certificate(&self) -> &Cert {
        &self.id.cert
    }

    fn chain(&self) -> &[Cert] {
        &self.id.chain
    }

    fn sign(&self, hash: Hash, digest: &[u8]) -> Result<Vec<u8>, apex_sign::Error> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        match &self.id.key {
            PrivKey::Rsa(k) => Ok(k.sign(rsa::Pkcs1v15Sign::new_unprefixed(), &signer::digest_info(hash, digest)).unwrap()),
            PrivKey::P256(k) => {
                use p256::ecdsa::signature::hazmat::PrehashSigner;
                let s: p256::ecdsa::Signature = k.sign_prehash(digest).unwrap();
                signer::ecdsa_der(&s.to_bytes())
            }
            PrivKey::P384(_) => unreachable!(),
        }
    }
}

#[test]
fn signer_outside_the_process() {
    trust_test_root();
    for id in [alice(), bob()] {
        let name = id.cert.subject.display();
        let card = Card { id, calls: Default::default() };
        let opts = SignOptions { reason: Some("Tarjeta".into()), time: Some(SIGNED_AT), ..Default::default() };
        let signed = apex_sign::sign_pdf(&pdf(), &card, &opts).unwrap();
        let s = &apex_sign::verify(&signed)[0];
        assert_eq!((s.signer(), s.status()), (name.clone(), Status::Valid), "{:?}", s.note);
        let pkg = apex_sign::sign_package(&docx(), &card, &opts).unwrap();
        let s = &apex_sign::verify(&pkg)[0];
        assert_eq!((s.signer(), s.status()), (name, Status::Valid), "{:?}", s.note);
        assert_eq!(card.calls.load(std::sync::atomic::Ordering::Relaxed), 2);
    }
    // A key that isn't the certificate's: refused, not a broken file.
    let mut wrong = bob();
    wrong.key = PrivKey::P256(p256::ecdsa::SigningKey::from_slice(&[7; 32]).unwrap());
    let card = Card { id: wrong, calls: Default::default() };
    assert!(apex_sign::sign_pdf(&pdf(), &card, &SignOptions::default()).is_err());
    assert!(apex_sign::sign_package(&docx(), &card, &SignOptions::default()).is_err());
}

#[test]
fn certificate_facts() {
    let a = alice();
    let info = signer::CertInfo::of(&a.cert);
    assert_eq!(info.subject, "Alice Tester");
    assert_eq!(info.sha1.len(), 40);
    assert!(signer::can_sign(&a.cert, SIGNED_AT));
    // The root is a CA's: it doesn't sign documents.
    assert!(!signer::can_sign(&a.chain[0], SIGNED_AT));
    assert!(!signer::can_sign(&a.cert, a.cert.not_after + 1));
    assert_eq!(signer::chain_from(&a.cert, &a.chain).len(), 1);
    assert_eq!(KeyKind::of(&bob().cert).unwrap(), KeyKind::P256);
}

/// NSS's PKCS #11 module, if installed.
fn softokn() -> Option<std::path::PathBuf> {
    ["/usr/lib/x86_64-linux-gnu", "/usr/lib/aarch64-linux-gnu", "/usr/lib64", "/usr/lib"].iter().map(|d| std::path::Path::new(d).join("libsoftokn3.so")).find(|p| p.is_file())
}

/// Puts Alice's and Bob's keys and certificates on an NSS token with the
/// PIN `pin`.
fn fill_token(lib: &std::path::Path, params: &str, pin: &str) {
    use cryptoki::context::{CInitializeArgs, CInitializeFlags, Pkcs11};
    use cryptoki::object::{Attribute, KeyType, ObjectClass};
    use cryptoki::session::UserType;
    use cryptoki::types::AuthPin;
    let ctx = Pkcs11::new(lib).unwrap();
    let c = std::ffi::CString::new(params).unwrap();
    let args = unsafe { CInitializeArgs::new_with_reserved(CInitializeFlags::OS_LOCKING_OK, std::ptr::NonNull::new(c.as_ptr() as *mut _).unwrap()) };
    match ctx.initialize(args) {
        Ok(()) | Err(cryptoki::error::Error::Pkcs11(cryptoki::error::RvError::CryptokiAlreadyInitialized, _)) => {}
        Err(e) => panic!("{e}"),
    }
    // The database's token (the other slot is NSS's internal one).
    let slot = *ctx.get_slots_with_token().unwrap().iter().find(|s| ctx.get_token_info(**s).unwrap().label().contains("NSS Certificate DB") || ctx.get_token_info(**s).unwrap().label().contains("Software Security Device")).unwrap();
    let s = ctx.open_rw_session(slot).unwrap();
    // A new database's password is set by its security officer (empty).
    s.login(UserType::So, Some(&AuthPin::from(String::new()))).unwrap();
    s.init_pin(&AuthPin::from(pin.to_string())).unwrap();
    s.logout().unwrap();
    s.login(UserType::User, Some(&AuthPin::from(pin.to_string()))).unwrap();
    for (n, id) in [alice(), bob()].into_iter().enumerate() {
        let key_id = vec![n as u8 + 1];
        let mut key = vec![Attribute::Class(ObjectClass::PRIVATE_KEY), Attribute::Token(true), Attribute::Private(true), Attribute::Sign(true), Attribute::Id(key_id.clone())];
        match &id.key {
            PrivKey::Rsa(k) => {
                use rsa::traits::PrivateKeyParts;
                let p = k.primes();
                let (dp, dq) = (k.dp().unwrap().to_bytes_be(), k.dq().unwrap().to_bytes_be());
                let qinv = k.crt_coefficient().unwrap().to_bytes_be();
                key.extend([
                    Attribute::KeyType(KeyType::RSA),
                    Attribute::Modulus(k.n().to_bytes_be()),
                    Attribute::PublicExponent(k.e().to_bytes_be()),
                    Attribute::PrivateExponent(k.d().to_bytes_be()),
                    Attribute::Prime1(p[0].to_bytes_be()),
                    Attribute::Prime2(p[1].to_bytes_be()),
                    Attribute::Exponent1(dp),
                    Attribute::Exponent2(dq),
                    Attribute::Coefficient(qinv),
                ]);
            }
            PrivKey::P256(k) => {
                // P-256's OID, as ECParameters.
                key.extend([Attribute::KeyType(KeyType::EC), Attribute::EcParams(vec![6, 8, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 3, 1, 7]), Attribute::Value(k.to_bytes().to_vec())]);
            }
            PrivKey::P384(_) => unreachable!(),
        }
        s.create_object(&key).unwrap();
        let label = if n == 0 { "Alice" } else { "Bob" };
        s.create_object(&cert_object(&id.cert, label, Some(key_id))).unwrap();
        if n == 0 {
            // The root, as cards carry their issuers.
            s.create_object(&cert_object(&id.chain[0], "Root", None)).unwrap();
        }
    }
    s.logout().unwrap();
}

fn cert_object(c: &Cert, label: &str, id: Option<Vec<u8>>) -> Vec<cryptoki::object::Attribute> {
    use cryptoki::object::{Attribute, CertificateType, ObjectClass};
    let serial = apex_sign::asn1::tlv(apex_sign::asn1::INTEGER, &c.serial);
    let mut v = vec![
        Attribute::Class(ObjectClass::CERTIFICATE),
        Attribute::CertificateType(CertificateType::X_509),
        Attribute::Token(true),
        Attribute::Label(label.as_bytes().to_vec()),
        Attribute::Value(c.der.clone()),
        Attribute::Subject(c.subject_der().to_vec()),
        Attribute::Issuer(c.issuer_der().to_vec()),
        Attribute::SerialNumber(serial),
    ];
    v.extend(id.map(Attribute::Id));
    v
}

#[test]
fn pkcs11_token() {
    let Some(lib) = softokn() else {
        eprintln!("NSS's softokn isn't installed: PKCS #11 not tested");
        return;
    };
    trust_test_root();
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("nssdb-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let params = format!("configdir='sql:{}' certPrefix='' keyPrefix='' secmod='secmod.db' flags=''", dir.display());
    fill_token(&lib, &params, "1234");
    let found = apex_sign::pkcs11::list_with(&lib, Some(&params)).unwrap();
    let mut names: Vec<String> = found.iter().map(|k| k.cert.subject.display()).collect();
    names.sort();
    assert_eq!(names, ["Alice Tester", "Bob Tester"]);
    assert!(found.iter().all(|k| k.needs_pin && k.slot().is_some()));
    for k in &found {
        assert!(matches!(k.signer(None), Err(apex_sign::store::KeyError::NeedPin)));
        assert!(matches!(k.signer(Some("0000")), Err(apex_sign::store::KeyError::WrongPin)));
        let signer = k.signer(Some("1234")).unwrap();
        let who = k.cert.subject.display();
        if who == "Alice Tester" {
            assert_eq!(signer.chain().len(), 1, "the root came from the token");
        }
        let opts = SignOptions { reason: Some("PKCS #11".into()), time: Some(SIGNED_AT), ..Default::default() };
        let signed = apex_sign::sign_pdf(&pdf(), signer.as_ref(), &opts).unwrap();
        let s = &apex_sign::verify(&signed)[0];
        assert_eq!((s.signer(), s.status()), (who.clone(), Status::Valid), "{:?}", s.note);
        let pkg = apex_sign::sign_package(&docx(), signer.as_ref(), &opts).unwrap();
        let s = &apex_sign::verify(&pkg)[0];
        assert_eq!((s.signer(), s.status()), (who.clone(), Status::Valid), "{:?}", s.note);
        let tag = if who.starts_with("Alice") { "rsa" } else { "ecdsa" };
        keep(&format!("pkcs11-{tag}.pdf"), &signed);
        keep(&format!("pkcs11-{tag}.docx"), &pkg);
        // Next to LibreOffice's own signatures.
        for name in ["signed.xlsx", "signed.odt"] {
            let both = apex_sign::sign_package(&data(name), signer.as_ref(), &SignOptions::default()).unwrap();
            assert!(apex_sign::verify(&both).iter().all(|s| s.status() == Status::Valid), "{name}");
            keep(&format!("pkcs11-{tag}-{name}"), &both);
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
