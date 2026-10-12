//! Named styles: changes to the built-in paragraph styles (one per
//! `ParaKind`), custom paragraph styles based on a kind, and character
//! styles. Character formatting of styles is resolved when the text is laid
//! out; paragraph formatting (alignment, spacing, indents) is copied into
//! the paragraphs when a style is applied or modified.

use std::sync::Arc;

use crate::{Align, Attrs, Document, EditKind, ParaKind, ParaStyle, Pos, Selection};

#[derive(Clone, Debug, PartialEq, Default)]
pub struct StyleDef {
    /// Identifier in files (docx styleId, odt style name).
    pub id: Arc<str>,
    /// Name shown to the user.
    pub name: Arc<str>,
    pub char_style: bool,
    /// Paragraph styles: the kind the style behaves as (headings stay
    /// headings for outlines, bookmarks and the PDF).
    pub kind: ParaKind,
    /// Changes the built-in style of `kind` instead of being a new style.
    pub builtin: bool,
    pub font: Option<Arc<str>>,
    /// Half points.
    pub size: Option<u16>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub color: Option<u32>,
    pub align: Option<Align>,
    pub space_before: Option<f32>,
    pub space_after: Option<f32>,
    /// Multiple of single spacing.
    pub line_spacing: Option<f32>,
    pub indent_left: Option<f32>,
    pub first_line: Option<f32>,
    /// Caps, spacing and other effects.
    pub fx: Option<Arc<crate::CharFx>>,
    /// Paragraph styles: the style of the paragraph Enter starts after
    /// one of this style (an id; `None`: Normal after titles and headings,
    /// the same style otherwise).
    pub next: Option<Arc<str>>,
    /// Paragraph styles that also work as character styles: applied to
    /// part of a paragraph they format only that text.
    pub linked: bool,
    pub shading: Option<u32>,
    pub borders: Option<crate::ParaBorders>,
    pub tabs: Option<crate::TabStops>,
    /// Widow/orphan control turned off (`Some(false)`: on).
    pub no_widow_control: Option<bool>,
    /// No space between paragraphs of this style (`ParaStyle::contextual_spacing`).
    pub contextual_spacing: Option<bool>,
    /// Asian font and language (see `AsianAttrs`).
    pub asian: Option<Arc<crate::AsianAttrs>>,
    /// Asian line breaking and spacing of paragraph styles.
    pub asian_para: crate::AsianPara,
}

impl StyleDef {
    /// Whether the style changes anything.
    pub fn is_empty(&self) -> bool {
        self.font.is_none()
            && self.size.is_none()
            && self.bold.is_none()
            && self.italic.is_none()
            && self.underline.is_none()
            && self.color.is_none()
            && self.align.is_none()
            && self.space_before.is_none()
            && self.space_after.is_none()
            && self.line_spacing.is_none()
            && self.indent_left.is_none()
            && self.first_line.is_none()
            && self.fx.is_none()
            && self.shading.is_none()
            && self.borders.is_none()
            && self.tabs.is_none()
            && self.no_widow_control.is_none()
            && self.contextual_spacing.is_none()
            && self.asian.is_none()
            && self.asian_para.is_default()
    }

    /// Copies the paragraph formatting of the style into `st`.
    pub fn apply_para(&self, st: &mut ParaStyle) {
        if let Some(a) = self.align {
            st.align = a;
        }
        if self.space_before.is_some() {
            st.space_before = self.space_before;
        }
        if self.space_after.is_some() {
            st.space_after = self.space_after;
        }
        if let Some(l) = self.line_spacing {
            st.line_spacing = l;
            st.line_rule = crate::LineRule::Multiple;
        }
        if let Some(i) = self.indent_left {
            st.indent_left = i;
        }
        if let Some(f) = self.first_line {
            st.first_line = f;
        }
        if self.shading.is_some() {
            st.shading = self.shading;
        }
        if let Some(b) = self.borders {
            st.borders = b;
        }
        if let Some(t) = self.tabs {
            st.tabs = t;
        }
        if let Some(w) = self.no_widow_control {
            st.no_widow_control = w;
        }
        if let Some(c) = self.contextual_spacing {
            st.contextual_spacing = c;
        }
        st.asian.overlay(&self.asian_para);
    }

