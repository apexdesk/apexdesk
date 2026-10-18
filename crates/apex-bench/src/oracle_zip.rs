//! A minimal ZIP reader and XML tokenizer for reading what Office stored
//! in its packages (the oracle reads the files itself, not through
//! apex-io, so that it is an independent witness).

use std::borrow::Cow;

pub struct Zip<'a> {
    data: &'a [u8],
    entries: Vec<(String, u16, usize, usize, usize)>,
}

fn u16_at(d: &[u8], p: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(p..p + 2)?.try_into().ok()?))
}

fn u32_at(d: &[u8], p: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(p..p + 4)?.try_into().ok()?))
}

impl<'a> Zip<'a> {
    pub fn new(data: &'a [u8]) -> Option<Zip<'a>> {
        if data.len() < 22 {
            return None;
        }
        let min = data.len().saturating_sub(22 + 65535);
        let eocd = (min..=data.len() - 22).rev().find(|&i| data[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])?;
        let count = u16_at(data, eocd + 10)? as usize;
        let mut p = u32_at(data, eocd + 16)? as usize;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            if u32_at(data, p)? != 0x0201_4b50 {
                return None;
            }
            let method = u16_at(data, p + 10)?;
            let comp = u32_at(data, p + 20)? as usize;
            let size = u32_at(data, p + 24)? as usize;
            let nlen = u16_at(data, p + 28)? as usize;
            let elen = u16_at(data, p + 30)? as usize;
            let clen = u16_at(data, p + 32)? as usize;
            let off = u32_at(data, p + 42)? as usize;
            let name = String::from_utf8_lossy(data.get(p + 46..p + 46 + nlen)?).replace('\\', "/");
            entries.push((name, method, comp, size, off));
            p += 46 + nlen + elen + clen;
        }
        Some(Zip { data, entries })
    }

    /// A part's bytes (names compared without a leading slash, and without
    /// case as a fallback, like Office does).
    pub fn read(&self, name: &str) -> Option<Vec<u8>> {
        let name = name.trim_start_matches('/');
        let e = self.entries.iter().find(|e| e.0 == name).or_else(|| self.entries.iter().find(|e| e.0.eq_ignore_ascii_case(name)))?;
        let (_, method, comp, size, off) = *e;
        let d = self.data;
        if u32_at(d, off)? != 0x0403_4b50 {
            return None;
        }
        let start = off + 30 + u16_at(d, off + 26)? as usize + u16_at(d, off + 28)? as usize;
        let raw = d.get(start..start.checked_add(comp)?)?;
        match method {
            0 => Some(raw.to_vec()),
            8 => miniz_oxide::inflate::decompress_to_vec_with_limit(raw, size.max(1).saturating_mul(2) + (512 << 20)).ok(),
            _ => None,
        }
    }

    pub fn read_string(&self, name: &str) -> Option<String> {
        let b = self.read(name)?;
        let b = b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&b);
        Some(String::from_utf8_lossy(b).into_owned())
    }
}

/// One XML event. Names keep their prefix (`w:p`).
#[derive(Debug, PartialEq)]
pub enum Ev<'a> {
    Start(&'a str, &'a str),
    Empty(&'a str, &'a str),
    End(&'a str),
    Text(&'a str),
}

/// A forgiving pull tokenizer: comments, processing instructions,
/// doctypes are skipped; CDATA comes as text.
pub struct Xml<'a> {
    s: &'a str,
    pos: usize,
}

impl<'a> Xml<'a> {
    pub fn new(s: &'a str) -> Xml<'a> {
        Xml { s, pos: 0 }
    }
}

impl<'a> Iterator for Xml<'a> {
    type Item = Ev<'a>;

