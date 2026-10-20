#![expect(clippy::unwrap_used)] // TODO(emilk): remove unwraps

use core::{iter, ops::Range};
use std::sync::Arc;

use emath::{Align, GuiRounding as _, NumExt as _, Pos2, Rect, Vec2, pos2, vec2};

use crate::{
    Color32, Mesh, Stroke, Vertex,
    stroke::PathStroke,
    text::{
        ByteIndex, ByteRange,
        font::{StyledMetrics, UvRect, is_cjk, is_cjk_break_allowed},
        fonts::FontFaceKey,
    },
};

use super::{
    ByteRangeExt as _, FontsImpl, Galley, Glyph, LayoutJob, LayoutSection, PlacedRow, Row,
    RowVisuals, VariationCoords,
    font::{Font, FontFace, ShapedGlyph},
};

// ----------------------------------------------------------------------------

/// Returns `true` if the character is a Unicode combining mark (categories Mn, Mc, Me).
///
/// These characters modify the preceding base character and should not be
/// rendered as standalone replacement glyphs when the shaper can't handle them.
#[inline]
fn is_combining_mark(c: char) -> bool {
    use unicode_general_category::{GeneralCategory, get_general_category};
    matches!(
        get_general_category(c),
        GeneralCategory::NonspacingMark
            | GeneralCategory::SpacingMark
            | GeneralCategory::EnclosingMark
    )
}

/// Represents GUI scale and convenience methods for rounding to pixels.
#[derive(Clone, Copy)]
struct PointScale {
    pub pixels_per_point: f32,
}

impl PointScale {
    #[inline(always)]
    pub fn new(pixels_per_point: f32) -> Self {
        Self { pixels_per_point }
    }

    #[inline(always)]
    pub fn pixels_per_point(&self) -> f32 {
        self.pixels_per_point
    }

    #[inline(always)]
    pub fn round_to_pixel(&self, point: f32) -> f32 {
        (point * self.pixels_per_point).round() / self.pixels_per_point
    }
}

// ----------------------------------------------------------------------------

/// Temporary storage before line-wrapping.
#[derive(Clone)]
struct Paragraph {
    /// Start of the next glyph to be added. In screen-space / physical pixels.
    pub cursor_x_px: f32,

    /// This is included in case there are no glyphs
    pub section_index_at_start: u32,

    pub glyphs: Vec<Glyph>,

    /// In case of an empty paragraph ("\n"), use this as height.
    pub empty_paragraph_height: f32,
}

impl Paragraph {
    pub fn from_section_index(section_index_at_start: u32) -> Self {
        Self {
            cursor_x_px: 0.0,
            section_index_at_start,
            glyphs: vec![],
            empty_paragraph_height: 0.0,
        }
    }
}

/// Layout text into a [`Galley`].
///
/// In most cases you should use [`crate::FontsView::layout_job`] instead
/// since that memoizes the input, making subsequent layouting of the same text much faster.
pub fn layout(fonts: &mut FontsImpl, pixels_per_point: f32, job: Arc<LayoutJob>) -> Galley {
    profiling::function_scope!();

    job.debug_sanity_check();

    if job.wrap.max_rows == 0 {
        // Early-out: no text
        return Galley {
            job,
            rows: Default::default(),
            rect: Rect::ZERO,
            mesh_bounds: Rect::NOTHING,
            num_vertices: 0,
            num_indices: 0,
            pixels_per_point,
            elided: true,
            intrinsic_size: Vec2::ZERO,
        };
    }

    // For most of this we ignore the y coordinate:

    let mut paragraphs = vec![Paragraph::from_section_index(0)];
    {
        let mut shape_buffer = fonts.take_shape_buffer();
        for (section_index, section) in job.sections.iter().enumerate() {
            let mut font = fonts.font(&section.format.font_id.family);
            shape_buffer = layout_section(
                &mut font,
                shape_buffer,
                pixels_per_point,
                &job,
                section_index as u32,
                section,
                &mut paragraphs,
            );
        }
        fonts.return_shape_buffer(shape_buffer);
    }

    let point_scale = PointScale::new(pixels_per_point);

    let intrinsic_size = calculate_intrinsic_size(point_scale, &job, &paragraphs);

    let mut elided = false;
    let mut rows = rows_from_paragraphs(paragraphs, &job, pixels_per_point, &mut elided);
    if elided && let Some(last_placed) = rows.last_mut() {
        let last_row = Arc::make_mut(&mut last_placed.row);
        replace_last_glyph_with_overflow_character(fonts, pixels_per_point, &job, last_row);
        if let Some(last) = last_row.glyphs.last() {
            last_row.size.x = last.max_x();
        }
    }

    let justify = job.justify && job.wrap.max_width.is_finite();

    // ApexDesk patch: the hanging indent of each row.
    let indent = |i: usize| if i > 0 || job.wrap.indent_all { job.wrap.indent_rest } else { 0.0 };
    if justify || job.halign != Align::LEFT {
        let num_rows = rows.len();
        for (i, placed_row) in rows.iter_mut().enumerate() {
            let is_last_row = i + 1 == num_rows;
            let justify_row = justify && !placed_row.ends_with_newline && !is_last_row;
            // ApexDesk patch: a last row that took a word more by
            // shrinking its spaces shrinks them, without stretching.
            let squeeze = justify && job.wrap.shrink_spaces > 0.0;
            halign_and_justify_row(
                point_scale,
                placed_row,
                job.halign,
                job.wrap.max_width - indent(i),
                justify_row || squeeze,
                !justify_row,
                job.keep_trailing_whitespace,
                i == 0,
            );
        }
    }
    if job.wrap.indent_rest != 0.0 {
        for (i, placed_row) in rows.iter_mut().enumerate() {
            // (Centered and right-aligned rows sit in the narrower width.)
            placed_row.pos.x += match job.halign {
                Align::LEFT => indent(i),
                Align::Center => indent(i) / 2.0,
                Align::RIGHT => 0.0,
            };
        }
    }

    // ApexDesk patch: right-to-left text.
    for placed_row in &mut rows {
        if placed_row.glyphs.iter().any(|g| !g.chr.is_ascii()) || job.rtl {
            bidi_row(Arc::make_mut(&mut placed_row.row), job.rtl);
        }
    }

    // Calculate the Y positions and tessellate the text:
    galley_from_rows(point_scale, job, rows, elided, intrinsic_size)
}

/// Shared context for emitting shaped glyphs into a [`Paragraph`].
struct ShapingContext {
    pixels_per_point: f32,
    font_size: f32,
    line_height: f32,
    extra_letter_spacing: f32,
    section_index: u32,
    font_metrics: StyledMetrics,
    is_first_glyph_in_section: bool,
    prev_cluster: Option<u32>,
}

impl ShapingContext {
    fn glyph(
        &self,
        chr: char,
        physical_x: i32,
        advance_width_px: f32,
        face_metrics: &StyledMetrics,
        uv_rect: UvRect,
    ) -> Glyph {
        Glyph {
            chr,
            pos: pos2(physical_x as f32 / self.pixels_per_point, f32::NAN),
            advance_width: advance_width_px / self.pixels_per_point,
            line_height: self.line_height,
            font_face_height: face_metrics.row_height,
            font_face_ascent: face_metrics.ascent,
            font_height: self.font_metrics.row_height,
            font_ascent: self.font_metrics.ascent,
            uv_rect,
            shaped: None,
            section_index: self.section_index,
            first_vertex: 0,
        }
    }
}

/// Produced by [`segment_into_runs`] for text shaping.
#[derive(Clone, Debug)]
pub(crate) struct TextRun {
    /// Which font face should shape this run.
    font_key: FontFaceKey,

    /// Byte range within the section text.
    byte_range: ByteRange,
}

