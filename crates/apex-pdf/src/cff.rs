//! Minimal CFF (Compact Font Format) support: the GID to CID map of
//! CID-keyed fonts and a glyph-id preserving subsetter.
//!
//! The subsetter keeps the structure (and so every glyph id and CID) but
//! replaces unused charstrings with `endchar` and unused subroutines with
//! `return`, which is where nearly all the bytes of a CJK font are. Anything
//! it doesn't understand makes it give up, and the caller embeds the whole
//! CFF instead.

use std::collections::BTreeSet;
use std::ops::Range;

use crate::font::be16;

fn byte(d: &[u8], o: usize) -> Option<usize> {
    d.get(o).map(|&b| b as usize)
}

fn be_n(d: &[u8], o: usize, n: usize) -> Option<usize> {
    let b = d.get(o..o.checked_add(n)?)?;
    Some(b.iter().fold(0usize, |acc, &x| (acc << 8) | x as usize))
}

pub(crate) struct Index {
    start: usize,
    data_start: usize,
    offsets: Vec<usize>,
    pub end: usize,
}

impl Index {
    fn read(d: &[u8], pos: usize) -> Option<Index> {
        let count = be16(d, pos)? as usize;
        if count == 0 {
            return Some(Index { start: pos, data_start: pos + 2, offsets: Vec::new(), end: pos + 2 });
        }
        let off_size = byte(d, pos + 2)?;
        if !(1..=4).contains(&off_size) {
            return None;
        }
        let mut offsets = Vec::with_capacity(count + 1);
        for i in 0..=count {
            offsets.push(be_n(d, pos + 3 + i * off_size, off_size)?);
        }
        // Offsets are 1-based from the byte before the data.
        let data_start = pos + 3 + (count + 1) * off_size - 1;
        if offsets[0] != 1 || offsets.windows(2).any(|w| w[0] > w[1]) {
            return None;
        }
        let end = data_start.checked_add(offsets[count])?;
        if end > d.len() {
            return None;
        }
        Some(Index { start: pos, data_start, offsets, end })
    }

    fn count(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    fn item<'a>(&self, d: &'a [u8], i: usize) -> Option<&'a [u8]> {
        let (s, e) = (*self.offsets.get(i)?, *self.offsets.get(i + 1)?);
        d.get(self.data_start + s..self.data_start + e)
    }
}

fn write_index(items: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(items.len() as u16).to_be_bytes());
    if items.is_empty() {
        return out;
    }
    let total: usize = items.iter().map(|i| i.len()).sum::<usize>() + 1;
    let off_size = match total {
        0..=0xFF => 1,
        0x100..=0xFFFF => 2,
        0x1_0000..=0xFF_FFFF => 3,
        _ => 4,
    };
    out.push(off_size as u8);
    let mut off = 1usize;
    for i in 0..=items.len() {
        out.extend_from_slice(&off.to_be_bytes()[std::mem::size_of::<usize>() - off_size..]);
        if let Some(item) = items.get(i) {
            off += item.len();
        }
    }
    for item in items {
        out.extend_from_slice(item);
    }
    out
}

struct Entry {
    op: u16,
    operands: Vec<f64>,
    raw: Range<usize>,
}

