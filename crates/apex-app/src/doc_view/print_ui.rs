//! Document pages for printing and PDF, headers and footers, page setup.

use apex_doc::{PageSetup, PageVAlign, ParaKind, Pos};
use eframe::egui::text::CCursor;
use eframe::egui::{self, Pos2, Rect, pos2};

use super::{DocView, Layout, Segment, char_count, seg_geometry};
use crate::i18n::tr;
use crate::print::{self, Job, LinkTarget, Page, margins_ui, paper_ui};
use crate::icons::Icon;
use crate::widgets::{self, DialogResult};

/// The PDF structure type of a paragraph.
fn pdf_role(kind: ParaKind) -> &'static str {
    match kind {
        ParaKind::Title => "H1",
        ParaKind::Heading(n) => ["H1", "H2", "H3", "H4", "H5", "H6"][n.clamp(1, 6) as usize - 1],
        ParaKind::Quote => "BlockQuote",
        _ => "P",
    }
}

/// Page setup dialog state.
pub struct SetupDialog {
    page: PageSetup,
    tab: usize,
    /// The caret section's different first page, and different even pages.
    first: bool,
    odd_even: bool,
    /// Edit the header (true) or footer when the dialog closes.
    edit: Option<bool>,
    /// The caret section's setup when opened, and what changes apply to.
    orig: PageSetup,
    apply: super::layout_ui::ApplyTo,
    /// The caret section's page numbering.
    numbering: apex_doc::PageNumbering,
}

impl DocView {
    /// Page and y (pixels from the page top) of a position in a layout.
    fn place_of(&self, l: &Layout, pos: Pos) -> (usize, f32) {
        let pl = &l.paras[pos.para];
        let text = &self.doc.paragraphs[pos.para].text;
        let ci = text[..pos.offset.min(text.len())].chars().count();
        let r = pl.galley.pos_from_cursor(CCursor::new(ci));
        let segs = l.segs_of(pos.para);
        let seg = segs.iter().rev().find(|s| r.min.y >= s.gy0 - 0.5).unwrap_or(&segs[0]);
        let g = l.geo_of(seg.page);
        // Vertical pages: their top.
        (seg.page, g.margin.y + if g.vertical { 0.0 } else { seg.y + r.min.y - seg.gy0 })
    }

    /// Rects (relative to the page) of the characters `from..to` of a segment.
    fn char_rects(l: &Layout, seg: &Segment, from: usize, to: usize, o: Pos2) -> Vec<Rect> {
        let pl = &l.paras[seg.para];
        let (galley_pos, _) = seg_geometry(l, seg, pl, o);
        let mut out = Vec::new();
        let mut start = 0usize;
        for row in &pl.galley.rows {
            let n = row.row.glyphs.len() + usize::from(row.ends_with_newline);
            let end = start + n;
            if row.pos.y + row.row.size.y > seg.gy0 && row.pos.y < seg.gy1 && to > start && from < end {
                let x_at = |i: usize| row.pos.x + row.row.glyphs.get(i - start).map_or(row.row.size.x, |g| g.pos.x);
                let (x0, x1) = (x_at(from.max(start)), x_at(to.min(end)));
                if x1 > x0 {
                    out.push(Rect::from_min_max(pos2(x0, row.pos.y), pos2(x1, row.pos.y + row.row.size.y)).translate(galley_pos.to_vec2()));
                }
            }
            start = end;
        }
        out
    }

