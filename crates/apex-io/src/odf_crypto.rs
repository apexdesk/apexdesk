//! Password-encrypted OpenDocument files (ODF 1.2 part 3, section 3.8):
//! each part of the package is deflated and encrypted, and the manifest
//! says how. Reads AES-CBC with PBKDF2 (ODF 1.2), the Blowfish CFB of
//! OpenOffice and ODF 1.0/1.1, and LibreOffice's AES-GCM with Argon2id
//! (from 24.2; it also wraps the whole package in one encrypted part).
//! Writes AES-256-CBC with PBKDF2, which every LibreOffice version opens.

use std::collections::HashMap;
use std::fmt::Write as _;

use sha2::Digest;

use crate::crypto::{CryptoError, b64, cbc_decrypt, cbc_encrypt, random, unb64};
use crate::xml::{Ev, Reader, esc};
use crate::zip::{ZipReader, ZipWriter};

const MANIFEST: &str = "META-INF/manifest.xml";
/// The part holding the whole package (LibreOffice 24.2 and later).
const WHOLE: &str = "encrypted-package";
/// PBKDF2 rounds for new files (as LibreOffice).
const ROUNDS: u32 = 100_000;

/// Whether the data is an OpenDocument package with encrypted parts.
pub fn is_encrypted(data: &[u8]) -> bool {
    data.starts_with(b"PK") && ZipReader::new(data).is_ok_and(|z| z.read_string(MANIFEST).is_ok_and(|m| m.contains("encryption-data")))
}

/// How one part is encrypted (`manifest:encryption-data`).
#[derive(Default, Debug)]
struct Part {
    path: String,
    checksum_type: String,
    checksum: Vec<u8>,
    algorithm: String,
    iv: Vec<u8>,
    start_key: String,
    kdf: String,
    rounds: u32,
    salt: Vec<u8>,
    key_len: usize,
    /// Argon2 iterations, memory (KiB) and lanes.
    argon: (u32, u32, u32),
}

fn parts(manifest: &str) -> Result<Vec<Part>, CryptoError> {
    let mut r = Reader::new(manifest);
    let mut out: Vec<Part> = Vec::new();
    let mut path = String::new();
    let num = |t: &crate::xml::Tag, k: &str| t.attr(k).and_then(|v| v.trim().parse::<u32>().ok());
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) => match t.local() {
                "file-entry" => path = t.attr("full-path").map(|v| v.into_owned()).unwrap_or_default(),
                "encryption-data" => out.push(Part {
                    path: path.clone(),
                    checksum_type: t.attr("checksum-type").map(|v| v.into_owned()).unwrap_or_default(),
                    checksum: t.attr("checksum").map_or(Ok(Vec::new()), |v| unb64(&v))?,
                    key_len: 16,
                    ..Default::default()
                }),
                "algorithm" if let Some(p) = out.last_mut() => {
                    p.algorithm = t.attr("algorithm-name").map(|v| v.into_owned()).unwrap_or_default();
                    p.iv = t.attr("initialisation-vector").map_or(Ok(Vec::new()), |v| unb64(&v))?;
                }
                "start-key-generation" if let Some(p) = out.last_mut() => {
                    p.start_key = t.attr("start-key-generation-name").map(|v| v.into_owned()).unwrap_or_default();
                }
                "key-derivation" if let Some(p) = out.last_mut() => {
                    p.kdf = t.attr("key-derivation-name").map(|v| v.into_owned()).unwrap_or_default();
                    p.rounds = num(&t, "iteration-count").unwrap_or(0);
                    p.salt = t.attr("salt").map_or(Ok(Vec::new()), |v| unb64(&v))?;
                    p.key_len = num(&t, "key-size").unwrap_or(16) as usize;
                    p.argon = (num(&t, "argon2-iterations").unwrap_or(0), num(&t, "argon2-memory").unwrap_or(0), num(&t, "argon2-lanes").unwrap_or(0));
                }
                _ => {}
            },
            _ => {}
        }
    }
    Ok(out)
}

