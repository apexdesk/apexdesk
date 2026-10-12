//! Sheet data tools of .xlsb files: conditional formats
//! (`BrtBeginConditionalFormatting`, `BrtBeginCFRule` and the color scale,
//! data bar and icon set records in it), their differential formats
//! (`BrtDXF` in `styles.bin`), data validation (`BrtDVal`) and the
//! autofilter (`BrtBeginAFilter`).

use apex_sheet::data::{AutoFilter, CfKind, CfRule, CfStyle, Cfvo, ColumnFilter, FilterKind, Period, TextOp};
use apex_sheet::functions::CritOp;
use apex_sheet::{CellRef, Range, Sheet};

use crate::biff::Cur;
use crate::xls_data::{Fmla, ICON_SETS, cmp_op};

/// A color (`BrtColor`, `XFPropColor`) as RGB.
pub(crate) type Color<'a> = &'a dyn Fn(&mut Cur) -> Option<Option<u32>>;

/// `XLWideString` (a null one is empty).
fn wide(c: &mut Cur) -> Option<String> {
    let n = c.u32()? as usize;
    if n == 0xFFFF_FFFF {
        return Some(String::new());
    }
    c.utf16(n.min(c.left() / 2))
}

fn rfx(c: &mut Cur) -> Option<Range> {
    let (r1, r2, c1, c2) = (c.u32()?, c.u32()?, c.u32()?, c.u32()?);
    Some(Range::new(CellRef::new(r1, c1), CellRef::new(r2, c2)))
}

/// `UncheckedSqRfX`: a count and the ranges.
fn sqrfx(c: &mut Cur) -> Option<Vec<Range>> {
    let n = c.u32()?;
    (0..n.min(10_000)).map(|_| rfx(c)).collect()
}

/// A parsed formula (`cce`, tokens, `cb`, extra data) as text.
fn formula(c: &mut Cur, base: CellRef, fmla: Fmla) -> Option<String> {
    let n = c.u32()? as usize;
    let rgce = c.bytes(n)?;
    let cb = c.u32()? as usize;
    c.skip(cb.min(c.left()))?;
    fmla(rgce, base)
}

/// The differential formats of `styles.bin` (`BrtDXF`).
pub(crate) fn read_dxfs(recs: &[(u32, &[u8])], color: Color) -> Vec<CfStyle> {
    let mut out = Vec::new();
    for (ty, d) in recs {
        if *ty != 0x01FB {
            continue;
        }
        let mut c = Cur::new(d);
        let mut s = CfStyle::default();
        let _ = (|| -> Option<()> {
            c.skip(2)?;
            let n = c.u16()?;
            let mut fill = (None, None);
            for _ in 0..n {
                let ty = c.u16()?;
                let cb = c.u16()? as usize;
                let mut p = Cur::new(c.bytes(cb.checked_sub(4)?)?);
                match ty {
                    0x01 => fill.0 = color(&mut p)?,
                    0x02 => fill.1 = color(&mut p)?,
                    0x05 => s.color = color(&mut p)?,
                    0x18 => s.bold = Some(p.u16()? >= 600),
                    0x19 => s.italic = Some(p.u8()? != 0),
                    0x1A => s.underline = Some(p.u8()? != 0),
                    0x1C => s.strike = Some(p.u8()? != 0),
                    _ => {}
                }
            }
            // Solid fills of differential formats keep their color as the
            // background.
            s.fill = fill.1.or(fill.0);
            Some(())
        })();
        out.push(s);
    }
    out
}

/// A conditional format rule being read.
struct Rule {
    ranges: Vec<Range>,
    priority: u32,
    kind: Option<CfKind>,
    style: CfStyle,
    stop: bool,
    /// Color scale, data bar or icon set: its thresholds and colors.
    graphic: u32,
    cfvos: Vec<Cfvo>,
    colors: Vec<u32>,
    icons: (u32, u8),
}

#[derive(Default)]
pub(crate) struct Tools {
    ranges: Vec<Range>,
    rule: Option<Rule>,
    rules: Vec<(u32, CfRule)>,
    filter: Option<AutoFilter>,
    col: Option<ColumnFilter>,
    customs: (bool, Vec<(CritOp, String)>),
    /// Ranges the rules have taken so far.
    cf_used: std::cell::Cell<usize>,
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

