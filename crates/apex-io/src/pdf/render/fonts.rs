//! Glyph shapes of a PDF font: from the font program it embeds
//! (TrueType, OpenType, CFF or Type 1), or from a font of the system for
//! fonts that are not embedded.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::raw::ps::cff::v1::Cff;
use skrifa::raw::ps::type1::Type1Font;
use skrifa::raw::tables::cmap::CmapSubtable;
use skrifa::raw::types::GlyphId16;
use skrifa::raw::{FontData, FontRead, TableProvider};
use skrifa::{FontRef, GlyphId, MetadataProvider};
use vello_cpu::kurbo::{Affine, BezPath};

use super::super::file::PdfFile;
use super::super::font::{self, Style};
use super::super::obj::{Dict, Obj};
use super::super::tables;

/// A system font for a font that is not embedded, by its style and base
/// font name: TrueType or OpenType bytes (maybe a collection) and the
/// index of the face in them.
pub type Fallback<'a> = &'a dyn Fn(&Style, &str) -> Option<(Arc<[u8]>, u32)>;

enum Program {
    /// TrueType or OpenType (a bare CFF font is wrapped into one).
    Sfnt { data: Vec<u8>, upem: f64, names: Option<HashMap<String, u32>>, cff: Option<CffInfo> },
    Type1 { font: Box<Type1Font>, names: HashMap<String, u32>, scale: f64 },
    None,
}

struct CffInfo {
    /// Glyph names (non-CID fonts) by name.
    names: HashMap<String, u32>,
    /// CID-keyed fonts: the glyph of each CID.
    cids: Option<HashMap<u32, u32>>,
    /// The font's own encoding: glyph by code.
    builtin: Vec<Option<u32>>,
}

pub struct Glyphs {
    prog: Rc<Program>,
    /// The program is the system's (the font is not embedded).
    substitute: bool,
    /// The system font is fitted to the PDF's widths: not for the standard
    /// 14's names nor the very font named (pdfium's heuristic).
    fit: bool,
    /// A composite font's system font is the one it names: codes without
    /// characters are its glyphs.
    by_gid: bool,
    /// A substitute for Symbol or ZapfDingbats: reached by the codes of its
    /// own encoding, as those fonts are.
    symbol_set: bool,
    composite: bool,
    truetype: bool,
    symbolic: bool,
    /// Glyph names of codes by the PDF's /Differences.
    diffs: HashMap<u32, String>,
    /// The encoding the PDF names (base encoding), as characters.
    base: Option<&'static [u16; 256]>,
    has_encoding: bool,
    cid_to_gid: Option<Vec<u16>>,
    cache: RefCell<HashMap<u32, Option<Rc<BezPath>>>>,
    advances: RefCell<HashMap<u32, Option<f64>>>,
}