/// Emit shaped glyphs from a [`harfrust::GlyphBuffer`] into a [`Paragraph`].
///
/// When a cluster maps multiple characters to fewer glyphs (e.g. flag emojis,
/// ligatures), zero-width "continuation" glyphs are emitted for the extra
/// characters so that `glyphs.len() == char_count` — an invariant that all
/// cursor and selection code relies on.
fn layout_shaped_run(
    font: &mut Font<'_>,
    run: &TextRun,
    run_text: &str,
    (glyph_infos, glyph_positions): (&[harfrust::GlyphInfo], &[harfrust::GlyphPosition]),
    face_metrics: &StyledMetrics,
    ctx: &mut ShapingContext,
    paragraph: &mut Paragraph,
) {
    let px_scale = face_metrics.px_scale_factor;

    // Reset cluster tracking — cluster values are byte offsets within run_text,
    // so they are not comparable across runs.
    ctx.prev_cluster = None;

    // Track how many glyphs we emit per cluster so we can add zero-width
    // continuation glyphs when a cluster has more chars than glyphs.
    let mut cluster_start_byte: usize = 0;
    let mut cluster_glyph_count: usize = 0;

    for (info, pos) in iter::zip(glyph_infos, glyph_positions) {
        let glyph_id = skrifa::GlyphId::new(info.glyph_id);
        let cluster = info.cluster;
        let mut advance_width_px = pos.x_advance as f32 * px_scale;
        let x_offset_px = pos.x_offset as f32 * px_scale;
        let y_offset_px = -(pos.y_offset as f32 * px_scale); // harfrust Y+ up → screen Y+ down

        let chr = run_text
            .get(cluster as usize..)
            .and_then(|s| s.chars().next())
            .unwrap_or('\u{FFFD}'); // Unicode Replacement Character

        // Tab is a layout concept, not a glyph — the shaper doesn't know about tab stops.
        // Override the advance width using the font's configured tab size.
        if chr == '\t' {
            let tweak = font.fonts_by_id.get(&run.font_key).map(|ff| ff.tweak());
            let tab_size = tweak.map_or(4.0, |t| t.tab_size);
            let (_, space_info) = font.glyph_info(' ', face_metrics);
            let space_width_px = space_info.advance_width_unscaled.0 * px_scale;
            advance_width_px = tab_size * space_width_px;
        }

        // Thin space (U+2009) and narrow no-break space (U+202F):
        // override the shaper's advance width with the configured fraction of a space.
        if chr == '\u{2009}' || chr == '\u{202F}' {
            let tweak = font.fonts_by_id.get(&run.font_key).map(|ff| ff.tweak());
            let thin_space_width = tweak.map_or(0.5, |t| t.thin_space_width);
            let (_, space_info) = font.glyph_info(' ', face_metrics);
            let space_width_px = space_info.advance_width_unscaled.0 * px_scale;
            advance_width_px = thin_space_width * space_width_px;
        }

        // Apply extra_letter_spacing only at cluster boundaries,
        // never between glyphs within the same cluster (e.g. base + mark).
        let is_new_cluster = ctx.prev_cluster.is_none_or(|pc| pc != cluster);
        if is_new_cluster {
            if ctx.prev_cluster.is_some() {
                emit_continuation_glyphs(
                    ctx,
                    paragraph,
                    run_text,
                    cluster_start_byte..cluster as usize,
                    cluster_glyph_count,
                    face_metrics,
                );
            }
            if !ctx.is_first_glyph_in_section {
                paragraph.cursor_x_px += ctx.extra_letter_spacing * ctx.pixels_per_point;
            }
            cluster_start_byte = cluster as usize;
            cluster_glyph_count = 0;
            ctx.is_first_glyph_in_section = false;
        }
        ctx.prev_cluster = Some(cluster);

        let glyph = if glyph_id == skrifa::GlyphId::NOTDEF {
            // The shaper couldn't map this character. Drop combining marks
            // (Unicode category M) and duplicate NOTDEF glyphs within the same
            // cluster — only the first base character gets a replacement glyph.
            if is_combining_mark(chr) || !is_new_cluster {
                continue;
            }

            // Use the fallback font face (not run.font_key which returned NOTDEF).
            let fallback_key = font.resolve_face(chr);
            let fallback_metrics = font
                .fonts_by_id
                .get(&fallback_key)
                .map(|ff| {
                    ff.styled_metrics(ctx.pixels_per_point, ctx.font_size, &Default::default())
                })
                .unwrap_or_default();
            let (_, glyph_info) = font.glyph_info(chr, &fallback_metrics);
            let advance_width_px =
                glyph_info.advance_width_unscaled.0 * fallback_metrics.px_scale_factor;
            let (glyph_alloc, physical_x) =
                if let Some(ff) = font.fonts_by_id.get_mut(&fallback_key) {
                    ff.allocate_glyph(
                        font.atlas,
                        &fallback_metrics,
                        &ShapedGlyph {
                            glyph_id: glyph_info.id.unwrap_or(skrifa::GlyphId::NOTDEF),
                            h_pos: paragraph.cursor_x_px,
                            is_cjk: is_cjk(chr),
                        },
                    )
                } else {
                    Default::default()
                };

            paragraph.cursor_x_px += advance_width_px;

            ctx.glyph(
                chr,
                physical_x,
                advance_width_px,
                &fallback_metrics,
                glyph_alloc.uv_rect,
            )
        } else {
            let (mut glyph_alloc, physical_x) =
                if let Some(ff) = font.fonts_by_id.get_mut(&run.font_key) {
                    ff.allocate_glyph(
                        font.atlas,
                        face_metrics,
                        &ShapedGlyph {
                            glyph_id,
                            h_pos: paragraph.cursor_x_px + x_offset_px,
                            is_cjk: is_cjk(chr),
                        },
                    )
                } else {
                    Default::default()
                };

            // Apply shaper y_offset — this varies per glyph instance so it
            // is not part of the cached ShapedGlyph / GlyphAllocation.
            glyph_alloc.uv_rect.offset.y += y_offset_px / ctx.pixels_per_point;

            paragraph.cursor_x_px += advance_width_px;

            let mut glyph = ctx.glyph(
                chr,
                physical_x,
                advance_width_px,
                face_metrics,
                glyph_alloc.uv_rect,
            );
            // ApexDesk patch: contextual forms of complex scripts.
            if chr as u32 >= 0x0590 && !is_cjk(chr) {
                let mapped = font.fonts_by_id.get_mut(&run.font_key).and_then(|ff| ff.glyph_info(chr, face_metrics)).and_then(|g| g.id);
                if let Some(m) = mapped.filter(|m| *m != glyph_id) {
                    glyph.shaped = Some((m.to_u32() as u16, info.glyph_id as u16));
                }
            }
            glyph
        };
        paragraph.glyphs.push(glyph);
        cluster_glyph_count += 1;
    }

    // Emit continuation glyphs for the last cluster in the run.
    if ctx.prev_cluster.is_some() {
        emit_continuation_glyphs(
            ctx,
            paragraph,
            run_text,
            cluster_start_byte..run_text.len(),
            cluster_glyph_count,
            face_metrics,
        );
    }
}

/// Emit zero-width continuation glyphs when a cluster has more characters than
/// shaped glyphs.
///
/// This preserves the invariant `glyphs.len() == char_count` that all cursor
/// and text-selection code depends on. Continuation glyphs have
/// [`UvRect::default()`] so [`tessellate_glyphs`] skips them entirely.
fn emit_continuation_glyphs(
    ctx: &ShapingContext,
    paragraph: &mut Paragraph,
    run_text: &str,
    cluster_bytes: Range<usize>,
    cluster_glyph_count: usize,
    face_metrics: &StyledMetrics,
) {
    let Some(cluster_text) = run_text.get(cluster_bytes) else {
        return;
    };
    let char_count = cluster_text.chars().count();
    if char_count <= cluster_glyph_count {
        return;
    }

    let physical_x = paragraph.cursor_x_px.round() as i32;

    for chr in cluster_text.chars().skip(cluster_glyph_count) {
        paragraph
            .glyphs
            .push(ctx.glyph(chr, physical_x, 0.0, face_metrics, UvRect::default()));
    }
}

// Ignores the Y coordinate.
#[must_use]
fn layout_section(
    font: &mut Font<'_>,
    mut shape_buffer: harfrust::UnicodeBuffer,
    pixels_per_point: f32,
    job: &LayoutJob,
    section_index: u32,
    section: &LayoutSection,
    out_paragraphs: &mut Vec<Paragraph>,
) -> harfrust::UnicodeBuffer {
    let LayoutSection {
        leading_space,
        byte_range,
        format,
    } = section;

    let font_size = format.font_id.size;
    let font_metrics = font.styled_metrics(pixels_per_point, font_size, &format.coords);
    let line_height = section
        .format
        .line_height
        .unwrap_or(font_metrics.row_height);
    let extra_letter_spacing = section.format.extra_letter_spacing;

    let mut paragraph = out_paragraphs.last_mut().unwrap();
    if paragraph.glyphs.is_empty() {
        paragraph.empty_paragraph_height = line_height;
    }
    paragraph.cursor_x_px += leading_space * pixels_per_point;

    let section_text = &job.text[byte_range.as_usize()];
    let mut ctx = ShapingContext {
        pixels_per_point,
        font_size,
        line_height,
        extra_letter_spacing,
        section_index,
        font_metrics,
        is_first_glyph_in_section: paragraph.glyphs.is_empty(),
        prev_cluster: None,
    };
    let mut runs = Vec::new();

    // Process each paragraph segment (split on newlines — the shaper can't handle them).
    for (seg_idx, segment) in SplitOrWhole::new(section_text, job.break_on_newline).enumerate() {
        if 0 < seg_idx {
            paragraph = out_paragraphs.push_mut(Paragraph::from_section_index(section_index));
            paragraph.empty_paragraph_height = line_height;
            ctx.is_first_glyph_in_section = true;
        }

        if segment.is_empty() {
            continue;
        }

        let shaped;
        (shaped, shape_buffer) = shape_segment(font, segment, &format.coords, &mut runs, shape_buffer);
        for sr in &shaped.runs {
            let run_text = &segment[sr.run.byte_range.as_usize()];
            let Some(font_face) = font.fonts_by_id.get(&sr.run.font_key) else {
                continue;
            };

            let face_metrics =
                font_face.styled_metrics(pixels_per_point, font_size, &format.coords);

            layout_shaped_run(
                font,
                &sr.run,
                run_text,
                (&sr.infos, &sr.positions),
                &face_metrics,
                &mut ctx,
                paragraph,
            );
        }
    }

    shape_buffer
}

// ----------------------------------------------------------------------------
// ApexDesk patch: shaping cache.
//
// harfrust shapes in font units without a pixel size, so the result of
// splitting a paragraph into font runs and shaping them does not depend on
// the font size. Keeping it lets a zoom (every text laid out again at a new
// size) skip segmentation and shaping and only redo positioning.

/// One shaped run: its font, byte range and glyphs.
struct ShapedRun {
    run: TextRun,
    infos: Box<[harfrust::GlyphInfo]>,
    positions: Box<[harfrust::GlyphPosition]>,
}

struct ShapedSegment {
    text: Box<str>,
    coords: VariationCoords,
    runs: Box<[ShapedRun]>,
}

/// Two generations: when the current one fills up it becomes the old one, and
/// entries still in use move back on their next hit.
#[derive(Default)]
pub(crate) struct ShapeCache {
    current: ahash::HashMap<u64, Arc<ShapedSegment>>,
    old: ahash::HashMap<u64, Arc<ShapedSegment>>,
    /// Approximate memory of the current generation.
    current_bytes: usize,
}

