//! Shapes in .docx: a `wps:wsp` in a `w:drawing` (inline, or anchored to
//! its paragraph), written inside `mc:AlternateContent` as Word does. The
//! VML fallback isn't written.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{Align, DocShape, Document, HRel, Image, Shape, ShapeWrap, TextLink, VRel, Wrap};

use crate::docx_img::Media;
use crate::drawingml::{self, EMU_PT, Node};
use crate::xml::esc;

const NS_MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const NS_WPS: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingShape";
const NS_WPG: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup";
/// Word's `relativeHeight` for z = 0.
const Z_BASE: i64 = 251_658_240;

fn emu(pt: f32) -> i64 {
    if pt.is_finite() { (pt as f64 * EMU_PT as f64).round() as i64 } else { 0 }
}

fn pt(v: &str) -> Option<f32> {
    v.trim().parse::<f64>().ok().filter(|x| x.is_finite()).map(|x| (x / EMU_PT as f64) as f32)
}

/// The shape of a `w:drawing` (or of what holds one), if it has one.
/// `numbered(numId, level)`: see `drawingml::read_txbx`.
pub(crate) fn read(root: &Node, numbered: &dyn Fn(&str, u8) -> bool) -> Option<DocShape> {
    let holder = root.find("anchor").or_else(|| root.find("inline"))?;
    let wsp = holder.find("wsp")?;
    let sp_pr = wsp.child("spPr");
    let mut shape = read_wsp(wsp, numbered);
    if let Some(d) = holder.child("docPr") {
        shape.name = d.attr("name").unwrap_or_default().to_string();
        shape.alt = d.attr("descr").or_else(|| d.attr("title")).unwrap_or_default().to_string();
        shape.hidden = d.attr("hidden").is_some_and(|v| v == "1" || v == "true");
    }
    let size = holder.child("extent").and_then(|e| Some((pt(e.attr("cx")?)?, pt(e.attr("cy")?)?))).or_else(|| drawingml::xfrm_ext(sp_pr, EMU_PT)).unwrap_or((72.0, 72.0));
    let mut s = DocShape::new(shape, size.0, size.1);
    if holder.local() == "inline" {
        s.wrap = ShapeWrap::Inline;
        s.sanitize();
        return Some(s);
    }
    place(holder, &mut s);
    s.sanitize();
    Some(s)
}

/// The look and text of a `wps:wsp`.
fn read_wsp(wsp: &Node, numbered: &dyn Fn(&str, u8) -> bool) -> Shape {
    let sp_pr = wsp.child("spPr");
    let style = wsp.child("style");
    let text_box = wsp.child("cNvSpPr").and_then(|n| n.attr("txBox")).is_some_and(|v| v == "1" || v == "true");
    let mut shape = drawingml::read_look(sp_pr, style, text_box);
    if let Some(ink) = crate::inkml::from_sp_pr(sp_pr) {
        shape = Shape::ink(ink);
    }
    if let Some(content) = wsp.child("txbx").and_then(|t| t.find("txbxContent")) {
        drawingml::read_txbx(content, wsp.child("bodyPr"), style, &mut shape, numbered);
    }
    // Linked text boxes: the first holds the text (`txbx` with an id),
    // the others point to it.
    let id = |n: &Node, k: &str| n.attr(k).and_then(|v| v.trim().parse::<u32>().ok());
    if let Some(chain) = wsp.child("txbx").and_then(|t| id(t, "id")) {
        shape.link = Some(TextLink { chain, seq: 0 });
    } else if let Some(l) = wsp.child("linkedTxbx") {
        if let (Some(chain), Some(seq)) = (id(l, "id"), id(l, "seq")) {
            shape.link = Some(TextLink { chain, seq: seq.min(u16::MAX as u32) as u16 });
        }
    }
    if let Some(n) = wsp.child("cNvPr") {
        shape.name = n.attr("name").unwrap_or_default().to_string();
        shape.alt = n.attr("descr").unwrap_or_default().to_string();
    }
    shape
}

