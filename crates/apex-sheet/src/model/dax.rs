//! A subset of DAX: tokens, the expression tree and its parser, and the
//! pass that resolves table, column and measure names against the model.
//!
//! Supported: numbers, strings, `TRUE`/`FALSE`, `Table[Column]`,
//! `'Table name'[Column]`, `[Measure]`, `+ - * / ^ &`, comparisons,
//! `&& ||`, and the functions of [`FUNCS`].

use std::fmt;

use super::data::ModelData;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DaxErrorKind {
    /// Unexpected text; `arg` is what was found.
    Syntax,
    /// Something was expected (`arg`) and not found.
    Expected,
    UnknownFunction,
    /// Wrong number of arguments for function `arg`.
    Arity,
    /// Function `arg` needs a column reference.
    NeedColumn,
    /// Function `arg` needs a table.
    NeedTable,
    UnknownTable,
    /// `arg` is `Table[Column]`.
    UnknownColumn,
    UnknownMeasure,
    /// A column without its table outside of a row context.
    NeedRowContext,
    /// A CALCULATE filter must compare one column.
    FilterColumn,
    /// Measure `arg` refers back to itself.
    Circular,
    Empty,
}

impl DaxErrorKind {
    /// The interface text key of the error.
    pub fn key(self) -> &'static str {
        match self {
            DaxErrorKind::Syntax => "dax.err_syntax",
            DaxErrorKind::Expected => "dax.err_expected",
            DaxErrorKind::UnknownFunction => "dax.err_function",
            DaxErrorKind::Arity => "dax.err_arity",
            DaxErrorKind::NeedColumn => "dax.err_need_column",
            DaxErrorKind::NeedTable => "dax.err_need_table",
            DaxErrorKind::UnknownTable => "dax.err_table",
            DaxErrorKind::UnknownColumn => "dax.err_column",
            DaxErrorKind::UnknownMeasure => "dax.err_measure",
            DaxErrorKind::NeedRowContext => "dax.err_row_context",
            DaxErrorKind::FilterColumn => "dax.err_filter_column",
            DaxErrorKind::Circular => "dax.err_circular",
            DaxErrorKind::Empty => "dax.err_empty",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DaxError {
    pub kind: DaxErrorKind,
    pub arg: String,
    /// 1-based character position in the formula (0: unknown).
    pub pos: usize,
}

impl DaxError {
    fn new(kind: DaxErrorKind, arg: impl Into<String>, pos: usize) -> DaxError {
        DaxError { kind, arg: arg.into(), pos }
    }
}

impl fmt::Display for DaxError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?} {}", self.kind, self.arg)?;
        if self.pos > 0 {
            write!(f, " @{}", self.pos)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tokens

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Tok {
    Num(f64),
    Str(String),
    Ident(String),
    /// `'Table name'`.
    Quoted(String),
    /// `[Column]` or `[Measure]`.
    Bracket(String),
    Op(&'static str),
    LParen,
    RParen,
    Comma,
}

const OPS: [&str; 16] = ["&&", "||", "==", "<>", "<=", ">=", "<", ">", "=", "+", "-", "*", "/", "^", "&", "!"];

/// Tokens with their character spans.
pub(crate) fn lex(src: &str) -> Result<Vec<(Tok, usize, usize)>, DaxError> {
    let cs: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    // Closing-quote text: `'it''s'`, `[a]]b]`, `"say ""hi"""`.
    let quoted = |i: &mut usize, close: char| -> Option<String> {
        let mut s = String::new();
        *i += 1;
        while *i < cs.len() {
            if cs[*i] == close {
                if cs.get(*i + 1) == Some(&close) {
                    s.push(close);
                    *i += 2;
                    continue;
                }
                *i += 1;
                return Some(s);
            }
            s.push(cs[*i]);
            *i += 1;
        }
        None
    };
    while i < cs.len() {
        let c = cs[i];
        let start = i;
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // Comments.
        if (c == '/' && cs.get(i + 1) == Some(&'/')) || (c == '-' && cs.get(i + 1) == Some(&'-')) {
            while i < cs.len() && cs[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && cs.get(i + 1) == Some(&'*') {
            i += 2;
            while i < cs.len() && !(cs[i] == '*' && cs.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i = (i + 2).min(cs.len());
            continue;
        }
        let tok = match c {
            '"' => Tok::Str(quoted(&mut i, '"').ok_or_else(|| DaxError::new(DaxErrorKind::Expected, "\"", start + 1))?),
            '\'' => Tok::Quoted(quoted(&mut i, '\'').ok_or_else(|| DaxError::new(DaxErrorKind::Expected, "'", start + 1))?),
            '[' => Tok::Bracket(quoted(&mut i, ']').ok_or_else(|| DaxError::new(DaxErrorKind::Expected, "]", start + 1))?),
            '(' => {
                i += 1;
                Tok::LParen
            }
            ')' => {
                i += 1;
                Tok::RParen
            }
            ',' | ';' => {
                i += 1;
                Tok::Comma
            }
            _ if c.is_ascii_digit() || (c == '.' && cs.get(i + 1).is_some_and(|d| d.is_ascii_digit())) => {
                while i < cs.len() && (cs[i].is_ascii_digit() || cs[i] == '.') {
                    i += 1;
                }
                if i < cs.len() && (cs[i] == 'e' || cs[i] == 'E') {
                    let mut j = i + 1;
                    if j < cs.len() && (cs[j] == '+' || cs[j] == '-') {
                        j += 1;
                    }
                    if j < cs.len() && cs[j].is_ascii_digit() {
                        i = j;
                        while i < cs.len() && cs[i].is_ascii_digit() {
                            i += 1;
                        }
                    }
                }
                let t: String = cs[start..i].iter().collect();
                Tok::Num(t.parse().map_err(|_| DaxError::new(DaxErrorKind::Syntax, t.clone(), start + 1))?)
            }
            _ if c.is_alphabetic() || c == '_' => {
                while i < cs.len() && (cs[i].is_alphanumeric() || cs[i] == '_' || cs[i] == '.') {
                    i += 1;
                }
                Tok::Ident(cs[start..i].iter().collect())
            }
            _ => {
                let rest: String = cs[i..(i + 2).min(cs.len())].iter().collect();
                let Some(op) = OPS.iter().find(|o| rest.starts_with(**o)) else {
                    return Err(DaxError::new(DaxErrorKind::Syntax, c.to_string(), start + 1));
                };
                i += op.chars().count();
                Tok::Op(op)
            }
        };
        out.push((tok, start, i));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Expressions

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Cat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Func {
    Sum,
    Average,
    Min,
    Max,
    Count,
    CountA,
    CountBlank,
    CountRows,
    DistinctCount,
    Divide,
    Related,
    Calculate,
    Filter,
    All,
    Values,
    SumX,
    AverageX,
    MinX,
    MaxX,
    CountX,
    If,
    And,
    Or,
    Not,
    Blank,
    IsBlank,
    Abs,
    Round,
    True,
    False,
}

/// Name, function, least and most arguments.
pub const FUNCS: [(&str, Func, usize, usize); 30] = [
    ("SUM", Func::Sum, 1, 1),
    ("AVERAGE", Func::Average, 1, 1),
    ("MIN", Func::Min, 1, 2),
    ("MAX", Func::Max, 1, 2),
    ("COUNT", Func::Count, 1, 1),
    ("COUNTA", Func::CountA, 1, 1),
    ("COUNTBLANK", Func::CountBlank, 1, 1),
    ("COUNTROWS", Func::CountRows, 1, 1),
    ("DISTINCTCOUNT", Func::DistinctCount, 1, 1),
    ("DIVIDE", Func::Divide, 2, 3),
    ("RELATED", Func::Related, 1, 1),
    ("CALCULATE", Func::Calculate, 1, 64),
    ("FILTER", Func::Filter, 2, 2),
    ("ALL", Func::All, 1, 1),
    ("VALUES", Func::Values, 1, 1),
    ("SUMX", Func::SumX, 2, 2),
    ("AVERAGEX", Func::AverageX, 2, 2),
    ("MINX", Func::MinX, 2, 2),
    ("MAXX", Func::MaxX, 2, 2),
    ("COUNTX", Func::CountX, 2, 2),
    ("IF", Func::If, 2, 3),
    ("AND", Func::And, 2, 2),
    ("OR", Func::Or, 2, 2),
    ("NOT", Func::Not, 1, 1),
    ("BLANK", Func::Blank, 0, 0),
    ("ISBLANK", Func::IsBlank, 1, 1),
    ("ABS", Func::Abs, 1, 1),
    ("ROUND", Func::Round, 2, 2),
    ("TRUE", Func::True, 0, 0),
    ("FALSE", Func::False, 0, 0),
];

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Num(f64),
    Str(String),
    /// `Table[Column]`; the table is filled in by [`resolve`] for columns
    /// written bare inside a row context.
    Col { table: Option<String>, col: String },
    Table(String),
    Measure(String),
    Neg(Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Call(Func, Vec<Expr>),
}

impl Expr {
    /// Evaluates to a table (not a scalar).
    pub fn is_table(&self) -> bool {
        matches!(self, Expr::Table(_) | Expr::Call(Func::Filter | Func::All | Func::Values, _))
    }

    /// The table whose rows a table expression iterates.
    pub fn base_table(&self) -> Option<&str> {
        match self {
            Expr::Table(t) => Some(t),
            Expr::Call(Func::Filter, a) => a.first()?.base_table(),
            Expr::Call(Func::All | Func::Values, a) => match a.first()? {
                Expr::Col { table, .. } => table.as_deref(),
                e => e.base_table(),
            },
            _ => None,
        }
    }

    /// The columns an expression reads, as (table, column).
    pub fn columns(&self, out: &mut Vec<(String, String)>) {
        match self {
            Expr::Col { table: Some(t), col } => out.push((t.clone(), col.clone())),
            Expr::Neg(e) => e.columns(out),
            Expr::Bin(_, a, b) => {
                a.columns(out);
                b.columns(out);
            }
            Expr::Call(_, args) => args.iter().for_each(|a| a.columns(out)),
            _ => {}
        }
    }

    /// The measures an expression refers to.
    pub fn measures(&self, out: &mut Vec<String>) {
        match self {
            Expr::Measure(m) => out.push(m.clone()),
            Expr::Neg(e) => e.measures(out),
            Expr::Bin(_, a, b) => {
                a.measures(out);
                b.measures(out);
            }
            Expr::Call(_, args) => args.iter().for_each(|a| a.measures(out)),
            _ => {}
        }
    }
}

struct Parser<'a> {
    toks: Vec<(Tok, usize, usize)>,
    i: usize,
    end: usize,
    is_measure: &'a dyn Fn(&str) -> bool,
    /// How many brackets, signs and powers deep the parser is.
    depth: u32,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.i).map(|t| &t.0)
    }

    fn pos(&self) -> usize {
        self.toks.get(self.i).map_or(self.end + 1, |t| t.1 + 1)
    }

    fn found(&self) -> DaxError {
        match self.toks.get(self.i) {
            Some((t, ..)) => DaxError::new(DaxErrorKind::Syntax, tok_text(t), self.pos()),
            None => DaxError::new(DaxErrorKind::Expected, "…", self.pos()),
        }
    }

    fn op(&mut self, ops: &[&str]) -> Option<&'static str> {
        match self.peek() {
            Some(Tok::Op(o)) if ops.contains(o) => {
                let o = *o;
                self.i += 1;
                Some(o)
            }
            _ => None,
        }
    }

    fn binary(&mut self, level: usize) -> Result<Expr, DaxError> {
        const LEVELS: [&[&str]; 6] = [&["||"], &["&&"], &["=", "==", "<>", "<", "<=", ">", ">="], &["&"], &["+", "-"], &["*", "/"]];
        if level == LEVELS.len() {
            return self.power();
        }
        let mut a = self.binary(level + 1)?;
        while let Some(o) = self.op(LEVELS[level]) {
            let b = self.binary(level + 1)?;
            let op = match o {
                "||" => BinOp::Or,
                "&&" => BinOp::And,
                "=" | "==" => BinOp::Eq,
                "<>" => BinOp::Ne,
                "<" => BinOp::Lt,
                "<=" => BinOp::Le,
                ">" => BinOp::Gt,
                ">=" => BinOp::Ge,
                "&" => BinOp::Cat,
                "+" => BinOp::Add,
                "-" => BinOp::Sub,
                "*" => BinOp::Mul,
                _ => BinOp::Div,
            };
            a = Expr::Bin(op, Box::new(a), Box::new(b));
            // Comparisons don't chain.
            if level == 2 {
                break;
            }
        }
        Ok(a)
    }

    fn power(&mut self) -> Result<Expr, DaxError> {
        let a = self.unary()?;
        if self.op(&["^"]).is_some() {
            let b = self.nested(Self::power)?;
            return Ok(Expr::Bin(BinOp::Pow, Box::new(a), Box::new(b)));
        }
        Ok(a)
    }

    /// `f` one level deeper; past 200 the formula is refused, as it would
    /// overflow the stack.
    fn nested(&mut self, f: fn(&mut Self) -> Result<Expr, DaxError>) -> Result<Expr, DaxError> {
        if self.depth >= 200 {
            return Err(self.found());
        }
        self.depth += 1;
        let r = f(self);
        self.depth -= 1;
        r
    }

    fn unary(&mut self) -> Result<Expr, DaxError> {
        self.nested(Self::signed)
    }

    fn signed(&mut self) -> Result<Expr, DaxError> {
        if self.op(&["-"]).is_some() {
            return Ok(Expr::Neg(Box::new(self.unary()?)));
        }
        if self.op(&["+"]).is_some() {
            return self.unary();
        }
        if self.op(&["!"]).is_some() {
            return Ok(Expr::Call(Func::Not, vec![self.unary()?]));
        }
        self.primary()
    }

    fn column_after(&mut self, table: String) -> Expr {
        if let Some(Tok::Bracket(c)) = self.peek() {
            let col = c.clone();
            self.i += 1;
            return Expr::Col { table: Some(table), col };
        }
        Expr::Table(table)
    }

    fn primary(&mut self) -> Result<Expr, DaxError> {
        let pos = self.pos();
        let Some(t) = self.peek().cloned() else { return Err(self.found()) };
        self.i += 1;
        match t {
            Tok::Num(n) => Ok(Expr::Num(n)),
            Tok::Str(s) => Ok(Expr::Str(s)),
            Tok::LParen => {
                let e = self.binary(0)?;
                self.expect_close()?;
                Ok(e)
            }
            Tok::Bracket(name) => Ok(if (self.is_measure)(&name) { Expr::Measure(name) } else { Expr::Col { table: None, col: name } }),
            Tok::Quoted(t) => Ok(self.column_after(t)),
            Tok::Ident(name) => {
                if self.peek() == Some(&Tok::LParen) {
                    self.i += 1;
                    let up = name.to_uppercase();
                    let Some(&(_, f, lo, hi)) = FUNCS.iter().find(|x| x.0 == up) else {
                        return Err(DaxError::new(DaxErrorKind::UnknownFunction, name, pos));
                    };
                    let mut args = Vec::new();
                    if self.peek() != Some(&Tok::RParen) {
                        loop {
                            args.push(self.binary(0)?);
                            if self.peek() == Some(&Tok::Comma) {
                                self.i += 1;
                                continue;
                            }
                            break;
                        }
                    }
                    self.expect_close()?;
                    if args.len() < lo || args.len() > hi {
                        return Err(DaxError::new(DaxErrorKind::Arity, up, pos));
                    }
                    check_args(f, &up, &args, pos)?;
                    return Ok(Expr::Call(f, args));
                }
                match name.to_uppercase().as_str() {
                    "TRUE" if !matches!(self.peek(), Some(Tok::Bracket(_))) => return Ok(Expr::Call(Func::True, Vec::new())),
                    "FALSE" if !matches!(self.peek(), Some(Tok::Bracket(_))) => return Ok(Expr::Call(Func::False, Vec::new())),
                    _ => {}
                }
                Ok(self.column_after(name))
            }
            other => Err(DaxError::new(DaxErrorKind::Syntax, tok_text(&other), pos)),
        }
    }

    fn expect_close(&mut self) -> Result<(), DaxError> {
        if self.peek() == Some(&Tok::RParen) {
            self.i += 1;
            Ok(())
        } else if self.peek().is_none() {
            Err(DaxError::new(DaxErrorKind::Expected, ")", self.pos()))
        } else {
            Err(self.found())
        }
    }
}

fn tok_text(t: &Tok) -> String {
    match t {
        Tok::Num(n) => n.to_string(),
        Tok::Str(s) => format!("\"{s}\""),
        Tok::Ident(s) => s.clone(),
        Tok::Quoted(s) => format!("'{s}'"),
        Tok::Bracket(s) => format!("[{s}]"),
        Tok::Op(o) => o.to_string(),
        Tok::LParen => "(".into(),
        Tok::RParen => ")".into(),
        Tok::Comma => ",".into(),
    }
}

/// Arguments that must be columns or tables.
fn check_args(f: Func, name: &str, args: &[Expr], pos: usize) -> Result<(), DaxError> {
    let col = |e: &Expr| matches!(e, Expr::Col { .. });
    let need_col = || DaxError::new(DaxErrorKind::NeedColumn, name, pos);
    let need_table = || DaxError::new(DaxErrorKind::NeedTable, name, pos);
    match f {
        Func::Sum | Func::Average | Func::Count | Func::CountA | Func::CountBlank | Func::DistinctCount | Func::Related if !col(&args[0]) => Err(need_col()),
        Func::Min | Func::Max if args.len() == 1 && !col(&args[0]) => Err(need_col()),
        Func::CountRows | Func::Filter | Func::SumX | Func::AverageX | Func::MinX | Func::MaxX | Func::CountX if !args[0].is_table() => Err(need_table()),
        Func::All | Func::Values if !args[0].is_table() && !col(&args[0]) => Err(need_table()),
        _ => Ok(()),
    }
}

/// Parses a formula (a leading `=` is allowed). `is_measure` tells
/// `[Name]` measures from bare columns.
pub fn parse(src: &str, is_measure: &dyn Fn(&str) -> bool) -> Result<Expr, DaxError> {
    let body = src.trim_start();
    let body = body.strip_prefix('=').unwrap_or(body);
    let off = src.chars().count() - body.chars().count();
    let mut toks = lex(body)?;
    for t in &mut toks {
        t.1 += off;
        t.2 += off;
    }
    if toks.is_empty() {
        return Err(DaxError::new(DaxErrorKind::Empty, "", 0));
    }
    // Longer formulas make trees too deep to walk.
    if toks.len() > 8192 {
        return Err(DaxError::new(DaxErrorKind::Syntax, "…", 0));
    }
    let mut p = Parser { toks, i: 0, end: src.chars().count(), is_measure, depth: 0 };
    let e = p.binary(0)?;
    if p.i < p.toks.len() {
        return Err(p.found());
    }
    Ok(e)
}

/// Checks the names of an expression against the model and gives bare
/// columns the table of their row context. `row` is the table iterated
/// around the expression (FILTER, SUMX...).
pub fn resolve(e: &Expr, data: &ModelData, is_measure: &dyn Fn(&str) -> bool, row: Option<&str>) -> Result<Expr, DaxError> {
    let table = |t: &str| data.table(t).map(|i| data.tables[i].name.clone()).ok_or_else(|| DaxError::new(DaxErrorKind::UnknownTable, t, 0));
    Ok(match e {
        Expr::Table(t) => Expr::Table(table(t)?),
        Expr::Col { table: None, col } => match row {
            Some(t) => resolve(&Expr::Col { table: Some(t.to_string()), col: col.clone() }, data, is_measure, row)?,
            None => return Err(DaxError::new(if data.tables.iter().any(|t| t.columns.iter().any(|c| c.eq_ignore_ascii_case(col))) { DaxErrorKind::NeedRowContext } else { DaxErrorKind::UnknownMeasure }, format!("[{col}]"), 0)),
        },
        Expr::Col { table: Some(t), col } => {
            let name = table(t)?;
            let ti = data.table(&name).unwrap_or(0);
            match data.column(ti, col) {
                Some(ci) => Expr::Col { table: Some(name), col: data.tables[ti].columns[ci].clone() },
                // `Table[Measure]` is a measure too.
                None if is_measure(col) => Expr::Measure(col.clone()),
                None => return Err(DaxError::new(DaxErrorKind::UnknownColumn, format!("{name}[{col}]"), 0)),
            }
        }
        Expr::Measure(m) if !is_measure(m) => return Err(DaxError::new(DaxErrorKind::UnknownMeasure, format!("[{m}]"), 0)),
        Expr::Neg(a) => Expr::Neg(Box::new(resolve(a, data, is_measure, row)?)),
        Expr::Bin(op, a, b) => Expr::Bin(*op, Box::new(resolve(a, data, is_measure, row)?), Box::new(resolve(b, data, is_measure, row)?)),
        Expr::Call(f, args) => {
            let mut out = Vec::with_capacity(args.len());
            let iterates = matches!(f, Func::Filter | Func::SumX | Func::AverageX | Func::MinX | Func::MaxX | Func::CountX);
            for (i, a) in args.iter().enumerate() {
                let r = if iterates && i == 1 {
                    let t: Option<String> = out.first().and_then(|x: &Expr| x.base_table()).map(String::from);
                    resolve(a, data, is_measure, t.as_deref().or(row))?
                } else if *f == Func::Calculate && i > 0 && !a.is_table() {
                    // A boolean filter compares the column of one table.
                    let x = resolve(a, data, is_measure, row)?;
                    let mut cols = Vec::new();
                    x.columns(&mut cols);
                    cols.sort();
                    cols.dedup();
                    if cols.len() != 1 {
                        return Err(DaxError::new(DaxErrorKind::FilterColumn, "", 0));
                    }
                    x
                } else {
                    resolve(a, data, is_measure, row)?
                };
                out.push(r);
            }
            Expr::Call(*f, out)
        }
        other => other.clone(),
    })
}

/// Rewrites the references to table `old` in a formula.
pub fn rename_table(src: &str, old: &str, new: &str) -> String {
    let Ok(toks) = lex(src) else { return src.to_string() };
    let cs: Vec<char> = src.chars().collect();
    let plain = new.chars().all(|c| c.is_alphanumeric() || c == '_') && new.chars().next().is_some_and(|c| !c.is_ascii_digit());
    let quoted = if plain { new.to_string() } else { format!("'{}'", new.replace('\'', "''")) };
    let mut out = String::new();
    let mut last = 0;
    for (k, (t, a, b)) in toks.iter().enumerate() {
        let hit = match t {
            Tok::Ident(n) => n.eq_ignore_ascii_case(old) && toks.get(k + 1).map(|x| &x.0) != Some(&Tok::LParen),
            Tok::Quoted(n) => n.eq_ignore_ascii_case(old),
            _ => false,
        };
        if hit {
            out.extend(&cs[last..*a]);
            out.push_str(&quoted);
            last = *b;
        }
    }
    out.extend(&cs[last..]);
    out
}
