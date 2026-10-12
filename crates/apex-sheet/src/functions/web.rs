//! Spreadsheet functions: web (ENCODEURL, FILTERXML, IMAGE) and INFO.

use super::*;

pub(super) fn array_params(_f: Func) -> Option<u64> {
    None
}

pub(super) fn call(env: &Env, f: Func, a: &[Expr]) -> Option<R<Value>> {
    use Func::*;
    let mine = matches!(f, EncodeUrl | FilterXml | Info | Image);
    mine.then(|| eval(env, f, a))
}

fn eval(env: &Env, f: Func, a: &[Expr]) -> R<Value> {
    use Func::*;
    Ok(match f {
        EncodeUrl => Value::text(encode_url(&env.text1(a)?)),
        FilterXml => {
            if a.len() != 2 {
                return Err(ErrorKind::Value);
            }
            let xml = env.text(&a[0])?;
            let path = env.text(&a[1])?;
            filter_xml(&xml, &path).ok_or(ErrorKind::Value)?
        }
        Image => {
            // IMAGE(source, [alt_text], [sizing], [height], [width]): the
            // picture has to be downloaded, which the engine doesn't do.
            if a.is_empty() || a.len() > 5 {
                return Err(ErrorKind::Value);
            }
            let src = env.text(&a[0])?;
            let sizing = match a.get(2) {
                Some(e) if !matches!(e, Expr::Missing) => env.scalar(e).to_number()?,
                _ => 0.0,
            };
            if !(0.0..=3.0).contains(&sizing) || src.trim().is_empty() {
                return Err(ErrorKind::Value);
            }
            return Err(ErrorKind::Connect);
        }
        Info => {
            let what = env.text1(a)?.to_lowercase();
            let es = env.locale.decimal == ',';
            match what.as_str() {
                "numfile" => Value::Number(env.ctx.sheet_count() as f64),
                // The window isn't known here: the view of a sheet at the top.
                "origin" => Value::text("$A:$A$1"),
                "osversion" => Value::text(os_version()),
                "recalc" => Value::text(match (env.ctx.manual_calc(), es) {
                    (true, _) => "Manual",
                    (false, true) => "Automático",
                    (false, false) => "Automatic",
                }),
                "release" => Value::text("16.0"),
                "system" => Value::text(match std::env::consts::OS {
                    "windows" => "pcdos".to_string(),
                    "macos" => "mac".to_string(),
                    os => os.to_uppercase(),
                }),
                "directory" | "memavail" | "memused" | "totmem" => return Err(ErrorKind::Na),
                _ => return Err(ErrorKind::Value),
            }
        }
        _ => return Err(ErrorKind::Name),
    })
}

/// Percent-encodes UTF-8, keeping the unreserved characters of RFC 3986.
fn encode_url(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn os_version() -> String {
    match std::env::consts::OS {
        "windows" => format!("Windows ({}-bit) NT 10.00", usize::BITS),
        "macos" => "Macintosh".to_string(),
        // Not the kernel's version: a saved workbook would carry it.
        "linux" => "Linux".to_string(),
        os => os.to_string(),
    }
}

/// FILTERXML: the texts of the nodes `path` selects, down a column.
fn filter_xml(xml: &str, path: &str) -> Option<Value> {
    let doc = Doc::parse(xml)?;
    let t = lex(path)?;
    // Longer paths make expression trees too deep to walk.
    if t.len() > 4096 {
        return None;
    }
    let x = Parser { t, i: 0, depth: 0 }.all()?;
    let cell = |s: String| match number(&s) {
        Some(n) => Value::Number(n),
        None => Value::text(s),
    };
    Some(match doc.eval(&x, It::N(0), 1, 1)? {
        V::Nodes(v) if v.is_empty() => return None,
        V::Nodes(v) if v.len() == 1 => cell(doc.string(v[0])),
        V::Nodes(v) => Value::Array(Arc::new(Matrix::new(v.len(), 1, v.iter().map(|&it| cell(doc.string(it))).collect()))),
        V::Num(n) => num_ok(n),
        V::Str(s) => cell(s),
        V::Bool(b) => Value::Bool(b),
    })
}

/// A plain decimal number (XML data doesn't follow the locale).
fn number(s: &str) -> Option<f64> {
    let s = s.trim();
    let plain = s.bytes().all(|b| b.is_ascii_digit() || b"+-.eE".contains(&b)) && s.bytes().any(|b| b.is_ascii_digit());
    plain.then(|| s.parse().ok()).flatten()
}

enum Kind {
    Root,
    Elem(String, Vec<(String, String)>),
    Text(String),
}

struct Node {
    parent: usize,
    kind: Kind,
    children: Vec<usize>,
}

/// A parsed document; node 0 is the document itself and nodes are
/// numbered in document order.
struct Doc {
    nodes: Vec<Node>,
    /// Nodes visited so far: nested predicates over the whole document
    /// multiply each other's cost.
    work: std::cell::Cell<usize>,
}

/// Nodes a FILTERXML query may visit.
const MAX_WORK: usize = 20_000_000;

/// A node or an attribute (element, index).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum It {
    N(usize),
    A(usize, usize),
}

