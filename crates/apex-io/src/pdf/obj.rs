//! PDF objects and their syntax: the lexer shared by the file structure
//! and the content streams.

use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub enum Obj {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    /// A name without its slash, with `#xx` escapes resolved.
    Name(Arc<str>),
    Str(Vec<u8>),
    Array(Vec<Obj>),
    Dict(Dict),
    Ref(u32, u16),
    /// A stream: its dictionary and its bytes as stored (still encoded).
    Stream(Dict, Arc<[u8]>),
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Dict(pub Vec<(Arc<str>, Obj)>);

impl Dict {
    pub fn get(&self, key: &str) -> Option<&Obj> {
        self.0.iter().find(|(k, _)| &**k == key).map(|(_, v)| v)
    }

    /// Sets `key` (replacing its value, or adding it at the end).
    pub fn set(&mut self, key: &str, v: Obj) {
        match self.0.iter_mut().find(|(k, _)| &**k == key) {
            Some(e) => e.1 = v,
            None => self.0.push((Arc::from(key), v)),
        }
    }

    pub fn remove(&mut self, key: &str) -> Option<Obj> {
        let i = self.0.iter().position(|(k, _)| &**k == key)?;
        Some(self.0.remove(i).1)
    }

    /// A dictionary from its entries.
    pub fn of(items: Vec<(&str, Obj)>) -> Dict {
        Dict(items.into_iter().map(|(k, v)| (Arc::from(k), v)).collect())
    }
}

impl Obj {
    /// The objects it is made of, itself included (what copying it costs).
    pub fn weight(&self) -> usize {
        match self {
            Obj::Array(a) => a.iter().fold(1usize, |w, o| w.saturating_add(o.weight())),
            Obj::Dict(d) | Obj::Stream(d, _) => d.0.iter().fold(1usize, |w, (_, o)| w.saturating_add(o.weight())),
            // Long strings weigh as their size: they're copied whole.
            Obj::Str(s) => 1 + s.len() / 16,
            _ => 1,
        }
    }

    pub fn num(&self) -> Option<f64> {
        match self {
            Obj::Int(i) => Some(*i as f64),
            Obj::Real(r) => Some(*r),
            _ => None,
        }
    }

    pub fn int(&self) -> Option<i64> {
        match self {
            Obj::Int(i) => Some(*i),
            Obj::Real(r) => Some(*r as i64),
            _ => None,
        }
    }

    pub fn name(&self) -> Option<&str> {
        match self {
            Obj::Name(n) => Some(n),
            _ => None,
        }
    }

    pub fn dict(&self) -> Option<&Dict> {
        match self {
            Obj::Dict(d) | Obj::Stream(d, _) => Some(d),
            _ => None,
        }
    }

    pub fn array(&self) -> Option<&[Obj]> {
        match self {
            Obj::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            Obj::Str(s) => Some(s),
            _ => None,
        }
    }

    /// A name object.
    pub fn name_of(s: &str) -> Obj {
        Obj::Name(Arc::from(s))
    }

    /// A number: an integer when it is one.
    pub fn num_of(v: f64) -> Obj {
        if v.fract() == 0.0 && v.abs() < 1e15 { Obj::Int(v as i64) } else { Obj::Real(v) }
    }
}

pub fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'\x0C' | 0)
}