impl core::fmt::Debug for ShapeCache {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "ShapeCache({} + {})", self.current.len(), self.old.len())
    }
}

const SHAPE_CACHE_SIZE: usize = 16_384;
/// Memory limit of a generation: a long document shaped whole (to paginate
/// it) would otherwise keep tens of MB of glyphs.
const SHAPE_CACHE_BYTES: usize = 4 << 20;

impl ShapedSegment {
    fn bytes(&self) -> usize {
        let glyph = core::mem::size_of::<harfrust::GlyphInfo>() + core::mem::size_of::<harfrust::GlyphPosition>();
        let runs: usize = self.runs.iter().map(|r| core::mem::size_of::<ShapedRun>() + r.infos.len() * glyph).sum();
        core::mem::size_of::<Self>() + self.text.len() + runs + 64
    }
}

impl ShapeCache {
    fn key(text: &str, coords: &VariationCoords) -> u64 {
        use core::hash::{BuildHasher as _, Hash as _, Hasher as _};
        let mut h = ahash::RandomState::with_seeds(1, 2, 3, 4).build_hasher();
        text.hash(&mut h);
        for (tag, value) in coords.as_ref() {
            tag.to_be_bytes().hash(&mut h);
            value.to_bits().hash(&mut h);
        }
        h.finish()
    }

    fn get(&mut self, text: &str, coords: &VariationCoords) -> Option<Arc<ShapedSegment>> {
        let key = Self::key(text, coords);
        let found = |s: &Arc<ShapedSegment>| &*s.text == text && s.coords == *coords;
        if let Some(s) = self.current.get(&key).filter(|s| found(s)) {
            return Some(s.clone());
        }
        let s = self.old.remove(&key).filter(|s| found(s))?;
        self.insert(key, s.clone());
        Some(s)
    }

    fn insert(&mut self, key: u64, s: Arc<ShapedSegment>) {
        let bytes = s.bytes();
        if self.current.len() >= SHAPE_CACHE_SIZE || self.current_bytes + bytes > SHAPE_CACHE_BYTES {
            self.old = core::mem::take(&mut self.current);
            self.current_bytes = 0;
        }
        self.current_bytes += bytes;
        self.current.insert(key, s);
    }
}

/// Shaped runs of `segment`, from the cache or shaped now.
fn shape_segment(
    font: &mut Font<'_>,
    segment: &str,
    coords: &VariationCoords,
    runs: &mut Vec<TextRun>,
    mut shape_buffer: harfrust::UnicodeBuffer,
) -> (Arc<ShapedSegment>, harfrust::UnicodeBuffer) {
    if let Some(s) = font.cached_family.shape_cache.get(segment, coords) {
        return (s, shape_buffer);
    }
    segment_into_runs(font, segment, runs);
    let num_runs = runs.len();
    let mut shaped = Vec::with_capacity(num_runs);
    for (run_idx, run) in runs.iter().enumerate() {
        let run_text = &segment[run.byte_range.as_usize()];
        let Some(font_face) = font.fonts_by_id.get(&run.font_key) else {
            continue;
        };
        let mut flags = harfrust::BufferFlags::empty();
        if run_idx == 0 {
            flags |= harfrust::BufferFlags::BEGINNING_OF_TEXT;
        }
        if run_idx + 1 == num_runs {
            flags |= harfrust::BufferFlags::END_OF_TEXT;
        }
        let (glyph_buffer, rtl) = shape_text(font_face, run_text, coords, shape_buffer, flags);
        let (mut infos, mut positions): (Vec<_>, Vec<_>) = (glyph_buffer.glyph_infos().into(), glyph_buffer.glyph_positions().into());
        // ApexDesk patch: right-to-left runs come shaped in visual order;
        // their glyphs follow the characters (`bidi_row` turns them back).
        if rtl {
            infos.reverse();
            positions.reverse();
        }
        shaped.push(ShapedRun {
            run: run.clone(),
            infos: infos.into(),
            positions: positions.into(),
        });
        shape_buffer = glyph_buffer.clear();
    }
    let s = Arc::new(ShapedSegment { text: segment.into(), coords: coords.clone(), runs: shaped.into() });
    font.cached_family.shape_cache.insert(ShapeCache::key(segment, coords), s.clone());
    (s, shape_buffer)
}

/// Iterator that either splits on `'\n'` or yields the whole string once.
/// Avoids `Box<dyn Iterator>` and `Vec<&str>` allocation.
enum SplitOrWhole<'a> {
    Split(core::str::Split<'a, char>),
    Whole(iter::Once<&'a str>),
}

impl<'a> SplitOrWhole<'a> {
    fn new(text: &'a str, split: bool) -> Self {
        if split {
            Self::Split(text.split('\n'))
        } else {
            Self::Whole(iter::once(text))
        }
    }
}

impl<'a> Iterator for SplitOrWhole<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        match self {
            Self::Split(iter) => iter.next(),
            Self::Whole(iter) => iter.next(),
        }
    }
}

/// Calculate the intrinsic size of the text.
///
/// The result is eventually passed to `Response::intrinsic_size`.
/// This works by calculating the size of each `Paragraph` (instead of each `Row`).
fn calculate_intrinsic_size(
    point_scale: PointScale,
    job: &LayoutJob,
    paragraphs: &[Paragraph],
) -> Vec2 {
    let mut intrinsic_size = Vec2::ZERO;
    for (idx, paragraph) in paragraphs.iter().enumerate() {
        // Use the precise cursor position instead of `last_glyph.max_x()`,
        // because glyph positions are pixel-snapped but the cursor tracks
        // the exact subpixel advance. This ensures that when two galleys are
        // placed side-by-side, the gap matches what it would be within a
        // single galley.
        let width = paragraph.cursor_x_px / point_scale.pixels_per_point;
        intrinsic_size.x = f32::max(intrinsic_size.x, width);

        let mut height = paragraph
            .glyphs
            .iter()
            .map(|g| g.line_height)
            .max_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal))
            .unwrap_or(paragraph.empty_paragraph_height);
        if idx == 0 {
            height = f32::max(height, job.first_row_min_height);
        }
        intrinsic_size.y += point_scale.round_to_pixel(height);
    }
    intrinsic_size
}

// Ignores the Y coordinate.
fn rows_from_paragraphs(
    paragraphs: Vec<Paragraph>,
    job: &LayoutJob,
    pixels_per_point: f32,
    elided: &mut bool,
) -> Vec<PlacedRow> {
    let num_paragraphs = paragraphs.len();

    let mut rows = vec![];

    for (i, paragraph) in paragraphs.into_iter().enumerate() {
        if job.wrap.max_rows <= rows.len() {
            *elided = true;
            break;
        }

        let is_last_paragraph = (i + 1) == num_paragraphs;
        // ApexDesk patch: hanging indents (see `TextWrapping::indent_rest`).
        let first_indent = if rows.is_empty() && !job.wrap.indent_all { 0.0 } else { job.wrap.indent_rest };

        if paragraph.glyphs.is_empty() {
            rows.push(PlacedRow {
                pos: pos2(0.0, f32::NAN),
                row: Arc::new(Row {
                    section_index_at_start: paragraph.section_index_at_start,
                    glyphs: vec![],
                    visuals: Default::default(),
                    size: vec2(0.0, paragraph.empty_paragraph_height),
                }),
                ends_with_newline: !is_last_paragraph,
            });
        } else {
            // Use precise cursor position for width instead of pixel-snapped
            // `last_glyph.max_x()`, so that side-by-side galleys have the same
            // spacing as characters within a single galley.
            let paragraph_width = paragraph.cursor_x_px / pixels_per_point;
            if paragraph_width <= job.effective_wrap_width() - first_indent {
                // Early-out optimization: the whole paragraph fits on one row.
                rows.push(PlacedRow {
                    pos: pos2(0.0, f32::NAN),
                    row: Arc::new(Row {
                        section_index_at_start: paragraph.section_index_at_start,
                        glyphs: paragraph.glyphs,
                        visuals: Default::default(),
                        size: vec2(paragraph_width, 0.0),
                    }),
                    ends_with_newline: !is_last_paragraph,
                });
            } else {
                line_break(&paragraph, job, first_indent, &mut rows, elided);
                let placed_row = rows.last_mut().unwrap();
                placed_row.ends_with_newline = !is_last_paragraph;
            }
        }
    }

    rows
}

