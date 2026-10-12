//! CMS (PKCS #7) SignedData, as PDF signatures and time-stamps carry it:
//! reading and checking one, and making a CAdES one for PAdES.

use crate::Error;
use crate::asn1::{self, Tlv};
use crate::crypto::{Hash, PrivKey};
use crate::signer::{self, KeyKind, Signer};
use crate::x509::Cert;

pub const DATA: &str = "1.2.840.113549.1.7.1";
pub const SIGNED_DATA: &str = "1.2.840.113549.1.7.2";
pub const TST_INFO: &str = "1.2.840.113549.1.9.16.1.4";
const CONTENT_TYPE: &str = "1.2.840.113549.1.9.3";
const MESSAGE_DIGEST: &str = "1.2.840.113549.1.9.4";
const SIGNING_TIME: &str = "1.2.840.113549.1.9.5";
const SIGNING_CERT_V2: &str = "1.2.840.113549.1.9.16.2.47";
pub const TIMESTAMP_TOKEN: &str = "1.2.840.113549.1.9.16.2.14";

pub struct SignerInfo {
    issuer: Vec<u8>,
    serial: Vec<u8>,
    ski: Option<Vec<u8>>,
    pub hash: Option<Hash>,
    /// The signed attributes as signed (with the SET tag).
    signed_attrs: Option<Vec<u8>>,
    message_digest: Option<Vec<u8>>,
    pub signing_time: Option<i64>,
    sig_alg: Vec<u8>,
    pub signature: Vec<u8>,
    /// A time-stamp token over the signature (unsigned attribute).
    pub timestamp: Option<Vec<u8>>,
    /// Revocation answers signed along (Adobe's revocationInfoArchival):
    /// CRLs and OCSPResponses.
    pub crls: Vec<Vec<u8>>,
    pub ocsps: Vec<Vec<u8>>,
}

pub struct SignedData {
    pub certs: Vec<Cert>,
    pub content_type: String,
    /// The encapsulated content (none when detached).
    pub content: Option<Vec<u8>>,
    pub signers: Vec<SignerInfo>,
    /// Revocation answers that came with the signature: CRLs and
    /// OCSPResponses.
    pub crls: Vec<Vec<u8>>,
    pub ocsps: Vec<Vec<u8>>,
}

const REVOCATION_ARCHIVAL: &str = "1.2.840.113583.1.1.8";
const RI_OCSP_RESPONSE: &str = "1.3.6.1.5.5.7.16.2";

fn attr_values<'a>(attrs: &Tlv<'a>) -> Vec<(String, Tlv<'a>)> {
    let mut out = Vec::new();
    let mut r = attrs.children();
    while let Some(a) = r.next() {
        let mut ar = a.children();
        let (Some(ty), Some(vals)) = (ar.next(), ar.next()) else { continue };
        if let Some(v) = vals.children().next() {
            out.push((ty.oid(), v));
        }
    }
    out
}

