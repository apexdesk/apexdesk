//! Certificate files (.p12, .pfx: PKCS #12): the private key, its
//! certificate and the rest of the chain. Reads what browsers, Windows,
//! macOS and OpenSSL write: PBES2 with PBKDF2 and AES (or 3DES), and the
//! older PKCS #12 schemes with 3DES and RC2.

use aes::cipher::{BlockDecryptMut, InnerIvInit, KeyIvInit, block_padding::Pkcs7};
use hmac::Mac;

use crate::Error;
use crate::asn1::{self, Tlv};
use crate::crypto::{Hash, PrivKey};
use crate::x509::Cert;

/// A signing identity: key, certificate and chain.
#[derive(Clone)]
pub struct Identity {
    pub key: PrivKey,
    pub cert: Cert,
    /// The other certificates in the file (intermediates, root).
    pub chain: Vec<Cert>,
}

/// A key read from a file signs in this process.
impl crate::Signer for Identity {
    fn certificate(&self) -> &Cert {
        &self.cert
    }

    fn chain(&self) -> &[Cert] {
        &self.chain
    }

    fn sign(&self, hash: Hash, digest: &[u8]) -> Result<Vec<u8>, Error> {
        self.key.sign_with(hash, digest)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum P12Error {
    WrongPassword,
    NoKey,
    Unsupported(String),
    Malformed,
}

impl std::fmt::Display for P12Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            P12Error::WrongPassword => write!(f, "wrong password"),
            P12Error::NoKey => write!(f, "the file has no private key with its certificate"),
            P12Error::Unsupported(m) => write!(f, "unsupported: {m}"),
            P12Error::Malformed => write!(f, "not a valid certificate file"),
        }
    }
}

impl From<Error> for P12Error {
    fn from(_: Error) -> Self {
        P12Error::Malformed
    }
}

/// The password as PKCS #12 wants it: UTF-16BE with a final zero.
fn bmp(password: &str) -> Vec<u8> {
    let mut v: Vec<u8> = password.encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
    v.extend_from_slice(&[0, 0]);
    v
}

/// A password in the two forms the schemes use: UTF-16 for the PKCS #12
/// ones, UTF-8 for PBES2.
struct Pass {
    bmp: Vec<u8>,
    utf8: Vec<u8>,
}

/// The ways a password may have been used: an empty one is written as a
/// lone zero or as nothing.
fn candidates(password: &str) -> Vec<Pass> {
    let mut v = vec![Pass { bmp: bmp(password), utf8: password.as_bytes().to_vec() }];
    if password.is_empty() {
        v.push(Pass { bmp: Vec::new(), utf8: Vec::new() });
    }
    v
}

/// The key derivation of PKCS #12 (RFC 7292, appendix B), with SHA-1 or
/// SHA-2.
fn kdf(hash: Hash, pass: &[u8], salt: &[u8], id: u8, iterations: u64, len: usize) -> Vec<u8> {
    let (u, v) = match hash {
        Hash::Sha1 => (20, 64),
        Hash::Sha224 => (28, 64),
        Hash::Sha256 => (32, 64),
        Hash::Sha384 => (48, 128),
        Hash::Sha512 => (64, 128),
    };
    let fill = |s: &[u8]| -> Vec<u8> {
        if s.is_empty() {
            return Vec::new();
        }
        let n = v * s.len().div_ceil(v);
        (0..n).map(|i| s[i % s.len()]).collect()
    };
    let mut i_buf = fill(salt);
    i_buf.extend(fill(pass));
    let d = vec![id; v];
    let mut out = Vec::new();
    while out.len() < len {
        let mut a = hash.digest_parts(&[&d, &i_buf]);
        for _ in 1..iterations {
            a = hash.digest(&a);
        }
        out.extend_from_slice(&a[..u.min(a.len())]);
        if out.len() >= len {
            break;
        }
        let b = fill(&a);
        let b = &b[..v];
        for chunk in i_buf.chunks_mut(v) {
            // chunk = (chunk + b + 1) mod 2^(v*8)
            let mut carry = 1u16;
            for k in (0..v).rev() {
                let s = chunk[k] as u16 + b[k] as u16 + carry;
                chunk[k] = s as u8;
                carry = s >> 8;
            }
        }
    }
    out.truncate(len);
    out
}