/// Maps child coordinates (EMU) of a group to points from the group's
/// corner: `x = ax * c + bx`.
#[derive(Clone, Copy)]
struct Xf {
    ax: f32,
    bx: f32,
    ay: f32,
    by: f32,
}

impl Xf {
    /// The mapping inside a group whose `xfrm` is `x`, placed by `self`.
    fn inside(self, x: &Node) -> Xf {
        let v = |n: &str, k: &str| x.child(n).and_then(|c| c.num(k)).map(|v| v as f32);
        let (ox, oy) = (v("off", "x").unwrap_or(0.0), v("off", "y").unwrap_or(0.0));
        let (cx, cy) = (v("chOff", "x").unwrap_or(ox), v("chOff", "y").unwrap_or(oy));
        let ratio = |e: Option<f32>, c: Option<f32>| match (e, c) {
            (Some(e), Some(c)) if c > 0.0 && e > 0.0 => e / c,
            _ => 1.0,
        };
        let (sx, sy) = (ratio(v("ext", "cx"), v("chExt", "cx")), ratio(v("ext", "cy"), v("chExt", "cy")));
        // Parent coordinate: o + (c - ch) * s.
        Xf { ax: self.ax * sx, bx: self.ax * (ox - cx * sx) + self.bx, ay: self.ay * sy, by: self.ay * (oy - cy * sy) + self.by }
    }
}

/// The shapes of a group (and of the groups in it), with their boxes in
/// points from the group's corner.
fn group_members(g: &Node, xf: Xf, media: &HashMap<String, Arc<[u8]>>, out: &mut Vec<(Shape, [f32; 4])>, numbered: &dyn Fn(&str, u8) -> bool) {
    for k in &g.kids {
        let sp_pr = k.child("spPr").or_else(|| k.child("grpSpPr"));
        let Some(x) = sp_pr.and_then(|p| p.child("xfrm")) else { continue };
        let v = |n: &str, a: &str| x.child(n).and_then(|c| c.num(a)).map(|v| v as f32).unwrap_or(0.0);
        let rect = [xf.ax * v("off", "x") + xf.bx, xf.ay * v("off", "y") + xf.by, xf.ax * v("ext", "cx"), xf.ay * v("ext", "cy")];
        match k.local() {
            "wsp" => out.push((read_wsp(k, numbered), rect)),
            "pic" => {
                let Some(data) = k.find("blip").and_then(|b| b.attr("r:embed")).and_then(|id| media.get(id)) else { continue };
                let mut shape = Shape::picture(Image::new(data.clone(), rect[2], rect[3]));
                if let Some(n) = k.find("cNvPr") {
                    shape.name = n.attr("name").unwrap_or_default().to_string();
                    shape.alt = n.attr("descr").unwrap_or_default().to_string();
                }
                shape.rotation = x.num("rot").map_or(0.0, |v| (v / 60_000.0) as f32);
                out.push((shape, rect));
            }
            "grpSp" => group_members(k, xf.inside(x), media, out, numbered),
            _ => {}
        }
    }
}

/// The shapes of a group of shapes anchored to its paragraph (`wpg:wgp`),
/// sharing a group id.
pub(crate) fn read_group(root: &Node, media: &HashMap<String, Arc<[u8]>>, numbered: &dyn Fn(&str, u8) -> bool) -> Vec<DocShape> {
    let Some(holder) = root.find("anchor") else { return Vec::new() };
    let Some(wgp) = holder.find("wgp") else { return Vec::new() };
    let mut base = DocShape::new(Shape::default(), 72.0, 72.0);
    place(holder, &mut base);
    let unit = 1.0 / EMU_PT;
    let top = Xf { ax: unit, bx: 0.0, ay: unit, by: 0.0 };
    // The group's own offset is where the anchor puts it.
    let xf = match wgp.child("grpSpPr").and_then(|p| p.child("xfrm")) {
        Some(x) => {
            let f = top.inside(x);
            let v = |n: &str, a: &str| x.child(n).and_then(|c| c.num(a)).map(|v| v as f32).unwrap_or(0.0);
            Xf { bx: f.bx - v("off", "x") * unit, by: f.by - v("off", "y") * unit, ..f }
        }
        None => top,
    };
    let mut members = Vec::new();
    group_members(wgp, xf, media, &mut members, numbered);
    let id = apex_doc::fresh_group_id();
    members
        .into_iter()
        .map(|(shape, [x, y, w, h])| {
            let mut d = DocShape { shape, width: w, height: h, x: base.x + x, y: base.y + y, h_align: None, v_align: None, group: Some(id), ..base.clone() };
            d.sanitize();
            d
        })
        .collect()
}

