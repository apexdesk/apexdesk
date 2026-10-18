//! `apexdesk sign` and `apexdesk verify`: digital signatures of OOXML
//! (docx, xlsx…), ODF (odt, ods…) and PDF files with a certificate file
//! (.p12, .pfx), the system's store or a card or token (PKCS #11); the
//! key is picked in [`super::sign_key`]. Passwords and PINs never go on
//! the command line, where other users could see them.

use std::path::{Path, PathBuf};

use apex_sign::{Check, Signature, Status, Trust};

use super::args::Parsed;
use super::show::json_str;
use super::{Fail, Res, emit, error, glob, sign_key, usage};

pub const SIGN_HELP: &str = "Usage: apexdesk sign FILE --cert CERT.p12 [options]
       apexdesk sign FILE --store [--cert-subject TEXT | --cert-sha1 HASH]
       apexdesk sign FILE --pkcs11 MODULE [--slot N] [--cert-label L] [...]
       apexdesk sign --store --list | --pkcs11 MODULE --list

Signs a document: docx, docm, xlsx and xlsm as Office does (XAdES), odt
and ods as LibreOffice does, and PDF files (PAdES). Signatures already in
the file are kept. Any X.509 certificate works, with its private key in a
certificate file (.p12, .pfx), in the system's store (Windows certificate
store, macOS keychains; on Linux, the NSS databases of Firefox,
Thunderbird and Chromium) or on a smart card, token or HSM through its
PKCS #11 module (OpenSC's opensc-pkcs11, p11-kit-proxy, a vendor's).

Options:
      --cert FILE          certificate file
      --password-file F    read the certificate's password from file F
      --password-env VAR   read it from the environment variable VAR
      --store              use a certificate of the system's store
      --pkcs11 MODULE      use a card or token through this PKCS #11 module
      --slot N             only the token in slot N
      --cert-label L       the certificate with this label on the token
      --cert-subject TEXT  the certificate whose subject contains TEXT
      --cert-sha1 HASH     the certificate with this SHA-1 fingerprint
      --pin-env VAR        read the token's PIN from the environment variable
      --list               list the certificates of the store or token
      --reason TEXT        the signer's comment (the reason in PDF files)
      --tsa URL            time-stamp the signature with this authority
                           (RFC 3161)
      --level LEVEL        b, t (with --tsa), lt (with the chain and the
                           revocation answers: PAdES B-LT, XAdES-XL) or lta
                           (and an archive time-stamp: B-LTA, XAdES-A);
                           from t up --tsa is needed
  -o, --output FILE        write the signed file here (default: FILE itself)
  -q, --quiet              do not print what was signed

Without --password-file or --password-env the password is asked for in the
terminal (without showing it), or read from the first line of the standard
input when it is not a terminal. A certificate without a password needs
none of them. A token's PIN is asked for the same way without --pin-env;
the system's store asks for what it needs itself. Without --cert-subject,
--cert-sha1 or --cert-label the only certificate fit for signing is used.
";

pub const VERIFY_HELP: &str = "Usage: apexdesk verify FILE... [options]

Checks the digital signatures of files (docx, xlsx, odt, ods, pdf...):
who signed, when, whether the content changed after signing and whether
the certificate is trusted by this system.

Options:
      --json          print JSON
  -p, --password PW   password of encrypted files

Exit status: 0 if every file is signed by someone and no signature is
broken or followed by changes (a certificate that isn't trusted is only
reported), 1 otherwise.
";

pub const EXTEND_HELP: &str = "Usage: apexdesk extend FILE --tsa URL [--level lt|lta] [options]

Adds to the signatures of a file what verifying them years from now
needs: the certificates' chains and the answers of their OCSP responders
or CRLs (level lt: PAdES B-LT, XAdES-XL), and an archive time-stamp over
all of it (level lta, the default: PAdES B-LTA, XAdES-A). Run again on a
file at lta, it renews the archive time-stamp before the authority's
certificate expires. The signatures must be valid; the answers are asked
for on the network (through the proxy of the environment, if any).

