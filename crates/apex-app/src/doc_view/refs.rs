//! Fields, note marks and tables of contents on the page: the values they
//! show (worked out on every layout, page numbers included), the
//! placeholders that make room for them in the text, their painting, and
//! the References menu with its dialogs.

use std::collections::HashMap;
use std::sync::Arc;

use apex_doc::{Field, FieldCtx, FieldKind, NoteKind, NoteRestart, OBJECT, Refs, Run, TocOpts, objects_of, para_has_objects};
use eframe::egui::text::{ByteIndex, LayoutJob, LayoutSection};

use super::*;
use crate::widgets::DialogResult;

/// Everything this module and the notes and navigation panes keep in the view.
#[derive(Default)]
pub(super) struct RefState {
    pub dialog: Option<RefDialog>,
    pub nav: Option<super::nav::NavPane>,
    pub pane: Option<super::notes::NotesPane>,
    /// For work outside frames (saving): layouts need fonts.
    pub ctx: Option<egui::Context>,
}

/// What the layout keeps about fields and notes.
#[derive(Default)]
pub(super) struct RefLayout {
    /// Shown text of every note mark and field of the body.
    pub values: HashMap<Pos, String>,
    pub notes: super::notes::NoteLayout,
}

/// A copy of paragraph `p` (number `i`) as it is laid out: note marks become
/// superscript fields, and fields carry the text they show so the layout
/// makes room for it (and its cache sees when that text changes).
pub(super) fn shown_para(p: &Paragraph, i: usize, values: &HashMap<Pos, String>) -> Paragraph {
    let mut out = Paragraph { runs: Vec::with_capacity(p.runs.len() + 2), ..p.clone() };
    for (s, e, a) in p.spans() {
        if a.note.is_none() && a.field.is_none() || e == s {
            out.runs.push(Run { len: e - s, attrs: a.clone() });
            continue;
        }
        for (o, c) in p.text[s..e].char_indices() {
            let mut attrs = a.clone();
            if c == OBJECT {
                let cached = a.field.as_ref().map(|f| f.cached.clone()).unwrap_or_default();
                let text = values.get(&Pos::new(i, s + o)).cloned().unwrap_or(cached);
                if attrs.note.take().is_some() {
                    attrs.superscript = true;
                    attrs.subscript = false;
                }
                attrs.field = Some(Arc::new(Field { cached: text, ..Field::new(FieldKind::Page) }));
            }
            out.runs.push(Run { len: c.len_utf8(), attrs });
        }
    }
    out
}

fn text_width(ctx: &egui::Context, text: &str, font: &egui::FontId) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    ctx.fonts_mut(|f| f.layout_job(LayoutJob::simple_singleline(text.to_string(), font.clone(), Color32::BLACK))).size().x
}

/// Text format of a field's character: an invisible no-break space as wide
/// as the text it shows, on the same line box as the text around it.
pub(super) fn placeholder(ctx: &egui::Context, base: &TextFormat, text: &str, nbsp: f32) -> TextFormat {
    let w = text_width(ctx, text, &base.font_id);
    let mut f = base.clone();
    f.font_id = fonts::font(fonts::Face::Sans, false, (w / nbsp).max(0.5));
    f.color = Color32::TRANSPARENT;
    f.underline = Stroke::NONE;
    f.strikethrough = Stroke::NONE;
    f.italics = false;
    f
}

/// Width of a tab per unit of font size.
fn tab_ratio(ctx: &egui::Context) -> f32 {
    let font = fonts::font(fonts::Face::Sans, false, 100.0);
    (ctx.fonts_mut(|f| f.glyph_width(&font, '\t')) / 100.0).max(0.05)
}

/// Makes the tab of a table of contents entry reach from the text to the
/// right edge, so the page number after it ends at the margin.
pub(super) fn toc_tab(ctx: &egui::Context, job: &mut LayoutJob, width: f32) {
    let Some(tab) = job.text.rfind('\t') else { return };
    let Some(si) = job.sections.iter().position(|s| (s.byte_range.start.0..s.byte_range.end.0).contains(&tab)) else { return };
    let sec = job.sections.remove(si);
    let (start, end) = (sec.byte_range.start.0, sec.byte_range.end.0);
    let r = |a: usize, b: usize| ByteIndex(a)..ByteIndex(b);
    let mut parts = Vec::new();
    if start < tab {
        parts.push(LayoutSection { leading_space: sec.leading_space, byte_range: r(start, tab), format: sec.format.clone() });
    }
    let ti = si + parts.len();
    let mut tf = sec.format.clone();
    let ratio = tab_ratio(ctx);
    tf.font_id = fonts::font(fonts::Face::Sans, false, 1.0 / ratio);
    tf.color = Color32::TRANSPARENT;
    tf.underline = Stroke::NONE;
    parts.push(LayoutSection { leading_space: if parts.is_empty() { sec.leading_space } else { 0.0 }, byte_range: r(tab, tab + 1), format: tf });
    if tab + 1 < end {
        parts.push(LayoutSection { leading_space: 0.0, byte_range: r(tab + 1, end), format: sec.format });
    }
    job.sections.splice(si..si, parts);
    // Lay out once with a 1 px tab to measure the rest of its row.
    let g = ctx.fonts_mut(|f| f.layout_job(job.clone()));
    let ci = job.text[..tab].chars().count();
    let mut start = 0usize;
    for row in &g.rows {
        let n = row.row.glyphs.len() + usize::from(row.ends_with_newline);
        if ci < start + row.row.glyphs.len() {
            let tab_w = row.row.glyphs[ci - start].advance_width;
            let need = (width - (row.row.size.x - tab_w) - 1.0).max(4.0);
            job.sections[ti].format.font_id.size = need / ratio;
            return;
        }
        start += n;
    }
}

/// The glyph of char `ci` of a galley: its row and the glyph.
fn glyph_of(galley: &Galley, ci: usize) -> Option<(&egui::epaint::text::PlacedRow, &egui::epaint::text::Glyph)> {
    let mut start = 0usize;
    for row in &galley.rows {
        let n = row.row.glyphs.len() + usize::from(row.ends_with_newline);
        if ci < start + row.row.glyphs.len() {
            return Some((row, &row.row.glyphs[ci - start]));
        }
        start += n;
    }
    None
}

