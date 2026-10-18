//! Characters of the symbol fonts (Symbol, Wingdings) as Unicode, for
//! text that names one by code (`w:sym`): the fonts themselves are rarely
//! installed.

/// The Unicode character for code `code` of font `font` (the code as Word
/// keeps it, from U+F000 for a symbol font); `None` for a control code.
pub(crate) fn to_unicode(font: &str, code: u32) -> Option<char> {
    let f = font.trim().to_ascii_lowercase();
    let low = if (0xF000..=0xF0FF).contains(&code) { code - 0xF000 } else { code };
    if f == "symbol" && low < 0x100 {
        return (low >= 0x20).then(|| symbol(low as u8));
    }
    if f.starts_with("wingdings") && low < 0x100 {
        return (low >= 0x20).then(|| if f.ends_with('2') { wingdings2(low as u8) } else { wingdings(low as u8) });
    }
    if f.starts_with("webdings") && low < 0x100 {
        return (low >= 0x20).then_some('•');
    }
    char::from_u32(code).filter(|c| !c.is_control())
}

fn symbol(b: u8) -> char {
    const GREEK_UP: &str = "ΑΒΧΔΕΦΓΗΙϑΚΛΜΝΟΠΘΡΣΤΥςΩΞΨΖ";
    const GREEK_LOW: &str = "αβχδεφγηιϕκλμνοπθρστυϖωξψζ";
    match b {
        b'A'..=b'Z' => GREEK_UP.chars().nth((b - b'A') as usize).unwrap_or('?'),
        b'a'..=b'z' => GREEK_LOW.chars().nth((b - b'a') as usize).unwrap_or('?'),
        0x22 => '∀',
        0x24 => '∃',
        0x27 => '∋',
        0x2A => '∗',
        0x2D => '−',
        0x40 => '≅',
        0x5C => '∴',
        0x5E => '⊥',
        0xA2 => '′',
        0xA3 => '≤',
        0xA5 => '∞',
        0xA6 => 'ƒ',
        0xA7 => '♣',
        0xA8 => '♦',
        0xA9 => '♥',
        0xAA => '♠',
        0xAB => '↔',
        0xAC => '←',
        0xAD => '↑',
        0xAE => '→',
        0xAF => '↓',
        0xB0 => '°',
        0xB1 => '±',
        0xB2 => '″',
        0xB3 => '≥',
        0xB4 => '×',
        0xB5 => '∝',
        0xB6 => '∂',
        0xB7 => '•',
        0xB8 => '÷',
        0xB9 => '≠',
        0xBA => '≡',
        0xBB => '≈',
        0xBC => '…',
        0xC5 => '⊕',
        0xC6 => '∅',
        0xC7 => '∩',
        0xC8 => '∪',
        0xCE => '∈',
        0xCF => '∉',
        0xD1 => '∇',
        0xD2 | 0xE2 => '®',
        0xD3 | 0xE3 => '©',
        0xD4 | 0xE4 => '™',
        0xD6 => '√',
        0xD7 => '⋅',
        0xD8 => '¬',
        0xD9 => '∧',
        0xDA => '∨',
        0xDB => '⇔',
        0xDE => '⇒',
        0xE5 => '∑',
        0xF2 => '∫',
        0x20..=0x7E => b as char,
        _ => '•',
    }
}

fn wingdings(b: u8) -> char {
    match b {
        0x20 => ' ',
        0x28 => '☎',
        0x45 => '☜',
        0x46 => '☞',
        0x47 => '☝',
        0x48 => '☟',
        0x4A => '☺',
        0x4C => '☹',
        0x4E => '☠',
        0x51 => '✈',
        0x52 => '☼',
        0x54 => '❄',
        0x58 => '✠',
        0x59 => '✡',
        0x5B => '☯',
        0x5D => '☸',
        0x6C => '●',
        0x6D => '❍',
        0x6E => '■',
        0x6F | 0x70 | 0xA8 => '□',
        0x71 => '❑',
        0x72 => '❒',
        0x73 | 0x77 => '◆',
        0x74 => '⧫',
        0x75 => '◆',
        0x76 => '❖',
        0x78 => '⌧',
        0x7A => '⌘',
        0x80 => '⓪',
        0x81..=0x8A => char::from_u32(0x2460 + u32::from(b - 0x81)).unwrap_or('•'),
        0x9E => '·',
        0x9F => '•',
        0xA1 | 0xA2 | 0xA3 => '○',
        0xA4 => '◉',
        0xA5 => '◎',
        0xA7 => '▪',
        0xAB => '★',
        0xD8 | 0xE0 => '➢',
        0xE8 | 0xF0 => '➔',
        0xEF => '⇦',
        0xF1 => '⇧',
        0xF2 => '⇩',
        0xFB => '✗',
        0xFC => '✓',
        0xFD => '☒',
        0xFE => '☑',
        _ => '•',
    }
}

fn wingdings2(b: u8) -> char {
    match b {
        0x20 => ' ',
        0x4F => '✗',
        0x50 => '✓',
        0x51 | 0x53 | 0x54 => '☒',
        0x52 => '☑',
        0xA3 => '□',
        0x97..=0x9A => '●',
        _ => '•',
    }
}

#[cfg(test)]
mod tests {
    use super::to_unicode;

    #[test]
    fn symbol_fonts_map_to_unicode() {
        assert_eq!(to_unicode("Wingdings", 0xF0A3), Some('○'));
        assert_eq!(to_unicode("Wingdings", 0xF0FC), Some('✓'));
        assert_eq!(to_unicode("Symbol", 0xF061), Some('α'));
        assert_eq!(to_unicode("Symbol", 0xB3), Some('≥'));
        assert_eq!(to_unicode("Arial", 0x263A), Some('☺'));
        assert_eq!(to_unicode("Wingdings", 0xF009), None);
    }
}
