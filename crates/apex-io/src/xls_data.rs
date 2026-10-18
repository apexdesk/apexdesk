//! Sheet data tools of .xls files: conditional formats (`CONDFMT` and `CF`,
//! with Excel 2007's `CONDFMT12`, `CF12` and `CFEX`: color scales, data
//! bars and icon sets), data validation (`DV`) and autofilters
//! (`AUTOFILTER`, placed by the sheet's `_FilterDatabase` name).

use apex_sheet::data::{CfKind, CfRule, CfStyle, Cfvo, CmpOp, ColumnFilter, ErrorStyle, FilterKind, ValKind, Validation};
use apex_sheet::functions::CritOp;
use apex_sheet::{CellRef, Range};

use crate::biff::Cur;

/// Decompiles formula tokens for a base cell (relative `PtgRefN` tokens).
pub(crate) type Fmla<'a> = &'a dyn Fn(&[u8], CellRef) -> Option<String>;
/// A palette index as RGB.
pub(crate) type Color<'a> = &'a dyn Fn(u16) -> Option<u32>;

/// Icon sets by their index in .xls and .xlsb files (`KPISets`), as the
/// nearest xlsx name we draw.
pub(crate) const ICON_SETS: [&str; 20] = [
    "3Arrows", "3ArrowsGray", "3Flags", "3TrafficLights1", "3TrafficLights2", "3Signs", "3Symbols", "3Symbols2", "4Arrows", "4ArrowsGray", "4RedToBlack", "4Rating",
    "4TrafficLights", "5Arrows", "5Arrows", "5Rating", "5Rating", "3Stars", "3Triangles", "5Rating",
];

/// "Cell value is" operators (`CFOper`, 1-based).
pub(crate) fn cmp_op(i: u32) -> CmpOp {
    CmpOp::ALL.get((i as usize).wrapping_sub(1)).copied().unwrap_or_default()
}

/// Rules of the autofilter's columns: (column in the range, kind).
pub(crate) type Columns = Vec<(u32, FilterKind)>;

#[derive(Default)]
pub(crate) struct Tools {
    /// Ranges and id of the last `CONDFMT`, and how many of its rules
    /// were read.
    ranges: Vec<Range>,
    id: u16,
    icf: u16,
    /// Ranges the rules have taken so far.
    cf_used: std::cell::Cell<usize>,
    /// Priority (known for Excel 2007 rules), `CONDFMT` id and index, rule.
    rules: Vec<(Option<u16>, u16, u16, CfRule)>,
    /// `CFEX`: `CONDFMT` id and index -> priority, Stop If True and the
    /// format with Excel 2007's exact colors.
    ex: Vec<(u16, u16, u16, bool, Option<CfStyle>)>,
    pub validations: Vec<Validation>,
    /// Set by `AUTOFILTERINFO`.
    pub filter: Option<Columns>,
}

/// `Ref8U`: first and last row, first and last column.
fn ref8(c: &mut Cur) -> Option<Range> {
    let (r1, r2, c1, c2) = (c.u16()?, c.u16()?, c.u16()?, c.u16()?);
    Some(Range::new(CellRef::new(u32::from(r1), u32::from(c1)), CellRef::new(u32::from(r2), u32::from(c2))))
}

/// `SqRefU`: a count and the ranges.
fn sqref(c: &mut Cur) -> Option<Vec<Range>> {
    let n = c.u16()?;
    (0..n).map(|_| ref8(c)).collect()
}

fn origin(ranges: &[Range]) -> CellRef {
    ranges.iter().map(|r| r.start).min_by_key(|c| (c.row, c.col)).unwrap_or_default()
}

