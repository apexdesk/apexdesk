//! East Asian layout drawn over the text: phonetic guides (ruby) over or
//! under their base text, emphasis marks, two lines in one (combined
//! characters), text fitted to a width and, in vertical text, horizontal
//! digits in one character's cell (tate-chu-yoko).
//!
//! Before the layout the base text is spaced out (a guide wider than its
//! base, fitted text) or hidden in a cell as wide as its two lines;
//! after it the rows with guides or marks grow to make room for them and
//! the guides, marks and lines are laid out as galleys of their own
//! (so they reach PDFs too).

use std::f32::consts::FRAC_PI_2;
use std::ops::Range;
use std::sync::Arc;

use apex_doc::{AsianAttrs, Emphasis, Ruby};

use eframe::egui::epaint::text::{ByteIndex, EM_BOX_MIDDLE, Galley, LayoutJob, TextFormat};
use eframe::egui::{self, Color32, FontFamily, FontId, Pos2, pos2, vec2};

/// Text of a paragraph with East Asian layout (appended to the job at
/// `bytes`).
pub(super) struct Mark {
    pub bytes: Range<usize>,
    pub asian: Arc<AsianAttrs>,
    /// Font size (pixels), family and color of the text.
    pub size: f32,
    pub family: FontFamily,
    pub color: Color32,
}

/// Whether text with these settings needs [`prepare`] and [`place`].
pub(super) fn wanted(a: &AsianAttrs) -> bool {
    a.ruby.is_some() || a.emphasis.is_some() || a.fit_text.is_some() || a.combine.as_ref().is_some_and(|c| c.lines || c.letters || c.vert)
}

/// A galley drawn over the paragraph's text.
#[derive(Clone)]
pub(super) struct Overlay {
    /// Top left corner, relative to the paragraph's galley.
    pub pos: Pos2,
    /// Top of its row: it's drawn on the page that shows the row.
    pub row_top: f32,
    pub galley: Arc<Galley>,
    /// Turned about `pos` (radians, clockwise): horizontal text in
    /// vertical text is drawn turned back.
    pub angle: f32,
}

enum Kind {
    Ruby(Arc<Ruby>),
    Combine { lines: [String; 2], brackets: Option<(char, char)> },
    Fit(f32),
    Emphasis(Emphasis),
    /// Horizontal in vertical text, compressed to the line's width.
    Upright { text: String, compress: bool },
}

/// A run of marked text laid out together.
struct Group {
    bytes: Range<usize>,
    kind: Kind,
    size: f32,
    family: FontFamily,
    color: Color32,
}

/// What [`prepare`] did, for [`place`].
pub(super) struct Prepared {
    groups: Vec<Group>,
    /// Pixels per point.
    pt: f32,
    /// The text is vertical.
    vertical: bool,
}

/// Brackets of two lines in one.
fn brackets(b: &str) -> (char, char) {
    match b {
        "round" => ('(', ')'),
        "square" => ('[', ']'),
        "angle" => ('<', '>'),
        "curly" => ('{', '}'),
        _ => {
            let mut c = b.chars();
            let open = c.next().unwrap_or('(');
            (open, c.next_back().unwrap_or(match open {
                '[' => ']',
                '<' => '>',
                '{' => '}',
                _ => ')',
            }))
        }
    }
}

/// The two lines of combined characters: the first half of the text over
/// the second.
pub(super) fn split_lines(text: &str) -> [String; 2] {
    let chars: Vec<char> = text.chars().filter(|c| !matches!(c, '\n' | '\r')).collect();
    let half = chars.len().div_ceil(2);
    [chars[..half].iter().collect(), chars[half..].iter().collect()]
}

