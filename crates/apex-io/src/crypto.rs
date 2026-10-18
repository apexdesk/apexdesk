//! Password-encrypted Office files (MS-OFFCRYPTO): the zip package of an
//! .xlsx or .docx sits encrypted in an OLE compound file, next to its
//! encryption info. Reads the agile form (Office 2010 and later, and
//! LibreOffice) and the standard one (Office 2007); writes agile AES-256
//! with SHA-512, as Office does.

use std::io::{Cursor, Read, Write};

use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use sha2::Digest;

use crate::xml::{Ev, Reader};

/// Why an encrypted file couldn't be opened.
#[derive(Debug, PartialEq)]
pub enum CryptoError {
    WrongPassword,
    /// An encryption this reader doesn't handle (or a damaged file).
    Unsupported(String),
}

const OLE_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

// Block keys of the agile key derivation.
const VERIFIER_INPUT: [u8; 8] = [0xfe, 0xa7, 0xd2, 0x76, 0x3b, 0x4b, 0x9e, 0x79];
const VERIFIER_VALUE: [u8; 8] = [0xd7, 0xaa, 0x0f, 0x6d, 0x30, 0x61, 0x34, 0x4e];
const KEY_VALUE: [u8; 8] = [0x14, 0x6e, 0x0b, 0xe7, 0xab, 0xac, 0xd0, 0xd6];
const INTEGRITY_KEY: [u8; 8] = [0x5f, 0xb2, 0xad, 0x01, 0x0c, 0xb9, 0xe1, 0xf6];
const INTEGRITY_VALUE: [u8; 8] = [0xa0, 0x67, 0x7f, 0x02, 0xb2, 0x2c, 0x84, 0x33];

/// Package bytes encrypted as one unit.
const SEGMENT: usize = 4096;
/// Hash rounds for new files (as Office).
const SPIN: u32 = 100_000;

/// Whether the data is an OLE compound file holding an encrypted package.
pub fn is_encrypted(data: &[u8]) -> bool {
    if !data.starts_with(&OLE_MAGIC) {
        return false;
    }
    cfb::CompoundFile::open(Cursor::new(data)).is_ok_and(|c| c.is_stream("/EncryptionInfo") && c.is_stream("/EncryptedPackage"))
}

/// Whether the data is a file protected with Information Rights Management
/// (Azure RMS, a sensitivity label with encryption): its package is
/// encrypted with keys only the organization's service gives out.
pub fn is_rights_managed(data: &[u8]) -> bool {
    if !data.starts_with(&OLE_MAGIC) {
        return false;
    }
    let Ok(c) = cfb::CompoundFile::open(Cursor::new(data)) else { return false };
    let mut found = false;
    walk_within(&c, &mut |e| {
        let p = e.path().to_string_lossy();
        found = p.contains("DRMEncrypted") || p.ends_with("DRMContent");
        !found
    });
    found
}