fn parse_dict(d: &[u8]) -> Option<Vec<Entry>> {
    let mut out = Vec::new();
    let mut operands = Vec::new();
    let (mut i, mut start) = (0, 0);
    while i < d.len() {
        let b = d[i];
        match b {
            0..=21 => {
                let op = if b == 12 {
                    i += 1;
                    1200 + *d.get(i)? as u16
                } else {
                    b as u16
                };
                i += 1;
                out.push(Entry { op, operands: std::mem::take(&mut operands), raw: start..i });
                start = i;
            }
            28 => {
                operands.push(be16(d, i + 1)? as i16 as f64);
                i += 3;
            }
            29 => {
                operands.push(be_n(d, i + 1, 4)? as u32 as i32 as f64);
                i += 5;
            }
            30 => {
                let mut s = String::new();
                i += 1;
                'real: loop {
                    let b = *d.get(i)?;
                    i += 1;
                    for nib in [b >> 4, b & 15] {
                        match nib {
                            0..=9 => s.push((b'0' + nib) as char),
                            0xa => s.push('.'),
                            0xb => s.push('E'),
                            0xc => s.push_str("E-"),
                            0xe => s.push('-'),
                            0xf => break 'real,
                            _ => {}
                        }
                    }
                }
                operands.push(s.parse().unwrap_or(0.0));
            }
            32..=246 => {
                operands.push(b as f64 - 139.0);
                i += 1;
            }
            247..=250 => {
                operands.push(((b as f64 - 247.0) * 256.0) + byte(d, i + 1)? as f64 + 108.0);
                i += 2;
            }
            251..=254 => {
                operands.push(-((b as f64 - 251.0) * 256.0) - byte(d, i + 1)? as f64 - 108.0);
                i += 2;
            }
            _ => return None,
        }
    }
    Some(out)
}

fn op<'a>(dict: &'a [Entry], code: u16) -> Option<&'a [f64]> {
    dict.iter().find(|e| e.op == code).map(|e| e.operands.as_slice())
}

fn op_usize(dict: &[Entry], code: u16, i: usize) -> Option<usize> {
    let v = *op(dict, code)?.get(i)?;
    if v >= 0.0 && v.is_finite() { Some(v as usize) } else { None }
}

