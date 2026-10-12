//! East Asian typography: a font of its own for Chinese, Japanese and
//! Korean text, the Asian line breaking and spacing rules of a paragraph,
//! the document grid of a section, and what files have that we keep for
//! the next save without showing it yet (phonetic guides, emphasis marks,
//! combined characters, fitted text, vertical text).

use std::sync::Arc;

use crate::{Attrs, Document, ParaStyle};

/// Emphasis mark over or under each character (Word's `w:em`, ODF's
/// `style:text-emphasize`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Emphasis {
    Dot,
    /// Word's "comma"; ODF's "accent".
    Comma,
    Circle,
    /// A filled circle (ODF only; Word shows a dot).
    Disc,
    /// A dot under the character.
    UnderDot,
}

/// Characters laid out together (Word's `w:eastAsianLayout`, ODF's
/// `style:text-combine` and character rotation).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct Combine {
    /// Id shared by the runs of one combined group.
    pub id: Option<i32>,
    /// Two lines in one.
    pub lines: bool,
    /// Characters combined in one cell (ODF's "letters").
    pub letters: bool,
    /// Brackets around two lines in one: "round", "square", "angle",
    /// "curly" (Word) or the two characters (ODF).
    pub brackets: Option<Arc<str>>,
    /// Horizontal text in vertical text.
    pub vert: bool,
    /// ...compressed to the line height.
    pub vert_compress: bool,
}

/// A phonetic guide (ruby, furigana) over the characters that carry it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct Ruby {
    /// The guide text.
    pub text: Arc<str>,
    /// Alignment: Word's `w:rubyAlign` value ("center", "distributeSpace"...).
    pub align: Option<Arc<str>>,
    /// Guide font size and base text size in half points, and how far the
    /// guide is raised (half points).
    pub size: Option<u16>,
    pub base_size: Option<u16>,
    pub raise: Option<i32>,
    /// Language id of the guide (`w:lid`).
    pub lang: Option<Arc<str>>,
    /// Under the text instead of over it (ODF's `style:ruby-position`).
    pub below: bool,
    /// The guide run's own properties as written in docx (`w:rPr`).
    pub docx_rpr: Option<Arc<str>>,
    /// Tells apart two adjacent guides that are otherwise equal.
    pub seq: u32,
}

/// East Asian and complex script formatting of some text.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct AsianAttrs {
    /// Font of Chinese, Japanese and Korean characters (Word's
    /// `w:eastAsia`, ODF's `style:font-name-asian`).
    pub font: Option<Arc<str>>,
    /// Font of complex scripts (Arabic, Hebrew, Thai...: `w:cs`,
    /// `style:font-name-complex`).
    pub cs_font: Option<Arc<str>>,
    /// Characters both fonts have (quotes, dashes) take the Asian font
    /// (`w:hint="eastAsia"`).
    pub hint: bool,
    /// Language of Asian text ("ja-JP") and of complex scripts.
    pub lang: Option<Arc<str>>,
    pub cs_lang: Option<Arc<str>>,
    /// Asian and complex script font sizes in half points, when they
    /// differ from the size.
    pub size: Option<u16>,
    pub cs_size: Option<u16>,
    pub emphasis: Option<Emphasis>,
    pub combine: Option<Combine>,
    /// Fit the text in this width (twips) (`w:fitText`), and its id.
    pub fit_text: Option<(u32, i32)>,
    pub ruby: Option<Arc<Ruby>>,
}

impl AsianAttrs {
    pub fn is_empty(&self) -> bool {
        *self == AsianAttrs::default()
    }

    /// Lays `o` over these settings (what it sets wins).
    pub fn overlay(&mut self, o: &AsianAttrs) {
        macro_rules! take {
            ($($f:ident),*) => {$(
                if o.$f.is_some() {
                    self.$f.clone_from(&o.$f);
                }
            )*};
        }
        take!(font, cs_font, lang, cs_lang, size, cs_size, emphasis, combine, fit_text, ruby);
        self.hint |= o.hint;
    }
}

