//! Defined names on a workbook: looking them up for formulas, editing them
//! (one undo step), and keeping them right when sheets change.

use super::*;
use crate::names::{self, DefinedName};

impl Workbook {
    /// The name `text` means on `sheet`: its local name first, then the
    /// workbook one. `Sheet2!Total` looks on Sheet2 only.
    pub fn find_name(&self, sheet: usize, text: &str) -> Option<&DefinedName> {
        if let Some((s, n)) = split_sheet(text) {
            let si = self.sheet_index(&s)?;
            let own = &self.sheets[si].name;
            return self.names.iter().find(|d| d.is(n) && d.scope.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(own)));
        }
        let own = self.sheets.get(sheet).map(|s| s.name.as_str());
        self.names
            .iter()
            .find(|d| d.is(text) && d.scope.as_deref().zip(own).is_some_and(|(a, b)| a.eq_ignore_ascii_case(b)))
            .or_else(|| self.names.iter().find(|d| d.is(text) && d.scope.is_none()))
    }

    pub(crate) fn name_expr(&self, sheet: usize, text: &str) -> Option<Arc<Expr>> {
        if self.names.is_empty() {
            return None;
        }
        self.find_name(sheet, text).map(|d| d.expr.clone())
    }

    /// Whether the name `text` on `sheet` may stand for several cells (a
    /// range, what OFFSET gives...): a plain formula takes one of them.
    fn may_be_many(&self, sheet: usize, text: &str, depth: u32) -> bool {
        fn many(wb: &Workbook, sheet: usize, e: &Expr, depth: u32) -> bool {
            match e {
                Expr::Range { a, b, .. } => a != b,
                Expr::Ref { .. } | Expr::Number(_) | Expr::Text(_) | Expr::Bool(_) | Expr::Error(_) | Expr::Missing | Expr::Lit(_) => false,
                Expr::Paren(x) => many(wb, sheet, x, depth),
                Expr::Name(n) => depth < 8 && wb.may_be_many(sheet, n, depth + 1),
                _ => true,
            }
        }
        self.name_expr(sheet, text).is_some_and(|e| many(self, sheet, &e, depth))
    }

    /// Formulas read from files carry an `@` on every name used for one
    /// value (`Expr::legacy`); it stays only on the names that may stand
    /// for several cells.
    pub(crate) fn settle_name_intersections(&mut self) {
        for si in 0..self.sheets.len() {
            let found: Vec<CellRef> = self.sheets[si].iter().filter(|(_, c)| c.formula().is_some_and(|e| e.shape().has_at_name())).map(|(c, _)| c).collect();
            for c in found {
                let Some(mut e) = self.sheets[si].get(c).and_then(|x| x.formula()).map(|e| e.expr().into_owned()) else { continue };
                e.drop_at_names(&|n| self.may_be_many(si, n, 0));
                if let Some(Content::Formula { expr, .. }) = self.sheets[si].get_mut(c).map(|x| &mut x.content) {
                    *expr = e.into();
                }
            }
        }
    }

    /// Whether `expr` uses a name whose formula is volatile (NOW, RAND...).
    pub(crate) fn uses_volatile_name(&self, sheet: usize, expr: &Expr) -> bool {
        if self.names.is_empty() {
            return false;
        }
        let mut v = false;
        expr.visit_names(&mut |n| v |= self.name_expr(sheet, n).is_some_and(|e| e.is_volatile()));
        v
    }

    /// The cells a name stands for, to select them (Name Box, Go To).
    pub fn name_target(&self, sheet: usize, text: &str) -> Option<(usize, Range)> {
        let d = self.find_name(sheet, text)?;
        let (s, r) = d.range()?;
        let home = d.scope.as_deref().and_then(|n| self.sheet_index(n)).unwrap_or(sheet);
        let si = match s {
            Some(s) => self.sheet_index(&s)?,
            None => home,
        };
        Some((si, r))
    }

    /// The name of exactly `range` on `sheet`, for the Name Box.
    pub fn name_of_range(&self, sheet: usize, range: Range) -> Option<&str> {
        self.names
            .iter()
            .filter(|d| !d.hidden)
            .find(|d| d.range().is_some_and(|(s, r)| r == range && s.as_deref().map_or(Some(sheet), |s| self.sheet_index(s)) == Some(sheet)))
            .map(|d| d.name.as_str())
    }

    /// Replaces all names (one undo step).
    pub fn set_names(&mut self, list: Vec<DefinedName>) {
        if list != self.names {
            self.execute(Op::Names(list));
        }
    }

    /// Replaces the names and renames their uses in formulas (`renames`:
    /// old text, new text), in one undo step.
    pub fn replace_names(&mut self, list: Vec<DefinedName>, renames: &[(String, String)]) {
        let mut ops = vec![Op::Names(list)];
        if !renames.is_empty() {
            for (si, sh) in self.sheets.iter().enumerate() {
                let mut cells = Vec::new();
                for (c, cell) in sh.iter() {
                    let Content::Formula { expr, value } = &cell.content else { continue };
                    let mut e = expr.expr().into_owned();
                    let mut changed = false;
                    e.visit_names_mut(&mut |n| {
                        if let Some((_, new)) = renames.iter().find(|(old, _)| old.eq_ignore_ascii_case(n)) {
                            *n = Arc::from(new.as_str());
                            changed = true;
                        }
                    });
                    if changed {
                        let mut nc = cell.clone();
                        nc.content = Content::Formula { expr: e.into(), value: value.clone() };
                        cells.push((c, Some(nc)));
                    }
                }
                if !cells.is_empty() {
                    ops.push(Op::SetCells { sheet: si, cells });
                }
            }
        }
        self.execute_as(Op::Batch(ops), "undo.names");
    }

    /// Adds or replaces one name.
    pub fn define_name(&mut self, d: DefinedName) {
        let mut list = self.names.clone();
        list.retain(|x| !(x.is(&d.name) && x.scope == d.scope));
        list.push(d);
        self.set_names(list);
    }

    /// Names for the rows or columns of `range` from its labels (Create
    /// from Selection). Returns how many names were made.
    pub fn names_from_selection(&mut self, sheet: usize, range: Range, top: bool, left: bool, bottom: bool, right: bool) -> usize {
        let sname = self.sheets[sheet].name.clone();
        let label = |c: CellRef| names::name_from_label(&self.display(sheet, c).0);
        let (r0, r1, c0, c1) = (range.start.row, range.end.row, range.start.col, range.end.col);
        // The data area leaves out the label rows and columns.
        let dr0 = r0 + top as u32;
        let dr1 = r1.saturating_sub(bottom as u32);
        let dc0 = c0 + left as u32;
        let dc1 = c1.saturating_sub(right as u32);
        if dr0 > dr1 || dc0 > dc1 {
            return 0;
        }
        let mut made: Vec<DefinedName> = Vec::new();
        let mut add = |name: Option<String>, r: Range| {
            if let Some(n) = name {
                if !made.iter().any(|d| d.is(&n)) {
                    made.push(DefinedName::new(&n, None, names::absolute_ref(&sname, r)));
                }
            }
        };
        if top {
            for c in dc0..=dc1 {
                add(label(CellRef::new(r0, c)), Range::new(CellRef::new(dr0, c), CellRef::new(dr1, c)));
            }
        }
        if bottom {
            for c in dc0..=dc1 {
                add(label(CellRef::new(r1, c)), Range::new(CellRef::new(dr0, c), CellRef::new(dr1, c)));
            }
        }
        if left {
            for r in dr0..=dr1 {
                add(label(CellRef::new(r, c0)), Range::new(CellRef::new(r, dc0), CellRef::new(r, dc1)));
            }
        }
        if right {
            for r in dr0..=dr1 {
                add(label(CellRef::new(r, c1)), Range::new(CellRef::new(r, dc0), CellRef::new(r, dc1)));
            }
        }
        let n = made.len();
        if n > 0 {
            let mut list = self.names.clone();
            for d in made {
                list.retain(|x| !(x.is(&d.name) && x.scope.is_none()));
                list.push(d);
            }
            self.set_names(list);
        }
        n
    }

    /// Rewrites the names' references with `f(own sheet, expr)`.
    pub(crate) fn names_edit(&mut self, mut f: impl FnMut(&str, &mut Expr) -> bool) {
        let first = self.sheets.first().map(|s| s.name.clone()).unwrap_or_default();
        for d in &mut self.names {
            let own = d.scope.clone().unwrap_or_else(|| first.clone());
            let mut e = (*d.expr).clone();
            if f(&own, &mut e) {
                d.expr = Arc::new(e);
            }
        }
    }

    pub(crate) fn names_rename_sheet(&mut self, old: &str, new: &str) {
        for d in &mut self.names {
            if d.scope.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(old)) {
                d.scope = Some(new.to_string());
            }
        }
        self.names_edit(|_, e| e.rename_sheet(old, new));
    }
}

