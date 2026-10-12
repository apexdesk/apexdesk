//! File formats for ApexDesk: xlsx, ods, csv (spreadsheets) and docx, odt,
//! md, rtf, txt (documents).

/// Ranges all the conditional formats of a sheet may hold (each rule
/// copies its own).
pub(crate) const MAX_CF_RANGES: usize = 1 << 16;

#[cfg(feature = "empresa")]
pub mod bank;
pub mod chart_object;
pub mod crtx;
pub mod csv;
pub mod database;
pub mod font_check;
pub mod notebook_store;
pub mod note_crypt;
pub mod obsidian;
pub mod encoding;
pub mod glossary;
pub use encoding::Encoding;
pub mod docx;
pub mod fidelity;
pub mod html;
pub mod html_in;
pub mod cells_clip;
pub mod markdown;
pub mod mail_msg;
pub mod net;
pub mod ods;
pub mod odt;
pub mod pdf;
pub mod picture;
pub mod pptx;
pub mod odp;
pub mod ppt;
mod odp_anim;
mod odp_text;
mod odp_write;
mod pptx_anim;
mod pptx_fill;
mod pptx_shape;
mod pptx_text;
pub mod inspect;
pub mod probe;
pub mod records;
pub mod rtf;
pub mod xlsx;
mod xlsx_data;
mod xlsx_chart;
mod xlsx_drawing;
mod xlsx_notes;
mod xlsx_page;
mod xlsx_pieces;
mod xlsx_styles;
mod theme_xml;
mod docx_comments;
mod xlsx_table;
mod plan_xml;
mod xlsx_external;
mod xlsx_rich;
mod xlsx_threaded;
mod xlsx_views;
mod xlsx_incell;
mod docx_hf;
mod docx_biblio;
mod book_xml;
mod docx_fonts;
mod docx_controls;
mod docx_forms;
mod odt_controls;
mod odt_forms;
mod docx_fx;
mod docx_asian;
mod docx_defaults;
mod odf_fx;
mod docx_revisions;
mod odt_comments;
mod odt_revisions;
#[cfg(test)]
mod names_tests;
#[cfg(test)]
mod rich_tests;
#[cfg(test)]
mod review_tests;
pub mod crypto;
mod bin_crypto;
pub mod odf_crypto;
mod biff;
mod ole;
mod ole_repair;
pub mod xls;
mod xls_art;
mod xls_old;
mod xml2003;
mod xls_protect;
mod xls_chart;
mod xls_data;
mod xlsb_data;
pub mod xlsb;
pub mod vba;
mod macro_parts;
pub use macro_parts::Flavor;
pub mod doc;
mod props;
mod ooxml_keep;
pub mod docx_bind;
mod xlsx_keep;
mod xlsx_model;
mod whatif_values;
mod solver_names;
mod scenario_io;
mod pivot_xml;
mod xlsx_pivot;
mod xlsx_slicer;
mod xlsx_slicer_keep;
pub use xlsx_keep::{DataConnection, ExternalData, PowerQuery, external_data};
pub use xlsx_model::{ExcelModel, excel_model};
pub mod mathml;
mod omml;
#[cfg(test)]
mod shape_tests;
#[cfg(test)]
mod table_tools_tests;
#[cfg(test)]
mod protect_tests;
mod docx_img;
mod docx_ole;
mod odf_ole;
mod odf_flat;
mod xlsx_ole;
mod xlsx_controls;
pub mod embed;
pub mod safe_path;
mod diagram;
pub mod diagram_edit;
mod drawingml;
mod dml_fill;
mod docx_shape;
mod inkml;
mod docx_ink;
mod odf_ink;
#[cfg(test)]
mod ink_tests;
#[cfg(test)]
mod media_tests;
mod odf_shape;
mod odf_grad;
mod text_fx_xml;
mod docx_styles;
mod docx_layout;
mod odt_layout;
mod odt_asian;
mod odt_defaults;
mod odt_settings;
mod odt_fonts;
mod odt_img;
pub use odt_img::pixel_size;
mod docx_table;
mod odt_table;
pub mod xml;
pub mod zip;
pub mod queries_xml;
pub mod checks_xml;

use std::path::Path;

use apex_doc::Document;
use apex_sheet::Workbook;

#[derive(Debug, Clone)]
pub struct Error(pub String);

/// Messages of the errors that ask for a password.
const NEEDS_PASSWORD: &str = "the file is protected with a password";
const WRONG_PASSWORD: &str = "the password is not correct";
/// Message of the error for files protected with rights management.
const RIGHTS_MANAGED: &str = "the file is protected with Information Rights Management";

impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Error(msg.into())
    }

    /// The file is encrypted and no password was given.
    pub fn needs_password(&self) -> bool {
        self.0 == NEEDS_PASSWORD
    }

    pub fn wrong_password(&self) -> bool {
        self.0 == WRONG_PASSWORD
    }

    /// The file is encrypted by a sensitivity label (Azure RMS / IRM).
    /// A PDF without text (scanned pages): it needs text recognition.
    pub fn pdf_scanned(&self) -> bool {
        self.0 == pdf::SCANNED
    }

    pub fn rights_managed(&self) -> bool {
        self.0 == RIGHTS_MANAGED
    }
}

/// The error of an encrypted file opened without a password.
pub(crate) fn no_password() -> Error {
    Error::new(NEEDS_PASSWORD)
}

/// The error of a file that couldn't be decrypted.
pub(crate) fn crypto_error(e: crypto::CryptoError) -> Error {
    match e {
        crypto::CryptoError::WrongPassword => Error::new(WRONG_PASSWORD),
        crypto::CryptoError::Unsupported(m) => Error::new(format!("unsupported encryption: {m}")),
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error(e.to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Xlsx,
    Ods,
    Csv,
    Tsv,
    Docx,
    Odt,
    Markdown,
    Rtf,
    Text,
    /// Excel 97-2003 (read only).
    Xls,
    /// Excel binary workbook (read only).
    Xlsb,
    /// Word 97-2003 (read only).
    Doc,
    /// Flat OpenDocument spreadsheet: one XML file.
    Fods,
    /// Flat OpenDocument text: one XML file.
    Fodt,
    /// PDF (read only: imported as an editable document).
    Pdf,
    /// PowerPoint presentation (also .pptm, .potx, .ppsx).
    Pptx,
    /// OpenDocument presentation (also .otp).
    Odp,
    /// PowerPoint 97-2003 (read only; also .pps, .pot).
    Ppt,
    /// Access database (read only: a sheet per table; also .accdb).
    Access,
    /// dBase table with its memo file (read only).
    Dbase,
    /// E-mail message, MIME .eml or Outlook .msg (read only).
    Mail,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Sheet,
    Doc,
    Slides,
}

impl Format {
    pub fn from_path(p: &Path) -> Option<Format> {
        let ext = p.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "xlsx" | "xlsm" | "xltx" | "xltm" => Format::Xlsx,
            "ods" | "ots" => Format::Ods,
            "csv" => Format::Csv,
            "tsv" | "tab" => Format::Tsv,
            "docx" | "docm" | "dotx" | "dotm" => Format::Docx,
            "odt" | "ott" => Format::Odt,
            "md" | "markdown" => Format::Markdown,
            "rtf" => Format::Rtf,
            "txt" | "text" | "log" => Format::Text,
            // Kingsoft WPS saves .et/.ett as Excel 97 workbooks.
            "xls" | "xlt" | "et" | "ett" => Format::Xls,
            "xlsb" => Format::Xlsb,
            // Kingsoft WPS saves .wps/.wpt as Word 97 documents.
            "doc" | "dot" | "wps" | "wpt" => Format::Doc,
            "fods" => Format::Fods,
            "fodt" => Format::Fodt,
            "pdf" => Format::Pdf,
            "pptx" | "pptm" | "potx" | "potm" | "ppsx" | "ppsm" => Format::Pptx,
            "odp" | "otp" => Format::Odp,
            "ppt" | "pps" | "pot" => Format::Ppt,
            "mdb" | "accdb" | "mde" | "accde" => Format::Access,
            "dbf" => Format::Dbase,
            "eml" | "msg" => Format::Mail,
            // Text and source code: plain text, shown as code.
            e if CODE_EXTS.contains(&e) => Format::Text,
            _ => return None,
        })
    }

    pub fn kind(self) -> Kind {
        match self {
            Format::Xlsx | Format::Ods | Format::Csv | Format::Tsv | Format::Xls | Format::Xlsb | Format::Fods | Format::Access | Format::Dbase => Kind::Sheet,
            Format::Pptx | Format::Odp | Format::Ppt => Kind::Slides,
            _ => Kind::Doc,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Xlsx => "xlsx",
            Format::Ods => "ods",
            Format::Csv => "csv",
            Format::Tsv => "tsv",
            Format::Docx => "docx",
            Format::Odt => "odt",
            Format::Markdown => "md",
            Format::Rtf => "rtf",
            Format::Text => "txt",
            Format::Xls => "xls",
            Format::Xlsb => "xlsb",
            Format::Doc => "doc",
            Format::Fods => "fods",
            Format::Fodt => "fodt",
            Format::Pdf => "pdf",
            Format::Pptx => "pptx",
            Format::Odp => "odp",
            Format::Ppt => "ppt",
            Format::Access => "accdb",
            Format::Dbase => "dbf",
            Format::Mail => "eml",
        }
    }

    /// Formats ApexDesk can write (the legacy binary ones are only read:
    /// saving them asks for another format).
    pub fn can_save(self) -> bool {
        !matches!(self, Format::Xls | Format::Xlsb | Format::Doc | Format::Pdf | Format::Ppt | Format::Access | Format::Dbase | Format::Mail)
    }

    /// Formats that keep all features of the model (no data loss warning).
    pub fn is_rich(self) -> bool {
        matches!(self, Format::Xlsx | Format::Ods | Format::Docx | Format::Odt | Format::Fods | Format::Fodt | Format::Pptx | Format::Odp)
    }
}

