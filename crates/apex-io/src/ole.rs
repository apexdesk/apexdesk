//! OLE compound files (the legacy .xls and .doc): streams and the document
//! properties (`\u{5}SummaryInformation`).

use std::borrow::Cow;
use std::io::{Cursor, Read};

use crate::Error;
use crate::biff::Cur;

pub(crate) type Ole<'a> = cfb::CompoundFile<Cursor<Cow<'a, [u8]>>>;

pub(crate) fn open(data: &[u8]) -> Result<Ole<'_>, Error> {
    let err = match cfb::CompoundFile::open(Cursor::new(Cow::Borrowed(data))) {
        Ok(c) => return Ok(c),
        Err(e) => Error::new(format!("invalid compound file: {e}")),
    };
    // Some writers leave FAT entries for sectors past the end of the file
    // (end of chain instead of free); Word and LibreOffice open these.
    if let Some(c) = free_fat_tail(data).and_then(|fixed| cfb::CompoundFile::open(Cursor::new(Cow::Owned(fixed))).ok()) {
        return Ok(c);
    }
    // Other damage (siblings out of order, a short FAT...): the streams
    // copied into a new compound file.
    let fixed = crate::ole_repair::rebuild(data).ok_or_else(|| err.clone())?;
    cfb::CompoundFile::open(Cursor::new(Cow::Owned(fixed))).map_err(|_| err)
}

/// A copy of a compound file with the FAT entries of the sectors beyond
/// its end marked free (`None` if there are none to change).
fn free_fat_tail(data: &[u8]) -> Option<Vec<u8>> {
    let at = |p: usize| Some(u32::from_le_bytes(data.get(p..p + 4)?.try_into().ok()?));
    let shift = u16::from_le_bytes(data.get(30..32)?.try_into().ok()?);
    if !(9..=12).contains(&shift) {
        return None;
    }
    let len = 1usize << shift;
    let sectors = (data.len() / len).checked_sub(1)?;
    let offset = |s: u32| len.saturating_add((s as usize).saturating_mul(len));
    // The FAT's sectors: 109 in the header, the rest in a chain of DIFAT
    // sectors, each with `len / 4 - 1` of them and the next one.
    let per = len / 4 - 1;
    let mut fat = Vec::new();
    let mut difat = at(68)?;
    for i in 0..(at(44)? as usize).min(sectors) {
        let s = if i < 109 {
            at(76 + i * 4)?
        } else {
            let j = (i - 109) % per;
            if j == 0 && i > 109 {
                difat = at(offset(difat) + per * 4)?;
            }
            at(offset(difat) + j * 4)?
        };
        fat.push(s);
    }
    let mut out = data.to_vec();
    let mut changed = false;
    for (i, &s) in fat.iter().enumerate() {
        for j in 0..len / 4 {
            let p = offset(s) + j * 4;
            if i * (len / 4) + j >= sectors && out.get(p..p + 4).is_some_and(|e| e != [0xFF; 4]) {
                out[p..p + 4].copy_from_slice(&[0xFF; 4]);
                changed = true;
            }
        }
    }
    changed.then_some(out)
}

pub(crate) fn stream(c: &mut Ole, path: &str) -> Option<Vec<u8>> {
    let mut s = c.open_stream(path).ok()?;
    let mut v = Vec::new();
    s.read_to_end(&mut v).ok()?;
    Some(v)
}

/// Text of a Windows code page (1252 when unknown, which covers ASCII).
pub(crate) fn decode_cp(bytes: &[u8], cp: u16) -> String {
    if cp == 65001 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    match cp_label(cp).and_then(encoding_rs::Encoding::for_label) {
        Some(e) => e.decode_without_bom_handling(bytes).0.into_owned(),
        None => bytes.iter().map(|&b| cp1252(b)).collect(),
    }
}

