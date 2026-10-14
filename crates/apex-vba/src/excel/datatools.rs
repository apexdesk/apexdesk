//! Range.TextToColumns, Range.AdvancedFilter and Worksheet.ShowAllData: the
//! Data tab's tools, as the workbook runs them for the dialogs.

use apex_sheet::workbook::AdvancedFilterError;
use apex_sheet::{CellRef, Op, Range, SplitMode};

use super::{Args, Interp};
use crate::error::{Error, R};
use crate::object::RangeRef;
use crate::value::Value;

/// XlColumnDataType: General and Skip are the ones the workbook splits as.
const GENERAL: i64 = 1;
const SKIP: i64 = 9;

impl Interp<'_> {
    /// Range.TextToColumns Destination, DataType, TextQualifier,
    /// ConsecutiveDelimiter, Tab, Semicolon, Comma, Space, Other, OtherChar,
    /// FieldInfo.
    pub(super) fn text_to_columns(&mut self, r: RangeRef, a: &Args) -> R<Value> {
        let s = self.check_sheet(r.sheet)?;
        if r.range.start.col != r.range.end.col {
            return Err(Error::app("Text to Columns can only convert one column at a time"));
        }
        let dest = match a.get(0, "destination") {
            Some(v) => {
                let d = self.range_arg(v, s)?;
                if d.sheet != s {
                    return Err(Error::app("The destination must be on the same sheet as the data"));
                }
                d.range.start
            }
            None => r.range.start,
        };
        let fixed = match self.arg_i64(a, 1, "datatype")?.unwrap_or(1) {
            1 => false,
            2 => true,
            _ => return Err(Error::invalid_call()),
        };
        let fields = match a.get(10, "fieldinfo") {
            Some(v) => self.field_info(v)?,
            None => Vec::new(),
        };
        let mode = if fixed {
            let mut starts: Vec<(i64, i64)> = fields.clone();
            starts.sort_by_key(|f| f.0);
            SplitMode::Fixed(starts.iter().filter(|f| f.0 > 0).map(|f| f.0.min(32_767) as usize).collect())
        } else {
            let quote = match self.arg_i64(a, 2, "textqualifier")?.unwrap_or(1) {
                1 => Some('"'),
                2 => Some('\''),
                -4142 => None,
                _ => return Err(Error::invalid_call()),
            };
            let mut delims = Vec::new();
            for (pos, name, ch) in [(4, "tab", '\t'), (5, "semicolon", ';'), (6, "comma", ','), (7, "space", ' ')] {
                if self.flag(a, pos, name)? {
                    delims.push(ch);
                }
            }
            if self.flag(a, 8, "other")?
                && let Some(c) = self.arg_str(a, 9, "otherchar")?.and_then(|t| t.chars().next())
            {
                delims.push(c);
            }
            SplitMode::Delimited { delims, merge: self.flag(a, 3, "consecutivedelimiter")?, quote }
        };
        // Fields are numbered from 1 (delimited) or in order of position (fixed).
        let order: Vec<i64> = if fixed {
            let mut f = fields.clone();
            f.sort_by_key(|f| f.0);
            // A first field that starts past 0 still has the text before it.
            let lead = f.first().is_none_or(|x| x.0 > 0);
            std::iter::repeat_n(GENERAL, usize::from(lead)).chain(f.iter().map(|x| x.1)).collect()
        } else {
            let mut out = Vec::new();
            for (col, ty) in &fields {
                let i = usize::try_from(*col - 1).map_err(|_| Error::invalid_call())?.min(16_384);
                if out.len() <= i {
                    out.resize(i + 1, GENERAL);
                }
                out[i] = *ty;
            }
            out
        };
        if let Some(t) = order.iter().find(|t| **t != GENERAL && **t != SKIP) {
            return Err(Error::unsupported(&format!("TextToColumns column data type {t}")));
        }
        let skip: Vec<usize> = order.iter().enumerate().filter(|(_, t)| **t == SKIP).map(|(i, _)| i).collect();
        let rg = r.range;
        self.via(|wb| wb.text_to_columns(s, &rg, &mode, dest, &skip))?;
        Ok(Value::Empty)
    }

    fn flag(&mut self, a: &Args, pos: usize, name: &str) -> R<bool> {
        match a.get(pos, name) {
            Some(v) => self.scalar(v)?.to_bool(),
            None => Ok(false),
        }
    }

    /// FieldInfo: `Array(Array(column, type), ...)` or a two-column array.
    fn field_info(&mut self, v: Value) -> R<Vec<(i64, i64)>> {
        let Value::Array(arr) = v else { return Err(Error::type_mismatch()) };
        let mut out = Vec::new();
        if arr.bounds.len() == 2 {
            for pair in arr.data.chunks(2) {
                if let [c, t] = pair {
                    out.push((c.to_i64()?, t.to_i64()?));
                }
            }
            return Ok(out);
        }
        for item in &arr.data {
            let Value::Array(p) = item else { return Err(Error::type_mismatch()) };
            match p.data.as_slice() {
                [c, t] => out.push((c.to_i64()?, t.to_i64()?)),
                _ => return Err(Error::invalid_call()),
            }
        }
        Ok(out)
    }

    /// Range.AdvancedFilter Action, CriteriaRange, CopyToRange, Unique.
    pub(super) fn advanced_filter(&mut self, r: RangeRef, a: &Args) -> R<Value> {
        let s = self.check_sheet(r.sheet)?;
        let copy = match self.arg_i64(a, 0, "action")?.ok_or_else(|| Error::new(449, "Argument not optional"))? {
            1 => false,
            2 => true,
            _ => return Err(Error::invalid_call()),
        };
        let list = if r.range.is_single() { self.current_region(s, r.range) } else { r.range };
        let list = self.wb.clamp_to_used(s, &list).unwrap_or(list);
        let (cs, crit) = match a.get(1, "criteriarange") {
            Some(v) => {
                let c = self.range_arg(v, s)?;
                let cs = self.check_sheet(c.sheet)?;
                (cs, self.wb.clamp_to_used(cs, &c.range).unwrap_or(c.range))
            }
            // No criteria: every row passes (a header row alone lets all through).
            None => (s, Range::new(list.start, CellRef::new(list.start.row, list.end.col))),
        };
        let to = match a.get(2, "copytorange") {
            Some(v) if copy => {
                let d = self.range_arg(v, s)?;
                Some((self.check_sheet(d.sheet)?, d.range.start))
            }
            _ if copy => return Err(Error::app("The extract range is missing")),
            _ => None,
        };
        let unique = self.flag(a, 3, "unique")?;
        match self.via(|wb| wb.advanced_filter(s, &list, cs, &crit, unique, to))? {
            Ok(_) => {
                if !copy && !self.filtered.contains(&(s, list)) {
                    self.filtered.push((s, list));
                }
                Ok(Value::Empty)
            }
            Err(AdvancedFilterError::List) => Err(Error::app("The list range needs a header row and data")),
            Err(AdvancedFilterError::Criteria) => Err(Error::app("The criteria range is not valid")),
        }
    }

    /// Worksheet.ShowAllData: clears the AutoFilter's criteria and shows the
    /// rows an in-place AdvancedFilter hid.
    pub(super) fn show_all_data(&mut self, s: usize) -> R<Value> {
        let auto = self.wb.sheets[s].filter.clone().filter(|f| !f.cols.is_empty());
        let mine: Vec<Range> = self.filtered.iter().filter(|(fs, _)| *fs == s).map(|(_, l)| *l).collect();
        if auto.is_none() && mine.is_empty() {
            return Err(Error::app("ShowAllData method of Worksheet class failed"));
        }
        if let Some(mut f) = auto {
            f.cols.clear();
            self.via(|wb| wb.set_filter(s, Some(f), "undo.filter"))?;
        }
        for list in mine {
            let sh = &self.wb.sheets[s];
            let sizes: Vec<(u32, Option<f32>)> = sh.row_heights.range(list.start.row + 1..=list.end.row).filter(|(_, h)| **h == 0.0).map(|(r, _)| (*r, None)).collect();
            if !sizes.is_empty() {
                self.apply(Op::RowHeights { sheet: s, sizes })?;
            }
        }
        self.filtered.retain(|(fs, _)| *fs != s);
        Ok(Value::Empty)
    }
}
