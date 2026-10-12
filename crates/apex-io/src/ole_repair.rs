//! Lenient reading of damaged OLE compound files: the streams are read
//! following the FAT and the directory tree without the checks of the `cfb`
//! crate (sibling name order, sector counts...), and written into a new,
//! valid compound file. Excel, Word and LibreOffice open such files.

use std::collections::HashSet;
use std::io::{Cursor, Write};

const END: u32 = 0xFFFF_FFFA;

struct Header<'a> {
    data: &'a [u8],
    shift: u32,
    fat: Vec<u32>,
}

impl<'a> Header<'a> {
    fn u32(&self, p: usize) -> Option<u32> {
        Some(u32::from_le_bytes(self.data.get(p..p + 4)?.try_into().ok()?))
    }

    fn sector(&self, s: u32) -> Option<&'a [u8]> {
        let len = 1usize << self.shift;
        let at = (s as usize).checked_add(1)?.checked_mul(len)?;
        // The last sector may be cut short.
        self.data.get(at..(at + len).min(self.data.len())).filter(|b| !b.is_empty())
    }

    /// The sectors of the chain starting at `s`, stopping at a loop or at
    /// a sector out of the file.
    /// `sectors`: how many there are (the chain stops at one past them).
    fn chain(&self, mut s: u32, table: &[u32], most: usize, sectors: usize) -> Vec<u32> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        while s < END && (s as usize) < sectors && out.len() < most && seen.insert(s) && out.len() <= table.len() {
            out.push(s);
            s = table.get(s as usize).copied().unwrap_or(END);
        }
        out
    }

    /// The chain's bytes, at most `most` sectors.
    fn read(&self, start: u32, most: usize) -> Vec<u8> {
        let mut out = Vec::new();
        // No more than the file has: a chain past its end reads nothing,
        // and each stream would walk it all.
        let most = most.min((self.data.len() >> self.shift) + 1);
        let len = 1usize << self.shift;
        let sectors = self.data.len().div_ceil(len).saturating_sub(1);
        for s in self.chain(start, &self.fat, most, sectors) {
            match self.sector(s) {
                Some(b) => out.extend_from_slice(b),
                None => break,
            }
        }
        out
    }
}

/// The deepest storages are copied: files have a few levels.
const MAX_DEPTH: usize = 32;

struct Entry {
    name: String,
    kind: u8,
    left: u32,
    right: u32,
    child: u32,
    start: u32,
    size: usize,
}

