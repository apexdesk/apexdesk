//! Tables in .odt files (`table:table`): cell, column and table styles,
//! reading into tagged paragraphs and writing the table markup around them.

use std::collections::HashMap;
use std::fmt::Write as _;

use apex_doc::{Align, Border, Borders, CellProps, Document, InCell, MAX_COLS, Paragraph, RowRule, TableLook, TableMap, TableProps, TextDir, VAlign, table_style};

use crate::xml::{Tag, esc};
use crate::{parse_hex_color, parse_length_pt};

#[derive(Default, Clone)]
struct CellStyle {
    fill: Option<u32>,
    borders: Borders,
    valign: VAlign,
    /// Padding: top, left, bottom, right.
    pad: Option<[f32; 4]>,
    text_dir: TextDir,
    flow: Option<apex_doc::TextFlow>,
}

#[derive(Default, Clone)]
struct TblStyle {
    width: Option<f32>,
    align: Align,
    indent: f32,
}

enum Cur {
    Table(String),
    Column(String),
    Row(String),
    Cell(String),
}

/// Table, column, row and cell styles.
#[derive(Default)]
pub(crate) struct TableStyles {
    tables: HashMap<String, TblStyle>,
    cols: HashMap<String, f32>,
    /// Row height and rule.
    rows: HashMap<String, (f32, RowRule)>,
    cells: HashMap<String, CellStyle>,
    cur: Option<Cur>,
}

/// An ODF border ("0.5pt solid #000000", "none").
fn border(v: &str) -> Border {
    let mut b = Border::THIN;
    for tok in v.split_whitespace() {
        if matches!(tok, "none" | "hidden") {
            return Border::NONE;
        } else if let Some(c) = tok.strip_prefix('#').and_then(|_| parse_hex_color(tok)) {
            b.color = c;
        } else if let Some(w) = parse_length_pt(tok) {
            b.width = w.clamp(0.25, 6.0);
        }
    }
    b
}

impl TableStyles {
    /// Starts a `style:style` of a table family; false for other families.
    pub(crate) fn start(&mut self, t: &Tag, is_start: bool) -> bool {
        let name = t.attr("style:name").unwrap_or_default().into_owned();
        let cur = match t.attr("style:family").as_deref() {
            Some("table") => Cur::Table(name),
            Some("table-column") => Cur::Column(name),
            Some("table-row") => Cur::Row(name),
            Some("table-cell") => Cur::Cell(name),
            _ => return false,
        };
        if is_start {
            self.cur = Some(cur);
        }
        true
    }

    pub(crate) fn end(&mut self) {
        self.cur = None;
    }