pub const SHEET_EXTS: &[&str] = &["xlsx", "xlsm", "xltx", "xltm", "ods", "ots", "csv", "tsv", "xls", "xlt", "et", "ett", "xlsb", "fods"];
pub const DOC_EXTS: &[&str] = &["docx", "docm", "dotx", "dotm", "odt", "ott", "md", "markdown", "rtf", "txt", "doc", "dot", "wps", "wpt", "fodt", "pdf", "eml", "msg"];
/// Text and source code files opened as plain text documents, shown as
/// code (CSV stays a workbook).
pub const CODE_EXTS: &[&str] = &[
    "json", "xml", "yaml", "yml", "toml", "ini", "cfg", "conf", "properties", "env", "html", "htm", "css", "scss", "js", "mjs", "cjs", "ts", "jsx", "tsx", "vue", "rs", "py", "c", "h", "cpp",
    "cc", "cxx", "hpp", "hh", "cs", "java", "kt", "kts", "go", "rb", "php", "lua", "pl", "swift", "sh", "bash", "zsh", "fish", "ps1", "bat", "cmd", "sql", "r", "diff", "patch", "gradle", "cmake",
];

/// Whether the file is text or source code by its extension (.rs, .json,
/// .xml...): it opens as code.
pub fn is_code_path(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| CODE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Databases opened as workbooks.
pub const DB_EXTS: &[&str] = &["mdb", "accdb", "mde", "accde", "dbf"];
pub const SLIDES_EXTS: &[&str] = &["pptx", "pptm", "potx", "potm", "ppsx", "ppsm", "odp", "otp", "ppt", "pps", "pot"];

/// Whether the file is a template (.xltx, .ots, .dotx...): it opens as a
/// new untitled file.
pub fn is_template(p: &Path) -> bool {
    let ext = p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    matches!(ext.as_str(), "xltx" | "xltm" | "ots" | "dotx" | "dotm" | "ott" | "xlt" | "dot" | "potx" | "potm" | "otp" | "pot")
}

pub enum Loaded {
    Sheet(Box<Workbook>),
    Doc(Box<Document>),
    Slides(Box<apex_slides::Deck>),
}

/// Loads any supported file, detecting the format by extension and content.
pub fn open(path: &Path) -> Result<(Loaded, Format), Error> {
    open_with(path, None)
}

/// Loads a file that may be encrypted with `password` (an encrypted file
/// without one gives an error that [`Error::needs_password`]).
pub fn open_with(path: &Path, password: Option<&str>) -> Result<(Loaded, Format), Error> {
    // A reader that trips over a damaged file fails the opening, not the app.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| open_unguarded(path, password))).unwrap_or_else(|_| Err(Error::new("the file is damaged and could not be read")))
}

fn open_unguarded(path: &Path, password: Option<&str>) -> Result<(Loaded, Format), Error> {
    // A big CSV file is read a piece at a time (unless it is really a
    // package or an encrypted file with that extension).
    if let Some(fmt @ (Format::Csv | Format::Tsv)) = Format::from_path(path) {
        use std::io::Read;
        let mut head = [0u8; 4];
        let big = std::fs::metadata(path)?.len() > 16 << 20;
        if big && std::fs::File::open(path)?.read_exact(&mut head).is_ok() && head != *b"PK\x03\x04" && head != [0xD0, 0xCF, 0x11, 0xE0] {
            if let Some(wb) = csv::read_file(path, (fmt == Format::Tsv).then_some(b'\t'))? {
                return Ok((Loaded::Sheet(Box::new(wb)), fmt));
            }
        }
    }
    // Databases read by path: a dBase table's memo file is next to it.
    if let Some(fmt @ (Format::Access | Format::Dbase)) = Format::from_path(path) {
        return Ok((Loaded::Sheet(Box::new(database::read_path(path)?)), fmt));
    }
    let mut data = std::fs::read(path)?;
    if crypto::is_rights_managed(&data) {
        return Err(Error::new(RIGHTS_MANAGED));
    }
    if crypto::is_encrypted(&data) {
        let Some(pw) = password else { return Err(Error::new(NEEDS_PASSWORD)) };
        data = crypto::decrypt(&data, pw).map_err(crypto_error)?;
    } else if odf_crypto::is_encrypted(&data) {
        let Some(pw) = password else { return Err(no_password()) };
        data = odf_crypto::decrypt(&data, pw).map_err(crypto_error)?;
    }
    let fmt = Format::from_path(path).or_else(|| sniff(&data)).ok_or_else(|| Error::new("unrecognized file format"))?;
    let fmt = match (fmt, sniff(&data)) {
        // A code extension on an office file (a flat .xml document).
        (Format::Text, Some(s)) if s.is_rich() && is_code_path(path) => s,
        // Content wins over a misleading extension for zip packages, and
        // for legacy extensions (a .doc that is RTF, an .xls that is xlsx).
        (f, Some(s)) if (f.is_rich() && s.is_rich()) || !f.can_save() => s,
        // A package extension on a file that isn't one (a .docm that is a
        // Word 97 .doc): Office opens it by its content too.
        (f, Some(s)) if f.is_rich() && !data.starts_with(b"PK") => s,
        (f, _) => f,
    };
    let loaded = match fmt {
        Format::Xlsx => Loaded::Sheet(Box::new(xlsx::read(&data)?)),
        Format::Ods => Loaded::Sheet(Box::new(ods::read(&data)?)),
        Format::Csv => Loaded::Sheet(Box::new(csv::read(&data, None))),
        Format::Tsv => Loaded::Sheet(Box::new(csv::read(&data, Some(b'\t')))),
        Format::Docx => Loaded::Doc(Box::new(docx::read(&data)?)),
        Format::Odt => Loaded::Doc(Box::new(odt::read(&data)?)),
        Format::Markdown => Loaded::Doc(Box::new(markdown::read(&decode_text(&data)))),
        Format::Rtf => Loaded::Doc(Box::new(rtf::read(&data)?)),
        Format::Text => Loaded::Doc(Box::new(Document::from_plain_text(&decode_text(&data)))),
        Format::Xls => Loaded::Sheet(Box::new(xls::read_with(&data, password)?)),
        Format::Xlsb => Loaded::Sheet(Box::new(xlsb::read(&data)?)),
        Format::Doc => Loaded::Doc(Box::new(doc::read_with(&data, password)?)),
        Format::Fods => Loaded::Sheet(Box::new(ods::read(&odf_flat::to_package(&data)?)?)),
        Format::Fodt => Loaded::Doc(Box::new(odt::read(&odf_flat::to_package(&data)?)?)),
        Format::Pdf => Loaded::Doc(Box::new(pdf::read(&data, password)?)),
        Format::Pptx => Loaded::Slides(Box::new(pptx::read(&data)?)),
        Format::Odp => Loaded::Slides(Box::new(odp::read(&data)?)),
        Format::Ppt => Loaded::Slides(Box::new(ppt::read(&data)?)),
        Format::Access => Loaded::Sheet(Box::new(database::read_access(&data)?)),
        Format::Dbase => Loaded::Sheet(Box::new(database::read_dbase(&data)?)),
        Format::Mail => Loaded::Doc(Box::new(mail_msg::read_doc(&data)?)),
    };
    Ok((capped(loaded), fmt))
}

/// The format of an OLE compound file (legacy .doc/.xls).
fn sniff_ole(data: &[u8]) -> Option<Format> {
    let c = cfb::CompoundFile::open(std::io::Cursor::new(data)).ok()?;
    if c.is_stream("/WordDocument") {
        Some(Format::Doc)
    } else if c.is_stream("/Workbook") || c.is_stream("/Book") {
        Some(Format::Xls)
    } else if c.is_stream("/PowerPoint Document") {
        Some(Format::Ppt)
    } else if c.is_stream("/__properties_version1.0") {
        Some(Format::Mail)
    } else {
        None
    }
}

fn sniff(data: &[u8]) -> Option<Format> {
    if data.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        return sniff_ole(data);
    }
    if data.starts_with(b"{\\rtf") {
        return Some(Format::Rtf);
    }
    if memchr::memmem::find(&data[..data.len().min(1024)], b"%PDF-").is_some() {
        return Some(Format::Pdf);
    }
    if !data.starts_with(b"PK") {
        return match odf_flat::sniff(data)? {
            m if m.contains("spreadsheet") => Some(Format::Fods),
            m if m.contains("text") => Some(Format::Fodt),
            _ => None,
        };
    }
    let z = zip::ZipReader::new(data).ok()?;
    if z.contains("xl/workbook.xml") {
        return Some(Format::Xlsx);
    }
    if z.contains("xl/workbook.bin") {
        return Some(Format::Xlsb);
    }
    if z.contains("word/document.xml") {
        return Some(Format::Docx);
    }
    if z.contains("ppt/presentation.xml") {
        return Some(Format::Pptx);
    }
    let mime = z.read_string("mimetype").ok()?;
    if mime.contains("spreadsheet") {
        Some(Format::Ods)
    } else if mime.contains("presentation") {
        Some(Format::Odp)
    } else if mime.contains("text") {
        Some(Format::Odt)
    } else {
        None
    }
}