Options:
      --tsa URL        the time-stamp authority (RFC 3161)
      --level LEVEL    lt or lta (default)
  -o, --output FILE    write the file here (default: FILE itself)
  -q, --quiet          do not print the result
";

/// The `--level` given (b when none).
fn level_of(a: &Parsed, default: apex_sign::Level) -> Res<apex_sign::Level> {
    match a.value("level") {
        None => Ok(default),
        Some(l) => apex_sign::Level::parse(l).map_or_else(|| usage(format!("unknown level '{l}' (b, t, lt or lta)")), Ok),
    }
}

pub fn extend(a: &Parsed) -> Res<i32> {
    let [file] = a.pos.as_slice() else {
        return usage(if a.pos.is_empty() { "no file given".to_string() } else { format!("unexpected argument '{}'", a.pos[1]) });
    };
    let level = level_of(a, apex_sign::Level::Lta)?;
    if level < apex_sign::Level::Lt {
        return usage("--level is lt or lta");
    }
    let Some(tsa) = a.value("tsa") else { return usage("give --tsa URL") };
    let path = Path::new(file);
    let data = apex_fs::read(path).map_err(|e| Fail::Error(format!("{}: {e}", path.display())))?;
    let out_data = apex_sign::extend(&data, level, Some(tsa)).map_err(|e| Fail::Error(format!("{}: {e}", path.display())))?;
    let out = a.value("output").map(PathBuf::from).unwrap_or_else(|| path.to_path_buf());
    crate::common::write_out(&out, &out_data).map_err(|e| Fail::Error(format!("{}: {e}", out.display())))?;
    if !a.flag("quiet") {
        for s in std::panic::catch_unwind(|| apex_sign::verify(&out_data)).unwrap_or_default().iter().filter(|s| s.kind != apex_sign::Kind::PdfTimestamp) {
            let until = s.verifiable_until.map(|t| format!(", verifiable until {}", day(t))).unwrap_or_default();
            emit(&format!("{}: {} — {}{until}\n", out.display(), s.signer(), s.level_name()));
        }
    }
    Ok(0)
}

/// `2026-09-26`.
fn day(t: i64) -> String {
    let (y, mo, d, ..) = apex_sign::time::civil(t);
    format!("{y:04}-{mo:02}-{d:02}")
}

pub fn sign(a: &Parsed) -> Res<i32> {
    sign_key::check(a)?;
    if a.flag("list") {
        if a.value("cert").is_some() || !a.pos.is_empty() {
            return usage("--list goes with --store or --pkcs11, without a file");
        }
        return sign_key::list(a);
    }
    let [file] = a.pos.as_slice() else {
        return usage(if a.pos.is_empty() { "no file given".to_string() } else { format!("unexpected argument '{}'", a.pos[1]) });
    };
    let path = Path::new(file);
    let data = apex_fs::read(path).map_err(|e| Fail::Error(format!("{}: {e}", path.display())))?;
    let pdf = data.get(..1024).unwrap_or(&data).windows(5).any(|w| w == b"%PDF-");
    if !pdf && !apex_sign::can_sign_package(&data) {
        let why = if apex_io::is_encrypted(&data) { "encrypted files can't be signed" } else { "only docx, xlsx, odt, ods (and their macro and template kinds) and pdf files can be signed" };
        return error(format!("{}: {why}", path.display()));
    }
    let level = level_of(a, apex_sign::Level::B)?;
    if level >= apex_sign::Level::T && a.value("tsa").is_none() {
        return usage("--level t, lt and lta need --tsa");
    }
    let id = sign_key::signer(a)?;
    let opts = apex_sign::SignOptions { reason: a.value("reason").map(str::to_string).filter(|r| !r.trim().is_empty()), tsa_url: a.value("tsa").map(str::to_string), level, ..Default::default() };
    let signed = if pdf { apex_sign::sign_pdf(&data, id.as_ref(), &opts) } else { apex_sign::sign_package(&data, id.as_ref(), &opts) };
    let signed = signed.map_err(|e| Fail::Error(format!("{}: {e}", path.display())))?;
    let out = a.value("output").map(PathBuf::from).unwrap_or_else(|| path.to_path_buf());
    crate::common::write_out(&out, signed).map_err(|e| Fail::Error(format!("{}: {e}", out.display())))?;
    if !a.flag("quiet") {
        emit(&format!("{}: signed by {}\n", out.display(), id.certificate().subject.display()));
    }
    Ok(0)
}

