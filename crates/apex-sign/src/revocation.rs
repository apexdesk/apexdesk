//! Whether a certificate was revoked: OCSP (RFC 6960) and CRLs (RFC 5280).
//! The answers are asked for when signing for the long term and kept in
//! the document; checking reads them back from there.

use crate::Error;
use crate::asn1::{self, Tlv};
use crate::crypto::Hash;
use crate::net::Net;
use crate::x509::{Cert, Name};

const OCSP_BASIC: &str = "1.3.6.1.5.5.7.48.1.1";
const OCSP_SIGNING: &str = "1.3.6.1.5.5.7.3.9";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Ocsp,
    Crl,
}

/// What an answer says of one certificate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CertStatus {
    Good,
    /// Revoked at this time.
    Revoked(i64),
    Unknown,
}

/// An answer about one certificate, its signature checked.
#[derive(Clone, Debug)]
pub struct Evidence {
    pub source: Source,
    pub status: CertStatus,
    /// When the information was current.
    pub this_update: i64,
    pub next_update: Option<i64>,
    /// An OCSP responder other than the issuer (delegated), whose own
    /// revocation may need checking too.
    pub responder: Option<Cert>,
}

// --- OCSP ----------------------------------------------------------------------------

/// The CertID of `cert` (issued by `issuer`), with SHA-1 as responders
/// all take.
fn cert_id(cert: &Cert, issuer: &Cert) -> Vec<u8> {
    let h = Hash::Sha1;
    asn1::seq(&[&asn1::alg(h.oid(), true), &asn1::octets(&h.digest(issuer.subject_der())), &asn1::octets(&h.digest(&issuer.key_bits())), &asn1::tlv(asn1::INTEGER, &cert.serial)])
}

/// An OCSP request for `cert`.
pub fn ocsp_request(cert: &Cert, issuer: &Cert) -> Vec<u8> {
    let request = asn1::seq(&[&cert_id(cert, issuer)]);
    asn1::seq(&[&asn1::seq(&[&asn1::seq(&[&request])])])
}

/// Who signed an OCSP response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Responder {
    /// The Name as written.
    ByName(Vec<u8>),
    /// SHA-1 of the responder's key.
    ByKey(Vec<u8>),
}

struct Single {
    hash: Option<Hash>,
    name_hash: Vec<u8>,
    key_hash: Vec<u8>,
    serial: Vec<u8>,
    status: CertStatus,
    this_update: i64,
    next_update: Option<i64>,
}

/// A BasicOCSPResponse read from an OCSPResponse.
pub struct Ocsp {
    tbs: Vec<u8>,
    alg: Vec<u8>,
    sig: Vec<u8>,
    pub certs: Vec<Cert>,
    pub responder: Responder,
    pub produced: i64,
    singles: Vec<Single>,
}

fn trim(b: &[u8]) -> &[u8] {
    let n = b.iter().take_while(|&&x| x == 0).count();
    &b[n.min(b.len().saturating_sub(1))..]
}

/// Reads an OCSPResponse (only successful basic ones).
pub fn parse_ocsp(der: &[u8]) -> Option<Ocsp> {
    let resp = asn1::parse(der).ok()?;
    let mut r = resp.children();
    let status = r.next()?;
    if status.tag != 0x0A || status.body != [0] {
        return None;
    }
    let bytes = r.take(0xA0)?.inner().ok()?;
    let mut br = bytes.children();
    if br.next()?.oid() != OCSP_BASIC {
        return None;
    }
    let basic = asn1::parse(&br.next()?.octets()).ok()?.raw.to_vec();
    parse_basic(&basic)
}

fn parse_basic(basic: &[u8]) -> Option<Ocsp> {
    let b = asn1::parse(basic).ok()?;
    let mut r = b.children();
    let tbs = r.next()?;
    let alg = r.next()?.raw.to_vec();
    let sig = r.next()?.bits().to_vec();
    let mut certs = Vec::new();
    if let Some(c) = r.take(0xA0)
        && let Ok(list) = c.inner()
    {
        let mut cr = list.children();
        while let Some(x) = cr.next() {
            if let Ok(cert) = Cert::from_der(x.raw) {
                certs.push(cert);
            }
        }
    }
    let mut t = tbs.children();
    t.take(0xA0);
    let rid = t.next()?;
    let responder = match rid.tag {
        0xA1 => Responder::ByName(rid.inner().ok()?.raw.to_vec()),
        0xA2 => Responder::ByKey(rid.inner().ok()?.octets().into_owned()),
        _ => return None,
    };
    let produced = t.next()?.time()?;
    let list = t.next()?;
    let mut singles = Vec::new();
    let mut lr = list.children();
    while let Some(s) = lr.next() {
        if let Some(x) = single(&s) {
            singles.push(x);
        }
    }
    Some(Ocsp { tbs: tbs.raw.to_vec(), alg, sig, certs, responder, produced, singles })
}

