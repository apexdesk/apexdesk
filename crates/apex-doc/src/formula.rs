//! Table formulas: Word's `{ =SUM(ABOVE) \# "0.00" }` fields. Cells are
//! named like a sheet's (A1 is the first cell); ABOVE, LEFT, BELOW and
//! RIGHT take the numbers next to the field's cell.

use crate::{Document, FieldKind, OBJECT, Pos, TableMap};

/// The number in a cell's text ("1.234,50 €", "(12)", "7%"), if any.
pub fn cell_number(text: &str) -> Option<f64> {
    let t: String = text.chars().filter(|c| !c.is_whitespace() && !matches!(c, '€' | '$' | '£' | '¥' | '%' | '\u{a0}')).collect();
    let (neg, t) = match t.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
        Some(inner) => (true, inner.to_string()),
        None => (false, t),
    };
    if t.is_empty() || !t.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '-' | '+')) || !t.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let (dots, commas) = (t.matches('.').count(), t.matches(',').count());
    let decimal = match (dots, commas) {
        (0, 0) => None,
        (_, 0) if dots > 1 => None,
        (_, 0) => Some('.'),
        (0, 1) => {
            // "1,5" is a decimal comma; "1,500" groups thousands.
            let after = t.len() - t.find(',').unwrap_or(0) - 1;
            (after != 3).then_some(',')
        }
        (0, _) => None,
        _ => Some(if t.rfind(',') > t.rfind('.') { ',' } else { '.' }),
    };
    let clean: String = t.chars().filter_map(|c| match c {
        '.' | ',' if Some(c) == decimal => Some('.'),
        '.' | ',' => None,
        c => Some(c),
    }).collect();
    let v: f64 = clean.parse().ok()?;
    Some(if neg { -v } else { v })
}

/// A number as a field shows it: with a numeric picture ("#,##0.00",
/// "0%", "$#,##0") or, without one, as short as it goes.
pub fn format_number(v: f64, picture: Option<&str>, decimal: char) -> String {
    // No "-0".
    let v = if v == 0.0 { 0.0 } else { v };
    let group = if decimal == ',' { '.' } else { ',' };
    let Some(pic) = picture.filter(|p| p.chars().any(|c| c == '0' || c == '#')) else {
        let s = if v.fract() == 0.0 && v.abs() < 1e15 { format!("{v:.0}") } else { format!("{:.10}", v).trim_end_matches('0').trim_end_matches('.').to_string() };
        return s.replace('.', &decimal.to_string());
    };
    let first = pic.find(['0', '#']).unwrap_or(0);
    let last = pic.rfind(['0', '#']).unwrap_or(0);
    let (prefix, body, suffix) = (&pic[..first], &pic[first..=last], &pic[last + 1..]);
    let decimals = body.split_once('.').map_or(0, |(_, d)| d.chars().filter(|c| matches!(c, '0' | '#')).count());
    let grouped = body.contains(',');
    // Half away from zero, as Word rounds (format! rounds half to even).
    let scale = 10f64.powi(decimals as i32);
    let s = format!("{:.*}", decimals, (v.abs() * scale).round() / scale);
    let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
    let mut int_out = String::new();
    for (i, ch) in int.chars().enumerate() {
        if grouped && i > 0 && (int.len() - i) % 3 == 0 {
            int_out.push(group);
        }
        int_out.push(ch);
    }
    let mut out = String::new();
    if v < 0.0 && s.chars().any(|c| c.is_ascii_digit() && c != '0') {
        out.push('-');
    }
    out.push_str(prefix);
    out.push_str(&int_out);
    if !frac.is_empty() {
        out.push(decimal);
        out.push_str(frac);
    }
    out.push_str(suffix);
    out
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Name(String),
    Op(char),
}

fn tokens(s: &str) -> Option<Vec<Tok>> {
    let cs: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || (c == '.' && cs.get(i + 1).is_some_and(char::is_ascii_digit)) {
            let start = i;
            while i < cs.len() && (cs[i].is_ascii_digit() || cs[i] == '.') {
                i += 1;
            }
            out.push(Tok::Num(cs[start..i].iter().collect::<String>().parse().ok()?));
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < cs.len() && (cs[i].is_alphanumeric() || cs[i] == '_') {
                i += 1;
            }
            out.push(Tok::Name(cs[start..i].iter().collect::<String>().to_ascii_uppercase()));
        } else if "+-*/^%(),;:<>=".contains(c) {
            out.push(Tok::Op(c));
            i += 1;
        } else {
            return None;
        }
    }
    Some(out)
}

