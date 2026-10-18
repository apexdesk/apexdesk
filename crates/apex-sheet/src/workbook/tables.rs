//! Tables on a workbook: creating, resizing and converting them, the
//! totals row, calculated columns, automatic expansion while typing, and
//! structured references (resolving, rewriting, exporting).

use super::*;
use crate::formula::{CellAddr, RangeKind};
use crate::sheet::{Line, LineStyle};
use crate::table::{self, CellLook, StructRef, Table, TableColumn, TotalFn};

/// Why a table command could not run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableError {
    /// The range overlaps another table.
    Overlap,
    /// The range holds merged cells.
    Merged,
    /// Not a valid table name.
    BadName,
    /// The name is used by another table or a sheet.
    NameTaken,
    /// The range must keep the header row and at least one data row, and
    /// the same header row.
    BadRange,
    NotFound,
}

/// Where a changed table sits: (sheet, index in `Sheet::tables`).
pub type TableId = (usize, usize);

impl Workbook {
    // --- lookup ------------------------------------------------------------

    /// All tables, with their sheets.
    pub fn tables(&self) -> impl Iterator<Item = (usize, &Table)> {
        self.sheets.iter().enumerate().flat_map(|(i, s)| s.tables.iter().map(move |t| (i, t)))
    }

    /// The table named `name` (case-insensitive).
    pub fn find_table(&self, name: &str) -> Option<TableId> {
        let low = name.to_lowercase();
        self.sheets.iter().enumerate().find_map(|(si, s)| s.tables.iter().position(|t| t.name.to_lowercase() == low).map(|i| (si, i)))
    }

    /// The table holding cell `c`.
    pub fn table_at(&self, sheet: usize, c: CellRef) -> Option<TableId> {
        let i = self.sheets.get(sheet)?.tables.iter().position(|t| t.range.contains(c))?;
        Some((sheet, i))
    }

    pub fn table(&self, id: TableId) -> Option<&Table> {
        self.sheets.get(id.0)?.tables.get(id.1)
    }

    /// What a structured reference written in `cell` stands for.
    pub(crate) fn table_range(&self, sheet: usize, cell: CellRef, r: &StructRef) -> Option<(usize, Range)> {
        let (si, t) = match &r.table {
            Some(n) => {
                let (si, i) = self.find_table(n)?;
                (si, &self.sheets[si].tables[i])
            }
            None => (sheet, self.sheets.get(sheet)?.table_at(cell)?),
        };
        // `[@Col]` reads the formula's own row.
        if r.items & table::items::THIS_ROW != 0 && si != sheet {
            return None;
        }
        t.resolve(r, cell).ok().map(|range| (si, range))
    }

    /// A table name that is free: `Table1`, `Table2`...
    pub fn unique_table_name(&self, base: &str) -> String {
        (1..).map(|i| format!("{base}{i}")).find(|n| self.table_name_free(n, None)).unwrap_or_else(|| base.to_string())
    }

    /// Whether `name` can name a table (`except`: the table being renamed).
    pub fn table_name_free(&self, name: &str, except: Option<TableId>) -> bool {
        self.find_table(name).is_none_or(|id| Some(id) == except) && self.sheet_index(name).is_none()
    }

    pub(crate) fn next_table_id(&self) -> u32 {
        self.tables().map(|(_, t)| t.id).max().unwrap_or(0) + 1
    }

    /// The table style's look for a cell (None outside tables).
    pub fn table_look(&self, sheet: usize, c: CellRef) -> Option<CellLook> {
        self.sheets.get(sheet)?.table_at(c)?.look(c)
    }

    // --- editing -------------------------------------------------------------

    fn header_text(&self, sheet: usize, c: CellRef) -> String {
        self.display(sheet, c).0.trim().to_string()
    }

    /// Makes `names` unique and non-empty (as Excel does: `Column1`, `Sales2`).
    pub(crate) fn unique_names(names: Vec<String>) -> Vec<String> {
        let mut out: Vec<String> = Vec::with_capacity(names.len());
        for (i, n) in names.into_iter().enumerate() {
            let base = if n.is_empty() { format!("Column{}", i + 1) } else { n };
            let taken = |x: &str, out: &Vec<String>| out.iter().any(|o| o.to_lowercase() == x.to_lowercase());
            let mut name = base.clone();
            let mut k = 2;
            while taken(&name, &out) {
                name = format!("{base}{k}");
                k += 1;
            }
            out.push(name);
        }
        out
    }

    fn check_area(&self, sheet: usize, r: &Range, except: Option<usize>) -> Result<(), TableError> {
        let sh = self.sheets.get(sheet).ok_or(TableError::NotFound)?;
        if sh.tables.iter().enumerate().any(|(i, t)| Some(i) != except && t.range.intersect(r).is_some()) {
            return Err(TableError::Overlap);
        }
        if sh.merges.iter().any(|m| m.intersect(r).is_some()) {
            return Err(TableError::Merged);
        }
        Ok(())
    }

