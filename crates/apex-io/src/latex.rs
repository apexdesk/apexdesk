//! LaTeX export of documents: headings, emphasis, lists, quotes, code,
//! tables (merged cells too), pictures (written beside the .tex file),
//! equations, notes, bookmarks and the references to them, and the table
//! of contents. What LaTeX has no plain counterpart for (shapes, fonts and
//! sizes, headers and footers) is left out. It compiles with pdflatex,
//! xelatex or lualatex.

use std::collections::HashMap;
#[cfg(test)]
use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;

use apex_doc::{Align, Attrs, Document, FieldKind, LINE_BREAK, NoteKind, OBJECT, ParaKind, Paragraph, SOFT_HYPHEN, TableMap};

use crate::Error;

/// A document as LaTeX: the source and the pictures it includes, by the
/// file names it gives them.
pub struct Tex {
    pub source: String,
    pub images: Vec<(String, Arc<[u8]>)>,
}

/// Writes `doc` as LaTeX to `path`, its pictures beside it
/// (`name-1.png`...).
pub fn save(doc: &Document, path: &Path) -> Result<(), Error> {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
    let tex = write(&doc.shared(), &stem);
    let dir = path.parent().unwrap_or(Path::new("."));
    for (name, data) in &tex.images {
        crate::write_atomic(&dir.join(name), data)?;
    }
    crate::write_atomic(path, tex.source.as_bytes())
}

/// The LaTeX of `doc` (as shown: pass `Document::shared`); pictures are
/// named after `stem`.
pub fn write(doc: &Document, stem: &str) -> Tex {
    let mut w = Writer { doc, stem: file_stem(stem), images: Vec::new(), endnotes: false, labels: labels(doc), moving: false, after: String::new() };
    let body = w.body();
    let mut out = String::with_capacity(body.len() + 1024);
    w.preamble(&mut out, &body);
    out.push_str("\\begin{document}\n\n");
    out.push_str(&body);
    if w.endnotes {
        out.push_str("\n\\theendnotes\n");
    }
    out.push_str("\n\\end{document}\n");
    Tex { source: out, images: w.images }
}

/// A name safe in file names and in `\includegraphics`.
fn file_stem(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    if t.is_empty() { "document".into() } else { t }
}

/// A bookmark's `\label` key.
fn label(name: &str) -> String {
    let t: String = name.chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '-' }).collect();
    format!("bm:{t}")
}

/// Where each bookmark starts: paragraph and byte offset, its labels.
fn labels(doc: &Document) -> HashMap<usize, Vec<(usize, String)>> {
    let mut out: HashMap<usize, Vec<(usize, String)>> = HashMap::new();
    for b in doc.bookmarks() {
        // Word's hidden ones (_Toc..., _Ref...) are targets too.
        out.entry(b.start.para).or_default().push((b.start.offset, label(&b.name)));
    }
    for v in out.values_mut() {
        v.sort();
    }
    out
}

/// A character pdflatex sets with T1 fonts and UTF-8 input.
fn pdflatex_char(c: char) -> bool {
    c.is_ascii() || ('\u{A0}'..='\u{17F}').contains(&c) || "–—‘’‚“”„†‡•…‰‹›€™".contains(c)
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF)
}

/// What pdflatex shows for a character it has no glyph for.
fn stand_in(c: char) -> String {
    match c {
        '\u{2002}' | '\u{2007}' | '\u{2008}' => "\\enspace{}".into(),
        '\u{2003}' => "\\quad{}".into(),
        '\u{2000}'..='\u{200A}' | '\u{202F}' | '\u{205F}' => "\\,".into(),
        '\u{200B}'..='\u{200F}' | '\u{2060}' | '\u{FEFF}' => String::new(),
        '●' | '◦' | '▪' | '■' | '◆' | '►' | '▶' | '‣' | '⁃' | '∙' => "\\textbullet{}".into(),
        '☐' | '□' | '❑' | '◻' => "\\ensuremath{\\square}".into(),
        '☑' | '☒' | '✓' | '✔' | '✗' | '✘' => "\\ensuremath{\\checkmark}".into(),
        '−' => "\\ensuremath{-}".into(),
        // Combining marks: left out.
        c if ('\u{300}'..='\u{36F}').contains(&c) || ('\u{20D0}'..='\u{20FF}').contains(&c) => String::new(),
        c => match apex_doc::math::latex_name(c) {
            Some(n) => format!("\\ensuremath{{\\{n}}}"),
            None => "?".into(),
        },
    }
}

