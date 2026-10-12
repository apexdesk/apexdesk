//! ApexDesk text document model: paragraphs of styled runs, editing and undo.

use std::sync::Arc;

pub mod math;
pub mod merge;
pub mod assemble;
pub mod theme;
pub use theme::{Theme, ThemeColor};
mod styles;
pub use styles::*;
mod lists;
pub use lists::*;
mod captions;
pub use captions::*;
mod find;
mod protection;
pub use protection::*;
mod formula;
pub use formula::{cell_number, format_number};
pub use find::*;
mod fields;
mod notes;
mod toc;
mod biblio;
pub use biblio::*;
pub mod macros;
pub use macros::Macros;
pub use fields::*;
pub use notes::*;
mod comments;
mod compare;
mod code_merge;
pub use code_merge::merge_code;
mod revisions;
mod table;
pub use comments::*;
pub use compare::{Op as DiffOp, compare, diff};
pub use revisions::*;
mod para_deco;
mod section;
mod headers;
pub use headers::*;
mod shape;
pub mod gradient;
pub use gradient::{GradKind, GradStop, Gradient};
pub mod text_fx;
pub use text_fx::{TextFill, TextFx};
pub mod preset;
pub use preset::{PresetFill, PresetPath, is_preset, preset_adjusts, preset_names, preset_paths, preset_sites, preset_text_rect};
pub mod ink;
pub use ink::{Ink, InkSource, InkStroke, InkTool};
pub use para_deco::*;
pub use shape::*;
pub use section::*;
pub use table::*;
pub use toc::*;
mod hyphen;
pub use hyphen::*;
mod asian;
mod asian_edit;
mod defaults;
mod transplant;
pub use transplant::Origin;
pub use defaults::*;
mod tategaki;
pub use tategaki::upright_fits;
pub use asian::*;
mod controls;
mod form_fields;
pub use form_fields::*;
mod pic_fx;
pub use pic_fx::*;
mod text_links;
mod shape_group;
mod undo_group;
pub use undo_group::UndoMark;
mod embed;
pub use embed::*;
mod inspect;
pub mod reveal;
pub mod access;
pub use inspect::{FileParts, Inspect, Inspection, cleared_meta, is_file_link};
pub use shape_group::fresh_group_id;
pub use text_links::*;
pub use controls::*;
pub mod kept;
pub mod book;
pub mod book_review;
pub mod book_publish;
pub mod notebook;
mod fonts;
mod brand;
pub use brand::{nearest_color, neutral_color};
pub use fonts::{EmbeddedFont, EmbeddedFonts};
pub use kept::{KeptLink, KeptPart, KeptParts, SensitivityLabel};

/// Character formatting.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct Attrs {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub code: bool,
    pub superscript: bool,
    pub subscript: bool,
    /// Font size in half points (24 = 12pt); `None` uses the paragraph default.
    pub size: Option<u16>,
    pub color: Option<u32>,
    pub highlight: Option<u32>,
    pub font: Option<Arc<str>>,
    /// Hyperlink target: a URL, `mailto:` address or `#bookmark`.
    pub link: Option<Arc<str>>,
    /// Hyperlink screen tip.
    pub link_tip: Option<Arc<str>>,
    /// A picture: set on an `OBJECT` character.
    pub image: Option<Arc<Image>>,
    /// Character style id (see `Styles`).
    pub char_style: Option<Arc<str>>,
    /// A tracked insertion or deletion.
    pub rev: Option<Arc<Revision>>,
    /// A footnote or endnote whose mark is this `OBJECT` character.
    pub note: Option<Arc<Note>>,
    /// A field shown in place of this `OBJECT` character.
    pub field: Option<Arc<Field>>,
    /// An equation: set on an `OBJECT` character.
    pub math: Option<Arc<math::Equation>>,
    /// A shape anchored at this `OBJECT` character.
    pub shape: Option<Arc<DocShape>>,
    /// Less common effects (caps, spacing, underline style...).
    pub fx: Option<Arc<CharFx>>,
    /// The content control this text is in.
    pub control: Option<Arc<ContentControl>>,
    /// Language of the text ("en-US"); `None`: the document's.
    pub lang: Option<Arc<str>>,
    /// Not checked for spelling.
    pub no_proof: bool,
    /// Asian and complex script fonts, phonetic guides and the like.
    pub asian: Option<Arc<AsianAttrs>>,
    /// Artistic effects: fill, outline, shadow, glow, reflection, bevel.
    pub text_fx: Option<Arc<TextFx>>,
}

/// How an underline is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum UnderlineStyle {
    #[default]
    Single,
    /// Under words only (not the spaces between them).
    Words,
    Double,
    Thick,
    Dotted,
    Dashed,
    DotDash,
    Wave,
}

impl UnderlineStyle {
    pub const ALL: [UnderlineStyle; 8] = [UnderlineStyle::Single, UnderlineStyle::Words, UnderlineStyle::Double, UnderlineStyle::Thick, UnderlineStyle::Dotted, UnderlineStyle::Dashed, UnderlineStyle::DotDash, UnderlineStyle::Wave];
}

/// Character effects beyond bold, italic and the like (Word's Font dialog).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct CharFx {
    pub small_caps: bool,
    pub all_caps: bool,
    pub double_strike: bool,
    /// Hidden text: shown only with the formatting marks, never printed.
    pub hidden: bool,
    /// Extra space between letters in twentieths of a point (negative
    /// condenses).
    pub spacing: i16,
    /// Width in percent (0 means 100).
    pub scale: u16,
    /// Raised (or, negative, lowered) text, in half points.
    pub position: i16,
    pub underline: UnderlineStyle,
    pub underline_color: Option<u32>,
}

impl CharFx {
    pub fn is_default(&self) -> bool {
        *self == CharFx::default() || *self == CharFx { scale: 100, ..Default::default() }
    }

    /// Lays `o` over these effects (what it sets wins).
    pub fn overlay(&mut self, o: &CharFx) {
        self.small_caps |= o.small_caps;
        self.all_caps |= o.all_caps;
        self.double_strike |= o.double_strike;
        self.hidden |= o.hidden;
        if o.spacing != 0 {
            self.spacing = o.spacing;
        }
        if o.scale != 0 && o.scale != 100 {
            self.scale = o.scale;
        }
        if o.position != 0 {
            self.position = o.position;
        }
        if o.underline != UnderlineStyle::Single {
            self.underline = o.underline;
        }
        if o.underline_color.is_some() {
            self.underline_color = o.underline_color;
        }
    }

    /// Width scale as a factor.
    pub fn scale(&self) -> f32 {
        if self.scale == 0 { 1.0 } else { self.scale as f32 / 100.0 }
    }
}

impl Attrs {
    /// The effects (defaults when none are set).
    pub fn fx(&self) -> CharFx {
        self.fx.as_deref().cloned().unwrap_or_default()
    }

    /// Changes the effects; defaults are stored as none.
    pub fn set_fx(&mut self, f: impl FnOnce(&mut CharFx)) {
        let mut fx = self.fx();
        f(&mut fx);
        self.fx = (!fx.is_default()).then(|| Arc::new(fx));
    }

    /// Drops what belongs to one character only (pictures, notes, fields, equations),
    /// for text typed next to it.
    pub fn clear_object(&mut self) {
        (self.image, self.note, self.field, self.rev, self.math, self.shape) = (None, None, None, None, None, None);
    }
}

/// The character that holds a picture in the text.
pub const OBJECT: char = '\u{FFFC}';

/// How text flows around a picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Wrap {
    /// In the line, like a big character.
    #[default]
    Inline,
    /// Text flows beside it (approximated inside its paragraph).
    Square,
    /// On a line of its own.
    TopBottom,
}

/// A picture in a document.
#[derive(Clone, Debug, Default)]
pub struct Image {
    /// The encoded file (PNG, JPEG...).
    pub data: Arc<[u8]>,
    /// Shown size in points (after cropping).
    pub width: f32,
    pub height: f32,
    /// Cropped fractions of the source: left, top, right, bottom.
    pub crop: [f32; 4],
    /// Clockwise rotation in degrees.
    pub rotation: f32,
    pub wrap: Wrap,
    /// For `Square` and `TopBottom`: offset from the left of the text area
    /// in points, unless aligned.
    pub offset_x: f32,
    pub align: Option<Align>,
    pub name: String,
    pub alt: String,
    /// Hidden (Selection Pane): not shown nor printed, kept in files.
    pub hidden: bool,
    /// Corrections, recoloring and effects.
    pub fx: Option<Arc<PicFx>>,
    /// An embedded object or a diagram shown by this picture.
    pub object: Option<Arc<Embedded>>,
}

impl Image {
    pub fn new(data: Arc<[u8]>, width: f32, height: f32) -> Self {
        Image { data, width, height, ..Default::default() }
    }

    /// A short stable hash of the picture bytes (to key caches).
    pub fn data_key(&self) -> u64 {
        data_key(&self.data)
    }
}

/// Hash of picture bytes: length plus a sample, cheap for big files.
pub fn data_key(data: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    data.len().hash(&mut h);
    if data.len() <= 8192 {
        data.hash(&mut h);
    } else {
        for i in 0..64 {
            let at = i * (data.len() - 64) / 63;
            data[at..at + 64].hash(&mut h);
        }
    }
    h.finish()
}

impl PartialEq for Image {
    fn eq(&self, o: &Self) -> bool {
        (Arc::ptr_eq(&self.data, &o.data) || self.data == o.data)
            && self.width.to_bits() == o.width.to_bits()
            && self.height.to_bits() == o.height.to_bits()
            && self.crop.map(f32::to_bits) == o.crop.map(f32::to_bits)
            && self.rotation.to_bits() == o.rotation.to_bits()
            && self.wrap == o.wrap
            && self.offset_x.to_bits() == o.offset_x.to_bits()
            && self.align == o.align
            && self.name == o.name
            && self.alt == o.alt
            && self.fx == o.fx
            && self.object == o.object
    }
}

impl Eq for Image {}

impl std::hash::Hash for Image {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.data_key().hash(h);
        for v in [self.width, self.height, self.rotation, self.offset_x].into_iter().chain(self.crop) {
            v.to_bits().hash(h);
        }
        self.wrap.hash(h);
        self.align.hash(h);
        (&self.name, self.hidden).hash(h);
        self.alt.hash(h);
        self.fx.hash(h);
        self.object.hash(h);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    /// Length in bytes of `Paragraph::text`.
    pub len: usize,
    pub attrs: Attrs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum ParaKind {
    #[default]
    Normal,
    /// Heading level 1..=9.
    Heading(u8),
    Title,
    Quote,
    Code,
    Bullet,
    Numbered,
}

impl ParaKind {
    pub fn is_list(self) -> bool {
        matches!(self, ParaKind::Bullet | ParaKind::Numbered)
    }

    /// Default font size (half points) and bold for the kind.
    pub fn base_size(self) -> u16 {
        match self {
            ParaKind::Title => 52,
            ParaKind::Heading(1) => 40,
            ParaKind::Heading(2) => 32,
            ParaKind::Heading(3) => 28,
            ParaKind::Heading(4) => 26,
            ParaKind::Heading(_) => 24,
            ParaKind::Code => 20,
            _ => 24,
        }
    }

    pub fn base_bold(self) -> bool {
        matches!(self, ParaKind::Title | ParaKind::Heading(_))
    }
}

/// How `ParaStyle::line_spacing` is interpreted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum LineRule {
    /// Multiplier of the single line height.
    #[default]
    Multiple,
    /// Exact line height in points.
    Exact,
    /// Minimum line height in points.
    AtLeast,
}

/// Soft line break inside a paragraph (Shift+Enter).
pub const LINE_BREAK: char = '\u{2028}';

