//! Smart cards, USB tokens and HSMs through their PKCS #11 module (a
//! .so, .dll or .dylib loaded at run time: OpenSC, p11-kit's proxy, a
//! vendor's own): lists the certificates that have a private key, logs in
//! with the PIN and signs digests with C_Sign — RSA PKCS #1 v1.5 over the
//! DigestInfo, or ECDSA. Keys that ask for the PIN again for every
//! signature (CKA_ALWAYS_AUTHENTICATE, as some national identity cards
//! do) get it right after C_SignInit.

use apex_fs::PathExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use cryptoki::context::{CInitializeArgs, CInitializeFlags, Pkcs11};
use cryptoki::error::{Error as CkError, RvError};
use cryptoki::mechanism::Mechanism;
use cryptoki::object::{Attribute, AttributeType, CertificateType, ObjectClass, ObjectHandle};
use cryptoki::session::{Session, UserType};
use cryptoki::slot::Slot;
use cryptoki::types::AuthPin;

use crate::Error;
use crate::crypto::Hash;
use crate::signer::{self, KeyKind, Signer};
use crate::store::{KeyCert, KeyError, Loc};
use crate::x509::Cert;

/// Modules usually installed, per system, to suggest in the options.
pub fn suggestions() -> Vec<PathBuf> {
    let candidates: &[&str] = if cfg!(windows) {
        &["C:\\Windows\\System32\\opensc-pkcs11.dll", "C:\\Program Files\\OpenSC Project\\OpenSC\\pkcs11\\opensc-pkcs11.dll", "C:\\Program Files\\Yubico\\Yubico PIV Tool\\bin\\libykcs11.dll", "C:\\Windows\\System32\\eTPKCS11.dll", "C:\\Windows\\System32\\UsrPkcs11.dll"]
    } else if cfg!(target_os = "macos") {
        &["/Library/OpenSC/lib/opensc-pkcs11.so", "/opt/homebrew/lib/opensc-pkcs11.so", "/usr/local/lib/opensc-pkcs11.so", "/opt/homebrew/lib/p11-kit-proxy.so", "/usr/local/lib/libykcs11.dylib", "/opt/homebrew/lib/libykcs11.dylib", "/usr/local/lib/libeTPkcs11.dylib"]
    } else {
        &[
            "/usr/lib/x86_64-linux-gnu/p11-kit-proxy.so",
            "/usr/lib64/p11-kit-proxy.so",
            "/usr/lib/p11-kit-proxy.so",
            "/usr/lib/x86_64-linux-gnu/opensc-pkcs11.so",
            "/usr/lib64/opensc-pkcs11.so",
            "/usr/lib/opensc-pkcs11.so",
            "/usr/lib/x86_64-linux-gnu/libykcs11.so",
            "/usr/lib64/libykcs11.so",
            "/usr/lib/libeTPkcs11.so",
            "/usr/lib/softhsm/libsofthsm2.so",
            "/usr/lib/x86_64-linux-gnu/softhsm/libsofthsm2.so",
        ]
    };
    candidates.iter().map(PathBuf::from).filter(|p| p.fs_is_file()).collect()
}

/// The modules loaded and initialized, kept for the whole run: most
/// modules don't take a second C_Initialize, and cards keep their login
/// per session anyway. With their parameters (NSS's database).
static MODULES: Mutex<Vec<(PathBuf, Option<String>, Pkcs11)>> = Mutex::new(Vec::new());

fn err(e: CkError) -> Error {
    Error::new(format!("PKCS #11: {e}"))
}

/// The module at `path`, loaded and initialized once (again when NSS is
/// asked for another database: it holds one at a time).
fn module(path: &Path, params: Option<&str>) -> Result<Pkcs11, Error> {
    let mut mods = MODULES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = mods.iter().position(|(p, _, _)| p == path) {
        if mods[i].1.as_deref() == params {
            return Ok(mods[i].2.clone());
        }
        let (_, _, old) = mods.remove(i);
        let _ = old.finalize();
    }
    let ctx = Pkcs11::new(path).map_err(|e| Error::new(format!("{}: {e}", path.display())))?;
    let flags = CInitializeFlags::OS_LOCKING_OK;
    let res = match params {
        // NSS reads its parameters from the reserved pointer, a C string.
        Some(p) => {
            let c = std::ffi::CString::new(p).map_err(|_| Error::new("bad module parameters"))?;
            let ptr = std::ptr::NonNull::new(c.as_ptr() as *mut std::ffi::c_void).expect("CString isn't null");
            // SAFETY: the string outlives the call, which copies what it needs.
            ctx.initialize(unsafe { CInitializeArgs::new_with_reserved(flags, ptr) })
        }
        None => ctx.initialize(CInitializeArgs::new(flags)),
    };
    match res {
        Ok(()) | Err(CkError::Pkcs11(RvError::CryptokiAlreadyInitialized, _)) => {}
        Err(e) => return Err(err(e)),
    }
    mods.push((path.to_path_buf(), params.map(str::to_string), ctx.clone()));
    Ok(ctx)
}

