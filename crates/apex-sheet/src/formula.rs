//! Formula lexer, parser, printer and reference rewriting.
//!
//! Formulas are stored as an AST. The canonical text form (used in files)
//! is English: `.` decimals and `,` argument separators. The display form
//! follows the [`Locale`] (e.g. `;` and `,` decimals in Spanish).

use std::sync::Arc;

use crate::addr::{CellRef, MAX_COLS, MAX_ROWS, col_name, parse_col};
use crate::format::Locale;
use crate::functions::Func;
use crate::value::ErrorKind;

#[derive(Clone, Debug, PartialEq)]
pub struct CellAddr {
    pub row: u32,
    pub col: u32,
    pub row_abs: bool,
    pub col_abs: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeKind {
    Cells,
    /// `A:C`
    Cols,
    /// `1:3`
    Rows,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Number(f64),
    Text(Arc<str>),
    Bool(bool),
    Error(ErrorKind),
    /// An unknown name (evaluates to #NAME?, keeps its text).
    Name(Arc<str>),
    /// Omitted function argument: `IF(A1,,2)`.
    Missing,
    Ref {
        sheet: Option<Arc<str>>,
        addr: CellAddr,
    },
    Range {
        sheet: Option<Arc<str>>,
        a: CellAddr,
        b: CellAddr,
        kind: RangeKind,
    },
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Percent(Box<Expr>),
    Paren(Box<Expr>),
    Call {
        name: Arc<str>,
        func: Option<Func>,
        args: Vec<Expr>,
    },
    /// Array constant `{1,2;3,4}` (rows of literals).
    Array(Vec<Vec<Expr>>),
    /// The whole spill range of a dynamic array anchor: `A1#`.
    SpillRef {
        sheet: Option<Arc<str>>,
        addr: CellAddr,
    },
    /// An already evaluated value (built by the evaluator, never parsed).
    Lit(crate::value::Value),
    /// A structured reference to a table: `Table1[Sales]`, `[@Qty]`.
    Structured(Box<crate::table::StructRef>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Plus,
    /// `@`: implicit intersection (one value from a range or array).
    At,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    /// Intersection of two references: `A1:C3 B2:D4` (a space).
    Isect,
    /// Union of references: `(A1:B2,D4)`, only inside parentheses.
    Union,
    /// The range between two references that aren't plain cells:
    /// `A1:INDEX(B:B,5)`, `Start:End` with names.
    Range,
}

impl BinOp {
    fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Pow => "^",
            BinOp::Concat => "&",
            BinOp::Eq => "=",
            BinOp::Ne => "<>",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::Isect => " ",
            BinOp::Union => ",",
            BinOp::Range => ":",
        }
    }

    /// (left binding power, right binding power)
    fn bp(self) -> (u8, u8) {
        match self {
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => (1, 2),
            BinOp::Concat => (3, 4),
            BinOp::Add | BinOp::Sub => (5, 6),
            BinOp::Mul | BinOp::Div => (7, 8),
            // Excel evaluates ^ left-to-right.
            BinOp::Pow => (9, 10),
            // Reference operators bind tighter than everything else.
            BinOp::Isect => (15, 16),
            BinOp::Range => (17, 18),
            BinOp::Union => (0, 0),
        }
    }
}

/// What a part of a formula that predates dynamic arrays is calculated
/// for, which decides where implicit intersection happens.
#[derive(Clone, Copy, PartialEq)]
enum Ctx {
    /// One value: a range gives the cell in the formula's row or column.
    Value,
    /// A reference argument (SUM, COUNT, CHOOSE...): a range is taken
    /// whole, but what is calculated from it is one value.
    Ref,
    /// An array argument (SUMPRODUCT, LOOKUP's vectors...): everything in
    /// it is calculated for whole arrays.
    Array,
}

impl Ctx {
    /// The context of argument `i` of `f`.
    fn arg(self, f: Func, i: usize) -> Ctx {
        let bit = |mask: u64| (i < 64 && mask & (1 << i) != 0) || (i >= 64 && mask == u64::MAX);
        // IF still wants an array formula to take arrays, even inside
        // SUMPRODUCT.
        if self == Ctx::Array && f != Func::If {
            Ctx::Array
        } else if bit(f.array_class()) {
            Ctx::Array
        } else if bit(f.array_params()) {
            Ctx::Ref
        } else {
            Ctx::Value
        }
    }

    /// The context of the operands of an operator.
    fn operand(self) -> Ctx {
        if self == Ctx::Array { Ctx::Array } else { Ctx::Value }
    }
}

const PREFIX_BP: u8 = 11;
const POSTFIX_BP: u8 = 13;

#[derive(Clone, Debug, PartialEq)]
pub struct ParseError {
    pub msg: String,
    pub pos: usize,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (position {})", self.msg, self.pos)
    }
}

// ---------------------------------------------------------------------------
// Lexer

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Ident(String),
    Sheet(String),
    Err(ErrorKind),
    Op(&'static str),
    LParen,
    RParen,
    Sep,
    /// `;` (a row break inside array constants, else an argument separator).
    Semi,
    /// `\` (a column break inside array constants in some locales).
    Backslash,
    LBrace,
    RBrace,
    At,
    Hash,
    Colon,
    /// Whitespace between two references: the intersection operator.
    Space,
    /// A structured reference: optional table name, text between the
    /// outer brackets.
    Struct(Option<String>, String),
    End,
}

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
    decimal: char,
}

impl<'a> Lexer<'a> {
    fn err(&self, msg: &str) -> ParseError {
        ParseError { msg: msg.to_string(), pos: self.pos }
    }

