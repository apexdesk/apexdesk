//! Properties of tables and columns (the catalog's `LvProp` column): what
//! the table designer sets beyond the definition page, like descriptions,
//! default values, validation rules and, for calculated columns, the
//! expression and the type of its result.
//!
//! The value starts with "MR2\0" (or "KKD\0" in Access 97), then chunks of
//! a length (4 bytes, counting its 6-byte head) and a type (2 bytes): type
//! 0x80 lists the property names, the others are one map each: the name
//! of the column it belongs to (empty for the table) and its properties.

use std::collections::HashMap;

use super::tdef::Rd;

/// A property's stored type (the column types' numbers) and its bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct Prop {
    pub ty: u8,
    pub data: Vec<u8>,
}

/// Properties by the column they belong to ("" for the table), then by
/// their name.
pub type Props = HashMap<String, HashMap<String, Prop>>;

/// Names are UTF-16 after Access 97, the code page before.
pub fn parse(b: &[u8], jet3: bool, codepage: u16) -> Props {
    let mut out = Props::new();
    if b.len() < 4 || !(&b[..4] == b"MR2\0" || &b[..4] == b"KKD\0") {
        return out;
    }
    let text = |d: &[u8]| {
        if jet3 {
            crate::codepage::decode(d, codepage)
        } else {
            String::from_utf16_lossy(&d.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>())
        }
    };
    let r = Rd(b);
    let mut names: Vec<String> = Vec::new();
    let mut pos = 4;
    while pos + 6 <= b.len() {
        let len = r.u32(pos) as usize;
        let ty = r.u16(pos + 4);
        let end = (pos + len).min(b.len());
        if len < 6 {
            break;
        }
        let mut p = pos + 6;
        if ty == 0x80 {
            while p + 2 <= end {
                let n = r.u16(p) as usize;
                names.push(text(r.bytes(p + 2, n)));
                p += 2 + n;
            }
        } else if p + 4 <= end {
            let head = r.u32(p) as usize;
            let map = if head > 6 { text(r.bytes(p + 6, r.u16(p + 4) as usize)) } else { String::new() };
            p += head.max(4);
            let entry = out.entry(map).or_default();
            while p + 8 <= end {
                let vlen = r.u16(p) as usize;
                if vlen < 8 {
                    break;
                }
                let dty = r.u8(p + 3);
                let idx = r.u16(p + 4) as usize;
                let size = r.u16(p + 6) as usize;
                if let Some(name) = names.get(idx) {
                    entry.insert(name.clone(), Prop { ty: dty, data: r.bytes(p + 8, size).to_vec() });
                }
                p += vlen;
            }
        }
        pos = end;
    }
    out
}

/// A property as text (text properties), or its number.
pub fn text(p: &Prop, jet3: bool, codepage: u16) -> String {
    let r = Rd(&p.data);
    match p.ty {
        0x01 | 0x02 => r.u8(0).to_string(),
        0x03 => (r.u16(0) as i16).to_string(),
        0x04 => (r.u32(0) as i32).to_string(),
        0x0a | 0x0c if jet3 => crate::codepage::decode(&p.data, codepage),
        0x0a | 0x0c => String::from_utf16_lossy(&p.data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>()),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
    }

    #[test]
    fn maps() {
        let mut b = b"MR2\0".to_vec();
        // Names.
        let mut names = Vec::new();
        for n in ["ResultType", "Description"] {
            names.extend((utf16(n).len() as u16).to_le_bytes());
            names.extend(utf16(n));
        }
        b.extend(((names.len() + 6) as u32).to_le_bytes());
        b.extend(0x80u16.to_le_bytes());
        b.extend(names);
        // The map of column "Total": ResultType = 7 (a byte).
        let col = utf16("Total");
        let mut m = Vec::new();
        m.extend(((col.len() + 6) as u32).to_le_bytes());
        m.extend((col.len() as u16).to_le_bytes());
        m.extend(col);
        m.extend([9u8, 0, 0, 2, 0, 0, 1, 0, 7]);
        b.extend(((m.len() + 6) as u32).to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(m);
        let p = parse(&b, false, 1252);
        let rt = &p["Total"]["ResultType"];
        assert_eq!(rt.data, vec![7]);
        assert_eq!(text(rt, false, 1252), "7");
    }
}
