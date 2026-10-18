//! Page layout in docx: sections (`w:sectPr` in paragraphs and at the end
//! of the body), text columns, page borders, the page background, text
//! and picture watermarks (shapes in the headers, as Word writes them);
//! paragraph borders, tab stops and drop caps (`w:framePr`).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{Columns, Document, DropCap, Line, LineStyle, PageBorders, PageSetup, ParaBorders, ParaStyle, Paragraph, Section, SectionStart, TabAlign, TabLeader, TabStop, TabStops, Watermark};

use crate::parse_hex_color;
use crate::xml::{Ev, Reader, Tag, esc, local};
use crate::zip::ZipReader;

const TWIP: f32 = 20.0;

/// A section's properties as read.
#[derive(Clone, Default)]
pub(crate) struct SectInfo {
    pub page: PageSetup,
    pub columns: Columns,
    pub start: SectionStart,
    pub borders: Option<PageBorders>,
    /// Header and footer references: (header?, 0 default / 1 first /
    /// 2 even, relationship id).
    pub hf_refs: Vec<(bool, u8, String)>,
    pub title_pg: bool,
    /// The headers and footers, once their parts are read.
    pub headers: apex_doc::Headers,
}

/// Paragraph properties kept aside while a paragraph is read.
#[derive(Default)]
pub(crate) struct PprExtra {
    /// The section this paragraph ends.
    pub sect: Option<SectInfo>,
    /// A drop cap frame (the paragraph holds only the letter).
    pub cap: Option<DropCap>,
    /// A frame text wraps around, placed across the margins (a header or
    /// footer's page number).
    pub float: Option<apex_doc::Align>,
    /// HTML auto spacing before and after.
    pub auto_space: [bool; 2],
}

fn is_on(t: &Tag, key: &str) -> bool {
    t.attr(key).is_some_and(|v| matches!(v.as_ref(), "1" | "true" | "on"))
}

// --- reading ------------------------------------------------------------------------

/// Reads a `w:sectPr` (the reader is past its start tag).
pub(crate) fn read_sect(r: &mut Reader, s: &mut SectInfo) {
    let page = &mut s.page;
    let mut depth = 1;
    let mut in_borders = false;
    let mut custom: Vec<(f32, f32)> = Vec::new();
    let mut equal = true;
    loop {
        let ev = r.next();
        match ev {
            Ev::Eof => return,
            Ev::End(n) => {
                if local(n) == "pgBorders" {
                    in_borders = false;
                }
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Ev::Start(t) | Ev::Empty(t) => {
                let tw = |k: &str| t.attr(k).and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite()).map(|v| v / TWIP);
                match t.local() {
                    "sectPrChange" if matches!(ev, Ev::Start(_)) => {
                        r.skip();
                        continue;
                    }
                    name @ ("headerReference" | "footerReference") => {
                        let ty = match t.attr("w:type").as_deref() {
                            Some("first") => 1,
                            Some("even") => 2,
                            _ => 0,
                        };
                        if let Some(id) = t.attr("r:id") {
                            s.hf_refs.push((name == "headerReference", ty, id.into_owned()));
                        }
                    }
                    "pgNumType" => {
                        let n = &mut s.headers.numbering;
                        if let Some(f) = t.attr("w:fmt").and_then(|v| crate::docx::notes::format_of(&v)) {
                            n.format = f;
                        }
                        n.start = t.attr("w:start").and_then(|v| v.parse::<u32>().ok());
                    }
                    "lnNumType" => {
                        let n = |k: &str| t.attr(k).and_then(|v| v.parse::<i32>().ok());
                        page.line_numbers = Some(apex_doc::LineNumbering {
                            count_by: n("w:countBy").unwrap_or(1).clamp(1, 100) as u16,
                            // Stored 0-based.
                            start: (n("w:start").unwrap_or(0) + 1).clamp(1, 32767) as u16,
                            restart: match t.attr("w:restart").as_deref() {
                                Some("newSection") => apex_doc::LineRestart::EachSection,
                                Some("continuous") => apex_doc::LineRestart::Continuous,
                                _ => apex_doc::LineRestart::EachPage,
                            },
                            distance: tw("w:distance").filter(|&d| d > 0.0),
                        });
                    }
                    "docGrid" | "textDirection" => crate::docx_asian::read_sect(&t, page),
                    "titlePg" => s.title_pg =t.attr("w:val").is_none_or(|v| !matches!(v.as_ref(), "0" | "false" | "off")),
                    "pgSz" => {
                        if let (Some(w), Some(h)) = (tw("w:w"), tw("w:h")) {
                            if w > 1.0 && h > 1.0 {
                                page.width = w;
                                page.height = h;
                            }
                        }
                    }
                    "pgMar" => {
                        page.margin_top = tw("w:top").map_or(page.margin_top, f32::abs);
                        page.margin_bottom = tw("w:bottom").map_or(page.margin_bottom, f32::abs);
                        page.margin_left = tw("w:left").or_else(|| tw("w:start")).unwrap_or(page.margin_left);
                        page.margin_right = tw("w:right").or_else(|| tw("w:end")).unwrap_or(page.margin_right);
                        page.margin_header = tw("w:header").map_or(page.margin_header, f32::abs);
                        page.margin_footer = tw("w:footer").map_or(page.margin_footer, f32::abs);
                        page.gutter = tw("w:gutter").map_or(page.gutter, f32::abs);
                    }
                    "vAlign" => {
                        page.v_align = match t.attr("w:val").as_deref() {
                            Some("center") => apex_doc::PageVAlign::Center,
                            Some("both") => apex_doc::PageVAlign::Justify,
                            Some("bottom") => apex_doc::PageVAlign::Bottom,
                            _ => apex_doc::PageVAlign::Top,
                        };
                    }
                    "type" => {
                        s.start = match t.attr("w:val").as_deref() {
                            Some("continuous") => SectionStart::Continuous,
                            Some("evenPage") => SectionStart::EvenPage,
                            Some("oddPage") => SectionStart::OddPage,
                            _ => SectionStart::NextPage,
                        }
                    }
                    "cols" => {
                        let n = t.attr("w:num").and_then(|v| v.parse::<u8>().ok()).unwrap_or(1).max(1);
                        s.columns = Columns { count: n, spacing: tw("w:space").unwrap_or(36.0), separator: is_on(&t, "w:sep"), custom: Vec::new() };
                        equal = t.attr("w:equalWidth").is_none_or(|v| !matches!(v.as_ref(), "0" | "false" | "off"));
                    }
                    "col" => custom.push((tw("w:w").unwrap_or(0.0), tw("w:space").unwrap_or(0.0))),
                    "pgBorders" => {
                        in_borders = matches!(ev, Ev::Start(_));
                        let b = s.borders.get_or_insert_with(PageBorders::default);
                        b.from_edge = t.attr("w:offsetFrom").as_deref() == Some("page");
                    }
                    side @ ("top" | "left" | "bottom" | "right") if in_borders => {
                        let l = read_line(&t);
                        if let Some(b) = &mut s.borders {
                            *match side {
                                "top" => &mut b.top,
                                "left" => &mut b.left,
                                "bottom" => &mut b.bottom,
                                _ => &mut b.right,
                            } = l;
                        }
                    }
                    _ => {}
                }
                if matches!(ev, Ev::Start(_)) {
                    depth += 1;
                }
            }
            _ => {}
        }
    }
    if !equal && custom.len() > 1 {
        s.columns.count = custom.len().min(apex_doc::MAX_COLUMNS) as u8;
        s.columns.custom = custom;
    }
    if s.borders.is_some_and(|b| b.is_empty()) {
        s.borders = None;
    }
}

