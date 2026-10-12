//! Rows: which data pages a table has (its usage map), the rows on each
//! page, and the values in a row (fixed-length columns first, then
//! variable-length ones found through a table of offsets at the row's end,
//! and a bit per column saying whether it has a value). Long values (memo,
//! OLE) are stored inline or in rows of their own ("LVAL" pages).

use crate::model::Value;

use super::tdef::*;
use super::AccessFile;

impl AccessFile {
    /// Where the rows of data pages start: the row count, then 2-byte
    /// offsets.
    fn row_count_at(&self) -> usize {
        if self.jet.is_jet3() { 8 } else { 12 }
    }

    /// Row `row` of page `pg`: its flags (0x8000 deleted, 0x4000 a pointer
    /// to the row elsewhere) and its bytes.
    pub(super) fn find_row(&self, pg: u32, row: u32) -> Option<(u16, &[u8])> {
        let p = self.page(pg);
        let r = Rd(p);
        let rc = self.row_count_at();
        let count = r.u16(rc) as u32;
        if row >= count {
            return None;
        }
        let off = r.u16(rc + 2 + 2 * row as usize);
        let start = (off & 0x1fff) as usize;
        let end = if row == 0 { self.page_size } else { (r.u16(rc + 2 * row as usize) & 0x1fff) as usize };
        // Rows lie past the table of their offsets, and are never empty.
        if start >= end || end > p.len() || start < rc + 2 + 2 * count as usize {
            return None;
        }
        Some((off & 0xe000, &p[start..end]))
    }

    /// The row a pointer (row number in the low byte, page above) names.
    fn row_at(&self, ptr: u32) -> Option<&[u8]> {
        self.find_row(ptr >> 8, ptr & 0xff).map(|r| r.1)
    }

    /// The pages a usage map lists.
    fn usage_pages(&self, ptr: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let Some(map) = self.row_at(ptr) else { return out };
        match map.first() {
            Some(0) if map.len() >= 5 => {
                let start = u32::from_le_bytes([map[1], map[2], map[3], map[4]]);
                bits(&map[5..], |i| out.extend(start.checked_add(i)));
            }
            Some(1) => {
                let per_page = ((self.page_size - 4) * 8) as u32;
                for (j, c) in map[1..].chunks_exact(4).enumerate() {
                    let pg = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                    if pg == 0 || pg >= self.pages() {
                        continue;
                    }
                    let p = self.page(pg);
                    bits(&p[4..], |i| out.push(j as u32 * per_page + i));
                }
            }
            _ => {}
        }
        out
    }