fn line_break(
    paragraph: &Paragraph,
    job: &LayoutJob,
    first_indent: f32,
    out_rows: &mut Vec<PlacedRow>,
    elided: &mut bool,
) {
    let mut wrap_width = job.effective_wrap_width() - first_indent;

    // Keeps track of good places to insert row break if we exceed `wrap_width`.
    let mut row_break_candidates = RowBreakCandidates::default();

    let mut first_row_indentation = paragraph.glyphs[0].pos.x;
    let mut row_start_x = 0.0;
    let mut row_start_idx = 0;
    // ApexDesk patch: the width of the row's spaces, which may shrink.
    let shrink = if job.justify { job.wrap.shrink_spaces } else { 0.0 };
    let mut row_spaces = 0.0;

    for i in 0..paragraph.glyphs.len() {
        if job.wrap.max_rows <= out_rows.len() {
            *elided = true;
            break;
        }

        let potential_row_width = paragraph.glyphs[i].max_x() - row_start_x;

        // ApexDesk patch: punctuation and spaces hanging past the edge.
        let hangs = (job.wrap.hanging_punctuation
            && i > row_start_idx
            && super::kinsoku::hangs(paragraph.glyphs[i].chr)
            && paragraph.glyphs[i - 1].max_x() - row_start_x <= wrap_width)
            || (job.wrap.hanging_spaces && paragraph.glyphs[i].chr == ' ')
            || (potential_row_width - wrap_width <= shrink * row_spaces && !paragraph.glyphs[i].chr.is_whitespace());

        if wrap_width < potential_row_width && !hangs {
            // Row break:

            if first_row_indentation > 0.0
                && !row_break_candidates.has_good_candidate(job.wrap.break_anywhere)
            {
                // Allow the first row to be completely empty, because we know there will be more space on the next row:
                // TODO(emilk): this records the height of this first row as zero, though that is probably fine since first_row_indentation usually comes with a first_row_min_height.
                out_rows.push(PlacedRow {
                    pos: pos2(0.0, f32::NAN),
                    row: Arc::new(Row {
                        section_index_at_start: paragraph.section_index_at_start,
                        glyphs: vec![],
                        visuals: Default::default(),
                        size: Vec2::ZERO,
                    }),
                    ends_with_newline: false,
                });
                row_start_x += first_row_indentation;
                first_row_indentation = 0.0;
            } else if let Some(last_kept_index) = row_break_candidates.get(job.wrap.break_anywhere)
                && last_kept_index >= row_start_idx
            {
                let glyphs: Vec<Glyph> = paragraph.glyphs[row_start_idx..=last_kept_index]
                    .iter()
                    .copied()
                    .map(|mut glyph| {
                        glyph.pos.x -= row_start_x;
                        glyph
                    })
                    .collect();

                let section_index_at_start = glyphs[0].section_index;
                let paragraph_max_x = glyphs.last().unwrap().max_x();

                out_rows.push(PlacedRow {
                    pos: pos2(0.0, f32::NAN),
                    row: Arc::new(Row {
                        section_index_at_start,
                        glyphs,
                        visuals: Default::default(),
                        size: vec2(paragraph_max_x, 0.0),
                    }),
                    ends_with_newline: false,
                });

                // Start a new row:
                wrap_width = job.effective_wrap_width() - job.wrap.indent_rest;
                row_start_idx = last_kept_index + 1;
                row_start_x = paragraph.glyphs[row_start_idx].pos.x;
                row_break_candidates.forget_before_idx(row_start_idx);
                row_spaces = paragraph.glyphs[row_start_idx..i].iter().filter(|g| g.chr == ' ').map(|g| g.advance_width).sum();
            } else {
                // Found no place to break, so we have to overrun wrap_width.
            }
        }

        // ApexDesk patch: no break inside text kept together.
        let glued = !job.wrap.keep_together.is_empty()
            && paragraph.glyphs.get(i + 1).is_some_and(|n| {
                let (a, b) = (paragraph.glyphs[i].section_index, n.section_index);
                job.wrap.keep_together.iter().any(|r| r.contains(&a) && r.contains(&b))
            });
        if !glued {
            row_break_candidates.add(i, &paragraph.glyphs[i..], job.wrap.kinsoku);
        }
        if paragraph.glyphs[i].chr == ' ' {
            row_spaces += paragraph.glyphs[i].advance_width;
        }
    }

    if row_start_idx < paragraph.glyphs.len() {
        // Final row of text:

        if job.wrap.max_rows <= out_rows.len() {
            *elided = true; // can't fit another row
        } else {
            let paragraph_min_x = paragraph.glyphs[row_start_idx].pos.x - row_start_x;
            let paragraph_max_x = paragraph.glyphs.last().unwrap().max_x() - row_start_x;

            let glyphs: Vec<Glyph> = paragraph.glyphs[row_start_idx..]
                .iter()
                .copied()
                .map(|mut glyph| {
                    glyph.pos.x -= row_start_x + paragraph_min_x;
                    glyph
                })
                .collect();

            let section_index_at_start = glyphs[0].section_index;

            out_rows.push(PlacedRow {
                pos: pos2(paragraph_min_x, 0.0),
                row: Arc::new(Row {
                    section_index_at_start,
                    glyphs,
                    visuals: Default::default(),
                    size: vec2(paragraph_max_x - paragraph_min_x, 0.0),
                }),
                ends_with_newline: false,
            });
        }
    }
}

/// Trims the last glyphs in the row and replaces it with an overflow character (e.g. `…`).
///
/// Called before we have any Y coordinates.
fn replace_last_glyph_with_overflow_character(
    fonts: &mut FontsImpl,
    pixels_per_point: f32,
    job: &LayoutJob,
    row: &mut Row,
) {
    let Some(overflow_character) = job.wrap.overflow_character else {
        return;
    };

    let mut section_index = row
        .glyphs
        .last()
        .map(|g| g.section_index)
        .unwrap_or(row.section_index_at_start);
    loop {
        let section = &job.sections[section_index as usize];
        let extra_letter_spacing = section.format.extra_letter_spacing;
        let mut font = fonts.font(&section.format.font_id.family);
        let font_size = section.format.font_id.size;

        let font_id = font.resolve_face(overflow_character);
        let font_face_metrics = font
            .fonts_by_id
            .get(&font_id)
            .map(|f| f.styled_metrics(pixels_per_point, font_size, &section.format.coords))
            .unwrap_or_default();
        let (_, glyph_info) = font.glyph_info(overflow_character, &font_face_metrics);
        let mut font_face = font.fonts_by_id.get_mut(&font_id);

        let overflow_glyph_x = if let Some(prev_glyph) = row.glyphs.last() {
            prev_glyph.max_x() + extra_letter_spacing
        } else {
            0.0 // TODO(emilk): heed paragraph leading_space 😬
        };

        let advance_width_px =
            glyph_info.advance_width_unscaled.0 * font_face_metrics.px_scale_factor;
        let replacement_glyph_width = advance_width_px / pixels_per_point;

        // Check if we're within width budget:
        if overflow_glyph_x + replacement_glyph_width <= job.effective_wrap_width()
            || row.glyphs.is_empty()
        {
            // we are done

            let (replacement_glyph_alloc, physical_x) = font_face
                .as_mut()
                .map(|f| {
                    f.allocate_glyph(
                        font.atlas,
                        &font_face_metrics,
                        &ShapedGlyph {
                            glyph_id: glyph_info.id.unwrap_or(skrifa::GlyphId::NOTDEF),
                            h_pos: overflow_glyph_x * pixels_per_point,
                            is_cjk: is_cjk(overflow_character),
                        },
                    )
                })
                .unwrap_or_default();

            let font_metrics =
                font.styled_metrics(pixels_per_point, font_size, &section.format.coords);
            let line_height = section
                .format
                .line_height
                .unwrap_or(font_metrics.row_height);

            row.glyphs.push(Glyph {
                chr: overflow_character,
                pos: pos2(physical_x as f32 / pixels_per_point, f32::NAN),
                advance_width: advance_width_px / pixels_per_point,
                line_height,
                font_face_height: font_face_metrics.row_height,
                font_face_ascent: font_face_metrics.ascent,
                font_height: font_metrics.row_height,
                font_ascent: font_metrics.ascent,
                uv_rect: replacement_glyph_alloc.uv_rect,
                shaped: None,
                section_index,
                first_vertex: 0, // filled in later
            });
            return;
        }

        // We didn't fit - pop the last glyph and try again.
        if let Some(last_glyph) = row.glyphs.pop() {
            section_index = last_glyph.section_index;
        } else {
            section_index = row.section_index_at_start;
        }
    }
}

/// Horizontally aligned the text on a row.
///
/// Ignores the Y coordinate.
fn halign_and_justify_row(
    point_scale: PointScale,
    placed_row: &mut PlacedRow,
    halign: Align,
    wrap_width: f32,
    mut justify: bool,
    shrink_only: bool,
    keep_trailing_whitespace: bool,
    first_row: bool,
) {
    #![expect(clippy::useless_let_if_seq)] // False positive

    let row = Arc::make_mut(&mut placed_row.row);

    if row.glyphs.is_empty() {
        return;
    }

    let num_leading_spaces = row
        .glyphs
        .iter()
        .take_while(|glyph| glyph.chr.is_whitespace())
        .count();

    let glyph_range = if num_leading_spaces == row.glyphs.len() {
        // There is only whitespace
        (0, row.glyphs.len())
    } else if keep_trailing_whitespace {
        (num_leading_spaces, row.glyphs.len())
    } else {
        let num_trailing_spaces = row
            .glyphs
            .iter()
            .rev()
            .take_while(|glyph| glyph.chr.is_whitespace())
            .count();

        (num_leading_spaces, row.glyphs.len() - num_trailing_spaces)
    };
    let num_glyphs_in_range = glyph_range.1 - glyph_range.0;
    assert!(num_glyphs_in_range > 0, "Should have at least one glyph");

    let original_min_x = row.glyphs[glyph_range.0].logical_rect().min.x;
    let original_max_x = row.glyphs[glyph_range.1 - 1].logical_rect().max.x;
    let original_width = original_max_x - original_min_x;
    if shrink_only && original_max_x <= wrap_width {
        justify = false;
    }

    // ApexDesk patch: a justified row keeps the leading space of its first
    // section (a first-line indent), and the first row its leading spaces,
    // unstretched; stock epaint moved them to the margin.
    let lead = if justify && halign == Align::LEFT && (glyph_range.0 == 0 || first_row) { original_min_x.clamp(0.0, wrap_width) } else { 0.0 };
    let target_width = if justify && num_glyphs_in_range > 1 {
        wrap_width - lead
    } else {
        original_width
    };

    let (target_min_x, target_max_x) = match halign {
        Align::LEFT => (lead, lead + target_width),
        Align::Center => (-target_width / 2.0, target_width / 2.0),
        Align::RIGHT => (-target_width, 0.0),
    };

    let num_spaces_in_range = row.glyphs[glyph_range.0..glyph_range.1]
        .iter()
        .filter(|glyph| glyph.chr.is_whitespace())
        .count();

    let mut extra_x_per_glyph = if num_glyphs_in_range == 1 {
        0.0
    } else {
        (target_width - original_width) / (num_glyphs_in_range as f32 - 1.0)
    };
    extra_x_per_glyph = extra_x_per_glyph.at_least(0.0); // Don't contract

    let mut extra_x_per_space = 0.0;
    if 0 < num_spaces_in_range && num_spaces_in_range < num_glyphs_in_range {
        // ApexDesk patch: all the space goes to the spaces, as in word
        // processors (stock epaint put whole pixels between letters too,
        // which spread the letters of short rows).
        extra_x_per_glyph = 0.0;
        extra_x_per_space = (target_width - original_width) / (num_spaces_in_range as f32);
    }

    placed_row.pos.x = point_scale.round_to_pixel(target_min_x);
    let mut translate_x = -original_min_x - extra_x_per_glyph * glyph_range.0 as f32;

    for (i, glyph) in row.glyphs.iter_mut().enumerate() {
        glyph.pos.x += translate_x;
        glyph.pos.x = point_scale.round_to_pixel(glyph.pos.x);
        translate_x += extra_x_per_glyph;
        if glyph.chr.is_whitespace() && i >= glyph_range.0 {
            translate_x += extra_x_per_space;
        }
    }

    // Note we ignore the leading/trailing whitespace here!
    row.size.x = target_max_x - target_min_x;
}