impl DocView {
    /// Paints the values of the note marks and fields of `para` (paragraph
    /// `idx` of the body, `None` for note text) laid out as `pl` at
    /// `galley_pos`, on the rows between galley y `gy.0` and `gy.1`; and the
    /// dot leaders of a table of contents entry.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn paint_objects(&self, p: &egui::Painter, para: &Paragraph, idx: Option<usize>, pl: &ParaLayout, galley_pos: Pos2, gy: (f32, f32), values: &HashMap<Pos, String>, z: f32) {
        let in_rows = |row: &egui::epaint::text::PlacedRow| row.pos.y + row.row.size.y > gy.0 + 0.5 && row.pos.y < gy.1 - 0.5;
        let has_images = para.runs.iter().any(|r| r.attrs.image.is_some());
        if let Some(toc) = para.style.toc.filter(|t| t.opts.leader && t.opts.right_align && t.opts.page_numbers)
            && let Some(tab) = para.text.rfind('\t')
            && let Some((row, g)) = glyph_of(&pl.galley, char_count(&para.text, tab))
            && in_rows(row)
        {
            let _ = toc;
            // The baseline of the text before the tab.
            let base = row.row.glyphs.iter().take_while(|x| x.pos.x < g.pos.x).last().map_or(row.row.size.y * 0.8, |x| x.pos.y);
            let step = 4.0 * z;
            let y = galley_pos.y + row.pos.y + base - 0.8 * z;
            let (x0, x1) = (galley_pos.x + row.pos.x + g.pos.x + step * 0.5, galley_pos.x + row.pos.x + g.pos.x + g.advance_width - step * 0.5);
            // On a grid, so the dots of all entries line up.
            let mut x = (x0 / step).ceil() * step;
            while x <= x1 {
                p.circle_filled(pos2(x, y), (0.6 * z).max(0.5), Paper::TEXT);
                x += step;
            }
        }
        if !para_has_objects(para) {
            return;
        }
        for (byte, a) in objects_of(para) {
            let Some((row, g)) = glyph_of(&pl.galley, char_count(&para.text, byte)) else { continue };
            if !in_rows(row) {
                continue;
            }
            let text = match idx.and_then(|i| values.get(&Pos::new(i, byte))) {
                Some(v) => v.clone(),
                None => a.field.as_ref().map(|f| f.cached.clone()).unwrap_or_default(),
            };
            if text.is_empty() {
                continue;
            }
            let shown = Attrs { superscript: a.superscript || a.note.is_some(), note: None, field: None, image: None, ..a.clone() };
            let mut fmt = self.text_format(&para.style, &shown, z);
            if has_images && fmt.valign == egui::Align::Center {
                fmt.valign = egui::Align::BOTTOM;
            }
            // Where the line box of the text starts in the row (see how egui
            // places glyphs): same format, same box, same baseline.
            let box_top = g.pos.y - g.font_face_ascent - 0.5 * (g.font_height - g.font_face_height);
            let mut job = LayoutJob::default();
            job.append(&text, 0.0, fmt);
            let vg = p.layout_job(job);
            let right = para.style.toc.is_some() && a.field.as_ref().is_some_and(|f| matches!(f.kind, FieldKind::PageRef(_)));
            let x = galley_pos.x + row.pos.x + g.pos.x + if right { g.advance_width - vg.size().x } else { 0.0 };
            p.galley(pos2(x, galley_pos.y + row.pos.y + box_top), vg, Paper::TEXT);
        }
    }

    /// Page (0-based) of position `p` in layout `l`.
    pub(super) fn page_in(&self, l: &Layout, p: Pos) -> Option<usize> {
        let pl = l.paras.get(p.para)?;
        let text = &self.doc.paragraphs.get(p.para)?.text;
        let r = pl.galley.pos_from_cursor(CCursor::new(char_count(text, p.offset)));
        let segs = l.segs_of(p.para);
        segs.iter().rev().find(|s| r.min.y >= s.gy0 - 0.5).or(segs.first()).map(|s| s.page)
    }

    /// The text every note mark and field shows, with page numbers from
    /// layout `l` (without one, page fields keep their last value).
    pub(super) fn object_values(&self, refs: &Refs, l: Option<&Layout>) -> HashMap<Pos, String> {
        let doc = &self.doc;
        let mut out = HashMap::new();
        let mut page = usize::MAX;
        let mut on_page = 0;
        for &(pos, kind, n) in &refs.notes {
            let s = doc.notes.get(kind);
            let num = match l.and_then(|l| self.page_in(l, pos)) {
                Some(pg) if kind == NoteKind::Foot && s.restart == NoteRestart::EachPage => {
                    if pg != page {
                        (page, on_page) = (pg, 0);
                    }
                    on_page += 1;
                    on_page
                }
                _ => n,
            };
            out.insert(pos, s.format.label(num + s.start.max(1) as usize - 1));
        }
        let fields = doc.fields();
        if fields.is_empty() {
            return out;
        }
        let notes = out.clone();
        let (date, time) = common::date_time_now();
        let (y, mo, d, h, mi) = common::local_now();
        let months: Vec<&str> = tr("field.months").split('|').collect();
        let days: Vec<&str> = tr("field.days").split('|').collect();
        let page_of = |p: Pos| l.and_then(|l| self.page_in(l, p)).map(|x| x + 1);
        let note_label = |p: Pos| notes.get(&p).cloned();
        let sp = self.story_page.as_ref();
        let page_label = |p: usize| match sp {
            Some(s) if s.page == p => s.label.clone(),
            _ => l.and_then(|l| l.labels.get(p.wrapping_sub(1)).cloned()).unwrap_or_else(|| p.to_string()),
        };
        for (pos, f) in fields {
            let cx = FieldCtx {
                page: sp.map(|s| s.page.max(1)).or_else(|| page_of(pos)).unwrap_or(1),
                pages: sp.map_or(l.map_or(1, |l| l.pages), |s| s.pages.max(1)),
                now: (y, mo, d, h, mi, 0),
                date: &date,
                time: &time,
                months: &months,
                days: &days,
                file: &self.file_name,
                page_of: &page_of,
                page_label: &page_label,
                above: tr("xref.above"),
                below: tr("xref.below"),
                on_page: tr("xref.on_page"),
                note_label: &note_label,
                decimal: common::decimal(),
            };
            // Index entries show only with the formatting marks, as Word's hidden text.
            if matches!(f.kind, FieldKind::IndexEntry(_) | FieldKind::Authority { .. }) {
                // Shortened: the mark can't wrap.
                let code = f.instr().trim().to_string();
                let code = if code.chars().count() > 32 { format!("{}…", code.chars().take(31).collect::<String>()) } else { code };
                out.insert(pos, if self.show_marks { format!("{{{code}}}") } else { String::new() });
                continue;
            }
            let v = if l.is_none() && f.needs_pages() && self.story_page.is_none() { f.cached.clone() } else { refs.value(doc, &f, pos, &cx) };
            out.insert(pos, v);
        }
        out
    }

    /// Lays the document out until the page numbers that fields show stop
    /// changing (they make room for their text, which moves pages).
    pub(super) fn build_layout(&self, ctx: &egui::Context, z: f32, key: (u64, u32, u32, u64, u64), old: Layout) -> Layout {
        let mut l = self.build_layout_fields(ctx, z, key, old);
        l.line_nums = self.line_numbers(&l, z);
        l
    }

    fn build_layout_fields(&self, ctx: &egui::Context, z: f32, key: (u64, u32, u32, u64, u64), old: Layout) -> Layout {
        if !self.doc.has_objects() {
            return self.build_pass(ctx, z, key, old, None);
        }
        let refs = self.doc.refs();
        let mut vals = self.object_values(&refs, None);
        // Start from what was shown last time: usually right already.
        for (k, v) in &old.refs.values {
            if let Some(x) = vals.get_mut(k)
                && self.doc.field_at(*k).is_some_and(|(_, f)| f.needs_pages())
            {
                x.clone_from(v);
            }
        }
        let mut l = self.build_pass(ctx, z, key, old, Some(&vals));
        for _ in 0..3 {
            let new = self.object_values(&refs, Some(&l));
            if new == vals {
                break;
            }
            vals = new;
            l = self.build_pass(ctx, z, key, l, Some(&vals));
        }
        l.refs.values = vals;
        l
    }

    /// Whether some field shows other than the result its file keeps, which
    /// Word shows until the fields are updated (not page numbers, dates
    /// and times, which it updates itself).
    pub fn fields_stale(&self) -> bool {
        self.layout.refs.values.iter().any(|(&pos, v)| {
            self.doc.field_at(pos).is_some_and(|(_, f)| {
                !f.needs_pages() && !matches!(f.kind, FieldKind::Date | FieldKind::Time | FieldKind::IndexEntry(_) | FieldKind::Authority { .. }) && f.cached != *v
            })
        })
    }

    /// Values of the fields of the body as shown now, stored in the fields
    /// before saving so the file carries them.
    pub(super) fn store_field_values(&mut self) {
        if self.layout.refs.values.is_empty() {
            return;
        }
        let values = std::mem::take(&mut self.layout.refs.values);
        let rev = self.doc.revision;
        self.doc.store_field_values(&values);
        if self.doc.revision != rev {
            // Only the cached text changed: the layout stays valid.
            self.layout.key.0 = self.doc.revision;
        }
        self.layout.refs.values = values;
    }

    /// Updates the tables of contents, then the stored field values; before
    /// saving and printing, like other editors.
    pub fn update_for_output(&mut self) {
        if self.doc.toc_blocks().is_empty() && !self.doc.has_objects() {
            return;
        }
        let (changed, sel) = self.update_all_tocs();
        if changed {
            self.sel = sel;
        }
        let Some(ctx) = self.refs.ctx.clone() else { return };
        self.relayout(&ctx);
        self.store_field_values();
    }

    /// Updates the table of contents or field at the caret (F9, Update Field).
    pub(super) fn update_field(&mut self) {
        let c = self.sel.cursor;
        if self.doc.toc_at(c.para).is_some() {
            let a = self.doc.toc_at(c.para).map_or(c.para, |t| t.0);
            if self.doc.update_toc(c.para, self.sel, self.no_entries(c.para)) {
                self.set_sel(Selection::caret(Pos::new(a, 0)));
            } else {
                self.notices.push(Notice::info(tr("toc.up_to_date")));
            }
            return;
        }
        // Fields always show current values; refresh the time ones.
        self.layout.key.0 = u64::MAX;
    }

    /// Word's shortcuts for notes and fields. True if the key was used.
    pub(super) fn refs_key(&mut self, key: Key, m: Modifiers) -> bool {
        match key {
            Key::F if m.command && m.alt => self.insert_note(NoteKind::Foot),
            Key::D if m.command && m.alt => self.insert_note(NoteKind::End),
            Key::F9 if !m.command => self.update_field(),
            Key::X if m.alt && m.shift && !m.command => self.open_mark_entry_dialog(),
            Key::I if m.alt && m.shift && !m.command => self.open_mark_citation(),
            _ => return false,
        }
        true
    }

    /// Inserts a field at the caret.
    pub(super) fn insert_field(&mut self, f: Field) {
        self.clear_block();
        let s = self.doc.insert_field(self.sel, f);
        self.sel = Selection::caret(s.end());
        self.scroll_to_caret = true;
    }

    // --- menus -------------------------------------------------------------------------

    /// The References menu.
    /// The References tab of the toolstrip.
    pub(super) fn references_tab(&mut self, b: &mut crate::toolstrip::Body) {
        use crate::toolstrip;
        let a = b.accent();
        b.group("toolstrip.g.toc", Icon::Toc, |ui| {
            if toolstrip::large(ui, Icon::Toc, tr("toc.insert"), "", a, false).clicked() {
                self.open_toc_dialog();
            }
            let in_toc = self.doc.toc_at(self.sel.cursor.para).is_some();
            let has_toc = in_toc || !self.doc.toc_blocks().is_empty();
            toolstrip::column(ui, |ui| {
                if toolstrip::enabled(ui, has_toc, tr("toc.none"), |ui| toolstrip::small(ui, Some(Icon::Calculate), tr("toc.update"), "", a, false)).clicked() {
                    let (changed, sel) = self.update_all_tocs();
                    if changed {
                        self.set_sel(sel);
                    } else {
                        self.notices.push(Notice::info(tr("toc.up_to_date")));
                    }
                }
            });
        });
        b.group("toolstrip.g.index", Icon::Bookmark, |ui| {
            if toolstrip::large(ui, Icon::Bookmark, tr("index.mark"), &format!("{} (Alt+Shift+X)", tr("index.mark")), a, false).clicked() {
                self.open_mark_entry_dialog();
            }
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::Toc), tr("index.insert"), "", a, false).clicked() {
                    self.open_index_dialog();
                }
                let has = self.doc.toc_blocks().iter().any(|&(a, _)| self.doc.paragraphs[a].style.toc.is_some_and(|t| t.opts.index));
                ui.add_enabled_ui(has, |ui| {
                    if toolstrip::small(ui, Some(Icon::Calculate), tr("index.update"), "", a, false).on_disabled_hover_text(tr("index.none")).clicked() {
                        let (changed, sel) = self.update_all_tocs();
                        if changed {
                            self.set_sel(sel);
                        } else {
                            self.notices.push(Notice::info(tr("toc.up_to_date")));
                        }
                    }
                });
            });
        });
        b.group("toolstrip.g.footnotes", Icon::Footnote, |ui| {
            if toolstrip::large(ui, Icon::Footnote, tr("fnote.insert_foot"), &format!("{} (Ctrl+Alt+F)", tr("fnote.insert_foot")), a, false).clicked() {
                self.insert_note(NoteKind::Foot);
            }
            toolstrip::column(ui, |ui| {
                if toolstrip::small(ui, Some(Icon::Footnote), tr("fnote.insert_end"), &format!("{} (Ctrl+Alt+D)", tr("fnote.insert_end")), a, false).clicked() {
                    self.insert_note(NoteKind::End);
                }
                if toolstrip::small(ui, Some(Icon::ChevronDown), tr("fnote.next"), "", a, false).clicked() {
                    self.next_note();
                }
                let pane = self.refs.pane.is_some();
                if toolstrip::small(ui, Some(Icon::NavPane), tr("fnote.show_pane"), "", a, pane).clicked() {
                    self.toggle_notes_pane(None);
                }
            });
            if toolstrip::large(ui, Icon::Options, &format!("{}…", tr("fnote.options_button")), &format!("{}…", tr("fnote.options")), a, false).clicked() {
                self.refs.dialog = Some(RefDialog::Notes(Box::new(NotesDlg { cfg: self.doc.notes, kind: 0 })));
            }
        });
        b.group("toolstrip.g.captions", Icon::CrossRef, |ui| {
            if toolstrip::large(ui, Icon::Caption, tr("caption.insert"), "", a, false).clicked() {
                self.open_caption_dialog();
            }
            if toolstrip::large(ui, Icon::Toc, tr("figures.insert"), "", a, false).clicked() {
                let k = [1u8, 2, 3].into_iter().find(|&k| !self.doc.captions(k).is_empty()).unwrap_or(1);
                self.refs.dialog = Some(RefDialog::Toc(TocOpts { caption: k, ..TocOpts::default() }));
            }
            if toolstrip::large(ui, Icon::CrossRef, tr("xref.insert"), "", a, false).clicked() {
                self.open_xref_dialog();
            }
            if toolstrip::large(ui, Icon::Field, tr("field.insert"), "", a, false).clicked() {
                self.refs.dialog = Some(RefDialog::Field(Box::default()));
            }
            if toolstrip::large(ui, Icon::Bookmark, tr("doc.bookmark"), "", a, false).clicked() {
                self.open_bookmark_dialog();
            }
        });
        self.biblio_group(b);
        self.toa_group(b);
    }

    /// Context menu items for a table of contents or field at the caret.
    pub(super) fn refs_context_items(&mut self, ui: &mut egui::Ui) {
        let c = self.sel.cursor;
        let on_field = self.doc.field_at(c).is_some() || self.doc.field_at(self.doc.prev_char(c)).is_some();
        if self.doc.toc_at(c.para).is_some() || on_field {
            if ui.add(egui::Button::new(tr("field.update")).shortcut_text("F9")).clicked() {
                self.update_field();
                ui.close();
            }
            ui.separator();
        }
        let note = self.doc.note_at(c).or_else(|| self.doc.note_at(self.doc.prev_char(c)));
        if let Some((s, _)) = note {
            if ui.button(tr("fnote.go_to_text")).clicked() {
                self.toggle_notes_pane(Some(s.start()));
                ui.close();
            }
            ui.separator();
        }
    }

    // --- dialogs -------------------------------------------------------------------------

    /// "No entries" text of the table of contents (or index) at `para`.
    fn no_entries(&self, para: usize) -> &'static str {
        match self.doc.paragraphs.get(para).and_then(|p| p.style.toc) {
            Some(t) if t.opts.index => tr("index.no_entries"),
            Some(t) if t.opts.toa > 0 => tr("toa.no_entries"),
            Some(t) if t.opts.bib => tr("cite.no_sources"),
            Some(t) if t.opts.caption > 0 => tr("figures.no_entries"),
            _ => tr("toc.no_entries"),
        }
    }

    /// Updates every table of contents and index. Returns the selection
    /// moved along.
    pub(super) fn update_all_tocs(&mut self) -> (bool, Selection) {
        let mut sel = self.sel;
        let mut any = false;
        for (a, _) in self.doc.toc_blocks().into_iter().rev() {
            let none = self.no_entries(a);
            if self.doc.update_toc(a, sel, none) {
                any = true;
                if let Some(s) = self.doc.last_sel_after() {
                    sel = s;
                }
            }
        }
        (any, sel)
    }

    pub(super) fn open_index_dialog(&mut self) {
        let opts = self.doc.toc_at(self.sel.cursor.para).and_then(|(a, _)| self.doc.paragraphs[a].style.toc).map(|t| t.opts).filter(|o| o.index);
        self.refs.dialog = Some(RefDialog::Toc(opts.unwrap_or(TocOpts { index: true, letters: true, links: false, ..TocOpts::default() })));
    }

    /// Mark Index Entry, with the selected text as the entry.
    pub(super) fn open_mark_entry_dialog(&mut self) {
        let t: String = self.doc.text_range(self.sel.start(), self.sel.end()).chars().filter(|&c| c != OBJECT).map(|c| if c.is_control() || c == apex_doc::LINE_BREAK { ' ' } else { c }).collect();
        let t = t.trim().trim_end_matches(['.', ',', ';', ':']).replace(':', "\\:");
        self.refs.dialog = Some(RefDialog::Mark(Box::new(MarkDlg { main: t.clone(), sub: String::new(), found: t, all: false })));
    }

    pub(super) fn open_toc_dialog(&mut self) {
        let opts = self.doc.toc_at(self.sel.cursor.para).and_then(|(a, _)| self.doc.paragraphs[a].style.toc).map(|t| t.opts).unwrap_or_default();
        self.refs.dialog = Some(RefDialog::Toc(opts));
    }

    pub(super) fn open_caption_dialog(&mut self) {
        let mut labels: Vec<String> = CAPTION_KINDS.iter().map(|k| tr(k).to_string()).collect();
        for (_, f) in self.doc.fields() {
            if let FieldKind::Seq(n) = &f.kind {
                let n = n.replace('_', " ");
                if !labels.iter().any(|l| l.eq_ignore_ascii_case(&n)) {
                    labels.push(n);
                }
            }
        }
        // Below a picture or a table, above an equation (as Word).
        let para = self.sel.cursor.para;
        let table = self.doc.in_table(para);
        let label = if table { 1 } else { 0 };
        self.refs.dialog = Some(RefDialog::Caption(Box::new(CaptionDlg { labels, label, text: String::new(), above: table, new_label: None })));
    }

    pub(super) fn open_xref_dialog(&mut self) {
        self.refs.dialog = Some(RefDialog::Xref(Box::new(XrefDlg { kind: 0, what: 0, link: true, pick: None })));
    }

    /// Items that a cross-reference of type `kind` can point to: label,
    /// paragraph and note mark.
    fn xref_items(&self, kind: usize) -> Vec<(String, usize, Option<Pos>)> {
        let doc = &self.doc;
        let clip = |s: &str| {
            let s: String = s.chars().filter(|&c| c != OBJECT).map(|c| if c.is_control() || c == apex_doc::LINE_BREAK { ' ' } else { c }).collect();
            let s = s.trim().to_string();
            if s.chars().count() > 60 { format!("{}…", s.chars().take(60).collect::<String>()) } else { s }
        };
        match kind {
            0 => doc.headings().into_iter().map(|(i, l)| (format!("{}{}", "   ".repeat(l as usize - 1), clip(&doc.paragraphs[i].text)), i, None)).collect(),
            1 => {
                let mut v: Vec<_> = doc.bookmarks().into_iter().filter(|b| !b.is_hidden()).map(|b| (b.name.to_string(), b.start.para, Some(b.start))).collect();
                v.sort_by_key(|x| x.0.to_lowercase());
                v
            }
            2 | 3 => {
                let want = if kind == 2 { NoteKind::Foot } else { NoteKind::End };
                doc.notes()
                    .into_iter()
                    .filter(|(_, n)| n.kind == want)
                    .map(|(p, n)| (format!("{}  {}", self.layout.refs.values.get(&p).cloned().unwrap_or_default(), clip(&n.text())), p.para, Some(p)))
                    .collect()
            }
            // Captions: paragraphs with a sequence field (Figure 1, Table 2…).
            _ => doc
                .fields()
                .into_iter()
                .filter(|(_, f)| matches!(&f.kind, FieldKind::Seq(n) if n.to_lowercase().starts_with(if kind == 4 { "fig" } else { "tab" }) || (kind == 4 && n.to_lowercase().starts_with("ilus"))))
                .map(|(p, _)| {
                    let t = shown_text(&doc.paragraphs[p.para], p.para, &self.layout.refs.values);
                    (clip(&t), p.para, None)
                })
                .collect(),
        }
    }

    pub(super) fn ref_dialogs(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.refs.dialog.take() else { return };
        if matches!(d, RefDialog::Source(_) | RefDialog::Sources(_)) {
            self.refs.dialog = self.biblio_dialog(ctx, d);
            return;
        }
        if matches!(d, RefDialog::MarkCitation(_) | RefDialog::Toa(_)) {
            self.refs.dialog = self.toa_dialog(ctx, d);
            return;
        }
        let res = match &mut d {
            RefDialog::Toc(o) => widgets::dialog(ctx, "doc_toc", tr(if o.index { "index.title" } else if o.caption > 0 { "figures.title" } else { "toc.title" }), 400.0, |ui| toc_page(ui, o)),
            RefDialog::Source(_) | RefDialog::Sources(_) | RefDialog::MarkCitation(_) | RefDialog::Toa(_) => DialogResult::Cancel,
            RefDialog::Mark(m) => widgets::dialog(ctx, "doc_mark_entry", tr("index.mark_title"), 440.0, |ui| mark_page(ui, m)),
            RefDialog::Caption(c) => widgets::dialog(ctx, "doc_caption", tr("caption.title"), 420.0, |ui| caption_page(ui, c)),
            RefDialog::Field(f) => {
                let preview = self.field_preview(f);
                let pages = [
                    (Icon::PageNumber, FIELD_TYPES[0].0),
                    (Icon::PageNumber, FIELD_TYPES[1].0),
                    (Icon::Calendar, FIELD_TYPES[2].0),
                    (Icon::DateTime, FIELD_TYPES[3].0),
                    (Icon::Comment, FIELD_TYPES[4].0),
                    (Icon::Doc, FIELD_TYPES[5].0),
                    (Icon::Folder, FIELD_TYPES[6].0),
                    (Icon::Numbering, FIELD_TYPES[7].0),
                    (Icon::Doc, FIELD_TYPES[8].0),
                ];
                let mut names: Vec<String> = apex_doc::BUILTIN_PROPS.iter().map(|s| s.to_string()).collect();
                names.extend(self.doc.meta.custom.iter().map(|p| p.name.clone()));
                let mut page = f.ty;
                let r = widgets::nav_dialog(ctx, "doc_field", tr("field.title"), &pages, &mut page, egui::vec2(420.0, 330.0), |ui, page| {
                    if page != f.ty {
                        f.ty = page;
                        f.format.clear();
                    }
                    field_page(ui, f, &preview, &names)
                });
                r
            }
            RefDialog::Notes(n) => {
                let pages = [(Icon::Footnote, "fnote.footnotes"), (Icon::Footnote, "fnote.endnotes")];
                let mut page = n.kind;
                let r = widgets::nav_dialog(ctx, "doc_notes", tr("fnote.options"), &pages, &mut page, egui::vec2(380.0, 240.0), |ui, page| {
                    n.kind = page;
                    notes_page(ui, n)
                });
                r
            }
            RefDialog::Xref(x) => {
                let items = self.xref_items(x.kind);
                let r = widgets::dialog(ctx, "doc_xref", tr("xref.title"), 480.0, |ui| xref_page(ui, x, &items));
                if r == DialogResult::Ok {
                    x.pick = x.pick.filter(|&i| i < items.len());
                    if let Some(i) = x.pick {
                        let (_, para, at) = items[i].clone();
                        self.insert_xref(x, para, at);
                    }
                }
                r
            }
        };
        match res {
            DialogResult::None => self.refs.dialog = Some(d),
            DialogResult::Cancel => self.focus = true,
            DialogResult::Ok => {
                match d {
                    RefDialog::Toc(o) => {
                        let s = self.doc.insert_toc(self.sel, o, tr(if o.index { "index.no_entries" } else if o.caption > 0 { "figures.no_entries" } else { "toc.no_entries" }));
                        self.set_sel(s);
                    }
                    RefDialog::Mark(m) => self.mark_entry(&m),
                    RefDialog::Source(_) | RefDialog::Sources(_) | RefDialog::MarkCitation(_) | RefDialog::Toa(_) => {}
                    RefDialog::Caption(c) => {
                        let label = c.labels.get(c.label).cloned().unwrap_or_default();
                        let text = if c.text.is_empty() || c.text.starts_with([':', '.', ' ', '-']) { c.text.clone() } else { format!(": {}", c.text) };
                        let s = self.doc.insert_caption(self.sel, &label, &text, c.above, tr("caption.style"));
                        self.set_sel(s);
                    }
                    RefDialog::Field(f) => {
                        if let Some(field) = f.field() {
                            self.insert_field(field);
                        }
                    }
                    RefDialog::Notes(n) => {
                        if n.cfg != self.doc.notes {
                            self.doc.notes = n.cfg;
                            self.doc.modified = true;
                            self.doc.revision += 1;
                        }
                    }
                    RefDialog::Xref(_) => {}
                }
                self.focus = true;
            }
        }
    }

    fn field_preview(&self, f: &FieldDlg) -> String {
        let Some(field) = f.field() else { return String::new() };
        let mut d = Document::from_paragraphs(vec![Paragraph::new("")]);
        d.meta = self.doc.meta.clone();
        let s = d.insert_field(Selection::default(), field);
        let refs = d.refs();
        let (date, time) = common::date_time_now();
        let (y, mo, dd, h, mi) = common::local_now();
        let months: Vec<&str> = tr("field.months").split('|').collect();
        let days: Vec<&str> = tr("field.days").split('|').collect();
        let page = self.caret_page() + 1;
        let cx = FieldCtx {
            page,
            pages: self.layout.pages,
            now: (y, mo, dd, h, mi, 0),
            date: &date,
            time: &time,
            months: &months,
            days: &days,
            file: &self.file_name,
            page_of: &|_| Some(page),
            page_label: &|p| self.layout.labels.get(p.wrapping_sub(1)).cloned().unwrap_or_else(|| p.to_string()),
            above: tr("xref.above"),
            below: tr("xref.below"),
            on_page: tr("xref.on_page"),
            note_label: &|_| None,
            decimal: common::decimal(),
        };
        let Some((_, fld)) = d.field_at(s.start()) else { return String::new() };
        refs.value(&d, &fld, s.start(), &cx)
    }

    fn mark_entry(&mut self, m: &MarkDlg) {
        // A colon inside an entry is written "\\:" in the dialog.
        let part = |s: &str| s.trim().replace("\\:", "\u{1}").replace(':', " ").replace('\u{1}', ":");
        let main = part(&m.main);
        if main.is_empty() {
            return;
        }
        let sub = part(&m.sub);
        let entry = if sub.is_empty() { main } else { format!("{main}:{sub}") };
        if m.all && !m.found.is_empty() {
            let found = m.found.replace("\\:", ":");
            let n = self.doc.mark_index_entries(&found, &entry, self.sel);
            self.notices.push(Notice::info(trf("index.marked", &[("n", &n)])));
            return;
        }
        // After the selection, as Word does.
        self.clear_block();
        let at = Selection::caret(self.sel.end());
        let s = self.doc.insert_field(at, Field::new(FieldKind::IndexEntry(entry)));
        self.sel = Selection::caret(s.end());
    }

    fn insert_xref(&mut self, x: &XrefDlg, para: usize, at: Option<Pos>) {
        let sel = self.sel;
        // Notes are referenced by a bookmark around their mark; headings and
        // captions by one around the paragraph.
        let (para, note) = match x.kind {
            2 | 3 => (para, at),
            1 => (para, None),
            _ => (para, None),
        };
        let name = if x.kind == 1 {
            let Some(b) = self.doc.bookmarks().into_iter().find(|b| Some(b.start) == at) else { return };
            b.name.to_string()
        } else {
            match self.doc.reference_bookmark(para, note, sel) {
                Some(n) => n,
                None => return,
            }
        };
        let is_note = matches!(x.kind, 2 | 3);
        let kind = match (x.what, is_note) {
            (1, _) => FieldKind::PageRef(name),
            (0, true) => FieldKind::NoteRef(name),
            _ => FieldKind::Ref(name),
        };
        let f = Field { hyperlink: x.link, position: x.what == 2, ..Field::new(kind) };
        self.insert_field(f);
    }
}

