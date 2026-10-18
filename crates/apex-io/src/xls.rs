//! Excel workbooks (.xls: BIFF8 from Excel 97-2003 and BIFF5 from Excel
//! 5.0/95), read only: cells, formulas, formats, merged cells, column widths
//! and row heights, frozen panes, notes, hyperlinks, defined names, print
//! areas, conditional formats, validations and autofilters (see
//! [`crate::xls_data`]), pictures ([`crate::xls_art`]) and charts
//! ([`crate::xls_chart`]).

use std::collections::HashMap;
use std::sync::Arc;

use apex_sheet::formula::parse_canonical;
use apex_sheet::sheet::ROTATION_STACKED;
use apex_sheet::{Borders, CellRef, CellStyle, Chart, Content, HAlign, ImageFormat, Line, LineStyle, Link, Note, Range, Sheet, VAlign, Value, Workbook};

use crate::Error;
use crate::biff::{self, Cur, Refs};

/// A record with the data of the `CONTINUE` records after it appended;
/// `breaks` are where each one started.
pub(crate) struct Rec {
    pub ty: u16,
    pub offset: usize,
    pub data: Vec<u8>,
    pub breaks: Vec<usize>,
}

fn records(stream: &[u8]) -> Vec<Rec> {
    let mut out: Vec<Rec> = Vec::new();
    let mut p = 0;
    while p + 4 <= stream.len() {
        let ty = u16::from_le_bytes([stream[p], stream[p + 1]]);
        let len = u16::from_le_bytes([stream[p + 2], stream[p + 3]]) as usize;
        let d = &stream[p + 4..(p + 4 + len).min(stream.len())];
        if ty == 0x003C
            && let Some(last) = out.last_mut()
        {
            // Empty ones add no break (kept sorted and once each).
            if !d.is_empty() && last.breaks.last() != Some(&last.data.len()) {
                last.breaks.push(last.data.len());
            }
            last.data.extend_from_slice(d);
        } else {
            out.push(Rec { ty, offset: p, data: d.to_vec(), breaks: Vec::new() });
        }
        p += 4 + len;
    }
    out
}

/// Reading strings that may go on in a `CONTINUE` record, where their
/// characters start again with a flags byte.
struct Seg<'a> {
    c: Cur<'a>,
    breaks: &'a [usize],
}

impl Seg<'_> {
    fn chars(&mut self, n: usize, mut wide: bool) -> Option<String> {
        let mut out = String::new();
        let mut left = n;
        while left > 0 {
            if self.breaks.binary_search(&self.c.p).is_ok() {
                wide = self.c.u8()? & 1 != 0;
            }
            let end = self.breaks.get(self.breaks.partition_point(|&b| b <= self.c.p)).copied().unwrap_or(self.c.d.len());
            let take = ((end - self.c.p) / if wide { 2 } else { 1 }).min(left);
            if take == 0 {
                return None;
            }
            out.push_str(&self.c.chars8(take, wide)?);
            left -= take;
        }
        Some(out)
    }

    /// An `XLUnicodeRichExtendedString` (shared strings).
    fn rich_string(&mut self) -> Option<String> {
        let n = self.c.u16()? as usize;
        let flags = self.c.u8()?;
        let runs = if flags & 8 != 0 { self.c.u16()? as usize } else { 0 };
        let ext = if flags & 4 != 0 { self.c.u32()? as usize } else { 0 };
        let s = self.chars(n, flags & 1 != 0)?;
        self.c.skip(runs * 4 + ext)?;
        Some(s)
    }
}

/// `XLUnicodeString`: a 16-bit length, flags and the characters.
fn xl_string(c: &mut Cur) -> Option<String> {
    let n = c.u16()? as usize;
    let wide = c.u8()? & 1 != 0;
    c.chars8(n, wide)
}

/// `ShortXLUnicodeString`: an 8-bit length.
fn short_string(c: &mut Cur) -> Option<String> {
    let n = c.u8()? as usize;
    let wide = c.u8()? & 1 != 0;
    c.chars8(n, wide)
}

pub(crate) fn rk(v: u32) -> f64 {
    let n = if v & 2 != 0 { ((v as i32) >> 2) as f64 } else { f64::from_bits(u64::from(v & 0xFFFF_FFFC) << 32) };
    if v & 1 != 0 { n / 100.0 } else { n }
}

/// Where an `EXTERNSHEET` entry points.
enum Book {
    /// This workbook.
    Own,
    /// Add-in functions (the Analysis ToolPak, newer functions).
    Addin(Vec<String>),
    /// Another workbook: its place in `Globals::externals` and the names
    /// defined in it that formulas use.
    File(usize, Vec<String>),
    /// Another file (DDE and OLE links).
    Other,
}

/// The path of an external workbook as `SUPBOOK` encodes it (`\x01C` for
/// drive C:, `\x03` between folders...).
fn decode_path(p: &str) -> String {
    let Some(rest) = p.strip_prefix('\u{1}') else { return p.to_string() };
    let mut out = String::new();
    let mut it = rest.chars();
    while let Some(c) = it.next() {
        match c {
            '\u{1}' => match it.next() {
                Some('@') => out.push_str("\\\\"),
                Some(d) => {
                    out.push(d);
                    out.push_str(":\\");
                }
                None => {}
            },
            '\u{2}' | '\u{3}' => out.push('\\'),
            '\u{4}' => out.push_str("..\\"),
            '\u{5}' => {
                it.next();
            }
            '\u{6}' | '\u{7}' | '\u{8}' => {}
            c => out.push(c),
        }
    }
    out
}

/// A cached value of an external cell (`SerAr` in `CRN`).
fn ser_value(c: &mut Cur) -> Option<Value> {
    Some(match c.u8()? {
        0x01 => Value::Number(f64::from_le_bytes(c.bytes(8)?.try_into().ok()?)),
        0x02 => Value::text(xl_string(c)?),
        0x04 => {
            let b = c.u8()? != 0;
            c.skip(7)?;
            Value::Bool(b)
        }
        0x10 => {
            let e = c.u8()?;
            c.skip(7)?;
            Value::Error(biff::error_kind(e))
        }
        _ => {
            c.skip(8)?;
            Value::Empty
        }
    })
}

struct Font {
    name: String,
    size: u16,
    bold: bool,
    italic: bool,
    strike: bool,
    underline: bool,
    color: u16,
}

