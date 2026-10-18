//! Mutations of the XML parts of a package, on the text: elements removed,
//! duplicated, moved, renamed or nested very deep; attributes removed,
//! repeated or set to extreme values; numbers and references broken;
//! documents cut short. A light tokenizer finds the tags; malformed input
//! is fine (it only yields fewer elements).

use super::rng::Rng;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TagKind {
    Open,
    Close,
    Empty,
}

#[derive(Clone, Debug)]
struct Tag {
    start: usize,
    /// After the `>`.
    end: usize,
    name_end: usize,
    kind: TagKind,
}

/// An element: its start tag and its whole span (to the end of the
/// closing tag).
#[derive(Clone, Debug)]
pub struct Elem {
    pub start: usize,
    pub open_end: usize,
    pub close_start: usize,
    pub end: usize,
    pub name: String,
}

fn tags(s: &str) -> Vec<Tag> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(p) = b[i..].iter().position(|&c| c == b'<') {
        let start = i + p;
        let rest = &s[start..];
        let skip_to = |pat: &str| rest.find(pat).map(|e| start + e + pat.len());
        let special = if rest.starts_with("<!--") {
            Some(skip_to("-->"))
        } else if rest.starts_with("<![CDATA[") {
            Some(skip_to("]]>"))
        } else if rest.starts_with("<?") {
            Some(skip_to("?>"))
        } else if rest.starts_with("<!") {
            Some(skip_to(">"))
        } else {
            None
        };
        if let Some(end) = special {
            match end {
                Some(e) => i = e,
                None => break,
            }
            continue;
        }
        // A tag: up to the `>` outside quotes.
        let mut j = start + 1;
        let mut quote = 0u8;
        while j < b.len() {
            let c = b[j];
            if quote != 0 {
                if c == quote {
                    quote = 0;
                }
            } else if c == b'"' || c == b'\'' {
                quote = c;
            } else if c == b'>' {
                break;
            }
            j += 1;
        }
        if j >= b.len() {
            break;
        }
        let end = j + 1;
        let close = b.get(start + 1) == Some(&b'/');
        let name_start = start + 1 + close as usize;
        let name_end = name_start + b[name_start..j].iter().take_while(|c| !c.is_ascii_whitespace() && **c != b'/' && **c != b'>').count();
        let kind = if close {
            TagKind::Close
        } else if b[j - 1] == b'/' {
            TagKind::Empty
        } else {
            TagKind::Open
        };
        if name_end > name_start {
            out.push(Tag { start, end, name_end, kind });
        }
        i = end;
    }
    out
}

fn tag_name<'a>(s: &'a str, t: &Tag) -> &'a str {
    &s[t.start + 1 + (t.kind == TagKind::Close) as usize..t.name_end]
}

/// The elements of a document, in document order (outer before inner).
pub fn elements(s: &str) -> Vec<Elem> {
    let tags = tags(s);
    let mut out: Vec<Elem> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for t in &tags {
        let name = tag_name(s, t);
        match t.kind {
            TagKind::Empty => out.push(Elem { start: t.start, open_end: t.end, close_start: t.end, end: t.end, name: name.to_string() }),
            TagKind::Open => {
                stack.push(out.len());
                out.push(Elem { start: t.start, open_end: t.end, close_start: usize::MAX, end: usize::MAX, name: name.to_string() });
            }
            TagKind::Close => {
                if let Some(pos) = stack.iter().rposition(|&i| out[i].name == name) {
                    let i = stack[pos];
                    stack.truncate(pos);
                    out[i].close_start = t.start;
                    out[i].end = t.end;
                }
            }
        }
    }
    out.retain(|e| e.end != usize::MAX);
    out
}

/// Attributes of a start tag: (whole span, value span).
fn attributes(s: &str, e: &Elem) -> Vec<((usize, usize), (usize, usize))> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = e.start + 1 + e.name.len();
    let end = e.open_end;
    while i < end {
        while i < end && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let ns = i;
        while i < end && b[i] != b'=' && !b[i].is_ascii_whitespace() && b[i] != b'>' && b[i] != b'/' {
            i += 1;
        }
        if i >= end || b[i] != b'=' || i == ns {
            i += 1;
            continue;
        }
        i += 1;
        let Some(&q) = b.get(i) else { break };
        if q != b'"' && q != b'\'' {
            continue;
        }
        let vs = i + 1;
        let Some(len) = b[vs..end].iter().position(|&c| c == q) else { break };
        out.push(((ns, vs + len + 1), (vs, vs + len)));
        i = vs + len + 1;
    }
    out
}

