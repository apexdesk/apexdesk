//! Formulas of calculated columns: `[Price] * [Qty]`, `[First] & " " &
//! [Last]`, `IF([Qty] > 10, "big", "small")`, `UPPER(LEFT([Code], 3))`.
//! Columns go in brackets; the operators and functions are the usual
//! spreadsheet ones (names in English), evaluated row by row.

use crate::table::{DataTable, Val};

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Col(String),
    Ident(String),
    Op(&'static str),
    LParen,
    RParen,
    Comma,
}

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let mut out = Vec::new();
    let cs: Vec<char> = src.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        match c {
            _ if c.is_whitespace() => i += 1,
            '[' => {
                let end = cs[i..].iter().position(|&x| x == ']').ok_or("missing ]")? + i;
                out.push(Tok::Col(cs[i + 1..end].iter().collect()));
                i = end + 1;
            }
            '"' => {
                let mut s = String::new();
                i += 1;
                loop {
                    match cs.get(i) {
                        None => return Err("missing \"".into()),
                        Some('"') if cs.get(i + 1) == Some(&'"') => {
                            s.push('"');
                            i += 2;
                        }
                        Some('"') => {
                            i += 1;
                            break;
                        }
                        Some(&x) => {
                            s.push(x);
                            i += 1;
                        }
                    }
                }
                out.push(Tok::Str(s));
            }
            '0'..='9' | '.' => {
                let start = i;
                while i < cs.len() && (cs[i].is_ascii_digit() || cs[i] == '.' || ((cs[i] == 'e' || cs[i] == 'E') && i + 1 < cs.len() && (cs[i + 1].is_ascii_digit() || cs[i + 1] == '-'))) {
                    if cs[i] == 'e' || cs[i] == 'E' {
                        i += 1;
                    }
                    i += 1;
                }
                let t: String = cs[start..i].iter().collect();
                out.push(Tok::Num(t.parse().map_err(|_| format!("bad number {t}"))?));
            }
            '(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            ',' | ';' => {
                out.push(Tok::Comma);
                i += 1;
            }
            '<' | '>' | '=' | '!' => {
                let two: String = cs[i..(i + 2).min(cs.len())].iter().collect();
                let op = match two.as_str() {
                    "<=" => "<=",
                    ">=" => ">=",
                    "<>" | "!=" => "<>",
                    "==" => "=",
                    _ => "",
                };
                if !op.is_empty() {
                    out.push(Tok::Op(op));
                    i += 2;
                } else {
                    out.push(Tok::Op(match c {
                        '<' => "<",
                        '>' => ">",
                        '=' => "=",
                        _ => return Err("unexpected !".into()),
                    }));
                    i += 1;
                }
            }
            '+' | '-' | '*' | '/' | '^' | '&' | '%' => {
                out.push(Tok::Op(match c {
                    '+' => "+",
                    '-' => "-",
                    '*' => "*",
                    '/' => "/",
                    '^' => "^",
                    '&' => "&",
                    _ => "%",
                }));
                i += 1;
            }
            _ if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < cs.len() && (cs[i].is_alphanumeric() || cs[i] == '_' || cs[i] == '.') {
                    i += 1;
                }
                out.push(Tok::Ident(cs[start..i].iter().collect::<String>().to_ascii_uppercase()));
            }
            _ => return Err(format!("unexpected {c}")),
        }
    }
    Ok(out)
}

/// A parsed formula.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Lit(Val),
    /// Column index in the table the formula was bound to.
    Col(usize),
    Neg(Box<Expr>),
    Bin(&'static str, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
}

