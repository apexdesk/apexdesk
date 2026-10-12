//! Tiny, fast XML pull parser and writer helpers.
//!
//! Enough for OOXML/ODF: elements, attributes, text, entities, CDATA.
//! Comments, processing instructions and doctypes are skipped.

use std::borrow::Cow;

#[derive(Debug)]
pub enum Ev<'a> {
    Start(Tag<'a>),
    /// Self-closing element: no matching `End` is emitted.
    Empty(Tag<'a>),
    End(&'a str),
    Text(Cow<'a, str>),
    Eof,
}

#[derive(Debug, Clone, Copy)]
pub struct Tag<'a> {
    /// Qualified name, e.g. `w:p`.
    pub name: &'a str,
    raw_attrs: &'a str,
}

impl<'a> Tag<'a> {
    /// Name without namespace prefix.
    pub fn local(&self) -> &'a str {
        local(self.name)
    }

    /// Looks up an attribute by qualified name, or by local name when `key`
    /// has no prefix and no exact match exists.
    pub fn attr(&self, key: &str) -> Option<Cow<'a, str>> {
        // Values are unescaped only when they match (a cell asks for
        // attributes it rarely has, millions of times).
        let plain = !key.contains(':');
        let mut fallback = None;
        let mut rest = self.raw_attrs;
        while let Some((k, v)) = next_attr(&mut rest) {
            if k == key {
                return Some(unescape(v));
            }
            if plain && fallback.is_none() && k.len() > key.len() && local(k) == key {
                fallback = Some(v);
            }
        }
        fallback.map(unescape)
    }

    /// Several attributes in one pass over the tag, each found as
    /// [`Tag::attr`] finds it (a cell asks for five).
    pub fn attrs_of<const N: usize>(&self, keys: [&str; N]) -> [Option<Cow<'a, str>>; N] {
        let mut exact: [Option<&'a str>; N] = [None; N];
        let mut fallback: [Option<&'a str>; N] = [None; N];
        let mut rest = self.raw_attrs;
        while let Some((k, v)) = next_attr(&mut rest) {
            for (i, key) in keys.iter().enumerate() {
                if k == *key {
                    exact[i] = exact[i].or(Some(v));
                } else if fallback[i].is_none() && k.len() > key.len() && !key.contains(':') && local(k) == *key {
                    fallback[i] = Some(v);
                }
            }
        }
        std::array::from_fn(|i| exact[i].or(fallback[i]).map(unescape))
    }

    pub fn attrs(&self) -> AttrIter<'a> {
        AttrIter { s: self.raw_attrs }
    }

    /// The attributes as written (with their leading space).
    pub fn raw_attrs(&self) -> &'a str {
        self.raw_attrs
    }
}

pub fn local(name: &str) -> &str {
    // Names are short: a plain loop beats setting up a searcher.
    match name.bytes().rposition(|b| b == b':') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

pub struct AttrIter<'a> {
    s: &'a str,
}

impl<'a> Iterator for AttrIter<'a> {
    type Item = (&'a str, Cow<'a, str>);

    fn next(&mut self) -> Option<Self::Item> {
        next_attr(&mut self.s).map(|(k, v)| (k, unescape(v)))
    }
}

/// The next attribute of `s` as written (value still escaped); `s` moves
/// past it.
fn next_attr<'a>(s: &mut &'a str) -> Option<(&'a str, &'a str)> {
    let t = trim_start(s);
    let eq = memchr::memchr(b'=', t.as_bytes())?;
    let key = &t[..eq];
    let key = if key.as_bytes().last().is_some_and(|&b| b > b' ' && b < 0x80) { key } else { key.trim() };
    let rest = trim_start(&t[eq + 1..]);
    let q = *rest.as_bytes().first()?;
    if q != b'"' && q != b'\'' {
        return None;
    }
    let end = memchr::memchr(q, &rest.as_bytes()[1..])? + 1;
    let val = &rest[1..end];
    *s = &rest[end + 1..];
    Some((key, val))
}

