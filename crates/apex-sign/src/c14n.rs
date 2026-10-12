//! A small XML tree and its canonical form (Canonical XML 1.0 and 1.1,
//! and Exclusive Canonical XML, with or without comments): what XML
//! Signature digests and signs.

use std::collections::BTreeMap;

use crate::Error;

pub const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

#[derive(Clone, Debug)]
pub enum Node {
    Elem(Elem),
    Text(String),
    Comment(String),
    Pi(String, String),
}

#[derive(Clone, Debug, Default)]
pub struct Elem {
    /// Qualified name as written.
    pub name: String,
    /// Attributes other than namespace declarations.
    pub attrs: Vec<(String, String)>,
    /// Namespace declarations: prefix ("" for the default) and URI.
    pub ns: Vec<(String, String)>,
    pub children: Vec<Node>,
}

#[derive(Clone, Debug, Default)]
pub struct Doc {
    /// The root element with the comments and instructions around it.
    pub nodes: Vec<Node>,
    /// Where each Id is (an element entry, each with its parent's), built
    /// on the first look-up: a signature looks up many.
    #[allow(clippy::type_complexity)]
    ids: std::cell::OnceCell<(Vec<(usize, usize)>, std::collections::HashMap<String, usize>)>,
}

/// Nodes a document may have: more would only take memory.
const MAX_NODES: usize = 2_000_000;

fn prefix_of(name: &str) -> &str {
    name.split_once(':').map_or("", |(p, _)| p)
}

fn local(name: &str) -> &str {
    name.rsplit_once(':').map_or(name, |(_, l)| l)
}

impl Elem {
    pub fn local(&self) -> &str {
        local(&self.name)
    }

    /// An attribute by its name, or by local name when there is none of
    /// that name exactly: the rule documents are read by
    /// (`apex_io::xml`), so that what is checked is what is shown.
    pub fn attr(&self, name: &str) -> Option<&str> {
        let exact = self.attrs.iter().find(|(k, _)| k == name);
        exact.or_else(|| self.attrs.iter().find(|(k, _)| !name.contains(':') && local(k) == name)).map(|(_, v)| v.as_str())
    }

    pub fn elems(&self) -> impl Iterator<Item = &Elem> {
        self.children.iter().filter_map(|n| if let Node::Elem(e) = n { Some(e) } else { None })
    }

    /// The first child element with local name `name`.
    pub fn child(&self, name: &str) -> Option<&Elem> {
        self.elems().find(|e| e.local() == name)
    }

    /// The first element named `name` below this one (depth first).
    pub fn find(&self, name: &str) -> Option<&Elem> {
        for e in self.elems() {
            if e.local() == name {
                return Some(e);
            }
            if let Some(f) = e.find(name) {
                return Some(f);
            }
        }
        None
    }

    /// Every element named `name` below this one.
    pub fn find_all<'a>(&'a self, name: &str, out: &mut Vec<&'a Elem>) {
        for e in self.elems() {
            if e.local() == name {
                out.push(e);
            }
            e.find_all(name, out);
        }
    }

    /// The text inside, all of it.
    pub fn text(&self) -> String {
        let mut s = String::new();
        for n in &self.children {
            match n {
                Node::Text(t) => s.push_str(t),
                Node::Elem(e) => s.push_str(&e.text()),
                _ => {}
            }
        }
        s
    }
}

impl Doc {
    pub fn root(&self) -> Option<&Elem> {
        self.nodes.iter().find_map(|n| if let Node::Elem(e) = n { Some(e) } else { None })
    }

    pub fn new(nodes: Vec<Node>) -> Doc {
        Doc { nodes, ids: Default::default() }
    }

