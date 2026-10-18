//! Reveal Formatting (Shift+F1): a pane with the font, paragraph, section
//! and style of the selected text, where each value comes from (the
//! defaults, the paragraph style, a character style or direct formatting),
//! and links to the dialogs that change them.

use apex_doc::reveal::FormatSource;
use apex_doc::{Align, LineRule, PAPER_SIZES};
use eframe::egui::{self, Id, RichText, vec2};

use super::DocView;
use super::dialogs::ruler_unit;
use crate::i18n::{tr, trf};
use crate::theme;
use crate::widgets::FitWindow as _;

/// What a link of the pane opens.
enum Open {
    Font,
    Paragraph,
    Style,
    PageSetup,
    ClearChar,
}

fn source_text(s: FormatSource) -> &'static str {
    match s {
        FormatSource::Default => tr("reveal.src_default"),
        FormatSource::ParaStyle => tr("reveal.src_style"),
        FormatSource::CharStyle => tr("reveal.src_char_style"),
        FormatSource::Direct => tr("reveal.src_direct"),
    }
}

/// A length in the ruler's unit.
fn len(pt: f32) -> String {
    let (unit, _, suffix) = ruler_unit();
    format!("{:.2} {suffix}", pt / unit)
}

fn row(ui: &mut egui::Ui, label: &str, value: &str, src: FormatSource) {
    ui.label(RichText::new(label).weak());
    let mut v = RichText::new(value);
    if src == FormatSource::Direct {
        // Direct formatting stands out, as in Word.
        v = v.strong();
    }
    ui.label(v);
    ui.label(RichText::new(source_text(src)).small().color(match src {
        FormatSource::Direct => ui.visuals().warn_fg_color,
        _ => ui.visuals().weak_text_color(),
    }));
    ui.end_row();
}

fn yes_no(b: bool) -> &'static str {
    if b { tr("reveal.yes") } else { tr("reveal.no") }
}

impl DocView {
    /// Shows or hides the pane.
    pub(super) fn toggle_reveal(&mut self) {
        self.styles.reveal = !self.styles.reveal;
    }

