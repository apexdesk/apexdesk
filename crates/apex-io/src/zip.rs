//! Minimal ZIP reader/writer (stored + deflate) for OOXML and ODF packages,
//! with the Zip64 extensions (more than 65535 entries, or sizes and offsets
//! past 4 GB) when reading and when a package needs them.

use std::collections::HashMap;

use crate::Error;

/// CRC tables for eight bytes at a time ("slicing by 8"): table `k` is the
/// CRC of a byte followed by `k` zero bytes.
const CRC_TABLES: [[u32; 256]; 8] = {
    let mut t = [[0u32; 256]; 8];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[0][i] = c;
        i += 1;
    }
    let mut k = 1;
    while k < 8 {
        let mut i = 0;
        while i < 256 {
            let c = t[k - 1][i];
            t[k][i] = t[0][(c & 0xFF) as usize] ^ (c >> 8);
            i += 1;
        }
        k += 1;
    }
    t
};

pub fn crc32(data: &[u8]) -> u32 {
    let t = &CRC_TABLES;
    let mut c = !0u32;
    let mut chunks = data.chunks_exact(8);
    for b in &mut chunks {
        let lo = c ^ u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        c = t[7][(lo & 0xFF) as usize]
            ^ t[6][(lo >> 8 & 0xFF) as usize]
            ^ t[5][(lo >> 16 & 0xFF) as usize]
            ^ t[4][(lo >> 24) as usize]
            ^ t[3][b[4] as usize]
            ^ t[2][b[5] as usize]
            ^ t[1][b[6] as usize]
            ^ t[0][b[7] as usize];
    }
    for &b in chunks.remainder() {
        c = t[0][((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

/// Deflates `data` (raw, as ZIP stores it). Big data is compressed a
/// slice at a time on a few threads, as pigz does: each slice but the
/// last ends with a flush to a byte boundary, so their outputs one after
/// another are a single stream.
fn deflate(data: &[u8]) -> Vec<u8> {
    use miniz_oxide::deflate::core::{CompressorOxide, TDEFLFlush, TDEFLStatus, compress_to_output, create_comp_flags_from_zip_params};
    const SLICE: usize = 1 << 20;
    let threads = apex_fs::thread::available_parallelism().map_or(1, |n| n.get()).min(4);
    if data.len() <= 2 * SLICE || threads < 2 {
        return miniz_oxide::deflate::compress_to_vec(data, 6);
    }
    let slice = |i: usize| -> Vec<u8> {
        let part = &data[i * SLICE..((i + 1) * SLICE).min(data.len())];
        let last = (i + 1) * SLICE >= data.len();
        let mut c = CompressorOxide::new(create_comp_flags_from_zip_params(6, 0, 0));
        let mut out = Vec::with_capacity(part.len() / 3);
        let flush = if last { TDEFLFlush::Finish } else { TDEFLFlush::Sync };
        let (status, used) = compress_to_output(&mut c, part, flush, |b| {
            out.extend_from_slice(b);
            true
        });
        assert!(used == part.len() && matches!(status, TDEFLStatus::Okay | TDEFLStatus::Done), "deflate");
        out
    };
    let n = data.len().div_ceil(SLICE);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut parts: Vec<(usize, Vec<u8>)> = apex_fs::thread::scope(|s| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= n {
                            break done;
                        }
                        done.push((i, slice(i)));
                    }
                })
            })
            .collect();
        workers.into_iter().flat_map(|w| w.join().unwrap_or_else(|e| std::panic::resume_unwind(e))).collect()
    });
    parts.sort_unstable_by_key(|p| p.0);
    let mut out = Vec::with_capacity(parts.iter().map(|p| p.1.len()).sum());
    for (_, p) in parts {
        out.extend_from_slice(&p);
    }
    out
}

fn u16_at(b: &[u8], i: usize) -> Result<u16, Error> {
    b.get(i..i + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or_else(|| Error::new("truncated ZIP"))
}

fn u32_at(b: &[u8], i: usize) -> Result<u32, Error> {
    b.get(i..i + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])).ok_or_else(|| Error::new("truncated ZIP"))
}

