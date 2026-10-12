//! Export to HTML (documents and workbooks) and EPUB 3 (documents).
//!
//! The markup is XHTML-compatible (self-closed void elements, escaped
//! text), so the same body serves a standalone `.html` page, with pictures
//! embedded as data URIs, and the chapters of an EPUB, with pictures as
//! files of the package.

use std::collections::HashMap;
use std::fmt::Write as _;

use apex_doc::{Align, Attrs, Document, LINE_BREAK, OBJECT, ParaKind, Paragraph, TableMap, Wrap};
use apex_sheet::{CellRef, HAlign, Workbook};

use crate::docx_img::image_type;
use crate::markdown::base64_encode;
use crate::zip::ZipWriter;

fn esc(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            // Not allowed in XML.
            '\u{0}'..='\u{8}' | '\u{B}' | '\u{C}' | '\u{E}'..='\u{1F}' | '\u{FFFE}' | '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
}

fn escaped(s: &str) -> String {
    let mut o = String::new();
    esc(&mut o, s);
    o
}

fn hex(c: u32) -> String {
    format!("#{:06X}", c & 0xFF_FFFF)
}

/// Where pictures go: data URIs in a page, files in an EPUB.
enum Pictures {
    Inline,
    Files(Vec<(String, &'static str, Vec<u8>)>, HashMap<(u64, [u32; 4]), String>),
}

impl Pictures {
    /// Where `img` is: `None` when it can't be written (cropped, and
    /// it can't be cut to what shows).
    fn src(&mut self, img: &apex_doc::Image) -> Option<String> {
        let data = crate::picture::shown(img)?;
        let (ext, mime) = image_type(&data);
        Some(match self {
            Pictures::Inline => format!("data:{mime};base64,{}", base64_encode(&data)),
            Pictures::Files(files, seen) => seen
                .entry((img.data_key(), img.crop.map(f32::to_bits)))
                .or_insert_with(|| {
                    let name = format!("images/image{}.{ext}", files.len() + 1);
                    files.push((name.clone(), mime, data.to_vec()));
                    name
                })
                .clone(),
        })
    }
}

struct Writer<'a> {
    doc: &'a Document,
    out: String,
    pics: Pictures,
    /// Open lists: (kind, level).
    lists: Vec<(ParaKind, u8)>,
    in_code: bool,
    /// Heading ids for the table of contents: (level, id, text).
    headings: Vec<(u8, String, String)>,
}

impl Writer<'_> {
    fn close_lists(&mut self, to: usize) {
        while self.lists.len() > to {
            let (k, _) = self.lists.pop().unwrap_or((ParaKind::Bullet, 0));
            self.out.push_str(if k == ParaKind::Numbered { "</li></ol>\n" } else { "</li></ul>\n" });
        }
    }

    fn close_code(&mut self) {
        if self.in_code {
            self.out.push_str("</code></pre>\n");
            self.in_code = false;
        }
    }

    fn para_css(&self, p: &Paragraph, list: bool) -> String {
        let st = &p.style;
        let mut css = String::new();
        match st.align {
            Align::Center => css.push_str("text-align:center;"),
            Align::Right => css.push_str("text-align:right;"),
            Align::Justify => css.push_str("text-align:justify;"),
            _ => {}
        }
        let indent = st.indent_left + if list { 0.0 } else { st.level as f32 * 36.0 };
        if indent.abs() > 0.1 {
            let _ = write!(css, "margin-left:{indent}pt;");
        }
        if st.indent_right.abs() > 0.1 {
            let _ = write!(css, "margin-right:{}pt;", st.indent_right);
        }
        if st.first_line.abs() > 0.1 {
            let _ = write!(css, "text-indent:{}pt;", st.first_line);
        }
        if st.space_before.is_some() || st.space_after.is_some() {
            let (b, a) = st.spacing();
            let _ = write!(css, "margin-top:{b}pt;margin-bottom:{a}pt;");
        }
        if st.line_spacing > 0.0 {
            match st.line_rule {
                apex_doc::LineRule::Multiple => {
                    let _ = write!(css, "line-height:{};", st.line_spacing * 1.15);
                }
                _ => {
                    let _ = write!(css, "line-height:{}pt;", st.line_spacing);
                }
            }
        }
        if let Some(c) = st.shading {
            let _ = write!(css, "background:{};", hex(c));
        }
        if st.page_break_before {
            css.push_str("page-break-before:always;");
        }
        css
    }

    fn style_attr(css: &str) -> String {
        if css.is_empty() { String::new() } else { format!(" style=\"{css}\"") }
    }

    /// The paragraph's text with its formatting, pictures, links and
    /// bookmarks.
    fn inline(&mut self, p: &Paragraph) {
        let mut marks: Vec<(usize, &str)> = p.marks.iter().filter(|m| !m.end && !apex_doc::is_perm_mark(&m.name)).map(|m| (m.offset, &*m.name)).collect();
        marks.sort_by_key(|m| m.0);
        let mut mi = 0;
        let mut pos = 0;
        let mut open_link: Option<&str> = None;
        for run in &p.runs {
            let (start, end) = (pos, (pos + run.len).min(p.text.len()));
            pos = end;
            let Some(text) = p.text.get(start..end) else { continue };
            let a = &run.attrs;
            let link = a.link.as_deref();
            if open_link.is_some() && open_link != link {
                self.out.push_str("</a>");
                open_link = None;
            }
            // Bookmarks before this run.
            while mi < marks.len() && marks[mi].0 <= start {
                let _ = write!(self.out, "<a id=\"{}\"></a>", escaped(marks[mi].1));
                mi += 1;
            }
            if let Some(l) = link
                && open_link.is_none()
            {
                let _ = write!(self.out, "<a href=\"{}\"", escaped(&safe_href(l)));
                if let Some(t) = &a.link_tip {
                    let _ = write!(self.out, " title=\"{}\"", escaped(t));
                }
                self.out.push('>');
                open_link = Some(l);
            }
            if let Some(img) = &a.image {
                for c in text.chars() {
                    if c == OBJECT {
                        self.picture(img);
                    }
                }
            } else if let Some(eq) = &a.math {
                for _ in text.matches(OBJECT) {
                    self.out.push_str(&crate::mathml::to_mathml(eq, apex_doc::math::is_display(p), false));
                }
            } else if let Some(sh) = &a.shape {
                // Shapes keep their text only.
                for _ in text.matches(OBJECT) {
                    let t = sh.shape.plain_text();
                    if !t.trim().is_empty() {
                        let _ = write!(self.out, "<span class=\"shape\">{}</span>", escaped(&t).replace('\n', "<br/>"));
                    }
                }
            } else {
                self.run(text, a);
            }
        }
        if open_link.is_some() {
            self.out.push_str("</a>");
        }
        while mi < marks.len() {
            let _ = write!(self.out, "<a id=\"{}\"></a>", escaped(marks[mi].1));
            mi += 1;
        }
    }