    /// Reads a `style:table*-properties` element of the current style.
    pub(crate) fn props(&mut self, t: &Tag) {
        let len = |k: &str| t.attr(k).and_then(|v| parse_length_pt(&v)).filter(|v| v.is_finite());
        match &self.cur {
            Some(Cur::Table(n)) if t.name == "style:table-properties" => {
                let s = TblStyle {
                    width: len("style:width"),
                    align: match t.attr("table:align").as_deref() {
                        Some("center") => Align::Center,
                        Some("right") => Align::Right,
                        _ => Align::Left,
                    },
                    indent: len("fo:margin-left").unwrap_or(0.0).clamp(-500.0, 500.0),
                };
                self.tables.insert(n.clone(), s);
            }
            Some(Cur::Column(n)) if t.name == "style:table-column-properties" => {
                if let Some(w) = len("style:column-width") {
                    self.cols.insert(n.clone(), w.clamp(0.0, 2000.0));
                }
            }
            Some(Cur::Row(n)) if t.name == "style:table-row-properties" => {
                // A fixed height is exact; rows kept together don't break.
                let min = len("style:min-row-height");
                let h = min.or_else(|| len("style:row-height")).unwrap_or(0.0).clamp(0.0, 1000.0);
                let rule = RowRule { cant_split: t.attr("fo:keep-together").as_deref() == Some("always"), exact: min.is_none() && h > 0.0 };
                if h > 0.0 || rule.cant_split {
                    self.rows.insert(n.clone(), (h, rule));
                }
            }
            Some(Cur::Cell(n)) if t.name == "style:table-cell-properties" => {
                let mut s = CellStyle::default();
                if let Some(c) = t.attr("fo:background-color") {
                    s.fill = parse_hex_color(&c);
                }
                let b = &mut s.borders;
                if let Some(v) = t.attr("fo:border") {
                    let v = Some(border(&v));
                    (b.top, b.bottom, b.left, b.right) = (v, v, v, v);
                }
                for (k, side) in [("fo:border-top", &mut b.top), ("fo:border-bottom", &mut b.bottom), ("fo:border-left", &mut b.left), ("fo:border-right", &mut b.right)] {
                    if let Some(v) = t.attr(k) {
                        *side = Some(border(&v));
                    }
                }
                s.valign = match t.attr("style:vertical-align").as_deref() {
                    Some("middle") => VAlign::Center,
                    Some("bottom") => VAlign::Bottom,
                    _ => VAlign::Top,
                };
                let all = len("fo:padding");
                let side = |k: &str| len(k).or(all);
                if all.is_some() || ["fo:padding-top", "fo:padding-left", "fo:padding-bottom", "fo:padding-right"].iter().any(|k| t.attr(k).is_some()) {
                    s.pad = Some([side("fo:padding-top").unwrap_or(0.0), side("fo:padding-left").unwrap_or(0.0), side("fo:padding-bottom").unwrap_or(0.0), side("fo:padding-right").unwrap_or(0.0)].map(|v| v.clamp(0.0, 200.0)));
                }
                s.text_dir = match (t.attr("style:writing-mode").as_deref(), t.attr("style:rotation-angle").as_deref()) {
                    (Some("bt-lr"), _) | (_, Some("90" | "90deg")) => TextDir::Up,
                    (Some("tb-rl" | "tb" | "tb-lr"), _) | (_, Some("270" | "270deg")) => TextDir::Down,
                    _ => TextDir::Horizontal,
                };
                s.flow = t.attr("style:writing-mode").filter(|v| v == "tb-lr").and_then(|v| apex_doc::TextFlow::from_odf(&v));
                self.cells.insert(n.clone(), s);
            }
            _ => {}
        }
    }
}

/// A table being read. Nested tables flatten into the outer cell (their
/// paragraphs are retagged when the outer cell ends).
pub(crate) struct TableRead {
    id: u32,
    start: usize,
    props: TableProps,
    template: bool,
    row: Option<u32>,
    col: usize,
    in_header: bool,
    widths: Vec<f32>,
    width: Option<f32>,
    /// Columns of the row still covered by the last spanned cell, for
    /// files that leave out the covered cells (LibreOffice reads them so).
    span_left: usize,
}

fn attr_n(t: &Tag, k: &str, max: usize) -> usize {
    t.attr(k).and_then(|v| v.parse::<usize>().ok()).unwrap_or(1).clamp(1, max)
}

impl TableRead {
    pub(crate) fn new(id: u32, start: usize, t: &Tag, styles: &TableStyles) -> TableRead {
        let mut props = TableProps::default();
        let st = t.attr("table:style-name").and_then(|n| styles.tables.get(n.as_ref())).cloned().unwrap_or_default();
        props.align = st.align;
        props.indent = st.indent;
        let template = t.attr("table:template-name").and_then(|n| table_style(&n)).map(|s| s.id.clone());
        if let Some(id) = &template {
            props.style = Some(id.as_ref().into());
            let on = |k: &str| t.attr(k).as_deref() == Some("true");
            props.look = TableLook {
                first_row: on("table:use-first-row-styles"),
                last_row: on("table:use-last-row-styles"),
                first_col: on("table:use-first-column-styles"),
                last_col: on("table:use-last-column-styles"),
                banded_rows: on("table:use-banding-rows-styles"),
                banded_cols: on("table:use-banding-columns-styles"),
            };
        }
        TableRead { id, start, props, template: template.is_some(), row: None, col: 0, in_header: false, widths: Vec::new(), width: st.width, span_left: 0 }
    }

    pub(crate) fn column(&mut self, t: &Tag, styles: &TableStyles) {
        let n = attr_n(t, "table:number-columns-repeated", MAX_COLS);
        let w = t.attr("table:style-name").and_then(|s| styles.cols.get(s.as_ref()).copied()).unwrap_or(0.0);
        for _ in 0..n {
            if self.widths.len() < MAX_COLS {
                self.widths.push(w);
            }
        }
    }

