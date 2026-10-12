//! Advanced Find and Replace: Word's special characters (^p, ^t...),
//! wildcards, regular expressions and formatting (find bold text, replace
//! with a style...).

use std::sync::Arc;

use regex::{Captures, RegexBuilder};

use crate::{Attrs, Document, EditKind, LINE_BREAK, ParaKind, ParaStyle, Pos, SOFT_HYPHEN, Selection};

/// How the query is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum FindMode {
    /// Literal text with Word's special characters (^p, ^t, ^?, ^#...).
    #[default]
    Plain,
    /// Word's wildcards: ? * @ [ ] [! ] { } < > ( ) and \1 in replacements.
    Wildcards,
    /// Regular expressions, with $1 in replacements.
    Regex,
}

/// Formatting to look for, or to give the replacement text: `None` leaves
/// a property out.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct FindFormat {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    /// Highlighted (any color) or not; replacing sets yellow.
    pub highlight: Option<bool>,
    pub font: Option<Arc<str>>,
    /// Half points.
    pub size: Option<u16>,
    pub color: Option<u32>,
    /// A paragraph style: a built-in kind (custom 0) or a custom style.
    pub para_style: Option<(ParaKind, u16)>,
}

impl FindFormat {
    pub fn is_empty(&self) -> bool {
        !self.has_char() && self.para_style.is_none()
    }

    fn has_char(&self) -> bool {
        self.bold.is_some() || self.italic.is_some() || self.underline.is_some() || self.strike.is_some() || self.highlight.is_some() || self.font.is_some() || self.size.is_some() || self.color.is_some()
    }

    fn para_ok(&self, st: &ParaStyle) -> bool {
        match self.para_style {
            None => true,
            Some((_, c)) if c > 0 => st.custom == c,
            Some((k, _)) => st.kind == k && st.custom == 0,
        }
    }

    /// Whether text with attributes `a` in a paragraph styled `st` has the
    /// character formatting.
    fn char_ok(&self, doc: &Document, st: &ParaStyle, a: &Attrs) -> bool {
        if !self.has_char() {
            return true;
        }
        let f = doc.char_format(st, a);
        let same = |want: Option<bool>, have: bool| want.is_none_or(|w| w == have);
        same(self.bold, f.bold)
            && same(self.italic, f.italic)
            && same(self.underline, f.underline)
            && same(self.strike, a.strike)
            && same(self.highlight, a.highlight.is_some())
            && self.font.as_deref().is_none_or(|w| f.font.as_deref().or(doc.default_font_name().as_deref()).is_some_and(|h| h.eq_ignore_ascii_case(w)))
            && self.size.is_none_or(|s| s == f.size)
            && self.color.is_none_or(|c| c == f.color.unwrap_or(0))
    }

    /// Gives `a` the character formatting.
    fn apply(&self, a: &mut Attrs) {
        if let Some(b) = self.bold {
            a.bold = b;
        }
        if let Some(b) = self.italic {
            a.italic = b;
        }
        if let Some(b) = self.underline {
            a.underline = b;
        }
        if let Some(b) = self.strike {
            a.strike = b;
        }
        if let Some(b) = self.highlight {
            a.highlight = b.then_some(0xFFFF00);
        }
        if self.font.is_some() {
            a.font.clone_from(&self.font);
        }
        if self.size.is_some() {
            a.size = self.size;
        }
        if self.color.is_some() {
            a.color = self.color;
        }
    }
}

/// Find options (the Find and Replace dialog's "More" part).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct FindOpts {
    pub case: bool,
    pub whole: bool,
    pub mode: FindMode,
    pub format: FindFormat,
}

/// The character of Word's `^n` code (a decimal character code).
fn code_char(n: u32) -> Option<char> {
    match n {
        13 => Some('\n'),
        11 => Some(LINE_BREAK),
        _ => char::from_u32(n),
    }
}