/// "B3" as (row, column), 0-based.
fn cell_ref(name: &str) -> Option<(usize, usize)> {
    let split = name.find(|c: char| c.is_ascii_digit())?;
    let (letters, digits) = name.split_at(split);
    if letters.is_empty() || !letters.chars().all(|c| c.is_ascii_uppercase()) || letters.len() > 2 {
        return None;
    }
    let col = letters.chars().fold(0usize, |a, c| a * 26 + (c as usize - 'A' as usize + 1)) - 1;
    let row: usize = digits.parse().ok()?;
    (row >= 1).then_some((row - 1, col))
}

struct Eval<'a> {
    doc: &'a Document,
    map: &'a TableMap,
    /// The field's cell.
    at: (usize, usize),
    toks: Vec<Tok>,
    i: usize,
    depth: usize,
    /// How many brackets and signs deep the expression is.
    nest: u32,
}

/// Formulas one field's value may work out, its own and those it reads.
const MAX_FORMULAS: usize = 10_000;

thread_local! {
    static FORMULAS_LEFT: std::cell::Cell<usize> = const { std::cell::Cell::new(MAX_FORMULAS) };
    /// While one field's value is worked out: the formulas it read, and
    /// the tables by their paragraphs (a column of SUM(ABOVE) reads each
    /// cell once, and maps its table once).
    static WORKED: std::cell::RefCell<std::collections::HashMap<Pos, Result<f64, ()>>> = Default::default();
    static MAPS: std::cell::RefCell<std::collections::HashMap<usize, std::rc::Rc<TableMap>>> = Default::default();
}

