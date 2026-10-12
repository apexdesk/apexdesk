//! Excel's object model over the workbook: Application, Workbook,
//! Worksheets, Range and Cells with their values, formulas and formats,
//! WorksheetFunction, and the Err, Collection and Dictionary objects.
//! Every change is an undoable op (see `Workbook::apply_op`).

use std::rc::Rc;
use std::sync::Arc;

use apex_sheet::format::{Locale, parse_input};
use apex_sheet::sheet::{Content, HAlign, VAlign};
use apex_sheet::{Cell, CellRef, CellStyle, ErrorKind, MAX_COLS, MAX_ROWS, Op, Range, Sheet};

use crate::ast::Arg;
use crate::error::{Error, R};
use crate::interp::Interp;
use crate::object::{By, Obj, RangeRef};
use crate::value::{Array, VType, Value, number_text};

/// Arguments by position or by name (`After:=ws`).
pub struct Args(Vec<(Option<String>, Value)>);

impl Args {
    pub fn get(&self, pos: usize, name: &str) -> Option<Value> {
        let v = match self.0.iter().find(|(n, _)| n.as_deref() == Some(name)) {
            Some((_, v)) => v.clone(),
            None => self.0.get(pos).filter(|(n, _)| n.is_none())?.1.clone(),
        };
        (!matches!(v, Value::Missing)).then_some(v)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Excel's 56-color palette (ColorIndex), as 0xRRGGBB.
const PALETTE: [u32; 56] = [
    0x000000, 0xFFFFFF, 0xFF0000, 0x00FF00, 0x0000FF, 0xFFFF00, 0xFF00FF, 0x00FFFF, 0x800000, 0x008000, 0x000080, 0x808000, 0x800080, 0x008080, 0xC0C0C0, 0x808080, 0x9999FF, 0x993366, 0xFFFFCC, 0xCCFFFF, 0x660066, 0xFF8080, 0x0066CC, 0xCCCCFF, 0x000080, 0xFF00FF, 0xFFFF00, 0x00FFFF, 0x800080, 0x800000, 0x008080, 0x0000FF, 0x00CCFF, 0xCCFFFF, 0xCCFFCC, 0xFFFF99, 0x99CCFF, 0xFF99CC, 0xCC99FF, 0xFFCC99, 0x3366FF, 0x33CCCC, 0x99CC00, 0xFFCC00, 0xFF9900, 0xFF6600, 0x666699, 0x969696, 0x003366, 0x339966, 0x003300, 0x333300, 0x993300, 0x993366, 0x333399, 0x333333,
];

/// VBA colors are 0xBBGGRR; cells keep 0xRRGGBB (the same swap both ways).
fn swap_rb(c: u32) -> u32 {
    ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF)
}

const COL_SCALE: f32 = 88.0 / 64.0;
const ROW_SCALE: f32 = 22.0 / 15.0;

fn error_code(k: ErrorKind) -> i32 {
    match k {
        ErrorKind::Null => 2000,
        ErrorKind::Div0 => 2007,
        ErrorKind::Ref => 2023,
        ErrorKind::Name => 2029,
        ErrorKind::Num => 2036,
        ErrorKind::Na => 2042,
        _ => 2015,
    }
}

fn error_kind(n: i32) -> ErrorKind {
    match n {
        2000 => ErrorKind::Null,
        2007 => ErrorKind::Div0,
        2023 => ErrorKind::Ref,
        2029 => ErrorKind::Name,
        2036 => ErrorKind::Num,
        2042 => ErrorKind::Na,
        _ => ErrorKind::Value,
    }
}

fn full_rows(r: &Range) -> bool {
    r.start.col == 0 && r.end.col == MAX_COLS - 1
}

fn full_cols(r: &Range) -> bool {
    r.start.row == 0 && r.end.row == MAX_ROWS - 1
}

fn rng(r0: i64, c0: i64, r1: i64, c1: i64) -> R<Range> {
    let ok = |r: i64, c: i64| r >= 0 && c >= 0 && r < MAX_ROWS as i64 && c < MAX_COLS as i64;
    if !ok(r0, c0) || !ok(r1, c1) {
        return Err(Error::app("Application-defined or object-defined error"));
    }
    Ok(Range::new(CellRef::new(r0 as u32, c0 as u32), CellRef::new(r1 as u32, c1 as u32)))
}

fn obj(o: Obj) -> Value {
    Value::Object(o)
}

fn range_obj(sheet: usize, range: Range) -> Value {
    obj(Obj::Range(RangeRef::new(sheet, range)))
}

/// A sheet name that Excel accepts.
fn valid_sheet_name(s: &str) -> bool {
    !s.trim().is_empty() && s.chars().count() <= 31 && !s.contains(['[', ']', ':', '*', '?', '/', '\\']) && !s.starts_with('\'') && !s.ends_with('\'')
}

impl Interp<'_> {
    pub fn args(&mut self, args: &[Arg]) -> R<Args> {
        let vals = self.arg_raw(args)?;
        Ok(Args(args.iter().zip(vals).map(|(a, v)| (a.name.clone(), v)).collect()))
    }

    fn arg_i64(&mut self, a: &Args, pos: usize, name: &str) -> R<Option<i64>> {
        match a.get(pos, name) {
            Some(v) => Ok(Some(self.scalar(v)?.to_i64()?)),
            None => Ok(None),
        }
    }

    fn arg_str(&mut self, a: &Args, pos: usize, name: &str) -> R<Option<String>> {
        match a.get(pos, name) {
            Some(v) => Ok(Some(self.scalar(v)?.to_str()?)),
            None => Ok(None),
        }
    }

    /// Applies a change, kept for the macro's undo step.
    pub fn apply(&mut self, op: Op) -> R<()> {
        match self.wb.apply_op(op) {
            Some(inv) => {
                self.undo.push(inv);
                Ok(())
            }
            None => Err(Error::app("The cell or sheet is protected or the workbook is read-only")),
        }
    }

    fn check_sheet(&self, i: usize) -> R<usize> {
        if i < self.wb.sheets.len() { Ok(i) } else { Err(Error::subscript()) }
    }

    // --- references ----------------------------------------------------------------