fn single(s: &Tlv) -> Option<Single> {
    let mut r = s.children();
    let id = r.next()?;
    let mut ir = id.children();
    let hash = Hash::from_alg(&ir.next()?);
    let name_hash = ir.next()?.body.to_vec();
    let key_hash = ir.next()?.body.to_vec();
    let serial = ir.next()?.body.to_vec();
    let st = r.next()?;
    let status = match st.tag {
        0x80 => CertStatus::Good,
        0xA1 => CertStatus::Revoked(st.children().next()?.time()?),
        _ => CertStatus::Unknown,
    };
    let this_update = r.next()?.time()?;
    let next_update = r.take(0xA0).and_then(|n| n.inner().ok()).and_then(|t| t.time());
    Some(Single { hash, name_hash, key_hash, serial, status, this_update, next_update })
}

impl Ocsp {
    /// Whether `c` is the responder the response names.
    fn named(&self, c: &Cert) -> bool {
        match &self.responder {
            Responder::ByKey(k) => Hash::Sha1.digest(&c.key_bits()) == *k,
            Responder::ByName(n) => c.subject_der() == n.as_slice() || asn1::parse(n).is_ok_and(|t| Name::parse(&t) == c.subject),
        }
    }

    /// The certificate that signed the response: the issuer itself, or
    /// one it authorized to answer for it.
    fn signer(&self, issuer: &Cert, pool: &[Cert]) -> Option<Cert> {
        if self.named(issuer) && issuer.verifies(&self.tbs, &self.alg, &self.sig) {
            return Some(issuer.clone());
        }
        self.certs.iter().chain(pool).find(|c| self.named(c) && c.eku.iter().any(|e| e == OCSP_SIGNING) && issuer.signed(c) && c.verifies(&self.tbs, &self.alg, &self.sig)).cloned()
    }
}

/// What an OCSP response (DER OCSPResponse) says of `cert`, if it answers
/// for it with a right signature.
pub fn check_ocsp(der: &[u8], cert: &Cert, issuer: &Cert, pool: &[Cert]) -> Option<Evidence> {
    let o = parse_ocsp(der)?;
    let s = o.singles.iter().find(|s| {
        let h = s.hash.unwrap_or(Hash::Sha1);
        trim(&s.serial) == trim(&cert.serial) && s.name_hash == h.digest(issuer.subject_der()) && s.key_hash == h.digest(&issuer.key_bits())
    })?;
    let signer = o.signer(issuer, pool)?;
    let responder = (signer.der != issuer.der).then_some(signer);
    Some(Evidence { source: Source::Ocsp, status: s.status, this_update: s.this_update, next_update: s.next_update, responder })
}

// --- CRL -----------------------------------------------------------------------------

/// A certificate revocation list.
pub struct Crl {
    tbs: Vec<u8>,
    alg: Vec<u8>,
    sig: Vec<u8>,
    pub issuer_der: Vec<u8>,
    pub issuer: Name,
    pub this_update: i64,
    pub next_update: Option<i64>,
    revoked: Vec<(Vec<u8>, i64)>,
}