/// A valid compound file with the streams of a damaged one (None when not
/// even its header and directory can be read).
pub(crate) fn rebuild(data: &[u8]) -> Option<Vec<u8>> {
    if !data.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        return None;
    }
    let shift = u16::from_le_bytes(data.get(30..32)?.try_into().ok()?) as u32;
    if !(7..=16).contains(&shift) {
        return None;
    }
    let mut h = Header { data, shift, fat: Vec::new() };
    // The FAT's sectors: 109 in the header, then the DIFAT chain.
    let per = (1usize << shift) / 4;
    let mut fat_sectors: Vec<u32> = (0..109).filter_map(|i| h.u32(76 + i * 4)).filter(|&s| s < END).collect();
    let mut difat = h.u32(68)?;
    let mut seen = HashSet::new();
    while difat < END && seen.insert(difat) {
        let Some(b) = h.sector(difat) else { break };
        let words: Vec<u32> = b.chunks_exact(4).map(|w| u32::from_le_bytes(w.try_into().unwrap())).collect();
        fat_sectors.extend(words.iter().take(per - 1).copied().filter(|&s| s < END));
        difat = words.get(per - 1).copied().unwrap_or(END);
    }
    let sectors = data.len() >> shift;
    for s in fat_sectors {
        if let Some(b) = h.sector(s) {
            h.fat.extend(b.chunks_exact(4).map(|w| u32::from_le_bytes(w.try_into().unwrap())));
        }
        if h.fat.len() > sectors * 2 + per {
            break;
        }
    }
    let dir = h.read(h.u32(48)?, usize::MAX);
    let entries: Vec<Entry> = dir
        .chunks_exact(128)
        .map(|e| {
            let n = (u16::from_le_bytes([e[64], e[65]]) as usize).min(64) / 2;
            let units: Vec<u16> = e[..n * 2].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            let word = |p: usize| u32::from_le_bytes(e[p..p + 4].try_into().unwrap());
            Entry {
                name: String::from_utf16_lossy(&units).trim_end_matches('\0').to_string(),
                kind: e[66],
                left: word(68),
                right: word(72),
                child: word(76),
                start: word(116),
                size: word(120) as usize,
            }
        })
        .collect();
    let root = entries.first().filter(|e| e.kind == 5)?;
    let cutoff = h.u32(56).unwrap_or(4096) as usize;
    let mini_stream = h.read(root.start, usize::MAX);
    let minifat_bytes = h.read(h.u32(60)?, usize::MAX);
    let minifat: Vec<u32> = minifat_bytes.chunks_exact(4).map(|w| u32::from_le_bytes(w.try_into().unwrap())).collect();
    let mini_shift = u16::from_le_bytes(data.get(32..34)?.try_into().ok()?).clamp(1, 12) as usize;
    let stream_data = |e: &Entry| -> (Vec<u8>, usize) {
        let mut out = if e.size < cutoff {
            let mut v = Vec::new();
            // Only the sectors its size needs: entries sharing a long
            // chain don't walk all of it each time.
            let most = (e.size >> mini_shift).min(mini_stream.len() >> mini_shift) + 1;
            for s in h.chain(e.start, &minifat, most, mini_stream.len() >> mini_shift) {
                let at = (s as usize) << mini_shift;
                match mini_stream.get(at..at + (1 << mini_shift)) {
                    Some(b) => v.extend_from_slice(b),
                    None => break,
                }
            }
            v
        } else {
            h.read(e.start, (e.size >> h.shift) + 1)
        };
        // What was read is charged, then cut to the stream's size.
        let read = out.len();
        out.truncate(e.size);
        (out, read)
    };
    // Walk the tree from the root, each entry once.
    let mut out = cfb::CompoundFile::create(Cursor::new(Vec::new())).ok()?;
    let mut visited = HashSet::new();
    // Entries with their parent's path (shared by siblings) and depth.
    let mut todo: Vec<(u32, std::rc::Rc<str>, usize)> = vec![(root.child, std::rc::Rc::from(""), 0)];
    let mut streams = 0;
    // Streams of a sound file use different sectors: entries that share
    // one would copy it again each time.
    let mut copied = 0usize;
    while let Some((i, parent, depth)) = todo.pop() {
        let Some(e) = entries.get(i as usize).filter(|_| visited.insert(i)) else { continue };
        todo.push((e.left, parent.clone(), depth));
        todo.push((e.right, parent.clone(), depth));
        if e.name.is_empty() || e.name.contains('/') {
            continue;
        }
        let path = format!("{parent}/{}", e.name);
        match e.kind {
            // (Storages deeper than files have are left out.)
            1 if depth < MAX_DEPTH => {
                if out.create_storage(&path).is_ok() {
                    todo.push((e.child, std::rc::Rc::from(path), depth + 1));
                }
            }
            2 => {
                let (body, read) = stream_data(e);
                copied = copied.saturating_add(read);
                if copied > data.len() {
                    return None;
                }
                if let Ok(mut s) = out.create_stream(&path) {
                    s.write_all(&body).ok()?;
                    streams += 1;
                }
            }
            _ => {}
        }
    }
    if streams == 0 {
        return None;
    }
    out.flush().ok()?;
    Some(out.into_inner().into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn sibling_out_of_the_directory() {
        let mut c = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        c.create_stream("/Workbook").unwrap().write_all(&[7; 5000]).unwrap();
        c.create_storage("/Sub").unwrap();
        c.create_stream("/Sub/Small").unwrap().write_all(b"small").unwrap();
        c.create_stream("/Another").unwrap().write_all(&[1; 100]).unwrap();
        c.flush().unwrap();
        let mut data = c.into_inner().into_inner();
        // A sibling index past the directory (as in fuzzed files).
        let dir = (u32::from_le_bytes(data[48..52].try_into().unwrap()) as usize + 1) << data[30];
        let name_at = |data: &[u8], name: &str| (0..8).map(|i| dir + i * 128).find(|&p| String::from_utf16_lossy(&data[p..p + 64].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>()).starts_with(name)).unwrap();
        let small = name_at(&data, "Small");
        data[small + 68..small + 72].copy_from_slice(&65530u32.to_le_bytes());
        assert!(cfb::CompoundFile::open(Cursor::new(&data[..])).is_err());
        let fixed = rebuild(&data).unwrap();
        let mut c = cfb::CompoundFile::open(Cursor::new(fixed)).unwrap();
        let mut v = Vec::new();
        c.open_stream("/Workbook").unwrap().read_to_end(&mut v).unwrap();
        assert_eq!(v, vec![7; 5000]);
        v.clear();
        c.open_stream("/Sub/Small").unwrap().read_to_end(&mut v).unwrap();
        assert_eq!(v, b"small");
    }
}