/// `s` without leading whitespace: a byte loop for the usual ASCII,
/// `str::trim_start` past anything else.
#[inline]
fn trim_start(s: &str) -> &str {
    let b = s.as_bytes();
    let i = b.iter().position(|&c| !matches!(c, b' ' | b'\t' | b'\n' | b'\r')).unwrap_or(b.len());
    match b.get(i) {
        Some(&c) if c >= 0x80 || c == 0x0B || c == 0x0C => s[i..].trim_start(),
        _ => &s[i..],
    }
}

#[derive(Clone)]
pub struct Reader<'a> {
    s: &'a str,
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(s: &'a str) -> Self {
        Reader { s: s.strip_prefix('\u{feff}').unwrap_or(s), pos: 0 }
    }

    pub fn next(&mut self) -> Ev<'a> {
        let s = self.s;
        let b = s.as_bytes();
        loop {
            if self.pos >= b.len() {
                return Ev::Eof;
            }
            if b[self.pos] != b'<' {
                let end = memchr::memchr(b'<', &b[self.pos..]).map_or(s.len(), |i| self.pos + i);
                let t = &s[self.pos..end];
                self.pos = end;
                return Ev::Text(unescape(t));
            }
            let rest = &s[self.pos..];
            if let Some(body) = rest.strip_prefix("<!--") {
                // The end after the opening, as signatures read it: "<!-->"
                // doesn't close itself.
                self.pos = body.find("-->").map_or(s.len(), |i| self.pos + 4 + i + 3);
                continue;
            }
            if let Some(r) = rest.strip_prefix("<![CDATA[") {
                let end = r.find("]]>").unwrap_or(r.len());
                self.pos += 9 + end + 3;
                return Ev::Text(Cow::Borrowed(&r[..end]));
            }
            // Where XML ends them (as signatures read them too): an
            // instruction at "?>", a declaration past its brackets.
            if rest.starts_with("<?") {
                self.pos = rest.find("?>").map_or(s.len(), |i| self.pos + i + 2);
                continue;
            }
            if rest.starts_with("<!") {
                let mut depth = 0i32;
                let end = rest.bytes().position(|c| match c {
                    b'[' => {
                        depth += 1;
                        false
                    }
                    b']' => {
                        depth -= 1;
                        false
                    }
                    b'>' => depth == 0,
                    _ => false,
                });
                self.pos = end.map_or(s.len(), |i| self.pos + i + 1);
                continue;
            }
            // Find tag end, honoring quoted attribute values.
            let mut i = 1;
            let mut quote = 0u8;
            let rb = rest.as_bytes();
            while i < rb.len() {
                let c = rb[i];
                if quote != 0 {
                    if c == quote {
                        quote = 0;
                    }
                } else if c == b'"' || c == b'\'' {
                    quote = c;
                } else if c == b'>' {
                    break;
                }
                i += 1;
            }
            let inner = &rest[1..i.min(rest.len())];
            self.pos += i + 1;
            if let Some(name) = inner.strip_prefix('/') {
                return Ev::End(name.trim());
            }
            let (inner, empty) = match inner.strip_suffix('/') {
                Some(x) => (x, true),
                None => (inner, false),
            };
            // Bytes for the usual ASCII names, chars past the first other.
            let name_end = match inner.bytes().position(|b| b.is_ascii_whitespace() || b == 0x0B || b >= 0x80) {
                Some(i) if inner.as_bytes()[i] >= 0x80 => inner.find(|c: char| c.is_whitespace()).unwrap_or(inner.len()),
                Some(i) => i,
                None => inner.len(),
            };
            let tag = Tag { name: &inner[..name_end], raw_attrs: &inner[name_end..] };
            return if empty { Ev::Empty(tag) } else { Ev::Start(tag) };
        }
    }

    /// Byte offset of the next event in the source.
    pub fn pos(&self) -> usize {
        self.pos.min(self.s.len())
    }

    /// Skips to the end of the element whose `Start` was just returned.
    pub fn skip(&mut self) {
        let mut depth = 1;
        loop {
            match self.next() {
                Ev::Start(_) => depth += 1,
                Ev::End(_) => {
                    depth -= 1;
                    if depth == 0 {
                        return;
                    }
                }
                Ev::Eof => return,
                _ => {}
            }
        }
    }

    /// The source of the element whose `Start` was just returned, from
    /// after its start tag to after its end tag; the reader moves past it.
    pub fn raw_element(&mut self) -> &'a str {
        let start = self.pos.min(self.s.len());
        self.skip();
        &self.s[start..self.pos.min(self.s.len())]
    }

    /// Concatenated text content of the element whose `Start` was just returned.
    pub fn text(&mut self) -> String {
        let mut out = String::new();
        let mut depth = 1;
        loop {
            match self.next() {
                Ev::Start(_) => depth += 1,
                Ev::End(_) => {
                    depth -= 1;
                    if depth == 0 {
                        return out;
                    }
                }
                Ev::Text(t) => out.push_str(&t),
                Ev::Eof => return out,
                Ev::Empty(_) => {}
            }
        }
    }

    /// [`Reader::text`] borrowed from the source when it is one piece
    /// without entities (a cell value: no copy per cell).
    pub fn text_cow(&mut self) -> Cow<'a, str> {
        let mut out: Option<Cow<'a, str>> = None;
        let mut depth = 1;
        loop {
            match self.next() {
                Ev::Start(_) => depth += 1,
                Ev::End(_) => {
                    depth -= 1;
                    if depth == 0 {
                        return out.unwrap_or_default();
                    }
                }
                Ev::Text(t) => match &mut out {
                    None => out = Some(t),
                    Some(o) => o.to_mut().push_str(&t),
                },
                Ev::Eof => return out.unwrap_or_default(),
                Ev::Empty(_) => {}
            }
        }
    }
}

