//! Printing setup of a sheet from macros: Worksheet.PageSetup (paper,
//! margins, scale, print area, titles, headers and footers), the manual
//! page breaks (HPageBreaks, VPageBreaks, Range.PageBreak) and
//! Application.InchesToPoints. Printing itself stays the user's.

use apex_sheet::page::HfText;
use apex_sheet::{CellRef, MAX_COLS, MAX_ROWS, Range, SheetPage};

use super::{Args, Interp, obj, range_obj};
use crate::error::{Error, R};
use crate::object::{Obj, Part, RangeRef};
use crate::value::Value;

/// XlPaperSize of the paper sizes a sheet offers, and their portrait
/// size in points.
const PAPERS: [(i64, f32, f32); 8] = [(9, 595.3, 841.9), (1, 612.0, 792.0), (5, 612.0, 1008.0), (8, 841.9, 1190.6), (11, 419.5, 595.3), (13, 498.9, 708.7), (7, 522.0, 756.0), (3, 792.0, 1224.0)];

fn part(p: Part) -> Value {
    obj(Obj::Part(Box::new(p)))
}

fn object_error() -> Error {
    Error::app("Application-defined or object-defined error")
}

/// `$A$1`-style text of a range.
fn address(r: &Range) -> String {
    let cell = |c: CellRef| format!("${}${}", apex_sheet::addr::col_name(c.col), c.row + 1);
    if r.is_single() { cell(r.start) } else { format!("{}:{}", cell(r.start), cell(r.end)) }
}

/// A part of an address without its sheet (`'My sheet'!$A$1` → `$A$1`).
fn local(s: &str) -> &str {
    s.rsplit_once('!').map_or(s, |(_, a)| a).trim()
}

/// `$1:$3` (rows) or `$A:$B` (columns) as an inclusive span.
fn span(s: &str, rows: bool) -> R<Option<(u32, u32)>> {
    let s = local(s).replace('$', "");
    if s.is_empty() {
        return Ok(None);
    }
    let (a, b) = s.split_once(':').unwrap_or((&s, &s));
    let one = |t: &str| -> Option<u32> {
        if rows {
            t.parse::<u32>().ok().filter(|n| (1..=MAX_ROWS).contains(n)).map(|n| n - 1)
        } else {
            apex_sheet::addr::parse_col(t).filter(|c| *c < MAX_COLS)
        }
    };
    match (one(a), one(b)) {
        (Some(x), Some(y)) => Ok(Some((x.min(y), x.max(y)))),
        _ => Err(object_error()),
    }
}

