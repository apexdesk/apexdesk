//! Values of the interpreter: the subtypes a Variant holds, arrays, and the
//! conversions and comparisons between them (VBA's rules, simplified:
//! integer arithmetic widens to Long and then to Double instead of
//! overflowing).

use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;

use apex_sheet::format::{self, Locale, NumFormat};

use crate::error::{Error, R};
use crate::object::Obj;

/// Declared type of a variable, parameter or function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VType {
    Variant,
    Boolean,
    Byte,
    Integer,
    Long,
    /// LongLong and LongPtr: kept as a whole Double.
    LongLong,
    /// Single, Double and Currency.
    Double,
    Currency,
    Date,
    String,
    /// Object and the classes of the object model (Range, Worksheet...).
    Object,
}

impl VType {
    /// The type named in `As …`; unknown names (classes) are objects.
    pub fn parse(name: &str) -> VType {
        match name.to_ascii_lowercase().as_str() {
            "variant" | "any" => VType::Variant,
            "boolean" => VType::Boolean,
            "byte" => VType::Byte,
            "integer" => VType::Integer,
            "long" => VType::Long,
            "longlong" | "longptr" => VType::LongLong,
            "single" | "double" | "decimal" => VType::Double,
            "currency" => VType::Currency,
            "date" => VType::Date,
            "string" => VType::String,
            _ => VType::Object,
        }
    }

