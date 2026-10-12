//! Password-protected binary Office files (MS-OFFCRYPTO 2.3.5 to 2.3.7):
//! .xls and .doc encrypted with RC4 (Office 97 and 2000, and LibreOffice),
//! with RC4 CryptoAPI (Office XP and 2003), or obfuscated with XOR (Excel
//! 5.0/95, Word 95). Only decrypts, with the password the user gives.

use md5::Digest as _;
use sha1::Digest as _;

use crate::Error;
use crate::crypto::CryptoError;

/// The RC4 stream cipher.
pub(crate) struct Rc4 {
    s: [u8; 256],
    i: u8,
    j: u8,
}

impl Rc4 {
    pub(crate) fn new(key: &[u8]) -> Rc4 {
        let mut s = [0u8; 256];
        for (i, x) in s.iter_mut().enumerate() {
            *x = i as u8;
        }
        let mut j = 0u8;
        for i in 0..256 {
            j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
            s.swap(i, j as usize);
        }
        Rc4 { s, i: 0, j: 0 }
    }

    pub(crate) fn apply(&mut self, data: &mut [u8]) {
        for b in data {
            self.i = self.i.wrapping_add(1);
            self.j = self.j.wrapping_add(self.s[self.i as usize]);
            self.s.swap(self.i as usize, self.j as usize);
            *b ^= self.s[self.s[self.i as usize].wrapping_add(self.s[self.j as usize]) as usize];
        }
    }
}

