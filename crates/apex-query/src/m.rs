//! Power Query's M formulas (the queries of Excel workbooks) translated
//! into Get Data queries: the sources and steps Excel's query editor
//! writes ("Csv.Document", "Table.SelectRows", "Table.Group"…). M is a
//! whole language; what has no step of ours stops the translation there
//! and is named, so the user knows what is missing.

use std::collections::HashMap;

use apex_sheet::query::{AggFn, Aggregate, ColType, DbKind, FilterOp, JoinKind, Query, Source, Step};

/// A parsed M expression.
#[derive(Clone, Debug, PartialEq)]
enum M {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    /// A name: a step, a function (`Table.SelectRows`), an enum value
    /// (`Order.Ascending`), `_`, `#date`…
    Id(String),
    List(Vec<M>),
    Record(Vec<(String, M)>),
    Call(Box<M>, Vec<M>),
    /// `x[name]`; `[name]` alone is a field of `_`.
    Field(Box<M>, String),
    /// `x{i}` or `x{[Item="…"]}`.
    Item(Box<M>, Box<M>),
    Each(Box<M>),
    Func(Vec<String>, Box<M>),
    Bin(&'static str, Box<M>, Box<M>),
    Not(Box<M>),
    Neg(Box<M>),
    If(Box<M>, Box<M>, Box<M>),
    Let(Vec<(String, M)>, Box<M>),
    /// `type text`, `Int64.Type`…: the type's name.
    Type(String),
}

struct Parser {
    s: Vec<char>,
    i: usize,
    /// How many brackets and signs deep the parser is.
    depth: u32,
    /// Operations, calls and fields chained so far.
    links: u32,
}

type PResult<T> = Result<T, String>;

const KEYWORDS: [&str; 12] = ["let", "in", "each", "if", "then", "else", "and", "or", "not", "type", "as", "is"];

impl Parser {
    fn new(src: &str) -> Parser {
        Parser { s: src.chars().collect(), i: 0, depth: 0, links: 0 }
    }

    /// `f` one level deeper; past 100 the query is refused, as it would
    /// overflow the stack.
    fn nested(&mut self, f: fn(&mut Self) -> PResult<M>) -> PResult<M> {
        if self.depth >= 100 {
            return Err("the query is nested too deeply".into());
        }
        self.depth += 1;
        let r = f(self);
        self.depth -= 1;
        r
    }

    /// Counts one more operation in a chain: a chain of tens of thousands
    /// makes a tree too deep to walk.
    fn link(&mut self) -> PResult<()> {
        self.links += 1;
        if self.links > 20_000 { Err("the query is too long".into()) } else { Ok(()) }
    }

    fn ws(&mut self) {
        loop {
            while self.i < self.s.len() && self.s[self.i].is_whitespace() {
                self.i += 1;
            }
            if self.at("//") {
                while self.i < self.s.len() && self.s[self.i] != '\n' {
                    self.i += 1;
                }
            } else if self.at("/*") {
                self.i += 2;
                while self.i < self.s.len() && !self.at("*/") {
                    self.i += 1;
                }
                self.i = (self.i + 2).min(self.s.len());
            } else {
                return;
            }
        }
    }

    fn at(&self, t: &str) -> bool {
        t.chars().enumerate().all(|(k, c)| self.s.get(self.i + k) == Some(&c))
    }

