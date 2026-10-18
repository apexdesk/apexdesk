//! Autofilter, data validation and conditional formatting on a workbook.

use super::*;
use crate::data::{AutoFilter, CfKind, CfResult, Cfvo, CmpOp, FilterKind, TextOp, ValKind, Validation, icon_count};
use crate::functions::Criteria;

/// A column filter made ready to test rows.
enum ColTest {
    Values(HashSet<String>, bool),
    Custom(bool, Criteria, Option<Criteria>),
    AtLeast(f64),
    AtMost(f64),
    Above(f64),
    Below(f64),
    Color(bool, Option<u32>),
}

/// Conditional formatting of a sheet prepared for painting: range
/// statistics are computed once per revision.
pub struct CfCache {
    rules: Vec<Prep>,
}

enum Prep {
    /// Formulas relative to `origin`.
    Formulas(Option<Expr>, Option<Expr>, CellRef),
    Nothing,
    Top(f64),
    Average(f64),
    Dups(HashMap<DupKey, u32>),
    Scale(Vec<(f64, u32)>),
    Bar(f64, f64),
    Icons(Vec<f64>),
}

#[derive(Hash, PartialEq, Eq)]
enum DupKey {
    Num(u64),
    Text(String),
    Bool(bool),
}

/// Cells the conditional formats (or the validations) of a sheet read in
/// all each time they are worked out: a file may give any number of rules
/// over whole sheets.
const MAX_RULE_WORK: u64 = 1 << 27;

/// Takes `cost` from `left`, false when it isn't there.
fn afford(left: &mut u64, cost: u64) -> bool {
    let ok = cost <= *left;
    *left = left.saturating_sub(cost);
    ok
}

fn dup_key(v: &Value) -> Option<DupKey> {
    match v {
        Value::Number(n) => Some(DupKey::Num(n.to_bits())),
        Value::Text(t) if !t.is_empty() => Some(DupKey::Text(t.to_lowercase())),
        Value::Bool(b) => Some(DupKey::Bool(*b)),
        _ => None,
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Number(n) => *n != 0.0,
        Value::Bool(b) => *b,
        Value::Text(t) => t.eq_ignore_ascii_case("TRUE"),
        _ => false,
    }
}

fn is_blank(v: &Value) -> bool {
    match v {
        Value::Empty => true,
        Value::Text(t) => t.trim().is_empty(),
        _ => false,
    }
}

/// `p`-th percentile (0..=1) of sorted numbers, interpolated like PERCENTILE.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let x = p.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let i = x.floor() as usize;
    let f = x - i as f64;
    match sorted.get(i + 1) {
        Some(n) => sorted[i] + (n - sorted[i]) * f,
        None => sorted[i],
    }
}

fn lerp_rgb(a: u32, b: u32, t: f64) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let ch = |s: u32| {
        let x = ((a >> s) & 0xFF) as f64;
        let y = ((b >> s) & 0xFF) as f64;
        ((x + (y - x) * t).round() as u32) << s
    };
    ch(16) | ch(8) | ch(0)
}

impl Workbook {
    // --- evaluation helpers ------------------------------------------------

    /// Evaluates a canonical formula written for `origin` as if it were in `at`.
    fn eval_relative(&self, sheet: usize, expr: &Expr, origin: CellRef, at: CellRef) -> Value {
        let env = Env::new(self, sheet, at, self.locale);
        if origin == at {
            return env.eval(expr);
        }
        let mut e = expr.clone();
        e.offset(at.row as i64 - origin.row as i64, at.col as i64 - origin.col as i64);
        env.eval(&e)
    }

    fn parse_opt(src: &str) -> Option<Expr> {
        let s = src.trim();
        let s = s.strip_prefix('=').unwrap_or(s);
        if s.is_empty() { None } else { formula::parse_canonical(s).ok() }
    }

    /// Numeric values of the cells in `ranges`, sorted.
    fn numbers_in(&self, sheet: usize, ranges: &[Range]) -> Vec<f64> {
        let mut v = Vec::new();
        for r in ranges {
            let Some(r) = self.clamp_to_used(sheet, r) else { continue };
            self.sheets[sheet].for_each_in(&r, |_, cell| {
                if let Value::Number(n) = cell.value() {
                    v.push(*n);
                }
            });
        }
        v.sort_by(|a, b| a.total_cmp(b));
        v
    }

    fn cfvo(&self, sheet: usize, c: &Cfvo, nums: &[f64], origin: CellRef) -> f64 {
        let (min, max) = (nums.first().copied().unwrap_or(0.0), nums.last().copied().unwrap_or(0.0));
        match c {
            Cfvo::Min => min,
            Cfvo::Max => max,
            Cfvo::Num(n) => *n,
            Cfvo::Percent(p) => min + (max - min) * p / 100.0,
            Cfvo::Percentile(p) => percentile(nums, p / 100.0),
            Cfvo::Formula(f) => Self::parse_opt(f).and_then(|e| self.eval_relative(sheet, &e, origin, origin).as_number()).unwrap_or(0.0),
        }
    }


    // --- rule operands ---------------------------------------------------------

    /// Stored (canonical) form of a rule operand typed by the user: `=...`
    /// is a formula, a number or date becomes a number, other text a string.
    pub fn rule_input(&self, text: &str) -> String {
        let t = text.trim();
        if t.is_empty() {
            return String::new();
        }
        if t.len() > 1 && t.starts_with('=') {
            return formula::parse(t, &self.locale).map(|e| e.to_canonical()).unwrap_or_else(|_| t[1..].to_string());
        }
        match format::parse_input(t, &self.locale).value {
            Value::Number(n) => Expr::Number(n).to_canonical(),
            Value::Bool(b) => Expr::Bool(b).to_canonical(),
            _ => Expr::Text(t.into()).to_canonical(),
        }
    }

    /// Text for editing a stored rule operand.
    pub fn rule_display(&self, stored: &str) -> String {
        if stored.is_empty() {
            return String::new();
        }
        match formula::parse_canonical(stored) {
            Ok(Expr::Number(n)) => self.format_value(0, &Value::Number(n)).0,
            Ok(Expr::Text(t)) => t.to_string(),
            Ok(e) => e.to_display(&self.locale),
            Err(_) => stored.to_string(),
        }
    }

    // --- autofilter --------------------------------------------------------