struct Globals {
    sheets: Vec<(String, usize, bool, u8)>,
    strings: Vec<Arc<str>>,
    formats: HashMap<u32, String>,
    fonts: Vec<Font>,
    xfs: Vec<Vec<u8>>,
    palette: [u32; 56],
    date1904: bool,
    /// PRECISION off: Precision as displayed.
    as_shown: bool,
    books: Vec<Book>,
    /// The other workbooks formulas read, with their cached cells.
    externals: Vec<apex_sheet::ExternalBook>,
    /// Book, first and last sheet.
    xti: Vec<(u16, i16, i16)>,
    /// Name, local sheet, tokens, hidden, built-in.
    names: Vec<(String, u16, Vec<u8>, Vec<u8>, bool, bool)>,
    /// Excel 5.0/95 rather than 97-2003.
    biff5: bool,
    /// The code page of BIFF5's 8-bit strings.
    cp: u16,
    /// The workbook's drawing data (`MSODRAWINGGROUP`), with the pictures.
    drawings: Vec<u8>,
}

fn builtin_name(code: u8) -> &'static str {
    match code {
        0 => "Consolidate_Area",
        1 => "Auto_Open",
        2 => "Auto_Close",
        3 => "Extract",
        4 => "Database",
        5 => "Criteria",
        6 => "Print_Area",
        7 => "Print_Titles",
        8 => "Recorder",
        9 => "Data_Form",
        10 => "Auto_Activate",
        11 => "Auto_Deactivate",
        12 => "Sheet_Title",
        _ => "_FilterDatabase",
    }
}

/// A BIFF5 `XF` in BIFF8's layout.
fn xf_from_biff5(x: &[u8]) -> Vec<u8> {
    if x.len() < 16 {
        return x.to_vec();
    }
    let u32_at = |i: usize| u32::from_le_bytes([x[i], x[i + 1], x[i + 2], x[i + 3]]);
    let align = u16::from_le_bytes([x[6], x[7]]);
    let (w8, w12) = (u32_at(8), u32_at(12));
    let rot: u8 = match (align >> 8) & 3 {
        1 => 255,
        2 => 90,
        3 => 180,
        _ => 0,
    };
    let b1 = (w12 >> 3 & 7) | (w12 >> 6 & 7) << 4 | (w12 & 7) << 8 | (w8 >> 22 & 7) << 12 | (w12 >> 16 & 0x7F) << 16 | (w12 >> 23 & 0x7F) << 23;
    let b2 = (w12 >> 9 & 0x7F) | (w8 >> 25 & 0x7F) << 7 | (w8 >> 16 & 0x3F) << 26;
    let fill = (w8 & 0x3FFF) as u16;
    let mut out = x[..6].to_vec();
    out.extend_from_slice(&[(align & 0x7F) as u8, rot, 0, 0]);
    out.extend_from_slice(&b1.to_le_bytes());
    out.extend_from_slice(&b2.to_le_bytes());
    out.extend_from_slice(&fill.to_le_bytes());
    out
}

fn read_globals(recs: &[Rec], biff5: bool) -> Result<Globals, Error> {
    let mut g = Globals {
        sheets: Vec::new(),
        strings: Vec::new(),
        formats: HashMap::new(),
        fonts: Vec::new(),
        xfs: Vec::new(),
        palette: biff::PALETTE,
        date1904: false,
        as_shown: false,
        books: Vec::new(),
        externals: Vec::new(),
        xti: Vec::new(),
        names: Vec::new(),
        biff5,
        cp: 1252,
        drawings: Vec::new(),
    };
    // The external sheet whose cells the next `CRN` records hold.
    let mut xct: Option<(usize, usize)> = None;
    for r in recs {
        let mut c = Cur::new(&r.data);
        match r.ty {
            0x000A => break,
            0x0022 => g.date1904 = c.u16() == Some(1),
            0x000E => g.as_shown = c.u16() == Some(0),
            0x0042 => g.cp = c.u16().unwrap_or(1252),
            0x0085 => {
                let pos = c.u32().unwrap_or(0) as usize;
                let hidden = c.u8().unwrap_or(0) & 3 != 0;
                let kind = c.u8().unwrap_or(0);
                if let Some(name) = g.text(&mut c, false) {
                    g.sheets.push((name, pos, hidden, kind));
                }
            }
            0x00FC => {
                let mut s = Seg { c, breaks: &r.breaks };
                s.c.skip(4);
                let n = s.c.u32().unwrap_or(0);
                for _ in 0..n {
                    match s.rich_string() {
                        Some(t) => g.strings.push(Arc::from(t)),
                        None => break,
                    }
                }
            }
            0x041E => {
                if let (Some(id), Some(code)) = (c.u16(), g.text(&mut c, !g.biff5)) {
                    g.formats.insert(u32::from(id), code);
                }
            }
            0x0031 => {
                let f = (|| {
                    let size = c.u16()?;
                    let flags = c.u16()?;
                    let color = c.u16()?;
                    let weight = c.u16()?;
                    c.skip(2)?;
                    let underline = c.u8()? != 0;
                    c.skip(3)?;
                    let name = g.text(&mut c, false)?;
                    Some(Font { name, size, bold: weight >= 600, italic: flags & 2 != 0, strike: flags & 8 != 0, underline, color })
                })();
                if let Some(f) = f {
                    g.fonts.push(f);
                }
            }
            0x00E0 => g.xfs.push(if biff5 { xf_from_biff5(&r.data) } else { r.data.clone() }),
            0x0092 => {
                let n = c.u16().unwrap_or(0) as usize;
                for i in 0..n.min(56) {
                    if let Some(b) = c.bytes(4) {
                        g.palette[i] = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
                    }
                }
            }
            0x01AE => {
                let sheets = c.u16().unwrap_or(0);
                xct = None;
                g.books.push(match c.u16() {
                    Some(0x0401) => Book::Own,
                    Some(0x3A01) => Book::Addin(Vec::new()),
                    // Another workbook: its path and sheet names.
                    Some(n) if sheets > 0 && !biff5 => {
                        let book = (|| {
                            let wide = c.u8()? & 1 != 0;
                            let target = decode_path(&c.chars8(n as usize, wide)?);
                            let names: Vec<String> = (0..sheets).map_while(|_| xl_string(&mut c)).collect();
                            let cells = vec![Default::default(); names.len()];
                            Some(apex_sheet::ExternalBook { target, sheets: names, names: Vec::new(), cells })
                        })();
                        match book {
                            Some(b) => {
                                g.externals.push(b);
                                Book::File(g.externals.len() - 1, Vec::new())
                            }
                            None => Book::Other,
                        }
                    }
                    _ => Book::Other,
                });
            }
            0x0023 => match g.books.last_mut() {
                Some(Book::Addin(list) | Book::File(_, list)) => {
                    c.skip(6);
                    list.push(short_string(&mut c).unwrap_or_default());
                }
                _ => {}
            },
            // The cached cells of a sheet of the last external workbook.
            0x0059 if !biff5 => {
                let _count = c.u16();
                xct = match (g.books.last(), c.u16()) {
                    (Some(Book::File(b, _)), Some(sheet)) => Some((*b, sheet as usize)),
                    _ => None,
                };
            }
            0x005A => {
                if let Some((b, sheet)) = xct
                    && let (Some(last), Some(first), Some(row)) = (c.u8(), c.u8(), c.u16())
                    && let Some(cells) = g.externals.get_mut(b).and_then(|x| x.cells.get_mut(sheet))
                {
                    for col in first..=last {
                        let Some(v) = ser_value(&mut c) else { break };
                        if !matches!(v, Value::Empty) {
                            cells.insert(CellRef::new(u32::from(row), u32::from(col)), v);
                        }
                    }
                }
            }
            0x0017 if !biff5 => {
                let n = c.u16().unwrap_or(0);
                for _ in 0..n {
                    let (Some(b), Some(first), Some(last)) = (c.u16(), c.u16(), c.u16()) else { break };
                    g.xti.push((b, first as i16, last as i16));
                }
            }
            0x00EB => g.drawings.extend_from_slice(&r.data),
            0x0018 => {
                let n = (|| {
                    let flags = c.u16()?;
                    c.skip(1)?;
                    let len = c.u8()? as usize;
                    let cce = c.u16()? as usize;
                    c.skip(2)?;
                    let local = c.u16()?;
                    c.skip(4)?;
                    // BIFF5 names have no flags byte and are 8-bit.
                    let wide = !g.biff5 && c.u8()? & 1 != 0;
                    let builtin = flags & 0x20 != 0;
                    let name = if builtin {
                        let code = if wide { c.u16()? as u8 } else { c.u8()? };
                        builtin_name(code).to_string()
                    } else if g.biff5 {
                        crate::ole::decode_cp(c.bytes(len)?, g.cp)
                    } else {
                        c.chars8(len, wide)?
                    };
                    let rgce = c.bytes(cce.min(c.left()))?.to_vec();
                    // Array constants (`={1,2;3,4}`) keep their values after the tokens.
                    let rgcb = c.bytes(c.left())?.to_vec();
                    Some((name, local, rgce, rgcb, flags & 1 != 0, builtin))
                })();
                g.names.push(n.unwrap_or_default());
            }
            _ => {}
        }
    }
    Ok(g)
}

