//! Shapes in OpenDocument files, shared by .odt and .ods: `draw:rect`,
//! `draw:ellipse`, `draw:line`, `draw:custom-shape` (with the
//! `draw:enhanced-geometry` types LibreOffice writes) and text boxes
//! (`draw:frame` with a `draw:text-box`); their graphic, paragraph and text
//! styles, and the dash and arrow styles they use.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{Align, Dash, DocShape, HRel, Shape, ShapeWrap, VRel, ShapeKind, ShapePara, ShapeRun, VAnchor, shape_outline};

use crate::drawingml::{Node, TEXT};
use crate::parse_length_pt;
use crate::xml::{Ev, Reader, esc};

/// Dash and arrow styles the shapes written use (for `office:styles`).
pub(crate) fn common_styles() -> String {
    let mut s = String::from(r#"<draw:marker draw:name="Arrow" svg:viewBox="0 0 20 30" svg:d="M10 0l-10 30h20z"/>"#);
    for d in [Dash::Dot, Dash::Dash, Dash::DashDot, Dash::LongDash] {
        let p = d.pattern();
        let pc = |v: f32| format!("{}%", (v * 100.0).round() as i64);
        let _ = write!(s, r#"<draw:stroke-dash draw:name="{}" draw:style="rect" draw:dots1="1" draw:dots1-length="{}""#, dash_name(d), pc(p[0]));
        if p.len() > 2 {
            let _ = write!(s, r#" draw:dots2="1" draw:dots2-length="{}""#, pc(p[2]));
        }
        let _ = write!(s, r#" draw:distance="{}"/>"#, pc(p[1]));
    }
    s
}

pub(crate) fn dash_name(d: Dash) -> &'static str {
    match d {
        Dash::Dot => "ApexDot",
        Dash::Dash => "ApexDash",
        Dash::DashDot => "ApexDashDot",
        Dash::LongDash => "ApexLongDash",
        Dash::Solid => "",
    }
}

// --- styles --------------------------------------------------------------------------------

#[derive(Default)]
struct Def {
    parent: Option<String>,
    props: HashMap<String, String>,
}

/// Styles a shape may use, from `styles.xml` and `content.xml`.
#[derive(Default)]
pub(crate) struct Styles {
    defs: HashMap<String, Def>,
    dashes: HashMap<String, Dash>,
    /// Named gradients and their transparencies.
    pub(crate) gradients: HashMap<String, crate::odf_grad::Def>,
    pub(crate) opacities: HashMap<String, crate::odf_grad::Def>,
}

/// Name of the default graphic style among the styles.
const DEFAULT_GRAPHIC: &str = "\u{0}graphic";

/// A hidden object (Selection Pane), as LibreOffice writes it.
pub(crate) const HIDDEN: &str = r#" drawooo:display="none""#;

/// Whether a shape or frame is hidden (LibreOffice's `drawooo:display`).
pub(crate) fn is_hidden(n: &Node) -> bool {
    n.attr("drawooo:display").or_else(|| n.attr("draw:display")).is_some_and(|v| v == "none")
}

impl Styles {
    pub(crate) fn read(&mut self, xml: &str) {
        crate::odf_grad::read_all(xml, &mut self.gradients, &mut self.opacities);
        let mut r = Reader::new(xml);
        let mut cur: Option<String> = None;
        loop {
            match r.next() {
                Ev::Eof => break,
                Ev::Start(t) | Ev::Empty(t) => match t.name {
                    "style:style" => {
                        let name = t.attr("style:name").unwrap_or_default().into_owned();
                        let parent = t.attr("style:parent-style-name").map(|v| v.into_owned());
                        self.defs.insert(name.clone(), Def { parent, props: HashMap::new() });
                        cur = Some(name);
                    }
                    "style:default-style" => {
                        cur = (t.attr("style:family").as_deref() == Some("graphic")).then(|| DEFAULT_GRAPHIC.to_string());
                        if let Some(c) = &cur {
                            self.defs.insert(c.clone(), Def::default());
                        }
                    }
                    "style:graphic-properties" | "style:paragraph-properties" | "style:text-properties" | "style:drawing-page-properties" | "style:table-column-properties" | "style:table-row-properties" | "style:table-cell-properties" => {
                        if let Some(d) = cur.as_ref().and_then(|c| self.defs.get_mut(c)) {
                            for (k, v) in t.attrs() {
                                d.props.insert(k.to_string(), v.into_owned());
                            }
                        }
                    }
                    "draw:stroke-dash" => {
                        let name = t.attr("draw:name").unwrap_or_default().into_owned();
                        let d = dash_of(&name, &t);
                        self.dashes.insert(name, d);
                    }
                    _ => {}
                },
                Ev::End("style:style" | "style:default-style") => cur = None,
                _ => {}
            }
        }
    }

    /// A property of style `name` (or of the ones it inherits from).
    pub(crate) fn get(&self, name: Option<&str>, key: &str) -> Option<&str> {
        let mut n = name;
        for _ in 0..16 {
            let Some(d) = n.and_then(|n| self.defs.get(n)) else { break };
            if let Some(v) = d.props.get(key) {
                return Some(v);
            }
            n = d.parent.as_deref();
        }
        None
    }

    /// The properties style `name` sets itself.
    pub(crate) fn own(&self, name: &str) -> Option<&HashMap<String, String>> {
        self.defs.get(name).map(|d| &d.props)
    }

    /// The gradient a graphic style fills with, with its transparency.
    pub(crate) fn gradient(&self, name: Option<&str>) -> Option<apex_doc::Gradient> {
        let def = self.gradients.get(self.graphic(name, "draw:fill-gradient-name")?)?;
        let op = self.graphic(name, "draw:opacity-name").and_then(|o| self.opacities.get(o));
        let uniform = self.graphic(name, "draw:opacity").and_then(|v| v.trim().trim_end_matches('%').parse::<f32>().ok()).map(|p| p / 100.0);
        Some(def.gradient(op, uniform))
    }

    /// A graphic property, falling back on the default graphic style.
    pub(crate) fn graphic(&self, name: Option<&str>, key: &str) -> Option<&str> {
        self.get(name, key).or_else(|| self.get(Some(DEFAULT_GRAPHIC), key))
    }
}

fn dash_of(name: &str, t: &crate::xml::Tag) -> Dash {
    match name {
        "ApexDot" => return Dash::Dot,
        "ApexDash" => return Dash::Dash,
        "ApexDashDot" => return Dash::DashDot,
        "ApexLongDash" => return Dash::LongDash,
        _ => {}
    }
    if t.attr("draw:dots2").and_then(|v| v.trim().parse::<u32>().ok()).is_some_and(|n| n > 0) && t.attr("draw:dots1").is_some() {
        return Dash::DashDot;
    }
    // Length of the dashes in line widths (a length counts a 1pt line).
    let len = t.attr("draw:dots1-length").and_then(|v| match v.trim().strip_suffix('%') {
        Some(p) => p.trim().parse::<f32>().ok().map(|p| p / 100.0),
        None => parse_length_pt(&v),
    });
    match len {
        Some(l) if l <= 1.5 => Dash::Dot,
        Some(l) if l > 5.5 => Dash::LongDash,
        _ => Dash::Dash,
    }
}

pub(crate) fn hex_color(v: Option<&str>) -> Option<u32> {
    let v = v?.trim().strip_prefix('#')?;
    u32::from_str_radix(v, 16).ok().filter(|c| *c <= 0xFFFFFF && v.len() == 6)
}

// --- reading -------------------------------------------------------------------------------

/// A shape as read: its look and its box (points; the top-left corner
/// before turning, in the coordinates of what holds it).
pub(crate) struct Raw {
    pub shape: Shape,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub z: i64,
    /// Graphic style (for the container's own properties).
    pub style: Option<String>,
}

/// Whether an element is a shape we read.
pub(crate) fn is_shape(name: &str) -> bool {
    matches!(name, "draw:rect" | "draw:ellipse" | "draw:circle" | "draw:line" | "draw:custom-shape") || crate::odf_ink::is_ink_element(name)
}

fn kind_of_type(t: &str) -> (ShapeKind, Option<Arc<str>>) {
    let k = match t {
        "rectangle" | "mso-spt1" | "flowchart-process" => ShapeKind::Rect,
        "round-rectangle" | "mso-spt2" | "flowchart-alternate-process" => ShapeKind::RoundRect,
        "ellipse" | "circle" | "flowchart-connector" => ShapeKind::Ellipse,
        "isosceles-triangle" | "flowchart-extract" => ShapeKind::Triangle,
        "right-triangle" => ShapeKind::RightTriangle,
        "diamond" | "flowchart-decision" => ShapeKind::Diamond,
        "pentagon" => ShapeKind::Pentagon,
        "hexagon" | "octagon" | "flowchart-preparation" => ShapeKind::Hexagon,
        "right-arrow" | "notched-right-arrow" | "striped-right-arrow" | "chevron" | "pentagon-right" => ShapeKind::RightArrow,
        "left-arrow" => ShapeKind::LeftArrow,
        "up-arrow" => ShapeKind::UpArrow,
        "down-arrow" => ShapeKind::DownArrow,
        "star5" | "star4" | "star6" | "star8" | "star12" | "star24" => ShapeKind::Star5,
        "rectangular-callout" | "round-rectangular-callout" | "round-callout" | "cloud-callout" => ShapeKind::Callout,
        "line" => ShapeKind::Line,
        _ => {
            // LibreOffice keeps OOXML presets as "ooxml-<name>".
            if let Some(p) = t.strip_prefix("ooxml-") {
                let (k, exact) = ShapeKind::from_preset(p);
                return (k, (!exact).then(|| Arc::from(p)));
            }
            ShapeKind::Rect
        }
    };
    (k, None)
}

fn type_of_kind(k: ShapeKind) -> &'static str {
    match k {
        ShapeKind::Rect | ShapeKind::TextBox => "rectangle",
        ShapeKind::RoundRect => "round-rectangle",
        ShapeKind::Ellipse => "ellipse",
        ShapeKind::Triangle => "isosceles-triangle",
        ShapeKind::RightTriangle => "right-triangle",
        ShapeKind::Diamond => "diamond",
        ShapeKind::Pentagon => "pentagon",
        ShapeKind::Hexagon => "hexagon",
        ShapeKind::RightArrow => "right-arrow",
        ShapeKind::LeftArrow => "left-arrow",
        ShapeKind::UpArrow => "up-arrow",
        ShapeKind::DownArrow => "down-arrow",
        ShapeKind::Line => "line",
        ShapeKind::Star5 => "star5",
        ShapeKind::Callout => "rectangular-callout",
    }
}

fn len(n: &Node, key: &str) -> Option<f32> {
    n.attr(key).and_then(parse_length_pt).filter(|v| v.is_finite())
}

/// "rotate (a) translate (x y)": the angle (radians, counterclockwise)
/// and the translation (points).
pub(crate) fn parse_transform(s: &str) -> (f32, Option<(f32, f32)>) {
    let (mut rot, mut tr) = (0.0, None);
    let mut rest = s;
    while let Some(open) = rest.find('(') {
        let name = rest[..open].trim().trim_start_matches(',').trim();
        let Some(close) = rest[open..].find(')') else { break };
        let args: Vec<&str> = rest[open + 1..open + close].split([' ', ',']).filter(|a| !a.is_empty()).collect();
        match name {
            "rotate" => rot += args.first().and_then(|a| a.parse::<f32>().ok()).filter(|v| v.is_finite()).unwrap_or(0.0),
            "translate" => {
                let l = |i: usize| args.get(i).and_then(|a| parse_length_pt(a)).filter(|v| v.is_finite()).unwrap_or(0.0);
                tr = Some((l(0), l(1)));
            }
            _ => {}
        }
        rest = &rest[open + close + 1..];
    }
    (rot, tr)
}

/// A frame's border: "0.06pt solid #000000".
pub(crate) fn border(v: &str) -> Option<(f32, u32)> {
    if v.trim() == "none" || v.contains("hidden") {
        return None;
    }
    let mut w = 0.75;
    let mut c = 0;
    for part in v.split_whitespace() {
        if let Some(x) = hex_color(Some(part)) {
            c = x;
        } else if let Some(l) = parse_length_pt(part) {
            w = l;
        }
    }
    Some((w, c))
}

/// Reads a shape element (see [`is_shape`]) or a text box frame.
pub(crate) fn read(n: &Node, st: &Styles) -> Option<Raw> {
    let frame = n.local() == "frame";
    let geom = n.child("enhanced-geometry");
    let (kind, preset) = match n.local() {
        "rect" if len(n, "draw:corner-radius").is_some_and(|r| r > 0.0) => (ShapeKind::RoundRect, None),
        "rect" => (ShapeKind::Rect, None),
        "ellipse" | "circle" => (ShapeKind::Ellipse, None),
        "line" => (ShapeKind::Line, None),
        "custom-shape" => kind_of_type(geom.and_then(|g| g.attr("draw:type")).unwrap_or("")),
        "frame" if n.child("text-box").is_some() => (ShapeKind::TextBox, None),
        // Ink, if the lines aren't closed (see below).
        "path" | "polyline" => (ShapeKind::Rect, None),
        _ => return None,
    };
    let mut s = Shape::new(kind);
    s.preset = preset;
    let style = n.attr("draw:style-name").map(str::to_string);
    let g = |k: &str| st.graphic(style.as_deref(), k);
    // Fill.
    if !kind.is_line() {
        s.fill = match g("draw:fill") {
            Some("none") => None,
            Some(_) => hex_color(g("draw:fill-color")).or(Some(0x729FCF)),
            None if frame => hex_color(g("fo:background-color")),
            // LibreOffice fills shapes when no style says otherwise.
            None => hex_color(g("draw:fill-color")).or(Some(0x729FCF)),
        };
        if g("draw:fill") == Some("gradient") {
            s.gradient = st.gradient(style.as_deref());
            if let Some(gr) = &s.gradient {
                s.fill = Some(gr.main_color());
            }
        }
    }
    // Outline.
    match g("draw:stroke") {
        Some("none") => s.line = None,
        Some(v) => {
            s.line = Some(hex_color(g("svg:stroke-color")).unwrap_or(0));
            s.line_width = g("svg:stroke-width").and_then(parse_length_pt).unwrap_or(0.0).max(0.0);
            if v == "dash" {
                s.dash = g("draw:stroke-dash").and_then(|d| st.dashes.get(d)).copied().unwrap_or(Dash::Dash);
            }
        }
        None if frame => match g("fo:border").and_then(border) {
            Some((w, c)) => (s.line, s.line_width) = (Some(c), w),
            None => s.line = None,
        },
        None => {
            s.line = Some(hex_color(g("svg:stroke-color")).unwrap_or(0x3465A4));
            s.line_width = g("svg:stroke-width").and_then(parse_length_pt).unwrap_or(0.0).max(0.0);
        }
    }
    s.arrow_start = g("draw:marker-start").is_some_and(|m| !m.is_empty());
    s.arrow_end = g("draw:marker-end").is_some_and(|m| !m.is_empty());
    // Vertical text is kept (in DrawingML's terms), not shown yet.
    s.vert = match g("style:writing-mode") {
        Some("tb-rl" | "tb") => Some(Arc::from("eaVert")),
        Some("tb-lr") => Some(Arc::from("mongolianVert")),
        Some("bt-lr") => Some(Arc::from("vert270")),
        _ => None,
    };
    if let Some(v) = g("draw:textarea-vertical-align") {
        s.valign = match v {
            "middle" => VAnchor::Middle,
            "bottom" => VAnchor::Bottom,
            _ => VAnchor::Top,
        };
    }
    if let Some(geom) = geom {
        s.flip_h = geom.attr("draw:mirror-horizontal") == Some("true");
        s.flip_v = geom.attr("draw:mirror-vertical") == Some("true");
        // LibreOffice keeps an OOXML preset's adjust values as modifiers.
        let ty = geom.attr("draw:type").unwrap_or("");
        if let (Some(p), Some(m)) = (ty.strip_prefix("ooxml-"), geom.attr("draw:modifiers")) {
            for ((name, _), v) in apex_doc::preset_adjusts(p).into_iter().zip(m.split_whitespace()) {
                if let Ok(v) = v.parse::<f64>() {
                    if v.is_finite() {
                        s.adj.push((Arc::from(name), v.round() as i64));
                    }
                }
            }
        }
    }
    // The box.
    let (mut x, mut y, mut w, mut h) = if n.local() == "line" {
        let (x1, y1, x2, y2) = (len(n, "svg:x1").unwrap_or(0.0), len(n, "svg:y1").unwrap_or(0.0), len(n, "svg:x2").unwrap_or(0.0), len(n, "svg:y2").unwrap_or(0.0));
        s.flip_h = x2 < x1;
        s.flip_v = y2 < y1;
        (x1.min(x2), y1.min(y2), (x2 - x1).abs(), (y2 - y1).abs())
    } else {
        let (w, h) = (len(n, "svg:width").unwrap_or(0.0).max(0.0), len(n, "svg:height").unwrap_or(0.0).max(0.0));
        match n.attr("draw:transform").map(parse_transform) {
            Some((rot, Some((tx, ty)))) => {
                // The translation places the turned top-left corner.
                let t = -rot;
                s.rotation = (t.to_degrees() * 100.0).round() / 100.0;
                let (sn, c) = t.sin_cos();
                let (hw, hh) = (w / 2.0, h / 2.0);
                let (cx, cy) = (tx + hw * c - hh * sn, ty + hw * sn + hh * c);
                (cx - hw, cy - hh, w, h)
            }
            _ => (len(n, "svg:x").unwrap_or(0.0), len(n, "svg:y").unwrap_or(0.0), w, h),
        }
    };
    s.name = n.attr("draw:name").unwrap_or_default().to_string();
    s.hidden = is_hidden(n);
    s.alt = n.child("desc").or_else(|| n.child("title")).map(|d| d.text.clone()).unwrap_or_default();
    let body = if frame { n.child("text-box") } else { Some(n) };
    if let (Some(body), true) = (body, s.can_have_text()) {
        read_text(body, st, &mut s);
    }
    if crate::odf_ink::is_ink_element(&n.name) {
        let mut ink = crate::odf_ink::read(n, &|k| g(k).map(str::to_string))?;
        let (name, alt, rotation) = (std::mem::take(&mut s.name), std::mem::take(&mut s.alt), s.rotation);
        // A box around the strokes with room for their width (a straight
        // line's own box has no height).
        if rotation == 0.0 {
            if let Some((fit, b)) = apex_doc::Ink::from_abs(ink.to_abs([x, y, w, h]), 1.0) {
                (ink, x, y, w, h) = (fit, b[0], b[1], b[2], b[3]);
            }
        }
        s = Shape { name: if name.is_empty() { "Ink".into() } else { name }, alt, rotation, ..Shape::ink(ink) };
    }
    s.sanitize();
    Some(Raw { shape: s, x, y, w, h, z: n.attr("draw:z-index").and_then(|v| v.trim().parse::<i64>().ok()).unwrap_or(0), style })
}

fn run_props(st: &Styles, style: Option<&str>, r: &mut ShapeRun) {
    let get = |k: &str| st.get(style, k);
    if let Some(w) = get("fo:font-weight") {
        r.bold = w == "bold" || w.parse::<u32>().is_ok_and(|v| v >= 600);
    }
    if let Some(v) = get("fo:font-style") {
        r.italic = v == "italic" || v == "oblique";
    }
    if let Some(v) = get("style:text-underline-style") {
        r.underline = v != "none";
    }
    if let Some(v) = get("fo:font-size").and_then(parse_length_pt) {
        r.size = Some(v);
    }
    if let Some(c) = hex_color(get("fo:color")) {
        r.color = Some(c);
    }
    r.fx = crate::text_fx_xml::odf_read(r.fx.take(), get("fo:text-shadow"), get("style:text-outline"), get("style:font-relief"), r.color);
    if let Some(f) = get("fo:font-family").or_else(|| get("style:font-name")) {
        let f = f.trim().trim_matches(['\'', '"']);
        if !f.is_empty() {
            r.font = Some(Arc::from(f));
        }
    }
}

/// The paragraphs of a shape (`text:p` in it).
fn read_text(body: &Node, st: &Styles, s: &mut Shape) {
    let mut paras: Vec<ShapePara> = Vec::new();
    for p in body.kids.iter().filter(|k| matches!(k.local(), "p" | "h")) {
        let pstyle = p.attr("text:style-name");
        let align = match st.get(pstyle, "fo:text-align") {
            Some("center") => Align::Center,
            Some("end" | "right") => Align::Right,
            Some("justify") => Align::Justify,
            Some(_) => Align::Left,
            None if s.is_text_box() => Align::Left,
            None => Align::Center,
        };
        let mut base = ShapeRun::default();
        run_props(st, pstyle, &mut base);
        let mut cur = ShapePara { align, runs: Vec::new() };
        let mut prev_space = true;
        walk(p, st, &base, &mut cur, &mut paras, &mut prev_space);
        if cur.runs.is_empty() {
            cur.runs.push(base.clone());
        }
        paras.push(cur);
    }
    if paras.iter().all(|p| p.runs.iter().all(|r| r.text.is_empty())) {
        s.text.clear();
        return;
    }
    // Text in the automatic color is written with it: read it as automatic.
    let auto = {
        let mut probe = s.clone();
        probe.text.clear();
        probe.auto_text_color()
    };
    for p in &mut paras {
        for r in &mut p.runs {
            if r.color == Some(auto) {
                r.color = None;
            }
            // Likewise the default size.
            if r.size == Some(apex_doc::SHAPE_TEXT_PT) {
                r.size = None;
            }
        }
    }
    s.text = paras;
}

fn push(para: &mut ShapePara, fmt: &ShapeRun, text: &str) {
    if let Some(last) = para.runs.last_mut() {
        if last.with_text("") == fmt.with_text("") {
            last.text.push_str(text);
            return;
        }
    }
    para.runs.push(fmt.with_text(text));
}

fn walk(n: &Node, st: &Styles, fmt: &ShapeRun, cur: &mut ShapePara, paras: &mut Vec<ShapePara>, prev_space: &mut bool) {
    for k in &n.kids {
        if k.name == TEXT {
            // Whitespace collapses as in any ODF paragraph.
            let mut t = String::with_capacity(k.text.len());
            for ch in k.text.chars() {
                if ch.is_whitespace() {
                    if !*prev_space {
                        t.push(' ');
                    }
                    *prev_space = true;
                } else {
                    t.push(ch);
                    *prev_space = false;
                }
            }
            if !t.is_empty() {
                push(cur, fmt, &t);
            }
            continue;
        }
        match k.local() {
            "span" | "a" => {
                let mut f = fmt.clone();
                run_props(st, k.attr("text:style-name"), &mut f);
                walk(k, st, &f, cur, paras, prev_space);
            }
            "s" => {
                let c = k.attr("text:c").and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(1).min(1000);
                push(cur, fmt, &" ".repeat(c));
                *prev_space = false;
            }
            "tab" => {
                push(cur, fmt, "\t");
                *prev_space = false;
            }
            "line-break" => {
                if cur.runs.is_empty() {
                    cur.runs.push(fmt.clone());
                }
                let next = ShapePara { align: cur.align, runs: Vec::new() };
                paras.push(std::mem::replace(cur, next));
                *prev_space = true;
            }
            _ => {}
        }
    }
}

// --- writing -------------------------------------------------------------------------------

/// Where a shape goes: its box in points (the top-left corner before
/// turning, in the coordinates of what holds it), anchoring attributes of
/// the element and the container's graphic properties.
pub(crate) struct Place<'a> {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub attrs: &'a str,
    pub graphic: &'a str,
    pub z: i64,
}

/// Writes shapes and collects their automatic styles.
#[derive(Default)]
pub(crate) struct Writer {
    pub styles: String,
    n: usize,
    paras: HashMap<String, String>,
    texts: HashMap<String, String>,
    /// Names of linked text boxes by (chain, seq).
    chains: HashMap<(u32, u16), String>,
    /// Named gradients, for `office:styles`.
    pub named: crate::odf_grad::Named,
}

fn pt(v: f32) -> String {
    let v = if v.is_finite() { v } else { 0.0 };
    format!("{}pt", (v * 1000.0).round() / 1000.0)
}

fn hex(c: u32) -> String {
    format!("#{:06x}", c & 0xFFFFFF)
}

impl Writer {
    /// Names the linked text boxes of a document (ODF links them by name).
    pub(crate) fn name_chains(&mut self, doc: &apex_doc::Document) {
        let shapes = doc.shapes();
        let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (_, s) in &shapes {
            let Some(l) = s.shape.link else { continue };
            let name = if s.shape.name.is_empty() || used.contains(&s.shape.name) { format!("LinkedText{}_{}", l.chain, l.seq + 1) } else { s.shape.name.clone() };
            used.insert(name.clone());
            self.chains.insert((l.chain, l.seq), name);
        }
    }

    fn para_style(&mut self, align: Align) -> String {
        let a = match align {
            Align::Center => "center",
            Align::Right => "end",
            Align::Justify => "justify",
            Align::Left => "start",
        };
        let props = format!(r#"<style:paragraph-properties fo:text-align="{a}" fo:margin-top="0pt" fo:margin-bottom="0pt" fo:margin-left="0pt" fo:margin-right="0pt" fo:text-indent="0pt"/>"#);
        let n = self.paras.len();
        let styles = &mut self.styles;
        self.paras
            .entry(props.clone())
            .or_insert_with(|| {
                let name = format!("ShP{}", n + 1);
                let _ = write!(styles, r#"<style:style style:name="{name}" style:family="paragraph">{props}</style:style>"#);
                name
            })
            .clone()
    }

    fn text_style(&mut self, s: &Shape, r: &ShapeRun) -> String {
        let mut p = format!(r#" fo:font-size="{}" fo:color="{}""#, pt(r.size.unwrap_or(apex_doc::SHAPE_TEXT_PT)), hex(r.color.unwrap_or_else(|| s.auto_text_color())));
        if let Some(fx) = &r.fx {
            p.push_str(&crate::text_fx_xml::odf_attrs(fx, true));
        }
        if r.bold {
            p.push_str(r#" fo:font-weight="bold" style:font-weight-asian="bold" style:font-weight-complex="bold""#);
        }
        if r.italic {
            p.push_str(r#" fo:font-style="italic" style:font-style-asian="italic" style:font-style-complex="italic""#);
        }
        if r.underline {
            p.push_str(r#" style:text-underline-style="solid" style:text-underline-width="auto" style:text-underline-color="font-color""#);
        }
        if let Some(f) = &r.font {
            let _ = write!(p, r#" fo:font-family="'{}'""#, esc(&f.replace('\'', "")));
        }
        let n = self.texts.len();
        let styles = &mut self.styles;
        self.texts
            .entry(p.clone())
            .or_insert_with(|| {
                let name = format!("ShT{}", n + 1);
                let _ = write!(styles, r#"<style:style style:name="{name}" style:family="text"><style:text-properties{p}/></style:style>"#);
                name
            })
            .clone()
    }

    fn paras(&mut self, out: &mut String, s: &Shape) {
        for para in &s.text {
            let ps = self.para_style(para.align);
            let _ = write!(out, r#"<text:p text:style-name="{ps}">"#);
            for r in para.runs.iter().filter(|r| !r.text.is_empty()) {
                let ts = self.text_style(s, r);
                let _ = write!(out, r#"<text:span text:style-name="{ts}">"#);
                crate::odt::write_text(out, &r.text);
                out.push_str("</text:span>");
            }
            out.push_str("</text:p>");
        }
        if s.text.is_empty() {
            out.push_str("<text:p/>");
        }
    }

    /// Ink: a line for each look of its strokes, grouped in text
    /// documents.
    fn write_ink(&mut self, out: &mut String, s: &Shape, ink: &apex_doc::Ink, at: &Place) {
        let looks = crate::inkml::looks(ink);
        let z = at.z.max(0);
        let grouped = looks.len() > 1 && at.attrs.contains("text:anchor-type");
        if grouped {
            let _ = write!(out, r#"<draw:g {} draw:z-index="{z}">"#, at.attrs);
        }
        let mut name = if s.name.is_empty() { String::new() } else { format!(r#" draw:name="{}""#, esc(&s.name)) };
        if s.hidden {
            name.push_str(HIDDEN);
        }
        let desc = if s.alt.is_empty() { String::new() } else { format!("<svg:desc>{}</svg:desc>", esc(&s.alt)) };
        for g in looks {
            self.n += 1;
            let style = format!("Shg{}", self.n);
            let _ = write!(self.styles, r#"<style:style style:name="{style}" style:family="graphic"><style:graphic-properties{}{}/></style:style>"#, crate::odf_ink::graphic(g[0]), at.graphic);
            let ([x, y, w, h], vb, d) = crate::odf_ink::path(&g, [at.x, at.y, at.w.max(0.0), at.h.max(0.0)], s);
            let _ = write!(
                out,
                r#"<draw:path draw:style-name="{style}"{name} {} draw:z-index="{z}" svg:width="{}" svg:height="{}" svg:x="{}" svg:y="{}" svg:viewBox="{vb}" svg:d="{d}">{desc}<text:p/></draw:path>"#,
                at.attrs,
                pt(w),
                pt(h),
                pt(x),
                pt(y)
            );
        }
        if grouped {
            out.push_str("</draw:g>");
        }
    }

    /// A shape element, with its styles collected.
    pub(crate) fn write(&mut self, out: &mut String, s: &Shape, at: &Place) {
        if let Some(ink) = &s.ink {
            return self.write_ink(out, s, ink, at);
        }
        self.n += 1;
        let style = format!("Shg{}", self.n);
        let mut g = String::new();
        let frame = s.is_text_box();
        if !s.kind.is_line() {
            match s.fill {
                Some(c) if s.gradient.is_some() => {
                    if let Some(gr) = &s.gradient {
                        g.push_str(&self.named.props(gr));
                    }
                    if frame {
                        let _ = write!(g, r#" fo:background-color="{}""#, hex(c));
                    }
                }
                Some(c) => {
                    let _ = write!(g, r#" draw:fill="solid" draw:fill-color="{}""#, hex(c));
                    if frame {
                        let _ = write!(g, r#" fo:background-color="{}""#, hex(c));
                    }
                }
                None => {
                    g.push_str(r#" draw:fill="none""#);
                    if frame {
                        g.push_str(r#" fo:background-color="transparent""#);
                    }
                }
            }
        }
        match s.line {
            Some(c) if frame => {
                let _ = write!(g, r#" fo:border="{} solid {}""#, pt(s.line_width), hex(c));
            }
            None if frame => g.push_str(r#" fo:border="none""#),
            Some(c) => {
                let kind = if s.dash == Dash::Solid { "solid" } else { "dash" };
                let _ = write!(g, r#" draw:stroke="{kind}" svg:stroke-color="{}" svg:stroke-width="{}""#, hex(c), pt(s.line_width));
                if s.dash != Dash::Solid {
                    let _ = write!(g, r#" draw:stroke-dash="{}""#, dash_name(s.dash));
                }
                let head = pt((s.line_width * 4.0).max(6.0));
                if s.arrow_start {
                    let _ = write!(g, r#" draw:marker-start="Arrow" draw:marker-start-width="{head}""#);
                }
                if s.arrow_end {
                    let _ = write!(g, r#" draw:marker-end="Arrow" draw:marker-end-width="{head}""#);
                }
            }
            None => g.push_str(r#" draw:stroke="none""#),
        }
        if s.can_have_text() {
            let v = match s.valign {
                VAnchor::Top => "top",
                VAnchor::Middle => "middle",
                VAnchor::Bottom => "bottom",
            };
            let mode = match s.vert.as_deref() {
                Some("eaVert" | "vert" | "wordArtVert" | "wordArtVertRtl") => r#" style:writing-mode="tb-rl""#,
                Some("mongolianVert") => r#" style:writing-mode="tb-lr""#,
                Some("vert270") => r#" style:writing-mode="bt-lr""#,
                _ => "",
            };
            g.push_str(mode);
            let _ = write!(g, r#" draw:textarea-vertical-align="{v}" draw:auto-grow-height="false" draw:auto-grow-width="false" fo:padding-top="3.6pt" fo:padding-bottom="3.6pt" fo:padding-left="7.2pt" fo:padding-right="7.2pt""#);
        }
        let _ = write!(self.styles, r#"<style:style style:name="{style}" style:family="graphic"><style:graphic-properties{g}{}/></style:style>"#, at.graphic);
        let own = s.link.and_then(|l| self.chains.get(&(l.chain, l.seq)).cloned()).unwrap_or_else(|| s.name.clone());
        let mut name = if own.is_empty() { String::new() } else { format!(r#" draw:name="{}""#, esc(&own)) };
        if s.hidden {
            name.push_str(HIDDEN);
        }
        let next = s.link.and_then(|l| self.chains.get(&(l.chain, l.seq + 1))).map_or(String::new(), |n| format!(r#" draw:chain-next-name="{}""#, esc(n)));
        let z = at.z.max(0);
        let desc = if s.alt.is_empty() { String::new() } else { format!("<svg:desc>{}</svg:desc>", esc(&s.alt)) };
        let (w, h) = (at.w.max(0.0), at.h.max(0.0));
        let (cx, cy) = (at.x + w / 2.0, at.y + h / 2.0);
        let (sn, c) = s.rotation.to_radians().sin_cos();
        let turn = |x: f32, y: f32| (cx + (x - cx) * c - (y - cy) * sn, cy + (x - cx) * sn + (y - cy) * c);
        let geometry = s.geometry().filter(|g| !matches!(*g, "line" | "straightConnector1"));
        if s.kind.is_line() && geometry.is_none() {
            let (x0, x1) = if s.flip_h { (at.x + w, at.x) } else { (at.x, at.x + w) };
            let (y0, y1) = if s.flip_v { (at.y + h, at.y) } else { (at.y, at.y + h) };
            let (a, b) = (turn(x0, y0), turn(x1, y1));
            let _ = write!(
                out,
                r#"<draw:line draw:style-name="{style}"{name} {} draw:z-index="{z}" svg:x1="{}" svg:y1="{}" svg:x2="{}" svg:y2="{}">{desc}<text:p/></draw:line>"#,
                at.attrs,
                pt(a.0),
                pt(a.1),
                pt(b.0),
                pt(b.1)
            );
            return;
        }
        let pos = if s.rotation != 0.0 {
            // ODF turns counterclockwise about the origin, then moves the
            // turned top-left corner into place.
            let (tx, ty) = turn(at.x, at.y);
            format!(r#"draw:transform="rotate ({}) translate ({} {})""#, -s.rotation.to_radians(), pt(tx), pt(ty))
        } else {
            format!(r#"svg:x="{}" svg:y="{}""#, pt(at.x), pt(at.y))
        };
        let head = format!(r#"draw:style-name="{style}"{name} {} draw:z-index="{z}" svg:width="{}" svg:height="{}" {pos}"#, at.attrs, pt(w), pt(h));
        let mut text = String::new();
        self.paras(&mut text, s);
        if frame {
            let _ = write!(out, r#"<draw:frame {head}><draw:text-box{next}>{text}</draw:text-box>{desc}</draw:frame>"#);
            return;
        }
        let path = if let Some(g) = geometry {
            preset_path(g, w, h, &s.adj)
        } else if s.kind == ShapeKind::Ellipse {
            "U 10800 10800 10800 10800 0 360 Z N".to_string()
        } else {
            let pts = shape_outline(s.kind, 21600.0, 21600.0);
            let mut p = String::new();
            for (i, (x, y)) in pts.iter().enumerate() {
                let _ = write!(p, "{}{} {} ", if i == 0 { "M " } else if i == 1 { "L " } else { "" }, x.round() as i64, y.round() as i64);
            }
            p.push_str("Z N");
            p
        };
        let mirror = format!("{}{}", if s.flip_h { r#" draw:mirror-horizontal="true""# } else { "" }, if s.flip_v { r#" draw:mirror-vertical="true""# } else { "" });
        let _ = write!(
            out,
            r#"<draw:custom-shape {head}>{desc}{text}<draw:enhanced-geometry svg:viewBox="0 0 21600 21600" draw:type="{}"{} draw:enhanced-path="{path}"{mirror}/></draw:custom-shape>"#,
            geometry.map_or(type_of_kind(s.kind).to_string(), |g| format!("ooxml-{g}")),
            geometry.map_or(String::new(), |g| modifiers(g, &s.adj))
        );
    }
}

/// The enhanced path of preset `g` in a `w` by `h` box, in a 21600 unit view box.
fn preset_path(g: &str, w: f32, h: f32, adj: &[(Arc<str>, i64)]) -> String {
    let (w, h) = (w.max(1.0), h.max(1.0));
    let mut out = String::new();
    for p in apex_doc::preset_paths(g, w, h, adj).unwrap_or_default() {
        for (pts, closed) in &p.subpaths {
            for (i, (x, y)) in pts.iter().enumerate() {
                let _ = write!(out, "{}{} {} ", if i == 0 { "M " } else if i == 1 { "L " } else { "" }, (x / w * 21600.0).round() as i64, (y / h * 21600.0).round() as i64);
            }
            if *closed {
                out.push_str("Z ");
            }
        }
        if p.fill == apex_doc::PresetFill::None {
            out.push_str("F ");
        }
        if !p.stroke {
            out.push_str("S ");
        }
        out.push_str("N ");
    }
    out.pop();
    out
}

/// The `draw:modifiers` attribute for preset `g`: its adjust values in the
/// preset's order (none when all take their defaults).
fn modifiers(g: &str, adj: &[(Arc<str>, i64)]) -> String {
    if adj.is_empty() {
        return String::new();
    }
    let vals: Vec<String> = apex_doc::preset_adjusts(g).into_iter().map(|(n, d)| adj.iter().find(|(k, _)| &**k == n).map_or(d, |(_, v)| *v).to_string()).collect();
    format!(r#" draw:modifiers="{}""#, vals.join(" "))
}

// --- text documents ------------------------------------------------------------------------

/// A shape of a text document from what was read: `anchor` is its
/// `text:anchor-type`; page-anchored shapes are placed from the page.
pub(crate) fn to_doc(raw: Raw, anchor: &str, st: &Styles) -> DocShape {
    let g = |k: &str| st.graphic(raw.style.as_deref(), k);
    let mut d = DocShape::new(raw.shape.clone(), raw.w, raw.h);
    d.z = raw.z.clamp(-1_000_000, 1_000_000) as i32;
    if anchor == "as-char" {
        d.wrap = ShapeWrap::Inline;
        d.sanitize();
        return d;
    }
    d.wrap = match (g("style:wrap"), g("style:run-through")) {
        (Some("run-through") | None, Some("background")) => ShapeWrap::Behind,
        (Some("run-through") | None, _) => ShapeWrap::InFront,
        (Some("none"), _) => ShapeWrap::TopBottom,
        _ => ShapeWrap::Square,
    };
    let page = anchor == "page";
    d.h_rel = match g("style:horizontal-rel") {
        Some("page" | "page-start-margin" | "page-end-margin") => HRel::Page,
        Some(_) => HRel::Margin,
        None if page => HRel::Page,
        None => HRel::Margin,
    };
    d.v_rel = match g("style:vertical-rel") {
        Some("page") => VRel::Page,
        Some("page-content") => VRel::Margin,
        _ if page => VRel::Page,
        _ => VRel::Paragraph,
    };
    d.h_align = match g("style:horizontal-pos") {
        Some("center") => Some(Align::Center),
        Some("right" | "outside") => Some(Align::Right),
        Some("left" | "inside") => Some(Align::Left),
        _ => None,
    };
    (d.x, d.y) = (raw.x, raw.y);
    d.sanitize();
    d
}

/// Reads a shape of a text document from its element.
pub(crate) fn read_doc(n: &Node, st: &Styles) -> Option<DocShape> {
    let anchor = n.attr("text:anchor-type").unwrap_or("paragraph").to_string();
    read(n, st).map(|raw| to_doc(raw, &anchor, st))
}

/// Whether the source of a `draw:frame` (see [`Reader::raw_element`]) is a
/// text box rather than a picture.
pub(crate) fn frame_is_text_box(raw: &str) -> bool {
    raw.contains("<draw:text-box") && !raw.contains("<draw:image")
}

impl Writer {
    /// A shape of a text document, inside its paragraph; `z_min` is the
    /// lowest z of the document (ODF's z-index starts at 0).
    pub(crate) fn write_doc(&mut self, out: &mut String, d: &DocShape, z_min: i32) {
        let mut graphic = String::new();
        let (attrs, x, y) = if d.wrap == ShapeWrap::Inline {
            graphic.push_str(r#" style:vertical-pos="top" style:vertical-rel="baseline""#);
            (r#"text:anchor-type="as-char""#, 0.0, 0.0)
        } else {
            let wrap = match d.wrap {
                ShapeWrap::Behind => r#"style:wrap="run-through" style:run-through="background""#,
                ShapeWrap::Square => r#"style:wrap="parallel" style:run-through="foreground""#,
                ShapeWrap::TopBottom => r#"style:wrap="none" style:run-through="foreground""#,
                _ => r#"style:wrap="run-through" style:run-through="foreground""#,
            };
            let h_pos = match d.h_align {
                Some(Align::Center) => "center",
                Some(Align::Right) => "right",
                Some(_) => "left",
                None => "from-left",
            };
            let h_rel = if d.h_rel == HRel::Page { "page" } else { "page-content" };
            // Shapes in the text flow sit on their paragraph.
            let v_rel = match d.v_rel {
                _ if !d.wrap.floats() => "paragraph",
                VRel::Paragraph => "paragraph",
                VRel::Margin => "page-content",
                VRel::Page => "page",
            };
            let y = if d.wrap.floats() { d.y } else { 0.0 };
            let _ = write!(graphic, r#" {wrap} style:horizontal-pos="{h_pos}" style:horizontal-rel="{h_rel}" style:vertical-pos="from-top" style:vertical-rel="{v_rel}""#);
            (r#"text:anchor-type="paragraph""#, d.x, y)
        };
        let z = (d.z as i64 - z_min as i64).max(0);
        self.write(out, &d.shape, &Place { x, y, w: d.width, h: d.height, attrs, graphic: &graphic, z });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(s: &Shape, rot_box: (f32, f32, f32, f32)) -> Raw {
        let mut w = Writer::default();
        let mut x = String::new();
        let (bx, by, bw, bh) = rot_box;
        w.write(&mut x, s, &Place { x: bx, y: by, w: bw, h: bh, attrs: r#"text:anchor-type="paragraph""#, graphic: "", z: 3 });
        let mut st = Styles::default();
        st.read(&format!("<r><office:styles>{}{}</office:styles>{}</r>", common_styles(), w.named.xml, w.styles));
        let root = crate::drawingml::read_fragment(&x);
        read(&root.kids[0], &st).expect("shape")
    }

    #[test]
    fn shapes_round_trip() {
        let mut s = Shape::new(ShapeKind::Star5);
        s.rotation = 30.0;
        s.flip_h = true;
        s.dash = Dash::DashDot;
        s.set_plain_text("A  b\tc\nSecond");
        s.format_text(&|r| {
            r.italic = true;
            r.size = Some(9.0);
            r.font = Some(Arc::from("DejaVu Sans"));
        });
        s.name = "Star".into();
        s.alt = "a <star>".into();
        let back = round_trip(&s, (10.0, 20.0, 100.0, 50.0));
        assert_eq!(back.shape, s);
        assert!((back.x - 10.0).abs() < 0.01 && (back.y - 20.0).abs() < 0.01 && (back.w - 100.0).abs() < 0.01, "{} {}", back.x, back.y);
        assert_eq!(back.z, 3);
        let mut l = Shape::new(ShapeKind::Line);
        l.flip_v = true;
        l.arrow_end = true;
        l.dash = Dash::Dot;
        let back = round_trip(&l, (5.0, 5.0, 80.0, 40.0));
        assert_eq!(back.shape, l);
        assert!((back.w - 80.0).abs() < 0.01 && (back.h - 40.0).abs() < 0.01);
        let mut t = Shape::new(ShapeKind::TextBox);
        t.set_plain_text("Box");
        t.format_text(&|r| r.color = Some(0xFF0000));
        let back = round_trip(&t, (0.0, 0.0, 144.0, 54.0));
        assert_eq!(back.shape, t);
        let mut e = Shape::new(ShapeKind::Ellipse);
        e.fill = None;
        e.line_width = 3.0;
        assert_eq!(round_trip(&e, (0.0, 0.0, 10.0, 10.0)).shape, e);
        // Presets we have no kind for, with their adjust values.
        let mut c = Shape::new(ShapeKind::Rect);
        c.preset = Some(Arc::from("smileyFace"));
        c.adj = vec![(Arc::from("adj"), -4000)];
        assert_eq!(round_trip(&c, (0.0, 0.0, 50.0, 50.0)).shape, c);
        let mut r = Shape::new(ShapeKind::RoundRect);
        r.adj = vec![(Arc::from("adj"), 30000)];
        assert_eq!(round_trip(&r, (0.0, 0.0, 50.0, 30.0)).shape, r);
        // A bent connector.
        let mut b = Shape::new(ShapeKind::Line);
        b.preset = Some(Arc::from("bentConnector3"));
        b.arrow_end = true;
        b.flip_h = true;
        let back = round_trip(&b, (5.0, 5.0, 80.0, 40.0));
        assert_eq!(back.shape, b);
        assert!((back.w - 80.0).abs() < 0.01 && (back.h - 40.0).abs() < 0.01);
        // Gradient fills, with see-through stops, of shapes and text boxes.
        for kind in [ShapeKind::Rect, ShapeKind::TextBox] {
            let mut g = Shape::new(kind);
            let mut gr = apex_doc::Gradient::two(0x203040, 0xF0E0D0, 30.0);
            gr.add_stop(0.25);
            gr.stops[2].alpha = 0.5;
            g.fill = Some(gr.main_color());
            g.gradient = Some(gr);
            assert_eq!(round_trip(&g, (0.0, 0.0, 50.0, 30.0)).shape, g);
        }
    }

    #[test]
    fn libreoffice_shapes() {
        let styles = r##"<office:styles><style:default-style style:family="graphic"><style:graphic-properties svg:stroke-color="#3465a4" draw:fill-color="#729fcf"/></style:default-style></office:styles><office:automatic-styles><style:style style:name="gr1" style:family="graphic"><style:graphic-properties draw:fill="solid" draw:fill-color="#ff0000" draw:stroke="none"/></style:style><style:style style:name="P1" style:family="paragraph"><style:paragraph-properties fo:text-align="center"/></style:style></office:automatic-styles>"##;
        let mut st = Styles::default();
        st.read(styles);
        let x = r#"<draw:custom-shape draw:style-name="gr1" svg:width="2cm" svg:height="1cm" svg:x="1cm" svg:y="0cm"><text:p text:style-name="P1">Hi <text:span>there</text:span></text:p><draw:enhanced-geometry draw:type="ooxml-wedgeRoundRectCallout"/></draw:custom-shape>"#;
        let root = crate::drawingml::read_fragment(x);
        let r = read(&root.kids[0], &st).expect("shape");
        assert_eq!(r.shape.kind, ShapeKind::Callout);
        assert_eq!(r.shape.preset.as_deref(), Some("wedgeRoundRectCallout"));
        assert_eq!((r.shape.fill, r.shape.line), (Some(0xFF0000), None));
        assert_eq!(r.shape.plain_text(), "Hi there");
        assert_eq!(r.shape.text_align(), Align::Center);
        let e = crate::drawingml::read_fragment(r#"<draw:ellipse svg:width="bad" draw:transform="rotate (x) translate"/>"#);
        assert!(read(&e.kids[0], &st).is_some());
    }
}