    /// The element whose Id (or ID, id) is `id`, with its ancestors
    /// (the root first, the element last). The first in document order
    /// wins.
    pub fn find_id(&self, id: &str) -> Option<Vec<&Elem>> {
        let root = self.root()?;
        let (nodes, ids) = self.ids.get_or_init(|| {
            // Every element once, as (its parent's entry, its index among
            // the parent's children), in document order.
            let mut nodes: Vec<(usize, usize)> = Vec::new();
            let mut map = std::collections::HashMap::new();
            let mut stack: Vec<(&Elem, usize, usize)> = vec![(root, usize::MAX, 0)];
            while let Some((e, parent, at)) = stack.pop() {
                let me = nodes.len();
                nodes.push((parent, at));
                for (k, v) in &e.attrs {
                    if matches!(local(k), "Id" | "ID" | "id") {
                        map.entry(v.clone()).or_insert(me);
                    }
                }
                for (i, c) in e.children.iter().enumerate().rev() {
                    if let Node::Elem(c) = c {
                        stack.push((c, me, i));
                    }
                }
            }
            (nodes, map)
        });
        let mut steps = Vec::new();
        let mut k = *ids.get(id)?;
        while k != 0 {
            let (parent, at) = nodes[k];
            steps.push(at);
            k = parent;
        }
        let mut path = vec![root];
        for &i in steps.iter().rev() {
            let Some(Node::Elem(c)) = path.last()?.children.get(i) else { return None };
            path.push(c);
        }
        Some(path)
    }
}

// --- parsing -------------------------------------------------------------------------

fn bad(msg: &str) -> Error {
    Error::new(format!("bad XML: {msg}"))
}

/// Replaces the references of a text or attribute value.
fn unescape(s: &str) -> Result<String, Error> {
    if !s.contains('&') {
        return Ok(s.to_string());
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 1..];
        let end = tail.find(';').ok_or_else(|| bad("reference"))?;
        // Longer than the document's reader decodes (it would read the
        // text as written, not the character the digest covers).
        if end > 10 {
            return Err(bad("reference"));
        }
        let name = &tail[..end];
        let c = match name {
            "lt" => '<',
            "gt" => '>',
            "amp" => '&',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let code = if let Some(h) = name.strip_prefix("#x") {
                    u32::from_str_radix(h, 16).ok()
                } else if let Some(d) = name.strip_prefix('#') {
                    d.parse().ok()
                } else {
                    None
                };
                code.and_then(char::from_u32).ok_or_else(|| bad("reference"))?
            }
        };
        out.push(c);
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Parses a document (bytes in UTF-8, with or without a BOM).
pub fn parse(data: &[u8]) -> Result<Doc, Error> {
    let data = data.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(data);
    let text = std::str::from_utf8(data).map_err(|_| bad("not UTF-8"))?;
    // Line ends are normalized before anything else.
    let text = if text.contains('\r') { text.replace("\r\n", "\n").replace('\r', "\n") } else { text.to_string() };
    let mut p = Parser { s: &text, i: 0 };
    let mut stack: Vec<Elem> = Vec::new();
    let mut doc = Doc::default();
    let mut count = 0usize;
    fn push(stack: &mut [Elem], doc: &mut Doc, n: Node) {
        let list = match stack.last_mut() {
            Some(e) => &mut e.children,
            None => &mut doc.nodes,
        };
        if let (Node::Text(t), Some(Node::Text(prev))) = (&n, list.last_mut()) {
            prev.push_str(t);
            return;
        }
        list.push(n);
    }
    while p.i < p.s.len() {
        count += 1;
        if count > MAX_NODES {
            return Err(bad("too many nodes"));
        }
        let rest = &p.s[p.i..];
        if !rest.starts_with('<') {
            let end = rest.find('<').unwrap_or(rest.len());
            if !stack.is_empty() {
                push(&mut stack, &mut doc, Node::Text(unescape(&rest[..end])?));
            }
            p.i += end;
            continue;
        }
        if let Some(r) = rest.strip_prefix("<!--") {
            let end = r.find("-->").ok_or_else(|| bad("comment"))?;
            push(&mut stack, &mut doc, Node::Comment(r[..end].to_string()));
            p.i += 4 + end + 3;
        } else if let Some(r) = rest.strip_prefix("<![CDATA[") {
            let end = r.find("]]>").ok_or_else(|| bad("CDATA"))?;
            push(&mut stack, &mut doc, Node::Text(r[..end].to_string()));
            p.i += 9 + end + 3;
        } else if rest.starts_with("<!") {
            // A document type declaration, internal subset included. One
            // inside the content, or with a bracket closed before it was
            // opened, is refused: the document's own reader could end it
            // elsewhere and see content the digest left out.
            if !stack.is_empty() {
                return Err(bad("declaration inside the content"));
            }
            let mut depth = 0;
            let mut end = None;
            for (k, c) in rest.char_indices() {
                match c {
                    '[' => depth += 1,
                    ']' if depth == 0 => return Err(bad("doctype")),
                    ']' => depth -= 1,
                    '>' if depth == 0 => {
                        end = Some(k);
                        break;
                    }
                    _ => {}
                }
            }
            p.i += end.ok_or_else(|| bad("doctype"))? + 1;
        } else if let Some(r) = rest.strip_prefix("<?") {
            let end = r.find("?>").ok_or_else(|| bad("instruction"))?;
            let body = &r[..end];
            let (target, data) = body.split_once(|c: char| c.is_whitespace()).map_or((body, ""), |(t, d)| (t, d.trim_start()));
            if target != "xml" {
                push(&mut stack, &mut doc, Node::Pi(target.to_string(), data.to_string()));
            }
            p.i += 2 + end + 2;
        } else if let Some(r) = rest.strip_prefix("</") {
            let end = r.find('>').ok_or_else(|| bad("end tag"))?;
            let name = r[..end].trim();
            let e = stack.pop().ok_or_else(|| bad("unbalanced"))?;
            if e.name != name {
                return Err(bad("mismatched tag"));
            }
            push(&mut stack, &mut doc, Node::Elem(e));
            p.i += 2 + end + 1;
        } else {
            let (e, empty) = p.start_tag()?;
            if empty {
                push(&mut stack, &mut doc, Node::Elem(e));
            } else {
                stack.push(e);
                // Deeper trees would overflow the stack in the recursive walks.
                if stack.len() > 256 {
                    return Err(bad("too deeply nested"));
                }
            }
        }
    }
    if !stack.is_empty() || doc.root().is_none() {
        return Err(bad("unclosed element"));
    }
    // Readers that take a second root would see what wasn't signed.
    if doc.nodes.iter().filter(|n| matches!(n, Node::Elem(_))).count() > 1 {
        return Err(bad("more than one root element"));
    }
    Ok(doc)
}

