//! Cells on the clipboard. A copy goes out as an HTML table with the
//! cells' formatting, which spreadsheets, word processors and mail
//! programs paste formatted, and as an .xlsx of the cells themselves for
//! other ApexDesk windows. A paste reads either back; the tables of Excel
//! (classes in a style sheet), LibreOffice (tags and attributes) and
//! Google Sheets (inline styles) come in with their formatting.

use std::collections::HashSet;
use std::fmt::Write;
use std::sync::Arc;

use apex_doc::{Align, Attrs};
use apex_sheet::names::absolute_ref;
use apex_sheet::{CellRef, CellStyle, Clip, Content, DefinedName, HAlign, Line, LineStyle, Range, VAlign, Value, Workbook};

use crate::html_in::{align_word, color, declarations, decode, family, inline_css, split_tag, tag_end};

/// The defined name that marks where the copied cells are in the .xlsx.
const CLIP_NAME: &str = "ApexDeskClip";
/// Logical pixels of a column per CSS pixel (88 for Excel's 64).
const COL_SCALE: f32 = 88.0 / 64.0;
/// Size of cells that name none, in points.
const DEFAULT_PT: f32 = 11.0;

/// The cells of `r` as an HTML table: text as shown, each cell's
/// formatting inline, numbers also as numbers (`x:num` for Excel, `sdval`
/// for LibreOffice), merged cells spanning.
pub fn html(wb: &Workbook, sheet: usize, r: &Range) -> String {
    let sh = &wb.sheets[sheet];
    let mut s = String::from("<meta charset=\"utf-8\"><table style=\"border-collapse:collapse\">");
    for col in r.start.col..=r.end.col {
        let w = sh.col_widths.get(&col).copied().unwrap_or(sh.default_col_width);
        let _ = write!(s, "<col width=\"{}\">", (w / COL_SCALE).round().max(0.0));
    }
    let merges: Vec<Range> = sh.merges.iter().filter_map(|m| m.intersect(r)).collect();
    for row in r.start.row..=r.end.row {
        s.push_str("<tr>");
        for col in r.start.col..=r.end.col {
            let c = CellRef::new(row, col);
            let merge = merges.iter().find(|m| m.contains(c));
            if merge.is_some_and(|m| m.start != c) {
                continue;
            }
            let cell = wb.cell(sheet, c);
            let style = wb.styles.get(cell.map_or(0, |x| x.style));
            s.push_str("<td");
            if let Some(m) = merge {
                if m.cols() > 1 {
                    let _ = write!(s, " colspan=\"{}\"", m.cols());
                }
                if m.rows() > 1 {
                    let _ = write!(s, " rowspan=\"{}\"", m.rows());
                }
            }
            let css = cell_css(style);
            if !css.is_empty() {
                let _ = write!(s, " style=\"{}\"", escape(&css));
            }
            if let Some(n) = cell.and_then(|x| number(&x.content)) {
                let _ = write!(s, " x:num=\"{n}\" sdval=\"{n}\"");
            }
            // The same in HTML's older words: LibreOffice Calc reads
            // these and not the CSS.
            if let Some(c) = style.fill {
                let _ = write!(s, " bgcolor=\"#{c:06X}\"");
            }
            match style.halign {
                HAlign::Left => s.push_str(" align=\"left\""),
                HAlign::Center => s.push_str(" align=\"center\""),
                HAlign::Right => s.push_str(" align=\"right\""),
                HAlign::Justify => s.push_str(" align=\"justify\""),
                HAlign::General => {}
            }
            s.push_str(match style.valign {
                VAlign::Top => " valign=\"top\"",
                VAlign::Center => " valign=\"middle\"",
                VAlign::Bottom => " valign=\"bottom\"",
            });
            s.push('>');
            let (text, _) = wb.display(sheet, c);
            let font = style.font.is_some() || style.color.is_some() || style.font_size > 0;
            if font {
                s.push_str("<font");
                if let Some(f) = &style.font {
                    let _ = write!(s, " face=\"{}\"", escape(f));
                }
                if let Some(c) = style.color {
                    let _ = write!(s, " color=\"#{c:06X}\"");
                }
                if style.font_size > 0 {
                    let pt = f32::from(style.font_size) / 10.0;
                    let _ = write!(s, " size=\"{}\" style=\"font-size:{pt}pt\"", html_size(pt));
                }
                s.push('>');
            }
            // And the characters' look on a span, which OnlyOffice reads.
            let chars = char_css(style);
            if !chars.is_empty() {
                let _ = write!(s, "<span style=\"{}\">", escape(&chars));
            }
            let tags: Vec<&str> = [(style.bold, "b"), (style.italic, "i"), (style.underline, "u"), (style.strike, "s")].into_iter().filter_map(|(on, t)| on.then_some(t)).collect();
            for t in &tags {
                let _ = write!(s, "<{t}>");
            }
            s.push_str(&escape(&text).replace('\n', "<br>"));
            for t in tags.iter().rev() {
                let _ = write!(s, "</{t}>");
            }
            if !chars.is_empty() {
                s.push_str("</span>");
            }
            if font {
                s.push_str("</font>");
            }
            s.push_str("</td>");
        }
        s.push_str("</tr>");
    }
    s.push_str("</table>");
    s
}