/// Bullet glyphs by list format (1..; 0 cycles • ◦ ▪ by level).
pub const BULLETS: [char; 7] = ['•', '•', '◦', '■', '–', '✓', '➢'];
/// Numbering formats by list format (1..; 0 cycles 1. a) i. by level).
pub const NUM_FORMATS: [&str; 7] = ["", "1.", "1)", "a)", "A.", "i.", "I."];

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct ParaStyle {
    pub kind: ParaKind,
    pub align: Align,
    /// List / indentation level (0-based).
    pub level: u8,
    /// Line spacing: a multiplier (1.0 = single) or points depending on
    /// `line_rule`. 0 means default.
    pub line_spacing: f32,
    pub line_rule: LineRule,
    pub page_break_before: bool,
    pub keep_with_next: bool,
    /// Keep lines together: the paragraph isn't split across pages.
    pub keep_lines: bool,
    /// Widow/orphan control turned off (on by default, as in Word).
    pub no_widow_control: bool,
    /// Left out of line numbering.
    pub no_line_numbers: bool,
    /// No space between this paragraph and a neighbour of the same style
    /// (Word's "don't add space between paragraphs of the same style").
    pub contextual_spacing: bool,
    /// Space before/after in points; `None` uses the kind's default.
    pub space_before: Option<f32>,
    pub space_after: Option<f32>,
    /// Extra indentation in points, on top of the level indentation.
    pub indent_left: f32,
    pub indent_right: f32,
    /// First line offset in points (negative = hanging).
    pub first_line: f32,
    /// Paragraph background color.
    pub shading: Option<u32>,
    /// Bullet glyph / numbering style (see `BULLETS`, `NUM_FORMATS`).
    pub list_format: u8,
    /// Numbered list restarts at 1 on this paragraph.
    pub restart: bool,
    /// Multilevel list: 1-based index in `Styles::lists`, 0 for the
    /// simple list of `list_format`.
    pub list: u16,
    /// Set numbering value: this item's number (0: continues).
    pub start_at: u16,
    /// Custom paragraph style: 1-based index in `Document::styles`, 0 for
    /// the built-in style of `kind`.
    pub custom: u16,
    /// Part of a generated table of contents.
    pub toc: Option<TocMark>,
    /// Starts in the next text column.
    pub column_break_before: bool,
    pub borders: ParaBorders,
    pub tabs: TabStops,
    pub drop_cap: Option<DropCap>,
    /// Asian line breaking and spacing.
    pub asian: AsianPara,
}

impl ParaStyle {
    /// Default space before/after in points for the kind.
    pub fn default_spacing(&self) -> (f32, f32) {
        match self.kind {
            ParaKind::Title => (0.0, 12.0),
            ParaKind::Heading(_) => (12.0, 4.0),
            ParaKind::Bullet | ParaKind::Numbered => (0.0, 2.0),
            ParaKind::Code => (0.0, 0.0),
            _ => (0.0, 8.0),
        }
    }

    pub fn spacing(&self) -> (f32, f32) {
        let (b, a) = self.default_spacing();
        (self.space_before.unwrap_or(b), self.space_after.unwrap_or(a))
    }
}

/// Appends paragraph text, turning soft line breaks into '\n'.
fn push_plain(out: &mut String, t: &str) {
    if t.contains(LINE_BREAK) {
        out.extend(t.chars().map(|c| if c == LINE_BREAK { '\n' } else { c }));
    } else {
        out.push_str(t);
    }
}

fn roman(mut n: usize) -> String {
    // Roman numerals end at 3999; past it a number is written out.
    if n > 3999 {
        return n.to_string();
    }
    const T: [(usize, &str); 13] = [(1000, "m"), (900, "cm"), (500, "d"), (400, "cd"), (100, "c"), (90, "xc"), (50, "l"), (40, "xl"), (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i")];
    let mut s = String::new();
    for (v, r) in T {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

fn letters(mut n: usize) -> String {
    let mut s = Vec::new();
    while n > 0 {
        n -= 1;
        s.push(b'a' + (n % 26) as u8);
        n /= 26;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

/// Bullet glyph for a list format and level.
pub fn bullet_char(format: u8, level: u8) -> char {
    match format {
        0 => ['•', '◦', '▪'][level as usize % 3],
        f => BULLETS[(f as usize).min(BULLETS.len() - 1)],
    }
}

/// Effective numbering format (1..=6, see `NUM_FORMATS`).
pub fn num_format(format: u8, level: u8) -> u8 {
    match format {
        0 => [1, 3, 5][level as usize % 3],
        f => f.min(NUM_FORMATS.len() as u8 - 1),
    }
}

/// Label of the `n`th item (1-based) of a numbered list.
pub fn number_label(format: u8, level: u8, n: usize) -> String {
    match num_format(format, level) {
        2 => format!("{n})"),
        3 => format!("{})", letters(n)),
        4 => format!("{}.", letters(n).to_uppercase()),
        5 => format!("{}.", roman(n)),
        6 => format!("{}.", roman(n).to_uppercase()),
        _ => format!("{n}."),
    }
}

/// One end of a bookmark, anchored in a paragraph so that it follows edits
/// (and undo, which restores whole paragraphs). A bookmark is a start and an
/// end mark with the same name, both at the same place for a point bookmark.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mark {
    pub name: Arc<str>,
    /// Byte offset in `Paragraph::text`.
    pub offset: usize,
    pub end: bool,
}

/// Words in a text, as Word counts them: runs of characters between
/// spaces, except that each Chinese or Japanese character (written
/// without spaces) is a word of its own.
pub fn count_words(text: &str) -> usize {
    let ideo = |c: char| matches!(c as u32, 0x2E80..=0x2FDF | 0x3040..=0x30FF | 0x3190..=0x31FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0xFF66..=0xFF9F | 0x20000..=0x3FFFF);
    let mut n = 0;
    let mut in_word = false;
    for c in text.chars() {
        if ideo(c) {
            n += 1;
            in_word = false;
        } else if c.is_whitespace() || matches!(c, '\u{3000}'..='\u{303F}' | '\u{FF01}'..='\u{FF0F}') {
            in_word = false;
        } else if !in_word {
            n += 1;
            in_word = true;
        }
    }
    n
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Paragraph {
    pub text: String,
    /// Runs covering `text` exactly. Empty text keeps one zero-length run
    /// so typing at the start inherits its attributes.
    pub runs: Vec<Run>,
    pub style: ParaStyle,
    /// Bookmark boundaries in this paragraph.
    pub marks: Vec<Mark>,
    /// The table cell holding the paragraph, if any.
    pub cell: Option<InCell>,
    /// The section this paragraph starts (see `section.rs`).
    pub section: Option<Box<Section>>,
}

impl Paragraph {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let runs = vec![Run { len: text.len(), attrs: Attrs::default() }];
        Paragraph { text, runs, style: ParaStyle::default(), marks: Vec::new(), cell: None, section: None }
    }

    pub fn with_style(mut self, style: ParaStyle) -> Self {
        self.style = style;
        self
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Appends styled text.
    pub fn push(&mut self, text: &str, attrs: Attrs) {
        if text.is_empty() {
            return;
        }
        self.text.push_str(text);
        match self.runs.last_mut() {
            Some(r) if r.attrs == attrs || r.len == 0 => {
                r.len += text.len();
                r.attrs = attrs;
            }
            _ => self.runs.push(Run { len: text.len(), attrs }),
        }
    }

    /// Iterates `(start, end, attrs)` byte spans.
    pub fn spans(&self) -> impl Iterator<Item = (usize, usize, &Attrs)> {
        let mut pos = 0;
        self.runs.iter().map(move |r| {
            let s = pos;
            pos += r.len;
            (s, pos, &r.attrs)
        })
    }

    /// Attributes in effect for text typed at `offset` (inherits from the left).
    pub fn attrs_at(&self, offset: usize) -> Attrs {
        let mut pos = 0;
        let mut last = None;
        for r in &self.runs {
            if offset > pos && offset <= pos + r.len {
                return r.attrs.clone();
            }
            if last.is_none() {
                last = Some(&r.attrs);
            }
            pos += r.len;
        }
        last.cloned().unwrap_or_default()
    }

    fn normalize(&mut self) {
        let mut out: Vec<Run> = Vec::with_capacity(self.runs.len());
        // An empty paragraph keeps the look of its first empty run (its
        // mark's).
        let mut mark = None;
        for r in self.runs.drain(..) {
            if r.len == 0 {
                mark.get_or_insert(r.attrs);
                continue;
            }
            match out.last_mut() {
                Some(l) if l.attrs == r.attrs => l.len += r.len,
                _ => out.push(r),
            }
        }
        if out.is_empty() {
            out.push(Run { len: 0, attrs: mark.unwrap_or_default() });
        }
        self.runs = out;
    }

    /// Splits the run list so a boundary exists at `offset`.
    fn split_run_at(&mut self, offset: usize) -> usize {
        let mut pos = 0;
        for i in 0..self.runs.len() {
            if pos == offset {
                return i;
            }
            let len = self.runs[i].len;
            if offset < pos + len {
                let attrs = self.runs[i].attrs.clone();
                self.runs[i].len = offset - pos;
                self.runs.insert(i + 1, Run { len: pos + len - offset, attrs });
                return i + 1;
            }
            pos += len;
        }
        self.runs.len()
    }

    fn insert_str(&mut self, offset: usize, s: &str, attrs: Attrs) {
        if s.is_empty() {
            return;
        }
        let offset = self.text.floor_char_boundary(offset);
        let i = self.split_run_at(offset);
        self.text.insert_str(offset, s);
        self.runs.insert(i, Run { len: s.len(), attrs });
        self.normalize();
        // Marks at the insertion point stay before the new text, except
        // an editable range's end: typing at it goes on inside.
        for m in &mut self.marks {
            if m.offset > offset || m.offset == offset && m.end && is_perm_mark(&m.name) {
                m.offset += s.len();
            }
        }
    }

    fn remove(&mut self, start: usize, end: usize) {
        if start >= end {
            return;
        }
        let a = self.split_run_at(start);
        let b = self.split_run_at(end);
        let keep_attrs = self.runs.get(a).map(|r| r.attrs.clone());
        self.runs.drain(a..b);
        self.text.replace_range(start..end, "");
        for m in &mut self.marks {
            if m.offset > end {
                m.offset -= end - start;
            } else if m.offset > start {
                m.offset = start;
            }
        }
        if self.runs.iter().all(|r| r.len == 0) {
            self.runs = vec![Run { len: 0, attrs: keep_attrs.unwrap_or_default() }];
        } else {
            self.normalize();
        }
    }

    /// Splits off the tail at `offset` into a new paragraph.
    fn split_off(&mut self, offset: usize) -> Paragraph {
        let mut tail_attrs = self.attrs_at(offset);
        tail_attrs.clear_object();
        let i = self.split_run_at(offset);
        let runs: Vec<Run> = self.runs.drain(i..).collect();
        let text = self.text.split_off(offset);
        let mut style = self.style;
        style.page_break_before = false;
        style.column_break_before = false;
        style.drop_cap = None;
        style.restart = false;
        if matches!(self.style.kind, ParaKind::Heading(_) | ParaKind::Title) && text.is_empty() {
            style.kind = ParaKind::Normal;
        }
        let mut marks = Vec::new();
        self.marks.retain(|m| {
            if m.offset > offset || m.offset == offset && m.end && is_perm_mark(&m.name) {
                marks.push(Mark { offset: m.offset - offset, ..m.clone() });
                false
            } else {
                true
            }
        });
        let mut p = Paragraph { text, runs, style, marks, cell: self.cell.as_ref().map(InCell::bare), section: None };
        if p.text.is_empty() {
            p.runs = vec![Run { len: 0, attrs: tail_attrs.clone() }];
        }
        if self.runs.is_empty() {
            self.runs.push(Run { len: 0, attrs: tail_attrs });
        }
        self.normalize();
        p.normalize();
        p
    }

    fn append(&mut self, other: Paragraph) {
        let shift = self.text.len();
        self.marks.extend(other.marks.into_iter().map(|m| Mark { offset: m.offset + shift, ..m }));
        self.text.push_str(&other.text);
        self.runs.extend(other.runs);
        self.normalize();
    }

    pub fn map_attrs(&mut self, start: usize, end: usize, f: &dyn Fn(&mut Attrs)) {
        if start >= end {
            return;
        }
        let a = self.split_run_at(start);
        let b = self.split_run_at(end);
        for r in &mut self.runs[a..b] {
            f(&mut r.attrs);
        }
        self.normalize();
    }
}

/// A position: paragraph index and byte offset (always on a char boundary).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub struct Pos {
    pub para: usize,
    pub offset: usize,
}

impl Pos {
    pub const fn new(para: usize, offset: usize) -> Self {
        Pos { para, offset }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Selection {
    pub anchor: Pos,
    pub cursor: Pos,
}

impl Selection {
    pub fn caret(p: Pos) -> Self {
        Selection { anchor: p, cursor: p }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.cursor
    }

    pub fn start(&self) -> Pos {
        self.anchor.min(self.cursor)
    }

    pub fn end(&self) -> Pos {
        self.anchor.max(self.cursor)
    }
}

/// Page setup in points (1/72 inch).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageSetup {
    pub width: f32,
    pub height: f32,
    pub margin_top: f32,
    pub margin_bottom: f32,
    pub margin_left: f32,
    pub margin_right: f32,
    /// Distance from the top edge to the header, and from the bottom edge
    /// to the footer.
    pub margin_header: f32,
    pub margin_footer: f32,
    /// Line numbers in the margin.
    pub line_numbers: Option<LineNumbering>,
    /// Extra margin for binding: on the left (inside with mirrored
    /// margins) or at the top.
    pub gutter: f32,
    pub gutter_top: bool,
    /// Mirrored margins: left and right are inside and outside, swapped on
    /// even pages.
    pub mirror: bool,
    /// Where the text sits between the top and bottom margins.
    pub v_align: PageVAlign,
    /// The document grid (lines and Asian characters per line).
    pub grid: DocGrid,
    /// Direction of the text: vertical pages (tategaki) and the rest kept
    /// for files.
    pub text_flow: Option<TextFlow>,
}

/// Vertical alignment of the text on a page (Word's `w:vAlign`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PageVAlign {
    #[default]
    Top,
    Center,
    /// Paragraphs spread over the page.
    Justify,
    Bottom,
}

/// When line numbers start again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LineRestart {
    #[default]
    EachPage,
    EachSection,
    Continuous,
}

/// Line numbers of a section (Word's `w:lnNumType`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineNumbering {
    /// Number shown on every n-th line.
    pub count_by: u16,
    /// First number.
    pub start: u16,
    pub restart: LineRestart,
    /// From the text to the numbers, in points (None: automatic).
    pub distance: Option<f32>,
}

impl Default for LineNumbering {
    fn default() -> Self {
        LineNumbering { count_by: 1, start: 1, restart: LineRestart::EachPage, distance: None }
    }
}

impl PageSetup {
    pub const A4: PageSetup = PageSetup {
        width: 595.3,
        height: 841.9,
        margin_top: 72.0,
        margin_bottom: 72.0,
        margin_left: 72.0,
        margin_right: 72.0,
        margin_header: 35.4,
        margin_footer: 35.4,
        line_numbers: None,
        gutter: 0.0,
        gutter_top: false,
        mirror: false,
        v_align: PageVAlign::Top,
        grid: DocGrid { kind: GridKind::None, line_pitch: 0.0, char_space: 0, ruby_height: 0.0, color: None, display: false, print: false },
        text_flow: None,
    };
    pub const LETTER: PageSetup = PageSetup { width: 612.0, height: 792.0, ..PageSetup::A4 };

    pub fn landscape(&self) -> bool {
        self.width > self.height
    }

    /// Swaps width and height when the orientation differs.
    pub fn set_landscape(&mut self, on: bool) {
        if self.landscape() != on {
            std::mem::swap(&mut self.width, &mut self.height);
        }
    }

    /// The gutter is at the top (never with mirrored margins).
    fn gutter_at_top(&self) -> bool {
        self.gutter_top && !self.mirror
    }

    pub fn content_width(&self) -> f32 {
        self.width - self.margin_left - self.margin_right - if self.gutter_at_top() { 0.0 } else { self.gutter }
    }

    pub fn content_height(&self) -> f32 {
        self.height - self.margin_top - self.margin_bottom - if self.gutter_at_top() { self.gutter } else { 0.0 }
    }

    /// Left edge of the text on page `index` (0 is the first, a right-hand
    /// page).
    pub fn text_left(&self, index: usize) -> f32 {
        let verso = self.mirror && index % 2 == 1;
        let gutter = if self.gutter_at_top() || verso { 0.0 } else { self.gutter };
        (if verso { self.margin_right } else { self.margin_left }) + gutter
    }

    /// Top edge of the text.
    pub fn text_top(&self) -> f32 {
        self.margin_top + if self.gutter_at_top() { self.gutter } else { 0.0 }
    }
}

impl Default for PageSetup {
    fn default() -> Self {
        PageSetup::A4
    }
}

/// Paper sizes offered in page setup: name and portrait size in points.
pub const PAPER_SIZES: [(&str, f32, f32); 8] = [
    ("A4", 595.3, 841.9),
    ("Letter", 612.0, 792.0),
    ("Legal", 612.0, 1008.0),
    ("A3", 841.9, 1190.6),
    ("A5", 419.5, 595.3),
    ("B5", 498.9, 708.7),
    ("Executive", 522.0, 756.0),
    ("Tabloid", 792.0, 1224.0),
];

/// One header or footer: left, center and right parts. Parts use Excel's
/// codes: `&P` page number, `&N` number of pages, `&D` date, `&T` time,
/// `&F` file name, `&A` sheet name and `&&` for a literal `&`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct HfText {
    pub left: String,
    pub center: String,
    pub right: String,
}

impl HfText {
    pub fn center(s: &str) -> HfText {
        HfText { center: s.into(), ..Default::default() }
    }

    pub fn is_empty(&self) -> bool {
        self.left.is_empty() && self.center.is_empty() && self.right.is_empty()
    }

    pub fn parts(&self) -> [&str; 3] {
        [&self.left, &self.center, &self.right]
    }
}

/// Values for header and footer fields.
#[derive(Clone, Debug, Default)]
pub struct HfContext<'a> {
    pub page: usize,
    pub pages: usize,
    pub date: &'a str,
    pub time: &'a str,
    pub file: &'a str,
    pub sheet: &'a str,
}

/// Replaces the field codes in a header or footer part.
pub fn expand_fields(part: &str, cx: &HfContext) -> String {
    let mut out = String::new();
    let mut it = part.chars().peekable();
    while let Some(c) = it.next() {
        if c != '&' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('P' | 'p') => out.push_str(&cx.page.to_string()),
            Some('N' | 'n') => out.push_str(&cx.pages.to_string()),
            Some('D' | 'd') => out.push_str(cx.date),
            Some('T' | 't') => out.push_str(cx.time),
            Some('F' | 'f') => out.push_str(cx.file),
            Some('A' | 'a') => out.push_str(cx.sheet),
            Some('&') => out.push('&'),
            Some(o) => {
                out.push('&');
                out.push(o);
            }
            None => out.push('&'),
        }
    }
    out
}

