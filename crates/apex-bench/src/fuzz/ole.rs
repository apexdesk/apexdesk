//! OLE compound files (xls, doc, vbaProject.bin, encrypted OOXML): the
//! streams are read and changed with knowledge of what they hold (BIFF
//! records of a workbook, the FIB of a Word document, the records of a
//! VBA `dir` stream) and written into a new compound file; or the
//! container is damaged in place: header fields, FAT chains (loops, ends,
//! sectors out of the file) and directory entries (sizes, start sectors,
//! siblings).

use std::io::{Cursor, Read, Write};
use std::path::PathBuf;

use super::bytes;
use super::rng::Rng;

pub struct Streams {
    pub storages: Vec<PathBuf>,
    pub streams: Vec<(PathBuf, Vec<u8>)>,
}

pub fn read(data: &[u8]) -> Option<Streams> {
    let mut c = cfb::CompoundFile::open(Cursor::new(data)).ok()?;
    let entries: Vec<(PathBuf, bool)> = c.walk().map(|e| (e.path().to_path_buf(), e.is_stream())).collect();
    let mut s = Streams { storages: Vec::new(), streams: Vec::new() };
    for (p, stream) in entries {
        if stream {
            let mut v = Vec::new();
            c.open_stream(&p).ok()?.read_to_end(&mut v).ok()?;
            s.streams.push((p, v));
        } else if p != std::path::Path::new("/") {
            s.storages.push(p);
        }
    }
    Some(s)
}

pub fn write(s: &Streams) -> Option<Vec<u8>> {
    let mut c = cfb::CompoundFile::create_with_version(cfb::Version::V3, Cursor::new(Vec::new())).ok()?;
    for p in &s.storages {
        let _ = c.create_storage_all(p);
    }
    for (p, d) in &s.streams {
        if let Some(parent) = p.parent() {
            let _ = c.create_storage_all(parent);
        }
        if let Ok(mut w) = c.create_stream(p) {
            w.write_all(d).ok()?;
        }
    }
    c.flush().ok()?;
    Some(c.into_inner().into_inner())
}

fn u16_at(d: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(i..i + 2)?.try_into().ok()?))
}

fn u32_at(d: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(i..i + 4)?.try_into().ok()?))
}

/// Records of a BIFF stream: (offset, type, length).
pub fn biff_records(d: &[u8]) -> Vec<(usize, u16, usize)> {
    let mut out = Vec::new();
    let mut p = 0;
    while let (Some(t), Some(n)) = (u16_at(d, p), u16_at(d, p + 2)) {
        let n = n as usize;
        if p + 4 + n > d.len() {
            break;
        }
        out.push((p, t, n));
        p += 4 + n;
    }
    out
}

/// One change to the records of a workbook stream.
fn biff(rng: &mut Rng, d: &mut Vec<u8>) {
    let recs = biff_records(d);
    if recs.len() < 2 {
        return bytes::binary(rng, d);
    }
    let (p, _, n) = *rng.pick(&recs);
    match rng.below(8) {
        0 => bytes::put(d, p + 2, &rng.pick(bytes::U16S).to_le_bytes()),
        1..=3 => {
            // A field inside the record.
            if n == 0 {
                return;
            }
            let at = p + 4 + rng.below(n);
            if rng.one_in(2) {
                bytes::put(&mut d[..p + 4 + n], at, &rng.pick(bytes::U16S).to_le_bytes());
            } else {
                bytes::put(&mut d[..p + 4 + n], at, &rng.pick(bytes::U32S).to_le_bytes());
            }
        }
        4 => {
            d.drain(p..p + 4 + n);
        }
        5 => {
            let rec = d[p..p + 4 + n].to_vec();
            let times = if rng.one_in(4) { 1000 } else { 1 };
            d.splice(p..p, rec.repeat(times));
        }
        6 => {
            // Another record's type.
            let (_, t, _) = *rng.pick(&recs);
            bytes::put(d, p, &t.to_le_bytes());
        }
        _ => {
            // Swap two records.
            let (q, _, m) = *rng.pick(&recs);
            if q <= p {
                return;
            }
            let a = d[p..p + 4 + n].to_vec();
            let b = d[q..q + 4 + m].to_vec();
            d.splice(q..q + 4 + m, a);
            d.splice(p..p + 4 + n, b);
        }
    }
}

