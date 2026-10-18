//! Content controls bound to custom XML (`w:dataBinding`): a control shows
//! the node an XPath picks in a custom XML part (or in the document
//! properties), and editing the control writes the node and every other
//! control bound to it. The XPath is the kind Word writes: absolute steps
//! with namespace prefixes, `[n]` positions and a last `@attribute`.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use apex_doc::{CHECKED, ContentControl, ControlKind, DataBinding, Document, UNCHECKED};

use crate::ooxml_keep::{decode_xml, encode_xml};
use crate::xml::{esc, unescape};

/// Store ids Word gives the document properties.
const CORE_PROPS: &str = "6C3C8BC8-F283-45AE-878A-BAB7291924A1";
const APP_PROPS: &str = "6668398D-A668-4E3E-A5EB-62B293D839F1";

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

fn norm_id(s: &str) -> String {
    s.trim().trim_matches(|c| c == '{' || c == '}').to_ascii_uppercase()
}

// --- a small XML tree with source positions --------------------------------

struct Node {
    name: String,
    parent: Option<usize>,
    children: Vec<usize>,
    /// Namespace declarations: (prefix, "" for the default one; uri).
    decls: Vec<(String, String)>,
    /// Attributes: qualified name and the range of the value.
    attrs: Vec<(String, Range<usize>)>,
    /// Between the tags; `None` for an empty element, whose `/>` is at
    /// `close`.
    content: Option<Range<usize>>,
    close: usize,
}

struct Tree {
    nodes: Vec<Node>,
    /// Steps up the tree its lookups may still take: a deep part looked
    /// up many times (bindings may be many) mustn't take forever.
    work: std::cell::Cell<usize>,
}

/// The largest data part read, and the most elements in it: bound data
/// is a form's few fields.
const MAX_STORE_BYTES: usize = 2 << 20;
const MAX_NODES: usize = 20_000;

impl Tree {
    fn parse(s: &str) -> Option<Tree> {
        if s.len() > MAX_STORE_BYTES {
            return None;
        }
        let b = s.as_bytes();
        let mut nodes: Vec<Node> = Vec::new();
        let mut stack: Vec<usize> = Vec::new();
        let mut i = 0;
        while let Some(off) = s[i..].find('<') {
            let at = i + off;
            let rest = &s[at..];
            if rest.starts_with("<?") {
                i = at + rest.find("?>")? + 2;
            } else if rest.starts_with("<!--") {
                i = at + 4 + rest[4..].find("-->")? + 3;
            } else if rest.starts_with("<![CDATA[") {
                i = at + rest.find("]]>")? + 3;
            } else if rest.starts_with("<!") {
                i = at + rest.find('>')? + 1;
            } else if rest.starts_with("</") {
                let n = stack.pop()?;
                let start = nodes[n].content.as_ref()?.start;
                nodes[n].content = Some(start..at);
                i = at + rest.find('>')? + 1;
            } else {
                // A start tag; '>' may be inside quoted values.
                let mut j = at + 1;
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
                    return None;
                }
                let empty = b[j - 1] == b'/';
                let inner_end = if empty { j - 1 } else { j };
                let inner = &s[at + 1..inner_end];
                let name_end = inner.find(|c: char| c.is_whitespace()).unwrap_or(inner.len());
                let mut node = Node { name: inner[..name_end].to_string(), parent: stack.last().copied(), children: Vec::new(), decls: Vec::new(), attrs: Vec::new(), content: None, close: j - 1 };
                // Attributes with the positions of their values.
                let mut k = at + 1 + name_end;
                while k < inner_end {
                    let a = &s[k..inner_end];
                    let Some(eq) = a.find('=') else { break };
                    let key = a[..eq].trim().to_string();
                    let v = a[eq + 1..].trim_start();
                    let vstart = inner_end - v.len();
                    let q = v.chars().next().filter(|q| matches!(q, '"' | '\''))?;
                    let vend = vstart + 1 + v[1..].find(q)?;
                    if let Some(p) = key.strip_prefix("xmlns:") {
                        node.decls.push((p.to_string(), unescape(&s[vstart + 1..vend]).into_owned()));
                    } else if key == "xmlns" {
                        node.decls.push((String::new(), unescape(&s[vstart + 1..vend]).into_owned()));
                    } else {
                        node.attrs.push((key, vstart + 1..vend));
                    }
                    k = vend + 1;
                }
                if !empty {
                    node.content = Some(j + 1..j + 1);
                }
                let id = nodes.len();
                if id >= MAX_NODES {
                    return None;
                }
                if let Some(&p) = stack.last() {
                    nodes[p].children.push(id);
                }
                nodes.push(node);
                if !empty {
                    stack.push(id);
                }
                i = j + 1;
            }
        }
        (!nodes.is_empty()).then_some(Tree { nodes, work: std::cell::Cell::new(1 << 24) })
    }

    /// The namespace of a prefix at a node.
    fn uri(&self, mut n: usize, prefix: &str) -> String {
        loop {
            let Some(left) = self.work.get().checked_sub(1) else {
                // (No namespace matches this one.)
                return "\u{0}".into();
            };
            self.work.set(left);
            if let Some((_, u)) = self.nodes[n].decls.iter().find(|(p, _)| p == prefix) {
                return u.clone();
            }
            match self.nodes[n].parent {
                Some(p) => n = p,
                None => return String::new(),
            }
        }
    }

    fn element_is(&self, n: usize, uri: &str, name: &str) -> bool {
        let (prefix, local) = split(&self.nodes[n].name);
        local == name && self.uri(n, prefix) == uri
    }
}