impl Attrs {
    /// The Asian settings (defaults when none are set).
    pub fn asian(&self) -> AsianAttrs {
        self.asian.as_deref().cloned().unwrap_or_default()
    }

    /// Changes the Asian settings; defaults are stored as none.
    pub fn set_asian(&mut self, f: impl FnOnce(&mut AsianAttrs)) {
        let mut a = self.asian();
        f(&mut a);
        self.asian = (!a.is_empty()).then(|| Arc::new(a));
    }

    /// The Asian font set on the text.
    pub fn asian_font(&self) -> Option<&Arc<str>> {
        self.asian.as_ref().and_then(|a| a.font.as_ref())
    }
}

/// Drops what belongs to the characters it was set on (a phonetic guide,
/// fitted text) from text typed at their end (`next`: the attributes of
/// the character after it).
pub(crate) fn typing(a: &mut Attrs, next: Option<&Attrs>) {
    let Some(x) = a.asian.as_deref() else { return };
    let same = |n: &Attrs| n.asian.as_ref().is_some_and(|y| y.ruby == x.ruby && y.fit_text == x.fit_text);
    if (x.ruby.is_some() || x.fit_text.is_some()) && !next.is_some_and(same) {
        a.set_asian(|s| (s.ruby, s.fit_text) = (None, None));
    }
}

/// Font alignment of characters of different sizes on a line (Word's
/// `w:textAlignment`, ODF's `style:vertical-align`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FontAlign {
    Auto,
    Baseline,
    Top,
    Center,
    Bottom,
}

/// Asian typography of a paragraph. `None` is the default (set by the
/// style or the application: all on but `top_line_punct`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct AsianPara {
    /// Line breaking rules: no line starts with closing punctuation or
    /// ends with an opening bracket (`w:kinsoku`, ODF's
    /// `style:line-break="strict"`).
    pub kinsoku: Option<bool>,
    /// Latin words wrap as words (off: anywhere) (`w:wordWrap`).
    pub word_wrap: Option<bool>,
    /// Punctuation may hang past the margin (`w:overflowPunct`,
    /// `style:punctuation-wrap="hanging"`).
    pub overflow_punct: Option<bool>,
    /// Compress punctuation at the start of lines (`w:topLinePunct`).
    pub top_line_punct: Option<bool>,
    /// A little space between Asian text and Latin text, and between Asian
    /// text and numbers (`w:autoSpaceDE`, `w:autoSpaceDN`,
    /// `style:text-autospace`).
    pub auto_space_de: Option<bool>,
    pub auto_space_dn: Option<bool>,
    /// Lines follow the document grid (`w:snapToGrid`,
    /// `style:snap-to-layout-grid`).
    pub snap_to_grid: Option<bool>,
    pub font_align: Option<FontAlign>,
}

impl AsianPara {
    pub fn kinsoku(&self) -> bool {
        self.kinsoku.unwrap_or(true)
    }

    pub fn word_wrap(&self) -> bool {
        self.word_wrap.unwrap_or(true)
    }

    pub fn overflow_punct(&self) -> bool {
        self.overflow_punct.unwrap_or(true)
    }

    pub fn auto_space_de(&self) -> bool {
        self.auto_space_de.unwrap_or(true)
    }

    pub fn auto_space_dn(&self) -> bool {
        self.auto_space_dn.unwrap_or(true)
    }

    pub fn snap_to_grid(&self) -> bool {
        self.snap_to_grid.unwrap_or(true)
    }

    pub fn is_default(&self) -> bool {
        *self == AsianPara::default()
    }

    /// Lays `o` over these settings (what it sets wins).
    pub fn overlay(&mut self, o: &AsianPara) {
        macro_rules! take {
            ($($f:ident),*) => {$(
                if o.$f.is_some() {
                    self.$f = o.$f;
                }
            )*};
        }
        take!(kinsoku, word_wrap, overflow_punct, top_line_punct, auto_space_de, auto_space_dn, snap_to_grid, font_align);
    }
}