pub fn save_sheet(wb: &Workbook, path: &Path, fmt: Format) -> Result<(), Error> {
    save_sheet_with(wb, path, fmt, None)
}

/// Saves a workbook; with a password, xlsx and ods files are encrypted.
pub fn save_sheet_with(wb: &Workbook, path: &Path, fmt: Format, password: Option<&str>) -> Result<(), Error> {
    let bytes = match fmt {
        Format::Xlsx => xlsx::write_as(wb, Flavor::for_path(path, &wb.macros)),
        Format::Ods => ods::write(wb),
        Format::Csv => csv::write(wb, 0, None).into_bytes(),
        Format::Tsv => csv::write(wb, 0, Some('\t')).into_bytes(),
        Format::Fods => odf_flat::from_package(&ods::write(wb))?,
        _ => return Err(Error::new("format not valid for a spreadsheet")),
    };
    write_atomic(path, &encrypted(bytes, fmt, password))
}

pub fn save_doc(doc: &Document, path: &Path, fmt: Format) -> Result<(), Error> {
    save_doc_with(doc, path, fmt, None)
}

/// Saves a document; with a password, docx and odt files are encrypted.
pub fn save_doc_with(doc: &Document, path: &Path, fmt: Format, password: Option<&str>) -> Result<(), Error> {
    let bytes = match fmt {
        Format::Docx => docx::write_as(doc, Flavor::for_path(path, &doc.macros)),
        Format::Odt => odt::write(doc),
        // Formats that keep no tracked changes (nor, but RTF, hidden
        // parts) get what shows.
        Format::Markdown => markdown::write(&doc.shared()).into_bytes(),
        Format::Rtf => rtf::write(&doc.resolved(true)),
        Format::Text => doc.shared().to_plain_text().into_bytes(),
        Format::Fodt => odf_flat::from_package(&odt::write(doc))?,
        _ => return Err(Error::new("format not valid for a document")),
    };
    write_atomic(path, &encrypted(bytes, fmt, password))
}