    /// Takes punctuation `t` after spaces.
    fn eat(&mut self, t: &str) -> bool {
        self.ws();
        if self.at(t) {
            self.i += t.chars().count();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, t: &str) -> PResult<()> {
        if self.eat(t) { Ok(()) } else { Err(format!("expected {t}")) }
    }

    /// Takes keyword `k` (a whole word).
    fn keyword(&mut self, k: &str) -> bool {
        self.ws();
        let save = self.i;
        if self.at(k) && !self.s.get(self.i + k.len()).is_some_and(|c| c.is_alphanumeric() || *c == '_' || *c == '.') {
            self.i += k.len();
            return true;
        }
        self.i = save;
        false
    }

    fn string(&mut self) -> PResult<String> {
        // After the opening quote.
        let mut out = String::new();
        loop {
            match self.s.get(self.i) {
                None => return Err("missing \"".into()),
                Some('"') if self.s.get(self.i + 1) == Some(&'"') => {
                    out.push('"');
                    self.i += 2;
                }
                Some('"') => {
                    self.i += 1;
                    return Ok(unescape(&out));
                }
                Some(&c) => {
                    out.push(c);
                    self.i += 1;
                }
            }
        }
    }

    /// A name: `Source`, `Table.SelectRows`, `#"Changed Type"`, `#date`.
    fn name(&mut self) -> Option<String> {
        self.ws();
        if self.at("#\"") {
            self.i += 2;
            return self.string().ok();
        }
        let start = self.i;
        if self.at("#") {
            self.i += 1;
        }
        while self.i < self.s.len() && (self.s[self.i].is_alphanumeric() || self.s[self.i] == '_' || (self.s[self.i] == '.' && self.i > start)) {
            self.i += 1;
        }
        let n: String = self.s[start..self.i].iter().collect();
        if n.is_empty() || n == "#" || KEYWORDS.contains(&n.as_str()) {
            self.i = start;
            return None;
        }
        Some(n.trim_end_matches('.').to_string())
    }

    /// The name inside `[…]` up to `]` or `=` (field names may have spaces).
    fn field_name(&mut self) -> PResult<String> {
        self.ws();
        if self.at("#\"") {
            self.i += 2;
            return self.string();
        }
        if self.at("\"") {
            self.i += 1;
            return self.string();
        }
        let start = self.i;
        while self.i < self.s.len() && !matches!(self.s[self.i], ']' | '=' | ',') {
            self.i += 1;
        }
        Ok(self.s[start..self.i].iter().collect::<String>().trim().to_string())
    }

    fn expr(&mut self) -> PResult<M> {
        self.nested(Self::expression)
    }

    fn expression(&mut self) -> PResult<M> {
        if self.keyword("let") {
            let mut binds = Vec::new();
            loop {
                let n = self.name().ok_or("expected a name")?;
                self.expect("=")?;
                binds.push((n, self.expr()?));
                if !self.eat(",") {
                    break;
                }
            }
            if !self.keyword("in") {
                return Err("expected in".into());
            }
            return Ok(M::Let(binds, Box::new(self.expr()?)));
        }
        if self.keyword("each") {
            return Ok(M::Each(Box::new(self.expr()?)));
        }
        if self.keyword("if") {
            let c = self.expr()?;
            if !self.keyword("then") {
                return Err("expected then".into());
            }
            let t = self.expr()?;
            if !self.keyword("else") {
                return Err("expected else".into());
            }
            return Ok(M::If(Box::new(c), Box::new(t), Box::new(self.expr()?)));
        }
        if let Some(f) = self.function()? {
            return Ok(f);
        }
        self.or()
    }

    /// `(a, b as text) => …`, or nothing (and nothing taken).
    fn function(&mut self) -> PResult<Option<M>> {
        let save = self.i;
        if !self.eat("(") {
            return Ok(None);
        }
        let mut params = Vec::new();
        if !self.eat(")") {
            loop {
                self.keyword("optional");
                let Some(p) = self.name() else {
                    self.i = save;
                    return Ok(None);
                };
                params.push(p);
                if self.keyword("as") {
                    self.type_name()?;
                }
                if self.eat(")") {
                    break;
                }
                if !self.eat(",") {
                    self.i = save;
                    return Ok(None);
                }
            }
        }
        if self.keyword("as") {
            self.type_name()?;
        }
        if !self.eat("=>") {
            self.i = save;
            return Ok(None);
        }
        Ok(Some(M::Func(params, Box::new(self.expr()?))))
    }

    fn or(&mut self) -> PResult<M> {
        let mut l = self.and()?;
        while self.keyword("or") {
            self.link()?;
            l = M::Bin("or", Box::new(l), Box::new(self.and()?));
        }
        Ok(l)
    }

    fn and(&mut self) -> PResult<M> {
        let mut l = self.compare()?;
        while self.keyword("and") {
            self.link()?;
            l = M::Bin("and", Box::new(l), Box::new(self.compare()?));
        }
        Ok(l)
    }

    fn compare(&mut self) -> PResult<M> {
        let mut l = self.additive()?;
        loop {
            self.ws();
            let op = ["<>", "<=", ">=", "=", "<", ">", "??"].into_iter().find(|o| self.at(o) && !(*o == "=" && self.at("=>")));
            let Some(op) = op else { break };
            self.i += op.len();
            self.link()?;
            l = M::Bin(op, Box::new(l), Box::new(self.additive()?));
        }
        // `x as T` is x; `x is T` is a test we have no formula for.
        if self.keyword("as") {
            self.type_name()?;
        } else if self.keyword("is") {
            self.link()?;
            l = M::Bin("is", Box::new(l), Box::new(M::Type(self.type_name()?)));
        }
        Ok(l)
    }

    fn additive(&mut self) -> PResult<M> {
        let mut l = self.multiplicative()?;
        loop {
            self.ws();
            let op = ["+", "-", "&"].into_iter().find(|o| self.at(o));
            let Some(op) = op else { break };
            self.i += 1;
            self.link()?;
            l = M::Bin(op, Box::new(l), Box::new(self.multiplicative()?));
        }
        Ok(l)
    }

    fn multiplicative(&mut self) -> PResult<M> {
        let mut l = self.unary()?;
        loop {
            self.ws();
            let op = ["*", "/"].into_iter().find(|o| self.at(o) && !self.at("/*") && !self.at("//"));
            let Some(op) = op else { break };
            self.i += 1;
            self.link()?;
            l = M::Bin(op, Box::new(l), Box::new(self.unary()?));
        }
        Ok(l)
    }

    fn unary(&mut self) -> PResult<M> {
        if self.keyword("not") {
            return Ok(M::Not(Box::new(self.nested(Self::unary)?)));
        }
        if self.eat("-") {
            return Ok(M::Neg(Box::new(self.nested(Self::unary)?)));
        }
        if self.eat("+") {
            return self.nested(Self::unary);
        }
        if self.keyword("type") {
            return Ok(M::Type(self.type_name()?));
        }
        self.postfix()
    }

    /// A type after `type` or `as`: its name ("text", "nullable number" is
    /// "number", "table [...]" is "table").
    fn type_name(&mut self) -> PResult<String> {
        self.keyword("nullable");
        self.ws();
        if self.at("[") || self.at("{") {
            self.skip_group()?;
            return Ok("record".into());
        }
        let n = self.name().ok_or("expected a type")?;
        self.ws();
        if (n == "table" || n == "function") && (self.at("[") || self.at("(")) {
            self.skip_group()?;
        }
        Ok(n)
    }

    /// Skips a bracketed group (`[…]`, `{…}`, `(…)`) with what's inside.
    fn skip_group(&mut self) -> PResult<()> {
        let mut depth = 0;
        while let Some(&c) = self.s.get(self.i) {
            self.i += 1;
            match c {
                '"' => {
                    self.string()?;
                }
                '[' | '{' | '(' => depth += 1,
                ']' | '}' | ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                _ => {}
            }
        }
        Err("unclosed group".into())
    }

    fn postfix(&mut self) -> PResult<M> {
        let mut e = self.primary()?;
        loop {
            self.ws();
            if self.at("(") {
                self.i += 1;
                let args = self.items(")")?;
                self.link()?;
                e = M::Call(Box::new(e), args);
            } else if self.at("{") {
                self.i += 1;
                let k = self.expr()?;
                self.expect("}")?;
                e = M::Item(Box::new(e), Box::new(k));
            } else if self.at("[") && !self.record_ahead() {
                self.i += 1;
                let f = self.field_name()?;
                self.expect("]")?;
                self.link()?;
                e = M::Field(Box::new(e), f);
            } else {
                break;
            }
            // An optional access (`x[a]?`), not `??`.
            self.ws();
            if self.at("?") && !self.at("??") {
                self.i += 1;
            }
        }
        Ok(e)
    }

    /// At `[`: a record (`[a = 1]`) rather than a field access.
    fn record_ahead(&mut self) -> bool {
        let save = self.i;
        self.i += 1;
        let rec = self.field_name().is_ok() && {
            self.ws();
            true
        } && self.s.get(self.i) == Some(&'=') && self.s.get(self.i + 1) != Some(&'>');
        self.i = save;
        rec
    }

    /// Comma-separated expressions up to `close`.
    fn items(&mut self, close: &str) -> PResult<Vec<M>> {
        let mut out = Vec::new();
        if self.eat(close) {
            return Ok(out);
        }
        loop {
            out.push(self.expr()?);
            if self.eat(close) {
                return Ok(out);
            }
            self.expect(",")?;
        }
    }

    fn primary(&mut self) -> PResult<M> {
        self.ws();
        let Some(&c) = self.s.get(self.i) else { return Err("unexpected end".into()) };
        match c {
            '"' => {
                self.i += 1;
                Ok(M::Str(self.string()?))
            }
            '0'..='9' => {
                let start = self.i;
                while self.i < self.s.len() && (self.s[self.i].is_ascii_alphanumeric() || self.s[self.i] == '.' || ((self.s[self.i] == '-' || self.s[self.i] == '+') && matches!(self.s[self.i - 1], 'e' | 'E'))) {
                    self.i += 1;
                }
                let t: String = self.s[start..self.i].iter().collect();
                let n = if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) { i64::from_str_radix(h, 16).map(|v| v as f64).ok() } else { t.parse().ok() };
                n.map(M::Num).ok_or_else(|| format!("bad number {t}"))
            }
            '(' => {
                self.i += 1;
                let e = self.expr()?;
                self.expect(")")?;
                Ok(e)
            }
            '{' => {
                self.i += 1;
                Ok(M::List(self.items("}")?))
            }
            '[' => {
                if self.record_ahead() || self.at("[]") {
                    self.i += 1;
                    let mut fields = Vec::new();
                    if self.eat("]") {
                        return Ok(M::Record(fields));
                    }
                    loop {
                        let n = self.field_name()?;
                        self.expect("=")?;
                        fields.push((n, self.expr()?));
                        if self.eat("]") {
                            return Ok(M::Record(fields));
                        }
                        self.expect(",")?;
                    }
                }
                self.i += 1;
                let f = self.field_name()?;
                self.expect("]")?;
                Ok(M::Field(Box::new(M::Id("_".into())), f))
            }
            _ => {
                let n = self.name().ok_or_else(|| format!("unexpected {c}"))?;
                Ok(match n.as_str() {
                    "null" => M::Null,
                    "true" => M::Bool(true),
                    "false" => M::Bool(false),
                    _ if n.ends_with(".Type") => M::Type(n),
                    _ => M::Id(n),
                })
            }
        }
    }
}

/// M's escapes in text: `#(tab)`, `#(cr,lf)`, `#(00E9)`, `#(#)`.
fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("#(") {
        out.push_str(&rest[..i]);
        let Some(j) = rest[i..].find(')') else { break };
        for code in rest[i + 2..i + j].split(',') {
            match code.trim() {
                "tab" => out.push('\t'),
                "cr" => out.push('\r'),
                "lf" => out.push('\n'),
                "#" => out.push('#'),
                h => match u32::from_str_radix(h, 16).ok().and_then(char::from_u32) {
                    Some(c) => out.push(c),
                    None => out.push_str(&rest[i..=i + j]),
                },
            }
        }
        rest = &rest[i + j + 1..];
    }
    out.push_str(rest);
    out
}