/// The text of paragraph `i` with its fields shown.
fn shown_text(p: &Paragraph, i: usize, values: &HashMap<Pos, String>) -> String {
    let mut out = String::new();
    for (o, c) in p.text.char_indices() {
        if c == OBJECT {
            if let Some(v) = values.get(&Pos::new(i, o)) {
                out.push_str(v);
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub(super) enum RefDialog {
    Toc(TocOpts),
    Source(Box<super::biblio_ui::SourceDlg>),
    Sources(Box<super::biblio_ui::SourcesDlg>),
    MarkCitation(Box<super::toa_ui::MarkCitationDlg>),
    Toa(Box<super::toa_ui::ToaDlg>),
    Mark(Box<MarkDlg>),
    Caption(Box<CaptionDlg>),
    Field(Box<FieldDlg>),
    Notes(Box<NotesDlg>),
    Xref(Box<XrefDlg>),
}

pub(super) struct MarkDlg {
    main: String,
    sub: String,
    /// The selected text (what Mark All looks for).
    found: String,
    all: bool,
}

pub(super) struct CaptionDlg {
    labels: Vec<String>,
    label: usize,
    text: String,
    above: bool,
    /// The New Label box.
    new_label: Option<String>,
}

const CAPTION_KINDS: [&str; 3] = ["caption.figure", "caption.table", "caption.equation"];

pub(super) struct NotesDlg {
    cfg: apex_doc::NotesConfig,
    /// 0 footnotes, 1 endnotes.
    kind: usize,
}

pub(super) struct XrefDlg {
    /// 0 heading, 1 bookmark, 2 footnote, 3 endnote, 4 figure, 5 table.
    kind: usize,
    /// 0 text (or note number), 1 page number, 2 above/below.
    what: usize,
    link: bool,
    pick: Option<usize>,
}

/// Field types offered in the Field dialog.
const FIELD_TYPES: [(&str, &str); 9] = [
    ("field.page", "PAGE"),
    ("field.num_pages", "NUMPAGES"),
    ("field.date", "DATE"),
    ("field.time", "TIME"),
    ("field.author", "AUTHOR"),
    ("field.doc_title", "TITLE"),
    ("field.file_name", "FILENAME"),
    ("field.seq", "SEQ"),
    ("field.doc_property", "DOCPROPERTY"),
];

#[derive(Default)]
pub(super) struct FieldDlg {
    ty: usize,
    /// Date or time picture.
    format: String,
    seq: String,
    /// DOCPROPERTY: the property's name.
    prop: String,
}

impl FieldDlg {
    fn field(&self) -> Option<Field> {
        let kind = match self.ty {
            0 => FieldKind::Page,
            1 => FieldKind::NumPages,
            2 => FieldKind::Date,
            3 => FieldKind::Time,
            4 => FieldKind::Author,
            5 => FieldKind::Title,
            6 => FieldKind::FileName,
            7 => {
                let s = self.seq.trim();
                if s.is_empty() {
                    return None;
                }
                FieldKind::Seq(s.replace(' ', "_"))
            }
            _ => {
                let s = self.prop.trim();
                if s.is_empty() {
                    return None;
                }
                FieldKind::DocProperty(s.to_string())
            }
        };
        let dated = matches!(kind, FieldKind::Date | FieldKind::Time);
        let format = Some(self.format.trim().to_string()).filter(|f| dated && !f.is_empty());
        Some(Field { format, ..Field::new(kind) })
    }
}

fn field_page(ui: &mut egui::Ui, f: &mut FieldDlg, preview: &str, props: &[String]) {
    widgets::section(ui, tr("field.properties"));
    match f.ty {
        2 | 3 => {
            widgets::form(ui, "field_form", |ui| {
                widgets::form_row(ui, tr("field.format").trim_end_matches(':'), |ui| ui.add(egui::TextEdit::singleline(&mut f.format).desired_width(220.0).hint_text(tr("field.default_format"))));
            });
            let key = if f.ty == 2 { "field.date_formats" } else { "field.time_formats" };
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::ScrollArea::vertical().max_height(130.0).show(ui, |ui| {
                    for pic in tr(key).split('|') {
                        if ui.selectable_label(f.format == pic, pic).clicked() {
                            f.format = pic.to_string();
                        }
                    }
                });
            });
        }
        7 => {
            widgets::form(ui, "field_form", |ui| {
                widgets::form_row(ui, tr("field.seq_name").trim_end_matches(':'), |ui| ui.add(egui::TextEdit::singleline(&mut f.seq).desired_width(220.0).hint_text(tr("field.seq_hint"))));
            });
        }
        8 => {
            widgets::form(ui, "field_form", |ui| {
                widgets::form_row(ui, tr("field.prop_name"), |ui| ui.add(egui::TextEdit::singleline(&mut f.prop).desired_width(220.0)));
            });
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::ScrollArea::vertical().max_height(130.0).show(ui, |ui| {
                    for n in props {
                        if ui.selectable_label(f.prop == *n, n).clicked() {
                            f.prop = n.clone();
                        }
                    }
                });
            });
        }
        _ => {
            ui.label(egui::RichText::new(tr("field.no_options")).weak());
        }
    }
    widgets::section(ui, tr("field.preview").trim_end_matches(':'));
    ui.label(if preview.is_empty() { "—" } else { preview });
    if let Some(field) = f.field() {
        ui.label(egui::RichText::new(field.instr().trim().to_string()).weak().monospace());
    }
}

