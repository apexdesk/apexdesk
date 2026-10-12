//! Ink in Office files: InkML parts (`word/ink/inkN.xml`, `xl/ink/inkN.xml`)
//! read into [`apex_doc::Ink`] and written from it, and the DrawingML
//! freeform shapes written for programs that don't know ink (read back as
//! ink too, as are unfilled freeform lines).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{Ink, InkSource, InkStroke, InkTool};

use crate::drawingml::{self, Node};

pub(crate) const CONTENT_TYPE: &str = "application/inkml+xml";
/// Relationship type of ink parts (Office uses the custom XML one).
pub(crate) const REL_TYPE: &str = crate::ooxml_keep::REL_CUSTOM_XML;
/// InkML units per point: X and Y are written in 1/1000 cm.
const UNITS_PT: f32 = 2540.0 / 72.0;
/// Pressure channel maximum written.
const F_MAX: f32 = 32767.0;
/// Type of the annotation giving the size of the box of ink written here.
const FRAME: &str = "apexdeskFrame";

// --- reading -------------------------------------------------------------------------------

/// How the values of a trace are laid out.
#[derive(Clone)]
struct Format {
    /// Channel names in order.
    names: Vec<String>,
    /// Points per unit of X and Y.
    scale: (f32, f32),
    /// Maximum of the pressure channel.
    f_max: f32,
}

impl Default for Format {
    fn default() -> Self {
        Format { names: vec!["X".into(), "Y".into()], scale: (1.0 / UNITS_PT, 1.0 / UNITS_PT), f_max: F_MAX }
    }
}

/// Points per unit of a length written with `units` ("cm", "in"…).
fn unit_pt(units: &str) -> Option<f32> {
    Some(match units.trim() {
        "cm" => 72.0 / 2.54,
        "mm" => 72.0 / 25.4,
        "in" => 72.0,
        "pt" => 1.0,
        "himetric" => 72.0 / 2540.0,
        "px" | "dev" => 0.75,
        _ => return None,
    })
}

fn format_of(tf: &Node, props: Option<&Node>) -> Format {
    let mut f = Format { names: Vec::new(), ..Format::default() };
    let mut res: HashMap<String, f32> = HashMap::new();
    if let Some(props) = props {
        for p in props.children("channelProperty") {
            if p.attr("name") != Some("resolution") {
                continue;
            }
            let (Some(ch), Some(v)) = (p.attr("channel"), p.num("value")) else { continue };
            // "1/cm": so many units per centimeter.
            let per = p.attr("units").and_then(|u| u.trim().strip_prefix("1/")).and_then(unit_pt);
            if let Some(per) = per.filter(|_| v > 0.0) {
                res.insert(ch.to_string(), per / v as f32);
            }
        }
    }
    // Pens have a handful of channels (position, pressure, tilt, time).
    for c in tf.kids.iter().filter(|k| k.local() == "channel").take(32) {
        let name = c.attr("name").unwrap_or_default().to_string();
        let scale = res.get(&name).copied().or_else(|| c.attr("units").and_then(unit_pt)).unwrap_or(1.0 / UNITS_PT);
        match name.as_str() {
            "X" => f.scale.0 = scale,
            "Y" => f.scale.1 = scale,
            "F" => f.f_max = c.num("max").map_or(F_MAX, |m| m as f32).max(1.0),
            _ => {}
        }
        f.names.push(name);
    }
    if f.names.is_empty() {
        return Format::default();
    }
    f
}

/// A brush: look of the strokes drawn with it.
#[derive(Clone)]
struct Brush {
    tool: InkTool,
    color: u32,
    width: f32,
    pressure: bool,
}

impl Default for Brush {
    fn default() -> Self {
        Brush { tool: InkTool::Pen, color: 0, width: 1.5, pressure: true }
    }
}

