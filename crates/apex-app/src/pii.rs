//! Personal data in text: e-mail addresses, Spanish DNI and NIE numbers,
//! IBANs, card numbers and phone numbers. What PDF redaction marks and what
//! the check before sharing warns about. `strict` matches only numbers
//! whose check digits are right (a DNI letter, IBAN mod 97, card Luhn), so
//! that a warning isn't raised by any long number; redaction takes what
//! merely looks like one, since a mistyped number still gives someone away.

use std::ops::Range;
use std::sync::OnceLock;

use regex::Regex;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Email,
    /// Spanish DNI and NIE numbers.
    IdNumber,
    Iban,
    Card,
    Phone,
}

impl Kind {
    /// In the order a scan takes them: a card or an IBAN is not also a
    /// phone number.
    pub const ALL: [Kind; 5] = [Kind::Email, Kind::Iban, Kind::Card, Kind::IdNumber, Kind::Phone];

    /// Translation key of the plural name ("E-mail addresses"...).
    pub fn key(self) -> &'static str {
        match self {
            Kind::Email => "pii.emails",
            Kind::IdNumber => "pii.ids",
            Kind::Iban => "pii.ibans",
            Kind::Card => "pii.cards",
            Kind::Phone => "pii.phones",
        }
    }

    pub fn regex(self) -> &'static Regex {
        static RES: OnceLock<Vec<Regex>> = OnceLock::new();
        let all = RES.get_or_init(|| {
            [
                r"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}",
                r"\b(?:[XYZxyz][- ]?)?\d{7,8}[- ]?[A-Za-z]\b",
                r"\b[A-Z]{2}\d{2}(?: ?[A-Z0-9]{4}){2,7}(?: ?[A-Z0-9]{1,4})?\b",
                r"\b\d(?:[ -]?\d){12,18}\b",
                r"(?:\+\d{1,3}[ .-]?)?(?:\(\d{1,4}\)[ .-]?)?\b\d{2,4}(?:[ .-]?\d{2,4}){2,5}\b",
            ]
            .iter()
            .map(|r| Regex::new(r).expect("pii regex"))
            .collect()
        });
        let i = match self {
            Kind::Email => 0,
            Kind::IdNumber => 1,
            Kind::Iban => 2,
            Kind::Card => 3,
            Kind::Phone => 4,
        };
        &all[i]
    }

    /// Whether a match of the pattern is one.
    pub fn accept(self, s: &str, strict: bool) -> bool {
        let digits: String = s.chars().filter(char::is_ascii_digit).collect();
        match self {
            Kind::Email => true,
            Kind::IdNumber => !strict || id_ok(s),
            Kind::Iban => !strict || iban_ok(s),
            Kind::Card => luhn(&digits) && (!strict || matches!(digits.as_bytes()[0], b'2'..=b'6')),
            Kind::Phone => {
                let n = digits.len();
                if !strict {
                    return (9..=15).contains(&n);
                }
                // International numbers, or Spanish ones (9 digits from 6 to 9).
                let intl = (s.starts_with('+') || digits.starts_with("00")) && (9..=15).contains(&n);
                intl || (n == 9 && matches!(digits.as_bytes()[0], b'6'..=b'9') && !s.contains('.'))
            }
        }
    }

    /// Byte ranges of what this kind matches in `text`.
    pub fn find(self, text: &str, strict: bool) -> Vec<Range<usize>> {
        self.regex().find_iter(text).filter(|m| self.accept(m.as_str(), strict)).map(|m| m.range()).collect()
    }
}

/// Everything in `text` with right check digits, by kind, in the order of
/// `Kind::ALL`; a match overlapping an earlier kind's is left out.
pub fn scan(text: &str) -> Vec<(Kind, Range<usize>)> {
    let mut out: Vec<(Kind, Range<usize>)> = Vec::new();
    for k in Kind::ALL {
        for r in k.find(text, true) {
            if !out.iter().any(|(_, o)| o.start < r.end && r.start < o.end) {
                out.push((k, r));
            }
        }
    }
    out
}

