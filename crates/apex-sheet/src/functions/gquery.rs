//! QUERY of Google Sheets: the Google Visualization API query language
//! over a range or an array. Supported: select (columns, arithmetic,
//! sum/avg/count/min/max and a few scalar functions), where (comparisons,
//! contains, starts with, ends with, matches, like, is null, and/or/not),
//! group by, order by, limit, offset and label. Format and options are
//! ignored; pivot is #VALUE!.

use super::*;
use crate::format::{date_to_serial, serial_to_date};
use std::cmp::Ordering;

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Word(String),
    /// A column name between backquotes.
    Quoted(String),
    Str(String),
    Num(f64),
    Sym(&'static str),
}

fn lex(s: &str) -> R<Vec<Tok>> {
    let c: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if ch == '\'' || ch == '"' || ch == '`' {
            let end = c[i + 1..].iter().position(|&x| x == ch).ok_or(ErrorKind::Value)? + i + 1;
            let t: String = c[i + 1..end].iter().collect();
            out.push(if ch == '`' { Tok::Quoted(t) } else { Tok::Str(t) });
            i = end + 1;
        } else if ch.is_ascii_digit() || (ch == '.' && c.get(i + 1).is_some_and(char::is_ascii_digit)) {
            let start = i;
            while i < c.len() && (c[i].is_ascii_digit() || c[i] == '.' || ((c[i] == 'e' || c[i] == 'E') && c.get(i + 1).is_some_and(|d| d.is_ascii_digit() || *d == '-'))) {
                i += if c[i] == 'e' || c[i] == 'E' { 2 } else { 1 };
            }
            let t: String = c[start..i].iter().collect();
            out.push(Tok::Num(t.parse().map_err(|_| ErrorKind::Value)?));
        } else if ch.is_alphanumeric() || ch == '_' {
            let start = i;
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') {
                i += 1;
            }
            out.push(Tok::Word(c[start..i].iter().collect()));
        } else {
            let two: String = c[i..(i + 2).min(c.len())].iter().collect();
            let sym = ["!=", "<>", "<=", ">="].into_iter().find(|s| *s == two).or_else(|| ["(", ")", ",", "*", "+", "-", "/", "%", "=", "<", ">"].into_iter().find(|s| s.starts_with(ch))).ok_or(ErrorKind::Value)?;
            i += sym.chars().count();
            out.push(Tok::Sym(sym));
        }
    }
    Ok(out)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Contains,
    Starts,
    Ends,
    Matches,
    Like,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Agg {
    Sum,
    Avg,
    Count,
    Min,
    Max,
}

