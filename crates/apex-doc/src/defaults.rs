//! The document defaults: the formatting every style starts from (Word's
//! `w:docDefaults`, the ODF default paragraph style) as the opened file
//! has it. Documents made in ApexDesk have none and are saved with our
//! own defaults. The fonts of the defaults are `Document::default_font`
//! and `Styles::asian`.

use std::sync::Arc;

use crate::{Document, LineRule, ParaKind};

#[derive(Clone, Debug, PartialEq, Default)]
pub struct DocDefaults {
    /// Font size in half points (`None`: the program's, 10 pt in Word).
    pub size: Option<u16>,
    /// Complex script font size in half points.
    pub cs_size: Option<u16>,
    /// Language of Latin text ("en-US").
    pub lang: Option<Arc<str>>,
    /// Space before and after paragraphs in points.
    pub space_before: Option<f32>,
    pub space_after: Option<f32>,
    /// Line spacing: a multiple of single spacing or points by the rule.
    pub line: Option<(LineRule, f32)>,
    /// The defaults as the file has them (see `FileDefaults`).
    pub file: Option<Arc<FileDefaults>>,
}

/// What the file's defaults have besides the values ApexDesk reads, to
/// write them back as they were.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct FileDefaults {
    /// The file format ("docx", "odt"); the rest only means something in it.
    pub format: Arc<str>,
    /// The defaults' XML in the file, and as ApexDesk writes it from the
    /// values it read: while that doesn't change, the file's XML is
    /// written as it is.
    pub xml: Arc<str>,
    pub written: Arc<str>,
    /// Settings that aren't read (elements or attributes), by the part of
    /// the defaults they are in, written back after a change.
    pub other: Vec<(Arc<str>, Arc<str>)>,
}

impl ParaKind {
    /// Kinds whose styles take their size from Normal (see
    /// `Styles::base_size`).
    pub fn inherits_size(self) -> bool {
        matches!(self, ParaKind::Normal | ParaKind::Quote | ParaKind::Bullet | ParaKind::Numbered)
    }
}

impl crate::Styles {
    /// Font size (half points) text of a kind starts from: headings,
    /// titles and code have their own; normal text, quotes and list
    /// paragraphs take the Normal style's, else the document defaults'.
    pub fn base_size(&self, kind: ParaKind) -> u16 {
        if !kind.inherits_size() {
            return kind.base_size();
        }
        self.builtin(ParaKind::Normal).and_then(|d| d.size).or_else(|| self.defaults.as_ref().and_then(|d| d.size)).filter(|&s| s > 0).unwrap_or(kind.base_size())
    }
}

impl Document {
    /// See `Styles::base_size`.
    pub fn base_size(&self, kind: ParaKind) -> u16 {
        self.styles.base_size(kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Attrs, ParaStyle, StyleDef};

    #[test]
    fn size_comes_from_normal_then_defaults() {
        let mut doc = Document::default();
        assert_eq!(doc.base_size(ParaKind::Normal), 24);
        doc.styles.defaults = Some(DocDefaults { size: Some(21), ..Default::default() });
        assert_eq!(doc.base_size(ParaKind::Bullet), 21);
        assert_eq!(doc.base_size(ParaKind::Heading(1)), 40);
        let f = doc.char_format(&ParaStyle { kind: ParaKind::Quote, ..Default::default() }, &Attrs::default());
        assert_eq!(f.size, 21);
        doc.styles.defs.push(StyleDef { id: "Normal".into(), kind: ParaKind::Normal, builtin: true, size: Some(22), ..Default::default() });
        assert_eq!(doc.base_size(ParaKind::Numbered), 22);
        assert_eq!(doc.normal_size_pt(), 11.0);
    }
}