/// Whether a format is plain text, read and written in an encoding.
pub fn is_text_format(fmt: Format) -> bool {
    matches!(fmt, Format::Csv | Format::Tsv | Format::Text | Format::Markdown)
}

/// The contents of a text file (csv, tsv, txt, md) read in `enc`, and
/// for CSV split at `delim` (None: detected).
pub fn text_contents(data: &[u8], fmt: Format, enc: Encoding, delim: Option<u8>) -> Result<Loaded, Error> {
    let text = enc.decode(data);
    Ok(match fmt {
        Format::Csv | Format::Tsv => Loaded::Sheet(Box::new(csv::read(text.as_bytes(), delim.or((fmt == Format::Tsv).then_some(b'\t'))))),
        Format::Markdown => Loaded::Doc(Box::new(markdown::read(&text))),
        Format::Text => Loaded::Doc(Box::new(Document::from_plain_text(&text))),
        _ => return Err(Error::new("not a text format")),
    })
    .map(capped)
}

/// A document keeps at most `MAX_PARAGRAPHS` paragraphs (it says when it
/// left some out): a file of millions of lines would take gigabytes.
fn capped(mut loaded: Loaded) -> Loaded {
    if let Loaded::Doc(d) = &mut loaded {
        d.cap_paragraphs();
    }
    loaded
}

