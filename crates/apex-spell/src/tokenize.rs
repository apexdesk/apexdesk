//! Splitting running text into the words a spell checker should look at.

use std::ops::Range;

/// Splits text into words to check: `(byte range, word)`. Skips URLs, e-mail
/// addresses and words containing digits; ALL-CAPS words too when asked with
/// [`Words::skip_all_caps`]. Apostrophes, hyphens and dots between letters
/// stay inside the word ("didn't", "well-known", "e.g.").
pub fn words(text: &str) -> Words<'_> {
    Words { text, pos: 0, chunk_end: 0, skip_caps: false }
}

/// Iterator returned by [`words`].
#[derive(Clone, Debug)]
pub struct Words<'a> {
    text: &'a str,
    pos: usize,
    /// End of the current whitespace-delimited chunk.
    chunk_end: usize,
    skip_caps: bool,
}

impl<'a> Words<'a> {
    /// Also skip words written entirely in capitals (acronyms like "NASA").
    pub fn skip_all_caps(mut self, yes: bool) -> Words<'a> {
        self.skip_caps = yes;
        self
    }
}

#[inline]
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || is_mark(c)
}

/// Combining marks (decomposed accents) belong to the preceding letter.
#[inline]
fn is_mark(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

#[inline]
fn is_joiner(c: char) -> bool {
    matches!(c, '\'' | '\u{2019}' | '-' | '\u{2010}' | '.' | '\u{00AD}')
}

/// URLs, e-mail addresses and similar chunks are not prose.
fn is_address(chunk: &str) -> bool {
    let t = chunk.trim_start_matches(|c: char| !c.is_alphanumeric());
    let l = t.get(..7).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    if chunk.contains("://") || l.starts_with("www.") || l.starts_with("mailto:") {
        return true;
    }
    if let Some(at) = t.find('@') {
        let (user, host) = (&t[..at], &t[at + 1..]);
        return !user.is_empty() && host.trim_end_matches(|c: char| !c.is_alphanumeric()).contains('.');
    }
    false
}

impl<'a> Iterator for Words<'a> {
    type Item = (Range<usize>, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        let text = self.text;
        loop {
            if self.pos >= self.chunk_end {
                // Next whitespace-delimited chunk.
                let start = self.pos + text[self.pos..].find(|c: char| !c.is_whitespace())?;
                let end = text[start..].find(char::is_whitespace).map(|i| start + i).unwrap_or(text.len());
                self.pos = start;
                self.chunk_end = end;
                if is_address(&text[start..end]) {
                    self.pos = end;
                    continue;
                }
            }
            let chunk = &text[self.pos..self.chunk_end];
            let Some(off) = chunk.find(|c: char| c.is_alphanumeric()) else {
                self.pos = self.chunk_end;
                continue;
            };
            let start = self.pos + off;
            let mut end = start;
            let mut has_dot = false;
            let mut it = text[start..self.chunk_end].char_indices().peekable();
            while let Some((i, c)) = it.next() {
                if is_word_char(c) {
                    end = start + i + c.len_utf8();
                } else if is_joiner(c) && it.peek().is_some_and(|&(_, n)| is_word_char(n)) {
                    has_dot |= c == '.';
                } else {
                    // Abbreviations with inner dots keep their final dot ("e.g.").
                    if c == '.' && has_dot {
                        end = start + i + 1;
                    }
                    break;
                }
            }
            self.pos = end.max(start + 1);
            let word = &text[start..end];
            if word.chars().any(|c| c.is_numeric()) {
                continue;
            }
            if self.skip_caps && word.chars().filter(|c| c.is_alphabetic()).count() > 1 && !word.chars().any(char::is_lowercase)
            {
                continue;
            }
            return Some((start..end, word));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(t: &str) -> Vec<&str> {
        words(t).map(|(_, w)| w).collect()
    }

    #[test]
    fn splits_prose() {
        assert_eq!(list("Hello, world! It's a well-known fact."), ["Hello", "world", "It's", "a", "well-known", "fact"]);
        assert_eq!(list("¿Qué tal? «Bien»—gracias…"), ["Qué", "tal", "Bien", "gracias"]);
        assert_eq!(list("'quoted' dogs' -dash- e.g. U.S.A."), ["quoted", "dogs", "dash", "e.g.", "U.S.A."]);
        assert_eq!(list("  "), Vec::<&str>::new());
    }

    #[test]
    fn ranges_are_byte_offsets() {
        let t = "año  canción";
        let v: Vec<_> = words(t).collect();
        assert_eq!(v[1].0, 6..14);
        assert_eq!(&t[v[1].0.clone()], "canción");
    }

    #[test]
    fn skips_addresses_digits_and_caps() {
        assert_eq!(
            list("see https://example.com/a-b or www.foo.org, mail me@host.com (x@y.es) 21st 1990 a1b ok"),
            ["see", "or", "mail", "ok"]
        );
        let v: Vec<_> = words("NASA and Nasa, I think").skip_all_caps(true).map(|(_, w)| w).collect();
        assert_eq!(v, ["and", "Nasa", "I", "think"]);
    }
}
