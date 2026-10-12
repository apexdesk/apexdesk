//! Signing OOXML and ODF packages as Office 2010+ and LibreOffice do: an
//! XML Signature (RSA or ECDSA with SHA-256) with XAdES-BES properties
//! (signing time, signing certificate v2). OOXML gets a new part
//! `_xmlsignatures/sigN.xml` that signs the parts through a manifest; ODF
//! gets one more `<Signature>` in `META-INF/documentsignatures.xml` that
//! references every stream. The signatures already there stay valid:
//! neither covers the other signatures.

use std::collections::HashMap;

use apex_io::zip::{ZipReader, ZipWriter};

use crate::c14n::{self, Elem, Method};
use crate::crypto::Hash;
use crate::signer::{self, KeyKind, Signer};
use crate::package::{ODF_SIGNATURES, odf_needs_signing};
use crate::{Error, SignOptions, asn1, base64_encode};

const DSIG_NS: &str = "http://www.w3.org/2000/09/xmldsig#";
const XADES_NS: &str = "http://uri.etsi.org/01903/v1.3.2#";
const C14N: &str = "http://www.w3.org/TR/2001/REC-xml-c14n-20010315";
const SHA256: &str = "http://www.w3.org/2001/04/xmlenc#sha256";
const OBJECT_TYPE: &str = "http://www.w3.org/2000/09/xmldsig#Object";
const SIGNED_PROPS_TYPE: &str = "http://uri.etsi.org/01903#SignedProperties";
const OPC_DSIG_NS: &str = "http://schemas.openxmlformats.org/package/2006/digital-signature";
const RELS_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const REL_ORIGIN: &str = "http://schemas.openxmlformats.org/package/2006/relationships/digital-signature/origin";
const REL_SIGNATURE: &str = "http://schemas.openxmlformats.org/package/2006/relationships/digital-signature/signature";
const CT_ORIGIN: &str = "application/vnd.openxmlformats-package.digital-signature-origin";
const CT_SIGNATURE: &str = "application/vnd.openxmlformats-package.digital-signature-xmlsignature+xml";
const ODF_DSIG_NS: &str = "urn:oasis:names:tc:opendocument:xmlns:digitalsignature:1.0";

/// Whether `data` is a package this module can sign: an OOXML or ODF
/// ZIP (not an encrypted one).
pub fn can_sign_package(data: &[u8]) -> bool {
    ZipReader::new(data).is_ok_and(|z| z.contains("[Content_Types].xml") || odf_package(&z))
}

fn odf_package(z: &ZipReader) -> bool {
    z.contains("META-INF/manifest.xml")
}

/// Signs an OOXML or ODF package with `id`, keeping the signatures it has;
/// `opts.reason` is the signer's comment. Returns the signed package.
pub fn sign_package(data: &[u8], id: &dyn Signer, opts: &SignOptions) -> Result<Vec<u8>, Error> {
    sign_package_with(data, id, opts, &crate::net::Http)
}

/// [`sign_package`] asking for time-stamps and revocation through `net`:
/// from level T up the new signature gets XAdES-T, -XL or -A properties.
pub fn sign_package_with(data: &[u8], id: &dyn Signer, opts: &SignOptions, net: &dyn crate::net::Net) -> Result<Vec<u8>, Error> {
    let level = opts.level();
    if level >= crate::Level::T && opts.tsa().is_none() {
        return Err(Error::new("a time-stamp server is needed for this level"));
    }
    let signed = sign_bes(data, id, opts)?;
    crate::xades::extend_some(&signed, level, opts.tsa(), net, true)
}

fn sign_bes(data: &[u8], id: &dyn Signer, opts: &SignOptions) -> Result<Vec<u8>, Error> {
    let z = ZipReader::new(data).map_err(|_| Error::new("only unencrypted OOXML and ODF files can be signed"))?;
    let time = opts.time.unwrap_or_else(crate::time::now);
    let comment = opts.reason.as_deref().map(str::trim).unwrap_or("");
    if z.contains("[Content_Types].xml") {
        ooxml(&z, id, time, comment)
    } else if odf_package(&z) {
        odf(&z, id, time, comment)
    } else {
        Err(Error::new("only OOXML and ODF files can be signed"))
    }
}