    fn col_test(&self, sheet: usize, f: &AutoFilter, col: u32, kind: &FilterKind) -> ColTest {
        let loc = Locale::EN;
        match kind {
            FilterKind::Values { items, blanks } => ColTest::Values(items.iter().map(|s| s.to_lowercase()).collect(), *blanks),
            FilterKind::Custom { and, a, b } => ColTest::Custom(*and, Criteria::with_op(a.0, &a.1, &loc), b.as_ref().map(|b| Criteria::with_op(b.0, &b.1, &loc))),
            FilterKind::Top { top, percent, n } => {
                let data = f.data_rows().map(|r| Range::new(CellRef::new(r.start.row, col), CellRef::new(r.end.row, col)));
                let mut nums = data.map(|r| self.numbers_in(sheet, &[r])).unwrap_or_default();
                if *top {
                    nums.reverse();
                }
                let count = if *percent { (nums.len() as f64 * n / 100.0).floor() as usize } else { *n as usize }.max(1);
                let t = nums.get(count.min(nums.len()).saturating_sub(1)).copied().unwrap_or(f64::NAN);
                if *top { ColTest::AtLeast(t) } else { ColTest::AtMost(t) }
            }
            FilterKind::Average { above } => {
                let data = f.data_rows().map(|r| Range::new(CellRef::new(r.start.row, col), CellRef::new(r.end.row, col)));
                let nums = data.map(|r| self.numbers_in(sheet, &[r])).unwrap_or_default();
                let avg = if nums.is_empty() { f64::NAN } else { nums.iter().sum::<f64>() / nums.len() as f64 };
                if *above { ColTest::Above(avg) } else { ColTest::Below(avg) }
            }
            FilterKind::Color { font, color } => ColTest::Color(*font, *color),
        }
    }

    fn passes(&self, sheet: usize, test: &ColTest, c: CellRef) -> bool {
        let v = self.sheets[sheet].value(c);
        match test {
            ColTest::Values(set, blanks) => {
                if is_blank(v) {
                    return *blanks;
                }
                set.contains(&self.display(sheet, c).0.to_lowercase())
            }
            ColTest::Custom(and, a, b) => {
                let ra = a.matches(v);
                match b {
                    None => ra,
                    Some(b) if *and => ra && b.matches(v),
                    Some(b) => ra || b.matches(v),
                }
            }
            ColTest::AtLeast(t) => v.as_number().is_some_and(|n| n >= *t),
            ColTest::AtMost(t) => v.as_number().is_some_and(|n| n <= *t),
            ColTest::Above(t) => v.as_number().is_some_and(|n| n > *t),
            ColTest::Below(t) => v.as_number().is_some_and(|n| n < *t),
            ColTest::Color(font, color) => {
                let st = self.sheets[sheet].get(c).map(|x| self.styles.get(x.style));
                let has = st.and_then(|s| if *font { s.color } else { s.fill });
                has == *color
            }
        }
    }

    /// Data rows of `f` that its column filters hide, skipping `except`.
    fn filtered_out(&self, sheet: usize, f: &AutoFilter, except: Option<u32>) -> Vec<u32> {
        let Some(data) = f.data_rows() else { return Vec::new() };
        let tests: Vec<(u32, ColTest)> = f.cols.iter().filter(|c| Some(c.col) != except).map(|c| (c.col, self.col_test(sheet, f, c.col, &c.kind))).collect();
        if tests.is_empty() {
            return Vec::new();
        }
        let last = self.sheets[sheet].extent().map_or(data.start.row, |e| e.row).min(data.end.row);
        (data.start.row..=last).filter(|&r| !tests.iter().all(|(col, t)| self.passes(sheet, t, CellRef::new(r, *col)))).collect()
    }

    /// Sets (or removes) the sheet's filter and hides the rows it filters out.
    pub fn set_filter(&mut self, sheet: usize, f: Option<AutoFilter>, label: &'static str) {
        let old = self.sheets[sheet].filter.clone();
        let hide: HashSet<u32> = f.as_ref().map(|f| self.filtered_out(sheet, f, None).into_iter().collect()).unwrap_or_default();
        let mut rows: Vec<u32> = Vec::new();
        for fl in [&old, &f].into_iter().flatten() {
            if let Some(d) = fl.data_rows() {
                let last = self.sheets[sheet].extent().map_or(d.start.row, |e| e.row).min(d.end.row).max(d.start.row);
                rows.extend(d.start.row..=last);
            }
        }
        // Rows hidden before the filter reached them may sit past the data.
        rows.extend(self.sheets[sheet].row_heights.iter().filter(|(_, h)| **h == 0.0).map(|(r, _)| *r).filter(|r| old.as_ref().is_some_and(|o| o.range.contains(CellRef::new(*r, o.range.start.col)))));
        rows.sort_unstable();
        rows.dedup();
        let sh = &self.sheets[sheet];
        let mut sizes = Vec::new();
        for r in rows {
            let hidden = sh.row_heights.get(&r) == Some(&0.0);
            let want = hide.contains(&r);
            if want && !hidden {
                sizes.push((r, Some(0.0)));
            } else if !want && hidden {
                sizes.push((r, None));
            }
        }
        let mut ops = vec![Op::Filter { sheet, filter: f }];
        if !sizes.is_empty() {
            ops.push(Op::RowHeights { sheet, sizes });
        }
        self.execute_as(Op::Batch(ops), label);
    }

    /// Filters the data again after it changed.
    pub fn reapply_filter(&mut self, sheet: usize) {
        let f = self.sheets[sheet].filter.clone();
        if f.is_some() {
            self.set_filter(sheet, f, "undo.filter");
        }
    }

