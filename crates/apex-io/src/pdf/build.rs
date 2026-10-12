//! Builds an editable document from what the pages draw: paragraphs with
//! their fonts, alignment, indents and spacing; headings by size and
//! weight; lists; tables; pictures; and the headers and footers that
//! repeat on every page.

use std::collections::HashMap;
use std::sync::Arc;

use apex_doc::{Align, Attrs, Document, Field, FieldKind, HfSlot, Image, LineRule, OBJECT, ParaKind, Paragraph, Table};

use super::content::{Char, Page, PageImage};
use super::font::Style;
use super::layout::{self, Item, Line, Seg, TableBlock};

/// A paragraph before it becomes one of the document.
struct Draft {
    lines: Vec<Line>,
    frame: (f32, f32),
    list: Option<ListMark>,
    align: Align,
    indent_left: f32,
    first_line: f32,
    space_before: f32,
    line_spacing: f32,
    page: usize,
    size: f32,
    bold: bool,
    heading: Option<u8>,
    title: bool,
}

#[derive(Clone, Copy)]
struct ListMark {
    numbered: bool,
    format: u8,
    number: u32,
    /// Left edge of the marker, from the frame.
    offset: f32,
    level: u8,
}

enum Piece {
    Para(Draft),
    Table(TableBlock, usize),
    Image(PageImage, (f32, f32)),
}

/// What the pages need besides their characters.
struct Ctx<'a> {
    pages: &'a [Page],
    /// Horizontal rules per page that underline or strike text.
    marks: Vec<Vec<layout::HRule>>,
}

impl Ctx<'_> {
    fn attrs(&self, page: usize, c: &Char, line: &Line) -> Attrs {
        let p = &self.pages[page];
        let st = p.fonts.get(c.font as usize).cloned().unwrap_or_default();
        let small = c.size < line.size * 0.85;
        let superscript = small && c.y < line.y - 0.2 * line.size;
        let subscript = small && c.y > line.y + 0.08 * line.size;
        let size = if superscript || subscript { line.size } else { c.size };
        let mut a = Attrs { bold: st.bold, italic: st.italic, superscript, subscript, size: Some(((size * 2.0).round() as u16).clamp(2, 3276)), font: Some(st.family.clone()), color: (c.color != 0).then_some(c.color), ..Default::default() };
        let cx = c.x + c.w / 2.0;
        for r in &self.marks[page] {
            if cx < r.x0 || cx > r.x1 {
                continue;
            }
            let d = r.y - c.y;
            if d >= -0.5 && d <= 0.3 * c.size {
                a.underline = true;
            } else if d < -0.15 * c.size && d > -0.45 * c.size {
                a.strike = true;
            }
        }
        for (rect, uri) in &p.links {
            if cx >= rect[0] && cx <= rect[2] && c.y - c.size * 0.3 >= rect[1] - 1.0 && c.y - c.size * 0.3 <= rect[3] + 1.0 {
                a.link = Some(uri.clone());
            }
        }
        a
    }
}

fn percentile(mut v: Vec<f32>, p: f32) -> Option<f32> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f32::total_cmp);
    Some(v[((v.len() - 1) as f32 * p).round() as usize])
}

/// Text with digits as `#` and spaces collapsed (to spot what repeats).
fn shape_of(t: &str) -> String {
    let mut out = String::new();
    let mut digit = false;
    for c in t.split_whitespace().collect::<Vec<_>>().join(" ").chars() {
        if c.is_ascii_digit() {
            if !digit {
                out.push('#');
            }
            digit = true;
        } else {
            digit = false;
            out.push(c);
        }
    }
    out
}

