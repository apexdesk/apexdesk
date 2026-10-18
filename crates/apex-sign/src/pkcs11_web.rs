//! The browser can't load a PKCS #11 module: no cards or tokens there.

use std::path::{Path, PathBuf};

use crate::Error;
use crate::signer::Signer;
use crate::store::{KeyCert, KeyError};
use crate::x509::Cert;

fn none() -> Error {
    Error::new("Smart cards and tokens aren't available in the browser")
}

pub fn suggestions() -> Vec<PathBuf> {
    Vec::new()
}

pub fn list(path: &Path) -> Result<Vec<KeyCert>, Error> {
    list_with(path, None)
}

pub fn list_with(_path: &Path, _params: Option<&str>) -> Result<Vec<KeyCert>, Error> {
    Err(none())
}

pub(crate) fn signer(_cert: &Cert, _path: &Path, _params: Option<&str>, _slot: u64, _id: &[u8], _pin: Option<&str>) -> Result<Box<dyn Signer>, KeyError> {
    Err(none().into())
}
