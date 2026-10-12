//! PDF fonts as far as text goes: which character each code is, how wide
//! it is, and the family and style to show it in. Glyph shapes are not
//! read (the text is shown in a system font).

use std::collections::HashMap;
use std::sync::Arc;

use super::file::PdfFile;
use super::obj::{Dict, Item, Lexer, Obj};
use super::tables;

/// Family and style of a font, as the document shows it.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Style {
    pub family: Arc<str>,
    pub bold: bool,
    pub italic: bool,
    pub mono: bool,
    pub serif: bool,
}

/// One code of a string: its characters and its width in text space
/// (thousandths of the font size already applied).
pub struct Glyph {
    pub text: Vec<char>,
    pub width: f64,
    /// A single-byte code 32: word spacing applies.
    pub space: bool,
    /// The font gives no width at all: `width` is a guess.
    pub guessed: bool,
    /// The code, and its CID in composite fonts (the code in simple ones).
    pub code: u32,
    pub cid: u32,
}

enum Codes {
    /// One byte per code.
    Simple,
    /// Codes split by code space ranges; CIDs by the CMap.
    Cid { spaces: Vec<(Vec<u8>, Vec<u8>)>, cids: CidMap },
    /// A legacy multi-byte encoding (Shift-JIS, GBK, Big5, EUC-KR), and in
    /// composite fonts the CID of the space (ASCII's CIDs follow it).
    Legacy(&'static encoding_rs::Encoding, Option<u32>),
}

#[derive(Default)]
enum CidMap {
    #[default]
    Identity,
    Ranges(Vec<(u32, u32, u32)>),
}

pub struct Font {
    pub style: Style,
    codes: Codes,
    /// Simple fonts: the character of each code by the encoding.
    enc: Vec<Option<Vec<char>>>,
    to_unicode: Option<ToUnicode>,
    /// Characters by glyph id, from an embedded TrueType font's cmap.
    gid_chars: HashMap<u32, char>,
    cid_to_gid: Option<Vec<u16>>,
    widths: HashMap<u32, f64>,
    /// Width of codes the font gives none for (None: unknown).
    default_width: Option<f64>,
    /// The descriptor's MissingWidth, which pdfium gives every code a
    /// /Widths array leaves out.
    missing_width: Option<f64>,
    /// Glyph space to text space (0.001, or a Type 3 font's matrix).
    scale: f64,
    /// Widths of a standard 14 font without a /Widths array.
    std14: Option<Std14>,
    /// UCS-2 or UTF-16 CMaps: the code is the character.
    unicode_codes: bool,
    /// A simple font without widths: the standard font of its kind, whose
    /// widths pdfium's stand-in has.
    stand_in: Option<Std14>,
    /// Vertical writing: DW2's position and advance, and W2's by CID.
    vertical: Option<((f64, f64), HashMap<u32, (f64, f64, f64)>)>,
}

#[derive(Clone, Copy)]
enum Std14 {
    Table(usize),
    Courier,
    Symbol,
    Dingbats,
}

impl Font {
    /// A fallback for text without a usable font.
    pub fn fallback() -> Font {
        Font { style: Style { family: Arc::from("Helvetica"), ..Default::default() }, codes: Codes::Simple, enc: base_enc(&tables::WIN_ANSI), to_unicode: None, gid_chars: HashMap::new(), cid_to_gid: None, widths: HashMap::new(), default_width: Some(500.0), missing_width: None, scale: 0.001, std14: Some(Std14::Table(0)), unicode_codes: false, stand_in: None, vertical: None }
    }

