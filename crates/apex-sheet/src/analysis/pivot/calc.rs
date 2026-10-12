//! Formulas of calculated fields (`=Sales/Units`): arithmetic, comparisons
//! and a few functions over field names. Like Excel, a calculated field is
//! evaluated on the sums of the fields it names in each cell of the table.

/// A parsed formula. Fields are source columns.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Num(f64),
    Field(u32),
    Neg(Box<Expr>),
    Percent(Box<Expr>),
    Bin(Op, Box<Expr>, Box<Expr>),
    Call(Func, Vec<Expr>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Func {
    If,
    Abs,
    Round,
    Int,
    Sqrt,
    Min,
    Max,
    Sum,
    Average,
    And,
    Or,
    Not,
    Mod,
    Power,
}

/// Function names, English and Spanish.
const FUNCS: [(&str, &str, Func); 14] = [
    ("IF", "SI", Func::If),
    ("ABS", "ABS", Func::Abs),
    ("ROUND", "REDONDEAR", Func::Round),
    ("INT", "ENTERO", Func::Int),
    ("SQRT", "RAIZ", Func::Sqrt),
    ("MIN", "MIN", Func::Min),
    ("MAX", "MAX", Func::Max),
    ("SUM", "SUMA", Func::Sum),
    ("AVERAGE", "PROMEDIO", Func::Average),
    ("AND", "Y", Func::And),
    ("OR", "O", Func::Or),
    ("NOT", "NO", Func::Not),
    ("MOD", "RESIDUO", Func::Mod),
    ("POWER", "POTENCIA", Func::Power),
];

struct Parser<'a> {
    s: Vec<char>,
    i: usize,
    names: &'a [String],
    /// Decimal comma; arguments are then separated by `;`.
    comma: bool,
    /// How many signs and brackets deep the parser is.
    depth: u32,
}

fn ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '.'
}