    pub(crate) fn header(&mut self, on: bool) {
        self.in_header = on;
    }

    pub(crate) fn start_row(&mut self, t: &Tag, styles: &TableStyles) {
        self.span_left = 0;
        let r = self.row.map_or(0, |r| r + 1);
        self.row = Some(r);
        self.col = 0;
        if self.in_header && self.props.header_rows == r {
            self.props.header_rows += 1;
        }
        if let Some((h, rule)) = t.attr("table:style-name").and_then(|s| styles.rows.get(s.as_ref()).copied()) {
            if h > 0.0 {
                let hs = &mut self.props.row_heights;
                hs.resize(hs.len().max(r as usize + 1), 0.0);
                hs[r as usize] = h;
            }
            if rule != RowRule::default() {
                let rr = &mut self.props.row_rules;
                rr.resize(rr.len().max(r as usize + 1), RowRule::default());
                rr[r as usize] = rule;
            }
        }
    }

    /// Tags `out[start..]` as the next cell(s) of the row; a cell repeated
    /// with `number-columns-repeated` gets copies of its paragraphs.
    /// `copy_left`: bytes of text repeated cells may still copy, for the
    /// whole document (repeats nest, each level multiplying them).
    pub(crate) fn cell(&mut self, t: &Tag, covered: bool, start: usize, out: &mut Vec<Paragraph>, styles: &TableStyles, copy_left: &mut usize) {
        // A cell has a paragraph and doesn't end with a nested table.
        if out.len() <= start || out.last().is_some_and(|p| p.cell.is_some()) {
            out.push(Paragraph::new(""));
        }
        let row = self.row.unwrap_or(0);
        if self.row.is_none() {
            self.row = Some(0);
        }
        if covered {
            self.span_left = self.span_left.saturating_sub(attr_n(t, "table:number-columns-repeated", MAX_COLS));
        } else if self.span_left > 0 {
            // A cell where a covered one was due: the missing ones first.
            let n = self.span_left;
            let fill: Vec<Paragraph> = (0..n).map(|k| self.covered_para(row, k)).collect();
            out.splice(start..start, fill);
            self.col += n;
            self.span_left = 0;
            return self.cell(t, covered, start + n, out, styles, copy_left);
        }
        let mut props = CellProps { covered, ..Default::default() };
        if !covered {
            props.col_span = attr_n(t, "table:number-columns-spanned", MAX_COLS) as u16;
            props.row_span = attr_n(t, "table:number-rows-spanned", 100_000) as u32;
            if let Some(s) = t.attr("table:style-name").and_then(|n| styles.cells.get(n.as_ref())) {
                props.shading = s.fill;
                props.borders = s.borders;
                props.valign = s.valign;
                props.text_dir = s.text_dir;
                props.flow = s.flow;
                // The table's cell margins come from its first cell.
                if row == 0 && self.col == 0 && s.pad.is_some() && s.pad != Some(TableProps::default().margins()) {
                    self.props.cell_margins = s.pad;
                }
            }
        }
        let repeat = attr_n(t, "table:number-columns-repeated", MAX_COLS);
        if !covered {
            self.span_left = props.col_span as usize - 1;
        }
        let proto: Vec<Paragraph> = if repeat > 1 { out[start..].to_vec() } else { Vec::new() };
        let proto_bytes = proto.iter().map(|p| p.text.len() + 64).sum::<usize>();
        for k in 0..repeat {
            let col = self.col.min(MAX_COLS - 1);
            if k > 0 {
                // Repeated cells holding repeated tables multiply: the
                // copies stop at the paragraphs a document may have.
                if out.len() + proto.len() > apex_doc::MAX_PARAGRAPHS || *copy_left < proto_bytes {
                    break;
                }
                *copy_left -= proto_bytes;
                out.extend(proto.iter().cloned());
            }
            let from = if k == 0 { start } else { out.len() - proto.len() };
            for (i, p) in out[from..].iter_mut().enumerate() {
                let mut tag = InCell::new(self.id, row, col as u16);
                if i == 0 {
                    tag.cell = Some(Box::new(props.clone()));
                }
                // Tables in the cell nest in it.
                tag.nested = p.cell.take().map(Box::new);
                p.cell = Some(tag);
            }
            self.col += 1;
        }
    }

