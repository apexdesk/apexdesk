//! HTML import: what browsers, mail programs and office suites put on the
//! clipboard. Real HTML is rarely valid XML, so this is a forgiving
//! scanner: paragraphs, headings, lists, quotes, preformatted text,
//! tables, links, pictures in data URIs and the character formatting of
//! tags, `<font>` and inline `style` attributes. Style sheets and classes
//! are left out.

use std::sync::Arc;

use apex_doc::{Align, Attrs, Document, Image, LINE_BREAK, LineRule, OBJECT, ParaKind, ParaStyle, Paragraph, Table};

/// Width of tables read from HTML, in points.
const TABLE_WIDTH: f32 = 450.0;
/// Size of text that names none, in points (a browser's 16px).
const BASE_SIZE: f32 = 12.0;
/// Open elements kept, inline and block apiece.
const MAX_OPEN: usize = 256;
/// Tables nested in cells past this are left out.
const MAX_TABLE_DEPTH: usize = 64;

/// Reads an HTML page or fragment. On Windows' clipboard the fragment
/// comes after a header of offsets; only the text between the
/// `StartFragment` and `EndFragment` marks is read when they are there,
/// in the lists, tables and formatting the tags before it open.
pub fn read(html: &str) -> Document {
    let html = match html.find('<') {
        Some(i) if html.starts_with("Version:") => &html[i..],
        _ => html,
    };
    let (context, html) = match (html.find("<!--StartFragment-->"), html.find("<!--EndFragment-->")) {
        (Some(a), Some(b)) if a < b => (&html[..a], &html[a + 20..b]),
        _ => ("", html),
    };
    let mut r = Reader { context: true, ..Default::default() };
    r.run(context);
    (r.context, r.para, r.space) = (false, None, true);
    r.run(html);
    r.end_para();
    while !r.tables.is_empty() {
        r.end_table();
    }
    let mut paras = r.out.into_iter().next().unwrap_or_default();
    if paras.is_empty() {
        paras.push(Paragraph::new(""));
    }
    Document::from_paragraphs(paras)
}

struct TableB {
    rows: Vec<Vec<Vec<Paragraph>>>,
    row: Option<Vec<Vec<Paragraph>>>,
    head_rows: usize,
    in_head: bool,
    /// The open cell spans this many columns.
    span: usize,
}

#[derive(Default)]
struct Reader {
    /// Where paragraphs go: the body, or the open cells of tables.
    out: Vec<Vec<Paragraph>>,
    /// Character formatting of the open inline tags.
    inline: Vec<(String, Attrs)>,
    /// Paragraph formatting of the open block tags.
    blocks: Vec<(String, ParaStyle)>,
    /// The open lists.
    lists: Vec<ListB>,
    tables: Vec<TableB>,
    /// Tables left out, nested too deep.
    tables_skipped: usize,
    /// The paragraph being read: its style and runs.
    para: Option<(ParaStyle, Vec<(String, Attrs)>)>,
    /// The last character written was a space (or the paragraph's start).
    space: bool,
    pre: usize,
    tables_made: u32,
    /// Reading the tags before the fragment: their text is left out.
    context: bool,
}

/// A list being read.
#[derive(Clone, Copy, Default)]
struct ListB {
    numbered: bool,
    /// Bullet glyph or numbering style (`ParaStyle::list_format`).
    format: u8,
    /// `list-style-type: none`: items without a mark.
    unmarked: bool,
    /// The number of the next item, when the list says one.
    start: u16,
}

impl Reader {
    fn attrs(&self) -> Attrs {
        self.inline.last().map(|(_, a)| a.clone()).unwrap_or_default()
    }

    fn block_style(&self) -> ParaStyle {
        self.blocks.last().map(|(_, s)| s.clone()).unwrap_or_default()
    }

    /// The size of the text being read, in points (for `em` lengths).
    fn size_pt(&self) -> f32 {
        self.attrs().size.map_or(BASE_SIZE, |s| s as f32 / 2.0)
    }

    fn sink(&mut self) -> &mut Vec<Paragraph> {
        if self.out.is_empty() {
            self.out.push(Vec::new());
        }
        self.out.last_mut().unwrap()
    }

    fn push_text(&mut self, s: &str) {
        let attrs = self.attrs();
        let mut buf = String::new();
        for ch in s.chars() {
            let ch = if ch == '\u{a0}' { ' ' } else { ch };
            if self.pre == 0 && ch.is_whitespace() && ch != LINE_BREAK {
                if !self.space {
                    buf.push(' ');
                    self.space = true;
                }
                continue;
            }
            if self.pre > 0 && ch == '\n' {
                // A line of preformatted text is a paragraph.
                self.push_run(std::mem::take(&mut buf), attrs.clone());
                self.end_para();
                self.para = Some((self.block_style(), Vec::new()));
                continue;
            }
            buf.push(ch);
            self.space = false;
        }
        self.push_run(buf, attrs);
    }

    fn push_run(&mut self, s: String, attrs: Attrs) {
        if s.is_empty() || self.context {
            return;
        }
        // Spaces at the start of a paragraph go.
        if self.para.as_ref().is_none_or(|(_, runs)| runs.iter().all(|(t, _)| t.trim().is_empty())) && s.trim().is_empty() && self.pre == 0 {
            return;
        }
        let style = self.block_style();
        let (_, runs) = self.para.get_or_insert_with(|| (style, Vec::new()));
        match runs.last_mut() {
            Some((t, a)) if *a == attrs => t.push_str(&s),
            _ => runs.push((s, attrs)),
        }
    }

    fn end_para(&mut self) {
        self.space = true;
        let Some((style, mut runs)) = self.para.take() else { return };
        // Spaces at the end of a paragraph go too.
        while let Some((t, _)) = runs.last_mut() {
            let keep = t.trim_end_matches(' ').len();
            t.truncate(keep);
            if !t.is_empty() {
                break;
            }
            runs.pop();
        }
        let mut p = Paragraph::new("");
        p.style = style;
        for (t, a) in runs {
            p.push(&t, a);
        }
        self.sink().push(p);
    }

