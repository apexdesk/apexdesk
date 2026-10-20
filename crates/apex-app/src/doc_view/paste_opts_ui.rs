//! Paste Options: the button by pasted text, to keep its formatting, take
//! the formatting where it went, or keep only the text.

use apex_doc::{Attrs, Paragraph, ParaStyle, Selection};
use eframe::egui::{self, Pos2, vec2};

use super::DocView;
use crate::i18n::tr;
use crate::icons::{self, Icon};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PasteMode {
    /// As copied.
    Keep,
    /// The formatting where it went, keeping emphasis, links and objects.
    Merge,
    Text,
}

impl PasteMode {
    const ALL: [PasteMode; 3] = [PasteMode::Keep, PasteMode::Merge, PasteMode::Text];

    fn key(self) -> &'static str {
        match self {
            PasteMode::Keep => "paste_opts.keep",
            PasteMode::Merge => "paste_opts.merge",
            PasteMode::Text => "paste_opts.text",
        }
    }
}

/// What was just pasted.
pub(super) struct Pasted {
    /// The selection it replaced.
    before: Selection,
    /// The caret after it (the button goes when it moves).
    after: Selection,
    undo_len: usize,
    text: String,
    paras: Option<Vec<Paragraph>>,
    mode: PasteMode,
}

/// Formatted text taking the formatting of `base` and `style`, keeping
/// emphasis, links, objects and list levels.
fn merged(paras: &[Paragraph], base: &Attrs, style: &ParaStyle) -> Vec<Paragraph> {
    paras
        .iter()
        .map(|p| {
            let mut p = p.clone();
            if p.cell.is_none() {
                let src = p.style.clone();
                p.style = style.clone();
                if src.kind.is_list() {
                    (p.style.kind, p.style.level, p.style.list_format) = (src.kind, src.level, src.list_format);
                }
            }
            p.section = None;
            for r in &mut p.runs {
                let s = &r.attrs;
                r.attrs = Attrs {
                    bold: s.bold,
                    italic: s.italic,
                    underline: s.underline,
                    strike: s.strike,
                    superscript: s.superscript,
                    subscript: s.subscript,
                    link: s.link.clone(),
                    link_tip: s.link_tip.clone(),
                    image: s.image.clone(),
                    note: s.note.clone(),
                    field: s.field.clone(),
                    math: s.math.clone(),
                    shape: s.shape.clone(),
                    control: s.control.clone(),
                    ..base.clone()
                };
            }
            p
        })
        .collect()
}

impl DocView {
    /// Remembers a paste, for the button.
    pub(super) fn pasted(&mut self, before: Selection, text: &str, paras: Option<Vec<Paragraph>>) {
        let mode = if paras.is_some() { PasteMode::Keep } else { PasteMode::Text };
        self.paste_opts = Some(Pasted { before, after: self.sel, undo_len: self.doc.undo_history().len(), text: text.to_string(), paras, mode });
    }

    /// The paste the button is for, while nothing else happened.
    fn live_paste(&self) -> Option<&Pasted> {
        self.paste_opts.as_ref().filter(|p| p.after == self.sel && p.undo_len == self.doc.undo_history().len())
    }

    /// Pastes the last paste again in another way.
    fn repaste(&mut self, mode: PasteMode) {
        let Some(mut p) = self.paste_opts.take() else { return };
        self.undo();
        let before = p.before;
        let start = self.doc.clamp(before.start());
        let base = self.doc.paragraphs[start.para].attrs_at(start.offset);
        let style = self.doc.paragraphs[start.para].style.clone();
        let end = match (mode, &p.paras) {
            (PasteMode::Keep, Some(paras)) => self.doc.insert_paragraphs(before, paras),
            (PasteMode::Merge, Some(paras)) => self.doc.insert_paragraphs(before, &merged(paras, &base, &style)),
            _ => self.doc.insert_text(before, &p.text, Some(base)),
        };
        self.sel = Selection::caret(end);
        (p.after, p.undo_len, p.mode) = (self.sel, self.doc.undo_history().len(), mode);
        self.paste_opts = Some(p);
        self.focus = true;
    }