    /// All the rows of a table, as values in the order of its columns.
    pub(super) fn rows(&self, def: &TableDef) -> Vec<Vec<Value>> {
        use std::sync::atomic::Ordering;
        let mut out = Vec::with_capacity((def.num_rows as usize).min(1 << 16));
        let mut pages = self.usage_pages(def.usage_map);
        if pages.is_empty() {
            // No readable map: every data page that says it is this table's.
            pages = (1..self.pages()).collect();
        }
        let rc = self.row_count_at();
        for pg in pages {
            let p = self.page(pg);
            let r = Rd(p);
            if p.first() != Some(&0x01) || r.u32(4) != def.page {
                continue;
            }
            let count = r.u16(rc) as u32;
            for row in 0..count {
                let Some((flags, bytes)) = self.find_row(pg, row) else { continue };
                if flags & 0x8000 != 0 {
                    continue;
                }
                let bytes = if flags & 0x4000 != 0 {
                    // Moved to another page: follow the pointer.
                    if bytes.len() < 4 {
                        continue;
                    }
                    let ptr = u32::from_le_bytes([bytes[1], bytes[2], bytes[3], 0]) << 8 | bytes[0] as u32;
                    match self.row_at(ptr) {
                        Some(b) => b,
                        None => continue,
                    }
                } else {
                    bytes
                };
                let cost = def.columns.len().max(1);
                if self.cells_left.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |l| l.checked_sub(cost)).is_err() {
                    return out;
                }
                let Some(row) = self.crack(def, bytes) else { return out };
                out.push(row);
            }
        }
        out
    }

    /// The values of a row.
    /// None once the values copied add up to more than the file can hold.
    fn crack(&self, def: &TableDef, row: &[u8]) -> Option<Vec<Value>> {
        use std::sync::atomic::Ordering;
        let jet3 = self.jet.is_jet3();
        let r = Rd(row);
        let len = row.len();
        let head = if jet3 { 1 } else { 2 };
        let ncols = if jet3 { r.u8(0) as usize } else { r.u16(0) as usize };
        let mask_len = ncols.div_ceil(8);
        if len < head + mask_len {
            return Some(vec![Value::Null; def.columns.len()]);
        }
        let mask = &row[len - mask_len..];
        let (nvar, offsets) = if jet3 { var_offsets3(row, mask_len) } else { var_offsets4(row, mask_len) };
        let nfixed_row = ncols.saturating_sub(nvar);
        let mut fixed_seen = 0;
        let mut vals = Vec::with_capacity(def.columns.len());
        for c in &def.columns {
            let num = c.num as usize;
            let present = num < ncols && mask.get(num / 8).is_some_and(|b| b >> (num % 8) & 1 == 1);
            if c.raw_type == T_BOOL {
                vals.push(Value::Bool(present));
                continue;
            }
            let data: Option<&[u8]> = if c.fixed {
                let ok = fixed_seen < nfixed_row;
                fixed_seen += 1;
                let a = head + c.fixed_offset as usize;
                (ok && present).then(|| r.bytes(a, c.len as usize)).filter(|d| d.len() == c.len as usize)
            } else {
                let i = c.var_index as usize;
                if present && i < nvar && i + 1 < offsets.len() {
                    let (a, b) = (offsets[i] as usize, offsets[i + 1] as usize);
                    row.get(a..b)
                } else {
                    None
                }
            };
            if let Some(d) = data {
                if self.inline_left.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |l| l.checked_sub(d.len())).is_err() {
                    return None;
                }
            }
            vals.push(match data {
                Some(d) => self.value(c, d),
                None => Value::Null,
            });
        }
        Some(vals)
    }

    /// A column's value from its bytes.
    fn value(&self, c: &ColDef, d: &[u8]) -> Value {
        if c.calculated {
            // Calculated columns: the value's length at 16, the value at
            // 20; memos have it inside their long value.
            let unwrap = |d: &[u8]| -> Vec<u8> {
                if d.len() < 20 {
                    return d.to_vec();
                }
                let n = u32::from_le_bytes([d[16], d[17], d[18], d[19]]) as usize;
                d[20..(20 + n).min(d.len())].to_vec()
            };
            let inner = if matches!(c.raw_type, T_MEMO | T_OLE) || matches!(c.result, T_MEMO | T_OLE) { unwrap(&self.long_value(d)) } else { unwrap(d) };
            if inner.is_empty() {
                return Value::Null;
            }
            return match c.result {
                T_MEMO | T_TEXT => Value::Text(text_of(self, &inner)),
                T_OLE => Value::Blob(inner),
                T_BOOL => Value::Bool(inner.first().is_some_and(|b| *b != 0)),
                T_NUMERIC => Value::Decimal(calc_numeric(&inner)),
                t => self.plain(t, false, c.col.scale, &inner),
            };
        }
        self.plain(c.raw_type, c.fixed, c.col.scale, d)
    }

    /// A value of stored type `ty` from its bytes.
    fn plain(&self, ty: u8, fixed: bool, scale: u8, d: &[u8]) -> Value {
        let r = Rd(d);
        match ty {
            T_BYTE => Value::Int(r.u8(0) as i64),
            T_INT => Value::Int(r.u16(0) as i16 as i64),
            T_LONG => Value::Int(r.u32(0) as i32 as i64),
            T_COMPLEX => Value::Int(r.u32(0) as i32 as i64),
            T_BIGINT => Value::Int(i64::from_le_bytes(eight(d))),
            T_MONEY => Value::Money(i64::from_le_bytes(eight(d))),
            T_FLOAT => {
                let f = f32::from_le_bytes([r.u8(0), r.u8(1), r.u8(2), r.u8(3)]);
                // The shortest decimal that is this float, as a double.
                Value::Num(format!("{f}").parse().unwrap_or(f as f64))
            }
            T_DOUBLE => Value::Num(f64::from_le_bytes(eight(d))),
            T_DATETIME => Value::Date(f64::from_le_bytes(eight(d))),
            T_EXT_DATETIME => ext_date(d),
            T_GUID => Value::Guid(guid(d)),
            T_NUMERIC => Value::Decimal(numeric(d, scale)),
            T_TEXT => {
                if fixed {
                    Value::Text(text_of(self, d).trim_end_matches(['\0', ' ']).to_string())
                } else {
                    Value::Text(text_of(self, d))
                }
            }
            T_BINARY => Value::Blob(d.to_vec()),
            T_MEMO => Value::Text(text_of(self, &self.long_value(d))),
            T_OLE => Value::Blob(self.long_value(d)),
            _ => Value::Blob(d.to_vec()),
        }
    }

    /// A memo or OLE value: 12 bytes (length and flags, a pointer) then,
    /// when inline, the data.
    fn long_value(&self, d: &[u8]) -> Vec<u8> {
        use std::sync::atomic::Ordering;
        let r = Rd(d);
        let head = r.u32(0);
        let left = self.long_left.load(Ordering::Relaxed);
        let len = ((head & 0x3fff_ffff) as usize).min(left);
        // Inline and one-row values count too: many cells may point at
        // the same row.
        let charged = |out: Vec<u8>| {
            self.long_left.fetch_sub(out.len().min(left), Ordering::Relaxed);
            out
        };
        if head & 0x8000_0000 != 0 {
            return charged(d.get(12..12 + len.min(d.len().saturating_sub(12))).unwrap_or(&[]).to_vec());
        }
        let mut ptr = r.u32(4);
        if head & 0x4000_0000 != 0 {
            // All in one row.
            return charged(self.row_at(ptr).map(|b| b[..len.min(b.len())].to_vec()).unwrap_or_default());
        }
        // A chain of rows, each starting with the pointer to the next.
        let mut out = Vec::with_capacity(len.min(1 << 24));
        // A row met again is a loop: the chain ends there.
        let mut seen = std::collections::HashSet::new();
        while ptr != 0 && out.len() < len && seen.insert(ptr) {
            let Some(b) = self.row_at(ptr) else { break };
            if b.len() < 4 {
                break;
            }
            out.extend_from_slice(&b[4..]);
            ptr = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        }
        out.truncate(len);
        self.long_left.fetch_sub(out.len().min(left), Ordering::Relaxed);
        out
    }
}

