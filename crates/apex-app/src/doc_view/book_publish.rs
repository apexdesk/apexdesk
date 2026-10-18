//! Publishing a book: a manuscript for publishers or a book to print, in a
//! new tab (the latter exported to PDF), or an EPUB with its cover.

use super::*;
use crate::widgets::DialogResult;
use apex_doc::book_publish::{Publish, SceneBreak, TRIM_SIZES};

/// What is published.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Output {
    Manuscript,
    Print,
    Epub,
}

/// Fonts offered for a manuscript.
const FONTS: [&str; 2] = ["Times New Roman", "Courier New"];

/// The publish dialog, while open.
pub(super) struct PublishDialog {
    out: Output,
    title: String,
    author: String,
    breaks: SceneBreak,
    font: usize,
    trim: usize,
    cover: Option<std::path::PathBuf>,
}

fn break_name(b: SceneBreak) -> String {
    match b {
        SceneBreak::Hashes => "###".to_string(),
        SceneBreak::Stars => "* * *".to_string(),
        SceneBreak::Blank => tr("book.p_blank").to_string(),
        SceneBreak::Titles => tr("book.p_titles").to_string(),
    }
}

/// `words` as a manuscript gives them: to the hundred, to the thousand
/// in a long book.
fn about(words: usize) -> usize {
    let step = if words >= 20_000 { 1000 } else { 100 };
    ((words + step / 2) / step * step).max(step)
}

/// The media type of a picture file.
fn image_type(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref() {
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "image/jpeg",
    }
}

impl DocView {
    pub(super) fn open_publish(&mut self) {
        let title = if self.doc.meta.title.trim().is_empty() {
            std::path::Path::new(&self.file_name).file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string()
        } else {
            self.doc.meta.title.clone()
        };
        let author = if self.doc.meta.author.trim().is_empty() { self.doc.user.to_string() } else { self.doc.meta.author.clone() };
        self.book.publish = Some(PublishDialog { out: Output::Manuscript, title, author, breaks: SceneBreak::Hashes, font: 0, trim: 0, cover: None });
    }

    fn publish_with(&mut self, d: &PublishDialog) -> Publish {
        let (words, ..) = self.book_stats();
        let author = d.author.trim().to_string();
        Publish {
            title: d.title.trim().to_string(),
            by: trf("book.p_by", &[("name", &author)]),
            author,
            breaks: d.breaks,
            about_words: trf("book.p_about", &[("n", &about(words))]),
        }
    }

    /// Publishes as `d` says.
    pub(super) fn publish(&mut self, d: &PublishDialog) {
        let p = self.publish_with(d);
        match d.out {
            Output::Manuscript => self.book.open = Some((DocToOpen(Arc::new(self.doc.manuscript(&p, FONTS[d.font]))), false)),
            Output::Print => {
                let (_, w, h) = TRIM_SIZES[d.trim];
                self.book.open = Some((DocToOpen(Arc::new(self.doc.print_book(&p, (w, h)))), true));
            }
            Output::Epub => {
                let doc = self.doc.ebook(&p);
                let cover = d.cover.as_ref().and_then(|c| apex_fs::read(c).ok().map(|b| (b, image_type(c))));
                let lang = self.review_lang();
                let name = if p.title.is_empty() { "book".to_string() } else { p.title.clone() };
                crate::file_dialog::FileDialog::new().set_title(tr("book.p_epub_save")).add_filter("EPUB", &["epub"]).set_file_name(format!("{name}.epub")).save_file(crate::file_dialog::in_view(move |v: &mut DocView, path: std::path::PathBuf| {
                    let bytes = apex_io::html::book_epub(&doc, cover.as_ref().map(|(b, t)| (b.as_slice(), *t)), &lang);
                    match crate::common::write_out(&path, bytes) {
                        Ok(()) => v.notices.push(Notice::info(trf("book.p_saved", &[("name", &path.display())]))),
                        Err(e) => v.notices.push(Notice::error(e.to_string())),
                    }
                }));
            }
        }
    }