    fn run(&mut self, text: &str, a: &Attrs) {
        let mut css = String::new();
        if let Some(f) = &a.font {
            let _ = write!(css, "font-family:'{}';", css_font(&crate::theme_xml::font_name(f)));
        }
        if let Some(s) = a.size {
            let _ = write!(css, "font-size:{}pt;", s as f32 / 2.0);
        }
        if let Some(c) = a.color {
            let _ = write!(css, "color:{};", hex(c));
        }
        if let Some(c) = a.highlight {
            let _ = write!(css, "background:{};", hex(c));
        }
        if let Some(fx) = &a.fx {
            if fx.small_caps {
                css.push_str("font-variant:small-caps;");
            }
            if fx.all_caps {
                css.push_str("text-transform:uppercase;");
            }
            if fx.hidden {
                css.push_str("display:none;");
            }
            if fx.spacing != 0 {
                let _ = write!(css, "letter-spacing:{}pt;", fx.spacing as f32 / 20.0);
            }
            if fx.position != 0 {
                let _ = write!(css, "position:relative;top:{}pt;", -fx.position as f32 / 2.0);
            }
            if fx.double_strike {
                css.push_str("text-decoration:line-through double;");
            }
            if a.underline && fx.underline != apex_doc::UnderlineStyle::Single {
                let st = match fx.underline {
                    apex_doc::UnderlineStyle::Double => "double",
                    apex_doc::UnderlineStyle::Dotted => "dotted",
                    apex_doc::UnderlineStyle::Dashed | apex_doc::UnderlineStyle::DotDash => "dashed",
                    apex_doc::UnderlineStyle::Wave => "wavy",
                    _ => "solid",
                };
                let _ = write!(css, "text-decoration-style:{st};");
            }
            if let Some(c) = fx.underline_color {
                let _ = write!(css, "text-decoration-color:{};", hex(c));
            }
        }
        let mut tags: Vec<&str> = Vec::new();
        if a.bold {
            tags.push("strong");
        }
        if a.italic {
            tags.push("em");
        }
        if a.underline && a.link.is_none() {
            tags.push("u");
        }
        if a.strike {
            tags.push("s");
        }
        if a.superscript {
            tags.push("sup");
        } else if a.subscript {
            tags.push("sub");
        }
        if a.code && !self.in_code {
            tags.push("code");
        }
        if !css.is_empty() {
            let _ = write!(self.out, "<span style=\"{css}\">");
        }
        for t in &tags {
            let _ = write!(self.out, "<{t}>");
        }
        for (i, piece) in text.split(LINE_BREAK).enumerate() {
            if i > 0 {
                self.out.push_str(if self.in_code { "\n" } else { "<br/>" });
            }
            let piece = if self.in_code { piece.to_string() } else { piece.replace('\t', "\u{2003}") };
            esc(&mut self.out, &piece);
        }
        for t in tags.iter().rev() {
            let _ = write!(self.out, "</{t}>");
        }
        if !css.is_empty() {
            self.out.push_str("</span>");
        }
    }

    fn picture(&mut self, img: &apex_doc::Image) {
        let Some(src) = self.pics.src(img) else { return };
        let mut css = format!("width:{}pt;height:{}pt;", img.width, img.height);
        if img.rotation != 0.0 {
            let _ = write!(css, "transform:rotate({}deg);", img.rotation);
        }
        match (img.wrap, img.align) {
            (Wrap::Square, Some(Align::Right)) => css.push_str("float:right;margin:0 0 6pt 9pt;"),
            (Wrap::Square, _) => css.push_str("float:left;margin:0 9pt 6pt 0;"),
            (Wrap::TopBottom, Some(Align::Center)) => css.push_str("display:block;margin:6pt auto;"),
            (Wrap::TopBottom, Some(Align::Right)) => css.push_str("display:block;margin:6pt 0 6pt auto;"),
            (Wrap::TopBottom, _) => css.push_str("display:block;margin:6pt 0;"),
            _ => {}
        }
        let _ = write!(self.out, "<img src=\"{}\" alt=\"{}\" style=\"{css}\"/>", escaped(&src), escaped(&img.alt));
    }

