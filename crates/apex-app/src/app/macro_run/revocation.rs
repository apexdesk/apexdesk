//! Whether the certificate that signed a file's macros was revoked. Asked
//! only when macros signed by an authority are about to be enabled (not on
//! opening, not for self-signed certificates or the publishers trusted by
//! name): the authority's OCSP responder, else its CRL, for 5 seconds at
//! most. The answer is kept for a day. The authority sees this computer's
//! address and which certificate was asked about, not the file.
//!
//! A revoked certificate never runs. With no answer the macros run with a
//! notice, unless the `MacroRevocation` policy says `require` (or `off`:
//! networks with no way out).

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use web_time::{Duration, Instant};

use apex_sign::revocation::{self, CertStatus};
use apex_sign::x509::Cert;

/// The longest the question may hold up enabling the macros.
const WAIT: Duration = Duration::from_secs(5);
/// How long an answer is kept.
const KEEP: Duration = Duration::from_secs(24 * 3600);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Good,
    Revoked,
    NoAnswer,
}

/// Answers by certificate (SHA-256), and when they came.
static ANSWERS: Mutex<Option<HashMap<String, (Verdict, Instant)>>> = Mutex::new(None);
/// Bumped when a certificate is found revoked (checks done before must be
/// done again).
static REVOKED: AtomicU64 = AtomicU64::new(0);

pub(crate) fn generation() -> u64 {
    REVOKED.load(Ordering::Relaxed)
}

/// Whether `cert` is known to be revoked (from an answer of today).
pub(crate) fn known_revoked(cert: &Cert) -> bool {
    cached(&cert.fingerprint()) == Some(Verdict::Revoked)
}

fn cached(fp: &str) -> Option<Verdict> {
    let a = ANSWERS.lock().unwrap_or_else(|e| e.into_inner());
    a.as_ref()?.get(fp).filter(|(_, at)| at.elapsed() < KEEP).map(|(v, _)| *v)
}

fn remember(fp: String, v: Verdict) {
    let mut a = ANSWERS.lock().unwrap_or_else(|e| e.into_inner());
    a.get_or_insert_with(HashMap::new).insert(fp, (v, Instant::now()));
    if v == Verdict::Revoked {
        REVOKED.fetch_add(1, Ordering::Relaxed);
    }
}

/// What the authority says of the certificate at the head of `chain`
/// (with its issuer next), asked now or earlier today.
pub(crate) fn verdict(chain: &[Cert]) -> Verdict {
    let (Some(cert), Some(issuer)) = (chain.first(), chain.get(1)) else { return Verdict::NoAnswer };
    let fp = cert.fingerprint();
    if let Some(v) = cached(&fp) {
        return v;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let (cert, issuer, pool) = (cert.clone(), issuer.clone(), chain.to_vec());
    // The request goes on in the background if it takes longer.
    let spawned = apex_fs::thread::Builder::new().name("macro-revocation".into()).spawn(move || {
        let _ = tx.send(revocation::fetch(&apex_sign::net::Http, &cert, &issuer, &pool));
    });
    let v = match spawned.ok().and_then(|_| rx.recv_timeout(WAIT).ok()) {
        Some(Ok((_, _, e))) => match e.status {
            CertStatus::Good => Verdict::Good,
            CertStatus::Revoked(_) => Verdict::Revoked,
            CertStatus::Unknown => Verdict::NoAnswer,
        },
        _ => Verdict::NoAnswer,
    };
    // No answer isn't kept: the next try may get one.
    if v != Verdict::NoAnswer {
        remember(fp, v);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_are_kept() {
        let c = Cert::from_der(include_bytes!("../../../../apex-sign/tests/data/ca.der")).unwrap();
        assert!(!known_revoked(&c));
        let g = generation();
        remember(c.fingerprint(), Verdict::Revoked);
        assert!(known_revoked(&c));
        assert_eq!(verdict(&[c.clone(), c.clone()]), Verdict::Revoked);
        assert!(generation() > g);
        // A certificate alone (self-signed) has no one to ask.
        assert_eq!(verdict(&[c]), Verdict::NoAnswer);
    }
}