/// A border line (`w:top` and the like); `None` for no line.
fn read_line(t: &Tag) -> Option<Line> {
    let style = match t.attr("w:val").as_deref() {
        None | Some("nil" | "none") => return None,
        Some("double" | "triple" | "thinThickSmallGap" | "thickThinSmallGap" | "thinThickMediumGap" | "thickThinMediumGap" | "thinThickLargeGap" | "thickThinLargeGap" | "thinThickThinSmallGap" | "thinThickThinMediumGap" | "thinThickThinLargeGap") => LineStyle::Double,
        Some("dotted") => LineStyle::Dotted,
        Some("dashed" | "dashSmallGap") => LineStyle::Dashed,
        Some("dotDash" | "dotDotDash") => LineStyle::DashDot,
        Some("thick") => LineStyle::Thick,
        Some("wave" | "doubleWave") => LineStyle::Wave,
        _ => LineStyle::Single,
    };
    let sz = t.attr("w:sz").and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite()).unwrap_or(4.0);
    let color = t.attr("w:color").filter(|v| v != "auto").and_then(|v| parse_hex_color(&v)).unwrap_or(0);
    let space = t.attr("w:space").and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite()).unwrap_or(0.0);
    Some(Line { style, width: (sz / 8.0).clamp(0.25, 12.0), color, space: space.clamp(0.0, 31.0) })
}

/// Reads `w:pBdr` (the reader is past its start tag).
pub(crate) fn read_pbdr(r: &mut Reader, b: &mut ParaBorders) {
    let mut depth = 1;
    loop {
        let ev = r.next();
        match ev {
            Ev::Eof => return,
            Ev::End(_) => {
                depth -= 1;
                if depth == 0 {
                    return;
                }
            }
            Ev::Start(t) | Ev::Empty(t) => {
                let l = read_line(&t);
                match t.local() {
                    "top" => b.top = l,
                    "bottom" => b.bottom = l,
                    "left" | "start" => b.left = l,
                    "right" | "end" => b.right = l,
                    "between" => b.between = l,
                    _ => {}
                }
                if matches!(ev, Ev::Start(_)) {
                    depth += 1;
                }
            }
            _ => {}
        }
    }
}

/// Reads `w:tabs` (the reader is past its start tag).
pub(crate) fn read_tabs(r: &mut Reader, tabs: &mut TabStops) {
    let mut depth = 1;
    loop {
        let ev = r.next();
        match ev {
            Ev::Eof => return,
            Ev::End(_) => {
                depth -= 1;
                if depth == 0 {
                    return;
                }
            }
            Ev::Start(t) | Ev::Empty(t) => {
                if t.local() == "tab" {
                    let pos = t.attr("w:pos").and_then(|v| v.parse::<f32>().ok()).map(|v| v / TWIP);
                    let align = match t.attr("w:val").as_deref() {
                        Some("center") => Some(TabAlign::Center),
                        Some("right" | "end") => Some(TabAlign::Right),
                        Some("decimal") => Some(TabAlign::Decimal),
                        Some("clear" | "bar") => None,
                        _ => Some(TabAlign::Left),
                    };
                    let leader = match t.attr("w:leader").as_deref() {
                        Some("dot" | "middleDot") => TabLeader::Dots,
                        Some("hyphen") => TabLeader::Dashes,
                        Some("underscore" | "heavy") => TabLeader::Underline,
                        _ => TabLeader::None,
                    };
                    match (pos, align) {
                        (Some(pos), Some(align)) if pos >= 0.0 => tabs.set(TabStop { pos, align, leader }),
                        (Some(pos), None) => tabs.remove(pos),
                        _ => {}
                    }
                }
                if matches!(ev, Ev::Start(_)) {
                    depth += 1;
                }
            }
            _ => {}
        }
    }
}

/// The drop cap of a `w:framePr`.
pub(crate) fn frame_cap(t: &Tag) -> Option<DropCap> {
    let margin = match t.attr("w:dropCap").as_deref() {
        Some("drop") => false,
        Some("margin") => true,
        _ => return None,
    };
    let lines = t.attr("w:lines").and_then(|v| v.parse::<u8>().ok()).unwrap_or(3);
    let distance = t.attr("w:hSpace").and_then(|v| v.parse::<f32>().ok()).map_or(0.0, |v| v / TWIP);
    Some(DropCap { lines, margin, chars: 1, distance }.sane())
}

/// Where a frame that isn't a drop cap sits across the margins, when text
/// flows around it.
pub(crate) fn frame_float(t: &Tag) -> Option<apex_doc::Align> {
    if t.attr("w:dropCap").is_some_and(|v| v != "none") || t.attr("w:wrap").is_some_and(|v| v == "none" || v == "notBeside") {
        return None;
    }
    match t.attr("w:xAlign").as_deref() {
        Some("right" | "outside") => Some(apex_doc::Align::Right),
        Some("center") => Some(apex_doc::Align::Center),
        Some("left" | "inside") => Some(apex_doc::Align::Left),
        _ => None,
    }
}