    fn paragraph(&mut self, idx: usize) {
        let doc = self.doc;
        let p = &doc.paragraphs[idx];
        let kind = p.style.kind;
        if kind != ParaKind::Code {
            self.close_code();
        }
        if kind.is_list() {
            let level = p.style.level;
            // Close deeper lists, and a list of another kind at this level.
            while let Some(&(k, l)) = self.lists.last() {
                if l > level || (l == level && k != kind) {
                    self.close_lists(self.lists.len() - 1);
                } else {
                    break;
                }
            }
            let same = self.lists.last().is_some_and(|&(k, l)| k == kind && l == level);
            if same {
                self.out.push_str("</li>\n");
            } else {
                let fmt = p.style.list_format;
                let tag = if kind == ParaKind::Numbered {
                    let t = match apex_doc::num_format(fmt, level) {
                        3 => "a",
                        4 => "A",
                        5 => "i",
                        6 => "I",
                        _ => "1",
                    };
                    let start = doc.list_number(idx);
                    if start > 1 { format!("<ol type=\"{t}\" start=\"{start}\">") } else { format!("<ol type=\"{t}\">") }
                } else {
                    let st = match apex_doc::bullet_char(fmt, level) {
                        '◦' => "circle",
                        '■' | '▪' => "square",
                        _ => "disc",
                    };
                    format!("<ul style=\"list-style-type:{st}\">")
                };
                self.out.push_str(&tag);
                self.out.push('\n');
                self.lists.push((kind, level));
            }
            let css = self.para_css(p, true);
            let _ = write!(self.out, "<li{}>", Self::style_attr(&css));
            self.inline(p);
            return;
        }
        self.close_lists(0);
        let css = self.para_css(p, false);
        let sa = Self::style_attr(&css);
        match kind {
            ParaKind::Title | ParaKind::Heading(_) => {
                let n = if let ParaKind::Heading(n) = kind { n.clamp(1, 6) } else { 1 };
                let id = format!("h{idx}");
                let class = if kind == ParaKind::Title { " class=\"title\"" } else { "" };
                let _ = write!(self.out, "<h{n} id=\"{id}\"{class}{sa}>");
                self.inline(p);
                let _ = writeln!(self.out, "</h{n}>");
                let text: String = p.text.chars().filter(|&c| c != OBJECT).map(|c| if c == LINE_BREAK { ' ' } else { c }).collect();
                if !text.trim().is_empty() {
                    self.headings.push((if kind == ParaKind::Title { 1 } else { n }, id, text.trim().to_string()));
                }
            }
            ParaKind::Quote => {
                let _ = write!(self.out, "<blockquote{sa}><p>");
                self.inline(p);
                self.out.push_str("</p></blockquote>\n");
            }
            ParaKind::Code => {
                if !self.in_code {
                    self.out.push_str("<pre><code>");
                    self.in_code = true;
                } else {
                    self.out.push('\n');
                }
                self.inline(p);
            }
            _ => {
                let _ = write!(self.out, "<p{sa}>");
                if p.text.is_empty() {
                    self.out.push_str("<br/>");
                }
                self.inline(p);
                self.out.push_str("</p>\n");
            }
        }
    }

    fn table(&mut self, m: &TableMap) {
        self.close_code();
        self.close_lists(0);
        let doc = self.doc;
        let props = m.props(&doc.paragraphs);
        let header_rows = props.map_or(0, |p| p.header_rows as usize);
        let bordered = props.is_none_or(|p| p.style.is_some() || p.borders.top.is_some() || p.borders.inside_h.is_some());
        let mut css = String::from("border-collapse:collapse;");
        if let Some(p) = props {
            match p.align {
                Align::Center => css.push_str("margin-left:auto;margin-right:auto;"),
                Align::Right => css.push_str("margin-left:auto;"),
                _ => {}
            }
        }
        let _ = writeln!(self.out, "<table style=\"{css}\">");
        if let Some(p) = props
            && !p.widths.is_empty()
        {
            self.out.push_str("<colgroup>");
            for w in &p.widths {
                let _ = write!(self.out, "<col style=\"width:{w}pt\"/>");
            }
            self.out.push_str("</colgroup>\n");
        }
        for r in 0..m.rows {
            if r == 0 && header_rows > 0 {
                self.out.push_str("<thead>\n");
            }
            if r == header_rows && header_rows > 0 {
                self.out.push_str("<tbody>\n");
            }
            self.out.push_str("<tr>");
            for c in 0..m.cols {
                if !m.is_anchor(r, c) {
                    continue;
                }
                let (a, b) = m.cell(r, c);
                let (rs, cs) = m.span_of(r, c);
                let cell = m.cell_props(&doc.paragraphs, r, c);
                let tag = if r < header_rows { "th" } else { "td" };
                let mut ccss = String::from("vertical-align:top;padding:2pt 5pt;");
                if bordered {
                    ccss.push_str("border:0.5pt solid #808080;");
                }
                if let Some(cp) = cell {
                    if let Some(s) = cp.shading {
                        let _ = write!(ccss, "background:{};", hex(s));
                    }
                    match cp.valign {
                        apex_doc::VAlign::Center => ccss = ccss.replace("vertical-align:top", "vertical-align:middle"),
                        apex_doc::VAlign::Bottom => ccss = ccss.replace("vertical-align:top", "vertical-align:bottom"),
                        _ => {}
                    }
                }
                let _ = write!(self.out, "<{tag}");
                if rs > 1 {
                    let _ = write!(self.out, " rowspan=\"{rs}\"");
                }
                if cs > 1 {
                    let _ = write!(self.out, " colspan=\"{cs}\"");
                }
                let _ = write!(self.out, " style=\"{ccss}\">");
                let mut i = a;
                while i < b {
                    // Tables nested in the cell.
                    let inner = doc.paragraphs[i].cell.as_ref().and_then(|t| t.at(m.depth + 1)).is_some();
                    match inner.then(|| TableMap::build_at(&doc.paragraphs, i, m.depth + 1)).flatten() {
                        Some(n) => {
                            self.table(&n);
                            i = n.end.max(i + 1);
                        }
                        None => {
                            self.paragraph(i);
                            i += 1;
                        }
                    }
                }
                self.close_code();
                self.close_lists(0);
                let _ = write!(self.out, "</{tag}>");
            }
            self.out.push_str("</tr>\n");
            if r + 1 == header_rows {
                self.out.push_str("</thead>\n");
            }
        }
        if header_rows > 0 && header_rows < m.rows {
            self.out.push_str("</tbody>\n");
        }
        self.out.push_str("</table>\n");
    }

