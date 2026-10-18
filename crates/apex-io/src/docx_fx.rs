//! Character effects (`CharFx`) in docx run properties.

use std::fmt::Write as _;

use apex_doc::{CharFx, UnderlineStyle};

use crate::xml::Tag;

fn on(t: &Tag) -> bool {
    t.attr("w:val").is_none_or(|v| !matches!(v.as_ref(), "0" | "false" | "off" | "none"))
}

/// Reads an effect element of `rPr` into `fx`; false when `t` isn't one.
/// `w:u` is read here for its style and color only.
pub(crate) fn read(t: &Tag, fx: &mut CharFx) -> bool {
    let num = |k: &str| t.attr(k).and_then(|v| v.parse::<f32>().ok());
    match t.local() {
        // Off over the styles' (on is the run's own bold or italic).
        "b" | "i" if on(t) => return false,
        "b" => fx.not_bold = true,
        "i" => fx.not_italic = true,
        "caps" => fx.all_caps = on(t),
        "smallCaps" => fx.small_caps = on(t),
        "dstrike" => fx.double_strike = on(t),
        "vanish" => fx.hidden = on(t),
        "spacing" if t.attr("w:val").is_some() => fx.spacing = num("w:val").unwrap_or(0.0).clamp(-3000.0, 3000.0) as i16,
        "w" => fx.scale = num("w:val").unwrap_or(100.0).clamp(1.0, 600.0) as u16,
        "position" => fx.position = num("w:val").unwrap_or(0.0).clamp(-3000.0, 3000.0) as i16,
        "u" => {
            fx.underline = underline_of(t.attr("w:val").as_deref().unwrap_or("single"));
            fx.underline_color = t.attr("w:color").filter(|c| c != "auto").and_then(|c| crate::parse_hex_color(&c));
        }
        _ => return false,
    }
    true
}

fn underline_of(v: &str) -> UnderlineStyle {
    match v {
        "words" => UnderlineStyle::Words,
        "double" | "wavyDouble" => UnderlineStyle::Double,
        "thick" => UnderlineStyle::Thick,
        "dotted" | "dottedHeavy" => UnderlineStyle::Dotted,
        "dash" | "dashedHeavy" | "dashLong" | "dashLongHeavy" => UnderlineStyle::Dashed,
        "dotDash" | "dotDotDash" | "dashDotHeavy" | "dashDotDotHeavy" => UnderlineStyle::DotDash,
        "wave" | "wavyHeavy" => UnderlineStyle::Wave,
        _ => UnderlineStyle::Single,
    }
}

/// The `w:u` element of an underline.
pub(crate) fn underline_xml(fx: &CharFx) -> String {
    let v = match fx.underline {
        UnderlineStyle::Single => "single",
        UnderlineStyle::Words => "words",
        UnderlineStyle::Double => "double",
        UnderlineStyle::Thick => "thick",
        UnderlineStyle::Dotted => "dotted",
        UnderlineStyle::Dashed => "dash",
        UnderlineStyle::DotDash => "dotDash",
        UnderlineStyle::Wave => "wave",
    };
    match fx.underline_color {
        Some(c) => format!(r#"<w:u w:val="{v}" w:color="{:06X}"/>"#, c & 0xFF_FFFF),
        None => format!(r#"<w:u w:val="{v}"/>"#),
    }
}

/// Effects written before the color (caps, strike, vanish) with the plain
/// strike, in schema order.
pub(crate) fn write_early(p: &mut String, fx: &CharFx, strike: bool, no_proof: bool) {
    if fx.all_caps {
        p.push_str("<w:caps/>");
    }
    if fx.small_caps {
        p.push_str("<w:smallCaps/>");
    }
    if strike {
        p.push_str("<w:strike/>");
    }
    if fx.double_strike {
        p.push_str("<w:dstrike/>");
    }
    if no_proof {
        p.push_str("<w:noProof/>");
    }
    if fx.hidden {
        p.push_str("<w:vanish/>");
    }
}

/// Effects written after the color and before the size.
pub(crate) fn write_late(p: &mut String, fx: &CharFx) {
    if fx.spacing != 0 {
        let _ = write!(p, r#"<w:spacing w:val="{}"/>"#, fx.spacing);
    }
    if fx.scale != 0 && fx.scale != 100 {
        let _ = write!(p, r#"<w:w w:val="{}"/>"#, fx.scale);
    }
    if fx.position != 0 {
        let _ = write!(p, r#"<w:position w:val="{}"/>"#, fx.position);
    }
}