/// Reads a CRL (DER, or PEM as some servers send).
pub fn parse_crl(data: &[u8]) -> Option<Crl> {
    let der = pem_body(data).unwrap_or_else(|| data.to_vec());
    let crl = asn1::parse(&der).ok()?;
    let mut r = crl.children();
    let tbs = r.next()?;
    let alg = r.next()?.raw.to_vec();
    let sig = r.next()?.bits().to_vec();
    let mut t = tbs.children();
    t.take(asn1::INTEGER);
    t.next()?;
    let issuer = t.next()?;
    let this_update = t.next()?.time()?;
    let next_update = match t.peek_tag() {
        Some(asn1::UTC_TIME | asn1::GENERALIZED_TIME) => t.next().and_then(|x| x.time()),
        _ => None,
    };
    let mut revoked = Vec::new();
    if let Some(list) = t.take(asn1::SEQUENCE) {
        let mut lr = list.children();
        while let Some(e) = lr.next() {
            let mut er = e.children();
            if let (Some(serial), Some(when)) = (er.next(), er.next()) {
                revoked.push((trim(serial.body).to_vec(), when.time().unwrap_or(i64::MIN)));
            }
        }
    }
    Some(Crl { tbs: tbs.raw.to_vec(), alg, sig, issuer_der: issuer.raw.to_vec(), issuer: Name::parse(&issuer), this_update, next_update, revoked })
}

/// The DER inside a PEM block.
fn pem_body(data: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(data).ok()?;
    let start = text.find("-----BEGIN")?;
    let body = &text[start..];
    let from = body.find('\n')? + 1;
    let to = body.find("-----END")?;
    crate::base64(&body[from..to])
}

/// What a CRL says of `cert`, if `issuer` signed it.
pub fn check_crl(data: &[u8], cert: &Cert, issuer: &Cert) -> Option<Evidence> {
    let c = parse_crl(data)?;
    if c.issuer_der != cert.issuer_der() && c.issuer != cert.issuer {
        return None;
    }
    if !issuer.verifies(&c.tbs, &c.alg, &c.sig) {
        return None;
    }
    let serial = trim(&cert.serial);
    let status = match c.revoked.iter().find(|(s, _)| s == serial) {
        Some((_, t)) => CertStatus::Revoked(*t),
        None => CertStatus::Good,
    };
    Some(Evidence { source: Source::Crl, status, this_update: c.this_update, next_update: c.next_update, responder: None })
}

// --- asking --------------------------------------------------------------------------

/// Asks about `cert`: its OCSP responders first, then its CRLs. Returns
/// where the answer came from and the answer as the document keeps it.
pub fn fetch(net: &dyn Net, cert: &Cert, issuer: &Cert, pool: &[Cert]) -> Result<(Source, Vec<u8>, Evidence), Error> {
    let mut why = Vec::new();
    for url in &cert.ocsp_urls {
        // The certificate says where: never this computer's own services.
        if let Err(e) = crate::net::not_local(url) {
            why.push(e.to_string());
            continue;
        }
        match net.post(url, "application/ocsp-request", &ocsp_request(cert, issuer)) {
            Ok(resp) => match check_ocsp(&resp, cert, issuer, pool) {
                Some(e) if e.status != CertStatus::Unknown => return Ok((Source::Ocsp, resp, e)),
                Some(_) => why.push(format!("{url}: the responder doesn't know the certificate")),
                None => why.push(format!("{url}: not a valid OCSP answer")),
            },
            Err(e) => why.push(e.to_string()),
        }
    }
    for url in &cert.crl_urls {
        if !url.starts_with("http") {
            continue;
        }
        if let Err(e) = crate::net::not_local(url) {
            why.push(e.to_string());
            continue;
        }
        match net.get(url) {
            Ok(data) => match check_crl(&data, cert, issuer) {
                Some(e) => {
                    let der = pem_body(&data).unwrap_or(data);
                    return Ok((Source::Crl, der, e));
                }
                None => why.push(format!("{url}: not a valid CRL of the issuer")),
            },
            Err(e) => why.push(e.to_string()),
        }
    }
    if why.is_empty() {
        return Err(Error::new(format!("the certificate of {} says nowhere to check its revocation", cert.subject.display())));
    }
    Err(Error::new(format!("{}: {}", cert.subject.display(), why.join("; "))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_garbage() {
        assert!(parse_ocsp(b"nothing").is_none());
        // An unsuccessful OCSPResponse (tryLater).
        assert!(parse_ocsp(&asn1::seq(&[&asn1::tlv(0x0A, &[3])])).is_none());
        assert!(parse_crl(b"-----BEGIN X509 CRL-----\nAAAA\n-----END X509 CRL-----\n").is_none());
        assert_eq!(trim(&[0, 0, 5]), [5]);
        assert_eq!(trim(&[0]), [0]);
    }
}