pub fn document(pages: &[Page]) -> Document {
    let n = pages.len();
    // Characters, rules and ruled tables per page.
    let mut segs_of: Vec<Vec<Seg>> = Vec::with_capacity(n);
    let mut grids_of: Vec<Vec<TableBlock>> = Vec::with_capacity(n);
    let mut marks = Vec::with_capacity(n);
    for p in pages {
        let (h, v) = layout::rules(&p.shapes);
        let mut chars = p.chars.clone();
        let grids = layout::grids(&h, &v, &mut chars, p.width, p.height);
        // Short rules under or through text underline or strike it.
        marks.push(h.iter().filter(|r| r.x1 - r.x0 < p.width * 0.9).copied().take(4000).collect());
        segs_of.push(layout::segments(chars, p.width, p.height));
        grids_of.push(grids);
    }
    let ctx = Ctx { pages, marks };
    // Headers and footers: lines near the top or bottom edge that repeat.
    let mut header: Option<(Line, usize)> = None;
    let mut footer: Option<(Line, usize)> = None;
    if n >= 2 {
        let mut count: HashMap<(bool, String), usize> = HashMap::new();
        let zone_lines = |i: usize, segs: &[Seg]| -> Vec<(bool, Line)> {
            let h = pages[i].height;
            let zone: Vec<Seg> = segs.iter().filter(|s| s.bottom() < h * 0.1 || s.top() > h * 0.9).cloned().collect();
            layout::lines(zone).into_iter().map(|l| (l.y < h / 2.0, l)).collect()
        };
        for (i, segs) in segs_of.iter().enumerate() {
            let mut seen = std::collections::HashSet::new();
            for (top, l) in zone_lines(i, segs) {
                let key = (top, shape_of(&l.text()));
                if seen.insert(key.clone()) {
                    *count.entry(key).or_default() += 1;
                }
            }
        }
        let repeated = |k: &(bool, String)| count.get(k).is_some_and(|&c| c >= 2 && c * 2 >= n);
        for i in 0..n {
            let lines = zone_lines(i, &segs_of[i]);
            let mut drop: Vec<(f32, f32)> = Vec::new();
            for (top, l) in lines {
                if !repeated(&(top, shape_of(&l.text()))) {
                    continue;
                }
                drop.push((l.y, l.size));
                let slot = if top { &mut header } else { &mut footer };
                match slot {
                    None => *slot = Some((l, i)),
                    // More than one repeated line: the one nearest the edge.
                    Some((old, oi)) if *oi == i => {
                        let closer = if top { l.y < old.y } else { l.y > old.y };
                        if closer {
                            *slot = Some((l, i));
                        }
                    }
                    _ => {}
                }
            }
            let h = pages[i].height;
            segs_of[i].retain(|s| !((s.bottom() < h * 0.1 || s.top() > h * 0.9) && drop.iter().any(|(y, size)| (s.y - y).abs() <= 0.4 * size)));
        }
    }
    // The body's size and frame.
    let mut sizes: HashMap<u32, usize> = HashMap::new();
    let mut starts = Vec::new();
    let mut ends = Vec::new();
    for segs in &segs_of {
        for s in segs {
            *sizes.entry((s.size * 2.0).round() as u32).or_default() += s.chars.len();
        }
        for l in layout::lines(segs.clone()) {
            starts.push(l.x0);
            ends.push(l.x1);
        }
    }
    let body = sizes.iter().max_by_key(|(k, v)| (**v, u32::MAX - **k)).map_or(11.0, |(k, _)| *k as f32 / 2.0);
    let pw = pages.first().map_or(612.0, |p| p.width);
    let frame = (percentile(starts, 0.05).unwrap_or(72.0), percentile(ends, 0.95).unwrap_or(pw - 72.0));
    let frame = if frame.1 - frame.0 < 50.0 { (72.0, pw - 72.0) } else { frame };
    // Reading order and paragraphs, page by page.
    let mut pieces: Vec<Piece> = Vec::new();
    let mut tops = Vec::new();
    let mut bottoms = Vec::new();
    for (i, (segs, grids)) in segs_of.into_iter().zip(grids_of).enumerate() {
        let p = &pages[i];
        let has_text = !segs.is_empty() || !grids.is_empty();
        let mut lines = layout::lines(segs);
        let aligned = layout::aligned_tables(&mut lines);
        let mut items: Vec<Item> = lines.into_iter().flat_map(|l| l.segs.into_iter().map(Item::Seg)).collect();
        items.extend(grids.into_iter().chain(aligned).map(Item::Table));
        for im in &p.images {
            let full = im.w * im.h > 0.75 * p.width * p.height;
            if im.w >= 6.0 && im.h >= 6.0 && !(full && has_text) {
                items.push(Item::Image(im.clone()));
            }
        }
        for it in &items {
            let b = it.bbox();
            tops.push(b[1]);
            bottoms.push(p.height - b[3]);
        }
        let blocks = layout::order_page(items, body, frame);
        let first = pieces.len();
        for b in blocks {
            let mut run: Vec<Seg> = Vec::new();
            for it in b.items {
                match it {
                    Item::Seg(s) => run.push(s),
                    other => {
                        flush(&mut run, b.frame, i, &p.fonts, &mut pieces);
                        match other {
                            Item::Table(t) => pieces.push(Piece::Table(t, i)),
                            Item::Image(im) => pieces.push(Piece::Image(im, b.frame)),
                            Item::Seg(_) => {}
                        }
                    }
                }
            }
            flush(&mut run, b.frame, i, &p.fonts, &mut pieces);
        }
        // A paragraph going on from the previous page joins it.
        if first > 0 && first < pieces.len() {
            let (before, after) = pieces.split_at_mut(first);
            if let (Some(Piece::Para(prev)), Piece::Para(next)) = (before.last_mut(), &after[0])
                && continues(prev, next)
            {
                let Piece::Para(next) = pieces.remove(first) else { unreachable!() };
                if let Some(Piece::Para(prev)) = pieces.get_mut(first - 1) {
                    prev.lines.extend(next.lines);
                }
            }
        }
    }
    headings(&mut pieces, body);
    list_levels(&mut pieces);
    // The document.
    let mut paras: Vec<Paragraph> = Vec::new();
    let mut table_id = 0u32;
    let mut numbers: HashMap<u8, u32> = HashMap::new();
    for piece in pieces {
        match piece {
            Piece::Para(d) => {
                if d.list.is_none() {
                    numbers.clear();
                }
                paras.push(paragraph(&ctx, &d, &mut numbers, true));
            }
            Piece::Table(t, page) => {
                numbers.clear();
                table_id += 1;
                paras.extend(table(&ctx, t, page).into_paras(table_id));
            }
            Piece::Image(im, fr) => {
                numbers.clear();
                paras.push(picture(im, fr, frame));
            }
        }
    }
    let mut doc = Document::from_paragraphs(paras);
    if let Some(p) = pages.first() {
        doc.page.width = p.width;
        doc.page.height = p.height;
        doc.page.margin_left = frame.0.clamp(18.0, 144.0);
        doc.page.margin_right = (p.width - frame.1).clamp(18.0, 144.0);
        doc.page.margin_top = percentile(tops, 0.05).unwrap_or(72.0).clamp(18.0, 144.0);
        doc.page.margin_bottom = percentile(bottoms, 0.05).unwrap_or(72.0).clamp(18.0, 144.0);
    }
    let total = n;
    if let Some((l, i)) = header {
        doc.page.margin_header = (l.y - l.size).clamp(12.0, (doc.page.margin_top - 6.0).max(12.0));
        doc.headers.set(HfSlot::Header, Some(Arc::new(vec![edge_paragraph(&ctx, &l, i, total)])));
    }
    if let Some((l, i)) = footer {
        let h = pages[i].height;
        doc.page.margin_footer = (h - l.y - l.size * layout::DESC).clamp(12.0, (doc.page.margin_bottom - 6.0).max(12.0));
        doc.headers.set(HfSlot::Footer, Some(Arc::new(vec![edge_paragraph(&ctx, &l, i, total)])));
    }
    doc.modified = false;
    doc
}

