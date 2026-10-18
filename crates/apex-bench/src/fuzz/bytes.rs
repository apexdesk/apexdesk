//! Mutations of plain bytes (binary records, streams, compressed data) and
//! of text files (csv, rtf, markdown, plain text): bit flips, interesting
//! integers, cut and paste of ranges, tokens of the format, encodings, and
//! repetition for the files that are slow because they are big or deep.

use super::rng::Rng;

pub const U8S: &[u8] = &[0, 1, 0x7F, 0x80, 0xFF, 0xFE, 0x10, 0x20, 0x40];
pub const U16S: &[u16] = &[0, 1, 2, 0x7F, 0x80, 0xFF, 0x100, 0x7FFF, 0x8000, 0xFFFE, 0xFFFF, 0x2000, 0x2020, 0x4000];
pub const U32S: &[u32] = &[
    0, 1, 2, 0x7F, 0xFF, 0x100, 0xFFFF, 0x10000, 0x7FFF_FFFF, 0x8000_0000, 0xFFFF_FFFE, 0xFFFF_FFFF, 0xFFFF_FFFD, 0xFFFF_FFFA, 0x0010_0000, 0x1000_0000,
    0x4000, 0x3FFF,
];

/// One random change to binary data.
pub fn binary(rng: &mut Rng, d: &mut Vec<u8>) {
    if d.is_empty() {
        d.extend((0..rng.small_len(16)).map(|_| rng.next() as u8));
        return;
    }
    let len = d.len();
    let at = rng.below(len);
    match rng.below(12) {
        0 => d[at] ^= 1 << rng.below(8),
        1 => d[at] = *rng.pick(U8S),
        2 => d[at] = rng.next() as u8,
        3 | 4 => {
            let v = *rng.pick(U16S);
            let v = if rng.one_in(3) { v.wrapping_add(rng.below(3) as u16).wrapping_sub(1) } else { v };
            put(d, at, &v.to_le_bytes());
        }
        5 | 6 => {
            let v = *rng.pick(U32S);
            let v = if rng.one_in(3) { v.wrapping_add(rng.below(3) as u32).wrapping_sub(1) } else { v };
            put(d, at, &v.to_le_bytes());
        }
        7 => {
            // Delete a range.
            let n = rng.small_len(len - at);
            d.drain(at..at + n);
        }
        8 => {
            // Duplicate a range in place.
            let n = rng.small_len(len - at);
            let chunk = d[at..at + n].to_vec();
            d.splice(at..at, chunk);
        }
        9 => {
            // Copy a range over another place.
            let n = rng.small_len(len - at);
            let chunk = d[at..at + n].to_vec();
            let to = rng.below(len);
            put(d, to, &chunk);
        }
        10 => {
            let n = rng.small_len(32);
            let bytes: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
            d.splice(at..at, bytes);
        }
        _ => d.truncate(at),
    }
}

/// Writes `v` at `at`, as far as the data goes.
pub fn put(d: &mut [u8], at: usize, v: &[u8]) {
    for (i, b) in v.iter().enumerate() {
        if let Some(x) = d.get_mut(at + i) {
            *x = *b;
        }
    }
}

/// Tokens that matter to each text format.
fn tokens(ext: &str) -> &'static [&'static str] {
    match ext {
        "csv" | "tsv" => &[
            ",", ";", "\t", "\"", "\"\"", "\r\n", "\n", "\r", "=", "=SUM(A1:XFD1048576)", "=A1", "=1/0", "=INDIRECT(\"A\"&ROW())", "=B1", "1e308", "-1e308",
            "NaN", "-0", "1,5", "1.5", "12/31/9999", "0000-00-00", "2024-02-30", "99999999999999999999", "TRUE", "#REF!", "'", "+", "-", "@", "%", "$1,000",
            "\u{FEFF}", "\u{0}", "sep=;\n", "=HYPERLINK(\"x\")",
        ],
        "rtf" => &[
            "{", "}", "\\", "\\par ", "\\b ", "\\fs-2147483648 ", "\\fs2147483647 ", "\\f99999 ", "\\cf-1 ", "\\u-32768?", "\\u65535?", "\\u1114112?",
            "\\'ff", "\\'", "\\bin99999999 ", "\\bin-1 ", "{\\*\\unknown ", "{\\fonttbl{\\f0 A;}}", "{\\colortbl;\\red255\\green0\\blue0;}", "\\trowd ",
            "\\cellx-5 ", "\\cellx99999999 ", "\\cell ", "\\row ", "\\intbl ", "\\pict ", "\\pngblip ", "\\picw-1 ", "\\pich99999 ", "\\ansicpg65001 ",
            "\\ansicpg-1 ", "\\uc0 ", "\\uc99 ", "\\sect ", "\\page ", "\\footnote ", "\\field ", "{\\fldinst HYPERLINK \"x\"}", "\\li-99999 ",
            "\\tx-1 ", "\\ls99999 ", "\\ilvl99 ", "{\\listtable ", "\\stylesheet ", "\\s99999 ", "\\clvmgf ", "\\clmgf ", "\\cellx0 ",
        ],
        "md" | "markdown" => &[
            "#", "######## ", "> ", "- ", "* ", "1. ", "999999999999. ", "```", "`", "**", "__", "~~", "|", "|---|", "| a | b |\n|---|---|\n", "[x](y)",
            "![a](b)", "![a](/etc/passwd)", "<div>", "</div>", "<br>", "&amp;", "&#0;", "&#x110000;", "\\", "  \n", "---\n", "[^1]", "[^1]: x", "- [ ] ",
            "- [x] ", "    ", "\t", "<!--", "-->", "$$", "$x$", "***", "\u{0}", "\u{FEFF}", "\r",
        ],
        _ => &["\n", "\r\n", "\r", "\t", "\u{0}", "\u{FEFF}", "\u{202E}", "\u{FFFF}", "\u{1F600}", "\u{200B}", "\u{0301}", "\u{0}\u{0}", "\u{C}", "\u{B}"],
    }
}

