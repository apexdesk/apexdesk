//! Whether a signer's certificate is trusted: a chain from it, through
//! the certificates that came with the signature, up to a root of the
//! system's store (Windows and macOS keychains, the Linux CA bundle).
//! Revocation (CRL, OCSP) is judged from the answers the document keeps
//! ([`crate::ltv`]).

use std::sync::OnceLock;

use crate::x509::Cert;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trust {
    /// Chains up to this root of the system.
    Trusted(String),
    /// The chain doesn't reach a trusted root.
    UnknownIssuer,
    /// A certificate signed by itself that the system doesn't trust.
    SelfSigned,
    /// Trusted, but issued for another use (a web server's).
    WrongUse,
    /// A certificate of the chain was expired (or not yet valid) at the
    /// signing time.
    Expired,
    /// The system's roots couldn't be read: nothing was verified.
    NotVerified,
    /// A certificate of the chain was revoked (at this time) before the
    /// document was signed.
    Revoked(i64),
}

impl Trust {
    pub fn is_trusted(&self) -> bool {
        matches!(self, Trust::Trusted(_))
    }
}

static ROOTS: OnceLock<Option<Vec<Cert>>> = OnceLock::new();
static EXTRA: OnceLock<std::sync::Mutex<Vec<Cert>>> = OnceLock::new();

/// The system's trusted roots (read once).
fn roots() -> Option<&'static [Cert]> {
    ROOTS.get_or_init(|| {
        // The browser keeps its roots to itself.
        #[cfg(target_arch = "wasm32")]
        {
            None
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let found = rustls_native_certs::load_native_certs();
            if found.certs.is_empty() {
                return None;
            }
            Some(found.certs.iter().filter_map(|c| Cert::from_der(c.as_ref()).ok()).collect())
        }
    }).as_deref()
}

/// How many roots [`add_root`] has added (results worked out with fewer
/// may be stale).
pub fn roots_added() -> usize {
    EXTRA.get().map_or(0, |e| e.lock().unwrap_or_else(|e| e.into_inner()).len())
}

/// Trusts `cert` as a root too, for this process (the tests' own CA).
pub fn add_root(cert: Cert) {
    EXTRA.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner()).push(cert);
}

/// Trusts the certificates in the files of `dir` (.cer, .crt, .der, .pem;
/// PEM files may hold several), as an organization's own roots. Returns
/// how many were added.
pub fn add_roots_from(dir: &std::path::Path) -> usize {
    let Ok(rd) = apex_fs::read_dir(dir) else { return 0 };
    let mut n = 0;
    for p in rd.flatten().map(|e| e.path()) {
        let ext = p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
        if !matches!(ext.as_str(), "cer" | "crt" | "der" | "pem") {
            continue;
        }
        let Ok(data) = apex_fs::read(&p) else { continue };
        for der in certificates_in(&data) {
            if let Ok(c) = Cert::from_der(&der) {
                add_root(c);
                n += 1;
            }
        }
    }
    n
}

/// The DER certificates of a file: PEM blocks, or the file itself.
fn certificates_in(data: &[u8]) -> Vec<Vec<u8>> {
    let Ok(text) = std::str::from_utf8(data) else { return vec![data.to_vec()] };
    if !text.contains("-----BEGIN CERTIFICATE-----") {
        return vec![data.to_vec()];
    }
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("-----BEGIN CERTIFICATE-----") {
        let body = &rest[i + 27..];
        let Some(j) = body.find("-----END CERTIFICATE-----") else { break };
        out.extend(crate::base64(&body[..j]));
        rest = &body[j..];
    }
    out
}

fn extra() -> Vec<Cert> {

    EXTRA.get().map(|m| m.lock().unwrap_or_else(|e| e.into_inner()).clone()).unwrap_or_default()
}

/// Whether `cert` is one of the trusted roots.
pub fn is_root(cert: &Cert) -> bool {
    extra().iter().chain(roots().unwrap_or(&[])).any(|r| r.der == cert.der)
}

/// The chain of `leaf` (leaf first, root last when found) and its trust
/// at time `at`, using `pool` for the intermediate certificates.
pub fn check(leaf: &Cert, pool: &[Cert], at: i64) -> (Vec<Cert>, Trust) {
    let extra = extra();
    let system = roots();
    let trusted: Vec<&Cert> = extra.iter().chain(system.unwrap_or(&[])).collect();
    let mut chain = vec![leaf.clone()];
    let mut expired = false;
    loop {
        let cur = chain.last().expect("chain has the leaf");
        expired |= !cur.valid_at(at);
        // The certificate itself is a trusted root.
        if let Some(root) = trusted.iter().find(|r| r.der == cur.der) {
            let name = root.subject.display();
            let t = if expired { Trust::Expired } else if !within_limits(&chain) { Trust::WrongUse } else { Trust::Trusted(name) };
            return (chain, t);
        }
        // Issued by a trusted root.
        if let Some(root) = trusted.iter().find(|r| r.named_issuer_of(cur) && r.signed(cur)) {
            let root = (*root).clone();
            expired |= !root.valid_at(at);
            let name = root.subject.display();
            chain.push(root);
            let t = if expired { Trust::Expired } else if !within_limits(&chain) { Trust::WrongUse } else { Trust::Trusted(name) };
            return (chain, t);
        }
        if cur.self_issued() {
            let t = if system.is_none() && extra.is_empty() {
                Trust::NotVerified
            } else if chain.len() == 1 {
                Trust::SelfSigned
            } else {
                Trust::UnknownIssuer
            };
            return (chain, t);
        }
        // Up through an intermediate that came with the signature.
        let next = pool.iter().find(|c| c.ca && may_sign_certificates(c) && !chain.iter().any(|x| x.der == c.der) && c.named_issuer_of(cur) && c.signed(cur));
        match next {
            Some(n) if chain.len() < 10 => chain.push(n.clone()),
            _ => {
                let t = if system.is_none() && extra.is_empty() { Trust::NotVerified } else { Trust::UnknownIssuer };
                return (chain, t);
            }
        }
    }
}