fn flush(run: &mut Vec<Seg>, frame: (f32, f32), page: usize, fonts: &[Style], out: &mut Vec<Piece>) {
    if run.is_empty() {
        return;
    }
    let lines = layout::lines(std::mem::take(run));
    let prev_bottom = match out.last() {
        Some(Piece::Para(d)) if d.page == page => d.lines.last().map(|l| l.y),
        _ => None,
    };
    let mut drafts = paragraphs(lines, frame, page, fonts);
    if let (Some(first), Some(py)) = (drafts.first_mut(), prev_bottom)
        && let Some(l) = first.lines.first()
    {
        first.space_before = ((l.y - py) - 1.15 * l.size).clamp(0.0, 48.0).round();
    }
    out.extend(drafts.into_iter().map(Piece::Para));
}

fn first_word_width(l: &Line) -> f32 {
    let s = &l.segs[0];
    let end = s.chars.iter().position(|c| c.ch == ' ').unwrap_or(s.chars.len());
    s.chars[..end].iter().map(|c| c.w).sum()
}

fn bold_line(ctx_fonts: &[Style], l: &Line) -> bool {
    let chars: Vec<&Char> = l.segs.iter().flat_map(|s| s.chars.iter()).filter(|c| !c.ch.is_whitespace()).collect();
    !chars.is_empty() && chars.iter().filter(|c| ctx_fonts.get(c.font as usize).is_some_and(|f| f.bold)).count() * 10 >= chars.len() * 9
}