impl Globals {
    /// A string with an 8- or 16-bit length: `ShortXLUnicodeString` and
    /// `XLUnicodeString`, or in BIFF5 bytes in the code page.
    fn text(&self, c: &mut Cur, len16: bool) -> Option<String> {
        if self.biff5 {
            let n = if len16 { c.u16()? as usize } else { c.u8()? as usize };
            Some(crate::ole::decode_cp(c.bytes(n.min(c.left()))?, self.cp))
        } else if len16 {
            xl_string(c)
        } else {
            short_string(c)
        }
    }

    fn color(&self, i: u16) -> Option<u32> {
        match i {
            0..=7 => Some([0x000000, 0xFFFFFF, 0xFF0000, 0x00FF00, 0x0000FF, 0xFFFF00, 0xFF00FF, 0x00FFFF][i as usize]),
            8..=63 => Some(self.palette[i as usize - 8]),
            _ => None,
        }
    }

    fn font(&self, i: u16) -> Option<&Font> {
        // Index 4 doesn't exist.
        self.fonts.get(if i >= 4 { i as usize - 1 } else { i as usize })
    }

    fn style(&self, x: &[u8], wb: &Workbook) -> CellStyle {
        let mut s = CellStyle::default();
        let mut c = Cur::new(x);
        let (Some(font), Some(fmt), Some(prot), Some(align), Some(rot), Some(indent), Some(_), Some(b1), Some(b2), Some(fill)) =
            (c.u16(), c.u16(), c.u16(), c.u8(), c.u8(), c.u8(), c.u8(), c.u32(), c.u32(), c.u16())
        else {
            return s;
        };
        s.num_fmt = Arc::from(crate::xlsx_styles::numfmt_code(u32::from(fmt), &self.formats, wb));
        let base = self.fonts.first();
        if let Some(f) = self.font(font) {
            if base.is_some_and(|b| b.name != f.name) {
                s.font = Some(Arc::from(f.name.as_str()));
            }
            if base.is_some_and(|b| b.size != f.size) && f.size > 0 {
                s.font_size = f.size / 2;
            }
            s.bold = f.bold;
            s.italic = f.italic;
            s.strike = f.strike;
            s.underline = f.underline;
            s.color = self.color(f.color).filter(|&c| c != 0);
        }
        s.unlocked = prot & 1 == 0;
        s.formula_hidden = prot & 2 != 0;
        s.halign = match align & 7 {
            1 => HAlign::Left,
            2 | 6 => HAlign::Center,
            3 => HAlign::Right,
            5 | 7 => HAlign::Justify,
            _ => HAlign::General,
        };
        s.wrap = align & 8 != 0;
        s.valign = match (align >> 4) & 7 {
            0 => VAlign::Top,
            1 | 3 | 4 => VAlign::Center,
            _ => VAlign::Bottom,
        };
        s.rotation = match rot {
            255 => ROTATION_STACKED,
            v @ 1..=90 => i16::from(v),
            v @ 91..=180 => -(i16::from(v) - 90),
            _ => 0,
        };
        s.indent = indent & 15;
        let line = |style: u32, color: u32| {
            let st = match style {
                0 => LineStyle::None,
                1 => LineStyle::Thin,
                2 | 8 | 10 | 12 => LineStyle::Medium,
                3 | 9 | 11 | 13 => LineStyle::Dashed,
                4 => LineStyle::Dotted,
                5 => LineStyle::Thick,
                6 => LineStyle::Double,
                _ => LineStyle::Hair,
            };
            Line::new(st, self.color(color as u16).filter(|&c| c != 0 && st != LineStyle::None))
        };
        s.borders = Borders { left: line(b1 & 15, (b1 >> 16) & 0x7F), right: line((b1 >> 4) & 15, (b1 >> 23) & 0x7F), top: line((b1 >> 8) & 15, b2 & 0x7F), bottom: line((b1 >> 12) & 15, (b2 >> 7) & 0x7F) };
        if b2 >> 26 == 1 {
            s.fill = self.color(fill & 0x7F);
        }
        s
    }