    /// Lays the document out at print size and paints every page.
    pub fn print_job(&self, ctx: &egui::Context, file: &str) -> Job {
        // While a header is edited, the body is printed.
        if let Some(body) = self.hf_body() {
            return body.print_job(ctx, file);
        }
        // Pictures decode now so that they're in the pages: the text's,
        // floating ones and those of headers and footers.
        let d = &self.doc;
        let stories = d.headers.parts.iter().chain(d.paragraphs.iter().filter_map(|p| p.section.as_ref()).flat_map(|s| s.headers.parts.iter())).flatten().map(|s| s.as_slice());
        let runs = std::iter::once(d.paragraphs.as_slice()).chain(stories).flatten().flat_map(|p| p.runs.iter());
        for img in runs.flat_map(|r| r.attrs.image.as_deref().into_iter().chain(r.attrs.shape.as_ref().and_then(|s| s.shape.picture.as_deref()))) {
            crate::images::texture(ctx, crate::images::key(&img.data), &img.data, true);
            if let Some(fx) = img.fx.as_deref() {
                crate::images::texture_fx(ctx, &img.data, fx, img.width, true);
                if fx.reflection {
                    crate::images::reflection_texture(ctx, &img.data, fx, img.width, true);
                }
            }
        }
        if let Some(data) = self.doc.deco.watermark.as_ref().and_then(|w| w.picture.as_ref()) {
            crate::images::texture(ctx, crate::images::key(data), data, true);
        }
        self.printing.set(true);
        let l = self.build_layout(ctx, 1.0, Default::default(), Layout::default());
        self.printing.set(false);
        let title = if self.doc.meta.title.is_empty() { file.to_string() } else { self.doc.meta.title.clone() };
        let mut job = Job { title, author: self.doc.meta.author.clone(), ..Default::default() };
        job.lang = self.doc.language().map(|l| l.to_string()).unwrap_or_default();
        for (_, img) in self.doc.images() {
            if !img.alt.trim().is_empty() {
                job.alts.insert(crate::images::key(&img.data), img.alt.clone());
            }
        }
        let hf = self.doc.has_headers();
        for page in 0..l.pages {
            let g = l.geo_of(page);
            let o = pos2(g.margin.x, g.margin.y);
            let page_rect = Rect::from_min_size(Pos2::ZERO, g.size);
            let segs: Vec<Segment> = l.page_segs(page, page).filter(|s| !s.hidden).copied().collect();
            let shapes = print::capture(ctx, |p| {
                self.paint_page_extras(p, &l, page, page_rect, 1.0);
                // Headers and footers go behind the text (as on screen).
                if hf {
                    self.paint_page_hf(p, ctx, &l, page, page_rect, 1.0, None);
                }
                self.paint_content(p, &l, page, Pos2::ZERO, 1.0);
            });
            let mut links = Vec::new();
            for seg in &segs {
                let para = &self.doc.paragraphs[seg.para];
                for (s, e, a) in para.spans() {
                    let Some(target) = &a.link else { continue };
                    let target = match target.strip_prefix('#') {
                        Some(name) => match self.doc.bookmark(name) {
                            Some(bm) => {
                                let (pg, y) = self.place_of(&l, bm.start);
                                LinkTarget::Page(pg, y)
                            }
                            None => continue,
                        },
                        None => LinkTarget::Uri(target.to_string()),
                    };
                    let mut rects = Self::char_rects(&l, seg, char_count(&para.text, s), char_count(&para.text, e), o + egui::vec2(seg.x, 0.0));
                    if g.vertical {
                        let t = super::tategaki::Turn { o, w: g.content.x };
                        rects.iter_mut().for_each(|r| *r = t.rect(*r));
                    }
                    for r in rects {
                        let t = match &target {
                            LinkTarget::Uri(u) => LinkTarget::Uri(u.clone()),
                            LinkTarget::Page(pg, y) => LinkTarget::Page(*pg, *y),
                        };
                        links.push((r, t));
                    }
                }
            }
            let turn = super::tategaki::Turn { o, w: g.content.x };
            let tags = segs
                .iter()
                .map(|seg| {
                    let r = Rect::from_min_size(o + egui::vec2(seg.x, seg.y), egui::vec2(seg.w, (seg.gy1 - seg.gy0).max(1.0)));
                    print::Tag { rect: if g.vertical { turn.rect(r) } else { r }, role: pdf_role(self.doc.paragraphs[seg.para].style.kind), key: seg.para as u64 }
                })
                .collect();
            job.pages.push(Page { size: g.size, shapes, links, tags });
        }
        job.textures = crate::images::used_by(job.pages.iter().flat_map(|p| p.shapes.iter().map(|c| &c.shape)));
        // Headings become the PDF bookmarks.
        for (i, para) in self.doc.paragraphs.iter().enumerate() {
            let level = match para.style.kind {
                ParaKind::Title => 0,
                ParaKind::Heading(n) => n.saturating_sub(1),
                _ => continue,
            };
            let text: String = para.text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            let seg = &l.segs[l.first_seg[i]];
            let g = l.geo_of(seg.page);
            job.outline.push((text.to_string(), level, seg.page, g.margin.y + if g.vertical { 0.0 } else { seg.y }));
        }
        job
    }

