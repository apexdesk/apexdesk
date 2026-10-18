//! Long-term validation: the proof, kept in the document, that the
//! certificates were valid when it was signed (the chains, OCSP answers
//! or CRLs), gathered when signing or later, and read back to tell how
//! long a signature stays verifiable.

use crate::Error;
use crate::net::Net;
use crate::revocation::{self, CertStatus, Evidence};
use crate::x509::Cert;

/// How much a signature carries to be verified later (PAdES baseline
/// names; XAdES-BES, -T, -XL and -A in packages).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// The signature alone.
    #[default]
    B,
    /// With a time-stamp proving when.
    T,
    /// With the chain and the revocation answers too.
    Lt,
    /// And an archive time-stamp over all of it (renewed before it
    /// expires).
    Lta,
}

impl Level {
    pub fn pades(self) -> &'static str {
        match self {
            Level::B => "B-B",
            Level::T => "B-T",
            Level::Lt => "B-LT",
            Level::Lta => "B-LTA",
        }
    }

    pub fn xades(self) -> &'static str {
        match self {
            Level::B => "XAdES-BES",
            Level::T => "XAdES-T",
            Level::Lt => "XAdES-XL",
            Level::Lta => "XAdES-A",
        }
    }

    /// From its command line name: b, t, lt, lta (and the XAdES ones).
    pub fn parse(s: &str) -> Option<Level> {
        Some(match s.trim().to_ascii_lowercase().trim_start_matches("b-").trim_start_matches("xades-") {
            "b" | "bes" => Level::B,
            "t" => Level::T,
            "lt" | "xl" | "x-l" => Level::Lt,
            "lta" | "a" => Level::Lta,
            _ => return None,
        })
    }
}

/// What the revocation answers kept in the document say of a chain.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Revocation {
    /// The document keeps no answer (or not for every certificate).
    #[default]
    NotChecked,
    /// Not revoked, as of this time (the oldest of the answers).
    Good(i64),
    /// A certificate of the chain was revoked at this time.
    Revoked(i64),
}

/// Certificates and revocation answers (DER), as a document keeps them.
#[derive(Clone, Debug, Default)]
pub struct Validation {
    pub certs: Vec<Cert>,
    /// OCSPResponse structures.
    pub ocsps: Vec<Vec<u8>>,
    pub crls: Vec<Vec<u8>>,
}

impl Validation {
    pub fn is_empty(&self) -> bool {
        self.certs.is_empty() && self.ocsps.is_empty() && self.crls.is_empty()
    }

    pub fn add_cert(&mut self, c: &Cert) {
        if self.certs.len() < crate::MAX_CERTS && !self.certs.iter().any(|x| x.der == c.der) {
            self.certs.push(c.clone());
        }
    }

    pub fn add_ocsp(&mut self, d: Vec<u8>) {
        if self.ocsps.len() < crate::MAX_REVOCATION && !self.ocsps.contains(&d) {
            self.ocsps.push(d);
        }
    }

    pub fn add_crl(&mut self, d: Vec<u8>) {
        if self.crls.len() < crate::MAX_REVOCATION && !self.crls.contains(&d) {
            self.crls.push(d);
        }
    }

    pub fn merge(&mut self, o: &Validation) {
        for c in &o.certs {
            self.add_cert(c);
        }
        for d in &o.ocsps {
            self.add_ocsp(d.clone());
        }
        for d in &o.crls {
            self.add_crl(d.clone());
        }
    }

    /// What's in `self` and not in `have`.
    pub fn minus(&self, have: &Validation) -> Validation {
        Validation {
            certs: self.certs.iter().filter(|c| !have.certs.iter().any(|h| h.der == c.der)).cloned().collect(),
            ocsps: self.ocsps.iter().filter(|d| !have.ocsps.contains(d)).cloned().collect(),
            crls: self.crls.iter().filter(|d| !have.crls.contains(d)).cloned().collect(),
        }
    }

    /// The answers about `cert` (issued by `issuer`) whose signatures hold.
    pub fn evidence(&self, cert: &Cert, issuer: &Cert) -> Vec<Evidence> {
        let mut out: Vec<Evidence> = self.ocsps.iter().filter_map(|d| revocation::check_ocsp(d, cert, issuer, &self.certs)).collect();
        out.extend(self.crls.iter().filter_map(|d| revocation::check_crl(d, cert, issuer)));
        out.retain(|e| e.status != CertStatus::Unknown);
        out
    }

