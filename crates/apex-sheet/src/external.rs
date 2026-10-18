//! Other workbooks that formulas refer to: `[1]Sheet1!A1` or
//! `'[Budget.xlsx]Sheet 1'!A1`. The linked files are not opened; formulas
//! use the values the file saved from them the last time.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::addr::{CellRef, Range};
use crate::formula::Expr;
use crate::value::{ErrorKind, Matrix, Value};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExternalBook {
    /// The linked file as the saving program wrote it (a path or URL).
    pub target: String,
    pub sheets: Vec<String>,
    /// Names defined in the linked book: name, sheet (None: the book) and
    /// the formula they stand for, in the linked book's terms.
    pub names: Vec<(String, Option<usize>, String)>,
    /// Saved values of the cells formulas use, per sheet.
    pub cells: Vec<BTreeMap<CellRef, Value>>,
}

impl ExternalBook {
    /// The file name without folders (`Budget.xlsx`).
    pub fn file_name(&self) -> &str {
        self.target.rsplit(['/', '\\']).next().unwrap_or(&self.target)
    }

    fn sheet(&self, name: &str) -> Option<usize> {
        self.sheets.iter().position(|s| s.eq_ignore_ascii_case(name))
    }
}

/// Whether a sheet name in a formula points into another workbook.
pub fn is_external(sheet: &str) -> bool {
    sheet.starts_with('[') && sheet.contains(']')
}

/// `[1]Sheet1` → ("1", "Sheet1").
pub fn split_external(sheet: &str) -> Option<(&str, &str)> {
    let rest = sheet.strip_prefix('[')?;
    rest.split_once(']')
}

/// The book a `[1]` or `[Budget.xlsx]` prefix stands for.
pub fn find_book<'a>(books: &'a [ExternalBook], key: &str) -> Option<(usize, &'a ExternalBook)> {
    if let Ok(n) = key.parse::<usize>() {
        return books.get(n.checked_sub(1)?).map(|b| (n - 1, b));
    }
    books.iter().enumerate().find(|(_, b)| b.file_name().eq_ignore_ascii_case(key) || b.target.eq_ignore_ascii_case(key))
}

/// The arrays of external ranges built during one recalculation: thousands
/// of VLOOKUPs into the same linked table share one copy of it. Off (and
/// empty) outside a recalculation, when the links may change.
#[derive(Default)]
pub(crate) struct RangeCache(std::sync::Mutex<Option<(std::collections::HashMap<(String, Range), Value>, usize)>>);

/// Cells the cache keeps in all: past them ranges are built each time
/// (formulas each naming another million-cell range would keep them all).
const MAX_CACHED_CELLS: usize = 1 << 24;

impl Clone for RangeCache {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl RangeCache {
    pub(crate) fn set_active(&self, on: bool) {
        if let Ok(mut c) = self.0.lock() {
            *c = on.then(Default::default);
        }
    }