    /// Lays the character formatting of the style over `f`.
    pub fn apply_char(&self, f: &mut CharFormat) {
        if let Some(s) = self.size {
            f.size = s;
        }
        if let Some(b) = self.bold {
            f.bold = b;
        }
        if let Some(i) = self.italic {
            f.italic = i;
        }
        if let Some(u) = self.underline {
            f.underline = u;
        }
        if self.font.is_some() {
            f.font.clone_from(&self.font);
        }
        if self.color.is_some() {
            f.color = self.color;
        }
        if let Some(fx) = &self.fx {
            f.fx.overlay(fx);
        }
        if let Some(a) = &self.asian {
            f.asian.overlay(a);
        }
    }
}

/// The styles of a document.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Styles {
    pub defs: Vec<StyleDef>,
    /// Colors and fonts; fonts named `+mj-lt`/`+mn-lt` are its heading and
    /// body fonts. Colors in the text are plain RGB.
    pub theme: crate::Theme,
    /// The document is on the theme fonts although it has no document
    /// font (set when a font scheme is picked).
    pub theme_fonts: bool,
    /// Multilevel list definitions (see `ParaStyle::list`).
    pub lists: Vec<crate::ListDef>,
    /// The list numbering the headings (1-based, 0: none).
    pub heading_list: u16,
    /// The document's own table styles.
    pub tables: Vec<crate::TableStyle>,
    /// Asian formatting of the document defaults.
    pub asian: crate::AsianDefaults,
    /// The rest of the document defaults of the opened file (`None`:
    /// ours).
    pub defaults: Option<crate::DocDefaults>,
}

impl Styles {
    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    /// A table style id not used yet, made from a name.
    pub fn new_table_id(&self, name: &str) -> String {
        let base: String = name.chars().filter(|c| c.is_alphanumeric()).collect();
        let base = if base.is_empty() { "TableStyle".to_string() } else { base };
        let taken = |id: &str| self.tables.iter().any(|t| t.id.eq_ignore_ascii_case(id)) || crate::table_style(id).is_some();
        if !taken(&base) {
            return base;
        }
        (2..).map(|n| format!("{base}{n}")).find(|id| !taken(id)).unwrap_or_default()
    }

    /// Hashes everything that affects layout (to key layout caches).
    pub fn hash_into(&self, h: &mut impl std::hash::Hasher) {
        use std::hash::Hash;
        (&self.theme.fonts.major, &self.theme.fonts.minor, self.theme_fonts).hash(h);
        (&self.lists, self.heading_list).hash(h);
        (&self.asian.text, self.asian.para).hash(h);
        self.defaults.as_ref().and_then(|d| d.size).hash(h);
        if !self.tables.is_empty() {
            format!("{:?}", self.tables).hash(h);
        }
        for d in &self.defs {
            (&d.id, d.char_style, d.kind, d.builtin, &d.font, d.size, d.bold, d.italic, d.underline, d.color, d.align, d.shading).hash(h);
            (&d.asian, d.asian_para, d.no_widow_control, d.contextual_spacing).hash(h);
            for v in [d.space_before, d.space_after, d.line_spacing, d.indent_left, d.first_line] {
                v.map(f32::to_bits).hash(h);
            }
        }
    }

    /// The change to the built-in style of a kind, if any.
    pub fn builtin(&self, kind: ParaKind) -> Option<&StyleDef> {
        self.defs.iter().find(|d| d.builtin && !d.char_style && d.kind == kind)
    }

    /// Line spacing (multiple) of a paragraph that doesn't set its own
    /// (`line_spacing` 0): its style's (its custom style, its kind's,
    /// Normal's), else the document defaults', else single.
    pub fn default_line(&self, st: &ParaStyle) -> f32 {
        let defaults = || match self.defaults.as_ref().and_then(|d| d.line) {
            Some((crate::LineRule::Multiple, l)) if l > 0.0 => Some(l),
            _ => None,
        };
        self.custom(st.custom).and_then(|d| d.line_spacing)
            .or_else(|| self.builtin(st.kind).and_then(|d| d.line_spacing))
            .or_else(|| self.builtin(ParaKind::Normal).and_then(|d| d.line_spacing))
            .filter(|&l| l > 0.0)
            .or_else(defaults)
            .unwrap_or(1.0)
    }

    /// The custom paragraph style of a `ParaStyle::custom` number.
    pub fn custom(&self, n: u16) -> Option<&StyleDef> {
        let d = self.defs.get((n as usize).checked_sub(1)?)?;
        (!d.char_style && !d.builtin).then_some(d)
    }

