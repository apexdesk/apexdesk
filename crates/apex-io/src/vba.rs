//! VBA projects (MS-OVBA): the modules of a vbaProject.bin (name, kind and
//! source code) and the project written back with modules changed, added,
//! renamed or removed.
//!
//! A project keeps each module twice: its source, compressed, and p-code
//! compiled for one version of Office (`_VBA_PROJECT`, the `__SRP_`
//! streams and each module's PerformanceCache). A rewritten project drops
//! the compiled parts, as the specification asks of writers
//! (`_VBA_PROJECT` with version 0xFFFF, modules without cache): Office
//! then compiles the modules again from their source when it opens them.

use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use crate::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleKind {
    /// A standard (procedural) module.
    Standard,
    Class,
    /// The module of a sheet, the workbook or the document (ThisWorkbook,
    /// Sheet1, ThisDocument).
    Document,
    /// The code of a UserForm (its design is kept, not shown).
    Form,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VbaModule {
    pub name: String,
    pub kind: ModuleKind,
    /// Source code with `\n` line ends, without the header lines the VBA
    /// editor hides.
    pub code: String,
    /// Those hidden lines: `VERSION`/`BEGIN`…`END` of classes and the
    /// `Attribute VB_…` of the module (VB_Name follows `name` on save).
    pub attributes: String,
    /// Index of the module in the project as read (None: added).
    origin: Option<usize>,
    /// A form's design to write, its storage's streams as
    /// [`VbaProject::form_streams`] gives them (None: as the file has it).
    pub form: Option<FormStreams>,
}

/// The streams of a form's storage, by path inside it.
pub type FormStreams = std::collections::HashMap<String, Vec<u8>>;

impl VbaModule {
    /// An empty standard module.
    pub fn standard(name: &str) -> VbaModule {
        VbaModule { name: name.to_string(), kind: ModuleKind::Standard, code: String::new(), attributes: format!("Attribute VB_Name = \"{name}\"\n"), origin: None, form: None }
    }

    /// An empty module of a worksheet (`name`: its code name).
    pub fn sheet(name: &str) -> VbaModule {
        VbaModule { name: name.to_string(), kind: ModuleKind::Document, code: String::new(), attributes: document_attributes(name, EXCEL_SHEET), origin: None, form: None }
    }

    /// A new form: its code module, with `design` (the streams of its
    /// storage) as its design.
    pub fn form(name: &str, design: FormStreams) -> VbaModule {
        let attributes = format!(
            "Attribute VB_Name = \"{name}\"\nAttribute VB_Base = \"0{}{}\"\nAttribute VB_GlobalNameSpace = False\nAttribute VB_Creatable = False\nAttribute VB_PredeclaredId = True\nAttribute VB_Exposed = False\nAttribute VB_TemplateDerived = False\nAttribute VB_Customizable = False\n",
            new_guid(),
            new_guid()
        );
        VbaModule { name: name.to_string(), kind: ModuleKind::Form, code: String::new(), attributes, origin: None, form: Some(design) }
    }

    /// An empty class module.
    pub fn class(name: &str) -> VbaModule {
        VbaModule { name: name.to_string(), kind: ModuleKind::Class, code: String::new(), attributes: class_attributes(name, &[]), origin: None, form: None }
    }

    /// The extension the VBA editor exports it with.
    pub fn file_ext(&self) -> &'static str {
        match self.kind {
            ModuleKind::Standard => "bas",
            ModuleKind::Form => "frm",
            ModuleKind::Class | ModuleKind::Document => "cls",
        }
    }

    /// The module as the VBA editor exports it (File > Export File): a
    /// `.bas` or `.cls` file in the code page `cp`, CRLF line ends. A form's
    /// code only: its design isn't kept apart.
    pub fn export(&self, cp: u16) -> Vec<u8> {
        let attributes = set_vb_name(&self.attributes, &self.name);
        let head = match self.kind {
            ModuleKind::Standard => format!("Attribute VB_Name = \"{}\"\n", self.name),
            ModuleKind::Class | ModuleKind::Document | ModuleKind::Form => {
                let keep: String = attributes.lines().filter(|l| {
                    let t = l.trim_start();
                    t.starts_with("Attribute VB_") && !["VB_Base", "VB_TemplateDerived", "VB_Customizable"].iter().any(|k| t["Attribute ".len()..].starts_with(k))
                }).map(|l| format!("{l}\n")).collect();
                format!("VERSION 1.0 CLASS\nBEGIN\n  MultiUse = -1  'True\nEND\n{keep}")
            }
        };
        let mut code = self.code.clone();
        if !code.is_empty() && !code.ends_with('\n') {
            code.push('\n');
        }
        crate::ole::encode_cp(&crlf(&format!("{head}{code}")), cp)
    }

    /// A module from an exported `.bas` or `.cls` file (UTF-8, or else the
    /// code page `cp`); `name` if the file doesn't say its own. A `.cls`
    /// comes in as a class module, as in the VBA editor.
    pub fn import(bytes: &[u8], name: &str, cp: u16) -> VbaModule {
        let text = match std::str::from_utf8(bytes) {
            Ok(t) => t.trim_start_matches('\u{feff}').to_string(),
            Err(_) => crate::ole::decode_cp(bytes, cp),
        };
        let (head, code) = split_header(&normalize(&text));
        let class = head.starts_with("VERSION ");
        let name = head.lines().find_map(|l| l.trim().strip_prefix("Attribute VB_Name = ")).map(|v| v.trim().trim_matches('"').to_string()).filter(|n| is_identifier(n)).unwrap_or_else(|| name.to_string());
        if class {
            // Its own settings (PredeclaredId, Exposed...) over the defaults.
            let own: Vec<&str> = head.lines().map(str::trim).filter(|l| l.starts_with("Attribute VB_") && !l.starts_with("Attribute VB_Name")).collect();
            VbaModule { name: name.clone(), kind: ModuleKind::Class, code, attributes: class_attributes(&name, &own), origin: None, form: None }
        } else {
            let mut m = VbaModule::standard(&name);
            m.code = code;
            m
        }
    }

    /// Whether this is a document module extending `base` (a workbook's
    /// or a worksheet's).
    fn extends(&self, base: &str) -> bool {
        self.kind == ModuleKind::Document && self.attributes.lines().any(|l| l.trim_start().starts_with("Attribute VB_Base") && l.contains(&base[1..]))
    }
}

/// A dir stream record: id and data (PROJECTVERSION has a size field that
/// doesn't count its data: `size` keeps it).
#[derive(Clone, Debug, PartialEq)]
struct Rec {
    id: u16,
    size: u32,
    data: Vec<u8>,
}

impl Rec {
    fn new(id: u16, data: Vec<u8>) -> Rec {
        Rec { id, size: data.len() as u32, data }
    }
}

/// A module as read: where its records are and its stream.
#[derive(Clone, Debug)]
struct ReadModule {
    recs: std::ops::Range<usize>,
    stream: String,
    module: VbaModule,
}

/// A reference of a project (Tools > References): a type library, another
/// project or a control library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reference {
    pub name: String,
    /// The library's GUID (`{...}`, upper case), when it has one.
    pub guid: Option<String>,
    /// Where it was found on the machine that saved the file.
    pub path: String,
    pub description: String,
    /// Another VBA project (a workbook or add-in), not a library.
    pub project: bool,
}

impl Reference {
    /// From a LibidReference (`*\G{guid}#ver#lcid#path#description`) or a
    /// project's (`*\C` / `*\D` and a path).
    fn from_libid(name: String, libid: &str, project: bool) -> Reference {
        let body = libid.strip_prefix("*\\").map(|s| s.get(1..).unwrap_or("")).unwrap_or(libid);
        let parts: Vec<&str> = body.split('#').collect();
        let guid = parts.first().filter(|g| g.starts_with('{') && g.ends_with('}')).map(|g| g.to_ascii_uppercase());
        let (path, description) = if guid.is_some() { (parts.get(3).copied().unwrap_or(""), parts.get(4).copied().unwrap_or("")) } else { (body, "") };
        Reference { name, guid, path: path.to_string(), description: description.to_string(), project }
    }
}

#[derive(Clone, Debug)]
pub struct VbaProject {
    /// The project's name (VBAProject) and description, as in Tools >
    /// Project Properties; written back when changed.
    pub name: String,
    pub description: String,
    /// Code page of names and source code.
    pub code_page: u16,
    pub modules: Vec<VbaModule>,
    /// Locked with a password or for viewing: shown, not changed.
    pub protected: bool,
    /// The lock to write: `None` keeps the file's, `Some(None)` takes it
    /// away, `Some(Some(password))` locks it with that password.
    lock: Option<Option<String>>,
    read_name: String,
    read_description: String,
    bin: Vec<u8>,
    records: Vec<Rec>,
    /// Records before the first module and from the end of the last one.
    head: usize,
    tail: usize,
    read: Vec<ReadModule>,
}

// Record ids of the dir stream (MS-OVBA 2.3.4.2).
const PROJECTCODEPAGE: u16 = 0x0003;
const PROJECTNAME: u16 = 0x0004;
const PROJECTDOCSTRING: u16 = 0x0005;
const PROJECTDOCSTRING_UNICODE: u16 = 0x0040;
const PROJECTVERSION: u16 = 0x0009;
const PROJECTCONSTANTS: u16 = 0x000C;
const REFERENCEREGISTERED: u16 = 0x000D;
const REFERENCEPROJECT: u16 = 0x000E;
const PROJECTMODULES: u16 = 0x000F;
const MODULENAME: u16 = 0x0019;
const MODULESTREAMNAME: u16 = 0x001A;
const MODULETYPE_PROCEDURAL: u16 = 0x0021;
const MODULE_TERMINATOR: u16 = 0x002B;
const MODULEOFFSET: u16 = 0x0031;
const MODULESTREAMNAME_UNICODE: u16 = 0x0032;
const MODULENAMEUNICODE: u16 = 0x0047;

/// `_VBA_PROJECT` as written: no p-code, version 0xFFFF (2.3.4.1).
const EMPTY_VBA_PROJECT: [u8; 7] = [0xCC, 0x61, 0xFF, 0xFF, 0x00, 0x00, 0x00];

type Ole<R> = cfb::CompoundFile<R>;

fn stream<R: Read + std::io::Seek>(c: &mut Ole<R>, path: &str) -> Option<Vec<u8>> {
    let mut s = c.open_stream(path).ok()?;
    let mut v = Vec::new();
    s.read_to_end(&mut v).ok()?;
    Some(v)
}

fn u16_at(d: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(i..i + 2)?.try_into().ok()?))
}

fn u32_at(d: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(i..i + 4)?.try_into().ok()?))
}

fn utf16(d: &[u8]) -> String {
    let units: Vec<u16> = d.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&units)
}

fn to_utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

// --- compression (2.4.1) -----------------------------------------------------------

/// Bits of a copy token's offset at `diff` bytes into a chunk.
fn offset_bits(diff: usize) -> u32 {
    let mut n = 0;
    while (1usize << n) < diff {
        n += 1;
    }
    n.max(4)
}

/// Decompresses a CompressedContainer (None if it isn't one).
pub fn decompress(src: &[u8]) -> Option<Vec<u8>> {
    if src.first() != Some(&1) {
        return None;
    }
    let mut out = Vec::with_capacity(src.len() * 3);
    let mut pos = 1;
    while pos + 2 <= src.len() {
        let header = u16_at(src, pos)?;
        let end = (pos + (header & 0x0FFF) as usize + 3).min(src.len());
        pos += 2;
        let start = out.len();
        if header & 0x8000 == 0 {
            // A raw chunk: 4096 bytes (fewer at the end of a stream).
            let n = (end - pos).min(4096);
            out.extend_from_slice(&src[pos..pos + n]);
            pos += n;
            continue;
        }
        while pos < end {
            let flags = src[pos];
            pos += 1;
            for bit in 0..8 {
                // A chunk holds at most 4096 bytes once decompressed.
                if out.len() - start >= 4096 {
                    pos = end;
                }
                if pos >= end {
                    break;
                }
                if flags >> bit & 1 == 0 {
                    out.push(src[pos]);
                    pos += 1;
                } else {
                    let token = u16_at(src, pos)? as usize;
                    pos += 2;
                    let bits = offset_bits(out.len() - start);
                    let len = ((token & (0xFFFF >> bits)) + 3).min(4096 - (out.len() - start));
                    let off = (token >> (16 - bits)) + 1;
                    let from = out.len().checked_sub(off)?;
                    if from < start {
                        return None;
                    }
                    for k in 0..len {
                        out.push(out[from + k]);
                    }
                }
            }
        }
        pos = end;
    }
    Some(out)
}

/// Compresses `data` into a CompressedContainer.
pub fn compress(data: &[u8]) -> Vec<u8> {
    let mut out = vec![1u8];
    for chunk in data.chunks(4096) {
        let mut body = Vec::with_capacity(chunk.len() + chunk.len() / 8 + 1);
        let mut pos = 0;
        while pos < chunk.len() {
            let flag_at = body.len();
            body.push(0u8);
            for bit in 0..8 {
                if pos >= chunk.len() {
                    break;
                }
                let bits = offset_bits(pos);
                let max_len = (0xFFFF >> bits) + 3;
                // The longest match before `pos` (the nearest on ties), as
                // the specification measures it: up to the end of the chunk,
                // then cut to what a token holds.
                let limit = chunk.len() - pos;
                let (mut best, mut best_off) = (0, 0);
                for from in (0..pos).rev() {
                    let mut n = 0;
                    while n < limit && chunk[from + n] == chunk[pos + n] {
                        n += 1;
                    }
                    if n > best {
                        best = n;
                        best_off = pos - from;
                        if n == limit {
                            break;
                        }
                    }
                }
                let best = best.min(max_len);
                if best >= 3 {
                    let token = ((best_off - 1) << (16 - bits)) | (best - 3);
                    body.extend_from_slice(&(token as u16).to_le_bytes());
                    body[flag_at] |= 1 << bit;
                    pos += best;
                } else {
                    body.push(chunk[pos]);
                    pos += 1;
                }
            }
        }
        if body.len() > 4096 {
            // Not worth compressing: a raw chunk.
            out.extend_from_slice(&(0x3000u16 | (chunk.len() as u16 + 2 - 3)).to_le_bytes());
            out.extend_from_slice(chunk);
        } else {
            out.extend_from_slice(&(0xB000u16 | (body.len() as u16 + 2 - 3)).to_le_bytes());
            out.extend_from_slice(&body);
        }
    }
    out
}

// --- dir stream ----------------------------------------------------------------------

fn parse_records(d: &[u8]) -> Option<Vec<Rec>> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos + 6 <= d.len() {
        let id = u16_at(d, pos)?;
        let size = u32_at(d, pos + 2)?;
        // PROJECTVERSION: a Reserved field of 4, then 6 bytes.
        let n = if id == PROJECTVERSION { 6 } else { size as usize };
        let data = d.get(pos + 6..pos + 6 + n)?.to_vec();
        pos += 6 + n;
        out.push(Rec { id, size, data });
    }
    Some(out)
}

fn write_records(recs: &[Rec]) -> Vec<u8> {
    let mut out = Vec::new();
    for r in recs {
        out.extend_from_slice(&r.id.to_le_bytes());
        out.extend_from_slice(&r.size.to_le_bytes());
        out.extend_from_slice(&r.data);
    }
    out
}

// --- PROJECT stream ------------------------------------------------------------------

/// Decrypts a CMG, DPB or GC value of the PROJECT stream (2.4.3.3).
fn decrypt(hex: &str) -> Option<Vec<u8>> {
    let hex = hex.trim().trim_matches('"');
    let enc: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok()).collect::<Option<_>>()?;
    let seed = *enc.first()?;
    let (version_enc, key_enc) = (*enc.get(1)?, *enc.get(2)?);
    let (mut plain1, mut enc1, mut enc2) = (seed ^ key_enc, key_enc, version_enc);
    let mut pos = 3;
    let mut next = || {
        let b = *enc.get(pos)?;
        pos += 1;
        let d = b ^ enc2.wrapping_add(plain1);
        enc2 = enc1;
        enc1 = b;
        plain1 = d;
        Some(d)
    };
    for _ in 0..(seed & 6) / 2 {
        next()?;
    }
    let len = u32::from_le_bytes([next()?, next()?, next()?, next()?]) as usize;
    (0..len.min(enc.len())).map(|_| next()).collect()
}

