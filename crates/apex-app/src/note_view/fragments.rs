//! Fragments: a passage marked in a PDF or a document open in ApexDesk,
//! sent to the notebook as a page under its source, with a link back to
//! where it was.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use apex_doc::notebook::{Rel, links as note_links};
use apex_doc::{Attrs, ParaKind, Selection, Source, SourceKind};

use super::*;

/// Links back to the original start with this.
pub const ORIGIN: &str = "origin:";

/// Where a fragment was in its file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Spot {
    /// A page of a PDF, from 0.
    Page(usize),
    /// A paragraph of a document, from 0.
    Para(usize),
}

/// The link back to `spot` in `path`.
pub fn origin(path: &Path, spot: Spot) -> Arc<str> {
    let at = match spot {
        Spot::Page(p) => format!("p{p}"),
        Spot::Para(p) => format!("q{p}"),
    };
    format!("{ORIGIN}{at}:{}", path.display()).into()
}

/// The file and spot a link back points at.
pub fn parse_origin(target: &str) -> Option<(PathBuf, Spot)> {
    let (at, path) = target.strip_prefix(ORIGIN)?.split_once(':')?;
    let spot = match at.split_at_checked(1)? {
        ("p", n) => Spot::Page(n.parse().ok()?),
        ("q", n) => Spot::Para(n.parse().ok()?),
        _ => return None,
    };
    (!path.is_empty()).then(|| (PathBuf::from(path), spot))
}

impl NoteView {
    /// Keeps `text`, marked at `spot` of the file at `path` (titled
    /// `title`), as a page under that file's source page, made in the
    /// section shown when missing. Returns the new page.
    pub fn add_fragment(&mut self, text: &str, path: &Path, spot: Spot, title: &str) -> i64 {
        self.flush();
        let steps = self.undo.len();
        let url = path.display().to_string();
        let found = self.nb.pages.iter().find(|p| self.nb.is_live(p.id) && p.source.as_ref().is_some_and(|s| s.url == url)).map(|p| (p.id, p.section));
        let (source, section) = match found {
            Some(f) => f,
            None => {
                let section = self.here();
                let mut id = 0;
                self.change(|nb| {
                    let last = nb.children(section, None).last().map(|p| p.id);
                    id = nb.add_page(section, None, last, title, now());
                    if let Some(p) = nb.page_mut(id) {
                        p.source = Some(Source { kind: SourceKind::Other, title: title.to_string(), url: url.clone(), ..Source::default() });
                    }
                });
                (id, section)
            }
        };
        let name = inbox::idea_title(text);
        let mut id = 0;
        self.change(|nb| {
            let last = nb.children(section, Some(source)).last().map(|p| p.id);
            id = nb.add_page(section, Some(source), last, &name, now());
        });
        // One step for all.
        self.undo.truncate(steps + 1);
        let source_title = self.nb.page(source).map(|p| p.title.clone()).unwrap_or_default();
        let doc = fragment_doc(text, source, &source_title, path, spot);
        let plain = plain_text(&doc);
        let found = note_links::page_links(&doc);
        let r = self.store.save_page(id, &doc, &plain, &found, now());
        self.nb.texts.insert(id, plain);
        self.nb.links.insert(id, found);
        self.nb.tasks.insert(id, apex_doc::notebook::tasks::tasks_in(&doc));
        self.report(r);
        self.publish_sources();
        if self.page.is_none() {
            self.open_page(id);
        }
        id
    }
}

/// The page of a fragment: the passage quoted, then where it comes from.
fn fragment_doc(text: &str, source: i64, source_title: &str, path: &Path, spot: Spot) -> Document {
    let mut paras: Vec<Paragraph> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| {
            let mut p = Paragraph::new(l);
            p.style.kind = ParaKind::Quote;
            p
        })
        .collect();
    paras.push(Paragraph::new("— "));
    let mut doc = Document::from_paragraphs(paras);
    let plain = Some(Attrs::default());
    let end = |d: &Document| Selection::caret(d.end());
    doc.insert_link(end(&doc), source_title, note_links::target(source, Rel::Link), None);
    let at = match spot {
        Spot::Page(p) => format!(", {} ", trf("nb.fragment_page", &[("n", &(p + 1))])),
        Spot::Para(_) => " ".to_string(),
    };
    doc.insert_text(end(&doc), &format!("{at}· "), plain);
    doc.insert_link(end(&doc), &tr("nb.fragment_origin"), origin(path, spot), None);
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_back_names_the_file_and_the_spot() {
        let path = Path::new("/home/a/Libros/Faros: guía.pdf");
        assert_eq!(parse_origin(&origin(path, Spot::Page(4))), Some((path.to_path_buf(), Spot::Page(4))));
        assert_eq!(parse_origin(&origin(path, Spot::Para(12))), Some((path.to_path_buf(), Spot::Para(12))));
        assert_eq!(parse_origin("origin:x1:/a"), None);
        assert_eq!(parse_origin("note:3"), None);
    }
}
