//! Equations in the document: layout, painting and the equation editor.

use std::sync::Arc;

use apex_doc::math::{self, Equation, Measure, Paint};
use apex_doc::{OBJECT, Paragraph, Pos, Selection};
use eframe::egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use eframe::egui::{self, Color32, FontId, Galley, Id, Pos2, Sense, Shape, Stroke, pos2, vec2};

use super::{DocView, ParaLayout, Segment};
use crate::fonts::{self, Face};
use crate::i18n::tr;
use crate::widgets::{self, DialogResult};

/// An equation placed in a paragraph layout.
#[derive(Clone)]
pub(super) struct Placed {
    /// Top of its row, relative to the galley.
    row_top: f32,
    /// Left end of its baseline, relative to the galley.
    origin: Pos2,
    shapes: Arc<Vec<Shape>>,
}

/// An equation laid out in a paragraph, waiting for the galley.
pub(super) struct Pending {
    byte: usize,
    shapes: Arc<Vec<Shape>>,
    /// Baseline and line height of the text around it, and where the text
    /// sits in taller rows (0.5 centered, 1 at the bottom).
    text_ascent: f32,
    text_line: f32,
    valign: f32,
}

struct Fonts<'a>(&'a egui::Context);

impl Measure for Fonts<'_> {
    fn width(&mut self, text: &str, size: f32, italic: bool) -> f32 {
        self.0.fonts_mut(|f| {
            let j = job(f, text, size, italic, Color32::BLACK);
            f.layout_job(j)
        })
        .size()
        .x
    }
}

/// Installed fonts tried, in order, for symbols the serif face lacks.
const SYMBOL_FONTS: &[&str] = &["Cambria Math", "STIX Two Math", "DejaVu Sans", "Segoe UI Symbol", "Noto Sans Math", "DejaVu Math TeX Gyre", "Latin Modern Math", "Noto Sans Symbols", "OpenSymbol", "Noto Sans Mono"];

/// Font for `text`: the serif face, or an installed font with its symbols
/// (loaded in the background; the layout is redone when it arrives).
fn symbol_font(f: &mut egui::epaint::text::FontsView<'_>, text: &str, size: f32) -> FontId {
    let serif = fonts::font(Face::Serif, false, size);
    if f.has_glyphs(&serif, text) {
        return serif;
    }
    let installed = fonts::families();
    for name in SYMBOL_FONTS {
        if !installed.iter().any(|n| n.eq_ignore_ascii_case(name)) {
            continue;
        }
        let (id, _) = fonts::font_for(Some(name), false, false, size);
        if id == fonts::font(fonts::face_for(Some(name)), false, size) {
            // Not loaded yet: requested.
            break;
        }
        if f.has_glyphs(&id, text) {
            return id;
        }
    }
    serif
}

fn job(f: &mut egui::epaint::text::FontsView<'_>, text: &str, size: f32, italic: bool, color: Color32) -> LayoutJob {
    let font_id = symbol_font(f, text, (size * 2.0).round().max(1.0) / 2.0);
    let format = TextFormat { font_id, italics: italic, color, ..Default::default() };
    LayoutJob::single_section(text.to_string(), format)
}

/// Equation `eq` at `size` pixels: its width, ascent, descent and shapes
/// around the left end of its baseline.
pub(super) fn render(ctx: &egui::Context, eq: &Equation, size: f32, display: bool, color: Color32) -> (f32, f32, f32, Vec<Shape>) {
    let b = math::layout(eq.root(), size, display, &mut Fonts(ctx));
    let mut shapes = Vec::with_capacity(b.items.len());
    for item in &b.items {
        match item {
            Paint::Text { x, y, text, size, italic } => {
                let g: Arc<Galley> = ctx.fonts_mut(|f| {
                    let j = job(f, text, *size, *italic, color);
                    f.layout_job(j)
                });
                let base = g.rows.first().and_then(|r| r.row.glyphs.first().map(|gl| r.pos.y + gl.pos.y)).unwrap_or(size * 0.8);
                shapes.push(Shape::galley(pos2(*x, *y - base), g, color));
            }
            Paint::Line { points, width } => {
                let pts: Vec<Pos2> = points.iter().map(|&(x, y)| pos2(x, y)).collect();
                shapes.push(Shape::line(pts, Stroke::new(width.max(0.5), color)));
            }
        }
    }
    (b.width, b.ascent, b.descent, shapes)
}