/// Whether the CAs between the leaf and the root (the last) allow the
/// leaf: no more CAs below one than its path length, uses its own
/// extended key usage allows, names within their name constraints, and
/// no limits this program can't check.
fn within_limits(chain: &[Cert]) -> bool {
    let Some(leaf) = chain.first() else { return true };
    if chain.len() > 1 && leaf.unchecked_limits {
        return false;
    }
    let below = chain.len().saturating_sub(1);
    chain.iter().enumerate().take(below).skip(1).all(|(k, c)| {
        let uses = c.eku.is_empty() || c.eku.iter().any(|u| u == "2.5.29.37.0") || !leaf.eku.is_empty() && leaf.eku.iter().all(|u| c.eku.contains(u));
        // Self-issued ones (a CA's new key) don't count toward it.
        let cas = chain[1..k].iter().filter(|x| !x.self_issued()).count() as u64;
        // Name constraints hold for every certificate below it but a
        // CA's new key (the leaf always).
        let names = c.name_limits.as_ref().is_none_or(|l| chain[..k].iter().enumerate().all(|(i, x)| (i > 0 && x.self_issued()) || l.allow(&x.names)));
        !c.unchecked_limits && uses && names && c.path_len.is_none_or(|n| cas <= n)
    })
}

/// Key usage bits as [`Cert::key_usage`] keeps them (the first two bytes).
const DIGITAL_SIGNATURE: u16 = 0x8000;
const NON_REPUDIATION: u16 = 0x4000;
const KEY_CERT_SIGN: u16 = 0x0400;

/// Whether an intermediate may issue certificates: a CA whose key usage,
/// when it has one, allows it.
fn may_sign_certificates(c: &Cert) -> bool {
    c.key_usage.is_none_or(|k| k & KEY_CERT_SIGN != 0)
}

/// Whether `c` may sign documents: its key usage, when given, allows
/// signatures, and its extended key usage, when given, isn't only for
/// other things (a web server's certificate chains to a trusted root
/// too). Personal certificates for e-mail or client sign-in (national
/// ID cards) and those for documents pass.
pub fn fit_for_documents(c: &Cert) -> bool {
    const FOR_SIGNING: &[&str] = &[
        "2.5.29.37.0",               // any purpose
        "1.3.6.1.5.5.7.3.2",         // client authentication
        "1.3.6.1.5.5.7.3.4",         // e-mail protection
        "1.3.6.1.5.5.7.3.36",        // document signing
        "1.3.6.1.4.1.311.10.3.12",   // Microsoft document signing
        "1.2.840.113583.1.1.5",      // Adobe authentic documents
    ];
    const CLIENT: &str = "1.3.6.1.5.5.7.3.2";
    const SERVER: &str = "1.3.6.1.5.5.7.3.1";
    let has = |oid: &str| c.eku.iter().any(|e| e == oid);
    // Client sign-in alone vouches for a person only without the web
    // server's purpose (public TLS certificates often carry both).
    let for_signing = c.eku.iter().any(|e| FOR_SIGNING.contains(&e.as_str()) && (e != CLIENT || !has(SERVER)));
    c.key_usage.is_none_or(|k| k & (DIGITAL_SIGNATURE | NON_REPUDIATION) != 0) && (c.eku.is_empty() || for_signing)
}

/// Whether `c` may time-stamp: its extended key usage says so (RFC 3161
/// requires it of a TSA).
pub fn fit_for_timestamping(c: &Cert) -> bool {
    c.eku.iter().any(|e| e == "1.3.6.1.5.5.7.3.8")
}

/// Whether `c` may sign code (VBA projects): its key usage, when given,
/// allows signatures, and its extended key usage names code signing. A
/// web server's certificate doesn't.
pub fn fit_for_code_signing(c: &Cert) -> bool {
    const CODE_SIGNING: &str = "1.3.6.1.5.5.7.3.3";
    // The purpose must be said: a certificate for any use (no EKU, or
    // anyExtendedKeyUsage) is not one issued for code, and code-signing
    // certificates from public CAs always say so.
    c.key_usage.is_none_or(|k| k & DIGITAL_SIGNATURE != 0) && c.eku.iter().any(|e| e == CODE_SIGNING)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An intermediate kept to `.example.com` mail and `O=Acme` (made with
    /// OpenSSL) vouches for names within them only.
    #[test]
    fn name_constraints() {
        let cert = |b: &[u8]| Cert::from_der(b).unwrap();
        let root = cert(include_bytes!("../tests/data/nc-root.der"));
        let inter = cert(include_bytes!("../tests/data/nc-inter.der"));
        assert!(!inter.unchecked_limits && inter.name_limits.is_some());
        let ok = cert(include_bytes!("../tests/data/nc-leaf_ok.der"));
        assert!(within_limits(&[ok, inter.clone(), root.clone()]));
        let other_mail = cert(include_bytes!("../tests/data/nc-leaf_bad.der"));
        assert!(!within_limits(&[other_mail, inter.clone(), root.clone()]));
        let other_org = cert(include_bytes!("../tests/data/nc-leaf_dir.der"));
        assert!(!within_limits(&[other_org, inter, root]));
    }
}
