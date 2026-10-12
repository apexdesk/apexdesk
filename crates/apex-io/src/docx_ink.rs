//! Ink in .docx, as Word 365 writes it: a `w14:contentPart` pointing at an
//! InkML part (see [`crate::inkml`]), in a `w:drawing` inside
//! `mc:AlternateContent` (`Requires="wpi"`), with freeform shapes as the
//! fallback for programs without ink.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{DocShape, Ink, Shape, ShapeWrap};

use crate::docx_img::Media;
use crate::drawingml::{EMU_PT, Node};
use crate::xml::esc;

const NS_MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const NS_WPI: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingInk";
const NS_WPS: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingShape";
const NS_WPG: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup";

fn emu(pt: f32) -> i64 {
    if pt.is_finite() { (pt as f64 * EMU_PT as f64).round() as i64 } else { 0 }
}

/// Whether an `mc:AlternateContent` (or a `w:drawing`) holds ink.
pub(crate) fn is_ink(root: &Node) -> bool {
    root.find("contentPart").is_some()
}

/// The ink of an `mc:AlternateContent` or `w:drawing` holding a content
/// part; `parts`: the package's parts by relationship id. Ink whose part is
/// missing is dropped; ink ApexDesk can't read is kept as it was.
pub(crate) fn read(root: &Node, parts: &HashMap<String, Arc<[u8]>>) -> Option<DocShape> {
    // The choice holding the content part, not the fallback.
    let choice = root.children("Choice").find(|c| c.find("contentPart").is_some()).unwrap_or(root);
    let part = choice.find("contentPart")?;
    let rid = part.attr("r:id").or_else(|| part.attr("id"))?;
    let data = parts.get(rid)?;
    let (xml, _) = crate::ooxml_keep::decode_xml(data);
    let ink = crate::inkml::read(&xml);
    let holder = choice.find("anchor").or_else(|| choice.find("inline"));
    let ext = |n: &Node| Some((n.num("cx")? as f32 / EMU_PT, n.num("cy")? as f32 / EMU_PT));
    let size = holder.and_then(|h| h.child("extent")).and_then(ext).or_else(|| part.child("xfrm").and_then(|x| x.child("ext")).and_then(ext)).unwrap_or((72.0, 72.0));
    let mut shape = Shape::ink(ink);
    if let Some(d) = holder.and_then(|h| h.child("docPr")) {
        shape.name = d.attr("name").unwrap_or("Ink").to_string();
        shape.alt = d.attr("descr").or_else(|| d.attr("title")).unwrap_or_default().to_string();
    }
    let mut s = DocShape::new(shape, size.0, size.1);
    match holder {
        Some(h) if h.local() == "anchor" => crate::docx_shape::place(h, &mut s),
        _ => s.wrap = ShapeWrap::Inline,
    }
    s.sanitize();
    Some(s)
}