/// Text format of an equation's character: an invisible no-break space as
/// wide as the equation, on a line tall enough for it with its baseline on
/// the text's. Queues the equation for [`place`].
#[allow(clippy::too_many_arguments)]
pub(super) fn object_format(ctx: &egui::Context, base: &TextFormat, eq: &Equation, display: bool, nbsp: f32, text: &str, start: usize, out: &mut Vec<Pending>) -> TextFormat {
    let (w, a, d, shapes) = render(ctx, eq, base.font_id.size, display, base.color);
    // The glyph extents are estimates: keep some air above and below.
    let (a, d) = (a + base.font_id.size * 0.1, d + base.font_id.size * 0.05);
    // Where the text's baseline falls in a line of its own height.
    let lone = ctx.fonts_mut(|f| f.layout_job(LayoutJob::single_section("x".into(), base.clone())));
    let (text_ascent, text_line) = lone.rows.first().map(|r| (r.row.glyphs.first().map_or(r.row.size.y * 0.8, |g| g.pos.y), r.row.size.y)).unwrap_or((base.font_id.size, base.font_id.size * 1.2));
    let bottom = base.valign == egui::Align::BOTTOM;
    let mut f = base.clone();
    f.font_id = fonts::font(Face::Sans, false, (w / nbsp).max(0.5));
    f.color = Color32::TRANSPARENT;
    f.underline = Stroke::NONE;
    f.strikethrough = Stroke::NONE;
    f.background = Color32::TRANSPARENT;
    f.italics = false;
    // Centered text leaves as much room above as below; text at the bottom
    // only leaves it above.
    let h = if bottom { text_line - text_ascent + a } else { (2.0 * (a - text_ascent) + text_line).max(2.0 * (d + text_ascent) - text_line) };
    f.line_height = Some(h.max(text_line));
    f.valign = if bottom { egui::Align::BOTTOM } else { egui::Align::Center };
    let shapes = Arc::new(shapes);
    for (o, _) in text.match_indices(OBJECT) {
        out.push(Pending { byte: start + o, shapes: shapes.clone(), text_ascent, text_line, valign: if bottom { 1.0 } else { 0.5 } });
    }
    f
}

/// Where the equations of a paragraph go, relative to its galley.
pub(super) fn place(p: &Paragraph, galley: &Galley, pending: Vec<Pending>) -> Vec<Placed> {
    let mut out = Vec::with_capacity(pending.len());
    for m in pending {
        let ci = p.text[..m.byte].chars().count();
        let mut start = 0usize;
        for row in &galley.rows {
            let n = row.row.glyphs.len();
            if ci < start + n {
                let g = &row.row.glyphs[ci - start];
                let base = row.pos.y + m.text_ascent + m.valign * (row.row.size.y - m.text_line);
                out.push(Placed { row_top: row.pos.y, origin: pos2(row.pos.x + g.pos.x, base), shapes: m.shapes });
                break;
            }
            start += n + usize::from(row.ends_with_newline);
        }
    }
    out
}

impl DocView {
    /// Equations whose row is in the segment.
    pub(super) fn paint_equations(&self, p: &egui::Painter, pl: &ParaLayout, seg: &Segment, galley_pos: Pos2) {
        for m in &pl.maths {
            if m.row_top < seg.gy0 - 0.5 || m.row_top >= seg.gy1 - 0.5 {
                continue;
            }
            let off = galley_pos.to_vec2() + m.origin.to_vec2();
            p.extend(m.shapes.iter().map(|s| {
                let mut s = s.clone();
                s.translate(off);
                s
            }));
        }
    }
}

// --- the equation editor ---------------------------------------------------------------

/// The equation editor.
pub(super) struct MathEdit {
    text: String,
    /// The equation edited, or `None` to insert one.
    at: Option<Pos>,
    display: bool,
    /// Where buttons insert (a char index in `text`).
    cursor: usize,
    first: bool,
}

/// Structure buttons: sample shown, text inserted, caret position in it
/// (chars) and tooltip key.
const STRUCTURES: &[(&str, &str, usize, &str)] = &[
    ("a/b", "()/()", 1, "eq.fraction"),
    ("x^n", "^()", 2, "eq.superscript"),
    ("x_i", "_()", 2, "eq.subscript"),
    ("x_i^n", "_()^()", 2, "eq.subsup"),
    ("√x", "√()", 2, "eq.sqrt"),
    ("√(n&x)", "√(&)", 2, "eq.root"),
    ("∑_(i=1)^n", "∑_()^() ", 3, "eq.sum"),
    ("∫_a^b", "∫_()^() ", 3, "eq.integral"),
    ("lim_(n→∞)", "lim_() ", 5, "eq.limit"),
    ("(x)", "()", 1, "eq.parens"),
    ("[x]", "[]", 1, "eq.brackets"),
    ("|x|", "||", 1, "eq.abs"),
    ("(■(a&b@c&d))", "(■(&@&))", 3, "eq.matrix"),
    ("sin θ", "sin ", 4, "eq.function"),
];