fn u64_at(b: &[u8], i: usize) -> Result<u64, Error> {
    b.get(i..i + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap())).ok_or_else(|| Error::new("truncated ZIP"))
}

/// Marks a 16/32-bit field whose real value is in the Zip64 records.
const FULL16: u16 = 0xFFFF;
const FULL32: u32 = 0xFFFF_FFFF;

struct Entry {
    name: String,
    method: u16,
    comp_size: usize,
    size: usize,
    offset: usize,
}

pub struct ZipReader<'a> {
    data: &'a [u8],
    entries: Vec<Entry>,
    /// Where each name is in `entries` (the first of its name), as
    /// written and in lowercase: packages may have very many.
    by_name: HashMap<String, usize>,
    by_lower: HashMap<String, usize>,
    /// What whole-part reads may still inflate to, all parts together:
    /// a package of many bombs stops there too. A package read while
    /// another is open on the same thread (a workbook inside a document)
    /// shares the outer one's: many copies of a small one add up.
    budget: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

thread_local! {
    /// The budget of the package open on this thread, if any.
    static OPEN_BUDGET: std::cell::RefCell<std::sync::Weak<std::sync::atomic::AtomicUsize>> = const { std::cell::RefCell::new(std::sync::Weak::new()) };
}

/// The budget of the package open on this thread, or a new one.
fn package_budget() -> std::sync::Arc<std::sync::atomic::AtomicUsize> {
    OPEN_BUDGET.with(|open| {
        let mut open = open.borrow_mut();
        open.upgrade().unwrap_or_else(|| {
            let b = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(PACKAGE_BUDGET));
            *open = std::sync::Arc::downgrade(&b);
            b
        })
    })
}

/// Most one part read whole may inflate to, whatever size it claims.
const PART_CAP: usize = 1 << 30;
/// Most one part read a piece at a time may inflate to.
/// (4 GiB is more than a 32-bit address space: as much as it holds.)
const PIECES_CAP: usize = (1usize << 30).saturating_mul(4);
/// Most all whole-part reads of one package may inflate to.
const PACKAGE_BUDGET: usize = 3 << 30;
// No cap wrapped to nothing on 32-bit targets (Android armv7, the browser).
const _: () = assert!(PIECES_CAP >= PART_CAP && PACKAGE_BUDGET >= PART_CAP);

