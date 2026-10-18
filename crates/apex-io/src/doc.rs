//! Word 97-2003 documents (.doc), read only: text, character and paragraph
//! formatting, styles (headings), lists, tables, hyperlinks and fields,
//! footnotes and endnotes, comments and bookmarks, tab stops, borders and
//! shading, headers and footers, inline and floating pictures, shapes and
//! text boxes, the page setup and the document properties. Word 6.0/95
//! files go through `ww6`, which hands the same reader their text and
//! formatting.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use apex_doc::{Align, Attrs, Border, Borders, CharFx, Document, Field, HfSlot, Image, LINE_BREAK, Line, LineRule, LineStyle, Note, NoteKind, OBJECT, ParaBorders, ParaKind, ParaStyle, Paragraph, TabAlign, TabLeader, TabStop, TabStops, Table};

use crate::Error;

mod art;
mod forms;
mod notes;
mod ww2;
mod ww6;

/// Twips per point.
const TW: f32 = 20.0;

fn u16_at(d: &[u8], p: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(p..p + 2)?.try_into().ok()?))
}

fn u32_at(d: &[u8], p: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(p..p + 4)?.try_into().ok()?))
}

fn i16_at(d: &[u8], p: usize) -> Option<i16> {
    u16_at(d, p).map(|v| v as i16)
}

// --- the file information block ---------------------------------------------

struct Fib {
    /// Characters in the main text, footnotes, headers, comments, endnotes.
    ccp: [u32; 5],
    fclcb: Vec<(u32, u32)>,
    /// A Word 6.0/95 file (see `ww6`).
    word6: bool,
}

const PLCF_FND_REF: usize = 2;
const PLCF_FND_TXT: usize = 3;
const PLCF_SED: usize = 6;
const PLCF_HDD: usize = 11;
const PLCF_BTE_CHPX: usize = 12;
const PLCF_BTE_PAPX: usize = 13;
const STTBF_FFN: usize = 15;
const STSHF: usize = 1;
const DOP: usize = 31;
const CLX: usize = 33;
const PLCF_END_REF: usize = 46;
const PLCF_END_TXT: usize = 47;
const PLF_LST: usize = 73;
const PLF_LFO: usize = 74;

/// The FIB of a Word 97-2003 file, or of a Word 6.0/95 one with what
/// only it has.
fn read_fib(wd: &[u8]) -> Result<(Fib, Option<ww6::Info>), Error> {
    let bad = || Error::new("not a Word document");
    // Word 6.0 and 95 (also their Far East versions) and 97 and later.
    if !matches!(u16_at(wd, 0), Some(0xA5EC | 0xA5DC | 0xA699 | 0xA697)) {
        return Err(bad());
    }
    let nfib = u16_at(wd, 2).ok_or_else(bad)?;
    if nfib < 0x0065 {
        return Err(Error::new("documents older than Word 6.0 are not supported"));
    }
    if nfib < 0x00C1 {
        return ww6::read_fib(wd).map(|(f, i)| (f, Some(i)));
    }
    let flags = u16_at(wd, 0x0A).ok_or_else(bad)?;
    if flags & 0x0100 != 0 {
        return Err(Error::new("encrypted .doc files are not supported"));
    }
    let csw = u16_at(wd, 32).ok_or_else(bad)? as usize;
    let p = 34 + csw * 2;
    let cslw = u16_at(wd, p).ok_or_else(bad)? as usize;
    let lw = p + 2;
    let lw_at = |i: usize| if i < cslw { u32_at(wd, lw + i * 4).unwrap_or(0) } else { 0 };
    let ccp = [lw_at(3), lw_at(4), lw_at(5), lw_at(7), lw_at(8)];
    let p = lw + cslw * 4;
    let n = u16_at(wd, p).ok_or_else(bad)? as usize;
    let fclcb = (0..n).map(|i| (u32_at(wd, p + 2 + i * 8).unwrap_or(0), u32_at(wd, p + 6 + i * 8).unwrap_or(0))).collect();
    Ok((Fib { ccp, fclcb, word6: false }, None))
}

impl Fib {
    /// A structure in the table stream.
    fn part<'a>(&self, table: &'a [u8], i: usize) -> &'a [u8] {
        match self.fclcb.get(i) {
            Some(&(fc, lcb)) if lcb > 0 => table.get(fc as usize..(fc as usize).saturating_add(lcb as usize)).unwrap_or(&[]),
            _ => &[],
        }
    }
}

/// A PLC: `n + 1` character positions and `n` items of `size` bytes.
fn plc(d: &[u8], size: usize) -> (Vec<u32>, Vec<&[u8]>) {
    if d.len() < 4 {
        return (Vec::new(), Vec::new());
    }
    let n = (d.len() - 4) / (4 + size);
    let cps = (0..=n).filter_map(|i| u32_at(d, i * 4)).collect();
    let items = (0..n).filter_map(|i| d.get(4 * (n + 1) + i * size..4 * (n + 1) + (i + 1) * size)).collect();
    (cps, items)
}

// --- properties ---------------------------------------------------------------

/// The operations (`sprm`) of a property list with their operands.
fn sprms(g: &[u8]) -> Vec<(u16, &[u8])> {
    let mut out = Vec::new();
    let mut p = 0;
    while p + 2 <= g.len() {
        let op = u16::from_le_bytes([g[p], g[p + 1]]);
        p += 2;
        let (start, len) = match op >> 13 {
            0 | 1 => (p, 1),
            2 | 4 | 5 => (p, 2),
            3 => (p, 4),
            7 => (p, 3),
            _ => match op {
                // Table definitions: a 16-bit size, one more than the rest.
                0xD608 | 0xD606 => {
                    let cb = u16_at(g, p).unwrap_or(0) as usize;
                    (p + 2, cb.saturating_sub(1))
                }
                0xC615 if g.get(p) == Some(&255) => {
                    // Tab changes with deletion distances: computed size.
                    let del = *g.get(p + 1).unwrap_or(&0) as usize;
                    let add = *g.get(p + 2 + del * 4).unwrap_or(&0) as usize;
                    (p + 1, 1 + del * 4 + 1 + add * 3)
                }
                _ => {
                    let cb = *g.get(p).unwrap_or(&0) as usize;
                    (p + 1, cb)
                }
            },
        };
        let Some(operand) = g.get(start..start + len) else { break };
        out.push((op, operand));
        p = start + len;
    }
    out
}

#[derive(Clone, Default)]
struct Chp {
    bold: bool,
    italic: bool,
    strike: bool,
    dstrike: bool,
    underline: bool,
    caps: bool,
    small_caps: bool,
    hidden: bool,
    deleted: bool,
    size: u16,
    font: Option<u16>,
    color: Option<u32>,
    highlight: Option<u32>,
    /// 1 superscript, 2 subscript.
    iss: u8,
    special: bool,
    pic: Option<u32>,
    /// Embedded objects keep their picture in the Data stream too.
    ole: bool,
}

const ICO: [u32; 17] = [0, 0x000000, 0x0000FF, 0x00FFFF, 0x00FF00, 0xFF00FF, 0xFF0000, 0xFFFF00, 0xFFFFFF, 0x000080, 0x008080, 0x008000, 0x800080, 0x800000, 0x808000, 0x808080, 0xC0C0C0];

fn ico(i: u8) -> Option<u32> {
    (i > 0).then(|| ICO.get(i as usize).copied()).flatten()
}

fn toggle(v: u8, base: bool) -> bool {
    match v {
        0 => false,
        1 => true,
        0x80 => base,
        0x81 => !base,
        _ => base,
    }
}

impl Chp {
    fn apply(&mut self, g: &[u8], base: &Chp) {
        for (op, o) in sprms(g) {
            let b = o.first().copied().unwrap_or(0);
            match op {
                0x0835 => self.bold = toggle(b, base.bold),
                0x0836 => self.italic = toggle(b, base.italic),
                0x0837 => self.strike = toggle(b, base.strike),
                0x2A53 => self.dstrike = toggle(b, base.dstrike),
                0x083B => self.caps = toggle(b, base.caps),
                0x083A => self.small_caps = toggle(b, base.small_caps),
                0x083C => self.hidden = toggle(b, base.hidden),
                0x0800 => self.deleted = b != 0,
                0x2A3E => self.underline = b != 0,
                0x4A43 => self.size = u16_at(o, 0).unwrap_or(24),
                0x4A4F | 0x4A3D => self.font = u16_at(o, 0),
                0x2A42 => self.color = ico(b),
                0x6870 => self.color = colorref(o),
                0x2A0C => self.highlight = ico(b),
                0x2A48 => self.iss = b,
                0x0855 => self.special = b != 0,
                0x6A03 => self.pic = u32_at(o, 0),
                0x080A => self.ole = b != 0,
                _ => {}
            }
        }
    }
}

