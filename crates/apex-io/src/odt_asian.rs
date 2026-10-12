//! East Asian typography in ODF: the Asian and complex script fonts,
//! languages and sizes of text (`style:font-name-asian` and the like),
//! emphasis marks and combined characters, phonetic guides
//! (`text:ruby`), the Asian line breaking of paragraphs
//! (`style:line-break`, `style:punctuation-wrap`, `style:text-autospace`),
//! and the layout grid and writing mode of page layouts.

use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{AsianAttrs, AsianPara, Combine, DocGrid, Emphasis, FontAlign, GridKind, PageSetup, TextFlow};

use crate::parse_length_pt;
use crate::xml::{Tag, esc};

fn lang_tag(t: &Tag, lang: &str, country: &str) -> Option<Arc<str>> {
    let l = t.attr(lang).filter(|l| !l.is_empty() && l != "none" && l != "zxx")?;
    Some(match t.attr(country).filter(|c| !c.is_empty() && c != "none") {
        Some(c) => Arc::from(format!("{l}-{c}").as_str()),
        None => Arc::from(l.as_ref()),
    })
}

/// A language tag's language and country, as ODF's `fo:language` and
/// `fo:country` take them (letters and digits only): "sr-Latn-RS" is "sr"
/// and "RS"; tags ODF can't hold ("x-none") are `None`.
pub(crate) fn split_lang(l: &str) -> Option<(&str, Option<&str>)> {
    let mut parts = l.split(['-', '_']);
    let lang = parts.next().filter(|p| (1..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphabetic()))?;
    let country = parts.find(|p| (p.len() == 2 && p.bytes().all(|b| b.is_ascii_alphabetic())) || (p.len() == 3 && p.bytes().all(|b| b.is_ascii_digit())));
    Some((lang, country))
}

fn font(t: &Tag, name: &str, family: &str) -> Option<Arc<str>> {
    t.attr(name).or_else(|| t.attr(family)).map(|f| f.trim_matches('\'').to_string()).filter(|f| !f.is_empty()).map(|f| Arc::from(f.as_str()))
}

fn half_points(t: &Tag, k: &str) -> Option<u16> {
    t.attr(k).and_then(|v| parse_length_pt(&v)).map(|pt| (pt * 2.0).round() as u16)
}

/// Reads the Asian part of `style:text-properties`.
pub(crate) fn read_text(t: &Tag, a: &mut AsianAttrs) {
    if let Some(f) = font(t, "style:font-name-asian", "style:font-family-asian") {
        a.font = Some(f);
    }
    if let Some(f) = font(t, "style:font-name-complex", "style:font-family-complex") {
        a.cs_font = Some(f);
    }
    if let Some(l) = lang_tag(t, "style:language-asian", "style:country-asian") {
        a.lang = Some(l);
    }
    if let Some(l) = lang_tag(t, "style:language-complex", "style:country-complex") {
        a.cs_lang = Some(l);
    }
    if let Some(s) = half_points(t, "style:font-size-asian") {
        a.size = Some(s);
    }
    if let Some(s) = half_points(t, "style:font-size-complex") {
        a.cs_size = Some(s);
    }
    if let Some(v) = t.attr("style:text-emphasize") {
        let mut w = v.split_whitespace();
        let (mark, place) = (w.next().unwrap_or("none"), w.next().unwrap_or("above"));
        a.emphasis = match mark {
            "dot" if place == "below" => Some(Emphasis::UnderDot),
            "dot" => Some(Emphasis::Dot),
            "accent" => Some(Emphasis::Comma),
            "circle" => Some(Emphasis::Circle),
            "disc" => Some(Emphasis::Disc),
            _ => None,
        };
    }
    let combine = t.attr("style:text-combine");
    let rotated = t.attr("style:text-rotation-angle").and_then(|v| v.trim_end_matches("deg").parse::<f32>().ok()).is_some_and(|d| (d - 90.0).abs() < 0.5 || (d - 270.0).abs() < 0.5);
    if combine.as_deref().is_some_and(|c| c != "none") || rotated {
        let mut c = a.combine.clone().unwrap_or_default();
        c.lines = combine.as_deref() == Some("lines");
        c.letters = combine.as_deref() == Some("letters");
        let open = t.attr("style:text-combine-start-char").unwrap_or_default();
        let close = t.attr("style:text-combine-end-char").unwrap_or_default();
        if !open.is_empty() || !close.is_empty() {
            c.brackets = Some(Arc::from(match (open.as_ref(), close.as_ref()) {
                ("(", ")") => "round".to_string(),
                ("[", "]") => "square".to_string(),
                ("<", ">") => "angle".to_string(),
                ("{", "}") => "curly".to_string(),
                (o, e) => format!("{o}{e}"),
            }.as_str()));
        }
        c.vert = rotated;
        c.vert_compress = rotated && t.attr("style:text-rotation-scale").as_deref() == Some("line-height");
        a.combine = Some(c);
    }
}

