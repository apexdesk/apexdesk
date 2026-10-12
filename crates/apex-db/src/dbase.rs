//! dBase tables (`.dbf`): dBase III, IV, 5 and 7, FoxPro and Visual FoxPro,
//! and the shapefiles' attribute tables. A header, a 32-byte (48 in dBase
//! 7) descriptor per field, then fixed-length records. Memo fields keep a
//! block number into the memo file: `.dbt` (dBase) or `.fpt` (FoxPro).
//! Text is in the code page the header's language driver names, or the one
//! of a `.cpg` file next to the table.

use std::path::{Path, PathBuf};

use crate::codepage;
use crate::model::{ColType, Column, Database, Table, Value, ymd_to_days};
use crate::Error;

#[derive(Clone, Debug)]
struct Field {
    name: String,
    ty: u8,
    len: usize,
    dec: u8,
    offset: usize,
    /// Visual FoxPro: 0x02 nullable.
    flags: u8,
    /// Bits in _NullFlags: "shorter than the field" (varchar) and null.
    var_bit: Option<usize>,
    null_bit: Option<usize>,
}

/// The table's name: the file's name without its extension.
pub fn table_name(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Table".into())
}

/// Finds a file next to `path` with extension `ext` in either case.
fn sibling(path: &Path, ext: &str) -> Option<PathBuf> {
    for e in [ext.to_ascii_lowercase(), ext.to_ascii_uppercase()] {
        let p = path.with_extension(&e);
        // Only a file of its own: a link could bring in any other file
        // (a key) as the memo text.
        if std::fs::symlink_metadata(&p).is_ok_and(|m| m.is_file()) {
            return Some(p);
        }
    }
    None
}

pub fn read_path(path: &Path) -> Result<Database, Error> {
    let data = crate::read_file(path)?;
    let memo = sibling(path, "fpt").or_else(|| sibling(path, "dbt")).and_then(|p| std::fs::read(p).ok());
    let cpg = sibling(path, "cpg").and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| codepage::from_name(&s));
    let mut db = Database::default();
    let mut warnings = Vec::new();
    let t = read(&data, memo.as_deref(), &table_name(path), cpg, &mut warnings)?;
    db.tables.push(t);
    db.warnings = warnings;
    Ok(db)
}

/// The code page a language driver id means.
fn driver_codepage(id: u8) -> Option<u16> {
    Some(match id {
        0x01 => 437,
        0x02 => 850,
        0x03 => 1252,
        0x04 => 10000,
        0x08 | 0x09 | 0x0a | 0x0b | 0x0d | 0x0e | 0x0f | 0x10 | 0x11 | 0x12 | 0x13 | 0x14 | 0x15 | 0x16 | 0x17 | 0x18 | 0x19 | 0x1a | 0x1b | 0x1c | 0x1d | 0x1e | 0x1f | 0x24 | 0x25 | 0x26 | 0x37 | 0x40 => {
            // DOS code pages of dBase's national drivers: 865, 437, 850...
            match id {
                0x08 | 0x0b | 0x0e | 0x10 | 0x12 | 0x14 | 0x16 | 0x18 | 0x1b | 0x1d | 0x1f | 0x25 | 0x37 => 850,
                0x26 => 866,
                _ => 437,
            }
        }
        0x57 => 1252,
        0x58 | 0x59 => 1252,
        0x64 => 852,
        0x65 => 866,
        0x66 => 865,
        0x67 => 861,
        0x6a => 737,
        0x6b => 857,
        0x78 => 950,
        0x79 => 949,
        0x7a => 936,
        0x7b => 932,
        0x7c => 874,
        0x7d => 1255,
        0x7e => 1256,
        0x96 => 10007,
        0xc8 => 1250,
        0xc9 => 1251,
        0xca => 1254,
        0xcb => 1253,
        0xcc => 1257,
        _ => return None,
    })
}