/// Whether the PROJECT stream locks the project (a password, or locked
/// for viewing).
fn is_locked(project: &str) -> bool {
    let value = |key: &str| project.lines().find_map(|l| l.strip_prefix(key).and_then(|v| v.strip_prefix('='))).map(str::to_string);
    let state = value("CMG").and_then(|v| decrypt(&v)).and_then(|d| u32_at(&d, 0)).unwrap_or(0);
    let password = value("DPB").and_then(|v| decrypt(&v)).is_some_and(|d| d.len() > 1 || d.first().is_some_and(|&b| b != 0));
    let hidden = value("GC").and_then(|v| decrypt(&v)).is_some_and(|d| d.first() == Some(&0));
    state != 0 || password || hidden
}

/// The kind of each module the PROJECT stream names (lower-case name).
fn kinds(project: &str) -> Vec<(String, ModuleKind)> {
    let mut out = Vec::new();
    for line in project.lines() {
        if line.starts_with('[') {
            break;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let kind = match key {
            "Module" => ModuleKind::Standard,
            "Class" => ModuleKind::Class,
            "BaseClass" => ModuleKind::Form,
            "Document" => ModuleKind::Document,
            _ => continue,
        };
        let name = value.split('/').next().unwrap_or(value).trim();
        out.push((name.to_lowercase(), kind));
    }
    out
}

/// Splits the hidden header off a module's source (lines end in `\n`).
fn split_header(src: &str) -> (String, String) {
    let mut at = 0;
    let mut in_block = false;
    for (i, line) in src.split_inclusive('\n').enumerate() {
        let t = line.trim();
        let header = if i == 0 && t.starts_with("VERSION ") {
            true
        } else if in_block {
            if t == "END" {
                in_block = false;
            }
            true
        } else if t == "BEGIN" && at > 0 {
            in_block = true;
            true
        } else {
            t.starts_with("Attribute VB_")
        };
        if !header {
            break;
        }
        at += line.len();
    }
    (src[..at].to_string(), src[at..].to_string())
}

fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn crlf(text: &str) -> String {
    let t = normalize(text);
    let mut out = t.replace('\n', "\r\n");
    if !out.is_empty() && !out.ends_with("\r\n") {
        out.push_str("\r\n");
    }
    out
}

// --- reading -------------------------------------------------------------------------

/// Reads the project of a vbaProject.bin.
pub fn read(bin: &[u8]) -> Result<VbaProject, Error> {
    let bad = || Error::new("not a valid VBA project");
    let mut ole = cfb::CompoundFile::open(Cursor::new(bin)).map_err(|e| Error::new(format!("invalid VBA project: {e}")))?;
    let dir = stream(&mut ole, "/VBA/dir").and_then(|d| decompress(&d)).ok_or_else(bad)?;
    let records = parse_records(&dir).ok_or_else(bad)?;
    let find = |id: u16| records.iter().find(|r| r.id == id);
    let code_page = find(PROJECTCODEPAGE).and_then(|r| u16_at(&r.data, 0)).unwrap_or(1252);
    let name = find(PROJECTNAME).map(|r| crate::ole::decode_cp(&r.data, code_page)).unwrap_or_default();
    let description = find(PROJECTDOCSTRING_UNICODE).map(|r| utf16(&r.data)).filter(|s| !s.is_empty()).or_else(|| find(PROJECTDOCSTRING).map(|r| crate::ole::decode_cp(&r.data, code_page))).unwrap_or_default();
    let project = stream(&mut ole, "/PROJECT").map(|d| crate::ole::decode_cp(&d, code_page)).unwrap_or_default();
    let kinds = kinds(&project);
    let head = records.iter().position(|r| r.id == MODULENAME).unwrap_or(records.len());
    let mut read = Vec::new();
    let mut i = head;
    // Each module has a stream of its own, and their text is bounded as a
    // whole: a dir listing one big stream many times would copy it again.
    let mut streams = std::collections::HashSet::new();
    let mut text_left: usize = 256 << 20;
    while i < records.len() && records[i].id == MODULENAME {
        let start = i;
        while i < records.len() && records[i].id != MODULE_TERMINATOR {
            i += 1;
        }
        i = (i + 1).min(records.len());
        let recs = &records[start..i];
        let get = |id: u16| recs.iter().find(|r| r.id == id);
        let mbcs = get(MODULENAME).map(|r| crate::ole::decode_cp(&r.data, code_page)).unwrap_or_default();
        let name = get(MODULENAMEUNICODE).map(|r| utf16(&r.data)).filter(|s| !s.is_empty()).unwrap_or(mbcs);
        let stream_name = get(MODULESTREAMNAME_UNICODE)
            .map(|r| utf16(&r.data))
            .filter(|s| !s.is_empty())
            .or_else(|| get(MODULESTREAMNAME).map(|r| crate::ole::decode_cp(&r.data, code_page)))
            .unwrap_or_else(|| name.clone());
        if !streams.insert(stream_name.to_lowercase()) {
            return Err(bad());
        }
        let offset = get(MODULEOFFSET).and_then(|r| u32_at(&r.data, 0)).unwrap_or(0) as usize;
        let procedural = recs.iter().any(|r| r.id == MODULETYPE_PROCEDURAL);
        let kind = kinds.iter().find(|(n, _)| *n == name.to_lowercase()).map(|k| k.1).unwrap_or(if procedural { ModuleKind::Standard } else { ModuleKind::Class });
        let text = stream(&mut ole, &format!("/VBA/{stream_name}")).and_then(|d| decompress(d.get(offset..)?)).map(|d| normalize(&crate::ole::decode_cp(&d, code_page))).unwrap_or_default();
        text_left = text_left.checked_sub(text.len()).ok_or_else(bad)?;
        let (attributes, code) = split_header(&text);
        let module = VbaModule { name, kind, code, attributes, origin: Some(read.len()), form: None };
        read.push(ReadModule { recs: start..i, stream: stream_name, module });
    }
    let modules = read.iter().map(|m| m.module.clone()).collect();
    Ok(VbaProject { read_name: name.clone(), read_description: description.clone(), name, description, code_page, modules, protected: is_locked(&project), lock: None, bin: bin.to_vec(), tail: i, records, head, read })
}

/// The digital signature streams of a vbaProject.bin (the newest kind
/// first): none when the project is unsigned. (In OOXML files they are
/// parts of their own next to the project instead.)
pub fn signatures(bin: &[u8]) -> Vec<Vec<u8>> {
    let Ok(mut ole) = cfb::CompoundFile::open(Cursor::new(bin)) else { return Vec::new() };
    ["/\u{5}DigitalSignatureExt", "/\u{5}DigitalSignatureV3", "/\u{5}DigitalSignatureEx", "/\u{5}DigitalSignatureAgile", "/\u{5}DigitalSignature"].iter().filter_map(|n| stream(&mut ole, n)).collect()
}

// --- what a signature signs (2.4.2) ----------------------------------------------------

/// Whether the content a signature signs (its SpcIndirectDataContent)
/// carries a hash of this project's code as it is now: false for a
/// signature made for other code, and for kinds of hash not computed
/// here (the V3 hash; the Agile hash of a project with forms), which are
/// then not verified, never taken as good.
///
/// The hashed data (2.4.2.1, Content Normalized Data) is the project's
/// name and constants, its references and every line of every module's
/// source except the `Attribute` lines. Office's references part differs
/// from the specification's text, so each known reading is tried; all of
/// them cover the code in full. V1 hashes it with MD5; the Agile hash
/// (2.4.2.4) appends the forms' data and uses the signature's digest.
pub fn signed_for(bin: &[u8], signed: &[u8]) -> bool {
    use md5::Digest as _;
    let Some((datas, forms)) = content_normalized(bin) else { return false };
    let has = |h: &[u8]| !h.is_empty() && signed.windows(h.len()).any(|w| w == h);
    datas.iter().any(|d| {
        if has(&md5::Md5::digest(d)) {
            return true;
        }
        !forms && (has(&sha1::Sha1::digest_sha(d)) || has(&sha2::Sha256::digest_sha(d)) || has(&sha2::Sha384::digest_sha(d)) || has(&sha2::Sha512::digest_sha(d)))
    })
}

/// Whether the content a signature signs carries the V3 hash (2.4.2.7)
/// of this project as it is now. Unlike the older hashes, which join the
/// lines of all modules into one run of text, it tells lines, modules and
/// the kinds of modules apart: with those, a line break moved (a comment
/// becoming code) or a module made one whose procedures run on opening
/// still matches, so only this one says the code is what was signed.
pub fn signed_for_v3(bin: &[u8], signed: &[u8]) -> bool {
    let Some(datas) = v3_normalized(bin) else { return false };
    let has = |h: &[u8]| signed.windows(h.len()).any(|w| w == h);
    datas.iter().any(|d| has(&sha2::Sha256::digest_sha(d)) || has(&sha1::Sha1::digest_sha(d)) || has(&sha2::Sha384::digest_sha(d)) || has(&sha2::Sha512::digest_sha(d)))
}

/// The SHA-256 V3 hash (2.4.2.7) to sign for this project, None when it
/// can't be told for sure: the readings of what it covers differ (a
/// module ending in a line break; [`VbaProject::write_for_signing`]
/// writes none) or the project is one it can't vouch for.
pub fn v3_source_hash(bin: &[u8]) -> Option<Vec<u8>> {
    let datas = v3_normalized(bin)?;
    if datas.iter().any(|d| *d != datas[0]) {
        return None;
    }
    Some(sha2::Sha256::digest_sha(&datas[0]).to_vec())
}

/// The readings of the data the V3 hash covers (2.4.2.5 and 2.4.2.6): the
/// dir stream's records as the specification lists them, each module's
/// lines ended by a LF (leaving out only its `Attribute VB_Name` line and
/// the attributes every document has) and then its name, and the PROJECT
/// stream's properties but those of protection and document modules.
/// The readings differ in the empty line after a module's last line
/// break, which the specification counts and other signers don't.
///
/// None for what this can't vouch for: records it doesn't know, a module
/// named twice in the PROJECT stream or there of a kind its record
/// denies (the kind of a document module isn't hashed), and a left-out
/// line that holds a statement the interpreter would run.
fn v3_normalized(bin: &[u8]) -> Option<Vec<Vec<u8>>> {
    const DEFAULTS: [&[u8]; 7] = [
        b"Attribute VB_Base = \"0{00020820-0000-0000-C000-000000000046}\"",
        b"Attribute VB_GlobalNameSpace = False",
        b"Attribute VB_Creatable = False",
        b"Attribute VB_PredeclaredId = True",
        b"Attribute VB_Exposed = True",
        b"Attribute VB_TemplateDerived = False",
        b"Attribute VB_Customizable = True",
    ];
    let p = read(bin).ok()?;
    let mut ole = cfb::CompoundFile::open(Cursor::new(bin)).ok()?;
    let mut out = Vec::new();
    let id_size = |v: &mut Vec<u8>, r: &Rec| {
        v.extend_from_slice(&r.id.to_le_bytes());
        v.extend_from_slice(&r.size.to_le_bytes());
    };
    for r in &p.records[..p.head] {
        match r.id {
            // PROJECTCOMPATVERSION.
            0x004A => {}
            // Sys kind, code page, doc string, help file and context,
            // modules and cookie: their sizes only.
            0x0001 | 0x0003 | 0x0005 | 0x0040 | 0x0006 | 0x003D | 0x0007 | PROJECTMODULES | 0x0013 => id_size(&mut out, r),
            // LCIDs, name, flags, version, constants, reference names and
            // original references: whole.
            0x0002 | 0x0014 | PROJECTNAME | 0x0008 | PROJECTVERSION | PROJECTCONSTANTS | 0x003C | 0x0016 | 0x003E | 0x0033 => {
                id_size(&mut out, r);
                out.extend_from_slice(&r.data);
            }
            // Control, registered and project references: without the
            // record's size.
            0x002F | 0x0030 | REFERENCEREGISTERED | REFERENCEPROJECT => {
                out.extend_from_slice(&r.id.to_le_bytes());
                out.extend_from_slice(&r.data);
            }
            _ => return None,
        }
    }
    // What the PROJECT stream says each module is.
    let project = stream(&mut ole, "/PROJECT")?;
    let props: Vec<&[u8]> = project.split(|&b| b == b'\n').map(|l| l.strip_suffix(b"\r").unwrap_or(l).trim_ascii()).collect();
    let main: Vec<(&[u8], &[u8])> = props.iter().take_while(|l| !l.starts_with(b"[")).filter_map(|l| {
        let at = l.iter().position(|&b| b == b'=')?;
        Some((l[..at].trim_ascii(), &l[at + 1..]))
    }).collect();
    // Read as the reader reads them, which decides what runs on opening.
    let kinds = kinds(&crate::ole::decode_cp(&project, p.code_page));
    let listed = |name: &str| -> Vec<ModuleKind> {
        let name = name.to_lowercase();
        kinds.iter().filter(|(n, _)| *n == name).map(|k| k.1).collect()
    };
    let mut alt = out.clone();
    for m in &p.read {
        let recs = &p.records[m.recs.clone()];
        let get = |id: u16| recs.iter().find(|r| r.id == id);
        let procedural = get(MODULETYPE_PROCEDURAL).is_some();
        match listed(&m.module.name)[..] {
            [] => {}
            [kind] if (kind == ModuleKind::Standard) == procedural => {}
            _ => return None,
        }
        for id in [MODULETYPE_PROCEDURAL, 0x0025, 0x0028] {
            if let Some(r) = get(id) {
                id_size(&mut out, r);
                id_size(&mut alt, r);
            }
        }
        let offset = get(MODULEOFFSET).and_then(|r| u32_at(&r.data, 0)).unwrap_or(0) as usize;
        let src = stream(&mut ole, &format!("/VBA/{}", m.stream)).and_then(|d| decompress(d.get(offset..)?))?;
        // Lines end at a CR, a LF or both.
        let mut lines = Vec::new();
        let mut line = Vec::new();
        let mut prev = 0u8;
        for &c in &src {
            if c == b'\r' || (c == b'\n' && prev != b'\r') {
                lines.push(std::mem::take(&mut line));
            } else if c != b'\n' {
                line.push(c);
            }
            prev = c;
        }
        lines.push(line);
        let (mut hashed, mut hashed_alt) = (false, false);
        let n = lines.len();
        for (i, line) in lines.iter().enumerate() {
            let lower = line.to_ascii_lowercase();
            if lower.starts_with(b"attribute vb_name = ") {
                // Left out: so it must be only that.
                let mut quoted = false;
                for &c in line.iter() {
                    match c {
                        b'"' => quoted = !quoted,
                        b':' if !quoted => return None,
                        _ => {}
                    }
                }
                if line.trim_ascii_end().ends_with(b"_") {
                    return None;
                }
                continue;
            }
            if lower.starts_with(b"attribute") && DEFAULTS.contains(&&line[..]) {
                continue;
            }
            for (buf, f, last) in [(&mut out, &mut hashed, false), (&mut alt, &mut hashed_alt, true)] {
                if last && i + 1 == n && line.is_empty() {
                    continue;
                }
                buf.extend_from_slice(line);
                buf.push(b'\n');
                *f = true;
            }
        }
        let name = get(MODULENAMEUNICODE).map(|r| &r.data[..]).filter(|d| !d.is_empty()).or_else(|| get(MODULENAME).map(|r| &r.data[..]))?;
        for (buf, f) in [(&mut out, hashed), (&mut alt, hashed_alt)] {
            if f {
                buf.extend_from_slice(name);
                buf.push(b'\n');
            }
        }
    }
    let mut tail = vec![0x10, 0x00];
    tail.extend_from_slice(&0u32.to_le_bytes());
    // The PROJECT stream (2.4.2.6).
    for (key, value) in &main {
        if *key == b"BaseClass" {
            let name = String::from_utf8_lossy(value);
            tail.extend(normalized_storage(&mut ole, &format!("/{}", name.trim().trim_matches('"')), 0)?);
        }
        if !matches!(*key, b"ID" | b"Document" | b"CMG" | b"DPB" | b"GC") {
            tail.extend_from_slice(key);
            let mut v = *value;
            while let [b'"', rest @ ..] = v {
                v = rest;
            }
            while let [rest @ .., b'"'] = v {
                v = rest;
            }
            tail.extend_from_slice(v);
        }
    }
    if let Some(at) = props.iter().position(|l| *l == b"[Host Extender Info]") {
        tail.extend_from_slice(b"Host Extender Info");
        for l in props[at + 1..].iter().take_while(|l| !l.starts_with(b"[")) {
            if l.starts_with(b"&H") {
                tail.extend_from_slice(l);
            }
        }
    }
    Some([out, alt].into_iter().map(|mut v| {
        v.extend_from_slice(&tail);
        v
    }).collect())
}

/// A form's storage as the hashes take it (2.4.2.2): each stream padded
/// with zeros to a multiple of 1023 bytes, storages in turn.
fn normalized_storage<R: Read + std::io::Seek>(ole: &mut Ole<R>, path: &str, depth: u32) -> Option<Vec<u8>> {
    if depth > 8 {
        return None;
    }
    let entries: Vec<(String, bool)> = ole.read_storage(path).ok()?.map(|e| (e.path().to_string_lossy().into_owned(), e.is_stream())).collect();
    let mut out = Vec::new();
    for (p, is_stream) in entries {
        if is_stream {
            let mut d = stream(ole, &p)?;
            d.resize(d.len().div_ceil(1023) * 1023, 0);
            out.extend(d);
        } else {
            out.extend(normalized_storage(ole, &p, depth + 1)?);
        }
        if out.len() > 64 << 20 {
            return None;
        }
    }
    Some(out)
}

/// SHA digests (their crates use another version of the digest traits
/// than md-5).
trait DigestSha {
    fn digest_sha(d: &[u8]) -> Vec<u8>;
}

impl<T: sha2::Digest> DigestSha for T {
    fn digest_sha(d: &[u8]) -> Vec<u8> {
        T::digest(d).to_vec()
    }
}

/// The readings of the project's Content Normalized Data, and whether it
/// has forms (designers).
fn content_normalized(bin: &[u8]) -> Option<(Vec<Vec<u8>>, bool)> {
    let p = read(bin).ok()?;
    let mut ole = cfb::CompoundFile::open(Cursor::new(bin)).ok()?;
    let get = |id: u16| p.records.iter().find(|r| r.id == id).map(|r| r.data.as_slice()).unwrap_or_default();
    let mut head = get(PROJECTNAME).to_vec();
    head.extend_from_slice(get(PROJECTCONSTANTS));
    // The references: as the specification lists the fields (with and
    // without the record's size), and as Office was found to hash them.
    let (mut spec, mut sized, mut office) = (head.clone(), head.clone(), head);
    for r in p.records[..p.head].iter().take_while(|r| r.id != PROJECTMODULES) {
        if r.id != REFERENCEREGISTERED && r.id != REFERENCEPROJECT {
            continue;
        }
        for v in [&mut spec, &mut sized] {
            v.extend_from_slice(&r.id.to_le_bytes());
        }
        sized.extend_from_slice(&r.size.to_le_bytes());
        for v in [&mut spec, &mut sized] {
            v.extend_from_slice(&r.data);
        }
        if r.id == REFERENCEREGISTERED {
            office.push(0x7B);
        } else {
            office.extend(u32_at(&r.data, 0).unwrap_or(0).to_le_bytes().into_iter().take_while(|&b| b != 0));
        }
    }
    // Attribute lines are left out of the hash, as Office does, when the
    // interpreter skips them whole: not a longer word (`Attributes = 0`),
    // a label (`Attribute: ...`), a second statement after a colon or a
    // line continued onto the next. Office would leave those out too, so
    // a project with one shows as changed, never as signed.
    fn attribute_line(line: &[u8]) -> bool {
        if !line.get(..9).is_some_and(|w| w.eq_ignore_ascii_case(b"attribute")) || !line.get(9).is_some_and(|c| matches!(c, b' ' | b'\t')) {
            return false;
        }
        let mut quoted = false;
        for &c in line {
            match c {
                b'"' => quoted = !quoted,
                b':' if !quoted => return false,
                _ => {}
            }
        }
        !line.trim_ascii_end().ends_with(b" _") && !line.trim_ascii_end().ends_with(b"\t_")
    }
    let mut code = Vec::new();
    for m in &p.read {
        let offset = p.records[m.recs.clone()].iter().find(|r| r.id == MODULEOFFSET).and_then(|r| u32_at(&r.data, 0)).unwrap_or(0) as usize;
        let src = stream(&mut ole, &format!("/VBA/{}", m.stream)).and_then(|d| decompress(d.get(offset..)?))?;
        for line in src.split(|&b| b == b'\r' || b == b'\n') {
            if !attribute_line(line) {
                code.extend_from_slice(line);
            }
        }
    }
    let forms = p.modules.iter().any(|m| m.kind == ModuleKind::Form);
    Some(([spec, sized, office].into_iter().map(|mut v| {
        v.extend_from_slice(&code);
        v
    }).collect(), forms))
}

/// The modules of a vbaProject.bin (for a future interpreter, and tools).
pub fn modules(bin: &[u8]) -> Result<Vec<VbaModule>, Error> {
    read(bin).map(|p| p.modules)
}

/// The most a form's design may take in all.
const MAX_FORM_BYTES: usize = 32 << 20;

impl VbaProject {
    /// The streams of the design of the form module `module`: its storage
    /// beside the VBA one, by path inside it ("f", "o", "i08/f"...), with
    /// the `VBFrame` text as UTF-8. None when the form has no design.
    pub fn form_streams(&self, module: usize) -> Option<FormStreams> {
        let m = self.modules.get(module).filter(|m| m.kind == ModuleKind::Form)?;
        if let Some(f) = &m.form {
            return Some(f.clone());
        }
        self.stored_form(&self.read.get(m.origin?)?.stream)
    }

    /// The streams of the storage `storage` as the file has it.
    fn stored_form(&self, storage: &str) -> Option<FormStreams> {
        let mut ole = cfb::CompoundFile::open(Cursor::new(&self.bin)).ok()?;
        let root = format!("/{storage}");
        if !ole.is_storage(&root) {
            return None;
        }
        let paths: Vec<String> = ole.walk_storage(&root).ok()?.filter(|e| e.is_stream()).map(|e| e.path().to_string_lossy().replace('\\', "/")).take(4096).collect();
        let mut out = std::collections::HashMap::new();
        let mut left = MAX_FORM_BYTES;
        for p in paths {
            let Some(rel) = p.get(root.len() + 1..) else { continue };
            let mut s = ole.open_stream(&p).ok()?;
            let mut v = Vec::new();
            (&mut s).take(left as u64 + 1).read_to_end(&mut v).ok()?;
            left = left.checked_sub(v.len())?;
            if rel == "\u{3}VBFrame" {
                v = crate::ole::decode_cp(&v, self.code_page).into_bytes();
            }
            out.insert(rel.to_string(), v);
        }
        Some(out)
    }
}

/// The class of a UserForm (`Begin {…}` of its `.frm`, its storage's).
const FORM_CLSID: &str = "{C62A69F0-16DC-11CE-9E98-00AA00574A4F}";

impl VbaProject {
    /// Form module `module` as the VBA editor exports it: the `.frm` text
    /// (code page `cp`, CRLF) naming `frx`, and the `.frx` with its design.
    pub fn export_form(&self, module: usize, frx: &str, cp: u16) -> Result<(Vec<u8>, Vec<u8>), Error> {
        let m = self.modules.get(module).filter(|m| m.kind == ModuleKind::Form).ok_or_else(|| Error::new("not a form"))?;
        let mut streams = self.form_streams(module).ok_or_else(|| Error::new(format!("the form {} has no design", m.name)))?;
        let frame = streams.remove("\u{3}VBFrame").map(|f| String::from_utf8_lossy(&f).into_owned()).unwrap_or_default();
        let frame = frame_named(&normalize(&frame), &m.name);
        // The header with where its design is, before its End.
        let mut head = String::new();
        let mut placed = false;
        for line in frame.lines() {
            if !placed && line.trim() == "End" {
                head.push_str(&format!("   OleObjectBlob   =   \"{}\":0000\n", frx.replace('"', "")));
                placed = true;
            }
            if !line.trim_start().starts_with("OleObjectBlob") {
                head.push_str(line);
                head.push('\n');
            }
        }
        if !placed || !head.starts_with("VERSION ") {
            return Err(Error::new(format!("the form {} has no design", m.name)));
        }
        let attributes = set_vb_name(&m.attributes, &m.name);
        let keep: String = attributes
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                t.starts_with("Attribute VB_") && !["VB_Base", "VB_TemplateDerived", "VB_Customizable"].iter().any(|k| t["Attribute ".len()..].starts_with(k))
            })
            .map(|l| format!("{l}\n"))
            .collect();
        let mut code = m.code.clone();
        if !code.is_empty() && !code.ends_with('\n') {
            code.push('\n');
        }
        let frm = crate::ole::encode_cp(&crlf(&format!("{head}{keep}{code}")), cp);
        let io = |e: std::io::Error| Error::new(e.to_string());
        let mut ole = cfb::CompoundFile::create_with_version(cfb::Version::V3, Cursor::new(Vec::new())).map_err(io)?;
        put_form(&mut ole, "", &m.name, &streams, cp)?;
        let class = uuid::Uuid::parse_str(FORM_CLSID.trim_matches(['{', '}'])).map_err(|e| Error::new(e.to_string()))?;
        ole.set_storage_clsid("/", class).map_err(io)?;
        ole.flush().map_err(io)?;
        Ok((frm, ole.into_inner().into_inner()))
    }
}

