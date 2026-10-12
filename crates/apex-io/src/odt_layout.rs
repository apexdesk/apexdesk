//! Page layout in ODT: sections (`text:section` with columns, and a master
//! page per page setup, switched to by the paragraph that starts the
//! section), the page background and borders, text and picture watermarks
//! (shapes in the master page's header, as LibreOffice writes them);
//! paragraph borders, tab stops and drop caps.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{Columns, Document, DropCap, Line, LineStyle, NoteFormat, PageBorders, PageDeco, PageSetup, ParaBorders, ParaStyle, Section, SectionStart, TabAlign, TabLeader, TabStop, TabStops, Watermark};

use crate::ods::page::{Pages, edge_margins};
use crate::xml::{Ev, Reader, Tag, esc};
use crate::{parse_hex_color, parse_length_pt};

// --- reading: styles --------------------------------------------------------------

/// Layout properties of a paragraph style.
#[derive(Clone, Default)]
pub(crate) struct ParaExtra {
    pub column_break: bool,
    /// top, bottom, left, right (without their distance to the text).
    pub borders: [Option<Line>; 4],
    pub padding: [Option<f32>; 4],
    /// Tab stops relative to the paragraph's left indent.
    pub tabs: Option<Vec<TabStop>>,
    pub drop_cap: Option<DropCap>,
    /// The master page this paragraph switches to.
    pub master: Option<String>,
    /// The page number the new page starts with.
    pub page_number: Option<u32>,
    /// Asian line breaking and spacing.
    pub asian: apex_doc::AsianPara,
}

const SIDES: [&str; 4] = ["top", "bottom", "left", "right"];

/// Reads a border value such as `0.5pt solid #000000`.
fn parse_border(v: &str) -> Option<Line> {
    let mut l = Line { width: 0.75, space: 0.0, ..Default::default() };
    let mut seen = false;
    for w in v.split_whitespace() {
        match w {
            "none" | "hidden" => return None,
            "solid" | "groove" | "ridge" | "inset" | "outset" => seen = true,
            "double" | "double-thin" => (l.style, seen) = (LineStyle::Double, true),
            "dotted" => (l.style, seen) = (LineStyle::Dotted, true),
            "dashed" | "fine-dashed" | "dash" => (l.style, seen) = (LineStyle::Dashed, true),
            "dot-dash" | "dash-dot" | "dot-dot-dash" | "dash-dot-dot" => (l.style, seen) = (LineStyle::DashDot, true),
            "thin" => l.width = 0.5,
            "medium" => l.width = 1.5,
            "thick" => l.width = 3.0,
            _ => {
                if let Some(c) = parse_hex_color(w) {
                    l.color = c;
                } else if let Some(x) = parse_length_pt(w) {
                    l.width = x;
                }
            }
        }
    }
    if !seen || !(l.width > 0.0) {
        return None;
    }
    if l.style == LineStyle::Double {
        l.width /= 3.0;
    }
    l.width = (l.width * 100.0).round() / 100.0;
    Some(l)
}

/// Reads `fo:border*` and `fo:padding*` of paragraph or page layout
/// properties.
fn read_borders(t: &Tag, borders: &mut [Option<Line>; 4], padding: &mut [Option<f32>; 4]) {
    if let Some(v) = t.attr("fo:border") {
        *borders = [parse_border(&v); 4];
    }
    if let Some(v) = t.attr("fo:padding").and_then(|v| parse_length_pt(&v)) {
        *padding = [Some(v); 4];
    }
    for (i, side) in SIDES.iter().enumerate() {
        if let Some(v) = t.attr(&format!("fo:border-{side}")) {
            borders[i] = parse_border(&v);
        }
        if let Some(v) = t.attr(&format!("fo:padding-{side}")).and_then(|v| parse_length_pt(&v)) {
            padding[i] = Some(v);
        }
    }
}

/// Attributes of a paragraph `style:style`.
pub(crate) fn style_attrs(t: &Tag, e: &mut ParaExtra) {
    e.master = t.attr("style:master-page-name").filter(|m| !m.is_empty()).map(|m| m.into_owned());
}

/// Reads `style:paragraph-properties`.
pub(crate) fn para_props(t: &Tag, e: &mut ParaExtra) {
    if t.attr("fo:break-before").as_deref() == Some("column") {
        e.column_break = true;
    }
    read_borders(t, &mut e.borders, &mut e.padding);
    crate::odt_asian::read_para(t, &mut e.asian);
    if let Some(n) = t.attr("style:page-number").and_then(|v| v.trim().parse::<u32>().ok()) {
        e.page_number = Some(n);
    }
}

/// Reads the children of `style:paragraph-properties`.
pub(crate) fn para_child(t: &Tag, e: &mut ParaExtra) {
    match t.name {
        "style:tab-stops" => e.tabs = Some(Vec::new()),
        "style:tab-stop" => {
            let Some(pos) = t.attr("style:position").and_then(|v| parse_length_pt(&v)) else { return };
            let align = match t.attr("style:type").as_deref() {
                Some("center") => TabAlign::Center,
                Some("right") => TabAlign::Right,
                Some("char") => TabAlign::Decimal,
                _ => TabAlign::Left,
            };
            let text = t.attr("style:leader-text").unwrap_or_default();
            let style = t.attr("style:leader-style").unwrap_or_default();
            let leader = match (text.chars().next(), style.as_ref()) {
                (_, "none") => TabLeader::None,
                (Some('.' | '…' | '·'), _) => TabLeader::Dots,
                (Some('-' | '–'), _) => TabLeader::Dashes,
                (Some('_'), _) => TabLeader::Underline,
                (Some(' '), _) => TabLeader::None,
                (_, "dotted") => TabLeader::Dots,
                (_, "dash" | "long-dash" | "dot-dash" | "dot-dot-dash") => TabLeader::Dashes,
                (_, "solid") => TabLeader::Underline,
                _ => TabLeader::None,
            };
            e.tabs.get_or_insert_with(Vec::new).push(TabStop { pos, align, leader });
        }
        "style:drop-cap" => {
            let lines: u8 = t.attr("style:lines").and_then(|v| v.parse().ok()).unwrap_or(1);
            if lines > 1 {
                let chars = t.attr("style:length").and_then(|v| v.parse().ok()).unwrap_or(1);
                let distance = t.attr("style:distance").and_then(|v| parse_length_pt(&v)).unwrap_or(0.0);
                e.drop_cap = Some(DropCap { lines, margin: false, chars, distance: (distance * 10.0).round() / 10.0 }.sane());
            }
        }
        _ => {}
    }
}