    /// The sheet prefix of an `EXTERNSHEET` entry (BIFF5: of the first
    /// and last sheet indexes).
    fn sheet_ref(&self, ixti: u32, map: &[Option<usize>], wb: &Workbook) -> Option<String> {
        let (first, last) = if self.biff5 {
            ((ixti & 0xFFFF) as u16 as i16, (ixti >> 16) as u16 as i16)
        } else {
            let &(book, first, last) = self.xti.get(ixti as usize)?;
            match self.books.get(book as usize)? {
                Book::Own => {}
                // `'[1]Sheet'!A1`: numbered as in `Workbook::externals`.
                Book::File(b, _) => {
                    let x = self.externals.get(*b)?;
                    let (a, z) = (x.sheets.get(usize::try_from(first).ok()?)?, x.sheets.get(usize::try_from(last).ok()?)?);
                    let s = if a == z { format!("[{}]{a}", b + 1) } else { format!("[{}]{a}:{z}", b + 1) };
                    return Some(format!("'{}'", s.replace('\'', "''")));
                }
                _ => return None,
            }
            (first, last)
        };
        if first < 0 || last < 0 {
            return None;
        }
        let name = |i: i16| map.get(i as usize).copied().flatten().map(|j| wb.sheets[j].name.clone());
        let (a, b) = (name(first)?, name(last)?);
        Some(if a == b { biff::quote_sheet(&a) } else { biff::quote_sheet(&format!("{a}:{b}")) })
    }

    /// Formula text of tokens for a base cell, without extra data.
    fn formula(&self, rgce: &[u8], base: CellRef, map: &[Option<usize>], names: &[String], wb: &Workbook) -> Option<String> {
        let sheet = |i: u32| self.sheet_ref(i, map, wb);
        let name_of = |i: u32| names.get((i as usize).checked_sub(1)?).cloned();
        let ext = |i: u16, n: u32| self.extern_name(i, n);
        let r = Refs { biff12: false, biff5: self.biff5, base, in_name: false, sheet: &sheet, name: &name_of, extern_name: &ext };
        biff::decompile(rgce, &[], &r)
    }

    fn extern_name(&self, ixti: u16, i: u32) -> Option<String> {
        let &(book, _, _) = self.xti.get(ixti as usize)?;
        let name = |list: &[String]| list.get((i as usize).checked_sub(1)?).cloned().filter(|n| !n.is_empty());
        match self.books.get(book as usize)? {
            Book::Addin(list) => name(list),
            Book::File(b, list) => Some(format!("[{}]!{}", b + 1, name(list)?)),
            _ => None,
        }
    }
}

/// A formula cell waiting for the shared formulas of its sheet.
enum Fsrc {
    Tokens(Vec<u8>, Vec<u8>),
    Shared(CellRef),
}

/// The workbook in an .xls file (the whole compound file).
pub fn read(data: &[u8]) -> Result<Workbook, Error> {
    read_with(data, None)
}

/// The workbook in an .xls file that may be encrypted with `password`.
pub fn read_with(data: &[u8], password: Option<&str>) -> Result<Workbook, Error> {
    // Excel 2003 XML saved as .xls.
    if crate::xml2003::is_xml(data) {
        return crate::xml2003::read(data);
    }
    // A bare BIFF stream, without the compound file (Excel 2.x to 4.0
    // files, and some programs' exports).
    let bare = crate::xls_old::is_bare_biff(data) && !data.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]);
    let mut ole = if bare { None } else { Some(crate::ole::open(data)?) };
    let stream = match ole.as_mut() {
        Some(ole) => crate::ole::stream(ole, "/Workbook").or_else(|| crate::ole::stream(ole, "/Book")),
        None => Some(data.to_vec()),
    };
    let Some(mut stream) = stream else { return Err(Error::new("not an Excel workbook")) };
    if crate::xls_old::is_old_bof(&stream) {
        return crate::xls_old::read(&stream);
    }
    // The low byte of the version varies (0x0500, 0x0580...).
    let biff5 = match stream.get(..6) {
        Some([0x09, 0x08, _, _, _, 0x06]) => false,
        Some([0x09, 0x08, _, _, _, 0x05]) => true,
        // Other versions (some programs write 0): BIFF8 has the longer BOF.
        Some([0x09, 0x08, len, 0, _, _]) => *len < 16,
        _ => return Err(Error::new("only Excel 2.x to 2003 workbooks (BIFF2 to BIFF8) are supported")),
    };
    crate::bin_crypto::decrypt_biff(&mut stream, biff5, password)?;
    let recs = records(&stream);
    let mut g = read_globals(&recs, biff5)?;
    // A bare worksheet stream without the workbook globals (no sheet list):
    // the stream is the one sheet.
    if g.sheets.is_empty() && ole.is_none() {
        g.sheets.push(("Sheet1".to_string(), 0, false, 0));
    }
    let mut wb = Workbook::empty();
    let xf_ids: Vec<u32> = g.xfs.iter().map(|x| {
        let s = g.style(x, &wb);
        wb.styles.intern(s)
    }).collect();
    let style_of = |i: u16| xf_ids.get(i as usize).copied().unwrap_or(0);
    // Worksheets only; the file's sheet index -> ours.
    let mut map: Vec<Option<usize>> = Vec::new();
    // A sheet listed again (same name or same place) is read once, as
    // Excel repairs it.
    let mut seen = std::collections::HashSet::new();
    let mut places = std::collections::HashSet::new();
    for (name, pos, hidden, kind) in &g.sheets {
        if *kind == 0 && wb.sheets.len() < 10_000 && !seen.contains(&name.to_lowercase()) && places.insert(*pos) {
            seen.insert(name.to_lowercase());
            let mut sh = apex_sheet::Sheet::new(name.clone());
            sh.hidden = *hidden;
            map.push(Some(wb.sheets.len()));
            wb.sheets.push(sh);
        } else {
            map.push(None);
        }
    }
    // Only chart or macro sheets: one empty sheet, as LibreOffice opens it
    // (the chart sheets themselves are not read).
    if wb.sheets.is_empty() {
        if g.sheets.is_empty() {
            return Err(Error::new("the workbook has no worksheets"));
        }
        wb.sheets.push(apex_sheet::Sheet::new("Sheet1"));
    }
    let names: Vec<String> = g.names.iter().map(|n| n.0.clone()).collect();
    let blips = crate::xls_art::blips(&g.drawings);
    // Autofilter columns by sheet, placed by the _FilterDatabase names.
    let mut filters: Vec<Option<crate::xls_data::Columns>> = vec![None; wb.sheets.len()];
    let mut at: HashMap<usize, usize> = HashMap::new();
    for (i, r) in recs.iter().enumerate() {
        at.entry(r.offset).or_insert(i);
    }
    // Each substream's end (past its EOF), nested ones too.
    let mut ends: HashMap<usize, usize> = HashMap::new();
    let mut open = Vec::new();
    for (i, r) in recs.iter().enumerate() {
        match r.ty {
            0x0809 => open.push(i),
            0x000A => {
                if let Some(s) = open.pop() {
                    ends.insert(s, i + 1);
                }
            }
            _ => {}
        }
    }
    // Sheets read at most the records twice over (sheets placed inside
    // one another are not read again and again).
    let mut left = recs.len().saturating_mul(2);
    for (fi, (_, pos, _, _)) in g.sheets.iter().enumerate() {
        let Some(si) = map[fi] else { continue };
        let Some(&start) = at.get(pos) else { continue };
        if recs[start].ty != 0x0809 && start != 0 {
            continue;
        }
        let end = ends.get(&start).copied().unwrap_or(recs.len());
        let Some(l) = left.checked_sub(end - start) else { continue };
        left = l;
        let recs = &recs[start..end];
        filters[si] = read_sheet(recs, &g, &mut wb, si, &map, &names, &style_of, &blips);
        wb.sheets[si].protection = crate::xls_protect::sheet(recs);
        crate::scenario_io::read_xls(recs, &mut wb, si, biff5);
    }
    // Defined names and print areas.
    let mut user = Vec::new();
    for (name, local, rgce, rgcb, hidden, builtin) in &g.names {
        let text = {
            let sheet = |i: u32| g.sheet_ref(i, &map, &wb);
            let name_of = |i: u32| names.get((i as usize).checked_sub(1)?).cloned();
            let ext = |i: u16, n: u32| g.extern_name(i, n);
            let r = Refs { biff12: false, biff5: g.biff5, base: CellRef::new(0, 0), in_name: true, sheet: &sheet, name: &name_of, extern_name: &ext };
            biff::decompile(rgce, rgcb, &r)
        };
        let Some(text) = text else { continue };
        let local = (*local > 0).then(|| map.get(*local as usize - 1).copied().flatten()).flatten();
        if *builtin {
            if name.starts_with("Print_") {
                crate::xlsx_page::apply_defined_name(&mut wb, &format!("_xlnm.{name}"), local, &text);
            } else if name == "_FilterDatabase"
                && let Some(si) = local
                && let Some(cols) = filters[si].take()
            {
                let area = text.rsplit('!').next().unwrap_or("").replace('$', "");
                if let Some(range) = Range::parse(&area) {
                    wb.sheets[si].filter = Some(crate::xls_data::filter(range, cols));
                }
            }
        } else if !name.is_empty() && !name.starts_with("_xlfn.") {
            // "_xlfn." names stand for newer functions, not user names.
            user.push((name.clone(), local, text, String::new(), *hidden));
        }
    }
    crate::xlsx::read_user_names(&mut wb, user);
    wb.externals = std::mem::take(&mut g.externals);
    wb.refresh_cell_styles();
    wb.calc.as_shown = g.as_shown;
    if g.date1904 {
        wb.shift_dates(apex_sheet::workbook::DAYS_1904);
    }
    if let Some(ole) = ole.as_mut() {
        wb.meta = crate::ole::read_meta(ole);
    }
    wb.macros = legacy_macros(data, "_VBA_PROJECT_CUR");
    crate::xls_protect::globals(&recs, &mut wb);
    wb.finish_load();
    Ok(wb)
}