/// A border: line type, width in eighths of a point, color and distance
/// from the text in points.
#[derive(Clone, Copy, Default)]
struct Brc {
    ty: u8,
    width: u8,
    color: u32,
    space: u8,
}

fn colorref(o: &[u8]) -> Option<u32> {
    (o.len() >= 4 && o[3] != 0xFF).then(|| u32::from(o[0]) << 16 | u32::from(o[1]) << 8 | u32::from(o[2]))
}

impl Brc {
    /// A `Brc80` (4 bytes; all ones, type 255, is none).
    fn old(o: &[u8]) -> Brc {
        match o.get(..4) {
            Some(b) => Brc { width: b[0], ty: b[1], color: ico(b[2]).unwrap_or(0), space: b[3] & 0x1F },
            None => Brc::default(),
        }
    }

    /// A `Brc` (8 bytes, with an RGB color).
    fn new(o: &[u8]) -> Brc {
        match o.get(..8) {
            Some(b) => Brc { color: colorref(b).unwrap_or(0), width: b[4], ty: b[5], space: b[6] & 0x1F },
            None => Brc::default(),
        }
    }

    fn is_none(&self) -> bool {
        matches!(self.ty, 0 | 0xFF)
    }

    fn width(&self) -> f32 {
        (f32::from(self.width) / 8.0).clamp(0.25, 12.0)
    }

    fn line(&self) -> Option<Line> {
        if self.is_none() {
            return None;
        }
        let style = match self.ty {
            2 => LineStyle::Thick,
            3 | 10..=19 => LineStyle::Double,
            6 => LineStyle::Dotted,
            7 | 22 => LineStyle::Dashed,
            8 | 9 | 23 => LineStyle::DashDot,
            20 | 21 => LineStyle::Wave,
            _ => LineStyle::Single,
        };
        Some(Line { style, width: self.width(), color: self.color, space: f32::from(self.space).min(31.0) })
    }

    /// A table border (none is an explicit no line).
    fn border(&self) -> Border {
        if self.is_none() { Border::NONE } else { Border::new(self.width(), self.color) }
    }
}

/// A background color from a `Shd80` (`None` for none, automatic or white).
fn shd80(v: u16) -> Option<u32> {
    let (fore, back, pat) = ((v & 0x1F) as u8, ((v >> 5) & 0x1F) as u8, v >> 10);
    let c = if pat == 1 { ico(fore) } else { ico(back) };
    c.filter(|&c| c != 0xFFFFFF)
}

/// A background color from a `Shd` (10 bytes).
fn shd(o: &[u8]) -> Option<u32> {
    let pat = u16_at(o, 8)?;
    if pat == 0xFFFF {
        return None;
    }
    let c = if pat == 1 { colorref(o.get(0..4)?) } else { colorref(o.get(4..8)?) };
    c.filter(|&c| c != 0xFFFFFF)
}

/// Tab stop changes: deleted positions, then added stops (`close` holds
/// the extra deletion distances of `sprmPChgTabs`).
fn change_tabs(tabs: &mut TabStops, o: &[u8], close: bool) {
    let del = *o.first().unwrap_or(&0) as usize;
    for i in 0..del {
        if let Some(x) = i16_at(o, 1 + i * 2) {
            tabs.remove(f32::from(x) / TW);
        }
    }
    let p = 1 + del * if close { 4 } else { 2 };
    let add = *o.get(p).unwrap_or(&0) as usize;
    for i in 0..add {
        let (Some(x), Some(&tbd)) = (i16_at(o, p + 1 + i * 2), o.get(p + 1 + add * 2 + i)) else { break };
        let align = match tbd & 7 {
            1 => TabAlign::Center,
            2 => TabAlign::Right,
            3 => TabAlign::Decimal,
            // Bar tabs draw a line; they aren't stops.
            4 => continue,
            _ => TabAlign::Left,
        };
        let leader = match (tbd >> 3) & 7 {
            1 | 5 => TabLeader::Dots,
            2 => TabLeader::Dashes,
            3 | 4 => TabLeader::Underline,
            _ => TabLeader::None,
        };
        if x >= 0 {
            tabs.set(TabStop { pos: f32::from(x) / TW, align, leader });
        }
    }
}

/// What a table row's end mark says about the row.
/// The most of a `sprmTDefTable` kept: 64 cells' edges and descriptions.
const MAX_TDEF: usize = 1 + 65 * 2 + 64 * 20;

#[derive(Clone, Default)]
struct RowDef {
    /// `sprmTDefTable`: the cell edges and the old cell descriptions.
    tdef: Vec<u8>,
    /// The table's borders: top, left, bottom, right, inside lines.
    borders: Option<[Brc; 6]>,
    /// Cell backgrounds (`Some(None)`: none).
    shd: Vec<Option<Option<u32>>>,
    /// Borders set on cells `a..b`: (a, b, sides top-left-bottom-right as
    /// bits 1-2-4-8, border).
    set_brc: Vec<(u8, u8, u8, Brc)>,
}

#[derive(Clone, Default)]
struct Pap {
    istd: u16,
    jc: u8,
    outline: Option<u8>,
    ilfo: u16,
    ilvl: u8,
    /// Old style numbering (Word 6/95): the number format and the level
    /// (1-9 outline levels, 10 a numbered and 11 a bulleted paragraph).
    anld: Option<u8>,
    nlvlanm: u8,
    in_table: bool,
    ttp: bool,
    itap: u32,
    inner_ttp: bool,
    left: i16,
    right: i16,
    first: i16,
    before: Option<u16>,
    after: Option<u16>,
    line: Option<(i16, u16)>,
    page_break: bool,
    keep: bool,
    keep_next: bool,
    tabs: TabStops,
    borders: ParaBorders,
    shading: Option<u32>,
    row: RowDef,
}

impl Pap {
    fn apply(&mut self, g: &[u8]) {
        for (op, o) in sprms(g) {
            let b = o.first().copied().unwrap_or(0);
            match op {
                0x2403 | 0x2461 => self.jc = b,
                0x2640 => self.outline = (b < 9).then_some(b),
                0x260A => self.ilvl = b.min(8),
                0x460B => self.ilfo = u16_at(o, 0).unwrap_or(0),
                0xC63E => self.anld = o.first().copied(),
                0x25FF => self.nlvlanm = b,
                0x2416 => self.in_table = b != 0,
                0x2417 => self.ttp = b != 0,
                0x6649 => self.itap = u32_at(o, 0).unwrap_or(0),
                0x244C => self.inner_ttp = b != 0,
                0x840F | 0x845E => self.left = i16_at(o, 0).unwrap_or(0),
                0x840E | 0x845D => self.right = i16_at(o, 0).unwrap_or(0),
                0x8411 | 0x8460 => self.first = i16_at(o, 0).unwrap_or(0),
                0xA413 => self.before = u16_at(o, 0),
                0xA414 => self.after = u16_at(o, 0),
                0x6412 => self.line = Some((i16_at(o, 0).unwrap_or(240), u16_at(o, 2).unwrap_or(1))),
                0x2407 => self.page_break = b != 0,
                0x2405 => self.keep = b != 0,
                0x2406 => self.keep_next = b != 0,
                0xC60D => change_tabs(&mut self.tabs, o, false),
                0xC615 => change_tabs(&mut self.tabs, o, true),
                0x6424..=0x6428 => self.set_border(op - 0x6424, Brc::old(o)),
                0xC64E..=0xC652 => self.set_border(op - 0xC64E, Brc::new(o)),
                0x442D => self.shading = shd80(u16_at(o, 0).unwrap_or(0)),
                0xC64D => self.shading = shd(o),
                // (A row's definitions are copied with each paragraph: no
                // more than 64 cells' worth is kept.)
                0xD608 => self.row.tdef = o[..o.len().min(MAX_TDEF)].to_vec(),
                0xD605 => self.row.borders = Some(std::array::from_fn(|i| Brc::old(o.get(i * 4..).unwrap_or(&[])))),
                0xD613 => self.row.borders = Some(std::array::from_fn(|i| Brc::new(o.get(i * 8..).unwrap_or(&[])))),
                0xD609 => {
                    self.row.shd = o.chunks_exact(2).take(64).map(|c| Some(shd80(u16::from_le_bytes([c[0], c[1]])))).collect();
                }
                // Cells from 0, 22 and 44.
                0xD612 | 0xD616 | 0xD60C => {
                    let from = match op {
                        0xD612 => 0,
                        0xD616 => 22,
                        _ => 44,
                    };
                    for (i, c) in o.chunks_exact(10).enumerate().take(64usize.saturating_sub(from)) {
                        if self.row.shd.len() <= from + i {
                            self.row.shd.resize(from + i + 1, None);
                        }
                        self.row.shd[from + i] = Some(shd(c));
                    }
                }
                0xD620 | 0xD62F if o.len() >= 3 => {
                    let brc = if op == 0xD620 { Brc::old(&o[3..]) } else { Brc::new(&o[3..]) };
                    if self.row.set_brc.len() < 256 {
                        self.row.set_brc.push((o[0], o[1], o[2], brc));
                    }
                }
                _ => {}
            }
        }
    }