/// What the document grid of a section does (Word's `w:docGrid w:type`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum GridKind {
    /// No grid.
    #[default]
    None,
    /// Lines snap to the line pitch.
    Lines,
    /// Lines and Asian characters snap to the grid.
    LinesAndChars,
    /// Characters snap to the character pitch too, whatever their size.
    SnapToChars,
}

impl GridKind {
    pub const ALL: [GridKind; 4] = [GridKind::None, GridKind::Lines, GridKind::LinesAndChars, GridKind::SnapToChars];

    pub fn lines(self) -> bool {
        self != GridKind::None
    }

    pub fn chars(self) -> bool {
        matches!(self, GridKind::LinesAndChars | GridKind::SnapToChars)
    }
}

/// The document grid of a section: the pitch of lines and characters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DocGrid {
    pub kind: GridKind,
    /// Line pitch in points (0: none given).
    pub line_pitch: f32,
    /// Character pitch as Word stores it: 1/4096 points added to the size
    /// of the Normal style's font.
    pub char_space: i32,
    /// ODF extras: the ruby line height (points), grid color, and whether
    /// the grid is shown and printed.
    pub ruby_height: f32,
    pub color: Option<u32>,
    pub display: bool,
    pub print: bool,
}

impl Default for DocGrid {
    fn default() -> Self {
        DocGrid { kind: GridKind::None, line_pitch: 0.0, char_space: 0, ruby_height: 0.0, color: None, display: false, print: false }
    }
}

impl DocGrid {
    /// Character pitch in points for a Normal font of `base` points.
    pub fn char_pitch(&self, base: f32) -> f32 {
        (base + self.char_space as f32 / 4096.0).max(1.0)
    }

    /// `char_space` that gives a pitch of `pitch` points.
    pub fn space_for(pitch: f32, base: f32) -> i32 {
        ((pitch - base) * 4096.0).round() as i32
    }

    /// The line pitch in use (18 pt when none was given, as in Word).
    pub fn pitch(&self) -> f32 {
        if self.line_pitch > 0.5 { self.line_pitch } else { 18.0 }
    }

    /// Lines per page and characters per line for a text area `w` × `h`
    /// points.
    pub fn counts(&self, w: f32, h: f32, base: f32) -> (u32, u32) {
        let lines = (h / self.pitch()).floor().max(1.0) as u32;
        let chars = (w / self.char_pitch(base)).floor().max(1.0) as u32;
        (lines, chars)
    }
}

/// Direction of text in a section, cell or text box (Word's
/// `w:textDirection`, ODF's `style:writing-mode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextFlow {
    /// Left to right, top to bottom (normal).
    LrTb,
    /// Vertical, lines from right to left (tategaki).
    TbRl,
    /// Turned: bottom to top, lines left to right.
    BtLr,
    /// Horizontal with Asian characters turned.
    LrTbV,
    /// Vertical with Latin text turned.
    TbRlV,
    TbLrV,
}