    /// Distinct displayed values of a filter column, sorted, among the rows
    /// the other columns let through; and whether there are blanks.
    pub fn filter_values(&self, sheet: usize, f: &AutoFilter, col: u32) -> (Vec<String>, bool) {
        let Some(data) = f.data_rows() else { return (Vec::new(), false) };
        let out: HashSet<u32> = self.filtered_out(sheet, f, Some(col)).into_iter().collect();
        let mut seen: HashMap<String, Value> = HashMap::default();
        let mut blanks = false;
        let last = self.sheets[sheet].extent().map_or(data.start.row, |e| e.row).min(data.end.row);
        for r in data.start.row..=last.max(data.start.row) {
            if out.contains(&r) {
                continue;
            }
            let c = CellRef::new(r, col);
            let v = self.sheets[sheet].value(c);
            if is_blank(v) {
                blanks = true;
                continue;
            }
            seen.entry(self.display(sheet, c).0.to_lowercase()).or_insert_with(|| v.clone());
        }
        let mut items: Vec<(String, Value)> = Vec::with_capacity(seen.len());
        // Keep the first spelling of each value.
        let mut firsts: HashMap<String, String> = HashMap::default();
        for r in data.start.row..=last.max(data.start.row) {
            let c = CellRef::new(r, col);
            if out.contains(&r) || is_blank(self.sheets[sheet].value(c)) {
                continue;
            }
            let t = self.display(sheet, c).0;
            firsts.entry(t.to_lowercase()).or_insert(t);
        }
        for (k, v) in seen {
            items.push((firsts.remove(&k).unwrap_or(k), v));
        }
        items.sort_by(|a, b| a.1.compare(&b.1).then_with(|| a.0.cmp(&b.0)));
        (items.into_iter().map(|x| x.0).collect(), blanks)
    }

    /// Whether the rows `f` filters out include `row` (for painting buttons).
    pub fn filter_hides(&self, sheet: usize, row: u32) -> bool {
        self.sheets[sheet].filter.as_ref().is_some_and(|f| f.data_rows().is_some_and(|d| d.contains(CellRef::new(row, d.start.col)))) && self.sheets[sheet].row_height(row) == 0.0
    }

    /// Sorts the filter's data rows by one column.
    pub fn filter_sort(&mut self, sheet: usize, col: u32, ascending: bool) {
        let Some(mut f) = self.sheets[sheet].filter.clone() else { return };
        let Some(data) = f.data_rows() else { return };
        let last = self.sheets[sheet].extent().map_or(data.start.row, |e| e.row).min(data.end.row);
        let r = Range::new(data.start, CellRef::new(last.max(data.start.row), data.end.col));
        self.sort_by(sheet, &r, &[SortKey::new(col, ascending)], false);
        f.sort = Some((col, ascending));
        self.set_filter(sheet, Some(f), "undo.sort");
        // One undo step for both.
        if let (Some((b, _)), Some((a, l))) = (self.undo.pop(), self.undo.pop()) {
            self.undo.push((Op::Batch(vec![b, a]), l));
        }
    }

    // --- data validation ------------------------------------------------------

    pub fn validation_at(&self, sheet: usize, c: CellRef) -> Option<&Validation> {
        self.sheets.get(sheet)?.validations.iter().find(|v| v.contains(c))
    }

    fn val_origin(v: &Validation) -> CellRef {
        v.ranges.iter().map(|r| r.start).min_by_key(|c| (c.row, c.col)).unwrap_or_default()
    }

    /// Choices of a list validation for cell `c`.
    pub fn validation_items(&self, sheet: usize, c: CellRef) -> Vec<String> {
        let Some(v) = self.validation_at(sheet, c) else { return Vec::new() };
        if v.kind != ValKind::List {
            return Vec::new();
        }
        if let Some(items) = v.literal_items() {
            return items;
        }
        let Some(mut e) = Self::parse_opt(&v.f1) else { return Vec::new() };
        let o = Self::val_origin(v);
        e.offset(c.row as i64 - o.row as i64, c.col as i64 - o.col as i64);
        let mut out = Vec::new();
        let mut e = &e;
        while let Expr::Paren(inner) = e {
            e = inner;
        }
        match e {
            Expr::Range { sheet: s, a, b, .. } => {
                let si = match s {
                    Some(n) => self.sheet_index(n),
                    None => Some(sheet),
                };
                if let Some(si) = si {
                    let r = Range::new(CellRef::new(a.row, a.col), CellRef::new(b.row, b.col));
                    if let Some(r) = self.clamp_to_used(si, &r) {
                        for row in r.start.row..=r.end.row {
                            for col in r.start.col..=r.end.col {
                                let t = self.display(si, CellRef::new(row, col)).0;
                                if !t.is_empty() {
                                    out.push(t);
                                }
                            }
                        }
                    }
                }
            }
            Expr::Ref { sheet: s, addr } => {
                let si = match s {
                    Some(n) => self.sheet_index(n),
                    None => Some(sheet),
                };
                if let Some(si) = si {
                    out.push(self.display(si, CellRef::new(addr.row, addr.col)).0);
                }
            }
            other => {
                if let Ok(t) = Env::new(self, sheet, c, self.locale).eval(other).to_text() {
                    out.extend(t.split(',').map(|s| s.trim().to_string()));
                }
            }
        }
        out
    }

    /// Whether cell `c` satisfies its validation rule (true without one).
    pub fn is_valid(&self, sheet: usize, c: CellRef) -> bool {
        let Some(v) = self.validation_at(sheet, c) else { return true };
        let val = self.sheets[sheet].value(c);
        if v.kind == ValKind::Any {
            return true;
        }
        if is_blank(val) {
            return v.allow_blank;
        }
        let o = Self::val_origin(v);
        let bound = |f: &str| Self::parse_opt(f).map(|e| self.eval_relative(sheet, &e, o, c));
        let num_test = |n: f64| {
            let a = bound(&v.f1).and_then(|x| x.as_number());
            let b = bound(&v.f2).and_then(|x| x.as_number());
            match (a, b) {
                (Some(a), Some(b)) => v.op.test(n, a, b),
                (Some(a), None) if !v.op.two_args() => v.op.test(n, a, a),
                _ => true,
            }
        };
        match v.kind {
            ValKind::Any => true,
            ValKind::Whole => val.as_number().is_some_and(|n| n.fract() == 0.0 && num_test(n)),
            ValKind::Decimal | ValKind::Date | ValKind::Time => val.as_number().is_some_and(num_test),
            ValKind::TextLength => {
                let len = match val {
                    Value::Text(t) => t.chars().count(),
                    _ => self.display(sheet, c).0.chars().count(),
                };
                num_test(len as f64)
            }
            ValKind::List => {
                let text = self.display(sheet, c).0;
                self.validation_items(sheet, c).iter().any(|i| i.to_lowercase() == text.to_lowercase() || (val.as_number().is_some() && format::parse_number(i, &self.locale) == val.as_number()))
            }
            ValKind::Custom => bound(&v.f1).is_none_or(|x| truthy(&x)),
        }
    }