    pub fn by_id(&self, id: &str) -> Option<&StyleDef> {
        self.defs.iter().find(|d| &*d.id == id)
    }

    /// `ParaStyle::custom` number of a paragraph style id.
    /// Gives `st` a paragraph style: a built-in kind (`custom` 0) or a
    /// custom style. Resets the paragraph formatting the style defines,
    /// keeps lists' levels.
    pub fn restyle(&self, st: &mut ParaStyle, kind: ParaKind, custom: u16) {
        let kind = self.custom(custom).map_or(kind, |d| d.kind);
        let level = if st.kind.is_list() && kind.is_list() { st.level } else { 0 };
        let keep = *st;
        let list = kind.is_list();
        *st = ParaStyle { kind, level, list_format: if list { keep.list_format } else { 0 }, restart: keep.restart && list, list: if list { keep.list } else { 0 }, start_at: if list { keep.start_at } else { 0 }, custom, ..ParaStyle::default() };
        if let Some(d) = self.builtin(kind) {
            d.apply_para(st);
        }
        if let Some(d) = self.custom(custom) {
            d.apply_para(st);
        }
    }

    /// The paragraph style (kind, custom number) of a style id: a custom
    /// paragraph style or a built-in one.
    pub fn para_style_of(&self, id: &str) -> Option<(ParaKind, u16)> {
        match self.custom_number(id) {
            Some(n) => Some((self.custom(n)?.kind, n)),
            None => builtin_id(id).map(|k| (k, 0)),
        }
    }

    /// The id of a paragraph style.
    pub fn para_style_id(&self, kind: ParaKind, custom: u16) -> Arc<str> {
        match self.custom(custom) {
            Some(d) => d.id.clone(),
            None => kind_id(kind).into(),
        }
    }

    /// Whether a paragraph style is linked (see `StyleDef::linked`):
    /// titles, headings and quotes always are, as in Word.
    pub fn is_linked(&self, kind: ParaKind, custom: u16) -> bool {
        match self.custom(custom) {
            Some(d) => d.linked,
            None => matches!(kind, ParaKind::Title | ParaKind::Heading(_) | ParaKind::Quote),
        }
    }

    /// The style (kind, custom number) of the paragraph Enter starts after
    /// one with style `st`; `None` keeps the style.
    pub fn next_style(&self, st: &ParaStyle) -> Option<(ParaKind, u16)> {
        let def = self.custom(st.custom).or_else(|| if st.custom == 0 { self.builtin(st.kind) } else { None });
        let kind = def.map_or(st.kind, |d| d.kind);
        match def.and_then(|d| d.next.as_deref()) {
            Some(id) => self.para_style_of(id),
            None if matches!(kind, ParaKind::Heading(_) | ParaKind::Title) => Some((ParaKind::Normal, 0)),
            None => None,
        }
    }

    /// The look of the character part of a linked paragraph style over `f`.
    fn apply_linked(&self, id: &str, f: &mut CharFormat) -> bool {
        let Some((kind, custom)) = self.para_style_of(id) else { return false };
        f.size = self.base_size(kind);
        f.bold = kind.base_bold();
        f.italic = kind == ParaKind::Quote;
        if let Some(d) = self.builtin(kind) {
            d.apply_char(f);
        }
        if let Some(d) = self.custom(custom) {
            d.apply_char(f);
        }
        true
    }

    pub fn custom_number(&self, id: &str) -> Option<u16> {
        self.defs.iter().position(|d| &*d.id == id && !d.char_style && !d.builtin).map(|i| i as u16 + 1)
    }

    pub fn char_styles(&self) -> impl Iterator<Item = &StyleDef> {
        self.defs.iter().filter(|d| d.char_style)
    }

    pub fn custom_para_styles(&self) -> impl Iterator<Item = (u16, &StyleDef)> {
        self.defs.iter().enumerate().filter(|(_, d)| !d.char_style && !d.builtin).map(|(i, d)| (i as u16 + 1, d))
    }

    /// An id not used yet, made from a name (letters and digits only).
    pub fn new_id(&self, name: &str) -> Arc<str> {
        let base: String = name.chars().filter(|c| c.is_alphanumeric()).collect();
        let base = if base.is_empty() { "Style".to_string() } else { base };
        let taken = |id: &str| self.defs.iter().any(|d| d.id.eq_ignore_ascii_case(id)) || builtin_id(id).is_some();
        if !taken(&base) {
            return base.into();
        }
        (2..).map(|n| format!("{base}{n}")).find(|id| !taken(id)).unwrap_or_default().into()
    }
}

