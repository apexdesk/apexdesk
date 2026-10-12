//! Table definitions: the pages (type 2) that list a table's columns, its
//! indexes and where its rows are. Several pages are chained by their
//! "next page" field; after the first, each adds its bytes after the page's
//! 8-byte header.

use crate::model::{ColType, Column, Index};
use crate::Error;

use super::AccessFile;

// Column types as stored.
pub const T_BOOL: u8 = 0x01;
pub const T_BYTE: u8 = 0x02;
pub const T_INT: u8 = 0x03;
pub const T_LONG: u8 = 0x04;
pub const T_MONEY: u8 = 0x05;
pub const T_FLOAT: u8 = 0x06;
pub const T_DOUBLE: u8 = 0x07;
pub const T_DATETIME: u8 = 0x08;
pub const T_BINARY: u8 = 0x09;
pub const T_TEXT: u8 = 0x0a;
pub const T_OLE: u8 = 0x0b;
pub const T_MEMO: u8 = 0x0c;
pub const T_GUID: u8 = 0x0f;
pub const T_NUMERIC: u8 = 0x10;
pub const T_COMPLEX: u8 = 0x12;
pub const T_BIGINT: u8 = 0x13;
pub const T_EXT_DATETIME: u8 = 0x14;

/// A column as stored: its model and where its data is in a row.
#[derive(Clone, Debug)]
pub struct ColDef {
    pub col: Column,
    pub raw_type: u8,
    /// Its number in rows (counts deleted columns too).
    pub num: u16,
    /// Its place among variable-length columns.
    pub var_index: u16,
    /// Its place in the fixed-length part of rows.
    pub fixed_offset: u16,
    pub len: u16,
    pub fixed: bool,
    /// Access 2010 calculated columns store the value with a header.
    pub calculated: bool,
    /// The type of the values: the stored one, or for calculated columns
    /// the type of their result (a property).
    pub result: u8,
}

#[derive(Clone, Debug)]
pub struct TableDef {
    pub name: String,
    /// Its first definition page (the table's id in the catalog).
    pub page: u32,
    pub num_rows: u32,
    /// Columns in their order of definition.
    pub columns: Vec<ColDef>,
    pub indexes: Vec<Index>,
    /// Row pointer (row number and page) of the map of its data pages.
    pub usage_map: u32,
}

/// Reads little-endian numbers without panicking past the end.
pub(super) struct Rd<'a>(pub &'a [u8]);

impl Rd<'_> {
    pub fn u8(&self, o: usize) -> u8 {
        self.0.get(o).copied().unwrap_or(0)
    }
    pub fn u16(&self, o: usize) -> u16 {
        u16::from_le_bytes([self.u8(o), self.u8(o + 1)])
    }
    pub fn u32(&self, o: usize) -> u32 {
        u32::from_le_bytes([self.u8(o), self.u8(o + 1), self.u8(o + 2), self.u8(o + 3)])
    }
    pub fn bytes(&self, a: usize, n: usize) -> &[u8] {
        self.0.get(a..a.saturating_add(n)).unwrap_or(&[])
    }
}

/// Text of names: UCS-2 in Jet 4 and later, the code page in Jet 3.
fn name_text(f: &AccessFile, b: &[u8]) -> String {
    if f.jet.is_jet3() {
        crate::codepage::decode(b, f.codepage)
    } else {
        super::rows::text(b, f)
    }
}

/// The model's type of a stored type.
pub fn col_type(raw: u8) -> ColType {
    match raw {
        T_BOOL => ColType::Bool,
        T_BYTE => ColType::Byte,
        T_INT => ColType::Int16,
        T_LONG => ColType::Int32,
        T_MONEY => ColType::Money,
        T_FLOAT => ColType::Single,
        T_DOUBLE => ColType::Double,
        T_DATETIME | T_EXT_DATETIME => ColType::DateTime,
        T_BINARY => ColType::Binary,
        T_TEXT => ColType::Text,
        T_OLE => ColType::Ole,
        T_MEMO => ColType::Memo,
        T_GUID => ColType::Guid,
        T_NUMERIC => ColType::Decimal,
        T_COMPLEX => ColType::MultiValue,
        T_BIGINT => ColType::Int64,
        _ => ColType::Binary,
    }
}