/// Most PBKDF2 rounds of a key (LibreOffice uses 100 000).
const MAX_ROUNDS: u32 = 1_000_000;
/// Most Argon2 work of a key, iterations × KiB (LibreOffice: 3 × 64 MiB).
const MAX_ARGON_WORK: u64 = 1 << 22;
/// Most work to open a file, in PBKDF2 rounds: 100 parts of 100 000
/// rounds, or 4 Argon2 keys of the most work.
const MAX_WORK: u64 = 10_000_000 + (MAX_ARGON_WORK * 8) * 4;

fn sha1(d: &[u8]) -> Vec<u8> {
    sha1::Sha1::digest(d).to_vec()
}

fn sha256(d: &[u8]) -> Vec<u8> {
    sha2::Sha256::digest(d).to_vec()
}

impl Part {
    /// What deriving the key costs, in PBKDF2 rounds (an Argon2 pass over
    /// 1 KiB counts as about 8).
    fn cost(&self) -> u64 {
        if self.kdf.eq_ignore_ascii_case("PBKDF2") {
            u64::from(self.rounds.clamp(1, MAX_ROUNDS))
        } else {
            u64::from(self.argon.0) * u64::from(self.argon.1) * 8
        }
    }

    /// The key: the password hashed (SHA-1 unless the manifest says
    /// otherwise), then stretched.
    fn key(&self, pw: &str) -> Result<Vec<u8>, CryptoError> {
        let start = if self.start_key.to_ascii_lowercase().contains("sha256") { sha256(pw.as_bytes()) } else { sha1(pw.as_bytes()) };
        let mut key = vec![0u8; self.key_len.clamp(1, 64)];
        if self.kdf.eq_ignore_ascii_case("PBKDF2") {
            pbkdf2::pbkdf2_hmac::<sha1::Sha1>(&start, &self.salt, self.rounds.clamp(1, MAX_ROUNDS), &mut key);
        } else if self.kdf.to_ascii_lowercase().ends_with("argon2id") {
            let (t, m, p) = self.argon;
            // More than 1 GiB of memory, or passes over it that take
            // minutes, are no document's.
            if m > 1 << 20 || u64::from(t) * u64::from(m) > MAX_ARGON_WORK {
                return Err(CryptoError::Unsupported(format!("Argon2 with {m} KiB and {t} iterations")));
            }
            let params = argon2::Params::new(m, t, p, Some(key.len())).map_err(|e| CryptoError::Unsupported(e.to_string()))?;
            argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
                .hash_password_into(&start, &self.salt, &mut key)
                .map_err(|e| CryptoError::Unsupported(e.to_string()))?;
        } else {
            return Err(CryptoError::Unsupported(format!("key derivation {}", self.kdf)));
        }
        Ok(key)
    }

    /// The part decrypted, still deflated.
    fn decipher(&self, key: &[u8], data: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let alg = self.algorithm.to_ascii_lowercase();
        let plain = if alg.ends_with("-gcm") {
            // The IV goes first and the tag last (XML Encryption 1.1).
            if data.len() < 28 {
                return Err(CryptoError::Unsupported("short encrypted part".into()));
            }
            gcm_decrypt(key, &data[..12], &data[12..data.len() - 16], &data[data.len() - 16..])?
        } else if alg.ends_with("-cbc") {
            let mut p = cbc_decrypt(key, &self.iv, data)?;
            // Padding: the last byte says how long it is.
            let pad = p.last().copied().unwrap_or(0) as usize;
            if (1..=16).contains(&pad) && pad <= p.len() {
                p.truncate(p.len() - pad);
            } else if self.checksum.is_empty() {
                return Err(CryptoError::WrongPassword);
            }
            p
        } else if alg.contains("blowfish") {
            blowfish_cfb(key, &self.iv, data)?
        } else {
            return Err(CryptoError::Unsupported(format!("cipher {}", self.algorithm)));
        };
        // The digest of the first kilobyte tells a wrong password.
        if !self.checksum.is_empty() {
            let head = &plain[..plain.len().min(1024)];
            let t = self.checksum_type.to_ascii_lowercase();
            let sum = if t.contains("sha256") { sha256(head) } else { sha1(head) };
            if sum != self.checksum {
                return Err(CryptoError::WrongPassword);
            }
        }
        Ok(plain)
    }
}