impl VbaModule {
    /// A form from an exported `.frm` (UTF-8, or else the code page `cp`)
    /// and the `.frx` it names, which `frx` reads by its file name; `name`
    /// if the file doesn't say its own.
    pub fn import_form(frm: &[u8], frx: &dyn Fn(&str) -> Option<Vec<u8>>, name: &str, cp: u16) -> Result<VbaModule, Error> {
        let text = match std::str::from_utf8(frm) {
            Ok(t) => t.trim_start_matches('\u{feff}').to_string(),
            Err(_) => crate::ole::decode_cp(frm, cp),
        };
        let text = normalize(&text);
        let bad = || Error::new("not a UserForm (.frm) file");
        // VERSION 5.00, then Begin {class} Name … End: the form's header.
        let mut lines = text.split_inclusive('\n');
        let mut head = String::new();
        let mut blob = None;
        let mut used = 0;
        let first = lines.next().ok_or_else(bad)?;
        if !first.trim().starts_with("VERSION ") {
            return Err(bad());
        }
        head.push_str(first);
        used += first.len();
        let begin = lines.next().ok_or_else(bad)?;
        used += begin.len();
        let mut words = begin.split_whitespace();
        if words.next() != Some("Begin") || !words.next().is_some_and(|c| c.eq_ignore_ascii_case(FORM_CLSID)) {
            return Err(bad());
        }
        let own = words.next().map(str::to_string);
        head.push_str(begin);
        let mut ended = false;
        for line in lines.by_ref() {
            used += line.len();
            let t = line.trim();
            if t == "End" {
                head.push_str(line);
                ended = true;
                break;
            }
            if let Some(v) = t.strip_prefix("OleObjectBlob").map(|v| v.trim_start().trim_start_matches('=').trim()) {
                // "File.frx":0000 — the design starts at that offset.
                let (file, at) = v.rsplit_once(':').ok_or_else(bad)?;
                if u64::from_str_radix(at.trim(), 16).map_err(|_| bad())? != 0 {
                    return Err(Error::new("the .frx of this form isn't one this version reads"));
                }
                blob = Some(file.trim().trim_matches('"').to_string());
                continue;
            }
            // A form's own header has no controls in it.
            if t.starts_with("Begin ") {
                return Err(bad());
            }
            head.push_str(line);
        }
        if !ended {
            return Err(bad());
        }
        let blob = blob.ok_or_else(|| Error::new("the .frm names no .frx with its design"))?;
        let bytes = frx(&blob).ok_or_else(|| Error::new(format!("{blob} can't be read")))?;
        let mut streams = frx_streams(&bytes).ok_or_else(|| Error::new(format!("{blob} isn't a form's design")))?;
        streams.insert("\u{3}VBFrame".into(), crlf(&head).into_bytes());
        let (attributes, code) = split_header(&text[used..]);
        let name = attributes
            .lines()
            .find_map(|l| l.trim().strip_prefix("Attribute VB_Name = "))
            .map(|v| v.trim().trim_matches('"').to_string())
            .or(own)
            .filter(|n| is_identifier(n))
            .unwrap_or_else(|| name.to_string());
        let mut m = VbaModule::form(&name, streams);
        m.code = code;
        Ok(m)
    }
}

/// The streams of a `.frx`: a compound file with the form's storage
/// (none of its own `VBFrame`). None when it isn't one or is too big.
fn frx_streams(bytes: &[u8]) -> Option<FormStreams> {
    if bytes.len() > MAX_FORM_BYTES {
        return None;
    }
    let mut ole = cfb::CompoundFile::open(Cursor::new(bytes)).ok()?;
    let paths: Vec<String> = ole.walk().filter(|e| e.is_stream()).map(|e| e.path().to_string_lossy().replace('\\', "/")).take(4097).collect();
    if paths.len() > 4096 {
        return None;
    }
    let mut out = FormStreams::new();
    let mut left = MAX_FORM_BYTES;
    for p in paths {
        let rel = p.trim_start_matches('/').to_string();
        let mut v = Vec::new();
        (&mut ole.open_stream(&p).ok()?).take(left as u64 + 1).read_to_end(&mut v).ok()?;
        left = left.checked_sub(v.len())?;
        if rel != "\u{3}VBFrame" {
            out.insert(rel, v);
        }
    }
    out.contains_key("f").then_some(out)
}

// --- new projects ------------------------------------------------------------------

/// The hidden header of a workbook's or a sheet's module as Excel writes
/// it (`base`: the class it extends).
fn document_attributes(name: &str, base: &str) -> String {
    format!(
        "Attribute VB_Name = \"{name}\"\nAttribute VB_Base = \"{base}\"\nAttribute VB_GlobalNameSpace = False\nAttribute VB_Creatable = False\nAttribute VB_PredeclaredId = True\nAttribute VB_Exposed = True\nAttribute VB_TemplateDerived = False\nAttribute VB_Customizable = True\n"
    )
}

/// The hidden header of a class module as Office writes it; `own`:
/// `Attribute` lines that replace the defaults of the same name.
fn class_attributes(name: &str, own: &[&str]) -> String {
    let mut out = format!("Attribute VB_Name = \"{name}\"\n");
    for line in ["Attribute VB_Base = \"0{FCFB3D2A-A0FA-1068-A738-08002B3371B5}\"", "Attribute VB_GlobalNameSpace = False", "Attribute VB_Creatable = False", "Attribute VB_PredeclaredId = False", "Attribute VB_Exposed = False", "Attribute VB_TemplateDerived = False", "Attribute VB_Customizable = False"] {
        let key = line.split(" = ").next().unwrap_or(line);
        let pick = own.iter().find(|l| l.split(" = ").next() == Some(key)).copied().unwrap_or(line);
        out.push_str(pick);
        out.push('\n');
    }
    // Others of its own (a description...).
    let extra: Vec<&str> = own.iter().filter(|l| !out.contains(l.split(" = ").next().unwrap_or(l))).copied().collect();
    for l in extra {
        out.push_str(l);
        out.push('\n');
    }
    out
}

/// Excel's Workbook and Worksheet classes, which ThisWorkbook and the
/// sheets' modules extend.
const EXCEL_WORKBOOK: &str = "0{00020819-0000-0000-C000-000000000046}";
const EXCEL_SHEET: &str = "0{00020820-0000-0000-C000-000000000046}";
const WORD_DOCUMENT: &str = "0{00020906-0000-0000-C000-000000000046}";

/// The library every project refers to (OLE Automation, stdole).
const STDOLE: &str = "*\\G{00020430-0000-0000-C000-000000000046}#2.0#0#C:\\Windows\\System32\\stdole2.tlb#OLE Automation";

/// Random bytes, from the standard library's randomly keyed hasher.
fn random_bytes(n: usize) -> Vec<u8> {
    use std::hash::{BuildHasher, Hasher};
    let state = std::collections::hash_map::RandomState::new();
    let mut out = Vec::with_capacity(n + 8);
    let mut i = 0u64;
    while out.len() < n {
        let mut h = state.build_hasher();
        h.write_u64(i);
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
        out.extend(h.finish().to_le_bytes());
        i += 1;
    }
    out.truncate(n);
    out
}