    /// `A1`, `A1:B2`, `A:C`, `1:3`, `Sheet2!A1`, `'My sheet'!A1` or a
    /// defined name, on `sheet` unless the text names one.
    pub fn parse_ref(&self, text: &str, sheet: usize) -> R<RangeRef> {
        let bad = || Error::app(format!("Method 'Range' failed: {text}"));
        let t = text.trim();
        let (sheet, addr) = match t.rsplit_once('!') {
            Some((s, a)) => {
                let s = s.trim().trim_matches('\'').replace("''", "'");
                (self.wb.sheet_index(&s).ok_or_else(bad)?, a)
            }
            None => (sheet, t),
        };
        let addr = addr.replace('$', "");
        if addr.contains(',') {
            return Err(Error::unsupported("ranges of several areas"));
        }
        let parts: Vec<&str> = addr.split(':').collect();
        let range = if parts.len() == 2 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphabetic())) {
            let (a, b) = (apex_sheet::addr::parse_col(parts[0]).ok_or_else(bad)?, apex_sheet::addr::parse_col(parts[1]).ok_or_else(bad)?);
            Range::new(CellRef::new(0, a), CellRef::new(MAX_ROWS - 1, b))
        } else if parts.len() == 2 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) {
            let (a, b): (u32, u32) = (parts[0].parse().map_err(|_| bad())?, parts[1].parse().map_err(|_| bad())?);
            if a == 0 || b == 0 || a > MAX_ROWS || b > MAX_ROWS {
                return Err(bad());
            }
            Range::new(CellRef::new(a - 1, 0), CellRef::new(b - 1, MAX_COLS - 1))
        } else if let Some(r) = Range::parse(&addr) {
            r
        } else {
            let (s, r) = self.wb.name_target(sheet, t).ok_or_else(bad)?;
            return Ok(RangeRef::new(s, r));
        };
        let by = if parts.len() == 2 && full_rows(&range) {
            By::Rows
        } else if parts.len() == 2 && full_cols(&range) {
            By::Cols
        } else {
            By::Cells
        };
        Ok(RangeRef { sheet, range, by })
    }

    /// A range given as text or as a Range object.
    fn range_arg(&mut self, v: Value, sheet: usize) -> R<RangeRef> {
        match v {
            Value::Object(Obj::Range(r)) => Ok(r),
            Value::Object(_) | Value::Nothing => Err(Error::type_mismatch()),
            v => {
                let t = v.to_str()?;
                self.parse_ref(&t, sheet)
            }
        }
    }

    /// `Range(a)` or `Range(a, b)` on `sheet`.
    fn range_call(&mut self, sheet: usize, a: &Args) -> R<Value> {
        let first = a.get(0, "cell1").ok_or_else(|| Error::new(449, "Argument not optional"))?;
        let r1 = self.range_arg(first, sheet)?;
        match a.get(1, "cell2") {
            Some(second) => {
                let r2 = self.range_arg(second, sheet)?;
                let r = &r1.range;
                let o = &r2.range;
                let range = Range::new(CellRef::new(r.start.row.min(o.start.row), r.start.col.min(o.start.col)), CellRef::new(r.end.row.max(o.end.row), r.end.col.max(o.end.col)));
                Ok(range_obj(r1.sheet, range))
            }
            None => Ok(obj(Obj::Range(r1))),
        }
    }

    /// A column given as a number or letters.
    fn col_arg(&mut self, v: Value) -> R<i64> {
        match self.scalar(v)? {
            Value::Str(s) => apex_sheet::addr::parse_col(s.trim()).map(|c| c as i64 + 1).ok_or_else(Error::invalid_call),
            v => v.to_i64(),
        }
    }

    /// `Cells(r, c)`, `Cells(n)` or `Cells` relative to `base`.
    fn cells_call(&mut self, base: RangeRef, a: &Args) -> R<Value> {
        if a.is_empty() {
            return Ok(obj(Obj::Range(RangeRef { by: By::Cells, ..base })));
        }
        let r = &base.range;
        let row = self.arg_i64(a, 0, "rowindex")?.ok_or_else(Error::invalid_call)?;
        let (dr, dc) = match a.get(1, "columnindex") {
            Some(c) => (row - 1, self.col_arg(c)? - 1),
            None => {
                let w = r.cols() as i64;
                ((row - 1).div_euclid(w), (row - 1).rem_euclid(w))
            }
        };
        let (rr, cc) = (r.start.row as i64 + dr, r.start.col as i64 + dc);
        Ok(range_obj(base.sheet, rng(rr, cc, rr, cc)?))
    }

    /// `Rows(i)`, `Rows("2:4")`, `Columns("B")`, `Columns(2)` of `base`.
    fn lines_call(&mut self, base: RangeRef, a: &Args, rows: bool) -> R<Value> {
        let by = if rows { By::Rows } else { By::Cols };
        let r = base.range;
        let Some(v) = a.get(0, "index") else { return Ok(obj(Obj::Range(RangeRef { by, ..base }))) };
        let v = self.scalar(v)?;
        let (from, to) = match &v {
            Value::Str(s) => {
                let s = s.replace('$', "");
                let (x, y) = s.split_once(':').unwrap_or((&s, &s));
                let one = |t: &str| -> R<i64> {
                    if rows {
                        t.trim().parse::<i64>().map_err(|_| Error::type_mismatch())
                    } else {
                        apex_sheet::addr::parse_col(t.trim()).map(|c| c as i64 + 1).ok_or_else(Error::type_mismatch)
                    }
                };
                (one(x)?, one(y)?)
            }
            v => {
                let i = v.to_i64()?;
                (i, i)
            }
        };
        let range = if rows {
            rng(r.start.row as i64 + from - 1, r.start.col as i64, r.start.row as i64 + to - 1, r.end.col as i64)?
        } else {
            rng(r.start.row as i64, r.start.col as i64 + from - 1, r.end.row as i64, r.start.col as i64 + to - 1)?
        };
        Ok(obj(Obj::Range(RangeRef { sheet: base.sheet, range, by })))
    }

    fn whole_sheet(&self, sheet: usize) -> RangeRef {
        RangeRef::new(sheet, Range::new(CellRef::new(0, 0), CellRef::new(MAX_ROWS - 1, MAX_COLS - 1)))
    }

    // --- globals -----------------------------------------------------------------------

    /// Names of Excel's object model usable without a qualifier.
    pub fn global(&mut self, n: &str, args: &[Arg]) -> Option<R<Value>> {
        let sheet = self.home_sheet();
        let r = match n {
            "application" | "excel" => Ok(obj(Obj::Application)),
            "activeworkbook" | "thisworkbook" => Ok(obj(Obj::Workbook)),
            "worksheets" | "sheets" => {
                if args.is_empty() {
                    Ok(obj(Obj::Sheets))
                } else {
                    return Some(self.member(obj(Obj::Sheets), "item", args));
                }
            }
            "workbooks" => {
                if args.is_empty() {
                    Err(Error::unsupported("the Workbooks collection"))
                } else {
                    Ok(obj(Obj::Workbook))
                }
            }
            "activesheet" => Ok(obj(Obj::Sheet(self.active))),
            "activechart" | "charts" | "activewindow" | "windows" | "commandbars" | "shapes" => Err(Error::unsupported(n)),
            "activecell" => Ok(range_obj(self.active, Range::single(self.selection.start))),
            "selection" => Ok(range_obj(self.active, self.selection)),
            "worksheetfunction" => Ok(obj(Obj::WorksheetFunction)),
            "err" => {
                if args.is_empty() {
                    Ok(obj(Obj::Err))
                } else {
                    return Some(self.member(obj(Obj::Err), "item", args));
                }
            }
            "range" | "cells" | "rows" | "columns" | "usedrange" | "evaluate" | "calculate" | "intersect" | "union" | "inputbox" | "run" => {
                return Some(match n {
                    "intersect" | "union" | "run" | "calculate" => self.member(obj(Obj::Application), n, args),
                    _ => self.member(obj(Obj::Sheet(sheet)), n, args),
                });
            }
            _ if n.starts_with('[') && n.ends_with(']') => {
                let inner = n[1..n.len() - 1].to_string();
                self.evaluate(&inner, sheet)
            }
            _ => return None,
        };
        Some(r)
    }

    /// Evaluate("A1:B2") (a range) or Evaluate("=SUM(A1:A3)") (a value).
    fn evaluate(&mut self, text: &str, sheet: usize) -> R<Value> {
        if let Ok(r) = self.parse_ref(text, sheet) {
            return Ok(obj(Obj::Range(r)));
        }
        let e = apex_sheet::formula::parse_canonical(text).map_err(|e| Error::app(e.to_string()))?;
        let v = apex_sheet::functions::Env::new(&*self.wb, sheet, CellRef::default(), Locale::EN).eval(&e);
        Ok(self.from_cell_value(&v, false))
    }

    // --- members -------------------------------------------------------------------------

    pub fn member(&mut self, target: Value, name: &str, args: &[Arg]) -> R<Value> {
        let o = match target {
            Value::Object(o) => o,
            Value::Nothing => return Err(Error::not_set()),
            _ => return Err(Error::object_required()),
        };
        if matches!(o, Obj::Module(_) | Obj::Sheet(_) | Obj::Workbook)
            && let Some(r) = self.code_member(&o, name, args)
        {
            return r;
        }
        match o {
            Obj::Module(_) => Err(Error::no_member(name)),
            Obj::Range(r) => self.range_member(r, name, args),
            Obj::Sheet(s) => self.sheet_member(s, name, args),
            Obj::Sheets => self.sheets_member(name, args),
            Obj::Workbook => self.workbook_member(name, args),
            Obj::Application => self.app_member(name, args),
            Obj::Font(r) => self.font_get(r, name),
            Obj::Interior(r) => self.interior_get(r, name),
            Obj::WorksheetFunction => {
                let a = self.arg_raw(args)?;
                self.worksheet_function(name, a, true)
            }
            Obj::Err => {
                let a = self.args(args)?;
                match name {
                    "number" | "item" => Ok(Value::Long(self.err.number)),
                    "description" => Ok(Value::str(self.err.description.clone())),
                    "source" => Ok(Value::str(self.err.source.clone())),
                    "clear" => {
                        self.err = Default::default();
                        Ok(Value::Empty)
                    }
                    "raise" => {
                        let n = self.arg_i64(&a, 0, "number")?.ok_or_else(Error::invalid_call)? as i32;
                        let desc = self.arg_str(&a, 2, "description")?.unwrap_or_else(|| format!("Application-defined or object-defined error ({n})"));
                        Err(Error::new(n, desc))
                    }
                    _ => Err(Error::no_member(name)),
                }
            }
            Obj::Collection(c) => {
                let a = self.args(args)?;
                match name {
                    "count" => Ok(Value::Long(c.borrow().items.len() as i32)),
                    "item" | "_default" => {
                        let k = a.get(0, "index").ok_or_else(Error::invalid_call)?;
                        let k = self.scalar(k)?;
                        let b = c.borrow();
                        let i = b.find(&k)?;
                        Ok(b.items[i].1.clone())
                    }
                    "add" => {
                        let item = a.get(0, "item").ok_or_else(Error::invalid_call)?;
                        let key = self.arg_str(&a, 1, "key")?;
                        let before = match a.get(2, "before") {
                            Some(v) => Some(c.borrow().find(&self.scalar(v)?)?),
                            None => None,
                        };
                        let after = match a.get(3, "after") {
                            Some(v) => Some(c.borrow().find(&self.scalar(v)?)?),
                            None => None,
                        };
                        c.borrow_mut().add(item, key, before, after)?;
                        Ok(Value::Empty)
                    }
                    "remove" => {
                        let k = a.get(0, "index").ok_or_else(Error::invalid_call)?;
                        let k = self.scalar(k)?;
                        let i = c.borrow().find(&k)?;
                        c.borrow_mut().items.remove(i);
                        Ok(Value::Empty)
                    }
                    _ => Err(Error::no_member(name)),
                }
            }
            Obj::Dictionary(d) => {
                let a = self.args(args)?;
                let key = |it: &mut Self, pos: usize| -> R<Value> {
                    let k = a.get(pos, "key").ok_or_else(Error::invalid_call)?;
                    it.scalar(k)
                };
                match name {
                    "count" => Ok(Value::Long(d.borrow().keys.len() as i32)),
                    "add" => {
                        let k = key(self, 0)?;
                        let item = a.get(1, "item").unwrap_or_default();
                        d.borrow_mut().add(k, item)?;
                        Ok(Value::Empty)
                    }
                    "exists" => {
                        let k = key(self, 0)?;
                        Ok(Value::Bool(d.borrow().find(&k)?.is_some()))
                    }
                    "item" | "_default" => {
                        let k = key(self, 0)?;
                        let found = d.borrow().find(&k)?;
                        match found {
                            Some(i) => Ok(d.borrow().items[i].clone()),
                            None => {
                                d.borrow_mut().insert(k, Value::Empty)?;
                                Ok(Value::Empty)
                            }
                        }
                    }
                    "keys" => Ok(Value::Array(Rc::new(Array::from_vec(d.borrow().keys.clone(), 0)))),
                    "items" => Ok(Value::Array(Rc::new(Array::from_vec(d.borrow().items.clone(), 0)))),
                    "remove" => {
                        let k = key(self, 0)?;
                        d.borrow_mut().remove(&k)?;
                        Ok(Value::Empty)
                    }
                    "removeall" => {
                        d.borrow_mut().clear();
                        Ok(Value::Empty)
                    }
                    "comparemode" => Ok(Value::Int(d.borrow().text as i16)),
                    _ => Err(Error::no_member(name)),
                }
            }
        }
    }

    pub fn set_member(&mut self, target: Value, name: &str, args: &[Arg], v: Value) -> R<()> {
        let o = match target {
            Value::Object(o) => o,
            Value::Nothing => return Err(Error::not_set()),
            _ => return Err(Error::object_required()),
        };
        if matches!(o, Obj::Module(_) | Obj::Sheet(_) | Obj::Workbook)
            && let Some(r) = self.set_code_member(&o, name, args, v.clone())
        {
            return r;
        }
        let scalar = |it: &mut Self, v: Value| it.scalar(v);
        match (&o, name) {
            (Obj::Range(r), _) if args.is_empty() && self.range_set(*r, name, v.clone())? => Ok(()),
            (Obj::Font(r), _) => {
                let v = scalar(self, v)?;
                self.font_set(*r, name, v)
            }
            (Obj::Interior(r), _) => {
                let v = scalar(self, v)?;
                self.interior_set(*r, name, v)
            }
            (Obj::Sheet(s), "name") => {
                let s = self.check_sheet(*s)?;
                let n = scalar(self, v)?.to_str()?;
                if !valid_sheet_name(&n) || self.wb.sheet_index(&n).is_some_and(|i| i != s) {
                    return Err(Error::app(format!("That sheet name is not valid or is already taken: {n}")));
                }
                self.apply(Op::RenameSheet { index: s, name: n })
            }
            (Obj::Sheet(s), "visible") => {
                let s = self.check_sheet(*s)?;
                let v = scalar(self, v)?;
                let hidden = match v {
                    Value::Bool(b) => !b,
                    v => v.to_i64()? != -1,
                };
                if hidden && self.wb.sheets.iter().enumerate().all(|(i, x)| i == s || x.hidden) {
                    return Err(Error::app("A workbook must have at least one visible sheet"));
                }
                let tab_color = self.wb.sheets[s].tab_color;
                self.apply(Op::SheetProps { index: s, hidden, tab_color })
            }
            (Obj::Workbook, "saved") => {
                self.wb.modified = !scalar(self, v)?.to_bool()?;
                Ok(())
            }
            (Obj::Application, _) => {
                let v = scalar(self, v)?;
                match name {
                    "screenupdating" => self.app.screen_updating = v.to_bool()?,
                    "displayalerts" => self.app.display_alerts = v.to_bool()?,
                    "enableevents" => self.app.enable_events = v.to_bool()?,
                    "calculation" => self.app.calculation = v.to_i64()? as i32,
                    "statusbar" => {
                        self.app.status_bar = match v {
                            Value::Bool(false) => None,
                            v => Some(v.to_str()?),
                        };
                        let s = self.app.status_bar.clone();
                        self.ui.status(s.as_deref());
                    }
                    "cutcopymode" | "displaystatusbar" | "asktoupdatelinks" | "visible" | "interactive" | "displayformulabar" | "cursor" | "enablecancelkey" | "iteration" => {}
                    _ => return Err(Error::no_member(name)),
                }
                Ok(())
            }
            (Obj::Err, _) => {
                let v = scalar(self, v)?;
                match name {
                    "number" => self.err.number = v.to_long()?,
                    "description" => self.err.description = v.to_str()?,
                    "source" => self.err.source = v.to_str()?,
                    _ => return Err(Error::no_member(name)),
                }
                Ok(())
            }
            (Obj::Dictionary(d), "item" | "_default") => {
                let a = self.args(args)?;
                let k = a.get(0, "key").ok_or_else(Error::invalid_call)?;
                let k = self.scalar(k)?;
                d.borrow_mut().insert(k, v)
            }
            (Obj::Dictionary(d), "comparemode") => {
                let text = scalar(self, v)?.to_i64()? == 1;
                let mut d = d.borrow_mut();
                d.text = text;
                d.reindex();
                Ok(())
            }
            _ => {
                // `ws.Cells(1, 1) = 5`, `r.Offset(1, 0) = 2`: the default member of the result.
                let t = self.member(Value::Object(o), name, args)?;
                if !t.is_object() {
                    return Err(Error::no_member(name));
                }
                self.set_default(t, v)
            }
        }
    }

    /// The default property of an object (a Range's Value, Err's Number).
    pub fn default_value(&mut self, o: Obj) -> R<Value> {
        match o {
            Obj::Range(r) => Ok(self.range_value(r, false)),
            Obj::Err => Ok(Value::Long(self.err.number)),
            Obj::Application => Ok(Value::str("ApexDesk")),
            Obj::Sheet(_) | Obj::Workbook | Obj::Font(_) | Obj::Interior(_) | Obj::Sheets | Obj::WorksheetFunction | Obj::Collection(_) | Obj::Dictionary(_) | Obj::Module(_) => Err(Error::no_member(&format!("default property of {}", o.type_name()))),
        }
    }

    pub fn set_default(&mut self, target: Value, v: Value) -> R<()> {
        match target {
            Value::Object(Obj::Range(r)) => {
                let v = match v {
                    v @ Value::Object(_) => self.scalar(v)?,
                    v => v,
                };
                self.set_range_value(r, v)
            }
            Value::Object(o) => Err(Error::no_member(&format!("default property of {}", o.type_name()))),
            Value::Nothing => Err(Error::not_set()),
            _ => Err(Error::object_required()),
        }
    }

    // --- cells ---------------------------------------------------------------------

    fn from_cell_value(&self, v: &apex_sheet::Value, date: bool) -> Value {
        match v {
            apex_sheet::Value::Empty => Value::Empty,
            apex_sheet::Value::Number(n) if date => Value::Date(*n),
            apex_sheet::Value::Number(n) => Value::Double(*n),
            apex_sheet::Value::Text(t) => Value::str(t.as_ref()),
            apex_sheet::Value::Bool(b) => Value::Bool(*b),
            apex_sheet::Value::Error(k) => Value::Error(error_code(*k)),
            apex_sheet::Value::Array(m) => {
                let mut data = Vec::with_capacity(m.data.len());
                for x in &m.data {
                    data.push(self.from_cell_value(x, false));
                }
                Value::Array(Rc::new(Array { bounds: vec![(1, m.rows as i64), (1, m.cols as i64)], data, ty: VType::Variant }))
            }
            apex_sheet::Value::Lambda(_) => Value::Error(2015),
        }
    }

    /// A cell's value for VBA (a date format gives a Date; Value2 doesn't).
    fn cell_value(&self, sheet: usize, c: CellRef, value2: bool) -> Value {
        let Some(cell) = self.wb.cell(sheet, c) else { return Value::Empty };
        let date = !value2 && self.wb.styles.format(cell.style).is_date();
        self.from_cell_value(cell.value(), date)
    }

    /// Values of a range: one value, or a 2-D array from 1.
    fn range_value(&self, r: RangeRef, value2: bool) -> Value {
        let rg = r.range;
        if r.sheet >= self.wb.sheets.len() {
            return Value::Empty;
        }
        if rg.is_single() {
            return self.cell_value(r.sheet, rg.start, value2);
        }
        let rg = if rg.cell_count() > 4_000_000 { self.wb.clamp_to_used(r.sheet, &rg).unwrap_or(Range::single(rg.start)) } else { rg };
        // Cells far apart still make a huge used area: not read whole.
        if rg.cell_count() > 4_000_000 {
            return Value::Empty;
        }
        let mut data = Vec::with_capacity(rg.cell_count() as usize);
        for row in rg.start.row..=rg.end.row {
            for col in rg.start.col..=rg.end.col {
                data.push(self.cell_value(r.sheet, CellRef::new(row, col), value2));
            }
        }
        Value::Array(Rc::new(Array { bounds: vec![(1, rg.rows() as i64), (1, rg.cols() as i64)], data, ty: VType::Variant }))
    }

    /// What a VBA value becomes in a cell, with a number format for dates
    /// and typed-in text ("10%").
    fn content_of(&self, v: &Value) -> R<(Content, Option<String>)> {
        Ok(match v {
            Value::Empty | Value::Missing | Value::Null | Value::Nothing => (Content::Empty, None),
            Value::Bool(b) => (Content::Value(apex_sheet::Value::Bool(*b)), None),
            Value::Int(_) | Value::Long(_) | Value::Double(_) => (Content::Value(apex_sheet::Value::Number(v.to_f64()?)), None),
            Value::Date(d) => {
                let date = self.wb.locale.short_date_code();
                let code = if d.fract() == 0.0 {
                    date.to_string()
                } else if d.trunc() == 0.0 {
                    "hh:mm:ss".into()
                } else {
                    format!("{date} hh:mm")
                };
                (Content::Value(apex_sheet::Value::Number(*d)), Some(code))
            }
            Value::Str(s) if s.len() > 1 && s.starts_with('=') => {
                let e = apex_sheet::formula::parse_canonical(s).map_err(|e| Error::app(format!("Invalid formula {s}: {e}")))?;
                (Content::Formula { expr: e.into(), value: apex_sheet::Value::Empty }, None)
            }
            Value::Str(s) => {
                let p = parse_input(s, &Locale::EN);
                match p.value {
                    apex_sheet::Value::Empty => (Content::Empty, None),
                    x => (Content::Value(x), p.format),
                }
            }
            Value::Error(n) => (Content::Value(apex_sheet::Value::Error(error_kind(*n))), None),
            Value::Array(_) | Value::Object(_) => return Err(Error::type_mismatch()),
        })
    }

    /// The cell `c` would become with `content` (and `fmt` when its format is General).
    fn cell_with(&mut self, sheet: usize, c: CellRef, content: Content, fmt: Option<&str>) -> Option<Cell> {
        let old = self.wb.cell(sheet, c).cloned();
        let mut cell = Cell::with_content(old.as_ref(), content);
        if let (Some(f), Some(x)) = (fmt, cell.as_mut()) {
            if self.wb.styles.get(x.style).num_fmt.as_ref() == "General" {
                let mut s = self.wb.styles.get(x.style).clone();
                s.num_fmt = Arc::from(f);
                x.style = self.wb.styles.intern(s);
            }
        }
        cell
    }

    fn check_size(r: &Range) -> R<()> {
        if r.cell_count() > 2_000_000 {
            return Err(Error::app("Too many cells for a macro to change at once"));
        }
        Ok(())
    }

    pub fn set_range_value(&mut self, r: RangeRef, v: Value) -> R<()> {
        self.check_sheet(r.sheet)?;
        let rg = r.range;
        Self::check_size(&rg)?;
        let mut cells = Vec::new();
        match &v {
            Value::Array(a) => {
                let (rows, cols, two) = match a.bounds.len() {
                    1 => (1, a.bounds[0].1 - a.bounds[0].0 + 1, false),
                    2 => (a.bounds[0].1 - a.bounds[0].0 + 1, a.bounds[1].1 - a.bounds[1].0 + 1, true),
                    _ => return Err(Error::type_mismatch()),
                };
                for (i, row) in (rg.start.row..=rg.end.row).enumerate() {
                    for (j, col) in (rg.start.col..=rg.end.col).enumerate() {
                        let (i, j) = (i as i64, j as i64);
                        let x = if two {
                            if i < rows && j < cols { a.data[(i * cols + j) as usize].clone() } else { Value::Error(2042) }
                        } else if j < cols {
                            a.data[j as usize].clone()
                        } else {
                            Value::Error(2042)
                        };
                        let (content, fmt) = self.content_of(&x)?;
                        let c = CellRef::new(row, col);
                        let cell = self.cell_with(r.sheet, c, content, fmt.as_deref());
                        cells.push((c, cell));
                    }
                }
            }
            v => {
                let (content, fmt) = self.content_of(v)?;
                // A formula keeps its relative references per cell.
                let formula = match &content {
                    Content::Formula { expr, .. } => Some(expr.clone()),
                    _ => None,
                };
                let bounded = if rg.cell_count() > 100_000 && matches!(content, Content::Empty) { self.wb.clamp_to_used(r.sheet, &rg) } else { Some(rg) };
                let Some(bounded) = bounded else { return Ok(()) };
                for row in bounded.start.row..=bounded.end.row {
                    for col in bounded.start.col..=bounded.end.col {
                        let c = CellRef::new(row, col);
                        let content = match &formula {
                            Some(e) if c != rg.start => Content::Formula { expr: e.offset((row - rg.start.row) as i64, (col - rg.start.col) as i64), value: apex_sheet::Value::Empty },
                            _ => content.clone(),
                        };
                        let cell = self.cell_with(r.sheet, c, content, fmt.as_deref());
                        cells.push((c, cell));
                    }
                }
            }
        }
        self.apply(Op::SetCells { sheet: r.sheet, cells })
    }

    fn formula_text(&self, sheet: usize, c: CellRef, kind: &str) -> Value {
        let Some(cell) = self.wb.cell(sheet, c) else { return Value::str("") };
        match cell.formula().map(|f| f.expr()) {
            Some(e) => Value::str(match kind {
                "formulalocal" => e.to_display(&self.wb.locale),
                "formular1c1" => e.to_display_r1c1(&Locale::EN, c),
                "formular1c1local" => e.to_display_r1c1(&self.wb.locale, c),
                _ => format!("={}", e.to_canonical()),
            }),
            None => {
                let v = self.cell_value(sheet, c, false);
                Value::str(match v {
                    Value::Double(n) => number_text(n),
                    v => v.to_str().unwrap_or_default(),
                })
            }
        }
    }

    fn set_formula(&mut self, r: RangeRef, kind: &str, v: Value) -> R<()> {
        let rg = r.range;
        Self::check_size(&rg)?;
        let text = match &v {
            Value::Array(_) => return self.set_range_value(r, v),
            v => v.to_str()?,
        };
        if !(text.len() > 1 && text.starts_with('=')) {
            return self.set_range_value(r, Value::str(text));
        }
        let loc = if kind.ends_with("local") { self.wb.locale } else { Locale::EN };
        let mut cells = Vec::new();
        for row in rg.start.row..=rg.end.row {
            for col in rg.start.col..=rg.end.col {
                let c = CellRef::new(row, col);
                let src = if kind.contains("r1c1") { apex_sheet::formula::r1c1_to_a1(&text, c).ok_or_else(|| Error::app(format!("Invalid formula: {text}")))? } else { text.clone() };
                let mut e = apex_sheet::formula::parse(&src, &loc).map_err(|e| Error::app(format!("Invalid formula {text}: {e}")))?;
                if !kind.contains("r1c1") && c != rg.start {
                    e.offset((row - rg.start.row) as i64, (col - rg.start.col) as i64);
                }
                let cell = self.cell_with(r.sheet, c, Content::Formula { expr: e.into(), value: apex_sheet::Value::Empty }, None);
                cells.push((c, cell));
            }
        }
        self.apply(Op::SetCells { sheet: r.sheet, cells })
    }

    /// Changes the style of every cell of `r` (whole rows/columns: of the used part).
    fn restyle(&mut self, r: RangeRef, f: impl Fn(&mut CellStyle)) -> R<()> {
        self.check_sheet(r.sheet)?;
        let rg = if r.range.cell_count() > 1_000_000 {
            match self.wb.clamp_to_used(r.sheet, &r.range) {
                Some(x) => x,
                None => return Ok(()),
            }
        } else {
            r.range
        };
        Self::check_size(&rg)?;
        let mut cache: std::collections::HashMap<u32, u32> = Default::default();
        let mut cells = Vec::new();
        for row in rg.start.row..=rg.end.row {
            for col in rg.start.col..=rg.end.col {
                let c = CellRef::new(row, col);
                let old = self.wb.cell(r.sheet, c).cloned();
                let os = old.as_ref().map_or(0, |x| x.style);
                let ns = match cache.get(&os) {
                    Some(&s) => s,
                    None => {
                        let mut s = self.wb.styles.get(os).clone();
                        f(&mut s);
                        let id = self.wb.styles.intern(s);
                        cache.insert(os, id);
                        id
                    }
                };
                if ns != os {
                    cells.push((c, Cell::with_style(old.as_ref(), ns)));
                }
            }
        }
        if cells.is_empty() {
            return Ok(());
        }
        self.apply(Op::SetCells { sheet: r.sheet, cells })
    }

    fn style_of(&self, r: RangeRef) -> CellStyle {
        let id = self.wb.cell(r.sheet, r.range.start).map_or(0, |c| c.style);
        self.wb.styles.get(id).clone()
    }

    // --- Range ---------------------------------------------------------------------

    fn address(r: &Range, row_abs: bool, col_abs: bool) -> String {
        let (rd, cd) = (if row_abs { "$" } else { "" }, if col_abs { "$" } else { "" });
        if full_cols(r) {
            let (a, b) = (apex_sheet::addr::col_name(r.start.col), apex_sheet::addr::col_name(r.end.col));
            return format!("{cd}{a}:{cd}{b}");
        }
        if full_rows(r) {
            return format!("{rd}{}:{rd}{}", r.start.row + 1, r.end.row + 1);
        }
        let one = |c: CellRef| format!("{cd}{}{rd}{}", apex_sheet::addr::col_name(c.col), c.row + 1);
        if r.is_single() { one(r.start) } else { format!("{}:{}", one(r.start), one(r.end)) }
    }

    fn range_member(&mut self, r: RangeRef, name: &str, args: &[Arg]) -> R<Value> {
        self.check_sheet(r.sheet)?;
        let rg = r.range;
        let a = self.args(args)?;
        Ok(match name {
            "value" | "_default" if a.is_empty() => self.range_value(r, false),
            "value2" => self.range_value(r, true),
            "formula" | "formulalocal" | "formular1c1" | "formular1c1local" => {
                if rg.is_single() {
                    self.formula_text(r.sheet, rg.start, name)
                } else {
                    Self::check_size(&rg)?;
                    let mut data = Vec::new();
                    for row in rg.start.row..=rg.end.row {
                        for col in rg.start.col..=rg.end.col {
                            data.push(self.formula_text(r.sheet, CellRef::new(row, col), name));
                        }
                    }
                    Value::Array(Rc::new(Array { bounds: vec![(1, rg.rows() as i64), (1, rg.cols() as i64)], data, ty: VType::Variant }))
                }
            }
            "text" => Value::str(self.wb.display(r.sheet, rg.start).0),
            "numberformat" | "numberformatlocal" => Value::str(self.style_of(r).num_fmt.to_string()),
            "address" | "addresslocal" => {
                let row_abs = match a.get(0, "rowabsolute") {
                    Some(v) => self.scalar(v)?.to_bool()?,
                    None => true,
                };
                let col_abs = match a.get(1, "columnabsolute") {
                    Some(v) => self.scalar(v)?.to_bool()?,
                    None => true,
                };
                let ext = match a.get(3, "external") {
                    Some(v) => self.scalar(v)?.to_bool()?,
                    None => false,
                };
                let addr = Self::address(&rg, row_abs, col_abs);
                Value::str(if ext { format!("'{}'!{addr}", self.wb.sheets[r.sheet].name) } else { addr })
            }
            "row" => Value::Long(rg.start.row as i32 + 1),
            "column" => Value::Long(rg.start.col as i32 + 1),
            "count" | "countlarge" => {
                let n = match r.by {
                    By::Cells => rg.cell_count(),
                    By::Rows => rg.rows() as u64,
                    By::Cols => rg.cols() as u64,
                };
                if n > i32::MAX as u64 { Value::Double(n as f64) } else { Value::Long(n as i32) }
            }
            "rows" => return self.lines_call(r, &a, true),
            "columns" => return self.lines_call(r, &a, false),
            "cells" => return self.cells_call(r, &a),
            "item" | "_default" => match r.by {
                By::Rows if a.len() == 1 => return self.lines_call(r, &a, true),
                By::Cols if a.len() == 1 => return self.lines_call(r, &a, false),
                _ => return self.cells_call(r, &a),
            },
            "range" => {
                // Relative to this range's top-left corner.
                let inner = self.range_call(r.sheet, &a)?;
                let Value::Object(Obj::Range(x)) = inner else { return Ok(inner) };
                let range = rng(rg.start.row as i64 + x.range.start.row as i64, rg.start.col as i64 + x.range.start.col as i64, rg.start.row as i64 + x.range.end.row as i64, rg.start.col as i64 + x.range.end.col as i64)?;
                range_obj(r.sheet, range)
            }
            "offset" => {
                let dr = self.arg_i64(&a, 0, "rowoffset")?.unwrap_or(0);
                let dc = self.arg_i64(&a, 1, "columnoffset")?.unwrap_or(0);
                let range = rng(rg.start.row as i64 + dr, rg.start.col as i64 + dc, rg.end.row as i64 + dr, rg.end.col as i64 + dc)?;
                obj(Obj::Range(RangeRef { range, ..r }))
            }
            "resize" => {
                let rows = self.arg_i64(&a, 0, "rowsize")?.unwrap_or(rg.rows() as i64);
                let cols = self.arg_i64(&a, 1, "columnsize")?.unwrap_or(rg.cols() as i64);
                if rows < 1 || cols < 1 {
                    return Err(Error::app("Application-defined or object-defined error"));
                }
                let range = rng(rg.start.row as i64, rg.start.col as i64, rg.start.row as i64 + rows - 1, rg.start.col as i64 + cols - 1)?;
                obj(Obj::Range(RangeRef { range, ..r }))
            }
            "end" => {
                let dir = self.arg_i64(&a, 0, "direction")?.ok_or_else(Error::invalid_call)?;
                let (dr, dc) = match dir {
                    -4162 => (-1, 0),
                    -4121 => (1, 0),
                    -4159 => (0, -1),
                    -4161 => (0, 1),
                    _ => return Err(Error::invalid_call()),
                };
                let to = self.wb.sheets[r.sheet].jump(rg.start, dr, dc, CellRef::new(MAX_ROWS - 1, MAX_COLS - 1));
                range_obj(r.sheet, Range::single(to))
            }
            "entirerow" => obj(Obj::Range(RangeRef { sheet: r.sheet, range: Range::new(CellRef::new(rg.start.row, 0), CellRef::new(rg.end.row, MAX_COLS - 1)), by: By::Rows })),
            "entirecolumn" => obj(Obj::Range(RangeRef { sheet: r.sheet, range: Range::new(CellRef::new(0, rg.start.col), CellRef::new(MAX_ROWS - 1, rg.end.col)), by: By::Cols })),
            "worksheet" | "parent" => obj(Obj::Sheet(r.sheet)),
            "application" => obj(Obj::Application),
            "font" => obj(Obj::Font(r)),
            "interior" => obj(Obj::Interior(r)),
            "horizontalalignment" => Value::Long(match self.style_of(r).halign {
                HAlign::General => 1,
                HAlign::Left => -4131,
                HAlign::Center => -4108,
                HAlign::Right => -4152,
                HAlign::Justify => -4130,
            }),
            "verticalalignment" => Value::Long(match self.style_of(r).valign {
                VAlign::Top => -4160,
                VAlign::Center => -4108,
                VAlign::Bottom => -4107,
            }),
            "wraptext" => Value::Bool(self.style_of(r).wrap),
            "hasformula" => {
                let (mut some, mut all) = (false, true);
                for row in rg.start.row..=rg.end.row.min(rg.start.row + 10_000) {
                    for col in rg.start.col..=rg.end.col.min(rg.start.col + 1000) {
                        let f = self.wb.cell(r.sheet, CellRef::new(row, col)).is_some_and(|c| c.formula().is_some());
                        some |= f;
                        all &= f;
                    }
                }
                if all { Value::Bool(true) } else if some { Value::Null } else { Value::Bool(false) }
            }
            "columnwidth" => {
                let px = self.wb.sheets[r.sheet].col_width(rg.start.col);
                Value::Double((((px / COL_SCALE - 5.0) / 7.0 * 100.0).round() / 100.0).max(0.0) as f64)
            }
            "rowheight" => Value::Double(((self.wb.sheets[r.sheet].row_height(rg.start.row) / ROW_SCALE * 4.0).round() / 4.0) as f64),
            "hidden" => {
                let sh = &self.wb.sheets[r.sheet];
                Value::Bool(if r.by == By::Cols || full_cols(&rg) { sh.col_width(rg.start.col) == 0.0 } else { sh.row_height(rg.start.row) == 0.0 })
            }
            "mergecells" => Value::Bool(self.wb.sheets[r.sheet].merges.iter().any(|m| m.intersect(&rg).is_some())),
            "currentregion" => range_obj(r.sheet, self.current_region(r.sheet, rg)),
            "find" => {
                let what = a.get(0, "what").ok_or_else(|| Error::new(449, "Argument not optional"))?;
                let what = self.scalar(what)?.to_str()?;
                let whole = self.arg_i64(&a, 3, "lookat")?.is_some_and(|x| x == 1);
                let case = match a.get(6, "matchcase") {
                    Some(v) => self.scalar(v)?.to_bool()?,
                    None => false,
                };
                match self.find(r.sheet, rg, &what, whole, case) {
                    Some(c) => range_obj(r.sheet, Range::single(c)),
                    None => Value::Nothing,
                }
            }
            "select" | "activate" => {
                self.active = r.sheet;
                self.selection = if name == "select" || !self.selection.contains(rg.start) { rg } else { self.selection };
                if name == "activate" && rg.is_single() {
                    self.selection = rg;
                }
                Value::Empty
            }
            "clearcontents" => {
                let mut cells = Vec::new();
                self.wb.sheets[r.sheet].for_each_in(&rg, |c, cell| {
                    if !matches!(cell.content, Content::Empty) {
                        cells.push((c, Cell::with_content(Some(cell), Content::Empty)));
                    }
                });
                if !cells.is_empty() {
                    self.apply(Op::SetCells { sheet: r.sheet, cells })?;
                }
                Value::Empty
            }
            "clear" | "clearformats" => {
                let mut cells = Vec::new();
                self.wb.sheets[r.sheet].for_each_in(&rg, |c, cell| {
                    let new = if name == "clear" { None } else { Cell::with_style(Some(cell), 0) };
                    cells.push((c, new));
                });
                if !cells.is_empty() {
                    self.apply(Op::SetCells { sheet: r.sheet, cells })?;
                }
                Value::Empty
            }
            "delete" | "insert" => {
                let insert = name == "insert";
                let shift = self.arg_i64(&a, 0, "shift")?;
                let rows = full_rows(&rg) || r.by == By::Rows && !full_cols(&rg) || shift.is_some_and(|s| s == -4162 || s == -4121);
                let cols = full_cols(&rg) || r.by == By::Cols;
                if rows && full_rows(&rg) {
                    let (at, count) = (rg.start.row, rg.rows());
                    self.apply(if insert { Op::InsertRows { sheet: r.sheet, at, count } } else { Op::DeleteRows { sheet: r.sheet, at, count } })?;
                } else if cols && full_cols(&rg) {
                    let (at, count) = (rg.start.col, rg.cols());
                    self.apply(if insert { Op::InsertCols { sheet: r.sheet, at, count } } else { Op::DeleteCols { sheet: r.sheet, at, count } })?;
                } else {
                    return Err(Error::unsupported("inserting or deleting part of a row or column (use EntireRow or EntireColumn)"));
                }
                Value::Empty
            }
            "copy" => {
                let Some(dest) = a.get(0, "destination") else { return Err(Error::unsupported("Copy without a Destination")) };
                let dest = self.range_arg(dest, r.sheet)?;
                self.copy_cells(r, dest.sheet, dest.range.start)?;
                Value::Empty
            }
            "merge" | "unmerge" => {
                let mut merges: Vec<Range> = self.wb.sheets[r.sheet].merges.iter().filter(|m| m.intersect(&rg).is_none()).copied().collect();
                if name == "merge" && !rg.is_single() {
                    merges.push(rg);
                }
                self.apply(Op::Merges { sheet: r.sheet, merges })?;
                Value::Empty
            }
            "autofit" | "calculate" | "show" => Value::Empty,
            "sort" | "specialcells" | "pastespecial" | "autofilter" | "removeduplicates" | "texttocolumns" | "copypicture" | "printout" | "areas" | "cut" | "borders" | "characters" | "addcomment" | "comment" | "hyperlinks" | "validation" | "formatconditions" | "listobject" | "pivottable" => return Err(Error::unsupported(&format!("Range.{name}"))),
            _ => return Err(Error::no_member(name)),
        })
    }

    /// Sets a property of a range; false when it is not one it knows.
    fn range_set(&mut self, r: RangeRef, name: &str, v: Value) -> R<bool> {
        self.check_sheet(r.sheet)?;
        let v = match v {
            v @ Value::Object(_) => self.scalar(v)?,
            v => v,
        };
        match name {
            "value" | "value2" | "_default" => self.set_range_value(r, v)?,
            "formula" | "formulalocal" | "formular1c1" | "formular1c1local" => self.set_formula(r, name, v)?,
            "numberformat" | "numberformatlocal" => {
                let code: Arc<str> = Arc::from(v.to_str()?);
                self.restyle(r, |s| s.num_fmt = code.clone())?;
            }
            "horizontalalignment" => {
                let h = match v.to_i64()? {
                    -4131 => HAlign::Left,
                    -4108 | 7 => HAlign::Center,
                    -4152 => HAlign::Right,
                    -4130 => HAlign::Justify,
                    _ => HAlign::General,
                };
                self.restyle(r, |s| s.halign = h)?;
            }
            "verticalalignment" => {
                let va = match v.to_i64()? {
                    -4160 => VAlign::Top,
                    -4108 => VAlign::Center,
                    _ => VAlign::Bottom,
                };
                self.restyle(r, |s| s.valign = va)?;
            }
            "wraptext" => {
                let w = v.to_bool()?;
                self.restyle(r, |s| s.wrap = w)?;
            }
            "columnwidth" => {
                let w = v.to_f64()?;
                if !(0.0..=255.0).contains(&w) {
                    return Err(Error::app("Unable to set the ColumnWidth property"));
                }
                let px = ((w as f32 * 7.0 + 5.0).round() * COL_SCALE).max(0.0);
                let sizes = (r.range.start.col..=r.range.end.col.min(r.range.start.col + MAX_COLS)).map(|c| (c, Some(if w == 0.0 { 0.0 } else { px }))).collect();
                self.apply(Op::ColWidths { sheet: r.sheet, sizes })?;
            }
            "rowheight" => {
                let h = v.to_f64()?;
                if !(0.0..=409.0).contains(&h) {
                    return Err(Error::app("Unable to set the RowHeight property"));
                }
                let px = (h as f32 * ROW_SCALE).round();
                let rg = r.range;
                Self::check_size(&Range::new(rg.start, CellRef::new(rg.end.row, rg.start.col)))?;
                let sizes = (rg.start.row..=rg.end.row).map(|row| (row, Some(px))).collect();
                self.apply(Op::RowHeights { sheet: r.sheet, sizes })?;
            }
            "hidden" => {
                let hide = v.to_bool()?;
                let rg = r.range;
                if r.by == By::Cols || full_cols(&rg) && !full_rows(&rg) {
                    let sizes = (rg.start.col..=rg.end.col).map(|c| (c, if hide { Some(0.0) } else { None })).collect();
                    self.apply(Op::ColWidths { sheet: r.sheet, sizes })?;
                } else {
                    Self::check_size(&Range::new(rg.start, CellRef::new(rg.end.row, rg.start.col)))?;
                    let sizes = (rg.start.row..=rg.end.row).map(|row| (row, if hide { Some(0.0) } else { None })).collect();
                    self.apply(Op::RowHeights { sheet: r.sheet, sizes })?;
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn filled(&self, sheet: usize, c: CellRef) -> bool {
        self.wb.cell(sheet, c).is_some_and(|x| !x.value().is_empty())
    }

    /// The block of filled cells around `rg` (Ctrl+* in Excel).
    fn current_region(&self, sheet: usize, rg: Range) -> Range {
        let mut r = rg;
        loop {
            let (r0, c0) = (r.start.row.saturating_sub(1), r.start.col.saturating_sub(1));
            let (r1, c1) = ((r.end.row + 1).min(MAX_ROWS - 1), (r.end.col + 1).min(MAX_COLS - 1));
            let mut grown = r;
            let any = |c: CellRef, grown: &mut Range| {
                if self.filled(sheet, c) {
                    *grown = Range::new(CellRef::new(grown.start.row.min(c.row), grown.start.col.min(c.col)), CellRef::new(grown.end.row.max(c.row), grown.end.col.max(c.col)));
                }
            };
            for col in c0..=c1 {
                any(CellRef::new(r0, col), &mut grown);
                any(CellRef::new(r1, col), &mut grown);
            }
            for row in r0..=r1 {
                any(CellRef::new(row, c0), &mut grown);
                any(CellRef::new(row, c1), &mut grown);
            }
            if grown == r {
                break;
            }
            r = grown;
        }
        // A lone empty cell stays itself.
        r
    }

    fn find(&self, sheet: usize, rg: Range, what: &str, whole: bool, case: bool) -> Option<CellRef> {
        let norm = |s: &str| if case { s.to_string() } else { s.to_lowercase() };
        let what = norm(what);
        let mut hits = Vec::new();
        self.wb.sheets[sheet].for_each_in(&rg, |c, cell| {
            if cell.value().is_empty() {
                return;
            }
            let text = norm(&self.wb.display(sheet, c).0);
            if if whole { text == what } else { text.contains(&what) } {
                hits.push(c);
            }
        });
        hits.into_iter().min_by_key(|c| (c.row, c.col))
    }

    /// Copies the cells of `src` to `to` (formulas move their references).
    fn copy_cells(&mut self, src: RangeRef, sheet: usize, to: CellRef) -> R<()> {
        let rg = src.range;
        Self::check_size(&rg)?;
        let (dr, dc) = (to.row as i64 - rg.start.row as i64, to.col as i64 - rg.start.col as i64);
        let dest = rng(to.row as i64, to.col as i64, rg.end.row as i64 + dr, rg.end.col as i64 + dc)?;
        let mut cells = Vec::new();
        for row in rg.start.row..=rg.end.row {
            for col in rg.start.col..=rg.end.col {
                let mut cell = self.wb.cell(src.sheet, CellRef::new(row, col)).cloned();
                if let Some(Cell { content: Content::Formula { expr, .. }, .. }) = &mut cell {
                    *expr = expr.offset(dr, dc);
                }
                if let Some(Cell { content: c @ Content::Spill(_), .. }) = &mut cell {
                    if let Content::Spill(v) = c.clone() {
                        *c = Content::Value(v);
                    }
                }
                cells.push((CellRef::new((row as i64 + dr) as u32, (col as i64 + dc) as u32), cell));
            }
        }
        debug_assert!(cells.iter().all(|(c, _)| dest.contains(*c)));
        self.apply(Op::SetCells { sheet, cells })
    }

    // --- Font and Interior ------------------------------------------------------------

    fn font_get(&mut self, r: RangeRef, name: &str) -> R<Value> {
        let s = self.style_of(r);
        Ok(match name {
            "bold" => Value::Bool(s.bold),
            "italic" => Value::Bool(s.italic),
            "underline" => Value::Long(if s.underline { 2 } else { -4142 }),
            "strikethrough" => Value::Bool(s.strike),
            "size" => Value::Double(if s.font_size == 0 { 11.0 } else { s.font_size as f64 / 10.0 }),
            "name" => Value::str(s.font.as_deref().unwrap_or("Calibri")),
            "color" => Value::Long(swap_rb(s.color.unwrap_or(0)) as i32),
            "colorindex" => Value::Long(match s.color {
                None => -4105,
                Some(c) => PALETTE.iter().position(|&p| p == c).map_or(-4105, |i| i as i32 + 1),
            }),
            _ => return Err(Error::no_member(name)),
        })
    }

    fn font_set(&mut self, r: RangeRef, name: &str, v: Value) -> R<()> {
        match name {
            "bold" => {
                let b = v.to_bool()?;
                self.restyle(r, |s| s.bold = b)
            }
            "italic" => {
                let b = v.to_bool()?;
                self.restyle(r, |s| s.italic = b)
            }
            "underline" => {
                let b = match v {
                    Value::Bool(b) => b,
                    v => !matches!(v.to_i64()?, -4142 | 0),
                };
                self.restyle(r, |s| s.underline = b)
            }
            "strikethrough" => {
                let b = v.to_bool()?;
                self.restyle(r, |s| s.strike = b)
            }
            "size" => {
                let pt = v.to_f64()?;
                if !(1.0..=409.0).contains(&pt) {
                    return Err(Error::app("Unable to set the Size property of the Font class"));
                }
                self.restyle(r, |s| s.font_size = (pt * 10.0).round() as u16)
            }
            "name" => {
                let f: Arc<str> = Arc::from(v.to_str()?);
                self.restyle(r, |s| {
                    s.font = Some(f.clone());
                    s.theme.font = 0;
                })
            }
            "color" => {
                let c = swap_rb(v.to_i64()? as u32 & 0xFF_FFFF);
                self.restyle(r, |s| {
                    s.color = Some(c);
                    s.theme.color = None;
                })
            }
            "colorindex" => {
                let i = v.to_i64()?;
                let c = match i {
                    -4105 | -4142 | 0 => None,
                    1..=56 => Some(PALETTE[i as usize - 1]),
                    _ => return Err(Error::app("Unable to set the ColorIndex property of the Font class")),
                };
                self.restyle(r, |s| {
                    s.color = c;
                    s.theme.color = None;
                })
            }
            _ => Err(Error::no_member(name)),
        }
    }

    fn interior_get(&mut self, r: RangeRef, name: &str) -> R<Value> {
        let s = self.style_of(r);
        Ok(match name {
            "color" => Value::Long(swap_rb(s.fill.unwrap_or(0xFFFFFF)) as i32),
            "colorindex" => Value::Long(match s.fill {
                None => -4142,
                Some(c) => PALETTE.iter().position(|&p| p == c).map_or(-4142, |i| i as i32 + 1),
            }),
            "pattern" => Value::Long(if s.fill.is_some() { 1 } else { -4142 }),
            _ => return Err(Error::no_member(name)),
        })
    }

    fn interior_set(&mut self, r: RangeRef, name: &str, v: Value) -> R<()> {
        let fill = match name {
            "color" => Some(swap_rb(v.to_i64()? as u32 & 0xFF_FFFF)),
            "colorindex" => match v.to_i64()? {
                -4142 | -4105 | 0 => None,
                i @ 1..=56 => Some(PALETTE[i as usize - 1]),
                _ => return Err(Error::app("Unable to set the ColorIndex property of the Interior class")),
            },
            "pattern" => {
                if v.to_i64()? == -4142 {
                    None
                } else {
                    return Ok(());
                }
            }
            "patterncolor" | "patterncolorindex" | "tintandshade" | "themecolor" => return Ok(()),
            _ => return Err(Error::no_member(name)),
        };
        self.restyle(r, |s| {
            s.fill = fill;
            s.theme.fill = None;
        })
    }

    // --- Worksheet, Sheets, Workbook, Application -----------------------------------

    fn sheet_member(&mut self, s: usize, name: &str, args: &[Arg]) -> R<Value> {
        let s = self.check_sheet(s)?;
        let a = self.args(args)?;
        let whole = self.whole_sheet(s);
        Ok(match name {
            "name" => Value::str(self.wb.sheets[s].name.clone()),
            "codename" => Value::str(self.wb.sheets[s].code_name.clone().unwrap_or_else(|| format!("Sheet{}", s + 1))),
            "index" => Value::Long(s as i32 + 1),
            "range" => return self.range_call(s, &a),
            "cells" => return self.cells_call(whole, &a),
            "rows" => return self.lines_call(whole, &a, true),
            "columns" => return self.lines_call(whole, &a, false),
            "usedrange" => {
                let sh = &self.wb.sheets[s];
                let range = match sh.extent() {
                    Some(end) => {
                        let (mut r0, mut c0) = (end.row, end.col);
                        for (c, _) in sh.iter() {
                            r0 = r0.min(c.row);
                            c0 = c0.min(c.col);
                        }
                        Range::new(CellRef::new(r0, c0), end)
                    }
                    None => Range::single(CellRef::new(0, 0)),
                };
                range_obj(s, range)
            }
            "activate" | "select" => {
                if self.active != s {
                    self.active = s;
                    self.selection = Range::single(CellRef::new(0, 0));
                }
                Value::Empty
            }
            "delete" => {
                if self.wb.sheets.len() <= 1 {
                    return Err(Error::app("A workbook must contain at least one visible worksheet"));
                }
                self.apply(Op::RemoveSheet { index: s })?;
                if self.active >= self.wb.sheets.len() || self.active > s {
                    self.active = self.active.saturating_sub(1).min(self.wb.sheets.len() - 1);
                } else if self.active == s {
                    self.active = s.min(self.wb.sheets.len() - 1);
                }
                Value::Empty
            }
            "visible" => Value::Long(if self.wb.sheets[s].hidden { 0 } else { -1 }),
            "parent" => obj(Obj::Workbook),
            "application" => obj(Obj::Application),
            "calculate" => Value::Empty,
            "type" => Value::Long(-4167),
            "next" => if s + 1 < self.wb.sheets.len() { obj(Obj::Sheet(s + 1)) } else { Value::Nothing },
            "previous" => if s > 0 { obj(Obj::Sheet(s - 1)) } else { Value::Nothing },
            "protectcontents" => Value::Bool(self.wb.sheets[s].protection.is_some()),
            "evaluate" => {
                let t = self.arg_str(&a, 0, "name")?.ok_or_else(Error::invalid_call)?;
                return self.evaluate(&t, s);
            }
            "protect" => {
                let pw = self.arg_str(&a, 0, "password")?.unwrap_or_default();
                if self.wb.sheets[s].protection.is_none() {
                    let p = apex_sheet::Protection::with_password(&pw, apex_sheet::Protection::default().allow);
                    self.apply(Op::Protect { sheet: Some(s), protection: Some(p) })?;
                }
                Value::Empty
            }
            "unprotect" => {
                let pw = self.arg_str(&a, 0, "password")?.unwrap_or_default();
                if let Some(p) = &self.wb.sheets[s].protection {
                    if p.has_password() && !p.unlocks(&pw) {
                        return Err(Error::app("The password you supplied is not correct"));
                    }
                    self.apply(Op::Protect { sheet: Some(s), protection: None })?;
                }
                Value::Empty
            }
            "copy" | "move" | "paste" | "pastespecial" | "shapes" | "chartobjects" | "listobjects" | "pivottables" | "printout" | "hyperlinks" | "comments" | "sort" | "autofilter" | "querytables" | "oleobjects" | "buttons" | "checkboxes" | "dropdowns" | "optionbuttons" | "listboxes" | "drawingobjects" => return Err(Error::unsupported(&format!("Worksheet.{name}"))),
            _ => return Err(Error::no_member(name)),
        })
    }

    /// A sheet by 1-based index or by name.
    fn sheet_index(&mut self, v: Value) -> R<usize> {
        match self.scalar(v)? {
            Value::Str(n) => self.wb.sheet_index(&n).ok_or_else(Error::subscript),
            v => {
                let i = v.to_i64()?;
                if i < 1 || i as usize > self.wb.sheets.len() {
                    return Err(Error::subscript());
                }
                Ok(i as usize - 1)
            }
        }
    }

    fn sheets_member(&mut self, name: &str, args: &[Arg]) -> R<Value> {
        let a = self.args(args)?;
        Ok(match name {
            "count" => Value::Long(self.wb.sheets.len() as i32),
            "item" | "_default" => {
                let v = a.get(0, "index").ok_or_else(Error::invalid_call)?;
                obj(Obj::Sheet(self.sheet_index(v)?))
            }
            "add" => {
                let at = if let Some(b) = a.get(0, "before") {
                    match b {
                        Value::Object(Obj::Sheet(i)) => i,
                        _ => return Err(Error::type_mismatch()),
                    }
                } else if let Some(x) = a.get(1, "after") {
                    match x {
                        Value::Object(Obj::Sheet(i)) => i + 1,
                        _ => return Err(Error::type_mismatch()),
                    }
                } else {
                    self.active
                };
                let count = self.arg_i64(&a, 2, "count")?.unwrap_or(1).clamp(1, 255) as usize;
                let at = at.min(self.wb.sheets.len());
                for k in 0..count {
                    let name = self.wb.unique_sheet_name("Sheet");
                    self.apply(Op::AddSheet { index: at + k, sheet: Box::new(Sheet::new(name)) })?;
                }
                self.active = at;
                self.selection = Range::single(CellRef::new(0, 0));
                obj(Obj::Sheet(at))
            }
            "application" => obj(Obj::Application),
            "parent" => obj(Obj::Workbook),
            _ => return Err(Error::no_member(name)),
        })
    }

    fn workbook_member(&mut self, name: &str, args: &[Arg]) -> R<Value> {
        Ok(match name {
            "name" => Value::str(self.file_name.clone()),
            "path" => Value::str(self.file_path.clone()),
            "fullname" => {
                if self.file_path.is_empty() {
                    Value::str(self.file_name.clone())
                } else {
                    Value::str(std::path::Path::new(&self.file_path).join(&self.file_name).to_string_lossy().to_string())
                }
            }
            "worksheets" | "sheets" => {
                if args.is_empty() {
                    obj(Obj::Sheets)
                } else {
                    return self.sheets_member("item", args);
                }
            }
            "activesheet" => obj(Obj::Sheet(self.active)),
            "save" => {
                self.save = true;
                Value::Empty
            }
            "saved" => Value::Bool(!self.wb.modified),
            "readonly" => Value::Bool(self.wb.read_only),
            "application" | "parent" => obj(Obj::Application),
            "item" | "_default" => obj(Obj::Workbook),
            "activate" => Value::Empty,
            "close" | "saveas" | "savecopyas" | "names" | "protect" | "unprotect" | "printout" => return Err(Error::unsupported(&format!("Workbook.{name}"))),
            _ => return Err(Error::no_member(name)),
        })
    }

    fn app_member(&mut self, name: &str, args: &[Arg]) -> R<Value> {
        let sheet = self.home_sheet();
        match name {
            "activesheet" | "activecell" | "selection" | "activeworkbook" | "thisworkbook" | "worksheets" | "sheets" | "worksheetfunction" => {
                return self.global(name, args).unwrap_or_else(|| Err(Error::no_member(name)));
            }
            "range" | "cells" | "rows" | "columns" => return self.sheet_member(sheet, name, args),
            _ => {}
        }
        let a = self.args(args)?;
        Ok(match name {
            "screenupdating" => Value::Bool(self.app.screen_updating),
            "displayalerts" => Value::Bool(self.app.display_alerts),
            "enableevents" => Value::Bool(self.app.enable_events),
            "calculation" => Value::Long(self.app.calculation),
            "statusbar" => match &self.app.status_bar {
                Some(s) => Value::str(s.clone()),
                None => Value::Bool(false),
            },
            "cutcopymode" => Value::Bool(false),
            "visible" | "interactive" => Value::Bool(true),
            "goto" => {
                let r = match a.get(0, "reference") {
                    Some(Value::Object(Obj::Range(r))) => r,
                    Some(v) => {
                        let t = self.scalar(v)?.to_str()?;
                        self.parse_ref(&t, sheet)?
                    }
                    None => return Err(Error::invalid_call()),
                };
                self.active = r.sheet;
                self.selection = r.range;
                Value::Empty
            }
            "name" => Value::str("ApexDesk"),
            "version" => Value::str("16.0"),
            "username" => Value::str(self.user_name.clone()),
            "operatingsystem" => Value::str(std::env::consts::OS),
            "pathseparator" => Value::str(std::path::MAIN_SEPARATOR.to_string()),
            "application" | "parent" => obj(Obj::Application),
            "calculate" | "calculatefull" => {
                self.wb.recalc_all();
                Value::Empty
            }
            "wait" | "volatile" | "doevents" => Value::Empty,
            "intersect" | "union" => {
                let mut ranges = Vec::new();
                for i in 0..a.len() {
                    match a.get(i, "") {
                        Some(Value::Object(Obj::Range(r))) => ranges.push(r),
                        Some(Value::Nothing) => return Err(Error::not_set()),
                        Some(_) => return Err(Error::type_mismatch()),
                        None => {}
                    }
                }
                let Some(first) = ranges.first().copied() else { return Err(Error::invalid_call()) };
                if ranges.iter().any(|r| r.sheet != first.sheet) {
                    return Err(Error::app(format!("Method '{name}' of object '_Application' failed")));
                }
                if name == "intersect" {
                    let mut acc = Some(first.range);
                    for r in &ranges[1..] {
                        acc = acc.and_then(|x| x.intersect(&r.range));
                    }
                    match acc {
                        Some(x) => range_obj(first.sheet, x),
                        None => Value::Nothing,
                    }
                } else {
                    // Only unions that are one rectangle.
                    let mut b = first.range;
                    for r in &ranges[1..] {
                        b = Range::new(CellRef::new(b.start.row.min(r.range.start.row), b.start.col.min(r.range.start.col)), CellRef::new(b.end.row.max(r.range.end.row), b.end.col.max(r.range.end.col)));
                    }
                    let covered: u64 = {
                        let mut n = 0u64;
                        for row in b.start.row..=b.end.row.min(b.start.row + 2000) {
                            for col in b.start.col..=b.end.col.min(b.start.col + 2000) {
                                if ranges.iter().any(|r| r.range.contains(CellRef::new(row, col))) {
                                    n += 1;
                                }
                            }
                        }
                        n
                    };
                    let size = (b.rows().min(2001) as u64) * (b.cols().min(2001) as u64);
                    if covered != size {
                        return Err(Error::unsupported("ranges of several areas"));
                    }
                    range_obj(first.sheet, b)
                }
            }
            "run" => {
                let macro_name = self.arg_str(&a, 0, "macro")?.ok_or_else(Error::invalid_call)?;
                let rest: Vec<Value> = (1..a.len()).filter_map(|i| a.get(i, "")).collect();
                let mut values = Vec::new();
                for v in rest {
                    values.push(self.scalar(v)?);
                }
                return self.run_named(macro_name.rsplit('!').next().unwrap_or(&macro_name).trim_matches('\''), values);
            }
            "evaluate" => {
                let t = self.arg_str(&a, 0, "name")?.ok_or_else(Error::invalid_call)?;
                return self.evaluate(&t, sheet);
            }
            "inputbox" => {
                let prompt = self.arg_str(&a, 0, "prompt")?.unwrap_or_default();
                let title = self.arg_str(&a, 1, "title")?.unwrap_or_default();
                let default = self.arg_str(&a, 2, "default")?.unwrap_or_default();
                let ty = self.arg_i64(&a, 7, "type")?.unwrap_or(2);
                let r = self.paused(|ui| ui.input_box(&prompt, &title, &default));
                match r {
                    None => Value::Bool(false),
                    Some(t) if ty & 1 != 0 && ty & 2 == 0 => match crate::value::parse_number(&t) {
                        Some(n) => Value::Double(n),
                        None => return Err(Error::type_mismatch()),
                    },
                    Some(t) if ty == 8 => obj(Obj::Range(self.parse_ref(&t, sheet)?)),
                    Some(t) => Value::str(t),
                }
            }
            "ontime" | "onkey" | "quit" | "getopenfilename" | "getsaveasfilename" | "filedialog" | "workbooks" | "vbe" | "sendkeys" | "dialogs" | "commandbars" | "addins" | "caller" | "activechart" | "charts" | "activewindow" | "windows" => return Err(Error::unsupported(&format!("Application.{name}"))),
            _ => {
                // Application.Sum(...) and the like: a worksheet function
                // whose errors come back as values.
                let vals: Vec<Value> = (0..a.len()).map(|i| a.get(i, "").unwrap_or(Value::Missing)).collect();
                return self.worksheet_function(name, vals, false);
            }
        })
    }

    // --- WorksheetFunction ---------------------------------------------------------

    fn formula_arg(&mut self, v: Value) -> R<String> {
        Ok(match v {
            Value::Missing => String::new(),
            Value::Object(Obj::Range(r)) => {
                let name = self.wb.sheets.get(r.sheet).map(|s| s.name.replace('\'', "''")).unwrap_or_default();
                format!("'{name}'!{}", r.range.to_a1())
            }
            Value::Object(_) | Value::Nothing => return Err(Error::type_mismatch()),
            Value::Str(s) => format!("\"{}\"", s.replace('"', "\"\"")),
            Value::Bool(b) => if b { "TRUE" } else { "FALSE" }.into(),
            Value::Empty => "0".into(),
            Value::Error(n) => error_kind(n).as_str().into(),
            Value::Array(a) => {
                let (rows, cols) = match a.bounds.len() {
                    1 => (1, a.data.len()),
                    2 => ((a.bounds[0].1 - a.bounds[0].0 + 1) as usize, (a.bounds[1].1 - a.bounds[1].0 + 1) as usize),
                    _ => return Err(Error::type_mismatch()),
                };
                let mut out = String::from("{");
                for i in 0..rows {
                    if i > 0 {
                        out.push(';');
                    }
                    for j in 0..cols {
                        if j > 0 {
                            out.push(',');
                        }
                        let x = a.data[i * cols + j].clone();
                        out.push_str(&self.formula_arg(x)?);
                    }
                }
                out.push('}');
                out
            }
            v => {
                let n = v.to_f64()?;
                if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", n as i64) } else { format!("{n:?}") }
            }
        })
    }

    /// `WorksheetFunction.Name(args)`, evaluated by the formula engine.
    fn worksheet_function(&mut self, name: &str, args: Vec<Value>, raise: bool) -> R<Value> {
        let mut parts = Vec::with_capacity(args.len());
        for v in args {
            parts.push(self.formula_arg(v)?);
        }
        while parts.last().is_some_and(String::is_empty) {
            parts.pop();
        }
        let src = format!("={}({})", name.to_uppercase(), parts.join(","));
        let fail = || Error::app(format!("Unable to get the {name} property of the WorksheetFunction class"));
        let e = apex_sheet::formula::parse_canonical(&src).map_err(|_| Error::no_member(name))?;
        let mut unknown = false;
        e.visit_unknown_calls(&mut |_| unknown = true);
        if unknown {
            return Err(Error::no_member(name));
        }
        let sheet = self.home_sheet();
        let v = apex_sheet::functions::Env::new(&*self.wb, sheet, CellRef::default(), Locale::EN).eval(&e);
        match v {
            apex_sheet::Value::Error(_) if raise => Err(fail()),
            v => Ok(self.from_cell_value(&v, false)),
        }
    }
}