/// A differential format (`DXFN`) as far as rules draw it.
fn dxfn(c: &mut Cur, color: Color) -> Option<CfStyle> {
    let flags = c.u32()?;
    let flags2 = c.u16()?;
    let mut s = CfStyle::default();
    if flags & 1 << 25 != 0 {
        if flags2 & 1 != 0 {
            // A format code: its size, then the string.
            c.skip(2)?;
            let n = c.u16()? as usize;
            let wide = c.u8()? & 1 != 0;
            c.skip(if wide { n * 2 } else { n })?;
        } else {
            c.skip(2)?;
        }
    }
    if flags & 1 << 26 != 0 {
        let f = c.bytes(118)?;
        let mut f = Cur { d: f, p: 64 };
        let (_height, ts, weight, _sss, uls) = (f.u32()?, f.u32()?, f.u16()?, f.u16()?, f.u8()?);
        f.skip(3)?;
        let icv = f.u32()?;
        f.skip(4)?;
        let (ts_ninch, _sss_ninch, uls_ninch, bls_ninch) = (f.u32()?, f.u32()?, f.u32()?, f.u32()?);
        if ts_ninch & 2 == 0 {
            s.italic = Some(ts & 2 != 0);
        }
        if ts_ninch & 0x80 == 0 {
            s.strike = Some(ts & 0x80 != 0);
        }
        if bls_ninch == 0 {
            s.bold = Some(weight >= 600);
        }
        if uls_ninch == 0 {
            s.underline = Some(uls != 0);
        }
        s.color = u16::try_from(icv).ok().and_then(color);
    }
    if flags & 1 << 27 != 0 {
        c.skip(8)?;
    }
    if flags & 1 << 28 != 0 {
        c.skip(8)?;
    }
    if flags & 1 << 29 != 0 {
        let (w1, w2) = (c.u16()?, c.u16()?);
        let (fore, back) = (w2 & 0x7F, (w2 >> 7) & 0x7F);
        let pattern = (flags & 1 << 16 == 0).then_some(w1 >> 10);
        if pattern != Some(0) {
            s.fill = if flags & 1 << 18 == 0 { color(back) } else { None }.or_else(|| if flags & 1 << 17 == 0 { color(fore) } else { None });
        }
    }
    if flags & 1 << 30 != 0 {
        c.skip(2)?;
    }
    Some(s)
}

/// Excel 2007's exact colors after a `DXFN` (`XFExtNoFRT`).
fn xf_ext(c: &mut Cur, s: &mut CfStyle) -> Option<()> {
    c.skip(6)?;
    let n = c.u16()?;
    for _ in 0..n {
        let (ty, cb) = (c.u16()?, c.u16()? as usize);
        let mut d = Cur::new(c.bytes(cb.checked_sub(4)?)?);
        if matches!(ty, 0x05 | 0x0D) && d.u16()? == 2 {
            d.skip(2)?;
            let b = d.bytes(3)?;
            let rgb = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
            if ty == 0x05 {
                s.fill = Some(rgb);
            } else {
                s.color = Some(rgb);
            }
        }
    }
    Some(())
}

/// `DXFN12`: a size, the format and its exact colors.
fn dxfn12(c: &mut Cur, color: Color) -> Option<CfStyle> {
    let n = c.u32()? as usize;
    if n == 0 {
        c.skip(2)?;
        return Some(CfStyle::default());
    }
    let mut d = Cur::new(c.bytes(n)?);
    let mut s = dxfn(&mut d, color)?;
    if d.left() >= 8 {
        xf_ext(&mut d, &mut s);
    }
    Some(s)
}

/// A threshold (`CFVO`), with the extra bytes of its kind after it.
fn cfvo(c: &mut Cur, base: CellRef, fmla: Fmla) -> Option<Cfvo> {
    let ty = c.u8()?;
    let n = c.u16()? as usize;
    let f = c.bytes(n)?;
    if n > 0 {
        return Some(fmla(f, base).map_or(Cfvo::Min, Cfvo::Formula));
    }
    let v = if matches!(ty, 2 | 3) { 0.0 } else { c.f64()? };
    Some(match ty {
        2 => Cfvo::Min,
        3 => Cfvo::Max,
        4 => Cfvo::Percent(v),
        5 => Cfvo::Percentile(v),
        _ => Cfvo::Num(v),
    })
}

/// `CFColor`: automatic, indexed, RGB or theme color, and a tint.
fn cf_color(c: &mut Cur, color: Color) -> Option<Option<u32>> {
    let ty = c.u32()?;
    let v = c.bytes(4)?;
    c.skip(8)?;
    Some(match ty {
        1 => color(u16::from_le_bytes([v[0], v[1]])),
        2 => Some(u32::from(v[0]) << 16 | u32::from(v[1]) << 8 | u32::from(v[2])),
        _ => None,
    })
}

