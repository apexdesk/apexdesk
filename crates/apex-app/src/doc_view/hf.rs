//! Headers and footers on the pages. Each one is laid out as a small
//! document of its own (same styles, the page's width) in a cached view,
//! and painted at the top or bottom of the pages that show it. Editing
//! one swaps it in as the view's document (so every command works in it)
//! with the page's body dimmed behind; leaving stores it back as one
//! undo step.

use std::sync::Arc;

use apex_doc::{Document, HfSlot, PageSetup, Paragraph, Pos, Selection, Story, story_is_empty};
use eframe::egui::{self, Color32, Pos2, Rect, pos2, vec2};

use super::{DocView, Layout, PT, Segment, seg_geometry};
use crate::i18n::tr;

/// Laid out headers and footers.
/// The page a header or footer is shown on, for its page fields.
#[derive(Clone, Debug, PartialEq, Default)]
pub(super) struct StoryPage {
    /// Page (1-based) and number of pages.
    pub page: usize,
    pub pages: usize,
    /// The number the page shows.
    pub label: String,
}

impl StoryPage {
    /// Page `page` (0-based) of layout `l`.
    pub fn of(l: &Layout, page: usize) -> StoryPage {
        StoryPage { page: page + 1, pages: l.pages, label: l.labels.get(page).cloned().unwrap_or_else(|| (page + 1).to_string()) }
    }
}

#[derive(Default)]
pub(super) struct HfCache {
    /// Styles and fonts of the layouts (they're dropped when they change).
    key: (u64, u64),
    items: Vec<Entry>,
}

struct Entry {
    story: Story,
    setup: PageSetup,
    /// Page number and count (0 when the story has no page fields).
    page: StoryPage,
    zoom: u32,
    view: Box<DocView>,
    height: f32,
}

/// A header or footer being edited: the body is set aside meanwhile.
pub(super) struct HfEdit {
    body: Box<DocView>,
    section: usize,
    slot: HfSlot,
    /// The body page it's edited on.
    page: usize,
    revision: u64,
}

fn has_page_fields(st: &[Paragraph]) -> bool {
    st.iter().any(|p| p.runs.iter().any(|r| r.attrs.field.is_some()))
}

impl DocView {
    /// A header or footer as a document laid out `setup`'s width, from the
    /// top of its page (footers are moved up to fit).
    fn story_doc(&self, story: &[Paragraph], setup: &PageSetup) -> Document {
        let mut d = Document::from_paragraphs(story.to_vec());
        d.styles = self.doc.styles.clone();
        d.default_font = self.doc.default_font.clone();
        // Headers and footers of vertical pages stay horizontal (as in Word).
        // Their text starts at the top whatever the body's vertical alignment.
        d.page = PageSetup { margin_top: 0.0, margin_bottom: 0.0, height: setup.height + 2000.0, text_flow: None, v_align: apex_doc::PageVAlign::Top, ..*setup };
        d
    }

    /// Height (pixels) of the text on a view's first page.
    fn story_height(&self) -> f32 {
        let l = &self.layout;
        l.page_segs(0, 0).map(|s| s.y + s.gy1 - s.gy0).fold(0.0, f32::max)
    }

    /// Runs `f` with the laid out view of a header or footer and its height.
    /// `key` is the body layout's styles and fonts.
    fn with_story<R>(&self, ctx: &egui::Context, key: (u64, u64), story: &Story, setup: &PageSetup, page: StoryPage, z: f32, f: impl FnOnce(&DocView, f32) -> R) -> R {
        let mut c = self.hf_cache.borrow_mut();
        if c.key != key {
            c.items.clear();
            c.key = key;
        }
        let page = if has_page_fields(story) { page } else { StoryPage::default() };
        let found = c.items.iter().position(|e| Arc::ptr_eq(&e.story, story) && e.setup == *setup && e.page == page && e.zoom == z.to_bits());
        let i = match found {
            Some(i) => i,
            None => {
                let mut v = Box::new(DocView::new(self.story_doc(story, setup)));
                v.zoom = z;
                v.file_name.clone_from(&self.file_name);
                v.story_page = Some(page.clone());
                v.relayout(ctx);
                let height = v.story_height();
                if c.items.len() >= 48 {
                    c.items.remove(0);
                }
                c.items.push(Entry { story: story.clone(), setup: *setup, page: page.clone(), zoom: z.to_bits(), view: v, height });
                c.items.len() - 1
            }
        };
        let e = &c.items[i];
        f(&e.view, e.height)
    }

