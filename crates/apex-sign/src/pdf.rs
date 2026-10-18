//! Signatures of PDF files: every signature dictionary (/ByteRange and
//! /Contents), checked over the bytes it covers — PKCS #7 and CAdES
//! (adbe.pkcs7.detached, adbe.pkcs7.sha1, ETSI.CAdES.detached) and
//! document time-stamps (ETSI.RFC3161). Revisions added after a
//! signature are reported, and the validation data the file keeps (its
//! DSS, what the signatures carry) tells how long each stays verifiable.

use crate::cms;
use crate::crypto::Hash;
use crate::ltv::Validation;
use crate::pdf_obj::{self, Obj};
use crate::{Archive, Check, Kind, Signature};

/// At most this many `/ByteRange` keys are looked at: each costs a look
/// back for its object, and a file repeating the key in a stream would
/// otherwise keep the check going for hours.
const MAX_KEYS: usize = 100;

/// Where each `/ByteRange` key is (the first [`MAX_KEYS`]).
fn byte_ranges(d: &[u8]) -> Vec<usize> {
    let key = b"/ByteRange";
    let mut out = Vec::new();
    let mut i = 0;
    while out.len() < MAX_KEYS
        && let Some(k) = d[i..].windows(key.len()).position(|w| w == key)
    {
        out.push(i + k);
        i += k + key.len();
    }
    out
}

/// The dictionary that holds the key at `at`: the object it's in.
fn enclosing_dict(d: &[u8], at: usize) -> Option<Obj> {
    let from = at.saturating_sub(256 * 1024);
    let hay = &d[from..at];
    let obj = hay.windows(3).rposition(|w| w == b"obj")? + from;
    if obj >= 3 && &d[obj - 3..obj] == b"end" {
        return None;
    }
    // Back to the object number.
    let mut s = obj;
    let mut fields = 0;
    while s > 0 && fields < 2 {
        s -= 1;
        while s > 0 && d[s].is_ascii_whitespace() {
            s -= 1;
        }
        while s > 0 && d[s - 1].is_ascii_digit() {
            s -= 1;
        }
        fields += 1;
    }
    let (_, _, o) = pdf_obj::object_at(d, s)?;
    match o {
        Obj::Dict(_) if o.get("ByteRange").is_some() => Some(o),
        // A signature dictionary inside a field (the /V value written
        // directly).
        Obj::Dict(_) => o.get("V").filter(|v| v.get("ByteRange").is_some()).cloned(),
        _ => None,
    }
}

/// The signature dictionaries of a PDF file with their byte ranges.
pub(crate) fn signatures(d: &[u8]) -> Vec<(Obj, Vec<i64>)> {
    let mut out = Vec::new();
    let mut seen: std::collections::HashSet<Vec<i64>> = std::collections::HashSet::new();
    for at in byte_ranges(d) {
        let Some(dict) = enclosing_dict(d, at) else { continue };
        let Some(Obj::Array(br)) = dict.get("ByteRange") else { continue };
        let br: Vec<i64> = br.iter().filter_map(Obj::int).collect();
        if br.len() != 4 || !seen.insert(br.clone()) {
            continue;
        }
        out.push((dict, br));
    }
    out
}

/// Whether a signature dictionary is a document time-stamp.
pub(crate) fn is_doc_timestamp(dict: &Obj) -> bool {
    dict.get("SubFilter").and_then(Obj::name) == Some("ETSI.RFC3161") || dict.get("Type").and_then(Obj::name) == Some("DocTimeStamp")
}

/// The DER in a signature dictionary's /Contents (without the padding).
pub(crate) fn contents(dict: &Obj) -> Option<Vec<u8>> {
    let Some(Obj::Str(c)) = dict.get("Contents") else { return None };
    crate::asn1::parse(c).ok().map(|t| t.raw.to_vec())
}

/// Checks every signature of a PDF file.
pub fn verify(d: &[u8]) -> Vec<Signature> {
    let found: Vec<(Signature, usize, Validation)> = signatures(d).iter().map(|(dict, br)| check(d, dict, br)).collect();
    let mut sigs = judge(d, found);
    // More signature dictionaries than are checked: one past them would go
    // unlisted, so the file shows as having a bad one.
    let keys = byte_ranges(d);
    if keys.len() == MAX_KEYS && key_after(d, keys[MAX_KEYS - 1] + 1) {
        let mut s = Signature::new(Kind::Pdf);
        s.intact = Check::Failed;
        s.note = Some(format!("more than {MAX_KEYS} signature dictionaries: the rest weren't checked"));
        sigs.push(s);
    }
    sigs
}