/// One change to the FIB of a WordDocument stream: an offset or a length
/// of a table (fc/lcb pairs), or a flag of the base.
fn fib(rng: &mut Rng, d: &mut [u8]) {
    let len = d.len();
    let pairs = len.saturating_sub(0x9A).min(0x500) / 8;
    match rng.below(4) {
        0 if len > 0x0C => {
            // fWhichTblStm, fEncrypted, fComplex...
            let flags = u16_at(d, 0x0A).unwrap_or(0) ^ (1 << rng.below(16));
            bytes::put(d, 0x0A, &flags.to_le_bytes());
        }
        1 if pairs > 0 => {
            let at = 0x9A + 8 * rng.below(pairs) + 4 * rng.below(2);
            let old = u32_at(d, at).unwrap_or(0);
            let v = match rng.below(4) {
                0 => *rng.pick(bytes::U32S),
                1 => old.wrapping_add(rng.below(16) as u32).wrapping_sub(8),
                2 => old.wrapping_mul(2).wrapping_add(1),
                _ => len as u32,
            };
            bytes::put(d, at, &v.to_le_bytes());
        }
        2 if pairs > 1 => {
            // Two tables swapped.
            let (a, b) = (0x9A + 8 * rng.below(pairs), 0x9A + 8 * rng.below(pairs));
            let (x, y) = (d[a..a + 8].to_vec(), d[b..b + 8].to_vec());
            bytes::put(d, a, &y);
            bytes::put(d, b, &x);
        }
        _ => {
            let mut v = d.to_vec();
            bytes::binary(rng, &mut v);
            let n = v.len().min(d.len());
            d[..n].copy_from_slice(&v[..n]);
        }
    }
}

/// One change to a VBA `dir` stream (compressed records).
fn vba_dir(rng: &mut Rng, d: &mut Vec<u8>) {
    let Some(mut plain) = apex_io::vba::decompress(d) else { return bytes::binary(rng, d) };
    let mut recs = Vec::new();
    let mut p = 0;
    while let (Some(id), Some(size)) = (u16_at(&plain, p), u32_at(&plain, p + 2)) {
        let n = if id == 0x0009 { 6 } else { size as usize };
        if p + 6 + n > plain.len() {
            break;
        }
        recs.push((p, n));
        p += 6 + n;
    }
    if recs.is_empty() {
        bytes::binary(rng, &mut plain);
    } else {
        let (p, n) = *rng.pick(&recs);
        match rng.below(5) {
            0 => bytes::put(&mut plain, p + 2, &rng.pick(bytes::U32S).to_le_bytes()),
            1 => {
                plain.drain(p..p + 6 + n);
            }
            2 => {
                let r = plain[p..p + 6 + n].to_vec();
                plain.splice(p..p, r);
            }
            3 if n > 0 => {
                let at = p + 6 + rng.below(n);
                bytes::put(&mut plain[..p + 6 + n], at, &rng.pick(bytes::U32S).to_le_bytes());
            }
            _ => bytes::put(&mut plain, p, &rng.pick(bytes::U16S).to_le_bytes()),
        }
    }
    *d = apex_io::vba::compress(&plain);
}

fn mutate_stream(rng: &mut Rng, name: &str, d: &mut Vec<u8>) {
    match name {
        "Workbook" | "Book" if !rng.one_in(4) => biff(rng, d),
        "WordDocument" if !rng.one_in(4) => fib(rng, d),
        "dir" if !rng.one_in(4) => vba_dir(rng, d),
        "PROJECT" | "PROJECTwm" => bytes::text(rng, d, "txt"),
        "EncryptionInfo" | "EncryptedPackage" if d.len() > 16 && rng.one_in(2) => {
            let at = 4 * rng.below(d.len().min(256) / 4);
            bytes::put(d, at, &rng.pick(bytes::U32S).to_le_bytes());
        }
        _ => {
            for _ in 0..rng.small_len(4) {
                bytes::binary(rng, d);
            }
        }
    }
}

/// Where the sectors of the FAT are (the first 109, from the header).
fn fat_sectors(d: &[u8]) -> Vec<u32> {
    (0..109).filter_map(|i| u32_at(d, 0x4C + 4 * i)).filter(|&s| s < 0xFFFF_FFFA).collect()
}

fn sector_size(d: &[u8]) -> usize {
    match u16_at(d, 0x1E) {
        Some(12) => 4096,
        _ => 512,
    }
}

