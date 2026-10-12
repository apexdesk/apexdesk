//! Text of shapes, placeholders, table cells and notes. The paragraphs are
//! the document engine's (`apex_doc::Paragraph`: runs with `Attrs`), so
//! typing, deleting, finding and spelling work as in documents; what
//! DrawingML adds on top lives here: paragraph properties that may be
//! left unset (they are inherited from the layout, the master and the
//! theme), list styles by level, and the text frame (insets, anchoring,
//! autofit, columns).
//!
//! A paragraph points at its properties with `style.custom` (1-based
//! index in [`TextBody::props`], 0 for none), so that a paragraph split by
//! Enter keeps them, as in PowerPoint.

use std::sync::Arc;

use apex_doc::{Align, Attrs, Paragraph, Run, VAnchor};

use crate::color::SColor;

/// Distinct paragraph property entries a text body keeps.
pub const MAX_PROPS: usize = 1024;

/// A distance given as a share of the line (1.0 = single) or in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Spacing {
    Pct(f32),
    Pts(f32),
}

/// The marker of a list paragraph.
#[derive(Clone, Debug, PartialEq)]
pub enum Bullet {
    None,
    Char(char),
    /// Automatic numbering: DrawingML scheme ("arabicPeriod", "alphaLcParenR"...) and start.
    Auto(Arc<str>, u32),
}

/// Capitals.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Caps {
    None,
    Small,
    All,
}

/// Character properties that may be unset (inherited).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunProps {
    /// Points.
    pub size: Option<f32>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    pub color: Option<SColor>,
    /// Latin font; may be a theme token ("+mj-lt", "+mn-lt").
    pub font: Option<Arc<str>>,
    /// East Asian and complex script fonts.
    pub ea_font: Option<Arc<str>>,
    pub cs_font: Option<Arc<str>>,
    pub caps: Option<Caps>,
    /// Letter spacing in points.
    pub spacing: Option<f32>,
    /// Raised (positive) or lowered text, in percent of the size.
    pub baseline: Option<f32>,
}

impl RunProps {
    /// Lays `o` over these properties: what it sets wins.
    pub fn overlay(&mut self, o: &RunProps) {
        macro_rules! take {
            ($($f:ident),*) => {$(if o.$f.is_some() { self.$f = o.$f.clone(); })*};
        }
        take!(size, bold, italic, underline, strike, color, font, ea_font, cs_font, caps, spacing, baseline);
    }

    pub fn is_empty(&self) -> bool {
        *self == RunProps::default()
    }
}

/// Paragraph properties that may be unset: those of a paragraph (`a:pPr`)
/// and those of a list style level (`a:lvl1pPr`...), with the default
/// character properties of the level.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParaProps {
    pub align: Option<Align>,
    /// Left margin of the text, points.
    pub mar_l: Option<f32>,
    /// First line offset from `mar_l` (negative: hanging, where the bullet goes).
    pub indent: Option<f32>,
    pub before: Option<Spacing>,
    pub after: Option<Spacing>,
    pub line: Option<Spacing>,
    pub bullet: Option<Bullet>,
    /// `Some(None)`: the bullet takes the text's color (`buClrTx`).
    pub bullet_color: Option<Option<SColor>>,
    /// Share of the text size (1.0 = same).
    pub bullet_size: Option<f32>,
    /// `Some(None)`: the text's font (`buFontTx`).
    pub bullet_font: Option<Option<Arc<str>>>,
    pub rtl: Option<bool>,
    pub run: RunProps,
}

impl ParaProps {
    pub fn overlay(&mut self, o: &ParaProps) {
        macro_rules! take {
            ($($f:ident),*) => {$(if o.$f.is_some() { self.$f = o.$f.clone(); })*};
        }
        take!(align, mar_l, indent, before, after, line, bullet, bullet_color, bullet_size, bullet_font, rtl);
        self.run.overlay(&o.run);
    }

    pub fn is_empty(&self) -> bool {
        *self == ParaProps::default()
    }
}

/// Paragraph properties by list level (0..9), and the defaults of every
/// level (`a:defPPr`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListStyle {
    pub def: Option<ParaProps>,
    pub levels: [Option<ParaProps>; 9],
}

impl ListStyle {
    pub fn is_empty(&self) -> bool {
        self.def.is_none() && self.levels.iter().all(Option::is_none)
    }

    pub fn level(&self, l: u8) -> Option<&ParaProps> {
        self.levels.get(l as usize).and_then(Option::as_ref)
    }

    /// Lays this style's level `l` over `out` (the defaults first).
    pub fn apply(&self, l: u8, out: &mut ParaProps) {
        if let Some(d) = &self.def {
            out.overlay(d);
        }
        if let Some(p) = self.level(l) {
            out.overlay(p);
        }
    }