/// The VBA project of a legacy file (in storage `storage`), kept to be
/// saved as .xlsm or .docm.
pub(crate) fn legacy_macros(data: &[u8], storage: &str) -> apex_doc::Macros {
    match crate::vba::from_legacy(data, storage) {
        Some(bin) => apex_doc::Macros { project: Some(std::sync::Arc::new(bin)), ..Default::default() },
        None => apex_doc::Macros::default(),
    }
}

/// Reads a sheet's records; returns its autofilter's columns (placed later
/// by its `_FilterDatabase` name).
#[allow(clippy::too_many_arguments)]
fn read_sheet(
    recs: &[Rec],
    g: &Globals,
    wb: &mut Workbook,
    si: usize,
    map: &[Option<usize>],
    names: &[String],
    style_of: &dyn Fn(u16) -> u32,
    blips: &[Option<(Arc<[u8]>, ImageFormat)>],
) -> Option<crate::xls_data::Columns> {
    let mut formulas: Vec<(CellRef, u32, Value, Fsrc)> = Vec::new();
    let mut shared: HashMap<CellRef, (Vec<u8>, Vec<u8>)> = HashMap::new();
    // The areas of the shared formulas, in the order read.
    let mut shared_areas: Vec<Range> = Vec::new();
    let mut arrays: HashMap<CellRef, (Vec<u8>, Vec<u8>, Range)> = HashMap::new();
    // The formula whose text result comes in the next STRING record.
    let mut want_string: Option<usize> = None;
    let mut frozen = false;
    // Note texts by object id, and the last object's id.
    let mut texts: HashMap<u16, String> = HashMap::new();
    let mut last_obj: Option<u16> = None;
    let mut notes: Vec<(CellRef, u16, String, bool)> = Vec::new();
    let mut links: Vec<(Range, Link)> = Vec::new();
    let mut link_bytes = 0usize;
    let mut tools = crate::xls_data::Tools::default();
    // The drawing (all MSODRAWING data), the objects in order (type, and
    // the chart that followed) and the substream being skipped.
    let mut drawing: Vec<u8> = Vec::new();
    let mut objs: Vec<(u16, Option<Chart>)> = Vec::new();
    let mut skip_to = 0;
    let color = |i: u16| g.color(i);
    let put = |wb: &mut Workbook, row: u16, col: u16, v: Value, xf: u16| {
        let cell = CellRef::new(u32::from(row), u32::from(col));
        let style = style_of(xf);
        let content = if v == Value::Empty { Content::Empty } else { Content::Value(v) };
        if !matches!(content, Content::Empty) || style != 0 {
            wb.load_cell(si, cell, content, style);
        }
    };
    for (i, r) in recs.iter().enumerate().skip(1) {
        if i < skip_to {
            continue;
        }
        let mut c = Cur::new(&r.data);
        let d = &r.data;
        match r.ty {
            0x000A => break,
            0x0809 => {
                // An embedded chart's substream, up to its own EOF.
                let mut depth = 0;
                let end = recs[i..].iter().position(|x| {
                    match x.ty {
                        0x0809 => depth += 1,
                        0x000A => depth -= 1,
                        _ => {}
                    }
                    depth == 0
                });
                let end = end.map_or(recs.len(), |e| i + e);
                skip_to = end + 1;
                let is_chart = d.get(2..4) == Some(&[0x20, 0x00]);
                if let (true, Some(o)) = (is_chart, objs.last_mut()) {
                    let wbr: &Workbook = wb;
                    let fmla = |rgce: &[u8], base: CellRef| g.formula(rgce, base, map, names, wbr);
                    o.1 = crate::xls_chart::read(&recs[i + 1..end], &fmla);
                }
            }
            0x00EC => drawing.extend_from_slice(d),
            // WSBOOL: alternate expression evaluation (Lotus 1-2-3 rules).
            0x0081 => wb.sheets[si].lotus_eval = c.u16().is_some_and(|f| f & 0x4000 != 0),
            // Calculation settings (each sheet repeats the workbook's).
            0x000D => wb.calc.manual = c.u16() == Some(0),
            0x0011 => wb.calc.iterate = c.u16() == Some(1),
            0x000C => {
                if let Some(n) = c.u16().filter(|n| *n > 0) {
                    wb.calc.max_iter = u32::from(n);
                }
            }
            0x0010 => {
                if let Some(x) = c.bytes(8).and_then(|b| b.try_into().ok()).map(f64::from_le_bytes).filter(|x| x.is_finite() && *x > 0.0) {
                    wb.calc.max_change = x;
                }
            }
            0x01B0 | 0x0879 => tools.condfmt(d, r.ty == 0x0879),
            0x01B1 | 0x087A => {
                let wbr: &Workbook = wb;
                let fmla = |rgce: &[u8], base: CellRef| g.formula(rgce, base, map, names, wbr);
                if r.ty == 0x01B1 {
                    tools.cf(d, &fmla, &color);
                } else {
                    tools.cf12(d, &fmla, &color);
                }
            }
            0x087B => tools.cfex(d, &color),
            0x01BE if !g.biff5 => {
                let wbr: &Workbook = wb;
                let fmla = |rgce: &[u8], base: CellRef| g.formula(rgce, base, map, names, wbr);
                tools.dv(d, &fmla);
            }
            0x009D => tools.filter = Some(Vec::new()),
            0x009E => {
                let text = |c: &mut Cur, n: usize| -> Option<String> {
                    if g.biff5 {
                        return Some(crate::ole::decode_cp(c.bytes(n)?, g.cp));
                    }
                    let wide = c.u8()? & 1 != 0;
                    c.chars8(n, wide)
                };
                tools.autofilter(d, &text);
            }
            0x0203 => {
                if let (Some(row), Some(col), Some(xf), Some(v)) = (c.u16(), c.u16(), c.u16(), c.f64()) {
                    put(wb, row, col, Value::Number(v), xf);
                }
            }
            0x027E => {
                if let (Some(row), Some(col), Some(xf), Some(v)) = (c.u16(), c.u16(), c.u16(), c.u32()) {
                    put(wb, row, col, Value::Number(rk(v)), xf);
                }
            }
            0x00BD => {
                if let (Some(row), Some(first)) = (c.u16(), c.u16()) {
                    let n = c.left().saturating_sub(2) / 6;
                    for i in 0..n {
                        if let (Some(xf), Some(v)) = (c.u16(), c.u32()) {
                            put(wb, row, first + i as u16, Value::Number(rk(v)), xf);
                        }
                    }
                }
            }
            0x00FD => {
                if let (Some(row), Some(col), Some(xf), Some(i)) = (c.u16(), c.u16(), c.u16(), c.u32()) {
                    let v = g.strings.get(i as usize).map(|s| Value::Text(s.clone())).unwrap_or(Value::Empty);
                    put(wb, row, col, v, xf);
                }
            }
            0x0204 | 0x00D6 => {
                // LABEL (and BIFF5's RSTRING, with formatting runs after).
                if let (Some(row), Some(col), Some(xf), Some(s)) = (c.u16(), c.u16(), c.u16(), g.text(&mut c, true)) {
                    put(wb, row, col, Value::text(s), xf);
                }
            }
            0x0205 => {
                if let (Some(row), Some(col), Some(xf), Some(v), Some(err)) = (c.u16(), c.u16(), c.u16(), c.u8(), c.u8()) {
                    let v = if err != 0 { Value::Error(biff::error_kind(v)) } else { Value::Bool(v != 0) };
                    put(wb, row, col, v, xf);
                }
            }
            0x0201 => {
                if let (Some(row), Some(col), Some(xf)) = (c.u16(), c.u16(), c.u16()) {
                    put(wb, row, col, Value::Empty, xf);
                }
            }
            0x00BE => {
                if let (Some(row), Some(first)) = (c.u16(), c.u16()) {
                    let n = c.left().saturating_sub(2) / 2;
                    for i in 0..n {
                        if let Some(xf) = c.u16() {
                            put(wb, row, first + i as u16, Value::Empty, xf);
                        }
                    }
                }
            }
            0x0006 => {
                let f = (|| {
                    let row = c.u16()?;
                    let col = c.u16()?;
                    let xf = c.u16()?;
                    let v = c.bytes(8)?;
                    c.skip(6)?;
                    let cce = c.u16()? as usize;
                    let rgce = c.bytes(cce.min(c.left()))?.to_vec();
                    let rgcb = c.bytes(c.left())?.to_vec();
                    let value = if v[6] == 0xFF && v[7] == 0xFF {
                        match v[0] {
                            1 => Value::Bool(v[2] != 0),
                            2 => Value::Error(biff::error_kind(v[2])),
                            3 => Value::text(""),
                            _ => Value::Empty,
                        }
                    } else {
                        Value::Number(f64::from_le_bytes(v.try_into().ok()?))
                    };
                    let text_next = v[6] == 0xFF && v[7] == 0xFF && v[0] == 0;
                    let cell = CellRef::new(u32::from(row), u32::from(col));
                    let src = if rgce.len() == 5 && rgce[0] == 0x01 {
                        Fsrc::Shared(CellRef::new(u32::from(u16::from_le_bytes([rgce[1], rgce[2]])), u32::from(u16::from_le_bytes([rgce[3], rgce[4]]))))
                    } else {
                        Fsrc::Tokens(rgce, rgcb)
                    };
                    Some((cell, style_of(xf), value, src, text_next))
                })();
                if let Some((cell, style, value, src, text_next)) = f {
                    formulas.push((cell, style, value, src));
                    want_string = text_next.then(|| formulas.len() - 1);
                }
            }
            0x0207 => {
                if let (Some(i), Some(s)) = (want_string.take(), g.text(&mut c, true)) {
                    formulas[i].2 = Value::text(s);
                }
            }
            0x04BC | 0x0221 => {
                let f = (|| {
                    let r1 = c.u16()?;
                    let r2 = c.u16()?;
                    let c1 = c.u8()?;
                    let c2 = c.u8()?;
                    // SHRFMLA: reserved + count; ARRAY: options + 4 unused.
                    c.skip(if r.ty == 0x04BC { 2 } else { 6 })?;
                    let cce = c.u16()? as usize;
                    let rgce = c.bytes(cce.min(c.left()))?.to_vec();
                    let rgcb = c.bytes(c.left())?.to_vec();
                    Some((Range::new(CellRef::new(u32::from(r1), u32::from(c1)), CellRef::new(u32::from(r2), u32::from(c2))), rgce, rgcb))
                })();
                if let Some((area, rgce, rgcb)) = f {
                    if r.ty == 0x04BC {
                        shared_areas.push(area);
                        shared.insert(area.start, (rgce, rgcb));
                    } else {
                        arrays.insert(area.start, (rgce, rgcb, area));
                    }
                }
            }
            0x00E5 => {
                let n = c.u16().unwrap_or(0);
                for _ in 0..n {
                    let (Some(r1), Some(r2), Some(c1), Some(c2)) = (c.u16(), c.u16(), c.u16(), c.u16()) else { break };
                    wb.sheets[si].merges.push(Range::new(CellRef::new(u32::from(r1), u32::from(c1)), CellRef::new(u32::from(r2), u32::from(c2))));
                }
            }
            0x007D => {
                if let (Some(first), Some(last), Some(w), Some(_), Some(flags)) = (c.u16(), c.u16(), c.u16(), c.u16(), c.u16()) {
                    let sh = &mut wb.sheets[si];
                    let level = ((flags >> 8) & 7) as u8;
                    for col in u32::from(first)..=u32::from(last.min(first.saturating_add(2000)).min(255)) {
                        if level > 0 {
                            sh.col_levels.insert(col, level);
                        }
                        if flags & 1 != 0 {
                            sh.col_widths.insert(col, 0.0);
                        } else {
                            let px = crate::xlsx::width_from_chars(f32::from(w) / 256.0);
                            if (px - sh.default_col_width).abs() > 1.0 {
                                sh.col_widths.insert(col, px);
                            }
                        }
                    }
                }
            }
            0x0055 => {
                if let Some(w) = c.u16() {
                    wb.sheets[si].default_col_width = crate::xlsx::default_size(crate::xlsx::width_from_chars(f32::from(w) + 0.71), apex_sheet::sheet::DEFAULT_COL_WIDTH);
                }
            }
            0x0208 => {
                if d.len() >= 14 {
                    let row = u32::from(u16::from_le_bytes([d[0], d[1]]));
                    let h = u16::from_le_bytes([d[6], d[7]]) & 0x7FFF;
                    let flags = u16::from_le_bytes([d[12], d[13]]);
                    let sh = &mut wb.sheets[si];
                    let level = (flags & 7) as u8;
                    if level > 0 {
                        sh.row_levels.insert(row, level);
                    }
                    if flags & 0x20 != 0 {
                        sh.row_heights.insert(row, 0.0);
                    } else if flags & 0x40 != 0 {
                        sh.row_heights.insert(row, (f32::from(h) / 20.0 * crate::xlsx::ROW_SCALE).round());
                    }
                }
            }
            0x023E => frozen = d.len() >= 2 && d[0] & 8 != 0,
            0x0041 => {
                if frozen && let (Some(x), Some(y)) = (c.u16(), c.u16()) {
                    wb.sheets[si].frozen_cols = u32::from(x);
                    wb.sheets[si].frozen_rows = u32::from(y);
                }
            }
            0x005D => {
                // ftCmo first: type, size, object type, id.
                if let (Some(_), Some(_), Some(ot), Some(id)) = (c.u16(), c.u16(), c.u16(), c.u16()) {
                    last_obj = (ot == 0x19).then_some(id);
                    objs.push((ot, None));
                }
            }
            0x01B6 if !g.biff5 => {
                if let (Some(id), Some(&b)) = (last_obj, r.breaks.first()) {
                    let n = d.get(10..12).map_or(0, |x| u16::from_le_bytes([x[0], x[1]])) as usize;
                    // The text starts the first CONTINUE, with its flags.
                    let mut s = Seg { c: Cur { d, p: b }, breaks: &r.breaks };
                    if let Some(t) = s.chars(n, false) {
                        texts.insert(id, t.replace('\r', ""));
                    }
                }
            }
            0x001C if g.biff5 => {
                // The text itself, going on in NOTE records with row 0xFFFF.
                if let (Some(row), Some(col), Some(n)) = (c.u16(), c.u16(), c.u16()) {
                    let t = crate::ole::decode_cp(c.bytes((n as usize).min(c.left())).unwrap_or(&[]), g.cp).replace('\r', "");
                    if row == 0xFFFF {
                        if let Some(&(_, id, _, _)) = notes.last() {
                            texts.entry(id).or_default().push_str(&t);
                        }
                    } else {
                        let id = notes.len() as u16;
                        texts.insert(id, t);
                        notes.push((CellRef::new(u32::from(row), u32::from(col)), id, String::new(), false));
                    }
                }
            }
            0x001C => {
                if let (Some(row), Some(col), Some(flags), Some(id), Some(author)) = (c.u16(), c.u16(), c.u16(), c.u16(), xl_string(&mut c)) {
                    notes.push((CellRef::new(u32::from(row), u32::from(col)), id, author, flags & 2 != 0));
                }
            }
            0x01B8 => {
                if let Some(l) = read_hlink(d) {
                    // What the links take is spent below; more read than
                    // that would only take memory first.
                    link_bytes += l.1.target.len() + l.1.tip.len();
                    if link_bytes <= crate::xlsx_notes::LINK_BUDGET {
                        links.push(l);
                    }
                }
            }
            0x0800 => {
                // The tip of the hyperlink before.
                if d.len() > 10 {
                    let mut t = Cur::new(&d[10..]);
                    let tip = t.utf16(t.left() / 2).unwrap_or_default();
                    let tip = tip.trim_end_matches('\0').to_string();
                    if let Some(l) = links.last_mut() {
                        l.1.tip = tip;
                    }
                }
            }
            _ => {}
        }
    }
    // Formulas, now that the shared ones are known.
    let sheet = |i: u32| g.sheet_ref(i, map, wb);
    let name_of = |i: u32| names.get((i as usize).checked_sub(1)?).cloned();
    let ext = |i: u16, n: u32| g.extern_name(i, n);
    let mut out: Vec<(CellRef, u32, Content)> = Vec::new();
    let mut spilled = Vec::new();
    let mut area_of: HashMap<CellRef, Option<Range>> = HashMap::new();
    let mut scans: usize = 1 << 26;
    let mut copies_left = crate::biff::SHARED_COPIES;
    for (cell, style, value, src) in formulas {
        let r = Refs { biff12: false, biff5: g.biff5, base: cell, in_name: false, sheet: &sheet, name: &name_of, extern_name: &ext };
        let (text, array) = match &src {
            Fsrc::Tokens(rgce, rgcb) => (biff::decompile(rgce, rgcb, &r), false),
            // Excel 5 points at the first cell written, not always the
            // corner of the area.
            Fsrc::Shared(anchor) => match (
                shared.get(anchor).or_else(|| {
                    // Bounded: a file of many areas and stray anchors
                    // would otherwise cost areas × formulas.
                    let a = *area_of.entry(*anchor).or_insert_with(|| {
                        scans = scans.checked_sub(shared_areas.len())?;
                        shared_areas.iter().find(|a| a.contains(*anchor)).copied()
                    });
                    a.filter(|a| a.contains(cell)).and_then(|a| shared.get(&a.start))
                }),
                arrays.get(anchor),
            ) {
                (Some((rgce, rgcb)), _) if biff::charge_copy(&mut copies_left, rgce, rgcb) => (biff::decompile(rgce, rgcb, &r), false),
                (Some(_), _) => (None, false),
                // (Many formulas may name one array's corner: each is a copy.)
                (None, Some((rgce, rgcb, area))) if *anchor == cell && biff::charge_copy(&mut copies_left, rgce, rgcb) => {
                    spilled.push(*area);
                    (biff::decompile(rgce, rgcb, &r), true)
                }
                _ => (None, false),
            },
        };
        let expr = text.and_then(|t| parse_canonical(&t).ok()).map(|e| Arc::new(if array { e } else { e.legacy() }));
        let content = match expr {
            Some(expr) => Content::Formula { expr: expr.into(), value },
            None if value == Value::Empty => Content::Empty,
            None => Content::Value(value),
        };
        out.push((cell, style, content));
    }
    for (cell, style, content) in out {
        wb.load_cell(si, cell, content, style);
    }
    let mut work = 0;
    for area in spilled {
        if work > crate::xlsx::MAX_ARRAY_WORK {
            break;
        }
        work += wb.sheets[si].mark_fixed_array(&area);
    }
    for (cell, id, author, visible) in notes {
        let text = crate::xlsx_notes::without_author(&texts.remove(&id).unwrap_or_default(), &author);
        wb.sheets[si].edit_extra(cell, |x| x.note = Some(Note { text, author, visible, ..Default::default() }));
    }
    let mut budget = crate::xlsx_notes::LINK_BUDGET;
    for (area, link) in links {
        for (c, l) in crate::xlsx_notes::spread_link(&area, link, &mut budget) {
            wb.sheets[si].edit_extra(c, |x| x.link = Some(l));
        }
    }
    let filter = tools.filter.take();
    tools.finish(&mut wb.sheets[si]);
    read_drawing(&mut wb.sheets[si], &drawing, objs, blips);
    filter
}