    /// The Paste Options button under the caret; `origin`: the canvas on
    /// screen.
    pub(super) fn paste_options_button(&mut self, ui: &mut egui::Ui, origin: Pos2) {
        let Some(p) = self.live_paste() else {
            self.paste_opts = None;
            return;
        };
        let (cur, rich) = (p.mode, p.paras.is_some());
        let caret = self.caret_rect(self.sel.cursor).translate(origin.to_vec2());
        let mut chosen = None;
        egui::Area::new(egui::Id::new(("paste-opts", self.uid))).order(egui::Order::Middle).fixed_pos(caret.right_bottom() + vec2(4.0, 2.0)).show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).inner_margin(2.0).show(ui, |ui| {
                let r = crate::widgets::menu_button(ui, egui::RichText::new("      ⏷").small(), |ui| {
                    ui.weak(tr("paste_opts.title"));
                    for m in PasteMode::ALL {
                        let can = rich || m == PasteMode::Text;
                        if ui.add_enabled(can, egui::Button::new(tr(m.key())).selected(m == cur)).clicked() {
                            chosen = Some(m);
                            ui.close();
                        }
                    }
                });
                let rect = r.response.rect;
                icons::paint(ui.painter(), egui::Rect::from_center_size(rect.left_center() + vec2(11.0, 0.0), vec2(14.0, 14.0)), Icon::Paste, ui.visuals().text_color());
                r.response.on_hover_text(tr("paste_opts.title"));
            });
        });
        if let Some(m) = chosen.filter(|m| *m != cur) {
            self.repaste(m);
        }
    }
}

#[cfg(test)]
mod tests {
    use apex_doc::{Document, ParaKind, Pos, StyleDef};

    use super::*;

    fn select_all(v: &mut DocView) {
        let last = v.doc.paragraphs.len() - 1;
        v.sel = Selection { anchor: Pos::new(0, 0), cursor: Pos::new(last, v.doc.paragraphs[last].text.len()) };
    }

    /// Size, bold and font of the text at `at`.
    fn look(d: &Document, para: usize, at: usize) -> (u16, bool, Option<std::sync::Arc<str>>) {
        let p = &d.paragraphs[para];
        let f = d.char_format(&p.style, &p.attrs_at(at));
        (f.size, f.bold, f.font.or_else(|| d.default_font_name()))
    }

    #[test]
    fn cells_of_a_workbook_come_as_a_table() {
        use apex_sheet::{CellRef, Range, Workbook};
        let ctx = egui::Context::default();
        let c = |a: &str| CellRef::parse(a).unwrap();
        let mut wb = Workbook::new();
        for (a, v) in [("A1", "Item"), ("B1", "Price"), ("A2", "Pen"), ("B2", "1.5")] {
            wb.set_input(0, c(a), v);
        }
        wb.modify_style(0, &Range::new(c("A1"), c("B1")), |s| s.bold = true);
        let mut s = crate::sheet_view::SheetView::new(wb);
        s.select_range(c("A1"), c("B2"));
        s.copy(&ctx, false);
        let text = crate::clip_history::newest().unwrap().text;

        let mut d = DocView::new(Document::from_paragraphs(vec![Paragraph::new("Prices:"), Paragraph::new("")]));
        d.sel = Selection::caret(Pos::new(1, 0));
        d.paste(&text, false);
        let t = d.doc.tables();
        assert_eq!(t.len(), 1);
        let cell = |i: usize| &d.doc.paragraphs[1 + i];
        assert_eq!((cell(0).text.as_str(), cell(2).text.as_str()), ("Item", "Pen"));
        assert!(cell(0).attrs_at(1).bold && !cell(2).attrs_at(1).bold);
        assert_eq!(cell(3).style.align, apex_doc::Align::Right);
        // As text only, the same copy is its text.
        let mut e = DocView::new(Document::default());
        e.paste(&text, true);
        assert!(e.doc.tables().is_empty());
    }

    #[test]
    fn the_format_painter_goes_to_another_document() {
        let mut doc = Document::from_paragraphs(vec![Paragraph::new("bold and plain")]);
        let caret = Selection::caret(Pos::default());
        doc.set_style(StyleDef { id: "Normal".into(), name: "Normal".into(), builtin: true, size: Some(21), ..Default::default() }, caret);
        doc.format_range(Selection { anchor: Pos::new(0, 0), cursor: Pos::new(0, 4) }, &|a: &mut Attrs| a.bold = true);
        let mut a = DocView::new(doc);
        a.sel = Selection { anchor: Pos::new(0, 1), cursor: Pos::new(0, 3) };
        a.start_brush(false);

        let mut b = DocView::new(Document::from_paragraphs(vec![Paragraph::new("other words")]));
        b.sync_brush();
        b.sel = Selection::caret(Pos::new(0, 8));
        b.apply_brush();
        // Bold, at the size its document's style gave it.
        assert_eq!(look(&b.doc, 0, 8), (21, true, None));
        assert_eq!(look(&b.doc, 0, 1), (24, false, None));
        // Painted once: off there, and off where it was picked up.
        b.sync_brush();
        a.sync_brush();
        assert!(a.brush.is_none() && b.brush.is_none());
    }