    pub fn level_mut(&mut self, l: u8) -> &mut ParaProps {
        self.levels[(l as usize).min(8)].get_or_insert_with(ParaProps::default)
    }
}

/// How text that doesn't fit its box is handled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Autofit {
    /// It overflows.
    None,
    /// Smaller text: the font scale and the line spacing reduction (0..1).
    Shrink { scale: f32, reduce: f32 },
    /// The shape grows to fit the text.
    Resize,
}

/// The text frame (`a:bodyPr`): unset values are inherited.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BodyProps {
    /// Insets left, top, right, bottom, points.
    pub insets: [Option<f32>; 4],
    pub anchor: Option<VAnchor>,
    pub anchor_ctr: Option<bool>,
    /// Lines wrap at the box edge (else only at paragraph ends).
    pub wrap: Option<bool>,
    pub autofit: Option<Autofit>,
    /// Vertical text ("vert", "vert270", "eaVert"...).
    pub vert: Option<Arc<str>>,
    pub columns: Option<u8>,
    /// Space between columns, points.
    pub col_gap: Option<f32>,
    /// Text turned in its box, degrees.
    pub rot: Option<f32>,
}

impl BodyProps {
    pub fn overlay(&mut self, o: &BodyProps) {
        for i in 0..4 {
            if o.insets[i].is_some() {
                self.insets[i] = o.insets[i];
            }
        }
        macro_rules! take {
            ($($f:ident),*) => {$(if o.$f.is_some() { self.$f = o.$f.clone(); })*};
        }
        take!(anchor, anchor_ctr, wrap, autofit, vert, columns, col_gap, rot);
    }

    /// Insets with PowerPoint's defaults (0.1" sides, 0.05" top and bottom).
    pub fn insets_or_default(&self) -> [f32; 4] {
        let d = [7.2, 3.6, 7.2, 3.6];
        [0, 1, 2, 3].map(|i| self.insets[i].unwrap_or(d[i]))
    }
}

/// The text of a shape.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextBody {
    pub paras: Vec<Paragraph>,
    /// Paragraph properties, pointed at by `Paragraph::style.custom`.
    pub props: Vec<ParaProps>,
    pub body: BodyProps,
    /// The shape's own list style (`a:lstStyle`).
    pub list: ListStyle,
}

impl TextBody {
    /// Text of plain paragraphs, one per line of `s`.
    pub fn plain(s: &str) -> TextBody {
        let mut t = TextBody::default();
        t.set_plain(s);
        t
    }

    pub fn is_empty(&self) -> bool {
        self.paras.iter().all(|p| p.text.trim().is_empty())
    }

    /// The text, paragraphs separated by '\n'.
    pub fn text(&self) -> String {
        self.paras.iter().map(|p| p.text.replace(apex_doc::LINE_BREAK, "\n")).collect::<Vec<_>>().join("\n")
    }

    /// Replaces the text by plain paragraphs (keeping the first
    /// paragraph's properties and level for them all).
    pub fn set_plain(&mut self, s: &str) {
        let style = self.paras.first().map(|p| p.style).unwrap_or_default();
        let attrs = self.paras.first().and_then(|p| p.runs.first()).map(|r| r.attrs.clone()).unwrap_or_default();
        self.paras = s
            .split('\n')
            .map(|l| {
                let mut p = Paragraph::new(l);
                p.style = style;
                p.runs = vec![Run { len: l.len(), attrs: attrs.clone() }];
                p
            })
            .collect();
    }

    /// The properties of paragraph `i` (empty when it has none).
    pub fn para_props(&self, i: usize) -> Option<&ParaProps> {
        let c = self.paras.get(i)?.style.custom as usize;
        if c == 0 { None } else { self.props.get(c - 1) }
    }

    /// Changes the properties of paragraph `i` (its own copy).
    pub fn edit_props(&mut self, i: usize, f: impl FnOnce(&mut ParaProps)) {
        if i >= self.paras.len() {
            return;
        }
        let mut pp = self.para_props(i).cloned().unwrap_or_default();
        f(&mut pp);
        self.set_props(i, pp);
    }

    /// Sets the properties of paragraph `i`, sharing an equal entry (or
    /// none past [`MAX_PROPS`] entries).
    pub fn set_props(&mut self, i: usize, pp: ParaProps) {
        if i >= self.paras.len() {
            return;
        }
        let at = if pp.is_empty() {
            0
        } else {
            // The latest alike first; a body keeps at most MAX_PROPS.
            match self.props.iter().rposition(|x| *x == pp) {
                Some(k) => k + 1,
                None if self.props.len() >= MAX_PROPS => 0,
                None => {
                    self.props.push(pp);
                    self.props.len()
                }
            }
        };
        self.paras[i].style.custom = at as u16;
    }