/// Whether another `/ByteRange` key comes after `from`.
fn key_after(d: &[u8], from: usize) -> bool {
    d.get(from..).is_some_and(|r| r.windows(10).any(|w| w == b"/ByteRange"))
}

/// Long-term validation: the DSS as the file ends, and as each document
/// time-stamp after a signature covered it.
fn judge(d: &[u8], found: Vec<(Signature, usize, Validation)>) -> Vec<Signature> {
    let kept_now = crate::pdf_ltv::dss_of(d);
    let stamps: Vec<(usize, i64, Option<i64>, Validation)> = found
        .iter()
        .filter(|(s, ..)| s.kind == Kind::PdfTimestamp && s.intact == Check::Ok && s.valid == Check::Ok && s.trust.is_trusted())
        .map(|(s, end, _)| (*end, s.time.unwrap_or(0), crate::ltv::chain_until(&s.chain), crate::pdf_ltv::dss_of(&d[..*end])))
        .collect();
    found
        .into_iter()
        .map(|(mut s, end, embedded)| {
            let mut kept = embedded.clone();
            kept.merge(&kept_now);
            let archive = stamps.iter().filter(|st| st.0 > end).max_by_key(|st| st.0).map(|st| {
                let mut covered = st.3.clone();
                covered.merge(&embedded);
                Archive { time: st.1, until: st.2, kept: covered }
            });
            s.judge(&kept, archive.as_ref());
            s
        })
        .collect()
}

/// Checks one signature: returns it, the end of the bytes it covers and
/// the validation data it carries itself.
fn check(d: &[u8], dict: &Obj, br: &[i64]) -> (Signature, usize, Validation) {
    let mut kept = Validation::default();
    let end = if br.len() == 4 { br[2].saturating_add(br[3]).max(0) as usize } else { 0 };
    let sig = check_one(d, dict, br, &mut kept);
    (sig, end, kept)
}

/// Whether the bytes the signature leaves out are exactly its /Contents:
/// one hex string, nothing else hidden there.
fn hole_is_contents(hole: &[u8], contents: Option<&Obj>) -> bool {
    let Some(Obj::Str(value)) = contents else { return false };
    let [b'<', hex @ .., b'>'] = hole else { return false };
    let digits: Vec<u8> = hex.iter().copied().filter(|x| !x.is_ascii_whitespace()).collect();
    if digits.len() % 2 != 0 || !digits.iter().all(u8::is_ascii_hexdigit) {
        return false;
    }
    let bytes: Vec<u8> = digits.chunks(2).map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap_or("0"), 16).unwrap_or(0)).collect();
    // The value may be shorter than the room kept for it (zeros after).
    bytes.len() >= value.len() && bytes[..value.len()] == value[..] && bytes[value.len()..].iter().all(|&x| x == 0)
}