    /// A covered cell the file left out, `k` columns on.
    fn covered_para(&mut self, row: u32, k: usize) -> Paragraph {
        let mut p = Paragraph::new("");
        let mut tag = InCell::new(self.id, row, (self.col + k).min(MAX_COLS - 1) as u16);
        tag.cell = Some(Box::new(CellProps { covered: true, ..Default::default() }));
        p.cell = Some(tag);
        p
    }

    /// The row's end: covered cells the file left out after a spanned one.
    pub(crate) fn end_row(&mut self, out: &mut Vec<Paragraph>) {
        let row = self.row.unwrap_or(0);
        for k in 0..std::mem::take(&mut self.span_left) {
            let p = self.covered_para(row, k);
            out.push(p);
        }
    }

    /// Stores the table properties on its first paragraph.
    pub(crate) fn finish(mut self, out: &mut [Paragraph]) {
        let (rows, cols) = match TableMap::build(out, self.start) {
            Some(m) => (m.rows, m.cols),
            None => return,
        };
        self.props.widths = match self.width {
            // Columns without widths share the table's.
            Some(w) if self.widths.iter().all(|c| *c <= 0.0) => vec![w / cols as f32; cols],
            _ => self.widths,
        };
        if self.template {
            // Cell formatting that only repeats the table style's look.
            if let Some(m) = TableMap::build(out, self.start) {
                for r in 0..rows {
                    for c in 0..cols {
                        let (a, _) = m.cell(r, c);
                        let Some(cp) = out.get_mut(a).and_then(|p| p.cell.as_mut()).and_then(|x| x.cell.as_mut()) else { continue };
                        let bare = CellProps { col_span: cp.col_span, row_span: cp.row_span, ..Default::default() };
                        let look = self.props.cell_look(&Default::default(), &bare, r, c, rows, cols);
                        if cp.shading == look.fill {
                            cp.shading = None;
                        }
                        for (side, l) in [(&mut cp.borders.top, look.top), (&mut cp.borders.bottom, look.bottom), (&mut cp.borders.left, look.left), (&mut cp.borders.right, look.right)] {
                            if side.is_some_and(|b| b == l || b.is_none() && l.is_none()) {
                                *side = None;
                            }
                        }
                    }
                }
            }
        }
        if let Some(tag) = out.get_mut(self.start).and_then(|p| p.cell.as_mut()) {
            tag.table_props = Some(Box::new(self.props));
        }
    }
}

// ---------------------------------------------------------------------------
// Writing

fn pt(v: f32) -> String {
    format!("{}pt", (v * 100.0).round() / 100.0)
}

fn border_attr(b: Border) -> String {
    if b.is_none() { "none".into() } else { format!("{} solid #{:06x}", pt(b.width), b.color & 0xFF_FFFF) }
}

/// Writes tables around the body paragraphs, adding their automatic styles.
pub(crate) struct TblWriter<'a> {
    doc: &'a Document,
    maps: HashMap<(usize, usize), TableMap>,
    /// Open tables, outermost first.
    levels: Vec<Level>,
    n: usize,
    cell_styles: HashMap<String, String>,
}

/// An open table while writing.
struct Level {
    map: TableMap,
    id: u32,
    /// Number of the table (for style names).
    n: usize,
    row: Option<usize>,
    header_open: bool,
    cell: Option<(usize, usize)>,
    written: bool,
    tc_open: bool,
}

fn tag_at(p: &Paragraph, d: usize) -> Option<&InCell> {
    p.cell.as_ref()?.at(d)
}

impl<'a> TblWriter<'a> {
    pub(crate) fn new(doc: &'a Document) -> Self {
        let maps = doc.all_tables().into_iter().map(|m| ((m.start, m.depth), m)).collect();
        TblWriter { doc, maps, levels: Vec::new(), n: 0, cell_styles: HashMap::new() }
    }