    /// The text, notes, tables and floating objects of page `page` of
    /// layout `l` (no page, caret or selection), the page's top left at
    /// `page_min`.
    pub(super) fn paint_content(&self, p: &egui::Painter, l: &Layout, page: usize, page_min: Pos2, z: f32) {
        let g = l.geo_of(page);
        let o = page_min + g.margin;
        let segs: Vec<Segment> = l.page_segs(page, page).filter(|s| !s.hidden).copied().collect();
        let frags = l.frags_on(page, page);
        self.paint_floats(p, l, page, page_min, z, true);
        // A vertical page is painted as laid out and turned.
        let turn = g.vertical.then_some(super::tategaki::Turn { o, w: g.content.x });
        let from = super::tategaki::mark(p);
        let outer = p.clip_rect();
        let p = &if turn.is_some() { super::tategaki::unclipped(p) } else { p.clone() };
        self.paint_notes(p, l, page, o, z);
        let origin = |_: usize| o;
        self.paint_table_back(p, l, frags, &origin, None);
        for seg in l.ghosts_on(page, page).iter().chain(&segs) {
            let so = o + vec2(seg.x, 0.0);
            let (_, clip) = seg_geometry(l, seg, &l.paras[seg.para], so);
            let cp = p.with_clip_rect(clip.intersect(p.clip_rect()));
            self.paint_seg_back(&cp, p, l, seg, so, z);
            self.paint_seg_text(&cp, p, l, seg, so, z);
        }
        self.paint_table_borders(p, l, frags, &origin, z, false);
        if let Some(t) = turn {
            super::tategaki::turn_painted(p, from, &t, outer);
        }
        self.paint_floats(p, l, page, page_min, z, false);
    }

    /// The header and footer of page `page` of layout `l` (the page at
    /// `rect`); `skip` leaves out the header (true) or the footer.
    pub(super) fn paint_page_hf(&self, p: &egui::Painter, ctx: &egui::Context, l: &Layout, page: usize, rect: Rect, z: f32, skip: Option<bool>) {
        let g = l.geo_of(page);
        let sects = self.doc.sections();
        let Some(setup) = sects.get(g.sect).map(|s| *s.page) else { return };
        let (hs, fs) = self.doc.page_slots(g.sect, g.first, page + 1);
        for (slot, header) in [(hs, true), (fs, false)] {
            if skip == Some(header) {
                continue;
            }
            let Some(st) = self.doc.hf_story(g.sect, slot).filter(|st| !story_is_empty(st)) else { continue };
            self.with_story(ctx, (l.key.3, l.key.4), st, &setup, StoryPage::of(l, page), z, |v, h| {
                let pt = PT * z;
                let top = if header { rect.min.y + setup.margin_header * pt } else { rect.max.y - setup.margin_footer * pt - h };
                v.paint_content(p, &v.layout, 0, pos2(rect.min.x, top), z);
            });
        }
    }

    /// Makes room for headers and footers taller than the top and bottom
    /// margins: the text of their sections starts below (ends above) them.
    pub(super) fn fit_headers(&self, ctx: &egui::Context, key: (u64, u64), z: f32, sects: &mut [super::sections::Sect]) {
        if self.story_page.is_some() || !self.doc.has_headers() {
            return;
        }
        let pt = PT * z;
        for (k, sect) in sects.iter_mut().enumerate() {
            let setup = sect.page;
            let mut page = setup;
            let h = self.doc.section_headers(k);
            let kinds = [Some(0u8), h.different_first.then_some(1), self.doc.odd_even.then_some(2)];
            for kind in kinds.into_iter().flatten() {
                for header in [true, false] {
                    let Some(st) = self.doc.hf_story(k, HfSlot::of(header, kind)).filter(|st| !story_is_empty(st)) else { continue };
                    let height = self.with_story(ctx, key, st, &setup, StoryPage::default(), z, |_, h| h) / pt;
                    if header {
                        page.margin_top = page.margin_top.max(setup.margin_header + height);
                    } else {
                        page.margin_bottom = page.margin_bottom.max(setup.margin_footer + height);
                    }
                }
            }
            if page != setup {
                sect.set_page(page, pt);
            }
        }
    }

    // --- editing --------------------------------------------------------------------------

    /// The body view while a header or footer is edited.
    pub(super) fn hf_body(&self) -> Option<&DocView> {
        self.hf_edit.as_ref().map(|e| &*e.body)
    }