/// Calls `f` on the entries of a compound file a few storages deep, until
/// it returns false (`cfb`'s own walk takes a deep tree's paths again at
/// each level).
pub(crate) fn walk_within<F: std::io::Read + std::io::Seek>(c: &cfb::CompoundFile<F>, f: &mut dyn FnMut(&cfb::Entry) -> bool) {
    let mut todo = vec![(std::path::PathBuf::from("/"), 0)];
    let mut left = 100_000usize;
    while let Some((path, depth)) = todo.pop() {
        let Ok(entries) = c.read_storage(&path) else { continue };
        for e in entries {
            left = match left.checked_sub(1) {
                Some(l) => l,
                None => return,
            };
            if !f(&e) {
                return;
            }
            if e.is_storage() && depth < 8 {
                todo.push((e.path().to_path_buf(), depth + 1));
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hash {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

impl Hash {
    fn parse(name: &str) -> Option<Hash> {
        Some(match name.to_ascii_uppercase().replace('-', "").as_str() {
            "SHA1" => Hash::Sha1,
            "SHA256" => Hash::Sha256,
            "SHA384" => Hash::Sha384,
            "SHA512" => Hash::Sha512,
            _ => return None,
        })
    }

    fn digest(self, parts: &[&[u8]]) -> Vec<u8> {
        fn run<D: Digest>(parts: &[&[u8]]) -> Vec<u8> {
            let mut d = D::new();
            for p in parts {
                d.update(p);
            }
            d.finalize().to_vec()
        }
        match self {
            Hash::Sha1 => run::<sha1::Sha1>(parts),
            Hash::Sha256 => run::<sha2::Sha256>(parts),
            Hash::Sha384 => run::<sha2::Sha384>(parts),
            Hash::Sha512 => run::<sha2::Sha512>(parts),
        }
    }

    fn block_len(self) -> usize {
        match self {
            Hash::Sha1 | Hash::Sha256 => 64,
            Hash::Sha384 | Hash::Sha512 => 128,
        }
    }
}

/// AES with a 128, 192 or 256-bit key.
pub(crate) enum Aes {
    A128(aes::Aes128),
    A192(aes::Aes192),
    A256(aes::Aes256),
}

impl Aes {
    pub(crate) fn new(key: &[u8]) -> Result<Aes, CryptoError> {
        Ok(match key.len() {
            16 => Aes::A128(aes::Aes128::new(GenericArray::from_slice(key))),
            24 => Aes::A192(aes::Aes192::new(GenericArray::from_slice(key))),
            32 => Aes::A256(aes::Aes256::new(GenericArray::from_slice(key))),
            n => return Err(CryptoError::Unsupported(format!("AES key of {n} bytes"))),
        })
    }

    pub(crate) fn decrypt(&self, b: &mut [u8]) {
        let b = GenericArray::from_mut_slice(b);
        match self {
            Aes::A128(c) => c.decrypt_block(b),
            Aes::A192(c) => c.decrypt_block(b),
            Aes::A256(c) => c.decrypt_block(b),
        }
    }

    pub(crate) fn encrypt(&self, b: &mut [u8]) {
        let b = GenericArray::from_mut_slice(b);
        match self {
            Aes::A128(c) => c.encrypt_block(b),
            Aes::A192(c) => c.encrypt_block(b),
            Aes::A256(c) => c.encrypt_block(b),
        }
    }
}

/// AES-CBC without padding (a trailing partial block is left out).
pub(crate) fn cbc_decrypt(key: &[u8], iv: &[u8], data: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let aes = Aes::new(key)?;
    let mut prev = [0u8; 16];
    prev.copy_from_slice(iv.get(..16).ok_or_else(|| CryptoError::Unsupported("IV of less than 16 bytes".into()))?);
    let mut out = Vec::with_capacity(data.len());
    for c in data.chunks_exact(16) {
        let mut b = [0u8; 16];
        b.copy_from_slice(c);
        aes.decrypt(&mut b);
        for (x, p) in b.iter_mut().zip(prev) {
            *x ^= p;
        }
        out.extend_from_slice(&b);
        prev.copy_from_slice(c);
    }
    Ok(out)
}

/// AES-CBC; the data is padded with zeros to whole blocks.
pub(crate) fn cbc_encrypt(key: &[u8], iv: &[u8], data: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let aes = Aes::new(key)?;
    let mut prev = [0u8; 16];
    prev.copy_from_slice(iv.get(..16).ok_or_else(|| CryptoError::Unsupported("IV of less than 16 bytes".into()))?);
    let mut out = Vec::with_capacity(data.len().div_ceil(16) * 16);
    for c in data.chunks(16) {
        let mut b = [0u8; 16];
        b[..c.len()].copy_from_slice(c);
        for (x, p) in b.iter_mut().zip(prev) {
            *x ^= p;
        }
        aes.encrypt(&mut b);
        out.extend_from_slice(&b);
        prev = b;
    }
    Ok(out)
}

/// `h` cut or padded with 0x36 to `len` bytes.
fn fit(mut h: Vec<u8>, len: usize) -> Vec<u8> {
    h.resize(len, 0x36);
    h
}

fn utf16(pw: &str) -> Vec<u8> {
    pw.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

/// The iterated password hash: H(salt + password), then `spin` rounds of
/// H(round + hash).
fn password_hash(hash: Hash, salt: &[u8], pw: &str, spin: u32) -> Vec<u8> {
    let mut h = hash.digest(&[salt, &utf16(pw)]);
    for i in 0..spin {
        h = hash.digest(&[&i.to_le_bytes(), &h]);
    }
    h
}

fn hmac(hash: Hash, key: &[u8], data: &[u8]) -> Vec<u8> {
    let block = hash.block_len();
    let mut k = if key.len() > block { hash.digest(&[key]) } else { key.to_vec() };
    k.resize(block, 0);
    let ipad: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    let opad: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    let inner = hash.digest(&[&ipad, data]);
    hash.digest(&[&opad, &inner])
}

pub(crate) fn random(n: usize) -> Vec<u8> {
    let mut v = vec![0u8; n];
    if getrandom::fill(&mut v).is_err() {
        // No system randomness: still unique per file, if weaker.
        use std::hash::{BuildHasher, Hasher};
        for (i, chunk) in v.chunks_mut(8).enumerate() {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_usize(i);
            h.write_u128(web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
            chunk.copy_from_slice(&h.finish().to_le_bytes()[..chunk.len()]);
        }
    }
    v
}

pub(crate) fn b64(data: &[u8]) -> String {
    crate::markdown::base64_encode(data)
}

pub(crate) fn unb64(s: &str) -> Result<Vec<u8>, CryptoError> {
    crate::markdown::base64_decode(s).ok_or_else(|| CryptoError::Unsupported("bad base64".into()))
}

/// Cipher settings of `keyData` or of the password key encryptor.
#[derive(Default)]
struct Params {
    salt: Vec<u8>,
    hash: String,
    key_bits: usize,
    block: usize,
    hash_size: usize,
    cipher: String,
    chaining: String,
    spin: u32,
    verifier_input: Vec<u8>,
    verifier_value: Vec<u8>,
    key_value: Vec<u8>,
}

impl Params {
    fn hash(&self) -> Result<Hash, CryptoError> {
        Hash::parse(&self.hash).ok_or_else(|| CryptoError::Unsupported(format!("hash {}", self.hash)))
    }

    fn check(&self) -> Result<(), CryptoError> {
        if !self.cipher.eq_ignore_ascii_case("AES") || !self.chaining.eq_ignore_ascii_case("ChainingModeCBC") || self.block != 16 {
            return Err(CryptoError::Unsupported(format!("{} {}", self.cipher, self.chaining)));
        }
        // AES keys: the file's size would otherwise be allocated as given.
        if !matches!(self.key_bits, 128 | 192 | 256) {
            return Err(CryptoError::Unsupported(format!("key of {} bits", self.key_bits)));
        }
        Ok(())
    }

    /// The IV for a block key: H(salt + block key), fitted to a block.
    fn iv(&self, block_key: &[u8]) -> Result<Vec<u8>, CryptoError> {
        Ok(fit(self.hash()?.digest(&[&self.salt, block_key]), self.block))
    }
}

fn read_params(t: &crate::xml::Tag) -> Result<Params, CryptoError> {
    let s = |k: &str| t.attr(k).map(|v| v.into_owned()).unwrap_or_default();
    let n = |k: &str| t.attr(k).and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
    let bytes = |k: &str| t.attr(k).map_or(Ok(Vec::new()), |v| unb64(&v));
    Ok(Params {
        salt: bytes("saltValue")?,
        hash: s("hashAlgorithm"),
        key_bits: n("keyBits"),
        block: n("blockSize"),
        hash_size: n("hashSize"),
        cipher: s("cipherAlgorithm"),
        chaining: s("cipherChaining"),
        spin: n("spinCount").min(10_000_000) as u32,
        verifier_input: bytes("encryptedVerifierHashInput")?,
        verifier_value: bytes("encryptedVerifierHashValue")?,
        key_value: bytes("encryptedKeyValue")?,
    })
}

fn read_stream(c: &mut cfb::CompoundFile<Cursor<&[u8]>>, path: &str) -> Result<Vec<u8>, CryptoError> {
    let mut s = c.open_stream(path).map_err(|e| CryptoError::Unsupported(e.to_string()))?;
    let mut out = Vec::new();
    s.read_to_end(&mut out).map_err(|e| CryptoError::Unsupported(e.to_string()))?;
    Ok(out)
}

/// Decrypts an encrypted Office file into its zip package.
pub fn decrypt(data: &[u8], password: &str) -> Result<Vec<u8>, CryptoError> {
    let mut c = cfb::CompoundFile::open(Cursor::new(data)).map_err(|e| CryptoError::Unsupported(e.to_string()))?;
    let info = read_stream(&mut c, "/EncryptionInfo")?;
    let package = read_stream(&mut c, "/EncryptedPackage")?;
    if info.len() < 8 || package.len() < 8 {
        return Err(CryptoError::Unsupported("short encryption streams".into()));
    }
    let (major, minor) = (u16::from_le_bytes([info[0], info[1]]), u16::from_le_bytes([info[2], info[3]]));
    match (major, minor) {
        (4, 4) => decrypt_agile(&info[8..], &package, password),
        (2..=4, 2) => decrypt_standard(&info[8..], &package, password),
        _ => Err(CryptoError::Unsupported(format!("encryption version {major}.{minor}"))),
    }
}

fn decrypt_agile(xml: &[u8], package: &[u8], password: &str) -> Result<Vec<u8>, CryptoError> {
    let xml = String::from_utf8_lossy(xml);
    let mut r = Reader::new(&xml);
    let (mut data, mut key, mut integrity) = (None, None, None);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) => match t.local() {
                "keyData" => data = Some(read_params(&t)?),
                "dataIntegrity" => {
                    let bytes = |k: &str| t.attr(k).map_or(Ok(Vec::new()), |v| unb64(&v));
                    integrity = Some((bytes("encryptedHmacKey")?, bytes("encryptedHmacValue")?));
                }
                "encryptedKey" if t.name.starts_with("p:") || key.is_none() => key = Some(read_params(&t)?),
                _ => {}
            },
            _ => {}
        }
    }
    let (Some(data), Some(key)) = (data, key) else { return Err(CryptoError::Unsupported("no password key encryptor".into())) };
    data.check()?;
    key.check()?;
    let kh = key.hash()?;
    let h = password_hash(kh, &key.salt, password, key.spin);
    let key_len = key.key_bits / 8;
    let iv = fit(key.salt.clone(), key.block);
    let derive = |block_key: &[u8]| fit(kh.digest(&[&h, block_key]), key_len);
    let input = cbc_decrypt(&derive(&VERIFIER_INPUT), &iv, &key.verifier_input)?;
    let value = cbc_decrypt(&derive(&VERIFIER_VALUE), &iv, &key.verifier_value)?;
    let salt_len = key.salt.len().min(input.len());
    let check = kh.digest(&[&input[..salt_len]]);
    let hash_len = key.hash_size.min(check.len()).min(value.len());
    if hash_len == 0 || check[..hash_len] != value[..hash_len] {
        return Err(CryptoError::WrongPassword);
    }
    let mut secret = cbc_decrypt(&derive(&KEY_VALUE), &iv, &key.key_value)?;
    secret.truncate(data.key_bits / 8);
    // The package's HMAC: without it, someone without the password could
    // still change what the file says (CBC lets blocks be altered).
    let altered = || CryptoError::Unsupported("the file was altered after it was encrypted".into());
    let (hmac_key, hmac_value) = integrity.ok_or_else(altered)?;
    let hmac_key = cbc_decrypt(&secret, &data.iv(&INTEGRITY_KEY)?, &hmac_key)?;
    let mut expected = cbc_decrypt(&secret, &data.iv(&INTEGRITY_VALUE)?, &hmac_value)?;
    // The whole digest: hashSize isn't covered by anything, so a smaller
    // one would let a guess pass.
    let mac = hmac(data.hash()?, &hmac_key[..data.hash_size.min(hmac_key.len())], package);
    expected.truncate(mac.len());
    if data.hash_size != mac.len() || expected != mac {
        return Err(altered());
    }
    let size = u64::from_le_bytes(package[..8].try_into().unwrap_or_default()) as usize;
    let mut out = Vec::with_capacity(package.len());
    for (i, seg) in package[8..].chunks(SEGMENT).enumerate() {
        out.extend(cbc_decrypt(&secret, &data.iv(&(i as u32).to_le_bytes())?, seg)?);
    }
    if size > out.len() {
        return Err(CryptoError::Unsupported("truncated package".into()));
    }
    out.truncate(size);
    Ok(out)
}

fn decrypt_standard(info: &[u8], package: &[u8], password: &str) -> Result<Vec<u8>, CryptoError> {
    let u32_at = |o: usize| info.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let bad = || CryptoError::Unsupported("bad standard encryption header".into());
    let header_len = u32_at(0).ok_or_else(bad)? as usize;
    let h = 4;
    let alg = u32_at(h + 8).ok_or_else(bad)?;
    let key_bits = u32_at(h + 16).ok_or_else(bad)? as usize;
    if !matches!(alg, 0x660E..=0x6610) {
        return Err(CryptoError::Unsupported(format!("cipher {alg:#x}")));
    }
    let v = h + header_len;
    let salt_len = u32_at(v).ok_or_else(bad)? as usize;
    let salt = info.get(v + 4..v + 4 + salt_len).ok_or_else(bad)?;
    let enc_verifier = info.get(v + 4 + salt_len..v + 20 + salt_len).ok_or_else(bad)?;
    let enc_hash = info.get(v + 24 + salt_len..v + 56 + salt_len).ok_or_else(bad)?;
    let hfinal = Hash::Sha1.digest(&[&password_hash(Hash::Sha1, salt, password, 50_000), &0u32.to_le_bytes()]);
    let mut buf1 = [0x36u8; 64];
    let mut buf2 = [0x5cu8; 64];
    for (i, b) in hfinal.iter().enumerate() {
        buf1[i] ^= b;
        buf2[i] ^= b;
    }
    let mut x = Hash::Sha1.digest(&[&buf1]);
    x.extend(Hash::Sha1.digest(&[&buf2]));
    let key = &x[..(key_bits / 8).min(x.len())];
    let aes = Aes::new(key)?;
    let ecb = |data: &[u8]| {
        let mut out = data.to_vec();
        for b in out.chunks_exact_mut(16) {
            aes.decrypt(b);
        }
        out
    };
    let verifier = ecb(enc_verifier);
    let hash = ecb(enc_hash);
    if Hash::Sha1.digest(&[&verifier])[..] != hash[..20] {
        return Err(CryptoError::WrongPassword);
    }
    let size = u64::from_le_bytes(package[..8].try_into().unwrap_or_default()) as usize;
    let body = &package[8..package.len() - (package.len() - 8) % 16];
    let mut out = ecb(body);
    if size > out.len() {
        return Err(CryptoError::Unsupported("truncated package".into()));
    }
    out.truncate(size);
    Ok(out)
}

/// A length-prefixed UTF-16 string padded to 4 bytes (UNICODE-LP-P4).
fn lp_p4(out: &mut Vec<u8>, s: &str) {
    let b = utf16(s);
    out.extend((b.len() as u32).to_le_bytes());
    out.extend(&b);
    out.resize(out.len().div_ceil(4) * 4, 0);
}

/// The `\x06DataSpaces` streams that say the package is encrypted.
fn data_spaces() -> [(&'static str, Vec<u8>); 4] {
    let mut version = Vec::new();
    lp_p4(&mut version, "Microsoft.Container.DataSpaces");
    for _ in 0..3 {
        version.extend([1, 0, 0, 0]);
    }
    let mut entry = Vec::new();
    entry.extend(1u32.to_le_bytes());
    entry.extend(0u32.to_le_bytes());
    lp_p4(&mut entry, "EncryptedPackage");
    lp_p4(&mut entry, "StrongEncryptionDataSpace");
    let mut map = Vec::new();
    map.extend(8u32.to_le_bytes());
    map.extend(1u32.to_le_bytes());
    map.extend((entry.len() as u32 + 4).to_le_bytes());
    map.extend(entry);
    let mut space = Vec::new();
    space.extend(8u32.to_le_bytes());
    space.extend(1u32.to_le_bytes());
    lp_p4(&mut space, "StrongEncryptionTransform");
    let mut primary = Vec::new();
    let id = "{FF9A3F03-56EF-4613-BDD5-5A41C1D07246}";
    primary.extend((12 + 2 * id.len() as u32).to_le_bytes());
    primary.extend(1u32.to_le_bytes());
    lp_p4(&mut primary, id);
    lp_p4(&mut primary, "Microsoft.Container.EncryptionTransform");
    for _ in 0..3 {
        primary.extend([1, 0, 0, 0]);
    }
    primary.extend(0u32.to_le_bytes());
    primary.extend(0u32.to_le_bytes());
    primary.extend(0u32.to_le_bytes());
    primary.extend(4u32.to_le_bytes());
    [
        ("/\u{6}DataSpaces/Version", version),
        ("/\u{6}DataSpaces/DataSpaceMap", map),
        ("/\u{6}DataSpaces/DataSpaceInfo/StrongEncryptionDataSpace", space),
        ("/\u{6}DataSpaces/TransformInfo/StrongEncryptionTransform/\u{6}Primary", primary),
    ]
}

/// Encrypts a zip package with a password (agile, AES-256, SHA-512).
pub fn encrypt(package: &[u8], password: &str) -> Vec<u8> {
    encrypt_with(package, password, SPIN)
}

fn encrypt_with(package: &[u8], password: &str, spin: u32) -> Vec<u8> {
    let hash = Hash::Sha512;
    let data = Params { salt: random(16), hash: "SHA512".into(), key_bits: 256, block: 16, hash_size: 64, ..Default::default() };
    let key_salt = random(16);
    let secret = random(32);
    // The key and IVs have the right sizes, so encryption can't fail.
    let enc = |key: &[u8], iv: &[u8], d: &[u8]| cbc_encrypt(key, iv, d).unwrap_or_default();
    let iv = |block_key: &[u8]| data.iv(block_key).unwrap_or_default();
    let mut stream = (package.len() as u64).to_le_bytes().to_vec();
    for (i, seg) in package.chunks(SEGMENT).enumerate() {
        stream.extend(enc(&secret, &iv(&(i as u32).to_le_bytes()), seg));
    }
    let h = password_hash(hash, &key_salt, password, spin);
    let derive = |block_key: &[u8]| fit(hash.digest(&[&h, block_key]), 32);
    let verifier = random(16);
    let enc_input = enc(&derive(&VERIFIER_INPUT), &key_salt, &verifier);
    let enc_value = enc(&derive(&VERIFIER_VALUE), &key_salt, &hash.digest(&[&verifier]));
    let enc_key = enc(&derive(&KEY_VALUE), &key_salt, &secret);
    let hmac_key = random(64);
    let enc_hmac_key = enc(&secret, &iv(&INTEGRITY_KEY), &hmac_key);
    let enc_hmac_value = enc(&secret, &iv(&INTEGRITY_VALUE), &hmac(hash, &hmac_key, &stream));
    let cipher = r#"blockSize="16" keyBits="256" hashSize="64" cipherAlgorithm="AES" cipherChaining="ChainingModeCBC" hashAlgorithm="SHA512""#;
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<encryption xmlns="http://schemas.microsoft.com/office/2006/encryption" xmlns:p="http://schemas.microsoft.com/office/2006/keyEncryptor/password" xmlns:c="http://schemas.microsoft.com/office/2006/keyEncryptor/certificate"><keyData saltSize="16" {cipher} saltValue="{}"/><dataIntegrity encryptedHmacKey="{}" encryptedHmacValue="{}"/><keyEncryptors><keyEncryptor uri="http://schemas.microsoft.com/office/2006/keyEncryptor/password"><p:encryptedKey spinCount="{spin}" saltSize="16" {cipher} saltValue="{}" encryptedVerifierHashInput="{}" encryptedVerifierHashValue="{}" encryptedKeyValue="{}"/></keyEncryptor></keyEncryptors></encryption>"#,
        b64(&data.salt),
        b64(&enc_hmac_key),
        b64(&enc_hmac_value),
        b64(&key_salt),
        b64(&enc_input),
        b64(&enc_value),
        b64(&enc_key)
    );
    let mut info = vec![4, 0, 4, 0, 0x40, 0, 0, 0];
    info.extend(xml.as_bytes());
    compound(&[("/EncryptionInfo", info), ("/EncryptedPackage", stream)])
}

/// An OLE compound file with the data spaces and these streams.
fn compound(streams: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let build = || -> std::io::Result<Vec<u8>> {
        let mut c = cfb::CompoundFile::create(Cursor::new(Vec::new()))?;
        for dir in ["/\u{6}DataSpaces", "/\u{6}DataSpaces/DataSpaceInfo", "/\u{6}DataSpaces/TransformInfo", "/\u{6}DataSpaces/TransformInfo/StrongEncryptionTransform"] {
            c.create_storage(dir)?;
        }
        for (path, data) in data_spaces().iter().map(|(p, d)| (*p, d)).chain(streams.iter().map(|(p, d)| (*p, d))) {
            let mut s = c.create_stream(path)?;
            s.write_all(data)?;
        }
        c.flush()?;
        Ok(c.into_inner().into_inner())
    };
    build().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rights_management() {
        let mut c = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        for dir in ["/\u{6}DataSpaces", "/\u{6}DataSpaces/TransformInfo", "/\u{6}DataSpaces/TransformInfo/DRMEncryptedTransform"] {
            c.create_storage(dir).unwrap();
        }
        c.create_stream("/\u{6}DataSpaces/TransformInfo/DRMEncryptedTransform/\u{6}Primary").unwrap().write_all(b"x").unwrap();
        c.create_stream("/EncryptedPackage").unwrap().write_all(&[0; 64]).unwrap();
        c.flush().unwrap();
        let rms = c.into_inner().into_inner();
        assert!(is_rights_managed(&rms) && !is_encrypted(&rms));
        let pw = encrypt_with(b"PK", "x", 10);
        assert!(!is_rights_managed(&pw) && is_encrypted(&pw));
    }

    #[test]
    fn short_iv_is_an_error() {
        // A damaged encryption header with a 2-byte salt.
        assert!(cbc_decrypt(&[0; 16], &[0; 2], &[0; 32]).is_err());
        assert!(cbc_encrypt(&[0; 16], &[0; 2], &[0; 32]).is_err());
    }

    #[test]
    fn round_trip() {
        let zip: Vec<u8> = (0..10_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let file = encrypt_with(&zip, "Secreto ñ", 1000);
        assert!(is_encrypted(&file));
        assert_eq!(decrypt(&file, "Secreto ñ").unwrap(), zip);
        assert_eq!(decrypt(&file, "secreto ñ"), Err(CryptoError::WrongPassword));
        assert!(!is_encrypted(&zip));
        // A changed block is caught by the package's HMAC.
        use std::io::{Seek, Write};
        let mut c = cfb::CompoundFile::open(Cursor::new(file.clone())).unwrap();
        let mut stream = read_stream(&mut cfb::CompoundFile::open(Cursor::new(&file[..])).unwrap(), "/EncryptedPackage").unwrap();
        stream[40] ^= 1;
        let mut s = c.open_stream("/EncryptedPackage").unwrap();
        s.seek(std::io::SeekFrom::Start(0)).unwrap();
        s.write_all(&stream).unwrap();
        drop(s);
        let tampered = c.into_inner().into_inner();
        assert!(matches!(decrypt(&tampered, "Secreto ñ"), Err(CryptoError::Unsupported(_))));
    }

    #[test]
    fn hmac_sha512_vector() {
        // RFC 4231, test case 2.
        let mac = hmac(Hash::Sha512, b"Jefe", b"what do ya want for nothing?");
        assert_eq!(mac[..8], [0x16, 0x4b, 0x7a, 0x7b, 0xfc, 0xf8, 0x19, 0xe2]);
    }

    #[test]
    fn standard_encryption() {
        // A standard (Office 2007) file built by hand the same way Office
        // derives its key.
        let zip: Vec<u8> = (0..1000u32).map(|i| (i % 256) as u8).collect();
        let salt = [7u8; 16];
        let hfinal = Hash::Sha1.digest(&[&password_hash(Hash::Sha1, &salt, "pw", 50_000), &0u32.to_le_bytes()]);
        let mut b1 = [0x36u8; 64];
        let mut b2 = [0x5cu8; 64];
        for (i, b) in hfinal.iter().enumerate() {
            b1[i] ^= b;
            b2[i] ^= b;
        }
        let mut x = Hash::Sha1.digest(&[&b1]);
        x.extend(Hash::Sha1.digest(&[&b2]));
        let aes = Aes::new(&x[..16]).unwrap();
        let ecb = |d: &[u8]| {
            let mut o = d.to_vec();
            o.resize(o.len().div_ceil(16) * 16, 0);
            for b in o.chunks_exact_mut(16) {
                aes.encrypt(b);
            }
            o
        };
        let verifier = [3u8; 16];
        let mut vh = Hash::Sha1.digest(&[&verifier]);
        vh.resize(32, 0);
        let mut header = Vec::new();
        for v in [0u32, 0, 0x660E, 0x8004, 128, 0x18, 0, 0] {
            header.extend(v.to_le_bytes());
        }
        let mut info = vec![3, 0, 2, 0, 0x24, 0, 0, 0];
        info.extend((header.len() as u32).to_le_bytes());
        info.extend(&header);
        info.extend(16u32.to_le_bytes());
        info.extend(salt);
        info.extend(ecb(&verifier));
        info.extend(20u32.to_le_bytes());
        info.extend(ecb(&vh));
        let mut pkg = (zip.len() as u64).to_le_bytes().to_vec();
        pkg.extend(ecb(&zip));
        let file = compound(&[("/EncryptionInfo", info), ("/EncryptedPackage", pkg)]);
        assert_eq!(decrypt(&file, "pw").unwrap(), zip);
        assert_eq!(decrypt(&file, "no"), Err(CryptoError::WrongPassword));
    }
}

/// Cross-check with other tools: `APEX_CRYPTO_DIR=dir cargo test -p apex-io
/// cross_check -- --ignored` writes `ours.docx` (password "pw") from
/// `plain.docx` and decrypts `theirs.docx` into `theirs_plain.docx`.
#[cfg(test)]
#[test]
#[ignore]
fn cross_check() {
    let dir = std::path::PathBuf::from(std::env::var("APEX_CRYPTO_DIR").unwrap());
    let plain = apex_fs::read(dir.join("plain.docx")).unwrap();
    apex_fs::write(dir.join("ours.docx"), encrypt(&plain, "pw")).unwrap();
    if let Ok(theirs) = apex_fs::read(dir.join("theirs.docx")) {
        apex_fs::write(dir.join("theirs_plain.docx"), decrypt(&theirs, "pw").unwrap()).unwrap();
    }
}
