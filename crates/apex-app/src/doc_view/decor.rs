//! Paragraph and page decoration in the page view: tab stops and drop
//! caps (applied when the text is laid out), paragraph borders and
//! shading, and the page color, borders and watermark.

use std::sync::Arc;

use apex_doc::{Align, DropCap, Line, LineRestart, LineStyle, PageDeco, ParaStyle, Paragraph, TabAlign, TabLeader, TabStops};
use eframe::egui::epaint::text::{ByteIndex, LayoutJob, LayoutSection};
use eframe::egui::epaint::TextShape;
use eframe::egui::{self, Color32, Galley, Pos2, Rect, Shape, Stroke, pos2, vec2};

use super::{DocView, Layout, PT, ParaLayout, Segment};
use crate::theme::{self, Paper};

/// A tab's leader: the row and the x range it fills (galley coordinates).
#[derive(Clone, Copy, Debug)]
pub(super) struct TabFill {
    pub row: usize,
    pub x0: f32,
    pub x1: f32,
    pub leader: TabLeader,
}

// --- laying out text ---------------------------------------------------------

/// Lays out a paragraph's text: tabs go to its tab stops and a drop cap
/// spans its first lines. `x_off` is where the galley's x = 0 is from the
/// left edge of the text area (for the tab stop positions), in pixels.
pub(super) fn layout_text(ctx: &egui::Context, mut job: LayoutJob, st: &ParaStyle, x_off: f32, pt: f32, every: f32) -> (Arc<Galley>, Vec<TabFill>) {
    // A table of contents entry has its own right tab (`refs::toc_tab`).
    let toc_tab = st.toc.is_some_and(|t| t.opts.right_align && t.opts.page_numbers);
    let tabs = job.text.contains('\t') && job.halign == egui::Align::LEFT && !toc_tab;
    let cap = st.drop_cap.filter(|_| matches!(st.align, Align::Left | Align::Justify)).and_then(|d| cap_end(&job.text, d.sane()).map(|e| (e, d.sane())));
    if !tabs && cap.is_none() {
        return (ctx.fonts_mut(|f| f.layout_job(job)), Vec::new());
    }
    // Justified rows would move the text after the tabs.
    if tabs {
        job.justify = false;
    }
    match cap {
        Some((end, d)) => drop_cap_layout(ctx, job, end, d, &st.tabs, x_off, pt, every),
        None => tab_layout(ctx, job, &st.tabs, x_off, pt, every),
    }
}

/// Byte end of the characters a drop cap takes (letters, digits and
/// punctuation, not spaces or pictures).
fn cap_end(text: &str, d: DropCap) -> Option<usize> {
    let mut end = 0;
    for (n, (i, c)) in text.char_indices().enumerate() {
        if n == d.chars as usize {
            break;
        }
        if c.is_whitespace() || c.is_control() || c == '\u{A0}' || c == '\u{2028}' {
            return None;
        }
        end = i + c.len_utf8();
    }
    (end > 0).then_some(end)
}

/// The part `from..to` of a job, as a job of its own.
fn sub_job(job: &LayoutJob, from: usize, to: usize) -> LayoutJob {
    let mut j = LayoutJob { text: job.text[from..to].to_string(), wrap: job.wrap.clone(), halign: job.halign, justify: job.justify, ..Default::default() };
    for s in &job.sections {
        let (a, b) = (s.byte_range.start.0.max(from), s.byte_range.end.0.min(to));
        if a < b {
            let leading_space = if s.byte_range.start.0 >= from { s.leading_space } else { 0.0 };
            j.sections.push(LayoutSection { leading_space, byte_range: ByteIndex(a - from)..ByteIndex(b - from), format: s.format.clone() });
        }
    }
    if j.sections.is_empty() {
        let format = job.sections.iter().rev().find(|s| s.byte_range.start.0 <= from).or(job.sections.first()).map(|s| s.format.clone()).unwrap_or_default();
        j.sections.push(LayoutSection { leading_space: 0.0, byte_range: ByteIndex(0)..ByteIndex(j.text.len()), format });
    }
    j
}

/// Gives every tab character a section of its own; returns their indices.
fn split_tabs(job: &mut LayoutJob) -> Vec<usize> {
    let mut out = Vec::new();
    let mut sections = Vec::with_capacity(job.sections.len());
    for s in std::mem::take(&mut job.sections) {
        let (a, b) = (s.byte_range.start.0, s.byte_range.end.0);
        let mut pos = a;
        let mut lead = s.leading_space;
        for (i, _) in job.text[a..b].match_indices('\t') {
            let t = a + i;
            if t > pos {
                sections.push(LayoutSection { leading_space: lead, byte_range: ByteIndex(pos)..ByteIndex(t), format: s.format.clone() });
                lead = 0.0;
            }
            out.push(sections.len());
            sections.push(LayoutSection { leading_space: lead, byte_range: ByteIndex(t)..ByteIndex(t + 1), format: s.format.clone() });
            lead = 0.0;
            pos = t + 1;
        }
        if pos < b || (a == b && sections.is_empty()) {
            sections.push(LayoutSection { leading_space: lead, byte_range: ByteIndex(pos)..ByteIndex(b), format: s.format.clone() });
        }
    }
    job.sections = sections;
    out
}

