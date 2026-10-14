//! Pivot tables from macros: a sheet's `PivotTables`, a `PivotTable` to
//! refresh (`RefreshTable`, `PivotCache.Refresh`), `Range.PivotTable` and
//! `Workbook.RefreshAll`. They are made in Insert > Pivot Table; macros
//! refresh and read them. Named PivotTable1, 2... in each sheet's order.

use apex_sheet::workbook::AnalysisError;

use super::{Args, Interp, obj, range_obj};
use crate::error::{Error, R};
use crate::object::{Obj, Part, RangeRef};
use crate::value::Value;

fn pivot(i: usize) -> Value {
    obj(Obj::Part(Box::new(Part::PivotTable(i))))
}

fn failed(e: AnalysisError) -> Error {
    Error::app(format!("The pivot table can't be refreshed: {e:?}"))
}

impl Interp<'_> {
    /// The pivot tables of sheet `s`, as indexes of the workbook's.
    fn pivots_of(&self, s: usize) -> Vec<usize> {
        let Some(sh) = self.wb.sheets.get(s) else { return Vec::new() };
        self.wb.analysis.pivots.iter().enumerate().filter(|(_, p)| p.sheet == sh.name).map(|(i, _)| i).collect()
    }

    /// The sheet of pivot `i` and its name there.
    fn pivot_place(&self, i: usize) -> R<(usize, String)> {
        let p = self.wb.analysis.pivots.get(i).ok_or_else(|| Error::app("The pivot table is gone"))?;
        let s = self.wb.sheet_index(&p.sheet).ok_or_else(|| Error::app("The pivot table is gone"))?;
        let k = self.pivots_of(s).iter().position(|&x| x == i).unwrap_or(0);
        Ok((s, format!("PivotTable{}", k + 1)))
    }

    pub(super) fn pivot_tables(&mut self, s: usize, name: &str, a: &Args) -> R<Value> {
        let list = self.pivots_of(s);
        match name {
            "count" => Ok(Value::Long(list.len() as i32)),
            "item" | "_default" => {
                let v = a.get(0, "index").ok_or_else(Error::invalid_call)?;
                let k = match self.scalar(v)? {
                    Value::Str(n) => n.to_ascii_lowercase().strip_prefix("pivottable").and_then(|k| k.parse::<usize>().ok()).and_then(|k| k.checked_sub(1)),
                    v => usize::try_from(v.to_i64()? - 1).ok(),
                };
                k.and_then(|k| list.get(k)).map(|&i| pivot(i)).ok_or_else(Error::subscript)
            }
            "parent" => Ok(obj(Obj::Sheet(s))),
            _ => Err(Error::no_member(name)),
        }
    }

    pub(super) fn pivot_items(&self, s: usize) -> Vec<Value> {
        self.pivots_of(s).into_iter().map(pivot).collect()
    }

    pub(super) fn pivot_member(&mut self, i: usize, name: &str) -> R<Value> {
        let (s, pname) = self.pivot_place(i)?;
        let p = &self.wb.analysis.pivots[i];
        Ok(match name {
            "name" | "_default" | "value" => Value::str(pname),
            "refreshtable" => {
                self.via(|wb| wb.refresh_pivot(i))?.map_err(failed)?;
                Value::Bool(true)
            }
            "pivotcache" => obj(Obj::Part(Box::new(Part::PivotCache(i)))),
            "tablerange1" | "tablerange2" => match p.area {
                Some(r) => range_obj(s, r),
                None => Value::Nothing,
            },
            "sourcedata" => {
                let src = &p.spec.source_sheet;
                if p.spec.model.is_some() || src.is_empty() {
                    Value::str("")
                } else {
                    Value::str(format!("'{}'!{}", src.replace('\'', "''"), Self::address(&p.spec.source, true, true)))
                }
            }
            "parent" => obj(Obj::Sheet(s)),
            "application" => obj(Obj::Application),
            _ => return Err(Error::no_member(name)),
        })
    }

    pub(super) fn pivot_cache_member(&mut self, i: usize, name: &str) -> R<Value> {
        self.pivot_place(i)?;
        match name {
            "refresh" => {
                self.via(|wb| wb.refresh_pivot(i))?.map_err(failed)?;
                Ok(Value::Empty)
            }
            "recordcount" => {
                let spec = self.wb.analysis.pivots[i].spec.clone();
                let n = self.wb.pivot_source(&spec).map(|s| s.records).unwrap_or(0);
                Ok(Value::Long(n as i32))
            }
            _ => Err(Error::no_member(name)),
        }
    }

    /// `Range.PivotTable`: the one whose output holds its first cell.
    pub(super) fn range_pivot(&self, r: RangeRef) -> R<Value> {
        let found = self.pivots_of(r.sheet).into_iter().find(|&i| self.wb.analysis.pivots[i].area.is_some_and(|a| a.contains(r.range.start)));
        found.map(pivot).ok_or_else(|| Error::app("The cell isn't in a pivot table"))
    }

    /// `Workbook.RefreshAll`: every pivot table.
    pub(super) fn refresh_all(&mut self) -> R<Value> {
        if !self.wb.analysis.pivots.is_empty() {
            self.via(|wb| wb.refresh_all_pivots())?;
        }
        Ok(Value::Empty)
    }
}