impl Parser<'_> {
    fn skip(&mut self) {
        while self.s.get(self.i).is_some_and(|c| c.is_whitespace()) {
            self.i += 1;
        }
    }

    fn peek(&mut self) -> Option<char> {
        self.skip();
        self.s.get(self.i).copied()
    }

    fn eat(&mut self, t: &str) -> bool {
        self.skip();
        let t: Vec<char> = t.chars().collect();
        if self.s[self.i..].starts_with(&t) {
            self.i += t.len();
            true
        } else {
            false
        }
    }

    fn err(&self) -> String {
        let rest: String = self.s[self.i.min(self.s.len())..].iter().collect();
        if rest.is_empty() { "…".to_string() } else { rest.chars().take(20).collect() }
    }

    fn compare(&mut self) -> Result<Expr, String> {
        let mut a = self.additive()?;
        loop {
            let op = if self.eat("<>") {
                Op::Ne
            } else if self.eat("<=") {
                Op::Le
            } else if self.eat(">=") {
                Op::Ge
            } else if self.eat("=") {
                Op::Eq
            } else if self.eat("<") {
                Op::Lt
            } else if self.eat(">") {
                Op::Gt
            } else {
                return Ok(a);
            };
            let b = self.additive()?;
            a = Expr::Bin(op, Box::new(a), Box::new(b));
        }
    }

    fn additive(&mut self) -> Result<Expr, String> {
        let mut a = self.term()?;
        loop {
            let op = if self.eat("+") {
                Op::Add
            } else if self.eat("-") {
                Op::Sub
            } else {
                return Ok(a);
            };
            let b = self.term()?;
            a = Expr::Bin(op, Box::new(a), Box::new(b));
        }
    }

    fn term(&mut self) -> Result<Expr, String> {
        let mut a = self.power()?;
        loop {
            let op = if self.eat("*") {
                Op::Mul
            } else if self.eat("/") {
                Op::Div
            } else {
                return Ok(a);
            };
            let b = self.power()?;
            a = Expr::Bin(op, Box::new(a), Box::new(b));
        }
    }

    fn power(&mut self) -> Result<Expr, String> {
        let mut a = self.unary()?;
        while self.eat("^") {
            let b = self.unary()?;
            a = Expr::Bin(Op::Pow, Box::new(a), Box::new(b));
        }
        Ok(a)
    }

    /// A signed operand; nesting past 200 is refused, as it would
    /// overflow the stack.
    fn unary(&mut self) -> Result<Expr, String> {
        if self.depth >= 200 {
            return Err(self.err());
        }
        self.depth += 1;
        let r = self.signed();
        self.depth -= 1;
        r
    }

    fn signed(&mut self) -> Result<Expr, String> {
        if self.eat("-") {
            return Ok(Expr::Neg(Box::new(self.unary()?)));
        }
        if self.eat("+") {
            return self.unary();
        }
        let mut e = self.atom()?;
        while self.eat("%") {
            e = Expr::Percent(Box::new(e));
        }
        Ok(e)
    }

    fn atom(&mut self) -> Result<Expr, String> {
        let Some(c) = self.peek() else { return Err(self.err()) };
        if c == '(' {
            self.i += 1;
            let e = self.compare()?;
            return if self.eat(")") { Ok(e) } else { Err(self.err()) };
        }
        let dec = if self.comma { ',' } else { '.' };
        if c.is_ascii_digit() || c == dec {
            let start = self.i;
            while self.s.get(self.i).is_some_and(|c| c.is_ascii_digit() || *c == dec) {
                self.i += 1;
            }
            // Exponent (1E3, 2.5e-2).
            if self.s.get(self.i).is_some_and(|c| *c == 'e' || *c == 'E') {
                let j = self.i + 1 + usize::from(self.s.get(self.i + 1).is_some_and(|c| *c == '-' || *c == '+'));
                if self.s.get(j).is_some_and(|c| c.is_ascii_digit()) {
                    self.i = j;
                    while self.s.get(self.i).is_some_and(|c| c.is_ascii_digit()) {
                        self.i += 1;
                    }
                }
            }
            let t: String = self.s[start..self.i].iter().map(|c| if *c == dec { '.' } else { *c }).collect();
            return t.parse().map(Expr::Num).map_err(|_| t);
        }
        if c == '\'' {
            // 'Field name' ('' is a quote).
            self.i += 1;
            let mut name = String::new();
            loop {
                match self.s.get(self.i) {
                    None => return Err(self.err()),
                    Some('\'') if self.s.get(self.i + 1) == Some(&'\'') => {
                        name.push('\'');
                        self.i += 2;
                    }
                    Some('\'') => {
                        self.i += 1;
                        break;
                    }
                    Some(c) => {
                        name.push(*c);
                        self.i += 1;
                    }
                }
            }
            return self.field(&name).ok_or(name);
        }
        if !ident_char(c) {
            return Err(self.err());
        }
        // The longest field name the text starts with, spaces included.
        let rest: String = self.s[self.i..].iter().collect();
        let lower = rest.to_lowercase();
        let mut best: Option<(usize, u32)> = None;
        for (k, n) in self.names.iter().enumerate() {
            let nl = n.to_lowercase();
            if nl.is_empty() || !lower.starts_with(&nl) {
                continue;
            }
            let len = n.chars().count();
            let next = rest.chars().nth(len);
            if next.is_some_and(ident_char) || next == Some('(') {
                continue;
            }
            if best.is_none_or(|b| len > b.0) {
                best = Some((len, k as u32));
            }
        }
        if let Some((len, col)) = best {
            self.i += len;
            return Ok(Expr::Field(col));
        }
        let start = self.i;
        while self.s.get(self.i).is_some_and(|c| ident_char(*c)) {
            self.i += 1;
        }
        let word: String = self.s[start..self.i].iter().collect::<String>().to_uppercase();
        if self.eat("(") {
            let Some(f) = FUNCS.iter().find(|(en, es, _)| *en == word || *es == word).map(|x| x.2) else { return Err(word) };
            let mut args = Vec::new();
            if !self.eat(")") {
                loop {
                    args.push(self.compare()?);
                    if self.eat(")") {
                        break;
                    }
                    let sep = if self.comma { ";" } else { "," };
                    if !self.eat(sep) && !self.eat(";") {
                        return Err(self.err());
                    }
                }
            }
            return Ok(Expr::Call(f, args));
        }
        match word.as_str() {
            "TRUE" | "VERDADERO" => Ok(Expr::Num(1.0)),
            "FALSE" | "FALSO" => Ok(Expr::Num(0.0)),
            _ => Err(word),
        }
    }

    fn field(&self, name: &str) -> Option<Expr> {
        let n = name.trim().to_lowercase();
        self.names.iter().position(|x| x.to_lowercase() == n).map(|k| Expr::Field(k as u32))
    }
}

/// Parses a formula (with or without `=`) whose names are `names` (the
/// source columns). The error is the text that could not be read.
pub fn parse(text: &str, names: &[String], decimal_comma: bool) -> Result<Expr, String> {
    let t = text.trim();
    let t = t.strip_prefix('=').unwrap_or(t);
    // Longer formulas than Excel takes make trees too deep to walk.
    if t.chars().count() > 8192 {
        return Err(t.chars().take(20).collect());
    }
    let mut p = Parser { s: t.chars().collect(), i: 0, names, comma: decimal_comma, depth: 0 };
    let e = p.compare()?;
    if p.peek().is_some() {
        return Err(p.err());
    }
    Ok(e)
}