/// Lays out text with tabs at the tab stops: each tab's section gets the
/// leading space that puts the text after it at its stop. A few passes,
/// as moving a tab can rewrap the rows.
fn tab_layout(ctx: &egui::Context, mut job: LayoutJob, stops: &TabStops, x_off: f32, pt: f32, every: f32) -> (Arc<Galley>, Vec<TabFill>) {
    let idx = split_tabs(&mut job);
    let mut lead: Vec<f32> = idx.iter().map(|&i| job.sections[i].leading_space).collect();
    let mut galley = ctx.fonts_mut(|f| f.layout_job(job.clone()));
    // A one-line paragraph reaches its stops past the right indent, as in
    // Word (figures tabbed into narrow cells); the tab chars' own widths
    // don't wrap it.
    let far = stops.as_slice().last().map_or(0.0, |s| s.pos * pt - x_off).max(job.wrap.max_width);
    if galley.rows.len() > 1 && !job.text.contains('\n') {
        let mut wide = job.clone();
        wide.wrap.max_width = f32::INFINITY;
        let g = ctx.fonts_mut(|f| f.layout_job(wide.clone()));
        let tabs: f32 = g.rows.iter().flat_map(|r| &r.row.glyphs).filter(|c| c.chr == '\t').map(|c| c.advance_width).sum();
        if g.size().x - tabs <= far {
            wide.wrap.max_width = far + tabs + 0.5;
            galley = ctx.fonts_mut(|f| f.layout_job(wide.clone()));
            job = wide;
        }
    }
    let mut fills = Vec::new();
    for _ in 0..4 {
        let (want, f) = tab_targets(&galley, &lead, stops, x_off, pt, job.wrap.indent_rest, every);
        fills = f;
        if want.len() != lead.len() || want.iter().zip(&lead).all(|(w, l)| (w - l).abs() < 0.5) {
            break;
        }
        lead = want;
        for (k, &i) in idx.iter().enumerate() {
            job.sections[i].leading_space = lead[k];
        }
        galley = ctx.fonts_mut(|f| f.layout_job(job.clone()));
    }
    (galley, fills)
}

/// Where each tab (in text order) should end, as the leading space of its
/// section, given the current layout; and the leaders to draw.
/// `hang` is the hanging indent: in the first row, a stop of its own.
fn tab_targets(g: &Galley, lead: &[f32], stops: &TabStops, x_off: f32, pt: f32, hang: f32, every: f32) -> (Vec<f32>, Vec<TabFill>) {
    let mut out = Vec::with_capacity(lead.len());
    let mut fills = Vec::new();
    for (ri, row) in g.rows.iter().enumerate() {
        let glyphs = &row.row.glyphs;
        let mut shift = 0.0;
        for (gi, t) in glyphs.iter().enumerate() {
            if t.chr != '\t' {
                continue;
            }
            let k = out.len();
            let old = lead.get(k).copied().unwrap_or(0.0);
            let x_before = t.pos.x - old + shift;
            let x_pt = (x_before + row.pos.x + x_off) / pt;
            // Text aligned to a stop ends a hair before it: the tab after
            // it goes to the next one.
            let mut stop = stops.next_every(x_pt + 0.5, every);
            let hang_pt = (x_off + hang) / pt;
            // (Default stops before it don't count, as in Word.)
            let custom = stops.as_slice().iter().any(|s| s.pos > x_pt + 0.5 + 0.01);
            if ri == 0 && hang > 0.0 && x_pt + 0.5 < hang_pt && (stop.pos > hang_pt || !custom) {
                stop = apex_doc::TabStop { pos: hang_pt, ..Default::default() };
            }
            // The text up to the next tab (or the end of the row).
            let after = &glyphs[gi + 1..];
            let seg = after.iter().position(|g| g.chr == '\t').map_or(after, |n| &after[..n]);
            let start = t.pos.x + t.advance_width;
            let seg_w = seg.iter().rfind(|g| !g.chr.is_whitespace()).map_or(0.0, |g| g.max_x() - start);
            let before_dec = seg.iter().rfind(|g| g.chr == '.' || g.chr == ',').map_or(seg_w, |g| g.pos.x - start);
            let back = match stop.align {
                TabAlign::Left => 0.0,
                TabAlign::Center => seg_w / 2.0,
                TabAlign::Right => seg_w,
                TabAlign::Decimal => before_dec,
            };
            let mut end = (stop.pos * pt - x_off - row.pos.x - back).max(x_before);
            // A tab with no text after it doesn't push the line past its end.
            if seg_w <= 0.0 {
                end = end.min(g.job.wrap.max_width - row.pos.x - t.advance_width).max(x_before);
            }
            let new = end - x_before - t.advance_width;
            shift += new - old;
            out.push(new);
            if stop.leader != TabLeader::None {
                fills.push(TabFill { row: ri, x0: x_before, x1: end, leader: stop.leader });
            }
        }
    }
    (out, fills)
}

