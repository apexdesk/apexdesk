//! East Asian typography on screen: Chinese, Japanese and Korean
//! characters in the Asian font, a little space between them and Latin
//! letters or digits, the document grid's character and line pitch, and
//! the paragraph's line breaking rules (kinsoku, applied by the wrapping
//! of the vendored epaint).

use std::hash::{Hash, Hasher};

use apex_doc::{AsianPara, DocGrid, auto_space, uses_asian_font};
use eframe::egui::FontId;
use eframe::egui::epaint::text::{ByteIndex, LayoutJob, LayoutSection};

/// The text of one span of a paragraph: the job sections it was laid out
/// in and its Asian font.
pub(super) struct Span {
    pub sections: std::ops::Range<usize>,
    /// The Asian font at the size the text was appended with (None: the
    /// text has no Asian characters, or no CJK font is installed).
    pub font: Option<FontId>,
    /// Size of the text appended (effects like small capitals change the
    /// size of some sections: the Asian font follows them).
    pub size: f32,
    /// Word's `w:hint="eastAsia"`: characters shared with Latin text
    /// (quotes, dashes) take the Asian font.
    pub hint: bool,
    /// Single line spacing of the document's Asian font where `font`
    /// stands in for it (see `fonts::known_line`).
    pub line: Option<f32>,
}

/// What decides the layout of a paragraph's Asian text.
pub(super) struct Rules {
    pub para: AsianPara,
    pub grid: DocGrid,
    /// Size of the Normal style's font (points): the character pitch's base.
    pub base: f32,
    /// Pixels per point.
    pub pt: f32,
    /// The line spacing multiple, for paragraphs with multiple spacing
    /// (they snap to the line grid).
    pub multiple: Option<f32>,
    /// The least line height (pixels) of paragraphs with "at least"
    /// spacing (exact spacing: neither).
    pub at_least: Option<f32>,
}

/// Space between Asian and Latin text, as a share of the font size.
const AUTO_SPACE: f32 = 0.25;

/// Mixes the grid of a section into a paragraph's layout hash.
pub(super) fn grid_hash(g: &DocGrid) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (g.kind, g.line_pitch.to_bits(), g.char_space).hash(&mut h);
    h.finish()
}

/// Applies the Asian rules to a paragraph's job: Asian characters get the
/// Asian font (split into sections of their own), spacing, the grid's
/// pitch; the wrapping gets the line breaking rules.
pub(super) fn apply(job: &mut LayoutJob, spans: &[Span], r: &Rules) {
    job.wrap.kinsoku = r.para.kinsoku();
    job.wrap.hanging_punctuation = r.para.overflow_punct();
    // Spaces at the end of a row never wrap (Word's).
    job.wrap.hanging_spaces = true;
    if !r.para.word_wrap() {
        job.wrap.break_anywhere = true;
    }
    let snap = r.para.snap_to_grid();
    if job.text.chars().any(apex_doc::is_cjk) {
        let chars = snap && r.grid.kind.chars();
        split(job, spans, r, chars);
        // The last character of a line takes its whole cell too.
        let extra = (r.grid.char_pitch(r.base) - r.base) * r.pt;
        if chars && extra > 0.0 {
            job.wrap.max_width = (job.wrap.max_width - extra).max(1.0);
        }
    }
    if snap && r.grid.kind.lines() {
        if let Some(m) = r.multiple {
            let pitch = r.grid.pitch() * r.pt;
            for s in &mut job.sections {
                if let Some(lh) = s.format.line_height {
                    s.format.line_height = Some(snap_line(lh, m, pitch));
                }
            }
        }
    }
}

/// Makes a job vertical text: drawn turned a quarter clockwise with its
/// Asian characters upright and the vertical forms of punctuation.
pub(super) fn vertical(job: &mut LayoutJob) {
    job.vertical = true;
    if let std::borrow::Cow::Owned(t) = apex_doc::to_vertical(&job.text) {
        job.text = t;
    }
}

/// A line `lh` high (at `multiple` spacing) on a grid of `pitch`: whole
/// grid lines (two when the font is taller than one) times the multiple.
pub(super) fn snap_line(lh: f32, multiple: f32, pitch: f32) -> f32 {
    let single = lh / multiple.max(0.01);
    let units = (single / pitch - 0.05).ceil().max(1.0);
    units * pitch * multiple
}