fn unescape(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let end = rest[i..].find(';')? + i;
        let ent = &rest[i + 1..end];
        out.push(match ent {
            "lt" => '<',
            "gt" => '>',
            "amp" => '&',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let code = match ent.strip_prefix("#x").or_else(|| ent.strip_prefix("#X")) {
                    Some(h) => u32::from_str_radix(h, 16).ok()?,
                    None => ent.strip_prefix('#')?.parse().ok()?,
                };
                char::from_u32(code)?
            }
        });
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

impl Doc {
    fn parse(s: &str) -> Option<Doc> {
        let mut doc = Doc { nodes: vec![Node { parent: 0, kind: Kind::Root, children: Vec::new() }], work: std::cell::Cell::new(0) };
        let mut stack = vec![0];
        let mut i = 0;
        let name_end = |t: &str| t.find(|c: char| c.is_whitespace() || c == '/' || c == '>' || c == '=').unwrap_or(t.len());
        while i < s.len() {
            let r = &s[i..];
            let top = *stack.last()?;
            if let Some(body) = r.strip_prefix("<!--") {
                i += 4 + body.find("-->")? + 3;
            } else if let Some(body) = r.strip_prefix("<![CDATA[") {
                let end = body.find("]]>")?;
                if top == 0 {
                    return None;
                }
                doc.add(top, Kind::Text(body[..end].to_string()));
                i += 9 + end + 3;
            } else if r.starts_with("<?") {
                i += r.find("?>")? + 2;
            } else if r.starts_with("<!") {
                i += r.find('>')? + 1;
            } else if let Some(body) = r.strip_prefix("</") {
                let end = body.find('>')?;
                match &doc.nodes[top].kind {
                    Kind::Elem(n, _) if n == body[..end].trim() => stack.pop(),
                    _ => return None,
                };
                i += 2 + end + 1;
            } else if let Some(body) = r.strip_prefix('<') {
                let n = name_end(body);
                if n == 0 || (top == 0 && !doc.nodes[0].children.is_empty()) {
                    return None;
                }
                let name = body[..n].to_string();
                let mut j = 1 + n;
                let mut attrs = Vec::new();
                let closed = loop {
                    j += s[i + j..].len() - s[i + j..].trim_start().len();
                    let t = &s[i + j..];
                    if t.starts_with("/>") {
                        j += 2;
                        break true;
                    } else if t.starts_with('>') {
                        j += 1;
                        break false;
                    }
                    let an = name_end(t);
                    let after = t[an..].trim_start().strip_prefix('=')?.trim_start();
                    let q = after.chars().next().filter(|&q| q == '"' || q == '\'')?;
                    let end = after[1..].find(q)?;
                    if an == 0 {
                        return None;
                    }
                    attrs.push((t[..an].to_string(), unescape(&after[1..1 + end])?));
                    j += t.len() - after.len() + end + 2;
                };
                let id = doc.add(top, Kind::Elem(name, attrs));
                if !closed {
                    stack.push(id);
                    // Deeper trees would overflow the stack when walked.
                    if stack.len() > 256 {
                        return None;
                    }
                }
                i += j;
            } else {
                let end = r.find('<').unwrap_or(r.len());
                let text = unescape(&r[..end])?;
                if !text.trim().is_empty() {
                    if top == 0 {
                        return None;
                    }
                    doc.add(top, Kind::Text(text));
                }
                i += end;
            }
        }
        (stack.len() == 1 && !doc.nodes[0].children.is_empty()).then_some(doc)
    }