/// Values that break readers: empty, huge, negative, not numbers, broken
/// cell references and relations, odd characters.
pub const VALUES: &[&str] = &[
    "", "0", "-1", "1", "-2147483648", "2147483647", "2147483648", "4294967295", "4294967296", "-9223372036854775809", "18446744073709551616",
    "99999999999999999999999999999", "1e308", "-1e308", "1e-320", "NaN", "INF", "-0", "0.5", "1048576", "1048577", "16384", "16385", "65535",
    "65536", "A0", "XFD1048576", "XFE1", "A1:XFD1048576", "ZZZZZZ99999999", "$A$1:$B$-1", "B2:A1", "A1:A1:A1", "#REF!", "#REF!A1", "Sheet99!A1",
    "'", "\"", "rId0", "rId999999", "../../../x.xml", "/", "//", "C:\\x", "http://example.invalid/x", "true", "false", "none", "auto", "\u{202E}",
    "\u{1F600}", "=1/0", "=A1", "=SUM(A:A)", "=INDIRECT(\"A\"&ROW())", "=A1+", "=((((((((((1))))))))))", "=LAMBDA(x,x)(1)", "#,##0.00;[Red]",
    "@@@@@@@@", "[$-F800]dddd", "yyyy-mm-dd hh:mm:ss.000000000", "0.0E+99999", "-2", "3.4028235e38", "0x10", "1pt", "-5cm", "99999in", "1e9mm",
    "100%", "-100%", "#GGGGGG", "#", "rgb(1,2", "x", "sans-serif", "Symbol", "\t", "a b", "P1", "T1", "Table1", "Heading 1", "ooow:x", "of:=1",
];

pub const NUMBERS: &[&str] = &["0", "-1", "1", "2", "255", "256", "65535", "65536", "2147483647", "2147483648", "4294967296", "1048576", "1048577", "16384", "16385", "99999999999", "1e308", "-99999"];

fn escape(v: &str) -> String {
    v.replace('&', "&amp;").replace('<', "&lt;").replace('"', "&quot;")
}

