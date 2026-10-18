//! DrawingML gradients and see-through colors of documents and sheets
//! (shapes, text boxes, text effects): `a:gradFill` read into and written
//! from [`apex_doc::Gradient`]. Word's text effects (`w14:gradFill`) have
//! the same elements in another namespace, with namespaced attributes, so
//! both go through here with their prefix.

use std::fmt::Write as _;

use apex_doc::{GradKind, GradStop, Gradient};

use crate::drawingml::{self, Node};

/// The opacity of the first color child of `n` (its `alpha` change;
/// Word's `w14:alpha` is a transparency).
pub(crate) fn alpha(n: &Node) -> f32 {
    let Some(c) = n.kids.iter().find(|k| matches!(k.local(), "srgbClr" | "schemeClr" | "sysClr" | "prstClr" | "scrgbClr" | "hslClr")) else { return 1.0 };
    let v = c.child("alpha").and_then(|a| a.num("val")).map_or(1.0, |v| (v / 100_000.0).clamp(0.0, 1.0) as f32);
    if c.name.starts_with("w14:") && c.child("alpha").is_some() { 1.0 - v } else { v }
}

/// A gradient from a `gradFill` element.
pub(crate) fn read(n: &Node) -> Option<Gradient> {
    let mut stops = Vec::new();
    for gs in n.child("gsLst")?.children("gs") {
        let Some(color) = drawingml::color(gs) else { continue };
        stops.push(GradStop { pos: (gs.num("pos").unwrap_or(0.0) / 100_000.0) as f32, color, alpha: alpha(gs) });
    }
    if stops.is_empty() {
        return None;
    }
    let kind = GradKind::from_path(n.child("path").and_then(|p| p.attr("path")));
    // Path gradients have no angle (we keep 0 for them).
    let angle = n.child("lin").and_then(|l| l.num("ang")).map_or(if kind == GradKind::Linear { 90.0 } else { 0.0 }, |a| (a / 60_000.0) as f32);
    // Path gradients: the first stop is at the center (the fill-to rect),
    // as ours.
    let mut g = Gradient { kind, angle, stops };
    g.sanitize();
    Some(g)
}

/// A color element with its opacity: `<p:srgbClr val="…"/>` (Word's text
/// effects: `pre` "w14" also names the attributes).
pub(crate) fn write_color(out: &mut String, pre: &str, c: u32, alpha: f32) {
    let at = if pre == "a" { String::new() } else { format!("{pre}:") };
    let _ = write!(out, r#"<{pre}:srgbClr {at}val="{:06X}""#, c & 0xFF_FFFF);
    if alpha < 1.0 {
        let a = alpha.clamp(0.0, 1.0);
        let v = if pre == "w14" { 1.0 - a } else { a };
        let _ = write!(out, r#"><{pre}:alpha {at}val="{}"/></{pre}:srgbClr>"#, (v * 100_000.0).round() as i64);
    } else {
        out.push_str("/>");
    }
}

/// A `gradFill` element in namespace `pre` ("a" or "w14").
pub(crate) fn write(out: &mut String, pre: &str, g: &Gradient) {
    let at = if pre == "a" { String::new() } else { format!("{pre}:") };
    if pre == "a" {
        out.push_str(r#"<a:gradFill rotWithShape="1">"#);
    } else {
        let _ = write!(out, "<{pre}:gradFill>");
    }
    let _ = write!(out, "<{pre}:gsLst>");
    for s in &g.stops {
        let _ = write!(out, r#"<{pre}:gs {at}pos="{}">"#, (s.pos.clamp(0.0, 1.0) * 100_000.0).round() as i64);
        write_color(out, pre, s.color, s.alpha);
        let _ = write!(out, "</{pre}:gs>");
    }
    let _ = write!(out, "</{pre}:gsLst>");
    match g.kind.path() {
        Some(p) => {
            let _ = write!(out, r#"<{pre}:path {at}path="{p}"><{pre}:fillToRect {at}l="50000" {at}t="50000" {at}r="50000" {at}b="50000"/></{pre}:path>"#);
        }
        None => {
            let _ = write!(out, r#"<{pre}:lin {at}ang="{}" {at}scaled="0"/>"#, (g.angle * 60_000.0).round() as i64);
        }
    }
    let _ = write!(out, "</{pre}:gradFill>");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(pre: &str, g: &Gradient) -> Gradient {
        let mut s = String::new();
        write(&mut s, pre, g);
        let n = drawingml::read_fragment(&s);
        read(n.find("gradFill").unwrap_or(&n)).unwrap()
    }

    #[test]
    fn gradients_round_trip() {
        let mut g = Gradient::two(0x112233, 0xAABBCC, 45.0);
        g.stops[1].alpha = 0.25;
        g.add_stop(0.3);
        g.sanitize();
        for pre in ["a", "w14"] {
            let back = round(pre, &g);
            assert_eq!(back.stops.len(), 3);
            assert_eq!((back.kind, back.angle, back.stops[0].color, back.stops[2].alpha), (g.kind, g.angle, 0x112233, 0.25));
            assert!((back.stops[1].pos - 0.3).abs() < 1e-4);
        }
        let r = Gradient { kind: GradKind::Radial, ..g.clone() };
        let back = round("a", &r);
        assert_eq!((back.kind, back.stops[0].color, back.stops[2].color), (GradKind::Radial, 0x112233, 0xAABBCC));
        assert!((back.stops[1].pos - 0.3).abs() < 1e-4);
    }
}