fn groups(text: &str, marks: &[Mark]) -> Vec<Group> {
    let mut out: Vec<Group> = Vec::new();
    // Runs of consecutive marks with equal `key`.
    let runs = |key: &dyn Fn(&AsianAttrs) -> Option<String>| {
        let mut runs: Vec<(Range<usize>, usize)> = Vec::new();
        let mut last: Option<String> = None;
        for (i, m) in marks.iter().enumerate() {
            let k = key(&m.asian);
            match (runs.last_mut(), &k) {
                (Some(r), Some(_)) if k == last && r.0.end == m.bytes.start => r.0.end = m.bytes.end,
                (_, Some(_)) => runs.push((m.bytes.clone(), i)),
                _ => {}
            }
            last = k;
        }
        runs
    };
    let ruby_key = |a: &AsianAttrs| a.ruby.as_ref().map(|r| format!("{r:?}"));
    for (bytes, i) in runs(&ruby_key) {
        let m = &marks[i];
        let ruby = m.asian.ruby.clone().unwrap_or_default();
        out.push(Group { bytes, kind: Kind::Ruby(ruby), size: m.size, family: m.family.clone(), color: m.color });
    }
    let combine_key = |a: &AsianAttrs| a.combine.as_ref().filter(|c| c.lines || c.letters).map(|c| format!("{c:?}"));
    for (bytes, i) in runs(&combine_key) {
        let m = &marks[i];
        let c = m.asian.combine.clone().unwrap_or_default();
        let lines = split_lines(&text[bytes.clone()]);
        out.push(Group { bytes, kind: Kind::Combine { lines, brackets: c.brackets.as_deref().map(brackets) }, size: m.size, family: m.family.clone(), color: m.color });
    }
    let upright_key = |a: &AsianAttrs| a.combine.as_ref().filter(|c| c.vert && !c.lines && !c.letters).map(|c| format!("{c:?}"));
    for (bytes, i) in runs(&upright_key) {
        let m = &marks[i];
        let compress = m.asian.combine.as_ref().is_some_and(|c| c.vert_compress);
        let text = text[bytes.clone()].to_string();
        out.push(Group { bytes, kind: Kind::Upright { text, compress }, size: m.size, family: m.family.clone(), color: m.color });
    }
    let fit_key = |a: &AsianAttrs| a.fit_text.map(|f| format!("{f:?}"));
    for (bytes, i) in runs(&fit_key) {
        let m = &marks[i];
        let w = m.asian.fit_text.map_or(0, |f| f.0) as f32 / 20.0;
        out.push(Group { bytes, kind: Kind::Fit(w), size: m.size, family: m.family.clone(), color: m.color });
    }
    for m in marks {
        if let Some(e) = m.asian.emphasis {
            out.push(Group { bytes: m.bytes.clone(), kind: Kind::Emphasis(e), size: m.size, family: m.family.clone(), color: m.color });
        }
    }
    out
}

/// Splits the section of `job` that has `at` inside it in two.
fn split_at(job: &mut LayoutJob, at: usize) {
    let Some(i) = job.sections.iter().position(|s| s.byte_range.start.0 < at && at < s.byte_range.end.0) else { return };
    let mut second = job.sections[i].clone();
    second.byte_range.start = ByteIndex(at);
    second.leading_space = 0.0;
    job.sections[i].byte_range.end = ByteIndex(at);
    job.sections.insert(i + 1, second);
}

/// The sections of `job` inside `bytes`.
fn sections_in(job: &LayoutJob, bytes: &Range<usize>) -> Range<usize> {
    let from = job.sections.iter().position(|s| s.byte_range.start.0 >= bytes.start).unwrap_or(job.sections.len());
    let to = job.sections.iter().rposition(|s| s.byte_range.end.0 <= bytes.end).map_or(from, |i| i + 1).max(from);
    from..to
}

