//! Tables from macros: Worksheet.ListObjects and a ListObject's columns
//! and rows. A table is known by its id, which stays when it is renamed
//! or moved.

use apex_sheet::sheet::Content;
use apex_sheet::table::{DEFAULT_STYLE, Table, TableColumn, TotalFn};
use apex_sheet::workbook::{TableError, TableId};
use apex_sheet::{Cell, CellRef, MAX_COLS, MAX_ROWS, Op, Range};

use super::{Args, Interp, obj, range_obj};
use crate::error::{Error, R};
use crate::object::{Obj, Part, RangeRef};
use crate::value::Value;

fn part(p: Part) -> Value {
    obj(Obj::Part(Box::new(p)))
}

fn table_error(e: TableError) -> Error {
    Error::app(match e {
        TableError::Overlap => "A table can't overlap another table",
        TableError::Merged => "A table can't hold merged cells",
        TableError::BadName => "The name that you entered is not valid",
        TableError::NameTaken => "The name is already in use",
        TableError::BadRange => "The range isn't valid for a table",
        TableError::NotFound => "The table no longer exists",
    })
}

/// XlTotalsCalculation, in order.
const TOTALS: [TotalFn; 10] = [TotalFn::None, TotalFn::Sum, TotalFn::Average, TotalFn::Count, TotalFn::CountNums, TotalFn::Min, TotalFn::Max, TotalFn::StdDev, TotalFn::Var, TotalFn::Custom];