    /// The array of `range`, built by `f` the first time.
    pub(crate) fn get(&self, sheet: &str, range: &Range, f: impl FnOnce() -> Value) -> Value {
        if range.is_single() {
            return f();
        }
        let key = (sheet.to_string(), *range);
        if let Some(v) = self.0.lock().ok().and_then(|c| c.as_ref()?.0.get(&key).cloned()) {
            return v;
        }
        let v = f();
        if let Ok(mut c) = self.0.lock()
            && let Some((map, cells)) = c.as_mut()
        {
            let n = match &v {
                Value::Array(m) => m.data.len(),
                _ => 1,
            };
            if *cells + n <= MAX_CACHED_CELLS {
                *cells += n;
                map.insert(key, v.clone());
            }
        }
        v
    }
}

/// Saved value of `range` on an external `sheet` (`[1]Sheet1`): one value,
/// or an array.
pub fn value(books: &[ExternalBook], sheet: &str, range: &Range) -> Value {
    let Some((key, name)) = split_external(sheet) else { return Value::Error(ErrorKind::Ref) };
    let Some((_, book)) = find_book(books, key) else { return Value::Error(ErrorKind::Ref) };
    let Some(si) = book.sheet(name) else { return Value::Error(ErrorKind::Ref) };
    let cells = &book.cells[si];
    // No saved values (the link came without them): cells keep the values
    // the file saved for them.
    if cells.is_empty() {
        return Value::Error(ErrorKind::Ref);
    }
    let get = |c: CellRef| cells.get(&c).cloned().unwrap_or(Value::Empty);
    if range.is_single() {
        return get(range.start);
    }
    // Whole columns and rows stop at the last saved cell.
    let (last_row, last_col) = cells.keys().fold((0, 0), |(r, c), k| (r.max(k.row), c.max(k.col)));
    let mut range = *range;
    if range.end.row == crate::addr::MAX_ROWS - 1 {
        range.end.row = last_row.max(range.start.row);
    }
    if range.end.col == crate::addr::MAX_COLS - 1 {
        range.end.col = last_col.max(range.start.col);
    }
    if range.cell_count() > 1_000_000 {
        return Value::Error(ErrorKind::Num);
    }
    let mut data = Vec::with_capacity(range.cell_count() as usize);
    for r in range.start.row..=range.end.row {
        for c in range.start.col..=range.end.col {
            data.push(get(CellRef::new(r, c)));
        }
    }
    Matrix::new(range.rows() as usize, range.cols() as usize, data).into_value()
}

/// What a name of a linked book stands for (`[1]!Total`,
/// `[1]Sheet1!Local`), with its references pointing into that book.
pub fn name_expr(books: &[ExternalBook], text: &str) -> Option<Arc<Expr>> {
    let (prefix, name) = text.rsplit_once('!')?;
    let prefix = prefix.trim_matches('\'');
    let (key, sheet) = split_external(prefix)?;
    let (bi, book) = find_book(books, key)?;
    let local = if sheet.is_empty() { None } else { Some(book.sheet(sheet)?) };
    let (_, _, refers) = book
        .names
        .iter()
        .find(|(n, s, _)| n.eq_ignore_ascii_case(name) && *s == local)
        .or_else(|| book.names.iter().find(|(n, s, _)| n.eq_ignore_ascii_case(name) && s.is_none()))?;
    let mut e = crate::formula::parse_canonical(refers.trim_start_matches('=')).ok()?;
    let home = local.map(|i| book.sheets[i].clone());
    e.visit_refs_mut(&mut |r| {
        if let Expr::Ref { sheet, .. } | Expr::Range { sheet, .. } = r {
            let own = sheet.as_deref().map(str::to_string).or_else(|| home.clone()).unwrap_or_default();
            *sheet = Some(Arc::from(format!("[{}]{own}", bi + 1)));
        }
    });
    Some(Arc::new(e))
}

/// Numbers the books that references name by file, adding the new ones.
fn index_books(books: &mut Vec<ExternalBook>, e: &mut Expr) -> bool {
    let mut changed = false;
    e.visit_refs_mut(&mut |r| {
        let (Expr::Ref { sheet: Some(s), .. } | Expr::Range { sheet: Some(s), .. }) = r else { return };
        let Some((key, name)) = split_external(s) else { return };
        if key.parse::<usize>().is_ok() {
            return;
        }
        let i = match find_book(books, key) {
            Some((i, _)) => i,
            None => {
                books.push(ExternalBook { target: key.to_string(), ..Default::default() });
                books.len() - 1
            }
        };
        let b = &mut books[i];
        if !name.is_empty() && b.sheet(name).is_none() {
            b.sheets.push(name.to_string());
            b.cells.push(BTreeMap::new());
        }
        *s = Arc::from(format!("[{}]{name}", i + 1));
        changed = true;
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> ExternalBook {
        let mut cells = BTreeMap::new();
        cells.insert(CellRef::new(0, 0), Value::Number(5.0));
        cells.insert(CellRef::new(0, 1), Value::Number(7.0));
        ExternalBook { target: "file:///home/u/Budget.xlsx".into(), sheets: vec!["Data".into()], names: vec![("Total".into(), None, "Data!$A$1:$B$1".into())], cells: vec![cells] }
    }

    #[test]
    fn values_and_names() {
        let books = [book()];
        assert_eq!(value(&books, "[1]Data", &Range::single(CellRef::new(0, 1))), Value::Number(7.0));
        assert_eq!(value(&books, "[Budget.xlsx]data", &Range::single(CellRef::new(0, 0))), Value::Number(5.0));
        assert_eq!(value(&books, "[2]Data", &Range::single(CellRef::new(0, 0))), Value::Error(ErrorKind::Ref));
        assert_eq!(name_expr(&books, "[1]!Total").unwrap().to_canonical(), "[1]Data!$A$1:$B$1");
    }
}

impl crate::Workbook {
    /// Makes every reference to another workbook use its number (`[1]`),
    /// adding the books named by file (`[Budget.xlsx]Sheet1`, as some
    /// programs write them) to [`Self::externals`]. Call after loading.
    pub fn normalize_externals(&mut self) {
        let mut books = std::mem::take(&mut self.externals);
        let mut fix = |e: &mut Expr| index_books(&mut books, e);
        for sh in &mut self.sheets {
            // Copies of one formula follow each other: its tree is looked at once.
            let mut last: Option<(*const Expr, bool)> = None;
            let mut external = |e: &Expr| match last {
                Some((p, x)) if std::ptr::eq(p, e) => x,
                _ => last.insert((e, e.has_external())).1,
            };
            let keys: Vec<CellRef> = sh.iter().filter(|(_, c)| c.formula().is_some_and(|f| external(f.shape()))).map(|(c, _)| c).collect();
            for c in keys {
                if let Some(cell) = sh.get_mut(c) {
                    if let crate::Content::Formula { expr, .. } = &mut cell.content {
                        let mut e = expr.expr().into_owned();
                        if fix(&mut e) {
                            *expr = e.into();
                        }
                    }
                }
            }
        }
        for d in &mut self.names {
            let mut e = (*d.expr).clone();
            if fix(&mut e) {
                d.expr = Arc::new(e);
            }
        }
        self.externals = books;
    }

    /// Makes a typed formula's references to other books use their number
    /// (`[Budget.xlsx]Sheet1!A1` → `[1]Sheet1!A1`).
    pub fn link_formula(&mut self, e: &mut Expr) -> bool {
        e.has_external() && index_books(&mut self.externals, e)
    }

    /// A copy of `e` for people: references to other books show the file
    /// name (`[Budget.xlsx]Sheet1!A1`), as Excel does.
    pub fn externals_by_name(&self, e: &Expr) -> Expr {
        let mut e = e.clone();
        e.visit_refs_mut(&mut |r| {
            let (Expr::Ref { sheet: Some(s), .. } | Expr::Range { sheet: Some(s), .. }) = r else { return };
            let Some((key, name)) = split_external(s) else { return };
            if let Some((_, b)) = find_book(&self.externals, key).filter(|_| key.parse::<usize>().is_ok()) {
                *s = Arc::from(format!("[{}]{name}", b.file_name()));
            }
        });
        e
    }

    /// A copy of `e` whose references to other books name the linked file
    /// (`[file:///home/u/Budget.ods]Sheet1`), as OpenDocument writes them.
    pub fn externals_by_file(&self, e: &Expr) -> Expr {
        let mut e = e.clone();
        e.visit_refs_mut(&mut |r| {
            let (Expr::Ref { sheet: Some(s), .. } | Expr::Range { sheet: Some(s), .. }) = r else { return };
            let Some((key, name)) = split_external(s) else { return };
            if let Some((_, b)) = find_book(&self.externals, key).filter(|_| key.parse::<usize>().is_ok()) {
                *s = Arc::from(format!("[{}]{name}", b.target));
            }
        });
        e
    }
}

/// The book number a sheet prefix (`[2]Sheet1`) or an external name
/// (`[2]!Total`, `'[2]Sheet 1'!Local`) points to, from 0.
fn book_of(text: &str) -> Option<usize> {
    let (key, _) = split_external(text.trim_start_matches('\''))?;
    key.parse::<usize>().ok()?.checked_sub(1)
}

/// `text` pointing to book `to` (from 0) instead of the one it names.
fn renumber(text: &str, to: usize) -> String {
    let quoted = text.starts_with('\'');
    let rest = text.trim_start_matches('\'').split_once(']').map_or("", |(_, r)| r);
    format!("{}[{}]{rest}", if quoted { "'" } else { "" }, to + 1)
}

/// The cells a reference covers, with whole columns and rows cut to `extent`.
fn ref_range(e: &Expr, extent: CellRef) -> Option<Range> {
    use crate::formula::RangeKind;
    match e {
        Expr::Ref { addr, .. } => Some(Range::single(CellRef::new(addr.row, addr.col))),
        Expr::Range { a, b, kind, .. } => {
            let (a, b) = match kind {
                RangeKind::Cells => (CellRef::new(a.row, a.col), CellRef::new(b.row, b.col)),
                RangeKind::Cols => (CellRef::new(0, a.col), CellRef::new(extent.row, b.col)),
                RangeKind::Rows => (CellRef::new(a.row, 0), CellRef::new(b.row, extent.col)),
            };
            Some(Range::new(a, b))
        }
        _ => None,
    }
}

/// How updating a linked book went.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinkUpdate {
    /// Sheets formulas use that the source no longer has.
    pub missing_sheets: Vec<String>,
}

impl crate::Workbook {
    /// Every formula of the workbook (cells and names) with where it is.
    fn all_formulas(&self) -> impl Iterator<Item = std::borrow::Cow<'_, Expr>> {
        let cells = self.sheets.iter().flat_map(|sh| sh.iter().filter_map(|(_, c)| c.formula().map(|e| e.expr())));
        cells.chain(self.names.iter().map(|d| std::borrow::Cow::Borrowed(&*d.expr)))
    }

    /// Whether `e` refers to linked book `book`, directly or by one of its names.
    fn uses_book(e: &Expr, book: usize) -> bool {
        let mut found = false;
        e.visit_refs(&mut |r| {
            if let Expr::Ref { sheet: Some(s), .. } | Expr::Range { sheet: Some(s), .. } = r {
                found |= book_of(s) == Some(book);
            }
        });
        e.visit_names(&mut |n| found |= n.contains(']') && book_of(n) == Some(book));
        found
    }

    /// How many cells and names use each linked book.
    pub fn external_use_counts(&self) -> Vec<usize> {
        let mut counts = vec![0; self.externals.len()];
        for e in self.all_formulas() {
            for (i, n) in counts.iter_mut().enumerate() {
                if Self::uses_book(&e, i) {
                    *n += 1;
                }
            }
        }
        counts
    }

    /// Takes the values formulas use from `src`, the linked book `book` as it
    /// is now, and recalculates.
    pub fn update_external(&mut self, book: usize, src: &crate::Workbook) -> LinkUpdate {
        let mut out = LinkUpdate::default();
        let Some(b) = self.externals.get(book) else { return out };
        // What formulas use: (sheet, reference), including through the
        // book's names.
        let mut uses: Vec<(String, Expr)> = Vec::new();
        let mut collect = |e: &Expr, home: Option<&str>| {
            e.visit_refs(&mut |r| {
                let (Expr::Ref { sheet, .. } | Expr::Range { sheet, .. }) = r else { return };
                match sheet.as_deref() {
                    Some(s) if is_external(s) => {
                        if book_of(s) == Some(book) {
                            uses.push((split_external(s).map_or("", |x| x.1).to_string(), r.clone()));
                        }
                    }
                    Some(s) => {
                        if home.is_some() {
                            uses.push((s.to_string(), r.clone()));
                        }
                    }
                    None => {
                        if let Some(h) = home {
                            uses.push((h.to_string(), r.clone()));
                        }
                    }
                }
            })
        };
        for e in self.all_formulas() {
            collect(&e, None);
        }
        let names: Vec<(String, Option<usize>, String)> = src
            .names
            .iter()
            .filter_map(|d| {
                let scope = match &d.scope {
                    None => None,
                    Some(s) => Some(b.sheet(s)?),
                };
                Some((d.name.clone(), scope, d.expr.to_canonical()))
            })
            .collect();
        for (_, scope, text) in &names {
            if let Ok(e) = crate::formula::parse_canonical(text.trim_start_matches('=')) {
                let home = scope.map(|i| b.sheets[i].as_str()).unwrap_or("");
                collect(&e, Some(home));
            }
        }
        let mut sheets = b.sheets.clone();
        for (s, _) in &uses {
            if !s.is_empty() && !sheets.iter().any(|x| x.eq_ignore_ascii_case(s)) {
                sheets.push(s.clone());
            }
        }
        let mut cells: Vec<BTreeMap<CellRef, Value>> = vec![BTreeMap::new(); sheets.len()];
        let mut budget = 1_000_000usize;
        for (s, r) in &uses {
            let Some(si) = sheets.iter().position(|x| x.eq_ignore_ascii_case(s)) else { continue };
            let Some(sh) = src.sheets.iter().find(|x| x.name.eq_ignore_ascii_case(s)) else {
                if !out.missing_sheets.iter().any(|x| x.eq_ignore_ascii_case(s)) {
                    out.missing_sheets.push(s.clone());
                }
                continue;
            };
            let Some(extent) = sh.extent() else { continue };
            let Some(range) = ref_range(r, extent) else { continue };
            let (r1, c1) = (range.end.row.min(extent.row), range.end.col.min(extent.col));
            for row in range.start.row..=r1 {
                for col in range.start.col..=c1 {
                    let c = CellRef::new(row, col);
                    let v = sh.value(c);
                    if *v != Value::Empty && budget > 0 {
                        budget -= 1;
                        cells[si].insert(c, v.clone());
                    }
                }
            }
        }
        let b = &mut self.externals[book];
        b.sheets = sheets;
        b.cells = cells;
        if !names.is_empty() {
            b.names = names;
        }
        self.modified = true;
        self.recalc_all();
        out
    }

    /// Points linked book `book` to another file.
    pub fn change_external_source(&mut self, book: usize, target: &str) {
        if let Some(b) = self.externals.get_mut(book) {
            b.target = target.to_string();
            self.modified = true;
        }
    }

    /// Replaces every formula that uses linked book `book` with its value
    /// and forgets the book (one undo step).
    pub fn break_external(&mut self, book: usize) {
        if book >= self.externals.len() {
            return;
        }
        let fix = |e: &Expr| -> Expr {
            let mut e = e.clone();
            e.visit_refs_mut(&mut |r| {
                let (Expr::Ref { sheet: Some(s), .. } | Expr::Range { sheet: Some(s), .. }) = r else { return };
                if let Some(i) = book_of(s).filter(|&i| i > book) {
                    *s = Arc::from(renumber(s, i - 1));
                }
            });
            e.visit_names_mut(&mut |n| {
                if let Some(i) = book_of(n).filter(|&i| n.contains(']') && i > book) {
                    *n = Arc::from(renumber(n, i - 1));
                }
            });
            e
        };
        // Names that use the book go; so do the formulas that use them.
        let gone: Vec<String> = self.names.iter().filter(|d| Self::uses_book(&d.expr, book)).map(|d| d.name.to_lowercase()).collect();
        let mut ops = Vec::new();
        for (si, sh) in self.sheets.iter().enumerate() {
            let mut changed = Vec::new();
            for (c, cell) in sh.iter() {
                let Some(e) = cell.formula().map(|f| f.expr()) else { continue };
                let mut via_name = false;
                e.visit_names(&mut |n| via_name |= gone.iter().any(|g| n.eq_ignore_ascii_case(g) || n.to_lowercase().ends_with(&format!("!{g}"))));
                if via_name || Self::uses_book(&e, book) {
                    changed.push((c, crate::sheet::Cell::with_content(Some(cell), crate::sheet::Content::Value(cell.value().clone()))));
                } else if e.has_external() {
                    let f = fix(&e);
                    if f != *e {
                        changed.push((c, crate::sheet::Cell::with_content(Some(cell), crate::sheet::Content::Formula { expr: f.into(), value: cell.value().clone() })));
                    }
                }
            }
            if !changed.is_empty() {
                ops.push(crate::Op::SetCells { sheet: si, cells: changed });
            }
        }
        let names: Vec<crate::names::DefinedName> = self
            .names
            .iter()
            .filter(|d| !gone.contains(&d.name.to_lowercase()))
            .map(|d| crate::names::DefinedName { expr: Arc::new(fix(&d.expr)), ..d.clone() })
            .collect();
        ops.push(crate::Op::Names(names));
        let mut books = self.externals.clone();
        books.remove(book);
        ops.push(crate::Op::Externals(books));
        self.execute_as(crate::Op::Batch(ops), "undo.break_link");
    }
}

#[cfg(test)]
mod link_tests {
    use crate::Workbook;
    use crate::addr::CellRef;
    use crate::value::Value;