impl Glyphs {
    pub fn load(f: &PdfFile, d: &Dict, style: &Style, fallback: Fallback) -> Glyphs {
        let subtype = f.key(d, "Subtype").name().unwrap_or("").to_string();
        let base_font = f.key(d, "BaseFont").name().unwrap_or("").to_string();
        let composite = subtype == "Type0";
        let desc_font = if composite {
            match f.key(d, "DescendantFonts") {
                Obj::Array(a) => a.first().and_then(|o| f.resolve(o).dict().cloned()),
                _ => None,
            }
        } else {
            None
        };
        let owner = desc_font.as_ref().unwrap_or(d);
        let desc = f.key_dict(owner, "FontDescriptor");
        let flags = desc.as_ref().and_then(|fd| f.key_num(fd, "Flags")).unwrap_or(0.0) as u32;
        let cid_sub = desc_font.as_ref().and_then(|df| f.key(df, "Subtype").name().map(str::to_string)).unwrap_or_default();
        let truetype = subtype == "TrueType" || cid_sub == "CIDFontType2";
        let mut g = Glyphs {
            prog: Rc::new(Program::None),
            substitute: false,
            fit: false,
            by_gid: false,
            symbol_set: false,
            composite,
            truetype,
            symbolic: flags & 4 != 0,
            diffs: HashMap::new(),
            base: None,
            has_encoding: false,
            cid_to_gid: None,
            cache: RefCell::new(HashMap::new()),
            advances: RefCell::new(HashMap::new()),
        };
        if let Some(df) = &desc_font
            && let Obj::Stream(sd, raw) = f.key(df, "CIDToGIDMap")
        {
            let m = f.decode(&sd, &raw).data;
            g.cid_to_gid = Some(m.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect());
        }
        if !composite {
            match f.key(d, "Encoding") {
                Obj::Name(n) => {
                    g.base = named(&n);
                    g.has_encoding = g.base.is_some();
                }
                Obj::Dict(ed) => {
                    g.has_encoding = true;
                    g.base = f.key(&ed, "BaseEncoding").name().and_then(named);
                    if let Obj::Array(diffs) = f.key(&ed, "Differences") {
                        let mut code = 0u32;
                        for o in diffs {
                            match f.resolve(&o) {
                                Obj::Int(c) => code = c.clamp(0, 255) as u32,
                                Obj::Name(n) => {
                                    g.diffs.insert(code, n.to_string());
                                    code += 1;
                                }
                                _ => {}
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        // The embedded program.
        if let Some(fd) = &desc {
            let mut prog = Program::None;
            if let Obj::Stream(sd, raw) = f.key(fd, "FontFile2") {
                prog = sfnt(f.decode(&sd, &raw).data);
            } else if let Obj::Stream(sd, raw) = f.key(fd, "FontFile3") {
                let data = f.decode(&sd, &raw).data;
                prog = if data.starts_with(b"OTTO") || data.starts_with(&[0, 1, 0, 0]) || data.starts_with(b"true") { sfnt(data) } else { cff(data) };
            } else if let Obj::Stream(sd, raw) = f.key(fd, "FontFile") {
                let data = f.decode(&sd, &raw).data;
                prog = match Type1Font::new(&data) {
                    Ok(t1) => {
                        let names = t1.glyph_names().map(|(gid, n)| (n.to_string(), gid.to_u32())).collect();
                        let scale = t1.upem().max(1) as f64;
                        Program::Type1 { font: Box::new(t1), names, scale }
                    }
                    // Some "Type 1" fonts are CFF or TrueType.
                    Err(_) => {
                        let p = sfnt(data.clone());
                        if matches!(p, Program::None) { cff(data) } else { p }
                    }
                };
            }
            g.prog = Rc::new(prog);
        }
        // The standard 14 in pdfium's system stand-ins when installed
        // (Arial for Helvetica, Times New Roman, Courier New).
        if matches!(*g.prog, Program::None)
            && !composite
            && let Some(data) = std14(&base_font, flags)
            && let Some(slot) = STD14.iter().position(|s| s.as_ptr() == data.as_ptr())
            && slot < 12
        {
            let fam = ["Arial", "Times New Roman", "Courier New"][slot / 4];
            let mut style = style.clone();
            style.family = fam.into();
            style.bold = slot & 1 == 1;
            style.italic = slot & 2 == 2;
            let key = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
            if let Some((bytes, index)) = fallback(&style, fam) {
                let prog = system(bytes, index);
                if family(&prog).is_some_and(|n| key(&n) == key(fam)) {
                    g.prog = prog;
                    g.substitute = true;
                }
            }
        }
        // The standard 14 (and their usual aliases), as pdfium draws them:
        // with its own fonts.
        if matches!(*g.prog, Program::None)
            && !composite
            && let Some(data) = std14(&base_font, flags)
        {
            g.prog = shared((data.as_ptr() as usize, u32::MAX), None, || cff(data.to_vec()));
            // Symbol and ZapfDingbats by their own encodings.
            if g.base.is_none() {
                if data.as_ptr() == STD14[12].as_ptr() {
                    g.base = Some(&tables::SYMBOL);
                } else if data.as_ptr() == STD14[13].as_ptr() {
                    for (code, n) in DINGBATS.iter().enumerate().filter(|(_, n)| !n.is_empty()) {
                        g.diffs.entry(code as u32).or_insert_with(|| n.to_string());
                    }
                }
            }
        }
        if matches!(*g.prog, Program::None) && subtype != "Type3" {
            if let Some((bytes, index)) = fallback(style, &base_font) {
                let prog = system(bytes, index);
                let name = family(&prog);
                if composite {
                    let key = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
                    let want = key(&style.family);
                    g.by_gid = name.as_ref().is_some_and(|n| key(n) == want);
                }
                let key = |s: &str| s.chars().filter(|c| *c != ' ').collect::<String>().to_lowercase();
                // (The PDF's name compared as it is, spaces and all.)
                let actual = name.as_ref().is_some_and(|n| base_font.to_lowercase().starts_with(&key(n)));
                g.fit = (base_font.contains(' ') || std14(&base_font, 0).is_none()) && !actual;
                g.prog = prog;
                g.substitute = true;
                let b = base_font.to_ascii_lowercase();
                g.symbol_set = b.contains("symbol") || b.contains("dingbats");
                // A system font is reached by the characters.
                g.cid_to_gid = None;
            }
        }
        g
    }

    pub fn substitute(&self) -> bool {
        self.substitute
    }

    pub fn fits(&self) -> bool {
        self.fit
    }

    pub fn by_gid(&self) -> bool {
        self.by_gid
    }

    /// The shape of the glyph for `code` (`cid` in composite fonts) in text
    /// space (an em is 1), and its advance in the program (in ems), if the
    /// program has it.
    pub fn outline(&self, code: u32, cid: u32, text: &[char]) -> Option<Rc<BezPath>> {
        let key = if self.composite { cid } else { code };
        if let Some(p) = self.cache.borrow().get(&key) {
            return p.clone();
        }
        let p = self.draw(code, cid, text).map(Rc::new);
        let mut c = self.cache.borrow_mut();
        if c.len() < 20_000 {
            c.insert(key, p.clone());
        }
        p
    }

    /// The width of the glyph in the program, in ems (for system fonts,
    /// which are narrowed or widened to the widths the PDF gives).
    pub fn advance(&self, code: u32, cid: u32, text: &[char]) -> Option<f64> {
        let key = if self.composite { cid } else { code };
        if let Some(a) = self.advances.borrow().get(&key) {
            return *a;
        }
        let a = self.measure(code, cid, text);
        let mut c = self.advances.borrow_mut();
        if c.len() < 20_000 {
            c.insert(key, a);
        }
        a
    }

    fn measure(&self, code: u32, cid: u32, text: &[char]) -> Option<f64> {
        let Program::Sfnt { data, upem, .. } = &*self.prog else { return None };
        let font = FontRef::new(data).ok()?;
        let gid = self.gid(&font, code, cid, text)?;
        let m = font.glyph_metrics(Size::unscaled(), LocationRef::default());
        m.advance_width(gid).map(|w| w as f64 / upem)
    }

    fn draw(&self, code: u32, cid: u32, text: &[char]) -> Option<BezPath> {
        match &*self.prog {
            Program::None => None,
            Program::Type1 { font, names, scale } => {
                let gid = self.type1_gid(font, names, code)?;
                let mut pen = Pen::default();
                font.draw(GlyphId::new(gid), None, &mut pen).ok()?;
                let mut p = pen.path;
                p.apply_affine(Affine::scale(1.0 / scale));
                Some(p)
            }
            Program::Sfnt { data, upem, .. } => {
                let font = FontRef::new(data).ok()?;
                let gid = self.gid(&font, code, cid, text)?;
                if broken_glyf(&font, gid) {
                    return None;
                }
                let outlines = font.outline_glyphs();
                let glyph = outlines.get(gid)?;
                let mut pen = Pen::default();
                glyph.draw(DrawSettings::unhinted(Size::unscaled(), LocationRef::default()), &mut pen).ok()?;
                let mut p = pen.path;
                p.apply_affine(Affine::scale(1.0 / upem));
                Some(p)
            }
        }
    }

    /// The name of the glyph of a simple font's code by the PDF's encoding.
    fn name(&self, code: u32) -> Option<&str> {
        self.diffs.get(&code).map(String::as_str)
    }

    /// The character of a simple font's code by the PDF's encoding.
    fn enc_char(&self, code: u32) -> Option<char> {
        if let Some(n) = self.name(code) {
            return font::glyph_chars(n).and_then(|v| v.first().copied());
        }
        let t = self.base.unwrap_or(if self.truetype { &tables::WIN_ANSI } else { &tables::STANDARD });
        t.get(code as usize).copied().filter(|&u| u != 0).and_then(|u| char::from_u32(u as u32))
    }

    fn type1_gid(&self, t1: &Type1Font, names: &HashMap<String, u32>, code: u32) -> Option<u32> {
        if let Some(n) = self.name(code)
            && let Some(&g) = names.get(n)
        {
            return Some(g);
        }
        if !self.has_encoding
            && let Some(enc) = t1.encoding()
            && let Some(g) = enc.map(code as u8)
            && g.to_u32() != 0
        {
            return Some(g.to_u32());
        }
        if let Some(c) = self.enc_char(code)
            && let Some(g) = t1.unicode_charmap().map(c as u32).or_else(|| if c == '\u{a0}' { t1.unicode_charmap().map(32u32) } else { None })
        {
            return Some(g.to_u32());
        }
        t1.encoding().and_then(|e| e.map(code as u8)).map(|g| g.to_u32())
    }

    fn gid(&self, font: &FontRef, code: u32, cid: u32, text: &[char]) -> Option<GlyphId> {
        let Program::Sfnt { names, cff, .. } = &*self.prog else { return None };
        // A system font: by the character.
        if self.substitute {
            // By the glyph's name, or the font's own encoding.
            if !self.composite {
                let by_name = |n: &str| cff.as_ref().and_then(|i| i.names.get(n).copied()).or_else(|| names.as_ref().and_then(|m| m.get(n).copied()));
                if let Some(g) = self.name(code).and_then(by_name) {
                    return Some(GlyphId::new(g));
                }
                if self.symbol_set
                    && self.base.is_none()
                    && let Some(Some(g)) = cff.as_ref().and_then(|i| i.builtin.get(code as usize))
                {
                    return Some(GlyphId::new(*g));
                }
            }
            // A composite font's code without a character is its glyph, as
            // pdfium takes it, in the font the PDF names.
            let Some(ch) = text.first().copied().or_else(|| if self.composite { None } else { self.enc_char(code) }) else {
                // Code 0 (.notdef) draws nothing.
                return (self.by_gid && code != 0).then_some(GlyphId::new(code));
            };
            return font.charmap().map(ch);
        }
        if self.composite {
            if let Some(info) = cff
                && let Some(m) = &info.cids
            {
                return m.get(&cid).map(|&g| GlyphId::new(g));
            }
            let gid = match &self.cid_to_gid {
                Some(m) => m.get(cid as usize).copied().unwrap_or(0) as u32,
                None => cid,
            };
            return Some(GlyphId::new(gid));
        }
        // Simple CFF fonts: by glyph name.
        if let Some(info) = cff {
            // A TrueType font (by the PDF) not symbolic: by the character's
            // cmap first, as pdfium ("o.sc" draws "o").
            if self.truetype
                && !self.symbolic
                && let Some(c) = self.enc_char(code)
                && let Some(g) = font.charmap().map(c).filter(|g| g.to_u32() != 0)
            {
                return Some(g);
            }
            if let Some(n) = self.name(code)
                && let Some(&g) = info.names.get(n)
            {
                return Some(GlyphId::new(g));
            }
            if !self.has_encoding
                && let Some(Some(g)) = info.builtin.get(code as usize)
            {
                return Some(GlyphId::new(*g));
            }
            if let Some(c) = self.enc_char(code) {
                // The font's cmap (OpenType), else the plainest name of the
                // character ("o" before "o.sups").
                if let Some(g) = font.charmap().map(c).filter(|g| g.to_u32() != 0) {
                    return Some(g);
                }
                let best = info.names.iter().filter(|(n, _)| font::glyph_chars(n).is_some_and(|v| v.first() == Some(&c))).min_by_key(|(n, g)| (n.contains(['.', '_']), **g));
                if let Some((_, &g)) = best {
                    return Some(GlyphId::new(g));
                }
            }
            return info.builtin.get(code as usize).copied().flatten().map(GlyphId::new);
        }
        // Simple TrueType fonts: by the cmap subtables.
        let cmap = font.cmap().ok();
        let sub = |p: u16, e: Option<u16>| -> Option<CmapSubtable> {
            let cm = cmap.as_ref()?;
            let i = cm.encoding_records().iter().position(|r| r.platform_id() as u16 == p && e.is_none_or(|e| r.encoding_id() == e))?;
            cm.subtable(i as u16).ok()
        };
        let (sym, uni, mac) = (sub(3, Some(0)), sub(3, Some(1)).or_else(|| sub(0, None)), sub(1, Some(0)));
        let found = |g: Option<GlyphId>| g.filter(|g| g.to_u32() != 0);
        // A name of no character: the character ToUnicode gives, as pdfium
        // takes it.
        if self.name(code).is_some()
            && self.enc_char(code).is_none()
            && let Some(u) = &uni
            && let Some(&c) = text.first()
            && let Some(g) = found(map_cp(u, c as u32))
        {
            return Some(g);
        }
        if let Some(s) = &sym
            && (self.symbolic || uni.is_none())
            && self.name(code).is_none()
        {
            for base in [0, 0xF000, 0xF100, 0xF200] {
                if let Some(g) = found(map_cp(s, base + code)) {
                    return Some(g);
                }
            }
        }
        if let Some(u) = &uni {
            if let Some(c) = self.enc_char(code)
                && let Some(g) = found(map_cp(u, c as u32))
            {
                return Some(g);
            }
            if self.symbolic
                && let Some(g) = found(map_cp(u, code))
            {
                return Some(g);
            }
        }
        if let Some(m) = &mac {
            if let Some(g) = found(map_cp(m, code)) {
                return Some(g);
            }
        }
        // By the name, in the post table.
        if let Some(n) = self.name(code).map(str::to_string).or_else(|| self.enc_char(code).and_then(|c| tables::GLYPHS.iter().find(|(_, u)| *u as u32 == c as u32).map(|(n, _)| n.to_string()))) {
            if let Some(names) = names
                && let Some(&g) = names.get(&n)
            {
                return Some(GlyphId::new(g));
            }
        }
        if let Some(s) = &sym {
            for base in [0xF000, 0] {
                if let Some(g) = found(map_cp(s, base + code)) {
                    return Some(g);
                }
            }
        }
        if cmap.is_none() || (sym.is_none() && uni.is_none() && mac.is_none()) {
            return Some(GlyphId::new(code));
        }
        None
    }
}

/// A TrueType glyph with no points, or made of one (nothing to draw;
/// read-fonts panics on some), or that unfolds into too many points.
fn broken_glyf(font: &FontRef, gid: GlyphId) -> bool {
    !crate::font_check::glyph_bounded(font, gid)
}

/// pdfium's fonts for the standard 14 (BSD-3-Clause, The PDFium Authors):
/// sans, serif and fixed in regular, bold, italic and bold italic, then
/// symbol and dingbats.
static STD14: [&[u8]; 14] = [
    include_bytes!("std14/sans.cff"),
    include_bytes!("std14/sans-bold.cff"),
    include_bytes!("std14/sans-italic.cff"),
    include_bytes!("std14/sans-bolditalic.cff"),
    include_bytes!("std14/serif.cff"),
    include_bytes!("std14/serif-bold.cff"),
    include_bytes!("std14/serif-italic.cff"),
    include_bytes!("std14/serif-bolditalic.cff"),
    include_bytes!("std14/fixed.cff"),
    include_bytes!("std14/fixed-bold.cff"),
    include_bytes!("std14/fixed-italic.cff"),
    include_bytes!("std14/fixed-bolditalic.cff"),
    include_bytes!("std14/symbol.cff"),
    include_bytes!("std14/dingbats.cff"),
];

/// ZapfDingbats' own encoding: the glyph name of each code.
static DINGBATS: [&str; 256] = ["", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "space", "a1", "a2", "a202", "a3", "a4", "a5", "a119", "a118", "a117", "a11", "a12", "a13", "a14", "a15", "a16", "a105", "a17", "a18", "a19", "a20", "a21", "a22", "a23", "a24", "a25", "a26", "a27", "a28", "a6", "a7", "a8", "a9", "a10", "a29", "a30", "a31", "a32", "a33", "a34", "a35", "a36", "a37", "a38", "a39", "a40", "a41", "a42", "a43", "a44", "a45", "a46", "a47", "a48", "a49", "a50", "a51", "a52", "a53", "a54", "a55", "a56", "a57", "a58", "a59", "a60", "a61", "a62", "a63", "a64", "a65", "a66", "a67", "a68", "a69", "a70", "a71", "a72", "a73", "a74", "a203", "a75", "a204", "a76", "a77", "a78", "a79", "a81", "a82", "a83", "a84", "a97", "a98", "a99", "a100", "", "a89", "a90", "a93", "a94", "a91", "a92", "a205", "a85", "a206", "a86", "a87", "a88", "a95", "a96", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "a101", "a102", "a103", "a104", "a106", "a107", "a108", "a112", "a111", "a110", "a109", "a120", "a121", "a122", "a123", "a124", "a125", "a126", "a127", "a128", "a129", "a130", "a131", "a132", "a133", "a134", "a135", "a136", "a137", "a138", "a139", "a140", "a141", "a142", "a143", "a144", "a145", "a146", "a147", "a148", "a149", "a150", "a151", "a152", "a153", "a154", "a155", "a156", "a157", "a158", "a159", "a160", "a161", "a163", "a164", "a196", "a165", "a192", "a166", "a167", "a168", "a169", "a170", "a171", "a172", "a173", "a162", "a174", "a175", "a176", "a177", "a178", "a179", "a193", "a180", "a199", "a181", "a200", "a182", "", "a201", "a183", "a184", "a197", "a185", "a194", "a198", "a186", "a195", "a187", "a188", "a189", "a190", "a191", ""];

/// The font for a standard 14 font (or an alias of one) not embedded.
fn std14(base: &str, flags: u32) -> Option<&'static [u8]> {
    let name = base.split_once('+').filter(|(t, _)| t.len() == 6).map_or(base, |(_, r)| r);
    let name: String = name.chars().filter(|c| *c != ' ').collect::<String>().to_ascii_lowercase();
    let (family, style) = name.split_once([',', '-']).unwrap_or((&name, ""));
    let family = family.trim_end_matches("mt").trim_end_matches("ps");
    let at = match family {
        "helvetica" | "arial" => 0,
        "times" | "timesroman" | "timesnewroman" => 4,
        "courier" | "couriernew" => 8,
        "symbol" => return Some(STD14[12]),
        "zapfdingbats" => return Some(STD14[13]),
        _ => return None,
    };
    let bold = style.contains("bold") || style.contains("black") || (flags & 0x40000 != 0);
    let italic = style.contains("italic") || style.contains("oblique") || (style.is_empty() && flags & 0x40 != 0);
    Some(STD14[at + bold as usize + 2 * italic as usize])
}

fn named(n: &str) -> Option<&'static [u16; 256]> {
    match n {
        "WinAnsiEncoding" => Some(&tables::WIN_ANSI),
        "MacRomanEncoding" => Some(&tables::MAC_ROMAN),
        "StandardEncoding" => Some(&tables::STANDARD),
        _ => None,
    }
}

thread_local! {
    /// The programs of system fonts and the standard 14, built once in a
    /// thread (the one drawing pages keeps them for every document); with
    /// the system font's bytes, which keep their address the key.
    static SHARED: RefCell<HashMap<(usize, u32), (Option<Arc<[u8]>>, Rc<Program>)>> = RefCell::new(HashMap::new());
}

/// The program kept under `key`, made when missing.
fn shared(key: (usize, u32), keep: Option<Arc<[u8]>>, make: impl FnOnce() -> Program) -> Rc<Program> {
    if let Some(p) = SHARED.with_borrow(|m| m.get(&key).map(|e| e.1.clone())) {
        return p;
    }
    let p = Rc::new(make());
    SHARED.with_borrow_mut(|m| {
        if m.len() >= 64 {
            m.clear();
        }
        m.insert(key, (keep, p.clone()));
    });
    p
}

/// The program of face `index` of a system font.
fn system(bytes: Arc<[u8]>, index: u32) -> Rc<Program> {
    let key = (bytes.as_ptr() as usize, index);
    let data = bytes.clone();
    shared(key, Some(bytes), move || sfnt(face(&data, index)))
}

/// A TrueType or OpenType program's family name.
fn family(p: &Program) -> Option<String> {
    let Program::Sfnt { data, .. } = p else { return None };
    FontRef::new(data).ok()?.localized_strings(skrifa::string::StringId::FAMILY_NAME).english_or_first().map(|n| n.to_string())
}

/// One face of a font file (collections hold several) as a font of its own.
fn face(data: &[u8], index: u32) -> Vec<u8> {
    if !data.starts_with(b"ttcf") {
        return data.to_vec();
    }
    let Ok(font) = FontRef::from_index(data, index) else { return Vec::new() };
    let Some(tables) = tables_of(&font, data.len(), &|_| None) else { return Vec::new() };
    build_sfnt(tables)
}

/// The tables of `font` (`len` bytes), each tag once, with `swap` giving
/// new contents for some: None when they'd copy many times the file
/// (records that all point at the whole of it).
fn tables_of(font: &FontRef, len: usize, swap: &dyn Fn(&[u8; 4]) -> Option<Vec<u8>>) -> Option<Vec<([u8; 4], Vec<u8>)>> {
    let mut seen = std::collections::HashSet::new();
    let mut left = len.saturating_mul(2).saturating_add(1 << 16);
    let mut out = Vec::new();
    for r in font.table_directory().table_records() {
        let t = r.tag().to_be_bytes();
        if !seen.insert(t) {
            continue;
        }
        let d = match swap(&t) {
            Some(d) => d,
            None => match font.table_data(r.tag()) {
                Some(d) => d.as_bytes().to_vec(),
                None => continue,
            },
        };
        left = left.checked_sub(d.len())?;
        out.push((t, d));
    }
    Some(out)
}

/// A TrueType or OpenType program, with the tables a renderer needs added
/// when a PDF's subset leaves them out.
fn sfnt(data: Vec<u8>) -> Program {
    // An embedded collection: its first face.
    let data = if data.starts_with(b"ttcf") { face(&data, 0) } else { data };
    let data = complete(data);
    let Ok(font) = FontRef::new(&data) else { return Program::None };
    // A head table without its magic number: the tables aren't where the
    // directory says (bytes lost); a system font draws the text instead.
    if font.head().is_ok_and(|h| h.magic_number() != 0x5F0F_3CF5) {
        return Program::None;
    }
    let upem = font.head().map(|h| h.units_per_em()).unwrap_or(1000).max(1) as f64;
    let names = font.post().ok().map(|post| {
        let n = font.maxp().map(|m| m.num_glyphs()).unwrap_or(0);
        (0..n).filter_map(|g| post.glyph_name(GlyphId16::new(g)).map(|s| (s.to_string(), g as u32))).collect()
    });
    let cff_info = font.table_data(skrifa::raw::types::Tag::new(b"CFF ")).and_then(|t| cff_info(t.as_bytes()));
    if font.outline_glyphs().format().is_none() {
        return Program::None;
    }
    Program::Sfnt { data, upem, names, cff: cff_info }
}

/// A bare CFF program, wrapped into an OpenType font.
fn cff(mut data: Vec<u8>) -> Program {
    fix_private(&mut data);
    let Some(info) = cff_info(&data) else { return Program::None };
    let n = Cff::read(FontData::new(&data)).ok().and_then(|c| charset(&c).map(|cs| cs.num_glyphs())).unwrap_or(1).max(1);
    let n = n.min(65535) as u16;
    let otf = build_sfnt(vec![(*b"CFF ", data), (*b"head", head(1000)), (*b"hhea", hhea(1)), (*b"hmtx", vec![0, 0, 0, 0]), (*b"maxp", maxp(n))]);
    match sfnt(otf) {
        Program::Sfnt { data, upem, names, .. } => Program::Sfnt { data, upem, names, cff: Some(info) },
        p => p,
    }
}

/// Turns operators the Private DICT doesn't define into an inert one taking
/// any numbers (FamilyOtherBlues), as FreeType skips them: read-fonts reads
/// them by their Top DICT meaning and drops the whole font when the value
/// isn't an integer there.
fn fix_private(data: &mut [u8]) {
    use skrifa::raw::ps::cff::dict::{Entry, entries};
    let range = {
        let Ok(c) = Cff::read(FontData::new(data)) else { return };
        let Ok(top) = c.top_dicts().get(0) else { return };
        let r = entries(top, None).flatten().find_map(|e| match e {
            Entry::PrivateDictRange(r) => Some(r),
            _ => None,
        });
        match r {
            Some(r) => r,
            None => return,
        }
    };
    let Some(dict) = data.get_mut(range) else { return };
    let mut i = 0;
    while i < dict.len() {
        match dict[i] {
            12 => i += 2,
            6..=11 | 19..=21 => i += 1,
            0..=21 => {
                dict[i] = 9;
                i += 1;
            }
            28 => i += 3,
            29 => i += 5,
            30 => {
                i += 1;
                while i < dict.len() && dict[i] & 0x0f != 0x0f && dict[i] >> 4 != 0x0f {
                    i += 1;
                }
                i += 1;
            }
            247..=254 => i += 2,
            _ => i += 1,
        }
    }
}

/// The charset of a CFF font's first face (glyph to name, or to CID in
/// CID-keyed fonts, which read-fonts leaves out), the predefined ISOAdobe
/// one when the font names none.
fn charset<'a>(c: &Cff<'a>) -> Option<skrifa::raw::ps::cff::charset::Charset<'a>> {
    use skrifa::raw::ps::cff::dict::{Entry, entries};
    let top = c.top_dicts().get(0).ok()?;
    let (mut offset, mut charstrings) = (0, None);
    for e in entries(top, None).flatten() {
        match e {
            Entry::Charset(o) => offset = o,
            Entry::CharstringsOffset(o) => charstrings = Some(o),
            _ => {}
        }
    }
    let data = c.offset_data();
    let n = skrifa::raw::ps::cff::index::Index::new(data.as_bytes().get(charstrings?..)?, false).ok()?.count();
    skrifa::raw::ps::cff::charset::Charset::new(data, offset, n).ok()
}

fn cff_info(data: &[u8]) -> Option<CffInfo> {
    use skrifa::raw::ps::cff::dict::{Entry, entries};
    use skrifa::raw::ps::cff::encoding::Encoding;
    let c = Cff::read(FontData::new(data)).ok()?;
    let top = c.top_dicts().get(0).ok()?;
    let mut cid_keyed = false;
    let mut enc_off = 0usize;
    for e in entries(top, None).flatten() {
        match e {
            Entry::Ros { .. } => cid_keyed = true,
            Entry::Encoding(o) => enc_off = o,
            _ => {}
        }
    }
    let charset = charset(&c);
    let mut names = HashMap::new();
    let mut cids = None;
    if let Some(cs) = &charset {
        if cid_keyed {
            cids = Some(cs.iter().map(|(g, sid)| (sid.to_u16() as u32, g.to_u32())).collect());
        } else {
            for (g, sid) in cs.iter() {
                if let Some(s) = c.string(sid) {
                    names.insert(String::from_utf8_lossy(s).into_owned(), g.to_u32());
                }
            }
        }
    }
    let mut builtin = vec![None; 256];
    if !cid_keyed && let Some(cs) = &charset {
        match enc_off {
            0 | 1 => {
                let pe = if enc_off == 0 { skrifa::raw::ps::encoding::PredefinedEncoding::Standard } else { skrifa::raw::ps::encoding::PredefinedEncoding::Expert };
                for (code, slot) in builtin.iter_mut().enumerate() {
                    let n = pe.name(code as u8);
                    *slot = names.get(n).copied().filter(|&g| g != 0);
                }
            }
            off => {
                if let Ok(enc) = Encoding::new(data, off) {
                    for (code, slot) in builtin.iter_mut().enumerate() {
                        *slot = enc.map(cs, code as u8).map(|g| g.to_u32()).filter(|&g| g != 0);
                    }
                }
            }
        }
    }
    Some(CffInfo { names, cids, builtin })
}

fn be16(v: u16) -> [u8; 2] {
    v.to_be_bytes()
}

fn head(upem: u16) -> Vec<u8> {
    let mut h = Vec::with_capacity(54);
    h.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    h.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    h.extend_from_slice(&0u32.to_be_bytes());
    h.extend_from_slice(&0x5F0F_3CF5u32.to_be_bytes());
    h.extend_from_slice(&be16(0));
    h.extend_from_slice(&be16(upem));
    h.extend_from_slice(&[0; 16]);
    h.extend_from_slice(&[0; 8]);
    h.extend_from_slice(&be16(0));
    h.extend_from_slice(&be16(8));
    h.extend_from_slice(&be16(2));
    h.extend_from_slice(&be16(0));
    h.extend_from_slice(&be16(0));
    h
}

fn hhea(n_metrics: u16) -> Vec<u8> {
    let mut h = Vec::with_capacity(36);
    h.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    h.extend_from_slice(&[0; 30]);
    h.extend_from_slice(&be16(n_metrics));
    h
}

fn maxp(n: u16) -> Vec<u8> {
    let mut m = 0x0000_5000u32.to_be_bytes().to_vec();
    m.extend_from_slice(&be16(n));
    m
}

/// An OpenType file of the tables.
fn build_sfnt(mut tables: Vec<([u8; 4], Vec<u8>)>) -> Vec<u8> {
    tables.sort_by(|a, b| a.0.cmp(&b.0));
    let n = tables.len() as u16;
    let cff = tables.iter().any(|t| &t.0 == b"CFF ");
    let mut out = Vec::new();
    out.extend_from_slice(if cff { b"OTTO" } else { &[0, 1, 0, 0] });
    out.extend_from_slice(&be16(n));
    out.extend_from_slice(&[0; 6]);
    let mut off = 12 + 16 * tables.len();
    let mut body = Vec::new();
    for (tag, data) in &tables {
        out.extend_from_slice(tag);
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&(off as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        body.extend_from_slice(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        off = 12 + 16 * tables.len() + body.len();
    }
    out.extend_from_slice(&body);
    out
}

/// Adds the tables a TrueType subset in a PDF may lack (hhea, hmtx, maxp,
/// head) and drops a broken table directory's duplicates.
fn complete(mut data: Vec<u8>) -> Vec<u8> {
    fix_loc_format(&mut data);
    if let Some(d) = order_glyf(&data) {
        data = d;
    }
    let Ok(font) = FontRef::new(&data) else { return data };
    let has = |t: &[u8; 4]| font.table_data(skrifa::raw::types::Tag::new(t)).is_some();
    if has(b"hhea") && has(b"hmtx") && has(b"maxp") && has(b"head") {
        return data;
    }
    let Some(mut tables) = tables_of(&font, data.len(), &|_| None) else { return data };
    let n_glyphs = font.loca(None).ok().map(|l| l.len().saturating_sub(1)).unwrap_or(0).clamp(1, 65535) as u16;
    if !has(b"head") {
        tables.push((*b"head", head(1000)));
    }
    if !has(b"maxp") {
        tables.push((*b"maxp", maxp(n_glyphs)));
    }
    if !has(b"hhea") || !has(b"hmtx") {
        tables.retain(|t| &t.0 != b"hhea" && &t.0 != b"hmtx");
        tables.push((*b"hhea", hhea(1)));
        tables.push((*b"hmtx", vec![0, 0, 0, 0]));
    }
    build_sfnt(tables)
}

/// A loca table whose offsets go back (glyphs stored out of order): the
/// glyphs rewritten in order, each running to the next offset after its
/// start, as FreeType reads them.
fn order_glyf(data: &[u8]) -> Option<Vec<u8>> {
    let font = FontRef::new(data).ok()?;
    let tag = |t: &[u8; 4]| font.table_data(skrifa::raw::types::Tag::new(t)).map(|d| d.as_bytes());
    let (head, loca, glyf) = (tag(b"head")?, tag(b"loca")?, tag(b"glyf")?);
    let long = head.get(50..52)? != [0, 0];
    let offs: Vec<usize> = if long {
        loca.chunks_exact(4).map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]]) as usize).collect()
    } else {
        loca.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]]) as usize * 2).collect()
    };
    if offs.windows(2).all(|w| w[0] <= w[1]) {
        return None;
    }
    let mut sorted: Vec<usize> = offs.clone();
    sorted.push(glyf.len());
    sorted.sort_unstable();
    sorted.dedup();
    let mut new_glyf = Vec::new();
    let mut new_loca = Vec::with_capacity(offs.len() * 4);
    // Offsets going back and forth would copy the glyphs many times over.
    let most = glyf.len().saturating_mul(2).saturating_add(1 << 16);
    for (i, &a) in offs.iter().enumerate() {
        new_loca.extend_from_slice(&(new_glyf.len() as u32).to_be_bytes());
        if i + 1 == offs.len() {
            break;
        }
        if offs[i + 1] == a || a >= glyf.len() {
            continue;
        }
        let end = sorted.get(sorted.partition_point(|&o| o <= a)).copied().unwrap_or(glyf.len()).min(glyf.len());
        if new_glyf.len() + (end - a) > most {
            return None;
        }
        new_glyf.extend_from_slice(&glyf[a..end]);
        while new_glyf.len() % 4 != 0 {
            new_glyf.push(0);
        }
    }
    let mut head = head.to_vec();
    head[50..52].copy_from_slice(&[0, 1]);
    let tables = tables_of(&font, data.len().saturating_add(new_glyf.len()).saturating_add(new_loca.len()), &|t| match t {
        b"glyf" => Some(new_glyf.clone()),
        b"loca" => Some(new_loca.clone()),
        b"head" => Some(head.clone()),
        _ => None,
    })?;
    Some(build_sfnt(tables))
}