    fn peek_char(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn tokens(mut self) -> Result<Vec<(Tok, usize)>, ParseError> {
        let mut out = Vec::new();
        let mut depth = 0i32;
        loop {
            let before = self.pos;
            while self.peek_char().is_some_and(char::is_whitespace) {
                self.pos += self.peek_char().unwrap().len_utf8();
            }
            if self.pos > before && depth == 0 && self.intersects(&out) {
                out.push((Tok::Space, before));
            }
            let start = self.pos;
            let Some(c) = self.peek_char() else {
                out.push((Tok::End, start));
                return Ok(out);
            };
            let tok = match c {
                '0'..='9' => self.number()?,
                c if c == self.decimal && self.src[self.pos + c.len_utf8()..].starts_with(|c: char| c.is_ascii_digit()) => self.number()?,
                '"' => self.string()?,
                '\'' => self.quoted_sheet()?,
                '#' if matches!(out.last(), Some((Tok::Ident(_), p)) if self.src[*p..start].chars().all(|c| !c.is_whitespace())) => {
                    self.pos += 1;
                    Tok::Hash
                }
                '#' => self.error_lit()?,
                '[' => {
                    // `Table1[...]` when the brackets follow a name directly.
                    let table = match out.last() {
                        Some((Tok::Ident(id), p)) if self.src[*p..start].chars().all(|c| !c.is_whitespace()) => {
                            let id = id.clone();
                            out.pop();
                            Some(id)
                        }
                        _ => None,
                    };
                    let body = self.brackets()?;
                    // Another workbook: `[1]Sheet1!A1`, `[1]!Name`.
                    if table.is_none() {
                        let rest = &self.src[self.pos..];
                        let len = rest.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.')).unwrap_or(rest.len());
                        // `[0]` is this workbook.
                        if body == "0" && rest[len..].starts_with('!') {
                            if len == 0 {
                                self.pos += 1;
                                continue;
                            }
                            let sheet = rest[..len].to_string();
                            self.pos += len + 1;
                            out.push((Tok::Sheet(sheet), start));
                            continue;
                        }
                        if rest[len..].starts_with('!') {
                            let sheet = format!("[{body}]{}", &rest[..len]);
                            self.pos += len + 1;
                            out.push((Tok::Sheet(sheet), start));
                            continue;
                        }
                    }
                    Tok::Struct(table, body)
                }
                '(' => {
                    self.pos += 1;
                    Tok::LParen
                }
                ')' => {
                    self.pos += 1;
                    Tok::RParen
                }
                ';' => {
                    self.pos += 1;
                    Tok::Semi
                }
                '{' | '}' | '@' => {
                    self.pos += 1;
                    match c {
                        '{' => {
                            depth += 1;
                            Tok::LBrace
                        }
                        '}' => {
                            depth -= 1;
                            Tok::RBrace
                        }
                        _ => Tok::At,
                    }
                }
                '\\' if depth > 0 => {
                    self.pos += 1;
                    Tok::Backslash
                }
                ',' => {
                    self.pos += 1;
                    Tok::Sep
                }
                ':' => {
                    self.pos += 1;
                    Tok::Colon
                }
                '+' | '-' | '*' | '/' | '^' | '&' | '=' | '%' => {
                    self.pos += 1;
                    Tok::Op(match c {
                        '+' => "+",
                        '-' => "-",
                        '*' => "*",
                        '/' => "/",
                        '^' => "^",
                        '&' => "&",
                        '=' => "=",
                        _ => "%",
                    })
                }
                '<' | '>' => {
                    self.pos += 1;
                    let next = self.peek_char();
                    let op = match (c, next) {
                        ('<', Some('=')) => "<=",
                        ('<', Some('>')) => "<>",
                        ('>', Some('=')) => ">=",
                        ('<', _) => "<",
                        _ => ">",
                    };
                    if op.len() == 2 {
                        self.pos += 1;
                    }
                    Tok::Op(op)
                }
                c if c.is_alphabetic() || c == '_' || c == '$' || c == '\\' => self.ident(),
                _ => return Err(self.err(&format!("unexpected character '{}'", c))),
            };
            out.push((tok, start));
        }
    }

    /// Whether the whitespace just skipped separates two references
    /// (`A1:B5 B2:C3`), making it the intersection operator.
    fn intersects(&self, out: &[(Tok, usize)]) -> bool {
        let prev_ref = match out.last() {
            Some((Tok::Ident(_) | Tok::RParen | Tok::Struct(..), _)) => true,
            // The end of a row range: `1:1 A:A`.
            Some((Tok::Num(_), _)) => matches!(out.len().checked_sub(2).map(|i| &out[i].0), Some(Tok::Colon)),
            _ => false,
        };
        if !prev_ref {
            return false;
        }
        match self.peek_char() {
            // `SUM (A1)` keeps being a call.
            Some('(') => !matches!(out.last(), Some((Tok::Ident(_), _))),
            Some(c) => c.is_alphabetic() || c == '_' || c == '$' || c == '\\' || c == '\'',
            None => false,
        }
    }

    /// The text between balanced brackets (`'` escapes the next character).
    fn brackets(&mut self) -> Result<String, ParseError> {
        let open = self.pos;
        self.pos += 1;
        let mut depth = 1;
        let body_start = self.pos;
        let mut it = self.src[self.pos..].char_indices();
        while let Some((i, c)) = it.next() {
            match c {
                '\'' => {
                    it.next();
                }
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        let body = self.src[body_start..body_start + i].to_string();
                        self.pos = body_start + i + 1;
                        return Ok(body);
                    }
                }
                _ => {}
            }
        }
        self.pos = open;
        Err(self.err("missing ']'"))
    }

    fn number(&mut self) -> Result<Tok, ParseError> {
        let b = self.src.as_bytes();
        let start = self.pos;
        let mut s = String::new();
        while self.pos < b.len() && b[self.pos].is_ascii_digit() {
            s.push(b[self.pos] as char);
            self.pos += 1;
        }
        let dl = self.decimal.len_utf8();
        if self.peek_char() == Some(self.decimal) && self.src[self.pos + dl..].starts_with(|c: char| c.is_ascii_digit()) || (self.peek_char() == Some(self.decimal) && !s.is_empty() && self.decimal == '.') {
            self.pos += dl;
            s.push('.');
            while self.pos < b.len() && b[self.pos].is_ascii_digit() {
                s.push(b[self.pos] as char);
                self.pos += 1;
            }
        }
        if self.pos < b.len() && (b[self.pos] == b'e' || b[self.pos] == b'E') {
            let save = self.pos;
            let mut e = String::from("e");
            self.pos += 1;
            if self.pos < b.len() && (b[self.pos] == b'+' || b[self.pos] == b'-') {
                e.push(b[self.pos] as char);
                self.pos += 1;
            }
            let ds = self.pos;
            while self.pos < b.len() && b[self.pos].is_ascii_digit() {
                e.push(b[self.pos] as char);
                self.pos += 1;
            }
            if ds == self.pos {
                self.pos = save;
            } else {
                s.push_str(&e);
            }
        }
        // `1:3` style row ranges are lexed as identifiers by the parser.
        s.parse::<f64>().map(Tok::Num).map_err(|_| ParseError { msg: "invalid number".into(), pos: start })
    }

    fn string(&mut self) -> Result<Tok, ParseError> {
        self.pos += 1;
        let mut s = String::new();
        loop {
            let Some(c) = self.peek_char() else {
                return Err(self.err("unterminated string"));
            };
            self.pos += c.len_utf8();
            if c == '"' {
                if self.peek_char() == Some('"') {
                    self.pos += 1;
                    s.push('"');
                } else {
                    return Ok(Tok::Str(s));
                }
            } else {
                s.push(c);
            }
        }
    }

    fn quoted_sheet(&mut self) -> Result<Tok, ParseError> {
        self.pos += 1;
        let mut s = String::new();
        loop {
            let Some(c) = self.peek_char() else {
                return Err(self.err("unterminated quoted sheet name"));
            };
            self.pos += c.len_utf8();
            if c == '\'' {
                if self.peek_char() == Some('\'') {
                    self.pos += 1;
                    s.push('\'');
                } else {
                    break;
                }
            } else {
                s.push(c);
            }
        }
        if self.peek_char() != Some('!') {
            return Err(self.err("expected '!' after the sheet name"));
        }
        self.pos += 1;
        Ok(Tok::Sheet(s))
    }

    fn error_lit(&mut self) -> Result<Tok, ParseError> {
        for lit in ["#DIV/0!", "#N/A", "#NAME?", "#NULL!", "#NUM!", "#REF!", "#VALUE!", "#CIRC!", "#SPILL!", "#CALC!"] {
            // `get`: the text may go on with a multi-byte character.
            if self.src.get(self.pos..self.pos + lit.len()).is_some_and(|x| x.eq_ignore_ascii_case(lit)) {
                self.pos += lit.len();
                return Ok(Tok::Err(ErrorKind::parse(lit).unwrap()));
            }
        }
        Err(self.err("unknown error literal"))
    }

    fn ident(&mut self) -> Tok {
        let start = self.pos;
        while let Some(c) = self.peek_char() {
            if c.is_alphanumeric() || c == '_' || c == '$' || c == '.' || c == '\\' {
                self.pos += c.len_utf8();
            } else {
                break;
            }
        }
        let mut s = self.src[start..self.pos].to_string();
        // Prefixes that xlsx files put on newer function and parameter names.
        for p in ["_xlfn.", "_xlws.", "_xlpm.", "_xleta."] {
            if s.len() > p.len() && s.get(..p.len()).is_some_and(|x| x.eq_ignore_ascii_case(p)) {
                s.drain(..p.len());
            }
        }
        if self.peek_char() == Some('!') {
            self.pos += 1;
            return Tok::Sheet(s);
        }
        Tok::Ident(s)
    }
}

// ---------------------------------------------------------------------------
// Parser

struct Parser {
    toks: Vec<(Tok, usize)>,
    i: usize,
    /// How deep the parser is in itself (parentheses, calls, signs).
    depth: usize,
}

/// Longest formula read (bytes): Excel's limit is 8192 characters.
pub const MAX_FORMULA_LEN: usize = 32 * 1024;
/// Deepest nesting of parentheses, calls and signs (Excel allows 64
/// levels of calls).
const MAX_NESTING: usize = 200;
/// Deepest formula tree (long chains like `A1+A2+...` are deep too):
/// evaluating and printing go down it on the stack.
const MAX_DEPTH: usize = 2048;

/// Whether `e` is deeper than `max`, without recursing (a file's formula
/// may be as deep as its length).
fn deeper_than(e: &Expr, max: usize) -> bool {
    let mut stack = vec![(e, 1usize)];
    while let Some((e, d)) = stack.pop() {
        if d > max {
            return true;
        }
        match e {
            Expr::Unary(_, a) | Expr::Percent(a) | Expr::Paren(a) => stack.push((a, d + 1)),
            Expr::Binary(_, a, b) => {
                stack.push((a, d + 1));
                stack.push((b, d + 1));
            }
            Expr::Call { args, .. } => stack.extend(args.iter().map(|a| (a, d + 1))),
            Expr::Array(rows) => stack.extend(rows.iter().flatten().map(|a| (a, d + 1))),
            _ => {}
        }
    }
    false
}

pub(crate) fn parse_addr(s: &str) -> Option<CellAddr> {
    let b = s.as_bytes();
    let mut i = 0;
    let col_abs = b.first() == Some(&b'$');
    if col_abs {
        i += 1;
    }
    let cs = i;
    while i < b.len() && b[i].is_ascii_alphabetic() {
        i += 1;
    }
    let col = parse_col(&s[cs..i])?;
    let row_abs = b.get(i) == Some(&b'$');
    if row_abs {
        i += 1;
    }
    let rs = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i != b.len() || rs == i {
        return None;
    }
    let row: u32 = s[rs..].parse().ok()?;
    if row == 0 || row > MAX_ROWS {
        return None;
    }
    Some(CellAddr { row: row - 1, col, row_abs, col_abs })
}

/// `$A` / `B` for whole-column ranges.
fn parse_col_only(s: &str) -> Option<(u32, bool)> {
    let abs = s.starts_with('$');
    Some((parse_col(s.trim_start_matches('$'))?, abs))
}