/// Calculate the Y positions and tessellate the text.
fn galley_from_rows(
    point_scale: PointScale,
    job: Arc<LayoutJob>,
    mut rows: Vec<PlacedRow>,
    elided: bool,
    intrinsic_size: Vec2,
) -> Galley {
    let mut first_row_min_height = job.first_row_min_height;
    let mut cursor_y = 0.0;

    for placed_row in &mut rows {
        let mut max_row_height = first_row_min_height.at_least(placed_row.height());
        let row = Arc::make_mut(&mut placed_row.row);

        first_row_min_height = 0.0;
        // Spaces beside other text don't make the row taller (as in Word).
        let solid = row.glyphs.iter().any(|g| g.chr != ' ');
        for glyph in row.glyphs.iter().filter(|g| !solid || g.chr != ' ') {
            max_row_height = max_row_height.at_least(glyph.line_height);
        }
        // Unrounded jobs (document pages) keep exact row heights so that
        // pagination doesn't depend on the zoom; rows are snapped to pixels
        // when tessellated.
        if job.round_output_to_gui {
            max_row_height = point_scale.round_to_pixel(max_row_height);
        }

        // Now position each glyph vertically:
        for glyph in &mut row.glyphs {
            let format = &job.sections[glyph.section_index as usize].format;

            glyph.pos.y = glyph.font_face_ascent

                // Apply valign to the different in height of the entire row, and the height of this `Font`:
                + format.valign.to_factor() * (max_row_height - glyph.line_height)

                // When mixing different `FontImpl` (e.g. latin and emojis),
                // we always center the difference:
                + 0.5 * (glyph.font_height - glyph.font_face_height);

            glyph.pos.y = point_scale.round_to_pixel(glyph.pos.y);
        }

        placed_row.pos.y = cursor_y;
        row.size.y = max_row_height;

        cursor_y += max_row_height;
        if job.round_output_to_gui {
            cursor_y = point_scale.round_to_pixel(cursor_y);
        } // TODO(emilk): it would be better to do the calculations in pixels instead.
    }

    let format_summary = format_summary(&job);

    let mut rect = Rect::ZERO;
    let mut mesh_bounds = Rect::NOTHING;
    let mut num_vertices = 0;
    let mut num_indices = 0;

    for placed_row in &mut rows {
        rect |= placed_row.rect();

        let row = Arc::make_mut(&mut placed_row.row);
        row.visuals = tessellate_row(point_scale, &job, &format_summary, row);

        mesh_bounds |= row.visuals.mesh_bounds.translate(placed_row.pos.to_vec2());
        num_vertices += row.visuals.mesh.vertices.len();
        num_indices += row.visuals.mesh.indices.len();

        row.section_index_at_start = u32::MAX; // No longer in use.
        for glyph in &mut row.glyphs {
            glyph.section_index = u32::MAX; // No longer in use.
        }
    }

    let mut galley = Galley {
        job,
        rows,
        elided,
        rect,
        mesh_bounds,
        num_vertices,
        num_indices,
        pixels_per_point: point_scale.pixels_per_point,
        intrinsic_size,
    };

    if galley.job.round_output_to_gui {
        galley.round_output_to_gui();
    }

    galley
}

#[derive(Default)]
struct FormatSummary {
    any_background: bool,
    any_underline: bool,
    any_strikethrough: bool,
}

fn format_summary(job: &LayoutJob) -> FormatSummary {
    let mut format_summary = FormatSummary::default();
    for section in &job.sections {
        format_summary.any_background |= section.format.background != Color32::TRANSPARENT;
        format_summary.any_underline |= section.format.underline != Stroke::NONE;
        format_summary.any_strikethrough |= section.format.strikethrough != Stroke::NONE;
    }
    format_summary
}

fn tessellate_row(
    point_scale: PointScale,
    job: &LayoutJob,
    format_summary: &FormatSummary,
    row: &mut Row,
) -> RowVisuals {
    if row.glyphs.is_empty() {
        return Default::default();
    }

    let mut mesh = Mesh::default();

    mesh.reserve_triangles(row.glyphs.len() * 2);
    mesh.reserve_vertices(row.glyphs.len() * 4);

    if format_summary.any_background {
        add_row_backgrounds(point_scale, job, row, &mut mesh);
    }

    let glyph_index_start = mesh.indices.len();
    let glyph_vertex_start = mesh.vertices.len();
    tessellate_glyphs(point_scale, job, row, &mut mesh);
    let glyph_vertex_end = mesh.vertices.len();

    if format_summary.any_underline {
        add_row_hline(point_scale, row, &mut mesh, |glyph| {
            let format = &job.sections[glyph.section_index as usize].format;
            let stroke = format.underline;
            let y = glyph.underline_y();
            (stroke, y)
        });
    }

    if format_summary.any_strikethrough {
        add_row_hline(point_scale, row, &mut mesh, |glyph| {
            let format = &job.sections[glyph.section_index as usize].format;
            let stroke = format.strikethrough;
            let y = glyph.logical_rect().center().y;
            (stroke, y)
        });
    }

    let mesh_bounds = mesh.calc_bounds();

    RowVisuals {
        mesh,
        mesh_bounds,
        glyph_index_start,
        glyph_vertex_range: glyph_vertex_start..glyph_vertex_end,
    }
}

/// Create background for glyphs that have them.
/// Creates as few rectangular regions as possible.
fn add_row_backgrounds(point_scale: PointScale, job: &LayoutJob, row: &Row, mesh: &mut Mesh) {
    if row.glyphs.is_empty() {
        return;
    }

    let mut end_run = |start: Option<(Color32, Rect, f32)>, stop_x: f32| {
        if let Some((color, start_rect, expand)) = start {
            let rect = Rect::from_min_max(start_rect.left_top(), pos2(stop_x, start_rect.bottom()));
            let rect = rect.expand(expand);
            let rect = rect.round_to_pixels(point_scale.pixels_per_point());
            mesh.add_colored_rect(rect, color);
        }
    };

    let mut run_start = None;
    let mut last_rect = Rect::NAN;

    for glyph in &row.glyphs {
        let format = &job.sections[glyph.section_index as usize].format;
        let color = format.background;
        let rect = glyph.logical_rect();

        if color == Color32::TRANSPARENT {
            end_run(run_start.take(), last_rect.right());
        } else if let Some((existing_color, start, expand)) = run_start {
            if existing_color == color
                && start.top() == rect.top()
                && start.bottom() == rect.bottom()
                && format.expand_bg == expand
            {
                // continue the same background rectangle
            } else {
                end_run(run_start.take(), last_rect.right());
                run_start = Some((color, rect, format.expand_bg));
            }
        } else {
            run_start = Some((color, rect, format.expand_bg));
        }

        last_rect = rect;
    }

    end_run(run_start.take(), last_rect.right());
}

fn tessellate_glyphs(point_scale: PointScale, job: &LayoutJob, row: &mut Row, mesh: &mut Mesh) {
    for glyph in &mut row.glyphs {
        glyph.first_vertex = mesh.vertices.len() as u32;
        let uv_rect = glyph.uv_rect;
        if !uv_rect.is_nothing() {
            let mut left_top = glyph.pos + uv_rect.offset;
            left_top.x = point_scale.round_to_pixel(left_top.x);
            left_top.y = point_scale.round_to_pixel(left_top.y);

            let rect = Rect::from_min_max(left_top, left_top + uv_rect.size);
            let uv = Rect::from_min_max(
                pos2(uv_rect.min[0] as f32, uv_rect.min[1] as f32),
                pos2(uv_rect.max[0] as f32, uv_rect.max[1] as f32),
            );

            let format = &job.sections[glyph.section_index as usize].format;

            let color = format.color;

            if format.italics {
                let idx = mesh.vertices.len() as u32;
                mesh.add_triangle(idx, idx + 1, idx + 2);
                mesh.add_triangle(idx + 2, idx + 1, idx + 3);

                let top_offset = rect.height() * 0.25 * Vec2::X;

                mesh.vertices.push(Vertex {
                    pos: rect.left_top() + top_offset,
                    uv: uv.left_top(),
                    color,
                });
                mesh.vertices.push(Vertex {
                    pos: rect.right_top() + top_offset,
                    uv: uv.right_top(),
                    color,
                });
                mesh.vertices.push(Vertex {
                    pos: rect.left_bottom(),
                    uv: uv.left_bottom(),
                    color,
                });
                mesh.vertices.push(Vertex {
                    pos: rect.right_bottom(),
                    uv: uv.right_bottom(),
                    color,
                });
            } else if job.vertical && super::kinsoku::upright_in_vertical(glyph.chr) {
                // ApexDesk patch: turned back a quarter about the middle of
                // its em box, to stand upright in the turned galley.
                let size = format.font_id.size;
                let c = pos2(glyph.pos.x + glyph.advance_width / 2.0, glyph.pos.y - super::kinsoku::EM_BOX_MIDDLE * size);
                let turn = |p: Pos2| pos2(c.x + (p.y - c.y), c.y - (p.x - c.x));
                let idx = mesh.vertices.len() as u32;
                mesh.add_triangle(idx, idx + 1, idx + 2);
                mesh.add_triangle(idx + 2, idx + 1, idx + 3);
                for (p, t) in [(rect.left_top(), uv.left_top()), (rect.right_top(), uv.right_top()), (rect.left_bottom(), uv.left_bottom()), (rect.right_bottom(), uv.right_bottom())] {
                    mesh.vertices.push(Vertex { pos: turn(p), uv: t, color });
                }
            } else {
                mesh.add_rect_with_uv(rect, uv, color);
            }
        }
    }
}

