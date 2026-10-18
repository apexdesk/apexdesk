//! Decoding of .aff/.dic files. The encoding is declared by the `SET` line of
//! the .aff file and applies to both files.

use crate::charsets::TABLES;

#[derive(Clone, Copy)]
pub(crate) enum Encoding {
    Utf8,
    Latin1,
    Table(&'static [u16; 128]),
}

/// Finds the `SET` directive in the raw .aff bytes (it is plain ASCII in every
/// encoding Hunspell supports) and maps it to a decoder.
pub(crate) fn detect(aff: &[u8]) -> Encoding {
    for line in aff.split(|&b| b == b'\n') {
        let line = line.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(line);
        let Some(rest) = line.strip_prefix(b"SET") else { continue };
        if !rest.first().is_some_and(|b| b.is_ascii_whitespace()) {
            continue;
        }
        let name = String::from_utf8_lossy(rest).trim().to_ascii_uppercase();
        return by_name(&name);
    }
    // Hunspell's default is ISO8859-1, but a file without SET that is valid
    // UTF-8 is far more likely to be UTF-8 nowadays.
    if std::str::from_utf8(aff).is_ok() { Encoding::Utf8 } else { Encoding::Latin1 }
}

pub(crate) fn by_name(name: &str) -> Encoding {
    let norm = name.replace("ISO-8859", "ISO8859").replace("ISO_8859", "ISO8859");
    match norm.as_str() {
        "UTF-8" | "UTF8" => return Encoding::Utf8,
        "ISO8859-1" | "LATIN1" => return Encoding::Latin1,
        "CP1251" | "WINDOWS-1251" => return table("MICROSOFT-CP1251"),
        "TIS620" | "TIS-620" => return table("TIS620-2533"),
        _ => {}
    }
    table(&norm)
}

fn table(name: &str) -> Encoding {
    TABLES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, t)| Encoding::Table(t))
        .unwrap_or(Encoding::Latin1)
}

pub(crate) fn decode(bytes: &[u8], enc: Encoding) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match enc {
        Encoding::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
        Encoding::Latin1 => bytes.iter().map(|&b| b as char).collect(),
        Encoding::Table(t) => bytes
            .iter()
            .map(|&b| {
                if b < 0x80 {
                    b as char
                } else {
                    char::from_u32(t[(b - 0x80) as usize] as u32).unwrap_or('\u{FFFD}')
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_and_decodes() {
        let aff = b"# comment\nSET ISO8859-15\nTRY abc\n";
        let enc = detect(aff);
        assert_eq!(decode(b"\xA4 caf\xE9", enc), "\u{20AC} caf\u{E9}");
        assert_eq!(decode(b"caf\xE9", detect(b"SET ISO8859-1\n")), "caf\u{E9}");
        assert_eq!(decode("café".as_bytes(), detect(b"SET UTF-8\n")), "café");
        // KOI8-R: 0xC1 is Cyrillic small a.
        assert_eq!(decode(b"\xC1", detect(b"SET KOI8-R\n")), "\u{430}");
        // Unknown names fall back to Latin-1 instead of failing.
        assert_eq!(decode(b"\xE9", detect(b"SET FOO\n")), "\u{E9}");
    }
}
