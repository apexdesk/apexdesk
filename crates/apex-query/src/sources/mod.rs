//! The sources a query reads: files and URLs (text, JSON, XML, HTML),
//! workbooks, folders, databases, and the workbook's own tables.

pub mod book;
pub mod db;
pub mod html;
pub mod json;
pub mod pdf;
pub mod sqlserver;
pub mod text;
pub mod web;
pub mod xml;

use apex_sheet::query::{Source, is_url};

use crate::{Context, DataTable, Error};

/// Largest file or download read (bytes).
pub const MAX_BYTES: u64 = 512 * 1024 * 1024;

/// What a database query may return: a sheet's rows, [`MAX_BYTES`] and a
/// couple of minutes (a query or a server can give rows forever).
#[allow(dead_code)]
pub(crate) struct Budget {
    rows: usize,
    bytes: u64,
    end: std::time::Instant,
}

#[allow(dead_code)]
impl Budget {
    pub(crate) fn new() -> Self {
        Budget { rows: 0, bytes: 0, end: std::time::Instant::now() + std::time::Duration::from_secs(120) }
    }

    /// Counts one more row.
    pub(crate) fn take(&mut self, row: &[crate::table::Val]) -> Result<(), Error> {
        self.rows += 1;
        self.bytes += row.iter().map(|v| 16 + if let crate::table::Val::Text(s) = v { s.len() as u64 } else { 0 }).sum::<u64>();
        if self.rows > 1 << 20 || self.bytes > MAX_BYTES || std::time::Instant::now() > self.end {
            return Err(Error::Database("the query returns too much data or takes too long".into()));
        }
        Ok(())
    }
}

/// The bytes at a location: a URL (downloaded) or a path.
pub fn read_bytes(location: &str, ctx: &Context) -> Result<Vec<u8>, Error> {
    if is_url(location) {
        return web::get(location.trim());
    }
    let p = ctx.path(location);
    let meta = std::fs::metadata(&p).map_err(|e| Error::Io(format!("{}: {e}", p.display())))?;
    // Devices and pipes say nothing of their length, and may never end.
    if !meta.is_file() {
        return Err(Error::Io(format!("{}: not a file", p.display())));
    }
    if meta.len() > MAX_BYTES {
        return Err(Error::Io(format!("{}: too big", p.display())));
    }
    std::fs::read(&p).map_err(|e| Error::Io(format!("{}: {e}", p.display())))
}

/// Text in UTF-8/16 (by its BOM) or Windows-1252, or in `encoding` (a
/// WHATWG label: "utf-8", "latin1", "shift_jis"…) when given.
pub fn decode(data: &[u8], encoding: &str) -> Result<String, Error> {
    let label = encoding.trim();
    if label.is_empty() {
        return Ok(apex_io::decode_text(data));
    }
    // The office ones (DOS 850 among them, which WHATWG lacks).
    if let Some(enc) = apex_io::Encoding::from_key(label) {
        return Ok(enc.decode(data));
    }
    let enc = encoding_rs::Encoding::for_label(label.as_bytes()).ok_or_else(|| Error::Invalid(format!("unknown encoding {label}")))?;
    let (text, _, _) = enc.decode(data);
    Ok(text.into_owned())
}

pub fn load(src: &Source, ctx: &Context) -> Result<DataTable, Error> {
    if crate::disabled() && !matches!(src, Source::Table { .. } | Source::Query { .. }) {
        return Err(Error::Disabled);
    }
    match src {

        Source::Text { location, delimiter, encoding, header, skip } => {
            let data = read_bytes(location, ctx)?;
            let text = decode(&data, encoding)?;
            Ok(text::parse(&text, *delimiter, *header, *skip as usize))
        }
        Source::Workbook { path, item, header } => book::file_table(&ctx.path(path), item, *header),
        Source::Folder { path, extension, header, file_column } => book::folder(&ctx.path(path), extension, *header, *file_column),
        Source::Json { location, path } => {
            let data = read_bytes(location, ctx)?;
            json::table(&decode(&data, "")?, path)
        }
        Source::Xml { location, element } => {
            let data = read_bytes(location, ctx)?;
            xml::table(&decode_xml(&data), element)
        }
        Source::Html { location, index } => {
            let data = read_bytes(location, ctx)?;
            let tables = html::tables(&decode(&data, "")?);
            let n = tables.len();
            tables.into_iter().nth(*index).ok_or_else(|| Error::Parse(format!("the page has {n} tables")))
        }
        Source::Pdf { location, index, header } => {
            let tables = pdf::tables(&read_bytes(location, ctx)?, *header)?;
            let n = tables.len();
            tables.into_iter().nth(*index).ok_or_else(|| Error::Parse(format!("the PDF has {n} tables")))
        }
        Source::Sqlite { path, sql } => db::sqlite(&ctx.path(path), sql),
        Source::DbFile { path, table } => db::db_file(&ctx.path(path), table),
        Source::Database { .. } => {
            let key = crate::password_key(src).unwrap_or_default();
            let pw = ctx.passwords.get(&key).ok_or(Error::NeedsPassword(key))?;
            db::server(src, pw)
        }
        Source::Table { name } => ctx.tables.get(&name.to_lowercase()).cloned().ok_or_else(|| Error::NoTable(name.clone())),
        Source::Query { name } => ctx.result_of(name),
    }
}

/// XML text: UTF-16 when it starts so, else as text files are read.
fn decode_xml(data: &[u8]) -> String {
    if data.len() >= 2 && data[0] == b'<' && data[1] == 0 {
        let units: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        return String::from_utf16_lossy(&units);
    }
    apex_io::decode_text(data)
}
