//! The macOS keychains: identities (a certificate with its private key)
//! of the user's keychains and of smart cards (CryptoTokenKit), signing
//! with SecKeyCreateSignature. macOS itself asks for the keychain's
//! consent or the card's PIN.

use security_framework::certificate::SecCertificate;
use security_framework::identity::SecIdentity;
use security_framework::item::{ItemClass, ItemSearchOptions, Limit, Reference, SearchResult};
use security_framework::key::{Algorithm, SecKey};

use crate::Error;
use crate::crypto::Hash;
use crate::signer::{self, KeyKind, Signer};
use crate::store::{KeyCert, KeyError, Loc};
use crate::x509::Cert;

/// The identities of the keychains and of the cards (tokens).
fn identities() -> Vec<(Cert, SecIdentity)> {
    let mut out: Vec<(Cert, SecIdentity)> = Vec::new();
    for token in [false, true] {
        let mut opts = ItemSearchOptions::new();
        opts.class(ItemClass::identity()).load_refs(true).limit(Limit::All);
        if token {
            opts.access_group_token();
        }
        for r in opts.search().unwrap_or_default() {
            if let SearchResult::Ref(Reference::Identity(id)) = r {
                let Ok(c) = id.certificate() else { continue };
                if let Ok(cert) = Cert::from_der(&c.to_der())
                    && !out.iter().any(|(o, _)| o.der == cert.der)
                {
                    out.push((cert, id));
                }
            }
        }
    }
    out
}

/// Every certificate of the keychains, for the chains.
fn all_certs() -> Vec<Cert> {
    let mut opts = ItemSearchOptions::new();
    opts.class(ItemClass::certificate()).load_refs(true).limit(Limit::All);
    let found = opts.search().unwrap_or_default();
    found.into_iter().filter_map(|r| if let SearchResult::Ref(Reference::Certificate(c)) = r { Some(c) } else { None }).filter_map(|c: SecCertificate| Cert::from_der(&c.to_der()).ok()).collect()
}

pub(super) fn list() -> Result<Vec<KeyCert>, Error> {
    Ok(identities().into_iter().map(|(cert, _)| KeyCert { cert, place: "Keychain".into(), label: String::new(), needs_pin: false, loc: Loc::System }).collect())
}

struct KeychainSigner {
    key: SecKey,
    cert: Cert,
    chain: Vec<Cert>,
    kind: KeyKind,
}

pub(super) fn signer(cert: &Cert) -> Result<Box<dyn Signer>, KeyError> {
    let (_, id) = identities().into_iter().find(|(c, _)| c.der == cert.der).ok_or(KeyError::NotFound)?;
    let key = id.private_key().map_err(|e| KeyError::Other(e.to_string()))?;
    let chain = signer::chain_from(cert, &all_certs());
    let kind = KeyKind::of(cert)?;
    Ok(Box::new(KeychainSigner { key, cert: cert.clone(), chain, kind }))
}

impl Signer for KeychainSigner {
    fn certificate(&self) -> &Cert {
        &self.cert
    }

    fn chain(&self) -> &[Cert] {
        &self.chain
    }

    fn sign(&self, hash: Hash, digest: &[u8]) -> Result<Vec<u8>, Error> {
        let rsa = matches!(self.kind, KeyKind::Rsa(_));
        let alg = match (hash, rsa) {
            (Hash::Sha1, true) => Algorithm::RSASignatureDigestPKCS1v15SHA1,
            (Hash::Sha224, true) => Algorithm::RSASignatureDigestPKCS1v15SHA224,
            (Hash::Sha256, true) => Algorithm::RSASignatureDigestPKCS1v15SHA256,
            (Hash::Sha384, true) => Algorithm::RSASignatureDigestPKCS1v15SHA384,
            (Hash::Sha512, true) => Algorithm::RSASignatureDigestPKCS1v15SHA512,
            (Hash::Sha1, false) => Algorithm::ECDSASignatureDigestX962SHA1,
            (Hash::Sha224, false) => Algorithm::ECDSASignatureDigestX962SHA224,
            (Hash::Sha256, false) => Algorithm::ECDSASignatureDigestX962SHA256,
            (Hash::Sha384, false) => Algorithm::ECDSASignatureDigestX962SHA384,
            (Hash::Sha512, false) => Algorithm::ECDSASignatureDigestX962SHA512,
        };
        // ECDSA comes in DER (X9.62), as the signer gives it.
        self.key.create_signature(alg, digest).map_err(|e| Error::new(format!("the keychain couldn't sign: {e}")))
    }
}
