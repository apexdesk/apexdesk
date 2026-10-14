//! The objects of a slide, layout or master (`p:spTree`): shapes,
//! connectors, pictures, groups and graphic frames (tables, charts,
//! diagrams, embedded objects). Each keeps the XML it was read from, and
//! goes back as it came while unchanged.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{Theme, VAnchor};
use apex_slides::color::ClrMap;
use apex_slides::object::{Frame, Geometry, Group, Media as ObjMedia, ObjKind, Object, PhType, Picture, Placeholder, Raw, ShapeProps, Src};
use apex_slides::table::{Cell, Row, Table, TableLook};

use crate::drawingml::{EMU_PT, Node, TEXT, read_node};
use crate::pptx_fill::{self as fill, Images, Media};
use crate::pptx_text::{self as text, Links, TextCx};
use crate::xml::{Ev, Reader, esc};
use crate::zip::ZipReader;

pub(crate) const URI_TABLE: &str = "http://schemas.openxmlformats.org/drawingml/2006/table";
pub(crate) const URI_CHART: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
pub(crate) const URI_DIAGRAM: &str = "http://schemas.openxmlformats.org/drawingml/2006/diagram";
pub(crate) const REL_CHART: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart";
pub(crate) const REL_HYPERLINK: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink";
pub(crate) const REL_SLIDE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide";
pub(crate) const REL_IMAGE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";
pub(crate) const REL_VIDEO: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/video";
pub(crate) const REL_AUDIO: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/audio";
pub(crate) const REL_MEDIA: &str = "http://schemas.microsoft.com/office/2007/relationships/media";
pub(crate) const REL_PACKAGE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/package";

/// A part being read: its relationships and what they point at.
pub(crate) struct PartCx<'a> {
    pub z: &'a ZipReader<'a>,
    pub path: String,
    /// id → (type, target in the package or external, external).
    pub rels: HashMap<String, (String, String, bool)>,
    pub media: Media,
    /// Hyperlinks by id: URLs, or "#slide:ID" for slides.
    pub links: HashMap<String, Arc<str>>,
    pub theme: &'a Theme,
    pub map: ClrMap,
    pub diagrams: Option<crate::diagram::Parts>,
    /// Bytes of XML groups may still read and keep: each level reads and
    /// keeps its own again, and nesting would multiply them.
    pub raw_left: usize,
    /// Objects the part may still read, groups' included.
    pub objects_left: usize,
    /// Table cells the part may still make (an empty row of a wide grid
    /// is filled with cells).
    pub cells_left: std::cell::Cell<usize>,
}

thread_local! {
    /// The pictures and media of the package being read, each read once.
    static READ: std::cell::RefCell<Option<HashMap<String, Arc<[u8]>>>> = const { std::cell::RefCell::new(None) };
}

/// Keeps the parts [`read_once`] reads while it lives.
pub(crate) struct ReadOnce;

impl ReadOnce {
    pub(crate) fn new() -> ReadOnce {
        READ.with_borrow_mut(|c| *c = Some(HashMap::new()));
        ReadOnce
    }
}

impl Drop for ReadOnce {
    fn drop(&mut self) {
        READ.with_borrow_mut(|c| *c = None);
    }
}

/// The part at `target`, read once while a [`ReadOnce`] lives.
fn read_once(z: &ZipReader, target: &str) -> Option<Arc<[u8]>> {
    READ.with_borrow_mut(|c| {
        if let Some(d) = c.as_ref().and_then(|m| m.get(target)) {
            return Some(d.clone());
        }
        let d: Arc<[u8]> = Arc::from(z.read(target).ok()?);
        if let Some(m) = c {
            m.insert(target.to_string(), d.clone());
        }
        Some(d)
    })
}

impl<'a> PartCx<'a> {
    /// `slide_ids`: slide part → slide id, for links between slides.
    pub(crate) fn new(z: &'a ZipReader<'a>, path: &str, theme: &'a Theme, map: ClrMap, slide_ids: &HashMap<String, u32>) -> PartCx<'a> {
        let mut rels = HashMap::new();
        let mut media = Media::new();
        let mut links = HashMap::new();
        for (id, ty, target, ext) in crate::ooxml_keep::read_rels(z, path) {
            let short = ty.rsplit('/').next().unwrap_or("");
            if !ext && matches!(short, "image") {
                // A picture many ids point at is read once.
                if let Some(d) = read_once(z, &target) {
                    media.insert(id.clone(), d);
                }
            }
            if short == "hyperlink" {
                links.insert(id.clone(), Arc::from(target.as_str()));
            } else if short == "slide" {
                if let Some(sid) = slide_ids.get(&target) {
                    links.insert(id.clone(), Arc::from(format!("#slide:{sid}")));
                }
            }
            rels.insert(id, (ty, target, ext));
        }
        PartCx { z, path: path.to_string(), rels, media, links, theme, map, diagrams: None, raw_left: usize::MAX, objects_left: apex_slides::MAX_OBJECTS, cells_left: std::cell::Cell::new(1 << 16) }
    }

    pub(crate) fn text_cx(&self) -> TextCx<'_> {
        TextCx { theme: self.theme, map: self.map, links: &self.links }
    }

    /// The bytes of the part a relationship points at.
    fn target(&self, id: &str) -> Option<(String, Vec<u8>)> {
        let (_, t, ext) = self.rels.get(id)?;
        if *ext {
            return None;
        }
        self.z.read(t).ok().map(|d| (t.clone(), d))
    }
}

