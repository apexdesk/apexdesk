//! Markdown (CommonMark subset) import/export.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use apex_doc::{Align, Attrs, Document, Image, LINE_BREAK, NOTE_SIZE, Note, NoteKind, OBJECT, ParaKind, ParaStyle, Paragraph, Table, TableMap};

/// Width of tables read from Markdown, in points.
const TABLE_WIDTH: f32 = 450.0;

pub fn read(text: &str) -> Document {
    // Footnote definitions (`[^label]: text`) come out of the text first.
    let (text, defs) = footnote_defs(text);
    let text = text.as_str();
    let mut paras: Vec<Paragraph> = Vec::new();
    let mut buf: Option<(ParaStyle, String)> = None;
    let mut in_code = false;
    let flush = |buf: &mut Option<(ParaStyle, String)>, paras: &mut Vec<Paragraph>| {
        if let Some((style, text)) = buf.take() {
            let mut p = parse_inline(&text);
            p.style = style;
            paras.push(p);
        }
    };
    // No more lines than paragraphs a document keeps may make.
    let lines: Vec<&str> = text.lines().take(4 * apex_doc::MAX_PARAGRAPHS).collect();
    let mut li = 0;
    let mut tables = 0u32;
    while li < lines.len() && paras.len() <= apex_doc::MAX_PARAGRAPHS {
        let line = lines[li].trim_end_matches('\r');
        li += 1;
        if line.trim_start().starts_with("```") {
            flush(&mut buf, &mut paras);
            in_code = !in_code;
            continue;
        }
        if in_code {
            let mut p = Paragraph::new(line);
            p.style.kind = ParaKind::Code;
            paras.push(p);
            continue;
        }
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if trimmed.is_empty() {
            flush(&mut buf, &mut paras);
            continue;
        }
        if let Some(aligns) = lines.get(li).and_then(|l| delimiter_row(l)).filter(|_| trimmed.contains('|')) {
            flush(&mut buf, &mut paras);
            let mut rows = vec![split_row(trimmed)];
            li += 1;
            while let Some(l) = lines.get(li).map(|l| l.trim()).filter(|l| !l.is_empty() && l.contains('|')) {
                rows.push(split_row(l));
                li += 1;
            }
            // Each cell is a paragraph at least: no more than a document
            // keeps (a short file of '|' lines would fill every column).
            let cols = aligns.len().clamp(1, apex_doc::MAX_COLS);
            rows.truncate(apex_doc::MAX_PARAGRAPHS.saturating_sub(paras.len()) / cols);
            // Past the row limit, the rows go on in another table.
            for chunk in rows.chunks(apex_doc::MAX_ROWS) {
                tables += 1;
                paras.extend(read_table(chunk, &aligns).into_paras(tables));
            }
            continue;
        }
        let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            flush(&mut buf, &mut paras);
            let kind = ParaKind::Heading(hashes as u8);
            buf = Some((ParaStyle { kind, ..Default::default() }, trimmed[hashes + 1..].trim().trim_end_matches('#').trim_end().to_string()));
            flush(&mut buf, &mut paras);
            continue;
        }
        if matches!(trimmed, "---" | "***" | "___") {
            flush(&mut buf, &mut paras);
            continue;
        }
        let level = (indent / 2).min(8) as u8;
        if let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")).or_else(|| trimmed.strip_prefix("+ ")) {
            flush(&mut buf, &mut paras);
            buf = Some((ParaStyle { kind: ParaKind::Bullet, level, ..Default::default() }, rest.to_string()));
            continue;
        }
        let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && digits < 10 && (trimmed[digits..].starts_with(". ") || trimmed[digits..].starts_with(") ")) {
            flush(&mut buf, &mut paras);
            let restart = trimmed[..digits].parse::<u32>() == Ok(1) && crate::list_continues(&paras, level);
            buf = Some((ParaStyle { kind: ParaKind::Numbered, level, restart, ..Default::default() }, trimmed[digits + 2..].to_string()));
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('>') {
            let rest = rest.strip_prefix(' ').unwrap_or(rest);
            match &mut buf {
                Some((s, t)) if s.kind == ParaKind::Quote => {
                    if t.ends_with("  ") || t.ends_with('\\') {
                        let keep = t.trim_end_matches('\\').trim_end().len();
                        t.truncate(keep);
                        t.push(LINE_BREAK);
                    } else {
                        t.push(' ');
                    }
                    t.push_str(rest);
                }
                _ => {
                    flush(&mut buf, &mut paras);
                    buf = Some((ParaStyle { kind: ParaKind::Quote, ..Default::default() }, rest.to_string()));
                }
            }
            continue;
        }
        match &mut buf {
            Some((_, t)) => {
                // Hard line break: two trailing spaces or backslash.
                if t.ends_with("  ") || t.ends_with('\\') {
                    let keep = t.trim_end_matches('\\').trim_end().len();
                    t.truncate(keep);
                    t.push(LINE_BREAK);
                    t.push_str(trimmed);
                } else {
                    t.push(' ');
                    t.push_str(trimmed);
                }
            }
            None => buf = Some((ParaStyle::default(), trimmed.to_string())),
        }
    }
    flush(&mut buf, &mut paras);
    if !defs.is_empty() {
        for p in &mut paras {
            attach_notes(p, &defs);
        }
    }
    Document::from_paragraphs(paras)
}

