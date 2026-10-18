//! Text of shapes, placeholders and table cells laid out and painted:
//! each paragraph with what it inherits (the master's text styles, the
//! layout's placeholder), its bullet or number, indents, spacing and
//! alignment, in the text frame's box with its anchoring and autofit.

use std::borrow::Cow;
use std::sync::Arc;

use apex_doc::{Align, VAnchor};
use apex_slides::text::{Autofit, Bullet, Caps, Spacing};
use apex_slides::{Chain, Object, Rgba, SColor, Scheme, TextBody};
use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{self, Color32, Galley, Painter, Pos2, Rect, Stroke, Vec2, pos2, vec2};

use crate::fonts;

pub fn c32(c: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied((c.rgb >> 16) as u8, (c.rgb >> 8) as u8, c.rgb as u8, c.alpha)
}

/// What a table style puts on its cells' text: a color and bold.
#[derive(Clone, Copy, Default)]
pub struct Over {
    pub color: Option<Rgba>,
    pub bold: bool,
}

/// A laid out paragraph: its text and bullet, and where they go from
/// the text box's left edge and the text's top.
pub struct Line {
    pub galley: Arc<Galley>,
    /// Left of the text for left aligned paragraphs; the middle (centered)
    /// or the right end (right aligned) of the paragraph's width otherwise.
    pub x: f32,
    pub y: f32,
    pub bullet: Option<(Arc<Galley>, f32)>,
    /// Artistic text effects around the galley.
    pub fx: crate::text_fx::Layers,
}

pub struct Laid {
    pub lines: Vec<Line>,
    pub height: f32,
}

/// Wingdings and Symbol characters bullets use, as Unicode.
fn symbol_char(font: &str, c: char) -> char {
    let f = font.to_ascii_lowercase();
    if f.starts_with("wingdings") {
        match c {
            '§' | 'n' => '■',
            'l' => '●',
            'Ø' | 'à' => '➢',
            'ü' => '✓',
            'q' | 'o' => '❑',
            'v' => '❖',
            'Ÿ' => '•',
            'ð' => '➔',
            'è' => '➔',
            _ => '•',
        }
    } else if f == "symbol" {
        match c {
            '·' => '•',
            '-' => '–',
            _ => c,
        }
    } else {
        c
    }
}

/// Lays out `t` (the text of `o`) `width` pixels wide at `scale` pixels
/// per point, with fonts `font_scale` times their size and line spacing
/// reduced by `reduce` (autofit).
#[allow(clippy::too_many_arguments)]
pub fn layout(p: &Painter, ch: &Chain, o: &Object, t: &TextBody, width: f32, wrap: bool, scale: f32, font_scale: f32, reduce: f32, over: Over) -> Laid {
    apex_slides::resolve::memo(|| layout_now(p, ch, o, t, width, wrap, scale, font_scale, reduce, over))
}