fn utf16(pw: &str) -> Vec<u8> {
    pw.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn md5(parts: &[&[u8]]) -> Vec<u8> {
    let mut d = md5::Md5::new();
    for p in parts {
        d.update(p);
    }
    d.finalize().to_vec()
}

fn sha1(parts: &[&[u8]]) -> Vec<u8> {
    let mut d = sha1::Sha1::new();
    for p in parts {
        d.update(p);
    }
    d.finalize().to_vec()
}

/// The key of an RC4-encrypted stream: each block of it (512 bytes in
/// .doc, 1024 in .xls) starts the cipher again with a key of its own.
pub(crate) enum Rc4Key {
    /// Office 97: MD5 based, 128-bit block keys.
    Standard([u8; 5]),
    /// CryptoAPI: SHA-1 based, `bits` long (40-bit keys padded to 128).
    CryptoApi { h0: Vec<u8>, bits: usize },
}

impl Rc4Key {
    fn block_key(&self, n: u32) -> Vec<u8> {
        match self {
            Rc4Key::Standard(base) => md5(&[base, &n.to_le_bytes()]),
            Rc4Key::CryptoApi { h0, bits } => {
                let h = sha1(&[h0, &n.to_le_bytes()]);
                if *bits == 40 {
                    let mut k = h[..5].to_vec();
                    k.resize(16, 0);
                    k
                } else {
                    h[..(bits / 8).min(h.len())].to_vec()
                }
            }
        }
    }

    /// The bytes XORed with a stream of `len` bytes from its start.
    pub(crate) fn keystream(&self, len: usize, block: usize) -> Vec<u8> {
        let mut out = vec![0u8; len];
        for (n, chunk) in out.chunks_mut(block).enumerate() {
            Rc4::new(&self.block_key(n as u32)).apply(chunk);
        }
        out
    }
}

/// The RC4 key from an encryption header (`EncryptionVersionInfo` and
/// what follows), checking the password against its verifier.
pub(crate) fn rc4_key(info: &[u8], pw: &str) -> Result<Rc4Key, CryptoError> {
    let bad = || CryptoError::Unsupported("bad RC4 encryption header".into());
    let u16_at = |o: usize| info.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |o: usize| info.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let (major, minor) = (u16_at(0).ok_or_else(bad)?, u16_at(2).ok_or_else(bad)?);
    let (key, verifier, hash_len) = match (major, minor) {
        (1, 1) => {
            let salt = info.get(4..20).ok_or_else(bad)?;
            let h0 = md5(&[&utf16(pw)]);
            let buf: Vec<u8> = (0..16).flat_map(|_| h0[..5].iter().chain(salt).copied()).collect();
            let h1 = md5(&[&buf]);
            let mut base = [0u8; 5];
            base.copy_from_slice(&h1[..5]);
            (Rc4Key::Standard(base), info.get(20..52).ok_or_else(bad)?.to_vec(), 16)
        }
        (2..=4, 2) => {
            let header_len = u32_at(8).ok_or_else(bad)? as usize;
            let bits = match u32_at(12 + 16).ok_or_else(bad)? as usize {
                0 => 40,
                n => n,
            };
            if !(40..=128).contains(&bits) || bits % 8 != 0 {
                return Err(CryptoError::Unsupported(format!("RC4 key of {bits} bits")));
            }
            let v = 12 + header_len;
            let salt_len = u32_at(v).ok_or_else(bad)? as usize;
            let salt = info.get(v + 4..v + 4 + salt_len).ok_or_else(bad)?;
            let enc_verifier = info.get(v + 4 + salt_len..v + 20 + salt_len).ok_or_else(bad)?;
            let enc_hash = info.get(v + 24 + salt_len..v + 44 + salt_len).ok_or_else(bad)?;
            let h0 = sha1(&[salt, &utf16(pw)]);
            (Rc4Key::CryptoApi { h0, bits }, [enc_verifier, enc_hash].concat(), 20)
        }
        _ => return Err(CryptoError::Unsupported(format!("encryption version {major}.{minor}"))),
    };
    let mut v = verifier;
    Rc4::new(&key.block_key(0)).apply(&mut v);
    let check = if hash_len == 16 { md5(&[&v[..16]]) } else { sha1(&[&v[..16]]) };
    if check[..] != v[16..16 + hash_len] {
        return Err(CryptoError::WrongPassword);
    }
    Ok(key)
}

// --- XOR obfuscation (MS-OFFCRYPTO 2.3.7) ----------------------------------

const PAD: [u8; 15] = [0xBB, 0xFF, 0xFF, 0xBA, 0xFF, 0xFF, 0xB9, 0x80, 0x00, 0xBE, 0x0F, 0x00, 0xBF, 0x0F, 0x00];
const INITIAL_CODE: [u16; 15] = [0xE1F0, 0x1D0F, 0xCC9C, 0x84C0, 0x110C, 0x0E10, 0xF1CE, 0x313E, 0x1872, 0xE139, 0xD40F, 0x84F9, 0x280C, 0xA96A, 0x4EC3];
const XOR_MATRIX: [u16; 105] = [
    0xAEFC, 0x4DD9, 0x9BB2, 0x2745, 0x4E8A, 0x9D14, 0x2A09, 0x7B61, 0xF6C2, 0xFDA5, 0xEB6B, 0xC6F7, 0x9DCF, 0x2BBF, 0x4563, 0x8AC6, 0x05AD, 0x0B5A, 0x16B4, 0x2D68, 0x5AD0,
    0x0375, 0x06EA, 0x0DD4, 0x1BA8, 0x3750, 0x6EA0, 0xDD40, 0xD849, 0xA0B3, 0x5147, 0xA28E, 0x553D, 0xAA7A, 0x44D5, 0x6F45, 0xDE8A, 0xAD35, 0x4A4B, 0x9496, 0x390D, 0x721A,
    0xEB23, 0xC667, 0x9CEF, 0x29FF, 0x53FE, 0xA7FC, 0x5FD9, 0x47D3, 0x8FA6, 0x0F6D, 0x1EDA, 0x3DB4, 0x7B68, 0xF6D0, 0xB861, 0x60E3, 0xC1C6, 0x93AD, 0x377B, 0x6EF6, 0xDDEC,
    0x45A0, 0x8B40, 0x06A1, 0x0D42, 0x1A84, 0x3508, 0x6A10, 0xAA51, 0x4483, 0x8906, 0x022D, 0x045A, 0x08B4, 0x1168, 0x76B4, 0xED68, 0xCAF1, 0x85C3, 0x1BA7, 0x374E, 0x6E9C,
    0x3730, 0x6E60, 0xDCC0, 0xA9A1, 0x4363, 0x86C6, 0x1DAD, 0x3331, 0x6662, 0xCCC4, 0x89A9, 0x0373, 0x06E6, 0x0DCC, 0x1021, 0x2042, 0x4084, 0x8108, 0x1231, 0x2462, 0x48C4,
];

/// The password as the XOR methods see it: up to 15 bytes, each the low
/// byte of a character (the high one when that is zero).
fn xor_bytes(pw: &str) -> Vec<u8> {
    pw.encode_utf16().take(15).map(|u| if u & 0xFF != 0 { u as u8 } else { (u >> 8) as u8 }).collect()
}

/// The 16-bit password verifier (method 1).
pub(crate) fn xor_verifier(pw: &str) -> u16 {
    let b = xor_bytes(pw);
    let mut v: u16 = 0;
    for &c in b.iter().rev().chain(std::iter::once(&(b.len() as u8))) {
        v = (((v >> 14) & 1) | ((v << 1) & 0x7FFF)) ^ u16::from(c);
    }
    v ^ 0xCE4B
}

fn xor_key(b: &[u8]) -> u16 {
    let Some(&first) = b.len().checked_sub(1).and_then(|i| INITIAL_CODE.get(i)) else { return 0 };
    let mut key = first;
    let mut element = 0x68usize;
    for &c in b.iter().rev() {
        let mut c = c;
        for _ in 0..7 {
            if c & 0x40 != 0 {
                key ^= XOR_MATRIX[element];
            }
            c <<= 1;
            element = element.saturating_sub(1);
        }
    }
    key
}

/// The 16 bytes XORed with the data (array initialization method 1).
pub(crate) fn xor_array(pw: &str) -> [u8; 16] {
    let b = xor_bytes(pw);
    let mut out = [0u8; 16];
    if b.is_empty() {
        return out;
    }
    let key = xor_key(&b);
    let (hi, lo) = ((key >> 8) as u8, key as u8);
    let xor_ror = |a: u8, k: u8| (a ^ k).rotate_right(1);
    let mut i = b.len();
    if i % 2 == 1 {
        out[i] = xor_ror(PAD[0], hi);
        i -= 1;
        out[i] = xor_ror(b[b.len() - 1], lo);
    }
    while i > 0 {
        i -= 1;
        out[i] = xor_ror(b[i], hi);
        i -= 1;
        out[i] = xor_ror(b[i], lo);
    }
    // The rest, from the end, with the padding bytes in pairs.
    let (mut i, mut pad) = (15isize, 15 - b.len() as isize);
    while pad > 0 {
        out[i as usize] = xor_ror(PAD[pad as usize], hi);
        out[i as usize - 1] = xor_ror(PAD[pad as usize - 1], lo);
        i -= 2;
        pad -= 2;
    }
    out
}

// --- Excel ----------------------------------------------------------------

/// How the records of an encrypted workbook stream are decrypted.
enum Biff {
    Rc4(Rc4Key),
    Xor([u8; 16]),
}

/// Records whose data is never encrypted.
fn plain_record(ty: u16) -> bool {
    // BOF, FILEPASS, USREXCL, FILELOCK, INTERFACEHDR, RRDINFO, RRDHEAD.
    matches!(ty, 0x0809 | 0x002F | 0x0194 | 0x0195 | 0x00E1 | 0x0196 | 0x0138)
}

/// The data of a workbook stream's `FILEPASS` record.
fn filepass(stream: &[u8]) -> Option<Vec<u8>> {
    let mut p = 0;
    while p + 4 <= stream.len() {
        let ty = u16::from_le_bytes([stream[p], stream[p + 1]]);
        let len = u16::from_le_bytes([stream[p + 2], stream[p + 3]]) as usize;
        match ty {
            0x002F => return stream.get(p + 4..p + 4 + len).map(<[u8]>::to_vec),
            // It follows the BOF, before any sheet: no need to look further.
            0x000A | 0x0085 | 0x00FC => return None,
            _ => {}
        }
        p += 4 + len;
    }
    None
}

/// Whether the data is an .xls or .doc file encrypted with a password.
pub(crate) fn is_encrypted(data: &[u8]) -> bool {
    let Ok(mut ole) = crate::ole::open(data) else { return false };
    if let Some(wd) = crate::ole::stream(&mut ole, "/WordDocument") {
        // fEncrypted.
        return wd.get(0x0B).is_some_and(|b| b & 1 != 0);
    }
    crate::ole::stream(&mut ole, "/Workbook").or_else(|| crate::ole::stream(&mut ole, "/Book")).is_some_and(|s| filepass(&s).is_some())
}

/// Decrypts a workbook stream protected by a `FILEPASS` record in place;
/// false when it isn't encrypted.
pub(crate) fn decrypt_biff(stream: &mut [u8], biff5: bool, pw: Option<&str>) -> Result<bool, Error> {
    let Some(fp) = filepass(stream) else { return Ok(false) };
    let Some(pw) = pw else { return Err(crate::no_password()) };
    let u16_at = |o: usize| fp.get(o..o + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
    // BIFF5 has only the XOR form, without the type.
    let (kind, rest) = if biff5 { (0, &fp[..]) } else { (u16_at(0), fp.get(2..).unwrap_or(&[])) };
    let method = match kind {
        0 => {
            let verifier = rest.get(2..4).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
            if xor_verifier(pw) != verifier {
                return Err(crate::crypto_error(CryptoError::WrongPassword));
            }
            Biff::Xor(xor_array(pw))
        }
        1 => Biff::Rc4(rc4_key(rest, pw).map_err(crate::crypto_error)?),
        k => return Err(crate::crypto_error(CryptoError::Unsupported(format!("Excel encryption type {k}")))),
    };
    let ks = match &method {
        Biff::Rc4(k) => k.keystream(stream.len(), 1024),
        Biff::Xor(_) => Vec::new(),
    };
    let mut p = 0;
    while p + 4 <= stream.len() {
        let ty = u16::from_le_bytes([stream[p], stream[p + 1]]);
        let len = u16::from_le_bytes([stream[p + 2], stream[p + 3]]) as usize;
        let start = p + 4;
        let end = (start + len).min(stream.len());
        if !plain_record(ty) {
            // A sheet's stream position stays readable.
            let from = if ty == 0x0085 { start + 4 } else { start };
            for q in from..end {
                stream[q] = match &method {
                    Biff::Rc4(_) => stream[q] ^ ks[q],
                    Biff::Xor(a) => (stream[q] ^ a[(q + len) % 16]).rotate_right(5),
                };
            }
        }
        p = end;
    }
    Ok(true)
}

// --- Word -----------------------------------------------------------------

/// Bytes at the start of the WordDocument stream that stay readable (the
/// FIB's base).
const FIB_BASE: usize = 68;

/// Decrypts the streams of an encrypted Word document in place: the
/// WordDocument stream `wd`, the table stream and the Data stream (Word
/// 6/95 files have everything in `wd`). False when it isn't encrypted.
pub(crate) fn decrypt_word(wd: &mut [u8], table: &mut [u8], data: &mut [u8], pw: Option<&str>) -> Result<bool, Error> {
    let Some(flags) = wd.get(0x0A..0x0C).map(|b| u16::from_le_bytes([b[0], b[1]])) else { return Ok(false) };
    if flags & 0x0100 == 0 {
        return Ok(false);
    }
    let Some(pw) = pw else { return Err(crate::no_password()) };
    let nfib = u16::from_le_bytes([wd[2], wd[3]]);
    let lkey = wd.get(0x0E..0x12).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    // Word 95 only obfuscates; later versions say so with fObfuscation.
    if flags & 0x8000 != 0 || nfib < 0x00C1 {
        if xor_verifier(pw) != lkey as u16 {
            return Err(crate::crypto_error(CryptoError::WrongPassword));
        }
        let a = xor_array(pw);
        let xor = |d: &mut [u8], from: usize| {
            for (q, b) in d.iter_mut().enumerate().skip(from) {
                let x = a[q % 16];
                if *b != 0 && *b != x {
                    *b ^= x;
                }
            }
        };
        xor(wd, FIB_BASE);
        xor(table, 0);
        xor(data, 0);
    } else {
        let header = table.get(..lkey as usize).ok_or_else(|| Error::new("the document's encryption header is missing"))?;
        let key = rc4_key(header, pw).map_err(crate::crypto_error)?;
        for (s, from) in [(&mut *wd, FIB_BASE), (&mut *table, 0), (&mut *data, 0)] {
            let ks = key.keystream(s.len(), 512);
            for (b, k) in s.iter_mut().zip(ks).skip(from) {
                *b ^= k;
            }
        }
    }
    // Readable now: fEncrypted and fObfuscation off.
    let flags = flags & !0x8100;
    wd[0x0A..0x0C].copy_from_slice(&flags.to_le_bytes());
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc4_vector() {
        // RFC 6229 is for keystreams; the classic "Key"/"Plaintext" one.
        let mut d = *b"Plaintext";
        Rc4::new(b"Key").apply(&mut d);
        assert_eq!(d, [0xBB, 0xF3, 0x16, 0xE8, 0xD9, 0x40, 0xAF, 0x0A, 0xD3]);
    }

    #[test]
    fn xor_verifier_vector() {
        // Excel's default "VelvetSweatshop" password (MS-OFFCRYPTO).
        assert_eq!(xor_verifier("VelvetSweatshop"), 0x9A0A);
        assert_eq!(xor_verifier("abc"), apex_sheet::protect::legacy_hash("abc"));
    }

    #[test]
    fn standard_key_vector() {
        // The block key of MS-OFFCRYPTO's own example (as msoffcrypto-tool
        // checks it).
        let salt = [0xe8, 0x77, 0x2c, 0x1d, 0x91, 0xc5, 0x6a, 0x37, 0x96, 0x47, 0x61, 0xb2, 0x80, 0x18, 0x32, 0x17];
        let h0 = md5(&[&utf16("password1")]);
        let buf: Vec<u8> = (0..16).flat_map(|_| h0[..5].iter().chain(&salt).copied()).collect();
        let mut base = [0u8; 5];
        base.copy_from_slice(&md5(&[&buf])[..5]);
        let expected = b" \xbf2\xdd\xf5@\x85\x8cQ7D\xaf\x0f$\xe0<";
        assert_eq!(Rc4Key::Standard(base).block_key(0), expected);
    }
}