/// Lays out a paragraph with a drop cap: the cap in a large size, the
/// first lines beside it and the rest under them, joined in one galley so
/// that the caret and selection work as in any other paragraph.
fn drop_cap_layout(ctx: &egui::Context, job: LayoutJob, cap: usize, d: DropCap, stops: &TabStops, x_off: f32, pt: f32, every: f32) -> (Arc<Galley>, Vec<TabFill>) {
    let Some(first) = job.sections.first() else { return (ctx.fonts_mut(|f| f.layout_job(job)), Vec::new()) };
    let base = first.format.font_id.size;
    let line_h = first.format.line_height.unwrap_or(base * 1.2).max(1.0);
    let lines = d.lines.max(1) as usize;
    // Capital letters are about 0.7 em tall: the cap's letters reach from
    // the top of the first line's capitals to the last line's baseline.
    let size = ((lines - 1) as f32 * line_h + 0.7 * base) / 0.7;
    let mut cap_fmt = first.format.clone();
    cap_fmt.font_id.size = size.clamp(1.0, 2000.0);
    cap_fmt.line_height = None;
    cap_fmt.valign = egui::Align::BOTTOM;
    cap_fmt.underline = Stroke::NONE;
    cap_fmt.strikethrough = Stroke::NONE;
    let cap_job = LayoutJob::single_section(job.text[..cap].to_string(), cap_fmt.clone());
    let cap_g = ctx.fonts_mut(|f| f.layout_job(cap_job));
    let width = job.wrap.max_width;
    let dw = cap_g.size().x + d.distance * pt + base * 0.15;
    let rest = sub_job(&job, cap, job.text.len());
    let mut first_job = rest.clone();
    let beside = if d.margin { 0.0 } else { dw };
    first_job.wrap.max_width = (width - beside).max(20.0);
    let has_tabs = rest.text.contains('\t');
    let lay = |j: LayoutJob, off: f32| if has_tabs { tab_layout(ctx, j, stops, off, pt, every) } else { (ctx.fonts_mut(|f| f.layout_job(j)), Vec::new()) };
    let (t1, f1) = lay(first_job, x_off + beside);
    // The rows beside the cap, and the rest laid out full width under them.
    let k = t1.rows.len().min(lines);
    let (t2, f2) = if t1.rows.len() > lines && !d.margin {
        let chars: usize = t1.rows[..k].iter().map(|r| r.char_count_including_newline().0).sum();
        let b = rest.text.char_indices().nth(chars).map_or(rest.text.len(), |(b, _)| b);
        let (g, f) = lay(sub_job(&rest, b, rest.text.len()), x_off);
        (Some(g), f)
    } else {
        (None, Vec::new())
    };
    let t1_rows: &[eframe::egui::epaint::text::PlacedRow] = if t2.is_some() { &t1.rows[..k] } else { &t1.rows };
    // The cap's baseline sits on the baseline of its last line.
    let base_of = |r: &eframe::egui::epaint::text::PlacedRow| r.pos.y + r.row.glyphs.first().map_or(r.row.size.y * 0.8, |g| g.pos.y);
    let target = t1_rows.get(k.saturating_sub(1)).map_or(0.0, base_of);
    let Some(cap_row) = cap_g.rows.first() else { return (t1, f1) };
    let cap_base = cap_row.row.glyphs.first().map_or(cap_row.row.size.y * 0.8, |g| g.pos.y);
    let mut cap_top = target - cap_base;
    // A cap taller than its lines moves them down.
    let push = (-cap_top).max(0.0);
    cap_top += push;
    let mut rows = Vec::with_capacity(1 + t1.rows.len() + t2.as_ref().map_or(0, |g| g.rows.len()));
    let row0 = t1_rows.first().map_or((0.0, line_h), |r| (r.pos.y + push, r.row.size.y));
    let mut c = cap_row.clone();
    {
        // The cap's row is as tall as the first line (so clicks beside the
        // cap reach the lines), its letters moved down to their place.
        let dy = cap_top - row0.0;
        let r = Arc::make_mut(&mut c.row);
        for g in &mut r.glyphs {
            g.pos.y += dy;
        }
        r.visuals.mesh.translate(vec2(0.0, dy));
        r.visuals.mesh_bounds = r.visuals.mesh_bounds.translate(vec2(0.0, dy));
        r.size.y = row0.1;
        c.pos = pos2(if d.margin { -dw } else { 0.0 }, row0.0);
        c.ends_with_newline = false;
    }
    let cap_bottom = cap_top + cap_row.row.size.y;
    rows.push(c);
    let mut fills: Vec<TabFill> = f1.iter().filter(|f| f.row < t1_rows.len()).map(|f| TabFill { row: f.row + 1, x0: f.x0, x1: f.x1, ..*f }).collect();
    for r in t1_rows {
        let mut r = r.clone();
        r.pos += vec2(beside, push);
        rows.push(r);
    }
    let t1_bottom = t1_rows.last().map_or(0.0, |r| r.pos.y + r.row.size.y) + push;
    if let Some(g2) = &t2 {
        let n = rows.len();
        fills.extend(f2.iter().map(|f| TabFill { row: f.row + n, ..*f }));
        for r in &g2.rows {
            let mut r = r.clone();
            r.pos.y += t1_bottom;
            rows.push(r);
        }
    }
    // One galley over the whole text: the cap's characters in their size.
    let mut full = job.clone();
    full.sections.retain(|s| s.byte_range.end.0 > cap);
    for s in &mut full.sections {
        s.byte_range.start = ByteIndex(s.byte_range.start.0.max(cap));
    }
    full.sections.insert(0, LayoutSection { leading_space: 0.0, byte_range: ByteIndex(0)..ByteIndex(cap), format: cap_fmt });
    let mut g: Galley = (*t1).clone();
    let mut rect = Rect::NOTHING;
    let mut mesh = Rect::NOTHING;
    let (mut nv, mut ni) = (0, 0);
    for r in &rows {
        rect |= r.rect();
        mesh |= r.row.visuals.mesh_bounds.translate(r.pos.to_vec2());
        nv += r.row.visuals.mesh.vertices.len();
        ni += r.row.visuals.mesh.indices.len();
    }
    rect.min.y = 0.0;
    rect.max.y = rect.max.y.max(cap_bottom);
    g.rows = rows;
    g.rect = rect;
    g.mesh_bounds = mesh;
    g.num_vertices = nv;
    g.num_indices = ni;
    g.job = Arc::new(full);
    (Arc::new(g), fills)
}