#[allow(clippy::too_many_arguments)]
fn layout_now(p: &Painter, ch: &Chain, o: &Object, t: &TextBody, width: f32, wrap: bool, scale: f32, font_scale: f32, reduce: f32, over: Over) -> Laid {
    let mut lines = Vec::new();
    let mut y = 0.0;
    // Automatic numbers count per level.
    let mut counters: [u32; 9] = [0; 9];
    let mut prev_after = 0.0_f32;
    for (i, para) in t.paras.iter().enumerate() {
        let pp = ch.para(o, t, i);
        let level = para.style.level.min(8) as usize;
        let first = para.runs.first().map(|r| r.attrs.clone()).unwrap_or_default();
        let mut base = ch.run(o, &pp, &first);
        if over.bold {
            base.bold = true;
        }
        if let (Some(c), None) = (over.color, first.color) {
            base.color = c;
        }
        let size0 = base.size * font_scale;
        let line_h = |size: f32| match pp.line {
            Some(Spacing::Pts(v)) => v * scale,
            Some(Spacing::Pct(k)) => size * 1.2 * k * (1.0 - reduce) * scale,
            None => size * 1.2 * (1.0 - reduce) * scale,
        };
        let space = |s: Option<Spacing>| match s {
            Some(Spacing::Pts(v)) => v * scale,
            Some(Spacing::Pct(k)) => size0 * 1.2 * k * scale,
            None => 0.0,
        };
        let before = if i == 0 { 0.0 } else { space(pp.before) };
        y += before.max(0.0) + prev_after;
        prev_after = space(pp.after).max(0.0);
        // A glyph must fit the font atlas (egui asserts it does), in
        // pixels: one about 3 ems wide (U+2E3B) as well.
        let top = (p.ctx().input(|i| i.max_texture_side) as f32 / (4.0 * p.pixels_per_point().max(0.1))).clamp(8.0, 1024.0);
        let fmt = |f: &apex_slides::resolve::RunFmt, size: f32| -> TextFormat {
            let px = (size * scale).clamp(0.5, top);
            let small = f.baseline != 0.0;
            let px_run = if small { px * 0.65 } else { px };
            let (font_id, synth) = fonts::font_for(Some(&f.font), f.bold, f.italic, px_run);
            let color = if f.link.is_some() { c32(ch.color(&SColor::scheme(Scheme::Hlink))) } else { c32(f.color) };
            let under = f.underline || f.link.is_some();
            TextFormat {
                font_id,
                color,
                italics: synth,
                underline: if under { Stroke::new((px / 16.0).max(1.0), color) } else { Stroke::NONE },
                strikethrough: if f.strike { Stroke::new((px / 16.0).max(1.0), color) } else { Stroke::NONE },
                line_height: Some(line_h(size)),
                extra_letter_spacing: f.spacing * scale,
                valign: if f.baseline > 0.0 { egui::Align::TOP } else if f.baseline < 0.0 { egui::Align::BOTTOM } else { egui::Align::Center },
                background: f.highlight.map_or(Color32::TRANSPARENT, crate::theme::rgb),
                ..Default::default()
            }
        };
        let empty = para.text.trim().is_empty();
        // The bullet or number.
        let mut bullet_text = None;
        match pp.bullet.clone() {
            Some(Bullet::Char(c)) if !empty => {
                let font = match &pp.bullet_font {
                    Some(Some(f)) => f.to_string(),
                    _ => String::new(),
                };
                bullet_text = Some(symbol_char(&font, c).to_string());
                counters[level] = 0;
            }
            Some(Bullet::Auto(scheme, start)) if !empty => {
                counters[level] = if counters[level] == 0 { start.max(1) } else { counters[level].saturating_add(1) };
                bullet_text = Some(apex_slides::text::auto_number(&scheme, counters[level]));
            }
            _ if !empty => counters[level] = 0,
            _ => {}
        }
        for c in counters.iter_mut().skip(level + 1) {
            *c = 0;
        }
        let mar_l = pp.mar_l.unwrap_or(0.0) * scale;
        let indent = pp.indent.unwrap_or(0.0) * scale;
        let bullet = bullet_text.map(|b| {
            let mut f = base.clone();
            f.underline = false;
            f.link = None;
            if let Some(Some(c)) = &pp.bullet_color {
                f.color = ch.color(c);
            }
            let size = size0 * pp.bullet_size.filter(|s| s.is_finite()).unwrap_or(1.0).clamp(0.25, 4.0);
            let mut tf = fmt(&f, size);
            tf.line_height = Some(line_h(size0));
            p.layout_job(LayoutJob::single_section(b, tf))
        });
        let first_x = (mar_l + indent).max(0.0);
        let text_x = match &bullet {
            // Hanging bullets leave the text at the margin, if they fit.
            Some(b) if first_x + b.size().x <= mar_l => mar_l,
            Some(b) => first_x + b.size().x + size0 * scale * 0.25,
            None => first_x,
        };
        let mut job = LayoutJob::default();
        let lead = (text_x - mar_l).max(0.0);
        let mut at = 0;
        let mut first_section = true;
        let mut fx_runs = Vec::new();
        let mut chars = 0;
        for run in &para.runs {
            let end = (at + run.len).min(para.text.len());
            let Some(s) = para.text.get(at..end) else { break };
            at = end;
            if s.is_empty() {
                continue;
            }
            let mut f = ch.run(o, &pp, &run.attrs);
            if over.bold {
                f.bold = true;
            }
            if let (Some(c), None) = (over.color, run.attrs.color) {
                f.color = c;
            }
            let mut s = s.replace(apex_doc::LINE_BREAK, "\n");
            if f.caps == Caps::All {
                s = s.to_uppercase();
            }
            let size = f.size * font_scale;
            let n = s.chars().count();
            if let Some(fx) = &run.attrs.text_fx {
                fx_runs.push(crate::text_fx::Run { from: chars, to: chars + n, fx: fx.clone() });
            }
            chars += n;
            job.append(&s, if first_section { lead } else { 0.0 }, fmt(&f, size));
            first_section = false;
        }
        if first_section {
            // An empty paragraph still takes a line.
            job.append("", 0.0, fmt(&base, size0));
        }
        job.wrap.max_width = if wrap { (width - mar_l).max(1.0) } else { f32::INFINITY };
        let align = pp.align.unwrap_or(Align::Left);
        job.halign = match align {
            Align::Center => egui::Align::Center,
            Align::Right => egui::Align::Max,
            _ => egui::Align::Min,
        };
        job.justify = align == Align::Justify && wrap;
        let galley = p.layout_job(job);
        let (galley, fx) = crate::text_fx::apply(galley, &fx_runs, scale, Color32::WHITE);
        let x = match align {
            Align::Center => mar_l + (width - mar_l) / 2.0,
            Align::Right => width,
            _ => mar_l,
        };
        let h = galley.size().y;
        let bullet = bullet.map(|b| {
            // With centered text the bullet goes before the first line.
            let bx = match align {
                Align::Center | Align::Right => {
                    let w0 = galley.rows.first().map_or(0.0, |r| r.rect().width());
                    let left = if align == Align::Center { x - w0 / 2.0 } else { x - w0 };
                    left - b.size().x - size0 * scale * 0.25
                }
                _ => first_x,
            };
            (b, bx)
        });
        lines.push(Line { galley, x, y, bullet, fx });
        y += h;
    }
    Laid { lines, height: y }
}

