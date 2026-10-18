//! Tables in .docx files (`w:tbl`): reading into tagged paragraphs and the
//! table markup around paragraphs when writing.

use std::collections::HashMap;
use std::fmt::Write as _;

use apex_doc::{Align, Border, Borders, CellProps, Document, InCell, Paragraph, TABLE_STYLES, TableLook, TableMap, TableProps, TableStyle, TextDir, VAlign};

use crate::parse_hex_color;
use crate::xml::{Ev, Reader, Tag, esc};

const TWIP: f32 = 20.0;

fn twips(t: &Tag, k: &str) -> Option<f32> {
    t.attr(k).and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite()).map(|v| v / TWIP)
}

fn on(t: &Tag) -> bool {
    t.attr("w:val").is_none_or(|v| !matches!(v.as_ref(), "0" | "false" | "off"))
}

/// The rule of row `r` to set.
fn row_rule(p: &mut TableProps, r: usize) -> &mut apex_doc::RowRule {
    if p.row_rules.len() <= r {
        p.row_rules.resize(r + 1, Default::default());
    }
    &mut p.row_rules[r]
}

/// A border element (`w:top`, `w:insideH`…).
fn border(t: &Tag) -> Border {
    if matches!(t.attr("w:val").as_deref(), Some("nil" | "none")) {
        return Border::NONE;
    }
    let w = t.attr("w:sz").and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite()).map_or(0.5, |v| v / 8.0);
    let color = t.attr("w:color").filter(|c| c != "auto").and_then(|c| parse_hex_color(&c)).unwrap_or(0);
    Border::new(w.clamp(0.25, 6.0), color)
}