    /// Reads one record of a sheet part.
    pub fn record(&mut self, sh: &mut Sheet, ty: u32, d: &[u8], dxfs: &[CfStyle], fmla: Fmla, color: Color) {
        let mut c = Cur::new(d);
        match ty {
            0x01CD => {
                let r = (|| {
                    c.skip(8)?;
                    sqrfx(&mut c)
                })();
                self.ranges = r.unwrap_or_default();
            }
            0x01CF => self.rule = self.cf_rule(d, dxfs, fmla),
            0x01D0 => {
                if let Some(r) = self.rule.take() {
                    self.finish_rule(r);
                }
            }
            0x01D1 => {
                if let (Some(r), Some(set), Some(flags)) = (&mut self.rule, c.u32(), c.u8()) {
                    r.icons = (set, flags);
                }
            }
            0x01D7 => {
                let base = self.ranges.first().map(|r| r.start).unwrap_or_default();
                if let (Some(r), Some(v)) = (&mut self.rule, cfvo(d, base, fmla)) {
                    r.cfvos.push(v);
                }
            }
            // A color in a rule (`BrtColor`).
            _ if d.len() == 8 && self.rule.as_ref().is_some_and(|r| r.graphic != 0) => {
                if let (Some(r), Some(Some(v))) = (&mut self.rule, color(&mut c)) {
                    r.colors.push(v);
                }
            }
            0x0040 if sh.validations.len() < crate::xlsx_data::MAX_VALIDATIONS => {
                if let Some(v) = dval(d, fmla) {
                    sh.validations.push(v);
                }
            }
            0x00A1 => self.filter = rfx(&mut c).map(AutoFilter::new),
            0x00A3 => {
                if let (Some(f), Some(i)) = (&self.filter, c.u32()) {
                    self.col = Some(ColumnFilter { col: f.range.start.col.saturating_add(i), kind: FilterKind::Values { items: Vec::new(), blanks: false } });
                    self.customs = (false, Vec::new());
                }
            }
            0x00A5 => {
                if let (Some(col), Some(blank)) = (&mut self.col, c.u32()) {
                    col.kind = FilterKind::Values { items: Vec::new(), blanks: blank != 0 };
                }
            }
            0x00A7 => {
                if let (Some(ColumnFilter { kind: FilterKind::Values { items, .. }, .. }), Some(v)) = (&mut self.col, wide(&mut c)) {
                    items.push(v);
                }
            }
            0x00AC => self.customs = (c.u32().is_some_and(|a| a != 0), Vec::new()),
            0x00AE => {
                if let Some(x) = custom_filter(d) {
                    self.customs.1.push(x);
                }
            }
            0x00AA => {
                if let (Some(col), Some(flags), Some(v)) = (&mut self.col, c.u32(), c.f64()) {
                    col.kind = FilterKind::Top { top: flags & 1 != 0, percent: flags & 2 != 0, n: v };
                }
            }
            0x00A4 => {
                if let (Some(f), Some(mut col)) = (&mut self.filter, self.col.take()) {
                    let (and, customs) = std::mem::take(&mut self.customs);
                    let mut it = customs.into_iter();
                    if let Some(a) = it.next() {
                        col.kind = FilterKind::Custom { and, a, b: it.next() };
                    }
                    // Columns with only a button do not filter.
                    if col.kind != (FilterKind::Values { items: Vec::new(), blanks: false }) {
                        f.cols.push(col);
                    }
                }
            }
            0x00A2 => {
                if let Some(f) = self.filter.take() {
                    sh.filter = Some(f);
                }
            }
            _ => {}
        }
    }

    /// `BrtBeginCFRule`.
    fn cf_rule(&self, d: &[u8], dxfs: &[CfStyle], fmla: Fmla) -> Option<Rule> {
        let base = self.ranges.iter().map(|r| r.start).min_by_key(|c| (c.row, c.col)).unwrap_or_default();
        let mut c = Cur::new(d);
        let (ty, template, dxf, priority, param) = (c.u32()?, c.u32()?, c.u32()?, c.u32()?, c.u32()?);
        c.skip(8)?;
        let flags = c.u16()?;
        let (n1, n2, n3) = (c.u32()? as usize, c.u32()? as usize, c.u32()? as usize);
        let text = wide(&mut c)?;
        let mut f = [String::new(), String::new()];
        for (i, n) in [n1, n2].into_iter().enumerate() {
            let mut p = Cur::new(c.bytes(n)?);
            if n > 0 {
                f[i] = formula(&mut p, base, fmla).unwrap_or_default();
            }
        }
        c.skip(n3)?;
        let [f1, f2] = f;
        let period = |i: u32| ["today", "tomorrow", "yesterday", "last7Days", "lastMonth", "nextMonth", "thisWeek", "nextWeek", "lastWeek", "thisMonth"].get(i as usize).copied();
        let kind = match (ty, template) {
            (3 | 4 | 6, _) => None,
            (1, _) => Some(CfKind::CellIs { op: cmp_op(param), f1, f2 }),
            (_, 0x05) => Some(CfKind::Top { bottom: flags & 8 != 0, percent: flags & 0x10 != 0, rank: param }),
            (_, 0x07) => Some(CfKind::Duplicates(false)),
            (_, 0x1B) => Some(CfKind::Duplicates(true)),
            (_, 0x08) => Some(CfKind::Text {
                op: match param {
                    1 => TextOp::NotContains,
                    2 => TextOp::BeginsWith,
                    3 => TextOp::EndsWith,
                    _ => TextOp::Contains,
                },
                text,
            }),
            (_, 0x09 | 0x0A) => Some(CfKind::Blanks(template == 0x09)),
            (_, 0x0B | 0x0C) => Some(CfKind::Errors(template == 0x0B)),
            (_, 0x0F..=0x18) => Some(CfKind::Period(Period::from_xlsx(period(template - 0x0F).unwrap_or("today")))),
            (_, 0x19 | 0x1A | 0x1D | 0x1E) => Some(CfKind::Average { below: matches!(template, 0x1A | 0x1E), equal: template >= 0x1D }),
            _ => Some(CfKind::Expression(f1)),
        };
        let style = dxfs.get(dxf as usize).cloned().unwrap_or_default();
        Some(Rule { ranges: self.cf_ranges(), priority, kind, style, stop: flags & 2 != 0, graphic: if matches!(ty, 3 | 4 | 6) { ty } else { 0 }, cfvos: Vec::new(), colors: Vec::new(), icons: (0, 0) })
    }

