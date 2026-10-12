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
}

impl VbaModule {
    /// An empty standard module.
    pub fn standard(name: &str) -> VbaModule {
        VbaModule { name: name.to_string(), kind: ModuleKind::Standard, code: String::new(), attributes: format!("Attribute VB_Name = \"{name}\"\n"), origin: None }
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

#[derive(Clone, Debug)]
pub struct VbaProject {
    pub name: String,
    /// Code page of names and source code.
    pub code_page: u16,
    pub modules: Vec<VbaModule>,
    /// Locked with a password or for viewing: shown, not changed.
    pub protected: bool,
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
        let module = VbaModule { name, kind, code, attributes, origin: Some(read.len()) };
        read.push(ReadModule { recs: start..i, stream: stream_name, module });
    }
    let modules = read.iter().map(|m| m.module.clone()).collect();
    Ok(VbaProject { name, code_page, modules, protected: is_locked(&project), bin: bin.to_vec(), tail: i, records, head, read })
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

/// Why a module name can't be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameError {
    Empty,
    /// Not a VBA identifier (a letter, then letters, digits or `_`; up to 31).
    Invalid,
    Taken,
}

impl VbaProject {
    /// Whether the modules differ from the file's.
    pub fn changed(&self) -> bool {
        self.modules.len() != self.read.len() || self.modules.iter().zip(&self.read).any(|(m, r)| *m != r.module)
    }

    /// Whether `name` fits a new or renamed module (`except`: its index).
    pub fn check_name(&self, name: &str, except: Option<usize>) -> Result<(), NameError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(NameError::Empty);
        }
        let mut chars = name.chars();
        let first_ok = chars.next().is_some_and(|c| c.is_alphabetic());
        if !first_ok || name.chars().count() > 31 || !chars.all(|c| c.is_alphanumeric() || c == '_') || name.eq_ignore_ascii_case("dir") {
            return Err(NameError::Invalid);
        }
        let lower = name.to_lowercase();
        // Nor the stream of another module (it may be named otherwise).
        let stream_of = |m: &VbaModule| m.origin.and_then(|o| self.read.get(o)).map(|r| r.stream.to_lowercase());
        let taken = self.modules.iter().enumerate().any(|(i, m)| Some(i) != except && (m.name.to_lowercase() == lower || stream_of(m).as_deref() == Some(lower.as_str()))) || self.name.to_lowercase() == lower;
        if taken { Err(NameError::Taken) } else { Ok(()) }
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
        let cp = self.code_page;
        let mut recs: Vec<Rec> = self.records[..self.head].to_vec();
        for r in &mut recs {
            if r.id == PROJECTMODULES {
                *r = Rec::new(PROJECTMODULES, (self.modules.len() as u16).to_le_bytes().to_vec());
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
                    Rec::new(MODULETYPE_PROCEDURAL, Vec::new()),
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
            let source = crlf(&format!("{}{}", normalize(&attributes), m.code));
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
        let io = |e: std::io::Error| Error::new(e.to_string());
        let mut dst = cfb::CompoundFile::create_with_version(cfb::Version::V3, Cursor::new(Vec::new())).map_err(io)?;
        copy_tree(&mut src, Path::new("/"), &mut dst, Path::new("/"), &|p: &Path| {
            let s = p.to_string_lossy().replace('\\', "/");
            let lower = s.to_lowercase();
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
        let added: Vec<String> = self.modules.iter().filter(|m| m.origin.is_none()).map(|m| format!("{}={}", key(m.kind), m.name)).collect();
        out.splice(at..at, added);
        let mut s = out.join("\r\n");
        if !s.ends_with("\r\n") {
            s.push_str("\r\n");
        }
        s
    }
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
