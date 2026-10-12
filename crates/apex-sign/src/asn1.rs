//! A small ASN.1 reader (DER and the BER that signers still write:
//! indefinite lengths, segmented octet strings) and a DER writer: enough
//! for certificates, CMS signatures, PKCS#12 files and time-stamps.

use std::borrow::Cow;

use crate::Error;

pub const BOOLEAN: u8 = 0x01;
pub const INTEGER: u8 = 0x02;
pub const BIT_STRING: u8 = 0x03;
pub const OCTET_STRING: u8 = 0x04;
pub const NULL: u8 = 0x05;
pub const OID: u8 = 0x06;
pub const UTF8_STRING: u8 = 0x0C;
pub const PRINTABLE_STRING: u8 = 0x13;
pub const T61_STRING: u8 = 0x14;
pub const IA5_STRING: u8 = 0x16;
pub const UTC_TIME: u8 = 0x17;
pub const GENERALIZED_TIME: u8 = 0x18;
pub const BMP_STRING: u8 = 0x1E;
pub const SEQUENCE: u8 = 0x30;
pub const SET: u8 = 0x31;

/// One element: its tag (first identifier byte), its content and all of
/// its bytes as written.
#[derive(Clone, Copy, Debug)]
pub struct Tlv<'a> {
    pub tag: u8,
    pub body: &'a [u8],
    pub raw: &'a [u8],
}

fn bad() -> Error {
    Error::new("malformed ASN.1")
}

/// Reads the element at the start of `data`; returns it and its length.
fn read_one(data: &[u8], depth: u32) -> Result<(Tlv<'_>, usize), Error> {
    if depth > 64 {
        return Err(bad());
    }
    let tag = *data.first().ok_or_else(bad)?;
    let mut p = 1;
    if tag & 0x1F == 0x1F {
        // High tag numbers: skipped over, the first byte stands for them.
        while *data.get(p).ok_or_else(bad)? & 0x80 != 0 {
            p += 1;
        }
        p += 1;
    }
    let first = *data.get(p).ok_or_else(bad)?;
    p += 1;
    if first == 0x80 {
        // Indefinite length (BER): children up to the end-of-contents.
        if tag & 0x20 == 0 {
            return Err(bad());
        }
        let start = p;
        loop {
            if data.get(p..p + 2) == Some(&[0, 0]) {
                return Ok((Tlv { tag, body: &data[start..p], raw: &data[..p + 2] }, p + 2));
            }
            let (_, n) = read_one(&data[p..], depth + 1)?;
            p += n;
        }
    }
    let len = if first & 0x80 == 0 {
        first as usize
    } else {
        let n = (first & 0x7F) as usize;
        if n == 0 || n > 4 {
            return Err(bad());
        }
        let mut len = 0usize;
        for i in 0..n {
            len = (len << 8) | *data.get(p + i).ok_or_else(bad)? as usize;
        }
        p += n;
        len
    };
    let end = p.checked_add(len).filter(|&e| e <= data.len()).ok_or_else(bad)?;
    Ok((Tlv { tag, body: &data[p..end], raw: &data[..end] }, end))
}

/// The element at the start of `data` (bytes after it are ignored, like
/// the zero padding of a PDF signature).
pub fn parse(data: &[u8]) -> Result<Tlv<'_>, Error> {
    read_one(data, 0).map(|(t, _)| t)
}

/// The elements one after another.
#[derive(Clone)]
pub struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data }
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// The next element (`None` at the end or on garbage).
    pub fn next(&mut self) -> Option<Tlv<'a>> {
        if self.data.is_empty() {
            return None;
        }
        match read_one(self.data, 0) {
            Ok((t, n)) => {
                self.data = &self.data[n..];
                Some(t)
            }
            Err(_) => {
                self.data = &[];
                None
            }
        }
    }

    /// The next element, which must be there.
    pub fn req(&mut self) -> Result<Tlv<'a>, Error> {
        self.next().ok_or_else(bad)
    }

    pub fn peek_tag(&self) -> Option<u8> {
        self.data.first().copied()
    }

    /// The next element if it has `tag`.
    pub fn take(&mut self, tag: u8) -> Option<Tlv<'a>> {
        if self.peek_tag() == Some(tag) { self.next() } else { None }
    }
}