/// Width of the sections `secs` of `job` laid out on one line.
fn measure(ctx: &egui::Context, job: &LayoutJob, secs: Range<usize>) -> f32 {
    let Some(first) = job.sections.get(secs.start) else { return 0.0 };
    let base = first.byte_range.start.0;
    let end = job.sections[secs.end - 1].byte_range.end.0;
    let mut j = LayoutJob { text: job.text[base..end].to_string(), round_output_to_gui: false, ..Default::default() };
    for (k, s) in job.sections[secs].iter().enumerate() {
        let mut s = s.clone();
        s.byte_range = ByteIndex(s.byte_range.start.0 - base)..ByteIndex(s.byte_range.end.0 - base);
        if k == 0 {
            s.leading_space = 0.0;
        }
        j.sections.push(s);
    }
    ctx.fonts_mut(|f| f.layout_job(j)).rect.width()
}

/// Number of characters of `secs`.
fn char_len(job: &LayoutJob, secs: &Range<usize>) -> usize {
    job.sections[secs.clone()].iter().map(|s| job.text[s.byte_range.start.0..s.byte_range.end.0].chars().count()).sum()
}

/// Spaces the characters of `secs` out (or in) by `extra` in all: `lead`
/// of it before the first one, the rest between them.
fn stretch(job: &mut LayoutJob, secs: Range<usize>, extra: f32, lead: f32) {
    let n = char_len(job, &secs);
    if secs.is_empty() || extra.abs() < 0.01 {
        return;
    }
    let between = extra - lead;
    if n < 2 {
        job.sections[secs.start].leading_space += extra;
        return;
    }
    let s = between / (n - 1) as f32;
    // epaint spaces every character but the first of a paragraph.
    let start = job.sections[secs.start].byte_range.start.0;
    let spaced = start > 0 && !job.text[..start].ends_with('\n');
    job.sections[secs.start].leading_space += lead - if spaced { s } else { 0.0 };
    for i in secs {
        job.sections[i].format.extra_letter_spacing += s;
    }
}

/// One line of text in its own galley (`vertical`: vertical text, like the
/// text it goes with).
fn text_galley(ctx: &egui::Context, text: &str, size: f32, family: &FontFamily, color: Color32, spacing: f32, vertical: bool) -> Arc<Galley> {
    let fmt = TextFormat { font_id: FontId::new(size.max(1.0), family.clone()), color, extra_letter_spacing: spacing, ..Default::default() };
    let text = if vertical { apex_doc::to_vertical(text) } else { std::borrow::Cow::Borrowed(text) };
    let mut job = LayoutJob::single_section(text.into_owned(), fmt);
    job.round_output_to_gui = false;
    job.vertical = vertical;
    ctx.fonts_mut(|f| f.layout_job(job))
}

/// Size of a guide (pixels) over text of `size`.
fn ruby_size(r: &Ruby, size: f32, pt: f32) -> f32 {
    r.size.map_or(size / 2.0, |h| h as f32 / 2.0 * pt)
}