fn parse_row_only(s: &str) -> Option<(u32, bool)> {
    let abs = s.starts_with('$');
    let n: u32 = s.trim_start_matches('$').parse().ok()?;
    (n >= 1 && n <= MAX_ROWS).then_some((n - 1, abs))
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.i].0
    }

    fn pos(&self) -> usize {
        self.toks[self.i].1
    }

    fn next(&mut self) -> Tok {
        let t = self.toks[self.i].0.clone();
        if self.i < self.toks.len() - 1 {
            self.i += 1;
        }
        t
    }

    fn err<T>(&self, msg: &str) -> Result<T, ParseError> {
        Err(ParseError { msg: msg.to_string(), pos: self.pos() })
    }

    fn expr(&mut self, min_bp: u8) -> Result<Expr, ParseError> {
        if self.depth >= MAX_NESTING {
            return self.err("the formula is nested too deeply");
        }
        self.depth += 1;
        let r = self.expr_in(min_bp);
        self.depth -= 1;
        r
    }

    fn expr_in(&mut self, min_bp: u8) -> Result<Expr, ParseError> {
        let mut lhs = self.prefix()?;
        loop {
            let op = match self.peek() {
                Tok::Op("%") => {
                    if POSTFIX_BP < min_bp {
                        break;
                    }
                    self.next();
                    lhs = Expr::Percent(Box::new(lhs));
                    continue;
                }
                Tok::Space => BinOp::Isect,
                Tok::Colon => BinOp::Range,
                Tok::Op(o) => match *o {
                    "+" => BinOp::Add,
                    "-" => BinOp::Sub,
                    "*" => BinOp::Mul,
                    "/" => BinOp::Div,
                    "^" => BinOp::Pow,
                    "&" => BinOp::Concat,
                    "=" => BinOp::Eq,
                    "<>" => BinOp::Ne,
                    "<" => BinOp::Lt,
                    "<=" => BinOp::Le,
                    ">" => BinOp::Gt,
                    ">=" => BinOp::Ge,
                    _ => break,
                },
                _ => break,
            };
            let (l, r) = op.bp();
            if l < min_bp {
                break;
            }
            self.next();
            let rhs = self.expr(r)?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn prefix(&mut self) -> Result<Expr, ParseError> {
        match self.next() {
            Tok::Num(n) => {
                // Row range such as `1:3`.
                if n.fract() == 0.0 && *self.peek() == Tok::Colon {
                    if let Some(Tok::Num(m)) = self.toks.get(self.i + 1).map(|t| t.0.clone()) {
                        if let (Some(a), Some(b)) = (parse_row_only(&format!("{}", n as u64)), parse_row_only(&format!("{}", m as u64))) {
                            self.next();
                            self.next();
                            return Ok(row_range(None, a, b));
                        }
                    }
                }
                Ok(Expr::Number(n))
            }
            Tok::Str(s) => Ok(Expr::Text(intern(&s))),
            Tok::Err(e) => Ok(Expr::Error(e)),
            Tok::Op("-") => Ok(Expr::Unary(UnOp::Neg, Box::new(self.expr(PREFIX_BP)?))),
            Tok::Op("+") => Ok(Expr::Unary(UnOp::Plus, Box::new(self.expr(PREFIX_BP)?))),
            Tok::At => Ok(Expr::Unary(UnOp::At, Box::new(self.expr(PREFIX_BP)?))),
            Tok::LBrace => self.array(),
            Tok::LParen => {
                let mut e = self.expr(0)?;
                // A list of references in parentheses is their union.
                while matches!(self.peek(), Tok::Sep | Tok::Semi) && e.is_reference() {
                    self.next();
                    let rhs = self.expr(0)?;
                    if !rhs.is_reference() {
                        return self.err("missing ')'");
                    }
                    e = Expr::Binary(BinOp::Union, Box::new(e), Box::new(rhs));
                }
                if self.next() != Tok::RParen {
                    return self.err("missing ')'");
                }
                Ok(Expr::Paren(Box::new(e)))
            }
            Tok::Sheet(name) => {
                let sheet = Some(intern(&name));
                match self.next() {
                    Tok::Ident(id) => self.reference(sheet, &id),
                    Tok::Num(n) if n.fract() == 0.0 => self.reference(sheet, &format!("{}", n as u64)),
                    // A deleted reference: `Sheet1!#REF!`.
                    Tok::Err(k) => Ok(Expr::Error(k)),
                    _ => self.err("expected a reference after the sheet name"),
                }
            }
            Tok::Struct(table, body) => match crate::table::parse_struct(table.as_deref(), &body) {
                Some(r) => Ok(Expr::Structured(Box::new(r))),
                None => self.err("invalid table reference"),
            },
            Tok::Ident(id) => {
                if *self.peek() == Tok::LParen {
                    self.next();
                    return self.call(&id);
                }
                match id.to_ascii_uppercase().as_str() {
                    "TRUE" | "VERDADERO" => return Ok(Expr::Bool(true)),
                    "FALSE" | "FALSO" => return Ok(Expr::Bool(false)),
                    _ => {}
                }
                self.reference(None, &id)
            }
            Tok::End => self.err("incomplete formula"),
            _ => self.err("invalid expression"),
        }
    }

    /// `{1,2;3,4}` after the `{`.
    fn array(&mut self) -> Result<Expr, ParseError> {
        let mut rows = vec![Vec::new()];
        loop {
            let neg = match self.peek() {
                Tok::Op("-") => {
                    self.next();
                    true
                }
                _ => false,
            };
            // An empty element (`{1,,3}`, as some writers leave in defined
            // names) is kept empty, as in LibreOffice.
            let empty = !neg && matches!(self.peek(), Tok::Sep | Tok::Backslash | Tok::Semi | Tok::RBrace);
            let item = match if empty { Tok::End } else { self.next() } {
                _ if empty => Expr::Missing,
                Tok::Num(n) => Expr::Number(if neg { -n } else { n }),
                Tok::Str(s) if !neg => Expr::Text(intern(&s)),
                Tok::Err(e) if !neg => Expr::Error(e),
                Tok::Ident(id) if !neg && (id.eq_ignore_ascii_case("TRUE") || id.eq_ignore_ascii_case("VERDADERO")) => Expr::Bool(true),
                Tok::Ident(id) if !neg && (id.eq_ignore_ascii_case("FALSE") || id.eq_ignore_ascii_case("FALSO")) => Expr::Bool(false),
                _ => return self.err("array constants hold numbers, text, logical values or errors"),
            };
            rows.last_mut().unwrap().push(item);
            match self.next() {
                Tok::Sep | Tok::Backslash => {}
                Tok::Semi => rows.push(Vec::new()),
                Tok::RBrace => break,
                _ => return self.err("expected '}'"),
            }
        }
        if rows.iter().any(|r| r.len() != rows[0].len()) {
            return self.err("array rows have different lengths");
        }
        Ok(Expr::Array(rows))
    }

    fn reference(&mut self, sheet: Option<Arc<str>>, id: &str) -> Result<Expr, ParseError> {
        if *self.peek() == Tok::Hash {
            if let Some(addr) = parse_addr(id) {
                self.next();
                return Ok(Expr::SpillRef { sheet, addr });
            }
        }
        // A 3D reference: `Sheet1:Sheet3!A1`.
        if sheet.is_none() && *self.peek() == Tok::Colon {
            if let Some(Tok::Sheet(last)) = self.toks.get(self.i + 1).map(|t| t.0.clone()) {
                self.next();
                self.next();
                let sheet = Some(Arc::from(format!("{id}:{last}")));
                return match self.next() {
                    Tok::Ident(a) => self.reference(sheet, &a),
                    Tok::Num(n) if n.fract() == 0.0 => self.reference(sheet, &format!("{}", n as u64)),
                    _ => self.err("expected a reference after the sheet name"),
                };
            }
        }
        // A plain range; anything else after the `:` (a name, a function)
        // is the range operator.
        let plain = *self.peek() == Tok::Colon && {
            let at = |k: usize| self.toks.get(self.i + k).map(|t| &t.0);
            let (second, after) = match at(1) {
                Some(Tok::Sheet(_)) => (at(2), at(3)),
                t => (t, at(2)),
            };
            let second = match second {
                Some(Tok::Ident(s)) => Some(s.clone()),
                Some(Tok::Num(n)) if n.fract() == 0.0 => Some(format!("{}", *n as u64)),
                _ => None,
            };
            after != Some(&Tok::LParen)
                && second.is_some_and(|b| {
                    (parse_addr(id).is_some() && parse_addr(&b).is_some())
                        || (parse_col_only(id).is_some() && parse_col_only(&b).is_some())
                        || (parse_row_only(id).is_some() && parse_row_only(&b).is_some())
                })
        };
        if plain {
            self.next();
            let second = match self.next() {
                Tok::Ident(s) => s,
                Tok::Num(n) if n.fract() == 0.0 => format!("{}", n as u64),
                Tok::Sheet(_) => match self.next() {
                    Tok::Ident(s) => s,
                    _ => return self.err("invalid range"),
                },
                _ => return self.err("invalid range"),
            };
            if let (Some(a), Some(b)) = (parse_addr(id), parse_addr(&second)) {
                return Ok(Expr::Range { sheet, a, b, kind: RangeKind::Cells });
            }
            if let (Some(a), Some(b)) = (parse_col_only(id), parse_col_only(&second)) {
                return Ok(Expr::Range {
                    sheet,
                    a: CellAddr { row: 0, col: a.0, row_abs: true, col_abs: a.1 },
                    b: CellAddr { row: MAX_ROWS - 1, col: b.0, row_abs: true, col_abs: b.1 },
                    kind: RangeKind::Cols,
                });
            }
            if let (Some(a), Some(b)) = (parse_row_only(id), parse_row_only(&second)) {
                return Ok(row_range(sheet, a, b));
            }
            return self.err("invalid range");
        }
        match (parse_addr(id), sheet) {
            (Some(addr), sheet) => Ok(Expr::Ref { sheet, addr }),
            // A name local to another sheet: `Sheet2!Total`.
            (None, Some(s)) => Ok(Expr::Name(format!("{}!{id}", crate::chart::quote_sheet(&s)).into())),
            (None, None) => Ok(Expr::Name(id.into())),
        }
    }

    fn call(&mut self, name: &str) -> Result<Expr, ParseError> {
        let mut args = Vec::new();
        if *self.peek() == Tok::RParen {
            self.next();
        } else {
            loop {
                if matches!(self.peek(), Tok::Sep | Tok::Semi | Tok::RParen) {
                    args.push(Expr::Missing);
                } else {
                    args.push(self.expr(0)?);
                }
                match self.next() {
                    Tok::Sep | Tok::Semi => continue,
                    Tok::RParen => break,
                    _ => return self.err("expected ',' or ')' in function"),
                }
            }
        }
        // Functions that files use to spell operators.
        match (name.to_ascii_uppercase().as_str(), args.len()) {
            ("SINGLE", 1) => return Ok(Expr::Unary(UnOp::At, Box::new(args.pop().unwrap()))),
            // The logical constants as functions (how OpenDocument has to
            // write them): one spelling whatever the file.
            ("TRUE", 0) => return Ok(Expr::Bool(true)),
            ("FALSE", 0) => return Ok(Expr::Bool(false)),
            ("ANCHORARRAY", 1) => {
                if let Expr::Ref { sheet, addr } = &args[0] {
                    return Ok(Expr::SpillRef { sheet: sheet.clone(), addr: addr.clone() });
                }
            }
            _ => {}
        }
        let func = Func::lookup(name);
        // Function names passed as values read in canonical case (`_xleta.sum`).
        if let Some(f) = func {
            let n = args.len();
            for (i, a) in args.iter_mut().enumerate() {
                if let Expr::Name(x) = a
                    && f.takes_lambda(i, n)
                    && Func::lookup(x).is_some_and(|g| g.name().eq_ignore_ascii_case(x))
                {
                    *x = intern(Func::lookup(x).unwrap().name());
                }
            }
        }
        let name: Arc<str> = match func {
            Some(f) => intern(f.name()),
            None => intern(&name.to_uppercase()),
        };
        args.shrink_to_fit();
        Ok(Expr::Call { name, func, args })
    }
}

/// One shared copy of the short strings formulas repeat (function and
/// sheet names, short texts), so a million formulas do not keep a million
/// copies of `VLOOKUP`. Bounded: it starts over when it fills up.
fn intern(s: &str) -> Arc<str> {
    const MAX_LEN: usize = 64;
    const MAX_ENTRIES: usize = 8192;
    thread_local! {
        static SEEN: std::cell::RefCell<std::collections::HashSet<Arc<str>>> = Default::default();
    }
    if s.len() > MAX_LEN {
        return Arc::from(s);
    }
    SEEN.with_borrow_mut(|seen| {
        if let Some(a) = seen.get(s) {
            return a.clone();
        }
        if seen.len() >= MAX_ENTRIES {
            seen.clear();
        }
        let a: Arc<str> = Arc::from(s);
        seen.insert(a.clone());
        a
    })
}

fn row_range(sheet: Option<Arc<str>>, a: (u32, bool), b: (u32, bool)) -> Expr {
    Expr::Range {
        sheet,
        a: CellAddr { row: a.0, col: 0, row_abs: a.1, col_abs: true },
        b: CellAddr { row: b.0, col: MAX_COLS - 1, row_abs: b.1, col_abs: true },
        kind: RangeKind::Rows,
    }
}

/// Parses a formula (with or without the leading `=`).
/// `locale` decides whether `,` is a decimal separator.
pub fn parse(src: &str, locale: &Locale) -> Result<Expr, ParseError> {
    let body = src.strip_prefix('=').unwrap_or(src);
    let offset = src.len() - body.len();
    if body.len() > MAX_FORMULA_LEN {
        return Err(ParseError { msg: "the formula is too long".into(), pos: offset + MAX_FORMULA_LEN });
    }
    let toks = Lexer { src: body, pos: 0, decimal: if locale.decimal == ',' { ',' } else { '.' } }
        .tokens()
        .map_err(|e| ParseError { pos: e.pos + offset, ..e })?;
    let mut p = Parser { toks, i: 0, depth: 0 };
    let e = p.expr(0).map_err(|e| ParseError { pos: e.pos + offset, ..e })?;
    if *p.peek() != Tok::End {
        return Err(ParseError { msg: "unexpected text after the formula".into(), pos: p.pos() + offset });
    }
    if deeper_than(&e, MAX_DEPTH) {
        return Err(ParseError { msg: "the formula is nested too deeply".into(), pos: offset });
    }
    Ok(e)
}

/// Parses canonical (file) syntax: `.` decimal, `,` separator.
pub fn parse_canonical(src: &str) -> Result<Expr, ParseError> {
    parse(src, &Locale::EN)
}

// ---------------------------------------------------------------------------
// Printer

fn needs_quotes(name: &str) -> bool {
    // Another workbook: `[1]Sheet1`, `[1]` (its names).
    if let Some((_, rest)) = crate::external::split_external(name) {
        return !rest.is_empty() && needs_quotes(rest);
    }
    name.is_empty()
        || name.starts_with(|c: char| c.is_ascii_digit())
        || !name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.')
        || parse_addr(name).is_some()
}

fn write_sheet(out: &mut String, sheet: &Option<Arc<str>>) {
    if let Some(s) = sheet {
        let quote = match split_3d(s) {
            Some((a, b)) => needs_quotes(a) || needs_quotes(b),
            None => needs_quotes(s),
        };
        if quote {
            out.push('\'');
            out.push_str(&s.replace('\'', "''"));
            out.push('\'');
        } else {
            out.push_str(s);
        }
        out.push('!');
    }
}

fn write_addr(out: &mut String, a: &CellAddr) {
    if a.col_abs {
        out.push('$');
    }
    out.push_str(&col_name(a.col));
    if a.row_abs {
        out.push('$');
    }
    out.push_str(&(a.row + 1).to_string());
}

/// `R2C3`, `R[-1]C`, `RC[2]`: the row and/or column part of a reference
/// in R1C1 style, relative to `at`.
fn write_r1c1(out: &mut String, a: &CellAddr, at: CellRef, row: bool, col: bool) {
    let part = |out: &mut String, tag: char, v: u32, abs: bool, here: u32| {
        out.push(tag);
        if abs {
            out.push_str(&(v + 1).to_string());
        } else if v != here {
            out.push_str(&format!("[{}]", v as i64 - here as i64));
        }
    };
    if row {
        part(out, 'R', a.row, a.row_abs, at.row);
    }
    if col {
        part(out, 'C', a.col, a.col_abs, at.col);
    }
}

/// Rewrites the R1C1 references of a formula typed in cell `at` in A1
/// style (`=R[-1]C*2` in B3 is `=B2*2`); strings and quoted sheet names
/// are left alone. None when a reference falls off the sheet.
pub fn r1c1_to_a1(src: &str, at: CellRef) -> Option<String> {
    let cs: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '.' || c == '\\';
    while i < cs.len() {
        let c = cs[i];
        if c == '"' || c == '\'' {
            // Copy the quoted text as it is ("" and '' escape the quote).
            out.push(c);
            i += 1;
            while i < cs.len() {
                out.push(cs[i]);
                if cs[i] == c {
                    if cs.get(i + 1) == Some(&c) {
                        out.push(c);
                        i += 2;
                        continue;
                    }
                    break;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        let starts = (c == 'R' || c == 'r' || c == 'C' || c == 'c') && (i == 0 || !ident(cs[i - 1]));
        if starts {
            if let Some((a1, len)) = r1c1_ref(&cs[i..], at) {
                let next = cs.get(i + len).copied();
                if !next.is_some_and(|n| ident(n) || n == '(' || n == '!') {
                    out.push_str(&a1?);
                    i += len;
                    continue;
                }
            }
        }
        // Skip the rest of a word (function or name).
        if ident(c) {
            while i < cs.len() && ident(cs[i]) {
                out.push(cs[i]);
                i += 1;
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    Some(out)
}

/// One R1C1 reference at the start of `cs`, possibly a range
/// (`R1C1:R2C2`, `C2:C4`, `R[1]`): its A1 text and length.
fn r1c1_ref(cs: &[char], at: CellRef) -> Option<(Option<String>, usize)> {
    // One part: `R`, `R5`, `R[-2]`; returns (index, absolute, length).
    fn part(cs: &[char], tag: char, here: u32, max: u32) -> Option<(Option<u32>, bool, usize)> {
        if !cs.first()?.eq_ignore_ascii_case(&tag) {
            return None;
        }
        let rest = &cs[1..];
        if rest.first() == Some(&'[') {
            let end = rest.iter().position(|&c| c == ']')?;
            let n: i64 = rest[1..end].iter().collect::<String>().parse().ok()?;
            let v = here as i64 + n;
            return Some(((0..max as i64).contains(&v).then_some(v as u32), false, end + 2));
        }
        let digits = rest.iter().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 {
            return Some((Some(here), false, 1));
        }
        let n: u64 = rest[..digits].iter().collect::<String>().parse().ok()?;
        Some(((1..=max as u64).contains(&n).then(|| n as u32 - 1), true, digits + 1))
    }
    // One corner: row and/or column parts.
    let corner = |cs: &[char]| -> Option<(Option<(Option<u32>, bool)>, Option<(Option<u32>, bool)>, usize)> {
        let r = part(cs, 'R', at.row, MAX_ROWS);
        let off = r.map_or(0, |x| x.2);
        let c = part(&cs[off..], 'C', at.col, MAX_COLS);
        let len = off + c.map_or(0, |x| x.2);
        (len > 0).then(|| (r.map(|x| (x.0, x.1)), c.map(|x| (x.0, x.1)), len))
    };
    let a1 = |r: Option<(Option<u32>, bool)>, c: Option<(Option<u32>, bool)>| -> Option<String> {
        let mut s = String::new();
        if let Some((v, abs)) = c {
            if abs {
                s.push('$');
            }
            s.push_str(&col_name(v?));
        }
        if let Some((v, abs)) = r {
            if abs {
                s.push('$');
            }
            s.push_str(&(v? + 1).to_string());
        }
        Some(s)
    };
    let (r1, c1, n1) = corner(cs)?;
    if cs.get(n1) == Some(&':') {
        if let Some((r2, c2, n2)) = corner(&cs[n1 + 1..]) {
            if r1.is_some() == r2.is_some() && c1.is_some() == c2.is_some() {
                let text = a1(r1, c1).zip(a1(r2, c2)).map(|(a, b)| format!("{a}:{b}"));
                return Some((text, n1 + 1 + n2));
            }
        }
    }
    let text = match (r1, c1) {
        (Some(_), Some(_)) => a1(r1, c1),
        // A whole row or column alone: `R2` is `2:2`.
        _ => a1(r1, c1).map(|x| format!("{x}:{x}")),
    };
    Some((text, n1))
}

/// The first and last sheet of a 3D reference (`Sheet1:Sheet3`).
pub fn split_3d(sheet: &str) -> Option<(&str, &str)> {
    if crate::external::is_external(sheet) {
        return None;
    }
    sheet.split_once(':')
}

/// `kind`: whole columns and rows write just their column or row
/// (`[.C:.C]`, `[.$2:.$2]`), as LibreOffice does.
fn write_odf_addr(out: &mut String, sheet: &Option<Arc<str>>, a: &CellAddr, kind: RangeKind) {
    match sheet {
        // Another document: `'file:///home/u/Budget.ods'#$Sheet1.A1`.
        Some(s) if let Some((file, name)) = crate::external::split_external(s) => {
            out.push('\'');
            out.push_str(&file.replace('\'', "''"));
            out.push_str("'#$");
            if name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                out.push_str(name);
            } else {
                out.push('\'');
                out.push_str(&name.replace('\'', "''"));
                out.push('\'');
            }
        }
        Some(s) => {
            out.push('$');
            if s.chars().all(|c| c.is_alphanumeric() || c == '_') {
                out.push_str(s);
            } else {
                out.push('\'');
                out.push_str(&s.replace('\'', "''"));
                out.push('\'');
            }
        }
        None => {}
    }
    out.push('.');
    match kind {
        RangeKind::Cells => write_addr(out, a),
        RangeKind::Cols => {
            if a.col_abs {
                out.push('$');
            }
            out.push_str(&col_name(a.col));
        }
        RangeKind::Rows => {
            if a.row_abs {
                out.push('$');
            }
            out.push_str(&(a.row + 1).to_string());
        }
    }
}

fn write_number(out: &mut String, n: f64, decimal: char) {
    let s = if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", n as i64) } else { format!("{}", n) };
    if decimal == '.' {
        out.push_str(&s);
    } else {
        out.push_str(&s.replace('.', &decimal.to_string()));
    }
}

/// How a formula is written out.
struct Printer {
    out: String,
    dec: char,
    sep: char,
    odf: bool,
    /// xlsx: `_xlfn.` prefixes on newer functions, `_xlpm.` on LET and
    /// LAMBDA names.
    xlsx: bool,
    /// Names bound by the LET/LAMBDA calls being written.
    scope: Vec<Arc<str>>,
    /// The table holding the formula (left out of `[@Col]` references).
    own: Option<Arc<str>>,
    /// R1C1 style, relative to the formula's cell.
    r1c1: Option<CellRef>,
}

impl Printer {
    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Number(n) => write_number(&mut self.out, *n, self.dec),
            Expr::Text(s) => {
                self.out.push('"');
                self.out.push_str(&s.replace('"', "\"\""));
                self.out.push('"');
            }
            Expr::Bool(b) if self.odf => self.out.push_str(if *b { "TRUE()" } else { "FALSE()" }),
            Expr::Bool(b) => self.out.push_str(if *b { "TRUE" } else { "FALSE" }),
            Expr::Error(e) => self.out.push_str(e.as_str()),
            Expr::Name(n) => {
                if (self.xlsx || self.odf) && self.scope.iter().any(|s| s.eq_ignore_ascii_case(n)) {
                    self.out.push_str("_xlpm.");
                }
                self.out.push_str(n)
            }
            Expr::Missing => {}
            // The workbook names the table first (see `formula_for_odf`).
            Expr::Structured(r) if self.odf => r.write_odf(&mut self.out),
            Expr::Structured(r) => r.write(&mut self.out, self.xlsx, self.own.as_deref()),
            Expr::Lit(v) => match v {
                crate::value::Value::Number(n) => write_number(&mut self.out, *n, self.dec),
                crate::value::Value::Text(s) => self.expr(&Expr::Text(s.clone())),
                crate::value::Value::Bool(b) => self.expr(&Expr::Bool(*b)),
                crate::value::Value::Error(k) => self.out.push_str(k.as_str()),
                _ => self.out.push_str("#VALUE!"),
            },
            Expr::Ref { sheet, addr } if self.odf => {
                self.out.push('[');
                match sheet.as_deref().and_then(split_3d) {
                    Some((first, last)) => {
                        write_odf_addr(&mut self.out, &Some(Arc::from(first)), addr, RangeKind::Cells);
                        self.out.push(':');
                        write_odf_addr(&mut self.out, &Some(Arc::from(last)), addr, RangeKind::Cells);
                    }
                    None => write_odf_addr(&mut self.out, sheet, addr, RangeKind::Cells),
                }
                self.out.push(']');
            }
            Expr::SpillRef { sheet, addr } if self.odf || self.xlsx => {
                // Files spell the spill operator as a function.
                self.out.push_str("_xlfn.ANCHORARRAY(");
                let odf = self.odf;
                self.expr(&Expr::Ref { sheet: sheet.clone(), addr: addr.clone() });
                self.odf = odf;
                self.out.push(')');
            }
            Expr::SpillRef { sheet, addr } => {
                write_sheet(&mut self.out, sheet);
                match self.r1c1 {
                    Some(at) => write_r1c1(&mut self.out, addr, at, true, true),
                    None => write_addr(&mut self.out, addr),
                }
                self.out.push('#');
            }
            Expr::Ref { sheet, addr } if self.r1c1.is_some() => {
                write_sheet(&mut self.out, sheet);
                write_r1c1(&mut self.out, addr, self.r1c1.unwrap(), true, true);
            }
            Expr::Range { sheet, a, b, kind } if self.r1c1.is_some() => {
                let at = self.r1c1.unwrap();
                write_sheet(&mut self.out, sheet);
                let (row, col) = match kind {
                    RangeKind::Cells => (true, true),
                    RangeKind::Cols => (false, true),
                    RangeKind::Rows => (true, false),
                };
                write_r1c1(&mut self.out, a, at, row, col);
                // A single whole row or column needs no second part (`C2`).
                let same = match kind {
                    RangeKind::Cells => false,
                    RangeKind::Cols => (a.col, a.col_abs) == (b.col, b.col_abs),
                    RangeKind::Rows => (a.row, a.row_abs) == (b.row, b.row_abs),
                };
                if !same {
                    self.out.push(':');
                    write_r1c1(&mut self.out, b, at, row, col);
                }
            }
            Expr::Range { sheet, a, b, kind } if self.odf => {
                let kind = *kind;
                self.out.push('[');
                match sheet.as_deref().and_then(split_3d) {
                    Some((first, last)) => {
                        write_odf_addr(&mut self.out, &Some(Arc::from(first)), a, kind);
                        self.out.push(':');
                        write_odf_addr(&mut self.out, &Some(Arc::from(last)), b, kind);
                    }
                    None => {
                        write_odf_addr(&mut self.out, sheet, a, kind);
                        self.out.push(':');
                        write_odf_addr(&mut self.out, &None, b, kind);
                    }
                }
                self.out.push(']');
            }
            Expr::Ref { sheet, addr } => {
                write_sheet(&mut self.out, sheet);
                write_addr(&mut self.out, addr);
            }
            Expr::Range { sheet, a, b, kind } => {
                let out = &mut self.out;
                write_sheet(out, sheet);
                match kind {
                    RangeKind::Cells => {
                        write_addr(out, a);
                        out.push(':');
                        write_addr(out, b);
                    }
                    RangeKind::Cols => {
                        for (i, x) in [a, b].into_iter().enumerate() {
                            if i == 1 {
                                out.push(':');
                            }
                            if x.col_abs {
                                out.push('$');
                            }
                            out.push_str(&col_name(x.col));
                        }
                    }
                    RangeKind::Rows => {
                        for (i, x) in [a, b].into_iter().enumerate() {
                            if i == 1 {
                                out.push(':');
                            }
                            if x.row_abs {
                                out.push('$');
                            }
                            out.push_str(&(x.row + 1).to_string());
                        }
                    }
                }
            }
            Expr::Unary(UnOp::At, e) if self.xlsx || self.odf => {
                self.out.push_str("_xlfn.SINGLE(");
                self.expr(e);
                self.out.push(')');
            }
            Expr::Unary(op, e) => {
                self.out.push(match op {
                    UnOp::Neg => '-',
                    UnOp::Plus => '+',
                    UnOp::At => '@',
                });
                self.expr(e);
            }
            Expr::Binary(op, l, r) => {
                self.expr(l);
                match op {
                    // OpenFormula spells the reference operators `!` and `~`.
                    BinOp::Isect if self.odf => self.out.push('!'),
                    BinOp::Union if self.odf => self.out.push('~'),
                    BinOp::Union => self.out.push(self.sep),
                    _ => self.out.push_str(op.symbol()),
                }
                self.expr(r);
            }
            Expr::Percent(e) => {
                self.expr(e);
                self.out.push('%');
            }
            Expr::Paren(e) => {
                self.out.push('(');
                self.expr(e);
                self.out.push(')');
            }
            Expr::Array(rows) => {
                // OpenFormula: `;` between columns, `|` between rows.
                let (col, row) = if self.odf {
                    (';', '|')
                } else if self.sep == ';' {
                    ('\\', ';')
                } else {
                    (',', ';')
                };
                self.out.push('{');
                for (i, r) in rows.iter().enumerate() {
                    if i > 0 {
                        self.out.push(row);
                    }
                    for (j, x) in r.iter().enumerate() {
                        if j > 0 {
                            self.out.push(col);
                        }
                        self.expr(x);
                    }
                }
                self.out.push('}');
            }
            Expr::Call { name, func, args } => {
                match func {
                    Some(f) if self.odf => self.out.push_str(&f.odf_name()),
                    Some(f) if self.xlsx => {
                        self.out.push_str(f.xlsx_prefix());
                        self.out.push_str(name);
                    }
                    _ => self.out.push_str(name),
                }
                self.out.push('(');
                // LET and LAMBDA bind names for the arguments that follow.
                let depth = self.scope.len();
                let binder = matches!(func, Some(Func::Let | Func::Lambda));
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        self.out.push(self.sep);
                    }
                    let is_name = binder && i + 1 < args.len() && (*func == Some(Func::Lambda) || i % 2 == 0);
                    match a {
                        Expr::Name(n) if is_name => {
                            self.scope.push(n.clone());
                            if self.xlsx || self.odf {
                                self.out.push_str("_xlpm.");
                            }
                            self.out.push_str(n);
                        }
                        // A function passed as a value: `_xleta.SUM`.
                        Expr::Name(n) if (self.xlsx || self.odf) && func.is_some_and(|f| f.takes_lambda(i, args.len())) && !self.scope.iter().any(|s| s.eq_ignore_ascii_case(n)) && Func::lookup(n).is_some() => {
                            self.out.push_str("_xleta.");
                            self.out.push_str(Func::lookup(n).unwrap().name());
                        }
                        _ => self.expr(a),
                    }
                }
                self.scope.truncate(depth);
                self.out.push(')');
            }
        }
    }
}

impl Expr {
    fn print(&self, prefix: &str, dec: char, sep: char, odf: bool, xlsx: bool) -> String {
        let mut p = Printer { out: String::from(prefix), dec, sep, odf, xlsx, scope: Vec::new(), own: None, r1c1: None };
        p.expr(self);
        p.out
    }

    /// Canonical text without the leading `=`.
    pub fn to_canonical(&self) -> String {
        self.print("", '.', ',', false, false)
    }

    /// Text as xlsx stores it: canonical, with the `_xlfn.` prefixes Excel
    /// needs on functions newer than 2007.
    pub fn to_xlsx(&self) -> String {
        self.print("", '.', ',', false, true)
    }

    /// OpenFormula text as stored in ODF files (`of:=SUM([.A1:.B2])`).
    /// A formula from a file that predates dynamic arrays: ranges where a
    /// single value is expected get implicit intersection (`@`), as Excel
    /// does when it opens such files.
    pub fn legacy(self) -> Expr {
        fn walk(e: Expr, ctx: Ctx) -> Expr {
            let scalar = ctx == Ctx::Value;
            match e {
                Expr::Range { .. } if scalar => Expr::Unary(UnOp::At, Box::new(e)),
                // A name may stand for a range: once the names are known,
                // the `@` stays only on those (`Workbook::finish_load`).
                Expr::Name(_) if scalar => Expr::Unary(UnOp::At, Box::new(e)),
                // `[@Col]` is one cell already.
                Expr::Structured(ref r) if scalar && r.items & crate::table::items::THIS_ROW == 0 => Expr::Unary(UnOp::At, Box::new(e)),
                Expr::Call { name, func: Some(f), args } if !matches!(f, Func::Let | Func::Lambda) => {
                    let args: Vec<Expr> = args.into_iter().enumerate().map(|(i, a)| walk(a, Ctx::arg(ctx, f, i))).collect();
                    // What gives a range or an array gives one value too
                    // (FREQUENCY alone in a cell shows its first count).
                    use Func::*;
                    let multi = |a: Option<&Expr>| matches!(a, Some(Expr::Range { a, b, .. }) if a != b);
                    let not_one = |a: Option<&Expr>| !matches!(a, None | Some(Expr::Missing)) && !matches!(a, Some(Expr::Number(n)) if *n == 1.0);
                    let many = match f {
                        Frequency | Transpose | MMult | MInverse | Linest | Logest | Trend | Growth => true,
                        Row | Column => multi(args.first()),
                        Offset => not_one(args.get(3)) || not_one(args.get(4)),
                        _ => false,
                    };
                    let e = Expr::Call { name, func: Some(f), args };
                    if scalar && many { Expr::Unary(UnOp::At, Box::new(e)) } else { e }
                }
                // The reference operators work on whole ranges; the result
                // is what takes one value.
                Expr::Binary(op @ (BinOp::Isect | BinOp::Union | BinOp::Range), l, r) => {
                    let e = Expr::Binary(op, Box::new(walk(*l, Ctx::Ref)), Box::new(walk(*r, Ctx::Ref)));
                    if scalar && op != BinOp::Union { Expr::Unary(UnOp::At, Box::new(e)) } else { e }
                }
                Expr::Binary(op, l, r) => Expr::Binary(op, Box::new(walk(*l, ctx.operand())), Box::new(walk(*r, ctx.operand()))),
                Expr::Unary(op, x) if op != UnOp::At => Expr::Unary(op, Box::new(walk(*x, ctx.operand()))),
                Expr::Percent(x) => Expr::Percent(Box::new(walk(*x, ctx.operand()))),
                Expr::Paren(x) => Expr::Paren(Box::new(walk(*x, ctx))),
                e => e,
            }
        }
        walk(self, Ctx::Value)
    }

    /// Whether some name has an `@` on it.
    pub fn has_at_name(&self) -> bool {
        match self {
            Expr::Unary(UnOp::At, e) if matches!(**e, Expr::Name(_)) => true,
            Expr::Unary(_, e) | Expr::Percent(e) | Expr::Paren(e) => e.has_at_name(),
            Expr::Binary(_, l, r) => l.has_at_name() || r.has_at_name(),
            Expr::Call { args, .. } => args.iter().any(|a| a.has_at_name()),
            _ => false,
        }
    }

    /// Takes the `@` off the names for which `keep` is false.
    pub fn drop_at_names(&mut self, keep: &dyn Fn(&str) -> bool) {
        match self {
            Expr::Unary(UnOp::At, e) if matches!(&**e, Expr::Name(n) if !keep(n)) => *self = std::mem::replace(&mut **e, Expr::Missing),
            Expr::Unary(_, e) | Expr::Percent(e) | Expr::Paren(e) => e.drop_at_names(keep),
            Expr::Binary(_, l, r) => {
                l.drop_at_names(keep);
                r.drop_at_names(keep);
            }
            Expr::Call { args, .. } => args.iter_mut().for_each(|a| a.drop_at_names(keep)),
            _ => {}
        }
    }

    /// Whether read back as a plain formula (one that predates dynamic
    /// arrays) the expression would change: [`Expr::legacy`] would put an
    /// `@` on some range. Files keep such formulas as array formulas.
    pub fn needs_array(&self) -> bool {
        fn walk(e: &Expr, ctx: Ctx) -> bool {
            let scalar = ctx == Ctx::Value;
            match e {
                Expr::Range { .. } => scalar,
                Expr::Structured(r) => scalar && r.items & crate::table::items::THIS_ROW == 0,
                Expr::Call { func: Some(f), args, .. } if !matches!(f, Func::Let | Func::Lambda) => args.iter().enumerate().any(|(i, a)| walk(a, Ctx::arg(ctx, *f, i))),
                Expr::Binary(op @ (BinOp::Isect | BinOp::Union | BinOp::Range), l, r) => (scalar && *op != BinOp::Union) || walk(l, Ctx::Ref) || walk(r, Ctx::Ref),
                Expr::Binary(_, l, r) => walk(l, ctx.operand()) || walk(r, ctx.operand()),
                Expr::Unary(op, x) => *op != UnOp::At && walk(x, ctx.operand()),
                Expr::Percent(x) => walk(x, ctx.operand()),
                Expr::Paren(x) => walk(x, ctx),
                _ => false,
            }
        }
        walk(self, Ctx::Value)
    }

    /// Whether the expression can stand for cells (what the union and
    /// intersection operators take).
    pub fn is_reference(&self) -> bool {
        match self {
            Expr::Ref { .. } | Expr::Range { .. } | Expr::SpillRef { .. } | Expr::Structured(_) | Expr::Name(_) => true,
            Expr::Paren(e) => e.is_reference(),
            Expr::Binary(BinOp::Isect | BinOp::Union | BinOp::Range, ..) => true,
            Expr::Call { func, .. } => !matches!(func, Some(f) if !matches!(f, Func::Offset | Func::Indirect | Func::Index | Func::Choose | Func::If | Func::XLookup | Func::Let | Func::TrimRange)),
            _ => false,
        }
    }

    pub fn to_odf(&self) -> String {
        self.print("of:=", '.', ';', true, false)
    }

    /// Display text with `=` for the formula bar.
    pub fn to_display(&self, loc: &Locale) -> String {
        let sep = if loc.decimal == ',' { ';' } else { loc.arg_sep };
        self.print("=", loc.decimal, sep, false, false)
    }

    /// Display text in R1C1 style for the formula of cell `at`.
    pub fn to_display_r1c1(&self, loc: &Locale, at: CellRef) -> String {
        let sep = if loc.decimal == ',' { ';' } else { loc.arg_sep };
        let mut p = Printer { out: String::from("="), dec: loc.decimal, sep, odf: false, xlsx: false, scope: Vec::new(), own: None, r1c1: Some(at) };
        p.expr(self);
        p.out
    }

    /// Display text for a formula inside table `own`: its `[@Col]`
    /// references leave out the table name.
    pub fn to_display_in(&self, loc: &Locale, own: Option<&str>) -> String {
        let sep = if loc.decimal == ',' { ';' } else { loc.arg_sep };
        let mut p = Printer { out: String::from("="), dec: loc.decimal, sep, odf: false, xlsx: false, scope: Vec::new(), own: own.map(Arc::from), r1c1: None };
        p.expr(self);
        p.out
    }

    /// Whether the formula has structured references.
    pub fn has_structured(&self) -> bool {
        let mut found = false;
        self.visit_refs(&mut |e| found |= matches!(e, Expr::Structured(_)));
        found
    }

    /// Visits every structured reference mutably.
    pub fn visit_structured_mut(&mut self, f: &mut dyn FnMut(&mut Expr)) {
        match self {
            Expr::Structured(_) => f(self),
            Expr::Unary(_, e) | Expr::Percent(e) | Expr::Paren(e) => e.visit_structured_mut(f),
            Expr::Binary(_, l, r) => {
                l.visit_structured_mut(f);
                r.visit_structured_mut(f);
            }
            Expr::Call { args, .. } => args.iter_mut().for_each(|a| a.visit_structured_mut(f)),
            _ => {}
        }
    }

    /// Visits every reference (single cells and ranges) mutably.
    pub fn visit_refs_mut(&mut self, f: &mut dyn FnMut(&mut Expr)) {
        match self {
            Expr::Ref { .. } | Expr::Range { .. } | Expr::SpillRef { .. } => f(self),
            Expr::Unary(_, e) | Expr::Percent(e) | Expr::Paren(e) => e.visit_refs_mut(f),
            Expr::Binary(_, l, r) => {
                l.visit_refs_mut(f);
                r.visit_refs_mut(f);
            }
            Expr::Call { args, .. } => args.iter_mut().for_each(|a| a.visit_refs_mut(f)),
            _ => {}
        }
    }

    /// Visits every reference, structured ones included.
    pub fn visit_refs(&self, f: &mut dyn FnMut(&Expr)) {
        match self {
            Expr::Ref { .. } | Expr::Range { .. } | Expr::SpillRef { .. } | Expr::Structured(_) => f(self),
            Expr::Unary(_, e) | Expr::Percent(e) | Expr::Paren(e) => e.visit_refs(f),
            Expr::Binary(_, l, r) => {
                l.visit_refs(f);
                r.visit_refs(f);
            }
            Expr::Call { args, .. } => args.iter().for_each(|a| a.visit_refs(f)),
            _ => {}
        }
    }

    /// Visits every name (`Sales`, and `MyFunc` in `MyFunc(1)` when it is
    /// not a known function).
    pub fn visit_names(&self, f: &mut dyn FnMut(&str)) {
        match self {
            Expr::Name(n) => f(n),
            Expr::Call { func, name, args } => {
                if func.is_none() {
                    f(name);
                }
                args.iter().for_each(|a| a.visit_names(f));
            }
            Expr::Unary(_, e) | Expr::Percent(e) | Expr::Paren(e) => e.visit_names(f),
            Expr::Binary(_, l, r) => {
                l.visit_names(f);
                r.visit_names(f);
            }
            _ => {}
        }
    }

    /// Visits every name mutably.
    pub fn visit_names_mut(&mut self, f: &mut dyn FnMut(&mut Arc<str>)) {
        match self {
            Expr::Name(n) => f(n),
            Expr::Call { func, name, args } => {
                if func.is_none() {
                    f(name);
                }
                args.iter_mut().for_each(|a| a.visit_names_mut(f));
            }
            Expr::Unary(_, e) | Expr::Percent(e) | Expr::Paren(e) => e.visit_names_mut(f),
            Expr::Binary(_, l, r) => {
                l.visit_names_mut(f);
                r.visit_names_mut(f);
            }
            _ => {}
        }
    }

    /// Whether any reference is relative (moves when copied).
    pub fn has_relative_refs(&self) -> bool {
        let mut rel = false;
        self.visit_refs(&mut |e| match e {
            Expr::Ref { addr, .. } | Expr::SpillRef { addr, .. } => rel |= !addr.row_abs || !addr.col_abs,
            Expr::Range { a, b, kind, .. } => {
                rel |= match kind {
                    RangeKind::Cells => !(a.row_abs && a.col_abs && b.row_abs && b.col_abs),
                    RangeKind::Cols => !(a.col_abs && b.col_abs),
                    RangeKind::Rows => !(a.row_abs && b.row_abs),
                }
            }
            _ => {}
        });
        rel
    }

    /// Whether the formula calls `f` anywhere.
    /// Whether the formula refers to another workbook.
    pub fn has_external(&self) -> bool {
        let mut found = false;
        self.visit_refs(&mut |e| {
            if let Expr::Ref { sheet: Some(s), .. } | Expr::Range { sheet: Some(s), .. } = e {
                found |= crate::external::is_external(s);
            }
        });
        found
    }

    /// Calls `f` with the name of every call to a function that isn't
    /// built in (a defined LAMBDA name, an add-in or a macro).
    pub fn visit_unknown_calls(&self, f: &mut dyn FnMut(&str)) {
        match self {
            Expr::Call { func, name, args } => {
                if func.is_none() {
                    f(name);
                }
                args.iter().for_each(|a| a.visit_unknown_calls(f));
            }
            Expr::Unary(_, e) | Expr::Percent(e) | Expr::Paren(e) => e.visit_unknown_calls(f),
            Expr::Binary(_, l, r) => {
                l.visit_unknown_calls(f);
                r.visit_unknown_calls(f);
            }
            _ => {}
        }
    }

    pub fn calls(&self, f: crate::functions::Func) -> bool {
        match self {
            Expr::Call { func, args, .. } => *func == Some(f) || args.iter().any(|a| a.calls(f)),
            Expr::Unary(_, e) | Expr::Percent(e) | Expr::Paren(e) => e.calls(f),
            Expr::Binary(_, l, r) => l.calls(f) || r.calls(f),
            _ => false,
        }
    }

    pub fn is_volatile(&self) -> bool {
        match self {
            Expr::Call { func, args, .. } => func.is_some_and(|f| f.is_volatile()) || args.iter().any(Expr::is_volatile),
            Expr::Unary(_, e) | Expr::Percent(e) | Expr::Paren(e) => e.is_volatile(),
            Expr::Binary(_, l, r) => l.is_volatile() || r.is_volatile(),
            _ => false,
        }
    }

    /// Shifts relative references by (drow, dcol), as when copying a formula.
    /// References that fall off the sheet become `#REF!`.
    pub fn offset(&mut self, drow: i64, dcol: i64) {
        self.visit_refs_mut(&mut |e| {
            let ok = match e {
                Expr::Ref { addr, .. } | Expr::SpillRef { addr, .. } => shift_addr(addr, drow, dcol),
                Expr::Range { a, b, kind, .. } => {
                    let (dr, dc) = match kind {
                        RangeKind::Cells => (drow, dcol),
                        RangeKind::Cols => (0, dcol),
                        RangeKind::Rows => (drow, 0),
                    };
                    shift_addr(a, dr, dc) && shift_addr(b, dr, dc)
                }
                _ => true,
            };
            if !ok {
                *e = Expr::Error(ErrorKind::Ref);
            }
        });
    }

    /// Moves relative references by (drow, dcol), wrapping around the
    /// sheet edges: how the A1-relative references of a defined name
    /// follow the cell that uses it.
    pub fn offset_wrap(&mut self, drow: i64, dcol: i64) {
        let wrap = |a: &mut CellAddr, dr: i64, dc: i64| {
            if !a.row_abs {
                a.row = (a.row as i64 + dr).rem_euclid(MAX_ROWS as i64) as u32;
            }
            if !a.col_abs {
                a.col = (a.col as i64 + dc).rem_euclid(MAX_COLS as i64) as u32;
            }
        };
        self.visit_refs_mut(&mut |e| match e {
            Expr::Ref { addr, .. } | Expr::SpillRef { addr, .. } => wrap(addr, drow, dcol),
            Expr::Range { a, b, kind, .. } => {
                let (dr, dc) = match kind {
                    RangeKind::Cells => (drow, dcol),
                    RangeKind::Cols => (0, dcol),
                    RangeKind::Rows => (drow, 0),
                };
                wrap(a, dr, dc);
                wrap(b, dr, dc);
            }
            _ => {}
        });
    }

    /// Adjusts references after inserting (`count > 0`) or deleting
    /// (`count < 0`) rows or columns at `at` in the sheet `target`.
    /// `own_sheet` is the name of the sheet containing this formula.
    /// Returns true if anything changed.
    pub fn adjust_structure(&mut self, own_sheet: &str, target: &str, rows: bool, at: u32, count: i64) -> bool {
        let mut changed = false;
        self.visit_refs_mut(&mut |e| {
            let sheet_matches = |s: &Option<Arc<str>>| match s {
                Some(s) => s.eq_ignore_ascii_case(target),
                None => own_sheet.eq_ignore_ascii_case(target),
            };
            let before = e.clone();
            match e {
                Expr::Ref { sheet, addr } | Expr::SpillRef { sheet, addr } if sheet_matches(sheet) => {
                    let v = if rows { &mut addr.row } else { &mut addr.col };
                    if let Some(nv) = adjust_index(*v, at, count) {
                        *v = nv;
                    } else {
                        *e = Expr::Error(ErrorKind::Ref);
                    }
                }
                Expr::Range { sheet, a, b, kind } if sheet_matches(sheet) => {
                    let whole = matches!((kind, rows), (RangeKind::Cols, true) | (RangeKind::Rows, false));
                    if !whole {
                        let (s, t) = if rows { (&mut a.row, &mut b.row) } else { (&mut a.col, &mut b.col) };
                        match adjust_span(*s, *t, at, count) {
                            Some((ns, nt)) => {
                                *s = ns;
                                *t = nt;
                            }
                            None => *e = Expr::Error(ErrorKind::Ref),
                        }
                    }
                }
                _ => {}
            }
            if *e != before {
                changed = true;
            }
        });
        changed
    }

    /// Like [`Expr::adjust_structure`] for rows inserted or deleted one at
    /// a time at each of `points` (ascending, before the edit).
    pub fn adjust_rows_at(&mut self, own_sheet: &str, target: &str, points: &[u32], insert: bool) -> bool {
        use crate::sheet::map_index;
        let mut changed = false;
        self.visit_refs_mut(&mut |e| {
            let sheet_matches = |s: &Option<Arc<str>>| match s {
                Some(s) => s.eq_ignore_ascii_case(target),
                None => own_sheet.eq_ignore_ascii_case(target),
            };
            match e {
                Expr::Ref { sheet, addr } | Expr::SpillRef { sheet, addr } if sheet_matches(sheet) => match map_index(addr.row, points, insert) {
                    Some(r) if r == addr.row => {}
                    Some(r) => {
                        addr.row = r;
                        changed = true;
                    }
                    None => {
                        *e = Expr::Error(ErrorKind::Ref);
                        changed = true;
                    }
                },
                Expr::Range { sheet, a, b, kind } if sheet_matches(sheet) && !matches!(kind, RangeKind::Cols) => {
                    let (s, t) = (a.row, b.row);
                    let span = if insert {
                        Some((map_index(s, points, true).unwrap_or(s), map_index(t, points, true).unwrap_or(t)))
                    } else {
                        let before = |v: u32| points.partition_point(|&p| p < v) as u32;
                        let upto = |v: u32| points.partition_point(|&p| p <= v) as u32;
                        if upto(t) - before(s) == t - s + 1 {
                            None
                        } else {
                            let ns = s - before(s);
                            Some((ns, (t - upto(t)).max(ns)))
                        }
                    };
                    match span {
                        Some((ns, nt)) if (ns, nt) == (s, t) => {}
                        Some((ns, nt)) => {
                            a.row = ns;
                            b.row = nt;
                            changed = true;
                        }
                        None => {
                            *e = Expr::Error(ErrorKind::Ref);
                            changed = true;
                        }
                    }
                }
                _ => {}
            }
        });
        changed
    }

    /// Renames sheet references (`old!A1` -> `new!A1`).
    pub fn rename_sheet(&mut self, old: &str, new: &str) -> bool {
        let mut changed = false;
        self.visit_refs_mut(&mut |e| {
            if let Expr::Ref { sheet: Some(s), .. } | Expr::Range { sheet: Some(s), .. } | Expr::SpillRef { sheet: Some(s), .. } = e {
                if s.eq_ignore_ascii_case(old) {
                    *s = Arc::from(new);
                    changed = true;
                } else if let Some((a, b)) = split_3d(s) {
                    if a.eq_ignore_ascii_case(old) || b.eq_ignore_ascii_case(old) {
                        let side = |x: &str| if x.eq_ignore_ascii_case(old) { new.to_string() } else { x.to_string() };
                        *s = Arc::from(format!("{}:{}", side(a), side(b)));
                        changed = true;
                    }
                }
            }
        });
        changed
    }
}

fn shift_addr(a: &mut CellAddr, drow: i64, dcol: i64) -> bool {
    if !a.row_abs {
        let r = a.row as i64 + drow;
        if r < 0 || r >= MAX_ROWS as i64 {
            return false;
        }
        a.row = r as u32;
    }
    if !a.col_abs {
        let c = a.col as i64 + dcol;
        if c < 0 || c >= MAX_COLS as i64 {
            return false;
        }
        a.col = c as u32;
    }
    true
}

fn adjust_index(v: u32, at: u32, count: i64) -> Option<u32> {
    if v < at {
        return Some(v);
    }
    if count >= 0 {
        return Some((v as i64 + count) as u32);
    }
    let del_end = at as i64 - count; // exclusive
    if (v as i64) < del_end { None } else { Some((v as i64 + count) as u32) }
}

fn adjust_span(s: u32, t: u32, at: u32, count: i64) -> Option<(u32, u32)> {
    if count >= 0 {
        let ns = if s >= at { s + count as u32 } else { s };
        let nt = if t >= at { t + count as u32 } else { t };
        return Some((ns, nt));
    }
    let n = (-count) as u32;
    let del_end = at + n; // exclusive
    let map = |v: u32| if v < at { v } else if v >= del_end { v - n } else { at };
    if s >= at && t < del_end {
        return None; // entire span deleted
    }
    let ns = map(s);
    let nt = if t >= at && t < del_end { at.saturating_sub(1).max(ns) } else { map(t) };
    Some((ns, nt))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file's formula can't take the parser (or later the evaluator)
    /// deeper than the stack goes.
    #[test]
    fn deep_formulas_are_refused() {
        let parens = format!("={}1{}", "(".repeat(100_000), ")".repeat(100_000));
        assert!(parse_canonical(&parens).is_err());
        assert!(parse_canonical(&format!("={}1", "-".repeat(100_000))).is_err());
        assert!(parse_canonical(&format!("={}1", "1+".repeat(3000))).is_err());
        assert!(parse_canonical(&format!("={}1", "A1+".repeat(20_000))).is_err());
        // What people write still reads.
        assert!(parse_canonical(&format!("={}1{}", "(".repeat(64), ")".repeat(64))).is_ok());
        assert!(parse_canonical(&format!("={}1", "A1+".repeat(1500))).is_ok());
        let nested = (0..64).fold("1".to_string(), |acc, _| format!("IF(TRUE,{acc},0)"));
        assert!(parse_canonical(&nested).is_ok());
    }

    fn rt(s: &str) -> String {
        parse_canonical(s).unwrap().to_canonical()
    }

    #[test]
    fn r1c1_style() {
        let at = CellRef::new(2, 1); // B3
        let show = |f: &str| parse_canonical(f).unwrap().to_display_r1c1(&Locale::EN, at);
        assert_eq!(show("=B2*2+$A$1"), "=R[-1]C*2+R1C1");
        assert_eq!(show("=SUM(A1:C3)+Hoja2!B$5"), "=SUM(R[-2]C[-1]:RC[1])+Hoja2!R5C");
        assert_eq!(show("=SUM(C:C)+SUM($2:$4)+A1#"), "=SUM(C[1])+SUM(R2:R4)+R[-2]C[-1]#");
        let back = |f: &str| r1c1_to_a1(f, at).unwrap();
        assert_eq!(back("=R[-1]C*2+R1C1"), "=B2*2+$A$1");
        assert_eq!(back("=SUM(R[-2]C[-1]:RC[1])+Hoja2!R5C"), "=SUM(A1:C3)+Hoja2!B$5");
        assert_eq!(back("=SUM(C[1])+SUM(R2:R4)"), "=SUM(C:C)+SUM($2:$4)");
        // Functions, names and strings are left alone.
        assert_eq!(back("=ROUND(RC[1],2)&\"RC\"&Rate&'R1C1'!C1"), "=ROUND(C3,2)&\"RC\"&Rate&'R1C1'!$A:$A");
        assert_eq!(r1c1_to_a1("=R[-5]C", at), None);
    }

    #[test]
    fn roundtrip() {
        for s in [
            "SUM(A1:B2)",
            "A1+B2*3",
            "SU+1",
            "FOO(1)+myName",
            "(A1+B2)*3",
            "-A1^2",
            "IF(A1>=10,\"big\",\"small\")",
            "Sheet2!A1+'My Sheet'!$B$3",
            "SUM(A:A)",
            "SUM(1:3)",
            "10%",
            "\"a\"\"b\"&C1",
            "IF(A1,,2)",
        ] {
            assert_eq!(rt(s), s);
        }
    }

    #[test]
    fn spanish_locale() {
        let e = parse("=SUMA(A1;2,5)", &Locale::ES).unwrap();
        assert_eq!(e.to_canonical(), "SUM(A1,2.5)");
        assert_eq!(e.to_display(&Locale::ES), "=SUM(A1;2,5)");
    }

    #[test]
    fn offsets() {
        let mut e = parse_canonical("A1+$B$2+C$3+SUM(D1:D4)").unwrap();
        e.offset(2, 1);
        assert_eq!(e.to_canonical(), "B3+$B$2+D$3+SUM(E3:E6)");
        let mut e = parse_canonical("A1").unwrap();
        e.offset(-1, 0);
        assert_eq!(e.to_canonical(), "#REF!");
    }

    #[test]
    fn structure() {
        let mut e = parse_canonical("A5+SUM(A1:A10)+Other!A5").unwrap();
        e.adjust_structure("S", "S", true, 2, 3);
        assert_eq!(e.to_canonical(), "A8+SUM(A1:A13)+Other!A5");
        let mut e = parse_canonical("A5+SUM(A1:A10)").unwrap();
        e.adjust_structure("S", "S", true, 4, -1);
        assert_eq!(e.to_canonical(), "#REF!+SUM(A1:A9)");
        let mut e = parse_canonical("SUM(A3:A4)").unwrap();
        e.adjust_structure("S", "S", true, 2, -2);
        assert_eq!(e.to_canonical(), "SUM(#REF!)");
    }

    #[test]
    fn multibyte_names() {
        // Names shorter than a prefix or an error literal but longer in
        // bytes must not be cut inside a character.
        assert_eq!(rt("Прибыль+1"), "Прибыль+1");
        assert_eq!(rt("日本語名前+1"), "日本語名前+1");
        assert!(parse_canonical("#Прибыль").is_err());
        assert_eq!(rt("_xlfn.CONCAT(\"ы\")"), "CONCAT(\"ы\")");
        assert_eq!(rt("IF(TRUE(),FALSE(),TRUE)"), "IF(TRUE,FALSE,TRUE)");
        assert_eq!(parse("=1,5+Прибыль", &Locale::ES).unwrap().to_canonical(), "1.5+Прибыль");
    }

    #[test]
    fn precedence() {
        assert_eq!(rt("1+2*3^2"), "1+2*3^2");
        assert!(parse_canonical("1+").is_err());
        assert!(parse_canonical("SUM(1,2").is_err());
    }
}