    fn finish_rule(&mut self, r: Rule) {
        let kind = match r.graphic {
            3 if r.cfvos.len() >= 2 && r.cfvos.len() == r.colors.len() => Some(CfKind::ColorScale(r.cfvos.into_iter().zip(r.colors).collect())),
            4 if r.cfvos.len() >= 2 => {
                let mut v = r.cfvos.into_iter();
                let (min, max) = (v.next().unwrap_or(Cfvo::Min), v.next().unwrap_or(Cfvo::Max));
                Some(CfKind::DataBar { color: r.colors.first().copied().unwrap_or(0x638EC6), min, max })
            }
            6 => Some(CfKind::IconSet {
                set: ICON_SETS.get(r.icons.0 as usize).unwrap_or(&"3TrafficLights1").to_string(),
                reverse: r.icons.1 & 2 != 0,
                show_value: r.icons.1 & 1 == 0,
                points: r.cfvos.into_iter().skip(1).collect(),
            }),
            0 => r.kind,
            _ => None,
        };
        if let (Some(kind), false) = (kind, r.ranges.is_empty()) {
            self.rules.push((r.priority, CfRule { ranges: r.ranges, kind, style: r.style, stop: r.stop }));
        }
    }

    /// Adds the conditional formats in their priority order.
    pub fn finish(mut self, sh: &mut Sheet) {
        self.rules.sort_by_key(|r| r.0);
        sh.cond_formats.extend(self.rules.into_iter().map(|r| r.1));
    }
}

/// `BrtCFVO`: type and number, then (for a formula) its size and tokens.
fn cfvo(d: &[u8], base: CellRef, fmla: Fmla) -> Option<Cfvo> {
    let mut c = Cur::new(d);
    let ty = c.u32()?;
    let v = c.f64()?;
    Some(match ty {
        2 => Cfvo::Min,
        3 => Cfvo::Max,
        4 => Cfvo::Percent(v),
        5 => Cfvo::Percentile(v),
        7 => {
            // The formula's size follows one or two flag words.
            let f = [4usize, 8].into_iter().find_map(|skip| {
                let mut p = Cur { d, p: 12 + skip };
                let n = p.u32()? as usize;
                (n > 0 && n == p.left()).then(|| formula(&mut p, base, fmla)).flatten()
            });
            f.map_or(Cfvo::Num(v), Cfvo::Formula)
        }
        _ => Cfvo::Num(v),
    })
}

/// `BrtDVal`: flags, ranges, the error and input texts and two formulas.
fn dval(d: &[u8], fmla: Fmla) -> Option<apex_sheet::data::Validation> {
    let mut c = Cur::new(d);
    let flags = c.u32()?;
    let ranges = sqrfx(&mut c)?;
    let (error_title, error_msg, input_title, input_msg) = (wide(&mut c)?, wide(&mut c)?, wide(&mut c)?, wide(&mut c)?);
    let mut f = [Vec::new(), Vec::new()];
    for x in &mut f {
        let n = c.u32()? as usize;
        *x = c.bytes(n)?.to_vec();
        let cb = c.u32()? as usize;
        c.skip(cb.min(c.left()))?;
    }
    let v = crate::xls_data::validation(flags, ranges, [input_title, input_msg, error_title, error_msg], &f[0], &f[1], fmla);
    (!v.ranges.is_empty()).then_some(v)
}

/// `BrtCustomFilter`: the value's type, the operator and the value.
fn custom_filter(d: &[u8]) -> Option<(CritOp, String)> {
    let mut c = Cur::new(d);
    let (vt, sign) = (c.u8()?, c.u8()?);
    let op = match sign {
        1 => CritOp::Lt,
        3 => CritOp::Le,
        4 => CritOp::Gt,
        5 => CritOp::Ne,
        6 => CritOp::Ge,
        _ => CritOp::Eq,
    };
    let v = match vt {
        4 => c.f64()?.to_string(),
        6 => {
            // The string may come after the 8-byte value union.
            let n = Cur { d, p: 2 }.u32()? as usize;
            if n * 2 + 6 != d.len() {
                c.skip(8)?;
            }
            wide(&mut c)?
        }
        8 => (if c.u8()? != 0 { "TRUE" } else { "FALSE" }).to_string(),
        0x0C => String::new(),
        0x0E => return Some((CritOp::Ne, String::new())),
        _ => return None,
    };
    Some((op, v))
}