fn toc_page(ui: &mut egui::Ui, o: &mut TocOpts) {
    if !o.index {
        widgets::section(ui, tr("refs.sec_general"));
        widgets::form(ui, "toc_grid", |ui| {
            if o.caption > 0 {
                widgets::form_row(ui, tr("figures.label").trim_end_matches(':'), |ui| {
                    let k = (o.caption as usize - 1).min(2);
                    egui::ComboBox::from_id_salt("fig_label").selected_text(tr(CAPTION_KINDS[k])).show_ui(ui, |ui| {
                        for (i, key) in CAPTION_KINDS.iter().enumerate() {
                            ui.selectable_value(&mut o.caption, i as u8 + 1, tr(key));
                        }
                    });
                });
            } else {
                widgets::form_row(ui, tr("toc.levels").trim_end_matches(':'), |ui| ui.add(egui::DragValue::new(&mut o.levels).range(1..=9)));
            }
        });
    }
    widgets::section(ui, tr("refs.sec_page_numbers"));
    ui.checkbox(&mut o.page_numbers, tr("toc.page_numbers"));
    ui.add_enabled(o.page_numbers, egui::Checkbox::new(&mut o.right_align, tr("toc.right_align")));
    ui.add_enabled(o.page_numbers && o.right_align, egui::Checkbox::new(&mut o.leader, tr("toc.leader")));
    widgets::section(ui, tr("refs.sec_options"));
    if o.index {
        ui.checkbox(&mut o.letters, tr("index.letters"));
    } else {
        ui.checkbox(&mut o.links, tr("toc.links"));
    }
}