/// An ink shape as the inside of a `w:r`.
pub(crate) fn write(out: &mut String, ds: &DocShape, ink: &Ink, media: &mut Media) {
    let (cx, cy) = (emu(ds.width), emu(ds.height));
    media.inks.push(crate::inkml::write(ink, ds.width, ds.height));
    let n = media.inks.len();
    let id = media.next_doc_id();
    let s = &ds.shape;
    let name = if s.name.is_empty() || s.name == "Ink" { format!("Ink {id}") } else { s.name.clone() };
    let mut doc_pr = format!(r#"<wp:docPr id="{id}" name="{}""#, esc(&name));
    if !s.alt.is_empty() {
        let _ = write!(doc_pr, r#" descr="{}""#, esc(&s.alt));
    }
    doc_pr.push_str("/>");
    let (holder_end, rot) = (if ds.wrap == ShapeWrap::Inline { "</wp:inline>" } else { "</wp:anchor>" }, (s.rotation * 60_000.0).round() as i64);
    let rot = if rot != 0 { format!(r#" rot="{rot}""#) } else { String::new() };
    let flips = format!("{}{}", if s.flip_h { r#" flipH="1""# } else { "" }, if s.flip_v { r#" flipV="1""# } else { "" });
    let _ = write!(out, r#"<mc:AlternateContent xmlns:mc="{NS_MC}"><mc:Choice xmlns:wpi="{NS_WPI}" Requires="wpi"><w:drawing>"#);
    crate::docx_shape::write_holder(out, ds, &doc_pr);
    let _ = write!(
        out,
        r#"<a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="{NS_WPI}"><w14:contentPart bwMode="auto" r:id="rIdInk{n}"><w14:nvContentPartPr><w14:cNvContentPartPr/></w14:nvContentPartPr><w14:xfrm{rot}{flips}><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></w14:xfrm></w14:contentPart></a:graphicData></a:graphic>{holder_end}</w:drawing></mc:Choice><mc:Fallback><w:drawing>"#
    );
    // Freeform shapes: one for each look of the strokes, grouped when
    // there are several.
    let fid = media.next_doc_id();
    crate::docx_shape::write_holder(out, ds, &format!(r#"<wp:docPr id="{fid}" name="{}"/>"#, esc(&name)));
    let looks = crate::inkml::looks(ink);
    let wsp = |out: &mut String, strokes: &[&apex_doc::InkStroke], id: Option<usize>| {
        out.push_str("<wps:wsp>");
        if let Some(id) = id {
            let _ = write!(out, r#"<wps:cNvPr id="{id}" name="{}"/>"#, esc(&name));
        }
        out.push_str("<wps:cNvSpPr/><wps:spPr>");
        let mut sp = String::new();
        crate::inkml::write_sp_pr(&mut sp, strokes, (0, 0), (cx, cy));
        if !rot.is_empty() || !flips.is_empty() {
            sp = sp.replacen("<a:xfrm>", &format!("<a:xfrm{rot}{flips}>"), 1);
        }
        out.push_str(&sp);
        out.push_str(r#"</wps:spPr><wps:bodyPr/></wps:wsp>"#);
    };
    if looks.len() > 1 {
        let _ = write!(
            out,
            r#"<a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="{NS_WPG}"><wpg:wgp xmlns:wpg="{NS_WPG}" xmlns:wps="{NS_WPS}"><wpg:cNvGrpSpPr/><wpg:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/><a:chOff x="0" y="0"/><a:chExt cx="{cx}" cy="{cy}"/></a:xfrm></wpg:grpSpPr>"#
        );
        for g in &looks {
            let id = media.next_doc_id();
            wsp(out, g, Some(id));
        }
        out.push_str("</wpg:wgp>");
    } else {
        let _ = write!(out, r#"<a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="{NS_WPS}">"#);
        wsp(out, looks.first().map_or(&[][..], |g| &g[..]), None);
    }
    let _ = write!(out, "</a:graphicData></a:graphic>{holder_end}</w:drawing></mc:Fallback></mc:AlternateContent>");
}

#[cfg(test)]
mod tests {
    use apex_doc::{Document, InkStroke, InkTool, Paragraph, Pos, Selection};

    use super::*;

    fn sample() -> Document {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("Notes by hand"), Paragraph::new("Second")]);
        let ink = Ink::new(vec![
            InkStroke { points: vec![(0.1, 0.1), (0.5, 0.8), (0.9, 0.2)], pressure: vec![0.3, 0.9, 0.5], ..InkStroke::new(InkTool::Pen, 0xE71224, 1.5) },
            InkStroke { points: vec![(0.05, 0.5), (0.95, 0.5)], ..InkStroke::new(InkTool::Highlighter, 0xFFFF00, 10.0) },
        ]);
        let mut s = DocShape::new(Shape::ink(ink), 120.0, 60.0);
        (s.x, s.y, s.z) = (30.0, 12.0, 3);
        d.insert_shape(Selection::caret(Pos::new(1, 0)), s);
        let one = Ink::new(vec![InkStroke { points: vec![(0.2, 0.2), (0.8, 0.8)], ..InkStroke::new(InkTool::Pencil, 0x404040, 1.0) }]);
        let mut inline = DocShape::new(Shape::ink(one), 40.0, 20.0);
        inline.wrap = ShapeWrap::Inline;
        d.insert_shape(Selection::caret(Pos::new(0, 5)), inline);
        d
    }

    #[test]
    fn docx_ink_round_trip() {
        let d = sample();
        let bytes = crate::docx::write(&d);
        let z = crate::zip::ZipReader::new(&bytes).unwrap();
        let body = z.read_string("word/document.xml").unwrap();
        assert!(body.contains(r#"Requires="wpi""#) && body.contains("<w14:contentPart") && body.contains("<mc:Fallback>") && body.contains("<a:custGeom>"));
        assert!(z.read_string("word/ink/ink1.xml").unwrap().contains("InkML"));
        let types = z.read_string("[Content_Types].xml").unwrap();
        assert!(types.contains("/word/ink/ink2.xml") && types.contains("application/inkml+xml"));
        let back = crate::docx::read(&bytes).unwrap();
        let (a, b) = (d.shapes(), back.shapes());
        assert_eq!(b.len(), 2);
        assert!(back.kept.parts.is_empty(), "ink isn't kept twice as custom XML");
        for ((pa, sa), (pb, sb)) in a.iter().zip(&b) {
            assert_eq!(pa, pb);
            assert_eq!((sa.wrap, sa.z), (sb.wrap, sb.z));
            assert!((sa.width - sb.width).abs() < 0.01 && (sa.x - sb.x).abs() < 0.01 && (sa.y - sb.y).abs() < 0.01);
            let (ia, ib) = (sa.shape.ink.as_ref().unwrap(), sb.shape.ink.as_ref().unwrap());
            assert_eq!(ia.strokes.len(), ib.strokes.len());
            for (x, y) in ia.strokes.iter().zip(&ib.strokes) {
                assert_eq!((x.tool, x.color, x.points.len(), x.has_pressure()), (y.tool, y.color, y.points.len(), y.has_pressure()));
                for (p, q) in x.points.iter().zip(&y.points) {
                    assert!((p.0 - q.0).abs() < 0.01 && (p.1 - q.1).abs() < 0.01, "{p:?} {q:?}");
                }
            }
        }
        // Saved again untouched: the InkML read is written back as it was.
        let again = crate::docx::write(&back);
        let z2 = crate::zip::ZipReader::new(&again).unwrap();
        assert_eq!(z2.read_string("word/ink/ink1.xml").unwrap(), z.read_string("word/ink/ink1.xml").unwrap());
    }

    #[test]
    fn unreadable_ink_is_kept() {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("x")]);
        let odd = "<inkml:ink xmlns:inkml=\"http://www.w3.org/2003/InkML\"><inkml:annotation>made elsewhere</inkml:annotation></inkml:ink>";
        let ink = crate::inkml::read(odd);
        assert!(ink.strokes.is_empty());
        d.insert_shape(Selection::caret(Pos::new(0, 0)), DocShape::new(Shape::ink(ink), 50.0, 50.0));
        let bytes = crate::docx::write(&d);
        let back = crate::docx::read(&bytes).unwrap();
        let again = crate::docx::write(&back);
        let z = crate::zip::ZipReader::new(&again).unwrap();
        assert_eq!(z.read_string("word/ink/ink1.xml").unwrap(), odd);
    }
}
