//! Digital signatures of documents: checking the signatures of OOXML
//! (.docx, .xlsx…), ODF (.odt, .ods…) and PDF files, and signing them
//! (XAdES in packages, PAdES in PDF) with a certificate file (.p12,
//! .pfx), a certificate of the system's store or one on a smart card or
//! token (PKCS #11). Any X.509 certificate works: nothing here is tied to
//! a country or an authority. Signatures can carry what verifying them
//! years later needs (PAdES B-LT and B-LTA, XAdES-XL and XAdES-A), when
//! signing or afterwards ([`extend`]).

pub mod asn1;

/// Most certificates read from one signature or validation store: each
/// may be tried against every other while building a chain.
pub(crate) const MAX_CERTS: usize = 256;
/// Revocation answers (OCSP, CRLs) kept: a chain needs a few.
pub(crate) const MAX_REVOCATION: usize = 1024;

mod c14n;
pub mod cms;
pub mod crypto;
pub mod ltv;
pub mod name_limits;
pub mod net;
mod package;
pub mod p12;
mod pdf;
mod pdf_obj;
mod pdf_ltv;
mod pdf_sign;
#[cfg_attr(target_arch = "wasm32", path = "pkcs11_web.rs")]
pub mod pkcs11;
pub mod revocation;
pub mod signer;
pub mod store;
pub mod time;
pub mod trust;
pub mod tsa;
pub mod vba;
pub mod x509;
mod xades;
mod xml_sign;
mod xmldsig;

pub use ltv::{Level, Revocation};
pub use p12::{Identity, P12Error};
pub use pdf_sign::{SignOptions, pdf_can_add_to, sign_pdf, sign_pdf_with};
pub use signer::Signer;
pub use trust::Trust;
pub use xml_sign::{can_sign_package, sign_package, sign_package_with};
pub use x509::Cert;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(String);

impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Error(msg.into())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<apex_io::Error> for Error {
    fn from(e: apex_io::Error) -> Self {
        Error(e.to_string())
    }
}

/// Where a signature lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Ooxml,
    Odf,
    Pdf,
    /// A PDF document time-stamp (it proves a time, not a person).
    PdfTimestamp,
}

/// The outcome of one check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    Ok,
    Failed,
    /// It couldn't be checked (an unsupported algorithm, a missing part).
    Unknown,
}

/// One signature of a file and what checking it found.
#[derive(Clone, Debug)]
pub struct Signature {
    pub kind: Kind,
    /// "XAdES", "PAdES (CAdES)"…
    pub format: String,
    /// The signer's certificate.
    pub cert: Option<Cert>,
    /// Every certificate that came with the signature.
    pub certs: Vec<Cert>,
    /// The chain from the signer up to the root found.
    pub chain: Vec<Cert>,
    /// When it was signed (Unix seconds, UTC).
    pub time: Option<i64>,
    /// The time came without a zone: it's the signer's local time, to be
    /// shown as is.
    pub local_time: bool,
    /// The time comes from a time-stamp authority (else the signer's
    /// computer claims it).
    pub timestamped: bool,
    /// The signed content is unchanged.
    pub intact: Check,
    /// The signature value is right for the certificate.
    pub valid: Check,
    pub trust: Trust,
    /// The file changed after signing in a way the signature doesn't
    /// cover: later PDF revisions, parts added to a package.
    pub changed_after: bool,
    pub unsigned_parts: Vec<String>,
    /// Parts whose content no longer matches.
    pub changed_parts: Vec<String>,
    pub name: Option<String>,
    pub reason: Option<String>,
    pub location: Option<String>,
    /// Why something couldn't be checked.
    pub note: Option<String>,
    /// A PDF signature's /ByteRange (which field it is).
    pub byte_range: Vec<i64>,
    /// The time-stamp authority of the signature's time-stamp, and its
    /// chain.
    pub tsa: Option<Cert>,
    pub tsa_chain: Vec<Cert>,
    /// What the document keeps for verifying it later.
    pub level: Level,
    /// What the revocation answers kept in the document say.
    pub revocation: Revocation,
    /// Until when it can still be verified: the first expiry among the
    /// certificates its proof rests on (an archive time-stamp can be
    /// renewed before then).
    pub verifiable_until: Option<i64>,
    /// When the newest archive time-stamp covering it was made.
    pub archive_time: Option<i64>,
}

