//! Signatures of VBA projects: the PKCS #7 SignedData in a project's
//! DigitalSignature stream (MS-OVBA 2.3.4.x), its signer, whether the
//! signature is intact and whether the certificate is trusted.
//!
//! A signature only counts for a project whose code hashes (MS-OVBA
//! 2.4.2) to the hash it signs ([`apex_io::vba::signed_for`]): one copied
//! from another project, or a kind of hash not computed here, is not
//! verified. The certificate must be one for signing code. Only the V3
//! hash binds the code as it runs ([`apex_io::vba::signed_for_v3`]): the
//! older ones are shown, but don't let macros run under a "signed only"
//! policy.

use crate::asn1;
use crate::cms::{self, SignedData};
use crate::crypto::Hash;
use crate::trust::{self, Trust};
use crate::x509::Cert;

pub struct VbaSigner {
    pub cert: Cert,
    pub trust: Trust,
    /// The signature is right for the content it signs.
    pub intact: bool,
    /// The certificate is one for signing code (not, say, a web server's).
    pub code_signing: bool,
    /// The signature was made for this project's code as it is now (not
    /// verified: copied from other code, or a hash not computed here).
    pub code_matches: bool,
    /// It is a V3 signature made for this code: the older hashes join all
    /// lines into one run, so moved line breaks or module kinds still match.
    pub v3: bool,
    /// The certificates from the signer's up to a root, as far as found.
    pub chain: Vec<Cert>,
}

impl VbaSigner {
    /// Good enough to let the macros run under a "signed only" policy.
    pub fn is_trusted(&self) -> bool {
        self.intact && self.code_matches && self.v3 && self.code_signing && self.trust.is_trusted()
    }

    /// [`is_trusted`](Self::is_trusted), or made by one of `publishers`
    /// (certificates' SHA-256 or SHA-1 in hex, with or without colons) the
    /// user or the organization trusts even without a chain to a root;
    /// with `only_listed`, only by those (an organization's own).
    pub fn is_trusted_with(&self, publishers: &[String], only_listed: bool) -> bool {
        let sound = self.intact && self.code_matches && self.v3 && self.code_signing;
        let listed = publishers.iter().any(|p| is_publisher(&self.cert, p)) && self.cert.valid_at(crate::time::now()) && !matches!(self.trust, Trust::Expired | Trust::Revoked(_) | Trust::WrongUse);
        sound && (listed || (!only_listed && self.trust.is_trusted()))
    }

    /// Signed for this code as it is now by a code-signing certificate,
    /// whoever that is: what the user may choose to trust.
    pub fn is_sound(&self) -> bool {
        self.intact && self.code_matches && self.v3 && self.code_signing && !matches!(self.trust, Trust::Expired | Trust::Revoked(_) | Trust::WrongUse) && self.cert.valid_at(crate::time::now())
    }
}

/// Whether `cert` is the one named by `id`: its SHA-256 or SHA-1 in hex
/// (what follows a space, a name to show, doesn't count).
pub fn is_publisher(cert: &Cert, id: &str) -> bool {
    let norm = |s: &str| s.chars().filter(|c| *c != ':').collect::<String>().to_ascii_uppercase();
    let id = norm(id.split_whitespace().next().unwrap_or(""));
    !id.is_empty() && (id == norm(&cert.fingerprint()) || id == cert.sha1())
}

/// Authenticode's SpcIndirectDataContent, whose V2 carries the V3 hash.
const SPC_INDIRECT_DATA: &str = "1.3.6.1.4.1.311.2.1.4";
const SPC_INDIRECT_DATA_V2: &str = "1.3.6.1.4.1.311.2.1.31";
const SPC_SP_OPUS_INFO: &str = "1.3.6.1.4.1.311.2.1.12";
const SHA256: &str = "2.16.840.1.101.3.4.2.1";

/// A V3 signature (MS-OSHARED 2.3.2.4.3.1) of the vbaProject.bin
/// `project` by `signer`: the PKCS #7 SignedData to put in the project's
/// vbaProjectSignatureV3.bin part. An error for a project whose V3 hash
/// can't be told for sure (write it with
/// [`apex_io::vba::VbaProject::write_for_signing`]).
pub fn sign(signer: &dyn crate::signer::Signer, project: &[u8]) -> Result<Vec<u8>, crate::Error> {
    let hash = apex_io::vba::v3_source_hash(project).ok_or_else(|| crate::Error::new("this macro project can't be signed"))?;
    crate::cms::sign_embedded(signer, SPC_INDIRECT_DATA, &signed_content_v3(&hash), &[asn1::seq(&[&asn1::oid(SPC_SP_OPUS_INFO), &asn1::set_of(&[&asn1::seq(&[])])])])
}

/// SpcIndirectDataContentV2 for the SHA-256 V3 hash `hash`: a
/// SigFormatDescriptorV1 and a SigDataV1Serialized, both raw structures
/// with little-endian fields.
fn signed_content_v3(hash: &[u8]) -> Vec<u8> {
    let le = |v: &mut Vec<u8>, n: u32| v.extend_from_slice(&n.to_le_bytes());
    let mut format = Vec::new();
    for n in [12, 1, 1] {
        le(&mut format, n);
    }
    let alg_id = format!("{SHA256}\0");
    let mut data = Vec::new();
    let alg_offset = 24;
    let hash_offset = alg_offset + alg_id.len() as u32;
    for n in [alg_id.len() as u32, 0, hash.len() as u32, alg_offset, hash_offset, hash_offset] {
        le(&mut data, n);
    }
    data.extend_from_slice(alg_id.as_bytes());
    data.extend_from_slice(hash);
    asn1::seq(&[&asn1::seq(&[&asn1::oid(SPC_INDIRECT_DATA_V2), &asn1::octets(&format)]), &asn1::seq(&[&asn1::alg(SHA256, true), &asn1::octets(&data)])])
}

