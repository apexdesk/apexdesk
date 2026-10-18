//! Brand control: the fonts and text colors a workbook's cells use that
//! aren't an organization's own, and putting its own in their place.

use std::collections::BTreeSet;
use std::sync::Arc;

use apex_doc::{nearest_color, neutral_color};

use crate::sheet::CellStyle;
use crate::workbook::Workbook;

fn font_ok(f: &str, fonts: &[String]) -> bool {
    fonts.is_empty() || fonts.iter().any(|b| b.trim().eq_ignore_ascii_case(f.trim()))
}

fn color_ok(c: u32, colors: &[u32]) -> bool {
    colors.is_empty() || neutral_color(c) || colors.contains(&c)
}

impl Workbook {
    /// The formats of the cells in use (the default one too).
    fn used_styles(&self) -> Vec<&CellStyle> {
        let ids: BTreeSet<u32> = self.sheets.iter().flat_map(|s| s.iter().map(|(_, c)| c.style)).chain([0]).collect();
        ids.into_iter().map(|id| self.styles.get(id)).collect()
    }

    /// The font a format shows: its own, else Normal's, else the theme's.
    fn shown_font(&self, s: &CellStyle) -> Arc<str> {
        let normal = self.cell_styles.normal().and_then(|n| n.style.font.clone());
        s.font.clone().or(normal).unwrap_or_else(|| self.cell_styles.theme.fonts.minor.clone())
    }

    /// The fonts cells show that aren't in `fonts`, and their text colors
    /// that aren't in `colors` nor neutral (none for an empty list).
    pub fn off_brand(&self, fonts: &[String], colors: &[u32]) -> (Vec<Arc<str>>, Vec<u32>) {
        let (mut f, mut c) = (BTreeSet::new(), BTreeSet::new());
        for s in self.used_styles() {
            let font = self.shown_font(s);
            if !font_ok(&font, fonts) {
                f.insert(font);
            }
            if let Some(col) = s.color.filter(|&col| !color_ok(col, colors)) {
                c.insert(col);
            }
        }
        (f.into_iter().collect(), c.into_iter().collect())
    }

    /// Puts the first of `fonts` in place of every other font and the
    /// closest of `colors` in place of every other text color, in the
    /// cells, the cell styles and the theme, as one undo step. False when
    /// nothing was off brand.
    pub fn apply_brand(&mut self, fonts: &[String], colors: &[u32]) -> bool {
        let (f, c) = self.off_brand(fonts, colors);
        if f.is_empty() && c.is_empty() {
            return false;
        }
        let brand: Option<Arc<str>> = fonts.iter().map(|f| f.trim()).find(|f| !f.is_empty()).map(Arc::from);
        let fix = |s: &CellStyle| -> Option<CellStyle> {
            let mut n = s.clone();
            if let (Some(b), Some(cur)) = (&brand, &s.font) {
                if !font_ok(cur, fonts) {
                    n.font = Some(b.clone());
                    n.theme.font = 0;
                }
            }
            if let Some(cur) = s.color.filter(|&cur| !color_ok(cur, colors)) {
                n.color = nearest_color(cur, colors);
                n.theme.color = None;
            }
            (n != *s).then_some(n)
        };
        let mut state = self.cell_styles.clone();
        if let Some(b) = &brand {
            let t = &mut state.theme.fonts;
            for font in [&mut t.major, &mut t.minor] {
                if !font_ok(font, fonts) {
                    *font = b.clone();
                }
            }
        }
        for s in &mut state.list {
            if let Some(n) = fix(&s.style) {
                s.style = n;
            }
        }
        self.restyle(state, "undo.brand", fix);
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::addr::CellRef;
    use crate::workbook::Workbook;

    #[test]
    fn brand_cells() {
        let mut wb = Workbook::new();
        wb.set_input(0, CellRef::new(0, 0), "Hola");
        let mut st = wb.styles.get(0).clone();
        st.font = Some("Comic Sans MS".into());
        st.color = Some(0xFF0000);
        let id = wb.styles.intern(st);
        wb.sheets[0].get_mut(CellRef::new(0, 0)).unwrap().style = id;
        let fonts = vec!["Inter".to_string()];
        let colors = vec![0xC00000];
        let (f, c) = wb.off_brand(&fonts, &colors);
        assert!(f.iter().any(|f| &**f == "Comic Sans MS"));
        assert_eq!(c, [0xFF0000]);
        assert!(wb.apply_brand(&fonts, &colors));
        assert_eq!(wb.off_brand(&fonts, &colors), (vec![], vec![]));
        let s = wb.styles.get(wb.sheets[0].get(CellRef::new(0, 0)).unwrap().style);
        assert_eq!((s.font.as_deref(), s.color), (Some("Inter"), Some(0xC00000)));
        assert!(wb.undo());
        assert_eq!(wb.off_brand(&fonts, &colors).1, [0xFF0000]);
    }
}