/// Opens a text file in encoding `enc` (None: the one detected); returns
/// the encoding used too.
pub fn open_text(path: &Path, fmt: Format, enc: Option<Encoding>, delim: Option<u8>) -> Result<(Loaded, Encoding), Error> {
    let data = std::fs::read(path)?;
    let enc = enc.unwrap_or_else(|| encoding::detect(&data));
    Ok((text_contents(&data, fmt, enc, delim)?, enc))
}

/// The encoding of a text file, from its first bytes.
pub fn detect_file_encoding(path: &Path) -> std::io::Result<Encoding> {
    use std::io::Read;
    let mut head = Vec::new();
    std::fs::File::open(path)?.take(1 << 16).read_to_end(&mut head)?;
    Ok(encoding::detect(&head))
}

/// Saves the first sheet as CSV or TSV (or the text a document) in `enc`;
/// returns how many characters the encoding had no room for (saved as
/// `?`).
pub fn save_text(loaded: TextSource<'_>, path: &Path, fmt: Format, enc: Encoding) -> Result<usize, Error> {
    let text = match (loaded, fmt) {
        (TextSource::Sheet(wb), Format::Csv) => csv::write(wb, 0, None),
        (TextSource::Sheet(wb), Format::Tsv) => csv::write(wb, 0, Some('\t')),
        (TextSource::Doc(doc), Format::Text) => doc.shared().to_plain_text(),
        (TextSource::Doc(doc), Format::Markdown) => markdown::write(&doc.shared()),
        (TextSource::Raw(text), Format::Markdown | Format::Text) => text.to_string(),
        _ => return Err(Error::new("not a text format")),
    };
    let (bytes, lost) = enc.encode(&text);
    write_atomic(path, &bytes)?;
    Ok(lost)
}