    fn add(&mut self, parent: usize, kind: Kind) -> usize {
        let id = self.nodes.len();
        self.nodes.push(Node { parent, kind, children: Vec::new() });
        self.nodes[parent].children.push(id);
        id
    }

    fn string(&self, it: It) -> String {
        // Each node read is work too (checked at the next evaluation).
        self.work.set(self.work.get().saturating_add(1));
        match it {
            It::A(e, k) => match &self.nodes[e].kind {
                Kind::Elem(_, a) => a[k].1.clone(),
                _ => String::new(),
            },
            It::N(n) => match &self.nodes[n].kind {
                Kind::Text(t) => t.clone(),
                _ => self.nodes[n].children.iter().map(|&c| self.string(It::N(c))).collect(),
            },
        }
    }

    fn descendants(&self, n: usize, out: &mut Vec<It>) {
        for &c in &self.nodes[n].children {
            out.push(It::N(c));
            self.descendants(c, out);
        }
    }

    fn step(&self, it: It, s: &Step) -> Option<Vec<It>> {
        let mut v = Vec::new();
        match (s.axis, it) {
            (Axis::SelfAxis, _) => v.push(it),
            (Axis::Parent, It::N(n)) if n != 0 => v.push(It::N(self.nodes[n].parent)),
            (Axis::Parent, It::A(e, _)) => v.push(It::N(e)),
            (Axis::Child, It::N(n)) => v.extend(self.nodes[n].children.iter().map(|&c| It::N(c))),
            (Axis::Desc, It::N(n)) => self.descendants(n, &mut v),
            (Axis::DescSelf, It::N(n)) => {
                v.push(it);
                self.descendants(n, &mut v);
            }
            (Axis::Attr, It::N(n)) => {
                if let Kind::Elem(_, a) = &self.nodes[n].kind {
                    v.extend((0..a.len()).map(|k| It::A(n, k)));
                }
            }
            _ => {}
        }
        self.work.set(self.work.get() + v.len() + 1);
        if self.work.get() > MAX_WORK {
            return None;
        }
        v.retain(|&it| match (it, &s.test) {
            (_, Test::Node) => true,
            (It::A(e, k), t) => match (&self.nodes[e].kind, t) {
                (Kind::Elem(_, a), Test::Name(q)) => a[k].0 == *q,
                (_, t) => matches!(t, Test::Any),
            },
            (It::N(n), t) => match (&self.nodes[n].kind, t) {
                (Kind::Elem(name, _), Test::Name(q)) => name == q,
                (Kind::Elem(..), Test::Any) | (Kind::Text(_), Test::Text) => true,
                _ => false,
            },
        });
        self.filter(v, &s.preds)
    }

    /// Keeps the items every predicate holds for (a number: the position).
    fn filter(&self, mut v: Vec<It>, preds: &[X]) -> Option<Vec<It>> {
        for p in preds {
            let n = v.len();
            let mut keep = Vec::new();
            for (i, &it) in v.iter().enumerate() {
                let ok = match self.eval(p, it, i + 1, n)? {
                    V::Num(x) => x == (i + 1) as f64,
                    x => self.truth(&x),
                };
                if ok {
                    keep.push(it);
                }
            }
            v = keep;
        }
        Some(v)
    }

    fn truth(&self, v: &V) -> bool {
        match v {
            V::Nodes(n) => !n.is_empty(),
            V::Str(s) => !s.is_empty(),
            V::Num(x) => *x != 0.0 && !x.is_nan(),
            V::Bool(b) => *b,
        }
    }

    fn text(&self, v: &V) -> String {
        match v {
            V::Nodes(n) => n.first().map(|&it| self.string(it)).unwrap_or_default(),
            V::Str(s) => s.clone(),
            V::Num(x) if x.fract() == 0.0 && x.abs() < 1e15 => format!("{}", *x as i64),
            V::Num(x) => x.to_string(),
            V::Bool(b) => b.to_string(),
        }
    }

    fn num(&self, v: &V) -> f64 {
        match v {
            V::Num(x) => *x,
            V::Bool(b) => *b as u8 as f64,
            v => self.text(v).trim().parse().unwrap_or(f64::NAN),
        }
    }