// --- common pieces -------------------------------------------------------------------

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\r' => out.push_str("&#13;"),
            _ => out.push(c),
        }
    }
    out
}

/// A part name as a reference URI: escaped where URIs need it.
fn uri_of(name: &str) -> String {
    let mut out = String::new();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/!$&'()*+,;=:@".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    esc(&out)
}

/// `2026-09-26T12:00:00Z`.
fn iso(t: i64) -> String {
    let (y, mo, d, h, mi, s) = crate::time::civil(t);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

fn digest_b64(data: &[u8]) -> String {
    base64_encode(&Hash::Sha256.digest(data))
}

/// The canonical form of a whole XML part (what the C14N transform of a
/// part reference digests).
fn canonical(data: &[u8]) -> Result<Vec<u8>, Error> {
    Ok(c14n::doc(&c14n::parse(data)?, &Method::default()))
}

/// A `<Reference>`; `digest` in base64.
fn reference(uri: &str, kind: Option<&str>, transforms: &str, digest: &str) -> String {
    let kind = kind.map(|k| format!(" Type=\"{k}\"")).unwrap_or_default();
    let transforms = if transforms.is_empty() { String::new() } else { format!("<Transforms>{transforms}</Transforms>") };
    format!("<Reference{kind} URI=\"{uri}\">{transforms}<DigestMethod Algorithm=\"{SHA256}\"/><DigestValue>{digest}</DigestValue></Reference>")
}

fn c14n_transform() -> String {
    format!("<Transform Algorithm=\"{C14N}\"/>")
}

/// A random identifier for elements of an ODF signature (they share one
/// file with the other signatures).
fn random_id() -> Result<String, Error> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).map_err(|_| Error::new("no random numbers"))?;
    Ok(format!("ID_{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>()))
}

/// XAdES-BES: the signed properties (signing time and the signer's
/// certificate v2) and the chain among the unsigned ones.
fn xades_object(id: &dyn Signer, sig_id: &str, props_id: &str, time: i64) -> String {
    let cert = id.certificate();
    let issuer_serial = asn1::seq(&[&asn1::seq(&[&asn1::ctx(4, &[cert.issuer_der()])]), &asn1::tlv(asn1::INTEGER, &cert.serial)]);
    let mut values = format!("<xd:EncapsulatedX509Certificate>{}</xd:EncapsulatedX509Certificate>", base64_encode(&cert.der));
    for c in id.chain().iter().filter(|c| c.der != cert.der) {
        values.push_str(&format!("<xd:EncapsulatedX509Certificate>{}</xd:EncapsulatedX509Certificate>", base64_encode(&c.der)));
    }
    format!(
        "<Object><xd:QualifyingProperties xmlns:xd=\"{XADES_NS}\" Target=\"#{sig_id}\"><xd:SignedProperties Id=\"{props_id}\"><xd:SignedSignatureProperties><xd:SigningTime>{}</xd:SigningTime><xd:SigningCertificateV2><xd:Cert><xd:CertDigest><DigestMethod Algorithm=\"{SHA256}\"/><DigestValue>{}</DigestValue></xd:CertDigest><xd:IssuerSerialV2>{}</xd:IssuerSerialV2></xd:Cert></xd:SigningCertificateV2><xd:SignaturePolicyIdentifier><xd:SignaturePolicyImplied/></xd:SignaturePolicyIdentifier></xd:SignedSignatureProperties></xd:SignedProperties><xd:UnsignedProperties><xd:UnsignedSignatureProperties><xd:CertificateValues>{values}</xd:CertificateValues></xd:UnsignedSignatureProperties></xd:UnsignedProperties></xd:QualifyingProperties></Object>",
        iso(time),
        digest_b64(&cert.der),
        base64_encode(&issuer_serial),
    )
}

/// A signature to write between `pre` and `post` (the rest of its file):
/// `refs` are the finished references, `objects` the `<Object>`s, and
/// `object_refs` the Ids of elements in them to reference (with their
/// type, and whether with a C14N transform).
struct Draft<'a> {
    id: &'a dyn Signer,
    sig_id: &'a str,
    pre: &'a str,
    post: &'a str,
    refs: String,
    objects: String,
    object_refs: Vec<(String, Option<&'static str>, bool)>,
}

impl Draft<'_> {
    fn text(&self, signed_info: &str, value: &str) -> String {
        let key_info = format!("<KeyInfo><X509Data><X509Certificate>{}</X509Certificate></X509Data></KeyInfo>", base64_encode(&self.id.certificate().der));
        format!("{}<Signature xmlns=\"{DSIG_NS}\" Id=\"{}\">{signed_info}<SignatureValue>{value}</SignatureValue>{key_info}{}</Signature>{}", self.pre, self.sig_id, self.objects, self.post)
    }

    /// The whole file with the signature made.
    fn finish(self) -> Result<String, Error> {
        let key = KeyKind::of(self.id.certificate())?;
        // The objects' digests (they don't depend on SignedInfo).
        let draft = self.text("", "");
        let doc = c14n::parse(draft.as_bytes())?;
        let mut refs = String::new();
        for (target, kind, transform) in &self.object_refs {
            let path = doc.find_id(target).ok_or_else(|| Error::new(format!("#{target} not found")))?;
            let bytes = c14n::subtree(&path, &Method::default());
            refs.push_str(&reference(&format!("#{target}"), *kind, if *transform { c14n_transform() } else { String::new() }.as_str(), &digest_b64(&bytes)));
        }
        let signed_info = format!("<SignedInfo><CanonicalizationMethod Algorithm=\"{C14N}\"/><SignatureMethod Algorithm=\"{}\"/>{}{refs}</SignedInfo>", key.xml_method(), self.refs);
        // The canonical SignedInfo, where it will be.
        let draft = self.text(&signed_info, "");
        let doc = c14n::parse(draft.as_bytes())?;
        let mut path = doc.find_id(self.sig_id).ok_or_else(|| Error::new("signature not found"))?;
        let si: &Elem = path.last().and_then(|s| s.child("SignedInfo")).ok_or_else(|| Error::new("SignedInfo not found"))?;
        path.push(si);
        let hash = key.hash();
        let value = signer::sign_raw(self.id, key, hash, &hash.digest(&c14n::subtree(&path, &Method::default())))?;
        Ok(self.text(&signed_info, &base64_encode(&value)))
    }
}