/// Paints laid out text with its top left at `origin`, turned `angle`
/// radians around `pivot`.
pub fn paint(p: &Painter, laid: &Laid, origin: Pos2, angle: f32, pivot: Pos2) {
    let at = if angle == 0.0 { origin } else { pivot + egui::emath::Rot2::from_angle(angle) * (origin - pivot) };
    paint_at(p, laid, at, angle);
}

/// Laid out text placed on the screen: a point `q` of the text (from its
/// top left) shows at `at + rotation(angle) * q`.
pub struct Placed {
    pub laid: Laid,
    pub at: Pos2,
    pub angle: f32,
}

impl Placed {
    /// Screen point of text point `q`.
    pub fn screen(&self, q: Vec2) -> Pos2 {
        self.at + egui::emath::Rot2::from_angle(self.angle) * q
    }

    /// Text point of screen point `s`.
    pub fn local(&self, s: Pos2) -> Vec2 {
        egui::emath::Rot2::from_angle(-self.angle) * (s - self.at)
    }
}

/// Paints laid out text whose top left is at `at`, all turned `angle`.
fn paint_at(p: &Painter, laid: &Laid, at: Pos2, angle: f32) {
    let rot = egui::emath::Rot2::from_angle(angle);
    for l in &laid.lines {
        if angle == 0.0 {
            crate::text_fx::paint(p, at + vec2(l.x, l.y), &l.fx);
        } else {
            crate::text_fx::paint_turned(p, at + rot * vec2(l.x, l.y), angle, &l.fx);
        }
        p.add(egui::epaint::TextShape::new(at + rot * vec2(l.x, l.y), l.galley.clone(), Color32::BLACK).with_angle(angle));
        if let Some((b, bx)) = &l.bullet {
            // On the first line's baseline.
            let base = l.galley.rows.first().map_or(0.0, |r| r.rect().bottom());
            let by = base - b.rows.first().map_or(b.size().y, |r| r.rect().bottom());
            p.add(egui::epaint::TextShape::new(at + rot * vec2(*bx, l.y + by), b.clone(), Color32::BLACK).with_angle(angle));
        }
    }
}

