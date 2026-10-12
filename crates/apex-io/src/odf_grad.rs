//! Gradients in OpenDocument files (.odt, .ods, .odp): `draw:gradient`
//! with its start and end colors and LibreOffice's color stops
//! (`loext:gradient-stop`), and `draw:opacity` for their transparency
//! (with `loext:opacity-stop`), read into and written from
//! [`apex_doc::Gradient`].
//!
//! ODF turns the top to bottom axis counterclockwise; DrawingML (and we)
//! measure clockwise from left to right. Radial and rectangular gradients
//! start at the outside (ours at the center).

use std::fmt::Write as _;

use apex_doc::{GradKind, GradStop, Gradient};

use crate::odf_shape::hex_color;

/// A `draw:gradient` or `draw:opacity` as read: its attributes and stops
/// (offset, color or opacity).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Def {
    pub style: String,
    /// ODF degrees.
    pub angle: f32,
    /// 0..1 of the gradient kept at the start value.
    pub border: f32,
    pub start: u32,
    pub end: u32,
    /// Opacity definitions: start and end, 0..1.
    pub start_op: f32,
    pub end_op: f32,
    pub stops: Vec<(f32, u32)>,
    pub op_stops: Vec<(f32, f32)>,
}

fn angle(v: &str) -> f32 {
    let v = v.trim();
    let a = match v.strip_suffix("deg") {
        Some(d) => d.trim().parse::<f32>().unwrap_or(0.0),
        None => match v.strip_suffix("rad") {
            Some(r) => r.trim().parse::<f32>().unwrap_or(0.0).to_degrees(),
            None => match v.strip_suffix("grad") {
                Some(g) => g.trim().parse::<f32>().unwrap_or(0.0) * 0.9,
                // ODF 1.2 wrote tenths of a degree.
                None => v.parse::<f32>().unwrap_or(0.0) / 10.0,
            },
        },
    };
    if a.is_finite() { a } else { 0.0 }
}

fn percent(v: Option<&str>, default: f32) -> f32 {
    v.and_then(|v| v.trim().trim_end_matches('%').parse::<f32>().ok()).filter(|p| p.is_finite()).map_or(default, |p| (p / 100.0).clamp(0.0, 1.0))
}

impl Def {
    /// From the attributes of a `draw:gradient` or `draw:opacity`.
    pub(crate) fn of(get: &dyn Fn(&str) -> Option<String>) -> Def {
        let intensity = |c: u32, k: &str| {
            let i = percent(get(k).as_deref(), 1.0);
            if i >= 1.0 { c } else { apex_doc::gradient::tint(c, -(1.0 - i)) }
        };
        Def {
            style: get("draw:style").unwrap_or_else(|| "linear".into()),
            angle: get("draw:angle").map_or(0.0, |a| angle(&a)),
            border: percent(get("draw:border").as_deref(), 0.0),
            start: intensity(hex_color(get("draw:start-color").as_deref()).unwrap_or(0), "draw:start-intensity"),
            end: intensity(hex_color(get("draw:end-color").as_deref()).unwrap_or(0xFFFFFF), "draw:end-intensity"),
            start_op: percent(get("draw:start").as_deref(), 1.0),
            end_op: percent(get("draw:end").as_deref(), 1.0),
            stops: Vec::new(),
            op_stops: Vec::new(),
        }
    }

