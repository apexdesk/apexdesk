//! Named cell styles (Excel's Cell Styles gallery) and the workbook theme.
//!
//! A cell's format is flat ([`CellStyle`]); `named` says which named style
//! it started from. Applying a style copies the parts the style includes
//! into the cells. Modifying a style updates the cells that use it, except
//! for the values the cells changed themselves (direct formatting wins).
//! Theme colors are kept as references next to the RGB, so a new theme
//! recolors the cells.

use std::sync::Arc;

use apex_doc::theme::{self, Theme, ThemeColor, slot};

use crate::addr::{CellRef, Range};
use crate::format::Locale;
use crate::sheet::{Borders, Cell, CellStyle, Line, LineStyle, THEME_MAJOR, THEME_MINOR, border};
use crate::workbook::{HashMap, Op, Workbook};

/// Parts of the format a style includes.
pub mod part {
    pub const NUMBER: u8 = 1;
    pub const ALIGN: u8 = 2;
    pub const FONT: u8 = 4;
    pub const BORDER: u8 = 8;
    pub const FILL: u8 = 16;
    pub const PROTECT: u8 = 32;
    pub const ALL: u8 = 63;
}

#[derive(Clone, Debug, PartialEq)]
pub struct NamedStyle {
    /// The English name for built-in styles (as files store it).
    pub name: Arc<str>,
    /// Excel's `builtinId` for built-in styles.
    pub builtin: Option<u16>,
    /// [`part`] flags.
    pub parts: u8,
    pub style: CellStyle,
}

impl NamedStyle {
    pub fn is_normal(&self) -> bool {
        self.builtin == Some(0)
    }
}

/// The named styles and the theme of a workbook.
#[derive(Clone, Debug, PartialEq)]
pub struct CellStyles {
    /// Normal first, then the other built-ins and the custom styles.
    pub list: Vec<NamedStyle>,
    pub theme: Theme,
}

impl CellStyles {
    pub fn new(theme: Theme, loc: &Locale) -> CellStyles {
        let list = BUILTINS.iter().filter_map(|&id| builtin_style(id, &theme, loc)).collect();
        CellStyles { list, theme }
    }

    pub fn get(&self, name: &str) -> Option<&NamedStyle> {
        self.list.iter().find(|s| s.name.eq_ignore_ascii_case(name))
    }

    pub fn normal(&self) -> Option<&NamedStyle> {
        self.list.iter().find(|s| s.is_normal())
    }

    /// Whether a built-in style differs from its default.
    pub fn is_modified(&self, s: &NamedStyle, loc: &Locale) -> bool {
        match s.builtin {
            Some(id) => builtin_style(id, &self.theme, loc).is_none_or(|d| d.parts != s.parts || d.style != s.style),
            None => true,
        }
    }