/// A dBase II file (a 521-byte header: the record count at 1, the record
/// length at 6, then up to 32 16-byte field descriptors) rewritten with a
/// dBase III header, which reads the same from there.
fn from_dbase2(d: &[u8]) -> Vec<u8> {
    let nrec = u16::from_le_bytes([d[1], d[2]]) as u32;
    let rec_len = u16::from_le_bytes([d[6], d[7]]);
    let mut fields = Vec::new();
    let mut p = 8;
    while p + 16 <= 520 && d[p] != 0x0d && d[p] != 0 {
        let mut f = [0u8; 32];
        f[..11].copy_from_slice(&d[p..p + 11]);
        f[11] = d[p + 11];
        f[16] = d[p + 12];
        f[17] = d[p + 15];
        fields.push(f);
        p += 16;
    }
    let header_len = 32 + fields.len() * 32 + 1;
    let mut out = vec![0u8; 32];
    out[0] = 0x03;
    out[4..8].copy_from_slice(&nrec.to_le_bytes());
    out[8..10].copy_from_slice(&(header_len as u16).to_le_bytes());
    out[10..12].copy_from_slice(&rec_len.to_le_bytes());
    for f in fields {
        out.extend_from_slice(&f);
    }
    out.push(0x0d);
    out.extend_from_slice(&d[521..]);
    out
}