fn split(q: &str) -> (&str, &str) {
    q.split_once(':').unwrap_or(("", q))
}

/// What an XPath picks: an element or one of its attributes.
enum Target {
    Element(usize),
    Attr(Range<usize>),
}

/// Evaluates `/a:root[1]/a:item[2]/@b:attr` with `prefixes` like
/// "xmlns:a='urn:x' xmlns:b='urn:y'".
fn select(tree: &Tree, xpath: &str, prefixes: &str) -> Option<Target> {
    let map = prefix_map(prefixes);
    let ns = |p: &str| -> Option<String> { if p.is_empty() { Some(String::new()) } else { map.iter().find(|(k, _)| k == p).map(|(_, v)| v.clone()) } };
    let steps: Vec<&str> = xpath.trim().strip_prefix('/')?.split('/').collect();
    let mut cur: Option<usize> = None;
    for (i, step) in steps.iter().enumerate() {
        let (name, pos) = match step.find('[') {
            Some(b) => (&step[..b], step[b + 1..].strip_suffix(']')?.trim().parse::<usize>().ok()?),
            None => (*step, 1),
        };
        if let Some(attr) = name.strip_prefix('@') {
            if i + 1 != steps.len() {
                return None;
            }
            let n = cur?;
            let (p, local) = split(attr);
            let uri = ns(p)?;
            let (_, range) = tree.nodes[n].attrs.iter().find(|(k, _)| {
                let (kp, kl) = split(k);
                kl == local && if kp.is_empty() { uri.is_empty() } else { tree.uri(n, kp) == uri }
            })?;
            return Some(Target::Attr(range.clone()));
        }
        let (p, local) = split(name);
        let uri = ns(p)?;
        let candidates: &[usize] = match cur {
            None => &[0],
            Some(n) => &tree.nodes[n].children,
        };
        cur = Some(candidates.iter().copied().filter(|&c| tree.element_is(c, &uri, local)).nth(pos.checked_sub(1)?)?);
    }
    cur.map(Target::Element)
}

fn prefix_map(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find("xmlns:") {
        rest = &rest[i + 6..];
        let Some(eq) = rest.find('=') else { break };
        let p = rest[..eq].trim().to_string();
        let v = rest[eq + 1..].trim_start();
        let Some(q) = v.chars().next().filter(|q| matches!(q, '"' | '\'')) else { break };
        let Some(end) = v[1..].find(q) else { break };
        out.push((p, v[1..1 + end].to_string()));
        rest = &v[1 + end..];
    }
    out
}

/// The text of an element (its text nodes, CDATA included).
fn text_of(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("<![CDATA[") {
            let end = r.find("]]>").unwrap_or(r.len());
            out.push_str(&r[..end]);
            rest = r.get(end + 3..).unwrap_or("");
        } else if rest.starts_with('<') {
            rest = rest.find('>').map_or("", |i| &rest[i + 1..]);
        } else {
            let end = rest.find('<').unwrap_or(rest.len());
            out.push_str(&unescape(&rest[..end]));
            rest = &rest[end..];
        }
    }
    out
}

/// The value at `xpath` in an XML part.
#[cfg(test)]
fn get(xml: &str, b: &DataBinding) -> Option<String> {
    get_in(xml, &Tree::parse(xml)?, b)
}