impl Agg {
    fn name(self) -> &'static str {
        match self {
            Agg::Sum => "sum",
            Agg::Avg => "avg",
            Agg::Count => "count",
            Agg::Min => "min",
            Agg::Max => "max",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Q {
    Col(usize),
    Lit(Value),
    Neg(Box<Q>),
    Not(Box<Q>),
    Bin(Op, Box<Q>, Box<Q>),
    IsNull(Box<Q>, bool),
    Agg(Agg, Box<Q>),
    Func(String, Vec<Q>),
}

impl Q {
    fn has_agg(&self) -> bool {
        match self {
            Q::Agg(..) => true,
            Q::Neg(e) | Q::Not(e) | Q::IsNull(e, _) => e.has_agg(),
            Q::Bin(_, l, r) => l.has_agg() || r.has_agg(),
            Q::Func(_, a) => a.iter().any(Q::has_agg),
            Q::Col(_) | Q::Lit(_) => false,
        }
    }

    fn has_col(&self) -> bool {
        match self {
            Q::Col(_) => true,
            Q::Agg(..) | Q::Lit(_) => false,
            Q::Neg(e) | Q::Not(e) | Q::IsNull(e, _) => e.has_col(),
            Q::Bin(_, l, r) => l.has_col() || r.has_col(),
            Q::Func(_, a) => a.iter().any(Q::has_col),
        }
    }
}

struct Parser<'c> {
    t: Vec<Tok>,
    i: usize,
    /// Column names: letters of the sheet (or Col1, Col2... for arrays).
    ids: &'c [String],
    /// How many brackets and signs deep the parser is.
    depth: u32,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i)
    }

    fn at_kw(&self, k: &str) -> bool {
        matches!(self.peek(), Some(Tok::Word(w)) if w.eq_ignore_ascii_case(k))
    }

    fn kw(&mut self, k: &str) -> bool {
        let yes = self.at_kw(k);
        self.i += yes as usize;
        yes
    }

    fn expect_kw(&mut self, k: &str) -> R<()> {
        if self.kw(k) { Ok(()) } else { Err(ErrorKind::Value) }
    }

    fn sym(&mut self, s: &str) -> bool {
        let yes = matches!(self.peek(), Some(Tok::Sym(x)) if *x == s);
        self.i += yes as usize;
        yes
    }

    fn col(&self, name: &str) -> Option<usize> {
        self.ids.iter().position(|x| x == name).or_else(|| {
            let n = name.strip_prefix("Col").or_else(|| name.strip_prefix("col"))?.parse::<usize>().ok()?;
            (n >= 1 && n <= self.ids.len()).then(|| n - 1)
        })
    }

    fn expr(&mut self) -> R<Q> {
        self.nested(Self::either)
    }

    /// `f` one level deeper; past 200 the query is refused, as it would
    /// overflow the stack.
    fn nested(&mut self, f: fn(&mut Self) -> R<Q>) -> R<Q> {
        if self.depth >= 200 {
            return Err(ErrorKind::Value);
        }
        self.depth += 1;
        let q = f(self);
        self.depth -= 1;
        q
    }

    fn either(&mut self) -> R<Q> {
        let mut l = self.and()?;
        while self.kw("or") {
            l = Q::Bin(Op::Or, Box::new(l), Box::new(self.and()?));
        }
        Ok(l)
    }

    fn and(&mut self) -> R<Q> {
        let mut l = self.not()?;
        while self.kw("and") {
            l = Q::Bin(Op::And, Box::new(l), Box::new(self.not()?));
        }
        Ok(l)
    }

    fn not(&mut self) -> R<Q> {
        if self.kw("not") { Ok(Q::Not(Box::new(self.nested(Self::not)?))) } else { self.cmp() }
    }

    fn cmp(&mut self) -> R<Q> {
        let l = self.add()?;
        let op = if self.sym("=") {
            Op::Eq
        } else if self.sym("!=") || self.sym("<>") {
            Op::Ne
        } else if self.sym("<=") {
            Op::Le
        } else if self.sym(">=") {
            Op::Ge
        } else if self.sym("<") {
            Op::Lt
        } else if self.sym(">") {
            Op::Gt
        } else if self.kw("contains") {
            Op::Contains
        } else if self.kw("starts") {
            self.expect_kw("with")?;
            Op::Starts
        } else if self.kw("ends") {
            self.expect_kw("with")?;
            Op::Ends
        } else if self.kw("matches") {
            Op::Matches
        } else if self.kw("like") {
            Op::Like
        } else if self.kw("is") {
            let not = self.kw("not");
            self.expect_kw("null")?;
            return Ok(Q::IsNull(Box::new(l), !not));
        } else {
            return Ok(l);
        };
        Ok(Q::Bin(op, Box::new(l), Box::new(self.add()?)))
    }

    fn add(&mut self) -> R<Q> {
        let mut l = self.mul()?;
        loop {
            let op = if self.sym("+") {
                Op::Add
            } else if self.sym("-") {
                Op::Sub
            } else {
                return Ok(l);
            };
            l = Q::Bin(op, Box::new(l), Box::new(self.mul()?));
        }
    }

    fn mul(&mut self) -> R<Q> {
        let mut l = self.unary()?;
        loop {
            let op = if self.sym("*") {
                Op::Mul
            } else if self.sym("/") {
                Op::Div
            } else if self.sym("%") {
                Op::Mod
            } else {
                return Ok(l);
            };
            l = Q::Bin(op, Box::new(l), Box::new(self.unary()?));
        }
    }

    fn unary(&mut self) -> R<Q> {
        if self.sym("-") { Ok(Q::Neg(Box::new(self.nested(Self::unary)?))) } else { self.primary() }
    }

    fn primary(&mut self) -> R<Q> {
        let tok = self.peek().cloned().ok_or(ErrorKind::Value)?;
        self.i += 1;
        match tok {
            Tok::Num(n) => Ok(Q::Lit(Value::Number(n))),
            Tok::Str(s) => Ok(Q::Lit(Value::text(s))),
            Tok::Quoted(s) => self.col(&s).map(Q::Col).ok_or(ErrorKind::Value),
            Tok::Sym("(") => {
                let e = self.expr()?;
                if self.sym(")") { Ok(e) } else { Err(ErrorKind::Value) }
            }
            Tok::Sym(_) => Err(ErrorKind::Value),
            Tok::Word(w) => {
                let lw = w.to_ascii_lowercase();
                if self.sym("(") {
                    let mut args = Vec::new();
                    if lw == "count" && self.sym("*") {
                        args.push(Q::Lit(Value::Number(1.0)));
                    } else if !self.sym(")") {
                        loop {
                            args.push(self.expr()?);
                            if self.sym(",") {
                                continue;
                            }
                            break;
                        }
                    }
                    if !self.sym(")") {
                        return Err(ErrorKind::Value);
                    }
                    let agg = match lw.as_str() {
                        "sum" => Some(Agg::Sum),
                        "avg" => Some(Agg::Avg),
                        "count" => Some(Agg::Count),
                        "min" => Some(Agg::Min),
                        "max" => Some(Agg::Max),
                        _ => None,
                    };
                    return match (agg, args.len()) {
                        (Some(k), 1) => Ok(Q::Agg(k, Box::new(args.pop().unwrap()))),
                        (Some(_), _) => Err(ErrorKind::Value),
                        (None, _) if ["upper", "lower", "year", "month", "day", "abs", "round", "now", "todate"].contains(&lw.as_str()) => Ok(Q::Func(lw, args)),
                        _ => Err(ErrorKind::Value),
                    };
                }
                match lw.as_str() {
                    "true" => return Ok(Q::Lit(Value::Bool(true))),
                    "false" => return Ok(Q::Lit(Value::Bool(false))),
                    "null" => return Ok(Q::Lit(Value::Empty)),
                    "date" | "datetime" | "timestamp" if matches!(self.peek(), Some(Tok::Str(_))) => {
                        let Some(Tok::Str(s)) = self.peek().cloned() else { unreachable!() };
                        self.i += 1;
                        return date_literal(&s).map(|d| Q::Lit(Value::Number(d))).ok_or(ErrorKind::Value);
                    }
                    _ => {}
                }
                self.col(&w).map(Q::Col).ok_or(ErrorKind::Value)
            }
        }
    }

    /// Items separated by commas.
    fn list<T>(&mut self, mut item: impl FnMut(&mut Self) -> R<T>) -> R<Vec<T>> {
        let mut out = vec![item(self)?];
        while self.sym(",") {
            out.push(item(self)?);
        }
        Ok(out)
    }
}

