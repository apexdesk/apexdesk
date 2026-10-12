//! Character encodings of CSV and text files: the ones found in offices
//! (UTF-8 with or without a byte order mark, UTF-16, Windows-1252, the
//! MS-DOS code page 850 of old exports, ISO-8859-1 and -15), their
//! detection, and reading and writing text in them.

/// An encoding a text file can be read or written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Encoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Cp1252,
    Cp850,
    Latin1,
    Latin9,
}

impl Encoding {
    pub const ALL: [Encoding; 8] = [Encoding::Utf8, Encoding::Utf8Bom, Encoding::Utf16Le, Encoding::Utf16Be, Encoding::Cp1252, Encoding::Cp850, Encoding::Latin1, Encoding::Latin9];

    /// The name saved in settings and given on the command line.
    pub fn key(self) -> &'static str {
        match self {
            Encoding::Utf8 => "utf-8",
            Encoding::Utf8Bom => "utf-8-bom",
            Encoding::Utf16Le => "utf-16le",
            Encoding::Utf16Be => "utf-16be",
            Encoding::Cp1252 => "windows-1252",
            Encoding::Cp850 => "cp850",
            Encoding::Latin1 => "iso-8859-1",
            Encoding::Latin9 => "iso-8859-15",
        }
    }

    /// The encoding a name means (the keys and the usual other names).
    pub fn from_key(s: &str) -> Option<Encoding> {
        let s = s.trim().to_ascii_lowercase().replace('_', "-");
        Some(match s.as_str() {
            "utf-8" | "utf8" => Encoding::Utf8,
            "utf-8-bom" | "utf8-bom" | "utf-8-sig" | "utf8bom" => Encoding::Utf8Bom,
            "utf-16" | "utf-16le" | "utf16" | "utf16le" | "unicode" => Encoding::Utf16Le,
            "utf-16be" | "utf16be" => Encoding::Utf16Be,
            "windows-1252" | "cp1252" | "1252" | "ansi" => Encoding::Cp1252,
            "cp850" | "ibm850" | "850" | "dos" | "oem" => Encoding::Cp850,
            "iso-8859-1" | "latin1" | "latin-1" | "iso8859-1" => Encoding::Latin1,
            "iso-8859-15" | "latin9" | "latin-9" | "iso8859-15" => Encoding::Latin9,
            _ => return None,
        })
    }

    /// A readable name ("UTF-8", "Windows-1252"…).
    pub fn name(self) -> &'static str {
        match self {
            Encoding::Utf8 => "UTF-8",
            Encoding::Utf8Bom => "UTF-8 BOM",
            Encoding::Utf16Le => "UTF-16 LE",
            Encoding::Utf16Be => "UTF-16 BE",
            Encoding::Cp1252 => "Windows-1252",
            Encoding::Cp850 => "DOS 850",
            Encoding::Latin1 => "ISO-8859-1",
            Encoding::Latin9 => "ISO-8859-15",
        }
    }

    /// The text of `data`, without the byte order mark if it has one.
    /// Bytes that are not valid in the encoding become U+FFFD.
    pub fn decode(self, data: &[u8]) -> String {
        match self {
            Encoding::Utf8 | Encoding::Utf8Bom => String::from_utf8_lossy(data.strip_prefix(UTF8_BOM).unwrap_or(data)).into_owned(),
            Encoding::Utf16Le | Encoding::Utf16Be => {
                let le = self == Encoding::Utf16Le;
                let bom: &[u8] = if le { &[0xFF, 0xFE] } else { &[0xFE, 0xFF] };
                let d = data.strip_prefix(bom).unwrap_or(data);
                let units: Vec<u16> = d.chunks_exact(2).map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) }).collect();
                let mut s = String::from_utf16_lossy(&units);
                if d.len() % 2 == 1 {
                    s.push('\u{FFFD}');
                }
                s
            }
            _ => data.iter().map(|&b| self.char_of(b)).collect(),
        }
    }

    /// `text` in the encoding, with the byte order mark for UTF-8 BOM and
    /// UTF-16; and how many characters the encoding has no room for
    /// (written as `?`).
    pub fn encode(self, text: &str) -> (Vec<u8>, usize) {
        match self {
            Encoding::Utf8 => (text.as_bytes().to_vec(), 0),
            Encoding::Utf8Bom => ([UTF8_BOM, text.as_bytes()].concat(), 0),
            Encoding::Utf16Le | Encoding::Utf16Be => {
                let le = self == Encoding::Utf16Le;
                let mut out = Vec::with_capacity(text.len() * 2 + 2);
                for u in std::iter::once(0xFEFF).chain(text.encode_utf16()) {
                    out.extend_from_slice(&if le { u.to_le_bytes() } else { u.to_be_bytes() });
                }
                (out, 0)
            }
            _ => {
                let mut lost = 0;
                let out = text
                    .chars()
                    .map(|c| {
                        self.byte_of(c).unwrap_or_else(|| {
                            lost += 1;
                            b'?'
                        })
                    })
                    .collect();
                (out, lost)
            }
        }
    }

    /// The character of byte `b` in a one-byte encoding.
    fn char_of(self, b: u8) -> char {
        if b < 0x80 {
            return b as char;
        }
        let u = match self {
            Encoding::Cp1252 if b < 0xA0 => CP1252_80[(b - 0x80) as usize],
            Encoding::Cp850 => CP850_80[(b - 0x80) as usize],
            Encoding::Latin9 => latin9(b),
            _ => b as u16,
        };
        char::from_u32(u as u32).unwrap_or('\u{FFFD}')
    }

    /// The byte of `c` in a one-byte encoding, if it has one.
    fn byte_of(self, c: char) -> Option<u8> {
        if (c as u32) < 0x80 {
            return Some(c as u8);
        }
        (0x80..=0xFFu8).find(|&b| self.char_of(b) == c && !(self == Encoding::Cp1252 && CP1252_80.get((b - 0x80) as usize) == Some(&(b as u16))))
    }
}