fn mark_page(ui: &mut egui::Ui, m: &mut MarkDlg) {
    widgets::section(ui, tr("refs.sec_entry"));
    widgets::form(ui, "mark_grid", |ui| {
        widgets::form_row(ui, tr("index.main").trim_end_matches(':'), |ui| ui.add(egui::TextEdit::singleline(&mut m.main).desired_width(f32::INFINITY)));
        widgets::form_row(ui, tr("index.sub").trim_end_matches(':'), |ui| ui.add(egui::TextEdit::singleline(&mut m.sub).desired_width(f32::INFINITY)));
    });
    widgets::section(ui, tr("refs.sec_options"));
    ui.add_enabled(!m.found.is_empty(), egui::Checkbox::new(&mut m.all, trf("index.mark_all", &[("text", &m.found.as_str())])));
    ui.label(egui::RichText::new(tr("index.mark_hint")).small().weak());
}

fn caption_page(ui: &mut egui::Ui, c: &mut CaptionDlg) {
    let label = c.labels.get(c.label).cloned().unwrap_or_default();
    widgets::section(ui, tr("refs.sec_caption"));
    ui.label(tr("caption.caption"));
    let r = ui.add(egui::TextEdit::singleline(&mut c.text).desired_width(f32::INFINITY).hint_text(format!("{label} 1")));
    if !r.has_focus() && !r.lost_focus() && c.new_label.is_none() {
        r.request_focus();
    }
    widgets::section(ui, tr("refs.sec_options"));
    widgets::form(ui, "caption_grid", |ui| {
        widgets::form_row(ui, tr("caption.label").trim_end_matches(':'), |ui| {
            egui::ComboBox::from_id_salt("caption_label").selected_text(&label).width(180.0).show_ui(ui, |ui| {
                for (i, l) in c.labels.iter().enumerate() {
                    ui.selectable_value(&mut c.label, i, l);
                }
            });
        });
        widgets::form_row(ui, tr("caption.position").trim_end_matches(':'), |ui| {
            egui::ComboBox::from_id_salt("caption_pos").selected_text(tr(if c.above { "caption.above" } else { "caption.below" })).width(180.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut c.above, true, tr("caption.above"));
                ui.selectable_value(&mut c.above, false, tr("caption.below"));
            });
        });
    });
    ui.add_space(6.0);
    match &mut c.new_label {
        None => {
            if ui.button(format!("{}…", tr("caption.new_label"))).clicked() {
                c.new_label = Some(String::new());
            }
        }
        Some(t) => {
            let mut done = false;
            ui.horizontal(|ui| {
                let r = ui.add(egui::TextEdit::singleline(t).desired_width(180.0));
                if !r.has_focus() && !r.lost_focus() {
                    r.request_focus();
                }
                done = ui.button(tr("caption.add")).clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
            });
            if done {
                let t = t.trim().to_string();
                if !t.is_empty() {
                    c.label = c.labels.iter().position(|l| *l == t).unwrap_or_else(|| {
                        c.labels.push(t);
                        c.labels.len() - 1
                    });
                }
                c.new_label = None;
            }
        }
    }
}

