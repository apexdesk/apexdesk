//! `xl/styles.xml`: fonts, fills, borders, number formats, the cell formats
//! (`cellXfs`), the named cell styles (`cellStyleXfs`, `cellStyles` and the
//! cells' `xfId`) and theme colors (`theme=` + `tint=`).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_sheet::cellstyles::{self, CellStyles, NamedStyle, part};
use apex_sheet::sheet::{ROTATION_STACKED, THEME_MAJOR, THEME_MINOR, border};
use apex_sheet::{Borders, CellStyle, HAlign, Line, LineStyle, Theme, ThemeColor, VAlign, Workbook};

use crate::parse_hex_color;
use crate::xml::{Ev, HEADER, Reader, Tag, esc};

pub(crate) fn builtin_numfmt(id: u32) -> Option<&'static str> {
    Some(match id {
        0 => "General",
        1 => "0",
        2 => "0.00",
        3 => "#,##0",
        4 => "#,##0.00",
        5 => "#,##0 \"€\";-#,##0 \"€\"",
        6 => "#,##0 \"€\";[Red]-#,##0 \"€\"",
        7 => "#,##0.00 \"€\";-#,##0.00 \"€\"",
        8 => "#,##0.00 \"€\";[Red]-#,##0.00 \"€\"",
        9 => "0%",
        10 => "0.00%",
        11 => "0.00E+00",
        12 => "# ?/?",
        13 => "# ??/??",
        14 => "dd/mm/yyyy",
        15 => "d-mmm-yy",
        16 => "d-mmm",
        17 => "mmm-yy",
        18 => "h:mm AM/PM",
        19 => "h:mm:ss AM/PM",
        20 => "h:mm",
        21 => "h:mm:ss",
        22 => "dd/mm/yyyy h:mm",
        37 => "#,##0 ;(#,##0)",
        38 => "#,##0 ;[Red](#,##0)",
        39 => "#,##0.00;(#,##0.00)",
        40 => "#,##0.00;[Red](#,##0.00)",
        45 => "mm:ss",
        46 => "[h]:mm:ss",
        47 => "mm:ss.0",
        48 => "##0.0E+0",
        49 => "@",
        _ => return None,
    })
}

/// Number format of an id, including the locale's accounting formats
/// (41-44, which files don't spell out).
pub(crate) fn numfmt_code(id: u32, custom: &HashMap<u32, String>, wb: &Workbook) -> String {
    if let Some(c) = custom.get(&id) {
        return c.clone();
    }
    if id == 14 {
        return wb.locale.short_date_code().to_string();
    }
    if (41..=44).contains(&id) {
        let f = cellstyles::accounting_formats(&wb.locale);
        return f[[1, 3, 0, 2][(id - 41) as usize]].clone();
    }
    builtin_numfmt(id).unwrap_or("General").to_string()
}

fn truthy(t: &Tag, key: &str) -> bool {
    match t.attr(key) {
        None => true,
        Some(v) => !matches!(v.as_ref(), "0" | "false" | "none"),
    }
}

/// Excel numbers theme colors lt1, dk1, lt2, dk2, then the accents.
const THEME_INDEX: [u8; 12] = [1, 0, 3, 2, 4, 5, 6, 7, 8, 9, 10, 11];

fn theme_index(slot: u8) -> u8 {
    THEME_INDEX.iter().position(|&s| s == slot).unwrap_or(0) as u8
}

/// A color element: `rgb="FFRRGGBB"` or `theme="n" tint="x"`.
pub(crate) fn read_color(t: &Tag, theme: &Theme) -> Option<(u32, Option<ThemeColor>)> {
    if let Some(v) = t.attr("rgb") {
        return parse_hex_color(&v).map(|c| (c, None));
    }
    let i: usize = t.attr("theme")?.trim().parse().ok()?;
    let slot = *THEME_INDEX.get(i)?;
    let tint = t.attr("tint").and_then(|v| v.trim().parse::<f64>().ok()).unwrap_or(0.0);
    let tc = ThemeColor::from_tint_f(slot, tint);
    Some((theme.resolve(tc), Some(tc)))
}

/// A theme color by its index in files (`theme=`), lightened or darkened.
pub(crate) fn theme_color(theme: &Theme, i: usize, tint: f64) -> Option<(u32, Option<ThemeColor>)> {
    let tc = ThemeColor::from_tint_f(*THEME_INDEX.get(i)?, tint);
    Some((theme.resolve(tc), Some(tc)))
}