/// `2024-01-31` or `2024-01-31 10:30:00` as a serial number.
fn date_literal(s: &str) -> Option<f64> {
    let (date, time) = s.trim().split_once(' ').unwrap_or((s.trim(), ""));
    let mut it = date.split('-').map(|x| x.parse::<i64>());
    let (y, m, d) = (it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?);
    let mut t = 0.0;
    for (i, p) in time.split(':').filter(|p| !p.is_empty()).enumerate() {
        t += p.parse::<f64>().ok()? / [24.0, 1440.0, 86400.0].get(i)?;
    }
    Some(date_to_serial(y, m, d) + t)
}

struct Query {
    select: Option<Vec<(Q, String)>>,
    filter: Option<Q>,
    group: Vec<Q>,
    order: Vec<(Q, bool)>,
    limit: Option<usize>,
    offset: usize,
    labels: Vec<(Q, String)>,
}

fn parse(text: &str, ids: &[String]) -> R<Query> {
    let t = lex(text)?;
    // Longer queries make expression trees too deep to walk.
    if t.len() > 4096 {
        return Err(ErrorKind::Value);
    }
    let mut p = Parser { t, i: 0, ids, depth: 0 };
    let mut q = Query { select: None, filter: None, group: Vec::new(), order: Vec::new(), limit: None, offset: 0, labels: Vec::new() };
    if p.kw("select") && !p.sym("*") {
        q.select = Some(p.list(|p| {
            let start = p.i;
            let e = p.expr()?;
            let src = p.t[start..p.i].iter().map(tok_text).collect::<Vec<_>>().join(" ");
            Ok((e, src))
        })?);
    }
    if p.kw("where") {
        q.filter = Some(p.expr()?);
    }
    if p.kw("group") {
        p.expect_kw("by")?;
        q.group = p.list(Parser::expr)?;
    }
    if p.at_kw("pivot") {
        return Err(ErrorKind::Value);
    }
    if p.kw("order") {
        p.expect_kw("by")?;
        q.order = p.list(|p| {
            let e = p.expr()?;
            let desc = p.kw("desc");
            if !desc {
                p.kw("asc");
            }
            Ok((e, !desc))
        })?;
    }
    let count = |p: &mut Parser| match p.peek().cloned() {
        Some(Tok::Num(n)) if n >= 0.0 => {
            p.i += 1;
            Ok(n as usize)
        }
        _ => Err(ErrorKind::Value),
    };
    if p.kw("limit") {
        q.limit = Some(count(&mut p)?);
    }
    if p.kw("offset") {
        q.offset = count(&mut p)?;
    }
    if p.kw("label") {
        q.labels = p.list(|p| {
            let e = p.expr()?;
            match p.peek().cloned() {
                Some(Tok::Str(s)) => {
                    p.i += 1;
                    Ok((e, s))
                }
                _ => Err(ErrorKind::Value),
            }
        })?;
    }
    // Formats and options change nothing here.
    if p.kw("format") {
        while p.peek().is_some() && !p.at_kw("options") {
            p.i += 1;
        }
    }
    if p.kw("options") {
        p.i = p.t.len();
    }
    if p.peek().is_some() { Err(ErrorKind::Value) } else { Ok(q) }
}