/// The list marker a line starts with.
fn marker_of(l: &Line) -> Option<(ListMark, usize)> {
    let s = &l.segs[0];
    let text: String = s.chars.iter().map(|c| c.ch).collect();
    let first = text.chars().next()?;
    let (numbered, format, number, len) = if layout::is_bullet(first) && text[first.len_utf8()..].starts_with([' ', '\t']) {
        let format = match first {
            '◦' | '○' => 2,
            '■' | '▪' | '□' | '▫' => 3,
            '–' | '-' | '⁃' => 4,
            '✓' | '✔' => 5,
            '➢' | '➤' | '►' | '‣' => 6,
            _ => 0,
        };
        (false, format, 0, first.len_utf8())
    } else {
        let (n, f, len) = layout::list_number(&text)?;
        if !text[len..].starts_with([' ', '\t']) {
            return None;
        }
        (true, f, n, len)
    };
    let chars = text[..len].chars().count();
    Some((ListMark { numbered, format, number, offset: s.x0, level: 0 }, chars))
}

fn paragraphs(lines: Vec<Line>, frame: (f32, f32), page: usize, fonts: &[Style]) -> Vec<Draft> {
    let mut diffs: Vec<f32> = lines.windows(2).map(|w| w[1].y - w[0].y).filter(|d| *d > 0.0).collect();
    diffs.retain(|d| *d < 3.0 * lines.first().map_or(12.0, |l| l.size));
    let spacing = percentile(diffs, 0.5);
    let mut out: Vec<Draft> = Vec::new();
    let mut text_x = 0.0;
    for l in lines {
        let mark = marker_of(&l);
        let start_new = match out.last() {
            None => true,
            Some(d) => {
                let prev = d.lines.last().expect("a line");
                let gap = l.y - prev.y;
                let spacing = spacing.unwrap_or(1.2 * l.size);
                let prev_text = prev.text();
                let hyphen = prev_text.ends_with('-');
                mark.is_some()
                    || gap > spacing * 1.3 + 1.0
                    || gap <= 0.0
                    || (l.size - prev.size).abs() > 0.8
                    || bold_line(fonts, &l) != bold_line(fonts, prev)
                    || (!hyphen && prev.x1 + 0.3 * l.size + first_word_width(&l) < frame.1 - 2.0 && prev.x1 < frame.1 - 2.0 * l.size)
                    || match d.list {
                        Some(_) => (l.x0 - text_x).abs() > 4.0,
                        None => l.x0 > prev.x0 + 0.8 * l.size && d.lines.len() >= 1 && l.x0 > d.lines[0].x0.min(prev.x0) + 0.8 * l.size,
                    }
            }
        };
        if start_new {
            if let Some((m, n)) = mark {
                text_x = l.segs[0].chars.get(n..).and_then(|cs| cs.iter().find(|c| c.ch != ' ' && c.ch != '\t')).map_or(l.x0, |c| c.x);
                let mut l = l;
                // The marker goes; the list shows its own.
                let s = &mut l.segs[0];
                s.chars.drain(..n);
                while s.chars.first().is_some_and(|c| c.ch == ' ' || c.ch == '\t') {
                    s.chars.remove(0);
                }
                if s.chars.is_empty() {
                    l.segs.remove(0);
                }
                if l.segs.is_empty() {
                    continue;
                }
                out.push(draft(vec![l], frame, page, Some(m)));
            } else {
                out.push(draft(vec![l], frame, page, None));
            }
        } else if let Some(d) = out.last_mut() {
            d.lines.push(l);
        }
    }
    for d in &mut out {
        finish(d, spacing);
        d.bold = d.lines.iter().all(|l| bold_line(fonts, l));
    }
    out
}