    /// A block starts: what was being read ends and a new paragraph
    /// takes `style`.
    fn open_block(&mut self, tag: &str, style: ParaStyle) {
        self.end_para();
        self.push_block((tag.to_string(), style));
    }

    // Elements left open past this depth are not kept: closing tags look
    // back through the open ones, which deeper nesting would make slow.
    fn push_block(&mut self, b: (String, ParaStyle)) {
        if self.blocks.len() < MAX_OPEN {
            self.blocks.push(b);
        }
    }

    fn push_inline(&mut self, i: (String, Attrs)) {
        if self.inline.len() < MAX_OPEN {
            self.inline.push(i);
        }
    }

    fn close_block(&mut self, tag: &str) {
        if let Some(i) = self.blocks.iter().rposition(|(t, _)| t == tag) {
            self.end_para();
            self.blocks.truncate(i);
        }
    }

    fn close_inline(&mut self, tag: &str) {
        if let Some(i) = self.inline.iter().rposition(|(t, _)| t == tag) {
            self.inline.truncate(i);
        }
    }

    fn end_cell(&mut self) {
        if self.tables.is_empty() || self.out.len() < self.tables.len() + 1 {
            return;
        }
        self.end_para();
        let paras = self.out.pop().unwrap_or_default();
        let t = self.tables.last_mut().unwrap();
        let row = t.row.get_or_insert_with(Vec::new);
        row.push(paras);
        for _ in 1..t.span {
            row.push(Vec::new());
        }
    }

    fn end_row(&mut self) {
        self.end_cell();
        let Some(t) = self.tables.last_mut() else { return };
        if let Some(r) = t.row.take().filter(|r| !r.is_empty()) {
            if t.in_head && t.head_rows == t.rows.len() {
                t.head_rows += 1;
            }
            t.rows.push(r);
        }
    }

    fn end_table(&mut self) {
        self.end_row();
        let Some(t) = self.tables.pop() else { return };
        let cols = t.rows.iter().map(Vec::len).max().unwrap_or(0);
        if cols == 0 {
            return;
        }
        let nested = !self.tables.is_empty();
        if nested {
            // A table in a cell: its cells' paragraphs, one after another.
            let paras: Vec<Paragraph> = t.rows.into_iter().flatten().flatten().collect();
            self.sink().extend(paras);
            return;
        }
        // Each cell is a paragraph at least: no more than a document keeps
        // (short rows are filled to the widest).
        let cols = cols.min(apex_doc::MAX_COLS);
        let mut rows = t.rows;
        rows.truncate(apex_doc::MAX_PARAGRAPHS.saturating_sub(self.sink().len()) / cols);
        for chunk in rows.chunks(apex_doc::MAX_ROWS) {
            let mut table = Table::new(chunk.len(), cols, TABLE_WIDTH);
            table.props.header_rows = t.head_rows.min(chunk.len()) as _;
            for (r, row) in chunk.iter().enumerate() {
                for c in 0..cols {
                    let paras = row.get(c).cloned().unwrap_or_default();
                    table.rows[r][c].paras = if paras.is_empty() { vec![Paragraph::new("")] } else { paras };
                }
            }
            self.tables_made += 1;
            let made = self.tables_made;
            self.sink().extend(table.into_paras(made));
        }
    }

