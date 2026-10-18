//! Who signs: a certificate and something that signs digests with its
//! private key, wherever the key lives — read from a certificate file
//! into memory ([`crate::Identity`]), in the system's store
//! ([`crate::store`]), or on a smart card, token or HSM
//! ([`crate::pkcs11`]). The document formats only see a [`Signer`].

use crate::Error;
use crate::asn1;
use crate::crypto::{Hash, PubKey, Scheme};
use crate::x509::Cert;

/// A signing key and its certificate. The key may be outside the process:
/// only digests go to it.
pub trait Signer: Send {
    /// The signer's certificate.
    fn certificate(&self) -> &Cert;
    /// The other certificates to send with the signature (intermediates,
    /// maybe the root).
    fn chain(&self) -> &[Cert];
    /// Signs `digest`, made with `hash`: RSA PKCS #1 v1.5 (of the
    /// DigestInfo), or ECDSA with the signature in DER.
    fn sign(&self, hash: Hash, digest: &[u8]) -> Result<Vec<u8>, Error>;
}

/// The kind of key, from the certificate: it picks the digest and the
/// algorithm names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyKind {
    /// RSA with a modulus of this many bytes.
    Rsa(usize),
    P256,
    P384,
}

impl KeyKind {
    pub fn of(cert: &Cert) -> Result<KeyKind, Error> {
        Ok(match cert.public_key()? {
            PubKey::Rsa(k) => {
                use rsa::traits::PublicKeyParts;
                KeyKind::Rsa(k.size())
            }
            PubKey::P256(_) => KeyKind::P256,
            PubKey::P384(_) => KeyKind::P384,
        })
    }

    /// The digest its signatures use.
    pub fn hash(self) -> Hash {
        match self {
            KeyKind::P384 => Hash::Sha384,
            _ => Hash::Sha256,
        }
    }

    /// The CMS signature AlgorithmIdentifier.
    pub fn alg(self) -> Vec<u8> {
        match self {
            KeyKind::Rsa(_) => asn1::alg("1.2.840.113549.1.1.1", true),
            KeyKind::P256 => asn1::alg("1.2.840.10045.4.3.2", false),
            KeyKind::P384 => asn1::alg("1.2.840.10045.4.3.3", false),
        }
    }

    /// The XML Signature method URI.
    pub fn xml_method(self) -> &'static str {
        match self {
            KeyKind::Rsa(_) => "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256",
            KeyKind::P256 => "http://www.w3.org/2001/04/xmldsig-more#ecdsa-sha256",
            KeyKind::P384 => "http://www.w3.org/2001/04/xmldsig-more#ecdsa-sha384",
        }
    }

    /// The largest signature it makes (ECDSA in DER), in bytes.
    pub fn max_sig_len(self) -> usize {
        match self {
            KeyKind::Rsa(n) => n,
            KeyKind::P256 => 72,
            KeyKind::P384 => 104,
        }
    }

    /// The size of ECDSA's r and s (0 for RSA).
    fn field_len(self) -> usize {
        match self {
            KeyKind::Rsa(_) => 0,
            KeyKind::P256 => 32,
            KeyKind::P384 => 48,
        }
    }
}

/// Signs `digest` with `s` and checks the result against the certificate:
/// a card or store that signed with another key would otherwise make a
/// broken signature. ECDSA signatures in DER.
pub(crate) fn sign_checked(s: &dyn Signer, kind: KeyKind, hash: Hash, digest: &[u8]) -> Result<Vec<u8>, Error> {
    let sig = s.sign(hash, digest)?;
    let key = s.certificate().public_key()?;
    let scheme = if matches!(kind, KeyKind::Rsa(_)) { Scheme::RsaPkcs1 } else { Scheme::EcdsaDer };
    if !key.verify(scheme, hash, digest, &sig) {
        return Err(Error::new("the key's signature doesn't match its certificate"));
    }
    Ok(sig)
}

/// [`sign_checked`] as XML Signature wants it: ECDSA's r and s side by
/// side.
pub(crate) fn sign_raw(s: &dyn Signer, kind: KeyKind, hash: Hash, digest: &[u8]) -> Result<Vec<u8>, Error> {
    let sig = sign_checked(s, kind, hash, digest)?;
    match kind {
        KeyKind::Rsa(_) => Ok(sig),
        _ => ecdsa_raw(&sig, kind.field_len()),
    }
}

/// The DigestInfo that RSA PKCS #1 v1.5 signs, for keys that take it
/// whole (PKCS #11's CKM_RSA_PKCS).
pub fn digest_info(hash: Hash, digest: &[u8]) -> Vec<u8> {
    asn1::seq(&[&asn1::alg(hash.oid(), true), &asn1::octets(digest)])
}