    /// The Reveal Formatting pane.
    pub(super) fn reveal_pane(&mut self, ctx: &egui::Context) {
        if !self.styles.reveal {
            return;
        }
        let a = self.attrs_at_caret();
        let para = self.sel.start().para;
        let r = self.doc.reveal(para, &a);
        let f = &r.format;
        let st = r.para;
        let style_name = super::kind_label(st.kind).to_string();
        let style_name = self.doc.styles.custom(st.custom).map_or(style_name, |d| d.name.to_string());
        let char_style = a.char_style.as_deref().and_then(|id| self.doc.styles.by_id(id)).map(|d| d.name.to_string());
        let mut open = true;
        let mut action = None;
        let right = ctx.content_rect().right_top() + vec2(-330.0, 110.0);
        crate::widgets::esc_closes(ctx, Id::new(("reveal_pane", self.uid)), &mut open);
        egui::Window::new(tr("reveal.title")).id(Id::new(("reveal_pane", self.uid))).open(&mut open).default_pos(right).default_width(310.0).resizable(true).collapsible(false).show_fit(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(520.0).show(ui, |ui| {
                let grid = |ui: &mut egui::Ui, id: &str, add: &mut dyn FnMut(&mut egui::Ui)| {
                    egui::Grid::new((id, self.uid)).num_columns(3).spacing(vec2(10.0, 3.0)).striped(false).show(ui, |ui| add(ui));
                };
                // Style.
                ui.horizontal(|ui| {
                    ui.heading(RichText::new(tr("reveal.style")).size(14.0));
                    if ui.link(format!("{}…", tr("style.modify"))).clicked() {
                        action = Some(Open::Style);
                    }
                });
                ui.label(format!("{}: {style_name}", tr("doc.paragraph_style")));
                ui.separator();
                // Font.
                ui.horizontal(|ui| {
                    ui.heading(RichText::new(tr("reveal.font")).size(14.0));
                    if ui.link(format!("{}…", tr("reveal.change"))).on_hover_text(tr("font.title")).clicked() {
                        action = Some(Open::Font);
                    }
                    if ui.link(tr("reveal.clear_char")).clicked() {
                        action = Some(Open::ClearChar);
                    }
                });
                grid(ui, "reveal_font", &mut |ui| {
                    let name = f.font.as_deref().map_or_else(|| tr("font.default").to_string(), str::to_string);
                    row(ui, tr("reveal.font_name"), &name, r.chars.font);
                    row(ui, tr("reveal.size"), &format!("{} pt", f.size as f32 / 2.0), r.chars.size);
                    row(ui, tr("font.bold"), yes_no(f.bold), r.chars.bold);
                    row(ui, tr("font.italic"), yes_no(f.italic), r.chars.italic);
                    row(ui, tr("reveal.underline"), yes_no(f.underline), r.chars.underline);
                    let color = f.color.map_or_else(|| tr("reveal.automatic").to_string(), |c| format!("#{c:06X}"));
                    row(ui, tr("reveal.color"), &color, r.chars.color);
                    let fx = &f.fx;
                    let mut on: Vec<String> = [(fx.small_caps, "font.small_caps"), (fx.all_caps, "font.all_caps"), (fx.double_strike, "font.double_strike"), (fx.hidden, "font.hidden")].iter().filter(|x| x.0).map(|x| tr(x.1).to_string()).collect();
                    if a.strike {
                        on.push(tr("font.strike").to_string());
                    }
                    if fx.scale != 0 && fx.scale != 100 {
                        on.push(format!("{} {}%", tr("font.scale"), fx.scale));
                    }
                    if fx.spacing != 0 {
                        on.push(format!("{} {} pt", tr("font.spacing"), fx.spacing as f32 / 20.0));
                    }
                    if fx.position != 0 {
                        on.push(format!("{} {} pt", tr("font.position"), fx.position as f32 / 2.0));
                    }
                    if a.superscript {
                        on.push(tr("reveal.superscript").to_string());
                    }
                    if a.subscript {
                        on.push(tr("reveal.subscript").to_string());
                    }
                    if !on.is_empty() {
                        row(ui, tr("font.effects"), &on.join(", "), if a.strike || a.superscript || a.subscript { FormatSource::Direct } else { r.chars.fx });
                    }
                    if let Some(h) = a.highlight {
                        row(ui, tr("reveal.highlight"), &format!("#{h:06X}"), FormatSource::Direct);
                    }
                    if let Some(l) = a.lang.as_deref() {
                        row(ui, tr("reveal.language"), l, FormatSource::Direct);
                    }
                    if let Some(cs) = &char_style {
                        row(ui, tr("reveal.char_style"), cs, FormatSource::CharStyle);
                    }
                });
                // A swatch of the text's color.
                if let Some(c) = f.color {
                    let (rect, _) = ui.allocate_exact_size(vec2(40.0, 6.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 2.0, theme::rgb(c));
                }
                ui.separator();
                // Paragraph.
                ui.horizontal(|ui| {
                    ui.heading(RichText::new(tr("reveal.paragraph")).size(14.0));
                    if ui.link(format!("{}…", tr("reveal.change"))).on_hover_text(tr("doc.paragraph")).clicked() {
                        action = Some(Open::Paragraph);
                    }
                });
                grid(ui, "reveal_para", &mut |ui| {
                    let align = match st.align {
                        Align::Left => tr("reveal.align_left"),
                        Align::Center => tr("reveal.align_center"),
                        Align::Right => tr("reveal.align_right"),
                        Align::Justify => tr("reveal.align_justify"),
                    };
                    row(ui, tr("para.alignment"), align, r.paras.align);
                    row(ui, tr("reveal.indent_left"), &len(st.indent_left), r.paras.indent_left);
                    row(ui, tr("reveal.indent_right"), &len(st.indent_right), r.paras.indent_right);
                    let first = if st.first_line < 0.0 { format!("{} {}", tr("para.hanging"), len(-st.first_line)) } else { len(st.first_line) };
                    row(ui, tr("para.first_line"), &first, r.paras.first_line);
                    let pts = |v: Option<f32>| v.map_or_else(|| tr("reveal.automatic").to_string(), |v| format!("{v} pt"));
                    row(ui, tr("reveal.space_before"), &pts(st.space_before), r.paras.space_before);
                    row(ui, tr("reveal.space_after"), &pts(st.space_after), r.paras.space_after);
                    let line = match st.line_rule {
                        LineRule::Multiple if st.line_spacing <= 0.0 => tr("para.single").to_string(),
                        LineRule::Multiple => trf("reveal.line_multiple", &[("n", &st.line_spacing)]),
                        LineRule::AtLeast => trf("reveal.line_at_least", &[("n", &st.line_spacing)]),
                        LineRule::Exact => trf("reveal.line_exact", &[("n", &st.line_spacing)]),
                    };
                    row(ui, tr("para.line_spacing"), &line, r.paras.line_spacing);
                    if let Some(s) = st.shading {
                        row(ui, tr("reveal.shading"), &format!("#{s:06X}"), r.paras.shading);
                    }
                    if !st.borders.is_empty() {
                        row(ui, tr("reveal.borders"), tr("reveal.yes"), r.paras.borders);
                    }
                    if !st.tabs.as_slice().is_empty() {
                        row(ui, tr("reveal.tabs"), &st.tabs.as_slice().len().to_string(), r.paras.tabs);
                    }
                    let keep: Vec<&str> = [(st.keep_with_next, "reveal.keep_next"), (st.keep_lines, "reveal.keep_lines"), (st.page_break_before, "reveal.break_before")].iter().filter(|x| x.0).map(|x| tr(x.1)).collect();
                    if !keep.is_empty() {
                        row(ui, tr("reveal.pagination"), &keep.join(", "), r.paras.keep);
                    }
                });
                ui.separator();
                // Section.
                let (page, cols) = self.doc.section_setup(para);
                let sections = self.doc.sections().len().max(1);
                ui.horizontal(|ui| {
                    ui.heading(RichText::new(tr("reveal.section")).size(14.0));
                    if ui.link(format!("{}…", tr("page.setup"))).clicked() {
                        action = Some(Open::PageSetup);
                    }
                });
                grid(ui, "reveal_section", &mut |ui| {
                    let d = FormatSource::Default;
                    row(ui, tr("reveal.section_n"), &format!("{} / {sections}", r.section + 1), d);
                    let (w, h) = (page.width, page.height);
                    let named = PAPER_SIZES.iter().find(|p| ((p.1 - w).abs() < 2.0 && (p.2 - h).abs() < 2.0) || ((p.1 - h).abs() < 2.0 && (p.2 - w).abs() < 2.0)).map(|p| p.0.to_string());
                    let size = format!("{} × {}", len(w), len(h));
                    row(ui, tr("reveal.paper"), &named.map_or(size.clone(), |n| format!("{n} ({size})")), d);
                    row(ui, tr("reveal.orientation"), if w > h { tr("reveal.landscape") } else { tr("reveal.portrait") }, d);
                    row(ui, tr("reveal.margins"), &format!("{} · {} · {} · {}", len(page.margin_top), len(page.margin_bottom), len(page.margin_left), len(page.margin_right)), d);
                    row(ui, tr("reveal.columns"), &cols.n().to_string(), d);
                });
                ui.label(RichText::new(tr("reveal.margins_hint")).small().weak());
            });
        });
        if !open {
            self.styles.reveal = false;
        }
        match action {
            Some(Open::Font) => self.open_font_dialog(),
            Some(Open::Paragraph) => self.open_paragraph_dialog(),
            Some(Open::Style) => self.modify_current_style(),
            Some(Open::PageSetup) => self.open_page_setup(0),
            Some(Open::ClearChar) => self.clear_formatting(false),
            None => {}
        }
    }
}