/// `Sheet2!Total` or `'My sheet'!Total` into (sheet, name).
fn split_sheet(text: &str) -> Option<(String, &str)> {
    let i = text.rfind('!')?;
    let (s, n) = (&text[..i], &text[i + 1..]);
    let s = s.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')).map_or_else(|| s.to_string(), |s| s.replace("''", "'"));
    Some((s, n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formula::parse_canonical;

    fn wb() -> Workbook {
        let mut wb = Workbook::new();
        wb.sheets[0].name = "Data".into();
        wb.execute(Op::AddSheet { index: 1, sheet: Box::new(Sheet::new("Other")) });
        for r in 0..3 {
            wb.set_input(0, CellRef::new(r + 1, 1), &format!("{}", (r + 1) * 10));
        }
        wb
    }

    #[test]
    fn names_in_formulas() {
        let mut wb = wb();
        wb.define_name(DefinedName::new("Sales", None, parse_canonical("Data!$B$2:$B$4").unwrap()));
        wb.define_name(DefinedName::new("Rate", None, parse_canonical("0.5").unwrap()));
        wb.define_name(DefinedName::new("Double", None, parse_canonical("LAMBDA(x,x*2)").unwrap()));
        wb.set_input(0, CellRef::new(0, 3), "=SUM(Sales)*Rate");
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 3)), &Value::Number(30.0));
        wb.set_input(0, CellRef::new(0, 4), "=Double(21)");
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 4)), &Value::Number(42.0));
        // Dependents follow the cells the name stands for.
        wb.set_input(0, CellRef::new(1, 1), "110");
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 3)), &Value::Number(80.0));
        // Changing the name recalculates.
        wb.define_name(DefinedName::new("Rate", None, parse_canonical("1").unwrap()));
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 3)), &Value::Number(160.0));
        wb.undo();
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 3)), &Value::Number(80.0));
        // Unknown names stay #NAME?.
        wb.set_input(0, CellRef::new(0, 5), "=Nope+1");
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 5)), &Value::Error(ErrorKind::Name));
    }

    #[test]
    fn local_names_and_structure() {
        let mut wb = wb();
        wb.define_name(DefinedName::new("X", None, parse_canonical("1").unwrap()));
        wb.define_name(DefinedName::new("X", Some("Other".into()), parse_canonical("2").unwrap()));
        wb.set_input(0, CellRef::new(0, 3), "=X");
        wb.set_input(1, CellRef::new(0, 0), "=X");
        wb.set_input(0, CellRef::new(0, 4), "=Other!X");
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 3)), &Value::Number(1.0));
        assert_eq!(wb.sheets[1].value(CellRef::new(0, 0)), &Value::Number(2.0));
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 4)), &Value::Number(2.0));
        // Inserting rows moves the name's range.
        wb.define_name(DefinedName::new("Sales", None, parse_canonical("Data!$B$2:$B$4").unwrap()));
        wb.execute(Op::InsertRows { sheet: 0, at: 0, count: 2 });
        assert_eq!(wb.find_name(0, "Sales").unwrap().expr.to_canonical(), "Data!$B$4:$B$6");
        assert_eq!(wb.name_target(0, "sales"), Some((0, Range::new(CellRef::new(3, 1), CellRef::new(5, 1)))));
        wb.execute(Op::RenameSheet { index: 0, name: "Datos".into() });
        assert_eq!(wb.find_name(0, "Sales").unwrap().expr.to_canonical(), "Datos!$B$4:$B$6");
        wb.execute(Op::RenameSheet { index: 1, name: "Otra".into() });
        assert_eq!(wb.find_name(1, "X").unwrap().scope.as_deref(), Some("Otra"));
    }

    #[test]
    fn relative_names_and_selection() {
        let mut wb = wb();
        // A1-relative: "the cell to the left".
        wb.define_name(DefinedName::new("Left", None, parse_canonical("Data!XFD1").unwrap()));
        wb.set_input(0, CellRef::new(1, 2), "=Left*2");
        assert_eq!(wb.sheets[0].value(CellRef::new(1, 2)), &Value::Number(20.0));
        wb.set_input(0, CellRef::new(0, 5), "Price");
        wb.set_input(0, CellRef::new(1, 5), "3");
        wb.set_input(0, CellRef::new(2, 5), "4");
        assert_eq!(wb.names_from_selection(0, Range::new(CellRef::new(0, 5), CellRef::new(2, 5)), true, false, false, false), 1);
        wb.set_input(0, CellRef::new(5, 5), "=SUM(Price)");
        assert_eq!(wb.sheets[0].value(CellRef::new(5, 5)), &Value::Number(7.0));
        assert_eq!(wb.name_of_range(0, Range::new(CellRef::new(1, 5), CellRef::new(2, 5))), Some("Price"));
        // Renaming a name rewrites the formulas that use it.
        let mut list = wb.names.clone();
        list.iter_mut().find(|d| d.is("Price")).unwrap().name = "Cost".into();
        wb.replace_names(list, &[("Price".into(), "Cost".into())]);
        assert_eq!(wb.sheets[0].get(CellRef::new(5, 5)).unwrap().formula().unwrap().expr().to_canonical(), "SUM(Cost)");
        assert_eq!(wb.sheets[0].value(CellRef::new(5, 5)), &Value::Number(7.0));
        wb.undo();
        assert_eq!(wb.sheets[0].get(CellRef::new(5, 5)).unwrap().formula().unwrap().expr().to_canonical(), "SUM(Price)");
    }
    #[test]
    fn names_doubling_up_stop() {
        let mut wb = wb();
        for i in 1..60 {
            wb.define_name(DefinedName::new(&format!("Nm_{i}"), None, parse_canonical(&format!("Nm_{0}+Nm_{0}", i + 1)).unwrap()));
        }
        wb.define_name(DefinedName::new("Nm_60", None, parse_canonical("1").unwrap()));
        wb.define_name(DefinedName::new("F", None, parse_canonical("LAMBDA(n,IF(n<1,0,F(n-1)+F(n-1)))").unwrap()));
        wb.set_input(0, CellRef::new(0, 3), "=Nm_1");
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 3)), &Value::Error(ErrorKind::Num));
        // Ends, one way or another.
        wb.set_input(0, CellRef::new(0, 4), "=F(100)");
        assert!(matches!(wb.sheets[0].value(CellRef::new(0, 4)), Value::Number(_) | Value::Error(_)));
        wb.set_input(0, CellRef::new(0, 5), "=F(3)");
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 5)), &Value::Number(0.0));
    }
    #[test]
    fn deep_names_run_out_of_depth_not_stack() {
        let mut wb = wb();
        let zeros = "+0".repeat(1000);
        for i in 1..30 {
            wb.define_name(DefinedName::new(&format!("Nm_{i}"), None, parse_canonical(&format!("Nm_{}{zeros}", i + 1)).unwrap()));
        }
        wb.define_name(DefinedName::new("Nm_30", None, parse_canonical("1").unwrap()));
        wb.set_input(0, CellRef::new(0, 3), "=Nm_1");
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 3)), &Value::Error(ErrorKind::Num));
        wb.define_name(DefinedName::new("SelfCall", None, parse_canonical(&format!("LAMBDA(g,n,IF(n=0,0,g(g,n-1){zeros}))")).unwrap()));
        wb.define_name(DefinedName::new("Run", None, parse_canonical("SelfCall(SelfCall,150)").unwrap()));
        wb.set_input(0, CellRef::new(0, 4), "=Run");
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 4)), &Value::Error(ErrorKind::Num));
        // An ordinary long sum still adds up.
        wb.set_input(0, CellRef::new(0, 5), &format!("=1{}", "+1".repeat(150)));
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 5)), &Value::Number(151.0));
    }
}
