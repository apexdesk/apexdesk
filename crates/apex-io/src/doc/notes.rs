//! Bookmarks and comments (annotations) of .doc files. Both are ranges of
//! character positions, turned into marks placed as the text is read;
//! comment bodies are in their own story, authors in a list of names.

use std::collections::HashMap;
use std::sync::Arc;

use apex_doc::{Comment, comment_mark};

use super::{Doc, Fib, plc, u16_at, u32_at};

const PLCF_AND_REF: usize = 4;
const PLCF_AND_TXT: usize = 5;
const STTBF_BKMK: usize = 21;
const PLCF_BKF: usize = 22;
const PLCF_BKL: usize = 23;
const GRP_XST_ATN_OWNERS: usize = 36;
const STTBF_ATN_BKMK: usize = 37;
const PLCF_ATN_BKF: usize = 42;
const PLCF_ATN_BKL: usize = 43;
/// Word 2002+: the comments' dates.
const ATRD_EXTRA: usize = 113;

/// The strings of a string table, with each one's extra data.
fn sttb(d: &[u8]) -> Vec<(String, &[u8])> {
    let mut out = Vec::new();
    let wide = u16_at(d, 0) == Some(0xFFFF);
    let mut p = if wide { 2 } else { 0 };
    let (Some(n), Some(extra)) = (u16_at(d, p), u16_at(d, p + 2)) else { return out };
    let extra = extra as usize;
    p += 4;
    for _ in 0..n {
        let s = if wide {
            let Some(cch) = u16_at(d, p) else { break };
            let units: Vec<u16> = (0..cch as usize).filter_map(|i| u16_at(d, p + 2 + i * 2)).collect();
            p += 2 + cch as usize * 2;
            String::from_utf16_lossy(&units)
        } else {
            let Some(&cch) = d.get(p) else { break };
            let s = d.get(p + 1..p + 1 + cch as usize).unwrap_or(&[]).iter().map(|&b| crate::ole::cp1252(b)).collect();
            p += 1 + cch as usize;
            s
        };
        out.push((s, d.get(p..p + extra).unwrap_or(&[])));
        p += extra;
    }
    out
}

/// A Word 6 string table: its size in bytes, then 8-bit strings.
fn sttb6(d: &[u8]) -> Vec<(String, &[u8])> {
    let end = (u16_at(d, 0).unwrap_or(0) as usize).min(d.len());
    let mut out = Vec::new();
    let mut p = 2;
    while p < end {
        let cch = d[p] as usize;
        out.push((d.get(p + 1..p + 1 + cch).unwrap_or(&[]).iter().map(|&b| crate::ole::cp1252(b)).collect(), &[][..]));
        p += 1 + cch;
    }
    out
}

/// Ranges of a pair of bookmark tables: start and end of each (in the
/// order of the starts).
fn ranges(bkf: &[u8], bkl: &[u8]) -> Vec<(u32, u32)> {
    let (starts, fbkf) = plc(bkf, 4);
    let (ends, _) = plc(bkl, 0);
    fbkf.iter().zip(&starts).filter_map(|(f, &a)| Some((a, *ends.get(u16_at(f, 0)? as usize)?))).collect()
}

pub(super) fn read_bookmarks(doc: &mut Doc, fib: &Fib, table: &[u8]) {
    let names = if fib.word6 { sttb6(fib.part(table, STTBF_BKMK)) } else { sttb(fib.part(table, STTBF_BKMK)) };
    for ((name, _), (a, b)) in names.into_iter().zip(ranges(fib.part(table, PLCF_BKF), fib.part(table, PLCF_BKL))) {
        if !name.is_empty() {
            doc.add_range(Arc::from(name.as_str()), a, b);
        }
    }
}

/// A date and time (`DTTM`) as ISO 8601, or empty.
fn dttm(v: u32) -> String {
    let (min, hr, dom, mon, yr) = (v & 0x3F, (v >> 6) & 0x1F, (v >> 11) & 0x1F, (v >> 16) & 0xF, (v >> 20) & 0x1FF);
    if dom == 0 || mon == 0 || mon > 12 || hr > 23 || min > 59 {
        return String::new();
    }
    format!("{:04}-{mon:02}-{dom:02}T{hr:02}:{min:02}:00Z", 1900 + yr)
}

/// The comments, numbered from 1 in the order of their references; their
/// ranges become marks. `story` is where the comments' text starts.
pub(super) fn read_comments(doc: &mut Doc, fib: &Fib, table: &[u8], story: usize) -> Vec<Comment> {
    let (refs, atrds) = plc(fib.part(table, PLCF_AND_REF), 30);
    let (tcps, _) = plc(fib.part(table, PLCF_AND_TXT), 0);
    // Authors: a run of length-prefixed UTF-16 names.
    let owners_data = fib.part(table, GRP_XST_ATN_OWNERS);
    let mut owners = Vec::new();
    let mut p = 0;
    while let Some(cch) = u16_at(owners_data, p) {
        let units: Vec<u16> = (0..cch as usize).filter_map(|i| u16_at(owners_data, p + 2 + i * 2)).collect();
        owners.push(String::from_utf16_lossy(&units));
        p += 2 + cch as usize * 2;
    }
    // Comment ranges by their tag.
    let tags = sttb(fib.part(table, STTBF_ATN_BKMK));
    let spans: HashMap<u32, (u32, u32)> = tags.iter().zip(ranges(fib.part(table, PLCF_ATN_BKF), fib.part(table, PLCF_ATN_BKL))).filter_map(|((_, x), r)| Some((u32_at(x, 2)?, r))).collect();
    // Dates: 18 bytes per comment, the date first.
    let dates = Some(fib.part(table, ATRD_EXTRA)).filter(|d| d.len() == atrds.len() * 18).unwrap_or(&[]);
    let mut out = Vec::new();
    for (i, (atrd, &at)) in atrds.iter().zip(&refs).enumerate() {
        let (Some(&a), Some(&b)) = (tcps.get(i), tcps.get(i + 1)) else { break };
        let id = i as u32 + 1;
        let text = doc.plain(story + a as usize, story + b as usize);
        let author = u16_at(atrd, 20).and_then(|k| owners.get(k as usize)).cloned().unwrap_or_default();
        let date = u32_at(dates, i * 18).map(dttm).unwrap_or_default();
        out.push(Comment { id, author, date, text: text.trim_matches(['\n', ' ']).to_string(), resolved: false, replies: Vec::new() });
        // Without a range, the comment marks its reference.
        let (s, e) = u32_at(atrd, 26).and_then(|t| spans.get(&t).copied()).unwrap_or((at, at));
        doc.add_range(comment_mark(id), s, e);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_tables_and_dates() {
        // Extended: two strings with 2 bytes of extra data each.
        let mut d = vec![0xFF, 0xFF, 2, 0, 2, 0];
        d.extend([2, 0, b'a', 0, b'b', 0, 7, 7, 0, 0, 9, 9]);
        let s = sttb(&d);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].0, "ab");
        assert_eq!(s[0].1, &[7, 7]);
        assert_eq!(s[1].0, "");
        // 2026-09-20 10:30.
        let v = 30 | 10 << 6 | 20 << 11 | 9 << 16 | 126 << 20;
        assert_eq!(dttm(v), "2026-09-20T10:30:00Z");
        assert_eq!(dttm(0), "");
    }
}