/// Reads the children of `w:tblBorders` / `w:tcBorders`.
fn read_borders(r: &mut Reader, b: &mut Borders) {
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
                let v = Some(border(&t));
                match t.local() {
                    "top" => b.top = v,
                    "bottom" => b.bottom = v,
                    "left" | "start" => b.left = v,
                    "right" | "end" => b.right = v,
                    "insideH" => b.inside_h = v,
                    "insideV" => b.inside_v = v,
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

fn shading(t: &Tag) -> Option<u32> {
    t.attr("w:fill").filter(|v| v != "auto").and_then(|v| parse_hex_color(&v))
}

/// A table style of styles.xml (its own settings, without the ones it
/// inherits).
#[derive(Default, Clone)]
pub(crate) struct TblStyleDef {
    name: String,
    based_on: Option<String>,
    borders: Borders,
    header_fill: Option<u32>,
    header_color: Option<u32>,
    header_bold: Option<bool>,
    header_line: Option<Border>,
    band_fill: Option<u32>,
    band2_fill: Option<u32>,
    first_col_bold: Option<bool>,
    /// The default table style (applies to tables without a style).
    default: bool,
    /// Space before and after the paragraphs of its cells (points), from
    /// the style's own `w:pPr`: Word puts it above the document defaults
    /// and Normal, below paragraph styles and direct formatting.
    spacing: [Option<f32>; 2],
    /// Its line spacing, a multiple of single (`w:lineRule="auto"`).
    line: Option<f32>,
    /// The font size of its cells' text (half points).
    size: Option<u16>,
    /// Its cell margins (top, left, bottom, right; points).
    margins: [Option<f32>; 4],
}

/// Reads a `w:tblCellMar` into `m`; false at the end of the file.
fn read_cell_mar(r: &mut Reader, m: &mut [Option<f32>; 4]) -> bool {
    let mut d = 1;
    while d > 0 {
        let ev = r.next();
        match ev {
            Ev::Eof => return false,
            Ev::End(_) => d -= 1,
            Ev::Start(side) | Ev::Empty(side) => {
                let k = match side.local() {
                    "top" => 0,
                    "left" | "start" => 1,
                    "bottom" => 2,
                    _ => 3,
                };
                if let Some(v) = twips(&side, "w:w").filter(|_| side.attr("w:type").is_none_or(|v| v == "dxa")) {
                    m[k] = Some(v.clamp(0.0, 200.0));
                }
                if matches!(ev, Ev::Start(_)) {
                    d += 1;
                }
            }
            _ => {}
        }
    }
    true
}

impl TblStyleDef {
    /// Whether it sets anything beyond the borders.
    fn has_look(&self) -> bool {
        self.header_fill.is_some() || self.header_color.is_some() || self.header_bold.is_some() || self.header_line.is_some() || self.band_fill.is_some() || self.band2_fill.is_some() || self.first_col_bold.is_some()
    }
}

pub(crate) fn read_table_styles(xml: &str) -> HashMap<String, TblStyleDef> {
    let mut out = HashMap::new();
    let mut r = Reader::new(xml);
    let mut cur: Option<(String, TblStyleDef)> = None;
    // The conditional part being read (`w:tblStylePr` type).
    let mut part = String::new();
    let mut in_ppr = false;
    loop {
        let ev = r.next();
        let is_start = matches!(ev, Ev::Start(_));
        match ev {
            Ev::Eof => break,
            Ev::Start(t) if t.local() == "style" => {
                let default = t.attr("w:default").is_some_and(|v| on_str(&v));
                cur = (t.attr("w:type").as_deref() == Some("table")).then(|| (t.attr("w:styleId").unwrap_or_default().into_owned(), TblStyleDef { default, ..Default::default() }));
                part.clear();
            }
            Ev::End(n) if crate::xml::local(n) == "style" => {
                if let Some((id, s)) = cur.take() {
                    out.insert(id, s);
                }
            }
            Ev::End(n) if crate::xml::local(n) == "tblStylePr" => part.clear(),
            Ev::End(n) if crate::xml::local(n) == "pPr" => in_ppr = false,
            Ev::Start(t) | Ev::Empty(t) if cur.is_some() => {
                let Some((_, s)) = &mut cur else { continue };
                let on = || t.attr("w:val").is_none_or(|v| !matches!(v.as_ref(), "0" | "false" | "off"));
                match (t.local(), part.as_str()) {
                    ("name", "") => s.name = t.attr("w:val").unwrap_or_default().into_owned(),
                    ("basedOn", "") => s.based_on = t.attr("w:val").map(|v| v.into_owned()),
                    ("tblBorders", "") if is_start => read_borders(&mut r, &mut s.borders),
                    ("tblCellMar", "") if is_start => {
                        if !read_cell_mar(&mut r, &mut s.margins) {
                            break;
                        }
                    }
                    ("tblStylePr", _) => part = t.attr("w:type").unwrap_or_default().into_owned(),
                    ("pPr", "") => in_ppr = is_start,
                    ("pPrChange", _) if is_start => r.skip(),
                    ("spacing", "") if in_ppr => {
                        let pt = |k: &str| t.attr(k).and_then(|v| v.parse::<f32>().ok()).map(|v| (v / 20.0 * 10.0).round() / 10.0);
                        s.spacing = [pt("w:before").or(s.spacing[0]), pt("w:after").or(s.spacing[1])];
                        if t.attr("w:lineRule").is_none_or(|v| v == "auto") {
                            s.line = t.attr("w:line").and_then(|v| v.parse::<f32>().ok()).map(|l| (l / 240.0 * 100.0).round() / 100.0).or(s.line);
                        }
                    }
                    ("sz", "") if !in_ppr => s.size = t.attr("w:val").and_then(|v| v.parse().ok()).filter(|&v: &u16| v > 0),
                    ("b", "firstRow") => s.header_bold = Some(on()),
                    ("b", "firstCol") => s.first_col_bold = Some(on()),
                    ("color", "firstRow") => s.header_color = t.attr("w:val").filter(|v| v != "auto").and_then(|v| parse_hex_color(&v)),
                    ("shd", "firstRow") => s.header_fill = shading(&t),
                    ("shd", "band1Horz") => s.band_fill = shading(&t),
                    ("shd", "band2Horz") => s.band2_fill = shading(&t),
                    ("tcBorders", "firstRow") if is_start => {
                        let mut b = Borders::default();
                        read_borders(&mut r, &mut b);
                        s.header_line = b.bottom.filter(|l| !l.is_none());
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    out
}

fn squash(s: &str) -> String {
    s.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect()
}

/// One of our styles matching a style id or name ("Table Grid",
/// "Grid Table 4 Accent 1"…).
fn builtin(id_or_name: &str) -> Option<&'static TableStyle> {
    let k = squash(id_or_name);
    TABLE_STYLES.iter().find(|s| squash(&s.id) == k || squash(&s.name) == k)
}

/// Resolves `w:tblStyle`: one of ours, a style of the file with a look of
/// its own (kept as a document table style, see `import_styles`), or the
/// style's borders as direct formatting.
fn resolve_style(id: &str, styles: &HashMap<String, TblStyleDef>, props: &mut TableProps) {
    if squash(id) == "tablenormal" {
        return;
    }
    let mut chain = Vec::new();
    let mut cur = Some(id.to_string());
    while let Some(c) = cur.take() {
        if chain.len() > 12 {
            break;
        }
        if let Some(s) = builtin(&c).or_else(|| styles.get(&c).and_then(|d| builtin(&d.name))) {
            if chain.is_empty() {
                props.style = Some(s.id.as_ref().into());
                return;
            }
            break;
        }
        let Some(d) = styles.get(&c) else { break };
        chain.push(d);
        cur = d.based_on.clone();
    }
    if chain.iter().any(|d| d.has_look()) || !chain.is_empty() && builtin_base(id, styles).is_some() {
        props.style = Some(id.into());
        return;
    }
    let mut b = Borders::default();
    for d in chain.iter().rev() {
        b.overlay(&d.borders);
    }
    b.overlay(&props.borders);
    props.borders = b;
}

/// The built-in style a file style is based on, if any.
fn builtin_base(id: &str, styles: &HashMap<String, TblStyleDef>) -> Option<&'static TableStyle> {
    let mut cur = styles.get(id)?.based_on.clone();
    for _ in 0..12 {
        let c = cur.take()?;
        if let Some(s) = builtin(&c).or_else(|| styles.get(&c).and_then(|d| builtin(&d.name))) {
            return Some(s);
        }
        cur = styles.get(&c)?.based_on.clone();
    }
    None
}

/// The document table styles for the file styles its tables use (those
/// `resolve_style` kept by id).
pub(crate) fn import_styles(styles: &HashMap<String, TblStyleDef>, doc: &Document) -> Vec<TableStyle> {
    let mut out: Vec<TableStyle> = Vec::new();
    for m in doc.all_tables() {
        let Some(id) = m.props(&doc.paragraphs).and_then(|p| p.style.clone()) else { continue };
        if apex_doc::table_style(&id).is_some() || out.iter().any(|s| *s.id == *id) || !styles.contains_key(&*id) {
            continue;
        }
        let mut st = builtin_base(&id, styles).cloned().unwrap_or(apex_doc::PLAIN);
        let mut chain = Vec::new();
        let mut cur = Some(id.to_string());
        while let Some(c) = cur.take() {
            let Some(d) = styles.get(&c).filter(|_| chain.len() < 12 && builtin(&c).is_none()) else { break };
            chain.push(d);
            cur = d.based_on.clone();
        }
        for d in chain.iter().rev() {
            st.borders.overlay(&d.borders);
            st.header_fill = d.header_fill.or(st.header_fill);
            st.header_color = d.header_color.or(st.header_color);
            st.header_bold = d.header_bold.unwrap_or(st.header_bold);
            st.header_line = d.header_line.or(st.header_line);
            st.band_fill = d.band_fill.or(st.band_fill);
            st.band2_fill = d.band2_fill.or(st.band2_fill);
            st.first_col_bold = d.first_col_bold.unwrap_or(st.first_col_bold);
        }
        let name = styles.get(&*id).map(|d| d.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| id.to_string());
        st.id = id.to_string().into();
        st.name = name.into();
        out.push(st);
    }
    out
}

struct CellState {
    start: usize,
    props: CellProps,
    span: usize,
    /// `Some(true)` starts a vertical merge, `Some(false)` continues one.
    vmerge: Option<bool>,
    width: Option<f32>,
}

#[derive(Default)]
struct Tbl {
    id: u32,
    /// Index of the table's first paragraph in the output.
    start: usize,
    props: TableProps,
    grid: Vec<f32>,
    style: Option<String>,
    row: Option<u32>,
    col: usize,
    row_header: bool,
    headers_done: bool,
    cell: Option<CellState>,
    /// Column of a vertical merge in progress: output index of its first
    /// paragraph.
    vmerge: HashMap<usize, usize>,
    first_row_widths: Vec<f32>,
    /// Cell margins set on the table itself.
    mar: [Option<f32>; 4],
}

/// Table state while reading a document: the open tables, innermost last
/// (a nested table's paragraphs get the outer cell's tag when it ends).
#[derive(Default)]
pub(crate) struct TblReader {
    stack: Vec<Tbl>,
    next_id: u32,
    /// Tables open past `MAX_NESTING`: their text stays in the cell.
    flat: usize,
}

impl TblReader {
    fn top(&mut self) -> Option<&mut Tbl> {
        if self.flat > 0 {
            return None;
        }
        self.stack.last_mut()
    }

    pub(crate) fn start_table(&mut self, out_len: usize) {
        if self.stack.len() >= apex_doc::MAX_NESTING || self.flat > 0 {
            self.flat += 1;
            return;
        }
        self.next_id += 1;
        self.stack.push(Tbl { id: self.next_id, start: out_len, ..Default::default() });
    }

    pub(crate) fn read_tbl_pr(&mut self, r: &mut Reader) {
        let Some(t) = self.top() else { return r.skip() };
        let p = &mut t.props;
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
                Ev::Start(tag) | Ev::Empty(tag) => {
                    let start = matches!(ev, Ev::Start(_));
                    match tag.local() {
                        "tblStyle" => t.style = tag.attr("w:val").map(|v| v.into_owned()),
                        "jc" => {
                            p.align = match tag.attr("w:val").as_deref() {
                                Some("center") => Align::Center,
                                Some("right" | "end") => Align::Right,
                                _ => Align::Left,
                            }
                        }
                        "tblInd" if tag.attr("w:type").is_none_or(|v| v == "dxa") => p.indent = twips(&tag, "w:w").unwrap_or(0.0).clamp(-500.0, 500.0),
                        "tblBorders" if start => {
                            read_borders(r, &mut p.borders);
                            continue;
                        }
                        "tblLook" => {
                            let bit = |k: &str, mask: u32| match tag.attr(k) {
                                Some(v) => on_str(&v),
                                None => tag.attr("w:val").and_then(|v| u32::from_str_radix(&v, 16).ok()).is_some_and(|v| v & mask != 0),
                            };
                            p.look = TableLook {
                                first_row: bit("w:firstRow", 0x20),
                                last_row: bit("w:lastRow", 0x40),
                                first_col: bit("w:firstColumn", 0x80),
                                last_col: bit("w:lastColumn", 0x100),
                                banded_rows: !bit("w:noHBand", 0x200),
                                banded_cols: !bit("w:noVBand", 0x400),
                            };
                        }
                        "tblCellMar" if start => {
                            if !read_cell_mar(r, &mut t.mar) {
                                return;
                            }
                            continue;
                        }
                        "tblPrChange" | "tblpPr" if start => {
                            r.skip();
                            continue;
                        }
                        _ => {}
                    }
                    if start {
                        depth += 1;
                    }
                }
                _ => {}
            }
        }
    }

    pub(crate) fn read_grid(&mut self, r: &mut Reader) {
        let Some(t) = self.top() else { return r.skip() };
        let mut depth = 1;
        loop {
            match r.next() {
                Ev::Eof => return,
                Ev::End(_) => {
                    depth -= 1;
                    if depth == 0 {
                        return;
                    }
                }
                Ev::Start(tag) => {
                    if tag.local() == "gridCol" && t.grid.len() < apex_doc::MAX_COLS {
                        t.grid.push(twips(&tag, "w:w").unwrap_or(0.0));
                    }
                    depth += 1;
                }
                Ev::Empty(tag) if tag.local() == "gridCol" && t.grid.len() < apex_doc::MAX_COLS => t.grid.push(twips(&tag, "w:w").unwrap_or(0.0)),
                _ => {}
            }
        }
    }

    pub(crate) fn start_row(&mut self) {
        if let Some(t) = self.top() {
            t.row = Some(t.row.map_or(0, |r| r + 1));
            t.col = 0;
            t.row_header = false;
            t.cell = None;
        }
    }

    pub(crate) fn read_tr_pr(&mut self, r: &mut Reader, out: &mut Vec<Paragraph>) {
        let Some(t) = self.top() else { return r.skip() };
        let row = t.row.unwrap_or(0);
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
                Ev::Start(tag) | Ev::Empty(tag) => {
                    match tag.local() {
                        "tblHeader" => t.row_header = on(&tag),
                        "trHeight" if tag.attr("w:hRule").as_deref() != Some("auto") => {
                            let h = twips(&tag, "w:val").unwrap_or(0.0).clamp(0.0, 1000.0);
                            let hs = &mut t.props.row_heights;
                            if hs.len() <= row as usize {
                                hs.resize(row as usize + 1, 0.0);
                            }
                            hs[row as usize] = h;
                            if tag.attr("w:hRule").as_deref() == Some("exact") && h > 0.0 {
                                row_rule(&mut t.props, row as usize).exact = true;
                            }
                        }
                        "cantSplit" if on(&tag) => row_rule(&mut t.props, row as usize).cant_split = true,
                        "gridBefore" => {
                            // Empty, borderless cells stand for the skipped columns.
                            let n: usize = tag.attr("w:val").and_then(|v| v.parse().ok()).unwrap_or(0);
                            for _ in 0..n.min(apex_doc::MAX_COLS.saturating_sub(t.col as usize)) {
                                let mut p = Paragraph::new("");
                                let mut tag = InCell::new(t.id, row, t.col as u16);
                                let none = Some(Border::NONE);
                                tag.cell = Some(Box::new(CellProps { borders: Borders { top: none, bottom: none, left: none, right: none, ..Default::default() }, ..Default::default() }));
                                p.cell = Some(tag);
                                out.push(p);
                                t.col += 1;
                            }
                        }
                        _ => {}
                    }
                    if matches!(ev, Ev::Start(_)) {
                        if tag.local() == "trPrChange" {
                            r.skip();
                            continue;
                        }
                        depth += 1;
                    }
                }
                _ => {}
            }
        }
    }

    pub(crate) fn start_cell(&mut self, out_len: usize) {
        if let Some(t) = self.top() {
            if t.row.is_none() {
                t.row = Some(0);
            }
            t.cell = Some(CellState { start: out_len, props: CellProps::default(), span: 1, vmerge: None, width: None });
        }
    }

    /// Where the open cell's paragraphs start.
    pub(crate) fn cell_start(&mut self) -> Option<usize> {
        self.top()?.cell.as_ref().map(|c| c.start)
    }

    pub(crate) fn read_tc_pr(&mut self, r: &mut Reader) {
        let Some(t) = self.top() else { return r.skip() };
        // No fill set on a cell of a styled table hides the style's.
        let styled = t.style.is_some();
        let Some(c) = t.cell.as_mut() else { return r.skip() };
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
                Ev::Start(tag) | Ev::Empty(tag) => {
                    let start = matches!(ev, Ev::Start(_));
                    match tag.local() {
                        "tcW" if tag.attr("w:type").is_none_or(|v| v == "dxa") => c.width = twips(&tag, "w:w").filter(|w| *w > 0.0),
                        "gridSpan" => c.span = tag.attr("w:val").and_then(|v| v.parse().ok()).unwrap_or(1usize).clamp(1, apex_doc::MAX_COLS),
                        "vMerge" => c.vmerge = Some(tag.attr("w:val").as_deref() == Some("restart")),
                        "hideMark" => c.props.hide_mark = tag.attr("w:val").is_none_or(|v| !matches!(v.as_ref(), "0" | "false" | "off")),
                        "vAlign" => {
                            c.props.valign = match tag.attr("w:val").as_deref() {
                                Some("center") => VAlign::Center,
                                Some("bottom") => VAlign::Bottom,
                                _ => VAlign::Top,
                            }
                        }
                        "shd" => c.props.shading = shading(&tag).or_else(|| (styled && tag.attr("w:fill").as_deref() == Some("auto") && tag.attr("w:val").as_deref() != Some("nil")).then_some(0xFFFFFF)),
                        "textDirection" => {
                            c.props.text_dir = match tag.attr("w:val").as_deref() {
                                Some("btLr" | "btLrV") => TextDir::Up,
                                Some("tbRl" | "tbRlV" | "tbLrV") => TextDir::Down,
                                _ => TextDir::Horizontal,
                            };
                            c.props.flow = tag.attr("w:val").and_then(|v| apex_doc::TextFlow::from_word(&v)).filter(|f| !matches!(f, apex_doc::TextFlow::BtLr | apex_doc::TextFlow::TbRl | apex_doc::TextFlow::LrTb));
                        }
                        "tcBorders" if start => {
                            read_borders(r, &mut c.props.borders);
                            continue;
                        }
                        "tcMar" if start => {
                            if !read_cell_mar(r, &mut c.props.margins) {
                                return;
                            }
                            continue;
                        }
                        "tcPrChange" if start => {
                            r.skip();
                            continue;
                        }
                        _ => {}
                    }
                    if start {
                        depth += 1;
                    }
                }
                _ => {}
            }
        }
    }

    /// Tags the paragraphs read since the cell started.
    pub(crate) fn end_cell(&mut self, out: &mut Vec<Paragraph>) {
        let Some(t) = self.top() else { return };
        let Some(cs) = t.cell.take() else { return };
        if out.len() <= cs.start {
            out.push(Paragraph::new(""));
        }
        let row = t.row.unwrap_or(0);
        let c = t.col;
        if c >= apex_doc::MAX_COLS {
            // Past the column limit: the text joins the last cell.
            for p in &mut out[cs.start..] {
                tag_para(p, InCell::new(t.id, row, (apex_doc::MAX_COLS - 1) as u16));
            }
            return;
        }
        let mut props = cs.props;
        props.col_span = cs.span as u16;
        match cs.vmerge {
            Some(false) => match t.vmerge.get(&c).and_then(|&i| out.get_mut(i)).and_then(|p| p.cell.as_mut()).and_then(|x| x.cell.as_mut()) {
                Some(anchor) => {
                    anchor.row_span += 1;
                    props.covered = true;
                }
                None => {
                    t.vmerge.insert(c, cs.start);
                }
            },
            Some(true) => {
                t.vmerge.insert(c, cs.start);
            }
            None => {
                t.vmerge.remove(&c);
            }
        }
        let covered = props.covered;
        // A cell doesn't end with a nested table.
        if out.last().is_some_and(|p| p.cell.is_some()) {
            out.push(Paragraph::new(""));
        }
        for (k, p) in out[cs.start..].iter_mut().enumerate() {
            let mut tag = InCell::new(t.id, row, c as u16);
            if k == 0 {
                tag.cell = Some(Box::new(props.clone()));
            }
            tag_para(p, tag);
        }
        for k in 1..cs.span {
            let mut p = Paragraph::new("");
            let mut tag = InCell::new(t.id, row, (c + k) as u16);
            tag.cell = Some(Box::new(CellProps { covered: true, ..Default::default() }));
            p.cell = Some(tag);
            out.push(p);
            t.vmerge.remove(&(c + k));
        }
        if row == 0 && !covered {
            let w = cs.width.map_or(0.0, |w| w / cs.span as f32);
            if t.first_row_widths.len() < c + cs.span {
                t.first_row_widths.resize(c + cs.span, 0.0);
            }
            for k in 0..cs.span {
                t.first_row_widths[c + k] = w;
            }
        }
        t.col += cs.span;
    }

    pub(crate) fn end_row(&mut self) {
        if let Some(t) = self.top() {
            if t.row_header && !t.headers_done {
                t.props.header_rows += 1;
            } else {
                t.headers_done = true;
            }
        }
    }

    /// `sets` tells which of space before, after and line spacing a
    /// paragraph style (by its temporary number) sets; `sized`, whether
    /// Normal sets a font size.
    pub(crate) fn end_table(&mut self, out: &mut [Paragraph], styles: &HashMap<String, TblStyleDef>, sets: &dyn Fn(u16) -> [bool; 3], sized: bool) {
        if self.flat > 0 {
            self.flat -= 1;
            return;
        }
        let Some(mut t) = self.stack.pop() else { return };
        style_spacing(&t, out, styles, sets, sized);
        cell_margins(&mut t, styles);
        t.props.widths = if t.grid.iter().any(|w| *w > 0.0) { t.grid } else { t.first_row_widths };
        if let Some(s) = &t.style {
            resolve_style(s, styles, &mut t.props);
        }
        if let Some(tag) = out.get_mut(t.start).and_then(|p| p.cell.as_mut()).filter(|c| c.table == t.id) {
            tag.table_props = Some(Box::new(t.props));
        }
    }
}

