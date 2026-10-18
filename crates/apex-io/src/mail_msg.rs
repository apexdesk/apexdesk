//! E-mail messages, read only: MIME files (.eml: headers with encoded
//! words, multipart bodies, quoted-printable and base64, charsets) and
//! Outlook .msg files (the `__substg1.0_` property streams of an OLE
//! compound file). A message opens as a document: the subject as a
//! heading, the sender, recipients and date, the body and a line listing
//! the attachments.

use apex_doc::{Attrs, Document, ParaKind, ParaStyle, Paragraph, Run};
use apex_i18n::tr;

use crate::Error;
use crate::ole::{self, Ole};

/// A message as read from a file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Message {
    pub subject: String,
    pub from: String,
    pub to: String,
    pub cc: String,
    /// As the file gives it (the `Date:` header, or the submit time).
    pub date: String,
    pub body_text: String,
    pub body_html: Option<String>,
    /// File names and contents.
    pub attachments: Vec<(String, Vec<u8>)>,
}

impl Message {
    /// The body as plain text: the text part, or the HTML one without
    /// its markup.
    pub fn body(&self) -> String {
        match &self.body_html {
            Some(h) if self.body_text.trim().is_empty() => html_text(h),
            _ => self.body_text.clone(),
        }
    }
}

/// Whether `data` is an OLE compound file (an Outlook .msg).
fn is_ole(data: &[u8]) -> bool {
    data.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1])
}

/// Reads a message of either kind, by its content.
pub fn read(data: &[u8]) -> Result<Message, Error> {
    if is_ole(data) { read_msg(data) } else { Ok(read_eml(data)) }
}

/// Reads a message as a document (see [`document`]).
pub fn read_doc(data: &[u8]) -> Result<Document, Error> {
    Ok(document(&read(data)?))
}

/// The message as a document: the subject as a heading, a row per header
/// with its label in bold, the body and a line with the attachments.
pub fn document(m: &Message) -> Document {
    let subject = if m.subject.trim().is_empty() { tr("email.no_subject") } else { m.subject.trim() };
    let mut paras = vec![Paragraph::new(subject).with_style(ParaStyle { kind: ParaKind::Heading(1), ..Default::default() })];
    for (label, value) in [(tr("email.from"), &m.from), (tr("email.to"), &m.to), (tr("email.cc"), &m.cc), (tr("email.date"), &m.date)] {
        if !value.trim().is_empty() {
            paras.push(labeled(label, value.trim()));
        }
    }
    paras.push(Paragraph::new(""));
    let body = m.body();
    paras.extend(body.trim_end().split('\n').map(Paragraph::new));
    if !m.attachments.is_empty() {
        let list: Vec<String> = m.attachments.iter().map(|(n, d)| format!("{n} ({})", size(d.len()))).collect();
        paras.push(Paragraph::new(""));
        paras.push(labeled(tr("email.attachments"), &list.join("; ")));
    }
    let mut doc = Document::from_paragraphs(paras);
    doc.meta.title = m.subject.trim().to_string();
    doc.meta.author = m.from.trim().to_string();
    doc
}

/// "Label: value" with the label in bold.
fn labeled(label: &str, value: &str) -> Paragraph {
    let mut p = Paragraph::new(format!("{label}: {value}"));
    let bold = Attrs { bold: true, ..Default::default() };
    p.runs = vec![Run { len: label.len() + 1, attrs: bold }, Run { len: value.len() + 1, attrs: Attrs::default() }];
    p
}

/// A readable file size ("820 bytes", "12 KB", "3.4 MB").
fn size(n: usize) -> String {
    match n {
        0..1024 => format!("{n} bytes"),
        1024..1_048_576 => format!("{} KB", n.div_ceil(1024)),
        _ => format!("{:.1} MB", n as f64 / 1_048_576.0),
    }
}

// --- MIME (.eml) -----------------------------------------------------------------------------

/// Reads a MIME message. Anything readable is read: a file without
/// headers is all body.
pub fn read_eml(data: &[u8]) -> Message {
    let (head, body) = split_head(data);
    let heads = headers(head);
    let mut m = Message {
        subject: decode_words(header(&heads, "subject")),
        from: decode_words(header(&heads, "from")),
        to: decode_words(header(&heads, "to")),
        cc: decode_words(header(&heads, "cc")),
        date: header(&heads, "date").trim().to_string(),
        ..Default::default()
    };
    walk(&mut m, &heads, body, 0);
    m
}

