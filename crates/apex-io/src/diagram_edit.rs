//! Diagram editing: the items of a diagram, their text, adding and
//! removing them, and the diagram's colors and style.
//!
//! A diagram's data part holds its items (points) and how they hang
//! together (connections); its drawing part is the picture Office made
//! from them, which is what is shown. Changing only text patches both
//! parts where they are. Changing how many items there are or their
//! levels rebuilds the connections and, for the layouts drawn here
//! ([`Kind::drawn`]), draws the picture again; other layouts only let
//! their text change. The layout part, the items' own formatting and
//! whatever else isn't understood are kept as they were.

use std::collections::{HashMap, HashSet};
use std::f32::consts::PI;
use std::fmt::Write as _;
use std::hash::{BuildHasher, Hasher};
use std::sync::Arc;

use apex_doc::{Diagram, DiagramPart, DiagramRole};

use crate::diagram::{node_texts, read_drawing};
use crate::drawingml::{Node, TEXT, read_fragment};
use crate::pptx_shape::node_xml;
use crate::xml::esc;

const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const NS_DGM: &str = "http://schemas.openxmlformats.org/drawingml/2006/diagram";
const NS_DSP: &str = "http://schemas.microsoft.com/office/drawing/2008/diagram";
const DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";
/// Items a diagram may have here, and how deep.
pub const MAX_ITEMS: usize = 200;
pub const MAX_LEVEL: usize = 8;
const EMU: f32 = 12700.0;
const LAYOUT_URN: &str = "urn:microsoft.com/office/officeart/2005/8/layout/";
const COLORS_URN: &str = "urn:microsoft.com/office/officeart/2005/8/colors/";
const STYLE_URN: &str = "urn:microsoft.com/office/officeart/2005/8/quickstyle/";

/// Color schemes offered (the ids Office gives them).
pub const COLORS: [&str; 8] = ["accent1_2", "accent2_2", "accent3_2", "accent4_2", "accent5_2", "accent6_2", "colorful1", "accent1_1"];
/// Styles offered.
pub const STYLES: [&str; 4] = ["simple1", "simple2", "simple3", "simple4"];

/// The family of a diagram's layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Blocks,
    List,
    Process,
    Chevron,
    Cycle,
    Radial,
    Hierarchy,
    OrgChart,
    Venn,
    Matrix,
    Pyramid,
    Other,
}

impl Kind {
    /// The family of a layout id (its urn or its last part).
    pub fn of(layout: &str) -> Kind {
        let name = layout.rsplit('/').next().unwrap_or(layout);
        let starts = |p: &str| name.starts_with(p);
        match name {
            "default" => Kind::Blocks,
            "list1" => Kind::List,
            _ if starts("vList") => Kind::List,
            _ if starts("process") || starts("hProcess") => Kind::Process,
            _ if starts("chevron") => Kind::Chevron,
            _ if starts("cycle") => Kind::Cycle,
            _ if starts("radial") => Kind::Radial,
            _ if starts("hierarchy") => Kind::Hierarchy,
            _ if starts("orgChart") => Kind::OrgChart,
            _ if starts("venn") => Kind::Venn,
            _ if starts("matrix") => Kind::Matrix,
            _ if starts("pyramid") => Kind::Pyramid,
            _ => Kind::Other,
        }
    }

    /// Whether its picture is drawn here, so that items can be added and
    /// removed and colors and style changed.
    pub fn drawn(self) -> bool {
        self != Kind::Other
    }

    /// Key of its name in the UI translations.
    pub fn key(self) -> &'static str {
        match self {
            Kind::Blocks => "slides.diagram.k.blocks",
            Kind::List => "slides.diagram.k.list",
            Kind::Process => "slides.diagram.k.process",
            Kind::Chevron => "slides.diagram.k.chevron",
            Kind::Cycle => "slides.diagram.k.cycle",
            Kind::Radial => "slides.diagram.k.radial",
            Kind::Hierarchy => "slides.diagram.k.hierarchy",
            Kind::OrgChart => "slides.diagram.k.org_chart",
            Kind::Venn => "slides.diagram.k.venn",
            Kind::Matrix => "slides.diagram.k.matrix",
            Kind::Pyramid => "slides.diagram.k.pyramid",
            Kind::Other => "slides.diagram.k.other",
        }
    }
}

/// An item: its outline level (0: top) and text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub level: usize,
    pub text: String,
}

impl Item {
    pub fn new(level: usize, text: &str) -> Item {
        Item { level, text: text.to_string() }
    }
}

/// What a diagram is and holds.
#[derive(Clone, Debug)]
pub struct Info {
    pub kind: Kind,
    /// Layout, colors and style ids (their last part, "" if not given).
    pub layout: String,
    pub colors: String,
    pub style: String,
    pub items: Vec<Item>,
    /// Holds points this can't edit (in `mc:AlternateContent`, such as
    /// equations): it is left as it is.
    pub locked: bool,
}

/// Items written one a line, each level indented by a tab or 2 spaces.
pub fn outline_text(items: &[Item]) -> String {
    items.iter().map(|i| format!("{}{}", "\t".repeat(i.level), i.text.replace(['\n', '\r'], " "))).collect::<Vec<_>>().join("\n")
}

/// Items from text written like [`outline_text`] (empty lines skipped,
/// levels made to step one at a time).
pub fn parse_outline(s: &str) -> Vec<Item> {
    let mut out: Vec<Item> = Vec::new();
    for line in s.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let mut depth = 0;
        let mut spaces = 0;
        for c in line.chars() {
            match c {
                '\t' => depth += 1,
                ' ' => {
                    spaces += 1;
                    if spaces == 2 {
                        depth += 1;
                        spaces = 0;
                    }
                }
                _ => break,
            }
        }
        let max = out.last().map_or(0, |p| p.level + 1);
        out.push(Item { level: depth.min(max).min(MAX_LEVEL), text: line.trim().to_string() });
        if out.len() >= MAX_ITEMS {
            break;
        }
    }
    out
}

/// Levels made to step one at a time from 0, and at most [`MAX_ITEMS`].
fn normalized(items: &[Item]) -> Vec<Item> {
    let mut out: Vec<Item> = Vec::new();
    for i in items.iter().take(MAX_ITEMS) {
        let max = out.last().map_or(0, |p| p.level + 1);
        out.push(Item { level: i.level.min(max).min(MAX_LEVEL), text: i.text.replace(['\n', '\r'], " ") });
    }
    out
}

// --- reading -----------------------------------------------------------------------------

fn part(d: &Diagram, role: DiagramRole) -> Option<&DiagramPart> {
    d.parts.iter().find(|p| p.role == role)
}

fn tree_of(p: &DiagramPart) -> Node {
    read_fragment(&String::from_utf8_lossy(&p.xml))
}

fn top<'a>(root: &'a Node, local: &str) -> Option<&'a Node> {
    root.kids.iter().find(|k| k.local() == local)
}

fn top_mut<'a>(root: &'a mut Node, local: &str) -> Option<&'a mut Node> {
    root.kids.iter_mut().find(|k| k.local() == local)
}

fn child_mut<'a>(n: &'a mut Node, local: &str) -> Option<&'a mut Node> {
    n.kids.iter_mut().find(|k| k.local() == local)
}

fn prefix(name: &str) -> &str {
    name.split_once(':').map_or("", |p| p.0)
}

fn q(prefix: &str, local: &str) -> String {
    if prefix.is_empty() { local.to_string() } else { format!("{prefix}:{local}") }
}

fn el(name: String, attrs: &[(&str, &str)]) -> Node {
    Node { name, attrs: attrs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), ..Default::default() }
}

fn text_el(name: String, text: &str) -> Node {
    let mut n = el(name, &[]);
    n.text = text.to_string();
    n.kids.push(Node { name: TEXT.into(), text: text.to_string(), ..Default::default() });
    n
}

fn set_attr(n: &mut Node, key: &str, v: &str) {
    match n.attrs.iter_mut().find(|(k, _)| k == key) {
        Some(a) => a.1 = v.to_string(),
        None => n.attrs.push((key.to_string(), v.to_string())),
    }
}

fn serialize(root: &Node) -> String {
    let mut s = String::from(DECL);
    for k in root.kids.iter().filter(|k| k.name != TEXT) {
        node_xml(&mut s, k);
    }
    s
}

/// A paragraph's text (runs and fields; breaks as spaces).
fn para_text(p: &Node) -> String {
    let mut s = String::new();
    for k in &p.kids {
        match k.local() {
            "r" | "fld" => s.push_str(k.child("t").map_or("", |t| t.text.as_str())),
            "br" => s.push(' '),
            _ => {}
        }
    }
    s
}

fn pt_text(pt: &Node) -> String {
    pt.child("t").map(|t| t.children("p").map(para_text).collect::<Vec<_>>().join(" ")).unwrap_or_default()
}

fn pt_type(pt: &Node) -> &str {
    pt.attr("type").unwrap_or("node")
}

fn is_node(pt: &Node) -> bool {
    matches!(pt_type(pt), "node" | "asst")
}

/// The items of a data model: ids and levels, depth first.
struct Tree {
    doc: Option<String>,
    nodes: Vec<(String, usize)>,
}