const SYMBOLS: &str = "αβγδεθλμπρσφωΔΣΩ±×÷·≤≥≠≈≡∞∂∇∈∉⊂∪∩→⇒∀∃";

/// The equation dialog's fields (shared with presentations): the linear
/// text in field `id`, its preview, and buttons that put structures and
/// symbols at `cursor` (a char index in `text`). `first`: the field takes
/// the focus.
pub(crate) fn editor(ui: &mut egui::Ui, id: Id, text: &mut String, cursor: &mut usize, first: &mut bool, display: bool) {
    ui.label(tr("eq.linear"));
    let out = egui::TextEdit::singleline(text).id(id).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace).show(ui);
    // The field keeps the focus (the click that opened the dialog
    // may be released on the page).
    if std::mem::take(first) || ui.memory(|m| m.focused().is_none()) {
        out.response.request_focus();
    }
    if let Some(r) = out.cursor_range {
        *cursor = r.primary.index.0;
    }
    ui.add_space(6.0);
    ui.label(tr("eq.preview"));
    preview(ui, &Equation::new(text), display);
    ui.add_space(6.0);
    ui.label(tr("eq.structures"));
    let mut put = None;
    ui.horizontal_wrapped(|ui| {
        for &(sample, insert, caret, key) in STRUCTURES {
            if math_button(ui, sample).on_hover_text(tr(key)).clicked() {
                put = Some((insert.to_string(), caret));
            }
        }
    });
    ui.add_space(4.0);
    ui.label(tr("eq.symbols"));
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for c in SYMBOLS.chars() {
            if math_button(ui, &c.to_string()).clicked() {
                put = Some((c.to_string(), 1));
            }
        }
    });
    if let Some((s, caret)) = put {
        let at = text.char_indices().nth(*cursor).map_or(text.len(), |(i, _)| i);
        text.insert_str(at, &s);
        *cursor += caret;
        let mut st = egui::text_edit::TextEditState::load(ui.ctx(), id).unwrap_or_default();
        st.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(*cursor))));
        st.store(ui.ctx(), id);
        ui.memory_mut(|m| m.request_focus(id));
    }
}

impl DocView {
    pub(super) fn equation_dialog_open(&self) -> bool {
        self.math_edit.is_some()
    }

    /// Insert > Equation (Alt+=): edits the selected equation or inserts one.
    pub(crate) fn open_equation(&mut self) {
        let edit = self.doc.equation_near(self.sel);
        let para = &self.doc.paragraphs[self.sel.start().para];
        let (text, at, display) = match edit {
            Some((s, eq)) => (eq.source().to_string(), Some(s.start()), math::is_display(para)),
            None => (String::new(), None, self.sel.is_empty() && para.text.trim().is_empty()),
        };
        let cursor = text.chars().count();
        self.math_edit = Some(MathEdit { text, at, display, cursor, first: true });
    }

    /// Double click on an equation: edits it.
    pub(super) fn equation_double_click(&mut self, local: Pos2) -> bool {
        let pos = self.pos_at(local);
        let Some((s, _)) = self.doc.equation_near(Selection::caret(pos)) else { return false };
        self.set_sel(s);
        self.open_equation();
        true
    }

    /// "Edit Equation" in the context menu, when there is one at the caret.
    pub(super) fn equation_menu_item(&mut self, ui: &mut egui::Ui) {
        if self.doc.equation_near(self.sel).is_some() && ui.button(format!("{}…", tr("eq.edit"))).clicked() {
            self.open_equation();
            ui.close();
        }
    }

    pub(super) fn equation_dialog(&mut self, ctx: &egui::Context) {
        let Some(e) = &mut self.math_edit else { return };
        let title = if e.at.is_some() { tr("eq.edit") } else { tr("eq.insert") };
        let id = Id::new(("eq_text", self.uid));
        let r = widgets::dialog(ctx, "doc_equation", title, 520.0, |ui| {
            editor(ui, id, &mut e.text, &mut e.cursor, &mut e.first, e.display);
        });
        match r {
            DialogResult::None => return,
            DialogResult::Cancel => {}
            DialogResult::Ok => {
                let text = e.text.trim();
                let same = e.at.and_then(|p| self.doc.equation_at(p)).is_some_and(|(_, eq)| eq.source() == text);
                match e.at {
                    Some(p) if !text.is_empty() && !same => self.doc.set_equation(p, Equation::new(text)),
                    None if !text.is_empty() => {
                        let s = self.doc.insert_equation(self.sel, Equation::new(text));
                        self.sel = Selection::caret(s.end());
                        self.pending = None;
                        self.scroll_to_caret = true;
                    }
                    _ => {}
                }
            }
        }
        self.math_edit = None;
        // Give the keyboard back to the page.
        ctx.memory_mut(|m| m.surrender_focus(id));
        self.refocus = true;
    }
}