/// One random change to a text file of the format `ext`.
pub fn text(rng: &mut Rng, d: &mut Vec<u8>, ext: &str) {
    let len = d.len();
    let at = rng.below(len + 1);
    match rng.below(14) {
        0..=3 => {
            let t = rng.pick(tokens(ext));
            let reps = if rng.one_in(4) { rng.small_len(200) } else { 1 };
            let s = t.repeat(reps);
            d.splice(at..at, s.bytes());
        }
        4 => {
            // Deep nesting: an opening token many times.
            let open: &[&str] = match ext {
                "rtf" => &["{", "{\\*\\x ", "{\\trowd "],
                "md" | "markdown" => &["> ", "- ", "  - ", "1. ", "["],
                "csv" | "tsv" => &["\"", "=(", "(("],
                _ => &["\n", "\t"],
            };
            let n = *rng.pick(&[100usize, 1000, 10_000, 100_000]);
            d.splice(at..at, rng.pick(open).repeat(n).bytes());
        }
        5 => {
            // A line or a chunk repeated (quadratic work, long lines).
            if len > 0 {
                let n = rng.small_len((len - at.min(len - 1)).min(4096));
                let from = at.min(len - 1);
                let chunk = d[from..(from + n).min(len)].to_vec();
                let reps = *rng.pick(&[10usize, 1000, 50_000]);
                let rep: Vec<u8> = chunk.iter().copied().cycle().take(chunk.len() * reps).take(64 << 20).collect();
                d.splice(from..from, rep);
            }
        }
        6 => encoding(rng, d),
        7 => {
            // Invalid UTF-8 or a lone surrogate.
            let bad: &[&[u8]] = &[&[0xFF], &[0xC0, 0x80], &[0xED, 0xA0, 0x80], &[0xF4, 0x90, 0x80, 0x80], &[0xE2, 0x82], &[0x80], &[0xFE, 0xFF]];
            d.splice(at..at, rng.pick(bad).iter().copied());
        }
        8 => {
            // Numbers: replace the digits at a place with an extreme value.
            if let Some(p) = d.iter().skip(at).position(u8::is_ascii_digit) {
                let s = at + p;
                let e = s + d[s..].iter().take_while(|b| b.is_ascii_digit()).count();
                let v = rng.pick(&["0", "-1", "4294967296", "2147483648", "99999999999999999999", "1e308", "65536", "1048577", "-2147483649"]);
                d.splice(s..e, v.bytes());
            }
        }
        9 => {
            // Line ends: all of one kind to another.
            let s = String::from_utf8_lossy(d).into_owned();
            let s = match rng.below(3) {
                0 => s.replace("\r\n", "\n").replace('\n', "\r"),
                1 => s.replace("\r\n", "\n").replace('\n', "\r\n"),
                _ => s.replace('\n', ""),
            };
            *d = s.into_bytes();
        }
        _ => binary(rng, d),
    }
}

/// Re-encodes the text: BOMs, UTF-16 (sometimes with an odd length),
/// Latin-1 bytes.
fn encoding(rng: &mut Rng, d: &mut Vec<u8>) {
    let s = String::from_utf8_lossy(d).into_owned();
    match rng.below(5) {
        0 => d.splice(0..0, [0xEF, 0xBB, 0xBF]).for_each(drop),
        1 | 2 => {
            let le = rng.one_in(2);
            let mut out = if le { vec![0xFF, 0xFE] } else { vec![0xFE, 0xFF] };
            for u in s.encode_utf16() {
                out.extend(if le { u.to_le_bytes() } else { u.to_be_bytes() });
            }
            if rng.one_in(2) {
                out.push(b'x');
            }
            *d = out;
        }
        3 => *d = s.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).collect(),
        _ => d.splice(0..0, [0xFF, 0xFE]).for_each(drop),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutations_stay_in_bounds() {
        let mut rng = Rng::new(3);
        for i in 0..2000 {
            let mut d = b"a,b,\"c\"\n1,2,3\n{\\rtf1 x}".to_vec();
            if i % 2 == 0 {
                binary(&mut rng, &mut d);
            } else {
                text(&mut rng, &mut d, ["csv", "rtf", "md", "txt"][i % 4]);
            }
        }
        let mut e = Vec::new();
        binary(&mut rng, &mut e);
        assert!(!e.is_empty());
    }
}