/// A new GUID, `{XXXXXXXX-XXXX-4XXX-YXXX-XXXXXXXXXXXX}` (version 4).
fn new_guid() -> String {
    let mut b = random_bytes(16);
    b[6] = (b[6] & 0x0F) | 0x40;
    b[8] = (b[8] & 0x3F) | 0x80;
    let h: Vec<String> = b.iter().map(|x| format!("{x:02X}")).collect();
    format!("{{{}-{}-{}-{}-{}}}", h[0..4].concat(), h[4..6].concat(), h[6..8].concat(), h[8..10].concat(), h[10..16].concat())
}

/// Encrypts a value of the PROJECT stream (CMG, DPB, GC; 2.4.3.2): the
/// project's key is the sum of the bytes of its ID.
fn encrypt(data: &[u8], project_id: &str, seed: u8, filler: &[u8]) -> String {
    let key = project_id.bytes().fold(0u8, |a, b| a.wrapping_add(b));
    let (version_enc, key_enc) = (seed ^ 2, seed ^ key);
    let mut out = vec![seed, version_enc, key_enc];
    let (mut plain1, mut enc1, mut enc2) = (key, key_enc, version_enc);
    let mut push = |b: u8, out: &mut Vec<u8>| {
        let e = b ^ enc2.wrapping_add(plain1);
        out.push(e);
        enc2 = enc1;
        enc1 = e;
        plain1 = b;
    };
    for i in 0..(seed & 6) / 2 {
        push(filler.get(i as usize).copied().unwrap_or(0), &mut out);
    }
    for b in (data.len() as u32).to_le_bytes().into_iter().chain(data.iter().copied()) {
        push(b, &mut out);
    }
    out.iter().map(|b| format!("{b:02X}")).collect()
}

/// A new VBA project for a workbook: the workbook's module (`workbook`,
/// its code name, as ThisWorkbook) and one for each sheet (their code
/// names), with no code and not locked. Excel compiles it when it opens
/// the file; the workbook and sheets must carry the same code names.
/// Fails when a name isn't an identifier ([`is_identifier`]) or two are
/// the same.
pub fn new_workbook_project(workbook: &str, sheets: &[String]) -> Result<Vec<u8>, Error> {
    let mut docs = vec![(workbook.to_string(), EXCEL_WORKBOOK)];
    docs.extend(sheets.iter().map(|s| (s.clone(), EXCEL_SHEET)));
    let mut seen = std::collections::HashSet::new();
    for (name, _) in &docs {
        if !is_identifier(name) || name.eq_ignore_ascii_case("dir") || name.eq_ignore_ascii_case("VBAProject") || !seen.insert(name.to_ascii_lowercase()) {
            return Err(Error::new(format!("invalid code name: {name}")));
        }
    }
    Ok(new_project(&docs))
}

/// A new VBA project for a Word document: its ThisDocument module, empty.
pub fn new_document_project() -> Vec<u8> {
    new_project(&[("ThisDocument".to_string(), WORD_DOCUMENT)])
}

/// A new VBA project for a presentation: PowerPoint has no document
/// modules, so it starts with none.
pub fn new_presentation_project() -> Vec<u8> {
    new_project(&[])
}

fn new_project(docs: &[(String, &str)]) -> Vec<u8> {
    let cp = 1252u16;
    let name = "VBAProject";
    let enc = |s: &str| crate::ole::encode_cp(s, cp);
    let mut libid = (STDOLE.len() as u32).to_le_bytes().to_vec();
    libid.extend(STDOLE.as_bytes());
    libid.extend([0; 6]);
    let mut recs = vec![
        // Win32, US English, the code page, the name, no description, help
        // or flags; a version; no constants; the reference to stdole.
        Rec::new(0x0001, 1u32.to_le_bytes().to_vec()),
        Rec::new(0x0002, 0x409u32.to_le_bytes().to_vec()),
        Rec::new(0x0014, 0x409u32.to_le_bytes().to_vec()),
        Rec::new(PROJECTCODEPAGE, cp.to_le_bytes().to_vec()),
        Rec::new(PROJECTNAME, enc(name)),
        Rec::new(0x0005, Vec::new()),
        Rec::new(0x0040, Vec::new()),
        Rec::new(0x0006, Vec::new()),
        Rec::new(0x003D, Vec::new()),
        Rec::new(0x0007, vec![0; 4]),
        Rec::new(0x0008, vec![0; 4]),
        Rec { id: PROJECTVERSION, size: 4, data: vec![0x5B, 0x9B, 0x31, 0x1A, 0x06, 0x00] },
        Rec::new(PROJECTCONSTANTS, Vec::new()),
        Rec::new(0x003C, Vec::new()),
        Rec::new(0x0016, b"stdole".to_vec()),
        Rec::new(0x003E, to_utf16("stdole")),
        Rec::new(REFERENCEREGISTERED, libid),
        Rec::new(PROJECTMODULES, (docs.len() as u16).to_le_bytes().to_vec()),
        Rec::new(0x0013, vec![0xFF, 0xFF]),
    ];
    let mut streams = Vec::new();
    for (module, base) in docs {
        recs.extend([
            Rec::new(MODULENAME, enc(module)),
            Rec::new(MODULENAMEUNICODE, to_utf16(module)),
            Rec::new(MODULESTREAMNAME, enc(module)),
            Rec::new(MODULESTREAMNAME_UNICODE, to_utf16(module)),
            Rec::new(0x001C, Vec::new()),
            Rec::new(0x0048, Vec::new()),
            Rec::new(MODULEOFFSET, vec![0; 4]),
            Rec::new(0x001E, vec![0; 4]),
            Rec::new(0x002C, vec![0xFF, 0xFF]),
            // A document module is not procedural.
            Rec::new(0x0022, Vec::new()),
            Rec::new(MODULE_TERMINATOR, Vec::new()),
        ]);
        streams.push((module.clone(), compress(&enc(&crlf(&document_attributes(module, base))))));
    }
    recs.push(Rec::new(0x0010, Vec::new()));

    let id = new_guid();
    let r = random_bytes(6);
    // Not locked: protection state 0, no password (one null byte), visible.
    let (cmg, dpb, gc) = (encrypt(&[0; 4], &id, r[0], &r[3..]), encrypt(&[0], &id, r[1], &r[3..]), encrypt(&[0xFF], &id, r[2], &r[3..]));
    let mut project = format!("ID=\"{id}\"\r\n");
    for (module, _) in docs {
        project.push_str(&format!("Document={module}/&H00000000\r\n"));
    }
    project.push_str(&format!("Name=\"{name}\"\r\nHelpContextID=\"0\"\r\nVersionCompatible32=\"393222000\"\r\nCMG=\"{cmg}\"\r\nDPB=\"{dpb}\"\r\nGC=\"{gc}\"\r\n\r\n[Host Extender Info]\r\n&H00000001={{3832D640-CF90-11CF-8E43-00A0C911005A}};VBE;&H00000000\r\n"));
    let mut wm = Vec::new();
    for (module, _) in docs {
        wm.extend(enc(module));
        wm.push(0);
        wm.extend(to_utf16(module));
        wm.extend([0, 0]);
    }
    wm.extend([0, 0]);

    // Writing to memory doesn't fail.
    let mut c = cfb::CompoundFile::create_with_version(cfb::Version::V3, Cursor::new(Vec::new())).expect("compound file in memory");
    c.create_storage("/VBA").expect("VBA storage");
    let mut put = |path: &str, data: &[u8]| c.create_stream(path).and_then(|mut s| s.write_all(data)).expect("stream in memory");
    put("/VBA/_VBA_PROJECT", &EMPTY_VBA_PROJECT);
    put("/VBA/dir", &compress(&write_records(&recs)));
    for (module, data) in &streams {
        put(&format!("/VBA/{module}"), data);
    }
    put("/PROJECT", &enc(&project));
    put("/PROJECTwm", &wm);
    c.flush().expect("compound file in memory");
    c.into_inner().into_inner()
}

/// Whether `name` can be a new code name: an ASCII letter, then letters,
/// digits or `_`, up to 31 (any code page writes it as it is).
pub fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic()) && name.len() <= 31 && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Why a module name can't be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameError {
    Empty,
    /// Not a VBA identifier (a letter, then letters, digits or `_`; up to 31).
    Invalid,
    Taken,
}

impl VbaProject {
    /// Whether the modules, the name, the description or the lock differ
    /// from the file's.
    pub fn changed(&self) -> bool {
        self.modules.len() != self.read.len() || self.modules.iter().zip(&self.read).any(|(m, r)| *m != r.module) || self.name != self.read_name || self.description != self.read_description || self.lock.is_some()
    }

    /// The PROJECT stream as read.
    fn project_stream(&self) -> Option<String> {
        let mut ole = cfb::CompoundFile::open(Cursor::new(&self.bin)).ok()?;
        stream(&mut ole, "/PROJECT").map(|d| crate::ole::decode_cp(&d, self.code_page))
    }

    /// The project's references, in order.
    pub fn references(&self) -> Vec<Reference> {
        let cp = self.code_page;
        let mut out = Vec::new();
        let mut name = String::new();
        // A libid after a 4-byte size.
        let libid = |d: &[u8]| -> String {
            let n = u32_at(d, 0).unwrap_or(0) as usize;
            d.get(4..4 + n).map(|b| crate::ole::decode_cp(b, cp)).unwrap_or_default()
        };
        for r in &self.records[..self.head] {
            match r.id {
                0x0016 => name = crate::ole::decode_cp(&r.data, cp),
                0x003E => {
                    let u = utf16(&r.data);
                    if !u.is_empty() {
                        name = u;
                    }
                }
                REFERENCEREGISTERED | 0x0033 => out.push(Reference::from_libid(std::mem::take(&mut name), &libid(&r.data), false)),
                REFERENCEPROJECT => out.push(Reference::from_libid(std::mem::take(&mut name), &libid(&r.data), true)),
                // A control library: the original reference (0x0033) came
                // first and stands for it.
                _ => {}
            }
        }
        out
    }

    /// Whether the file's project has a password (Tools > Project
    /// Properties > Protection).
    pub fn has_password(&self) -> bool {
        self.project_stream().is_some_and(|p| !matches!(stored_password(&p), Stored::None))
    }

    /// Whether `password` is the one the project is locked with; a
    /// project with none takes none.
    pub fn password_matches(&self, password: &str) -> bool {
        let pw = crate::ole::encode_cp(password, self.code_page);
        match self.project_stream().map(|p| stored_password(&p)) {
            Some(Stored::Hash(key, hash)) => {
                use sha1::Digest;
                let mut h = sha1::Sha1::new();
                h.update(&pw);
                h.update(key);
                // In constant time: the hash is no secret, but no need to
                // tell how much of a guess was right.
                h.finalize().iter().zip(hash).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0
            }
            Some(Stored::Plain(p)) => p == pw,
            Some(Stored::None) | None => password.is_empty(),
        }
    }

    /// Locks the project for viewing with `password` (as Excel's "Lock
    /// project for viewing"), or takes the lock away (`None`). Refused
    /// while it is locked ([`VbaProject::protected`]).
    pub fn set_password(&mut self, password: Option<&str>) -> Result<(), Error> {
        if self.protected {
            return Err(Error::new("the project is locked"));
        }
        if password.is_some_and(str::is_empty) {
            return Err(Error::new("empty password"));
        }
        self.lock = Some(password.map(str::to_string));
        Ok(())
    }