    /// Drops property entries no paragraph uses (before saving or copying).
    pub fn compact(&mut self) {
        let mut used = vec![0u16; self.props.len() + 1];
        let mut out: Vec<ParaProps> = Vec::new();
        for p in &mut self.paras {
            let c = p.style.custom as usize;
            if c == 0 || c > self.props.len() {
                p.style.custom = 0;
                continue;
            }
            if used[c] == 0 {
                out.push(self.props[c - 1].clone());
                used[c] = out.len() as u16;
            }
            p.style.custom = used[c];
        }
        self.props = out;
    }

    /// Paragraphs with their properties folded in, for pasting elsewhere.
    pub fn paragraphs(&self) -> Vec<Paragraph> {
        self.paras.clone()
    }

    /// Every run's attributes changed by `f` (a whole-shape format).
    pub fn format_all(&mut self, f: &dyn Fn(&mut Attrs)) {
        for p in &mut self.paras {
            for r in &mut p.runs {
                f(&mut r.attrs);
            }
        }
    }
}

/// DrawingML numbering schemes and how they number.
pub fn auto_number(scheme: &str, n: u32) -> String {
    let (kind, punct) = split_scheme(scheme);
    let s = match kind {
        "alphaLc" => alpha(n, false),
        "alphaUc" => alpha(n, true),
        "romanLc" => roman(n).to_lowercase(),
        "romanUc" => roman(n),
        "circleNumDbPlain" | "circleNumWdBlackPlain" | "circleNumWdWhitePlain" => char::from_u32(0x2460 + n.clamp(1, 20) - 1).map(String::from).unwrap_or_default(),
        _ => n.to_string(),
    };
    match punct {
        "ParenBoth" => format!("({s})"),
        "ParenR" => format!("{s})"),
        "Period" => format!("{s}."),
        "Minus" => format!("{s} -"),
        _ => s,
    }
}

fn split_scheme(s: &str) -> (&str, &str) {
    for p in ["ParenBoth", "ParenR", "Period", "Plain", "Minus"] {
        if let Some(k) = s.strip_suffix(p) {
            return (k, p);
        }
    }
    (s, "Plain")
}

fn alpha(n: u32, upper: bool) -> String {
    let n = n.max(1) - 1;
    // The letter repeats once per round of the alphabet: past a few
    // rounds the number is written out.
    if n / 26 >= 16 {
        return (n + 1).to_string();
    }
    let c = (b'a' + (n % 26) as u8) as char;
    let s: String = std::iter::repeat_n(c, (n / 26 + 1) as usize).collect();
    if upper { s.to_uppercase() } else { s }
}

fn roman(mut n: u32) -> String {
    if n == 0 || n > 3999 {
        return n.to_string();
    }
    let t = [(1000, "M"), (900, "CM"), (500, "D"), (400, "CD"), (100, "C"), (90, "XC"), (50, "L"), (40, "XL"), (10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I")];
    let mut s = String::new();
    for (v, r) in t {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn props_follow_split_paragraphs() {
        let mut t = TextBody::plain("one\ntwo");
        t.edit_props(0, |p| p.align = Some(Align::Center));
        assert_eq!(t.paras[0].style.custom, 1);
        assert_eq!(t.para_props(0).unwrap().align, Some(Align::Center));
        assert!(t.para_props(1).is_none());
        // Splitting in the engine copies the style: the new paragraph shares them.
        let mut doc = apex_doc::Document::from_paragraphs(t.paras.clone());
        doc.new_paragraph(apex_doc::Selection::caret(apex_doc::Pos { para: 0, offset: 2 }));
        t.paras = doc.paragraphs.clone();
        assert_eq!(t.para_props(1).unwrap().align, Some(Align::Center));
        t.edit_props(2, |p| p.align = Some(Align::Center));
        assert_eq!(t.props.len(), 1);
        t.edit_props(0, |p| p.align = None);
        t.compact();
        assert_eq!(t.props.len(), 1);
        assert_eq!(t.paras[0].style.custom, 0);
    }

    #[test]
    fn numbering() {
        assert_eq!(auto_number("arabicPeriod", 3), "3.");
        assert_eq!(auto_number("alphaLcParenR", 2), "b)");
        assert_eq!(auto_number("romanUcPeriod", 4), "IV.");
        assert_eq!(auto_number("arabicParenBoth", 12), "(12)");
    }
}