/// Pictures and charts: the shapes with client data go with the `OBJ`
/// records in order.
fn read_drawing(sh: &mut Sheet, drawing: &[u8], objs: Vec<(u16, Option<Chart>)>, blips: &[Option<(Arc<[u8]>, ImageFormat)>]) {
    let shapes = crate::xls_art::shapes(drawing);
    let mut objs = objs.into_iter();
    for s in shapes.iter().filter(|s| s.client) {
        let Some((ot, chart)) = objs.next() else { break };
        match (ot, chart) {
            (0x05, Some(mut chart)) if !s.child => {
                let Some(a) = &s.anchor else { continue };
                let (from, dx, dy, w, h, anchor) = crate::xls_art::place(sh, a);
                (chart.from, chart.dx, chart.dy, chart.width, chart.height, chart.anchor) = (from, dx, dy, w, h, anchor);
                chart.name = s.name.clone();
                chart.alt = s.descr.clone();
                chart.id = sh.charts.len() as u32 + 1;
                if chart.name.is_empty() {
                    chart.name = format!("Chart {}", chart.id);
                }
                chart.sanitize();
                sh.charts.push(chart);
            }
            (0x08, _) => {
                if let Some(mut img) = crate::xls_art::image(sh, s, blips) {
                    img.id = sh.images.len() as u32 + 1;
                    sh.images.push(img);
                }
            }
            _ => {}
        }
    }
}