fn brush_of(b: &Node) -> Brush {
    let mut out = Brush::default();
    let (mut mask, mut rect) = (false, false);
    let mut height = 0.0f32;
    for p in b.children("brushProperty") {
        let v = p.attr("value").unwrap_or_default().trim();
        let len = || v.parse::<f32>().ok().filter(|w| w.is_finite()).map(|w| w * p.attr("units").and_then(unit_pt).unwrap_or(72.0 / 2.54));
        match p.attr("name").unwrap_or_default() {
            "width" => out.width = len().unwrap_or(out.width),
            "height" => height = len().unwrap_or(0.0),
            "color" => {
                if let Some(c) = v.strip_prefix('#').and_then(|h| u32::from_str_radix(h.get(..6)?, 16).ok()) {
                    out.color = c;
                }
            }
            "tip" => rect = v == "rectangle",
            "rasterOp" => mask = v == "maskPen",
            "inkEffects" if v == "pencil" => out.tool = InkTool::Pencil,
            "ignorePressure" => out.pressure = !(v == "1" || v == "true"),
            _ => {}
        }
    }
    if mask || (rect && out.tool == InkTool::Pen) {
        // Office's highlighters are taller than wide: the height is the
        // width of the stroke.
        out.tool = InkTool::Highlighter;
        out.width = out.width.max(height);
    }
    out
}

fn id_of(r: Option<&str>) -> &str {
    r.unwrap_or_default().trim().trim_start_matches('#')
}

/// The values of a trace: one list per point. Values may be joined
/// without spaces and carry the InkML difference qualifiers (`!` explicit,
/// `'` first difference, `"` second difference), which hold for their
/// channel until another one comes.
fn parse_trace(text: &str, channels: usize) -> Vec<Vec<f32>> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mode {
        Abs,
        Diff,
        Diff2,
    }
    let n = channels.max(1);
    let mut mode = vec![Mode::Abs; n];
    let mut last = vec![0.0f64; n];
    let mut vel = vec![0.0f64; n];
    let mut out = Vec::new();
    for point in text.split(',') {
        let mut vals: Vec<f32> = Vec::with_capacity(n.min(32));
        let b = point.as_bytes();
        let mut i = 0;
        let mut ch = 0;
        while i < b.len() && ch < n {
            let c = b[i];
            if c.is_ascii_whitespace() {
                i += 1;
                continue;
            }
            let q = match c {
                b'!' => Some(Mode::Abs),
                b'\'' => Some(Mode::Diff),
                b'"' => Some(Mode::Diff2),
                _ => None,
            };
            if let Some(q) = q {
                mode[ch] = q;
                i += 1;
            }
            // A number: sign, digits, one point, exponent.
            let start = i;
            if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
                i += 1;
            }
            let mut dot = false;
            while i < b.len() && (b[i].is_ascii_digit() || (b[i] == b'.' && !dot)) {
                dot |= b[i] == b'.';
                i += 1;
            }
            if i < b.len() && (b[i] == b'e' || b[i] == b'E') && start < i {
                let save = i;
                i += 1;
                if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
                    i += 1;
                }
                if i < b.len() && b[i].is_ascii_digit() {
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                } else {
                    i = save;
                }
            }
            let tok = &point[start..i];
            let v = match tok {
                // "T"/"F" of boolean channels, "*" (unchanged), "?" (unknown).
                "" => {
                    let skip = matches!(b.get(i), Some(b'T' | b'F' | b'*' | b'?'));
                    i += 1;
                    if !skip {
                        continue;
                    }
                    let v = last[ch];
                    vals.push(v as f32);
                    ch += 1;
                    continue;
                }
                t => match t.parse::<f64>() {
                    Ok(v) if v.is_finite() => v,
                    _ => 0.0,
                },
            };
            let first = out.is_empty();
            let value = match mode[ch] {
                _ if first => v,
                Mode::Abs => v,
                Mode::Diff => last[ch] + v,
                Mode::Diff2 => last[ch] + vel[ch] + v,
            };
            vel[ch] = if first { 0.0 } else { value - last[ch] };
            last[ch] = value;
            vals.push(value as f32);
            ch += 1;
        }
        if !vals.is_empty() && vals.len() >= n.min(2) {
            out.push(vals);
        }
        if out.len() >= apex_doc::ink::MAX_POINTS {
            break;
        }
    }
    out
}

