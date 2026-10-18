//! Artistic text effects (`apex_doc::TextFx`) in DrawingML: Word's `w14`
//! run properties (`w14:glow`, `shadow`, `reflection`, `textOutline`,
//! `textFill`, `props3d`) and a DrawingML run's `a:ln`, fill and
//! `a:effectLst` (presentations, sheet shapes). Word's `w14:alpha` is a
//! transparency where DrawingML's `a:alpha` is an opacity (see
//! `dml_fill`).

use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::text_fx::{TextGlow, TextOutline, TextReflection, TextShadow, hundredths};
use apex_doc::{Dash, TextFill, TextFx};

use crate::drawingml::{self, Node};

const EMU_PT: f64 = 12_700.0;

fn pt(n: &Node, key: &str) -> f32 {
    n.num(key).map_or(0.0, |v| (v / EMU_PT) as f32)
}

/// Hundredths of a point in EMU.
fn emu(h: u16) -> i64 {
    (f64::from(h) / 100.0 * EMU_PT).round() as i64
}

/// The color and opacity (0..255) of an element holding a color.
fn color_alpha(n: &Node) -> Option<(u32, u8)> {
    let c = drawingml::color(n)?;
    Some((c, (crate::dml_fill::alpha(n) * 255.0).round() as u8))
}

fn dash_of(v: &str) -> Dash {
    match v {
        "sysDot" | "dot" => Dash::Dot,
        "dash" | "sysDash" | "lgDash" => Dash::Dash,
        "dashDot" | "sysDashDot" | "lgDashDot" | "lgDashDotDot" | "sysDashDotDot" => Dash::DashDot,
        _ => Dash::Solid,
    }
}

fn dash_name(d: Dash) -> &'static str {
    match d {
        Dash::Dot => "sysDot",
        Dash::Dash => "dash",
        Dash::DashDot => "dashDot",
        _ => "solid",
    }
}

/// A fill element (`solidFill`, `gradFill`, `noFill`) as the letters'
/// fill. `word`: a plain solid color counts too (DrawingML runs keep
/// theirs as the run color unless it is see-through).
fn read_fill(n: &Node, fx: &mut TextFx, word: bool) -> bool {
    match n.local() {
        "noFill" => fx.fill = TextFill::None,
        "solidFill" => {
            if let Some((c, a)) = color_alpha(n) {
                if word || a < 255 {
                    fx.fill = TextFill::Solid(c, a);
                }
            }
        }
        "gradFill" => {
            if let Some(g) = crate::dml_fill::read(n) {
                fx.fill = TextFill::Gradient(g);
            }
        }
        _ => return false,
    }
    true
}

/// Reads effect element `n` (a child of `w:rPr` or `a:rPr`) into `fx`;
/// false when it isn't one.
pub(crate) fn read(n: &Node, fx: &mut TextFx) -> bool {
    let word = n.name.starts_with("w14:");
    match n.local() {
        "textFill" => {
            for k in &n.kids {
                read_fill(k, fx, true);
            }
        }
        "solidFill" | "gradFill" | "noFill" => return read_fill(n, fx, word),
        "textOutline" | "ln" => {
            if n.child("noFill").is_some() {
                fx.outline = None;
            } else {
                let fill = n.child("solidFill").and_then(color_alpha).or_else(|| n.child("gradFill").and_then(|g| g.find("gs")).and_then(color_alpha));
                let (color, alpha) = fill.unwrap_or((0, 255));
                let width = if n.attr("w").is_some() { pt(n, "w") } else { 0.75 };
                let dash = n.child("prstDash").and_then(|d| d.attr("val")).map_or(Dash::Solid, dash_of);
                fx.outline = Some(TextOutline { color, alpha, width: hundredths(width), dash });
            }
        }
        // (`w:shadow` is Word's old on/off shadow.)
        "shadow" | "outerShdw" if !n.name.starts_with("w:") => {
            let (color, alpha) = color_alpha(n).unwrap_or((0, 255));
            let angle = n.num("dir").map_or(0.0, |d| d / 60_000.0).rem_euclid(360.0).round() as u16;
            fx.shadow = Some(TextShadow::new(color, alpha, pt(n, "blurRad"), pt(n, "dist"), angle));
        }
        "glow" => {
            if let Some((color, alpha)) = color_alpha(n) {
                fx.glow = Some(TextGlow::new(color, alpha, pt(n, "rad")));
            }
        }
        "reflection" => {
            let st = n.num("stA").map_or(0.5, |v| v / 100_000.0).clamp(0.0, 1.0);
            let size = n.num("endPos").map_or(90.0, |v| v / 1000.0).clamp(1.0, 100.0);
            fx.reflection = Some(TextReflection::new((st * 255.0).round() as u8, pt(n, "dist"), size.round() as u8));
        }
        "props3d" | "sp3d" => fx.bevel = n.child("bevelT").is_some(),
        "effectLst" => {
            for k in &n.kids {
                read(k, fx);
            }
        }
        _ => return false,
    }
    true
}