/// The signer of a DigitalSignature stream (or OOXML signature part) of
/// the vbaProject.bin `project`, None when there is no readable
/// signature in it.
pub fn check(stream: &[u8], project: &[u8]) -> Option<VbaSigner> {
    let sd = find_signed_data(stream)?;
    let s = sd.signers.first()?;
    let content = sd.content.as_deref()?;
    let digest = s.hash.unwrap_or(Hash::Sha256).digest(content);
    let checked = sd.check(s, &digest);
    let cert = checked.cert?;
    let (chain, trust) = trust::check(&cert, &sd.certs, crate::time::now());
    let code_signing = trust::fit_for_code_signing(&cert);
    // A signature over a SHA-1 digest binds nothing: collisions can be
    // made for it.
    let v3 = s.hash != Some(Hash::Sha1) && apex_io::vba::signed_for_v3(project, content);
    let code_matches = v3 || apex_io::vba::signed_for(project, content);
    Some(VbaSigner { cert, trust, intact: checked.digest_ok && checked.signature_ok, code_signing, code_matches, v3, chain })
}

/// The first ContentInfo holding a SignedData in `b` (the stream has a
/// small header before it, of a different size in each version).
fn find_signed_data(b: &[u8]) -> Option<SignedData> {
    const OID: [u8; 11] = [0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x07, 0x02];
    (0..b.len()).filter(|&i| b[i] == 0x30).find_map(|i| {
        let head = &b[i..(i + 17).min(b.len())];
        if head.windows(OID.len()).any(|w| w == OID) { cms::parse(&b[i..]).ok() } else { None }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_code_signing_certificates() {
        let mut c = Cert::from_der(include_bytes!("../tests/data/ca.der")).unwrap();
        c.key_usage = None;
        c.eku = vec!["1.3.6.1.5.5.7.3.1".into()];
        assert!(!trust::fit_for_code_signing(&c));
        c.eku = vec!["1.3.6.1.5.5.7.3.1".into(), "1.3.6.1.5.5.7.3.3".into()];
        assert!(trust::fit_for_code_signing(&c));
        c.key_usage = Some(0x2000);
        assert!(!trust::fit_for_code_signing(&c));
        c.key_usage = Some(0x8000);
        assert!(trust::fit_for_code_signing(&c));
        // A certificate for any purpose isn't one for code.
        c.eku.clear();
        assert!(!trust::fit_for_code_signing(&c));
        c.eku = vec!["2.5.29.37.0".into()];
        assert!(!trust::fit_for_code_signing(&c));
    }

    #[test]
    fn publishers_by_fingerprint() {
        let c = Cert::from_der(include_bytes!("../tests/data/ca.der")).unwrap();
        assert!(is_publisher(&c, &c.fingerprint()));
        assert!(is_publisher(&c, &c.fingerprint().replace(':', "").to_lowercase()));
        assert!(is_publisher(&c, &c.sha1()));
        assert!(!is_publisher(&c, ""));
        assert!(!is_publisher(&c, &c.sha1()[..20]));
        assert!(is_publisher(&c, &format!("{} ACME Ltd", c.fingerprint())));
        // A name made of hex letters doesn't make up the rest of the hash.
        assert!(!is_publisher(&c, &format!("{} {}", &c.sha1()[..20], &c.sha1()[20..])));
    }

    #[test]
    fn trusted_publishers() {
        let mut cert = Cert::from_der(include_bytes!("../tests/data/ca.der")).unwrap();
        cert.not_before = 0;
        cert.not_after = i64::MAX;
        let fp = cert.fingerprint();
        let mut s = VbaSigner { cert, trust: Trust::SelfSigned, intact: true, code_signing: true, code_matches: true, v3: true, chain: Vec::new() };
        let none: [String; 0] = [];
        assert!(!s.is_trusted_with(&none, false) && s.is_sound());
        assert!(s.is_trusted_with(&[fp.clone()], false) && s.is_trusted_with(&[fp.clone()], true));
        // Only the organization's: a trusted chain alone isn't enough.
        s.trust = Trust::Trusted("Root".into());
        assert!(s.is_trusted_with(&none, false) && !s.is_trusted_with(&none, true));
        // A listed publisher never vouches for changed code or a revoked certificate.
        s.code_matches = false;
        assert!(!s.is_trusted_with(&[fp.clone()], false) && !s.is_sound());
        s.code_matches = true;
        s.trust = Trust::Revoked(5);
        assert!(!s.is_trusted_with(&[fp.clone()], false) && !s.is_sound());
        s.trust = Trust::SelfSigned;
        s.cert.not_after = 1;
        assert!(!s.is_trusted_with(&[fp], false) && !s.is_sound());
    }

    #[test]
    fn unsigned_or_broken() {
        assert!(check(b"", b"").is_none());
        assert!(check(&[0u8; 64], b"").is_none());
        // The OID with nothing valid around it.
        let mut b = vec![0u8; 8];
        b.extend([0x30, 0x82, 0x00, 0x10, 0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x07, 0x02, 0xA0, 0x00]);
        assert!(check(&b, b"").is_none());
    }
}