/// Blowfish in 64-bit CFB mode.
fn blowfish_cfb(key: &[u8], iv: &[u8], data: &[u8]) -> Result<Vec<u8>, CryptoError> {
    use blowfish::cipher::{BlockEncrypt, KeyInit};
    use blowfish::cipher::generic_array::GenericArray;
    let bf: blowfish::Blowfish = blowfish::Blowfish::new_from_slice(key).map_err(|_| CryptoError::Unsupported("Blowfish key".into()))?;
    let mut prev = [0u8; 8];
    prev.copy_from_slice(iv.get(..8).ok_or_else(|| CryptoError::Unsupported("Blowfish IV".into()))?);
    let mut out = Vec::with_capacity(data.len());
    for c in data.chunks(8) {
        let mut ks = GenericArray::clone_from_slice(&prev);
        bf.encrypt_block(&mut ks);
        out.extend(c.iter().zip(ks.iter()).map(|(a, b)| a ^ b));
        prev[..c.len()].copy_from_slice(c);
    }
    Ok(out)
}

/// Multiplication in GCM's field.
fn gf_mul(x: u128, y: u128) -> u128 {
    let (mut z, mut v) = (0u128, y);
    for i in 0..128 {
        if (x >> (127 - i)) & 1 == 1 {
            z ^= v;
        }
        v = if v & 1 == 1 { (v >> 1) ^ (0xE1 << 120) } else { v >> 1 };
    }
    z
}

/// AES-GCM with a 96-bit IV and no associated data; a wrong tag means a
/// wrong password.
fn gcm_decrypt(key: &[u8], iv: &[u8], ct: &[u8], tag: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let aes = crate::crypto::Aes::new(key)?;
    let block = |x: u128| {
        let mut b = x.to_be_bytes();
        aes.encrypt(&mut b);
        u128::from_be_bytes(b)
    };
    let h = block(0);
    let mut j0 = [0u8; 16];
    j0[..12].copy_from_slice(iv);
    j0[15] = 1;
    let j0 = u128::from_be_bytes(j0);
    let mut ghash = 0u128;
    let mut out = Vec::with_capacity(ct.len());
    for (i, c) in ct.chunks(16).enumerate() {
        let mut b = [0u8; 16];
        b[..c.len()].copy_from_slice(c);
        ghash = gf_mul(ghash ^ u128::from_be_bytes(b), h);
        // The counter is the IV's last 32 bits.
        let ctr = (j0 & !0xFFFF_FFFF) | u128::from((j0 as u32).wrapping_add(i as u32 + 1));
        let ks = block(ctr).to_be_bytes();
        out.extend(c.iter().zip(ks).map(|(a, k)| a ^ k));
    }
    ghash = gf_mul(ghash ^ (ct.len() as u128 * 8), h);
    if (block(j0) ^ ghash).to_be_bytes()[..] != tag[..] {
        return Err(CryptoError::WrongPassword);
    }
    Ok(out)
}

/// The manifest without its `encryption-data` elements.
fn strip_encryption(m: &str) -> String {
    let mut out = String::with_capacity(m.len());
    let mut rest = m;
    while let Some(i) = rest.find("encryption-data") {
        let Some(lt) = rest[..i].rfind('<') else { break };
        // Only start tags (`<manifest:encryption-data`).
        if rest[lt + 1..i].contains(|c: char| c == '/' || c == '>' || c.is_whitespace()) {
            out.push_str(&rest[..i + 15]);
            rest = &rest[i + 15..];
            continue;
        }
        out.push_str(&rest[..lt]);
        let tag_end = rest[i..].find('>').map_or(rest.len(), |e| i + e + 1);
        rest = if rest[..tag_end].ends_with("/>") {
            &rest[tag_end..]
        } else {
            let close = rest[tag_end..].find("encryption-data").map(|e| tag_end + e);
            match close.and_then(|c| rest[c..].find('>').map(|e| c + e + 1)) {
                Some(end) => &rest[end..],
                None => "",
            }
        };
    }
    out.push_str(rest);
    out
}