/// Puts the layout properties read for a paragraph into its style;
/// `indent` is its left indent as written (tab stops are relative to it).
pub(crate) fn apply_para(st: &mut ParaStyle, e: &ParaExtra, indent: f32) {
    st.column_break_before = e.column_break && !st.page_break_before;
    let mut b = ParaBorders::default();
    for (i, slot) in [&mut b.top, &mut b.bottom, &mut b.left, &mut b.right].into_iter().enumerate() {
        *slot = e.borders[i].map(|l| Line { space: e.padding[i].unwrap_or(0.0).max(0.0), ..l });
    }
    st.borders = b;
    if let Some(tabs) = &e.tabs {
        let v: Vec<TabStop> = tabs.iter().map(|t| TabStop { pos: ((t.pos + indent) * 10.0).round() / 10.0, ..*t }).filter(|t| t.pos >= 0.0).collect();
        st.tabs = TabStops::from_slice(&v);
    }
    st.drop_cap = e.drop_cap;
    st.asian = e.asian;
}

/// Columns of a section or page layout as written.
#[derive(Clone, Default, Debug)]
pub(crate) struct ColSpec {
    count: u8,
    gap: f32,
    separator: bool,
    /// Relative width, start and end indent of each column.
    cols: Vec<(f32, f32, f32)>,
}

impl ColSpec {
    /// The columns for a text area `width` wide.
    fn columns(&self, width: f32) -> Columns {
        let n = (self.count as usize).clamp(1, apex_doc::MAX_COLUMNS);
        if n == 1 {
            return Columns::default();
        }
        let base = Columns { count: n as u8, spacing: self.gap.max(0.0), separator: self.separator, custom: Vec::new() };
        if self.cols.len() != n {
            return base;
        }
        let total: f32 = self.cols.iter().map(|c| c.0.max(0.0)).sum();
        if !(total > 0.0) {
            return base;
        }
        let parts: Vec<(f32, f32)> = (0..n)
            .map(|i| {
                let (rel, start, end) = self.cols[i];
                let w = (rel.max(0.0) / total * width - start - end).max(1.0);
                let gap = if i + 1 < n { end + self.cols[i + 1].1 } else { 0.0 };
                (w, gap.max(0.0))
            })
            .collect();
        let same = parts.iter().all(|p| (p.0 - parts[0].0).abs() < 0.5) && parts[..n - 1].iter().all(|p| (p.1 - parts[0].1).abs() < 0.5);
        if same {
            return Columns { spacing: (parts[0].1 * 10.0).round() / 10.0, ..base };
        }
        let r = |v: f32| (v * 10.0).round() / 10.0;
        Columns { custom: parts.into_iter().map(|(w, g)| (r(w), r(g))).collect(), ..base }
    }
}

/// Layout properties of a page layout.
#[derive(Clone, Default)]
pub(crate) struct LayoutExtra {
    cols: ColSpec,
    borders: [Option<Line>; 4],
    padding: [Option<f32>; 4],
    color: Option<u32>,
    /// A gradient fill's gradient and opacity names.
    gradient: Option<(String, Option<String>)>,
    start: SectionStart,
    num_format: NoteFormat,
    /// `style:page-usage="mirrored"` and `loext:margin-gutter`.
    mirror: bool,
    gutter: f32,
    /// The layout grid and writing mode (a page setup holding only them).
    asian: PageSetup,
}

/// The page number format of a `style:num-format`.
fn num_format(v: &str) -> NoteFormat {
    match v {
        "i" => NoteFormat::LowerRoman,
        "I" => NoteFormat::UpperRoman,
        "a" => NoteFormat::LowerLetter,
        "A" => NoteFormat::UpperLetter,
        _ => NoteFormat::Arabic,
    }
}

/// The `style:num-format` of a page number format.
pub(crate) fn num_format_attr(f: NoteFormat) -> &'static str {
    match f {
        NoteFormat::LowerRoman => "i",
        NoteFormat::UpperRoman => "I",
        NoteFormat::LowerLetter => "a",
        NoteFormat::UpperLetter => "A",
        _ => "1",
    }
}

enum Cur {
    None,
    Section(String, ColSpec),
    Layout(String, LayoutExtra),
}

/// Section styles and page layouts of a document.
pub(crate) struct Styles {
    sections: HashMap<String, ColSpec>,
    layouts: HashMap<String, LayoutExtra>,
    cur: Cur,
}

impl Default for Styles {
    fn default() -> Self {
        Styles { sections: HashMap::new(), layouts: HashMap::new(), cur: Cur::None }
    }
}

impl Styles {
    /// A `style:style`; true when it is a section style (read here).
    pub(crate) fn start_style(&mut self, t: &Tag, is_start: bool) -> bool {
        if t.attr("style:family").as_deref() != Some("section") {
            return false;
        }
        let name = t.attr("style:name").unwrap_or_default().into_owned();
        if is_start {
            self.cur = Cur::Section(name, ColSpec::default());
        } else {
            self.sections.insert(name, ColSpec::default());
        }
        true
    }

    pub(crate) fn start_layout(&mut self, t: &Tag, is_start: bool) {
        let start = match t.attr("style:page-usage").as_deref() {
            Some("left") => SectionStart::EvenPage,
            Some("right") => SectionStart::OddPage,
            _ => SectionStart::NextPage,
        };
        let name = t.attr("style:name").unwrap_or_default().into_owned();
        let mirror = t.attr("style:page-usage").as_deref() == Some("mirrored");
        let l = LayoutExtra { start, mirror, ..Default::default() };
        if is_start {
            self.cur = Cur::Layout(name, l);
        } else {
            self.layouts.insert(name, l);
        }
    }

