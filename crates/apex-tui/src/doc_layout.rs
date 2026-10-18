//! A document laid out for the terminal: paragraphs wrapped to the width,
//! headings and emphasis as terminal attributes, list markers, tables drawn
//! with box characters, pictures and other objects as bracketed marks, and
//! the notes at the end. Every piece of text remembers where it comes from
//! in the document so that the caret and the mouse can map to it.

use apex_doc::{Align, Attrs, Document, LINE_BREAK, Marker, NoteKind, OBJECT, ParaKind, Paragraph, RevKind, SOFT_HYPHEN};
use apex_i18n::tr;
use ratatui::style::{Modifier, Style};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::term::{Colors, luminance};

pub const ACCENT: u32 = 0x4F9DE8;

/// Where a piece of text comes from: paragraph and byte range. An atomic
/// piece is one character shown as several (a picture's mark, a tab).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Src {
    pub para: usize,
    pub start: usize,
    pub end: usize,
    pub atomic: bool,
}

#[derive(Clone, Debug)]
pub struct Seg {
    pub x: u16,
    pub text: String,
    pub style: Style,
    pub src: Option<Src>,
}

impl Seg {
    pub fn width(&self) -> u16 {
        self.text.width() as u16
    }
}

#[derive(Clone, Debug, Default)]
pub struct Line {
    pub segs: Vec<Seg>,
}

#[derive(Default)]
pub struct Layout {
    pub lines: Vec<Line>,
    /// First and last line of each paragraph (`usize::MAX` when it has none).
    pub para_lines: Vec<(usize, usize)>,
    /// Headings: paragraph, level and text.
    pub headings: Vec<(usize, u8, String)>,
}

/// One character (or object mark) before wrapping.
struct Item {
    text: String,
    w: u16,
    style: Style,
    src: Option<Src>,
    space: bool,
    /// A line break follows.
    brk: bool,
}

/// Notes met in the text, shown at the end.
struct Notes {
    foot: Vec<(String, Vec<Paragraph>)>,
    end: Vec<(String, Vec<Paragraph>)>,
}

fn roman(mut n: usize) -> String {
    if n > 3999 {
        return n.to_string();
    }
    const T: [(usize, &str); 7] = [(100, "c"), (90, "xc"), (50, "l"), (40, "xl"), (10, "x"), (9, "ix"), (5, "v")];
    let mut s = String::new();
    for (v, r) in T {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s.push_str(&["", "i", "ii", "iii", "iv"][n.min(4)]);
    s
}

fn attr_style(a: &Attrs, colors: Colors) -> Style {
    let mut s = Style::new();
    let mut m = Modifier::empty();
    if a.bold {
        m |= Modifier::BOLD;
    }
    if a.italic {
        m |= Modifier::ITALIC;
    }
    if a.underline {
        m |= Modifier::UNDERLINED;
    }
    if a.strike {
        m |= Modifier::CROSSED_OUT;
    }
    if let Some(c) = a.color
        && (0.2..=0.85).contains(&luminance(c))
    {
        s = s.fg(colors.color(c));
    }
    if let Some(h) = a.highlight {
        s = s.bg(colors.color(h)).fg(colors.color(if luminance(h) > 0.5 { 0x101010 } else { 0xF0F0F0 }));
    }
    if a.code {
        s = s.fg(colors.color(0xCE9178));
    }
    if a.link.is_some() {
        m |= Modifier::UNDERLINED;
        s = s.fg(colors.color(ACCENT));
    }
    match a.rev.as_deref().map(|r| r.kind) {
        Some(RevKind::Insert) => {
            m |= Modifier::UNDERLINED;
            s = s.fg(colors.color(0x6CC66C));
        }
        Some(RevKind::Delete) => {
            m |= Modifier::CROSSED_OUT;
            s = s.fg(colors.color(0xE06C6C));
        }
        None => {}
    }
    s.add_modifier(m)
}

/// The base style of a paragraph kind.
fn kind_style(k: ParaKind, colors: Colors) -> Style {
    match k {
        ParaKind::Title => Style::new().fg(colors.color(ACCENT)).add_modifier(Modifier::BOLD),
        ParaKind::Heading(1) => Style::new().fg(colors.color(ACCENT)).add_modifier(Modifier::BOLD),
        ParaKind::Heading(2) => Style::new().fg(colors.color(0x7FB8F0)).add_modifier(Modifier::BOLD),
        ParaKind::Heading(_) => Style::new().add_modifier(Modifier::BOLD),
        ParaKind::Quote => Style::new().add_modifier(Modifier::ITALIC),
        ParaKind::Code => Style::new().fg(colors.color(0xCE9178)),
        _ => Style::new(),
    }
}

struct Builder<'a> {
    doc: &'a Document,
    colors: Colors,
    markers: Vec<Option<Marker>>,
    notes: Notes,
    out: Layout,
}