/// Removes footnote definitions from `text` (a definition goes on over
/// indented lines) and returns them as notes by label.
fn footnote_defs(text: &str) -> (String, HashMap<String, Arc<Note>>) {
    let mut out = String::with_capacity(text.len());
    let mut defs: HashMap<String, Arc<Note>> = HashMap::new();
    let mut cur: Option<(String, Vec<String>)> = None;
    let mut in_code = false;
    let finish = |cur: &mut Option<(String, Vec<String>)>, defs: &mut HashMap<String, Arc<Note>>| {
        if let Some((label, lines)) = cur.take() {
            let paras: Vec<Paragraph> = lines.split(|l| l.is_empty()).filter(|b| !b.is_empty()).map(|b| {
                let mut p = parse_inline(&b.join(" "));
                for r in &mut p.runs {
                    r.attrs.size.get_or_insert(NOTE_SIZE);
                }
                p
            }).collect();
            let mut n = Note::new(NoteKind::Foot);
            if !paras.is_empty() {
                n.paras = paras;
            }
            defs.insert(label, Arc::new(n));
        }
    };
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
        }
        if !in_code && line.starts_with("[^") && let Some(end) = line.find("]:") {
            finish(&mut cur, &mut defs);
            let label = line[2..end].to_string();
            if !label.is_empty() && !label.contains(char::is_whitespace) {
                cur = Some((label, vec![line[end + 2..].trim().to_string()]));
                continue;
            }
        }
        if let Some((_, lines)) = &mut cur {
            if line.starts_with("    ") || line.starts_with('\t') {
                lines.push(line.trim().to_string());
                continue;
            }
            if line.trim().is_empty() {
                lines.push(String::new());
                continue;
            }
            if lines.last().is_some_and(|l| l.is_empty()) {
                out.push('\n');
            }
            finish(&mut cur, &mut defs);
        }
        out.push_str(line);
        out.push('\n');
    }
    finish(&mut cur, &mut defs);
    (out, defs)
}

/// Turns `[^label]` references with a definition into note marks.
fn attach_notes(p: &mut Paragraph, defs: &HashMap<String, Arc<Note>>) {
    if !p.text.contains("[^") {
        return;
    }
    let mut out = Paragraph { text: String::new(), runs: Vec::new(), ..p.clone() };
    for (s, e, a) in p.spans() {
        let t = &p.text[s..e];
        let mut rest = t;
        while let Some(i) = rest.find("[^") {
            let Some(j) = rest[i..].find(']').map(|j| i + j) else { break };
            match defs.get(&rest[i + 2..j]) {
                Some(n) if !a.code => {
                    out.push(&rest[..i], a.clone());
                    out.push(&OBJECT.to_string(), Attrs { note: Some(n.clone()), image: None, field: None, link: None, link_tip: None, ..a.clone() });
                }
                _ => out.push(&rest[..j + 1], a.clone()),
            }
            rest = &rest[j + 1..];
        }
        out.push(rest, a.clone());
    }
    if out.runs.is_empty() {
        out.runs.push(apex_doc::Run { len: 0, attrs: Attrs::default() });
    }
    *p = out;
}