fn draft(lines: Vec<Line>, frame: (f32, f32), page: usize, list: Option<ListMark>) -> Draft {
    Draft { lines, frame, list, align: Align::Left, indent_left: 0.0, first_line: 0.0, space_before: 0.0, line_spacing: 0.0, page, size: 0.0, bold: false, heading: None, title: false }
}

/// Alignment, indents, line spacing and the main size of a paragraph.
fn finish(d: &mut Draft, spacing: Option<f32>) {
    let (f0, f1) = d.frame;
    let mut sizes: HashMap<u32, usize> = HashMap::new();
    for c in d.lines.iter().flat_map(|l| l.segs.iter().flat_map(|s| s.chars.iter())) {
        *sizes.entry((c.size * 2.0).round() as u32).or_default() += 1;
    }
    d.size = sizes.into_iter().max_by_key(|(k, v)| (*v, *k)).map_or(11.0, |(k, _)| k as f32 / 2.0);
    let n = d.lines.len();
    let tol = 3.0;
    let mid = (f0 + f1) / 2.0;
    let centered = d.lines.iter().all(|l| ((l.x0 + l.x1) / 2.0 - mid).abs() < 4.0) && d.lines.iter().all(|l| l.x0 - f0 > 12.0);
    let right = d.lines.iter().all(|l| (l.x1 - f1).abs() < tol) && d.lines.iter().all(|l| l.x0 - f0 > 24.0);
    let full = n >= 2 && d.lines[..n - 1].iter().all(|l| (l.x1 - f1).abs() < tol);
    d.align = if d.list.is_none() && centered {
        Align::Center
    } else if d.list.is_none() && right {
        Align::Right
    } else if full && d.lines[n - 1].x1 < f1 - tol {
        Align::Justify
    } else {
        Align::Left
    };
    if d.list.is_none() && matches!(d.align, Align::Left | Align::Justify) {
        let left = if n > 1 { d.lines[1..].iter().map(|l| l.x0).fold(f32::MAX, f32::min) } else { d.lines[0].x0 };
        let indent = left - f0;
        if indent > 4.0 {
            d.indent_left = indent.round();
        }
        let first = d.lines[0].x0 - left;
        if first.abs() > 3.0 {
            d.first_line = first.round();
        }
    }
    if n >= 2
        && let Some(sp) = spacing
    {
        let ratio = sp / (1.17 * d.size);
        if ratio > 1.2 {
            d.line_spacing = ((ratio * 20.0).round() / 20.0).clamp(1.0, 3.0);
        }
    }
}

fn is_terminal(t: &str) -> bool {
    t.trim_end().ends_with(['.', '!', '?', ':', ';', '"', '”', ')', '»', '。', '！', '？'])
}

fn continues(prev: &Draft, next: &Draft) -> bool {
    let (Some(pl), Some(nl)) = (prev.lines.last(), next.lines.first()) else { return false };
    prev.list.is_none() && next.list.is_none() && (pl.size - nl.size).abs() < 0.5 && !is_terminal(&pl.text()) && nl.text().chars().next().is_some_and(char::is_lowercase)
}