fn cbc_decrypt<C>(key: &[u8], iv: &[u8], data: &[u8]) -> Option<Vec<u8>>
where
    C: aes::cipher::BlockCipher + aes::cipher::BlockDecryptMut + aes::cipher::KeyInit,
{
    cbc::Decryptor::<C>::new_from_slices(key, iv).ok()?.decrypt_padded_vec_mut::<Pkcs7>(data).ok()
}

/// The most key-derivation rounds a file may ask for: far above what any
/// tool writes (OpenSSL 2048, Windows 2000, recent tools some 600 000),
/// low enough that a hostile file cannot keep the app busy for hours.
const MAX_ITER: u64 = 10_000_000;

/// Decrypts `data` encrypted with algorithm `alg` and `password`.
/// `None`: the password doesn't fit.
fn decrypt(alg: &Tlv, pass: &Pass, data: &[u8]) -> Result<Option<Vec<u8>>, P12Error> {
    let mut r = alg.children();
    let oid = r.req()?.oid();
    let params = r.req()?;
    let pkcs12_pbe = |key_len: usize, cipher: &str| -> Result<Option<Vec<u8>>, P12Error> {
        let mut pr = params.children();
        let salt = pr.req()?.octets().into_owned();
        let iter = pr.req()?.uint().unwrap_or(1);
        if iter > MAX_ITER {
            return Err(P12Error::Unsupported("key derivation rounds".into()));
        }
        let key = kdf(Hash::Sha1, &pass.bmp, &salt, 1, iter, key_len);
        let iv = kdf(Hash::Sha1, &pass.bmp, &salt, 2, iter, 8);
        Ok(match cipher {
            "3des" => cbc_decrypt::<des::TdesEde3>(&key, &iv, data),
            "2des" => {
                let mut k3 = key.clone();
                k3.extend_from_slice(&key[..8]);
                cbc_decrypt::<des::TdesEde3>(&k3, &iv, data)
            }
            _ => {
                let rc2 = rc2::Rc2::new_with_eff_key_len(&key, key_len * 8);
                cbc::Decryptor::<rc2::Rc2>::inner_iv_slice_init(rc2, &iv).ok().and_then(|d| d.decrypt_padded_vec_mut::<Pkcs7>(data).ok())
            }
        })
    };
    match oid.as_str() {
        "1.2.840.113549.1.12.1.3" => pkcs12_pbe(24, "3des"),
        "1.2.840.113549.1.12.1.4" => pkcs12_pbe(16, "2des"),
        "1.2.840.113549.1.12.1.5" => pkcs12_pbe(16, "rc2"),
        "1.2.840.113549.1.12.1.6" => pkcs12_pbe(5, "rc2"),
        "1.2.840.113549.1.5.13" => {
            let mut pr = params.children();
            let kdf_alg = pr.req()?;
            let enc = pr.req()?;
            let mut kr = kdf_alg.children();
            if kr.req()?.oid() != "1.2.840.113549.1.5.12" {
                return Err(P12Error::Unsupported("key derivation".into()));
            }
            let kp = kr.req()?;
            let mut kpr = kp.children();
            let salt = kpr.req()?.octets().into_owned();
            let iter = kpr.req()?.uint().unwrap_or(1);
            if iter > MAX_ITER {
                return Err(P12Error::Unsupported("key derivation rounds".into()));
            }
            let iter = iter as u32;
            let mut prf = "1.2.840.113549.2.7".to_string();
            while let Some(x) = kpr.next() {
                if x.tag == asn1::SEQUENCE {
                    prf = x.children().next().map(|o| o.oid()).unwrap_or(prf);
                }
            }
            let mut er = enc.children();
            let cipher = er.req()?.oid();
            let iv = er.req()?.octets().into_owned();
            let key_len = match cipher.as_str() {
                "2.16.840.1.101.3.4.1.2" => 16,
                "2.16.840.1.101.3.4.1.22" => 24,
                "2.16.840.1.101.3.4.1.42" => 32,
                "1.2.840.113549.3.7" => 24,
                c => return Err(P12Error::Unsupported(format!("cipher {c}"))),
            };
            let mut key = vec![0u8; key_len];
            let pass = pass.utf8.as_slice();
            match prf.as_str() {
                "1.2.840.113549.2.7" => pbkdf2::pbkdf2_hmac::<sha1::Sha1>(pass, &salt, iter, &mut key),
                "1.2.840.113549.2.8" => pbkdf2::pbkdf2_hmac::<sha2::Sha224>(pass, &salt, iter, &mut key),
                "1.2.840.113549.2.9" => pbkdf2::pbkdf2_hmac::<sha2::Sha256>(pass, &salt, iter, &mut key),
                "1.2.840.113549.2.10" => pbkdf2::pbkdf2_hmac::<sha2::Sha384>(pass, &salt, iter, &mut key),
                "1.2.840.113549.2.11" => pbkdf2::pbkdf2_hmac::<sha2::Sha512>(pass, &salt, iter, &mut key),
                p => return Err(P12Error::Unsupported(format!("PRF {p}"))),
            }
            Ok(match key_len {
                16 => cbc_decrypt::<aes::Aes128>(&key, &iv, data),
                24 if cipher == "1.2.840.113549.3.7" => cbc_decrypt::<des::TdesEde3>(&key, &iv, data),
                24 => cbc_decrypt::<aes::Aes192>(&key, &iv, data),
                _ => cbc_decrypt::<aes::Aes256>(&key, &iv, data),
            })
        }
        o => Err(P12Error::Unsupported(format!("encryption {o}"))),
    }
}

