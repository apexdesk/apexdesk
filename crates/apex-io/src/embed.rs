//! The data of embedded objects: what file an object holds, OLE packages
//! (`\u{1}Ole10Native`) and objects made from files ([MS-OLEDS]).

use std::io::{Cursor, Read, Write};
use std::sync::Arc;

use apex_doc::{Document, OleObject};

use crate::{Error, Format, Loaded};

/// CLSID of OLE packages (`Package`).
const CLSID_PACKAGE: &str = "0003000C-0000-0000-C000-000000000046";
const CFB_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

/// Content types of Office Open XML packages by extension.
const OOXML: [(&str, &str, &str, &str); 6] = [
    ("xlsx", "Excel.Sheet.12", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", "00020830-0000-0000-C000-000000000046"),
    ("xlsm", "Excel.SheetMacroEnabled.12", "application/vnd.ms-excel.sheet.macroEnabled.12", "00020832-0000-0000-C000-000000000046"),
    ("docx", "Word.Document.12", "application/vnd.openxmlformats-officedocument.wordprocessingml.document", "F4754C9B-64F5-4B40-8AF4-679732AC0607"),
    ("docm", "Word.DocumentMacroEnabled.12", "application/vnd.ms-word.document.macroEnabled.12", "18A06B6B-2F3F-4E2B-A611-52BE631B2D22"),
    ("pptx", "PowerPoint.Show.12", "application/vnd.openxmlformats-officedocument.presentationml.presentation", "CF4F55F4-8F87-4D47-80BB-5808164BB3F8"),
    ("pptm", "PowerPoint.ShowMacroEnabled.12", "application/vnd.ms-powerpoint.presentation.macroEnabled.12", "DC020317-E6E2-4A62-B9FA-B3EFE16626F4"),
];

/// Media type of an OLE object part in Office Open XML.
pub const CT_OLE: &str = "application/vnd.openxmlformats-officedocument.oleObject";
pub const REL_OLE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/oleObject";
pub const REL_PACKAGE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/package";

fn is_cfb(data: &[u8]) -> bool {
    data.starts_with(&CFB_MAGIC)
}

/// Extension of an Office Open XML or ODF package (by its main part).
fn package_ext(data: &[u8]) -> Option<&'static str> {
    if !data.starts_with(b"PK") {
        return None;
    }
    let z = crate::zip::ZipReader::new(data).ok()?;
    if let Ok(mime) = z.read_string("mimetype") {
        let m = mime.trim();
        return Some(match m.strip_prefix("application/vnd.oasis.opendocument.")? {
            "spreadsheet" => "ods",
            "text" => "odt",
            "presentation" => "odp",
            "graphics" => "odg",
            "chart" => "odc",
            "formula" => "odf",
            _ => return None,
        });
    }
    let ct = z.read_string("[Content_Types].xml").ok()?;
    let has = |s: &str| ct.contains(s);
    Some(if has("macroEnabled.main+xml") && z.contains("xl/workbook.xml") {
        "xlsm"
    } else if z.contains("xl/workbook.xml") {
        "xlsx"
    } else if has("document.macroEnabled") {
        "docm"
    } else if z.contains("word/document.xml") {
        "docx"
    } else if has("presentation.macroEnabled") {
        "pptm"
    } else if z.contains("ppt/presentation.xml") {
        "pptx"
    } else {
        return None;
    })
}

fn read_stream(c: &mut cfb::CompoundFile<Cursor<&[u8]>>, path: &str) -> Option<Vec<u8>> {
    let mut s = c.open_stream(path).ok()?;
    let mut out = Vec::new();
    s.read_to_end(&mut out).ok()?;
    Some(out)
}

/// The ProgID a compound file names in its `\u{1}CompObj` stream.
pub fn compobj_prog_id(data: &[u8]) -> Option<String> {
    let mut c = cfb::CompoundFile::open(Cursor::new(data)).ok()?;
    let s = read_stream(&mut c, "/\u{1}CompObj")?;
    // Header (28 bytes), then the user type, the clipboard format and the
    // ProgID as length-prefixed strings.
    let mut at = 28;
    let ansi = |at: &mut usize| -> Option<String> {
        let n = u32::from_le_bytes(s.get(*at..*at + 4)?.try_into().ok()?) as usize;
        *at += 4;
        if n == 0xFFFF_FFFF || n == 0xFFFF_FFFE {
            *at += 4;
            return Some(String::new());
        }
        let b = s.get(*at..*at + n)?;
        *at += n;
        Some(String::from_utf8_lossy(b).trim_end_matches('\0').to_string())
    };
    ansi(&mut at)?;
    ansi(&mut at)?;
    ansi(&mut at).filter(|p| !p.is_empty())
}