    /// Paragraph border `i`: top, left, bottom, right, between.
    fn set_border(&mut self, i: u16, brc: Brc) {
        let b = &mut self.borders;
        let side = match i {
            0 => &mut b.top,
            1 => &mut b.left,
            2 => &mut b.bottom,
            3 => &mut b.right,
            _ => &mut b.between,
        };
        *side = brc.line();
    }
}

// --- formatting runs (FKPs) ------------------------------------------------------

/// The pages of a bin table.
fn bte_pages(plc_data: &[u8]) -> Vec<u32> {
    plc(plc_data, 4).1.iter().map(|it| u32::from_le_bytes([it[0], it[1], it[2], it[3]]) & 0x3F_FFFF).collect()
}

/// Runs of file positions with their property list (from the formatting
/// pages `pns`); paragraph runs start with the style. Word 6 pages have
/// shorter paragraph entries and sizes in words.
fn read_fkps(pns: &[u32], wd: &[u8], para: bool, word6: bool) -> Vec<(u32, u32, Vec<u8>)> {
    let mut out = Vec::new();
    // A page named over and over is read once: each would copy its runs.
    let mut seen = std::collections::HashSet::new();
    for &pn in pns {
        if !seen.insert(pn) {
            continue;
        }
        let pn = pn as usize;
        let Some(page) = wd.get(pn * 512..pn * 512 + 512) else { continue };
        let n = page[511] as usize;
        for i in 0..n {
            let (Some(a), Some(b)) = (u32_at(page, i * 4), u32_at(page, (i + 1) * 4)) else { break };
            let base = 4 * (n + 1);
            let g = if para {
                let off = page.get(base + if word6 { 7 } else { 13 } * i).copied().unwrap_or(0) as usize * 2;
                let cb = page.get(off).copied().unwrap_or(0) as usize;
                let (start, len) = match cb {
                    _ if word6 => (off + 1, cb * 2),
                    0 => (off + 2, page.get(off + 1).copied().unwrap_or(0) as usize * 2),
                    _ => (off + 1, cb * 2 - 1),
                };
                page.get(start..start + len).unwrap_or(&[]).to_vec()
            } else {
                let off = page.get(base + i).copied().unwrap_or(0) as usize * 2;
                if off == 0 {
                    Vec::new()
                } else {
                    let cb = page.get(off).copied().unwrap_or(0) as usize;
                    page.get(off + 1..off + 1 + cb).unwrap_or(&[]).to_vec()
                }
            };
            out.push((a, b, g));
        }
    }
    out.sort_by_key(|r| r.0);
    out
}

fn find_run(runs: &[(u32, u32, Vec<u8>)], fc: u32) -> Option<usize> {
    let i = runs.partition_point(|r| r.1 <= fc);
    runs.get(i).filter(|r| r.0 <= fc).map(|_| i)
}

// --- styles, fonts and lists -------------------------------------------------------

struct Style {
    sti: u16,
    base: u16,
    papx: Vec<u8>,
    chpx: Vec<u8>,
}

/// The style sheet (Word 6 names are 8-bit).
fn read_styles(d: &[u8], word6: bool) -> Vec<Option<Style>> {
    let mut out = Vec::new();
    let Some(cb_stshi) = u16_at(d, 0) else { return out };
    let stshi = 2;
    let cstd = u16_at(d, stshi).unwrap_or(0) as usize;
    let cb_base = u16_at(d, stshi + 2).unwrap_or(10) as usize;
    let mut p = 2 + cb_stshi as usize;
    for _ in 0..cstd {
        let Some(cb) = u16_at(d, p) else { break };
        let std = d.get(p + 2..p + 2 + cb as usize).unwrap_or(&[]);
        p += 2 + cb as usize;
        if std.len() < cb_base {
            out.push(None);
            continue;
        }
        let sti = u16_at(std, 0).unwrap_or(0) & 0x0FFF;
        let w1 = u16_at(std, 2).unwrap_or(0);
        let cupx = u16_at(std, 4).unwrap_or(0) & 0xF;
        let para = w1 & 0xF == 1;
        let base = w1 >> 4;
        let mut q = if word6 {
            cb_base + 1 + std.get(cb_base).copied().unwrap_or(0) as usize + 1
        } else {
            cb_base + 2 + u16_at(std, cb_base).unwrap_or(0) as usize * 2 + 2
        };
        let mut upx = Vec::new();
        for _ in 0..cupx {
            if q % 2 == 1 {
                q += 1;
            }
            let n = u16_at(std, q).unwrap_or(0) as usize;
            upx.push(std.get(q + 2..q + 2 + n).unwrap_or(&[]).to_vec());
            q += 2 + n;
        }
        let (papx, chpx) = if para {
            (upx.first().map(|u| u.get(2..).unwrap_or(&[]).to_vec()).unwrap_or_default(), upx.get(1).cloned().unwrap_or_default())
        } else {
            (Vec::new(), upx.first().cloned().unwrap_or_default())
        };
        out.push(Some(Style { sti, base, papx, chpx }));
    }
    out
}

fn read_fonts(d: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let n = u16_at(d, 0).unwrap_or(0) as usize;
    let mut p = 4;
    for _ in 0..n {
        let Some(&cb) = d.get(p) else { break };
        let ffn = d.get(p + 1..p + 1 + cb as usize).unwrap_or(&[]);
        let name: Vec<u16> = ffn.get(39..).unwrap_or(&[]).chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
        out.push(String::from_utf16_lossy(&name));
        p += 1 + cb as usize;
    }
    out
}

/// List formats: `ilfo` (from 1) -> the number format of each level (0x17
/// is a bullet, 0xFF none).
fn read_lists(lst: &[u8], lfo: &[u8]) -> Vec<Vec<u8>> {
    let mut by_id: HashMap<i32, Vec<u8>> = HashMap::new();
    let n = u16_at(lst, 0).unwrap_or(0) as usize;
    let mut lists = Vec::new();
    for i in 0..n {
        let p = 2 + i * 28;
        let (Some(id), Some(flags)) = (u32_at(lst, p), lst.get(p + 26)) else { break };
        lists.push((id as i32, if flags & 1 != 0 { 1 } else { 9 }));
    }
    let mut p = 2 + n * 28;
    for (id, levels) in lists {
        let mut nfcs = Vec::new();
        for _ in 0..levels {
            let Some(lvlf) = lst.get(p..p + 28) else { break };
            nfcs.push(lvlf[4]);
            let (chpx, papx) = (lvlf[24] as usize, lvlf[25] as usize);
            p += 28 + papx + chpx;
            let cch = u16_at(lst, p).unwrap_or(0) as usize;
            p += 2 + cch * 2;
        }
        by_id.insert(id, nfcs);
    }
    let m = u32_at(lfo, 0).unwrap_or(0) as usize;
    (0..m.min(4096)).map(|i| u32_at(lfo, 4 + i * 16).and_then(|id| by_id.get(&(id as i32)).cloned()).unwrap_or_default()).collect()
}

// --- pictures -------------------------------------------------------------------

thread_local! {
    /// Bytes the pictures of the file being read may still copy: pictures
    /// pointing at one large region over and over would copy it each time.
    static PICTURE_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(usize::MAX) };
}

/// Takes `n` bytes from the pictures' budget: false once spent.
fn picture_bytes(n: usize) -> bool {
    PICTURE_BYTES.with(|c| {
        let left = c.get();
        c.set(left.saturating_sub(n));
        left >= n
    })
}