/// Makes room in `job` for the East Asian layout of `marks`: spaces out
/// fitted text and bases narrower than their guides, hides combined
/// characters in a cell as wide as their two lines, and keeps each base
/// on one line. `pt` is pixels per point.
pub(super) fn prepare(ctx: &egui::Context, job: &mut LayoutJob, marks: &[Mark], pt: f32) -> Prepared {
    let v = job.vertical;
    let mut groups = self::groups(&job.text, marks);
    // Horizontal text is horizontal anyway.
    groups.retain(|g| v || !matches!(g.kind, Kind::Upright { .. }));
    for g in &groups {
        split_at(job, g.bytes.start);
        split_at(job, g.bytes.end);
    }
    // Fitted text first: guides go over the text as it is shown.
    for g in &groups {
        if let Kind::Fit(w) = g.kind {
            let secs = sections_in(job, &g.bytes);
            let natural = measure(ctx, job, secs.clone());
            let target = w * pt;
            if target > 1.0 {
                stretch(job, secs, target - natural, 0.0);
            }
        }
    }
    for g in &groups {
        if let Kind::Combine { lines, brackets } = &g.kind {
            let secs = sections_in(job, &g.bytes);
            let natural = measure(ctx, job, secs.clone());
            let half = g.size / 2.0;
            let inner = lines.iter().map(|l| text_galley(ctx, l, half, &g.family, g.color, 0.0, v).rect.width()).fold(0.0, f32::max);
            let br = brackets.map_or(0.0, |(a, b)| text_galley(ctx, &format!("{a}{b}"), g.size, &g.family, g.color, 0.0, v).rect.width());
            for i in secs.clone() {
                job.sections[i].format.color = Color32::TRANSPARENT;
            }
            stretch(job, secs.clone(), inner + br - natural, 0.0);
            job.wrap.keep_together.push(secs.start as u32..secs.end as u32);
        }
    }
    // Horizontal in vertical text: hidden in a character's cell.
    for g in groups.iter().filter(|g| matches!(g.kind, Kind::Upright { .. })) {
        let secs = sections_in(job, &g.bytes);
        if secs.is_empty() {
            continue;
        }
        let natural = measure(ctx, job, secs.clone());
        for i in secs.clone() {
            job.sections[i].format.color = Color32::TRANSPARENT;
        }
        stretch(job, secs.clone(), g.size - natural, 0.0);
        job.wrap.keep_together.push(secs.start as u32..secs.end as u32);
    }
    for g in &groups {
        if let Kind::Ruby(r) = &g.kind {
            let secs = sections_in(job, &g.bytes);
            if secs.is_empty() {
                continue;
            }
            let base = measure(ctx, job, secs.clone());
            let guide = text_galley(ctx, &r.text, ruby_size(r, g.size, pt), &g.family, g.color, 0.0, v).rect.width();
            let pad = guide - base;
            if pad > 0.01 {
                let n = char_len(job, &secs) as f32;
                let (before, between, after) = match r.align.as_deref() {
                    Some("left") => (0.0, 0.0, pad),
                    Some("right") => (pad, 0.0, 0.0),
                    Some("distributeLetter") if n > 1.0 => (0.0, pad, 0.0),
                    Some("distributeSpace") => (pad / (2.0 * n), pad * (n - 1.0) / n, pad / (2.0 * n)),
                    _ => (pad / 2.0, 0.0, pad / 2.0),
                };
                stretch(job, secs.clone(), before + between, before);
                if let Some(next) = job.sections.get_mut(secs.end) {
                    next.leading_space += after;
                }
            }
            job.wrap.keep_together.push(secs.start as u32..secs.end as u32);
        }
    }
    Prepared { groups, pt, vertical: v }
}

/// Is `c` punctuation (it takes no emphasis mark)?
fn is_punct(c: char) -> bool {
    c.is_ascii_punctuation() || c.is_whitespace() || matches!(c, '\u{3000}'..='\u{303F}' | '\u{FF01}'..='\u{FF0F}' | '\u{FF1A}'..='\u{FF20}' | '\u{FF3B}'..='\u{FF40}' | '\u{FF5B}'..='\u{FF65}' | '\u{2010}'..='\u{2027}' | '\u{FE10}'..='\u{FE4F}')
}

/// A galley drawn on a row: `x` from the row's left, `top` from its top
/// (negative: above it).
struct Item {
    row: usize,
    x: f32,
    top: f32,
    galley: Arc<Galley>,
    angle: f32,
}

/// The mark char of an emphasis, its size as a share of the text's, and
/// whether it goes under the text.
fn emphasis_mark(e: Emphasis) -> (char, f32, bool) {
    match e {
        Emphasis::Dot => ('•', 0.6, false),
        Emphasis::Comma => ('﹅', 0.4, false),
        Emphasis::Circle => ('○', 0.3, false),
        Emphasis::Disc => ('●', 0.3, false),
        Emphasis::UnderDot => ('•', 0.6, true),
    }
}