pub fn is_delim(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

/// What the lexer reads: an object, or a bare keyword (an operator in a
/// content stream, `obj`, `R`, `stream`...).
#[derive(Debug)]
pub enum Item<'a> {
    Obj(Obj),
    Kw(&'a [u8]),
}

/// The most items an array or dictionary keeps (the rest are read past).
const MAX_ITEMS: usize = 1 << 20;

pub struct Lexer<'a> {
    pub d: &'a [u8],
    pub p: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(d: &'a [u8], p: usize) -> Self {
        Lexer { d, p }
    }

    pub fn skip_ws(&mut self) {
        let d = self.d;
        while self.p < d.len() {
            if is_ws(d[self.p]) {
                self.p += 1;
            } else if d[self.p] == b'%' {
                while self.p < d.len() && d[self.p] != b'\n' && d[self.p] != b'\r' {
                    self.p += 1;
                }
            } else {
                break;
            }
        }
    }

    fn token(&mut self) -> &'a [u8] {
        let d = self.d;
        let s = self.p;
        while self.p < d.len() && !is_ws(d[self.p]) && !is_delim(d[self.p]) {
            self.p += 1;
        }
        &d[s..self.p]
    }

    /// The next object or keyword; `refs` reads `n g R` as a reference
    /// (in files; content streams have none).
    pub fn next(&mut self, refs: bool) -> Option<Item<'a>> {
        self.next_depth(refs, 0)
    }

    fn next_depth(&mut self, refs: bool, depth: u32) -> Option<Item<'a>> {
        if depth > 100 {
            return None;
        }
        self.skip_ws();
        let d = self.d;
        let c = *d.get(self.p)?;
        Some(Item::Obj(match c {
            b'/' => {
                self.p += 1;
                Obj::Name(name(self.token()))
            }
            b'(' => Obj::Str(self.literal()),
            b'<' if d.get(self.p + 1) == Some(&b'<') => {
                self.p += 2;
                let mut items: Vec<(Arc<str>, Obj)> = Vec::new();
                // A key given twice keeps its last value, as other readers
                // (pdf.js, pdfium) take it: a signature checked here then
                // covers what they show.
                let mut at: std::collections::HashMap<Arc<str>, usize> = std::collections::HashMap::new();
                let mut put = |items: &mut Vec<(Arc<str>, Obj)>, key: Arc<str>, v: Obj| match at.get(&key) {
                    Some(&i) => items[i].1 = v,
                    None if items.len() < MAX_ITEMS => {
                        at.insert(key.clone(), items.len());
                        items.push((key, v));
                    }
                    None => {}
                };
                loop {
                    self.skip_ws();
                    match d.get(self.p) {
                        None => break,
                        Some(b'>') => {
                            self.p += if d.get(self.p + 1) == Some(&b'>') { 2 } else { 1 };
                            break;
                        }
                        // "<<" where a key goes: a stray word, as pdfium
                        // skips it (not a dictionary).
                        Some(b'<') if d.get(self.p + 1) == Some(&b'<') => {
                            self.p += 2;
                            continue;
                        }
                        _ => {}
                    }
                    let key = match self.next_depth(refs, depth + 1)? {
                        Item::Obj(Obj::Name(k)) => k,
                        // A stray token: skipped.
                        Item::Kw(k) if k.is_empty() => {
                            self.p += 1;
                            continue;
                        }
                        _ => continue,
                    };
                    self.skip_ws();
                    if d.get(self.p) == Some(&b'>') {
                        put(&mut items, key, Obj::Null);
                        continue;
                    }
                    let v = match self.next_depth(refs, depth + 1)? {
                        Item::Obj(o) => o,
                        Item::Kw(k) if k.is_empty() => {
                            self.p += 1;
                            Obj::Null
                        }
                        Item::Kw(_) => Obj::Null,
                    };
                    put(&mut items, key, v);
                }
                Obj::Dict(Dict(items))
            }
            b'<' => Obj::Str(self.hex()),
            b'[' => {
                self.p += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_ws();
                    match d.get(self.p) {
                        None => break,
                        Some(b']') => {
                            self.p += 1;
                            break;
                        }
                        _ => {}
                    }
                    match self.next_depth(refs, depth + 1)? {
                        Item::Obj(o) if items.len() < MAX_ITEMS => items.push(o),
                        Item::Obj(_) => {}
                        Item::Kw(k) if k.is_empty() => self.p += 1,
                        Item::Kw(b"R") if refs => {}
                        Item::Kw(_) => {}
                    }
                }
                Obj::Array(items)
            }
            b'+' | b'-' | b'.' | b'0'..=b'9' => {
                let t = self.token();
                let n = number(t);
                if refs && let Obj::Int(num) = n
                    && num >= 0
                    && let Some(r) = self.reference(num)
                {
                    return Some(Item::Obj(r));
                }
                n
            }
            b')' | b'>' | b']' | b'{' | b'}' => {
                // Stray delimiters: a one-byte keyword.
                self.p += 1;
                return Some(Item::Kw(&d[self.p - 1..self.p]));
            }
            _ => {
                let t = self.token();
                match t {
                    b"true" => Obj::Bool(true),
                    b"false" => Obj::Bool(false),
                    b"null" => Obj::Null,
                    _ => return Some(Item::Kw(t)),
                }
            }
        }))
    }

    /// `g R` after an object number.
    fn reference(&mut self, num: i64) -> Option<Obj> {
        let save = self.p;
        self.skip_ws();
        let g = self.token();
        if !g.is_empty() && g.iter().all(u8::is_ascii_digit) {
            self.skip_ws();
            if self.d.get(self.p) == Some(&b'R') && self.d.get(self.p + 1).is_none_or(|&b| is_ws(b) || is_delim(b)) {
                self.p += 1;
                let g = std::str::from_utf8(g).ok()?.parse::<u32>().ok()?;
                return Some(Obj::Ref(num as u32, g.min(u16::MAX as u32) as u16));
            }
        }
        self.p = save;
        None
    }

    fn literal(&mut self) -> Vec<u8> {
        let d = self.d;
        self.p += 1;
        let mut out = Vec::new();
        let mut depth = 0;
        while self.p < d.len() {
            let c = d[self.p];
            self.p += 1;
            match c {
                b'\\' => {
                    let Some(&e) = d.get(self.p) else { break };
                    self.p += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'0'..=b'7' => {
                            let mut v = (e - b'0') as u32;
                            for _ in 0..2 {
                                match d.get(self.p) {
                                    Some(&x @ b'0'..=b'7') => {
                                        v = v * 8 + (x - b'0') as u32;
                                        self.p += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push(v as u8);
                        }
                        b'\r' => {
                            if d.get(self.p) == Some(&b'\n') {
                                self.p += 1;
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
                b')' if depth == 0 => break,
                b')' => {
                    depth -= 1;
                    out.push(c);
                }
                _ => out.push(c),
            }
        }
        out
    }

    fn hex(&mut self) -> Vec<u8> {
        let d = self.d;
        self.p += 1;
        let mut out = Vec::new();
        let mut hi: Option<u8> = None;
        while self.p < d.len() && d[self.p] != b'>' {
            if let Some(v) = (d[self.p] as char).to_digit(16) {
                match hi.take() {
                    Some(h) => out.push(h << 4 | v as u8),
                    None => hi = Some(v as u8),
                }
            }
            self.p += 1;
        }
        self.p += 1;
        if let Some(h) = hi {
            out.push(h << 4);
        }
        out
    }
}

fn name(t: &[u8]) -> Arc<str> {
    // Most names: no escapes, plain ASCII.
    if t.iter().all(|&b| b != b'#' && b < 0x80) {
        return Arc::from(std::str::from_utf8(t).unwrap_or_default());
    }
    let mut out = Vec::with_capacity(t.len());
    let mut i = 0;
    while i < t.len() {
        if t[i] == b'#' && i + 2 < t.len() {
            if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&t[i + 1..i + 3]).unwrap_or("zz"), 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(t[i]);
        i += 1;
    }
    // Not UTF-8: each byte as the Latin-1 character, so its bytes stay
    // known (font names in GBK and the like).
    match String::from_utf8(out) {
        Ok(s) => Arc::from(s),
        Err(e) => Arc::from(e.into_bytes().iter().map(|&b| b as char).collect::<String>()),
    }
}

/// A number token, tolerant of the odd forms real files have (`--5`,
/// `5.-`, `.`).
pub fn number(t: &[u8]) -> Obj {
    // The usual forms: one sign at most, digits and one dot at most.
    let body = t.strip_prefix(b"-").or_else(|| t.strip_prefix(b"+")).unwrap_or(t);
    let dots = body.iter().filter(|&&b| b == b'.').count();
    if body.iter().any(u8::is_ascii_digit) && body.iter().all(|&b| b.is_ascii_digit() || b == b'.') && dots <= 1 && body.len() <= 18 {
        let neg = t[0] == b'-';
        if dots == 0 {
            let v = body.iter().fold(0i64, |v, &b| v * 10 + (b - b'0') as i64);
            return Obj::Int(if neg { -v } else { v });
        }
        if let Ok(v) = std::str::from_utf8(body).unwrap_or_default().parse::<f64>() {
            return Obj::Real(if neg { -v } else { v });
        }
    }
    let s = String::from_utf8_lossy(t);
    let neg = s.starts_with('-') && !s[1..].starts_with('-') || s.starts_with("--");
    let body: String = s.trim_start_matches(['+', '-']).chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();
    if !body.contains('.') {
        let v = body.parse::<i64>().unwrap_or(0);
        return Obj::Int(if neg { -v } else { v });
    }
    let mut parts = body.splitn(2, '.');
    let a = parts.next().unwrap_or("");
    let b: String = parts.next().unwrap_or("").chars().filter(|c| *c != '.').collect();
    let v = format!("{}.{}", if a.is_empty() { "0" } else { a }, if b.is_empty() { "0" } else { &b }).parse::<f64>().unwrap_or(0.0);
    Obj::Real(if neg { -v } else { v })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(s: &str) -> Obj {
        match Lexer::new(s.as_bytes(), 0).next(true) {
            Some(Item::Obj(o)) => o,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn objects() {
        assert_eq!(one("12 0 R"), Obj::Ref(12, 0));
        assert_eq!(one("-.5"), Obj::Real(-0.5));
        assert_eq!(one("--3"), Obj::Int(-3));
        assert_eq!(one("/A#20B"), Obj::Name(Arc::from("A B")));
        assert_eq!(one("(a\\(b\\) \\101)"), Obj::Str(b"a(b) A".to_vec()));
        assert_eq!(one("<414>"), Obj::Str(vec![0x41, 0x40]));
        let Obj::Dict(d) = one("<< /Type /Page /Kids [1 0 R 2 0 R] /N 3 >>") else { panic!() };
        assert_eq!(d.get("Kids"), Some(&Obj::Array(vec![Obj::Ref(1, 0), Obj::Ref(2, 0)])));
        assert_eq!(d.get("N"), Some(&Obj::Int(3)));
    }
}