fn status_name(s: Status) -> &'static str {
    match s {
        Status::Valid => "valid",
        Status::Untrusted => "untrusted",
        Status::ChangedAfter => "changed-after",
        Status::Unknown => "unknown",
        Status::Invalid => "invalid",
    }
}

fn trust_text(t: &Trust) -> String {
    match t {
        Trust::Trusted(root) => format!("trusted (root: {root})"),
        Trust::UnknownIssuer => "issuer not trusted".into(),
        Trust::SelfSigned => "self-signed".into(),
        Trust::WrongUse => "certificate not for signing documents".into(),
        Trust::Expired => "expired".into(),
        Trust::NotVerified => "not verified".into(),
        Trust::Revoked(t) => format!("revoked on {}", day(*t)),
    }
}

fn check_name(c: Check) -> &'static str {
    match c {
        Check::Ok => "yes",
        Check::Failed => "no",
        Check::Unknown => "unknown",
    }
}

/// `2026-09-26T12:00:00Z` (or without the zone for a signer's local time).
fn when(s: &Signature) -> Option<String> {
    let (y, mo, d, h, mi, sec) = apex_sign::time::civil(s.time?);
    Some(format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{sec:02}{}", if s.local_time { "" } else { "Z" }))
}

/// The facts of one signature: name and value (a string, or JSON as is).
pub(super) fn facts(s: &Signature) -> Vec<(&'static str, String, bool)> {
    let mut f = vec![("status", status_name(s.status()).to_string(), false), ("signer", s.signer(), false)];
    let kind = match s.kind {
        apex_sign::Kind::Ooxml => "ooxml",
        apex_sign::Kind::Odf => "odf",
        apex_sign::Kind::Pdf => "pdf",
        apex_sign::Kind::PdfTimestamp => "pdf-timestamp",
    };
    f.push(("kind", kind.into(), false));
    f.push(("format", s.format.clone(), false));
    if let Some(t) = when(s) {
        f.push(("time", t, false));
        f.push(("timestamped", s.timestamped.to_string(), true));
    }
    f.push(("intact", check_name(s.intact).into(), false));
    f.push(("signature_ok", check_name(s.valid).into(), false));
    f.push(("certificate", trust_text(&s.trust), false));
    f.push(("level", s.level_name(), false));
    let revocation = match s.revocation {
        apex_sign::Revocation::NotChecked => "not in the file".to_string(),
        apex_sign::Revocation::Good(t) => format!("not revoked as of {}", day(t)),
        apex_sign::Revocation::Revoked(t) => format!("revoked on {}", day(t)),
    };
    f.push(("revocation", revocation, false));
    if let Some(t) = s.verifiable_until {
        f.push(("verifiable_until", day(t), false));
    }
    if let Some(t) = s.archive_time {
        f.push(("archive_timestamp", day(t), false));
    }
    if let Some(c) = &s.cert {
        f.push(("subject", c.subject.full(), false));
        f.push(("issuer", c.issuer.full(), false));
        f.push(("serial", c.serial_hex(), false));
    }
    for (k, v) in [("reason", &s.reason), ("location", &s.location), ("note", &s.note)] {
        if let Some(v) = v.as_ref().filter(|v| !v.is_empty()) {
            f.push((k, v.clone(), false));
        }
    }
    for (k, v) in [("changed_parts", &s.changed_parts), ("unsigned_parts", &s.unsigned_parts)] {
        if !v.is_empty() {
            f.push((k, v.join(", "), false));
        }
    }
    f
}

pub fn verify(a: &Parsed) -> Res<i32> {
    if a.pos.is_empty() {
        return usage("no file given");
    }
    let json = a.flag("json");
    let mut failed = false;
    let mut blocks = Vec::new();
    for p in &a.pos {
        for f in glob::expand(p).map_err(Fail::Error)? {
            match signatures(&f, a) {
                Ok(sigs) => {
                    failed |= sigs.is_empty() || apex_sign::stamps_only(&sigs) || sigs.iter().any(|s| matches!(s.status(), Status::Invalid | Status::Unknown | Status::ChangedAfter));
                    blocks.push((f, sigs));
                }
                Err(Fail::Error(e) | Fail::Usage(e)) => {
                    super::say(&format!("apexdesk: {e}"));
                    failed = true;
                }
            }
        }
    }
    if json {
        let items: Vec<String> = blocks
            .iter()
            .map(|(f, sigs)| {
                let list: Vec<String> = sigs
                    .iter()
                    .map(|s| {
                        let fields: Vec<String> = facts(s).into_iter().map(|(k, v, raw)| format!("\"{k}\": {}", if raw { v } else { json_str(&v) })).collect();
                        format!("{{{}}}", fields.join(", "))
                    })
                    .collect();
                let inner = if list.is_empty() { String::new() } else { format!("\n    {}\n  ", list.join(",\n    ")) };
                format!("{{\n  \"file\": {},\n  \"signed\": {},\n  \"signatures\": [{inner}]\n}}", json_str(&f.display().to_string()), !sigs.is_empty())
            })
            .collect();
        emit(&if items.len() == 1 { format!("{}\n", items[0]) } else { format!("[\n{}\n]\n", items.join(",\n")) });
    } else {
        let mut out = String::new();
        for (i, (f, sigs)) in blocks.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            if sigs.is_empty() {
                out.push_str(&format!("{}: not signed\n", f.display()));
                continue;
            }
            out.push_str(&format!("{}: {} signature{}\n", f.display(), sigs.len(), if sigs.len() == 1 { "" } else { "s" }));
            // The document's own words (reason, names) on one line each:
            // a line break in them could draw a signature that isn't there.
            let line = apex_sign::x509::one_line;
            for s in sigs {
                let mut facts = facts(s).into_iter().map(|(k, v, x)| (k, line(&v), x));
                if let (Some(status), Some(signer)) = (facts.next(), facts.next()) {
                    out.push_str(&format!("  - {} ({})\n", signer.1, status.1));
                }
                for (k, v, _) in facts {
                    out.push_str(&format!("      {k}: {v}\n"));
                }
            }
        }
        emit(&out);
    }
    Ok(i32::from(failed))
}

/// The signatures of a file (decrypted with `--password` if needed).
pub(super) fn signatures(path: &Path, a: &Parsed) -> Res<Vec<Signature>> {
    let data = apex_fs::read(path).map_err(|e| Fail::Error(format!("{}: {e}", path.display())))?;
    if apex_io::is_encrypted(&data) {
        let Some(pw) = a.value("password") else { return error(format!("{}: encrypted (use --password)", path.display())) };
        let plain = apex_io::crypto::decrypt(&data, pw).map_err(|e| {
            let why = match e {
                apex_io::crypto::CryptoError::WrongPassword => "wrong password".to_string(),
                apex_io::crypto::CryptoError::Unsupported(m) => m,
            };
            Fail::Error(format!("{}: {why}", path.display()))
        })?;
        return Ok(std::panic::catch_unwind(|| apex_sign::verify(&plain)).unwrap_or_default());
    }
    Ok(std::panic::catch_unwind(|| apex_sign::verify(&data)).unwrap_or_default())
}