/// A BMP file from a device-independent bitmap.
fn bmp_from_dib(dib: &[u8]) -> Option<Vec<u8>> {
    if !picture_bytes(dib.len()) {
        return None;
    }
    let header = u32_at(dib, 0)? as usize;
    let bits = u16_at(dib, 14)? as usize;
    let used = u32_at(dib, 32).unwrap_or(0) as usize;
    let colors = if used > 0 { used } else if bits <= 8 { 1 << bits } else { 0 };
    let offset = 14 + header + colors * 4;
    let mut out = b"BM".to_vec();
    out.extend_from_slice(&((14 + dib.len()) as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(offset as u32).to_le_bytes());
    out.extend_from_slice(dib);
    Some(out)
}

/// The picture of an Office Art record tree (PNG, JPEG or bitmap);
/// containers nested deeper than 32 are not searched.
fn find_blip(d: &[u8], depth: u32) -> Option<Vec<u8>> {
    if depth > 32 {
        return None;
    }
    let mut p = 0;
    while p + 8 <= d.len() {
        let vi = u16_at(d, p)?;
        let ty = u16_at(d, p + 2)?;
        let len = u32_at(d, p + 4)? as usize;
        let body = d.get(p + 8..(p + 8 + len).min(d.len()))?;
        let inst = vi >> 4;
        let found = match ty {
            _ if vi & 0xF == 0xF => find_blip(body, depth + 1),
            0xF007 => body.get(33).and_then(|&n| find_blip(body.get(36 + n as usize..)?, depth + 1)),
            0xF01D | 0xF02A => body.get(if matches!(inst, 0x46B | 0x6E3) { 33 } else { 17 }..).filter(|b| picture_bytes(b.len())).map(<[u8]>::to_vec),
            0xF01E => body.get(if inst == 0x6E1 { 33 } else { 17 }..).filter(|b| picture_bytes(b.len())).map(<[u8]>::to_vec),
            0xF01F => body.get(if inst == 0x7A9 { 33 } else { 17 }..).and_then(bmp_from_dib),
            _ => None,
        };
        if found.is_some() {
            return found;
        }
        p += 8 + len;
    }
    None
}

fn read_picture(data: &[u8], at: u32) -> Option<Image> {
    let d = data.get(at as usize..)?;
    let lcb = u32_at(d, 0)? as usize;
    let header = u16_at(d, 4)? as usize;
    let d = d.get(..lcb.min(d.len()))?;
    let mm = u16_at(d, 6)?;
    let (gx, gy) = (i16_at(d, 0x1C)? as f32, i16_at(d, 0x1E)? as f32);
    let (mx, my) = (u16_at(d, 0x20)? as f32, u16_at(d, 0x22)? as f32);
    let mut p = header;
    if mm == 0x66 {
        p += 1 + *d.get(p)? as usize;
    }
    let bytes = find_blip(d.get(p..)?, 0)?;
    let scale = |g: f32, m: f32| g * if m > 0.0 { m / 1000.0 } else { 1.0 } / TW;
    Some(Image::new(Arc::from(bytes), scale(gx, mx).max(1.0), scale(gy, my).max(1.0)))
}

// --- the reader --------------------------------------------------------------------

struct Doc<'a> {
    /// Characters (UTF-16 units) by character position, and where each is
    /// in the WordDocument stream.
    text: Vec<u16>,
    fcs: Vec<u32>,
    chpx: Vec<(u32, u32, Vec<u8>)>,
    papx: Vec<(u32, u32, Vec<u8>)>,
    styles: Vec<Option<Style>>,
    fonts: Vec<String>,
    lists: Vec<Vec<u8>>,
    data: &'a [u8],
    /// Field start -> separator (if any) and end.
    fields: HashMap<u32, (Option<u32>, u32)>,
    /// Note reference position -> the note.
    notes: HashMap<u32, Arc<apex_doc::Note>>,
    default_font: Option<String>,
    chp_cache: HashMap<(usize, u16), Chp>,
    pap_styles: std::cell::RefCell<HashMap<u16, Pap>>,
    chp_styles: std::cell::RefCell<HashMap<u16, Chp>>,
    /// Bookmark and comment range boundaries by character position:
    /// (name, end), placed as the text is read.
    marks: BTreeMap<u32, Vec<(Arc<str>, bool)>>,
    /// Floating shapes and pictures by the position of their anchor.
    floats: HashMap<u32, art::Float>,
    /// Word 6/95: pictures are in the WordDocument stream (`data`).
    word6: bool,
    /// Inline pictures by where they're kept: characters pointing at the
    /// same one share it.
    pics: HashMap<u32, Option<Arc<Image>>>,
    /// Characters notes, comments and text boxes may still copy: each
    /// names its own range, and many may name the whole text.
    copy_left: std::cell::Cell<usize>,
}

impl<'a> Doc<'a> {
    fn new(text: Vec<u16>, fcs: Vec<u32>, styles: Vec<Option<Style>>, fonts: Vec<String>, data: &'a [u8]) -> Self {
        let text_len = text.len();
        Doc {
            text,
            fcs,
            chpx: Vec::new(),
            papx: Vec::new(),
            styles,
            fonts,
            lists: Vec::new(),
            data,
            fields: HashMap::new(),
            notes: HashMap::new(),
            default_font: None,
            chp_cache: HashMap::new(),
            pap_styles: Default::default(),
            chp_styles: Default::default(),
            marks: BTreeMap::new(),
            floats: HashMap::new(),
            word6: false,
            pics: HashMap::new(),
            copy_left: std::cell::Cell::new(4 * text_len + (1 << 20)),
        }
    }

    /// The number format of a list paragraph (0x17 a bullet, 0xFF none).
    fn nfc(&self, pap: &Pap) -> Option<u8> {
        if pap.ilfo > 0 {
            return self.lists.get(pap.ilfo as usize - 1).and_then(|l| l.get(pap.ilvl as usize).or(l.first())).copied();
        }
        // Word 97 keeps the old numbering only for older readers.
        match pap.nlvlanm {
            10 if self.word6 => pap.anld,
            11 if self.word6 => Some(0x17),
            _ => None,
        }
    }
    /// Marks the range `a..b` with bookmark or comment `name`.
    fn add_range(&mut self, name: Arc<str>, a: u32, b: u32) {
        self.marks.entry(a).or_default().push((name.clone(), false));
        self.marks.entry(b.max(a)).or_default().push((name, true));
    }

    /// Places the marks at positions `from..=cp` at the end of `p`.
    fn place_marks(&mut self, from: usize, cp: usize, p: Option<&mut Paragraph>) {
        if self.marks.is_empty() || from > cp {
            return;
        }
        let Some(p) = p else { return };
        let keys: Vec<u32> = self.marks.range(from as u32..=cp as u32).map(|(k, _)| *k).collect();
        for k in keys {
            for (name, end) in self.marks.remove(&k).unwrap_or_default() {
                p.marks.push(apex_doc::Mark { name, offset: p.text.len(), end });
            }
        }
    }

    /// [`Self::style_pap`] from the top, each style worked out once.
    fn style_pap_once(&self, istd: u16) -> Pap {
        if let Some(p) = self.pap_styles.borrow().get(&istd) {
            return p.clone();
        }
        let p = self.style_pap(istd, 0);
        self.pap_styles.borrow_mut().insert(istd, p.clone());
        p
    }

    /// [`Self::style_chp`] from the top, each style worked out once.
    fn style_chp_once(&self, istd: u16) -> Chp {
        if let Some(c) = self.chp_styles.borrow().get(&istd) {
            return c.clone();
        }
        let c = self.style_chp(istd, 0);
        self.chp_styles.borrow_mut().insert(istd, c.clone());
        c
    }

    fn style_pap(&self, istd: u16, depth: u32) -> Pap {
        let mut p = Pap::default();
        if let Some(Some(s)) = self.styles.get(istd as usize) {
            if s.base != 0x0FFF && depth < 10 {
                p = self.style_pap(s.base, depth + 1);
            }
            p.apply(&s.papx);
        }
        p.istd = istd;
        p
    }

    fn style_chp(&self, istd: u16, depth: u32) -> Chp {
        let mut c = Chp { size: 20, ..Default::default() };
        if let Some(Some(s)) = self.styles.get(istd as usize) {
            if s.base != 0x0FFF && depth < 10 {
                c = self.style_chp(s.base, depth + 1);
            }
            let base = c.clone();
            c.apply(&s.chpx, &base);
        }
        c
    }

    fn pap_at(&self, cp: usize) -> Pap {
        let fc = self.fcs.get(cp).copied().unwrap_or(0);
        match find_run(&self.papx, fc) {
            Some(i) => {
                let g = &self.papx[i].2;
                let istd = u16_at(g, 0).unwrap_or(0);
                let mut p = self.style_pap_once(istd);
                p.apply(g.get(2..).unwrap_or(&[]));
                p
            }
            None => self.style_pap_once(0),
        }
    }

    fn chp_at(&mut self, cp: usize, istd: u16) -> Chp {
        let fc = self.fcs.get(cp).copied().unwrap_or(0);
        let run = find_run(&self.chpx, fc).unwrap_or(usize::MAX);
        if let Some(c) = self.chp_cache.get(&(run, istd)) {
            return c.clone();
        }
        let base = self.style_chp_once(istd);
        let mut c = base.clone();
        if let Some(r) = self.chpx.get(run) {
            // A character style first, then the direct formatting.
            for (op, o) in sprms(&r.2) {
                if op == 0x4A30
                    && let Some(cs) = u16_at(o, 0)
                {
                    let mut chain = Vec::new();
                    let mut i = cs;
                    while let Some(Some(st)) = self.styles.get(i as usize)
                        && chain.len() < 10
                    {
                        chain.push(i);
                        i = st.base;
                    }
                    for i in chain.into_iter().rev() {
                        if let Some(Some(st)) = self.styles.get(i as usize) {
                            let b = c.clone();
                            c.apply(&st.chpx, &b);
                        }
                    }
                }
            }
            let styled = c.clone();
            c.apply(&r.2, &styled);
        }
        self.chp_cache.insert((run, istd), c.clone());
        c
    }