/// The strokes of an InkML part (points absolute, in points) with the
/// part kept to write back.
pub(crate) fn read(xml: &str) -> Ink {
    let root = drawingml::read_fragment(xml);
    let mut formats: HashMap<String, Format> = HashMap::new();
    let mut first: Option<Format> = None;
    let mut contexts = Vec::new();
    root.find_all("context", &mut contexts);
    for c in &contexts {
        let Some(tf) = c.find("traceFormat") else { continue };
        let f = format_of(tf, c.find("channelProperties"));
        first.get_or_insert_with(|| f.clone());
        formats.insert(id_of(c.attr("id")).to_string(), f);
    }
    if first.is_none() {
        if let Some(tf) = root.find("traceFormat") {
            first = Some(format_of(tf, root.find("channelProperties")));
        }
    }
    let base = first.unwrap_or_default();
    let mut brushes: HashMap<String, Brush> = HashMap::new();
    let mut list = Vec::new();
    root.find_all("brush", &mut list);
    let mut default_brush = None;
    for b in list {
        let br = brush_of(b);
        default_brush.get_or_insert_with(|| br.clone());
        brushes.insert(id_of(b.attr("id")).to_string(), br);
    }
    let default_brush = default_brush.unwrap_or_default();
    let mut strokes = Vec::new();
    collect(&root, None, None, &formats, &base, &brushes, &default_brush, &mut strokes);
    // Ink ApexDesk wrote says what box its points are in; other ink is
    // fitted to its box.
    let mut notes = Vec::new();
    root.find_all("annotation", &mut notes);
    let frame = notes.into_iter().find(|a| a.attr("type") == Some(FRAME)).and_then(|a| {
        let mut v = a.text.split_whitespace().map(|v| v.parse::<f32>().ok().filter(|v| v.is_finite() && *v > 0.0));
        Some((v.next()?? * base.scale.0, v.next()?? * base.scale.1))
    });
    let mut ink = match frame {
        Some((w, h)) => {
            let mut ink = Ink::new(strokes.into_iter().map(|s| InkStroke { points: s.points.iter().map(|&(x, y)| (x / w, y / h)).collect(), ..s }).collect());
            ink.sanitize();
            ink
        }
        None => Ink::from_abs(strokes, 1.0).map(|(ink, _)| ink).unwrap_or_default(),
    };
    ink.source = Some(Arc::new(InkSource { xml: xml.to_string(), signature: ink.signature() }));
    ink
}

/// Traces in `n` (and in its trace groups, which pass on their context
/// and brush).
#[allow(clippy::too_many_arguments)]
fn collect(n: &Node, ctx: Option<&str>, brush: Option<&str>, formats: &HashMap<String, Format>, base: &Format, brushes: &HashMap<String, Brush>, default_brush: &Brush, out: &mut Vec<InkStroke>) {
    for k in &n.kids {
        match k.local() {
            "traceGroup" => {
                let c = k.attr("contextRef").map(|v| id_of(Some(v))).or(ctx);
                let b = k.attr("brushRef").map(|v| id_of(Some(v))).or(brush);
                collect(k, c, b, formats, base, brushes, default_brush, out);
            }
            "trace" if out.len() < apex_doc::ink::MAX_STROKES => {
                let c = k.attr("contextRef").map(|v| id_of(Some(v))).or(ctx);
                let b = k.attr("brushRef").map(|v| id_of(Some(v))).or(brush);
                let f = c.and_then(|c| formats.get(c)).unwrap_or(base);
                let br = b.and_then(|b| brushes.get(b)).unwrap_or(default_brush);
                let (xi, yi) = (f.names.iter().position(|c| c == "X").unwrap_or(0), f.names.iter().position(|c| c == "Y").unwrap_or(1));
                let fi = f.names.iter().position(|c| c == "F");
                let mut s = InkStroke::new(br.tool, br.color, br.width);
                for v in parse_trace(&k.text, f.names.len()) {
                    let (Some(x), Some(y)) = (v.get(xi), v.get(yi)) else { continue };
                    s.points.push((x * f.scale.0, y * f.scale.1));
                    if let (Some(fi), true) = (fi, br.pressure) {
                        s.pressure.push(v.get(fi).map_or(1.0, |p| (p / f.f_max).clamp(0.0, 1.0)));
                    }
                }
                if s.pressure.len() != s.points.len() || s.pressure.iter().all(|p| *p == 0.0) {
                    s.pressure.clear();
                }
                if !s.points.is_empty() {
                    out.push(s);
                }
            }
            "definitions" | "annotation" | "annotationXML" => {}
            _ => collect(k, ctx, brush, formats, base, brushes, default_brush, out),
        }
    }
}

// --- writing -------------------------------------------------------------------------------

fn hex(c: u32) -> String {
    format!("{:06X}", c & 0xFF_FFFF)
}

