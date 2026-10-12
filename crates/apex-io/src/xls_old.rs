//! Excel 2.x, 3.0 and 4.0 worksheets (BIFF2, BIFF3, BIFF4), bare streams
//! without a compound file: cell values (formulas keep their last result)
//! and number formats, one sheet per worksheet substream.

use apex_sheet::{CellRef, CellStyle, Content, Sheet, Value, Workbook};

use crate::Error;
use crate::biff::{Cur, error_kind};

/// Whether `data` starts with the BOF record of BIFF2, BIFF3 or BIFF4.
pub(crate) fn is_old_bof(data: &[u8]) -> bool {
    matches!(data.get(..2), Some([0x09, 0x00 | 0x02 | 0x04]))
}

/// A bare BIFF stream (any version) starts with a BOF record.
pub(crate) fn is_bare_biff(data: &[u8]) -> bool {
    matches!(data.get(..4), Some([0x09, 0x00 | 0x02 | 0x04 | 0x08, len, 0x00]) if (4..=20).contains(len))
}

/// A number format code as stored: a byte count and 8-bit text.
fn format_code(c: &mut Cur, cp: u16) -> Option<String> {
    let n = c.u8()? as usize;
    Some(crate::ole::decode_cp(c.bytes(n.min(c.left()))?, cp))
}

