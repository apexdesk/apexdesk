//! The document's side of File > Inspect File (the dialog is the app's,
//! see `app::inspector_ui`): removing what the inspector found.

use apex_io::inspect::{self, Hidden};

use super::*;

impl DocView {
    /// Removes what the inspector found of `what`, as one undo step.
    pub(crate) fn inspect_clean(&mut self, what: Hidden) -> bool {
        self.clear_block();
        let done = inspect::clean_document(&mut self.doc, what);
        if done {
            self.set_sel(Selection::caret(Pos::new(0, 0)));
        }
        done
    }

    /// Check Before Sharing's Apply Brand: the organization's fonts and
    /// colors in place of the others, as one undo step.
    pub(crate) fn apply_brand(&mut self, b: &crate::brand::Brand) -> bool {
        self.clear_block();
        self.doc.apply_brand(&b.fonts, &b.colors, self.sel)
    }

    /// Document control: the watermark for the document's status, when
    /// it has one; otherwise a watermark from `ours` (an earlier status's)
    /// goes. Others are left alone.
    pub(crate) fn set_status_watermark(&mut self, text: Option<&str>, ours: &[&str]) {
        let mut deco = self.doc.deco.clone();
        let current = deco.watermark.as_ref().filter(|w| w.picture.is_none()).map(|w| w.text.trim().to_string());
        match text {
            Some(t) if current.as_deref() != Some(t) => {
                if current.is_some_and(|c| !c.is_empty() && !ours.contains(&c.as_str())) {
                    return;
                }
                deco.watermark = Some(apex_doc::Watermark { color: 0xC00000, ..apex_doc::Watermark::text(t) });
            }
            None if current.as_deref().is_some_and(|c| ours.contains(&c)) => deco.watermark = None,
            _ => return,
        }
        self.clear_block();
        self.doc.set_page_deco(deco, self.sel);
    }

    /// Puts Markdown (a table, say) at the selection.
    pub(crate) fn insert_markdown(&mut self, md: &str) {
        let part = apex_io::markdown::read(md);
        self.clear_block();
        let p = self.doc.insert_paragraphs(self.sel, &part.paragraphs);
        self.sel = apex_doc::Selection::caret(p);
        self.scroll_to_caret = true;
    }

    /// File > Embed Fonts in the File: turns the setting on or off (the
    /// fonts are gathered when saving).
    pub(crate) fn toggle_embed_fonts(&mut self) {
        let fonts = apex_doc::EmbeddedFonts { embed: !self.doc.fonts.embed, faces: self.doc.fonts.faces.clone() };
        self.doc.set_embedded_fonts(fonts, self.sel);
    }
}