/// Writes the package again with parts replaced (`Some`) or added.
pub(crate) fn repack(z: &ZipReader, mut changes: Vec<(String, Vec<u8>)>) -> Result<Vec<u8>, Error> {
    let mut w = ZipWriter::new();
    for n in z.names() {
        let data = match changes.iter().position(|(c, _)| c == n) {
            Some(i) => changes.remove(i).1,
            None => z.read(n)?,
        };
        w.add(n, &data, n != "mimetype");
    }
    for (n, data) in changes {
        w.add(&n, &data, true);
    }
    Ok(w.finish())
}

/// Inserts `item` before the root's end tag.
fn insert_last(xml: &str, item: &str) -> Result<String, Error> {
    let at = xml.rfind("</").ok_or_else(|| Error::new("malformed XML part"))?;
    Ok(format!("{}{item}{}", &xml[..at], &xml[at..]))
}

// --- OOXML ---------------------------------------------------------------------------

/// Parts Office and LibreOffice leave out of the signature: the document
/// properties can change without breaking it.
pub(crate) fn ooxml_unsigned_part(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    // A name readers could reach as another (".." or empty segments,
    // backslashes, escapes) is never one of them.
    if n.split('/').any(|s| s == ".." || s == ".") || n.contains("//") || n.contains(['\\', '%']) {
        return false;
    }
    n == "[content_types].xml" || n.starts_with("_xmlsignatures/") || n == "docprops/core.xml" || n == "docprops/app.xml" || n.ends_with('/')
}