fn tok_text(t: &Tok) -> String {
    match t {
        Tok::Word(w) => w.clone(),
        Tok::Quoted(w) => format!("`{w}`"),
        Tok::Str(s) => format!("'{s}'"),
        Tok::Num(n) => Value::Number(*n).to_text().map(|t| t.to_string()).unwrap_or_default(),
        Tok::Sym(s) => s.to_string(),
    }
}

/// Values of the same kind in order (text case-sensitive, as Google
/// compares them); None between different kinds or with a null.
fn order(a: &Value, b: &Value) -> Option<Ordering> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.partial_cmp(y),
        (Value::Text(x), Value::Text(y)) => Some(x.cmp(y)),
        (Value::Bool(x), Value::Bool(y)) => Some(x.cmp(y)),
        _ => None,
    }
}

fn like(text: &str, pat: &str) -> bool {
    let mut re = String::from("^");
    for c in pat.chars() {
        match c {
            '%' => re.push_str(".*"),
            '_' => re.push('.'),
            c => re.push_str(&regex::escape(&c.to_string())),
        }
    }
    re.push('$');
    matches_re(&re, text)
}

/// Whether `text` matches the regular expression `re`, compiled once for
/// all the rows a condition is checked on.
fn matches_re(re: &str, text: &str) -> bool {
    thread_local! {
        static LAST: std::cell::RefCell<Option<(String, Option<regex::Regex>)>> = const { std::cell::RefCell::new(None) };
    }
    LAST.with_borrow_mut(|last| {
        if last.as_ref().is_none_or(|(p, _)| p != re) {
            *last = Some((re.to_string(), regex::Regex::new(re).ok()));
        }
        last.as_ref().and_then(|(_, r)| r.as_ref()).is_some_and(|r| r.is_match(text))
    })
}

