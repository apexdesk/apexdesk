//! Flat OpenDocument files (.fods, .fodt): the whole package in one XML
//! `office:document`, with pictures inline (base64 in `office:binary-data`)
//! and embedded objects as nested `office:document`s.
//!
//! Reading splits the file into the parts of a package (content.xml,
//! styles.xml, meta.xml, settings.xml, Pictures/..., Object N/..., the
//! Basic libraries) for the .ods/.odt readers; writing joins the package
//! they write back into one file. Package files with no place in a flat
//! file (ApexDesk's own parts) go in `apexdesk:file` elements, so nothing
//! is lost on the way back.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use crate::Error;
use crate::markdown::{base64_decode, base64_encode};
use crate::xml::{Ev, Reader, Tag, esc, local};
use crate::zip::{ZipReader, ZipWriter};

const HEAD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n";
const NS_XLINK: &str = "http://www.w3.org/1999/xlink";
const NS_SCRIPT: &str = "urn:oasis:names:tc:opendocument:xmlns:script:1.0";
const NS_OOO: &str = "http://openoffice.org/2004/office";
/// Namespace of the package files kept as `apexdesk:file`.
const NS_APEX: &str = "urn:apexdesk:names:flat-package:1.0";
const FILE_TAG: &str = "apexdesk:file";

/// The media type of a flat OpenDocument file (empty if it names none),
/// or `None` if `data` is not one.
pub(crate) fn sniff(data: &[u8]) -> Option<String> {
    let head = String::from_utf8_lossy(&data[..data.len().min(16384)]);
    if !head.trim_start_matches('\u{feff}').trim_start().starts_with('<') {
        return None;
    }
    let mut r = Reader::new(&head);
    loop {
        match r.next() {
            Ev::Start(t) if t.name == "office:document" => return Some(t.attr("office:mimetype").map(|m| m.into_owned()).unwrap_or_default()),
            Ev::Start(_) | Ev::Empty(_) | Ev::Eof => return None,
            _ => {}
        }
    }
}

/// Text of the file: UTF-8, or else the encoding it declares.
fn decode(data: &[u8]) -> String {
    let data = data.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(data);
    if let Ok(s) = std::str::from_utf8(data) {
        return s.to_string();
    }
    let head = String::from_utf8_lossy(&data[..data.len().min(256)]).into_owned();
    let label = head.split_once("encoding=").and_then(|(_, r)| {
        let q = r.chars().next()?;
        r[q.len_utf8()..].split(q).next().map(str::to_string)
    });
    let enc = label.and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes())).unwrap_or(encoding_rs::WINDOWS_1252);
    enc.decode(data).0.into_owned()
}

/// Escapes element text (quotes and line breaks stay as they are).
fn esc_text(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains(['<', '>', '&']) {
        return s.into();
    }
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").into()
}

fn attrs_xml(attrs: &[(String, String)]) -> String {
    let mut s = String::new();
    for (k, v) in attrs {
        let _ = write!(s, " {k}=\"{}\"", esc(v));
    }
    s
}

fn add_attr(attrs: &mut Vec<(String, String)>, k: &str, v: &str) {
    if !attrs.iter().any(|(a, _)| a == k) {
        attrs.push((k.to_string(), v.to_string()));
    }
}

/// The element `el` (its source, start tag first) and its child elements
/// with their sources.
fn children(el: &str) -> Option<(Tag<'_>, Vec<(Tag<'_>, &str)>)> {
    let mut r = Reader::new(el);
    let root = loop {
        match r.next() {
            Ev::Start(t) => break t,
            Ev::Empty(t) => return Some((t, Vec::new())),
            Ev::Eof | Ev::End(_) => return None,
            Ev::Text(_) => {}
        }
    };
    let mut kids = Vec::new();
    loop {
        let p0 = r.pos();
        match r.next() {
            Ev::Start(t) => {
                r.skip();
                kids.push((t, &el[p0..r.pos()]));
            }
            Ev::Empty(t) => kids.push((t, &el[p0..r.pos()])),
            Ev::End(_) | Ev::Eof => break,
            Ev::Text(_) => {}
        }
    }
    Some((root, kids))
}

fn write_tag(out: &mut String, t: &Tag, empty: bool) {
    out.push('<');
    out.push_str(t.name);
    out.push_str(t.raw_attrs().trim_end());
    out.push_str(if empty { "/>" } else { ">" });
}

// ---------------------------------------------------------------------------
// Reading: flat file -> package

#[derive(Default)]
struct Parts {
    /// (path, data, media type); folders end in `/` and have no data.
    files: Vec<(String, Vec<u8>, String)>,
    pictures: usize,
    objects: usize,
}