/// An archive time-stamp over a signature: when, until when its own
/// certificates last, and the validation data it covers.
pub(crate) struct Archive {
    pub time: i64,
    pub until: Option<i64>,
    pub kept: ltv::Validation,
}

/// A signature's overall state, from worst to best.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    /// The content changed or the signature is wrong.
    Invalid,
    /// It couldn't be checked.
    Unknown,
    /// Right, but the file changed afterwards (later revisions).
    ChangedAfter,
    /// Right, but the certificate isn't trusted (or couldn't be checked).
    Untrusted,
    Valid,
}

impl Signature {
    pub fn new(kind: Kind) -> Self {
        Signature {
            kind,
            format: String::new(),
            cert: None,
            certs: Vec::new(),
            chain: Vec::new(),
            time: None,
            local_time: false,
            timestamped: false,
            intact: Check::Unknown,
            valid: Check::Unknown,
            trust: Trust::NotVerified,
            changed_after: false,
            unsigned_parts: Vec::new(),
            changed_parts: Vec::new(),
            name: None,
            reason: None,
            location: None,
            note: None,
            byte_range: Vec::new(),
            tsa: None,
            tsa_chain: Vec::new(),
            level: Level::B,
            revocation: Revocation::NotChecked,
            verifiable_until: None,
            archive_time: None,
        }
    }

    /// Works out the chain and the trust once the rest is known.
    fn finish(mut self) -> Self {
        if let Some(cert) = self.cert.clone() {
            let (chain, trust) = trust::check(&cert, &self.certs, self.trusted_time());
            self.chain = chain;
            self.trust = self.fit(&cert, trust);
        }
        self
    }

    /// When the certificate is judged: at a trusted time-stamp's time;
    /// without one, now. The time the signer's computer gave is the
    /// signer's to choose (a revoked or expired key could be dated back).
    fn trusted_time(&self) -> i64 {
        self.time.filter(|_| self.timestamped).unwrap_or_else(time::now)
    }

    /// `trust`, unless the certificate was issued for another use.
    fn fit(&self, cert: &x509::Cert, trust: Trust) -> Trust {
        let fit = if self.kind == Kind::PdfTimestamp { trust::fit_for_timestamping(cert) } else { trust::fit_for_documents(cert) };
        if trust.is_trusted() && !fit { Trust::WrongUse } else { trust }
    }

    /// Works out the level, the revocation and how long it stays
    /// verifiable from the validation data the document keeps (`kept`)
    /// and the newest archive time-stamp covering the signature.
    pub(crate) fn judge(&mut self, kept: &ltv::Validation, archive: Option<&Archive>) {
        let Some(cert) = self.cert.clone() else { return };
        if !kept.certs.is_empty() && !self.trust.is_trusted() {
            // The chain may go on through certificates kept apart.
            let mut pool = self.certs.clone();
            pool.extend(kept.certs.iter().cloned());
            let (chain, trust) = trust::check(&cert, &pool, self.trusted_time());
            self.chain = chain;
            self.trust = self.fit(&cert, trust);
        }
        let rev = kept.assess(&self.chain);
        self.revocation = rev;
        if let Revocation::Revoked(t) = rev
            && self.time.filter(|_| self.timestamped).is_none_or(|s| t <= s)
        {
            self.trust = Trust::Revoked(t);
        }
        let own = self.kind == Kind::PdfTimestamp;
        let has_time = self.timestamped || archive.is_some();
        let lt = has_time && matches!(rev, Revocation::Good(_));
        let covered = archive.is_some_and(|a| matches!(a.kept.assess(&self.chain), Revocation::Good(_)));
        self.level = if lt && covered {
            Level::Lta
        } else if lt {
            Level::Lt
        } else if has_time {
            Level::T
        } else {
            Level::B
        };
        let signer_until = ltv::chain_until(&self.chain);
        let stamp_until = if own { signer_until } else if self.timestamped { ltv::chain_until(&self.tsa_chain) } else { None };
        let stamp_until = stamp_until.or(archive.and_then(|a| a.until));
        self.verifiable_until = match self.level {
            Level::Lta => archive.and_then(|a| a.until),
            Level::Lt => stamp_until,
            Level::T => match (signer_until, stamp_until) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            },
            Level::B => signer_until,
        };
        self.archive_time = if self.level == Level::Lta { archive.map(|a| a.time) } else { None };
    }

    /// The level by the format's own names (PAdES B-LT, XAdES-XL…).
    pub fn level_name(&self) -> String {
        match self.kind {
            Kind::Pdf => format!("PAdES {}", self.level.pades()),
            Kind::PdfTimestamp => "RFC 3161".into(),
            Kind::Ooxml | Kind::Odf if self.format == "XAdES" => self.level.xades().into(),
            _ => self.format.clone(),
        }
    }

    pub fn status(&self) -> Status {
        if matches!(self.trust, Trust::Revoked(_)) {
            return Status::Invalid;
        }
        if self.intact == Check::Failed || self.valid == Check::Failed {
            Status::Invalid
        } else if self.intact == Check::Unknown || self.valid == Check::Unknown {
            Status::Unknown
        } else if self.changed_after {
            Status::ChangedAfter
        } else if self.trust.is_trusted() {
            Status::Valid
        } else {
            Status::Untrusted
        }
    }

    /// Who signed: the certificate's name, else the name the signer gave.
    pub fn signer(&self) -> String {
        self.cert.as_ref().map(|c| c.subject.display()).or_else(|| self.name.as_deref().map(x509::one_line)).unwrap_or_default()
    }
}

