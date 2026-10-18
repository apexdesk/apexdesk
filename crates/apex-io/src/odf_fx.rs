//! Character effects (`CharFx`) in ODF text properties.

use std::fmt::Write as _;

use apex_doc::{CharFx, UnderlineStyle};

use crate::xml::Tag;
use crate::{parse_hex_color, parse_length_pt};

/// Reads the effects of a `style:text-properties` over `fx`.
pub(crate) fn read(t: &Tag, fx: &mut CharFx) {
    if let Some(v) = t.attr("fo:font-variant") {
        fx.small_caps = v == "small-caps";
    }
    if let Some(v) = t.attr("fo:text-transform") {
        fx.all_caps = v == "uppercase";
    }
    if let Some(v) = t.attr("style:text-line-through-type") {
        fx.double_strike = v == "double";
    }
    if let Some(v) = t.attr("text:display") {
        fx.hidden = v == "none";
    }
    if let Some(v) = t.attr("fo:letter-spacing") {
        fx.spacing = if v == "normal" { 0 } else { parse_length_pt(&v).map_or(0, |pt| (pt * 20.0).round().clamp(-3000.0, 3000.0) as i16) };
    }
    if let Some(v) = t.attr("style:text-scale") {
        fx.scale = v.trim_end_matches('%').parse::<f32>().map_or(100, |p| p.round().clamp(1.0, 600.0) as u16);
    }
    if let Some(v) = t.attr("style:text-position") {
        // "33% 100%": raised by a third of the font size, full size.
        let mut it = v.split_whitespace();
        let off = it.next().unwrap_or("0");
        let size = it.next().and_then(|s| s.trim_end_matches('%').parse::<f32>().ok()).unwrap_or(100.0);
        if size >= 99.0 {
            if let Ok(p) = off.trim_end_matches('%').parse::<f32>() {
                // Stored as a percentage of the font: taken for 12pt text.
                fx.position = (p / 100.0 * 24.0).round() as i16;
            }
        }
    }
    let style = t.attr("style:text-underline-style");
    let ty = t.attr("style:text-underline-type");
    if style.as_deref().is_some_and(|s| s != "none") {
        let width = t.attr("style:text-underline-width");
        fx.underline = match (style.as_deref().unwrap_or("solid"), ty.as_deref()) {
            (_, Some("double")) => UnderlineStyle::Double,
            ("dotted", _) => UnderlineStyle::Dotted,
            ("dash" | "long-dash", _) => UnderlineStyle::Dashed,
            ("dot-dash" | "dot-dot-dash", _) => UnderlineStyle::DotDash,
            ("wave", _) => UnderlineStyle::Wave,
            _ if width.as_deref().is_some_and(|w| w == "bold" || w == "thick") => UnderlineStyle::Thick,
            _ if t.attr("style:text-underline-mode").is_some_and(|m| m == "skip-white-space") => UnderlineStyle::Words,
            _ => UnderlineStyle::Single,
        };
        fx.underline_color = t.attr("style:text-underline-color").filter(|c| c != "font-color").and_then(|c| parse_hex_color(&c));
    }
}

/// The underline attributes of text properties.
pub(crate) fn underline_attrs(fx: &CharFx) -> String {
    let (style, ty, width) = match fx.underline {
        UnderlineStyle::Single | UnderlineStyle::Words => ("solid", "single", "auto"),
        UnderlineStyle::Double => ("solid", "double", "auto"),
        UnderlineStyle::Thick => ("solid", "single", "bold"),
        UnderlineStyle::Dotted => ("dotted", "single", "auto"),
        UnderlineStyle::Dashed => ("dash", "single", "auto"),
        UnderlineStyle::DotDash => ("dot-dash", "single", "auto"),
        UnderlineStyle::Wave => ("wave", "single", "auto"),
    };
    let color = fx.underline_color.map_or("font-color".to_string(), |c| format!("#{:06x}", c & 0xFF_FFFF));
    let mut s = format!(r#" style:text-underline-style="{style}" style:text-underline-type="{ty}" style:text-underline-width="{width}" style:text-underline-color="{color}""#);
    if fx.underline == UnderlineStyle::Words {
        s.push_str(r#" style:text-underline-mode="skip-white-space""#);
    }
    s
}

/// The other effect attributes (the strike type is written by the caller).
pub(crate) fn attrs(fx: &CharFx, superscript: bool, subscript: bool) -> String {
    let mut s = String::new();
    if fx.small_caps {
        s.push_str(r#" fo:font-variant="small-caps""#);
    }
    if fx.all_caps {
        s.push_str(r#" fo:text-transform="uppercase""#);
    }
    if fx.hidden {
        s.push_str(r#" text:display="none""#);
    }
    if fx.spacing != 0 {
        let _ = write!(s, r#" fo:letter-spacing="{}pt""#, fx.spacing as f32 / 20.0);
    }
    if fx.scale != 0 && fx.scale != 100 {
        let _ = write!(s, r#" style:text-scale="{}%""#, fx.scale);
    }
    if fx.position != 0 && !superscript && !subscript {
        let _ = write!(s, r#" style:text-position="{}% 100%""#, (fx.position as f32 / 24.0 * 100.0).round());
    }
    s
}