/// The style id a table follows: its own, or the default table style.
fn style_of(t: &Tbl, styles: &HashMap<String, TblStyleDef>) -> Option<String> {
    t.style.clone().or_else(|| {
        let mut defaults: Vec<&String> = styles.iter().filter(|(_, d)| d.default).map(|(k, _)| k).collect();
        defaults.sort();
        defaults.first().map(|k| k.to_string())
    })
}

/// A table's cell margins: its own over its style's (and the styles that
/// one is based on).
fn cell_margins(t: &mut Tbl, styles: &HashMap<String, TblStyleDef>) {
    let mut m = t.mar;
    let mut id = style_of(t, styles);
    for _ in 0..12 {
        let Some(d) = id.take().and_then(|i| styles.get(&i)) else { break };
        for k in 0..4 {
            m[k] = m[k].or(d.margins[k]);
        }
        id = d.based_on.clone();
    }
    let base = t.props.margins();
    let m: [f32; 4] = std::array::from_fn(|k| m[k].unwrap_or(base[k]));
    if t.mar.iter().any(Option::is_some) || m != base {
        t.props.cell_margins = Some(m);
    }
}

/// Space around the paragraphs of a table's cells from its style (or the
/// default table style), where neither the paragraph nor its style (other
/// than Normal) sets it; the same for the size of their text (which Normal
/// setting one keeps).
fn style_spacing(t: &Tbl, out: &mut [Paragraph], styles: &HashMap<String, TblStyleDef>, sets: &dyn Fn(u16) -> [bool; 3], sized: bool) {
    let mut id = style_of(t, styles);
    let (mut spacing, mut line, mut size) = ([None, None], None, None);
    for _ in 0..12 {
        let Some(d) = id.take().and_then(|i| styles.get(&i)) else { break };
        for k in 0..2 {
            spacing[k] = spacing[k].or(d.spacing[k]);
        }
        line = line.or(d.line);
        size = size.or(d.size);
        id = d.based_on.clone();
    }
    let size = size.filter(|_| !sized);
    if spacing == [None, None] && line.is_none() && size.is_none() {
        return;
    }
    for p in out.iter_mut().skip(t.start) {
        if !p.cell.as_ref().is_some_and(|c| c.table == t.id) {
            continue;
        }
        let own = if p.style.custom != 0 { sets(p.style.custom) } else { [p.style.kind != apex_doc::ParaKind::Normal; 3] };
        if let Some(sz) = size.filter(|_| p.style.custom == 0 && p.style.kind == apex_doc::ParaKind::Normal) {
            for r in &mut p.runs {
                r.attrs.size = r.attrs.size.or(Some(sz));
            }
        }
        if p.style.space_before.is_none() && !own[0] {
            p.style.space_before = spacing[0];
        }
        if p.style.space_after.is_none() && !own[1] {
            p.style.space_after = spacing[1];
        }
        if let Some(l) = line.filter(|&l| l > 0.0)
            && !own[2]
            && p.style.line_rule == apex_doc::LineRule::Multiple
            && p.style.line_spacing == 0.0
        {
            p.style.line_spacing = l;
        }
    }
}