const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// Windows-1252 from 0x80 to 0x9F (the five unassigned bytes stay C1).
const CP1252_80: [u16; 32] = [
    0x20AC, 0x81, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x8D, 0x017D, 0x8F, 0x90, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122,
    0x0161, 0x203A, 0x0153, 0x9D, 0x017E, 0x0178,
];

/// Code page 850 from 0x80 to 0xFF.
const CP850_80: [u16; 128] = [
    0xC7, 0xFC, 0xE9, 0xE2, 0xE4, 0xE0, 0xE5, 0xE7, 0xEA, 0xEB, 0xE8, 0xEF, 0xEE, 0xEC, 0xC4, 0xC5, // 80
    0xC9, 0xE6, 0xC6, 0xF4, 0xF6, 0xF2, 0xFB, 0xF9, 0xFF, 0xD6, 0xDC, 0xF8, 0xA3, 0xD8, 0xD7, 0x192, // 90
    0xE1, 0xED, 0xF3, 0xFA, 0xF1, 0xD1, 0xAA, 0xBA, 0xBF, 0xAE, 0xAC, 0xBD, 0xBC, 0xA1, 0xAB, 0xBB, // A0
    0x2591, 0x2592, 0x2593, 0x2502, 0x2524, 0xC1, 0xC2, 0xC0, 0xA9, 0x2563, 0x2551, 0x2557, 0x255D, 0xA2, 0xA5, 0x2510, // B0
    0x2514, 0x2534, 0x252C, 0x251C, 0x2500, 0x253C, 0xE3, 0xC3, 0x255A, 0x2554, 0x2569, 0x2566, 0x2560, 0x2550, 0x256C, 0xA4, // C0
    0xF0, 0xD0, 0xCA, 0xCB, 0xC8, 0x131, 0xCD, 0xCE, 0xCF, 0x2518, 0x250C, 0x2588, 0x2584, 0xA6, 0xCC, 0x2580, // D0
    0xD3, 0xDF, 0xD4, 0xD2, 0xF5, 0xD5, 0xB5, 0xFE, 0xDE, 0xDA, 0xDB, 0xD9, 0xFD, 0xDD, 0xAF, 0xB4, // E0
    0xAD, 0xB1, 0x2017, 0xBE, 0xB6, 0xA7, 0xF7, 0xB8, 0xB0, 0xA8, 0xB7, 0xB9, 0xB3, 0xB2, 0x25A0, 0xA0, // F0
];

/// ISO-8859-15: Latin-1 with eight characters changed.
fn latin9(b: u8) -> u16 {
    match b {
        0xA4 => 0x20AC,
        0xA6 => 0x160,
        0xA8 => 0x161,
        0xB4 => 0x17D,
        0xB8 => 0x17E,
        0xBC => 0x152,
        0xBD => 0x153,
        0xBE => 0x178,
        _ => b as u16,
    }
}