/// A DNI (8 digits and a letter) or NIE (X, Y or Z, 7 digits, a letter)
/// with the right letter.
fn id_ok(s: &str) -> bool {
    let s: String = s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_uppercase();
    let (lead, rest) = match s.as_bytes().first() {
        Some(b'X') => ("0", &s[1..]),
        Some(b'Y') => ("1", &s[1..]),
        Some(b'Z') => ("2", &s[1..]),
        _ => ("", s.as_str()),
    };
    let Some(letter) = rest.chars().last() else { return false };
    let digits = &rest[..rest.len() - 1];
    if digits.len() + lead.len() != 8 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let n: u32 = format!("{lead}{digits}").parse().unwrap_or(u32::MAX);
    "TRWAGMYFPDXBNJZSQVHLCKE".as_bytes().get((n % 23) as usize) == Some(&(letter as u8))
}

/// An IBAN whose check digits are right (ISO 13616, mod 97).
fn iban_ok(s: &str) -> bool {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if !(15..=34).contains(&s.len()) {
        return false;
    }
    let mut rem = 0u32;
    for c in s[4..].chars().chain(s[..4].chars()) {
        let v = match c.to_digit(36) {
            Some(v) => v,
            None => return false,
        };
        rem = if v < 10 { (rem * 10 + v) % 97 } else { (rem * 100 + v) % 97 };
    }
    rem == 1
}

/// The Luhn check of a card number (13 to 19 digits).
fn luhn(digits: &str) -> bool {
    if !(13..=19).contains(&digits.len()) {
        return false;
    }
    let sum: u32 = digits
        .bytes()
        .rev()
        .enumerate()
        .map(|(i, b)| {
            let d = u32::from(b - b'0');
            if i % 2 == 1 { if d * 2 > 9 { d * 2 - 9 } else { d * 2 } } else { d }
        })
        .sum();
    sum % 10 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(s: &str) -> Vec<(Kind, &str)> {
        scan(s).into_iter().map(|(k, r)| (k, &s[r])).collect()
    }

    #[test]
    fn check_digits() {
        assert!(id_ok("12345678Z"));
        assert!(id_ok("12345678-z"));
        assert!(!id_ok("12345678A"));
        assert!(id_ok("X1234567L"));
        assert!(id_ok("Y-1234567-X"));
        assert!(iban_ok("ES91 2100 0418 4502 0005 1332"));
        assert!(iban_ok("GB82WEST12345698765432"));
        assert!(!iban_ok("ES92 2100 0418 4502 0005 1332"));
        assert!(luhn("4111111111111111"));
        assert!(!luhn("4111111111111112"));
    }

    #[test]
    fn finds_personal_data() {
        let s = "Ana (ana.lopez@example.com), DNI 12345678Z, IBAN ES91 2100 0418 4502 0005 1332, \
                 tarjeta 4111 1111 1111 1111, móvil 612 345 678, fijo +34 915 555 555.";
        assert_eq!(
            kinds(s),
            [
                (Kind::Email, "ana.lopez@example.com"),
                (Kind::Iban, "ES91 2100 0418 4502 0005 1332"),
                (Kind::Card, "4111 1111 1111 1111"),
                (Kind::IdNumber, "12345678Z"),
                (Kind::Phone, "612 345 678"),
                (Kind::Phone, "+34 915 555 555"),
            ]
        );
    }

    #[test]
    fn numbers_that_are_not() {
        // A wrong DNI letter, amounts, dates, a reference number.
        let s = "Ref. 12345678A, total 1.234.567,89 €, pedido 2026-09-29, factura 123456789, 4111111111111112.";
        assert_eq!(kinds(s), []);
        // Redaction takes what looks like one.
        assert_eq!(Kind::IdNumber.find("12345678A", false).len(), 1);
    }
}