    pub(super) fn publish_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = self.book.publish.as_mut() else { return };
        let mut pick_cover = false;
        let r = crate::widgets::dialog(ctx, "doc_book_publish", tr("book.publish_title"), 440.0, |ui| {
            for (o, key) in [(Output::Manuscript, "book.p_manuscript"), (Output::Print, "book.p_print"), (Output::Epub, "book.p_epub")] {
                ui.radio_value(&mut d.out, o, tr(key));
            }
            ui.add_space(6.0);
            egui::Grid::new("book_publish").num_columns(2).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
                ui.label(tr("book.p_book_title"));
                ui.add(egui::TextEdit::singleline(&mut d.title).desired_width(260.0));
                ui.end_row();
                ui.label(tr("book.p_author"));
                ui.add(egui::TextEdit::singleline(&mut d.author).desired_width(260.0));
                ui.end_row();
                ui.label(tr("book.p_breaks"));
                egui::ComboBox::from_id_salt("book_publish_breaks").width(180.0).selected_text(break_name(d.breaks)).show_ui(ui, |ui| {
                    for b in SceneBreak::ALL {
                        ui.selectable_value(&mut d.breaks, b, break_name(b));
                    }
                });
                ui.end_row();
                match d.out {
                    Output::Manuscript => {
                        ui.label(tr("book.p_font"));
                        egui::ComboBox::from_id_salt("book_publish_font").width(180.0).selected_text(FONTS[d.font]).show_ui(ui, |ui| {
                            for (i, f) in FONTS.iter().enumerate() {
                                ui.selectable_value(&mut d.font, i, *f);
                            }
                        });
                    }
                    Output::Print => {
                        ui.label(tr("book.p_trim"));
                        egui::ComboBox::from_id_salt("book_publish_trim").width(180.0).selected_text(TRIM_SIZES[d.trim].0).show_ui(ui, |ui| {
                            for (i, t) in TRIM_SIZES.iter().enumerate() {
                                ui.selectable_value(&mut d.trim, i, t.0);
                            }
                        });
                    }
                    Output::Epub => {
                        ui.label(tr("book.p_cover"));
                        ui.horizontal(|ui| {
                            let name = d.cover.as_ref().and_then(|c| c.file_name()).map_or_else(|| tr("book.p_no_cover").to_string(), |n| n.to_string_lossy().into_owned());
                            ui.label(name);
                            if ui.button(tr("book.p_pick_cover")).clicked() {
                                pick_cover = true;
                            }
                            if d.cover.is_some() && ui.button(tr("book.p_no_cover_btn")).clicked() {
                                d.cover = None;
                            }
                        });
                    }
                }
                ui.end_row();
            });
            let note = match d.out {
                Output::Manuscript => "book.p_manuscript_note",
                Output::Print => "book.p_print_note",
                Output::Epub => "book.p_epub_note",
            };
            ui.add_space(4.0);
            ui.label(RichText::new(tr(note)).weak().small());
        });
        if pick_cover {
            crate::file_dialog::FileDialog::new().set_title(tr("book.p_pick_cover")).add_filter(tr("pic.files"), &["png", "jpg", "jpeg", "gif", "webp"]).pick_file(crate::file_dialog::in_view(|v: &mut DocView, path: std::path::PathBuf| {
                if let Some(d) = v.book.publish.as_mut() {
                    d.cover = Some(path);
                }
            }));
        }
        match r {
            DialogResult::None => {}
            DialogResult::Ok => {
                let d = self.book.publish.take().unwrap_or_else(|| unreachable!());
                self.publish(&d);
            }
            DialogResult::Cancel => self.book.publish = None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heading(t: &str, level: u8) -> Paragraph {
        let mut p = Paragraph::new(t);
        p.style.kind = apex_doc::ParaKind::Heading(level);
        p
    }

    #[test]
    fn a_book_is_published_in_a_new_tab() {
        assert_eq!((about(0), about(149), about(150), about(63_400)), (100, 100, 200, 63_000));
        let mut v = DocView::new(Document::from_paragraphs(vec![heading("Uno", 1), heading("Llega", 2), Paragraph::new("Ana llega."), heading("Cena", 2), Paragraph::new("Cenan.")]));
        v.make_book();
        v.file_name = "El puerto.docx".into();
        v.doc.meta.author = "Ana Pérez".into();
        v.open_publish();
        let d = v.book.publish.take().unwrap();
        assert_eq!((d.title.as_str(), d.author.as_str()), ("El puerto", "Ana Pérez"));
        v.publish(&d);
        let (m, pdf) = v.book.open.take().unwrap();
        assert!(!pdf);
        assert_eq!(m.0.paragraphs[0].text, "Ana Pérez");
        assert!(m.0.paragraphs.iter().any(|p| p.text == "###"));
        v.publish(&PublishDialog { out: Output::Print, trim: 1, ..d });
        let (b, pdf) = v.book.open.take().unwrap();
        assert!(pdf && b.0.page.mirror);
        assert_eq!(b.0.page.width, 432.0);
    }
}