    /// Cells with a value that breaks their validation rule.
    pub fn invalid_cells(&self, sheet: usize) -> Vec<CellRef> {
        let mut out = Vec::new();
        let mut left = MAX_RULE_WORK;
        for v in &self.sheets[sheet].validations {
            for r in &v.ranges {
                let Some(r) = self.clamp_to_used(sheet, r) else { continue };
                if !afford(&mut left, r.cell_count()) {
                    break;
                }
                self.sheets[sheet].for_each_in(&r, |c, cell| {
                    if !cell.value().is_empty() {
                        out.push(c);
                    }
                });
            }
        }
        out.retain(|c| !self.is_valid(sheet, *c));
        out.sort_by_key(|c| (c.row, c.col));
        out
    }

    // --- conditional formatting ----------------------------------------------

    pub fn cf_prepare(&self, sheet: usize) -> CfCache {
        let sh = &self.sheets[sheet];
        // Rules reading their ranges, in all no more than so many cells:
        // later ones read nothing.
        let left = std::cell::Cell::new(MAX_RULE_WORK);
        let reads = |ranges: &[Range]| {
            let mut l = left.get();
            let ok = ranges.iter().filter_map(|r| self.clamp_to_used(sheet, r)).all(|r| afford(&mut l, r.cell_count()));
            left.set(if ok { l } else { 0 });
            ok
        };
        let numbers_in = |ranges: &[Range]| if reads(ranges) { self.numbers_in(sheet, ranges) } else { Vec::new() };
        let rules = sh
            .cond_formats
            .iter()
            .map(|rule| {
                let origin = rule.origin();
                match &rule.kind {
                    CfKind::CellIs { f1, f2, .. } => Prep::Formulas(Self::parse_opt(f1), Self::parse_opt(f2), origin),
                    CfKind::Expression(f) => Prep::Formulas(Self::parse_opt(f), None, origin),
                    CfKind::Text { .. } | CfKind::Blanks(_) | CfKind::Errors(_) | CfKind::Period(_) => Prep::Nothing,
                    CfKind::Top { bottom, percent, rank } => {
                        let mut nums = numbers_in(&rule.ranges);
                        if !bottom {
                            nums.reverse();
                        }
                        let n = if *percent { (nums.len() as f64 * *rank as f64 / 100.0).floor() as usize } else { *rank as usize }.max(1);
                        Prep::Top(nums.get(n.min(nums.len()).saturating_sub(1)).copied().unwrap_or(f64::NAN))
                    }
                    CfKind::Average { .. } => {
                        let nums = numbers_in(&rule.ranges);
                        Prep::Average(if nums.is_empty() { f64::NAN } else { nums.iter().sum::<f64>() / nums.len() as f64 })
                    }
                    CfKind::Duplicates(_) => {
                        let mut m: HashMap<DupKey, u32> = HashMap::default();
                        for r in if reads(&rule.ranges) { &rule.ranges[..] } else { &[] } {
                            let Some(r) = self.clamp_to_used(sheet, r) else { continue };
                            sh.for_each_in(&r, |_, cell| {
                                if let Some(k) = dup_key(cell.value()) {
                                    *m.entry(k).or_default() += 1;
                                }
                            });
                        }
                        Prep::Dups(m)
                    }
                    CfKind::ColorScale(stops) => {
                        let nums = numbers_in(&rule.ranges);
                        Prep::Scale(stops.iter().map(|(c, col)| (self.cfvo(sheet, c, &nums, origin), *col)).collect())
                    }
                    CfKind::DataBar { min, max, .. } => {
                        let nums = numbers_in(&rule.ranges);
                        Prep::Bar(self.cfvo(sheet, min, &nums, origin), self.cfvo(sheet, max, &nums, origin))
                    }
                    CfKind::IconSet { points, .. } => {
                        let nums = numbers_in(&rule.ranges);
                        Prep::Icons(points.iter().map(|c| self.cfvo(sheet, c, &nums, origin)).collect())
                    }
                }
            })
            .collect();
        CfCache { rules }
    }