fn eval(q: &Q, m: &Matrix, rows: &[usize]) -> Value {
    match q {
        Q::Col(c) => rows.first().map_or(Value::Empty, |&r| m.get(r, *c).clone()),
        Q::Lit(v) => v.clone(),
        Q::Neg(e) => match eval(e, m, rows) {
            Value::Number(x) => Value::Number(-x),
            _ => Value::Empty,
        },
        Q::Not(e) => match eval(e, m, rows) {
            Value::Bool(b) => Value::Bool(!b),
            _ => Value::Empty,
        },
        Q::IsNull(e, null) => Value::Bool(matches!(eval(e, m, rows), Value::Empty) == *null),
        Q::Agg(k, e) => {
            let vals: Vec<Value> = rows.iter().map(|&r| eval(e, m, &[r])).filter(|v| !matches!(v, Value::Empty)).collect();
            let nums = || vals.iter().filter_map(Value::as_number);
            match k {
                Agg::Count => Value::Number(vals.len() as f64),
                Agg::Sum => Value::Number(nums().sum()),
                Agg::Avg => {
                    let n = nums().count();
                    if n == 0 { Value::Empty } else { Value::Number(nums().sum::<f64>() / n as f64) }
                }
                Agg::Min | Agg::Max => {
                    let pick = if *k == Agg::Min { Ordering::Less } else { Ordering::Greater };
                    vals.iter().fold(None::<&Value>, |best, v| match best {
                        Some(b) if order(v, b) != Some(pick) => Some(b),
                        _ => Some(v),
                    }).cloned().unwrap_or(Value::Empty)
                }
            }
        }
        Q::Func(name, args) => {
            let a: Vec<Value> = args.iter().map(|e| eval(e, m, rows)).collect();
            let num = |i: usize| a.get(i).and_then(Value::as_number);
            let date = |f: fn((i64, u32, u32)) -> f64| num(0).map_or(Value::Empty, |x| Value::Number(f(serial_to_date(x.floor()))));
            match name.as_str() {
                "upper" | "lower" => match a.first() {
                    Some(Value::Text(t)) => Value::text(if name == "upper" { t.to_uppercase() } else { t.to_lowercase() }),
                    _ => Value::Empty,
                },
                "year" => date(|d| d.0 as f64),
                // Months count from 0 in this language.
                "month" => date(|d| d.1 as f64 - 1.0),
                "day" => date(|d| d.2 as f64),
                "abs" => num(0).map_or(Value::Empty, |x| Value::Number(x.abs())),
                "round" => num(0).map_or(Value::Empty, |x| Value::Number(x.round())),
                "todate" => num(0).map_or(Value::Empty, |x| Value::Number(x.floor())),
                _ => Value::Empty,
            }
        }
        Q::Bin(op, l, r) => {
            let (a, b) = (eval(l, m, rows), eval(r, m, rows));
            match op {
                Op::And | Op::Or => {
                    let (x, y) = (matches!(a, Value::Bool(true)), matches!(b, Value::Bool(true)));
                    Value::Bool(if *op == Op::And { x && y } else { x || y })
                }
                Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Mod => match (a.as_number(), b.as_number()) {
                    (Some(x), Some(y)) => {
                        let v = match op {
                            Op::Add => x + y,
                            Op::Sub => x - y,
                            Op::Mul => x * y,
                            Op::Div => x / y,
                            _ => x % y,
                        };
                        if v.is_finite() { Value::Number(v) } else { Value::Empty }
                    }
                    _ => Value::Empty,
                },
                Op::Eq | Op::Ne | Op::Lt | Op::Le | Op::Gt | Op::Ge => Value::Bool(order(&a, &b).is_some_and(|o| match op {
                    Op::Eq => o == Ordering::Equal,
                    Op::Ne => o != Ordering::Equal,
                    Op::Lt => o == Ordering::Less,
                    Op::Le => o != Ordering::Greater,
                    Op::Gt => o == Ordering::Greater,
                    _ => o != Ordering::Less,
                })),
                Op::Contains | Op::Starts | Op::Ends | Op::Matches | Op::Like => Value::Bool(match (&a, &b) {
                    (Value::Text(t), Value::Text(p)) => match op {
                        Op::Contains => t.contains(&**p),
                        Op::Starts => t.starts_with(&**p),
                        Op::Ends => t.ends_with(&**p),
                        Op::Matches => matches_re(&format!("^(?:{p})$"), t),
                        _ => like(t, p),
                    },
                    _ => false,
                }),
            }
        }
    }
}