/// A file packaged in an `\u{1}Ole10Native` stream: its name and bytes.
fn parse_native(s: &[u8]) -> Option<(String, Vec<u8>)> {
    let mut at = 4;
    if s.get(at..at + 2)? != [2, 0] {
        // Some writers store just the size and the bytes.
        let n = u32::from_le_bytes(s.get(0..4)?.try_into().ok()?) as usize;
        return Some((String::new(), s.get(4..4 + n)?.to_vec()));
    }
    at += 2;
    let cstr = |at: &mut usize| -> Option<String> {
        let end = *at + s.get(*at..)?.iter().position(|&b| b == 0)?;
        let v = String::from_utf8_lossy(&s[*at..end]).into_owned();
        *at = end + 1;
        Some(v)
    };
    let label = cstr(&mut at)?;
    let path = cstr(&mut at)?;
    at += 4;
    let n = u32::from_le_bytes(s.get(at..at + 4)?.try_into().ok()?) as usize;
    at += 4 + n;
    let size = u32::from_le_bytes(s.get(at..at + 4)?.try_into().ok()?) as usize;
    at += 4;
    let data = s.get(at..at + size)?.to_vec();
    let name = if label.is_empty() { path.rsplit(['\\', '/']).next().unwrap_or_default().to_string() } else { label };
    Some((name, data))
}

/// The file an object holds: a name for it (with its extension) and its
/// bytes. Packages give their file, compound files holding an Office
/// Open XML package give the package, legacy .xls and .doc files and
/// PDFs themselves.
pub fn contents(o: &OleObject) -> (String, Vec<u8>) {
    let data = &o.data[..];
    let stem = |ext: &str| format!("{}.{ext}", object_stem(&o.prog_id));
    if let Some(ext) = package_ext(data) {
        return (stem(ext), data.to_vec());
    }
    if data.starts_with(b"%PDF") {
        return (stem("pdf"), data.to_vec());
    }
    if is_cfb(data) {
        if let Ok(mut c) = cfb::CompoundFile::open(Cursor::new(data)) {
            if let Some((name, bytes)) = read_stream(&mut c, "/\u{1}Ole10Native").and_then(|s| parse_native(&s)) {
                let name = if name.is_empty() { stem("bin") } else { name };
                return (name, bytes);
            }
            for s in ["/Package", "/package"] {
                if let Some(p) = read_stream(&mut c, s) {
                    let ext = package_ext(&p).unwrap_or("zip");
                    return (stem(ext), p);
                }
            }
            if let Some(p) = read_stream(&mut c, "/CONTENTS").filter(|p| p.starts_with(b"%PDF")) {
                return (stem("pdf"), p);
            }
            let ext = if c.is_stream("/WordDocument") {
                "doc"
            } else if c.is_stream("/Workbook") || c.is_stream("/Book") {
                "xls"
            } else if c.is_stream("/PowerPoint Document") {
                "ppt"
            } else {
                "bin"
            };
            return (stem(ext), data.to_vec());
        }
    }
    let ext = if o.ext.is_empty() || o.ext == "bin" { "bin" } else { o.ext.as_str() };
    (stem(ext), data.to_vec())
}

/// A file name stem for an object of `prog_id`.
fn object_stem(prog_id: &str) -> &'static str {
    let p = prog_id.to_ascii_lowercase();
    if p.starts_with("excel") || p.contains("calc") || p.contains("sheet") {
        "Worksheet"
    } else if p.starts_with("word") || p.contains("writer") || p.contains("text") {
        "Document"
    } else if p.starts_with("powerpoint") || p.contains("impress") {
        "Presentation"
    } else {
        "Object"
    }
}

