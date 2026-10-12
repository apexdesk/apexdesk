//! The organization's brand, from policies: its fonts and colors (listed
//! first in the font and color pickers, checked before sharing) and the
//! terms its documents must not use.

use std::sync::RwLock;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Brand {
    pub fonts: Vec<String>,
    /// 0xRRGGBB.
    pub colors: Vec<u32>,
    pub terms: Vec<String>,
}

static BRAND: RwLock<Option<Brand>> = RwLock::new(None);

pub fn set(b: Brand) {
    *BRAND.write().unwrap_or_else(|e| e.into_inner()) = Some(b).filter(|b| *b != Brand::default());
}

pub fn get() -> Brand {
    BRAND.read().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default()
}

/// A color as a policy writes it: "#1F4E79", "1f4e79".
pub fn parse_color(s: &str) -> Option<u32> {
    let h = s.trim().trim_start_matches('#');
    (h.len() == 6).then(|| u32::from_str_radix(h, 16).ok()).flatten()
}

/// Where the terms appear in `text` (whole words, case aside): each term
/// with how many times.
pub fn find_terms(text: &str, terms: &[String]) -> Vec<(String, usize)> {
    let lower = text.to_lowercase();
    let word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    let mut out = Vec::new();
    for t in terms.iter().map(|t| t.trim()).filter(|t| !t.is_empty()) {
        let tl = t.to_lowercase();
        let n = lower.match_indices(&tl).filter(|(i, m)| !word(lower[..*i].chars().next_back()) && !word(lower[i + m.len()..].chars().next())).count();
        if n > 0 {
            out.push((t.to_string(), n));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terms_and_colors() {
        let terms = vec!["confidencial".to_string(), "ACME Corp".to_string()];
        assert_eq!(find_terms("Documento Confidencial de acme corp; no confidenciales.", &terms), [("confidencial".to_string(), 1), ("ACME Corp".to_string(), 1)]);
        assert_eq!(parse_color("#1f4e79"), Some(0x1F4E79));
        assert_eq!(parse_color("red"), None);
    }
}