/// Add a horizontal line over a row of glyphs with a stroke and y decided by a callback.
fn add_row_hline(
    point_scale: PointScale,
    row: &Row,
    mesh: &mut Mesh,
    stroke_and_y: impl Fn(&Glyph) -> (Stroke, f32),
) {
    let mut path = crate::tessellator::Path::default(); // reusing path to avoid re-allocations.

    let mut end_line = |start: Option<(Stroke, Pos2)>, stop_x: f32| {
        if let Some((stroke, start)) = start {
            let stop = pos2(stop_x, start.y);
            path.clear();
            path.add_line_segment([start, stop]);
            let feathering = 1.0 / point_scale.pixels_per_point();
            path.stroke_open(feathering, &PathStroke::from(stroke), mesh);
        }
    };

    let mut line_start = None;
    let mut last_right_x = f32::NAN;

    for glyph in &row.glyphs {
        let (stroke, mut y) = stroke_and_y(glyph);
        stroke.round_center_to_pixel(point_scale.pixels_per_point, &mut y);

        if stroke.is_empty() {
            end_line(line_start.take(), last_right_x);
        } else if let Some((existing_stroke, start)) = line_start {
            if existing_stroke == stroke && start.y == y {
                // continue the same line
            } else {
                end_line(line_start.take(), last_right_x);
                line_start = Some((stroke, pos2(glyph.pos.x, y)));
            }
        } else {
            line_start = Some((stroke, pos2(glyph.pos.x, y)));
        }

        last_right_x = glyph.max_x();
    }

    end_line(line_start.take(), last_right_x);
}

// ----------------------------------------------------------------------------

/// Keeps track of good places to break a long row of text.
/// Will focus primarily on spaces, secondarily on things like `-`
#[derive(Clone, Copy, Default)]
struct RowBreakCandidates {
    /// Breaking at ` ` or other whitespace
    /// is always the primary candidate.
    space: Option<usize>,

    /// Logograms (single character representing a whole word) or kana (Japanese hiragana and katakana) are good candidates for line break.
    cjk: Option<usize>,

    /// Breaking anywhere before a CJK character is acceptable too.
    pre_cjk: Option<usize>,

    /// Breaking at a dash is a super-
    /// good idea.
    dash: Option<usize>,

    /// This is nicer for things like URLs, e.g. www.
    /// example.com.
    punctuation: Option<usize>,

    /// Breaking after just random character is some
    /// times necessary.
    any: Option<usize>,

    /// ApexDesk patch: `any` that keeps the kinsoku rules.
    any_kinsoku: Option<usize>,
}

impl RowBreakCandidates {
    fn add(&mut self, index: usize, glyphs: &[Glyph], kinsoku: bool) {
        let chr = glyphs[0].chr;
        let next = glyphs.get(1).map(|g| g.chr);
        const NON_BREAKING_SPACE: char = '\u{A0}';
        // ApexDesk patch: kinsoku rules between any East Asian characters.
        let rules_allow = !kinsoku || next.is_none_or(|n| super::kinsoku::allowed(chr, n));
        if rules_allow {
            self.any_kinsoku = Some(index);
        }
        if chr.is_whitespace() && chr != NON_BREAKING_SPACE {
            self.space = Some(index);
        } else if next.is_some_and(|n| super::kinsoku::breaks_freely(chr) || super::kinsoku::breaks_freely(n)) {
            if rules_allow {
                self.cjk = Some(index);
            }
        } else if is_cjk(chr) && (glyphs.len() == 1 || is_cjk_break_allowed(glyphs[1].chr)) {
            self.cjk = Some(index);
        } else if chr == '-' {
            self.dash = Some(index);
        } else if chr.is_ascii_punctuation() {
            self.punctuation = Some(index);
        } else if glyphs.len() > 1 && is_cjk(glyphs[1].chr) {
            self.pre_cjk = Some(index);
        }
        self.any = Some(index);
    }

    fn word_boundary(&self) -> Option<usize> {
        [self.space, self.cjk, self.pre_cjk]
            .into_iter()
            .max()
            .flatten()
    }

    fn has_good_candidate(&self, break_anywhere: bool) -> bool {
        if break_anywhere {
            self.any.is_some()
        } else {
            self.word_boundary().is_some()
        }
    }

    fn get(&self, break_anywhere: bool) -> Option<usize> {
        if break_anywhere {
            self.any_kinsoku.or(self.any)
        } else {
            self.word_boundary()
                .or(self.dash)
                .or(self.punctuation)
                .or(self.any)
        }
    }

    fn forget_before_idx(&mut self, index: usize) {
        let Self {
            space,
            cjk,
            pre_cjk,
            dash,
            punctuation,
            any,
            any_kinsoku,
        } = self;
        if space.is_some_and(|s| s < index) {
            *space = None;
        }
        if cjk.is_some_and(|s| s < index) {
            *cjk = None;
        }
        if pre_cjk.is_some_and(|s| s < index) {
            *pre_cjk = None;
        }
        if dash.is_some_and(|s| s < index) {
            *dash = None;
        }
        if punctuation.is_some_and(|s| s < index) {
            *punctuation = None;
        }
        if any.is_some_and(|s| s < index) {
            *any = None;
        }
        if any_kinsoku.is_some_and(|s| s < index) {
            *any_kinsoku = None;
        }
    }
}

// ----------------------------------------------------------------------------

/// Segment text into runs where each run uses a single font face.
///
/// Grapheme clusters are never split across runs: if a combining mark
/// falls back to a different font than its base character, it stays
/// with the base character's font (the shaper will handle it).
///
/// NOTE: Segmentation is by font face, not by Unicode script. A run may
/// mix scripts (e.g. Latin + Cyrillic) when they share the same font.
/// This is acceptable for scripts with similar shaping rules, but would
/// need script-aware splitting once RTL/bidi support is added.
///
/// Results are appended to `out` (which is cleared first) to allow
/// the caller to reuse the allocation across calls.
fn segment_into_runs(font: &mut Font<'_>, text: &str, out: &mut Vec<TextRun>) {
    use unicode_segmentation::UnicodeSegmentation as _;

    out.clear();

    for (byte_offset, grapheme_str) in text.grapheme_indices(true) {
        let byte_offset = ByteIndex(byte_offset);
        let byte_end = byte_offset + grapheme_str.len();

        let base_char = grapheme_str.chars().next().unwrap_or(' ');
        let font_key = font.resolve_face(base_char);

        if let Some(last_run) = out.last_mut()
            && last_run.font_key == font_key
        {
            last_run.byte_range.end = byte_end;
            continue;
        }
        out.push(TextRun {
            font_key,
            byte_range: byte_offset..byte_end,
        });
    }
}

/// Shape a text run and return the raw [`harfrust::GlyphBuffer`].
///
/// The caller should iterate `glyph_infos()` / `glyph_positions()` (both
/// `Copy` slices) and convert font units to pixels using `metrics.px_scale_factor`.
/// After iteration, recycle the buffer via `glyph_buffer.clear()`.
fn shape_text(
    font_face: &FontFace,
    text: &str,
    coords: &VariationCoords,
    mut buffer: harfrust::UnicodeBuffer,
    flags: harfrust::BufferFlags,
) -> (harfrust::GlyphBuffer, bool) {
    let font_ref = font_face.skrifa_font_ref();
    let tweak = font_face.tweak();

    // Build shaper with variable font instance if variation coordinates are set.
    let variations: Vec<harfrust::Variation> = iter::chain(tweak.coords.as_ref(), coords.as_ref())
        .map(|&(tag, value)| harfrust::Variation { tag, value })
        .collect();

    let instance = if variations.is_empty() {
        None
    } else {
        Some(harfrust::ShaperInstance::from_variations(
            font_ref, variations,
        ))
    };

    let shaper = font_face
        .shaper_data()
        .shaper(font_ref)
        .instance(instance.as_ref())
        .build();

    buffer.set_flags(flags);
    buffer.push_str(text);
    buffer.guess_segment_properties();
    let rtl = buffer.direction() == harfrust::Direction::RightToLeft;

    (shaper.shape(buffer, harfrust::ShapeOptions::new()), rtl)
}

