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
}

impl VbaSigner {
    /// Good enough to let the macros run under a "signed only" policy.
    pub fn is_trusted(&self) -> bool {
        self.intact && self.code_matches && self.v3 && self.code_signing && self.trust.is_trusted()
    }
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
    let (_, trust) = trust::check(&cert, &sd.certs, crate::time::now());
    let code_signing = trust::fit_for_code_signing(&cert);
    let v3 = apex_io::vba::signed_for_v3(project, content);
    let code_matches = v3 || apex_io::vba::signed_for(project, content);
    Some(VbaSigner { cert, trust, intact: checked.digest_ok && checked.signature_ok, code_signing, code_matches, v3 })
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
    fn unsigned_or_broken() {
        assert!(check(b"", b"").is_none());
        assert!(check(&[0u8; 64], b"").is_none());
        // The OID with nothing valid around it.
        let mut b = vec![0u8; 8];
        b.extend([0x30, 0x82, 0x00, 0x10, 0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x07, 0x02, 0xA0, 0x00]);
        assert!(check(&b, b"").is_none());
    }
}