/// One change to the container itself.
pub fn damage(rng: &mut Rng, d: &mut Vec<u8>) {
    if d.len() < 512 {
        return bytes::binary(rng, d);
    }
    let ssz = sector_size(d);
    match rng.below(6) {
        0 => {
            // A header field.
            let (off, wide) = *rng.pick(&[(0x18usize, false), (0x1A, false), (0x1E, false), (0x20, false), (0x28, true), (0x2C, true), (0x30, true), (0x38, true), (0x3C, true), (0x40, true), (0x44, true), (0x48, true), (0x4C, true), (0x50, true)]);
            if wide {
                bytes::put(d, off, &rng.pick(bytes::U32S).to_le_bytes());
            } else {
                bytes::put(d, off, &rng.pick(bytes::U16S).to_le_bytes());
            }
        }
        1 | 2 => {
            // A FAT entry: a loop, an early end, a free sector, a sector
            // past the end of the file.
            let fats = fat_sectors(d);
            if fats.is_empty() {
                return;
            }
            let f = rng.below(fats.len());
            let e = rng.below(ssz / 4);
            let own = (f * ssz / 4 + e) as u32;
            let sectors = (d.len() / ssz) as u32;
            let v = match rng.below(6) {
                0 => own,
                1 => own.saturating_sub(1),
                2 => 0xFFFF_FFFE,
                3 => 0xFFFF_FFFF,
                4 => sectors + rng.below(1000) as u32,
                _ => rng.below(sectors as usize + 1) as u32,
            };
            bytes::put(d, (fats[f] as usize + 1) * ssz + 4 * e, &v.to_le_bytes());
        }
        3 | 4 => {
            // A directory entry (the first sector of the directory).
            let Some(first) = u32_at(d, 0x30) else { return };
            let base = (first as usize + 1) * ssz + 128 * rng.below(ssz / 128);
            if base + 128 > d.len() {
                return;
            }
            let (off, wide) = *rng.pick(&[(0x40usize, false), (0x42, false), (0x44, true), (0x48, true), (0x4C, true), (0x74, true), (0x78, true), (0x7C, true)]);
            if off == 0x42 {
                d[base + off] = rng.below(8) as u8;
            } else if wide {
                bytes::put(d, base + off, &rng.pick(bytes::U32S).to_le_bytes());
            } else {
                bytes::put(d, base + off, &rng.pick(bytes::U16S).to_le_bytes());
            }
        }
        _ => {
            if rng.one_in(2) {
                let at = rng.below(d.len());
                d.truncate(at);
            } else {
                bytes::binary(rng, d);
            }
        }
    }
}

/// A mutated compound file.
pub fn mutate(rng: &mut Rng, data: &[u8]) -> Vec<u8> {
    if rng.one_in(4) {
        let mut d = data.to_vec();
        for _ in 0..rng.small_len(3) {
            damage(rng, &mut d);
        }
        return d;
    }
    let Some(mut s) = read(data) else {
        let mut d = data.to_vec();
        damage(rng, &mut d);
        return d;
    };
    if s.streams.is_empty() {
        return data.to_vec();
    }
    for _ in 0..rng.small_len(3) {
        let i = rng.below(s.streams.len());
        match rng.below(12) {
            0 => {
                s.streams.remove(i);
                if s.streams.is_empty() {
                    break;
                }
            }
            1 => s.streams[i].1.clear(),
            _ => {
                let name = s.streams[i].0.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
                mutate_stream(rng, &name, &mut s.streams[i].1);
            }
        }
    }
    let mut out = write(&s).unwrap_or_else(|| data.to_vec());
    if rng.one_in(6) {
        damage(rng, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        // BOF, a record, EOF.
        let mut wb = Vec::new();
        for (t, data) in [(0x0809u16, vec![0u8; 16]), (0x0204, vec![1, 0, 2, 0, 0, 0, 1, 0, b'x']), (0x000A, vec![])] {
            wb.extend(t.to_le_bytes());
            wb.extend((data.len() as u16).to_le_bytes());
            wb.extend(data);
        }
        let s = Streams { storages: vec![PathBuf::from("/VBA")], streams: vec![(PathBuf::from("/Workbook"), wb), (PathBuf::from("/VBA/dir"), apex_io::vba::compress(&[3, 0, 2, 0, 0, 0, 0xE4, 0x04]))] };
        write(&s).unwrap()
    }

    #[test]
    fn streams_roundtrip() {
        let s = read(&sample()).unwrap();
        assert_eq!(s.streams.len(), 2);
        assert_eq!(biff_records(&s.streams.iter().find(|x| x.0.ends_with("Workbook")).unwrap().1).len(), 3);
    }

    #[test]
    fn mutations_run() {
        let mut rng = Rng::new(9);
        let base = sample();
        let differ = (0..300).filter(|_| mutate(&mut rng, &base) != base).count();
        assert!(differ > 200);
    }
}
