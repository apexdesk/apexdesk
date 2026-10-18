//! Digests, public keys and signatures: RSA (PKCS #1 v1.5 and PSS) and
//! ECDSA on P-256 and P-384, verified and made on precomputed digests.

use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey};
use rsa::signature::hazmat::{PrehashSigner, PrehashVerifier};
use rsa::traits::PublicKeyParts;
use sha1::Digest;

use crate::Error;
use crate::asn1::{self, Tlv};
use crate::signer::KeyKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hash {
    Sha1,
    Sha224,
    Sha256,
    Sha384,
    Sha512,
}

impl Hash {
    pub fn from_oid(oid: &str) -> Option<Hash> {
        Some(match oid {
            "1.3.14.3.2.26" => Hash::Sha1,
            "2.16.840.1.101.3.4.2.4" => Hash::Sha224,
            "2.16.840.1.101.3.4.2.1" => Hash::Sha256,
            "2.16.840.1.101.3.4.2.2" => Hash::Sha384,
            "2.16.840.1.101.3.4.2.3" => Hash::Sha512,
            _ => return None,
        })
    }

    pub fn oid(self) -> &'static str {
        match self {
            Hash::Sha1 => "1.3.14.3.2.26",
            Hash::Sha224 => "2.16.840.1.101.3.4.2.4",
            Hash::Sha256 => "2.16.840.1.101.3.4.2.1",
            Hash::Sha384 => "2.16.840.1.101.3.4.2.2",
            Hash::Sha512 => "2.16.840.1.101.3.4.2.3",
        }
    }

    /// The XML Signature digest method URIs.
    pub fn from_uri(uri: &str) -> Option<Hash> {
        Some(match uri.rsplit(['#', '/']).next()? {
            "sha1" => Hash::Sha1,
            "sha224" => Hash::Sha224,
            "sha256" => Hash::Sha256,
            "sha384" => Hash::Sha384,
            "sha512" => Hash::Sha512,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Hash::Sha1 => "SHA-1",
            Hash::Sha224 => "SHA-224",
            Hash::Sha256 => "SHA-256",
            Hash::Sha384 => "SHA-384",
            Hash::Sha512 => "SHA-512",
        }
    }

    pub fn digest(self, data: &[u8]) -> Vec<u8> {
        self.digest_parts(&[data])
    }

    pub fn digest_parts(self, parts: &[&[u8]]) -> Vec<u8> {
        fn run<D: Digest>(parts: &[&[u8]]) -> Vec<u8> {
            let mut h = D::new();
            for p in parts {
                h.update(p);
            }
            h.finalize().to_vec()
        }
        match self {
            Hash::Sha1 => run::<sha1::Sha1>(parts),
            Hash::Sha224 => run::<sha2::Sha224>(parts),
            Hash::Sha256 => run::<sha2::Sha256>(parts),
            Hash::Sha384 => run::<sha2::Sha384>(parts),
            Hash::Sha512 => run::<sha2::Sha512>(parts),
        }
    }

    /// The digest from an AlgorithmIdentifier (Windows has put signature
    /// algorithms there: their digest is meant).
    pub fn from_alg(alg: &Tlv) -> Option<Hash> {
        Hash::from_oid(&alg.children().next()?.oid()).or_else(|| scheme_of(alg)?.1)
    }
}

/// How a signature was made, from its AlgorithmIdentifier (or URI).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    RsaPkcs1,
    /// RSASSA-PSS with its salt length.
    RsaPss(usize),
    /// ECDSA with a DER encoded signature (CMS).
    EcdsaDer,
    /// ECDSA with r and s side by side (XML Signature).
    EcdsaRaw,
}