/// Headers and footers of a document or sheet.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct HeaderFooter {
    /// Used on every page (odd pages when `odd_even`).
    pub header: HfText,
    pub footer: HfText,
    /// The first page has its own (possibly empty) header and footer.
    pub different_first: bool,
    pub first_header: HfText,
    pub first_footer: HfText,
    /// Even pages have their own header and footer.
    pub odd_even: bool,
    pub even_header: HfText,
    pub even_footer: HfText,
}

impl HeaderFooter {
    pub fn is_empty(&self) -> bool {
        [&self.header, &self.footer, &self.first_header, &self.first_footer, &self.even_header, &self.even_footer].iter().all(|t| t.is_empty())
    }

    /// Header and footer of page `page` (0-based).
    pub fn for_page(&self, page: usize) -> (&HfText, &HfText) {
        if page == 0 && self.different_first {
            (&self.first_header, &self.first_footer)
        } else if page % 2 == 1 && self.odd_even {
            (&self.even_header, &self.even_footer)
        } else {
            (&self.header, &self.footer)
        }
    }
}

/// Document properties (File > Properties).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meta {
    pub title: String,
    pub author: String,
    pub subject: String,
    /// Keywords, as typed (separated by commas or semicolons).
    pub keywords: String,
    /// Comments.
    pub description: String,
    pub category: String,
    pub company: String,
    pub manager: String,
    /// When the document was created (ISO 8601, kept from the file).
    pub created: String,
    pub custom: Vec<CustomProp>,
    /// The author recommends opening the file read-only (Always Open
    /// Read-Only in Office).
    pub read_only_recommended: bool,
}

/// A custom property.
#[derive(Clone, Debug, PartialEq)]
pub struct CustomProp {
    pub name: String,
    pub value: PropValue,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PropValue {
    Text(String),
    Number(f64),
    Bool(bool),
    /// ISO 8601 date ("2026-09-25" or with a time).
    Date(String),
}

impl PropValue {
    /// The value as the properties dialog shows it.
    pub fn text(&self) -> String {
        match self {
            PropValue::Text(s) | PropValue::Date(s) => s.clone(),
            PropValue::Number(n) => if n.fract() == 0.0 && n.abs() < 1e15 { format!("{n:.0}") } else { n.to_string() },
            PropValue::Bool(b) => b.to_string(),
        }
    }
}

/// The built-in properties DOCPROPERTY reads, by the names Word uses.
pub const BUILTIN_PROPS: [&str; 8] = ["Title", "Subject", "Author", "Keywords", "Comments", "Category", "Company", "Manager"];

impl Meta {
    /// A property by name (case aside): built-in first, then custom.
    pub fn property(&self, name: &str) -> Option<String> {
        let n = name.trim();
        let builtin = match n.to_ascii_lowercase().as_str() {
            "title" => Some(&self.title),
            "subject" => Some(&self.subject),
            "author" => Some(&self.author),
            "keywords" => Some(&self.keywords),
            "comments" => Some(&self.description),
            "category" => Some(&self.category),
            "company" => Some(&self.company),
            "manager" => Some(&self.manager),
            _ => None,
        };
        builtin.cloned().or_else(|| self.custom.iter().find(|p| p.name.eq_ignore_ascii_case(n)).map(|p| p.value.text()))
    }
}

#[derive(Clone, Debug)]
struct Edit {
    start: usize,
    /// Paragraphs that were replaced.
    before: Vec<Paragraph>,
    /// Number of paragraphs that replaced them.
    after_len: usize,
    sel_before: Selection,
    sel_after: Selection,
    /// Typing edits can be merged into one undo step.
    kind: EditKind,
    /// Undone/redone typing never merges with new typing.
    merge_closed: bool,
    /// The styles before the edit, when it changed them.
    styles: Option<Box<Styles>>,
    /// The comment list before the edit, when it changed.
    comments: Option<Vec<Comment>>,
    /// Document-wide page settings before the step, when it changed them.
    page: Option<Box<PageState>>,
    /// Properties, macros and kept parts before the step, when it changed
    /// them (the file inspector).
    parts: Option<Box<inspect::FileParts>>,
}

/// What an undo step did, for the undo history list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    Typing,
    Insert,
    Delete,
    Format,
    Paragraph,
    Replace,
    Link,
    Bookmark,
    Image,
    Table,
    Note,
    Field,
    Toc,
    /// A heading's section moved.
    Outline,
    Comment,
    Revision,
    Shape,
}

/// A named position or range of the document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bookmark {
    pub name: Arc<str>,
    pub start: Pos,
    pub end: Pos,
}

impl Bookmark {
    /// Names starting with '_' (`_Toc…`, `_GoBack`) are kept in files but
    /// not shown in the bookmark list, like other editors do.
    pub fn is_hidden(&self) -> bool {
        self.name.starts_with('_')
    }
}

/// A bookmark name the user may create: a letter first, then letters,
/// digits or '_', at most 40 characters.
pub fn valid_bookmark_name(name: &str) -> bool {
    let mut it = name.chars();
    it.next().is_some_and(char::is_alphabetic) && it.all(|c| c.is_alphanumeric() || c == '_') && name.chars().count() <= 40
}

/// The link target for a URL-looking word typed in the text ("www.x.com",
/// "https://…", "ana@x.es"), for autoformat as you type.
pub fn autolink_target(word: &str) -> Option<String> {
    let lower = word.to_ascii_lowercase();
    let schemes = ["http://", "https://", "ftp://", "file://", "mailto:"];
    if schemes.iter().any(|s| lower.starts_with(s) && word.len() > s.len() + 1) {
        return Some(word.to_string());
    }
    if lower.starts_with("www.") && word.len() > 6 && word[4..].contains('.') {
        return Some(format!("http://{word}"));
    }
    if let Some((user, domain)) = word.split_once('@') {
        let ok = |c: char| c.is_alphanumeric() || "._-+".contains(c);
        if !user.is_empty() && user.chars().all(ok) && domain.chars().all(|c| c.is_alphanumeric() || ".-".contains(c)) && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.') {
            return Some(format!("mailto:{word}"));
        }
    }
    None
}