/// Groups written as one `wpg:wgp`: the members of each group when they
/// all float from the same frame in one paragraph.
pub(crate) fn group_plan(doc: &Document) -> HashMap<u32, Vec<Arc<DocShape>>> {
    let mut by: HashMap<u32, Vec<(apex_doc::Pos, Arc<DocShape>)>> = HashMap::new();
    for (p, s) in doc.shapes() {
        if let Some(g) = s.group {
            by.entry(g).or_default().push((p, s));
        }
    }
    by.into_iter()
        .filter(|(_, m)| {
            let f = &m[0];
            m.len() > 1 && m.iter().all(|(p, s)| p.para == f.0.para && s.wrap.floats() && s.h_align.is_none() && (s.wrap, s.h_rel, s.v_rel) == (f.1.wrap, f.1.h_rel, f.1.v_rel))
        })
        .map(|(g, m)| (g, m.into_iter().map(|(_, s)| s).collect()))
        .collect()
}

/// A picture anchored in front of or behind the text, as a floating
/// shape holding it (None for pictures in the text flow).
pub(crate) fn read_floating_picture(root: &Node, img: Image) -> Option<DocShape> {
    read_anchored_picture(root, img, false)
}

/// [`read_floating_picture`]; `free`: a picture text wraps around floats
/// too, at its own place (in headers and footers, where it only moves
/// their text).
pub(crate) fn read_anchored_picture(root: &Node, img: Image, free: bool) -> Option<DocShape> {
    let holder = root.find("anchor")?;
    let (w, h) = (img.width, img.height);
    let mut s = DocShape::new(Shape::picture(Image { wrap: Wrap::Inline, offset_x: 0.0, align: None, ..img }), w, h);
    place(holder, &mut s);
    if free && !s.wrap.floats() {
        let behind = holder.attr("behindDoc").is_some_and(|v| v == "1" || v == "true");
        s.wrap = if behind { ShapeWrap::Behind } else { ShapeWrap::InFront };
    }
    if !s.wrap.floats() {
        return None;
    }
    s.sanitize();
    Some(s)
}

