//! The Windows certificate store ("MY", the current user's personal
//! certificates): the key is opened with CryptAcquireCertificatePrivateKey
//! as a CNG key and signs with NCryptSignHash. Windows itself asks for a
//! smart card's PIN or a protected key's consent.

use windows_sys::Win32::Security::Cryptography::*;
use windows_sys::core::BOOL;

use crate::Error;
use crate::crypto::Hash;
use crate::signer::{self, KeyKind, Signer};
use crate::store::{KeyCert, KeyError, Loc};
use crate::x509::Cert;

const ENCODING: u32 = X509_ASN_ENCODING | PKCS_7_ASN_ENCODING;

/// A system store opened for reading, closed on drop.
struct Store(HCERTSTORE);

impl Store {
    fn open(name: &str) -> Option<Store> {
        let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
        // SAFETY: a valid, NUL-terminated name.
        let h = unsafe { CertOpenSystemStoreW(0, wide.as_ptr()) };
        (!h.is_null()).then_some(Store(h))
    }

    /// Its certificates, each with whether it has a private key.
    fn certs(&self) -> Vec<(Cert, bool)> {
        let mut out = Vec::new();
        let mut ctx: *mut CERT_CONTEXT = std::ptr::null_mut();
        loop {
            // SAFETY: the previous context is handed back to be freed.
            ctx = unsafe { CertEnumCertificatesInStore(self.0, ctx) };
            if ctx.is_null() {
                break;
            }
            // SAFETY: a context the store just gave.
            let (der, has_key) = unsafe {
                let c = &*ctx;
                let der = std::slice::from_raw_parts(c.pbCertEncoded, c.cbCertEncoded as usize).to_vec();
                let mut size = 0u32;
                (der, CertGetCertificateContextProperty(ctx, CERT_KEY_PROV_INFO_PROP_ID, std::ptr::null_mut(), &mut size) != 0)
            };
            if let Ok(cert) = Cert::from_der(&der) {
                out.push((cert, has_key));
            }
        }
        out
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        // SAFETY: a store this opened.
        unsafe { CertCloseStore(self.0, 0) };
    }
}

pub(super) fn list() -> Result<Vec<KeyCert>, Error> {
    let store = Store::open("MY").ok_or_else(|| Error::new("the personal certificate store can't be opened"))?;
    Ok(store.certs().into_iter().filter(|(_, k)| *k).map(|(cert, _)| KeyCert { cert, place: "Windows".into(), label: String::new(), needs_pin: false, loc: Loc::System }).collect())
}

/// A CNG key, freed on drop.
struct CngSigner {
    key: NCRYPT_KEY_HANDLE,
    free: bool,
    cert: Cert,
    chain: Vec<Cert>,
    kind: KeyKind,
}

pub(super) fn signer(cert: &Cert) -> Result<Box<dyn Signer>, KeyError> {
    let store = Store::open("MY").ok_or(KeyError::NotFound)?;
    let sha1 = Hash::Sha1.digest(&cert.der);
    let blob = CRYPT_INTEGER_BLOB { cbData: sha1.len() as u32, pbData: sha1.as_ptr() as *mut u8 };
    // SAFETY: the blob points at the digest, alive for the call.
    let ctx = unsafe { CertFindCertificateInStore(store.0, ENCODING, 0, CERT_FIND_SHA1_HASH, &blob as *const _ as *const core::ffi::c_void, std::ptr::null()) };
    if ctx.is_null() {
        return Err(KeyError::NotFound);
    }
    let mut key: HCRYPTPROV_OR_NCRYPT_KEY_HANDLE = 0;
    let mut spec: CERT_KEY_SPEC = 0;
    let mut free: BOOL = 0;
    // SAFETY: a context found above, freed right after.
    let ok = unsafe {
        let ok = CryptAcquireCertificatePrivateKey(ctx, CRYPT_ACQUIRE_ONLY_NCRYPT_KEY_FLAG | CRYPT_ACQUIRE_COMPARE_KEY_FLAG, std::ptr::null(), &mut key, &mut spec, &mut free);
        CertFreeCertificateContext(ctx);
        ok != 0
    };
    if !ok || spec != CERT_NCRYPT_KEY_SPEC {
        return Err(KeyError::Other("the certificate's private key can't be used".into()));
    }
    // The issuers, from the personal and the intermediate stores.
    let mut pool: Vec<Cert> = store.certs().into_iter().map(|(c, _)| c).collect();
    for name in ["CA", "ROOT"] {
        if let Some(s) = Store::open(name) {
            pool.extend(s.certs().into_iter().map(|(c, _)| c));
        }
    }
    let chain = signer::chain_from(cert, &pool);
    let kind = KeyKind::of(cert)?;
    Ok(Box::new(CngSigner { key, free: free != 0, cert: cert.clone(), chain, kind }))
}

impl Drop for CngSigner {
    fn drop(&mut self) {
        if self.free {
            // SAFETY: a key handle this acquired.
            unsafe { NCryptFreeObject(self.key) };
        }
    }
}

impl Signer for CngSigner {
    fn certificate(&self) -> &Cert {
        &self.cert
    }

    fn chain(&self) -> &[Cert] {
        &self.chain
    }

    fn sign(&self, hash: Hash, digest: &[u8]) -> Result<Vec<u8>, Error> {
        let alg = match hash {
            Hash::Sha1 => BCRYPT_SHA1_ALGORITHM,
            Hash::Sha256 => BCRYPT_SHA256_ALGORITHM,
            Hash::Sha384 => BCRYPT_SHA384_ALGORITHM,
            Hash::Sha512 => BCRYPT_SHA512_ALGORITHM,
            Hash::Sha224 => return Err(Error::new("SHA-224 isn't supported by Windows keys")),
        };
        let padding = BCRYPT_PKCS1_PADDING_INFO { pszAlgId: alg };
        let (info, flags) = match self.kind {
            KeyKind::Rsa(_) => (&padding as *const _ as *const core::ffi::c_void, BCRYPT_PAD_PKCS1),
            _ => (std::ptr::null(), 0),
        };
        let mut out = vec![0u8; 1024];
        let mut len = 0u32;
        // SAFETY: the buffers are as long as said; the padding outlives
        // the call.
        let hr = unsafe { NCryptSignHash(self.key, info, digest.as_ptr(), digest.len() as u32, out.as_mut_ptr(), out.len() as u32, &mut len, flags) };
        if hr != 0 {
            return Err(Error::new(format!("Windows couldn't sign ({:#010x})", hr as u32)));
        }
        out.truncate(len as usize);
        match self.kind {
            KeyKind::Rsa(_) => Ok(out),
            // CNG gives r and s side by side.
            _ => signer::ecdsa_der(&out),
        }
    }
}