/// Headings by size (and short bold lines at the body's size).
fn headings(pieces: &mut [Piece], body: f32) {
    let text_len = |d: &Draft| d.lines.iter().map(|l| l.segs.iter().map(|s| s.chars.len()).sum::<usize>()).sum::<usize>();
    let mut cands: Vec<(usize, f32, bool)> = Vec::new();
    for (i, p) in pieces.iter().enumerate() {
        let Piece::Para(d) = p else { continue };
        if d.list.is_some() || d.lines.len() > 3 || text_len(d) > 160 {
            continue;
        }
        let t = d.lines.iter().map(Line::text).collect::<Vec<_>>().join(" ");
        if t.trim().ends_with([',', ';']) || !t.chars().any(char::is_alphabetic) {
            continue;
        }
        if d.size >= body * 1.12 {
            cands.push((i, d.size, false));
        } else if d.bold && d.size >= body * 0.95 && d.lines.len() == 1 && text_len(d) <= 100 && !t.trim().ends_with('.') {
            cands.push((i, d.size, true));
        }
    }
    let mut levels: Vec<f32> = cands.iter().filter(|c| !c.2).map(|c| (c.1 * 2.0).round() / 2.0).collect();
    levels.sort_by(|a, b| b.total_cmp(a));
    levels.dedup();
    let biggest = levels.first().copied();
    let biggest_count = cands.iter().filter(|c| !c.2 && Some((c.1 * 2.0).round() / 2.0) == biggest).count();
    for (i, size, bold_only) in cands {
        let Piece::Para(d) = &mut pieces[i] else { continue };
        let level = if bold_only { levels.len() + 1 } else { levels.iter().position(|l| (l - (size * 2.0).round() / 2.0).abs() < 0.01).unwrap_or(0) + 1 };
        d.heading = Some(level.clamp(1, 6) as u8);
        if !bold_only && level == 1 && biggest_count == 1 && d.page == 0 && size >= body * 1.6 && i < 4 {
            d.title = true;
        }
    }
}

/// List levels by how far each marker is indented.
fn list_levels(pieces: &mut [Piece]) {
    let offs: Vec<f32> = pieces.iter().filter_map(|p| if let Piece::Para(d) = p { d.list.map(|m| m.offset - d.frame.0) } else { None }).collect();
    let Some(min) = offs.iter().copied().reduce(f32::min) else { return };
    for p in pieces {
        if let Piece::Para(d) = p
            && let Some(m) = &mut d.list
        {
            m.level = (((m.offset - d.frame.0 - min) / 18.0).round().max(0.0) as u8).min(8);
        }
    }
}

fn join_lines(ctx: &Ctx, lines: &[Line], page: usize, p: &mut Paragraph) {
    for (k, l) in lines.iter().enumerate() {
        if k > 0 {
            let prev: String = p.text.chars().rev().take(2).collect();
            let mut pc = prev.chars();
            let (last, before) = (pc.next(), pc.next());
            let next = l.segs.first().and_then(|s| s.chars.first()).map(|c| c.ch);
            let hyphen = last == Some('-') && before.is_some_and(char::is_alphabetic) && next.is_some_and(char::is_lowercase);
            if hyphen {
                // A word split at the line end is joined again.
                let cut = p.text.len() - 1;
                p.text.truncate(cut);
                if let Some(r) = p.runs.last_mut() {
                    r.len -= 1;
                }
                p.runs.retain(|r| r.len > 0);
            } else if !(last.is_some_and(|c| c as u32 >= 0x2E80) || next.is_some_and(|c| c as u32 >= 0x2E80)) && last != Some(' ') {
                let a = p.runs.last().map(|r| r.attrs.clone()).unwrap_or_default();
                p.push(" ", Attrs { superscript: false, subscript: false, ..a });
            }
        }
        for (j, s) in l.segs.iter().enumerate() {
            if j > 0 {
                let a = p.runs.last().map(|r| r.attrs.clone()).unwrap_or_default();
                p.push("\t", a);
            }
            let mut buf = [0u8; 4];
            for c in &s.chars {
                p.push(c.ch.encode_utf8(&mut buf), ctx.attrs(page, c, l));
            }
        }
    }
    if p.runs.is_empty() {
        p.runs.push(apex_doc::Run { len: p.text.len(), attrs: Attrs::default() });
    }
}