// --- paragraph borders ---------------------------------------------------------

/// Paragraphs `a` and `b` share their borders (one box around both).
fn same_box(a: &Paragraph, b: &Paragraph) -> bool {
    let (x, y) = (&a.style, &b.style);
    x.borders == y.borders && x.indent_left == y.indent_left && x.indent_right == y.indent_right && x.level == y.level && a.cell.as_ref().map(|c| c.key()) == b.cell.as_ref().map(|c| c.key())
}

/// Whether paragraph `i` starts / ends its border box.
fn box_ends(paras: &[Paragraph], i: usize) -> (bool, bool) {
    let first = i == 0 || !same_box(&paras[i - 1], &paras[i]);
    let last = i + 1 >= paras.len() || !same_box(&paras[i], &paras[i + 1]);
    (first, last)
}

/// Extra room above and below paragraph `i` for its borders, in points.
pub(super) fn border_room(paras: &[Paragraph], i: usize) -> (f32, f32) {
    let b = &paras[i].style.borders;
    if b.is_empty() {
        return (0.0, 0.0);
    }
    let (first, last) = box_ends(paras, i);
    let room = |l: Option<Line>| l.map_or(0.0, |l| l.space.max(0.0) + l.thickness());
    (if first { room(b.top) } else { room(b.between) }, if last { room(b.bottom) } else { 0.0 })
}

/// Draws a border line from `a` to `b` in its style.
pub(super) fn draw_line(p: &egui::Painter, a: Pos2, b: Pos2, l: &Line, px: f32) {
    let w = (l.width * px).max(1.0);
    let color = theme::rgb(l.color);
    let stroke = Stroke::new(w, color);
    let d = (b - a).normalized();
    let n = vec2(-d.y, d.x);
    match l.style {
        LineStyle::Single | LineStyle::Thick => {
            p.line_segment([a, b], stroke);
        }
        LineStyle::Double => {
            p.line_segment([a - n * w, b - n * w], stroke);
            p.line_segment([a + n * w, b + n * w], stroke);
        }
        LineStyle::Dotted => {
            p.extend(Shape::dotted_line(&[a, b], color, (w * 2.5).max(3.0), w / 2.0 + 0.3));
        }
        LineStyle::Dashed => {
            p.extend(Shape::dashed_line(&[a, b], stroke, (w * 4.0).max(4.0), (w * 2.5).max(2.5)));
        }
        LineStyle::DashDot => {
            let (dash, dot, gap) = ((w * 5.0).max(5.0), w.max(1.0), (w * 2.5).max(2.5));
            p.extend(Shape::dashed_line_with_offset(&[a, b], stroke, &[dash, dot], &[gap, gap], 0.0));
        }
        LineStyle::Wave => {
            let len = (b - a).length();
            let step = (w * 3.0).max(3.0);
            let mut pts = Vec::new();
            let mut t = 0.0;
            let mut up = true;
            while t <= len {
                pts.push(a + d * t + n * if up { w } else { -w });
                up = !up;
                t += step;
            }
            p.add(Shape::line(pts, stroke));
        }
    }
}