impl Builder<'_> {
    /// Items of a paragraph; `map` keeps where they come from.
    fn items(&mut self, i: usize, p: &Paragraph, base: Style, map: bool) -> Vec<Item> {
        let mut v = Vec::with_capacity(p.text.len());
        for (s, e, a) in p.spans() {
            let st = base.patch(attr_style(a, self.colors));
            for (off, ch) in p.text[s..e].char_indices() {
                let start = s + off;
                let src = map.then_some(Src { para: i, start, end: start + ch.len_utf8(), atomic: false });
                let atomic = |text: String, style: Style| {
                    let w = text.width() as u16;
                    Item { text, w, style, src: src.map(|x| Src { atomic: true, ..x }), space: false, brk: false }
                };
                let item = match ch {
                    OBJECT => {
                        let dim = st.add_modifier(Modifier::DIM);
                        if let Some(img) = &a.image {
                            let name = if !img.alt.trim().is_empty() { img.alt.trim() } else if !img.name.is_empty() { img.name.as_str() } else { "" };
                            let t = if name.is_empty() { format!("[{}]", tr("tui.image")) } else { format!("[{}: {}]", tr("tui.image"), name) };
                            atomic(t, dim.fg(self.colors.color(0x9CDCFE)))
                        } else if let Some(n) = &a.note {
                            let mark = match n.kind {
                                NoteKind::Foot => {
                                    let m = (self.notes.foot.len() + 1).to_string();
                                    self.notes.foot.push((m.clone(), n.paras.clone()));
                                    m
                                }
                                NoteKind::End => {
                                    let m = roman(self.notes.end.len() + 1);
                                    self.notes.end.push((m.clone(), n.paras.clone()));
                                    m
                                }
                            };
                            atomic(format!("[{mark}]"), st.fg(self.colors.color(ACCENT)))
                        } else if let Some(f) = &a.field {
                            let t = if f.cached.is_empty() { format!("[{}]", tr("tui.field")) } else { f.cached.clone() };
                            atomic(t.replace(['\n', '\r'], " "), st)
                        } else if a.math.is_some() {
                            atomic(format!("[{}]", tr("tui.equation")), dim)
                        } else if let Some(sh) = &a.shape {
                            let t = sh.shape.plain_text();
                            let t = t.trim();
                            atomic(if t.is_empty() { format!("[{}]", tr("tui.shape")) } else { format!("[{}]", t.replace(['\n', '\r'], " ")) }, dim)
                        } else {
                            atomic("◻".to_string(), dim)
                        }
                    }
                    LINE_BREAK => Item { brk: true, ..atomic(String::new(), st) },
                    SOFT_HYPHEN => atomic(String::new(), st),
                    '\t' => atomic("    ".to_string(), st),
                    c if c.is_control() => atomic(String::new(), st),
                    c => Item { text: c.to_string(), w: c.width().unwrap_or(0) as u16, style: st, src, space: c.is_whitespace(), brk: false },
                };
                v.push(item);
            }
        }
        v
    }

    /// Greedy wrap: indices of the items of each line.
    fn wrap(items: &[Item], first: u16, rest: u16) -> Vec<std::ops::Range<usize>> {
        let mut lines = Vec::new();
        let mut start = 0;
        let mut cur = 0u16;
        let mut brk_at: Option<usize> = None;
        let mut limit = first.max(1);
        let mut i = 0;
        while i < items.len() {
            let it = &items[i];
            if cur + it.w > limit && i > start && !it.space {
                let cut = match brk_at {
                    Some(b) if b > start => b,
                    _ => i,
                };
                lines.push(start..cut);
                start = cut;
                cur = items[cut..i].iter().map(|x| x.w).sum();
                brk_at = None;
                limit = rest.max(1);
            }
            cur += it.w;
            if it.space {
                brk_at = Some(i + 1);
            }
            if it.brk {
                lines.push(start..i + 1);
                start = i + 1;
                cur = 0;
                brk_at = None;
                limit = rest.max(1);
            }
            i += 1;
        }
        if start < items.len() || lines.is_empty() {
            lines.push(start..items.len());
        }
        lines
    }

    /// Segments of one wrapped line from `x`.
    fn segs(items: &[Item], mut x: u16) -> Vec<Seg> {
        let mut out: Vec<Seg> = Vec::new();
        for it in items {
            let joinable = |s: &Seg| s.style == it.style && matches!((s.src, it.src), (Some(a), Some(b)) if !a.atomic && !b.atomic && a.para == b.para && a.end == b.start) || (s.src.is_none() && it.src.is_none() && s.style == it.style);
            match out.last_mut() {
                Some(s) if joinable(s) => {
                    s.text.push_str(&it.text);
                    if let (Some(a), Some(b)) = (&mut s.src, it.src) {
                        a.end = b.end;
                    }
                }
                _ => out.push(Seg { x, text: it.text.clone(), style: it.style, src: it.src }),
            }
            x += it.w;
        }
        out
    }

    /// Lays out paragraph `i` into lines of `width` from `x0`.
    fn paragraph(&mut self, i: usize, p: &Paragraph, x0: u16, width: u16, map: bool) -> Vec<Line> {
        let st = p.style;
        let base = kind_style(st.kind, self.colors);
        let mut prefix = String::new();
        let mut prefix_style = base;
        let mut indent = 0u16;
        if st.kind.is_list() || matches!(st.kind, ParaKind::Heading(_)) {
            indent = if st.kind.is_list() { 2 * st.level as u16 } else { 0 };
            match self.markers.get(i).cloned().flatten() {
                Some(Marker::Bullet(c)) => prefix = format!("{c} "),
                Some(Marker::Text(t)) if !t.is_empty() => prefix = format!("{t} "),
                _ if st.kind.is_list() => prefix = "• ".to_string(),
                _ => {}
            }
            prefix_style = prefix_style.fg(self.colors.color(ACCENT));
        } else if st.kind == ParaKind::Quote {
            prefix = "▌ ".to_string();
            prefix_style = Style::new().fg(self.colors.color(0x808080));
        } else if st.level > 0 {
            indent = 2 * st.level as u16;
        }
        let pw = prefix.width() as u16;
        let avail = width.saturating_sub(indent + pw).max(8);
        let items = self.items(i, p, base, map);
        let mut lines = Vec::new();
        if items.is_empty() {
            let mut segs = Vec::new();
            if pw > 0 {
                segs.push(Seg { x: x0 + indent, text: prefix.clone(), style: prefix_style, src: None });
            }
            segs.push(Seg { x: x0 + indent + pw, text: String::new(), style: base, src: map.then_some(Src { para: i, start: 0, end: 0, atomic: false }) });
            lines.push(Line { segs });
            return lines;
        }
        for (n, r) in Self::wrap(&items, avail, avail).into_iter().enumerate() {
            let mut segs = Vec::new();
            if pw > 0 && (n == 0 || st.kind == ParaKind::Quote) {
                segs.push(Seg { x: x0 + indent, text: prefix.clone(), style: prefix_style, src: None });
            }
            let mut body = Self::segs(&items[r.clone()], x0 + indent + pw);
            // Alignment of the line (trailing spaces left out).
            let used: u16 = items[r].iter().map(|x| x.w).sum::<u16>();
            let trimmed = used - body.last().map_or(0, |s| (s.text.width() - s.text.trim_end().width()) as u16);
            let shift = match st.align {
                Align::Center => avail.saturating_sub(trimmed) / 2,
                Align::Right => avail.saturating_sub(trimmed),
                _ => 0,
            };
            for s in &mut body {
                s.x += shift;
            }
            segs.extend(body);
            lines.push(Line { segs });
        }
        lines
    }

    fn push_lines(&mut self, i: Option<usize>, lines: Vec<Line>) {
        for l in lines {
            let n = self.out.lines.len();
            if let Some(i) = i {
                let e = &mut self.out.para_lines[i];
                if e.0 == usize::MAX {
                    e.0 = n;
                }
                e.1 = n;
            }
            self.out.lines.push(l);
        }
    }

    fn blank(&mut self) {
        self.out.lines.push(Line::default());
    }

    fn rule(&mut self, x: u16, w: u16, ch: char, style: Style) {
        self.out.lines.push(Line { segs: vec![Seg { x, text: ch.to_string().repeat(w as usize), style, src: None }] });
    }

    /// A table: the paragraphs `from..to` (all in one table).
    fn table(&mut self, from: usize, to: usize, x0: u16, width: u16) {
        let doc = self.doc;
        let mut cells: std::collections::BTreeMap<(u32, u16), Vec<usize>> = Default::default();
        for i in from..to {
            if let Some(c) = &doc.paragraphs[i].cell {
                cells.entry((c.row, c.col)).or_default().push(i);
            }
        }
        let ncols = cells.keys().map(|k| k.1 as usize + 1).max().unwrap_or(1);
        let nrows = cells.keys().map(|k| k.0 as usize + 1).max().unwrap_or(1);
        // Natural and least widths of the columns.
        let mut natural = vec![1u16; ncols];
        let mut least = vec![1u16; ncols];
        for (&(_, c), paras) in &cells {
            for &i in paras {
                let t = &doc.paragraphs[i].text;
                let w = t.width().min(60) as u16;
                let word = t.split_whitespace().map(|x| x.width()).max().unwrap_or(0).min(20) as u16;
                natural[c as usize] = natural[c as usize].max(w);
                least[c as usize] = least[c as usize].max(word);
            }
        }
        let avail = width.saturating_sub(3 * ncols as u16 + 1).max(ncols as u16);
        let widths: Vec<u16> = if natural.iter().sum::<u16>() <= avail {
            natural.clone()
        } else {
            let mut w: Vec<u16> = least.iter().map(|&l| l.max(3)).collect();
            let mut total: u16 = w.iter().sum();
            if total > avail {
                // Even the words don't fit: share the room.
                w = vec![(avail / ncols as u16).max(1); ncols];
                total = w.iter().sum();
            }
            let spare = avail.saturating_sub(total) as u32;
            let want: u32 = natural.iter().zip(&w).map(|(n, x)| n.saturating_sub(*x) as u32).sum();
            if want > 0 {
                for k in 0..ncols {
                    w[k] += (natural[k].saturating_sub(w[k]) as u32 * spare / want) as u16;
                }
            }
            w
        };
        let border = Style::new().fg(self.colors.color(0x808080));
        let edge = |l: char, m: char, r: char| {
            let mut s = String::from(l);
            for (k, w) in widths.iter().enumerate() {
                s.push_str(&"─".repeat(*w as usize + 2));
                s.push(if k + 1 == ncols { r } else { m });
            }
            Line { segs: vec![Seg { x: x0, text: s, style: border, src: None }] }
        };
        self.out.lines.push(edge('┌', '┬', '┐'));
        for r in 0..nrows {
            // Each cell's lines, with the paragraph they belong to.
            let mut col_lines: Vec<Vec<(usize, Line)>> = Vec::with_capacity(ncols);
            let mut x = x0 + 2;
            for (c, &cw) in widths.iter().enumerate() {
                let mut ls = Vec::new();
                for &i in cells.get(&(r as u32, c as u16)).map(Vec::as_slice).unwrap_or(&[]) {
                    let p = &doc.paragraphs[i];
                    ls.extend(self.paragraph(i, p, x, cw, true).into_iter().map(|l| (i, l)));
                }
                col_lines.push(ls);
                x += cw + 3;
            }
            let height = col_lines.iter().map(Vec::len).max().unwrap_or(0).max(1);
            for n in 0..height {
                let line_no = self.out.lines.len();
                let mut segs = Vec::new();
                let mut x = x0;
                for (c, &cw) in widths.iter().enumerate() {
                    segs.push(Seg { x, text: "│".to_string(), style: border, src: None });
                    if let Some((i, l)) = col_lines[c].get(n) {
                        segs.extend(l.segs.iter().cloned());
                        let e = &mut self.out.para_lines[*i];
                        if e.0 == usize::MAX {
                            e.0 = line_no;
                        }
                        e.1 = line_no;
                    }
                    x += cw + 3;
                }
                segs.push(Seg { x, text: "│".to_string(), style: border, src: None });
                self.out.lines.push(Line { segs });
            }
            self.out.lines.push(if r + 1 == nrows { edge('└', '┴', '┘') } else { edge('├', '┼', '┤') });
        }
    }
}