struct Parser<'a> {
    toks: Vec<Tok>,
    pos: usize,
    table: &'a DataTable,
    /// How many brackets, signs and powers deep the parser is.
    depth: u32,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn bump(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    /// `f` one level deeper; past 200 the formula is refused, as it would
    /// overflow the stack.
    fn nested(&mut self, f: fn(&mut Self) -> Result<Expr, String>) -> Result<Expr, String> {
        if self.depth >= 200 {
            return Err("the formula is nested too deeply".into());
        }
        self.depth += 1;
        let r = f(self);
        self.depth -= 1;
        r
    }

    fn whole(&mut self) -> Result<Expr, String> {
        self.binary(0)
    }

    fn binary(&mut self, level: usize) -> Result<Expr, String> {
        const LEVELS: [&[&str]; 4] = [&["=", "<>", "<", ">", "<=", ">="], &["&"], &["+", "-"], &["*", "/"]];
        if level == LEVELS.len() {
            return self.power();
        }
        let mut l = self.binary(level + 1)?;
        while let Some(Tok::Op(op)) = self.peek() {
            let op = *op;
            if !LEVELS[level].contains(&op) {
                break;
            }
            self.pos += 1;
            let r = self.binary(level + 1)?;
            l = Expr::Bin(op, Box::new(l), Box::new(r));
        }
        Ok(l)
    }

    fn power(&mut self) -> Result<Expr, String> {
        let base = self.unary()?;
        if self.peek() == Some(&Tok::Op("^")) {
            self.pos += 1;
            let e = self.nested(Self::power)?;
            return Ok(Expr::Bin("^", Box::new(base), Box::new(e)));
        }
        Ok(base)
    }

    fn unary(&mut self) -> Result<Expr, String> {
        match self.peek() {
            Some(Tok::Op("-")) => {
                self.pos += 1;
                Ok(Expr::Neg(Box::new(self.nested(Self::unary)?)))
            }
            Some(Tok::Op("+")) => {
                self.pos += 1;
                self.nested(Self::unary)
            }
            _ => {
                let e = self.atom()?;
                if self.peek() == Some(&Tok::Op("%")) {
                    self.pos += 1;
                    return Ok(Expr::Bin("/", Box::new(e), Box::new(Expr::Lit(Val::Num(100.0)))));
                }
                Ok(e)
            }
        }
    }

    fn atom(&mut self) -> Result<Expr, String> {
        match self.bump() {
            Some(Tok::Num(n)) => Ok(Expr::Lit(Val::Num(n))),
            Some(Tok::Str(s)) => Ok(Expr::Lit(Val::Text(s))),
            Some(Tok::Col(c)) => self.table.col(c.trim()).map(Expr::Col).ok_or_else(|| format!("no column [{c}]")),
            Some(Tok::LParen) => {
                let e = self.nested(Self::whole)?;
                if self.bump() != Some(Tok::RParen) {
                    return Err("missing )".into());
                }
                Ok(e)
            }
            Some(Tok::Ident(id)) => {
                if self.peek() != Some(&Tok::LParen) {
                    return match id.as_str() {
                        "TRUE" => Ok(Expr::Lit(Val::Bool(true))),
                        "FALSE" => Ok(Expr::Lit(Val::Bool(false))),
                        "NULL" => Ok(Expr::Lit(Val::Null)),
                        _ => Err(format!("unknown name {id}")),
                    };
                }
                self.pos += 1;
                let mut args = Vec::new();
                if self.peek() == Some(&Tok::RParen) {
                    self.pos += 1;
                } else {
                    loop {
                        args.push(self.nested(Self::whole)?);
                        match self.bump() {
                            Some(Tok::Comma) => {}
                            Some(Tok::RParen) => break,
                            _ => return Err("missing )".into()),
                        }
                    }
                }
                if !FUNCTIONS.contains(&id.as_str()) {
                    return Err(format!("unknown function {id}"));
                }
                Ok(Expr::Call(id, args))
            }
            Some(t) => Err(format!("unexpected {t:?}")),
            None => Err("the formula ends too soon".into()),
        }
    }
}

/// Functions a formula may call.
pub const FUNCTIONS: [&str; 36] = [
    "IF", "AND", "OR", "NOT", "ISBLANK", "COALESCE", "EXACT", "UPPER", "LOWER", "PROPER", "TRIM", "TRIMENDS", "LEN", "LEFT", "RIGHT", "MID", "CONCAT", "SUBSTITUTE", "FIND", "CONTAINS", "TEXT", "VALUE", "ROUND", "ROUNDUP", "ROUNDDOWN", "ABS", "INT", "MOD", "SQRT", "POWER",
    "MIN", "MAX", "YEAR", "MONTH", "DAY", "DATE",
];