/// Relationships left out likewise.
fn ooxml_unsigned_rel(kind: &str) -> bool {
    kind.ends_with("/extended-properties") || kind.ends_with("/metadata/core-properties") || kind == REL_ORIGIN
}

/// The part a relationships part belongs to ("word/_rels/document.xml.rels"
/// → "word/document.xml"; the package's own → "").
fn rels_source(name: &str) -> Option<String> {
    let (dir, file) = name.rsplit_once("_rels/").filter(|(d, _)| d.is_empty() || d.ends_with('/'))?;
    Some(format!("{dir}{}", file.strip_suffix(".rels")?))
}

struct ContentTypes {
    defaults: HashMap<String, String>,
    overrides: HashMap<String, String>,
}

impl ContentTypes {
    fn read(xml: &[u8]) -> Result<ContentTypes, Error> {
        let doc = c14n::parse(xml)?;
        let root = doc.root().ok_or_else(|| Error::new("empty content types"))?;
        let mut ct = ContentTypes { defaults: HashMap::new(), overrides: HashMap::new() };
        for e in root.elems() {
            match (e.local(), e.attr("ContentType")) {
                ("Default", Some(t)) => {
                    ct.defaults.insert(e.attr("Extension").unwrap_or("").to_ascii_lowercase(), t.to_string());
                }
                ("Override", Some(t)) => {
                    ct.overrides.insert(e.attr("PartName").unwrap_or("").trim_start_matches('/').to_ascii_lowercase(), t.to_string());
                }
                _ => {}
            }
        }
        Ok(ct)
    }

    fn of(&self, name: &str) -> Option<&str> {
        let lower = name.to_ascii_lowercase();
        self.overrides.get(&lower).or_else(|| self.defaults.get(lower.rsplit_once('.')?.1)).map(String::as_str)
    }
}

/// The relationships part without the white space between its elements,
/// if it has some: Office digests relationships without it, LibreOffice
/// (xmlsec) with a mark for each, so only compact ones verify in both.
fn compact_rels(xml: &str) -> Option<String> {
    let start = xml.find("<Relationships")?;
    let body = start + xml[start..].find('>')? + 1;
    let mut out = String::with_capacity(xml.len());
    out.push_str(&xml[..body]);
    let mut in_tag = false;
    let mut pending = String::new();
    for c in xml[body..].chars() {
        match c {
            '<' => {
                in_tag = true;
                pending.clear();
                out.push(c);
            }
            '>' => {
                in_tag = false;
                out.push(c);
            }
            c if !in_tag && c.is_whitespace() => pending.push(c),
            c => {
                out.push_str(&pending);
                pending.clear();
                out.push(c);
            }
        }
    }
    (out.len() != xml.len()).then_some(out)
}