/// Column alignments of a table delimiter row (`| --- | :-: |`).
fn delimiter_row(line: &str) -> Option<Vec<Align>> {
    let t = line.trim();
    if !t.contains('-') || !t.contains('|') {
        return None;
    }
    let cells = split_row(t);
    let mut out = Vec::new();
    for c in &cells {
        let c = c.trim();
        let core = c.trim_matches(':');
        if core.is_empty() || !core.bytes().all(|b| b == b'-') {
            return None;
        }
        out.push(match (c.starts_with(':'), c.ends_with(':')) {
            (true, true) => Align::Center,
            (false, true) => Align::Right,
            _ => Align::Left,
        });
    }
    (!out.is_empty()).then_some(out)
}

/// The cells of a table row, split at unescaped pipes outside code spans.
fn split_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut code = false;
    let mut chars = t.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' if chars.peek() == Some(&'|') => {
                cur.push('|');
                chars.next();
            }
            '`' => {
                code = !code;
                cur.push(ch);
            }
            '|' if !code => cells.push(std::mem::take(&mut cur)),
            _ => cur.push(ch),
        }
    }
    if !cur.trim().is_empty() {
        cells.push(cur);
    }
    cells.into_iter().map(|c| c.trim().to_string()).collect()
}

fn read_table(rows: &[Vec<String>], aligns: &[Align]) -> Table {
    let cols = aligns.len().clamp(1, apex_doc::MAX_COLS);
    let mut t = Table::new(rows.len(), cols, TABLE_WIDTH);
    t.props.header_rows = 1;
    // Paragraphs the cells may still take (rows were cut to a document's
    // most with one each).
    let mut left = apex_doc::MAX_PARAGRAPHS;
    for (r, row) in rows.iter().enumerate() {
        for c in 0..cols {
            let text = row.get(c).map_or("", |s| s.as_str());
            // `<br>` separates the paragraphs of a cell.
            let n = text.split("<br>").count().clamp(1, 64).min(left.max(1));
            left = left.saturating_sub(n);
            t.rows[r][c].paras = text
                .split("<br>")
                .take(n)
                .map(|part| {
                    let mut p = parse_inline(part.trim());
                    p.style.align = aligns[c];
                    p
                })
                .collect();
        }
    }
    t
}

fn write_table(out: &mut String, doc: &Document, m: &TableMap) {
    let paras = &doc.paragraphs;
    let mut aligns = vec![Align::Left; m.cols];
    for r in 0..m.rows {
        for c in 0..m.cols {
            let mut cell = String::new();
            if m.is_anchor(r, c) {
                let (a, b) = m.cell(r, c);
                if r == 0 {
                    aligns[c] = paras[a].style.align;
                }
                for i in a..b {
                    if i > a {
                        cell.push_str("<br>");
                    }
                    let mut s = String::new();
                    write_inline(&mut s, &paras[i], "");
                    cell.push_str(&s.replace("\\\n", "<br>").replace('\n', " ").replace('|', "\\|"));
                }
            }
            out.push_str(if c == 0 { "| " } else { " | " });
            out.push_str(&cell);
        }
        out.push_str(" |\n");
        if r == 0 {
            for a in &aligns {
                out.push('|');
                out.push_str(match a {
                    Align::Center => ":-:",
                    Align::Right => "--:",
                    _ => "---",
                });
            }
            out.push_str("|\n");
        }
    }
}