    /// The document with the header or footer being edited stored in it.
    pub fn is_modified(&self) -> bool {
        // An edited Markdown source is a change the document doesn't have yet.
        if self.md.code.as_ref().is_some_and(|c| c.dirty()) {
            return true;
        }
        match &self.hf_edit {
            Some(e) => e.body.doc.modified || self.doc.revision != e.revision,
            None => self.doc.modified,
        }
    }

    /// The header (or footer) zone of a body page at canvas point `local`:
    /// (page, header?).
    pub(super) fn hf_zone_at(&self, local: Pos2) -> Option<(usize, bool)> {
        let l = &self.layout;
        let page = l.page_at(local.x - self.page_x, local.y);
        let g = l.geo_of(page);
        let top = g.top;
        let y = local.y - top;
        if y < 0.0 || y > g.size.y {
            return None;
        }
        if y < g.margin.y {
            Some((page, true))
        } else if y > g.margin.y + g.content.y {
            Some((page, false))
        } else {
            None
        }
    }

    /// Starts editing the header or footer shown on body page `page`.
    pub(super) fn start_hf_edit(&mut self, page: usize, header: bool) {
        self.end_hf_edit();
        let g = self.layout.geo_of(page);
        let (hs, fs) = self.doc.page_slots(g.sect, g.first, page + 1);
        let slot = if header { hs } else { fs };
        let Some(setup) = self.doc.sections().get(g.sect).map(|s| *s.page) else { return };
        let story: Vec<Paragraph> = self.doc.hf_story(g.sect, slot).map(|s| s.to_vec()).unwrap_or_else(|| vec![Paragraph::new("")]);
        let mut d = self.story_doc(&story, &setup);
        d.page.margin_top = if header { setup.margin_header } else { (setup.height - setup.margin_footer - 14.0).max(0.0) };
        d.user = self.doc.user.clone();
        d.track_changes = self.doc.track_changes;
        let revision = d.revision;
        let body_doc = std::mem::replace(&mut self.doc, d);
        let mut body = Box::new(DocView::new(body_doc));
        body.layout = std::mem::take(&mut self.layout);
        body.sel = self.sel;
        body.zoom = self.zoom;
        body.file_name.clone_from(&self.file_name);
        self.story_page = Some(StoryPage::of(&body.layout, page));
        self.hf_edit = Some(Box::new(HfEdit { body, section: g.sect, slot, page, revision }));
        let end = self.doc.paragraphs.len() - 1;
        let caret = if header { Pos::new(0, 0) } else { Pos::new(end, self.doc.paragraphs[end].len()) };
        self.sel = Selection::caret(caret);
        self.scroll_to_caret = true;
        self.focus = true;
    }

    /// The document to keep a copy of: the body while a header or footer
    /// is edited (its changes join it when the editing ends).
    pub fn backup_doc(&self) -> Document {
        match &self.hf_edit {
            Some(e) => e.body.doc.clone(),
            // Changed code is part of it.
            None => self.code_backup().unwrap_or_else(|| self.doc.clone()),
        }
    }