impl<'a> Tlv<'a> {
    pub fn children(&self) -> Reader<'a> {
        Reader::new(self.body)
    }

    pub fn expect(self, tag: u8) -> Result<Self, Error> {
        if self.tag == tag { Ok(self) } else { Err(bad()) }
    }

    /// The single element inside an explicitly tagged one.
    pub fn inner(&self) -> Result<Tlv<'a>, Error> {
        parse(self.body)
    }

    /// An object identifier in dotted form.
    pub fn oid(&self) -> String {
        let mut out = String::new();
        let mut v: u64 = 0;
        for &b in self.body {
            v = (v << 7) | (b & 0x7F) as u64;
            if b & 0x80 != 0 {
                continue;
            }
            if out.is_empty() {
                let (a, rest) = if v < 40 { (0, v) } else if v < 80 { (1, v - 40) } else { (2, v - 80) };
                out = format!("{a}.{rest}");
            } else {
                out.push('.');
                out.push_str(&v.to_string());
            }
            v = 0;
        }
        out
    }

    /// A non-negative integer that fits (iteration counts, versions).
    pub fn uint(&self) -> Option<u64> {
        let b = self.body;
        let b = if b.len() > 1 && b[0] == 0 { &b[1..] } else { b };
        if b.len() > 8 || b.first().is_some_and(|&x| x & 0x80 != 0 && self.body.len() == b.len()) {
            return None;
        }
        Some(b.iter().fold(0u64, |a, &x| (a << 8) | x as u64))
    }

    /// The content of an octet string, joining the segments of a
    /// constructed one.
    pub fn octets(&self) -> Cow<'a, [u8]> {
        if self.tag & 0x20 == 0 {
            return Cow::Borrowed(self.body);
        }
        let mut out = Vec::new();
        self.octets_into(&mut out, 0);
        Cow::Owned(out)
    }

    /// Segments nested deeper than any encoder makes them are dropped
    /// (thousands of levels would overflow the stack).
    fn octets_into(&self, out: &mut Vec<u8>, depth: u32) {
        if self.tag & 0x20 == 0 {
            out.extend_from_slice(self.body);
            return;
        }
        if depth > 16 {
            return;
        }
        let mut r = self.children();
        while let Some(t) = r.next() {
            t.octets_into(out, depth + 1);
        }
    }

    /// The bits of a bit string (its whole bytes).
    pub fn bits(&self) -> &'a [u8] {
        self.body.get(1..).unwrap_or(&[])
    }

    /// A character string of any of the usual kinds.
    pub fn string(&self) -> String {
        match self.tag {
            BMP_STRING => {
                let units: Vec<u16> = self.body.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                String::from_utf16_lossy(&units)
            }
            0x1C => self.body.chunks_exact(4).filter_map(|c| char::from_u32(u32::from_be_bytes([c[0], c[1], c[2], c[3]]))).collect(),
            T61_STRING if std::str::from_utf8(self.body).is_err() => self.body.iter().map(|&b| b as char).collect(),
            _ => String::from_utf8_lossy(self.body).into_owned(),
        }
    }

    /// A UTCTime or GeneralizedTime as Unix seconds.
    pub fn time(&self) -> Option<i64> {
        let s = std::str::from_utf8(self.body).ok()?;
        let (year, rest) = match self.tag {
            UTC_TIME => {
                let y: i64 = s.get(0..2)?.parse().ok()?;
                (if y < 50 { 2000 + y } else { 1900 + y }, s.get(2..)?)
            }
            GENERALIZED_TIME => (s.get(0..4)?.parse().ok()?, s.get(4..)?),
            _ => return None,
        };
        let num = |i: usize| -> Option<i64> { rest.get(i..i + 2)?.parse().ok() };
        let (mo, d, h, mi) = (num(0)?, num(2)?, num(4)?, num(6)?);
        let sec = num(8).unwrap_or(0);
        // Fractions of a second, then Z or an offset.
        let mut tail = rest.get(if rest.len() >= 10 && rest.as_bytes()[8].is_ascii_digit() { 10 } else { 8 }..).unwrap_or("");
        if let Some(t) = tail.strip_prefix('.') {
            tail = t.trim_start_matches(|c: char| c.is_ascii_digit());
        }
        let offset = match tail.as_bytes().first() {
            Some(b'+') | Some(b'-') => {
                let sign = if tail.starts_with('-') { -1 } else { 1 };
                let oh: i64 = tail.get(1..3)?.parse().ok()?;
                let om: i64 = tail.get(3..5).and_then(|m| m.parse().ok()).unwrap_or(0);
                sign * (oh * 3600 + om * 60)
            }
            _ => 0,
        };
        Some(crate::time::unix(year, mo, d, h, mi, sec)? - offset)
    }
}

// --- writing (DER) -------------------------------------------------------------------

fn put_len(out: &mut Vec<u8>, n: usize) {
    if n < 0x80 {
        out.push(n as u8);
    } else {
        let bytes = n.to_be_bytes();
        let skip = bytes.iter().take_while(|&&b| b == 0).count();
        out.push(0x80 | (bytes.len() - skip) as u8);
        out.extend_from_slice(&bytes[skip..]);
    }
}

pub fn tlv(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 6);
    out.push(tag);
    put_len(&mut out, body.len());
    out.extend_from_slice(body);
    out
}

pub fn seq(parts: &[&[u8]]) -> Vec<u8> {
    tlv(SEQUENCE, &parts.concat())
}

/// A SET OF, its elements sorted as DER wants.
pub fn set_of(parts: &[&[u8]]) -> Vec<u8> {
    let mut v: Vec<&[u8]> = parts.to_vec();
    v.sort();
    tlv(SET, &v.concat())
}