/// Moves `off` back to a char boundary of `t` (and inside it).
fn floor_boundary(t: &str, off: usize) -> usize {
    let mut o = off.min(t.len());
    while !t.is_char_boundary(o) {
        o -= 1;
    }
    o
}

/// Collects bookmarks from paragraph marks: the first start and first end of
/// each name (case-insensitive), a missing half taking the other's place.
fn collect_bookmarks(paragraphs: &[Paragraph]) -> Vec<Bookmark> {
    let mut out: Vec<(Arc<str>, Option<Pos>, Option<Pos>)> = Vec::new();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (i, p) in paragraphs.iter().enumerate() {
        for m in p.marks.iter().filter(|m| !is_comment_mark(&m.name) && !is_perm_mark(&m.name)) {
            let k = m.name.to_lowercase();
            let j = *index.entry(k).or_insert_with(|| {
                out.push((m.name.clone(), None, None));
                out.len() - 1
            });
            let slot = if m.end { &mut out[j].2 } else { &mut out[j].1 };
            if slot.is_none() {
                *slot = Some(Pos::new(i, m.offset));
            }
        }
    }
    let mut v: Vec<Bookmark> = out
        .into_iter()
        .filter_map(|(name, s, e)| {
            let (s, e) = match (s, e) {
                (Some(s), Some(e)) => (s.min(e), s.max(e)),
                (Some(p), None) | (None, Some(p)) => (p, p),
                (None, None) => return None,
            };
            Some(Bookmark { name, start: s, end: e })
        })
        .collect();
    v.sort_by(|a, b| (a.start, a.end).cmp(&(b.start, b.end)));
    v
}

/// An optional hyphen: shows as "-" only where a line breaks at it.
pub const SOFT_HYPHEN: char = '\u{AD}';

#[derive(Clone, Debug)]
pub struct Document {
    pub paragraphs: Vec<Paragraph>,
    pub page: PageSetup,
    pub meta: Meta,
    /// Default font family (None = UI default).
    pub default_font: Option<Arc<str>>,
    /// Headers and footers of the first section.
    pub headers: Headers,
    /// Even pages have their own headers and footers.
    pub odd_even: bool,
    pub styles: Styles,
    /// Comment bodies; their anchors are marks in the paragraphs.
    pub comments: Vec<Comment>,
    /// Track changes: typing and deleting leave revisions.
    pub track_changes: bool,
    /// Words may break at the line end with a hyphen (Layout > Hyphenation).
    pub auto_hyphenation: bool,
    /// Restrict Editing.
    pub protection: Option<Protection>,
    /// Author of new comments and tracked changes.
    pub user: Arc<str>,
    /// Text columns of the first section.
    pub columns: Columns,
    /// Page color, borders and watermark.
    pub deco: PageDeco,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    /// Incremented on every change; paragraphs changed since are tracked in
    /// `dirty_from` so layout caches can invalidate cheaply.
    pub revision: u64,
    pub modified: bool,
    /// Footnote and endnote numbering.
    pub notes: NotesConfig,
    /// Sources of citations and the citation style.
    pub biblio: Bibliography,
    /// Parts of the file kept as they were (custom XML, label list…).
    pub kept: KeptParts,
    /// Book data, when the document is a book (book mode).
    pub book: Option<book::Book>,
    /// Macros kept from the file (not run).
    pub macros: Macros,
    /// Fonts saved inside the file (see `fonts.rs`).
    pub fonts: EmbeddedFonts,
    /// Paragraphs past `MAX_PARAGRAPHS` were left out when the file was
    /// read (see `cap_paragraphs`).
    pub truncated: bool,
}