impl Expr {
    /// The value with each field replaced by `field(col)`. Errors (division
    /// by zero, square root of a negative) come out as NaN.
    pub fn eval(&self, field: &dyn Fn(u32) -> f64) -> f64 {
        let b = |x: bool| if x { 1.0 } else { 0.0 };
        match self {
            Expr::Num(n) => *n,
            Expr::Field(c) => field(*c),
            Expr::Neg(e) => -e.eval(field),
            Expr::Percent(e) => e.eval(field) / 100.0,
            Expr::Bin(op, x, y) => {
                let (x, y) = (x.eval(field), y.eval(field));
                match op {
                    Op::Add => x + y,
                    Op::Sub => x - y,
                    Op::Mul => x * y,
                    Op::Div if y == 0.0 => f64::NAN,
                    Op::Div => x / y,
                    Op::Pow => x.powf(y),
                    Op::Eq => b(x == y),
                    Op::Ne => b(x != y),
                    Op::Lt => b(x < y),
                    Op::Le => b(x <= y),
                    Op::Gt => b(x > y),
                    Op::Ge => b(x >= y),
                }
            }
            Expr::Call(f, args) => {
                let v: Vec<f64> = match f {
                    // IF evaluates only the branch it takes.
                    Func::If => {
                        let c = args.first().map_or(f64::NAN, |a| a.eval(field));
                        if c.is_nan() {
                            return c;
                        }
                        let k = if c != 0.0 { 1 } else { 2 };
                        return args.get(k).map_or(if k == 1 { 1.0 } else { 0.0 }, |a| a.eval(field));
                    }
                    _ => args.iter().map(|a| a.eval(field)).collect(),
                };
                let a = |i: usize| v.get(i).copied().unwrap_or(f64::NAN);
                match f {
                    Func::If => unreachable!(),
                    Func::Abs => a(0).abs(),
                    Func::Round => {
                        let m = 10f64.powi(v.get(1).copied().unwrap_or(0.0) as i32);
                        (a(0) * m).round() / m
                    }
                    Func::Int => a(0).floor(),
                    Func::Sqrt => a(0).sqrt(),
                    Func::Min => v.iter().copied().fold(f64::INFINITY, f64::min),
                    Func::Max => v.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                    Func::Sum => v.iter().sum(),
                    Func::Average if v.is_empty() => f64::NAN,
                    Func::Average => v.iter().sum::<f64>() / v.len() as f64,
                    Func::And => b(v.iter().all(|x| *x != 0.0)),
                    Func::Or => b(v.iter().any(|x| *x != 0.0)),
                    Func::Not => b(a(0) == 0.0),
                    Func::Mod if a(1) == 0.0 => f64::NAN,
                    Func::Mod => a(0) - a(1) * (a(0) / a(1)).floor(),
                    Func::Power => a(0).powf(a(1)),
                }
            }
        }
    }

    /// The fields the formula names, in order of first use.
    pub fn fields(&self, out: &mut Vec<u32>) {
        match self {
            Expr::Num(_) => {}
            Expr::Field(c) => {
                if !out.contains(c) {
                    out.push(*c);
                }
            }
            Expr::Neg(e) | Expr::Percent(e) => e.fields(out),
            Expr::Bin(_, a, b) => {
                a.fields(out);
                b.fields(out);
            }
            Expr::Call(_, args) => args.iter().for_each(|a| a.fields(out)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_formulas_are_refused() {
        let deep = format!("{}1{}", "(".repeat(5000), ")".repeat(5000));
        assert!(parse(&deep, &[], false).is_err());
        assert!(parse(&"-".repeat(5000), &[], false).is_err());
        assert!(parse(&format!("{}1{}", "(".repeat(50), ")".repeat(50)), &[], false).is_ok());
    }

    #[test]
    fn formulas() {
        let names: Vec<String> = ["Sales", "Units", "Unit price", "Sales tax"].iter().map(|s| s.to_string()).collect();
        let vals = [100.0, 4.0, 2.5, 7.0];
        let f = |c: u32| vals[c as usize];
        let ev = |t: &str| parse(t, &names, false).map(|e| e.eval(&f));
        assert_eq!(ev("=Sales/Units"), Ok(25.0));
        assert_eq!(ev("Sales - sales tax"), Ok(93.0));
        assert_eq!(ev("'Unit price' * 2"), Ok(5.0));
        assert_eq!(ev("unit price*units"), Ok(10.0));
        assert_eq!(ev("IF(Sales>50, 1, 2) + 10%"), Ok(1.1));
        assert_eq!(ev("-2^2"), Ok(4.0));
        assert_eq!(ev("MAX(Units, 3, 9) + ROUND(2.567, 1)"), Ok(9.0 + 2.6));
        assert!(ev("Sales/0").unwrap().is_nan());
        assert_eq!(ev("Profit*2"), Err("PROFIT".to_string()));
        assert!(ev("Sales +").is_err());
        let e = parse("=SI(Ventas>0;Ventas*0,5;0)", &["Ventas".to_string()], true).unwrap();
        assert_eq!(e.eval(&|_| 10.0), 5.0);
        let mut used = Vec::new();
        parse("Units*Sales+Units", &names, false).unwrap().fields(&mut used);
        assert_eq!(used, vec![1, 0]);
    }
}