/// The scheme and, when the algorithm names it, the digest of a CMS or
/// X.509 signature algorithm.
pub fn scheme_of(alg: &Tlv) -> Option<(Scheme, Option<Hash>)> {
    let mut r = alg.children();
    let oid = r.next()?.oid();
    Some(match oid.as_str() {
        "1.2.840.113549.1.1.1" => (Scheme::RsaPkcs1, None),
        "1.2.840.113549.1.1.5" => (Scheme::RsaPkcs1, Some(Hash::Sha1)),
        "1.2.840.113549.1.1.14" => (Scheme::RsaPkcs1, Some(Hash::Sha224)),
        "1.2.840.113549.1.1.11" => (Scheme::RsaPkcs1, Some(Hash::Sha256)),
        "1.2.840.113549.1.1.12" => (Scheme::RsaPkcs1, Some(Hash::Sha384)),
        "1.2.840.113549.1.1.13" => (Scheme::RsaPkcs1, Some(Hash::Sha512)),
        "1.2.840.113549.1.1.10" => {
            // RSASSA-PSS-params: [0] hash, [1] mask, [2] salt length.
            let (mut hash, mut salt) = (Hash::Sha1, 20);
            if let Some(p) = r.next() {
                let mut pr = p.children();
                while let Some(t) = pr.next() {
                    match t.tag {
                        // Read by its OID alone: an algorithm nested in
                        // itself mustn't be read for ever.
                        0xA0 => hash = t.inner().ok().and_then(|a| Hash::from_oid(&a.children().next()?.oid())).unwrap_or(hash),
                        // Past any key's size; a huge one overflows the check.
                        0xA2 => salt = t.inner().ok().and_then(|i| i.uint()).unwrap_or(20).min(4096) as usize,
                        _ => {}
                    }
                }
            }
            (Scheme::RsaPss(salt), Some(hash))
        }
        "1.2.840.10045.2.1" => (Scheme::EcdsaDer, None),
        "1.2.840.10045.4.1" => (Scheme::EcdsaDer, Some(Hash::Sha1)),
        "1.2.840.10045.4.3.1" => (Scheme::EcdsaDer, Some(Hash::Sha224)),
        "1.2.840.10045.4.3.2" => (Scheme::EcdsaDer, Some(Hash::Sha256)),
        "1.2.840.10045.4.3.3" => (Scheme::EcdsaDer, Some(Hash::Sha384)),
        "1.2.840.10045.4.3.4" => (Scheme::EcdsaDer, Some(Hash::Sha512)),
        _ => return None,
    })
}

/// The scheme and digest of an XML Signature method URI.
pub fn scheme_of_uri(uri: &str) -> Option<(Scheme, Hash)> {
    let tail = uri.rsplit(['#', '/']).next()?;
    let (kind, hash) = tail.split_once('-')?;
    let hash = Hash::from_uri(hash)?;
    Some(match kind {
        "rsa" => (Scheme::RsaPkcs1, hash),
        "ecdsa" => (Scheme::EcdsaRaw, hash),
        _ => return None,
    })
}

#[derive(Clone, Debug)]
pub enum PubKey {
    Rsa(rsa::RsaPublicKey),
    P256(p256::ecdsa::VerifyingKey),
    P384(p384::ecdsa::VerifyingKey),
}

impl PubKey {
    /// From a SubjectPublicKeyInfo.
    pub fn from_spki(der: &[u8]) -> Result<PubKey, Error> {
        let spki = asn1::parse(der)?;
        let mut r = spki.children();
        let alg = r.req()?;
        let mut ar = alg.children();
        let oid = ar.req()?.oid();
        let unsupported = || Error::new("unsupported key type");
        match oid.as_str() {
            "1.2.840.113549.1.1.1" | "1.2.840.113549.1.1.10" => {
                use rsa::pkcs1::DecodeRsaPublicKey;
                let bad = |_| Error::new("bad RSA key");
                match rsa::RsaPublicKey::from_public_key_der(der) {
                    Ok(k) => Ok(PubKey::Rsa(k)),
                    // PSS keys: the PKCS #1 key inside.
                    Err(_) => rsa::RsaPublicKey::from_pkcs1_der(r.req()?.bits()).map(PubKey::Rsa).map_err(bad),
                }
            }
            "1.2.840.10045.2.1" => {
                let curve = ar.req()?.oid();
                let point = r.req()?.bits();
                match curve.as_str() {
                    "1.2.840.10045.3.1.7" => p256::ecdsa::VerifyingKey::from_sec1_bytes(point).map(PubKey::P256).map_err(|_| Error::new("bad EC key")),
                    "1.3.132.0.34" => p384::ecdsa::VerifyingKey::from_sec1_bytes(point).map(PubKey::P384).map_err(|_| Error::new("bad EC key")),
                    _ => Err(unsupported()),
                }
            }
            _ => Err(unsupported()),
        }
    }