    /// Whether `name` fits a new or renamed module (`except`: its index).
    pub fn check_name(&self, name: &str, except: Option<usize>) -> Result<(), NameError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(NameError::Empty);
        }
        let mut chars = name.chars();
        let first_ok = chars.next().is_some_and(|c| c.is_alphabetic());
        // Written in the project's code page: a name it can't hold would
        // come back changed.
        let fits = crate::ole::decode_cp(&crate::ole::encode_cp(name, self.code_page), self.code_page) == name;
        if !first_ok || name.chars().count() > 31 || !chars.all(|c| c.is_alphanumeric() || c == '_') || name.eq_ignore_ascii_case("dir") || !fits {
            return Err(NameError::Invalid);
        }
        let lower = name.to_lowercase();
        // Nor the stream of another module (it may be named otherwise).
        let stream_of = |m: &VbaModule| m.origin.and_then(|o| self.read.get(o)).map(|r| r.stream.to_lowercase());
        let taken = self.modules.iter().enumerate().any(|(i, m)| Some(i) != except && (m.name.to_lowercase() == lower || stream_of(m).as_deref() == Some(lower.as_str()))) || self.name.to_lowercase() == lower;
        if taken { Err(NameError::Taken) } else { Ok(()) }
    }

    /// Matches the document modules with the workbook's code names
    /// (`workbook`, and one per sheet, None when it has none): a sheet
    /// without one gets a free name (`SheetN`) and a module, as do code
    /// names without a module; modules added since the project was read
    /// whose sheet is gone go if they have no code. When the file doesn't
    /// say which sheet an old module is (sheets without code names, old
    /// modules matching none) the sheets are left alone. Whether anything
    /// changed. A locked project is not touched.
    pub fn match_book(&mut self, workbook: &mut Option<String>, sheets: &mut [Option<String>]) -> bool {
        if self.protected {
            return false;
        }
        let mut changed = false;
        let has = |p: &VbaProject, n: &str| p.modules.iter().any(|m| m.kind == ModuleKind::Document && m.name.eq_ignore_ascii_case(n));
        if let Some(book) = self.modules.iter().find(|m| m.extends(EXCEL_WORKBOOK)).map(|m| m.name.clone())
            && !workbook.as_deref().is_some_and(|w| has(self, w))
        {
            *workbook = Some(book);
            changed = true;
        }
        // A code name given twice: the later sheets lose it.
        let mut seen = std::collections::HashSet::new();
        for s in sheets.iter_mut() {
            if s.as_ref().is_some_and(|n| !seen.insert(n.to_lowercase())) {
                *s = None;
                changed = true;
            }
        }
        let named = |sheets: &[Option<String>], n: &str| sheets.iter().flatten().any(|s| s.eq_ignore_ascii_case(n));
        // Any old document module but the workbook's may be a sheet's.
        let unmatched_old = self.modules.iter().any(|m| m.origin.is_some() && m.kind == ModuleKind::Document && !m.extends(EXCEL_WORKBOOK) && !named(sheets, &m.name));
        let has_sheet = |p: &VbaProject, n: &str| p.modules.iter().any(|m| m.kind == ModuleKind::Document && !m.extends(EXCEL_WORKBOOK) && m.name.eq_ignore_ascii_case(n));
        if unmatched_old && sheets.iter().any(Option::is_none) {
            return changed;
        }
        // Code names without a module: one is added (a name the project
        // can't take is given up).
        for s in sheets.iter_mut() {
            let Some(n) = s.clone() else { continue };
            if has_sheet(self, &n) {
                continue;
            }
            if is_identifier(&n) && self.check_name(&n, None).is_ok() {
                self.modules.push(VbaModule::sheet(&n));
            } else {
                *s = None;
            }
            changed = true;
        }
        // Sheets without one: modules added for sheets now gone, then new.
        let mut spare: Vec<String> = self.modules.iter().filter(|m| m.origin.is_none() && m.extends(EXCEL_SHEET) && !named(sheets, &m.name)).map(|m| m.name.clone()).collect();
        spare.reverse();
        for i in 0..sheets.len() {
            if sheets[i].is_some() {
                continue;
            }
            let name = match spare.pop() {
                Some(n) => n,
                None => {
                    let n = (1..).map(|k| format!("Sheet{k}")).find(|n| self.check_name(n, None).is_ok() && !named(sheets, n)).unwrap_or_default();
                    self.modules.push(VbaModule::sheet(&n));
                    n
                }
            };
            sheets[i] = Some(name);
            changed = true;
        }
        let before = self.modules.len();
        self.modules.retain(|m| !(m.origin.is_none() && m.extends(EXCEL_SHEET) && !named(sheets, &m.name) && m.code.trim().is_empty()));
        changed | (self.modules.len() != before)
    }

    /// A free name like `Module3`.
    pub fn next_name(&self, base: &str) -> String {
        (1..).map(|i| format!("{base}{i}")).find(|n| self.check_name(n, None).is_ok()).unwrap_or_default()
    }

    /// The vbaProject.bin with the modules as they are now: the file's
    /// bytes when nothing changed.
    pub fn write(&self) -> Result<Vec<u8>, Error> {
        if !self.changed() {
            return Ok(self.bin.clone());
        }
        self.write_as(false)
    }

    /// The vbaProject.bin written anew to be signed: no module ends in a
    /// line break, so the readings of the V3 hash agree on it
    /// ([`v3_source_hash`]).
    pub fn write_for_signing(&self) -> Result<Vec<u8>, Error> {
        self.write_as(true)
    }

    fn write_as(&self, bare_ends: bool) -> Result<Vec<u8>, Error> {
        let cp = self.code_page;
        let mut recs: Vec<Rec> = self.records[..self.head].to_vec();
        let description = self.description_line();
        for r in &mut recs {
            match r.id {
                PROJECTMODULES => *r = Rec::new(PROJECTMODULES, (self.modules.len() as u16).to_le_bytes().to_vec()),
                PROJECTNAME if self.name != self.read_name => *r = Rec::new(PROJECTNAME, crate::ole::encode_cp(&self.name, cp)),
                PROJECTDOCSTRING if self.description != self.read_description => *r = Rec::new(PROJECTDOCSTRING, crate::ole::encode_cp(&description, cp)),
                PROJECTDOCSTRING_UNICODE if self.description != self.read_description => *r = Rec::new(PROJECTDOCSTRING_UNICODE, to_utf16(&description)),
                _ => {}
            }
        }
        // Stream name and source of each module.
        let mut streams: Vec<(String, Vec<u8>)> = Vec::new();
        for m in &self.modules {
            let orig = m.origin.and_then(|i| self.read.get(i));
            let renamed = orig.is_none_or(|o| o.module.name != m.name);
            let stream_name = match orig {
                Some(o) if !renamed => o.stream.clone(),
                _ => m.name.clone(),
            };
            let mut group: Vec<Rec> = match orig {
                Some(o) => self.records[o.recs.clone()].to_vec(),
                None => vec![
                    Rec::new(MODULENAME, Vec::new()),
                    Rec::new(MODULENAMEUNICODE, Vec::new()),
                    Rec::new(MODULESTREAMNAME, Vec::new()),
                    Rec::new(MODULESTREAMNAME_UNICODE, Vec::new()),
                    Rec::new(0x001C, Vec::new()),
                    Rec::new(0x0048, Vec::new()),
                    Rec::new(MODULEOFFSET, vec![0; 4]),
                    Rec::new(0x001E, vec![0; 4]),
                    Rec::new(0x002C, vec![0xFF, 0xFF]),
                    // Only standard modules are procedural.
                    Rec::new(if m.kind == ModuleKind::Standard { MODULETYPE_PROCEDURAL } else { 0x0022 }, Vec::new()),
                    Rec::new(MODULE_TERMINATOR, Vec::new()),
                ],
            };
            for r in &mut group {
                match r.id {
                    MODULENAME if renamed => *r = Rec::new(r.id, crate::ole::encode_cp(&m.name, cp)),
                    MODULENAMEUNICODE if renamed => *r = Rec::new(r.id, to_utf16(&m.name)),
                    MODULESTREAMNAME => *r = Rec::new(r.id, crate::ole::encode_cp(&stream_name, cp)),
                    MODULESTREAMNAME_UNICODE => *r = Rec::new(r.id, to_utf16(&stream_name)),
                    MODULEOFFSET => *r = Rec::new(r.id, vec![0; 4]),
                    _ => {}
                }
            }
            recs.extend(group);
            let attributes = set_vb_name(&m.attributes, &m.name);
            let mut source = crlf(&format!("{}{}", normalize(&attributes), m.code));
            if bare_ends {
                source.truncate(source.trim_end_matches(['\r', '\n']).len());
            }
            streams.push((stream_name, compress(&crate::ole::encode_cp(&source, cp))));
        }
        recs.extend_from_slice(&self.records[self.tail..]);

        // A damaged file whose other streams can't all be read: they are
        // copied as far as they can be.
        let repaired;
        let bin: &[u8] = if streams_readable(&self.bin) {
            &self.bin
        } else {
            repaired = crate::ole_repair::rebuild(&self.bin).ok_or_else(|| Error::new("invalid VBA project"))?;
            &repaired
        };
        let mut src = cfb::CompoundFile::open(Cursor::new(bin)).map_err(|e| Error::new(e.to_string()))?;
        let project = stream(&mut src, "/PROJECT").map(|d| crate::ole::decode_cp(&d, cp));
        let has_wm = src.is_stream("/PROJECTwm");
        let old_streams: Vec<String> = self.read.iter().map(|r| r.stream.to_lowercase()).collect();
        // The storages of forms are written below, as the forms are now.
        let old_forms: Vec<String> = self.read.iter().filter(|r| r.module.kind == ModuleKind::Form).map(|r| r.stream.to_lowercase()).collect();
        let io = |e: std::io::Error| Error::new(e.to_string());
        let mut dst = cfb::CompoundFile::create_with_version(cfb::Version::V3, Cursor::new(Vec::new())).map_err(io)?;
        copy_tree(&mut src, Path::new("/"), &mut dst, Path::new("/"), &|p: &Path| {
            let s = p.to_string_lossy().replace('\\', "/");
            let lower = s.to_lowercase();
            let top = lower.trim_start_matches('/').split('/').next().unwrap_or("");
            if old_forms.iter().any(|f| f == top) {
                return true;
            }
            let Some(name) = lower.strip_prefix("/vba/") else { return matches!(lower.as_str(), "/project" | "/projectwm") };
            name == "dir" || name == "_vba_project" || name.starts_with("__srp_") || old_streams.iter().any(|o| o == name)
        })
        .map_err(io)?;
        let mut put = |path: &str, data: &[u8]| -> Result<(), Error> { dst.create_stream(path).and_then(|mut s| s.write_all(data)).map_err(io) };
        put("/VBA/_VBA_PROJECT", &EMPTY_VBA_PROJECT)?;
        put("/VBA/dir", &compress(&write_records(&recs)))?;
        for (name, data) in &streams {
            put(&format!("/VBA/{name}"), data)?;
        }
        if let Some(p) = project {
            put("/PROJECT", &crate::ole::encode_cp(&self.project_text(&p), cp))?;
        }
        if has_wm {
            let mut wm = Vec::new();
            for m in &self.modules {
                wm.extend(crate::ole::encode_cp(&m.name, cp));
                wm.push(0);
                wm.extend(to_utf16(&m.name));
                wm.extend([0, 0]);
            }
            wm.extend([0, 0]);
            put("/PROJECTwm", &wm)?;
        }
        for (m, (stream_name, _)) in self.modules.iter().zip(&streams) {
            if m.kind != ModuleKind::Form {
                continue;
            }
            let orig = m.origin.and_then(|i| self.read.get(i));
            let design = match (&m.form, orig) {
                (Some(f), _) => f.clone(),
                (None, Some(o)) => {
                    let from = Path::new("/").join(&o.stream);
                    if o.stream != *stream_name || !src.is_storage(&from) {
                        // Renamed: its design under the new name.
                        match self.stored_form(&o.stream) {
                            Some(f) => f,
                            None => continue,
                        }
                    } else {
                        dst.create_storage(&from).map_err(io)?;
                        copy_tree(&mut src, &from, &mut dst, &from, &|_| false).map_err(io)?;
                        continue;
                    }
                }
                (None, None) => return Err(Error::new(format!("the form {} has no design", m.name))),
            };
            put_form(&mut dst, stream_name, &m.name, &design, cp)?;
        }
        dst.flush().map_err(io)?;
        Ok(dst.into_inner().into_inner())
    }

    /// The PROJECT stream with the modules as they are now: lines of
    /// removed modules go, renamed ones change, new ones are added.
    fn project_text(&self, text: &str) -> String {
        let key = |k: ModuleKind| match k {
            ModuleKind::Standard => "Module",
            ModuleKind::Class => "Class",
            ModuleKind::Form => "BaseClass",
            ModuleKind::Document => "Document",
        };
        let now = |old: &str| -> Option<&VbaModule> {
            let i = self.read.iter().position(|r| r.module.name.eq_ignore_ascii_case(old))?;
            self.modules.iter().find(|m| m.origin == Some(i))
        };
        let mut out = Vec::new();
        let mut section = false;
        let mut last_module = None;
        for line in text.split("\r\n").flat_map(|l| l.split('\n')) {
            if line.starts_with('[') {
                section = true;
            }
            let (k, v) = line.split_once('=').unwrap_or((line, ""));
            let module_line = !section && matches!(k, "Module" | "Class" | "BaseClass" | "Document");
            if module_line {
                let name = v.split('/').next().unwrap_or(v);
                if self.read.iter().any(|r| r.module.name.eq_ignore_ascii_case(name)) {
                    match now(name) {
                        Some(m) => out.push(format!("{k}={}{}", m.name, &v[name.len()..])),
                        None => continue,
                    }
                } else {
                    out.push(line.to_string());
                }
                last_module = Some(out.len());
                continue;
            }
            if section && !line.starts_with('[') && !v.is_empty() && self.read.iter().any(|r| r.module.name.eq_ignore_ascii_case(k)) {
                // [Workspace] window positions.
                match now(k) {
                    Some(m) => out.push(format!("{}={v}", m.name)),
                    None => continue,
                }
                continue;
            }
            if !section && last_module.is_none() && k == "ID" {
                last_module = Some(out.len() + 1);
            }
            out.push(line.to_string());
        }
        let at = last_module.unwrap_or(0).min(out.len());
        let added: Vec<String> = self
            .modules
            .iter()
            .filter(|m| m.origin.is_none())
            .map(|m| match m.kind {
                ModuleKind::Document => format!("Document={}/&H00000000", m.name),
                k => format!("{}={}", key(k), m.name),
            })
            .collect();
        out.splice(at..at, added);
        self.project_properties(&mut out);
        let mut s = out.join("\r\n");
        if !s.ends_with("\r\n") {
            s.push_str("\r\n");
        }
        s
    }
}

impl VbaProject {
    /// The description as written: one line, up to 2000 characters, no
    /// double quotes (the PROJECT stream quotes it).
    fn description_line(&self) -> String {
        self.description.chars().map(|c| if c.is_control() { ' ' } else if c == '"' { '\'' } else { c }).take(2000).collect::<String>().trim().to_string()
    }

    /// The PROJECT stream's Name, Description, CMG, DPB and GC lines as
    /// they are now.
    fn project_properties(&self, out: &mut Vec<String>) {
        let end = out.iter().position(|l| l.starts_with('[')).unwrap_or(out.len());
        if self.name != self.read_name
            && let Some(l) = out[..end].iter_mut().find(|l| l.starts_with("Name="))
        {
            *l = format!("Name=\"{}\"", self.name);
        }
        if self.description != self.read_description {
            out.retain(|l| !l.starts_with("Description="));
            let d = self.description_line();
            if !d.is_empty() {
                let at = out.iter().position(|l| l.starts_with("Name=")).map_or(0, |i| i + 1);
                out.insert(at, format!("Description=\"{d}\""));
            }
        }
        let Some(lock) = &self.lock else { return };
        let id = out.iter().find_map(|l| l.strip_prefix("ID=")).unwrap_or("").trim_matches('"').to_string();
        let r = random_bytes(10);
        let (state, password, visible) = match lock {
            Some(pw) => {
                use sha1::Digest;
                let key: [u8; 4] = [r[4], r[5], r[6], r[7]];
                let mut h = sha1::Sha1::new();
                h.update(crate::ole::encode_cp(pw, self.code_page));
                h.update(key);
                (1u8, password_hash_data(key, h.finalize().into()), 0u8)
            }
            None => (0, vec![0], 0xFF),
        };
        let values = [("CMG", encrypt(&[state, 0, 0, 0], &id, r[0], &r[3..])), ("DPB", encrypt(&password, &id, r[1], &r[3..])), ("GC", encrypt(&[visible], &id, r[2], &r[3..]))];
        for (key, v) in values {
            let line = format!("{key}=\"{v}\"");
            let end = out.iter().position(|l| l.starts_with('[')).unwrap_or(out.len());
            match out[..end].iter().position(|l| l.starts_with(&format!("{key}="))) {
                Some(i) => out[i] = line,
                None => {
                    let at = out[..end].iter().rposition(|l| !l.is_empty()).map_or(0, |i| i + 1);
                    out.insert(at, line);
                }
            }
        }
    }
}

/// The password the PROJECT stream's DPB keeps.
enum Stored {
    None,
    /// A Password Hash Data Structure (2.4.4.1): key and SHA-1 hash.
    Hash([u8; 4], [u8; 20]),
    /// The password itself, as older files keep it.
    Plain(Vec<u8>),
}

fn stored_password(project: &str) -> Stored {
    let Some(d) = project.lines().find_map(|l| l.strip_prefix("DPB=")).and_then(decrypt) else { return Stored::None };
    if d.len() == 29 && d[0] == 0xFF && d[28] == 0 {
        // A clear bit says the byte was a null, written as 1 (2.4.4.2).
        let set = |i: usize| d[1 + i / 8] & (0x80 >> (i % 8)) != 0;
        let mut key = [0u8; 4];
        let mut hash = [0u8; 20];
        for (i, b) in key.iter_mut().chain(hash.iter_mut()).enumerate() {
            *b = if set(i) { d[4 + i] } else { 0 };
        }
        return Stored::Hash(key, hash);
    }
    let plain: Vec<u8> = d.iter().copied().take_while(|&b| b != 0).collect();
    if plain.is_empty() { Stored::None } else { Stored::Plain(plain) }
}

/// The Password Hash Data Structure of `key` and `hash` (2.4.4.1): nulls
/// written as 1 and marked by a clear bit, most significant first.
fn password_hash_data(key: [u8; 4], hash: [u8; 20]) -> Vec<u8> {
    let mut out = vec![0xFF, 0, 0, 0];
    for (i, b) in key.iter().chain(hash.iter()).enumerate() {
        if *b != 0 {
            out[1 + i / 8] |= 0x80 >> (i % 8);
        }
    }
    out.extend(key.iter().chain(hash.iter()).map(|&b| if b == 0 { 1 } else { b }));
    out.push(0);
    out
}

/// Header lines with `Attribute VB_Name` set to `name`.
fn set_vb_name(attributes: &str, name: &str) -> String {
    let line = format!("Attribute VB_Name = \"{name}\"");
    let mut found = false;
    let mut out: Vec<String> = normalize(attributes)
        .lines()
        .map(|l| {
            if l.trim_start().starts_with("Attribute VB_Name") {
                found = true;
                line.clone()
            } else {
                l.to_string()
            }
        })
        .collect();
    if !found {
        // After the VERSION…END block of a class, if any.
        let at = out.iter().position(|l| l.trim() == "END").map_or(0, |i| i + 1);
        out.insert(at, line);
    }
    out.iter().map(|l| format!("{l}\n")).collect()
}

/// Copies the storages and streams under `from` into `to` (skipping the
/// paths `skip` says), with their class ids.
/// Whether every stream of a compound file reads to its end.
fn streams_readable(data: &[u8]) -> bool {
    let Ok(mut c) = cfb::CompoundFile::open(Cursor::new(data)) else { return false };
    let mut paths: Vec<PathBuf> = Vec::new();
    crate::crypto::walk_within(&c, &mut |e| {
        if e.is_stream() {
            paths.push(e.path().to_path_buf());
        }
        true
    });
    paths.iter().all(|p| c.open_stream(p).is_ok_and(|mut s| std::io::copy(&mut s, &mut std::io::sink()).is_ok()))
}

/// Writes a form's design as the storage `storage` (or the root, ""): its streams, the
/// storages they are in (each with the class its `CompObj` names), the
/// form named `name` in its `VBFrame`.
fn put_form<W: Read + Write + std::io::Seek>(dst: &mut Ole<W>, storage: &str, name: &str, streams: &FormStreams, cp: u16) -> Result<(), Error> {
    let io = |e: std::io::Error| Error::new(e.to_string());
    let bad = || Error::new(format!("the design of the form {name} can't be written"));
    if streams.len() > 4096 || streams.values().map(Vec::len).sum::<usize>() > MAX_FORM_BYTES {
        return Err(bad());
    }
    let root = Path::new("/").join(storage);
    // (None: the file's root, a .frx.)
    if !storage.is_empty() {
        dst.create_storage(&root).map_err(io)?;
    }
    let mut keys: Vec<&String> = streams.keys().collect();
    keys.sort();
    for key in keys {
        let parts: Vec<&str> = key.split('/').collect();
        if parts.iter().any(|p| matches!(*p, "" | "." | "..") || p.encode_utf16().count() > 31 || p.contains(['\\', ':', '!'])) {
            return Err(bad());
        }
        let mut dir = root.clone();
        for p in &parts[..parts.len() - 1] {
            dir.push(p);
            if !dst.is_storage(&dir) {
                dst.create_storage(&dir).map_err(io)?;
            }
        }
        let data = &streams[key];
        let data = if key == "\u{3}VBFrame" { crate::ole::encode_cp(&frame_named(&String::from_utf8_lossy(data), name), cp) } else { data.clone() };
        dst.create_stream(root.join(key)).and_then(|mut s| s.write_all(&data)).map_err(io)?;
        if parts.last() == Some(&"\u{1}CompObj") {
            if let Some(id) = data.get(12..28).filter(|id| id.iter().any(|&b| b != 0)) {
                let mut b = [0u8; 16];
                b.copy_from_slice(id);
                dst.set_storage_clsid(&dir, uuid::Uuid::from_bytes_le(b)).map_err(io)?;
            }
        }
    }
    Ok(())
}

