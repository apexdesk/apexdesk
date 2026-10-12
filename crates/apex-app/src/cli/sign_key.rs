//! The signing key of `apexdesk sign`: a certificate file (`--cert`), the
//! system's store (`--store`) or a card or token (`--pkcs11 MODULE`),
//! with the certificate picked by `--cert-sha1`, `--cert-subject`,
//! `--cert-label` and `--slot`, or the only one fit to sign. Passwords and
//! PINs never go on the command line: they come from a file or an
//! environment variable, or are typed without echo.

use std::path::Path;

use apex_sign::Signer;
use apex_sign::signer::{self, CertInfo};
use apex_sign::store::{KeyCert, KeyError};

use super::args::Parsed;
use super::{Fail, Res, emit, error, secret, usage};

/// The certificate's password: from `--password-file`, `--password-env`,
/// none if the file needs none, else asked for.
fn password(a: &Parsed, cert: &Path, data: &[u8]) -> Res<String> {
    if let Some(f) = a.value("password-file") {
        let text = std::fs::read_to_string(f).map_err(|e| Fail::Error(format!("{f}: {e}")))?;
        return Ok(text.lines().next().unwrap_or("").to_string());
    }
    if let Some(var) = a.value("password-env") {
        return std::env::var(var).map_err(|_| Fail::Error(format!("the environment variable {var} is not set")));
    }
    if apex_sign::p12::read(data, "").is_ok() {
        return Ok(String::new());
    }
    let name = cert.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    secret::read(&format!("Password of {name}: ")).map_err(|e| Fail::Error(format!("could not read the password: {e}")))
}

/// Checks that one source is given (and its options fit it).
pub fn check(a: &Parsed) -> Res<()> {
    let n = [a.value("cert").is_some(), a.flag("store"), a.value("pkcs11").is_some()].iter().filter(|b| **b).count();
    if n == 0 {
        return usage("give --cert, --store or --pkcs11");
    }
    if n > 1 {
        return usage("give only one of --cert, --store and --pkcs11");
    }
    if a.value("password-file").is_some() && a.value("password-env").is_some() {
        return usage("give --password-file or --password-env, not both");
    }
    if a.value("cert").is_some() && ["slot", "cert-label", "cert-subject", "cert-sha1", "pin-env"].iter().any(|o| a.value(o).is_some()) {
        return usage("--slot, --cert-label, --cert-subject, --cert-sha1 and --pin-env go with --store or --pkcs11");
    }
    if a.value("pkcs11").is_none() && (a.value("slot").is_some() || a.value("cert-label").is_some()) {
        return usage("--slot and --cert-label go with --pkcs11");
    }
    if a.value("slot").is_some_and(|s| s.parse::<u64>().is_err()) {
        return usage("--slot takes a number");
    }
    Ok(())
}

/// The certificates of the store or the token given.
fn listing(a: &Parsed) -> Res<Vec<KeyCert>> {
    let res = match a.value("pkcs11") {
        Some(m) => apex_sign::pkcs11::list(Path::new(m)),
        None => apex_sign::store::list(),
    };
    res.map_err(|e| Fail::Error(e.to_string()))
}

fn normalized_hex(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_hexdigit()).collect::<String>().to_ascii_uppercase()
}

/// The certificates matching `--slot`, `--cert-label`, `--cert-subject`
/// and `--cert-sha1`.
fn matching<'a>(a: &Parsed, list: &'a [KeyCert]) -> Vec<&'a KeyCert> {
    let sha1 = a.value("cert-sha1").map(normalized_hex);
    let subject = a.value("cert-subject").map(str::to_lowercase);
    let slot = a.value("slot").and_then(|s| s.parse::<u64>().ok());
    list.iter()
        .filter(|k| sha1.as_ref().is_none_or(|h| k.cert.sha1() == *h))
        .filter(|k| subject.as_ref().is_none_or(|s| k.cert.subject.full().to_lowercase().contains(s.as_str())))
        .filter(|k| a.value("cert-label").is_none_or(|l| k.label == l))
        .filter(|k| slot.is_none_or(|s| k.slot() == Some(s)))
        .collect()
}

/// `--list`: the store's or the token's certificates.
pub fn list(a: &Parsed) -> Res<i32> {
    let list = listing(a)?;
    let now = apex_sign::time::now();
    let mut out = String::new();
    for k in matching(a, &list) {
        let info = CertInfo::of(&k.cert);
        let (y, m, d, ..) = apex_sign::time::civil(info.not_after);
        out.push_str(&format!("{}  {}\n", info.sha1, info.subject));
        let mut facts = vec![format!("issuer: {}", info.issuer), format!("valid until {y:04}-{m:02}-{d:02}")];
        if !info.usage.is_empty() {
            facts.push(format!("usage: {}", info.usage.join(", ")));
        }
        if let Some(s) = k.slot() {
            facts.push(format!("slot {s}"));
        }
        for (name, v) in [("token", &k.place), ("label", &k.label)] {
            if !v.is_empty() {
                facts.push(format!("{name}: {v}"));
            }
        }
        if !signer::can_sign(&k.cert, now) {
            facts.push("not fit for signing".into());
        }
        out.push_str(&format!("    {}\n", facts.join("; ")));
    }
    if out.is_empty() {
        out = format!("no certificates with a private key found ({})\n", if a.flag("store") { apex_sign::store::describe() } else { "PKCS #11" });
    }
    emit(&out);
    Ok(0)
}

/// The signer the options give.
pub fn signer(a: &Parsed) -> Res<Box<dyn Signer>> {
    if let Some(cert) = a.value("cert") {
        let path = Path::new(cert);
        let data = std::fs::read(path).map_err(|e| Fail::Error(format!("{cert}: {e}")))?;
        let pw = password(a, path, &data)?;
        let id = apex_sign::p12::read(&data, &pw).map_err(|e| Fail::Error(format!("{cert}: {e}")))?;
        return Ok(Box::new(id));
    }
    let list = listing(a)?;
    let found = matching(a, &list);
    let now = apex_sign::time::now();
    let fit: Vec<&KeyCert> = found.iter().copied().filter(|k| signer::can_sign(&k.cert, now)).collect();
    // A certificate named by its SHA-1 is used even if it seems unfit.
    let pick = if a.value("cert-sha1").is_some() && found.len() == 1 { &found } else { &fit };
    let k = match pick.as_slice() {
        [k] => *k,
        [] if found.is_empty() => return error("no certificate with a private key matches (see --list)"),
        [] => return error("the matching certificates can't sign (expired, a CA's or another key usage; see --list)"),
        _ => return error("several certificates match: choose one with --cert-sha1 (see --list)"),
    };
    let pin = if k.needs_pin {
        Some(match a.value("pin-env") {
            Some(var) => std::env::var(var).map_err(|_| Fail::Error(format!("the environment variable {var} is not set")))?,
            None => secret::read(&format!("PIN of {}: ", if k.place.is_empty() { "the token" } else { &k.place })).map_err(|e| Fail::Error(format!("could not read the PIN: {e}")))?,
        })
    } else {
        None
    };
    k.signer(pin.as_deref()).map_err(|e| match e {
        KeyError::NotFound => Fail::Error("the certificate or its key is no longer there".into()),
        e => Fail::Error(e.to_string()),
    })
}