fn eight(d: &[u8]) -> [u8; 8] {
    let mut b = [0; 8];
    for (i, x) in d.iter().take(8).enumerate() {
        b[i] = *x;
    }
    b
}

/// Calls `f` with the number of every set bit.
fn bits(map: &[u8], mut f: impl FnMut(u32)) {
    for (i, b) in map.iter().enumerate() {
        if *b == 0 {
            continue;
        }
        for k in 0..8 {
            if b >> k & 1 == 1 {
                f(i as u32 * 8 + k);
            }
        }
    }
}

/// Jet 4 rows end with the variable columns' offsets (2 bytes each,
/// backwards, plus the end of the data), their count and the null mask.
fn var_offsets4(row: &[u8], mask_len: usize) -> (usize, Vec<u16>) {
    let r = Rd(row);
    let len = row.len();
    let Some(at) = len.checked_sub(mask_len + 2) else { return (0, Vec::new()) };
    let nvar = r.u16(at) as usize;
    let mut offs = Vec::with_capacity(nvar + 1);
    for i in 0..=nvar {
        let Some(o) = at.checked_sub(2 + 2 * i) else { return (0, Vec::new()) };
        offs.push(r.u16(o));
    }
    (nvar, offs)
}

/// Jet 3 rows keep 1-byte offsets, with a "jump table" saying where the
/// offsets pass another 256 bytes.
fn var_offsets3(row: &[u8], mask_len: usize) -> (usize, Vec<u16>) {
    let len = row.len();
    let Some(end) = len.checked_sub(1) else { return (0, Vec::new()) };
    let Some(at) = end.checked_sub(mask_len) else { return (0, Vec::new()) };
    let nvar = row[at] as usize;
    let mut jumps = end / 256;
    let Some(col_ptr) = at.checked_sub(jumps + 1) else { return (0, Vec::new()) };
    if (col_ptr.saturating_sub(nvar)) / 256 < jumps {
        jumps -= 1;
    }
    let mut used = 0;
    let mut offs = Vec::with_capacity(nvar + 1);
    for i in 0..=nvar {
        while used < jumps && at.checked_sub(used + 1).is_some_and(|k| row[k] as usize == i) {
            used += 1;
        }
        let Some(k) = col_ptr.checked_sub(i) else { return (0, Vec::new()) };
        offs.push(row[k] as u16 + used as u16 * 256);
    }
    (nvar, offs)
}

fn text_of(f: &AccessFile, d: &[u8]) -> String {
    if f.jet.is_jet3() { crate::codepage::decode(d, f.codepage) } else { text(d, f) }
}

/// Jet 4 text: UTF-16, or "compressed" (after FF FE) in runs that switch
/// between one byte per character and two at every zero byte.
pub(super) fn text(d: &[u8], _f: &AccessFile) -> String {
    if d.len() >= 2 && d[0] == 0xff && d[1] == 0xfe {
        let mut units: Vec<u16> = Vec::with_capacity(d.len());
        let mut compressed = true;
        let mut i = 2;
        let mut start = 2;
        let flush = |a: usize, b: usize, compressed: bool, units: &mut Vec<u16>| {
            if compressed {
                units.extend(d[a..b].iter().map(|&x| x as u16));
            } else {
                units.extend(d[a..b].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])));
            }
        };
        while i < d.len() {
            if d[i] == 0 {
                flush(start, i, compressed, &mut units);
                compressed = !compressed;
                start = i + 1;
            }
            i += 1;
        }
        flush(start, d.len(), compressed, &mut units);
        String::from_utf16_lossy(&units)
    } else {
        let units: Vec<u16> = d.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&units)
    }
}