    fn body(&mut self) {
        let doc = self.doc;
        let tables: HashMap<usize, TableMap> = doc.tables().into_iter().map(|m| (m.start, m)).collect();
        let mut i = 0;
        while i < doc.paragraphs.len() {
            if let Some(m) = tables.get(&i) {
                self.table(m);
                i = m.end.max(i + 1);
                continue;
            }
            if doc.paragraphs[i].cell.is_some() {
                i += 1;
                continue;
            }
            self.paragraph(i);
            i += 1;
        }
        self.close_code();
        self.close_lists(0);
    }
}

/// A link of the document for a web page: one that runs script there
/// (`javascript:`, `vbscript:`, `data:`) is left out.
pub(crate) fn safe_href(l: &str) -> String {
    let t: String = l.trim().chars().filter(|c| !c.is_control() && !c.is_whitespace()).collect::<String>().to_ascii_lowercase();
    let scheme = t.split_once(':').map(|(s, _)| s).filter(|s| !s.contains(['/', '?', '#']));
    // A scheme spelled with entities or escapes (`javascript&#58;`), which
    // Markdown readers turn back into one.
    let head = t.split(['/', '?', '#']).next().unwrap_or("");
    let drive = head.as_bytes().get(1) == Some(&b':') && head.as_bytes()[0].is_ascii_alphabetic();
    if head.contains(['&', '\\']) && !drive {
        return "#".to_string();
    }
    match scheme {
        None => l.to_string(),
        Some("http" | "https" | "mailto" | "ftp" | "tel") => l.to_string(),
        // A drive letter (C:\...) is a path, not a scheme.
        Some(s) if s.len() == 1 => l.to_string(),
        Some(_) => "#".to_string(),
    }
}

/// A font's name inside a quoted CSS string of a style attribute: only
/// letters, digits, spaces and `-_.`, so it can't end the string.
fn css_font(f: &str) -> String {
    f.chars().filter(|c| c.is_alphanumeric() || " -_.".contains(*c)).collect()
}

const DOC_CSS: &str = "body{font-family:'Noto Serif',Georgia,serif;font-size:12pt;line-height:1.35;max-width:48em;margin:2em auto;padding:0 1em;color:#1a1a1a}\
p{margin:0 0 8pt}h1,h2,h3,h4,h5,h6{font-family:'Noto Sans',Arial,sans-serif;color:#1F3864;margin:12pt 0 4pt}\
h1.title{font-size:2.2em;color:#1a1a1a}blockquote{margin:0 0 8pt 12pt;padding-left:10pt;border-left:3pt solid #c8c8c8}\
pre{background:#F3F4F6;padding:6pt;white-space:pre-wrap}code{font-family:'Noto Sans Mono',monospace}\
table{margin:0 0 8pt}td p,th p{margin:0}li{margin-bottom:2pt}img{max-width:100%}";

fn title_of(doc: &Document) -> String {
    let t = doc.meta.title.trim();
    if !t.is_empty() {
        return t.to_string();
    }
    doc.paragraphs
        .iter()
        .find(|p| matches!(p.style.kind, ParaKind::Title | ParaKind::Heading(_)) && !p.text.trim().is_empty())
        .or_else(|| doc.paragraphs.iter().find(|p| !p.text.trim().is_empty()))
        .map(|p| p.text.chars().filter(|&c| c != OBJECT && c != LINE_BREAK).take(80).collect::<String>().trim().to_string())
        .unwrap_or_default()
}

fn lang_of(doc: &Document) -> &str {
    let _ = doc;
    "en"
}

/// A standalone HTML page with the pictures embedded.
pub fn doc_html(doc: &Document) -> String {
    let shared = doc.shared();
    let doc = &shared;
    crate::theme_xml::set_writing_theme(&doc.styles.theme);
    let mut w = Writer { doc, out: String::new(), pics: Pictures::Inline, lists: Vec::new(), in_code: false, headings: Vec::new() };
    w.body();
    let mut out = String::from("<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\"/>\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"/>\n");
    let _ = writeln!(out, "<title>{}</title>", escaped(&title_of(doc)));
    if !doc.meta.author.is_empty() {
        let _ = writeln!(out, "<meta name=\"author\" content=\"{}\"/>", escaped(&doc.meta.author));
    }
    let _ = writeln!(out, "<meta name=\"generator\" content=\"ApexDesk\"/>\n<style>{DOC_CSS}</style>\n</head>\n<body>");
    out.push_str(&w.out);
    out.push_str("</body>\n</html>\n");
    out
}

/// The body of `doc` for the clipboard: what other programs paste. Their
/// own style sheet applies there, so the document's font and size go on
/// a wrapper and the rest is in inline styles.
pub fn fragment_html(doc: &Document) -> String {
    let shared = doc.shared();
    let doc = &shared;
    crate::theme_xml::set_writing_theme(&doc.styles.theme);
    let mut w = Writer { doc, out: String::new(), pics: Pictures::Inline, lists: Vec::new(), in_code: false, headings: Vec::new() };
    w.body();
    let mut css = String::new();
    if let Some(f) = doc.default_font_name() {
        let _ = write!(css, "font-family:'{}';", css_font(&crate::theme_xml::font_name(&f)));
    }
    let _ = write!(css, "font-size:{}pt", doc.base_size(ParaKind::Normal) as f32 / 2.0);
    format!("<meta charset=\"utf-8\"><div style=\"{css}\">{}</div>", w.out)
}

/// Days since 1970-01-01 to a civil date (proleptic Gregorian).
pub(crate) fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn now_utc() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let (y, m, d) = civil(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", s / 3600, s / 60 % 60, s % 60)
}

fn xhtml_page(title: &str, lang: &str, body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" xml:lang=\"{lang}\" lang=\"{lang}\">\n<head>\n<meta charset=\"utf-8\"/>\n<title>{}</title>\n<link rel=\"stylesheet\" type=\"text/css\" href=\"style.css\"/>\n</head>\n<body>\n{body}</body>\n</html>\n",
        escaped(title)
    )
}

