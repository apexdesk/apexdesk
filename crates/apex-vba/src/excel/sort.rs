//! Range.Sort Key1, Order1, Key2, Type, Order2, Key3, Order3, Header,
//! OrderCustom, MatchCase, Orientation: as the Sort dialog sorts.

use apex_sheet::{CellRef, Range, SortKey};

use super::{Args, Interp, obj};
use crate::error::{Error, R};
use crate::object::{Obj, Part, RangeRef, SortState};
use crate::value::Value;

impl Interp<'_> {
    pub(super) fn range_sort(&mut self, r: RangeRef, a: &Args) -> R<Value> {
        let by_cols = self.arg_i64(a, 10, "orientation")? == Some(2);
        let mut rg = if r.range.cell_count() > 1_000_000 {
            match self.wb.clamp_to_used(r.sheet, &r.range) {
                Some(x) => x,
                None => return Ok(Value::Empty),
            }
        } else {
            r.range
        };
        if r.range.is_single() {
            rg = self.current_region(r.sheet, r.range);
        }
        Self::check_size(&rg)?;
        let mut keys = Vec::new();
        for (k, o, names) in [(0, 1, ("key1", "order1")), (2, 4, ("key2", "order2")), (5, 6, ("key3", "order3"))] {
            let Some(key) = a.get(k, names.0) else { continue };
            let key = match key {
                Value::Object(Obj::Range(x)) => x.range.start,
                v => {
                    let t = self.scalar(v)?.to_str()?;
                    self.parse_ref(&t, r.sheet)?.range.start
                }
            };
            let line = if by_cols { key.row } else { key.col };
            let (lo, hi) = if by_cols { (rg.start.row, rg.end.row) } else { (rg.start.col, rg.end.col) };
            if !(lo..=hi).contains(&line) {
                return Err(Error::app("The sort reference is not valid"));
            }
            let ascending = self.arg_i64(a, o, names.1)?.unwrap_or(1) != 2;
            keys.push(SortKey::new(line, ascending));
        }
        if keys.is_empty() {
            return Err(Error::app("The sort reference is not valid"));
        }
        // xlYes: the first line is a header; xlGuess and xlNo sort it too.
        if self.arg_i64(a, 7, "header")? == Some(1) {
            if by_cols {
                if rg.start.col == rg.end.col {
                    return Ok(Value::Empty);
                }
                rg = Range::new(CellRef::new(rg.start.row, rg.start.col + 1), rg.end);
            } else {
                if rg.start.row == rg.end.row {
                    return Ok(Value::Empty);
                }
                rg = Range::new(CellRef::new(rg.start.row + 1, rg.start.col), rg.end);
            }
        }
        let case = match a.get(9, "matchcase") {
            Some(v) => self.scalar(v)?.to_bool()?,
            None => false,
        };
        let op = self.wb.sort_op(r.sheet, &rg, &keys, case, by_cols);
        self.apply(op)?;
        Ok(Value::Empty)
    }

    /// Worksheet.Sort: SortFields, SetRange, Header, Orientation, MatchCase
    /// and Apply, which sorts as Range.Sort does.
    pub(super) fn sort_member(&mut self, s: usize, name: &str, a: &Args) -> R<Value> {
        let st = self.sorts.get(&s).cloned().unwrap_or_default();
        Ok(match name {
            "sortfields" => obj(Obj::Part(Box::new(Part::SortFields(s)))),
            "setrange" => {
                let Some(Value::Object(Obj::Range(r))) = a.get(0, "rng") else { return Err(Error::invalid_call()) };
                self.sorts.entry(s).or_default().range = Some(r);
                Value::Empty
            }
            "rng" => match st.range {
                Some(r) => obj(Obj::Range(r)),
                None => Value::Nothing,
            },
            "header" => Value::Long(st.header as i32),
            "orientation" => Value::Long(st.orientation.max(1) as i32),
            "matchcase" => Value::Bool(st.match_case),
            "parent" => obj(Obj::Sheet(s)),
            "apply" => {
                let r = st.range.ok_or_else(|| Error::app("The sort reference is not valid"))?;
                if st.keys.is_empty() || st.keys.len() > 3 {
                    return Err(Error::app("The sort reference is not valid"));
                }
                let mut args = Vec::new();
                for (i, (k, asc)) in st.keys.iter().enumerate() {
                    args.push((Some(format!("key{}", i + 1)), obj(Obj::Range(*k))));
                    args.push((Some(format!("order{}", i + 1)), Value::Long(if *asc { 1 } else { 2 })));
                }
                args.push((Some("header".into()), Value::Long(st.header as i32)));
                args.push((Some("matchcase".into()), Value::Bool(st.match_case)));
                args.push((Some("orientation".into()), Value::Long(st.orientation.max(1) as i32)));
                return self.range_sort(r, &Args(args));
            }
            _ => return Err(Error::no_member(name)),
        })
    }

    pub(super) fn sort_set(&mut self, s: usize, name: &str, v: Value) -> R<()> {
        let st: &mut SortState = self.sorts.entry(s).or_default();
        match name {
            "header" => st.header = v.to_i64()?,
            "orientation" => st.orientation = v.to_i64()?,
            "matchcase" => st.match_case = v.to_bool()?,
            _ => return Err(Error::no_member(name)),
        }
        Ok(())
    }

    /// Worksheet.Sort.SortFields: Add(Key, SortOn, Order), Clear, Count, Item.
    pub(super) fn sort_fields_member(&mut self, s: usize, name: &str, a: &Args) -> R<Value> {
        match name {
            "clear" => {
                self.sorts.entry(s).or_default().keys.clear();
                Ok(Value::Empty)
            }
            "count" => Ok(Value::Long(self.sorts.get(&s).map_or(0, |x| x.keys.len()) as i32)),
            "add" => {
                let Some(Value::Object(Obj::Range(k))) = a.get(0, "key") else { return Err(Error::invalid_call()) };
                let ascending = self.arg_i64(a, 2, "order")?.unwrap_or(1) != 2;
                let st = self.sorts.entry(s).or_default();
                st.keys.push((k, ascending));
                Ok(obj(Obj::Part(Box::new(Part::SortField(s, st.keys.len() - 1)))))
            }
            "item" | "_default" => {
                let i = self.arg_i64(a, 0, "index")?.ok_or_else(Error::invalid_call)?;
                let n = self.sorts.get(&s).map_or(0, |x| x.keys.len());
                if i < 1 || i as usize > n {
                    return Err(Error::subscript());
                }
                Ok(obj(Obj::Part(Box::new(Part::SortField(s, i as usize - 1)))))
            }
            _ => Err(Error::no_member(name)),
        }
    }

    pub(super) fn sort_field_member(&mut self, s: usize, i: usize, name: &str) -> R<Value> {
        let (k, asc) = *self.sorts.get(&s).and_then(|x| x.keys.get(i)).ok_or_else(Error::subscript)?;
        Ok(match name {
            "key" => obj(Obj::Range(k)),
            "order" => Value::Long(if asc { 1 } else { 2 }),
            "sorton" => Value::Long(0),
            _ => return Err(Error::no_member(name)),
        })
    }
}