fn get_in(xml: &str, tree: &Tree, b: &DataBinding) -> Option<String> {
    Some(match select(tree, &b.xpath, &b.prefixes)? {
        Target::Attr(r) => unescape(&xml[r]).into_owned(),
        Target::Element(n) => tree.nodes[n].content.as_ref().map_or(String::new(), |r| text_of(&xml[r.clone()])),
    })
}

/// The part with `value` at `xpath` (elements with child elements aren't
/// written over).
fn set(xml: &str, b: &DataBinding, value: &str) -> Option<String> {
    let tree = Tree::parse(xml)?;
    let v = esc(value);
    let mut out = xml.to_string();
    match select(&tree, &b.xpath, &b.prefixes)? {
        Target::Attr(r) => out.replace_range(r, &v),
        Target::Element(n) => {
            let node = &tree.nodes[n];
            match &node.content {
                Some(_) if !node.children.is_empty() => return None,
                Some(r) => out.replace_range(r.clone(), &v),
                None if v.is_empty() => {}
                None => out.replace_range(node.close..node.close + 2, &format!(">{v}</{}>", node.name)),
            }
        }
    }
    Some(out)
}

// --- the stores ---------------------------------------------------------------

/// The kept custom XML parts by the id in their properties (the first
/// with each; properties are a line, a large one is left out).
fn store_parts(doc: &Document) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for p in doc.kept.parts.iter().filter(|p| p.path.starts_with("customXml/itemProps") && p.data.len() <= 64 << 10) {
        let x = decode_xml(&p.data).0;
        let id = (|| {
            let at = x.find("itemID=")?;
            let v = &x[at + 7..];
            let q = v.chars().next().filter(|q| matches!(q, '"' | '\''))?;
            Some(norm_id(&v[1..1 + v[1..].find(q)?]))
        })();
        if let Some(id) = id {
            out.entry(id).or_insert_with(|| p.path.replace("itemProps", "item"));
        }
    }
    out
}

/// The document property a binding to the core or extended properties
/// stands for.
fn meta_field<'a>(doc: &'a mut Document, b: &DataBinding) -> Option<&'a mut String> {
    let id = norm_id(&b.store_item);
    let last = b.xpath.rsplit('/').next()?;
    let local = split(last.split('[').next()?).1;
    let m = &mut doc.meta;
    match (id.as_str(), local) {
        (CORE_PROPS, "title") => Some(&mut m.title),
        (CORE_PROPS, "subject") => Some(&mut m.subject),
        (CORE_PROPS, "creator") => Some(&mut m.author),
        (CORE_PROPS, "keywords") => Some(&mut m.keywords),
        (CORE_PROPS, "description") => Some(&mut m.description),
        (CORE_PROPS, "category") => Some(&mut m.category),
        (APP_PROPS, "Company") => Some(&mut m.company),
        (APP_PROPS, "Manager") => Some(&mut m.manager),
        _ => None,
    }
}

/// What one sync has found of the data parts: each store's part, and
/// each part read (once, until written).
#[derive(Default)]
struct Stores {
    paths: Option<HashMap<String, String>>,
    trees: HashMap<String, Option<(String, Tree)>>,
    /// Bytes of data parts read so far: a sync reads a few parts' worth.
    read: usize,
}

impl Stores {
    fn path(&mut self, doc: &Document, id: &str) -> Option<String> {
        self.paths.get_or_insert_with(|| store_parts(doc)).get(&norm_id(id)).cloned()
    }
}

fn store_get(doc: &mut Document, b: &DataBinding, stores: &mut Stores) -> Option<String> {
    if let Some(f) = meta_field(doc, b) {
        return Some(f.clone());
    }
    let path = stores.path(doc, &b.store_item)?;
    if !stores.trees.contains_key(&path) {
        let read = &mut stores.read;
        let parsed = (|| {
            let data = &doc.kept.part(&path)?.data;
            if data.len() > 2 * MAX_STORE_BYTES || *read > 8 * MAX_STORE_BYTES {
                return None;
            }
            *read += data.len();
            let xml = decode_xml(data).0;
            let tree = Tree::parse(&xml)?;
            Some((xml, tree))
        })();
        stores.trees.insert(path.clone(), parsed);
    }
    let (xml, tree) = stores.trees.get(&path)?.as_ref()?;
    get_in(xml, tree, b)
}

