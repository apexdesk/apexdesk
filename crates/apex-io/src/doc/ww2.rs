//! Word for Windows 1.x and 2.0 documents (FIB versions below 101). They
//! aren't compound files: the FIB starts the file and the text follows,
//! 8-bit in the code page of the document's language. Only the text and
//! its paragraphs are read (their formatting uses structures of their
//! own); fields show their results.

use apex_doc::{Document, LINE_BREAK, Paragraph};

use super::{u16_at, u32_at, ww6::lid_cp};
use crate::Error;

/// For Word files that aren't compound files and can't be read.
pub(super) const TOO_OLD: &str = "the Word document is damaged or from a version too old to read (before Word 6.0)";

/// Whether `data` is a Word file without a compound file around it: a
/// Word 1.x/2.0 one (or a damaged one).
pub(super) fn is_bare(data: &[u8]) -> bool {
    matches!(u16_at(data, 0), Some(0xA59B | 0xA59C | 0xA5DB | 0xA5DC | 0xA5EC))
}

/// Whether `data` starts like a Word 1.x/2.0 file.
pub(super) fn is_old(data: &[u8]) -> bool {
    is_bare(data) && u16_at(data, 2).is_some_and(|n| n < 0x0065)
}

pub(super) fn read(data: &[u8]) -> Result<Document, Error> {
    let bad = || Error::new(TOO_OLD);
    let fc_min = u32_at(data, 0x18).ok_or_else(bad)? as usize;
    let fc_mac = u32_at(data, 0x1C).ok_or_else(bad)? as usize;
    let ccp_text = u32_at(data, 0x34).ok_or_else(bad)? as usize;
    let cp = lid_cp(u16_at(data, 6).unwrap_or(0x0409));
    let end = fc_mac.min(data.len());
    if fc_min >= end {
        return Err(bad());
    }
    // The main text comes first; notes and headers follow it.
    let end = match fc_min.checked_add(ccp_text) {
        Some(e) if ccp_text > 0 && e <= end => e,
        _ => end,
    };
    let text = crate::ole::decode_cp(&data[fc_min..end], cp);
    let mut paras = Vec::new();
    let mut cur = String::new();
    let mut page_break = false;
    // Field nesting: whether each open field is still in its code.
    let mut fields: Vec<bool> = Vec::new();
    let mut chars = text.chars().peekable();
    let mut prev = '\0';
    while let Some(c) = chars.next() {
        let after_cell = std::mem::replace(&mut prev, c) == '\u{7}';
        match c {
            // Nesting past this is left out (each character looks at it).
            '\u{13}' if fields.len() < 64 => fields.push(true),
            '\u{13}' => {}
            '\u{14}' => {
                if let Some(f) = fields.last_mut() {
                    *f = false;
                }
            }
            '\u{15}' => {
                fields.pop();
            }
            _ if fields.iter().any(|&code| code) => {}
            '\u{c}' if cur.is_empty() => page_break = true,
            // Table cells end with a mark, rows with one more: a row
            // becomes a paragraph with its cells separated by tabs.
            '\u{7}' if !after_cell => cur.push('\t'),
            '\r' | '\u{7}' | '\u{c}' => {
                if after_cell && cur.ends_with('\t') {
                    cur.pop();
                }
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                let mut p = Paragraph::new(&std::mem::take(&mut cur));
                p.style.page_break_before = std::mem::take(&mut page_break);
                paras.push(p);
                if paras.len() > apex_doc::MAX_PARAGRAPHS {
                    break;
                }
                page_break = c == '\u{c}';
            }
            '\n' | '\u{b}' => cur.push(LINE_BREAK),
            '\t' => cur.push('\t'),
            '\u{1e}' => cur.push('\u{2011}'),
            '\u{1f}' => cur.push('\u{ad}'),
            c if (c as u32) < 0x20 => {}
            c => cur.push(c),
        }
    }
    if !cur.is_empty() || paras.is_empty() {
        let mut p = Paragraph::new(&cur);
        p.style.page_break_before = page_break;
        paras.push(p);
    }
    Ok(Document::from_paragraphs(paras))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Word 2.0 file: a FIB of 0x180 bytes, then the text.
    fn word2(text: &[u8]) -> Vec<u8> {
        let mut d = vec![0u8; 0x180];
        d[0..2].copy_from_slice(&0xA5DBu16.to_le_bytes());
        d[2..4].copy_from_slice(&45u16.to_le_bytes());
        d[6..8].copy_from_slice(&0x0C0Au16.to_le_bytes());
        d[0x18..0x1C].copy_from_slice(&0x180u32.to_le_bytes());
        d[0x1C..0x20].copy_from_slice(&(0x180 + text.len() as u32 + 4).to_le_bytes());
        d[0x34..0x38].copy_from_slice(&(text.len() as u32).to_le_bytes());
        d.extend_from_slice(text);
        // Text of a header, after the main text.
        d.extend_from_slice(b"Hdr\r");
        d
    }

    #[test]
    fn damaged_old_files_say_so() {
        let mut data = word2(b"x\r");
        data[0x18..0x1C].copy_from_slice(&0xFF00_0180u32.to_le_bytes());
        assert_eq!(super::super::read(&data).unwrap_err().to_string(), TOO_OLD);
        data[2] = 0x20;
        data[3] = 0x20;
        assert_eq!(super::super::read(&data).unwrap_err().to_string(), TOO_OLD);
    }

    #[test]
    fn reads_the_text_of_word_2_files() {
        let data = word2(b"Hola, se\xf1or\r\x13 PAGE \x141\x15 de 2\x0bsegunda l\xednea\r\x0cNueva p\xe1gina\rA\x07B\x07\x07");
        assert!(is_old(&data));
        let doc = super::super::read(&data).unwrap();
        let texts: Vec<&str> = doc.paragraphs.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(texts, ["Hola, señor", "1 de 2\u{2028}segunda línea", "Nueva página", "A\tB"]);
        assert!(doc.paragraphs[2].style.page_break_before && !doc.paragraphs[1].style.page_break_before);
    }
}