impl DocView {
    /// The border box and shading of a paragraph segment (behind its text).
    /// Returns true when it painted the shading.
    pub(super) fn paint_para_box(&self, p: &egui::Painter, l: &Layout, seg: &Segment, o: Pos2, z: f32) -> bool {
        let paras = &self.doc.paragraphs;
        let para = &paras[seg.para];
        let b = &para.style.borders;
        if b.is_empty() {
            return false;
        }
        let px = PT * z;
        let pl = &l.paras[seg.para];
        let (first, last) = box_ends(paras, seg.para);
        let (room_top, room_bottom) = border_room(paras, seg.para);
        let segs = l.segs_of(seg.para);
        let first_seg = segs.first().is_some_and(|s| s.page == seg.page && s.y == seg.y);
        let last_seg = segs.last().is_some_and(|s| s.page == seg.page && s.y == seg.y);
        let top = o.y + seg.y;
        let bottom = top + seg.gy1 - seg.gy0;
        // Inside a box the spacing between paragraphs is part of it.
        let y0 = if first_seg { top - room_top * px - if first { 0.0 } else { pl.space_before } } else { top };
        let y1 = if last_seg { bottom + if last { room_bottom * px } else { pl.space_after } } else { bottom };
        let x0 = o.x + pl.indent - b.left.map_or(0.0, |l| (l.space + l.thickness()) * px);
        let x1 = o.x + pl.indent + pl.width + b.right.map_or(0.0, |l| (l.space + l.thickness()) * px);
        if let Some(c) = para.style.shading {
            p.rect_filled(Rect::from_min_max(pos2(x0, y0), pos2(x1, y1)), 0.0, theme::rgb(c));
        }
        let half = |l: &Line| l.thickness() * px / 2.0;
        if first_seg && first {
            if let Some(t) = &b.top {
                let y = y0 + half(t);
                draw_line(p, pos2(x0, y), pos2(x1, y), t, px);
            }
        } else if first_seg {
            if let Some(t) = &b.between {
                let y = y0 - pl.space_before + (t.thickness() * px / 2.0).max(0.5) + pl.space_before;
                draw_line(p, pos2(x0, y), pos2(x1, y), t, px);
            }
        }
        if last_seg && last {
            if let Some(t) = &b.bottom {
                let y = y1 - half(t);
                draw_line(p, pos2(x0, y), pos2(x1, y), t, px);
            }
        }
        if let Some(t) = &b.left {
            let x = x0 + half(t);
            draw_line(p, pos2(x, y0), pos2(x, y1), t, px);
        }
        if let Some(t) = &b.right {
            let x = x1 - half(t);
            draw_line(p, pos2(x, y0), pos2(x, y1), t, px);
        }
        para.style.shading.is_some()
    }