/// Reads a table from its bytes and those of its memo file.
pub fn read(data: &[u8], memo: Option<&[u8]>, name: &str, cpg: Option<u16>, warnings: &mut Vec<String>) -> Result<Table, Error> {
    if data.len() < 32 {
        return Err(Error::Format("not a dBase file".into()));
    }
    if data[0] == 0x02 && data.len() >= 521 {
        return read(&from_dbase2(data), memo, name, cpg, warnings);
    }
    const MAX_CELLS: usize = 64 << 20;
    let version = data[0];
    let level7 = version & 0x07 == 4;
    let nrec = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
    let header_len = u16::from_le_bytes([data[8], data[9]]) as usize;
    let rec_len = u16::from_le_bytes([data[10], data[11]]) as usize;
    if header_len < 33 || rec_len == 0 || header_len > data.len() {
        return Err(Error::Format("not a dBase file".into()));
    }
    // Without a known driver, text that is valid UTF-8 (and not plain
    // ASCII) is taken as UTF-8, as many newer programs write it.
    let (desc_len, name_len, first) = if level7 { (48, 32, 68) } else { (32, 11, 32) };
    let sniff = || {
        let mut text = Vec::new();
        let mut p = first;
        while p + desc_len <= header_len && data[p] != 0x0d {
            let name = &data[p..p + name_len];
            text.extend_from_slice(&name[..name.iter().position(|&b| b == 0).unwrap_or(name_len)]);
            text.push(b' ');
            p += desc_len;
        }
        for r in data[header_len.min(data.len())..].chunks(rec_len).take(200) {
            text.extend_from_slice(r);
            text.push(b' ');
        }
        (!text.is_ascii() && std::str::from_utf8(&text).is_ok()).then_some(65001)
    };
    let cp = cpg.or_else(|| driver_codepage(data[29])).or_else(sniff).unwrap_or(if matches!(version, 0x30 | 0x31 | 0x32) { 1252 } else { 437 });
    let mut fields = Vec::new();
    let mut pos = first;
    let mut offset = 1;
    while pos + desc_len <= header_len && data[pos] != 0x0d {
        let d = &data[pos..pos + desc_len];
        let raw = &d[..name_len];
        let end = raw.iter().position(|&b| b == 0).unwrap_or(name_len);
        let fname = codepage::decode(&raw[..end], cp).trim().to_string();
        let (ty, len, dec, flags) = if level7 { (d[32], d[33] as usize, d[34], 0) } else { (d[11], d[16] as usize, d[17], d[18]) };
        // Character fields longer than 255 keep the high byte in the
        // decimals (Clipper, FoxPro).
        let len = if ty == b'C' && !level7 && dec > 0 { len + ((dec as usize) << 8) } else { len };
        let dec = if ty == b'C' { 0 } else { dec };
        fields.push(Field { name: fname, ty, len, dec, offset, flags, var_bit: None, null_bit: None });
        offset += len;
        pos += desc_len;
    }
    // Fields that don't fit the records hold nothing: thousands of them
    // over one-byte records would make a row of each byte of the file.
    fields.retain(|f| f.len > 0 && f.offset + f.len <= rec_len);
    let memo_block = memo_block_size(memo, version);
    let mut bit = 0;
    for f in &mut fields {
        if f.ty == b'0' {
            continue;
        }
        if matches!(f.ty, b'V' | b'Q') {
            f.var_bit = Some(bit);
            bit += 1;
        }
        if f.flags & 0x02 != 0 {
            f.null_bit = Some(bit);
            bit += 1;
        }
    }
    // Visual FoxPro's hidden _NullFlags field: a bit per nullable field.
    let nullflags = fields.iter().position(|f| f.ty == b'0');
    let columns: Vec<Column> = fields
        .iter()
        .filter(|f| f.ty != b'0')
        .map(|f| {
            let ty = match f.ty {
                b'C' | b'V' => ColType::Text,
                b'N' | b'F' => {
                    if f.dec == 0 && f.len < 10 { ColType::Int32 } else if f.dec == 0 && f.len < 19 { ColType::Int64 } else { ColType::Double }
                }
                b'L' => ColType::Bool,
                b'D' | b'T' | b'@' => ColType::DateTime,
                b'M' => ColType::Memo,
                b'G' | b'P' | b'W' | b'Q' => ColType::Ole,
                b'B' if version & 0xf0 == 0x30 => ColType::Double,
                b'B' => ColType::Ole,
                b'I' | b'+' => ColType::Int32,
                b'Y' => ColType::Money,
                b'O' => ColType::Double,
                _ => ColType::Text,
            };
            let mut c = Column::new(f.name.clone(), ty);
            c.size = if ty == ColType::Text { f.len as u32 } else { 0 };
            c.precision = if matches!(f.ty, b'N' | b'F') { f.len as u8 } else { 0 };
            c.scale = f.dec;
            c.auto = f.ty == b'+';
            c
        })
        .collect();
    let mut t = Table::new(name, columns);
    let mut bad_memo = false;
    let mut at = header_len;
    let mut cells_left = MAX_CELLS;
    // Bytes of memo copied or searched: records may all point at one block.
    let mut memo_left: usize = 1 << 30;
    for _ in 0..nrec {
        let Some(rec) = data.get(at..at + rec_len) else { break };
        let Some(left) = cells_left.checked_sub(t.columns.len().max(1)) else {
            warnings.push(format!("{name}: too large, only the first rows were read"));
            break;
        };
        cells_left = left;
        at += rec_len;
        if rec[0] == b'*' {
            continue;
        }
        if rec[0] == 0x1a {
            break;
        }
        let nulls = nullflags.and_then(|i| rec.get(fields[i].offset..fields[i].offset + fields[i].len));
        let flag = |b: Option<usize>| match (nulls, b) {
            (Some(n), Some(b)) => n.get(b / 8).is_some_and(|x| x >> (b % 8) & 1 == 1),
            _ => false,
        };
        let mut row = Vec::with_capacity(t.columns.len());
        for f in fields.iter().filter(|f| f.ty != b'0') {
            let mut raw = rec.get(f.offset..f.offset + f.len).unwrap_or(&[]);
            let is_null = flag(f.null_bit);
            if flag(f.var_bit)
                && let Some(&n) = raw.last()
            {
                raw = &raw[..(n as usize).min(raw.len())];
            }
            let v = if is_null { Value::Null } else { field_value(f, raw, version, cp, (memo, &mut memo_left), memo_block, &mut bad_memo) };
            row.push(v);
        }
        t.rows.push(row);
    }
    if bad_memo {
        warnings.push(format!("{name}: memo file missing or damaged"));
    }
    Ok(t)
}

fn memo_block_size(memo: Option<&[u8]>, version: u8) -> usize {
    let Some(m) = memo else { return 512 };
    if m.len() < 8 {
        return 512;
    }
    if version & 0xf0 == 0x30 || version == 0xf5 || version == 0xfb {
        // FoxPro: big-endian block size at 6.
        let b = u16::from_be_bytes([m[6], m[7]]) as usize;
        return if b == 0 { 64 } else { b };
    }
    if version == 0x83 {
        return 512;
    }
    // dBase IV and later: the block size at 20 (0 means 512).
    let b = m.get(20..22).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]) as usize);
    if b == 0 { 512 } else { b }
}

