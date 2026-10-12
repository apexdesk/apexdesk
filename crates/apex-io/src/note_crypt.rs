//! A notebook section sealed with a password: a key made from it with
//! PBKDF2-SHA256, the text in AES-256 counter mode and an HMAC-SHA256 tag
//! that tells a wrong password (or a changed file) from the right one.

use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockEncrypt, KeyInit};
use sha2::{Digest, Sha256};

/// What a sealed body starts with.
const MAGIC: &[u8] = b"APXE1";
const ROUNDS: u32 = 210_000;
const NONCE: usize = 16;
const TAG: usize = 32;
/// Sealed to check a password with.
const CHECK: &[u8] = b"apexnote section";

/// The key of a section (kept only in memory).
#[derive(Clone)]
pub struct Key {
    enc: [u8; 32],
    mac: [u8; 32],
}

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Key(…)")
    }
}

fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

fn hmac(key: &[u8; 32], parts: &[&[u8]]) -> [u8; 32] {
    let ipad: Vec<u8> = key.iter().map(|b| b ^ 0x36).chain(std::iter::repeat_n(0x36, 32)).collect();
    let opad: Vec<u8> = key.iter().map(|b| b ^ 0x5c).chain(std::iter::repeat_n(0x5c, 32)).collect();
    let mut inner = vec![&ipad[..]];
    inner.extend_from_slice(parts);
    let inner = sha256(&inner);
    sha256(&[&opad, &inner])
}

/// The key `password` makes with `salt`.
pub fn derive(password: &str, salt: &[u8]) -> Key {
    let mut k = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(password.as_bytes(), salt, ROUNDS, &mut k);
    Key { enc: sha256(&[&k, b"enc"]), mac: sha256(&[&k, b"mac"]) }
}

/// AES-256-CTR: the same both ways.
fn ctr(key: &[u8; 32], nonce: &[u8], data: &mut [u8]) {
    let aes = aes::Aes256::new(GenericArray::from_slice(key));
    let start = u128::from_be_bytes(nonce.try_into().unwrap_or([0; NONCE]));
    for (i, chunk) in data.chunks_mut(16).enumerate() {
        let mut block = GenericArray::from(start.wrapping_add(i as u128).to_be_bytes());
        aes.encrypt_block(&mut block);
        for (b, k) in chunk.iter_mut().zip(block.iter()) {
            *b ^= k;
        }
    }
}

pub fn is_sealed(data: &[u8]) -> bool {
    data.starts_with(MAGIC)
}

pub fn seal(key: &Key, plain: &[u8]) -> Vec<u8> {
    let nonce = crate::crypto::random(NONCE);
    let mut body = plain.to_vec();
    ctr(&key.enc, &nonce, &mut body);
    let mut out = Vec::with_capacity(MAGIC.len() + NONCE + body.len() + TAG);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&body);
    let tag = hmac(&key.mac, &[&out]);
    out.extend_from_slice(&tag);
    out
}

/// What was sealed, or None with the wrong key.
pub fn open(key: &Key, sealed: &[u8]) -> Option<Vec<u8>> {
    if !is_sealed(sealed) || sealed.len() < MAGIC.len() + NONCE + TAG {
        return None;
    }
    let (head, tag) = sealed.split_at(sealed.len() - TAG);
    // Compared without stopping at the first difference.
    if hmac(&key.mac, &[head]).iter().zip(tag).fold(0u8, |acc, (a, b)| acc | (a ^ b)) != 0 {
        return None;
    }
    let (nonce, body) = head[MAGIC.len()..].split_at(NONCE);
    let mut body = body.to_vec();
    ctr(&key.enc, nonce, &mut body);
    Some(body)
}

/// A new lock for `password`: what is kept to check it ("salt:check", in
/// hex), and its key.
pub fn new_lock(password: &str) -> (String, Key) {
    let salt = crate::crypto::random(16);
    let key = derive(password, &salt);
    (format!("{}:{}", hex(&salt), hex(&seal(&key, CHECK))), key)
}

/// The key of `password` when it is the lock's.
pub fn try_lock(lock: &str, password: &str) -> Option<Key> {
    let (salt, check) = lock.split_once(':')?;
    let key = derive(password, &unhex(salt)?);
    (open(&key, &unhex(check)?)? == CHECK).then_some(key)
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub(crate) fn unhex(s: &str) -> Option<Vec<u8>> {
    (0..s.len()).step_by(2).map(|i| s.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_sealed_opens_with_its_password_only() {
        let (lock, key) = new_lock("faro");
        assert!(try_lock(&lock, "Faro").is_none());
        let again = try_lock(&lock, "faro").expect("right password");
        let text = "El faro lleva diez años apagado. ".repeat(5);
        let s = seal(&key, text.as_bytes());
        assert!(is_sealed(&s) && !s.windows(4).any(|w| w == b"faro"));
        assert_eq!(open(&again, &s).as_deref(), Some(text.as_bytes()));
        assert_ne!(seal(&key, text.as_bytes()), s, "a new nonce each time");
        let mut bad = s.clone();
        bad[MAGIC.len() + NONCE + 3] ^= 1;
        assert!(open(&key, &bad).is_none(), "a changed byte is caught");
        assert!(open(&derive("otra", b"x"), &s).is_none());
        assert_eq!(open(&key, &seal(&key, b"")).as_deref(), Some(&b""[..]));
    }
}