    /// Where each printed page starts: the paragraph of its first line, and
    /// whether the page goes on with a paragraph begun on the page before.
    pub fn page_starts(&self, ctx: &egui::Context) -> Vec<(usize, bool)> {
        self.printing.set(true);
        let l = self.build_layout(ctx, 1.0, Default::default(), Layout::default());
        self.printing.set(false);
        (0..l.pages)
            .map(|page| {
                let first = l.page_segs(page, page).filter(|s| !s.hidden).min_by_key(|s| s.para);
                first.map_or((usize::MAX, false), |s| (s.para, s.gy0 > 0.5))
            })
            .collect()
    }

    // --- page setup ------------------------------------------------------------------

    pub fn open_page_setup(&mut self, tab: usize) {
        self.end_hf_edit();
        let page = *self.doc.section_setup(self.sel.cursor.para).0;
        let apply = if self.doc.has_sections() { super::layout_ui::ApplyTo::Section } else { super::layout_ui::ApplyTo::Whole };
        let h = self.doc.section_headers(self.cur_section());
        let (first, numbering) = (h.different_first, h.numbering);
        self.page_setup = Some(SetupDialog { page, tab, first, odd_even: self.doc.odd_even, edit: None, orig: page, apply, numbering });
    }

    pub(super) fn page_setup_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.page_setup else { return };
        let sections = self.doc.has_sections();
        let pages = [(Icon::PageSize, "page.tab_page"), (Icon::Margins, "page.tab_margins"), (Icon::HeaderFooter, "page.tab_hf"), (Icon::Gridlines, "page.tab_grid")];
        let base = self.doc.normal_size_pt();
        let mut tab = d.tab;
        let res = widgets::nav_dialog(ctx, "doc-page-setup", tr("page.setup"), &pages, &mut tab, egui::vec2(500.0, 400.0), |ui, page| {
            match page {
                0 => {
                    let mut landscape = d.page.landscape();
                    paper_ui(ui, &mut d.page.width, &mut d.page.height, landscape);
                    print::orientation_ui(ui, &mut landscape);
                    d.page.set_landscape(landscape);
                    v_align_ui(ui, &mut d.page.v_align);
                }
                1 => {
                    let p = &mut d.page;
                    let paper = (p.width, p.height);
                    margins_ui(ui, [&mut p.margin_top, &mut p.margin_bottom, &mut p.margin_left, &mut p.margin_right, &mut p.margin_header, &mut p.margin_footer], paper);
                    binding_ui(ui, p);
                }
                3 => super::asian_ui::grid_ui(ui, &mut d.page, base),
                _ => {
                    ui.checkbox(&mut d.first, tr("hf.different_first"));
                    ui.checkbox(&mut d.odd_even, tr("hf.odd_even"));
                    ui.add_space(12.0);
                    numbering_ui(ui, &mut d.numbering);
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.button(tr("hf.edit_header")).clicked() {
                            d.edit = Some(true);
                        }
                        if ui.button(tr("hf.edit_footer")).clicked() {
                            d.edit = Some(false);
                        }
                    });
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(tr("hf.hint")).weak());
                }
            }
            if page != 2 {
                ui.add_space(12.0);
                super::layout_ui::apply_to_ui(ui, &mut d.apply, sections);
            }
        });
        d.tab = tab;
        let edit = d.edit;
        let apply = res == DialogResult::Ok || edit.is_some();
        let close = res != DialogResult::None || edit.is_some();
        if close {
            if let Some(d) = self.page_setup.take().filter(|_| apply) {
                self.apply_page_setup(d.page, d.orig, (d.first, d.odd_even), d.apply);
                let (s, sel) = (self.cur_section(), self.sel);
                self.doc.set_page_numbering(s, d.numbering, sel);
                if let Some(header) = d.edit {
                    self.relayout(ctx);
                    self.edit_hf_at_caret(header);
                }
            }
        }
    }
}