/// Space after an Asian character of `size` on a character grid of
/// `pitch` (for a Normal font of `base`): the difference of the pitch and
/// the Normal size, or (`snap`) up to whole cells.
pub(super) fn grid_extra(size: f32, pitch: f32, base: f32, snap: bool) -> f32 {
    if !snap {
        return pitch - base;
    }
    let cells = (size / pitch - 0.01).ceil().max(1.0);
    cells * pitch - size
}

fn split(job: &mut LayoutJob, spans: &[Span], r: &Rules, chars: bool) {
    let (de, dn) = (r.para.auto_space_de(), r.para.auto_space_dn());
    let pitch = r.grid.char_pitch(r.base) * r.pt;
    let text = &job.text;
    let mut out = Vec::with_capacity(job.sections.len() + 4);
    let mut si = 0;
    for (i, sec) in job.sections.iter().enumerate() {
        while si < spans.len() && spans[si].sections.end <= i {
            si += 1;
        }
        // Fields, objects and hidden text stay as they are.
        let Some(span) = spans.get(si).filter(|s| s.sections.contains(&i) && sec.format.font_id.size > 0.1) else {
            out.push(sec.clone());
            continue;
        };
        let mut pieces: Vec<(usize, usize, bool)> = Vec::new();
        let range = sec.byte_range.start.0..sec.byte_range.end.0;
        for (o, c) in text[range.clone()].char_indices() {
            let asian = uses_asian_font(c, span.hint);
            let at = range.start + o;
            match pieces.last_mut() {
                Some(p) if p.2 == asian => p.1 = at + c.len_utf8(),
                _ => pieces.push((at, at + c.len_utf8(), asian)),
            }
        }
        if pieces.is_empty() {
            out.push(sec.clone());
            continue;
        }
        let scale = if span.size > 0.0 { sec.format.font_id.size / span.size } else { 1.0 };
        for (k, (from, to, asian)) in pieces.into_iter().enumerate() {
            let mut f = sec.format.clone();
            if asian {
                if let Some(font) = &span.font {
                    f.font_id = FontId::new(font.size * scale, font.family.clone());
                    // Lines of Asian text are as high as the Asian font needs.
                    let single = f.font_id.size * span.line.unwrap_or_else(|| crate::fonts::line_factor(&f.font_id.family));
                    if let Some(m) = r.multiple {
                        f.line_height = Some(single * m);
                    } else if let Some(min) = r.at_least {
                        f.line_height = Some(single.max(min));
                    }
                }
                if chars {
                    f.extra_letter_spacing += grid_extra(f.font_id.size, pitch, r.base * r.pt, r.grid.kind == apex_doc::GridKind::SnapToChars);
                }
            }
            let mut lead = if k == 0 { sec.leading_space } else { 0.0 };
            let first = text[from..].chars().next();
            if let (Some(p), Some(c)) = (text[..from].chars().next_back(), first) {
                if auto_space(p, c, de, dn) {
                    lead += AUTO_SPACE * f.font_id.size;
                }
            }
            out.push(LayoutSection { leading_space: lead, byte_range: ByteIndex(from)..ByteIndex(to), format: f });
        }
    }
    job.sections = out;
}