/// The id used in files for the built-in style of a kind.
pub fn kind_id(kind: ParaKind) -> &'static str {
    match kind {
        ParaKind::Normal => "Normal",
        ParaKind::Heading(1) => "Heading1",
        ParaKind::Heading(2) => "Heading2",
        ParaKind::Heading(3) => "Heading3",
        ParaKind::Heading(4) => "Heading4",
        ParaKind::Heading(5) => "Heading5",
        ParaKind::Heading(6) => "Heading6",
        ParaKind::Heading(7) => "Heading7",
        ParaKind::Heading(8) => "Heading8",
        ParaKind::Heading(_) => "Heading9",
        ParaKind::Title => "Title",
        ParaKind::Quote => "Quote",
        ParaKind::Code => "Code",
        ParaKind::Bullet | ParaKind::Numbered => "ListParagraph",
    }
}

/// The built-in kind of a file id.
pub fn builtin_id(id: &str) -> Option<ParaKind> {
    Some(match id {
        "Normal" => ParaKind::Normal,
        "Title" => ParaKind::Title,
        "Quote" => ParaKind::Quote,
        "Code" => ParaKind::Code,
        _ => ParaKind::Heading(id.strip_prefix("Heading")?.parse::<u8>().ok().filter(|n| (1..=9).contains(n))?),
    })
}

/// Resolved character formatting of some text.
#[derive(Clone, Debug, PartialEq)]
pub struct CharFormat {
    /// Half points.
    pub size: u16,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub font: Option<Arc<str>>,
    pub color: Option<u32>,
    pub fx: crate::CharFx,
    /// Asian font, language and hint (the rest of `AsianAttrs` is the
    /// text's own).
    pub asian: crate::AsianAttrs,
}

impl Document {
    /// Character formatting of text with attributes `a` in a paragraph with
    /// style `st`: kind defaults, then the styles, then direct formatting.
    pub fn char_format(&self, st: &ParaStyle, a: &Attrs) -> CharFormat {
        let mut f = CharFormat { size: self.base_size(st.kind), bold: st.kind.base_bold(), italic: st.kind == ParaKind::Quote, underline: false, font: None, color: None, fx: Default::default(), asian: self.styles.asian.text.as_deref().cloned().unwrap_or_default() };
        // Documents on the theme's body font take its headings font for
        // titles and headings.
        if matches!(st.kind, ParaKind::Heading(_) | ParaKind::Title) && self.default_font_token().as_deref() == Some(crate::theme::MINOR_TOKEN) {
            f.font = Some(Arc::from(crate::theme::MAJOR_TOKEN));
        }
        if !self.styles.is_empty() {
            if st.kind != ParaKind::Normal {
                if let Some(n) = self.styles.builtin(ParaKind::Normal) {
                    // Normal's font and color carry over to other kinds, not its size.
                    if n.font.is_some() {
                        f.font.clone_from(&n.font);
                    }
                    if n.color.is_some() && !matches!(st.kind, ParaKind::Heading(_) | ParaKind::Title | ParaKind::Quote) {
                        f.color = n.color;
                    }
                    if let Some(a) = &n.asian {
                        f.asian.overlay(a);
                    }
                }
            }
            if let Some(d) = self.styles.builtin(st.kind) {
                d.apply_char(&mut f);
            }
            if let Some(d) = self.styles.custom(st.custom) {
                d.apply_char(&mut f);
            }
        }
        if let Some(id) = a.char_style.as_deref() {
            match self.styles.by_id(id) {
                Some(d) if d.char_style => d.apply_char(&mut f),
                _ => {
                    self.styles.apply_linked(id, &mut f);
                }
            }
        }
        if let Some(s) = a.size {
            f.size = s;
        }
        f.bold |= a.bold;
        f.italic |= a.italic;
        f.underline |= a.underline;
        if a.font.is_some() {
            f.font.clone_from(&a.font);
        }
        if a.color.is_some() {
            f.color = a.color;
        }
        if let Some(fx) = &a.fx {
            f.fx.overlay(fx);
        }
        if let Some(x) = &a.asian {
            f.asian.overlay(x);
        }
        if let Some(real) = f.asian.font.as_deref().and_then(|n| self.styles.theme.token_font(n).or_else(|| self.styles.theme.ea_font(n, f.asian.lang.as_deref()))) {
            f.asian.font = Some(real.clone());
        }
        if let Some(real) = f.font.as_deref().and_then(|n| self.styles.theme.token_font(n)) {
            f.font = Some(real.clone());
        }
        f
    }