    fn kind(&self, pap: &Pap) -> ParaKind {
        match self.nfc(pap) {
            Some(0x17) => return ParaKind::Bullet,
            Some(0xFF) | None => {}
            Some(_) => return ParaKind::Numbered,
        }
        match self.styles.get(pap.istd as usize) {
            Some(Some(s)) if (1..=9).contains(&s.sti) => ParaKind::Heading(s.sti as u8),
            Some(Some(s)) if s.sti == 62 => ParaKind::Title,
            _ => match pap.outline {
                Some(l) => ParaKind::Heading(l + 1),
                None => ParaKind::Normal,
            },
        }
    }

    fn para_style(&self, pap: &Pap) -> ParaStyle {
        let kind = self.kind(pap);
        let mut st = ParaStyle { kind, ..Default::default() };
        st.align = match pap.jc {
            1 => Align::Center,
            2 => Align::Right,
            3 | 4 | 5 => Align::Justify,
            _ => Align::Left,
        };
        if kind.is_list() {
            st.level = pap.ilvl;
            st.list = pap.ilfo;
            if kind == ParaKind::Numbered {
                st.list_format = match self.nfc(pap).unwrap_or(0) {
                    0 => 1,
                    4 => 3,
                    3 => 4,
                    2 => 5,
                    1 => 6,
                    _ => 0,
                };
            }
        } else {
            st.indent_left = f32::from(pap.left) / TW;
            st.first_line = f32::from(pap.first) / TW;
        }
        st.indent_right = f32::from(pap.right) / TW;
        st.space_before = pap.before.map(|v| f32::from(v) / TW);
        st.space_after = pap.after.map(|v| f32::from(v) / TW);
        if let Some((line, mult)) = pap.line {
            if mult == 1 && line > 0 {
                st.line_spacing = f32::from(line) / 240.0;
                st.line_rule = LineRule::Multiple;
            } else if line != 0 {
                st.line_spacing = f32::from(line.unsigned_abs()) / TW;
                st.line_rule = if line < 0 { LineRule::Exact } else { LineRule::AtLeast };
            }
        }
        st.page_break_before = pap.page_break;
        st.keep_lines = pap.keep;
        st.keep_with_next = pap.keep_next;
        st.tabs = pap.tabs;
        st.borders = pap.borders;
        st.shading = pap.shading;
        st
    }

    fn attrs(&self, c: &Chp, kind: ParaKind) -> Attrs {
        let mut a = Attrs { bold: c.bold && !kind.base_bold(), italic: c.italic, strike: c.strike, underline: c.underline, ..Default::default() };
        a.superscript = c.iss == 1;
        a.subscript = c.iss == 2;
        if c.size > 0 && c.size != kind.base_size() {
            a.size = Some(c.size);
        }
        a.color = c.color.filter(|&c| c != 0);
        a.highlight = c.highlight;
        let font = c.font.and_then(|f| self.fonts.get(f as usize)).filter(|f| Some(*f) != self.default_font.as_ref() && !f.is_empty());
        a.font = font.map(|f| Arc::from(f.as_str()));
        if c.caps || c.small_caps || c.hidden || c.dstrike {
            a.fx = Some(Arc::new(CharFx { all_caps: c.caps, small_caps: c.small_caps, hidden: c.hidden, double_strike: c.dstrike, ..Default::default() }));
        }
        a
    }

    /// The character at `cp` (a surrogate pair takes two positions).
    fn char_at(&self, cp: usize) -> (char, usize) {
        let u = self.text[cp];
        if (0xD800..0xDC00).contains(&u)
            && let Some(&l) = self.text.get(cp + 1)
            && (0xDC00..0xE000).contains(&l)
        {
            let c = char::decode_utf16([u, l]).next().and_then(Result::ok).unwrap_or('\u{FFFD}');
            return (c, 2);
        }
        (char::from_u32(u32::from(u)).unwrap_or('\u{FFFD}'), 1)
    }

    /// Takes the characters `a..b` from `copy_left`, if there are as many.
    fn afford(&self, a: usize, b: usize) -> bool {
        let n = b.min(self.text.len()).saturating_sub(a);
        let left = self.copy_left.get();
        self.copy_left.set(left.saturating_sub(n));
        n <= left
    }

    fn plain(&self, a: usize, b: usize) -> String {
        let mut s = String::new();
        if !self.afford(a, b) {
            return s;
        }
        let mut cp = a;
        while cp < b.min(self.text.len()) {
            let (c, n) = self.char_at(cp);
            if c >= ' ' || c == '\t' {
                s.push(c);
            } else if c == '\r' || c == '\u{7}' || c == '\u{b}' {
                s.push('\n');
            }
            cp += n;
        }
        s
    }

    /// The instruction of a field, without its nested fields' results.
    fn field_code(&self, start: usize, sep: usize) -> String {
        let mut s = String::new();
        let mut depth = 0;
        for cp in start + 1..sep.min(self.text.len()) {
            match self.text[cp] {
                0x13 => depth += 1,
                0x15 => depth -= 1,
                0x14 => {}
                u if depth == 0 && u >= 0x20 => s.push(char::from_u32(u32::from(u)).unwrap_or(' ')),
                _ => {}
            }
        }
        s.trim().to_string()
    }