    /// The whole document as it is now: while a header or footer is
    /// edited, the body with that story in it (as ending the editing would
    /// leave it). The QA compares it before and after an action.
    #[cfg(test)]
    pub fn whole_doc(&self) -> std::borrow::Cow<'_, Document> {
        let Some(e) = &self.hf_edit else { return std::borrow::Cow::Borrowed(&self.doc) };
        let mut body = e.body.doc.clone();
        if self.doc.revision != e.revision {
            store_story(&mut body, e.section, e.slot, self.doc.paragraphs.clone(), e.body.sel);
        }
        std::borrow::Cow::Owned(body)
    }

    /// Changes to the document kept by `backup_doc`.
    pub fn backup_revision(&self) -> u64 {
        self.hf_edit.as_ref().map_or(self.doc.revision, |e| e.body.doc.revision) ^ self.code_revision()
    }

    /// Stores the header or footer being edited in the document and goes
    /// back to the body.
    pub fn end_hf_edit(&mut self) {
        let Some(e) = self.hf_edit.take() else { return };
        let HfEdit { mut body, section, slot, revision, .. } = *e;
        let story = std::mem::replace(&mut self.doc, std::mem::take(&mut body.doc));
        self.layout = std::mem::take(&mut body.layout);
        self.sel = body.sel;
        self.story_page = None;
        if story.revision != revision {
            store_story(&mut self.doc, section, slot, story.paragraphs, self.sel);
        }
        self.hf_cache.borrow_mut().items.clear();
        self.scroll_to_caret = true;
        self.focus = true;
    }

    /// The size of the body page a header or footer is edited on.
    pub(super) fn hf_page_size(&self) -> Option<egui::Vec2> {
        let e = self.hf_edit.as_ref()?;
        Some(e.body.layout.geo_of(e.page).size)
    }

    /// Whether canvas point `local` is in the header (or footer) area being
    /// edited.
    pub(super) fn in_edited_hf(&self, local: Pos2) -> bool {
        let Some(e) = &self.hf_edit else { return false };
        let g = e.body.layout.geo_of(e.page);
        let y = local.y - self.layout.geo_of(0).top;
        if e.slot.is_header() { y < g.margin.y } else { y > g.margin.y + g.content.y }
    }

    /// Moves an edited footer up so that it ends at the footer margin.
    pub(super) fn fit_edited_footer(&mut self) -> bool {
        let Some(e) = &self.hf_edit else { return false };
        if e.slot.is_header() {
            return false;
        }
        let Some(setup) = e.body.doc.sections().get(e.section).map(|s| *s.page) else { return false };
        let h = (self.story_height() / (PT * self.zoom)).max(14.0);
        let top = (setup.height - setup.margin_footer - h).max(0.0);
        if (self.doc.page.margin_top - top).abs() > 0.5 {
            self.doc.page.margin_top = top;
            return true;
        }
        false
    }

    /// The body page behind the header or footer being edited, dimmed.
    pub(super) fn paint_hf_backdrop(&self, p: &egui::Painter, ctx: &egui::Context, rect: Rect) {
        let Some(e) = &self.hf_edit else { return };
        let b = &e.body;
        if b.layout.pages > e.page {
            b.paint_content(p, &b.layout, e.page, rect.min, self.zoom);
            b.paint_page_hf(p, ctx, &b.layout, e.page, rect, self.zoom, Some(e.slot.is_header()));
        }
        p.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(255, 255, 255, 150));
        // The edge of the header or footer area.
        let g = self.layout.geo_of(0);
        let pt = PT * self.zoom;
        let y = if e.slot.is_header() { rect.min.y + e.body.doc.sections().get(e.section).map_or(g.margin.y, |s| s.page.margin_top * pt) } else { rect.min.y + g.margin.y };
        let blue = Color32::from_rgb(0x2B, 0x57, 0x9A);
        p.extend(egui::Shape::dashed_line(&[pos2(rect.min.x, y), pos2(rect.max.x, y)], egui::Stroke::new(1.0, blue), 4.0, 3.0));
        let label = format!("{} — {}", tr(hf_label(e.slot)), tr("hf.section").replace("{}", &(e.section + 1).to_string()));
        let font = egui::FontId::proportional(11.0);
        let gal = p.layout_no_wrap(label, font, Color32::WHITE);
        let r = Rect::from_min_size(pos2(rect.min.x + 4.0, if e.slot.is_header() { y } else { y - gal.size().y - 4.0 }), gal.size() + vec2(8.0, 4.0));
        p.rect_filled(r, 2.0, blue);
        p.galley(r.min + vec2(4.0, 2.0), gal, Color32::WHITE);
    }

    /// The bar above the page while a header or footer is edited.
    pub(super) fn hf_bar(&mut self, ui: &mut egui::Ui) {
        let Some(e) = &self.hf_edit else { return };
        let (section, slot, page) = (e.section, e.slot, e.page);
        let body = &e.body.doc;
        let mut first = body.section_headers(section).different_first;
        let mut odd_even = body.odd_even;
        let linked = body.hf_linked(section, slot);
        let mut action = None;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tr(hf_label(slot))).strong());
            if section > 0 {
                ui.label(format!("· {}", tr("hf.section").replace("{}", &(section + 1).to_string())));
            }
            ui.separator();
            if ui.checkbox(&mut first, tr("hf.different_first")).changed() || ui.checkbox(&mut odd_even, tr("hf.odd_even")).changed() {
                action = Some(0);
            }
            if section > 0 {
                let mut link = linked;
                if ui.checkbox(&mut link, tr("hf.link_previous")).changed() {
                    action = Some(if link { 1 } else { 2 });
                }
            }
            ui.separator();
            let other = if slot.is_header() { "hf.goto_footer" } else { "hf.goto_header" };
            if ui.button(tr(other)).clicked() {
                action = Some(3);
            }
            if ui.button(tr("hf.close")).clicked() {
                action = Some(4);
            }
        });
        // Esc goes back to the body (the canvas doesn't get it).
        if action.is_none() && ui.input(|i| i.key_pressed(egui::Key::Escape)) && !ui.ctx().any_popup_open() {
            action = Some(4);
        }
        let header = slot.is_header();
        match action {
            Some(0) => {
                self.end_hf_edit();
                let sel = self.sel;
                self.doc.set_hf_options(section, first, odd_even, sel);
                self.relayout(ui.ctx());
                self.start_hf_edit(page.min(self.layout.pages.saturating_sub(1)), header);
            }
            Some(1) => {
                // Linking drops this section's own header or footer.
                self.end_hf_edit();
                let sel = self.sel;
                self.doc.set_hf_story(section, slot, None, sel);
                self.relayout(ui.ctx());
                self.start_hf_edit(page, header);
            }
            Some(2) => {
                // Unlinking starts from a copy of the previous one.
                self.end_hf_edit();
                let copy = self.doc.hf_story(section, slot).map(|s| s.to_vec()).unwrap_or_else(|| vec![Paragraph::new("")]);
                let sel = self.sel;
                self.doc.set_hf_story(section, slot, Some(copy), sel);
                self.relayout(ui.ctx());
                self.start_hf_edit(page, header);
            }
            Some(3) => {
                self.end_hf_edit();
                self.relayout(ui.ctx());
                self.start_hf_edit(page, !header);
            }
            Some(4) => self.end_hf_edit(),
            _ => {}
        }
    }

    /// Edits the header or footer of the caret's page.
    pub fn edit_hf_at_caret(&mut self, header: bool) {
        if self.hf_edit.is_some() {
            let page = self.hf_edit.as_ref().map_or(0, |e| e.page);
            self.start_hf_edit(page, header);
            return;
        }
        let page = self.layout.segs_of(self.sel.cursor.para).first().map_or(0, |s| s.page);
        self.start_hf_edit(page, header);
    }

    /// Puts a centered page number in the footer of every page.
    pub fn insert_page_number(&mut self, header: bool) {
        self.end_hf_edit();
        let mut p = Paragraph::new("");
        p.style.align = apex_doc::Align::Center;
        p.push(&apex_doc::OBJECT.to_string(), apex_doc::Attrs { field: Some(Arc::new(apex_doc::Field::new(apex_doc::FieldKind::Page))), ..Default::default() });
        let slot = HfSlot::of(header, 0);
        let mut story = self.doc.hf_story(0, slot).map(|s| s.to_vec()).filter(|s| !story_is_empty(s)).unwrap_or_default();
        story.push(p);
        let sel = self.sel;
        self.doc.set_hf_story(0, slot, Some(story), sel);
    }
}