const FORMAT_KEYS: [&str; 6] = ["fnote.fmt_arabic", "fnote.fmt_lower_roman", "fnote.fmt_upper_roman", "fnote.fmt_lower_letter", "fnote.fmt_upper_letter", "fnote.fmt_symbols"];

fn notes_page(ui: &mut egui::Ui, n: &mut NotesDlg) {
    let s = if n.kind == 0 { &mut n.cfg.foot } else { &mut n.cfg.end };
    widgets::section(ui, tr("refs.sec_format"));
    widgets::form(ui, "notes_grid", |ui| {
        widgets::form_row(ui, tr("fnote.format").trim_end_matches(':'), |ui| {
            let cur = apex_doc::NoteFormat::ALL.iter().position(|f| *f == s.format).unwrap_or(0);
            egui::ComboBox::from_id_salt("note_fmt").width(190.0).selected_text(format!("{}  ({})", tr(FORMAT_KEYS[cur]), s.format.label(1))).show_ui(ui, |ui| {
                for (i, f) in apex_doc::NoteFormat::ALL.iter().enumerate() {
                    ui.selectable_value(&mut s.format, *f, format!("{}  ({}, {}, {})", tr(FORMAT_KEYS[i]), f.label(1), f.label(2), f.label(3)));
                }
            });
        });
        widgets::form_row(ui, tr("fnote.start_at").trim_end_matches(':'), |ui| ui.add(egui::DragValue::new(&mut s.start).range(1..=999)));
    });
    widgets::section(ui, tr("fnote.numbering").trim_end_matches(':'));
    ui.radio_value(&mut s.restart, NoteRestart::Continuous, tr("fnote.continuous"));
    if n.kind == 0 {
        ui.radio_value(&mut s.restart, NoteRestart::EachPage, tr("fnote.each_page"));
    }
    if n.kind == 1 && s.restart == NoteRestart::EachPage {
        s.restart = NoteRestart::Continuous;
    }
}