/// An EPUB 3 book: one chapter per top-level heading, a navigation
/// document built from the headings and the pictures as files.
pub fn doc_epub(doc: &Document) -> Vec<u8> {
    epub(doc, None, lang_of(doc))
}

/// A book's EPUB: as [`doc_epub`] with a cover first, the picture `cover`
/// (its bytes and media type) or else a page with the title and author;
/// `lang` the book's language ("es").
pub fn book_epub(doc: &Document, cover: Option<(&[u8], &str)>, lang: &str) -> Vec<u8> {
    epub(doc, Some(cover), lang)
}

/// `cover`: None no cover, Some(None) a title page.
fn epub(doc: &Document, cover: Option<Option<(&[u8], &str)>>, lang: &str) -> Vec<u8> {
    let shared = doc.shared();
    let doc = &shared;
    crate::theme_xml::set_writing_theme(&doc.styles.theme);
    let title = {
        let t = title_of(doc);
        if t.is_empty() { "Untitled".to_string() } else { t }
    };
    // Chapters start at each Title or Heading 1 (the first one wherever).
    let tables: HashMap<usize, TableMap> = doc.tables().into_iter().map(|m| (m.start, m)).collect();
    let mut cuts = vec![0usize];
    let mut i = 0;
    while i < doc.paragraphs.len() {
        if let Some(m) = tables.get(&i) {
            i = m.end.max(i + 1);
            continue;
        }
        let p = &doc.paragraphs[i];
        if p.cell.is_none() && matches!(p.style.kind, ParaKind::Title | ParaKind::Heading(1)) && i > 0 && doc.paragraphs[..i].iter().any(|q| !q.text.trim().is_empty()) {
            cuts.push(i);
        }
        i += 1;
    }
    cuts.push(doc.paragraphs.len());
    let mut w = Writer { doc, out: String::new(), pics: Pictures::Files(Vec::new(), HashMap::new()), lists: Vec::new(), in_code: false, headings: Vec::new() };
    let mut chapters = Vec::new();
    let mut toc: Vec<(u8, String, String)> = Vec::new();
    for (n, win) in cuts.windows(2).enumerate() {
        let (a, b) = (win[0], win[1]);
        if a == b {
            continue;
        }
        w.out.clear();
        w.headings.clear();
        let mut i = a;
        while i < b {
            if let Some(m) = tables.get(&i) {
                w.table(m);
                i = m.end.max(i + 1);
                continue;
            }
            if doc.paragraphs[i].cell.is_none() {
                w.paragraph(i);
            }
            i += 1;
        }
        w.close_code();
        w.close_lists(0);
        let file = format!("chapter{}.xhtml", n + 1);
        for (lvl, id, text) in w.headings.drain(..) {
            toc.push((lvl, format!("{file}#{id}"), text));
        }
        let chapter_title = toc.last().map_or(title.clone(), |t| t.2.clone());
        chapters.push((file, xhtml_page(&chapter_title, lang, &w.out)));
    }
    if chapters.is_empty() {
        chapters.push(("chapter1.xhtml".to_string(), xhtml_page(&title, lang, "<p></p>\n")));
    }
    // Navigation: nested lists by heading level.
    let mut nav = String::from("<nav epub:type=\"toc\" id=\"toc\">\n<h1>");
    esc(&mut nav, &title);
    nav.push_str("</h1>\n<ol>\n");
    if toc.is_empty() {
        let _ = writeln!(nav, "<li><a href=\"{}\">{}</a></li>", chapters[0].0, escaped(&title));
    } else {
        let base = toc.iter().map(|t| t.0).min().unwrap_or(1);
        let mut depth = 0usize;
        for (k, (lvl, href, text)) in toc.iter().enumerate() {
            let d = (lvl - base) as usize;
            if k > 0 {
                if d > depth {
                    // Only one level deeper at a time keeps the list valid.
                    nav.push_str("\n<ol>\n");
                    depth += 1;
                } else {
                    nav.push_str("</li>\n");
                    while depth > d {
                        nav.push_str("</ol>\n</li>\n");
                        depth -= 1;
                    }
                }
            }
            let _ = write!(nav, "<li><a href=\"{}\">{}</a>", escaped(href), escaped(text));
        }
        nav.push_str("</li>\n");
        while depth > 0 {
            nav.push_str("</ol>\n</li>\n");
            depth -= 1;
        }
    }
    nav.push_str("</ol>\n</nav>\n");
    let nav_page = xhtml_page(&title, lang, &nav);
    let Pictures::Files(images, _) = w.pics else { unreachable!() };

    let id = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        title.hash(&mut h);
        doc.paragraphs.len().hash(&mut h);
        for p in doc.paragraphs.iter().take(200) {
            p.text.hash(&mut h);
        }
        let a = h.finish();
        a.hash(&mut h);
        let b = h.finish();
        format!("{:08x}-{:04x}-4{:03x}-a{:03x}-{:012x}", a >> 32, (a >> 16) & 0xFFFF, a & 0xFFF, (b >> 52) & 0xFFF, b & 0xFFFF_FFFF_FFFF)
    };
    let mut opf = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"bookid\" xml:lang=\"{lang}\">\n<metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n<dc:identifier id=\"bookid\">urn:uuid:{id}</dc:identifier>\n<dc:title>{}</dc:title>\n<dc:language>{lang}</dc:language>\n",
        escaped(&title)
    );
    if !doc.meta.author.is_empty() {
        let _ = writeln!(opf, "<dc:creator>{}</dc:creator>", escaped(&doc.meta.author));
    }
    let _ = writeln!(opf, "<meta property=\"dcterms:modified\">{}</meta>\n</metadata>\n<manifest>", now_utc());
    // The cover: its picture and a page showing it (or the title).
    let cover_ext = |mime: &str| match mime {
        "image/png" => "png",
        "image/gif" => "gif",
        "image/svg+xml" => "svg",
        "image/webp" => "webp",
        _ => "jpg",
    };
    let cover_page = cover.map(|c| match c {
        Some((_, mime)) => xhtml_page(&title, lang, &format!("<div class=\"cover\"><img src=\"cover.{}\" alt=\"{}\"/></div>\n", cover_ext(mime), escaped(&title))),
        None => {
            let mut body = format!("<div class=\"cover\"><h1 class=\"cover-title\">{}</h1>\n", escaped(&title));
            if !doc.meta.author.is_empty() {
                let _ = writeln!(body, "<p class=\"cover-author\">{}</p>", escaped(&doc.meta.author));
            }
            body.push_str("</div>\n");
            xhtml_page(&title, lang, &body)
        }
    });
    if let Some(Some((_, mime))) = cover {
        opf = opf.replace("</metadata>", "<meta name=\"cover\" content=\"cover-img\"/>\n</metadata>");
        let _ = writeln!(opf, "<item id=\"cover-img\" href=\"cover.{}\" media-type=\"{mime}\" properties=\"cover-image\"/>", cover_ext(mime));
    }
    if cover_page.is_some() {
        opf.push_str("<item id=\"cover\" href=\"cover.xhtml\" media-type=\"application/xhtml+xml\"/>\n");
    }
    opf.push_str("<item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/>\n<item id=\"css\" href=\"style.css\" media-type=\"text/css\"/>\n");
    for (k, (file, _)) in chapters.iter().enumerate() {
        let _ = writeln!(opf, "<item id=\"c{}\" href=\"{file}\" media-type=\"application/xhtml+xml\"/>", k + 1);
    }
    for (k, (file, mime, _)) in images.iter().enumerate() {
        let _ = writeln!(opf, "<item id=\"i{}\" href=\"{file}\" media-type=\"{mime}\"/>", k + 1);
    }
    opf.push_str("</manifest>\n<spine>\n");
    if cover_page.is_some() {
        opf.push_str("<itemref idref=\"cover\" linear=\"no\"/>\n");
    }
    for k in 0..chapters.len() {
        let _ = writeln!(opf, "<itemref idref=\"c{}\"/>", k + 1);
    }
    opf.push_str("</spine>\n</package>\n");

    let mut z = ZipWriter::new();
    // The mimetype comes first and uncompressed (EPUB OCF).
    z.add("mimetype", b"application/epub+zip", false);
    z.add(
        "META-INF/container.xml",
        b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\">\n<rootfiles>\n<rootfile full-path=\"OEBPS/content.opf\" media-type=\"application/oebps-package+xml\"/>\n</rootfiles>\n</container>\n",
        true,
    );
    z.add("OEBPS/content.opf", opf.as_bytes(), true);
    z.add("OEBPS/nav.xhtml", nav_page.as_bytes(), true);
    let mut css = DOC_CSS.replace("max-width:48em;margin:2em auto;padding:0 1em;", "");
    if let Some(page) = &cover_page {
        css.push_str(".cover{text-align:center;margin:0;padding:0}.cover img{max-width:100%;max-height:100vh}.cover-title{margin-top:30%}.cover-author{font-size:1.2em}");
        z.add("OEBPS/cover.xhtml", page.as_bytes(), true);
    }
    if let Some(Some((data, mime))) = cover {
        z.add(&format!("OEBPS/cover.{}", cover_ext(mime)), data, false);
    }
    z.add("OEBPS/style.css", css.as_bytes(), true);
    for (file, page) in &chapters {
        z.add(&format!("OEBPS/{file}"), page.as_bytes(), true);
    }
    for (file, _, data) in &images {
        z.add(&format!("OEBPS/{file}"), data, false);
    }
    z.finish()
}