impl Parts {
    fn add(&mut self, path: String, data: Vec<u8>, mime: &str) {
        self.files.retain(|f| f.0 != path);
        self.files.push((path, data, mime.to_string()));
    }

    /// Stores inline data of element `owner`; returns its path relative
    /// to the document at `prefix`.
    fn add_binary(&mut self, prefix: &str, owner: &str, data: Vec<u8>) -> String {
        if owner == "object-ole" {
            self.objects += 1;
            let rel = format!("Object {}", self.objects);
            self.add(format!("{prefix}{rel}"), data, "application/vnd.sun.star.oleobject");
            return format!("./{rel}");
        }
        self.pictures += 1;
        let (ext, mime) = match apex_sheet::ImageFormat::sniff(&data) {
            Some(f) => (f.ext(), f.mime()),
            None if data.starts_with(b"%PDF") => ("pdf", "application/pdf"),
            None => ("bin", "application/octet-stream"),
        };
        let rel = format!("Pictures/{}.{ext}", self.pictures);
        self.add(format!("{prefix}{rel}"), data, mime);
        rel
    }
}

/// Converts a flat OpenDocument file into the equivalent package (zip).
pub(crate) fn to_package(data: &[u8]) -> Result<Vec<u8>, Error> {
    let src = decode(data);
    let mut parts = Parts::default();
    let mime = split_document(&src, "", &[], &mut parts)?;
    let mut z = ZipWriter::new();
    z.add("mimetype", mime.as_bytes(), false);
    let mut entries = String::new();
    for (path, data, mt) in &parts.files {
        let _ = write!(entries, r#"<manifest:file-entry manifest:full-path="{}" manifest:media-type="{}"/>"#, esc(path), esc(mt));
        if !path.ends_with('/') {
            z.add(path, data, false);
        }
    }
    z.add("META-INF/manifest.xml", format!(
        r#"{HEAD}<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3"><manifest:file-entry manifest:full-path="/" manifest:version="1.3" manifest:media-type="{}"/>{entries}</manifest:manifest>"#,
        esc(&mime)
    ).as_bytes(), false);
    Ok(z.finish())
}

/// Splits the `office:document` in `src` into parts under `prefix`;
/// returns its media type. `outer` are the attributes of the enclosing
/// document (a nested one may rely on its namespaces).
fn split_document(src: &str, prefix: &str, outer: &[(String, String)], parts: &mut Parts) -> Result<String, Error> {
    // Objects in objects: a few levels at most (each level goes down the
    // stack and over its text again).
    if prefix.matches('/').count() > 8 {
        return Err(Error::new("objects nested too deeply"));
    }
    let (root, kids) = children(src).filter(|(t, _)| local(t.name) == "document").ok_or_else(|| Error::new("not a flat OpenDocument file"))?;
    let mime = root.attr("office:mimetype").map(|m| m.into_owned()).unwrap_or_default();
    let mut attrs: Vec<(String, String)> = root.attrs().filter(|(k, _)| *k != "office:mimetype").map(|(k, v)| (k.to_string(), v.into_owned())).collect();
    for (k, v) in outer.iter().filter(|(k, _)| k.starts_with("xmlns:")) {
        add_attr(&mut attrs, k, v);
    }
    add_attr(&mut attrs, "xmlns:xlink", NS_XLINK);
    let kept_basic = kids.iter().any(|(t, _)| t.name == FILE_TAG && t.attr("apexdesk:path").is_some_and(|p| p.starts_with("Basic/")));
    let (mut content, mut styles) = (String::new(), String::new());
    let (mut scripts, mut fonts, mut auto, mut body) = (String::new(), String::new(), String::new(), String::new());
    let (mut named, mut masters) = (String::new(), String::new());
    for (t, full) in &kids {
        if t.name == FILE_TAG {
            // A package file kept by ApexDesk.
            let (Some(path), mut r) = (t.attr("apexdesk:path"), Reader::new(full)) else { continue };
            r.next();
            let data = if full.ends_with("/>") { Some(Vec::new()) } else { b64(&r.text()) };
            if let Some(data) = data {
                parts.add(format!("{prefix}{path}"), data, &t.attr("apexdesk:media-type").unwrap_or_default());
            }
            continue;
        }
        let x = extract(full, prefix, &attrs, parts)?;
        match local(t.name) {
            "meta" => parts.add(format!("{prefix}meta.xml"), format!("{HEAD}<office:document-meta{}>{x}</office:document-meta>", attrs_xml(&attrs)).into_bytes(), "text/xml"),
            "settings" => parts.add(format!("{prefix}settings.xml"), format!("{HEAD}<office:document-settings{}>{x}</office:document-settings>", attrs_xml(&attrs)).into_bytes(), "text/xml"),
            "scripts" => {
                if prefix.is_empty() && !kept_basic {
                    for (path, data) in basic_files(&x) {
                        parts.add(path, data, "text/xml");
                    }
                }
                scripts.push_str(&x);
            }
            "font-face-decls" => fonts.push_str(&x),
            "automatic-styles" => auto.push_str(&x),
            "styles" => named.push_str(&x),
            "master-styles" => masters.push_str(&x),
            "body" => body.push_str(&x),
            _ => {}
        }
    }
    let a = attrs_xml(&attrs);
    let _ = write!(content, "{HEAD}<office:document-content{a}>{scripts}{fonts}{auto}{body}</office:document-content>");
    // The automatic styles go to both parts: header and footer paragraphs
    // and page layouts use them from styles.xml, the body from content.xml.
    let _ = write!(styles, "{HEAD}<office:document-styles{a}>{fonts}{named}{auto}{masters}</office:document-styles>");
    parts.add(format!("{prefix}content.xml"), content.into_bytes(), "text/xml");
    parts.add(format!("{prefix}styles.xml"), styles.into_bytes(), "text/xml");
    Ok(mime)
}

/// The element `src` with its inline pictures and objects moved to parts:
/// each gets an `xlink:href` to its file instead.
fn extract(src: &str, prefix: &str, attrs: &[(String, String)], parts: &mut Parts) -> Result<String, Error> {
    if !src.contains("binary-data") && !src.contains("<office:document") {
        return Ok(src.to_string());
    }
    let mut out = String::with_capacity(src.len());
    let mut r = Reader::new(src);
    let mut pending: Option<Tag> = None;
    // Spaces between a start tag and its first child.
    let mut gap = String::new();
    loop {
        let p0 = r.pos();
        let ev = r.next();
        if let Some(parent) = pending.take() {
            if let Ev::Text(t) = &ev
                && t.trim().is_empty()
            {
                gap.push_str(t);
                pending = Some(parent);
                continue;
            }
            let href = match &ev {
                Ev::Start(t) if t.name == "office:binary-data" => b64(&r.text()).map(|data| parts.add_binary(prefix, parent.local(), data)),
                Ev::Start(t) if t.name == "office:document" => {
                    r.skip();
                    parts.objects += 1;
                    let rel = format!("Object {}", parts.objects);
                    let sub = format!("{prefix}{rel}/");
                    let mime = split_document(&src[p0..r.pos()], &sub, attrs, parts)?;
                    parts.add(sub, Vec::new(), &mime);
                    Some(format!("./{rel}"))
                }
                _ => None,
            };
            if let Some(href) = href {
                out.push('<');
                out.push_str(parent.name);
                out.push_str(parent.raw_attrs().trim_end());
                if parent.attr("xlink:href").is_none() {
                    let _ = write!(out, r#" xlink:href="{}" xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad""#, esc(&href));
                }
                out.push('>');
                out.push_str(&std::mem::take(&mut gap));
                continue;
            }
            write_tag(&mut out, &parent, false);
            out.push_str(&std::mem::take(&mut gap));
            if matches!(ev, Ev::Start(ref t) if t.name == "office:binary-data") {
                // Not base64: dropped, as other programs do.
                continue;
            }
        }
        match ev {
            Ev::Start(t) => pending = Some(t),
            Ev::Empty(t) => write_tag(&mut out, &t, true),
            Ev::End(n) => {
                out.push_str("</");
                out.push_str(n);
                out.push('>');
            }
            Ev::Text(t) => out.push_str(&esc_text(&t)),
            Ev::Eof => break,
        }
    }
    Ok(out)
}

fn b64(s: &str) -> Option<Vec<u8>> {
    let s: String = s.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    base64_decode(&s)
}

const DOCTYPE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE "#;

/// The Basic libraries of a flat file's `office:scripts` as package files
/// (Basic/script-lc.xml and a folder per library).
fn basic_files(scripts: &str) -> Vec<(String, Vec<u8>)> {
    // (library, [(module, source)])
    let mut libs: Vec<(String, Vec<(String, String)>)> = Vec::new();
    let mut r = Reader::new(scripts);
    let mut module = String::new();
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.name == "ooo:library-embedded" => libs.push((t.attr("ooo:name").unwrap_or_default().into_owned(), Vec::new())),
            Ev::Start(t) if t.name == "ooo:module" => module = t.attr("ooo:name").unwrap_or_default().into_owned(),
            Ev::Start(t) if t.name == "ooo:source-code" => {
                let code = r.text();
                if let Some(l) = libs.last_mut() {
                    l.1.push((std::mem::take(&mut module), code));
                }
            }
            _ => {}
        }
    }
    let mut files = Vec::new();
    if libs.is_empty() {
        return files;
    }
    let mut lc = String::new();
    for (lib, modules) in &libs {
        let lib = esc(lib);
        let _ = write!(lc, "\n <library:library library:name=\"{lib}\" library:link=\"false\"/>");
        let mut elements = String::new();
        for (name, code) in modules {
            let _ = write!(elements, "\n <library:element library:name=\"{}\"/>", esc(name));
            files.push((format!("Basic/{lib}/{name}.xml"), format!(r#"{DOCTYPE}script:module PUBLIC "-//OpenOffice.org//DTD OfficeDocument 1.0//EN" "module.dtd">
<script:module xmlns:script="http://openoffice.org/2000/script" script:name="{}" script:language="StarBasic" script:moduleType="normal">{}</script:module>"#, esc(name), esc_text(code)).into_bytes()));
        }
        files.push((format!("Basic/{lib}/script-lb.xml"), format!(r#"{DOCTYPE}library:library PUBLIC "-//OpenOffice.org//DTD OfficeDocument 1.0//EN" "library.dtd">
<library:library xmlns:library="http://openoffice.org/2000/library" library:name="{lib}" library:readonly="false" library:passwordprotected="false">{elements}
</library:library>"#).into_bytes()));
    }
    files.push(("Basic/script-lc.xml".to_string(), format!(r#"{DOCTYPE}library:libraries PUBLIC "-//OpenOffice.org//DTD OfficeDocument 1.0//EN" "libraries.dtd">
<library:libraries xmlns:library="http://openoffice.org/2000/library" xmlns:xlink="http://www.w3.org/1999/xlink">{lc}
</library:libraries>"#).into_bytes()));
    files
}

// ---------------------------------------------------------------------------
// Writing: package -> flat file

/// Converts an OpenDocument package (zip) into a flat file.
pub(crate) fn from_package(data: &[u8]) -> Result<Vec<u8>, Error> {
    let z = ZipReader::new(data)?;
    let manifest = z.read_string("META-INF/manifest.xml").map(|m| media_types(&m)).unwrap_or_default();
    let mime = z.read_string("mimetype").unwrap_or_default();
    let mut used: HashSet<String> = ["mimetype", "META-INF/manifest.xml"].into_iter().map(String::from).collect();
    let basic = basic_script(&z, &mut used);
    let mut pkg = Package { z: &z, manifest: &manifest, used };
    let (mut attrs, inner) = pkg.join("", &mime, basic.as_deref())?;
    // Files a flat file has no place for, kept as they are.
    let mut kept = String::new();
    let names: Vec<&str> = z.names().collect();
    for name in names {
        if pkg.used.contains(name) || name.ends_with('/') {
            continue;
        }
        let Ok(data) = z.read(name) else { continue };
        let mt = manifest.get(name).map_or("", String::as_str);
        let _ = write!(kept, r#"<{FILE_TAG} apexdesk:path="{}" apexdesk:media-type="{}">{}</{FILE_TAG}>"#, esc(name), esc(mt), base64_encode(&data));
    }
    if !kept.is_empty() {
        add_attr(&mut attrs, "xmlns:apexdesk", NS_APEX);
    }
    Ok(format!("{HEAD}<office:document{}>{inner}{kept}</office:document>", attrs_xml(&attrs)).into_bytes())
}

/// Media type of each file named in a manifest.
fn media_types(manifest: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut r = Reader::new(manifest);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "file-entry" => {
                if let (Some(p), Some(m)) = (t.attr("manifest:full-path"), t.attr("manifest:media-type")) {
                    out.insert(p.into_owned(), m.into_owned());
                }
            }
            _ => {}
        }
    }
    out
}

/// The package's Basic libraries as an `office:script` element; their
/// files are marked used, unless something would be lost inline (class
/// modules, protected libraries): then they are also kept as files.
fn basic_script(z: &ZipReader, used: &mut HashSet<String>) -> Option<String> {
    let lc = z.read_string("Basic/script-lc.xml").ok()?;
    let mut libs = String::new();
    let mut read = vec!["Basic/script-lc.xml".to_string()];
    let mut exact = true;
    let mut r = Reader::new(&lc);
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if t.local() == "library" => {
                let lib = t.attr("library:name")?.into_owned();
                if t.attr("library:link").as_deref() == Some("true") {
                    exact = false;
                    continue;
                }
                let lb_path = format!("Basic/{lib}/script-lb.xml");
                let lb = z.read_string(&lb_path).ok()?;
                exact &= !lb.contains("library:passwordprotected=\"true\"") && !lb.contains("library:readonly=\"true\"");
                read.push(lb_path);
                let _ = write!(libs, r#"<ooo:library-embedded ooo:name="{}">"#, esc(&lib));
                let mut lr = Reader::new(&lb);
                loop {
                    match lr.next() {
                        Ev::Eof => break,
                        Ev::Start(e) | Ev::Empty(e) if e.local() == "element" => {
                            let name = e.attr("library:name")?.into_owned();
                            let path = format!("Basic/{lib}/{name}.xml");
                            let m = z.read_string(&path).ok()?;
                            let mut mr = Reader::new(&m);
                            let code = loop {
                                match mr.next() {
                                    Ev::Start(s) if s.local() == "module" => {
                                        exact &= s.attr("script:moduleType").is_none_or(|v| v == "normal");
                                        break mr.text();
                                    }
                                    Ev::Eof => break String::new(),
                                    _ => {}
                                }
                            };
                            read.push(path);
                            let _ = write!(libs, r#"<ooo:module ooo:name="{}"><ooo:source-code>{}</ooo:source-code></ooo:module>"#, esc(&name), esc_text(&code));
                        }
                        _ => {}
                    }
                }
                libs.push_str("</ooo:library-embedded>");
            }
            _ => {}
        }
    }
    if exact {
        used.extend(read);
    }
    Some(format!(r#"<office:script script:language="ooo:Basic"><ooo:libraries xmlns:ooo="{NS_OOO}" xmlns:xlink="{NS_XLINK}">{libs}</ooo:libraries></office:script>"#))
}

struct Package<'a> {
    z: &'a ZipReader<'a>,
    manifest: &'a HashMap<String, String>,
    used: HashSet<String>,
}

impl Package<'_> {
    fn read(&mut self, path: String) -> Option<String> {
        let s = self.z.read_string(&path).ok()?;
        self.used.insert(path);
        Some(s)
    }

    /// The document at `prefix`: the attributes of its `office:document`
    /// and what goes inside.
    fn join(&mut self, prefix: &str, mime: &str, basic: Option<&str>) -> Result<(Vec<(String, String)>, String), Error> {
        let content = self.read(format!("{prefix}content.xml")).ok_or_else(|| Error::new("missing part 'content.xml' in package"))?;
        let styles = self.read(format!("{prefix}styles.xml")).unwrap_or_default();
        let meta = self.read(format!("{prefix}meta.xml")).unwrap_or_default();
        let settings = self.read(format!("{prefix}settings.xml")).unwrap_or_default();
        let mut attrs: Vec<(String, String)> = Vec::new();
        // Top-level elements by local name, in document order.
        let mut kids: Vec<(&str, &str, Tag)> = Vec::new();
        for (part, src) in [("content", &content), ("styles", &styles), ("meta", &meta), ("settings", &settings)] {
            let Some((root, ks)) = children(src) else { continue };
            for (k, v) in root.attrs().filter(|(k, _)| *k != "office:mimetype") {
                add_attr(&mut attrs, k, &v);
            }
            for (t, full) in ks {
                kids.push((part, full, t));
            }
        }
        add_attr(&mut attrs, "xmlns:office", "urn:oasis:names:tc:opendocument:xmlns:office:1.0");
        if !mime.is_empty() {
            add_attr(&mut attrs, "office:mimetype", mime);
        }
        let find = |part: &str, name: &str| kids.iter().find(|k| k.0 == part && local(k.2.name) == name).map(|k| k.1);
        let mut out = String::new();
        for (part, name) in [("meta", "meta"), ("settings", "settings")] {
            if let Some(x) = find(part, name) {
                out.push_str(&self.embed(x, prefix)?);
            }
        }
        match (find("content", "scripts"), basic) {
            (Some(s), Some(b)) => out.push_str(&s.replacen("</office:scripts>", &format!("{b}</office:scripts>"), 1)),
            (Some(s), None) => out.push_str(s),
            (None, Some(b)) => {
                let _ = write!(out, "<office:scripts>{b}</office:scripts>");
            }
            (None, None) => {}
        }
        if basic.is_some() {
            add_attr(&mut attrs, "xmlns:script", NS_SCRIPT);
        }
        // Fonts of both parts, each once.
        let mut fonts = String::new();
        let mut seen = HashSet::new();
        for part in ["styles", "content"] {
            for (t, full) in find(part, "font-face-decls").and_then(children).map(|c| c.1).unwrap_or_default() {
                if seen.insert(t.attr("style:name").unwrap_or_default().into_owned()) {
                    fonts.push_str(full);
                }
            }
        }
        if !fonts.is_empty() {
            let _ = write!(out, "<office:font-face-decls>{fonts}</office:font-face-decls>");
        }
        if let Some(x) = find("styles", "styles") {
            out.push_str(&self.embed(x, prefix)?);
        }
        // The automatic styles of both parts in one list: a name used by
        // both for different styles is renamed in the styles.xml ones.
        let mut auto = String::new();
        let mut names: HashMap<String, &str> = HashMap::new();
        let content_auto = find("content", "automatic-styles").and_then(children).map(|c| c.1).unwrap_or_default();
        for (t, full) in &content_auto {
            names.insert(format!("{}\u{1}{}", t.name, t.attr("style:name").unwrap_or_default()), full);
            auto.push_str(full);
        }
        let taken: HashSet<String> = content_auto.iter().filter_map(|(t, _)| t.attr("style:name").map(|n| n.into_owned())).collect();
        let mut renames: HashMap<String, String> = HashMap::new();
        let styles_auto = find("styles", "automatic-styles").and_then(children).map(|c| c.1).unwrap_or_default();
        let mut from_styles = Vec::new();
        for (t, full) in &styles_auto {
            let name = t.attr("style:name").unwrap_or_default().into_owned();
            match names.get(&format!("{}\u{1}{name}", t.name)) {
                Some(f) if f == full => continue,
                _ if taken.contains(&name) => {
                    let mut n = 1;
                    while taken.contains(&format!("{name}_{n}")) || renames.values().any(|v| *v == format!("{name}_{n}")) {
                        n += 1;
                    }
                    renames.insert(name.clone(), format!("{name}_{n}"));
                }
                _ => {}
            }
            from_styles.push(*full);
        }
        for full in from_styles {
            auto.push_str(&rename(full, &renames));
        }
        if !auto.is_empty() {
            let x = self.embed(&auto, prefix)?;
            let _ = write!(out, "<office:automatic-styles>{x}</office:automatic-styles>");
        }
        if let Some(x) = find("styles", "master-styles") {
            out.push_str(&self.embed(&rename(x, &renames), prefix)?);
        }
        if let Some(x) = find("content", "body") {
            out.push_str(&self.embed(x, prefix)?);
        }
        Ok((attrs, out))
    }

    /// `src` with the package files its elements link to put inline.
    fn embed(&mut self, src: &str, prefix: &str) -> Result<String, Error> {
        if !src.contains("xlink:href") {
            return Ok(src.to_string());
        }
        let mut out = String::with_capacity(src.len());
        let mut r = Reader::new(src);
        loop {
            match r.next() {
                Ev::Eof => break,
                Ev::Start(t) => self.embed_tag(&mut out, &t, false, prefix)?,
                Ev::Empty(t) => self.embed_tag(&mut out, &t, true, prefix)?,
                Ev::End(n) => {
                    out.push_str("</");
                    out.push_str(n);
                    out.push('>');
                }
                Ev::Text(t) => out.push_str(&esc_text(&t)),
            }
        }
        Ok(out)
    }

    fn embed_tag(&mut self, out: &mut String, t: &Tag, empty: bool, prefix: &str) -> Result<(), Error> {
        let target = t.attr("xlink:href").and_then(|h| {
            let h = h.trim();
            let inside = !(h.is_empty() || h.starts_with(['#', '/']) || h.starts_with("../") || h.contains(':'));
            inside.then(|| format!("{prefix}{}", h.trim_start_matches("./").trim_end_matches('/')))
        });
        let Some(path) = target else {
            write_tag(out, t, empty);
            return Ok(());
        };
        let dir = format!("{path}/");
        let inner = if self.manifest.contains_key(&dir) || self.z.contains(&format!("{dir}content.xml")) {
            let mime = self.manifest.get(&dir).cloned().unwrap_or_default();
            let (attrs, inner) = self.join(&dir, &mime, None)?;
            format!("<office:document{}>{inner}</office:document>", attrs_xml(&attrs))
        } else if let Ok(data) = self.z.read(&path) {
            self.used.insert(path);
            format!("<office:binary-data>{}</office:binary-data>", base64_encode(&data))
        } else {
            write_tag(out, t, empty);
            return Ok(());
        };
        out.push('<');
        out.push_str(t.name);
        for (k, v) in t.attrs().filter(|(k, _)| !k.starts_with("xlink:")) {
            let _ = write!(out, " {k}=\"{}\"", esc(&v));
        }
        out.push('>');
        out.push_str(&inner);
        if empty {
            let _ = write!(out, "</{}>", t.name);
        }
        Ok(())
    }
}

/// `src` with the style names in `renames` changed where they are named
/// or used.
fn rename(src: &str, renames: &HashMap<String, String>) -> String {
    if renames.is_empty() {
        return src.to_string();
    }
    let mut out = String::with_capacity(src.len());
    let mut r = Reader::new(src);
    loop {
        let (t, empty) = match r.next() {
            Ev::Eof => break,
            Ev::Start(t) => (t, false),
            Ev::Empty(t) => (t, true),
            Ev::End(n) => {
                let _ = write!(out, "</{n}>");
                continue;
            }
            Ev::Text(x) => {
                out.push_str(&esc_text(&x));
                continue;
            }
        };
        if !t.attrs().any(|(k, v)| local(k).ends_with("name") && renames.contains_key(v.as_ref())) {
            write_tag(&mut out, &t, empty);
            continue;
        }
        out.push('<');
        out.push_str(t.name);
        for (k, v) in t.attrs() {
            let v = if local(k).ends_with("name") { renames.get(v.as_ref()).map_or(v.as_ref(), String::as_str) } else { v.as_ref() };
            let _ = write!(out, " {k}=\"{}\"", esc(v));
        }
        out.push_str(if empty { "/>" } else { ">" });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_sheet::CellRef;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0\x1f\x15\xc4\x89\0\0\0\rIDATx\x9cc\xf8\xcf\xc0\xf0\x1f\0\x05\0\x01\xff\x89\x99=\x1d\0\0\0\0IEND\xaeB`\x82";

    const A1: CellRef = CellRef { row: 0, col: 0 };

    #[test]
    fn odd_encoding_declaration() {
        // A cut file: the lossy decoding ends in U+FFFD where the quote goes.
        assert_eq!(decode(b"encoding=\xe2"), "encoding=\u{e2}");
        assert_eq!(decode(b"<?xml encoding='latin1'?>\xe9"), "<?xml encoding='latin1'?>\u{e9}");
    }

    fn bold(wb: &apex_sheet::Workbook) -> bool {
        wb.styles.get(wb.sheets[0].get(A1).unwrap().style).bold
    }

    fn flat_sheet() -> String {
        format!(r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:script="urn:oasis:names:tc:opendocument:xmlns:script:1.0" xmlns:ooo="http://openoffice.org/2004/office" office:version="1.3" office:mimetype="application/vnd.oasis.opendocument.spreadsheet">
 <office:meta><dc:title>Plano</dc:title></office:meta>
 <office:scripts><office:script script:language="ooo:Basic"><ooo:libraries><ooo:library-embedded ooo:name="Standard"><ooo:module ooo:name="Module1"><ooo:source-code>Sub Hola
  MsgBox &quot;a &lt; b&quot;
End Sub</ooo:source-code></ooo:module></ooo:library-embedded></ooo:libraries></office:script></office:scripts>
 <office:automatic-styles>
  <style:style style:name="ce1" style:family="table-cell"><style:text-properties fo:font-weight="bold"/></style:style>
 </office:automatic-styles>
 <office:body><office:spreadsheet><table:table table:name="Hoja1">
  <table:table-row><table:table-cell table:style-name="ce1" office:value-type="string"><text:p>Negrita</text:p></table:table-cell><table:table-cell office:value-type="float" office:value="42"><text:p>42</text:p></table:table-cell></table:table-row>
  <table:table-row><table:table-cell><draw:frame table:end-cell-address="Hoja1.B3" svg:width="1cm" svg:height="1cm" svg:x="0cm" svg:y="0cm"><draw:image>
   <office:binary-data>{}</office:binary-data>
  </draw:image></draw:frame></table:table-cell></table:table-row>
 </table:table></office:spreadsheet></office:body>
</office:document>"#, base64_encode(PNG))
    }

    #[test]
    fn objects_nested_deeply_are_refused() {
        let src = flat_sheet();
        let (head, rest) = src.split_once("<office:body>").unwrap();
        let open = r#"<office:body><office:text><draw:frame><draw:object><office:document office:mimetype="application/vnd.oasis.opendocument.text">"#;
        let close = "</office:document></draw:object></draw:frame></office:text></office:body>";
        let tail = rest.split_once("</office:body>").unwrap().1;
        let deep = format!("{head}{}{}{tail}", open.repeat(5000), close.repeat(5000));
        assert!(to_package(deep.as_bytes()).is_err());
        let few = format!("{head}{}{}{tail}", open.repeat(3), close.repeat(3));
        assert!(to_package(few.as_bytes()).is_ok());
    }

    #[test]
    fn reads_and_writes_flat_spreadsheets() {
        let src = flat_sheet();
        assert_eq!(sniff(src.as_bytes()).as_deref(), Some("application/vnd.oasis.opendocument.spreadsheet"));
        let wb = crate::ods::read(&to_package(src.as_bytes()).unwrap()).unwrap();
        let sh = &wb.sheets[0];
        assert_eq!(sh.name, "Hoja1");
        assert_eq!(wb.display(0, A1).0, "Negrita");
        assert!(bold(&wb));
        assert_eq!(wb.display(0, CellRef::new(0, 1)).0, "42");
        assert_eq!(sh.images.len(), 1);
        assert_eq!(&sh.images[0].data[..], PNG);
        assert_eq!(wb.meta.title, "Plano");
        assert!(wb.macros.odf.iter().any(|p| p.path == "Basic/Standard/Module1.xml"));
        // And back: one XML file with everything inline.
        let flat = from_package(&crate::ods::write(&wb)).unwrap();
        let text = String::from_utf8(flat.clone()).unwrap();
        assert!(text.contains("<office:binary-data>") && text.contains("<ooo:module ooo:name=\"Module1\">"));
        assert!(text.contains("MsgBox &quot;a &lt; b&quot;") || text.contains("MsgBox \"a &lt; b\""));
        assert!(!text.contains("Pictures/"), "pictures inline");
        let again = crate::ods::read(&to_package(&flat).unwrap()).unwrap();
        assert_eq!(again.display(0, A1).0, "Negrita");
        assert!(bold(&again));
        assert_eq!(&again.sheets[0].images[0].data[..], PNG);
        assert_eq!(again.macros.odf.len(), wb.macros.odf.len());
    }

    #[test]
    fn keeps_package_files_with_no_place_in_the_flat_file() {
        let mut z = ZipWriter::new();
        z.add("mimetype", b"application/vnd.oasis.opendocument.text", false);
        z.add("content.xml", br#"<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" office:version="1.3"><office:automatic-styles><style:style style:name="P1" style:family="paragraph"/></office:automatic-styles><office:body><office:text><text:p text:style-name="P1">x</text:p></office:text></office:body></office:document-content>"#, true);
        z.add("styles.xml", br#"<office:document-styles xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" office:version="1.3"><office:automatic-styles><style:style style:name="P1" style:family="paragraph"><style:paragraph-properties/></style:style></office:automatic-styles><office:master-styles><style:master-page style:name="Standard"><style:header><text:p text:style-name="P1">h</text:p></style:header></style:master-page></office:master-styles></office:document-styles>"#, true);
        z.add("apexdesk/model.xml", b"<model/>", true);
        let flat = String::from_utf8(from_package(&z.finish()).unwrap()).unwrap();
        // The header's style, a different "P1", is renamed.
        assert!(flat.contains(r#"<style:style style:name="P1_1" style:family="paragraph"><style:paragraph-properties/>"#), "{flat}");
        assert!(flat.contains(r#"<text:p text:style-name="P1_1">h</text:p>"#));
        assert!(flat.contains(r#"<text:p text:style-name="P1">x</text:p>"#));
        assert!(flat.contains(r#"apexdesk:path="apexdesk/model.xml""#));
        let pkg = to_package(flat.as_bytes()).unwrap();
        let z = ZipReader::new(&pkg).unwrap();
        assert_eq!(z.read("apexdesk/model.xml").unwrap(), b"<model/>");
        assert!(z.read_string("styles.xml").unwrap().contains("P1_1"));
    }

    #[test]
    fn nested_documents_become_objects() {
        let src = r#"<office:document xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" office:mimetype="application/vnd.oasis.opendocument.text"><office:body><office:text><draw:frame><draw:object>
 <office:document office:mimetype="application/vnd.oasis.opendocument.chart"><office:body><office:chart/></office:body></office:document></draw:object></draw:frame></office:text></office:body></office:document>"#;
        let pkg = to_package(src.as_bytes()).unwrap();
        let z = ZipReader::new(&pkg).unwrap();
        assert!(z.read_string("content.xml").unwrap().contains(r#"<draw:object xlink:href="./Object 1""#));
        assert!(z.read_string("Object 1/content.xml").unwrap().contains("<office:chart/>"));
        assert!(z.read_string("META-INF/manifest.xml").unwrap().contains(r#"manifest:full-path="Object 1/" manifest:media-type="application/vnd.oasis.opendocument.chart""#));
        let back = String::from_utf8(from_package(&pkg).unwrap()).unwrap();
        assert!(back.contains(r#"<draw:object><office:document"#) && back.contains("<office:chart/>"), "{back}");
    }
}
