//! Ranges of several areas (`Range("A1:B2,D4")`, Union, SpecialCells): a
//! list of objects of one sheet. Members that act or give objects are
//! done on each area (the objects they give make another list); those
//! that give a value are the first area's, like Excel.

use std::rc::Rc;

use apex_sheet::{MAX_COLS, MAX_ROWS, Range};

use super::{Interp, obj, range_obj};
use crate::ast::{Arg, Expr};
use crate::error::{Error, R};
use crate::object::{By, Collection, Obj, RangeRef};
use crate::value::Value;

/// More areas than this aren't made (a SpecialCells of scattered cells).
const MAX_AREAS: usize = 100_000;

/// `ranges` of `sheet` as one Range, several or Nothing.
pub(super) fn areas(sheet: usize, ranges: Vec<Range>) -> R<Value> {
    if ranges.len() > MAX_AREAS {
        return Err(Error::app("Too many areas"));
    }
    let mut keys = std::collections::HashSet::with_capacity(ranges.len());
    let seen: Vec<Range> = ranges.into_iter().filter(|r| keys.insert((r.start.row, r.start.col, r.end.row, r.end.col))).collect();
    Ok(match seen.len() {
        0 => Value::Nothing,
        1 => range_obj(sheet, seen[0]),
        _ => obj(Obj::Multi(seen.into_iter().map(|r| Obj::Range(RangeRef::new(sheet, r))).collect())),
    })
}

/// The areas of a Range value (one, or those of a list).
pub(super) fn ranges_of(v: &Value) -> Option<Vec<RangeRef>> {
    match v {
        Value::Object(Obj::Range(r)) => Some(vec![*r]),
        Value::Object(Obj::Multi(items)) => items.iter().map(|o| if let Obj::Range(r) = o { Some(*r) } else { None }).collect(),
        _ => None,
    }
}