/// Wrapping, position and stacking of an anchored shape.
pub(crate) fn place(holder: &Node, s: &mut DocShape) {
    let behind = holder.attr("behindDoc").is_some_and(|v| v == "1" || v == "true");
    s.wrap = if holder.child("wrapSquare").or_else(|| holder.child("wrapTight")).or_else(|| holder.child("wrapThrough")).is_some() {
        ShapeWrap::Square
    } else if holder.child("wrapTopAndBottom").is_some() {
        ShapeWrap::TopBottom
    } else if behind {
        ShapeWrap::Behind
    } else {
        ShapeWrap::InFront
    };
    // (A position in Word 2010's percentages has a fallback in points.)
    let position = |k: &str| holder.child(k).or_else(|| holder.child("AlternateContent").and_then(|a| a.child("Fallback")).and_then(|f| f.child(k)));
    if let Some(h) = position("positionH") {
        let from = h.attr("relativeFrom");
        s.h_rel = if matches!(from, Some("page") | Some("leftMargin")) { HRel::Page } else { HRel::Margin };
        s.x = h.child("posOffset").and_then(|o| pt(&o.text)).unwrap_or(0.0);
        s.h_align = h.child("align").map(|a| match a.text.trim() {
            "center" => Align::Center,
            "right" | "outside" => Align::Right,
            // (In the right margin: at the text's right edge, near enough.)
            _ if from == Some("rightMargin") => Align::Right,
            _ => Align::Left,
        });
    }
    if let Some(v) = position("positionV") {
        s.v_rel = match v.attr("relativeFrom") {
            Some("page") => VRel::Page,
            Some("margin") | Some("topMargin") | Some("bottomMargin") | Some("insideMargin") | Some("outsideMargin") => VRel::Margin,
            _ => VRel::Paragraph,
        };
        s.y = v.child("posOffset").and_then(|o| pt(&o.text)).unwrap_or(0.0);
        s.v_align = v.child("align").filter(|_| s.v_rel != VRel::Paragraph).map(|a| match a.text.trim() {
            "center" => Align::Center,
            "bottom" | "outside" => Align::Right,
            _ => Align::Left,
        });
    }
    if let Some(z) = holder.attr("relativeHeight").and_then(|v| v.trim().parse::<i64>().ok()) {
        // Word counts from Z_BASE; other programs from 0.
        let z = if z >= Z_BASE / 2 { z - Z_BASE } else { z };
        s.z = z.clamp(-1_000_000, 1_000_000) as i32;
    }
}