/// A `VBFrame` text naming the form `name` (`Begin {…} Name`).
fn frame_named(text: &str, name: &str) -> String {
    let mut out = String::new();
    let mut done = false;
    for line in text.split_inclusive('\n') {
        let t = line.trim_start();
        match t.strip_prefix("Begin ").and_then(|r| Some(&r[..=r.find('}')?])) {
            Some(class) if !done => {
                done = true;
                let end = if line.ends_with("\r\n") { "\r\n" } else if line.ends_with('\n') { "\n" } else { "" };
                out.push_str(&format!("Begin {class} {name} {end}"));
            }
            _ => out.push_str(line),
        }
    }
    out
}

fn copy_tree<R: Read + std::io::Seek, W: Read + Write + std::io::Seek>(src: &mut Ole<R>, from: &Path, dst: &mut Ole<W>, to: &Path, skip: &dyn Fn(&Path) -> bool) -> std::io::Result<()> {
    let entries: Vec<(PathBuf, bool, _)> = src.walk_storage(from)?.map(|e| (e.path().to_path_buf(), e.is_stream(), *e.clsid())).collect();
    for (path, is_stream, clsid) in entries {
        let Ok(rel) = path.strip_prefix(from) else { continue };
        let target = to.join(rel);
        if rel.as_os_str().is_empty() {
            dst.set_storage_clsid(to, clsid)?;
            continue;
        }
        if skip(&path) {
            continue;
        }
        if is_stream {
            let mut data = Vec::new();
            src.open_stream(&path)?.read_to_end(&mut data)?;
            dst.create_stream(&target)?.write_all(&data)?;
        } else {
            dst.create_storage(&target)?;
            dst.set_storage_clsid(&target, clsid)?;
        }
    }
    Ok(())
}