/// The encoding `data` most likely is in: by its byte order mark, then
/// UTF-16 by its zero bytes, UTF-8 if valid, else the one-byte encoding
/// whose characters look most like text (letters rather than box
/// drawing or control characters).
pub fn detect(data: &[u8]) -> Encoding {
    if data.starts_with(UTF8_BOM) {
        return Encoding::Utf8Bom;
    }
    if data.starts_with(&[0xFF, 0xFE]) {
        return Encoding::Utf16Le;
    }
    if data.starts_with(&[0xFE, 0xFF]) {
        return Encoding::Utf16Be;
    }
    let head = &data[..data.len().min(4096) & !1];
    if head.len() >= 4 {
        let pairs = head.len() / 2;
        let odd = head.chunks_exact(2).filter(|c| c[1] == 0 && c[0] != 0).count();
        let even = head.chunks_exact(2).filter(|c| c[0] == 0 && c[1] != 0).count();
        if odd * 10 >= pairs * 3 && even * 10 < pairs {
            return Encoding::Utf16Le;
        }
        if even * 10 >= pairs * 3 && odd * 10 < pairs {
            return Encoding::Utf16Be;
        }
    }
    // A cut in the middle of a character at the end of a sample is fine.
    match std::str::from_utf8(data) {
        Ok(_) => return Encoding::Utf8,
        Err(e) if e.error_len().is_none() => return Encoding::Utf8,
        Err(_) => {}
    }
    let mut best = (i64::MIN, Encoding::Cp1252);
    for enc in [Encoding::Cp1252, Encoding::Cp850, Encoding::Latin9, Encoding::Latin1] {
        let mut prev = ' ';
        let mut score = 0;
        for &b in data {
            let c = enc.char_of(b);
            if b >= 0x80 {
                score += weight(c, prev);
            }
            prev = c;
        }
        if score > best.0 {
            best = (score, enc);
        }
    }
    best.1
}

/// How much a character read from a high byte looks like real text (a
/// capital right after a small letter, as in "cafÚ", does not).
fn weight(c: char, prev: char) -> i64 {
    match c {
        c if c.is_uppercase() && prev.is_lowercase() => 0,
        c if c.is_alphabetic() => 2,
        '€' | '£' | '§' | '°' | '«' | '»' | '¿' | '¡' | '“' | '”' | '‘' | '’' | '–' | '—' | '…' | '·' | '©' | '®' | 'ª' | 'º' | '\u{A0}' => 1,
        '\u{80}'..='\u{9F}' | '\u{2500}'..='\u{25FF}' | '\u{FFFD}' => -3,
        _ => -1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let text = "Año, café; niño – 20 €";
        for enc in Encoding::ALL {
            let (bytes, lost) = enc.encode(text);
            let back = enc.decode(&bytes);
            // No dash in the ISO and DOS ones, no € in two of them.
            let want = match enc {
                Encoding::Cp850 | Encoding::Latin1 => 2,
                Encoding::Latin9 => 1,
                _ => 0,
            };
            assert_eq!(lost, want, "{enc:?}");
            assert!(back.starts_with("Año, café; niño") && (lost > 0 || back == text), "{enc:?} {back}");
            assert_eq!(Encoding::from_key(enc.key()), Some(enc));
        }
        assert_eq!(Encoding::Cp850.encode("ñ").0, vec![0xA4]);
        assert_eq!(Encoding::Cp1252.encode("\u{81}").1, 1);
    }

    #[test]
    fn detection() {
        let spanish = "Descripción;Año;Señal\nCamión;2024;Sí\n";
        for enc in Encoding::ALL {
            let (bytes, _) = enc.encode(spanish);
            let got = detect(&bytes);
            let want = match enc {
                // Without € or C1 bytes these read the same.
                Encoding::Latin1 | Encoding::Latin9 => Encoding::Cp1252,
                e => e,
            };
            assert_eq!(got, want, "{enc:?}");
            assert_eq!(got.decode(&bytes), spanish);
        }
        assert_eq!(detect(&Encoding::Cp1252.encode("Un café, por favor").0), Encoding::Cp1252);
        assert_eq!(detect(&Encoding::Cp850.encode("Un café, por favor").0), Encoding::Cp850);
        // UTF-16 without its mark.
        let bare: Vec<u8> = "Nombre;Edad\nAna;30\n".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(detect(&bare), Encoding::Utf16Le);
        assert_eq!(detect(b"plain ascii"), Encoding::Utf8);
    }
}