    /// Adds a `loext:gradient-stop` or `loext:opacity-stop`.
    pub(crate) fn stop(&mut self, get: &dyn Fn(&str) -> Option<String>) {
        // Far more than a gradient keeps (see `MAX_STOPS`).
        if self.stops.len() + self.op_stops.len() >= 256 {
            return;
        }
        let off = get("svg:offset").and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| v.is_finite()).unwrap_or(0.0).clamp(0.0, 1.0);
        if let Some(c) = hex_color(get("loext:color-value").as_deref()) {
            self.stops.push((off, c));
        } else if let Some(o) = get("svg:stop-opacity").and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| v.is_finite()) {
            self.op_stops.push((off, o.clamp(0.0, 1.0)));
        }
    }

    fn kind(&self) -> GradKind {
        match self.style.as_str() {
            "radial" | "ellipsoid" => GradKind::Radial,
            "square" | "rectangular" => GradKind::Rect,
            _ => GradKind::Linear,
        }
    }

    /// Places along ours of offsets along this one (0 at the start).
    fn place(&self, off: f32) -> Vec<f32> {
        let b = self.border.min(0.99);
        match self.style.as_str() {
            "linear" => vec![b + off * (1.0 - b)],
            // Start at both edges, end on the middle line.
            "axial" => {
                let h = (b + off * (1.0 - b)) / 2.0;
                if h >= 0.5 { vec![0.5] } else { vec![h, 1.0 - h] }
            }
            // Start at the outside, end at the center.
            _ => vec![(1.0 - off) * (1.0 - b)],
        }
    }

    /// The gradient this defines, with transparency `op` (a `draw:opacity`)
    /// or `uniform` opacity.
    pub(crate) fn gradient(&self, op: Option<&Def>, uniform: Option<f32>) -> Gradient {
        let stops: Vec<(f32, u32)> = if self.stops.len() >= 2 { self.stops.clone() } else { vec![(0.0, self.start), (1.0, self.end)] };
        let mut out = Vec::new();
        for (off, c) in stops {
            for pos in self.place(off) {
                out.push(GradStop { pos, color: c, alpha: 1.0 });
            }
        }
        // Where the border keeps the start value.
        if self.border > 0.0 {
            let (c, at) = (out.iter().min_by(|a, b| a.pos.total_cmp(&b.pos)).map(|s| s.color), 0.0);
            let edge = match self.style.as_str() {
                "linear" | "axial" => c.map(|c| GradStop::new(at, c)),
                _ => out.iter().min_by(|a, b| a.pos.total_cmp(&b.pos)).map(|s| GradStop::new(1.0, s.color)),
            };
            if let Some(e) = edge {
                out.push(e);
                if self.style == "axial" {
                    out.push(GradStop::new(1.0, e.color));
                }
            }
        }
        let mut g = Gradient { kind: self.kind(), angle: (90.0 - self.angle).rem_euclid(360.0), stops: out };
        g.sanitize();
        // Remove repeats (the same place and color).
        g.stops.dedup_by(|a, b| (a.pos - b.pos).abs() < 1e-4 && a.color == b.color);
        if g.stops.len() < 2 {
            g.sanitize();
        }
        if let Some(op) = op {
            let stops: Vec<(f32, f32)> = if op.op_stops.len() >= 2 { op.op_stops.clone() } else { vec![(0.0, op.start_op), (1.0, op.end_op)] };
            let mut places: Vec<(f32, f32)> = stops.iter().flat_map(|&(off, a)| op.place(off).into_iter().map(move |p| (p, a))).collect();
            places.sort_by(|a, b| a.0.total_cmp(&b.0));
            let at = |t: f32| {
                let Some(first) = places.first() else { return 1.0 };
                if t <= first.0 {
                    return first.1;
                }
                for w in places.windows(2) {
                    if t <= w[1].0 {
                        let k = if w[1].0 > w[0].0 { (t - w[0].0) / (w[1].0 - w[0].0) } else { 1.0 };
                        return w[0].1 + (w[1].1 - w[0].1) * k;
                    }
                }
                places.last().map_or(1.0, |l| l.1)
            };
            for s in &mut g.stops {
                s.alpha = at(s.pos);
            }
        } else if let Some(u) = uniform {
            for s in &mut g.stops {
                s.alpha = u.clamp(0.0, 1.0);
            }
        }
        g
    }
}

fn hex(c: u32) -> String {
    format!("#{:06x}", c & 0xFF_FFFF)
}