/// The nearest of HTML's seven font sizes to `pt` points.
fn html_size(pt: f32) -> usize {
    const SIZES: [f32; 7] = [7.5, 10.0, 12.0, 13.5, 18.0, 24.0, 36.0];
    SIZES.iter().enumerate().min_by(|a, b| (a.1 - pt).abs().total_cmp(&(b.1 - pt).abs())).map_or(3, |(i, _)| i + 1)
}

fn number(c: &Content) -> Option<f64> {
    match c {
        Content::Value(Value::Number(n)) | Content::Formula { value: Value::Number(n), .. } | Content::Spill(Value::Number(n)) => Some(*n),
        _ => None,
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The look of a cell's characters as CSS declarations (none for the
/// default look).
fn char_css(st: &CellStyle) -> String {
    let mut s = String::new();
    if let Some(f) = &st.font {
        let _ = write!(s, "font-family:'{}';", f.replace('\'', ""));
    }
    if st.font_size > 0 {
        let _ = write!(s, "font-size:{}pt;", f32::from(st.font_size) / 10.0);
    }
    if st.bold {
        s.push_str("font-weight:bold;");
    }
    if st.italic {
        s.push_str("font-style:italic;");
    }
    match (st.underline, st.strike) {
        (true, true) => s.push_str("text-decoration:underline line-through;"),
        (true, false) => s.push_str("text-decoration:underline;"),
        (false, true) => s.push_str("text-decoration:line-through;"),
        _ => {}
    }
    if let Some(c) = st.color {
        let _ = write!(s, "color:#{c:06X};");
    }
    s
}

/// A cell format as CSS declarations.
fn cell_css(st: &CellStyle) -> String {
    let mut s = char_css(st);
    if st.font_size == 0 {
        let _ = write!(s, "font-size:{DEFAULT_PT}pt;");
    }
    if let Some(c) = st.fill {
        let _ = write!(s, "background-color:#{c:06X};");
    }
    match st.halign {
        HAlign::General => {}
        HAlign::Left => s.push_str("text-align:left;"),
        HAlign::Center => s.push_str("text-align:center;"),
        HAlign::Right => s.push_str("text-align:right;"),
        HAlign::Justify => s.push_str("text-align:justify;"),
    }
    s.push_str(match st.valign {
        VAlign::Top => "vertical-align:top;",
        VAlign::Center => "vertical-align:middle;",
        VAlign::Bottom => "vertical-align:bottom;",
    });
    if st.wrap {
        s.push_str("white-space:normal;");
    } else {
        s.push_str("white-space:nowrap;");
    }
    for (side, l) in [("top", st.borders.top), ("right", st.borders.right), ("bottom", st.borders.bottom), ("left", st.borders.left)] {
        let (w, kind) = match l.style {
            LineStyle::None => continue,
            LineStyle::Thin | LineStyle::Hair => ("1px", "solid"),
            LineStyle::Medium => ("2px", "solid"),
            LineStyle::Thick => ("3px", "solid"),
            LineStyle::Dashed => ("1px", "dashed"),
            LineStyle::Dotted => ("1px", "dotted"),
            LineStyle::Double => ("3px", "double"),
        };
        let _ = write!(s, "border-{side}:{w} {kind} #{:06X};", l.color.unwrap_or(0));
    }
    s
}

/// The cells of `clip`, copied from `sheet` of `wb`, as an .xlsx with
/// them where they were (so formulas keep their references) and a
/// defined name over them.
pub fn xlsx(wb: &Workbook, sheet: usize, clip: &Clip) -> Vec<u8> {
    let mut tmp = Workbook::new();
    let name = wb.sheets[sheet].name.clone();
    tmp.sheets[0].name = name.clone();
    let c = tmp.adopt_clip(clip);
    tmp.paste(0, &Range::single(clip.origin), &c);
    for &(i, w) in &clip.col_widths {
        tmp.sheets[0].col_widths.insert(clip.origin.col + i, w);
    }
    for &(i, h) in &clip.row_heights {
        tmp.sheets[0].row_heights.insert(clip.origin.row + i, h);
    }
    let end = CellRef::new(clip.origin.row + clip.rows.max(1) - 1, clip.origin.col + clip.cols.max(1) - 1);
    tmp.names.push(DefinedName::new(CLIP_NAME, None, absolute_ref(&name, Range::new(clip.origin, end))));
    crate::xlsx::write(&tmp)
}

/// The cells an .xlsx from [`xlsx`] holds, with their formats (for
/// `Workbook::adopt_clip`).
pub fn read_xlsx(data: &[u8]) -> Option<Clip> {
    let wb = crate::xlsx::read(data).ok()?;
    let (sheet, range) = wb.names.iter().find(|n| n.is(CLIP_NAME))?.range()?;
    let si = match sheet {
        Some(s) => wb.sheets.iter().position(|x| *x.name == *s)?,
        None => 0,
    };
    Some(wb.copy(si, &range))
}

/// Cells read from another program's HTML table, for
/// `Workbook::clip_from_inputs`: each one's row and column, the number it
/// carries, its text and its format.
#[derive(Debug, Default)]
pub struct HtmlCells {
    pub rows: u32,
    pub cols: u32,
    pub cells: Vec<(u32, u32, Option<f64>, String, CellStyle)>,
    /// Columns that have a width, in logical pixels.
    pub col_widths: Vec<(u32, f32)>,
}

/// The first table of an HTML page or fragment (Windows' header of
/// offsets is skipped); `None` when it holds no table.
pub fn read_html(html: &str) -> Option<HtmlCells> {
    let html = match html.find('<') {
        Some(i) if html.starts_with("Version:") => &html[i..],
        _ => html,
    };
    let mut r = TableReader::default();
    r.run(html);
    r.end_cell();
    (r.rows > 0 && r.out.cols > 0).then(|| {
        r.out.rows = r.rows;
        r.out
    })
}

struct CellB {
    row: u32,
    col: u32,
    text: String,
    num: Option<f64>,
    style: CellStyle,
    /// The format of the first text is the cell's.
    styled: bool,
}

#[derive(Default)]
struct TableReader {
    out: HtmlCells,
    /// Style sheet rules: selector and declarations.
    rules: Vec<(String, String)>,
    /// Bytes of declarations the rules hold, and those given to tags.
    css_bytes: usize,
    css_spent: usize,
    /// Open tables; cells are read in the outermost one only.
    depth: usize,
    /// The first table is over.
    done: bool,
    /// Rows started, and the next column of the open row.
    rows: u32,
    col: u32,
    /// Cells taken by spans from rows above.
    taken: HashSet<(u32, u32)>,
    cell: Option<CellB>,
    /// Character formatting of the open inline tags in the cell.
    inline: Vec<(String, Attrs)>,
    next_col_width: u32,
}

impl TableReader {
    fn run(&mut self, html: &str) {
        let b = html.as_bytes();
        let mut i = 0;
        while i < b.len() && !self.done {
            if b[i] != b'<' {
                let end = html[i..].find('<').map_or(html.len(), |e| i + e);
                self.text(&decode(&html[i..end]));
                i = end;
                continue;
            }
            let rest = &html[i..];
            if rest.starts_with("<!--") {
                i += rest.find("-->").map_or(rest.len(), |e| e + 3);
                continue;
            }
            if rest.starts_with("<!") || rest.starts_with("<?") {
                i += rest.find('>').map_or(rest.len(), |e| e + 1);
                continue;
            }
            let Some(end) = tag_end(rest) else {
                // (See `html_in`: a tag never ended takes the rest as text.)
                if rest.len() > 1 && (rest.as_bytes()[1].is_ascii_alphabetic() || rest.as_bytes()[1] == b'/') {
                    self.text(&decode(rest));
                    break;
                }
                self.text("<");
                i += 1;
                continue;
            };
            let inner = &rest[1..end];
            i += end + 1;
            if let Some(name) = inner.strip_prefix('/') {
                self.end_tag(&name.trim().to_ascii_lowercase());
                continue;
            }
            let (name, attrs) = split_tag(inner);
            let name = name.to_ascii_lowercase();
            if matches!(name.as_str(), "style" | "script" | "title") && !inner.ends_with('/') {
                let close = format!("</{name}");
                let body_end = find_ci(&html[i..], &close).map_or(html.len(), |e| i + e);
                if name == "style" {
                    self.style_sheet(&html[i..body_end]);
                }
                i = body_end + html[body_end..].find('>').map_or(html.len() - body_end, |g| g + 1);
                continue;
            }
            self.start_tag(&name, &attrs);
        }
    }

    fn style_sheet(&mut self, css: &str) {
        let css = css.replace("<!--", "").replace("-->", "");
        // Comments out, in one pass.
        let mut plain = String::with_capacity(css.len());
        let mut rest = css.as_str();
        while let Some(a) = rest.find("/*") {
            plain.push_str(&rest[..a]);
            rest = rest[a + 2..].find("*/").map_or("", |e| &rest[a + 2 + e + 2..]);
        }
        plain.push_str(rest);
        let css = plain;
        for rule in css.split('}') {
            let Some((sel, decls)) = rule.split_once('{') else { continue };
            // LibreOffice gives the whole page, cells too, a size its
            // cells don't have ("x-small"): sizes for the page aren't
            // the cells'.
            let page = sel.split(',').any(|s| s.trim().eq_ignore_ascii_case("body"));
            let decls: String = if page { decls.split(';').filter(|d| !d.trim().to_ascii_lowercase().starts_with("font-size")).collect::<Vec<_>>().join(";") } else { decls.to_string() };
            for s in sel.split(',') {
                let s = s.trim().to_ascii_lowercase();
                // Rules kept, and what they hold in all, are bounded: a
                // selector repeated thousands of times copies its
                // declarations as often.
                if !s.is_empty() && self.rules.len() < 4096 && decls.len() <= 4096 && self.css_bytes + decls.len() <= 1 << 20 {
                    self.css_bytes += decls.len();
                    self.rules.push((s, decls.trim().to_string()));
                }
            }
        }
    }

    /// The declarations that apply to a `tag` with `classes`, the style
    /// sheet's first and the inline ones last.
    fn css_for(&mut self, tag: &str, classes: &str, inline: &str) -> String {
        // What the style sheet gives all the tags of a paste together is
        // bounded too (thousands of tags each taking its rules).
        if self.css_spent > 1 << 26 {
            return inline.to_string();
        }
        let classes: Vec<String> = classes.split_whitespace().take(16).map(|c| c.to_ascii_lowercase()).collect();
        let mut out = String::new();
        for (sel, decls) in &self.rules {
            let hit = sel == tag || classes.iter().any(|c| sel.strip_prefix('.') == Some(c) || sel.strip_prefix(tag).and_then(|s| s.strip_prefix('.')) == Some(c));
            if hit && out.len() + decls.len() < 1 << 16 {
                out.push_str(decls);
                out.push(';');
            }
        }
        self.css_spent += out.len() + self.rules.len();
        out.push_str(inline);
        out
    }

    fn attrs(&self) -> Attrs {
        self.inline.last().map(|(_, a)| a.clone()).unwrap_or_default()
    }

    fn text(&mut self, t: &str) {
        let Some(cell) = &mut self.cell else { return };
        let mut collapsed = String::with_capacity(t.len());
        let mut space = cell.text.is_empty() || cell.text.ends_with([' ', '\n']);
        for ch in t.chars() {
            if ch.is_whitespace() && ch != '\u{a0}' {
                if !space {
                    collapsed.push(' ');
                    space = true;
                }
            } else {
                collapsed.push(if ch == '\u{a0}' { ' ' } else { ch });
                space = false;
            }
        }
        if collapsed.trim().is_empty() {
            cell.text.push_str(&collapsed);
            return;
        }
        if !cell.styled {
            cell.styled = true;
            let a = self.inline.last().map(|(_, a)| a.clone()).unwrap_or_default();
            char_style(&mut cell.style, &a);
        }
        cell.text.push_str(&collapsed);
    }

    fn start_tag(&mut self, name: &str, attrs: &[(String, String)]) {
        let attr = |k: &str| attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        match name {
            "table" => {
                if self.depth > 0 {
                    self.text(" ");
                }
                self.depth += 1;
            }
            _ if self.depth != 1 => {
                if name == "br" {
                    self.newline();
                }
            }
            // A group of columns with a width (LibreOffice), else its
            // columns say theirs.
            "col" | "colgroup" if name == "col" || attr("width").is_some() || attr("style").is_some_and(|c| c.contains("width")) => {
                let span = attr("span").and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(1).clamp(1, 1000);
                let css = self.css_for("col", attr("class").unwrap_or(""), attr("style").unwrap_or(""));
                let w = declarations(&css).filter(|(k, _)| k == "width").filter_map(|(_, v)| css_px(v)).last().or_else(|| attr("width").and_then(|w| w.trim().trim_end_matches("px").parse::<f32>().ok()));
                for _ in 0..span {
                    if let Some(w) = w.filter(|w| *w > 0.0) {
                        self.out.col_widths.push((self.next_col_width, (w * COL_SCALE).round()));
                    }
                    self.next_col_width += 1;
                }
            }
            "tr" => {
                self.end_cell();
                self.rows += 1;
                self.col = 0;
            }
            "td" | "th" => {
                self.end_cell();
                if self.rows == 0 {
                    self.rows = 1;
                }
                let row = self.rows - 1;
                while self.taken.contains(&(row, self.col)) {
                    self.col += 1;
                }
                let span = |k: &str| attr(k).and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(1).clamp(1, 1000);
                let (mut cs, mut rs) = (span("colspan"), span("rowspan"));
                // Spans that would cover more cells than a sheet takes
                // in a paste are dropped.
                if self.taken.len() + (cs * rs) as usize > 1 << 22 {
                    (cs, rs) = (1, 1);
                }
                for r in row..row + rs {
                    for c in self.col..self.col + cs {
                        if (r, c) != (row, self.col) {
                            self.taken.insert((r, c));
                        }
                    }
                }
                let css = self.css_for(name, attr("class").unwrap_or(""), attr("style").unwrap_or(""));
                let mut a = Attrs { bold: name == "th", ..Attrs::default() };
                inline_css(&mut a, &css);
                let mut style = CellStyle::default();
                cell_style(&mut style, &css);
                if let Some(c) = attr("bgcolor").and_then(color) {
                    style.fill = Some(c);
                }
                if let Some(al) = attr("align").and_then(align_word) {
                    style.halign = halign(al);
                }
                if let Some(v) = attr("valign").and_then(valign) {
                    style.valign = v;
                }
                // LibreOffice writes sdval with the locale's decimal comma.
                let num = attr("x:num").or(attr("sdval")).and_then(|v| v.trim().replace(',', ".").parse::<f64>().ok()).or_else(|| attr("data-sheets-value").and_then(sheets_number));
                self.inline = vec![(name.to_string(), a)];
                self.cell = Some(CellB { row, col: self.col, text: String::new(), num, style, styled: false });
                self.out.cols = self.out.cols.max(self.col + cs);
                self.col += cs;
            }
            "br" => self.newline(),
            "p" | "div" => {
                if self.cell.as_ref().is_some_and(|c| !c.text.trim().is_empty()) {
                    self.newline();
                }
            }
            "b" | "strong" | "i" | "em" | "u" | "ins" | "s" | "strike" | "del" | "font" | "span" | "a" => {
                if self.cell.is_none() {
                    return;
                }
                let mut a = self.attrs();
                match name {
                    "b" | "strong" => a.bold = true,
                    "i" | "em" => a.italic = true,
                    "u" | "ins" => a.underline = true,
                    "s" | "strike" | "del" => a.strike = true,
                    "font" => {
                        if let Some(c) = attr("color").and_then(color) {
                            a.color = Some(c);
                        }
                        if let Some(f) = attr("face").and_then(family) {
                            a.font = Some(f.into());
                        }
                        if let Some(n) = attr("size").and_then(|s| s.trim().parse::<usize>().ok()) {
                            const PT: [f32; 7] = [7.5, 10.0, 12.0, 13.5, 18.0, 24.0, 36.0];
                            a.size = Some((PT[n.clamp(1, 7) - 1] * 2.0) as u16);
                        }
                    }
                    _ => {}
                }
                let css = self.css_for(name, attr("class").unwrap_or(""), attr("style").unwrap_or(""));
                inline_css(&mut a, &css);
                // Tags left open stay few: each closing tag looks them up.
                if self.inline.len() < 256 {
                    self.inline.push((name.to_string(), a));
                }
            }
            _ => {}
        }
    }

    fn end_tag(&mut self, name: &str) {
        match name {
            "table" => {
                self.depth = self.depth.saturating_sub(1);
                if self.depth == 0 {
                    self.end_cell();
                    self.done = self.rows > 0;
                }
            }
            _ if self.depth != 1 => {}
            "td" | "th" | "tr" => self.end_cell(),
            _ => {
                if let Some(i) = self.inline.iter().rposition(|(n, _)| n == name) {
                    if i > 0 {
                        self.inline.truncate(i);
                    }
                }
            }
        }
    }

    fn newline(&mut self) {
        if let Some(c) = &mut self.cell {
            let t = c.text.trim_end_matches(' ').len();
            c.text.truncate(t);
            c.text.push('\n');
        }
    }

    fn end_cell(&mut self) {
        let Some(c) = self.cell.take() else { return };
        self.inline.clear();
        let text = c.text.trim_matches([' ', '\n']).to_string();
        let mut c = c;
        if c.style.num_fmt.as_ref() == "General"
            && let Some(f) = shown_format(&text)
        {
            c.style.num_fmt = f.into();
        }
        if !text.is_empty() || c.num.is_some() || c.style != CellStyle::default() {
            self.out.cells.push((c.row, c.col, c.num, text, c.style));
        }
    }
}

/// The number format a plain number shown as `text` needs to look the
/// same ("1,50", "1.234"), when the program didn't say: decimals and
/// thousands separators as written. `None` when the general format
/// already shows it so, or when it can't tell.
fn shown_format(text: &str) -> Option<String> {
    let t = text.strip_prefix('-').unwrap_or(text);
    if t.is_empty() || !t.starts_with(|c: char| c.is_ascii_digit()) || !t.chars().all(|c| c.is_ascii_digit() || c == ',' || c == '.') {
        return None;
    }
    let seps: Vec<(usize, char)> = t.char_indices().filter(|(_, c)| !c.is_ascii_digit()).collect();
    let last = seps.last()?.1;
    let kinds = seps.iter().filter(|(_, c)| *c != last).count();
    let (decimals, grouped) = if kinds > 0 {
        // Both: the last is the decimal one.
        (t.len() - seps.last()?.0 - 1, true)
    } else if seps.len() == 1 {
        let after = t.len() - seps[0].0 - 1;
        // "1,500" is a thousand or one and a half.
        if after == 3 {
            return None;
        }
        (after, false)
    } else {
        (0, true)
    };
    if decimals == 0 && !grouped || decimals > 0 && !t.ends_with('0') && !grouped {
        return None;
    }
    let mut f = String::from(if grouped { "#,##0" } else { "0" });
    if decimals > 0 {
        f.push('.');
        f.push_str(&"0".repeat(decimals));
    }
    Some(f)
}

/// A length in CSS pixels.
fn css_px(v: &str) -> Option<f32> {
    let v = v.trim();
    let n = |s: &str| s.trim().parse::<f32>().ok();
    if let Some(p) = v.strip_suffix("pt") {
        n(p).map(|p| p / 0.75)
    } else if let Some(p) = v.strip_suffix("px") {
        n(p)
    } else if let Some(p) = v.strip_suffix("in") {
        n(p).map(|p| p * 96.0)
    } else if let Some(p) = v.strip_suffix("cm") {
        n(p).map(|p| p * 96.0 / 2.54)
    } else {
        n(v)
    }
}

/// Google Sheets' `data-sheets-value`: `{"1":3,"3":12.5}` is the number 12.5.
fn sheets_number(v: &str) -> Option<f64> {
    let compact: String = v.chars().filter(|c| !c.is_whitespace()).collect();
    if !compact.contains("\"1\":3") {
        return None;
    }
    let rest = &compact[compact.find("\"3\":")? + 4..];
    let end = rest.find([',', '}']).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn halign(a: Align) -> HAlign {
    match a {
        Align::Left => HAlign::Left,
        Align::Center => HAlign::Center,
        Align::Right => HAlign::Right,
        Align::Justify => HAlign::Justify,
    }
}

fn valign(v: &str) -> Option<VAlign> {
    match v.trim().to_ascii_lowercase().as_str() {
        "top" | "text-top" => Some(VAlign::Top),
        "middle" | "center" => Some(VAlign::Center),
        "bottom" | "text-bottom" | "baseline" => Some(VAlign::Bottom),
        _ => None,
    }
}

/// The character formatting of a cell's first text.
fn char_style(st: &mut CellStyle, a: &Attrs) {
    st.bold = a.bold;
    st.italic = a.italic;
    st.underline = a.underline;
    st.strike = a.strike;
    st.color = a.color.filter(|&c| c != 0);
    st.font = a.font.clone().map(|f| Arc::from(&*f));
    st.font_size = a.size.map_or(0, |hp| (hp * 5).min(4090));
}

/// The cell formatting of CSS declarations: fill, alignment, wrapping and
/// borders.
fn cell_style(st: &mut CellStyle, css: &str) {
    for (k, v) in declarations(css) {
        let lv = v.to_ascii_lowercase();
        match k.as_str() {
            "background-color" | "background" => st.fill = color(&lv).filter(|&c| c != 0xFFFFFF),
            "text-align" => {
                if let Some(a) = align_word(&lv) {
                    st.halign = halign(a);
                }
            }
            "vertical-align" => {
                if let Some(a) = valign(&lv) {
                    st.valign = a;
                }
            }
            "white-space" => st.wrap = matches!(lv.as_str(), "normal" | "pre-wrap" | "pre-line" | "break-spaces"),
            "border" => {
                let l = border(&lv);
                st.borders.top = l;
                st.borders.bottom = l;
                st.borders.left = l;
                st.borders.right = l;
            }
            "border-top" => st.borders.top = border(&lv),
            "border-bottom" => st.borders.bottom = border(&lv),
            "border-left" => st.borders.left = border(&lv),
            "border-right" => st.borders.right = border(&lv),
            _ => {}
        }
    }
}

/// A CSS border shorthand (`.5pt solid windowtext`, `1px dashed #ccc`).
fn border(v: &str) -> Line {
    let mut style = None;
    let mut width = 1.0f32;
    let mut col = None;
    for w in v.split_whitespace() {
        match w {
            "none" | "hidden" => return Line { style: LineStyle::None, color: None },
            "solid" => style = Some(LineStyle::Thin),
            "dashed" => style = Some(LineStyle::Dashed),
            "dotted" => style = Some(LineStyle::Dotted),
            "double" => style = Some(LineStyle::Double),
            "thin" => width = 1.0,
            "medium" => width = 2.0,
            "thick" => width = 3.0,
            "windowtext" => col = Some(0),
            _ => {
                if let Some(px) = css_px(w).filter(|_| w.starts_with(|c: char| c.is_ascii_digit() || c == '.')) {
                    width = px;
                } else if let Some(c) = color(w) {
                    col = Some(c);
                }
            }
        }
    }
    let style = match style {
        None => return Line { style: LineStyle::None, color: None },
        Some(LineStyle::Thin) if width >= 2.5 => LineStyle::Thick,
        Some(LineStyle::Thin) if width >= 1.5 => LineStyle::Medium,
        Some(s) => s,
    };
    Line { style, color: col.filter(|&c| c != 0) }
}

/// Where `close` (lowercase ASCII) next is in `s`, in any case.
fn find_ci(s: &str, close: &str) -> Option<usize> {
    let (s, n) = (s.as_bytes(), close.as_bytes());
    s.windows(n.len()).position(|w| w.eq_ignore_ascii_case(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell<'a>(h: &'a HtmlCells, r: u32, c: u32) -> &'a (u32, u32, Option<f64>, String, CellStyle) {
        h.cells.iter().find(|x| x.0 == r && x.1 == c).unwrap()
    }

    #[test]
    fn excel_table_with_classes() {
        let html = "Version:0.9\r\nStartHTML:0000000105\r\n<html xmlns:x=\"urn:schemas-microsoft-com:office:excel\"><head><style><!--\ntd {font-family:Calibri, sans-serif; font-size:11.0pt;}\n.xl65 {font-weight:700; background:yellow; mso-pattern:black none; border-bottom:.5pt solid windowtext;}\n.xl66 {color:red; text-align:center;}\n--></style></head><body><table><col width=64 style='width:48pt'><col width=128><!--StartFragment--><tr><td class=xl65>Name</td><td class=xl66 x:num=\"1234.5\">1.234,50</td></tr><tr><td colspan=2>Total</td></tr><!--EndFragment--></table></body></html>";
        let h = read_html(html).unwrap();
        assert_eq!((h.rows, h.cols), (2, 2));
        let a = cell(&h, 0, 0);
        assert_eq!(a.3, "Name");
        assert!(a.4.bold);
        assert_eq!(a.4.fill, Some(0xFFFF00));
        assert_eq!(a.4.font.as_deref(), Some("Calibri"));
        assert_eq!(a.4.font_size, 110);
        assert_eq!(a.4.borders.bottom.style, LineStyle::Thin);
        let b = cell(&h, 0, 1);
        assert_eq!(b.2, Some(1234.5));
        assert_eq!(b.4.color, Some(0xFF0000));
        assert_eq!(b.4.halign, HAlign::Center);
        assert_eq!(cell(&h, 1, 0).3, "Total");
        assert_eq!(h.col_widths, vec![(0, 88.0), (1, 176.0)]);
    }

    #[test]
    fn libreoffice_and_google_tables() {
        let calc = "<style>body,div,table,thead,tbody,tfoot,tr,th,td,p { font-family:\"Liberation Sans\"; font-size:x-small }</style><table><colgroup span=\"2\" width=\"64\"></colgroup><tr><td sdval=\"1,5\" data-sheets-value='{\"1\":3,\"3\":1}'>1,50</td><td>x</td></tr></table>";
        let h = read_html(calc).unwrap();
        let a = cell(&h, 0, 0);
        assert_eq!(a.2, Some(1.5));
        assert_eq!(&*a.4.num_fmt, "0.00");
        assert_eq!(a.4.font.as_deref(), Some("Liberation Sans"));
        assert_eq!(a.4.font_size, 0);
        assert_eq!(h.col_widths, vec![(0, 88.0), (1, 88.0)]);
        let lo = "<table><tr><td bgcolor=\"#C0C0C0\" align=\"right\" sdval=\"3\"><b><font face=\"Liberation Sans\" color=\"#0000FF\">3</font></b></td><td>a<br>b</td></tr></table>";
        let h = read_html(lo).unwrap();
        let a = cell(&h, 0, 0);
        assert_eq!((a.2, a.3.as_str()), (Some(3.0), "3"));
        assert!(a.4.bold);
        assert_eq!(a.4.color, Some(0x0000FF));
        assert_eq!(a.4.font.as_deref(), Some("Liberation Sans"));
        assert_eq!(a.4.fill, Some(0xC0C0C0));
        assert_eq!(a.4.halign, HAlign::Right);
        assert_eq!(cell(&h, 0, 1).3, "a\nb");
        let g = "<google-sheets-html-origin><table><tbody><tr><td style=\"font-style:italic;background-color:#d9ead3\" data-sheets-value='{\"1\":3,\"3\":0.25}'>25%</td></tr></tbody></table>";
        let h = read_html(g).unwrap();
        let a = cell(&h, 0, 0);
        assert_eq!(a.2, Some(0.25));
        assert!(a.4.italic);
        assert_eq!(a.4.fill, Some(0xD9EAD3));
        assert!(read_html("<p>no table</p>").is_none());
    }

    #[test]
    fn numbers_keep_their_shown_decimals() {
        assert_eq!(shown_format("1,50").as_deref(), Some("0.00"));
        assert_eq!(shown_format("-2.2500").as_deref(), Some("0.0000"));
        assert_eq!(shown_format("1.234,50").as_deref(), Some("#,##0.00"));
        assert_eq!(shown_format("1,234,567").as_deref(), Some("#,##0"));
        for general in ["1,5", "12", "1,500", "2.250", "abc", "", "1,5%"] {
            assert_eq!(shown_format(general), None, "{general}");
        }
    }

    #[test]
    fn copied_cells_come_back() {
        let mut wb = Workbook::new();
        wb.sheets[0].name = "Data".into();
        wb.set_input(0, CellRef::new(1, 1), "2");
        wb.set_input(0, CellRef::new(2, 1), "=B2*3");
        wb.modify_style(0, &Range::single(CellRef::new(1, 1)), |s| {
            s.bold = true;
            s.font_size = 160;
            s.fill = Some(0xFFFF00);
            s.font = Some("Liberation Serif".into());
        });
        let r = Range::new(CellRef::new(1, 1), CellRef::new(2, 1));
        let clip = wb.copy(0, &r);
        let back = read_xlsx(&xlsx(&wb, 0, &clip)).unwrap();
        assert_eq!(back.origin, clip.origin);
        assert_eq!((back.rows, back.cols), (2, 1));
        let mut other = Workbook::new();
        let c = other.adopt_clip(&back);
        other.paste(0, &Range::single(CellRef::new(0, 0)), &c);
        assert_eq!(other.display(0, CellRef::new(1, 0)).0, "6");
        assert!(other.styles.get(other.cell(0, CellRef::new(0, 0)).unwrap().style).bold);
        let h = html(&wb, 0, &r);
        assert!(h.contains("font-weight:bold") && h.contains("x:num=\"6\""), "{h}");
        // HTML's older words too, for LibreOffice Calc.
        assert!(h.contains("bgcolor=\"#FFFF00\"") && h.contains("<font face=\"Liberation Serif\" size=\"5\"") && h.contains("<b>2</b>"), "{h}");
        // And on a span for OnlyOffice.
        assert!(h.contains("background-color:#FFFF00") && h.contains("<span style=\"font-family:'Liberation Serif';font-size:16pt;font-weight:bold;\">"), "{h}");
        let t = read_html(&h).unwrap();
        assert_eq!(cell(&t, 1, 0).3, "6");
        let a = &cell(&t, 0, 0).4;
        assert!(a.bold);
        assert_eq!((a.font_size, a.fill, a.font.as_deref()), (160, Some(0xFFFF00), Some("Liberation Serif")));
    }
}