    /// The document font, with a theme font token resolved.
    pub fn default_font_name(&self) -> Option<Arc<str>> {
        let f = self.default_font_token()?;
        Some(self.styles.theme.token_font(&f).unwrap_or(&f).clone())
    }

    /// The document font as stored in files: a name or a theme font token.
    pub fn default_font_token(&self) -> Option<Arc<str>> {
        match &self.default_font {
            Some(f) => Some(f.clone()),
            None if self.styles.theme_fonts => Some(Arc::from(crate::theme::MINOR_TOKEN)),
            None => None,
        }
    }

    /// Changes the theme. Text and style colors that are theme colors of
    /// the old theme (base colors and their gallery tints) take the new
    /// theme's; theme fonts follow by themselves. One undo step.
    pub fn set_theme(&mut self, theme: crate::Theme, sel: Selection) {
        let before_styles = self.styles.clone();
        let before = self.paragraphs.clone();
        let old = std::mem::replace(&mut self.styles.theme, theme);
        if old.fonts != self.styles.theme.fonts {
            self.styles.theme_fonts = true;
        }
        let new = &self.styles.theme;
        let map = |c: u32| old.match_rgb(c).map_or(c, |tc| new.resolve(tc));
        for p in &mut self.paragraphs {
            if let Some(c) = p.style.shading {
                p.style.shading = Some(map(c));
            }
            for r in &mut p.runs {
                if let Some(c) = r.attrs.color {
                    r.attrs.color = Some(map(c));
                }
            }
        }
        for d in &mut self.styles.defs {
            if let Some(c) = d.color {
                d.color = Some(map(c));
            }
        }
        let n = self.paragraphs.len();
        self.record_styles(before, n, sel, before_styles);
    }

    /// Applies a paragraph style to paragraphs `from..=to`: a built-in
    /// kind (`custom` 0) or a custom style. Resets the paragraph formatting
    /// the style defines, keeps lists' levels.
    pub fn apply_para_style(&mut self, from: usize, to: usize, sel: Selection, kind: ParaKind, custom: u16) {
        let styles = self.styles.clone();
        self.set_para_style(from, to, sel, &|st| styles.restyle(st, kind, custom));
    }

    /// Applies a paragraph style to the selection: to part of a paragraph,
    /// a linked style formats only the selected text (as a character
    /// style); otherwise it styles the paragraphs `from..=to`.
    pub fn apply_style(&mut self, from: usize, to: usize, sel: Selection, kind: ParaKind, custom: u16) {
        let (a, b) = (sel.start(), sel.end());
        let partial = !sel.is_empty() && a.para == b.para && (a.offset > 0 || b.offset < self.paragraphs[a.para].text.len());
        if partial && self.styles.is_linked(kind, custom) {
            let id = self.styles.para_style_id(kind, custom);
            self.apply_char_style(sel, Some(id));
        } else {
            self.apply_para_style(from, to, sel, kind, custom);
            self.clear_linked_text(sel);
        }
    }

    /// Takes off the selected text what a linked style left on it (its
    /// character half), so the text looks like its paragraph again.
    pub fn clear_linked_text(&mut self, sel: Selection) {
        if sel.is_empty() {
            return;
        }
        let (styles, (a, b)) = (self.styles.clone(), (sel.start(), sel.end()));
        let linked = |a: &Attrs| a.char_style.as_deref().is_some_and(|id| styles.para_style_of(id).is_some());
        let left = (a.para..=b.para).any(|i| {
            let (s, e) = (if i == a.para { a.offset } else { 0 }, if i == b.para { b.offset } else { usize::MAX });
            self.paragraphs[i].spans().any(|(x, y, at)| x < e && y > s && linked(at))
        });
        if left {
            self.format_range(sel, &|at| {
                if linked(at) {
                    at.char_style = None;
                }
            });
        }
    }