/// The regular expression of a Word `^` code (`c` is the character after
/// the caret; digits are read from `rest`). Returns it and the characters
/// used after the caret.
fn caret_code(c: char, rest: &[char]) -> Option<(String, usize)> {
    let lit = |c: char| regex::escape(&c.to_string());
    Some(match c {
        'p' => ("\n".into(), 1),
        't' => ("\t".into(), 1),
        'l' => (lit(LINE_BREAK), 1),
        '^' => (r"\^".into(), 1),
        '?' => (r"[^\n]".into(), 1),
        '#' => (r"\d".into(), 1),
        '$' => (r"\p{L}".into(), 1),
        'w' => (r"[ \t\u{a0}]+".into(), 1),
        's' => (r"\u{a0}".into(), 1),
        '+' => ("—".into(), 1),
        '=' => ("–".into(), 1),
        '~' => (r"\u{2011}".into(), 1),
        '-' => (r"\u{ad}".into(), 1),
        'g' => (lit(crate::OBJECT), 1),
        '0'..='9' => {
            let digits: String = std::iter::once(c).chain(rest.iter().copied().take_while(char::is_ascii_digit)).take(4).collect();
            let ch = code_char(digits.parse().ok()?)?;
            (lit(ch), digits.len())
        }
        _ => return None,
    })
}

/// The regular expression of a query.
fn pattern(query: &str, mode: FindMode) -> Result<String, String> {
    if mode == FindMode::Regex {
        return Ok(query.to_string());
    }
    let cs: Vec<char> = query.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let bad = |what: &str| Err(format!("{what}: {query}"));
    while i < cs.len() {
        let c = cs[i];
        i += 1;
        if c == '^' && i < cs.len() {
            match caret_code(cs[i], &cs[i + 1..]) {
                Some((re, used)) => {
                    out.push_str(&re);
                    i += used;
                }
                None => out.push_str(r"\^"),
            }
            continue;
        }
        if mode == FindMode::Plain {
            out.push_str(&regex::escape(&c.to_string()));
            continue;
        }
        match c {
            '?' => out.push_str(r"[^\n]"),
            '*' => out.push_str(r"[^\n]*?"),
            '@' => out.push('+'),
            '<' | '>' => out.push_str(r"\b"),
            '(' | ')' => out.push(c),
            '\\' if i < cs.len() => {
                out.push_str(&regex::escape(&cs[i].to_string()));
                i += 1;
            }
            '{' => {
                let Some(len) = cs[i..].iter().position(|&c| c == '}') else { return bad("{") };
                let inner: String = cs[i..i + len].iter().map(|&c| if c == ';' { ',' } else { c }).collect();
                if inner.is_empty() || !inner.chars().all(|c| c.is_ascii_digit() || c == ',') {
                    return bad("{}");
                }
                out.push('{');
                out.push_str(&inner);
                out.push('}');
                i += len + 1;
            }
            '[' => {
                let Some(len) = cs[i..].iter().position(|&c| c == ']') else { return bad("[") };
                let mut class = String::from("[");
                for (k, &c) in cs[i..i + len].iter().enumerate() {
                    match c {
                        '!' if k == 0 => class.push('^'),
                        '-' if k > 0 && k + 1 < len => class.push('-'),
                        _ => class.push_str(&regex::escape(&c.to_string())),
                    }
                }
                class.push(']');
                out.push_str(&class);
                i += len + 1;
            }
            _ => out.push_str(&regex::escape(&c.to_string())),
        }
    }
    Ok(out)
}