/// What ApexDesk opens of an object in a tab (workbooks and documents
/// of Office and ODF): the file and its format.
pub fn openable(o: &OleObject) -> Option<(Vec<u8>, Format)> {
    let (_, data) = contents(o);
    let fmt = crate::sniff(&data)?;
    matches!(fmt, Format::Xlsx | Format::Ods | Format::Xls | Format::Xlsb | Format::Docx | Format::Odt | Format::Doc).then_some((data, fmt))
}

/// Reads a file of format `fmt` from memory.
pub fn load(data: &[u8], fmt: Format) -> Result<Loaded, Error> {
    // A reader that trips over a damaged object fails the object, not the app.
    std::panic::catch_unwind(|| load_unguarded(data, fmt)).unwrap_or_else(|_| Err(Error::new("the object is damaged and could not be read")))
}

fn load_unguarded(data: &[u8], fmt: Format) -> Result<Loaded, Error> {
    Ok(match fmt {
        Format::Xlsx => Loaded::Sheet(Box::new(crate::xlsx::read(data)?)),
        Format::Ods => Loaded::Sheet(Box::new(crate::ods::read(data)?)),
        Format::Csv => Loaded::Sheet(Box::new(crate::csv::read(data, None))),
        Format::Tsv => Loaded::Sheet(Box::new(crate::csv::read(data, Some(b'\t')))),
        Format::Xls => Loaded::Sheet(Box::new(crate::xls::read(data)?)),
        Format::Xlsb => Loaded::Sheet(Box::new(crate::xlsb::read(data)?)),
        Format::Docx => Loaded::Doc(Box::new(crate::docx::read(data)?)),
        Format::Odt => Loaded::Doc(Box::new(crate::odt::read(data)?)),
        Format::Rtf => Loaded::Doc(Box::new(crate::rtf::read(data)?)),
        Format::Doc => Loaded::Doc(Box::new(crate::doc::read(data)?)),
        Format::Markdown => Loaded::Doc(Box::new(crate::markdown::read(&String::from_utf8_lossy(data)))),
        Format::Text => Loaded::Doc(Box::new(Document::from_plain_text(&String::from_utf8_lossy(data)))),
        Format::Fods => Loaded::Sheet(Box::new(crate::ods::read(&crate::odf_flat::to_package(data)?)?)),
        Format::Fodt => Loaded::Doc(Box::new(crate::odt::read(&crate::odf_flat::to_package(data)?)?)),
        Format::Pdf => Loaded::Doc(Box::new(crate::pdf::read(data, None)?)),
        Format::Pptx => Loaded::Slides(Box::new(crate::pptx::read(data)?)),
        Format::Odp => Loaded::Slides(Box::new(crate::odp::read(data)?)),
        Format::Ppt => Loaded::Slides(Box::new(crate::ppt::read(data)?)),
        Format::Access => Loaded::Sheet(Box::new(crate::database::read_access(data)?)),
        Format::Dbase => Loaded::Sheet(Box::new(crate::database::read_dbase(data)?)),
        Format::Mail => Loaded::Doc(Box::new(crate::mail_msg::read_doc(data)?)),
    })
}

/// The object `o` with its file replaced by `file` (a workbook or document
/// edited in ApexDesk, in format `fmt`): stored the way the object was.
pub fn replaced(o: &OleObject, file: Vec<u8>, fmt: Format) -> OleObject {
    let ext = fmt.extension();
    let mut n = o.clone();
    let data = &o.data[..];
    if package_ext(data).is_some() || data.starts_with(b"%PDF") || !is_cfb(data) {
        n.data = Arc::from(file);
        return n;
    }
    let inner = cfb::CompoundFile::open(Cursor::new(data)).ok().map(|c| (c.is_stream("/\u{1}Ole10Native"), c.is_stream("/Package")));
    n.data = Arc::from(match inner {
        Some((true, _)) => package(&format!("{}.{ext}", object_stem(&o.prog_id)), &file),
        Some((_, true)) => wrap_package(&file, &o.prog_id),
        // A legacy .xls or .doc edited here comes back as xlsx or docx.
        _ => {
            n.prog_id = prog_id_for(ext).to_string();
            (n.ext, n.content_type) = (String::new(), String::new());
            file
        }
    });
    n
}