fn paragraph(ctx: &Ctx, d: &Draft, numbers: &mut HashMap<u8, u32>, spacing: bool) -> Paragraph {
    let mut p = Paragraph { text: String::new(), runs: Vec::new(), ..Paragraph::new("") };
    join_lines(ctx, &d.lines, d.page, &mut p);
    let st = &mut p.style;
    st.align = d.align;
    st.indent_left = d.indent_left;
    st.first_line = d.first_line;
    if spacing {
        st.space_before = Some(d.space_before);
        st.space_after = Some(0.0);
    }
    if d.line_spacing > 0.0 {
        st.line_spacing = d.line_spacing;
        st.line_rule = LineRule::Multiple;
    }
    if d.title {
        st.kind = ParaKind::Title;
    } else if let Some(h) = d.heading {
        st.kind = ParaKind::Heading(h);
        st.keep_with_next = true;
    }
    if st.kind != ParaKind::Normal {
        // Headings keep the colour they had (not the style's).
        for r in &mut p.runs {
            r.attrs.color.get_or_insert(0);
        }
    }
    if let Some(m) = d.list {
        st.kind = if m.numbered { ParaKind::Numbered } else { ParaKind::Bullet };
        st.level = m.level;
        st.list_format = m.format;
        st.indent_left = 0.0;
        st.first_line = 0.0;
        if m.numbered {
            let expected = numbers.get(&m.level).map_or(1, |n| n + 1);
            if m.number == 1 {
                st.restart = true;
            } else if m.number != expected {
                st.start_at = m.number.min(u16::MAX as u32) as u16;
            }
            numbers.insert(m.level, m.number);
            numbers.retain(|&lv, _| lv <= m.level);
        }
    }
    p
}

fn table(ctx: &Ctx, t: TableBlock, page: usize) -> Table {
    let rows = t.cells.len();
    let cols = t.xs.len() - 1;
    // A little wider than drawn: the substitute fonts and the cells'
    // padding need room, or tight columns wrap every word.
    let room = (ctx.pages[page].width * 0.8 - (t.xs[cols] - t.xs[0])).clamp(0.0, 8.0 * cols as f32) / cols as f32;
    let widths: Vec<f32> = t.xs.windows(2).map(|w| (w[1] - w[0]).max(12.0) + room).collect();
    let mut table = Table::new(rows, cols, widths.iter().sum());
    table.props.widths = widths;
    // The drawn columns already hold the PDF's own padding.
    table.props.cell_margins = Some([1.0, 2.5, 1.0, 2.5]);
    if !t.ruled {
        table.props.style = None;
    }
    let mut all_bold_first = true;
    for (r, row) in t.cells.into_iter().enumerate() {
        for (c, cell) in row.into_iter().enumerate() {
            if r >= table.rows.len() || c >= cols {
                continue;
            }
            let target = &mut table.rows[r][c];
            if cell.col_span == 0 {
                target.props.covered = true;
                continue;
            }
            target.props.col_span = cell.col_span.max(1);
            target.props.row_span = cell.row_span.max(1);
            let right_edge = t.xs[(c + cell.col_span.max(1) as usize).min(cols)];
            let frame = (t.xs[c] + 2.0, right_edge - 2.0);
            if r == 0 && !cell.lines.is_empty() {
                all_bold_first &= cell.lines.iter().all(|l| bold_line(&ctx.pages[page].fonts, l));
            }
            let drafts = paragraphs(cell.lines, frame, page, &ctx.pages[page].fonts);
            if drafts.is_empty() {
                continue;
            }
            let mut numbers = HashMap::new();
            target.paras = drafts
                .iter()
                .map(|d| {
                    let mut p = paragraph(ctx, d, &mut numbers, false);
                    p.style.indent_left = 0.0;
                    p.style.first_line = 0.0;
                    p.style.space_before = Some(0.0);
                    p.style.space_after = Some(0.0);
                    if t.right.get(c) == Some(&true) {
                        p.style.align = Align::Right;
                    } else if p.style.align == Align::Justify {
                        p.style.align = Align::Left;
                    }
                    p
                })
                .collect();
        }
    }
    if all_bold_first && rows > 1 {
        table.props.header_rows = 1;
    }
    table
}