/// What [`save_text`] writes.
#[derive(Clone, Copy)]
pub enum TextSource<'a> {
    Sheet(&'a Workbook),
    Doc(&'a Document),
    /// Text written as it is (a Markdown source edited by hand).
    Raw(&'a str),
}

/// Saves a presentation as pptx or odp.
pub fn save_slides(deck: &apex_slides::Deck, path: &Path, fmt: Format) -> Result<(), Error> {
    save_slides_with(deck, path, fmt, None)
}

/// Saves a presentation; with a password, pptx files are encrypted.
pub fn save_slides_with(deck: &apex_slides::Deck, path: &Path, fmt: Format, password: Option<&str>) -> Result<(), Error> {
    let bytes = match fmt {
        Format::Pptx => pptx::write_as(deck, pptx::PptFlavor::for_path(path, &deck.macros)),
        Format::Odp => odp_write::write(deck),
        _ => return Err(Error::new("format not valid for a presentation")),
    };
    write_atomic(path, &encrypted(bytes, fmt, password))
}

/// Whether the file is encrypted with a password (any format).
pub fn is_encrypted(data: &[u8]) -> bool {
    crypto::is_encrypted(data) || odf_crypto::is_encrypted(data) || bin_crypto::is_encrypted(data)
}

/// Whether files of this format can be saved with a password.
pub fn can_encrypt(fmt: Format) -> bool {
    matches!(fmt, Format::Xlsx | Format::Docx | Format::Ods | Format::Odt | Format::Pptx | Format::Odp)
}

fn encrypted(bytes: Vec<u8>, fmt: Format, password: Option<&str>) -> Vec<u8> {
    match password.filter(|p| !p.is_empty()) {
        Some(pw) if matches!(fmt, Format::Ods | Format::Odt | Format::Odp) => odf_crypto::encrypt(&bytes, pw),
        Some(pw) if can_encrypt(fmt) => crypto::encrypt(&bytes, pw),
        _ => bytes,
    }
}

/// Writes to a temporary file and renames it, so a crash never leaves a
/// half-written document behind. The temporary name can't be guessed and
/// is created new (a link planted there is not followed); the document
/// keeps its permissions, and saving through the user's own link changes
/// the file it points at, not the link. A link someone else made (in a
/// shared folder, put in the document's place) is replaced, not followed.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    use std::io::Write;
    let path = own_target(path);
    // A link left in place is someone else's: its target's permissions
    // are not the document's.
    // A file someone else owns keeps at most what it allows (they may have
    // made it writable by all, but the new file must not let more people
    // read it), and no special bits are kept.
    let kept = std::fs::symlink_metadata(&path).ok().filter(|m| !m.file_type().is_symlink());
    // Its group too: the new file's would be the user's own, maybe one
    // everybody is in.
    #[cfg(unix)]
    let group = kept.as_ref().map(std::os::unix::fs::MetadataExt::gid);
    let perms = kept.map(|m| {
        let mut p = m.permissions();
        #[cfg(unix)]
        {
            let mode = std::os::unix::fs::PermissionsExt::mode(&p) & if owned(&m) { 0o777 } else { 0o666 };
            std::os::unix::fs::PermissionsExt::set_mode(&mut p, mode);
        }
        p
    });
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = path.file_name().map_or_else(|| "file".into(), |n| n.to_string_lossy().into_owned());
    let mut made = None;
    for _ in 0..8 {
        let tmp = dir.join(format!(".{name}.{:016x}.tmp~", safe_path::random_token()));
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        // Never readable by more people than the document is, even for a
        // moment.
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut o, perms.as_ref().map_or(0o666, |p| std::os::unix::fs::PermissionsExt::mode(p) & 0o777));
        match o.open(&tmp) {
            Ok(f) => {
                made = Some((tmp, f));
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            // A folder we may not create files in: write the document itself.
            Err(_) => break,
        }
    }
    let Some((tmp, mut f)) = made else {
        write_no_follow(&path, bytes)?;
        return Ok(());
    };
    let written = f.write_all(bytes).and_then(|()| {
        if let Some(p) = perms {
            // Where the group can't be kept, it reads nothing.
            #[cfg(unix)]
            let p = {
                let mut p = p;
                if std::os::unix::fs::fchown(&f, None, group).is_err() {
                    let mode = std::os::unix::fs::PermissionsExt::mode(&p) & !0o070;
                    std::os::unix::fs::PermissionsExt::set_mode(&mut p, mode);
                }
                p
            };
            f.set_permissions(p)?;
        }
        f.sync_all()
    });
    drop(f);
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    std::fs::rename(&tmp, &path).or_else(|_| {
        let _ = std::fs::remove_file(&tmp);
        write_no_follow(&path, bytes)
    })?;
    // The rename itself reaches the disk.
    #[cfg(unix)]
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// The file `path` stands for: through the links the user made, up to
/// the first one someone else did.
fn own_target(path: &Path) -> std::path::PathBuf {
    let mut p = path.to_path_buf();
    for _ in 0..40 {
        let Ok(m) = std::fs::symlink_metadata(&p) else { break };
        if !m.file_type().is_symlink() {
            break;
        }
        #[cfg(unix)]
        if std::os::unix::fs::MetadataExt::uid(&m) != safe_path::current_uid() {
            break;
        }
        let Ok(t) = std::fs::read_link(&p) else { break };
        p = match p.parent() {
            Some(d) if t.is_relative() => d.join(t),
            _ => t,
        };
    }
    p
}

/// Writes `bytes` over the file at `path`, never through a link there nor
/// into a file of someone else's.
pub fn write_no_follow(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(&mut o, libc::O_NOFOLLOW);
    #[cfg(not(unix))]
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(std::io::Error::other("the file is a link"));
    }
    let mut f = o.open(path)?;
    // Checked on the file opened, before anything in it changes.
    if !owned(&f.metadata()?) {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "the file belongs to another user"));
    }
    f.set_len(0)?;
    f.write_all(bytes)
}

/// Whether the file is this user's (always so off Unix).
fn owned(m: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    return std::os::unix::fs::MetadataExt::uid(m) == safe_path::current_uid();
    #[cfg(not(unix))]
    {
        let _ = m;
        true
    }
}

/// Decodes text files in the encoding detected (see [`encoding::detect`]):
/// UTF-8 (with or without BOM), UTF-16, or else Windows-1252, DOS 850 or
/// ISO-8859-1/15.
pub fn decode_text(data: &[u8]) -> String {
    encoding::detect(data).decode(data)
}

fn cp1252(b: u8) -> char {
    const HI: [u16; 32] = [
        0x20AC, 0x81, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x8D, 0x017D, 0x8F, 0x90, 0x2018,
        0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x9D, 0x017E, 0x0178,
    ];
    if (0x80..0xA0).contains(&b) { char::from_u32(HI[(b - 0x80) as usize] as u32).unwrap_or('?') } else { b as char }
}

