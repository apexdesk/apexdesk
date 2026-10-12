//! Page layouts, master pages, headers and footers in OpenDocument files
//! (shared by ods and odt).
//!
//! In ODF the page's top margin reaches the header, and the header's height
//! (spacing included) reaches the body; Office's model measures both from
//! the edge instead: top = page margin + header height.

use std::collections::HashMap;
use std::fmt::Write as _;

use apex_sheet::addr::col_name;
use apex_sheet::{CellRef, HeaderFooter, HfText, Range, Sheet, SheetPage};

use crate::parse_length_pt;
use crate::xml::{Ev, Reader, Tag, esc};

/// A `style:page-layout`.
#[derive(Clone, Debug, Default)]
pub(crate) struct Layout {
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub landscape: Option<bool>,
    /// top, bottom, left, right.
    pub margins: [Option<f32>; 4],
    /// Header and footer heights, spacing included.
    pub header: Option<f32>,
    pub footer: Option<f32>,
    pub scale: Option<u16>,
    pub fit: Option<(u16, u16)>,
    pub across_first: bool,
    pub first_page: Option<u32>,
    pub center_h: bool,
    pub center_v: bool,
    pub gridlines: bool,
    pub headings: bool,
    /// `style:background-image` of the page: LibreOffice's form of a sheet
    /// background (it turns Excel's `<picture>` into one).
    pub background: Option<String>,
}

/// A `style:master-page`.
#[derive(Clone, Debug, Default)]
pub(crate) struct Master {
    pub layout: String,
    pub hf: HeaderFooter,
    /// Whether the header and footer are shown at all.
    pub has_header: bool,
    pub has_footer: bool,
    /// The master page of the pages after the first (text documents).
    pub next: Option<String>,
}

#[derive(Default)]
pub(crate) struct Pages {
    pub layouts: HashMap<String, Layout>,
    pub masters: HashMap<String, Master>,
    /// Master page names in document order.
    pub order: Vec<String>,
}

impl Pages {
    pub fn layout_of(&self, master: &Master) -> Layout {
        self.layouts.get(&master.layout).cloned().unwrap_or_default()
    }
}

fn len(t: &Tag, k: &str) -> Option<f32> {
    t.attr(k).and_then(|v| parse_length_pt(&v)).filter(|v| v.is_finite() && *v >= 0.0 && *v < 10000.0)
}

/// Section of a header part: 0 left, 1 center, 2 right.
pub(crate) type Section = usize;