struct Parser<'a> {
    s: &'a str,
    i: usize,
}

impl Parser<'_> {
    fn skip_ws(&mut self) {
        while self.s[self.i..].starts_with(|c: char| c.is_ascii_whitespace()) {
            self.i += 1;
        }
    }

    fn name(&mut self) -> &str {
        let start = self.i;
        let rest = &self.s[start..];
        let len = rest.find(|c: char| c.is_ascii_whitespace() || matches!(c, '/' | '>' | '=')).unwrap_or(rest.len());
        self.i += len;
        &self.s[start..start + len]
    }

    /// A start tag; `true` when it closes itself.
    fn start_tag(&mut self) -> Result<(Elem, bool), Error> {
        self.i += 1;
        let mut e = Elem { name: self.name().to_string(), ..Default::default() };
        if e.name.is_empty() {
            return Err(bad("element name"));
        }
        loop {
            self.skip_ws();
            let rest = &self.s[self.i..];
            if rest.starts_with("/>") {
                self.i += 2;
                return Ok((e, true));
            }
            if rest.starts_with('>') {
                self.i += 1;
                return Ok((e, false));
            }
            let key = self.name().to_string();
            if key.is_empty() {
                return Err(bad("attribute"));
            }
            self.skip_ws();
            if !self.s[self.i..].starts_with('=') {
                return Err(bad("attribute"));
            }
            self.i += 1;
            self.skip_ws();
            let q = self.s[self.i..].chars().next().filter(|c| *c == '"' || *c == '\'').ok_or_else(|| bad("attribute value"))?;
            let start = self.i + 1;
            let len = self.s[start..].find(q).ok_or_else(|| bad("attribute value"))?;
            let raw = &self.s[start..start + len];
            self.i = start + len + 1;
            // Literal white space becomes spaces; references stay.
            let value = unescape(&raw.replace(['\t', '\n'], " "))?;
            if key == "xmlns" {
                e.ns.push((String::new(), value));
            } else if let Some(p) = key.strip_prefix("xmlns:") {
                e.ns.push((p.to_string(), value));
            } else {
                e.attrs.push((key, value));
            }
        }
    }
}

// --- canonical form ------------------------------------------------------------------