fn field_value(f: &Field, raw: &[u8], version: u8, cp: u16, (memo, memo_left): (Option<&[u8]>, &mut usize), block: usize, bad: &mut bool) -> Value {
    let text = || codepage::decode(raw, cp);
    match f.ty {
        b'C' => {
            let s = text();
            let s = s.trim_end_matches([' ', '\0']);
            if s.is_empty() { Value::Null } else { Value::Text(s.to_string()) }
        }
        b'V' => {
            // Visual FoxPro varchar: the length in the last byte when
            // shorter than the field.
            let s = text();
            Value::Text(s.trim_end_matches(['\0', ' ']).to_string())
        }
        b'N' | b'F' => {
            let s = text();
            let s = s.trim().trim_start_matches('+');
            if s.is_empty() || s.starts_with('*') {
                return Value::Null;
            }
            match s.parse::<f64>() {
                Ok(v) if f.dec == 0 && v.fract() == 0.0 && v.abs() < 9e15 => Value::Int(v as i64),
                Ok(v) => Value::Num(v),
                Err(_) => Value::Null,
            }
        }
        b'L' => match raw.first() {
            Some(b'T' | b't' | b'Y' | b'y') => Value::Bool(true),
            Some(b'F' | b'f' | b'N' | b'n') => Value::Bool(false),
            _ => Value::Null,
        },
        b'D' => {
            let s = String::from_utf8_lossy(raw);
            let s = s.trim();
            if s.len() != 8 || !s.bytes().all(|b| b.is_ascii_digit()) {
                return Value::Null;
            }
            match (s[0..4].parse::<i64>(), s[4..6].parse::<u32>(), s[6..8].parse::<u32>()) {
                (Ok(y), Ok(m), Ok(d)) if (1..=12).contains(&m) && (1..=31).contains(&d) => Value::Date(ymd_to_days(y, m, d) as f64),
                _ => Value::Null,
            }
        }
        b'T' if raw.len() == 8 => {
            // Julian day and milliseconds.
            let jd = i32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as i64;
            let ms = i32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]) as f64;
            if jd == 0 {
                return Value::Null;
            }
            Value::Date((jd - 2415019) as f64 + ms / 86_400_000.0)
        }
        b'@' if raw.len() == 8 => {
            // dBase 7 timestamps: milliseconds since 4713 BC (Julian day
            // 0) as a big-endian double with the sign bit flipped.
            let mut b = [0u8; 8];
            b.copy_from_slice(raw);
            if b.iter().all(|&x| x == 0) {
                return Value::Null;
            }
            b[0] ^= 0x80;
            let ms = f64::from_be_bytes(b);
            Value::Date(ms / 86_400_000.0 - 2415018.5 - 0.5)
        }
        b'I' | b'+' if raw.len() == 4 => {
            if f.ty == b'+' || version & 0x07 == 4 {
                // dBase 7: big-endian with the sign bit flipped.
                let v = u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]) ^ 0x8000_0000;
                Value::Int(v as i32 as i64)
            } else {
                Value::Int(i32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as i64)
            }
        }
        b'Y' if raw.len() == 8 => Value::Money(i64::from_le_bytes(raw.try_into().unwrap_or([0; 8]))),
        b'B' if version & 0xf0 == 0x30 && raw.len() == 8 => Value::Num(f64::from_le_bytes(raw.try_into().unwrap_or([0; 8]))),
        b'O' if raw.len() == 8 => {
            let mut b = [0u8; 8];
            b.copy_from_slice(raw);
            if b[0] & 0x80 != 0 {
                b[0] &= 0x7f;
            } else {
                for x in &mut b {
                    *x = !*x;
                }
            }
            Value::Num(f64::from_be_bytes(b))
        }
        b'M' | b'G' | b'P' | b'B' | b'W' | b'Q' => {
            // A block number: 10 digits, or 4 bytes in Visual FoxPro.
            let block_no = if raw.len() == 4 {
                u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize
            } else {
                String::from_utf8_lossy(raw).trim().parse::<usize>().unwrap_or(0)
            };
            if block_no == 0 {
                return Value::Null;
            }
            let Some(m) = memo else {
                *bad = true;
                return Value::Null;
            };
            match memo_data(m, block_no, block, version, memo_left) {
                Some(d) if f.ty == b'M' => Value::Text(codepage::decode(&d, cp)),
                Some(d) => Value::Blob(d),
                None => {
                    *bad = true;
                    Value::Null
                }
            }
        }
        _ => {
            let s = text();
            let s = s.trim();
            if s.is_empty() { Value::Null } else { Value::Text(s.to_string()) }
        }
    }
}