/// QUERY(data, query, [headers]).
pub(super) fn query(env: &Env, a: &[Expr]) -> R<Value> {
    let t = env.tab(&a[0])?;
    let first_col = match &t {
        Tab::R(_, _, r) => Some(r.start.col),
        Tab::A(_) => None,
    };
    let m = t.to_matrix()?;
    let text = match a.get(1) {
        None | Some(Expr::Missing) => String::new(),
        Some(e) => env.text(e)?.to_string(),
    };
    let ids: Vec<String> = (0..m.cols).map(|c| first_col.map_or_else(|| format!("Col{}", c + 1), |f| crate::addr::col_name(f + c as u32))).collect();
    let q = parse(&text, &ids)?;
    // Header rows: given, or guessed (a first row of texts over other kinds).
    let headers = match env.opt_num(a, 2, -1.0)?.trunc() {
        h if h < 0.0 => {
            let texts = (0..m.cols).all(|c| matches!(m.get(0, c), Value::Text(_) | Value::Empty)) && (0..m.cols).any(|c| matches!(m.get(0, c), Value::Text(_)));
            let other = (1..m.rows).any(|r| (0..m.cols).any(|c| matches!(m.get(r, c), Value::Number(_) | Value::Bool(_))));
            (m.rows > 1 && texts && other) as usize
        }
        h => (h as usize).min(m.rows),
    };
    let header = |c: usize| (0..headers).filter_map(|r| m.get(r, c).to_text().ok()).filter(|s| !s.is_empty()).map(|s| s.to_string()).collect::<Vec<_>>().join(" ");
    let select: Vec<(Q, String)> = q.select.clone().unwrap_or_else(|| (0..m.cols).map(|c| (Q::Col(c), String::new())).collect());
    let rows: Vec<usize> = (headers..m.rows).filter(|&r| q.filter.as_ref().is_none_or(|f| matches!(eval(f, &m, &[r]), Value::Bool(true)))).collect();
    let aggregated = select.iter().any(|(e, _)| e.has_agg()) || !q.group.is_empty();
    let mut groups: Vec<Vec<usize>> = if !q.group.is_empty() {
        let mut g: Vec<(Vec<Value>, Vec<usize>)> = Vec::new();
        // Groups by key: a scan of them all for each row would be quadratic.
        let mut index: std::collections::HashMap<String, usize> = Default::default();
        for &r in &rows {
            let key: Vec<Value> = q.group.iter().map(|e| eval(e, &m, &[r])).collect();
            match index.get(&super::google::key(&key)) {
                Some(&i) => g[i].1.push(r),
                None => {
                    index.insert(super::google::key(&key), g.len());
                    g.push((key, vec![r]));
                }
            }
        }
        // Groups come out in the order of their keys.
        g.sort_by(|x, y| x.0.iter().zip(&y.0).map(|(a, b)| a.compare(b)).find(|o| *o != Ordering::Equal).unwrap_or(Ordering::Equal));
        g.into_iter().map(|(_, rs)| rs).collect()
    } else if aggregated {
        vec![rows]
    } else {
        rows.into_iter().map(|r| vec![r]).collect()
    };
    // Columns outside an aggregation must be grouped.
    let loose = |e: &Q| !e.has_agg() && e.has_col() && !q.group.contains(e);
    if aggregated && (select.iter().any(|(e, _)| loose(e)) || q.order.iter().any(|(e, _)| loose(e))) {
        return Err(ErrorKind::Value);
    }
    if !q.order.is_empty() {
        let mut keyed: Vec<(Vec<Value>, Vec<usize>)> = groups.into_iter().map(|g| (q.order.iter().map(|(e, _)| eval(e, &m, &g)).collect(), g)).collect();
        keyed.sort_by(|x, y| {
            for (i, (_, asc)) in q.order.iter().enumerate() {
                let o = x.0[i].compare(&y.0[i]);
                if o != Ordering::Equal {
                    return if *asc { o } else { o.reverse() };
                }
            }
            Ordering::Equal
        });
        groups = keyed.into_iter().map(|(_, g)| g).collect();
    }
    let groups: Vec<Vec<usize>> = groups.into_iter().skip(q.offset).take(q.limit.unwrap_or(usize::MAX)).collect();
    let cols = select.len();
    if (groups.len() + 1).saturating_mul(cols) > MAX_ARRAY {
        return Err(ErrorKind::Num);
    }
    let mut data = Vec::with_capacity((groups.len() + 1) * cols);
    let titled = headers > 0 || aggregated || !q.labels.is_empty();
    if titled {
        for (e, src) in &select {
            let label = q.labels.iter().find(|(l, _)| l == e).map(|(_, s)| s.clone()).unwrap_or_else(|| match e {
                Q::Col(c) => header(*c),
                Q::Agg(k, inner) => match &**inner {
                    Q::Col(c) => format!("{} {}", k.name(), header(*c)).trim_end().to_string(),
                    _ => src.clone(),
                },
                _ => src.clone(),
            });
            data.push(if label.is_empty() { Value::Empty } else { Value::text(label) });
        }
    }
    for g in &groups {
        data.extend(select.iter().map(|(e, _)| eval(e, &m, g)));
    }
    if data.is_empty() || cols == 0 {
        return Err(ErrorKind::Na);
    }
    if data.len() > MAX_ARRAY {
        return Err(ErrorKind::Num);
    }
    Ok(Matrix::new(data.len() / cols, cols, data).into_value())
}