/// Reads the effects among the children of a run's properties.
pub(crate) fn read_all(rpr: &Node) -> Option<Arc<TextFx>> {
    let mut fx = TextFx::default();
    for k in &rpr.kids {
        read(k, &mut fx);
    }
    (!fx.is_empty()).then(|| Arc::new(fx))
}

/// A color element with an opacity in namespace `pre` ("a", "w14").
fn color(out: &mut String, pre: &str, c: u32, alpha: u8) {
    crate::dml_fill::write_color(out, pre, c, f32::from(alpha) / 255.0);
}

fn fill(out: &mut String, pre: &str, f: &TextFill) {
    match f {
        TextFill::Auto => {}
        TextFill::None => {
            let _ = write!(out, "<{pre}:noFill/>");
        }
        TextFill::Solid(c, a) => {
            let _ = write!(out, "<{pre}:solidFill>");
            color(out, pre, *c, *a);
            let _ = write!(out, "</{pre}:solidFill>");
        }
        TextFill::Gradient(g) => crate::dml_fill::write(out, pre, g),
    }
}

/// An outline: `tag` is `w14:textOutline` or `a:ln`.
fn outline(out: &mut String, pre: &str, tag: &str, o: &TextOutline) {
    let at = if pre == "a" { "" } else { "w14:" };
    let _ = write!(out, r#"<{tag} {at}w="{}" {at}cap="flat" {at}cmpd="sng" {at}algn="ctr"><{pre}:solidFill>"#, emu(o.width));
    color(out, pre, o.color, o.alpha);
    let _ = write!(out, r#"</{pre}:solidFill><{pre}:prstDash {at}val="{}"/><{pre}:round/></{tag}>"#, dash_name(o.dash));
}

fn shadow(out: &mut String, pre: &str, tag: &str, s: &TextShadow) {
    let at = if pre == "a" { "" } else { "w14:" };
    let _ = write!(out, r#"<{tag} {at}blurRad="{}" {at}dist="{}" {at}dir="{}""#, emu(s.blur), emu(s.dist), i64::from(s.angle) * 60_000);
    if pre != "a" {
        out.push_str(r#" w14:sx="100000" w14:sy="100000" w14:kx="0" w14:ky="0""#);
    }
    let _ = write!(out, r#" {at}algn="tl"{}>"#, if pre == "a" { r#" rotWithShape="0""# } else { "" });
    color(out, pre, s.color, s.alpha);
    let _ = write!(out, "</{tag}>");
}

fn glow(out: &mut String, pre: &str, g: &TextGlow) {
    let at = if pre == "a" { "" } else { "w14:" };
    let _ = write!(out, r#"<{pre}:glow {at}rad="{}">"#, emu(g.radius));
    color(out, pre, g.color, g.alpha);
    let _ = write!(out, "</{pre}:glow>");
}

fn reflection(out: &mut String, pre: &str, r: &TextReflection) {
    let at = if pre == "a" { "" } else { "w14:" };
    let st = (f64::from(r.alpha) / 255.0 * 100_000.0).round() as i64;
    let _ = write!(
        out,
        r#"<{pre}:reflection {at}blurRad="6350" {at}stA="{st}" {at}stPos="0" {at}endA="300" {at}endPos="{}" {at}dist="{}" {at}dir="5400000" {at}fadeDir="5400000" {at}sx="100000" {at}sy="-100000" {at}kx="0" {at}ky="0" {at}algn="bl"{}/>"#,
        i64::from(r.size) * 1000,
        emu(r.dist),
        if pre == "a" { r#" rotWithShape="0""# } else { "" }
    );
}

/// Word's run effects, in `w:rPr` order (they go after the `w:` ones).
pub(crate) fn write_w14(out: &mut String, fx: &TextFx) {
    if let Some(g) = &fx.glow {
        glow(out, "w14", g);
    }
    if let Some(s) = &fx.shadow {
        shadow(out, "w14", "w14:shadow", s);
    }
    if let Some(r) = &fx.reflection {
        reflection(out, "w14", r);
    }
    if let Some(o) = &fx.outline {
        outline(out, "w14", "w14:textOutline", o);
    }
    if fx.fill != TextFill::Auto {
        out.push_str("<w14:textFill>");
        fill(out, "w14", &fx.fill);
        out.push_str("</w14:textFill>");
    }
    if fx.bevel {
        out.push_str(r#"<w14:scene3d><w14:camera w14:prst="orthographicFront"/><w14:lightRig w14:rig="threePt" w14:dir="t"><w14:rot w14:lat="0" w14:lon="0" w14:rev="0"/></w14:lightRig></w14:scene3d><w14:props3d w14:extrusionH="0" w14:contourW="0" w14:prstMaterial="warmMatte"><w14:bevelT w14:w="38100" w14:h="38100" w14:prst="circle"/></w14:props3d>"#);
    }
}

/// Puts the effects into a DrawingML run's properties `rpr` (a complete
/// `<a:rPr …/>` or `<a:rPr …>…</a:rPr>`, any tag name): the outline
/// first, the fill in place of the run's color, the effects after it, as
/// the schema orders them. DrawingML runs have no bevel of their own.
pub(crate) fn into_dml(rpr: &str, fx: &TextFx) -> String {
    let Some(open_end) = rpr.find('>') else { return rpr.to_string() };
    let tag: String = rpr[1..].chars().take_while(|c| !c.is_whitespace() && *c != '/' && *c != '>').collect();
    // Children, and the element opened.
    let (open, kids) = if rpr[..open_end].ends_with('/') { (format!("{}>", &rpr[..open_end - 1]), String::new()) } else { (rpr[..=open_end].to_string(), rpr[open_end + 1..].trim_end_matches(&format!("</{tag}>")).to_string()) };
    let mut ln = String::new();
    if let Some(o) = &fx.outline {
        outline(&mut ln, "a", "a:ln", o);
    }
    let mut kids = kids;
    let mut fill_xml = String::new();
    fill(&mut fill_xml, "a", &fx.fill);
    // The run's own fill goes where the effect's is.
    if !fill_xml.is_empty() {
        if let (Some(a), Some(b)) = (kids.find("<a:solidFill>"), kids.find("</a:solidFill>")) {
            if a < b {
                kids.replace_range(a..b + "</a:solidFill>".len(), "");
            }
        }
    }
    let at = match (kids.find("<a:solidFill>"), kids.find("</a:solidFill>")) {
        (Some(0), Some(b)) => b + "</a:solidFill>".len(),
        _ => 0,
    };
    let mut eff = String::new();
    if fx.shadow.is_some() || fx.glow.is_some() || fx.reflection.is_some() {
        eff.push_str("<a:effectLst>");
        if let Some(g) = &fx.glow {
            glow(&mut eff, "a", g);
        }
        if let Some(s) = &fx.shadow {
            shadow(&mut eff, "a", "a:outerShdw", s);
        }
        if let Some(r) = &fx.reflection {
            reflection(&mut eff, "a", r);
        }
        eff.push_str("</a:effectLst>");
    }
    let (before, after) = kids.split_at(at);
    format!("{open}{ln}{fill_xml}{before}{eff}{after}</{tag}>")
}

/// ODF text properties for the effects it has: `fo:text-shadow`,
/// `style:text-outline` (hollow letters) and `style:font-relief` (the
/// raised letters of an embossed relief stand for the bevel). `colored`: `fo:color` is
/// written already; otherwise the letters' main color goes there.
pub(crate) fn odf_attrs(fx: &TextFx, colored: bool) -> String {
    let mut s = String::new();
    if let Some(sh) = &fx.shadow {
        let (dx, dy) = sh.offset_pt();
        let _ = write!(s, r##" fo:text-shadow="#{:06x} {dx:.2}pt {dy:.2}pt {:.2}pt""##, sh.color & 0xFF_FFFF, sh.blur_pt());
    }
    if fx.fill == TextFill::None && fx.outline.is_some() {
        s.push_str(r#" style:text-outline="true""#);
    }
    if fx.bevel {
        s.push_str(r#" style:font-relief="embossed""#);
    }
    if !colored {
        if let Some(c) = fx.main_color() {
            let _ = write!(s, r##" fo:color="#{:06x}""##, c & 0xFF_FFFF);
        }
    }
    s
}

/// `fx` with an ODF style's `fo:text-shadow`, `style:text-outline` and
/// `style:font-relief` values over it; `color` is the text color.
pub(crate) fn odf_read(fx: Option<Arc<TextFx>>, shadow: Option<&str>, hollow: Option<&str>, relief: Option<&str>, color: Option<u32>) -> Option<Arc<TextFx>> {
    if shadow.is_none() && hollow.is_none() && relief.is_none() {
        return fx;
    }
    let mut f = fx.as_deref().cloned().unwrap_or_default();
    if let Some(v) = relief {
        f.bevel = v == "embossed";
    }
    if let Some(v) = shadow {
        f.shadow = None;
        let v = v.trim();
        if v != "none" && !v.is_empty() {
            let mut c = 0x80_8080;
            let mut len = Vec::new();
            for w in v.split_whitespace() {
                match crate::parse_hex_color(w) {
                    Some(x) if w.starts_with('#') => c = x,
                    _ => len.extend(crate::parse_length_pt(w)),
                }
            }
            let (dx, dy) = (len.first().copied().unwrap_or(1.0), len.get(1).copied().unwrap_or(1.0));
            let angle = (dy.atan2(dx).to_degrees().round() as i32).rem_euclid(360) as u16;
            f.shadow = Some(TextShadow::new(c, 255, len.get(2).copied().unwrap_or(0.0), dx.hypot(dy), angle));
        }
    }
    if let Some(v) = hollow {
        if v == "true" {
            f.fill = TextFill::None;
            if f.outline.is_none() {
                f.outline = Some(TextOutline::new(color.unwrap_or(0), 0.75));
            }
        } else if f.fill == TextFill::None {
            f.fill = TextFill::Auto;
        }
    }
    (!f.is_empty()).then(|| Arc::new(f))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<TextFx> {
        let acc = [0x4472C4, 0xED7D31, 0xA5A5A5, 0xFFC000, 0x5B9BD5, 0x70AD47];
        (0..apex_doc::text_fx::STYLES).map(|i| apex_doc::text_fx::style(i, 0x1F1F1F, &acc)).collect()
    }

    /// What survives a round trip: DrawingML runs have no bevel.
    fn dml(fx: &TextFx) -> TextFx {
        TextFx { bevel: false, ..fx.clone() }
    }

    #[test]
    fn word_effects_round_trip() {
        for fx in all() {
            let mut x = String::from(r#"<w:rPr xmlns:w="w" xmlns:w14="w14"><w:b/>"#);
            write_w14(&mut x, &fx);
            x.push_str("</w:rPr>");
            let root = drawingml::read_fragment(&x);
            let back = read_all(root.find("rPr").unwrap()).expect("effects");
            assert_eq!(*back, fx, "{x}");
        }
    }

    #[test]
    fn drawingml_effects_round_trip() {
        for fx in all() {
            for rpr in [r#"<a:rPr lang="en-US" sz="2400"/>"#, r#"<a:rPr b="1"><a:solidFill><a:srgbClr val="112233"/></a:solidFill><a:latin typeface="Arial"/></a:rPr>"#] {
                let x = into_dml(rpr, &fx);
                assert!(x.starts_with("<a:rPr") && x.ends_with("</a:rPr>"), "{x}");
                let root = drawingml::read_fragment(&x);
                let n = root.find("rPr").unwrap();
                let back = read_all(n).map(|f| (*f).clone()).unwrap_or_default();
                // A plain opaque fill is the run's color there.
                let mut want = dml(&fx);
                if matches!(want.fill, TextFill::Solid(_, 255)) {
                    assert!(x.contains("<a:solidFill>"));
                    want.fill = TextFill::Auto;
                }
                assert_eq!(back, want, "{x}");
                assert!(!x.contains("112233") || want.fill == TextFill::Auto);
                if let Some(i) = x.find("<a:latin") {
                    assert!(x.find("<a:effectLst").is_none_or(|e| e < i));
                }
            }
        }
    }
}