/// A `HyperlinkString`: a character count (with the final 0) and UTF-16.
fn hl_string(c: &mut Cur) -> Option<String> {
    let n = c.u32()? as usize;
    Some(c.utf16(n.min(c.left() / 2))?.trim_end_matches('\0').to_string())
}

const URL_MONIKER: [u8; 16] = [0xE0, 0xC9, 0xEA, 0x79, 0xF9, 0xBA, 0xCE, 0x11, 0x8C, 0x82, 0x00, 0xAA, 0x00, 0x4B, 0xA9, 0x0B];
const FILE_MONIKER: [u8; 16] = [0x03, 0x03, 0, 0, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46];

fn read_hlink(d: &[u8]) -> Option<(Range, Link)> {
    let mut c = Cur::new(d);
    let (r1, r2, c1, c2) = (c.u16()?, c.u16()?, c.u16()?, c.u16()?);
    c.skip(16 + 4)?;
    let flags = c.u32()?;
    if flags & 0x10 != 0 {
        hl_string(&mut c)?;
    }
    if flags & 0x80 != 0 {
        hl_string(&mut c)?;
    }
    let mut target = String::new();
    if flags & 0x01 != 0 {
        if flags & 0x100 != 0 {
            target = hl_string(&mut c)?;
        } else {
            let clsid = c.bytes(16)?;
            if clsid == URL_MONIKER {
                let n = c.u32()? as usize;
                let url = c.bytes(n.min(c.left()))?;
                let mut u = Cur::new(url);
                target = u.utf16(url.len() / 2)?.split('\0').next().unwrap_or("").to_string();
            } else if clsid == FILE_MONIKER {
                let up = c.u16()?;
                let n = c.u32()? as usize;
                let ansi = c.bytes(n.min(c.left()))?;
                c.skip(2 + 2 + 20)?;
                let size = c.u32()?;
                let path = if size > 0 {
                    let n = c.u32()? as usize;
                    c.skip(2)?;
                    c.utf16(n / 2)?
                } else {
                    ansi.split(|&b| b == 0).next().unwrap_or(&[]).iter().map(|&b| crate::ole::cp1252(b)).collect()
                };
                target = format!("{}{path}", "../".repeat(up.min(64) as usize));
            } else {
                return None;
            }
        }
    }
    if flags & 0x08 != 0 {
        let place = hl_string(&mut c)?;
        if target.is_empty() {
            target = format!("#{place}");
        } else {
            target = format!("{target}#{place}");
        }
    }
    if target.is_empty() {
        return None;
    }
    let area = Range::new(CellRef::new(u32::from(r1), u32::from(c1)), CellRef::new(u32::from(r2), u32::from(c2)));
    Some((area, Link { target, tip: String::new() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rk_numbers() {
        assert_eq!(rk(0x3FF0_0000), 1.0);
        assert_eq!(rk((5 << 2) | 2), 5.0);
        assert_eq!(rk((1234 << 2) | 3), 12.34);
        assert_eq!(rk(((-7i32 << 2) | 2) as u32), -7.0);
    }

    #[test]
    fn strings_across_continue() {
        // "abcdef": 3 Latin-1 characters, then the rest as UTF-16 after a
        // break with its own flags byte.
        let mut d = vec![6, 0, 0, b'a', b'b', b'c'];
        let brk = d.len();
        d.extend_from_slice(&[1, b'd', 0, b'e', 0, 0xF1, 0]);
        let mut s = Seg { c: Cur::new(&d), breaks: &[brk] };
        assert_eq!(s.rich_string().as_deref(), Some("abcdeñ"));
    }
}