impl Tools {
    /// The current ranges for one more rule, or none once the sheet's
    /// rules have taken `MAX_CF_RANGES` in all.
    fn cf_ranges(&self) -> Vec<Range> {
        let used = self.cf_used.get() + self.ranges.len();
        if used > crate::MAX_CF_RANGES {
            return Vec::new();
        }
        self.cf_used.set(used);
        self.ranges.clone()
    }

    /// `CONDFMT` (and `CONDFMT12` after its 12-byte header).
    pub fn condfmt(&mut self, d: &[u8], frt: bool) {
        let mut c = Cur::new(d);
        let r = (|| {
            if frt {
                c.skip(12)?;
            }
            c.skip(2)?;
            let id = c.u16()? >> 1;
            ref8(&mut c)?;
            Some((id, sqref(&mut c)?))
        })();
        let (id, ranges) = r.unwrap_or_default();
        (self.id, self.ranges, self.icf) = (id, ranges, 0);
    }

    /// A rule of Excel 97 (`CF`).
    pub fn cf(&mut self, d: &[u8], fmla: Fmla, color: Color) {
        let icf = self.icf;
        self.icf += 1;
        let base = origin(&self.ranges);
        let mut c = Cur::new(d);
        let r = (|| {
            let (ct, cp, n1, n2) = (c.u8()?, c.u8()?, c.u16()? as usize, c.u16()? as usize);
            let style = dxfn(&mut c, color)?;
            let f1 = fmla(c.bytes(n1)?, base).unwrap_or_default();
            let f2 = fmla(c.bytes(n2)?, base).unwrap_or_default();
            let kind = match ct {
                1 => CfKind::CellIs { op: cmp_op(u32::from(cp)), f1, f2 },
                2 => CfKind::Expression(f1),
                _ => return None,
            };
            Some(CfRule { ranges: self.cf_ranges(), kind, style, stop: false })
        })();
        if let Some(rule) = r.filter(|r| !r.ranges.is_empty()) {
            self.rules.push((None, self.id, icf, rule));
        }
    }

    /// A rule of Excel 2007 (`CF12`).
    pub fn cf12(&mut self, d: &[u8], fmla: Fmla, color: Color) {
        let base = origin(&self.ranges);
        let mut c = Cur::new(d);
        let r = (|| {
            c.skip(12)?;
            let (ctp, cp, n1, n2) = (c.u8()?, c.u8()?, c.u16()? as usize, c.u16()? as usize);
            let style = dxfn12(&mut c, color)?;
            let f1 = fmla(c.bytes(n1)?, base).unwrap_or_default();
            let f2 = fmla(c.bytes(n2)?, base).unwrap_or_default();
            let n = c.u16()? as usize;
            c.skip(n)?;
            let opts = c.u8()?;
            let priority = c.u16()?;
            let template = c.u16()?;
            let n = c.u8()? as usize;
            c.skip(n)?;
            let kind = match ctp {
                1 => CfKind::CellIs { op: cmp_op(u32::from(cp)), f1, f2 },
                2 => CfKind::Expression(f1),
                3 => {
                    c.skip(3)?;
                    let (ni, ng) = (c.u8()?, c.u8()?);
                    c.skip(1)?;
                    let mut stops = Vec::new();
                    for _ in 0..ni {
                        stops.push(cfvo(&mut c, base, fmla)?);
                        c.skip(8)?;
                    }
                    let mut colors = Vec::new();
                    for _ in 0..ng {
                        c.skip(8)?;
                        colors.push(cf_color(&mut c, color)?.unwrap_or(0xFFFFFF));
                    }
                    if stops.len() < 2 || stops.len() != colors.len() {
                        return None;
                    }
                    CfKind::ColorScale(stops.into_iter().zip(colors).collect())
                }
                4 => {
                    c.skip(6)?;
                    let bar = cf_color(&mut c, color)?.unwrap_or(0x638EC6);
                    let min = cfvo(&mut c, base, fmla)?;
                    let max = cfvo(&mut c, base, fmla)?;
                    CfKind::DataBar { color: bar, min, max }
                }
                5 => match template {
                    0x07 => CfKind::Duplicates(false),
                    0x1B => CfKind::Duplicates(true),
                    0x19 | 0x1A | 0x1D | 0x1E => CfKind::Average { below: matches!(template, 0x1A | 0x1E), equal: template >= 0x1D },
                    _ => {
                        c.skip(3)?;
                        let flags = c.u8()?;
                        let rank = c.u16()?;
                        CfKind::Top { bottom: flags & 1 == 0, percent: flags & 2 != 0, rank: u32::from(rank) }
                    }
                },
                6 => {
                    c.skip(3)?;
                    let (n, set, flags) = (c.u8()?, c.u8()?, c.u8()?);
                    let mut points = Vec::new();
                    for i in 0..n {
                        let v = cfvo(&mut c, base, fmla)?;
                        c.skip(5)?;
                        if i > 0 {
                            points.push(v);
                        }
                    }
                    CfKind::IconSet { set: ICON_SETS.get(set as usize).unwrap_or(&"3TrafficLights1").to_string(), reverse: flags & 4 != 0, show_value: flags & 1 == 0, points }
                }
                _ => return None,
            };
            Some((priority, CfRule { ranges: self.cf_ranges(), kind, style, stop: opts & 2 != 0 }))
        })();
        if let Some((priority, rule)) = r.filter(|r| !r.1.ranges.is_empty()) {
            self.rules.push((Some(priority), self.id, u16::MAX, rule));
        }
    }