    /// A name not used yet ("Style 1", "Style 2"...).
    pub fn unique_name(&self, base: &str) -> String {
        let base = base.trim();
        if !base.is_empty() && self.get(base).is_none() {
            return base.to_string();
        }
        let base = if base.is_empty() { "Style" } else { base };
        (1..).map(|n| format!("{base} {n}")).find(|n| self.get(n).is_none()).unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// Built-in styles

/// Gallery groups, as Excel shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Custom,
    GoodBad,
    Data,
    Titles,
    Themed,
    Number,
}

/// Built-in style ids in gallery order.
pub const BUILTINS: [u16; 47] = [
    0, 27, 26, 28, // good, bad and neutral
    22, 23, 53, 20, 24, 10, 21, 11, // data and model
    16, 17, 18, 19, 15, 25, // titles and headings
    30, 34, 38, 42, 46, 50, 31, 35, 39, 43, 47, 51, 32, 36, 40, 44, 48, 52, 29, 33, 37, 41, 45, 49, // themed
    3, 6, 4, 7, 5, // number format
];

pub fn group(builtin: Option<u16>) -> Group {
    match builtin {
        None => Group::Custom,
        Some(0 | 26..=28) => Group::GoodBad,
        Some(10 | 11 | 20..=24 | 53) => Group::Data,
        Some(15..=19 | 25) => Group::Titles,
        Some(29..=52) => Group::Themed,
        Some(3..=7) => Group::Number,
        Some(_) => Group::Custom,
    }
}

/// English name of a built-in style id.
pub fn builtin_name(id: u16) -> Option<String> {
    Some(match id {
        0 => "Normal".into(),
        3 => "Comma".into(),
        4 => "Currency".into(),
        5 => "Percent".into(),
        6 => "Comma [0]".into(),
        7 => "Currency [0]".into(),
        8 => "Hyperlink".into(),
        9 => "Followed Hyperlink".into(),
        10 => "Note".into(),
        11 => "Warning Text".into(),
        15 => "Title".into(),
        16..=19 => format!("Heading {}", id - 15),
        20 => "Input".into(),
        21 => "Output".into(),
        22 => "Calculation".into(),
        23 => "Check Cell".into(),
        24 => "Linked Cell".into(),
        25 => "Total".into(),
        26 => "Good".into(),
        27 => "Bad".into(),
        28 => "Neutral".into(),
        29..=52 => {
            let n = (id - 29) / 4 + 1;
            match (id - 29) % 4 {
                0 => format!("Accent{n}"),
                k => format!("{}% - Accent{n}", k * 20),
            }
        }
        53 => "Explanatory Text".into(),
        _ => return None,
    })
}

/// The built-in id of an English style name.
pub fn builtin_id(name: &str) -> Option<u16> {
    (0..=53).find(|&id| builtin_name(id).is_some_and(|n| n.eq_ignore_ascii_case(name)))
}

/// The name and built-in id of a style read from a file. LibreOffice
/// saves Excel's built-in styles as "Excel Built-in Good" and the like.
pub fn file_style_name(name: &str, builtin: Option<u16>) -> (String, Option<u16>) {
    let name = name.trim();
    let bare = name.strip_prefix("Excel Built-in ").unwrap_or(name);
    match builtin.filter(|&id| builtin_name(id).is_some()).or_else(|| builtin_id(bare)) {
        Some(id) => (builtin_name(id).unwrap_or_default(), Some(id)),
        None => (name.to_string(), None),
    }
}

fn set_color(s: &mut CellStyle, t: &Theme, c: ThemeColor) {
    s.color = Some(t.resolve(c));
    s.theme.color = Some(c);
}

fn set_fill(s: &mut CellStyle, t: &Theme, c: ThemeColor) {
    s.fill = Some(t.resolve(c));
    s.theme.fill = Some(c);
}

fn side_index(flag: u8) -> usize {
    match flag {
        border::TOP => 0,
        border::BOTTOM => 1,
        border::LEFT => 2,
        _ => 3,
    }
}

fn set_border(s: &mut CellStyle, flags: u8, style: LineStyle, rgb: u32, tc: Option<ThemeColor>) {
    s.borders.set(flags, Line::new(style, Some(rgb)));
    for f in [border::TOP, border::BOTTOM, border::LEFT, border::RIGHT] {
        if flags & f != 0 {
            s.theme.borders[side_index(f)] = tc;
        }
    }
}

/// Excel's accounting formats for the locale: (comma, comma [0], currency,
/// currency [0]).
pub fn accounting_formats(loc: &Locale) -> [String; 4] {
    if loc.currency_after {
        let c = loc.currency;
        [
            r#"_-* #,##0.00_-;\-* #,##0.00_-;_-* "-"??_-;_-@_-"#.into(),
            r#"_-* #,##0_-;\-* #,##0_-;_-* "-"_-;_-@_-"#.into(),
            format!(r#"_-* #,##0.00 "{c}"_-;\-* #,##0.00 "{c}"_-;_-* "-"?? "{c}"_-;_-@_-"#),
            format!(r#"_-* #,##0 "{c}"_-;\-* #,##0 "{c}"_-;_-* "-" "{c}"_-;_-@_-"#),
        ]
    } else {
        let c = loc.currency;
        [
            r#"_(* #,##0.00_);_(* \(#,##0.00\);_(* "-"??_);_(@_)"#.into(),
            r#"_(* #,##0_);_(* \(#,##0\);_(* "-"_);_(@_)"#.into(),
            format!(r#"_("{c}"* #,##0.00_);_("{c}"* \(#,##0.00\);_("{c}"* "-"??_);_(@_)"#),
            format!(r#"_("{c}"* #,##0_);_("{c}"* \(#,##0\);_("{c}"* "-"_);_(@_)"#),
        ]
    }
}

/// The default definition of a built-in style.
pub fn builtin_style(id: u16, t: &Theme, loc: &Locale) -> Option<NamedStyle> {
    use part::*;
    let name = builtin_name(id)?;
    let mut s = CellStyle::default();
    let rgb = |s: &mut CellStyle, c: u32| s.color = Some(c);
    let fill = |s: &mut CellStyle, c: u32| s.fill = Some(c);
    let all = border::TOP | border::BOTTOM | border::LEFT | border::RIGHT;
    let accent1 = ThemeColor::new(slot::ACCENT1, 0);
    let parts = match id {
        0 => ALL,
        26 | 27 | 28 => {
            let (f, b) = match id {
                26 => (0x006100, 0xC6EFCE),
                27 => (0x9C0006, 0xFFC7CE),
                _ => (0x9C5700, 0xFFEB9C),
            };
            rgb(&mut s, f);
            fill(&mut s, b);
            FONT | FILL
        }
        22 => {
            s.bold = true;
            rgb(&mut s, 0xFA7D00);
            fill(&mut s, 0xF2F2F2);
            set_border(&mut s, all, LineStyle::Thin, 0x7F7F7F, None);
            FONT | FILL | BORDER
        }
        23 => {
            s.bold = true;
            set_color(&mut s, t, ThemeColor::new(slot::LT1, 0));
            fill(&mut s, 0xA5A5A5);
            set_border(&mut s, all, LineStyle::Double, 0x3F3F3F, None);
            FONT | FILL | BORDER
        }
        53 => {
            s.italic = true;
            rgb(&mut s, 0x7F7F7F);
            FONT
        }
        20 => {
            rgb(&mut s, 0x3F3F76);
            fill(&mut s, 0xFFCC99);
            set_border(&mut s, all, LineStyle::Thin, 0x7F7F7F, None);
            FONT | FILL | BORDER
        }
        24 => {
            rgb(&mut s, 0xFA7D00);
            set_border(&mut s, border::BOTTOM, LineStyle::Double, 0xFF8001, None);
            FONT | BORDER
        }
        10 => {
            fill(&mut s, 0xFFFFCC);
            set_border(&mut s, all, LineStyle::Thin, 0xB2B2B2, None);
            FILL | BORDER
        }
        21 => {
            s.bold = true;
            rgb(&mut s, 0x3F3F3F);
            fill(&mut s, 0xF2F2F2);
            set_border(&mut s, all, LineStyle::Thin, 0x3F3F3F, None);
            FONT | FILL | BORDER
        }
        11 => {
            rgb(&mut s, 0xFF0000);
            FONT
        }
        15 => {
            s.font = Some(t.fonts.major.clone());
            s.theme.font = THEME_MAJOR;
            s.font_size = 180;
            set_color(&mut s, t, ThemeColor::new(slot::DK2, 0));
            FONT
        }
        16..=19 => {
            s.bold = true;
            s.font_size = match id {
                16 => 150,
                17 => 130,
                _ => 0,
            };
            set_color(&mut s, t, ThemeColor::new(slot::DK2, 0));
            let line = match id {
                16 => Some((LineStyle::Thick, accent1)),
                17 => Some((LineStyle::Thick, ThemeColor::new(slot::ACCENT1, 5000))),
                18 => Some((LineStyle::Medium, ThemeColor::new(slot::ACCENT1, 4000))),
                _ => None,
            };
            match line {
                Some((ls, c)) => {
                    set_border(&mut s, border::BOTTOM, ls, t.resolve(c), Some(c));
                    FONT | BORDER
                }
                None => FONT,
            }
        }
        25 => {
            s.bold = true;
            set_border(&mut s, border::TOP, LineStyle::Thin, t.resolve(accent1), Some(accent1));
            set_border(&mut s, border::BOTTOM, LineStyle::Double, t.resolve(accent1), Some(accent1));
            FONT | BORDER
        }
        29..=52 => {
            let n = ((id - 29) / 4) as u8;
            let k = (id - 29) % 4;
            let tint = [0, 8000, 6000, 4000][k as usize];
            set_fill(&mut s, t, ThemeColor::new(slot::ACCENT1 + n, tint));
            if k == 0 || k == 3 {
                set_color(&mut s, t, ThemeColor::new(slot::LT1, 0));
            }
            FONT | FILL
        }
        3..=7 => {
            let f = accounting_formats(loc);
            s.num_fmt = Arc::from(match id {
                3 => f[0].as_str(),
                6 => f[1].as_str(),
                4 => f[2].as_str(),
                7 => f[3].as_str(),
                _ => "0%",
            });
            NUMBER
        }
        8 | 9 => {
            s.underline = true;
            set_color(&mut s, t, ThemeColor::new(if id == 8 { slot::HLINK } else { slot::FOL_HLINK }, 0));
            FONT
        }
        _ => return None,
    };
    Some(NamedStyle { name: Arc::from(name), builtin: Some(id), parts, style: s })
}

// ---------------------------------------------------------------------------
// Copying parts and following changes

/// Copies the parts `parts` of `src` into `dst`.
pub fn copy_parts(dst: &mut CellStyle, src: &CellStyle, parts: u8) {
    if parts & part::NUMBER != 0 {
        dst.num_fmt = src.num_fmt.clone();
    }
    if parts & part::ALIGN != 0 {
        dst.halign = src.halign;
        dst.valign = src.valign;
        dst.wrap = src.wrap;
        dst.indent = src.indent;
        dst.rotation = src.rotation;
    }
    if parts & part::FONT != 0 {
        dst.bold = src.bold;
        dst.italic = src.italic;
        dst.underline = src.underline;
        dst.strike = src.strike;
        dst.font = src.font.clone();
        dst.theme.font = src.theme.font;
        dst.font_size = src.font_size;
        dst.color = src.color;
        dst.theme.color = src.theme.color;
    }
    if parts & part::BORDER != 0 {
        dst.borders = src.borders;
        dst.theme.borders = src.theme.borders;
    }
    if parts & part::FILL != 0 {
        dst.fill = src.fill;
        dst.theme.fill = src.theme.fill;
    }
    if parts & part::PROTECT != 0 {
        dst.unlocked = src.unlocked;
        dst.formula_hidden = src.formula_hidden;
    }
}

/// Updates `dst` from a style changed from `old` to `new`: every value of
/// `parts` still equal to the old style's takes the new one; values the
/// cell set itself stay.
pub fn follow(dst: &mut CellStyle, old: &CellStyle, new: &CellStyle, parts: u8) {
    macro_rules! f {
        ($($x:ident),*) => { $(if dst.$x == old.$x { dst.$x = new.$x.clone(); })* };
    }
    if parts & part::NUMBER != 0 {
        f!(num_fmt);
    }
    if parts & part::ALIGN != 0 {
        f!(halign, valign, wrap, indent, rotation);
    }
    if parts & part::FONT != 0 {
        f!(bold, italic, underline, strike, font_size);
        if (&dst.font, dst.theme.font) == (&old.font, old.theme.font) {
            dst.font = new.font.clone();
            dst.theme.font = new.theme.font;
        }
        if (dst.color, dst.theme.color) == (old.color, old.theme.color) {
            dst.color = new.color;
            dst.theme.color = new.theme.color;
        }
    }
    if parts & part::BORDER != 0 {
        for (i, flag) in [border::TOP, border::BOTTOM, border::LEFT, border::RIGHT].into_iter().enumerate() {
            let get = |b: &Borders| match flag {
                border::TOP => b.top,
                border::BOTTOM => b.bottom,
                border::LEFT => b.left,
                _ => b.right,
            };
            if (get(&dst.borders), dst.theme.borders[i]) == (get(&old.borders), old.theme.borders[i]) {
                dst.borders.set(flag, get(&new.borders));
                dst.theme.borders[i] = new.theme.borders[i];
            }
        }
    }
    if parts & part::FILL != 0 && (dst.fill, dst.theme.fill) == (old.fill, old.theme.fill) {
        dst.fill = new.fill;
        dst.theme.fill = new.theme.fill;
    }
    if parts & part::PROTECT != 0 {
        f!(unlocked, formula_hidden);
    }
}

/// Recolors a style for a new theme. References whose RGB no longer
/// matches the old theme were overridden by a plain color and are left.
pub fn retheme(s: &mut CellStyle, old: &Theme, new: &Theme) {
    if let Some(c) = s.theme.color {
        if s.color == Some(old.resolve(c)) {
            s.color = Some(new.resolve(c));
        }
    }
    if let Some(c) = s.theme.fill {
        if s.fill == Some(old.resolve(c)) {
            s.fill = Some(new.resolve(c));
        }
    }
    for (i, flag) in [border::TOP, border::BOTTOM, border::LEFT, border::RIGHT].into_iter().enumerate() {
        let Some(c) = s.theme.borders[i] else { continue };
        let mut l = match flag {
            border::TOP => s.borders.top,
            border::BOTTOM => s.borders.bottom,
            border::LEFT => s.borders.left,
            _ => s.borders.right,
        };
        if !l.is_none() && l.color == Some(old.resolve(c)) {
            l.color = Some(new.resolve(c));
            s.borders.set(flag, l);
        }
    }
    let (from, to) = match s.theme.font {
        THEME_MAJOR => (&old.fonts.major, &new.fonts.major),
        THEME_MINOR => (&old.fonts.minor, &new.fonts.minor),
        _ => return,
    };
    if s.font.as_ref() == Some(from) {
        s.font = Some(to.clone());
    }
}

/// Points a format's colors at the theme colors (gallery entries) they
/// equal, so colors picked from the theme follow theme changes.
/// References that still hold are kept.
pub fn link_theme(s: &mut CellStyle, t: &Theme) {
    let link = |rgb: Option<u32>, r: Option<ThemeColor>| valid_ref(rgb, r, t).or_else(|| rgb.and_then(|c| t.match_rgb(c)));
    s.theme.color = link(s.color, s.theme.color);
    s.theme.fill = link(s.fill, s.theme.fill);
    let lines = [s.borders.top, s.borders.bottom, s.borders.left, s.borders.right];
    for (i, l) in lines.iter().enumerate() {
        s.theme.borders[i] = if l.is_none() { None } else { link(l.color, s.theme.borders[i]) };
    }
}

/// The theme color a style's color still stands for (None when a plain
/// color replaced it).
pub fn valid_ref(rgb: Option<u32>, c: Option<ThemeColor>, t: &Theme) -> Option<ThemeColor> {
    c.filter(|c| rgb == Some(t.resolve(*c)))
}

// ---------------------------------------------------------------------------
// Workbook operations

impl Workbook {
    /// Brings the style table in line with the named styles and the theme
    /// (after loading or an undo): recolors theme colors and puts the
    /// Normal style in id 0.
    pub(crate) fn sync_cell_styles(&mut self, old: &Theme) {
        let new = self.cell_styles.theme.clone();
        let base = self.cell_styles.normal().map(|n| CellStyle { named: None, ..n.style.clone() });
        self.styles.update_all(|id, s| {
            if id == 0 {
                if let Some(b) = &base {
                    *s = b.clone();
                }
            } else {
                retheme(s, old, &new);
            }
        });
    }

    /// Call after setting `cell_styles` directly (file readers).
    pub fn refresh_cell_styles(&mut self) {
        let t = self.cell_styles.theme.clone();
        self.sync_cell_styles(&t);
    }

    /// Applies a named style to the cells of `r`.
    pub fn apply_cell_style(&mut self, sheet: usize, r: &Range, name: &str) {
        let Some(ns) = self.cell_styles.get(name).cloned() else { return };
        let named = (!ns.is_normal()).then(|| ns.name.clone());
        self.modify_style_as(sheet, r, "undo.cell_style", move |s| {
            copy_parts(s, &ns.style, ns.parts);
            s.named = named.clone();
        });
    }

    /// Adds a style (`old` None) or replaces the style named `old` (which
    /// may rename it), updating the cells that use it. One undo step.
    pub fn set_cell_style(&mut self, old: Option<&str>, def: NamedStyle) {
        let mut state = self.cell_styles.clone();
        let prev = old.and_then(|n| state.list.iter().position(|s| s.name.eq_ignore_ascii_case(n)));
        let label = if prev.is_some() { "undo.modify_style" } else { "undo.new_style" };
        let before = prev.map(|i| state.list[i].clone());
        match prev {
            Some(i) => state.list[i] = def.clone(),
            None => state.list.push(def.clone()),
        }
        let Some(before) = before else {
            self.execute_as(Op::CellStyles(Box::new(state)), label);
            return;
        };
        let key = (!before.is_normal()).then(|| before.name.clone());
        let new_key = (!def.is_normal()).then(|| def.name.clone());
        let parts = before.parts | def.parts;
        self.restyle(state, label, |s| {
            if s.named != key {
                return None;
            }
            let mut n = s.clone();
            follow(&mut n, &before.style, &def.style, parts);
            n.named = new_key.clone();
            Some(n)
        });
    }

    /// Removes a custom style (or resets a modified built-in one); its
    /// cells go back to Normal for the values the style gave them.
    pub fn delete_cell_style(&mut self, name: &str) {
        let mut state = self.cell_styles.clone();
        let Some(i) = state.list.iter().position(|s| s.name.eq_ignore_ascii_case(name)) else { return };
        let removed = state.list[i].clone();
        if removed.is_normal() {
            return;
        }
        let normal = state.normal().map(|n| n.style.clone()).unwrap_or_default();
        match removed.builtin.and_then(|id| builtin_style(id, &state.theme, &self.locale)) {
            // Built-in styles can't go away: they get their defaults back.
            Some(d) => {
                self.set_cell_style(Some(name), d);
                return;
            }
            None => {
                state.list.remove(i);
            }
        }
        let key = Some(removed.name.clone());
        self.restyle(state, "undo.delete_style", |s| {
            if s.named != key {
                return None;
            }
            let mut n = s.clone();
            follow(&mut n, &removed.style, &normal, removed.parts);
            n.named = None;
            Some(n)
        });
    }

    /// Changes the theme; cells and styles with theme colors or fonts follow.
    pub fn set_theme(&mut self, theme: Theme) {
        let mut state = self.cell_styles.clone();
        let old = state.theme.clone();
        for s in &mut state.list {
            retheme(&mut s.style, &old, &theme);
        }
        state.theme = theme;
        self.execute_as(Op::CellStyles(Box::new(state)), "undo.theme");
    }

    /// Replaces the named styles with `state` and remaps every interned
    /// style `f` changes, as one undo step.
    pub(crate) fn restyle(&mut self, state: CellStyles, label: &'static str, f: impl Fn(&CellStyle) -> Option<CellStyle>) {
        let mut map: HashMap<u32, u32> = HashMap::default();
        let old: Vec<(u32, CellStyle)> = self.styles.iter().filter(|(id, _)| *id != 0).map(|(id, s)| (id, s.clone())).collect();
        for (id, s) in old {
            if let Some(n) = f(&s) {
                let nid = self.styles.intern(n);
                if nid != id {
                    map.insert(id, nid);
                }
            }
        }
        let mut ops = vec![Op::CellStyles(Box::new(state))];
        if !map.is_empty() {
            for (si, sh) in self.sheets.iter().enumerate() {
                let cells: Vec<(CellRef, Option<Cell>)> = sh.iter().filter_map(|(c, cell)| map.get(&cell.style).map(|&n| (c, Cell::with_style(Some(cell), n)))).collect();
                if !cells.is_empty() {
                    ops.push(Op::SetCells { sheet: si, cells });
                }
            }
        }
        self.execute_as(Op::Batch(ops), label);
    }

    /// The named styles files keep: Normal first, custom and modified
    /// built-in styles, and the built-ins cells use.
    pub fn saved_cell_styles(&self) -> Vec<&NamedStyle> {
        let used: std::collections::HashSet<&str> = self.styles.iter().filter_map(|(_, s)| s.named.as_deref()).collect();
        let cs = &self.cell_styles;
        let mut out: Vec<&NamedStyle> = cs.normal().into_iter().collect();
        out.extend(cs.list.iter().filter(|s| !s.is_normal() && (s.builtin.is_none() || used.contains(&*s.name) || cs.is_modified(s, &self.locale))));
        out
    }

    /// A new style from a cell format, including every part.
    pub fn style_from_format(&self, name: &str, st: &CellStyle) -> NamedStyle {
        NamedStyle { name: Arc::from(name), builtin: None, parts: part::ALL, style: CellStyle { named: None, ..st.clone() } }
    }
}

/// Theme font tokens for a font name of a style (for writers).
pub fn theme_font_token(s: &CellStyle, t: &Theme) -> Option<&'static str> {
    match s.theme.font {
        THEME_MAJOR if s.font.as_ref() == Some(&t.fonts.major) => Some(theme::MAJOR_TOKEN),
        THEME_MINOR if s.font.as_ref() == Some(&t.fonts.minor) => Some(theme::MINOR_TOKEN),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addr::CellRef;

    fn wb() -> Workbook {
        let mut wb = Workbook::new();
        wb.locale = Locale::EN;
        wb.cell_styles = CellStyles::new(Theme::default(), &Locale::EN);
        wb
    }

    fn style(wb: &Workbook, a1: &str) -> CellStyle {
        let c = CellRef::parse(a1).unwrap();
        wb.styles.get(wb.cell(0, c).map_or(0, |c| c.style)).clone()
    }

    #[test]
    fn builtins_and_names() {
        let s = CellStyles::new(Theme::default(), &Locale::EN);
        assert_eq!(s.list.len(), BUILTINS.len());
        for n in ["Normal", "Good", "Bad", "Neutral", "Title", "Heading 1", "Heading 4", "Total", "Note", "Warning Text", "Input", "Output", "Calculation", "Check Cell", "Explanatory Text", "Currency", "Percent", "Comma", "Accent1", "20% - Accent6", "60% - Accent3"] {
            assert!(s.get(n).is_some(), "{n}");
        }
        assert_eq!(builtin_id("40% - Accent2"), Some(35));
        assert_eq!(group(Some(35)), Group::Themed);
        // Accent fills come from the theme.
        assert_eq!(s.get("Accent1").unwrap().style.fill, Some(0x4472C4));
        assert_eq!(s.get("20% - Accent1").unwrap().style.fill, Some(0xDAE3F3));
        assert_eq!(&**s.get("Title").unwrap().style.font.as_ref().unwrap(), "Calibri Light");
        assert_eq!(s.unique_name("Good"), "Good 1");
        assert_eq!(s.unique_name("Mine"), "Mine");
    }

    #[test]
    fn apply_modify_delete_undo() {
        let mut wb = wb();
        wb.set_input(0, CellRef::parse("A1").unwrap(), "5");
        let r = Range::parse("A1:B2").unwrap();
        wb.apply_cell_style(0, &r, "Good");
        let a = style(&wb, "A1");
        assert_eq!((a.fill, a.color, a.named.as_deref()), (Some(0xC6EFCE), Some(0x006100), Some("Good")));
        // Direct formatting over the style: B1 gets its own fill.
        wb.modify_style(0, &Range::parse("B1").unwrap(), |s| s.fill = Some(0xFF0000));
        // Modify Good: new fill and bold.
        let mut g = wb.cell_styles.get("Good").unwrap().clone();
        g.style.fill = Some(0x00FF00);
        g.style.bold = true;
        wb.set_cell_style(Some("Good"), g);
        assert_eq!(style(&wb, "A1").fill, Some(0x00FF00));
        assert!(style(&wb, "A1").bold);
        assert_eq!(style(&wb, "B1").fill, Some(0xFF0000));
        assert!(style(&wb, "B1").bold);
        wb.undo();
        assert_eq!(style(&wb, "A1").fill, Some(0xC6EFCE));
        assert!(!wb.cell_styles.get("Good").unwrap().style.bold);
        wb.redo();
        assert_eq!(style(&wb, "A1").fill, Some(0x00FF00));

        // A custom style from a format, renamed, then deleted.
        let mut st = CellStyle::default();
        st.italic = true;
        st.num_fmt = Arc::from("0.00");
        let ns = wb.style_from_format("Mine", &st);
        wb.set_cell_style(None, ns);
        wb.apply_cell_style(0, &Range::parse("C1").unwrap(), "Mine");
        assert!(style(&wb, "C1").italic);
        let mut m = wb.cell_styles.get("Mine").unwrap().clone();
        m.name = Arc::from("Ours");
        wb.set_cell_style(Some("Mine"), m);
        assert_eq!(style(&wb, "C1").named.as_deref(), Some("Ours"));
        wb.delete_cell_style("Ours");
        assert!(wb.cell_styles.get("Ours").is_none());
        let c = style(&wb, "C1");
        assert!(!c.italic && c.named.is_none() && &*c.num_fmt == "General");
        wb.undo();
        assert!(style(&wb, "C1").italic);
        // Deleting a built-in resets it.
        wb.delete_cell_style("Good");
        assert_eq!(style(&wb, "A1").fill, Some(0xC6EFCE));
    }

    #[test]
    fn normal_and_theme() {
        let mut wb = wb();
        let mut n = wb.cell_styles.normal().unwrap().clone();
        n.style.font_size = 120;
        wb.set_cell_style(Some("Normal"), n);
        assert_eq!(wb.styles.get(0).font_size, 120);
        wb.undo();
        assert_eq!(wb.styles.get(0).font_size, 0);

        wb.apply_cell_style(0, &Range::parse("A1").unwrap(), "Accent1");
        wb.apply_cell_style(0, &Range::parse("A2").unwrap(), "Heading 1");
        wb.apply_cell_style(0, &Range::parse("A3").unwrap(), "Title");
        wb.modify_style(0, &Range::parse("A4").unwrap(), |s| {
            s.fill = Some(0xED7D31);
            s.theme.fill = Some(ThemeColor::new(5, 0));
        });
        // A plain color replaced the theme one: it stays put.
        wb.modify_style(0, &Range::parse("A1").unwrap(), |s| s.color = Some(0x123456));
        let green = Theme::builtin(7);
        wb.set_theme(green.clone());
        assert_eq!(style(&wb, "A1").fill, Some(green.color(4)));
        assert_eq!(style(&wb, "A1").color, Some(0x123456));
        assert_eq!(style(&wb, "A2").borders.bottom.color, Some(green.color(4)));
        assert_eq!(style(&wb, "A3").font, Some(green.fonts.major.clone()));
        assert_eq!(style(&wb, "A4").fill, Some(green.color(5)));
        assert_eq!(wb.cell_styles.get("Accent1").unwrap().style.fill, Some(green.color(4)));
        wb.undo();
        assert_eq!(style(&wb, "A1").fill, Some(0x4472C4));
        assert_eq!(style(&wb, "A3").font.as_deref(), Some("Calibri Light"));
    }
}