    /// "RSA 2048", "ECDSA P-256".
    pub fn describe(&self) -> String {
        match self {
            PubKey::Rsa(k) => format!("RSA {}", k.n().bits()),
            PubKey::P256(_) => "ECDSA P-256".into(),
            PubKey::P384(_) => "ECDSA P-384".into(),
        }
    }

    /// Whether `sig` signs the digest `hashed` (made with `hash`).
    pub fn verify(&self, scheme: Scheme, hash: Hash, hashed: &[u8], sig: &[u8]) -> bool {
        match (self, scheme) {
            (PubKey::Rsa(k), Scheme::RsaPkcs1) => k.verify(pkcs1(hash), hashed, sig).is_ok() || pkcs1_lenient(k, hash, hashed, sig),
            (PubKey::Rsa(k), Scheme::RsaPss(salt)) => k.verify(pss(hash, salt), hashed, sig).is_ok(),
            (PubKey::P256(k), Scheme::EcdsaDer | Scheme::EcdsaRaw) => {
                let s = if scheme == Scheme::EcdsaDer { p256::ecdsa::Signature::from_der(sig) } else { p256::ecdsa::Signature::from_slice(sig) };
                s.is_ok_and(|s| k.verify_prehash(hashed, &s).is_ok())
            }
            (PubKey::P384(k), Scheme::EcdsaDer | Scheme::EcdsaRaw) => {
                let s = if scheme == Scheme::EcdsaDer { p384::ecdsa::Signature::from_der(sig) } else { p384::ecdsa::Signature::from_slice(sig) };
                s.is_ok_and(|s| k.verify_prehash(hashed, &s).is_ok())
            }
            _ => false,
        }
    }
}

/// PKCS #1 v1.5 with the DigestInfo's algorithm also taken without its
/// NULL parameters (Office 2007 left them out, which strict checks
/// reject). Compared byte for byte with the one exact encoding: reading
/// it loosely would let forged signatures through for small exponents.
fn pkcs1_lenient(k: &rsa::RsaPublicKey, hash: Hash, hashed: &[u8], sig: &[u8]) -> bool {
    let size = k.size();
    if sig.len() != size || hashed.len() >= 128 {
        return false;
    }
    let Ok(m) = rsa::hazmat::rsa_encrypt(k, &rsa::BigUint::from_bytes_be(sig)) else { return false };
    let oid: &[u8] = match hash {
        Hash::Sha1 => &[0x2b, 0x0e, 0x03, 0x02, 0x1a],
        Hash::Sha224 => &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x04],
        Hash::Sha256 => &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01],
        Hash::Sha384 => &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02],
        Hash::Sha512 => &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03],
    };
    // SEQUENCE { SEQUENCE { OID }, OCTET STRING digest }
    let alg_len = 2 + oid.len();
    let mut info = vec![0x30, (2 + alg_len + 2 + hashed.len()) as u8, 0x30, oid.len() as u8 + 2, 0x06, oid.len() as u8];
    info.extend_from_slice(oid);
    info.extend_from_slice(&[0x04, hashed.len() as u8]);
    info.extend_from_slice(hashed);
    // 00 01 FF… 00 DigestInfo (the number drops the leading zero).
    let Some(pad) = (size - 1).checked_sub(2 + info.len()).filter(|&p| p >= 8) else { return false };
    let mut em = vec![1];
    em.extend(std::iter::repeat_n(0xFF, pad));
    em.push(0);
    em.extend_from_slice(&info);
    m.to_bytes_be() == em
}

fn pkcs1(hash: Hash) -> rsa::Pkcs1v15Sign {
    match hash {
        Hash::Sha1 => rsa::Pkcs1v15Sign::new::<sha1::Sha1>(),
        Hash::Sha224 => rsa::Pkcs1v15Sign::new::<sha2::Sha224>(),
        Hash::Sha256 => rsa::Pkcs1v15Sign::new::<sha2::Sha256>(),
        Hash::Sha384 => rsa::Pkcs1v15Sign::new::<sha2::Sha384>(),
        Hash::Sha512 => rsa::Pkcs1v15Sign::new::<sha2::Sha512>(),
    }
}

