//! TrueType / OpenType parsing: face selection in collections, cmap lookup,
//! metrics for the PDF font descriptor and subsetting for embedding.
//!
//! Every read is bounds checked: fonts come from the user's system and may
//! be truncated or malformed, and a bad font must never bring the app down.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::sync::Arc;

use crate::cff;

pub(crate) fn be16(d: &[u8], o: usize) -> Option<u16> {
    let b = d.get(o..o.checked_add(2)?)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}

pub(crate) fn be32(d: &[u8], o: usize) -> Option<u32> {
    let b = d.get(o..o.checked_add(4)?)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn i16_at(d: &[u8], o: usize) -> i16 {
    be16(d, o).unwrap_or(0) as i16
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Outlines {
    Glyf,
    Cff,
}

struct Cmap {
    range: Range<usize>,
    format: u16,
    /// Microsoft Symbol encoding: characters live at U+F000..U+F0FF.
    symbol: bool,
}

/// Font metrics for the PDF FontDescriptor, in 1/1000 em.
#[derive(Clone, Debug)]
pub(crate) struct Metrics {
    pub ascent: f32,
    pub descent: f32,
    pub cap_height: f32,
    pub bbox: [f32; 4],
    pub italic_angle: f32,
    pub flags: u32,
    pub stem_v: f32,
}

pub(crate) struct Font {
    pub data: Arc<[u8]>,
    pub index: u32,
    tables: Vec<([u8; 4], Range<usize>)>,
    pub outlines: Outlines,
    upem: u16,
    pub num_glyphs: u16,
    n_hmetrics: u16,
    cmap: Option<Cmap>,
    pub name: String,
    pub metrics: Metrics,
    /// GID to CID for CID-keyed CFF fonts (the PDF viewer maps CIDs through
    /// the CFF charset); None means CID == GID.
    cff_cids: Option<Vec<u16>>,
    cache: RefCell<HashMap<char, u16>>,
}

impl Font {
    pub fn parse(data: Arc<[u8]>, index: u32) -> Option<Font> {
        let d: &[u8] = &data;
        let base = if d.get(0..4)? == b"ttcf" {
            let n = be32(d, 8)?;
            if index >= n {
                return None;
            }
            be32(d, 12 + 4 * index as usize)? as usize
        } else {
            0
        };
        let version = be32(d, base)?;
        if !matches!(version, 0x0001_0000 | 0x7472_7565 | 0x4F54_544F) {
            return None;
        }
        let num = be16(d, base + 4)? as usize;
        let mut tables = Vec::with_capacity(num);
        for i in 0..num {
            let rec = base + 12 + 16 * i;
            let Some(tag) = d.get(rec..rec + 4) else { break };
            let (Some(off), Some(len)) = (be32(d, rec + 8), be32(d, rec + 12)) else { break };
            let (off, len) = (off as usize, len as usize);
            // Tables that point outside the file are ignored, not trusted.
            if off.checked_add(len).is_some_and(|end| end <= d.len()) {
                tables.push(([tag[0], tag[1], tag[2], tag[3]], off..off + len));
            }
        }
        let mut font = Font {
            data: data.clone(),
            index,
            tables,
            outlines: Outlines::Glyf,
            upem: 1000,
            num_glyphs: 0,
            n_hmetrics: 0,
            cmap: None,
            name: String::new(),
            metrics: Metrics {
                ascent: 0.0,
                descent: 0.0,
                cap_height: 0.0,
                bbox: [0.0; 4],
                italic_angle: 0.0,
                flags: 4,
                stem_v: 80.0,
            },
            cff_cids: None,
            cache: RefCell::new(HashMap::new()),
        };
        let (head, hhea, maxp) = (font.table(b"head").to_vec(), font.table(b"hhea").to_vec(), font.table(b"maxp").to_vec());
        if head.len() < 54 || hhea.len() < 36 || maxp.len() < 6 || font.table(b"cmap").is_empty() {
            return None;
        }
        font.outlines = if !font.table(b"glyf").is_empty() && !font.table(b"loca").is_empty() {
            Outlines::Glyf
        } else if !font.table(b"CFF ").is_empty() {
            Outlines::Cff
        } else {
            return None;
        };
        let upem = be16(&head, 18)?;
        font.upem = if (16..=16384).contains(&upem) { upem } else { 1000 };
        font.num_glyphs = be16(&maxp, 4)?;
        if font.num_glyphs == 0 {
            return None;
        }
        let hm_len = font.table(b"hmtx").len() / 4;
        font.n_hmetrics = be16(&hhea, 34)?.min(hm_len.min(u16::MAX as usize) as u16);
        font.cmap = font.pick_cmap();
        font.cmap.as_ref()?;
        font.name = font.ps_name();
        font.metrics = font.compute_metrics();
        if font.outlines == Outlines::Cff {
            let info = cff::Cff::parse(font.table(b"CFF "))?;
            font.cff_cids = info.gid_to_cid();
        }
        Some(font)
    }

    pub fn table(&self, tag: &[u8; 4]) -> &[u8] {
        self.tables
            .iter()
            .find(|(t, _)| t == tag)
            .and_then(|(_, r)| self.data.get(r.clone()))
            .unwrap_or(&[])
    }

    fn scale(&self) -> f32 {
        1000.0 / self.upem as f32
    }

    fn pick_cmap(&self) -> Option<Cmap> {
        let cmap = self.table(b"cmap");
        let cmap_start = self.tables.iter().find(|(t, _)| t == b"cmap")?.1.start;
        let n = be16(cmap, 2)? as usize;
        let mut best: Option<(u8, Cmap)> = None;
        for i in 0..n {
            let rec = 4 + 8 * i;
            let (Some(pid), Some(eid), Some(off)) = (be16(cmap, rec), be16(cmap, rec + 2), be32(cmap, rec + 4))
            else {
                break;
            };
            let off = off as usize;
            let Some(format) = be16(cmap, off) else { continue };
            let len = match format {
                0 | 4 | 6 => be16(cmap, off + 2).map(|l| l as usize),
                12 => be32(cmap, off + 4).map(|l| l as usize),
                _ => None,
            };
            let Some(len) = len else { continue };
            // Some fonts have a wrong length in format 4; clamp to the table.
            let end = (off + len).min(cmap.len());
            let score = match (pid, eid, format) {
                (3, 10, 12) | (0, 4, 12) | (0, 6, 12) => 6,
                (0, _, 12) => 5,
                (3, 1, 4) => 4,
                (0, _, 4) => 3,
                (3, 0, 4) => 2,
                (1, 0, 0 | 6) => 1,
                (0, _, 0 | 6) | (3, 1, 6) => 1,
                _ => 0,
            };
            if score == 0 {
                continue;
            }
            if best.as_ref().is_none_or(|(s, _)| score > *s) {
                let sub = Cmap { range: cmap_start + off..cmap_start + end, format, symbol: (pid, eid) == (3, 0) };
                best = Some((score, sub));
            }
        }
        best.map(|(_, c)| c)
    }

    fn lookup(&self, c: u32) -> u16 {
        let Some(cm) = &self.cmap else { return 0 };
        let Some(t) = self.data.get(cm.range.clone()) else { return 0 };
        let gid = match cm.format {
            0 => {
                if c < 256 {
                    t.get(6 + c as usize).copied().unwrap_or(0) as u16
                } else {
                    0
                }
            }
            6 => {
                let first = be16(t, 6).unwrap_or(0) as u32;
                let count = be16(t, 8).unwrap_or(0) as u32;
                if c >= first && c - first < count {
                    be16(t, 10 + 2 * (c - first) as usize).unwrap_or(0)
                } else {
                    0
                }
            }
            4 => cmap4(t, c).unwrap_or(0),
            12 => cmap12(t, c).unwrap_or(0),
            _ => 0,
        };
        if gid >= self.num_glyphs { 0 } else { gid }
    }

    pub fn glyph_id(&self, c: char) -> u16 {
        if let Ok(cache) = self.cache.try_borrow()
            && let Some(&g) = cache.get(&c)
        {
            return g;
        }
        let mut g = self.lookup(c as u32);
        if g == 0 && self.cmap.as_ref().is_some_and(|m| m.symbol) && (c as u32) < 0x100 {
            g = self.lookup(0xF000 + c as u32);
        }
        if let Ok(mut cache) = self.cache.try_borrow_mut() {
            cache.insert(c, g);
        }
        g
    }

    pub fn has_char(&self, c: char) -> bool {
        self.glyph_id(c) != 0
    }

    fn advance_units(&self, gid: u16) -> u16 {
        if self.n_hmetrics == 0 {
            return 0;
        }
        let i = gid.min(self.n_hmetrics - 1) as usize;
        be16(self.table(b"hmtx"), 4 * i).unwrap_or(0)
    }

    /// Advance in 1/1000 em, rounded to 0.1 so the value written in /W and
    /// the one used for TJ adjustments are exactly the same.
    pub fn width(&self, gid: u16) -> f32 {
        (self.advance_units(gid) as f32 * self.scale() * 10.0).round() / 10.0
    }

    /// Code (CID) under which a glyph is shown for CFF fonts.
    pub fn cff_cid(&self, gid: u16) -> u16 {
        match &self.cff_cids {
            Some(map) => map.get(gid as usize).copied().unwrap_or(0),
            None => gid,
        }
    }

    fn ps_name(&self) -> String {
        let t = self.table(b"name");
        let count = be16(t, 2).unwrap_or(0) as usize;
        let storage = be16(t, 4).unwrap_or(0) as usize;
        let mut best: Option<(u8, String)> = None;
        for i in 0..count {
            let r = 6 + 12 * i;
            let (Some(pid), Some(nid), Some(len), Some(off)) = (be16(t, r), be16(t, r + 6), be16(t, r + 8), be16(t, r + 10))
            else {
                break;
            };
            let rank = match nid {
                6 => 3,
                4 => 2,
                1 => 1,
                _ => continue,
            };
            let start = storage + off as usize;
            let Some(bytes) = t.get(start..start + len as usize) else { continue };
            let s: String = match pid {
                0 | 3 => {
                    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                    String::from_utf16_lossy(&units)
                }
                1 => bytes.iter().map(|&b| b as char).collect(),
                _ => continue,
            };
            let clean: String = s.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(56).collect();
            if !clean.is_empty() && best.as_ref().is_none_or(|(r, _)| rank > *r) {
                best = Some((rank, clean));
            }
        }
        best.map(|(_, s)| s).unwrap_or_else(|| "Font".to_string())
    }

    fn compute_metrics(&self) -> Metrics {
        let s = self.scale();
        let head = self.table(b"head");
        let hhea = self.table(b"hhea");
        let os2 = self.table(b"OS/2");
        let post = self.table(b"post");
        let bbox = [i16_at(head, 36), i16_at(head, 38), i16_at(head, 40), i16_at(head, 42)].map(|v| v as f32 * s);
        let mut ascent = i16_at(hhea, 4) as f32;
        let mut descent = i16_at(hhea, 6) as f32;
        if ascent == 0.0 && descent == 0.0 && os2.len() >= 78 {
            ascent = be16(os2, 74).unwrap_or(0) as f32;
            descent = -(be16(os2, 76).unwrap_or(0) as f32);
        }
        let cap = if os2.len() >= 90 && be16(os2, 0).unwrap_or(0) >= 2 { i16_at(os2, 88) as f32 } else { 0.0 };
        let cap_height = if cap > 0.0 { cap * s } else { ascent * s * 0.7 };
        let italic_angle = be32(post, 4).map(|v| v as i32 as f32 / 65536.0).unwrap_or(0.0);
        let fixed = be32(post, 12).unwrap_or(0) != 0;
        let mac_style = be16(head, 44).unwrap_or(0);
        let fs_sel = be16(os2, 62).unwrap_or(0);
        let family_class = be16(os2, 30).unwrap_or(0) >> 8;
        let mut flags = 4; // symbolic: glyphs are addressed by CID, not a standard encoding
        if fixed {
            flags |= 1;
        }
        if matches!(family_class, 1..=5 | 7) {
            flags |= 2;
        }
        if italic_angle != 0.0 || fs_sel & 1 != 0 || mac_style & 2 != 0 {
            flags |= 64;
        }
        let mut weight = be16(os2, 4).unwrap_or(0) as f32;
        if weight <= 0.0 {
            weight = if mac_style & 1 != 0 { 700.0 } else { 400.0 };
        }
        let stem_v = 10.0 + 220.0 * ((weight - 50.0) / 900.0).powi(2);
        Metrics {
            ascent: ascent * s,
            descent: descent * s,
            cap_height,
            bbox,
            italic_angle: if italic_angle.is_finite() { italic_angle } else { 0.0 },
            flags,
            stem_v,
        }
    }

    /// hmtx with the metrics of unused glyphs zeroed (they compress away).
    fn subset_hmtx(&self, used: &BTreeSet<u16>) -> Vec<u8> {
        let mut hmtx = self.table(b"hmtx").to_vec();
        let nh = self.n_hmetrics as usize;
        for g in 0..self.num_glyphs as usize {
            if used.contains(&(g as u16)) {
                continue;
            }
            let range = if g < nh { 4 * g..4 * g + 4 } else { 4 * nh + 2 * (g - nh)..4 * nh + 2 * (g - nh) + 2 };
            if let Some(b) = hmtx.get_mut(range) {
                b.fill(0);
            }
        }
        hmtx
    }

    fn glyph_range(&self, gid: u16) -> Range<usize> {
        let loca = self.table(b"loca");
        let glyf_len = self.table(b"glyf").len();
        let long = i16_at(self.table(b"head"), 50) == 1;
        let g = gid as usize;
        let (s, e) = if long {
            (be32(loca, 4 * g).map(|v| v as usize), be32(loca, 4 * g + 4).map(|v| v as usize))
        } else {
            (be16(loca, 2 * g).map(|v| v as usize * 2), be16(loca, 2 * g + 2).map(|v| v as usize * 2))
        };
        match (s, e) {
            (Some(s), Some(e)) if s < e && e <= glyf_len => s..e,
            _ => 0..0,
        }
    }

    /// Glyph ids referenced by a composite glyph.
    fn components(&self, gid: u16) -> Vec<u16> {
        let glyf = self.table(b"glyf");
        let Some(g) = glyf.get(self.glyph_range(gid)) else { return Vec::new() };
        let mut out = Vec::new();
        if g.len() < 10 || i16_at(g, 0) >= 0 {
            return out;
        }
        let mut pos = 10;
        while let (Some(flags), Some(comp)) = (be16(g, pos), be16(g, pos + 2)) {
            out.push(comp);
            pos += 4 + if flags & 1 != 0 { 4 } else { 2 };
            pos += if flags & 8 != 0 {
                2
            } else if flags & 0x40 != 0 {
                4
            } else if flags & 0x80 != 0 {
                8
            } else {
                0
            };
            if flags & 0x20 == 0 || out.len() > 256 {
                break;
            }
        }
        out
    }

    /// Standalone TrueType font keeping glyph ids but with every glyph not in
    /// `used` (nor a component of one) emptied.
    pub fn subset_glyf(&self, used: &BTreeSet<u16>) -> Vec<u8> {
        let mut keep: BTreeSet<u16> = BTreeSet::new();
        let mut todo: Vec<u16> = used.iter().copied().chain([0]).collect();
        while let Some(g) = todo.pop() {
            if g < self.num_glyphs && keep.insert(g) {
                todo.extend(self.components(g));
            }
        }
        let glyf = self.table(b"glyf");
        let mut new_glyf = Vec::new();
        let mut offsets = Vec::with_capacity(self.num_glyphs as usize + 1);
        // Glyphs may share bytes: no more copied than the table, twice.
        let most = glyf.len().saturating_mul(2).saturating_add(4096);
        for g in 0..self.num_glyphs {
            offsets.push(new_glyf.len());
            if keep.contains(&g)
                && let Some(bytes) = glyf.get(self.glyph_range(g))
                && new_glyf.len() + bytes.len() <= most
            {
                new_glyf.extend_from_slice(bytes);
                while new_glyf.len() % 4 != 0 {
                    new_glyf.push(0);
                }
            }
        }
        offsets.push(new_glyf.len());
        let short = new_glyf.len() / 2 <= u16::MAX as usize;
        let mut loca = Vec::with_capacity(offsets.len() * if short { 2 } else { 4 });
        for o in offsets {
            if short {
                loca.extend_from_slice(&((o / 2) as u16).to_be_bytes());
            } else {
                loca.extend_from_slice(&(o as u32).to_be_bytes());
            }
        }
        let mut head = self.table(b"head").to_vec();
        head[50..52].copy_from_slice(&(if short { 0u16 } else { 1u16 }).to_be_bytes());
        let mut tables: Vec<([u8; 4], Vec<u8>)> = vec![
            (*b"head", head),
            (*b"hhea", self.table(b"hhea").to_vec()),
            (*b"hmtx", self.subset_hmtx(&keep)),
            (*b"loca", loca),
            (*b"glyf", new_glyf),
            (*b"maxp", self.table(b"maxp").to_vec()),
        ];
        for tag in [b"cvt ", b"fpgm", b"prep"] {
            let t = self.table(tag);
            if !t.is_empty() {
                tables.push((*tag, t.to_vec()));
            }
        }
        build_sfnt(0x0001_0000, tables)
    }

    /// Standalone OpenType-CFF font for FontFile3, with the CFF subset when
    /// it can be parsed (the whole CFF otherwise).
    pub fn subset_cff(&self, used: &BTreeSet<u16>) -> Vec<u8> {
        let mut keep = used.clone();
        keep.insert(0);
        let cff_data = self.table(b"CFF ");
        let cff_new = cff::subset(cff_data, &keep).unwrap_or_else(|| cff_data.to_vec());
        let mut tables: Vec<([u8; 4], Vec<u8>)> = vec![
            (*b"CFF ", cff_new),
            (*b"head", self.table(b"head").to_vec()),
            (*b"hhea", self.table(b"hhea").to_vec()),
            (*b"hmtx", self.subset_hmtx(&keep)),
            (*b"maxp", self.table(b"maxp").to_vec()),
        ];
        for tag in [b"OS/2", b"post", b"name"] {
            let t = self.table(tag);
            if !t.is_empty() {
                tables.push((*tag, t.to_vec()));
            }
        }
        build_sfnt(0x4F54_544F, tables)
    }
}

fn cmap4(t: &[u8], c: u32) -> Option<u16> {
    if c > 0xFFFF {
        return None;
    }
    let seg2 = be16(t, 6)? as usize;
    let segs = seg2 / 2;
    let ends = 14;
    let starts = ends + seg2 + 2;
    let deltas = starts + seg2;
    let ranges = deltas + seg2;
    // Binary search for the first segment whose end code is >= c.
    let (mut lo, mut hi) = (0usize, segs);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if (be16(t, ends + 2 * mid)? as u32) < c {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo >= segs {
        return None;
    }
    let start = be16(t, starts + 2 * lo)? as u32;
    if start > c {
        return None;
    }
    let delta = be16(t, deltas + 2 * lo)?;
    let ro = be16(t, ranges + 2 * lo)? as usize;
    if ro == 0 {
        return Some((c as u16).wrapping_add(delta));
    }
    let addr = ranges + 2 * lo + ro + 2 * (c - start) as usize;
    let g = be16(t, addr)?;
    if g == 0 { None } else { Some(g.wrapping_add(delta)) }
}

fn cmap12(t: &[u8], c: u32) -> Option<u16> {
    let n = be32(t, 12)? as usize;
    let (mut lo, mut hi) = (0usize, n);
    while lo < hi {
        let mid = (lo + hi) / 2;
        let g = 16 + 12 * mid;
        let (start, end) = (be32(t, g)?, be32(t, g + 4)?);
        if c < start {
            hi = mid;
        } else if c > end {
            lo = mid + 1;
        } else {
            let gid = be32(t, g + 8)?.checked_add(c - start)?;
            return u16::try_from(gid).ok();
        }
    }
    None
}

fn checksum(d: &[u8]) -> u32 {
    d.chunks(4).fold(0u32, |acc, c| {
        let mut b = [0u8; 4];
        b[..c.len()].copy_from_slice(c);
        acc.wrapping_add(u32::from_be_bytes(b))
    })
}

/// Serializes an sfnt from its tables, fixing head.checkSumAdjustment.
pub(crate) fn build_sfnt(version: u32, mut tables: Vec<([u8; 4], Vec<u8>)>) -> Vec<u8> {
    tables.sort_by(|a, b| a.0.cmp(&b.0));
    let n = tables.len() as u16;
    let mut pow = 1u16;
    let mut log = 0u16;
    while pow * 2 <= n {
        pow *= 2;
        log += 1;
    }
    let mut out = Vec::new();
    out.extend_from_slice(&version.to_be_bytes());
    out.extend_from_slice(&n.to_be_bytes());
    out.extend_from_slice(&(pow * 16).to_be_bytes());
    out.extend_from_slice(&log.to_be_bytes());
    out.extend_from_slice(&(n * 16 - pow * 16).to_be_bytes());
    let mut offset = 12 + 16 * tables.len();
    let mut head_at = None;
    for (tag, data) in &mut tables {
        if tag == b"head" && data.len() >= 12 {
            data[8..12].fill(0);
        }
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum(data).to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        if tag == b"head" {
            head_at = Some(offset);
        }
        offset += data.len().div_ceil(4) * 4;
    }
    for (_, data) in &tables {
        out.extend_from_slice(data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    if let Some(h) = head_at
        && out.len() >= h + 12
    {
        let adj = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
        out[h + 8..h + 12].copy_from_slice(&adj.to_be_bytes());
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A tiny but complete TrueType font: glyph 0 (.notdef box), glyph 1
    /// ('A', simple), glyph 2 ('B', composite of glyph 1).
    pub(crate) fn tiny_font() -> Vec<u8> {
        let mut head = vec![0u8; 54];
        head[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        head[12..16].copy_from_slice(&0x5F0F_3CF5u32.to_be_bytes());
        head[18..20].copy_from_slice(&1000u16.to_be_bytes());
        head[40..42].copy_from_slice(&600i16.to_be_bytes());
        head[42..44].copy_from_slice(&700i16.to_be_bytes());
        head[50..52].copy_from_slice(&1u16.to_be_bytes());
        let mut hhea = vec![0u8; 36];
        hhea[4..6].copy_from_slice(&800i16.to_be_bytes());
        hhea[6..8].copy_from_slice(&(-200i16).to_be_bytes());
        hhea[34..36].copy_from_slice(&3u16.to_be_bytes());
        let mut maxp = vec![0u8; 6];
        maxp[0..4].copy_from_slice(&0x0000_5000u32.to_be_bytes());
        maxp[4..6].copy_from_slice(&3u16.to_be_bytes());
        let mut hmtx = Vec::new();
        for adv in [500u16, 600, 700] {
            hmtx.extend_from_slice(&adv.to_be_bytes());
            hmtx.extend_from_slice(&0u16.to_be_bytes());
        }
        // Simple glyph: one contour, a triangle.
        let mut simple = Vec::new();
        for v in [1i16, 0, 0, 500, 700] {
            simple.extend_from_slice(&v.to_be_bytes());
        }
        simple.extend_from_slice(&2u16.to_be_bytes()); // endPtsOfContours
        simple.extend_from_slice(&0u16.to_be_bytes()); // instructionLength
        simple.extend_from_slice(&[1, 1, 1]); // flags: on curve, long coords
        for v in [0i16, 250, 250] {
            simple.extend_from_slice(&v.to_be_bytes());
        }
        for v in [0i16, 700, -700] {
            simple.extend_from_slice(&v.to_be_bytes());
        }
        let mut composite = Vec::new();
        for v in [-1i16, 0, 0, 500, 700] {
            composite.extend_from_slice(&v.to_be_bytes());
        }
        composite.extend_from_slice(&0x0002u16.to_be_bytes()); // ARGS_ARE_XY_VALUES
        composite.extend_from_slice(&1u16.to_be_bytes());
        composite.extend_from_slice(&[10, 0]);
        let notdef = simple.clone();
        let mut glyf = Vec::new();
        let mut loca = Vec::new();
        for g in [&notdef, &simple, &composite] {
            loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());
            glyf.extend_from_slice(g);
            while glyf.len() % 4 != 0 {
                glyf.push(0);
            }
        }
        loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());
        // cmap format 4: 'A'..'B' -> 1..2, plus the 0xFFFF terminator.
        let mut sub = Vec::new();
        for v in [4u16, 32, 0, 4, 4, 1, 0] {
            sub.extend_from_slice(&v.to_be_bytes());
        }
        for v in [0x42u16, 0xFFFF, 0, 0x41, 0xFFFF] {
            sub.extend_from_slice(&v.to_be_bytes());
        }
        for v in [(1i32 - 0x41) as i16 as u16, 1] {
            sub.extend_from_slice(&v.to_be_bytes());
        }
        sub.extend_from_slice(&[0, 0, 0, 0]);
        let mut cmap = Vec::new();
        for v in [0u16, 1, 3, 1] {
            cmap.extend_from_slice(&v.to_be_bytes());
        }
        cmap.extend_from_slice(&12u32.to_be_bytes());
        cmap.extend_from_slice(&sub);
        build_sfnt(
            0x0001_0000,
            vec![
                (*b"head", head),
                (*b"hhea", hhea),
                (*b"maxp", maxp),
                (*b"hmtx", hmtx),
                (*b"glyf", glyf),
                (*b"loca", loca),
                (*b"cmap", cmap),
            ],
        )
    }

    #[test]
    fn parses_tiny_font() {
        let f = Font::parse(tiny_font().into(), 0).expect("font parses");
        assert_eq!(f.outlines, Outlines::Glyf);
        assert_eq!(f.glyph_id('A'), 1);
        assert_eq!(f.glyph_id('B'), 2);
        assert_eq!(f.glyph_id('C'), 0);
        assert!(f.has_char('A') && !f.has_char('z'));
        assert_eq!(f.width(2), 700.0);
        assert_eq!(f.metrics.ascent, 800.0);
        assert_eq!(f.metrics.descent, -200.0);
    }

    #[test]
    fn subset_keeps_components_and_empties_the_rest() {
        let f = Font::parse(tiny_font().into(), 0).unwrap();
        // Using only the composite glyph must keep its component (glyph 1).
        let sub = f.subset_glyf(&BTreeSet::from([2]));
        assert_eq!(checksum(&sub), 0xB1B0_AFBA);
        let lens = glyph_lengths(&sub);
        assert_eq!(lens.len(), 3);
        assert!(lens.iter().all(|&l| l > 0));
        // Nothing but .notdef: glyphs 1 and 2 become empty.
        let lens = glyph_lengths(&f.subset_glyf(&BTreeSet::new()));
        assert!(lens[0] > 0 && lens[1] == 0 && lens[2] == 0);
    }

    /// Glyph byte lengths of a subset font (which has no cmap, so it can't go
    /// through `Font::parse`).
    fn glyph_lengths(sfnt: &[u8]) -> Vec<usize> {
        let n = be16(sfnt, 4).unwrap() as usize;
        let table = |tag: &[u8]| {
            (0..n)
                .map(|i| 12 + 16 * i)
                .find(|&r| &sfnt[r..r + 4] == tag)
                .map(|r| {
                    let off = be32(sfnt, r + 8).unwrap() as usize;
                    &sfnt[off..off + be32(sfnt, r + 12).unwrap() as usize]
                })
                .unwrap()
        };
        let long = i16_at(table(b"head"), 50) == 1;
        let loca = table(b"loca");
        let offs: Vec<usize> = if long {
            loca.chunks(4).map(|c| be32(c, 0).unwrap() as usize).collect()
        } else {
            loca.chunks(2).map(|c| be16(c, 0).unwrap() as usize * 2).collect()
        };
        offs.windows(2).map(|w| w[1] - w[0]).collect()
    }

    #[test]
    fn truncated_fonts_never_panic() {
        let full = tiny_font();
        for len in 0..full.len() {
            if let Some(f) = Font::parse(full[..len].to_vec().into(), 0) {
                let _ = f.glyph_id('A');
                let _ = f.width(7);
                let _ = f.subset_glyf(&BTreeSet::from([0, 1, 2, 9]));
            }
        }
        // Garbage in every byte position of the directory and tables.
        for i in 0..full.len() {
            let mut bad = full.clone();
            bad[i] ^= 0xFF;
            if let Some(f) = Font::parse(bad.into(), 0) {
                let _ = f.glyph_id('B');
                let _ = f.subset_glyf(&BTreeSet::from([1, 2]));
            }
        }
    }
}