/// Text as LaTeX: its special characters escaped.
pub fn escape(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\textbackslash{}"),
            '{' | '}' | '$' | '&' | '%' | '#' | '_' => {
                out.push('\\');
                out.push(c);
            }
            '~' => out.push_str("\\textasciitilde{}"),
            '^' => out.push_str("\\textasciicircum{}"),
            '<' => out.push_str("\\textless{}"),
            '>' => out.push_str("\\textgreater{}"),
            '|' => out.push_str("\\textbar{}"),
            '\u{A0}' => out.push('~'),
            '\t' => out.push(' '),
            LINE_BREAK => out.push_str("\\leavevmode\\newline "),
            SOFT_HYPHEN => out.push_str("\\-"),
            '\u{C}' => out.push_str("\\newpage "),
            OBJECT => {}
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
}

fn escaped(s: &str) -> String {
    let mut o = String::new();
    escape(s, &mut o);
    o
}

/// A value for `\hypersetup`: no commands at all.
fn plain(s: &str) -> String {
    s.chars().filter(|c| !matches!(c, '\\' | '{' | '}' | '%' | '#' | '&' | '$' | '^' | '_' | '~' | ',' | '=') && !c.is_control()).collect()
}

struct Writer<'a> {
    doc: &'a Document,
    stem: String,
    images: Vec<(String, Arc<[u8]>)>,
    endnotes: bool,
    labels: HashMap<usize, Vec<(usize, String)>>,
    /// Writing a heading (a moving argument): no labels or links in it,
    /// its labels go to `after`.
    moving: bool,
    after: String,
}

