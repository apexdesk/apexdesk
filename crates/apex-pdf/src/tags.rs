//! Tagged PDF (ISO 32000-1, 14.8): the logical structure that screen
//! readers and reflow use. The caller says which structure element the
//! content drawn next belongs to ([`Pdf::tag`]) or that it is decoration
//! ([`Pdf::artifact`]); each change closes the marked-content sequence of
//! the page and opens another with the next MCID. Elements are children
//! of one Document element, in the order they are first used; one that
//! goes on over several pages (a paragraph) keeps its key.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::Pdf;

/// What the content being drawn belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mark {
    Artifact,
    Elem(usize),
}

pub(crate) struct Elem {
    /// Standard structure type: P, H1..H6, Figure, Table...
    pub(crate) role: String,
    pub(crate) alt: Option<String>,
    /// Marked content: (page index, MCID).
    pub(crate) kids: Vec<(usize, usize)>,
}

#[derive(Default)]
pub(crate) struct Tree {
    /// Natural language of the text (BCP 47), for the catalog.
    pub(crate) lang: String,
    pub(crate) elems: Vec<Elem>,
    by_key: HashMap<u64, usize>,
}

/// Structure types a caller may use; others become P.
const ROLES: &[&str] = &["P", "H1", "H2", "H3", "H4", "H5", "H6", "Figure", "Table", "L", "LI", "Caption", "Note", "Formula", "Code", "Quote", "BlockQuote", "Link", "Span"];

impl Pdf {
    /// Makes the document a tagged PDF in language `lang` (e.g. "es-ES").
    pub fn set_tagged(&mut self, lang: &str) {
        self.tags = Some(Tree { lang: lang.to_string(), ..Default::default() });
    }

    pub fn is_tagged(&self) -> bool {
        self.tags.is_some()
    }

    /// The content drawn next belongs to the element `key` (made with
    /// `role` and alternative text `alt` when first used).
    pub fn tag(&mut self, key: u64, role: &str, alt: Option<&str>) {
        let Some(t) = self.tags.as_mut() else { return };
        let i = *t.by_key.entry(key).or_insert_with(|| {
            let role = if ROLES.contains(&role) { role } else { "P" };
            t.elems.push(Elem { role: role.to_string(), alt: alt.filter(|a| !a.trim().is_empty()).map(str::to_string), kids: Vec::new() });
            t.elems.len() - 1
        });
        self.set_mark(Mark::Elem(i));
    }

    /// The content drawn next is decoration (page backgrounds, headers
    /// and footers, borders), which readers skip.
    pub fn artifact(&mut self) {
        if self.tags.is_some() {
            self.set_mark(Mark::Artifact);
        }
    }

    fn set_mark(&mut self, m: Mark) {
        let page = self.pages.len();
        let (Some(t), Some(p)) = (self.tags.as_mut(), self.cur.as_mut()) else { return };
        if p.mark == Some(m) {
            return;
        }
        if p.mark.is_some() {
            p.ops.push_str("EMC\n");
        }
        match m {
            Mark::Artifact => p.ops.push_str("/Artifact BMC\n"),
            Mark::Elem(i) => {
                let mcid = p.mcids.len();
                p.mcids.push(i);
                let e = &mut t.elems[i];
                e.kids.push((page, mcid));
                let _ = writeln!(p.ops, "/{} <</MCID {mcid}>> BDC", e.role);
            }
        }
        p.mark = Some(m);
    }
}

#[cfg(test)]
mod tests {
    use crate::Pdf;

    #[test]
    fn structure() {
        let mut pdf = Pdf::new();
        pdf.set_tagged("es-ES");
        pdf.begin_page(200.0, 200.0);
        pdf.artifact();
        pdf.fill_rect(0.0, 0.0, 10.0, 10.0, [0, 0, 0]);
        pdf.tag(1, "H1", None);
        pdf.fill_rect(0.0, 20.0, 10.0, 10.0, [0, 0, 0]);
        pdf.tag(1, "H1", None);
        pdf.tag(2, "Figure", Some("A cat"));
        pdf.fill_rect(0.0, 40.0, 10.0, 10.0, [0, 0, 0]);
        pdf.begin_page(200.0, 200.0);
        pdf.tag(2, "Figure", None);
        pdf.fill_rect(0.0, 40.0, 10.0, 10.0, [0, 0, 0]);
        let t = pdf.tags.as_ref().unwrap();
        assert_eq!(t.elems.len(), 2);
        assert_eq!(t.elems[1].kids, [(0, 1), (1, 0)]);
        let out = pdf.finish();
        let s = String::from_utf8_lossy(&out);
        for part in ["/StructTreeRoot", "/MarkInfo << /Marked true >>", "/Lang (es-ES)", "/S /Figure", "/Alt <FEFF", "/StructParents 1", "/ParentTree"] {
            assert!(s.contains(part), "{part}");
        }
    }
}