/// A head table's index-to-loca format other than 0 or 1 read as FreeType
/// does: anything not 0 means long offsets.
fn fix_loc_format(data: &mut [u8]) {
    let n = data.get(4..6).map_or(0, |b| u16::from_be_bytes([b[0], b[1]]) as usize);
    for i in 0..n {
        let Some(r) = data.get(12 + 16 * i..28 + 16 * i) else { return };
        if &r[..4] == b"head" {
            let at = u32::from_be_bytes([r[8], r[9], r[10], r[11]]) as usize + 50;
            if let Some(f) = data.get_mut(at..at + 2)
                && f[0] | f[1] != 0
            {
                f.copy_from_slice(&[0, 1]);
            }
            return;
        }
    }
}

#[derive(Default)]
struct Pen {
    path: BezPath,
}

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to((x as f64, y as f64));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.path.line_to((x as f64, y as f64));
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.path.quad_to((cx0 as f64, cy0 as f64), (x as f64, y as f64));
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.path.curve_to((cx0 as f64, cy0 as f64), (cx1 as f64, cy1 as f64), (x as f64, y as f64));
    }
    fn close(&mut self) {
        self.path.close_path();
    }
}


/// A cmap subtable's glyph for a character; format 2 (which skrifa leaves
/// out) for single bytes, as FreeType reads it.
fn map_cp(sub: &CmapSubtable, cp: u32) -> Option<GlyphId> {
    let CmapSubtable::Format2(t) = sub else { return sub.map_codepoint(cp) };
    if cp > 0xFF || t.sub_header_keys().get(cp as usize)?.get() != 0 {
        return None;
    }
    // Sub-header 0, after the 256 keys.
    let d = t.offset_data();
    let h = 6 + 512;
    let first: u16 = d.read_at(h).ok()?;
    let count: u16 = d.read_at(h + 2).ok()?;
    let delta: i16 = d.read_at(h + 4).ok()?;
    let range: u16 = d.read_at(h + 6).ok()?;
    let i = (cp as u16).checked_sub(first).filter(|i| *i < count)?;
    let g: u16 = d.read_at(h + 6 + range as usize + 2 * i as usize).ok()?;
    (g != 0).then(|| GlyphId::new(g.wrapping_add(delta as u16) as u32))
}