/// Whether a file (its bytes) has digital signatures, without checking
/// them.
pub fn is_signed(data: &[u8]) -> bool {
    if is_pdf(data) {
        return data.windows(10).any(|w| w == b"/ByteRange");
    }
    package::is_signed(data)
}

fn is_pdf(data: &[u8]) -> bool {
    // A package's first entry can hold the marker too.
    !data.starts_with(b"PK\x03\x04") && data.get(..1024).unwrap_or(data).windows(5).any(|w| w == b"%PDF-")
}

/// Checks the signatures of a file: a PDF, or an OOXML or ODF package
/// (decrypted if it was encrypted).
pub fn verify(data: &[u8]) -> Vec<Signature> {
    if is_pdf(data) {
        return pdf::verify(data);
    }
    let mut sigs = package::verify(data);
    // A package PDF readers would read as a PDF (its marker in the first
    // kilobyte): they'd show something else than what was signed.
    if data.get(..1024).unwrap_or(data).windows(5).any(|w| w == b"%PDF-") {
        sigs.iter_mut().for_each(|s| s.changed_after = true);
    }
    sigs
}

/// Whether some signatures are only time-stamps of a PDF: no one signed
/// what they cover.
pub fn stamps_only(sigs: &[Signature]) -> bool {
    !sigs.is_empty() && sigs.iter().all(|s| s.kind == Kind::PdfTimestamp)
}

/// Adds what verifying the signatures of a file years later needs, up to
/// `level`: the certificates' chains and revocation answers (B-LT,
/// XAdES-XL) and an archive time-stamp (B-LTA, XAdES-A) from the
/// authority at `tsa`. Run on a file at B-LTA it renews the archive
/// time-stamp. Returns the extended file.
pub fn extend(data: &[u8], level: Level, tsa: Option<&str>) -> Result<Vec<u8>, Error> {
    extend_with(data, level, tsa, &net::Http)
}

/// [`extend`] through `net`.
pub fn extend_with(data: &[u8], level: Level, tsa: Option<&str>, net: &dyn net::Net) -> Result<Vec<u8>, Error> {
    let tsa = tsa.map(str::trim).filter(|u| !u.is_empty());
    if is_pdf(data) { pdf_ltv::extend(data, level, tsa, net, false) } else { xades::extend(data, level, tsa, net) }
}

/// Encodes base64 (standard alphabet, with padding).
pub(crate) fn base64_encode(data: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            out.push(if i <= c.len() { ABC[(n >> (18 - 6 * i)) as usize & 63] as char } else { '=' });
        }
    }
    out
}

/// Decodes base64, ignoring white space.
pub(crate) fn base64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            c if c.is_ascii_whitespace() => continue,
            _ => return None,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}
