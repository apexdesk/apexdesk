//! DrawingML colors, fills, outlines, shadows and style references of
//! presentations (`apex_slides` keeps them unresolved: theme slots and
//! color changes), read from and written back to their elements.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_slides::color::{Base, Mod, SColor, Scheme};
use apex_slides::fill::{Arrow, Background, Dash, Fill, Gradient, Line, PictureFill, Shadow, StyleRefs};

use crate::drawingml::{EMU_PT, Node};
use crate::xml::esc;

/// Images of a part by relationship id.
pub(crate) type Media = HashMap<String, Arc<[u8]>>;

/// Names of the preset colors used most (`a:prstClr`); others are black.
fn preset_color(name: &str) -> u32 {
    match name {
        "black" => 0x000000,
        "white" => 0xFFFFFF,
        "red" => 0xFF0000,
        "green" => 0x008000,
        "lime" => 0x00FF00,
        "blue" => 0x0000FF,
        "yellow" => 0xFFFF00,
        "cyan" | "aqua" => 0x00FFFF,
        "magenta" | "fuchsia" => 0xFF00FF,
        "gray" | "grey" => 0x808080,
        "silver" => 0xC0C0C0,
        "maroon" => 0x800000,
        "navy" => 0x000080,
        "olive" => 0x808000,
        "purple" => 0x800080,
        "teal" => 0x008080,
        "orange" => 0xFFA500,
        "ltGray" => 0xD3D3D3,
        "dkGray" => 0xA9A9A9,
        _ => 0,
    }
}

/// The color of a color element (`a:srgbClr`, `a:schemeClr`...).
pub(crate) fn read_color_el(n: &Node) -> Option<SColor> {
    let base = match n.local() {
        "srgbClr" => Base::Rgb(u32::from_str_radix(n.attr("val")?.trim(), 16).ok()? & 0xFF_FFFF),
        "schemeClr" => match n.attr("val")? {
            "phClr" => Base::Placeholder,
            v => Base::Scheme(Scheme::parse(v)?),
        },
        "sysClr" => Base::Rgb(n.attr("lastClr").and_then(|v| u32::from_str_radix(v, 16).ok()).unwrap_or(if n.attr("val") == Some("window") { 0xFFFFFF } else { 0 })),
        "prstClr" => Base::Rgb(preset_color(n.attr("val")?)),
        "scrgbClr" => {
            let c = |k: &str| {
                let v = (n.num(k).unwrap_or(0.0) / 100_000.0).clamp(0.0, 1.0);
                let s = if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
                (s * 255.0).round() as u32
            };
            Base::Rgb((c("r") << 16) | (c("g") << 8) | c("b"))
        }
        "hslClr" => {
            let h = n.num("hue").unwrap_or(0.0) / 60_000.0 / 360.0;
            let s = n.num("sat").unwrap_or(0.0) / 100_000.0;
            let l = n.num("lum").unwrap_or(0.0) / 100_000.0;
            Base::Rgb(apex_slides::color::from_hsl(h, s, l))
        }
        _ => return None,
    };
    let mods = n.kids.iter().filter(|k| k.name != crate::drawingml::TEXT).map(|k| Mod::parse(k.local(), k.num("val").unwrap_or(0.0) as i32)).collect();
    Some(SColor { base, mods })
}

/// The color inside `n` (its first color child).
pub(crate) fn read_color(n: &Node) -> Option<SColor> {
    n.kids.iter().find_map(read_color_el)
}