/// Reads the page layouts and master pages of a styles.xml. `align` gives
/// the section of a paragraph style (its alignment), and `plain` is the
/// section of unaligned paragraphs.
pub(crate) fn read_pages(xml: &str, align: &dyn Fn(&str) -> Option<Section>, plain: Section) -> Pages {
    super::reset_spaces();
    let mut out = Pages::default();
    let mut r = Reader::new(xml);
    let mut layout: Option<(String, Layout)> = None;
    // Header or footer style being read: true for the header.
    let mut hf_style: Option<bool> = None;
    let mut master: Option<(String, Master)> = None;
    loop {
        let ev = r.next();
        let start = matches!(ev, Ev::Start(_));
        match ev {
            Ev::Eof => break,
            Ev::Start(t) | Ev::Empty(t) => match t.local() {
                "page-layout" if t.name == "style:page-layout" => {
                    let l = (t.attr("style:name").unwrap_or_default().into_owned(), Layout::default());
                    if start {
                        layout = Some(l);
                    } else {
                        out.layouts.insert(l.0, l.1);
                    }
                }
                "page-layout-properties" => {
                    if let Some((_, l)) = &mut layout {
                        read_layout_props(&t, l);
                    }
                }
                "background-image" if hf_style.is_none() => {
                    if let (Some((_, l)), Some(href)) = (&mut layout, t.attr("xlink:href")) {
                        l.background = Some(href.into_owned());
                    }
                }
                "header-style" => hf_style = start.then_some(true),
                "footer-style" => hf_style = start.then_some(false),
                "header-footer-properties" => {
                    if let (Some((_, l)), Some(header)) = (&mut layout, hf_style) {
                        let h = len(&t, "fo:min-height").or_else(|| len(&t, "svg:height"));
                        if header { l.header = h } else { l.footer = h }
                    }
                }
                "master-page" => {
                    let mut m = Master { layout: t.attr("style:page-layout-name").unwrap_or_default().into_owned(), ..Default::default() };
                    let name = t.attr("style:name").unwrap_or_default().into_owned();
                    m.next = t.attr("style:next-style-name").map(|n| n.into_owned()).filter(|n| *n != name);
                    out.order.push(name.clone());
                    if start {
                        master = Some((name, m));
                    } else {
                        out.masters.insert(name, m);
                    }
                }
                name @ ("header" | "header-left" | "header-first" | "footer" | "footer-left" | "footer-first") => {
                    let Some((_, m)) = &mut master else {
                        if start {
                            r.skip();
                        }
                        continue;
                    };
                    let shown = t.attr("style:display").is_none_or(|v| v != "false");
                    let text = if start { read_hf(&mut r, align, plain) } else { HfText::default() };
                    let hf = &mut m.hf;
                    match name {
                        "header" => {
                            m.has_header = shown;
                            hf.header = text;
                        }
                        "footer" => {
                            m.has_footer = shown;
                            hf.footer = text;
                        }
                        "header-left" if shown => {
                            hf.odd_even = true;
                            hf.even_header = text;
                        }
                        "footer-left" if shown => {
                            hf.odd_even = true;
                            hf.even_footer = text;
                        }
                        "header-first" if shown => {
                            hf.different_first = true;
                            hf.first_header = text;
                        }
                        "footer-first" if shown => {
                            hf.different_first = true;
                            hf.first_footer = text;
                        }
                        _ => {}
                    }
                }
                _ => {}
            },
            Ev::End(n) => match n {
                "style:page-layout" => {
                    if let Some((name, l)) = layout.take() {
                        out.layouts.insert(name, l);
                    }
                }
                "style:header-style" | "style:footer-style" => hf_style = None,
                "style:master-page" => {
                    if let Some((name, m)) = master.take() {
                        out.masters.insert(name, m);
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    // A hidden header or footer hides its left and first variants too.
    for m in out.masters.values_mut() {
        let hf = &mut m.hf;
        if !m.has_header {
            (hf.header, hf.even_header, hf.first_header) = Default::default();
        }
        if !m.has_footer {
            (hf.footer, hf.even_footer, hf.first_footer) = Default::default();
        }
        if !m.has_header && !m.has_footer {
            (hf.odd_even, hf.different_first) = (false, false);
        }
    }
    out
}

fn read_layout_props(t: &Tag, l: &mut Layout) {
    l.width = len(t, "fo:page-width").or(l.width);
    l.height = len(t, "fo:page-height").or(l.height);
    if let Some(o) = t.attr("style:print-orientation") {
        l.landscape = Some(o == "landscape");
    }
    if let Some(m) = len(t, "fo:margin") {
        l.margins = [Some(m); 4];
    }
    for (i, k) in ["fo:margin-top", "fo:margin-bottom", "fo:margin-left", "fo:margin-right"].into_iter().enumerate() {
        if let Some(m) = len(t, k) {
            l.margins[i] = Some(m);
        }
    }
    if let Some(s) = t.attr("style:scale-to").and_then(|v| v.trim().trim_end_matches('%').parse::<f32>().ok()) {
        l.scale = Some(s.clamp(10.0, 400.0).round() as u16);
    }
    let pages = |k: &str| t.attr(k).and_then(|v| v.trim().parse::<u16>().ok());
    // Local names, so both style: and loext: prefixes are read.
    let (x, y) = (pages("scale-to-X"), pages("scale-to-Y"));
    if x.is_some() || y.is_some() {
        l.fit = Some((x.unwrap_or(0), y.unwrap_or(0)));
    } else if let Some(n) = pages("style:scale-to-pages").filter(|&n| n > 0) {
        // N pages in all: approximated as one page wide.
        l.fit = Some((1, n));
    }
    if let Some(o) = t.attr("style:print-page-order") {
        l.across_first = o == "ltr";
    }
    l.first_page = t.attr("style:first-page-number").and_then(|v| v.trim().parse::<u32>().ok());
    if let Some(c) = t.attr("style:table-centering") {
        l.center_h = matches!(c.as_ref(), "horizontal" | "both");
        l.center_v = matches!(c.as_ref(), "vertical" | "both");
    }
    if let Some(p) = t.attr("style:print") {
        l.gridlines = p.split_whitespace().any(|w| w == "grid");
        l.headings = p.split_whitespace().any(|w| w == "headers");
    }
}

/// Reads the content of a header or footer element up to its end.
fn read_hf(r: &mut Reader, align: &dyn Fn(&str) -> Option<Section>, plain: Section) -> HfText {
    let mut parts: [String; 3] = Default::default();
    loop {
        match r.next() {
            Ev::Eof | Ev::End(_) => break,
            Ev::Start(t) => match t.name {
                "style:region-left" | "style:region-center" | "style:region-right" => {
                    let sec = match t.name {
                        "style:region-left" => 0,
                        "style:region-center" => 1,
                        _ => 2,
                    };
                    let mut paras = Vec::new();
                    loop {
                        match r.next() {
                            Ev::Eof | Ev::End(_) => break,
                            Ev::Start(p) if matches!(p.name, "text:p" | "text:h") => paras.push(read_para(r)),
                            Ev::Start(_) => r.skip(),
                            _ => {}
                        }
                    }
                    parts[sec] = paras.join("\n");
                }
                "text:p" | "text:h" => {
                    let sec = t.attr("text:style-name").and_then(|s| align(&s)).unwrap_or(plain);
                    let text = read_para(r);
                    place(&mut parts, &text, sec);
                }
                "text:list" | "table:table" | "text:section" => {
                    // Rare in headers: keep the text of their paragraphs.
                    let text = r.text();
                    place(&mut parts, &text.replace('&', "&&"), plain);
                }
                _ => r.skip(),
            },
            Ev::Empty(_) | Ev::Text(_) => {}
        }
    }
    // A header's worth of text (each sheet copies it).
    let [left, center, right] = parts.map(|p| if p.len() <= 4096 { p } else { p.chars().take(4096).collect() });
    HfText { left, center, right }
}

/// Adds a paragraph to the header parts: tab separated text fills left,
/// center and right in turn (as in Word's and Writer's default header);
/// otherwise its alignment decides.
pub(crate) fn place(parts: &mut [String; 3], text: &str, sec: Section) {
    let add = |p: &mut String, t: &str| {
        if t.is_empty() {
            return;
        }
        if !p.is_empty() {
            p.push('\n');
        }
        p.push_str(t);
    };
    let segs: Vec<&str> = text.split('\t').collect();
    match segs.as_slice() {
        [one] => add(&mut parts[sec.min(2)], one),
        [l, c] => {
            add(&mut parts[0], l);
            add(&mut parts[1], c);
        }
        [l, c, rest @ ..] => {
            add(&mut parts[0], l);
            add(&mut parts[1], c);
            add(&mut parts[2], &rest.concat());
        }
        [] => {}
    }
}

/// The field code of an ODF text field.
fn field_code(name: &str) -> Option<&'static str> {
    Some(match name {
        "text:page-number" => "&P",
        "text:page-count" => "&N",
        "text:date" => "&D",
        "text:time" => "&T",
        // The title often is the file name; Office has no title field.
        "text:file-name" | "text:title" => "&F",
        "text:sheet-name" => "&A",
        _ => return None,
    })
}

/// Text of a paragraph whose start was just read, with fields as codes.
fn read_para(r: &mut Reader) -> String {
    let mut s = String::new();
    let mut depth = 1;
    loop {
        match r.next() {
            Ev::Eof => break,
            Ev::Start(t) => {
                if let Some(code) = field_code(t.name) {
                    s.push_str(code);
                    r.skip();
                } else if matches!(t.name, "draw:frame" | "draw:custom-shape" | "office:annotation" | "text:note" | "text:bookmark-ref") {
                    r.skip();
                } else {
                    depth += 1;
                }
            }
            Ev::Empty(t) => match t.name {
                "text:s" => {
                    let n: usize = t.attr("text:c").and_then(|v| v.parse().ok()).unwrap_or(1);
                    s.extend(std::iter::repeat_n(' ', super::spaces(n)));
                }
                "text:tab" => s.push('\t'),
                "text:line-break" => s.push('\n'),
                n => {
                    if let Some(code) = field_code(n) {
                        s.push_str(code);
                    }
                }
            },
            Ev::Text(x) => s.push_str(&x.replace('&', "&&")),
            Ev::End(_) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
        }
    }
    s
}

/// Applies a master page and its layout to a sheet's page setup.
pub(crate) fn apply_to_sheet(pages: &Pages, master: &str, p: &mut SheetPage) {
    let Some(m) = pages.masters.get(master) else { return };
    let l = pages.layout_of(m);
    if let (Some(w), Some(h)) = (l.width, l.height) {
        (p.paper_width, p.paper_height) = (w.min(h), w.max(h));
        p.landscape = l.landscape.unwrap_or(w > h);
    }
    let [top, bottom, left, right] = l.margins;
    p.margin_left = left.unwrap_or(p.margin_left);
    p.margin_right = right.unwrap_or(p.margin_right);
    (p.margin_top, p.margin_header) = edge_margins(top, l.header, m.has_header, p.margin_top, p.margin_header);
    (p.margin_bottom, p.margin_footer) = edge_margins(bottom, l.footer, m.has_footer, p.margin_bottom, p.margin_footer);
    if let Some(s) = l.scale {
        p.scale = s;
    }
    p.fit = l.fit;
    p.across_first = l.across_first;
    p.first_page = l.first_page;
    (p.center_h, p.center_v, p.gridlines, p.headings) = (l.center_h, l.center_v, l.gridlines, l.headings);
    p.hf = m.hf.clone();
}

/// Path of the background picture of a master page's layout.
pub(crate) fn background_of(pages: &Pages, master: &str) -> Option<String> {
    let m = pages.masters.get(master)?;
    pages.layouts.get(&m.layout)?.background.clone()
}

/// Office-style body and header margins from an ODF page margin and header
/// height.
pub(crate) fn edge_margins(page: Option<f32>, height: Option<f32>, shown: bool, body: f32, header: f32) -> (f32, f32) {
    let Some(page) = page else { return (body, header) };
    match height {
        Some(h) if shown => (page + h, page),
        // Without a header its height, when written, still tells the header
        // distance.
        Some(h) if h < page => (page, page - h),
        _ => (page, header.min(page)),
    }
}

// ---------------------------------------------------------------------------
// Writing

/// Writes a header or footer part as paragraphs with ODF fields.
pub(crate) fn write_part(out: &mut String, text: &str, para_style: &str) {
    for line in text.split('\n') {
        let _ = write!(out, "<text:p{para_style}>");
        write_fields(out, line);
        out.push_str("</text:p>");
    }
}

/// Writes text with header codes as ODF fields (no line breaks).
pub(crate) fn write_fields(out: &mut String, line: &str) {
    let mut spaces = 0usize;
    let mut first = true;
    let flush = |out: &mut String, n: &mut usize, first: bool| {
        if *n == 0 {
            return;
        }
        if first {
            let _ = write!(out, r#"<text:s text:c="{n}"/>"#);
        } else {
            out.push(' ');
            if *n > 1 {
                let _ = write!(out, r#"<text:s text:c="{}"/>"#, *n - 1);
            }
        }
        *n = 0;
    };
    let mut it = line.chars().peekable();
    while let Some(c) = it.next() {
        if c == ' ' {
            spaces += 1;
            continue;
        }
        flush(out, &mut spaces, first);
        first = false;
        match c {
            '\t' => out.push_str("<text:tab/>"),
            '&' => match it.next() {
                Some('P' | 'p') => out.push_str("<text:page-number>1</text:page-number>"),
                Some('N' | 'n') => out.push_str("<text:page-count>1</text:page-count>"),
                Some('D' | 'd') => out.push_str("<text:date/>"),
                Some('T' | 't') => out.push_str("<text:time/>"),
                Some('F' | 'f') => out.push_str(r#"<text:file-name text:display="name-and-extension"/>"#),
                Some('A' | 'a') => out.push_str("<text:sheet-name/>"),
                Some('&') => out.push_str("&amp;"),
                Some(o) => {
                    out.push_str("&amp;");
                    let mut b = [0u8; 4];
                    out.push_str(&esc(o.encode_utf8(&mut b)));
                }
                None => out.push_str("&amp;"),
            },
            c => {
                let mut b = [0u8; 4];
                out.push_str(&esc(c.encode_utf8(&mut b)));
            }
        }
    }
    if spaces > 0 {
        let _ = write!(out, r#"<text:s text:c="{spaces}"/>"#);
    }
}

/// Header and footer extent properties for a page layout: `top` and
/// `bottom` are the page's (Office-style) body margins.
pub(crate) fn hf_styles(out: &mut String, top: f32, header: f32, bottom: f32, footer: f32) {
    for (tag, body, edge, space) in [("header-style", top, header, "fo:margin-bottom"), ("footer-style", bottom, footer, "fo:margin-top")] {
        let h = (body - edge).max(1.0);
        // As LibreOffice does: the spacing is the height less 0.1 cm.
        let gap = (h - 2.83).max(0.0);
        let _ = write!(out, r#"<style:{tag}><style:header-footer-properties fo:min-height="{}" fo:margin-left="0cm" fo:margin-right="0cm" {space}="{}" style:dynamic-spacing="true"/></style:{tag}>"#, pt(h), pt(gap));
    }
}

pub(crate) fn pt(v: f32) -> String {
    format!("{:.3}pt", v)
}

/// Page margins as written in ODF: up to the header when it is shown.
pub(crate) fn page_margins(top: f32, header: f32, bottom: f32, footer: f32, has_header: bool, has_footer: bool) -> (f32, f32) {
    (if has_header { header.min(top) } else { top }, if has_footer { footer.min(bottom) } else { bottom })
}

/// Whether the header (or footer) is shown on any page.
pub(crate) fn shown(hf: &HeaderFooter, header: bool) -> bool {
    if header {
        !hf.header.is_empty() || (hf.odd_even && !hf.even_header.is_empty()) || (hf.different_first && !hf.first_header.is_empty())
    } else {
        !hf.footer.is_empty() || (hf.odd_even && !hf.even_footer.is_empty()) || (hf.different_first && !hf.first_footer.is_empty())
    }
}

/// Writes the header and footer elements of a master page; `content`
/// writes one part, and every header starts with `header_extra` (a
/// watermark, which shows the header even when it is empty).
pub(crate) fn write_master_hf(out: &mut String, hf: &HeaderFooter, header_extra: &str, content: &dyn Fn(&mut String, &HfText)) {
    for header in [true, false] {
        let (main, even, first, tag) = if header { (&hf.header, &hf.even_header, &hf.first_header, "header") } else { (&hf.footer, &hf.even_footer, &hf.first_footer, "footer") };
        let extra = if header { header_extra } else { "" };
        if !shown(hf, header) {
            if extra.is_empty() {
                let _ = write!(out, r#"<style:{tag} style:display="false"/>"#);
            } else {
                let _ = write!(out, "<style:{tag}>{extra}</style:{tag}>");
            }
            continue;
        }
        for (suffix, t, on) in [("", main, true), ("-left", even, hf.odd_even), ("-first", first, hf.different_first)] {
            if !on {
                let _ = write!(out, r#"<style:{tag}{suffix} style:display="false"/>"#);
                continue;
            }
            let _ = write!(out, "<style:{tag}{suffix}>");
            out.push_str(extra);
            content(out, t);
            let _ = write!(out, "</style:{tag}{suffix}>");
        }
    }
}

/// Writes a spreadsheet header part as regions.
pub(crate) fn write_regions(out: &mut String, t: &HfText) {
    for (tag, part) in [("left", &t.left), ("center", &t.center), ("right", &t.right)] {
        if !part.is_empty() {
            let _ = write!(out, "<style:region-{tag}>");
            write_part(out, part, "");
            let _ = write!(out, "</style:region-{tag}>");
        }
    }
}

/// Splits at `sep` outside single quotes.
fn split_unquoted(s: &str, sep: impl Fn(char) -> bool) -> Vec<&str> {
    let mut out = Vec::new();
    let mut quoted = false;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        if c == '\'' {
            quoted = !quoted;
        } else if !quoted && sep(c) {
            out.push(&s[start..i]);
            start = i + c.len_utf8();
        }
    }
    out.push(&s[start..]);
    out
}

/// The areas of a `table:print-ranges` list like
/// `'My sheet'.A1:'My sheet'.D20 'My sheet'.F1:'My sheet'.G9`.
pub(crate) fn parse_print_ranges(s: &str) -> Vec<Range> {
    split_unquoted(s.trim(), char::is_whitespace).into_iter().filter(|x| !x.is_empty()).filter_map(parse_print_range).collect()
}

/// One area of a `table:print-ranges` list.
fn parse_print_range(one: &str) -> Option<Range> {
    let sides = split_unquoted(one, |c| c == ':');
    let cell = |x: &str| {
        let parts = split_unquoted(x, |c| c == '.');
        CellRef::parse(&parts.last()?.replace('$', ""))
    };
    let a = cell(sides.first()?)?;
    let b = sides.get(1).map_or(Some(a), |x| cell(x))?;
    Some(Range::new(a, b))
}

/// A `table:print-ranges` value for an area of sheet `name`.
pub(crate) fn print_range(name: &str, a: &Range) -> String {
    let q = format!("'{}'", name.replace('\'', "''"));
    format!("{q}.${}${}:{q}.${}${}", col_name(a.start.col), a.start.row + 1, col_name(a.end.col), a.end.row + 1)
}

/// Page layout and master page of a sheet, for styles.xml; `i` numbers
/// them.
pub(crate) fn write_sheet_page(auto: &mut String, masters: &mut String, i: usize, sh: &Sheet, background: Option<&str>) {
    let p = &sh.page;
    let (w, h) = p.paper();
    let (hh, fh) = (shown(&p.hf, true), shown(&p.hf, false));
    let (top, bottom) = page_margins(p.margin_top, p.margin_header, p.margin_bottom, p.margin_footer, hh, fh);
    let _ = write!(
        auto,
        r#"<style:page-layout style:name="pm{i}"><style:page-layout-properties fo:page-width="{}" fo:page-height="{}" style:print-orientation="{}" fo:margin-top="{}" fo:margin-bottom="{}" fo:margin-left="{}" fo:margin-right="{}" style:print-page-order="{}""#,
        pt(w),
        pt(h),
        if p.landscape { "landscape" } else { "portrait" },
        pt(top),
        pt(bottom),
        pt(p.margin_left),
        pt(p.margin_right),
        if p.across_first { "ltr" } else { "ttb" }
    );
    match p.first_page {
        Some(n) => {
            let _ = write!(auto, r#" style:first-page-number="{n}""#);
        }
        None => auto.push_str(r#" style:first-page-number="continue""#),
    }
    match p.fit {
        Some((fw, fh)) => {
            if fw > 0 {
                let _ = write!(auto, r#" style:scale-to-X="{fw}""#);
            }
            if fh > 0 {
                let _ = write!(auto, r#" style:scale-to-Y="{fh}""#);
            }
        }
        None => {
            let _ = write!(auto, r#" style:scale-to="{}%""#, p.scale.clamp(10, 400));
        }
    }
    let centering = match (p.center_h, p.center_v) {
        (true, true) => "both",
        (true, false) => "horizontal",
        (false, true) => "vertical",
        _ => "none",
    };
    let _ = write!(auto, r#" style:table-centering="{centering}" style:print="{}{}charts drawings objects zero-values""#, if p.gridlines { "grid " } else { "" }, if p.headings { "headers " } else { "" });
    match background {
        Some(href) => {
            let _ = write!(auto, r#"><style:background-image xlink:href="{}" xlink:type="simple" xlink:actuate="onLoad"/></style:page-layout-properties>"#, esc(href));
        }
        None => auto.push_str("/>"),
    }
    hf_styles(auto, p.margin_top, p.margin_header, p.margin_bottom, p.margin_footer);
    auto.push_str("</style:page-layout>");
    let _ = write!(masters, r#"<style:master-page style:name="PageStyle_{i}" style:display-name="PageStyle_{}" style:page-layout-name="pm{i}">"#, esc(&sh.name));
    write_master_hf(masters, &p.hf, "", &write_regions);
    masters.push_str("</style:master-page>");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hf_content() {
        let xml = r#"<office:document-styles><office:automatic-styles><style:page-layout style:name="pm1"><style:page-layout-properties fo:page-width="29.7cm" fo:page-height="21cm" fo:margin-top="1cm" style:scale-to-X="2" style:print="grid"/><style:header-style><style:header-footer-properties fo:min-height="1.5cm"/></style:header-style></style:page-layout></office:automatic-styles><office:master-styles><style:master-page style:name="M" style:page-layout-name="pm1"><style:header><style:region-left><text:p>A &amp; B<text:s text:c="2"/><text:page-number>1</text:page-number></text:p><text:p>two</text:p></style:region-left><style:region-right><text:p><text:span><text:sheet-name>???</text:sheet-name></text:span></text:p></style:region-right></style:header><style:header-left style:display="false"/><style:header-first/><style:footer><text:p>Left<text:tab/>Mid <text:date>x</text:date></text:p><text:p text:style-name="R">Right</text:p></style:footer></style:master-page></office:master-styles></office:document-styles>"#;
        let pages = read_pages(xml, &|s| (s == "R").then_some(2), 1);
        let m = &pages.masters["M"];
        assert_eq!(m.hf.header, HfText { left: "A && B  &P\ntwo".into(), center: "".into(), right: "&A".into() });
        assert_eq!(m.hf.footer, HfText { left: "Left".into(), center: "Mid &D".into(), right: "Right".into() });
        assert!(m.hf.different_first && !m.hf.odd_even && m.hf.first_header.is_empty());
        let mut p = SheetPage::default();
        apply_to_sheet(&pages, "M", &mut p);
        assert!(p.landscape && p.gridlines && p.fit == Some((2, 0)));
        assert!((p.margin_top - 2.5 / 2.54 * 72.0).abs() < 0.1 && (p.margin_header - 72.0 / 2.54).abs() < 0.1);
        let mut out = String::new();
        write_fields(&mut out, " a  &P&&&x");
        assert_eq!(out, r#"<text:s text:c="1"/>a <text:s text:c="1"/><text:page-number>1</text:page-number>&amp;&amp;x"#);
    }
}