impl<'a> ZipReader<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, Error> {
        if data.len() < 22 {
            return Err(Error::new("not a valid ZIP file"));
        }
        let min = data.len().saturating_sub(22 + 65535);
        let eocd = (min..=data.len() - 22)
            .rev()
            .find(|&i| data[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])
            .ok_or_else(|| Error::new("not a valid ZIP file"))?;
        let mut count = u16_at(data, eocd + 10)? as u64;
        let mut p = u32_at(data, eocd + 16)? as u64;
        // Zip64: a locator just before the end record points to the Zip64
        // end record, which has the real count and directory offset.
        if eocd >= 20 && u32_at(data, eocd - 20)? == 0x0706_4b50 {
            let z = usize::try_from(u64_at(data, eocd - 12)?).map_err(|_| Error::new("truncated ZIP"))?;
            if u32_at(data, z)? == 0x0606_4b50 {
                count = u64_at(data, z + 32)?;
                p = u64_at(data, z + 48)?;
            }
        }
        let mut p = usize::try_from(p).map_err(|_| Error::new("truncated ZIP"))?;
        // Each entry takes 46 bytes at least: a larger count is corrupt.
        let count = usize::try_from(count).unwrap_or(usize::MAX).min(data.len() / 46);
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            if u32_at(data, p)? != 0x0201_4b50 {
                return Err(Error::new("corrupt ZIP directory"));
            }
            let method = u16_at(data, p + 10)?;
            let comp32 = u32_at(data, p + 20)?;
            let size32 = u32_at(data, p + 24)?;
            let name_len = u16_at(data, p + 28)? as usize;
            let extra_len = u16_at(data, p + 30)? as usize;
            let comment_len = u16_at(data, p + 32)? as usize;
            let off32 = u32_at(data, p + 42)?;
            let name = data.get(p + 46..p + 46 + name_len).ok_or_else(|| Error::new("truncated ZIP"))?;
            let extra = data.get(p + 46 + name_len..p + 46 + name_len + extra_len).unwrap_or(&[]);
            let (mut size, mut comp_size, mut offset) = (size32 as u64, comp32 as u64, off32 as u64);
            // The Zip64 extra field has, in this order, the values of the
            // fields that are full in the fixed part.
            if let Some(z) = extra_field(extra, 0x0001) {
                let mut at = 0;
                for (v, full) in [(&mut size, size32 == FULL32), (&mut comp_size, comp32 == FULL32), (&mut offset, off32 == FULL32)] {
                    if full {
                        if let Ok(x) = u64_at(z, at) {
                            *v = x;
                        }
                        at += 8;
                    }
                }
            }
            let fit = |v: u64| usize::try_from(v).map_err(|_| Error::new("truncated ZIP"));
            let name = String::from_utf8_lossy(name).replace('\\', "/");
            entries.push(Entry { name, method, comp_size: fit(comp_size)?, size: fit(size)?, offset: fit(offset)? });
            p += 46 + name_len + extra_len + comment_len;
        }
        let mut by_name = HashMap::with_capacity(entries.len());
        let mut by_lower = HashMap::with_capacity(entries.len());
        for (i, e) in entries.iter().enumerate() {
            by_name.entry(e.name.clone()).or_insert(i);
            by_lower.entry(e.name.to_ascii_lowercase()).or_insert(i);
        }
        Ok(ZipReader { data, entries, by_name, by_lower, budget: package_budget() })
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.name.as_str())
    }

    pub fn contains(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    /// The name `name` is stored under (found as [`ZipReader::read`] finds
    /// it: without a leading `/`, or in another case).
    pub fn entry_name(&self, name: &str) -> Option<&str> {
        let name = name.trim_start_matches('/');
        self.by_name.get(name).or_else(|| self.by_lower.get(&name.to_ascii_lowercase())).map(|&i| self.entries[i].name.as_str())
    }

    /// The entry `name` and its data as stored.
    fn raw(&self, name: &str) -> Result<(&Entry, &'a [u8]), Error> {
        let name = name.trim_start_matches('/');
        let e = self
            .by_name
            .get(name)
            .or_else(|| self.by_lower.get(&name.to_ascii_lowercase()))
            .map(|&i| &self.entries[i])
            .ok_or_else(|| Error::new(format!("missing part '{}' in package", name)))?;
        let d = self.data;
        if u32_at(d, e.offset)? != 0x0403_4b50 {
            return Err(Error::new("corrupt ZIP header"));
        }
        let start = e.offset + 30 + u16_at(d, e.offset + 26)? as usize + u16_at(d, e.offset + 28)? as usize;
        let raw = start.checked_add(e.comp_size).and_then(|end| d.get(start..end)).ok_or_else(|| Error::new("truncated ZIP"))?;
        Ok((e, raw))
    }

    pub fn read(&self, name: &str) -> Result<Vec<u8>, Error> {
        let (e, raw) = self.raw(name)?;
        match e.method {
            0 => {
                if !self.charge(raw.len()) {
                    return Err(Error::new("ZIP package too large"));
                }
                Ok(raw.to_vec())
            }
            8 => {
                use std::sync::atomic::Ordering;
                let left = self.budget.load(Ordering::Relaxed);
                // A part that fails still took the work: read again and
                // again, it would cost that each time.
                let out = miniz_oxide::inflate::decompress_to_vec_with_limit(raw, inflate_limit(e, PART_CAP).min(left)).map_err(|err| {
                    self.charge(err.output.len().max(raw.len()));
                    Error::new(format!("could not decompress '{}'", e.name))
                })?;
                self.charge(out.len());
                Ok(out)
            }
            m => Err(Error::new(format!("unsupported ZIP compression method ({})", m))),
        }
    }

    /// The part `name` a piece at a time (up to 1 MiB each), for parts too
    /// big to hold whole. On damage midway, `f` has had the pieces before.
    pub fn read_pieces(&self, name: &str, mut f: impl FnMut(&[u8])) -> Result<(), Error> {
        use miniz_oxide::inflate::stream::{InflateState, inflate};
        use miniz_oxide::{DataFormat, MZFlush, MZStatus};
        const PIECE: usize = 1 << 20;
        let (e, mut raw) = self.raw(name)?;
        match e.method {
            0 => {
                raw.chunks(PIECE).for_each(f);
                return Ok(());
            }
            8 => {}
            m => return Err(Error::new(format!("unsupported ZIP compression method ({})", m))),
        }
        let damaged = || Error::new(format!("could not decompress '{}'", e.name));
        let mut state = InflateState::new_boxed(DataFormat::Raw);
        let mut buf = vec![0u8; PIECE];
        // Pieces count against the package's budget too, as they inflate:
        // several sheets that are bombs, read one after another or at
        // once, would otherwise add up.
        let (mut total, limit) = (0, inflate_limit(e, PIECES_CAP));
        let r = loop {
            let res = inflate(&mut state, raw, &mut buf, MZFlush::None);
            raw = &raw[res.bytes_consumed..];
            total += res.bytes_written;
            if total > limit || !self.charge(res.bytes_written) {
                break Err(damaged());
            }
            if res.bytes_written > 0 {
                f(&buf[..res.bytes_written]);
            }
            match res.status {
                Ok(MZStatus::StreamEnd) => break Ok(()),
                Ok(_) if res.bytes_consumed > 0 || res.bytes_written > 0 => {}
                _ => break Err(damaged()),
            }
        };
        r
    }

    /// Takes `n` bytes from the package's budget, never below nothing;
    /// false when there weren't that many left.
    fn charge(&self, n: usize) -> bool {
        use std::sync::atomic::Ordering;
        let before = self.budget.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |b| Some(b.saturating_sub(n)));
        before.is_ok_and(|b| b >= n)
    }

    pub fn read_string(&self, name: &str) -> Result<String, Error> {
        let mut b = self.read(name)?;
        if b.starts_with(&[0xEF, 0xBB, 0xBF]) {
            b.drain(..3);
        }
        // Valid text (nearly always) is kept without a copy.
        Ok(String::from_utf8(b).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
    }
}