fn tree(model: &Node) -> Tree {
    let pts: Vec<&Node> = model.child("ptLst").map(|l| l.children("pt").collect()).unwrap_or_default();
    let doc = pts.iter().find(|p| pt_type(p) == "doc").and_then(|p| p.attr("modelId")).map(str::to_string);
    let nodes: HashSet<&str> = pts.iter().filter(|p| is_node(p)).filter_map(|p| p.attr("modelId")).collect();
    let mut kids: HashMap<&str, Vec<(i64, usize, &str)>> = HashMap::new();
    let mut has_parent = HashSet::new();
    for (i, c) in model.child("cxnLst").into_iter().flat_map(|l| l.children("cxn")).enumerate() {
        if !matches!(c.attr("type"), None | Some("parOf")) {
            continue;
        }
        let (Some(src), Some(dest)) = (c.attr("srcId"), c.attr("destId")) else { continue };
        if nodes.contains(dest) {
            kids.entry(src).or_default().push((c.num("srcOrd").unwrap_or(0.0) as i64, i, dest));
            has_parent.insert(dest);
        }
    }
    for v in kids.values_mut() {
        v.sort();
    }
    let roots: Vec<&str> = match &doc {
        Some(d) => kids.get(d.as_str()).map(|v| v.iter().map(|k| k.2).collect()).unwrap_or_default(),
        None => pts.iter().filter(|p| is_node(p)).filter_map(|p| p.attr("modelId")).filter(|id| !has_parent.contains(id)).collect(),
    };
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut stack: Vec<(&str, usize)> = roots.iter().rev().map(|r| (*r, 0)).collect();
    while let Some((id, level)) = stack.pop() {
        if out.len() >= MAX_ITEMS || !seen.insert(id) {
            continue;
        }
        out.push((id.to_string(), level));
        if level < MAX_LEVEL {
            if let Some(v) = kids.get(id) {
                stack.extend(v.iter().rev().map(|k| (k.2, level + 1)));
            }
        }
    }
    Tree { doc, nodes: out }
}

fn last(id: &str) -> String {
    id.rsplit('/').next().unwrap_or("").to_string()
}

/// What a diagram is and holds; `None` without a data part.
pub fn info(d: &Diagram) -> Option<Info> {
    let root = tree_of(part(d, DiagramRole::Data)?);
    let model = top(&root, "dataModel")?;
    let t = tree(model);
    let pts: HashMap<&str, &Node> = model.child("ptLst").into_iter().flat_map(|l| l.children("pt")).filter_map(|p| Some((p.attr("modelId")?, p))).collect();
    let doc_set = t.doc.as_deref().and_then(|id| pts.get(id)).and_then(|p| p.child("prSet"));
    let layout = part(d, DiagramRole::Layout).and_then(|p| top(&tree_of(p), "layoutDef").and_then(|l| l.attr("uniqueId")).map(str::to_string)).filter(|s| !s.is_empty()).or_else(|| doc_set.and_then(|s| s.attr("loTypeId")).map(str::to_string)).unwrap_or_default();
    let colors = doc_set.and_then(|s| s.attr("csTypeId")).map(last).unwrap_or_default();
    let style = doc_set.and_then(|s| s.attr("qsTypeId")).map(last).unwrap_or_default();
    let items = t.nodes.iter().map(|(id, level)| Item { level: *level, text: pts.get(id.as_str()).map(|p| pt_text(p)).unwrap_or_default() }).collect();
    let locked = model.child("ptLst").is_some_and(|l| l.kids.iter().any(|k| k.name != TEXT && k.local() != "pt"));
    Some(Info { kind: Kind::of(&layout), layout: last(&layout), colors, style, items, locked })
}

// --- ids ---------------------------------------------------------------------------------

/// New model ids, unlike any in `used`.
struct Ids {
    state: u64,
    used: HashSet<String>,
}

impl Ids {
    fn new(model: &Node) -> Ids {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_usize(model.kids.len());
        let mut used = HashSet::new();
        let mut all = Vec::new();
        model.find_all("pt", &mut all);
        model.find_all("cxn", &mut all);
        for n in all {
            if let Some(id) = n.attr("modelId") {
                used.insert(id.to_ascii_uppercase());
            }
        }
        Ids { state: h.finish() | 1, used }
    }