/// Office's date formats of `a:fld type="datetimeN"` (the English ones,
/// with the UI language's names of months and days).
fn date_picture(format: &str) -> Option<&'static str> {
    Some(match format {
        "datetime" | "datetime1" => "M/d/yyyy",
        "datetime2" => "dddd, MMMM d, yyyy",
        "datetime3" => "d MMMM yyyy",
        "datetime4" => "MMMM d, yyyy",
        "datetime5" => "d-MMM-yy",
        "datetime6" => "MMMM yy",
        "datetime7" => "MMM-yy",
        "datetime8" => "M/d/yyyy h:mm AM/PM",
        "datetime9" => "M/d/yyyy h:mm:ss AM/PM",
        "datetime10" => "H:mm",
        "datetime11" => "H:mm:ss",
        "datetime12" => "h:mm AM/PM",
        "datetime13" => "h:mm:ss AM/PM",
        _ => return None,
    })
}

/// What a field shows on `ch`'s slide: its number (counting hidden
/// slides, from the deck's first number), today's date in the field's
/// format, or else the text saved with it.
pub fn field_text(ch: &Chain, f: &apex_doc::Field) -> String {
    match f.kind {
        apex_doc::FieldKind::Page => match ch.slide {
            Some(s) => ch.deck.slides.iter().position(|x| std::ptr::eq(x.as_ref(), s)).map_or_else(|| f.cached.clone(), |i| (ch.deck.first_number as usize + i).to_string()),
            None => "‹#›".into(),
        },
        apex_doc::FieldKind::Date => match f.format.as_deref().and_then(date_picture) {
            Some(pic) => {
                let (y, mo, d, h, mi) = crate::common::local_now();
                let months: Vec<&str> = crate::i18n::tr("field.months").split('|').collect();
                let days: Vec<&str> = crate::i18n::tr("field.days").split('|').collect();
                apex_doc::format_date(pic, (y, mo, d, h, mi, 0), &months, &days)
            }
            None => f.cached.clone(),
        },
        _ => f.cached.clone(),
    }
}

/// `t` with its fields shown as [`field_text`] (borrowed when it has
/// none): what painting and printing show, while typing keeps the marks.
pub fn with_fields<'t>(ch: &Chain, t: &'t TextBody) -> Cow<'t, TextBody> {
    let has = |p: &apex_doc::Paragraph| p.runs.iter().any(|r| r.attrs.field.is_some());
    if !t.paras.iter().any(has) {
        return Cow::Borrowed(t);
    }
    let mut out = t.clone();
    for para in out.paras.iter_mut().filter(|p| has(p)) {
        let text = std::mem::take(&mut para.text);
        let runs = std::mem::take(&mut para.runs);
        para.marks.clear();
        let first = runs.first().map(|r| r.attrs.clone()).unwrap_or_default();
        let mut at = 0;
        for r in runs {
            let s = text.get(at..at + r.len).unwrap_or("");
            at += r.len;
            let shown = match &r.attrs.field {
                Some(f) => s.replace(apex_doc::OBJECT, &field_text(ch, f)),
                None => s.to_string(),
            };
            para.push(&shown, r.attrs);
        }
        if para.runs.is_empty() {
            para.runs.push(apex_doc::Run { len: 0, attrs: first });
        }
    }
    Cow::Owned(out)
}

/// Lays out and paints the text `t` of `o` in `area` (screen pixels, the
/// box before turning), turned `rot` degrees around `pivot`.
#[allow(clippy::too_many_arguments)]
pub fn paint_body(p: &Painter, ch: &Chain, o: &Object, t: &TextBody, area: Rect, rot: f32, pivot: Pos2, scale: f32, over: Over) {
    let placed = place_body(p, ch, o, t, area, rot, pivot, scale, over);
    paint_at(p, &placed.laid, placed.at, placed.angle);
}