/// One random change to the XML text; `other` is XML of another file to
/// take elements from. None when the text has no elements to work on.
pub fn mutate(rng: &mut Rng, s: &str, other: Option<&str>) -> Option<String> {
    let els = elements(s);
    if els.is_empty() {
        return None;
    }
    // Bias to the inner elements: the records (cells, runs, rows).
    let pick = |rng: &mut Rng| -> Elem {
        let i = if rng.one_in(3) { rng.below(els.len()) } else { els.len() - 1 - rng.below(els.len().min(64)).min(els.len() - 1) };
        let i = if rng.one_in(2) { rng.below(els.len()) } else { i };
        els[i].clone()
    };
    let e = pick(rng);
    let mut out = s.to_string();
    match rng.below(20) {
        0 | 1 => out.replace_range(e.start..e.end, ""),
        2 => {
            let copy = s[e.start..e.end].to_string();
            let n = if rng.one_in(4) { *rng.pick(&[2usize, 10, 1000, 20_000]) } else { 1 };
            let total = copy.len().saturating_mul(n);
            if total > 32 << 20 {
                return None;
            }
            out.insert_str(e.end, &copy.repeat(n));
        }
        3..=5 => {
            // An attribute set to an extreme value.
            let attrs = attributes(s, &e);
            if attrs.is_empty() {
                return None;
            }
            let (_, (vs, ve)) = *rng.pick(&attrs);
            let v = rng.pick(VALUES);
            let v = if rng.one_in(8) { v.repeat(rng.small_len(5000)) } else { v.to_string() };
            out.replace_range(vs..ve, &if rng.one_in(6) { v } else { escape(&v) });
        }
        6 => {
            // A number inside an attribute value changed (A1 -> A1048577).
            let attrs = attributes(s, &e);
            let with_digits: Vec<_> = attrs.iter().filter(|(_, (vs, ve))| s[*vs..*ve].bytes().any(|b| b.is_ascii_digit())).collect();
            if with_digits.is_empty() {
                return None;
            }
            let (_, (vs, ve)) = **rng.pick(&with_digits);
            let v = &s[vs..ve];
            let p = v.find(|c: char| c.is_ascii_digit()).unwrap_or(0);
            let q = p + v[p..].find(|c: char| !c.is_ascii_digit()).unwrap_or(v.len() - p);
            let nv = format!("{}{}{}", &v[..p], rng.pick(NUMBERS), &v[q..]);
            out.replace_range(vs..ve, &nv);
        }
        7 => {
            let attrs = attributes(s, &e);
            if attrs.is_empty() {
                return None;
            }
            let ((a, b), _) = *rng.pick(&attrs);
            out.replace_range(a..b, "");
        }
        8 => {
            // An attribute written twice (not well-formed).
            let attrs = attributes(s, &e);
            if attrs.is_empty() {
                return None;
            }
            let ((a, b), _) = *rng.pick(&attrs);
            let dup = format!(" {}", &s[a..b]);
            out.insert_str(b, &dup);
        }
        9 | 10 => {
            // The text of a leaf element.
            if e.close_start <= e.open_end || s[e.open_end..e.close_start].contains('<') {
                return None;
            }
            let v = rng.pick(VALUES);
            out.replace_range(e.open_end..e.close_start, &escape(v));
        }
        11 => {
            // Deep nesting of the element in itself.
            let open = &s[e.start..e.open_end];
            if open.ends_with("/>") {
                return None;
            }
            let close = &s[e.close_start..e.end];
            let n = *rng.pick(&[64usize, 1000, 20_000, 200_000]);
            let inner = &s[e.open_end..e.close_start];
            if (open.len() + close.len()) * n > 32 << 20 {
                return None;
            }
            let mut w = String::with_capacity((open.len() + close.len()) * n + inner.len());
            for _ in 0..n {
                w.push_str(open);
            }
            w.push_str(inner);
            for _ in 0..n {
                w.push_str(close);
            }
            out.replace_range(e.start..e.end, &w);
        }
        12 => {
            // Move the element elsewhere (after another element's start tag).
            let f = pick(rng);
            if f.start >= e.start && f.start < e.end {
                return None;
            }
            let moved = s[e.start..e.end].to_string();
            if f.open_end > e.end {
                out.insert_str(f.open_end, &moved);
                out.replace_range(e.start..e.end, "");
            } else {
                out.replace_range(e.start..e.end, "");
                out.insert_str(f.open_end, &moved);
            }
        }
        13 => {
            // Rename the element (to another name of the document or junk).
            let names: Vec<&str> = els.iter().map(|x| x.name.as_str()).collect();
            let new = if rng.one_in(3) { "x:unknown".to_string() } else { rng.pick(&names).to_string() };
            if e.close_start < e.end && e.close_start != e.open_end {
                out.replace_range(e.close_start + 2..e.close_start + 2 + e.name.len(), &new);
            } else if e.close_start == e.end && e.open_end == e.end {
                // An empty element: only the start tag.
            }
            out.replace_range(e.start + 1..e.start + 1 + e.name.len(), &new);
        }
        14 => {
            // Cut the document short.
            let at = rng.below(s.len());
            let at = (0..=at).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0);
            out.truncate(at);
        }
        15 => {
            // Entities and characters XML forbids, in text or a value.
            let bad = rng.pick(&["&#0;", "&#xFFFFFFFF;", "&#x110000;", "&undefined;", "&", "&#x1;", "\u{1}", "&#55296;", "<", "]]>", "&amp"]);
            let at = if rng.one_in(2) && e.close_start > e.open_end { e.open_end } else { e.start + 1 + e.name.len() };
            out.insert_str(at.min(out.len()), bad);
        }
        16 => {
            // Swap two elements that don't overlap.
            let f = pick(rng);
            let (a, b) = if e.start < f.start { (e, f) } else { (f, e) };
            if a.end > b.start {
                return None;
            }
            let (ta, tb) = (s[a.start..a.end].to_string(), s[b.start..b.end].to_string());
            out.replace_range(b.start..b.end, &ta);
            out.replace_range(a.start..a.end, &tb);
        }
        17 => {
            // An element of another file of the same kind.
            let o = other?;
            let oe = elements(o);
            if oe.is_empty() {
                return None;
            }
            let x = rng.pick(&oe);
            let at = if rng.one_in(2) { e.open_end } else { e.end };
            if e.open_end == e.end && at == e.open_end {
                return None;
            }
            out.insert_str(at, &o[x.start..x.end]);
        }
        18 => {
            // A namespace declaration removed or changed.
            let root = &els[0];
            let attrs = attributes(s, root);
            let ns: Vec<_> = attrs.iter().filter(|((a, _), _)| s[*a..].starts_with("xmlns")).collect();
            if ns.is_empty() {
                return None;
            }
            let ((a, b), (vs, ve)) = **rng.pick(&ns);
            if rng.one_in(2) {
                out.replace_range(a..b, "");
            } else {
                out.replace_range(vs..ve, "urn:x");
            }
        }
        _ => {
            // Many copies of the last child of a random element (rows,
            // cells, paragraphs...).
            let copy = s[e.start..e.end].to_string();
            let n = *rng.pick(&[100usize, 5000, 100_000]);
            if copy.len() * n > 32 << 20 {
                return None;
            }
            out.insert_str(e.end, &copy.repeat(n));
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"<?xml version="1.0"?><!-- c --><a xmlns="urn:a" x="1"><b y='A1'>text</b><c/><b>more</b><![CDATA[<z>]]></a>"#;

    #[test]
    fn finds_elements_and_attributes() {
        let els = elements(DOC);
        let names: Vec<&str> = els.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "c", "b"]);
        assert_eq!(&DOC[els[1].start..els[1].end], "<b y='A1'>text</b>");
        let attrs = attributes(DOC, &els[0]);
        assert_eq!(attrs.len(), 2);
        assert_eq!(&DOC[attrs[1].1.0..attrs[1].1.1], "1");
    }

    #[test]
    fn mutations_keep_utf8_and_change_something() {
        let mut rng = Rng::new(11);
        let mut changed = 0;
        for _ in 0..3000 {
            if let Some(m) = mutate(&mut rng, DOC, Some("<q><r a=\"ñ\"/></q>")) {
                changed += (m != DOC) as usize;
            }
        }
        assert!(changed > 1000);
        assert!(elements("<a><b></a>").iter().any(|e| e.name == "a"));
        assert!(elements("<a b=\"x>").is_empty());
    }
}