    #[test]
    fn formatted_text_goes_from_one_document_to_another() {
        let ctx = egui::Context::default();
        let mut doc = Document::from_paragraphs(vec![Paragraph::new("Title").with_style(ParaStyle { kind: ParaKind::Heading(1), ..Default::default() }), Paragraph::new("bold and plain")]);
        doc.default_font = Some("Georgia".into());
        let caret = Selection::caret(Pos::default());
        doc.set_style(StyleDef { id: "Normal".into(), name: "Normal".into(), builtin: true, size: Some(21), ..Default::default() }, caret);
        doc.format_range(Selection { anchor: Pos::new(1, 0), cursor: Pos::new(1, 4) }, &|a: &mut Attrs| a.bold = true);
        let mut a = DocView::new(doc);
        select_all(&mut a);
        a.copy(&ctx, false);
        // The system clipboard gives the text back, in another document.
        let text = "Title\nbold and plain";
        let mut b = DocView::new(Document::default());
        b.paste(text, false);
        assert_eq!(b.doc.paragraphs.len(), 2);
        assert_eq!(b.doc.paragraphs[0].style.kind, ParaKind::Heading(1));
        for (para, at) in [(0, 1), (1, 1), (1, 8)] {
            assert_eq!(look(&b.doc, para, at), look(&a.doc, para, at), "paragraph {para} at {at}");
        }
        assert_eq!(look(&b.doc, 1, 1), (21, true, Some("Georgia".into())));
        // The paste options work on it as on text copied here.
        b.repaste(PasteMode::Text);
        assert_eq!(look(&b.doc, 1, 1), (24, false, None));
        b.repaste(PasteMode::Keep);
        assert_eq!(look(&b.doc, 1, 1), (21, true, Some("Georgia".into())));
        // Unformatted, and text of another program, take the caret's formatting.
        let mut c = DocView::new(Document::default());
        c.paste(text, true);
        assert_eq!(look(&c.doc, 1, 1), (24, false, None));
        c.paste("from elsewhere", false);
        assert!(c.paste_opts.as_ref().is_some_and(|p| p.paras.is_none()));
    }

    #[test]
    fn a_copy_reaches_other_programs_and_other_apexdesks_formatted() {
        use crate::sysclip::{DOC, HTML, RTF, TEXT};
        let ctx = egui::Context::default();
        let mut p = Paragraph::new("");
        p.push("Big ", apex_doc::Attrs { size: Some(40), ..Default::default() });
        p.push("bold", apex_doc::Attrs { bold: true, ..Default::default() });
        let mut a = DocView::new(Document::from_paragraphs(vec![p, Paragraph::new("second")]));
        select_all(&mut a);
        a.copy(&ctx, false);
        // Every format is there, the text in each.
        let formats = crate::sysclip::formats();
        assert_eq!(formats, [TEXT, HTML, RTF, DOC]);
        let html = crate::sysclip::text_of(&crate::sysclip::read(&[HTML]).unwrap().1);
        assert!(html.contains("font-size:20pt") && html.contains("bold") && html.contains("second"), "{html}");
        // ApexDesk in another process reads its own format first.
        let mut b = DocView::new(Document::default());
        let paras = b.system_clip("Big bold\nsecond").expect("formatted text");
        assert_eq!(paras.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(), ["Big bold", "second"]);
        assert_eq!((paras[0].attrs_at(1).size, paras[0].attrs_at(5).bold), (Some(40), true));
    }