/// Lays out the text `t` of `o` in `area` as [`paint_body`] paints it.
#[allow(clippy::too_many_arguments)]
pub fn place_body(p: &Painter, ch: &Chain, o: &Object, t: &TextBody, area: Rect, rot: f32, pivot: Pos2, scale: f32, over: Over) -> Placed {
    let body = ch.body(o, t);
    let ins = body.insets_or_default();
    let mut area = Rect::from_min_max(pos2(area.min.x + ins[0] * scale, area.min.y + ins[1] * scale), pos2(area.max.x - ins[2] * scale, area.max.y - ins[3] * scale));
    if area.width() < 1.0 {
        area = Rect::from_center_size(area.center(), vec2(1.0, area.height().max(1.0)));
    }
    // Vertical text: laid out along the box's height and turned.
    let (turn, w): (f32, f32) = match body.vert.as_deref() {
        Some("vert" | "eaVert" | "wordArtVertRtl" | "mongolianVert") => (90.0, area.height()),
        Some("vert270") => (270.0, area.height()),
        _ => (0.0, area.width()),
    };
    let (font_scale, reduce) = match body.autofit {
        Some(Autofit::Shrink { scale, reduce }) => (scale.clamp(0.05, 1.0), reduce.clamp(0.0, 0.9)),
        _ => (1.0, 0.0),
    };
    let wrap = body.wrap.unwrap_or(true);
    let mut laid = layout(p, ch, o, t, w, wrap, scale, font_scale, reduce, over);
    let room = if turn == 0.0 { area.height() } else { area.width() };
    // Text that "shrinks on overflow" without a saved scale: shrink it here.
    if matches!(body.autofit, Some(Autofit::Shrink { .. })) && laid.height > room * 1.02 {
        let mut k = font_scale;
        for _ in 0..6 {
            k *= 0.9;
            laid = layout(p, ch, o, t, w, wrap, scale, k, (reduce + 0.1).min(0.2), over);
            if laid.height <= room {
                break;
            }
        }
    }
    let anchor = body.anchor.unwrap_or(VAnchor::Top);
    let off = match anchor {
        VAnchor::Top => 0.0,
        VAnchor::Middle => (room - laid.height) / 2.0,
        VAnchor::Bottom => room - laid.height,
    };
    let turn_rot = |q: Pos2| if rot == 0.0 { q } else { pivot + egui::emath::Rot2::from_angle(rot.to_radians()) * (q - pivot) };
    if turn == 0.0 {
        Placed { laid, at: turn_rot(pos2(area.min.x, area.min.y + off)), angle: rot.to_radians() }
    } else {
        // The text's box, turned about the area's center, then with the shape.
        let c = area.center();
        let local = Rect::from_center_size(c, vec2(area.height(), area.width()));
        let origin = pos2(local.min.x, local.min.y + off);
        let turned = c + egui::emath::Rot2::from_angle(turn.to_radians()) * (origin - c);
        Placed { laid, at: turn_rot(turned), angle: (rot + turn).to_radians() }
    }
}

/// The height of `t` laid out `width` pixels wide (table rows grow to it).
pub fn height(p: &Painter, ch: &Chain, o: &Object, t: &TextBody, width: f32, scale: f32, over: Over) -> f32 {
    layout(p, ch, o, t, width, true, scale, 1.0, 0.0, over).height
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_doc::{Attrs, Field, FieldKind, OBJECT};

    #[test]
    fn fields_show_the_slide_number_and_date() {
        let mut deck = apex_slides::build::new_deck(0);
        apex_slides::edit::add_slide(&mut deck, 1, 0, 0);
        deck.first_number = 5;
        let mut t = TextBody::plain("Page ");
        let mut page = Field::new(FieldKind::Page);
        page.cached = "‹#›".into();
        let mut date = Field::new(FieldKind::Date);
        date.format = Some("datetime10".into());
        let mut fixed = Field::new(FieldKind::Date);
        fixed.format = Some("fixed".into());
        fixed.cached = "1 May".into();
        for f in [page, date, fixed] {
            t.paras[0].push(&OBJECT.to_string(), Attrs { field: Some(Arc::new(f)), ..Default::default() });
            t.paras[0].push(" ", Attrs::default());
        }
        let ch = Chain::for_slide(&deck, &deck.slides[1]);
        let shown = with_fields(&ch, &t);
        let s = &shown.paras[0].text;
        assert!(s.starts_with("Page 6 "), "{s}");
        assert!(s.ends_with(" 1 May "), "{s}");
        // H:mm, today.
        let time = s.split(' ').nth(2).unwrap_or("");
        assert!(time.contains(':') && time.len() >= 4, "{s}");
        let len: usize = shown.paras[0].runs.iter().map(|r| r.len).sum();
        assert_eq!(len, s.len());
        // Without fields nothing is copied.
        let plain = TextBody::plain("x");
        assert!(matches!(with_fields(&ch, &plain), Cow::Borrowed(_)));
    }
}