fn parse(src: &str) -> PResult<M> {
    let mut p = Parser::new(src);
    let e = p.expr()?;
    p.ws();
    p.eat(";");
    p.ws();
    if p.i < p.s.len() {
        return Err(format!("unexpected {}", p.s[p.i]));
    }
    Ok(e)
}

/// An M query as a Get Data query.
#[derive(Clone, Debug, PartialEq)]
pub struct Translation {
    /// None when not even its source could be translated.
    pub query: Option<Query>,
    /// What was left out, in the M query's words (step names and
    /// functions): the query stops before the first of them.
    pub untranslated: Vec<String>,
}

/// Translates the Power Query query `name` with M `formula`; `others` are
/// the names of the workbook's other queries (which it may read).
pub fn translate(name: &str, formula: &str, others: &[String]) -> Translation {
    let e = match parse(formula) {
        Ok(e) => e,
        Err(err) => return Translation { query: None, untranslated: vec![format!("M: {err}")] },
    };
    let (binds, result) = match e {
        M::Let(b, r) => (b, *r),
        e => (Vec::new(), e),
    };
    let tr = Tr { binds: binds.iter().map(|(n, e)| (n.as_str(), e)).collect(), others, depth: 0 };
    let chain = tr.table(&result, None);
    let mut untranslated = chain.dropped;
    let query = match chain.source {
        Some(source) => {
            let mut q = Query::new(name, source);
            q.steps = chain.steps;
            Some(q)
        }
        None => {
            if untranslated.is_empty() {
                untranslated.push(name.to_string());
            }
            None
        }
    };
    untranslated.dedup();
    Translation { query, untranslated }
}

/// The steps translated up to a node of the formula.
#[derive(Default)]
struct Chain {
    source: Option<Source>,
    steps: Vec<Step>,
    /// What wasn't translated: the first, then what came after it.
    dropped: Vec<String>,
    /// The column of the last merge, not expanded yet (with the query and
    /// the left keys).
    pending_join: Option<String>,
    /// Records of a JSON list, not expanded yet.
    json_list: bool,
}

impl Chain {
    fn stop(mut self, what: String) -> Chain {
        self.dropped.push(what);
        self
    }
}

struct Tr<'a> {
    binds: HashMap<&'a str, &'a M>,
    others: &'a [String],
    depth: usize,
}

/// The function a call calls, by name.
fn callee(e: &M) -> Option<(&str, &[M])> {
    match e {
        M::Call(f, args) => match &**f {
            M::Id(n) => Some((n.as_str(), args.as_slice())),
            _ => None,
        },
        _ => None,
    }
}

fn text(e: &M) -> Option<String> {
    match e {
        M::Str(s) => Some(s.clone()),
        _ => None,
    }
}

fn num(e: &M) -> Option<f64> {
    match e {
        M::Num(n) => Some(*n),
        M::Neg(x) => num(x).map(|n| -n),
        _ => None,
    }
}

/// `{"a", "b"}` or `"a"`.
fn names(e: &M) -> Option<Vec<String>> {
    match e {
        M::Str(s) => Some(vec![s.clone()]),
        M::List(xs) => xs.iter().map(text).collect(),
        _ => None,
    }
}

/// `{{"a", x}, {"b", y}}` or `{"a", x}`.
fn pairs(e: &M) -> Option<Vec<(&M, &M)>> {
    let M::List(xs) = e else { return None };
    if let [a, b] = xs.as_slice()
        && matches!(a, M::Str(_))
    {
        return Some(vec![(a, b)]);
    }
    xs.iter()
        .map(|x| match x {
            M::List(p) if p.len() >= 2 => Some((&p[0], &p[1])),
            _ => None,
        })
        .collect()
}

fn record<'m>(e: &'m M, key: &str) -> Option<&'m M> {
    match e {
        M::Record(fs) => fs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
        _ => None,
    }
}