pub(crate) fn read(data: &[u8]) -> Result<Workbook, Error> {
    let mut wb = Workbook::empty();
    let mut cp = 1252u16;
    // Number formats in file order, and each XF's format index.
    let mut formats: Vec<String> = Vec::new();
    let mut xf_format: Vec<usize> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut cur: Option<usize> = None;
    // A formula's text result comes in the STRING record after it.
    let mut pending: Option<(usize, CellRef, u32)> = None;
    let mut style_ids: std::collections::HashMap<usize, u32> = Default::default();
    let mut p = 0;
    while p + 4 <= data.len() {
        let id = u16::from_le_bytes([data[p], data[p + 1]]);
        let len = u16::from_le_bytes([data[p + 2], data[p + 3]]) as usize;
        let body = &data[p + 4..(p + 4 + len).min(data.len())];
        p += 4 + len;
        let mut c = Cur::new(body);
        let biff2 = id & 0xFF00 == 0;
        match id {
            // BOF: a worksheet starts (a BIFF4 workbook has one per sheet).
            0x0009 | 0x0209 | 0x0409 => {
                let kind = body.get(2..4).map_or(0x10, |b| u16::from_le_bytes([b[0], b[1]]));
                // As many sheets as LibreOffice opens at most.
                if kind == 0x10 && wb.sheets.len() < 10_000 {
                    let n = wb.sheets.len();
                    let name = names.get(n).cloned().unwrap_or_else(|| format!("Sheet{}", n + 1));
                    wb.sheets.push(Sheet::new(name));
                    cur = Some(n);
                } else {
                    cur = None;
                }
            }
            0x000A => cur = None,
            0x0042 => cp = c.u16().unwrap_or(cp),
            // BUNDLESHEET (BIFF4 workbooks): the sheet names.
            0x008F => {
                if c.skip(6).is_some() {
                    names.extend(format_code(&mut c, cp));
                }
            }
            0x001E | 0x041E => {
                if id == 0x041E {
                    c.skip(2);
                }
                formats.extend(format_code(&mut c, cp));
            }
            // XF: the format index is its second byte.
            0x0043 | 0x0243 | 0x0443 => xf_format.push(body.get(1).map_or(0, |&b| (b & 0x3F) as usize)),
            _ => {}
        }
        let Some(si) = cur else { continue };
        let is_cell = matches!(id, 0x0002..=0x0007 | 0x0203..=0x0207 | 0x027E | 0x0406 | 0x00D6);
        if !is_cell {
            continue;
        }
        // STRING: the text result of the formula before.
        if id == 0x0007 || id == 0x0207 {
            if let Some((psi, cell, st)) = pending.take() {
                let n = if id == 0x0007 { c.u8().map(usize::from) } else { c.u16().map(usize::from) };
                if let Some(t) = n.and_then(|n| c.bytes(n.min(c.left()))) {
                    wb.load_cell(psi, cell, Content::Value(Value::text(crate::ole::decode_cp(t, cp))), st);
                }
            }
            continue;
        }
        let (Some(row), Some(col)) = (c.u16(), c.u16()) else { continue };
        let cell = CellRef::new(u32::from(row), u32::from(col));
        // BIFF2 cells have 3 attribute bytes (the format in the second);
        // later ones an XF index.
        let fmt = if biff2 {
            c.bytes(3).map_or(0, |a| (a[1] & 0x3F) as usize)
        } else {
            c.u16().and_then(|x| xf_format.get(x as usize).copied()).unwrap_or(0)
        };
        let style = *style_ids.entry(fmt).or_insert_with(|| match formats.get(fmt) {
            Some(code) if fmt > 0 && !code.eq_ignore_ascii_case("General") => wb.styles.intern(CellStyle { num_fmt: code.as_str().into(), ..Default::default() }),
            _ => 0,
        });
        let value = match id {
            0x0002 => c.u16().map(|n| Value::Number(f64::from(n))),
            0x0003 | 0x0203 => c.f64().map(Value::Number),
            0x027E => c.u32().map(|v| Value::Number(crate::xls::rk(v))),
            0x0004 => c.u8().and_then(|n| c.bytes((n as usize).min(c.left()))).map(|t| Value::text(crate::ole::decode_cp(t, cp))),
            0x0204 | 0x00D6 => c.u16().and_then(|n| c.bytes((n as usize).min(c.left()))).map(|t| Value::text(crate::ole::decode_cp(t, cp))),
            0x0005 | 0x0205 => match (c.u8(), c.u8()) {
                (Some(v), Some(0)) => Some(Value::Bool(v != 0)),
                (Some(v), Some(_)) => Some(Value::Error(error_kind(v))),
                _ => None,
            },
            // FORMULA: the last result (a special marker in the last two
            // bytes for text, logical values and errors).
            0x0006 | 0x0206 | 0x0406 => match c.bytes(8) {
                Some(r) if r[6..8] == [0xFF, 0xFF] => match r[0] {
                    0 => {
                        pending = Some((si, cell, style));
                        None
                    }
                    1 => Some(Value::Bool(r[2] != 0)),
                    2 => Some(Value::Error(error_kind(r[2]))),
                    _ => None,
                },
                Some(r) => Some(Value::Number(f64::from_le_bytes(r.try_into().unwrap()))),
                None => None,
            },
            _ => None,
        };
        if let Some(v) = value {
            wb.load_cell(si, cell, Content::Value(v), style);
        }
    }
    if wb.sheets.is_empty() {
        return Err(Error::new("the workbook has no worksheets"));
    }
    wb.finish_load();
    Ok(wb)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(out: &mut Vec<u8>, id: u16, body: &[u8]) {
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&(body.len() as u16).to_le_bytes());
        out.extend_from_slice(body);
    }

    #[test]
    fn biff2_and_biff4() {
        // BIFF2: an integer, a number with a date format, a label and a
        // formula whose result is text.
        let mut d = Vec::new();
        rec(&mut d, 0x0009, &[0x00, 0x00, 0x10, 0x00]);
        rec(&mut d, 0x001E, b"\x07General");
        rec(&mut d, 0x001E, b"\x08dd/mm/yy");
        rec(&mut d, 0x0002, &[0, 0, 0, 0, 0, 0, 0, 42, 0]);
        let mut n = vec![1, 0, 0, 0, 0, 1, 0];
        n.extend_from_slice(&45000.0f64.to_le_bytes());
        rec(&mut d, 0x0003, &n);
        rec(&mut d, 0x0004, b"\x02\x00\x00\x00\x00\x00\x00\x03abc");
        rec(&mut d, 0x0006, &[3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xFF, 0, 0, 0]);
        rec(&mut d, 0x0007, b"\x02hi");
        rec(&mut d, 0x000A, &[]);
        assert!(is_bare_biff(&d) && is_old_bof(&d));
        let wb = read(&d).unwrap();
        let sh = &wb.sheets[0];
        assert_eq!(sh.value(CellRef::new(0, 0)), &Value::Number(42.0));
        assert_eq!(wb.display(0, CellRef::new(1, 0)).0, "15/03/23");
        assert_eq!(sh.value(CellRef::new(2, 0)), &Value::text("abc"));
        assert_eq!(sh.value(CellRef::new(3, 0)), &Value::text("hi"));
        // BIFF4: an XF with format 1, then an RK number and a label in it.
        let mut d = Vec::new();
        rec(&mut d, 0x0409, &[0x00, 0x00, 0x10, 0x00, 0, 0]);
        rec(&mut d, 0x041E, b"\x00\x00\x07General");
        rec(&mut d, 0x041E, b"\x01\x00\x040.00");
        rec(&mut d, 0x0443, &[0, 1, 0, 0]);
        rec(&mut d, 0x027E, &[0, 0, 1, 0, 0, 0, 0x0A, 0x00, 0x00, 0x00]);
        rec(&mut d, 0x0204, b"\x01\x00\x00\x00\x00\x00\x03\x00xyz");
        rec(&mut d, 0x000A, &[]);
        let wb = read(&d).unwrap();
        let x = wb.sheets[0].get(CellRef::new(0, 1)).unwrap();
        assert_eq!((x.value(), &*wb.styles.get(x.style).num_fmt), (&Value::Number(2.0), "0.00"));
        assert_eq!(wb.sheets[0].value(CellRef::new(1, 0)), &Value::text("xyz"));
    }
}