/// Joins a drop cap paragraph (Word keeps the letter in a frame paragraph
/// of its own) to the paragraph after it.
pub(crate) fn merge_cap(cap: Paragraph, dc: DropCap, p: Paragraph) -> Paragraph {
    let mut out = Paragraph::new("");
    for (s, e, a) in cap.spans() {
        // The frame's large size and lowered position are the cap's own.
        let a = apex_doc::Attrs { size: None, superscript: false, subscript: false, ..a.clone() };
        out.push(&cap.text[s..e], a);
    }
    let n = out.text.len();
    for (s, e, a) in p.spans() {
        out.push(&p.text[s..e], a.clone());
    }
    if out.runs.len() > 1 && out.runs[0].len == 0 {
        out.runs.remove(0);
    }
    out.marks = cap.marks;
    out.marks.extend(p.marks.into_iter().map(|m| apex_doc::Mark { offset: m.offset + n, ..m }));
    let chars = cap.text.chars().count().clamp(1, 255) as u8;
    out.style = ParaStyle { drop_cap: Some(DropCap { chars, ..dc }), ..p.style };
    out.cell = p.cell;
    out.section = p.section;
    out
}

/// Puts the sections read into the document: the first is the document's
/// setup and each later one starts at the paragraph after the previous
/// one's end. `ends` are the paragraph indexes each section ends before,
/// `infos` the sections (one more than `ends`, the last from the body).
pub(crate) fn apply_sections(doc: &mut Document, ends: &[usize], infos: &[SectInfo]) {
    let Some(first) = infos.first() else { return };
    doc.page = first.page;
    doc.columns = first.columns.clone();
    doc.headers = first.headers.clone();
    if let Some(b) = infos.iter().find_map(|s| s.borders) {
        doc.deco.borders = b;
    }
    // The last section's end and where it started: the cells between
    // aren't looked at again.
    let mut last = (0, 0);
    for (k, info) in infos.iter().enumerate().skip(1) {
        let Some(&end) = ends.get(k - 1) else { break };
        // A section starting with a table starts at the paragraph after it.
        let from = if (last.0..=last.1).contains(&end) { last.1 } else { end };
        let Some(i) = (from..doc.paragraphs.len()).find(|&i| doc.paragraphs[i].cell.is_none()) else { break };
        last = (end, i);
        if i == 0 {
            continue;
        }
        doc.paragraphs[i].section = Some(Box::new(Section { start: info.start, page: info.page, columns: info.columns.clone(), headers: info.headers.clone() }));
    }
}

/// The text or picture watermark in the header parts of a document.
pub(crate) fn read_watermark(z: &ZipReader, rels: &HashMap<String, String>, refs: &crate::docx_hf::Refs) -> Option<Watermark> {
    let mut ids: Vec<(&(bool, u8), &String)> = refs.parts.iter().filter(|(k, _)| k.0).collect();
    ids.sort();
    for (_, id) in ids {
        let Some(target) = rels.get(id.as_str()) else { continue };
        let path = crate::xlsx::resolve_target("word", target);
        let Ok(xml) = z.read_string(&path) else { continue };
        if let Some(mut w) = watermark_in(&xml) {
            if let Some(rid) = w.1.take() {
                let (dir, file) = path.rsplit_once('/').unwrap_or(("", path.as_str()));
                let part_rels = z.read_string(&format!("{dir}/_rels/{file}.rels")).map(|s| crate::docx::read_rels(&s)).unwrap_or_default();
                let media = crate::docx_img::load_media(z, &part_rels, dir);
                w.0.picture = media.get(&rid).cloned();
                let Some(data) = &w.0.picture else { continue };
                // The shape's width (kept in `scale`) against the natural one.
                let natural = crate::pixel_size(data).map_or(0.0, |(pw, _)| pw as f32 * 0.75);
                w.0.scale = if natural > 0.0 && w.0.scale > 0.0 { (w.0.scale / natural * 100.0).round() / 100.0 } else { 0.0 };
            } else {
                w.0.scale = 0.0;
            }
            return Some(w.0);
        }
    }
    None
}

/// A VML watermark shape in a header part, and the relationship of its
/// picture.
fn watermark_in(xml: &str) -> Option<(Watermark, Option<String>)> {
    let mut r = Reader::new(xml);
    let mut cur: Option<(Watermark, Option<String>)> = None;
    loop {
        match r.next() {
            Ev::Eof => return None,
            Ev::Start(t) | Ev::Empty(t) => match t.local() {
                "shape" if cur.is_none() => {
                    let id = t.attr("id").unwrap_or_default().to_ascii_lowercase();
                    if !id.contains("watermark") {
                        continue;
                    }
                    let style = t.attr("style").unwrap_or_default().to_string();
                    let rot = css(&style, "rotation").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
                    let color = t.attr("fillcolor").and_then(|c| vml_color(&c)).unwrap_or(0xC0C0C0);
                    let diagonal = (rot.rem_euclid(180.0) - 0.0).abs() > 1.0;
                    // The width, until the picture's natural width is known.
                    let scale = css(&style, "width").and_then(crate::parse_length_pt).unwrap_or(0.0);
                    cur = Some((Watermark { color, diagonal, semitransparent: false, scale, ..Default::default() }, None));
                }
                "fill" => {
                    if let Some((w, _)) = &mut cur {
                        w.semitransparent = t.attr("opacity").and_then(|o| vml_fraction(&o)).is_some_and(|o| o < 0.99);
                    }
                }
                "textpath" => {
                    if let Some((w, _)) = &mut cur {
                        w.text = t.attr("string").unwrap_or_default().trim().to_string();
                        let style = t.attr("style").unwrap_or_default().to_string();
                        w.font = css(&style, "font-family").map(|f| f.trim_matches(['"', '\'', ' ']).to_string()).filter(|f| !f.is_empty()).map(|f| Arc::from(f.as_str()));
                        // 1pt means "auto": the text fits the shape.
                        w.size = css(&style, "font-size").and_then(|v| v.trim_end_matches("pt").trim().parse::<f32>().ok()).filter(|&s| s > 1.5);
                    }
                }
                "imagedata" => {
                    if let Some((w, rid)) = &mut cur {
                        *rid = t.attr("r:id").or_else(|| t.attr("o:relid")).map(|v| v.into_owned());
                        w.washout = t.attr("gain").is_some() || t.attr("blacklevel").is_some();
                    }
                }
                _ => {}
            },
            Ev::End(n) if local(n) == "shape" => {
                if let Some((w, rid)) = cur.take() {
                    if !w.text.is_empty() || rid.is_some() {
                        return Some((w, rid));
                    }
                }
            }
            _ => {}
        }
    }
}