    pub fn load(f: &PdfFile, d: &Dict) -> Font {
        let subtype = f.key(d, "Subtype").name().unwrap_or("").to_string();
        let base = f.key(d, "BaseFont").name().unwrap_or("").to_string();
        let composite = subtype == "Type0";
        let desc_font = if composite {
            match f.key(d, "DescendantFonts") {
                Obj::Array(a) => a.first().and_then(|o| f.resolve(o).dict().cloned()),
                _ => None,
            }
        } else {
            None
        };
        let fd_owner = desc_font.as_ref().unwrap_or(d);
        let desc = f.key_dict(fd_owner, "FontDescriptor");
        let style = style_of(f, &base, desc.as_ref());
        let to_unicode = match f.key(d, "ToUnicode") {
            Obj::Stream(sd, raw) => Some(ToUnicode::parse(&f.decode(&sd, &raw).data)),
            _ => None,
        };
        // A ToUnicode named Identity: the codes are the characters.
        let identity = matches!(f.key(d, "ToUnicode").name(), Some("Identity-H" | "Identity-V"));
        let mut font = Font { style, codes: Codes::Simple, enc: Vec::new(), to_unicode, gid_chars: HashMap::new(), cid_to_gid: None, widths: HashMap::new(), default_width: None, missing_width: None, scale: 0.001, std14: None, unicode_codes: identity, stand_in: None, vertical: None };
        let flags = desc.as_ref().and_then(|fd| f.key_num(fd, "Flags")).unwrap_or(0.0) as u32;
        if composite {
            let df = desc_font.unwrap_or_default();
            font.default_width = Some(f.key_num(&df, "DW").unwrap_or(1000.0));
            if let Obj::Array(w) = f.key(&df, "W") {
                let w: Vec<Obj> = w.iter().map(|o| f.resolve(o)).collect();
                let mut i = 0;
                // Ranges that overlap again and again stop here.
                let mut work = 0usize;
                while i + 1 < w.len() && work < MAX_WIDTHS {
                    let Some(first) = w[i].int() else { break };
                    match &w[i + 1] {
                        Obj::Array(list) => {
                            work += list.len();
                            for (k, v) in list.iter().enumerate() {
                                font.widths.insert((first as u32).wrapping_add(k as u32), f.resolve(v).num().unwrap_or(0.0));
                            }
                            i += 2;
                        }
                        o => {
                            let (Some(last), Some(v)) = (o.int(), w.get(i + 2).and_then(Obj::num)) else { break };
                            work += last.min(first.saturating_add(65535)).saturating_sub(first).max(0) as usize + 1;
                            for c in first..=last.min(first.saturating_add(65535)) {
                                font.widths.insert(c as u32, v);
                            }
                            i += 3;
                        }
                    }
                }
            }
            font.codes = match f.key(d, "Encoding") {
                Obj::Name(n) => match &*n {
                    "Identity-H" | "Identity-V" => Codes::Cid { spaces: vec![(vec![0, 0], vec![0xFF, 0xFF])], cids: CidMap::Identity },
                    n if n.contains("UCS2") || n.contains("UTF16") => {
                        font.unicode_codes = true;
                        Codes::Cid { spaces: vec![(vec![0, 0], vec![0xFF, 0xFF])], cids: CidMap::Identity }
                    }
                    n => match legacy(n) {
                        // The CMaps with proportional roman letters put them at 231.
                        Some(e) => Codes::Legacy(e, Some(if ["90ms", "83pv"].iter().any(|v| n.starts_with(v)) { 231 } else { 1 })),
                        None => Codes::Cid { spaces: vec![(vec![0, 0], vec![0xFF, 0xFF])], cids: CidMap::Identity },
                    },
                },
                Obj::Stream(sd, raw) => {
                    let cm = ToUnicode::parse(&f.decode(&sd, &raw).data);
                    let spaces = if cm.spaces.is_empty() { vec![(vec![0, 0], vec![0xFF, 0xFF])] } else { cm.spaces };
                    Codes::Cid { spaces, cids: CidMap::Ranges(cm.cids) }
                }
                _ => Codes::Cid { spaces: vec![(vec![0, 0], vec![0xFF, 0xFF])], cids: CidMap::Identity },
            };
            let wmode = match f.key(d, "Encoding") {
                Obj::Name(n) => n.ends_with("-V"),
                Obj::Stream(sd, raw) => memchr::memmem::find(&f.decode(&sd, &raw).data, b"/WMode 1").is_some() || f.key(&sd, "WMode").int() == Some(1),
                _ => false,
            };
            if wmode {
                let dw2 = f.nums(df.get("DW2").unwrap_or(&Obj::Null));
                let dw2 = if dw2.len() == 2 { (dw2[0], dw2[1]) } else { (880.0, -1000.0) };
                let mut w2 = HashMap::new();
                if let Obj::Array(w) = f.key(&df, "W2") {
                    let w: Vec<Obj> = w.iter().map(|o| f.resolve(o)).collect();
                    let mut i = 0;
                    let mut work = 0usize;
                    while i + 1 < w.len() && work < MAX_WIDTHS {
                        let Some(first) = w[i].int() else { break };
                        if let Obj::Array(list) = &w[i + 1] {
                            work += list.len();
                            let v: Vec<f64> = list.iter().map(|o| f.resolve(o).num().unwrap_or(0.0)).collect();
                            for (k, t) in v.chunks_exact(3).enumerate() {
                                w2.insert((first as u32).wrapping_add(k as u32), (t[0], t[1], t[2]));
                            }
                            i += 2;
                        } else {
                            let v: Vec<f64> = w[i + 1..].iter().take(4).filter_map(Obj::num).collect();
                            let [last, w1, vx, vy] = v[..] else { break };
                            work += (last as i64).min(first.saturating_add(65535)).saturating_sub(first).max(0) as usize + 1;
                            for c in first..=(last as i64).min(first.saturating_add(65535)) {
                                w2.insert(c as u32, (w1, vx, vy));
                            }
                            i += 5;
                        }
                    }
                }
                font.vertical = Some((dw2, w2));
            }
            if let Obj::Stream(sd, raw) = f.key(&df, "CIDToGIDMap") {
                let m = f.decode(&sd, &raw).data;
                font.cid_to_gid = Some(m.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect());
            }
            let needs_cmap = font.to_unicode.as_ref().is_none_or(|t| t.is_empty());
            if needs_cmap && let Some(fd) = &desc && let Obj::Stream(sd, raw) = f.key(fd, "FontFile2") {
                font.gid_chars = truetype_chars(&f.decode(&sd, &raw).data);
            }
            return font;
        }
        // Simple fonts.
        let symbolic = flags & 4 != 0 && flags & 32 == 0;
        let std = std14(&base);
        font.std14 = std;
        let base_table: &[u16; 256] = match std {
            Some(Std14::Symbol) => &tables::SYMBOL,
            Some(Std14::Dingbats) => &tables::STANDARD,
            _ if subtype == "TrueType" => &tables::WIN_ANSI,
            _ if symbolic && subtype == "Type1" => &tables::STANDARD,
            _ => &tables::STANDARD,
        };
        let mut enc = base_enc(base_table);
        // A Type 1 font's own encoding.
        if let Some(fd) = &desc
            && let Obj::Stream(sd, raw) = f.key(fd, "FontFile")
        {
            let prog = f.decode(&sd, &raw).data;
            for (code, name) in type1_encoding(&prog) {
                if let Some(c) = glyph_chars(&name) {
                    enc[code as usize] = Some(c);
                }
            }
        }
        match f.key(d, "Encoding") {
            Obj::Name(n) => {
                if let Some(t) = named_enc(&n) {
                    enc = base_enc(t);
                }
            }
            Obj::Dict(ed) => {
                if let Some(t) = f.key(&ed, "BaseEncoding").name().and_then(named_enc) {
                    enc = base_enc(t);
                }
                if let Obj::Array(diffs) = f.key(&ed, "Differences") {
                    let mut code = 0usize;
                    for o in diffs {
                        match f.resolve(&o) {
                            Obj::Int(c) => code = c.clamp(0, 255) as usize,
                            Obj::Name(n) => {
                                if code < 256 {
                                    enc[code] = glyph_chars(&n);
                                }
                                code += 1;
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
        font.enc = enc;
        if subtype == "Type3" {
            font.scale = f.nums(d.get("FontMatrix").unwrap_or(&Obj::Null)).first().copied().unwrap_or(0.001);
        }
        let first = f.key_num(d, "FirstChar").unwrap_or(0.0) as u32;
        if let Obj::Array(ws) = f.key(d, "Widths") {
            for (i, w) in ws.iter().enumerate() {
                font.widths.insert(first + i as u32, f.resolve(w).num().unwrap_or(0.0));
            }
        }
        // Type 3 glyphs not listed have no width (as in pdfium).
        let missing = if subtype == "Type3" { Some(0.0) } else if font.widths.is_empty() { None } else { Some(500.0) };
        font.missing_width = desc.as_ref().and_then(|fd| f.key_num(fd, "MissingWidth")).filter(|w| *w > 0.0);
        font.default_width = font.missing_width.or(missing);
        let has_program = desc.as_ref().is_some_and(|fd| ["FontFile", "FontFile2", "FontFile3"].iter().any(|k| !matches!(f.key(fd, k), Obj::Null)));
        if font.widths.is_empty() && std.is_none() && !has_program {
            let st = &font.style;
            let at = usize::from(st.bold) + 2 * usize::from(st.italic);
            font.stand_in = Some(if st.mono { Std14::Courier } else if st.serif { Std14::Table(4 + at) } else { Std14::Table(at) });
        }
        // A Chinese system font named in GBK and not embedded: the text is
        // GBK, as pdfium reads it.
        let embedded = desc.as_ref().is_some_and(|fd| !matches!(f.key(fd, "FontFile2"), Obj::Null));
        let tag: Vec<u32> = base.chars().take(4).map(|c| c as u32).collect();
        const CHINESE: [[u32; 4]; 5] = [[0xCB, 0xCE, 0xCC, 0xE5], [0xBF, 0xAC, 0xCC, 0xE5], [0xBA, 0xDA, 0xCC, 0xE5], [0xB7, 0xC2, 0xCB, 0xCE], [0xD0, 0xC2, 0xCB, 0xCE]];
        if subtype == "TrueType" && !embedded && CHINESE.iter().any(|c| tag == c) {
            font.codes = Codes::Legacy(encoding_rs::GBK, None);
        }
        font
    }

    /// Splits a string operand into its codes.
    pub fn glyphs(&self, s: &[u8]) -> Vec<Glyph> {
        let mut out = Vec::new();
        match &self.codes {
            Codes::Simple => {
                for &b in s {
                    out.push(self.glyph(b as u32, 1));
                }
            }
            Codes::Legacy(e, space) => {
                let (text, _, _) = e.decode(s);
                for ch in text.chars() {
                    let wide = (ch as u32) >= 0x1100;
                    // Composite fonts: W by the CID where it is known, else DW.
                    let width = match space {
                        Some(sp) => {
                            let cid = (' '..='~').contains(&ch).then(|| sp + ch as u32 - 32);
                            cid.and_then(|c| self.widths.get(&c).copied()).or(self.default_width).unwrap_or(1000.0) * self.scale
                        }
                        None if wide => 1.0,
                        None => 0.5,
                    };
                    // The character stands for the code (glyphs are cached by it).
                    out.push(Glyph { text: vec![ch], width, space: ch == ' ', guessed: false, code: ch as u32, cid: ch as u32 });
                }
            }
            Codes::Cid { spaces, .. } => {
                let mut i = 0;
                while i < s.len() {
                    let n = code_len(spaces, &s[i..]);
                    let code = s[i..(i + n).min(s.len())].iter().fold(0u32, |a, &b| a << 8 | b as u32);
                    out.push(self.glyph(code, n));
                    i += n;
                }
            }
        }
        out
    }

    /// How many bytes each code of `s` takes, as [`Font::glyphs`] splits
    /// it (legacy encodings approximately).
    pub fn code_lens(&self, s: &[u8]) -> Vec<usize> {
        match &self.codes {
            Codes::Simple => vec![1; s.len()],
            Codes::Legacy(e, _) => {
                let (text, _, _) = e.decode(s);
                text.chars().map(|ch| e.encode(ch.encode_utf8(&mut [0; 4])).0.len().max(1)).collect()
            }
            Codes::Cid { spaces, .. } => {
                let mut out = Vec::new();
                let mut i = 0;
                while i < s.len() {
                    let n = code_len(spaces, &s[i..]).min(s.len() - i).max(1);
                    out.push(n);
                    i += n;
                }
                out
            }
        }
    }

    /// Vertical writing: a glyph's advance down the line and its origin
    /// (vx, vy) in thousandths, or None for horizontal fonts.
    pub fn vertical(&self, g: &Glyph) -> Option<(f64, f64, f64)> {
        let ((vy, w1), w2) = self.vertical.as_ref()?;
        Some(w2.get(&g.cid).copied().unwrap_or((*w1, g.width / self.scale / 2.0, *vy)))
    }

    pub fn is_vertical(&self) -> bool {
        self.vertical.is_some()
    }

    fn cid(&self, code: u32) -> u32 {
        match &self.codes {
            Codes::Cid { cids: CidMap::Ranges(r), .. } => r.iter().find(|(lo, hi, _)| (*lo..=*hi).contains(&code)).map_or(0, |(lo, _, c)| c + code - lo),
            _ => code,
        }
    }

    fn glyph(&self, code: u32, len: usize) -> Glyph {
        let mut text = self.to_unicode.as_ref().and_then(|t| t.get(code)).unwrap_or_default();
        if text.is_empty() {
            text = match &self.codes {
                Codes::Simple => self.enc.get(code as usize).cloned().flatten().unwrap_or_default(),
                _ if self.unicode_codes => char::from_u32(code).map(|c| vec![c]).unwrap_or_default(),
                _ => {
                    let cid = self.cid(code);
                    let gid = self.cid_to_gid.as_ref().map_or(cid, |m| m.get(cid as usize).copied().unwrap_or(0) as u32);
                    self.gid_chars.get(&gid).map(|&c| vec![c]).unwrap_or_default()
                }
            };
        }
        // Control characters are never text.
        text.retain(|c| !c.is_control() || *c == '\t');
        let key = if matches!(self.codes, Codes::Simple) { code } else { self.cid(code) };
        let w = match self.widths.get(&key) {
            Some(&w) => w,
            _ if !self.widths.is_empty() && self.missing_width.is_some() => self.missing_width.unwrap_or(0.0),
            _ => match self.std14 {
                // The width of the glyph the encoding names (else of the
                // character ToUnicode gives).
                Some(s) => match self.enc.get(code as usize).cloned().flatten() {
                    Some(named) => std_width(s, &named, code),
                    None => std_width(s, &text, code),
                },
                None => match (self.default_width, self.stand_in) {
                    (None, Some(s)) => std_width(s, &self.enc.get(code as usize).cloned().flatten().unwrap_or(text.clone()), code),
                    (d, _) => d.unwrap_or(500.0),
                },
            },
        };
        let guessed = self.widths.is_empty() && self.std14.is_none() && self.default_width.is_none();
        Glyph { width: w * self.scale, space: len == 1 && code == 32, guessed, text, code, cid: if matches!(self.codes, Codes::Simple) { code } else { self.cid(code) } }
    }
}

fn base_enc(t: &[u16; 256]) -> Vec<Option<Vec<char>>> {
    t.iter().map(|&u| if u == 0 { None } else { char::from_u32(u as u32).map(|c| vec![c]) }).collect()
}

fn named_enc(n: &str) -> Option<&'static [u16; 256]> {
    match n {
        "WinAnsiEncoding" => Some(&tables::WIN_ANSI),
        "MacRomanEncoding" => Some(&tables::MAC_ROMAN),
        "StandardEncoding" | "MacExpertEncoding" => Some(&tables::STANDARD),
        _ => None,
    }
}

fn legacy(cmap: &str) -> Option<&'static encoding_rs::Encoding> {
    if cmap.contains("RKSJ") {
        Some(encoding_rs::SHIFT_JIS)
    } else if cmap.starts_with("GB") {
        Some(encoding_rs::GBK)
    } else if cmap.contains("B5") {
        Some(encoding_rs::BIG5)
    } else if cmap.starts_with("KSC") {
        Some(encoding_rs::EUC_KR)
    } else if cmap.contains("EUC") {
        Some(encoding_rs::EUC_JP)
    } else {
        None
    }
}

/// The length of the code at the start of `s`, as pdfium reads it: codes
/// of one length if all the ranges have it; else a byte at a time until a
/// range (the last given first) is as long, its leading bytes matching as
/// far as they go.
fn code_len(spaces: &[(Vec<u8>, Vec<u8>)], s: &[u8]) -> usize {
    let first = spaces.first().map_or(1, |(lo, _)| lo.len().max(1));
    if spaces.iter().all(|(lo, _)| lo.len() == first) {
        return first;
    }
    let check = |size: usize| -> u8 {
        for (lo, hi) in spaces.iter().rev() {
            if lo.len() < size {
                continue;
            }
            let matched = (0..size).take_while(|&k| s.get(k).is_some_and(|&b| lo[k] <= b && hi.get(k).is_some_and(|&h| b <= h))).count();
            if matched == lo.len() {
                return 2;
            }
            if matched > 0 {
                return if size == lo.len() { 2 } else { 1 };
            }
        }
        0
    };
    let mut size = 1;
    while size < s.len().min(4) && check(size) == 1 {
        size += 1;
    }
    size
}

/// The character (or characters, for ligatures) a glyph name stands for.
pub fn glyph_chars(name: &str) -> Option<Vec<char>> {
    if let Ok(i) = tables::GLYPHS.binary_search_by(|(n, _)| (*n).cmp(name)) {
        return char::from_u32(tables::GLYPHS[i].1 as u32).map(|c| vec![c]);
    }
    if let Some(hex) = name.strip_prefix("uni")
        && hex.len() >= 4
        && hex.len() % 4 == 0
        && hex.chars().all(|c| c.is_ascii_hexdigit())
    {
        let v: Vec<char> = (0..hex.len() / 4).filter_map(|i| u32::from_str_radix(&hex[i * 4..i * 4 + 4], 16).ok().and_then(char::from_u32)).collect();
        return (!v.is_empty()).then_some(v);
    }
    if let Some(hex) = name.strip_prefix('u')
        && (4..=6).contains(&hex.len())
        && hex.chars().all(|c| c.is_ascii_hexdigit())
    {
        return u32::from_str_radix(hex, 16).ok().and_then(char::from_u32).map(|c| vec![c]);
    }
    // "a.sc", "one.oldstyle": the base glyph; "f_f_i": its parts.
    if let Some((b, _)) = name.split_once('.')
        && !b.is_empty()
    {
        return glyph_chars(b);
    }
    if name.contains('_') {
        let v: Vec<char> = name.split('_').filter_map(glyph_chars).flatten().collect();
        return (!v.is_empty()).then_some(v);
    }
    let mut cs = name.chars();
    if let (Some(c), None) = (cs.next(), cs.next()) {
        return Some(vec![c]);
    }
    None
}

fn std14(base: &str) -> Option<Std14> {
    let b = strip_subset(base).to_ascii_lowercase();
    let bold = b.contains("bold");
    let italic = b.contains("italic") || b.contains("oblique");
    let style = usize::from(bold) + 2 * usize::from(italic);
    if b.starts_with("helvetica") || b.starts_with("arial") {
        Some(Std14::Table(style))
    } else if b.starts_with("times") {
        Some(Std14::Table(4 + style))
    } else if b.starts_with("courier") {
        Some(Std14::Courier)
    } else if b.starts_with("symbol") {
        Some(Std14::Symbol)
    } else if b.starts_with("zapfdingbats") {
        Some(Std14::Dingbats)
    } else {
        None
    }
}

fn std_width(s: Std14, text: &[char], code: u32) -> f64 {
    match s {
        Std14::Courier => 600.0,
        Std14::Dingbats => 800.0,
        Std14::Symbol => tables::SYMBOL_WIDTHS.get(code as usize).copied().unwrap_or(500) as f64,
        Std14::Table(t) => {
            // A code the encoding names no glyph for: pdfium's .notdef, as
            // wide as the space.
            let Some(&c) = text.first() else { return tables::WIDTHS[t][0] as f64 };
            // The no-break space has the space's width.
            let c = if c == '\u{a0}' { ' ' } else { c };
            let w = tables::WIN_ANSI.iter().position(|&u| u as u32 == c as u32).filter(|&i| i >= 32).map_or(0, |i| tables::WIDTHS[t][i - 32]);
            if w == 0 { 556.0 } else { w as f64 }
        }
    }
}

fn strip_subset(base: &str) -> &str {
    match base.split_once('+') {
        Some((tag, rest)) if tag.len() == 6 && tag.chars().all(|c| c.is_ascii_uppercase()) => rest,
        _ => base,
    }
}

fn style_of(f: &PdfFile, base: &str, desc: Option<&Dict>) -> Style {
    let name = strip_subset(base);
    let (fam, rest) = match name.find(['-', ',']) {
        Some(i) => (&name[..i], &name[i + 1..]),
        None => (name, ""),
    };
    let lower = name.to_ascii_lowercase();
    let rest_l = rest.to_ascii_lowercase();
    let mut bold = ["bold", "black", "heavy", "semibold", "demi", "extrabold"].iter().any(|w| rest_l.contains(w) || lower.ends_with(w));
    let mut italic = ["italic", "oblique", "ital"].iter().any(|w| rest_l.contains(w) || lower.ends_with(w));
    let mut family = pretty_family(fam);
    let mut mono = lower.contains("mono") || lower.contains("courier") || lower.contains("consol");
    let mut serif = lower.contains("times") || lower.contains("serif") && !lower.contains("sans") || lower.contains("roman") || lower.contains("georgia") || lower.contains("garamond") || lower.contains("cambria") || lower.contains("minion");
    if let Some(fd) = desc {
        let flags = f.key_num(fd, "Flags").unwrap_or(0.0) as u32;
        mono |= flags & 1 != 0;
        serif |= flags & 2 != 0;
        italic |= flags & 64 != 0 || f.key_num(fd, "ItalicAngle").is_some_and(|a| a.abs() > 1.0);
        bold |= flags & 0x40000 != 0 || f.key_num(fd, "FontWeight").is_some_and(|w| w >= 600.0);
        if let Some(ff) = f.key(fd, "FontFamily").bytes() {
            let s = super::text_string(ff);
            if !s.trim().is_empty() {
                family = s.trim().to_string();
            }
        }
    }
    if family.is_empty() {
        family = if mono { "Courier New" } else if serif { "Times New Roman" } else { "Arial" }.to_string();
    }
    Style { family: Arc::from(family), bold, italic, mono, serif }
}

/// "TimesNewRomanPSMT" → "Times New Roman", "Helvetica" → "Arial".
fn pretty_family(fam: &str) -> String {
    let mut f = fam.trim();
    for suffix in ["PSMT", "MT", "PS"] {
        if let Some(s) = f.strip_suffix(suffix)
            && s.len() > 2
        {
            f = s;
            break;
        }
    }
    for suffix in ["BoldItalic", "Bold", "Italic", "Regular"] {
        if let Some(s) = f.strip_suffix(suffix)
            && s.len() > 2
        {
            f = s;
        }
    }
    match f {
        "Helvetica" | "Arial" | "ArialMT" => return "Arial".into(),
        "Times" | "TimesNewRoman" | "Times New Roman" => return "Times New Roman".into(),
        "Courier" | "CourierNew" => return "Courier New".into(),
        "Symbol" => return "Symbol".into(),
        "ZapfDingbats" => return "Wingdings".into(),
        _ => {}
    }
    if f.contains(' ') {
        return f.to_string();
    }
    let mut out = String::new();
    let cs: Vec<char> = f.chars().collect();
    for (i, &c) in cs.iter().enumerate() {
        if i > 0 && c.is_uppercase() && (cs[i - 1].is_lowercase() || cs[i - 1].is_ascii_digit()) {
            out.push(' ');
        }
        out.push(c);
    }
    for (split, joined) in [("Deja Vu", "DejaVu"), ("Open Symbol", "OpenSymbol"), ("Mono Space", "Monospace")] {
        out = out.replace(split, joined);
    }
    out
}

/// A CMap: code space ranges, characters (ToUnicode) and CIDs.
#[derive(Default)]
pub struct ToUnicode {
    spaces: Vec<(Vec<u8>, Vec<u8>)>,
    chars: HashMap<u32, Vec<char>>,
    ranges: Vec<(u32, u32, Vec<u16>)>,
    cids: Vec<(u32, u32, u32)>,
}

impl ToUnicode {
    pub fn parse(data: &[u8]) -> ToUnicode {
        let mut out = ToUnicode::default();
        let mut lx = Lexer::new(data, 0);
        let mut ops: Vec<Obj> = Vec::new();
        let code = |b: &[u8]| b.iter().fold(0u32, |a, &x| a << 8 | x as u32);
        while let Some(it) = lx.next(false) {
            match it {
                Item::Obj(o) => {
                    ops.push(o);
                    if ops.len() > 300_000 {
                        break;
                    }
                }
                Item::Kw(k) => {
                    match k {
                        b"endcodespacerange" => {
                            for p in ops.chunks(2) {
                                // Both ends as long (other ranges are broken).
                                if let [Obj::Str(lo), Obj::Str(hi)] = p
                                    && lo.len() == hi.len()
                                {
                                    out.spaces.push((lo.clone(), hi.clone()));
                                }
                            }
                        }
                        b"endbfchar" => {
                            for p in ops.chunks(2) {
                                if let [Obj::Str(src), Obj::Str(dst)] = p {
                                    out.chars.insert(code(src), utf16(dst));
                                }
                            }
                        }
                        b"endbfrange" => {
                            for p in ops.chunks(3) {
                                match p {
                                    [Obj::Str(lo), Obj::Str(hi), Obj::Str(dst)] => {
                                        let d: Vec<u16> = dst.chunks(2).map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)])).collect();
                                        out.ranges.push((code(lo), code(hi), d));
                                    }
                                    [Obj::Str(lo), Obj::Str(_), Obj::Array(list)] => {
                                        for (i, d) in list.iter().enumerate() {
                                            if let Obj::Str(d) = d {
                                                out.chars.insert(code(lo).wrapping_add(i as u32), utf16(d));
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        b"endcidrange" => {
                            for p in ops.chunks(3) {
                                if let [Obj::Str(lo), Obj::Str(hi), cid] = p {
                                    out.cids.push((code(lo), code(hi), cid.int().unwrap_or(0) as u32));
                                }
                            }
                        }
                        b"endcidchar" => {
                            for p in ops.chunks(2) {
                                if let [Obj::Str(c), cid] = p {
                                    let c = code(c);
                                    out.cids.push((c, c, cid.int().unwrap_or(0) as u32));
                                }
                            }
                        }
                        _ => {}
                    }
                    ops.clear();
                }
            }
        }
        out
    }

    fn is_empty(&self) -> bool {
        self.chars.is_empty() && self.ranges.is_empty()
    }

    fn get(&self, code: u32) -> Option<Vec<char>> {
        if let Some(c) = self.chars.get(&code) {
            return Some(c.clone());
        }
        let (lo, _, d) = self.ranges.iter().find(|(lo, hi, _)| (*lo..=*hi).contains(&code))?;
        let mut d = d.clone();
        if let Some(last) = d.last_mut() {
            *last = last.wrapping_add((code - lo) as u16);
        }
        Some(char::decode_utf16(d).filter_map(Result::ok).collect())
    }
}

fn utf16(b: &[u8]) -> Vec<char> {
    let units = b.chunks(2).map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]));
    char::decode_utf16(units).filter_map(Result::ok).collect()
}

/// `dup 32 /space put` lines of a Type 1 font program.
fn type1_encoding(prog: &[u8]) -> Vec<(u8, String)> {
    let head = &prog[..prog.len().min(64 << 10)];
    let Some(start) = memchr::memmem::find(head, b"/Encoding") else { return Vec::new() };
    let rest = &head[start..];
    if rest[9..].trim_ascii_start().starts_with(b"StandardEncoding") {
        return Vec::new();
    }
    let end = memchr::memmem::find(rest, b"readonly def").or_else(|| memchr::memmem::find(rest, b"eexec")).unwrap_or(rest.len());
    let text = String::from_utf8_lossy(&rest[..end]);
    let mut out = Vec::new();
    let words: Vec<&str> = text.split_ascii_whitespace().collect();
    for w in words.windows(4) {
        if w[0] == "dup"
            && let Ok(code) = w[1].parse::<u8>()
            && let Some(name) = w[2].strip_prefix('/')
            && w[3] == "put"
        {
            out.push((code, name.to_string()));
        }
    }
    out
}

/// The most codes a font's width arrays or cmap ranges are read for.
const MAX_WIDTHS: usize = 1 << 21;

/// Characters by glyph id from a TrueType font's cmap (formats 4 and 12).
fn truetype_chars(font: &[u8]) -> HashMap<u32, char> {
    let mut out = HashMap::new();
    // Codes looked at over every subtable: many overlapping ranges stop here.
    let mut work = 0usize;
    let be16 = |p: usize| font.get(p..p + 2).map_or(0, |b| u16::from_be_bytes([b[0], b[1]]) as u32);
    let be32 = |p: usize| font.get(p..p + 4).map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    let n = be16(4) as usize;
    let Some(cmap) = (0..n).map(|i| 12 + i * 16).find(|&r| font.get(r..r + 4) == Some(b"cmap")).map(|r| be32(r + 8) as usize) else { return out };
    let count = be16(cmap + 2) as usize;
    let mut subs: Vec<(u32, u32, usize)> = (0..count).map(|i| (be16(cmap + 4 + i * 8), be16(cmap + 6 + i * 8), cmap + be32(cmap + 8 + i * 8) as usize)).collect();
    // Unicode tables first.
    subs.sort_by_key(|&(p, e, _)| match (p, e) {
        (3, 10) => 0,
        (3, 1) | (0, _) => 1,
        _ => 2,
    });
    for (p, e, off) in subs {
        let sym = p == 3 && e == 0;
        match be16(off) {
            4 => {
                let segs = be16(off + 6) as usize / 2;
                let ends = off + 14;
                let starts = ends + segs * 2 + 2;
                let deltas = starts + segs * 2;
                let ranges = deltas + segs * 2;
                for s in 0..segs {
                    let (end, start) = (be16(ends + s * 2), be16(starts + s * 2));
                    let delta = be16(deltas + s * 2);
                    let ro = be16(ranges + s * 2) as usize;
                    if start > end || end == 0xFFFF && start == 0xFFFF {
                        continue;
                    }
                    work += (end.min(start + 0x4000) - start) as usize + 1;
                    if work > MAX_WIDTHS {
                        return out;
                    }
                    for c in start..=end.min(start + 0x4000) {
                        let gid = if ro == 0 {
                            (c + delta) & 0xFFFF
                        } else {
                            let g = be16(ranges + s * 2 + ro + (c - start) as usize * 2);
                            if g == 0 { 0 } else { (g + delta) & 0xFFFF }
                        };
                        let ch = if sym && (0xF000..=0xF0FF).contains(&c) { c - 0xF000 } else { c };
                        if gid != 0
                            && let Some(ch) = char::from_u32(ch)
                        {
                            out.entry(gid).or_insert(ch);
                        }
                    }
                }
            }
            12 => {
                let groups = be32(off + 12) as usize;
                for g in 0..groups.min(1 << 16) {
                    let b = off + 16 + g * 12;
                    let (s, e, gid) = (be32(b), be32(b + 4), be32(b + 8));
                    work += e.min(s.saturating_add(0x10000)).saturating_sub(s) as usize + 1;
                    if work > MAX_WIDTHS {
                        return out;
                    }
                    for c in s..=e.min(s.saturating_add(0x10000)) {
                        if let Some(ch) = char::from_u32(c) {
                            out.entry(gid.wrapping_add(c - s)).or_insert(ch);
                        }
                    }
                }
            }
            _ => {}
        }
        if !out.is_empty() {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_names() {
        assert_eq!(glyph_chars("eacute"), Some(vec!['é']));
        assert_eq!(glyph_chars("uni00410042"), Some(vec!['A', 'B']));
        assert_eq!(glyph_chars("u1F600"), Some(vec!['😀']));
        assert_eq!(glyph_chars("f_f_i"), Some(vec!['f', 'f', 'i']));
        assert_eq!(glyph_chars("a.sc"), Some(vec!['a']));
        assert_eq!(glyph_chars("g123"), None);
        assert_eq!(pretty_family("TimesNewRomanPSMT"), "Times New Roman");
        assert_eq!(pretty_family("LiberationSerif"), "Liberation Serif");
        assert_eq!(pretty_family("DejaVuSans"), "DejaVu Sans");
    }

    #[test]
    fn cmaps() {
        let cm = ToUnicode::parse(b"1 begincodespacerange <00> <7F> <8140> <FEFE> endcodespacerange 2 beginbfchar <01> <0041> <02> <00660069> endbfchar 1 beginbfrange <10> <12> <0061> endbfrange");
        assert_eq!(cm.get(1), Some(vec!['A']));
        assert_eq!(cm.get(2), Some(vec!['f', 'i']));
        assert_eq!(cm.get(0x11), Some(vec!['b']));
        assert_eq!(code_len(&cm.spaces, &[0x41, 0x81]), 1);
        assert_eq!(code_len(&cm.spaces, &[0x81, 0x41]), 2);
    }
}