/// Which canonical form.
#[derive(Clone, Debug, Default)]
pub struct Method {
    pub exclusive: bool,
    pub comments: bool,
    /// Exclusive: prefixes treated as in the inclusive form ("#default"
    /// for the default namespace).
    pub prefixes: Vec<String>,
}

impl Method {
    /// From an algorithm URI.
    pub fn from_uri(uri: &str) -> Option<Method> {
        let comments = uri.ends_with("#WithComments");
        let base = uri.trim_end_matches("#WithComments");
        let exclusive = match base {
            "http://www.w3.org/TR/2001/REC-xml-c14n-20010315" | "http://www.w3.org/2006/12/xml-c14n11" => false,
            "http://www.w3.org/2001/10/xml-exc-c14n#" | "http://www.w3.org/2001/10/xml-exc-c14n" => true,
            _ => return None,
        };
        Some(Method { exclusive, comments, prefixes: Vec::new() })
    }
}

fn esc_text(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#xD;"),
            c => out.push(c),
        }
    }
}

fn esc_attr(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' => out.push_str("&quot;"),
            '\t' => out.push_str("&#x9;"),
            '\n' => out.push_str("&#xA;"),
            '\r' => out.push_str("&#xD;"),
            c => out.push(c),
        }
    }
}

type Scope = BTreeMap<String, String>;

struct Writer<'a> {
    m: &'a Method,
    out: String,
    /// Namespace bookkeeping done so far: a file's many declarations
    /// over many elements would take hours.
    work: usize,
}

/// Most namespace bookkeeping one canonical form takes.
const MAX_WORK: usize = 200_000_000;

impl<'a> Writer<'a> {
    fn new(m: &'a Method) -> Self {
        Writer { m, out: String::new(), work: 0 }
    }

    /// What was written; past the work allowed, bytes no one can know
    /// beforehand, so no digest made or checked over them ever matches.
    fn finish(self) -> Vec<u8> {
        if self.work <= MAX_WORK {
            return self.out.into_bytes();
        }
        use std::hash::{BuildHasher, Hasher};
        let r = |k: u64| {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u64(k);
            h.finish()
        };
        format!("<!-- too costly to canonicalize {:x}{:x} -->", r(1), r(2)).into_bytes()
    }

    /// Writes `e`: `scope` holds the namespaces in scope at its parent,
    /// `rendered` those its nearest written ancestor declared, `xml_attrs`
    /// the xml:* attributes it inherits (the subset's top only).
    fn elem(&mut self, e: &Elem, scope: &Scope, rendered: &Scope, xml_attrs: &[(String, String)]) {
        self.work += 1;
        if self.work > MAX_WORK {
            return;
        }
        // In step with what was written: nothing new to declare.
        let same = e.ns.is_empty() && std::ptr::eq(scope, rendered);
        let own;
        let scope = if e.ns.is_empty() {
            scope
        } else {
            self.work += scope.len();
            let mut s = scope.clone();
            for (p, u) in &e.ns {
                s.insert(p.clone(), u.clone());
            }
            own = s;
            &own
        };
        let uri_of = |p: &str| -> String {
            if p == "xml" {
                return XML_NS.to_string();
            }
            scope.get(p).cloned().unwrap_or_default()
        };
        // Namespace declarations to write.
        let mut decls: Vec<(String, String)> = Vec::new();
        let differs = |p: &str, u: &str| match rendered.get(p) {
            Some(r) => r != u,
            None => !(p.is_empty() && u.is_empty()),
        };
        if self.m.exclusive {
            let mut used: Vec<String> = vec![prefix_of(&e.name).to_string()];
            for (k, _) in &e.attrs {
                let p = prefix_of(k);
                if !p.is_empty() {
                    used.push(p.to_string());
                }
            }
            self.work += self.m.prefixes.len();
            for p in &self.m.prefixes {
                let p = if p == "#default" { "" } else { p.as_str() };
                if scope.contains_key(p) {
                    used.push(p.to_string());
                }
            }
            used.sort();
            used.dedup();
            for p in used {
                if p == "xml" {
                    continue;
                }
                let u = uri_of(&p);
                if differs(&p, &u) {
                    decls.push((p, u));
                }
            }
        } else if !same {
            self.work += scope.len();
            for (p, u) in scope {
                if p != "xml" && differs(p, u) {
                    decls.push((p.clone(), u.clone()));
                }
            }
        }
        // Attributes, by namespace URI and then local name.
        let mut attrs: Vec<(String, String, &str, &str)> = e.attrs.iter().map(|(k, v)| {
            let p = prefix_of(k);
            let uri = if p.is_empty() { String::new() } else { uri_of(p) };
            (uri, local(k).to_string(), k.as_str(), v.as_str())
        }).collect();
        for (k, v) in xml_attrs {
            if !e.attrs.iter().any(|(a, _)| a == k) {
                attrs.push((XML_NS.to_string(), local(k).to_string(), k.as_str(), v.as_str()));
            }
        }
        attrs.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        self.out.push('<');
        self.out.push_str(&e.name);
        for (p, u) in &decls {
            self.out.push_str(if p.is_empty() { " xmlns" } else { " xmlns:" });
            self.out.push_str(p);
            self.out.push_str("=\"");
            esc_attr(u, &mut self.out);
            self.out.push('"');
        }
        for (_, _, k, v) in &attrs {
            self.out.push(' ');
            self.out.push_str(k);
            self.out.push_str("=\"");
            esc_attr(v, &mut self.out);
            self.out.push('"');
        }
        self.out.push('>');
        let now_rendered;
        let rendered = if decls.is_empty() {
            rendered
        } else if self.m.exclusive {
            self.work += rendered.len();
            let mut r = rendered.clone();
            for (p, u) in decls {
                r.insert(p, u);
            }
            now_rendered = r;
            &now_rendered
        } else {
            scope
        };
        for n in &e.children {
            match n {
                Node::Elem(c) => self.elem(c, scope, rendered, &[]),
                Node::Text(t) => esc_text(t, &mut self.out),
                Node::Comment(c) if self.m.comments => {
                    self.out.push_str("<!--");
                    self.out.push_str(c);
                    self.out.push_str("-->");
                }
                Node::Comment(_) => {}
                Node::Pi(t, d) => self.pi(t, d),
            }
        }
        self.out.push_str("</");
        self.out.push_str(&e.name);
        self.out.push('>');
    }