/// The header block and the body of a message or part (at the first
/// empty line).
fn split_head(d: &[u8]) -> (&[u8], &[u8]) {
    if let Some(rest) = d.strip_prefix(b"\r\n").or_else(|| d.strip_prefix(b"\n")) {
        return (&[], rest);
    }
    let crlf = memchr::memmem::find(d, b"\r\n\r\n").map(|p| (p, p + 4));
    let lf = memchr::memmem::find(d, b"\n\n").map(|p| (p, p + 2));
    match [crlf, lf].into_iter().flatten().min() {
        Some((end, start)) => (&d[..end], &d[start..]),
        None => (d, &[]),
    }
}

/// Header names (lowercase) and values, folded lines joined.
fn headers(h: &[u8]) -> Vec<(String, String)> {
    let text = bytes_text(h, None);
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if line.starts_with([' ', '\t']) {
            if let Some(last) = out.last_mut() {
                last.1.push(' ');
                last.1.push_str(line.trim());
            }
        } else if let Some((name, value)) = line.split_once(':') {
            out.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    out
}

/// The first header `name` (lowercase), or "".
fn header<'a>(heads: &'a [(String, String)], name: &str) -> &'a str {
    heads.iter().find(|h| h.0 == name).map_or("", |h| h.1.as_str())
}

/// Text in `charset` (None: UTF-8 if it is, else Windows-1252).
fn bytes_text(b: &[u8], charset: Option<&str>) -> String {
    if let Some(enc) = charset.and_then(|c| encoding_rs::Encoding::for_label(c.trim().as_bytes())) {
        return enc.decode(b).0.into_owned();
    }
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::WINDOWS_1252.decode(b).0.into_owned(),
    }
}

/// A header value with its RFC 2047 encoded words (`=?utf-8?B?...?=`)
/// decoded; the space between two encoded words goes.
fn decode_words(v: &str) -> String {
    let mut out = String::new();
    let mut rest = v;
    let mut after_word = false;
    while let Some(p) = rest.find("=?") {
        let Some((text, len)) = encoded_word(&rest[p..]) else {
            out.push_str(&rest[..p + 2]);
            rest = &rest[p + 2..];
            after_word = false;
            continue;
        };
        let between = &rest[..p];
        if !(after_word && between.trim().is_empty()) {
            out.push_str(between);
        }
        out.push_str(&text);
        rest = &rest[p + len..];
        after_word = true;
    }
    out.push_str(rest);
    out
}

/// The text of the encoded word at the start of `s` and its length.
fn encoded_word(s: &str) -> Option<(String, usize)> {
    let inner = &s[2..];
    let q1 = inner.find('?')?;
    let charset = inner[..q1].split('*').next()?;
    let enc = inner.as_bytes().get(q1 + 1)?.to_ascii_uppercase();
    if inner.as_bytes().get(q1 + 2) != Some(&b'?') {
        return None;
    }
    let text_at = q1 + 3;
    // The text has no '?' of its own: the next one ends the word (looking
    // further for "?=" would take quadratic time over a header of openers).
    let end = inner[text_at..].find('?')? + text_at;
    if inner.as_bytes().get(end + 1) != Some(&b'=') {
        return None;
    }
    let raw = &inner[text_at..end];
    let bytes = match enc {
        b'B' => b64(raw.as_bytes())?,
        b'Q' => quoted_printable(raw.replace('_', " ").as_bytes()),
        _ => return None,
    };
    Some((bytes_text(&bytes, Some(charset)), end + 4))
}

fn b64(b: &[u8]) -> Option<Vec<u8>> {
    let s: String = b.iter().filter(|c| !c.is_ascii_whitespace()).map(|&c| c as char).collect();
    crate::markdown::base64_decode(&s)
}

