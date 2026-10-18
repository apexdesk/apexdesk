//! PAdES B-LT and B-LTA: the validation data of a PDF's signatures (the
//! certificates, OCSP answers and CRLs) in a Document Security Store
//! added by an incremental update, and a document time-stamp over all of
//! it. Doing it again on a B-LTA file keeps the last time-stamp's own
//! validation data and stamps anew, which renews the archive.

use apex_io::pdf::{PdfFile, PdfObj};

use crate::Error;
use crate::cms;
use crate::ltv::{self, Level, Validation};
use crate::net::Net;
use crate::pdf_obj::{self, Obj};
use crate::x509::Cert;

/// The validation data in the DSS of `data`'s last revision.
pub(crate) fn dss_of(data: &[u8]) -> Validation {
    let mut v = Validation::default();
    let Ok(f) = PdfFile::open(data, None) else { return v };
    let Some(dss) = f.catalog().and_then(|c| f.key_dict(&c, "DSS")) else { return v };
    let streams = |key: &str| -> Vec<Vec<u8>> {
        match f.key(&dss, key) {
            PdfObj::Array(a) => a
                .iter()
                .filter_map(|o| match f.resolve(o) {
                    PdfObj::Stream(d, raw) => Some(f.decode(&d, &raw).data),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    };
    for d in streams("Certs") {
        if let Ok(c) = Cert::from_der(&d) {
            v.add_cert(&c);
        }
    }
    for d in streams("OCSPs") {
        v.add_ocsp(d);
    }
    for d in streams("CRLs") {
        v.add_crl(d);
    }
    v
}

/// Extends the signatures of `pdf` to `level` (B-LT or B-LTA), asking
/// through `net`; `tsa` makes the document time-stamp.
/// With `newest_only`, only the last signature's (the one just made:
/// others in the file are anyone's, and their certificates' addresses too).
pub(crate) fn extend(pdf: &[u8], level: Level, tsa: Option<&str>, net: &dyn Net, newest_only: bool) -> Result<Vec<u8>, Error> {
    if level < Level::Lt {
        return Err(Error::new("a PDF signature can only be extended to B-LT or B-LTA"));
    }
    let sigs = crate::pdf::signatures(pdf);
    if sigs.is_empty() {
        return Err(Error::new("the file isn't signed"));
    }
    // Every certificate to check: the signers', the time-stamp
    // authorities' (of signatures and of document time-stamps).
    let mut leaves: Vec<(Cert, Vec<Cert>)> = Vec::new();
    let mut have = dss_of(pdf);
    let mut untimed = false;
    let newest = sigs.iter().map(|(_, br)| br[2].saturating_add(br[3])).max();
    for (dict, br) in &sigs {
        if newest_only && Some(br[2].saturating_add(br[3])) != newest {
            continue;
        }
        let Some(token) = crate::pdf::contents(dict) else { continue };
        if crate::pdf::is_doc_timestamp(dict) {
            if let Ok(st) = cms::stamp(&token)
                && let Some(tsa) = st.tsa
            {
                leaves.push((tsa, st.certs));
            }
            continue;
        }
        let Ok(sd) = cms::parse(&token) else { continue };
        let Some(si) = sd.signers.first() else { continue };
        for c in sd.crls.iter().chain(&si.crls) {
            have.add_crl(c.clone());
        }
        for o in sd.ocsps.iter().chain(&si.ocsps) {
            have.add_ocsp(o.clone());
        }
        if let Some(cert) = sd.cert_of(si) {
            leaves.push((cert.clone(), sd.certs.clone()));
        }
        match si.timestamp.as_deref().map(cms::stamp) {
            Some(Ok(st)) => {
                if let Some(tsa) = st.tsa {
                    let mut pool = st.certs;
                    pool.extend(sd.certs.iter().cloned());
                    leaves.push((tsa, pool));
                }
            }
            _ => untimed = true,
        }
    }
    if leaves.is_empty() {
        return Err(Error::new("no signature of the file can be extended"));
    }
    let new = ltv::gather(net, &leaves, &have)?;
    let mut out = if new.is_empty() { pdf.to_vec() } else { add_dss(pdf, &new)? };
    // A signature without a time-stamp gets its time proven by a document
    // time-stamp; B-LTA always ends with one.
    if level == Level::Lta || untimed {
        let url = tsa.ok_or_else(|| Error::new("a time-stamp server is needed for this level"))?;
        out = crate::pdf_sign::doc_timestamp(&out, url, net)?;
    }
    Ok(out)
}

/// Whether the revisions after byte `end` only add validation data,
/// document time-stamps and signatures: a DSS, its streams, time-stamp
/// fields, empty signature fields given their value, and the catalog, form
/// and page changed only where those go.
pub(crate) fn only_validation_after(d: &[u8], end: usize) -> bool {
    // Revision by revision (each ends with %%EOF).
    let mut from = end;
    let mut ends = Vec::new();
    let mut i = end;
    while let Some(p) = d[i..].windows(5).position(|w| w == b"%%EOF") {
        i += p + 5;
        ends.push(i);
    }
    if ends.is_empty() || d[*ends.last().unwrap_or(&end)..].iter().any(|&x| !x.is_ascii_whitespace() && x != 0) {
        return false;
    }
    for e in ends {
        if !revision_harmless(&d[..e], from) {
            return false;
        }
        from = e;
    }
    true
}

fn revision_harmless(d: &[u8], end: usize) -> bool {
    let (Some(now), Some(before)) = (pdf_obj::read_xref(d), pdf_obj::read_xref(&d[..end])) else { return false };
    let old_part = &d[..end];
    // Tables where they are said to be, as the file is read here (else
    // it is repaired: read by scanning, where other objects can win).
    if !now.exact {
        return false;
    }
    // Nor a signed part that readers take otherwise than here (tables
    // misplaced, a stream besides them, freed objects still defined): a
    // revision rewriting an object as read here could bring back another.
    if crate::pdf_sign::refused(&before).is_some() {
        return false;
    }
    // Every object after `end` must be one the new tables list.
    let listed = now.offsets.values().filter(|(off, _)| *off >= end).count();
    let written = d[end..].windows(6).filter(|w| w == b"endobj").count();
    if now.start < end || listed != written || headers(&d[end..]) != listed || scanned_headers(&d[end..]) != listed {
        return false;
    }
    // Readers that follow a cross-reference stream as well would see
    // objects these tables don't list.
    if d[end..].windows(7).any(|w| w == b"XRefStm") || now.trailer.get("XRefStm").is_some() {
        return false;
    }
    // Every object listed before still listed: one left out would be
    // looked for through the file, where a later one by its number wins.
    if before.offsets.keys().any(|n| !now.offsets.contains_key(n)) {
        return false;
    }
    // No object taken away, and the same catalog, encryption and document
    // information (a dictionary written in the trailer is no object).
    if now.freed.iter().any(|&(_, at)| at >= end) || ["Root", "Encrypt", "Info"].iter().any(|k| now.trailer.get(k) != before.trailer.get(k)) {
        return false;
    }
    // Nor the file's first identifier (an encrypted file's key comes from
    // it); the second is the revision's own.
    let first_id = |x: &pdf_obj::Xref| match x.trailer.get("ID") {
        Some(Obj::Array(a)) => a.first().cloned(),
        other => other.cloned(),
    };
    if first_id(&now) != first_id(&before) {
        return false;
    }
    // Nor one pointed, by the new tables, at another signed object (its
    // number would then show what that one holds).
    if now.offsets.iter().any(|(n, v)| v.0 < end && before.offsets.get(n) != Some(v)) {
        return false;
    }
    let old_of = |n: u32| {
        let &(off, _) = before.offsets.get(&n)?;
        pdf_obj::object_and_kind_at(old_part, off).filter(|o| o.0 == n).map(|(_, _, o, s)| (o, s))
    };
    let changed: Vec<(u32, usize)> = now.offsets.iter().filter(|(_, (off, _))| *off >= end).map(|(&n, &(off, _))| (n, off)).collect();
    let added: Vec<u32> = changed.iter().filter(|(n, _)| !before.offsets.contains_key(n)).map(|(n, _)| *n).collect();
    // A new object the signed part already points at (left dangling when
    // signed, to be filled in later) would show there.
    if !added.is_empty() && {
        let refs = references(old_part);
        added.iter().any(|n| refs.contains(n))
    } {
        return false;
    }
    // Lists (a page's annotations, an array of them) may gain only new
    // signature widgets: any other new object there could be drawn.
    // New signatures and time-stamps: each signs its own revision, to its
    // %%EOF and the line end after it (one
    // that doesn't, or reuses another's byte range, would pass for a
    // signature it isn't).
    let signed: Vec<(u32, bool)> = changed
        .iter()
        .filter(|(n, _)| added.contains(n))
        .filter_map(|&(n, off)| {
            let (m, _, o, s) = pdf_obj::object_and_kind_at(d, off)?;
            let kind = o.get("Type").and_then(Obj::name);
            (m == n && !s && matches!(kind, Some("Sig" | "DocTimeStamp"))).then_some((n, kind == Some("Sig")))
        })
        .collect();
    let covers = |o: &Obj| {
        let r: Vec<i64> = match o.get("ByteRange") {
            Some(Obj::Array(r)) => r.iter().filter_map(Obj::int).collect(),
            _ => Vec::new(),
        };
        o.get("Contents").is_some() && r.len() == 4 && r[0] == 0 && r[1] > 0 && r[2] > r[1] && r[3] >= 0 && r[2].checked_add(r[3]).is_some_and(|e| (0..=2).contains(&(e - d.len() as i64)))
    };
    // The only value an empty field may get.
    let sigs: Vec<u32> = signed.iter().filter(|s| s.1).map(|s| s.0).collect();
    let all_sigs: Vec<u32> = signed.iter().map(|s| s.0).collect();
    // Lists (a page's annotations, an array of them) may gain only new
    // signature widgets, whose value (if any) is a new signature.
    let widgets: Vec<u32> = changed
        .iter()
        .filter(|(n, _)| added.contains(n))
        .filter_map(|&(n, off)| {
            let (m, _, o, s) = pdf_obj::object_and_kind_at(d, off)?;
            let value_ok = o.get("V").is_none_or(|v| v.reference().is_some_and(|(k, _)| all_sigs.contains(&k)));
            (m == n && sig_widget(&o, s) && value_ok).then_some(n)
        })
        .collect();
    changed.iter().all(|&(n, off)| {
        let Some((m, g, new, new_stream)) = pdf_obj::object_and_kind_at(d, off) else { return false };
        // The generation the tables give, and the one it had (0 for a
        // new object): references by the old one would find none else.
        if now.offsets.get(&n).map(|e| e.1) != Some(g) || before.offsets.get(&n).map_or(0, |e| e.1) != g {
            return false;
        }
        let old = old_of(n);
        // An object listed before but not read here (another at its offset,
        // or written as only other readers take it) is no new one.
        if old.is_none() && before.offsets.contains_key(&n) {
            return false;
        }
        if all_sigs.contains(&n) && !covers(&new) {
            return false;
        }
        // An array that may gain widgets is a list of annotations or
        // fields: not the pages' (a widget there would be another page).
        if let Some((Obj::Array(a), _)) = &old {
            // Each of its items an annotation or a field (a page needn't
            // say it is one).
            let page_node = |x: &Obj| {
                x.reference().and_then(|(k, _)| old_of(k)).is_none_or(|(o, _)| {
                    matches!(o.get("Type").and_then(Obj::name), Some("Page" | "Pages")) || ["Subtype", "FT", "T"].iter().all(|k| o.get(k).is_none())
                })
            };
            if a.is_empty() || a.iter().any(page_node) {
                return false;
            }
        }
        m == n && allowed_change(&new, new_stream, old.as_ref().map(|(o, s)| (o, *s)), &widgets, &sigs)
    })
}

/// The objects `d` refers to: as the document's reader reads them (inside
/// object streams too), and "n G R" anywhere in the bytes, separated by
/// white space, NULs or comments.
pub(crate) fn references(d: &[u8]) -> std::collections::HashSet<u32> {
    let mut out = std::collections::HashSet::new();
    if let Ok(f) = apex_io::pdf::PdfFile::open(d, None) {
        fn walk(o: &apex_io::pdf::PdfObj, out: &mut std::collections::HashSet<u32>) {
            use apex_io::pdf::PdfObj as O;
            match o {
                O::Ref(n, _) => {
                    out.insert(*n);
                }
                O::Array(a) => a.iter().for_each(|o| walk(o, out)),
                O::Dict(d) | O::Stream(d, _) => d.0.iter().for_each(|(_, o)| walk(o, out)),
                _ => {}
            }
        }
        walk(&apex_io::pdf::PdfObj::Dict(f.trailer.clone()), &mut out);
        for n in f.numbers() {
            walk(&f.get(n), &mut out);
        }
    }
    // Tokens, forwards: the last two integers before an `R`.
    let mut last: [Option<u32>; 2] = [None, None];
    let mut i = 0;
    while i < d.len() {
        let c = d[i];
        if c.is_ascii_whitespace() || c == 0 || c == 0x0c {
            i += 1;
        } else if c == b'%' {
            while i < d.len() && !matches!(d[i], b'\r' | b'\n') {
                i += 1;
            }
        } else if c.is_ascii_digit() {
            let s = i;
            while i < d.len() && d[i].is_ascii_digit() {
                i += 1;
            }
            let n = std::str::from_utf8(&d[s..i]).ok().and_then(|t| t.parse::<u32>().ok());
            last = [last[1], n];
        } else if c == b'R' && !d.get(i + 1).is_some_and(u8::is_ascii_alphanumeric) {
            if let [Some(n), Some(_)] = last {
                out.insert(n);
            }
            last = [None, None];
            i += 1;
        } else {
            last = [None, None];
            i += 1;
        }
    }
    out
}

/// How many objects `d` starts ("N G obj"), outside streams' data: an
/// object written without its `endobj` counts too.
/// The object headers a reader repairing the file would find in `d`
/// ("n g obj" anywhere, stream data included, as apex-io's scan looks):
/// one hidden in a stream would be what such a reader shows by that number.
fn scanned_headers(d: &[u8]) -> usize {
    let mut n = 0;
    for at in d.windows(3).enumerate().filter(|(_, w)| *w == b"obj").map(|(i, _)| i) {
        let mut i = at;
        let mut fields = 0;
        let mut start = at;
        while i > 0 && fields < 2 {
            while i > 0 && matches!(d[i - 1], b' ' | b'\r' | b'\n' | b'\t') {
                i -= 1;
            }
            let e = i;
            while i > 0 && d[i - 1].is_ascii_digit() {
                i -= 1;
            }
            if i == e {
                break;
            }
            fields += 1;
            start = i;
        }
        if fields == 2 && d.get(at + 3).is_none_or(|&b| !b.is_ascii_alphanumeric()) && (start == 0 || !d[start - 1].is_ascii_alphanumeric()) {
            n += 1;
        }
    }
    n
}

fn headers(d: &[u8]) -> usize {
    let mut n = 0;
    let mut i = 0;
    while i < d.len() {
        if d[i..].starts_with(b"endstream") {
            i += 9;
        } else if d[i..].starts_with(b"stream") {
            // Its data, up to its end.
            i = d[i..].windows(9).position(|w| w == b"endstream").map_or(d.len(), |p| i + p + 9);
        } else if d[i..].starts_with(b"endobj") {
            i += 6;
        } else if d[i..].starts_with(b"obj") {
            // After a number, a space, a number.
            let before = &d[..i];
            let t = before.trim_ascii_end();
            if t.len() < before.len() && t.last().is_some_and(u8::is_ascii_digit) {
                n += 1;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    n
}

/// Whether `o` is a DSS: its type, or no keys but a DSS's.
fn is_dss(o: &Obj) -> bool {
    let Obj::Dict(d) = o else { return false };
    let typed = o.get("Type").and_then(Obj::name);
    (typed.is_none() || typed == Some("DSS")) && d.iter().any(|(k, _)| k != "Type") && d.iter().all(|(k, _)| matches!(k.as_str(), "Type" | "Certs" | "OCSPs" | "CRLs" | "VRI"))
}

/// `new` (a stream when `new_stream`) in place of `old` (with whether it
/// was a stream; `None` for a new object). `added`: the signature widgets
/// the revision makes; `sigs`, its new signature dictionaries. Nothing
/// that can change what is shown passes.
fn allowed_change(new: &Obj, new_stream: bool, old: Option<(&Obj, bool)>, added: &[u32], sigs: &[u32]) -> bool {
    let kind = new.get("Type").and_then(Obj::name);
    // Changing a stream's data or turning an object into a stream (or
    // back) changes what it is.
    if old.is_some_and(|(_, s)| s) || (new_stream && old.is_some()) {
        return false;
    }
    match old {
        // A document time-stamp's signature dictionary is always new.
        // Only a signature's keys: one that is also an annotation
        // (`/Subtype /FreeText`) would be drawn.
        // So is a signature's.
        None if matches!(kind, Some("DocTimeStamp" | "Sig")) => {
            !new_stream
                && matches!(new, Obj::Dict(d) if d.iter().all(|(k, _)| matches!(k.as_str(), "Type" | "Filter" | "SubFilter" | "Contents" | "ByteRange" | "Reference" | "Changes" | "Name" | "M" | "Location" | "Reason" | "ContactInfo" | "R" | "V" | "Prop_Build" | "Prop_AuthTime" | "Prop_AuthType")))
        }
        // An empty signature field (one prepared to be signed) given a new
        // signature as its value, and nothing else.
        Some((o @ Obj::Dict(b), _)) if o.get("FT").and_then(Obj::name) == Some("Sig") && o.get("V").is_none() && new.get("V").is_some() => {
            let Obj::Dict(a) = new else { return false };
            new.get("V").and_then(Obj::reference).is_some_and(|(n, _)| sigs.contains(&n)) && a.iter().chain(b).all(|(k, _)| k == "V" || new.get(k) == o.get(k))
        }
        // A DSS: new, or the file's DSS with more data.
        _ if is_dss(new) => !new_stream && old.is_none_or(|(o, _)| is_dss(o)),
        // A time-stamp's field and widget, which draws nothing.
        None if kind == Some("Annot") => sig_widget(new, new_stream),
        // A stream of validation data (certificates, OCSP and CRL answers).
        None => new_stream && matches!(new, Obj::Dict(d) if d.iter().all(|(k, _)| matches!(k.as_str(), "Length" | "Filter" | "DecodeParms"))),
        // The catalog, the form or a page: only where those go.
        Some((o @ Obj::Dict(b), _)) => {
            let Obj::Dict(a) = new else { return false };
            let free = |k: &str| matches!(k, "DSS" | "Extensions" | "SigFlags");
            a.iter().chain(b).all(|(k, _)| match k.as_str() {
                k if free(k) => true,
                "Annots" | "Fields" => appended(new.get(k), o.get(k), added),
                "AcroForm" => form_ok(new.get(k), o.get(k), added),
                k => new.get(k) == o.get(k),
            })
        }
        // Annotations or fields added to a list.
        Some((Obj::Array(b), _)) => matches!(new, Obj::Array(a) if a.starts_with(b) && a[b.len()..].iter().all(|x| x.reference().is_some_and(|(n, _)| added.contains(&n)))),
        _ => false,
    }
}

/// A signature's or time-stamp's field and widget that draws nothing: an
/// empty rectangle, no appearance and only a field's keys (readers may
/// draw one without a box, or make one from the others).
fn sig_widget(o: &Obj, stream: bool) -> bool {
    let Obj::Dict(d) = o else { return false };
    !stream
        && o.get("Type").and_then(Obj::name) == Some("Annot")
        && o.get("FT").and_then(Obj::name) == Some("Sig")
        && o.get("Subtype").and_then(Obj::name) == Some("Widget")
        && d.iter().all(|(k, _)| matches!(k.as_str(), "Type" | "Subtype" | "FT" | "T" | "TU" | "TM" | "V" | "F" | "Ff" | "Rect" | "P"))
        && invisible(o)
}

/// A widget with an empty rectangle.
fn invisible(w: &Obj) -> bool {
    match w.get("Rect") {
        Some(Obj::Array(r)) if r.len() == 4 => {
            let v: Vec<f64> = r.iter().map(|x| if let Obj::Num(n) = x { n.parse().unwrap_or(f64::NAN) } else { f64::NAN }).collect();
            (v[0] - v[2]).abs() < 1e-9 || (v[1] - v[3]).abs() < 1e-9
        }
        _ => false,
    }
}

/// A page's annotations: the old ones, then only objects this revision
/// makes (each is checked by itself).
fn appended(new: Option<&Obj>, old: Option<&Obj>, added: &[u32]) -> bool {
    let items = |o: Option<&Obj>| match o {
        None => Some(Vec::new()),
        Some(Obj::Array(a)) => Some(a.clone()),
        _ => None,
    };
    match (items(new), items(old)) {
        (Some(a), Some(b)) => a.starts_with(&b) && a[b.len()..].iter().all(|x| x.reference().is_some_and(|(n, _)| added.contains(&n))),
        // A list kept apart: the same one (its object is checked itself).
        _ => new == old,
    }
}

/// The catalog's form in place: only its flags may change, and its
/// fields only by new signature fields after the old ones.
fn form_ok(new: Option<&Obj>, old: Option<&Obj>, added: &[u32]) -> bool {
    match (new, old) {
        (Some(n @ Obj::Dict(a)), o) => {
            let b: &[(String, Obj)] = match o {
                Some(Obj::Dict(b)) => b,
                None => &[],
                _ => return false,
            };
            a.iter().chain(b).all(|(k, _)| match k.as_str() {
                "SigFlags" => true,
                "Fields" => appended(n.get(k), o.and_then(|o| o.get(k)), added),
                k => n.get(k) == o.and_then(|o| o.get(k)),
            })
        }
        _ => new == old,
    }
}

fn stream(data: &[u8]) -> Vec<u8> {
    let mut out = format!("<</Length {}>>\nstream\n", data.len()).into_bytes();
    out.extend_from_slice(data);
    out.extend_from_slice(b"\nendstream");
    out
}

/// Adds `new` to the file's DSS (made if it has none) in an incremental
/// update.
fn add_dss(pdf: &[u8], new: &Validation) -> Result<Vec<u8>, Error> {
    let unsupported = |what: &str| Error::new(format!("this PDF can't be extended: {what}"));
    let xref = pdf_obj::read_xref(pdf).ok_or_else(|| unsupported("compressed cross-references"))?;
    if xref.trailer.get("Encrypt").is_some() {
        return Err(unsupported("it is encrypted"));
    }
    let mut next = xref.trailer.get("Size").and_then(Obj::int).ok_or_else(|| unsupported("no /Size"))? as u32;
    let (root_n, _) = xref.trailer.get("Root").and_then(Obj::reference).ok_or_else(|| unsupported("no catalog"))?;
    let (root_g, mut catalog) = xref.get(pdf, root_n).ok_or_else(|| unsupported("no catalog"))?;
    let (dss_n, dss_g, mut dss) = match catalog.get("DSS") {
        Some(Obj::Ref(n, _)) => {
            let (g, o) = xref.get(pdf, *n).ok_or_else(|| unsupported("broken DSS"))?;
            (*n, g, o)
        }
        other => {
            let d = match other {
                Some(d @ Obj::Dict(_)) => d.clone(),
                _ => Obj::Dict(vec![("Type".into(), Obj::Name("DSS".into()))]),
            };
            next += 1;
            (next - 1, 0, d)
        }
    };
    let mut objs: Vec<(u32, u16, Vec<u8>)> = Vec::new();
    let lists: [(&str, Vec<&[u8]>); 3] = [
        ("Certs", new.certs.iter().map(|c| c.der.as_slice()).collect()),
        ("OCSPs", new.ocsps.iter().map(Vec::as_slice).collect()),
        ("CRLs", new.crls.iter().map(Vec::as_slice).collect()),
    ];
    for (key, items) in lists {
        if items.is_empty() {
            continue;
        }
        let mut arr = match dss.get(key) {
            Some(Obj::Array(a)) => a.clone(),
            Some(Obj::Ref(n, _)) => match xref.get(pdf, *n) {
                Some((_, Obj::Array(a))) => a,
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        for data in items {
            objs.push((next, 0, stream(data)));
            arr.push(Obj::Ref(next, 0));
            next += 1;
        }
        dss.set(key, Obj::Array(arr));
    }
    objs.push((dss_n, dss_g, dss.to_bytes()));
    catalog.set("DSS", Obj::Ref(dss_n, dss_g));
    // The ETSI extension of PDF 1.7 that DSS and document time-stamps are.
    let esic = Obj::Dict(vec![("BaseVersion".into(), Obj::Name("1.7".into())), ("ExtensionLevel".into(), Obj::Num("5".into()))]);
    match catalog.get("Extensions").cloned() {
        None => catalog.set("Extensions", Obj::Dict(vec![("ESIC".into(), esic)])),
        Some(mut e @ Obj::Dict(_)) if e.get("ESIC").is_none() => {
            e.set("ESIC", esic);
            catalog.set("Extensions", e);
        }
        _ => {}
    }
    objs.push((root_n, root_g, catalog.to_bytes()));
    Ok(crate::pdf_sign::append_update(pdf, &xref, &objs, next)?.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-page file with classic cross-references: catalog 1, pages 2,
    /// page 3, its content stream 4.
    fn base() -> Vec<u8> {
        let objs: [&[u8]; 4] = [
            b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R >>",
            b"<< /Length 18 >>\nstream\n0 0 50 50 re f\n\nendstream",
        ];
        let mut d = b"%PDF-1.7\n".to_vec();
        let mut offs = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offs.push(d.len());
            d.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            d.extend_from_slice(o);
            d.extend_from_slice(b"\nendobj\n");
        }
        let start = d.len();
        d.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
        for o in offs {
            d.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        d.extend_from_slice(format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n").as_bytes());
        d
    }

    /// `d` with an incremental update writing `objs` (number, body).
    fn update(d: &[u8], objs: &[(u32, &str)]) -> Vec<u8> {
        let prev = pdf_obj::startxref(d).unwrap();
        let mut out = d.to_vec();
        let mut offs = Vec::new();
        for (n, body) in objs {
            offs.push((*n, out.len()));
            out.extend_from_slice(format!("{n} 0 obj\n{body}\nendobj\n").as_bytes());
        }
        let start = out.len();
        out.extend_from_slice(b"xref\n");
        for (n, off) in &offs {
            out.extend_from_slice(format!("{n} 1\n{off:010} 00000 n \n").as_bytes());
        }
        let size = objs.iter().map(|o| o.0 + 1).max().unwrap_or(5).max(5);
        out.extend_from_slice(format!("trailer\n<< /Size {size} /Root 1 0 R /Prev {prev} >>\nstartxref\n{start}\n%%EOF\n").as_bytes());
        out
    }

    #[test]
    fn validation_data_passes() {
        let d = base();
        let end = d.len();
        let dss = update(&d, &[(5, "<< /Length 3 >>\nstream\nabc\nendstream"), (6, "<< /Type /DSS /Certs [5 0 R] >>"), (1, "<< /Type /Catalog /Pages 2 0 R /DSS 6 0 R /Extensions << /ESIC << /BaseVersion /1.7 >> >> >>")]);
        assert!(only_validation_after(&dss, end));
        // A time-stamp's invisible widget on the page.
        let stamp = |sig: &str, value: &str| {
            let widget = format!("<< /Type /Annot /Subtype /Widget /FT /Sig /V {value} /Rect [0 0 0 0] >>");
            update(
                &dss,
                &[
                    (7, sig),
                    (8, &widget),
                    (3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Annots [8 0 R] >>"),
                    (1, "<< /Type /Catalog /Pages 2 0 R /DSS 6 0 R /AcroForm << /Fields [8 0 R] /SigFlags 3 >> >>"),
                ],
            )
        };
        // Its byte range made to reach the end of its revision.
        let sealed = |d: Vec<u8>| {
            let t = String::from_utf8(d).unwrap();
            let rest = t.len() as i64 - 2;
            t.replace("RRRRRRRRRR", &format!("{rest:010}")).into_bytes()
        };
        let ts = sealed(stamp("<< /Type /DocTimeStamp /ByteRange [0 1 2 RRRRRRRRRR] /Contents <00> >>", "7 0 R"));
        assert!(only_validation_after(&ts, end));
        // Not when the time-stamp doesn't sign its revision (or has
        // nothing to sign it with), nor when the widget's value is another.
        assert!(!only_validation_after(&stamp("<< /Type /DocTimeStamp /ByteRange [0 1 2 3] /Contents <00> >>", "7 0 R"), end));
        assert!(!only_validation_after(&sealed(stamp("<< /Type /DocTimeStamp /ByteRange [0 1 2 RRRRRRRRRR] >>", "7 0 R")), end));
        assert!(!only_validation_after(&sealed(stamp("<< /Type /DocTimeStamp /ByteRange [0 1 2 RRRRRRRRRR] /Contents <00> >>", "4 0 R")), end));
    }

    #[test]
    fn changes_dressed_as_validation_data_dont() {
        let d = base();
        let end = d.len();
        // The page's content stream replaced, typed as a DSS.
        let a = update(&d, &[(4, "<< /Type /DSS /Length 18 >>\nstream\n0 0 99 99 re f\n\nendstream")]);
        assert!(!only_validation_after(&a, end));
        // The same stream dictionary, other data.
        let b = update(&d, &[(4, "<< /Length 18 >>\nstream\n0 0 99 99 re f\n\nendstream")]);
        assert!(!only_validation_after(&b, end));
        // The page's dictionary turned into a DSS (its content gone).
        let c = update(&d, &[(3, "<< /Type /DSS /Certs [] >>")]);
        assert!(!only_validation_after(&c, end));
        // A visible "time-stamp" widget drawing on the page.
        let w = update(
            &d,
            &[
                (5, "<< /Length 3 >>\nstream\nabc\nendstream"),
                (6, "<< /Type /Annot /Subtype /Widget /FT /Sig /Rect [0 0 200 200] /AP << /N 5 0 R >> >>"),
                (3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Annots [6 0 R] >>"),
            ],
        );
        assert!(!only_validation_after(&w, end));
        // Nor one with an empty rectangle and an appearance (drawn
        // anyway by readers that don't scale it), however its keys are
        // spelt.
        for widget in ["<< /Type /Annot /Subtype /Widget /FT /Sig /Rect [0 0 0 0] /AP << /N 5 0 R >> >>", "<< /Type /Annot /Subtype /Widget /FT /Sig /Rect [0 0 200 200] /A#50 << /N 5 0 R >> >>"] {
            let w = update(&d, &[(5, "<< /Length 3 >>\nstream\nabc\nendstream"), (6, widget), (3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Annots [6 0 R] >>")]);
            assert!(!only_validation_after(&w, end), "{widget}");
        }
        // The document's information, written in the trailer.
        let i = String::from_utf8(update(&d, &[(5, "<< /Length 3 >>\nstream\nabc\nendstream")])).unwrap();
        let k = i.rfind("/Prev").unwrap();
        let i = format!("{}/Info << /Title (Other) >> {}", &i[..k], &i[k..]);
        assert!(!only_validation_after(i.as_bytes(), end));
        // Harmless data, but the table said to start at the line before it
        // (read here, while readers that want "xref" there repair the file).
        let ok = update(&d, &[(5, "<< /Length 3 >>\nstream\nabc\nendstream")]);
        assert!(only_validation_after(&ok, end));
        let t = String::from_utf8(ok).unwrap();
        let k = t.rfind("startxref\n").unwrap() + 10;
        let at: usize = t[k..].trim_end().trim_end_matches("%%EOF").trim().parse().unwrap();
        let off = format!("{}startxref\n{}\n%%EOF\n", &t[..k - 10], at - 1);
        assert!(!only_validation_after(off.as_bytes(), end));
        // A hybrid file's stream named in the new trailer, however spelt.
        let x = String::from_utf8(update(&d, &[(5, "<< /Length 3 >>\nstream\nabc\nendstream")])).unwrap();
        let k = x.rfind("/Prev").unwrap();
        let x = format!("{}/XRef#53tm 9 {}", &x[..k], &x[k..]);
        assert!(!only_validation_after(x.as_bytes(), end));
        // A stream listed before but not read here (its offset off by a
        // byte) given other data.
        let text = String::from_utf8(d.clone()).unwrap();
        let at = text.find("4 0 obj").unwrap();
        let shifted = text.replacen(&format!("{at:010} 00000 n"), &format!("{:010} 00000 n", at - 1), 1);
        assert_ne!(shifted, text);
        let s = update(shifted.as_bytes(), &[(4, "<< /Length 3 >>\nstream\nabc\nendstream")]);
        assert!(!only_validation_after(&s, shifted.len()));
        // An old object put among the page's annotations.
        let o = update(&d, &[(3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Annots [2 0 R] >>")]);
        assert!(!only_validation_after(&o, end));
        // An annotation typed as a time-stamp, put on the page.
        let f = update(
            &d,
            &[
                (5, "<< /Type /DocTimeStamp /Subtype /FreeText /Rect [0 0 200 200] /DA (/Helv 12 Tf) /Contents (90000) >>"),
                (3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Annots [5 0 R] >>"),
            ],
        );
        assert!(!only_validation_after(&f, end));
        // A plain new stream among the page's annotations.
        let s = update(&d, &[(5, "<< /Length 3 >>\nstream\nabc\nendstream"), (3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Annots [5 0 R] >>")]);
        assert!(!only_validation_after(&s, end));
        // A form with more than fields added (XFA).
        let x = update(&d, &[(1, "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [] /XFA 4 0 R >> >>")]);
        assert!(!only_validation_after(&x, end));
        // A time-stamp dictionary in place of an old object.
        let t = update(&d, &[(2, "<< /Type /DocTimeStamp /Contents <00> >>")]);
        assert!(!only_validation_after(&t, end));
        // The form's fields swapped for others (an unused object's).
        let g = update(&d, &[(1, "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [3 0 R] >> >>")]);
        assert!(!only_validation_after(&g, end));
        let k = update(&d, &[(5, "<< /Fields [2 0 R] >>"), (1, "<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>")]);
        assert!(!only_validation_after(&k, end));
    }
}