/// The encoding_rs label of a Windows code page (None: 1252 or unknown).
fn cp_label(cp: u16) -> Option<&'static [u8]> {
    Some(match cp {
        874 => b"windows-874",
        932 => b"shift_jis",
        936 => b"gbk",
        949 => b"euc-kr",
        950 => b"big5",
        1250 => b"windows-1250",
        1251 => b"windows-1251",
        1253 => b"windows-1253",
        1254 => b"windows-1254",
        1255 => b"windows-1255",
        1256 => b"windows-1256",
        1257 => b"windows-1257",
        1258 => b"windows-1258",
        10000 | 32768 => b"macintosh",
        20866 => b"koi8-r",
        _ => return None,
    })
}

/// `text` in a Windows code page; characters it lacks become `?`.
pub(crate) fn encode_cp(text: &str, cp: u16) -> Vec<u8> {
    if cp == 65001 {
        return text.as_bytes().to_vec();
    }
    let enc = cp_label(cp).and_then(encoding_rs::Encoding::for_label).unwrap_or(encoding_rs::WINDOWS_1252);
    let mut out = Vec::with_capacity(text.len());
    let mut buf = [0u8; 4];
    for ch in text.chars() {
        let (bytes, _, bad) = enc.encode(ch.encode_utf8(&mut buf));
        if bad { out.push(b'?') } else { out.extend_from_slice(&bytes) }
    }
    out
}

pub(crate) fn cp1252(b: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž', '\u{8F}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9D}', 'ž', 'Ÿ',
    ];
    if (0x80..0xA0).contains(&b) { HIGH[(b - 0x80) as usize] } else { b as char }
}

/// Title, subject, author, keywords and comments.
pub(crate) fn read_meta(c: &mut Ole) -> apex_doc::Meta {
    let mut meta = apex_doc::Meta::default();
    let Some(data) = stream(c, "/\u{5}SummaryInformation") else { return meta };
    let _ = (|| -> Option<()> {
        let mut h = Cur::new(&data);
        h.skip(24)?;
        if h.u32()? == 0 {
            return None;
        }
        h.skip(16)?;
        let start = h.u32()? as usize;
        let sect = data.get(start..)?;
        let mut s = Cur::new(sect);
        s.skip(4)?;
        let n = s.u32()?.min(1000);
        let mut props = Vec::new();
        for _ in 0..n {
            props.push((s.u32()?, s.u32()? as usize));
        }
        let mut cp = 1252u16;
        for &(id, off) in &props {
            if id == 1 {
                let mut v = Cur::new(sect.get(off..)?);
                if v.u32()? == 2 {
                    cp = v.u16()?;
                }
            }
        }
        for (id, off) in props {
            let mut v = Cur::new(sect.get(off..)?);
            let text = match v.u32()? {
                0x1E => {
                    let n = v.u32()? as usize;
                    let b = v.bytes(n.min(v.left()))?;
                    decode_cp(b, cp)
                }
                0x1F => {
                    let n = v.u32()? as usize;
                    v.utf16(n.min(v.left() / 2))?
                }
                _ => continue,
            };
            let text = text.trim_end_matches('\0').to_string();
            match id {
                2 => meta.title = text,
                3 => meta.subject = text,
                4 => meta.author = text,
                5 => meta.keywords = text,
                6 => meta.description = text,
                _ => {}
            }
        }
        Some(())
    })();
    meta
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn fat_entries_past_the_end() {
        let mut c = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        c.create_stream("/Data").unwrap().write_all(&[7; 5000]).unwrap();
        c.flush().unwrap();
        let mut data = c.into_inner().into_inner();
        // The last FAT entries say "end of chain" instead of "free".
        let len = 1 << data[30];
        let fat = len + u32::from_le_bytes(data[76..80].try_into().unwrap()) as usize * len;
        let sectors = data.len() / len - 1;
        for e in sectors..len / 4 {
            data[fat + e * 4..fat + e * 4 + 4].copy_from_slice(&0xFFFF_FFFEu32.to_le_bytes());
        }
        assert!(cfb::CompoundFile::open(Cursor::new(&data[..])).is_err());
        let mut ole = open(&data).unwrap();
        assert_eq!(stream(&mut ole, "/Data").unwrap(), vec![7; 5000]);
    }
}