impl Writer<'_> {
    fn preamble(&self, out: &mut String, body: &str) {
        let pg = &self.doc.page;
        let mut odd: Vec<char> = body.chars().filter(|&c| !pdflatex_char(c)).collect();
        odd.sort_unstable();
        odd.dedup();
        // Only so many: past that, the text is in another script.
        odd.truncate(2000);
        let cjk = odd.iter().any(|&c| is_cjk(c));
        let other = odd.iter().any(|&c| c.is_alphabetic());
        if other {
            out.push_str("% Written by ApexDesk. Compile with xelatex or lualatex: pdflatex can't set all of its text.\n");
        } else {
            out.push_str("% Written by ApexDesk. Compiles with pdflatex, xelatex or lualatex.\n");
        }
        out.push_str("\\documentclass{article}\n");
        out.push_str("\\usepackage{iftex}\n\\ifPDFTeX\n  \\usepackage[T1]{fontenc}\n  \\usepackage[utf8]{inputenc}\n");
        // What pdflatex has no glyph for: a stand-in, so it still compiles.
        for &c in &odd {
            let _ = writeln!(out, "  \\DeclareUnicodeCharacter{{{:04X}}}{{{}}}", c as u32, stand_in(c));
        }
        out.push_str("\\else\n  \\usepackage{fontspec}\n");
        if other {
            // A font with more than Latin, where there is one.
            out.push_str("  \\IfFontExistsTF{Noto Serif}{\\setmainfont{Noto Serif}}{\\IfFontExistsTF{DejaVu Serif}{\\setmainfont{DejaVu Serif}}{}}\n");
        }
        if cjk {
            out.push_str("  \\ifXeTeX\n    \\IfFileExists{xeCJK.sty}{\\usepackage{xeCJK}\n    \\IfFontExistsTF{Noto Serif CJK SC}{\\setCJKmainfont{Noto Serif CJK SC}}{}}{}\n  \\else\n    \\IfFileExists{luatexja-fontspec.sty}{\\usepackage{luatexja-fontspec}}{}\n  \\fi\n");
        }
        out.push_str("\\fi\n");
        if pg.width > 0.0 && pg.height > 0.0 {
            let _ = writeln!(
                out,
                "\\usepackage[paperwidth={:.1}bp,paperheight={:.1}bp,top={:.1}bp,bottom={:.1}bp,left={:.1}bp,right={:.1}bp]{{geometry}}",
                pg.width,
                pg.height,
                pg.margin_top.max(0.0),
                pg.margin_bottom.max(0.0),
                pg.margin_left.max(0.0),
                pg.margin_right.max(0.0)
            );
        }
        out.push_str("\\usepackage{amsmath,amssymb}\n\\usepackage{graphicx}\n\\usepackage[normalem]{ulem}\n\\usepackage{xcolor}\n\\usepackage{multirow}\n");
        if self.endnotes {
            out.push_str("\\usepackage{endnotes}\n");
        }
        out.push_str("\\usepackage{hyperref}\n");
        let m = &self.doc.meta;
        if !m.title.trim().is_empty() || !m.author.trim().is_empty() {
            let _ = writeln!(out, "\\hypersetup{{pdftitle={{{}}},pdfauthor={{{}}}}}", plain(&m.title), plain(&m.author));
        }
        // Headings as in Word: unnumbered, but in the table of contents.
        out.push_str("\\setcounter{secnumdepth}{0}\n\\setcounter{tocdepth}{3}\n\\setlength{\\parindent}{0pt}\n\\setlength{\\parskip}{0.5\\baselineskip}\n\n");
    }

    fn text_width(&self) -> f32 {
        let pg = &self.doc.page;
        (pg.width - pg.margin_left - pg.margin_right).max(72.0)
    }

    fn body(&mut self) -> String {
        let doc = self.doc;
        let mut out = String::new();
        let mut tables: HashMap<usize, TableMap> = doc.tables().into_iter().filter(|m| m.depth == 0).map(|m| (m.start, m)).collect();
        let tocs: HashMap<usize, usize> = doc.toc_blocks().into_iter().collect();
        let numbers = doc.list_numbers();
        // Open lists: their kind, innermost last.
        let mut lists: Vec<ParaKind> = Vec::new();
        let mut i = 0;
        while i < doc.paragraphs.len() {
            let p = &doc.paragraphs[i];
            let kind = p.style.kind;
            if !kind.is_list() || p.cell.is_some() || tocs.contains_key(&i) {
                close_lists(&mut lists, 0, &mut out);
            }
            if p.style.page_break_before {
                out.push_str("\\clearpage\n");
            }
            if let Some(&end) = tocs.get(&i) {
                out.push_str("\\tableofcontents\n\n");
                i = end;
                continue;
            }
            if p.cell.is_some() {
                match tables.remove(&i) {
                    Some(m) => {
                        self.table(&m, &mut out);
                        i = m.end.max(i + 1);
                    }
                    None => i += 1,
                }
                continue;
            }
            match kind {
                ParaKind::Code => {
                    let end = (i..doc.paragraphs.len()).find(|&j| doc.paragraphs[j].style.kind != ParaKind::Code || doc.paragraphs[j].cell.is_some()).unwrap_or(doc.paragraphs.len());
                    out.push_str("\\begin{verbatim}\n");
                    for q in &doc.paragraphs[i..end] {
                        let line: String = q.text.chars().filter(|&c| c != OBJECT).map(|c| if c == LINE_BREAK { '\n' } else { c }).collect();
                        out.push_str(&line.replace("\\end{verbatim}", "\\end {verbatim}"));
                        out.push('\n');
                    }
                    out.push_str("\\end{verbatim}\n\n");
                    i = end;
                    continue;
                }
                ParaKind::Quote => {
                    let end = (i..doc.paragraphs.len()).find(|&j| doc.paragraphs[j].style.kind != ParaKind::Quote || doc.paragraphs[j].cell.is_some()).unwrap_or(doc.paragraphs.len());
                    out.push_str("\\begin{quote}\n");
                    for (k, q) in doc.paragraphs[i..end].iter().enumerate() {
                        if k > 0 {
                            out.push_str("\n\n");
                        }
                        self.para_text(i + k, q, &mut out);
                    }
                    out.push_str("\n\\end{quote}\n\n");
                    i = end;
                    continue;
                }
                ParaKind::Bullet | ParaKind::Numbered => {
                    let level = (p.style.level as usize).min(3);
                    let restart = kind == ParaKind::Numbered && p.style.restart;
                    // Deeper lists close; one of another kind (or restarted) too.
                    close_lists(&mut lists, level + 1, &mut out);
                    if lists.len() == level + 1 && (lists[level] != kind || restart) {
                        close_lists(&mut lists, level, &mut out);
                    }
                    let mut fresh = false;
                    while lists.len() < level + 1 {
                        // Levels skipped open as the same kind, in an
                        // empty item (a list can't start with a list).
                        if fresh {
                            let _ = writeln!(out, "{}\\item[]", "  ".repeat(lists.len()));
                        }
                        fresh = true;
                        let env = if kind == ParaKind::Numbered { "enumerate" } else { "itemize" };
                        let _ = writeln!(out, "{}\\begin{{{env}}}", "  ".repeat(lists.len()));
                        if kind == ParaKind::Numbered && lists.len() == level {
                            let depth = lists.iter().filter(|k| **k == ParaKind::Numbered).count().min(3);
                            let first = numbers.get(i).copied().unwrap_or(1);
                            if first > 1 {
                                let _ = writeln!(out, "\\setcounter{{enum{}}}{{{}}}", ["i", "ii", "iii", "iv"][depth], first - 1);
                            }
                        }
                        lists.push(kind);
                    }
                    let _ = write!(out, "{}\\item ", "  ".repeat(level + 1));
                    self.para_text(i, p, &mut out);
                    out.push('\n');
                    i += 1;
                    continue;
                }
                _ => {}
            }
            self.block(i, p, &mut out);
            i += 1;
        }
        close_lists(&mut lists, 0, &mut out);
        out
    }

    /// A heading, title or plain paragraph.
    fn block(&mut self, idx: usize, p: &Paragraph, out: &mut String) {
        let mut text = String::new();
        self.moving = matches!(p.style.kind, ParaKind::Heading(_));
        self.para_text(idx, p, &mut text);
        self.moving = false;
        let after = std::mem::take(&mut self.after);
        let display = p.spans().any(|(_, _, a)| a.math.is_some()) && apex_doc::math::is_display(p);
        if text.trim().is_empty() && !display {
            out.push_str("\\vspace{\\baselineskip}\n\n");
            return;
        }
        match p.style.kind {
            ParaKind::Title => {
                let _ = write!(out, "{{\\centering\\LARGE\\bfseries {text}\\par}}\n\n");
            }
            ParaKind::Heading(n) => {
                let cmd = match n {
                    0 | 1 => "section",
                    2 => "subsection",
                    3 => "subsubsection",
                    4 => "paragraph",
                    _ => "subparagraph",
                };
                // A heading's text is moving: `\newline` would break it.
                let _ = write!(out, "\\{cmd}{{{}}}{after}\n\n", text.replace("\\leavevmode\\newline ", " ").trim());
            }
            _ if display => {
                let _ = write!(out, "{}\n\n", text.trim());
            }
            _ => {
                let env = match p.style.align {
                    Align::Center => Some("center"),
                    Align::Right => Some("flushright"),
                    _ => None,
                };
                match env {
                    Some(e) => {
                        let _ = write!(out, "\\begin{{{e}}}\n{text}\n\\end{{{e}}}\n\n");
                    }
                    None => {
                        out.push_str(&text);
                        out.push_str("\n\n");
                    }
                }
            }
        }
    }

    /// The text of a paragraph, its formatting as commands.
    fn para_text(&mut self, idx: usize, p: &Paragraph, out: &mut String) {
        let mut labels = self.labels.remove(&idx).unwrap_or_default().into_iter().peekable();
        let display = apex_doc::math::is_display(p);
        for (s, e, a) in p.spans() {
            while let Some((_, l)) = labels.next_if(|(o, _)| *o < e.max(s + 1)) {
                let to = if self.moving { &mut self.after } else { &mut *out };
                let _ = write!(to, "\\phantomsection\\label{{{l}}}");
            }
            let t = &p.text[s..e];
            if t.is_empty() {
                continue;
            }
            self.span(t, &a, display, out);
        }
        for (_, l) in labels {
            let to = if self.moving { &mut self.after } else { &mut *out };
            let _ = write!(to, "\\phantomsection\\label{{{l}}}");
        }
    }

    fn span(&mut self, t: &str, a: &Attrs, display: bool, out: &mut String) {
        let objects = t.matches(OBJECT).count();
        if let Some(n) = &a.note {
            for _ in 0..objects {
                let mut body = String::new();
                for (k, q) in n.paras.iter().enumerate() {
                    if k > 0 {
                        body.push_str("\\par ");
                    }
                    for (s, e, qa) in q.spans() {
                        if qa.note.is_none() {
                            self.span(&q.text[s..e], &qa, false, &mut body);
                        }
                    }
                }
                let cmd = if n.kind == NoteKind::End {
                    self.endnotes = true;
                    "endnote"
                } else {
                    "footnote"
                };
                let protect = if self.moving { "\\protect" } else { "" };
                let _ = write!(out, "{protect}\\{cmd}{{{}}}", body.trim());
            }
            return;
        }
        if let Some(f) = &a.field {
            for _ in 0..objects.max(1) {
                match &f.kind {
                    FieldKind::Page => out.push_str("\\thepage{}"),
                    FieldKind::PageRef(b) if !self.moving => {
                        let _ = write!(out, "\\pageref{{{}}}", label(b));
                    }
                    FieldKind::Ref(b) if !self.moving => {
                        let _ = write!(out, "\\hyperref[{}]{{{}}}", label(b), escaped(&f.cached));
                    }
                    FieldKind::IndexEntry(_) | FieldKind::Authority { .. } => {}
                    _ => escape(&f.cached, out),
                }
            }
            return;
        }
        if a.shape.is_some() {
            return;
        }
        if let Some(img) = &a.image {
            for _ in 0..objects {
                let Some(data) = crate::picture::shown_png_or_jpeg(img) else { continue };
                let ext = if crate::docx_img::image_type(&data).0 == "png" { "png" } else { "jpg" };
                let name = format!("{}-{}.{ext}", self.stem, self.images.len() + 1);
                let size = if img.width > self.text_width() { "width=\\linewidth".to_string() } else { format!("width={:.1}bp", img.width.max(1.0)) };
                let _ = write!(out, "\\includegraphics[{size}]{{{name}}}");
                self.images.push((name, data));
            }
            return;
        }
        if let Some(eq) = &a.math {
            for _ in 0..objects {
                let tex = eq.latex();
                if display {
                    let _ = write!(out, "\\[ {tex} \\]");
                } else {
                    let _ = write!(out, "${tex}$");
                }
            }
            return;
        }
        let lead = &t[..t.len() - t.trim_start().len()];
        let core = t.trim();
        let trail = &t[lead.len() + core.len()..];
        escape(lead, out);
        if core.is_empty() {
            escape(trail, out);
            return;
        }
        let mut close = 0;
        let mut open = |cmd: &str, out: &mut String| {
            out.push_str(cmd);
            out.push('{');
            close += 1;
        };
        if let Some(l) = a.link.as_ref().filter(|_| !self.moving) {
            let l = crate::html::safe_href(l);
            match l.strip_prefix('#') {
                Some(b) => open(&format!("\\hyperref[{}]", label(b)), out),
                None => open(&format!("\\href{{{}}}", l.replace('\\', "/").replace('%', "\\%").replace('#', "\\#").replace(['{', '}'], "")), out),
            }
        }
        if let Some(c) = a.color.filter(|c| *c & 0xFF_FFFF != 0) {
            open(&format!("\\textcolor[HTML]{{{:06X}}}", c & 0xFF_FFFF), out);
        }
        for (on, cmd) in [
            (a.bold, "\\textbf"),
            (a.italic, "\\textit"),
            (a.underline, "\\uline"),
            (a.strike, "\\sout"),
            (a.code, "\\texttt"),
            (a.superscript, "\\textsuperscript"),
            (a.subscript, "\\textsubscript"),
        ] {
            if on {
                open(cmd, out);
            }
        }
        escape(core, out);
        for _ in 0..close {
            out.push('}');
        }
        escape(trail, out);
    }

    fn table(&mut self, m: &TableMap, out: &mut String) {
        let doc = self.doc;
        let (rows, cols) = (m.rows, m.cols.max(1));
        let width = |n: usize| format!("p{{\\dimexpr {:.4}\\linewidth-2\\tabcolsep\\relax}}", 0.98 * n as f32 / cols as f32);
        let _ = writeln!(out, "\\noindent\\begin{{tabular}}{{|{}}}\n\\hline", (0..cols).map(|_| width(1) + "|").collect::<String>());
        let at = |r: usize, c: usize| r * cols + c;
        for r in 0..rows {
            let mut cells = Vec::new();
            let mut c = 0;
            while c < cols {
                let (ar, ac) = m.anchor.get(at(r, c)).copied().unwrap_or((r, c));
                let (rs, cs) = m.span.get(at(ar, ac)).copied().unwrap_or((1, 1));
                let cs = cs.clamp(1, cols - c);
                let mut text = String::new();
                if ar == r && ac == c {
                    let (s, e) = m.cells.get(at(r, c)).copied().unwrap_or((0, 0));
                    for (k, i) in (s..e.min(doc.paragraphs.len())).enumerate() {
                        if k > 0 {
                            text.push_str("\\par ");
                        }
                        self.para_text(i, &doc.paragraphs[i], &mut text);
                    }
                    if rs > 1 {
                        text = format!("\\multirow{{{rs}}}{{=}}{{{text}}}");
                    }
                }
                if cs > 1 {
                    let left = if c == 0 { "|" } else { "" };
                    cells.push(format!("\\multicolumn{{{cs}}}{{{left}{}|}}{{{text}}}", width(cs).replace("-2\\tabcolsep", &format!("-2\\tabcolsep+{}\\tabcolsep", 2 * (cs - 1)))));
                } else {
                    cells.push(text);
                }
                c += cs;
            }
            out.push_str(&cells.join(" & "));
            out.push_str(" \\\\\n");
            // Lines under the row, but through cells merged downwards.
            let open: Vec<bool> = (0..cols).map(|c| r + 1 < rows && m.anchor.get(at(r + 1, c)) == m.anchor.get(at(r, c))).collect();
            if open.iter().all(|o| !o) {
                out.push_str("\\hline\n");
            } else {
                let mut c = 0;
                while c < cols {
                    if open[c] {
                        c += 1;
                        continue;
                    }
                    let start = c;
                    while c < cols && !open[c] {
                        c += 1;
                    }
                    let _ = write!(out, "\\cline{{{}-{}}}", start + 1, c);
                }
                out.push('\n');
            }
        }
        out.push_str("\\end{tabular}\n\n");
    }
}

