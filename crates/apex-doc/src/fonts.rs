//! Fonts saved inside the document (Word's "Embed fonts in the file",
//! the fonts of an ODF package), so that it looks the same on computers
//! without them. The data is plain TrueType/OpenType; the file formats
//! obfuscate or package it (see apex-io).

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::{Document, EditKind, Selection};

/// One face of an embedded font.
#[derive(Clone, Debug, PartialEq)]
pub struct EmbeddedFont {
    pub family: Arc<str>,
    pub bold: bool,
    pub italic: bool,
    pub data: Arc<[u8]>,
}

/// The embedded fonts and whether saving embeds the fonts in use.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EmbeddedFonts {
    /// Save the fonts the document uses into the file.
    pub embed: bool,
    pub faces: Vec<EmbeddedFont>,
}

impl EmbeddedFonts {
    /// The families embedded, without repeats.
    pub fn families(&self) -> Vec<Arc<str>> {
        let mut v: Vec<Arc<str>> = Vec::new();
        for f in &self.faces {
            if !v.iter().any(|x| x.eq_ignore_ascii_case(&f.family)) {
                v.push(f.family.clone());
            }
        }
        v
    }
}

impl Document {
    /// Changes the embedded fonts (or the setting) as one undo step.
    pub fn set_embedded_fonts(&mut self, fonts: EmbeddedFonts, sel: Selection) {
        if fonts == self.fonts {
            return;
        }
        self.change_layout(None, sel, EditKind::Format, &mut |d| d.fonts = fonts.clone());
    }

    /// Font families the document's text uses (document default, styles,
    /// runs and shapes), theme tokens resolved, sorted by name.
    pub fn fonts_used(&self) -> Vec<Arc<str>> {
        let mut set: BTreeSet<Arc<str>> = BTreeSet::new();
        let theme = &self.styles.theme;
        let mut add = |f: &Option<Arc<str>>| {
            if let Some(f) = f {
                let f = theme.token_font(f).unwrap_or(f);
                if !f.trim().is_empty() && !f.starts_with('+') {
                    set.insert(f.clone());
                }
            }
        };
        add(&self.default_font);
        if self.default_font.is_none() && self.styles.theme_fonts {
            add(&Some(theme.fonts.minor.clone()));
        }
        for s in &self.styles.defs {
            add(&s.font);
        }
        for p in &self.paragraphs {
            for r in &p.runs {
                add(&r.attrs.font);
            }
        }
        set.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Attrs, Paragraph, Run};

    #[test]
    fn used_and_undo() {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("Hi")]);
        d.default_font = Some("Carlito".into());
        d.paragraphs[0].runs = vec![Run { len: 2, attrs: Attrs { font: Some("Gelasio".into()), ..Default::default() } }];
        assert_eq!(d.fonts_used(), [Arc::from("Carlito"), Arc::from("Gelasio")]);
        let face = EmbeddedFont { family: "Gelasio".into(), bold: false, italic: false, data: Arc::from(&b"font"[..]) };
        let sel = Selection::default();
        d.set_embedded_fonts(EmbeddedFonts { embed: true, faces: vec![face.clone(), EmbeddedFont { bold: true, ..face }] }, sel);
        assert_eq!(d.fonts.families(), [Arc::from("Gelasio")]);
        d.undo();
        assert!(!d.fonts.embed && d.fonts.faces.is_empty());
        d.redo();
        assert!(d.fonts.embed);
    }
}