/// ProgID of an object made from a file with extension `ext`.
pub fn prog_id_for(ext: &str) -> &'static str {
    let e = ext.to_ascii_lowercase();
    if let Some(x) = OOXML.iter().find(|x| x.0 == e) {
        return x.1;
    }
    match e.as_str() {
        "xls" => "Excel.Sheet.8",
        "doc" => "Word.Document.8",
        "ppt" => "PowerPoint.Show.8",
        _ => "Package",
    }
}

/// An object made from a file: Office Open XML packages and legacy Office
/// files are embedded as themselves, anything else in an OLE package.
pub fn from_file(name: &str, data: Vec<u8>) -> OleObject {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    let sniffed = package_ext(&data);
    let prog = match sniffed {
        Some(e) if OOXML.iter().any(|x| x.0 == e) => prog_id_for(e),
        None if is_cfb(&data) && matches!(ext.as_str(), "xls" | "doc" | "ppt") => prog_id_for(&ext),
        _ => "Package",
    };
    let data = if prog == "Package" { package(name, &data) } else { data };
    OleObject { prog_id: prog.to_string(), data: Arc::from(data), ..Default::default() }
}

/// How an object is written in an Office Open XML package: its data,
/// extension, media type and relationship type. Objects Office can't
/// take as they are (ODF packages, loose files) go in an OLE package.
pub fn ooxml_part(o: &OleObject) -> (Arc<[u8]>, String, String, &'static str) {
    let data = &o.data;
    if !o.ext.is_empty() && !o.content_type.is_empty() {
        let package = (o.content_type.starts_with("application/vnd.openxmlformats-officedocument.") && o.content_type != CT_OLE) || o.content_type.contains("macroEnabled");
        let rel = if package { REL_PACKAGE } else { REL_OLE };
        return (data.clone(), o.ext.clone(), o.content_type.clone(), rel);
    }
    if let Some(x) = package_ext(data).and_then(|e| OOXML.iter().find(|x| x.0 == e)) {
        return (data.clone(), x.0.to_string(), x.2.to_string(), REL_PACKAGE);
    }
    if is_cfb(data) {
        return (data.clone(), "bin".into(), CT_OLE.into(), REL_OLE);
    }
    let (name, bytes) = contents(o);
    (Arc::from(package(&name, &bytes)), "bin".into(), CT_OLE.into(), REL_OLE)
}

/// How an object is written in an ODF package: a compound file for
/// `draw:object-ole` (Office Open XML packages go inside one, as Office
/// does in legacy files), or `None` for ODF packages, which are stored
/// as a sub-document.
pub fn odf_ole(o: &OleObject) -> Option<Vec<u8>> {
    let data = &o.data;
    match package_ext(data) {
        Some(e) if OOXML.iter().any(|x| x.0 == e) => Some(wrap_package(data, prog_id_for(e))),
        Some(_) => None,
        None if is_cfb(data) => Some(data.to_vec()),
        None => {
            let (name, bytes) = contents(o);
            Some(package(&name, &bytes))
        }
    }
}

/// The object's document converted to the other family of formats (to
/// ODF if `odf`, else to Office Open XML), as LibreOffice does when it
/// converts the document holding it; `None` if it stays as it is.
pub fn for_family(o: &OleObject, odf: bool) -> Option<OleObject> {
    // An ODF package in an OLE package (as it went to Office Open XML)
    // is a sub-document again.
    if odf && !is_odf(o) {
        let (_, data) = contents(o);
        if is_odf_data(&data) {
            let mime = crate::zip::ZipReader::new(&data).ok()?.read_string("mimetype").ok()?;
            return Some(OleObject { prog_id: odf_class(mime.trim()).into(), data: Arc::from(data), icon: o.icon, ..Default::default() });
        }
    }
    let (data, fmt) = openable(o)?;
    let (name, bytes) = match (fmt, load(&data, fmt).ok()?) {
        (Format::Xlsx, Loaded::Sheet(wb)) if odf => ("spreadsheet", crate::ods::write(&wb)),
        (Format::Docx, Loaded::Doc(d)) if odf => ("text", crate::odt::write(&d)),
        (Format::Ods, Loaded::Sheet(wb)) if !odf => ("Worksheet.xlsx", crate::xlsx::write(&wb)),
        (Format::Odt, Loaded::Doc(d)) if !odf => ("Document.docx", crate::docx::write(&d)),
        _ => return None,
    };
    let mut out = if odf { OleObject { prog_id: odf_class(name).into(), data: Arc::from(bytes), ..Default::default() } } else { from_file(name, bytes) };
    out.icon = o.icon;
    Some(out)
}