    fn word(&mut self) -> u64 {
        // splitmix64
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn next(&mut self) -> String {
        loop {
            let (a, b) = (self.word(), self.word());
            let id = format!("{{{:08X}-{:04X}-{:04X}-{:04X}-{:012X}}}", a >> 32, (a >> 16) & 0xFFFF, a & 0xFFFF, b >> 48, b & 0xFFFF_FFFF_FFFF);
            if self.used.insert(id.clone()) {
                return id;
            }
        }
    }
}

// --- text --------------------------------------------------------------------------------

/// `template` with its text replaced by `text` in one run that keeps the
/// first run's (or the paragraph end's) formatting.
fn para_with(template: &Node, text: &str) -> Node {
    let a = prefix(&template.name).to_string();
    let rpr = template.children("r").find_map(|r| r.child("rPr")).cloned().or_else(|| {
        template.child("endParaRPr").map(|e| {
            let mut e = e.clone();
            e.name = q(&a, "rPr");
            e
        })
    });
    let mut p = Node { name: template.name.clone(), attrs: template.attrs.clone(), ..Default::default() };
    if let Some(ppr) = template.child("pPr") {
        p.kids.push(ppr.clone());
    }
    if !text.is_empty() {
        let mut r = el(q(&a, "r"), &[]);
        if let Some(rpr) = rpr {
            r.kids.push(rpr);
        }
        r.kids.push(text_el(q(&a, "t"), text));
        p.kids.push(r);
    }
    if let Some(end) = template.child("endParaRPr") {
        p.kids.push(end.clone());
    }
    p
}

/// Sets a point's text (its `dgm:t`), keeping how it looked.
fn set_pt_text(pt: &mut Node, text: &str, a: &str) {
    let dgm = prefix(&pt.name).to_string();
    if pt.child("t").is_none() {
        let mut t = el(q(&dgm, "t"), &[]);
        t.kids.push(el(q(a, "bodyPr"), &[]));
        t.kids.push(el(q(a, "lstStyle"), &[]));
        t.kids.push(el(q(a, "p"), &[]));
        pt.kids.push(t);
    }
    if let Some(set) = child_mut(pt, "prSet") {
        if !text.is_empty() {
            set.attrs.retain(|(k, _)| crate::xml::local(k) != "phldr");
        }
    }
    let Some(t) = child_mut(pt, "t") else { return };
    let template = t.children("p").next().cloned().unwrap_or_else(|| el(q(a, "p"), &[]));
    let at = t.kids.iter().position(|k| k.local() == "p").unwrap_or(t.kids.len());
    t.kids.retain(|k| k.local() != "p");
    t.kids.insert(at.min(t.kids.len()), para_with(&template, text));
}

fn walk_mut(n: &mut Node, f: &mut dyn FnMut(&mut Node)) {
    f(n);
    for k in &mut n.kids {
        walk_mut(k, f);
    }
}

/// The drawing with the text of the shapes showing changed items patched
/// (`texts`: every item's text by id; `changed`: ids whose text changed).
fn patch_drawing(xml: &str, model: &Node, order: &HashMap<String, usize>, texts: &HashMap<String, String>, changed: &HashSet<String>) -> String {
    let mut shows: HashMap<&str, Vec<&str>> = HashMap::new();
    for c in model.child("cxnLst").into_iter().flat_map(|l| l.children("cxn")) {
        if c.attr("type") == Some("presOf") {
            if let (Some(s), Some(d)) = (c.attr("srcId"), c.attr("destId")) {
                if order.contains_key(s) {
                    let v = shows.entry(d).or_default();
                    if !v.contains(&s) {
                        v.push(s);
                    }
                }
            }
        }
    }
    for v in shows.values_mut() {
        v.sort_by_key(|s| order.get(*s).copied().unwrap_or(usize::MAX));
    }
    let mut root = read_fragment(xml);
    walk_mut(&mut root, &mut |sp: &mut Node| {
        if sp.local() != "sp" {
            return;
        }
        let Some(srcs) = sp.attr("modelId").and_then(|m| shows.get(m)) else { return };
        if !srcs.iter().any(|s| changed.contains(*s)) {
            return;
        }
        let srcs = srcs.clone();
        let Some(body) = child_mut(sp, "txBody") else { return };
        let paras: Vec<Node> = body.children("p").cloned().collect();
        if paras.is_empty() {
            return;
        }
        let at = body.kids.iter().position(|k| k.local() == "p").unwrap_or(body.kids.len());
        let new: Vec<Node> = if paras.len() == srcs.len() {
            paras.iter().zip(&srcs).map(|(p, s)| if changed.contains(*s) { para_with(p, &texts[*s]) } else { p.clone() }).collect()
        } else {
            srcs.iter().enumerate().map(|(k, s)| para_with(&paras[k.min(paras.len() - 1)], &texts[*s])).collect()
        };
        body.kids.retain(|k| k.local() != "p");
        let at = at.min(body.kids.len());
        body.kids.splice(at..at, new);
    });
    serialize(&root)
}

// --- editing -----------------------------------------------------------------------------

fn with_part(d: &Diagram, role: DiagramRole, xml: String) -> Vec<DiagramPart> {
    let mut parts = d.parts.clone();
    match parts.iter_mut().find(|p| p.role == role) {
        Some(p) => p.xml = Arc::from(xml.into_bytes()),
        None => parts.push(DiagramPart { role, xml: Arc::from(xml.into_bytes()), rels: Vec::new() }),
    }
    parts
}

fn a_prefix(model: &Node) -> String {
    model.attrs.iter().find(|(k, v)| v == NS_A && k.starts_with("xmlns:")).map_or_else(|| "a".to_string(), |(k, _)| k["xmlns:".len()..].to_string())
}

fn ensure_a(model: &mut Node) -> String {
    if !model.attrs.iter().any(|(k, v)| v == NS_A && k.starts_with("xmlns:")) {
        model.attrs.push(("xmlns:a".into(), NS_A.into()));
    }
    a_prefix(model)
}

/// The diagram with its items made `items`; `None` when the number of
/// items or their levels change in a layout not drawn here (only text
/// can change there) or it has no data.
pub fn set_items(d: &Diagram, items: &[Item]) -> Option<Diagram> {
    let items = normalized(items);
    let info = info(d).filter(|i| !i.locked)?;
    let mut root = tree_of(part(d, DiagramRole::Data)?);
    let model = top_mut(&mut root, "dataModel")?;
    let a = ensure_a(model);
    let t = tree(model);
    let same = items.len() == t.nodes.len() && items.iter().zip(&t.nodes).all(|(i, n)| i.level == n.1);
    if same {
        let mut changed = HashSet::new();
        let mut texts = HashMap::new();
        let mut order = HashMap::new();
        for (k, (item, (id, _))) in items.iter().zip(&t.nodes).enumerate() {
            if item.text != info.items[k].text {
                changed.insert(id.clone());
            }
            texts.insert(id.clone(), item.text.clone());
            order.insert(id.clone(), k);
        }
        if changed.is_empty() {
            return Some(d.clone());
        }
        if let Some(list) = child_mut(model, "ptLst") {
            for pt in list.kids.iter_mut().filter(|p| p.local() == "pt") {
                if let Some(id) = pt.attr("modelId").filter(|id| changed.contains(*id)).map(str::to_string) {
                    set_pt_text(pt, &texts[&id], &a);
                }
            }
        }
        let mut out = d.clone();
        let model = top(&root, "dataModel")?;
        let has_pres = model.child("cxnLst").is_some_and(|l| l.children("cxn").any(|c| c.attr("type") == Some("presOf")));
        if let Some(dr) = part(d, DiagramRole::Drawing).filter(|_| has_pres || !info.kind.drawn()) {
            let xml = patch_drawing(&String::from_utf8_lossy(&dr.xml), model, &order, &texts, &changed);
            out.shapes = read_drawing(&xml);
            out.parts = with_part(&out, DiagramRole::Drawing, xml);
        }
        out.parts = with_part(&out, DiagramRole::Data, serialize(&root));
        out.texts = node_texts(&root);
        if !has_pres && info.kind.drawn() {
            return Some(redraw(out, &info.colors, &info.style, &items, info.kind));
        }
        return Some(out);
    }
    if !info.kind.drawn() {
        return None;
    }
    rebuild(model, &t, &items, &a)?;
    let mut out = d.clone();
    out.parts = with_part(d, DiagramRole::Data, serialize(&root));
    out.texts = node_texts(&root);
    Some(redraw(out, &info.colors, &info.style, &items, info.kind))
}

/// Remakes the points and connections of a data model for `items`,
/// reusing the old items' points in order (they keep their formatting).
fn rebuild(model: &mut Node, t: &Tree, items: &[Item], a: &str) -> Option<()> {
    let doc = t.doc.clone()?;
    let mut ids = Ids::new(model);
    let dgm = prefix(&model.name).to_string();
    let list = child_mut(model, "ptLst")?;
    let mut old: HashMap<String, Node> = HashMap::new();
    let mut doc_pt = None;
    for pt in list.kids.drain(..) {
        if pt.local() != "pt" {
            continue;
        }
        match pt.attr("modelId") {
            Some(id) if id == doc => doc_pt = Some(pt),
            Some(id) if is_node(&pt) => {
                old.insert(id.to_string(), pt);
            }
            _ => {}
        }
    }
    list.kids.push(doc_pt?);
    let mut node_ids = Vec::new();
    for (k, item) in items.iter().enumerate() {
        let reused = t.nodes.get(k).and_then(|(id, _)| old.remove(id));
        let mut pt = reused.unwrap_or_else(|| {
            let id = ids.next();
            let mut pt = el(q(&dgm, "pt"), &[("modelId", &id)]);
            pt.kids.push(el(q(&dgm, "prSet"), &[("phldrT", "[Text]")]));
            pt.kids.push(el(q(&dgm, "spPr"), &[]));
            pt
        });
        set_pt_text(&mut pt, &item.text, a);
        if item.text.is_empty() {
            if let Some(set) = child_mut(&mut pt, "prSet") {
                set_attr(set, "phldr", "1");
            }
        }
        node_ids.push(pt.attr("modelId").unwrap_or_default().to_string());
        list.kids.push(pt);
    }
    // Connections: each item to its parent, with its transition points.
    let mut cxns = Vec::new();
    let mut ords: HashMap<String, usize> = HashMap::new();
    let mut parents: Vec<String> = Vec::new();
    for (k, item) in items.iter().enumerate() {
        parents.truncate(item.level);
        let parent = parents.last().cloned().unwrap_or_else(|| doc.clone());
        let ord = ords.entry(parent.clone()).or_insert(0);
        let (cid, par, sib) = (ids.next(), ids.next(), ids.next());
        for (tid, ty) in [(&par, "parTrans"), (&sib, "sibTrans")] {
            let mut pt = el(q(&dgm, "pt"), &[("modelId", tid), ("type", ty), ("cxnId", &cid)]);
            pt.kids.push(el(q(&dgm, "prSet"), &[]));
            pt.kids.push(el(q(&dgm, "spPr"), &[]));
            set_pt_text(&mut pt, "", a);
            list.kids.push(pt);
        }
        let so = ord.to_string();
        cxns.push(el(q(&dgm, "cxn"), &[("modelId", &cid), ("srcId", &parent), ("destId", &node_ids[k]), ("srcOrd", &so), ("destOrd", "0"), ("parTransId", &par), ("sibTransId", &sib)]));
        *ord += 1;
        parents.push(node_ids[k].clone());
    }
    match child_mut(model, "cxnLst") {
        Some(l) => l.kids = cxns,
        None => {
            let mut l = el(q(&dgm, "cxnLst"), &[]);
            l.kids = cxns;
            let at = model.kids.iter().position(|k| k.local() == "ptLst").map_or(model.kids.len(), |i| i + 1);
            model.kids.insert(at, l);
        }
    }
    // The drawing is related from the data model; one is made if missing.
    if model.find("dataModelExt").is_none() {
        let ext = read_fragment(&format!(r#"<{0}:extLst xmlns:{0}="{NS_DGM}"><a:ext xmlns:a="{NS_A}" uri="http://schemas.microsoft.com/office/drawing/2008/diagram"><dsp:dataModelExt xmlns:dsp="{NS_DSP}" relId="rIdDrawing" minVer="http://schemas.openxmlformats.org/drawingml/2006/diagram"/></a:ext></{0}:extLst>"#, if dgm.is_empty() { "dgm" } else { &dgm }));
        if let Some(e) = ext.kids.into_iter().find(|k| k.name != TEXT) {
            match child_mut(model, "extLst") {
                Some(l) => l.kids.extend(e.kids),
                None => model.kids.push(e),
            }
        }
    }
    Some(())
}

/// A doc point's `prSet` changed by `f`.
fn with_doc_set(d: &Diagram, f: impl Fn(&mut Node)) -> Option<Node> {
    let mut root = tree_of(part(d, DiagramRole::Data)?);
    let model = top_mut(&mut root, "dataModel")?;
    let dgm = prefix(&model.name).to_string();
    let list = child_mut(model, "ptLst")?;
    let doc = list.kids.iter_mut().find(|p| p.local() == "pt" && p.attr("type") == Some("doc"))?;
    if doc.child("prSet").is_none() {
        doc.kids.insert(0, el(q(&dgm, "prSet"), &[]));
    }
    f(child_mut(doc, "prSet")?);
    Some(root)
}

/// The diagram with color scheme `id` (one of [`COLORS`]); `None` for a
/// layout not drawn here.
pub fn set_colors(d: &Diagram, id: &str) -> Option<Diagram> {
    let info = info(d).filter(|i| i.kind.drawn() && !i.locked)?;
    let cat = if id.starts_with("colorful") { "colorful" } else { "accent1" };
    let root = with_doc_set(d, |s| {
        set_attr(s, "csTypeId", &format!("{COLORS_URN}{id}"));
        set_attr(s, "csCatId", cat);
    })?;
    let mut out = d.clone();
    out.parts = with_part(d, DiagramRole::Data, serialize(&root));
    out.parts = with_part(&out, DiagramRole::Colors, colors_def(id));
    Some(redraw(out, id, &info.style, &info.items, info.kind))
}

/// The diagram with style `id` (one of [`STYLES`]); `None` for a layout
/// not drawn here.
pub fn set_style(d: &Diagram, id: &str) -> Option<Diagram> {
    let info = info(d).filter(|i| i.kind.drawn() && !i.locked)?;
    let root = with_doc_set(d, |s| {
        set_attr(s, "qsTypeId", &format!("{STYLE_URN}{id}"));
        set_attr(s, "qsCatId", "simple");
    })?;
    let mut out = d.clone();
    out.parts = with_part(d, DiagramRole::Data, serialize(&root));
    out.parts = with_part(&out, DiagramRole::Style, style_def(id));
    Some(redraw(out, &info.colors, id, &info.items, info.kind))
}

fn redraw(mut d: Diagram, colors: &str, style: &str, items: &[Item], kind: Kind) -> Diagram {
    let size = if d.size.0 > 1.0 && d.size.1 > 1.0 { d.size } else { (400.0, 300.0) };
    let xml = draw(kind, items, size, colors, style);
    d.shapes = read_drawing(&xml);
    d.parts = with_part(&d, DiagramRole::Drawing, xml);
    d
}

// --- colors and style parts --------------------------------------------------------------

const NODE_LABELS: [&str; 18] = ["node0", "alignNode1", "node1", "lnNode1", "vennNode1", "node2", "node3", "node4", "asst0", "asst1", "asst2", "asst3", "asst4", "sibTrans2D1", "fgSibTrans2D1", "bgSibTrans2D1", "parChTrans2D1", "parChTrans2D2"];
const ACC_LABELS: [&str; 14] = ["fgAcc1", "conFgAcc1", "alignAcc1", "trAlignAcc1", "bgAcc1", "solidFgAcc1", "solidAlignAcc1", "solidBgAcc1", "fgAccFollowNode1", "alignAccFollowNode1", "bgAccFollowNode1", "fgAcc0", "fgAcc2", "callout"];
const LINE_LABELS: [&str; 5] = ["sibTrans1D1", "parChTrans1D1", "parChTrans1D2", "parChTrans1D3", "parChTrans1D4"];
const BG_LABELS: [&str; 7] = ["fgImgPlace1", "alignImgPlace1", "bgImgPlace1", "bgShp", "dkBgShp", "trBgShp", "fgShp"];

/// Accent used for item `i` and whether shapes are outlined.
fn scheme_of(colors: &str, i: usize) -> (usize, bool) {
    if let Some(n) = colors.strip_prefix("colorful") {
        let start = n.parse::<usize>().unwrap_or(1);
        return (2 + (i + start - 1) % 5, false);
    }
    let mut it = colors.strip_prefix("accent").unwrap_or("1_2").split('_');
    let accent = it.next().and_then(|n| n.parse::<usize>().ok()).filter(|n| (1..=6).contains(n)).unwrap_or(1);
    (accent, it.next() == Some("1"))
}

fn colors_def(id: &str) -> String {
    let (accent, outline) = scheme_of(id, 0);
    let clr = |v: &str, m: &str| format!(r#"<a:schemeClr val="{v}">{m}</a:schemeClr>"#);
    let fills = if id.starts_with("colorful") { (0..5).map(|i| clr(&format!("accent{}", scheme_of(id, i).0), "")).collect::<String>() } else { clr(&format!("accent{accent}"), "") };
    let acc = format!("accent{accent}");
    let mut s = format!(r#"{DECL}<dgm:colorsDef xmlns:dgm="{NS_DGM}" xmlns:a="{NS_A}" uniqueId="{COLORS_URN}{id}"><dgm:title val=""/><dgm:desc val=""/><dgm:catLst><dgm:cat type="{}" pri="10100"/></dgm:catLst>"#, if id.starts_with("colorful") { "colorful" } else { "accent1" });
    let mut lbl = |name: &str, fill: &str, line: &str, text: &str| {
        let _ = write!(s, r#"<dgm:styleLbl name="{name}"><dgm:fillClrLst meth="repeat">{fill}</dgm:fillClrLst><dgm:linClrLst meth="repeat">{line}</dgm:linClrLst><dgm:effectClrLst/><dgm:txLinClrLst/><dgm:txFillClrLst meth="repeat">{text}</dgm:txFillClrLst><dgm:txEffectClrLst/></dgm:styleLbl>"#);
    };
    for name in NODE_LABELS {
        if outline {
            lbl(name, &clr("lt1", ""), &clr(&acc, ""), &clr("dk1", ""));
        } else {
            lbl(name, &fills, &clr("lt1", ""), &clr("lt1", ""));
        }
    }
    for name in ACC_LABELS {
        lbl(name, &clr("lt1", r#"<a:alpha val="90000"/>"#), &clr(&acc, ""), &clr("dk1", ""));
    }
    for name in LINE_LABELS {
        lbl(name, &clr(&acc, ""), &clr(&acc, r#"<a:shade val="60000"/>"#), &clr("tx1", ""));
    }
    for name in BG_LABELS {
        lbl(name, &clr(&acc, r#"<a:tint val="40000"/>"#), &clr(&acc, ""), &clr("dk1", ""));
    }
    lbl("revTx", &clr("lt1", r#"<a:alpha val="0"/>"#), &clr("dk1", r#"<a:alpha val="0"/>"#), &clr("tx1", ""));
    s.push_str("</dgm:colorsDef>");
    s
}

/// Line width (EMU, 0: none) and shadow of a style.
fn style_look(style: &str) -> (u32, bool) {
    match style {
        "simple2" => (0, false),
        "simple3" => (25400, true),
        "simple4" => (38100, true),
        _ => (12700, false),
    }
}

fn style_def(id: &str) -> String {
    let effect = match id {
        "simple3" => 2,
        "simple4" => 3,
        "simple2" => 1,
        _ => 0,
    };
    let line = if id == "simple2" { 0 } else { 2 };
    let rgb = r#"<a:scrgbClr r="0" g="0" b="0"/>"#;
    let mut s = format!(r#"{DECL}<dgm:styleDef xmlns:dgm="{NS_DGM}" xmlns:a="{NS_A}" uniqueId="{STYLE_URN}{id}"><dgm:title val=""/><dgm:desc val=""/><dgm:catLst><dgm:cat type="simple" pri="10100"/></dgm:catLst><dgm:scene3d><a:camera prst="orthographicFront"/><a:lightRig rig="threePt" dir="t"/></dgm:scene3d><dgm:styleLbl name="node0"><dgm:scene3d><a:camera prst="orthographicFront"/><a:lightRig rig="threePt" dir="t"/></dgm:scene3d><dgm:sp3d/><dgm:txPr/><dgm:style><a:lnRef idx="{line}">{rgb}</a:lnRef><a:fillRef idx="1">{rgb}</a:fillRef><a:effectRef idx="{effect}">{rgb}</a:effectRef><a:fontRef idx="minor"><a:schemeClr val="lt1"/></a:fontRef></dgm:style></dgm:styleLbl>"#);
    for name in NODE_LABELS.iter().skip(1).chain(&ACC_LABELS).chain(&LINE_LABELS).chain(&BG_LABELS).chain(&["revTx"]) {
        let (ln, fill, fx, font) = if LINE_LABELS.contains(name) { (1, 0, 0, "tx1") } else if *name == "revTx" { (0, 0, 0, "tx1") } else if NODE_LABELS.contains(name) { (line, 1, effect, "lt1") } else { (line, 1, 0, "tx1") };
        let _ = write!(s, r#"<dgm:styleLbl name="{name}"><dgm:scene3d><a:camera prst="orthographicFront"/><a:lightRig rig="threePt" dir="t"/></dgm:scene3d><dgm:sp3d/><dgm:txPr/><dgm:style><a:lnRef idx="{ln}">{rgb}</a:lnRef><a:fillRef idx="{fill}">{rgb}</a:fillRef><a:effectRef idx="{fx}">{rgb}</a:effectRef><a:fontRef idx="minor"><a:schemeClr val="{font}"/></a:fontRef></dgm:style></dgm:styleLbl>"#);
    }
    s.push_str("</dgm:styleDef>");
    s
}

// --- drawing -----------------------------------------------------------------------------

/// A shape of the drawing, in points.
struct Sp {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    geom: &'static str,
    adj: Option<i64>,
    rot: f32,
    flip_h: bool,
    flip_v: bool,
    fill: String,
    line: String,
    shadow: bool,
    paras: Vec<(usize, String)>,
    size: f32,
    color: &'static str,
    left: bool,
}

/// How shapes look for a color scheme and style.
struct Look<'a> {
    colors: &'a str,
    line_w: u32,
    shadow: bool,
}

impl Look<'_> {
    fn accent(&self, i: usize) -> String {
        format!("accent{}", scheme_of(self.colors, i).0)
    }

    fn outlined(&self) -> bool {
        scheme_of(self.colors, 0).1
    }

    /// A filled item shape: item `i`, `tint` (0: full color).
    fn node(&self, i: usize, tint: u32, alpha: bool, geom: &'static str, r: (f32, f32, f32, f32)) -> Sp {
        let acc = self.accent(i);
        let mut m = String::new();
        if tint > 0 {
            let _ = write!(m, r#"<a:tint val="{tint}"/>"#);
        }
        if alpha {
            m.push_str(r#"<a:alpha val="50000"/>"#);
        }
        let (fill, line, color) = if self.outlined() {
            (format!(r#"<a:solidFill><a:schemeClr val="lt1">{}</a:schemeClr></a:solidFill>"#, if alpha { r#"<a:alpha val="50000"/>"# } else { "" }), ln(self.line_w.max(12700), &format!(r#"<a:schemeClr val="{acc}"/>"#)), "tx1")
        } else {
            let color = if tint > 0 || alpha { "tx1" } else { "lt1" };
            (format!(r#"<a:solidFill><a:schemeClr val="{acc}">{m}</a:schemeClr></a:solidFill>"#), ln(self.line_w, r#"<a:schemeClr val="lt1"/>"#), color)
        };
        Sp { x: r.0, y: r.1, w: r.2, h: r.3, geom, adj: None, rot: 0.0, flip_h: false, flip_v: false, fill, line, shadow: self.shadow, paras: Vec::new(), size: 12.0, color, left: false }
    }

    /// A shape between items (arrows, connectors).
    fn link(&self, i: usize, geom: &'static str, r: (f32, f32, f32, f32)) -> Sp {
        let acc = self.accent(i);
        let line_only = geom == "line";
        let fill = if line_only { "<a:noFill/>".to_string() } else { format!(r#"<a:solidFill><a:schemeClr val="{acc}"><a:tint val="60000"/></a:schemeClr></a:solidFill>"#) };
        let line = if line_only { ln(12700, &format!(r#"<a:schemeClr val="{acc}"><a:shade val="60000"/></a:schemeClr>"#)) } else { ln(0, "") };
        Sp { x: r.0, y: r.1, w: r.2, h: r.3, geom, adj: None, rot: 0.0, flip_h: false, flip_v: false, fill, line, shadow: false, paras: Vec::new(), size: 12.0, color: "tx1", left: false }
    }

    /// A text box with no fill.
    fn text(&self, r: (f32, f32, f32, f32)) -> Sp {
        Sp { x: r.0, y: r.1, w: r.2, h: r.3, geom: "rect", adj: None, rot: 0.0, flip_h: false, flip_v: false, fill: "<a:noFill/>".into(), line: ln(0, ""), shadow: false, paras: Vec::new(), size: 12.0, color: "tx1", left: true }
    }
}

fn ln(w: u32, clr: &str) -> String {
    if w == 0 { "<a:ln><a:noFill/></a:ln>".into() } else { format!(r#"<a:ln w="{w}" cap="flat" cmpd="sng" algn="ctr"><a:solidFill>{clr}</a:solidFill><a:prstDash val="solid"/><a:miter lim="800000"/></a:ln>"#) }
}

/// A line from (x1, y1) to (x2, y2).
fn line_sp(look: &Look, i: usize, x1: f32, y1: f32, x2: f32, y2: f32) -> Sp {
    let mut s = look.link(i, "line", (x1.min(x2), y1.min(y2), (x2 - x1).abs(), (y2 - y1).abs()));
    s.flip_h = x2 < x1;
    s.flip_v = y2 < y1;
    s
}

/// A font size (points) letting `paras` fit a `w` by `h` box.
fn fit(w: f32, h: f32, paras: &[(usize, String)]) -> f32 {
    let lines = paras.len().max(1) as f32;
    let chars: usize = paras.iter().map(|p| p.1.chars().count().max(1)).sum();
    let word = paras.iter().flat_map(|p| p.1.split_whitespace()).map(|w| w.chars().count()).max().unwrap_or(1).max(1) as f32;
    let by_h = h * 0.8 / (lines * 1.2);
    let by_area = (w * h * 0.5 / (chars.max(1) as f32 * 0.55 * 1.2)).sqrt();
    let by_word = w * 0.9 / (word * 0.6);
    by_h.min(by_area).min(by_word).clamp(5.0, 48.0)
}

/// Gives shapes in `group` the same font size, the smallest that fits.
fn same_size(sps: &mut [Sp], group: &[usize]) {
    let size = group.iter().map(|&i| fit(sps[i].w - 8.0, sps[i].h - 8.0, &sps[i].paras)).fold(48.0f32, f32::min);
    for &i in group {
        sps[i].size = (size * 2.0).floor() / 2.0;
    }
}

/// Top items and what's under each (levels from 0 under it).
fn groups(items: &[Item]) -> Vec<(String, Vec<(usize, String)>)> {
    let mut out: Vec<(String, Vec<(usize, String)>)> = Vec::new();
    for i in items {
        match (i.level, out.last_mut()) {
            (0, _) | (_, None) => out.push((i.text.clone(), Vec::new())),
            (l, Some(g)) => g.1.push((l - 1, i.text.clone())),
        }
    }
    out
}

fn with_kids(text: &str, kids: &[(usize, String)]) -> Vec<(usize, String)> {
    let mut v = vec![(0, text.to_string())];
    v.extend(kids.iter().map(|(l, t)| (l + 1, t.clone())));
    v
}

/// The `dsp:drawing` of `items` laid out as `kind` in a `size` box.
fn draw(kind: Kind, items: &[Item], size: (f32, f32), colors: &str, style: &str) -> String {
    let (line_w, shadow) = style_look(style);
    let look = Look { colors, line_w, shadow };
    let (w, h) = size;
    let gs = groups(items);
    let n = gs.len();
    let mut sps: Vec<Sp> = Vec::new();
    let mut main = Vec::new();
    let mut minor = Vec::new();
    match kind {
        Kind::Blocks | Kind::Other => {
            let n1 = n.max(1);
            let mut best = (0.0, 1, 0.0, 0.0);
            for cols in 1..=n1 {
                let rows = n1.div_ceil(cols);
                let cw = w / (cols as f32 + (cols - 1) as f32 * 0.1);
                let ch = h / (rows as f32 + (rows - 1) as f32 * 0.1);
                let bw = cw.min(ch * 1.6);
                let bh = bw / 1.6;
                if bw * bh > best.0 {
                    best = (bw * bh, cols, bw, bh);
                }
            }
            let (_, cols, bw, bh) = best;
            let rows = n1.div_ceil(cols);
            let y0 = (h - (rows as f32 * bh + (rows - 1) as f32 * bh * 0.1)) / 2.0;
            for (i, (t, kids)) in gs.iter().enumerate() {
                let (r, c) = (i / cols, i % cols);
                let in_row = if r == rows - 1 { n - r * cols } else { cols };
                let x0 = (w - (in_row as f32 * bw + (in_row - 1) as f32 * bw * 0.1)) / 2.0;
                let mut s = look.node(i, 0, false, "rect", (x0 + c as f32 * bw * 1.1, y0 + r as f32 * bh * 1.1, bw, bh));
                s.paras = with_kids(t, kids);
                main.push(sps.len());
                sps.push(s);
            }
        }
        Kind::List => {
            let weight = |k: &Vec<(usize, String)>| 1.0 + 0.55 * k.len() as f32;
            let total: f32 = gs.iter().map(|g| weight(&g.1)).sum::<f32>() + 0.15 * n.saturating_sub(1) as f32;
            let unit = h / total.max(1.0);
            let mut y = 0.0;
            for (i, (t, kids)) in gs.iter().enumerate() {
                let mut s = look.node(i, 0, false, "roundRect", (0.0, y, w, unit));
                s.paras = vec![(0, t.clone())];
                s.left = true;
                main.push(sps.len());
                sps.push(s);
                y += unit;
                if !kids.is_empty() {
                    let kh = unit * 0.55 * kids.len() as f32;
                    let mut s = look.text((w * 0.05, y, w * 0.95, kh));
                    s.paras = kids.iter().map(|(l, t)| (l + 1, t.clone())).collect();
                    minor.push(sps.len());
                    sps.push(s);
                    y += kh;
                }
                y += unit * 0.15;
            }
        }
        Kind::Process | Kind::Chevron => {
            let chevron = kind == Kind::Chevron;
            let n1 = n.max(1) as f32;
            let bw = if chevron { w / (0.9 * n1 + 0.1) } else { w / (n1 + (n1 - 1.0) * 0.45) };
            let most = gs.iter().map(|g| g.1.len()).max().unwrap_or(0) as f32;
            let bh = if chevron { h.min(bw * 0.4) } else { h.min(bw * (0.6 + 0.25 * most)) };
            let kid_h = if chevron && most > 0.0 { (h - bh).min(bh * most * 0.6) } else { 0.0 };
            let y = (h - bh - kid_h) / 2.0;
            for (i, (t, kids)) in gs.iter().enumerate() {
                let x = if chevron { i as f32 * bw * 0.9 } else { i as f32 * bw * 1.45 };
                let geom = if !chevron { "roundRect" } else if i == 0 { "homePlate" } else { "chevron" };
                let mut s = look.node(i, 0, false, geom, (x, y, bw, bh));
                if chevron {
                    s.paras = vec![(0, t.clone())];
                    if !kids.is_empty() {
                        let mut k = look.text((x + bw * 0.1, y + bh, bw * 0.8, kid_h));
                        k.paras = kids.iter().map(|(l, t)| (l + 1, t.clone())).collect();
                        minor.push(sps.len() + 1);
                        main.push(sps.len());
                        sps.push(s);
                        sps.push(k);
                        continue;
                    }
                } else {
                    s.paras = with_kids(t, kids);
                }
                main.push(sps.len());
                sps.push(s);
                if !chevron && i + 1 < n {
                    let aw = bw * 0.25;
                    let ah = (bw * 0.3).min(bh);
                    sps.push(look.link(i, "rightArrow", (x + bw + (bw * 0.45 - aw) / 2.0, y + (bh - ah) / 2.0, aw, ah)));
                }
            }
        }
        Kind::Cycle | Kind::Venn => {
            let r_all = w.min(h) / 2.0;
            let (cx, cy) = (w / 2.0, h / 2.0);
            let (r, ring) = if n <= 1 {
                (r_all * 0.9, 0.0)
            } else if kind == Kind::Venn {
                let r = r_all / 1.65;
                (r, r * 0.65)
            } else {
                let s = (PI / n as f32).sin();
                let r = (r_all * s / (1.0 + s) * 0.85).min(r_all * 0.35);
                (r, r_all - r)
            };
            let angle = |i: usize| if kind == Kind::Venn && n == 2 { PI * (1.0 - i as f32) } else { -PI / 2.0 + 2.0 * PI * i as f32 / n.max(1) as f32 };
            for (i, (t, kids)) in gs.iter().enumerate() {
                let a = angle(i);
                let (x, y) = (cx + ring * a.cos() - r, cy + ring * a.sin() - r);
                let mut s = look.node(i, 0, kind == Kind::Venn, "ellipse", (x, y, 2.0 * r, 2.0 * r));
                s.paras = with_kids(t, kids);
                main.push(sps.len());
                sps.push(s);
                if kind == Kind::Cycle && n > 1 {
                    let len = 2.0 * ring * (PI / n as f32).sin() - 2.0 * r;
                    if len > r * 0.3 {
                        let m = a + PI / n as f32;
                        let aw = (len * 0.6).min(r * 0.8);
                        let ah = aw * 0.6;
                        let (ax, ay) = (cx + ring * m.cos(), cy + ring * m.sin());
                        let mut s = look.link(i, "rightArrow", (ax - aw / 2.0, ay - ah / 2.0, aw, ah));
                        s.rot = (m + PI / 2.0).to_degrees();
                        sps.push(s);
                    }
                }
            }
        }
        Kind::Radial => {
            let (center, outer): (Item, Vec<Item>) = match gs.first() {
                Some((t, kids)) if !kids.is_empty() => (Item::new(0, t), kids.iter().filter(|k| k.0 == 0).map(|k| Item::new(0, &k.1)).collect()),
                Some((t, _)) => (Item::new(0, t), gs.iter().skip(1).map(|g| Item::new(0, &g.0)).collect()),
                None => (Item::new(0, ""), Vec::new()),
            };
            let m = outer.len();
            let r_all = w.min(h) / 2.0;
            let (cx, cy) = (w / 2.0, h / 2.0);
            let r = if m == 0 { 0.0 } else { let s = (PI / m.max(2) as f32).sin(); (r_all * s / (1.0 + s) * 0.9).min(r_all * 0.28) };
            let ring = r_all - r;
            let rc = if m == 0 { r_all * 0.6 } else { (r_all * 0.32).min((ring - r) * 0.9) };
            let mut lines = Vec::new();
            let mut nodes = Vec::new();
            for (i, it) in outer.iter().enumerate() {
                let a = -PI / 2.0 + 2.0 * PI * i as f32 / m as f32;
                let (ox, oy) = (cx + ring * a.cos(), cy + ring * a.sin());
                lines.push(line_sp(&look, 0, cx + rc * a.cos(), cy + rc * a.sin(), ox - r * a.cos(), oy - r * a.sin()));
                let mut s = look.node(i + 1, 0, false, "ellipse", (ox - r, oy - r, 2.0 * r, 2.0 * r));
                s.paras = vec![(0, it.text.clone())];
                nodes.push(s);
            }
            sps.extend(lines);
            let mut c = look.node(0, 0, false, "ellipse", (cx - rc, cy - rc, 2.0 * rc, 2.0 * rc));
            c.paras = vec![(0, center.text)];
            main.push(sps.len());
            sps.push(c);
            for s in nodes {
                minor.push(sps.len());
                sps.push(s);
            }
        }
        Kind::Hierarchy | Kind::OrgChart => {
            let m = items.len();
            let mut parent = vec![usize::MAX; m];
            let mut stack: Vec<usize> = Vec::new();
            for (i, it) in items.iter().enumerate() {
                stack.truncate(it.level);
                parent[i] = stack.last().copied().unwrap_or(usize::MAX);
                stack.push(i);
            }
            let kids = |p: usize| -> Vec<usize> { (0..m).filter(|&i| parent[i] == p).collect() };
            let depth = items.iter().map(|i| i.level).max().map_or(1, |d| d + 1) as f32;
            let total = (0..m).filter(|&i| kids(i).is_empty()).count();
            let slot = w / total.max(1) as f32;
            let bh = h / (depth + (depth - 1.0) * 0.6);
            let bw = (slot * 0.85).min(bh * 2.2);
            let mut cxs = vec![0.0f32; m];
            // Leaves left to right; parents centered over their children.
            let mut next = 0usize;
            fn place(i: usize, kids: &dyn Fn(usize) -> Vec<usize>, next: &mut usize, slot: f32, cxs: &mut [f32]) {
                let ks = kids(i);
                if ks.is_empty() {
                    cxs[i] = (*next as f32 + 0.5) * slot;
                    *next += 1;
                } else {
                    for &k in &ks {
                        place(k, kids, next, slot, cxs);
                    }
                    cxs[i] = (cxs[ks[0]] + cxs[ks[ks.len() - 1]]) / 2.0;
                }
            }
            for i in (0..m).filter(|&i| parent[i] == usize::MAX) {
                place(i, &kids, &mut next, slot, &mut cxs);
            }
            let top = |i: usize| items[i].level as f32 * bh * 1.6;
            for i in 0..m {
                let ks = kids(i);
                if ks.is_empty() {
                    continue;
                }
                let (px, py) = (cxs[i], top(i) + bh);
                let cy = top(ks[0]);
                if kind == Kind::Hierarchy {
                    for &k in &ks {
                        sps.push(line_sp(&look, 0, px, py, cxs[k], cy));
                    }
                } else {
                    let mid = (py + cy) / 2.0;
                    sps.push(line_sp(&look, 0, px, py, px, mid));
                    let (l, r) = (cxs[ks[0]].min(px), cxs[ks[ks.len() - 1]].max(px));
                    if r - l > 0.5 {
                        sps.push(line_sp(&look, 0, l, mid, r, mid));
                    }
                    for &k in &ks {
                        sps.push(line_sp(&look, 0, cxs[k], mid, cxs[k], cy));
                    }
                }
            }
            let geom = if kind == Kind::Hierarchy { "roundRect" } else { "rect" };
            for (i, it) in items.iter().enumerate() {
                let tint = if kind == Kind::Hierarchy { (it.level as u32 * 25_000).min(60_000) } else { 0 };
                let tint = if tint == 0 { 0 } else { 100_000 - tint };
                let mut s = look.node(if kind == Kind::OrgChart { 0 } else { it.level }, tint, false, geom, (cxs[i] - bw / 2.0, top(i), bw, bh));
                s.paras = vec![(0, it.text.clone())];
                main.push(sps.len());
                sps.push(s);
            }
        }
        Kind::Matrix => {
            let cells: Vec<String> = match gs.as_slice() {
                [(_, kids)] if !kids.is_empty() => kids.iter().filter(|k| k.0 == 0).map(|k| k.1.clone()).collect(),
                _ => gs.iter().map(|g| g.0.clone()).collect(),
            };
            let g = w.min(h) * 0.03;
            let (cw, ch) = ((w - g) / 2.0, (h - g) / 2.0);
            for (i, t) in cells.iter().take(4).enumerate() {
                let (c, r) = (i % 2, i / 2);
                let mut s = look.node(i, 0, false, "roundRect", (c as f32 * (cw + g), r as f32 * (ch + g), cw, ch));
                s.paras = vec![(0, t.clone())];
                main.push(sps.len());
                sps.push(s);
            }
        }
        Kind::Pyramid => {
            let pw = w.min(h * 1.15);
            let sh = h / n.max(1) as f32;
            for (i, (t, _)) in gs.iter().enumerate() {
                let (tw, bw) = (pw * i as f32 / n as f32, pw * (i + 1) as f32 / n as f32);
                let geom = if i == 0 { "triangle" } else { "trapezoid" };
                let mut s = look.node(i, 0, false, geom, ((w - bw) / 2.0, i as f32 * sh, bw, sh));
                if i > 0 {
                    s.adj = Some((((bw - tw) / 2.0) / bw.min(sh) * 100_000.0).round() as i64);
                }
                s.paras = vec![(0, t.clone())];
                main.push(sps.len());
                sps.push(s);
            }
        }
    }
    same_size(&mut sps, &main);
    same_size(&mut sps, &minor);
    let mut ids = Ids { state: 0x5EED_0000 ^ items.len() as u64, used: HashSet::new() };
    let mut out = format!(r#"{DECL}<dsp:drawing xmlns:dgm="{NS_DGM}" xmlns:dsp="{NS_DSP}" xmlns:a="{NS_A}"><dsp:spTree><dsp:nvGrpSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvGrpSpPr/></dsp:nvGrpSpPr><dsp:grpSpPr/>"#);
    for s in &sps {
        sp_xml(&mut out, s, &ids.next());
    }
    out.push_str("</dsp:spTree></dsp:drawing>");
    out
}

fn emu(v: f32) -> i64 {
    (v * EMU).round() as i64
}

fn sp_xml(out: &mut String, s: &Sp, id: &str) {
    let rot = if s.rot.abs() > 0.01 { format!(r#" rot="{}""#, (s.rot.rem_euclid(360.0) * 60_000.0).round() as i64) } else { String::new() };
    let flip = format!("{}{}", if s.flip_h { r#" flipH="1""# } else { "" }, if s.flip_v { r#" flipV="1""# } else { "" });
    let av = s.adj.map_or(String::new(), |v| format!(r#"<a:gd name="adj" fmla="val {v}"/>"#));
    let fx = if s.shadow { r#"<a:effectLst><a:outerShdw blurRad="40000" dist="23000" dir="5400000" rotWithShape="0"><a:srgbClr val="000000"><a:alpha val="35000"/></a:srgbClr></a:outerShdw></a:effectLst>"# } else { "<a:effectLst/>" };
    let _ = write!(out, r#"<dsp:sp modelId="{id}"><dsp:nvSpPr><dsp:cNvPr id="0" name=""/><dsp:cNvSpPr/></dsp:nvSpPr><dsp:spPr><a:xfrm{rot}{flip}><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/></a:xfrm><a:prstGeom prst="{}"><a:avLst>{av}</a:avLst></a:prstGeom>{}{}{fx}</dsp:spPr>"#, emu(s.x), emu(s.y), emu(s.w.max(0.0)), emu(s.h.max(0.0)), s.geom, s.fill, s.line);
    let _ = write!(out, r#"<dsp:style><a:lnRef idx="2"><a:scrgbClr r="0" g="0" b="0"/></a:lnRef><a:fillRef idx="1"><a:scrgbClr r="0" g="0" b="0"/></a:fillRef><a:effectRef idx="0"><a:scrgbClr r="0" g="0" b="0"/></a:effectRef><a:fontRef idx="minor"><a:schemeClr val="{}"/></a:fontRef></dsp:style>"#, s.color);
    if !s.paras.is_empty() {
        let ins = emu(s.size.min(12.0) * 0.4);
        let _ = write!(out, r#"<dsp:txBody><a:bodyPr spcFirstLastPara="0" vert="horz" wrap="square" lIns="{ins}" tIns="{ins}" rIns="{ins}" bIns="{ins}" numCol="1" spcCol="1270" anchor="{}" anchorCtr="0"><a:noAutofit/></a:bodyPr><a:lstStyle/>"#, if s.left && s.geom == "rect" && s.fill == "<a:noFill/>" { "t" } else { "ctr" });
        let sz = (s.size * 100.0).round() as i64;
        for (k, (lvl, text)) in s.paras.iter().enumerate() {
            let sub = *lvl > 0 || (k > 0 && s.left);
            let algn = if s.left || sub { "l" } else { "ctr" };
            let bullet = if *lvl > 0 { format!(r#" marL="{}" indent="-114300""#, 114_300 * *lvl as i64) } else { r#" marL="0" indent="0""#.into() };
            let bu = if *lvl > 0 { r#"<a:buChar char="•"/>"# } else { "<a:buNone/>" };
            let size = if *lvl > 0 && k > 0 && !s.left { sz * 3 / 4 } else { sz };
            let _ = write!(out, r#"<a:p><a:pPr{bullet} lvl="{}" algn="{algn}" defTabSz="{}" rtl="0"><a:lnSpc><a:spcPct val="90000"/></a:lnSpc><a:spcBef><a:spcPct val="0"/></a:spcBef><a:spcAft><a:spcPct val="35000"/></a:spcAft>{bu}</a:pPr>"#, (*lvl).min(8), emu(s.size * 3.0));
            if !text.is_empty() {
                let _ = write!(out, r#"<a:r><a:rPr lang="en-US" sz="{size}" kern="1200"><a:solidFill><a:schemeClr val="{}"/></a:solidFill></a:rPr><a:t>{}</a:t></a:r>"#, s.color, esc(text));
            }
            let _ = write!(out, r#"<a:endParaRPr lang="en-US" sz="{size}" kern="1200"/></a:p>"#);
        }
        out.push_str("</dsp:txBody>");
    }
    out.push_str("</dsp:sp>");
}

/// A diagram of family `kind` holding `items` in a `size` box (points).
/// Its layout part only names the layout (Office's definitions aren't
/// included), so it is for samples and tests, not for new files.
pub fn new_diagram(kind: Kind, items: &[Item], size: (f32, f32)) -> Diagram {
    let lo = layout_id(kind);
    let data = format!(r#"{DECL}<dgm:dataModel xmlns:dgm="{NS_DGM}" xmlns:a="{NS_A}"><dgm:ptLst><dgm:pt modelId="{{00000000-0000-0000-0000-000000000001}}" type="doc"><dgm:prSet loTypeId="{lo}" qsTypeId="{STYLE_URN}simple1" csTypeId="{COLORS_URN}accent1_2"/><dgm:spPr/><dgm:t><a:bodyPr/><a:lstStyle/><a:p><a:endParaRPr lang="en-US"/></a:p></dgm:t></dgm:pt></dgm:ptLst><dgm:cxnLst/><dgm:bg/><dgm:whole/></dgm:dataModel>"#);
    let layout = format!(r#"{DECL}<dgm:layoutDef xmlns:dgm="{NS_DGM}" xmlns:a="{NS_A}" uniqueId="{lo}"/>"#);
    let part = |role, xml: String| DiagramPart { role, xml: Arc::from(xml.into_bytes()), rels: Vec::new() };
    let d = Diagram { size, parts: vec![part(DiagramRole::Data, data), part(DiagramRole::Layout, layout), part(DiagramRole::Style, style_def("simple1")), part(DiagramRole::Colors, colors_def("accent1_2"))], ..Default::default() };
    match set_items(&d, items) {
        Some(d) if !items.is_empty() => d,
        _ => redraw(d, "accent1_2", "simple1", &[], kind),
    }
}

/// The layout id of a new diagram of family `kind`.
pub fn layout_id(kind: Kind) -> String {
    let name = match kind {
        Kind::Blocks | Kind::Other => "default",
        Kind::List => "vList2",
        Kind::Process => "process1",
        Kind::Chevron => "chevron1",
        Kind::Cycle => "cycle2",
        Kind::Radial => "radial1",
        Kind::Hierarchy => "hierarchy1",
        Kind::OrgChart => "orgChart1",
        Kind::Venn => "venn1",
        Kind::Matrix => "matrix3",
        Kind::Pyramid => "pyramid1",
    };
    format!("{LAYOUT_URN}{name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(role: DiagramRole, xml: &str) -> DiagramPart {
        DiagramPart { role, xml: Arc::from(xml.as_bytes()), rels: Vec::new() }
    }

    /// A process diagram: "Plan" with child "Budget", then "Do"; each
    /// shown by a pres point and a drawing shape.
    const DATA: &str = r#"<?xml version="1.0"?>
<dgm:dataModel xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><dgm:ptLst>
<dgm:pt modelId="{D}" type="doc"><dgm:prSet loTypeId="urn:microsoft.com/office/officeart/2005/8/layout/process1" qsTypeId="urn:microsoft.com/office/officeart/2005/8/quickstyle/simple1" csTypeId="urn:microsoft.com/office/officeart/2005/8/colors/accent1_2"/><dgm:spPr/><dgm:t><a:bodyPr/><a:lstStyle/><a:p><a:endParaRPr lang="en-US"/></a:p></dgm:t></dgm:pt>
<dgm:pt modelId="{A}"><dgm:prSet phldrT="[Text]"/><dgm:spPr><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></dgm:spPr><dgm:t><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" b="1"/><a:t>Plan</a:t></a:r></a:p></dgm:t></dgm:pt>
<dgm:pt modelId="{A1}"><dgm:prSet phldrT="[Text]"/><dgm:spPr/><dgm:t><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US"/><a:t>Budget</a:t></a:r></a:p></dgm:t></dgm:pt>
<dgm:pt modelId="{B}"><dgm:prSet phldrT="[Text]"/><dgm:spPr/><dgm:t><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US"/><a:t>Do</a:t></a:r></a:p></dgm:t></dgm:pt>
<dgm:pt modelId="{PA}" type="pres"><dgm:prSet presAssocID="{A}" presName="node"/></dgm:pt>
<dgm:pt modelId="{PB}" type="pres"><dgm:prSet presAssocID="{B}" presName="node"/></dgm:pt>
<dgm:pt modelId="{T1}" type="parTrans" cxnId="{C1}"/><dgm:pt modelId="{T2}" type="sibTrans" cxnId="{C1}"/>
</dgm:ptLst><dgm:cxnLst>
<dgm:cxn modelId="{C2}" srcId="{D}" destId="{B}" srcOrd="1" destOrd="0"/>
<dgm:cxn modelId="{C1}" srcId="{D}" destId="{A}" srcOrd="0" destOrd="0" parTransId="{T1}" sibTransId="{T2}"/>
<dgm:cxn modelId="{C3}" srcId="{A}" destId="{A1}" srcOrd="0" destOrd="0"/>
<dgm:cxn modelId="{C4}" type="presOf" srcId="{A}" destId="{PA}" srcOrd="0" destOrd="0"/>
<dgm:cxn modelId="{C5}" type="presOf" srcId="{A1}" destId="{PA}" srcOrd="0" destOrd="0"/>
<dgm:cxn modelId="{C6}" type="presOf" srcId="{B}" destId="{PB}" srcOrd="0" destOrd="0"/>
<dgm:cxn modelId="{C7}" srcId="{B}" destId="{B}" srcOrd="0" destOrd="0"/>
</dgm:cxnLst><dgm:bg/><dgm:whole/><dgm:extLst><a:ext uri="x"><dsp:dataModelExt xmlns:dsp="http://schemas.microsoft.com/office/drawing/2008/diagram" relId="rId9"/></a:ext></dgm:extLst></dgm:dataModel>"#;

    const DRAWING: &str = r#"<dsp:drawing xmlns:dsp="http://schemas.microsoft.com/office/drawing/2008/diagram" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><dsp:spTree><dsp:nvGrpSpPr/><dsp:grpSpPr/><dsp:sp modelId="{PA}"><dsp:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1270000" cy="635000"/></a:xfrm><a:prstGeom prst="roundRect"/></dsp:spPr><dsp:txBody><a:bodyPr/><a:p><a:pPr algn="ctr"/><a:r><a:rPr sz="2300" b="1"/><a:t>Plan</a:t></a:r></a:p><a:p><a:pPr lvl="1"/><a:r><a:rPr sz="1700"/><a:t>Budget</a:t></a:r></a:p></dsp:txBody></dsp:sp><dsp:sp modelId="{PB}"><dsp:spPr><a:xfrm><a:off x="1524000" y="0"/><a:ext cx="1270000" cy="635000"/></a:xfrm><a:prstGeom prst="roundRect"/></dsp:spPr><dsp:txBody><a:bodyPr/><a:p><a:r><a:rPr sz="2300"/><a:t>Do</a:t></a:r></a:p></dsp:txBody></dsp:sp></dsp:spTree></dsp:drawing>"#;

    fn sample(layout: &str) -> Diagram {
        let data = DATA.replace("layout/process1", &format!("layout/{layout}"));
        Diagram {
            size: (300.0, 100.0),
            shapes: read_drawing(DRAWING),
            parts: vec![part(DiagramRole::Data, &data), part(DiagramRole::Layout, &format!(r#"<dgm:layoutDef xmlns:dgm="{NS_DGM}" uniqueId="{LAYOUT_URN}{layout}"/>"#)), part(DiagramRole::Drawing, DRAWING)],
            texts: Vec::new(),
        }
    }

    fn xml(d: &Diagram, role: DiagramRole) -> String {
        String::from_utf8_lossy(&d.parts.iter().find(|p| p.role == role).unwrap().xml).into_owned()
    }

    fn texts(items: &[Item]) -> Vec<(usize, &str)> {
        items.iter().map(|i| (i.level, i.text.as_str())).collect()
    }

    #[test]
    fn reads_items_and_ids() {
        let i = info(&sample("process1")).unwrap();
        assert_eq!(i.kind, Kind::Process);
        assert_eq!((i.layout.as_str(), i.colors.as_str(), i.style.as_str()), ("process1", "accent1_2", "simple1"));
        assert_eq!(texts(&i.items), [(0, "Plan"), (1, "Budget"), (0, "Do")]);
        assert_eq!(Kind::of("urn:microsoft.com/office/officeart/2005/8/layout/orgChart1"), Kind::OrgChart);
        assert_eq!(Kind::of("gear1"), Kind::Other);
        assert!(info(&Diagram::default()).is_none());
    }

    #[test]
    fn outline_text_round_trip() {
        let items = vec![Item::new(0, "a"), Item::new(1, "b"), Item::new(2, "c"), Item::new(0, "d")];
        assert_eq!(parse_outline(&outline_text(&items)), items);
        assert_eq!(texts(&parse_outline("  x\n\n    y\n  z")), [(0, "x"), (1, "y"), (1, "z")]);
    }

    #[test]
    fn text_only_changes_keep_the_rest() {
        let d = sample("gear1");
        let items = vec![Item::new(0, "Plan well"), Item::new(1, "Budget"), Item::new(0, "Do & check")];
        let e = set_items(&d, &items).unwrap();
        assert_eq!(texts(&info(&e).unwrap().items), texts(&items));
        let data = xml(&e, DiagramRole::Data);
        // Formatting, pres points and presentation connections are kept.
        assert!(data.contains(r#"<a:rPr lang="en-US" b="1"/><a:t>Plan well</a:t>"#), "{data}");
        assert!(data.contains("FF0000") && data.contains("{PA}") && data.contains("presOf") && data.contains("<dgm:whole/>"));
        let drawing = xml(&e, DiagramRole::Drawing);
        assert!(drawing.contains(r#"<a:rPr sz="2300" b="1"/><a:t>Plan well</a:t>"#), "{drawing}");
        assert!(drawing.contains("<a:t>Budget</a:t>") && drawing.contains("Do &amp; check"));
        assert_eq!(e.shapes.len(), 2);
        assert_eq!(e.shapes[1].shape.plain_text(), "Do & check");
        assert!(e.texts.contains(&"Plan well".to_string()));
        // Layouts not drawn here can't change their items.
        assert!(set_items(&d, &items[..2]).is_none());
        assert!(set_colors(&d, "colorful1").is_none());
        // Nothing changed: the same diagram.
        assert_eq!(xml(&set_items(&d, &info(&d).unwrap().items).unwrap(), DiagramRole::Data), xml(&d, DiagramRole::Data));
    }

    #[test]
    fn adding_and_removing_rebuild_the_diagram() {
        let d = sample("process1");
        let items = vec![Item::new(0, "Plan"), Item::new(0, "Do"), Item::new(1, "Test"), Item::new(0, "Ship")];
        let e = set_items(&d, &items).unwrap();
        assert_eq!(texts(&info(&e).unwrap().items), texts(&items));
        let data = xml(&e, DiagramRole::Data);
        let root = read_fragment(&data);
        let model = top(&root, "dataModel").unwrap();
        let mut all = Vec::new();
        model.find_all("pt", &mut all);
        model.find_all("cxn", &mut all);
        let ids: Vec<&str> = all.iter().filter_map(|n| n.attr("modelId")).collect();
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len(), "ids are unique");
        // 1 doc + 4 items + 2 transitions each; old pres points gone.
        assert_eq!(model.child("ptLst").unwrap().children("pt").count(), 13);
        assert!(!data.contains("{PA}") && !data.contains("presOf"));
        // Reused points keep their look; "Budget" was reused as "Do".
        assert!(data.contains("FF0000") && data.contains("<dgm:whole/>") && data.contains("dataModelExt"));
        let drawing = xml(&e, DiagramRole::Drawing);
        assert!(drawing.contains("rightArrow") && drawing.contains("<a:t>Ship</a:t>"));
        // 3 boxes and 2 arrows.
        assert_eq!(e.shapes.len(), 5);
        assert!(e.shapes.iter().all(|s| s.x >= -0.5 && s.x + s.w <= 300.5 && s.y >= -0.5 && s.y + s.h <= 100.5));
        // Removing down to one item.
        let f = set_items(&e, &[Item::new(0, "Only")]).unwrap();
        assert_eq!(texts(&info(&f).unwrap().items), [(0, "Only")]);
        assert_eq!(f.shapes.len(), 1);
        // Text-only edits of a rebuilt diagram redraw it.
        let g = set_items(&f, &[Item::new(0, "Alone")]).unwrap();
        assert_eq!(g.shapes[0].shape.plain_text(), "Alone");
        // It writes to a package like any diagram.
        let w = crate::diagram::write(&g, 1, "ppt", "../");
        assert!(w.files.iter().any(|f| f.0 == "ppt/diagrams/drawing1.xml"));
    }

    #[test]
    fn every_layout_draws_inside_its_box() {
        let items = vec![Item::new(0, "One"), Item::new(1, "One a"), Item::new(1, "One b"), Item::new(0, "Two"), Item::new(2, "Deep"), Item::new(0, "Three"), Item::new(0, "")];
        let items = normalized(&items);
        for kind in [Kind::Blocks, Kind::List, Kind::Process, Kind::Chevron, Kind::Cycle, Kind::Radial, Kind::Hierarchy, Kind::OrgChart, Kind::Venn, Kind::Matrix, Kind::Pyramid] {
            for n in [0, 1, 2, items.len()] {
                let shapes = read_drawing(&draw(kind, &items[..n], (400.0, 250.0), "colorful1", "simple3"));
                assert!(n == 0 || !shapes.is_empty(), "{kind:?} {n}");
                for s in &shapes {
                    assert!(s.x.is_finite() && s.w.is_finite() && s.w >= 0.0 && s.h >= 0.0, "{kind:?}");
                    assert!(s.x >= -1.0 && s.y >= -1.0 && s.x + s.w <= 401.0 && s.y + s.h <= 251.0, "{kind:?} {n}: {:?}", (s.x, s.y, s.w, s.h));
                }
            }
        }
    }

    #[test]
    fn colors_and_style() {
        let d = sample("cycle2");
        let e = set_colors(&d, "accent3_2").unwrap();
        let i = info(&e).unwrap();
        assert_eq!((i.colors.as_str(), i.style.as_str()), ("accent3_2", "simple1"));
        assert!(xml(&e, DiagramRole::Colors).contains(&format!(r#"uniqueId="{COLORS_URN}accent3_2""#)));
        assert_eq!(e.shapes[0].shape.fill, Some(0xA5A5A5));
        let f = set_style(&e, "simple4").unwrap();
        assert_eq!(info(&f).unwrap().style, "simple4");
        assert!(xml(&f, DiagramRole::Style).contains("quickstyle/simple4"));
        assert_eq!(f.shapes[0].shape.line_width, 3.0);
        // The items and the layout part stay.
        assert_eq!(texts(&info(&f).unwrap().items), [(0, "Plan"), (1, "Budget"), (0, "Do")]);
        assert_eq!(xml(&f, DiagramRole::Layout), xml(&d, DiagramRole::Layout));
        for c in COLORS {
            assert!(read_fragment(&colors_def(c)).find("styleLbl").is_some());
        }
    }

    /// Edits every diagram of every .pptx under $DIAGRAM_CORPUS: its
    /// text, then (where drawn here) an added item and other colors,
    /// written and read back (and kept in $DIAGRAM_OUT, if set).
    #[test]
    #[ignore]
    fn corpus_edits() {
        let Ok(dir) = std::env::var("DIAGRAM_CORPUS") else { return };
        let mut files = Vec::new();
        let mut stack = vec![std::path::PathBuf::from(dir)];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "pptx") {
                    files.push(p);
                }
            }
        }
        files.sort();
        let (mut seen, mut drawn, mut bad) = (0, 0, Vec::new());
        for f in &files {
            let data = std::fs::read(f).unwrap();
            let Ok(mut deck) = crate::pptx::read(&data) else { continue };
            let mut found = false;
            for si in 0..deck.slides.len() {
                for o in &mut deck.slide_mut(si).objects {
                    let apex_slides::ObjKind::Diagram(d) = &o.kind else { continue };
                    let Some(i) = info(d).filter(|i| !i.locked) else { continue };
                    seen += 1;
                    found = true;
                    let mut items = i.items.clone();
                    match items.first_mut() {
                        Some(first) => first.text.push_str(" edited"),
                        None if i.kind.drawn() => items.push(Item::new(0, "edited")),
                        None => continue,
                    }
                    let Some(mut e) = set_items(d, &items) else {
                        bad.push(format!("{}: text", f.display()));
                        continue;
                    };
                    if i.kind.drawn() {
                        drawn += 1;
                        items.push(Item::new(0, "Added"));
                        e = set_items(&e, &items).and_then(|e| set_colors(&e, "colorful1")).unwrap();
                    }
                    if info(&e).map(|j| j.items) != Some(normalized(&items)) {
                        bad.push(format!("{}: items", f.display()));
                    }
                    o.kind = apex_slides::ObjKind::Diagram(Arc::new(e));
                    o.touch();
                }
            }
            if found {
                if let Ok(out) = std::env::var("DIAGRAM_OUT") {
                    let _ = std::fs::write(std::path::Path::new(&out).join(f.file_name().unwrap()), crate::pptx::write(&deck));
                }
                let back = crate::pptx::read(&crate::pptx::write(&deck));
                let texts: Vec<String> = back.iter().flat_map(|d| d.slides.iter()).flat_map(|s| s.objects.iter()).filter_map(|o| match &o.kind {
                    apex_slides::ObjKind::Diagram(d) => Some(d.texts.join(" ")),
                    _ => None,
                }).collect();
                if !texts.iter().any(|t| t.contains("edited")) {
                    bad.push(format!("{}: written", f.display()));
                }
            }
        }
        eprintln!("{seen} diagrams, {drawn} drawn here, {} bad", bad.len());
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    fn new_diagrams() {
        let items = vec![Item::new(0, "Boss"), Item::new(1, "Ann"), Item::new(1, "Bob")];
        let d = new_diagram(Kind::OrgChart, &items, (300.0, 200.0));
        let i = info(&d).unwrap();
        assert_eq!((i.kind, i.items.clone()), (Kind::OrgChart, items));
        // 3 boxes and 4 connector lines.
        assert_eq!(d.shapes.len(), 7);
        assert!(new_diagram(Kind::Venn, &[], (100.0, 100.0)).shapes.is_empty());
    }

    #[test]
    fn hostile_models() {
        // Connections in a loop and to missing points.
        let d = Diagram { parts: vec![part(DiagramRole::Data, r#"<dgm:dataModel xmlns:dgm="x"><dgm:ptLst><dgm:pt modelId="d" type="doc"/><dgm:pt modelId="a"/><dgm:pt modelId="b"/></dgm:ptLst><dgm:cxnLst><dgm:cxn srcId="d" destId="a"/><dgm:cxn srcId="a" destId="b"/><dgm:cxn srcId="b" destId="a"/><dgm:cxn srcId="a" destId="zz"/></dgm:cxnLst></dgm:dataModel>"#)], ..Default::default() };
        let i = info(&d).unwrap();
        assert_eq!(i.items.len(), 2);
        assert_eq!(i.kind, Kind::Other);
        let e = set_items(&d, &[Item::new(0, "x"), Item::new(1, "y")]).unwrap();
        assert_eq!(texts(&info(&e).unwrap().items), [(0, "x"), (1, "y")]);
        let many: Vec<Item> = (0..500).map(|k| Item::new(k, "deep")).collect();
        assert!(normalized(&many).iter().all(|i| i.level <= MAX_LEVEL) && normalized(&many).len() == MAX_ITEMS);
    }
}