    pub(crate) fn layout_props(&mut self, t: &Tag) {
        if let Cur::Layout(_, l) = &mut self.cur {
            read_borders(t, &mut l.borders, &mut l.padding);
            let fill = t.attr("draw:fill");
            let c = match fill.as_deref() {
                Some("solid") => t.attr("draw:fill-color").and_then(|c| parse_hex_color(&c)),
                Some(_) => None,
                None => t.attr("fo:background-color").and_then(|c| parse_hex_color(&c)),
            };
            l.color = c.filter(|&c| c != 0xFFFFFF);
            l.gradient = match (fill.as_deref(), t.attr("draw:fill-gradient-name")) {
                (Some("gradient"), Some(g)) => Some((g.into_owned(), t.attr("draw:opacity-name").map(|o| o.into_owned()))),
                _ => None,
            };
            if let Some(f) = t.attr("style:num-format") {
                l.num_format = num_format(&f);
            }
            l.gutter = t.attr("loext:margin-gutter").and_then(|v| crate::parse_length_pt(&v)).unwrap_or(l.gutter);
            crate::odt_asian::read_page(t, &mut l.asian);
        }
    }

    /// `style:columns` and its children.
    pub(crate) fn columns(&mut self, t: &Tag) {
        let spec = match &mut self.cur {
            Cur::Section(_, s) => s,
            Cur::Layout(_, l) => &mut l.cols,
            Cur::None => return,
        };
        let len = |k: &str| t.attr(k).and_then(|v| parse_length_pt(&v)).unwrap_or(0.0);
        match t.name {
            "style:columns" => {
                spec.count = t.attr("fo:column-count").and_then(|v| v.parse().ok()).unwrap_or(1);
                spec.gap = len("fo:column-gap");
            }
            "style:column-sep" => spec.separator = t.attr("style:style").is_none_or(|s| s != "none") && t.attr("style:width").and_then(|v| parse_length_pt(&v)).is_none_or(|w| w > 0.0),
            "style:column" => {
                let rel = t.attr("style:rel-width").and_then(|v| v.trim_end_matches('*').trim().parse::<f32>().ok()).unwrap_or(1.0);
                spec.cols.push((rel, len("fo:start-indent"), len("fo:end-indent")));
            }
            _ => {}
        }
    }

    pub(crate) fn end(&mut self) {
        match std::mem::replace(&mut self.cur, Cur::None) {
            Cur::Section(n, s) => {
                self.sections.insert(n, s);
            }
            Cur::Layout(n, l) => {
                self.layouts.insert(n, l);
            }
            Cur::None => {}
        }
    }

    /// Whether a section style has columns.
    pub(crate) fn has_columns(&self, name: &str) -> bool {
        self.sections.get(name).is_some_and(|s| s.count > 1)
    }
}

// --- reading: body ----------------------------------------------------------------

/// A body paragraph's page structure as read.
pub(crate) struct ParaMark {
    pub para: usize,
    /// The innermost section with columns around it.
    pub section: Option<String>,
    /// Text sections opened (outside tables) up to it: the first
    /// paragraph of a section counts it.
    pub sects_opened: u32,
    pub master: Option<String>,
    pub page_number: Option<u32>,
}

/// The page setup of a master page, its borders' room added to the margins.
fn page_of(pages: &Pages, styles: &Styles, name: &str) -> Option<PageSetup> {
    let m = pages.masters.get(name)?;
    let mut l = pages.layout_of(m);
    if let Some(ex) = styles.layouts.get(&m.layout) {
        for i in 0..4 {
            if let Some(b) = ex.borders[i] {
                l.margins[i] = Some(l.margins[i].unwrap_or(0.0) + b.thickness() + ex.padding[i].unwrap_or(0.0));
            }
        }
    }
    let mut p = PageSetup::default();
    if let Some(ex) = styles.layouts.get(&m.layout) {
        (p.mirror, p.gutter) = (ex.mirror, ex.gutter);
        (p.grid, p.text_flow) = (ex.asian.grid, ex.asian.text_flow);
    }
    p.width = l.width.unwrap_or(p.width);
    p.height = l.height.unwrap_or(p.height);
    p.margin_left = l.margins[2].unwrap_or(p.margin_left);
    p.margin_right = l.margins[3].unwrap_or(p.margin_right);
    (p.margin_top, p.margin_header) = edge_margins(l.margins[0], l.header, m.has_header, p.margin_top, p.margin_header);
    (p.margin_bottom, p.margin_footer) = edge_margins(l.margins[1], l.footer, m.has_footer, p.margin_bottom, p.margin_footer);
    Some(p)
}

/// The master page of the first page.
pub(crate) fn base_master<'a>(marks: &'a [ParaMark], pages: &'a Pages) -> Option<&'a str> {
    let first = marks.first().filter(|m| m.para == 0).and_then(|m| m.master.as_deref()).filter(|m| pages.masters.contains_key(*m));
    first.or_else(|| pages.masters.contains_key("Standard").then_some("Standard")).or_else(|| pages.order.first().map(String::as_str))
}