/// The VBA project of a legacy compound file (.xls: `_VBA_PROJECT_CUR`,
/// .doc: `Macros`) as a vbaProject.bin: the same storage, on its own.
/// (A signature of the file doesn't carry over: it signs the old format.)
pub(crate) fn from_legacy(data: &[u8], storage: &str) -> Option<Vec<u8>> {
    let mut src = cfb::CompoundFile::open(Cursor::new(data)).ok()?;
    let from = Path::new("/").join(storage);
    if !src.is_storage(&from) || !src.is_stream(from.join("VBA").join("dir")) {
        return None;
    }
    let mut dst = cfb::CompoundFile::create_with_version(cfb::Version::V3, Cursor::new(Vec::new())).ok()?;
    copy_tree(&mut src, &from, &mut dst, Path::new("/"), &|_| false).ok()?;
    dst.flush().ok()?;
    Some(dst.into_inner().into_inner())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    /// The examples of MS-OVBA 3.2.
    const VECTORS: [(&str, &str); 3] = [
        ("#aaabcdefaaaaghijaaaaaklaaamnopqaaaaaaaaaaaarstuvwxyzaaa", "01 2F B0 00 23 61 61 61 62 63 64 65 82 66 00 70 61 67 68 69 6A 01 38 08 61 6B 6C 00 30 6D 6E 6F 70 06 71 02 70 04 10 72 73 74 75 76 10 77 78 79 7A 00 3C"),
        ("abcdefghijklmnopqrstuv.", "01 19 B0 00 61 62 63 64 65 66 67 68 00 69 6A 6B 6C 6D 6E 6F 70 00 71 72 73 74 75 76 2E"),
        ("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "01 03 B0 02 61 45 00"),
    ];

    #[test]
    fn specification_vectors() {
        for (plain, packed) in VECTORS {
            assert_eq!(decompress(&hex(packed)).unwrap(), plain.as_bytes());
            let ours = compress(plain.as_bytes());
            assert_eq!(decompress(&ours).unwrap(), plain.as_bytes());
            // Office picks other matches of the same length in the first.
            assert_eq!(ours.len(), hex(packed).len());
            if !plain.starts_with('#') {
                assert_eq!(ours, hex(packed), "{plain}");
            }
        }
    }

    #[test]
    fn long_and_raw_chunks() {
        // Text over several chunks, and noise that doesn't compress.
        let text: Vec<u8> = (0..20000).map(|i| b"Sub Macro()\r\n    Range(\"A1\").Value = 1\r\nEnd Sub\r\n"[i % 47]).collect();
        let packed = compress(&text);
        assert!(packed.len() < text.len() / 4);
        assert_eq!(decompress(&packed).unwrap(), text);
        let mut x = 12345u32;
        let noise: Vec<u8> = (0..9000)
            .map(|_| {
                x = x.wrapping_mul(1103515245).wrapping_add(12345);
                (x >> 16) as u8
            })
            .collect();
        assert_eq!(decompress(&compress(&noise)).unwrap(), noise);
        assert_eq!(decompress(&compress(b"")).unwrap(), b"");
    }

    /// Encrypts like the PROJECT stream does (2.4.3.2), with a fixed seed.
    fn encrypt(data: &[u8], seed: u8) -> String {
        let key = 0xABu8;
        let (version_enc, key_enc) = (seed ^ 2, seed ^ key);
        let mut out = vec![seed, version_enc, key_enc];
        let (mut plain1, mut enc1, mut enc2) = (key, key_enc, version_enc);
        let mut push = |b: u8, out: &mut Vec<u8>| {
            let e = b ^ enc2.wrapping_add(plain1);
            out.push(e);
            enc2 = enc1;
            enc1 = e;
            plain1 = b;
        };
        for _ in 0..(seed & 6) / 2 {
            push(0x11, &mut out);
        }
        for b in (data.len() as u32).to_le_bytes().into_iter().chain(data.iter().copied()) {
            push(b, &mut out);
        }
        out.iter().map(|b| format!("{b:02X}")).collect()
    }

    /// A project as Office writes one: two document modules, a standard
    /// module and a class; p-code streams included.
    pub(crate) fn sample(locked: bool) -> Vec<u8> {
        let cp = 1252u16;
        let mut recs = vec![
            Rec::new(0x0001, 1u32.to_le_bytes().to_vec()),
            Rec::new(0x0002, 0x409u32.to_le_bytes().to_vec()),
            Rec::new(0x0014, 0x409u32.to_le_bytes().to_vec()),
            Rec::new(PROJECTCODEPAGE, cp.to_le_bytes().to_vec()),
            Rec::new(PROJECTNAME, b"VBAProject".to_vec()),
            Rec::new(0x0005, Vec::new()),
            Rec::new(0x0040, Vec::new()),
            Rec::new(0x0006, Vec::new()),
            Rec::new(0x003D, Vec::new()),
            Rec::new(0x0007, vec![0; 4]),
            Rec::new(0x0008, vec![0; 4]),
            Rec { id: PROJECTVERSION, size: 4, data: vec![0x5B, 0x9B, 0x31, 0x1A, 0x06, 0x00] },
            Rec::new(0x000C, Vec::new()),
            Rec::new(0x003C, Vec::new()),
            Rec::new(0x0016, b"stdole".to_vec()),
            Rec::new(0x003E, to_utf16("stdole")),
            Rec::new(0x000D, b"\x5E\x00\x00\x00*\\G{00020430-0000-0000-C000-000000000046}#2.0#0#C:\\Windows\\System32\\stdole2.tlb#OLE Automation\x00\x00\x00\x00\x00\x00".to_vec()),
        ];
        let modules = [("ThisWorkbook", false, "Attribute VB_Name = \"ThisWorkbook\"\r\nAttribute VB_Base = \"0{00020819-0000-0000-C000-000000000046}\"\r\nAttribute VB_GlobalNameSpace = False\r\nAttribute VB_Creatable = False\r\nAttribute VB_PredeclaredId = True\r\nAttribute VB_Exposed = True\r\nAttribute VB_TemplateDerived = False\r\nAttribute VB_Customizable = True\r\nPrivate Sub Workbook_Open()\r\nEnd Sub\r\n"),
            ("Sheet1", false, "Attribute VB_Name = \"Sheet1\"\r\nAttribute VB_Base = \"0{00020820-0000-0000-C000-000000000046}\"\r\n"),
            ("Module1", true, "Attribute VB_Name = \"Module1\"\r\nSub Hello()\r\n    MsgBox \"Hola, año\"\r\nEnd Sub\r\n"),
            ("Class1", false, "VERSION 1.0 CLASS\r\nBEGIN\r\n  MultiUse = -1  'True\r\nEND\r\nAttribute VB_Name = \"Class1\"\r\nAttribute VB_Exposed = False\r\nPublic Value As Long\r\n")];
        recs.push(Rec::new(PROJECTMODULES, (modules.len() as u16).to_le_bytes().to_vec()));
        recs.push(Rec::new(0x0013, vec![0xFF, 0xFF]));
        let mut streams = Vec::new();
        for (name, std, src) in modules {
            // A made-up p-code cache before the source.
            let cache = vec![0xAAu8; 37];
            recs.push(Rec::new(MODULENAME, name.as_bytes().to_vec()));
            recs.push(Rec::new(MODULENAMEUNICODE, to_utf16(name)));
            recs.push(Rec::new(MODULESTREAMNAME, name.as_bytes().to_vec()));
            recs.push(Rec::new(MODULESTREAMNAME_UNICODE, to_utf16(name)));
            recs.push(Rec::new(0x001C, Vec::new()));
            recs.push(Rec::new(0x0048, Vec::new()));
            recs.push(Rec::new(MODULEOFFSET, (cache.len() as u32).to_le_bytes().to_vec()));
            recs.push(Rec::new(0x001E, vec![0; 4]));
            recs.push(Rec::new(0x002C, vec![0xFF, 0xFF]));
            recs.push(Rec::new(if std { MODULETYPE_PROCEDURAL } else { 0x0022 }, Vec::new()));
            recs.push(Rec::new(MODULE_TERMINATOR, Vec::new()));
            let mut data = cache;
            data.extend(compress(&crate::ole::encode_cp(src, cp)));
            streams.push((name, data));
        }
        recs.push(Rec::new(0x0010, Vec::new()));
        let (cmg, dpb, gc) = if locked { (encrypt(&[1, 0, 0, 0], 0x42), encrypt(&[0xFF; 29], 0x17), encrypt(&[0], 0x7C)) } else { (encrypt(&[0, 0, 0, 0], 0x42), encrypt(&[0], 0x17), encrypt(&[0xFF], 0x7C)) };
        let project = format!("ID=\"{{11111111-2222-3333-4444-555555555555}}\"\r\nDocument=ThisWorkbook/&H00000000\r\nDocument=Sheet1/&H00000000\r\nModule=Module1\r\nClass=Class1\r\nName=\"VBAProject\"\r\nHelpContextID=\"0\"\r\nVersionCompatible32=\"393222000\"\r\nCMG=\"{cmg}\"\r\nDPB=\"{dpb}\"\r\nGC=\"{gc}\"\r\n\r\n[Host Extender Info]\r\n&H00000001={{3832D640-CF90-11CF-8E43-00A0C911005A}};VBE;&H00000000\r\n\r\n[Workspace]\r\nThisWorkbook=0, 0, 0, 0, C\r\nModule1=26, 26, 1062, 609, Z\r\n");
        let mut c = cfb::CompoundFile::create_with_version(cfb::Version::V3, Cursor::new(Vec::new())).unwrap();
        c.create_storage("/VBA").unwrap();
        let mut put = |p: &str, d: &[u8]| c.create_stream(p).unwrap().write_all(d).unwrap();
        put("/VBA/dir", &compress(&write_records(&recs)));
        put("/VBA/_VBA_PROJECT", &[0xCC, 0x61, 0xB5, 0x00, 0x00, 0x01, 0x00, 0xFF, 0x09, 0x04]);
        put("/VBA/__SRP_0", &[1, 2, 3]);
        for (name, data) in &streams {
            put(&format!("/VBA/{name}"), data);
        }
        put("/PROJECT", project.as_bytes());
        let mut wm = Vec::new();
        for (name, _, _) in modules {
            wm.extend(name.as_bytes());
            wm.push(0);
            wm.extend(to_utf16(name));
            wm.extend([0, 0]);
        }
        wm.extend([0, 0]);
        put("/PROJECTwm", &wm);
        c.flush().unwrap();
        c.into_inner().into_inner()
    }

    #[test]
    fn references() {
        let p = read(&sample(false)).unwrap();
        let refs = p.references();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, "stdole");
        assert_eq!(refs[0].guid.as_deref(), Some("{00020430-0000-0000-C000-000000000046}"));
        assert_eq!((refs[0].path.as_str(), refs[0].description.as_str(), refs[0].project), ("C:\\Windows\\System32\\stdole2.tlb", "OLE Automation", false));
        let r = Reference::from_libid("Book2".into(), "*\\CC:\\data\\Book2.xlsm", true);
        assert_eq!((r.guid, r.path.as_str()), (None, "C:\\data\\Book2.xlsm"));
    }

    #[test]
    fn project_properties_and_password() {
        let bin = new_workbook_project("ThisWorkbook", &[]).unwrap();
        let mut p = read(&bin).unwrap();
        assert!(!p.has_password() && p.password_matches("") && !p.password_matches("x"));
        p.name = "Informes".into();
        p.description = "Cierre \"mensual\"\nde ventas".into();
        p.set_password(Some("Año-1")).unwrap();
        assert!(p.changed());
        let back = read(&p.write().unwrap()).unwrap();
        assert_eq!((back.name.as_str(), back.description.as_str()), ("Informes", "Cierre 'mensual' de ventas"));
        let text = back.project_stream().unwrap();
        assert!(text.contains("Name=\"Informes\"\r\nDescription=\"Cierre 'mensual' de ventas\"\r\n"), "{text}");
        assert!(back.protected && back.has_password() && !back.changed());
        assert!(back.password_matches("Año-1") && !back.password_matches("año-1") && !back.password_matches(""));
        // Locked: no new password until opened with it.
        let mut back = back;
        assert!(back.set_password(None).is_err());
        back.protected = false;
        back.modules[0].code = "Sub A()\nEnd Sub\n".into();
        // Code changed while opened keeps the lock.
        let kept = read(&back.write().unwrap()).unwrap();
        assert!(kept.protected && kept.password_matches("Año-1"));
        back.set_password(None).unwrap();
        let open = read(&back.write().unwrap()).unwrap();
        assert!(!open.protected && !open.has_password());
    }

    #[test]
    fn password_hash_nulls() {
        let mut hash = [7u8; 20];
        hash[0] = 0;
        hash[19] = 0;
        let d = password_hash_data([0, 5, 0, 9], hash);
        assert_eq!(d.len(), 29);
        // Key bits 0 and 2 clear, hash bits 4 and 23 clear.
        assert_eq!(&d[..4], &[0xFF, 0b0101_0111, 0xFF, 0xFE]);
        assert_eq!(&d[4..8], &[1, 5, 1, 9]);
        let enc = super::encrypt(&d, "{X}", 0x33, &[1, 2, 3]);
        let Stored::Hash(key, back) = stored_password(&format!("ID=\"{{X}}\"\r\nDPB=\"{enc}\"\r\n")) else { panic!() };
        assert_eq!((key, back), ([0, 5, 0, 9], hash));
    }

    #[test]
    fn new_project_reads_back() {
        let bin = new_workbook_project("ThisWorkbook", &["Sheet1".into(), "Hoja2".into()]).unwrap();
        let p = read(&bin).unwrap();
        assert_eq!(p.name, "VBAProject");
        assert!(!p.protected);
        let kinds: Vec<(&str, ModuleKind)> = p.modules.iter().map(|m| (m.name.as_str(), m.kind)).collect();
        assert_eq!(kinds, [("ThisWorkbook", ModuleKind::Document), ("Sheet1", ModuleKind::Document), ("Hoja2", ModuleKind::Document)]);
        assert!(p.modules.iter().all(|m| m.code.trim().is_empty()));
        assert!(p.modules[0].attributes.contains("VB_Base = \"0{00020819-0000-0000-C000-000000000046}\""));
        assert!(p.modules[1].attributes.contains("VB_PredeclaredId = True"));
        let mut ole = cfb::CompoundFile::open(Cursor::new(&bin)).unwrap();
        assert_eq!(stream(&mut ole, "/VBA/_VBA_PROJECT").unwrap(), EMPTY_VBA_PROJECT);
        let text = String::from_utf8(stream(&mut ole, "/PROJECT").unwrap()).unwrap();
        assert!(text.contains("Document=ThisWorkbook/&H00000000\r\nDocument=Sheet1/&H00000000\r\nDocument=Hoja2/&H00000000\r\n"));
        // The values decrypt to an open project: not locked, no password, visible.
        let value = |k: &str| decrypt(text.lines().find_map(|l| l.strip_prefix(k)?.strip_prefix('=')).unwrap()).unwrap();
        assert_eq!((value("CMG"), value("DPB"), value("GC")), (vec![0; 4], vec![0], vec![0xFF]));
        // Every project gets its own ID.
        let id = |b: &[u8]| String::from_utf8(stream(&mut cfb::CompoundFile::open(Cursor::new(b)).unwrap(), "/PROJECT").unwrap()).unwrap().lines().next().unwrap().to_string();
        assert_ne!(id(&bin), id(&new_workbook_project("ThisWorkbook", &[]).unwrap()));
    }

    #[test]
    fn new_document_and_presentation_projects() {
        let mut p = read(&new_document_project()).unwrap();
        assert_eq!(p.modules.iter().map(|m| (m.name.as_str(), m.kind)).collect::<Vec<_>>(), [("ThisDocument", ModuleKind::Document)]);
        p.modules[0].code = "Private Sub Document_Open()\nEnd Sub\n".into();
        let back = read(&p.write().unwrap()).unwrap();
        assert!(back.modules[0].code.contains("Document_Open"));
        assert!(back.modules[0].attributes.contains("00020906"));
        let mut p = read(&new_presentation_project()).unwrap();
        assert!(p.modules.is_empty());
        let mut m = VbaModule::standard(&p.next_name("Module"));
        m.code = "Sub Hola()\nEnd Sub\n".into();
        p.modules.push(m);
        let back = read(&p.write().unwrap()).unwrap();
        assert_eq!(back.modules[0].name, "Module1");
    }

    #[test]
    fn class_modules_and_files() {
        let mut p = read(&new_workbook_project("ThisWorkbook", &[]).unwrap()).unwrap();
        let mut c = VbaModule::class("Point");
        c.code = "Public X As Long\n".into();
        p.modules.push(c);
        let back = read(&p.write().unwrap()).unwrap();
        let c = &back.modules[1];
        assert_eq!((c.name.as_str(), c.kind, c.code.as_str()), ("Point", ModuleKind::Class, "Public X As Long\n"));
        assert!(c.attributes.contains("FCFB3D2A"));
        // Exported and imported back, as the VBA editor does.
        let cls = c.export(1252);
        let text = String::from_utf8(cls.clone()).unwrap();
        assert!(text.starts_with("VERSION 1.0 CLASS\r\nBEGIN\r\n  MultiUse = -1  'True\r\nEND\r\nAttribute VB_Name = \"Point\"\r\n"));
        assert!(!text.contains("VB_Base") && text.ends_with("Public X As Long\r\n"));
        let again = VbaModule::import(&cls, "Other", 1252);
        assert_eq!((again.name.as_str(), again.kind, again.code.as_str()), ("Point", ModuleKind::Class, "Public X As Long\n"));
        assert!(again.attributes.contains("VB_Base") && again.attributes.contains("VB_Exposed = False"));
        // A .bas in the code page: accents survive.
        let mut m = VbaModule::standard("Tools");
        m.code = "Sub A()\n    MsgBox \"año\"\nEnd Sub".into();
        let bas = m.export(1252);
        assert!(bas.starts_with(b"Attribute VB_Name = \"Tools\"\r\nSub A()") && bas.contains(&0xF1));
        let again = VbaModule::import(&bas, "x", 1252);
        assert_eq!((again.name.as_str(), again.kind), ("Tools", ModuleKind::Standard));
        assert_eq!(again.code, "Sub A()\n    MsgBox \"año\"\nEnd Sub\n");
        // No name in the file: the one given; PredeclaredId kept.
        let m = VbaModule::import(b"VERSION 1.0 CLASS\nBEGIN\nEND\nAttribute VB_PredeclaredId = True\nDim a\n", "Fallback", 1252);
        assert_eq!((m.name.as_str(), m.code.as_str()), ("Fallback", "Dim a\n"));
        assert!(m.attributes.contains("VB_PredeclaredId = True") && !m.attributes.contains("VB_PredeclaredId = False"));
        assert_eq!(VbaModule::import(b"Sub B()\nEnd Sub\n", "Module2", 1252).code, "Sub B()\nEnd Sub\n");
    }

    #[test]
    fn forms_are_written_with_their_designs() {
        let mut p = read(&new_workbook_project("ThisWorkbook", &[]).unwrap()).unwrap();
        let mut design = FormStreams::new();
        design.insert("\u{3}VBFrame".into(), "VERSION 5.00\r\nBegin {C62A69F0-16DC-11CE-9E98-00AA00574A4F} Old \r\n   Caption         =   \"Año\"\r\nEnd\r\n".as_bytes().to_vec());
        design.insert("f".into(), vec![1, 2, 3]);
        design.insert("o".into(), Vec::new());
        let mut frame = vec![1, 0, 0xFE, 0xFF, 3, 0x0A, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF];
        frame.extend([0x20, 0x20, 0x18, 0x6E, 0x60, 0xF4, 0xCE, 0x11, 0x9B, 0xCD, 0x00, 0xAA, 0x00, 0x60, 0x8E, 0x01]);
        design.insert("i02/\u{1}CompObj".into(), frame);
        design.insert("i02/f".into(), vec![9]);
        let mut m = VbaModule::form("UserForm1", design.clone());
        m.code = "Private Sub UserForm_Click()\nEnd Sub\n".into();
        p.modules.push(m);
        let bin = p.write().unwrap();
        let back = read(&bin).unwrap();
        let f = &back.modules[1];
        assert_eq!((f.name.as_str(), f.kind), ("UserForm1", ModuleKind::Form));
        assert!(f.attributes.contains("VB_Base = \"0{") && f.attributes.contains("VB_PredeclaredId = True"));
        let streams = back.form_streams(1).unwrap();
        assert_eq!(streams["f"], [1, 2, 3]);
        assert_eq!(streams["i02/f"], [9]);
        let text = String::from_utf8(streams["\u{3}VBFrame"].clone()).unwrap();
        assert!(text.contains("} UserForm1 \r\n") && text.contains("Año"), "{text}");
        let mut ole = cfb::CompoundFile::open(Cursor::new(&bin)).unwrap();
        assert_eq!(ole.entry("/UserForm1/i02").unwrap().clsid().to_string(), "6e182020-f460-11ce-9bcd-00aa00608e01");
        let project = String::from_utf8(stream(&mut ole, "/PROJECT").unwrap()).unwrap();
        assert!(project.contains("BaseClass=UserForm1\r\n"), "{project}");
        // Renamed: its design goes with it, named anew; others are kept as
        // they are.
        let mut p = back;
        p.modules[1].name = "Picker".into();
        let back = read(&p.write().unwrap()).unwrap();
        let streams = back.form_streams(1).unwrap();
        assert_eq!(streams["i02/f"], [9]);
        assert!(String::from_utf8(streams["\u{3}VBFrame"].clone()).unwrap().contains("} Picker \r\n"));
        let bin = back.write().unwrap();
        let ole = cfb::CompoundFile::open(Cursor::new(&bin)).unwrap();
        assert!(ole.is_storage("/Picker/i02") && !ole.exists("/UserForm1"));
        assert_eq!(ole.entry("/Picker/i02").unwrap().clsid().to_string(), "6e182020-f460-11ce-9bcd-00aa00608e01");
        // A new design replaces the old; a removed form takes its own away.
        let mut p = read(&bin).unwrap();
        let mut d = FormStreams::new();
        d.insert("f".into(), vec![7]);
        p.modules[1].form = Some(d);
        let back = read(&p.write().unwrap()).unwrap();
        let streams = back.form_streams(1).unwrap();
        assert_eq!((streams.len(), streams["f"].as_slice()), (1, [7].as_slice()));
        let mut p = back;
        p.modules.remove(1);
        let bin = p.write().unwrap();
        assert!(!cfb::CompoundFile::open(Cursor::new(&bin)).unwrap().exists("/Picker"));
        // Names a compound file can't hold are refused.
        let p = read(&bin).unwrap();
        for bad in ["../f", "a//b", "./f", "x:y"] {
            let mut d = FormStreams::new();
            d.insert(bad.into(), vec![1]);
            let mut p = p.clone();
            p.modules.push(VbaModule::form("Bad", d));
            assert!(p.write().is_err(), "{bad}");
        }
    }

    #[test]
    fn forms_are_exported_and_imported_as_frm_and_frx() {
        let mut p = read(&new_workbook_project("ThisWorkbook", &[]).unwrap()).unwrap();
        let mut design = FormStreams::new();
        design.insert("\u{3}VBFrame".into(), "VERSION 5.00\r\nBegin {C62A69F0-16DC-11CE-9E98-00AA00574A4F} Picker \r\n   Caption         =   \"Año\"\r\n   StartUpPosition =   1  'CenterOwner\r\nEnd\r\n".as_bytes().to_vec());
        design.insert("f".into(), vec![1, 2, 3]);
        design.insert("o".into(), vec![4]);
        design.insert("i02/f".into(), vec![9]);
        let mut m = VbaModule::form("Picker", design);
        m.code = "Private Sub UserForm_Click()\nEnd Sub\n".into();
        p.modules.push(m);
        let (frm, frx) = p.export_form(1, "Picker.frx", 1252).unwrap();
        let text = String::from_utf8(frm.clone()).unwrap_or_else(|_| crate::ole::decode_cp(&frm, 1252));
        assert!(text.starts_with("VERSION 5.00\r\nBegin {C62A69F0-16DC-11CE-9E98-00AA00574A4F} Picker \r\n"), "{text}");
        assert!(text.contains("   OleObjectBlob   =   \"Picker.frx\":0000\r\n   StartUpPosition") || text.contains("OleObjectBlob   =   \"Picker.frx\":0000\r\nEnd\r\n"), "{text}");
        assert!(text.contains("End\r\nAttribute VB_Name = \"Picker\"\r\n") && text.contains("VB_PredeclaredId = True") && !text.contains("VB_Base"), "{text}");
        assert!(text.ends_with("Private Sub UserForm_Click()\r\nEnd Sub\r\n"));
        let ole = cfb::CompoundFile::open(Cursor::new(&frx)).unwrap();
        assert_eq!(ole.root_entry().clsid().to_string(), "c62a69f0-16dc-11ce-9e98-00aa00574a4f");
        assert!(ole.is_stream("/f") && ole.is_stream("/i02/f") && !ole.exists("/\u{3}VBFrame"));
        // Back in: the same design and code, under the name it has.
        let files = |name: &str| (name == "Picker.frx").then(|| frx.clone());
        let back = VbaModule::import_form(&frm, &files, "Other", 1252).unwrap();
        assert_eq!((back.name.as_str(), back.kind), ("Picker", ModuleKind::Form));
        assert_eq!(back.code, "Private Sub UserForm_Click()\nEnd Sub\n");
        let streams = back.form.unwrap();
        assert_eq!((streams["f"].as_slice(), streams["o"].as_slice(), streams["i02/f"].as_slice()), ([1, 2, 3].as_slice(), [4].as_slice(), [9].as_slice()));
        let frame = String::from_utf8(streams["\u{3}VBFrame"].clone()).unwrap();
        assert_eq!(frame, "VERSION 5.00\r\nBegin {C62A69F0-16DC-11CE-9E98-00AA00574A4F} Picker \r\n   Caption         =   \"Año\"\r\n   StartUpPosition =   1  'CenterOwner\r\nEnd\r\n");
        // What isn't a form's, or has no design, is refused.
        assert!(VbaModule::import_form(b"Attribute VB_Name = \"M\"\r\n", &files, "M", 1252).is_err());
        assert!(VbaModule::import_form(&frm, &|_: &str| None, "M", 1252).is_err());
        assert!(VbaModule::import_form(&frm, &|_: &str| Some(b"not a compound file".to_vec()), "M", 1252).is_err());
        let other = text.replace("C62A69F0-16DC-11CE-9E98-00AA00574A4F", "00000000-0000-0000-0000-000000000000");
        assert!(VbaModule::import_form(other.as_bytes(), &files, "M", 1252).is_err());
        let offset = text.replace(":0000", ":0010");
        assert!(VbaModule::import_form(offset.as_bytes(), &files, "M", 1252).is_err());
    }

    #[test]
    fn new_project_takes_code() {
        let mut p = read(&new_workbook_project("ThisWorkbook", &["Sheet1".into()]).unwrap()).unwrap();
        p.modules[0].code = "Private Sub Workbook_Open()\nEnd Sub\n".into();
        let mut m = VbaModule::standard(&p.next_name("Module"));
        m.code = "Sub Hola()\n    MsgBox \"año\"\nEnd Sub\n".into();
        p.modules.push(m);
        let back = read(&p.write().unwrap()).unwrap();
        assert_eq!(back.modules.len(), 3);
        assert_eq!(back.modules[0].code.trim(), "Private Sub Workbook_Open()\nEnd Sub");
        assert_eq!((back.modules[2].name.as_str(), back.modules[2].kind), ("Module1", ModuleKind::Standard));
        assert!(back.modules[2].code.contains("MsgBox \"año\""));
        assert!(!back.protected);
    }

    #[test]
    fn new_project_checks_names() {
        assert!(new_workbook_project("ThisWorkbook", &["sheet1".into(), "Sheet1".into()]).is_err());
        assert!(new_workbook_project("ThisWorkbook", &["Hoja 1".into()]).is_err());
        assert!(new_workbook_project("ThisWorkbook", &["Año".into()]).is_err());
        assert!(new_workbook_project("1Sheet", &[]).is_err());
        assert!(new_workbook_project("ThisWorkbook", &["dir".into()]).is_err());
        assert!(new_workbook_project("ThisWorkbook", &["A".repeat(32)]).is_err());
        assert!(new_workbook_project("ThisWorkbook", &["A".repeat(31)]).is_ok());
    }

    #[test]
    fn new_documents_are_written_as_such() {
        let mut p = read(&new_workbook_project("ThisWorkbook", &[]).unwrap()).unwrap();
        p.modules.push(VbaModule::sheet("Sheet1"));
        p.modules.push(VbaModule::standard("Module1"));
        let bin = p.write().unwrap();
        let back = read(&bin).unwrap();
        let kinds: Vec<(&str, ModuleKind)> = back.modules.iter().map(|m| (m.name.as_str(), m.kind)).collect();
        assert_eq!(kinds, [("ThisWorkbook", ModuleKind::Document), ("Sheet1", ModuleKind::Document), ("Module1", ModuleKind::Standard)]);
        let mut ole = cfb::CompoundFile::open(Cursor::new(&bin)).unwrap();
        let text = String::from_utf8(stream(&mut ole, "/PROJECT").unwrap()).unwrap();
        assert!(text.contains("Document=Sheet1/&H00000000\r\n") && text.contains("Module=Module1\r\n"), "{text}");
        let recs = parse_records(&decompress(&stream(&mut ole, "/VBA/dir").unwrap()).unwrap()).unwrap();
        let types: Vec<u16> = recs.iter().filter(|r| matches!(r.id, 0x0021 | 0x0022)).map(|r| r.id).collect();
        assert_eq!(types, [0x0022, 0x0022, 0x0021]);
        assert!(back.modules[1].attributes.contains("00020820"));
    }

    fn names(v: &[Option<&str>]) -> Vec<Option<String>> {
        v.iter().map(|n| n.map(str::to_string)).collect()
    }

    #[test]
    fn match_book_names_sheets() {
        // A new project: every sheet gets a code name and a module.
        let mut p = read(&new_workbook_project("ThisWorkbook", &[]).unwrap()).unwrap();
        let (mut book, mut sheets) = (None, names(&[None, Some("Datos"), None, Some("datos"), Some("Module1"), Some("Mal nombre")]));
        p.modules.push(VbaModule::standard("Module1"));
        assert!(p.match_book(&mut book, &mut sheets));
        assert_eq!(book.as_deref(), Some("ThisWorkbook"));
        assert_eq!(sheets, names(&[Some("Sheet1"), Some("Datos"), Some("Sheet2"), Some("Sheet3"), Some("Sheet4"), Some("Sheet5")]));
        let docs: Vec<&str> = p.modules.iter().filter(|m| m.kind == ModuleKind::Document).map(|m| m.name.as_str()).collect();
        assert_eq!(docs, ["ThisWorkbook", "Datos", "Sheet1", "Sheet2", "Sheet3", "Sheet4", "Sheet5"]);
        // Stable once matched.
        assert!(!p.match_book(&mut book, &mut sheets));
        // A sheet deleted: its empty module goes, one with code stays; a
        // new sheet takes the spare module.
        p.modules.iter_mut().find(|m| m.name == "Sheet2").unwrap().code = "Sub X()\nEnd Sub\n".into();
        let mut sheets = names(&[Some("Sheet1"), Some("Datos"), Some("Sheet3"), Some("Sheet4"), Some("Sheet5")]);
        assert!(!p.match_book(&mut book, &mut sheets));
        assert!(p.modules.iter().any(|m| m.name == "Sheet2"));
        let mut sheets = names(&[Some("Sheet1"), Some("Datos"), Some("Sheet3"), Some("Sheet4"), Some("Sheet5"), None]);
        assert!(p.match_book(&mut book, &mut sheets));
        assert_eq!(sheets[5].as_deref(), Some("Sheet2"));
        let mut sheets = names(&[Some("Sheet1"), Some("Datos")]);
        assert!(p.match_book(&mut book, &mut sheets));
        assert!(!p.modules.iter().any(|m| matches!(m.name.as_str(), "Sheet3" | "Sheet4" | "Sheet5")));
        assert!(p.modules.iter().any(|m| m.name == "Sheet2"));
    }

    #[test]
    fn match_book_leaves_unknown_sheets() {
        // Office's sample: Sheet1's module, but the sheets carry no code
        // names (an .xls): which sheet is which isn't known.
        let mut p = read(&sample(false)).unwrap();
        let (mut book, mut sheets) = (None, names(&[None, None]));
        assert!(p.match_book(&mut book, &mut sheets));
        assert_eq!(book.as_deref(), Some("ThisWorkbook"));
        assert_eq!(sheets, names(&[None, None]));
        assert_eq!(p.modules.len(), 4);
        // Named: the other sheet gets a module of its own.
        let mut sheets = names(&[Some("Sheet1"), None]);
        assert!(p.match_book(&mut book, &mut sheets));
        assert_eq!(sheets, names(&[Some("Sheet1"), Some("Sheet2")]));
        let back = read(&p.write().unwrap()).unwrap();
        assert!(back.modules.iter().any(|m| m.name == "Sheet2" && m.kind == ModuleKind::Document));
        // Locked: never touched.
        let mut p = read(&sample(true)).unwrap();
        let (mut book, mut sheets) = (None, names(&[Some("Sheet1"), None]));
        assert!(!p.match_book(&mut book, &mut sheets));
        assert_eq!((book, sheets), (None, names(&[Some("Sheet1"), None])));
    }

    #[test]
    fn names_fit_the_code_page() {
        let p = read(&new_workbook_project("ThisWorkbook", &[]).unwrap()).unwrap();
        assert_eq!(p.check_name("Año", None), Ok(()));
        assert_eq!(p.check_name("模块", None), Err(NameError::Invalid));
    }

    #[test]
    fn reads_modules() {
        let bin = sample(false);
        let p = read(&bin).unwrap();
        assert_eq!(p.name, "VBAProject");
        assert!(!p.protected);
        let kinds: Vec<(&str, ModuleKind)> = p.modules.iter().map(|m| (m.name.as_str(), m.kind)).collect();
        assert_eq!(kinds, [("ThisWorkbook", ModuleKind::Document), ("Sheet1", ModuleKind::Document), ("Module1", ModuleKind::Standard), ("Class1", ModuleKind::Class)]);
        assert_eq!(p.modules[2].code, "Sub Hello()\n    MsgBox \"Hola, año\"\nEnd Sub\n");
        assert_eq!(p.modules[2].attributes, "Attribute VB_Name = \"Module1\"\n");
        assert_eq!(p.modules[3].code, "Public Value As Long\n");
        assert!(p.modules[3].attributes.starts_with("VERSION 1.0 CLASS\nBEGIN\n"));
        assert_eq!(p.modules[0].code, "Private Sub Workbook_Open()\nEnd Sub\n");
        assert!(read(&sample(true)).unwrap().protected);
        // Unchanged: the same bytes.
        assert_eq!(p.write().unwrap(), bin);
    }

    #[test]
    fn dir_roundtrip() {
        let bin = sample(false);
        let mut ole = cfb::CompoundFile::open(Cursor::new(&bin[..])).unwrap();
        let dir = decompress(&stream(&mut ole, "/VBA/dir").unwrap()).unwrap();
        let recs = parse_records(&dir).unwrap();
        assert_eq!(write_records(&recs), dir);
        assert!(recs.iter().any(|r| r.id == PROJECTVERSION && r.data.len() == 6 && r.size == 4));
    }

    #[test]
    fn edits_are_written() {
        let mut p = read(&sample(false)).unwrap();
        p.modules[2].code = "Sub Hello()\n    MsgBox \"Changed\"\nEnd Sub\n".into();
        p.modules[0].code.push_str("' note\n");
        p.modules.remove(3);
        let mut m = VbaModule::standard("Tools");
        m.code = "Function Twice(x)\n    Twice = 2 * x\nEnd Function".into();
        p.modules.push(m);
        assert_eq!(p.check_name("module1", None), Err(NameError::Taken));
        assert_eq!(p.check_name("1abc", None), Err(NameError::Invalid));
        assert_eq!(p.next_name("Module"), "Module2");
        let bin = p.write().unwrap();
        let back = read(&bin).unwrap();
        let names: Vec<&str> = back.modules.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["ThisWorkbook", "Sheet1", "Module1", "Tools"]);
        assert_eq!(back.modules[2].code, "Sub Hello()\n    MsgBox \"Changed\"\nEnd Sub\n");
        assert_eq!(back.modules[3].code, "Function Twice(x)\n    Twice = 2 * x\nEnd Function\n");
        assert_eq!(back.modules[3].kind, ModuleKind::Standard);
        assert!(back.modules[0].code.ends_with("' note\n"));
        let mut ole = cfb::CompoundFile::open(Cursor::new(&bin[..])).unwrap();
        // No p-code left: Office compiles the source.
        assert_eq!(stream(&mut ole, "/VBA/_VBA_PROJECT").unwrap(), EMPTY_VBA_PROJECT);
        assert!(!ole.exists("/VBA/__SRP_0") && !ole.exists("/VBA/Class1"));
        assert_eq!(stream(&mut ole, "/VBA/Module1").unwrap()[0], 1);
        let project = String::from_utf8(stream(&mut ole, "/PROJECT").unwrap()).unwrap();
        assert!(project.contains("Module=Module1\r\nModule=Tools\r\n") && !project.contains("Class1"), "{project}");
        let wm = stream(&mut ole, "/PROJECTwm").unwrap();
        assert!(wm.windows(6).any(|w| w == b"Tools\0"));
    }

    #[test]
    fn rename() {
        let mut p = read(&sample(false)).unwrap();
        p.modules[2].name = "Greetings".into();
        let bin = p.write().unwrap();
        let back = read(&bin).unwrap();
        assert_eq!(back.modules[2].name, "Greetings");
        assert_eq!(back.modules[2].attributes, "Attribute VB_Name = \"Greetings\"\n");
        let mut ole = cfb::CompoundFile::open(Cursor::new(&bin[..])).unwrap();
        assert!(ole.is_stream("/VBA/Greetings") && !ole.exists("/VBA/Module1"));
        let project = String::from_utf8(stream(&mut ole, "/PROJECT").unwrap()).unwrap();
        assert!(project.contains("Module=Greetings\r\n") && project.contains("Greetings=26, 26"), "{project}");
    }

    #[test]
    fn header_split() {
        let (h, c) = split_header("Attribute VB_Name = \"M\"\nOption Explicit\nAttribute VB_Name = \"x\"\n");
        assert_eq!(h, "Attribute VB_Name = \"M\"\n");
        assert_eq!(c, "Option Explicit\nAttribute VB_Name = \"x\"\n");
        assert_eq!(set_vb_name("VERSION 1.0 CLASS\nBEGIN\nEND\n", "C"), "VERSION 1.0 CLASS\nBEGIN\nEND\nAttribute VB_Name = \"C\"\n");
    }

    #[test]
    fn damaged_streams() {
        // Streams that fail to read to their end (a fuzzed file): the
        // edited project is still written, keeping what reads.
        let mut bin = sample(false);
        // The mini stream's chain ends after its first sector.
        let sector = |s: u32| (s as usize + 1) << 9;
        let word = |b: &[u8], p: usize| u32::from_le_bytes(b[p..p + 4].try_into().unwrap());
        let root = sector(word(&bin, 48));
        let fat = sector(word(&bin, 76)) + word(&bin, root + 116) as usize * 4;
        bin[fat..fat + 4].copy_from_slice(&0xFFFF_FFFEu32.to_le_bytes());
        assert!(!streams_readable(&bin));
        let mut p = read(&bin).unwrap();
        p.modules[2].code.push_str("' more\n");
        let out = read(&p.write().unwrap()).unwrap();
        assert!(out.modules[2].code.ends_with("' more\n"));
    }

    #[test]
    fn signatures_follow_the_code() {
        use md5::Digest as _;
        let bin = sample(false);
        let (datas, forms) = content_normalized(&bin).unwrap();
        assert!(!forms);
        // Office's reading: the name, the constants (none), the stdole
        // reference and every line but the Attribute ones, joined.
        let code = "Private Sub Workbook_Open()End SubSub Hello()    MsgBox \"Hola, a\u{f1}o\"End SubVERSION 1.0 CLASSBEGIN  MultiUse = -1  'TrueENDPublic Value As Long";
        let mut office = b"VBAProject{".to_vec();
        office.extend(crate::ole::encode_cp(code, 1252));
        assert_eq!(datas[2], office);
        assert!(datas.iter().all(|d| d.ends_with(&crate::ole::encode_cp(code, 1252))));
        // A signed content (SpcIndirectDataContent) with the hash in it.
        let content = |h: &[u8]| [&[0x30, 0x2A, 0x04, h.len() as u8][..], h, &[0, 0]].concat();
        let v1 = content(&md5::Md5::digest(&datas[2]));
        assert!(signed_for(&bin, &v1));
        assert!(signed_for(&bin, &content(&sha2::Sha256::digest_sha(&datas[0]))), "Agile with SHA-256");
        // The same signature on other code: not good.
        let mut p = read(&bin).unwrap();
        p.modules[2].code.push_str("Sub Other()\nEnd Sub\n");
        let changed = p.write().unwrap();
        assert!(!signed_for(&changed, &v1));
        let mut p = read(&bin).unwrap();
        p.modules[0].code = p.modules[0].code.replace("Open", "Close");
        assert!(!signed_for(&p.write().unwrap(), &v1));
        // No hash of this project in it.
        assert!(!signed_for(&bin, &content(&[7; 16])));
        assert!(!signed_for(b"not a project", &v1));
    }

    #[test]
    fn v3_signatures_follow_lines_and_kinds() {
        use md5::Digest as _;
        let bin = sample(false);
        let datas = v3_normalized(&bin).unwrap();
        let line = |s: &str| crate::ole::encode_cp(s, 1252);
        // Lines one by one, the document's usual attributes left out but
        // ThisWorkbook's own base, and the project's properties after.
        assert!(datas[0].windows(18).any(|w| w == &line("Sub Hello()\n    Ms")[..]));
        assert!(datas[0].windows(31).any(|w| w == b"Attribute VB_Base = \"0{00020819"));
        assert!(!datas[0].windows(31).any(|w| w == b"Attribute VB_Base = \"0{00020820"));
        assert!(datas[0].ends_with(b"ModuleModule1ClassClass1NameVBAProjectHelpContextID0VersionCompatible32393222000Host Extender Info&H00000001={3832D640-CF90-11CF-8E43-00A0C911005A};VBE;&H00000000"));
        assert_ne!(datas[0], datas[1]);
        let content = |h: &[u8]| [&[0x30, 0x2A, 0x04, h.len() as u8][..], h, &[0, 0]].concat();
        let v3 = content(&sha2::Sha256::digest_sha(&datas[1]));
        assert!(signed_for_v3(&bin, &v3));
        assert!(!signed_for_v3(&bin, &content(&md5::Md5::digest(&datas[1]))));
        // A line break moved: the older hash can't tell, this one can.
        let v1 = content(&md5::Md5::digest(&content_normalized(&bin).unwrap().0[2]));
        let mut p = read(&bin).unwrap();
        p.modules[2].code = p.modules[2].code.replace("Sub Hello()\n    MsgBox", "Sub Hello()    \nMsgBox");
        let moved = p.write().unwrap();
        assert!(signed_for(&moved, &v1));
        assert!(!signed_for_v3(&moved, &v3));
        // A module said to be a document as well, or one of another kind.
        let with_project = |from: &str, to: &str| {
            let mut c = cfb::CompoundFile::open(Cursor::new(bin.clone())).unwrap();
            let text = String::from_utf8(stream(&mut c, "/PROJECT").unwrap()).unwrap();
            assert!(text.contains(from));
            c.create_stream("/PROJECT").unwrap().write_all(text.replace(from, to).as_bytes()).unwrap();
            c.flush().unwrap();
            c.into_inner().into_inner()
        };
        assert!(signed_for_v3(&with_project("Name=", "Name="), &v3));
        assert!(!signed_for_v3(&with_project("Class=Class1", "Document=Class1/&H00000000\r\nClass=Class1"), &v3));
        assert!(!signed_for_v3(&with_project("Module=Module1", "Document=Module1/&H00000000"), &v3));
        assert!(!signed_for_v3(&with_project("Class=Class1", "Document=Class1/&H00000000"), &v3));
        // Names as the reader decodes them: a name not in ASCII too.
        let mut p = read(&bin).unwrap();
        p.modules[3].name = "Clasé".into();
        let renamed = p.write().unwrap();
        let v3 = content(&sha2::Sha256::digest_sha(&v3_normalized(&renamed).unwrap()[1]));
        assert!(signed_for_v3(&renamed, &v3));
        let mut c = cfb::CompoundFile::open(Cursor::new(renamed.clone())).unwrap();
        let mut text = stream(&mut c, "/PROJECT").unwrap();
        let at = text.windows(6).position(|w| w == b"Class=").unwrap();
        text.splice(at..at, b"Document=Clas\xe9/&H00000000\r\n".iter().copied());
        c.create_stream("/PROJECT").unwrap().write_all(&text).unwrap();
        c.flush().unwrap();
        assert!(!signed_for_v3(&c.into_inner().into_inner(), &v3));
    }

    #[test]
    fn legacy_storage() {
        // An .xls keeps the project in _VBA_PROJECT_CUR.
        let bin = sample(false);
        let mut src = cfb::CompoundFile::open(Cursor::new(&bin[..])).unwrap();
        let mut c = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        c.create_stream("/Workbook").unwrap().write_all(&[0; 10]).unwrap();
        c.create_storage("/_VBA_PROJECT_CUR").unwrap();
        copy_tree(&mut src, Path::new("/"), &mut c, Path::new("/_VBA_PROJECT_CUR"), &|_| false).unwrap();
        c.flush().unwrap();
        let xls = c.into_inner().into_inner();
        let out = from_legacy(&xls, "_VBA_PROJECT_CUR").unwrap();
        assert_eq!(modules(&out).unwrap(), read(&bin).unwrap().modules);
        assert!(from_legacy(&xls, "Macros").is_none());
    }
}