fn store_set(doc: &mut Document, b: &DataBinding, value: &str, stores: &mut Stores) {
    if let Some(f) = meta_field(doc, b) {
        *f = value.to_string();
        return;
    }
    let Some(path) = stores.path(doc, &b.store_item) else { return };
    stores.trees.remove(&path);
    let Some(part) = doc.kept.parts.iter_mut().find(|p| p.path == path) else { return };
    let (xml, enc) = decode_xml(&part.data);
    if let Some(new) = set(&xml, b, value) {
        part.data = Arc::from(encode_xml(&new, enc));
    }
}

// --- controls and values ---------------------------------------------------------

/// The value a control gives its node.
fn value_of(c: &ContentControl, text: &str) -> String {
    if c.placeholder {
        return String::new();
    }
    match &c.kind {
        ControlKind::CheckBox { checked } => checked.to_string(),
        ControlKind::List { items, .. } => items.iter().find(|i| i.text == text).map_or_else(|| text.to_string(), |i| i.value.clone()),
        ControlKind::Date { date, .. } => date.as_ref().map(|d| format!("{d}T00:00:00Z")).unwrap_or_default(),
        _ => text.to_string(),
    }
}

fn same(c: &ContentControl, a: &str, b: &str) -> bool {
    match c.kind {
        ControlKind::Date { .. } => a.get(..10).unwrap_or(a) == b.get(..10).unwrap_or(b),
        ControlKind::CheckBox { .. } => truthy(a) == truthy(b),
        _ => a == b,
    }
}

fn truthy(v: &str) -> bool {
    matches!(v.trim(), "true" | "1")
}

/// A control showing `value`, and its text.
fn with_value(c: &ContentControl, value: &str, text: &str, placeholder: &dyn Fn(&ControlKind) -> String) -> (ContentControl, String) {
    let mut c = c.clone();
    if value.is_empty() && !matches!(c.kind, ControlKind::CheckBox { .. }) {
        let t = if c.placeholder { text.to_string() } else { placeholder(&c.kind) };
        if let ControlKind::Date { date, .. } = &mut c.kind {
            *date = None;
        }
        c.placeholder = true;
        return (c, t);
    }
    c.placeholder = false;
    let text = match &mut c.kind {
        ControlKind::CheckBox { checked } => {
            *checked = truthy(value);
            (if *checked { CHECKED } else { UNCHECKED }).to_string()
        }
        ControlKind::List { items, .. } => items.iter().find(|i| i.value == value).map_or_else(|| value.to_string(), |i| i.text.clone()),
        ControlKind::Date { date, format } => {
            let d: String = value.chars().take(10).collect();
            let t = apex_doc::control_date_text(&d, format, &MONTHS, &DAYS);
            *date = Some(d);
            t
        }
        _ => value.to_string(),
    };
    (c, text)
}

/// The bound controls, each with its text.
fn bound(doc: &Document) -> Vec<(apex_doc::ControlAt, DataBinding, String)> {
    doc.controls()
        .into_iter()
        .filter_map(|at| {
            let b = at.control.binding.clone()?;
            let text = doc.paragraphs[at.para].text[at.range.clone()].to_string();
            Some((at, b, text))
        })
        .collect()
}

/// Steps of one sync (a binding read, a control refreshed), each a walk of
/// the document.
const MAX_SYNC_STEPS: usize = 2000;

fn same_binding(a: &DataBinding, b: &DataBinding) -> bool {
    a.xpath == b.xpath && norm_id(&a.store_item) == norm_id(&b.store_item)
}