const XREF_KINDS: [&str; 6] = ["xref.heading", "xref.bookmark", "xref.footnote", "xref.endnote", "xref.figure", "xref.table"];

fn xref_page(ui: &mut egui::Ui, x: &mut XrefDlg, items: &[(String, usize, Option<Pos>)]) {
    widgets::section(ui, tr("refs.sec_reference"));
    widgets::form(ui, "xref_grid", |ui| {
        widgets::form_row(ui, tr("xref.type").trim_end_matches(':'), |ui| {
            egui::ComboBox::from_id_salt("xref_kind").width(200.0).selected_text(tr(XREF_KINDS[x.kind])).show_ui(ui, |ui| {
                for (i, k) in XREF_KINDS.iter().enumerate() {
                    if ui.selectable_value(&mut x.kind, i, tr(k)).changed() {
                        x.pick = None;
                    }
                }
            });
        });
        widgets::form_row(ui, tr("xref.insert_to").trim_end_matches(':'), |ui| {
            let note = matches!(x.kind, 2 | 3);
            let whats = [if note { "xref.note_number" } else { "xref.text" }, "xref.page", "xref.above_below"];
            egui::ComboBox::from_id_salt("xref_what").width(200.0).selected_text(tr(whats[x.what.min(2)])).show_ui(ui, |ui| {
                for (i, k) in whats.iter().enumerate() {
                    ui.selectable_value(&mut x.what, i, tr(k));
                }
            });
        });
    });
    ui.checkbox(&mut x.link, tr("xref.as_link"));
    widgets::section(ui, tr("xref.for_which").trim_end_matches(':'));
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().max_height(200.0).min_scrolled_height(200.0).auto_shrink([false, false]).show(ui, |ui| {
            if items.is_empty() {
                ui.label(egui::RichText::new(tr("xref.none")).weak());
            }
            for (i, (label, _, _)) in items.iter().enumerate() {
                let r = ui.selectable_label(x.pick == Some(i), label);
                if r.clicked() {
                    x.pick = Some(i);
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_doc::TocOpts;

    fn laid_out(doc: Document) -> (DocView, egui::Context) {
        let ctx = egui::Context::default();
        fonts::install(&ctx);
        let frame = || ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        frame();
        frame();
        let mut v = DocView::new(doc);
        v.layout = v.build_layout(&ctx, 1.0, (1, 0, 0, 0, 0), Layout::default());
        (v, ctx)
    }

    fn heading(t: &str) -> Paragraph {
        Paragraph::new(t).with_style(ParaStyle { kind: ParaKind::Heading(1), ..Default::default() })
    }

    #[test]
    fn notes_fields_and_toc_on_pages() {
        let long = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(12);
        let mut ps = vec![heading("First")];
        for _ in 0..30 {
            ps.push(Paragraph::new(&long));
        }
        ps.push(heading("Second"));
        ps.push(Paragraph::new("Page  of "));
        let mut d = Document::from_paragraphs(ps);
        // A footnote on the first page, long enough to go on to the next.
        let at = Pos::new(1, 5);
        d.insert_note(Selection::caret(at), NoteKind::Foot);
        let mut n = (*d.note_at(at).unwrap().1).clone();
        n.set_text(&"A long note that goes on. ".repeat(160));
        d.set_note(at, n);
        d.insert_note(Selection::caret(Pos::new(2, 0)), NoteKind::End);
        let last = d.paragraphs.len() - 1;
        d.insert_field(Selection::caret(Pos::new(last, 8)), Field::new(FieldKind::NumPages));
        d.insert_field(Selection::caret(Pos::new(last, 5)), Field::new(FieldKind::Page));
        d.insert_toc(Selection::caret(Pos::new(0, 0)), TocOpts::default(), "none");
        let (v, _ctx) = laid_out(d);
        let l = &v.layout;
        assert!(l.pages >= 3, "{} pages", l.pages);
        let np = &l.refs.notes.pages;
        // The footnote starts on page 1 and goes on at the top of page 2's
        // notes; the endnote comes after the text.
        assert!(!np[0].foot.is_empty());
        assert!(np.iter().skip(1).any(|p| p.cont && !p.foot.is_empty()));
        assert!(np.iter().any(|p| p.end_top.is_some() && !p.end.is_empty()));
        // No text runs into the notes.
        for s in &l.segs {
            let foot = np.get(s.page).map_or(0.0, |p| p.foot_h);
            assert!(s.y + (s.gy1 - s.gy0) <= l.content.y - foot + 1.0, "segment of {} on page {}", s.para, s.page);
        }
        // Field values: page and number of pages, and the table of contents.
        let last = v.doc.paragraphs.len() - 1;
        let vals: Vec<(Pos, String)> = v.doc.fields().into_iter().map(|(p, _)| (p, l.refs.values.get(&p).cloned().unwrap_or_default())).collect();
        let page_of = |para: usize| (v.page_in(l, Pos::new(para, 0)).unwrap() + 1).to_string();
        let (toc_a, toc_b) = v.doc.toc_blocks()[0];
        assert_eq!(toc_b - toc_a, 2);
        assert_eq!(vals[0].1, page_of(toc_b));
        assert_eq!(vals[1].1, page_of(last - 1));
        assert_eq!(vals[2].1, page_of(last));
        assert_eq!(vals[3].1, l.pages.to_string());
        // Note marks are numbered.
        let marks: Vec<String> = v.doc.notes().iter().map(|(p, _)| l.refs.values[p].clone()).collect();
        assert_eq!(marks, ["1", "i"]);
        // The page numbers of the table end at the right margin.
        let pl = &l.paras[toc_a];
        let row = pl.galley.rows.last().unwrap();
        let g = row.row.glyphs.last().unwrap();
        assert!((row.pos.x + g.pos.x + g.advance_width - pl.width).abs() < 3.0, "{} vs {}", row.pos.x + g.pos.x + g.advance_width, pl.width);
    }
}