/// Parses `src` against the columns of `table`.
pub fn parse(src: &str, table: &DataTable) -> Result<Expr, String> {
    let src = src.trim().strip_prefix('=').unwrap_or(src.trim());
    let toks = lex(src)?;
    // Longer formulas make trees too deep to walk.
    if toks.len() > 8192 {
        return Err("the formula is too long".into());
    }
    let mut p = Parser { toks, pos: 0, table, depth: 0 };
    let e = p.binary(0)?;
    if p.pos < p.toks.len() {
        return Err(format!("unexpected {:?}", p.toks[p.pos]));
    }
    Ok(e)
}

fn num(v: &Val) -> Option<f64> {
    if v.is_null() { Some(0.0) } else { v.as_num() }
}

fn truthy(v: &Val) -> bool {
    match v {
        Val::Bool(b) => *b,
        Val::Num(n) | Val::Date(n) => *n != 0.0,
        Val::Text(t) => t.eq_ignore_ascii_case("true"),
        Val::Null => false,
    }
}

fn round_to(x: f64, d: f64, f: fn(f64) -> f64) -> f64 {
    let m = 10f64.powi(d as i32);
    f(x * m) / m
}

/// Bytes a text a formula or a step makes may have (more when what it
/// starts from already has more).
const MAX_TEXT: usize = 1 << 20;

/// `s` with `find` replaced by `rep`, unless that makes it too long.
pub(crate) fn replace_capped(s: &str, find: &str, rep: &str) -> Option<String> {
    if find.is_empty() {
        return Some(s.to_string());
    }
    let n = s.matches(find).count();
    let len = (s.len() - n * find.len()).saturating_add(n.saturating_mul(rep.len()));
    (len <= MAX_TEXT.max(s.len())).then(|| s.replace(find, rep))
}

/// `parts` joined, unless that makes a text too long (null then).
fn joined(parts: &[String]) -> Val {
    let len = parts.iter().fold(0usize, |a, p| a.saturating_add(p.len()));
    if len > parts.iter().map(String::len).max().unwrap_or(0).max(MAX_TEXT) {
        return Val::Null;
    }
    Val::Text(parts.concat())
}

/// The value of `e` on `row`; errors become nulls (as a bad value in one
/// row shouldn't stop the query).
pub fn eval(e: &Expr, row: &[Val]) -> Val {
    match e {
        Expr::Lit(v) => v.clone(),
        Expr::Col(i) => row.get(*i).cloned().unwrap_or(Val::Null),
        Expr::Neg(x) => num(&eval(x, row)).map_or(Val::Null, |n| Val::Num(-n)),
        Expr::Bin(op, a, b) => {
            let (a, b) = (eval(a, row), eval(b, row));
            match *op {
                "&" => joined(&[a.to_text(), b.to_text()]),
                "=" | "<>" | "<" | ">" | "<=" | ">=" => {
                    let o = if matches!((&a, &b), (Val::Text(_), Val::Text(_))) { a.to_text().to_lowercase().cmp(&b.to_text().to_lowercase()) } else { a.compare(&b) };
                    Val::Bool(match *op {
                        "=" => o.is_eq(),
                        "<>" => o.is_ne(),
                        "<" => o.is_lt(),
                        ">" => o.is_gt(),
                        "<=" => o.is_le(),
                        _ => o.is_ge(),
                    })
                }
                _ => {
                    let (Some(x), Some(y)) = (num(&a), num(&b)) else { return Val::Null };
                    let r = match *op {
                        "+" => x + y,
                        "-" => x - y,
                        "*" => x * y,
                        "/" if y == 0.0 => return Val::Null,
                        "/" => x / y,
                        _ => x.powf(y),
                    };
                    // A date plus days stays a date.
                    if matches!(a, Val::Date(_)) && matches!(*op, "+" | "-") && !matches!(b, Val::Date(_)) {
                        return Val::Date(r);
                    }
                    if r.is_finite() { Val::Num(r) } else { Val::Null }
                }
            }
        }
        Expr::Call(f, args) => call(f, args, row),
    }
}