/// Attributes of `style:text-properties` for the Asian settings `a` (with
/// `latin_lang`, the text's language, to pick a theme font).
pub(crate) fn text_attrs(a: &AsianAttrs, latin_lang: Option<&str>) -> String {
    let mut s = String::new();
    if let Some(f) = a.font.as_deref().and_then(|f| crate::theme_xml::ea_font_name(f, a.lang.as_deref().or(latin_lang))) {
        let _ = write!(s, r#" style:font-family-asian="'{}'""#, esc(&f));
    }
    if let Some(f) = &a.cs_font {
        let _ = write!(s, r#" style:font-family-complex="'{}'""#, esc(&crate::theme_xml::font_name(f)));
    }
    for (l, k) in [(&a.lang, "asian"), (&a.cs_lang, "complex")] {
        if let Some((lang, country)) = l.as_deref().and_then(split_lang) {
            let _ = write!(s, r#" style:language-{k}="{}""#, esc(lang));
            if let Some(country) = country {
                let _ = write!(s, r#" style:country-{k}="{}""#, esc(country));
            }
        }
    }
    if let Some(sz) = a.size {
        let _ = write!(s, r#" style:font-size-asian="{}pt""#, sz as f32 / 2.0);
    }
    if let Some(sz) = a.cs_size {
        let _ = write!(s, r#" style:font-size-complex="{}pt""#, sz as f32 / 2.0);
    }
    if let Some(e) = a.emphasis {
        let v = match e {
            Emphasis::Dot => "dot above",
            Emphasis::Comma => "accent above",
            Emphasis::Circle => "circle above",
            Emphasis::Disc => "disc above",
            Emphasis::UnderDot => "dot below",
        };
        let _ = write!(s, r#" style:text-emphasize="{v}""#);
    }
    if let Some(c) = &a.combine {
        write_combine(&mut s, c);
    }
    s
}

fn write_combine(s: &mut String, c: &Combine) {
    if c.lines || c.letters {
        let _ = write!(s, r#" style:text-combine="{}""#, if c.lines { "lines" } else { "letters" });
        if let Some(b) = c.brackets.as_deref() {
            let (o, e) = match b {
                "round" => ("(".to_string(), ")".to_string()),
                "square" => ("[".to_string(), "]".to_string()),
                "angle" => ("<".to_string(), ">".to_string()),
                "curly" => ("{".to_string(), "}".to_string()),
                _ => {
                    let mut ch = b.chars();
                    (ch.next().map(String::from).unwrap_or_default(), ch.collect())
                }
            };
            let _ = write!(s, r#" style:text-combine-start-char="{}" style:text-combine-end-char="{}""#, esc(&o), esc(&e));
        }
    }
    if c.vert {
        s.push_str(r#" style:text-rotation-angle="90""#);
        if c.vert_compress {
            s.push_str(r#" style:text-rotation-scale="line-height""#);
        }
    }
}

// --- paragraphs ------------------------------------------------------------------------

/// Reads the Asian part of `style:paragraph-properties`.
pub(crate) fn read_para(t: &Tag, p: &mut AsianPara) {
    let flag = |k: &str, yes: &str, no: &str| match t.attr(k).as_deref() {
        Some(v) if v == yes => Some(true),
        Some(v) if v == no => Some(false),
        _ => None,
    };
    if let Some(v) = flag("style:line-break", "strict", "normal") {
        p.kinsoku = Some(v);
    }
    if let Some(v) = flag("style:punctuation-wrap", "hanging", "simple") {
        p.overflow_punct = Some(v);
    }
    if let Some(v) = flag("style:text-autospace", "ideograph-alpha", "none") {
        p.auto_space_de = Some(v);
        p.auto_space_dn = Some(v);
    }
    if let Some(v) = flag("style:snap-to-layout-grid", "true", "false") {
        p.snap_to_grid = Some(v);
    }
    p.font_align = match t.attr("style:vertical-align").as_deref() {
        Some("auto") => Some(FontAlign::Auto),
        Some("baseline") => Some(FontAlign::Baseline),
        Some("top") => Some(FontAlign::Top),
        Some("middle") => Some(FontAlign::Center),
        Some("bottom") => Some(FontAlign::Bottom),
        _ => p.font_align,
    };
}

/// Attributes of `style:paragraph-properties` for the Asian settings `p`.
pub(crate) fn para_attrs(p: &AsianPara) -> String {
    let mut s = String::new();
    if let Some(v) = p.kinsoku {
        let _ = write!(s, r#" style:line-break="{}""#, if v { "strict" } else { "normal" });
    }
    if let Some(v) = p.overflow_punct {
        let _ = write!(s, r#" style:punctuation-wrap="{}""#, if v { "hanging" } else { "simple" });
    }
    // ODF has one setting for letters and digits.
    if let Some(v) = p.auto_space_de.or(p.auto_space_dn) {
        let _ = write!(s, r#" style:text-autospace="{}""#, if v { "ideograph-alpha" } else { "none" });
    }
    if let Some(v) = p.snap_to_grid {
        let _ = write!(s, r#" style:snap-to-layout-grid="{v}""#);
    }
    if let Some(f) = p.font_align {
        let v = match f {
            FontAlign::Auto => "auto",
            FontAlign::Baseline => "baseline",
            FontAlign::Top => "top",
            FontAlign::Center => "middle",
            FontAlign::Bottom => "bottom",
        };
        let _ = write!(s, r#" style:vertical-align="{v}""#);
    }
    s
}

// --- page layouts --------------------------------------------------------------------

/// Reads the layout grid and writing mode of `style:page-layout-properties`.
/// The character pitch is kept in points in `char_space` until the size
/// of the Normal font is known (see [`fix_char_pitch`]).
pub(crate) fn read_page(t: &Tag, p: &mut PageSetup) {
    if let Some(f) = t.attr("style:writing-mode").and_then(|v| TextFlow::from_odf(&v)).filter(|f| *f != TextFlow::LrTb) {
        p.text_flow = Some(f);
    }
    let Some(mode) = t.attr("style:layout-grid-mode") else { return };
    let len = |k: &str| t.attr(k).and_then(|v| parse_length_pt(&v));
    let yes = |k: &str| t.attr(k).as_deref() == Some("true");
    let g = &mut p.grid;
    g.kind = match mode.as_ref() {
        "line" => GridKind::Lines,
        "both" if yes("style:layout-grid-snap-to") => GridKind::SnapToChars,
        "both" => GridKind::LinesAndChars,
        _ => GridKind::None,
    };
    g.ruby_height = len("style:layout-grid-ruby-height").unwrap_or(0.0);
    if let Some(h) = len("style:layout-grid-base-height") {
        g.line_pitch = ((h + g.ruby_height) * 20.0).round() / 20.0;
    }
    if let Some(w) = len("style:layout-grid-base-width") {
        g.char_space = (w * 4096.0).round() as i32;
    }
    g.color = t.attr("style:layout-grid-color").and_then(|c| crate::parse_hex_color(&c));
    g.display = yes("style:layout-grid-display");
    g.print = yes("style:layout-grid-print");
}

/// Turns the character pitch read by [`read_page`] into Word's terms once
/// the Normal font's size (`base`, points) is known.
pub(crate) fn fix_char_pitch(p: &mut PageSetup, base: f32) {
    if p.grid.char_space != 0 {
        p.grid.char_space = DocGrid::space_for(p.grid.char_space as f32 / 4096.0, base);
    }
}

/// Attributes of `style:page-layout-properties` for the grid and writing
/// mode of `p`, with `base` the Normal font's size (points).
pub(crate) fn page_attrs(p: &PageSetup, base: f32) -> String {
    let mut s = String::new();
    if let Some(f) = p.text_flow {
        let _ = write!(s, r#" style:writing-mode="{}""#, f.odf());
    }
    let g = &p.grid;
    if g.kind == GridKind::None && g.line_pitch <= 0.0 {
        return s;
    }
    let mode = match g.kind {
        GridKind::None => "none",
        GridKind::Lines => "line",
        GridKind::LinesAndChars | GridKind::SnapToChars => "both",
    };
    let w = p.width - p.margin_left - p.margin_right - p.gutter;
    let h = p.height - p.margin_top - p.margin_bottom;
    // Lines of a vertical page follow each other across its width.
    let (w, h) = if p.vertical() { (h, w) } else { (w, h) };
    let (lines, _) = g.counts(w, h, base);
    let pt = |v: f32| format!("{}pt", (v * 100.0).round() / 100.0);
    let _ = write!(
        s,
        r#" style:layout-grid-mode="{mode}" style:layout-grid-lines="{lines}" style:layout-grid-base-height="{}" style:layout-grid-ruby-height="{}" style:layout-grid-base-width="{}" style:layout-grid-snap-to="{}" style:layout-grid-display="{}" style:layout-grid-print="{}" style:layout-grid-ruby-below="false""#,
        pt((g.pitch() - g.ruby_height).max(1.0)),
        pt(g.ruby_height),
        pt(g.char_pitch(base)),
        g.kind == GridKind::SnapToChars,
        g.display,
        g.print
    );
    if let Some(c) = g.color {
        let _ = write!(s, r##" style:layout-grid-color="#{:06x}""##, c & 0xFF_FFFF);
    }
    s
}

// --- phonetic guides -----------------------------------------------------------------

/// Settings of a `style:ruby-properties`: (under the text, alignment).
pub(crate) fn read_ruby_style(t: &Tag) -> (bool, Option<Arc<str>>) {
    let below = t.attr("style:ruby-position").as_deref() == Some("below");
    let align = t.attr("style:ruby-align").map(|a| {
        Arc::from(match a.as_ref() {
            "left" => "left",
            "right" => "right",
            "distribute-letter" => "distributeLetter",
            "distribute-space" => "distributeSpace",
            _ => "center",
        })
    });
    (below, align)
}

/// Opens the `text:ruby-text` of `r`, with a text style for its size
/// (defined in `auto` the first time).
pub(crate) fn open_ruby_text(body: &mut String, auto: &mut String, prefix: &str, r: &apex_doc::Ruby) {
    let Some(hp) = r.size else {
        body.push_str("<text:ruby-text>");
        return;
    };
    let name = format!("{prefix}RuT{hp}");
    if !auto.contains(&format!(r#"style:name="{name}""#)) {
        let pt = hp as f32 / 2.0;
        let _ = write!(auto, r#"<style:style style:name="{name}" style:family="text"><style:text-properties fo:font-size="{pt}pt" style:font-size-asian="{pt}pt"/></style:style>"#);
    }
    let _ = write!(body, r#"<text:ruby-text text:style-name="{name}">"#);
}

/// The automatic style of a phonetic guide: its name and definition.
pub(crate) fn ruby_style(prefix: &str, r: &apex_doc::Ruby) -> (String, String) {
    let (i, align) = match r.align.as_deref() {
        Some("left") => (1, "left"),
        Some("right") => (2, "right"),
        Some("distributeLetter") => (3, "distribute-letter"),
        Some("distributeSpace") => (4, "distribute-space"),
        _ => (0, "center"),
    };
    let name = format!("{prefix}Ru{i}{}", if r.below { "b" } else { "" });
    let pos = if r.below { "below" } else { "above" };
    let def = format!(r#"<style:style style:name="{name}" style:family="ruby"><style:ruby-properties style:ruby-position="{pos}" style:ruby-align="{align}"/></style:style>"#);
    (name, def)
}
