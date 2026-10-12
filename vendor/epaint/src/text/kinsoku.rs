//! ApexDesk patch: East Asian line breaking (kinsoku shori).
//!
//! Lines may break between any two ideographs or kana, and between them and
//! other text, but no line starts with closing punctuation, small kana or
//! the prolonged sound mark, and none ends with an opening bracket. The
//! lists follow Word's default Japanese rules, with Chinese punctuation
//! (they match `apex_doc::asian`).

/// Ideographs, kana, bopomofo and full width punctuation: characters a
/// line may break before or after without a space. Hangul is left out:
/// Korean breaks at spaces.
pub(crate) fn breaks_freely(c: char) -> bool {
    matches!(c as u32,
        0x2E80..=0x2FDF
        | 0x2FF0..=0x303F
        | 0x3040..=0x312F
        | 0x3190..=0x31FF
        | 0x3200..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xF900..=0xFAFF
        | 0xFE10..=0xFE1F
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFF9F
        | 0xFFE0..=0xFFEF
        | 0x1B000..=0x1B16F
        | 0x20000..=0x3134F
    )
}

/// No line may start with these.
pub(crate) fn no_line_start(c: char) -> bool {
    "!%),.:;?]}¢°’”‰′″℃、。々〉》」』】〕〗〙〛゛゜ゝゞ・ヽヾ！％），．：；？］｝｡｣､･ﾞﾟ￠ぁぃぅぇぉっゃゅょゎゕゖァィゥェォッャュョヮヵヶㇰㇱㇲㇳㇴㇵㇶㇷㇸㇹㇺㇻㇼㇽㇾㇿｧｨｩｪｫｬｭｮｯｰー‐゠–〜～…‥〻〟»›︐︑︒︓︔︕︖︶︸︺︼︾﹀﹂﹄﹈︘"
        .contains(c)
}

/// No line may end with these.
pub(crate) fn no_line_end(c: char) -> bool {
    "$([{£¥‘“〈《「『【〔〖〘〚〝＄（［｛｢￡￥«‹︵︷︹︻︽︿﹁﹃﹇︗".contains(c)
}

/// Punctuation that may hang past the right edge
/// ([`crate::text::TextWrapping::hanging_punctuation`]).
pub(crate) fn hangs(c: char) -> bool {
    matches!(c, '、' | '。' | '，' | '．' | '､' | '｡' | '︑' | '︒' | '︐')
}

/// Middle of the ideographic em box above the baseline, as a share of
/// the font size (the box goes from 0.12 below it to 0.88 above).
pub const EM_BOX_MIDDLE: f32 = 0.38;

/// Characters that stand upright in vertical text: ideographs, kana,
/// hangul, full width forms and the vertical forms of punctuation. The
/// rest (Latin letters, digits, the prolonged sound mark, dashes and
/// tildes) lie on their side.
pub fn upright_in_vertical(c: char) -> bool {
    let u = c as u32;
    let cjk = matches!(u,
        0x1100..=0x11FF
        | 0x2E80..=0x2FDF
        | 0x2FF0..=0x303F
        | 0x3040..=0x31FF
        | 0x3200..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7FF
        | 0xF900..=0xFAFF
        | 0xFE10..=0xFE1F
        | 0xFE30..=0xFE4F
        | 0xFF01..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1B000..=0x1B16F
        | 0x20000..=0x3134F
    ) || matches!(c, '※' | '§' | '°' | '±' | '×' | '÷' | '■' | '□' | '▲' | '△' | '○' | '●' | '◎' | '★' | '☆');
    cjk && !matches!(c, 'ー' | '〜' | '～' | '＝' | '－' | '〰' | '…' | '‥' | '（' | '）' | '［' | '］' | '｛' | '｝' | '＜' | '＞' | '＿' | '｜' | '→' | '←')
}

/// Whether a line may break between `a` and `b` with the rules on.
pub(crate) fn allowed(a: char, b: char) -> bool {
    !no_line_end(a) && !no_line_start(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules() {
        assert!(breaks_freely('漢') && breaks_freely('か') && breaks_freely('。') && !breaks_freely('한') && !breaks_freely('a'));
        assert!(!allowed('漢', '。') && !allowed('「', '漢') && allowed('。', '漢') && allowed('漢', '字'));
        assert!(!allowed('漢', 'ー') && !allowed('か', 'っ'));
        assert!(!allowed('漢', '︒') && !allowed('﹁', '漢'));
        assert!(upright_in_vertical('漢') && upright_in_vertical('︒') && upright_in_vertical('Ａ'));
        assert!(!upright_in_vertical('a') && !upright_in_vertical('ー') && !upright_in_vertical('1'));
    }
}