pub(crate) fn write_color(out: &mut String, c: &SColor) {
    let (tag, val) = match c.base {
        Base::Rgb(v) => ("srgbClr", format!("{v:06X}")),
        Base::Scheme(s) => ("schemeClr", s.name()),
        Base::Placeholder => ("schemeClr", "phClr".to_string()),
    };
    if c.mods.is_empty() {
        let _ = write!(out, r#"<a:{tag} val="{val}"/>"#);
        return;
    }
    let _ = write!(out, r#"<a:{tag} val="{val}">"#);
    for m in &c.mods {
        match m.name() {
            n @ ("inv" | "gray" | "comp" | "gamma" | "invGamma") => {
                let _ = write!(out, "<a:{n}/>");
            }
            n => {
                let _ = write!(out, r#"<a:{n} val="{}"/>"#, m.value());
            }
        }
    }
    let _ = write!(out, "</a:{tag}>");
}

/// A fill element (`a:solidFill`, `a:gradFill`...), if `n` is one.
pub(crate) fn read_fill_el(n: &Node, media: &Media) -> Option<Fill> {
    Some(match n.local() {
        "noFill" => Fill::None,
        "solidFill" => Fill::Solid(read_color(n).unwrap_or_else(|| SColor::rgb(0))),
        "grpFill" => Fill::Group,
        "gradFill" => {
            let mut stops = Vec::new();
            if let Some(l) = n.child("gsLst") {
                for gs in l.children("gs") {
                    if let Some(c) = read_color(gs) {
                        stops.push(((gs.num("pos").unwrap_or(0.0) / 100_000.0) as f32, c));
                    }
                }
            }
            stops.sort_by(|a, b| a.0.total_cmp(&b.0));
            let angle = n.child("lin").and_then(|l| l.num("ang")).map_or(90.0, |a| (a / 60_000.0) as f32);
            let path = n.child("path").and_then(|p| p.attr("path")).map(Arc::from);
            Fill::Gradient(Gradient { stops, angle, path })
        }
        "blipFill" => {
            let blip = n.child("blip")?;
            let data = blip.attr("r:embed").or_else(|| blip.attr("embed")).and_then(|id| media.get(id)).cloned()?;
            let crop = n.child("srcRect").map_or([0.0; 4], |r| ["l", "t", "r", "b"].map(|k| (r.num(k).unwrap_or(0.0) / 100_000.0) as f32));
            let alpha = blip.child("alphaModFix").and_then(|a| a.num("amt")).map_or(1.0, |a| (a / 100_000.0) as f32);
            Fill::Picture(PictureFill { data, tile: n.child("tile").is_some(), crop, alpha })
        }
        "pattFill" => {
            let fg = n.child("fgClr").and_then(read_color).unwrap_or_else(|| SColor::rgb(0));
            let bg = n.child("bgClr").and_then(read_color).unwrap_or_else(|| SColor::rgb(0xFFFFFF));
            Fill::Pattern { preset: Arc::from(n.attr("prst").unwrap_or("pct50")), fg, bg }
        }
        _ => return None,
    })
}

/// The fill among the children of `n` (a `spPr`, `bgPr`, `tcPr`...).
pub(crate) fn read_fill(n: &Node, media: &Media) -> Option<Fill> {
    n.kids.iter().find_map(|k| read_fill_el(k, media))
}

/// Pictures a writer adds to a part: bytes and their relationship ids.
pub(crate) trait Images {
    fn image(&mut self, data: &Arc<[u8]>) -> String;
}

pub(crate) fn write_fill(out: &mut String, f: &Fill, imgs: &mut dyn Images) {
    match f {
        Fill::None => out.push_str("<a:noFill/>"),
        Fill::Group => out.push_str("<a:grpFill/>"),
        Fill::Solid(c) => {
            out.push_str("<a:solidFill>");
            write_color(out, c);
            out.push_str("</a:solidFill>");
        }
        Fill::Gradient(g) => {
            out.push_str(r#"<a:gradFill rotWithShape="1"><a:gsLst>"#);
            for (p, c) in &g.stops {
                let _ = write!(out, r#"<a:gs pos="{}">"#, (p * 100_000.0).round() as i64);
                write_color(out, c);
                out.push_str("</a:gs>");
            }
            out.push_str("</a:gsLst>");
            match &g.path {
                Some(p) => {
                    let _ = write!(out, r#"<a:path path="{}"><a:fillToRect l="50000" t="50000" r="50000" b="50000"/></a:path>"#, esc(p));
                }
                None => {
                    let _ = write!(out, r#"<a:lin ang="{}" scaled="0"/>"#, (g.angle * 60_000.0).round() as i64);
                }
            }
            out.push_str("</a:gradFill>");
        }
        Fill::Picture(p) => {
            let id = imgs.image(&p.data);
            out.push_str(r#"<a:blipFill dpi="0" rotWithShape="1">"#);
            let _ = write!(out, r#"<a:blip r:embed="{id}">"#);
            if p.alpha < 1.0 {
                let _ = write!(out, r#"<a:alphaModFix amt="{}"/>"#, (p.alpha * 100_000.0).round() as i64);
            }
            out.push_str("</a:blip>");
            if p.crop != [0.0; 4] {
                let [l, t, r, b] = p.crop.map(|v| (v * 100_000.0).round() as i64);
                let _ = write!(out, r#"<a:srcRect l="{l}" t="{t}" r="{r}" b="{b}"/>"#);
            } else {
                out.push_str("<a:srcRect/>");
            }
            if p.tile {
                out.push_str(r#"<a:tile tx="0" ty="0" sx="100000" sy="100000" flip="none" algn="tl"/>"#);
            } else {
                out.push_str("<a:stretch><a:fillRect/></a:stretch>");
            }
            out.push_str("</a:blipFill>");
        }
        Fill::Pattern { preset, fg, bg } => {
            let _ = write!(out, r#"<a:pattFill prst="{}"><a:fgClr>"#, esc(preset));
            write_color(out, fg);
            out.push_str("</a:fgClr><a:bgClr>");
            write_color(out, bg);
            out.push_str("</a:bgClr></a:pattFill>");
        }
    }
}

/// An outline (`a:ln`).
pub(crate) fn read_line(n: &Node, media: &Media) -> Line {
    Line {
        width: n.num("w").map(|w| w as f32 / EMU_PT),
        fill: read_fill(n, media),
        dash: n.child("prstDash").and_then(|d| d.attr("val")).map(Dash::parse),
        head: n.child("headEnd").and_then(|d| d.attr("type")).map(Arrow::parse),
        tail: n.child("tailEnd").and_then(|d| d.attr("type")).map(Arrow::parse),
    }
}

pub(crate) fn write_line(out: &mut String, l: &Line, imgs: &mut dyn Images) {
    out.push_str("<a:ln");
    if let Some(w) = l.width {
        let _ = write!(out, r#" w="{}""#, (w * EMU_PT).round() as i64);
    }
    out.push('>');
    if let Some(f) = &l.fill {
        write_fill(out, f, imgs);
    }
    if let Some(d) = l.dash {
        let _ = write!(out, r#"<a:prstDash val="{}"/>"#, d.name());
    }
    if let Some(h) = l.head {
        let _ = write!(out, r#"<a:headEnd type="{}"/>"#, h.name());
    }
    if let Some(t) = l.tail {
        let _ = write!(out, r#"<a:tailEnd type="{}"/>"#, t.name());
    }
    out.push_str("</a:ln>");
}

/// The outer shadow of an effect list.
pub(crate) fn read_shadow(effects: &Node) -> Option<Shadow> {
    let s = effects.child("outerShdw")?;
    Some(Shadow {
        color: read_color(s).unwrap_or_else(|| SColor::rgb(0)),
        blur: s.num("blurRad").map_or(0.0, |v| v as f32 / EMU_PT),
        dist: s.num("dist").map_or(0.0, |v| v as f32 / EMU_PT),
        dir: s.num("dir").map_or(0.0, |v| (v / 60_000.0) as f32),
    })
}

pub(crate) fn write_shadow(out: &mut String, s: &Shadow) {
    let _ = write!(out, r#"<a:effectLst><a:outerShdw blurRad="{}" dist="{}" dir="{}" algn="tl" rotWithShape="0">"#, (s.blur * EMU_PT) as i64, (s.dist * EMU_PT) as i64, (s.dir * 60_000.0) as i64);
    write_color(out, &s.color);
    out.push_str("</a:outerShdw></a:effectLst>");
}

/// A shape's `p:style`.
pub(crate) fn read_style(n: &Node) -> StyleRefs {
    let r = |k: &str| n.child(k).map(|x| (x.num("idx").unwrap_or(0.0) as u32, read_color(x).unwrap_or_else(|| SColor::scheme(Scheme::Tx1))));
    StyleRefs { line: r("lnRef"), fill: r("fillRef"), effect: r("effectRef"), font: n.child("fontRef").map(|f| (Arc::from(f.attr("idx").unwrap_or("minor")), read_color(f))) }
}

pub(crate) fn write_style(out: &mut String, tag: &str, s: &StyleRefs) {
    let _ = write!(out, "<{tag}>");
    for (k, v) in [("a:lnRef", &s.line), ("a:fillRef", &s.fill), ("a:effectRef", &s.effect)] {
        let (idx, c) = v.clone().unwrap_or((0, SColor::scheme(Scheme::Accent(0))));
        let _ = write!(out, r#"<{k} idx="{idx}">"#);
        write_color(out, &c);
        let _ = write!(out, "</{k}>");
    }
    let (idx, c) = s.font.clone().unwrap_or((Arc::from("minor"), None));
    let _ = write!(out, r#"<a:fontRef idx="{}">"#, esc(&idx));
    if let Some(c) = c {
        write_color(out, &c);
    }
    out.push_str("</a:fontRef>");
    let _ = write!(out, "</{tag}>");
}

/// A slide, layout or master background (`p:bg`).
pub(crate) fn read_background(bg: &Node, media: &Media) -> Option<Background> {
    if let Some(r) = bg.child("bgRef") {
        return Some(Background::Ref(r.num("idx").unwrap_or(1001.0) as u32, read_color(r).unwrap_or_else(|| SColor::scheme(Scheme::Bg1))));
    }
    read_fill(bg.child("bgPr")?, media).map(Background::Fill)
}

pub(crate) fn write_background(out: &mut String, b: &Background, imgs: &mut dyn Images) {
    out.push_str("<p:bg>");
    match b {
        Background::Ref(idx, c) => {
            let _ = write!(out, r#"<p:bgRef idx="{idx}">"#);
            write_color(out, c);
            out.push_str("</p:bgRef>");
        }
        Background::Fill(f) => {
            out.push_str("<p:bgPr>");
            write_fill(out, f, imgs);
            out.push_str("<a:effectLst/></p:bgPr>");
        }
    }
    out.push_str("</p:bg>");
}

/// The theme's fills, outlines and effects (`a:fmtScheme`).
pub(crate) fn read_fmt_scheme(n: &Node, media: &Media) -> apex_slides::FormatScheme {
    let mut f = apex_slides::FormatScheme::default();
    if let Some(l) = n.child("fillStyleLst") {
        f.fills = l.kids.iter().filter_map(|k| read_fill_el(k, media)).collect();
    }
    if let Some(l) = n.child("lnStyleLst") {
        f.lines = l.children("ln").map(|k| read_line(k, media)).collect();
    }
    if let Some(l) = n.child("bgFillStyleLst") {
        f.bg_fills = l.kids.iter().filter_map(|k| read_fill_el(k, media)).collect();
    }
    if let Some(l) = n.child("effectStyleLst") {
        f.shadows = l.children("effectStyle").map(|e| e.child("effectLst").and_then(read_shadow)).collect();
    }
    let office = apex_slides::FormatScheme::office();
    if f.fills.is_empty() {
        f.fills = office.fills;
    }
    if f.lines.is_empty() {
        f.lines = office.lines;
    }
    if f.bg_fills.is_empty() {
        f.bg_fills = office.bg_fills;
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawingml::read_fragment;

    struct NoImages;
    impl Images for NoImages {
        fn image(&mut self, _: &Arc<[u8]>) -> String {
            "rId1".into()
        }
    }

    #[test]
    fn fills_round_trip() {
        let xml = r#"<a:gradFill xmlns:a="a"><a:gsLst><a:gs pos="0"><a:schemeClr val="accent1"><a:lumMod val="60000"/><a:lumOff val="40000"/></a:schemeClr></a:gs><a:gs pos="100000"><a:srgbClr val="FF0000"/></a:gs></a:gsLst><a:lin ang="5400000" scaled="0"/></a:gradFill>"#;
        let root = read_fragment(xml);
        let f = read_fill_el(&root.kids[0], &Media::new()).unwrap();
        let mut out = String::new();
        write_fill(&mut out, &f, &mut NoImages);
        let back = read_fill_el(&read_fragment(&out).kids[0], &Media::new()).unwrap();
        assert_eq!(f, back);
        let Fill::Gradient(g) = f else { panic!() };
        assert_eq!(g.angle, 90.0);
        assert_eq!(g.stops[0].1.mods, vec![Mod::LumMod(60000), Mod::LumOff(40000)]);
    }
}