/// Most a part may inflate to: damage or a zip bomb stops there. The size
/// it claims is the file's to choose, so the limit is also held to what
/// deflate can give from the bytes stored (under 1032 to 1) and to `cap`.
fn inflate_limit(e: &Entry, cap: usize) -> usize {
    let claimed = e.size.max(1).saturating_mul(2).saturating_add(512 << 20);
    let possible = e.comp_size.saturating_mul(1040).saturating_add(1 << 20);
    claimed.min(possible).min(cap)
}

/// The data of the extra field `id` in a list of extra fields.
fn extra_field(mut extra: &[u8], id: u16) -> Option<&[u8]> {
    while extra.len() >= 4 {
        let (tag, len) = (u16::from_le_bytes([extra[0], extra[1]]), u16::from_le_bytes([extra[2], extra[3]]) as usize);
        let body = extra.get(4..4 + len)?;
        if tag == id {
            return Some(body);
        }
        extra = &extra[4 + len..];
    }
    None
}

pub struct ZipWriter {
    out: Vec<u8>,
    /// The directory's record of each entry; one replaced by a later entry
    /// of the same name is left empty.
    central: Vec<Vec<u8>>,
    /// Where each name's record is (names compared without case, as a
    /// package does).
    names: std::collections::HashMap<String, usize>,
    count: u64,
}

impl Default for ZipWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl ZipWriter {
    pub fn new() -> Self {
        ZipWriter { out: Vec::new(), central: Vec::new(), names: std::collections::HashMap::new(), count: 0 }
    }

