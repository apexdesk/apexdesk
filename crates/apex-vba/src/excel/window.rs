//! ActiveWindow (frozen panes; zoom and scrolling are the view's and are
//! taken without effect), Worksheet.Copy and Move within the workbook,
//! and AutoFilter.

use apex_sheet::data::{AutoFilter, ColumnFilter, FilterKind};
use apex_sheet::functions::CritOp;
use apex_sheet::{CellRef, Op, Range};

use super::{Args, Interp, obj};
use crate::error::{Error, R};
use crate::object::{Obj, RangeRef};
use crate::value::Value;

impl Interp<'_> {
    pub(super) fn window_get(&mut self, name: &str) -> R<Value> {
        let sh = &self.wb.sheets[self.active];
        let (rows, cols) = self.split.unwrap_or((sh.frozen_rows, sh.frozen_cols));
        Ok(match name {
            "freezepanes" => Value::Bool(sh.frozen_rows > 0 || sh.frozen_cols > 0),
            "splitrow" => Value::Long(rows as i32),
            "splitcolumn" => Value::Long(cols as i32),
            "zoom" => Value::Long(100),
            "scrollrow" | "scrollcolumn" => Value::Long(1),
            "displaygridlines" | "displayheadings" | "displayworkbooktabs" | "visible" => Value::Bool(true),
            "windowstate" => Value::Long(-4143),
            "caption" => Value::str(self.file_name.clone()),
            "activesheet" => obj(Obj::Sheet(self.active)),
            "activecell" => obj(Obj::Range(RangeRef::new(self.active, Range::single(self.selection.start)))),
            "selection" => obj(Obj::Range(RangeRef::new(self.active, self.selection))),
            _ => return Err(Error::no_member(name)),
        })
    }

    pub(super) fn window_set(&mut self, name: &str, v: Value) -> R<()> {
        let s = self.active;
        let frozen = self.wb.sheets[s].frozen_rows > 0 || self.wb.sheets[s].frozen_cols > 0;
        match name {
            "freezepanes" => {
                if v.to_bool()? {
                    // At the split, else above and left of the active cell.
                    let (rows, cols) = self.split.unwrap_or((self.selection.start.row, self.selection.start.col));
                    self.freeze(rows, cols)?;
                } else {
                    self.split = None;
                    self.freeze(0, 0)?;
                }
            }
            "splitrow" | "splitcolumn" => {
                let n = v.to_i64()?.clamp(0, 10_000) as u32;
                let sh = &self.wb.sheets[s];
                let (mut rows, mut cols) = self.split.unwrap_or((sh.frozen_rows, sh.frozen_cols));
                if name == "splitrow" {
                    rows = n;
                } else {
                    cols = n;
                }
                self.split = Some((rows, cols));
                if frozen {
                    self.freeze(rows, cols)?;
                }
            }
            "zoom" | "scrollrow" | "scrollcolumn" | "displaygridlines" | "displayheadings" | "displayworkbooktabs" | "windowstate" | "split" | "smallscroll" | "largescroll" => {}
            _ => return Err(Error::no_member(name)),
        }
        Ok(())
    }

    fn freeze(&mut self, rows: u32, cols: u32) -> R<()> {
        let sh = &self.wb.sheets[self.active];
        if (sh.frozen_rows, sh.frozen_cols) == (rows, cols) {
            return Ok(());
        }
        self.apply(Op::Freeze { sheet: self.active, rows, cols, top: CellRef::new(0, 0) })
    }

    /// Worksheet.Copy / Worksheet.Move with Before or After.
    pub(super) fn sheet_copy_move(&mut self, s: usize, copy: bool, a: &Args) -> R<Value> {
        let dest = match (a.get(0, "before"), a.get(1, "after")) {
            (Some(Value::Object(Obj::Sheet(i))), _) => i,
            (None, Some(Value::Object(Obj::Sheet(i)))) => i + 1,
            (None, None) => return Err(Error::unsupported(if copy { "Worksheet.Copy to a new workbook" } else { "Worksheet.Move to a new workbook" })),
            _ => return Err(Error::type_mismatch()),
        };
        let dest = dest.min(self.wb.sheets.len());
        if copy {
            let mut sh = self.wb.sheets[s].clone();
            let base = format!("{} (", sh.name);
            sh.name = (2..).map(|n| format!("{base}{n})")).find(|n| self.wb.sheet_index(n).is_none()).unwrap_or_default();
            sh.hidden = false;
            sh.code_name = None;
            self.apply(Op::AddSheet { index: dest, sheet: Box::new(sh) })?;
            self.active = dest;
            self.selection = Range::single(CellRef::new(0, 0));
        } else {
            let to = if s < dest { dest - 1 } else { dest };
            if to != s {
                let active = self.wb.sheets[self.active].name.clone();
                self.apply(Op::MoveSheet { from: s, to })?;
                self.active = self.wb.sheet_index(&active).unwrap_or(to);
            }
        }
        Ok(Value::Empty)
    }

    /// Range.AutoFilter: without arguments on for the range (its block of
    /// cells when it is one cell), off when the sheet has one; with
    /// Field and criteria, that column's filter (Field alone shows all).
    pub(super) fn auto_filter(&mut self, r: RangeRef, a: &Args) -> R<Value> {
        let failed = || Error::app("AutoFilter method of Range class failed");
        let cur = self.wb.sheets[r.sheet].filter.clone();
        if a.0.is_empty() {
            let filter = if cur.is_some() {
                None
            } else {
                let rg = if r.range.is_single() { self.current_region(r.sheet, r.range) } else { r.range };
                Some(AutoFilter::new(rg))
            };
            self.via(|wb| wb.set_filter(r.sheet, filter, "undo.filter"))?;
            return Ok(Value::Empty);
        }
        let mut filter = match cur {
            Some(f) if f.range.contains(r.range.start) => f,
            _ => AutoFilter::new(if r.range.is_single() { self.current_region(r.sheet, r.range) } else { r.range }),
        };
        let field = match self.arg_i64(a, 0, "field")? {
            Some(n) => n,
            None => {
                self.via(|wb| wb.set_filter(r.sheet, Some(filter), "undo.filter"))?;
                return Ok(Value::Empty);
            }
        };
        let width = i64::from(filter.range.end.col - filter.range.start.col) + 1;
        if !(1..=width).contains(&field) {
            return Err(failed());
        }
        let col = filter.range.start.col + (field - 1) as u32;
        let c1 = match a.get(1, "criteria1") {
            Some(v) => Some(self.scalar(v)?),
            None => None,
        };
        let op = self.arg_i64(a, 2, "operator")?;
        let c2 = self.arg_str(a, 3, "criteria2")?;
        let text = |v: &Value| v.to_str();
        let kind = match (c1, op) {
            (None, None) => None,
            (Some(Value::Array(arr)), None | Some(7)) => {
                let mut items = Vec::new();
                let mut blanks = false;
                for v in arr.data.iter() {
                    let t = text(v)?;
                    let t = t.strip_prefix('=').unwrap_or(&t).to_string();
                    if t.is_empty() {
                        blanks = true;
                    } else {
                        items.push(t);
                    }
                }
                Some(FilterKind::Values { items, blanks })
            }
            (Some(v), Some(3..=6)) => {
                let o = op.unwrap_or(3);
                Some(FilterKind::Top { top: o == 3 || o == 5, percent: o >= 5, n: v.to_f64()? })
            }
            (Some(v), Some(11)) => match v.to_i64()? {
                33 | 34 => Some(FilterKind::Average { above: v.to_i64()? == 33 }),
                d => return Err(Error::unsupported(&format!("AutoFilter with dynamic criteria {d}"))),
            },
            (Some(v), None | Some(1 | 2)) => {
                let b = c2.map(|t| criterion(&t));
                Some(FilterKind::Custom { and: op != Some(2), a: criterion(&text(&v)?), b })
            }
            (_, Some(o)) => return Err(Error::unsupported(&format!("AutoFilter with Operator {o}"))),
        };
        filter.cols.retain(|f| f.col != col);
        if let Some(kind) = kind {
            filter.cols.push(ColumnFilter { col, kind });
        }
        self.via(|wb| wb.set_filter(r.sheet, Some(filter), "undo.filter"))?;
        Ok(Value::Empty)
    }

    /// Worksheet.AutoFilterMode = False.
    pub(super) fn set_auto_filter_mode(&mut self, s: usize, on: bool) -> R<()> {
        if on {
            return Err(Error::app("Unable to set the AutoFilterMode property"));
        }
        if self.wb.sheets[s].filter.is_some() {
            self.via(|wb| wb.set_filter(s, None, "undo.filter"))?;
        }
        Ok(())
    }
}

/// An AutoFilter criterion: `">100"`, `"<>x"`, `"=a*"` or `"a"` (equals).
fn criterion(t: &str) -> (CritOp, String) {
    for (p, op) in [("<>", CritOp::Ne), (">=", CritOp::Ge), ("<=", CritOp::Le), ("=", CritOp::Eq), (">", CritOp::Gt), ("<", CritOp::Lt)] {
        if let Some(rest) = t.strip_prefix(p) {
            return (op, rest.to_string());
        }
    }
    (CritOp::Eq, t.to_string())
}