fn pss(hash: Hash, salt: usize) -> rsa::Pss {
    match hash {
        Hash::Sha1 => rsa::Pss::new_with_salt::<sha1::Sha1>(salt),
        Hash::Sha224 => rsa::Pss::new_with_salt::<sha2::Sha224>(salt),
        Hash::Sha256 => rsa::Pss::new_with_salt::<sha2::Sha256>(salt),
        Hash::Sha384 => rsa::Pss::new_with_salt::<sha2::Sha384>(salt),
        Hash::Sha512 => rsa::Pss::new_with_salt::<sha2::Sha512>(salt),
    }
}

/// A private key to sign with.
#[derive(Clone)]
pub enum PrivKey {
    Rsa(Box<rsa::RsaPrivateKey>),
    P256(p256::ecdsa::SigningKey),
    P384(p384::ecdsa::SigningKey),
}

impl PrivKey {
    /// From a PKCS #8 PrivateKeyInfo.
    pub fn from_pkcs8(der: &[u8]) -> Result<PrivKey, Error> {
        let info = asn1::parse(der)?;
        let mut r = info.children();
        r.req()?;
        let alg = r.req()?;
        let mut ar = alg.children();
        let bad = |_| Error::new("unreadable private key");
        match ar.req()?.oid().as_str() {
            "1.2.840.113549.1.1.1" | "1.2.840.113549.1.1.10" => rsa::RsaPrivateKey::from_pkcs8_der(der).map(|k| PrivKey::Rsa(Box::new(k))).map_err(bad),
            "1.2.840.10045.2.1" => {
                // The curve is in the algorithm (or else in the key).
                let curve = ar.next().map(|c| c.oid()).unwrap_or_default();
                match curve.as_str() {
                    "1.3.132.0.34" => p384::SecretKey::from_pkcs8_der(der).map(|k| PrivKey::P384(k.into())).map_err(bad),
                    _ => p256::SecretKey::from_pkcs8_der(der).map(|k| PrivKey::P256(k.into())).map_err(bad),
                }
            }
            _ => Err(Error::new("unsupported key type")),
        }
    }

    pub fn public(&self) -> PubKey {
        match self {
            PrivKey::Rsa(k) => PubKey::Rsa(k.to_public_key()),
            PrivKey::P256(k) => PubKey::P256(*k.verifying_key()),
            PrivKey::P384(k) => PubKey::P384(*k.verifying_key()),
        }
    }

    /// Whether `key` is this key's public half.
    pub fn matches(&self, key: &PubKey) -> bool {
        match (self.public(), key) {
            (PubKey::Rsa(a), PubKey::Rsa(b)) => a == *b,
            (PubKey::P256(a), PubKey::P256(b)) => a == *b,
            (PubKey::P384(a), PubKey::P384(b)) => a == *b,
            _ => false,
        }
    }

    /// Its kind, which picks the digest and the algorithm names.
    pub fn kind(&self) -> KeyKind {
        match self {
            PrivKey::Rsa(k) => KeyKind::Rsa(k.size()),
            PrivKey::P256(_) => KeyKind::P256,
            PrivKey::P384(_) => KeyKind::P384,
        }
    }

    /// The digest these signatures use.
    pub fn hash(&self) -> Hash {
        self.kind().hash()
    }

    /// The CMS signature AlgorithmIdentifier.
    pub fn alg(&self) -> Vec<u8> {
        self.kind().alg()
    }

    /// Signs the digest `hashed`, made with `hash`; ECDSA signatures in
    /// DER.
    pub fn sign_with(&self, hash: Hash, hashed: &[u8]) -> Result<Vec<u8>, Error> {
        let fail = |_| Error::new("signing failed");
        match self {
            PrivKey::Rsa(k) => k.sign(pkcs1(hash), hashed).map_err(|_| Error::new("signing failed")),
            PrivKey::P256(k) => PrehashSigner::<p256::ecdsa::Signature>::sign_prehash(k, hashed).map(|s| s.to_der().as_bytes().to_vec()).map_err(fail),
            PrivKey::P384(k) => PrehashSigner::<p384::ecdsa::Signature>::sign_prehash(k, hashed).map(|s| s.to_der().as_bytes().to_vec()).map_err(fail),
        }
    }

    /// Signs the digest `hashed` (made with [`Self::hash`]); ECDSA
    /// signatures in DER.
    pub fn sign(&self, hashed: &[u8]) -> Result<Vec<u8>, Error> {
        self.sign_with(self.hash(), hashed)
    }
}