/// Tags a paragraph as in a cell; a tag it already has (of a table in
/// the cell) nests in the new one.
fn tag_para(p: &mut Paragraph, mut tag: InCell) {
    tag.nested = p.cell.take().map(Box::new);
    p.cell = Some(tag);
}

fn on_str(v: &str) -> bool {
    !matches!(v, "0" | "false" | "off")
}

// ---------------------------------------------------------------------------
// Writing

fn border_xml(out: &mut String, name: &str, b: Border) {
    if b.is_none() {
        let _ = write!(out, r#"<w:{name} w:val="nil"/>"#);
    } else {
        let sz = (b.width * 8.0).round().clamp(2.0, 96.0) as u32;
        let _ = write!(out, r#"<w:{name} w:val="single" w:sz="{sz}" w:space="0" w:color="{:06X}"/>"#, b.color & 0xFF_FFFF);
    }
}

fn borders_xml(out: &mut String, tag: &str, b: &Borders, inside: bool) {
    if b.is_empty() {
        return;
    }
    let _ = write!(out, "<w:{tag}>");
    for (name, v) in [("top", b.top), ("left", b.left), ("bottom", b.bottom), ("right", b.right)] {
        if let Some(v) = v {
            border_xml(out, name, v);
        }
    }
    if inside {
        for (name, v) in [("insideH", b.inside_h), ("insideV", b.inside_v)] {
            if let Some(v) = v {
                border_xml(out, name, v);
            }
        }
    }
    let _ = write!(out, "</w:{tag}>");
}

fn tw(v: f32) -> i32 {
    (v * TWIP).round() as i32
}

/// An open table while writing: its map, current row and cell.
struct Level {
    map: TableMap,
    id: u32,
    row: Option<usize>,
    cell: Option<(usize, usize)>,
    /// The current cell is written (not merged away).
    written: bool,
    tc_open: bool,
}

/// Writes the table markup around the paragraphs of the body; tables
/// nested in cells open inside them.
pub(crate) struct TblWriter<'a> {
    doc: &'a Document,
    maps: HashMap<(usize, usize), TableMap>,
    /// Open tables, outermost first.
    levels: Vec<Level>,
}

/// A paragraph's tag at nesting level `d`.
fn tag_at(p: &Paragraph, d: usize) -> Option<&InCell> {
    p.cell.as_ref()?.at(d)
}

impl<'a> TblWriter<'a> {
    pub(crate) fn new(doc: &'a Document) -> Self {
        let maps = doc.all_tables().into_iter().map(|m| ((m.start, m.depth), m)).collect();
        TblWriter { doc, maps, levels: Vec::new() }
    }

    /// Closes the innermost open table.
    fn close_level(&mut self, out: &mut String) {
        if let Some(l) = self.levels.pop() {
            if l.tc_open {
                out.push_str("</w:tc>");
            }
            if l.row.is_some() {
                out.push_str("</w:tr>");
            }
            out.push_str("</w:tbl>");
        }
    }

    /// Closes every open table.
    pub(crate) fn close(&mut self, out: &mut String) {
        while !self.levels.is_empty() {
            self.close_level(out);
        }
    }

    /// Emits the markup needed before paragraph `idx`. False when the
    /// paragraph belongs to a cell hidden by a merge (and isn't written).
    pub(crate) fn before(&mut self, out: &mut String, idx: usize) -> bool {
        let paras = &self.doc.paragraphs;
        let p = &paras[idx];
        // Tables the paragraph isn't in close.
        while let Some(l) = self.levels.last() {
            let d = self.levels.len() - 1;
            if tag_at(p, d).is_some_and(|t| t.table == l.id) {
                break;
            }
            self.close_level(out);
        }
        let depth = p.cell.as_ref().map_or(0, InCell::depth);
        for d in 0..depth {
            if d >= self.levels.len() {
                let Some(m) = self.maps.remove(&(idx, d)).or_else(|| TableMap::build_at(paras, idx, d)) else { return true };
                self.open_table(out, &m);
                let id = tag_at(p, d).map_or(0, |t| t.table);
                self.levels.push(Level { map: m, id, row: None, cell: None, written: false, tc_open: false });
            }
            if !self.cell(out, d, idx) {
                return false;
            }
        }
        true
    }

    /// Opens the cell of table level `d` holding paragraph `idx`, if not
    /// open. False for a cell hidden by a merge.
    fn cell(&mut self, out: &mut String, d: usize, idx: usize) -> bool {
        let paras = &self.doc.paragraphs;
        let l = &mut self.levels[d];
        let m = &l.map;
        let (r, c) = m.pos_of(paras, idx);
        if l.cell == Some((r, c)) {
            return l.written;
        }
        if l.tc_open {
            out.push_str("</w:tc>");
            l.tc_open = false;
        }
        if l.row != Some(r) {
            if l.row.is_some() {
                out.push_str("</w:tr>");
            }
            let props = m.props(paras);
            out.push_str("<w:tr>");
            let mut tr = String::new();
            let rule = props.map(|p| p.row_rule(r)).unwrap_or_default();
            if rule.cant_split {
                tr.push_str("<w:cantSplit/>");
            }
            if let Some(h) = props.and_then(|p| p.row_heights.get(r)).filter(|h| **h > 0.0) {
                let _ = write!(tr, r#"<w:trHeight w:val="{}" w:hRule="{}"/>"#, tw(*h), if rule.exact { "exact" } else { "atLeast" });
            }
            if props.is_some_and(|p| (r as u32) < p.header_rows) {
                tr.push_str("<w:tblHeader/>");
            }
            if !tr.is_empty() {
                let _ = write!(out, "<w:trPr>{tr}</w:trPr>");
            }
            l.row = Some(r);
        }
        l.cell = Some((r, c));
        let (ar, ac) = m.anchor_of(r, c);
        let (rs, cs) = m.span_of(ar, ac);
        let widths = m.props(paras).map(|p| p.widths.as_slice()).unwrap_or(&[]);
        let width: f32 = (ac..ac + cs).filter_map(|k| widths.get(k)).sum();
        let vmerge = if (ar, ac) == (r, c) {
            (rs > 1).then_some(r#"<w:vMerge w:val="restart"/>"#)
        } else if c == ac && r > ar {
            Some("<w:vMerge/>")
        } else {
            l.written = false;
            return false;
        };
        let _ = write!(out, r#"<w:tc><w:tcPr><w:tcW w:w="{}" w:type="dxa"/>"#, tw(width));
        if cs > 1 {
            let _ = write!(out, r#"<w:gridSpan w:val="{cs}"/>"#);
        }
        if let Some(v) = vmerge {
            out.push_str(v);
        }
        if let Some(p) = m.cell_props(paras, ar, ac) {
            borders_xml(out, "tcBorders", &p.borders, false);
            if let Some(s) = p.shading {
                let _ = write!(out, r#"<w:shd w:val="clear" w:color="auto" w:fill="{:06X}"/>"#, s & 0xFF_FFFF);
            }
            // (Before the text direction, as the schema has it.)
            if p.margins.iter().any(Option::is_some) {
                out.push_str("<w:tcMar>");
                for (v, side) in p.margins.iter().zip(["top", "left", "bottom", "right"]) {
                    if let Some(v) = v {
                        let _ = write!(out, r#"<w:{side} w:w="{}" w:type="dxa"/>"#, tw(*v));
                    }
                }
                out.push_str("</w:tcMar>");
            }
            match (apex_doc::TextFlow::of_cell(p), p.text_dir) {
                (Some(f), _) => {
                    let _ = write!(out, r#"<w:textDirection w:val="{}"/>"#, f.word());
                }
                (None, TextDir::Horizontal) => {}
                (None, TextDir::Up) => out.push_str(r#"<w:textDirection w:val="btLr"/>"#),
                (None, TextDir::Down) => out.push_str(r#"<w:textDirection w:val="tbRl"/>"#),
            }
            match p.valign {
                VAlign::Top => {}
                VAlign::Center => out.push_str(r#"<w:vAlign w:val="center"/>"#),
                VAlign::Bottom => out.push_str(r#"<w:vAlign w:val="bottom"/>"#),
            }
            if p.hide_mark {
                out.push_str("<w:hideMark/>");
            }
        }
        out.push_str("</w:tcPr>");
        l.written = true;
        l.tc_open = true;
        true
    }

    fn open_table(&self, out: &mut String, m: &TableMap) {
        let default = TableProps::default();
        let p = m.props(&self.doc.paragraphs).unwrap_or(&default);
        out.push_str("<w:tbl><w:tblPr>");
        let style = p.style_def(&self.doc.styles).map_or("TableNormal", |s| &*s.id);
        let _ = write!(out, r#"<w:tblStyle w:val="{}"/><w:tblW w:w="{}" w:type="dxa"/>"#, esc(style), tw(p.width()));
        match p.align {
            Align::Center => out.push_str(r#"<w:jc w:val="center"/>"#),
            Align::Right => out.push_str(r#"<w:jc w:val="right"/>"#),
            _ => {}
        }
        if p.indent != 0.0 {
            let _ = write!(out, r#"<w:tblInd w:w="{}" w:type="dxa"/>"#, tw(p.indent));
        }
        borders_xml(out, "tblBorders", &p.borders, true);
        let l = p.look;
        let val = u32::from(l.first_row) * 0x20 + u32::from(l.last_row) * 0x40 + u32::from(l.first_col) * 0x80 + u32::from(l.last_col) * 0x100 + u32::from(!l.banded_rows) * 0x200 + u32::from(!l.banded_cols) * 0x400;
        let b = |v: bool| u8::from(v);
        let mar = match p.cell_margins {
            Some(_) => {
                let m = p.margins();
                format!(r#"<w:tblCellMar><w:top w:w="{}" w:type="dxa"/><w:left w:w="{}" w:type="dxa"/><w:bottom w:w="{}" w:type="dxa"/><w:right w:w="{}" w:type="dxa"/></w:tblCellMar>"#, tw(m[0]), tw(m[1]), tw(m[2]), tw(m[3]))
            }
            None => String::new(),
        };
        let _ = write!(
            out,
            r#"<w:tblLayout w:type="fixed"/>{mar}<w:tblLook w:val="{val:04X}" w:firstRow="{}" w:lastRow="{}" w:firstColumn="{}" w:lastColumn="{}" w:noHBand="{}" w:noVBand="{}"/></w:tblPr><w:tblGrid>"#,
            b(l.first_row),
            b(l.last_row),
            b(l.first_col),
            b(l.last_col),
            b(!l.banded_rows),
            b(!l.banded_cols)
        );
        for w in &p.widths {
            let _ = write!(out, r#"<w:gridCol w:w="{}"/>"#, tw(*w));
        }
        out.push_str("</w:tblGrid>");
    }
}

/// Table style definitions for styles.xml: the plain table and our styles.
pub(crate) fn styles_xml(doc: &Document) -> String {
    let mut used: Vec<&TableStyle> = Vec::new();
    for m in doc.all_tables() {
        if let Some(s) = m.props(&doc.paragraphs).and_then(|p| p.style_def(&doc.styles)) {
            if !used.iter().any(|u| u.id == s.id) {
                used.push(s);
            }
        }
    }
    // No space after in cells (see `Paragraph::cell`); their line spacing
    // is the document's (Normal's or the defaults'), as without a table.
    let spacing = r#"<w:pPr><w:spacing w:after="0"/></w:pPr>"#;
    let mut s = format!(
        r#"<w:style w:type="table" w:default="1" w:styleId="TableNormal"><w:name w:val="Normal Table"/><w:uiPriority w:val="99"/><w:semiHidden/><w:unhideWhenUsed/>{spacing}<w:tblPr><w:tblInd w:w="0" w:type="dxa"/><w:tblCellMar><w:top w:w="0" w:type="dxa"/><w:left w:w="108" w:type="dxa"/><w:bottom w:w="0" w:type="dxa"/><w:right w:w="108" w:type="dxa"/></w:tblCellMar></w:tblPr></w:style>"#
    );
    for st in used {
        let _ = write!(s, r#"<w:style w:type="table" w:styleId="{}"><w:name w:val="{}"/><w:basedOn w:val="TableNormal"/><w:uiPriority w:val="39"/>{spacing}<w:tblPr><w:tblStyleRowBandSize w:val="1"/><w:tblStyleColBandSize w:val="1"/>"#, esc(&st.id), esc(&st.name));
        borders_xml(&mut s, "tblBorders", &st.borders, true);
        s.push_str("</w:tblPr>");
        let fill = |s: &mut String, c: Option<u32>| {
            if let Some(c) = c {
                let _ = write!(s, r#"<w:tcPr><w:shd w:val="clear" w:color="auto" w:fill="{c:06X}"/></w:tcPr>"#);
            }
        };
        if st.header_bold || st.header_fill.is_some() || st.header_line.is_some() {
            s.push_str(r#"<w:tblStylePr w:type="firstRow"><w:rPr>"#);
            if st.header_bold {
                s.push_str("<w:b/><w:bCs/>");
            }
            if let Some(c) = st.header_color {
                let _ = write!(s, r#"<w:color w:val="{c:06X}"/>"#);
            }
            s.push_str("</w:rPr>");
            if st.header_fill.is_some() || st.header_line.is_some() {
                s.push_str("<w:tcPr>");
                if let Some(l) = st.header_line {
                    s.push_str("<w:tcBorders>");
                    border_xml(&mut s, "bottom", l);
                    s.push_str("</w:tcBorders>");
                }
                if let Some(c) = st.header_fill {
                    let _ = write!(s, r#"<w:shd w:val="clear" w:color="auto" w:fill="{c:06X}"/>"#);
                }
                s.push_str("</w:tcPr>");
            }
            s.push_str("</w:tblStylePr>");
        }
        if st.first_col_bold {
            s.push_str(r#"<w:tblStylePr w:type="lastRow"><w:rPr><w:b/><w:bCs/></w:rPr></w:tblStylePr><w:tblStylePr w:type="firstCol"><w:rPr><w:b/><w:bCs/></w:rPr></w:tblStylePr>"#);
        }
        if st.band_fill.is_some() {
            s.push_str(r#"<w:tblStylePr w:type="band1Horz">"#);
            fill(&mut s, st.band_fill);
            s.push_str("</w:tblStylePr>");
        }
        if st.band2_fill.is_some() {
            s.push_str(r#"<w:tblStylePr w:type="band2Horz">"#);
            fill(&mut s, st.band2_fill);
            s.push_str("</w:tblStylePr>");
        }
        s.push_str("</w:style>");
    }
    s
}
