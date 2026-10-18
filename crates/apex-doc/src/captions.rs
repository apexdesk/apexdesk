//! Captions ("Ilustración 1: …", numbered by a SEQ field) and the tables
//! of figures listing them.

use std::sync::Arc;

use crate::{Attrs, Document, EditKind, Field, FieldKind, OBJECT, ParaStyle, Paragraph, Pos, Refs, Selection, StyleDef};

/// The families of caption labels a table of figures lists: 1 figures,
/// 2 tables, 3 equations; 0 for other labels.
pub fn caption_kind(label: &str) -> u8 {
    let l = label.to_lowercase();
    let l = l.trim_start_matches('_');
    if ["fig", "ilus", "illus", "imag"].iter().any(|p| l.starts_with(p)) {
        1
    } else if ["tab"].iter().any(|p| l.starts_with(p)) {
        2
    } else if ["ec", "eq"].iter().any(|p| l.starts_with(p)) {
        3
    } else {
        0
    }
}

/// The style id of captions (Word's "Caption").
pub const CAPTION_STYLE: &str = "Caption";

impl Document {
    /// The SEQ label of paragraph `i`'s first sequence field.
    fn seq_label(&self, i: usize) -> Option<(usize, String)> {
        let p = &self.paragraphs[i];
        if !crate::para_has_objects(p) {
            return None;
        }
        crate::objects_of(p).into_iter().find_map(|(off, a)| match a.field.as_deref().map(|f| &f.kind) {
            Some(FieldKind::Seq(n)) => Some((off, n.clone())),
            _ => None,
        })
    }

    /// Paragraphs holding a caption of `kind` (see [`caption_kind`]),
    /// outside tables of contents.
    pub fn captions(&self, kind: u8) -> Vec<usize> {
        (0..self.paragraphs.len()).filter(|&i| self.paragraphs[i].style.toc.is_none() && self.seq_label(i).is_some_and(|(_, n)| caption_kind(&n) == kind)).collect()
    }

    /// The label the captions of `kind` use in this document.
    pub fn caption_label(&self, kind: u8) -> Option<String> {
        self.captions(kind).first().and_then(|&i| self.seq_label(i)).map(|(_, n)| n.replace('_', " "))
    }

    /// Caption `i` as a table of figures shows it: its text with the
    /// numbers of its sequence fields.
    pub fn caption_text(&self, i: usize, refs: &Refs) -> String {
        let p = &self.paragraphs[i];
        let seqs: Vec<usize> = if crate::para_has_objects(p) { crate::objects_of(p).into_iter().filter(|(_, a)| matches!(a.field.as_deref().map(|f| &f.kind), Some(FieldKind::Seq(_)))).map(|(off, _)| off).collect() } else { Vec::new() };
        let mut out = String::new();
        for (off, c) in p.text.char_indices() {
            if c == OBJECT {
                if seqs.contains(&off) {
                    if let Some(n) = refs.seq(Pos::new(i, off)) {
                        out.push_str(&n.to_string());
                    }
                }
            } else if c == crate::LINE_BREAK || c == '\t' {
                out.push(' ');
            } else {
                out.push(c);
            }
        }
        out.trim().to_string()
    }

    /// A copy whose sequence and formula fields hold their current values
    /// as their results (for files, which show the results); None when they do.
    pub fn with_seq_results(&self) -> Option<Document> {
        let refs = self.refs();
        let mut out: Option<Document> = None;
        for i in 0..self.paragraphs.len() {
            if !crate::para_has_objects(&self.paragraphs[i]) {
                continue;
            }
            let mut off = 0;
            for k in 0..self.paragraphs[i].runs.len() {
                let r = &self.paragraphs[i].runs[k];
                let (len, field) = (r.len, r.attrs.field.clone());
                if let Some(f) = field.filter(|f| matches!(f.kind, FieldKind::Seq(_) | FieldKind::Formula(_))) {
                    let at = Pos::new(i, off);
                    let v = match &f.kind {
                        FieldKind::Formula(e) => self.formula_value(at, e).ok().map(|v| crate::format_number(v, f.format.as_deref(), '.')),
                        _ => refs.seq(at).map(|n| n.to_string()),
                    };
                    if let Some(n) = v.filter(|n| *n != f.cached) {
                        let d = out.get_or_insert_with(|| self.clone());
                        d.paragraphs[i].runs[k].attrs.field = Some(Arc::new(Field { cached: n, ..(*f).clone() }));
                    }
                }
                off += len;
            }
        }
        out
    }