    fn close_level(&mut self, body: &mut String) {
        if let Some(l) = self.levels.pop() {
            if l.tc_open {
                body.push_str("</table:table-cell>");
            }
            if l.row.is_some() {
                body.push_str("</table:table-row>");
            }
            if l.header_open {
                body.push_str("</table:table-header-rows>");
            }
            body.push_str("</table:table>");
        }
    }

    pub(crate) fn close(&mut self, body: &mut String) {
        while !self.levels.is_empty() {
            self.close_level(body);
        }
    }

    /// Emits the markup before paragraph `idx`; false when it belongs to a
    /// covered cell and isn't written.
    pub(crate) fn before(&mut self, body: &mut String, auto: &mut String, idx: usize) -> bool {
        let paras = &self.doc.paragraphs;
        let p = &paras[idx];
        while let Some(l) = self.levels.last() {
            let d = self.levels.len() - 1;
            if tag_at(p, d).is_some_and(|t| t.table == l.id) {
                break;
            }
            self.close_level(body);
        }
        let depth = p.cell.as_ref().map_or(0, InCell::depth);
        for d in 0..depth {
            if d >= self.levels.len() {
                let Some(m) = self.maps.remove(&(idx, d)).or_else(|| TableMap::build_at(paras, idx, d)) else { return true };
                let (n, header_open) = self.open_table(body, auto, &m);
                let id = tag_at(p, d).map_or(0, |t| t.table);
                self.levels.push(Level { map: m, id, n, row: None, header_open, cell: None, written: false, tc_open: false });
            }
            if !self.cell(body, auto, d, idx) {
                return false;
            }
        }
        true
    }