impl TextFlow {
    pub fn word(self) -> &'static str {
        match self {
            TextFlow::LrTb => "lrTb",
            TextFlow::TbRl => "tbRl",
            TextFlow::BtLr => "btLr",
            TextFlow::LrTbV => "lrTbV",
            TextFlow::TbRlV => "tbRlV",
            TextFlow::TbLrV => "tbLrV",
        }
    }

    /// Word's value (and the older names of Word 2007).
    pub fn from_word(v: &str) -> Option<TextFlow> {
        Some(match v {
            "lrTb" | "tb" => TextFlow::LrTb,
            "tbRl" | "rl" => TextFlow::TbRl,
            "btLr" | "lr" => TextFlow::BtLr,
            "lrTbV" | "tbV" => TextFlow::LrTbV,
            "tbRlV" | "rlV" => TextFlow::TbRlV,
            "tbLrV" | "lrV" => TextFlow::TbLrV,
            _ => return None,
        })
    }

    pub fn odf(self) -> &'static str {
        match self {
            TextFlow::LrTb | TextFlow::LrTbV => "lr-tb",
            TextFlow::TbRl | TextFlow::TbRlV => "tb-rl",
            TextFlow::BtLr => "bt-lr",
            TextFlow::TbLrV => "tb-lr",
        }
    }

    pub fn from_odf(v: &str) -> Option<TextFlow> {
        Some(match v {
            "lr-tb" | "lr" => TextFlow::LrTb,
            "tb-rl" | "tb" => TextFlow::TbRl,
            "bt-lr" => TextFlow::BtLr,
            "tb-lr" => TextFlow::TbLrV,
            _ => return None,
        })
    }

    pub fn is_vertical(self) -> bool {
        !matches!(self, TextFlow::LrTb | TextFlow::LrTbV)
    }

    /// How a table cell shows it (turned text).
    pub fn cell_dir(self) -> crate::TextDir {
        match self {
            TextFlow::LrTb | TextFlow::LrTbV => crate::TextDir::Horizontal,
            TextFlow::BtLr => crate::TextDir::Up,
            TextFlow::TbRl | TextFlow::TbRlV | TextFlow::TbLrV => crate::TextDir::Down,
        }
    }

    /// The file's flow of a cell, while it agrees with its direction.
    pub fn of_cell(c: &crate::CellProps) -> Option<TextFlow> {
        c.flow.filter(|f| f.cell_dir() == c.text_dir)
    }
}

/// Asian formatting of the document defaults (Word's docDefaults, the
/// ODF default style).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct AsianDefaults {
    pub text: Option<Arc<AsianAttrs>>,
    pub para: AsianPara,
}

impl Document {
    /// The Asian typography of a paragraph: the document defaults, then
    /// its styles, then its own settings.
    pub fn asian_para(&self, st: &ParaStyle) -> AsianPara {
        let mut a = self.styles.asian.para;
        if st.kind != crate::ParaKind::Normal {
            if let Some(d) = self.styles.builtin(crate::ParaKind::Normal) {
                a.overlay(&d.asian_para);
            }
        }
        if let Some(d) = self.styles.builtin(st.kind) {
            a.overlay(&d.asian_para);
        }
        if let Some(d) = self.styles.custom(st.custom) {
            a.overlay(&d.asian_para);
        }
        a.overlay(&st.asian);
        a
    }

    /// Size of the Normal style's font in points (the base of the
    /// document grid's character pitch).
    pub fn normal_size_pt(&self) -> f32 {
        self.base_size(crate::ParaKind::Normal) as f32 / 2.0
    }
}

// --- characters ----------------------------------------------------------------------

/// Chinese, Japanese and Korean characters: ideographs, kana, hangul,
/// bopomofo, CJK punctuation and symbols, full width forms. They take
/// the Asian font.
pub fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x11FF // hangul jamo
        | 0x2E80..=0x2FDF // radicals
        | 0x2FF0..=0x303F // ideographic description, CJK symbols and punctuation
        | 0x3040..=0x31FF // kana, bopomofo, hangul compatibility jamo, kanbun, strokes, katakana ext
        | 0x3200..=0x33FF // enclosed CJK, compatibility
        | 0x3400..=0x4DBF // extension A
        | 0x4E00..=0x9FFF // unified ideographs
        | 0xA960..=0xA97F // hangul jamo extended A
        | 0xAC00..=0xD7FF // hangul syllables, jamo extended B
        | 0xF900..=0xFAFF // compatibility ideographs
        | 0xFE10..=0xFE1F // vertical forms
        | 0xFE30..=0xFE4F // compatibility forms
        | 0xFF00..=0xFFEF // half and full width forms
        | 0x1B000..=0x1B16F // kana supplement and extensions
        | 0x20000..=0x3134F // extensions B to G
    )
}