/// ApexDesk patch: lays a row's right-to-left text out right to left
/// (the Unicode bidirectional algorithm, `rtl` for a right-to-left
/// paragraph). The glyphs keep their order and the room each takes;
/// spaces at the end of the row stay there.
fn bidi_row(row: &mut Row, rtl: bool) {
    use unicode_bidi::{BidiInfo, Level};
    let n = row.glyphs.iter().rposition(|g| !g.chr.is_whitespace()).map_or(0, |i| i + 1);
    let is_rtl = |c: char| matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF);
    if n < 2 || !rtl && !row.glyphs[..n].iter().any(|g| is_rtl(g.chr)) {
        return;
    }
    let text: String = row.glyphs[..n].iter().map(|g| g.chr).collect();
    let info = BidiInfo::new(&text, Some(if rtl { Level::rtl() } else { Level::ltr() }));
    let levels: Vec<Level> = text.char_indices().map(|(i, _)| info.levels[i]).collect();
    if levels.iter().all(|l| l.is_ltr()) {
        return;
    }
    let order = BidiInfo::reorder_visual(&levels);
    let room: Vec<f32> = (0..n).map(|i| if i + 1 < n { row.glyphs[i + 1].pos.x - row.glyphs[i].pos.x } else { row.glyphs[i].advance_width }).collect();
    let mut x = row.glyphs[0].pos.x;
    for i in order {
        row.glyphs[i].pos.x = x;
        x += room[i];
    }
}

// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use core::iter;

    use super::{super::*, *};
    use crate::text::cursor::CCursor;

    #[test]
    fn test_zero_max_width() {
        let pixels_per_point = 1.0;
        let mut fonts = FontsImpl::new(TextOptions::default(), FontDefinitions::default());
        let mut layout_job = LayoutJob::single_section("W".into(), TextFormat::default());
        layout_job.wrap.max_width = 0.0;
        let galley = layout(&mut fonts, pixels_per_point, layout_job.into());
        assert_eq!(galley.rows.len(), 1);
    }

    #[test]
    fn test_truncate_with_newline() {
        // No matter where we wrap, we should be appending the newline character.

        let pixels_per_point = 1.0;

        let mut fonts = FontsImpl::new(TextOptions::default(), FontDefinitions::default());
        let text_format = TextFormat {
            font_id: FontId::monospace(12.0),
            ..Default::default()
        };

        for text in ["Hello\nworld", "\nfoo"] {
            for break_anywhere in [false, true] {
                for max_width in [0.0, 5.0, 10.0, 20.0, f32::INFINITY] {
                    let mut layout_job =
                        LayoutJob::single_section(text.into(), text_format.clone());
                    layout_job.wrap.max_width = max_width;
                    layout_job.wrap.max_rows = 1;
                    layout_job.wrap.break_anywhere = break_anywhere;

                    let galley = layout(&mut fonts, pixels_per_point, layout_job.into());

                    assert!(galley.elided);
                    assert_eq!(galley.rows.len(), 1);
                    let row_text = galley.rows[0].text();
                    assert!(
                        row_text.ends_with('…'),
                        "Expected row to end with `…`, got {row_text:?} when line-breaking the text {text:?} with max_width {max_width} and break_anywhere {break_anywhere}.",
                    );
                }
            }
        }

        {
            let mut layout_job = LayoutJob::single_section("Hello\nworld".into(), text_format);
            layout_job.wrap.max_width = 50.0;
            layout_job.wrap.max_rows = 1;
            layout_job.wrap.break_anywhere = false;

            let galley = layout(&mut fonts, pixels_per_point, layout_job.into());

            assert!(galley.elided);
            assert_eq!(galley.rows.len(), 1);
            let row_text = galley.rows[0].text();
            assert_eq!(row_text, "Hello…");
        }
    }

    #[test]
    fn test_cjk() {
        let pixels_per_point = 1.0;
        let mut fonts = FontsImpl::new(TextOptions::default(), FontDefinitions::default());
        let mut layout_job = LayoutJob::single_section(
            "日本語とEnglishの混在した文章".into(),
            TextFormat::default(),
        );
        layout_job.wrap.max_width = 90.0;
        let galley = layout(&mut fonts, pixels_per_point, layout_job.into());
        assert_eq!(
            galley.rows.iter().map(|row| row.text()).collect::<Vec<_>>(),
            vec!["日本語と", "Englishの混在", "した文章"]
        );
    }

    #[test]
    fn test_pre_cjk() {
        let pixels_per_point = 1.0;
        let mut fonts = FontsImpl::new(TextOptions::default(), FontDefinitions::default());
        let mut layout_job = LayoutJob::single_section(
            "日本語とEnglishの混在した文章".into(),
            TextFormat::default(),
        );
        layout_job.wrap.max_width = 110.0;
        let galley = layout(&mut fonts, pixels_per_point, layout_job.into());
        assert_eq!(
            galley.rows.iter().map(|row| row.text()).collect::<Vec<_>>(),
            vec!["日本語とEnglish", "の混在した文章"]
        );
    }

    #[test]
    fn test_truncate_width() {
        let pixels_per_point = 1.0;
        let mut fonts = FontsImpl::new(TextOptions::default(), FontDefinitions::default());
        let mut layout_job =
            LayoutJob::single_section("# DNA\nMore text".into(), TextFormat::default());
        layout_job.wrap.max_width = f32::INFINITY;
        layout_job.wrap.max_rows = 1;
        layout_job.round_output_to_gui = false;
        let galley = layout(&mut fonts, pixels_per_point, layout_job.into());
        assert!(galley.elided);
        assert_eq!(
            galley.rows.iter().map(|row| row.text()).collect::<Vec<_>>(),
            vec!["# DNA…"]
        );
        let row = &galley.rows[0];
        assert_eq!(row.pos, Pos2::ZERO);
        assert_eq!(row.rect().max.x, row.glyphs.last().unwrap().max_x());
    }

    #[test]
    fn test_truncate_with_pixels_per_point() {
        let mut fonts = FontsImpl::new(TextOptions::default(), FontDefinitions::default());

        for pixels_per_point in [
            0.33, 0.5, 0.67, 1.0, 1.25, 1.33, 1.5, 1.75, 2.0, 3.0, 4.0, 5.0,
        ] {
            for ch in ['W', 'A', 'n', 't', 'i'] {
                let target_width = 50.0;
                let text = (0..20).map(|_| ch).collect::<String>();

                let mut job = LayoutJob::single_section(text, TextFormat::default());
                job.wrap.max_width = target_width;
                job.wrap.max_rows = 1;
                let elided_galley = layout(&mut fonts, pixels_per_point, job.into());
                assert!(elided_galley.elided);

                let test_galley = layout(
                    &mut fonts,
                    pixels_per_point,
                    Arc::new(LayoutJob::single_section(
                        iter::chain(
                            (0..elided_galley.rows[0].char_count_excluding_newline().0).map(|_| ch),
                            iter::once('…'),
                        )
                        .collect::<String>(),
                        TextFormat::default(),
                    )),
                );

                assert!(elided_galley.size().x >= 0.0);
                assert!(elided_galley.size().x <= target_width);
                assert!(test_galley.size().x > target_width);
            }
        }
    }

    #[test]
    fn test_empty_row() {
        let pixels_per_point = 1.0;
        let mut fonts = FontsImpl::new(TextOptions::default(), FontDefinitions::default());

        let font_id = FontId::default();
        let font_height = fonts
            .font(&font_id.family)
            .styled_metrics(pixels_per_point, font_id.size, &VariationCoords::default())
            .row_height;

        let job = LayoutJob::simple(String::new(), font_id, Color32::WHITE, f32::INFINITY);

        let galley = layout(&mut fonts, pixels_per_point, job.into());

        assert_eq!(galley.rows.len(), 1, "Expected one row");
        assert_eq!(
            galley.rows[0].row.glyphs.len(),
            0,
            "Expected no glyphs in the empty row"
        );
        assert_eq!(
            galley.size(),
            Vec2::new(0.0, font_height.round()),
            "Unexpected galley size"
        );
        assert_eq!(
            galley.intrinsic_size(),
            Vec2::new(0.0, font_height.round()),
            "Unexpected intrinsic size"
        );
    }

    #[test]
    fn test_end_with_newline() {
        let pixels_per_point = 1.0;
        let mut fonts = FontsImpl::new(TextOptions::default(), FontDefinitions::default());

        let font_id = FontId::default();
        let font_height = fonts
            .font(&font_id.family)
            .styled_metrics(pixels_per_point, font_id.size, &VariationCoords::default())
            .row_height;

        let job = LayoutJob::simple("Hi!\n".to_owned(), font_id, Color32::WHITE, f32::INFINITY);

        let galley = layout(&mut fonts, pixels_per_point, job.into());

        assert_eq!(galley.rows.len(), 2, "Expected two rows");
        assert_eq!(
            galley.rows[1].row.glyphs.len(),
            0,
            "Expected no glyphs in the empty row"
        );
        assert_eq!(
            galley.size().round(),
            Vec2::new(17.0, font_height.round() * 2.0),
            "Unexpected galley size"
        );
        assert_eq!(
            galley.intrinsic_size().round(),
            Vec2::new(17.0, font_height.round() * 2.0),
            "Unexpected intrinsic size"
        );
    }

    #[test]
    fn test_combining_diacritics() {
        // ɔ̃ = U+0254 (LATIN SMALL LETTER OPEN O) + U+0303 (COMBINING TILDE)
        // With text shaping, the combining tilde should NOT produce a separate
        // advance — it should be positioned above ɔ via GPOS anchors.
        // Note: the default fonts don't contain U+0254, so the replacement glyph
        // is used. The key test is that the combining mark does NOT add extra width.
        let pixels_per_point = 1.0;
        let mut fonts = FontsImpl::new(TextOptions::default(), FontDefinitions::default());

        let job_combined = LayoutJob::simple(
            "ɔ\u{0303}".to_owned(),
            FontId::proportional(14.0),
            Color32::WHITE,
            f32::INFINITY,
        );
        let galley_combined = layout(&mut fonts, pixels_per_point, job_combined.into());

        let job_base = LayoutJob::simple(
            "ɔ".to_owned(),
            FontId::proportional(14.0),
            Color32::WHITE,
            f32::INFINITY,
        );
        let galley_base = layout(&mut fonts, pixels_per_point, job_base.into());

        let width_combined = galley_combined.size().x;
        let width_base = galley_base.size().x;

        assert!(
            (width_combined - width_base).abs() < 2.0,
            "Combining diacritic should not add significant width. \
             Base width: {width_base}, Combined width: {width_combined}"
        );

        let glyphs = &galley_combined.rows[0].row.glyphs;
        assert!(!glyphs.is_empty(), "Expected at least 1 glyph for ɔ̃");
    }

    #[test]
    fn test_shaping_basic_latin() {
        // Basic test: shaped Latin text should produce the same number of glyphs as characters.
        let pixels_per_point = 1.0;
        let mut fonts = FontsImpl::new(TextOptions::default(), FontDefinitions::default());

        let job = LayoutJob::simple(
            "Hello".to_owned(),
            FontId::proportional(14.0),
            Color32::WHITE,
            f32::INFINITY,
        );
        let galley = layout(&mut fonts, pixels_per_point, job.into());

        assert_eq!(galley.rows.len(), 1);
        assert_eq!(galley.rows[0].row.glyphs.len(), 5);
        assert!(galley.size().x > 0.0);
    }

    #[test]
    fn test_shaping_empty_string() {
        let pixels_per_point = 1.0;
        let mut fonts = FontsImpl::new(TextOptions::default(), FontDefinitions::default());

        let job = LayoutJob::simple(
            String::new(),
            FontId::proportional(14.0),
            Color32::WHITE,
            f32::INFINITY,
        );
        let galley = layout(&mut fonts, pixels_per_point, job.into());

        assert_eq!(galley.rows.len(), 1);
        assert_eq!(galley.rows[0].row.glyphs.len(), 0);
    }

    #[test]
    fn test_shaping_multiple_newlines() {
        let pixels_per_point = 1.0;
        let mut fonts = FontsImpl::new(TextOptions::default(), FontDefinitions::default());

        let job = LayoutJob::simple(
            "A\n\nB".to_owned(),
            FontId::proportional(14.0),
            Color32::WHITE,
            f32::INFINITY,
        );
        let galley = layout(&mut fonts, pixels_per_point, job.into());

        assert_eq!(galley.rows.len(), 3, "Expected 3 rows for 'A\\n\\nB'");
        assert_eq!(galley.rows[0].row.glyphs.len(), 1); // "A"
        assert_eq!(galley.rows[1].row.glyphs.len(), 0); // empty line
        assert_eq!(galley.rows[2].row.glyphs.len(), 1); // "B"
    }

    #[test]
    fn test_shaping_mixed_font_fallback() {
        // Text with both Latin and emoji should work without panicking,
        // even though they use different font faces.
        let pixels_per_point = 1.0;
        let mut fonts = FontsImpl::new(TextOptions::default(), FontDefinitions::default());

        let job = LayoutJob::simple(
            "Hi 🎉 bye".to_owned(),
            FontId::proportional(14.0),
            Color32::WHITE,
            f32::INFINITY,
        );
        let galley = layout(&mut fonts, pixels_per_point, job.into());

        assert_eq!(galley.rows.len(), 1);
        // "Hi " (3) + "🎉" (1) + " bye" (4) = at least 8 glyphs
        assert!(
            galley.rows[0].row.glyphs.len() >= 8,
            "Expected >= 8 glyphs, got {}",
            galley.rows[0].row.glyphs.len()
        );
    }

    #[test]
    fn test_gpos_kerning() {
        // GPOS kerning: pairs like "AV", "VA", "AT" should be tighter than
        // the sum of individual character widths. Without text shaping, egui
        // only uses the legacy `kern` table, so these pairs had diff ≈ 0.
        // With harfrust, GPOS kerning applies proper negative adjustments.
        let pixels_per_point = 1.0;
        let mut fonts = FontsImpl::new(TextOptions::default(), FontDefinitions::default());
        let font_id = FontId::proportional(14.0);

        for pair in ["AV", "VA", "AT"] {
            let (pair_w, _, _) = measure_text(&mut fonts, pair, &font_id, pixels_per_point);
            let chars: Vec<char> = pair.chars().collect();
            let (w1, _, _) = measure_text(
                &mut fonts,
                &chars[0].to_string(),
                &font_id,
                pixels_per_point,
            );
            let (w2, _, _) = measure_text(
                &mut fonts,
                &chars[1].to_string(),
                &font_id,
                pixels_per_point,
            );
            let sum = w1 + w2;
            let kern_adjustment = sum - pair_w;

            assert!(
                kern_adjustment > 0.5,
                "GPOS kerning for '{pair}': expected pair to be noticeably tighter \
                 than sum of individuals. pair_width={pair_w:.2}, sum={sum:.2}, \
                 kern_adjustment={kern_adjustment:.2} (should be > 0.5)",
            );
        }
    }

    /// Regression test for <https://github.com/emilk/egui/issues/8087>.
    ///
    /// Multi-codepoint grapheme clusters (flag emojis, combining marks) must
    /// produce exactly as many glyphs as characters so that cursor positioning
    /// and text selection remain correct.
    #[test]
    fn test_grapheme_cluster_glyph_count() {
        let pixels_per_point = 1.0;
        let mut fonts = FontsImpl::new(TextOptions::default(), FontDefinitions::default());
        let font_id = FontId::default();

        // Each test case: (input text, expected char count)
        let cases: &[(&str, usize)] = &[
            // Flag emoji: two Regional Indicator codepoints → one visual glyph
            ("\u{1F1EF}\u{1F1F5}", 2), // 🇯🇵
            // Flag surrounded by ASCII
            ("A\u{1F1EB}\u{1F1F7}B", 4), // A🇫🇷B
            // Base char + combining acute accent
            ("e\u{0301}", 2), // é as decomposed
            // Multiple combining marks
            ("o\u{0302}\u{0323}", 3), // ộ
            // Plain ASCII (sanity check)
            ("Hello", 5),
        ];

        for &(text, expected_chars) in cases {
            let job = LayoutJob::simple(
                text.to_owned(),
                font_id.clone(),
                Color32::WHITE,
                f32::INFINITY,
            );
            let galley = layout(&mut fonts, pixels_per_point, job.into());

            let total_glyphs: usize = galley.rows.iter().map(|r| r.row.glyphs.len()).sum();

            assert_eq!(
                total_glyphs,
                expected_chars,
                "Glyph count mismatch for {text:?}: \
                 expected {expected_chars} glyphs (one per char), got {total_glyphs}. \
                 Glyphs: {:?}",
                galley.rows[0]
                    .row
                    .glyphs
                    .iter()
                    .map(|g| (g.chr, g.advance_width))
                    .collect::<Vec<_>>(),
            );

            // Verify that Row::text() reconstructs the input text.
            let row_text: String = galley.rows.iter().map(|r| r.text()).collect();
            assert_eq!(row_text, text, "Row::text() mismatch for {text:?}");

            // Verify cursor round-trip: end cursor index == char count.
            assert_eq!(
                galley.end().index.0,
                expected_chars,
                "Galley::end().index mismatch for {text:?}",
            );
        }
    }

    /// Verify that cursor positioning round-trips correctly for text
    /// containing multi-codepoint grapheme clusters (regression test for #8087).
    #[test]
    fn test_grapheme_cluster_cursor_roundtrip() {
        let pixels_per_point = 1.0;
        let mut fonts = FontsImpl::new(TextOptions::default(), FontDefinitions::default());
        let font_id = FontId::default();

        // "A" + flag emoji (2 codepoints) + "B" = 4 chars
        let text = "A\u{1F1EF}\u{1F1F5}B";
        let job = LayoutJob::simple(
            text.to_owned(),
            font_id.clone(),
            Color32::WHITE,
            f32::INFINITY,
        );
        let galley = layout(&mut fonts, pixels_per_point, job.into());

        // Walking through every cursor index should produce valid positions.
        for i in 0..=galley.end().index.0 {
            let cursor = CCursor {
                index: CharIndex(i),
                prefer_next_row: false,
            };
            let rect = galley.pos_from_cursor(cursor);
            assert!(
                rect.is_finite(),
                "pos_from_cursor returned non-finite rect for index {i}",
            );

            // Round-trip: position → cursor → position should be stable.
            let cursor2 = galley.cursor_from_pos(Vec2::new(rect.center().x, rect.center().y));
            let rect2 = galley.pos_from_cursor(cursor2);
            assert!(
                (rect.min.x - rect2.min.x).abs() < 1.0,
                "Cursor round-trip unstable at index {i}: \
                 first={}, second={}, cursor2.index={}",
                rect.min.x,
                rect2.min.x,
                cursor2.index,
            );
        }
    }

    fn measure_text(
        fonts: &mut FontsImpl,
        text: &str,
        font_id: &FontId,
        pixels_per_point: f32,
    ) -> (f32, usize, Vec<(char, f32)>) {
        let job = LayoutJob::simple(
            text.to_owned(),
            font_id.clone(),
            Color32::WHITE,
            f32::INFINITY,
        );
        let galley = layout(fonts, pixels_per_point, job.into());
        let glyphs = &galley.rows[0].row.glyphs;
        let details: Vec<_> = glyphs.iter().map(|g| (g.chr, g.advance_width)).collect();
        (galley.size().x, glyphs.len(), details)
    }
}
