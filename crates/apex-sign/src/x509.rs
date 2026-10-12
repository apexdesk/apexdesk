//! X.509 certificates: the fields a signature check needs and names
//! as people read them.

use crate::Error;
use crate::asn1::{self, Tlv};
use crate::crypto::{Hash, PubKey};

/// A distinguished name: attribute types (dotted) and values in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Name {
    pub attrs: Vec<(String, String)>,
}

/// Short labels of the usual name attributes.
fn label(oid: &str) -> Option<&'static str> {
    Some(match oid {
        "2.5.4.3" => "CN",
        "2.5.4.4" => "SN",
        "2.5.4.42" => "GN",
        "2.5.4.5" => "SERIALNUMBER",
        "2.5.4.6" => "C",
        "2.5.4.7" => "L",
        "2.5.4.8" => "ST",
        "2.5.4.10" => "O",
        "2.5.4.11" => "OU",
        "2.5.4.12" => "T",
        "2.5.4.97" => "organizationIdentifier",
        "1.2.840.113549.1.9.1" => "E",
        "0.9.2342.19200300.100.1.25" => "DC",
        _ => return None,
    })
}

impl Name {
    pub(crate) fn parse(t: &Tlv) -> Name {
        let mut attrs = Vec::new();
        let mut rdns = t.children();
        while let Some(rdn) = rdns.next() {
            let mut atvs = rdn.children();
            while let Some(atv) = atvs.next() {
                let mut r = atv.children();
                if let (Some(ty), Some(v)) = (r.next(), r.next()) {
                    attrs.push((ty.oid(), v.string()));
                }
            }
        }
        Name { attrs }
    }

    pub fn get(&self, oid: &str) -> Option<&str> {
        self.attrs.iter().find(|(o, _)| o == oid).map(|(_, v)| v.as_str())
    }

    /// The name to show: the common name, else the given name and surname,
    /// the organization or the whole name. On one line, without control or
    /// direction characters, runs of spaces or more than 100 characters:
    /// the certificate's maker picks it, and it is shown before what was
    /// found of it.
    pub fn display(&self) -> String {
        let name = if let Some(cn) = self.get("2.5.4.3") {
            cn.to_string()
        } else if let (Some(g), Some(s)) = (self.get("2.5.4.42"), self.get("2.5.4.4")) {
            format!("{g} {s}")
        } else if let Some(o) = self.get("2.5.4.10") {
            o.to_string()
        } else {
            self.full()
        };
        let one = one_line(&name);
        match one.char_indices().nth(100) {
            Some((i, _)) => format!("{}…", &one[..i]),
            None => one,
        }
    }

    /// "CN=…, O=…, C=…" (most specific first).
    pub fn full(&self) -> String {
        let parts: Vec<String> = self.attrs.iter().rev().map(|(o, v)| format!("{}={}", label(o).unwrap_or(o), one_line(v))).collect();
        parts.join(", ")
    }

    /// The same name, compared loosely (case and spaces of the values).
    fn loose_eq(&self, other: &Name) -> bool {
        let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        self.attrs.len() == other.attrs.len() && self.attrs.iter().zip(&other.attrs).all(|(a, b)| a.0 == b.0 && norm(&a.1) == norm(&b.1))
    }
}

#[derive(Clone, Debug)]
pub struct Cert {
    pub der: Vec<u8>,
    tbs: (usize, usize),
    pub serial: Vec<u8>,
    issuer_der: Vec<u8>,
    subject_der: Vec<u8>,
    pub issuer: Name,
    pub subject: Name,
    pub not_before: i64,
    pub not_after: i64,
    spki: Vec<u8>,
    sig_alg: Vec<u8>,
    signature: Vec<u8>,
    /// Basic constraints: a certification authority.
    pub ca: bool,
    /// Key usage bits (bit 0 = digitalSignature, as in the standard, in
    /// the lowest bit of the first byte's highest position).
    pub key_usage: Option<u16>,
    pub ski: Option<Vec<u8>>,
    pub aki: Option<Vec<u8>>,
    /// Where to ask about its revocation: OCSP responders and CRLs.
    pub ocsp_urls: Vec<String>,
    pub crl_urls: Vec<String>,
    /// Where its issuer's certificate can be downloaded.
    pub ca_issuers: Vec<String>,
    /// Extended key usages (dotted).
    pub eku: Vec<String>,
    /// An OCSP responder whose own revocation isn't to be checked.
    pub ocsp_nocheck: bool,
    /// Basic constraints: most CAs that may sit below it.
    pub path_len: Option<u64>,
    /// Limits this program doesn't apply: name constraints, or another
    /// extension marked critical that it doesn't know.
    pub unchecked_limits: bool,
}