/// Lays out the whole document for a text column `width` wide.
pub fn build(doc: &Document, width: u16, colors: Colors) -> Layout {
    let width = width.max(10);
    let mut b = Builder {
        doc,
        colors,
        markers: doc.list_markers(),
        notes: Notes { foot: Vec::new(), end: Vec::new() },
        out: Layout { lines: Vec::new(), para_lines: vec![(usize::MAX, usize::MAX); doc.paragraphs.len()], headings: Vec::new() },
    };
    for (i, l) in doc.headings() {
        b.out.headings.push((i, l, doc.paragraphs[i].text.chars().filter(|&c| c != OBJECT && c != SOFT_HYPHEN).map(|c| if c == LINE_BREAK || c == '\t' { ' ' } else { c }).collect::<String>().trim().to_string()));
    }
    let n = doc.paragraphs.len();
    let mut i = 0;
    let mut prev: Option<&Paragraph> = None;
    while i < n {
        let p = &doc.paragraphs[i];
        if let Some(cell) = &p.cell {
            // The whole table.
            let t = cell.table;
            let mut j = i;
            while j < n && doc.paragraphs[j].cell.as_ref().is_some_and(|c| c.table == t) {
                j += 1;
            }
            if prev.is_some() {
                b.blank();
            }
            b.table(i, j, 0, width);
            prev = Some(&doc.paragraphs[j - 1]);
            i = j;
            continue;
        }
        // Space between paragraphs as blank lines.
        if let Some(q) = prev {
            let is_head = |k: &ParaKind| matches!(k, ParaKind::Heading(_) | ParaKind::Title);
            let heading = is_head(&p.style.kind) || is_head(&q.style.kind);
            let gap = q.style.spacing().1 + p.style.spacing().0;
            let same_run = (q.style.kind.is_list() && p.style.kind.is_list()) || (q.style.kind == p.style.kind && matches!(p.style.kind, ParaKind::Code | ParaKind::Quote));
            if q.cell.is_some() || heading || (gap >= 6.0 && !same_run) || p.style.page_break_before {
                b.blank();
            }
        }
        let lines = b.paragraph(i, p, 0, width, true);
        let head_w = lines.iter().map(|l| l.segs.iter().map(|s| s.x + s.width()).max().unwrap_or(0)).max().unwrap_or(0);
        b.push_lines(Some(i), lines);
        match p.style.kind {
            ParaKind::Title => b.rule(0, head_w.min(width), '━', Style::new().fg(colors.color(ACCENT))),
            ParaKind::Heading(1) => b.rule(0, head_w.min(width), '─', Style::new().fg(colors.color(ACCENT))),
            _ => {}
        }
        prev = Some(p);
        i += 1;
    }
    // Notes at the end.
    let notes = std::mem::replace(&mut b.notes, Notes { foot: Vec::new(), end: Vec::new() });
    if !notes.foot.is_empty() || !notes.end.is_empty() {
        b.blank();
        b.rule(0, 20.min(width), '─', Style::new().fg(colors.color(0x808080)));
        for (mark, paras) in notes.foot.iter().chain(notes.end.iter()) {
            for (k, p) in paras.iter().enumerate() {
                let tag = if k == 0 { format!("[{mark}] ") } else { " ".repeat(mark.len() + 3) };
                let tw = tag.width() as u16;
                let mut lines = b.paragraph(usize::MAX, p, tw, width.saturating_sub(tw), false);
                if let Some(first) = lines.first_mut() {
                    first.segs.insert(0, Seg { x: 0, text: tag, style: Style::new().fg(colors.color(ACCENT)), src: None });
                }
                b.push_lines(None, lines);
            }
        }
    }
    if b.out.lines.is_empty() {
        b.blank();
    }
    b.out
}