    fn next(&mut self) -> Option<Ev<'a>> {
        loop {
            let rest = self.s.get(self.pos..)?;
            if rest.is_empty() {
                return None;
            }
            if !rest.starts_with('<') {
                let end = rest.find('<').unwrap_or(rest.len());
                self.pos += end;
                return Some(Ev::Text(&rest[..end]));
            }
            let skip_to = |pat: &str| rest.find(pat).map(|i| i + pat.len()).unwrap_or(rest.len());
            if rest.starts_with("<!--") {
                self.pos += skip_to("-->");
                continue;
            }
            if let Some(body) = rest.strip_prefix("<![CDATA[") {
                let end = body.find("]]>").unwrap_or(body.len());
                self.pos += 9 + (end + 3).min(body.len());
                return Some(Ev::Text(&body[..end]));
            }
            if rest.starts_with("<?") || rest.starts_with("<!") {
                self.pos += skip_to(">");
                continue;
            }
            // A tag: find its end outside quoted attribute values.
            let b = rest.as_bytes();
            let mut i = 1;
            let mut quote = 0u8;
            while i < b.len() {
                let c = b[i];
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
            self.pos += (i + 1).min(rest.len());
            let inner = &rest[1..i.min(rest.len())];
            if let Some(name) = inner.strip_prefix('/') {
                return Some(Ev::End(name.trim()));
            }
            let (inner, empty) = match inner.strip_suffix('/') {
                Some(x) => (x, true),
                None => (inner, false),
            };
            let n = inner.find(|c: char| c.is_whitespace()).unwrap_or(inner.len());
            let (name, attrs) = (&inner[..n], &inner[n..]);
            return Some(if empty { Ev::Empty(name, attrs) } else { Ev::Start(name, attrs) });
        }
    }
}

/// The value of attribute `key` (with its prefix, as written) in the
/// attribute text of a tag.
pub fn attr<'a>(attrs: &'a str, key: &str) -> Option<Cow<'a, str>> {
    let mut rest = attrs;
    loop {
        let eq = rest.find('=')?;
        let name = rest[..eq].trim();
        let after = rest[eq + 1..].trim_start();
        let q = after.chars().next()?;
        if q != '"' && q != '\'' {
            return None;
        }
        let end = after[1..].find(q)?;
        let value = &after[1..1 + end];
        if name == key || name.rsplit(':').next() == Some(key) && !key.contains(':') {
            return Some(unescape(value));
        }
        rest = &after[end + 2..];
    }
}

pub fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

pub fn unescape(s: &str) -> Cow<'_, str> {
    if !s.contains('&') {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|&e| e < 12) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let c = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => ent
                .strip_prefix("#x")
                .or_else(|| ent.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match c {
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

/// The text of the first element named `tag` (prefix ignored), e.g. the
/// `Application` of docProps/app.xml.
pub fn element_text(xml: &str, tag: &str) -> Option<String> {
    let mut it = Xml::new(xml);
    while let Some(ev) = it.next() {
        if let Ev::Start(n, _) = ev {
            if local(n) == tag {
                let mut out = String::new();
                for ev in it.by_ref() {
                    match ev {
                        Ev::Text(t) => out.push_str(&unescape(t)),
                        Ev::End(_) => break,
                        _ => {}
                    }
                }
                return Some(out.trim().to_string());
            }
        }
    }
    None
}

#[cfg(test)]
pub fn build_zip(parts: &[(&str, &str)]) -> Vec<u8> {
    // Stored (uncompressed) entries: enough for the tests.
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, body) in parts {
        let off = out.len() as u32;
        let crc = 0u32;
        let mut h = Vec::new();
        h.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        h.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        h.extend_from_slice(&crc.to_le_bytes());
        h.extend_from_slice(&(body.len() as u32).to_le_bytes());
        h.extend_from_slice(&(body.len() as u32).to_le_bytes());
        h.extend_from_slice(&(name.len() as u16).to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&h);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(body.as_bytes());
        let mut c = Vec::new();
        c.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        c.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        c.extend_from_slice(&crc.to_le_bytes());
        c.extend_from_slice(&(body.len() as u32).to_le_bytes());
        c.extend_from_slice(&(body.len() as u32).to_le_bytes());
        c.extend_from_slice(&(name.len() as u16).to_le_bytes());
        c.extend_from_slice(&[0; 12]);
        c.extend_from_slice(&off.to_le_bytes());
        c.extend_from_slice(name.as_bytes());
        central.extend_from_slice(&c);
    }
    let cd = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&(parts.len() as u16).to_le_bytes());
    out.extend_from_slice(&(parts.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_parts_and_tokens() {
        let z = build_zip(&[("docProps/app.xml", "<?xml version=\"1.0\"?><Properties><Application>Microsoft Excel</Application><Pages>3</Pages></Properties>")]);
        let zip = Zip::new(&z).unwrap();
        let app = zip.read_string("/docProps/app.xml").unwrap();
        assert_eq!(element_text(&app, "Application").as_deref(), Some("Microsoft Excel"));
        assert_eq!(element_text(&app, "Pages").as_deref(), Some("3"));
        let evs: Vec<Ev> = Xml::new("<a x=\"1&amp;2\" y='>'><!-- c --><b/>t&lt;</a>").collect();
        assert_eq!(evs, vec![Ev::Start("a", " x=\"1&amp;2\" y='>'"), Ev::Empty("b", ""), Ev::Text("t&lt;"), Ev::End("a")]);
        assert_eq!(attr(" x=\"1&amp;2\" y='>'", "x").as_deref(), Some("1&2"));
        assert_eq!(attr(" w:type=\"page\"", "type").as_deref(), Some("page"));
        assert_eq!(unescape("a&#65;&#x42;&bogus"), "aAB&bogus");
    }
}