/// Re-encodes a DICT, replacing the operands of the given operators with
/// fixed-size (5 byte) integers so offsets can be patched without the dict
/// changing size.
fn rewrite_dict(raw: &[u8], dict: &[Entry], repl: &[(u16, Vec<usize>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for e in dict {
        match repl.iter().find(|(o, _)| *o == e.op) {
            Some((o, vals)) => {
                for v in vals {
                    out.push(29);
                    out.extend_from_slice(&(*v as u32).to_be_bytes());
                }
                if *o >= 1200 {
                    out.extend_from_slice(&[12, (*o - 1200) as u8]);
                } else {
                    out.push(*o as u8);
                }
            }
            None => out.extend_from_slice(raw.get(e.raw.clone()).unwrap_or(&[])),
        }
    }
    out
}

pub(crate) struct Cff<'a> {
    d: &'a [u8],
    hdr_size: usize,
    name: Index,
    top_raw: &'a [u8],
    top: Vec<Entry>,
    strings: Index,
    gsubrs: Index,
    charstrings: Index,
    is_cid: bool,
}

impl<'a> Cff<'a> {
    pub fn parse(d: &'a [u8]) -> Option<Cff<'a>> {
        if byte(d, 0)? != 1 {
            return None;
        }
        let hdr_size = byte(d, 2)?;
        let name = Index::read(d, hdr_size)?;
        let top_index = Index::read(d, name.end)?;
        let top_raw = top_index.item(d, 0)?;
        let top = parse_dict(top_raw)?;
        let strings = Index::read(d, top_index.end)?;
        let gsubrs = Index::read(d, strings.end)?;
        let charstrings = Index::read(d, op_usize(&top, 17, 0)?)?;
        let is_cid = op(&top, 1230).is_some();
        Some(Cff { d, hdr_size, name, top_raw, top, strings, gsubrs, charstrings, is_cid })
    }

    fn num_glyphs(&self) -> usize {
        self.charstrings.count()
    }

    fn charset_off(&self) -> usize {
        op_usize(&self.top, 15, 0).unwrap_or(0)
    }

    /// Walks a custom charset; returns the GID to SID/CID table and the end
    /// offset of the charset data.
    fn charset(&self) -> Option<(Vec<u16>, usize)> {
        let (d, off, n) = (self.d, self.charset_off(), self.num_glyphs());
        let mut out = vec![0u16; n.max(1)];
        let format = byte(d, off)?;
        let mut pos = off + 1;
        let mut gid = 1;
        match format {
            0 => {
                while gid < n {
                    out[gid] = be16(d, pos)?;
                    pos += 2;
                    gid += 1;
                }
            }
            1 | 2 => {
                while gid < n {
                    let first = be16(d, pos)? as usize;
                    let left = if format == 1 { byte(d, pos + 2)? } else { be16(d, pos + 2)? as usize };
                    pos += if format == 1 { 3 } else { 4 };
                    for k in 0..=left {
                        if gid >= n {
                            break;
                        }
                        out[gid] = (first + k).min(u16::MAX as usize) as u16;
                        gid += 1;
                    }
                }
            }
            _ => return None,
        }
        Some((out, pos))
    }

    /// GID to CID for CID-keyed fonts (None: CID == GID).
    pub fn gid_to_cid(&self) -> Option<Vec<u16>> {
        if !self.is_cid || self.charset_off() <= 2 {
            return None;
        }
        let (map, _) = self.charset()?;
        let identity = map.iter().enumerate().all(|(g, &c)| g == c as usize);
        if identity { None } else { Some(map) }
    }

    fn fd_select_len(&self, off: usize) -> Option<usize> {
        match byte(self.d, off)? {
            0 => Some(1 + self.num_glyphs()),
            3 => Some(3 + 3 * be16(self.d, off + 1)? as usize + 2),
            _ => None,
        }
    }

    fn fd_of(&self, off: usize, gid: usize) -> Option<usize> {
        let d = self.d;
        match byte(d, off)? {
            0 => byte(d, off + 1 + gid),
            3 => {
                let n = be16(d, off + 1)? as usize;
                let mut fd = None;
                for r in 0..n {
                    let p = off + 3 + 3 * r;
                    if be16(d, p)? as usize > gid {
                        break;
                    }
                    fd = byte(d, p + 2);
                }
                fd
            }
            _ => None,
        }
    }

    fn encoding_len(&self, off: usize) -> Option<usize> {
        let d = self.d;
        let f = byte(d, off)?;
        let mut len = match f & 0x7f {
            0 => 2 + byte(d, off + 1)?,
            1 => 2 + 2 * byte(d, off + 1)?,
            _ => return None,
        };
        if f & 0x80 != 0 {
            len += 1 + 3 * byte(d, off + len)?;
        }
        Some(len)
    }
}

struct Private<'a> {
    raw: &'a [u8],
    dict: Vec<Entry>,
    subrs: Option<Index>,
}

fn read_private<'a>(d: &'a [u8], size: usize, off: usize) -> Option<Private<'a>> {
    let raw = d.get(off..off.checked_add(size)?)?;
    let dict = parse_dict(raw)?;
    let subrs = match op_usize(&dict, 19, 0) {
        Some(rel) => Some(Index::read(d, off.checked_add(rel)?)?),
        None => None,
    };
    Some(Private { raw, dict, subrs })
}

fn bias(count: usize) -> i64 {
    if count < 1240 {
        107
    } else if count < 33900 {
        1131
    } else {
        32768
    }
}

/// Type 2 charstring walker that only records which subroutines are called.
struct Scan<'a> {
    d: &'a [u8],
    gsubrs: &'a Index,
    lsubrs: Option<&'a Index>,
    gused: &'a mut [bool],
    lused: &'a mut [bool],
    stack: Vec<f64>,
    stems: usize,
    done: bool,
    /// Bytes all the glyphs may still read: subroutines calling others
    /// many times over would take forever (the whole table is kept then).
    work: usize,
}