/// ODF style, angle and the stops as ODF offsets (0 at the start).
fn odf_of(g: &Gradient) -> (&'static str, f32, Vec<GradStop>) {
    let angle = (90.0 - g.angle).rem_euclid(360.0);
    let angle = if angle >= 359.95 { 0.0 } else { angle };
    match g.kind {
        GradKind::Linear => ("linear", angle, g.stops.clone()),
        k => {
            let stops = g.stops.iter().rev().map(|s| GradStop { pos: 1.0 - s.pos, ..*s }).collect();
            (if k == GradKind::Radial { "radial" } else { "rectangular" }, angle, stops)
        }
    }
}

/// The `draw:gradient` named `name` for `g`.
pub(crate) fn gradient_xml(name: &str, g: &Gradient) -> String {
    let (style, angle, stops) = odf_of(g);
    let (a, b) = (stops.first().map_or(0, |s| s.color), stops.last().map_or(0, |s| s.color));
    let mut s = format!(
        r#"<draw:gradient draw:name="{name}" draw:display-name="{name}" draw:style="{style}" draw:cx="50%" draw:cy="50%" draw:start-color="{}" draw:end-color="{}" draw:start-intensity="100%" draw:end-intensity="100%" draw:angle="{}deg" draw:border="0%">"#,
        hex(a),
        hex(b),
        (angle * 10.0).round() / 10.0
    );
    for st in &stops {
        let _ = write!(s, r#"<loext:gradient-stop svg:offset="{}" loext:color-type="rgb" loext:color-value="{}"/>"#, (st.pos * 10_000.0).round() / 10_000.0, hex(st.color));
    }
    s.push_str("</draw:gradient>");
    s
}

/// The `draw:opacity` named `name` for `g`'s transparency (none when it
/// is opaque).
pub(crate) fn opacity_xml(name: &str, g: &Gradient) -> Option<String> {
    if g.opaque() {
        return None;
    }
    let (style, angle, stops) = odf_of(g);
    let pc = |a: f32| (a.clamp(0.0, 1.0) * 100.0).round();
    let (a, b) = (stops.first().map_or(1.0, |s| s.alpha), stops.last().map_or(1.0, |s| s.alpha));
    let mut s = format!(r#"<draw:opacity draw:name="{name}" draw:display-name="{name}" draw:style="{style}" draw:cx="50%" draw:cy="50%" draw:start="{}%" draw:end="{}%" draw:angle="{}deg" draw:border="0%">"#, pc(a), pc(b), (angle * 10.0).round() / 10.0);
    for st in &stops {
        let _ = write!(s, r#"<loext:opacity-stop svg:offset="{}" svg:stop-opacity="{}"/>"#, (st.pos * 10_000.0).round() / 10_000.0, (st.alpha * 1000.0).round() / 1000.0);
    }
    s.push_str("</draw:opacity>");
    Some(s)
}

/// Named gradients and opacities a writer adds to `office:styles`, one
/// of each look.
#[derive(Default)]
pub(crate) struct Named {
    pub xml: String,
    names: std::collections::HashMap<String, String>,
}

impl Named {
    /// Graphic (or page) properties filling with `g`: the fill, the
    /// gradient's name and its transparency's.
    pub(crate) fn props(&mut self, g: &Gradient) -> String {
        let key = gradient_xml("", g);
        let n = self.names.len();
        let xml = &mut self.xml;
        let name = self
            .names
            .entry(key)
            .or_insert_with(|| {
                let name = format!("ApexGradient{}", n + 1);
                xml.push_str(&gradient_xml(&name, g));
                if let Some(o) = opacity_xml(&format!("{name}_T"), g) {
                    xml.push_str(&o);
                }
                name
            })
            .clone();
        let mut p = format!(r#" draw:fill="gradient" draw:fill-gradient-name="{name}" draw:fill-color="{}""#, hex(g.main_color()));
        if !g.opaque() {
            let _ = write!(p, r#" draw:opacity-name="{name}_T""#);
        }
        p
    }
}

/// Reads the `draw:gradient`s and `draw:opacity`s of a styles part.
pub(crate) fn read_all(xml: &str, grads: &mut std::collections::HashMap<String, Def>, ops: &mut std::collections::HashMap<String, Def>) {
    use crate::xml::{Ev, Reader};
    let mut r = Reader::new(xml);
    let mut cur: Option<(bool, String, Def)> = None;
    let finish = |c: Option<(bool, String, Def)>, grads: &mut std::collections::HashMap<String, Def>, ops: &mut std::collections::HashMap<String, Def>| {
        if let Some((op, name, d)) = c {
            if op { ops.insert(name, d) } else { grads.insert(name, d) };
        }
    };
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if matches!(t.name, "draw:gradient" | "draw:opacity") => {
                finish(cur.take(), grads, ops);
                let get = |k: &str| t.attr(k).map(|v| v.into_owned());
                let name = get("draw:name").unwrap_or_default();
                cur = Some((t.name == "draw:opacity", name, Def::of(&get)));
            }
            Ev::Start(t) | Ev::Empty(t) if matches!(t.name, "loext:gradient-stop" | "loext:opacity-stop") => {
                if let Some((_, _, d)) = &mut cur {
                    d.stop(&|k: &str| t.attr(k).map(|v| v.into_owned()));
                }
            }
            Ev::End("draw:gradient" | "draw:opacity") => finish(cur.take(), grads, ops),
            _ => {}
        }
    }
    finish(cur.take(), grads, ops);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn back(g: &Gradient) -> Gradient {
        let mut n = Named::default();
        let props = n.props(g);
        let (mut grads, mut ops) = (HashMap::new(), HashMap::new());
        read_all(&n.xml, &mut grads, &mut ops);
        let name = props.split("draw:fill-gradient-name=\"").nth(1).unwrap().split('"').next().unwrap();
        let op = props.split("draw:opacity-name=\"").nth(1).map(|v| v.split('"').next().unwrap());
        grads[name].gradient(op.and_then(|o| ops.get(o)), None)
    }

    #[test]
    fn gradients_round_trip() {
        let mut g = Gradient::two(0x112233, 0xAABBCC, 30.0);
        g.add_stop(0.4);
        g.stops[2].alpha = 0.5;
        let b = back(&g);
        assert_eq!((b.kind, b.angle, b.stops.len()), (GradKind::Linear, 30.0, 3));
        for (x, y) in b.stops.iter().zip(&g.stops) {
            assert!((x.pos - y.pos).abs() < 1e-3 && x.color == y.color && (x.alpha - y.alpha).abs() < 1e-3, "{b:?}");
        }
        let r = Gradient { kind: GradKind::Radial, ..g.clone() };
        let b = back(&r);
        assert_eq!(b.kind, GradKind::Radial);
        for (x, y) in b.stops.iter().zip(&r.stops) {
            assert!((x.pos - y.pos).abs() < 1e-3 && x.color == y.color && (x.alpha - y.alpha).abs() < 1e-3, "{b:?}");
        }
    }

    #[test]
    fn libreoffice_gradients() {
        // Two colors, an axial one and a border.
        let xml = r##"<office:styles><draw:gradient draw:name="A" draw:style="axial" draw:start-color="#000000" draw:end-color="#ffffff" draw:angle="900" draw:border="0%"/><draw:gradient draw:name="B" draw:style="linear" draw:start-color="#ff0000" draw:end-color="#0000ff" draw:angle="0deg" draw:border="50%"/><draw:opacity draw:name="T" draw:style="linear" draw:start="0%" draw:end="100%" draw:angle="0deg"/></office:styles>"##;
        let (mut grads, mut ops) = (HashMap::new(), HashMap::new());
        read_all(xml, &mut grads, &mut ops);
        let a = grads["A"].gradient(None, None);
        assert_eq!((a.angle, a.stops.len(), a.stops[0].color, a.stops[1].color), (0.0, 3, 0, 0xFFFFFF));
        let b = grads["B"].gradient(ops.get("T"), None);
        assert_eq!((b.angle, b.stops[0].color, b.stops[0].pos, b.stops[1].pos), (90.0, 0xFF0000, 0.0, 0.5));
        assert!(b.stops[0].alpha < 0.01 && b.stops.last().unwrap().alpha > 0.99);
    }
}