/// Paints `shapes` of an equation `w` wide centered in `rect`.
fn paint_centered(p: &egui::Painter, rect: egui::Rect, (w, a, d, shapes): (f32, f32, f32, Vec<Shape>)) {
    let off = vec2(rect.center().x - w / 2.0, rect.center().y + (a - d) / 2.0);
    p.extend(shapes.into_iter().map(|mut s| {
        s.translate(off);
        s
    }));
}

fn preview(ui: &mut egui::Ui, eq: &Equation, display: bool) {
    let r = render(ui.ctx(), eq, 22.0, display, crate::theme::Paper::TEXT);
    let h = (r.1 + r.2 + 16.0).max(64.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 4.0, crate::theme::Paper::BG);
    p.rect_stroke(rect, 4.0, ui.visuals().widgets.noninteractive.bg_stroke, egui::StrokeKind::Inside);
    paint_centered(&p, rect, r);
}

/// A button showing a sample equation.
fn math_button(ui: &mut egui::Ui, sample: &str) -> egui::Response {
    let color = ui.visuals().text_color();
    let r = render(ui.ctx(), &Equation::new(sample), 14.0, false, color);
    let size = vec2((r.0 + 12.0).max(26.0), (r.1 + r.2 + 8.0).max(26.0));
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    crate::widgets::name(&resp, egui::WidgetType::Button, sample, None);
    let v = ui.style().interact(&resp);
    ui.painter().rect(rect, 3.0, v.weak_bg_fill, v.bg_stroke, egui::StrokeKind::Inside);
    paint_centered(ui.painter(), rect, r);
    resp
}

#[cfg(test)]
mod tests {
    use super::super::Layout;
    use super::*;
    use crate::print::{Job, Page, capture};

    #[test]
    fn equations_lay_out_and_print() {
        let ctx = egui::Context::default();
        fonts::install(&ctx);
        let frame = || ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        frame();
        frame();
        let doc = apex_io::markdown::read("Area $\\pi r^2$ of a circle\n\n$$x=\\frac{-b\\pm\\sqrt{b^2-4ac}}{2a}$$\n");
        let mut v = DocView::new(doc);
        v.layout = v.build_layout(&ctx, 1.0, (1, 0, 0, 0, 0), Layout::default());
        let (inline, display) = (&v.layout.paras[0], &v.layout.paras[1]);
        assert_eq!((inline.maths.len(), display.maths.len()), (1, 1));
        // The inline equation sits on the text's baseline.
        let row = &inline.galley.rows[0];
        let text_base = row.pos.y + row.row.glyphs[0].pos.y;
        assert!((inline.maths[0].origin.y - text_base).abs() < 1.0, "{} {text_base}", inline.maths[0].origin.y);
        // The display equation is centered on a taller line.
        let d = &display.maths[0];
        let w = display.galley.rows[0].row.glyphs[0].advance_width;
        assert!((d.origin.x + w / 2.0).abs() < 2.0, "{} {w}", d.origin.x);
        assert!(display.galley.size().y > inline.galley.size().y * 1.5);
        let top = d.shapes.iter().fold(egui::Rect::NOTHING, |r, s| r.union(s.visual_bounding_rect())).translate(d.origin.to_vec2());
        assert!(top.min.y >= -0.5 && top.max.y <= display.galley.size().y + 0.5, "{top:?}");
        // Its text and lines reach the PDF.
        let shapes = capture(&ctx, |p| p.extend(d.shapes.iter().cloned()));
        let job = Job { pages: vec![Page { size: vec2(800.0, 1000.0), shapes, links: Vec::new(), tags: Vec::new() }], ..Default::default() };
        let pdf = crate::pdf_out::to_pdf(&job, &[0], &mut crate::print::no_paint);
        let s = String::from_utf8_lossy(&pdf);
        assert!(s.contains("/FontFile"), "no text");
        assert!(v.doc.paragraphs[1].text.contains(OBJECT));
    }
}