    fn eval(&self, x: &X, ctx: It, pos: usize, size: usize) -> Option<V> {
        self.work.set(self.work.get().saturating_add(1));
        if self.work.get() > MAX_WORK {
            return None;
        }
        Some(match x {
            X::Str(s) => V::Str(s.clone()),
            X::Num(n) => V::Num(*n),
            X::Neg(a) => V::Num(-self.num(&self.eval(a, ctx, pos, size)?)),
            X::Path(abs, steps) => {
                let mut cur = vec![if *abs { It::N(0) } else { ctx }];
                for s in steps {
                    let mut next = Vec::new();
                    for &it in &cur {
                        next.extend(self.step(it, s)?);
                    }
                    next.sort_unstable();
                    next.dedup();
                    cur = next;
                }
                V::Nodes(cur)
            }
            X::Filter(a, preds) => match self.eval(a, ctx, pos, size)? {
                V::Nodes(v) => V::Nodes(self.filter(v, preds)?),
                _ => return None,
            },
            X::Union(a, b) => match (self.eval(a, ctx, pos, size)?, self.eval(b, ctx, pos, size)?) {
                (V::Nodes(mut a), V::Nodes(b)) => {
                    a.extend(b);
                    a.sort_unstable();
                    a.dedup();
                    V::Nodes(a)
                }
                _ => return None,
            },
            X::Bin(op, a, b) => {
                let a = self.eval(a, ctx, pos, size)?;
                match *op {
                    "or" => return Some(V::Bool(self.truth(&a) || self.truth(&self.eval(b, ctx, pos, size)?))),
                    "and" => return Some(V::Bool(self.truth(&a) && self.truth(&self.eval(b, ctx, pos, size)?))),
                    _ => {}
                }
                let b = self.eval(b, ctx, pos, size)?;
                match *op {
                    "+" => V::Num(self.num(&a) + self.num(&b)),
                    "-" => V::Num(self.num(&a) - self.num(&b)),
                    op => V::Bool(self.compare(op, &a, &b)?),
                }
            }
            X::Call(name, args) => {
                let arg = |i: usize| match args.get(i) {
                    Some(e) => self.eval(e, ctx, pos, size),
                    None => Some(V::Nodes(vec![ctx])),
                };
                let n = args.len();
                let ok = |min: usize, max: usize| (min..=max).contains(&n).then_some(());
                match name.as_str() {
                    "last" => ok(0, 0).map(|_| V::Num(size as f64))?,
                    "position" => ok(0, 0).map(|_| V::Num(pos as f64))?,
                    "true" | "false" => ok(0, 0).map(|_| V::Bool(name == "true"))?,
                    "count" => match (ok(1, 1), arg(0)?) {
                        (Some(_), V::Nodes(v)) => V::Num(v.len() as f64),
                        _ => return None,
                    },
                    "not" => V::Bool(!self.truth(&arg(ok(1, 1).map(|_| 0)?)?)),
                    "string" => V::Str(self.text(&arg(ok(0, 1).map(|_| 0)?)?)),
                    "number" => V::Num(self.num(&arg(ok(0, 1).map(|_| 0)?)?)),
                    "normalize-space" => V::Str(self.text(&arg(ok(0, 1).map(|_| 0)?)?).split_whitespace().collect::<Vec<_>>().join(" ")),
                    "string-length" => V::Num(self.text(&arg(ok(0, 1).map(|_| 0)?)?).chars().count() as f64),
                    "contains" | "starts-with" => {
                        ok(2, 2)?;
                        let (a, b) = (self.text(&arg(0)?), self.text(&arg(1)?));
                        V::Bool(if name == "contains" { a.contains(&b) } else { a.starts_with(&b) })
                    }
                    "name" | "local-name" => {
                        let it = match ok(0, 1).map(|_| arg(0))?? {
                            V::Nodes(v) => v.first().copied(),
                            _ => return None,
                        };
                        let q = match it {
                            Some(It::N(n)) => match &self.nodes[n].kind {
                                Kind::Elem(q, _) => q.clone(),
                                _ => String::new(),
                            },
                            Some(It::A(e, k)) => match &self.nodes[e].kind {
                                Kind::Elem(_, a) => a[k].0.clone(),
                                _ => String::new(),
                            },
                            None => String::new(),
                        };
                        let local = q.rsplit(':').next().unwrap_or_default().to_string();
                        V::Str(if name == "name" { q } else { local })
                    }
                    _ => return None,
                }
            }
        })
    }

