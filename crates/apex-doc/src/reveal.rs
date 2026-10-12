//! Reveal formatting: the formatting of some text and, for each value,
//! where it comes from (the defaults, the paragraph style, a character
//! style or direct formatting), like Word's Reveal Formatting pane.

use crate::styles::CharFormat;
use crate::{Attrs, Document, ParaKind, ParaStyle};

/// Where a formatting value comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatSource {
    /// The document defaults.
    Default,
    /// The paragraph style.
    ParaStyle,
    /// The character style of the text.
    CharStyle,
    /// Formatting applied to the text or paragraph itself.
    Direct,
}

/// Sources of the character formatting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CharSources {
    pub font: FormatSource,
    pub size: FormatSource,
    pub bold: FormatSource,
    pub italic: FormatSource,
    pub underline: FormatSource,
    pub color: FormatSource,
    /// Effects: caps, spacing, scale, position, hidden...
    pub fx: FormatSource,
}

/// Sources of the paragraph formatting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParaSources {
    pub align: FormatSource,
    pub indent_left: FormatSource,
    pub indent_right: FormatSource,
    pub first_line: FormatSource,
    pub space_before: FormatSource,
    pub space_after: FormatSource,
    pub line_spacing: FormatSource,
    pub shading: FormatSource,
    pub borders: FormatSource,
    pub tabs: FormatSource,
    pub keep: FormatSource,
}

/// The formatting of the text at a position and its sources.
#[derive(Clone, Debug)]
pub struct Revealed {
    pub format: CharFormat,
    /// The text's own attributes (highlight, language, link...).
    pub attrs: Attrs,
    pub chars: CharSources,
    pub para: ParaStyle,
    pub paras: ParaSources,
    /// Section index of the paragraph.
    pub section: usize,
}

fn pick<T: PartialEq>(all: &T, cs: &T, para: &T, plain: &T) -> FormatSource {
    if all != cs {
        FormatSource::Direct
    } else if cs != para {
        FormatSource::CharStyle
    } else if para != plain {
        FormatSource::ParaStyle
    } else {
        FormatSource::Default
    }
}

impl Document {
    /// The formatting of the character at `offset` of paragraph `para`
    /// (with attributes `a`: the caller's view of that text) and where
    /// each value comes from.
    pub fn reveal(&self, para: usize, a: &Attrs) -> Revealed {
        let p = &self.paragraphs[para.min(self.paragraphs.len().saturating_sub(1))];
        let st = p.style;
        let all = self.char_format(&st, a);
        let cs = self.char_format(&st, &Attrs { char_style: a.char_style.clone(), ..Attrs::default() });
        let in_para = self.char_format(&st, &Attrs::default());
        // Text of a Normal paragraph without styles: the defaults.
        let plain = CharFormat {
            size: self.base_size(ParaKind::Normal),
            bold: false,
            italic: false,
            underline: false,
            font: self.default_font_name(),
            color: None,
            fx: Default::default(),
            asian: in_para.asian.clone(),
        };
        let chars = CharSources {
            font: pick(&all.font, &cs.font, &in_para.font, &plain.font),
            size: pick(&all.size, &cs.size, &in_para.size, &plain.size),
            bold: pick(&all.bold, &cs.bold, &in_para.bold, &plain.bold),
            italic: pick(&all.italic, &cs.italic, &in_para.italic, &plain.italic),
            underline: pick(&all.underline, &cs.underline, &in_para.underline, &plain.underline),
            color: pick(&all.color, &cs.color, &in_para.color, &plain.color),
            fx: pick(&all.fx, &cs.fx, &in_para.fx, &plain.fx),
        };
        // What the paragraph's styles alone give it.
        let mut styled = ParaStyle { kind: st.kind, custom: st.custom, level: st.level, ..ParaStyle::default() };
        let mut by_style = ParaStyle { kind: st.kind, custom: st.custom, level: st.level, ..ParaStyle::default() };
        if let Some(d) = self.styles.builtin(st.kind) {
            d.apply_para(&mut styled);
        }
        if let Some(d) = self.styles.custom(st.custom) {
            d.apply_para(&mut styled);
        }
        // A heading or other kind is itself a style.
        if st.kind != ParaKind::Normal || st.custom != 0 {
            by_style = styled;
        }
        let default = ParaStyle { kind: st.kind, custom: st.custom, level: st.level, ..ParaStyle::default() };
        let src = |same_as_style: bool, style_sets: bool| {
            if !same_as_style {
                FormatSource::Direct
            } else if style_sets {
                FormatSource::ParaStyle
            } else {
                FormatSource::Default
            }
        };
        macro_rules! field {
            ($($f:ident),+) => {
                src($(st.$f == styled.$f)&&+, $(by_style.$f != default.$f)||+)
            };
        }
        let paras = ParaSources {
            align: field!(align),
            indent_left: field!(indent_left),
            indent_right: field!(indent_right),
            first_line: field!(first_line),
            space_before: field!(space_before),
            space_after: field!(space_after),
            line_spacing: field!(line_spacing, line_rule),
            shading: field!(shading),
            borders: field!(borders),
            tabs: field!(tabs),
            keep: field!(keep_with_next, keep_lines, page_break_before, no_widow_control),
        };
        Revealed { format: all, attrs: a.clone(), chars, para: st, paras, section: self.section_index(para) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Align, Selection, StyleDef};
    use std::sync::Arc;

    #[test]
    fn sources() {
        let mut d = Document::from_plain_text("Hello world");
        d.paragraphs[0].style.kind = ParaKind::Heading(1);
        let r = d.reveal(0, &Attrs::default());
        assert!(r.format.bold);
        assert_eq!(r.chars.bold, FormatSource::ParaStyle);
        assert_eq!(r.chars.italic, FormatSource::Default);
        assert_eq!(r.paras.align, FormatSource::Default);
        let r = d.reveal(0, &Attrs { italic: true, size: Some(30), ..Attrs::default() });
        assert_eq!((r.chars.italic, r.chars.size), (FormatSource::Direct, FormatSource::Direct));
        // A character style.
        let cs = StyleDef { id: Arc::from("Emph"), name: Arc::from("Emph"), char_style: true, color: Some(0xFF0000), ..StyleDef::default() };
        d.set_style(cs, Selection::default());
        let r = d.reveal(0, &Attrs { char_style: Some(Arc::from("Emph")), ..Attrs::default() });
        assert_eq!(r.chars.color, FormatSource::CharStyle);
        assert_eq!(r.format.color, Some(0xFF0000));
        // Paragraph formatting: centered by hand.
        d.paragraphs[0].style.align = Align::Center;
        assert_eq!(d.reveal(0, &Attrs::default()).paras.align, FormatSource::Direct);
    }
}
