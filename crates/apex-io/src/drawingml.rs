//! DrawingML shapes, shared by .xlsx (`xdr:sp`) and .docx (`wps:wsp`):
//! a small element tree to read them, their look (`spPr` and `style`),
//! their text (`txBody`, or a Word text box's paragraphs) and writing it
//! all back.

use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{Align, Dash, Shape, ShapeKind, ShapePara, ShapeRun, VAnchor};

use crate::xml::{Ev, Reader, Tag, esc};

/// EMU per point.
pub const EMU_PT: f32 = 12700.0;
/// Elements read into one tree at most (hostile files).
const MAX_NODES: usize = 200_000;
const MAX_DEPTH: usize = 64;
/// Name of the text pieces among an element's children.
pub const TEXT: &str = "#text";

/// An element and what's in it.
#[derive(Clone, Debug, Default)]
pub struct Node {
    /// Qualified name.
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub kids: Vec<Node>,
    /// Text directly inside.
    pub text: String,
}

impl Node {
    pub fn local(&self) -> &str {
        crate::xml::local(&self.name)
    }

    /// An attribute by qualified name, or by local name.
    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == key).or_else(|| self.attrs.iter().find(|(k, _)| crate::xml::local(k) == key)).map(|(_, v)| v.as_str())
    }

    pub fn child(&self, local: &str) -> Option<&Node> {
        self.kids.iter().find(|k| k.local() == local)
    }

    pub fn children<'a>(&'a self, local: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.kids.iter().filter(move |k| k.local() == local)
    }

    /// The first element named `local` in the tree (depth first).
    pub fn find(&self, local: &str) -> Option<&Node> {
        for k in &self.kids {
            if k.local() == local {
                return Some(k);
            }
            if let Some(n) = k.find(local) {
                return Some(n);
            }
        }
        None
    }

    /// Every element named `local` in the tree, in document order, not
    /// looking inside the ones found.
    pub fn find_all<'a>(&'a self, local: &str, out: &mut Vec<&'a Node>) {
        for k in &self.kids {
            if k.local() == local {
                out.push(k);
            } else {
                k.find_all(local, out);
            }
        }
    }

    pub(crate) fn num(&self, key: &str) -> Option<f64> {
        self.attr(key)?.trim().parse::<f64>().ok().filter(|v| v.is_finite())
    }

    fn flag(&self, key: &str) -> Option<bool> {
        self.attr(key).map(|v| matches!(v.trim(), "1" | "true" | "on"))
    }
}

fn node_of(t: &Tag) -> Node {
    Node { name: t.name.to_string(), attrs: t.attrs().map(|(k, v)| (k.to_string(), v.into_owned())).collect(), ..Default::default() }
}

/// Reads the element whose start tag `t` was just returned (`empty`: it
/// was self-closing).
pub fn read_node(r: &mut Reader, t: &Tag, empty: bool) -> Node {
    read_node_within(r, t, empty, &mut 0)
}

/// [`read_node`] with the nodes read so far (`count`) shared with others:
/// at most `MAX_NODES` in all.
pub fn read_node_within(r: &mut Reader, t: &Tag, empty: bool, count: &mut usize) -> Node {
    let mut n = node_of(t);
    if !empty {
        read_kids(r, &mut n, 1, count);
    }
    n
}

/// Reads a piece of XML (such as [`Reader::raw_element`] returns) into a
/// root holding its top elements.
pub fn read_fragment(src: &str) -> Node {
    let mut r = Reader::new(src);
    let mut root = Node::default();
    let mut count = 0;
    read_kids(&mut r, &mut root, 1, &mut count);
    root
}

fn read_kids(r: &mut Reader, n: &mut Node, depth: usize, count: &mut usize) {
    loop {
        match r.next() {
            Ev::Eof | Ev::End(_) => return,
            Ev::Text(t) => {
                if n.text.len() < 1 << 20 {
                    n.text.push_str(&t);
                    // Also in order among the elements (mixed content).
                    *count += 1;
                    if *count < MAX_NODES {
                        n.kids.push(Node { name: TEXT.into(), text: t.into_owned(), ..Default::default() });
                    }
                }
            }
            Ev::Empty(t) => {
                *count += 1;
                if *count < MAX_NODES {
                    n.kids.push(node_of(&t));
                }
            }
            Ev::Start(t) => {
                *count += 1;
                if *count >= MAX_NODES || depth >= MAX_DEPTH {
                    r.skip();
                    continue;
                }
                let mut k = node_of(&t);
                read_kids(r, &mut k, depth + 1, count);
                n.kids.push(k);
            }
        }
    }
}