impl Layout {
    /// Screen line and column of a document position.
    pub fn locate(&self, para: usize, off: usize) -> Option<(usize, u16)> {
        let &(a, z) = self.para_lines.get(para)?;
        if a == usize::MAX {
            return None;
        }
        let mut at_end: Option<(usize, u16)> = None;
        for ln in a..=z {
            for s in &self.lines[ln].segs {
                let Some(src) = s.src else { continue };
                if src.para != para {
                    continue;
                }
                if src.start <= off && off < src.end {
                    if src.atomic {
                        return Some((ln, s.x));
                    }
                    let w = s.text[..off - src.start].width() as u16;
                    return Some((ln, s.x + w));
                }
                if off == src.end {
                    at_end = Some((ln, s.x + s.width()));
                }
                if src.start == src.end && off == src.start {
                    at_end = Some((ln, s.x));
                }
            }
        }
        at_end
    }

    /// The document position nearest to column `x` of line `ln`.
    pub fn hit(&self, ln: usize, x: u16) -> Option<(usize, usize)> {
        let line = self.lines.get(ln)?;
        let mapped: Vec<&Seg> = line.segs.iter().filter(|s| s.src.is_some()).collect();
        let first = mapped.first()?;
        let mut best = (first.src?.para, first.src?.start);
        // Past the end of the line: its end (before a line break).
        for s in &mapped {
            let src = s.src?;
            let w = s.width();
            if x < s.x {
                return Some(best);
            }
            if src.atomic {
                if x < s.x + w.div_ceil(2) || (w == 0 && x == s.x) {
                    return Some((src.para, src.start));
                }
                best = (src.para, if w == 0 { src.start } else { src.end });
                continue;
            }
            let mut cx = s.x;
            for (k, ch) in s.text.char_indices() {
                let cw = ch.width().unwrap_or(0) as u16;
                if x < cx + cw.max(1) {
                    return Some((src.para, src.start + k));
                }
                cx += cw;
            }
            best = (src.para, src.end);
        }
        // A line that wrapped ends before its trailing space.
        if let Some(last) = mapped.last()
            && let Some(src) = last.src
            && !src.atomic
            && last.text.ends_with(' ')
            && self.lines.get(ln + 1).is_some_and(|l| l.segs.iter().any(|s| s.src.is_some_and(|q| q.para == src.para)))
        {
            return Some((src.para, src.end - 1));
        }
        Some(best)
    }
}