    /// Value of a new variable of this type.
    pub fn default_value(self) -> Value {
        match self {
            VType::Variant => Value::Empty,
            VType::Boolean => Value::Bool(false),
            VType::Byte | VType::Integer => Value::Int(0),
            VType::Long => Value::Long(0),
            VType::LongLong | VType::Double | VType::Currency => Value::Double(0.0),
            VType::Date => Value::Date(0.0),
            VType::String => Value::Str(Rc::from("")),
            VType::Object => Value::Nothing,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum Value {
    #[default]
    Empty,
    Null,
    Bool(bool),
    Int(i16),
    Long(i32),
    Double(f64),
    /// Serial date (days since 1899-12-30, like the cells).
    Date(f64),
    Str(Rc<str>),
    Array(Rc<Array>),
    Object(Obj),
    Nothing,
    /// An Optional argument left out (IsMissing).
    Missing,
    /// An error value (CVErr, a cell with #N/A...): its number.
    Error(i32),
    /// A variable of a user-defined type (`Type ... End Type`): a value,
    /// copied on write like arrays.
    Record(Rc<Record>),
}

/// The value of a user-defined type: its name and fields in order.
#[derive(Clone, Debug)]
pub struct Record {
    pub name: Rc<str>,
    pub fields: Vec<Field>,
}

/// A field of a [`Record`]: what it takes (like a variable's slot) and
/// what it holds.
#[derive(Clone, Debug)]
pub struct Field {
    /// Lowercase.
    pub name: Rc<str>,
    pub ty: VType,
    /// `String * n`.
    pub fixed: Option<u32>,
    /// The user-defined type it holds (its elements', for an array).
    pub udt: Option<Rc<str>>,
    pub val: Value,
}

impl Record {
    pub fn field_mut(&mut self, name: &str) -> R<&mut Field> {
        self.fields.iter_mut().find(|f| f.name.eq_ignore_ascii_case(name)).ok_or_else(|| Error::new(461, format!("Method or data member not found: {name}")))
    }

    pub fn field(&self, name: &str) -> R<&Field> {
        self.fields.iter().find(|f| f.name.eq_ignore_ascii_case(name)).ok_or_else(|| Error::new(461, format!("Method or data member not found: {name}")))
    }
}

/// `s` as a `String * len`: cut or padded with spaces.
pub fn fixed_len(s: &str, len: u32) -> Value {
    let len = len as usize;
    let mut out: String = s.chars().take(len).collect();
    let have = out.chars().count();
    out.extend(std::iter::repeat_n(' ', len - have));
    Value::str(out)
}

/// An array: bounds of each dimension and the elements, the last index
/// running fastest. Arrays are values: `Rc::make_mut` copies on write.
#[derive(Clone, Debug)]
pub struct Array {
    pub bounds: Vec<(i64, i64)>,
    pub data: Vec<Value>,
    pub ty: VType,
}

/// Longer strings are refused (memory): VBA's "Out of string space".
pub const MAX_STRING: usize = 256 << 20;

/// Refuses a string of `bytes` bytes past [`MAX_STRING`].
pub fn string_room(bytes: usize) -> R<()> {
    if bytes > MAX_STRING { Err(Error::new(14, "Out of string space")) } else { Ok(()) }
}

/// Arrays of more elements are refused (memory).
const MAX_ELEMENTS: i64 = 50_000_000;

impl Array {
    pub fn new(bounds: Vec<(i64, i64)>, ty: VType) -> R<Array> {
        let mut n: i64 = 1;
        for &(lo, hi) in &bounds {
            if hi < lo.saturating_sub(1) {
                return Err(Error::subscript());
            }
            let len = hi.checked_sub(lo).and_then(|d| d.checked_add(1)).ok_or_else(|| Error::new(7, "Out of memory"))?;
            n = n.saturating_mul(len);
        }
        if n > MAX_ELEMENTS {
            return Err(Error::new(7, "Out of memory"));
        }
        Ok(Array { data: vec![ty.default_value(); n as usize], bounds, ty })
    }

    /// A 1-dimensional Variant array from `lower`.
    pub fn from_vec(data: Vec<Value>, lower: i64) -> Array {
        let hi = lower + data.len() as i64 - 1;
        Array { bounds: vec![(lower, hi)], data, ty: VType::Variant }
    }

    /// Position of the element at `idx`.
    pub fn offset(&self, idx: &[i64]) -> R<usize> {
        if idx.len() != self.bounds.len() {
            return Err(Error::subscript());
        }
        let mut at: usize = 0;
        for (&i, &(lo, hi)) in idx.iter().zip(&self.bounds) {
            if i < lo || i > hi {
                return Err(Error::subscript());
            }
            at = at * (hi - lo + 1) as usize + (i - lo) as usize;
        }
        Ok(at)
    }

    /// ReDim Preserve: the elements that still fit keep their place.
    pub fn resized(&self, bounds: Vec<(i64, i64)>) -> R<Array> {
        let mut out = Array::new(bounds, self.ty)?;
        if out.bounds.len() != self.bounds.len() {
            return Err(Error::subscript());
        }
        let dims = self.bounds.len();
        let mut idx: Vec<i64> = self.bounds.iter().map(|b| b.0).collect();
        if self.data.is_empty() {
            return Ok(out);
        }
        for v in &self.data {
            if let Ok(at) = out.offset(&idx) {
                out.data[at] = v.clone();
            }
            // Next index, last dimension fastest.
            for d in (0..dims).rev() {
                idx[d] += 1;
                if idx[d] <= self.bounds[d].1 {
                    break;
                }
                idx[d] = self.bounds[d].0;
            }
        }
        Ok(out)
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

/// Conversions that depend on the workbook's locale (dates as text).
#[derive(Clone, Copy)]
struct Conv {
    locale: Locale,
    date_code: &'static str,
}

thread_local! {
    static CONV: RefCell<Conv> = const { RefCell::new(Conv { locale: Locale::EN, date_code: "m/d/yyyy" }) };
}

/// Dates are shown and read in the order of `locale` (CStr, CDate).
pub fn set_locale(locale: Locale) {
    CONV.with(|c| *c.borrow_mut() = Conv { locale, date_code: locale.short_date_code() });
}

/// The locale of [`set_locale`].
pub fn locale() -> Locale {
    conv().locale
}

fn conv() -> Conv {
    CONV.with(|c| *c.borrow())
}

/// A Double as VBA shows it: up to 15 significant digits.
pub fn number_text(n: f64) -> String {
    if n == 0.0 {
        return "0".to_string();
    }
    if !n.is_finite() {
        return if n.is_nan() { "NaN".into() } else if n > 0.0 { "1.#INF".into() } else { "-1.#INF".into() };
    }
    let rounded: f64 = format!("{n:.14e}").parse().unwrap_or(n);
    let a = rounded.abs();
    if !(1e-4..1e15).contains(&a) {
        // 1.5E+20, 1E-05
        let s = format!("{rounded:.14e}");
        let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
        let mant = if mant.contains('.') { mant.trim_end_matches('0').trim_end_matches('.') } else { mant };
        let exp: i32 = exp.parse().unwrap_or(0);
        return format!("{mant}E{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs());
    }
    let s = format!("{rounded}");
    s
}

/// A date as CStr shows it: the short date, the time, or both.
pub fn date_text(d: f64) -> String {
    let c = conv();
    let code = if d.fract() == 0.0 {
        c.date_code.to_string()
    } else if d.trunc() == 0.0 {
        "hh:mm:ss".to_string()
    } else {
        format!("{} hh:mm:ss", c.date_code)
    };
    NumFormat::parse(&code).format_number(d, &c.locale).0
}

/// A number typed as text ("12", " -3.5 ", "1e3", "&HFF"), in VBA's rules
/// (the locale's decimal separator is also accepted).
pub fn parse_number(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let lower = t.to_ascii_lowercase();
    if let Some(h) = lower.strip_prefix("&h") {
        return i64::from_str_radix(h.trim_end_matches('&'), 16).ok().map(|v| v as f64);
    }
    if let Some(o) = lower.strip_prefix("&o") {
        return i64::from_str_radix(o.trim_end_matches('&'), 8).ok().map(|v| v as f64);
    }
    let ok = |x: &str| x.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-')) && x.bytes().any(|b| b.is_ascii_digit());
    if ok(t) {
        if let Ok(v) = t.parse::<f64>() {
            return Some(v);
        }
    }
    let c = conv();
    if c.locale.decimal == ',' {
        let alt = t.replace(c.locale.thousands, "").replace(',', ".");
        if ok(&alt) {
            return alt.parse().ok();
        }
    } else {
        let alt = t.replace(',', "");
        if alt != t && ok(&alt) {
            return alt.parse().ok();
        }
    }
    None
}

/// A date (and time) typed as text, in the locale's order or ISO.
pub fn parse_date(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let loc = conv().locale;
    let p = format::parse_input(t, &loc);
    if let (apex_sheet::Value::Number(n), Some(code)) = (&p.value, &p.format) {
        if NumFormat::parse(code).is_date() {
            return Some(*n);
        }
    }
    // "date time"
    if let Some((d, time)) = t.split_once(' ') {
        let (d, time) = (parse_date(d)?, parse_date(time)?);
        if d.fract() == 0.0 && d.trunc() != 0.0 && time.trunc() == 0.0 {
            return Some(d + time);
        }
    }
    None
}

impl Value {
    pub fn str(s: impl Into<Rc<str>>) -> Value {
        Value::Str(s.into())
    }

    pub fn is_object(&self) -> bool {
        matches!(self, Value::Object(_) | Value::Nothing)
    }

    /// Whether it is a number of any kind (not a string).
    pub fn is_numeric_type(&self) -> bool {
        matches!(self, Value::Int(_) | Value::Long(_) | Value::Double(_) | Value::Date(_) | Value::Bool(_) | Value::Empty)
    }

    pub fn to_f64(&self) -> R<f64> {
        match self {
            Value::Empty | Value::Missing => Ok(0.0),
            Value::Bool(b) => Ok(if *b { -1.0 } else { 0.0 }),
            Value::Int(i) => Ok(*i as f64),
            Value::Long(i) => Ok(*i as f64),
            Value::Double(d) | Value::Date(d) => Ok(*d),
            Value::Str(s) => parse_number(s).or_else(|| match s.trim().to_ascii_lowercase().as_str() {
                "true" => Some(-1.0),
                "false" => Some(0.0),
                _ => None,
            }).or_else(|| parse_date(s)).ok_or_else(Error::type_mismatch),
            Value::Null => Err(Error::new(94, "Invalid use of Null")),
            _ => Err(Error::type_mismatch()),
        }
    }

    /// A whole number (Long range), rounding half to even like CLng.
    pub fn to_i64(&self) -> R<i64> {
        let f = self.to_f64()?.round_ties_even();
        if f.abs() > 9.2e18 {
            return Err(Error::overflow());
        }
        Ok(f as i64)
    }

    pub fn to_long(&self) -> R<i32> {
        let v = self.to_i64()?;
        i32::try_from(v).map_err(|_| Error::overflow())
    }

    pub fn to_bool(&self) -> R<bool> {
        match self {
            Value::Bool(b) => Ok(*b),
            Value::Str(s) => match s.trim().to_ascii_lowercase().as_str() {
                "true" | "#true#" => Ok(true),
                "false" | "#false#" => Ok(false),
                _ => parse_number(s).map(|n| n != 0.0).ok_or_else(Error::type_mismatch),
            },
            v => Ok(v.to_f64()? != 0.0),
        }
    }

    pub fn to_str(&self) -> R<String> {
        Ok(match self {
            Value::Empty | Value::Missing => String::new(),
            Value::Bool(b) => if *b { "True" } else { "False" }.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Long(i) => i.to_string(),
            Value::Double(d) => number_text(*d),
            Value::Date(d) => date_text(*d),
            Value::Str(s) => s.to_string(),
            Value::Error(n) => format!("Error {n}"),
            Value::Null => return Err(Error::new(94, "Invalid use of Null")),
            _ => return Err(Error::type_mismatch()),
        })
    }

    pub fn to_date(&self) -> R<f64> {
        match self {
            Value::Str(s) => parse_date(s).or_else(|| parse_number(s)).ok_or_else(Error::type_mismatch),
            v => v.to_f64(),
        }
    }

    /// The value converted for a variable of type `ty` (Let, not Set).
    pub fn coerce(self, ty: VType) -> R<Value> {
        Ok(match ty {
            VType::Variant => self,
            _ if matches!(self, Value::Record(_)) => match ty {
                VType::Object => self,
                _ => return Err(Error::type_mismatch()),
            },
            VType::Boolean => Value::Bool(self.to_bool()?),
            VType::Byte => {
                let v = self.to_i64()?;
                if !(0..=255).contains(&v) {
                    return Err(Error::overflow());
                }
                Value::Int(v as i16)
            }
            VType::Integer => Value::Int(i16::try_from(self.to_i64()?).map_err(|_| Error::overflow())?),
            VType::Long => Value::Long(self.to_long()?),
            VType::LongLong => Value::Double(self.to_i64()? as f64),
            VType::Double => Value::Double(self.to_f64()?),
            VType::Currency => Value::Double((self.to_f64()? * 10_000.0).round_ties_even() / 10_000.0),
            VType::Date => Value::Date(self.to_date()?),
            VType::String => Value::str(self.to_str()?),
            VType::Object => match self {
                Value::Object(_) | Value::Nothing => self,
                _ => return Err(Error::object_required()),
            },
        })
    }

    /// TypeName.
    pub fn type_name(&self) -> String {
        match self {
            Value::Empty => "Empty".into(),
            Value::Null => "Null".into(),
            Value::Bool(_) => "Boolean".into(),
            Value::Int(_) => "Integer".into(),
            Value::Long(_) => "Long".into(),
            Value::Double(_) => "Double".into(),
            Value::Date(_) => "Date".into(),
            Value::Str(_) => "String".into(),
            Value::Array(a) => format!("{}()", Value::type_of(a.ty)),
            Value::Object(Obj::Instance(i)) => i.class.to_string(),
            Value::Object(o) => o.type_name().into(),
            Value::Nothing => "Nothing".into(),
            Value::Missing => "Error".into(),
            Value::Error(_) => "Error".into(),
            Value::Record(r) => r.name.to_string(),
        }
    }

    fn type_of(ty: VType) -> &'static str {
        match ty {
            VType::Variant => "Variant",
            VType::Boolean => "Boolean",
            VType::Byte => "Byte",
            VType::Integer => "Integer",
            VType::Long => "Long",
            VType::LongLong => "LongLong",
            VType::Double => "Double",
            VType::Currency => "Currency",
            VType::Date => "Date",
            VType::String => "String",
            VType::Object => "Object",
        }
    }

    /// VarType.
    pub fn var_type(&self) -> i32 {
        match self {
            Value::Empty => 0,
            Value::Null => 1,
            Value::Int(_) => 2,
            Value::Long(_) => 3,
            Value::Double(_) => 5,
            Value::Date(_) => 7,
            Value::Str(_) => 8,
            Value::Object(_) | Value::Nothing => 9,
            Value::Error(_) | Value::Missing => 10,
            Value::Bool(_) => 11,
            Value::Record(_) => 36,
            Value::Array(a) => 8192 + match a.ty {
                VType::Variant => 12,
                t => t.default_value().var_type(),
            },
        }
    }
}

/// Number result of arithmetic: Long when whole and in range, else Double.
pub fn num(v: f64, integer: bool) -> Value {
    if integer && v.fract() == 0.0 && v >= i32::MIN as f64 && v <= i32::MAX as f64 {
        if v >= i16::MIN as f64 && v <= i16::MAX as f64 { Value::Int(v as i16) } else { Value::Long(v as i32) }
    } else {
        Value::Double(v)
    }
}

/// Whether arithmetic on `v` stays integral.
pub fn is_integral(v: &Value) -> bool {
    matches!(v, Value::Int(_) | Value::Long(_) | Value::Bool(_) | Value::Empty)
}

/// Compares two scalars: numbers as numbers, strings as strings (case
/// ignored with `text`), a number and a numeric string as numbers.
/// `None` when one is Null.
pub fn compare(a: &Value, b: &Value, text: bool) -> R<Option<Ordering>> {
    if matches!(a, Value::Null) || matches!(b, Value::Null) {
        return Ok(None);
    }
    let cmp_str = |x: &str, y: &str| if text { x.to_lowercase().cmp(&y.to_lowercase()) } else { x.cmp(y) };
    Ok(Some(match (a, b) {
        (Value::Str(x), Value::Str(y)) => cmp_str(x, y),
        (Value::Str(x), Value::Empty) => cmp_str(x, ""),
        (Value::Empty, Value::Str(y)) => cmp_str("", y),
        (Value::Str(s), n) if n.is_numeric_type() => match parse_number(s) {
            Some(x) => x.partial_cmp(&n.to_f64()?).unwrap_or(Ordering::Equal),
            None => Ordering::Greater,
        },
        (n, Value::Str(s)) if n.is_numeric_type() => match parse_number(s) {
            Some(y) => n.to_f64()?.partial_cmp(&y).unwrap_or(Ordering::Equal),
            None => Ordering::Less,
        },
        (Value::Error(x), Value::Error(y)) => x.cmp(y),
        (x, y) => x.to_f64()?.partial_cmp(&y.to_f64()?).unwrap_or(Ordering::Equal),
    }))
}

/// `Like`: `?` one character, `*` any run, `#` a digit, `[a-z]` and `[!…]`
/// sets.
pub fn like(s: &str, pat: &str, text: bool) -> bool {
    enum Tok {
        Star,
        Any,
        Digit,
        Set(bool, Vec<char>),
        Lit(char),
    }
    let fold = |c: char| if text { c.to_lowercase().next().unwrap_or(c) } else { c };
    let s: Vec<char> = s.chars().map(fold).collect();
    let p: Vec<char> = pat.chars().map(fold).collect();
    let mut toks = Vec::new();
    let mut k = 0;
    while k < p.len() {
        toks.push(match p[k] {
            '*' => Tok::Star,
            '?' => Tok::Any,
            '#' => Tok::Digit,
            '[' => {
                // An unclosed set matches nothing.
                let Some(end) = p[k..].iter().position(|&x| x == ']').map(|e| k + e) else { return false };
                let set = &p[k + 1..end];
                let (neg, set) = if set.first() == Some(&'!') { (true, &set[1..]) } else { (false, set) };
                k = end;
                Tok::Set(neg, set.to_vec())
            }
            c => Tok::Lit(c),
        });
        k += 1;
    }
    let one = |t: &Tok, ch: char| match t {
        Tok::Star => false,
        Tok::Any => true,
        Tok::Digit => ch.is_ascii_digit(),
        Tok::Lit(c) => *c == ch,
        Tok::Set(neg, set) => {
            let mut hit = false;
            let mut i = 0;
            while i < set.len() {
                if i + 2 < set.len() && set[i + 1] == '-' {
                    hit |= set[i] <= ch && ch <= set[i + 2];
                    i += 3;
                } else {
                    hit |= set[i] == ch;
                    i += 1;
                }
            }
            hit != *neg
        }
    };
    // Greedy, going back only to the last star: no backtracking blow-up
    // (and no recursion) whatever the pattern.
    let (mut i, mut j) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while i < s.len() {
        if matches!(toks.get(j), Some(Tok::Star)) {
            star = Some((j, i));
            j += 1;
        } else if toks.get(j).is_some_and(|t| one(t, s[i])) {
            i += 1;
            j += 1;
        } else if let Some((sj, si)) = star {
            star = Some((sj, si + 1));
            j = sj + 1;
            i = si + 1;
        } else {
            return false;
        }
    }
    toks[j..].iter().all(|t| matches!(t, Tok::Star))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_as_text() {
        assert_eq!(number_text(0.1 + 0.2), "0.3");
        assert_eq!(number_text(2.5), "2.5");
        assert_eq!(number_text(-3.0), "-3");
        assert_eq!(number_text(1e20), "1E+20");
        assert_eq!(number_text(1.5e-5), "1.5E-05");
        assert_eq!(number_text(123456.789), "123456.789");
        assert_eq!(parse_number(" 12.5 "), Some(12.5));
        assert_eq!(parse_number("&HFF"), Some(255.0));
        assert_eq!(parse_number("abc"), None);
    }

    #[test]
    fn coercions() {
        assert!(matches!(Value::Double(2.5).coerce(VType::Integer), Ok(Value::Int(2))));
        assert!(matches!(Value::Double(3.5).coerce(VType::Integer), Ok(Value::Int(4))));
        assert_eq!(Value::Double(40000.0).coerce(VType::Integer).unwrap_err().number, 6);
        assert_eq!(Value::str("x").coerce(VType::Long).unwrap_err().number, 13);
        assert!(matches!(Value::str("12").coerce(VType::Long), Ok(Value::Long(12))));
        assert!(Value::str("True").to_bool().unwrap());
        assert_eq!(Value::Bool(true).to_str().unwrap(), "True");
    }

    #[test]
    fn arrays() {
        let mut a = Array::new(vec![(1, 2), (0, 2)], VType::Long).unwrap();
        assert_eq!(a.len(), 6);
        let at = a.offset(&[2, 1]).unwrap();
        a.data[at] = Value::Long(7);
        let b = a.resized(vec![(1, 2), (0, 4)]).unwrap();
        assert!(matches!(b.data[b.offset(&[2, 1]).unwrap()], Value::Long(7)));
        assert_eq!(a.offset(&[3, 0]).unwrap_err().number, 9);
    }

    #[test]
    fn like_patterns() {
        assert!(like("Hello", "H*o", false));
        assert!(like("a1", "?#", false));
        assert!(like("b", "[a-c]", false));
        assert!(!like("d", "[a-c]", false));
        assert!(like("d", "[!a-c]", false));
        assert!(like("HELLO", "h*", true));
        assert!(!like("HELLO", "h*", false));
        assert!(!like(&"a".repeat(5000), "*a*a*a*a*a*a*a*a*b", false));
        assert!(like("axbbyd", "a*b?d", false));
        assert!(!like("x", "[ab", false));
    }
}