// --- colors ------------------------------------------------------------------------------

fn scheme(name: &str) -> u32 {
    match name {
        "accent1" => 0x4472C4,
        "accent2" => 0xED7D31,
        "accent3" => 0xA5A5A5,
        "accent4" => 0xFFC000,
        "accent5" => 0x5B9BD5,
        "accent6" => 0x70AD47,
        "dk2" | "tx2" => 0x44546A,
        "lt2" | "bg2" => 0xE7E6E6,
        "lt1" | "bg1" => 0xFFFFFF,
        "hlink" => 0x0563C1,
        "folHlink" => 0x954F72,
        _ => 0x000000,
    }
}

fn preset_color(name: &str) -> u32 {
    match name {
        "white" => 0xFFFFFF,
        "red" => 0xFF0000,
        "green" => 0x008000,
        "blue" => 0x0000FF,
        "yellow" => 0xFFFF00,
        "gray" | "grey" => 0x808080,
        "orange" => 0xFFA500,
        _ => 0x000000,
    }
}

/// Applies DrawingML color changes (luminance, shade, tint) roughly.
fn modified(c: u32, n: &Node) -> u32 {
    let mut rgb = [((c >> 16) & 0xFF) as f32 / 255.0, ((c >> 8) & 0xFF) as f32 / 255.0, (c & 0xFF) as f32 / 255.0];
    for m in &n.kids {
        let v = m.num("val").map_or(1.0, |v| (v / 100_000.0) as f32).clamp(-1.0, 10.0);
        match m.local() {
            "lumMod" => rgb = rgb.map(|x| x * v),
            "lumOff" => rgb = rgb.map(|x| x + v),
            "shade" => rgb = rgb.map(|x| x * v),
            "tint" => rgb = rgb.map(|x| x + (1.0 - x) * (1.0 - v)),
            _ => {}
        }
    }
    let b = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u32;
    (b(rgb[0]) << 16) | (b(rgb[1]) << 8) | b(rgb[2])
}

/// The color in a fill or reference element (its first color child).
pub fn color(n: &Node) -> Option<u32> {
    for k in &n.kids {
        let c = match k.local() {
            "srgbClr" => u32::from_str_radix(k.attr("val")?.trim(), 16).ok().filter(|v| *v <= 0xFFFFFF)?,
            "schemeClr" => scheme(k.attr("val").unwrap_or("")),
            "sysClr" => k.attr("lastClr").and_then(|v| u32::from_str_radix(v.trim(), 16).ok()).filter(|v| *v <= 0xFFFFFF).unwrap_or(if k.attr("val") == Some("window") { 0xFFFFFF } else { 0 }),
            "prstClr" => preset_color(k.attr("val").unwrap_or("")),
            "scrgbClr" => {
                let p = |key| k.num(key).map_or(0, |v| ((v / 100_000.0).clamp(0.0, 1.0) * 255.0).round() as u32);
                (p("r") << 16) | (p("g") << 8) | p("b")
            }
            _ => continue,
        };
        return Some(modified(c, k));
    }
    None
}

/// Fill of an element holding a `*Fill`: Some(None) for no fill.
fn fill_of(n: &Node) -> Option<Option<u32>> {
    for k in &n.kids {
        match k.local() {
            "noFill" => return Some(None),
            "solidFill" => return Some(color(k)),
            "gradFill" => return Some(k.find("gs").and_then(color)),
            "pattFill" => return Some(k.child("fgClr").and_then(color)),
            "blipFill" | "grpFill" => return Some(Some(0xFFFFFF)),
            _ => {}
        }
    }
    None
}

// --- reading -----------------------------------------------------------------------------