    /// Opens the cell of level `d` holding paragraph `idx` if not open;
    /// false for a covered cell.
    fn cell(&mut self, body: &mut String, auto: &mut String, d: usize, idx: usize) -> bool {
        let paras = &self.doc.paragraphs;
        let l = &mut self.levels[d];
        let m = &l.map;
        let (r, c) = m.pos_of(paras, idx);
        if l.cell == Some((r, c)) {
            return l.written;
        }
        let default = TableProps::default();
        let props = m.props(paras).unwrap_or(&default);
        if l.tc_open {
            body.push_str("</table:table-cell>");
            l.tc_open = false;
        }
        if l.row != Some(r) {
            if l.row.is_some() {
                body.push_str("</table:table-row>");
            }
            if l.header_open && r as u32 >= props.header_rows {
                body.push_str("</table:table-header-rows>");
                l.header_open = false;
            }
            let rule = props.row_rule(r);
            let h = props.row_heights.get(r).copied().filter(|h| *h > 0.0);
            if h.is_some() || rule.cant_split {
                let name = format!("Tbl{}.R{}", l.n, r + 1);
                let mut rp = String::new();
                if let Some(h) = h {
                    let _ = write!(rp, r#" style:{}row-height="{}""#, if rule.exact { "" } else { "min-" }, pt(h));
                }
                if rule.cant_split {
                    rp.push_str(r#" fo:keep-together="always""#);
                }
                let _ = write!(auto, r#"<style:style style:name="{name}" style:family="table-row"><style:table-row-properties{rp}/></style:style>"#);
                let _ = write!(body, r#"<table:table-row table:style-name="{name}">"#);
            } else {
                body.push_str("<table:table-row>");
            }
            l.row = Some(r);
        }
        l.cell = Some((r, c));
        if !m.is_anchor(r, c) {
            body.push_str("<table:covered-table-cell/>");
            l.written = false;
            return false;
        }
        let (rs, cs) = m.span_of(r, c);
        let cp = m.cell_props(paras, r, c).cloned().unwrap_or_default();
        let look = props.cell_look(&self.doc.styles, &cp, r, c, m.rows, m.cols);
        let [top, left, bottom, right] = props.margins();
        let mut sp = format!(r#" fo:padding-top="{}" fo:padding-bottom="{}" fo:padding-left="{}" fo:padding-right="{}""#, pt(top), pt(bottom), pt(left), pt(right));
        match (apex_doc::TextFlow::of_cell(&cp), cp.text_dir) {
            (Some(f), _) => {
                let _ = write!(sp, r#" style:writing-mode="{}""#, f.odf());
            }
            (None, TextDir::Horizontal) => {}
            (None, TextDir::Up) => sp.push_str(r#" style:writing-mode="bt-lr""#),
            (None, TextDir::Down) => sp.push_str(r#" style:writing-mode="tb-rl""#),
        }
        if let Some(f) = look.fill {
            let _ = write!(sp, r##" fo:background-color="#{:06x}""##, f & 0xFF_FFFF);
        }
        for (k, b) in [("top", look.top), ("bottom", look.bottom), ("left", look.left), ("right", look.right)] {
            let _ = write!(sp, r#" fo:border-{k}="{}""#, border_attr(b));
        }
        match cp.valign {
            VAlign::Top => {}
            VAlign::Center => sp.push_str(r#" style:vertical-align="middle""#),
            VAlign::Bottom => sp.push_str(r#" style:vertical-align="bottom""#),
        }
        let mut tp = String::new();
        if look.bold {
            tp.push_str(r#" fo:font-weight="bold" style:font-weight-asian="bold" style:font-weight-complex="bold""#);
        }
        if let Some(c) = look.color {
            let _ = write!(tp, r##" fo:color="#{:06x}""##, c & 0xFF_FFFF);
        }
        let key = format!("{sp}|{tp}");
        let n = self.cell_styles.len();
        let name = self
            .cell_styles
            .entry(key)
            .or_insert_with(|| {
                let name = format!("Cell{}", n + 1);
                let _ = write!(auto, r#"<style:style style:name="{name}" style:family="table-cell"><style:table-cell-properties{sp}/>"#);
                if !tp.is_empty() {
                    let _ = write!(auto, "<style:text-properties{tp}/>");
                }
                auto.push_str("</style:style>");
                name
            })
            .clone();
        let _ = write!(body, r#"<table:table-cell table:style-name="{name}" office:value-type="string""#);
        if cs > 1 {
            let _ = write!(body, r#" table:number-columns-spanned="{cs}""#);
        }
        if rs > 1 {
            let _ = write!(body, r#" table:number-rows-spanned="{rs}""#);
        }
        body.push('>');
        let l = &mut self.levels[d];
        l.written = true;
        l.tc_open = true;
        true
    }

    /// Opens a table; returns its number and whether header rows opened.
    fn open_table(&mut self, body: &mut String, auto: &mut String, m: &TableMap) -> (usize, bool) {
        self.n += 1;
        let n = self.n;
        let default = TableProps::default();
        let p = m.props(&self.doc.paragraphs).unwrap_or(&default);
        let align = match p.align {
            Align::Center => "center",
            Align::Right => "right",
            _ => "left",
        };
        let _ = write!(auto, r#"<style:style style:name="Tbl{n}" style:family="table"><style:table-properties style:width="{}" table:align="{align}" fo:margin-left="{}"/></style:style>"#, pt(p.width()), pt(p.indent));
        let _ = write!(body, r#"<table:table table:name="Table{n}" table:style-name="Tbl{n}""#);
        // The document's own table styles are written as cell formatting.
        if let Some(s) = p.style.as_deref().and_then(table_style) {
            let l = p.look;
            let _ = write!(
                body,
                r#" table:template-name="{}" table:use-first-row-styles="{}" table:use-last-row-styles="{}" table:use-first-column-styles="{}" table:use-last-column-styles="{}" table:use-banding-rows-styles="{}" table:use-banding-columns-styles="{}""#,
                esc(&s.id),
                l.first_row,
                l.last_row,
                l.first_col,
                l.last_col,
                l.banded_rows,
                l.banded_cols
            );
        }
        body.push('>');
        let fallback = if p.widths.is_empty() { 72.0 } else { p.width() / p.widths.len() as f32 };
        for k in 0..m.cols.max(p.widths.len()) {
            let w = p.widths.get(k).copied().unwrap_or(fallback);
            let _ = write!(auto, r#"<style:style style:name="Tbl{n}.C{}" style:family="table-column"><style:table-column-properties style:column-width="{}"/></style:style>"#, k + 1, pt(w));
            let _ = write!(body, r#"<table:table-column table:style-name="Tbl{n}.C{}"/>"#, k + 1);
        }
        if p.header_rows > 0 {
            body.push_str("<table:table-header-rows>");
        }
        (n, p.header_rows > 0)
    }
}