/// The data of memo block `n`.
fn memo_data(m: &[u8], n: usize, block: usize, version: u8, left: &mut usize) -> Option<Vec<u8>> {
    let at = n.checked_mul(block)?;
    let b = m.get(at..)?;
    // What's searched and copied comes out of `left`.
    let b = &b[..b.len().min(*left)];
    if version & 0xf0 == 0x30 || version == 0xf5 || version == 0xfb {
        // FoxPro: type and length (big-endian), then the data.
        if b.len() < 8 {
            return None;
        }
        let len = u32::from_be_bytes([b[4], b[5], b[6], b[7]]) as usize;
        *left -= len.min(b.len());
        return b.get(8..8 + len).map(<[u8]>::to_vec);
    }
    if b.len() >= 8 && b[0..4] == [0xff, 0xff, 0x08, 0x00] {
        // dBase IV: a marker and the length counting these 8 bytes.
        let len = u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize;
        *left -= len.min(b.len());
        return b.get(8..len.max(8)).map(<[u8]>::to_vec);
    }
    // dBase III: up to two end-of-file marks.
    let double = b.windows(2).position(|w| w == [0x1a, 0x1a]);
    *left -= double.map_or(b.len(), |p| p + 2).min(b.len());
    let end = double.or_else(|| b.iter().position(|&x| x == 0x1a)).unwrap_or(b.len());
    let d = &b[..end];
    Some(d.strip_suffix(b"\0").unwrap_or(d).to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dBase III table with a text, a number, a date and a logical field.
    fn sample() -> Vec<u8> {
        let fields: [(&str, u8, u8, u8); 4] = [("NAME", b'C', 10, 0), ("QTY", b'N', 5, 0), ("DAY", b'D', 8, 0), ("OK", b'L', 1, 0)];
        let header_len = 32 + 32 * fields.len() + 1;
        let rec_len = 1 + 10 + 5 + 8 + 1;
        let mut d = vec![0x03, 124, 1, 1];
        d.extend(2u32.to_le_bytes());
        d.extend((header_len as u16).to_le_bytes());
        d.extend((rec_len as u16).to_le_bytes());
        d.resize(29, 0);
        d.push(0x02); // code page 850
        d.resize(32, 0);
        for (n, t, l, dec) in fields {
            let mut f = [0u8; 32];
            f[..n.len()].copy_from_slice(n.as_bytes());
            f[11] = t;
            f[16] = l;
            f[17] = dec;
            d.extend(f);
        }
        d.push(0x0d);
        d.extend(b" Pe\xa4a      ");
        d.extend(b"   42");
        d.extend(b"20240301");
        d.push(b'T');
        d.extend(b"*Gone      ");
        d.extend(b"    1");
        d.extend(b"        ");
        d.push(b'?');
        d.push(0x1a);
        d
    }

    #[test]
    fn reads_records() {
        let mut w = Vec::new();
        let t = read(&sample(), None, "t", None, &mut w).unwrap();
        assert_eq!(t.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["NAME", "QTY", "DAY", "OK"]);
        assert_eq!(t.rows.len(), 1, "the deleted record is left out");
        assert_eq!(t.rows[0], vec![Value::Text("Peña".into()), Value::Int(42), Value::Date(45352.0), Value::Bool(true)]);
    }

    #[test]
    fn memo_blocks() {
        let mut m = vec![0u8; 1024];
        m[512..517].copy_from_slice(b"hello");
        m[517] = 0x1a;
        m[518] = 0x1a;
        assert_eq!(memo_data(&m, 1, 512, 0x83, &mut { usize::MAX }).unwrap(), b"hello");
        let mut f = vec![0u8; 128];
        f[6..8].copy_from_slice(&64u16.to_be_bytes());
        f[64..68].copy_from_slice(&1u32.to_be_bytes());
        f[68..72].copy_from_slice(&3u32.to_be_bytes());
        f[72..75].copy_from_slice(b"abc");
        assert_eq!(memo_block_size(Some(&f), 0xf5), 64);
        assert_eq!(memo_data(&f, 1, 64, 0xf5, &mut { usize::MAX }).unwrap(), b"abc");
    }
}