/// Serializes an element tree again (for pieces kept as XML).
pub(crate) fn node_xml(out: &mut String, n: &Node) {
    if n.name == TEXT {
        out.push_str(&esc(&n.text));
        return;
    }
    let _ = write!(out, "<{}", n.name);
    for (k, v) in &n.attrs {
        let _ = write!(out, r#" {k}="{}""#, esc(v));
    }
    if n.kids.is_empty() {
        out.push_str("/>");
        return;
    }
    out.push('>');
    for k in &n.kids {
        node_xml(out, k);
    }
    let _ = write!(out, "</{}>", n.name);
}

pub(crate) fn node_string(n: &Node) -> String {
    let mut s = String::new();
    node_xml(&mut s, n);
    s
}

fn emu(n: &Node, k: &str) -> f32 {
    n.num(k).map_or(0.0, |v| v as f32 / EMU_PT)
}

/// A `a:xfrm` (or `p:xfrm`).
pub(crate) fn read_xfrm(x: &Node) -> Frame {
    let mut f = Frame::default();
    if let Some(o) = x.child("off") {
        (f.x, f.y) = (emu(o, "x"), emu(o, "y"));
    }
    if let Some(e) = x.child("ext") {
        (f.w, f.h) = (emu(e, "cx"), emu(e, "cy"));
    }
    f.rot = x.num("rot").map_or(0.0, |r| (r / 60_000.0) as f32);
    f.flip_h = x.attr("flipH").is_some_and(|v| v == "1" || v == "true");
    f.flip_v = x.attr("flipV").is_some_and(|v| v == "1" || v == "true");
    f.sanitize();
    f
}

fn e(v: f32) -> i64 {
    (v * EMU_PT).round() as i64
}

pub(crate) fn write_xfrm(out: &mut String, tag: &str, f: &Frame, child: Option<&Frame>) {
    let _ = write!(out, "<{tag}");
    if f.rot != 0.0 {
        let _ = write!(out, r#" rot="{}""#, (f.rot * 60_000.0).round() as i64);
    }
    if f.flip_h {
        out.push_str(r#" flipH="1""#);
    }
    if f.flip_v {
        out.push_str(r#" flipV="1""#);
    }
    let _ = write!(out, r#"><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/>"#, e(f.x), e(f.y), e(f.w), e(f.h));
    if let Some(c) = child {
        let _ = write!(out, r#"<a:chOff x="{}" y="{}"/><a:chExt cx="{}" cy="{}"/>"#, e(c.x), e(c.y), e(c.w), e(c.h));
    }
    let _ = write!(out, "</{tag}>");
}

/// A shape's `spPr`.
fn read_sp_pr(n: &Node, media: &Media) -> ShapeProps {
    let mut sp = ShapeProps::default();
    for k in &n.kids {
        match k.local() {
            "xfrm" => sp.frame = Some(read_xfrm(k)),
            "prstGeom" => {
                let av = k.child("avLst").map_or(Vec::new(), |l| {
                    l.children("gd").filter_map(|g| Some((Arc::from(g.attr("name")?), g.attr("fmla")?.strip_prefix("val ")?.trim().parse().ok()?))).collect()
                });
                sp.geom = Some(Geometry::Preset(Arc::from(k.attr("prst").unwrap_or("rect")), av));
            }
            "custGeom" => sp.geom = Some(Geometry::Custom(Arc::from(node_string(k)))),
            "ln" => sp.line = Some(fill::read_line(k, media)),
            "effectLst" => {
                sp.shadow = fill::read_shadow(k);
                sp.extra.push(Arc::from(node_string(k)));
            }
            TEXT => {}
            _ => {
                if let Some(f) = fill::read_fill_el(k, media) {
                    sp.fill = Some(f);
                } else {
                    sp.extra.push(Arc::from(node_string(k)));
                }
            }
        }
    }
    sp
}

fn write_sp_pr(out: &mut String, tag: &str, sp: &ShapeProps, imgs: &mut dyn Images) {
    let _ = write!(out, "<{tag}>");
    if let Some(f) = &sp.frame {
        write_xfrm(out, "a:xfrm", f, None);
    }
    match &sp.geom {
        Some(Geometry::Preset(name, av)) => {
            let _ = write!(out, r#"<a:prstGeom prst="{}"><a:avLst>"#, esc(name));
            for (n, v) in av {
                let _ = write!(out, r#"<a:gd name="{}" fmla="val {v}"/>"#, esc(n));
            }
            out.push_str("</a:avLst></a:prstGeom>");
        }
        Some(Geometry::Custom(xml)) => out.push_str(xml),
        None => {}
    }
    if let Some(f) = &sp.fill {
        fill::write_fill(out, f, imgs);
    }
    if let Some(l) = &sp.line {
        fill::write_line(out, l, imgs);
    }
    let has_effects = sp.extra.iter().any(|x| x.starts_with("<a:effectLst") || x.starts_with("<a:effectDag"));
    if let (Some(s), false) = (&sp.shadow, has_effects) {
        fill::write_shadow(out, s);
    }
    for x in &sp.extra {
        out.push_str(x);
    }
    let _ = write!(out, "</{tag}>");
}

/// The non-visual properties (`p:nvSpPr`...): id, name, alt text,
/// placeholder, click action.
fn read_nv(o: &mut Object, nv: &Node, cx: &PartCx) {
    if let Some(c) = nv.child("cNvPr") {
        o.id = c.num("id").unwrap_or(0.0) as u32;
        o.name = c.attr("name").unwrap_or("").to_string();
        o.descr = c.attr("descr").unwrap_or("").to_string();
        o.hidden = c.attr("hidden").is_some_and(|v| v == "1" || v == "true");
        if let Some(h) = c.child("hlinkClick") {
            let id = h.attr("r:id").unwrap_or("");
            o.click = cx.links.get(id).cloned().or_else(|| {
                let a = h.attr("action")?;
                let l = a.rsplit('=').next()?;
                Some(Arc::from(match l {
                    "nextslide" => "#next",
                    "previousslide" => "#prev",
                    "firstslide" => "#first",
                    "lastslide" => "#last",
                    "endshow" => "#end",
                    _ => return None,
                }))
            });
        }
    }
    if let Some(ph) = nv.child("nvPr").and_then(|p| p.child("ph")) {
        let mut p = Placeholder::new(PhType::parse(ph.attr("type").unwrap_or("obj")), ph.num("idx").unwrap_or(0.0) as u32);
        if ph.attr("type").is_none() {
            p.kind = PhType::Obj;
        }
        p.vert = ph.attr("orient") == Some("vert");
        p.custom_prompt = ph.attr("hasCustomPrompt").is_some_and(|v| v == "1" || v == "true");
        o.ph = Some(p);
    }
}

/// The `p:cNvPr` of an object.
fn write_cnv_pr(out: &mut String, o: &Object, links: &mut dyn Links) {
    let _ = write!(out, r#"<p:cNvPr id="{}" name="{}""#, o.id, esc(&o.name));
    if !o.descr.is_empty() {
        let _ = write!(out, r#" descr="{}""#, esc(&o.descr));
    }
    if o.hidden {
        out.push_str(r#" hidden="1""#);
    }
    match &o.click {
        None => out.push_str("/>"),
        Some(l) => {
            out.push('>');
            write_click(out, l, links);
            out.push_str("</p:cNvPr>");
        }
    }
}

pub(crate) fn write_click(out: &mut String, l: &str, links: &mut dyn Links) {
    let jump = |v: &str| format!(r#"<a:hlinkClick r:id="" action="ppaction://hlinkshowjump?jump={v}"/>"#);
    match l {
        "#next" => out.push_str(&jump("nextslide")),
        "#prev" => out.push_str(&jump("previousslide")),
        "#first" => out.push_str(&jump("firstslide")),
        "#last" => out.push_str(&jump("lastslide")),
        "#end" => out.push_str(&jump("endshow")),
        l if l.starts_with("#slide:") => {
            let id = links.link(l);
            let _ = write!(out, r#"<a:hlinkClick r:id="{id}" action="ppaction://hlinksldjump"/>"#);
        }
        l => {
            let id = links.link(l);
            let _ = write!(out, r#"<a:hlinkClick r:id="{id}"/>"#);
        }
    }
}

fn write_nv_pr(out: &mut String, o: &Object, extra: &str) {
    match o.ph {
        Some(p) => {
            out.push_str("<p:nvPr><p:ph");
            if p.kind != PhType::Obj {
                let _ = write!(out, r#" type="{}""#, p.kind.name());
            }
            if p.vert {
                out.push_str(r#" orient="vert""#);
            }
            if p.idx != 0 {
                let _ = write!(out, r#" idx="{}""#, p.idx);
            }
            if p.custom_prompt {
                out.push_str(r#" hasCustomPrompt="1""#);
            }
            let _ = write!(out, "/>{extra}</p:nvPr>");
        }
        None if extra.is_empty() => out.push_str("<p:nvPr/>"),
        None => {
            let _ = write!(out, "<p:nvPr>{extra}</p:nvPr>");
        }
    }
}

/// Reads the objects in the XML inside a `p:spTree` (or a group), each
/// with its source.
pub(crate) fn read_tree(xml: &str, cx: &mut PartCx) -> Vec<Object> {
    cx.raw_left = xml.len().saturating_mul(8).saturating_add(1 << 20);
    read_tree_in(xml, cx, 0)
}

/// Groups deeper than this are left out (each level reads its own XML
/// again; a file nesting thousands would exhaust the stack).
const MAX_GROUP_DEPTH: usize = 64;

fn read_tree_in(xml: &str, cx: &mut PartCx, depth: usize) -> Vec<Object> {
    let mut out = Vec::new();
    let mut r = Reader::new(xml);
    loop {
        let before = r.pos();
        match r.next() {
            Ev::Eof | Ev::End(_) => break,
            Ev::Start(t) => {
                let local = t.local();
                if matches!(local, "nvGrpSpPr" | "grpSpPr" | "extLst") {
                    r.skip();
                    continue;
                }
                let group_inner_start = r.pos();
                if cx.objects_left == 0 {
                    r.skip();
                    continue;
                }
                cx.objects_left -= 1;
                if local == "grpSp" && depth >= MAX_GROUP_DEPTH {
                    r.skip();
                    continue;
                }
                if local == "grpSp" {
                    // Children read with their own sources.
                    let mut rr = r.clone();
                    let n = read_node(&mut rr, &t, false);
                    let raw = &xml[before..rr.pos()];
                    let inner = &xml[group_inner_start..rr.pos()];
                    r = rr;
                    // Past the budget, the group is left out.
                    if raw.len() > cx.raw_left {
                        continue;
                    }
                    cx.raw_left -= raw.len();
                    if let Some(o) = read_group(&n, inner, raw, cx, depth) {
                        out.push(o);
                    }
                    continue;
                }
                let n = read_node(&mut r, &t, false);
                let raw = &xml[before..r.pos()];
                if let Some(mut o) = read_object(&n, cx) {
                    o.raw = Some(Raw { src: Src::Pptx, xml: Arc::from(raw) });
                    if let ObjKind::Other { xml, .. } = &mut o.kind {
                        *xml = o.raw.clone();
                    }
                    out.push(o);
                }
            }
            _ => {}
        }
    }
    out
}

fn read_group(n: &Node, inner: &str, raw: &str, cx: &mut PartCx, depth: usize) -> Option<Object> {
    let mut o = Object::new(0, "", ObjKind::Group(Box::default()));
    read_nv(&mut o, n.child("nvGrpSpPr")?, cx);
    let mut child = Frame::default();
    if let Some(x) = n.child("grpSpPr").and_then(|p| p.child("xfrm")) {
        o.sp.frame = Some(read_xfrm(x));
        if let Some(c) = x.child("chOff") {
            (child.x, child.y) = (emu(c, "x"), emu(c, "y"));
        }
        if let Some(c) = x.child("chExt") {
            (child.w, child.h) = (emu(c, "cx"), emu(c, "cy"));
        }
    }
    if let Some(gp) = n.child("grpSpPr") {
        let sp = read_sp_pr(gp, &cx.media);
        o.sp.fill = sp.fill;
        o.sp.extra = sp.extra;
    }
    // The children, after the group's own properties (the inner XML
    // includes them; `read_tree` skips them).
    let children = read_tree_in(inner, cx, depth + 1);
    o.kind = ObjKind::Group(Box::new(Group { children, child }));
    o.raw = Some(Raw { src: Src::Pptx, xml: Arc::from(raw) });
    Some(o)
}

/// One object of a shape tree.
fn read_object(n: &Node, cx: &mut PartCx) -> Option<Object> {
    match n.local() {
        "sp" | "cxnSp" => {
            let connector = n.local() == "cxnSp";
            let nv = n.child(if connector { "nvCxnSpPr" } else { "nvSpPr" })?;
            let text_box = nv.child("cNvSpPr").is_some_and(|c| c.attr("txBox").is_some_and(|v| v == "1" || v == "true"));
            let text = n.child("txBody").map(|b| text::read_text_body(b, &cx.text_cx()));
            let mut o = Object::new(0, "", ObjKind::Shape { text, text_box, connector });
            read_nv(&mut o, nv, cx);
            if let Some(sp) = n.child("spPr") {
                o.sp = read_sp_pr(sp, &cx.media);
            }
            o.style = n.child("style").map(fill::read_style);
            Some(o)
        }
        "pic" => {
            let nv = n.child("nvPicPr")?;
            let blip = n.child("blipFill");
            let data = blip.and_then(|b| b.child("blip")).and_then(|b| b.attr("r:embed")).and_then(|id| cx.media.get(id)).cloned().unwrap_or_else(|| Arc::from(&[][..]));
            let mut image = apex_doc::Image::new(data, 0.0, 0.0);
            if let Some(r) = blip.and_then(|b| b.child("srcRect")) {
                image.crop = ["l", "t", "r", "b"].map(|k| (r.num(k).unwrap_or(0.0) / 100_000.0) as f32);
            }
            let media = nv.child("nvPr").and_then(|p| read_media(p, cx));
            let mut o = Object::new(0, "", ObjKind::Picture(Box::new(Picture { image, media })));
            read_nv(&mut o, nv, cx);
            if let Some(sp) = n.child("spPr") {
                o.sp = read_sp_pr(sp, &cx.media);
            }
            o.style = n.child("style").map(fill::read_style);
            if let (ObjKind::Picture(p), Some(f)) = (&mut o.kind, o.sp.frame) {
                (p.image.width, p.image.height) = (f.w, f.h);
                p.image.name = o.name.clone();
                p.image.alt = o.descr.clone();
            }
            Some(o)
        }
        "graphicFrame" => read_frame(n, cx),
        "AlternateContent" => {
            // Newer content (math, ink, 3D models) and its fallback, shown.
            let choice = n.child("Choice");
            if let Some(sp) = choice.and_then(|c| c.child("sp")).filter(|s| s.find("m").is_some_and(|m| m.name == "a14:m")) {
                let mut o = read_object(sp, cx)?;
                if let Some(eq) = math_of(sp) {
                    o.kind = ObjKind::Math(eq);
                }
                return Some(o);
            }
            if let Some(gf) = choice.and_then(|c| c.child("graphicFrame")) {
                if let Some(o) = read_frame(gf, cx).filter(|o| !matches!(o.kind, ObjKind::Other { .. })) {
                    return Some(o);
                }
            }
            let fb = n.child("Fallback")?;
            let first = fb.kids.iter().find(|k| k.name != TEXT)?;
            let mut o = read_object(first, cx)?;
            if choice.is_some_and(|c| c.find("contentPart").is_some()) {
                o.kind = ObjKind::Other { preview: picture_of(&o), what: Arc::from("ink"), xml: None };
            }
            Some(o)
        }
        "contentPart" => {
            let mut o = Object::new(0, "Ink", ObjKind::Other { preview: None, what: Arc::from("ink"), xml: None });
            if let Some(x) = n.find("xfrm") {
                o.sp.frame = Some(read_xfrm(x));
            }
            Some(o)
        }
        _ => None,
    }
}

fn picture_of(o: &Object) -> Option<Arc<[u8]>> {
    match &o.kind {
        ObjKind::Picture(p) if !p.image.data.is_empty() => Some(p.image.data.clone()),
        _ => None,
    }
}

/// The equation of a shape holding `a14:m`.
fn math_of(sp: &Node) -> Option<Arc<apex_doc::math::Equation>> {
    let m = sp.find("m")?;
    let inner = m.kids.iter().find(|k| k.name != TEXT)?;
    let xml = node_string(inner);
    let mut r = Reader::new(&xml);
    let Ev::Start(t) = r.next() else { return None };
    crate::omml::read(&mut r, &t).into_iter().next().map(|e| e.0)
}

/// A video or sound on a picture's `p:nvPr`.
fn read_media(nv: &Node, cx: &PartCx) -> Option<ObjMedia> {
    let (el, video) = match (nv.child("videoFile"), nv.child("audioFile")) {
        (Some(v), _) => (v, true),
        (_, Some(a)) => (a, false),
        _ => return None,
    };
    let mut m = ObjMedia { video, volume: 1.0, ..Default::default() };
    let mut ids: Vec<&str> = el.attr("r:link").into_iter().collect();
    let mut p14 = Vec::new();
    nv.find_all("media", &mut p14);
    ids.extend(p14.iter().filter_map(|n| n.attr("r:embed")));
    for id in ids {
        match cx.rels.get(id) {
            Some((_, t, true)) => m.link = Some(t.clone()),
            Some((_, path, false)) => {
                if let Some(d) = read_once(cx.z, path) {
                    m.mime = mime_of(path).to_string();
                    m.data = Some(d);
                    m.link = None;
                    break;
                }
            }
            None => {}
        }
    }
    if let Some(t) = p14.first().and_then(|n| n.child("trim")) {
        m.trim = (t.num("st").unwrap_or(0.0) as f32 / 1000.0, t.num("end").unwrap_or(0.0) as f32 / 1000.0);
    }
    Some(m)
}

pub(crate) fn mime_of(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "wmv" => "video/x-ms-wmv",
        "avi" => "video/x-msvideo",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "wav" => "audio/wav",
        "wma" => "audio/x-ms-wma",
        "ogg" => "audio/ogg",
        _ => "application/octet-stream",
    }
}

/// The file extension of a video or sound's media type ("bin" if unknown).
pub(crate) fn ext_of_mime(mime: &str) -> &'static str {
    match mime {
        "video/mp4" => "mp4",
        "video/quicktime" => "mov",
        "video/x-ms-wmv" => "wmv",
        "video/x-msvideo" => "avi",
        "video/webm" => "webm",
        "video/x-matroska" => "mkv",
        "audio/mpeg" => "mp3",
        "audio/mp4" => "m4a",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/x-ms-wma" => "wma",
        "audio/ogg" => "ogg",
        _ => "bin",
    }
}

/// A graphic frame: a table, a chart, a diagram or an embedded object.
fn read_frame(n: &Node, cx: &mut PartCx) -> Option<Object> {
    let nv = n.child("nvGraphicFramePr")?;
    let data = n.child("graphic").and_then(|g| g.child("graphicData"));
    let uri = data.and_then(|d| d.attr("uri")).unwrap_or("");
    let frame = n.child("xfrm").map(read_xfrm).unwrap_or_default();
    let kind = if uri == URI_TABLE {
        ObjKind::Table(Box::new(read_table(data?.child("tbl")?, cx)))
    } else if uri == URI_CHART {
        let id = data?.child("chart").and_then(|c| c.attr("r:id"))?.to_string();
        match read_chart(cx, &id, (frame.w, frame.h)) {
            Some(c) => ObjKind::Chart(c),
            None => ObjKind::Other { preview: None, what: Arc::from("chart"), xml: None },
        }
    } else if uri == URI_DIAGRAM {
        if cx.diagrams.is_none() {
            cx.diagrams = Some(crate::diagram::Parts::load(cx.z, &cx.path));
        }
        match cx.diagrams.as_ref().and_then(|p| p.diagram(data?, (frame.w, frame.h))) {
            Some(d) => ObjKind::Diagram(Arc::new(d)),
            None => ObjKind::Other { preview: None, what: Arc::from("diagram"), xml: None },
        }
    } else {
        // An embedded object: its preview picture.
        let mut blips = Vec::new();
        n.find_all("blip", &mut blips);
        let preview = blips.iter().find_map(|b| b.attr("r:embed")).and_then(|id| cx.media.get(id)).cloned();
        let what = if n.find("oleObj").is_some() { "ole" } else { "object" };
        ObjKind::Other { preview, what: Arc::from(what), xml: None }
    };
    let mut o = Object::new(0, "", kind);
    read_nv(&mut o, nv, cx);
    o.sp.frame = Some(frame);
    Some(o)
}

fn read_chart(cx: &PartCx, id: &str, size: (f32, f32)) -> Option<apex_doc::ChartObject> {
    let (path, xml) = cx.target(id)?;
    let xml = String::from_utf8_lossy(&xml).into_owned();
    let embedded = crate::ooxml_keep::read_rels(cx.z, &path).into_iter().find(|r| !r.3 && r.1 == REL_PACKAGE).and_then(|r| cx.z.read(&r.2).ok());
    let book = crate::chart_object::from_docx(&xml, embedded.as_deref(), size)?;
    Some(apex_doc::ChartObject { book })
}

fn read_table(t: &Node, cx: &PartCx) -> Table {
    let mut table = Table::default();
    if let Some(p) = t.child("tblPr") {
        let f = |k: &str| p.attr(k).is_some_and(|v| v == "1" || v == "true");
        table.look = TableLook { first_row: f("firstRow"), last_row: f("lastRow"), first_col: f("firstCol"), last_col: f("lastCol"), band_rows: f("bandRow"), band_cols: f("bandCol") };
        table.style = p.child("tableStyleId").map(|s| Arc::from(s.text.trim()));
    }
    if let Some(g) = t.child("tblGrid") {
        table.cols = g.children("gridCol").take(apex_slides::table::MAX_COLS).map(|c| emu(c, "w")).collect();
    }
    let tcx = cx.text_cx();
    let width = table.cols.len().max(1);
    for tr in t.children("tr") {
        // Each row is made as wide as the grid.
        let Some(left) = cx.cells_left.get().checked_sub(width) else { break };
        cx.cells_left.set(left);
        let mut row = Row { height: emu(tr, "h"), cells: Vec::new() };
        for tc in tr.children("tc").take(width) {
            let mut c = Cell::new("");
            if let Some(b) = tc.child("txBody") {
                c.text = text::read_text_body(b, &tcx);
            }
            c.span = tc.num("gridSpan").unwrap_or(1.0).max(1.0) as u32;
            c.rspan = tc.num("rowSpan").unwrap_or(1.0).max(1.0) as u32;
            c.h_merge = tc.attr("hMerge").is_some_and(|v| v == "1" || v == "true");
            c.v_merge = tc.attr("vMerge").is_some_and(|v| v == "1" || v == "true");
            if let Some(p) = tc.child("tcPr") {
                c.margins = ["marL", "marR", "marT", "marB"].map(|k| p.num(k).map(|v| v as f32 / EMU_PT));
                c.anchor = p.attr("anchor").map(|a| match a {
                    "ctr" => VAnchor::Middle,
                    "b" => VAnchor::Bottom,
                    _ => VAnchor::Top,
                });
                for (i, k) in ["lnL", "lnR", "lnT", "lnB"].iter().enumerate() {
                    c.borders[i] = p.child(k).map(|l| fill::read_line(l, &cx.media));
                }
                c.fill = fill::read_fill(p, &cx.media);
            }
            row.cells.push(c);
        }
        table.rows.push(row);
    }
    table.normalize();
    table
}

/// What writing a part's objects needs: pictures, links, charts and
/// media get relationship ids from it.
pub(crate) trait PartOut: Images + Links {
    /// A chart part for the chart; its relationship id.
    fn chart(&mut self, c: &apex_doc::ChartObject) -> Option<String>;
    /// A diagram's parts; the `dgm:relIds` element.
    fn diagram(&mut self, d: &apex_doc::Diagram) -> String;
    /// A video or sound: (link id, media id).
    fn media(&mut self, data: &Arc<[u8]>, mime: &str, video: bool) -> (String, String);
    /// An external video or sound file.
    fn media_link(&mut self, target: &str, video: bool) -> String;
}

/// Writes objects as `p:spTree` children.
pub(crate) fn write_tree(out: &mut String, objs: &[Object], w: &mut dyn PartOut) {
    for o in objs {
        write_object(out, o, w);
    }
}

pub(crate) fn write_object(out: &mut String, o: &Object, w: &mut dyn PartOut) {
    if let Some(r) = o.raw.as_ref().filter(|r| r.src == Src::Pptx) {
        out.push_str(&r.xml);
        return;
    }
    match &o.kind {
        ObjKind::Shape { text, text_box, connector } => {
            let (tag, nv) = if *connector { ("p:cxnSp", "p:nvCxnSpPr") } else { ("p:sp", "p:nvSpPr") };
            let _ = write!(out, "<{tag}><{nv}>");
            write_cnv_pr(out, o, w);
            if *connector {
                out.push_str("<p:cNvCxnSpPr/>");
            } else if *text_box {
                out.push_str(r#"<p:cNvSpPr txBox="1"/>"#);
            } else if o.ph.is_some() {
                out.push_str(r#"<p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr>"#);
            } else {
                out.push_str("<p:cNvSpPr/>");
            }
            write_nv_pr(out, o, "");
            let _ = write!(out, "</{nv}>");
            write_sp_pr(out, "p:spPr", &o.sp, w);
            if let Some(s) = &o.style {
                fill::write_style(out, "p:style", s);
            }
            if let Some(t) = text.as_ref().filter(|_| !*connector) {
                text::write_text_body(out, "p:txBody", t, w);
            }
            let _ = write!(out, "</{tag}>");
        }
        ObjKind::Picture(p) => {
            out.push_str("<p:pic><p:nvPicPr>");
            write_cnv_pr(out, o, w);
            out.push_str(r#"<p:cNvPicPr><a:picLocks noChangeAspect="1"/></p:cNvPicPr>"#);
            let mut extra = String::new();
            if let Some(m) = &p.media {
                let tag = if m.video { "videoFile" } else { "audioFile" };
                match (&m.data, &m.link) {
                    (Some(d), _) => {
                        let (link, emb) = w.media(d, &m.mime, m.video);
                        let trim = if m.trim.0 > 0.0 || m.trim.1 > 0.0 {
                            format!(r#"<p14:trim st="{}" end="{}"/>"#, (m.trim.0.max(0.0) * 1000.0).round() as i64, (m.trim.1.max(0.0) * 1000.0).round() as i64)
                        } else {
                            String::new()
                        };
                        let _ = write!(
                            extra,
                            r#"<a:{tag} r:link="{link}"/><p:extLst><p:ext uri="{{DAA4B4D4-6D71-4841-9C94-3DE7FCFB9230}}"><p14:media xmlns:p14="http://schemas.microsoft.com/office/powerpoint/2010/main" r:embed="{emb}">{trim}</p14:media></p:ext></p:extLst>"#
                        );
                    }
                    (None, Some(l)) => {
                        let link = w.media_link(l, m.video);
                        let _ = write!(extra, r#"<a:{tag} r:link="{link}"/>"#);
                    }
                    _ => {}
                }
            }
            write_nv_pr(out, o, &extra);
            out.push_str("</p:nvPicPr><p:blipFill>");
            let id = w.image(&p.image.data);
            let _ = write!(out, r#"<a:blip r:embed="{id}"/>"#);
            if p.image.crop != [0.0; 4] {
                let [l, t, r, b] = p.image.crop.map(|v| (v * 100_000.0).round() as i64);
                let _ = write!(out, r#"<a:srcRect l="{l}" t="{t}" r="{r}" b="{b}"/>"#);
            }
            out.push_str("<a:stretch><a:fillRect/></a:stretch></p:blipFill>");
            let mut sp = o.sp.clone();
            if sp.geom.is_none() {
                sp.geom = Some(Geometry::preset("rect"));
            }
            write_sp_pr(out, "p:spPr", &sp, w);
            if let Some(s) = &o.style {
                fill::write_style(out, "p:style", s);
            }
            out.push_str("</p:pic>");
        }
        ObjKind::Group(g) => {
            out.push_str("<p:grpSp><p:nvGrpSpPr>");
            write_cnv_pr(out, o, w);
            out.push_str("<p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr>");
            let f = o.sp.frame.unwrap_or_default();
            write_xfrm(out, "a:xfrm", &f, Some(&g.child));
            if let Some(fl) = &o.sp.fill {
                fill::write_fill(out, fl, w);
            }
            out.push_str("</p:grpSpPr>");
            write_tree(out, &g.children, w);
            out.push_str("</p:grpSp>");
        }
        ObjKind::Table(t) => {
            frame_start(out, o, w);
            let _ = write!(out, r#"<a:graphic><a:graphicData uri="{URI_TABLE}">"#);
            write_table(out, t, w);
            out.push_str("</a:graphicData></a:graphic></p:graphicFrame>");
        }
        ObjKind::Chart(c) => match w.chart(c) {
            Some(id) => {
                frame_start(out, o, w);
                let _ = write!(out, r#"<a:graphic><a:graphicData uri="{URI_CHART}"><c:chart xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" r:id="{id}"/></a:graphicData></a:graphic></p:graphicFrame>"#);
            }
            None => write_placeholder_box(out, o, w, "Chart"),
        },
        ObjKind::Diagram(d) => {
            let ids = w.diagram(d);
            frame_start(out, o, w);
            let _ = write!(out, r#"<a:graphic><a:graphicData uri="{URI_DIAGRAM}">{ids}</a:graphicData></a:graphic></p:graphicFrame>"#);
        }
        ObjKind::Math(eq) => {
            // Office math in a text box, and its linear form as text for
            // readers without it.
            let mut b = Object::text_box(o.id, o.sp.frame.unwrap_or_default(), apex_slides::TextBody::plain(""));
            b.name = o.name.clone();
            let mut body = String::new();
            write_object(&mut body, &b, w);
            let rpr = r#"<a:rPr lang="en-US" i="1"><a:latin typeface="Cambria Math"/></a:rPr>"#;
            let mut math = String::from(r#"<a14:m xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math">"#);
            crate::omml::write_equation(&mut math, eq, rpr, true);
            math.push_str("</a14:m>");
            let choice = match body.rfind("<a:p>").or_else(|| body.rfind("<a:p/>")) {
                Some(at) => {
                    let end = body[at..].find("</a:p>").map(|e| at + e + 6).or_else(|| body[at..].starts_with("<a:p/>").then_some(at + 6)).unwrap_or(body.len());
                    format!("{}<a:p>{math}</a:p>{}", &body[..at], &body[end..])
                }
                None => body.clone(),
            };
            out.push_str(r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice xmlns:a14="http://schemas.microsoft.com/office/drawing/2010/main" Requires="a14">"#);
            out.push_str(&choice);
            out.push_str("</mc:Choice><mc:Fallback>");
            write_placeholder_box(out, o, w, eq.source());
            out.push_str("</mc:Fallback></mc:AlternateContent>");
        }
        ObjKind::Other { xml: Some(r), .. } if r.src == Src::Pptx => out.push_str(&moved(&r.xml, o)),
        ObjKind::Other { preview, .. } => match preview {
            Some(p) => {
                let mut pic = o.clone();
                pic.kind = ObjKind::Picture(Box::new(Picture { image: apex_doc::Image::new(p.clone(), 0.0, 0.0), media: None }));
                write_object(out, &pic, w);
            }
            None => write_placeholder_box(out, o, w, ""),
        },
    }
}

/// The XML of an object read with its place (first `xfrm`) and name
/// changed to the object's.
fn moved(xml: &str, o: &Object) -> String {
    let mut out = xml.to_string();
    if let Some(f) = &o.sp.frame {
        for tag in ["p:xfrm", "a:xfrm"] {
            let Some(a) = out.find(&format!("<{tag}")).filter(|&a| out[a + tag.len() + 1..].starts_with([' ', '>', '/'])) else { continue };
            let Some(he) = out[a..].find('>').map(|e| a + e + 1) else { break };
            let end = if out[..he].ends_with("/>") { he } else { out[he..].find(&format!("</{tag}>")).map_or(he, |e| he + e + tag.len() + 3) };
            let mut x = String::new();
            write_xfrm(&mut x, tag, f, None);
            out.replace_range(a..end, &x);
            break;
        }
    }
    out
}

/// An object we can't write as itself: a text box in its place.
fn write_placeholder_box(out: &mut String, o: &Object, w: &mut dyn PartOut, s: &str) {
    let mut b = Object::text_box(o.id, o.sp.frame.unwrap_or_default(), apex_slides::TextBody::plain(s));
    b.name = o.name.clone();
    if s.is_empty() {
        b.kind = ObjKind::Shape { text: None, text_box: false, connector: false };
    }
    write_object(out, &b, w);
}

fn frame_start(out: &mut String, o: &Object, w: &mut dyn PartOut) {
    out.push_str("<p:graphicFrame><p:nvGraphicFramePr>");
    write_cnv_pr(out, o, w);
    out.push_str(r#"<p:cNvGraphicFramePr><a:graphicFrameLocks noGrp="1"/></p:cNvGraphicFramePr>"#);
    write_nv_pr(out, o, "");
    out.push_str("</p:nvGraphicFramePr>");
    write_xfrm(out, "p:xfrm", &o.sp.frame.unwrap_or_default(), None);
}

fn write_table(out: &mut String, t: &Table, w: &mut dyn PartOut) {
    let l = &t.look;
    out.push_str("<a:tbl><a:tblPr");
    for (k, v) in [("firstRow", l.first_row), ("firstCol", l.first_col), ("lastRow", l.last_row), ("lastCol", l.last_col), ("bandRow", l.band_rows), ("bandCol", l.band_cols)] {
        if v {
            let _ = write!(out, r#" {k}="1""#);
        }
    }
    match &t.style {
        Some(s) => {
            let _ = write!(out, "><a:tableStyleId>{}</a:tableStyleId></a:tblPr>", esc(s));
        }
        None => out.push_str("/>"),
    }
    out.push_str("<a:tblGrid>");
    for c in &t.cols {
        let _ = write!(out, r#"<a:gridCol w="{}"/>"#, e(*c));
    }
    out.push_str("</a:tblGrid>");
    for r in &t.rows {
        let _ = write!(out, r#"<a:tr h="{}">"#, e(r.height));
        for c in &r.cells {
            out.push_str("<a:tc");
            if c.span > 1 {
                let _ = write!(out, r#" gridSpan="{}""#, c.span);
            }
            if c.rspan > 1 {
                let _ = write!(out, r#" rowSpan="{}""#, c.rspan);
            }
            if c.h_merge {
                out.push_str(r#" hMerge="1""#);
            }
            if c.v_merge {
                out.push_str(r#" vMerge="1""#);
            }
            out.push('>');
            text::write_text_body(out, "a:txBody", &c.text, w);
            out.push_str("<a:tcPr");
            for (k, v) in ["marL", "marR", "marT", "marB"].iter().zip(c.margins) {
                if let Some(v) = v {
                    let _ = write!(out, r#" {k}="{}""#, e(v));
                }
            }
            if let Some(a) = c.anchor {
                let _ = write!(out, r#" anchor="{}""#, match a {
                    VAnchor::Top => "t",
                    VAnchor::Middle => "ctr",
                    VAnchor::Bottom => "b",
                });
            }
            out.push('>');
            for (i, k) in ["lnL", "lnR", "lnT", "lnB"].iter().enumerate() {
                if let Some(l) = &c.borders[i] {
                    let mut s = String::new();
                    fill::write_line(&mut s, l, w);
                    out.push_str(&s.replacen("<a:ln", &format!("<a:{k}"), 1).replace("</a:ln>", &format!("</a:{k}>")));
                }
            }
            if let Some(f) = &c.fill {
                fill::write_fill(out, f, w);
            }
            out.push_str("</a:tcPr></a:tc>");
        }
        out.push_str("</a:tr>");
    }
    out.push_str("</a:tbl>");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_xml_round_trip() {
        let src = r#"<a:custGeom><a:pathLst><a:path w="10" h="10"><a:moveTo><a:pt x="0" y="0"/></a:moveTo><a:lnTo><a:pt x="10" y="10"/></a:lnTo></a:path></a:pathLst></a:custGeom>"#;
        let n = crate::drawingml::read_fragment(src);
        assert_eq!(node_string(&n.kids[0]), src);
    }
}