    /// What conditional formatting does to cell `c`.
    pub fn cf_eval(&self, sheet: usize, cache: &CfCache, c: CellRef) -> Option<CfResult> {
        let sh = &self.sheets[sheet];
        let mut out: Option<CfResult> = None;
        let v = sh.value(c);
        for (rule, prep) in sh.cond_formats.iter().zip(&cache.rules) {
            if !rule.contains(c) {
                continue;
            }
            let num = v.as_number();
            let res = out.get_or_insert_with(CfResult::default);
            let hit = match (&rule.kind, prep) {
                (CfKind::CellIs { op, .. }, Prep::Formulas(a, b, o)) => {
                    let a = a.as_ref().map(|e| self.eval_relative(sheet, e, *o, c));
                    let b = b.as_ref().map(|e| self.eval_relative(sheet, e, *o, c));
                    match (v, a) {
                        (Value::Empty, _) | (_, None) => false,
                        (Value::Number(n), Some(Value::Number(x))) => {
                            let y = b.and_then(|b| b.as_number()).unwrap_or(x);
                            op.test(*n, x, y)
                        }
                        (_, Some(a)) => {
                            let ord = v.compare(&a);
                            let ord_b = b.map(|b| v.compare(&b));
                            use std::cmp::Ordering::*;
                            match op {
                                CmpOp::Equal => ord == Equal,
                                CmpOp::NotEqual => ord != Equal,
                                CmpOp::Greater => ord == Greater,
                                CmpOp::Less => ord == Less,
                                CmpOp::GreaterEq => ord != Less,
                                CmpOp::LessEq => ord != Greater,
                                CmpOp::Between => ord != Less && ord_b.is_some_and(|o| o != Greater),
                                CmpOp::NotBetween => ord == Less || ord_b.is_some_and(|o| o == Greater),
                            }
                        }
                    }
                }
                (CfKind::Expression(_), Prep::Formulas(Some(e), _, o)) => truthy(&self.eval_relative(sheet, e, *o, c)),
                (CfKind::Text { op, text }, _) => {
                    let t = self.display(sheet, c).0.to_lowercase();
                    let n = text.to_lowercase();
                    match op {
                        TextOp::Contains => !n.is_empty() && t.contains(&n),
                        TextOp::NotContains => !t.contains(&n),
                        TextOp::BeginsWith => t.starts_with(&n),
                        TextOp::EndsWith => t.ends_with(&n),
                    }
                }
                (CfKind::Blanks(b), _) => is_blank(v) == *b,
                (CfKind::Errors(b), _) => matches!(v, Value::Error(_)) == *b,
                (CfKind::Period(p), _) => num.is_some_and(|n| p.contains(n, format::now_serial())),
                (CfKind::Duplicates(dup), Prep::Dups(m)) => dup_key(v).is_some_and(|k| (m.get(&k).copied().unwrap_or(0) > 1) == *dup),
                (CfKind::Top { bottom, .. }, Prep::Top(t)) => num.is_some_and(|n| if *bottom { n <= *t } else { n >= *t }),
                (CfKind::Average { below, equal }, Prep::Average(a)) => num.is_some_and(|n| match (below, equal) {
                    (false, false) => n > *a,
                    (false, true) => n >= *a,
                    (true, false) => n < *a,
                    (true, true) => n <= *a,
                }),
                (CfKind::ColorScale(_), Prep::Scale(stops)) => {
                    if let (Some(n), Some(first), Some(last)) = (num, stops.first(), stops.last()) {
                        if res.style.fill.is_none() {
                            let color = if n <= first.0 {
                                first.1
                            } else if n >= last.0 {
                                last.1
                            } else {
                                let i = stops.windows(2).position(|w| n >= w[0].0 && n <= w[1].0).unwrap_or(0);
                                let (a, b) = (stops[i], stops[i + 1]);
                                lerp_rgb(a.1, b.1, if b.0 > a.0 { (n - a.0) / (b.0 - a.0) } else { 0.0 })
                            };
                            res.style.fill = Some(color);
                        }
                        true
                    } else {
                        false
                    }
                }
                (CfKind::DataBar { color, .. }, Prep::Bar(lo, hi)) => match num {
                    Some(n) => {
                        if res.bar.is_none() {
                            let f = if hi > lo { ((n - lo) / (hi - lo)).clamp(0.0, 1.0) } else { 1.0 };
                            res.bar = Some((f as f32, *color));
                        }
                        true
                    }
                    None => false,
                },
                (CfKind::IconSet { set, reverse, show_value, .. }, Prep::Icons(points)) => match num {
                    Some(n) => {
                        if res.icon.is_none() {
                            let count = icon_count(set);
                            let mut i = points.iter().filter(|p| n >= **p).count().min(count - 1);
                            if *reverse {
                                i = count - 1 - i;
                            }
                            res.icon = Some((set.clone(), i as u8));
                            res.hide_value |= !show_value;
                        }
                        true
                    }
                    None => false,
                },
                _ => false,
            };
            if hit {
                if !rule.kind.is_graphic() {
                    res.style.merge_under(&rule.style);
                }
                if rule.stop {
                    break;
                }
            }
        }
        out.filter(|r| !r.is_empty())
    }
}

/// How Text to Columns splits a cell.
#[derive(Clone, Debug, PartialEq)]
pub enum SplitMode {
    /// Any of the characters separates fields.
    Delimited { delims: Vec<char>, merge: bool, quote: Option<char> },
    /// Character positions where fields start (after the first).
    Fixed(Vec<usize>),
}

/// Splits `text` into fields for Text to Columns.
pub fn split_text(text: &str, mode: &SplitMode) -> Vec<String> {
    match mode {
        SplitMode::Fixed(breaks) => {
            let chars: Vec<char> = text.chars().collect();
            let mut out = Vec::new();
            let mut at = 0;
            for &b in breaks.iter().filter(|b| **b > 0) {
                let b = b.min(chars.len());
                if b < at {
                    continue;
                }
                out.push(chars[at..b].iter().collect::<String>().trim_end().to_string());
                at = b;
            }
            out.push(chars[at.min(chars.len())..].iter().collect::<String>().trim_end().to_string());
            out
        }
        SplitMode::Delimited { delims, merge, quote } => {
            let mut out = Vec::new();
            let mut cur = String::new();
            let mut quoted = false;
            let mut chars = text.chars().peekable();
            let mut last_delim = false;
            while let Some(ch) = chars.next() {
                if Some(ch) == *quote {
                    if quoted && chars.peek() == Some(&ch) {
                        cur.push(ch);
                        chars.next();
                    } else {
                        quoted = !quoted;
                    }
                    last_delim = false;
                    continue;
                }
                if !quoted && delims.contains(&ch) {
                    if !(*merge && last_delim) {
                        out.push(std::mem::take(&mut cur));
                    }
                    last_delim = true;
                    continue;
                }
                last_delim = false;
                cur.push(ch);
            }
            out.push(cur);
            out
        }
    }
}

impl Workbook {
    /// The block of non-empty cells around `c` (Excel's current region).
    pub fn current_region(&self, sheet: usize, c: CellRef) -> Range {
        let sh = &self.sheets[sheet];
        let filled = |r: u32, col: u32| !sh.value(CellRef::new(r, col)).is_empty();
        let mut r = Range::single(c);
        loop {
            let mut grown = false;
            let (c0, c1) = (r.start.col.saturating_sub(1), (r.end.col + 1).min(crate::MAX_COLS - 1));
            // Rows first, as far as they go: a step per row that looked at
            // the whole height again went quadratic on long tables.
            while r.start.row > 0 && (c0..=c1).any(|col| filled(r.start.row - 1, col)) {
                r.start.row -= 1;
                grown = true;
            }
            while r.end.row < crate::MAX_ROWS - 1 && (c0..=c1).any(|col| filled(r.end.row + 1, col)) {
                r.end.row += 1;
                grown = true;
            }
            let (r0, r1) = (r.start.row.saturating_sub(1), (r.end.row + 1).min(crate::MAX_ROWS - 1));
            if r.start.col > 0 && (r0..=r1).any(|row| filled(row, c0)) {
                r.start.col = c0;
                grown = true;
            }
            if r.end.col < c1 && (r0..=r1).any(|row| filled(row, c1)) {
                r.end.col = c1;
                grown = true;
            }
            if !grown || r.cell_count() > 50_000_000 {
                return r;
            }
        }
    }