/// Quoted-printable: `=HH` bytes, soft line breaks (`=` at a line's end)
/// joined.
fn quoted_printable(b: &[u8]) -> Vec<u8> {
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'=' {
            if b[i + 1..].starts_with(b"\r\n") {
                i += 3;
                continue;
            }
            if b[i + 1..].starts_with(b"\n") {
                i += 2;
                continue;
            }
            if let (Some(h), Some(l)) = (b.get(i + 1).and_then(|&c| hex(c)), b.get(i + 2).and_then(|&c| hex(c))) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// The value of a header like Content-Type (lowercase, "text/plain") and
/// its parameters (names lowercase).
fn params(v: &str) -> (String, Vec<(String, String)>) {
    let mut pieces = Vec::new();
    let (mut cur, mut quoted) = (String::new(), false);
    for ch in v.chars() {
        match ch {
            '"' => quoted = !quoted,
            ';' if !quoted => pieces.push(std::mem::take(&mut cur)),
            _ => cur.push(ch),
        }
    }
    pieces.push(cur);
    let value = pieces[0].trim().to_ascii_lowercase();
    let ps = pieces[1..].iter().filter_map(|p| p.split_once('=')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
    (value, ps)
}

/// Parameter `name`, also in its RFC 2231 forms (`name*=utf-8''a%20b`,
/// continuations `name*0*=`, `name*1*=`...).
fn param(ps: &[(String, String)], name: &str) -> Option<String> {
    if let Some(v) = ps.iter().find(|p| p.0 == name) {
        return Some(decode_words(&v.1));
    }
    let star = format!("{name}*");
    if let Some(v) = ps.iter().find(|p| p.0 == star) {
        return Some(ext_value(&v.1, true));
    }
    let mut parts: Vec<(u32, bool, &str)> = ps
        .iter()
        .filter_map(|(k, v)| {
            let n = k.strip_prefix(&star)?;
            let (n, enc) = n.strip_suffix('*').map_or((n, false), |n| (n, true));
            Some((n.parse().ok()?, enc, v.as_str()))
        })
        .collect();
    if parts.is_empty() {
        return None;
    }
    parts.sort_by_key(|p| p.0);
    // The charset is given once, in the first piece.
    let charset = parts[0].2.split('\'').next().filter(|_| parts[0].1 && parts[0].2.matches('\'').count() >= 2).map(str::to_string);
    let mut bytes = Vec::new();
    for (i, (_, enc, v)) in parts.iter().enumerate() {
        let v = if i == 0 && charset.is_some() { v.splitn(3, '\'').nth(2).unwrap_or("") } else { v };
        if *enc { bytes.extend(percent(v)) } else { bytes.extend_from_slice(v.as_bytes()) }
    }
    Some(bytes_text(&bytes, charset.as_deref()))
}

/// An RFC 2231 value: `charset'language'percent-encoded`.
fn ext_value(v: &str, enc: bool) -> String {
    let mut it = v.splitn(3, '\'');
    match (it.next(), it.next(), it.next()) {
        (Some(cs), Some(_), Some(text)) if enc => bytes_text(&percent(text), Some(cs)),
        _ => v.to_string(),
    }
}

fn percent(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok()) {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

/// Takes the bodies and attachments of a part (the whole message at
/// depth 0), going into multipart ones.
fn walk(m: &mut Message, heads: &[(String, String)], body: &[u8], depth: usize) {
    let (ctype, cps) = params(header(heads, "content-type"));
    let ctype = if ctype.is_empty() { "text/plain".to_string() } else { ctype };
    if ctype.starts_with("multipart/") {
        if let Some(boundary) = param(&cps, "boundary").filter(|b| !b.is_empty() && depth < 20) {
            for part in split_multipart(body, &boundary) {
                let (h, b) = split_head(part);
                walk(m, &headers(h), b, depth + 1);
            }
        }
        return;
    }
    let data = match header(heads, "content-transfer-encoding").trim().to_ascii_lowercase().as_str() {
        "base64" => b64(body).unwrap_or_default(),
        "quoted-printable" => quoted_printable(body),
        _ => body.to_vec(),
    };
    let (disp, dps) = params(header(heads, "content-disposition"));
    let name = param(&dps, "filename").or_else(|| param(&cps, "name")).filter(|n| !n.trim().is_empty());
    if disp != "attachment" {
        let charset = param(&cps, "charset");
        if ctype == "text/plain" && m.body_text.is_empty() && (name.is_none() || depth == 0) {
            m.body_text = bytes_text(&data, charset.as_deref()).replace("\r\n", "\n");
            return;
        }
        if ctype == "text/html" && m.body_html.is_none() && (name.is_none() || depth == 0) {
            m.body_html = Some(bytes_text(&data, charset.as_deref()));
            return;
        }
    }
    // A message forwarded as an attachment, or any other part.
    let name = name.unwrap_or_else(|| {
        let ext = if ctype == "message/rfc822" { "eml" } else { ctype.rsplit('/').next().filter(|e| e.len() <= 4).unwrap_or("bin") };
        format!("attachment-{}.{ext}", m.attachments.len() + 1)
    });
    m.attachments.push((name.trim().to_string(), data));
}

/// The parts of a multipart body, between its `--boundary` lines.
fn split_multipart<'a>(body: &'a [u8], boundary: &str) -> Vec<&'a [u8]> {
    let delim = format!("--{boundary}");
    let d = delim.as_bytes();
    // Where each delimiter line starts and where the text after it does.
    let mut marks: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    let mut closed = false;
    while let Some(p) = memchr::memmem::find(&body[i..], d) {
        let at = i + p;
        i = at + d.len();
        if at > 0 && body[at - 1] != b'\n' {
            continue;
        }
        let next = memchr::memchr(b'\n', &body[i..]).map_or(body.len(), |q| i + q + 1);
        marks.push((at, next));
        if body[i..].starts_with(b"--") {
            closed = true;
            break;
        }
    }
    if !closed {
        marks.push((body.len(), body.len()));
    }
    marks
        .windows(2)
        .map(|w| {
            let part = &body[w[0].1..w[1].0.max(w[0].1)];
            let part = part.strip_suffix(b"\n").unwrap_or(part);
            part.strip_suffix(b"\r").unwrap_or(part)
        })
        .collect()
}

// --- HTML bodies -----------------------------------------------------------------------------

/// The text of an HTML body: paragraphs, line breaks, list items and
/// table cells kept, markup, styles and scripts left out.
pub fn html_text(html: &str) -> String {
    let mut out = String::new();
    let mut skip: Option<String> = None;
    let mut pre = false;
    let mut i = 0;
    while i < html.len() {
        let rest = &html[i..];
        if rest.starts_with("<!--") {
            i = rest.find("-->").map_or(html.len(), |p| i + p + 3);
            continue;
        }
        if rest.starts_with('<') {
            let end = rest.find('>').map_or(html.len(), |p| i + p + 1);
            let tag = html[i + 1..end.max(i + 1)].trim_end_matches('>');
            i = end;
            let closing = tag.starts_with('/');
            let name = tag.trim_start_matches('/').chars().take_while(char::is_ascii_alphanumeric).collect::<String>().to_ascii_lowercase();
            if let Some(s) = &skip {
                if closing && name == *s {
                    skip = None;
                }
                continue;
            }
            match name.as_str() {
                "script" | "style" | "head" | "title" if !closing && !tag.ends_with('/') => skip = Some(name),
                "br" => {
                    trim_spaces(&mut out);
                    out.push('\n');
                }
                "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "blockquote" | "table" | "ul" | "ol" | "hr" => blank_line(&mut out),
                "pre" => {
                    blank_line(&mut out);
                    pre = !closing;
                }
                "div" | "tr" => line_break(&mut out),
                "li" if !closing => {
                    line_break(&mut out);
                    out.push_str("• ");
                }
                "td" | "th" if !closing && !out.is_empty() && !out.ends_with('\n') => {
                    trim_spaces(&mut out);
                    out.push('\t');
                }
                _ => {}
            }
            continue;
        }
        let end = rest.find('<').map_or(html.len(), |p| i + p);
        if skip.is_none() {
            for ch in entities(&html[i..end]).chars() {
                if pre {
                    out.push(ch);
                } else if ch.is_whitespace() && ch != '\u{a0}' {
                    if !out.is_empty() && !out.ends_with([' ', '\n', '\t']) {
                        out.push(' ');
                    }
                } else {
                    out.push(if ch == '\u{a0}' { ' ' } else { ch });
                }
            }
        }
        i = end;
    }
    // At most one empty line in a row, none at the ends.
    let mut text = String::new();
    let mut blank = 0;
    for line in out.lines().map(str::trim_end) {
        blank = if line.is_empty() { blank + 1 } else { 0 };
        if blank <= 1 {
            text.push_str(line);
            text.push('\n');
        }
    }
    text.trim().to_string()
}

fn trim_spaces(out: &mut String) {
    out.truncate(out.trim_end_matches(' ').len());
}

fn line_break(out: &mut String) {
    trim_spaces(out);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
}

fn blank_line(out: &mut String) {
    line_break(out);
    if !out.is_empty() && !out.ends_with("\n\n") {
        out.push('\n');
    }
}

/// Character references: the usual named ones and the numeric ones.
fn entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        rest = &rest[p..];
        let end = rest.as_bytes()[..rest.len().min(12)].iter().position(|&b| b == b';');
        let ch = end.and_then(|e| {
            let name = &rest[1..e];
            if let Some(n) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
                return u32::from_str_radix(n, 16).ok().and_then(char::from_u32);
            }
            if let Some(n) = name.strip_prefix('#') {
                return n.parse().ok().and_then(char::from_u32);
            }
            Some(match name {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                "nbsp" => '\u{a0}',
                "copy" => '©',
                "reg" => '®',
                "euro" => '€',
                "mdash" => '—',
                "ndash" => '–',
                "hellip" => '…',
                "lsquo" => '‘',
                "rsquo" => '’',
                "ldquo" => '“',
                "rdquo" => '”',
                "laquo" => '«',
                "raquo" => '»',
                "bull" => '•',
                "middot" => '·',
                _ => return None,
            })
        });
        match (ch, end) {
            (Some(c), Some(e)) => {
                out.push(c);
                rest = &rest[e + 1..];
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

// --- Outlook (.msg) --------------------------------------------------------------------------

const PT_LONG: u16 = 0x0003;
const PT_SYSTIME: u16 = 0x0040;

/// Reads an Outlook message: its property streams, the fixed-size
/// properties (dates, code page) and the attachment storages.
pub fn read_msg(data: &[u8]) -> Result<Message, Error> {
    let mut c = ole::open(data)?;
    if !c.is_stream("/__properties_version1.0") {
        return Err(Error::new("not an Outlook message"));
    }
    // The message's properties stream has a 32-byte header.
    let fixed = fixed_props(&ole::stream(&mut c, "/__properties_version1.0").unwrap_or_default(), 32);
    let cp = fixed.iter().find(|p| p.0 == 0x3FFD && p.1 == PT_LONG).map_or(1252, |p| u32::from_le_bytes([p.2[0], p.2[1], p.2[2], p.2[3]]) as u16);
    let s = |c: &mut Ole, id: u16| string(c, "", id, cp).unwrap_or_default();
    let name = s(&mut c, 0x0C1A);
    // The Exchange address ("/O=...") says less than the SMTP one.
    let email = [0x5D01, 0x0C1F, 0x0065].into_iter().map(|id| s(&mut c, id)).find(|e| e.contains('@')).unwrap_or_default();
    let from = match (name.trim(), email.trim()) {
        (n, "") => n.to_string(),
        ("", e) => e.to_string(),
        (n, e) if n == e => e.to_string(),
        (n, e) => format!("{n} <{e}>"),
    };
    // The date as the Internet headers give it, else the submit time.
    let transport = s(&mut c, 0x007D);
    let date = header(&headers(transport.as_bytes()), "date").trim().to_string();
    let date = if date.is_empty() { fixed.iter().filter(|p| p.1 == PT_SYSTIME).find(|p| p.0 == 0x0039).or_else(|| fixed.iter().find(|p| p.0 == 0x0E06 && p.1 == PT_SYSTIME)).map(|p| filetime(p.2)).unwrap_or_default() } else { date };
    let body_html = binary(&mut c, "", 0x1013).map(|b| bytes_text_cp(&b, cp)).or_else(|| string(&mut c, "", 0x1013, cp)).filter(|h| !h.trim().is_empty());
    let mut m = Message {
        subject: s(&mut c, 0x0037),
        from,
        to: s(&mut c, 0x0E04),
        cc: s(&mut c, 0x0E03),
        date,
        body_text: s(&mut c, 0x1000).replace("\r\n", "\n"),
        body_html,
        attachments: Vec::new(),
    };
    let mut dirs: Vec<String> = c.read_root_storage().filter(|e| e.is_storage() && e.name().starts_with("__attach_version1.0_#")).map(|e| format!("/{}", e.name())).collect();
    dirs.sort();
    // Attachments may share their data's sectors: each would be a new copy.
    let mut left = 1usize << 30;
    for dir in dirs {
        // Embedded messages (a storage, not bytes) are left out.
        let Some(data) = binary(&mut c, &dir, 0x3701) else { continue };
        left = left.checked_sub(data.len()).ok_or_else(|| Error::new("the message's attachments are too large"))?;
        let name = [0x3707, 0x3704, 0x3001].into_iter().filter_map(|id| string(&mut c, &dir, id, cp)).find(|n| !n.trim().is_empty());
        let name = name.unwrap_or_else(|| format!("attachment-{}", m.attachments.len() + 1));
        m.attachments.push((name.trim().to_string(), data));
    }
    Ok(m)
}

/// A string property of the storage `dir` ("" for the message): Unicode
/// (001F) or 8-bit in code page `cp` (001E).
fn string(c: &mut Ole, dir: &str, id: u16, cp: u16) -> Option<String> {
    let s = if let Some(b) = ole::stream(c, &format!("{dir}/__substg1.0_{id:04X}001F")) {
        let units: Vec<u16> = b.chunks_exact(2).map(|u| u16::from_le_bytes([u[0], u[1]])).collect();
        String::from_utf16_lossy(&units)
    } else {
        ole::decode_cp(&ole::stream(c, &format!("{dir}/__substg1.0_{id:04X}001E"))?, cp)
    };
    Some(s.trim_end_matches('\0').to_string())
}

/// A binary property (0102) of the storage `dir`.
fn binary(c: &mut Ole, dir: &str, id: u16) -> Option<Vec<u8>> {
    ole::stream(c, &format!("{dir}/__substg1.0_{id:04X}0102"))
}

/// HTML bytes: UTF-8 if they are, else in the message's code page.
fn bytes_text_cp(b: &[u8], cp: u16) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.trim_end_matches('\0').to_string(),
        Err(_) => ole::decode_cp(b, cp),
    }
}

/// The fixed-size properties of a properties stream after its header of
/// `skip` bytes: (id, type, value) in 16-byte entries.
fn fixed_props(data: &[u8], skip: usize) -> Vec<(u16, u16, [u8; 8])> {
    data.get(skip..)
        .unwrap_or_default()
        .chunks_exact(16)
        .map(|e| {
            let tag = u32::from_le_bytes([e[0], e[1], e[2], e[3]]);
            let mut v = [0; 8];
            v.copy_from_slice(&e[8..16]);
            ((tag >> 16) as u16, tag as u16, v)
        })
        .collect()
}

/// A FILETIME (100 ns since 1601) as "2026-09-29 10:30 UTC".
fn filetime(v: [u8; 8]) -> String {
    let t = u64::from_le_bytes(v);
    if t == 0 {
        return String::new();
    }
    let secs = (t / 10_000_000) as i64 - 11_644_473_600;
    let (y, m, d) = crate::html::civil(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", s / 3600, s / 60 % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    const EML: &str = "From: =?utf-8?Q?Jos=C3=A9_P=C3=A9rez?= <jose@example.com>\r\n\
To: ana@example.com,\r\n\tluis@example.com\r\n\
Subject: =?utf-8?B?SW5mb3JtZSBkZSB2ZW50YXM=?= =?iso-8859-1?Q?_a=F1o?=\r\n\
Date: Tue, 29 Sep 2026 10:00:00 +0200\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"XYZ\"\r\n\
\r\n\
This is a multi-part message in MIME format.\r\n\
--XYZ\r\n\
Content-Type: multipart/alternative; boundary=ALT\r\n\
\r\n\
--ALT\r\n\
Content-Type: text/plain; charset=iso-8859-1\r\n\
Content-Transfer-Encoding: quoted-printable\r\n\
\r\n\
Hola, el informe del a=F1o est=E1 adjunto. Una l=ED=\r\nnea larga.\r\n\
--ALT\r\n\
Content-Type: text/html; charset=utf-8\r\n\
\r\n\
<p>Hola</p>\r\n\
--ALT--\r\n\
--XYZ\r\n\
Content-Type: application/pdf; name=\"x.pdf\"\r\n\
Content-Disposition: attachment; filename*=utf-8''ventas%20a%C3%B1o.pdf\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
JVBERi0xLjQK\r\n\
--XYZ--\r\n";

    #[test]
    fn eml_multipart() {
        let m = read(EML.as_bytes()).unwrap();
        assert_eq!(m.subject, "Informe de ventas año");
        assert_eq!(m.from, "José Pérez <jose@example.com>");
        assert_eq!(m.to, "ana@example.com, luis@example.com");
        assert_eq!(m.date, "Tue, 29 Sep 2026 10:00:00 +0200");
        assert_eq!(m.body_text.trim_end(), "Hola, el informe del año está adjunto. Una línea larga.");
        assert_eq!(m.body_html.as_deref(), Some("<p>Hola</p>"));
        assert_eq!(m.attachments, vec![("ventas año.pdf".to_string(), b"%PDF-1.4\n".to_vec())]);
        let doc = document(&m);
        assert_eq!(doc.paragraphs[0].text, "Informe de ventas año");
        assert_eq!(doc.paragraphs[0].style.kind, ParaKind::Heading(1));
        assert_eq!(doc.paragraphs[1].text, "From: José Pérez <jose@example.com>");
        assert!(doc.paragraphs[1].runs[0].attrs.bold && !doc.paragraphs[1].runs[1].attrs.bold);
        assert_eq!(doc.paragraphs.last().unwrap().text, "Attachments: ventas año.pdf (9 bytes)");
        assert_eq!(doc.meta.title, "Informe de ventas año");
    }

    #[test]
    fn eml_html_only() {
        let m = read_eml(b"Subject: Hi\nContent-Type: text/html\n\n<html><head><style>p{}</style></head><body><p>One &amp; two</p><ul><li>a</li><li>b</li></ul>x<br>y</body></html>");
        assert_eq!(m.body(), "One & two\n\n• a\n• b\n\nx\ny");
        // No headers at all: plain text.
        assert_eq!(read_eml(b"\nJust text").body_text, "Just text");
    }

    /// A small .msg: the property streams Outlook writes.
    fn msg() -> Vec<u8> {
        let mut c = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        let utf16 = |s: &str| s.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>();
        let put = |c: &mut cfb::CompoundFile<Cursor<Vec<u8>>>, path: &str, data: &[u8]| c.create_stream(path).unwrap().write_all(data).unwrap();
        put(&mut c, "/__substg1.0_0037001F", &utf16("Quarterly report"));
        put(&mut c, "/__substg1.0_0C1A001F", &utf16("Ana Gómez"));
        put(&mut c, "/__substg1.0_0C1F001E", b"/O=EXCHANGE/CN=ANA");
        put(&mut c, "/__substg1.0_5D01001F", &utf16("ana@example.com"));
        put(&mut c, "/__substg1.0_0E04001F", &utf16("Luis"));
        put(&mut c, "/__substg1.0_1000001E", b"Figures attached.\r\nRegards");
        // Header, then PR_CLIENT_SUBMIT_TIME: 2026-09-29 08:30 UTC.
        let mut props = vec![0u8; 32];
        props.extend_from_slice(&(0x0039_0040u32).to_le_bytes());
        props.extend_from_slice(&6u32.to_le_bytes());
        let ft = (1_790_670_600u64 + 11_644_473_600) * 10_000_000;
        props.extend_from_slice(&ft.to_le_bytes());
        put(&mut c, "/__properties_version1.0", &props);
        c.create_storage("/__attach_version1.0_#00000000").unwrap();
        put(&mut c, "/__attach_version1.0_#00000000/__substg1.0_3707001F", &utf16("figures.csv"));
        put(&mut c, "/__attach_version1.0_#00000000/__substg1.0_37010102", b"a,b\n1,2\n");
        c.flush().unwrap();
        c.into_inner().into_inner()
    }

    #[test]
    fn outlook_msg() {
        let m = read(&msg()).unwrap();
        assert_eq!(m.subject, "Quarterly report");
        assert_eq!(m.from, "Ana Gómez <ana@example.com>");
        assert_eq!(m.to, "Luis");
        assert_eq!(m.date, "2026-09-29 08:30 UTC");
        assert_eq!(m.body_text, "Figures attached.\nRegards");
        assert_eq!(m.attachments, vec![("figures.csv".to_string(), b"a,b\n1,2\n".to_vec())]);
        assert_eq!(crate::sniff(&msg()), Some(crate::Format::Mail));
    }
}
