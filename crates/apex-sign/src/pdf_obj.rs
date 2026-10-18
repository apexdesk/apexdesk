//! Just enough of the PDF object syntax for signatures: reading objects
//! and the classic cross-reference tables, and writing objects back.

use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Obj {
    Null,
    Bool(bool),
    /// A number as written.
    Num(String),
    /// A name without its slash, as written.
    Name(String),
    Str(Vec<u8>),
    Array(Vec<Obj>),
    Dict(Vec<(String, Obj)>),
    Ref(u32, u16),
}

fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'\x0C' | 0)
}

fn is_delim(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

/// Skips white space and comments.
pub fn skip_ws(d: &[u8], p: &mut usize) {
    while *p < d.len() {
        if is_ws(d[*p]) {
            *p += 1;
        } else if d[*p] == b'%' {
            while *p < d.len() && d[*p] != b'\n' && d[*p] != b'\r' {
                *p += 1;
            }
        } else {
            break;
        }
    }
}

fn token<'a>(d: &'a [u8], p: &mut usize) -> &'a [u8] {
    // Offsets come from the file: one past its end reads nothing.
    *p = (*p).min(d.len());
    let s = *p;
    while *p < d.len() && !is_ws(d[*p]) && !is_delim(d[*p]) {
        *p += 1;
    }
    &d[s..*p]
}

/// A name as readers take it, its `#xx` escapes decoded (else `/A#50`
/// would be another key here than the `/AP` they read). One that doesn't
/// decode to text, or holds a `#` itself, is kept as written.
fn name(raw: &[u8]) -> String {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match (raw[i], raw.get(i + 1).copied().and_then(hex), raw.get(i + 2).copied().and_then(hex)) {
            (b'#', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            (b, ..) => {
                out.push(b);
                i += 1;
            }
        }
    }
    match String::from_utf8(out) {
        Ok(s) if !s.contains('#') || !raw.contains(&b'#') => s,
        _ => String::from_utf8_lossy(raw).into_owned(),
    }
}