    /// `CFEX`: the priority of an Excel 97 rule, from Excel 2007.
    pub fn cfex(&mut self, d: &[u8], color: Color) {
        let mut c = Cur::new(d);
        let r = (|| {
            c.skip(12)?;
            if c.u32()? != 0 {
                return None;
            }
            let id = c.u16()?;
            let icf = c.u16()?;
            c.skip(2)?;
            let priority = c.u16()?;
            let flags = c.u8()?;
            let style = if c.u8()? != 0 { dxfn12(&mut c, color) } else { None };
            Some((id, icf, priority, flags & 2 != 0, style))
        })();
        self.ex.extend(r);
    }

    /// `DV`: a validation rule.
    pub fn dv(&mut self, d: &[u8], fmla: Fmla) {
        let mut c = Cur::new(d);
        let v = (|| {
            let flags = c.u32()?;
            let mut text = || -> Option<String> {
                let n = c.u16()? as usize;
                let wide = c.u8()? & 1 != 0;
                Some(c.chars8(n, wide)?.trim_end_matches('\0').to_string())
            };
            let (input_title, error_title, input_msg, error_msg) = (text()?, text()?, text()?, text()?);
            let n = c.u16()? as usize;
            c.skip(2)?;
            let f1 = c.bytes(n)?;
            let n = c.u16()? as usize;
            c.skip(2)?;
            let f2 = c.bytes(n)?;
            let ranges = sqref(&mut c)?;
            Some(validation(flags, ranges, [input_title, input_msg, error_title, error_msg], f1, f2, fmla))
        })();
        if let Some(v) = v.filter(|v| !v.ranges.is_empty() && self.validations.len() < crate::xlsx_data::MAX_VALIDATIONS) {
            self.validations.push(v);
        }
    }

    /// `AUTOFILTER`: the rule of one column.
    pub fn autofilter(&mut self, d: &[u8], text: &dyn Fn(&mut Cur, usize) -> Option<String>) {
        let Some(cols) = &mut self.filter else { return };
        let mut c = Cur::new(d);
        let r = (|| {
            let entry = c.u16()?;
            let flags = c.u16()?;
            if flags & 0x10 != 0 {
                let kind = FilterKind::Top { top: flags & 0x20 != 0, percent: flags & 0x40 != 0, n: f64::from(flags >> 7) };
                return Some((u32::from(entry), kind));
            }
            let dopers = [c.bytes(10)?, c.bytes(10)?];
            let mut conds: Vec<(CritOp, String)> = Vec::new();
            let mut blanks = false;
            for d in dopers {
                let op = match d[1] {
                    1 => CritOp::Lt,
                    3 => CritOp::Le,
                    4 => CritOp::Gt,
                    5 => CritOp::Ne,
                    6 => CritOp::Ge,
                    _ => CritOp::Eq,
                };
                let v = match d[0] {
                    2 => crate::xls::rk(u32::from_le_bytes([d[2], d[3], d[4], d[5]])).to_string(),
                    4 => f64::from_le_bytes(d[2..10].try_into().ok()?).to_string(),
                    6 => text(&mut c, d[6] as usize)?,
                    8 if d[2] == 0 => (if d[3] != 0 { "TRUE" } else { "FALSE" }).to_string(),
                    0x0C => {
                        blanks = true;
                        String::new()
                    }
                    0x0E => {
                        conds.push((CritOp::Ne, String::new()));
                        continue;
                    }
                    _ => continue,
                };
                conds.push((op, v));
            }
            let or = flags & 3 == 1;
            let kind = if (or || conds.len() + usize::from(blanks) == 1) && conds.iter().all(|(op, v)| *op == CritOp::Eq && !v.contains(['*', '?'])) {
                FilterKind::Values { items: conds.into_iter().map(|x| x.1).filter(|v| !v.is_empty()).collect(), blanks }
            } else {
                let mut it = conds.into_iter();
                FilterKind::Custom { and: !or, a: it.next()?, b: it.next() }
            };
            Some((u32::from(entry), kind))
        })();
        cols.extend(r);
    }