    /// XPath comparison: node-sets compare through any of their nodes.
    /// None past the budget: sets compare every item with every other.
    fn compare(&self, op: &str, a: &V, b: &V) -> Option<bool> {
        let len = |v: &V| if let V::Nodes(n) = v { n.len() } else { 1 };
        self.work.set(self.work.get().saturating_add(len(a).saturating_mul(len(b))));
        if self.work.get() > MAX_WORK {
            return None;
        }
        let atoms = |v: &V| match v {
            V::Nodes(n) => n.iter().map(|&it| V::Str(self.string(it))).collect(),
            v => vec![v.clone()],
        };
        let (xs, ys) = (atoms(a), atoms(b));
        Some(xs.iter().any(|x| {
            ys.iter().any(|y| match op {
                "=" | "!=" => {
                    let eq = match (x, y) {
                        (V::Bool(_), _) | (_, V::Bool(_)) => self.truth(x) == self.truth(y),
                        (V::Num(_), _) | (_, V::Num(_)) => self.num(x) == self.num(y),
                        _ => self.text(x) == self.text(y),
                    };
                    eq == (op == "=")
                }
                _ => {
                    let (p, q) = (self.num(x), self.num(y));
                    match op {
                        "<" => p < q,
                        "<=" => p <= q,
                        ">" => p > q,
                        _ => p >= q,
                    }
                }
            })
        }))
    }
}

#[derive(Clone)]
enum V {
    Nodes(Vec<It>),
    Str(String),
    Num(f64),
    Bool(bool),
}

#[derive(Clone, Copy, PartialEq)]
enum Axis {
    Child,
    Desc,
    DescSelf,
    SelfAxis,
    Parent,
    Attr,
}

enum Test {
    Name(String),
    Any,
    Text,
    Node,
}

struct Step {
    axis: Axis,
    test: Test,
    preds: Vec<X>,
}