impl Interp<'_> {
    /// Where the table `id` is now.
    fn table_id(&self, id: u32) -> R<(TableId, Table)> {
        self.wb.tables().enumerate().find(|(_, (_, t))| t.id == id).map(|(_, (s, t))| ((s, self.wb.sheets[s].tables.iter().position(|x| x.id == id).unwrap_or(0)), t.clone())).ok_or_else(|| table_error(TableError::NotFound))
    }

    fn set_table(&mut self, id: TableId, t: Table) -> R<()> {
        self.via(|wb| wb.set_table(id, t, "undo.table"))
    }

    pub(super) fn tables_member(&mut self, s: usize, name: &str, a: &Args) -> R<Value> {
        let list: Vec<u32> = self.wb.sheets.get(s).ok_or_else(Error::subscript)?.tables.iter().map(|t| t.id).collect();
        match name {
            "count" => Ok(Value::Long(list.len() as i32)),
            "item" | "_default" => {
                let v = a.get(0, "index").ok_or_else(Error::invalid_call)?;
                let id = match self.scalar(v)? {
                    Value::Str(n) => self.wb.sheets[s].tables.iter().find(|t| t.name.eq_ignore_ascii_case(&n)).map(|t| t.id),
                    v => usize::try_from(v.to_i64()? - 1).ok().and_then(|i| list.get(i)).copied(),
                };
                Ok(part(Part::ListObject(id.ok_or_else(Error::subscript)?)))
            }
            "add" => {
                if self.arg_i64(a, 0, "sourcetype")?.is_some_and(|t| t != 1) {
                    return Err(Error::unsupported("tables of external data (only xlSrcRange)"));
                }
                let r = match a.get(1, "source") {
                    Some(Value::Object(Obj::Range(r))) => r,
                    Some(Value::Str(t)) => match self.range_call(s, &Args(vec![(None, Value::Str(t))]))? {
                        Value::Object(Obj::Range(r)) => r,
                        _ => return Err(Error::type_mismatch()),
                    },
                    Some(_) => return Err(Error::type_mismatch()),
                    // The cells around the active cell.
                    None => RangeRef::new(s, self.current_region(s, Range::single(self.selection.start))),
                };
                if r.sheet != s {
                    return Err(Error::app("The range must be on the same sheet"));
                }
                let header = match self.arg_i64(a, 3, "xllistobjecthasheaders")?.unwrap_or(0) {
                    1 => true,
                    2 => false,
                    // xlGuess: a first row of text over other rows.
                    _ => r.range.rows() > 1 && (r.range.start.col..=r.range.end.col).all(|c| matches!(self.wb.sheets[s].value(CellRef::new(r.range.start.row, c)), apex_sheet::Value::Text(_))),
                };
                let style = self.arg_str(a, 5, "tablestylename")?.unwrap_or_else(|| DEFAULT_STYLE.to_string());
                let (sheet, i) = self.via(|wb| wb.create_table(s, r.range, header, &style))?.map_err(table_error)?;
                Ok(part(Part::ListObject(self.wb.sheets[sheet].tables[i].id)))
            }
            "parent" => Ok(obj(Obj::Sheet(s))),
            _ => Err(Error::no_member(name)),
        }
    }

    pub(super) fn table_member(&mut self, id: u32, name: &str, a: &Args) -> R<Value> {
        let ((s, i), t) = self.table_id(id)?;
        Ok(match name {
            "name" | "displayname" | "_default" => Value::str(t.name.clone()),
            "range" => range_obj(s, t.range),
            "databodyrange" => {
                let (a, b) = t.data_rows();
                if b < a { Value::Nothing } else { range_obj(s, t.data_range()) }
            }
            "headerrowrange" => match t.header_row() {
                Some(r) => range_obj(s, Range::new(CellRef::new(r, t.range.start.col), CellRef::new(r, t.range.end.col))),
                None => Value::Nothing,
            },
            "totalsrowrange" => match t.totals_row() {
                Some(r) => range_obj(s, Range::new(CellRef::new(r, t.range.start.col), CellRef::new(r, t.range.end.col))),
                None => Value::Nothing,
            },
            "showtotals" => Value::Bool(t.totals),
            "showheaders" => Value::Bool(t.header),
            "showautofilter" => Value::Bool(t.filter_button),
            "tablestyle" => Value::str(t.style.name.clone()),
            "showtablestylerowstripes" => Value::Bool(t.style.row_stripes),
            "showtablestylecolumnstripes" => Value::Bool(t.style.col_stripes),
            "showtablestylefirstcolumn" => Value::Bool(t.style.first_col),
            "showtablestylelastcolumn" => Value::Bool(t.style.last_col),
            "listcolumns" => return self.part_get(Part::ListColumns(id), a),
            "listrows" => return self.part_get(Part::ListRows(id), a),
            "index" => Value::Long(i as i32 + 1),
            "parent" => obj(Obj::Sheet(s)),
            "resize" => {
                let r = match a.get(0, "range") {
                    Some(Value::Object(Obj::Range(r))) if r.sheet == s => r,
                    Some(_) => return Err(Error::type_mismatch()),
                    None => return Err(Error::invalid_call()),
                };
                self.via(|wb| wb.resize_table((s, i), r.range))?.map_err(table_error)?;
                Value::Empty
            }
            "unlist" => {
                self.via(|wb| wb.convert_table_to_range((s, i)))?;
                Value::Empty
            }
            "delete" => {
                self.via(|wb| {
                    wb.convert_table_to_range((s, i));
                    wb.clear_all(s, &t.range);
                })?;
                Value::Empty
            }
            _ => return Err(Error::no_member(name)),
        })
    }

    pub(super) fn table_set(&mut self, id: u32, name: &str, v: Value) -> R<()> {
        let ((s, i), mut t) = self.table_id(id)?;
        match name {
            "name" | "displayname" => {
                let n = v.to_str()?;
                return self.via(|wb| wb.rename_table((s, i), &n))?.map_err(table_error);
            }
            "showtotals" => {
                let on = v.to_bool()?;
                if on && t.range.end.row + 1 >= MAX_ROWS {
                    return Err(table_error(TableError::BadRange));
                }
                return self.via(|wb| wb.set_table_totals((s, i), on));
            }
            "tablestyle" => {
                let n = v.to_str()?;
                if !(n.is_empty() || n.starts_with("TableStyle")) {
                    return Err(Error::app(format!("There is no table style {n}")));
                }
                t.style.name = n;
            }
            "showtablestylerowstripes" => t.style.row_stripes = v.to_bool()?,
            "showtablestylecolumnstripes" => t.style.col_stripes = v.to_bool()?,
            "showtablestylefirstcolumn" => t.style.first_col = v.to_bool()?,
            "showtablestylelastcolumn" => t.style.last_col = v.to_bool()?,
            "showautofilter" => t.filter_button = v.to_bool()?,
            _ => return Err(Error::no_member(name)),
        }
        self.set_table((s, i), t)
    }

    // --- columns ---------------------------------------------------------------

    pub(super) fn columns_member(&mut self, id: u32, name: &str, a: &Args) -> R<Value> {
        let ((s, i), t) = self.table_id(id)?;
        match name {
            "count" => Ok(Value::Long(t.columns.len() as i32)),
            "item" | "_default" => {
                let v = a.get(0, "index").ok_or_else(Error::invalid_call)?;
                let k = match self.scalar(v)? {
                    Value::Str(n) => t.column_index(&n),
                    v => usize::try_from(v.to_i64()? - 1).ok().filter(|k| *k < t.columns.len()),
                };
                Ok(part(Part::ListColumn(id, k.ok_or_else(Error::subscript)?)))
            }
            "add" => {
                let pos = match self.arg_i64(a, 0, "position")? {
                    None => t.columns.len(),
                    Some(p) => usize::try_from(p - 1).ok().filter(|p| *p <= t.columns.len()).ok_or_else(Error::invalid_call)?,
                };
                let col = t.range.start.col + pos as u32;
                let sh = &self.wb.sheets[s];
                let rows = Range::new(CellRef::new(t.range.start.row, col), CellRef::new(t.range.end.row, col));
                // Cells pushed off the sheet would be lost.
                let edge = Range::new(CellRef::new(t.range.start.row, MAX_COLS - 1), CellRef::new(t.range.end.row, MAX_COLS - 1));
                let mut full = false;
                sh.for_each_in(&edge, |_, _| full = true);
                if full || t.range.end.col + 1 >= MAX_COLS || sh.tables.iter().any(|x| x.id != id && x.range.start.col >= col && x.range.start.row <= t.range.end.row && x.range.end.row >= t.range.start.row) {
                    return Err(Error::app("This would move cells of another table or off the sheet"));
                }
                let mut n = 1;
                while t.column_index(&format!("Column{n}")).is_some() {
                    n += 1;
                }
                let head = format!("Column{n}");
                let mut nt = t.clone();
                nt.range.end.col += 1;
                nt.columns.insert(pos, TableColumn::new(head.clone()));
                self.via(|wb| wb.insert_cells(s, &rows, false))?;
                self.set_table((s, i), nt)?;
                if let Some(r) = t.header_row() {
                    let at = CellRef::new(r, col);
                    let cell = Cell::with_content(self.wb.sheets[s].get(at), Content::Value(apex_sheet::Value::text(head.as_str())));
                    self.apply(Op::SetCells { sheet: s, cells: vec![(at, cell)] })?;
                }
                Ok(part(Part::ListColumn(id, pos)))
            }
            "parent" => Ok(part(Part::ListObject(id))),
            _ => Err(Error::no_member(name)),
        }
    }

    pub(super) fn column_member(&mut self, id: u32, k: usize, name: &str) -> R<Value> {
        let ((s, i), t) = self.table_id(id)?;
        let c = t.columns.get(k).ok_or_else(|| Error::app("The column no longer exists"))?;
        let col = t.col_at(k);
        Ok(match name {
            "name" | "_default" => Value::str(c.name.clone()),
            "index" => Value::Long(k as i32 + 1),
            "range" => range_obj(s, Range::new(CellRef::new(t.range.start.row, col), CellRef::new(t.range.end.row, col))),
            "databodyrange" => {
                let (a, b) = t.data_rows();
                if b < a { Value::Nothing } else { range_obj(s, Range::new(CellRef::new(a, col), CellRef::new(b, col))) }
            }
            "totalscalculation" => Value::Long(TOTALS.iter().position(|f| *f == c.total).unwrap_or(0) as i32),
            "parent" => part(Part::ListObject(id)),
            "delete" => {
                if t.columns.len() == 1 {
                    return Err(Error::app("A table keeps at least one column"));
                }
                let rows = Range::new(CellRef::new(t.range.start.row, col), CellRef::new(t.range.end.row, col));
                let mut nt = t.clone();
                nt.range.end.col -= 1;
                nt.columns.remove(k);
                self.via(|wb| wb.delete_cells(s, &rows, false))?;
                self.set_table((s, i), nt)?;
                Value::Empty
            }
            _ => return Err(Error::no_member(name)),
        })
    }

    pub(super) fn column_set(&mut self, id: u32, k: usize, name: &str, v: Value) -> R<()> {
        let ((s, i), t) = self.table_id(id)?;
        if k >= t.columns.len() {
            return Err(Error::app("The column no longer exists"));
        }
        match name {
            "name" => {
                let n = v.to_str()?;
                if n.trim().is_empty() || t.columns.iter().enumerate().any(|(j, c)| j != k && c.name.eq_ignore_ascii_case(&n)) {
                    return Err(Error::app(format!("The column name {n} is empty or already used")));
                }
                match t.header_row() {
                    // Typed in its header, as in the sheet.
                    Some(r) => {
                        let at = CellRef::new(r, t.col_at(k));
                        self.set_default(range_obj(s, Range::single(at)), Value::str(n))?;
                        self.via(|wb| wb.table_after_input(s, at)).map(|_| ())
                    }
                    None => {
                        let mut nt = t.clone();
                        nt.columns[k].name = n;
                        self.set_table((s, i), nt)
                    }
                }
            }
            "totalscalculation" => {
                let f = *usize::try_from(v.to_i64()?).ok().and_then(|x| TOTALS.get(x)).filter(|f| **f != TotalFn::Custom).ok_or_else(Error::invalid_call)?;
                if !t.totals {
                    return Err(Error::app("The table has no totals row (ShowTotals)"));
                }
                self.via(|wb| wb.set_total_fn((s, i), k, f))
            }
            _ => Err(Error::no_member(name)),
        }
    }

    // --- rows ------------------------------------------------------------------

    pub(super) fn rows_member(&mut self, id: u32, name: &str, a: &Args) -> R<Value> {
        let ((s, i), t) = self.table_id(id)?;
        let (first, last) = t.data_rows();
        let count = if last < first { 0 } else { (last - first + 1) as usize };
        match name {
            "count" => Ok(Value::Long(count as i32)),
            "item" | "_default" => {
                let k = self.arg_i64(a, 0, "index")?.ok_or_else(Error::invalid_call)?;
                let k = usize::try_from(k - 1).ok().filter(|k| *k < count).ok_or_else(Error::subscript)?;
                Ok(part(Part::ListRow(id, k)))
            }
            "add" => {
                let pos = match self.arg_i64(a, 0, "position")? {
                    None => count,
                    Some(p) => usize::try_from(p - 1).ok().filter(|p| *p <= count).ok_or_else(Error::invalid_call)?,
                };
                let row = first + pos as u32;
                let sh = &self.wb.sheets[s];
                let edge = Range::new(CellRef::new(MAX_ROWS - 1, t.range.start.col), CellRef::new(MAX_ROWS - 1, t.range.end.col));
                let mut full = false;
                sh.for_each_in(&edge, |_, _| full = true);
                if full || t.range.end.row + 1 >= MAX_ROWS || sh.tables.iter().any(|x| x.id != id && x.range.start.row >= row && x.range.start.col <= t.range.end.col && x.range.end.col >= t.range.start.col) {
                    return Err(Error::app("This would move cells of another table or off the sheet"));
                }
                let cols = Range::new(CellRef::new(row, t.range.start.col), CellRef::new(row, t.range.end.col));
                let mut nt = t.clone();
                nt.range.end.row += 1;
                self.via(|wb| wb.insert_cells(s, &cols, true))?;
                self.set_table((s, i), nt)?;
                // Calculated columns fill the new row.
                let mut cells = Vec::new();
                for (k, c) in t.columns.iter().enumerate() {
                    let Some(f) = &c.formula else { continue };
                    let Ok(mut e) = apex_sheet::formula::parse_canonical(f) else { continue };
                    e.offset(row as i64 - first as i64, 0);
                    let at = CellRef::new(row, t.col_at(k));
                    cells.push((at, Cell::with_content(self.wb.sheets[s].get(at), Content::Formula { expr: e.into(), value: apex_sheet::Value::Empty })));
                }
                if !cells.is_empty() {
                    self.apply(Op::SetCells { sheet: s, cells })?;
                }
                Ok(part(Part::ListRow(id, pos)))
            }
            "parent" => Ok(part(Part::ListObject(id))),
            _ => Err(Error::no_member(name)),
        }
    }

    pub(super) fn row_member(&mut self, id: u32, k: usize, name: &str) -> R<Value> {
        let ((s, i), t) = self.table_id(id)?;
        let (first, last) = t.data_rows();
        let row = first + k as u32;
        if last < first || row > last {
            return Err(Error::app("The row no longer exists"));
        }
        let cells = Range::new(CellRef::new(row, t.range.start.col), CellRef::new(row, t.range.end.col));
        Ok(match name {
            "range" | "_default" => range_obj(s, cells),
            "index" => Value::Long(k as i32 + 1),
            "parent" => part(Part::ListObject(id)),
            "delete" => {
                if first == last {
                    // A table keeps one data row: it is emptied.
                    self.via(|wb| wb.clear_all(s, &cells))?;
                } else {
                    let mut nt = t.clone();
                    nt.range.end.row -= 1;
                    self.via(|wb| wb.delete_cells(s, &cells, true))?;
                    self.set_table((s, i), nt)?;
                }
                Value::Empty
            }
            _ => return Err(Error::no_member(name)),
        })
    }
}