    /// The paragraphs of the characters `a..b` of a story.
    fn story(&mut self, a: usize, b: usize) -> Vec<Paragraph> {
        let b = b.min(self.text.len());
        let mut out: Vec<Paragraph> = Vec::new();
        if !self.afford(a, b) {
            return out;
        }
        // Table being read: rows of cells of paragraphs, and the cell.
        let mut rows: Vec<(Vec<Vec<Paragraph>>, RowDef)> = Vec::new();
        let mut row: Vec<Vec<Paragraph>> = Vec::new();
        let mut cell: Vec<Paragraph> = Vec::new();
        // Links in effect: (end, target).
        let mut links: Vec<(usize, Arc<str>)> = Vec::new();
        let mut skip_to = 0usize;
        // A form text field whose result is being read: the field, its
        // end, and its paragraph and where its text starts in it.
        let mut form: Option<(apex_doc::ContentControl, usize, (usize, usize), usize)> = None;
        let mut break_before = false;
        let mut start = a;
        // Paragraphs made so far, in tables or not.
        let mut made = 0;
        while start < b {
            // Past what a document keeps: a file of paragraph (or cell)
            // marks alone would take gigabytes.
            made += 1;
            if made > apex_doc::MAX_PARAGRAPHS {
                break;
            }
            let end = (start..b).find(|&cp| matches!(self.text[cp], 0x0D | 0x07)).unwrap_or(b);
            let pap = self.pap_at(end.min(b.saturating_sub(1)));
            let mark = self.text.get(end).copied().unwrap_or(0x0D);
            let st = self.para_style(&pap);
            let mut paras = vec![Paragraph::new("").with_style(ParaStyle { page_break_before: st.page_break_before || break_before, ..st })];
            break_before = false;
            let mut cp = start;
            while cp < end {
                self.place_marks(a, cp, paras.last_mut());
                if cp < skip_to {
                    cp += 1;
                    continue;
                }
                links.retain(|l| cp < l.0);
                let (c, n) = self.char_at(cp);
                let chp = self.chp_at(cp, pap.istd);
                if chp.deleted {
                    cp += n;
                    continue;
                }
                let mut at = self.attrs(&chp, st.kind);
                if let Some(l) = links.last() {
                    at.link = Some(l.1.clone());
                    // The Hyperlink style's look comes with the link.
                    if matches!(at.color, Some(0x0563C1 | 0x0000FF | 0x000080)) {
                        at.color = None;
                    }
                }
                // Which paragraph `p` is (in this story).
                let here = (start, paras.len());
                let p = paras.last_mut().expect("paragraph");
                match c {
                    '\u{13}' => {
                        let (sep, fend) = self.fields.get(&(cp as u32)).copied().unwrap_or((None, cp as u32));
                        let (sep, fend) = (sep.map(|s| s as usize), fend as usize);
                        let code = self.field_code(cp, sep.unwrap_or(fend));
                        let word = code.split_whitespace().next().unwrap_or("").to_ascii_uppercase();
                        let result = sep.map(|s| (s + 1, fend));
                        let simple = result.is_some_and(|(x, y)| !self.text.get(x..y.min(self.text.len())).unwrap_or(&[]).iter().any(|&u| matches!(u, 0x0D | 0x07 | 0x13)));
                        // Its settings: where the picture character of its
                        // instruction points.
                        let code_end = sep.unwrap_or(fend).min(self.text.len());
                        let pic = (cp + 1..code_end).find(|&i| self.text[i] == 1).and_then(|i| self.chp_at(i, pap.istd).pic);
                        let ff = crate::docx_forms::form_word(&code).filter(|_| !self.word6 && form.is_none()).and_then(|w| Some((w, forms::read(self.data, pic?)?)));
                        if let Some((w, ff)) = ff {
                            let (c, text) = crate::docx_forms::control(ff, w, cp as u32 + 1);
                            match text {
                                Some(text) => {
                                    at.control = Some(Arc::new(c));
                                    p.push(&text, at);
                                    skip_to = fend + 1;
                                }
                                None => {
                                    // The result is read as usual, then made the field's.
                                    form = Some((c, fend, here, p.text.len()));
                                    skip_to = sep.map_or(fend, |s| s + 1);
                                }
                            }
                        } else if word == "HYPERLINK" {
                            let words = apex_doc::words(&code);
                            let mut target = String::new();
                            let mut i = 1;
                            while i < words.len() {
                                match words[i].as_str() {
                                    "\\l" => {
                                        target.push('#');
                                        target.push_str(words.get(i + 1).map_or("", |s| s.as_str()));
                                        i += 1;
                                    }
                                    s if s.starts_with('\\') => i += 1,
                                    s if target.is_empty() => target = s.to_string(),
                                    _ => {}
                                }
                                i += 1;
                            }
                            if !target.is_empty() {
                                if links.len() < 64 {
                                    links.push((fend, Arc::from(target.as_str())));
                                }
                            }
                            skip_to = sep.map_or(fend + 1, |s| s + 1);
                        } else if let (true, Some(mut f)) = (simple, Field::parse(&code, "")) {
                            let (x, y) = result.unwrap_or((fend, fend));
                            f.cached = self.plain(x, y);
                            at.field = Some(Arc::new(f));
                            p.push(&OBJECT.to_string(), at);
                            skip_to = fend + 1;
                        } else {
                            skip_to = sep.map_or(fend + 1, |s| s + 1);
                        }
                    }
                    '\u{15}' if form.as_ref().is_some_and(|f| f.1 == cp) => {
                        let (c, _, para, start) = form.take().expect("form");
                        if para == here && start <= p.text.len() {
                            let blank = crate::docx_forms::is_blank(&p.text[start..]);
                            let c = Arc::new(apex_doc::ContentControl { placeholder: blank, ..c });
                            if start == p.text.len() {
                                p.push(apex_doc::FORM_BLANK, Attrs { control: Some(c), ..at });
                            } else {
                                p.map_attrs(start, p.text.len(), &|a| a.control = Some(c.clone()));
                            }
                        }
                    }
                    '\u{14}' | '\u{15}' => {}
                    '\u{1}' if chp.special => {
                        let read = if self.word6 { ww6::picture } else { read_picture };
                        let data = self.data;
                        if let Some(img) = chp.pic.and_then(|pos| self.pics.entry(pos).or_insert_with(|| read(data, pos).map(Arc::new)).clone()) {
                            at.image = Some(img);
                            p.push(&OBJECT.to_string(), at);
                        }
                    }
                    '\u{8}' => {
                        if let Some(f) = self.floats.get(&(cp as u32)) {
                            match f {
                                art::Float::Shape(s) => at.shape = Some(s.clone()),
                                art::Float::Picture(i) => at.image = Some(i.clone()),
                            }
                            p.push(&OBJECT.to_string(), at);
                        }
                    }
                    '\u{2}' if chp.special => {
                        if let Some(note) = self.notes.get(&(cp as u32)) {
                            at.note = Some(note.clone());
                            p.push(&OBJECT.to_string(), at);
                        }
                    }
                    '\u{c}' | '\u{e}' => {
                        // A page, section or column break: what follows
                        // starts a new paragraph.
                        let mut next = Paragraph::new("").with_style(p.style);
                        next.style.page_break_before = c == '\u{c}';
                        next.style.column_break_before = c == '\u{e}';
                        if p.text.is_empty() {
                            *p = next;
                        } else {
                            paras.push(next);
                        }
                    }
                    '\u{b}' => p.push(&LINE_BREAK.to_string(), at),
                    '\u{1e}' => p.push("\u{2011}", at),
                    '\u{1f}' => p.push("\u{AD}", at),
                    '\t' => p.push("\t", at),
                    c if c < ' ' => {}
                    c => {
                        let mut buf = [0u8; 4];
                        p.push(c.encode_utf8(&mut buf), at);
                    }
                }
                cp += n;
            }
            // A section break at the end of the text leaves an empty page.
            if paras.len() > 1 && paras.last().is_some_and(|p| p.text.is_empty()) && end >= b {
                break_before = true;
                paras.pop();
            }
            self.place_marks(a, end, paras.last_mut());
            let in_table = pap.in_table || pap.itap > 0;
            if in_table && pap.inner_ttp {
                // The end of a row of a nested table: its cells stay as
                // paragraphs of the outer cell.
            } else if in_table && pap.ttp {
                if !cell.is_empty() {
                    row.push(std::mem::take(&mut cell));
                }
                // (A table keeps no more rows.)
                if rows.len() < apex_doc::MAX_ROWS {
                    rows.push((std::mem::take(&mut row), pap.row.clone()));
                } else {
                    row.clear();
                }
            } else if in_table {
                cell.extend(paras);
                if mark == 0x07 && pap.itap <= 1 {
                    row.push(std::mem::take(&mut cell));
                }
            } else {
                flush_table(&mut out, &mut rows);
                out.extend(paras);
            }
            start = end + 1;
        }
        if !cell.is_empty() {
            row.push(cell);
        }
        if !row.is_empty() {
            rows.push((row, RowDef::default()));
        }
        flush_table(&mut out, &mut rows);
        out
    }
}

/// Adds the table read so far to `out`.
fn flush_table(out: &mut Vec<Paragraph>, rows: &mut Vec<(Vec<Vec<Paragraph>>, RowDef)>) {
    if rows.is_empty() {
        return;
    }
    let mut rows = std::mem::take(rows);
    rows.truncate(apex_doc::MAX_ROWS);
    // Column edges and cell descriptions (`TC80`s) from the row definitions.
    let defs: Vec<(Vec<i16>, Vec<Vec<u8>>)> = rows
        .iter()
        .map(|(cells, def)| {
            let t = &def.tdef[..];
            let n = t.first().copied().unwrap_or(0) as usize;
            let edges: Vec<i16> = (0..=n).filter_map(|i| i16_at(t, 1 + i * 2)).collect();
            let tcs: Vec<Vec<u8>> = (0..n).map(|i| t.get(1 + (n + 1) * 2 + i * 20..).map_or(Vec::new(), |c| c[..c.len().min(20)].to_vec())).collect();
            // Without a table definition, an inch per cell (wrapping, which
            // keeps the edges of long rows distinct).
            if edges.len() == n + 1 && n > 0 { (edges, tcs) } else { ((0..=cells.len() as i16).map(|i| i.wrapping_mul(1440)).collect(), vec![Vec::new(); cells.len()]) }
        })
        .collect();
    let mut edges: Vec<i16> = defs.iter().flat_map(|d| d.0.iter().copied()).collect();
    edges.sort_unstable();
    edges.dedup_by(|a, b| (i32::from(*a) - i32::from(*b)).abs() < 30);
    if edges.len() < 2 {
        edges = vec![0, 1440];
    }
    let ncols = (edges.len() - 1).min(apex_doc::MAX_COLS);
    let col_of = |x: i16| edges.iter().enumerate().min_by_key(|(_, e)| (i32::from(**e) - i32::from(x)).abs()).map_or(0, |(i, _)| i);
    let mut table = Table::new(rows.len(), ncols, 0.0);
    table.props.widths = edges.windows(2).take(ncols).map(|w| ((f32::from(w[1]) - f32::from(w[0])) / TW).max(apex_doc::MIN_COL_WIDTH)).collect();
    table.props.indent = f32::from(edges[0]) / TW;
    // Borders given by the file replace the grid of a new table.
    if let Some(b) = rows.first().and_then(|r| r.1.borders) {
        let [top, left, bottom, right, inside_h, inside_v] = b.map(|b| Some(b.border()));
        table.props.borders = Borders { top, left, bottom, right, inside_h, inside_v };
        table.props.style = None;
    } else if defs.iter().any(|d| d.1.iter().any(|tc| tc.len() == 20)) {
        table.props.style = None;
    }
    for (r, ((cells, def), (dx, tcs))) in rows.into_iter().zip(defs).enumerate() {
        let mut prev: Option<usize> = None;
        for (i, paras) in cells.into_iter().enumerate() {
            let (Some(&x0), Some(&x1)) = (dx.get(i), dx.get(i + 1)) else { continue };
            let (c0, c1) = (col_of(x0), col_of(x1).max(col_of(x0) + 1).min(ncols));
            if c0 >= ncols {
                continue;
            }
            let tc = tcs.get(i).map_or(&[][..], Vec::as_slice);
            let f = u16_at(tc, 0).unwrap_or(0);
            if f & 3 >= 2 {
                if let Some(a) = prev {
                    table.rows[r][a].props.col_span += (c1 - c0) as u16;
                }
                for c in c0..c1 {
                    table.rows[r][c].props.covered = true;
                }
                continue;
            }
            if (f >> 5) & 3 == 1 && r > 0 {
                for c in c0..c1 {
                    table.rows[r][c].props.covered = true;
                }
                if let Some(ar) = (0..r).rev().find(|&ar| !table.rows[ar][c0].props.covered) {
                    table.rows[ar][c0].props.row_span = (r - ar + 1) as u32;
                }
                prev = None;
                continue;
            }
            let cell = &mut table.rows[r][c0];
            cell.props.col_span = (c1 - c0) as u16;
            cell.props.row_span = 1;
            // The cell's own borders: its description, then the ones set
            // on ranges of cells.
            let b = &mut cell.props.borders;
            let mut sides = [&mut b.top, &mut b.left, &mut b.bottom, &mut b.right];
            for (k, side) in sides.iter_mut().enumerate() {
                let brc = Brc::old(tc.get(4 + k * 4..).unwrap_or(&[]));
                if tc.len() == 20 && !brc.is_none() {
                    **side = Some(brc.border());
                }
            }
            for &(a, z, which, brc) in &def.set_brc {
                if (a as usize..z as usize).contains(&i) {
                    for (k, side) in sides.iter_mut().enumerate() {
                        if which & (1 << k) != 0 {
                            **side = Some(brc.border());
                        }
                    }
                }
            }
            if let Some(Some(c)) = def.shd.get(i) {
                cell.props.shading = *c;
            }
            if !paras.is_empty() {
                cell.paras = paras;
            }
            for c in c0 + 1..c1 {
                table.rows[r][c].props.covered = true;
            }
            prev = Some(c0);
        }
    }
    let id = out.iter().filter_map(|p| p.cell.as_ref().map(|c| c.table)).max().unwrap_or(0) + 1;
    out.extend(table.into_paras(id));
}