    /// The paragraph style whose character half the text at `at` has (a
    /// linked style put on part of a paragraph).
    pub fn linked_text_style(&self, at: Pos) -> Option<(ParaKind, u16)> {
        let id = self.paragraphs.get(at.para)?.attrs_at(at.offset).char_style?;
        self.styles.para_style_of(&id)
    }

    /// Replaces or adds a style and updates the paragraphs using it, as one
    /// undo step.
    pub fn set_style(&mut self, def: StyleDef, sel: Selection) {
        let before_styles = self.styles.clone();
        let idx = self.styles.defs.iter().position(|d| d.id == def.id || (def.builtin && d.builtin && !d.char_style && !def.char_style && d.kind == def.kind));
        let custom = match idx {
            Some(i) => {
                self.styles.defs[i] = def.clone();
                i as u16 + 1
            }
            None => {
                self.styles.defs.push(def.clone());
                self.styles.defs.len() as u16
            }
        };
        let before = self.paragraphs.clone();
        if !def.char_style {
            for p in &mut self.paragraphs {
                let uses = if def.builtin { p.style.kind == def.kind && p.style.custom == 0 } else { p.style.custom == custom };
                if uses {
                    if !def.builtin {
                        p.style.kind = def.kind;
                    }
                    def.apply_para(&mut p.style);
                }
            }
        }
        let n = self.paragraphs.len();
        self.record_styles(before, n, sel, before_styles);
    }

    /// Removes a custom or character style; its paragraphs become Normal
    /// (their kind is kept) and its text loses it.
    pub fn delete_style(&mut self, id: &str, sel: Selection) {
        let Some(i) = self.styles.defs.iter().position(|d| &*d.id == id) else { return };
        let before_styles = self.styles.clone();
        let before = self.paragraphs.clone();
        let removed = self.styles.defs.remove(i);
        let n = i as u16 + 1;
        for p in &mut self.paragraphs {
            if !removed.char_style && !removed.builtin {
                if p.style.custom == n {
                    p.style.custom = 0;
                } else if p.style.custom > n {
                    p.style.custom -= 1;
                }
            } else if !removed.char_style {
                continue;
            } else if p.runs.iter().any(|r| r.attrs.char_style.as_deref() == Some(id)) {
                for r in &mut p.runs {
                    if r.attrs.char_style.as_deref() == Some(id) {
                        r.attrs.char_style = None;
                    }
                }
                p.normalize();
            }
        }
        let len = self.paragraphs.len();
        self.record_styles(before, len, sel, before_styles);
    }

    /// Applies (or with `None` removes) a character style on the selection.
    pub fn apply_char_style(&mut self, sel: Selection, id: Option<Arc<str>>) {
        self.format_range(sel, &|a| a.char_style.clone_from(&id));
    }

    /// Adds or replaces (by id) a table style of the document, as one undo
    /// step.
    pub fn set_table_style(&mut self, st: crate::TableStyle, sel: Selection) {
        let before_styles = self.styles.clone();
        match self.styles.tables.iter_mut().find(|t| t.id == st.id) {
            Some(t) => *t = st,
            None => self.styles.tables.push(st),
        }
        self.record_styles(Vec::new(), 0, sel, before_styles);
    }

    /// Removes a table style of the document; its tables become plain.
    pub fn delete_table_style(&mut self, id: &str, sel: Selection) {
        let before_styles = self.styles.clone();
        self.styles.tables.retain(|t| t.id != id);
        let before = self.paragraphs.clone();
        for p in &mut self.paragraphs {
            let mut cell = p.cell.as_mut();
            while let Some(c) = cell {
                if let Some(tp) = c.table_props.as_mut().filter(|tp| tp.style.as_deref() == Some(id)) {
                    tp.style = None;
                }
                cell = c.nested.as_deref_mut();
            }
        }
        let n = self.paragraphs.len();
        self.record_styles(before, n, sel, before_styles);
    }