/// Whether the object is an ODF package (stored as a sub-document).
pub fn is_odf(o: &OleObject) -> bool {
    is_odf_data(&o.data)
}

fn is_odf_data(data: &[u8]) -> bool {
    matches!(package_ext(data), Some("ods" | "odt" | "odp" | "odg" | "odc" | "odf"))
}

// --- writing compound files -------------------------------------------------------

fn clsid_bytes(s: &str) -> [u8; 16] {
    let h: Vec<u8> = s.split('-').flat_map(|p| (0..p.len()).step_by(2).map(move |i| u8::from_str_radix(&p[i..i + 2], 16).unwrap_or(0))).collect();
    let mut b = [0u8; 16];
    b[..4].copy_from_slice(&[h[3], h[2], h[1], h[0]]);
    b[4..6].copy_from_slice(&[h[5], h[4]]);
    b[6..8].copy_from_slice(&[h[7], h[6]]);
    b[8..].copy_from_slice(&h[8..16]);
    b
}

/// Sets the CLSID of a compound file's root storage (in its bytes: the
/// first directory entry).
fn set_root_clsid(data: &mut [u8], clsid: &str) {
    let shift = u16::from_le_bytes([data[30], data[31]]) as u32;
    let first_dir = u32::from_le_bytes([data[48], data[49], data[50], data[51]]) as usize;
    let at = ((first_dir + 1) << shift) + 0x50;
    if let Some(slot) = data.get_mut(at..at + 16) {
        slot.copy_from_slice(&clsid_bytes(clsid));
    }
}

/// A `\u{1}CompObj` stream ([MS-OLEDS] 2.3.8).
fn compobj(clsid: &str, user_type: &str, prog_id: &str) -> Vec<u8> {
    let mut s = vec![1, 0, 0xFE, 0xFF, 3, 0x0A, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF];
    s.extend_from_slice(&clsid_bytes(clsid));
    for v in [user_type, "", prog_id] {
        if v.is_empty() {
            s.extend_from_slice(&0u32.to_le_bytes());
        } else {
            s.extend_from_slice(&(v.len() as u32 + 1).to_le_bytes());
            s.extend_from_slice(v.as_bytes());
            s.push(0);
        }
    }
    s.extend_from_slice(&0x71B2_39F4u32.to_le_bytes());
    s.extend_from_slice(&[0; 12]);
    s
}