/// Splits a link destination `url "title"` (the URL may be in <>).
fn link_dest(inner: &str) -> (&str, Option<&str>) {
    let inner = inner.trim();
    let (url, rest) = match inner.strip_prefix('<').and_then(|r| r.split_once('>')) {
        Some((u, rest)) => (u, rest),
        None => inner.split_at(inner.find(char::is_whitespace).unwrap_or(inner.len())),
    };
    let rest = rest.trim();
    let title = rest.strip_prefix('"').and_then(|r| r.strip_suffix('"')).or_else(|| rest.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')));
    (url, title)
}

/// Parses inline markup: **bold**, *italic*, _italic_, `code`, ~~strike~~,
/// [text](url "title"), <autolink>.
/// The next place of a pattern at or after a point, remembered so that
/// asking again further on doesn't search the line again: a long line of
/// openers without their closers would otherwise take quadratic time.
struct Ahead {
    pat: &'static str,
    at: usize,
    found: Option<usize>,
}

impl Ahead {
    fn new(pat: &'static str) -> Self {
        Ahead { pat, at: usize::MAX, found: None }
    }

    fn next(&mut self, src: &str, from: usize) -> Option<usize> {
        if self.at > from || self.found.is_some_and(|f| f < from) {
            self.at = from;
            self.found = src.get(from..).and_then(|r| r.find(self.pat)).map(|f| f + from);
        }
        self.found
    }
}

fn parse_inline(src: &str) -> Paragraph {
    let src = src.trim_end_matches('\\').trim_end();
    let mut p = Paragraph::new("");
    let mut attrs = Attrs::default();
    let mut text = String::new();
    let b = src.as_bytes();
    let mut i = 0;
    let (mut tick, mut bracket, mut link, mut link_gt, mut paren, mut gt, mut data) =
        (Ahead::new("`"), Ahead::new("]"), Ahead::new("]("), Ahead::new(">"), Ahead::new(")"), Ahead::new(">"), Ahead::new("](data:"));
    // The picture that didn't read, not to be decoded again from every `![` before it.
    let mut bad_image = None;
    // Where the last `$` and `$$` that didn't start an equation would have
    // ended: the ones before it can't start one either (a line of
    // unclosed dollars is read once, not once per dollar).
    let (mut math1, mut math2) = (0, 0);
    let flush = |text: &mut String, p: &mut Paragraph, attrs: &Attrs| {
        if !text.is_empty() {
            p.push(text, attrs.clone());
            text.clear();
        }
    };
    while i < b.len() {
        let rest = &src[i..];
        if b[i] == b'\\' && i + 1 < b.len() && b[i + 1].is_ascii_punctuation() {
            let ch = src[i + 1..].chars().next().unwrap();
            text.push(ch);
            i += 1 + ch.len_utf8();
            continue;
        }
        if rest.starts_with("**") || rest.starts_with("__") {
            flush(&mut text, &mut p, &attrs);
            attrs.bold = !attrs.bold;
            i += 2;
            continue;
        }
        if rest.starts_with("~~") {
            flush(&mut text, &mut p, &attrs);
            attrs.strike = !attrs.strike;
            i += 2;
            continue;
        }
        if b[i] == b'`' {
            if let Some(end) = tick.next(src, i + 1).map(|e| e - i - 1) {
                flush(&mut text, &mut p, &attrs);
                let mut a = attrs.clone();
                a.code = true;
                p.push(&src[i + 1..i + 1 + end], a);
                i += end + 2;
                continue;
            }
        }
        if b[i] == b'*' || (b[i] == b'_' && (i == 0 || !b[i - 1].is_ascii_alphanumeric())) || (b[i] == b'_' && attrs.italic) {
            flush(&mut text, &mut p, &attrs);
            attrs.italic = !attrs.italic;
            i += 1;
            continue;
        }
        // Equations: $...$ and $$...$$ (LaTeX).
        if b[i] == b'$' {
            let double = rest.starts_with("$$");
            if i >= if double { math2 } else { math1 } {
                match crate::mathml::md_math(rest) {
                    Ok((len, eq)) => {
                        flush(&mut text, &mut p, &attrs);
                        p.push(&OBJECT.to_string(), Attrs { math: Some(Arc::new(eq)), ..Default::default() });
                        i += len;
                        continue;
                    }
                    Err(n) if double => math2 = i + n,
                    Err(n) => math1 = i + n,
                }
            }
        }
        // Pictures embedded as data URIs (as written by `write`).
        if rest.starts_with("![") && let Some(c) = data.next(src, i) && bad_image != Some(c) {
            bad_image = Some(c);
            if let Some((len, img)) = parse_image(rest) {
                flush(&mut text, &mut p, &attrs);
                let a = Attrs { image: Some(Arc::new(img)), link: None, link_tip: None, ..attrs.clone() };
                p.push(&OBJECT.to_string(), a);
                i += len;
                continue;
            }
        }
        // Footnote references stay as text (see `attach_notes`).
        if rest.starts_with("[^") && let Some(end) = bracket.next(src, i).map(|e| e - i) && !rest[2..end].contains(char::is_whitespace) {
            text.push_str(&rest[..end + 1]);
            i += end + 1;
            continue;
        }
        if b[i] == b'[' {
            if let Some(close) = link.next(src, i).map(|c| c - i) {
                // A destination in <> may contain ')'.
                let from = if rest[close + 2..].starts_with('<') { link_gt.next(src, i + close).map_or(0, |g| g - i - close) } else { 0 };
                if let Some(end) = paren.next(src, i + close + from).map(|e| e - i - close) {
                    flush(&mut text, &mut p, &attrs);
                    let label = &rest[1..close];
                    let (url, title) = link_dest(&rest[close + 2..close + end]);
                    let mut a = attrs.clone();
                    a.link = Some(url.into());
                    a.link_tip = title.filter(|t| !t.is_empty()).map(Into::into);
                    p.push(label, a);
                    i += close + end + 1;
                    continue;
                }
            }
        }
        if b[i] == b'<' {
            if let Some(end) = gt.next(src, i).map(|e| e - i) {
                let inner = &rest[1..end];
                // CommonMark autolinks need a scheme (or are e-mail addresses).
                let target = apex_doc::autolink_target(inner).filter(|_| !inner.contains(char::is_whitespace) && !inner.to_ascii_lowercase().starts_with("www."));
                if let Some(target) = target {
                    flush(&mut text, &mut p, &attrs);
                    let mut a = attrs.clone();
                    a.link = Some(target.into());
                    p.push(inner, a);
                    i += end + 1;
                    continue;
                }
            }
        }
        let ch = rest.chars().next().unwrap();
        text.push(ch);
        i += ch.len_utf8();
    }
    flush(&mut text, &mut p, &attrs);
    p
}

/// The longest run of backticks in `s`.
fn ticks(s: &str) -> usize {
    s.split(|c| c != '`').map(str::len).max().unwrap_or(0)
}

/// `s` with what Markdown readers take as markup escaped.
fn escaped(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if matches!(ch, '*' | '_' | '`' | '[' | ']' | '\\' | '~' | '$' | '<' | '>') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

pub fn write(doc: &Document) -> String {
    let mut out = String::new();
    // Longer than any run of backticks in the code it holds.
    let mut fence = String::from("```");
    let mut prev: Option<ParaKind> = None;
    let mut top_list: Option<ParaKind> = None;
    let mut tables: HashMap<usize, TableMap> = doc.tables().into_iter().map(|m| (m.start, m)).collect();
    // All at once: one by one, each walks its list back.
    let numbers = doc.list_numbers();
    for (idx, p) in doc.paragraphs.iter().enumerate() {
        if p.cell.is_some() {
            if let Some(m) = tables.remove(&idx) {
                if prev == Some(ParaKind::Code) {
                    out.push_str(&fence);
                    out.push('\n');
                }
                if idx > 0 {
                    out.push('\n');
                }
                write_table(&mut out, doc, &m);
                prev = Some(ParaKind::Normal);
                top_list = None;
            }
            continue;
        }
        let kind = p.style.kind;
        if !kind.is_list() {
            top_list = None;
        }
        // Adjacent list items stay together unless a top-level list changes type.
        let both_list = prev.is_some_and(|k| k.is_list()) && kind.is_list() && (p.style.level > 0 || top_list == Some(kind));
        if kind.is_list() && p.style.level == 0 {
            top_list = Some(kind);
        }
        let both_code = prev == Some(ParaKind::Code) && kind == ParaKind::Code;
        if prev == Some(ParaKind::Code) && kind != ParaKind::Code {
            out.push_str(&fence);
            out.push('\n');
        }
        if idx > 0 && !both_list && !both_code {
            out.push('\n');
        }
        let indent = "  ".repeat(p.style.level as usize);
        match kind {
            ParaKind::Title => out.push_str("# "),
            ParaKind::Heading(n) => {
                out.push_str(&"#".repeat(n.clamp(1, 6) as usize));
                out.push(' ');
            }
            ParaKind::Quote => out.push_str("> "),
            ParaKind::Bullet => {
                out.push_str(&indent);
                out.push_str("- ");
            }
            ParaKind::Numbered => {
                out.push_str(&indent);
                out.push_str(&format!("{}. ", numbers[idx]));
            }
            ParaKind::Code => {
                if !both_code {
                    let run = doc.paragraphs[idx..].iter().take_while(|q| q.cell.is_none() && q.style.kind == ParaKind::Code).map(|q| ticks(&q.text)).max().unwrap_or(0);
                    fence = "`".repeat((run + 1).max(3));
                    out.push_str(&fence);
                    out.push('\n');
                }
                out.extend(p.text.chars().map(|c| if c == LINE_BREAK { '\n' } else { c }));
                out.push('\n');
                prev = Some(kind);
                continue;
            }
            ParaKind::Normal => {}
        }
        let cont = match kind {
            ParaKind::Quote => "> ".to_string(),
            ParaKind::Bullet | ParaKind::Numbered => "  ".repeat(p.style.level as usize + 1) + " ",
            _ => String::new(),
        };
        write_inline(&mut out, p, &cont);
        out.push('\n');
        prev = Some(kind);
    }
    if prev == Some(ParaKind::Code) {
        out.push_str(&fence);
        out.push('\n');
    }
    write_notes(out, doc)
}

/// Numbers the note marks (written as `OBJECT`) as `[^n]` and adds their
/// definitions at the end.
fn write_notes(out: String, doc: &Document) -> String {
    let notes = doc.notes();
    if notes.is_empty() {
        return out;
    }
    let mut s = String::with_capacity(out.len() + 64 * notes.len());
    let mut n = 0;
    for part in out.split(OBJECT) {
        if n > 0 {
            let _ = write!(s, "[^{n}]");
        }
        s.push_str(part);
        n += 1;
    }
    s.push('\n');
    for (i, (_, note)) in notes.iter().enumerate() {
        let _ = write!(s, "[^{}]: ", i + 1);
        for (k, p) in note.paras.iter().enumerate() {
            if k > 0 {
                s.push_str("\n    ");
            }
            let mut body = String::new();
            write_inline(&mut body, p, "    ");
            s.push_str(&body.replace(OBJECT, ""));
            s.push('\n');
        }
    }
    s
}

/// Writes whitespace, turning soft line breaks into Markdown hard breaks
/// followed by the block's continuation prefix.
fn push_ws(out: &mut String, s: &str, cont: &str) {
    for ch in s.chars() {
        if ch == LINE_BREAK {
            out.push_str("\\\n");
            out.push_str(cont);
        } else {
            out.push(ch);
        }
    }
}

/// `![alt](data:image/png;base64,...)` at the start of `src`: its length
/// and the picture (shown at 96 dpi).
fn parse_image(src: &str) -> Option<(usize, Image)> {
    let close = src.find("](data:")?;
    let alt = &src[2..close];
    if alt.contains(']') {
        return None;
    }
    let end = close + src[close..].find(')')?;
    let uri = &src[close + 2..end];
    let (_, b64) = uri.split_once(";base64,")?;
    let data = base64_decode(b64)?;
    let (w, h) = crate::odt_img::pixel_size(&data)?;
    let img = Image { alt: alt.to_string(), ..Image::new(Arc::from(data), w as f32 * 0.75, h as f32 * 0.75) };
    Some((end + 1, img))
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            if k <= c.len() {
                out.push(B64[(n >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub(crate) fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for ch in s.bytes() {
        let v = match ch {
            b'A'..=b'Z' => ch - b'A',
            b'a'..=b'z' => ch - b'a' + 26,
            b'0'..=b'9' => ch - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b' ' | b'\n' | b'\r' => continue,
            _ => return None,
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

fn write_inline(out: &mut String, p: &Paragraph, cont: &str) {
    for (s, e, a) in p.spans() {
        let t = &p.text[s..e];
        if t.is_empty() {
            continue;
        }
        if a.note.is_some() {
            // Numbered by `write_notes`.
            out.push_str(t);
            continue;
        }
        if let Some(f) = &a.field {
            out.push_str(&t.replace(OBJECT, &escaped(&f.cached)));
            continue;
        }
        if a.shape.is_some() {
            // Shapes have no Markdown.
            continue;
        }
        if let Some(img) = &a.image {
            let Some(data) = crate::picture::shown(img) else { continue };
            for _ in t.matches(OBJECT) {
                let mime = crate::docx_img::image_type(&data).1;
                let _ = write!(out, "![{}](data:{mime};base64,{})", img.alt.replace(['[', ']', '\n'], " "), base64_encode(&data));
            }
            continue;
        }
        if let Some(eq) = &a.math {
            for _ in t.matches(OBJECT) {
                crate::mathml::md_write(out, eq, apex_doc::math::is_display(p));
            }
            continue;
        }
        if a.code {
            // A delimiter longer than the backticks inside, spaced off
            // when the code starts or ends with one.
            let d = "`".repeat(ticks(t) + 1);
            let pad = if t.starts_with('`') || t.ends_with('`') { " " } else { "" };
            out.push_str(&d);
            out.push_str(pad);
            out.push_str(t);
            out.push_str(pad);
            out.push_str(&d);
            continue;
        }
        // Keep surrounding whitespace outside of markers.
        let lead = &t[..t.len() - t.trim_start().len()];
        let core = t.trim();
        let trail = &t[lead.len() + core.len()..];
        push_ws(out, lead, cont);
        if core.is_empty() {
            push_ws(out, trail, cont);
            continue;
        }
        let mut open = String::new();
        if a.bold {
            open.push_str("**");
        }
        if a.italic {
            open.push('*');
        }
        if a.strike {
            open.push_str("~~");
        }
        let close: String = open.chars().rev().collect();
        if a.link.is_some() {
            out.push('[');
        }
        out.push_str(&open);
        for ch in core.chars() {
            if ch == LINE_BREAK {
                push_ws(out, "\u{2028}", cont);
                continue;
            }
            // `<` too: some readers take what follows as HTML.
            if matches!(ch, '*' | '_' | '`' | '[' | ']' | '\\' | '~' | '$' | '<' | '>') {
                out.push('\\');
            }
            out.push(ch);
        }
        out.push_str(&close);
        if let Some(l) = &a.link {
            // No links that run script where the page is shown.
            let l = &crate::html::safe_href(l);
            out.push_str("](");
            if l.contains(|c: char| c.is_whitespace() || c == ')' || c == '(') {
                out.push('<');
                out.push_str(&l.replace('<', "%3C").replace('>', "%3E"));
                out.push('>');
            } else {
                out.push_str(l);
            }
            if let Some(t) = &a.link_tip {
                out.push_str(" \"");
                // One line, and no quote or backslash that ends it early.
                out.extend(t.chars().map(|c| match c {
                    '"' => '\'',
                    '\\' => '/',
                    c if c.is_control() => ' ',
                    c => c,
                }));
                out.push('"');
            }
            out.push(')');
        }
        push_ws(out, trail, cont);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unclosed_dollars_read_once() {
        let line = "$a\\\\".repeat(100_000) + " $x$";
        let t = std::time::Instant::now();
        let d = read(&line);
        assert!(t.elapsed() < std::time::Duration::from_secs(5));
        assert!(!d.paragraphs.is_empty());
        // The equations still read.
        assert!(!read("a $x$ b").paragraphs[0].text.contains('$'));
        assert!(!read("a $$x$$ b").paragraphs[0].text.contains('$'));
    }

    #[test]
    fn footnotes() {
        let md = "Text[^1] and more[^n].\n\n[^1]: First **note**.\n[^n]: Second\n\n    with two paragraphs.\n";
        let d = read(md);
        assert_eq!(d.paragraphs.len(), 1);
        let notes: Vec<String> = d.notes().iter().map(|(_, n)| n.text()).collect();
        assert_eq!(notes, ["First note.", "Second\nwith two paragraphs."]);
        assert!(d.notes()[0].1.paras[0].attrs_at(8).bold);
        let out = write(&d);
        assert_eq!(out, "Text[^1] and more[^2].\n\n[^1]: First **note**.\n[^2]: Second\n\n    with two paragraphs.\n");
        let back = read(&out);
        assert_eq!(back.notes().len(), 2);
    }

    #[test]
    fn roundtrip() {
        let md = "# Título\n\nTexto con **negrita**, *cursiva* y `código`.\n\n- uno\n- dos\n  - anidado\n\n1. a\n2. b\n\n> cita\n\n```\nfn main() {}\n```\n\nVer [web](https://x.y).\n\nlinea\\\nsiguiente\n";
        let d = read(md);
        assert_eq!(d.paragraphs[0].style.kind, ParaKind::Heading(1));
        assert_eq!(d.paragraphs[1].text, "Texto con negrita, cursiva y código.");
        assert!(d.paragraphs[1].runs[1].attrs.bold);
        assert_eq!(d.paragraphs[4].style.level, 1);
        assert_eq!(d.paragraphs.last().unwrap().text, "linea\u{2028}siguiente");
        assert_eq!(write(&d), md);
    }

    #[test]
    fn images() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend([0, 0, 0, 40, 0, 0, 0, 20, 8, 6, 0, 0, 0, 1]);
        for n in 0..3 {
            assert_eq!(base64_decode(&base64_encode(&png[..png.len() - n])).unwrap(), &png[..png.len() - n]);
        }
        let mut d = Document::from_plain_text("ab");
        let img = Image { alt: "A cat".into(), ..Image::new(Arc::from(png), 30.0, 15.0) };
        d.insert_image(apex_doc::Selection::caret(apex_doc::Pos::new(0, 1)), img);
        let md = write(&d);
        assert!(md.starts_with("a![A cat](data:image/png;base64,iVBORw0KGgo"), "{md}");
        let back = read(&md);
        assert_eq!(back.paragraphs[0].text, "a\u{FFFC}b");
        let (_, img) = back.image_at(apex_doc::Pos::new(0, 1)).unwrap();
        assert_eq!((img.width, img.height, img.alt.as_str()), (30.0, 15.0, "A cat"));
    }

    #[test]
    fn links() {
        let md = "Ver [aquí](#Marca), [doc](<a b.html> \"Título\") y <ana@x.es>.\n";
        let d = read(md);
        let p = &d.paragraphs[0];
        assert_eq!(p.text, "Ver aquí, doc y ana@x.es.");
        let links: Vec<_> = p.spans().filter_map(|(_, _, a)| a.link.clone().map(|l| (l, a.link_tip.clone()))).collect();
        assert_eq!(links[0].0.as_ref(), "#Marca");
        assert_eq!((links[1].0.as_ref(), links[1].1.as_deref()), ("a b.html", Some("Título")));
        assert_eq!(links[2].0.as_ref(), "mailto:ana@x.es");
        let back = read(&write(&d));
        assert_eq!(back.paragraphs[0].runs, p.runs);
    }

    #[test]
    fn restart() {
        let d = read("1. a\n2. b\n\n1. c\n");
        assert!(d.paragraphs[2].style.restart);
        assert_eq!(write(&d), "1. a\n2. b\n1. c\n");
        assert!(read(&write(&d)).paragraphs[2].style.restart);
    }
}