    /// Formats `r` as a table (one undo step). Without a header row, one is
    /// inserted above the data (cells shift down). Returns the new table.
    pub fn create_table(&mut self, sheet: usize, r: Range, has_header: bool, style: &str) -> Result<TableId, TableError> {
        let r = Range::new(r.start, r.end);
        if (has_header && r.rows() < 2) || r.end.row + 1 >= MAX_ROWS {
            return Err(TableError::BadRange);
        }
        self.check_area(sheet, &r, None)?;
        let before = self.undo.len();
        let range = if has_header {
            r
        } else {
            // Room for the headers: shift the data down one row.
            self.insert_cells(sheet, &Range::new(r.start, CellRef::new(r.start.row, r.end.col)), true);
            Range::new(r.start, CellRef::new(r.end.row + 1, r.end.col))
        };
        let head: Vec<String> = if has_header { (r.start.col..=r.end.col).map(|c| self.header_text(sheet, CellRef::new(r.start.row, c))).collect() } else { vec![String::new(); r.cols() as usize] };
        let names = Self::unique_names(head);
        let mut cells = Vec::new();
        for (i, n) in names.iter().enumerate() {
            let c = CellRef::new(range.start.row, range.start.col + i as u32);
            if self.header_text(sheet, c) != *n || !matches!(self.sheets[sheet].value(c), Value::Text(_)) {
                cells.push((c, Cell::with_content(self.sheets[sheet].get(c), Content::Value(Value::text(n.as_str())))));
            }
        }
        let name = self.unique_table_name("Table");
        let mut t = Table::new(self.next_table_id(), name, range, names);
        t.style.name = style.to_string();
        let mut list = self.sheets[sheet].tables.clone();
        list.push(t.clone());
        let mut ops = vec![Op::Tables { sheet, list }];
        if !cells.is_empty() {
            ops.push(Op::SetCells { sheet, cells });
        }
        self.execute_as(Op::Batch(ops), "undo.table");
        let id = (sheet, self.sheets[sheet].tables.len().saturating_sub(1));
        // The filter buttons (only when the sheet has no other filter).
        if self.sheets[sheet].filter.is_none() {
            self.set_filter(sheet, Some(AutoFilter::new(t.filter_range())), "undo.table");
        }
        let n = self.undo.len().saturating_sub(before);
        self.merge_undo(n, "undo.table");
        Ok(id)
    }

    /// Replaces a table's settings (style, options, name without formula
    /// rewriting): one undo step named `label`.
    pub fn set_table(&mut self, id: TableId, t: Table, label: &'static str) {
        let Some(sh) = self.sheets.get(id.0) else { return };
        if id.1 >= sh.tables.len() {
            return;
        }
        let mut list = sh.tables.clone();
        let old = std::mem::replace(&mut list[id.1], t.clone());
        let mut ops = vec![Op::Tables { sheet: id.0, list }];
        // Filter buttons on or off.
        let f = &sh.filter;
        let ours = f.as_ref().is_some_and(|f| f.range == old.filter_range());
        if old.filter_button && !t.filter_button && ours {
            ops.push(Op::Filter { sheet: id.0, filter: None });
            ops.extend(self.show_filtered(id.0, &old.filter_range()));
        } else if t.filter_button && (f.is_none() || ours) && f.as_ref().is_none_or(|f| f.range != t.filter_range()) {
            let mut nf = f.clone().unwrap_or_else(|| AutoFilter::new(t.filter_range()));
            nf.range = t.filter_range();
            ops.push(Op::Filter { sheet: id.0, filter: Some(nf) });
        }
        self.execute_as(Op::Batch(ops), label);
    }

    /// Row heights that show the rows a filter hid in `r`.
    fn show_filtered(&self, sheet: usize, r: &Range) -> Option<Op> {
        let sizes: Vec<(u32, Option<f32>)> = self.sheets[sheet].row_heights.iter().filter(|(row, h)| **h == 0.0 && **row > r.start.row && **row <= r.end.row).map(|(row, _)| (*row, None)).collect();
        (!sizes.is_empty()).then_some(Op::RowHeights { sheet, sizes })
    }

    /// Formula cells rewritten by `f` (which returns true when it changed
    /// the expression), as undoable ops.
    fn rewrite_formulas(&self, mut f: impl FnMut(usize, CellRef, &mut Expr) -> bool) -> Vec<Op> {
        let mut ops = Vec::new();
        for (si, sh) in self.sheets.iter().enumerate() {
            let mut cells = Vec::new();
            for (c, cell) in sh.iter() {
                if let Some(e) = cell.formula() {
                    let mut e2 = e.expr().into_owned();
                    if f(si, c, &mut e2) {
                        let mut nc = cell.clone();
                        nc.content = Content::Formula { expr: e2.into(), value: Value::Empty };
                        cells.push((c, Some(nc)));
                    }
                }
            }
            if !cells.is_empty() {
                ops.push(Op::SetCells { sheet: si, cells });
            }
        }
        ops
    }