/// The look of a shape from its `spPr` and `style`; `text_box`: it is one.
pub fn read_look(sp_pr: Option<&Node>, style: Option<&Node>, text_box: bool) -> Shape {
    let geom = sp_pr.and_then(|n| n.child("prstGeom")).and_then(|g| g.attr("prst")).unwrap_or("rect");
    let (mut kind, exact) = ShapeKind::from_preset(geom);
    if text_box && kind == ShapeKind::Rect {
        kind = ShapeKind::TextBox;
    }
    let mut s = Shape::new(kind);
    if !exact {
        s.preset = Some(Arc::from(geom));
    }
    if let Some(av) = sp_pr.and_then(|n| n.child("prstGeom")).and_then(|g| g.child("avLst")) {
        for gd in av.children("gd") {
            let v = gd.attr("fmla").and_then(|f| f.strip_prefix("val ")).and_then(|v| v.trim().parse::<i64>().ok());
            if let (Some(n), Some(v)) = (gd.attr("name"), v) {
                s.adj.push((Arc::from(n), v));
            }
        }
    }
    let style_ref = |key: &str| style.and_then(|st| st.child(key)).filter(|r| r.num("idx").is_some_and(|i| i > 0.0));
    s.fill = match sp_pr.and_then(fill_of) {
        Some(f) => f,
        None => style_ref("fillRef").and_then(color),
    };
    if let Some(g) = sp_pr.and_then(|n| n.child("gradFill")).and_then(crate::dml_fill::read) {
        s.fill = Some(g.main_color());
        s.gradient = Some(g);
    }
    if kind.is_line() {
        s.fill = None;
    }
    let ln = sp_pr.and_then(|n| n.child("ln"));
    let ln_style = style_ref("lnRef");
    s.line = match ln.and_then(fill_of) {
        Some(f) => f,
        None => ln_style.and_then(color),
    };
    s.line_width = match ln.and_then(|l| l.num("w")) {
        Some(w) => (w as f32 / EMU_PT).max(0.0),
        // The theme's line widths.
        None => match ln_style.and_then(|r| r.num("idx")) {
            Some(i) if i >= 3.0 => 1.5,
            Some(i) if i >= 2.0 => 1.0,
            _ => 0.75,
        },
    };
    if let Some(ln) = ln {
        s.dash = ln.child("prstDash").and_then(|d| d.attr("val")).map_or(Dash::Solid, Dash::from_preset);
        let arrow = |key: &str| ln.child(key).and_then(|e| e.attr("type")).is_some_and(|t| t != "none");
        s.arrow_start = arrow("headEnd");
        s.arrow_end = arrow("tailEnd");
    }
    if let Some(x) = sp_pr.and_then(|n| n.child("xfrm")) {
        s.rotation = x.num("rot").map_or(0.0, |v| (v / 60_000.0) as f32);
        s.flip_h = x.flag("flipH").unwrap_or(false);
        s.flip_v = x.flag("flipV").unwrap_or(false);
    }
    s.sanitize();
    s
}

/// Size (EMU converted by `unit`) in a `spPr`'s `xfrm`.
pub fn xfrm_ext(sp_pr: Option<&Node>, unit: f32) -> Option<(f32, f32)> {
    let e = sp_pr?.child("xfrm")?.child("ext")?;
    Some(((e.num("cx")? as f32 / unit).max(0.0), (e.num("cy")? as f32 / unit).max(0.0)))
}

/// The text color the style gives, when it isn't the automatic one.
fn style_text_color(style: Option<&Node>, s: &Shape) -> Option<u32> {
    let c = color(style?.child("fontRef")?)?;
    (c != s.auto_text_color()).then_some(c)
}

fn valign(body_pr: Option<&Node>, s: &mut Shape) {
    // Vertical text is kept, not shown yet.
    s.vert = body_pr.and_then(|b| b.attr("vert")).filter(|v| *v != "horz").map(Arc::from);
    if let Some(a) = body_pr.and_then(|b| b.attr("anchor")) {
        s.valign = match a {
            "ctr" => VAnchor::Middle,
            "b" => VAnchor::Bottom,
            _ => VAnchor::Top,
        };
    }
}