/// Attributes of a color element for an RGB and its theme reference.
fn color_attrs(rgb: u32, tc: Option<ThemeColor>, theme: &Theme) -> String {
    match cellstyles::valid_ref(Some(rgb), tc, theme) {
        Some(c) if c.tint != 0 => format!(r#"theme="{}" tint="{}""#, theme_index(c.slot), c.tint_f()),
        Some(c) => format!(r#"theme="{}""#, theme_index(c.slot)),
        None => format!(r#"rgb="FF{:06X}""#, rgb & 0xFF_FFFF),
    }
}

fn line_style(v: &str) -> LineStyle {
    match v {
        "none" => LineStyle::None,
        "hair" => LineStyle::Hair,
        "medium" => LineStyle::Medium,
        "thick" => LineStyle::Thick,
        "double" => LineStyle::Double,
        "dotted" => LineStyle::Dotted,
        "dashed" | "dashDot" | "dashDotDot" | "mediumDashed" | "mediumDashDot" | "mediumDashDotDot" | "slantDashDot" => LineStyle::Dashed,
        _ => LineStyle::Thin,
    }
}

fn line_style_name(s: LineStyle) -> &'static str {
    match s {
        LineStyle::None => "none",
        LineStyle::Thin => "thin",
        LineStyle::Medium => "medium",
        LineStyle::Thick => "thick",
        LineStyle::Dashed => "dashed",
        LineStyle::Dotted => "dotted",
        LineStyle::Double => "double",
        LineStyle::Hair => "hair",
    }
}

// ---------------------------------------------------------------------------
// Reading

#[derive(Default, Clone)]
struct Font {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    size: f32,
    color: Option<(u32, Option<ThemeColor>)>,
    name: Option<Arc<str>>,
    /// 0 none, 1 major, 2 minor.
    scheme: u8,
}

/// Borders with the theme colors of the sides (top, bottom, left, right).
type BorderDef = (Borders, [Option<ThemeColor>; 4]);

fn side_slot(flag: u8) -> usize {
    match flag {
        border::TOP => 0,
        border::BOTTOM => 1,
        border::LEFT => 2,
        _ => 3,
    }
}

/// A parsed `xf`: the format, the parts it applies and its style (`xfId`).
struct Xf {
    style: CellStyle,
    parts: u8,
    xf_id: usize,
    /// `xfComplement` index (a check box, see `xlsx_incell`).
    complement: Option<u32>,
}

struct Tables {
    numfmts: HashMap<u32, String>,
    fonts: Vec<Font>,
    fills: Vec<Option<(u32, Option<ThemeColor>)>>,
    borders: Vec<BorderDef>,
}

impl Tables {
    fn xf(&self, t: &Tag, wb: &Workbook) -> Xf {
        let mut s = CellStyle::default();
        let num: u32 = t.attr("numFmtId").and_then(|v| v.parse().ok()).unwrap_or(0);
        s.num_fmt = Arc::from(numfmt_code(num, &self.numfmts, wb));
        let font = t.attr("fontId").and_then(|v| v.parse::<usize>().ok()).and_then(|i| self.fonts.get(i)).cloned().unwrap_or_default();
        let base = self.fonts.first().map_or(11.0, |f| f.size);
        let base_name = self.fonts.first().and_then(|f| f.name.clone());
        // The default font is no font, unless it is the headings font.
        s.font = font.name.clone().filter(|n| Some(n) != base_name.as_ref() || font.scheme == 1);
        if s.font.is_some() {
            s.theme.font = match font.scheme {
                1 => THEME_MAJOR,
                2 => THEME_MINOR,
                _ => 0,
            };
        }
        s.bold = font.bold;
        s.italic = font.italic;
        s.underline = font.underline;
        s.strike = font.strike;
        // Black (usually "Text 1") is the automatic color.
        if let Some((c, tc)) = font.color.filter(|&(c, _)| c != 0) {
            s.color = Some(c);
            s.theme.color = tc;
        }
        if (font.size - base).abs() > 0.01 && font.size > 0.0 {
            s.font_size = (font.size * 10.0).round() as u16;
        }
        if let Some((c, tc)) = t.attr("fillId").and_then(|v| v.parse::<usize>().ok()).and_then(|i| self.fills.get(i).copied()).flatten() {
            s.fill = Some(c);
            s.theme.fill = tc;
        }
        if let Some((b, refs)) = t.attr("borderId").and_then(|v| v.parse::<usize>().ok()).and_then(|i| self.borders.get(i).copied()) {
            s.borders = b;
            s.theme.borders = refs;
        }
        let mut parts = 0;
        for (key, p) in [("applyNumberFormat", part::NUMBER), ("applyFont", part::FONT), ("applyFill", part::FILL), ("applyBorder", part::BORDER), ("applyAlignment", part::ALIGN), ("applyProtection", part::PROTECT)] {
            if truthy(t, key) {
                parts |= p;
            }
        }
        let xf_id = t.attr("xfId").and_then(|v| v.parse().ok()).unwrap_or(0);
        Xf { style: s, parts, xf_id, complement: None }
    }
}

fn read_alignment(t: &Tag, s: &mut CellStyle) {
    s.halign = match t.attr("horizontal").as_deref() {
        Some("left") => HAlign::Left,
        Some("center") | Some("centerContinuous") => HAlign::Center,
        Some("right") => HAlign::Right,
        Some("justify") | Some("distributed") => HAlign::Justify,
        _ => HAlign::General,
    };
    s.indent = t.attr("indent").and_then(|v| v.parse::<u32>().ok()).map_or(0, |v| v.min(15) as u8);
    s.rotation = match t.attr("textRotation").and_then(|v| v.parse::<i32>().ok()) {
        Some(255) => ROTATION_STACKED,
        Some(v @ 1..=90) => v as i16,
        Some(v @ 91..=180) => -(v as i16 - 90),
        _ => 0,
    };
    s.valign = match t.attr("vertical").as_deref() {
        Some("top") => VAlign::Top,
        Some("center") => VAlign::Center,
        _ => VAlign::Bottom,
    };
    s.wrap = t.attr("wrapText").is_some_and(|v| v == "1" || v == "true");
}

/// Reads the styles into `wb` (named styles included) and returns the style
/// id of each `cellXfs` entry.
/// `checkboxes`: the `xfComplement` indices that are check boxes.
pub(crate) fn read_styles(xml: &str, wb: &mut Workbook, theme: &Theme, checkboxes: &std::collections::HashSet<u32>) -> Vec<u32> {
    let mut tb = Tables { numfmts: HashMap::new(), fonts: Vec::new(), fills: Vec::new(), borders: Vec::new() };
    let mut cell_xfs: Vec<Xf> = Vec::new();
    let mut style_xfs: Vec<Xf> = Vec::new();
    // (name, xfId, builtinId)
    let mut named: Vec<(String, usize, Option<u16>)> = Vec::new();
    let mut r = Reader::new(xml);
    let mut section = "";
    let mut cur_font: Option<Font> = None;
    let mut cur_fill: Option<Option<(u32, Option<ThemeColor>)>> = None;
    let mut cur_border: Option<BorderDef> = None;
    let mut cur_side: Option<u8> = None;
    // An open xf and whether it belongs to cellStyleXfs.
    let mut cur_xf: Option<(Xf, bool)> = None;
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) if matches!(t.local(), "numFmts" | "fonts" | "fills" | "borders" | "cellXfs" | "cellStyleXfs" | "cellStyles" | "dxfs") => {
                section = match t.local() {
                    "numFmts" => "numFmts",
                    "fonts" => "fonts",
                    "fills" => "fills",
                    "borders" => "borders",
                    "cellXfs" => "cellXfs",
                    "cellStyleXfs" => "cellStyleXfs",
                    "cellStyles" => "cellStyles",
                    _ => "other",
                };
            }
            Ev::End(n) if matches!(crate::xml::local(n), "numFmts" | "fonts" | "fills" | "borders" | "cellXfs" | "cellStyleXfs" | "cellStyles" | "dxfs") => {
                section = "";
            }
            ev @ (Ev::Start(_) | Ev::Empty(_)) => {
                let (t, empty) = match ev {
                    Ev::Start(t) => (t, false),
                    Ev::Empty(t) => (t, true),
                    _ => continue,
                };
                match (section, t.local()) {
                    ("numFmts", "numFmt") => {
                        if let (Some(id), Some(code)) = (t.attr("numFmtId").and_then(|v| v.parse().ok()), t.attr("formatCode")) {
                            // Excel's longest code: each style copies it.
                            tb.numfmts.insert(id, code.chars().take(255).collect());
                        }
                    }
                    ("fonts", "font") => {
                        let f = Font { size: 11.0, ..Default::default() };
                        if empty {
                            tb.fonts.push(f);
                        } else {
                            cur_font = Some(f);
                        }
                    }
                    ("fonts", name) => {
                        if let Some(f) = &mut cur_font {
                            match name {
                                "b" => f.bold = truthy(&t, "val"),
                                "i" => f.italic = truthy(&t, "val"),
                                "u" => f.underline = truthy(&t, "val"),
                                "strike" => f.strike = truthy(&t, "val"),
                                "sz" => f.size = t.attr("val").and_then(|v| v.parse().ok()).unwrap_or(11.0),
                                "color" => f.color = read_color(&t, theme),
                                "name" => f.name = t.attr("val").map(|v| Arc::from(v.as_ref())),
                                "scheme" => {
                                    f.scheme = match t.attr("val").as_deref() {
                                        Some("major") => 1,
                                        Some("minor") => 2,
                                        _ => 0,
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    ("fills", "fill") => {
                        if empty {
                            tb.fills.push(None);
                        } else {
                            cur_fill = Some(None);
                        }
                    }
                    ("fills", "patternFill") => {
                        if t.attr("patternType").as_deref() != Some("solid") {
                            cur_fill = Some(None);
                            // Non-solid: its colors are ignored.
                            section = "fills-skip";
                        }
                    }
                    ("fills", "fgColor") => {
                        if let Some(f) = &mut cur_fill {
                            *f = read_color(&t, theme);
                        }
                    }
                    ("borders", "border") => {
                        if empty {
                            tb.borders.push((Borders::NONE, [None; 4]));
                        } else {
                            cur_border = Some((Borders::NONE, [None; 4]));
                        }
                    }
                    ("borders", side @ ("left" | "right" | "top" | "bottom" | "start" | "end")) => {
                        let flag = match side {
                            "left" | "start" => border::LEFT,
                            "right" | "end" => border::RIGHT,
                            "top" => border::TOP,
                            _ => border::BOTTOM,
                        };
                        if let (Some((b, _)), Some(style)) = (&mut cur_border, t.attr("style")) {
                            b.set(flag, Line::new(line_style(&style), None));
                        }
                        cur_side = (!empty).then_some(flag);
                    }
                    ("borders", "color") => {
                        if let (Some((b, refs)), Some(flag), Some((c, tc))) = (&mut cur_border, cur_side, read_color(&t, theme)) {
                            let mut line = match flag {
                                border::LEFT => b.left,
                                border::RIGHT => b.right,
                                border::TOP => b.top,
                                _ => b.bottom,
                            };
                            if !line.is_none() && (c != 0 || tc.is_some()) {
                                line.color = Some(c);
                                b.set(flag, line);
                                refs[side_slot(flag)] = tc;
                            }
                        }
                    }
                    // As many formats as Excel keeps.
                    (sec @ ("cellXfs" | "cellStyleXfs"), "xf") if (if sec == "cellStyleXfs" { style_xfs.len() } else { cell_xfs.len() }) < 64_000 => {
                        let xf = tb.xf(&t, wb);
                        let is_style = sec == "cellStyleXfs";
                        if empty {
                            if is_style { style_xfs.push(xf) } else { cell_xfs.push(xf) }
                        } else {
                            cur_xf = Some((xf, is_style));
                        }
                    }
                    ("cellXfs" | "cellStyleXfs", "protection") => {
                        if let Some((x, _)) = &mut cur_xf {
                            x.style.unlocked = t.attr("locked").is_some_and(|v| v == "0" || v == "false");
                            x.style.formula_hidden = t.attr("hidden").is_some_and(|v| v == "1" || v == "true");
                        }
                    }
                    ("cellXfs", "xfComplement") => {
                        if let Some((x, _)) = &mut cur_xf {
                            x.complement = t.attr("i").and_then(|v| v.trim().parse().ok());
                        }
                    }
                    ("cellXfs" | "cellStyleXfs", "alignment") => {
                        if let Some((x, _)) = &mut cur_xf {
                            read_alignment(&t, &mut x.style);
                        }
                    }
                    ("cellStyles", "cellStyle") => {
                        let name = t.attr("name").unwrap_or_default().trim().to_string();
                        let xf_id = t.attr("xfId").and_then(|v| v.parse().ok()).unwrap_or(0);
                        let builtin = t.attr("builtinId").and_then(|v| v.parse::<u16>().ok());
                        if !name.is_empty() {
                            named.push((name, xf_id, builtin));
                        }
                    }
                    _ => {}
                }
            }
            Ev::End(n) => match crate::xml::local(n) {
                "font" => {
                    if let Some(f) = cur_font.take() {
                        tb.fonts.push(f);
                    }
                }
                "fill" => {
                    if section == "fills-skip" {
                        section = "fills";
                        cur_fill = Some(None);
                    }
                    if let Some(f) = cur_fill.take() {
                        tb.fills.push(f);
                    }
                }
                "border" => {
                    if let Some(b) = cur_border.take() {
                        tb.borders.push(b);
                    }
                }
                "left" | "right" | "top" | "bottom" | "start" | "end" => cur_side = None,
                "xf" => {
                    if let Some((x, is_style)) = cur_xf.take() {
                        if is_style { style_xfs.push(x) } else { cell_xfs.push(x) }
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    // Named styles: the built-ins the file defines replace ours.
    let mut styles = CellStyles::new(theme.clone(), &wb.locale);
    // xfId -> name of the style (None for Normal).
    let mut by_xf: HashMap<usize, Option<Arc<str>>> = HashMap::new();
    // Where each named style is in the list, by its name in lower case.
    let mut at: HashMap<String, usize> = styles.list.iter().enumerate().map(|(i, s)| (s.name.to_ascii_lowercase(), i)).collect();
    for (name, xf_id, builtin) in named {
        // Outline level styles (1, 2) aren't in the gallery.
        if builtin.is_some_and(|id| cellstyles::builtin_name(id).is_none()) {
            continue;
        }
        let (name, builtin) = cellstyles::file_style_name(&name, builtin);
        let Some(x) = style_xfs.get(xf_id) else { continue };
        let name: Arc<str> = Arc::from(name);
        let def = NamedStyle { name: name.clone(), builtin, parts: if builtin == Some(0) { part::ALL } else { x.parts }, style: x.style.clone() };
        match at.get(&name.to_ascii_lowercase()) {
            Some(&i) => styles.list[i] = def,
            // As many named styles as Excel keeps.
            None if styles.list.len() >= 64_000 => continue,
            None => {
                at.insert(name.to_ascii_lowercase(), styles.list.len());
                styles.list.push(def);
            }
        }
        by_xf.entry(xf_id).or_insert_with(|| (builtin != Some(0)).then_some(name));
    }
    wb.cell_styles = styles;
    cell_xfs
        .into_iter()
        .map(|x| {
            let mut s = x.style;
            s.named = by_xf.get(&x.xf_id).cloned().flatten();
            s.checkbox = x.complement.is_some_and(|i| checkboxes.contains(&i));
            wb.styles.intern(s)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Writing

pub(crate) fn builtin_id_for(code: &str) -> Option<u32> {
    [0u32, 1, 2, 3, 4, 9, 10, 11, 12, 13, 18, 19, 20, 21, 45, 46, 47, 48, 49].into_iter().find(|&id| builtin_numfmt(id) == Some(code))
}

/// Deduplicated lists of the style parts.
struct Parts<'a> {
    theme: &'a Theme,
    fonts: Vec<String>,
    fills: Vec<String>,
    borders: Vec<String>,
    numfmts: Vec<(u32, String)>,
    /// Where each part is in its list (a workbook may have many styles).
    at: std::collections::HashMap<String, usize>,
}

/// Index of `s` in `list`, adding it if new; `at` indexes the lists
/// (parts of different kinds never have the same text).
fn idx(list: &mut Vec<String>, at: &mut std::collections::HashMap<String, usize>, s: String) -> usize {
    if let Some(&i) = at.get(&s) {
        return i;
    }
    list.push(s.clone());
    at.insert(s, list.len() - 1);
    list.len() - 1
}

/// The ids of an xf's parts and its child elements.
struct XfIds {
    num: u32,
    font: usize,
    fill: usize,
    border: usize,
    align: Option<String>,
    protect: Option<String>,
    checkbox: bool,
}

impl Parts<'_> {
    fn font_xml(&self, st: &CellStyle) -> String {
        let t = self.theme;
        let mut f = String::from("<font>");
        if st.bold {
            f.push_str("<b/>");
        }
        if st.italic {
            f.push_str("<i/>");
        }
        if st.strike {
            f.push_str("<strike/>");
        }
        if st.underline {
            f.push_str("<u/>");
        }
        let size = if st.font_size > 0 { st.font_size as f32 / 10.0 } else { 11.0 };
        let _ = write!(f, r#"<sz val="{}"/>"#, size);
        match st.color {
            Some(c) => {
                let _ = write!(f, "<color {}/>", color_attrs(c, st.theme.color, t));
            }
            None => f.push_str(r#"<color theme="1"/>"#),
        }
        match &st.font {
            Some(name) => {
                let _ = write!(f, r#"<name val="{}"/>"#, esc(name));
                match cellstyles::theme_font_token(st, t) {
                    Some(apex_doc::theme::MAJOR_TOKEN) => f.push_str(r#"<family val="2"/><scheme val="major"/>"#),
                    Some(_) => f.push_str(r#"<family val="2"/><scheme val="minor"/>"#),
                    None => {}
                }
            }
            None => {
                let _ = write!(f, r#"<name val="{}"/><family val="2"/><scheme val="minor"/>"#, esc(&t.fonts.minor));
            }
        }
        f.push_str("</font>");
        f
    }

    fn ids(&mut self, st: &CellStyle) -> XfIds {
        let t = self.theme;
        let font = self.font_xml(st);
        let font = idx(&mut self.fonts, &mut self.at, font);
        let fill = match st.fill {
            Some(c) => idx(&mut self.fills, &mut self.at, format!(r#"<fill><patternFill patternType="solid"><fgColor {}/><bgColor indexed="64"/></patternFill></fill>"#, color_attrs(c, st.theme.fill, t))),
            None => 0,
        };
        let border = if st.borders.is_empty() {
            0
        } else {
            let side = |l: Line, name: &str, tc: Option<ThemeColor>| {
                if l.is_none() {
                    format!("<{name}/>")
                } else {
                    let color = l.color.map_or(r#"<color auto="1"/>"#.to_string(), |c| format!("<color {}/>", color_attrs(c, tc, t)));
                    format!(r#"<{name} style="{}">{color}</{name}>"#, line_style_name(l.style))
                }
            };
            let (b, r) = (&st.borders, &st.theme.borders);
            let b = format!("<border>{}{}{}{}<diagonal/></border>", side(b.left, "left", r[2]), side(b.right, "right", r[3]), side(b.top, "top", r[0]), side(b.bottom, "bottom", r[1]));
            idx(&mut self.borders, &mut self.at, b)
        };
        let code = st.num_fmt.as_ref();
        let num = match builtin_id_for(code) {
            Some(id) => id,
            None => match self.numfmts.iter().find(|(_, c)| c == code) {
                Some((id, _)) => *id,
                None => {
                    let id = 164 + self.numfmts.len() as u32;
                    self.numfmts.push((id, code.to_string()));
                    id
                }
            },
        };
        let align = (st.halign != HAlign::General || st.valign != VAlign::Bottom || st.wrap || st.indent > 0 || st.rotation != 0).then(|| {
            let mut a = String::from("<alignment");
            match st.halign {
                HAlign::Left => a.push_str(r#" horizontal="left""#),
                HAlign::Center => a.push_str(r#" horizontal="center""#),
                HAlign::Right => a.push_str(r#" horizontal="right""#),
                HAlign::Justify => a.push_str(r#" horizontal="justify""#),
                HAlign::General => {}
            }
            if st.indent > 0 {
                let _ = write!(a, r#" indent="{}""#, st.indent);
            }
            match st.rotation {
                0 => {}
                ROTATION_STACKED => a.push_str(r#" textRotation="255""#),
                r if r > 0 => {
                    let _ = write!(a, r#" textRotation="{}""#, r.min(90));
                }
                r => {
                    let _ = write!(a, r#" textRotation="{}""#, 90 + (-r).min(90));
                }
            }
            match st.valign {
                VAlign::Top => a.push_str(r#" vertical="top""#),
                VAlign::Center => a.push_str(r#" vertical="center""#),
                VAlign::Bottom => {}
            }
            if st.wrap {
                a.push_str(r#" wrapText="1""#);
            }
            a.push_str("/>");
            a
        });
        let protect = (st.unlocked || st.formula_hidden).then(|| format!(r#"<protection locked="{}" hidden="{}"/>"#, !st.unlocked as u8, st.formula_hidden as u8));
        XfIds { num, font, fill, border, align, protect, checkbox: st.checkbox }
    }
}

fn write_xf(out: &mut String, x: &XfIds, xf_id: Option<usize>, apply: [bool; 6]) {
    let _ = write!(out, r#"<xf numFmtId="{}" fontId="{}" fillId="{}" borderId="{}""#, x.num, x.font, x.fill, x.border);
    if let Some(id) = xf_id {
        let _ = write!(out, r#" xfId="{id}""#);
    }
    for (on, key) in apply.iter().zip(["applyNumberFormat", "applyFont", "applyFill", "applyBorder", "applyAlignment", "applyProtection"]) {
        match (on, xf_id) {
            (true, Some(_)) => {
                let _ = write!(out, r#" {key}="1""#);
            }
            // Style xfs: the parts the style leaves out.
            (false, None) => {
                let _ = write!(out, r#" {key}="0""#);
            }
            _ => {}
        }
    }
    if x.align.is_none() && x.protect.is_none() && !x.checkbox {
        out.push_str("/>");
        return;
    }
    out.push('>');
    if let Some(a) = &x.align {
        out.push_str(a);
    }
    if let Some(p) = &x.protect {
        out.push_str(p);
    }
    if x.checkbox && xf_id.is_some() {
        out.push_str(&crate::xlsx_incell::xf_checkbox_ext());
    }
    out.push_str("</xf>");
}

pub(crate) fn write_styles(wb: &Workbook, dxfs: &[apex_sheet::data::CfStyle]) -> String {
    let theme = &wb.cell_styles.theme;
    let mut p = Parts { theme, fonts: Vec::new(), fills: vec![r#"<fill><patternFill patternType="none"/></fill>"#.into(), r#"<fill><patternFill patternType="gray125"/></fill>"#.into()], borders: vec!["<border><left/><right/><top/><bottom/><diagonal/></border>".into()], numfmts: Vec::new(), at: Default::default() };
    for (i, s) in p.fills.iter().enumerate().chain(p.borders.iter().enumerate()) {
        p.at.insert(s.clone(), i);
    }
    // Font 0 is the default font.
    let f0 = p.font_xml(&CellStyle::default());
    p.at.insert(f0.clone(), 0);
    p.fonts.push(f0);

    let named = wb.saved_cell_styles();
    let mut style_xfs = String::new();
    let mut style_ids = Vec::new();
    for ns in &named {
        let x = p.ids(&ns.style);
        let parts = if ns.is_normal() { part::ALL } else { ns.parts };
        let apply = [part::NUMBER, part::FONT, part::FILL, part::BORDER, part::ALIGN, part::PROTECT].map(|f| parts & f != 0);
        write_xf(&mut style_xfs, &x, None, apply);
        style_ids.push(x);
    }
    let pos: HashMap<&str, usize> = named.iter().enumerate().map(|(i, s)| (&*s.name, i)).collect();

    let mut xfs = String::new();
    for (_, st) in wb.styles.iter() {
        let x = p.ids(st);
        let si = st.named.as_deref().and_then(|n| pos.get(n).copied()).unwrap_or(0);
        let base = &style_ids[si.min(style_ids.len().saturating_sub(1))];
        // Parts that differ from the style are applied by the cell.
        let apply = [x.num != base.num, x.font != base.font, x.fill != base.fill, x.border != base.border, x.align != base.align, x.protect != base.protect];
        write_xf(&mut xfs, &x, Some(si), apply);
    }

    let mut s = String::from(HEADER);
    s.push_str(r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">"#);
    if !p.numfmts.is_empty() {
        let _ = write!(s, r#"<numFmts count="{}">"#, p.numfmts.len());
        for (id, code) in &p.numfmts {
            let _ = write!(s, r#"<numFmt numFmtId="{}" formatCode="{}"/>"#, id, esc(code));
        }
        s.push_str("</numFmts>");
    }
    let _ = write!(s, r#"<fonts count="{}">{}</fonts>"#, p.fonts.len(), p.fonts.concat());
    let _ = write!(s, r#"<fills count="{}">{}</fills>"#, p.fills.len(), p.fills.concat());
    let _ = write!(s, r#"<borders count="{}">{}</borders>"#, p.borders.len(), p.borders.concat());
    let _ = write!(s, r#"<cellStyleXfs count="{}">{}</cellStyleXfs>"#, named.len(), style_xfs);
    let _ = write!(s, r#"<cellXfs count="{}">{}</cellXfs>"#, wb.styles.len(), xfs);
    let _ = write!(s, r#"<cellStyles count="{}">"#, named.len());
    for (i, ns) in named.iter().enumerate() {
        let _ = write!(s, r#"<cellStyle name="{}" xfId="{i}""#, esc(&ns.name));
        if let Some(id) = ns.builtin {
            let _ = write!(s, r#" builtinId="{id}""#);
            if !ns.is_normal() && wb.cell_styles.is_modified(ns, &wb.locale) {
                s.push_str(r#" customBuiltin="1""#);
            }
        }
        s.push_str("/>");
    }
    s.push_str("</cellStyles>");
    s.push_str(&crate::xlsx_data::write_dxfs(dxfs));
    s.push_str("</styleSheet>");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_sheet::{CellRef, Range};

    #[test]
    fn named_styles_and_theme_roundtrip() {
        let mut wb = Workbook::new();
        wb.set_input(0, CellRef::new(0, 0), "12");
        wb.apply_cell_style(0, &Range::parse("A1").unwrap(), "Good");
        wb.apply_cell_style(0, &Range::parse("A2").unwrap(), "Heading 1");
        wb.apply_cell_style(0, &Range::parse("A3").unwrap(), "40% - Accent2");
        wb.apply_cell_style(0, &Range::parse("A4").unwrap(), "Title");
        let mut st = CellStyle::default();
        st.italic = true;
        st.fill = Some(0x123456);
        wb.set_cell_style(None, wb.style_from_format("Mine", &st));
        wb.apply_cell_style(0, &Range::parse("B1").unwrap(), "Mine");
        // Direct formatting over a style.
        wb.modify_style(0, &Range::parse("B1").unwrap(), |s| s.bold = true);
        let mut good = wb.cell_styles.get("Good").unwrap().clone();
        good.style.fill = Some(0x00FF00);
        wb.set_cell_style(Some("Good"), good);
        wb.set_theme(Theme::builtin(12));

        let data = crate::xlsx::write(&wb);
        let z = crate::zip::ZipReader::new(&data).unwrap();
        let xml = z.read_string("xl/styles.xml").unwrap();
        assert!(xml.contains(r#"<cellStyle name="Good" xfId="1" builtinId="26" customBuiltin="1"/>"#), "{xml}");
        assert!(xml.contains(r#"name="Mine""#));
        assert!(xml.contains(r#"<fgColor theme="5" tint="0.6"/>"#), "{xml}");
        assert!(xml.contains(r#"<scheme val="major"/>"#));
        assert!(z.read_string("xl/theme/theme1.xml").unwrap().contains("Paper"));

        let back = crate::xlsx::read(&data).unwrap();
        assert_eq!(back.cell_styles.theme, Theme::builtin(12));
        let style = |a1: &str| back.styles.get(back.cell(0, CellRef::parse(a1).unwrap()).map_or(0, |c| c.style)).clone();
        let a1 = style("A1");
        assert_eq!((a1.named.as_deref(), a1.fill), (Some("Good"), Some(0x00FF00)));
        assert_eq!(back.cell_styles.get("Good").unwrap().style.fill, Some(0x00FF00));
        let b1 = style("B1");
        assert!(b1.bold && b1.italic && b1.named.as_deref() == Some("Mine"));
        assert_eq!(back.cell_styles.get("Mine").unwrap().parts, part::ALL);
        let a3 = style("A3");
        assert_eq!(a3.theme.fill, Some(ThemeColor::new(5, 6000)));
        assert_eq!(a3.fill, Some(Theme::builtin(12).resolve(ThemeColor::new(5, 6000))));
        let a2 = style("A2");
        assert_eq!(a2.theme.borders[1], Some(ThemeColor::new(4, 0)));
        assert_eq!(style("A4").font.as_deref(), Some("Constantia"));
        // The theme still drives them after loading.
        let mut back = back;
        back.set_theme(Theme::default());
        let a3 = back.styles.get(back.cell(0, CellRef::parse("A3").unwrap()).unwrap().style).clone();
        assert_eq!(a3.fill, Some(Theme::default().resolve(ThemeColor::new(5, 6000))));
    }
}