    /// The line numbers of layout `l` (built at zoom `z`): each counted
    /// line of body text, by page, as the sections number them.
    pub(super) fn line_numbers(&self, l: &Layout, z: f32) -> Vec<LineNum> {
        let sections = self.doc.sections();
        if sections.iter().all(|s| s.page.line_numbers.is_none()) {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut n: u32 = 0;
        let mut sect = usize::MAX;
        for page in 0..l.pages {
            let g = l.geo_of(page);
            let Some(cfg) = sections.get(g.sect).and_then(|s| s.page.line_numbers) else {
                sect = g.sect;
                continue;
            };
            match cfg.restart {
                LineRestart::EachPage => n = 0,
                LineRestart::EachSection if g.sect != sect => n = 0,
                _ => {}
            }
            sect = g.sect;
            let dist = cfg.distance.unwrap_or(18.0) * PT * z;
            for seg in l.page_segs(page, page) {
                if seg.cell || seg.hidden || self.doc.paragraphs.get(seg.para).is_none_or(|p| p.style.no_line_numbers || p.cell.is_some()) {
                    continue;
                }
                let Some(pl) = l.paras.get(seg.para) else { continue };
                for row in &pl.galley.rows {
                    if row.pos.y < seg.gy0 - 0.5 || row.pos.y >= seg.gy1 - 0.5 {
                        continue;
                    }
                    n += 1;
                    let num = cfg.start.max(1) as u32 - 1 + n;
                    if num % cfg.count_by.max(1) as u32 != 0 {
                        continue;
                    }
                    let base = row.row.glyphs.first().map_or(row.row.size.y * 0.8, |g| g.pos.y);
                    out.push(LineNum { page, x: seg.x - dist, y: seg.y + row.pos.y - seg.gy0 + base, n: num });
                }
            }
        }
        out
    }

    /// Page color, watermark, page borders and the lines between columns
    /// of a page drawn at `rect`.
    pub(super) fn paint_page_extras(&self, p: &egui::Painter, l: &Layout, page: usize, rect: Rect, z: f32) {
        let g = l.geo_of(page);
        let content = Rect::from_min_size(rect.min + g.margin, g.content);
        paint_page_deco(p, &self.doc.deco, rect, content, PT * z);
        let a = l.line_nums.partition_point(|x| x.page < page);
        let font = egui::FontId::proportional(10.0 * PT * z);
        for ln in l.line_nums[a..].iter().take_while(|x| x.page == page) {
            let gal = p.layout_no_wrap(ln.n.to_string(), font.clone(), Paper::TEXT);
            let ascent = gal.rows.first().and_then(|r| r.row.glyphs.first()).map_or(gal.size().y * 0.8, |g| g.pos.y);
            p.galley(pos2(content.min.x + ln.x - gal.size().x, content.min.y + ln.y - ascent), gal, Paper::TEXT);
        }
        let stroke = Stroke::new(z.max(1.0) * 0.75, Paper::TEXT);
        let a = l.rules.partition_point(|r| r.page < page);
        for r in l.rules[a..].iter().take_while(|r| r.page == page) {
            p.vline(content.min.x + r.x, (content.min.y + r.y0)..=(content.min.y + r.y1), stroke);
        }
    }

    /// Tab leaders of a segment.
    pub(super) fn paint_tab_fills(&self, p: &egui::Painter, pl: &ParaLayout, seg: &Segment, galley_pos: Pos2) {
        let z = self.zoom;
        for f in &pl.tab_fills {
            let Some(row) = pl.galley.rows.get(f.row) else { continue };
            if row.pos.y + row.row.size.y <= seg.gy0 || row.pos.y >= seg.gy1 {
                continue;
            }
            let o = galley_pos + row.pos.to_vec2();
            let base = o.y + row.row.glyphs.first().map_or(row.row.size.y * 0.8, |g| g.pos.y);
            let (x0, x1) = (o.x + f.x0 + 2.0 * z, o.x + f.x1 - 2.0 * z);
            if x1 <= x0 {
                continue;
            }
            match f.leader {
                TabLeader::Dots => {
                    let step = 4.0 * z;
                    let mut x = (x0 / step).ceil() * step;
                    while x <= x1 {
                        p.circle_filled(pos2(x, base - 0.8 * z), 0.7 * z.max(1.0), Paper::TEXT);
                        x += step;
                    }
                }
                TabLeader::Dashes => {
                    p.extend(Shape::dashed_line(&[pos2(x0, base - 3.0 * z), pos2(x1, base - 3.0 * z)], Stroke::new(z.max(1.0), Paper::TEXT), 3.0 * z, 2.0 * z));
                }
                TabLeader::Underline => {
                    p.hline(x0..=x1, base + 1.0 * z, Stroke::new(z.max(1.0), Paper::TEXT));
                }
                TabLeader::None => {}
            }
        }
    }
}

/// What an optional hyphen lays out as: a (nearly invisible) space, so a
/// line may break there. One char each way, so char offsets stay the same.
pub(super) const SOFT_BREAK: &str = "\u{200A}";
/// Same width in bytes, but no break: optional hyphens not used as breaks
/// in justified text (spaces would get stretched).
const SOFT_NO_BREAK: &str = "\u{200B}";

/// Appends text with optional hyphens.
pub(super) fn append_soft_hyphens(job: &mut LayoutJob, t: &str, lead: f32, fmt: egui::TextFormat) {
    let tiny = egui::TextFormat { font_id: egui::FontId::new(1.0, fmt.font_id.family.clone()), ..fmt.clone() };
    let mut lead = lead;
    for (i, piece) in t.split(apex_doc::SOFT_HYPHEN).enumerate() {
        if i > 0 {
            job.append(SOFT_BREAK, lead, tiny.clone());
            lead = 0.0;
        }
        if !piece.is_empty() {
            job.append(piece, lead, fmt.clone());
            lead = 0.0;
        }
    }
}

/// Justified text with optional hyphens: laid out once to find the ones
/// lines break at, then again with the others unbreakable, so they don't
/// widen inside words.
pub(super) fn layout_justified_hyphens(ctx: &egui::Context, job: LayoutJob, st: &ParaStyle, x_off: f32, pt: f32, every: f32) -> (Arc<Galley>, Vec<TabFill>) {
    let (first, _) = layout_text(ctx, job.clone(), st, x_off, pt, every);
    let mut used = std::collections::HashSet::new();
    let mut idx = 0usize;
    for row in &first.rows {
        let n = row.row.glyphs.len();
        if row.row.glyphs.last().is_some_and(|g| g.chr == '\u{200A}') {
            used.insert(idx + n - 1);
        }
        idx += n + usize::from(row.ends_with_newline);
    }
    let mut job = job;
    let mut text = String::with_capacity(job.text.len());
    for (i, c) in job.text.chars().enumerate() {
        if c == '\u{200A}' && !used.contains(&i) {
            text.push_str(SOFT_NO_BREAK);
        } else {
            text.push(c);
        }
    }
    job.text = text;
    layout_text(ctx, job, st, x_off, pt, every)
}

/// The hyphen drawn where a line breaks at an optional hyphen.
pub(super) fn paint_soft_hyphens(p: &egui::Painter, pl: &ParaLayout, seg: &Segment, galley_pos: Pos2) {
    let job = &pl.galley.job;
    let mut idx = 0usize;
    for row in &pl.galley.rows {
        let gs = &row.row.glyphs;
        let start = idx;
        idx += gs.len() + usize::from(row.ends_with_newline);
        if row.pos.y + row.row.size.y <= seg.gy0 || row.pos.y >= seg.gy1 {
            continue;
        }
        let Some(g) = gs.last().filter(|g| g.chr == '\u{200A}') else { continue };
        let Some(prev) = gs.len().checked_sub(2).map(|i| &gs[i]) else { continue };
        // The format of the text before it.
        let Some(byte) = job.text.char_indices().nth(start + gs.len() - 2).map(|(b, _)| b) else { continue };
        let Some(sec) = job.sections.iter().find(|s| s.byte_range.contains(&ByteIndex(byte))) else { continue };
        let dash = p.layout_no_wrap("-".into(), sec.format.font_id.clone(), sec.format.color);
        let ascent = dash.rows.first().and_then(|r| r.row.glyphs.first()).map_or(0.0, |d| d.pos.y);
        let o = galley_pos + row.pos.to_vec2();
        p.galley(pos2(o.x + g.pos.x, o.y + prev.pos.y - ascent), dash, sec.format.color);
    }
}

/// A line number: its page, right edge and baseline (content coordinates).
#[derive(Clone, Copy, Debug)]
pub(super) struct LineNum {
    pub page: usize,
    pub x: f32,
    pub y: f32,
    pub n: u32,
}

// --- page decoration ------------------------------------------------------------

/// Paints the page color, watermark and page borders of a page whose rect
/// is `page` and text area `content`, `px` pixels per point.
pub(super) fn paint_page_deco(p: &egui::Painter, deco: &PageDeco, page: Rect, content: Rect, px: f32) {
    if deco.is_empty() {
        return;
    }
    let bg = deco.color.map_or(Paper::BG, theme::rgb);
    match (deco.color, &deco.gradient) {
        (Some(_), Some(g)) => crate::gradient::paint_rect(p, g, page),
        (Some(_), None) => {
            p.rect_filled(page, 0.0, bg);
        }
        _ => {}
    }
    if let Some(w) = deco.watermark.as_ref().filter(|w| !w.is_empty()) {
        paint_watermark(p, w, page, bg, px);
    }
    let b = &deco.borders;
    if b.is_empty() {
        return;
    }
    // Each side's line, measured from the text or from the page edge.
    let off = |l: &Option<Line>| l.map_or(0.0, |l| (l.space.max(0.0) + l.thickness() / 2.0) * px);
    let r = if b.from_edge {
        Rect::from_min_max(pos2(page.min.x + off(&b.left), page.min.y + off(&b.top)), pos2(page.max.x - off(&b.right), page.max.y - off(&b.bottom)))
    } else {
        Rect::from_min_max(pos2(content.min.x - off(&b.left), content.min.y - off(&b.top)), pos2(content.max.x + off(&b.right), content.max.y + off(&b.bottom)))
    };
    let r = r.intersect(page);
    if let Some(l) = &b.top {
        draw_line(p, r.left_top(), r.right_top(), l, px);
    }
    if let Some(l) = &b.bottom {
        draw_line(p, r.left_bottom(), r.right_bottom(), l, px);
    }
    if let Some(l) = &b.left {
        draw_line(p, r.left_top(), r.left_bottom(), l, px);
    }
    if let Some(l) = &b.right {
        draw_line(p, r.right_top(), r.right_bottom(), l, px);
    }
}

/// Mixes a watermark color with the page color (semitransparent text on a
/// plain page; PDF output needs no transparency).
fn washed(c: Color32, bg: Color32, amount: f32) -> Color32 {
    let m = |a: u8, b: u8| (a as f32 * (1.0 - amount) + b as f32 * amount).round() as u8;
    Color32::from_rgb(m(c.r(), bg.r()), m(c.g(), bg.g()), m(c.b(), bg.b()))
}

fn paint_watermark(p: &egui::Painter, w: &apex_doc::Watermark, page: Rect, bg: Color32, px: f32) {
    let center = page.center();
    if let Some(data) = &w.picture {
        let tex = crate::images::texture(p.ctx(), crate::images::key(data), data, false);
        let Some(t) = tex else { return };
        let size = t.size_vec2();
        if size.x <= 0.0 || size.y <= 0.0 {
            return;
        }
        // Natural size is 96 dpi pixels; "auto" fits the page width.
        let fit = (page.width() * 0.8 / size.x).min(page.height() * 0.8 / size.y);
        let k = if w.scale > 0.0 { w.scale * 0.75 * px } else { fit };
        let rect = Rect::from_center_size(center, size * k);
        crate::images::paint(p, Some(&t), rect, 0.0, [0.0; 4]);
        if w.washout {
            p.rect_filled(rect, 0.0, bg.gamma_multiply(0.75));
        }
        return;
    }
    let text = w.text.trim();
    let angle = if w.diagonal { -(page.height().atan2(page.width())) } else { 0.0 };
    let family = |size: f32| crate::fonts::font_for(w.font.as_deref(), false, false, size).0;
    let color = if w.semitransparent { washed(theme::rgb(w.color), bg, 0.5) } else { theme::rgb(w.color) };
    let size = match w.size {
        Some(s) if s > 0.0 => s * px,
        _ => {
            // Fit: about 80% of the diagonal (or the width).
            let g = p.layout_no_wrap(text.to_string(), family(100.0), color);
            let room = if w.diagonal { page.width().hypot(page.height()) * 0.55 } else { page.width() * 0.8 };
            (100.0 * room / g.size().x.max(1.0)).clamp(8.0, page.height() * 0.5)
        }
    };
    let g = p.layout_no_wrap(text.to_string(), family(size), color);
    let half = g.size() / 2.0;
    let (s, c) = angle.sin_cos();
    // The galley turns about its top left corner: put its center on the
    // page's center.
    let rot = vec2(c * half.x - s * half.y, s * half.x + c * half.y);
    let shape = TextShape::new(center - rot, g, color).with_angle(angle);
    p.add(shape);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_chars() {
        let d = DropCap::default();
        assert_eq!(cap_end("Hola", d), Some(1));
        assert_eq!(cap_end("Éste", d), Some(2));
        assert_eq!(cap_end(" x", d), None);
        assert_eq!(cap_end("", d), None);
        assert_eq!(cap_end("ab", DropCap { chars: 5, ..d }), Some(2));
    }

    #[test]
    fn justified_rows_keep_the_first_line_indent() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        let mut job = LayoutJob { justify: true, ..Default::default() };
        job.wrap.max_width = 200.0;
        job.append(&"word ".repeat(40), 30.0, egui::TextFormat::simple(egui::FontId::proportional(12.0), egui::Color32::BLACK));
        let g = ctx.fonts_mut(|f| f.layout_job(job));
        let x = |r: usize| g.rows[r].pos.x + g.rows[r].row.glyphs[0].pos.x;
        assert!((x(0) - 30.0).abs() < 1.0 && x(1).abs() < 1.0, "{} {}", x(0), x(1));
        let right = |r: usize| g.rows[r].pos.x + g.rows[r].row.glyphs.iter().rev().find(|c| !c.chr.is_whitespace()).unwrap().logical_rect().max.x;
        assert!((right(0) - 200.0).abs() < 2.0, "{}", right(0));
        // Leading spaces keep their width and the row its right edge.
        let mut job = LayoutJob { justify: true, ..Default::default() };
        job.wrap.max_width = 200.0;
        job.append(&format!("        {}", "word ".repeat(40)), 0.0, egui::TextFormat::simple(egui::FontId::proportional(12.0), egui::Color32::BLACK));
        let g = ctx.fonts_mut(|f| f.layout_job(job));
        let first = g.rows[0].row.glyphs.iter().find(|c| !c.chr.is_whitespace()).unwrap();
        let spaces = g.rows[0].row.glyphs[8].pos.x - g.rows[0].row.glyphs[0].pos.x;
        assert!((g.rows[0].pos.x + first.pos.x - spaces).abs() < 1.0 && spaces > 10.0, "{} {spaces}", first.pos.x);
        let right = |r: usize| g.rows[r].pos.x + g.rows[r].row.glyphs.iter().rev().find(|c| !c.chr.is_whitespace()).unwrap().logical_rect().max.x;
        assert!((right(0) - 200.0).abs() < 2.0, "{}", right(0));
    }

    #[test]
    fn tab_sections() {
        let mut job = LayoutJob::default();
        job.append("a\tb", 3.0, Default::default());
        job.append("\t", 0.0, egui::TextFormat { italics: true, ..Default::default() });
        let idx = split_tabs(&mut job);
        assert_eq!(idx, [1, 3]);
        let ranges: Vec<_> = job.sections.iter().map(|s| (s.byte_range.start.0, s.byte_range.end.0)).collect();
        assert_eq!(ranges, [(0, 1), (1, 2), (2, 3), (3, 4)]);
        assert_eq!(job.sections[0].leading_space, 3.0);
        let sub = sub_job(&job, 2, 4);
        assert_eq!(sub.text, "b\t");
        assert_eq!(sub.sections.len(), 2);
        assert_eq!(sub_job(&job, 4, 4).sections.len(), 1);
    }
}