    /// Records a change of every paragraph plus the styles.
    pub(crate) fn record_styles(&mut self, before: Vec<crate::Paragraph>, after_len: usize, sel: Selection, styles: Styles) {
        self.record(0, before, after_len, sel, sel, EditKind::Format);
        if let Some(e) = self.undo.last_mut() {
            e.styles = Some(Box::new(styles));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Paragraph;

    fn doc() -> Document {
        let h = ParaStyle { kind: ParaKind::Heading(1), ..Default::default() };
        Document::from_paragraphs(vec![Paragraph::new("Title").with_style(h), Paragraph::new("body text"), Paragraph::new("more")])
    }

    #[test]
    fn normal_takes_back_text_a_linked_style_formatted() {
        let mut d = doc();
        // "body" of "body text": part of the paragraph.
        let sel = Selection { anchor: Pos::new(1, 0), cursor: Pos::new(1, 4) };
        d.apply_style(1, 1, sel, ParaKind::Heading(2), 0);
        assert_eq!(d.paragraphs[1].style.kind, ParaKind::Normal);
        assert_eq!(d.linked_text_style(Pos::new(1, 1)), Some((ParaKind::Heading(2), 0)));
        d.apply_style(1, 1, sel, ParaKind::Normal, 0);
        assert_eq!(d.linked_text_style(Pos::new(1, 1)), None);
    }

    #[test]
    fn modify_builtin_and_undo() {
        let mut d = doc();
        let sel = Selection::caret(Pos::default());
        let f0 = d.char_format(&d.paragraphs[0].style, &Attrs::default());
        assert_eq!((f0.size, f0.bold), (40, true));
        let def = StyleDef { id: "Heading1".into(), name: "Heading 1".into(), kind: ParaKind::Heading(1), builtin: true, size: Some(32), color: Some(0x2F5496), align: Some(Align::Center), ..Default::default() };
        d.set_style(def, sel);
        let f = d.char_format(&d.paragraphs[0].style, &Attrs::default());
        assert_eq!((f.size, f.bold, f.color), (32, true, Some(0x2F5496)));
        assert_eq!(d.paragraphs[0].style.align, Align::Center);
        assert_eq!(d.paragraphs[1].style.align, Align::Left);
        d.undo();
        assert!(d.styles.is_empty());
        assert_eq!(d.paragraphs[0].style.align, Align::Left);
        d.redo();
        assert_eq!(d.styles.defs.len(), 1);
        assert_eq!(d.paragraphs[0].style.align, Align::Center);
    }

    #[test]
    fn custom_and_char_styles() {
        let mut d = doc();
        let sel = Selection::caret(Pos::default());
        let id = d.styles.new_id("My Note");
        assert_eq!(&*id, "MyNote");
        d.set_style(StyleDef { id: id.clone(), name: "My Note".into(), italic: Some(true), space_after: Some(20.0), ..Default::default() }, sel);
        let n = d.styles.custom_number(&id).unwrap();
        d.apply_para_style(1, 2, sel, ParaKind::Normal, n);
        assert_eq!(d.paragraphs[2].style.custom, n);
        assert_eq!(d.paragraphs[2].style.space_after, Some(20.0));
        assert!(d.char_format(&d.paragraphs[1].style, &Attrs::default()).italic);
        // A character style over part of the text.
        d.set_style(StyleDef { id: "Strong".into(), name: "Strong".into(), char_style: true, bold: Some(true), color: Some(0xC00000), ..Default::default() }, sel);
        d.apply_char_style(Selection { anchor: Pos::new(1, 0), cursor: Pos::new(1, 4) }, Some("Strong".into()));
        let a = d.paragraphs[1].attrs_at(2);
        assert_eq!(a.char_style.as_deref(), Some("Strong"));
        let f = d.char_format(&d.paragraphs[1].style, &a);
        assert!(f.bold && f.italic && f.color == Some(0xC00000));
        // Deleting the custom style leaves Normal paragraphs; later numbers shift.
        d.delete_style(&id, sel);
        assert_eq!(d.paragraphs[2].style.custom, 0);
        d.delete_style("Strong", sel);
        assert!(d.paragraphs[1].attrs_at(2).char_style.is_none());
        d.undo();
        assert_eq!(d.paragraphs[1].attrs_at(2).char_style.as_deref(), Some("Strong"));
    }

    #[test]
    fn theme_fonts_and_colors() {
        let mut d = doc();
        let sel = Selection::caret(Pos::default());
        // Headings follow the theme's headings font once the document uses
        // the body font.
        assert_eq!(d.char_format(&d.paragraphs[0].style, &Attrs::default()).font, None);
        d.default_font = Some(Arc::from(crate::theme::MINOR_TOKEN));
        assert_eq!(d.char_format(&d.paragraphs[0].style, &Attrs::default()).font.as_deref(), Some("Calibri Light"));
        assert_eq!(d.default_font_name().as_deref(), Some("Calibri"));
        // A blue accent 1 run and a plain color.
        d.format_range(Selection { anchor: Pos::new(1, 0), cursor: Pos::new(1, 4) }, &|a| a.color = Some(0x4472C4));
        d.format_range(Selection { anchor: Pos::new(2, 0), cursor: Pos::new(2, 4) }, &|a| a.color = Some(0x123456));
        let green = crate::Theme::builtin(7);
        d.set_theme(green.clone(), sel);
        assert_eq!(d.paragraphs[1].attrs_at(1).color, Some(green.color(4)));
        assert_eq!(d.paragraphs[2].attrs_at(1).color, Some(0x123456));
        assert_eq!(d.char_format(&d.paragraphs[0].style, &Attrs::default()).font.as_deref(), Some("Franklin Gothic Medium"));
        d.undo();
        assert_eq!(d.paragraphs[1].attrs_at(1).color, Some(0x4472C4));
        assert_eq!(d.styles.theme, crate::Theme::default());
    }

    #[test]
    fn ids() {
        assert_eq!(builtin_id("Heading3"), Some(ParaKind::Heading(3)));
        assert_eq!(builtin_id("Heading9"), Some(ParaKind::Heading(9)));
        assert_eq!(builtin_id("Heading10"), None);
        assert_eq!(kind_id(ParaKind::Heading(8)), "Heading8");
        let s = Styles::default();
        assert_eq!(&*s.new_id("Heading1"), "Heading12");
        assert_eq!(&*s.new_id("!!"), "Style");
    }

    #[test]
    fn next_linked_and_paragraph_looks() {
        let mut d = doc();
        let sel = Selection::caret(Pos::default());
        let bx = crate::ParaBorders::boxed(crate::Line::default());
        let tabs = crate::TabStops::from_slice(&[crate::TabStop { pos: 72.0, ..Default::default() }]);
        d.set_style(StyleDef { id: "Box".into(), name: "Box".into(), shading: Some(0xEEEEEE), borders: Some(bx), tabs: Some(tabs), next: Some("Heading2".into()), linked: true, italic: Some(true), ..Default::default() }, sel);
        let n = d.styles.custom_number("Box").unwrap();
        d.apply_style(1, 1, Selection::caret(Pos::new(1, 0)), ParaKind::Normal, n);
        let st = d.paragraphs[1].style;
        assert_eq!((st.shading, st.borders, st.tabs), (Some(0xEEEEEE), bx, tabs));
        // Enter at the end: the next style; in the middle: the same one.
        let end = Pos::new(1, d.paragraphs[1].text.len());
        let c = d.new_paragraph(Selection::caret(end));
        assert_eq!((d.paragraphs[c.para].style.kind, d.paragraphs[c.para].style.custom), (ParaKind::Heading(2), 0));
        let c = d.new_paragraph(Selection::caret(Pos::new(1, 4)));
        assert_eq!(d.paragraphs[c.para].style.custom, n);
        d.undo();
        // Headings go on with Normal.
        let c = d.new_paragraph(Selection::caret(Pos::new(0, 5)));
        assert_eq!(d.paragraphs[c.para].style.kind, ParaKind::Normal);
        d.undo();
        // A linked style on part of a paragraph formats only that text.
        let l = d.paragraphs.len() - 1;
        let part = Selection { anchor: Pos::new(l, 0), cursor: Pos::new(l, 2) };
        d.apply_style(l, l, part, ParaKind::Heading(1), 0);
        assert_eq!(d.paragraphs[l].style.kind, ParaKind::Normal);
        let a = d.paragraphs[l].attrs_at(1);
        assert_eq!(a.char_style.as_deref(), Some("Heading1"));
        let f = d.char_format(&d.paragraphs[l].style, &a);
        assert_eq!((f.size, f.bold), (ParaKind::Heading(1).base_size(), true));
        d.apply_style(l, l, part, ParaKind::Normal, n);
        let f = d.char_format(&d.paragraphs[l].style, &d.paragraphs[l].attrs_at(1));
        assert!(f.italic);
        // Normal isn't linked: the whole paragraph.
        d.apply_style(l, l, part, ParaKind::Quote, 0);
        d.apply_style(l, l, part, ParaKind::Normal, 0);
        assert_eq!(d.paragraphs[l].style.kind, ParaKind::Normal);
    }
}