    /// Renames a table and the references to it.
    pub fn rename_table(&mut self, id: TableId, name: &str) -> Result<(), TableError> {
        let name = name.trim();
        let t = self.table(id).ok_or(TableError::NotFound)?.clone();
        if !table::valid_name(name) {
            return Err(TableError::BadName);
        }
        if !self.table_name_free(name, Some(id)) {
            return Err(TableError::NameTaken);
        }
        if t.name == name {
            return Ok(());
        }
        let old = t.name.to_lowercase();
        let new: Arc<str> = Arc::from(name);
        let mut ops = self.rewrite_formulas(|_, _, e| {
            let mut changed = false;
            e.visit_structured_mut(&mut |x| {
                if let Expr::Structured(r) = x {
                    if r.table.as_deref().is_some_and(|n| n.to_lowercase() == old) {
                        r.table = Some(new.clone());
                        changed = true;
                    }
                }
            });
            changed
        });
        let mut list = self.sheets[id.0].tables.clone();
        list[id.1].name = name.to_string();
        ops.insert(0, Op::Tables { sheet: id.0, list });
        // The data model follows (relationships, measures, pivot tables).
        ops.extend(self.model_rename_op(&t.name, name));
        // Get Data queries loaded into it follow the new name.
        if self.queries.iter().any(|q| q.table.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(&t.name))) {
            let mut qs = self.queries.clone();
            for q in qs.iter_mut().filter(|q| q.table.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(&t.name))) {
                q.table = Some(name.to_string());
            }
            ops.push(Op::Queries(qs));
        }
        self.execute_as(Op::Batch(ops), "undo.table");
        Ok(())
    }

    /// Ops that rename column `from` of table `t` to `to` in every formula.
    fn rename_column_refs(&self, sheet: usize, t: &Table, from: &str, to: &str) -> Vec<Op> {
        let (tname, from_l) = (t.name.to_lowercase(), from.to_lowercase());
        let to: Arc<str> = Arc::from(to);
        let range = t.range;
        self.rewrite_formulas(|si, c, e| {
            let mut changed = false;
            e.visit_structured_mut(&mut |x| {
                let Expr::Structured(r) = x else { return };
                let ours = match &r.table {
                    Some(n) => n.to_lowercase() == tname,
                    None => si == sheet && range.contains(c),
                };
                if !ours {
                    return;
                }
                if let Some((a, b)) = &mut r.cols {
                    if a.to_lowercase() == from_l {
                        *a = to.clone();
                        changed = true;
                    }
                    if let Some(b) = b {
                        if b.to_lowercase() == from_l {
                            *b = to.clone();
                            changed = true;
                        }
                    }
                }
            });
            changed
        })
    }

    /// Changes the cells covered by a table (keeps the header row and the
    /// columns' settings by position).
    pub fn resize_table(&mut self, id: TableId, r: Range) -> Result<(), TableError> {
        let t = self.table(id).ok_or(TableError::NotFound)?.clone();
        let r = Range::new(r.start, r.end);
        if r.start.row != t.range.start.row || r.rows() < 1 + t.header as u32 + t.totals as u32 {
            return Err(TableError::BadRange);
        }
        self.check_area(id.0, &r, Some(id.1))?;
        let mut nt = t.clone();
        nt.range = r;
        let head: Vec<String> = (r.start.col..=r.end.col)
            .map(|c| {
                // Columns that stay keep their names.
                let i = c as i64 - t.range.start.col as i64;
                if i >= 0 && (i as usize) < t.columns.len() { t.columns[i as usize].name.clone() } else if t.header { self.header_text(id.0, CellRef::new(r.start.row, c)) } else { String::new() }
            })
            .collect();
        let names = Self::unique_names(head);
        nt.columns = names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let old = (r.start.col + i as u32).checked_sub(t.range.start.col).and_then(|k| t.columns.get(k as usize));
                old.cloned().unwrap_or_else(|| TableColumn::new(n.clone()))
            })
            .collect();
        for (c, n) in nt.columns.iter_mut().zip(names) {
            c.name = n;
        }
        let mut cells = Vec::new();
        if nt.header {
            for (i, c) in nt.columns.iter().enumerate() {
                let at = CellRef::new(r.start.row, nt.col_at(i));
                if self.header_text(id.0, at) != c.name {
                    cells.push((at, Cell::with_content(self.sheets[id.0].get(at), Content::Value(Value::text(c.name.as_str())))));
                }
            }
        }
        let mut list = self.sheets[id.0].tables.clone();
        list[id.1] = nt.clone();
        let mut ops = vec![Op::Tables { sheet: id.0, list }];
        if !cells.is_empty() {
            ops.push(Op::SetCells { sheet: id.0, cells });
        }
        if let Some(f) = &self.sheets[id.0].filter {
            if f.range == t.filter_range() {
                let mut nf = f.clone();
                nf.range = nt.filter_range();
                nf.cols.retain(|c| nf.range.start.col <= c.col && c.col <= nf.range.end.col);
                ops.push(Op::Filter { sheet: id.0, filter: Some(nf) });
            }
        }
        self.execute_as(Op::Batch(ops), "undo.table_resize");
        Ok(())
    }

    /// The canonical formula of a column's cell in the totals row.
    fn total_formula(c: &TableColumn) -> Option<String> {
        match c.total {
            TotalFn::Custom => c.total_formula.clone(),
            f => {
                let n = f.subtotal()?;
                let mut s = format!("SUBTOTAL({n},");
                StructRef { table: None, items: 0, cols: Some((Arc::from(c.name.as_str()), None)) }.write(&mut s, false, None);
                s.push(')');
                Some(s)
            }
        }
    }

    fn total_cell(&self, sheet: usize, at: CellRef, c: &TableColumn) -> Option<Cell> {
        let old = self.sheets[sheet].get(at);
        let content = match (Self::total_formula(c), &c.total_label) {
            (Some(f), _) => match formula::parse_canonical(&f) {
                Ok(e) => Content::Formula { expr: e.into(), value: Value::Empty },
                Err(_) => Content::Empty,
            },
            (None, Some(l)) => Content::Value(Value::text(l.as_str())),
            (None, None) => Content::Empty,
        };
        Cell::with_content(old, content)
    }

    /// Shows or hides the totals row.
    pub fn set_table_totals(&mut self, id: TableId, on: bool) {
        let Some(t) = self.table(id).cloned() else { return };
        if t.totals == on {
            return;
        }
        let sheet = id.0;
        let before = self.undo.len();
        let mut nt = t.clone();
        if on {
            let below = t.range.end.row + 1;
            if below >= MAX_ROWS {
                return;
            }
            let band = Range::new(CellRef::new(below, t.range.start.col), CellRef::new(below, t.range.end.col));
            let busy = self.sheets[sheet].tables.iter().any(|o| o.range.intersect(&band).is_some()) || self.clamp_to_used(sheet, &band).is_some_and(|r| {
                let mut any = false;
                self.sheets[sheet].for_each_in(&r, |_, c| any |= !matches!(c.content, Content::Empty));
                any
            });
            if busy {
                self.insert_cells(sheet, &band, true);
            }
            nt.totals = true;
            nt.range.end.row = below;
            // Excel's defaults: a label first, a sum (or count) last.
            if nt.columns.iter().all(|c| c.total == TotalFn::None && c.total_label.is_none()) {
                let last = nt.columns.len() - 1;
                let (d0, d1) = t.data_rows();
                let numeric = (d0..=d1).any(|r| matches!(self.sheets[sheet].value(CellRef::new(r, t.col_at(last))), Value::Number(_)));
                if last > 0 {
                    nt.columns[0].total_label = Some("Total".into());
                }
                nt.columns[last].total = if numeric { TotalFn::Sum } else { TotalFn::Count };
            }
            let cells: Vec<(CellRef, Option<Cell>)> = nt.columns.iter().enumerate().map(|(i, c)| {
                let at = CellRef::new(below, nt.col_at(i));
                (at, self.total_cell(sheet, at, c))
            }).collect();
            let mut list = self.sheets[sheet].tables.clone();
            list[id.1] = nt;
            self.execute_as(Op::Batch(vec![Op::Tables { sheet, list }, Op::SetCells { sheet, cells }]), "undo.table_totals");
        } else {
            let row = t.range.end.row;
            let cells: Vec<(CellRef, Option<Cell>)> = (t.range.start.col..=t.range.end.col).map(|c| (CellRef::new(row, c), Cell::with_content(self.sheets[sheet].get(CellRef::new(row, c)), Content::Empty))).collect();
            nt.totals = false;
            nt.range.end.row -= 1;
            let mut list = self.sheets[sheet].tables.clone();
            list[id.1] = nt;
            self.execute_as(Op::Batch(vec![Op::Tables { sheet, list }, Op::SetCells { sheet, cells }]), "undo.table_totals");
        }
        let n = self.undo.len().saturating_sub(before);
        self.merge_undo(n, "undo.table_totals");
    }

    /// Sets the totals-row function of column `col` (index in the table).
    pub fn set_total_fn(&mut self, id: TableId, col: usize, f: TotalFn) {
        let Some(t) = self.table(id).cloned() else { return };
        let Some(row) = t.totals_row() else { return };
        if col >= t.columns.len() {
            return;
        }
        let mut nt = t.clone();
        let c = &mut nt.columns[col];
        c.total = f;
        if f != TotalFn::None {
            c.total_label = None;
        }
        if f == TotalFn::Custom && c.total_formula.is_none() {
            c.total_formula = Self::total_formula(&TableColumn { total: TotalFn::Sum, ..c.clone() });
        }
        let at = CellRef::new(row, t.col_at(col));
        let cell = self.total_cell(id.0, at, &nt.columns[col]);
        let mut list = self.sheets[id.0].tables.clone();
        list[id.1] = nt;
        self.execute_as(Op::Batch(vec![Op::Tables { sheet: id.0, list }, Op::SetCells { sheet: id.0, cells: vec![(at, cell)] }]), "undo.table_totals");
    }

    /// Structured references to ordinary references. `only`: just those to
    /// this table. Returns whether anything changed.
    fn unstructure(&self, sheet: usize, c: CellRef, e: &mut Expr, only: Option<&str>) -> bool {
        let own = self.sheets.get(sheet).and_then(|s| s.table_at(c)).map(|t| t.name.to_lowercase());
        let only = only.map(str::to_lowercase);
        self.unstructure_where(sheet, c, e, &|r| {
            let Some(o) = &only else { return true };
            let name = r.table.as_deref().map(str::to_lowercase).or_else(|| own.clone());
            name.as_deref() == Some(o)
        })
    }

    /// Like [`Self::unstructure`], for the references `pick` chooses.
    fn unstructure_where(&self, sheet: usize, c: CellRef, e: &mut Expr, pick: &dyn Fn(&StructRef) -> bool) -> bool {
        let mut changed = false;
        e.visit_structured_mut(&mut |x| {
            let Expr::Structured(r) = x else { return };
            if !pick(r) {
                return;
            }
            let this_row = r.items & table::items::THIS_ROW != 0;
            *x = match self.table_range(sheet, c, r) {
                Some((si, range)) => {
                    let sname = (si != sheet).then(|| Arc::from(self.sheets[si].name.as_str()));
                    let abs = !this_row;
                    let addr = |p: CellRef| CellAddr { row: p.row, col: p.col, row_abs: abs, col_abs: abs };
                    if range.start == range.end {
                        Expr::Ref { sheet: sname, addr: addr(range.start) }
                    } else {
                        Expr::Range { sheet: sname, a: addr(range.start), b: addr(range.end), kind: RangeKind::Cells }
                    }
                }
                None => Expr::Error(ErrorKind::Ref),
            };
            changed = true;
        });
        changed
    }

    /// The formula of a cell as ODF stores it. Tables are database ranges
    /// over the header and data there, and LibreOffice reads structured
    /// references to them (`Table1[[#This Row];[Qty]]`); those it cannot
    /// follow become ordinary references: to totals rows (not part of the
    /// range), to tables without headers (no column names) or to unknown
    /// tables.
    pub fn formula_for_odf(&self, sheet: usize, c: CellRef, e: &Expr) -> String {
        let linked;
        let e = if e.has_external() {
            linked = self.externals_by_file(e);
            &linked
        } else {
            e
        };
        if !e.has_structured() {
            return e.to_odf();
        }
        let mut e = e.clone();
        self.qualify(sheet, c, &mut e);
        self.unstructure_where(sheet, c, &mut e, &|r| {
            let Some(t) = r.table.as_deref().and_then(|n| self.find_table(n)).and_then(|id| self.table(id)) else { return true };
            let totals = r.items & (table::items::TOTALS | table::items::ALL) != 0 && t.totals;
            totals || !t.header
        });
        e.to_odf()
    }

    /// The formula of a cell as xlsx stores it: every structured reference
    /// names its table.
    pub fn formula_for_xlsx(&self, sheet: usize, c: CellRef, e: &Expr) -> String {
        if !e.has_structured() {
            return e.to_xlsx();
        }
        let mut e = e.clone();
        self.qualify(sheet, c, &mut e);
        e.to_xlsx()
    }

    fn qualify(&self, sheet: usize, c: CellRef, e: &mut Expr) {
        let own: Option<Arc<str>> = self.sheets.get(sheet).and_then(|s| s.table_at(c)).map(|t| Arc::from(t.name.as_str()));
        e.visit_structured_mut(&mut |x| {
            if let Expr::Structured(r) = x {
                if r.table.is_none() {
                    r.table = own.clone();
                }
            }
        });
    }

    /// A canonical formula written for table `t` in xlsx form (calculated
    /// column and custom total formulas).
    pub fn table_formula_for_xlsx(t: &Table, canonical: &str) -> Option<String> {
        let mut e = formula::parse_canonical(canonical).ok()?;
        let own: Arc<str> = Arc::from(t.name.as_str());
        e.visit_structured_mut(&mut |x| {
            if let Expr::Structured(r) = x {
                if r.table.is_none() {
                    r.table = Some(own.clone());
                }
            }
        });
        Some(e.to_xlsx())
    }

    /// Turns a table into ordinary cells, keeping its look as cell formats.
    pub fn convert_table_to_range(&mut self, id: TableId) {
        let Some(t) = self.table(id).cloned() else { return };
        let sheet = id.0;
        let name = t.name.clone();
        let mut ops = self.rewrite_formulas(|si, c, e| self.unstructure(si, c, e, Some(&name)));
        let baked = self.intern_baked(sheet, &t);
        let mut list = self.sheets[sheet].tables.clone();
        list.remove(id.1);
        ops.insert(0, Op::Tables { sheet, list });
        // Merge the baked formats with any rewritten formulas.
        let mut map: BTreeMap<CellRef, Option<Cell>> = BTreeMap::new();
        for (c, cell) in baked {
            map.insert(c, cell);
        }
        for op in &mut ops {
            if let Op::SetCells { sheet: s, cells } = op {
                if *s == sheet {
                    for (c, cell) in cells.iter_mut() {
                        if let (Some(b), Some(cell)) = (map.remove(c).flatten(), cell.as_mut()) {
                            cell.style = b.style;
                        }
                    }
                }
            }
        }
        if !map.is_empty() {
            ops.push(Op::SetCells { sheet, cells: map.into_iter().collect() });
        }
        if self.sheets[sheet].filter.as_ref().is_some_and(|f| f.range == t.filter_range()) {
            ops.push(Op::Filter { sheet, filter: None });
            ops.extend(self.show_filtered(sheet, &t.filter_range()));
        }
        self.execute_as(Op::Batch(ops), "undo.table_convert");
    }

    /// The table's look as the cells' own formats.
    fn intern_baked(&mut self, sheet: usize, t: &Table) -> Vec<(CellRef, Option<Cell>)> {
        let mut out = Vec::new();
        for row in t.range.start.row..=t.range.end.row {
            for col in t.range.start.col..=t.range.end.col {
                let c = CellRef::new(row, col);
                let Some(l) = t.look(c) else { continue };
                let old = self.sheets[sheet].get(c).cloned();
                let base = self.styles.get(old.as_ref().map_or(0, |x| x.style)).clone();
                let s = apply_look(&base, &l);
                if s != base {
                    let id = self.styles.intern(s);
                    out.push((c, Cell::with_style(old.as_ref(), id)));
                }
            }
        }
        out
    }

    /// Drops the table name from the structured references of formulas
    /// inside their own table (`SUBTOTAL(109,[Sales])` in its totals row),
    /// as Excel shows them. Files may spell them either way; ApexDesk
    /// writes the name back for other programs (see [`Self::formula_for_xlsx`]).
    pub fn unqualify_own_tables(&mut self) {
        for sh in &mut self.sheets {
            for ti in 0..sh.tables.len() {
                let (range, name) = (sh.tables[ti].range, sh.tables[ti].name.to_lowercase());
                let mut found = Vec::new();
                sh.for_each_in(&range, |c, cell| {
                    if cell.formula().is_some_and(|e| e.shape().has_structured()) {
                        found.push(c);
                    }
                });
                for c in found {
                    let Some(Content::Formula { expr, .. }) = sh.get_mut(c).map(|x| &mut x.content) else { continue };
                    let mut changed = false;
                    let mut e = expr.expr().into_owned();
                    e.visit_structured_mut(&mut |x| {
                        if let Expr::Structured(r) = x {
                            if r.table.as_deref().is_some_and(|t| t.to_lowercase() == name) {
                                r.table = None;
                                changed = true;
                            }
                        }
                    });
                    if changed {
                        *expr = e.into();
                    }
                }
            }
        }
    }

    /// Bakes every table's look into cell formats (a copy for export to
    /// formats without table styles).
    pub fn with_baked_tables(&self) -> Workbook {
        let mut wb = self.clone();
        for si in 0..wb.sheets.len() {
            let tables = wb.sheets[si].tables.clone();
            for t in &tables {
                for (c, cell) in wb.intern_baked(si, t) {
                    wb.sheets[si].put(c, cell);
                }
            }
        }
        wb
    }

    // --- typing ----------------------------------------------------------

    /// Follows up an edit of cell `c` the way Excel's tables do: a new
    /// header renames the column, typing just below or right of a table
    /// grows it, and a formula in an empty column fills the whole column.
    /// Each of these is its own undo step (Ctrl+Z undoes it first).
    /// Returns whether anything changed.
    pub fn table_after_input(&mut self, sheet: usize, c: CellRef) -> bool {
        if self.sheets.get(sheet).is_none_or(|s| s.tables.is_empty()) {
            return false;
        }
        if let Some(id) = self.table_at(sheet, c) {
            let t = self.sheets[sheet].tables[id.1].clone();
            if t.header_row() == Some(c.row) {
                return self.rename_from_header(id, c);
            }
            let (d0, d1) = t.data_rows();
            if (d0..=d1).contains(&c.row) {
                return self.fill_calculated(id, c);
            }
            return false;
        }
        if matches!(self.sheets[sheet].value(c), Value::Empty) {
            return false;
        }
        // Just below a table without totals, or just right of one.
        let below = self.sheets[sheet].tables.iter().position(|t| !t.totals && c.row == t.range.end.row + 1 && (t.range.start.col..=t.range.end.col).contains(&c.col));
        let right = self.sheets[sheet].tables.iter().position(|t| c.col == t.range.end.col + 1 && (t.range.start.row..=t.data_rows().1).contains(&c.row));
        let Some(i) = below.or(right) else { return false };
        let t = self.sheets[sheet].tables[i].clone();
        let mut r = t.range;
        if below.is_some() {
            r.end.row += 1;
        } else {
            r.end.col += 1;
        }
        if self.resize_table((sheet, i), r).is_err() {
            return false;
        }
        // The undo label says what happened.
        if let Some(last) = self.undo.last_mut() {
            last.1 = "undo.table_expand";
        }
        if below.is_some() {
            // The new row gets the calculated columns.
            let nt = self.sheets[sheet].tables[i].clone();
            let (d0, _) = nt.data_rows();
            let mut cells = Vec::new();
            for (k, col) in nt.columns.iter().enumerate() {
                let at = CellRef::new(c.row, nt.col_at(k));
                if at == c || !matches!(self.sheets[sheet].get(at).map(|x| &x.content), None | Some(Content::Empty)) {
                    continue;
                }
                if let Some(e) = col.formula.as_deref().and_then(|f| formula::parse_canonical(f).ok()) {
                    let mut e = e;
                    e.offset(c.row as i64 - d0 as i64, 0);
                    cells.push((at, Cell::with_content(self.sheets[sheet].get(at), Content::Formula { expr: e.into(), value: Value::Empty })));
                }
            }
            if !cells.is_empty() {
                let before = self.undo.len();
                self.execute(Op::SetCells { sheet, cells });
                if self.undo.len() > before {
                    self.merge_undo(2, "undo.table_expand");
                }
            }
        }
        true
    }

    fn rename_from_header(&mut self, id: TableId, c: CellRef) -> bool {
        let t = self.sheets[id.0].tables[id.1].clone();
        let i = (c.col - t.range.start.col) as usize;
        let typed = self.header_text(id.0, c);
        let old = t.columns[i].name.clone();
        // Headers are unique text; an empty header gets a generic name.
        let mut names: Vec<String> = t.columns.iter().map(|x| x.name.clone()).collect();
        names[i] = String::new();
        let mut name = if typed.is_empty() { t.fresh_name("Column") } else { typed.clone() };
        let mut k = 2;
        while names.iter().any(|n| n.to_lowercase() == name.to_lowercase()) {
            name = format!("{}{k}", if typed.is_empty() { "Column" } else { &typed });
            k += 1;
        }
        let mut ops = Vec::new();
        if name != typed || !matches!(self.sheets[id.0].value(c), Value::Text(_)) {
            ops.push(Op::SetCells { sheet: id.0, cells: vec![(c, Cell::with_content(self.sheets[id.0].get(c), Content::Value(Value::text(name.as_str()))))] });
        }
        if name != old {
            let mut list = self.sheets[id.0].tables.clone();
            list[id.1].columns[i].name = name.clone();
            ops.insert(0, Op::Tables { sheet: id.0, list });
            ops.extend(self.rename_column_refs(id.0, &t, &old, &name));
        }
        if ops.is_empty() {
            return false;
        }
        let before = self.undo.len();
        self.execute_as(Op::Batch(ops), "undo.edit");
        // Part of the edit that caused it.
        if self.undo.len() > before {
            self.merge_undo(2, "undo.edit");
        }
        true
    }

    /// A formula typed into a data cell fills the column when the rest of
    /// the column is empty or already holds the same calculation.
    fn fill_calculated(&mut self, id: TableId, c: CellRef) -> bool {
        let t = self.sheets[id.0].tables[id.1].clone();
        let i = (c.col - t.range.start.col) as usize;
        let (d0, d1) = t.data_rows();
        let sh = &self.sheets[id.0];
        let Some(e) = sh.get(c).and_then(|x| x.formula()).map(|e| e.expr().into_owned()) else {
            return false;
        };
        let mut first = unqualified(&e, &t.name);
        first.offset(d0 as i64 - c.row as i64, 0);
        let canon = first.to_canonical();
        let same = |r: u32| -> bool {
            let at = CellRef::new(r, c.col);
            match sh.get(at).map(|x| &x.content) {
                None | Some(Content::Empty) => true,
                Some(Content::Formula { expr, .. }) => {
                    let mut f = unqualified(&expr.expr(), &t.name);
                    f.offset(d0 as i64 - r as i64, 0);
                    let text = f.to_canonical();
                    Some(&text) == t.columns[i].formula.as_ref() || text == canon
                }
                _ => false,
            }
        };
        if !(d0..=d1).filter(|&r| r != c.row).all(same) {
            return false;
        }
        let mut cells = Vec::new();
        for r in d0..=d1 {
            if r == c.row {
                continue;
            }
            let at = CellRef::new(r, c.col);
            let mut x = first.clone();
            x.offset(r as i64 - d0 as i64, 0);
            let cell = Cell::with_content(sh.get(at), Content::Formula { expr: x.into(), value: Value::Empty });
            if sh.get(at) != cell.as_ref() {
                cells.push((at, cell));
            }
        }
        let mut list = self.sheets[id.0].tables.clone();
        let changed_def = list[id.1].columns[i].formula.as_deref() != Some(canon.as_str());
        list[id.1].columns[i].formula = Some(canon);
        if cells.is_empty() && !changed_def {
            return false;
        }
        let mut ops = vec![Op::Tables { sheet: id.0, list }];
        if !cells.is_empty() {
            ops.push(Op::SetCells { sheet: id.0, cells });
        }
        self.execute_as(Op::Batch(ops), "undo.table_calc");
        true
    }

    /// Drops the calculated-column setting of column `col` (Excel's "Stop
    /// automatically creating calculated columns" for one column).
    pub fn clear_calculated(&mut self, id: TableId, col: usize) {
        let Some(t) = self.table(id) else { return };
        if t.columns.get(col).is_none_or(|c| c.formula.is_none()) {
            return;
        }
        let mut list = self.sheets[id.0].tables.clone();
        list[id.1].columns[col].formula = None;
        self.execute_as(Op::Tables { sheet: id.0, list }, "undo.table");
    }
}