/// The certificates with a private key on the tokens of the module at
/// `path`.
pub fn list(path: &Path) -> Result<Vec<KeyCert>, Error> {
    list_with(path, None)
}

/// [`list`] with the module's initialization parameters (NSS's database).
pub fn list_with(path: &Path, params: Option<&str>) -> Result<Vec<KeyCert>, Error> {
    let ctx = module(path, params)?;
    let mut out = Vec::new();
    for slot in ctx.get_slots_with_token().map_err(err)? {
        let Ok(info) = ctx.get_token_info(slot) else { continue };
        let Ok(session) = ctx.open_ro_session(slot) else { continue };
        let certs = certificates(&session);
        let keys: Vec<Vec<u8>> = find(&session, &[Attribute::Class(ObjectClass::PRIVATE_KEY)]).iter().map(|&k| attr_id(&session, k)).collect();
        let needs_pin = info.login_required() && !info.protected_authentication_path();
        for (cert, id, label) in certs {
            // Keys are often hidden until one logs in: then any certificate
            // that isn't a CA's may have one.
            let has_key = keys.iter().any(|k| *k == id) || (keys.is_empty() && info.login_required() && !cert.ca);
            if has_key {
                let loc = Loc::Pkcs11 { module: path.to_path_buf(), params: params.map(str::to_string), slot: slot.id(), id };
                out.push(KeyCert { cert, place: info.label().trim().to_string(), label, needs_pin, loc });
            }
        }
    }
    Ok(out)
}

fn find(s: &Session, template: &[Attribute]) -> Vec<ObjectHandle> {
    s.find_objects(template).unwrap_or_default()
}

fn attr_id(s: &Session, h: ObjectHandle) -> Vec<u8> {
    let attrs = s.get_attributes(h, &[AttributeType::Id]).unwrap_or_default();
    attrs.into_iter().find_map(|a| if let Attribute::Id(v) = a { Some(v) } else { None }).unwrap_or_default()
}

/// The token's X.509 certificates: certificate, CKA_ID and label.
fn certificates(s: &Session) -> Vec<(Cert, Vec<u8>, String)> {
    let mut out = Vec::new();
    for h in find(s, &[Attribute::Class(ObjectClass::CERTIFICATE), Attribute::CertificateType(CertificateType::X_509)]) {
        let Ok(attrs) = s.get_attributes(h, &[AttributeType::Value, AttributeType::Id, AttributeType::Label]) else { continue };
        let (mut der, mut id, mut label) = (None, Vec::new(), String::new());
        for a in attrs {
            match a {
                Attribute::Value(v) => der = Some(v),
                Attribute::Id(v) => id = v,
                Attribute::Label(v) => label = String::from_utf8_lossy(&v).trim_end_matches('\0').to_string(),
                _ => {}
            }
        }
        if let Some(cert) = der.and_then(|d| Cert::from_der(&d).ok()) {
            out.push((cert, id, label));
        }
    }
    out
}

fn key_error(e: CkError) -> KeyError {
    match e {
        CkError::Pkcs11(RvError::PinIncorrect | RvError::PinInvalid | RvError::PinLenRange, _) => KeyError::WrongPin,
        CkError::Pkcs11(RvError::PinLocked, _) => KeyError::PinLocked,
        CkError::Pkcs11(RvError::UserNotLoggedIn, _) => KeyError::NeedPin,
        CkError::Pkcs11(RvError::TokenNotPresent | RvError::DeviceRemoved | RvError::SlotIdInvalid, _) => KeyError::NotFound,
        e => KeyError::Other(err(e).to_string()),
    }
}

/// A key on a token, with its session open (and logged in).
struct TokenSigner {
    session: Session,
    key: ObjectHandle,
    cert: Cert,
    chain: Vec<Cert>,
    kind: KeyKind,
    /// For keys that want it before every signature; dropped (and wiped)
    /// with the signer.
    pin: Option<AuthPin>,
    always_auth: bool,
    module: PathBuf,
}