impl Scan<'_> {
    fn run(&mut self, cs: &[u8], depth: u8) -> Option<()> {
        if depth > 10 {
            return None;
        }
        let mut i = 0;
        while i < cs.len() && !self.done {
            self.work = self.work.checked_sub(1)?;
            let b = cs[i];
            match b {
                28 => {
                    self.stack.push(be16(cs, i + 1)? as i16 as f64);
                    i += 3;
                }
                32..=246 => {
                    self.stack.push(b as f64 - 139.0);
                    i += 1;
                }
                247..=250 => {
                    self.stack.push((b as f64 - 247.0) * 256.0 + *cs.get(i + 1)? as f64 + 108.0);
                    i += 2;
                }
                251..=254 => {
                    self.stack.push(-(b as f64 - 251.0) * 256.0 - *cs.get(i + 1)? as f64 - 108.0);
                    i += 2;
                }
                255 => {
                    self.stack.push(be_n(cs, i + 1, 4)? as u32 as i32 as f64 / 65536.0);
                    i += 5;
                }
                1 | 3 | 18 | 23 => {
                    self.stems += self.stack.len() / 2;
                    self.stack.clear();
                    i += 1;
                }
                19 | 20 => {
                    // Implicit vstems before the mask, then the mask bytes.
                    self.stems += self.stack.len() / 2;
                    self.stack.clear();
                    i += 1 + self.stems.div_ceil(8);
                }
                10 | 29 => {
                    let n = self.stack.pop()? as i64;
                    let (index, used): (&Index, &mut [bool]) = if b == 10 {
                        (self.lsubrs?, &mut *self.lused)
                    } else {
                        (self.gsubrs, &mut *self.gused)
                    };
                    let k = usize::try_from(n + bias(index.count())).ok()?;
                    *used.get_mut(k)? = true;
                    let sub = index.item(self.d, k)?;
                    self.run(sub, depth + 1)?;
                    i += 1;
                }
                11 => return Some(()),
                14 => {
                    self.done = true;
                    return Some(());
                }
                12 => {
                    self.stack.clear();
                    i += 2;
                }
                _ => {
                    self.stack.clear();
                    i += 1;
                }
            }
        }
        Some(())
    }
}