impl Eval<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.i)
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(&Tok::Op(c)) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    /// The number of cell (r, c): its text, or the result of a formula in it.
    fn cell(&self, r: usize, c: usize) -> Option<f64> {
        let (ar, ac) = self.map.anchor_of(r, c);
        let (a, b) = self.map.cell(ar, ac);
        let mut text = String::new();
        for i in a..b {
            let p = &self.doc.paragraphs[i];
            if crate::para_has_objects(p) && self.depth < 8 {
                for (off, at) in crate::objects_of(p) {
                    if let Some(FieldKind::Formula(e)) = at.field.as_deref().map(|f| &f.kind) {
                        if (ar, ac) != self.at {
                            return self.doc.formula_depth(Pos::new(i, off), e, self.depth + 1).ok();
                        }
                    }
                }
            }
            text.extend(p.text.chars().filter(|&c| c != OBJECT));
            text.push(' ');
        }
        cell_number(&text)
    }

    /// The numbers of the cells in a direction from the field's cell, up
    /// to the first cell with text after some numbers.
    fn direction(&self, d: &str) -> Vec<f64> {
        let (r, c) = self.at;
        let (rows, cols) = (self.map.rows, self.map.cols);
        let cells: Vec<(usize, usize)> = match d {
            "ABOVE" => (0..r).rev().map(|k| (k, c)).collect(),
            "BELOW" => (r + 1..rows).map(|k| (k, c)).collect(),
            "LEFT" => (0..c).rev().map(|k| (r, k)).collect(),
            _ => (c + 1..cols).map(|k| (r, k)).collect(),
        };
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (rr, cc) in cells {
            let anchor = self.map.anchor_of(rr, cc);
            if anchor == self.map.anchor_of(r, c) || seen.contains(&anchor) {
                continue;
            }
            seen.insert(anchor);
            match self.cell(rr, cc) {
                Some(v) => out.push(v),
                None => {
                    let (a, b) = self.map.cell(anchor.0, anchor.1);
                    let blank = (a..b).all(|i| self.doc.paragraphs[i].text.trim().is_empty());
                    if !blank && !out.is_empty() {
                        break;
                    }
                }
            }
        }
        out
    }

    /// A function argument: a direction, a range or an expression.
    fn arg(&mut self, out: &mut Vec<f64>) -> Option<()> {
        if let Some(Tok::Name(n)) = self.peek().cloned() {
            if matches!(n.as_str(), "ABOVE" | "BELOW" | "LEFT" | "RIGHT") {
                self.i += 1;
                out.extend(self.direction(&n));
                return Some(());
            }
            if let (Some(a), Some(Tok::Op(':')), Some(Tok::Name(m))) = (cell_ref(&n), self.toks.get(self.i + 1).cloned(), self.toks.get(self.i + 2).cloned()) {
                let b = cell_ref(&m)?;
                self.i += 3;
                for r in a.0.min(b.0)..=a.0.max(b.0).min(self.map.rows.saturating_sub(1)) {
                    for c in a.1.min(b.1)..=a.1.max(b.1).min(self.map.cols.saturating_sub(1)) {
                        out.extend(self.cell(r, c));
                    }
                }
                return Some(());
            }
        }
        out.push(self.expr()?);
        Some(())
    }

    fn call(&mut self, name: &str) -> Option<f64> {
        if !self.eat('(') {
            return None;
        }
        let mut vals = Vec::new();
        if !self.eat(')') {
            loop {
                self.arg(&mut vals)?;
                if self.eat(')') {
                    break;
                }
                if !(self.eat(',') || self.eat(';')) {
                    return None;
                }
            }
        }
        let first = |k: usize| vals.get(k).copied();
        Some(match name {
            "SUM" => vals.iter().sum(),
            "AVERAGE" => {
                if vals.is_empty() {
                    return None;
                }
                vals.iter().sum::<f64>() / vals.len() as f64
            }
            "MIN" => vals.iter().copied().fold(f64::INFINITY, f64::min).min(if vals.is_empty() { 0.0 } else { f64::INFINITY }),
            "MAX" => vals.iter().copied().fold(f64::NEG_INFINITY, f64::max).max(if vals.is_empty() { 0.0 } else { f64::NEG_INFINITY }),
            "COUNT" => vals.len() as f64,
            "PRODUCT" => vals.iter().product(),
            "ABS" => first(0)?.abs(),
            "INT" => first(0)?.trunc(),
            "SIGN" => first(0)?.signum(),
            "ROUND" => {
                let p = 10f64.powi(first(1).unwrap_or(0.0) as i32);
                (first(0)? * p).round() / p
            }
            "MOD" => {
                let b = first(1)?;
                if b == 0.0 {
                    return None;
                }
                first(0)? % b
            }
            "IF" => {
                if first(0)? != 0.0 {
                    first(1)?
                } else {
                    first(2).unwrap_or(0.0)
                }
            }
            "AND" => f64::from(vals.iter().all(|v| *v != 0.0)),
            "OR" => f64::from(vals.iter().any(|v| *v != 0.0)),
            "NOT" => f64::from(first(0)? == 0.0),
            _ => return None,
        })
    }

    /// An operand; nesting past 200 brackets or signs gives up, as it
    /// would overflow the stack.
    fn primary(&mut self) -> Option<f64> {
        if self.nest >= 200 {
            return None;
        }
        self.nest += 1;
        let v = self.operand();
        self.nest -= 1;
        v
    }

    fn operand(&mut self) -> Option<f64> {
        match self.peek().cloned()? {
            Tok::Num(v) => {
                self.i += 1;
                Some(v)
            }
            Tok::Op('(') => {
                self.i += 1;
                let v = self.expr()?;
                self.eat(')').then_some(v)
            }
            Tok::Op('-') => {
                self.i += 1;
                Some(-self.unary()?)
            }
            Tok::Op('+') => {
                self.i += 1;
                self.unary()
            }
            Tok::Name(n) => {
                self.i += 1;
                if matches!(self.peek(), Some(Tok::Op('('))) {
                    return self.call(&n);
                }
                match n.as_str() {
                    "TRUE" => Some(1.0),
                    "FALSE" => Some(0.0),
                    _ => {
                        let (r, c) = cell_ref(&n)?;
                        if r >= self.map.rows || c >= self.map.cols {
                            return None;
                        }
                        Some(self.cell(r, c).unwrap_or(0.0))
                    }
                }
            }
            Tok::Op(_) => None,
        }
    }

    fn unary(&mut self) -> Option<f64> {
        let v = self.primary()?;
        Some(if self.eat('%') { v / 100.0 } else { v })
    }

    fn power(&mut self) -> Option<f64> {
        let v = self.unary()?;
        if self.eat('^') {
            // Each `^` goes one deeper, as a bracket does.
            if self.nest >= 200 {
                return None;
            }
            self.nest += 1;
            let e = self.power();
            self.nest -= 1;
            return Some(v.powf(e?));
        }
        Some(v)
    }

    fn term(&mut self) -> Option<f64> {
        let mut v = self.power()?;
        loop {
            if self.eat('*') {
                v *= self.power()?;
            } else if self.eat('/') {
                let d = self.power()?;
                if d == 0.0 {
                    return None;
                }
                v /= d;
            } else {
                return Some(v);
            }
        }
    }

    fn sum(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        loop {
            if self.eat('+') {
                v += self.term()?;
            } else if self.eat('-') {
                v -= self.term()?;
            } else {
                return Some(v);
            }
        }
    }

    fn expr(&mut self) -> Option<f64> {
        let a = self.sum()?;
        let op: String = match (self.peek().cloned(), self.toks.get(self.i + 1).cloned()) {
            (Some(Tok::Op(x @ ('<' | '>'))), Some(Tok::Op(y @ ('=' | '>')))) if !(x == '>' && y == '>') => {
                self.i += 2;
                format!("{x}{y}")
            }
            (Some(Tok::Op(x @ ('<' | '>' | '='))), _) => {
                self.i += 1;
                x.to_string()
            }
            _ => return Some(a),
        };
        let b = self.sum()?;
        Some(f64::from(match op.as_str() {
            "<" => a < b,
            ">" => a > b,
            "=" => a == b,
            "<=" => a <= b,
            ">=" => a >= b,
            _ => a != b,
        }))
    }
}