/// Where the text sits between the top and bottom margins.
fn v_align_ui(ui: &mut egui::Ui, v: &mut PageVAlign) {
    const ALL: [(PageVAlign, &str); 4] = [(PageVAlign::Top, "page.valign_top"), (PageVAlign::Center, "page.valign_center"), (PageVAlign::Justify, "page.valign_justify"), (PageVAlign::Bottom, "page.valign_bottom")];
    widgets::section(ui, tr("page.valign"));
    let cur = ALL.iter().find(|a| a.0 == *v).map_or("", |a| tr(a.1));
    egui::ComboBox::from_id_salt("page_valign").selected_text(cur).width(180.0).show_ui(ui, |ui| {
        for (a, key) in ALL {
            ui.selectable_value(v, a, tr(key));
        }
    });
}

/// Gutter and mirrored margins.
fn binding_ui(ui: &mut egui::Ui, p: &mut PageSetup) {
    widgets::section(ui, tr("page.binding"));
    egui::Grid::new("binding").num_columns(2).min_col_width(110.0).spacing([10.0, 8.0]).show(ui, |ui| {
        ui.label(tr("page.gutter"));
        let mut cm = p.gutter * 2.54 / 72.0;
        if ui.add(egui::DragValue::new(&mut cm).range(0.0..=10.0).speed(0.05).fixed_decimals(2).suffix(" cm")).changed() {
            p.gutter = cm * 72.0 / 2.54;
        }
        ui.end_row();
        ui.label(tr("page.gutter_pos"));
        ui.add_enabled_ui(!p.mirror, |ui| {
            let name = |top: bool| tr(if top { "page.gutter_top" } else { "page.gutter_left" });
            egui::ComboBox::from_id_salt("gutter_pos").selected_text(name(p.gutter_top && !p.mirror)).width(160.0).show_ui(ui, |ui| {
                for top in [false, true] {
                    ui.selectable_value(&mut p.gutter_top, top, name(top));
                }
            });
        });
        ui.end_row();
        ui.label(tr("page.multiple"));
        let name = |m: bool| tr(if m { "page.mirror" } else { "page.normal" });
        egui::ComboBox::from_id_salt("page_multiple").selected_text(name(p.mirror)).width(160.0).show_ui(ui, |ui| {
            for m in [false, true] {
                ui.selectable_value(&mut p.mirror, m, name(m));
            }
        });
        ui.end_row();
    });
    if p.mirror {
        ui.label(egui::RichText::new(tr("page.mirror_hint")).weak());
    }
}

/// Page number format and start of a section.
fn numbering_ui(ui: &mut egui::Ui, n: &mut apex_doc::PageNumbering) {
    use apex_doc::NoteFormat;
    const FORMATS: [(NoteFormat, &str); 5] = [
        (NoteFormat::Arabic, "fnote.fmt_arabic"),
        (NoteFormat::LowerRoman, "fnote.fmt_lower_roman"),
        (NoteFormat::UpperRoman, "fnote.fmt_upper_roman"),
        (NoteFormat::LowerLetter, "fnote.fmt_lower_letter"),
        (NoteFormat::UpperLetter, "fnote.fmt_upper_letter"),
    ];
    ui.label(egui::RichText::new(tr("hf.page_numbers")).strong());
    egui::Grid::new("page_num_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        ui.label(tr("fnote.format"));
        let cur = FORMATS.iter().position(|f| f.0 == n.format).unwrap_or(0);
        egui::ComboBox::from_id_salt("page_num_fmt").selected_text(format!("{}  ({})", tr(FORMATS[cur].1), n.format.label(1))).show_ui(ui, |ui| {
            for (f, key) in FORMATS {
                ui.selectable_value(&mut n.format, f, format!("{}  ({}, {}, {})", tr(key), f.label(1), f.label(2), f.label(3)));
            }
        });
        ui.end_row();
        ui.label(tr("hf.numbering"));
        ui.vertical(|ui| {
            let mut restart = n.start.is_some();
            ui.radio_value(&mut restart, false, tr("hf.continue_numbering"));
            ui.horizontal(|ui| {
                ui.radio_value(&mut restart, true, tr("hf.start_at"));
                let mut v = n.start.unwrap_or(1);
                let r = ui.add_enabled(restart, egui::DragValue::new(&mut v).range(0..=9999));
                if r.changed() || restart {
                    n.start = Some(v);
                }
            });
            if !restart {
                n.start = None;
            }
        });
        ui.end_row();
    });
}