/// Decrypts an encrypted OpenDocument package into a plain one.
pub fn decrypt(data: &[u8], pw: &str) -> Result<Vec<u8>, CryptoError> {
    let bad = |e: crate::Error| CryptoError::Unsupported(e.0);
    let z = ZipReader::new(data).map_err(bad)?;
    let manifest = z.read_string(MANIFEST).map_err(bad)?;
    let parts = parts(&manifest)?;
    // Parts share their key derivation more often than not.
    let mut keys: HashMap<(Vec<u8>, String), Vec<u8>> = HashMap::new();
    // Each salt is a derivation of its own: a file of many parts, each
    // with its own salt and the most rounds, would take hours.
    let mut work = 0u64;
    let mut plain: HashMap<&str, Vec<u8>> = HashMap::new();
    // The smallest first: a wrong password shows fastest.
    let mut order: Vec<&Part> = parts.iter().collect();
    order.sort_by_key(|p| z.read(&p.path).map_or(0, |d| d.len()));
    // What all the parts may inflate to together.
    let mut budget: usize = 2 << 30;
    for p in order {
        let raw = z.read(&p.path).map_err(bad)?;
        let id = (p.salt.clone(), format!("{} {} {:?} {}", p.kdf, p.rounds, p.argon, p.start_key));
        let key = match keys.get(&id) {
            Some(k) => k.clone(),
            None => {
                work += p.cost();
                if work > MAX_WORK {
                    return Err(CryptoError::Unsupported("key derivation too costly".into()));
                }
                keys.entry(id).or_insert(p.key(pw)?).clone()
            }
        };
        let deflated = p.decipher(&key, &raw)?;
        // Parts are deflated before being encrypted.
        let bytes = match miniz_oxide::inflate::decompress_to_vec_with_limit(&deflated, budget) {
            Ok(b) => b,
            Err(e) if e.status == miniz_oxide::inflate::TINFLStatus::HasMoreOutput => {
                return Err(CryptoError::Unsupported("the document is too large".into()));
            }
            Err(_) => deflated,
        };
        budget = budget.saturating_sub(bytes.len());
        plain.insert(p.path.as_str(), bytes);
    }
    if let Some(inner) = plain.remove(WHOLE) {
        return Ok(inner);
    }
    let mut w = ZipWriter::new();
    let names: Vec<&str> = z.names().collect();
    for name in names.iter().filter(|n| **n == "mimetype").chain(names.iter().filter(|n| **n != "mimetype")) {
        match plain.remove(*name) {
            Some(bytes) => w.add(name, &bytes, true),
            None if *name == MANIFEST => w.add(name, strip_encryption(&manifest).as_bytes(), true),
            // Parts left unencrypted in an encrypted package were put
            // there by someone without the password (macros, a different
            // content): only the mimetype, folders and META-INF stay.
            None if *name == "mimetype" || name.ends_with('/') || name.starts_with("META-INF/") => w.add(name, &z.read(name).map_err(bad)?, *name != "mimetype"),
            None => {}
        }
    }
    Ok(w.finish())
}

/// Encrypts an OpenDocument package with a password: its parts with
/// AES-256-CBC, keyed by PBKDF2 over the password's SHA-256. The manifest
/// and the mimetype stay readable; a thumbnail would show the content, so
/// it is left out.
pub fn encrypt(package: &[u8], pw: &str) -> Vec<u8> {
    encrypt_with(package, pw, ROUNDS)
}