fn font_name(v: Option<&str>) -> Option<Arc<str>> {
    // Theme fonts ("+mn-lt") are the default.
    v.filter(|f| !f.is_empty() && !f.starts_with('+')).map(Arc::from)
}

fn finish_text(s: &mut Shape, paras: Vec<ShapePara>, style: Option<&Node>) {
    if paras.iter().all(|p| p.runs.iter().all(|r| r.text.is_empty())) {
        s.text.clear();
        return;
    }
    s.text = paras;
    if let Some(c) = style_text_color(style, s) {
        s.format_text(&|r| {
            r.color.get_or_insert(c);
        });
    }
}

/// The text of a `txBody` (spreadsheets).
pub fn read_tx_body(body: &Node, style: Option<&Node>, s: &mut Shape) {
    valign(body.child("bodyPr"), s);
    let mut paras = Vec::new();
    for p in body.children("p") {
        let align = match p.child("pPr").and_then(|x| x.attr("algn")) {
            Some("ctr") => Align::Center,
            Some("r") => Align::Right,
            Some("just") | Some("dist") => Align::Justify,
            Some(_) => Align::Left,
            None => s.text_align(),
        };
        let mut para = ShapePara { align, runs: Vec::new() };
        for r in &p.kids {
            match r.local() {
                "r" | "fld" => {
                    let mut run = ShapeRun::new(r.child("t").map_or(String::new(), |t| t.text.clone()));
                    if let Some(pr) = r.child("rPr") {
                        run.bold = pr.flag("b").unwrap_or(false);
                        run.italic = pr.flag("i").unwrap_or(false);
                        run.underline = pr.attr("u").is_some_and(|u| u != "none");
                        run.size = pr.num("sz").map(|v| v as f32 / 100.0);
                        run.color = pr.child("solidFill").and_then(color);
                        run.font = font_name(pr.child("latin").and_then(|l| l.attr("typeface")));
                        run.fx = crate::text_fx_xml::read_all(pr);
                    }
                    para.runs.push(run);
                }
                "br" => {
                    let next = ShapePara { align, runs: vec![para.first_run().with_text("")] };
                    paras.push(std::mem::replace(&mut para, next));
                }
                _ => {}
            }
        }
        if para.runs.is_empty() {
            // Keep the size of an empty paragraph.
            let mut run = ShapeRun::new("");
            run.size = p.child("endParaRPr").and_then(|e| e.num("sz")).map(|v| v as f32 / 100.0);
            para.runs.push(run);
        }
        paras.push(para);
    }
    finish_text(s, paras, style);
}

/// The paragraphs of a Word text box (`w:txbxContent`).
pub fn read_txbx(content: &Node, body_pr: Option<&Node>, style: Option<&Node>, s: &mut Shape) {
    valign(body_pr, s);
    let mut ps = Vec::new();
    content.find_all("p", &mut ps);
    let mut paras = Vec::new();
    for p in ps {
        let ppr = p.child("pPr");
        let align = match ppr.and_then(|x| x.child("jc")).and_then(|j| j.attr("val")) {
            Some("center") => Align::Center,
            Some("right") | Some("end") => Align::Right,
            Some("both") | Some("distribute") => Align::Justify,
            _ => Align::Left,
        };
        let mut para = ShapePara { align, runs: Vec::new() };
        let mut runs = Vec::new();
        p.find_all("r", &mut runs);
        for r in runs {
            let mut run = ShapeRun::default();
            if let Some(pr) = r.child("rPr") {
                let on = |k: &str| pr.child(k).map(|b| b.attr("val").is_none_or(|v| !matches!(v, "0" | "false" | "off")));
                run.bold = on("b").unwrap_or(false);
                run.italic = on("i").unwrap_or(false);
                run.underline = pr.child("u").and_then(|u| u.attr("val")).is_some_and(|v| v != "none");
                run.size = pr.child("sz").and_then(|z| z.num("val")).map(|v| v as f32 / 2.0).filter(|v| *v != apex_doc::SHAPE_TEXT_PT);
                run.color = pr.child("color").and_then(|c| c.attr("val")).and_then(|v| u32::from_str_radix(v.trim(), 16).ok()).filter(|v| *v <= 0xFFFFFF);
                run.font = font_name(pr.child("rFonts").and_then(|f| f.attr("ascii").or_else(|| f.attr("hAnsi"))));
                run.fx = crate::text_fx_xml::read_all(pr);
            }
            for k in &r.kids {
                match k.local() {
                    "t" => run.text.push_str(&k.text),
                    "tab" => run.text.push('\t'),
                    "br" | "cr" => {
                        let text = std::mem::take(&mut run.text);
                        let before = run.with_text(text);
                        para.runs.push(before);
                        let next = ShapePara { align, runs: Vec::new() };
                        paras.push(std::mem::replace(&mut para, next));
                    }
                    _ => {}
                }
            }
            para.runs.push(run);
        }
        if para.runs.is_empty() {
            let mut run = ShapeRun::new("");
            run.size = ppr.and_then(|x| x.child("rPr")).and_then(|x| x.child("sz")).and_then(|z| z.num("val")).map(|v| v as f32 / 2.0);
            para.runs.push(run);
        }
        paras.push(para);
    }
    finish_text(s, paras, style);
}