    pub fn add(&mut self, name: &str, data: &[u8], compress: bool) {
        let crc = crc32(data);
        let (method, body): (u16, std::borrow::Cow<[u8]>) = if compress && data.len() > 64 {
            (8, deflate(data).into())
        } else {
            (0, data.into())
        };
        let offset = self.out.len() as u64;
        let name_b = name.as_bytes();
        // Zip64 when a size or the offset does not fit in 32 bits: the fixed
        // fields say 0xFFFFFFFF and the Zip64 extra field has the values.
        let (size, comp) = (data.len() as u64, body.len() as u64);
        let big_size = size >= FULL32 as u64 || comp >= FULL32 as u64;
        let big_off = offset >= FULL32 as u64;
        let fit = |v: u64, big: bool| if big { FULL32 } else { v as u32 };
        let version: u16 = if big_size || big_off { 45 } else { 20 };
        let zip64_extra = |vals: &[u64]| {
            let mut e = Vec::new();
            if !vals.is_empty() {
                e.extend_from_slice(&1u16.to_le_bytes());
                e.extend_from_slice(&(vals.len() as u16 * 8).to_le_bytes());
                vals.iter().for_each(|v| e.extend_from_slice(&v.to_le_bytes()));
            }
            e
        };
        let sizes = [size, comp];
        let local_extra = zip64_extra(if big_size { &sizes } else { &[] });
        // DOS time: 1980-01-01 00:00
        let (time, date) = (0u16, 0x21u16);
        let mut h = Vec::with_capacity(30 + name_b.len() + local_extra.len());
        h.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        h.extend_from_slice(&version.to_le_bytes());
        h.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 names
        h.extend_from_slice(&method.to_le_bytes());
        h.extend_from_slice(&time.to_le_bytes());
        h.extend_from_slice(&date.to_le_bytes());
        h.extend_from_slice(&crc.to_le_bytes());
        h.extend_from_slice(&fit(comp, big_size).to_le_bytes());
        h.extend_from_slice(&fit(size, big_size).to_le_bytes());
        h.extend_from_slice(&(name_b.len() as u16).to_le_bytes());
        h.extend_from_slice(&(local_extra.len() as u16).to_le_bytes());
        h.extend_from_slice(name_b);
        h.extend_from_slice(&local_extra);
        self.out.extend_from_slice(&h);
        self.out.extend_from_slice(&body);

        let mut vals = Vec::new();
        if big_size {
            vals.extend([size, comp]);
        }
        if big_off {
            vals.push(offset);
        }
        let extra = zip64_extra(&vals);
        let mut c = Vec::with_capacity(46 + name_b.len() + extra.len());
        c.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        c.extend_from_slice(&version.to_le_bytes());
        c.extend_from_slice(&version.to_le_bytes());
        c.extend_from_slice(&0x0800u16.to_le_bytes());
        c.extend_from_slice(&method.to_le_bytes());
        c.extend_from_slice(&time.to_le_bytes());
        c.extend_from_slice(&date.to_le_bytes());
        c.extend_from_slice(&crc.to_le_bytes());
        c.extend_from_slice(&fit(comp, big_size).to_le_bytes());
        c.extend_from_slice(&fit(size, big_size).to_le_bytes());
        c.extend_from_slice(&(name_b.len() as u16).to_le_bytes());
        c.extend_from_slice(&(extra.len() as u16).to_le_bytes());
        c.extend_from_slice(&[0; 10]); // comment, disk, int attr, ext attr (2+2+2+4)
        c.extend_from_slice(&fit(offset, big_off).to_le_bytes());
        c.extend_from_slice(name_b);
        c.extend_from_slice(&extra);
        // A name written twice (a part kept from the file and the one
        // written now): the later one is the package's.
        match self.names.insert(name.to_lowercase(), self.central.len()) {
            Some(old) => self.central[old].clear(),
            None => self.count += 1,
        }
        self.central.push(c);
    }