fn ooxml(z: &ZipReader, id: &dyn Signer, time: i64, comment: &str) -> Result<Vec<u8>, Error> {
    let ct_xml = z.read_string("[Content_Types].xml")?;
    let ct = ContentTypes::read(ct_xml.as_bytes())?;
    let signed_before = !crate::package::ooxml_parts(z).is_empty();
    let mut changes: Vec<(String, Vec<u8>)> = Vec::new();
    if !signed_before {
        for name in z.names().filter(|n| n.ends_with(".rels")) {
            if let Some(x) = compact_rels(&z.read_string(name)?) {
                changes.push((name.to_string(), x.into_bytes()));
            }
        }
    }
    let current = |changes: &[(String, Vec<u8>)], name: &str| -> Result<Vec<u8>, Error> {
        match changes.iter().find(|(n, _)| n == name) {
            Some((_, d)) => Ok(d.clone()),
            None => Ok(z.read(name)?),
        }
    };
    let set = |changes: &mut Vec<(String, Vec<u8>)>, name: &str, data: Vec<u8>| {
        changes.retain(|(n, _)| n != name);
        changes.push((name.to_string(), data));
    };
    // The first free signature part.
    let n = (1..).find(|n| !z.names().any(|x| x.eq_ignore_ascii_case(&format!("_xmlsignatures/sig{n}.xml")))).unwrap_or(1);
    let part = format!("_xmlsignatures/sig{n}.xml");
    // Content types of the origin and the new signature.
    let mut ct_new = ct_xml.clone();
    if !ct.defaults.contains_key("sigs") && ct.of("_xmlsignatures/origin.sigs").is_none() {
        ct_new = insert_last(&ct_new, &format!("<Default Extension=\"sigs\" ContentType=\"{CT_ORIGIN}\"/>"))?;
    }
    ct_new = insert_last(&ct_new, &format!("<Override PartName=\"/{part}\" ContentType=\"{CT_SIGNATURE}\"/>"))?;
    set(&mut changes, "[Content_Types].xml", ct_new.into_bytes());
    // The package points to the origin, the origin to each signature.
    let origin = match z.names().find(|n| n.eq_ignore_ascii_case("_xmlsignatures/origin.sigs")) {
        Some(o) => o.to_string(),
        None => {
            set(&mut changes, "_xmlsignatures/origin.sigs", Vec::new());
            let rels = String::from_utf8_lossy(&current(&changes, "_rels/.rels")?).into_owned();
            let rid = free_id(&rels, "rIdSig");
            let rels = insert_last(&rels, &format!("<Relationship Id=\"{rid}\" Type=\"{REL_ORIGIN}\" Target=\"_xmlsignatures/origin.sigs\"/>"))?;
            set(&mut changes, "_rels/.rels", rels.into_bytes());
            "_xmlsignatures/origin.sigs".into()
        }
    };
    let (dir, file) = origin.rsplit_once('/').unwrap_or(("", &origin));
    let origin_rels = format!("{dir}/_rels/{file}.rels");
    let rels_name = z.names().find(|x| x.eq_ignore_ascii_case(&origin_rels)).map(str::to_string).unwrap_or(origin_rels);
    let rels = current(&changes, &rels_name).map(|d| String::from_utf8_lossy(&d).into_owned()).unwrap_or_else(|_| format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"{RELS_NS}\"></Relationships>"));
    let rid = free_id(&rels, "rId");
    let rels = insert_last(&rels, &format!("<Relationship Id=\"{rid}\" Type=\"{REL_SIGNATURE}\" Target=\"sig{n}.xml\"/>"))?;
    set(&mut changes, &rels_name, rels.into_bytes());
    // The manifest: every part but the unsigned ones, relationships through
    // the relationship transform.
    // The properties too when the text shows them (see `verify`).
    let props = crate::package::shows_properties(&z);
    let mut names: Vec<&str> = z.names().filter(|n| !ooxml_unsigned_part(n) || (props && (n.eq_ignore_ascii_case("docProps/core.xml") || n.eq_ignore_ascii_case("docProps/app.xml")))).collect();
    names.sort_by_key(|n| n.to_ascii_lowercase());
    let mut manifest = String::new();
    for name in names {
        let data = current(&changes, name)?;
        let query = ct.of(name).map(|t| format!("?ContentType={t}")).unwrap_or_default();
        let uri = format!("/{}{}", uri_of(name), esc(&query));
        if let Some(source) = rels_source(name) {
            if !source.is_empty() && ooxml_unsigned_part(&source) {
                continue;
            }
            let doc = c14n::parse(&data)?;
            let Some(root) = doc.root() else { continue };
            let ids: Vec<&str> = root.elems().filter(|r| r.local() == "Relationship" && !r.attr("Type").is_some_and(ooxml_unsigned_rel)).filter_map(|r| r.attr("Id")).collect();
            if ids.is_empty() {
                continue;
            }
            let mut t = String::from("<Transform Algorithm=\"http://schemas.openxmlformats.org/package/2006/RelationshipTransform\">");
            for i in &ids {
                t.push_str(&format!("<mdssi:RelationshipReference xmlns:mdssi=\"{OPC_DSIG_NS}\" SourceId=\"{}\"/>", esc(i)));
            }
            t.push_str("</Transform>");
            // Digested as the verifier will: the transform, then C14N (the
            // way the signatures there did it, if they left white space).
            let spec = c14n::parse(t.as_bytes())?;
            let picked = crate::xmldsig::relationship_transform(&data, spec.root().ok_or_else(|| Error::new("transform"))?, signed_before).map_err(Error::new)?;
            t.push_str(&c14n_transform());
            manifest.push_str(&reference(&uri, None, &t, &digest_b64(&canonical(&picked)?)));
        } else {
            manifest.push_str(&reference(&uri, None, "", &digest_b64(&data)));
        }
    }
    let sig_id = "idPackageSignature";
    let package_object = format!(
        "<Object Id=\"idPackageObject\"><Manifest>{manifest}</Manifest><SignatureProperties><SignatureProperty Id=\"idSignatureTime\" Target=\"#{sig_id}\"><mdssi:SignatureTime xmlns:mdssi=\"{OPC_DSIG_NS}\"><mdssi:Format>YYYY-MM-DDThh:mm:ssTZD</mdssi:Format><mdssi:Value>{}</mdssi:Value></mdssi:SignatureTime></SignatureProperty></SignatureProperties></Object>",
        iso(time)
    );
    let office_object = format!(
        "<Object Id=\"idOfficeObject\"><SignatureProperties><SignatureProperty Id=\"idOfficeV1Details\" Target=\"#{sig_id}\"><SignatureInfoV1 xmlns=\"http://schemas.microsoft.com/office/2006/digsig\"><SetupID></SetupID><SignatureText></SignatureText><SignatureImage/><SignatureComments>{}</SignatureComments><WindowsVersion>10.0</WindowsVersion><OfficeVersion>16.0</OfficeVersion><ApplicationVersion>16.0</ApplicationVersion><Monitors>1</Monitors><HorizontalResolution>1920</HorizontalResolution><VerticalResolution>1080</VerticalResolution><ColorDepth>32</ColorDepth><SignatureProviderId>{{00000000-0000-0000-0000-000000000000}}</SignatureProviderId><SignatureProviderUrl></SignatureProviderUrl><SignatureProviderDetails>9</SignatureProviderDetails><SignatureType>1</SignatureType></SignatureInfoV1></SignatureProperty></SignatureProperties></Object>",
        esc(comment)
    );
    let draft = Draft {
        id,
        sig_id,
        pre: "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
        post: "",
        refs: String::new(),
        objects: format!("{package_object}{office_object}{}", xades_object(id, sig_id, "idSignedProperties", time)),
        object_refs: vec![("idPackageObject".into(), Some(OBJECT_TYPE), false), ("idOfficeObject".into(), Some(OBJECT_TYPE), false), ("idSignedProperties".into(), Some(SIGNED_PROPS_TYPE), true)],
    };
    changes.push((part, draft.finish()?.into_bytes()));
    repack(z, changes)
}