fn check_one(d: &[u8], dict: &Obj, br: &[i64], kept: &mut Validation) -> Signature {
    let text = |k: &str| dict.get(k).and_then(Obj::text).filter(|s| !s.trim().is_empty());
    let sub = dict.get("SubFilter").and_then(Obj::name).unwrap_or("").to_string();
    let doc_ts = is_doc_timestamp(dict);
    let mut sig = Signature::new(if doc_ts { Kind::PdfTimestamp } else { Kind::Pdf });
    sig.format = match sub.as_str() {
        "ETSI.CAdES.detached" => "PAdES (CAdES)".into(),
        "ETSI.RFC3161" => "RFC 3161".into(),
        s if s.starts_with("adbe.pkcs7") => "PKCS #7".into(),
        s => s.to_string(),
    };
    sig.name = text("Name");
    sig.byte_range = br.to_vec();
    sig.reason = text("Reason");
    sig.location = text("Location");
    sig.time = text("M").and_then(|m| crate::time::parse_pdf(&m));
    // The covered bytes: all but the /Contents value.
    let (a, b, c, l) = (br[0], br[1], br[2], br[3]);
    let len = d.len() as i64;
    if a < 0 || b < 0 || l < 0 || a.checked_add(b).is_none_or(|ab| c < ab) || c.checked_add(l).is_none_or(|e| e > len) {
        sig.intact = Check::Failed;
        sig.note = Some("the byte range is outside the file".into());
        return sig;
    }
    let (a, b, c, l) = (a as usize, b as usize, c as usize, l as usize);
    let hole_ok = a == 0 && hole_is_contents(&d[a + b..c], dict.get("Contents"));
    let parts = [&d[a..a + b], &d[c..c + l]];
    // Later revisions: bytes past the covered end.
    // Validation data and document time-stamps (B-LT, B-LTA) don't count.
    sig.changed_after = d[c + l..].iter().any(|&x| !x.is_ascii_whitespace() && x != 0) && !crate::pdf_ltv::only_validation_after(d, c + l);
    let Some(Obj::Str(contents)) = dict.get("Contents") else {
        sig.note = Some("no signature value".into());
        return sig;
    };
    let token = match crate::asn1::parse(contents) {
        Ok(t) => t.raw.to_vec(),
        Err(e) => {
            sig.valid = Check::Failed;
            sig.note = Some(e.to_string());
            return sig;
        }
    };
    if doc_ts {
        match cms::stamp(&token) {
            Ok(st) => {
                sig.intact = if hole_ok && st.imprint == st.hash.digest_parts(&parts) { Check::Ok } else { Check::Failed };
                sig.valid = if st.valid { Check::Ok } else { Check::Failed };
                sig.time = Some(st.time);
                sig.timestamped = true;
                sig.cert = st.tsa;
                sig.certs = st.certs;
            }
            Err(e) => sig.note = Some(e.to_string()),
        }
        return sig.finish();
    }
    let sd = match cms::parse(&token) {
        Ok(sd) => sd,
        Err(e) => {
            sig.note = Some(e.to_string());
            return sig;
        }
    };
    let Some(signer) = sd.signers.first() else {
        sig.note = Some("no signer".into());
        return sig;
    };
    let hash = signer.hash.unwrap_or(Hash::Sha256);
    let checked = if sub == "adbe.pkcs7.sha1" {
        // The signed content is the SHA-1 digest of the covered bytes.
        let inner = sd.content.clone().unwrap_or_default();
        let c = sd.check(signer, &hash.digest(&inner));
        let matches = inner == Hash::Sha1.digest_parts(&parts);
        cms::Checked { digest_ok: c.digest_ok && matches, ..c }
    } else {
        sd.check(signer, &hash.digest_parts(&parts))
    };
    sig.intact = if hole_ok && checked.digest_ok { Check::Ok } else { Check::Failed };
    sig.valid = if checked.signature_ok { Check::Ok } else if checked.cert.is_some() { Check::Failed } else { Check::Unknown };
    sig.cert = checked.cert;
    sig.certs = sd.certs.clone();
    for c in sd.crls.iter().chain(&signer.crls) {
        kept.add_crl(c.clone());
    }
    for o in sd.ocsps.iter().chain(&signer.ocsps) {
        kept.add_ocsp(o.clone());
    }
    if sig.time.is_none() {
        sig.time = signer.signing_time;
    }
    // A time-stamp of the signature gives a trusted time, when its
    // authority is trusted to time-stamp (anyone can make one).
    if let Some(tok) = &signer.timestamp
        && let Ok(st) = cms::stamp(tok)
        && st.valid
        && st.imprint == st.hash.digest(&signer.signature)
        && let Some(tsa) = &st.tsa
    {
        let mut pool = st.certs.clone();
        pool.extend(sd.certs.iter().cloned());
        let (chain, trust) = crate::trust::check(tsa, &pool, st.time);
        if trust.is_trusted() && crate::trust::fit_for_timestamping(tsa) {
            sig.time = Some(st.time);
            sig.timestamped = true;
            sig.tsa_chain = chain;
            sig.tsa = st.tsa;
        } else {
            sig.note.get_or_insert_with(|| "its time-stamp is by an authority not trusted".into());
        }
    }
    sig.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hole_is_only_the_signature() {
        let v = Obj::Str(vec![0x30, 0x82]);
        assert!(hole_is_contents(b"<3082>", Some(&v)));
        assert!(hole_is_contents(b"<30820000>", Some(&v)));
        // Other bytes hidden where the signature goes.
        assert!(!hole_is_contents(b"<3082> /Annots [9 0 R] <00>", Some(&v)));
        assert!(!hole_is_contents(b"<30820001>", Some(&v)));
        assert!(!hole_is_contents(b"<3083>", Some(&v)));
        assert!(!hole_is_contents(b"(3082)", Some(&v)));
    }
}