/// Sets the document's sections, page setup and page decoration from the
/// master pages and text sections read.
pub(crate) fn apply(doc: &mut Document, marks: &[ParaMark], styles: &Styles, pages: &Pages, hf: &HashMap<String, apex_doc::Headers>, shapes: &crate::odf_shape::Styles) {
    let Some(base) = base_master(marks, pages) else { return };
    let mut page = page_of(pages, styles, base).unwrap_or(doc.page);
    doc.page = page;
    let extra = |master: &str| pages.masters.get(master).and_then(|m| styles.layouts.get(&m.layout));
    if let Some(m) = pages.masters.get(base) {
        doc.headers = hf.get(base).cloned().unwrap_or_default();
        doc.headers.numbering = apex_doc::PageNumbering { format: extra(base).map_or(NoteFormat::Arabic, |e| e.num_format), start: marks.first().filter(|m| m.para == 0).and_then(|m| m.page_number) };
        let l = pages.layout_of(m);
        if let Some(ex) = extra(base) {
            doc.deco.color = ex.color;
            doc.deco.gradient = ex.gradient.as_ref().and_then(|(g, o)| {
                let g = shapes.gradients.get(g)?.gradient(o.as_ref().and_then(|o| shapes.opacities.get(o)), None);
                Some(g)
            });
            if let Some(g) = &doc.deco.gradient {
                doc.deco.color = Some(g.main_color());
            }
            let raw = |i: usize| l.margins[i].unwrap_or(0.0);
            let pad = |i: usize| ex.padding[i].unwrap_or(0.0);
            let sides: Vec<usize> = (0..4).filter(|&i| ex.borders[i].is_some()).collect();
            // Borders nearer the page edge than the text are measured from it.
            let from_edge = sides.iter().filter(|&&i| raw(i) < pad(i)).count() * 2 > sides.len();
            let side = |i: usize| ex.borders[i].map(|b| Line { space: ((if from_edge { raw(i) } else { pad(i) }) * 10.0).round() / 10.0, ..b });
            doc.deco.borders = PageBorders { top: side(0), bottom: side(1), left: side(2), right: side(3), from_edge };
        }
    }
    let cols = |mk: &ParaMark, master: &str, page: &PageSetup| {
        let spec = mk.section.as_ref().and_then(|s| styles.sections.get(s)).or_else(|| extra(master).map(|e| &e.cols));
        spec.map(|s| s.columns(page.content_width())).unwrap_or_default()
    };
    // A new master page has its own headers (none are linked).
    let headers_of = |m: &str, start: Option<u32>| {
        let empty = Arc::new(vec![apex_doc::Paragraph::new("")]);
        let mut headers = hf.get(m).cloned().unwrap_or_default();
        for slot in apex_doc::HfSlot::ALL {
            if headers.get(slot).is_none() {
                headers.set(slot, Some(empty.clone()));
            }
        }
        headers.numbering = apex_doc::PageNumbering { format: extra(m).map_or(NoteFormat::Arabic, |e| e.num_format), start };
        headers
    };
    let mut master = base;
    let mut cur = Columns::default();
    // Text sections opened when `master` began.
    let mut opened = marks.first().map_or(0, |m| m.sects_opened);
    for mk in marks {
        if doc.paragraphs.get(mk.para).is_none_or(|p| p.cell.is_some()) {
            continue;
        }
        if mk.para == 0 {
            cur = cols(mk, master, &page);
            doc.columns = cur.clone();
            continue;
        }
        let next = mk.master.as_deref().filter(|m| pages.masters.contains_key(*m));
        // A master page followed by another one (after its first page)
        // switches to it at the next text section: a continuous section
        // with its own page setup, as LibreOffice imports them from docx.
        let follow = pages.masters.get(master).and_then(|m| m.next.as_deref()).filter(|n| pages.masters.contains_key(*n) && mk.sects_opened > opened);
        let sect = if let Some(m) = next {
            (master, opened) = (m, mk.sects_opened);
            page = page_of(pages, styles, m).unwrap_or(page);
            let start = extra(m).map_or(SectionStart::NextPage, |e| e.start);
            Some(Section { start, page, columns: cols(mk, m, &page), headers: headers_of(m, mk.page_number) })
        } else if let Some(m) = follow {
            // Headers as the previous master page's are linked to them.
            let mut headers = headers_of(m, None);
            for slot in apex_doc::HfSlot::ALL {
                if hf.get(m).and_then(|h| h.get(slot)) == hf.get(master).and_then(|h| h.get(slot)) {
                    headers.set(slot, None);
                }
            }
            (master, opened) = (m, mk.sects_opened);
            page = page_of(pages, styles, m).unwrap_or(page);
            Some(Section { start: SectionStart::Continuous, page, columns: cols(mk, m, &page), headers })
        } else {
            let c = cols(mk, master, &page);
            (c != cur).then(|| Section { start: SectionStart::Continuous, page, columns: c, headers: Default::default() })
        };
        if let Some(s) = sect {
            cur = s.columns.clone();
            doc.paragraphs[mk.para].section = Some(Box::new(s));
        }
    }
}