/// Brings bound controls and their data together. With `store_wins` (a
/// file just read) the controls take the data's values; otherwise a
/// control whose value differs was edited: its value goes to the data and
/// to the other controls bound to the same node. True if anything changed.
fn sync_inner(doc: &mut Document, store_wins: bool, placeholder: &dyn Fn(&ControlKind) -> String) -> bool {
    let all = bound(doc);
    if all.is_empty() {
        return false;
    }
    // Each binding once, with its controls; a document of many bound
    // controls is synced up to a bound, as each step walks the document.
    let mut index: HashMap<(String, String), usize> = HashMap::new();
    let mut groups: Vec<(DataBinding, Vec<usize>)> = Vec::new();
    for (i, (_, b, _)) in all.iter().enumerate() {
        let n = *index.entry((b.xpath.clone(), norm_id(&b.store_item))).or_insert_with(|| {
            groups.push((b.clone(), Vec::new()));
            groups.len() - 1
        });
        groups[n].1.push(i);
    }
    let mut steps = MAX_SYNC_STEPS;
    let mut changed = false;
    let mut stores = Stores::default();
    for (key, members) in groups {
        if steps == 0 {
            break;
        }
        steps -= 1;
        let Some(stored) = store_get(doc, &key, &mut stores) else { continue };
        let group: Vec<_> = members.iter().map(|&i| &all[i]).collect();
        let value = if store_wins {
            stored
        } else {
            match group.iter().map(|(at, _, t)| (value_of(&at.control, t), &at.control)).find(|(v, c)| !same(c, v, &stored)) {
                Some((v, _)) => {
                    store_set(doc, &key, &v, &mut stores);
                    changed = true;
                    v
                }
                None => continue,
            }
        };
        // Controls move as others change length: look them up again.
        for _ in 0..group.len() {
            if steps == 0 {
                break;
            }
            steps -= 1;
            let next = bound(doc).into_iter().find(|(at, b, t)| same_binding(b, &key) && !same(&at.control, &value_of(&at.control, t), &value));
            let Some((at, _, text)) = next else { break };
            let (c, t) = with_value(&at.control, &value, &text, placeholder);
            doc.refresh_control(&at, c, &t);
            changed = true;
        }
    }
    changed
}

/// A file just read: its bound controls show their data.
pub(crate) fn load(doc: &mut Document) {
    let (rev, modified, undo) = (doc.revision, doc.modified, doc.can_undo());
    sync_inner(doc, true, &|_| String::new());
    doc.revision = rev;
    doc.modified = modified;
    debug_assert_eq!(undo, doc.can_undo());
}

/// After an edit: the data takes the value of an edited bound control and
/// the other controls bound to the same node show it (`placeholder` is the
/// text of a control left empty). True if anything changed.
pub fn sync(doc: &mut Document, placeholder: &dyn Fn(&ControlKind) -> String) -> bool {
    sync_inner(doc, false, placeholder)
}

/// Whether the document has bound controls (cheap: they need custom XML or
/// may use the document properties).
pub fn has_bindings(doc: &Document) -> bool {
    doc.paragraphs.iter().any(|p| p.runs.iter().any(|r| r.attrs.control.as_ref().is_some_and(|c| c.binding.is_some())))
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0"?><ns0:datos xmlns:ns0="urn:x" xmlns:b="urn:b"><ns0:cliente>ACME &amp; Co</ns0:cliente><ns0:linea n="1">uno</ns0:linea><ns0:linea n="2"/><b:otro><![CDATA[a<b]]></b:otro></ns0:datos>"#;

    fn bind(xpath: &str) -> DataBinding {
        DataBinding { xpath: xpath.into(), store_item: "{X}".into(), prefixes: "xmlns:a='urn:x' xmlns:z=\"urn:b\"".into() }
    }

    #[test]
    fn xpath() {
        assert_eq!(get(XML, &bind("/a:datos[1]/a:cliente[1]")).as_deref(), Some("ACME & Co"));
        assert_eq!(get(XML, &bind("/a:datos[1]/a:linea[1]/@n")).as_deref(), Some("1"));
        assert_eq!(get(XML, &bind("/a:datos[1]/a:linea[2]")).as_deref(), Some(""));
        assert_eq!(get(XML, &bind("/a:datos[1]/z:otro[1]")).as_deref(), Some("a<b"));
        assert_eq!(get(XML, &bind("/a:datos[1]/a:linea[3]")), None);
        assert_eq!(get(XML, &bind("/a:datos/a:cliente")).as_deref(), Some("ACME & Co"));
        let x = set(XML, &bind("/a:datos[1]/a:linea[2]"), "dos <2>").unwrap();
        assert!(x.contains(r#"<ns0:linea n="2">dos &lt;2&gt;</ns0:linea>"#), "{x}");
        let x = set(&x, &bind("/a:datos[1]/a:linea[1]/@n"), "7").unwrap();
        assert_eq!(get(&x, &bind("/a:datos[1]/a:linea[1]/@n")).as_deref(), Some("7"));
        let x = set(&x, &bind("/a:datos[1]/a:cliente[1]"), "Otra").unwrap();
        assert_eq!(get(&x, &bind("/a:datos[1]/a:cliente[1]")).as_deref(), Some("Otra"));
        assert_eq!(set(&x, &bind("/a:datos[1]"), "no"), None);
    }
}