    fn pi(&mut self, target: &str, data: &str) {
        self.out.push_str("<?");
        self.out.push_str(target);
        if !data.is_empty() {
            self.out.push(' ');
            self.out.push_str(data);
        }
        self.out.push_str("?>");
    }
}

/// The canonical form of a whole document.
pub fn doc(d: &Doc, m: &Method) -> Vec<u8> {
    let mut w = Writer::new(m);
    let mut after = false;
    let empty = Scope::new();
    for n in &d.nodes {
        match n {
            Node::Elem(e) => {
                w.elem(e, &empty, &empty, &[]);
                after = true;
            }
            Node::Comment(c) if m.comments => {
                if after {
                    w.out.push('\n');
                }
                w.out.push_str(&format!("<!--{c}-->"));
                if !after {
                    w.out.push('\n');
                }
            }
            Node::Pi(t, d) => {
                if after {
                    w.out.push('\n');
                }
                w.pi(t, d);
                if !after {
                    w.out.push('\n');
                }
            }
            _ => {}
        }
    }
    w.finish()
}

/// The canonical form of the last element of `path` (its ancestors
/// before it): the namespaces of the ancestors are in scope.
pub fn subtree(path: &[&Elem], m: &Method) -> Vec<u8> {
    let Some((e, ancestors)) = path.split_last() else { return Vec::new() };
    let mut scope = Scope::new();
    let mut xml_attrs: Vec<(String, String)> = Vec::new();
    for a in ancestors {
        for (p, u) in &a.ns {
            scope.insert(p.clone(), u.clone());
        }
        for (k, v) in &a.attrs {
            if prefix_of(k) == "xml" {
                xml_attrs.retain(|(x, _)| x != k);
                xml_attrs.push((k.clone(), v.clone()));
            }
        }
    }
    if m.exclusive {
        xml_attrs.clear();
    }
    let mut w = Writer::new(m);
    w.elem(e, &scope, &Scope::new(), &xml_attrs);
    w.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c14n(xml: &str) -> String {
        String::from_utf8(doc(&parse(xml.as_bytes()).unwrap(), &Method::default())).unwrap()
    }

    #[test]
    fn spec_examples() {
        // Canonical XML 1.0, section 3.1 (PIs, comments, outside element).
        let x = "<?xml version=\"1.0\"?>\n\n<?xml-stylesheet   href=\"doc.xsl\"\n   type=\"text/xsl\"   ?>\n\n<!DOCTYPE doc SYSTEM \"doc.dtd\">\n\n<doc>Hello, world!<!-- Comment 1 --></doc>\n\n<?pi-without-data     ?>\n\n<!-- Comment 2 -->\n\n<!-- Comment 3 -->";
        assert_eq!(c14n(x), "<?xml-stylesheet href=\"doc.xsl\"\n   type=\"text/xsl\"   ?>\n<doc>Hello, world!</doc>\n<?pi-without-data?>");
        // Section 3.3: start and end tags.
        let x = "<doc>\n   <e1   />\n   <e2   ></e2>\n   <e3   name = \"elem3\"   id=\"elem3\"   />\n   <e5 a:attr=\"out\" b:attr=\"sorted\" attr2=\"all\" attr=\"I'm\"\n      xmlns:b=\"http://www.ietf.org\"\n      xmlns:a=\"http://www.w3.org\"\n      xmlns=\"http://example.org\"/>\n</doc>";
        assert_eq!(c14n(x), "<doc>\n   <e1></e1>\n   <e2></e2>\n   <e3 id=\"elem3\" name=\"elem3\"></e3>\n   <e5 xmlns=\"http://example.org\" xmlns:a=\"http://www.w3.org\" xmlns:b=\"http://www.ietf.org\" attr=\"I'm\" attr2=\"all\" b:attr=\"sorted\" a:attr=\"out\"></e5>\n</doc>");
        // Character references and escaping.
        let x = "<doc a=\"&#9;x&#10;y\r\nz\">&lt;&gt;&amp;\"&#13;<![CDATA[<>]]></doc>";
        assert_eq!(c14n(x), "<doc a=\"&#x9;x&#xA;y z\">&lt;&gt;&amp;\"&#xD;&lt;&gt;</doc>");
    }

    #[test]
    fn declarations_that_readers_end_elsewhere() {
        // Hidden from the digest, read as content by the document's reader.
        assert!(parse(b"<r><!]><x>forged</x>[></r>").is_err());
        assert!(parse(b"<!DOCTYPE r]><x/>[><r/>").is_err());
        assert!(parse(b"<!DOCTYPE r [<!ENTITY a \"b\">]><r/>").is_ok());
    }

    #[test]
    fn deep_nesting_is_refused() {
        let x = format!("{}{}", "<a>".repeat(100_000), "</a>".repeat(100_000));
        assert!(parse(x.as_bytes()).is_err());
        let x = format!("{}{}", "<a>".repeat(200), "</a>".repeat(200));
        assert!(parse(x.as_bytes()).is_ok());
    }

    #[test]
    fn subsets_and_namespaces() {
        let x = parse(b"<a:r xmlns:a=\"urn:a\" xmlns:b=\"urn:b\" xmlns=\"urn:d\" xml:lang=\"en\"><a:s Id=\"x\"><t b:k=\"1\"/></a:s></a:r>").unwrap();
        let path = x.find_id("x").unwrap();
        let inc = String::from_utf8(subtree(&path, &Method::default())).unwrap();
        assert_eq!(inc, "<a:s xmlns=\"urn:d\" xmlns:a=\"urn:a\" xmlns:b=\"urn:b\" Id=\"x\" xml:lang=\"en\"><t b:k=\"1\"></t></a:s>");
        let exc = String::from_utf8(subtree(&path, &Method { exclusive: true, ..Default::default() })).unwrap();
        assert_eq!(exc, "<a:s xmlns:a=\"urn:a\" Id=\"x\"><t xmlns=\"urn:d\" xmlns:b=\"urn:b\" b:k=\"1\"></t></a:s>");
        // xmlns="" only where a default was in force.
        assert_eq!(c14n("<a xmlns=\"urn:x\"><b xmlns=\"\"><c xmlns=\"\"/></b></a>"), "<a xmlns=\"urn:x\"><b xmlns=\"\"><c></c></b></a>");
        assert_eq!(c14n("<a><b xmlns=\"\"/></a>"), "<a><b></b></a>");
    }
}