    fn run(&mut self, html: &str) {
        let mut i = 0;
        let b = html.as_bytes();
        while i < b.len() {
            if b[i] != b'<' {
                let end = html[i..].find('<').map_or(html.len(), |e| i + e);
                let text = decode(&html[i..end]);
                self.push_text(&text);
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
                // A lone '<' is text; one that starts a tag never ended
                // takes the rest as text (else each '<' after it would
                // look to the end again).
                if rest.len() > 1 && (rest.as_bytes()[1].is_ascii_alphabetic() || rest.as_bytes()[1] == b'/') {
                    self.push_text(&decode(rest));
                    break;
                }
                self.push_text("<");
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
            // What these hold is not text of the page.
            if matches!(name.as_str(), "head" | "style" | "script" | "title" | "template" | "noscript" | "svg" | "math") && !inner.ends_with('/') {
                let close = format!("</{name}");
                i += find_ci(&html[i..], &close).map_or(html.len() - i, |e| e + html[i + e..].find('>').map_or(0, |g| g + 1));
                continue;
            }
            self.start_tag(&name, &attrs);
        }
    }

    fn start_tag(&mut self, name: &str, attrs: &[(String, String)]) {
        let attr = |k: &str| attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        let css = attr("style").unwrap_or("");
        match name {
            "br" => {
                let a = self.attrs();
                self.push_run(LINE_BREAK.to_string(), a);
                self.space = true;
            }
            "img" => {
                if let Some(img) = attr("src").and_then(|s| data_image(s, attr("alt").unwrap_or(""), attr("width"), attr("height"))) {
                    let a = Attrs { image: Some(Arc::new(img)), ..self.attrs() };
                    self.push_run(OBJECT.to_string(), a);
                    self.space = false;
                }
            }
            "hr" => self.end_para(),
            // Tables nested past the most are left out (each nested table's
            // paragraphs are moved to its parent's cell).
            "table" if self.tables.len() >= MAX_TABLE_DEPTH => self.tables_skipped += 1,
            "table" => {
                self.end_para();
                // The body exists before the first cell is stacked on it.
                self.sink();
                self.tables.push(TableB { rows: Vec::new(), row: None, head_rows: 0, in_head: false, span: 1 });
            }
            "thead" => {
                if let Some(t) = self.tables.last_mut() {
                    t.in_head = true;
                }
            }
            "tbody" | "tfoot" => {
                if let Some(t) = self.tables.last_mut() {
                    t.in_head = false;
                }
            }
            "tr" => self.end_row(),
            "td" | "th" => {
                if self.tables.is_empty() {
                    return;
                }
                self.end_cell();
                let span = attr("colspan").and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(1).clamp(1, apex_doc::MAX_COLS);
                if let Some(t) = self.tables.last_mut() {
                    t.span = span;
                    if name == "th" && t.rows.len() == t.head_rows && t.row.as_ref().is_none_or(Vec::is_empty) {
                        t.in_head = true;
                    }
                }
                self.out.push(Vec::new());
                self.space = true;
                let mut st = ParaStyle { align: block_align(css).unwrap_or_default(), ..Default::default() };
                st.align = block_align(css).or(attr("align").and_then(align_word)).unwrap_or(st.align);
                self.push_block((name.to_string(), st));
                if name == "th" {
                    let a = Attrs { bold: true, ..self.attrs() };
                    self.push_inline((name.to_string(), a));
                }
            }
            "ul" | "ol" | "dl" => {
                self.end_para();
                let mut l = ListB { numbered: name == "ol", ..Default::default() };
                if let Some(t) = attr("type") {
                    list_type(&mut l, t);
                }
                list_css(&mut l, css);
                l.start = attr("start").and_then(|s| s.trim().parse::<u16>().ok()).filter(|&n| n != 1).unwrap_or(0);
                self.lists.push(l);
                // The list's line height goes to its items; its margins don't.
                let mut st = self.block_style();
                para_css(&mut st, css, self.size_pt(), false);
                self.push_block((name.to_string(), st));
                // The look browsers give the list is its text's.
                let mut a = self.attrs();
                inline_css(&mut a, css);
                self.push_inline((name.to_string(), a));
            }
            "li" => {
                // An item ends the one before it, closed or not.
                self.close_block("li");
                self.close_inline("li");
                let mut a = self.attrs();
                inline_css(&mut a, css);
                self.push_inline((name.to_string(), a));
                let mut l = self.lists.last().copied().unwrap_or_default();
                if let Some(t) = attr("type") {
                    list_type(&mut l, t);
                }
                list_css(&mut l, css);
                let level = self.lists.len().saturating_sub(1).min(8) as u8;
                let mut st = self.block_style();
                (st.space_before, st.space_after, st.first_line) = (None, None, 0.0);
                if l.unmarked {
                    st.indent_left += 18.0 * (level + 1) as f32;
                } else {
                    st.kind = if l.numbered { ParaKind::Numbered } else { ParaKind::Bullet };
                    (st.level, st.list_format) = (level, l.format);
                    st.start_at = self.lists.last_mut().map_or(0, |l| std::mem::take(&mut l.start));
                }
                st.align = block_align(css).unwrap_or(st.align);
                para_css(&mut st, css, self.size_pt(), true);
                self.open_block("li", st);
            }
            _ if is_block(name) => {
                let mut st = self.block_style();
                // Margins belong to the block that names them.
                (st.space_before, st.space_after, st.first_line) = (None, None, 0.0);
                match name {
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => st.kind = ParaKind::Heading(name.as_bytes()[1] - b'0'),
                    "blockquote" => st.kind = ParaKind::Quote,
                    "pre" => st.kind = ParaKind::Code,
                    // A paragraph in a list item keeps the item's bullet.
                    _ => {}
                }
                if let Some(a) = block_align(css).or(attr("align").and_then(align_word)) {
                    st.align = a;
                }
                para_css(&mut st, css, self.size_pt(), !matches!(name, "body" | "html"));
                self.open_block(name, st);
                if name == "pre" {
                    self.pre += 1;
                }
                // Headings say their look themselves; their own tags may add to it.
                let mut a = self.attrs();
                inline_css(&mut a, css);
                self.push_inline((name.to_string(), a));
            }
            _ => {
                let mut a = self.attrs();
                match name {
                    "b" | "strong" => a.bold = true,
                    "i" | "em" | "cite" | "var" | "dfn" => a.italic = true,
                    "u" | "ins" => a.underline = true,
                    "s" | "strike" | "del" => a.strike = true,
                    "sup" => a.superscript = true,
                    "sub" => a.subscript = true,
                    "code" | "tt" | "kbd" | "samp" => a.code = true,
                    "a" => {
                        if let Some(h) = attr("href").map(str::trim).filter(|h| !h.is_empty() && !h.to_ascii_lowercase().starts_with("javascript:")) {
                            a.link = Some(h.into());
                            a.link_tip = attr("title").filter(|t| !t.is_empty()).map(Into::into);
                        }
                    }
                    "font" => {
                        if let Some(f) = attr("face").and_then(family) {
                            a.font = Some(f.into());
                        }
                        if let Some(c) = attr("color").and_then(color) {
                            a.color = Some(c);
                        }
                        if let Some(n) = attr("size").and_then(|s| s.trim().parse::<usize>().ok()) {
                            let pt = [8.0, 10.0, 12.0, 14.0, 18.0, 24.0, 36.0][n.clamp(1, 7) - 1];
                            a.size = Some(half_points(pt));
                        }
                    }
                    // Tags that hold nothing (or that we don't know) change nothing.
                    "wbr" | "input" | "meta" | "link" | "col" | "area" | "base" | "source" | "track" | "embed" | "param" => return,
                    _ => {}
                }
                inline_css(&mut a, css);
                self.push_inline((name.to_string(), a));
            }
        }
    }

    fn end_tag(&mut self, name: &str) {
        match name {
            "table" if self.tables_skipped > 0 => self.tables_skipped -= 1,
            "table" => {
                self.end_table();
            }
            "tr" => self.end_row(),
            "td" | "th" => {
                self.close_inline(name);
                self.end_cell();
                if let Some(i) = self.blocks.iter().rposition(|(t, _)| t == name) {
                    self.blocks.truncate(i);
                }
            }
            "thead" => {
                if let Some(t) = self.tables.last_mut() {
                    t.in_head = false;
                }
            }
            "ul" | "ol" | "dl" => {
                self.close_block("li");
                self.close_block(name);
                self.close_inline(name);
                self.lists.pop();
            }
            "li" => {
                self.close_block("li");
                self.close_inline("li");
            }
            _ if is_block(name) => {
                if name == "pre" {
                    self.pre = self.pre.saturating_sub(1);
                }
                self.close_inline(name);
                self.close_block(name);
            }
            _ => self.close_inline(name),
        }
    }
}

fn is_block(name: &str) -> bool {
    matches!(
        name,
        "p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "blockquote" | "pre" | "section" | "article" | "header" | "footer" | "main" | "nav" | "aside" | "address" | "figure" | "figcaption" | "dt" | "dd" | "center" | "body" | "html" | "caption" | "details" | "summary"
    )
}

/// Where a tag ends: the '>' outside quoted attribute values.
pub(crate) fn tag_end(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    if b.len() < 2 || !(b[1].is_ascii_alphabetic() || b[1] == b'/') {
        return None;
    }
    let mut quote = 0u8;
    for (i, &c) in b.iter().enumerate().skip(1) {
        match c {
            b'"' | b'\'' if quote == 0 => quote = c,
            _ if c == quote => quote = 0,
            b'>' if quote == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

/// A start tag's name and attributes (names lowercase, values decoded).
pub(crate) fn split_tag(inner: &str) -> (&str, Vec<(String, String)>) {
    let inner = inner.trim_end_matches('/');
    let name_end = inner.find(|c: char| c.is_whitespace() || c == '/').unwrap_or(inner.len());
    let (name, mut rest) = inner.split_at(name_end);
    let mut attrs = Vec::new();
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '/');
        if rest.is_empty() {
            break;
        }
        let k_end = rest.find(|c: char| c.is_whitespace() || c == '=').unwrap_or(rest.len());
        let key = rest[..k_end].to_ascii_lowercase();
        rest = rest[k_end..].trim_start();
        let value = if let Some(r) = rest.strip_prefix('=') {
            let r = r.trim_start();
            let (v, after) = match r.chars().next() {
                Some(q @ ('"' | '\'')) => {
                    let e = r[1..].find(q).map_or(r.len(), |e| e + 1);
                    (&r[1..e], r.get(e + 1..).unwrap_or(""))
                }
                _ => {
                    let e = r.find(char::is_whitespace).unwrap_or(r.len());
                    (&r[..e], &r[e..])
                }
            };
            rest = after;
            decode(v)
        } else {
            String::new()
        };
        if key.is_empty() {
            break;
        }
        attrs.push((key, value));
    }
    (name, attrs)
}

/// The declarations of a `style` attribute, names lowercase.
pub(crate) fn declarations(css: &str) -> impl Iterator<Item = (String, &str)> {
    css.split(';').filter_map(|d| {
        let (k, v) = d.split_once(':')?;
        Some((k.trim().to_ascii_lowercase(), v.trim().trim_end_matches("!important").trim()))
    })
}

fn block_align(css: &str) -> Option<Align> {
    declarations(css).filter(|(k, _)| k == "text-align").filter_map(|(_, v)| align_word(v)).last()
}

pub(crate) fn align_word(v: &str) -> Option<Align> {
    match v.trim().to_ascii_lowercase().as_str() {
        "left" | "start" => Some(Align::Left),
        "center" | "middle" => Some(Align::Center),
        "right" | "end" => Some(Align::Right),
        "justify" => Some(Align::Justify),
        _ => None,
    }
}

/// The list style a `type` attribute names.
fn list_type(l: &mut ListB, t: &str) {
    let format = match t.trim() {
        "1" => 1,
        "a" => 3,
        "A" => 4,
        "i" => 5,
        "I" => 6,
        t => {
            list_css(l, &format!("list-style-type:{t}"));
            return;
        }
    };
    (l.numbered, l.format, l.unmarked) = (true, format, false);
}

/// The list style of a `style` attribute: `list-style-type`, or the type
/// in `list-style`. A string mark is a bullet of that glyph, the dash
/// when it is one.
fn list_css(l: &mut ListB, css: &str) {
    for (k, v) in declarations(css) {
        let words: Vec<&str> = match k.as_str() {
            "list-style-type" => vec![v],
            "list-style" => v.split_whitespace().collect(),
            _ => continue,
        };
        for w in words {
            let (numbered, format) = match w.to_ascii_lowercase().as_str() {
                "disc" => (false, 1),
                "circle" => (false, 2),
                "square" => (false, 3),
                "decimal" | "decimal-leading-zero" => (true, 1),
                "lower-alpha" | "lower-latin" => (true, 3),
                "upper-alpha" | "upper-latin" => (true, 4),
                "lower-roman" => (true, 5),
                "upper-roman" => (true, 6),
                "none" => {
                    l.unmarked = true;
                    continue;
                }
                s if s.starts_with(['\'', '"']) => {
                    let format = match s.trim_matches(['\'', '"', ' ']).chars().next() {
                        Some('-' | '–' | '—' | '‐' | '−') => 4,
                        Some('◦') => 2,
                        Some('■' | '▪') => 3,
                        Some('✓' | '✔') => 5,
                        Some('➢' | '►' | '▶' | '>') => 6,
                        _ => 1,
                    };
                    (false, format)
                }
                _ => continue,
            };
            (l.numbered, l.format, l.unmarked) = (numbered, format, false);
        }
    }
}

/// A CSS length in points; `size` is the text's (for `em`). Plain numbers
/// are pixels.
fn length_pt(v: &str, size: f32) -> Option<f32> {
    let v = v.trim().to_ascii_lowercase();
    let num = |s: &str| s.trim().parse::<f32>().ok().filter(|n| n.is_finite());
    let pt = if let Some(n) = v.strip_suffix("pt") {
        num(n)?
    } else if let Some(n) = v.strip_suffix("px") {
        num(n)? * 0.75
    } else if let Some(n) = v.strip_suffix("rem") {
        num(n)? * BASE_SIZE
    } else if let Some(n) = v.strip_suffix("em") {
        num(n)? * size
    } else if let Some(n) = v.strip_suffix("cm") {
        num(n)? * 72.0 / 2.54
    } else if let Some(n) = v.strip_suffix("mm") {
        num(n)? * 72.0 / 25.4
    } else if let Some(n) = v.strip_suffix("in") {
        num(n)? * 72.0
    } else {
        num(&v)? * 0.75
    };
    Some(pt.clamp(-1000.0, 1000.0))
}

/// Paragraph formatting of a `style` attribute: the line height and, with
/// `margins`, the space before and after and the first line's indent.
/// `size` is the text's, in points.
fn para_css(st: &mut ParaStyle, css: &str, size: f32, margins: bool) {
    let space = |v: &str| length_pt(v, size).map(|p| p.clamp(0.0, 200.0));
    for (k, v) in declarations(css) {
        let lv = v.to_ascii_lowercase();
        match k.as_str() {
            "line-height" => {
                let num = |s: &str| s.trim().parse::<f32>().ok().filter(|n| n.is_finite() && *n > 0.0);
                if lv == "normal" || lv == "initial" {
                    (st.line_spacing, st.line_rule) = (0.0, LineRule::Multiple);
                } else if let Some(n) = num(&lv) {
                    (st.line_spacing, st.line_rule) = (n.clamp(0.25, 10.0), LineRule::Multiple);
                } else if let Some(n) = lv.strip_suffix('%').and_then(num) {
                    (st.line_spacing, st.line_rule) = ((n / 100.0).clamp(0.25, 10.0), LineRule::Multiple);
                } else if let Some(pt) = length_pt(&lv, size).filter(|pt| *pt > 0.0) {
                    (st.line_spacing, st.line_rule) = (pt, LineRule::Exact);
                }
            }
            "margin-top" if margins => st.space_before = space(&lv).or(st.space_before),
            "margin-bottom" if margins => st.space_after = space(&lv).or(st.space_after),
            // Top, then bottom when given apart (`top sides bottom ...`).
            "margin" if margins => {
                let parts: Vec<&str> = lv.split_whitespace().collect();
                if (1..=4).contains(&parts.len()) {
                    st.space_before = space(parts[0]).or(st.space_before);
                    st.space_after = space(parts[if parts.len() > 2 { 2 } else { 0 }]).or(st.space_after);
                }
            }
            "text-indent" if margins => st.first_line = length_pt(&lv, size).unwrap_or(st.first_line),
            _ => {}
        }
    }
}

fn half_points(pt: f32) -> u16 {
    (pt * 2.0).round().clamp(2.0, 3276.0) as u16
}

/// Character formatting of a `style` attribute.
pub(crate) fn inline_css(a: &mut Attrs, css: &str) {
    for (k, v) in declarations(css) {
        let lv = v.to_ascii_lowercase();
        match k.as_str() {
            "font-weight" => match lv.as_str() {
                "bold" | "bolder" => a.bold = true,
                "normal" | "lighter" => a.bold = false,
                n => {
                    if let Ok(w) = n.parse::<u32>() {
                        a.bold = w >= 600;
                    }
                }
            },
            "font-style" => a.italic = lv.starts_with("italic") || lv.starts_with("oblique"),
            "text-decoration" | "text-decoration-line" => {
                if lv.contains("underline") {
                    a.underline = true;
                }
                if lv.contains("line-through") {
                    a.strike = true;
                }
                if lv == "none" {
                    a.underline = false;
                    a.strike = false;
                }
            }
            "color" => {
                if let Some(c) = color(&lv) {
                    a.color = Some(c);
                }
            }
            "background-color" | "background" => {
                if let Some(c) = color(&lv) {
                    a.highlight = Some(c);
                }
            }
            "font-family" => {
                if let Some(f) = family(v) {
                    a.font = Some(f.into());
                }
            }
            "font-size" => {
                let current = a.size.map_or(BASE_SIZE, |s| f32::from(s) / 2.0);
                if let Some(pt) = font_size(&lv, current) {
                    a.size = Some(half_points(pt));
                }
            }
            "vertical-align" => match lv.as_str() {
                "super" => a.superscript = true,
                "sub" => a.subscript = true,
                "baseline" => {
                    a.superscript = false;
                    a.subscript = false;
                }
                _ => {}
            },
            _ => {}
        }
    }
}

/// The first family of a `font-family` list that is a real font.
pub(crate) fn family(v: &str) -> Option<String> {
    v.split(',').map(|f| decode(f.trim().trim_matches(|c| c == '"' || c == '\'')).trim().to_string()).find(|f| {
        !f.is_empty()
            && !matches!(
                f.to_ascii_lowercase().as_str(),
                "serif" | "sans-serif" | "monospace" | "cursive" | "fantasy" | "system-ui" | "ui-sans-serif" | "ui-serif" | "ui-monospace" | "-apple-system" | "blinkmacsystemfont" | "inherit" | "initial" | "unset"
            )
    })
}

fn font_size(v: &str, current: f32) -> Option<f32> {
    let num = |s: &str| s.trim().parse::<f32>().ok().filter(|n| n.is_finite() && *n > 0.0);
    let pt = if let Some(n) = v.strip_suffix("pt") {
        num(n)?
    } else if let Some(n) = v.strip_suffix("px") {
        num(n)? * 0.75
    } else if let Some(n) = v.strip_suffix("rem") {
        num(n)? * BASE_SIZE
    } else if let Some(n) = v.strip_suffix("em") {
        num(n)? * current
    } else if let Some(n) = v.strip_suffix('%') {
        num(n)? * current / 100.0
    } else {
        match v {
            "xx-small" => 7.0,
            "x-small" => 7.5,
            "small" => 10.0,
            "medium" => 12.0,
            "large" => 13.5,
            "x-large" => 18.0,
            "xx-large" => 24.0,
            "xxx-large" => 36.0,
            "smaller" => current / 1.2,
            "larger" => current * 1.2,
            _ => return None,
        }
    };
    Some(pt.clamp(1.0, 1638.0))
}

/// A CSS or HTML color as 0xRRGGBB; `None` for transparent or unknown.
pub(crate) fn color(v: &str) -> Option<u32> {
    let v = v.trim().to_ascii_lowercase();
    if let Some(h) = v.strip_prefix('#') {
        let h = h.split_whitespace().next()?;
        return match h.len() {
            3 | 4 => {
                let d: Vec<u32> = h.chars().take(3).map(|c| c.to_digit(16)).collect::<Option<_>>()?;
                Some(d[0] * 0x11 << 16 | d[1] * 0x11 << 8 | d[2] * 0x11)
            }
            6 | 8 => u32::from_str_radix(h.get(..6)?, 16).ok(),
            _ => None,
        };
    }
    if let Some(args) = v.strip_prefix("rgba(").or_else(|| v.strip_prefix("rgb(")) {
        let args = args.split(')').next()?;
        let parts: Vec<&str> = args.split(|c: char| c == ',' || c == '/' || c.is_whitespace()).filter(|s| !s.is_empty()).collect();
        if parts.len() < 3 {
            return None;
        }
        if parts.get(3).and_then(|a| a.trim_end_matches('%').parse::<f32>().ok()).is_some_and(|a| a == 0.0) {
            return None;
        }
        let ch = |s: &str| -> Option<u32> {
            Some(if let Some(p) = s.strip_suffix('%') { (p.parse::<f32>().ok()? * 2.55).round() } else { s.parse::<f32>().ok()?.round() }.clamp(0.0, 255.0) as u32)
        };
        return Some(ch(parts[0])? << 16 | ch(parts[1])? << 8 | ch(parts[2])?);
    }
    // The background shorthand may carry more than the color.
    let word = v.split_whitespace().next()?;
    Some(match word {
        "black" => 0x000000,
        "white" => 0xFFFFFF,
        "red" => 0xFF0000,
        "green" => 0x008000,
        "lime" => 0x00FF00,
        "blue" => 0x0000FF,
        "yellow" => 0xFFFF00,
        "cyan" | "aqua" => 0x00FFFF,
        "magenta" | "fuchsia" => 0xFF00FF,
        "gray" | "grey" => 0x808080,
        "silver" => 0xC0C0C0,
        "maroon" => 0x800000,
        "olive" => 0x808000,
        "navy" => 0x000080,
        "purple" => 0x800080,
        "teal" => 0x008080,
        "orange" => 0xFFA500,
        "brown" => 0xA52A2A,
        "pink" => 0xFFC0CB,
        "darkred" => 0x8B0000,
        "darkblue" => 0x00008B,
        "darkgreen" => 0x006400,
        "lightgray" | "lightgrey" => 0xD3D3D3,
        "darkgray" | "darkgrey" => 0xA9A9A9,
        _ => return None,
    })
}

/// A picture in a data URI, `width` and `height` in CSS pixels.
fn data_image(src: &str, alt: &str, width: Option<&str>, height: Option<&str>) -> Option<Image> {
    let uri = src.trim().strip_prefix("data:")?;
    let (_, b64) = uri.split_once(";base64,")?;
    let data = crate::markdown::base64_decode(b64)?;
    let (w, h) = crate::odt_img::pixel_size(&data)?;
    let px = |s: Option<&str>| s.and_then(|s| s.trim().trim_end_matches("px").parse::<f32>().ok()).filter(|n| *n > 0.0);
    let (mut fw, mut fh) = (w as f32, h as f32);
    match (px(width), px(height)) {
        (Some(a), Some(b)) => (fw, fh) = (a, b),
        (Some(a), None) => (fw, fh) = (a, fh * a / fw.max(1.0)),
        (None, Some(b)) => (fw, fh) = (fw * b / fh.max(1.0), b),
        _ => {}
    }
    Some(Image { alt: alt.to_string(), ..Image::new(Arc::from(data), fw * 0.75, fh * 0.75) })
}

/// Latin-1's named entities, from U+00A0.
const LATIN1: [&str; 96] = [
    "nbsp", "iexcl", "cent", "pound", "curren", "yen", "brvbar", "sect", "uml", "copy", "ordf", "laquo", "not", "shy", "reg", "macr", "deg", "plusmn", "sup2", "sup3", "acute", "micro", "para", "middot", "cedil", "sup1", "ordm", "raquo", "frac14", "frac12", "frac34", "iquest", "Agrave", "Aacute", "Acirc", "Atilde", "Auml", "Aring", "AElig",
    "Ccedil", "Egrave", "Eacute", "Ecirc", "Euml", "Igrave", "Iacute", "Icirc", "Iuml", "ETH", "Ntilde", "Ograve", "Oacute", "Ocirc", "Otilde", "Ouml", "times", "Oslash", "Ugrave", "Uacute", "Ucirc", "Uuml", "Yacute", "THORN", "szlig", "agrave", "aacute", "acirc", "atilde", "auml", "aring", "aelig", "ccedil", "egrave", "eacute", "ecirc", "euml", "igrave",
    "iacute", "icirc", "iuml", "eth", "ntilde", "ograve", "oacute", "ocirc", "otilde", "ouml", "divide", "oslash", "ugrave", "uacute", "ucirc", "uuml", "yacute", "thorn", "yuml",
];

fn entity(name: &str) -> Option<char> {
    if let Some(n) = name.strip_prefix('#') {
        let code = match n.strip_prefix(['x', 'X']) {
            Some(h) => u32::from_str_radix(h, 16).ok()?,
            None => n.parse().ok()?,
        };
        return char::from_u32(code).filter(|&c| c != '\0');
    }
    if let Some(i) = LATIN1.iter().position(|&e| e == name) {
        return char::from_u32(0xA0 + i as u32);
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "ndash" => '–',
        "mdash" => '—',
        "lsquo" => '‘',
        "rsquo" => '’',
        "sbquo" => '‚',
        "ldquo" => '“',
        "rdquo" => '”',
        "bdquo" => '„',
        "hellip" => '…',
        "bull" => '•',
        "euro" => '€',
        "trade" => '™',
        "dagger" => '†',
        "Dagger" => '‡',
        "permil" => '‰',
        "lsaquo" => '‹',
        "rsaquo" => '›',
        "OElig" => 'Œ',
        "oelig" => 'œ',
        "Scaron" => 'Š',
        "scaron" => 'š',
        "Yuml" => 'Ÿ',
        "fnof" => 'ƒ',
        "circ" => 'ˆ',
        "tilde" => '˜',
        "ensp" | "emsp" | "thinsp" => ' ',
        "zwnj" => '\u{200C}',
        "zwj" => '\u{200D}',
        "larr" => '←',
        "rarr" => '→',
        "uarr" => '↑',
        "darr" => '↓',
        "le" => '≤',
        "ge" => '≥',
        "ne" => '≠',
        "minus" => '−',
        "infin" => '∞',
        _ => return None,
    })
}

/// Text with its character references replaced.
pub(crate) fn decode(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest[1..].find(|c: char| !(c.is_ascii_alphanumeric() || c == '#')).map_or(rest.len(), |e| e + 1);
        match entity(&rest[1..end]) {
            Some(ch) if end > 1 => {
                out.push(ch);
                rest = &rest[end..];
                if let Some(r) = rest.strip_prefix(';') {
                    rest = r;
                }
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Where `close` (lowercase ASCII) next is in `s`, in any case.
fn find_ci(s: &str, close: &str) -> Option<usize> {
    let (s, n) = (s.as_bytes(), close.as_bytes());
    s.windows(n.len()).position(|w| w.eq_ignore_ascii_case(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paras(html: &str) -> Vec<Paragraph> {
        read(html).paragraphs
    }

    fn runs(p: &Paragraph) -> Vec<(&str, &Attrs)> {
        let mut at = 0;
        p.runs
            .iter()
            .map(|r| {
                let t = &p.text[at..at + r.len];
                at += r.len;
                (t, &r.attrs)
            })
            .collect()
    }

    #[test]
    fn latin1_names_line_up() {
        assert_eq!(entity("eacute"), Some('é'));
        assert_eq!(entity("yuml"), Some('ÿ'));
        assert_eq!(entity("times"), Some('×'));
        assert_eq!(decode("a&amp;b &lt;c&gt; &#233;&#xE9; &bogus; x&y"), "a&b <c> éé &bogus; x&y");
    }

    #[test]
    fn a_browser_fragment_keeps_its_character_formatting() {
        let html = r#"<meta charset="utf-8"><span style="color: rgb(32, 33, 36); font-family: Arial, sans-serif; font-size: 16px;">Plain <b>bold</b> <i>italic</i> <u>under</u> <a href="https://example.com" title="Ex">link</a></span>"#;
        let p = paras(html);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].text, "Plain bold italic under link");
        let r = runs(&p[0]);
        let get = |t: &str| r.iter().find(|(x, _)| *x == t).map(|(_, a)| *a).unwrap();
        assert!(get("bold").bold && !get("Plain ").bold);
        assert!(get("italic").italic);
        assert!(get("under").underline);
        assert_eq!(get("link").link.as_deref(), Some("https://example.com"));
        assert_eq!(get("link").link_tip.as_deref(), Some("Ex"));
        assert_eq!(get("Plain ").color, Some(0x202124));
        assert_eq!(get("Plain ").font.as_deref(), Some("Arial"));
        assert_eq!(get("Plain ").size, Some(24));
    }

    #[test]
    fn blocks_lists_and_headings() {
        let html = "<h2>Title</h2>\n<p style=\"text-align:center\">One\n  two</p><ul><li>a<ul><li>b</li></ul></li><li>c</ul><ol><li>first</ol><blockquote>q</blockquote><pre>x  y\nz</pre><p>line<br>break</p>";
        let p = paras(html);
        let summary: Vec<(String, ParaKind, u8)> = p.iter().map(|p| (p.text.clone(), p.style.kind, p.style.level)).collect();
        assert_eq!(
            summary,
            vec![
                ("Title".into(), ParaKind::Heading(2), 0),
                ("One two".into(), ParaKind::Normal, 0),
                ("a".into(), ParaKind::Bullet, 0),
                ("b".into(), ParaKind::Bullet, 1),
                ("c".into(), ParaKind::Bullet, 0),
                ("first".into(), ParaKind::Numbered, 0),
                ("q".into(), ParaKind::Quote, 0),
                ("x  y".into(), ParaKind::Code, 0),
                ("z".into(), ParaKind::Code, 0),
                (format!("line{LINE_BREAK}break"), ParaKind::Normal, 0),
            ]
        );
        assert_eq!(p[1].style.align, Align::Center);
    }

    #[test]
    fn tables_become_tables() {
        let html = "<table><thead><tr><th>Name</th><th>Qty</th></tr></thead><tbody><tr><td>Apples</td><td><b>3</b></td></tr><tr><td colspan=2>All</td></tr></tbody></table><p>after</p>";
        let doc = read(html);
        let cells: Vec<&str> = doc.paragraphs.iter().filter(|p| p.cell.is_some()).map(|p| p.text.as_str()).collect();
        assert_eq!(cells, ["Name", "Qty", "Apples", "3", "All", ""]);
        assert_eq!(doc.paragraphs.last().unwrap().text, "after");
        let m = doc.table_map(0).expect("a table");
        assert_eq!(m.props(&doc.paragraphs).map(|p| p.header_rows), Some(1));
    }

    #[test]
    fn windows_header_and_fragment_marks() {
        let html = "Version:0.9\r\nStartHTML:0000000105\r\nEndHTML:0000000220\r\n<html><body>\r\n<!--StartFragment--><p>Inside</p><!--EndFragment-->\r\n<p>Outside</p></body></html>";
        let p = paras(html);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].text, "Inside");
    }

    #[test]
    fn head_style_and_script_are_not_text() {
        let html = "<html><head><title>T</title><style>p{color:red}</style></head><body><script>var a = '<p>';</script><p>Body&nbsp;text</p></body></html>";
        let p = paras(html);
        assert_eq!(p.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(), ["Body text"]);
    }

    #[test]
    fn google_docs_wrapper_is_not_bold() {
        let html = r#"<b style="font-weight:normal;" id="docs-internal-guid-1"><p><span style="font-weight:700">Bold</span><span style="font-weight:400"> plain</span></p></b>"#;
        let p = paras(html);
        let r = runs(&p[0]);
        assert!(r[0].1.bold && r[0].0 == "Bold");
        assert!(!r[1].1.bold);
    }

    #[test]
    fn colors_and_sizes() {
        assert_eq!(color("#abc"), Some(0xAABBCC));
        assert_eq!(color("#A0B1C2"), Some(0xA0B1C2));
        assert_eq!(color("rgba(0, 0, 0, 0)"), None);
        assert_eq!(color("rgb(255 0 0)"), Some(0xFF0000));
        assert_eq!(color("transparent"), None);
        assert_eq!(font_size("11pt", 12.0), Some(11.0));
        assert_eq!(font_size("200%", 12.0), Some(24.0));
        assert_eq!(font_size("1.5em", 10.0), Some(15.0));
    }

    #[test]
    fn browser_lists_keep_their_marks() {
        // Chrome on Windows: the header, then the fragment with the styles
        // the page gave each tag.
        let html = "Version:0.9\r\nStartHTML:0000000105\r\nEndHTML:0000000999\r\nStartFragment:0000000141\r\nEndFragment:0000000900\r\nSourceURL:https://example.com/\r\n<html>\r\n<body>\r\n<!--StartFragment--><ul style=\"margin: 0px 0px 16px; padding-left: 2em; color: rgb(31, 35, 40); font-family: -apple-system, &quot;Segoe UI&quot;, sans-serif; font-size: 16px;\"><li style=\"margin-top: 0.25em;\">Disc item</li><li>Second<ul style=\"list-style-type: circle;\"><li>Nested</li></ul></li></ul><ul style=\"list-style-type: '- ';\"><li>Dash item</li></ul><ul style=\"list-style: square inside;\"><li>Square</li></ul><ol type=\"a\" start=\"3\"><li>Third letter</li><li>Fourth</li></ol><ol style=\"list-style-type: upper-roman;\"><li>Roman</li></ol><ul style=\"list-style-type: none;\"><li>Unmarked</li></ul><p>- typed dash</p><p>– typed en dash</p><!--EndFragment-->\r\n</body>\r\n</html>";
        let p = paras(html);
        let summary: Vec<(&str, ParaKind, u8, u8)> = p.iter().map(|p| (p.text.as_str(), p.style.kind, p.style.level, p.style.list_format)).collect();
        assert_eq!(
            summary,
            [
                ("Disc item", ParaKind::Bullet, 0, 0),
                ("Second", ParaKind::Bullet, 0, 0),
                ("Nested", ParaKind::Bullet, 1, 2),
                ("Dash item", ParaKind::Bullet, 0, 4),
                ("Square", ParaKind::Bullet, 0, 3),
                ("Third letter", ParaKind::Numbered, 0, 3),
                ("Fourth", ParaKind::Numbered, 0, 3),
                ("Roman", ParaKind::Numbered, 0, 6),
                ("Unmarked", ParaKind::Normal, 0, 0),
                ("- typed dash", ParaKind::Normal, 0, 0),
                ("– typed en dash", ParaKind::Normal, 0, 0),
            ]
        );
        assert_eq!((p[5].style.start_at, p[6].style.start_at), (3, 0));
        assert!(p[8].style.indent_left > 0.0);
        // The item's own margin, in the text's size (16px = 12pt).
        assert_eq!(p[0].style.space_before, Some(3.0));
        assert_eq!(p[1].style.space_before, None);
        // Firefox: a list item with the `disc` it names.
        let p = paras("<ul><li style=\"list-style-type: disc\">one</li></ul>");
        assert_eq!((p[0].style.kind, p[0].style.list_format), (ParaKind::Bullet, 1));
    }

    #[test]
    fn line_height_and_paragraph_spacing() {
        // Google Docs, then a page's paragraphs and line breaks.
        let html = "<meta charset=\"utf-8\"><b style=\"font-weight:normal;\" id=\"docs-internal-guid-1\"><p dir=\"ltr\" style=\"line-height:1.38;margin-top:0pt;margin-bottom:0pt;\"><span style=\"font-size:11pt;\">Docs line</span></p></b><p style=\"line-height: 1.5; margin: 0px 0px 1.5em;\">One<br>two</p><p style=\"line-height: 24px; margin-top: 12pt; text-indent: 2em;\">Fixed</p><div style=\"line-height: 200%\"><p>Inherits</p></div><ul style=\"line-height: 1.8; margin-bottom: 20px\"><li style=\"margin-bottom: 6px\">Item</li></ul>";
        let p = paras(html);
        let t: Vec<&str> = p.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(t, ["Docs line", "One\u{2028}two", "Fixed", "Inherits", "Item"]);
        let s = |i: usize| (p[i].style.line_spacing, p[i].style.line_rule, p[i].style.space_before, p[i].style.space_after);
        assert_eq!(s(0), (1.38, LineRule::Multiple, Some(0.0), Some(0.0)));
        assert_eq!(s(1), (1.5, LineRule::Multiple, Some(0.0), Some(18.0)));
        assert_eq!(s(2), (18.0, LineRule::Exact, Some(12.0), None));
        assert_eq!(p[2].style.first_line, 24.0);
        assert_eq!(s(3), (2.0, LineRule::Multiple, None, None));
        // A list's line height goes to its items, its margin doesn't.
        assert_eq!(s(4), (1.8, LineRule::Multiple, None, Some(4.5)));
        assert_eq!(p[4].style.kind, ParaKind::Bullet);
    }
}