/// The URIs among some GeneralNames.
fn uris(names: &Tlv, out: &mut Vec<String>) {
    let mut r = names.children();
    while let Some(n) = r.next() {
        if n.tag == 0x86 {
            out.push(String::from_utf8_lossy(n.body).trim().to_string());
        }
    }
}

impl Cert {
    pub fn from_der(der: &[u8]) -> Result<Cert, Error> {
        let cert = asn1::parse(der)?.expect(asn1::SEQUENCE)?;
        let der = cert.raw.to_vec();
        let base = cert.raw.as_ptr() as usize;
        let mut r = cert.children();
        let tbs = r.req()?.expect(asn1::SEQUENCE)?;
        let sig_alg = r.req()?.raw.to_vec();
        let signature = r.req()?.bits().to_vec();
        let off = tbs.raw.as_ptr() as usize - base;
        let mut t = tbs.children();
        t.take(0xA0);
        let serial = t.req()?.body.to_vec();
        t.req()?;
        let issuer = t.req()?;
        let validity = t.req()?;
        let subject = t.req()?;
        let spki = t.req()?.raw.to_vec();
        let mut v = validity.children();
        let not_before = v.req()?.time().unwrap_or(i64::MIN);
        let not_after = v.req()?.time().unwrap_or(i64::MAX);
        let mut c = Cert {
            der: der.clone(),
            tbs: (off, off + tbs.raw.len()),
            serial,
            issuer_der: issuer.raw.to_vec(),
            subject_der: subject.raw.to_vec(),
            issuer: Name::parse(&issuer),
            subject: Name::parse(&subject),
            not_before,
            not_after,
            spki,
            sig_alg,
            signature,
            ca: false,
            key_usage: None,
            ski: None,
            aki: None,
            ocsp_urls: Vec::new(),
            crl_urls: Vec::new(),
            ca_issuers: Vec::new(),
            eku: Vec::new(),
            ocsp_nocheck: false,
            path_len: None,
            unchecked_limits: false,
        };
        while let Some(x) = t.next() {
            if x.tag != 0xA3 {
                continue;
            }
            let Ok(exts) = x.inner() else { continue };
            let mut er = exts.children();
            while let Some(ext) = er.next() {
                let mut f = ext.children();
                let Some(id) = f.next().map(|o| o.oid()) else { continue };
                let critical = f.take(asn1::BOOLEAN).is_some_and(|b| b.body.first().is_some_and(|&v| v != 0));
                let Some(val) = f.next() else { continue };
                let Ok(inner) = asn1::parse(val.body) else {
                    // A limit that can't be read can't be applied.
                    c.unchecked_limits |= critical || id == "2.5.29.30";
                    continue;
                };
                match id.as_str() {
                    "2.5.29.19" => {
                        let mut bc = inner.children();
                        c.ca = bc.take(asn1::BOOLEAN).is_some_and(|b| b.body.first().is_some_and(|&v| v != 0));
                        c.path_len = bc.next().and_then(|n| n.uint());
                    }
                    // Name constraints: which names it may issue for.
                    "2.5.29.30" => c.unchecked_limits = true,
                    // Known and not limiting what is checked here: alternative
                    // names, policies and their constraints.
                    "2.5.29.17" | "2.5.29.32" | "2.5.29.33" | "2.5.29.36" | "2.5.29.54" => {}
                    "2.5.29.15" => {
                        let b = inner.bits();
                        c.key_usage = Some(u16::from_be_bytes([b.first().copied().unwrap_or(0), b.get(1).copied().unwrap_or(0)]));
                    }
                    "2.5.29.14" => c.ski = Some(inner.body.to_vec()),
                    "2.5.29.35" => c.aki = inner.children().take(0x80).map(|k| k.body.to_vec()),
                    // Authority information access: OCSP and the issuer.
                    "1.3.6.1.5.5.7.1.1" => {
                        let mut ar = inner.children();
                        while let Some(ad) = ar.next() {
                            let mut f = ad.children();
                            let (Some(method), Some(loc)) = (f.next(), f.next()) else { continue };
                            if loc.tag != 0x86 {
                                continue;
                            }
                            let url = String::from_utf8_lossy(loc.body).trim().to_string();
                            match method.oid().as_str() {
                                "1.3.6.1.5.5.7.48.1" => c.ocsp_urls.push(url),
                                "1.3.6.1.5.5.7.48.2" => c.ca_issuers.push(url),
                                _ => {}
                            }
                        }
                    }
                    // CRL distribution points: their full names.
                    "2.5.29.31" => {
                        let mut dr = inner.children();
                        while let Some(dp) = dr.next() {
                            if let Some(name) = dp.children().take(0xA0)
                                && let Some(full) = name.children().take(0xA0)
                            {
                                uris(&full, &mut c.crl_urls);
                            }
                        }
                    }
                    "2.5.29.37" => {
                        let mut kr = inner.children();
                        while let Some(k) = kr.next() {
                            c.eku.push(k.oid());
                        }
                    }
                    "1.3.6.1.5.5.7.48.1.5" => c.ocsp_nocheck = true,
                    _ => c.unchecked_limits |= critical,
                }
            }
        }
        Ok(c)
    }