/// Notes by the position of their reference mark.
fn read_notes(doc: &mut Doc, refs: &[u8], txt: &[u8], story: usize, kind: NoteKind) {
    let (rcps, _) = plc(refs, 2);
    let (tcps, _) = plc(txt, 0);
    for (i, &r) in rcps.iter().take(rcps.len().saturating_sub(1)).enumerate() {
        let (Some(&a), Some(&b)) = (tcps.get(i), tcps.get(i + 1)) else { break };
        let (a, b) = (story + a as usize, story + b as usize);
        // The note starts with its own mark and a space.
        let mut text = doc.plain(a, b);
        text = text.trim_start_matches(['\u{2}', ' ', '\t']).trim_end_matches('\n').to_string();
        doc.notes.insert(r, Arc::new(Note::from_text(kind, &text)));
    }
}

/// The text and formatting of a Word 97-2003 file.
fn load97<'a>(fib: &Fib, table: &[u8], wd: &[u8], data_stream: &'a [u8]) -> Result<Doc<'a>, Error> {
    // The piece table: where each run of text is stored.
    let clx = fib.part(table, CLX);
    let mut p = 0;
    let mut pieces = Vec::new();
    while p < clx.len() {
        match clx[p] {
            1 => p += 3 + i16_at(clx, p + 1).unwrap_or(0).max(0) as usize,
            2 => {
                let lcb = u32_at(clx, p + 1).unwrap_or(0) as usize;
                let (cps, pcds) = plc(clx.get(p + 5..p + 5 + lcb).unwrap_or(&[]), 8);
                for (i, pcd) in pcds.iter().enumerate() {
                    let fc = u32::from_le_bytes([pcd[2], pcd[3], pcd[4], pcd[5]]);
                    pieces.push((cps[i], cps[i + 1], fc & 0x3FFF_FFFF, fc & 0x4000_0000 != 0));
                }
                break;
            }
            _ => break,
        }
    }
    if pieces.is_empty() {
        return Err(Error::new("the document has no text"));
    }
    let mut text = Vec::new();
    let mut fcs = Vec::new();
    // Pieces that point at the same bytes again and again stop at about
    // the text the stream can hold.
    let most = wd.len() + (1 << 16);
    for (a, b, fc, compressed) in pieces {
        for i in 0..b.saturating_sub(a) {
            if text.len() >= most {
                break;
            }
            let (pos, u) = if compressed {
                let pos = (fc / 2).saturating_add(i);
                (pos, wd.get(pos as usize).map(|&b| crate::ole::cp1252(b) as u16))
            } else {
                let pos = fc.saturating_add(i.saturating_mul(2));
                (pos, u16_at(wd, pos as usize))
            };
            let Some(u) = u else { break };
            text.push(u);
            fcs.push(pos);
        }
    }
    let styles = read_styles(fib.part(table, STSHF), false);
    let fonts = read_fonts(fib.part(table, STTBF_FFN));
    let mut doc = Doc::new(text, fcs, styles, fonts, data_stream);
    doc.chpx = read_fkps(&bte_pages(fib.part(table, PLCF_BTE_CHPX)), wd, false, false);
    doc.papx = read_fkps(&bte_pages(fib.part(table, PLCF_BTE_PAPX)), wd, true, false);
    // The levels follow the list table, outside its stated size.
    doc.lists = read_lists(fib.fclcb.get(PLF_LST).and_then(|&(fc, _)| table.get(fc as usize..)).unwrap_or(&[]), fib.part(table, PLF_LFO));
    Ok(doc)
}

/// The document in a .doc file (the whole compound file).
pub fn read(data: &[u8]) -> Result<Document, Error> {
    read_with(data, None)
}