/// Characters that are Latin or Asian depending on the font the text asks
/// for (`w:hint="eastAsia"`): quotes, dashes, ellipsis, some symbols.
pub fn is_ambiguous(c: char) -> bool {
    matches!(c, '\u{2018}' | '\u{2019}' | '\u{201C}' | '\u{201D}' | '\u{2014}' | '\u{2015}' | '\u{2025}' | '\u{2026}' | '\u{00A7}' | '\u{00B0}' | '\u{00B1}' | '\u{00D7}' | '\u{00F7}' | '\u{2032}' | '\u{2033}' | '\u{203B}' | '\u{2103}' | '\u{2116}' | '\u{2192}' | '\u{2190}' | '\u{2191}' | '\u{2193}' | '\u{25A0}' | '\u{25A1}' | '\u{25B2}' | '\u{25B3}' | '\u{25CB}' | '\u{25CF}' | '\u{25CE}' | '\u{2605}' | '\u{2606}')
}

/// Whether a character is set in the Asian font of text with `hint`.
pub fn uses_asian_font(c: char, hint: bool) -> bool {
    is_cjk(c) || (hint && is_ambiguous(c))
}

/// Ideographs and kana: characters between which lines may break.
pub fn is_ideographic(c: char) -> bool {
    is_cjk(c) && !matches!(c as u32, 0x1100..=0x11FF | 0x3130..=0x318F | 0xA960..=0xA97F | 0xAC00..=0xD7FF | 0xFFA0..=0xFFDC | 0xFF01..=0xFF5E)
}

/// No line may start with these (closing brackets, punctuation, small
/// kana, the prolonged sound mark): Word's default Japanese list, with
/// Chinese and Korean punctuation.
pub fn no_line_start(c: char) -> bool {
    "!%),.:;?]}¢°’”‰′″℃、。々〉》」』】〕〗〙〛゛゜ゝゞ・ヽヾ！％），．：；？］｝｡｣､･ﾞﾟ￠ぁぃぅぇぉっゃゅょゎゕゖァィゥェォッャュョヮヵヶㇰㇱㇲㇳㇴㇵㇶㇷㇸㇹㇺㇻㇼㇽㇾㇿｧｨｩｪｫｬｭｮｯｰー‐゠–〜～…‥〻〟»›︐︑︒︓︔︕︖︶︸︺︼︾﹀﹂﹄﹈︘"
        .contains(c)
}

/// No line may end with these (opening brackets, currency signs).
pub fn no_line_end(c: char) -> bool {
    "$([{£¥‘“〈《「『【〔〖〘〚〝＄（［｛｢￡￥«‹︵︷︹︻︽︿﹁﹃﹇︗".contains(c)
}

/// Punctuation that may hang past the right margin.
pub fn hangs(c: char) -> bool {
    matches!(c, '、' | '。' | '，' | '．' | '､' | '｡')
}

/// Punctuation and its vertical form (Unicode's vertical forms and CJK
/// compatibility forms), as fonts show them in vertical text.
const VERTICAL_FORMS: [(char, char); 33] = [
    ('、', '︑'), ('。', '︒'), ('，', '︐'), ('：', '︓'), ('；', '︔'), ('！', '︕'), ('？', '︖'),
    ('〖', '︗'), ('〗', '︘'), ('…', '︙'), ('‥', '︰'), ('—', '︱'), ('―', '︱'), ('–', '︲'),
    ('（', '︵'), ('）', '︶'), ('｛', '︷'), ('｝', '︸'), ('〔', '︹'), ('〕', '︺'), ('【', '︻'),
    ('】', '︼'), ('《', '︽'), ('》', '︾'), ('〈', '︿'), ('〉', '﹀'), ('「', '﹁'), ('」', '﹂'),
    ('『', '﹃'), ('』', '﹄'), ('［', '﹇'), ('］', '﹈'), ('＿', '︳'),
];