/// Reads the watermark in the header of master page `master` of a
/// styles.xml.
pub(crate) fn read_watermark(xml: &str, master: &str, pictures: &HashMap<String, Arc<[u8]>>) -> Option<Watermark> {
    // Graphic styles: fill color, opacity, washed out picture.
    let mut graphics: HashMap<String, (Option<u32>, Option<f32>, bool)> = HashMap::new();
    // Paragraph styles: font and size.
    let mut fonts: HashMap<String, (Option<String>, Option<f32>)> = HashMap::new();
    let mut cur: Option<(String, bool)> = None;
    let mut in_master = false;
    let mut in_header = false;
    let mut r = Reader::new(xml);
    loop {
        let ev = r.next();
        let is_start = matches!(ev, Ev::Start(_));
        match ev {
            Ev::Eof => return None,
            Ev::Start(t) | Ev::Empty(t) => match t.name {
                "style:style" => {
                    let name = t.attr("style:name").unwrap_or_default().into_owned();
                    let graphic = t.attr("style:family").as_deref() == Some("graphic");
                    cur = is_start.then_some((name, graphic));
                }
                "style:graphic-properties" | "loext:graphic-properties" => {
                    if let Some((name, true)) = &cur {
                        let g = graphics.entry(name.clone()).or_default();
                        if t.attr("draw:fill").as_deref() != Some("none") {
                            g.0 = t.attr("draw:fill-color").and_then(|c| parse_hex_color(&c)).or(g.0);
                        }
                        g.1 = t.attr("draw:opacity").and_then(|v| v.trim_end_matches('%').parse::<f32>().ok()).or(g.1);
                        g.2 |= t.attr("draw:color-mode").as_deref() == Some("watermark");
                    }
                }
                "style:text-properties" => {
                    if let Some((name, false)) = &cur {
                        let f = fonts.entry(name.clone()).or_default();
                        f.0 = t.attr("fo:font-family").or_else(|| t.attr("style:font-name")).map(|v| v.trim_matches('\'').to_string()).or(f.0.take());
                        f.1 = t.attr("fo:font-size").and_then(|v| parse_length_pt(&v)).or(f.1);
                    }
                }
                "style:master-page" => in_master = is_start && t.attr("style:name").as_deref() == Some(master),
                "style:header" | "style:header-left" | "style:header-first" => in_header = is_start && in_master,
                "draw:custom-shape" if in_header && is_start => {
                    let named = t.attr("draw:name").is_some_and(|n| n.starts_with("PowerPlusWaterMarkObject"));
                    let g = t.attr("draw:style-name").and_then(|s| graphics.get(s.as_ref()).copied()).unwrap_or_default();
                    let mut font = t.attr("draw:text-style-name").and_then(|s| fonts.get(s.as_ref()).cloned()).unwrap_or_default();
                    let diagonal = t.attr("draw:transform").is_some_and(|tr| rotation(&tr).is_some_and(|a| a.abs() > 0.1));
                    let (mut text, mut fontwork) = (String::new(), false);
                    let mut depth = 1;
                    while depth > 0 {
                        match r.next() {
                            Ev::Eof => break,
                            Ev::Empty(c) if c.name == "draw:enhanced-geometry" => fontwork = is_fontwork(&c),
                            Ev::Start(c) if c.name == "draw:enhanced-geometry" => {
                                fontwork = is_fontwork(&c);
                                r.skip();
                            }
                            Ev::Start(c) => {
                                if c.name == "text:p" {
                                    if let Some(f) = c.attr("text:style-name").and_then(|s| fonts.get(s.as_ref())) {
                                        font = (f.0.clone().or(font.0), f.1.or(font.1));
                                    }
                                }
                                depth += 1;
                            }
                            Ev::End(_) => depth -= 1,
                            Ev::Text(x) => text.push_str(&x),
                            _ => {}
                        }
                    }
                    if (named || fontwork) && !text.trim().is_empty() {
                        return Some(Watermark {
                            text: text.trim().to_string(),
                            font: font.0.map(|f| Arc::from(f.as_str())),
                            size: font.1.filter(|&s| s > 1.5),
                            color: g.0.unwrap_or(0xC0C0C0),
                            semitransparent: g.1.is_some_and(|o| o < 100.0),
                            diagonal,
                            ..Default::default()
                        });
                    }
                }
                "draw:frame" if in_header && is_start => {
                    let named = t.attr("draw:name").is_some_and(|n| n.to_ascii_lowercase().contains("watermark"));
                    let g = t.attr("draw:style-name").and_then(|s| graphics.get(s.as_ref()).copied()).unwrap_or_default();
                    let width = t.attr("svg:width").and_then(|v| parse_length_pt(&v)).unwrap_or(0.0);
                    let image = |c: &Tag| c.attr("xlink:href").and_then(|h| pictures.get(h.trim_start_matches("./")).cloned());
                    let mut data = None;
                    let mut depth = 1;
                    while depth > 0 {
                        match r.next() {
                            Ev::Eof => break,
                            Ev::Empty(c) if c.name == "draw:image" && data.is_none() => data = image(&c),
                            Ev::Start(c) => {
                                if c.name == "draw:image" && data.is_none() {
                                    data = image(&c);
                                }
                                depth += 1;
                            }
                            Ev::End(_) => depth -= 1,
                            _ => {}
                        }
                    }
                    if let Some(data) = data.filter(|_| named || g.2) {
                        let natural = crate::pixel_size(&data).map_or(0.0, |(w, _)| w as f32 * 0.75);
                        let scale = if natural > 0.0 && width > 0.0 { (width / natural * 100.0).round() / 100.0 } else { 0.0 };
                        return Some(Watermark { picture: Some(data), scale, washout: g.2, ..Default::default() });
                    }
                }
                _ => {}
            },
            Ev::End(n) => match n {
                "style:style" => cur = None,
                "style:master-page" => in_master = false,
                "style:header" | "style:header-left" | "style:header-first" => in_header = false,
                _ => {}
            },
            _ => {}
        }
    }
}

fn is_fontwork(t: &Tag) -> bool {
    t.attr("draw:type").is_some_and(|v| v.starts_with("fontwork")) || t.attr("draw:text-path").as_deref() == Some("true")
}

/// The angle of a `rotate (a)` transform.
fn rotation(tr: &str) -> Option<f32> {
    tr.split("rotate").nth(1)?.trim_start().trim_start_matches('(').split(')').next()?.trim().parse().ok()
}

// --- writing ----------------------------------------------------------------------

fn ptx(v: f32) -> String {
    format!("{}pt", (v * 100.0).round() / 100.0)
}

fn border_value(l: &Line) -> String {
    let style = match l.style {
        LineStyle::Double => "double",
        LineStyle::Dotted => "dotted",
        LineStyle::Dashed => "dashed",
        LineStyle::DashDot => "dot-dash",
        _ => "solid",
    };
    let w = if l.style == LineStyle::Double { l.width * 3.0 } else { l.width };
    format!("{} {style} #{:06x}", ptx(w.max(0.05)), l.color & 0xFF_FFFF)
}