    /// Adds the rules to the sheet, in their priority order.
    pub fn finish(mut self, sh: &mut apex_sheet::Sheet) {
        // The rules by their (id, icf), first first: each CFEX takes the
        // next one without a priority.
        let mut by_id: std::collections::HashMap<(u16, u16), std::collections::VecDeque<usize>> = std::collections::HashMap::new();
        for (i, r) in self.rules.iter().enumerate() {
            if r.0.is_none() {
                by_id.entry((r.1, r.2)).or_default().push_back(i);
            }
        }
        for (id, icf, priority, stop, style) in std::mem::take(&mut self.ex) {
            if let Some(r) = by_id.get_mut(&(id, icf)).and_then(|q| q.pop_front()).map(|i| &mut self.rules[i]) {
                r.0 = Some(priority);
                r.3.stop = stop;
                if let Some(s) = style {
                    r.3.style = s;
                }
            }
        }
        // Stable: rules without a priority keep the file's order, last.
        self.rules.sort_by_key(|r| r.0.map_or(u32::MAX, u32::from));
        sh.cond_formats.extend(self.rules.into_iter().map(|r| r.3));
        sh.validations.extend(self.validations);
    }
}

/// A validation from the fields .xls and .xlsb share: the flags (`DValFlags`),
/// its ranges, texts (input title and message, error title and message)
/// and formulas.
pub(crate) fn validation(flags: u32, ranges: Vec<Range>, texts: [String; 4], f1: &[u8], f2: &[u8], fmla: Fmla) -> Validation {
    let base = origin(&ranges);
    let mut v = Validation::new(ranges);
    v.kind = ValKind::ALL.get((flags & 15) as usize).copied().unwrap_or_default();
    v.error_style = match (flags >> 4) & 7 {
        1 => ErrorStyle::Warning,
        2 => ErrorStyle::Info,
        _ => ErrorStyle::Stop,
    };
    v.allow_blank = flags & 1 << 8 != 0;
    v.dropdown = flags & 1 << 9 == 0;
    v.show_input = flags & 1 << 18 != 0;
    v.show_error = flags & 1 << 19 != 0;
    v.op = CmpOp::ALL.get(((flags >> 20) & 15) as usize).copied().unwrap_or_default();
    [v.input_title, v.input_msg, v.error_title, v.error_msg] = texts;
    v.f1 = fmla(f1, base).unwrap_or_default();
    if v.kind == ValKind::List && flags & 0x80 != 0 {
        // A literal list: one string with its items split by NULs.
        v.f1 = v.f1.replace('\0', ",");
    }
    if v.kind.has_op() && v.op.two_args() {
        v.f2 = fmla(f2, base).unwrap_or_default();
    }
    v
}

/// The autofilter of the columns read for a sheet, on the range of its
/// `_FilterDatabase` name.
pub(crate) fn filter(range: Range, cols: Columns) -> apex_sheet::data::AutoFilter {
    let mut f = apex_sheet::data::AutoFilter::new(range);
    for (i, kind) in cols {
        let col = range.start.col + i;
        if col <= range.end.col && f.column(col).is_none() {
            f.cols.push(ColumnFilter { col, kind });
        }
    }
    f
}