/// A shape as the inside of a `w:r`.
pub(crate) fn write(out: &mut String, ds: &DocShape, media: &mut Media) {
    if let Some(g) = ds.group
        && let Some(members) = media.groups.get(&g)
    {
        // The first member writes the group; the others are in it.
        if members.first().is_some_and(|m| std::ptr::eq(&**m, ds)) {
            let members = members.clone();
            write_group(out, &members, media);
        }
        return;
    }
    let s = &ds.shape;
    if let Some(img) = s.picture.as_ref().filter(|i| i.object.as_ref().is_some_and(|o| o.ole().is_some())) {
        return crate::docx_ole::write(out, img, Some(ds), media);
    }
    if let Some(ink) = &s.ink {
        return crate::docx_ink::write(out, ds, ink, media);
    }
    let (cx, cy) = (emu(ds.width), emu(ds.height));
    // A floating picture: a picture anchored like a shape.
    let pic = s.picture.as_ref().map(|img| media.pic_graphic(&Image { name: s.name.clone(), alt: s.alt.clone(), hidden: s.hidden, ..(**img).clone() }, cx, cy, s.rotation));
    let doc_pr = match &pic {
        Some((d, _, _)) => {
            out.push_str("<w:drawing>");
            d.clone()
        }
        None => {
            let id = media.next_doc_id();
            let name = if s.name.is_empty() { format!("{} {id}", if s.is_text_box() { "Text Box" } else { "Shape" }) } else { s.name.clone() };
            let _ = write!(out, r#"<mc:AlternateContent xmlns:mc="{NS_MC}"><mc:Choice xmlns:wps="{NS_WPS}" Requires="wps"><w:drawing>"#);
            let mut doc_pr = format!(r#"<wp:docPr id="{id}" name="{}""#, esc(&name));
            if !s.alt.is_empty() {
                let _ = write!(doc_pr, r#" descr="{}""#, esc(&s.alt));
            }
            if s.hidden {
                doc_pr.push_str(r#" hidden="1""#);
            }
            doc_pr.push_str("/>");
            doc_pr
        }
    };
    let inline = ds.wrap == ShapeWrap::Inline;
    write_holder(out, ds, &doc_pr);
    if let Some((_, graphic, _)) = pic {
        out.push_str(&graphic);
        out.push_str(if inline { "</wp:inline>" } else { "</wp:anchor>" });
        out.push_str("</w:drawing>");
        return;
    }
    let _ = write!(out, r#"<a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="{NS_WPS}">"#);
    write_wsp(out, s, (0, 0), (cx, cy), None, media);
    out.push_str("</a:graphicData></a:graphic>");
    out.push_str(if inline { "</wp:inline>" } else { "</wp:anchor>" });
    out.push_str("</w:drawing></mc:Choice></mc:AlternateContent>");
}

/// The opening of the `wp:inline` or `wp:anchor` of a shape.
pub(crate) fn write_holder(out: &mut String, ds: &DocShape, doc_pr: &str) {
    let (cx, cy) = (emu(ds.width), emu(ds.height));
    if ds.wrap == ShapeWrap::Inline {
        let _ = write!(out, r#"<wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{cx}" cy="{cy}"/><wp:effectExtent l="0" t="0" r="0" b="0"/>{doc_pr}<wp:cNvGraphicFramePr/>"#);
    } else {
        let rh = (Z_BASE + ds.z as i64).clamp(0, u32::MAX as i64);
        let behind = u8::from(ds.wrap == ShapeWrap::Behind);
        let _ = write!(
            out,
            r#"<wp:anchor distT="0" distB="0" distL="114300" distR="114300" simplePos="0" relativeHeight="{rh}" behindDoc="{behind}" locked="0" layoutInCell="1" allowOverlap="1"><wp:simplePos x="0" y="0"/>"#
        );
        // Shapes in the text flow sit in their column, on their paragraph.
        let (h_rel, v_rel) = if ds.wrap.floats() {
            (
                match ds.h_rel {
                    HRel::Page => "page",
                    HRel::Margin => "margin",
                },
                match ds.v_rel {
                    VRel::Paragraph => "paragraph",
                    VRel::Margin => "margin",
                    VRel::Page => "page",
                },
            )
        } else {
            ("column", "paragraph")
        };
        let _ = write!(out, r#"<wp:positionH relativeFrom="{h_rel}">"#);
        match ds.h_align {
            Some(a) => {
                let v = match a {
                    Align::Center => "center",
                    Align::Right => "right",
                    _ => "left",
                };
                let _ = write!(out, "<wp:align>{v}</wp:align>");
            }
            None => {
                let _ = write!(out, "<wp:posOffset>{}</wp:posOffset>", emu(ds.x));
            }
        }
        let y = if ds.wrap.floats() { emu(ds.y) } else { 0 };
        let v_pos = match ds.v_align.filter(|_| ds.wrap.floats() && ds.v_rel != VRel::Paragraph) {
            Some(Align::Center) => "<wp:align>center</wp:align>".to_string(),
            Some(Align::Right) => "<wp:align>bottom</wp:align>".to_string(),
            Some(_) => "<wp:align>top</wp:align>".to_string(),
            None => format!("<wp:posOffset>{y}</wp:posOffset>"),
        };
        let _ = write!(out, r#"</wp:positionH><wp:positionV relativeFrom="{v_rel}">{v_pos}</wp:positionV><wp:extent cx="{cx}" cy="{cy}"/><wp:effectExtent l="0" t="0" r="0" b="0"/>"#);
        out.push_str(match ds.wrap {
            ShapeWrap::Square => r#"<wp:wrapSquare wrapText="bothSides"/>"#,
            ShapeWrap::TopBottom => "<wp:wrapTopAndBottom/>",
            _ => "<wp:wrapNone/>",
        });
        let _ = write!(out, "{doc_pr}<wp:cNvGraphicFramePr/>");
    }
}

/// A group of shapes floating together, as one `wpg:wgp`.
fn write_group(out: &mut String, members: &[Arc<DocShape>], media: &mut Media) {
    let x0 = members.iter().map(|m| m.x).fold(f32::INFINITY, f32::min);
    let y0 = members.iter().map(|m| m.y).fold(f32::INFINITY, f32::min);
    let x1 = members.iter().map(|m| m.x + m.width).fold(f32::NEG_INFINITY, f32::max);
    let y1 = members.iter().map(|m| m.y + m.height).fold(f32::NEG_INFINITY, f32::max);
    let z = members.iter().map(|m| m.z).max().unwrap_or(0);
    let frame = DocShape { x: x0, y: y0, width: x1 - x0, height: y1 - y0, z, h_align: None, v_align: None, ..(*members[0]).clone() };
    let (cx, cy) = (emu(frame.width), emu(frame.height));
    let id = media.next_doc_id();
    let _ = write!(out, r#"<mc:AlternateContent xmlns:mc="{NS_MC}"><mc:Choice xmlns:wpg="{NS_WPG}" xmlns:wps="{NS_WPS}" Requires="wpg"><w:drawing>"#);
    write_holder(out, &frame, &format!(r#"<wp:docPr id="{id}" name="Group {id}"/>"#));
    let _ = write!(
        out,
        r#"<a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="{NS_WPG}"><wpg:wgp><wpg:cNvGrpSpPr/><wpg:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/><a:chOff x="0" y="0"/><a:chExt cx="{cx}" cy="{cy}"/></a:xfrm></wpg:grpSpPr>"#
    );
    for m in members {
        let (off, ext) = ((emu(m.x - x0), emu(m.y - y0)), (emu(m.width), emu(m.height)));
        let s = &m.shape;
        match &s.picture {
            Some(img) => {
                let (_, graphic, _) = media.pic_graphic(&Image { name: s.name.clone(), alt: s.alt.clone(), hidden: s.hidden, ..(**img).clone() }, ext.0, ext.1, s.rotation);
                if let (Some(a), Some(b)) = (graphic.find("<pic:pic"), graphic.find("</pic:pic>")) {
                    out.push_str(&graphic[a..b + "</pic:pic>".len()].replacen(r#"<a:off x="0" y="0"/>"#, &format!(r#"<a:off x="{}" y="{}"/>"#, off.0, off.1), 1));
                }
            }
            None => {
                let id = media.next_doc_id();
                write_wsp(out, s, off, ext, Some(id), media);
            }
        }
    }
    out.push_str("</wpg:wgp></a:graphicData></a:graphic></wp:anchor></w:drawing></mc:Choice></mc:AlternateContent>");
}

/// A `wps:wsp`; `id`: its own id and name, in a group.
fn write_wsp(out: &mut String, s: &Shape, off: (i64, i64), ext: (i64, i64), id: Option<usize>, media: &mut Media) {
    out.push_str("<wps:wsp>");
    if let Some(id) = id {
        let name = if s.name.is_empty() { format!("{} {id}", if s.is_text_box() { "Text Box" } else { "Shape" }) } else { s.name.clone() };
        let _ = write!(out, r#"<wps:cNvPr id="{id}" name="{}""#, esc(&name));
        if !s.alt.is_empty() {
            let _ = write!(out, r#" descr="{}""#, esc(&s.alt));
        }
        out.push_str("/>");
    }
    let _ = write!(out, r#"<wps:cNvSpPr{}/><wps:spPr>"#, if s.is_text_box() { r#" txBox="1""# } else { "" });
    match &s.ink {
        // Ink in a group: a freeform of its strokes (of their first look).
        Some(ink) => crate::inkml::write_sp_pr(out, &ink.strokes.iter().collect::<Vec<_>>(), off, ext),
        None => drawingml::write_sp_pr(out, s, off, ext),
    }
    out.push_str("</wps:spPr>");
    drawingml::write_style(out, "wps:style", s);
    match s.link {
        Some(l) if l.seq > 0 => {
            let _ = write!(out, r#"<wps:linkedTxbx id="{}" seq="{}"/>"#, l.chain, l.seq);
        }
        _ if s.can_have_text() && (s.has_text() || s.is_text_box()) => {
            match s.link {
                Some(l) => {
                    let _ = write!(out, r#"<wps:txbx id="{}"><w:txbxContent>"#, l.chain);
                }
                None => out.push_str("<wps:txbx><w:txbxContent>"),
            }
            drawingml::write_txbx_paras(out, s, &mut |n| media.shape_list_num(n));
            out.push_str("</w:txbxContent></wps:txbx>");
        }
        _ => {}
    }
    let _ = write!(
        out,
        r#"<wps:bodyPr rot="0" vert="{}" wrap="square" lIns="91440" tIns="45720" rIns="91440" bIns="45720" anchor="{}" anchorCtr="0"><a:noAutofit/></wps:bodyPr></wps:wsp>"#,
        crate::xml::esc(s.vert.as_deref().unwrap_or("horz")),
        drawingml::anchor_value(s.valign)
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_word_groups() {
        // A group scaled by half, with a nested group.
        let xml = r#"<w:drawing xmlns:w="w" xmlns:wp="wp" xmlns:a="a" xmlns:wpg="wpg" xmlns:wps="wps"><wp:anchor behindDoc="0" relativeHeight="251658240"><wp:positionH relativeFrom="margin"><wp:posOffset>127000</wp:posOffset></wp:positionH><wp:positionV relativeFrom="paragraph"><wp:posOffset>0</wp:posOffset></wp:positionV><wp:extent cx="1270000" cy="635000"/><wp:wrapNone/><wp:docPr id="1" name="Group 1"/><a:graphic><a:graphicData><wpg:wgp><wpg:grpSpPr><a:xfrm><a:off x="127000" y="0"/><a:ext cx="1270000" cy="635000"/><a:chOff x="0" y="0"/><a:chExt cx="2540000" cy="1270000"/></a:xfrm></wpg:grpSpPr><wps:wsp><wps:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1270000" cy="1270000"/></a:xfrm><a:prstGeom prst="rect"/></wps:spPr></wps:wsp><wpg:grpSp><wpg:grpSpPr><a:xfrm><a:off x="1270000" y="0"/><a:ext cx="1270000" cy="1270000"/><a:chOff x="0" y="0"/><a:chExt cx="1270000" cy="1270000"/></a:xfrm></wpg:grpSpPr><wps:wsp><wps:spPr><a:xfrm><a:off x="635000" y="635000"/><a:ext cx="635000" cy="635000"/></a:xfrm><a:prstGeom prst="ellipse"/></wps:spPr></wps:wsp></wpg:grpSp></wpg:wgp></a:graphicData></a:graphic></wp:anchor></w:drawing>"#;
        let v = read_group(&crate::drawingml::read_fragment(xml), &HashMap::new(), &|_, _| false);
        let boxes: Vec<[i32; 4]> = v.iter().map(|d| [d.x, d.y, d.width, d.height].map(|f| f.round() as i32)).collect();
        // 1 EMU scale: 12700 per point; group at 10pt, children halved.
        assert_eq!(boxes, vec![[10, 0, 50, 50], [85, 25, 25, 25]]);
        assert!(v.iter().all(|d| d.group.is_some() && d.group == v[0].group));
    }

    #[test]
    fn positions_in_percent_use_their_fallback() {
        let xml = r#"<wp:anchor xmlns:wp="wp" xmlns:mc="mc" xmlns:wp14="wp14"><wp:positionH relativeFrom="rightMargin"><wp:align>left</wp:align></wp:positionH><mc:AlternateContent><mc:Choice Requires="wp14"><wp:positionV relativeFrom="bottomMargin"><wp14:pctPosVOffset>7000</wp14:pctPosVOffset></wp:positionV></mc:Choice><mc:Fallback><wp:positionV relativeFrom="page"><wp:posOffset>9855200</wp:posOffset></wp:positionV></mc:Fallback></mc:AlternateContent><wp:wrapNone/></wp:anchor>"#;
        let mut s = DocShape::new(Shape::new(apex_doc::ShapeKind::Rect), 10.0, 10.0);
        place(crate::drawingml::read_fragment(xml).find("anchor").unwrap(), &mut s);
        assert_eq!((s.v_rel, s.y.round()), (VRel::Page, 776.0));
        assert_eq!((s.h_rel, s.h_align), (HRel::Margin, Some(Align::Right)));
    }
}
