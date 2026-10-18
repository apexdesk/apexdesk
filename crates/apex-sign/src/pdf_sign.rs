//! Signing a PDF file (PAdES baseline B-B, or B-T with a time-stamp; B-LT
//! and B-LTA are added on top by [`crate::pdf_ltv`]): an incremental
//! update adds an invisible signature field on the first page and its
//! signature dictionary, whose /Contents holds a detached CAdES signature
//! of every other byte of the file. Document time-stamps are made the
//! same way.

use crate::Error;
use crate::cms;
use crate::ltv::Level;
use crate::net::Net;
use crate::signer::{KeyKind, Signer};
use crate::pdf_obj::{self, Obj, Xref, text_str};

/// What goes with the signature.
#[derive(Clone, Debug, Default)]
pub struct SignOptions {
    pub reason: Option<String>,
    pub location: Option<String>,
    pub contact: Option<String>,
    /// A time-stamp authority's address (RFC 3161), for PAdES B-T.
    pub tsa_url: Option<String>,
    /// The signing time (Unix seconds); `None`: now.
    pub time: Option<i64>,
    /// How much to keep for verifying it later: from T up a time-stamp
    /// authority is needed.
    pub level: Level,
    /// An empty signature field of the file to sign (by its name), with
    /// its own look; `None`: a new invisible field on the first page.
    pub field: Option<String>,
}

impl SignOptions {
    /// The time-stamp authority, if one was given.
    pub fn tsa(&self) -> Option<&str> {
        self.tsa_url.as_deref().map(str::trim).filter(|u| !u.is_empty())
    }

    /// The level asked for: at least T with a time-stamp authority.
    pub fn level(&self) -> Level {
        if self.tsa().is_some() { self.level.max(Level::T) } else { self.level }
    }
}

/// Whether signing can add to `pdf` as it is: its cross-references are a
/// table (a file with compressed ones has to be written out whole first).
pub fn pdf_can_add_to(pdf: &[u8]) -> bool {
    pdf_obj::read_xref(pdf).is_some_and(|x| refused(&x).is_none())
}

/// Why a file's cross-references can't be added to, as other readers
/// would take them otherwise.
pub(crate) fn refused(xref: &pdf_obj::Xref) -> Option<&'static str> {
    // Tables not where the file says: readers repair such a file each
    // their own way, so what is signed may not be what they show.
    if !xref.exact {
        return Some("repaired cross-references");
    }
    if xref.hybrid {
        return Some("hybrid cross-references");
    }
    // An object a newer table frees but an older one defines: readers
    // see none, and rewriting the older one would bring it back.
    if xref.freed.iter().any(|(n, _)| xref.offsets.contains_key(n)) {
        return Some("freed objects");
    }
    None
}

/// Width reserved for each ByteRange number.
const BR_WIDTH: usize = 10;

/// Signs `pdf` with `signer`: returns the signed file.
pub fn sign_pdf(pdf: &[u8], signer: &dyn Signer, opts: &SignOptions) -> Result<Vec<u8>, Error> {
    sign_pdf_with(pdf, signer, opts, &crate::net::Http)
}

/// [`sign_pdf`] asking for time-stamps and revocation through `net`.
pub fn sign_pdf_with(pdf: &[u8], signer: &dyn Signer, opts: &SignOptions, net: &dyn Net) -> Result<Vec<u8>, Error> {
    let level = opts.level();
    let tsa = opts.tsa();
    if level >= Level::T && tsa.is_none() {
        return Err(Error::new("a time-stamp server is needed for this level"));
    }
    let hash = KeyKind::of(signer.certificate())?.hash();
    let signed = sign_with(pdf, signer, opts, &mut |sig| match tsa {
        Some(url) => crate::tsa::request_with(net, url, hash, &hash.digest(sig)).map(Some),
        None => Ok(None),
    })?;
    if level >= Level::Lt {
        return crate::pdf_ltv::extend(&signed, level, tsa, net, true);
    }
    Ok(signed)
}