pub fn unescape(s: &str) -> Cow<'_, str> {
    if memchr::memchr(b'&', s.as_bytes()).is_none() {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        // Only a few bytes on: a long run of '&' mustn't search to the end each time.
        let Some(end) = rest.as_bytes()[..rest.len().min(12)].iter().position(|&b| b == b';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let ch = match ent {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => {
                if let Some(h) = ent.strip_prefix("#x").or_else(|| ent.strip_prefix("#X")) {
                    u32::from_str_radix(h, 16).ok().and_then(char::from_u32)
                } else if let Some(d) = ent.strip_prefix('#') {
                    d.parse().ok().and_then(char::from_u32)
                } else {
                    None
                }
            }
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// Escapes text for element content and attribute values.
pub fn esc(s: &str) -> Cow<'_, str> {
    if !s.contains(['<', '>', '&', '"', '\'', '\t', '\r', '\n', '\u{FFFE}', '\u{FFFF}']) && !s.chars().any(|c| (c as u32) < 0x20) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' => out.push_str("&#9;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            // Not allowed in XML.
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
    Cow::Owned(out)
}

pub const HEADER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse() {
        let mut r = Reader::new("<?xml version='1.0'?><a x=\"1 &amp; 2\" y='>'><!-- c --><b/>t&lt;x<![CDATA[<raw>]]></a>");
        match r.next() {
            Ev::Start(t) => {
                assert_eq!(t.name, "a");
                assert_eq!(t.attr("x").unwrap(), "1 & 2");
                assert_eq!(t.attr("y").unwrap(), ">");
            }
            e => panic!("{:?}", e),
        }
        assert!(matches!(r.next(), Ev::Empty(t) if t.name == "b"));
        assert!(matches!(r.next(), Ev::Text(t) if t == "t<x"));
        assert!(matches!(r.next(), Ev::Text(t) if t == "<raw>"));
        assert!(matches!(r.next(), Ev::End("a")));
        assert!(matches!(r.next(), Ev::Eof));
        let mut r = Reader::new("<w:t xml:space=\"preserve\"> hi </w:t>");
        if let Ev::Start(t) = r.next() {
            assert_eq!(t.local(), "t");
            assert_eq!(t.attr("space").unwrap(), "preserve");
        }
        assert_eq!(r.text(), " hi ");
    }

    #[test]
    fn comment_opening_doesnt_close_itself() {
        // Signatures leave out all of it: so must the reader.
        let mut r = Reader::new("<a><!--><b/><!-- --></a>");
        assert!(matches!(r.next(), Ev::Start(t) if t.name == "a"));
        assert!(matches!(r.next(), Ev::End("a")));
    }
}