/// Reads a ContentInfo holding a SignedData.
pub fn parse(der: &[u8]) -> Result<SignedData, Error> {
    let ci = asn1::parse(der)?;
    let mut r = ci.children();
    if r.req()?.oid() != SIGNED_DATA {
        return Err(Error::new("not a CMS signature"));
    }
    let sd = r.req()?.inner()?;
    let mut s = sd.children();
    s.req()?; // version
    s.req()?; // digest algorithms
    let encap = s.req()?;
    let mut er = encap.children();
    let content_type = er.req()?.oid();
    let content = er.next().and_then(|c| c.inner().ok()).map(|o| o.octets().into_owned());
    let mut certs = Vec::new();
    let mut signers = Vec::new();
    let (mut crls, mut ocsps) = (Vec::new(), Vec::new());
    while let Some(t) = s.next() {
        match t.tag {
            // RevocationInfoChoices: CRLs, and OCSP answers as "other".
            0xA1 => {
                let mut rr = t.children();
                while let Some(c) = rr.next() {
                    match c.tag {
                        asn1::SEQUENCE => crls.push(c.raw.to_vec()),
                        0xA1 => {
                            let mut o = c.children();
                            if o.next().is_some_and(|f| f.oid() == RI_OCSP_RESPONSE)
                                && let Some(v) = o.next()
                            {
                                ocsps.push(v.raw.to_vec());
                            }
                        }
                        _ => {}
                    }
                }
            }
            0xA0 => {
                let mut cr = t.children();
                while let Some(c) = cr.next() {
                    if c.tag == asn1::SEQUENCE
                        && certs.len() < crate::MAX_CERTS
                        && let Ok(cert) = Cert::from_der(c.raw)
                    {
                        certs.push(cert);
                    }
                }
            }
            asn1::SET => {
                let mut sr = t.children();
                while let Some(si) = sr.next() {
                    if let Ok(x) = signer(&si) {
                        signers.push(x);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(SignedData { certs, content_type, content, signers, crls, ocsps })
}

fn signer(si: &Tlv) -> Result<SignerInfo, Error> {
    let mut r = si.children();
    r.req()?;
    let sid = r.req()?;
    let (mut issuer, mut serial, mut ski) = (Vec::new(), Vec::new(), None);
    if sid.tag == asn1::SEQUENCE {
        let mut ir = sid.children();
        issuer = ir.req()?.raw.to_vec();
        serial = ir.req()?.body.to_vec();
    } else {
        ski = Some(sid.body.to_vec());
    }
    let hash = Hash::from_alg(&r.req()?);
    let mut out = SignerInfo { issuer, serial, ski, hash, signed_attrs: None, message_digest: None, signing_time: None, sig_alg: Vec::new(), signature: Vec::new(), timestamp: None, crls: Vec::new(), ocsps: Vec::new() };
    let mut next = r.req()?;
    if next.tag == 0xA0 {
        // Signed over as a SET (tag 0x31), with the bytes as written.
        let mut raw = next.raw.to_vec();
        raw[0] = asn1::SET;
        out.signed_attrs = Some(raw);
        for (ty, v) in attr_values(&next) {
            match ty.as_str() {
                MESSAGE_DIGEST => out.message_digest = Some(v.octets().into_owned()),
                SIGNING_TIME => out.signing_time = v.time(),
                REVOCATION_ARCHIVAL => {
                    // [0] CRLs, [1] OCSPResponses (each a SEQUENCE OF).
                    let mut rr = v.children();
                    while let Some(part) = rr.next() {
                        let Ok(list) = part.inner() else { continue };
                        let mut lr = list.children();
                        while let Some(x) = lr.next() {
                            match part.tag {
                                0xA0 => out.crls.push(x.raw.to_vec()),
                                0xA1 => out.ocsps.push(x.raw.to_vec()),
                                _ => {}
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        next = r.req()?;
    }
    out.sig_alg = next.raw.to_vec();
    out.signature = r.req()?.octets().into_owned();
    if let Some(u) = r.take(0xA1) {
        for (ty, v) in attr_values(&u) {
            if ty == TIMESTAMP_TOKEN {
                out.timestamp = Some(v.raw.to_vec());
            }
        }
    }
    Ok(out)
}

/// What checking a signer found.
pub struct Checked {
    /// The content's digest matches the signed one.
    pub digest_ok: bool,
    /// The signature is mathematically right for the signer's key.
    pub signature_ok: bool,
    pub cert: Option<Cert>,
}

impl SignedData {
    /// The certificate of a signer.
    pub fn cert_of(&self, s: &SignerInfo) -> Option<&Cert> {
        self.certs.iter().find(|c| match &s.ski {
            Some(k) => c.ski.as_ref() == Some(k),
            None => c.is(&s.issuer, &s.serial),
        })
    }

    /// Checks signer `s` against the content's digest `content_digest`
    /// (made with the signer's digest algorithm).
    pub fn check(&self, s: &SignerInfo, content_digest: &[u8]) -> Checked {
        let cert = self.cert_of(s).cloned();
        let hash = s.hash.unwrap_or(Hash::Sha256);
        let (digest_ok, signed) = match &s.signed_attrs {
            Some(attrs) => (s.message_digest.as_deref() == Some(content_digest), hash.digest(attrs)),
            None => (true, content_digest.to_vec()),
        };
        let signature_ok = cert.as_ref().is_some_and(|c| {
            let Ok(key) = c.public_key() else { return false };
            let Ok(alg) = asn1::parse(&s.sig_alg) else { return false };
            let Some((scheme, named)) = crate::crypto::scheme_of(&alg) else { return false };
            // The digest named by the signature algorithm wins (it's the
            // one inside the signature); a mismatch fails.
            let signed = match named {
                Some(h) if h != hash => match &s.signed_attrs {
                    Some(attrs) => h.digest(attrs),
                    None => return false,
                },
                _ => signed.clone(),
            };
            key.verify(scheme, named.unwrap_or(hash), &signed, &s.signature)
        });
        Checked { digest_ok, signature_ok, cert }
    }
}

/// A time-stamp token's time and the digest it stamps.
pub struct Stamp {
    pub time: i64,
    pub hash: Hash,
    pub imprint: Vec<u8>,
    pub nonce: Option<Vec<u8>>,
    /// The token's own signature holds and covers its TSTInfo.
    pub valid: bool,
    pub tsa: Option<Cert>,
    pub certs: Vec<Cert>,
}

/// Reads and checks a time-stamp token (RFC 3161).
pub fn stamp(token: &[u8]) -> Result<Stamp, Error> {
    let sd = parse(token)?;
    if sd.content_type != TST_INFO {
        return Err(Error::new("not a time-stamp"));
    }
    let info = sd.content.clone().ok_or_else(|| Error::new("empty time-stamp"))?;
    let t = asn1::parse(&info)?;
    let mut r = t.children();
    r.req()?; // version
    r.req()?; // policy
    let mi = r.req()?;
    let mut mr = mi.children();
    let hash = Hash::from_alg(&mr.req()?).ok_or_else(|| Error::new("unknown digest"))?;
    let imprint = mr.req()?.octets().into_owned();
    r.req()?; // serial
    let time = r.req()?.time().ok_or_else(|| Error::new("bad time-stamp time"))?;
    let mut nonce = None;
    while let Some(x) = r.next() {
        if x.tag == asn1::INTEGER {
            nonce = Some(x.body.to_vec());
        }
    }
    let (mut valid, mut tsa) = (false, None);
    if let Some(s) = sd.signers.first() {
        let c = sd.check(s, &s.hash.unwrap_or(Hash::Sha256).digest(&info));
        valid = c.digest_ok && c.signature_ok;
        tsa = c.cert;
    }
    Ok(Stamp { time, hash, imprint, nonce, valid, tsa, certs: sd.certs })
}

// --- making one ----------------------------------------------------------------------

/// A detached CAdES signature (PAdES B-B) of content with digest
/// `content_digest` (made with the key's digest), by `signer` with its
/// certificate and chain; `stamp` gets the signature value and may return a time-stamp
/// token for it.
pub fn sign_detached(signer: &dyn Signer, content_digest: &[u8], stamp: &mut dyn FnMut(&[u8]) -> Result<Option<Vec<u8>>, Error>) -> Result<Vec<u8>, Error> {
    let (cert, chain) = (signer.certificate(), signer.chain());
    let kind = KeyKind::of(cert)?;
    let hash = kind.hash();
    let digest_alg = asn1::alg(hash.oid(), false);
    // ESS signing-certificate-v2: the certificate's digest (SHA-256 is
    // the default and left out).
    let cert_hash = Hash::Sha256.digest(&cert.der);
    let issuer_serial = asn1::seq(&[&asn1::seq(&[&asn1::ctx(4, &[cert.issuer_der()])]), &asn1::tlv(asn1::INTEGER, &cert.serial)]);
    let ess_cert = asn1::seq(&[&asn1::octets(&cert_hash), &issuer_serial]);
    let signing_cert = asn1::seq(&[&asn1::seq(&[&ess_cert])]);
    let attr = |oid: &str, v: &[u8]| asn1::seq(&[&asn1::oid(oid), &asn1::set_of(&[v])]);
    let attrs = [attr(CONTENT_TYPE, &asn1::oid(DATA)), attr(MESSAGE_DIGEST, &asn1::octets(content_digest)), attr(SIGNING_CERT_V2, &signing_cert)];
    let refs: Vec<&[u8]> = attrs.iter().map(|a| a.as_slice()).collect();
    let signed_set = asn1::set_of(&refs);
    let signature = signer::sign_checked(signer, kind, hash, &hash.digest(&signed_set))?;
    let mut signed_attrs = signed_set.clone();
    signed_attrs[0] = 0xA0;
    let sid = asn1::seq(&[cert.issuer_der(), &asn1::tlv(asn1::INTEGER, &cert.serial)]);
    let mut si = vec![asn1::uint(1), sid, digest_alg.clone(), signed_attrs, kind.alg(), asn1::octets(&signature)];
    if let Some(token) = stamp(&signature)? {
        si.push(asn1::ctx(1, &[&attr(TIMESTAMP_TOKEN, &token)]));
    }
    let si_refs: Vec<&[u8]> = si.iter().map(|x| x.as_slice()).collect();
    let signer_info = asn1::seq(&si_refs);
    let mut all = vec![cert.der.as_slice()];
    all.extend(chain.iter().filter(|c| c.der != cert.der).map(|c| c.der.as_slice()));
    let certs = asn1::tlv(0xA0, &all.concat());
    let sd = asn1::seq(&[&asn1::uint(1), &asn1::set_of(&[&digest_alg]), &asn1::seq(&[&asn1::oid(DATA)]), &certs, &asn1::set_of(&[&signer_info])]);
    Ok(asn1::seq(&[&asn1::oid(SIGNED_DATA), &asn1::ctx(0, &[&sd])]))
}

/// Signs `tbs` (DER) as an X.509 structure: the signed thing, its
/// algorithm and the signature (for the tests' certificates).
pub fn sign_x509(key: &PrivKey, tbs: &[u8]) -> Result<Vec<u8>, Error> {
    let hash = key.hash();
    let (alg, sig) = match key {
        PrivKey::Rsa(_) => (asn1::alg("1.2.840.113549.1.1.11", true), key.sign(&hash.digest(tbs))?),
        _ => (key.alg(), key.sign(&hash.digest(tbs))?),
    };
    Ok(asn1::seq(&[tbs, &alg, &asn1::bit_string(&sig)]))
}
