//! Capitalization patterns, following Hunspell's classification.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Cap {
    /// "hello"
    No,
    /// "Hello"
    Init,
    /// "HELLO" (case-less characters such as digits or apostrophes allowed)
    All,
    /// "HeLLo": first letter upper, mixed afterwards
    HuhInit,
    /// "heLLo"
    Huh,
}

pub(crate) fn captype(w: &str) -> Cap {
    let (mut n, mut ncap, mut nneutral, mut first) = (0usize, 0usize, 0usize, false);
    for (i, c) in w.chars().enumerate() {
        n += 1;
        if c.is_uppercase() {
            ncap += 1;
            if i == 0 {
                first = true;
            }
        } else if !c.is_lowercase() {
            nneutral += 1;
        }
    }
    if ncap == 0 {
        Cap::No
    } else if ncap == 1 && first {
        Cap::Init
    } else if ncap == n || ncap + nneutral == n {
        Cap::All
    } else if first {
        Cap::HuhInit
    } else {
        Cap::Huh
    }
}

pub(crate) fn lower(w: &str) -> String {
    w.to_lowercase()
}

pub(crate) fn upper(w: &str) -> String {
    w.to_uppercase()
}

/// First letter upper-cased, the rest untouched.
pub(crate) fn initcap(w: &str) -> String {
    let mut chars = w.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// First letter upper-cased, the rest lower-cased ("HELLO" -> "Hello").
pub(crate) fn initcap_lower(w: &str) -> String {
    let mut chars = w.chars();
    match chars.next() {
        Some(c) => {
            let mut s: String = c.to_uppercase().collect();
            s.push_str(&chars.as_str().to_lowercase());
            s
        }
        None => String::new(),
    }
}

#[inline]
pub(crate) fn lower_char(c: char) -> char {
    if c.is_ascii() { c.to_ascii_lowercase() } else { c.to_lowercase().next().unwrap_or(c) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies() {
        assert_eq!(captype("hello"), Cap::No);
        assert_eq!(captype("Hello"), Cap::Init);
        assert_eq!(captype("HELLO"), Cap::All);
        assert_eq!(captype("DIDN'T"), Cap::All);
        assert_eq!(captype("McDonald"), Cap::HuhInit);
        assert_eq!(captype("iPod"), Cap::Huh);
        assert_eq!(captype("Ángel"), Cap::Init);
        assert_eq!(initcap_lower("ÁRBOL"), "Árbol");
        assert_eq!(initcap("ñu"), "Ñu");
    }
}