/// [`sign_pdf`] with the time-stamping given (tests stamp locally).
pub fn sign_with(pdf: &[u8], signer: &dyn Signer, opts: &SignOptions, stamp: &mut dyn FnMut(&[u8]) -> Result<Option<Vec<u8>>, Error>) -> Result<Vec<u8>, Error> {
    let (cert, kind) = (signer.certificate(), KeyKind::of(signer.certificate())?);
    let now = opts.time.unwrap_or_else(crate::time::now);
    let chain_len: usize = signer.chain().iter().map(|c| c.der.len()).sum::<usize>() + cert.der.len();
    let room = chain_len + kind.max_sig_len() + 2048 + if opts.tsa().is_some() { 12_000 } else { 0 };
    let mut extra = vec![("M", text_str(&crate::time::pdf_date(now))), ("Name", text_str(&cert.subject.display()))];
    for (k, v) in [("Reason", &opts.reason), ("Location", &opts.location), ("ContactInfo", &opts.contact)] {
        if let Some(v) = v.as_ref().filter(|v| !v.trim().is_empty()) {
            extra.push((k, text_str(v)));
        }
    }
    let head = "/Type /Sig /Filter /Adobe.PPKLite /SubFilter /ETSI.CAdES.detached";
    add_signature(pdf, head, room, extra, opts.field.as_deref(), &mut |parts| cms::sign_detached(signer, &kind.hash().digest_parts(parts), stamp))
}

/// Adds a document time-stamp (ETSI.RFC3161) from the authority at `url`.
pub fn doc_timestamp(pdf: &[u8], url: &str, net: &dyn Net) -> Result<Vec<u8>, Error> {
    let hash = crate::crypto::Hash::Sha256;
    let head = "/Type /DocTimeStamp /Filter /Adobe.PPKLite /SubFilter /ETSI.RFC3161";
    add_signature(pdf, head, 16_000, Vec::new(), None, &mut |parts| crate::tsa::request_with(net, url, hash, &hash.digest_parts(parts)))
}