fn encrypt_with(package: &[u8], pw: &str, rounds: u32) -> Vec<u8> {
    let Ok(z) = ZipReader::new(package) else { return package.to_vec() };
    let manifest = z.read_string(MANIFEST).unwrap_or_default();
    let salt = random(16);
    let mut key = vec![0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(&sha256(pw.as_bytes()), &salt, rounds, &mut key);
    let mut w = ZipWriter::new();
    // What the manifest says of each encrypted part.
    let mut info: HashMap<String, String> = HashMap::new();
    let names: Vec<&str> = z.names().collect();
    for name in names.iter().filter(|n| **n == "mimetype").chain(names.iter().filter(|n| **n != "mimetype")) {
        let Ok(data) = z.read(name) else { continue };
        if *name == "mimetype" {
            w.add(name, &data, false);
        } else if *name == MANIFEST || name.starts_with("Thumbnails/") {
            // Written last, with the encryption data.
        } else if name.ends_with('/') || name.starts_with("META-INF/") {
            w.add(name, &data, true);
        } else {
            let mut deflated = miniz_oxide::deflate::compress_to_vec(&data, 6);
            let sum = sha256(&deflated[..deflated.len().min(1024)]);
            let pad = 16 - deflated.len() % 16;
            deflated.extend(std::iter::repeat_n(pad as u8, pad));
            let iv = random(16);
            // The key has the right size, so encryption can't fail.
            let enc = cbc_encrypt(&key, &iv, &deflated).unwrap_or_default();
            w.add(name, &enc, false);
            info.insert(
                name.to_string(),
                format!(
                    r#" manifest:size="{}"><manifest:encryption-data manifest:checksum-type="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#sha256-1k" manifest:checksum="{}"><manifest:algorithm manifest:algorithm-name="http://www.w3.org/2001/04/xmlenc#aes256-cbc" manifest:initialisation-vector="{}"/><manifest:start-key-generation manifest:start-key-generation-name="http://www.w3.org/2000/09/xmldsig#sha256" manifest:key-size="32"/><manifest:key-derivation manifest:key-derivation-name="PBKDF2" manifest:key-size="32" manifest:iteration-count="{rounds}" manifest:salt="{}"/></manifest:encryption-data></manifest:file-entry>"#,
                    data.len(),
                    b64(&sum),
                    b64(&iv),
                    b64(&salt)
                ),
            );
        }
    }
    w.add(MANIFEST, encrypted_manifest(&manifest, info).as_bytes(), true);
    w.finish()
}

/// The manifest with the encryption data of the parts in `info` (parts
/// it didn't list are added).
fn encrypted_manifest(manifest: &str, mut info: HashMap<String, String>) -> String {
    let mut r = Reader::new(manifest);
    let mut root = r#" xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3""#.to_string();
    let mut entries = String::new();
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "manifest" => root = t.raw_attrs().to_string(),
            ev @ (Ev::Start(t) | Ev::Empty(t)) if t.local() == "file-entry" => {
                if matches!(ev, Ev::Start(_)) {
                    r.skip();
                }
                let path = t.attr("full-path").map(|v| v.into_owned()).unwrap_or_default();
                if path.starts_with("Thumbnails/") {
                    continue;
                }
                let attrs = t.raw_attrs().trim_end();
                match info.remove(&path) {
                    Some(enc) => {
                        let _ = write!(entries, "<manifest:file-entry{attrs}{enc}");
                    }
                    None => {
                        let _ = write!(entries, "<manifest:file-entry{attrs}/>");
                    }
                }
            }
            _ => {}
        }
    }
    let mut rest: Vec<(String, String)> = info.into_iter().collect();
    rest.sort();
    for (path, enc) in rest {
        let _ = write!(entries, r#"<manifest:file-entry manifest:full-path="{}" manifest:media-type=""{enc}"#, esc(&path));
    }
    format!(r#"<?xml version="1.0" encoding="UTF-8"?><manifest:manifest{root}>{entries}</manifest:manifest>"#)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package() -> Vec<u8> {
        let mut w = ZipWriter::new();
        w.add("mimetype", b"application/vnd.oasis.opendocument.text", false);
        w.add("content.xml", "<office:document-content>hola ñ</office:document-content>".repeat(100).as_bytes(), true);
        w.add("styles.xml", b"<s/>", true);
        w.add("Thumbnails/thumbnail.png", b"PNG", false);
        w.add("Pictures/a.bin", &(0..3000u32).map(|i| (i * 13 % 256) as u8).collect::<Vec<_>>(), false);
        w.add(MANIFEST, br#"<?xml version="1.0" encoding="UTF-8"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3"><manifest:file-entry manifest:full-path="/" manifest:version="1.3" manifest:media-type="application/vnd.oasis.opendocument.text"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="Thumbnails/thumbnail.png" manifest:media-type="image/png"/></manifest:manifest>"#, true);
        w.finish()
    }

    #[test]
    fn round_trip() {
        let plain = package();
        let enc = encrypt_with(&plain, "Clave ñ", 100);
        assert!(is_encrypted(&enc) && !is_encrypted(&plain));
        let z = ZipReader::new(&enc).unwrap();
        assert!(!z.contains("Thumbnails/thumbnail.png"));
        let m = z.read_string(MANIFEST).unwrap();
        assert_eq!(m.matches("<manifest:encryption-data").count(), 3, "{m}");
        assert!(m.contains(r#"manifest:full-path="styles.xml" manifest:media-type="""#));
        assert_eq!(decrypt(&enc, "clave ñ"), Err(CryptoError::WrongPassword));
        let dec = decrypt(&enc, "Clave ñ").unwrap();
        let (a, b) = (ZipReader::new(&plain).unwrap(), ZipReader::new(&dec).unwrap());
        for name in ["mimetype", "content.xml", "styles.xml", "Pictures/a.bin"] {
            assert_eq!(a.read(name).unwrap(), b.read(name).unwrap(), "{name}");
        }
        assert!(!is_encrypted(&dec));
        assert!(!b.read_string(MANIFEST).unwrap().contains("encryption"));
        // A part added without the password doesn't come out with the rest.
        let mut w = ZipWriter::new();
        for name in z.names() {
            w.add(name, &z.read(name).unwrap(), name != "mimetype");
        }
        w.add("Basic/Standard/Module1.xml", b"<script/>", true);
        let dec = decrypt(&w.finish(), "Clave ñ").unwrap();
        assert!(!ZipReader::new(&dec).unwrap().contains("Basic/Standard/Module1.xml"));
    }

    /// `enc` with its manifest edited by `f`.
    fn with_manifest(enc: &[u8], f: impl Fn(&str) -> String) -> Vec<u8> {
        let z = ZipReader::new(enc).unwrap();
        let mut w = ZipWriter::new();
        for n in z.names().map(str::to_string).collect::<Vec<_>>() {
            let d = z.read(&n).unwrap();
            let d = if n == MANIFEST { f(&String::from_utf8(d).unwrap()).into_bytes() } else { d };
            w.add(&n, &d, true);
        }
        w.finish()
    }

    #[test]
    fn costly_key_derivations_refused() {
        let enc = encrypt_with(&package(), "x", 100);
        // Argon2 with two billion iterations: refused at once.
        let argon = with_manifest(&enc, |m| m.replace(r#"key-derivation-name="PBKDF2""#, r#"key-derivation-name="urn:org:documentfoundation:names:experimental:office:manifest:argon2id" loext:argon2-iterations="2147483647" loext:argon2-memory="65536" loext:argon2-lanes="4""#));
        let t = std::time::Instant::now();
        assert!(matches!(decrypt(&argon, "x"), Err(CryptoError::Unsupported(_))));
        assert!(t.elapsed().as_secs() < 2);
        // Rounds past the most count (and run) as the most.
        let part = Part { kdf: "PBKDF2".into(), rounds: u32::MAX, ..Default::default() };
        assert_eq!(part.cost(), u64::from(MAX_ROUNDS));
    }

    #[test]
    fn manifest_stripped() {
        let m = r#"<m:manifest><m:file-entry m:full-path="a" m:size="3"><m:encryption-data m:checksum="x"><m:algorithm m:algorithm-name="y"/></m:encryption-data></m:file-entry><m:file-entry m:full-path="b"><m:encryption-data/></m:file-entry></m:manifest>"#;
        assert_eq!(strip_encryption(m), r#"<m:manifest><m:file-entry m:full-path="a" m:size="3"></m:file-entry><m:file-entry m:full-path="b"></m:file-entry></m:manifest>"#);
    }

    #[test]
    fn gcm_vector() {
        // NIST SP 800-38D test case 3 (AES-128), without associated data.
        let key = hex("feffe9928665731c6d6a8f9467308308");
        let iv = hex("cafebabefacedbaddecaf888");
        let ct = hex("42831ec2217774244b7221b784d0d49ce3aa212f2c02a4e035c17e2329aca12e21d514b25466931c7d8f6a5aac84aa051ba30b396a0aac973d58e091473f5985");
        let tag = hex("4d5c2af327cd64a62cf35abd2ba6fab4");
        let pt = gcm_decrypt(&key, &iv, &ct, &tag).unwrap();
        assert_eq!(pt, hex("d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b391aafd255"));
        assert_eq!(gcm_decrypt(&key, &iv, &ct, &[0; 16]), Err(CryptoError::WrongPassword));
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }
}