fn call(f: &str, args: &[Expr], row: &[Val]) -> Val {
    let arg = |i: usize| args.get(i).map_or(Val::Null, |e| eval(e, row));
    let text = |i: usize| arg(i).to_text();
    let n = |i: usize| num(&arg(i));
    let chars = |s: &str| s.chars().collect::<Vec<_>>();
    match f {
        "IF" => {
            if truthy(&arg(0)) { arg(1) } else { arg(2) }
        }
        "AND" => Val::Bool((0..args.len()).all(|i| truthy(&arg(i)))),
        "OR" => Val::Bool((0..args.len()).any(|i| truthy(&arg(i)))),
        "NOT" => Val::Bool(!truthy(&arg(0))),
        "ISBLANK" => Val::Bool(arg(0).is_null()),
        "COALESCE" => (0..args.len()).map(arg).find(|v| !v.is_null()).unwrap_or(Val::Null),
        "UPPER" => Val::Text(text(0).to_uppercase()),
        "LOWER" => Val::Text(text(0).to_lowercase()),
        "PROPER" => {
            let mut out = String::new();
            let mut start = true;
            for c in text(0).chars() {
                if start { out.extend(c.to_uppercase()) } else { out.extend(c.to_lowercase()) }
                start = !c.is_alphanumeric();
            }
            Val::Text(out)
        }
        "TRIM" => Val::Text(text(0).split_whitespace().collect::<Vec<_>>().join(" ")),
        // Only the spaces at the ends (TRIM also joins those inside).
        "TRIMENDS" => Val::Text(text(0).trim().to_string()),
        // Equal, telling capitals from small letters (`=` doesn't).
        "EXACT" => {
            let (a, b) = (arg(0), arg(1));
            Val::Bool(match (&a, &b) {
                (Val::Text(x), Val::Text(y)) => x == y,
                _ => a.compare(&b).is_eq(),
            })
        }
        "LEN" => Val::Num(text(0).chars().count() as f64),
        "LEFT" | "RIGHT" => {
            let s = chars(&text(0));
            let k = if args.len() > 1 { n(1).unwrap_or(0.0).max(0.0) as usize } else { 1 }.min(s.len());
            Val::Text(if f == "LEFT" { s[..k].iter().collect() } else { s[s.len() - k..].iter().collect() })
        }
        "MID" => {
            let s = chars(&text(0));
            let a = (n(1).unwrap_or(1.0).max(1.0) as usize - 1).min(s.len());
            let k = n(2).unwrap_or(0.0).max(0.0) as usize;
            Val::Text(s[a..a.saturating_add(k).min(s.len())].iter().collect())
        }
        "CONCAT" => joined(&(0..args.len()).map(text).collect::<Vec<_>>()),
        "SUBSTITUTE" => replace_capped(&text(0), &text(1), &text(2)).map_or(Val::Null, Val::Text),
        "FIND" => {
            let (h, nd) = (text(1), text(0));
            h.find(&nd).map_or(Val::Null, |i| Val::Num(h[..i].chars().count() as f64 + 1.0))
        }
        "CONTAINS" => Val::Bool(text(0).to_lowercase().contains(&text(1).to_lowercase())),
        "TEXT" => {
            let v = arg(0);
            match (&v, args.len() > 1) {
                (Val::Num(x), true) => {
                    let d = n(1).unwrap_or(0.0).clamp(0.0, 15.0) as usize;
                    Val::Text(format!("{x:.d$}"))
                }
                _ => Val::Text(v.to_text()),
            }
        }
        "VALUE" => arg(0).as_num().map_or(Val::Null, Val::Num),
        "ROUND" | "ROUNDUP" | "ROUNDDOWN" => {
            let Some(x) = n(0) else { return Val::Null };
            let d = n(1).unwrap_or(0.0);
            let f: fn(f64) -> f64 = match f {
                "ROUND" => f64::round,
                "ROUNDUP" => |v: f64| if v < 0.0 { v.floor() } else { v.ceil() },
                _ => f64::trunc,
            };
            Val::Num(round_to(x, d, f))
        }
        "ABS" => n(0).map_or(Val::Null, |x| Val::Num(x.abs())),
        "INT" => n(0).map_or(Val::Null, |x| Val::Num(x.floor())),
        "MOD" => match (n(0), n(1)) {
            (Some(x), Some(y)) if y != 0.0 => Val::Num(x - y * (x / y).floor()),
            _ => Val::Null,
        },
        "SQRT" => n(0).filter(|x| *x >= 0.0).map_or(Val::Null, |x| Val::Num(x.sqrt())),
        "POWER" => match (n(0), n(1)) {
            (Some(x), Some(y)) => Val::Num(x.powf(y)),
            _ => Val::Null,
        },
        "MIN" | "MAX" => {
            let vals: Vec<f64> = (0..args.len()).filter_map(|i| arg(i).as_num()).collect();
            let r = if f == "MIN" { vals.iter().copied().fold(f64::INFINITY, f64::min) } else { vals.iter().copied().fold(f64::NEG_INFINITY, f64::max) };
            if r.is_finite() { Val::Num(r) } else { Val::Null }
        }
        "YEAR" | "MONTH" | "DAY" => {
            let d = match arg(0) {
                Val::Date(d) | Val::Num(d) => d,
                Val::Text(t) => match crate::table::parse_date(&t) {
                    Some(d) => d,
                    None => return Val::Null,
                },
                _ => return Val::Null,
            };
            let (y, m, dd) = apex_sheet::format::serial_to_date(d);
            Val::Num(match f {
                "YEAR" => y as f64,
                "MONTH" => m as f64,
                _ => dd as f64,
            })
        }
        "DATE" => match (n(0), n(1), n(2)) {
            (Some(y), Some(m), Some(d)) => Val::Date(apex_sheet::format::date_to_serial(y as i64, m as i64, d as i64)),
            _ => Val::Null,
        },
        _ => Val::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t() -> DataTable {
        let mut t = DataTable::new(vec!["Price".into(), "Qty".into(), "Name".into(), "Day".into()]);
        t.rows.push(vec![Val::Num(2.5), Val::Num(4.0), Val::text("ana maría"), Val::Date(45292.0)]);
        t
    }

    fn run(src: &str) -> Val {
        let t = t();
        eval(&parse(src, &t).unwrap(), &t.rows[0])
    }

    #[test]
    fn formulas() {
        assert_eq!(run("[Price] * [Qty] + 1"), Val::Num(11.0));
        assert_eq!(run("-2 ^ 2"), Val::Num(4.0));
        assert_eq!(run("2 * 3 ^ 2"), Val::Num(18.0));
        assert_eq!(run("=[Name] & \" (\" & [Qty] & \")\""), Val::text("ana maría (4)"));
        assert_eq!(run("IF([Qty] >= 4, \"big\", \"small\")"), Val::text("big"));
        assert_eq!(run("PROPER([Name])"), Val::text("Ana María"));
        assert_eq!(run("upper(left([name], 3))"), Val::text("ANA"));
        assert_eq!(run("EXACT([Name], \"ana maría\")"), Val::Bool(true));
        assert_eq!(run("EXACT([Name], \"Ana María\")"), Val::Bool(false));
        assert_eq!(run("[Name] = \"Ana María\""), Val::Bool(true));
        assert_eq!(run("EXACT([Qty], 4)"), Val::Bool(true));
        assert_eq!(run("TRIMENDS(\"  a  b \")"), Val::text("a  b"));
        assert_eq!(run("MID([Name], 5, 3)"), Val::text("mar"));
        assert_eq!(run("YEAR([Day]) * 100 + MONTH([Day])"), Val::Num(202401.0));
        assert_eq!(run("[Day] + 1"), Val::Date(45293.0));
        assert_eq!(run("ROUND([Price] / 3, 2)"), Val::Num(0.83));
        assert_eq!(run("[Qty] / 0"), Val::Null);
        assert_eq!(run("50%"), Val::Num(0.5));
        assert!(parse("[Nope] + 1", &t()).is_err());
        assert!(parse("FOO(1)", &t()).is_err());
        assert!(parse("(1 + 2", &t()).is_err());
        // Deep nesting is refused, not a stack overflow.
        assert!(parse(&format!("{}1{}", "(".repeat(100_000), ")".repeat(100_000)), &t()).is_err());
        assert!(parse(&format!("{}1", "-".repeat(5000)), &t()).is_err());
        assert!(parse(&"2^".repeat(3000).to_string(), &t()).is_err());
    }
}