enum X {
    Path(bool, Vec<Step>),
    Str(String),
    Num(f64),
    Neg(Box<X>),
    Union(Box<X>, Box<X>),
    /// A parenthesized node-set with predicates: `(//b)[1]`.
    Filter(Box<X>, Vec<X>),
    Bin(&'static str, Box<X>, Box<X>),
    Call(String, Vec<X>),
}

#[derive(Clone, PartialEq, Debug)]
enum Tok {
    Slash,
    DSlash,
    LBr,
    RBr,
    LPar,
    RPar,
    At,
    Comma,
    Pipe,
    Dot,
    DDot,
    Star,
    Op(&'static str),
    Name(String),
    Str(String),
    Num(f64),
}

fn lex(s: &str) -> Option<Vec<Tok>> {
    let c: Vec<char> = s.chars().collect();
    let mut t = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let next = c.get(i + 1).copied();
        let (tok, len) = match c[i] {
            ch if ch.is_whitespace() => {
                i += 1;
                continue;
            }
            '/' if next == Some('/') => (Tok::DSlash, 2),
            '/' => (Tok::Slash, 1),
            '[' => (Tok::LBr, 1),
            ']' => (Tok::RBr, 1),
            '(' => (Tok::LPar, 1),
            ')' => (Tok::RPar, 1),
            '@' => (Tok::At, 1),
            ',' => (Tok::Comma, 1),
            '|' => (Tok::Pipe, 1),
            '*' => (Tok::Star, 1),
            '.' if next == Some('.') => (Tok::DDot, 2),
            '=' => (Tok::Op("="), 1),
            '!' if next == Some('=') => (Tok::Op("!="), 2),
            '<' if next == Some('=') => (Tok::Op("<="), 2),
            '>' if next == Some('=') => (Tok::Op(">="), 2),
            '<' => (Tok::Op("<"), 1),
            '>' => (Tok::Op(">"), 1),
            '+' => (Tok::Op("+"), 1),
            '-' => (Tok::Op("-"), 1),
            q @ ('"' | '\'') => {
                let end = c[i + 1..].iter().position(|&x| x == q)?;
                (Tok::Str(c[i + 1..i + 1 + end].iter().collect()), end + 2)
            }
            ch if ch.is_ascii_digit() || ch == '.' => {
                let n = c[i..].iter().take_while(|x| x.is_ascii_digit() || **x == '.').count();
                let s: String = c[i..i + n].iter().collect();
                if s == "." { (Tok::Dot, 1) } else { (Tok::Num(s.parse().ok()?), n) }
            }
            ch if ch.is_alphabetic() || ch == '_' => {
                let mut n = c[i..].iter().take_while(|x| x.is_alphanumeric() || matches!(x, '_' | '-' | '.' | ':')).count();
                // An axis ends at its "::".
                let word: String = c[i..i + n].iter().collect();
                if let Some(k) = word.find("::") {
                    n = word[..k + 2].chars().count();
                }
                (Tok::Name(c[i..i + n].iter().collect()), n)
            }
            _ => return None,
        };
        t.push(tok);
        i += len;
    }
    Some(t)
}

struct Parser {
    t: Vec<Tok>,
    i: usize,
    /// How many brackets and signs deep the parser is; past 200 the path
    /// is refused, as it would overflow the stack.
    depth: u32,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i)
    }

    fn eat(&mut self, t: &Tok) -> bool {
        let hit = self.peek() == Some(t);
        self.i += hit as usize;
        hit
    }

    fn all(mut self) -> Option<X> {
        let x = self.or()?;
        (self.i == self.t.len()).then_some(x)
    }

    fn word(&mut self, w: &str) -> bool {
        self.eat(&Tok::Name(w.to_string()))
    }

    fn or(&mut self) -> Option<X> {
        self.nested(Self::either)
    }

    /// `f` one level deeper.
    fn nested(&mut self, f: fn(&mut Self) -> Option<X>) -> Option<X> {
        if self.depth >= 200 {
            return None;
        }
        self.depth += 1;
        let x = f(self);
        self.depth -= 1;
        x
    }

    fn either(&mut self) -> Option<X> {
        let mut x = self.and()?;
        while self.word("or") {
            x = X::Bin("or", Box::new(x), Box::new(self.and()?));
        }
        Some(x)
    }

    fn and(&mut self) -> Option<X> {
        let mut x = self.cmp()?;
        while self.word("and") {
            x = X::Bin("and", Box::new(x), Box::new(self.cmp()?));
        }
        Some(x)
    }

    fn cmp(&mut self) -> Option<X> {
        let mut x = self.add()?;
        while let Some(Tok::Op(op @ ("=" | "!=" | "<" | "<=" | ">" | ">="))) = self.peek() {
            let op = *op;
            self.i += 1;
            x = X::Bin(op, Box::new(x), Box::new(self.add()?));
        }
        Some(x)
    }

    fn add(&mut self) -> Option<X> {
        let mut x = self.unary()?;
        while let Some(Tok::Op(op @ ("+" | "-"))) = self.peek() {
            let op = *op;
            self.i += 1;
            x = X::Bin(op, Box::new(x), Box::new(self.unary()?));
        }
        Some(x)
    }

    fn unary(&mut self) -> Option<X> {
        if self.eat(&Tok::Op("-")) {
            return Some(X::Neg(Box::new(self.nested(Self::unary)?)));
        }
        let mut x = self.primary()?;
        while self.eat(&Tok::Pipe) {
            x = X::Union(Box::new(x), Box::new(self.primary()?));
        }
        Some(x)
    }

    fn primary(&mut self) -> Option<X> {
        match self.peek()?.clone() {
            Tok::Str(s) => {
                self.i += 1;
                Some(X::Str(s))
            }
            Tok::Num(n) => {
                self.i += 1;
                Some(X::Num(n))
            }
            Tok::LPar => {
                self.i += 1;
                let x = self.or()?;
                self.eat(&Tok::RPar).then_some(())?;
                let mut preds = Vec::new();
                while self.eat(&Tok::LBr) {
                    preds.push(self.or()?);
                    self.eat(&Tok::RBr).then_some(())?;
                }
                Some(if preds.is_empty() { x } else { X::Filter(Box::new(x), preds) })
            }
            Tok::Name(n) if self.t.get(self.i + 1) == Some(&Tok::LPar) && !matches!(n.as_str(), "text" | "node") => {
                self.i += 2;
                let mut args = Vec::new();
                if !self.eat(&Tok::RPar) {
                    loop {
                        args.push(self.or()?);
                        if self.eat(&Tok::RPar) {
                            break;
                        }
                        self.eat(&Tok::Comma).then_some(())?;
                    }
                }
                Some(X::Call(n, args))
            }
            _ => self.path(),
        }
    }

    fn path(&mut self) -> Option<X> {
        let desc = || Step { axis: Axis::DescSelf, test: Test::Node, preds: Vec::new() };
        let mut steps = Vec::new();
        let abs = match self.peek() {
            Some(Tok::Slash) => {
                self.i += 1;
                if !matches!(self.peek(), Some(Tok::Name(_) | Tok::Star | Tok::At | Tok::Dot | Tok::DDot)) {
                    return Some(X::Path(true, steps));
                }
                true
            }
            Some(Tok::DSlash) => {
                self.i += 1;
                steps.push(desc());
                true
            }
            _ => false,
        };
        loop {
            steps.push(self.step()?);
            if self.eat(&Tok::DSlash) {
                steps.push(desc());
            } else if !self.eat(&Tok::Slash) {
                break;
            }
        }
        Some(X::Path(abs, steps))
    }

    fn step(&mut self) -> Option<Step> {
        let mut axis = Axis::Child;
        let test = match self.peek()?.clone() {
            Tok::Dot => {
                self.i += 1;
                return Some(Step { axis: Axis::SelfAxis, test: Test::Node, preds: Vec::new() });
            }
            Tok::DDot => {
                self.i += 1;
                return Some(Step { axis: Axis::Parent, test: Test::Node, preds: Vec::new() });
            }
            _ => {
                if self.eat(&Tok::At) {
                    axis = Axis::Attr;
                } else if let Some(Tok::Name(n)) = self.peek()
                    && let Some(a) = n.strip_suffix("::")
                {
                    axis = match a {
                        "child" => Axis::Child,
                        "descendant" => Axis::Desc,
                        "descendant-or-self" => Axis::DescSelf,
                        "self" => Axis::SelfAxis,
                        "parent" => Axis::Parent,
                        "attribute" => Axis::Attr,
                        _ => return None,
                    };
                    self.i += 1;
                }
                match self.peek()?.clone() {
                    Tok::Star => {
                        self.i += 1;
                        Test::Any
                    }
                    Tok::Name(n) if self.t.get(self.i + 1) == Some(&Tok::LPar) => {
                        self.i += 2;
                        self.eat(&Tok::RPar).then_some(())?;
                        match n.as_str() {
                            "text" => Test::Text,
                            "node" => Test::Node,
                            _ => return None,
                        }
                    }
                    Tok::Name(n) => {
                        self.i += 1;
                        Test::Name(n)
                    }
                    _ => return None,
                }
            }
        };
        let mut preds = Vec::new();
        while self.eat(&Tok::LBr) {
            preds.push(self.or()?);
            self.eat(&Tok::RBr).then_some(())?;
        }
        Some(Step { axis, test, preds })
    }
}