/// {XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}, the first three parts stored
/// little-endian.
fn guid(d: &[u8]) -> String {
    if d.len() < 16 {
        return String::new();
    }
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        u32::from_le_bytes([d[0], d[1], d[2], d[3]]),
        u16::from_le_bytes([d[4], d[5]]),
        u16::from_le_bytes([d[6], d[7]]),
        d[8], d[9], d[10], d[11], d[12], d[13], d[14], d[15]
    )
}

/// NUMERIC: a sign byte, then a 128-bit integer as four 32-bit words
/// (most significant first), each little-endian; divided by 10^scale.
fn numeric(d: &[u8], scale: u8) -> String {
    if d.len() < 17 {
        return String::new();
    }
    let mut v: u128 = 0;
    for w in d[1..17].chunks_exact(4) {
        v = v << 32 | u32::from_le_bytes([w[0], w[1], w[2], w[3]]) as u128;
    }
    decimal_text(v, scale, d[0] != 0)
}

/// An integer with `scale` decimals as text, without trailing zeros.
fn decimal_text(v: u128, scale: u8, neg: bool) -> String {
    let mut s = v.to_string();
    let scale = scale as usize;
    if scale > 0 {
        if s.len() <= scale {
            s = format!("{}{s}", "0".repeat(scale + 1 - s.len()));
        }
        s.insert(s.len() - scale, '.');
        let t = s.trim_end_matches('0').trim_end_matches('.');
        s = t.to_string();
    }
    if neg && s != "0" {
        s.insert(0, '-');
    }
    s
}

/// The decimals of calculated columns: the length (2 bytes), the scale, the
/// sign, then the digits' bytes: a 4-byte little-endian word when their
/// length is not a multiple of 8, then little-endian 8-byte words, most
/// significant first.
fn calc_numeric(d: &[u8]) -> String {
    if d.len() < 4 {
        return String::new();
    }
    let total = u16::from_le_bytes([d[0], d[1]]) as usize;
    let n = if total > 0 { total } else { d.len() - 2 }.saturating_sub(2);
    let n = (n.div_ceil(4) * 4).min(16).min(d.len() - 4);
    let (scale, neg) = (d[2], d[3] != 0);
    let digits = &d[4..4 + n];
    let mut v: u128 = 0;
    let mut i = 0;
    if n % 8 != 0 {
        // Fewer than a word's digits given: no number.
        if n < 4 {
            return String::new();
        }
        v = u32::from_le_bytes([digits[0], digits[1], digits[2], digits[3]]) as u128;
        i = 4;
    }
    while i + 8 <= n {
        let mut w = [0u8; 8];
        w.copy_from_slice(&digits[i..i + 8]);
        v = v.checked_shl(64).unwrap_or(0) | u64::from_le_bytes(w) as u128;
        i += 8;
    }
    decimal_text(v, scale, neg)
}

/// Access 2019's extended dates: text "days:seconds:..." padded with
/// zeros ("0000000000001:0000000000000:7"...): days from 0001-01-01.
fn ext_date(d: &[u8]) -> Value {
    // "<19 digits of days since 0001-01-01>:<12 digits of seconds><7 digits
    // of 100 ns>" and a trailer.
    let digits = |a: usize, n: usize| -> Option<i64> {
        let b = d.get(a..a + n)?;
        b.iter().try_fold(0i64, |v, &c| if c.is_ascii_digit() { v.checked_mul(10)?.checked_add((c - b'0') as i64) } else { None })
    };
    let (Some(days), Some(secs), Some(ticks)) = (digits(0, 19), digits(20, 12), digits(32, 7)) else {
        return Value::Text(String::from_utf8_lossy(d).trim_end_matches('\0').to_string());
    };
    let Some(day) = crate::model::ymd_to_days(1, 1, 1).checked_add(days) else {
        return Value::Text(String::from_utf8_lossy(d).trim_end_matches('\0').to_string());
    };
    let frac = (secs as f64 + ticks as f64 / 1e7) / 86400.0;
    // OLE dates before 1899-12-30 count the time forward from the day.
    Value::Date(if day < 0 && frac > 0.0 { day as f64 - frac } else { day as f64 + frac })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numerics() {
        let mut d = [0u8; 17];
        d[13] = 0x39;
        d[14] = 0x30; // 12345 in the last word
        assert_eq!(numeric(&d, 2), "123.45");
        d[0] = 0x80;
        assert_eq!(numeric(&d, 6), "-0.012345");
        assert_eq!(numeric(&d, 0), "-12345");
    }

    #[test]
    fn guids() {
        let d: Vec<u8> = (0..16).collect();
        assert_eq!(guid(&d), "{03020100-0504-0706-0809-0A0B0C0D0E0F}");
    }
}