/// `e` with the references to table `own` written without its name (the
/// form stored for calculated columns).
pub fn unqualified(e: &Expr, own: &str) -> Expr {
    let mut e = e.clone();
    let own = own.to_lowercase();
    e.visit_structured_mut(&mut |x| {
        if let Expr::Structured(r) = x {
            if r.table.as_deref().is_some_and(|n| n.to_lowercase() == own) {
                r.table = None;
            }
        }
    });
    e
}

/// A cell format with a table look under it: the cell's own settings win.
pub fn apply_look(base: &CellStyle, l: &CellLook) -> CellStyle {
    let mut s = base.clone();
    if s.fill.is_none() {
        s.fill = l.fill;
    }
    if s.color.is_none() {
        s.color = l.color;
    }
    s.bold |= l.bold;
    let side = |own: &mut Line, t: Option<Line>| {
        if own.style == LineStyle::None {
            if let Some(t) = t {
                *own = t;
            }
        }
    };
    side(&mut s.borders.top, l.top);
    side(&mut s.borders.bottom, l.bottom);
    side(&mut s.borders.left, l.left);
    side(&mut s.borders.right, l.right);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> Workbook {
        let mut wb = Workbook::new();
        wb.locale = Locale::EN;
        let rows = [["Region", "Qty", "Price"], ["North", "2", "10"], ["South", "3", "20"], ["East", "5", "30"]];
        let mut inputs = Vec::new();
        for (r, row) in rows.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                inputs.push((CellRef::new(r as u32, c as u32), v.to_string()));
            }
        }
        wb.set_inputs(0, inputs);
        wb
    }

    fn a(s: &str) -> CellRef {
        CellRef::parse(s).unwrap()
    }

    fn num(wb: &Workbook, s: &str) -> Value {
        wb.sheets[0].value(a(s)).clone()
    }

    #[test]
    fn create_and_refs() {
        let mut wb = book();
        let id = wb.create_table(0, Range::parse("A1:C4").unwrap(), true, "TableStyleMedium2").unwrap();
        assert_eq!(id, (0, 0));
        let t = wb.table(id).unwrap();
        assert_eq!(t.name, "Table1");
        assert_eq!(t.columns.len(), 3);
        assert!(wb.sheets[0].filter.is_some());
        wb.set_input(0, a("E1"), "=SUM(Table1[Qty])");
        assert_eq!(num(&wb, "E1"), Value::Number(10.0));
        wb.set_input(0, a("E2"), "=Table1[[#Headers],[Price]]");
        assert_eq!(num(&wb, "E2"), Value::text("Price"));
        wb.set_input(0, a("E3"), "=ROWS(Table1[#All])+COLUMNS(Table1[])");
        assert_eq!(num(&wb, "E3"), Value::Number(7.0));
        // Dependencies follow the table.
        wb.set_input(0, a("B2"), "4");
        assert_eq!(num(&wb, "E1"), Value::Number(12.0));
        // Overlaps are refused; undo removes the table.
        assert_eq!(wb.create_table(0, Range::parse("B2:D5").unwrap(), true, "TableStyleLight1"), Err(TableError::Overlap));
        while wb.sheets[0].tables.len() == 1 {
            assert!(wb.undo());
        }
        assert!(wb.sheets[0].filter.is_none());
    }

    #[test]
    fn calculated_column_and_expansion() {
        let mut wb = book();
        let id = wb.create_table(0, Range::parse("A1:C4").unwrap(), true, "TableStyleLight9").unwrap();
        // A new column by typing a header to the right.
        wb.set_input(0, a("D1"), "Total");
        assert!(wb.table_after_input(0, a("D1")));
        assert_eq!(wb.table(id).unwrap().range.to_a1(), "A1:D4");
        assert_eq!(wb.table(id).unwrap().columns[3].name, "Total");
        // A formula fills the whole column.
        wb.set_input(0, a("D2"), "=[@Qty]*[@Price]");
        assert!(wb.table_after_input(0, a("D2")));
        assert_eq!(num(&wb, "D3"), Value::Number(60.0));
        assert_eq!(num(&wb, "D4"), Value::Number(150.0));
        assert_eq!(wb.table(id).unwrap().columns[3].formula.as_deref(), Some("[@Qty]*[@Price]"));
        assert_eq!(wb.edit_text(0, a("D3")), "=[@Qty]*[@Price]");
        // Typing below grows the table and brings the formula along.
        wb.set_input(0, a("A5"), "West");
        wb.set_input(0, a("B5"), "1");
        assert!(wb.table_after_input(0, a("A5")));
        wb.set_input(0, a("C5"), "7");
        assert_eq!(wb.table(id).unwrap().range.to_a1(), "A1:D5");
        assert_eq!(num(&wb, "D5"), Value::Number(7.0));
        // The expansion is its own undo step.
        wb.undo();
        wb.undo();
        assert_eq!(wb.table(id).unwrap().range.to_a1(), "A1:D4");
        assert!(wb.cell(0, a("D5")).is_none());
    }

    #[test]
    fn totals_rename_and_convert() {
        let mut wb = book();
        let id = wb.create_table(0, Range::parse("A1:C4").unwrap(), true, "TableStyleMedium2").unwrap();
        wb.set_input(0, a("F1"), "=Table1[[#Totals],[Price]]");
        wb.set_table_totals(id, true);
        let t = wb.table(id).unwrap().clone();
        assert_eq!(t.range.to_a1(), "A1:C5");
        assert_eq!(num(&wb, "A5"), Value::text("Total"));
        assert_eq!(num(&wb, "C5"), Value::Number(60.0));
        assert_eq!(num(&wb, "F1"), Value::Number(60.0));
        wb.set_total_fn(id, 1, TotalFn::Average);
        assert!(matches!(num(&wb, "B5"), Value::Number(n) if (n - 10.0 / 3.0).abs() < 1e-9));
        // Renaming a header renames the references.
        wb.set_input(0, a("F2"), "=SUM(Table1[Qty])");
        wb.set_input(0, a("B1"), "Units");
        wb.table_after_input(0, a("B1"));
        assert_eq!(wb.edit_text(0, a("F2")), "=SUM(Table1[Units])");
        assert_eq!(wb.edit_text(0, a("B5")), "=SUBTOTAL(101,[Units])");
        // And renaming the table too.
        wb.rename_table(id, "Sales").unwrap();
        assert_eq!(wb.edit_text(0, a("F2")), "=SUM(Sales[Units])");
        assert_eq!(wb.rename_table(id, "A1"), Err(TableError::BadName));
        // Export forms.
        let e = wb.cell(0, a("B5")).unwrap().formula().unwrap().to_arc();
        assert_eq!(wb.formula_for_xlsx(0, a("B5"), &e), "SUBTOTAL(101,Sales[Units])");
        assert_eq!(wb.formula_for_odf(0, a("B5"), &e), "of:=SUBTOTAL(101;Sales[Units])");
        // Totals off, then convert to a range.
        wb.set_table_totals(id, false);
        assert!(wb.cell(0, a("C5")).is_none_or(|c| matches!(c.content, Content::Empty)));
        wb.convert_table_to_range(id);
        assert!(wb.sheets[0].tables.is_empty());
        assert_eq!(wb.edit_text(0, a("F2")), "=SUM($B$2:$B$4)");
        assert!(wb.styles.get(wb.cell(0, a("A1")).unwrap().style).bold);
        wb.undo();
        assert_eq!(wb.sheets[0].tables.len(), 1);
        assert_eq!(wb.edit_text(0, a("F2")), "=SUM(Sales[Units])");
    }

    #[test]
    fn structure_edits() {
        let mut wb = book();
        let id = wb.create_table(0, Range::parse("A1:C4").unwrap(), false, "TableStyleLight1").unwrap();
        // Without headers the data moved down under generic names.
        assert_eq!(wb.table(id).unwrap().range.to_a1(), "A1:C5");
        assert_eq!(num(&wb, "A1"), Value::text("Column1"));
        assert_eq!(num(&wb, "A2"), Value::text("Region"));
        wb.set_input(0, a("E1"), "=COUNTA(Table1[Column2])");
        assert_eq!(num(&wb, "E1"), Value::Number(4.0));
        wb.execute(Op::InsertRows { sheet: 0, at: 2, count: 2 });
        assert_eq!(wb.table(id).unwrap().range.to_a1(), "A1:C7");
        wb.execute(Op::DeleteCols { sheet: 0, at: 0, count: 1 });
        let t = wb.table(id).unwrap();
        assert_eq!(t.range.to_a1(), "A1:B7");
        assert_eq!(t.columns[0].name, "Column2");
        assert_eq!(num(&wb, "D1"), Value::Number(4.0));
        wb.undo();
        wb.undo();
        assert_eq!(wb.table(id).unwrap().range.to_a1(), "A1:C5");
        wb.resize_table(id, Range::parse("A1:B3").unwrap()).unwrap();
        assert_eq!(num(&wb, "E1"), Value::Number(2.0));
        assert_eq!(wb.resize_table(id, Range::parse("A2:B3").unwrap()), Err(TableError::BadRange));
    }
}