fn literal(d: &[u8], p: &mut usize) -> Option<Vec<u8>> {
    *p += 1;
    let mut out = Vec::new();
    let mut depth = 0;
    while *p < d.len() {
        let c = d[*p];
        *p += 1;
        match c {
            b'\\' => {
                let e = *d.get(*p)?;
                *p += 1;
                match e {
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'b' => out.push(8),
                    b'f' => out.push(12),
                    b'0'..=b'7' => {
                        let mut v = (e - b'0') as u32;
                        for _ in 0..2 {
                            match d.get(*p) {
                                Some(&x @ b'0'..=b'7') => {
                                    v = v * 8 + (x - b'0') as u32;
                                    *p += 1;
                                }
                                _ => break,
                            }
                        }
                        out.push(v as u8);
                    }
                    b'\r' => {
                        if d.get(*p) == Some(&b'\n') {
                            *p += 1;
                        }
                    }
                    b'\n' => {}
                    x => out.push(x),
                }
            }
            b'(' => {
                depth += 1;
                out.push(c);
            }
            b')' if depth == 0 => return Some(out),
            b')' => {
                depth -= 1;
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    None
}

fn hex(d: &[u8], p: &mut usize) -> Option<Vec<u8>> {
    *p += 1;
    let mut digits = Vec::new();
    while *p < d.len() && d[*p] != b'>' {
        let c = d[*p];
        if let Some(v) = (c as char).to_digit(16) {
            digits.push(v as u8);
        }
        *p += 1;
    }
    // Unterminated.
    if *p >= d.len() {
        return None;
    }
    *p += 1;
    if digits.len() % 2 == 1 {
        digits.push(0);
    }
    Some(digits.chunks(2).map(|c| c[0] << 4 | c[1]).collect())
}

/// Parses the object at `*p` (moving past it).
pub fn parse(d: &[u8], p: &mut usize) -> Option<Obj> {
    parse_depth(d, p, 0)
}

fn parse_depth(d: &[u8], p: &mut usize, depth: u32) -> Option<Obj> {
    if depth > 64 {
        return None;
    }
    skip_ws(d, p);
    let c = *d.get(*p)?;
    match c {
        b'/' => {
            *p += 1;
            Some(Obj::Name(name(token(d, p))))
        }
        b'(' => literal(d, p).map(Obj::Str),
        b'<' if d.get(*p + 1) == Some(&b'<') => {
            *p += 2;
            let mut items: Vec<(String, Obj)> = Vec::new();
            // A key given twice keeps its last value, as the viewer and
            // other readers take it.
            let mut at: HashMap<String, usize> = HashMap::new();
            loop {
                skip_ws(d, p);
                if d.get(*p..*p + 2) == Some(b">>") {
                    *p += 2;
                    return Some(Obj::Dict(items));
                }
                let Obj::Name(k) = parse_depth(d, p, depth + 1)? else { return None };
                let v = parse_depth(d, p, depth + 1)?;
                match at.get(&k) {
                    Some(&i) => items[i].1 = v,
                    None => {
                        at.insert(k.clone(), items.len());
                        items.push((k, v));
                    }
                }
            }
        }
        b'<' => hex(d, p).map(Obj::Str),
        b'[' => {
            *p += 1;
            let mut items = Vec::new();
            loop {
                skip_ws(d, p);
                if d.get(*p) == Some(&b']') {
                    *p += 1;
                    return Some(Obj::Array(items));
                }
                items.push(parse_depth(d, p, depth + 1)?);
            }
        }
        _ => {
            let t = token(d, p);
            match t {
                b"null" => Some(Obj::Null),
                b"true" => Some(Obj::Bool(true)),
                b"false" => Some(Obj::Bool(false)),
                _ if !t.is_empty() && t.iter().all(|&b| b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'+')) => {
                    // "12 0 R" is a reference.
                    let save = *p;
                    if !t.contains(&b'.') && !t.starts_with(b"-") {
                        let mut q = *p;
                        skip_ws(d, &mut q);
                        let g = token(d, &mut q);
                        if !g.is_empty() && g.iter().all(u8::is_ascii_digit) {
                            skip_ws(d, &mut q);
                            if d.get(q) == Some(&b'R') && d.get(q + 1).is_none_or(|&b| is_ws(b) || is_delim(b)) {
                                *p = q + 1;
                                let n = std::str::from_utf8(t).ok()?.parse().ok()?;
                                let g = std::str::from_utf8(g).ok()?.parse().ok()?;
                                return Some(Obj::Ref(n, g));
                            }
                        }
                    }
                    *p = save;
                    Some(Obj::Num(String::from_utf8_lossy(t).into_owned()))
                }
                _ => None,
            }
        }
    }
}

impl Obj {
    pub fn get(&self, key: &str) -> Option<&Obj> {
        match self {
            Obj::Dict(items) => items.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn set(&mut self, key: &str, v: Obj) {
        if let Obj::Dict(items) = self {
            match items.iter_mut().find(|(k, _)| k == key) {
                Some(slot) => slot.1 = v,
                None => items.push((key.to_string(), v)),
            }
        }
    }

    pub fn name(&self) -> Option<&str> {
        if let Obj::Name(n) = self { Some(n) } else { None }
    }

    pub fn int(&self) -> Option<i64> {
        if let Obj::Num(n) = self { n.parse().ok() } else { None }
    }

    pub fn reference(&self) -> Option<(u32, u16)> {
        if let Obj::Ref(n, g) = self { Some((*n, *g)) } else { None }
    }

    /// A text string: UTF-16 with a byte order mark, UTF-8 with one, or
    /// PDFDocEncoding (taken as Latin-1).
    pub fn text(&self) -> Option<String> {
        let Obj::Str(b) = self else { return None };
        Some(if let Some(u) = b.strip_prefix(&[0xFE, 0xFF]) {
            String::from_utf16_lossy(&u.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect::<Vec<_>>())
        } else if let Some(u) = b.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
            String::from_utf8_lossy(u).into_owned()
        } else {
            b.iter().map(|&c| c as char).collect()
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut out);
        out
    }

    pub fn write(&self, out: &mut Vec<u8>) {
        match self {
            Obj::Null => out.extend_from_slice(b"null"),
            Obj::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
            Obj::Num(n) => out.extend_from_slice(n.as_bytes()),
            Obj::Name(n) => {
                out.push(b'/');
                // What isn't a regular character escaped (a name kept as
                // written already is, its `#` left as it was).
                for &b in n.as_bytes() {
                    if (0x21..0x7F).contains(&b) && !is_delim(b) {
                        out.push(b);
                    } else {
                        out.extend_from_slice(format!("#{b:02X}").as_bytes());
                    }
                }
            }
            Obj::Str(s) if s.iter().all(|b| (0x20..0x7F).contains(b)) => {
                out.push(b'(');
                for &b in s {
                    if matches!(b, b'(' | b')' | b'\\') {
                        out.push(b'\\');
                    }
                    out.push(b);
                }
                out.push(b')');
            }
            Obj::Str(s) => {
                out.push(b'<');
                for b in s {
                    out.extend_from_slice(format!("{b:02X}").as_bytes());
                }
                out.push(b'>');
            }
            Obj::Array(items) => {
                out.push(b'[');
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(b' ');
                    }
                    v.write(out);
                }
                out.push(b']');
            }
            Obj::Dict(items) => {
                out.extend_from_slice(b"<<");
                for (k, v) in items {
                    out.push(b'/');
                    out.extend_from_slice(k.as_bytes());
                    out.push(b' ');
                    v.write(out);
                }
                out.extend_from_slice(b">>");
            }
            Obj::Ref(n, g) => out.extend_from_slice(format!("{n} {g} R").as_bytes()),
        }
    }
}

/// A text string for writing: ASCII as is, anything else in UTF-16.
pub fn text_str(s: &str) -> Obj {
    if s.is_ascii() {
        Obj::Str(s.as_bytes().to_vec())
    } else {
        let mut b = vec![0xFE, 0xFF];
        b.extend(s.encode_utf16().flat_map(|u| u.to_be_bytes()));
        Obj::Str(b)
    }
}

/// The indirect object `n G obj … endobj` at `offset`.
pub fn object_at(d: &[u8], offset: usize) -> Option<(u32, u16, Obj)> {
    let mut p = offset;
    skip_ws(d, &mut p);
    let n: u32 = std::str::from_utf8(token(d, &mut p)).ok()?.parse().ok()?;
    skip_ws(d, &mut p);
    let g: u16 = std::str::from_utf8(token(d, &mut p)).ok()?.parse().ok()?;
    skip_ws(d, &mut p);
    if token(d, &mut p) != b"obj" {
        return None;
    }
    Some((n, g, parse(d, &mut p)?))
}

/// The object at `offset` and whether it is a stream (its dictionary is
/// followed by data).
pub fn object_and_kind_at(d: &[u8], offset: usize) -> Option<(u32, u16, Obj, bool)> {
    let mut p = offset;
    skip_ws(d, &mut p);
    let n: u32 = std::str::from_utf8(token(d, &mut p)).ok()?.parse().ok()?;
    skip_ws(d, &mut p);
    let g: u16 = std::str::from_utf8(token(d, &mut p)).ok()?.parse().ok()?;
    skip_ws(d, &mut p);
    if token(d, &mut p) != b"obj" {
        return None;
    }
    let o = parse(d, &mut p)?;
    skip_ws(d, &mut p);
    let stream = token(d, &mut p) == b"stream";
    Some((n, g, o, stream))
}

/// The objects of a file with classic cross-reference tables.
pub struct Xref {
    pub offsets: HashMap<u32, (usize, u16)>,
    /// The newest trailer.
    pub trailer: Obj,
    /// Where the newest table starts.
    pub start: usize,
    /// Objects a table marks free (but object 0), with where that table
    /// starts.
    pub freed: Vec<(u32, usize)>,
    /// Every table starts right where it is said to: readers that find
    /// none there (white space or a comment first) repair the file, and
    /// read it otherwise.
    pub exact: bool,
    /// Some trailer points at a cross-reference stream as well
    /// (`/XRefStm`), whose entries readers take over these tables'.
    pub hybrid: bool,
}

/// The offset after the last `startxref`.
pub fn startxref(d: &[u8]) -> Option<usize> {
    let tail_from = d.len().saturating_sub(2048);
    let at = d[tail_from..].windows(9).rposition(|w| w == b"startxref")? + tail_from + 9;
    let mut p = at;
    skip_ws(d, &mut p);
    std::str::from_utf8(token(d, &mut p)).ok()?.parse().ok()
}

/// Reads the cross-reference tables (following /Prev); `None` for files
/// with cross-reference streams.
pub fn read_xref(d: &[u8]) -> Option<Xref> {
    let start = startxref(d)?;
    let mut offsets = HashMap::new();
    let mut freed = Vec::new();
    let mut trailer = None;
    let mut next = Some(start);
    let mut seen = Vec::new();
    let mut exact = true;
    let mut hybrid = false;
    while let Some(at) = next.take() {
        if seen.contains(&at) {
            break;
        }
        // More sections than are followed here: readers going further see
        // objects these don't list.
        if seen.len() > 1000 {
            exact = false;
            break;
        }
        seen.push(at);
        exact &= d.get(at..).is_some_and(|r| r.starts_with(b"xref"));
        let mut p = at;
        skip_ws(d, &mut p);
        if token(d, &mut p) != b"xref" {
            return None;
        }
        // A number a table lists twice: readers take either entry.
        let mut listed = std::collections::HashSet::new();
        loop {
            skip_ws(d, &mut p);
            let t = token(d, &mut p);
            if t == b"trailer" {
                break;
            }
            let first: u32 = std::str::from_utf8(t).ok()?.parse().ok()?;
            skip_ws(d, &mut p);
            let count: u32 = std::str::from_utf8(token(d, &mut p)).ok()?.parse().ok()?;
            for i in 0..count {
                skip_ws(d, &mut p);
                let off: usize = std::str::from_utf8(token(d, &mut p)).ok()?.parse().ok()?;
                skip_ws(d, &mut p);
                let g: u16 = std::str::from_utf8(token(d, &mut p)).ok()?.parse().ok()?;
                skip_ws(d, &mut p);
                let kind = token(d, &mut p);
                let n = first.checked_add(i)?;
                exact &= listed.insert(n);
                if kind == b"n" {
                    offsets.entry(n).or_insert((off, g));
                } else if n != 0 {
                    freed.push((n, at));
                }
            }
        }
        let t = parse(d, &mut p)?;
        hybrid |= t.get("XRefStm").is_some();
        next = t.get("Prev").and_then(Obj::int).and_then(|v| usize::try_from(v).ok());
        if trailer.is_none() {
            trailer = Some(t);
        }
    }
    Some(Xref { offsets, trailer: trailer?, start, freed, exact, hybrid })
}

impl Xref {
    pub fn get(&self, d: &[u8], n: u32) -> Option<(u16, Obj)> {
        let &(off, _) = self.offsets.get(&n)?;
        let (m, g, o) = object_at(d, off)?;
        (m == n).then_some((g, o))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_twice_keeps_its_last_value() {
        let d = parse(b"<< /Contents 5 0 R /Type /Page /Contents 6 0 R >>", &mut 0).unwrap();
        assert_eq!(d.get("Contents"), Some(&Obj::Ref(6, 0)));
        assert!(matches!(&d, Obj::Dict(items) if items.len() == 2));
    }

    #[test]
    fn objects() {
        let d = b"<< /Type /Sig /ByteRange [0 10 20 30] /Name (A \\(b\\) \\101) /R 12 0 R /X <48 69> /N -1.5 >>";
        let o = parse(d, &mut 0).unwrap();
        assert_eq!(o.get("Type").and_then(Obj::name), Some("Sig"));
        assert_eq!(o.get("Name").and_then(Obj::text).as_deref(), Some("A (b) A"));
        assert_eq!(o.get("R").and_then(Obj::reference), Some((12, 0)));
        assert_eq!(o.get("X"), Some(&Obj::Str(b"Hi".to_vec())));
        assert_eq!(o.get("ByteRange"), Some(&Obj::Array(vec![Obj::Num("0".into()), Obj::Num("10".into()), Obj::Num("20".into()), Obj::Num("30".into())])));
        let mut out = Vec::new();
        o.write(&mut out);
        assert_eq!(parse(&out, &mut 0).unwrap(), o);
    }
}