fn picture(im: PageImage, fr: (f32, f32), page_frame: (f32, f32)) -> Paragraph {
    let avail = (page_frame.1 - page_frame.0).max(72.0);
    let scale = if im.w > avail { avail / im.w } else { 1.0 };
    let mut img = Image::new(im.data.clone(), im.w * scale, im.h * scale);
    img.name = "Picture".into();
    let mut p = Paragraph::new("");
    p.push(&OBJECT.to_string(), Attrs { image: Some(Arc::new(img)), ..Default::default() });
    let mid = (fr.0 + fr.1) / 2.0;
    if ((im.x + im.w / 2.0) - mid).abs() < 12.0 && im.x - fr.0 > 12.0 {
        p.style.align = Align::Center;
    } else if (im.x + im.w - fr.1).abs() < 6.0 && im.x - fr.0 > 24.0 {
        p.style.align = Align::Right;
    }
    p.style.space_after = Some(6.0);
    p
}

/// A header or footer line, its page number as a field.
fn edge_paragraph(ctx: &Ctx, l: &Line, page: usize, total: usize) -> Paragraph {
    let mut p = Paragraph { text: String::new(), runs: Vec::new(), ..Paragraph::new("") };
    join_lines(ctx, std::slice::from_ref(l), page, &mut p);
    // Digits that are the page number (or the page count) become fields.
    let text = p.text.clone();
    let this = (page + 1).to_string();
    let count = total.to_string();
    let mut out = Paragraph { text: String::new(), runs: Vec::new(), ..Paragraph::new("") };
    let mut i = 0;
    let bytes = text.as_bytes();
    let mut used_page = false;
    while i < text.len() {
        let attrs = p.spans().find(|(s, e, _)| *s <= i && i < *e).map(|(_, _, a)| a.clone()).unwrap_or_default();
        if bytes[i].is_ascii_digit() {
            let end = i + text[i..].bytes().take_while(u8::is_ascii_digit).count();
            let digits = &text[i..end];
            let kind = if digits == this && !used_page {
                used_page = true;
                Some(FieldKind::Page)
            } else if digits == count && used_page {
                Some(FieldKind::NumPages)
            } else {
                None
            };
            match kind {
                Some(k) => {
                    let mut f = Field::new(k);
                    f.cached = digits.to_string();
                    out.push(&OBJECT.to_string(), Attrs { field: Some(Arc::new(f)), ..attrs });
                }
                None => out.push(digits, attrs),
            }
            i = end;
        } else {
            let ch = text[i..].chars().next().expect("a char");
            out.push(ch.encode_utf8(&mut [0u8; 4]), attrs);
            i += ch.len_utf8();
        }
    }
    if out.runs.is_empty() {
        out.runs.push(apex_doc::Run { len: 0, attrs: Attrs::default() });
    }
    let w = ctx.pages[page].width;
    let mid = (l.x0 + l.x1) / 2.0;
    out.style.align = if l.segs.len() == 1 && (mid - w / 2.0).abs() < 20.0 {
        Align::Center
    } else if l.segs.len() == 1 && l.x0 > w / 2.0 {
        Align::Right
    } else {
        Align::Left
    };
    out.style.space_before = Some(0.0);
    out.style.space_after = Some(0.0);
    out
}

/// The tables of the pages as text, for Get Data.
pub fn tables(pages: &[Page]) -> Vec<(usize, Vec<Vec<String>>)> {
    let mut out = Vec::new();
    for (i, p) in pages.iter().enumerate() {
        let (h, v) = layout::rules(&p.shapes);
        let mut chars = p.chars.clone();
        let mut found: Vec<TableBlock> = layout::grids(&h, &v, &mut chars, p.width, p.height);
        let mut lines = layout::lines(layout::segments(chars, p.width, p.height));
        found.extend(layout::aligned_tables(&mut lines));
        found.sort_by(|a, b| a.ys[0].total_cmp(&b.ys[0]));
        for t in found {
            let rows = t.texts();
            if rows.iter().any(|r| r.iter().any(|c| !c.trim().is_empty())) {
                out.push((i, rows));
            }
        }
    }
    out
}