/// Checks the file's MAC with `password`: `Some(false)` when it doesn't
/// match, `None` when it can't be checked.
fn mac_ok(mac: &Tlv, content: &[u8], pass: &Pass) -> Option<bool> {
    let mut r = mac.children();
    let digest_info = r.next()?;
    let salt = r.next()?.octets().into_owned();
    let iter = r.next().and_then(|i| i.uint()).unwrap_or(1);
    if iter > MAX_ITER {
        return None;
    }
    let mut dr = digest_info.children();
    let hash = Hash::from_alg(&dr.next()?)?;
    let expected = dr.next()?.octets().into_owned();
    let key = kdf(hash, &pass.bmp, &salt, 3, iter, expected.len());
    fn hmac<M: Mac + hmac::digest::KeyInit>(key: &[u8], data: &[u8]) -> Vec<u8> {
        let mut m = <M as hmac::digest::KeyInit>::new_from_slice(key).expect("any key length");
        m.update(data);
        m.finalize().into_bytes().to_vec()
    }
    let got = match hash {
        Hash::Sha1 => hmac::<hmac::Hmac<sha1::Sha1>>(&key, content),
        Hash::Sha224 => hmac::<hmac::Hmac<sha2::Sha224>>(&key, content),
        Hash::Sha256 => hmac::<hmac::Hmac<sha2::Sha256>>(&key, content),
        Hash::Sha384 => hmac::<hmac::Hmac<sha2::Sha384>>(&key, content),
        Hash::Sha512 => hmac::<hmac::Hmac<sha2::Sha512>>(&key, content),
    };
    Some(got == expected)
}

/// A bag's local key id, to pair a key with its certificate.
fn local_id(bag: &mut asn1::Reader) -> Option<Vec<u8>> {
    let attrs = bag.take(asn1::SET)?;
    let mut r = attrs.children();
    while let Some(a) = r.next() {
        let mut ar = a.children();
        if ar.next()?.oid() == "1.2.840.113549.1.9.21" {
            return ar.next()?.children().next().map(|v| v.octets().into_owned());
        }
    }
    None
}

/// Reads a .p12/.pfx file with its password.
pub fn read(data: &[u8], password: &str) -> Result<Identity, P12Error> {
    let pfx = asn1::parse(data)?;
    let mut r = pfx.children();
    r.req()?;
    let auth = r.req()?;
    let mut ar = auth.children();
    if ar.req()?.oid() != crate::cms::DATA {
        return Err(P12Error::Unsupported("public-key protected file".into()));
    }
    let content = ar.req()?.inner()?.octets().into_owned();
    let mac = r.next();
    let mut last = P12Error::WrongPassword;
    for pass in candidates(password) {
        if let Some(mac) = &mac
            && mac_ok(mac, &content, &pass) == Some(false)
        {
            continue;
        }
        match read_with(&content, &pass) {
            Err(P12Error::WrongPassword) => {}
            Err(e) => last = e,
            ok => return ok,
        }
    }
    Err(last)
}