/// Closes the lists deeper than `keep` levels.
fn close_lists(lists: &mut Vec<ParaKind>, keep: usize, out: &mut String) {
    while lists.len() > keep {
        let k = lists.pop().unwrap_or_default();
        let env = if k == ParaKind::Numbered { "enumerate" } else { "itemize" };
        let _ = writeln!(out, "{}\\end{{{env}}}", "  ".repeat(lists.len()));
    }
}

/// The labels used, for checks.
#[cfg(test)]
fn used_labels(tex: &str) -> HashSet<String> {
    tex.match_indices("\\label{").map(|(i, _)| tex[i + 7..].split('}').next().unwrap_or_default().to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_doc::{Image, Pos, Selection};

    fn md(s: &str) -> Document {
        crate::markdown::read(s)
    }

    #[test]
    fn headings_emphasis_lists_and_code() {
        // A list that starts one level in.
        let mut d = Document::from_plain_text("deep");
        d.paragraphs[0].style.kind = ParaKind::Bullet;
        d.paragraphs[0].style.level = 1;
        assert!(write(&d, "d").source.contains("\\begin{itemize}\n  \\item[]\n  \\begin{itemize}\n    \\item deep\n"));
        let d = md("# Title & co\n\nSome **bold**, *italic* and `x_1` 50% $5.\n\n- one\n- two\n  - deep\n\n1. a\n2. b\n\n> quoted\n\n```\nfn main() {}\n```\n");
        let t = write(&d, "doc").source;
        assert!(t.contains("\\section{Title \\& co}"), "{t}");
        assert!(t.contains("\\textbf{bold}, \\textit{italic} and \\texttt{x\\_1} 50\\% \\$5."), "{t}");
        assert!(t.contains("\\begin{itemize}\n  \\item one\n  \\item two\n  \\begin{itemize}\n    \\item deep\n  \\end{itemize}\n\\end{itemize}\n"), "{t}");
        assert!(t.contains("\\begin{enumerate}\n  \\item a\n  \\item b\n\\end{enumerate}"), "{t}");
        assert!(t.contains("\\begin{quote}\nquoted\n\\end{quote}"));
        assert!(t.contains("\\begin{verbatim}\nfn main() {}\n\\end{verbatim}"));
        assert!(t.starts_with("% Written by ApexDesk") && t.trim_end().ends_with("\\end{document}"));
        // Every brace closes.
        let depth = t.replace("\\{", "").replace("\\}", "").chars().fold(0i32, |d, c| d + (c == '{') as i32 - (c == '}') as i32);
        assert_eq!(depth, 0);
    }

    #[test]
    fn equations_notes_links_and_pictures() {
        let mut d = md("Area $\\pi r^2$ here[^1], see [web](https://x.y/a_b) and [there](#Mark).\n\n$$\\frac{a}{b}$$\n\n[^1]: A **note**.\n");
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend([0, 0, 0, 40, 0, 0, 0, 20, 8, 6, 0, 0, 0, 1]);
        d.insert_image(Selection::caret(Pos::new(0, 0)), Image::new(Arc::from(png), 30.0, 15.0));
        d.add_bookmark("Mark", Selection::caret(Pos::new(1, 0)));
        let out = write(&d, "my doc");
        let t = &out.source;
        assert!(t.contains("$\\pi r^{2}$"), "{t}");
        assert!(t.contains("\\footnote{\\textbf{note}.}") || t.contains("\\footnote{A \\textbf{note}.}"), "{t}");
        assert!(t.contains("\\href{https://x.y/a_b}{web}"), "{t}");
        assert!(t.contains("\\hyperref[bm:Mark]{there}"), "{t}");
        assert!(t.contains("\\[ \\frac{a}{b} \\]"), "{t}");
        assert!(t.contains("\\includegraphics[width=30.0bp]{my-doc-1.png}"), "{t}");
        assert_eq!(out.images.len(), 1);
        assert_eq!(used_labels(t), HashSet::from(["bm:Mark".to_string()]));
    }

    #[test]
    fn tables_with_merged_cells() {
        let mut d = Document::new();
        d.insert_table(Selection::default(), 3, 3, 400.0);
        let m = d.tables()[0].clone();
        for (k, (s, _)) in m.cells.iter().enumerate() {
            d.insert_text(Selection::caret(Pos::new(*s, 0)), &format!("c{k}"), None);
        }
        // The first row's first two cells merged.
        let m = d.tables()[0].clone();
        assert!(d.merge_cells(Selection { anchor: Pos::new(m.cells[0].0, 0), cursor: Pos::new(m.cells[1].0, 0) }).is_some());
        let t = write(&d, "t").source;
        assert!(t.contains("\\begin{tabular}{|p{"), "{t}");
        assert!(t.contains("\\multicolumn{2}{|p{"), "{t}");
        assert!(t.contains("c4 & c5"), "{t}");
        assert_eq!(t.matches(" \\\\\n").count(), 3, "{t}");
    }
}