#[cfg(test)]
mod tests {
    use crate::Workbook;
    use crate::format::Locale;
    use crate::value::{ErrorKind, Matrix, Value};

    fn wb() -> Workbook {
        let mut wb = Workbook::new();
        wb.locale = Locale::EN;
        wb
    }

    fn ev(w: &Workbook, f: &str) -> Value {
        w.eval_formula(0, f)
    }

    fn t(s: &str) -> Value {
        Value::text(s)
    }

    fn col(v: &[Value]) -> Value {
        Value::Array(std::sync::Arc::new(Matrix::new(v.len(), 1, v.to_vec())))
    }

    #[test]
    fn encode_url() {
        let w = wb();
        assert_eq!(ev(&w, "=ENCODEURL(\"a b/ü~-_.*!\")"), t("a%20b%2F%C3%BC~-_.%2A%21"));
        assert_eq!(ev(&w, "=ENCODEURL(12.5)"), t("12.5"));
        assert_eq!(ev(&w, "=URLCODIF(\"http://x.com/?q=1&r=é\")"), t("http%3A%2F%2Fx.com%2F%3Fq%3D1%26r%3D%C3%A9"));
    }

    #[test]
    fn image_needs_the_network() {
        let w = wb();
        assert_eq!(ev(&w, "=IMAGE(\"https://x.com/a.png\",\"alt\",0)"), Value::Error(ErrorKind::Connect));
        assert_eq!(ev(&w, "=ERROR.TYPE(IMAGEN(\"https://x.com/a.png\"))"), Value::Number(10.0));
        assert_eq!(ev(&w, "=IMAGE(\"https://x.com/a.png\",,7)"), Value::Error(ErrorKind::Value));
        assert_eq!(ev(&w, "=IMAGE(\"\")"), Value::Error(ErrorKind::Value));
    }