fn write_borders(out: &mut String, sides: [Option<&Line>; 4], padding: [f32; 4]) {
    for (i, side) in SIDES.iter().enumerate() {
        if let Some(l) = sides[i] {
            let _ = write!(out, r#" fo:border-{side}="{}" fo:padding-{side}="{}""#, border_value(l), ptx(padding[i].max(0.0)));
            if l.style == LineStyle::Double {
                let w = ptx(l.width);
                let _ = write!(out, r#" style:border-line-width-{side}="{w} {w} {w}""#);
            }
        }
    }
}

/// Paragraph layout properties: attributes of `style:paragraph-properties`
/// and its children. `indent` is the left indent written.
pub(crate) fn para_xml(st: &ParaStyle, indent: f32) -> (String, String) {
    let mut props = String::new();
    let mut children = String::new();
    if st.column_break_before && !st.page_break_before {
        props.push_str(r#" fo:break-before="column""#);
    }
    let b = &st.borders;
    let sides = [b.top.as_ref(), b.bottom.as_ref(), b.left.as_ref(), b.right.as_ref()];
    write_borders(&mut props, sides, sides.map(|l| l.map_or(0.0, |l| l.space)));
    props.push_str(&crate::odt_asian::para_attrs(&st.asian));
    if !st.tabs.is_empty() {
        children.push_str("<style:tab-stops>");
        for t in st.tabs.as_slice() {
            let ty = match t.align {
                TabAlign::Left => "left",
                TabAlign::Center => "center",
                TabAlign::Right => "right",
                TabAlign::Decimal => "char",
            };
            let _ = write!(children, r#"<style:tab-stop style:position="{}" style:type="{ty}""#, ptx(t.pos - indent));
            if t.align == TabAlign::Decimal {
                children.push_str(r#" style:char=".""#);
            }
            let leader = match t.leader {
                TabLeader::None => None,
                TabLeader::Dots => Some(("dotted", ".")),
                TabLeader::Dashes => Some(("dash", "-")),
                TabLeader::Underline => Some(("solid", "_")),
            };
            if let Some((style, text)) = leader {
                let _ = write!(children, r#" style:leader-style="{style}" style:leader-text="{text}""#);
            }
            children.push_str("/>");
        }
        children.push_str("</style:tab-stops>");
    }
    if let Some(dc) = st.drop_cap {
        let dc = dc.sane();
        // Caps in the margin are dropped: ODF has no such position.
        let _ = write!(children, r#"<style:drop-cap style:lines="{}" style:length="{}" style:distance="{}"/>"#, dc.lines.max(2), dc.chars, ptx(dc.distance));
    }
    (props, children)
}

/// A master page: its page setup and the pages it starts on.
#[derive(Clone)]
pub(crate) struct Master {
    pub name: String,
    pub page: PageSetup,
    pub start: SectionStart,
    /// The headers and footers shown (links to previous sections resolved).
    pub hf: MasterHf,
    /// The master page of the pages after the first.
    pub next: Option<usize>,
}

/// The most master pages written: each writes its headers and footers,
/// and sections may be many.
const MAX_MASTERS: usize = 256;

/// The headers and footers of a master page.
#[derive(Clone, PartialEq, Default)]
pub(crate) struct MasterHf {
    pub parts: [Option<apex_doc::Story>; 6],
    pub different_first: bool,
    pub odd_even: bool,
    /// Page number format.
    pub format: NoteFormat,
}

impl MasterHf {
    pub(crate) fn of(doc: &Document, s: usize) -> MasterHf {
        let parts = apex_doc::HfSlot::ALL.map(|slot| doc.hf_story(s, slot).filter(|st| !apex_doc::story_is_empty(st)).cloned());
        let h = doc.section_headers(s);
        MasterHf { parts, different_first: h.different_first, odd_even: doc.odd_even, format: h.numbering.format }
    }
}

/// Sections as written: master pages and text sections with columns.
pub(crate) struct SectWriter {
    /// Per section: its first paragraph, the master page it switches to,
    /// its section style (with columns) and the page number it restarts at.
    sects: Vec<(usize, Option<usize>, Option<String>, Option<u32>)>,
    pub masters: Vec<Master>,
    open: bool,
}

impl SectWriter {
    /// Also writes the section styles to `auto`.
    pub(crate) fn new(doc: &Document, auto: &mut String) -> SectWriter {
        let mut masters = vec![Master { name: "Standard".into(), page: doc.page, start: SectionStart::NextPage, hf: MasterHf::of(doc, 0), next: None }];
        let mut sects: Vec<(usize, Option<usize>, Option<String>, Option<u32>)> = Vec::new();
        let mut page = doc.page;
        // The master page in effect and the section that switched to it.
        let (mut cur, mut owner) = (0, 0);
        for (k, s) in doc.sections().iter().enumerate() {
            let mut master = None;
            // A continuous section that only changes the document grid
            // stays on the page, as in Word.
            let same_page = PageSetup { grid: page.grid, ..*s.page } == page;
            // One with other margins or headers on paper of the same size
            // (as the layout takes it) too: a text section, whose master page follows the current
            // one after its first page (as LibreOffice imports docx).
            let follow = masters.len() + 2 <= MAX_MASTERS && k > 0 && s.start == SectionStart::Continuous && !same_page && (s.page.width - page.width).abs() < 0.5 && (s.page.height - page.height).abs() < 0.5;
            let indent = [s.page.margin_left - page.margin_left, s.page.margin_right - page.margin_right];
            if follow {
                let hf = MasterHf::of(doc, k);
                masters.push(Master { name: format!("Page{}", masters.len() + 1), page: *s.page, start: SectionStart::Continuous, hf, next: None });
                let new = masters.len() - 1;
                // A master page other sections show too gets a copy.
                let users = sects.iter().filter(|x| x.1 == Some(cur)).count() + usize::from(cur == 0 && sects[0].1 != Some(0));
                if users > 1 || masters[cur].next.is_some() {
                    let copy = Master { name: format!("Page{}", masters.len() + 1), next: Some(new), ..masters[cur].clone() };
                    masters.push(copy);
                    sects[owner].1 = Some(masters.len() - 1);
                } else {
                    masters[cur].next = Some(new);
                }
                (cur, owner) = (new, k);
                page = *s.page;
            } else if k > 0 && (s.start != SectionStart::Continuous || !same_page) {
                let start = if s.start == SectionStart::Continuous { SectionStart::NextPage } else { s.start };
                let hf = MasterHf::of(doc, k);
                let found = masters.iter().position(|m| m.page == *s.page && m.start == start && m.hf == hf && m.next.is_none());
                // (Past the most master pages, the section keeps the current one.)
                master = Some(match found {
                    Some(f) => f,
                    None if masters.len() < MAX_MASTERS => {
                        masters.push(Master { name: format!("Page{}", masters.len() + 1), page: *s.page, start, hf, next: None });
                        masters.len() - 1
                    }
                    None => cur,
                });
                (cur, owner) = (master.unwrap(), k);
                page = *s.page;
            }
            let style = (!s.columns.is_single() || follow).then(|| {
                let name = format!("Sect{}", k + 1);
                section_style(auto, &name, s.columns, s.page.content_width(), if follow { indent } else { [0.0; 2] });
                name
            });
            // A restart needs a new page (the first one names its master).
            let restart = doc.section_headers(k).numbering.start.filter(|_| k == 0 || master.is_some());
            if k == 0 && restart.is_some() {
                master = Some(0);
            }
            sects.push((s.first, master, style, restart));
        }
        SectWriter { sects, masters, open: false }
    }

    /// Whether a section starts at paragraph `idx`.
    pub(crate) fn starts(&self, idx: usize) -> bool {
        self.sects.iter().any(|s| s.0 == idx)
    }

    /// Closes the open text section and opens the one starting at `idx`.
    pub(crate) fn open(&mut self, body: &mut String, idx: usize) {
        self.close(body);
        if let Some((k, (_, _, Some(style), _))) = self.sects.iter().enumerate().find(|s| s.1.0 == idx) {
            let _ = write!(body, r#"<text:section text:style-name="{style}" text:name="Section{}">"#, k + 1);
            self.open = true;
        }
    }

    pub(crate) fn close(&mut self, body: &mut String) {
        if std::mem::take(&mut self.open) {
            body.push_str("</text:section>");
        }
    }

    /// The master page paragraph `idx` switches to.
    pub(crate) fn master_at(&self, idx: usize) -> Option<&str> {
        let m = self.sects.iter().find(|s| s.0 == idx)?.1?;
        Some(&self.masters[m].name)
    }

    /// The page number paragraph `idx` restarts at.
    pub(crate) fn page_number_at(&self, idx: usize) -> Option<u32> {
        self.sects.iter().find(|s| s.0 == idx)?.3
    }
}

/// `indent` is the left and right margins of the section beside the page's.
fn section_style(out: &mut String, name: &str, c: &Columns, width: f32, indent: [f32; 2]) {
    let cols = c.layout(width);
    let n = cols.len();
    let gap = |i: usize| if i + 1 < n { (cols[i + 1].0 - cols[i].0 - cols[i].1).max(0.0) } else { 0.0 };
    let margins: String = [("left", indent[0]), ("right", indent[1])].iter().filter(|m| m.1.abs() > 0.05).map(|(side, v)| format!(r#" fo:margin-{side}="{}""#, ptx(*v))).collect();
    let _ = write!(out, r#"<style:style style:name="{name}" style:family="section"><style:section-properties text:dont-balance-text-columns="false" style:editable="false"{margins}><style:columns fo:column-count="{n}" fo:column-gap="{}">"#, ptx(gap(0)));
    if c.separator {
        out.push_str(r##"<style:column-sep style:style="solid" style:width="0.5pt" style:color="#000000" style:height="100%" style:vertical-align="top"/>"##);
    }
    for (i, &(_, w)) in cols.iter().enumerate() {
        let start = if i > 0 { gap(i - 1) / 2.0 } else { 0.0 };
        let end = gap(i) / 2.0;
        let _ = write!(out, r#"<style:column style:rel-width="{}*" fo:start-indent="{}" fo:end-indent="{}"/>"#, ((w + start + end) * 20.0).round() as u32, ptx(start), ptx(end));
    }
    out.push_str("</style:columns></style:section-properties></style:style>");
}

/// Page layout properties: margins (less the borders' room), borders and
/// background. `margins` are top, bottom, left and right as written
/// without borders.
pub(crate) fn layout_props(margins: [f32; 4], deco: &PageDeco, fill: &str) -> String {
    let b = &deco.borders;
    let sides = [b.top.as_ref(), b.bottom.as_ref(), b.left.as_ref(), b.right.as_ref()];
    let mut m = margins;
    let mut padding = [0.0; 4];
    for i in 0..4 {
        if let Some(l) = sides[i] {
            let room = (margins[i] - l.thickness()).max(0.0);
            let (edge, pad) = if b.from_edge { (l.space.min(room), room - l.space.min(room)) } else { (room - l.space.min(room), l.space.min(room)) };
            m[i] = edge;
            padding[i] = pad;
        }
    }
    let mut s = String::new();
    for (i, side) in SIDES.iter().enumerate() {
        let _ = write!(s, r#" fo:margin-{side}="{}""#, ptx(m[i]));
    }
    write_borders(&mut s, sides, padding);
    if let Some(c) = deco.color {
        // draw:fill isn't valid here (LibreOffice's extension); fo:background-color is.
        let _ = write!(s, r##" fo:background-color="#{:06x}""##, c & 0xFF_FFFF);
        // A gradient: LibreOffice's fill properties (`fill`, see odf_grad).
        s.push_str(fill);
    }
    s
}

/// Automatic styles of the watermark shape.
pub(crate) fn watermark_styles(w: &Watermark) -> String {
    let mut s = String::from(r#"<style:style style:name="Mwg1" style:family="graphic"><style:graphic-properties draw:stroke="none""#);
    if w.picture.is_some() {
        if w.washout {
            s.push_str(r#" draw:color-mode="watermark""#);
        }
    } else {
        let _ = write!(s, r##" draw:fill="solid" draw:fill-color="#{:06x}" draw:opacity="{}%""##, w.color & 0xFF_FFFF, if w.semitransparent { 50 } else { 100 });
    }
    s.push_str(r#" draw:auto-grow-height="false" draw:auto-grow-width="false" style:run-through="background" style:wrap="run-through" style:number-wrapped-paragraphs="no-limit" style:vertical-pos="middle" style:vertical-rel="page-content" style:horizontal-pos="center" style:horizontal-rel="page-content" style:flow-with-text="false" draw:wrap-influence-on-position="once-concurrent" draw:textarea-vertical-align="top"/></style:style>"#);
    let font = w.font.as_deref().unwrap_or("Liberation Sans");
    let size = w.size.map_or("1pt".to_string(), ptx);
    let _ = write!(s, r#"<style:style style:name="Mwp1" style:family="paragraph"><style:text-properties fo:font-family="'{}'" fo:font-size="{size}"/></style:style>"#, esc(font));
    s
}

const FONTWORK: &str = r#"<draw:enhanced-geometry draw:text-path="true" draw:text-path-mode="shape" draw:text-path-scale="path" draw:text-path-same-letter-heights="false" svg:viewBox="0 0 21600 21600" draw:text-areas="0 0 21600 21600" draw:type="fontwork-plain-text" draw:modifiers="10800" draw:enhanced-path="M ?f3 0 L ?f5 0 N M ?f6 21600 L ?f7 21600 N"><draw:equation draw:name="f0" draw:formula="$0 -10800"/><draw:equation draw:name="f1" draw:formula="?f0 *2"/><draw:equation draw:name="f2" draw:formula="abs(?f1 )"/><draw:equation draw:name="f3" draw:formula="if(?f1 ,0,?f2 )"/><draw:equation draw:name="f4" draw:formula="21600-?f2 "/><draw:equation draw:name="f5" draw:formula="if(?f1 ,?f4 ,21600)"/><draw:equation draw:name="f6" draw:formula="if(?f1 ,?f2 ,0)"/><draw:equation draw:name="f7" draw:formula="if(?f1 ,21600,?f4 )"/><draw:handle draw:handle-position="$0 21600" draw:handle-position-x="$0" draw:handle-position-y="21600" draw:handle-range-x-minimum="6629" draw:handle-range-x-maximum="14971"/></draw:enhanced-geometry>"#;

/// The watermark paragraph of a header, as LibreOffice writes it: fontwork
/// text or a picture centered on the page; `href` is the picture's path.
pub(crate) fn watermark_xml(w: &Watermark, page: &PageSetup, href: Option<&str>) -> String {
    let (cw, ch) = (page.content_width(), page.content_height());
    let mut s = String::from(r#"<text:p text:style-name="MP1">"#);
    match (&w.picture, href) {
        (Some(data), Some(href)) => {
            let (nw, nh) = crate::pixel_size(data).map_or((cw, ch), |(pw, ph)| (pw.max(1) as f32 * 0.75, ph.max(1) as f32 * 0.75));
            let k = if w.scale > 0.0 { w.scale } else { (cw / nw).min(ch / nh) };
            let _ = write!(
                s,
                r#"<draw:frame draw:style-name="Mwg1" draw:name="WordPictureWatermark1" text:anchor-type="char" svg:x="0pt" svg:y="0pt" svg:width="{}" svg:height="{}" draw:z-index="0"><draw:image xlink:href="{href}" xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad" draw:mime-type="{}"/></draw:frame>"#,
                ptx(nw * k),
                ptx(nh * k),
                crate::docx_img::image_type(data).1
            );
        }
        _ => {
            let n = w.text.chars().count().max(1) as f32;
            let (width, height) = match w.size {
                Some(sz) => (sz * n * 0.62, sz),
                None => {
                    let width = if w.diagonal { cw.min(ch) } else { cw * 0.9 };
                    (width, width / (n * 0.62).max(1.0))
                }
            };
            let place = if w.diagonal {
                // Turned about the origin, then moved so its center is the
                // text area's center.
                let a = std::f32::consts::FRAC_PI_4;
                let (sin, cos) = a.sin_cos();
                let (cx, cy) = (width / 2.0 * cos + height / 2.0 * sin, -width / 2.0 * sin + height / 2.0 * cos);
                format!(r#"draw:transform="rotate ({a:.6}) translate ({} {})""#, ptx(cw / 2.0 - cx), ptx(ch / 2.0 - cy))
            } else {
                format!(r#"svg:x="{}" svg:y="{}""#, ptx((cw - width) / 2.0), ptx((ch - height) / 2.0))
            };
            let _ = write!(
                s,
                r#"<draw:custom-shape text:anchor-type="char" draw:z-index="0" draw:name="PowerPlusWaterMarkObject1" draw:style-name="Mwg1" draw:text-style-name="Mwp1" svg:width="{}" svg:height="{}" {place}><text:p text:style-name="Mwp1">{}</text:p>{FONTWORK}</draw:custom-shape>"#,
                ptx(width),
                ptx(height),
                esc(&w.text)
            );
        }
    }
    s.push_str("</text:p>");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn borders_and_columns() {
        let l = parse_border("2.25pt double #1f4e79").unwrap();
        assert_eq!((l.style, l.width, l.color), (LineStyle::Double, 0.75, 0x1F4E79));
        assert_eq!(parse_border(&border_value(&l)), Some(l));
        assert!(parse_border("none").is_none() && parse_border("0pt solid #000000").is_none());
        let c = Columns { count: 2, spacing: 36.0, separator: true, custom: vec![(100.0, 20.0), (300.0, 0.0)] };
        let mut xml = String::new();
        section_style(&mut xml, "S", &c, 420.0, [0.0; 2]);
        let mut st = Styles::default();
        let mut r = Reader::new(&xml);
        loop {
            match r.next() {
                Ev::Eof => break,
                Ev::Start(t) | Ev::Empty(t) if t.name == "style:style" => {
                    st.start_style(&t, true);
                }
                Ev::Start(t) | Ev::Empty(t) => st.columns(&t),
                Ev::End("style:style") => st.end(),
                _ => {}
            }
        }
        let back = st.sections["S"].columns(420.0);
        assert!(back.separator);
        assert_eq!(back.custom, vec![(100.0, 20.0), (300.0, 0.0)]);
        let mut xml = String::new();
        section_style(&mut xml, "E", &Columns::equal(3, 18.0), 420.0, [0.0; 2]);
        assert!(xml.contains(r#"fo:column-count="3""#) && !xml.contains("column-sep"));
    }

    #[test]
    fn layout_roundtrip() {
        let d = crate::docx_layout::tests::layout_sample();
        let bytes = crate::odt::write(&d);
        // APEX_DUMP=dir keeps both files, to check them in other programs.
        if let Some(dir) = std::env::var_os("APEX_DUMP") {
            let dir = std::path::Path::new(&dir);
            let _ = std::fs::write(dir.join("layout.odt"), &bytes);
            let _ = std::fs::write(dir.join("layout.docx"), crate::docx::write(&d));
        }
        let back = crate::odt::read(&bytes).unwrap();
        assert_eq!(back.paragraphs.len(), d.paragraphs.len());
        // ODF has no line between bordered paragraphs.
        let mut expect = d.clone();
        for p in &mut expect.paragraphs {
            p.style.borders.between = None;
        }
        crate::docx_layout::tests::check_layout(&back, &expect, false);
        assert!((back.page.margin_top - d.page.margin_top).abs() < 0.1 && (back.page.margin_left - d.page.margin_left).abs() < 0.1);
        assert!(!back.has_headers());
    }

    #[test]
    fn picture_watermark() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend_from_slice(&[0, 0, 0, 200, 0, 0, 0, 100, 8, 6, 0, 0, 0]);
        let mut d = Document::from_paragraphs(vec![apex_doc::Paragraph::new("x")]);
        d.deco.watermark = Some(Watermark { picture: Some(Arc::from(png)), scale: 1.5, washout: true, ..Default::default() });
        for (bytes, docx) in [(crate::odt::write(&d), false), (crate::docx::write(&d), true)] {
            let back = if docx { crate::docx::read(&bytes) } else { crate::odt::read(&bytes) }.unwrap();
            let w = back.deco.watermark.clone().expect("watermark");
            assert_eq!((w.picture.as_deref(), w.scale, w.washout), (d.deco.watermark.as_ref().and_then(|w| w.picture.as_deref()), 1.5, true), "docx {docx}");
            assert!(!back.has_headers());
        }
    }
}