// --- writing -----------------------------------------------------------------------------

fn hex(c: u32) -> String {
    format!("{:06X}", c & 0xFFFFFF)
}

fn solid(out: &mut String, c: Option<u32>) {
    match c {
        Some(c) => {
            let _ = write!(out, r#"<a:solidFill><a:srgbClr val="{}"/></a:solidFill>"#, hex(c));
        }
        None => out.push_str("<a:noFill/>"),
    }
}

/// The inside of a `spPr`: the box (`off`, `ext` in EMU), geometry, fill
/// and outline.
pub fn write_sp_pr(out: &mut String, s: &Shape, off: (i64, i64), ext: (i64, i64)) {
    out.push_str("<a:xfrm");
    let rot = (s.rotation * 60_000.0).round() as i64;
    if rot != 0 {
        let _ = write!(out, r#" rot="{rot}""#);
    }
    if s.flip_h {
        out.push_str(r#" flipH="1""#);
    }
    if s.flip_v {
        out.push_str(r#" flipV="1""#);
    }
    let _ = write!(out, r#"><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/></a:xfrm>"#, off.0, off.1, ext.0.max(0), ext.1.max(0));
    let preset = s.preset.as_deref().unwrap_or(s.kind.preset());
    let _ = write!(out, r#"<a:prstGeom prst="{}">"#, esc(preset));
    if s.adj.is_empty() {
        out.push_str("<a:avLst/>");
    } else {
        out.push_str("<a:avLst>");
        for (n, v) in &s.adj {
            let _ = write!(out, r#"<a:gd name="{}" fmla="val {v}"/>"#, esc(n));
        }
        out.push_str("</a:avLst>");
    }
    out.push_str("</a:prstGeom>");
    if !s.kind.is_line() {
        match (&s.gradient, s.fill) {
            (Some(g), Some(_)) => crate::dml_fill::write(out, "a", g),
            _ => solid(out, s.fill),
        }
    }
    match s.line {
        Some(c) => {
            let _ = write!(out, r#"<a:ln w="{}">"#, (s.line_width * EMU_PT).round() as i64);
            solid(out, Some(c));
            if s.dash != Dash::Solid {
                let _ = write!(out, r#"<a:prstDash val="{}"/>"#, s.dash.preset());
            }
            if s.arrow_start {
                out.push_str(r#"<a:headEnd type="triangle"/>"#);
            }
            if s.arrow_end {
                out.push_str(r#"<a:tailEnd type="triangle"/>"#);
            }
            out.push_str("</a:ln>");
        }
        None => {
            let _ = write!(out, r#"<a:ln w="{}"><a:noFill/></a:ln>"#, (s.line_width * EMU_PT).round() as i64);
        }
    }
}

/// A `style` element (tag `tag`) that only gives the automatic text color:
/// Office's shapes default to white text, text boxes to black.
pub fn write_style(out: &mut String, tag: &str, s: &Shape) {
    let _ = write!(
        out,
        r#"<{tag}><a:lnRef idx="0"><a:srgbClr val="000000"/></a:lnRef><a:fillRef idx="0"><a:srgbClr val="000000"/></a:fillRef><a:effectRef idx="0"><a:srgbClr val="000000"/></a:effectRef><a:fontRef idx="minor"><a:srgbClr val="{}"/></a:fontRef></{tag}>"#,
        hex(s.auto_text_color())
    );
}

pub fn anchor_value(v: VAnchor) -> &'static str {
    match v {
        VAnchor::Top => "t",
        VAnchor::Middle => "ctr",
        VAnchor::Bottom => "b",
    }
}

/// A `txBody`'s paragraphs (spreadsheets).
pub fn write_paras(out: &mut String, s: &Shape) {
    let paras: Vec<ShapePara> = if s.text.is_empty() { vec![ShapePara { align: s.text_align(), runs: Vec::new() }] } else { s.text.clone() };
    for p in &paras {
        let algn = match p.align {
            Align::Center => "ctr",
            Align::Right => "r",
            Align::Justify => "just",
            Align::Left => "l",
        };
        let _ = write!(out, r#"<a:p><a:pPr algn="{algn}"/>"#);
        for r in p.runs.iter().filter(|r| !r.text.is_empty()) {
            out.push_str("<a:r>");
            run_pr(out, "a:rPr", r);
            let _ = write!(out, "<a:t>{}</a:t></a:r>", esc(&r.text));
        }
        run_pr(out, "a:endParaRPr", &p.first_run());
        out.push_str("</a:p>");
    }
}

fn run_pr(out: &mut String, tag: &str, r: &ShapeRun) {
    match &r.fx {
        Some(fx) => {
            let mut s = String::new();
            plain_run_pr(&mut s, tag, r);
            out.push_str(&crate::text_fx_xml::into_dml(&s, fx));
        }
        None => plain_run_pr(out, tag, r),
    }
}

fn plain_run_pr(out: &mut String, tag: &str, r: &ShapeRun) {
    let _ = write!(out, r#"<{tag} lang="en-US""#);
    if let Some(sz) = r.size {
        let _ = write!(out, r#" sz="{}""#, (sz * 100.0).round() as i64);
    }
    if r.bold {
        out.push_str(r#" b="1""#);
    }
    if r.italic {
        out.push_str(r#" i="1""#);
    }
    if r.underline {
        out.push_str(r#" u="sng""#);
    }
    if r.color.is_none() && r.font.is_none() {
        out.push_str("/>");
        return;
    }
    out.push('>');
    if let Some(c) = r.color {
        solid(out, Some(c));
    }
    if let Some(f) = &r.font {
        let _ = write!(out, r#"<a:latin typeface="{0}"/><a:cs typeface="{0}"/>"#, esc(f));
    }
    let _ = write!(out, "</{tag}>");
}

/// A Word text box's paragraphs (`w:p`s for `w:txbxContent`).
pub fn write_txbx_paras(out: &mut String, s: &Shape) {
    let paras: Vec<ShapePara> = if s.text.is_empty() { vec![ShapePara { align: s.text_align(), runs: Vec::new() }] } else { s.text.clone() };
    for p in &paras {
        let jc = match p.align {
            Align::Center => "center",
            Align::Right => "right",
            Align::Justify => "both",
            Align::Left => "left",
        };
        // No space between paragraphs, as in the shape on screen.
        let _ = write!(out, r#"<w:p><w:pPr><w:spacing w:before="0" w:after="0" w:line="240" w:lineRule="auto"/><w:jc w:val="{jc}"/>"#);
        w_rpr(out, &p.first_run());
        out.push_str("</w:pPr>");
        for r in p.runs.iter().filter(|r| !r.text.is_empty()) {
            out.push_str("<w:r>");
            w_rpr(out, r);
            for (i, part) in r.text.split('\t').enumerate() {
                if i > 0 {
                    out.push_str("<w:tab/>");
                }
                if !part.is_empty() {
                    let _ = write!(out, r#"<w:t xml:space="preserve">{}</w:t>"#, esc(part));
                }
            }
            out.push_str("</w:r>");
        }
        out.push_str("</w:p>");
    }
}

fn w_rpr(out: &mut String, r: &ShapeRun) {
    let mut p = String::new();
    if let Some(f) = &r.font {
        let _ = write!(p, r#"<w:rFonts w:ascii="{0}" w:hAnsi="{0}" w:cs="{0}"/>"#, esc(f));
    }
    if r.bold {
        p.push_str("<w:b/>");
    }
    if r.italic {
        p.push_str("<w:i/>");
    }
    if let Some(c) = r.color {
        let _ = write!(p, r#"<w:color w:val="{}"/>"#, hex(c));
    }
    let sz = r.size.unwrap_or(apex_doc::SHAPE_TEXT_PT);
    let _ = write!(p, r#"<w:sz w:val="{0}"/><w:szCs w:val="{0}"/>"#, (sz * 2.0).round() as i64);
    if r.underline {
        p.push_str(r#"<w:u w:val="single"/>"#);
    }
    if let Some(fx) = &r.fx {
        crate::text_fx_xml::write_w14(&mut p, fx);
    }
    let _ = write!(out, "<w:rPr>{p}</w:rPr>");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_and_text_round_trip() {
        let mut s = Shape::new(ShapeKind::Callout);
        s.fill = Some(0x123456);
        s.line = None;
        s.rotation = 30.0;
        s.flip_v = true;
        s.preset = Some(Arc::from("cloudCallout"));
        s.adj = vec![(Arc::from("adj1"), -20000), (Arc::from("adj2"), 62500)];
        s.set_plain_text("Hello\nWorld");
        s.format_text(&|r| {
            r.bold = true;
            r.size = Some(14.0);
            r.font = Some(Arc::from("Arial"));
        });
        let mut x = String::from("<r xmlns:a=\"a\"><sp><spPr>");
        write_sp_pr(&mut x, &s, (0, 0), (12700, 12700));
        x.push_str("</spPr>");
        write_style(&mut x, "style", &s);
        x.push_str("<txBody><a:bodyPr anchor=\"ctr\"/>");
        write_paras(&mut x, &s);
        x.push_str("</txBody></sp></r>");
        let root = read_fragment(&x);
        let sp = root.find("sp").expect("sp");
        let mut back = read_look(sp.child("spPr"), sp.child("style"), false);
        read_tx_body(sp.child("txBody").expect("body"), sp.child("style"), &mut back);
        assert_eq!(back, Shape { name: String::new(), ..s.clone() });
        // A gradient fill.
        let mut g = s.clone();
        g.gradient = Some(apex_doc::gradient::variation(0x4472C4, true, 4));
        g.fill = g.gradient.as_ref().map(|g| g.main_color());
        let mut x = String::from("<r xmlns:a=\"a\"><spPr>");
        write_sp_pr(&mut x, &g, (0, 0), (12700, 12700));
        x.push_str("</spPr></r>");
        let root = read_fragment(&x);
        let back = read_look(root.find("spPr"), None, false);
        assert_eq!((back.fill, &back.gradient), (g.fill, &g.gradient));
        // Word's text box paragraphs.
        let mut w = String::from("<c>");
        write_txbx_paras(&mut w, &s);
        w.push_str("</c>");
        let mut back = read_look(None, None, false);
        back.valign = s.valign;
        read_txbx(&read_fragment(&w), None, None, &mut back);
        assert_eq!(back.plain_text(), "Hello\nWorld");
        assert_eq!(back.text_format().size, Some(14.0));
        assert!(back.text_format().bold);
    }

    #[test]
    fn scheme_colors_and_hostile_input() {
        let root = read_fragment(r#"<solidFill><schemeClr val="accent1"><lumMod val="50000"/></schemeClr></solidFill>"#);
        let c = color(root.find("solidFill").expect("fill")).expect("color");
        assert_eq!(c, 0x223962);
        for bad in ["<a", "<sp><spPr><a:xfrm rot=\"NaN\"/><prstGeom prst=\"weird\"/></spPr></sp>", "<<<>>>", ""] {
            let root = read_fragment(bad);
            let s = read_look(root.find("spPr"), None, false);
            assert!(s.rotation.is_finite());
        }
        let deep = "<a>".repeat(10_000);
        let _ = read_fragment(&deep);
    }
}