/// A literal as the text of a filter value.
fn literal(e: &M) -> Option<String> {
    match e {
        M::Str(s) => Some(s.clone()),
        M::Num(_) | M::Neg(_) => num(e).map(|n| n.to_string()),
        M::Bool(b) => Some(b.to_string()),
        M::Call(..) => match callee(e)? {
            ("#date", [y, m, d]) => Some(format!("{:04}-{:02}-{:02}", num(y)?, num(m)?, num(d)?)),
            ("#datetime", [y, mo, d, h, mi, s]) => Some(format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", num(y)?, num(mo)?, num(d)?, num(h)?, num(mi)?, num(s)?)),
            _ => None,
        },
        _ => None,
    }
}

fn col_type(e: &M) -> Option<ColType> {
    let M::Type(t) = e else { return None };
    Some(match t.as_str() {
        "text" | "Text.Type" => ColType::Text,
        "number" | "Number.Type" | "Currency.Type" | "Percentage.Type" | "Decimal.Type" | "Double.Type" | "Single.Type" => ColType::Number,
        "Int64.Type" | "Int32.Type" | "Int16.Type" | "Int8.Type" | "Byte.Type" => ColType::Integer,
        "logical" | "Logical.Type" => ColType::Bool,
        "date" | "Date.Type" => ColType::Date,
        "datetime" | "datetimezone" | "DateTime.Type" | "DateTimeZone.Type" => ColType::DateTime,
        _ => ColType::Any,
    })
}

/// Where `File.Contents(path)` or `Web.Contents(url)` reads.
fn contents(e: &M) -> Option<String> {
    match callee(e)? {
        ("File.Contents" | "Web.Contents", [p]) => text(p),
        _ => None,
    }
}

impl Tr<'_> {
    /// Follows names of steps to what they are.
    fn resolve<'b>(&'b self, mut e: &'b M) -> &'b M {
        for _ in 0..64 {
            match e {
                M::Id(n) => match self.binds.get(n.as_str()) {
                    Some(b) => e = b,
                    None => return e,
                },
                _ => return e,
            }
        }
        e
    }

    /// A name for an untranslated part: its step name or its function.
    fn label(e: &M, step: Option<&str>) -> String {
        let f = callee(e).map(|(f, _)| f.to_string());
        match (step, f) {
            (Some(s), Some(f)) => format!("{s} ({f})"),
            (Some(s), None) => s.to_string(),
            (None, Some(f)) => f,
            (None, None) => "?".into(),
        }
    }

    /// The table `e` is, as a source and the steps after it.
    fn table(&self, e: &M, step: Option<&str>) -> Chain {
        if self.depth > 200 {
            return Chain::default().stop("…".into());
        }
        let deeper = Tr { binds: self.binds.clone(), others: self.others, depth: self.depth + 1 };
        if let M::Id(n) = e {
            if let Some(b) = self.binds.get(n.as_str()) {
                return deeper.table(b, Some(n));
            }
            if self.others.iter().any(|o| o == n) {
                return Chain { source: Some(Source::Query { name: n.clone() }), ..Chain::default() };
            }
            return Chain::default().stop(n.clone());
        }
        if let Some(src) = self.source(e) {
            return src;
        }
        let Some((f, args)) = callee(e) else { return Chain::default().stop(Self::label(e, step)) };
        let identity = matches!(f, "Table.Buffer" | "Table.StopFolding" | "Table.FromRecords");
        let Some(prev) = args.first() else { return Chain::default().stop(Self::label(e, step)) };
        let prev = if f == "Table.Combine" {
            match prev {
                M::List(xs) if !xs.is_empty() => &xs[0],
                _ => return Chain::default().stop(Self::label(e, step)),
            }
        } else {
            prev
        };
        if !matches!(prev, M::Id(_) | M::Call(..) | M::Field(..) | M::Item(..)) {
            // Not a table: a function we don't know reading a path or such.
            return Chain::default().stop(Self::label(e, step));
        }
        let mut c = deeper.table(prev, None);
        if !c.dropped.is_empty() || c.source.is_none() {
            return c.stop(Self::label(e, step));
        }
        if identity {
            return c;
        }
        match self.step(f, args, &mut c) {
            Some(steps) => {
                c.steps.extend(steps);
                c
            }
            None => c.stop(Self::label(e, step)),
        }
    }

    /// A source (with the navigation steps Excel puts after it).
    fn source(&self, e: &M) -> Option<Chain> {
        self.source_at(e, 0)
    }

    fn source_at(&self, e: &M, depth: usize) -> Option<Chain> {
        // A name bound to a field of itself would never end.
        if depth > 64 {
            return None;
        }
        let src = |s: Source| Some(Chain { source: Some(s), ..Chain::default() });
        match e {
            M::Field(base, field) => {
                let M::Item(of, key) = self.resolve(base) else {
                    // JSON: Source[data][items].
                    let mut c = self.source_at(self.resolve(base), depth + 1)?;
                    if let Some(Source::Json { path, .. }) = &mut c.source {
                        if !path.is_empty() {
                            path.push('.');
                        }
                        path.push_str(field);
                        return Some(c);
                    }
                    return None;
                };
                let of = self.resolve(of);
                let key = self.resolve(key);
                match callee(of)? {
                    ("Excel.Workbook", args) if field == "Data" => {
                        let path = contents(self.resolve(args.first()?))?;
                        let item = text(record(key, "Item").or_else(|| record(key, "Name"))?)?;
                        let header = args.get(1).is_some_and(|h| matches!(h, M::Bool(true))) || record(key, "Kind").and_then(text).is_some_and(|k| k == "Table");
                        src(Source::Workbook { path, item, header })
                    }
                    ("Excel.CurrentWorkbook", []) if field == "Content" => src(Source::Table { name: text(record(key, "Name")?)? }),
                    ("Web.Page", [c]) if field == "Data" => src(Source::Html { location: contents(self.resolve(c))?, index: num(key)? as usize }),
                    ("Access.Database", [c, ..]) if field == "Data" => src(Source::DbFile { path: contents(self.resolve(c))?, table: text(record(key, "Item")?)? }),
                    ("Sql.Database" | "PostgreSQL.Database" | "MySQL.Database", [server, db, ..]) if field == "Data" => {
                        let mut s = self.database(of, server, db, None)?;
                        let item = text(record(key, "Item")?)?;
                        let schema = record(key, "Schema").and_then(text).filter(|s| !s.is_empty());
                        if let Source::Database { kind, sql, .. } = &mut s {
                            let q = |n: &str| match kind {
                                DbKind::MySql => format!("`{}`", n.replace('`', "``")),
                                DbKind::SqlServer => format!("[{}]", n.replace(']', "]]")),
                                DbKind::Postgres => format!("\"{}\"", n.replace('"', "\"\"")),
                            };
                            *sql = format!("SELECT * FROM {}{}", schema.map(|s| format!("{}.", q(&s))).unwrap_or_default(), q(&item));
                        }
                        src(s)
                    }
                    _ => None,
                }
            }
            M::Call(..) => match callee(e)? {
                ("Csv.Document", [c, rest @ ..]) => {
                    let location = contents(self.resolve(c))?;
                    let opts = rest.first();
                    let delim = opts.and_then(|o| record(o, "Delimiter")).or(rest.get(1)).and_then(text).and_then(|d| d.chars().next());
                    let code = opts.and_then(|o| record(o, "Encoding")).or(rest.get(3)).and_then(num);
                    let encoding = match code.map(|c| c as u32) {
                        Some(65001) => "utf-8",
                        Some(1252) => "windows-1252",
                        Some(1200) => "utf-16le",
                        Some(1201) => "utf-16be",
                        Some(28591) => "iso-8859-1",
                        _ => "",
                    };
                    src(Source::Text { location, delimiter: delim, encoding: encoding.into(), header: false, skip: 0 })
                }
                ("Json.Document", [c, ..]) => src(Source::Json { location: contents(self.resolve(c))?, path: String::new() }),
                ("Sql.Database" | "PostgreSQL.Database" | "MySQL.Database", [server, db, opts]) => {
                    let sql = text(record(opts, "Query")?)?;
                    src(self.database(e, server, db, Some(sql))?)
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn database(&self, call: &M, server: &M, db: &M, sql: Option<String>) -> Option<Source> {
        let (f, _) = callee(call)?;
        let kind = match f {
            "Sql.Database" => DbKind::SqlServer,
            "PostgreSQL.Database" => DbKind::Postgres,
            _ => DbKind::MySql,
        };
        let server = text(server)?;
        // "host,port" (SQL Server) or "host:port".
        let (host, port) = match server.rsplit_once([',', ':']) {
            Some((h, p)) if p.trim().parse::<u16>().is_ok() => (h.trim().to_string(), p.trim().parse().ok()?),
            _ => (server.trim().to_string(), kind.default_port()),
        };
        Some(Source::Database { kind, host, port, database: text(db)?, user: String::new(), sql: sql.unwrap_or_default(), tls: false })
    }

    /// A step on the table so far (`args[0]`); None when it has no
    /// equivalent.
    fn step(&self, f: &str, args: &[M], c: &mut Chain) -> Option<Vec<Step>> {
        let arg = |i: usize| args.get(i).map(|a| self.resolve(a));
        if c.pending_join.is_some() && f != "Table.ExpandTableColumn" {
            // A merge that brings a whole table into a column: we don't
            // have those.
            return None;
        }
        Some(match f {
            "Table.PromoteHeaders" => vec![Step::PromoteHeaders],
            "Table.TransformColumnTypes" => pairs(arg(1)?)?.into_iter().map(|(n, t)| Some(Step::ChangeType { column: text(n)?, ty: col_type(t)? })).collect::<Option<_>>()?,
            "Table.RemoveColumns" => vec![Step::RemoveColumns(names(arg(1)?)?)],
            "Table.SelectColumns" | "Table.ReorderColumns" => vec![Step::ChooseColumns(names(arg(1)?)?)],
            "Table.RenameColumns" => vec![Step::Rename(pairs(arg(1)?)?.into_iter().map(|(a, b)| Some((text(a)?, text(b)?))).collect::<Option<_>>()?)],
            "Table.SelectRows" => {
                let M::Each(cond) = arg(1)? else { return None };
                filters(cond)?
            }
            "Table.Skip" | "Table.RemoveFirstN" => vec![Step::KeepRows { skip: count(arg(1))?, take: 0 }],
            "Table.FirstN" => vec![Step::KeepRows { skip: 0, take: count(arg(1))?.max(1) }],
            "Table.Range" => vec![Step::KeepRows { skip: count(arg(1))?, take: arg(2).map_or(Some(0), |n| count(Some(n)).map(|n| n.max(1)))? }],
            "Table.Distinct" => vec![Step::RemoveDuplicates(arg(1).map_or(Some(Vec::new()), names)?)],
            "Table.Sort" => {
                let keys = match arg(1)? {
                    M::Str(s) => vec![(s.clone(), true)],
                    M::List(xs) if xs.iter().all(|x| matches!(x, M::Str(_))) => xs.iter().map(|x| Some((text(x)?, true))).collect::<Option<_>>()?,
                    l => pairs(l)?.into_iter().map(|(n, o)| Some((text(n)?, !matches!(o, M::Id(o) if o == "Order.Descending") && num(o) != Some(1.0)))).collect::<Option<_>>()?,
                };
                vec![Step::Sort(keys)]
            }
            "Table.AddColumn" => {
                let M::Each(body) = arg(2)? else { return None };
                vec![Step::AddColumn { name: text(arg(1)?)?, formula: formula(body)? }]
            }
            "Table.DuplicateColumn" => vec![Step::AddColumn { name: text(arg(2)?)?, formula: format!("[{}]", column_ref(&text(arg(1)?)?)?) }],
            "Table.ReplaceValue" => {
                let (find, replace) = (literal(arg(1)?)?, literal(arg(2)?)?);
                let whole = match arg(3)? {
                    M::Id(r) if r == "Replacer.ReplaceText" => false,
                    M::Id(r) if r == "Replacer.ReplaceValue" => true,
                    _ => return None,
                };
                names(arg(4)?)?.into_iter().map(|column| Step::Replace { column, find: find.clone(), replace: replace.clone(), whole }).collect()
            }
            "Table.SplitColumn" => {
                let column = text(arg(1)?)?;
                let delimiter = match callee(arg(2)?)? {
                    ("Splitter.SplitTextByDelimiter", [d, ..]) => text(d)?,
                    ("Splitter.SplitTextByEachDelimiter", [M::List(ds), ..]) if ds.len() == 1 => text(&ds[0])?,
                    _ => return None,
                };
                vec![Step::Split { column, delimiter }]
            }
            "Table.Unpivot" => vec![Step::Unpivot { columns: names(arg(1)?)?, attribute: text(arg(2)?)?, value: text(arg(3)?)? }],
            "Table.Pivot" => {
                let func = match arg(4) {
                    None => AggFn::First,
                    Some(M::Id(n)) => list_fn(n)?,
                    _ => return None,
                };
                vec![Step::Pivot { column: text(arg(2)?)?, values: text(arg(3)?)?, func }]
            }
            "Table.Group" => {
                let by = names(arg(1)?)?;
                let M::List(specs) = arg(2)? else { return None };
                let specs: Vec<&M> = if specs.first().is_some_and(|s| matches!(s, M::Str(_))) { vec![arg(2)?] } else { specs.iter().collect() };
                let aggs = specs
                    .into_iter()
                    .map(|s| {
                        let M::List(p) = s else { return None };
                        let name = text(p.first()?)?;
                        let M::Each(body) = p.get(1)? else { return None };
                        let (func, column) = aggregate(body)?;
                        Some(Aggregate { column, func, name })
                    })
                    .collect::<Option<_>>()?;
                vec![Step::Group { by, aggs }]
            }
            "Table.Combine" => {
                let M::List(xs) = args.first()? else { return None };
                xs[1..]
                    .iter()
                    .map(|x| match x {
                        M::Id(n) if !self.binds.contains_key(n.as_str()) && self.others.iter().any(|o| o == n) => Some(Step::Append { query: n.clone() }),
                        _ => None,
                    })
                    .collect::<Option<_>>()?
            }
            "Table.NestedJoin" => {
                let keys = names(arg(1)?)?;
                let M::Id(query) = args.get(2)? else { return None };
                if self.binds.contains_key(query.as_str()) || !self.others.iter().any(|o| o == query) {
                    return None;
                }
                let theirs = names(arg(3)?)?;
                if keys.len() != theirs.len() {
                    return None;
                }
                let join = match arg(5) {
                    None => JoinKind::Left,
                    Some(M::Id(k)) => match k.as_str() {
                        "JoinKind.Inner" => JoinKind::Inner,
                        "JoinKind.LeftOuter" => JoinKind::Left,
                        "JoinKind.RightOuter" => JoinKind::Right,
                        "JoinKind.FullOuter" => JoinKind::Full,
                        "JoinKind.LeftAnti" => JoinKind::Anti,
                        _ => return None,
                    },
                    _ => return None,
                };
                c.pending_join = Some(text(arg(4)?)?);
                vec![Step::Merge { query: query.clone(), keys: keys.into_iter().zip(theirs).collect(), join, columns: Vec::new(), names: Vec::new() }]
            }
            "Table.ExpandTableColumn" => {
                let col = text(arg(1)?)?;
                if c.pending_join.as_deref() != Some(col.as_str()) {
                    return None;
                }
                let Some(Step::Merge { columns, names: renamed, .. }) = c.steps.last_mut() else { return None };
                let cols = names(arg(2)?)?;
                let new = arg(3).map_or(Some(cols.clone()), names)?;
                if new.len() != cols.len() {
                    return None;
                }
                // The columns come with exactly the names M gives them,
                // whatever this table's are called.
                *renamed = cols.iter().cloned().zip(new).filter(|(o, n)| o != n).collect();
                *columns = cols;
                c.pending_join = None;
                Vec::new()
            }
            "Table.FromList" if matches!(c.source, Some(Source::Json { .. })) && c.steps.is_empty() => {
                c.json_list = true;
                Vec::new()
            }
            "Table.ExpandRecordColumn" if c.json_list && text(arg(1)?)? == "Column1" => {
                // Our JSON source already makes the records' fields columns.
                c.json_list = false;
                let cols = names(arg(2)?)?;
                let new = arg(3).map_or(Some(cols.clone()), names)?;
                let renames: Vec<(String, String)> = cols.iter().cloned().zip(new).filter(|(o, n)| o != n).collect();
                let mut steps = vec![Step::ChooseColumns(cols)];
                if !renames.is_empty() {
                    steps.push(Step::Rename(renames));
                }
                steps
            }
            _ => return None,
        })
    }
}

fn count(e: Option<&M>) -> Option<usize> {
    num(e?).filter(|n| *n >= 0.0 && n.fract() == 0.0).map(|n| n as usize)
}

/// `List.Sum` and the like as an aggregate.
fn list_fn(n: &str) -> Option<AggFn> {
    Some(match n {
        "List.Sum" => AggFn::Sum,
        "List.Average" => AggFn::Average,
        "List.Min" => AggFn::Min,
        "List.Max" => AggFn::Max,
        "List.Count" => AggFn::Count,
        "List.First" => AggFn::First,
        "List.Last" => AggFn::Last,
        _ => return None,
    })
}

/// The body of a grouping's `each`: the function and its column.
fn aggregate(body: &M) -> Option<(AggFn, String)> {
    let (f, args) = callee(body)?;
    match (f, args) {
        ("Table.RowCount", [M::Id(u)]) if u == "_" => Some((AggFn::Count, String::new())),
        ("List.Count", [inner]) if callee(inner).is_some_and(|(g, _)| g == "List.Distinct") => {
            let (_, a) = callee(inner)?;
            Some((AggFn::CountDistinct, own_field(a.first()?)?))
        }
        (f, [x]) => Some((list_fn(f)?, own_field(x)?)),
        _ => None,
    }
}

/// `[Col]` of the row (or group) at hand.
fn own_field(e: &M) -> Option<String> {
    match e {
        M::Field(b, n) if matches!(&**b, M::Id(u) if u == "_") => Some(n.clone()),
        _ => None,
    }
}

/// The condition of a `Table.SelectRows` as filter steps (all must hold).
fn filters(cond: &M) -> Option<Vec<Step>> {
    // M tells capitals apart in text.
    let f = |column: String, op: FilterOp, value: String| Some(vec![Step::Filter { case: cased(&value), column, op, value }]);
    match cond {
        M::Bin("and", a, b) => {
            let mut v = filters(a)?;
            v.extend(filters(b)?);
            Some(v)
        }
        M::Bin(op, a, b) => {
            let (column, value, flipped) = match (own_field(a), own_field(b)) {
                (Some(col), None) => (col, &**b, false),
                (None, Some(col)) => (col, &**a, true),
                _ => return None,
            };
            if matches!(value, M::Null) || matches!(value, M::Str(s) if s.is_empty()) {
                return match *op {
                    "=" => f(column, FilterOp::Empty, String::new()),
                    "<>" => f(column, FilterOp::NotEmpty, String::new()),
                    _ => None,
                };
            }
            let op = match (*op, flipped) {
                ("=", _) => FilterOp::Eq,
                ("<>", _) => FilterOp::Ne,
                (">", false) | ("<", true) => FilterOp::Gt,
                (">=", false) | ("<=", true) => FilterOp::Ge,
                ("<", false) | (">", true) => FilterOp::Lt,
                ("<=", false) | (">=", true) => FilterOp::Le,
                _ => return None,
            };
            f(column, op, literal(value)?)
        }
        M::Not(inner) => match &**inner {
            M::Bin("=", ..) => {
                let mut v = filters(inner)?;
                if let Some(Step::Filter { op, .. }) = v.first_mut() {
                    *op = if *op == FilterOp::Empty { FilterOp::NotEmpty } else { FilterOp::Ne };
                }
                Some(v)
            }
            M::Call(..) => {
                let mut v = filters(inner)?;
                match v.first_mut() {
                    Some(Step::Filter { op, .. }) if *op == FilterOp::Contains => *op = FilterOp::NotContains,
                    _ => return None,
                }
                Some(v)
            }
            _ => None,
        },
        M::Call(..) => {
            let (name, args) = callee(cond)?;
            let op = match name {
                "Text.Contains" => FilterOp::Contains,
                "Text.StartsWith" => FilterOp::Begins,
                "Text.EndsWith" => FilterOp::Ends,
                _ => return None,
            };
            let [x, v] = args else { return None };
            f(own_field(x)?, op, text(v)?)
        }
        _ => None,
    }
}

/// Whether capitals in `s` matter (M's comparisons tell them apart).
fn cased(s: &str) -> bool {
    s.to_lowercase() != s.to_uppercase()
}

/// A column name inside `[…]` of a calculated column's formula.
fn column_ref(n: &str) -> Option<&str> {
    (!n.contains(']')).then_some(n)
}

/// M's rounding of `x` to `d` digits with `mode` in our formulas: ROUND
/// goes away from zero, M goes to even by default; RoundUp and RoundDown
/// are a ceiling and a floor.
fn rounding(x: &str, d: &str, mode: &str) -> Option<String> {
    let scaled = if d == "0" { format!("({x})") } else { format!("(({x}) * POWER(10, {d}))") };
    let back = |e: String| if d == "0" { e } else { format!("({e} / POWER(10, {d}))") };
    Some(match mode {
        "RoundingMode.AwayFromZero" => format!("ROUND({x}, {d})"),
        "RoundingMode.TowardZero" => format!("ROUNDDOWN({x}, {d})"),
        "RoundingMode.Down" => back(format!("INT{scaled}")),
        "RoundingMode.Up" => back(format!("-INT(-{scaled})")),
        "RoundingMode.ToEven" => back(format!("IF(ABS({scaled} - INT{scaled}) = 0.5, 2 * ROUND({scaled} / 2, 0), ROUND({scaled}, 0))")),
        _ => return None,
    })
}

/// An `each` expression as a calculated column's formula.
fn formula(e: &M) -> Option<String> {
    Some(match e {
        M::Null => "NULL".into(),
        M::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
        M::Num(n) => n.to_string(),
        M::Str(s) => format!("\"{}\"", s.replace('"', "\"\"")),
        M::Field(..) => format!("[{}]", column_ref(&own_field(e)?)?),
        M::Bin(op, x, y) => {
            let (a, b) = (formula(x)?, formula(y)?);
            // M's `=` tells capitals apart; ours doesn't (EXACT does). With
            // a number, logical or null there's no text to tell.
            let plain = |e: &M| matches!(e, M::Num(_) | M::Bool(_) | M::Null);
            match *op {
                "=" if !plain(x) && !plain(y) => format!("EXACT({a}, {b})"),
                "<>" if !plain(x) && !plain(y) => format!("NOT(EXACT({a}, {b}))"),
                "and" => format!("AND({a}, {b})"),
                "or" => format!("OR({a}, {b})"),
                "??" => format!("COALESCE({a}, {b})"),
                op => format!("({a} {op} {b})"),
            }
        }
        M::Not(x) => format!("NOT({})", formula(x)?),
        M::Neg(x) => format!("-({})", formula(x)?),
        M::If(c, t, f) => format!("IF({}, {}, {})", formula(c)?, formula(t)?, formula(f)?),
        M::Call(..) => {
            let (name, args) = callee(e)?;
            // A rounding mode is a name, not a value.
            let values = if name.starts_with("Number.Round") && args.len() == 3 { &args[..2] } else { args };
            let a: Vec<String> = values.iter().map(formula).collect::<Option<_>>()?;
            let one = |f: &str| (a.len() == 1).then(|| format!("{f}({})", a[0]));
            let all = |f: &str| format!("{f}({})", a.join(", "));
            match name {
                "Text.Upper" => one("UPPER")?,
                "Text.Lower" => one("LOWER")?,
                "Text.Proper" => one("PROPER")?,
                "Text.Trim" => one("TRIMENDS")?,
                "Text.Length" => one("LEN")?,
                "Text.From" => one("CONCAT")?,
                "Text.Start" if a.len() == 2 => all("LEFT"),
                "Text.End" if a.len() == 2 => all("RIGHT"),
                // M counts from 0.
                "Text.Middle" if a.len() == 3 => format!("MID({}, ({}) + 1, {})", a[0], a[1], a[2]),
                "Text.Middle" if a.len() == 2 => format!("MID({}, ({}) + 1, 1000000)", a[0], a[1]),
                // FIND tells capitals apart, as M does.
                "Text.Contains" if a.len() == 2 => format!("NOT(ISBLANK(FIND({}, {})))", a[1], a[0]),
                "Text.Replace" if a.len() == 3 => all("SUBSTITUTE"),
                "Number.From" => one("VALUE")?,
                "Number.Abs" => one("ABS")?,
                "Number.Sqrt" => one("SQRT")?,
                "Number.Power" if a.len() == 2 => all("POWER"),
                "Number.Mod" if a.len() == 2 => all("MOD"),
                "Number.Round" | "Number.RoundUp" | "Number.RoundDown" if !a.is_empty() && a.len() <= 2 => {
                    let mode = match (name, args.get(2)) {
                        ("Number.RoundUp", _) => "RoundingMode.Up",
                        ("Number.RoundDown", _) => "RoundingMode.Down",
                        (_, None) => "RoundingMode.ToEven",
                        (_, Some(M::Id(m))) => m.as_str(),
                        _ => return None,
                    };
                    rounding(&a[0], a.get(1).map_or("0", String::as_str), mode)?
                }
                "Date.Year" => one("YEAR")?,
                "Date.Month" => one("MONTH")?,
                "Date.Day" => one("DAY")?,
                "#date" if a.len() == 3 => all("DATE"),
                _ => return None,
            }
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv(m: &str) -> Translation {
        translate("Q", m, &["Products".to_string()])
    }

    #[test]
    fn a_csv_query_with_its_usual_steps() {
        let m = r#"let
    Origen = Csv.Document(File.Contents("C:\datos\ventas.csv"),[Delimiter=";", Columns=4, Encoding=65001, QuoteStyle=QuoteStyle.None]),
    #"Encabezados promovidos" = Table.PromoteHeaders(Origen, [PromoteAllScalars=true]),
    #"Tipo cambiado" = Table.TransformColumnTypes(#"Encabezados promovidos",{{"Fecha", type date}, {"Importe", type number}, {"Unidades", Int64.Type}, {"Región", type text}}),
    // Only the north.
    #"Filas filtradas" = Table.SelectRows(#"Tipo cambiado", each ([Región] = "Norte") and [Importe] > 100 and [Fecha] <> null),
    #"Columnas quitadas" = Table.RemoveColumns(#"Filas filtradas",{"Unidades"}),
    #"Personalizada agregada" = Table.AddColumn(#"Columnas quitadas", "IVA", each if [Importe] > 1000 then Number.Round([Importe] * 0.21, 2, RoundingMode.AwayFromZero) else 0, type number),
    #"Filas ordenadas" = Table.Sort(#"Personalizada agregada",{{"Fecha", Order.Descending}})
in
    #"Filas ordenadas""#;
        let t = conv(m);
        assert!(t.untranslated.is_empty(), "{:?}", t.untranslated);
        let q = t.query.unwrap();
        assert_eq!(q.source, Source::Text { location: r"C:\datos\ventas.csv".into(), delimiter: Some(';'), encoding: "utf-8".into(), header: false, skip: 0 });
        assert_eq!(q.steps[0], Step::PromoteHeaders);
        assert_eq!(q.steps[1], Step::ChangeType { column: "Fecha".into(), ty: ColType::Date });
        assert_eq!(q.steps[3], Step::ChangeType { column: "Unidades".into(), ty: ColType::Integer });
        assert_eq!(q.steps[5], Step::Filter { column: "Región".into(), op: FilterOp::Eq, value: "Norte".into(), case: true });
        assert_eq!(q.steps[6], Step::Filter { column: "Importe".into(), op: FilterOp::Gt, value: "100".into(), case: false });
        assert_eq!(q.steps[7], Step::Filter { column: "Fecha".into(), op: FilterOp::NotEmpty, value: String::new(), case: false });
        assert_eq!(q.steps[8], Step::RemoveColumns(vec!["Unidades".into()]));
        assert_eq!(q.steps[9], Step::AddColumn { name: "IVA".into(), formula: "IF(([Importe] > 1000), ROUND(([Importe] * 0.21), 2), 0)".into() });
        assert_eq!(q.steps[10], Step::Sort(vec![("Fecha".into(), false)]));
    }

    #[test]
    fn workbooks_tables_and_databases() {
        let m = r#"let Source = Excel.Workbook(File.Contents("/d/b.xlsx"), null, true), Hoja1_Sheet = Source{[Item="Hoja1",Kind="Sheet"]}[Data] in Hoja1_Sheet"#;
        // The `true` is delayTypes: headers come with Table.PromoteHeaders.
        assert_eq!(conv(m).query.unwrap().source, Source::Workbook { path: "/d/b.xlsx".into(), item: "Hoja1".into(), header: false });
        let m = r#"let Source = Excel.Workbook(File.Contents("/d/b.xlsx"), true), T = Source{[Item="T1",Kind="Table"]}[Data] in T"#;
        assert_eq!(conv(m).query.unwrap().source, Source::Workbook { path: "/d/b.xlsx".into(), item: "T1".into(), header: true });
        let m = r#"let Source = Excel.CurrentWorkbook(){[Name="Tabla1"]}[Content], #"Changed Type" = Table.TransformColumnTypes(Source,{{"A", type any}}) in #"Changed Type""#;
        let q = conv(m).query.unwrap();
        assert_eq!(q.source, Source::Table { name: "Tabla1".into() });
        assert_eq!(q.steps, [Step::ChangeType { column: "A".into(), ty: ColType::Any }]);
        let m = r#"let Source = Sql.Database("srv,1500", "Ventas"), dbo_Pedidos = Source{[Schema="dbo",Item="Pedidos"]}[Data] in dbo_Pedidos"#;
        assert_eq!(
            conv(m).query.unwrap().source,
            Source::Database { kind: DbKind::SqlServer, host: "srv".into(), port: 1500, database: "Ventas".into(), user: String::new(), sql: "SELECT * FROM [dbo].[Pedidos]".into(), tls: false }
        );
        let m = r#"let Source = PostgreSQL.Database("db.local", "shop", [Query="select * from t"]) in Source"#;
        let Some(Source::Database { kind: DbKind::Postgres, port: 5432, sql, .. }) = conv(m).query.map(|q| q.source) else { panic!() };
        assert_eq!(sql, "select * from t");
        let m = r#"let Source = Web.Page(Web.Contents("https://example.com/t")), Data0 = Source{0}[Data] in Data0"#;
        assert_eq!(conv(m).query.unwrap().source, Source::Html { location: "https://example.com/t".into(), index: 0 });
        let m = r#"let Source = Json.Document(Web.Contents("https://api/x")), data = Source[data], #"Converted to Table" = Table.FromList(data, Splitter.SplitByNothing(), null, null, ExtraValues.Error), #"Expanded Column1" = Table.ExpandRecordColumn(#"Converted to Table", "Column1", {"id", "name"}, {"id", "Nombre"}) in #"Expanded Column1""#;
        let q = conv(m).query.unwrap();
        assert_eq!(q.source, Source::Json { location: "https://api/x".into(), path: "data".into() });
        assert_eq!(q.steps, [Step::ChooseColumns(vec!["id".into(), "name".into()]), Step::Rename(vec![("name".into(), "Nombre".into())])]);
    }

    #[test]
    fn groups_merges_and_other_queries() {
        let m = r#"let
    Source = Ventas,
    #"Grouped Rows" = Table.Group(Source, {"Region"}, {{"Total", each List.Sum([Amount]), type nullable number}, {"Count", each Table.RowCount(_), Int64.Type}, {"Clientes", each List.Count(List.Distinct([Cliente])), Int64.Type}}),
    #"Merged Queries" = Table.NestedJoin(#"Grouped Rows", {"Region"}, Products, {"Region"}, "Products", JoinKind.LeftOuter),
    #"Expanded Products" = Table.ExpandTableColumn(#"Merged Queries", "Products", {"Region", "Manager"}, {"Products.Region", "Jefe"}),
    Combined = Table.Combine({#"Expanded Products", Products})
in
    Combined"#;
        let t = translate("Q", m, &["Ventas".to_string(), "Products".to_string()]);
        assert!(t.untranslated.is_empty(), "{:?}", t.untranslated);
        let q = t.query.unwrap();
        assert_eq!(q.source, Source::Query { name: "Ventas".into() });
        let Step::Group { by, aggs } = &q.steps[0] else { panic!() };
        assert_eq!(by, &["Region"]);
        assert_eq!(aggs.iter().map(|a| (a.func, a.column.as_str(), a.name.as_str())).collect::<Vec<_>>(), [(AggFn::Sum, "Amount", "Total"), (AggFn::Count, "", "Count"), (AggFn::CountDistinct, "Cliente", "Clientes")]);
        assert_eq!(q.steps[1], Step::Merge { query: "Products".into(), keys: vec![("Region".into(), "Region".into())], join: JoinKind::Left, columns: vec!["Region".into(), "Manager".into()], names: vec![("Region".into(), "Products.Region".into()), ("Manager".into(), "Jefe".into())] });
        assert_eq!(q.steps[2], Step::Append { query: "Products".into() });
    }

    #[test]
    fn what_has_no_step_stops_the_query_and_is_named() {
        let m = r#"let
    Source = Csv.Document(File.Contents("a.csv")),
    #"Promoted Headers" = Table.PromoteHeaders(Source),
    #"Filled Down" = Table.FillDown(#"Promoted Headers",{"A"}),
    #"Removed Columns" = Table.RemoveColumns(#"Filled Down",{"B"})
in
    #"Removed Columns""#;
        let t = conv(m);
        assert_eq!(t.query.unwrap().steps, [Step::PromoteHeaders]);
        assert_eq!(t.untranslated, ["Filled Down (Table.FillDown)", "Removed Columns (Table.RemoveColumns)"]);
        // A filter we can't say.
        let t = conv(r#"let S = Csv.Document(File.Contents("a.csv")), F = Table.SelectRows(S, each [A] = 1 or [B] = 2) in F"#);
        assert_eq!(t.untranslated, ["F (Table.SelectRows)"]);
        // No source.
        let t = conv(r#"let Source = Folder.Files("C:\x") in Source"#);
        assert!(t.query.is_none());
        assert_eq!(t.untranslated, ["Source (Folder.Files)"]);
        assert!(conv("let a = in").query.is_none());
    }

    #[test]
    fn m_text_and_comments() {
        let t = conv("/* c */ let S = Csv.Document(File.Contents(\"a.csv\"), [Delimiter=\"#(tab)\", Encoding=1252]) // x\n in S");
        assert_eq!(t.query.unwrap().source, Source::Text { location: "a.csv".into(), delimiter: Some('\t'), encoding: "windows-1252".into(), header: false, skip: 0 });
        let t = conv(r#"let S = Csv.Document(File.Contents("a.csv")), R = Table.RenameColumns(S, {"Column1", "Name ""x"""}), A = Table.AddColumn(R, "L", each Text.Start([Name "x"], 2) & "-" & Text.From([Column2])) in A"#);
        let q = t.query.unwrap();
        assert_eq!(q.steps[0], Step::Rename(vec![("Column1".into(), "Name \"x\"".into())]));
        assert_eq!(q.steps[1], Step::AddColumn { name: "L".into(), formula: "((LEFT([Name \"x\"], 2) & \"-\") & CONCAT([Column2]))".into() });
    }

    #[test]
    fn deep_queries_are_refused() {
        assert!(parse(&format!("{}1{}", "(".repeat(100_000), ")".repeat(100_000))).is_err());
        assert!(parse(&format!("{}1", "-".repeat(100_000))).is_err());
        assert!(parse(&format!("1{}", "+1".repeat(100_000))).is_err());
        assert!(parse("let a = (1 + 2) * -3 in a").is_ok());
    }

    #[test]
    fn capitals_and_spaces_as_m_has_them() {
        let csv = r#"S = Csv.Document(File.Contents("a.csv"))"#;
        let q = conv(&format!(r#"let {csv}, A = Table.AddColumn(S, "x", each if [Column1] = "Ab" and [Column2] <> 3 then Text.Trim([Column1]) else Text.Contains([Column1], "b")) in A"#)).query.unwrap();
        let Step::AddColumn { formula, .. } = &q.steps[0] else { panic!() };
        assert_eq!(formula, r#"IF(AND(EXACT([Column1], "Ab"), ([Column2] <> 3)), TRIMENDS([Column1]), NOT(ISBLANK(FIND("b", [Column1]))))"#);
        let q = conv(&format!(r#"let {csv}, F = Table.SelectRows(S, each Text.StartsWith([Column1], "Ab") and [Column2] = "12") in F"#)).query.unwrap();
        assert_eq!(q.steps, [Step::Filter { column: "Column1".into(), op: FilterOp::Begins, value: "Ab".into(), case: true }, Step::Filter { column: "Column2".into(), op: FilterOp::Eq, value: "12".into(), case: false }]);
    }

    #[test]
    fn audit_cases() {
        let csv = r#"S = Csv.Document(File.Contents("a.csv"), 3, ",", ExtraValues.Ignore, 1252)"#;
        let q = conv(&format!("let {csv} in S")).query.unwrap();
        assert_eq!(q.source, Source::Text { location: "a.csv".into(), delimiter: Some(','), encoding: "windows-1252".into(), header: false, skip: 0 });
        // `??` after a call, and records with quoted names.
        let t = conv(&format!(r#"let {csv}, A = Table.AddColumn(S, "x", each Text.From([Column1]) ?? "-"), F = Table.SelectRows(A, each Text.Contains([x], "a")), R = Table.FromRecords({{[#"My Col" = 1]}}) in F"#));
        assert!(t.untranslated.is_empty(), "{:?}", t.untranslated);
        assert_eq!(t.query.unwrap().steps[0], Step::AddColumn { name: "x".into(), formula: "COALESCE(CONCAT([Column1]), \"-\")".into() });
        // A type test has no formula: the step stops.
        let t = conv(&format!(r#"let {csv}, A = Table.AddColumn(S, "n", each if [Column1] is number then 1 else 0) in A"#));
        assert_eq!(t.untranslated, ["A (Table.AddColumn)"]);
        let t = conv(&format!(r#"let {csv}, A = Table.AddColumn(S, "c", each Text.Clean([Column1])) in A"#));
        assert_eq!(t.untranslated, ["A (Table.AddColumn)"]);
    }

    #[test]
    fn rounding_as_m_does_it() {
        use crate::{DataTable, Val};
        let mut table = DataTable::new(vec!["v".into()]);
        let cases = [(2.5, [2.0, 3.0, 2.0, 3.0]), (3.5, [4.0, 4.0, 3.0, 4.0]), (-2.5, [-2.0, -2.0, -3.0, -3.0]), (-1.5, [-2.0, -1.0, -2.0, -2.0]), (1.26, [1.0, 2.0, 1.0, 1.0])];
        for (v, _) in cases {
            table.rows.push(vec![Val::Num(v)]);
        }
        // Number.Round (to even), RoundUp, RoundDown, Round away from zero.
        for (k, m) in ["Number.Round([v])", "Number.RoundUp([v])", "Number.RoundDown([v])", "Number.Round([v], 0, RoundingMode.AwayFromZero)"].into_iter().enumerate() {
            let f = formula(&parse(&format!("each {m}")).map(|e| match e { M::Each(b) => *b, e => e }).unwrap()).unwrap();
            let e = crate::expr::parse(&f, &table).unwrap();
            for (row, (v, want)) in table.rows.iter().zip(cases) {
                assert_eq!(crate::expr::eval(&e, row), Val::Num(want[k]), "{m} of {v}: {f}");
            }
        }
        let f = formula(&parse("Number.Round([v], 1)").unwrap()).unwrap();
        let e = crate::expr::parse(&f, &table).unwrap();
        assert_eq!(crate::expr::eval(&e, &table.rows[4]), Val::Num(1.3));
    }
}