/// Paragraphs a document read from a file keeps. Each costs about 650
/// bytes in memory, more with formatting: files of millions of lines
/// would take gigabytes. What is past it is left out, and the document
/// says so (`truncated`).
pub const MAX_PARAGRAPHS: usize = 500_000;

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    pub fn new() -> Self {
        Self::from_paragraphs(vec![Paragraph::new("")])
    }

    pub fn from_paragraphs(mut paragraphs: Vec<Paragraph>) -> Self {
        if paragraphs.is_empty() {
            paragraphs.push(Paragraph::new(""));
        }
        for p in &mut paragraphs {
            if p.runs.is_empty() {
                p.runs.push(Run { len: p.text.len(), attrs: Attrs::default() });
            }
            p.normalize();
        }
        let mut paragraphs = table::normalize_paras(paragraphs);
        fix_marks(&mut paragraphs);
        Document { paragraphs, page: PageSetup::default(), meta: Meta::default(), default_font: None, headers: Headers::default(), odd_even: false, comments: Vec::new(), track_changes: false, auto_hyphenation: false, protection: None, user: Arc::from(""), styles: Styles::default(), columns: Columns::default(), deco: PageDeco::default(), undo: Vec::new(), redo: Vec::new(), revision: 0, modified: false, notes: NotesConfig::default(), biblio: Bibliography::default(), kept: KeptParts::default(), book: None, macros: Macros::default(), fonts: EmbeddedFonts::default(), truncated: false }
    }

    pub fn from_plain_text(s: &str) -> Self {
        let s = s.strip_prefix('\u{feff}').unwrap_or(s);
        // One line past the limit, so `cap_paragraphs` sees there was more.
        let lines = s.split('\n').take(MAX_PARAGRAPHS + 1);
        let mut d = Self::from_paragraphs(lines.map(|l| Paragraph::new(l.strip_suffix('\r').unwrap_or(l))).collect());
        d.cap_paragraphs();
        d
    }

    /// Leaves out the paragraphs past `MAX_PARAGRAPHS` (a table cut short
    /// is completed) and marks the document `truncated`. Whether it did.
    pub fn cap_paragraphs(&mut self) -> bool {
        if self.paragraphs.len() <= MAX_PARAGRAPHS {
            return false;
        }
        let mut ps = std::mem::take(&mut self.paragraphs);
        ps.truncate(MAX_PARAGRAPHS);
        ps.shrink_to_fit();
        let mut ps = table::normalize_paras(ps);
        fix_marks(&mut ps);
        self.paragraphs = ps;
        self.truncated = true;
        true
    }

    pub fn to_plain_text(&self) -> String {
        let mut out = String::new();
        for (i, p) in self.paragraphs.iter().enumerate() {
            if i > 0 {
                out.push(self.separator(i));
            }
            push_plain(&mut out, &p.text);
        }
        out
    }

    /// What separates paragraph `i` from the previous one in plain text: a
    /// tab between cells of a row, else a newline.
    fn separator(&self, i: usize) -> char {
        match (self.container(i - 1), self.container(i)) {
            (Some(a), Some(b)) if a.0 == b.0 && a.1 == b.1 && a.2 != b.2 => '\t',
            _ => '\n',
        }
    }

    pub fn end(&self) -> Pos {
        let last = self.paragraphs.len() - 1;
        Pos::new(last, self.paragraphs[last].len())
    }

    pub fn clamp(&self, p: Pos) -> Pos {
        let para = p.para.min(self.paragraphs.len() - 1);
        let t = &self.paragraphs[para].text;
        let mut off = p.offset.min(t.len());
        while !t.is_char_boundary(off) {
            off -= 1;
        }
        Pos::new(para, off)
    }

    pub fn word_count(&self) -> usize {
        self.paragraphs.iter().map(|p| count_words(&p.visible(0, p.len()))).sum()
    }

    pub fn char_count(&self) -> usize {
        self.paragraphs.iter().map(|p| p.visible(0, p.len()).chars().count()).sum()
    }

    /// Text between two positions, with '\n' between paragraphs.
    pub fn text_range(&self, a: Pos, b: Pos) -> String {
        let (a, b) = (a.min(b), a.max(b));
        let mut out = String::new();
        for i in a.para..=b.para {
            let t = &self.paragraphs[i].text;
            let s = if i == a.para { a.offset } else { 0 };
            let e = if i == b.para { b.offset } else { t.len() };
            if i > a.para {
                out.push(self.separator(i));
            }
            push_plain(&mut out, &t[s..e]);
        }
        out
    }

    /// Copies a styled slice as standalone paragraphs.
    pub fn slice(&self, a: Pos, b: Pos) -> Vec<Paragraph> {
        let (a, b) = (a.min(b), a.max(b));
        // Text from one cell is plain text; across cells, table cells.
        let cells = !self.one_container(a.para, b.para);
        let mut out = Vec::new();
        for i in a.para..=b.para {
            let mut p = self.paragraphs[i].clone();
            let e = if i == b.para { b.offset } else { p.len() };
            p.split_off(e);
            let s = if i == a.para { a.offset } else { 0 };
            let mut p = p.split_off(s);
            p.style = self.paragraphs[i].style;
            p.marks.clear();
            p.cell = if cells { self.paragraphs[i].cell.clone() } else { None };
            out.push(p);
        }
        out
    }

    // --- navigation ------------------------------------------------------

    pub fn next_char(&self, p: Pos) -> Pos {
        let t = &self.paragraphs[p.para].text;
        if p.offset < t.len() {
            let n = t[p.offset..].chars().next().map_or(1, char::len_utf8);
            Pos::new(p.para, p.offset + n)
        } else {
            // Cells covered by a merged cell are skipped.
            let mut i = p.para + 1;
            while i < self.paragraphs.len() && self.is_hidden(i) {
                i += 1;
            }
            if i < self.paragraphs.len() { Pos::new(i, 0) } else { p }
        }
    }

    pub fn prev_char(&self, p: Pos) -> Pos {
        if p.offset > 0 {
            let t = &self.paragraphs[p.para].text;
            let n = t[..p.offset].chars().next_back().map_or(1, char::len_utf8);
            Pos::new(p.para, p.offset - n)
        } else {
            let mut i = p.para;
            while i > 0 {
                i -= 1;
                if !self.is_hidden(i) {
                    return Pos::new(i, self.paragraphs[i].len());
                }
            }
            p
        }
    }

    pub fn next_word(&self, p: Pos) -> Pos {
        let t = &self.paragraphs[p.para].text;
        if p.offset >= t.len() {
            return self.next_char(p);
        }
        let rest = &t[p.offset..];
        let mut it = rest.char_indices().peekable();
        let first_word = it.peek().is_some_and(|(_, c)| is_word(*c));
        let mut idx = rest.len();
        let mut in_space = !first_word;
        for (i, c) in rest.char_indices() {
            if in_space {
                if !c.is_whitespace() && (is_word(c) || first_word) {
                    idx = i;
                    break;
                }
                if !c.is_whitespace() && !first_word && i > 0 {
                    idx = i;
                    break;
                }
            } else if !is_word(c) {
                in_space = true;
                if !c.is_whitespace() {
                    idx = i;
                    break;
                }
            }
        }
        Pos::new(p.para, p.offset + idx)
    }

    pub fn prev_word(&self, p: Pos) -> Pos {
        if p.offset == 0 {
            return self.prev_char(p);
        }
        let t = &self.paragraphs[p.para].text[..p.offset];
        let mut chars = t.char_indices().rev().peekable();
        while let Some(&(_, c)) = chars.peek() {
            if c.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }
        let word = chars.peek().is_some_and(|(_, c)| is_word(*c));
        let mut start = 0;
        for (i, c) in chars {
            if is_word(c) != word || c.is_whitespace() {
                start = i + c.len_utf8();
                break;
            }
        }
        Pos::new(p.para, start)
    }

    /// Word containing `p` (for double click).
    pub fn word_at(&self, p: Pos) -> (Pos, Pos) {
        let p = self.clamp(p);
        let t = &self.paragraphs[p.para].text;
        let mut s = p.offset;
        while s > 0 {
            let c = t[..s].chars().next_back().unwrap();
            if !is_word(c) {
                break;
            }
            s -= c.len_utf8();
        }
        let mut e = p.offset;
        while e < t.len() {
            let c = t[e..].chars().next().unwrap();
            if !is_word(c) {
                break;
            }
            e += c.len_utf8();
        }
        if s == e && e < t.len() {
            e += t[e..].chars().next().unwrap().len_utf8();
        }
        (Pos::new(p.para, s), Pos::new(p.para, e))
    }

    /// Finds the next match of `needle` (case-insensitive) after `from`.
    pub fn find(&self, needle: &str, from: Pos, case_sensitive: bool) -> Option<(Pos, Pos)> {
        if needle.is_empty() || needle.contains('\n') {
            return None;
        }
        let n = if case_sensitive { needle.to_string() } else { needle.to_lowercase() };
        let count = self.paragraphs.len();
        for k in 0..=count {
            let i = (from.para + k) % count;
            let t = &self.paragraphs[i].text;
            let start = if k == 0 { from.offset } else { 0 };
            let hay = self.paragraphs[i].blank_deleted(if case_sensitive { t.clone() } else { lower_same_len(t) });
            let hit = if k == count { hay[..from.offset.min(hay.len())].find(&n) } else { hay.get(start..).and_then(|h| h.find(&n)).map(|x| x + start) };
            if let Some(o) = hit {
                return Some((Pos::new(i, o), Pos::new(i, o + n.len())));
            }
        }
        None
    }

    pub fn find_all(&self, needle: &str, case_sensitive: bool) -> Vec<(Pos, Pos)> {
        self.find_all_opts(needle, case_sensitive, false)
    }

    /// Like `find_all`; `whole_word` skips matches inside longer words.
    pub fn find_all_opts(&self, needle: &str, case_sensitive: bool, whole_word: bool) -> Vec<(Pos, Pos)> {
        let mut out = Vec::new();
        if needle.is_empty() || needle.contains('\n') {
            return out;
        }
        let n = if case_sensitive { needle.to_string() } else { needle.to_lowercase() };
        for (i, p) in self.paragraphs.iter().enumerate() {
            let hay = p.blank_deleted(if case_sensitive { p.text.clone() } else { lower_same_len(&p.text) });
            let mut s = 0;
            while let Some(o) = hay[s..].find(&n) {
                let (x, y) = (s + o, s + o + n.len());
                let t = &p.text;
                let ok = !whole_word || (!t[..x].chars().next_back().is_some_and(is_word) && !t[y..].chars().next().is_some_and(is_word));
                if ok {
                    out.push((Pos::new(i, x), Pos::new(i, y)));
                }
                s = x + n.len().max(1);
                while !hay.is_char_boundary(s) {
                    s += 1;
                }
            }
        }
        out
    }

    /// Numbering label of a numbered-list paragraph (1-based within its run).
    pub fn list_number(&self, idx: usize) -> usize {
        let st = self.paragraphs[idx].style;
        let mut n = 1;
        let mut i = idx;
        if st.start_at > 0 {
            return st.start_at as usize;
        }
        if st.restart {
            return 1;
        }
        while i > 0 {
            i -= 1;
            let s = self.paragraphs[i].style;
            if s.kind == ParaKind::Numbered && s.level == st.level {
                if s.start_at > 0 {
                    n += s.start_at as usize;
                    break;
                }
                n += 1;
                if s.restart {
                    break;
                }
            } else if s.kind.is_list() && s.level > st.level {
                continue;
            } else {
                break;
            }
        }
        n
    }

    /// `list_number` of every paragraph at once (0 for those that aren't
    /// numbered): one pass, where calling `list_number` for each paragraph
    /// of a long list walks it back each time.
    pub fn list_numbers(&self) -> Vec<usize> {
        // Per level, the number of the last numbered paragraph while its
        // run goes on (deeper list items don't end it).
        let mut chain: Vec<Option<usize>> = Vec::new();
        let mut out = Vec::with_capacity(self.paragraphs.len());
        for p in &self.paragraphs {
            let st = p.style;
            let l = st.level as usize;
            let mut n = 0;
            if st.kind == ParaKind::Numbered {
                n = if st.start_at > 0 {
                    st.start_at as usize
                } else if st.restart {
                    1
                } else {
                    chain.get(l).copied().flatten().map_or(1, |k| k + 1)
                };
            }
            if st.kind.is_list() {
                chain.truncate(l);
                chain.resize(l, None);
                if st.kind == ParaKind::Numbered {
                    chain.push(Some(n));
                }
            } else {
                chain.clear();
            }
            out.push(n);
        }
        out
    }

    // --- editing ---------------------------------------------------------

    fn record(&mut self, start: usize, before: Vec<Paragraph>, after_len: usize, sel_before: Selection, sel_after: Selection, kind: EditKind) {
        self.redo.clear();
        self.revision += 1;
        self.modified = true;
        if kind == EditKind::Typing {
            if let Some(last) = self.undo.last_mut() {
                if last.kind == EditKind::Typing && !last.merge_closed && last.sel_after == sel_before && before.len() == 1 && after_len == 1 && last.after_len == 1 && last.start == start {
                    last.sel_after = sel_after;
                    return;
                }
            }
        }
        // Typing in a note changes it again and again: one step.
        if kind == EditKind::Note && before.len() == 1 && after_len == 1 {
            if let Some(last) = self.undo.last_mut() {
                if last.kind == EditKind::Note && !last.merge_closed && last.after_len == 1 && last.start == start && last.sel_after == sel_before && last.before.len() == 1 && same_notes(&last.before[0], &before[0]) {
                    last.sel_after = sel_after;
                    return;
                }
            }
        }
        self.undo.push(Edit { start, before, after_len, sel_before, sel_after, kind, merge_closed: false, styles: None, comments: None, page: None, parts: None });
        if self.undo.len() > 1000 {
            self.undo.remove(0);
        }
    }

    /// Replaces the selection with `text` (may contain '\n'). Returns the new caret.
    pub fn insert_text(&mut self, sel: Selection, text: &str, attrs: Option<Attrs>) -> Pos {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let (a, b) = (self.clamp(sel.start()), self.clamp(sel.end()));
        let before: Vec<Paragraph> = self.paragraphs[a.para..=b.para].to_vec();
        let n0 = self.paragraphs.len();
        let mut attrs = attrs.unwrap_or_else(|| self.typing_attrs(a));
        self.stamp(&mut attrs);
        let pos = self.raw_delete(a, b);
        let mut pos = pos;
        let mut lines = text.split('\n');
        if let Some(first) = lines.next() {
            self.paragraphs[pos.para].insert_str(pos.offset, first, attrs.clone());
            pos.offset += first.len();
        }
        for line in lines {
            let tail = self.paragraphs[pos.para].split_off(pos.offset);
            self.paragraphs.insert(pos.para + 1, tail);
            pos = Pos::new(pos.para + 1, 0);
            self.paragraphs[pos.para].insert_str(0, line, attrs.clone());
            pos.offset = line.len();
        }
        let after_len = (b.para + 1 + self.paragraphs.len()).saturating_sub(a.para + n0);
        let kind = if !text.contains('\n') && sel.is_empty() && text.chars().count() == 1 && !text.starts_with(' ') { EditKind::Typing } else if text == "\n" { EditKind::Paragraph } else { EditKind::Insert };
        self.record(a.para, before, after_len, sel, Selection::caret(pos), kind);
        pos
    }

    /// Inserts styled paragraphs (from the clipboard) at the selection.
    pub fn insert_paragraphs(&mut self, sel: Selection, paras: &[Paragraph]) -> Pos {
        if paras.is_empty() {
            return sel.cursor;
        }
        let (a, b) = (sel.start(), sel.end());
        let before: Vec<Paragraph> = self.paragraphs[a.para..=b.para].to_vec();
        let n0 = self.paragraphs.len();
        let pos = self.raw_delete(a, b);
        let target = self.paragraphs[pos.para].cell.clone();
        let mut paras = self.retag_pasted(paras);
        self.stamp_paras(&mut paras);
        let caret = if paras.iter().any(|p| p.cell.is_some()) {
            // A table: it goes between the two halves of the paragraph (or
            // before it, at its start); in a cell it's nested in the cell.
            let mut head = self.paragraphs[pos.para].clone();
            let mut tail = head.split_off(pos.offset);
            let mut new = Vec::new();
            let first_in_cell = pos.offset == 0;
            if pos.offset == 0 {
                tail.marks.append(&mut head.marks);
                tail.style = head.style;
            } else {
                new.push(head);
            }
            // With the text around it (the paragraph closing a table that
            // ends the clip is the one at the caret).
            let ends_in_table = paras.last().is_some_and(|p| p.cell.is_some());
            let mut tables = table::normalize_paras(paras);
            if ends_in_table && tables.last().is_some_and(|p| p.cell.is_none()) {
                tables.pop();
            }
            if let Some(t) = &target {
                table::wrap_in(t, first_in_cell, &mut tables);
            }
            new.extend(tables);
            let at = pos.para + new.len();
            new.push(tail);
            self.paragraphs.splice(pos.para..=pos.para, new);
            Pos::new(at, 0)
        } else {
            let tail = self.paragraphs[pos.para].split_off(pos.offset);
            let mut cur = pos.para;
            {
                let mut first = paras[0].clone();
                first.cell = None;
                if self.paragraphs[cur].is_empty() {
                    self.paragraphs[cur].style = first.style;
                } else {
                    let now = self.paragraphs[cur].style;
                    self.keep_look_in(&mut first, &now);
                }
                self.paragraphs[cur].append(first);
            }
            for p in &paras[1..] {
                cur += 1;
                let mut p = p.clone();
                p.cell = target.as_ref().map(InCell::bare);
                self.paragraphs.insert(cur, p);
            }
            let caret = Pos::new(cur, self.paragraphs[cur].len());
            self.paragraphs[cur].append(tail);
            caret
        };
        let after_len = (b.para + 1 + self.paragraphs.len()).saturating_sub(a.para + n0);
        self.record(a.para, before, after_len, sel, Selection::caret(caret), EditKind::Insert);
        caret
    }

    /// True if paragraphs `a..=b` are all in the same cell, or all outside
    /// tables.
    fn one_container(&self, a: usize, b: usize) -> bool {
        let k = self.container(a);
        (a + 1..=b).all(|i| self.container(i) == k)
    }

    fn raw_delete(&mut self, a: Pos, b: Pos) -> Pos {
        // Tracked: the text stays, marked deleted; new text goes after it.
        if self.track_changes {
            return self.track_delete(a, b).1;
        }
        if a.para != b.para && !self.one_container(a.para, b.para) {
            self.delete_across(a, b);
        } else {
            self.delete_within(a, b);
        }
        a
    }

    /// Deletes a range that crosses cells or tables without merging
    /// paragraphs of different cells: each cell (or run of text outside
    /// tables) loses its part, so the grid stays. Tables entirely inside a
    /// range that reaches outside them are removed.
    fn delete_across(&mut self, a: Pos, b: Pos) {
        let mut segs: Vec<(usize, usize)> = Vec::new();
        for i in a.para..=b.para {
            match segs.last_mut() {
                Some(s) if self.container(i) == self.container(i - 1) => s.1 = i,
                _ => segs.push((i, i)),
            }
        }
        let mut remove: Vec<u32> = Vec::new();
        for &(s, _) in &segs {
            let Some((id, _, _)) = self.container(s) else { continue };
            if remove.contains(&id) {
                continue;
            }
            if let Some((ts, te)) = self.table_bounds(s) {
                let (first, last) = (Pos::new(ts, 0), Pos::new(te - 1, self.paragraphs[te - 1].len()));
                if a <= first && b >= last && (a < first || b > last) {
                    remove.push(id);
                }
            }
        }
        let body_ends = self.container(a.para).is_none() && self.container(b.para).is_none();
        let mut orphans = Vec::new();
        let mut counts = Vec::with_capacity(segs.len());
        for &(s, e) in segs.iter().rev() {
            if self.paragraphs[s].cell.as_ref().is_some_and(|c| remove.iter().any(|&id| c.in_table(id))) {
                // A nested table's cell properties around it stay with the
                // paragraph after it.
                if let (Some(from), Some(Some(to))) = (self.paragraphs[s].cell.clone(), self.paragraphs.get_mut(e + 1).map(|p| p.cell.as_mut())) {
                    table::inherit_props(&from, to);
                }
                for p in self.paragraphs.drain(s..=e) {
                    orphans.extend(p.marks);
                }
                counts.push(0);
            } else {
                let from = if s == a.para { a } else { Pos::new(s, 0) };
                let to = if e == b.para { b } else { Pos::new(e, self.paragraphs[e].len()) };
                self.delete_within(from, to);
                counts.push(1);
            }
        }
        counts.reverse();
        // With the tables in between gone, the text around them joins.
        let idx_b = a.para + counts[..counts.len() - 1].iter().sum::<usize>();
        if !remove.is_empty() && body_ends && idx_b == a.para + 1 && idx_b < self.paragraphs.len() && !self.in_table(idx_b) && !self.in_table(a.para) {
            let tail = self.paragraphs.remove(idx_b);
            self.paragraphs[a.para].append(tail);
        }
        if let Some(p) = self.paragraphs.get_mut(a.para) {
            let off = a.offset.min(p.len());
            p.marks.extend(orphans.into_iter().map(|m| Mark { offset: off, ..m }));
        }
    }

    fn delete_within(&mut self, a: Pos, b: Pos) {
        if a.para == b.para {
            self.paragraphs[a.para].remove(a.offset, b.offset);
        } else {
            let tail = self.paragraphs[b.para].split_off(b.offset);
            // Marks inside the deleted text collapse to its start.
            let mut orphans = Vec::new();
            for p in &mut self.paragraphs[a.para + 1..=b.para] {
                orphans.append(&mut p.marks);
            }
            let len = self.paragraphs[a.para].len();
            self.paragraphs[a.para].remove(a.offset, len);
            self.paragraphs.drain(a.para + 1..=b.para);
            let first = &mut self.paragraphs[a.para];
            first.marks.extend(orphans.into_iter().map(|m| Mark { offset: a.offset, ..m }));
            first.append(tail);
        }
    }

    /// Deletes the selection; returns the caret.
    pub fn delete(&mut self, sel: Selection) -> Pos {
        if sel.is_empty() {
            return sel.cursor;
        }
        if self.track_changes {
            return self.delete_tracked(sel, false);
        }
        let (a, b) = (sel.start(), sel.end());
        let before = self.paragraphs[a.para..=b.para].to_vec();
        let n0 = self.paragraphs.len();
        let pos = self.raw_delete(a, b);
        let after_len = (b.para + 1 + self.paragraphs.len()).saturating_sub(a.para + n0);
        self.record(a.para, before, after_len, sel, Selection::caret(pos), EditKind::Delete);
        self.clamp(pos)
    }

    /// Backspace behaviour. Returns new caret.
    pub fn backspace(&mut self, sel: Selection, word: bool) -> Pos {
        if !sel.is_empty() {
            return self.delete(sel);
        }
        let p = sel.cursor;
        // Backspace at start of a list/heading paragraph first resets its style.
        if p.offset == 0 {
            let st = self.paragraphs[p.para].style;
            if st.kind != ParaKind::Normal || st.level > 0 {
                self.set_para_style(p.para, p.para, sel, &|s| {
                    if s.level > 0 && s.kind.is_list() {
                        s.level -= 1
                    } else {
                        s.kind = ParaKind::Normal;
                        s.level = 0;
                    }
                });
                return p;
            }
        }
        let start = if word { self.prev_word(p) } else { self.prev_char(p) };
        // Never across the edge of a cell or table.
        if start.para != p.para && self.container(start.para) != self.container(p.para) {
            return p;
        }
        self.delete(Selection { anchor: start, cursor: p })
    }

    pub fn delete_forward(&mut self, sel: Selection, word: bool) -> Pos {
        if !sel.is_empty() {
            return self.delete(sel);
        }
        let p = sel.cursor;
        let end = if word { self.next_word(p) } else { self.next_char(p) };
        if end.para != p.para && self.container(end.para) != self.container(p.para) {
            // An empty paragraph right before a table goes away; otherwise
            // nothing happens at the edge of a cell or table.
            if self.paragraphs[p.para].is_empty() && !self.in_table(p.para) && self.in_table(p.para + 1) {
                let before = vec![self.paragraphs.remove(p.para)];
                let caret = Pos::new(p.para, 0);
                self.record(p.para, before, 0, sel, Selection::caret(caret), EditKind::Delete);
                return caret;
            }
            return p;
        }
        if self.track_changes {
            return self.delete_tracked(Selection { anchor: p, cursor: end }, true);
        }
        self.delete(Selection { anchor: p, cursor: end })
    }

    /// Enter key: splits the paragraph. Empty list items end the list.
    pub fn new_paragraph(&mut self, sel: Selection) -> Pos {
        let p = sel.start();
        // At the very start of a table with no text before it (at the top of
        // the document or right after another table), Enter opens a
        // paragraph above the table.
        if sel.is_empty() && p.offset == 0 && self.table_bounds(p.para).is_some_and(|(a, _)| a == p.para) && (p.para == 0 || self.in_table(p.para - 1)) {
            self.paragraphs.insert(p.para, Paragraph::new(""));
            let caret = Pos::new(p.para, 0);
            self.record(p.para, Vec::new(), 1, sel, Selection::caret(caret), EditKind::Paragraph);
            return caret;
        }
        let para = &self.paragraphs[p.para];
        if sel.is_empty() && para.is_empty() && para.style.kind.is_list() {
            self.set_para_style(p.para, p.para, sel, &|s| {
                if s.level > 0 {
                    s.level -= 1
                } else {
                    s.kind = ParaKind::Normal
                }
            });
            return p;
        }
        // Enter at the end of a paragraph starts one in the style's next style.
        let next = if sel.is_empty() && p.offset == para.text.len() { self.styles.next_style(&para.style) } else { None };
        let caret = self.insert_text(sel, "\n", None);
        if let Some((kind, custom)) = next {
            if caret.para != p.para && self.paragraphs.get(caret.para).is_some_and(Paragraph::is_empty) {
                let styles = std::mem::take(&mut self.styles);
                styles.restyle(&mut self.paragraphs[caret.para].style, kind, custom);
                self.styles = styles;
            }
        }
        caret
    }

    /// Applies character formatting to the selection.
    pub fn format_range(&mut self, sel: Selection, f: &dyn Fn(&mut Attrs)) {
        self.format_with(sel, &|_, a| f(a), None);
    }

    /// Character formatting that depends on the paragraph kind (e.g. its
    /// base size), plus optional paragraph formatting, as one undo step.
    pub fn format_with(&mut self, sel: Selection, f: &dyn Fn(ParaKind, &mut Attrs), para: Option<&dyn Fn(&mut ParaStyle)>) {
        if sel.is_empty() {
            return;
        }
        let (a, b) = (sel.start(), sel.end());
        let before = self.paragraphs[a.para..=b.para].to_vec();
        for i in a.para..=b.para {
            let len = self.paragraphs[i].len();
            let s = if i == a.para { a.offset } else { 0 };
            let e = if i == b.para { b.offset } else { len };
            let kind = self.paragraphs[i].style.kind;
            self.paragraphs[i].map_attrs(s, e, &|x| f(kind, x));
            if let Some(g) = para {
                g(&mut self.paragraphs[i].style);
            }
        }
        self.record(a.para, before, b.para - a.para + 1, sel, sel, EditKind::Format);
    }

    /// Toggles a boolean attribute: sets it unless the whole selection has it.
    pub fn toggle(&mut self, sel: Selection, get: fn(&Attrs) -> bool, set: fn(&mut Attrs, bool)) {
        let all = self.range_all(sel, get);
        self.format_range(sel, &move |a| set(a, !all));
    }

    /// True if every character in the selection satisfies `get`.
    pub fn range_all(&self, sel: Selection, get: fn(&Attrs) -> bool) -> bool {
        let (a, b) = (sel.start(), sel.end());
        if sel.is_empty() {
            return get(&self.paragraphs[a.para].attrs_at(a.offset));
        }
        for i in a.para..=b.para {
            let p = &self.paragraphs[i];
            let s = if i == a.para { a.offset } else { 0 };
            let e = if i == b.para { b.offset } else { p.len() };
            for (rs, re, attrs) in p.spans() {
                if re > s && rs < e && !get(attrs) {
                    return false;
                }
            }
        }
        true
    }

    /// Modifies the paragraph style of paragraphs `from..=to`.
    pub fn set_para_style(&mut self, from: usize, to: usize, sel: Selection, f: &dyn Fn(&mut ParaStyle)) {
        let before = self.paragraphs[from..=to].to_vec();
        for p in &mut self.paragraphs[from..=to] {
            f(&mut p.style);
        }
        self.record(from, before, to - from + 1, sel, sel, EditKind::Paragraph);
    }

    /// Replaces the body with `paras` (one undo step) keeping the
    /// comments whose anchors are still in it (code edited back in).
    pub fn replace_body_keeping(&mut self, paras: Vec<Paragraph>) {
        let all = self.comments.clone();
        self.replace_body(paras);
        let anchored: std::collections::HashSet<u32> = self.paragraphs.iter().flat_map(|p| &p.marks).filter_map(|m| comment_id(&m.name)).collect();
        self.comments = all.into_iter().filter(|c| anchored.contains(&c.id)).collect();
    }

    /// Puts `paras` in place of the whole body as one undo step (a document
    /// read again from its source). The comments go too: their marks were
    /// in the old paragraphs.
    pub fn replace_body(&mut self, paras: Vec<Paragraph>) {
        let paras = if paras.is_empty() { vec![Paragraph::new("")] } else { paras };
        let before = std::mem::replace(&mut self.paragraphs, paras);
        let comments = std::mem::take(&mut self.comments);
        let sel = Selection::caret(Pos::new(0, 0));
        let n = self.paragraphs.len();
        self.record(0, before, n, sel, sel, EditKind::Replace);
        if let Some(e) = self.undo.last_mut() {
            e.comments = Some(comments);
        }
    }

    /// Replaces the whole document content (undoable).
    pub fn replace_all(&mut self, needle: &str, with: &str, case_sensitive: bool) -> usize {
        self.replace_all_opts(needle, with, case_sensitive, false)
    }

    pub fn replace_all_opts(&mut self, needle: &str, with: &str, case_sensitive: bool, whole_word: bool) -> usize {
        let hits = self.find_all_opts(needle, case_sensitive, whole_word);
        if hits.is_empty() {
            return 0;
        }
        let first = hits[0].0.para;
        let last = hits.last().unwrap().0.para;
        let before = self.paragraphs[first..=last].to_vec();
        for &(a, b) in hits.iter().rev() {
            let mut attrs = self.paragraphs[a.para].attrs_at(b.offset);
            if self.track_changes {
                // The match stays as a deletion, the new text follows it.
                let (_, end) = self.track_delete(a, b);
                self.stamp(&mut attrs);
                self.paragraphs[a.para].insert_str(end.offset, with, attrs);
                continue;
            }
            self.paragraphs[a.para].remove(a.offset, b.offset);
            self.paragraphs[a.para].insert_str(a.offset, with, attrs);
        }
        let s = Selection::caret(Pos::new(first, 0));
        self.record(first, before, last - first + 1, s, s, EditKind::Replace);
        hits.len()
    }

    /// Attributes for text typed at `p`: those on the left, except that text
    /// typed right after a link doesn't extend it.
    pub fn typing_attrs(&self, p: Pos) -> Attrs {
        let para = &self.paragraphs[p.para];
        let mut a = para.attrs_at(p.offset);
        if a.link.is_some() {
            // `attrs_at` looks left, so ask past the next character for its attrs.
            let next = para.text.get(p.offset..).and_then(|r| r.chars().next()).map(|c| para.attrs_at(p.offset + c.len_utf8()));
            if next.is_none_or(|n| n.link != a.link) {
                a.link = None;
                a.link_tip = None;
            }
        }
        // Past the end of a control that isn't typed in, text is outside it.
        if a.control.as_ref().is_some_and(|c| !c.editable()) {
            let next = para.text.get(p.offset..).and_then(|r| r.chars().next()).map(|c| para.attrs_at(p.offset + c.len_utf8()));
            if next.is_none_or(|n| n.control.as_ref().map(|c| c.id) != a.control.as_ref().map(|c| c.id)) {
                a.control = None;
            }
        }
        if a.asian.is_some() {
            let next = para.text.get(p.offset..).and_then(|r| r.chars().next()).map(|c| para.attrs_at(p.offset + c.len_utf8()));
            asian::typing(&mut a, next.as_ref());
        }
        a.clear_object();
        a
    }

    // --- links -----------------------------------------------------------------

    /// The link on the character at `p` or, failing that, the one before it:
    /// its whole extent (runs with the same target), target and screen tip.
    pub fn link_at(&self, p: Pos) -> Option<(Selection, Arc<str>, Option<Arc<str>>)> {
        let para = self.paragraphs.get(p.para)?;
        let spans: Vec<(usize, usize, &Attrs)> = para.spans().filter(|(s, e, _)| e > s).collect();
        let hit = spans.iter().position(|(s, e, a)| *s <= p.offset && p.offset < *e && a.link.is_some()).or_else(|| spans.iter().position(|(s, e, a)| *s < p.offset && p.offset <= *e && a.link.is_some()))?;
        let link = spans[hit].2.link.clone()?;
        let (mut i, mut j) = (hit, hit);
        while i > 0 && spans[i - 1].2.link.as_ref() == Some(&link) {
            i -= 1;
        }
        while j + 1 < spans.len() && spans[j + 1].2.link.as_ref() == Some(&link) {
            j += 1;
        }
        let sel = Selection { anchor: Pos::new(p.para, spans[i].0), cursor: Pos::new(p.para, spans[j].1) };
        Some((sel, link, spans[hit].2.link_tip.clone()))
    }

    /// Sets (or with `None` removes) the link of the selection, as one undo step.
    pub fn set_link(&mut self, sel: Selection, link: Option<Arc<str>>, tip: Option<Arc<str>>) {
        if sel.is_empty() {
            return;
        }
        let tip = tip.filter(|_| link.is_some());
        self.format_range(sel, &|a| {
            a.link = link.clone();
            a.link_tip = tip.clone();
        });
        if let Some(e) = self.undo.last_mut() {
            e.kind = EditKind::Link;
        }
    }

    /// Replaces the selection with `text` linked to `link`, keeping the
    /// formatting at its start. Returns the new link's extent.
    pub fn insert_link(&mut self, sel: Selection, text: &str, link: Arc<str>, tip: Option<Arc<str>>) -> Selection {
        let text = text.replace(['\n', '\r'], " ");
        if text.is_empty() {
            return sel;
        }
        let a = sel.start();
        let mut attrs = self.paragraphs[a.para].attrs_at(if sel.is_empty() { a.offset } else { self.next_char(a).offset.max(a.offset) });
        attrs.link = Some(link);
        attrs.link_tip = tip;
        let end = self.insert_text(sel, &text, Some(attrs));
        if let Some(e) = self.undo.last_mut() {
            e.kind = EditKind::Link;
        }
        Selection { anchor: Pos::new(end.para, end.offset - text.len()), cursor: end }
    }

    // --- pictures ----------------------------------------------------------------

    /// Replaces the selection with a picture. Returns the picture's extent.
    pub fn insert_image(&mut self, sel: Selection, image: Image) -> Selection {
        let a = sel.start();
        let mut attrs = self.typing_attrs(a);
        (attrs.link, attrs.link_tip) = (None, None);
        attrs.image = Some(Arc::new(image));
        let end = self.insert_text(sel, &OBJECT.to_string(), Some(attrs));
        if let Some(e) = self.undo.last_mut() {
            e.kind = EditKind::Image;
        }
        Selection { anchor: Pos::new(end.para, end.offset - OBJECT.len_utf8()), cursor: end }
    }

    /// The picture at `p` (the character right after it) and its extent.
    pub fn image_at(&self, p: Pos) -> Option<(Selection, Arc<Image>)> {
        let para = self.paragraphs.get(p.para)?;
        if !para.text.get(p.offset..)?.starts_with(OBJECT) {
            return None;
        }
        let img = para.attrs_at(p.offset + OBJECT.len_utf8()).image?;
        Some((Selection { anchor: p, cursor: Pos::new(p.para, p.offset + OBJECT.len_utf8()) }, img))
    }

    /// Changes the picture at `p` (as one undo step).
    pub fn set_image(&mut self, p: Pos, image: Image) {
        let Some((sel, _)) = self.image_at(p) else { return };
        let image = Arc::new(image);
        self.format_range(sel, &|a| a.image = Some(image.clone()));
        if let Some(e) = self.undo.last_mut() {
            e.kind = EditKind::Image;
        }
    }

    /// Every picture: its position and data.
    pub fn images(&self) -> Vec<(Pos, Arc<Image>)> {
        let mut out = Vec::new();
        for (i, para) in self.paragraphs.iter().enumerate() {
            for (s, e, a) in para.spans() {
                if let Some(img) = &a.image {
                    for (o, c) in para.text[s..e].char_indices() {
                        if c == OBJECT {
                            out.push((Pos::new(i, s + o), img.clone()));
                        }
                    }
                }
            }
        }
        out
    }

    /// AutoFormat as you type: turns the URL or e-mail address that ends at
    /// `p` into a link (one undo step of its own). Returns its extent.
    pub fn autolink(&mut self, p: Pos) -> Option<Selection> {
        if p.para >= self.paragraphs.len() {
            return None;
        }
        let p = self.clamp(p);
        let t = &self.paragraphs[p.para].text;
        let start = t[..p.offset].rfind(char::is_whitespace).map_or(0, |i| i + t[i..].chars().next().map_or(1, char::len_utf8));
        let word = t[start..p.offset].trim_start_matches(['(', '<', '"', '\'', '«', '[']);
        let start = p.offset - word.len();
        let word = word.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '>', '"', '\'', '»', ']']);
        let target = autolink_target(word)?;
        let sel = Selection { anchor: Pos::new(p.para, start), cursor: Pos::new(p.para, start + word.len()) };
        if !self.range_all(sel, |a| a.link.is_none()) {
            return None;
        }
        self.set_link(sel, Some(Arc::from(target.as_str())), None);
        Some(sel)
    }

    // --- bookmarks -------------------------------------------------------------

    /// All bookmarks (hidden ones included), in document order.
    pub fn bookmarks(&self) -> Vec<Bookmark> {
        collect_bookmarks(&self.paragraphs)
    }

    /// Looks a bookmark up by name (case-insensitive).
    pub fn bookmark(&self, name: &str) -> Option<Bookmark> {
        let n = name.to_lowercase();
        self.bookmarks().into_iter().find(|b| b.name.to_lowercase() == n)
    }

    /// Adds a bookmark on the selection, moving an existing one with the same
    /// name. Undoable. False if the name is empty.
    pub fn add_bookmark(&mut self, name: &str, sel: Selection) -> bool {
        let name = name.trim();
        if name.is_empty() {
            return false;
        }
        let (a, b) = (self.clamp(sel.start()), self.clamp(sel.end()));
        let old = self.bookmark(name);
        let lo = old.as_ref().map_or(a.para, |o| o.start.para.min(a.para));
        let hi = old.as_ref().map_or(b.para, |o| o.end.para.max(b.para));
        let before = self.paragraphs[lo..=hi].to_vec();
        let key = name.to_lowercase();
        for p in &mut self.paragraphs[lo..=hi] {
            p.marks.retain(|m| m.name.to_lowercase() != key);
        }
        let name: Arc<str> = Arc::from(name);
        self.paragraphs[a.para].marks.push(Mark { name: name.clone(), offset: a.offset, end: false });
        self.paragraphs[b.para].marks.push(Mark { name, offset: b.offset, end: true });
        self.record(lo, before, hi - lo + 1, sel, sel, EditKind::Bookmark);
        true
    }

    /// Deletes a bookmark (not its text). Undoable.
    pub fn remove_bookmark(&mut self, name: &str, sel: Selection) -> bool {
        let Some(old) = self.bookmark(name) else { return false };
        let (lo, hi) = (old.start.para, old.end.para);
        let before = self.paragraphs[lo..=hi].to_vec();
        let key = old.name.to_lowercase();
        for p in &mut self.paragraphs[lo..=hi] {
            p.marks.retain(|m| m.name.to_lowercase() != key);
        }
        self.record(lo, before, hi - lo + 1, sel, sel, EditKind::Bookmark);
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Kinds of the undo steps, most recent first.
    pub fn undo_history(&self) -> Vec<EditKind> {
        self.undo.iter().rev().map(|e| e.kind).collect()
    }

    /// Kinds of the redo steps, next first.
    pub fn redo_history(&self) -> Vec<EditKind> {
        self.redo.iter().rev().map(|e| e.kind).collect()
    }

    /// Replaces the text of the selection through `f`, run by run, keeping
    /// each run's formatting (change case).
    pub fn map_text(&mut self, sel: Selection, f: &dyn Fn(&str) -> String) -> Selection {
        if sel.is_empty() {
            return sel;
        }
        let (a, b) = (sel.start(), sel.end());
        let before = self.paragraphs[a.para..=b.para].to_vec();
        let mut end = b;
        for i in a.para..=b.para {
            let p = &mut self.paragraphs[i];
            let marks = p.marks.clone();
            let s = if i == a.para { a.offset } else { 0 };
            let e = if i == b.para { b.offset } else { p.len() };
            let spans: Vec<(usize, usize, Attrs)> = p.spans().filter(|(rs, re, _)| *re > s && *rs < e).map(|(rs, re, at)| (rs.max(s), re.min(e), at.clone())).collect();
            let mut shift: isize = 0;
            for (rs, re, at) in spans {
                let (rs, re) = ((rs as isize + shift) as usize, (re as isize + shift) as usize);
                let new = f(&p.text[rs..re]);
                shift += new.len() as isize - (re - rs) as isize;
                p.remove(rs, re);
                p.insert_str(rs, &new, at);
            }
            // Case changes keep lengths almost always; keep marks in place.
            p.marks = marks.into_iter().map(|m| Mark { offset: floor_boundary(&p.text, m.offset), ..m }).collect();
            if i == b.para {
                end = Pos::new(i, (e as isize + shift) as usize);
            }
        }
        let after = Selection { anchor: if sel.anchor <= sel.cursor { a } else { end }, cursor: if sel.anchor <= sel.cursor { end } else { a } };
        self.record(a.para, before, b.para - a.para + 1, sel, after, EditKind::Format);
        after
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Undoes the last edit; returns the selection to restore.
    pub fn undo(&mut self) -> Option<Selection> {
        let e = self.undo.pop()?;
        let sel = e.sel_before;
        let inv = self.swap(e);
        self.redo.push(inv);
        Some(sel)
    }

    pub fn redo(&mut self) -> Option<Selection> {
        let e = self.redo.pop()?;
        let sel = e.sel_after;
        let inv = self.swap(e);
        self.undo.push(inv);
        Some(sel)
    }

    /// Applies an edit's paragraph replacement and returns its inverse.
    fn swap(&mut self, e: Edit) -> Edit {
        let restored = e.before.len();
        let removed: Vec<Paragraph> = self.paragraphs.splice(e.start..e.start + e.after_len, e.before).collect();
        let comments = e.comments.map(|c| std::mem::replace(&mut self.comments, c));
        let page = e.page.map(|s| Box::new(self.swap_page_state(*s)));
        self.revision += 1;
        self.modified = true;
        let styles = e.styles.map(|s| Box::new(std::mem::replace(&mut self.styles, *s)));
        let parts = e.parts.map(|p| Box::new(self.swap_parts(*p)));
        Edit { start: e.start, before: removed, after_len: restored, sel_before: e.sel_before, sel_after: e.sel_after, kind: e.kind, merge_closed: true, styles, comments, page, parts }
    }
}

/// Makes the marks consistent (file readers may produce stray or duplicate
/// ones): exactly one start and one end per bookmark, start first.
fn fix_marks(paragraphs: &mut [Paragraph]) {
    if paragraphs.iter().all(|p| p.marks.is_empty()) {
        return;
    }
    for p in paragraphs.iter_mut() {
        for m in &mut p.marks {
            m.offset = floor_boundary(&p.text, m.offset);
        }
    }
    let bms = collect_bookmarks(paragraphs);
    // Comment anchors and editable ranges aren't bookmarks: they stay as
    // they are.
    let mut kept = Vec::new();
    for (i, p) in paragraphs.iter_mut().enumerate() {
        kept.extend(p.marks.drain(..).filter(|m| is_comment_mark(&m.name) || is_perm_mark(&m.name)).map(|m| (i, m)));
    }
    for b in bms {
        paragraphs[b.start.para].marks.push(Mark { name: b.name.clone(), offset: b.start.offset, end: false });
        paragraphs[b.end.para].marks.push(Mark { name: b.name, offset: b.end.offset, end: true });
    }
    for (i, m) in kept {
        paragraphs[i].marks.push(m);
    }
}

/// True if two versions of a paragraph differ only in the text of their
/// notes (so note edits can merge into one undo step).
fn same_notes(a: &Paragraph, b: &Paragraph) -> bool {
    a.text == b.text && a.runs.len() == b.runs.len() && a.runs.iter().zip(&b.runs).all(|(x, y)| x.len == y.len && x.attrs.note.is_some() == y.attrs.note.is_some() && (x.attrs.note.is_some() || x.attrs == y.attrs))
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Lowercases while keeping byte offsets valid (chars whose lowercase has a
/// different UTF-8 length are left unchanged).
fn lower_same_len(s: &str) -> String {
    s.chars()
        .map(|c| {
            let mut l = c.to_lowercase();
            match (l.next(), l.next()) {
                (Some(x), None) if x.len_utf8() == c.len_utf8() => x,
                _ => c,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_east_asian_words() {
        assert_eq!(count_words("two words"), 2);
        assert_eq!(count_words("日本語の文書です。"), 8);
        assert_eq!(count_words("ApexDesk で表示"), 4);
        assert_eq!(count_words("这是 PDF 文档"), 5);
        assert_eq!(count_words("한국어 문서"), 2);
    }

    #[test]
    fn images() {
        let mut d = Document::from_plain_text("ab");
        let data: Arc<[u8]> = Arc::from(&b"png"[..]);
        let sel = d.insert_image(Selection::caret(Pos::new(0, 1)), Image::new(data.clone(), 20.0, 10.0));
        assert_eq!(sel.start(), Pos::new(0, 1));
        assert_eq!(d.paragraphs[0].text, "a\u{FFFC}b");
        // Typing after the picture doesn't make more pictures.
        let end = d.insert_text(Selection::caret(sel.end()), "x", None);
        assert!(d.paragraphs[0].attrs_at(end.offset).image.is_none());
        let (_, img) = d.image_at(Pos::new(0, 1)).unwrap();
        assert_eq!(img.width, 20.0);
        d.set_image(Pos::new(0, 1), Image { width: 40.0, ..(*img).clone() });
        assert_eq!(d.images()[0].1.width, 40.0);
        d.undo();
        assert_eq!(d.images()[0].1.width, 20.0);
        assert!(d.image_at(Pos::new(0, 0)).is_none());
        assert_eq!(data_key(&[1; 10000]), data_key(&vec![1; 10000]));
    }

    #[test]
    fn list_labels() {
        assert_eq!(number_label(0, 0, 3), "3.");
        assert_eq!(number_label(0, 1, 28), "ab)");
        assert_eq!(number_label(0, 2, 4), "iv.");
        assert_eq!(number_label(6, 0, 1994), "MCMXCIV.");
        assert_eq!(number_label(4, 0, 2), "B.");
        assert_eq!(bullet_char(0, 1), '◦');
        assert_eq!(bullet_char(5, 0), '✓');
    }

    #[test]
    fn restart_numbering() {
        let n = ParaStyle { kind: ParaKind::Numbered, ..Default::default() };
        let d = Document::from_paragraphs(vec![Paragraph::new("a").with_style(n), Paragraph::new("b").with_style(n), Paragraph::new("c").with_style(ParaStyle { restart: true, ..n }), Paragraph::new("d").with_style(n)]);
        assert_eq!([0, 1, 2, 3].map(|i| d.list_number(i)), [1, 2, 1, 2]);
        assert_eq!(Document::from_paragraphs(vec![Paragraph::new("x\u{2028}y")]).to_plain_text(), "x\ny");
    }

    #[test]
    fn list_numbers_in_one_pass() {
        let n = ParaStyle { kind: ParaKind::Numbered, ..Default::default() };
        let styles = [
            n,
            ParaStyle { level: 1, ..n },
            ParaStyle { kind: ParaKind::Bullet, level: 2, ..n },
            ParaStyle { level: 1, ..n },
            n,
            ParaStyle { start_at: 7, ..n },
            n,
            ParaStyle { kind: ParaKind::Bullet, ..n },
            n,
            ParaStyle::default(),
            ParaStyle { level: 1, ..n },
            ParaStyle { restart: true, ..n },
            n,
        ];
        let d = Document::from_paragraphs(styles.iter().map(|s| Paragraph::new("x").with_style(*s)).collect());
        let each: Vec<usize> = (0..styles.len()).map(|i| if styles[i].kind == ParaKind::Numbered { d.list_number(i) } else { 0 }).collect();
        assert_eq!(d.list_numbers(), each);
        assert_eq!(each, [1, 1, 0, 2, 2, 7, 8, 0, 1, 0, 1, 1, 2]);
    }

    fn doc(texts: &[&str]) -> Document {
        Document::from_paragraphs(texts.iter().map(|t| Paragraph::new(*t)).collect())
    }

    fn range(a: (usize, usize), b: (usize, usize)) -> Selection {
        Selection { anchor: Pos::new(a.0, a.1), cursor: Pos::new(b.0, b.1) }
    }

    fn span(d: &Document, name: &str) -> ((usize, usize), (usize, usize)) {
        let b = d.bookmark(name).unwrap();
        ((b.start.para, b.start.offset), (b.end.para, b.end.offset))
    }

    #[test]
    fn bookmark_names() {
        assert!(valid_bookmark_name("Intro_2"));
        assert!(valid_bookmark_name("Año"));
        assert!(!valid_bookmark_name("2intro"));
        assert!(!valid_bookmark_name("_Toc123"));
        assert!(!valid_bookmark_name("a b"));
        assert!(!valid_bookmark_name(""));
        assert!(!valid_bookmark_name(&"a".repeat(41)));
    }

    #[test]
    fn bookmarks_follow_edits() {
        let mut d = doc(&["hello world", "second line", "third"]);
        assert!(d.add_bookmark("W", range((0, 6), (0, 11))));
        assert!(d.add_bookmark("P", Selection::caret(Pos::new(2, 2))));
        assert_eq!(span(&d, "w"), ((0, 6), (0, 11)));
        // Typing before moves it; typing at its end doesn't grow it.
        d.insert_text(Selection::caret(Pos::new(0, 0)), "Oh, ", None);
        assert_eq!(span(&d, "W"), ((0, 10), (0, 15)));
        d.insert_text(Selection::caret(Pos::new(0, 15)), "!", None);
        assert_eq!(span(&d, "W"), ((0, 10), (0, 15)));
        // Typing inside grows it.
        d.insert_text(Selection::caret(Pos::new(0, 12)), "XX", None);
        assert_eq!(span(&d, "W"), ((0, 10), (0, 17)));
        // Enter before it moves it to the new paragraph; paragraphs below shift.
        d.new_paragraph(Selection::caret(Pos::new(0, 4)));
        assert_eq!(span(&d, "W"), ((1, 6), (1, 13)));
        assert_eq!(span(&d, "P"), ((3, 2), (3, 2)));
        // Joining paragraphs carries the marks along.
        d.backspace(Selection::caret(Pos::new(3, 0)), false);
        assert_eq!(span(&d, "P"), ((2, 13), (2, 13)));
        // Deleting across it collapses it to the deletion point.
        d.delete(range((1, 8), (2, 3)));
        assert_eq!(span(&d, "W"), ((1, 6), (1, 8)));
        assert_eq!(d.bookmarks().len(), 2);
        // Undo restores the positions.
        d.undo();
        assert_eq!(span(&d, "W"), ((1, 6), (1, 13)));
        d.undo();
        assert_eq!(span(&d, "P"), ((3, 2), (3, 2)));
        // Copying and pasting doesn't duplicate bookmarks.
        let clip = d.slice(Pos::new(0, 0), d.end());
        d.insert_paragraphs(Selection::caret(d.end()), &clip);
        assert_eq!(d.bookmarks().len(), 2);
    }

    #[test]
    fn bookmark_add_move_remove() {
        let mut d = doc(&["abc", "def"]);
        d.add_bookmark("B", range((0, 1), (1, 2)));
        assert_eq!(span(&d, "B"), ((0, 1), (1, 2)));
        // Adding an existing name moves it.
        d.add_bookmark("b", Selection::caret(Pos::new(1, 0)));
        assert_eq!(d.bookmarks().len(), 1);
        assert_eq!(span(&d, "B"), ((1, 0), (1, 0)));
        assert_eq!(d.undo_history()[0], EditKind::Bookmark);
        d.undo();
        assert_eq!(span(&d, "B"), ((0, 1), (1, 2)));
        assert!(d.remove_bookmark("B", Selection::default()));
        assert!(d.bookmarks().is_empty());
        d.undo();
        assert_eq!(d.bookmarks().len(), 1);
        // Stray marks from files are repaired.
        let mut p = Paragraph::new("xyz");
        p.marks.push(Mark { name: "E".into(), offset: 2, end: true });
        p.marks.push(Mark { name: "E".into(), offset: 1, end: true });
        p.marks.push(Mark { name: "_Toc1".into(), offset: 9, end: false });
        let d = Document::from_paragraphs(vec![p]);
        let b = d.bookmarks();
        assert_eq!(b.len(), 2);
        assert_eq!((b[0].start.offset, b[0].end.offset), (2, 2));
        assert!(b[1].is_hidden() && b[1].end.offset == 3);
    }

    #[test]
    fn links() {
        let mut d = doc(&["see here now"]);
        d.set_link(range((0, 4), (0, 8)), Some("https://a.b".into()), Some("tip".into()));
        let (sel, url, tip) = d.link_at(Pos::new(0, 6)).unwrap();
        assert_eq!((sel.start().offset, sel.end().offset, &*url, tip.as_deref()), (4, 8, "https://a.b", Some("tip")));
        assert!(d.link_at(Pos::new(0, 8)).is_some());
        assert!(d.link_at(Pos::new(0, 9)).is_none());
        // Typing after a link doesn't extend it; typing inside does.
        d.insert_text(Selection::caret(Pos::new(0, 8)), "X", None);
        assert!(d.paragraphs[0].attrs_at(9).link.is_none());
        d.insert_text(Selection::caret(Pos::new(0, 6)), "Y", None);
        assert_eq!(d.link_at(Pos::new(0, 4)).unwrap().0.end().offset, 9);
        // Insert with new text, then remove.
        let s = d.insert_link(Selection::caret(Pos::new(0, 0)), "web", "#B".into(), None);
        assert_eq!((s.start().offset, s.end().offset), (0, 3));
        assert_eq!(d.undo_history()[0], EditKind::Link);
        d.set_link(s, None, None);
        assert!(d.link_at(Pos::new(0, 1)).is_none());
    }

    #[test]
    fn autolinks() {
        assert_eq!(autolink_target("www.example.com").as_deref(), Some("http://www.example.com"));
        assert_eq!(autolink_target("ana@x.es").as_deref(), Some("mailto:ana@x.es"));
        assert_eq!(autolink_target("https://x.y/z?q=1").as_deref(), Some("https://x.y/z?q=1"));
        assert!(autolink_target("hello").is_none());
        assert!(autolink_target("a@b").is_none());
        assert!(autolink_target("http://").is_none());
        let mut d = doc(&["go to (www.rust-lang.org). "]);
        let s = d.autolink(Pos::new(0, 26)).unwrap();
        assert_eq!(d.text_range(s.start(), s.end()), "www.rust-lang.org");
        assert!(d.autolink(Pos::new(0, 26)).is_none());
        // One undo removes just the link.
        d.undo();
        assert!(d.link_at(Pos::new(0, 10)).is_none());
        assert_eq!(d.paragraphs[0].text, "go to (www.rust-lang.org). ");
    }

    #[test]
    fn positions_inside_a_letter_or_past_the_end_are_safe() {
        let mut d = Document::from_plain_text("ñandú");
        // Byte 1 is inside "ñ": the text goes before it.
        d.insert_text(Selection::caret(Pos::new(0, 1)), "Ω", None);
        assert_eq!(d.paragraphs[0].text, "Ωñandú");
        let (a, b) = d.word_at(Pos::new(0, 99));
        assert_eq!((a.offset, b.offset), (0, d.paragraphs[0].len()));
    }

}