/// The replacement text of a match: Word's ^ codes, ^& for the match, \1
/// (wildcards) or $1 (regex) for its groups.
fn expand(with: &str, mode: FindMode, caps: Option<&Captures>, hay_match: &str) -> String {
    if let (FindMode::Regex, Some(caps)) = (mode, caps) {
        let mut out = String::new();
        caps.expand(with, &mut out);
        return out.replace("\\n", "\n").replace("\\t", "\t");
    }
    let cs: Vec<char> = with.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        i += 1;
        match c {
            '^' if i < cs.len() => {
                let d = cs[i];
                i += 1;
                match d {
                    'p' => out.push('\n'),
                    't' => out.push('\t'),
                    'l' => out.push(LINE_BREAK),
                    '^' => out.push('^'),
                    '&' => out.push_str(hay_match),
                    's' => out.push('\u{a0}'),
                    '+' => out.push('—'),
                    '=' => out.push('–'),
                    '~' => out.push('\u{2011}'),
                    '-' => out.push('\u{ad}'),
                    '0'..='9' => {
                        let digits: String = std::iter::once(d).chain(cs[i..].iter().copied().take_while(char::is_ascii_digit)).take(4).collect();
                        i += digits.len() - 1;
                        out.extend(digits.parse().ok().and_then(code_char));
                    }
                    _ => {
                        out.push('^');
                        out.push(d);
                    }
                }
            }
            '\\' if mode == FindMode::Wildcards && i < cs.len() => {
                let d = cs[i];
                i += 1;
                match d.to_digit(10) {
                    Some(n) => out.push_str(caps.and_then(|c| c.get(n as usize)).map_or("", |m| m.as_str())),
                    None => out.push(d),
                }
            }
            _ => out.push(c),
        }
    }
    out.replace('\0', "")
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Document {
    /// The document's text, paragraphs joined by '\n' (deleted text as
    /// NULs, optional hyphens left out), where each paragraph starts in it
    /// and where optional hyphens were taken out.
    fn search_text(&self) -> (String, Vec<usize>, Vec<usize>) {
        let mut hay = String::new();
        let mut starts = Vec::with_capacity(self.paragraphs.len());
        let mut gaps = Vec::new();
        for (i, p) in self.paragraphs.iter().enumerate() {
            if i > 0 {
                hay.push('\n');
            }
            starts.push(hay.len());
            let t = p.blank_deleted(p.text.clone());
            for (k, piece) in t.split(SOFT_HYPHEN).enumerate() {
                if k > 0 {
                    gaps.push(hay.len());
                }
                hay.push_str(piece);
            }
        }
        (hay, starts, gaps)
    }

    /// Whether the text `a..b` has the formatting `f`.
    fn format_ok(&self, f: &FindFormat, a: Pos, b: Pos) -> bool {
        if f.is_empty() {
            return true;
        }
        let last = if b.offset == 0 && b.para > a.para { b.para - 1 } else { b.para };
        (a.para..=last).all(|i| {
            let p = &self.paragraphs[i];
            let s = if i == a.para { a.offset } else { 0 };
            let e = if i == b.para { b.offset } else { p.len() };
            f.para_ok(&p.style) && (!f.has_char() || p.spans().filter(|&(rs, re, _)| re > rs && re > s && rs < e).all(|(_, _, at)| f.char_ok(self, &p.style, at)))
        })
    }

    /// Stretches of text with formatting `f`, for a search by format only.
    fn format_runs(&self, f: &FindFormat) -> Vec<(Pos, Pos)> {
        let mut out = Vec::new();
        for (i, p) in self.paragraphs.iter().enumerate() {
            if !f.para_ok(&p.style) || p.text.is_empty() {
                continue;
            }
            let mut cur: Option<(usize, usize)> = None;
            for (s, e, a) in p.spans().filter(|(s, e, _)| e > s) {
                let ok = !a.is_deleted() && f.char_ok(self, &p.style, a);
                match (&mut cur, ok) {
                    (Some(c), true) if c.1 == s => c.1 = e,
                    (_, true) => {
                        if let Some((x, y)) = cur.replace((s, e)) {
                            out.push((Pos::new(i, x), Pos::new(i, y)));
                        }
                    }
                    (_, false) => {
                        if let Some((x, y)) = cur.take() {
                            out.push((Pos::new(i, x), Pos::new(i, y)));
                        }
                    }
                }
            }
            if let Some((x, y)) = cur {
                out.push((Pos::new(i, x), Pos::new(i, y)));
            }
        }
        out
    }

    /// Matches of `query` and, with `with`, their replacement texts.
    fn scan(&self, query: &str, o: &FindOpts, with: Option<&str>) -> Result<Vec<(Pos, Pos, String)>, String> {
        if query.is_empty() {
            let texts = |a: Pos, b: Pos| with.map(|w| expand(w, FindMode::Plain, None, &self.text_range(a, b))).unwrap_or_default();
            return Ok(if o.format.is_empty() { Vec::new() } else { self.format_runs(&o.format).into_iter().map(|(a, b)| (a, b, texts(a, b))).collect() });
        }
        let re = RegexBuilder::new(&pattern(query, o.mode)?).case_insensitive(!o.case).multi_line(true).build().map_err(|e| e.to_string())?;
        let (hay, starts, gaps) = self.search_text();
        // A match starts after the optional hyphens before it and ends
        // before the ones after it.
        let pos = |off: usize, start: bool| {
            let i = starts.partition_point(|&s| s <= off) - 1;
            let lo = gaps.partition_point(|&g| g < starts[i]);
            let hi = if start { gaps.partition_point(|&g| g <= off) } else { gaps.partition_point(|&g| g < off) };
            Pos::new(i, off - starts[i] + (hi - lo) * SOFT_HYPHEN.len_utf8())
        };
        let mut out = Vec::new();
        for caps in re.captures_iter(&hay) {
            let m = caps.get(0).unwrap();
            if m.as_str().chars().all(|c| c == '\0') {
                continue;
            }
            let (a, b) = (pos(m.start(), true), pos(m.end(), false));
            if a.para != b.para && !self.one_container(a.para, b.para) {
                continue;
            }
            if o.whole && (hay[..m.start()].chars().next_back().is_some_and(is_word) || hay[m.end()..].chars().next().is_some_and(is_word)) {
                continue;
            }
            if !self.format_ok(&o.format, a, b) {
                continue;
            }
            let text = with.map(|w| expand(w, o.mode, Some(&caps), m.as_str())).unwrap_or_default();
            out.push((a, b, text));
        }
        Ok(out)
    }

    /// Every match of `query` (an error for a bad pattern). An empty query
    /// with a format finds the text with that format.
    pub fn find_ex(&self, query: &str, o: &FindOpts) -> Result<Vec<(Pos, Pos)>, String> {
        Ok(self.scan(query, o, None)?.into_iter().map(|(a, b, _)| (a, b)).collect())
    }

    /// Inserts `text` (may hold '\n') at `pos` without recording undo;
    /// returns its end.
    fn raw_insert(&mut self, mut pos: Pos, text: &str, attrs: &Attrs) -> Pos {
        let mut lines = text.split('\n');
        if let Some(first) = lines.next() {
            self.paragraphs[pos.para].insert_str(pos.offset, first, attrs.clone());
            pos.offset += first.len();
        }
        for line in lines {
            let tail = self.paragraphs[pos.para].split_off(pos.offset);
            self.paragraphs.insert(pos.para + 1, tail);
            pos = Pos::new(pos.para + 1, 0);
            self.paragraphs[pos.para].insert_str(0, line, attrs.clone());
            pos.offset = line.len();
        }
        pos
    }

    /// Replaces the matches of `query` (only `only`, when given) with
    /// `with`, giving the new text the formatting `fmt`; an empty `with`
    /// with a format keeps the text and formats it. One undo step. Returns
    /// the count and the end of the first replacement.
    pub fn replace_ex(&mut self, query: &str, with: &str, o: &FindOpts, fmt: &FindFormat, only: Option<(Pos, Pos)>, sel: Selection) -> Result<(usize, Pos), String> {
        let mut hits = self.scan(query, o, Some(with))?;
        if let Some(h) = only {
            hits.retain(|&(a, b, _)| (a, b) == h);
        }
        let Some(first) = hits.first().map(|h| h.0.para) else { return Ok((0, sel.cursor)) };
        let last = hits.iter().map(|h| h.1.para).max().unwrap_or(first);
        let before = self.paragraphs[first..=last].to_vec();
        let n0 = self.paragraphs.len();
        let keep_text = with.is_empty() && !fmt.is_empty();
        let styles = self.styles.clone();
        let restyle = |d: &mut Document, from: usize, to: usize| {
            if let Some((k, c)) = fmt.para_style {
                for p in &mut d.paragraphs[from..=to] {
                    styles.restyle(&mut p.style, k, c);
                }
            }
        };
        let mut caret = sel.cursor;
        for (a, b, text) in hits.iter().rev() {
            let (a, b) = (*a, *b);
            if keep_text {
                for i in a.para..=b.para {
                    let p = &mut self.paragraphs[i];
                    let s = if i == a.para { a.offset } else { 0 };
                    let e = if i == b.para { b.offset } else { p.len() };
                    p.map_attrs(s, e, &|x| fmt.apply(x));
                }
                let to = if b.offset == 0 && b.para > a.para { b.para - 1 } else { b.para };
                restyle(self, a.para, to);
                caret = b;
                continue;
            }
            let p = &self.paragraphs[a.para];
            let next = p.text[a.offset..].chars().next().filter(|_| a.para < b.para || a.offset < b.offset).map_or(0, char::len_utf8);
            let mut attrs = p.attrs_at(a.offset + next);
            attrs.clear_object();
            self.stamp(&mut attrs);
            fmt.apply(&mut attrs);
            let at = self.raw_delete(a, b);
            let end = self.raw_insert(at, text, &attrs);
            restyle(self, at.para, end.para);
            caret = end;
        }
        let after_len = (last + 1 - first + self.paragraphs.len()).saturating_sub(n0);
        let caret = self.clamp(caret);
        self.record(first, before, after_len, sel, Selection::caret(caret), EditKind::Replace);
        Ok((hits.len(), caret))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Paragraph;

    fn doc(lines: &[&str]) -> Document {
        Document::from_paragraphs(lines.iter().map(|t| Paragraph::new(*t)).collect())
    }

    fn texts(d: &Document) -> Vec<String> {
        d.paragraphs.iter().map(|p| p.text.clone()).collect()
    }

    fn opts(mode: FindMode) -> FindOpts {
        FindOpts { mode, ..Default::default() }
    }

    #[test]
    fn optional_hyphens_are_skipped() {
        let d = doc(&["x", "la biblio\u{AD}teca y \u{AD}otra"]);
        let o = FindOpts::default();
        let h = SOFT_HYPHEN.len_utf8();
        assert_eq!(d.find_ex("biblioteca", &o).unwrap(), [(Pos::new(1, 3), Pos::new(1, 13 + h))]);
        assert_eq!(d.find_ex("biblio", &o).unwrap(), [(Pos::new(1, 3), Pos::new(1, 9))]);
        assert_eq!(d.find_ex("teca", &o).unwrap(), [(Pos::new(1, 9 + h), Pos::new(1, 13 + h))]);
        assert_eq!(d.find_ex("otra", &o).unwrap(), [(Pos::new(1, 16 + 2 * h), Pos::new(1, 20 + 2 * h))]);
    }

    #[test]
    fn special_characters() {
        let mut d = doc(&["uno", "dos\ttres", "cuatro 12"]);
        let o = FindOpts::default();
        assert_eq!(d.find_ex("o^pd", &o).unwrap(), [(Pos::new(0, 2), Pos::new(1, 1))]);
        assert_eq!(d.find_ex("^t", &o).unwrap().len(), 1);
        assert_eq!(d.find_ex("^#^#", &o).unwrap(), [(Pos::new(2, 7), Pos::new(2, 9))]);
        // Joining paragraphs and splitting them again.
        let s = Selection::caret(Pos::new(0, 0));
        assert_eq!(d.replace_ex("^p", " / ", &o, &FindFormat::default(), None, s).unwrap().0, 2);
        assert_eq!(texts(&d), ["uno / dos\ttres / cuatro 12"]);
        d.replace_ex(" / ", "^p", &o, &FindFormat::default(), None, s).unwrap();
        d.replace_ex("^t", "-^&-", &o, &FindFormat::default(), None, s).unwrap();
        assert_eq!(texts(&d), ["uno", "dos-\t-tres", "cuatro 12"]);
        d.undo();
        d.undo();
        d.undo();
        assert_eq!(texts(&d), ["uno", "dos\ttres", "cuatro 12"]);
    }

    #[test]
    fn wildcards_and_regex() {
        let mut d = doc(&["García, Ana; López, Luis", "colour color coloor"]);
        let w = opts(FindMode::Wildcards);
        let s = Selection::caret(Pos::new(0, 0));
        // Swap "Surname, Name" with groups.
        let n = d.replace_ex("(<[A-Z][a-zí]@), ([A-Z][a-z]@>)", "\\2 \\1", &FindOpts { case: true, ..w.clone() }, &FindFormat::default(), None, s).unwrap().0;
        assert_eq!(n, 1, "{:?}", texts(&d));
        assert_eq!(d.paragraphs[0].text, "Ana García; López, Luis");
        assert_eq!(d.find_ex("col?r", &w).unwrap().len(), 1);
        assert_eq!(d.find_ex("col*r", &w).unwrap().len(), 3);
        assert_eq!(d.find_ex("colo{1,2}r", &w).unwrap().len(), 2);
        assert_eq!(d.find_ex("colo{1;2}r", &w).unwrap().len(), 2);
        assert_eq!(d.find_ex("[!u]r", &w).unwrap().len(), 3);
        assert!(d.find_ex("[abc", &w).is_err());
        let r = opts(FindMode::Regex);
        assert_eq!(d.find_ex(r"colou?r\b", &r).unwrap().len(), 2);
        assert!(d.find_ex("(", &r).is_err());
        d.replace_ex(r"(\w+), (\w+)", "$2 $1", &r, &FindFormat::default(), None, s).unwrap();
        assert_eq!(d.paragraphs[0].text, "Ana García; Luis López");
        // ^ and $ are paragraph ends.
        assert_eq!(d.find_ex("^col", &r).unwrap(), [(Pos::new(1, 0), Pos::new(1, 3))]);
    }

    #[test]
    fn formats() {
        let mut d = doc(&["uno dos tres", "cuatro"]);
        d.format_range(Selection { anchor: Pos::new(0, 4), cursor: Pos::new(0, 7) }, &|a| a.bold = true);
        let bold = FindFormat { bold: Some(true), ..Default::default() };
        let o = FindOpts { format: bold.clone(), ..Default::default() };
        assert_eq!(d.find_ex("", &o).unwrap(), [(Pos::new(0, 4), Pos::new(0, 7))]);
        assert!(d.find_ex("uno", &o).unwrap().is_empty());
        assert_eq!(d.find_ex("o", &o).unwrap().len(), 1);
        // Bold becomes italic, the text stays.
        let s = Selection::caret(Pos::new(0, 0));
        let it = FindFormat { bold: Some(false), italic: Some(true), ..Default::default() };
        d.replace_ex("", "", &o, &it, None, s).unwrap();
        assert!(d.paragraphs[0].attrs_at(6).italic && !d.paragraphs[0].attrs_at(6).bold);
        // Text into a heading.
        let h = FindFormat { para_style: Some((ParaKind::Heading(2), 0)), ..Default::default() };
        d.replace_ex("cuatro", "Cuatro", &FindOpts::default(), &h, None, s).unwrap();
        assert_eq!(d.paragraphs[1].style.kind, ParaKind::Heading(2));
        let o = FindOpts { format: h, ..Default::default() };
        assert_eq!(d.find_ex("", &o).unwrap(), [(Pos::new(1, 0), Pos::new(1, 6))]);
        // Replacing one match only.
        let (n, end) = d.replace_ex("o", "0", &FindOpts::default(), &FindFormat::default(), Some((Pos::new(0, 2), Pos::new(0, 3))), s).unwrap();
        assert_eq!((n, end), (1, Pos::new(0, 3)));
        assert_eq!(d.paragraphs[0].text, "un0 dos tres");
    }
}