/// The `\u{1}Ole` stream of an embedded object ([MS-OLEDS] 2.3.3).
const OLE_STREAM: [u8; 20] = [1, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

fn compound(clsid: &str, streams: &[(&str, &[u8])]) -> Vec<u8> {
    let mut c = cfb::CompoundFile::create(Cursor::new(Vec::new())).expect("memory");
    for (name, data) in streams {
        let mut s = c.create_stream(format!("/{name}")).expect("memory");
        s.write_all(data).expect("memory");
    }
    c.flush().expect("memory");
    let mut out = c.into_inner().into_inner();
    set_root_clsid(&mut out, clsid);
    out
}

/// An OLE package holding the file `name` ([MS-OLEDS] "Package" objects).
pub fn package(name: &str, data: &[u8]) -> Vec<u8> {
    let name = name.rsplit(['\\', '/']).next().unwrap_or(name);
    let ansi: String = name.chars().map(|c| if c.is_ascii() && c != '\0' { c } else { '_' }).collect();
    let temp = format!("C:\\Temp\\{ansi}");
    let mut body = Vec::new();
    body.extend_from_slice(&[2, 0]);
    for s in [&ansi, &temp] {
        body.extend_from_slice(s.as_bytes());
        body.push(0);
    }
    body.extend_from_slice(&[0, 0, 3, 0]);
    body.extend_from_slice(&(temp.len() as u32 + 1).to_le_bytes());
    body.extend_from_slice(temp.as_bytes());
    body.push(0);
    body.extend_from_slice(&(data.len() as u32).to_le_bytes());
    body.extend_from_slice(data);
    // The Unicode names that follow in newer packages.
    let wide = |s: &str, out: &mut Vec<u8>| {
        let u: Vec<u16> = s.encode_utf16().collect();
        out.extend_from_slice(&(u.len() as u32).to_le_bytes());
        for c in u {
            out.extend_from_slice(&c.to_le_bytes());
        }
    };
    let full = format!("C:\\Temp\\{name}");
    wide(&full, &mut body);
    wide(name, &mut body);
    wide(&full, &mut body);
    let mut native = (body.len() as u32).to_le_bytes().to_vec();
    native.extend_from_slice(&body);
    compound(CLSID_PACKAGE, &[("\u{1}Ole10Native", &native), ("\u{1}CompObj", &compobj(CLSID_PACKAGE, "OLE Package", "Package")), ("\u{1}Ole", &OLE_STREAM)])
}

/// A compound file holding an Office Open XML package in a `Package`
/// stream, as legacy Office files embed them.
pub fn wrap_package(data: &[u8], prog_id: &str) -> Vec<u8> {
    let x = OOXML.iter().find(|x| x.1 == prog_id).unwrap_or(&OOXML[0]);
    let user = match x.0 {
        "xlsx" | "xlsm" => "Microsoft Excel Worksheet",
        "docx" | "docm" => "Microsoft Word Document",
        _ => "Microsoft PowerPoint Presentation",
    };
    compound(x.3, &[("Package", data), ("\u{1}CompObj", &compobj(x.3, user, x.1)), ("\u{1}Ole", &OLE_STREAM)])
}

/// Office Open XML media type of an ODF sub-document's `mimetype`.
pub fn odf_class(mime: &str) -> &'static str {
    match mime {
        m if m.ends_with("spreadsheet") => "opendocument.CalcDocument.1",
        m if m.ends_with("text") => "opendocument.WriterDocument.1",
        m if m.ends_with("presentation") => "opendocument.ImpressDocument.1",
        m if m.ends_with("chart") => "opendocument.ChartDocument.1",
        m if m.ends_with("formula") => "opendocument.MathDocument.1",
        _ => "opendocument.DrawDocument.1",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packages_round_trip() {
        let p = package("notes é.txt", b"hello");
        let o = OleObject { prog_id: "Package".into(), data: Arc::from(p.clone()), ..Default::default() };
        assert_eq!(contents(&o), ("notes _.txt".to_string(), b"hello".to_vec()));
        assert_eq!(compobj_prog_id(&p).as_deref(), Some("Package"));
        let c = cfb::CompoundFile::open(Cursor::new(&p[..])).unwrap();
        assert_eq!(c.root_entry().clsid().as_bytes(), &[0, 3, 0, 12, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
        assert!(openable(&o).is_none());
    }

    #[test]
    fn office_files() {
        let wb = crate::xlsx::write(&apex_sheet::Workbook::new());
        let o = from_file("Book.xlsx", wb.clone());
        assert_eq!(o.prog_id, "Excel.Sheet.12");
        assert_eq!(contents(&o).0, "Worksheet.xlsx");
        assert!(matches!(openable(&o), Some((_, Format::Xlsx))));
        let (_, ext, ct, rel) = ooxml_part(&o);
        assert_eq!((ext.as_str(), rel), ("xlsx", REL_PACKAGE));
        assert!(ct.ends_with("spreadsheetml.sheet"));
        // In ODF it goes inside a compound file; that opens too.
        let ole = odf_ole(&o).unwrap();
        let w = OleObject { prog_id: "Excel.Sheet.12".into(), data: Arc::from(ole), ..Default::default() };
        assert_eq!(contents(&w).1, wb);
        assert_eq!(compobj_prog_id(&w.data).as_deref(), Some("Excel.Sheet.12"));
        // Edited, it stays wrapped.
        let d = crate::docx::write(&Document::new());
        let back = replaced(&w, wb.clone(), Format::Xlsx);
        assert!(is_cfb(&back.data));
        assert_eq!(contents(&back).1, wb);
        let pdf = from_file("a.pdf", b"%PDF-1.4 x".to_vec());
        assert_eq!(pdf.prog_id, "Package");
        assert_eq!(contents(&pdf).0, "a.pdf");
        let doc = from_file("x.docx", d);
        assert_eq!(doc.prog_id, "Word.Document.12");
        assert!(matches!(openable(&doc), Some((_, Format::Docx))));
    }
}