/// An InkML part for ink shown in a `w` by `h` points box (the one read
/// from the file while the strokes are unchanged).
pub(crate) fn write(ink: &Ink, w: f32, h: f32) -> String {
    if let Some(x) = ink.kept_source() {
        return x.to_string();
    }
    let mut looks: Vec<(InkTool, u32, u32, bool)> = Vec::new();
    let look = |s: &InkStroke| (s.tool, s.color, s.width.to_bits(), s.has_pressure());
    for s in &ink.strokes {
        if !looks.contains(&look(s)) {
            looks.push(look(s));
        }
    }
    let (w, h) = (if w.is_finite() { w.max(0.0) } else { 0.0 }, if h.is_finite() { h.max(0.0) } else { 0.0 });
    let max = ((w.max(h) * UNITS_PT).ceil() as i64 + 1).max(32767);
    let mut x = String::from(r#"<inkml:ink xmlns:inkml="http://www.w3.org/2003/InkML"><inkml:definitions><inkml:context xml:id="ctx0"><inkml:inkSource xml:id="inkSrc0"><inkml:traceFormat>"#);
    let _ = write!(
        x,
        r#"<inkml:channel name="X" type="integer" max="{max}" units="cm"/><inkml:channel name="Y" type="integer" max="{max}" units="cm"/><inkml:channel name="F" type="integer" max="{}" units="dev"/></inkml:traceFormat><inkml:channelProperties><inkml:channelProperty channel="X" name="resolution" value="1000" units="1/cm"/><inkml:channelProperty channel="Y" name="resolution" value="1000" units="1/cm"/><inkml:channelProperty channel="F" name="resolution" value="0" units="1/dev"/></inkml:channelProperties></inkml:inkSource></inkml:context>"#,
        F_MAX as i64
    );
    for (i, &(tool, color, width, pressure)) in looks.iter().enumerate() {
        let cm = f32::from_bits(width) * 2.54 / 72.0;
        let (bw, bh) = (cm, cm);
        let _ = write!(x, r##"<inkml:brush xml:id="br{i}"><inkml:brushProperty name="width" value="{}" units="cm"/><inkml:brushProperty name="height" value="{}" units="cm"/><inkml:brushProperty name="color" value="#{}"/>"##, num(bw), num(bh), hex(color));
        match tool {
            InkTool::Highlighter => x.push_str(r#"<inkml:brushProperty name="tip" value="rectangle"/><inkml:brushProperty name="rasterOp" value="maskPen"/>"#),
            InkTool::Pencil => x.push_str(r#"<inkml:brushProperty name="inkEffects" value="pencil"/>"#),
            InkTool::Pen => {}
        }
        if !pressure {
            x.push_str(r#"<inkml:brushProperty name="ignorePressure" value="1"/>"#);
        }
        x.push_str("</inkml:brush>");
    }
    x.push_str("</inkml:definitions>");
    let _ = write!(x, r#"<inkml:annotation type="{FRAME}">{} {}</inkml:annotation>"#, (w * UNITS_PT).round().max(1.0) as i64, (h * UNITS_PT).round().max(1.0) as i64);
    for s in &ink.strokes {
        let b = looks.iter().position(|l| *l == look(s)).unwrap_or(0);
        let _ = write!(x, r##"<inkml:trace contextRef="#ctx0" brushRef="#br{b}">"##);
        for (i, &(px, py)) in s.points.iter().enumerate() {
            let f = s.pressure.get(i).filter(|_| s.has_pressure()).map_or(F_MAX / 2.0, |p| p * F_MAX);
            if i > 0 {
                x.push_str(", ");
            }
            let _ = write!(x, "{} {} {}", (px * w * UNITS_PT).round() as i64, (py * h * UNITS_PT).round() as i64, f.round() as i64);
        }
        x.push_str("</inkml:trace>");
    }
    x.push_str("</inkml:ink>");
    x
}

fn num(v: f32) -> String {
    let v = if v.is_finite() { v } else { 0.0 };
    format!("{}", (v * 10_000.0).round() / 10_000.0)
}

// --- shapes for programs without ink -------------------------------------------------------

/// The strokes grouped by look, each group drawn as one freeform shape.
pub(crate) fn looks(ink: &Ink) -> Vec<Vec<&InkStroke>> {
    let mut out: Vec<Vec<&InkStroke>> = Vec::new();
    for s in &ink.strokes {
        match out.iter_mut().find(|g| g[0].tool == s.tool && g[0].color == s.color && g[0].width == s.width) {
            Some(g) => g.push(s),
            None => out.push(vec![s]),
        }
    }
    out
}

/// The inside of the `spPr` of a freeform shape drawing `strokes` in a box
/// at `off` of `ext` (EMU).
pub(crate) fn write_sp_pr(out: &mut String, strokes: &[&InkStroke], off: (i64, i64), ext: (i64, i64)) {
    let (cx, cy) = (ext.0.max(1), ext.1.max(1));
    let _ = write!(out, r#"<a:xfrm><a:off x="{}" y="{}"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:custGeom><a:avLst/><a:gdLst/><a:ahLst/><a:cxnLst/><a:rect l="0" t="0" r="r" b="b"/><a:pathLst>"#, off.0, off.1);
    for s in strokes {
        let _ = write!(out, r#"<a:path w="{cx}" h="{cy}" fill="none">"#);
        let pt = |p: (f32, f32)| ((p.0 as f64 * cx as f64).round() as i64, (p.1 as f64 * cy as f64).round() as i64);
        for (i, &p) in s.points.iter().enumerate() {
            let (x, y) = pt(p);
            if i == 0 {
                let _ = write!(out, r#"<a:moveTo><a:pt x="{x}" y="{y}"/></a:moveTo>"#);
                if s.points.len() == 1 {
                    // A dot: a line of no length.
                    let _ = write!(out, r#"<a:lnTo><a:pt x="{x}" y="{y}"/></a:lnTo>"#);
                }
            } else {
                let _ = write!(out, r#"<a:lnTo><a:pt x="{x}" y="{y}"/></a:lnTo>"#);
            }
        }
        out.push_str("</a:path>");
    }
    out.push_str("</a:pathLst></a:custGeom><a:noFill/>");
    let Some(s) = strokes.first() else { return };
    let alpha = s.tool.alpha();
    let alpha = if alpha < 1.0 { format!(r#"<a:alpha val="{}"/>"#, (alpha * 100_000.0).round() as i64) } else { String::new() };
    let cap = if s.tool == InkTool::Highlighter { "flat" } else { "rnd" };
    let _ = write!(out, r#"<a:ln w="{}" cap="{cap}"><a:solidFill><a:srgbClr val="{}">{alpha}</a:srgbClr></a:solidFill><a:round/></a:ln>"#, (s.width * drawingml::EMU_PT).round() as i64, hex(s.color));
}

/// Ink from a freeform shape whose paths are all lines without fill (the
/// shapes written above, and scribbles drawn in other programs).
pub(crate) fn from_sp_pr(sp_pr: Option<&Node>) -> Option<Ink> {
    let sp_pr = sp_pr?;
    let geom = sp_pr.child("custGeom")?;
    let paths = geom.child("pathLst")?;
    let filled = sp_pr.child("noFill").is_none();
    let ln = sp_pr.child("ln");
    if ln.is_some_and(|l| l.child("noFill").is_some()) {
        return None;
    }
    let color = ln.and_then(|l| l.child("solidFill")).and_then(drawingml::color).unwrap_or(0);
    let width = ln.and_then(|l| l.num("w")).map_or(0.75, |w| (w / drawingml::EMU_PT as f64) as f32);
    let alpha = ln.and_then(|l| l.find("alpha")).and_then(|a| a.num("val")).map_or(1.0, |v| (v / 100_000.0) as f32);
    let tool = if alpha < 0.6 {
        InkTool::Highlighter
    } else if alpha < 0.95 {
        InkTool::Pencil
    } else {
        InkTool::Pen
    };
    let mut strokes = Vec::new();
    for p in paths.children("path") {
        if p.attr("fill").is_none_or(|f| f != "none") && filled {
            return None;
        }
        let (w, h) = (p.num("w").unwrap_or(0.0).max(1.0), p.num("h").unwrap_or(0.0).max(1.0));
        let mut s = InkStroke::new(tool, color, width);
        for c in &p.kids {
            match c.local() {
                "moveTo" => {
                    if s.points.len() > 1 {
                        strokes.push(std::mem::replace(&mut s, InkStroke::new(tool, color, width)));
                    } else {
                        s.points.clear();
                    }
                }
                "lnTo" | "cubicBezTo" | "quadBezTo" => {}
                "close" => {
                    if let Some(&f) = s.points.first() {
                        s.points.push(f);
                    }
                    continue;
                }
                _ => return None,
            }
            // Curves: their points, which lie close enough to them.
            for pt in c.children("pt") {
                let (x, y) = (pt.num("x").unwrap_or(0.0) / w, pt.num("y").unwrap_or(0.0) / h);
                s.points.push((x as f32, y as f32));
            }
        }
        if !s.points.is_empty() {
            strokes.push(s);
        }
    }
    if strokes.is_empty() {
        return None;
    }
    let mut ink = Ink::new(strokes);
    ink.sanitize();
    Some(ink)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ink as Word 365 writes it: joined values with difference
    /// qualifiers, a pressure channel, a highlighter brush.
    const WORD: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<inkml:ink xmlns:inkml="http://www.w3.org/2003/InkML"><inkml:definitions><inkml:context xml:id="ctx0"><inkml:inkSource xml:id="inkSrc0"><inkml:traceFormat><inkml:channel name="X" type="integer" max="32767" units="cm"/><inkml:channel name="Y" type="integer" max="32767" units="cm"/><inkml:channel name="F" type="integer" max="32767" units="dev"/><inkml:channel name="T" type="integer" max="2.14748E9" units="dev"/></inkml:traceFormat><inkml:channelProperties><inkml:channelProperty channel="X" name="resolution" value="1000" units="1/cm"/><inkml:channelProperty channel="Y" name="resolution" value="1000" units="1/cm"/><inkml:channelProperty channel="F" name="resolution" value="0" units="1/dev"/><inkml:channelProperty channel="T" name="resolution" value="1" units="1/dev"/></inkml:channelProperties></inkml:inkSource><inkml:timestamp xml:id="ts0" timeString="2024-05-02T10:11:12.345"/></inkml:context><inkml:brush xml:id="br0"><inkml:brushProperty name="width" value="0.05" units="cm"/><inkml:brushProperty name="height" value="0.05" units="cm"/><inkml:brushProperty name="color" value="#E71224"/></inkml:brush><inkml:brush xml:id="br1"><inkml:brushProperty name="width" value="0.1" units="cm"/><inkml:brushProperty name="height" value="0.3" units="cm"/><inkml:brushProperty name="color" value="#FFFF00"/><inkml:brushProperty name="tip" value="rectangle"/><inkml:brushProperty name="rasterOp" value="maskPen"/><inkml:brushProperty name="ignorePressure" value="1"/></inkml:brush></inkml:definitions><inkml:trace contextRef="#ctx0" brushRef="#br0">1000 2000 16000 0,'100'50'100 16,'0'0'0 16,"0"0"0 8</inkml:trace><inkml:traceGroup brushRef="#br1"><inkml:trace contextRef="#ctx0">3000 2000 0 40,'-500'0'0 8,-500'0 0 8</inkml:trace></inkml:traceGroup></inkml:ink>"##;

    #[test]
    fn reads_word_ink() {
        let ink = read(WORD);
        assert_eq!(ink.strokes.len(), 2);
        let (pen, marker) = (&ink.strokes[0], &ink.strokes[1]);
        assert_eq!((pen.tool, pen.color), (InkTool::Pen, 0xE71224));
        assert!((pen.width - 0.05 * 72.0 / 2.54).abs() < 1e-3);
        assert_eq!(pen.points.len(), 4);
        assert!(pen.has_pressure() && (pen.pressure[0] - 16000.0 / 32767.0).abs() < 1e-3);
        assert_eq!(marker.tool, InkTool::Highlighter);
        assert!(marker.pressure.is_empty());
        // The pen went 1000 -> 1100 -> 1100 -> 1100 (the second
        // difference of 0 after a still point).
        let xs: Vec<f32> = pen.points.iter().map(|p| p.0).collect();
        assert!(xs[1] > xs[0] && (xs[2] - xs[1]).abs() < 1e-5 && (xs[3] - xs[2]).abs() < 1e-5);
        // The marker went left twice: 3000, 2500, 2000 (the last value
        // is "-500" with the qualifier still in force).
        let mx: Vec<f32> = marker.points.iter().map(|p| p.0).collect();
        assert!(mx[0] > mx[1] && mx[1] > mx[2]);
        // Unchanged: written back as it was.
        assert_eq!(write(&ink, 100.0, 50.0), WORD);
    }

    #[test]
    fn values_joined_and_qualified() {
        let v = parse_trace("10 20, '1'-2, 3 4, !5 6", 2);
        assert_eq!(v, vec![vec![10.0, 20.0], vec![11.0, 18.0], vec![14.0, 22.0], vec![5.0, 28.0]]);
        let v = parse_trace("0 0,\"1\"1,2 2", 2);
        // Second differences: velocity 1 then 1+2.
        assert_eq!(v, vec![vec![0.0, 0.0], vec![1.0, 1.0], vec![4.0, 4.0]]);
        assert_eq!(parse_trace(".5.25 1e2", 3), vec![vec![0.5, 0.25, 100.0]]);
        assert_eq!(parse_trace("1 2 T, 3 4 F", 3).len(), 2);
        assert!(parse_trace("", 2).is_empty());
    }

    #[test]
    fn written_and_read_back() {
        let ink = Ink::new(vec![
            InkStroke { points: vec![(0.1, 0.2), (0.5, 0.9), (0.9, 0.3)], pressure: vec![0.2, 1.0, 0.5], ..InkStroke::new(InkTool::Pen, 0x0070C0, 2.0) },
            InkStroke { points: vec![(0.2, 0.5), (0.8, 0.5)], ..InkStroke::new(InkTool::Highlighter, 0xFFFF00, 12.0) },
            InkStroke { points: vec![(0.3, 0.3)], ..InkStroke::new(InkTool::Pencil, 0x404040, 1.0) },
        ]);
        let x = write(&ink, 200.0, 100.0);
        assert!(x.contains("maskPen") && x.contains("pencil") && x.contains("#0070C0"));
        let back = read(&x);
        assert_eq!(back.strokes.len(), 3);
        for (a, b) in ink.strokes.iter().zip(&back.strokes) {
            assert_eq!((a.tool, a.color, a.points.len(), a.has_pressure()), (b.tool, b.color, b.points.len(), b.has_pressure()));
            assert!((a.width - b.width).abs() < 0.01, "{} {}", a.width, b.width);
        }
        // Shapes for other programs, read back as ink.
        let mut sp = String::new();
        let groups = looks(&ink);
        assert_eq!(groups.len(), 3);
        write_sp_pr(&mut sp, &groups[0], (0, 0), (2_540_000, 1_270_000));
        let n = drawingml::read_fragment(&format!("<spPr>{sp}</spPr>"));
        let back = from_sp_pr(n.child("spPr")).unwrap();
        assert_eq!(back.strokes[0].points.len(), 3);
        assert!((back.strokes[0].points[1].1 - 0.9).abs() < 1e-4);
        assert_eq!((back.strokes[0].color, back.strokes[0].tool), (0x0070C0, InkTool::Pen));
        let mut sp = String::new();
        write_sp_pr(&mut sp, &groups[1], (0, 0), (100, 100));
        let n = drawingml::read_fragment(&format!("<spPr>{sp}</spPr>"));
        assert_eq!(from_sp_pr(n.child("spPr")).unwrap().strokes[0].tool, InkTool::Highlighter);
        // Filled freeforms are shapes, not ink.
        let n = drawingml::read_fragment(r#"<spPr><a:custGeom><a:pathLst><a:path w="10" h="10"><a:moveTo><a:pt x="0" y="0"/></a:moveTo><a:lnTo><a:pt x="10" y="10"/></a:lnTo></a:path></a:pathLst></a:custGeom><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></spPr>"#);
        assert!(from_sp_pr(n.child("spPr")).is_none());
    }

    #[test]
    fn hostile_ink() {
        for x in ["", "<inkml:ink/>", "<inkml:ink><inkml:trace>a b c, , ,--- 1e999 NaN</inkml:trace></inkml:ink>", &format!("<inkml:ink><inkml:trace>{}</inkml:trace></inkml:ink>", "1 1,".repeat(50_000))] {
            let ink = read(x);
            assert!(ink.strokes.iter().all(|s| s.points.len() <= apex_doc::ink::MAX_POINTS && s.points.iter().all(|p| p.0.is_finite() && p.1.is_finite())));
            assert_eq!(write(&ink, 10.0, 10.0), x, "kept as read");
        }
    }
}