impl Document {
    /// The value of the formula `expr` ("SUM(ABOVE)", "B2*2") of a field
    /// at `at`; an error when it can't be worked out.
    pub fn formula_value(&self, at: Pos, expr: &str) -> Result<f64, ()> {
        FORMULAS_LEFT.set(MAX_FORMULAS);
        let clear = || {
            WORKED.with_borrow_mut(|m| m.clear());
            MAPS.with_borrow_mut(|m| m.clear());
        };
        clear();
        let v = self.formula_depth(at, expr, 0);
        clear();
        v
    }

    /// The map of the table paragraph `para` is in (innermost), kept
    /// while one field's value is worked out.
    fn formula_map(&self, para: usize) -> Option<std::rc::Rc<TableMap>> {
        if let Some(m) = MAPS.with_borrow(|m| m.get(&para).cloned()) {
            return Some(m);
        }
        let map = std::rc::Rc::new(self.table_map(para)?);
        MAPS.with_borrow_mut(|m| {
            for i in map.start..map.end {
                if self.table_depth(i) == Some(map.depth) {
                    m.insert(i, map.clone());
                }
            }
            m.insert(para, map.clone());
        });
        Some(map)
    }

    fn formula_depth(&self, at: Pos, expr: &str, depth: usize) -> Result<f64, ()> {
        // Formulas in the cells a formula reads are worked out again for
        // each: a column of SUM(ABOVE) would take ever longer.
        let left = FORMULAS_LEFT.get();
        if left == 0 {
            return Err(());
        }
        FORMULAS_LEFT.set(left - 1);
        if let Some(v) = WORKED.with_borrow(|m| m.get(&at).copied()) {
            return v;
        }
        let v = self.formula_work(at, expr, depth);
        WORKED.with_borrow_mut(|m| m.insert(at, v));
        v
    }

    fn formula_work(&self, at: Pos, expr: &str, depth: usize) -> Result<f64, ()> {
        let toks = tokens(expr).ok_or(())?;
        let empty = TableMap { start: 0, end: 0, rows: 0, cols: 0, cells: Vec::new(), anchor: Vec::new(), span: Vec::new(), depth: 0 };
        let map = self.formula_map(at.para);
        let map = map.as_deref().unwrap_or(&empty);
        let cell = if map.rows > 0 { map.pos_of(&self.paragraphs, at.para) } else { (0, 0) };
        let mut e = Eval { doc: self, map, at: cell, toks, i: 0, depth, nest: 0 };
        let v = e.expr().ok_or(())?;
        if e.i != e.toks.len() || !v.is_finite() {
            return Err(());
        }
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Field, Paragraph, Selection};