    /// Removes rows of `r` whose values in `cols` repeat an earlier row;
    /// the rows below move up inside `r`. Returns (removed, kept).
    pub fn remove_duplicates(&mut self, sheet: usize, r: &Range, cols: &[u32], header: bool) -> (usize, usize) {
        let Some(used) = self.clamp_to_used(sheet, r) else { return (0, 0) };
        let first = if header { r.start.row + 1 } else { r.start.row };
        if first > used.end.row || cols.is_empty() {
            return (0, 0);
        }
        let sh = &self.sheets[sheet];
        let mut seen: HashSet<Vec<String>> = HashSet::default();
        let mut kept = Vec::new();
        for row in first..=used.end.row {
            let key: Vec<String> = cols.iter().map(|&col| self.display(sheet, CellRef::new(row, col)).0.to_lowercase()).collect();
            if seen.insert(key) {
                kept.push(row);
            }
        }
        let total = (used.end.row - first + 1) as usize;
        let removed = total - kept.len();
        if removed == 0 {
            return (0, kept.len());
        }
        let mut cells = Vec::new();
        for (i, &src) in kept.iter().enumerate() {
            let dst = first + i as u32;
            if dst == src {
                continue;
            }
            for col in r.start.col..=r.end.col {
                let cell = sh.get(CellRef::new(src, col)).cloned().map(|c| edit::offset_formula(c, dst as i64 - src as i64, 0));
                cells.push((CellRef::new(dst, col), cell));
            }
        }
        for row in first + kept.len() as u32..=used.end.row {
            for col in r.start.col..=r.end.col {
                if sh.get(CellRef::new(row, col)).is_some() {
                    cells.push((CellRef::new(row, col), None));
                }
            }
        }
        self.execute_as(Op::SetCells { sheet, cells }, "undo.remove_dups");
        (removed, kept.len())
    }

    /// Splits the texts of the first column of `r` into columns starting at
    /// `dest` (the top-left of the output). `skip` lists fields not imported.
    pub fn text_to_columns(&mut self, sheet: usize, r: &Range, mode: &SplitMode, dest: CellRef, skip: &[usize]) {
        let Some(used) = self.clamp_to_used(sheet, r) else { return };
        let mut cells = Vec::new();
        for row in r.start.row..=used.end.row {
            let src = CellRef::new(row, r.start.col);
            let text = self.display(sheet, src).0;
            if text.is_empty() {
                continue;
            }
            let dr = row - r.start.row;
            let mut k = 0;
            for (i, part) in split_text(&text, mode).into_iter().enumerate() {
                if skip.contains(&i) {
                    continue;
                }
                let c = CellRef::new(dest.row + dr, dest.col + k);
                k += 1;
                if c.col >= crate::MAX_COLS {
                    break;
                }
                let (content, fmt) = self.content_from_input(&part);
                let old = self.cell(sheet, c).cloned();
                let style = self.with_format(old.as_ref().map_or(0, |x| x.style), fmt);
                let mut cell = Cell::with_content(old.as_ref(), content);
                if let Some(x) = &mut cell {
                    x.style = style;
                }
                cells.push((c, cell.filter(|x| !x.is_blank())));
            }
        }
        if !cells.is_empty() {
            self.execute_as(Op::SetCells { sheet, cells }, "undo.text_columns");
        }
    }

    /// Distinct texts in the column around `c` (for typing autocomplete),
    /// looking at the contiguous block above and below.
    pub fn column_texts(&self, sheet: usize, c: CellRef) -> Vec<String> {
        let sh = &self.sheets[sheet];
        let filled = |r: u32| !sh.value(CellRef::new(r, c.col)).is_empty();
        let mut a = c.row;
        while a > 0 && filled(a - 1) && c.row - a < 5000 {
            a -= 1;
        }
        let mut b = c.row;
        while b + 1 < crate::MAX_ROWS && filled(b + 1) && b - c.row < 5000 {
            b += 1;
        }
        let mut seen = HashSet::default();
        let mut out = Vec::new();
        for r in a..=b {
            if r == c.row {
                continue;
            }
            if let Value::Text(t) = sh.value(CellRef::new(r, c.col)) {
                if !t.is_empty() && seen.insert(t.to_lowercase()) {
                    out.push(t.to_string());
                }
            }
        }
        out
    }
}

/// Days between the 1900 and the 1904 date systems.
pub const DAYS_1904: f64 = 1462.0;

impl Workbook {
    /// Adds `days` to the numbers shown as dates (constants and cached
    /// formula results; times alone are left alone), for files in the 1904
    /// date system.
    pub fn shift_dates(&mut self, days: f64) {
        let mut dated: HashMap<u32, bool> = HashMap::default();
        for sh in &mut self.sheets {
            let keys: Vec<CellRef> = sh.iter().filter(|(_, c)| matches!(c.value(), Value::Number(_))).map(|(k, _)| k).collect();
            for k in keys {
                let Some(cell) = sh.get_mut(k) else { continue };
                let style = cell.style;
                let date = *dated.entry(style).or_insert_with(|| NumFormat::parse(&self.styles.get(style).num_fmt).has_date_part());
                if !date {
                    continue;
                }
                match &mut cell.content {
                    Content::Value(Value::Number(n)) | Content::Formula { value: Value::Number(n), .. } => *n += days,
                    _ => {}
                }
            }
        }
    }
}

/// Review > Workbook Statistics: counts for a sheet or the whole book.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BookStats {
    /// Last cell with data (one sheet).
    pub last: Option<CellRef>,
    pub sheets: usize,
    pub cells: usize,
    pub formulas: usize,
    pub tables: usize,
    pub pivots: usize,
    pub charts: usize,
    pub images: usize,
    pub shapes: usize,
    pub comments: usize,
    pub notes: usize,
}