    /// Inserts a caption "`label` N`text`" in a paragraph of its own above
    /// or below the paragraph at the caret (or the table holding it),
    /// with the Caption style (`style_name` names it when it is added).
    pub fn insert_caption(&mut self, sel: Selection, label: &str, text: &str, above: bool, style_name: &str) -> Selection {
        let p = self.clamp(sel.cursor);
        let (a, b) = match self.all_tables().into_iter().find(|t| t.depth == 0 && (t.start..t.end).contains(&p.para)) {
            Some(t) => (t.start, t.end),
            None => (p.para, p.para + 1),
        };
        let at = if above { a } else { b };
        let styles = self.styles.clone();
        let custom = match self.styles.custom_number(CAPTION_STYLE) {
            Some(n) => n,
            None => {
                self.styles.defs.push(StyleDef {
                    id: Arc::from(CAPTION_STYLE),
                    name: Arc::from(style_name),
                    italic: Some(true),
                    size: Some(18),
                    color: Some(0x44546A),
                    space_after: Some(10.0),
                    ..Default::default()
                });
                self.styles.defs.len() as u16
            }
        };
        let mut st = ParaStyle { custom, ..Default::default() };
        if let Some(d) = self.styles.custom(custom) {
            d.apply_para(&mut st);
        }
        let mut para = Paragraph::new("").with_style(st);
        para.push(&format!("{label} "), Attrs::default());
        let field = Field::new(FieldKind::Seq(label.trim().replace(' ', "_")));
        para.push(&OBJECT.to_string(), Attrs { field: Some(Arc::new(field)), ..Default::default() });
        para.push(text, Attrs::default());
        let end = para.len();
        let caret = Selection::caret(Pos::new(at, end));
        self.replace_paras(at, at, vec![para], sel, caret, EditKind::Field);
        if self.styles != styles {
            if let Some(e) = self.undo.last_mut() {
                e.styles = Some(Box::new(styles));
            }
        }
        caret
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TocOpts;

    #[test]
    fn captions_and_figures() {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("Índice"), Paragraph::new("foto"), Paragraph::new("otra"), Paragraph::new("fin")]);
        let s = d.insert_caption(Selection::caret(Pos::new(1, 0)), "Ilustración", ": Una foto", false, "Descripción");
        assert_eq!(s.cursor.para, 2);
        d.insert_caption(Selection::caret(Pos::new(3, 0)), "Ilustración", ": Otra", false, "Descripción");
        d.insert_caption(Selection::caret(Pos::new(0, 0)), "Tabla", "", true, "Descripción");
        assert_eq!(d.styles.defs.len(), 1);
        assert_eq!(d.captions(1).len(), 2);
        assert_eq!(caption_kind("Figure"), 1);
        let refs = d.refs();
        let texts: Vec<String> = d.captions(1).into_iter().map(|i| d.caption_text(i, &refs)).collect();
        assert_eq!(texts, ["Ilustración 1: Una foto", "Ilustración 2: Otra"]);
        assert_eq!(d.caption_label(1).as_deref(), Some("Ilustración"));
        // A table of figures lists them.
        let n = d.paragraphs.len();
        d.insert_toc(Selection::caret(Pos::new(n - 1, 0)), TocOpts { caption: 1, ..TocOpts::default() }, "-");
        let toc: Vec<&str> = d.paragraphs.iter().filter(|p| p.style.toc.is_some()).map(|p| p.text.split('\t').next().unwrap_or("")).collect();
        assert_eq!(toc, ["Ilustración 1: Una foto", "Ilustración 2: Otra"]);
        d.undo();
        d.undo();
        d.undo();
        d.undo();
        assert!(d.styles.defs.is_empty());
        assert_eq!(d.paragraphs.len(), 4);
    }
}