/// The Grid page of the Page Setup dialog: the kind of grid, characters
/// per line and lines per page (`base`: the Normal font's size, points).
pub(super) fn grid_ui(ui: &mut eframe::egui::Ui, p: &mut apex_doc::PageSetup, base: f32) {
    use apex_doc::GridKind;
    use eframe::egui;
    use crate::i18n::tr;
    let name = |k: GridKind| {
        tr(match k {
            GridKind::None => "page.grid_none",
            GridKind::Lines => "page.grid_lines",
            GridKind::LinesAndChars => "page.grid_lines_chars",
            GridKind::SnapToChars => "page.grid_snap_chars",
        })
    };
    crate::widgets::section(ui, tr("page.text_dir"));
    ui.horizontal(|ui| {
        let v = p.vertical();
        for (on, key) in [(false, "page.dir_horizontal"), (true, "page.dir_vertical")] {
            if ui.radio(v == on, tr(key)).clicked() && v != on {
                p.set_vertical(on);
            }
        }
    });
    ui.add_space(8.0);
    crate::widgets::section(ui, tr("page.grid"));
    // Lines run down a vertical page: its grid counts characters along
    // its height and lines across its width.
    let flow = if p.vertical() { p.flow_setup() } else { *p };
    let (w, h) = (flow.content_width().max(1.0), flow.content_height().max(1.0));
    let g = &mut p.grid;
    egui::ComboBox::from_id_salt("page_grid").selected_text(name(g.kind)).width(260.0).show_ui(ui, |ui| {
        for k in GridKind::ALL {
            ui.selectable_value(&mut g.kind, k, name(k));
        }
    });
    ui.add_space(8.0);
    let (mut lines, mut chars) = g.counts(w, h, base);
    egui::Grid::new("page_grid_counts").num_columns(3).min_col_width(110.0).spacing([10.0, 8.0]).show(ui, |ui| {
        ui.add_enabled_ui(g.kind.chars(), |ui| ui.label(tr("page.grid_chars")));
        ui.add_enabled_ui(g.kind.chars(), |ui| {
            if ui.add(egui::DragValue::new(&mut chars).range(1..=200)).changed() {
                g.char_space = DocGrid::space_for(w / (chars.max(1) as f32 + 0.01), base);
            }
        });
        ui.label(egui::RichText::new(format!("{} {:.2} pt", tr("page.grid_pitch"), g.char_pitch(base))).weak());
        ui.end_row();
        ui.add_enabled_ui(g.kind.lines(), |ui| ui.label(tr("page.grid_lines_page")));
        ui.add_enabled_ui(g.kind.lines(), |ui| {
            if ui.add(egui::DragValue::new(&mut lines).range(1..=300)).changed() {
                g.line_pitch = ((h / lines.max(1) as f32) * 20.0).floor() / 20.0;
            }
        });
        ui.label(egui::RichText::new(format!("{} {:.2} pt", tr("page.grid_pitch"), g.pitch())).weak());
        ui.end_row();
    });
    ui.add_space(8.0);
    ui.label(egui::RichText::new(tr("page.grid_hint")).weak());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_pitch() {
        // 18 pt lines: a 10.5 pt font takes one, a 24 pt one two.
        assert_eq!(snap_line(10.5 * 1.2, 1.0, 18.0), 18.0);
        assert_eq!(snap_line(24.0 * 1.2, 1.0, 18.0), 36.0);
        assert_eq!(snap_line(10.5 * 1.2 * 1.5, 1.5, 18.0), 27.0);
        // Characters take whole cells.
        assert!((grid_extra(10.5, 11.0, 10.5, true) - 0.5).abs() < 1e-4);
        assert!((grid_extra(20.0, 11.0, 10.5, true) - 2.0).abs() < 1e-4);
        assert!((grid_extra(20.0, 11.0, 10.5, false) - 0.5).abs() < 1e-4);
    }

    /// Rows of `text` wrapped `cells` characters wide (every character of
    /// the test font is as wide as the others: CJK ones are missing glyphs).
    fn rows(text: &str, cells: f32, kinsoku: bool, hanging: bool) -> Vec<String> {
        use eframe::egui::{Color32, Context, FontId};
        let ctx = Context::default();
        let mut out = Vec::new();
        ctx.run_ui(Default::default(), |ui| {
            let font = FontId::monospace(10.0);
            let w = ui.fonts_mut(|f| f.layout_no_wrap("あ".into(), font.clone(), Color32::BLACK)).size().x;
            let mut job = LayoutJob::simple(text.into(), font, Color32::BLACK, w * cells + w * 0.5);
            job.wrap.kinsoku = kinsoku;
            job.wrap.hanging_punctuation = hanging;
            let g = ui.fonts_mut(|f| f.layout_job(job));
            out = g.rows.iter().map(|r| r.row.text()).collect();
        }).textures_delta.clear();
        out
    }

    #[test]
    fn kinsoku_line_breaks() {
        // Anywhere between CJK characters.
        assert_eq!(rows("あいうえおかきくけこさし", 5.0, true, false), ["あいうえお", "かきくけこ", "さし"]);
        // No closing punctuation at a line start: a character goes along.
        assert_eq!(rows("あいうえお。かきくけ", 5.0, false, false), ["あいうえお", "。かきくけ"]);
        assert_eq!(rows("あいうえお。かきくけ", 5.0, true, false), ["あいうえ", "お。かきく", "け"]);
        assert_eq!(rows("あいうえおーかきくけ", 5.0, true, false)[1], "おーかきく");
        // ...or it hangs in the margin.
        assert_eq!(rows("あいうえお。かきくけ", 5.0, true, true), ["あいうえお。", "かきくけ"]);
        // No opening bracket at a line end.
        assert_eq!(rows("あいうえ「かきくけ」", 5.0, false, false)[0], "あいうえ「");
        assert_eq!(rows("あいうえ「かきくけ」", 5.0, true, false), ["あいうえ", "「かきく", "け」"]);
        // Latin words still wrap as words.
        assert_eq!(rows("漢字 abcdefg", 5.0, true, false)[0], "漢字 ");
    }
}