    #[test]
    fn filter_xml() {
        let w = wb();
        let x = "\"<?xml version='1.0'?><!-- c --><r><b k='v'>1</b><b k='w'>x &amp; y</b><c><b>2.5</b><d>t</d></c></r>\"";
        let f = |p: &str| ev(&w, &format!("=FILTERXML({x},\"{p}\")"));
        assert_eq!(f("//b"), col(&[Value::Number(1.0), t("x & y"), Value::Number(2.5)]));
        assert_eq!(f("/r/b"), col(&[Value::Number(1.0), t("x & y")]));
        assert_eq!(f("r/b[2]"), t("x & y"));
        assert_eq!(f("//b[1]"), col(&[Value::Number(1.0), Value::Number(2.5)]));
        assert_eq!(f("(//b)[last()]"), Value::Number(2.5));
        assert_eq!(f("/r/b[last()]"), t("x & y"));
        assert_eq!(f("//b[@k='w']"), t("x & y"));
        assert_eq!(f("//b/@k"), col(&[t("v"), t("w")]));
        assert_eq!(f("//c[d='t']/b"), Value::Number(2.5));
        assert_eq!(f("/r/*[2]/text()"), t("x & y"));
        assert_eq!(f("/r/c/*"), col(&[Value::Number(2.5), t("t")]));
        assert_eq!(f("//b[@k]/@k"), col(&[t("v"), t("w")]));
        assert_eq!(f("//b[position()>1]"), t("x & y"));
        assert_eq!(f("count(//b)"), Value::Number(3.0));
        assert_eq!(f("//b[contains(.,'&')]"), t("x & y"));
        assert_eq!(f("//d/.."), t("2.5t"));
        assert_eq!(f("//z"), Value::Error(ErrorKind::Value));
        assert_eq!(f("//b["), Value::Error(ErrorKind::Value));
        assert_eq!(ev(&w, "=FILTERXML(\"<a><b></a>\",\"//b\")"), Value::Error(ErrorKind::Value));
        assert_eq!(ev(&w, "=FILTERXML(\"<a/><b/>\",\"//b\")"), Value::Error(ErrorKind::Value));
        // Deep documents and paths are refused, not a stack overflow.
        assert_eq!(ev(&w, "=FILTERXML(REPT(\"<a>\",3000)&REPT(\"</a>\",3000),\"/a\")"), Value::Error(ErrorKind::Value));
        assert_eq!(ev(&w, "=FILTERXML(\"<a>1</a>\",REPT(\"(\",3000)&\"/a\"&REPT(\")\",3000))"), Value::Error(ErrorKind::Value));
        assert_eq!(ev(&w, "=FILTERXML(\"<a>1</a>\",REPT(\"-\",3000)&\"1\")"), Value::Error(ErrorKind::Value));
        assert_eq!(ev(&w, "=FILTERXML(\"<a><![CDATA[<x>]]></a>\",\"/a\")"), t("<x>"));
        // Predicates over the whole document inside each other give up.
        assert_eq!(ev(&w, "=FILTERXML(\"<a>\"&REPT(\"<b/>\",5000)&\"</a>\",\"//*[//*[//*[//*]]]\")"), Value::Error(ErrorKind::Value));
        assert_eq!(ev(&w, "=FILTERXML(\"<r>\"&REPT(\"<a>1</a>\",5000)&REPT(\"<b>2</b>\",5000)&\"</r>\",\"//a[//a=//b]\")"), Value::Error(ErrorKind::Value));
    }

    #[test]
    fn info() {
        let mut w = wb();
        assert_eq!(ev(&w, "=INFO(\"numfile\")"), Value::Number(1.0));
        assert_eq!(ev(&w, "=INFO(\"origin\")"), t("$A:$A$1"));
        assert_eq!(ev(&w, "=INFO(\"recalc\")"), t("Automatic"));
        assert_eq!(ev(&w, "=INFO(\"directory\")"), Value::Error(ErrorKind::Na));
        assert_eq!(ev(&w, "=INFO(\"bad\")"), Value::Error(ErrorKind::Value));
        assert!(matches!(ev(&w, "=INFO(\"system\")"), Value::Text(_)));
        assert!(matches!(ev(&w, "=INFO(\"osversion\")"), Value::Text(_)));
        w.calc.manual = true;
        w.locale = Locale::ES;
        assert_eq!(ev(&w, "=INFO(\"recalc\")"), t("Manual"));
        w.calc.manual = false;
        assert_eq!(ev(&w, "=INFO(\"recalc\")"), t("Automático"));
    }
}