    fn a(r: u32, c: u32) -> CellRef {
        CellRef::new(r, c)
    }

    fn linked() -> Workbook {
        let mut wb = Workbook::new();
        wb.set_input(0, a(0, 0), "=[Budget.xlsx]Data!A1*2");
        wb.set_input(0, a(1, 0), "=SUM([Other.xlsx]S!B1:B3)");
        wb.set_input(0, a(2, 0), "=SUM([Budget.xlsx]Data!B:B)");
        wb
    }

    #[test]
    fn update_takes_values_from_the_source() {
        let mut wb = linked();
        assert_eq!(wb.externals.len(), 2);
        let mut src = Workbook::new();
        src.sheets[0].name = "Data".into();
        src.set_input(0, a(0, 0), "21");
        src.set_input(0, a(4, 1), "5");
        src.set_input(0, a(9, 1), "6");
        let r = wb.update_external(0, &src);
        assert!(r.missing_sheets.is_empty());
        assert_eq!(*wb.sheets[0].value(a(0, 0)), Value::Number(42.0));
        assert_eq!(*wb.sheets[0].value(a(2, 0)), Value::Number(11.0));
        assert_eq!(wb.external_use_counts(), vec![2, 1]);
        let mut other = Workbook::new();
        other.sheets[0].name = "Nope".into();
        assert_eq!(wb.update_external(1, &other).missing_sheets, vec!["S".to_string()]);
    }

    #[test]
    fn breaking_a_link_keeps_values_and_renumbers() {
        let mut wb = linked();
        let mut src = Workbook::new();
        src.sheets[0].name = "Data".into();
        src.set_input(0, a(0, 0), "21");
        wb.update_external(0, &src);
        wb.break_external(0);
        assert_eq!(wb.externals.len(), 1);
        let c = wb.sheets[0].get(a(0, 0)).unwrap();
        assert!(c.formula().is_none());
        assert_eq!(*c.value(), Value::Number(42.0));
        let f = wb.sheets[0].get(a(1, 0)).unwrap().formula().unwrap().expr().to_canonical();
        assert_eq!(f, "SUM([1]S!B1:B3)");
        assert!(wb.undo());
        assert_eq!(wb.externals.len(), 2);
        assert!(wb.sheets[0].get(a(0, 0)).unwrap().formula().is_some());
    }
}