/// Parses `#RRGGBB`, `RRGGBB` or `AARRGGBB` into 0xRRGGBB.
pub(crate) fn parse_hex_color(s: &str) -> Option<u32> {
    let s = s.trim().trim_start_matches('#');
    match s.len() {
        6 => u32::from_str_radix(s, 16).ok(),
        8 => u32::from_str_radix(&s[2..], 16).ok(),
        _ => None,
    }
}

/// Parses an ODF length ("2.5cm", "12pt", "1in", "20mm") into points.
pub(crate) fn parse_length_pt(s: &str) -> Option<f32> {
    let s = s.trim();
    let idx = s.find(|c: char| c.is_ascii_alphabetic() || c == '%')?;
    let v: f32 = s[..idx].trim().parse().ok()?;
    Some(match &s[idx..] {
        "pt" => v,
        "cm" => v * 72.0 / 2.54,
        "mm" => v * 72.0 / 25.4,
        "in" | "inch" => v * 72.0,
        "pc" => v * 12.0,
        "px" => v * 0.75,
        _ => return None,
    })
}

/// Whether a font family name is a monospace font (used to map to `code`).
/// Whether a numbered paragraph at `level` appended after `out` would
/// continue an existing list (see `Document::list_number`).
pub(crate) fn list_continues(out: &[apex_doc::Paragraph], level: u8) -> bool {
    for p in out.iter().rev() {
        let s = p.style;
        if s.kind == apex_doc::ParaKind::Numbered && s.level == level {
            return true;
        }
        if !(s.kind.is_list() && s.level > level) {
            return false;
        }
    }
    false
}

pub(crate) fn is_mono_font(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["mono", "courier", "consolas", "menlo", "monaco", "source code", "fira code", "inconsolata", "lucida console"].iter().any(|m| n.contains(m))
}

/// Named highlight colors used by Word.
pub(crate) fn highlight_color(name: &str) -> Option<u32> {
    Some(match name {
        "yellow" => 0xFFFF00,
        "green" => 0x00FF00,
        "cyan" => 0x00FFFF,
        "magenta" => 0xFF00FF,
        "blue" => 0x0000FF,
        "red" => 0xFF0000,
        "darkBlue" => 0x000080,
        "darkCyan" => 0x008080,
        "darkGreen" => 0x008000,
        "darkMagenta" => 0x800080,
        "darkRed" => 0x800000,
        "darkYellow" => 0x808000,
        "darkGray" => 0x808080,
        "lightGray" => 0xC0C0C0,
        "black" => 0x000000,
        "white" => 0xFFFFFF,
        _ => return None,
    })
}

#[cfg(test)]
mod format_tests {
    use super::*;

    #[test]
    fn templates_open_as_their_format() {
        let dir = std::env::temp_dir().join(format!("apex_tpl_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut wb = Workbook::new();
        wb.set_input(0, apex_sheet::CellRef::new(0, 0), "7");
        let p = dir.join("t.xltx");
        std::fs::write(&p, xlsx::write(&wb)).unwrap();
        assert!(is_template(&p) && !is_template(Path::new("a.xlsx")));
        let (l, f) = open(&p).unwrap();
        assert_eq!(f, Format::Xlsx);
        assert!(matches!(l, Loaded::Sheet(w) if w.sheets[0].value(apex_sheet::CellRef::new(0, 0)) == &apex_sheet::Value::Number(7.0)));
        assert_eq!(Format::from_path(Path::new("x.OTT")), Some(Format::Odt));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn code_files_open_as_text() {
        let dir = std::env::temp_dir().join(format!("apex_code_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(Format::from_path(Path::new("main.RS")), Some(Format::Text));
        assert!(is_code_path(Path::new("a.json")) && !is_code_path(Path::new("a.txt")) && !is_code_path(Path::new("a.csv")));
        assert_eq!(Format::from_path(Path::new("a.csv")), Some(Format::Csv));
        let p = dir.join("a.json");
        std::fs::write(&p, "{\n  \"a\": 1\n}\n").unwrap();
        let (l, f) = open(&p).unwrap();
        assert_eq!(f, Format::Text);
        assert!(matches!(l, Loaded::Doc(d) if d.paragraphs[1].text == "  \"a\": 1"));
        // A flat OpenDocument file named .xml still opens as what it is.
        let x = dir.join("a.xml");
        std::fs::write(&x, odf_flat::from_package(&odt::write(&Document::from_plain_text("hola"))).unwrap()).unwrap();
        assert_eq!(open(&x).unwrap().1, Format::Fodt);
        std::fs::remove_dir_all(&dir).ok();
    }
}