/// Rows grow to make room for guides and emphasis marks (by whole lines
/// of the grid's `pitch` pixels, if there is one), and the guides, marks
/// and combined lines are laid out over the text.
pub(super) fn place(ctx: &egui::Context, galley: Arc<Galley>, prep: &Prepared, pitch: Option<f32>) -> (Arc<Galley>, Vec<Overlay>) {
    if prep.groups.is_empty() {
        return (galley, Vec::new());
    }
    // (row, glyph) of each char.
    let mut at: Vec<Option<(usize, usize)>> = Vec::new();
    for (ri, r) in galley.rows.iter().enumerate() {
        at.extend((0..r.row.glyphs.len()).map(|gi| Some((ri, gi))));
        if r.ends_with_newline {
            at.push(None);
        }
    }
    let text = &galley.job.text;
    let char_at = |b: usize| text.get(..b).map_or(0, |t| t.chars().count());
    let glyphs_of = |bytes: &Range<usize>| -> Vec<(usize, usize)> { (char_at(bytes.start)..char_at(bytes.end)).filter_map(|c| at.get(c).copied().flatten()).collect() };
    let v = prep.vertical;
    let mut items: Vec<Item> = Vec::new();
    let mut mark_galleys: Vec<(char, u32, Color32, Arc<Galley>)> = Vec::new();
    for g in &prep.groups {
        let gl = glyphs_of(&g.bytes);
        let Some(&(row, _)) = gl.first() else { continue };
        let on_row: Vec<&egui::epaint::text::Glyph> = gl.iter().filter(|(r, _)| *r == row).map(|&(r, i)| &galley.rows[r].row.glyphs[i]).collect();
        let (x0, x1) = (on_row[0].pos.x, on_row.last().map_or(0.0, |l| l.pos.x + l.advance_width));
        let baseline = on_row.iter().map(|g| g.pos.y).fold(f32::MIN, f32::max);
        let ascent = on_row.iter().map(|g| g.font_ascent).fold(0.0, f32::max);
        let height = on_row.iter().map(|g| g.font_height).fold(0.0, f32::max);
        match &g.kind {
            Kind::Ruby(r) => {
                let size = ruby_size(r, g.size, prep.pt);
                let plain = text_galley(ctx, &r.text, size, &g.family, g.color, 0.0, v);
                let (w, m) = (plain.rect.width(), r.text.chars().count() as f32);
                let room = x1 - x0 - w;
                let (galley, x) = match r.align.as_deref() {
                    Some("left") => (plain, x0),
                    Some("right") => (plain, x1 - w),
                    Some("distributeLetter") if room > 0.5 && m > 1.0 => (text_galley(ctx, &r.text, size, &g.family, g.color, room / (m - 1.0), v), x0),
                    Some("distributeSpace") if room > 0.5 => (text_galley(ctx, &r.text, size, &g.family, g.color, room / m, v), x0 + room / (2.0 * m)),
                    _ => (plain, (x0 + x1 - w) / 2.0),
                };
                let gh = galley.rect.height();
                let top = if r.below {
                    baseline - ascent + height
                } else {
                    // Without a raise (odt, rtf) as Word places a guide by
                    // default: its baseline 1 pt below the base's size.
                    let raise = r.raise.unwrap_or_else(|| (g.size / prep.pt * 2.0).round() as i32 - 2);
                    let ruby_ascent = galley.rows.first().and_then(|r| r.row.glyphs.first()).map_or(gh * 0.8, |g| g.pos.y);
                    // Never into the base text.
                    (baseline - raise as f32 / 2.0 * prep.pt - ruby_ascent).min(baseline - ascent * 0.8 - gh * 0.2)
                };
                items.push(Item { row, x, top, galley, angle: 0.0 });
            }
            Kind::Combine { lines, brackets } => {
                let half = g.size / 2.0;
                let top = baseline - ascent;
                let (l, r) = match brackets {
                    Some((a, b)) => (Some(text_galley(ctx, &a.to_string(), g.size, &g.family, g.color, 0.0, v)), Some(text_galley(ctx, &b.to_string(), g.size, &g.family, g.color, 0.0, v))),
                    None => (None, None),
                };
                let bl = l.as_ref().map_or(0.0, |g| g.rect.width());
                let br = r.as_ref().map_or(0.0, |g| g.rect.width());
                let inner = (x1 - x0 - bl - br).max(0.0);
                let lg: Vec<Arc<Galley>> = lines.iter().map(|t| text_galley(ctx, t, half, &g.family, g.color, 0.0, v)).collect();
                let line_h = lg.first().map_or(half, |g| g.rect.height());
                // The two lines fill the height of the text.
                let gap = ((height - 2.0 * line_h) / 2.0).max(0.0);
                for (k, lgal) in lg.into_iter().enumerate() {
                    let x = x0 + bl + (inner - lgal.rect.width()) / 2.0;
                    items.push(Item { row, x, top: top + gap / 2.0 + k as f32 * (line_h + gap), galley: lgal, angle: 0.0 });
                }
                if let Some(l) = l {
                    let a = l.rows.first().and_then(|r| r.row.glyphs.first()).map_or(0.0, |g| g.pos.y);
                    items.push(Item { row, x: x0, top: baseline - a, galley: l, angle: 0.0 });
                }
                if let Some(r) = r {
                    let a = r.rows.first().and_then(|r| r.row.glyphs.first()).map_or(0.0, |g| g.pos.y);
                    items.push(Item { row, x: x1 - br, top: baseline - a, galley: r, angle: 0.0 });
                }
            }
            Kind::Fit(_) => {}
            Kind::Upright { text, compress } => {
                // Horizontal text turned back a quarter, its ink centered
                // on the middle of the line (where upright characters are
                // turned about) and of its cell.
                let mut hg = text_galley(ctx, text, g.size, &g.family, g.color, 0.0, false);
                let w = hg.rect.width();
                if *compress && w > g.size * 1.05 {
                    hg = text_galley(ctx, text, g.size * g.size / w, &g.family, g.color, 0.0, false);
                }
                let ink = if hg.mesh_bounds.is_positive() { hg.mesh_bounds } else { hg.rect };
                let c = ink.center();
                let (xc, yc) = ((x0 + x1) / 2.0, baseline - EM_BOX_MIDDLE * g.size);
                items.push(Item { row, x: xc - c.y, top: yc + c.x, galley: hg, angle: -FRAC_PI_2 });
            }
            Kind::Emphasis(e) => {
                let (c, share, below) = emphasis_mark(*e);
                let size = g.size * share;
                let key = (c, size.to_bits(), g.color);
                let mg = match mark_galleys.iter().find(|m| (m.0, m.1, m.2) == key) {
                    Some(m) => m.3.clone(),
                    None => {
                        let mg = text_galley(ctx, &c.to_string(), size, &g.family, g.color, 0.0, v);
                        mark_galleys.push((c, size.to_bits(), g.color, mg.clone()));
                        mg
                    }
                };
                // Placed by their ink: fonts put these marks anywhere in
                // the em box.
                let ink = if mg.mesh_bounds.is_positive() { mg.mesh_bounds } else { mg.rect };
                let (ink_x, ink_top, ink_bottom) = (ink.center().x - mg.rect.min.x, ink.min.y - mg.rect.min.y, ink.max.y - mg.rect.min.y);
                let gap = g.size * 0.15;
                for &(r, i) in &gl {
                    let glyph = &galley.rows[r].row.glyphs[i];
                    if is_punct(glyph.chr) || glyph.advance_width <= 0.0 {
                        continue;
                    }
                    let b = glyph.pos.y;
                    // Just outside the em box (0.88 of the size over the
                    // baseline, 0.12 under it).
                    let top = if below { b + g.size * 0.12 + gap - ink_top } else { b - g.size * 0.88 - gap - ink_bottom };
                    items.push(Item { row: r, x: glyph.pos.x + glyph.advance_width / 2.0 - ink_x, top, galley: mg.clone(), angle: 0.0 });
                }
            }
        }
    }
    // Room over and under each row.
    let n = galley.rows.len();
    let (mut over, mut under) = (vec![0.0f32; n], vec![0.0f32; n]);
    for it in items.iter().filter(|it| it.angle == 0.0) {
        let h = galley.rows[it.row].row.size.y;
        over[it.row] = over[it.row].max(-it.top);
        under[it.row] = under[it.row].max(it.top + it.galley.rect.height() - h);
    }
    let snap = |v: f32| match pitch {
        _ if v < 0.5 => 0.0,
        Some(p) if p > 1.0 => (v / p - 0.01).ceil() * p,
        _ => v,
    };
    let mut g = (*galley).clone();
    let mut shift = 0.0;
    let mut tops = Vec::with_capacity(n);
    for (ri, prow) in g.rows.iter_mut().enumerate() {
        let (up, down) = (snap(over[ri]), snap(under[ri]));
        prow.pos.y += shift;
        tops.push((prow.pos, up));
        if up > 0.0 || down > 0.0 {
            let row = Arc::make_mut(&mut prow.row);
            if up > 0.0 {
                for gl in &mut row.glyphs {
                    gl.pos.y += up;
                }
                for v in &mut row.visuals.mesh.vertices {
                    v.pos.y += up;
                }
                row.visuals.mesh_bounds = row.visuals.mesh_bounds.translate(vec2(0.0, up));
            }
            row.size.y += up + down;
        }
        shift += up + down;
    }
    g.rect.max.y += shift;
    g.mesh_bounds.max.y += shift;
    let overlays = items
        .into_iter()
        .map(|it| {
            let (pos, up) = tops[it.row];
            Overlay { pos: pos2(pos.x + it.x, pos.y + up + it.top), row_top: pos.y, galley: it.galley, angle: it.angle }
        })
        .collect();
    (Arc::new(g), overlays)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combined_lines_split_in_half() {
        assert_eq!(split_lines("東京大阪"), ["東京".to_string(), "大阪".to_string()]);
        assert_eq!(split_lines("abc"), ["ab".to_string(), "c".to_string()]);
        assert_eq!(brackets("square"), ('[', ']'));
        assert_eq!(brackets("〔〕"), ('〔', '〕'));
    }

    #[test]
    fn punctuation_takes_no_mark() {
        assert!(is_punct('。') && is_punct('、') && is_punct('!') && is_punct('　'));
        assert!(!is_punct('漢') && !is_punct('a'));
    }

    fn job(text: &str, cuts: &[usize]) -> LayoutJob {
        let mut j = LayoutJob::default();
        let mut from = 0;
        for (k, &c) in cuts.iter().chain([text.len()].iter()).enumerate() {
            // (Equal formats would be merged.)
            j.append(&text[from..c], 0.0, TextFormat { color: Color32::from_gray(k as u8), ..Default::default() });
            from = c;
        }
        j
    }

    #[test]
    fn sections_split_and_stretch() {
        let mut j = job("abcdef", &[3]);
        split_at(&mut j, 1);
        split_at(&mut j, 5);
        assert_eq!(j.sections.len(), 4);
        let secs = sections_in(&j, &(1..5));
        assert_eq!(secs, 1..3);
        stretch(&mut j, secs, 6.0, 1.5);
        // Four chars: three gaps of 1.5, plus 1.5 before (less the
        // spacing epaint puts before the first one).
        assert_eq!(j.sections[1].leading_space, 0.0);
        assert_eq!(j.sections[1].format.extra_letter_spacing, 1.5);
        assert_eq!(j.sections[2].leading_space, 0.0);
    }
}