/// The form of a character in vertical text (itself when it has none).
pub fn vertical_form(c: char) -> char {
    VERTICAL_FORMS.iter().find(|v| v.0 == c).map_or(c, |v| v.1)
}

/// Text with the vertical forms of its punctuation. Every form takes as
/// many bytes as its character, so offsets stay.
pub fn to_vertical(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.chars().any(|c| vertical_form(c) != c) {
        return std::borrow::Cow::Borrowed(text);
    }
    std::borrow::Cow::Owned(text.chars().map(vertical_form).collect())
}

/// Kind of character for the automatic spacing between Asian and other
/// text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    Asian,
    Latin,
    Digit,
    /// Spaces, punctuation and symbols: never spaced.
    Other,
}

pub fn script_of(c: char) -> Script {
    if c.is_ascii_digit() {
        Script::Digit
    } else if is_ideographic(c) && !no_line_start(c) && !no_line_end(c) && !matches!(c as u32, 0x3000..=0x303F | 0xFF00..=0xFFEF) {
        Script::Asian
    } else if c.is_alphabetic() && !is_cjk(c) {
        Script::Latin
    } else {
        Script::Other
    }
}

/// Whether the automatic spacing goes between `a` and `b`.
pub fn auto_space(a: char, b: char, de: bool, dn: bool) -> bool {
    match (script_of(a), script_of(b)) {
        (Script::Asian, Script::Latin) | (Script::Latin, Script::Asian) => de,
        (Script::Asian, Script::Digit) | (Script::Digit, Script::Asian) => dn,
        _ => false,
    }
}

/// A language tag for Asian text guessed from the characters: kana is
/// Japanese, hangul Korean, other ideographs `fallback`.
pub fn guess_lang(text: &str, fallback: &str) -> String {
    for c in text.chars() {
        match c as u32 {
            0x3040..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F => return "ja".into(),
            0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF => return "ko".into(),
            _ => {}
        }
    }
    fallback.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes() {
        assert!(is_cjk('漢') && is_cjk('か') && is_cjk('한') && is_cjk('。') && is_cjk('Ａ'));
        assert!(!is_cjk('a') && !is_cjk('1'));
        assert!(no_line_start('。') && no_line_start('」') && no_line_start('ー') && no_line_start('っ'));
        assert!(no_line_end('「') && no_line_end('（') && !no_line_end('漢'));
        assert!(auto_space('漢', 'a', true, true) && auto_space('9', 'か', true, true) && !auto_space('9', 'か', true, false));
        assert!(!auto_space('。', 'a', true, true) && !auto_space('a', 'b', true, true));
        assert_eq!(guess_lang("漢字とかな", "zh"), "ja");
        assert_eq!(to_vertical("「縦書き」、です。"), "﹁縦書き﹂︑です︒");
        assert!(VERTICAL_FORMS.iter().all(|(a, b)| a.len_utf8() == b.len_utf8()));
        assert!(no_line_start('︒') && no_line_end('﹁'));
    }

    #[test]
    fn grid_pitch() {
        let g = DocGrid { kind: GridKind::LinesAndChars, line_pitch: 18.0, char_space: DocGrid::space_for(11.0, 10.5), ..Default::default() };
        assert!((g.char_pitch(10.5) - 11.0).abs() < 0.001);
        assert_eq!(g.counts(440.0, 720.0, 10.5), (40, 40));
    }

    #[test]
    fn typing_after_ruby() {
        let r = Arc::new(Ruby { text: "かん".into(), ..Default::default() });
        let mut a = Attrs::default();
        a.set_asian(|s| s.ruby = Some(r.clone()));
        let inside = a.clone();
        typing(&mut a, Some(&inside));
        assert!(a.asian.is_some());
        typing(&mut a, None);
        assert!(a.asian.is_none());
    }
}