impl Workbook {
    /// Statistics of one sheet, or of all of them (`None`).
    pub fn statistics(&self, sheet: Option<usize>) -> BookStats {
        let mut st = BookStats::default();
        for (i, sh) in self.sheets.iter().enumerate().filter(|(i, _)| sheet.is_none_or(|s| s == *i)) {
            st.sheets += 1;
            for (_, c) in sh.iter() {
                if !c.value().is_empty() || c.formula().is_some() {
                    st.cells += 1;
                }
                st.formulas += c.formula().is_some() as usize;
            }
            st.tables += sh.tables.len();
            st.pivots += self.analysis.pivots.iter().filter(|p| p.sheet == sh.name).count();
            st.charts += sh.charts.len();
            st.images += sh.images.len();
            st.shapes += sh.shapes.len();
            for (_, n) in self.notes(i) {
                if n.threaded { st.comments += 1 } else { st.notes += 1 }
            }
            if sheet.is_some() {
                st.last = sh.extent();
            }
        }
        st
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{CfRule, CfStyle, ColumnFilter};
    use crate::functions::CritOp;

    fn wb(rows: &[&[&str]]) -> Workbook {
        let mut w = Workbook::new();
        w.locale = Locale::EN;
        let mut inputs = Vec::new();
        for (r, row) in rows.iter().enumerate() {
            for (c, t) in row.iter().enumerate() {
                inputs.push((CellRef::new(r as u32, c as u32), t.to_string()));
            }
        }
        w.set_inputs(0, inputs);
        w
    }

    fn rg(s: &str) -> Range {
        Range::parse(s).unwrap()
    }

    fn hidden(w: &Workbook) -> Vec<u32> {
        (0..10).filter(|r| w.sheets[0].row_height(*r) == 0.0).collect()
    }

    #[test]
    fn dates_shift_for_1904() {
        let mut wb = Workbook::new();
        wb.locale = Locale::EN;
        wb.set_input(0, CellRef::new(0, 0), "2024-03-01");
        wb.set_input(0, CellRef::new(1, 0), "12:00");
        wb.set_input(0, CellRef::new(2, 0), "5");
        let d = wb.sheets[0].value(CellRef::new(0, 0)).clone();
        wb.shift_dates(-DAYS_1904);
        let Value::Number(n) = d else { panic!() };
        assert_eq!(wb.sheets[0].value(CellRef::new(0, 0)), &Value::Number(n - DAYS_1904));
        assert_eq!(wb.sheets[0].value(CellRef::new(1, 0)), &Value::Number(0.5));
        assert_eq!(wb.sheets[0].value(CellRef::new(2, 0)), &Value::Number(5.0));
    }

    #[test]
    fn r1c1_input_and_display() {
        let mut wb = Workbook::new();
        wb.locale = Locale::EN;
        wb.calc.r1c1 = true;
        wb.set_input(0, CellRef::new(1, 1), "4");
        wb.set_input(0, CellRef::new(2, 1), "=R[-1]C*2+SUM(R2C2:R[-1]C)");
        assert_eq!(wb.sheets[0].value(CellRef::new(2, 1)), &Value::Number(12.0));
        assert_eq!(wb.edit_text(0, CellRef::new(2, 1)), "=R[-1]C*2+SUM(R2C2:R[-1]C)");
        wb.calc.r1c1 = false;
        assert_eq!(wb.edit_text(0, CellRef::new(2, 1)), "=B2*2+SUM($B$2:B2)");
    }

    #[test]
    fn book_statistics() {
        let mut wb = Workbook::new();
        wb.locale = Locale::EN;
        wb.set_input(0, CellRef::new(0, 0), "1");
        wb.set_input(0, CellRef::new(4, 2), "=A1*2");
        wb.execute(Op::AddSheet { index: 1, sheet: Box::new(Sheet::new("B")) });
        wb.set_input(1, CellRef::new(1, 1), "x");
        let s = wb.statistics(Some(0));
        assert_eq!((s.last, s.cells, s.formulas, s.sheets), (Some(CellRef::new(4, 2)), 2, 1, 1));
        let all = wb.statistics(None);
        assert_eq!((all.last, all.cells, all.formulas, all.sheets), (None, 3, 1, 2));
    }

    #[test]
    fn filter_values_and_undo() {
        let mut w = wb(&[&["Name", "N"], &["ana", "5"], &["Bob", "3"], &["Ana", "9"], &["", "1"], &["cy", "7"]]);
        let mut f = AutoFilter::new(rg("A1:B6"));
        let (vals, blanks) = w.filter_values(0, &f, 0);
        assert_eq!(vals, vec!["ana", "Bob", "cy"]);
        assert!(blanks);
        f.cols.push(ColumnFilter { col: 0, kind: FilterKind::Values { items: vec!["ANA".into()], blanks: false } });
        w.set_filter(0, Some(f.clone()), "undo.filter");
        assert_eq!(hidden(&w), vec![2, 4, 5]);
        // Values offered for column B only come from visible rows.
        assert_eq!(w.filter_values(0, &f, 1).0, vec!["5", "9"]);
        f.cols.push(ColumnFilter { col: 1, kind: FilterKind::Custom { and: true, a: (CritOp::Gt, "6".into()), b: None } });
        w.set_filter(0, Some(f.clone()), "undo.filter");
        assert_eq!(hidden(&w), vec![1, 2, 4, 5]);
        w.undo();
        assert_eq!(hidden(&w), vec![2, 4, 5]);
        w.set_filter(0, None, "undo.filter");
        assert!(hidden(&w).is_empty());
        assert!(w.sheets[0].filter.is_none());
    }

    #[test]
    fn filter_top_and_wildcards() {
        let mut w = wb(&[&["N", "T"], &["5", "apple"], &["3", "pear"], &["9", "apricot"], &["7", "fig"]]);
        let mut f = AutoFilter::new(rg("A1:B5"));
        f.cols.push(ColumnFilter { col: 0, kind: FilterKind::Top { top: true, percent: false, n: 2.0 } });
        w.set_filter(0, Some(f.clone()), "undo.filter");
        assert_eq!(hidden(&w), vec![1, 2]);
        f.cols[0] = ColumnFilter { col: 1, kind: FilterKind::Custom { and: true, a: (CritOp::Eq, "ap*".into()), b: None } };
        w.set_filter(0, Some(f.clone()), "undo.filter");
        assert_eq!(hidden(&w), vec![2, 4]);
        w.filter_sort(0, 0, false);
        assert_eq!(w.display(0, CellRef::new(1, 0)).0, "9");
        assert_eq!(hidden(&w), vec![2, 4]);
        w.undo();
        assert_eq!(w.display(0, CellRef::new(1, 0)).0, "5");
        assert_eq!(hidden(&w), vec![2, 4]);
    }

    #[test]
    fn validation() {
        let mut w = wb(&[&["Sí", "4"], &["No", "12"], &["x", "2.5"]]);
        let mut v = Validation::new(vec![rg("C1:C9")]);
        v.kind = ValKind::List;
        v.f1 = "$A$1:$A$2".into();
        let mut n = Validation::new(vec![rg("B1:B3")]);
        n.kind = ValKind::Whole;
        n.op = CmpOp::Between;
        n.f1 = "1".into();
        n.f2 = "10".into();
        w.execute(Op::Validations { sheet: 0, list: vec![v, n] });
        assert_eq!(w.validation_items(0, CellRef::new(4, 2)), vec!["Sí", "No"]);
        w.set_input(0, CellRef::new(0, 2), "no");
        assert!(w.is_valid(0, CellRef::new(0, 2)));
        w.set_input(0, CellRef::new(0, 2), "maybe");
        assert!(!w.is_valid(0, CellRef::new(0, 2)));
        assert_eq!(w.invalid_cells(0), vec![CellRef::new(0, 2), CellRef::new(1, 1), CellRef::new(2, 1)]);
        // Inserting a row moves the rule.
        w.execute(Op::InsertRows { sheet: 0, at: 0, count: 1 });
        assert_eq!(w.sheets[0].validations[1].ranges, vec![rg("B2:B4")]);
    }

    #[test]
    fn conditional_formats() {
        let mut w = wb(&[&["1", "a"], &["5", "b"], &["10", "a"], &["", "c"]]);
        let red = CfStyle { fill: Some(0xFF0000), ..Default::default() };
        let bold = CfStyle { bold: Some(true), fill: Some(0x00FF00), ..Default::default() };
        let rules = vec![
            CfRule { ranges: vec![rg("A1:A4")], kind: CfKind::CellIs { op: CmpOp::Greater, f1: "4".into(), f2: String::new() }, style: red.clone(), stop: false },
            CfRule { ranges: vec![rg("A1:A4")], kind: CfKind::Expression("A1=$A$3".into()), style: bold, stop: false },
            CfRule { ranges: vec![rg("B1:B4")], kind: CfKind::Duplicates(true), style: CfStyle { italic: Some(true), ..Default::default() }, stop: false },
            CfRule { ranges: vec![rg("A1:A4")], kind: CfKind::DataBar { color: 0x638EC6, min: Cfvo::Min, max: Cfvo::Max }, style: CfStyle::default(), stop: false },
            CfRule {
                ranges: vec![rg("A1:A4")],
                kind: CfKind::IconSet { set: "3Arrows".into(), reverse: false, show_value: true, points: vec![Cfvo::Percent(33.0), Cfvo::Percent(67.0)] },
                style: CfStyle::default(),
                stop: false,
            },
        ];
        w.execute(Op::CondFormats { sheet: 0, list: rules });
        let cache = w.cf_prepare(0);
        let at = |r, c| w.cf_eval(0, &cache, CellRef::new(r, c));
        let a1 = at(0, 0).unwrap();
        assert_eq!(a1.style.fill, None);
        assert_eq!(a1.bar, Some((0.0, 0x638EC6)));
        assert_eq!(a1.icon, Some(("3Arrows".into(), 0)));
        let a3 = at(2, 0).unwrap();
        assert_eq!(a3.style.fill, Some(0xFF0000));
        assert_eq!(a3.style.bold, Some(true));
        assert_eq!(a3.icon.unwrap().1, 2);
        assert_eq!(at(1, 0).unwrap().icon.unwrap().1, 1);
        assert_eq!(at(0, 1).unwrap().style.italic, Some(true));
        assert!(at(1, 1).is_none());
        assert!(at(3, 0).is_none());

        let mut s = w.sheets[0].cond_formats.clone();
        s.truncate(0);
        s.push(CfRule { ranges: vec![rg("A1:A3")], kind: CfKind::ColorScale(vec![(Cfvo::Min, 0x000000), (Cfvo::Max, 0xFFFFFF)]), style: CfStyle::default(), stop: false });
        s.push(CfRule { ranges: vec![rg("A1:A3")], kind: CfKind::Top { bottom: false, percent: false, rank: 1 }, style: red.clone(), stop: false });
        w.execute(Op::CondFormats { sheet: 0, list: s });
        let cache = w.cf_prepare(0);
        let fill = |r| w.cf_eval(0, &cache, CellRef::new(r, 0)).unwrap().style.fill;
        assert_eq!(fill(0), Some(0x000000));
        assert_eq!(fill(1), Some(0x717171));
        assert_eq!(fill(2), Some(0xFFFFFF));
    }

    #[test]
    fn duplicates_and_split() {
        let mut w = wb(&[&["K", "V"], &["a", "1"], &["A", "1"], &["b", "2"], &["a", "3"]]);
        assert_eq!(w.current_region(0, CellRef::new(2, 1)), rg("A1:B5"));
        assert_eq!(w.remove_duplicates(0, &rg("A1:B5"), &[0], true), (2, 2));
        assert_eq!(w.display(0, CellRef::new(2, 0)).0, "b");
        assert!(w.sheets[0].get(CellRef::new(3, 0)).is_none());
        w.undo();
        assert_eq!(w.remove_duplicates(0, &rg("A1:B5"), &[0, 1], true), (1, 3));
        assert_eq!(w.column_texts(0, CellRef::new(4, 0)), vec!["K", "a", "b"]);

        let d = SplitMode::Delimited { delims: vec![',', ' '], merge: true, quote: Some('"') };
        assert_eq!(split_text("x,  \"y,z\",3", &d), vec!["x", "y,z", "3"]);
        assert_eq!(split_text("abcdef", &SplitMode::Fixed(vec![2, 4])), vec!["ab", "cd", "ef"]);
        let mut w = wb(&[&["1;ana;x"], &["2;bo"]]);
        w.text_to_columns(0, &rg("A1:A2"), &SplitMode::Delimited { delims: vec![';'], merge: false, quote: None }, CellRef::new(0, 0), &[2]);
        assert_eq!(w.sheets[0].value(CellRef::new(0, 0)), &Value::Number(1.0));
        assert_eq!(w.display(0, CellRef::new(1, 1)).0, "bo");
        assert!(w.sheets[0].get(CellRef::new(0, 2)).is_none());
    }
}
