//! Brand control: the fonts and colors a document uses that aren't an
//! organization's own, and putting its own in their place.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::{Document, Selection};

/// Black, white and grays: every brand allows them.
pub fn neutral_color(c: u32) -> bool {
    let (r, g, b) = ((c >> 16) & 255, (c >> 8) & 255, c & 255);
    r.abs_diff(g) <= 8 && g.abs_diff(b) <= 8 && r.abs_diff(b) <= 8
}

/// The color of `palette` closest to `c` (weighted RGB distance).
pub fn nearest_color(c: u32, palette: &[u32]) -> Option<u32> {
    let parts = |c: u32| (((c >> 16) & 255) as i64, ((c >> 8) & 255) as i64, (c & 255) as i64);
    let (r, g, b) = parts(c);
    palette.iter().copied().min_by_key(|&p| {
        let (pr, pg, pb) = parts(p);
        2 * (r - pr).pow(2) + 4 * (g - pg).pow(2) + 3 * (b - pb).pow(2)
    })
}

fn font_ok(f: &str, fonts: &[String]) -> bool {
    fonts.is_empty() || fonts.iter().any(|b| b.trim().eq_ignore_ascii_case(f.trim()))
}

fn color_ok(c: u32, colors: &[u32]) -> bool {
    colors.is_empty() || neutral_color(c) || colors.contains(&c)
}

impl Document {
    /// The fonts in use that aren't in `fonts` (none when it's empty).
    pub fn off_brand_fonts(&self, fonts: &[String]) -> Vec<Arc<str>> {
        self.fonts_used().into_iter().filter(|f| !font_ok(f, fonts)).collect()
    }

    /// The text colors in use that aren't in `colors` nor neutral (none
    /// when it's empty).
    pub fn off_brand_colors(&self, colors: &[u32]) -> Vec<u32> {
        let mut set = BTreeSet::new();
        let runs = self.paragraphs.iter().flat_map(|p| &p.runs).filter_map(|r| r.attrs.color);
        for c in runs.chain(self.styles.defs.iter().filter_map(|s| s.color)) {
            if !color_ok(c, colors) {
                set.insert(c);
            }
        }
        set.into_iter().collect()
    }

    /// Puts the first of `fonts` in place of every other font and the
    /// closest of `colors` in place of every other text color, in the text,
    /// the styles and the theme, as one undo step. False when nothing
    /// changed.
    pub fn apply_brand(&mut self, fonts: &[String], colors: &[u32], sel: Selection) -> bool {
        let brand_font: Option<Arc<str>> = fonts.iter().map(|f| f.trim()).find(|f| !f.is_empty()).map(Arc::from);
        let fix_font = |f: &mut Option<Arc<str>>| match (&brand_font, f.as_deref()) {
            (Some(b), Some(cur)) if !cur.starts_with('+') && !font_ok(cur, fonts) => {
                *f = Some(b.clone());
                true
            }
            _ => false,
        };
        let fix_color = |c: &mut Option<u32>| match *c {
            Some(cur) if !color_ok(cur, colors) => {
                *c = nearest_color(cur, colors);
                true
            }
            _ => false,
        };
        let (styles, page) = (self.styles.clone(), self.page_state());
        let before = self.paragraphs.clone();
        let mut changed = fix_font(&mut self.default_font);
        if let Some(b) = &brand_font {
            let t = &mut self.styles.theme.fonts;
            for f in [&mut t.major, &mut t.minor] {
                if !font_ok(f, fonts) {
                    *f = b.clone();
                    changed = true;
                }
            }
        }
        for s in &mut self.styles.defs {
            changed |= fix_font(&mut s.font);
            changed |= fix_color(&mut s.color);
        }
        for p in &mut self.paragraphs {
            for r in &mut p.runs {
                changed |= fix_font(&mut r.attrs.font);
                changed |= fix_color(&mut r.attrs.color);
            }
        }
        if !changed {
            return false;
        }
        let n = self.paragraphs.len();
        self.record_styles(before, n, sel, styles);
        if let Some(e) = self.undo.last_mut() {
            e.page = Some(Box::new(page));
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Pos, Selection};

    #[test]
    fn brand_fonts_and_colors() {
        let mut d = Document::from_plain_text("Hola mundo");
        let all = Selection { anchor: Pos::new(0, 0), cursor: Pos::new(0, 4) };
        d.toggle(all, |a| a.bold, |a, v| a.bold = v);
        for r in &mut d.paragraphs[0].runs {
            r.attrs.font = Some("Comic Sans MS".into());
            r.attrs.color = Some(0xFF0000);
        }
        d.default_font = Some("Times New Roman".into());
        let fonts = vec!["Inter".to_string()];
        let colors = vec![0x1F4E79, 0xC00000];
        assert!(d.off_brand_fonts(&fonts).iter().any(|f| &**f == "Comic Sans MS"));
        assert_eq!(d.off_brand_colors(&colors), [0xFF0000]);
        // Neutral colors and brand ones pass.
        assert!(d.off_brand_colors(&[0xFF0000]).is_empty());
        let caret = Selection::caret(Pos::new(0, 0));
        assert!(d.apply_brand(&fonts, &colors, caret));
        assert!(d.off_brand_fonts(&fonts).is_empty(), "{:?}", d.off_brand_fonts(&fonts));
        assert!(d.off_brand_colors(&colors).is_empty());
        assert_eq!(d.paragraphs[0].runs[0].attrs.color, Some(0xC00000));
        assert!(!d.apply_brand(&fonts, &colors, caret));
        d.undo();
        assert_eq!(d.default_font.as_deref(), Some("Times New Roman"));
        assert_eq!(d.paragraphs[0].runs[0].attrs.color, Some(0xFF0000));
    }

    #[test]
    fn nearest() {
        assert!(neutral_color(0x000000) && neutral_color(0xFFFFFF) && neutral_color(0x808284));
        assert!(!neutral_color(0x1F4E79));
        assert_eq!(nearest_color(0x2050A0, &[0xC00000, 0x1F4E79]), Some(0x1F4E79));
        assert_eq!(nearest_color(0x2050A0, &[]), None);
    }
}