/// A property of a CSS-like style attribute.
fn css<'a>(style: &'a str, key: &str) -> Option<&'a str> {
    style.split(';').find_map(|p| {
        let (k, v) = p.split_once(':')?;
        (k.trim().eq_ignore_ascii_case(key)).then(|| v.trim())
    })
}

/// A VML fraction: ".5", "0.5" or "32768f" (65536ths).
fn vml_fraction(s: &str) -> Option<f32> {
    match s.strip_suffix('f') {
        Some(v) => v.parse::<f32>().ok().map(|v| v / 65536.0),
        None => s.parse::<f32>().ok(),
    }
}

/// A VML color: "#RRGGBB", "#RGB" or a name.
fn vml_color(s: &str) -> Option<u32> {
    let s = s.split_whitespace().next()?;
    if let Some(h) = s.strip_prefix('#') {
        if h.len() == 3 {
            let v = u32::from_str_radix(h, 16).ok()?;
            let (r, g, b) = ((v >> 8) & 0xF, (v >> 4) & 0xF, v & 0xF);
            return Some((r * 17) << 16 | (g * 17) << 8 | b * 17);
        }
        return parse_hex_color(h);
    }
    Some(match s.to_ascii_lowercase().as_str() {
        "silver" => 0xC0C0C0,
        "gray" | "grey" => 0x808080,
        "black" => 0,
        "white" => 0xFFFFFF,
        "red" => 0xFF0000,
        "maroon" => 0x800000,
        "yellow" => 0xFFFF00,
        "olive" => 0x808000,
        "lime" => 0x00FF00,
        "green" => 0x008000,
        "aqua" => 0x00FFFF,
        "teal" => 0x008080,
        "blue" => 0x0000FF,
        "navy" => 0x000080,
        "fuchsia" => 0xFF00FF,
        "purple" => 0x800080,
        _ => return parse_hex_color(s),
    })
}

/// The page background color (`w:background` of the document).
pub(crate) fn background(t: &Tag) -> Option<u32> {
    t.attr("w:color").filter(|v| v != "auto").and_then(|v| parse_hex_color(&v))
}