#[cfg(test)]
mod tests {
    use crate::{CellRef, Locale, Value, Workbook};

    fn wb() -> Workbook {
        let mut w = Workbook::new();
        w.locale = Locale::EN;
        let rows: [&[&str]; 7] = [
            &["Region", "Product", "Sales", "Date"],
            &["West", "Pen", "10", "2024-01-05"],
            &["East", "Ink", "5", "2024-02-01"],
            &["West", "Ink", "7", "2024-03-10"],
            &["East", "Pen", "2", "2023-12-31"],
            &["North", "Pen", "6", "2024-01-20"],
            &["West", "Pen", "4", ""],
        ];
        for (r, row) in rows.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                w.set_input(0, CellRef::new(r as u32, c as u32), v);
            }
        }
        w
    }

    fn t(w: &Workbook, f: &str) -> String {
        let s = |v: &Value| v.to_text().map(|t| t.to_string()).unwrap_or_else(|e| e.as_str().into());
        match w.eval_formula(0, f) {
            Value::Array(m) => m.data.chunks(m.cols).map(|r| r.iter().map(s).collect::<Vec<_>>().join(",")).collect::<Vec<_>>().join(";"),
            v => s(&v),
        }
    }

    #[test]
    fn query() {
        let w = wb();
        assert_eq!(t(&w, "=QUERY(A1:C7,\"select A, C where C > 5 order by C desc\")"), "Region,Sales;West,10;West,7;North,6");
        assert_eq!(t(&w, "=QUERY(A1:C7,\"select A, sum(C) group by A order by sum(C) desc label sum(C) 'Total'\",1)"), "Region,Total;West,21;East,7;North,6");
        assert_eq!(t(&w, "=QUERY(A1:C7,\"select A, count(B) where B = 'Pen' group by A\")"), "Region,count Product;East,1;North,1;West,2");
        assert_eq!(t(&w, "=QUERY(A1:C7,\"select B, avg(C) group by B\")"), "Product,avg Sales;Ink,6;Pen,5.5");
        assert_eq!(t(&w, "=QUERY(A2:C7,\"select A where A starts with 'W' and not C < 5 limit 1\",0)"), "West");
        assert_eq!(t(&w, "=QUERY(A1:C7,\"select * where A matches 'N.*' or A like '_ast'\")"), "Region,Product,Sales;East,Ink,5;East,Pen,2;North,Pen,6");
        assert_eq!(t(&w, "=QUERY(A1:C7,\"select A order by C limit 2 offset 1\")"), "Region;West;East");
        assert_eq!(t(&w, "=QUERY(A1:D7,\"select A where D >= date '2024-01-10' and D is not null\")"), "Region;East;West;North");
        assert_eq!(t(&w, "=QUERY(A1:D7,\"select max(D), min(C) where D is null\")"), "max Date,min Sales;,4");
        assert_eq!(t(&w, "=QUERY({1,\"a\";2,\"b\";3,\"a\"},\"select Col2, sum(Col1) group by Col2\")"), ",sum;a,4;b,2");
        assert_eq!(t(&w, "=QUERY(B1:C7,\"select C * 2 where B contains 'nk'\")"), "C * 2;10;14");
        assert_eq!(t(&w, "=QUERY(A1:C7,\"select A, C group by A\")"), "#VALUE!");
        assert_eq!(t(&w, "=QUERY(A1:C7,\"select Z\")"), "#VALUE!");
        // Deep nesting is refused, not a stack overflow.
        assert_eq!(t(&w, "=QUERY(A1:C7,\"select A where \"&REPT(\"(\",3000)&\"C > 5\"&REPT(\")\",3000))"), "#VALUE!");
        assert_eq!(t(&w, "=QUERY(A1:C7,\"select A where \"&REPT(\"not \",3000)&\"C > 5\")"), "#VALUE!");
        assert_eq!(t(&w, "=QUERY(A1:C7,\"select A where C > 100\",0)"), "#N/A");
    }
}