    #[test]
    fn deep_formulas_give_up() {
        let d = Document::from_paragraphs(vec![Paragraph::new("")]);
        let at = Pos::new(0, 0);
        assert!(d.formula_value(at, &format!("{}1{}", "(".repeat(100_000), ")".repeat(100_000))).is_err());
        assert!(d.formula_value(at, &format!("{}1", "-".repeat(100_000))).is_err());
        assert!(d.formula_value(at, &format!("1{}", "^1".repeat(100_000))).is_err());
        assert_eq!(d.formula_value(at, "2^3^2"), Ok(512.0));
        assert_eq!(d.formula_value(at, "-(-(2+3))"), Ok(5.0));
    }

    #[test]
    fn negative_zero() {
        assert_eq!(format_number(-0.0, None, '.'), "0");
        assert_eq!(format_number(-0.0, Some("0.00"), '.'), "0.00");
    }

    #[test]
    fn numbers() {
        assert_eq!(cell_number("1.234,50 €"), Some(1234.5));
        assert_eq!(cell_number("1,234.50"), Some(1234.5));
        assert_eq!(cell_number("1,5"), Some(1.5));
        assert_eq!(cell_number("1,500"), Some(1500.0));
        assert_eq!(cell_number("(12)"), Some(-12.0));
        assert_eq!(cell_number("Total"), None);
        assert_eq!(format_number(1234.5, Some("#,##0.00"), ','), "1.234,50");
        assert_eq!(format_number(1234.5, Some("$#,##0"), '.'), "$1,235");
        assert_eq!(format_number(0.25, Some("0%"), '.'), "0%");
        assert_eq!(format_number(2.5, None, ','), "2,5");
        assert_eq!(format_number(-3.0, None, '.'), "-3");
    }

    #[test]
    fn table_formulas() {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("")]);
        let s = d.insert_table(Selection::caret(Pos::new(0, 0)), 4, 3, 300.0);
        let m = d.table_map(s.cursor.para).unwrap();
        let text = [["Mes", "Norte", "Sur"], ["Ene", "10", "20,5"], ["Feb", "30", "40"]];
        // Fill from the end so positions stay valid.
        for r in (0..3).rev() {
            for c in (0..3).rev() {
                let (a, _) = m.cell(r, c);
                d.insert_text(Selection::caret(Pos::new(a, 0)), text[r][c], None);
            }
        }
        let m = d.table_map(s.cursor.para).unwrap();
        let at = |r: usize, c: usize| Pos::new(m.cell(r, c).0, 0);
        assert_eq!(d.formula_value(at(3, 1), "SUM(ABOVE)"), Ok(40.0));
        assert_eq!(d.formula_value(at(3, 2), "AVERAGE(ABOVE)"), Ok(30.25));
        assert_eq!(d.formula_value(at(1, 0), "SUM(RIGHT)"), Ok(30.5));
        assert_eq!(d.formula_value(at(3, 0), "B2+C3*2"), Ok(90.0));
        assert_eq!(d.formula_value(at(3, 0), "MAX(B2:C3)"), Ok(40.0));
        assert_eq!(d.formula_value(at(3, 0), "ROUND(C2;0)"), Ok(21.0));
        assert_eq!(d.formula_value(at(3, 0), "IF(B3>B2;1;2)"), Ok(1.0));
        assert!(d.formula_value(at(3, 0), "SUM(").is_err());
        // A formula field in the last row, and one that adds it up.
        let f = Field::parse("=SUM(ABOVE) \\# \"#,##0.00\"", "").unwrap();
        assert_eq!(f.kind, FieldKind::Formula("SUM(ABOVE)".into()));
        assert_eq!(f.format.as_deref(), Some("#,##0.00"));
        assert_eq!(f.instr(), " =SUM(ABOVE) \\# \"#,##0.00\" ");
        d.insert_field(Selection::caret(at(3, 1)), f);
        assert_eq!(d.formula_value(at(3, 0), "SUM(RIGHT)"), Ok(40.0));
        let doc = d.with_seq_results().unwrap();
        let (_, fld) = doc.field_at(at(3, 1)).unwrap();
        assert_eq!(fld.cached, "40.00");
    }
}