/// Adds a signature field whose dictionary starts with `head`, has `room`
/// bytes for /Contents and the `extra` entries; `fill` gets the bytes the
/// byte range covers and makes the contents. With `field` the file's
/// empty signature field of that name gets the signature instead.
fn add_signature(pdf: &[u8], head: &str, room: usize, extra: Vec<(&str, Obj)>, field: Option<&str>, fill: &mut dyn FnMut(&[&[u8]]) -> Result<Vec<u8>, Error>) -> Result<Vec<u8>, Error> {
    let unsupported = |what: &str| Error::new(format!("this PDF can't be signed: {what}"));
    let xref = pdf_obj::read_xref(pdf).ok_or_else(|| unsupported("compressed cross-references"))?;
    if let Some(why) = refused(&xref) {
        return Err(unsupported(why));
    }
    let trailer = &xref.trailer;
    if trailer.get("Encrypt").is_some() {
        return Err(unsupported("it is encrypted"));
    }
    let size = trailer.get("Size").and_then(Obj::int).ok_or_else(|| unsupported("no /Size"))? as u32;
    let (root_n, _) = trailer.get("Root").and_then(Obj::reference).ok_or_else(|| unsupported("no catalog"))?;
    let (root_g, mut catalog) = xref.get(pdf, root_n).ok_or_else(|| unsupported("no catalog"))?;
    // The first page, for the field's widget.
    let mut node = catalog.get("Pages").cloned().ok_or_else(|| unsupported("no pages"))?;
    let mut page = None;
    for _ in 0..32 {
        let Some((n, _)) = node.reference() else { break };
        let Some((g, o)) = xref.get(pdf, n) else { break };
        if o.get("Type").and_then(Obj::name) == Some("Page") {
            page = Some((n, g, o));
            break;
        }
        match o.get("Kids") {
            Some(Obj::Array(k)) if !k.is_empty() => node = k[0].clone(),
            _ => break,
        }
    }
    let (page_n, page_g, mut page_obj) = page.ok_or_else(|| unsupported("no pages"))?;
    // New numbers past any the file already names: a field left pointing
    // at the next one would otherwise get this signature as its value.
    let size = crate::pdf_ltv::references(pdf).into_iter().map(|n| n.saturating_add(1)).max().unwrap_or(0).max(size);
    let sig_n = size;
    let field_n = size + 1;
    let field_ref = Obj::Ref(field_n, 0);
    let mut changed: Vec<(u32, u16, Obj)> = Vec::new();
    if let Some(name) = field {
        // The form's own field: it only gets its value.
        let form_n = match catalog.get("AcroForm") {
            Some(Obj::Ref(n, _)) => Some(*n),
            _ => None,
        };
        let mut form = match (form_n, catalog.get("AcroForm")) {
            (Some(n), _) => xref.get(pdf, n).ok_or_else(|| unsupported("broken form"))?,
            (None, Some(f @ Obj::Dict(_))) => (0, f.clone()),
            _ => return Err(unsupported("no form")),
        };
        let refs = match form.1.get("Fields") {
            Some(Obj::Array(f)) => f.clone(),
            _ => return Err(unsupported("its form fields are stored apart")),
        };
        // A field's own name has no periods (they join a field's to its
        // parent's): one with them would stand for another field's full
        // name. And the name must pick one field.
        if name.contains('.') {
            return Err(Error::new(format!("no signature field \"{name}\"")));
        }
        let mut found = refs.iter().filter_map(Obj::reference).filter_map(|(n, _)| {
            let (g, o) = xref.get(pdf, n)?;
            (o.get("T").and_then(Obj::text).as_deref() == Some(name)).then_some((n, g, o))
        });
        let (Some((fn_, fg, mut fo)), None) = (found.next(), found.next()) else { return Err(Error::new(format!("no signature field \"{name}\""))) };
        if fo.get("FT").and_then(Obj::name) != Some("Sig") || fo.get("V").is_some() {
            return Err(Error::new(format!("\"{name}\" isn't an empty signature field")));
        }
        fo.set("V", Obj::Ref(sig_n, 0));
        changed.push((fn_, fg, fo));
        form.1.set("SigFlags", Obj::Num("3".into()));
        match form_n {
            Some(n) => changed.push((n, form.0, form.1)),
            None => {
                catalog.set("AcroForm", form.1);
                changed.push((root_n, root_g, catalog));
            }
        }
        return finish(pdf, &xref, changed, sig_n, size + 1, head, room, extra, fill);
    }
    // The form: a new one, or the file's own with the field added.
    let mut field_count = 0;
    let mut add_field = |form: &mut Obj| {
        if let Some(Obj::Array(f)) = form.get("Fields") {
            field_count = f.len();
        }
        let mut fields = match form.get("Fields") {
            Some(Obj::Array(f)) => f.clone(),
            _ => Vec::new(),
        };
        fields.push(field_ref.clone());
        form.set("Fields", Obj::Array(fields));
        form.set("SigFlags", Obj::Num("3".into()));
    };
    match catalog.get("AcroForm").cloned() {
        Some(Obj::Ref(n, _)) => {
            let (g, mut form) = xref.get(pdf, n).ok_or_else(|| unsupported("broken form"))?;
            if let Some(Obj::Ref(..)) = form.get("Fields") {
                return Err(unsupported("its form fields are stored apart"));
            }
            add_field(&mut form);
            changed.push((n, g, form));
        }
        Some(mut form @ Obj::Dict(_)) => {
            if let Some(Obj::Ref(..)) = form.get("Fields") {
                return Err(unsupported("its form fields are stored apart"));
            }
            add_field(&mut form);
            catalog.set("AcroForm", form);
        }
        _ => {
            let mut form = Obj::Dict(Vec::new());
            add_field(&mut form);
            catalog.set("AcroForm", form);
        }
    }
    changed.push((root_n, root_g, catalog));
    // The page lists the widget among its annotations.
    match page_obj.get("Annots").cloned() {
        Some(Obj::Ref(n, _)) => {
            let (g, a) = xref.get(pdf, n).ok_or_else(|| unsupported("broken annotations"))?;
            let mut items = if let Obj::Array(v) = a { v } else { Vec::new() };
            items.push(field_ref.clone());
            changed.push((n, g, Obj::Array(items)));
        }
        Some(Obj::Array(mut v)) => {
            v.push(field_ref.clone());
            page_obj.set("Annots", Obj::Array(v));
            changed.push((page_n, page_g, page_obj));
        }
        _ => {
            page_obj.set("Annots", Obj::Array(vec![field_ref.clone()]));
            changed.push((page_n, page_g, page_obj));
        }
    }
    let field = Obj::Dict(vec![
        ("Type".into(), Obj::Name("Annot".into())),
        ("Subtype".into(), Obj::Name("Widget".into())),
        ("FT".into(), Obj::Name("Sig".into())),
        ("T".into(), text_str(&format!("Signature{}", field_count + 1))),
        ("V".into(), Obj::Ref(sig_n, 0)),
        ("F".into(), Obj::Num("132".into())),
        ("Rect".into(), Obj::Array(vec![Obj::Num("0".into()); 4])),
        ("P".into(), Obj::Ref(page_n, page_g)),
    ]);
    changed.push((field_n, 0, field));
    finish(pdf, &xref, changed, sig_n, size + 2, head, room, extra, fill)
}