    pub fn tbs(&self) -> &[u8] {
        &self.der[self.tbs.0..self.tbs.1]
    }

    pub fn issuer_der(&self) -> &[u8] {
        &self.issuer_der
    }

    pub fn subject_der(&self) -> &[u8] {
        &self.subject_der
    }

    pub fn public_key(&self) -> Result<PubKey, Error> {
        PubKey::from_spki(&self.spki)
    }

    pub fn self_issued(&self) -> bool {
        self.issuer_der == self.subject_der
    }

    /// Whether `other` has this certificate's subject as its issuer.
    pub fn named_issuer_of(&self, other: &Cert) -> bool {
        if let (Some(ski), Some(aki)) = (&self.ski, &other.aki)
            && ski != aki
        {
            return false;
        }
        self.subject_der == other.issuer_der || self.subject.loose_eq(&other.issuer)
    }

    /// Whether this certificate's key signed `other`.
    pub fn signed(&self, other: &Cert) -> bool {
        // Authorities stopped signing certificates with SHA-1 by 2017:
        // a later one so signed could be a collision with one of theirs.
        const SHA1_SUNSET: i64 = 1_483_228_800;
        if other.not_before >= SHA1_SUNSET
            && asn1::parse(&other.sig_alg).ok().and_then(|a| crate::crypto::scheme_of(&a)).is_some_and(|(_, h)| h == Some(crate::crypto::Hash::Sha1))
        {
            return false;
        }
        self.verifies(other.tbs(), &other.sig_alg, &other.signature)
    }

    /// Whether this certificate's key made `sig` over `tbs` with the
    /// algorithm `alg` (an AlgorithmIdentifier as written): X.509
    /// structures such as CRLs and OCSP responses.
    pub fn verifies(&self, tbs: &[u8], alg: &[u8], sig: &[u8]) -> bool {
        let Ok(key) = self.public_key() else { return false };
        let Ok(alg) = asn1::parse(alg) else { return false };
        let Some((scheme, Some(hash))) = crate::crypto::scheme_of(&alg) else { return false };
        key.verify(scheme, hash, &hash.digest(tbs), sig)
    }

    /// The public key's bits (what OCSP's key hashes digest).
    pub fn key_bits(&self) -> Vec<u8> {
        let Ok(spki) = asn1::parse(&self.spki) else { return Vec::new() };
        let mut r = spki.children();
        r.next();
        r.next().map(|b| b.bits().to_vec()).unwrap_or_default()
    }

    /// Whether `t` is inside its validity period.
    pub fn valid_at(&self, t: i64) -> bool {
        self.not_before <= t && t <= self.not_after
    }

    /// Matches a CMS IssuerAndSerialNumber.
    pub fn is(&self, issuer: &[u8], serial: &[u8]) -> bool {
        let trim = |s: &[u8]| -> Vec<u8> { s.iter().skip_while(|&&b| b == 0).copied().collect() };
        trim(&self.serial) == trim(serial) && (self.issuer_der == issuer || asn1::parse(issuer).is_ok_and(|t| Name::parse(&t).loose_eq(&self.issuer)))
    }

    pub fn serial_hex(&self) -> String {
        let s: String = self.serial.iter().map(|b| format!("{b:02X}")).collect();
        let t = s.trim_start_matches('0');
        if t.is_empty() { "0".into() } else { t.into() }
    }

    /// SHA-1 of the certificate in hex, as Windows names certificates
    /// (the "thumbprint").
    pub fn sha1(&self) -> String {
        Hash::Sha1.digest(&self.der).iter().map(|b| format!("{b:02X}")).collect()
    }

    /// SHA-256 of the certificate, in hex.
    pub fn fingerprint(&self) -> String {
        Hash::Sha256.digest(&self.der).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
    }
}

/// Text a file's maker picked, shown on one line: without control or
/// direction characters (which could make it read as something else) and
/// with runs of spaces as one.
pub fn one_line(s: &str) -> String {
    let hidden = |c: char| c.is_control() || matches!(c, '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{2028}'..='\u{202E}' | '\u{2060}'..='\u{206F}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}');
    let words: Vec<&str> = s.split(|c: char| c.is_whitespace() || hidden(c)).filter(|w| !w.is_empty()).collect();
    words.join(" ")
}