/// Puts the edited story of `slot` of `section` in `body`, as an undo step.
fn store_story(body: &mut Document, section: usize, slot: HfSlot, paras: Vec<Paragraph>, sel: Selection) {
    // A linked header is the previous section's: that one changes.
    let owner = (0..=section).rev().find(|&s| body.section_headers(s).get(slot).is_some()).unwrap_or(section);
    let value = if story_is_empty(&paras) && owner == section && section == 0 { None } else { Some(paras) };
    body.set_hf_story(owner, slot, value, sel);
}

fn hf_label(slot: HfSlot) -> &'static str {
    match slot {
        HfSlot::Header => "hf.header",
        HfSlot::Footer => "hf.footer",
        HfSlot::FirstHeader => "hf.first_header",
        HfSlot::FirstFooter => "hf.first_footer",
        HfSlot::EvenHeader => "hf.even_header",
        HfSlot::EvenFooter => "hf.even_footer",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_whole_document_while_a_header_is_edited() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let frame = || ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        frame();
        frame();
        let mut v = DocView::new(Document::from_plain_text("Body one\nBody two"));
        v.layout = v.build_layout(&ctx, 1.0, (1, 0, 0, 0, 0), Layout::default());
        let before = v.doc.clone();
        v.edit_hf_at_caret(true);
        assert!(v.hf_edit.is_some());
        // Only editing: the document is as it was.
        assert_eq!((&v.whole_doc().paragraphs, &v.whole_doc().headers), (&before.paragraphs, &before.headers));
        // Typed in the header: there, not in the body.
        let sel = v.sel;
        v.doc.insert_text(sel, "Head", None);
        let whole = v.whole_doc().into_owned();
        assert_eq!(whole.paragraphs, before.paragraphs);
        assert_eq!(whole.hf_story(0, HfSlot::Header).map(|s| s[0].text.clone()), Some("Head".to_string()));
    }
}