// --- Workbooks --------------------------------------------------------------------------------

const SHEET_CSS: &str = "body{font-family:'Noto Sans',Arial,sans-serif;font-size:11pt;margin:1.5em}\
table{border-collapse:collapse;margin-bottom:2em}td,th{border:0.5pt solid #D0D0D0;padding:2pt 5pt;white-space:pre-wrap;vertical-align:bottom}\
th{background:#F3F3F3;font-weight:normal;color:#555}h2{font-size:1.2em;margin:1em 0 .4em}";

/// Every visible sheet as a table of the displayed values, with the
/// cells' fonts, colors, fills, alignment and merges.
pub fn sheet_html(wb: &Workbook, title: &str) -> String {
    let mut out = String::from("<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\"/>\n");
    let _ = writeln!(out, "<title>{}</title>\n<meta name=\"generator\" content=\"ApexDesk\"/>\n<style>{SHEET_CSS}</style>\n</head>\n<body>", escaped(title));
    let visible: Vec<usize> = (0..wb.sheets.len()).filter(|&i| !wb.sheets[i].hidden).collect();
    for &si in &visible {
        let sh = &wb.sheets[si];
        if visible.len() > 1 {
            let _ = writeln!(out, "<h2>{}</h2>", escaped(&sh.name));
        }
        let Some(ext) = sh.extent() else {
            out.push_str("<table></table>\n");
            continue;
        };
        // Hidden rows and columns (filtered out too) are left out: a page
        // can't keep them aside.
        let rows: std::collections::HashSet<u32> = sh.row_heights.shown(0, ext.row).into_iter().collect();
        let cols: std::collections::HashSet<u32> = sh.col_widths.shown(0, ext.col).into_iter().collect();
        let shown = |set: &std::collections::HashSet<u32>, a: u32, b: u32| (a..=b).filter(|i| set.contains(i)).count() as u32;
        // A huge rectangle (a lone cell far away) is written by the cells
        // it has, with the gaps as spans, and without its merges.
        let big = (u64::from(ext.row) + 1) * (u64::from(ext.col) + 1) > 4_000_000;
        let mut by_row: std::collections::BTreeMap<u32, Vec<u32>> = Default::default();
        if big {
            for (cr, _) in sh.iter() {
                by_row.entry(cr.row).or_default().push(cr.col);
            }
            for v in by_row.values_mut() {
                v.sort_unstable();
                v.dedup();
            }
        }
        // Merged areas: anchor -> spans, covered cells skipped.
        let mut spans: HashMap<(u32, u32), (u32, u32)> = HashMap::new();
        let mut covered: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
        // Overlapping merges would cover the table many times over: what
        // they cover in all is bounded (the rest are left out).
        let mut area_left = 4_000_000usize;
        for m in sh.merges.iter().filter(|_| !big) {
            // Only the part inside the table: a merge may cover the whole sheet.
            let (end_row, end_col) = (m.end.row.min(ext.row), m.end.col.min(ext.col));
            if m.start.row > end_row || m.start.col > end_col {
                continue;
            }
            // One whose first cell is hidden isn't merged (its other cells
            // are written, as the empty cells they are).
            if !rows.contains(&m.start.row) || !cols.contains(&m.start.col) {
                continue;
            }
            let area = (end_row - m.start.row + 1) as usize * (end_col - m.start.col + 1) as usize;
            let Some(left) = area_left.checked_sub(area) else { continue };
            area_left = left;
            spans.insert((m.start.row, m.start.col), (shown(&rows, m.start.row, end_row), shown(&cols, m.start.col, end_col)));
            for r in m.start.row..=end_row {
                for c in m.start.col..=end_col {
                    if (r, c) != (m.start.row, m.start.col) {
                        covered.insert((r, c));
                    }
                }
            }
        }
        out.push_str("<table>\n<colgroup>");
        for c in (0..=ext.col).filter(|c| cols.contains(c)) {
            let _ = write!(out, "<col style=\"width:{:.0}pt\"/>", sh.col_width(c) * 0.75);
        }
        out.push_str("</colgroup>\n");
        for r in (0..=ext.row).filter(|r| rows.contains(r)) {
            out.push_str("<tr>");
            let these: Box<dyn Iterator<Item = u32>> = if big { Box::new(by_row.get(&r).into_iter().flatten().copied()) } else { Box::new(0..=ext.col) };
            let mut next = 0;
            for c in these.filter(|c| cols.contains(c)) {
                if c > next && shown(&cols, next, c - 1) > 0 {
                    let _ = write!(out, "<td colspan=\"{}\"></td>", shown(&cols, next, c - 1));
                }
                next = c + 1;
                if covered.contains(&(r, c)) {
                    continue;
                }
                let at = CellRef::new(r, c);
                let (text, color) = wb.display(si, at);
                let mut css = String::new();
                if let Some(cell) = wb.cell(si, at) {
                    let numeric = matches!(cell.value(), apex_sheet::Value::Number(_));
                    let st = wb.styles.get(cell.style);
                    if st.bold {
                        css.push_str("font-weight:bold;");
                    }
                    if st.italic {
                        css.push_str("font-style:italic;");
                    }
                    match (st.underline, st.strike) {
                        (true, true) => css.push_str("text-decoration:underline line-through;"),
                        (true, false) => css.push_str("text-decoration:underline;"),
                        (false, true) => css.push_str("text-decoration:line-through;"),
                        _ => {}
                    }
                    if let Some(f) = &st.font {
                        let _ = write!(css, "font-family:'{}';", css_font(f));
                    }
                    if st.font_size > 0 {
                        let _ = write!(css, "font-size:{}pt;", st.font_size as f32 / 10.0);
                    }
                    if let Some(c) = color.or(st.color) {
                        let _ = write!(css, "color:{};", hex(c));
                    }
                    if let Some(f) = st.fill {
                        let _ = write!(css, "background:{};", hex(f));
                    }
                    match st.halign {
                        HAlign::Left => css.push_str("text-align:left;"),
                        HAlign::Center => css.push_str("text-align:center;"),
                        HAlign::Right => css.push_str("text-align:right;"),
                        _ if numeric => css.push_str("text-align:right;"),
                        _ => {}
                    }
                }
                out.push_str("<td");
                if let Some(&(rs, cs)) = spans.get(&(r, c)) {
                    if rs > 1 {
                        let _ = write!(out, " rowspan=\"{rs}\"");
                    }
                    if cs > 1 {
                        let _ = write!(out, " colspan=\"{cs}\"");
                    }
                }
                if !css.is_empty() {
                    let _ = write!(out, " style=\"{css}\"");
                }
                out.push('>');
                esc(&mut out, &text);
                out.push_str("</td>");
            }
            if big && next > 0 && next <= ext.col && shown(&cols, next, ext.col) > 0 {
                let _ = write!(out, "<td colspan=\"{}\"></td>", shown(&cols, next, ext.col));
            }
            out.push_str("</tr>\n");
        }
        out.push_str("</table>\n");
    }
    out.push_str("</body>\n</html>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_doc::{Pos, Selection};

    #[test]
    fn what_doesnt_show_stays_out() {
        let mut wb = Workbook::new();
        for (r, c, t) in [(0, 0, "a"), (0, 1, "salary"), (1, 0, "filtered"), (2, 0, "b")] {
            wb.set_input(0, CellRef::new(r, c), t);
        }
        wb.sheets[0].col_widths.insert(1, 0.0);
        wb.sheets[0].row_heights.insert(1, 0.0);
        let out = sheet_html(&wb, "t");
        assert!(out.contains(">a<") && out.contains(">b<") && !out.contains("salary") && !out.contains("filtered"));
        // Deleted (tracked) and hidden text.
        let mut d = Document::from_paragraphs(vec![apex_doc::Paragraph::new("keep secret")]);
        d.track_changes = true;
        d.delete(Selection { anchor: Pos::new(0, 4), cursor: Pos::new(0, 11) });
        let h = doc_html(&d);
        assert!(h.contains("keep") && !h.contains("secret"));
        assert!(!crate::markdown::write(&d.shared()).contains("secret"));
    }

    #[test]
    fn a_far_cell_spans_the_gap() {
        let mut wb = Workbook::new();
        wb.set_input(0, CellRef::new(0, 1), "a");
        wb.set_input(0, CellRef::new(apex_sheet::MAX_ROWS - 1, apex_sheet::MAX_COLS - 1), "z");
        let out = sheet_html(&wb, "t");
        assert!(out.len() < 64 << 20, "{}", out.len());
        assert!(out.contains(&format!("<tr><td colspan=\"1\"></td><td>a</td><td colspan=\"{}\"></td></tr>", apex_sheet::MAX_COLS - 2)));
        assert!(out.contains(&format!("<tr><td colspan=\"{}\"></td><td>z</td></tr>", apex_sheet::MAX_COLS - 1)));
    }

    #[test]
    fn links_and_fonts_cannot_inject() {
        assert_eq!(safe_href("javascript:alert(1)"), "#");
        assert_eq!(safe_href(" JaVa\tScript:alert(1)"), "#");
        assert_eq!(safe_href("data:text/html,<script>"), "#");
        assert_eq!(safe_href("vbscript:x"), "#");
        assert_eq!(safe_href("https://example.com/a?b=c:d"), "https://example.com/a?b=c:d");
        assert_eq!(safe_href("mailto:a@b.c"), "mailto:a@b.c");
        assert_eq!(safe_href("#_Toc1"), "#_Toc1");
        assert_eq!(safe_href("other.html#x"), "other.html#x");
        assert_eq!(safe_href("javascript&#58;alert(1)"), "#");
        assert_eq!(safe_href("C:\\docs\\a.html"), "C:\\docs\\a.html");
        assert_eq!(safe_href("C:\\docs\\a.pdf"), "C:\\docs\\a.pdf");
        assert_eq!(css_font("Arial\\';background:url(https://x/y)"), "Arialbackgroundurlhttpsxy");
        assert_eq!(css_font("Noto Sans CJK-JP"), "Noto Sans CJK-JP");
    }

    fn sample() -> Document {
        let mut d = crate::markdown::read("# Intro\n\nHello **bold** & <b>.\n\n- one\n- two\n  - nested\n\n1. first\n2. second\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n# Second\n\nText [link](https://example.com).\n");
        d.meta.title = "Test".into();
        let _ = d.insert_text(Selection::caret(Pos::new(0, 0)), "", None);
        d
    }

    #[test]
    fn html_page() {
        let h = doc_html(&sample());
        assert!(h.contains("<h1 id=\"h0\">Intro</h1>"));
        assert!(h.contains("<strong>bold</strong> &amp; &lt;b&gt;."));
        assert!(h.contains("<ul"));
        assert!(h.contains("<ol type=\"1\">"));
        assert!(h.contains("<table"));
        assert!(h.contains("<a href=\"https://example.com\">link</a>"));
        // Balanced list markup.
        assert_eq!(h.matches("<ul").count(), h.matches("</ul>").count());
        assert_eq!(h.matches("<ol").count(), h.matches("</ol>").count());
        assert_eq!(h.matches("<li").count(), h.matches("</li>").count());
    }

    fn well_formed(x: &str) -> bool {
        let mut r = crate::xml::Reader::new(x);
        let mut stack = Vec::new();
        loop {
            match r.next() {
                crate::xml::Ev::Start(t) => stack.push(t.name.to_string()),
                crate::xml::Ev::End(n) => {
                    if stack.pop().as_deref() != Some(n) {
                        return false;
                    }
                }
                crate::xml::Ev::Eof => return stack.is_empty(),
                _ => {}
            }
        }
    }

    #[test]
    fn epub_package() {
        let bytes = doc_epub(&sample());
        let z = crate::zip::ZipReader::new(&bytes).expect("zip");
        assert_eq!(z.read_string("mimetype").expect("mimetype"), "application/epub+zip");
        let opf = z.read_string("OEBPS/content.opf").expect("opf");
        assert!(opf.contains("chapter2.xhtml"));
        let nav = z.read_string("OEBPS/nav.xhtml").expect("nav");
        assert!(nav.contains("chapter2.xhtml#h"));
        for name in ["OEBPS/chapter1.xhtml", "OEBPS/chapter2.xhtml", "OEBPS/nav.xhtml"] {
            let x = z.read_string(name).expect("page");
            assert!(well_formed(&x), "{name} is not well-formed XML");
        }
        assert!(!opf.contains("cover"));
        // A book's: its cover first, a picture or the title.
        let mut doc = sample();
        doc.meta.author = "Ana & Luis".into();
        let bytes = book_epub(&doc, Some((b"\x89PNG", "image/png")), "es");
        let z = crate::zip::ZipReader::new(&bytes).expect("zip");
        let opf = z.read_string("OEBPS/content.opf").expect("opf");
        assert!(opf.contains("properties=\"cover-image\"") && opf.contains("<itemref idref=\"cover\"") && opf.contains("xml:lang=\"es\""));
        assert!(z.read("OEBPS/cover.png").is_ok());
        assert!(well_formed(&z.read_string("OEBPS/cover.xhtml").expect("cover")));
        let bytes = book_epub(&doc, None, "es");
        let z = crate::zip::ZipReader::new(&bytes).expect("zip");
        let page = z.read_string("OEBPS/cover.xhtml").expect("cover");
        assert!(well_formed(&page) && page.contains("Ana &amp; Luis"));
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_000), (2024, 10, 4));
    }

    #[test]
    fn sheet_page() {
        let mut wb = Workbook::new();
        wb.set_input(0, CellRef::new(0, 0), "Name");
        wb.set_input(0, CellRef::new(1, 1), "42");
        let h = sheet_html(&wb, "Book");
        assert!(h.contains("<td>Name</td>"));
        assert!(h.contains("text-align:right;\">42</td>"));
    }
}