/// An ECDSA signature given as r and s side by side (PKCS #11, Windows'
/// CNG), in DER.
pub fn ecdsa_der(raw: &[u8]) -> Result<Vec<u8>, Error> {
    if raw.is_empty() || raw.len() % 2 != 0 {
        return Err(Error::new("bad ECDSA signature"));
    }
    let (r, s) = raw.split_at(raw.len() / 2);
    let int = |v: &[u8]| {
        let v: Vec<u8> = v.iter().copied().skip_while(|&b| b == 0).collect();
        let mut body = if v.first().is_none_or(|&b| b & 0x80 != 0) { vec![0] } else { Vec::new() };
        body.extend(v);
        asn1::tlv(asn1::INTEGER, &body)
    };
    Ok(asn1::seq(&[&int(r), &int(s)]))
}

/// An ECDSA signature in DER as r and s of `len` bytes each.
fn ecdsa_raw(der: &[u8], len: usize) -> Result<Vec<u8>, Error> {
    let bad = || Error::new("bad ECDSA signature");
    let t = asn1::parse(der)?;
    let mut r = t.children();
    let mut out = Vec::with_capacity(len * 2);
    for _ in 0..2 {
        let v = r.req()?.body;
        let v = &v[v.iter().take_while(|&&b| b == 0).count()..];
        if v.len() > len {
            return Err(bad());
        }
        out.extend(std::iter::repeat_n(0, len - v.len()));
        out.extend_from_slice(v);
    }
    Ok(out)
}

/// The chain of `cert` from a pool of certificates (a store's, a
/// token's): its issuers up to the root, as far as the pool goes.
pub fn chain_from(cert: &Cert, pool: &[Cert]) -> Vec<Cert> {
    let mut chain: Vec<Cert> = Vec::new();
    let mut cur = cert.clone();
    while !cur.self_issued() && chain.len() < 10 {
        let Some(next) = pool.iter().find(|c| c.der != cur.der && !chain.iter().any(|x| x.der == c.der) && c.named_issuer_of(&cur) && c.signed(&cur)) else { break };
        chain.push(next.clone());
        cur = next.clone();
    }
    chain
}

/// A certificate as the lists of certificates show it, and whether it
/// can sign.
#[derive(Clone, Debug)]
pub struct CertInfo {
    pub subject: String,
    pub issuer: String,
    pub not_before: i64,
    pub not_after: i64,
    /// "Digital signature, Non-repudiation"… (empty: any use).
    pub usage: Vec<&'static str>,
    /// SHA-1 of the certificate in hex (upper case), as Windows shows it.
    pub sha1: String,
}

impl CertInfo {
    pub fn of(c: &Cert) -> CertInfo {
        // Key usage bits, highest first: digitalSignature,
        // nonRepudiation, keyEncipherment, dataEncipherment, keyAgreement,
        // keyCertSign, cRLSign.
        const NAMES: [&str; 7] = ["digital_signature", "non_repudiation", "key_encipherment", "data_encipherment", "key_agreement", "cert_sign", "crl_sign"];
        let usage = c.key_usage.map(|u| NAMES.iter().enumerate().filter(|(i, _)| u & (0x8000 >> i) != 0).map(|(_, n)| *n).collect()).unwrap_or_default();
        CertInfo { subject: c.subject.display(), issuer: c.issuer.display(), not_before: c.not_before, not_after: c.not_after, usage, sha1: c.sha1() }
    }
}

/// Whether a certificate is fit to sign documents at time `at`: in its
/// validity period, not a CA's, with a key usage (if any) that allows
/// signatures, and a key kind we can sign with.
pub fn can_sign(c: &Cert, at: i64) -> bool {
    let usage_ok = c.key_usage.is_none_or(|u| u & 0xC000 != 0);
    c.valid_at(at) && !c.ca && usage_ok && KeyKind::of(c).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ecdsa_forms() {
        let raw: Vec<u8> = (0..64).map(|i| if i == 0 || i == 32 { 0x80 } else { i as u8 }).collect();
        let der = ecdsa_der(&raw).unwrap();
        assert_eq!(der[3], 33);
        assert_eq!(ecdsa_raw(&der, 32).unwrap(), raw);
        let mut short = vec![0u8; 64];
        short[31] = 5;
        short[63] = 7;
        assert_eq!(ecdsa_der(&short).unwrap(), vec![0x30, 6, 2, 1, 5, 2, 1, 7]);
        assert_eq!(ecdsa_raw(&ecdsa_der(&short).unwrap(), 32).unwrap(), short);
    }
}