pub fn parse(buf: &[u8], name: &str, page: u32, f: &AccessFile) -> Result<TableDef, Error> {
    let r = Rd(buf);
    let jet3 = f.jet.is_jet3();
    let (o_rows, o_ncols, o_nidx, o_nridx, o_map, o_start, ridx_sz, col_sz) =
        if jet3 { (12, 25, 27, 31, 35, 43, 8, 18) } else { (16, 45, 47, 51, 55, 63, 12, 25) };
    let num_rows = r.u32(o_rows);
    let ncols = r.u16(o_ncols) as usize;
    let nidx = r.u32(o_nidx) as usize;
    let nridx = r.u32(o_nridx) as usize;
    if ncols > 4096 || nidx > 4096 || nridx > 4096 {
        return Err(Error::Format(format!("{name}: bad table definition")));
    }
    let usage_map = r.u32(o_map);
    let mut pos = o_start + nridx * ridx_sz;
    let mut columns = Vec::with_capacity(ncols);
    for _ in 0..ncols {
        let e = Rd(r.bytes(pos, col_sz));
        let raw_type = e.u8(0);
        let (num, var_index, flags, ext, fixed_offset, len) =
            if jet3 { (e.u16(1), e.u16(3), e.u8(13), 0, e.u16(14), e.u16(16)) } else { (e.u16(5), e.u16(7), e.u8(15), e.u8(16), e.u16(21), e.u16(23)) };
        let ty = col_type(raw_type);
        let mut col = Column::new(String::new(), ty);
        col.size = match raw_type {
            T_TEXT if !jet3 => len as u32 / 2,
            _ => len as u32,
        };
        col.precision = e.u8(11);
        col.scale = e.u8(12);
        col.auto = flags & 0x04 != 0 || (raw_type == T_GUID && flags & 0x40 != 0);
        col.hyperlink = raw_type == T_MEMO && flags & 0x80 != 0;
        columns.push(ColDef { col, raw_type, num, var_index, fixed_offset, len, fixed: flags & 0x01 != 0, calculated: !jet3 && ext & 0xc0 != 0, result: raw_type });
        pos += col_sz;
    }
    for c in &mut columns {
        let n = if jet3 { r.u8(pos) as usize } else { r.u16(pos) as usize };
        pos += if jet3 { 1 } else { 2 };
        c.col.name = name_text(f, r.bytes(pos, n));
        pos += n;
    }
    // Physical indexes: up to ten columns each, and flags.
    let mut real: Vec<(Vec<(u16, bool)>, u8)> = Vec::with_capacity(nridx);
    for _ in 0..nridx {
        if !jet3 {
            pos += 4;
        }
        let mut cols = Vec::new();
        for k in 0..10 {
            let n = r.u16(pos + k * 3);
            if n != 0xffff {
                cols.push((n, r.u8(pos + k * 3 + 2) & 1 != 0));
            }
        }
        pos += 30;
        // Usage map and first page, then the flags.
        let flags = r.u8(pos + 8 + if jet3 { 0 } else { 4 });
        pos += if jet3 { 9 } else { 4 + 4 + 4 + 1 + 5 };
        real.push((cols, flags));
    }
    // Logical indexes: a physical one each, and what they are for.
    let mut logical = Vec::with_capacity(nidx);
    for _ in 0..nidx {
        if !jet3 {
            pos += 4;
        }
        let real_no = r.u32(pos + 4) as usize;
        let kind = r.u8(pos + 19);
        pos += 20;
        if !jet3 {
            pos += 4;
        }
        logical.push((real_no, kind));
    }
    let mut indexes = Vec::with_capacity(nidx);
    for (real_no, kind) in logical {
        let n = if jet3 { r.u8(pos) as usize } else { r.u16(pos) as usize };
        pos += if jet3 { 1 } else { 2 };
        let iname = name_text(f, r.bytes(pos, n));
        pos += n;
        let Some((cols, flags)) = real.get(real_no) else { continue };
        let columns: Vec<(String, bool)> =
            cols.iter().filter_map(|(num, asc)| columns.iter().find(|c| c.num == *num).map(|c| (c.col.name.clone(), *asc))).collect();
        indexes.push(Index { name: iname, columns, primary: kind == 1, unique: kind == 1 || flags & 0x01 != 0, foreign: kind == 2 });
    }
    // Columns in the order of their numbers, which is the order Access
    // shows them in.
    columns.sort_by_key(|c| c.num);
    for c in &mut columns {
        if indexes.iter().any(|i| i.primary && i.columns.iter().any(|k| k.0 == c.col.name)) {
            c.col.required = true;
        }
    }
    Ok(TableDef { name: name.to_string(), page, num_rows, columns, indexes, usage_map })
}