/// Glyph-id preserving subset of a CFF table (see the module docs).
pub(crate) fn subset(d: &[u8], used: &BTreeSet<u16>) -> Option<Vec<u8>> {
    let c = Cff::parse(d)?;
    if op(&c.top, 1206).is_some_and(|v| v.first() != Some(&2.0)) {
        return None;
    }
    let n = c.num_glyphs();

    // Private dicts: one per Font DICT for CID fonts, one otherwise.
    let fdarray = match op_usize(&c.top, 1236, 0) {
        Some(off) if c.is_cid => Some(Index::read(d, off)?),
        _ => None,
    };
    let mut font_dicts: Vec<(&[u8], Vec<Entry>)> = Vec::new();
    let mut privates: Vec<Private> = Vec::new();
    if let Some(fda) = &fdarray {
        for i in 0..fda.count() {
            let raw = fda.item(d, i)?;
            let dict = parse_dict(raw)?;
            let p = read_private(d, op_usize(&dict, 18, 0)?, op_usize(&dict, 18, 1)?)?;
            font_dicts.push((raw, dict));
            privates.push(p);
        }
    } else if c.is_cid {
        return None;
    } else if let (Some(size), Some(off)) = (op_usize(&c.top, 18, 0), op_usize(&c.top, 18, 1)) {
        privates.push(read_private(d, size, off)?);
    }
    let fd_select = if c.is_cid { Some(op_usize(&c.top, 1237, 0)?) } else { None };

    // Find the subroutines the used glyphs need.
    let mut work: usize = 1 << 24;
    let mut gused = vec![false; c.gsubrs.count()];
    let mut lused: Vec<Vec<bool>> =
        privates.iter().map(|p| vec![false; p.subrs.as_ref().map_or(0, |s| s.count())]).collect();
    for &gid in used {
        let gid = gid as usize;
        if gid >= n {
            continue;
        }
        let fd = match fd_select {
            Some(off) => c.fd_of(off, gid)?,
            None => 0,
        };
        let cs = c.charstrings.item(d, gid)?;
        let mut empty: Vec<bool> = Vec::new();
        let (lsubrs, lu) = match (privates.get(fd), lused.get_mut(fd)) {
            (Some(p), Some(lu)) => (p.subrs.as_ref(), lu.as_mut_slice()),
            _ => (None, empty.as_mut_slice()),
        };
        let mut scan = Scan { d, gsubrs: &c.gsubrs, lsubrs, gused: &mut gused, lused: lu, stack: Vec::new(), stems: 0, done: false, work };
        scan.run(cs, 0)?;
        work = scan.work;
    }

    let filter = |index: &Index, keep: &dyn Fn(usize) -> bool, filler: &'static [u8]| -> Option<Vec<u8>> {
        let items: Option<Vec<&[u8]>> =
            (0..index.count()).map(|i| if keep(i) { index.item(d, i) } else { Some(filler) }).collect();
        Some(write_index(&items?))
    };
    let gsubrs = filter(&c.gsubrs, &|i| gused[i], &[11])?;
    let charstrings = filter(&c.charstrings, &|i| used.contains(&(i as u16)), &[14])?;
    // Each private dict with its local subrs right after it (Subrs offset =
    // dict size, which is fixed thanks to the 5-byte operand).
    let mut private_blobs = Vec::new();
    for (p, lu) in privates.iter().zip(&lused) {
        let blob = match &p.subrs {
            Some(s) => {
                let size = rewrite_dict(p.raw, &p.dict, &[(19, vec![0])]).len();
                let mut blob = rewrite_dict(p.raw, &p.dict, &[(19, vec![size])]);
                blob.extend(filter(s, &|i| lu[i], &[11])?);
                (blob, size)
            }
            None => (p.raw.to_vec(), p.raw.len()),
        };
        private_blobs.push(blob);
    }

    // Raw pieces copied as they are.
    let charset = match c.charset_off() {
        off if off > 2 => d.get(off..c.charset()?.1)?,
        _ => &[][..],
    };
    let fdsel = match fd_select {
        Some(off) => d.get(off..off + c.fd_select_len(off)?)?,
        None => &[][..],
    };
    let encoding = match op_usize(&c.top, 16, 0) {
        Some(off) if off > 1 && !c.is_cid => d.get(off..off + c.encoding_len(off)?)?,
        _ => &[][..],
    };

    // Layout: header, name, top dict, strings, gsubrs, charset, fdselect,
    // encoding, charstrings, fdarray, private dicts (+ local subrs).
    let top_ops: Vec<u16> = [15, 16, 17, 18, 1236, 1237].into_iter().filter(|&o| op(&c.top, o).is_some()).collect();
    let top_with = |vals: &dyn Fn(u16) -> Vec<usize>| {
        let repl: Vec<(u16, Vec<usize>)> = top_ops
            .iter()
            .filter(|&&o| o != 16 || !encoding.is_empty())
            .filter(|&&o| o != 15 || !charset.is_empty())
            .map(|&o| (o, vals(o)))
            .collect();
        rewrite_dict(c.top_raw, &c.top, &repl)
    };
    let zeros = |o: u16| if o == 18 { vec![0, 0] } else { vec![0] };
    let top_len = write_index(&[&top_with(&zeros)]).len();
    let name = d.get(c.name.start..c.name.end)?;
    let strings = d.get(c.strings.start..c.strings.end)?;
    let mut pos = c.hdr_size + name.len() + top_len + strings.len() + gsubrs.len();
    let charset_at = pos;
    pos += charset.len();
    let fdsel_at = pos;
    pos += fdsel.len();
    let enc_at = pos;
    pos += encoding.len();
    let cs_at = pos;
    pos += charstrings.len();
    let fdarray_at = pos;
    let fd_index_len = if fdarray.is_some() {
        let dummy: Vec<Vec<u8>> = font_dicts.iter().map(|(raw, dict)| rewrite_dict(raw, dict, &[(18, vec![0, 0])])).collect();
        write_index(&dummy.iter().map(|v| v.as_slice()).collect::<Vec<_>>()).len()
    } else {
        0
    };
    pos += fd_index_len;
    let mut priv_at = Vec::new();
    for (blob, _) in &private_blobs {
        priv_at.push(pos);
        pos += blob.len();
    }
    let fdarray_bytes = if fdarray.is_some() {
        let dicts: Vec<Vec<u8>> = font_dicts
            .iter()
            .zip(priv_at.iter().zip(&private_blobs))
            .map(|((raw, dict), (&at, (_, len)))| rewrite_dict(raw, dict, &[(18, vec![*len, at])]))
            .collect();
        write_index(&dicts.iter().map(|v| v.as_slice()).collect::<Vec<_>>())
    } else {
        Vec::new()
    };
    let top = top_with(&|o| match o {
        15 => vec![charset_at],
        16 => vec![enc_at],
        17 => vec![cs_at],
        18 => match (priv_at.first(), private_blobs.first()) {
            (Some(&at), Some((_, len))) => vec![*len, at],
            _ => vec![0, 0],
        },
        1236 => vec![fdarray_at],
        _ => vec![fdsel_at],
    });

    let mut out = Vec::with_capacity(pos);
    out.extend_from_slice(d.get(..c.hdr_size)?);
    out.extend_from_slice(name);
    out.extend(write_index(&[&top]));
    out.extend_from_slice(strings);
    out.extend(gsubrs);
    out.extend_from_slice(charset);
    out.extend_from_slice(fdsel);
    out.extend_from_slice(encoding);
    out.extend(charstrings);
    out.extend(fdarray_bytes);
    for (blob, _) in private_blobs {
        out.extend(blob);
    }
    // Sanity check: the result must parse back with the same glyph count.
    if out.len() != pos || Cff::parse(&out)?.num_glyphs() != n {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_round_trip() {
        let items: [&[u8]; 3] = [b"a", b"", b"xyz"];
        let data = write_index(&items);
        let idx = Index::read(&data, 0).unwrap();
        assert_eq!(idx.count(), 3);
        assert_eq!(idx.item(&data, 2), Some(&b"xyz"[..]));
        assert_eq!(idx.end, data.len());
        assert_eq!(Index::read(&write_index(&[]), 0).unwrap().end, 2);
    }

    #[test]
    fn dict_parsing_and_rewrite() {
        // 100 200 Private (18), -5 1.5 BlueScale-ish (12 9)
        let raw = [239u8, 247, 92, 18, 134, 30, 0x1a, 0x5f, 12, 9];
        let d = parse_dict(&raw).unwrap();
        assert_eq!(op(&d, 18), Some(&[100.0, 200.0][..]));
        assert_eq!(op(&d, 1209), Some(&[-5.0, 1.5][..]));
        let out = rewrite_dict(&raw, &d, &[(18, vec![7, 70000])]);
        let d2 = parse_dict(&out).unwrap();
        assert_eq!(op(&d2, 18), Some(&[7.0, 70000.0][..]));
        assert_eq!(op(&d2, 1209), Some(&[-5.0, 1.5][..]));
    }

    #[test]
    fn garbage_never_panics() {
        let used = BTreeSet::from([0, 1, 2]);
        for seed in 0..200u32 {
            let data: Vec<u8> = (0..300u32).map(|i| (i.wrapping_mul(2654435761).wrapping_add(seed * 7919) >> 13) as u8).collect();
            let mut d = data.clone();
            d[0] = 1;
            d[2] = 4;
            let _ = subset(&d, &used);
            if let Some(c) = Cff::parse(&d) {
                let _ = c.gid_to_cid();
            }
        }
    }
}
