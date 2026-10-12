//! The system's certificates with their private keys: the personal store
//! of Windows (CryptoAPI/CNG), the macOS keychains, and on Linux, which
//! has no system store, the NSS databases that Firefox, Thunderbird and
//! Chromium keep (through NSS's own PKCS #11 module). The keys stay where
//! they are: the store (or the card behind it) signs.

use std::path::PathBuf;

use crate::Error;
use crate::signer::Signer;
use crate::x509::Cert;

#[cfg(windows)]
mod windows;
#[cfg(target_os = "macos")]
mod macos;

/// Why a key couldn't be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyError {
    /// It needs the PIN (or password) and none was given.
    NeedPin,
    WrongPin,
    PinLocked,
    /// The certificate or its key isn't there any more (a card taken out).
    NotFound,
    Other(String),
}

impl std::fmt::Display for KeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            KeyError::NeedPin => write!(f, "the PIN is needed"),
            KeyError::WrongPin => write!(f, "wrong PIN"),
            KeyError::PinLocked => write!(f, "the PIN is locked"),
            KeyError::NotFound => write!(f, "the certificate or its key was not found"),
            KeyError::Other(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for KeyError {}

impl From<Error> for KeyError {
    fn from(e: Error) -> Self {
        KeyError::Other(e.to_string())
    }
}

/// A certificate whose private key is in a store or on a token.
#[derive(Clone, Debug)]
pub struct KeyCert {
    pub cert: Cert,
    /// Where it is: the token's or the store's name.
    pub place: String,
    /// The certificate's label on a token (else empty).
    pub label: String,
    /// The PIN is typed in ApexDesk (the system asks for it itself
    /// otherwise, or a PIN pad does).
    pub needs_pin: bool,
    pub(crate) loc: Loc,
}

#[derive(Clone, Debug)]
pub(crate) enum Loc {
    Pkcs11 { module: PathBuf, params: Option<String>, slot: u64, id: Vec<u8> },
    #[cfg(any(windows, target_os = "macos"))]
    System,
}

impl KeyCert {
    /// A signer with this key, logging in with `pin` if it needs one (the
    /// PIN is dropped with the signer, never kept).
    pub fn signer(&self, pin: Option<&str>) -> Result<Box<dyn Signer>, KeyError> {
        match &self.loc {
            Loc::Pkcs11 { module, params, slot, id } => crate::pkcs11::signer(&self.cert, module, params.as_deref(), *slot, id, pin),
            #[cfg(windows)]
            Loc::System => windows::signer(&self.cert),
            #[cfg(target_os = "macos")]
            Loc::System => macos::signer(&self.cert),
        }
    }

    /// The PKCS #11 slot it's in.
    pub fn slot(&self) -> Option<u64> {
        match &self.loc {
            Loc::Pkcs11 { slot, .. } => Some(*slot),
            #[cfg(any(windows, target_os = "macos"))]
            Loc::System => None,
        }
    }
}

/// The certificates of the system's store that have a private key.
pub fn list() -> Result<Vec<KeyCert>, Error> {
    #[cfg(windows)]
    return windows::list();
    #[cfg(target_os = "macos")]
    return macos::list();
    #[cfg(not(any(windows, target_os = "macos")))]
    return nss_list();
}

/// What the system's store is, for messages: on Linux, the NSS databases.
pub fn describe() -> &'static str {
    if cfg!(windows) {
        "Windows certificate store"
    } else if cfg!(target_os = "macos") {
        "Keychain"
    } else {
        "NSS databases (Firefox, Thunderbird, Chromium)"
    }
}

/// NSS's PKCS #11 module (softokn), as the system installs it.
#[cfg(not(any(windows, target_os = "macos")))]
fn softokn() -> Option<PathBuf> {
    let dirs = ["/usr/lib/x86_64-linux-gnu", "/usr/lib/aarch64-linux-gnu", "/usr/lib64", "/usr/lib", "/usr/lib/x86_64-linux-gnu/nss", "/usr/lib64/nss", "/usr/lib/nss", "/usr/local/lib"];
    dirs.iter().map(|d| PathBuf::from(d).join("libsoftokn3.so")).find(|p| p.is_file())
}

/// The user's NSS databases (sql format): Chromium's shared one, and each
/// Firefox and Thunderbird profile (native, snap and flatpak).
#[cfg(not(any(windows, target_os = "macos")))]
pub fn nss_databases() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return Vec::new() };
    let mut out = vec![home.join(".pki/nssdb")];
    for base in [".mozilla/firefox", "snap/firefox/common/.mozilla/firefox", ".var/app/org.mozilla.firefox/.mozilla/firefox", ".thunderbird", "snap/thunderbird/common/.thunderbird", ".var/app/org.mozilla.Thunderbird/.thunderbird"] {
        if let Ok(rd) = std::fs::read_dir(home.join(base)) {
            let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
            v.sort();
            out.extend(v);
        }
    }
    out.retain(|d| d.join("cert9.db").is_file());
    out
}

/// The softokn parameters that open the database in `dir`, read only.
#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) fn nss_params(dir: &std::path::Path, read_only: bool) -> String {
    let dir = dir.to_string_lossy().replace('\'', "\\'");
    format!("configdir='sql:{dir}' certPrefix='' keyPrefix='' secmod='secmod.db' flags={}", if read_only { "readOnly" } else { "''" })
}

#[cfg(not(any(windows, target_os = "macos")))]
fn nss_list() -> Result<Vec<KeyCert>, Error> {
    let dbs = nss_databases();
    if dbs.is_empty() {
        return Ok(Vec::new());
    }
    let Some(lib) = softokn() else { return Err(Error::new("NSS (libsoftokn3) is not installed")) };
    let mut out: Vec<KeyCert> = Vec::new();
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    for db in dbs {
        let Ok(found) = crate::pkcs11::list_with(&lib, Some(&nss_params(&db, true))) else { continue };
        let place = db.strip_prefix(&home).map(|p| format!("~/{}", p.display())).unwrap_or_else(|_| db.display().to_string());
        for mut k in found {
            // NSS's internal slot: only the key database's token matters.
            if !out.iter().any(|o| o.cert.der == k.cert.der) {
                k.place = place.clone();
                out.push(k);
            }
        }
    }
    Ok(out)
}