pub fn oid(dotted: &str) -> Vec<u8> {
    let n: Vec<u64> = dotted.split('.').filter_map(|p| p.parse().ok()).collect();
    let mut body = Vec::new();
    let mut put = |mut v: u64| {
        let mut tmp = vec![(v & 0x7F) as u8];
        v >>= 7;
        while v > 0 {
            tmp.push(0x80 | (v & 0x7F) as u8);
            v >>= 7;
        }
        tmp.reverse();
        body.extend_from_slice(&tmp);
    };
    if n.len() >= 2 {
        put(n[0] * 40 + n[1]);
    }
    for &v in n.iter().skip(2) {
        put(v);
    }
    tlv(OID, &body)
}

/// An integer from big-endian magnitude bytes (non-negative).
pub fn uint_bytes(b: &[u8]) -> Vec<u8> {
    let skip = b.iter().take_while(|&&x| x == 0).count().min(b.len().saturating_sub(1));
    let b = &b[skip..];
    if b.is_empty() {
        return tlv(INTEGER, &[0]);
    }
    if b[0] & 0x80 != 0 {
        let mut v = vec![0];
        v.extend_from_slice(b);
        tlv(INTEGER, &v)
    } else {
        tlv(INTEGER, b)
    }
}

pub fn uint(v: u64) -> Vec<u8> {
    uint_bytes(&v.to_be_bytes())
}

pub fn octets(b: &[u8]) -> Vec<u8> {
    tlv(OCTET_STRING, b)
}

pub fn null() -> Vec<u8> {
    vec![NULL, 0]
}

pub fn boolean(v: bool) -> Vec<u8> {
    vec![BOOLEAN, 1, if v { 0xFF } else { 0 }]
}

pub fn utf8(s: &str) -> Vec<u8> {
    tlv(UTF8_STRING, s.as_bytes())
}

pub fn bit_string(b: &[u8]) -> Vec<u8> {
    let mut v = vec![0];
    v.extend_from_slice(b);
    tlv(BIT_STRING, &v)
}

/// A context-specific constructed tag ([n] around the parts).
pub fn ctx(n: u8, parts: &[&[u8]]) -> Vec<u8> {
    tlv(0xA0 | n, &parts.concat())
}

/// An AlgorithmIdentifier, with NULL parameters or none.
pub fn alg(dotted: &str, null_params: bool) -> Vec<u8> {
    if null_params { seq(&[&oid(dotted), &null()]) } else { seq(&[&oid(dotted)]) }
}

pub fn generalized_time(t: i64) -> Vec<u8> {
    let (y, mo, d, h, mi, s) = crate::time::civil(t);
    tlv(GENERALIZED_TIME, format!("{y:04}{mo:02}{d:02}{h:02}{mi:02}{s:02}Z").as_bytes())
}

pub fn utc_time(t: i64) -> Vec<u8> {
    let (y, mo, d, h, mi, s) = crate::time::civil(t);
    tlv(UTC_TIME, format!("{:02}{mo:02}{d:02}{h:02}{mi:02}{s:02}Z", y % 100).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oids_round_trip() {
        for o in ["1.2.840.113549.1.7.2", "2.5.4.3", "1.3.6.1.4.1.311.10.3.12", "2.16.840.1.101.3.4.2.1"] {
            assert_eq!(parse(&oid(o)).unwrap().oid(), o);
        }
    }

    #[test]
    fn integers_and_lengths() {
        assert_eq!(uint(0), [2, 1, 0]);
        assert_eq!(uint(128), [2, 2, 0, 128]);
        assert_eq!(parse(&uint(65537)).unwrap().uint(), Some(65537));
        let long = octets(&[7u8; 300]);
        assert_eq!(&long[..4], &[4, 0x82, 1, 44]);
        assert_eq!(parse(&long).unwrap().body.len(), 300);
    }

    #[test]
    fn ber_indefinite_and_segments() {
        // SEQUENCE (indefinite) { OCTET STRING constructed (indefinite) { "ab", "c" } }
        let ber = [0x30, 0x80, 0x24, 0x80, 4, 2, b'a', b'b', 4, 1, b'c', 0, 0, 0, 0, 0xFF];
        let t = parse(&ber).unwrap();
        assert_eq!(t.raw.len(), 15);
        let inner = t.children().next().unwrap();
        assert_eq!(&*inner.octets(), b"abc");
    }

    #[test]
    fn times() {
        let t = parse(&[UTC_TIME, 13, b'2', b'6', b'0', b'9', b'2', b'6', b'1', b'2', b'0', b'0', b'0', b'0', b'Z']).unwrap();
        assert_eq!(t.time(), crate::time::unix(2026, 9, 26, 12, 0, 0));
        let g = generalized_time(1_790_000_000);
        assert_eq!(parse(&g).unwrap().time(), Some(1_790_000_000));
        let off = tlv(GENERALIZED_TIME, b"20260926140000.5+0200");
        assert_eq!(parse(&off).unwrap().time(), crate::time::unix(2026, 9, 26, 12, 0, 0));
    }
}