fn read_with(content: &[u8], pass: &Pass) -> Result<Identity, P12Error> {
    let mut keys: Vec<(Option<Vec<u8>>, PrivKey)> = Vec::new();
    let mut certs: Vec<(Option<Vec<u8>>, Cert)> = Vec::new();
    let mut shrouded = Vec::new();
    let safe = asn1::parse(content)?;
    let mut sr = safe.children();
    while let Some(ci) = sr.next() {
        let mut cr = ci.children();
        let ty = cr.req()?.oid();
        let body = cr.req()?.inner()?;
        let bags = match ty.as_str() {
            crate::cms::DATA => body.octets().into_owned(),
            "1.2.840.113549.1.7.6" => {
                let mut er = body.children();
                er.req()?;
                let eci = er.req()?;
                let mut ecr = eci.children();
                ecr.req()?;
                let alg = ecr.req()?;
                let enc = ecr.req()?;
                let enc = if enc.tag & 0x20 != 0 { enc.octets().into_owned() } else { enc.body.to_vec() };
                decrypt(&alg, pass, &enc)?.ok_or(P12Error::WrongPassword)?
            }
            _ => continue,
        };
        let list = asn1::parse(&bags)?;
        let mut br = list.children();
        while let Some(bag) = br.next() {
            let mut b = bag.children();
            let Some(id) = b.next().map(|o| o.oid()) else { continue };
            let Some(value) = b.next().and_then(|v| v.inner().ok()) else { continue };
            let lid = local_id(&mut b);
            match id.as_str() {
                "1.2.840.113549.1.12.10.1.1" => keys.push((lid, PrivKey::from_pkcs8(value.raw).map_err(|_| P12Error::Unsupported("key type".into()))?)),
                "1.2.840.113549.1.12.10.1.2" => shrouded.push((lid, value.raw.to_vec())),
                "1.2.840.113549.1.12.10.1.3" => {
                    let mut vr = value.children();
                    if vr.next().map(|o| o.oid()).as_deref() == Some("1.2.840.113549.1.9.22.1")
                        && let Some(c) = vr.next().and_then(|c| c.inner().ok())
                        && let Ok(cert) = Cert::from_der(&c.octets())
                    {
                        certs.push((lid, cert));
                    }
                }
                _ => {}
            }
        }
    }
    for (lid, raw) in shrouded {
        let epki = asn1::parse(&raw)?;
        let mut er = epki.children();
        let alg = er.req()?;
        let data = er.req()?.octets().into_owned();
        let Some(der) = decrypt(&alg, pass, &data)? else { return Err(P12Error::WrongPassword) };
        keys.push((lid, PrivKey::from_pkcs8(&der).map_err(|_| P12Error::Unsupported("key type".into()))?));
    }
    // The key's certificate: same local id, else same public key.
    for (lid, key) in keys {
        let found = certs.iter().position(|(cid, c)| (lid.is_some() && *cid == lid) && c.public_key().is_ok_and(|k| key.matches(&k)))
            .or_else(|| certs.iter().position(|(_, c)| c.public_key().is_ok_and(|k| key.matches(&k))));
        if let Some(i) = found {
            let (_, cert) = certs.remove(i);
            let chain = certs.into_iter().map(|(_, c)| c).collect();
            return Ok(Identity { key, cert, chain });
        }
    }
    Err(P12Error::NoKey)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kdf_vectors() {
        // From RFC 7292 test data used by OpenSSL and others: password
        // "smeg", salt 0A58CF64530D823F, 1 iteration, ID 1, 24 bytes.
        let salt = [0x0A, 0x58, 0xCF, 0x64, 0x53, 0x0D, 0x82, 0x3F];
        let k = kdf(Hash::Sha1, &bmp("smeg"), &salt, 1, 1, 24);
        let hex: String = k.iter().map(|b| format!("{b:02X}")).collect();
        assert_eq!(hex, "8AAAE6297B6CB04642AB5B077851284EB7128F1A2A7FBCA3");
        let iv = kdf(Hash::Sha1, &bmp("smeg"), &salt, 2, 1, 8);
        assert_eq!(iv, [0x79, 0x99, 0x3D, 0xFE, 0x04, 0x8D, 0x3B, 0x76]);
    }
}