/// Writes the update with the `changed` objects and the signature
/// dictionary as object `sig_n`, then signs it.
#[allow(clippy::too_many_arguments)]
fn finish(pdf: &[u8], xref: &Xref, mut changed: Vec<(u32, u16, Obj)>, sig_n: u32, size: u32, head: &str, room: usize, extra: Vec<(&str, Obj)>, fill: &mut dyn FnMut(&[&[u8]]) -> Result<Vec<u8>, Error>) -> Result<Vec<u8>, Error> {
    // The signature dictionary, with room for the signature.
    let mut sig_dict = Vec::new();
    sig_dict.extend_from_slice(format!("<<{head} /ByteRange [").as_bytes());
    let br_at = sig_dict.len();
    sig_dict.extend(std::iter::repeat_n(b' ', BR_WIDTH * 4 + 3));
    sig_dict.extend_from_slice(b"] /Contents <");
    let contents_at = sig_dict.len() - 1;
    sig_dict.extend(std::iter::repeat_n(b'0', room * 2));
    sig_dict.extend_from_slice(b">");
    let contents_end = sig_dict.len();
    for (k, v) in extra {
        sig_dict.extend_from_slice(format!(" /{k} ").as_bytes());
        v.write(&mut sig_dict);
    }
    sig_dict.extend_from_slice(b">>");
    // The update: objects, cross-references, trailer.
    changed.sort_by_key(|c| c.0);
    let mut objs: Vec<(u32, u16, Vec<u8>)> = changed.iter().map(|(n, g, o)| (*n, *g, o.to_bytes())).collect();
    objs.push((sig_n, 0, sig_dict));
    let (mut out, at) = append_update(pdf, xref, &objs, size)?;
    let sig_pos = at[objs.len() - 1];
    // The byte range, then the signature over it.
    let hole = (sig_pos + contents_at, sig_pos + contents_end);
    let range = format!("0 {} {} {}", hole.0, hole.1, out.len() - hole.1);
    let slot = &mut out[sig_pos + br_at..sig_pos + br_at + BR_WIDTH * 4 + 3];
    slot[..range.len()].copy_from_slice(range.as_bytes());
    let cms = fill(&[&out[..hole.0], &out[hole.1..]])?;
    if cms.len() > room {
        return Err(Error::new("the signature doesn't fit"));
    }
    let hex: String = cms.iter().map(|b| format!("{b:02X}")).collect();
    out[hole.0 + 1..hole.0 + 1 + hex.len()].copy_from_slice(hex.as_bytes());
    Ok(out)
}

/// Appends an incremental update to `pdf` (read into `xref`): the objects
/// (number, generation and body), a cross-reference section and a trailer
/// with `size`. Returns the file and where each object's body starts.
pub(crate) fn append_update(pdf: &[u8], xref: &Xref, objs: &[(u32, u16, Vec<u8>)], size: u32) -> Result<(Vec<u8>, Vec<usize>), Error> {
    let trailer = &xref.trailer;
    let (root_n, root_g) = trailer.get("Root").and_then(Obj::reference).ok_or_else(|| Error::new("no catalog"))?;
    let mut out = pdf.to_vec();
    if out.last() != Some(&b'\n') {
        out.push(b'\n');
    }
    let mut entries: Vec<(u32, u16, usize)> = Vec::new();
    let mut at = Vec::new();
    for (n, g, body) in objs {
        entries.push((*n, *g, out.len()));
        out.extend_from_slice(format!("{n} {g} obj\n").as_bytes());
        at.push(out.len());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    entries.sort_by_key(|e| e.0);
    let xref_at = out.len();
    out.extend_from_slice(b"xref\n");
    let mut i = 0;
    while i < entries.len() {
        let mut j = i;
        while j + 1 < entries.len() && entries[j + 1].0 == entries[j].0 + 1 {
            j += 1;
        }
        out.extend_from_slice(format!("{} {}\n", entries[i].0, j - i + 1).as_bytes());
        for (_, g, off) in &entries[i..=j] {
            out.extend_from_slice(format!("{off:010} {g:05} n \n").as_bytes());
        }
        i = j + 1;
    }
    let mut new_id = [0u8; 16];
    getrandom::fill(&mut new_id).map_err(|_| Error::new("no random numbers"))?;
    let first_id = match trailer.get("ID") {
        Some(Obj::Array(v)) if !v.is_empty() => v[0].clone(),
        _ => Obj::Str(new_id.to_vec()),
    };
    let mut t = vec![("Size".to_string(), Obj::Num(size.to_string())), ("Root".into(), Obj::Ref(root_n, root_g))];
    if let Some(info) = trailer.get("Info") {
        t.push(("Info".into(), info.clone()));
    }
    t.push(("ID".into(), Obj::Array(vec![first_id, Obj::Str(new_id.to_vec())])));
    t.push(("Prev".into(), Obj::Num(xref.start.to_string())));
    out.extend_from_slice(b"trailer\n");
    Obj::Dict(t).write(&mut out);
    out.extend_from_slice(format!("\nstartxref\n{xref_at}\n%%EOF\n").as_bytes());
    Ok((out, at))
}