impl Interp<'_> {
    fn page(&self, s: usize) -> R<SheetPage> {
        Ok(self.wb.sheets.get(s).ok_or_else(Error::subscript)?.page.clone())
    }

    fn set_page(&mut self, s: usize, page: SheetPage) -> R<()> {
        self.via(|wb| wb.set_page_setup(s, page))
    }

    /// One of the six headers and footers: `kind` 0 the usual ones, 1
    /// even pages', 2 the first page's; `slot` left, center, right
    /// header then footer.
    fn hf(page: &mut SheetPage, kind: u8, slot: u8) -> &mut String {
        let hf = &mut page.hf;
        let t: &mut HfText = match (kind, slot / 3) {
            (1, 0) => &mut hf.even_header,
            (1, _) => &mut hf.even_footer,
            (2, 0) => &mut hf.first_header,
            (2, _) => &mut hf.first_footer,
            (_, 0) => &mut hf.header,
            _ => &mut hf.footer,
        };
        match slot % 3 {
            0 => &mut t.left,
            1 => &mut t.center,
            _ => &mut t.right,
        }
    }

    fn hf_slot(name: &str) -> Option<u8> {
        ["leftheader", "centerheader", "rightheader", "leftfooter", "centerfooter", "rightfooter"].iter().position(|x| *x == name).map(|i| i as u8)
    }

    pub(super) fn page_member(&mut self, s: usize, name: &str) -> R<Value> {
        let mut p = self.page(s)?;
        if let Some(slot) = Self::hf_slot(name) {
            return Ok(Value::str(Self::hf(&mut p, 0, slot).clone()));
        }
        Ok(match name {
            "orientation" => Value::Long(if p.landscape { 2 } else { 1 }),
            "papersize" => {
                let near = PAPERS.iter().min_by(|a, b| ((a.1 - p.paper_width).abs() + (a.2 - p.paper_height).abs()).total_cmp(&((b.1 - p.paper_width).abs() + (b.2 - p.paper_height).abs())));
                Value::Long(near.map_or(9, |x| x.0 as i32))
            }
            "zoom" => match p.fit {
                Some(_) => Value::Bool(false),
                None => Value::Long(p.scale as i32),
            },
            "fittopageswide" | "fittopagestall" => match p.fit {
                Some((w, h)) => {
                    let n = if name == "fittopageswide" { w } else { h };
                    if n == 0 { Value::Bool(false) } else { Value::Long(n as i32) }
                }
                None => Value::Long(1),
            },
            "leftmargin" => Value::Double(p.margin_left as f64),
            "rightmargin" => Value::Double(p.margin_right as f64),
            "topmargin" => Value::Double(p.margin_top as f64),
            "bottommargin" => Value::Double(p.margin_bottom as f64),
            "headermargin" => Value::Double(p.margin_header as f64),
            "footermargin" => Value::Double(p.margin_footer as f64),
            "printarea" => Value::str(p.print_areas.iter().map(address).collect::<Vec<_>>().join(",")),
            "printtitlerows" => Value::str(p.title_rows.map_or(String::new(), |(a, b)| format!("${}:${}", a + 1, b + 1))),
            "printtitlecolumns" => Value::str(p.title_cols.map_or(String::new(), |(a, b)| format!("${}:${}", apex_sheet::addr::col_name(a), apex_sheet::addr::col_name(b)))),
            "centerhorizontally" => Value::Bool(p.center_h),
            "centervertically" => Value::Bool(p.center_v),
            "printgridlines" => Value::Bool(p.gridlines),
            "printheadings" => Value::Bool(p.headings),
            "order" => Value::Long(if p.across_first { 2 } else { 1 }),
            "firstpagenumber" => Value::Long(p.first_page.map_or(-4105, |n| n as i32)),
            "differentfirstpageheaderfooter" => Value::Bool(p.hf.different_first),
            "oddandevenpagesheaderfooter" => Value::Bool(p.hf.odd_even),
            "evenpage" => part(Part::HfPage(s, 1)),
            "firstpage" => part(Part::HfPage(s, 2)),
            "blackandwhite" | "draft" => Value::Bool(false),
            "printcomments" => Value::Long(-4142),
            "printerrors" => Value::Long(0),
            "scalewithdocheaderfooter" | "alignmarginsheaderfooter" => Value::Bool(true),
            "parent" => obj(Obj::Sheet(s)),
            _ => return Err(Error::no_member(name)),
        })
    }

    pub(super) fn page_set(&mut self, s: usize, name: &str, v: Value) -> R<()> {
        let mut p = self.page(s)?;
        let pts = |v: &Value| -> R<f32> {
            let x = v.to_f64()?;
            if !(0.0..=1440.0).contains(&x) {
                return Err(Error::app(format!("Unable to set the {name} property of the PageSetup class")));
            }
            Ok(x as f32)
        };
        if let Some(slot) = Self::hf_slot(name) {
            *Self::hf(&mut p, 0, slot) = v.to_str()?;
            return self.set_page(s, p);
        }
        match name {
            "orientation" => {
                p.landscape = match v.to_i64()? {
                    1 => false,
                    2 => true,
                    _ => return Err(Error::invalid_call()),
                }
            }
            "papersize" => {
                let n = v.to_i64()?;
                let (_, w, h) = PAPERS.iter().find(|x| x.0 == n).ok_or_else(|| Error::unsupported("this paper size"))?;
                (p.paper_width, p.paper_height) = (*w, *h);
            }
            "zoom" => match v {
                Value::Bool(false) => {
                    if p.fit.is_none() {
                        p.fit = Some((1, 1));
                    }
                }
                v => {
                    let z = v.to_i64()?;
                    if !(10..=400).contains(&z) {
                        return Err(Error::app("Unable to set the Zoom property of the PageSetup class"));
                    }
                    p.scale = z as u16;
                    p.fit = None;
                }
            },
            "fittopageswide" | "fittopagestall" => {
                // They count once Zoom is False (as in Excel).
                if let Some((w, h)) = &mut p.fit {
                    let n = match v {
                        Value::Bool(false) => 0,
                        v => u16::try_from(v.to_i64()?).ok().filter(|n| *n <= 32767).ok_or_else(Error::invalid_call)?,
                    };
                    *(if name == "fittopageswide" { w } else { h }) = n;
                }
            }
            "leftmargin" => p.margin_left = pts(&v)?,
            "rightmargin" => p.margin_right = pts(&v)?,
            "topmargin" => p.margin_top = pts(&v)?,
            "bottommargin" => p.margin_bottom = pts(&v)?,
            "headermargin" => p.margin_header = pts(&v)?,
            "footermargin" => p.margin_footer = pts(&v)?,
            "printarea" => {
                let t = match v {
                    Value::Bool(false) => String::new(),
                    v => v.to_str()?,
                };
                let mut list = Vec::new();
                for a in t.split(',').map(local).filter(|a| !a.is_empty()) {
                    let r = Range::parse(&a.replace('$', "")).ok_or_else(object_error)?;
                    list.push(r);
                    if list.len() > 1000 {
                        return Err(object_error());
                    }
                }
                p.print_areas = list;
            }
            "printtitlerows" => p.title_rows = span(&v.to_str()?, true)?,
            "printtitlecolumns" => p.title_cols = span(&v.to_str()?, false)?,
            "centerhorizontally" => p.center_h = v.to_bool()?,
            "centervertically" => p.center_v = v.to_bool()?,
            "printgridlines" => p.gridlines = v.to_bool()?,
            "printheadings" => p.headings = v.to_bool()?,
            "order" => p.across_first = v.to_i64()? == 2,
            "firstpagenumber" => {
                p.first_page = match v.to_i64()? {
                    -4105 => None,
                    n => Some(u32::try_from(n).map_err(|_| Error::invalid_call())?),
                }
            }
            "differentfirstpageheaderfooter" => p.hf.different_first = v.to_bool()?,
            "oddandevenpagesheaderfooter" => p.hf.odd_even = v.to_bool()?,
            // How the printer prints: the printing dialog has them.
            "blackandwhite" | "draft" | "printcomments" | "printerrors" | "printquality" | "scalewithdocheaderfooter" | "alignmarginsheaderfooter" | "printnotes" => return Ok(()),
            _ => return Err(Error::no_member(name)),
        }
        self.set_page(s, p)
    }

    /// PageSetup.EvenPage and FirstPage: their LeftHeader... sections.
    pub(super) fn hf_page_member(&mut self, s: usize, kind: u8, name: &str) -> R<Value> {
        match Self::hf_slot(name) {
            Some(slot) => Ok(part(Part::HfSection(s, kind, slot))),
            None => Err(Error::no_member(name)),
        }
    }

    /// A section's `.Text`.
    pub(super) fn hf_section(&mut self, s: usize, kind: u8, slot: u8, name: &str, set: Option<Value>) -> R<Value> {
        if name != "text" {
            return Err(Error::no_member(name));
        }
        let mut p = self.page(s)?;
        match set {
            None => Ok(Value::str(Self::hf(&mut p, kind, slot).clone())),
            Some(v) => {
                *Self::hf(&mut p, kind, slot) = v.to_str()?;
                self.set_page(s, p)?;
                Ok(Value::Empty)
            }
        }
    }

    /// The manual breaks of a sheet: before these rows (or columns).
    fn breaks(&self, s: usize, rows: bool) -> R<Vec<u32>> {
        let p = &self.wb.sheets.get(s).ok_or_else(Error::subscript)?.page;
        Ok(if rows { &p.row_breaks } else { &p.col_breaks }.iter().copied().collect())
    }

    fn set_break(&mut self, s: usize, rows: bool, at: u32, on: bool) -> R<()> {
        let mut p = self.page(s)?;
        let set = if rows { &mut p.row_breaks } else { &mut p.col_breaks };
        if on {
            if set.len() >= 1026 {
                return Err(object_error());
            }
            set.insert(at);
        } else {
            set.remove(&at);
        }
        self.set_page(s, p)
    }

    pub(super) fn breaks_member(&mut self, s: usize, rows: bool, name: &str, a: &Args) -> R<Value> {
        let list = self.breaks(s, rows)?;
        match name {
            "count" => Ok(Value::Long(list.len() as i32)),
            "item" | "_default" => {
                let i = self.arg_i64(a, 0, "index")?.ok_or_else(Error::invalid_call)?;
                let at = usize::try_from(i - 1).ok().and_then(|i| list.get(i)).ok_or_else(Error::subscript)?;
                Ok(part(Part::PageBreak(s, rows, *at)))
            }
            "add" => {
                let r = match a.get(0, "before") {
                    Some(Value::Object(Obj::Range(r))) if r.sheet == s => r,
                    Some(_) => return Err(Error::type_mismatch()),
                    None => return Err(Error::invalid_call()),
                };
                let at = if rows { r.range.start.row } else { r.range.start.col };
                if at == 0 {
                    return Err(object_error());
                }
                self.set_break(s, rows, at, true)?;
                Ok(part(Part::PageBreak(s, rows, at)))
            }
            _ => Err(Error::no_member(name)),
        }
    }

    pub(super) fn break_member(&mut self, s: usize, rows: bool, at: u32, name: &str) -> R<Value> {
        if !self.breaks(s, rows)?.contains(&at) {
            return Err(object_error());
        }
        Ok(match name {
            "location" => {
                let r = if rows { Range::new(CellRef::new(at, 0), CellRef::new(at, MAX_COLS - 1)) } else { Range::new(CellRef::new(0, at), CellRef::new(MAX_ROWS - 1, at)) };
                range_obj(s, r)
            }
            "type" => Value::Long(-4135),
            "delete" => {
                self.set_break(s, rows, at, false)?;
                Value::Empty
            }
            _ => return Err(Error::no_member(name)),
        })
    }

    /// Worksheet.ResetAllPageBreaks.
    pub(super) fn reset_breaks(&mut self, s: usize) -> R<Value> {
        let mut p = self.page(s)?;
        p.row_breaks.clear();
        p.col_breaks.clear();
        self.set_page(s, p)?;
        Ok(Value::Empty)
    }

    /// Range.PageBreak: a manual break above its first row (or before its
    /// first column, for whole columns).
    pub(super) fn page_break(&mut self, r: RangeRef, set: Option<Value>) -> R<Value> {
        let rows = !(r.range.start.row == 0 && r.range.end.row == MAX_ROWS - 1);
        let at = if rows { r.range.start.row } else { r.range.start.col };
        match set {
            None => Ok(Value::Long(if self.breaks(r.sheet, rows)?.contains(&at) { -4135 } else { -4142 })),
            Some(v) => {
                let on = match v.to_i64()? {
                    -4142 => false,
                    -4135 | -4105 => true,
                    _ => return Err(Error::invalid_call()),
                };
                if on && at == 0 {
                    return Err(object_error());
                }
                self.set_break(r.sheet, rows, at, on)?;
                Ok(Value::Empty)
            }
        }
    }
}