/// Whole rows (`by_rows`) or whole columns that touch or overlap, joined:
/// deleting them one by one would move the others.
fn joined(mut spans: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    spans.sort_unstable();
    let mut out: Vec<(u32, u32)> = Vec::new();
    for (a, b) in spans {
        match out.last_mut() {
            Some(l) if a <= l.1.saturating_add(1) => l.1 = l.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

impl Interp<'_> {
    pub(super) fn multi_member(&mut self, items: Rc<[Obj]>, name: &str, args: &[Arg]) -> R<Value> {
        // The arguments once, not once per area.
        let a = self.args(args)?;
        let lit: Vec<Arg> = a.0.iter().map(|(n, v)| Arg { name: n.clone(), expr: (!matches!(v, Value::Missing)).then(|| Expr::Lit(v.clone())) }).collect();
        let first = || Value::Object(items[0].clone());
        let ranges = ranges_of(&obj(Obj::Multi(items.clone())));
        match name {
            "areas" => {
                let Some(ranges) = ranges else { return Err(Error::no_member(name)) };
                return match a.get(0, "index") {
                    Some(i) => {
                        let i = self.scalar(i)?.to_i64()?;
                        let r = usize::try_from(i - 1).ok().and_then(|i| ranges.get(i)).ok_or_else(Error::subscript)?;
                        Ok(obj(Obj::Range(*r)))
                    }
                    None => {
                        let mut c = Collection::default();
                        c.items = ranges.iter().map(|r| (None, obj(Obj::Range(*r)))).collect();
                        Ok(obj(Obj::Collection(Rc::new(std::cell::RefCell::new(c)))))
                    }
                };
            }
            "count" | "countlarge" => {
                let mut n = 0f64;
                for o in items.iter() {
                    n += self.member(Value::Object(o.clone()), name, &lit)?.to_f64()?;
                }
                return Ok(if n <= i32::MAX as f64 { Value::Long(n as i32) } else { Value::Double(n) });
            }
            "address" | "addresslocal" => {
                let mut parts = Vec::with_capacity(items.len());
                for o in items.iter() {
                    parts.push(self.member(Value::Object(o.clone()), name, &lit)?.to_str()?);
                }
                return Ok(Value::str(parts.join(",")));
            }
            "cells" if a.is_empty() => return Ok(obj(Obj::Multi(items))),
            // The first area's, like Excel.
            "select" | "activate" | "cells" | "rows" | "columns" | "item" | "_default" | "range" | "end" | "currentregion" | "find" | "findnext" | "findprevious" | "worksheet" | "parent" | "application" | "row" | "column" => {
                return self.member(first(), name, &lit);
            }
            // Areas in the same columns (the visible rows of a filter) or
            // the same rows go one after the other, as Excel pastes them.
            "copy" if a.get(0, "destination").is_some() => {
                let Some(mut ranges) = ranges else { return Err(Error::no_member(name)) };
                let Some(Value::Object(Obj::Range(dest))) = a.get(0, "destination") else {
                    return Err(Error::app("That command cannot be used on multiple selections"));
                };
                let r0 = ranges[0].range;
                let same_cols = ranges.iter().all(|r| r.sheet == ranges[0].sheet && (r.range.start.col, r.range.end.col) == (r0.start.col, r0.end.col));
                let same_rows = ranges.iter().all(|r| r.sheet == ranges[0].sheet && (r.range.start.row, r.range.end.row) == (r0.start.row, r0.end.row));
                if !same_cols && !same_rows {
                    return Err(Error::app("That command cannot be used on multiple selections"));
                }
                ranges.sort_by_key(|r| (r.range.start.row, r.range.start.col));
                let mut at = dest.range.start;
                for r in ranges {
                    let to = RangeRef { sheet: dest.sheet, range: Range::single(at), by: By::Cells };
                    self.member(obj(Obj::Range(r)), "copy", &[Arg { name: None, expr: Some(Expr::Lit(obj(Obj::Range(to)))) }])?;
                    if same_cols {
                        at.row += r.range.end.row - r.range.start.row + 1;
                    } else {
                        at.col += r.range.end.col - r.range.start.col + 1;
                    }
                }
                return Ok(Value::Bool(true));
            }
            "copy" | "cut" | "pastespecial" | "sort" | "autofilter" | "merge" => {
                return Err(Error::app("That command cannot be used on multiple selections"));
            }
            "delete" | "insert" => {
                let Some(ranges) = ranges else { return Err(Error::no_member(name)) };
                return self.multi_shift(ranges, name, &lit);
            }
            _ => {}
        }
        let mut out = Vec::with_capacity(items.len());
        for o in items.iter() {
            out.push(self.member(Value::Object(o.clone()), name, &lit)?);
        }
        if out.iter().all(|v| matches!(v, Value::Object(_))) {
            let objs: Vec<Obj> = out.into_iter().filter_map(|v| if let Value::Object(o) = v { Some(o) } else { None }).collect();
            return Ok(if objs.len() == 1 { obj(objs.into_iter().next().unwrap_or(Obj::Window)) } else { obj(Obj::Multi(objs.into())) });
        }
        Ok(out.into_iter().next().unwrap_or(Value::Empty))
    }

    pub(super) fn multi_set(&mut self, items: &Rc<[Obj]>, name: &str, args: &[Arg], v: Value) -> R<()> {
        let a = self.args(args)?;
        let lit: Vec<Arg> = a.0.iter().map(|(n, v)| Arg { name: n.clone(), expr: (!matches!(v, Value::Missing)).then(|| Expr::Lit(v.clone())) }).collect();
        for o in items.iter() {
            self.set_member(Value::Object(o.clone()), name, &lit, v.clone())?;
        }
        Ok(())
    }

    /// Delete or Insert of several areas: from the last one up, so the
    /// others stay where they are; whole rows or columns joined first.
    fn multi_shift(&mut self, ranges: Vec<RangeRef>, name: &str, args: &[Arg]) -> R<Value> {
        let sheet = ranges[0].sheet;
        let rows = ranges.iter().all(|r| r.range.start.col == 0 && r.range.end.col == MAX_COLS - 1);
        let cols = !rows && ranges.iter().all(|r| r.range.start.row == 0 && r.range.end.row == MAX_ROWS - 1);
        let mut list: Vec<RangeRef> = if rows && name == "delete" {
            joined(ranges.iter().map(|r| (r.range.start.row, r.range.end.row)).collect()).into_iter().map(|(a, b)| RangeRef { sheet, range: Range::new(apex_sheet::CellRef::new(a, 0), apex_sheet::CellRef::new(b, MAX_COLS - 1)), by: By::Rows }).collect()
        } else if cols && name == "delete" {
            joined(ranges.iter().map(|r| (r.range.start.col, r.range.end.col)).collect()).into_iter().map(|(a, b)| RangeRef { sheet, range: Range::new(apex_sheet::CellRef::new(0, a), apex_sheet::CellRef::new(MAX_ROWS - 1, b)), by: By::Cols }).collect()
        } else {
            let overlap = ranges.iter().enumerate().any(|(i, r)| ranges[i + 1..].iter().any(|s| s.range.intersect(&r.range).is_some()));
            if overlap {
                return Err(Error::app("Cannot use that command on overlapping selections"));
            }
            ranges
        };
        list.sort_by_key(|r| std::cmp::Reverse((r.range.start.row, r.range.start.col)));
        for r in list {
            self.member(obj(Obj::Range(r)), name, args)?;
        }
        Ok(Value::Empty)
    }
}

#[cfg(test)]
mod tests {
    use super::joined;

    #[test]
    fn spans_join() {
        assert_eq!(joined(vec![(5, 6), (1, 2), (3, 4), (9, 9), (2, 3)]), [(1, 6), (9, 9)]);
    }
}