/// A signer with the key `id` in `slot`, for `cert`.
pub(crate) fn signer(cert: &Cert, path: &Path, params: Option<&str>, slot: u64, id: &[u8], pin: Option<&str>) -> Result<Box<dyn Signer>, KeyError> {
    let ctx = module(path, params)?;
    let slot = Slot::try_from(slot).map_err(|e| KeyError::Other(e.to_string()))?;
    let info = ctx.get_token_info(slot).map_err(key_error)?;
    let session = ctx.open_ro_session(slot).map_err(key_error)?;
    let pin = pin.map(|p| AuthPin::from(p.to_string()));
    if info.login_required() {
        let pin_pad = info.protected_authentication_path();
        if pin.is_none() && !pin_pad {
            return Err(KeyError::NeedPin);
        }
        match session.login(UserType::User, if pin_pad { None } else { pin.as_ref() }) {
            Ok(()) | Err(CkError::Pkcs11(RvError::UserAlreadyLoggedIn, _)) => {}
            Err(e) => return Err(key_error(e)),
        }
    }
    let mut template = vec![Attribute::Class(ObjectClass::PRIVATE_KEY)];
    if !id.is_empty() {
        template.push(Attribute::Id(id.to_vec()));
    }
    let keys = session.find_objects(&template).map_err(key_error)?;
    let &[key] = keys.as_slice() else { return Err(KeyError::NotFound) };
    let always_auth = session.get_attributes(key, &[AttributeType::AlwaysAuthenticate]).unwrap_or_default().into_iter().any(|a| matches!(a, Attribute::AlwaysAuthenticate(true)));
    let pool: Vec<Cert> = certificates(&session).into_iter().map(|(c, _, _)| c).collect();
    let chain = signer::chain_from(cert, &pool);
    let kind = KeyKind::of(cert)?;
    Ok(Box::new(TokenSigner { session, key, cert: cert.clone(), chain, kind, pin, always_auth, module: path.to_path_buf() }))
}

impl Signer for TokenSigner {
    fn certificate(&self) -> &Cert {
        &self.cert
    }

    fn chain(&self) -> &[Cert] {
        &self.chain
    }

    fn sign(&self, hash: Hash, digest: &[u8]) -> Result<Vec<u8>, Error> {
        let (mech, data) = match self.kind {
            KeyKind::Rsa(_) => (Mechanism::RsaPkcs, signer::digest_info(hash, digest)),
            _ => (Mechanism::Ecdsa, digest.to_vec()),
        };
        let sig = if self.always_auth {
            self.session.sign_init(&mech, self.key).map_err(err)?;
            self.session.login(UserType::ContextSpecific, self.pin.as_ref()).map_err(|e| Error::new(key_error(e).to_string()))?;
            raw_sign(&self.module, &self.session, &data)?
        } else {
            self.session.sign(&mech, self.key, &data).map_err(err)?
        };
        match self.kind {
            KeyKind::Rsa(_) => Ok(sig),
            _ => signer::ecdsa_der(&sig),
        }
    }
}

/// C_Sign on a signing already started: cryptoki only has it together
/// with C_SignInit, and the context-specific login goes in between.
fn raw_sign(module: &Path, session: &Session, data: &[u8]) -> Result<Vec<u8>, Error> {
    use cryptoki_sys::{CK_FUNCTION_LIST, CK_ULONG, CKR_OK};
    let fail = || Error::new("PKCS #11: C_Sign failed");
    // SAFETY: the module is already loaded (this only takes another
    // reference to it), its function list is what the standard says, and
    // the buffers are as long as their lengths say.
    unsafe {
        let lib = cryptoki_sys::Pkcs11::new(module).map_err(|e| Error::new(e.to_string()))?;
        if lib.C_GetFunctionList.is_err() {
            return Err(fail());
        }
        let mut list: *mut CK_FUNCTION_LIST = std::ptr::null_mut();
        if lib.C_GetFunctionList(&mut list) != CKR_OK || list.is_null() {
            return Err(fail());
        }
        let Some(c_sign) = std::ptr::read_unaligned(list).C_Sign else { return Err(fail()) };
        let mut out = vec![0u8; 1024];
        let mut len = out.len() as CK_ULONG;
        let rv = c_sign(session.handle(), data.as_ptr() as *mut u8, data.len() as CK_ULONG, out.as_mut_ptr(), &mut len);
        if rv != CKR_OK {
            return Err(Error::new(format!("PKCS #11: C_Sign failed ({rv:#x})")));
        }
        out.truncate(len as usize);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn missing_module() {
        assert!(super::list(std::path::Path::new("/nonexistent/pkcs11.so")).is_err());
    }
}