/// The page background of a `w:background` element with content: its
/// color and a VML gradient (`v:background` holding a gradient `v:fill`).
pub(crate) fn read_background(r: &mut Reader, t: &Tag) -> (Option<u32>, Option<apex_doc::Gradient>) {
    let color = background(t);
    let mut grad = None;
    let mut fillcolor = None;
    let mut depth = 1;
    loop {
        let ev = r.next();
        match &ev {
            Ev::Eof => break,
            Ev::End(_) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Ev::Start(t) | Ev::Empty(t) => {
                if matches!(ev, Ev::Start(_)) {
                    depth += 1;
                }
                match t.local() {
                    "background" => fillcolor = t.attr("fillcolor").and_then(|c| vml_color(&c)),
                    "fill" if t.attr("type").is_some_and(|v| v.starts_with("gradient")) => grad = Some(vml_gradient(t, fillcolor.or(color).unwrap_or(0xFFFFFF))),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    match grad {
        Some(g) => (Some(color.unwrap_or(g.main_color())), Some(g)),
        None => (color, None),
    }
}

/// A VML gradient `v:fill` from `color` (the shape's fill color).
fn vml_gradient(t: &Tag, color: u32) -> apex_doc::Gradient {
    use apex_doc::{GradKind, GradStop, Gradient};
    let color2 = t.attr("color2").and_then(|c| vml_color(&c)).unwrap_or(0xFFFFFF);
    let num = |v: &str| {
        let v = v.trim();
        match v.strip_suffix('%') {
            Some(p) => p.trim().parse::<f32>().ok().map(|x| x / 100.0),
            None => vml_fraction(v),
        }
        .filter(|x| x.is_finite())
    };
    // `colors`: "0 #rrggbb;.5 #rrggbb;1 #rrggbb".
    let mut stops: Vec<GradStop> = t
        .attr("colors")
        .map(|c| {
            c.split(';')
                .filter_map(|p| {
                    let mut it = p.split_whitespace();
                    Some(GradStop::new(num(it.next()?)?.clamp(0.0, 1.0), vml_color(it.next()?)?))
                })
                .collect()
        })
        .unwrap_or_default();
    if stops.len() < 2 {
        stops = vec![GradStop::new(0.0, color), GradStop::new(1.0, color2)];
    }
    let op = |k: &str| t.attr(k).and_then(|v| num(&v)).map_or(1.0, |o| o.clamp(0.0, 1.0));
    let (o1, o2) = (op("opacity"), op("o:opacity2"));
    let (lo, hi) = (stops.first().map_or(0.0, |s| s.pos), stops.last().map_or(1.0, |s| s.pos));
    for s in &mut stops {
        let k = if hi > lo { (s.pos - lo) / (hi - lo) } else { 0.0 };
        s.alpha = o1 + (o2 - o1) * k;
    }
    // Focus 100% (or -100%): the colors run backward.
    if t.attr("focus").and_then(|v| num(&v)).is_some_and(|f| f.abs() >= 0.99) {
        stops = stops.into_iter().rev().map(|s| GradStop { pos: 1.0 - s.pos, ..s }).collect();
    }
    let mut g = if t.attr("type").as_deref() == Some("gradientRadial") {
        // Word's "from center": rectangles (ours are circles when we say so).
        let kind = if t.attr("o:title").as_deref() == Some("radial") { GradKind::Radial } else { GradKind::Rect };
        Gradient { kind, angle: 0.0, stops }
    } else {
        // VML turns counterclockwise, 0 running from the bottom up (checked
        // against LibreOffice).
        let a = t.attr("angle").and_then(|v| v.trim().parse::<f32>().ok()).filter(|a| a.is_finite()).unwrap_or(0.0);
        Gradient { kind: GradKind::Linear, angle: (270.0 - a).rem_euclid(360.0), stops }
    };
    g.sanitize();
    g
}

/// A `w:background` for the page `color` or gradient `g` (a VML fill, as
/// Word writes gradient backgrounds).
pub(crate) fn write_background(out: &mut String, color: u32, g: Option<&apex_doc::Gradient>) {
    let hex = |c: u32| format!("#{:06x}", c & 0xFF_FFFF);
    let Some(g) = g else {
        let _ = write!(out, r#"<w:background w:color="{:06X}"/>"#, color & 0xFF_FFFF);
        return;
    };
    let (first, last) = (g.stops.first().map_or(color, |s| s.color), g.stops.last().map_or(color, |s| s.color));
    let _ = write!(out, r#"<w:background w:color="{:06X}"><v:background id="_x0000_s1025" o:bwmode="white" fillcolor="{}" o:targetscreensize="1024,768">"#, first & 0xFF_FFFF, hex(first));
    let colors: Vec<String> = g.stops.iter().map(|s| format!("{} {}", (s.pos * 1000.0).round() / 1000.0, hex(s.color))).collect();
    let (o1, o2) = (g.stops.first().map_or(1.0, |s| s.alpha), g.stops.last().map_or(1.0, |s| s.alpha));
    let _ = write!(out, r#"<v:fill color2="{}" colors="{}""#, hex(last), colors.join(";"));
    if o1 < 1.0 || o2 < 1.0 {
        let _ = write!(out, r#" opacity="{}" o:opacity2="{}""#, (o1 * 1000.0).round() / 1000.0, (o2 * 1000.0).round() / 1000.0);
    }
    match g.kind {
        apex_doc::GradKind::Linear => {
            let a = (270.0 - g.angle).rem_euclid(360.0);
            let _ = write!(out, r#" method="none" focus="0" type="gradient" angle="{}"/>"#, (a * 10.0).round() / 10.0);
        }
        k => {
            // From the center out, as the stops.
            let title = if k == apex_doc::GradKind::Radial { r#" o:title="radial""# } else { "" };
            let _ = write!(out, r#" method="none" focus="0" focusposition=".5,.5" focussize="" type="gradientRadial"{title}/>"#);
        }
    }
    out.push_str("</v:background></w:background>");
}

// --- writing ------------------------------------------------------------------------

fn line_val(s: LineStyle) -> &'static str {
    match s {
        LineStyle::Single => "single",
        LineStyle::Double => "double",
        LineStyle::Dotted => "dotted",
        LineStyle::Dashed => "dashed",
        LineStyle::DashDot => "dotDash",
        LineStyle::Thick => "thick",
        LineStyle::Wave => "wave",
    }
}

fn write_line(out: &mut String, tag: &str, l: &Line) {
    let sz = (l.width * 8.0).round().clamp(2.0, 96.0) as i32;
    let _ = write!(out, r#"<w:{tag} w:val="{}" w:sz="{sz}" w:space="{}" w:color="{:06X}"/>"#, line_val(l.style), l.space.round().clamp(0.0, 31.0) as i32, l.color & 0xFF_FFFF);
}

/// `w:pBdr` (goes before `w:shd`).
pub(crate) fn write_pbdr(out: &mut String, st: &ParaStyle) {
    let b = &st.borders;
    if b.is_empty() {
        return;
    }
    out.push_str("<w:pBdr>");
    for (tag, l) in [("top", &b.top), ("left", &b.left), ("bottom", &b.bottom), ("right", &b.right), ("between", &b.between)] {
        if let Some(l) = l {
            write_line(out, tag, l);
        }
    }
    out.push_str("</w:pBdr>");
}

/// `w:tabs` (goes after `w:shd`).
pub(crate) fn write_tabs(out: &mut String, st: &ParaStyle) {
    if st.tabs.is_empty() {
        return;
    }
    out.push_str("<w:tabs>");
    for t in st.tabs.as_slice() {
        let val = match t.align {
            TabAlign::Left => "left",
            TabAlign::Center => "center",
            TabAlign::Right => "right",
            TabAlign::Decimal => "decimal",
        };
        let leader = match t.leader {
            TabLeader::None => "",
            TabLeader::Dots => r#" w:leader="dot""#,
            TabLeader::Dashes => r#" w:leader="hyphen""#,
            TabLeader::Underline => r#" w:leader="underscore""#,
        };
        let _ = write!(out, r#"<w:tab w:val="{val}"{leader} w:pos="{}"/>"#, (t.pos * TWIP).round() as i32);
    }
    out.push_str("</w:tabs>");
}

/// Splits a paragraph with a drop cap into the cap's frame paragraph
/// (written by `write_cap_ppr` and the caller's runs) and the rest.
pub(crate) fn split_cap(p: &Paragraph) -> Option<(Paragraph, Paragraph)> {
    let dc = p.style.drop_cap?;
    let end = p.text.char_indices().nth(dc.chars.max(1) as usize).map_or(p.text.len(), |(i, _)| i);
    if end == 0 || end >= p.text.len() || p.text[..end].chars().any(|c| c.is_whitespace() || c == apex_doc::OBJECT) || p.cell.is_some() {
        return None;
    }
    let mut cap = Paragraph::new("");
    let mut rest = Paragraph::new("");
    for (s, e, a) in p.spans() {
        if s < end {
            cap.push(&p.text[s..e.min(end)], a.clone());
        }
        if e > end {
            rest.push(&p.text[s.max(end)..e], a.clone());
        }
    }
    for m in &p.marks {
        if m.offset < end {
            cap.marks.push(m.clone());
        } else {
            rest.marks.push(apex_doc::Mark { offset: m.offset - end, ..m.clone() });
        }
    }
    rest.style = ParaStyle { drop_cap: None, ..p.style };
    rest.section = p.section.clone();
    Some((cap, rest))
}

/// Paragraph properties of a drop cap frame, and the size of its letter
/// in half points.
pub(crate) fn cap_ppr(st: &ParaStyle, base_hp: u16) -> (String, u32) {
    let Some(dc) = st.drop_cap else { return (String::new(), 0) };
    let base = base_hp as f32 / 2.0;
    let line = base * 1.15;
    let lines = dc.lines.max(1) as f32;
    let size = ((lines - 1.0) * line + 0.7 * base) / 0.7;
    let exact = (lines * line * TWIP).round() as i32;
    let ppr = format!(
        r#"<w:keepNext/><w:framePr w:dropCap="{}" w:lines="{}" w:hSpace="{}" w:wrap="around" w:vAnchor="text" w:hAnchor="{}"/><w:spacing w:before="0" w:after="0" w:line="{exact}" w:lineRule="exact"/><w:textAlignment w:val="baseline"/>"#,
        if dc.margin { "margin" } else { "drop" },
        dc.lines,
        (dc.distance * TWIP).round() as i32,
        if dc.margin { "page" } else { "text" },
    );
    (ppr, (size * 2.0).round() as u32)
}

/// The largest page margin Word takes, in twips (22 inches).
const MAX_MARGIN: i32 = 31680;

/// A section's `w:sectPr`.
pub(crate) fn sect_xml(refs: &str, headers: &apex_doc::Headers, page: &PageSetup, cols: &Columns, start: SectionStart, borders: &PageBorders) -> String {
    let tw = |v: f32| (v * TWIP).round() as i32;
    let mut s = String::from("<w:sectPr>");
    s.push_str(refs);
    let ty = match start {
        SectionStart::NextPage => "",
        SectionStart::Continuous => "continuous",
        SectionStart::EvenPage => "evenPage",
        SectionStart::OddPage => "oddPage",
    };
    if !ty.is_empty() {
        let _ = write!(s, r#"<w:type w:val="{ty}"/>"#);
    }
    let _ = write!(s, r#"<w:pgSz w:w="{}" w:h="{}"{}/>"#, tw(page.width), tw(page.height), if page.landscape() { r#" w:orient="landscape""# } else { "" });
    let _ = write!(
        s,
        r#"<w:pgMar w:top="{}" w:right="{}" w:bottom="{}" w:left="{}" w:header="{}" w:footer="{}" w:gutter="{}"/>"#,
        tw(page.margin_top).clamp(-MAX_MARGIN, MAX_MARGIN),
        tw(page.margin_right).clamp(0, MAX_MARGIN),
        tw(page.margin_bottom).clamp(-MAX_MARGIN, MAX_MARGIN),
        tw(page.margin_left).clamp(0, MAX_MARGIN),
        tw(page.margin_header).clamp(0, MAX_MARGIN),
        tw(page.margin_footer).clamp(0, MAX_MARGIN),
        tw(page.gutter).clamp(0, MAX_MARGIN)
    );
    if !borders.is_empty() {
        let _ = write!(s, r#"<w:pgBorders w:offsetFrom="{}">"#, if borders.from_edge { "page" } else { "text" });
        for (tag, l) in [("top", &borders.top), ("left", &borders.left), ("bottom", &borders.bottom), ("right", &borders.right)] {
            if let Some(l) = l {
                write_line(&mut s, tag, l);
            }
        }
        s.push_str("</w:pgBorders>");
    }
    if let Some(l) = page.line_numbers {
        let _ = write!(s, r#"<w:lnNumType w:countBy="{}""#, l.count_by.max(1));
        if l.start > 1 {
            let _ = write!(s, r#" w:start="{}""#, l.start - 1);
        }
        if let Some(d) = l.distance {
            let _ = write!(s, r#" w:distance="{}""#, tw(d));
        }
        let restart = match l.restart {
            apex_doc::LineRestart::EachPage => "newPage",
            apex_doc::LineRestart::EachSection => "newSection",
            apex_doc::LineRestart::Continuous => "continuous",
        };
        let _ = write!(s, r#" w:restart="{restart}"/>"#);
    }
    let n = headers.numbering;
    if n != apex_doc::PageNumbering::default() {
        s.push_str("<w:pgNumType");
        if n.format != apex_doc::NoteFormat::Arabic {
            let _ = write!(s, r#" w:fmt="{}""#, crate::docx::notes::format_name(n.format));
        }
        if let Some(v) = n.start {
            let _ = write!(s, r#" w:start="{v}""#);
        }
        s.push_str("/>");
    }
    if !cols.is_single() {
        let sep = if cols.separator { r#" w:sep="1""# } else { "" };
        if cols.custom.len() > 1 {
            let _ = write!(s, r#"<w:cols w:num="{}"{sep} w:equalWidth="0">"#, cols.n());
            for (i, (w, g)) in cols.custom.iter().take(cols.n()).enumerate() {
                if i + 1 < cols.n() {
                    let _ = write!(s, r#"<w:col w:w="{}" w:space="{}"/>"#, tw(*w), tw(*g));
                } else {
                    let _ = write!(s, r#"<w:col w:w="{}"/>"#, tw(*w));
                }
            }
            s.push_str("</w:cols>");
        } else {
            let _ = write!(s, r#"<w:cols w:num="{}" w:space="{}"{sep}/>"#, cols.n(), tw(cols.spacing));
        }
    } else {
        s.push_str(r#"<w:cols w:space="720"/>"#);
    }
    let valign = match page.v_align {
        apex_doc::PageVAlign::Top => "",
        apex_doc::PageVAlign::Center => "center",
        apex_doc::PageVAlign::Justify => "both",
        apex_doc::PageVAlign::Bottom => "bottom",
    };
    if !valign.is_empty() {
        let _ = write!(s, r#"<w:vAlign w:val="{valign}"/>"#);
    }
    if headers.different_first {
        s.push_str("<w:titlePg/>");
    }
    crate::docx_asian::write_sect(&mut s, page);
    s.push_str("</w:sectPr>");
    s
}

/// Word's VML shape type for text effects (WordArt), used by watermarks.
const TEXT_SHAPETYPE: &str = r##"<v:shapetype id="_x0000_t136" coordsize="21600,21600" o:spt="136" adj="10800" path="m@7,l@8,m@5,21600l@6,21600e"><v:formulas><v:f eqn="sum #0 0 10800"/><v:f eqn="prod #0 2 1"/><v:f eqn="sum 21600 0 @1"/><v:f eqn="sum 0 0 @2"/><v:f eqn="sum 21600 0 @3"/><v:f eqn="if @0 @3 0"/><v:f eqn="if @0 21600 @1"/><v:f eqn="if @0 0 @2"/><v:f eqn="if @0 @4 21600"/><v:f eqn="mid @5 @6"/><v:f eqn="mid @8 @5"/><v:f eqn="mid @7 @8"/><v:f eqn="mid @6 @7"/><v:f eqn="sum @6 0 @5"/></v:formulas><v:path textpathok="t" o:connecttype="custom" o:connectlocs="@9,0;@10,10800;@11,21600;@12,10800" o:connectangles="270,180,90,0"/><v:textpath on="t" fitshape="t"/><v:handles><v:h position="#0,bottomRight" xrange="6629,14971"/></v:handles><o:lock v:ext="edit" text="t" shapetype="t"/></v:shapetype>"##;

const PICTURE_SHAPETYPE: &str = r##"<v:shapetype id="_x0000_t75" coordsize="21600,21600" o:spt="75" o:preferrelative="t" path="m@4@5l@4@11@9@11@9@5xe" filled="f" stroked="f"><v:stroke joinstyle="miter"/><v:formulas><v:f eqn="if lineDrawn pixelLineWidth 0"/><v:f eqn="sum @0 1 0"/><v:f eqn="sum 0 0 @1"/><v:f eqn="prod @2 1 2"/><v:f eqn="prod @3 21600 pixelWidth"/><v:f eqn="prod @3 21600 pixelHeight"/><v:f eqn="sum @0 0 1"/><v:f eqn="prod @6 1 2"/><v:f eqn="prod @7 21600 pixelWidth"/><v:f eqn="sum @8 21600 0"/><v:f eqn="prod @7 21600 pixelHeight"/><v:f eqn="sum @10 21600 0"/></v:formulas><v:path o:extrusionok="f" gradientshapeok="t" o:connecttype="rect"/><o:lock v:ext="edit" aspectratio="t"/></v:shapetype>"##;

/// Namespaces a header with a watermark needs.
pub(crate) const VML_NS: &str = r#" xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:w10="urn:schemas-microsoft-com:office:word""#;

/// The watermark paragraph of a header part, as Word writes it; `pic` is
/// the relationship id and pixel size of a picture watermark.
pub(crate) fn watermark_xml(w: &Watermark, page: &PageSetup, pic: Option<(&str, (u32, u32))>) -> String {
    let pos = "position:absolute;margin-left:0;margin-top:0;z-index:-251657216;mso-position-horizontal:center;mso-position-horizontal-relative:margin;mso-position-vertical:center;mso-position-vertical-relative:margin";
    let mut s = String::from(r#"<w:p><w:pPr><w:spacing w:before="0" w:after="0"/></w:pPr><w:r><w:rPr><w:noProof/></w:rPr><w:pict>"#);
    match (&w.picture, pic) {
        (Some(_), Some((rid, (pw, ph)))) => {
            let (nw, nh) = (pw.max(1) as f32 * 0.75, ph.max(1) as f32 * 0.75);
            let fit = (page.content_width() / nw).min(page.content_height() / nh);
            let k = if w.scale > 0.0 { w.scale } else { fit };
            let wash = if w.washout { r#" gain="19661f" blacklevel="22938f""# } else { "" };
            let _ = write!(
                s,
                r##"{PICTURE_SHAPETYPE}<v:shape id="WordPictureWatermark1" o:spid="_x0000_s2050" type="#_x0000_t75" style="{pos};width:{:.1}pt;height:{:.1}pt" o:allowincell="f"><v:imagedata r:id="{rid}" o:title=""{wash}/></v:shape>"##,
                nw * k,
                nh * k
            );
        }
        _ => {
            let n = w.text.chars().count().max(1) as f32;
            let (width, height) = match w.size {
                Some(sz) => (sz * n * 0.62, sz),
                None => {
                    let width = if w.diagonal { page.content_width().min(page.content_height()) } else { page.content_width() * 0.9 };
                    (width, width / (n * 0.62).max(1.0))
                }
            };
            let rot = if w.diagonal { "rotation:315;" } else { "" };
            let font = w.font.as_deref().unwrap_or("Calibri");
            let size = w.size.map_or("1pt".to_string(), |s| format!("{s}pt"));
            let opacity = if w.semitransparent { r#"<v:fill opacity=".5"/>"# } else { "" };
            let _ = write!(
                s,
                r##"{TEXT_SHAPETYPE}<v:shape id="PowerPlusWaterMarkObject1" o:spid="_x0000_s2049" type="#_x0000_t136" style="{pos};{rot}width:{width:.1}pt;height:{height:.1}pt" o:allowincell="f" fillcolor="#{:06X}" stroked="f">{opacity}<v:textpath style="font-family:&quot;{}&quot;;font-size:{size}" string="{}"/><w10:wrap anchorx="margin" anchory="margin"/></v:shape>"##,
                w.color & 0xFF_FFFF,
                esc(font),
                esc(&w.text)
            );
        }
    }
    s.push_str("</w:pict></w:r></w:p>");
    s
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn vml_values() {
        assert_eq!(vml_color("silver"), Some(0xC0C0C0));
        assert_eq!(vml_color("#f00"), Some(0xFF0000));
        assert_eq!(vml_color("#123456 [3]"), Some(0x123456));
        assert_eq!(vml_fraction(".5"), Some(0.5));
        assert_eq!(vml_fraction("32768f"), Some(0.5));
        assert_eq!(css("position:absolute;rotation:315;width:1pt", "rotation"), Some("315"));
    }

    /// A document with every page layout feature, shared with the odt tests.
    pub(crate) fn layout_sample() -> Document {
        let mut ps: Vec<Paragraph> = ["Primero", "Dos columnas", "más texto", "Siguiente columna", "Apaisada", "Par", "Fin"].iter().map(|t| Paragraph::new(*t)).collect();
        let base = PageSetup::A4;
        ps[1].section = Some(Box::new(Section { headers: Default::default(), start: SectionStart::Continuous, page: base, columns: Columns { separator: true, ..Columns::equal(2, 24.0) } }));
        ps[3].style.column_break_before = true;
        let mut land = base;
        land.set_landscape(true);
        ps[4].section = Some(Box::new(Section { headers: Default::default(), start: SectionStart::NextPage, page: land, columns: Columns { count: 2, spacing: 36.0, separator: false, custom: vec![(200.0, 30.0), (400.0, 0.0)] } }));
        ps[5].section = Some(Box::new(Section { headers: Default::default(), start: SectionStart::EvenPage, page: base, columns: Columns::default() }));
        // Roman numbers first, then from 1, then letters going on.
        if let Some(s) = ps[4].section.as_deref_mut() {
            s.headers.numbering = apex_doc::PageNumbering { format: apex_doc::NoteFormat::Arabic, start: Some(1) };
        }
        if let Some(s) = ps[5].section.as_deref_mut() {
            s.headers.numbering = apex_doc::PageNumbering { format: apex_doc::NoteFormat::UpperLetter, start: None };
        }
        ps[0].style.tabs = TabStops::from_slice(&[TabStop { pos: 72.0, align: TabAlign::Center, leader: TabLeader::None }, TabStop { pos: 300.0, align: TabAlign::Right, leader: TabLeader::Dots }, TabStop { pos: 350.0, align: TabAlign::Decimal, leader: TabLeader::Dashes }]);
        ps[0].text = "a\tb\tc\t1.5".into();
        ps[0].runs[0].len = ps[0].text.len();
        let l = Line { style: LineStyle::Double, width: 0.75, color: 0x1F4E79, space: 4.0 };
        ps[1].style.borders = ParaBorders { top: Some(l), bottom: Some(Line { style: LineStyle::Dotted, ..l }), left: Some(Line { style: LineStyle::Single, ..l }), right: Some(l), between: Some(Line { style: LineStyle::Dashed, width: 0.5, color: 0, space: 1.0 }) };
        ps[1].style.shading = Some(0xEEEEEE);
        ps[2].style.borders = ps[1].style.borders;
        ps[6].style.drop_cap = Some(DropCap { lines: 3, margin: false, chars: 1, distance: 6.0 });
        ps[6].text = "Final con letra capital y bastante texto para ocupar varias líneas. ".repeat(4).trim_end().to_string();
        ps[6].runs[0].len = ps[6].text.len();
        let mut d = Document::from_paragraphs(ps);
        d.page = base;
        d.headers.numbering = apex_doc::PageNumbering { format: apex_doc::NoteFormat::LowerRoman, start: Some(3) };
        d.deco.color = Some(0xFFF4E0);
        let pl = Line { style: LineStyle::Single, width: 1.5, color: 0x2E75B6, space: 24.0 };
        d.deco.borders = PageBorders { top: Some(pl), bottom: Some(pl), left: Some(pl), right: Some(pl), from_edge: true };
        d.deco.watermark = Some(Watermark { text: "BORRADOR".into(), font: Some("Arial".into()), color: 0xC0C0C0, semitransparent: true, diagonal: true, ..Default::default() });
        d
    }

    pub(crate) fn check_layout(back: &Document, d: &Document, exact_cols: bool) {
        let sa = d.sections();
        let sb = back.sections();
        assert_eq!(sb.len(), sa.len());
        for k in 0..sa.len() {
            assert_eq!(back.section_headers(k).numbering, d.section_headers(k).numbering, "section {k}");
        }
        for (a, b) in sa.iter().zip(&sb) {
            assert_eq!((a.first, a.start), (b.first, b.start));
            assert!((a.page.width - b.page.width).abs() < 0.1 && (a.page.height - b.page.height).abs() < 0.1);
            assert_eq!(a.columns.n(), b.columns.n());
            assert_eq!(a.columns.separator, b.columns.separator);
            let (la, lb) = (a.columns.layout(500.0), b.columns.layout(500.0));
            for (x, y) in la.iter().zip(&lb) {
                assert!((x.0 - y.0).abs() < if exact_cols { 0.5 } else { 3.0 } && (x.1 - y.1).abs() < if exact_cols { 0.5 } else { 3.0 }, "{la:?} {lb:?}");
            }
        }
        for (a, b) in d.paragraphs.iter().zip(&back.paragraphs) {
            assert_eq!(a.text, b.text);
            assert_eq!(a.style.column_break_before, b.style.column_break_before, "{}", a.text);
            assert_eq!(a.style.tabs, b.style.tabs, "{}", a.text);
            assert_eq!(a.style.borders, b.style.borders, "{}", a.text);
            assert_eq!(a.style.drop_cap, b.style.drop_cap, "{}", a.text);
            assert_eq!(a.style.shading, b.style.shading, "{}", a.text);
        }
        assert_eq!(back.deco, d.deco);
    }

    #[test]
    fn layout_roundtrip() {
        let d = layout_sample();
        let back = crate::docx::read(&crate::docx::write(&d)).unwrap();
        assert_eq!(back.paragraphs.len(), d.paragraphs.len());
        check_layout(&back, &d, true);
    }

    #[test]
    fn watermark_shape() {
        let w = Watermark { text: "BORRADOR & co".into(), font: Some("Arial".into()), size: None, color: 0xFF0000, semitransparent: true, diagonal: true, ..Default::default() };
        let xml = format!("<w:hdr{VML_NS}>{}</w:hdr>", watermark_xml(&w, &PageSetup::A4, None));
        let (back, rid) = watermark_in(&xml).unwrap();
        assert!(rid.is_none());
        assert_eq!((back.text.as_str(), back.font.as_deref(), back.color, back.semitransparent, back.diagonal, back.size), ("BORRADOR & co", Some("Arial"), 0xFF0000, true, true, None));
        let w = Watermark { diagonal: false, semitransparent: false, size: Some(72.0), ..w };
        let (back, _) = watermark_in(&watermark_xml(&w, &PageSetup::A4, None)).unwrap();
        assert!(!back.diagonal && !back.semitransparent && back.size == Some(72.0));
    }
}