/// A relationship Id not yet in `rels`: `base`, else `base` with a number.
fn free_id(rels: &str, base: &str) -> String {
    let taken = |id: &str| rels.contains(&format!("Id=\"{id}\""));
    if base != "rId" && !taken(base) {
        return base.to_string();
    }
    (1..).map(|i| format!("{base}{i}")).find(|id| !taken(id)).unwrap_or_default()
}

// --- ODF -----------------------------------------------------------------------------

/// The manifest's media types, and which streams are encrypted.
fn odf_manifest(z: &ZipReader) -> HashMap<String, (String, bool)> {
    let mut out = HashMap::new();
    let Ok(doc) = z.read("META-INF/manifest.xml").map_err(|e| e.to_string()).and_then(|x| c14n::parse(&x).map_err(|e| e.to_string())) else { return out };
    if let Some(root) = doc.root() {
        for e in root.elems().filter(|e| e.local() == "file-entry") {
            if let Some(path) = e.attr("full-path") {
                out.insert(path.to_string(), (e.attr("media-type").unwrap_or("").to_string(), e.child("encryption-data").is_some()));
            }
        }
    }
    out
}

fn odf(z: &ZipReader, id: &dyn Signer, time: i64, comment: &str) -> Result<Vec<u8>, Error> {
    let manifest = odf_manifest(z);
    let mut refs = String::new();
    for name in z.names().filter(|n| odf_needs_signing(n)) {
        let data = z.read(name)?;
        // XML streams are signed in their canonical form (not the
        // encrypted ones).
        let xml = match manifest.get(name) {
            Some((kind, encrypted)) => kind == "text/xml" && !encrypted,
            None => name.ends_with(".xml"),
        };
        if xml {
            refs.push_str(&reference(&uri_of(name), None, &c14n_transform(), &digest_b64(&canonical(&data)?)));
        } else {
            refs.push_str(&reference(&uri_of(name), None, "", &digest_b64(&data)));
        }
    }
    // The other signatures stay as they are, the new one goes last.
    let existing = z.read_string(ODF_SIGNATURES).ok().filter(|x| c14n::parse(x.as_bytes()).is_ok_and(|d| d.root().is_some_and(|r| r.local() == "document-signatures")));
    let file = existing.unwrap_or_else(|| format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<document-signatures xmlns=\"{ODF_DSIG_NS}\"></document-signatures>"));
    let at = file.rfind("</").ok_or_else(|| Error::new("malformed signatures"))?;
    let (pre, post) = file.split_at(at);
    let sig_id = random_id()?;
    let date_id = random_id()?;
    let props_id = format!("idSignedProperties_{sig_id}");
    let dc = "xmlns:dc=\"http://purl.org/dc/elements/1.1/\"";
    let mut props = format!("<SignatureProperty Id=\"{date_id}\" Target=\"#{sig_id}\"><dc:date {dc}>{}</dc:date></SignatureProperty>", iso(time));
    let mut object_refs = vec![(date_id, None, false)];
    if !comment.is_empty() {
        let desc_id = random_id()?;
        props.push_str(&format!("<SignatureProperty Id=\"{desc_id}\" Target=\"#{sig_id}\"><dc:description {dc}>{}</dc:description></SignatureProperty>", esc(comment)));
        object_refs.push((desc_id, None, false));
    }
    object_refs.push((props_id.clone(), Some(SIGNED_PROPS_TYPE), false));
    let draft = Draft {
        id,
        sig_id: &sig_id,
        pre,
        post,
        refs,
        objects: format!("<Object><SignatureProperties>{props}</SignatureProperties></Object>{}", xades_object(id, &sig_id, &props_id, time)),
        object_refs,
    };
    let signed = draft.finish()?;
    repack(z, vec![(ODF_SIGNATURES.to_string(), signed.into_bytes())])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(rels_source("_rels/.rels").as_deref(), Some(""));
        assert_eq!(rels_source("word/_rels/document.xml.rels").as_deref(), Some("word/document.xml"));
        assert_eq!(rels_source("word/document.xml"), None);
        assert_eq!(uri_of("Pictures/a b&c.png"), "Pictures/a%20b&amp;c.png");
        assert_eq!(free_id("<Relationship Id=\"rId1\"/>", "rId"), "rId2");
        assert_eq!(free_id("", "rIdSig"), "rIdSig");
        let rels = "<?xml version=\"1.0\"?>\n<Relationships xmlns=\"urn:r\">\n  <Relationship Id=\"a b\" Target=\"x\"/>\n</Relationships>";
        assert_eq!(compact_rels(rels).as_deref(), Some("<?xml version=\"1.0\"?>\n<Relationships xmlns=\"urn:r\"><Relationship Id=\"a b\" Target=\"x\"/></Relationships>"));
        assert_eq!(compact_rels("<Relationships><Relationship/></Relationships>"), None);
        assert_eq!(base64_encode(b"ab"), "YWI=");
        assert_eq!(crate::base64(&base64_encode(b"hello world!")).unwrap(), b"hello world!");
    }
}
