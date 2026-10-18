//! Cell values and errors.

use std::cmp::Ordering;
use std::sync::Arc;

thread_local! {
    /// The locale text is read in when a formula takes it as a number
    /// ("1,234.5", "31-Dec-2003"): the one of the formula being calculated.
    static COERCE_LOCALE: std::cell::Cell<Option<crate::format::Locale>> = const { std::cell::Cell::new(None) };
    /// Text that is not a number reads as 0 (a sheet with Lotus rules).
    static TEXT_ZERO: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Sets whether [`Value::to_number`] reads other text as 0 on this
/// thread; returns the previous setting.
pub(crate) fn set_text_zero(on: bool) -> bool {
    TEXT_ZERO.with(|z| z.replace(on))
}

/// Sets the locale of [`Value::to_number`] on this thread.
pub(crate) fn set_coerce_locale(loc: crate::format::Locale) {
    COERCE_LOCALE.with(|l| l.set(Some(loc)));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    Div0,
    Na,
    Name,
    Null,
    Num,
    Ref,
    Value,
    Circular,
    /// A dynamic array result that has no room to spill.
    Spill,
    /// A calculation the engine can't express, such as an empty array.
    Calc,
    /// Data from outside (IMAGE's picture) that could not be fetched.
    Connect,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Div0 => "#DIV/0!",
            ErrorKind::Na => "#N/A",
            ErrorKind::Name => "#NAME?",
            ErrorKind::Null => "#NULL!",
            ErrorKind::Num => "#NUM!",
            ErrorKind::Ref => "#REF!",
            ErrorKind::Value => "#VALUE!",
            ErrorKind::Circular => "#CIRC!",
            ErrorKind::Spill => "#SPILL!",
            ErrorKind::Calc => "#CALC!",
            ErrorKind::Connect => "#CONNECT!",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_uppercase().as_str() {
            "#DIV/0!" => ErrorKind::Div0,
            "#N/A" => ErrorKind::Na,
            "#NAME?" => ErrorKind::Name,
            "#NULL!" => ErrorKind::Null,
            "#NUM!" => ErrorKind::Num,
            "#REF!" => ErrorKind::Ref,
            "#VALUE!" => ErrorKind::Value,
            "#CIRC!" => ErrorKind::Circular,
            "#SPILL!" => ErrorKind::Spill,
            "#CALC!" => ErrorKind::Calc,
            "#CONNECT!" => ErrorKind::Connect,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum Value {
    #[default]
    Empty,
    Number(f64),
    Text(Arc<str>),
    Bool(bool),
    Error(ErrorKind),
    /// A formula result with several values (spills from its cell).
    /// Cells never store arrays.
    Array(Arc<Matrix>),
    /// A LAMBDA value (only lives during evaluation).
    Lambda(Arc<Lambda>),
}

/// A dense row-major array of values.
#[derive(Clone, Debug, PartialEq)]
pub struct Matrix {
    pub rows: usize,
    pub cols: usize,
    pub data: Vec<Value>,
}

impl Matrix {
    pub fn new(rows: usize, cols: usize, data: Vec<Value>) -> Self {
        debug_assert_eq!(rows * cols, data.len());
        // Arrays made count against the formula's budget.
        crate::functions::array_charge(data.len());
        Matrix { rows, cols, data }
    }

    pub fn filled(rows: usize, cols: usize, v: Value) -> Self {
        Matrix { rows, cols, data: vec![v; rows * cols] }
    }

    pub fn get(&self, r: usize, c: usize) -> &Value {
        &self.data[r * self.cols + c]
    }

    /// Element with broadcasting: a single row or column repeats, and
    /// anything else out of range is #N/A.
    pub fn at(&self, r: usize, c: usize) -> Value {
        let r = if self.rows == 1 { 0 } else { r };
        let c = if self.cols == 1 { 0 } else { c };
        if r < self.rows && c < self.cols { self.data[r * self.cols + c].clone() } else { Value::Error(ErrorKind::Na) }
    }

    pub fn into_value(self) -> Value {
        if self.rows == 1 && self.cols == 1 { self.data.into_iter().next().unwrap() } else { Value::Array(Arc::new(self)) }
    }
}

/// A function value: parameters, body and the names it closes over.
#[derive(Debug, PartialEq)]
pub struct Lambda {
    pub params: Vec<Arc<str>>,
    pub body: crate::formula::Expr,
    pub captured: Vec<(Arc<str>, Value)>,
}

/// Shares equal texts while a file loads: a column that repeats a few
/// names keeps one copy of each instead of one per cell.
#[derive(Default)]
pub struct Interner(std::collections::HashSet<Arc<str>>);

impl Interner {
    /// Past this many different texts new ones are no longer remembered.
    const MAX: usize = 1 << 20;

    pub fn text(&mut self, s: &str) -> Value {
        match self.0.get(s) {
            Some(t) => Value::Text(t.clone()),
            None => self.share(Value::text(s)),
        }
    }

    /// `v` with its text shared with the equal ones seen before.
    pub fn share(&mut self, v: Value) -> Value {
        match v {
            Value::Text(t) if t.len() <= 256 => match self.0.get(&t) {
                Some(s) => Value::Text(s.clone()),
                None => {
                    if self.0.len() < Self::MAX {
                        self.0.insert(t.clone());
                    }
                    Value::Text(t)
                }
            },
            v => v,
        }
    }
}

impl Value {
    pub fn text(s: impl Into<Arc<str>>) -> Self {
        Value::Text(s.into())
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, Value::Empty)
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// Coercion used by arithmetic operators.
    pub fn to_number(&self) -> Result<f64, ErrorKind> {
        match self {
            Value::Empty => Ok(0.0),
            Value::Number(n) => Ok(*n),
            Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
            Value::Text(s) => {
                let t = s.trim();
                let n = if t.is_empty() {
                    Err(ErrorKind::Value)
                } else if let Some(p) = t.strip_suffix('%') {
                    p.trim().parse::<f64>().map(|n| n / 100.0).map_err(|_| ErrorKind::Value)
                } else {
                    t.parse::<f64>().or_else(|_| COERCE_LOCALE.with(|l| l.get()).and_then(|loc| crate::functions::text_number(t, &loc)).ok_or(ErrorKind::Value))
                };
                if n.is_err() && TEXT_ZERO.with(|z| z.get()) { Ok(0.0) } else { n }
            }
            Value::Error(e) => Err(*e),
            Value::Array(_) | Value::Lambda(_) => Err(ErrorKind::Value),
        }
    }

    pub fn to_bool(&self) -> Result<bool, ErrorKind> {
        match self {
            Value::Empty => Ok(false),
            Value::Number(n) => Ok(*n != 0.0),
            Value::Bool(b) => Ok(*b),
            Value::Text(s) => match s.to_ascii_uppercase().as_str() {
                "TRUE" | "VERDADERO" => Ok(true),
                "FALSE" | "FALSO" => Ok(false),
                _ => Err(ErrorKind::Value),
            },
            Value::Error(e) => Err(*e),
            Value::Array(_) | Value::Lambda(_) => Err(ErrorKind::Value),
        }
    }

    /// Text coercion (the `&` operator, text functions). Numbers use the
    /// canonical, locale-independent representation.
    pub fn to_text(&self) -> Result<Arc<str>, ErrorKind> {
        match self {
            Value::Empty => Ok(Arc::from("")),
            Value::Number(n) => Ok(Arc::from(crate::format::general_number(*n, '.').as_str())),
            Value::Text(s) => Ok(s.clone()),
            Value::Bool(b) => Ok(Arc::from(if *b { "TRUE" } else { "FALSE" })),
            Value::Error(e) => Err(*e),
            Value::Array(_) | Value::Lambda(_) => Err(ErrorKind::Value),
        }
    }

    fn type_rank(&self) -> u8 {
        match self {
            Value::Empty | Value::Number(_) => 0,
            Value::Text(_) => 1,
            Value::Bool(_) => 2,
            Value::Error(_) | Value::Array(_) | Value::Lambda(_) => 3,
        }
    }

    /// Spreadsheet ordering: numbers < text < booleans; text is case-insensitive.
    pub fn compare(&self, other: &Value) -> Ordering {
        match (self, other) {
            (Value::Empty, Value::Empty) => Ordering::Equal,
            (Value::Empty, Value::Text(s)) => "".cmp(&*s.to_lowercase()),
            (Value::Text(s), Value::Empty) => s.to_lowercase().as_str().cmp(""),
            (Value::Empty, Value::Bool(b)) => false.cmp(b),
            (Value::Bool(b), Value::Empty) => b.cmp(&false),
            (a, b) if a.type_rank() != b.type_rank() => a.type_rank().cmp(&b.type_rank()),
            (a, b) if a.type_rank() == 0 => {
                let x = a.as_number().unwrap_or(0.0);
                let y = b.as_number().unwrap_or(0.0);
                x.partial_cmp(&y).unwrap_or(Ordering::Equal)
            }
            (Value::Text(a), Value::Text(b)) => cmp_text_ci(a, b),
            (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
            _ => Ordering::Equal,
        }
    }
}

pub fn cmp_text_ci(a: &str, b: &str) -> Ordering {
    // ASCII lowercases byte by byte, to the same order.
    if a.is_ascii() && b.is_ascii() {
        return a.bytes().map(|c| c.to_ascii_lowercase()).cmp(b.bytes().map(|c| c.to_ascii_lowercase()));
    }
    let mut ai = a.chars().flat_map(char::to_lowercase);
    let mut bi = b.chars().flat_map(char::to_lowercase);
    loop {
        match (ai.next(), bi.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x != y => return x.cmp(&y),
            _ => {}
        }
    }
}

impl From<f64> for Value {
    fn from(n: f64) -> Self {
        Value::Number(n)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Text(Arc::from(s))
    }
}