/// The document in a .doc file that may be encrypted with `password`.
pub fn read_with(data: &[u8], password: Option<&str>) -> Result<Document, Error> {
    PICTURE_BYTES.with(|c| c.set(data.len().saturating_mul(2).saturating_add(16 << 20)));
    // Word for Windows 1.x and 2.0 files aren't compound files.
    if ww2::is_old(data) {
        return ww2::read(data);
    }
    if ww2::is_bare(data) {
        return Err(Error::new(ww2::TOO_OLD));
    }
    let mut ole = crate::ole::open(data)?;
    let mut wd = crate::ole::stream(&mut ole, "/WordDocument").ok_or_else(|| Error::new("not a Word document"))?;
    // Word 6/95 keeps everything in the WordDocument stream.
    let flags = u16_at(&wd, 0x0A).unwrap_or(0);
    let word97 = u16_at(&wd, 2).is_some_and(|n| n >= 0x00C1);
    let (mut table, mut data_stream) = match word97 {
        true => (crate::ole::stream(&mut ole, if flags & 0x0200 != 0 { "/1Table" } else { "/0Table" }), crate::ole::stream(&mut ole, "/Data").unwrap_or_default()),
        false => (None, Vec::new()),
    };
    let mut no_table = Vec::new();
    crate::bin_crypto::decrypt_word(&mut wd, table.as_mut().unwrap_or(&mut no_table), &mut data_stream, password)?;
    let (fib, word6) = read_fib(&wd)?;
    let table = match word6 {
        Some(_) => wd.clone(),
        None => table.ok_or_else(|| Error::new("the document's table stream is missing"))?,
    };
    let mut doc = match &word6 {
        Some(info) => ww6::load(&fib, info, &wd)?,
        None => load97(&fib, &table, &wd, &data_stream)?,
    };
    let normal = doc.style_chp(0, 0);
    // Without a font in Normal, the style sheet's standard one
    // (rgftcStandardChpStsh, after six words of the STSHI).
    let stsh = fib.part(&table, STSHF);
    let standard = u16_at(stsh, 0).filter(|&cb| cb >= 14).and_then(|_| u16_at(stsh, 14));
    doc.default_font = normal.font.or(standard).and_then(|f| doc.fonts.get(f as usize).cloned());
    // Fields: begin, separator and end marks.
    let mut stack: Vec<(u32, Option<u32>)> = Vec::new();
    for (cp, &u) in doc.text.iter().enumerate() {
        match u {
            0x13 => stack.push((cp as u32, None)),
            0x14 => {
                if let Some(f) = stack.last_mut() {
                    f.1 = Some(cp as u32);
                }
            }
            0x15 => {
                if let Some((s, sep)) = stack.pop() {
                    doc.fields.insert(s, (sep, cp as u32));
                }
            }
            _ => {}
        }
    }
    let [ccp_text, ccp_ftn, ccp_hdd, ccp_atn, ccp_edn] = fib.ccp.map(|v| v as usize);
    read_notes(&mut doc, fib.part(&table, PLCF_FND_REF), fib.part(&table, PLCF_FND_TXT), ccp_text, NoteKind::Foot);
    let atn_story = ccp_text + ccp_ftn + ccp_hdd;
    let end_story = atn_story + ccp_atn;
    read_notes(&mut doc, fib.part(&table, PLCF_END_REF), fib.part(&table, PLCF_END_TXT), end_story, NoteKind::End);
    notes::read_bookmarks(&mut doc, &fib, &table);
    let mut comments = Vec::new();
    if word6.is_none() {
        comments = notes::read_comments(&mut doc, &fib, &table, atn_story);
        doc.floats = art::read(&mut doc, &fib, &table, &wd, end_story + ccp_edn);
    }
    let mut paras = doc.story(0, ccp_text);
    if paras.is_empty() {
        paras.push(Paragraph::new(""));
    }
    let mut d = Document::from_paragraphs(paras);
    d.default_font = doc.default_font.as_deref().map(Arc::from);
    d.comments = comments;
    // Different even pages: the document's setting (fFacingPages).
    d.odd_even = fib.part(&table, DOP).first().is_some_and(|b| b & 1 != 0);
    if !fib.word6 {
        d.protection = protection(fib.part(&table, DOP));
    }
    // Headers and footers of the first section: after the six note
    // separators, its even and odd headers, even and odd footers, and
    // first page header and footer.
    let (hcps, _) = plc(fib.part(&table, PLCF_HDD), 0);
    let hdd = ccp_text + ccp_ftn;
    let mut title_page = false;
    // Word 6: which of the six stories the section has.
    let mut hdr_bits = 0u8;
    let (scps, seds) = plc(fib.part(&table, PLCF_SED), 12);
    if let (Some(sed), Some(_)) = (seds.first(), scps.first()) {
        let fc = u32::from_le_bytes([sed[2], sed[3], sed[4], sed[5]]) as usize;
        if fc != 0xFFFF_FFFF
            && let Some(cb) = u16_at(&wd, fc)
        {
            let g = wd.get(fc + 2..fc + 2 + cb as usize).unwrap_or(&[]);
            let g = if fib.word6 { ww6::translate(g) } else { g.to_vec() };
            let page = &mut d.page;
            let (mut w, mut h) = (12240.0, 15840.0);
            let (mut l, mut r, mut t, mut bm) = (1800.0, 1800.0, 1440.0, 1440.0);
            for (op, o) in sprms(&g) {
                let v = u16_at(o, 0).unwrap_or(0) as f32;
                let s = i16_at(o, 0).unwrap_or(0) as f32;
                match op {
                    0xB01F => w = v,
                    0xB020 => h = v,
                    0xB021 => l = v,
                    0xB022 => r = v,
                    0x9023 => t = s.abs(),
                    0x9024 => bm = s.abs(),
                    0xB017 => page.margin_header = v / TW,
                    0xB018 => page.margin_footer = v / TW,
                    0x300A => title_page = o.first().is_some_and(|&b| b != 0),
                    0x3014 => hdr_bits = o.first().copied().unwrap_or(0),
                    _ => {}
                }
            }
            page.width = w / TW;
            page.height = h / TW;
            page.margin_left = l / TW;
            page.margin_right = r / TW;
            page.margin_top = t / TW;
            page.margin_bottom = bm / TW;
        }
    }
    let slots = [HfSlot::EvenHeader, HfSlot::Header, HfSlot::EvenFooter, HfSlot::Footer, HfSlot::FirstHeader, HfSlot::FirstFooter];
    // Word 6 lists only the separators and stories there are.
    let dop_bits = fib.part(&table, DOP).get(1).copied().unwrap_or(0);
    for (k, slot) in slots.into_iter().enumerate() {
        let Some(i) = (if fib.word6 { ww6::header_index(k, hdr_bits, dop_bits) } else { Some(6 + k) }) else { continue };
        let (Some(&a), Some(&b)) = (hcps.get(i), hcps.get(i + 1)) else { continue };
        if (slot.kind() == 1 && !title_page) || (slot.kind() == 2 && !d.odd_even) {
            continue;
        }
        if b <= a + 1 {
            continue;
        }
        let mut ps = doc.story(hdd + a as usize, hdd + b as usize);
        while ps.len() > 1 && ps.last().is_some_and(|p| p.text.is_empty()) {
            ps.pop();
        }
        if ps.iter().any(|p| !p.text.is_empty()) {
            d.headers.set(slot, Some(Arc::new(ps)));
        }
    }
    d.headers.different_first = title_page;
    d.meta = crate::ole::read_meta(&mut ole);
    // fReadOnlyRecommended.
    d.meta.read_only_recommended = flags & 0x0400 != 0;
    d.macros = crate::xls::legacy_macros(data, "Macros");
    Ok(d)
}

/// Restrict Editing from the document properties (DOP): fLockAtn (made
/// read-only by Word 2003's fTreatLockAtnAsReadOnly), fProtEnabled and
/// fLockRev. The 32-bit password hash (lKeyProtDoc) can't be checked
/// here: a password is only marked as there.
fn protection(dop: &[u8]) -> Option<apex_doc::Protection> {
    use apex_doc::EditLimit;
    let b = |i: usize| dop.get(i).copied().unwrap_or(0);
    let limit = if b(6) & 0x10 != 0 {
        if b(594) & 1 != 0 { EditLimit::ReadOnly } else { EditLimit::Comments }
    } else if b(7) & 0x02 != 0 {
        EditLimit::Forms
    } else if b(7) & 0x40 != 0 {
        EditLimit::TrackedChanges
    } else {
        return None;
    };
    let key = u32_at(dop, 78).unwrap_or(0);
    let password = (key != 0).then(|| apex_doc::ProtectionHash { algorithm: String::new(), hash: String::new(), salt: String::new(), spin: 0, legacy: Vec::new() });
    Some(apex_doc::Protection { limit, enforced: true, password })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn property_lists() {
        // Bold on, 14 pt, a variable-length operand and justified.
        let g = [0x35, 0x08, 0x01, 0x43, 0x4A, 28, 0, 0x2A, 0xC6, 2, 9, 9, 0x61, 0x24, 3];
        let s = sprms(&g);
        assert_eq!(s.iter().map(|x| x.0).collect::<Vec<_>>(), vec![0x0835, 0x4A43, 0xC62A, 0x2461]);
        let mut c = Chp::default();
        c.apply(&g, &Chp::default());
        assert!(c.bold);
        assert_eq!(c.size, 28);
        let mut p = Pap::default();
        p.apply(&g);
        assert_eq!(p.jc, 3);
        // Toggles relative to the style.
        assert!(!toggle(0x81, true));
        assert!(toggle(0x80, true));
    }

    #[test]
    fn old_borders_shading_and_tab_changes() {
        let mut p = Pap::default();
        // A double blue bottom line (Word 97), a yellow background, a stop
        // at 0.5" and then one at 1" instead.
        p.apply(&[0x26, 0x64, 4, 3, 2, 1, 0x2D, 0x44, 0xE0, 0, 0x0D, 0xC6, 5, 0, 1, 0xD0, 0x02, 0]);
        p.apply(&[0x15, 0xC6, 9, 1, 0xD0, 0x02, 0, 0, 1, 0xA0, 0x05, 3]);
        let b = p.borders.bottom.unwrap();
        assert_eq!((b.style, b.width, b.color, b.space), (LineStyle::Double, 0.5, 0x0000FF, 1.0));
        assert!(p.borders.top.is_none());
        assert_eq!(p.shading, Some(0xFFFF00));
        let tabs = p.tabs.as_slice();
        assert_eq!(tabs.len(), 1);
        assert_eq!((tabs[0].pos, tabs[0].align), (72.0, TabAlign::Decimal));
        // No line, and a nil shading.
        p.apply(&[0x26, 0x64, 0xFF, 0xFF, 0xFF, 0xFF, 0x4D, 0xC6, 10, 0, 0, 0, 0xFF, 0, 0, 0, 0xFF, 0xFF, 0xFF]);
        assert!(p.borders.bottom.is_none());
        assert_eq!(p.shading, None);
    }

    #[test]
    fn dib_to_bmp() {
        // A 1x1 24-bit bitmap.
        let mut dib = 40u32.to_le_bytes().to_vec();
        dib.extend_from_slice(&1i32.to_le_bytes());
        dib.extend_from_slice(&1i32.to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&24u16.to_le_bytes());
        dib.extend_from_slice(&[0; 24]);
        dib.extend_from_slice(&[0, 0, 255, 0]);
        let bmp = bmp_from_dib(&dib).unwrap();
        assert_eq!(&bmp[..2], b"BM");
        assert_eq!(u32_at(&bmp, 10), Some(54));
    }
}