    pub fn finish(mut self) -> Vec<u8> {
        let cd_offset = self.out.len() as u64;
        let central = self.central.concat();
        let cd_len = central.len() as u64;
        self.out.extend_from_slice(&central);
        let zip64 = self.count >= FULL16 as u64 || cd_offset >= FULL32 as u64 || cd_len >= FULL32 as u64;
        if zip64 {
            // Zip64 end record and its locator; the classic end record
            // that follows has its fields full.
            let at = self.out.len() as u64;
            let o = &mut self.out;
            o.extend_from_slice(&0x0606_4b50u32.to_le_bytes());
            o.extend_from_slice(&44u64.to_le_bytes());
            o.extend_from_slice(&45u16.to_le_bytes());
            o.extend_from_slice(&45u16.to_le_bytes());
            o.extend_from_slice(&[0; 8]); // this disk, disk with the directory
            o.extend_from_slice(&self.count.to_le_bytes());
            o.extend_from_slice(&self.count.to_le_bytes());
            o.extend_from_slice(&cd_len.to_le_bytes());
            o.extend_from_slice(&cd_offset.to_le_bytes());
            o.extend_from_slice(&0x0706_4b50u32.to_le_bytes());
            o.extend_from_slice(&0u32.to_le_bytes());
            o.extend_from_slice(&at.to_le_bytes());
            o.extend_from_slice(&1u32.to_le_bytes());
        }
        let count = if zip64 { FULL16 } else { self.count as u16 };
        let fit = |v: u64| if zip64 { FULL32 } else { v as u32 };
        self.out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        self.out.extend_from_slice(&[0; 4]);
        self.out.extend_from_slice(&count.to_le_bytes());
        self.out.extend_from_slice(&count.to_le_bytes());
        self.out.extend_from_slice(&fit(cd_len).to_le_bytes());
        self.out.extend_from_slice(&fit(cd_offset).to_le_bytes());
        self.out.extend_from_slice(&0u16.to_le_bytes());
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A part compressed in slices on several threads reads back whole,
    /// with the CRC a byte at a time gives.
    #[test]
    fn big_part() {
        let mut data = Vec::new();
        let mut x = 1u32;
        while data.len() < 5 << 20 {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            data.extend_from_slice(format!("<c r=\"A{}\"><v>{}</v></c>", data.len(), x >> 16).as_bytes());
        }
        let mut c = !0u32;
        for &b in &data[..1001] {
            c = CRC_TABLES[0][((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
        }
        assert_eq!(crc32(&data[..1001]), !c);
        let mut z = ZipWriter::new();
        z.add("xl/worksheets/sheet1.xml", &data, true);
        let bytes = z.finish();
        assert!(bytes.len() < data.len() / 2);
        assert_eq!(ZipReader::new(&bytes).unwrap().read("xl/worksheets/sheet1.xml").unwrap(), data);
    }

    #[test]
    fn claimed_sizes_do_not_lift_the_limit() {
        let e = |size: usize, comp_size: usize| Entry { name: String::new(), method: 8, comp_size, size, offset: 0 };
        // A zip64 size near the top neither overflows nor lifts the cap.
        assert_eq!(inflate_limit(&e(usize::MAX, usize::MAX), PART_CAP), PART_CAP);
        assert_eq!(inflate_limit(&e(1 << 62, 10 << 20), PART_CAP), PART_CAP);
        // A few stored bytes can't claim gigabytes.
        assert!(inflate_limit(&e(0xFFFF_FFFE, 4096), PART_CAP) < 6 << 20);
        // An honest size still fits.
        assert!(inflate_limit(&e(5 << 20, 1 << 20), PART_CAP) >= 5 << 20);
    }

    #[test]
    fn package_budget_is_shared_by_parts() {
        let zeros = vec![0u8; 4 << 20];
        let mut w = ZipWriter::new();
        for i in 0..3 {
            w.add(&format!("p{i}.xml"), &zeros, true);
        }
        let bytes = w.finish();
        let r = ZipReader::new(&bytes).unwrap();
        r.budget.store(9 << 20, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(r.read("p0.xml").unwrap().len(), 4 << 20);
        assert_eq!(r.read("p1.xml").unwrap().len(), 4 << 20);
        assert!(r.read("p2.xml").is_err());
        // A package opened inside it takes from the same budget.
        let inner = ZipReader::new(&bytes).unwrap();
        assert!(inner.read("p0.xml").is_err());
        drop((r, inner));
        assert!(ZipReader::new(&bytes).unwrap().read("p2.xml").is_ok());
    }

    #[test]
    fn roundtrip() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        let mut w = ZipWriter::new();
        w.add("mimetype", b"application/test", false);
        let big = "hola mundo ".repeat(1000);
        w.add("dir/a.xml", big.as_bytes(), true);
        let bytes = w.finish();
        let r = ZipReader::new(&bytes).unwrap();
        assert_eq!(r.read("mimetype").unwrap(), b"application/test");
        assert_eq!(r.read_string("dir/a.xml").unwrap(), big);
        assert!(bytes.len() < big.len() / 10);
    }

    #[test]
    fn zip64_many_entries() {
        // More than 65535 entries need the Zip64 end records.
        let mut w = ZipWriter::new();
        for i in 0..70_000 {
            w.add(&format!("f{i}"), b"x", false);
        }
        let bytes = w.finish();
        let r = ZipReader::new(&bytes).unwrap();
        assert_eq!(r.names().count(), 70_000);
        assert_eq!(r.read("f69999").unwrap(), b"x");
    }

    #[test]
    fn zip64_extra_fields() {
        // Sizes and offset in the Zip64 extra fields (as streaming zippers
        // write them, even for small files) and the Zip64 end records.
        let data = b"hello zip64";
        let name = b"a.txt";
        let n = (data.len() as u64).to_le_bytes();
        let mut z = Vec::new();
        z.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        z.extend_from_slice(&[45, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        z.extend_from_slice(&crc32(data).to_le_bytes());
        z.extend_from_slice(&[0xFF; 8]);
        z.extend_from_slice(&[name.len() as u8, 0, 20, 0]);
        z.extend_from_slice(name);
        z.extend_from_slice(&[1, 0, 16, 0]);
        z.extend_from_slice(&n);
        z.extend_from_slice(&n);
        z.extend_from_slice(data);
        let cd = z.len() as u64;
        z.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        z.extend_from_slice(&[45, 0, 45, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        z.extend_from_slice(&crc32(data).to_le_bytes());
        z.extend_from_slice(&[0xFF; 8]);
        z.extend_from_slice(&[name.len() as u8, 0, 28, 0]);
        z.extend_from_slice(&[0; 10]);
        z.extend_from_slice(&[0xFF; 4]);
        z.extend_from_slice(name);
        z.extend_from_slice(&[1, 0, 24, 0]);
        z.extend_from_slice(&n);
        z.extend_from_slice(&n);
        z.extend_from_slice(&0u64.to_le_bytes());
        let (cd_len, at) = (z.len() as u64 - cd, z.len() as u64);
        z.extend_from_slice(&0x0606_4b50u32.to_le_bytes());
        z.extend_from_slice(&44u64.to_le_bytes());
        z.extend_from_slice(&[45, 0, 45, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        z.extend_from_slice(&1u64.to_le_bytes());
        z.extend_from_slice(&1u64.to_le_bytes());
        z.extend_from_slice(&cd_len.to_le_bytes());
        z.extend_from_slice(&cd.to_le_bytes());
        z.extend_from_slice(&0x0706_4b50u32.to_le_bytes());
        z.extend_from_slice(&0u32.to_le_bytes());
        z.extend_from_slice(&at.to_le_bytes());
        z.extend_from_slice(&1u32.to_le_bytes());
        z.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        z.extend_from_slice(&[0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF]);
        z.extend_from_slice(&[0xFF; 8]);
        z.extend_from_slice(&[0, 0]);
        let r = ZipReader::new(&z).unwrap();
        assert_eq!(r.read("a.txt").unwrap(), data);
    }
}
