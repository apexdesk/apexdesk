//! XML: one row per repeated element, its attributes and child elements
//! as columns (nested ones as `child.grandchild`).

use apex_io::xml::{Ev, Reader};

use crate::sources::json::from_pairs;
use crate::table::{DataTable, Val};
use crate::Error;

struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    kids: Vec<usize>,
    text: String,
    depth: usize,
}

fn tree(xml: &str) -> Result<Vec<Node>, Error> {
    let mut nodes: Vec<Node> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut r = Reader::new(xml);
    loop {
        match r.next() {
            Ev::Eof => break,
            ev @ (Ev::Start(_) | Ev::Empty(_)) => {
                let (t, empty) = match ev {
                    Ev::Start(t) => (t, false),
                    Ev::Empty(t) => (t, true),
                    _ => unreachable!(),
                };
                let attrs = t.attrs().filter(|(k, _)| !k.starts_with("xmlns")).map(|(k, v)| (apex_io::xml::local(k).to_string(), v.into_owned())).collect();
                let id = nodes.len();
                nodes.push(Node { name: t.local().to_string(), attrs, kids: Vec::new(), text: String::new(), depth: stack.len() });
                if let Some(&p) = stack.last() {
                    nodes[p].kids.push(id);
                }
                if !empty {
                    stack.push(id);
                }
            }
            Ev::End(_) => {
                stack.pop();
            }
            Ev::Text(t) => {
                if let Some(&p) = stack.last() {
                    nodes[p].text.push_str(&t);
                }
            }
        }
    }
    if nodes.is_empty() {
        return Err(Error::Parse("XML: no elements".into()));
    }
    Ok(nodes)
}

/// The name of the elements that make the rows: the most repeated one
/// that has attributes or children (or else any repeated one).
fn row_name(nodes: &[Node]) -> Option<String> {
    let mut counts: Vec<(String, usize, usize, bool)> = Vec::new();
    for n in nodes {
        // Repeated under the same parent.
        let mut seen: Vec<(&str, usize)> = Vec::new();
        for &k in &n.kids {
            match seen.iter_mut().find(|(s, _)| *s == nodes[k].name) {
                Some(x) => x.1 += 1,
                None => seen.push((&nodes[k].name, 1)),
            }
        }
        for (name, c) in seen.into_iter().filter(|(_, c)| *c > 1) {
            let rich = n.kids.iter().any(|&k| nodes[k].name == name && (!nodes[k].kids.is_empty() || !nodes[k].attrs.is_empty()));
            let depth = n.depth + 1;
            match counts.iter_mut().find(|x| x.0 == name) {
                Some(x) => x.1 += c,
                None => counts.push((name.to_string(), c, depth, rich)),
            }
        }
    }
    counts.sort_by(|a, b| b.3.cmp(&a.3).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    counts.into_iter().next().map(|x| x.0)
}

fn row(nodes: &[Node], id: usize, prefix: &str, out: &mut Vec<(String, Val)>, depth: usize) {
    let n = &nodes[id];
    let name = |k: &str| if prefix.is_empty() { k.to_string() } else { format!("{prefix}.{k}") };
    for (k, v) in &n.attrs {
        out.push((name(k), Val::Text(v.clone())));
    }
    if n.kids.is_empty() {
        let t = n.text.trim();
        if !t.is_empty() || n.attrs.is_empty() {
            out.push((if prefix.is_empty() { n.name.clone() } else { prefix.to_string() }, if t.is_empty() { Val::Null } else { Val::Text(t.to_string()) }));
        }
        return;
    }
    for &k in &n.kids {
        let kid = &nodes[k];
        let col = name(&kid.name);
        if kid.kids.is_empty() && kid.attrs.is_empty() {
            let t = kid.text.trim().to_string();
            // Repeated plain children are joined.
            if let Some((_, v)) = out.iter_mut().find(|(c, _)| *c == col) {
                let joined = format!("{}, {t}", v.to_text());
                *v = Val::Text(joined);
            } else {
                out.push((col, if t.is_empty() { Val::Null } else { Val::Text(t) }));
            }
        } else if depth < 4 {
            row(nodes, k, &col, out, depth + 1);
        }
    }
}

/// A table of the elements named `element` (empty: found by itself).
pub fn table(xml: &str, element: &str) -> Result<DataTable, Error> {
    let nodes = tree(xml)?;
    let want = if element.trim().is_empty() { row_name(&nodes) } else { Some(apex_io::xml::local(element.trim()).to_string()) };
    let ids: Vec<usize> = match &want {
        Some(w) => (0..nodes.len()).filter(|&i| nodes[i].name.eq_ignore_ascii_case(w)).collect(),
        None => nodes[0].kids.clone(),
    };
    if ids.is_empty() {
        return Err(Error::Parse(format!("XML: no <{}> elements", want.unwrap_or_default())));
    }
    let rows = ids
        .into_iter()
        .map(|i| {
            let mut r = Vec::new();
            row(&nodes, i, "", &mut r, 0);
            r
        })
        .collect();
    let mut t = from_pairs(rows);
    t.detect_types();
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOOKS: &str = r#"<?xml version="1.0"?>
<catalog xmlns="urn:x">
  <info><title>Shop</title></info>
  <book id="b1" lang="en"><title>Rust</title><price>39.5</price><author>Ann</author><author>Bo</author>
    <pub><name>ACME</name><year>2021</year></pub></book>
  <book id="b2"><title>Egui &amp; you</title><price>12</price></book>
</catalog>"#;

    #[test]
    fn rows_of_repeated_elements() {
        for el in ["", "book"] {
            let t = table(BOOKS, el).unwrap();
            assert_eq!(t.columns, ["id", "lang", "title", "price", "author", "pub.name", "pub.year"]);
            assert_eq!(t.rows[0][3], Val::Num(39.5));
            assert_eq!(t.rows[0][4], Val::text("Ann, Bo"));
            assert_eq!(t.rows[0][6], Val::Num(2021.0));
            assert_eq!(t.rows[1][2], Val::text("Egui & you"));
            assert_eq!(t.rows[1][1], Val::Null);
        }
        assert!(table(BOOKS, "movie").is_err());
        let t = table("<r><v>1</v><v>2</v></r>", "").unwrap();
        assert_eq!(t.columns, ["v"]);
        assert_eq!(t.rows, vec![vec![Val::Num(1.0)], vec![Val::Num(2.0)]]);
    }
}