    /// What the answers say of `chain` (leaf first): every certificate
    /// but a self-issued root needs one. A revoked one wins; else the
    /// oldest of the newest answers dates the check.
    pub fn assess(&self, chain: &[Cert]) -> Revocation {
        let mut oldest = i64::MAX;
        let mut checked = 0;
        for (i, c) in chain.iter().enumerate() {
            if c.self_issued() {
                break;
            }
            let Some(issuer) = chain.get(i + 1) else { return Revocation::NotChecked };
            let ev = self.evidence(c, issuer);
            if let Some(t) = ev.iter().find_map(|e| if let CertStatus::Revoked(t) = e.status { Some(t) } else { None }) {
                return Revocation::Revoked(t);
            }
            let Some(newest) = ev.iter().map(|e| e.this_update).max() else { return Revocation::NotChecked };
            oldest = oldest.min(newest);
            checked += 1;
        }
        if checked == 0 { Revocation::NotChecked } else { Revocation::Good(oldest) }
    }
}

/// The chain of `leaf` (leaf first) from `pool`, downloading issuers its
/// certificates point to when the chain stops short.
fn chain_of(net: &dyn Net, leaf: &Cert, pool: &mut Vec<Cert>) -> Vec<Cert> {
    for _ in 0..4 {
        let (chain, _) = crate::trust::check(leaf, pool, crate::time::now());
        let last = chain.last().expect("chain has the leaf");
        if last.self_issued() || crate::trust::is_root(last) {
            return chain;
        }
        let mut found = false;
        for url in last.ca_issuers.iter().filter(|u| u.starts_with("http") && crate::net::not_private(u).is_ok()) {
            let Ok(data) = net.get(url) else { continue };
            for c in certs_of(&data) {
                if c.named_issuer_of(last) && c.signed(last) && !pool.iter().any(|p| p.der == c.der) {
                    pool.push(c);
                    found = true;
                }
            }
            if found {
                break;
            }
        }
        if !found {
            return chain;
        }
    }
    crate::trust::check(leaf, pool, crate::time::now()).0
}

/// The certificates of a download: one in DER, or a PKCS #7 bundle.
fn certs_of(data: &[u8]) -> Vec<Cert> {
    if let Ok(c) = Cert::from_der(data) {
        return vec![c];
    }
    crate::cms::parse(data).map(|sd| sd.certs).unwrap_or_default()
}

/// Gathers what checking `leaves` later needs (each with the certificates
/// that came with it): their chains and an answer about each certificate
/// of them, beyond what `have` already holds. Fails when a certificate
/// can't be checked.
pub fn gather(net: &dyn Net, leaves: &[(Cert, Vec<Cert>)], have: &Validation) -> Result<Validation, Error> {
    let mut all = have.clone();
    let mut pool: Vec<Cert> = have.certs.clone();
    for (_, certs) in leaves {
        for c in certs {
            if !pool.iter().any(|p| p.der == c.der) {
                pool.push(c.clone());
            }
        }
    }
    let mut work: Vec<Cert> = leaves.iter().map(|(c, _)| c.clone()).collect();
    let mut done: Vec<Vec<u8>> = Vec::new();
    while let Some(leaf) = work.pop() {
        if done.contains(&leaf.der) || done.len() > 32 {
            continue;
        }
        done.push(leaf.der.clone());
        let chain = chain_of(net, &leaf, &mut pool);
        // The addresses of a chain no trusted authority vouches for are
        // anyone's (a signature put in the document to make us ask the
        // local network): nothing is asked for it.
        if !chain.last().is_some_and(crate::trust::is_root) {
            continue;
        }
        for c in &chain {
            all.add_cert(c);
        }
        for (i, c) in chain.iter().enumerate() {
            if c.self_issued() {
                break;
            }
            let Some(issuer) = chain.get(i + 1) else {
                return Err(Error::new(format!("the issuer of {} wasn't found", c.subject.display())));
            };
            let known = all.evidence(c, issuer);
            let ev = match known.into_iter().max_by_key(|e| e.this_update) {
                Some(e) => e,
                None => {
                    let (source, der, e) = revocation::fetch(net, c, issuer, &pool)?;
                    match source {
                        revocation::Source::Ocsp => all.add_ocsp(der),
                        revocation::Source::Crl => all.add_crl(der),
                    }
                    e
                }
            };
            if let Some(r) = ev.responder {
                all.add_cert(&r);
                if !r.ocsp_nocheck {
                    // A delegated responder is checked too, unless it says
                    // not to.
                    if !pool.iter().any(|p| p.der == r.der) {
                        pool.push(r.clone());
                    }
                    work.push(r);
                }
            }
        }
    }
    Ok(all.minus(have))
}

/// The earliest expiry along a chain: past it the chain can't be checked.
pub fn chain_until(chain: &[Cert]) -> Option<i64> {
    chain.iter().map(|c| c.not_after).min()
}
