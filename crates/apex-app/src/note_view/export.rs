//! A page, or a section with its pages under their titles, written out as
//! Markdown, DOCX or PDF.

use std::path::PathBuf;

use apex_doc::notebook::canvas::PageKind;
use apex_doc::{ParaKind, Paragraph};

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Markdown,
    Docx,
    Pdf,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::Markdown, Format::Docx, Format::Pdf];

    fn ext(self) -> &'static str {
        match self {
            Format::Markdown => "md",
            Format::Docx => "docx",
            Format::Pdf => "pdf",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Format::Markdown => "Markdown",
            Format::Docx => "Word (DOCX)",
            Format::Pdf => "PDF",
        }
    }
}

/// What is written out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Page(i64),
    Section(i64),
}

fn heading(text: &str, level: usize) -> Paragraph {
    let mut p = Paragraph::new(text);
    p.style.kind = ParaKind::Heading(level.clamp(1, 6) as u8);
    p
}

impl NoteView {
    /// Page `id`'s text as kept (links to pages and files are left as
    /// their text, which means nothing outside the notebook).
    fn page_body(&self, id: i64) -> Vec<Paragraph> {
        if self.nb.page(id).is_none_or(|p| p.kind != PageKind::Text) {
            return Vec::new();
        }
        let Ok(doc) = self.store.load_page(id) else { return Vec::new() };
        let mut paras = doc.paragraphs;
        for p in &mut paras {
            for r in &mut p.runs {
                if r.attrs.link.as_deref().is_some_and(|l| l.starts_with(apex_doc::notebook::links::SCHEME) || l.starts_with(super::attachments::ATTACH)) {
                    r.attrs.link = None;
                    r.attrs.link_tip = None;
                }
            }
        }
        // A page that is only an empty line adds nothing.
        if paras.len() == 1 && paras[0].text.is_empty() {
            paras.clear();
        }
        paras
    }

    /// The document written out: a page under its title, or a section
    /// under its name with each page under a heading as deep as it is.
    pub fn export_doc(&mut self, scope: Scope) -> Document {
        self.flush();
        let mut paras = Vec::new();
        match scope {
            Scope::Page(id) => {
                let title = self.nb.page(id).map(|p| p.title.clone()).unwrap_or_default();
                paras.push(heading(&title, 1));
                paras.extend(self.page_body(id));
            }
            Scope::Section(s) => {
                paras.push(heading(&self.nb.section(s).map(|s| s.name.clone()).unwrap_or_default(), 1));
                let pages: Vec<(usize, i64, String)> = self.nb.tree(s).into_iter().map(|(depth, p)| (depth, p.id, p.title.clone())).collect();
                for (depth, id, title) in pages {
                    paras.push(heading(&title, depth + 2));
                    paras.extend(self.page_body(id));
                }
            }
        }
        Document::from_paragraphs(paras)
    }

    /// Asks where to write `scope` as `format`, and writes it.
    pub(super) fn pick_export(&mut self, scope: Scope, format: Format) {
        let name = match scope {
            Scope::Page(id) => self.nb.page(id).map(|p| p.title.clone()),
            Scope::Section(s) => self.nb.section(s).map(|s| s.name.clone()),
        }
        .unwrap_or_default();
        let name = name.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "-");
        let name = if name.trim().is_empty() { tr("nb.untitled").to_string() } else { name };
        let ext = format.ext();
        crate::file_dialog::FileDialog::new()
            .set_title(tr("nb.export"))
            .set_file_name(format!("{name}.{ext}"))
            .add_filter(format.name(), &[ext])
            .save_file(crate::file_dialog::in_view(move |v: &mut NoteView, mut path: PathBuf| {
                if path.extension().is_none_or(|e| !e.eq_ignore_ascii_case(ext)) {
                    path.as_mut_os_string().push(format!(".{ext}"));
                }
                v.export(scope, format, &path);
            }));
    }

    pub fn export(&mut self, scope: Scope, format: Format, path: &Path) {
        let doc = self.export_doc(scope);
        let data = match format {
            Format::Markdown => apex_io::markdown::write(&doc.shared()).into_bytes(),
            Format::Docx => apex_io::docx::write(&doc),
            Format::Pdf => {
                let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                crate::headless::pdf(apex_tui::Book::Doc(&doc), &stem)
            }
        };
        match crate::common::write_out(path, data) {
            Ok(()) => self.notices.push(Notice::info(trf("nb.exported", &[("file", &path.display())]))),
            Err(e) => self.notices.push(Notice::error(e.to_string())),
        }
    }

    /// The toolstrip's menu: the page or the section, in each format.
    pub(super) fn export_menu(&mut self, ui: &mut egui::Ui) {
        let page = self.page.filter(|&p| self.nb.page(p).is_some_and(|p| p.kind == PageKind::Text));
        for (label, scope) in [("nb.export_page", page.map(Scope::Page)), ("nb.export_section", self.section.map(Scope::Section))] {
            ui.label(RichText::new(tr(label)).weak().small());
            for f in Format::ALL {
                if ui.add_enabled(scope.is_some(), egui::Button::new(format!("{}…", f.name()))).clicked()
                    && let Some(s) = scope
                {
                    self.pick_export(s, f);
                    ui.close();
                }
            }
        }
    }
}