    #[test]
    fn rtf_and_html_of_other_programs_paste_formatted() {
        use crate::sysclip::{HTML, RTF, TEXT};
        let mut b = DocView::new(Document::from_paragraphs(vec![Paragraph::new("[]")]));
        b.sel = Selection::caret(Pos::new(0, 1));
        // A word processor: RTF, with its closing paragraph mark.
        let rtf = br"{\rtf1\ansi{\fonttbl{\f0 Arial;}}\f0\fs24 plain {\b bold}\par}";
        crate::sysclip::write(&[(TEXT, b"plain bold"), (RTF, rtf), (HTML, b"<p>html</p>")]);
        b.paste_from("plain bold", false, true);
        assert_eq!(b.doc.paragraphs.len(), 1);
        assert_eq!(b.doc.paragraphs[0].text, "[plain bold]");
        assert!(b.doc.paragraphs[0].attrs_at(8).bold && !b.doc.paragraphs[0].attrs_at(3).bold);
        // A browser: HTML only.
        let mut c = DocView::new(Document::default());
        crate::sysclip::write(&[(TEXT, b"Title\nitem"), (HTML, b"<h1>Title</h1><ul><li><i>item</i></li></ul>")]);
        c.paste_from("Title\nitem", false, true);
        let kinds: Vec<_> = c.doc.paragraphs.iter().map(|p| (p.text.as_str(), p.style.kind)).collect();
        assert_eq!(kinds[..2], [("Title", ParaKind::Heading(1)), ("item", ParaKind::Bullet)]);
        assert!(c.doc.paragraphs[1].attrs_at(1).italic);
        // Unformatted paste (Ctrl+Shift+V) and pastes not from the system
        // clipboard leave it alone.
        let mut d = DocView::new(Document::default());
        d.paste_from("Title\nitem", true, true);
        assert_eq!(d.doc.paragraphs[0].style.kind, ParaKind::Normal);
        let mut e = DocView::new(Document::default());
        e.paste("Title\nitem", false);
        assert_eq!(e.doc.paragraphs[0].style.kind, ParaKind::Normal);
    }

    #[test]
    fn a_web_page_pastes_its_lists_and_spacing() {
        use crate::sysclip::{HTML, TEXT};
        // A browser on Windows: the header, the context, the fragment.
        let html = "Version:0.9\r\nStartHTML:0000000097\r\n<html><body>\r\n<!--StartFragment--><p style=\"line-height: 1.5; margin: 0px 0px 12px;\">Intro</p><ul style=\"list-style-type: '- ';\"><li>one</li><li>two</li></ul><!--EndFragment-->\r\n</body></html>";
        let text = "Intro\n- one\n- two";
        crate::sysclip::write(&[(TEXT, text.as_bytes()), (HTML, html.as_bytes())]);
        let mut v = DocView::new(Document::default());
        v.paste_from(text, false, true);
        let look = |v: &DocView| -> Vec<(String, ParaKind, u8)> { v.doc.paragraphs.iter().map(|p| (p.text.clone(), p.style.kind, p.style.list_format)).collect() };
        let kept = vec![("Intro".to_string(), ParaKind::Normal, 0), ("one".to_string(), ParaKind::Bullet, 4), ("two".to_string(), ParaKind::Bullet, 4)];
        assert_eq!(look(&v), kept);
        let spacing = |v: &DocView| (v.doc.paragraphs[0].style.line_spacing, v.doc.paragraphs[0].style.space_after);
        assert_eq!(spacing(&v), (1.5, Some(9.0)));
        // Taking the formatting where it goes keeps the list marks; keeping
        // the source's brings the spacing back.
        v.repaste(PasteMode::Merge);
        assert_eq!(look(&v), kept);
        assert_eq!(spacing(&v), (0.0, None));
        v.repaste(PasteMode::Keep);
        assert_eq!(spacing(&v), (1.5, Some(9.0)));
    }

    #[test]
    fn a_picture_copied_in_word_is_no_object_mark() {
        use crate::sysclip::{RTF, TEXT};
        const PNG: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0,
            0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        let hex: String = PNG.iter().map(|b| format!("{b:02x}")).collect();
        let mark = apex_doc::OBJECT.to_string();
        // Word's RTF: the picture comes in, once.
        let rtf = format!(r"{{\rtf1\ansi{{\*\shppict{{\pict\picwgoal300\pichgoal300\pngblip {hex}}}}}{{\nonshppict{{\pict\wmetafile8 0100090000}}}}\par}}");
        crate::sysclip::write(&[(TEXT, mark.as_bytes()), (RTF, rtf.as_bytes())]);
        let mut v = DocView::new(Document::default());
        v.paste_from(&mark, false, true);
        let pics: Vec<_> = v.doc.paragraphs.iter().flat_map(|p| p.runs.iter().filter_map(|r| r.attrs.image.clone())).collect();
        assert_eq!(pics.len(), 1);
        assert_eq!(&pics[0].data[..], PNG);
        // Without formatted text the mark is no text: nothing goes in (the
        // clipboard's picture does), and around text it goes.
        crate::sysclip::write(&[(TEXT, mark.as_bytes())]);
        let mut w = DocView::new(Document::default());
        w.paste_from(&mark, false, true);
        assert_eq!(w.doc.paragraphs[0].text, "");
        assert!(w.paste_opts.is_none());
        w.paste_from(&format!("a{mark}b"), false, true);
        assert_eq!(w.doc.paragraphs[0].text, "ab");
    }
}
