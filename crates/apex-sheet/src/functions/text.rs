//! Spreadsheet functions: text.

use super::*;

use regex::{Regex, RegexBuilder};

pub(super) fn array_params(f: Func) -> Option<u64> {
    match f {
        // Delimiters may be arrays of alternatives.
        Func::TextBefore | Func::TextAfter => Some(0b10),
        Func::TextSplit => Some(0b111),
        Func::ArrayToText => Some(0b1),
        _ => None,
    }
}

pub(super) fn call(env: &Env, f: Func, a: &[Expr]) -> Option<R<Value>> {
    use Func::*;
    let mine = matches!(f, TextBefore | TextAfter | TextSplit | RegexTest | RegexExtract | RegexReplace | UniChar | Unicode | Clean | NumberValue | Fixed | Dollar | UsDollar | T | N | ValueToText | ArrayToText);
    mine.then(|| eval(env, f, a))
}

fn eval(env: &Env, f: Func, a: &[Expr]) -> R<Value> {
    use Func::*;
    let n = a.len();
    let need = |min: usize, max: usize| if n < min || n > max { Err(ErrorKind::Value) } else { Ok(()) };
    Ok(match f {
        TextBefore | TextAfter => {
            need(2, 6)?;
            let text = env.text(&a[0])?;
            let ci = flag(env, a, 3)?;
            let ds = delims(env, &a[1], ci)?;
            let k = env.opt_num(a, 2, 1.0)?.trunc();
            let at_end = env.opt_bool(a, 4, false)?;
            let s: Vec<char> = text.chars().collect();
            if k == 0.0 || k.abs() > s.len().max(1) as f64 {
                return Err(ErrorKind::Value);
            }
            match find_nth(&fold(&text, ci), &ds, k as i64, at_end) {
                Some((start, end)) => Value::text(if f == TextBefore { s[..start].iter().collect::<String>() } else { s[end..].iter().collect() }),
                None => match a.get(5) {
                    None | Some(Expr::Missing) => return Err(ErrorKind::Na),
                    Some(e) => env.eval(e),
                },
            }
        }
        TextSplit => {
            need(2, 6)?;
            let ci = flag(env, a, 4)?;
            let cols = match &a[1] {
                Expr::Missing => Vec::new(),
                e => delims(env, e, ci)?,
            };
            let rows = match a.get(2) {
                None | Some(Expr::Missing) => Vec::new(),
                Some(e) => delims(env, e, ci)?,
            };
            if (cols.is_empty() && rows.is_empty()) || cols.iter().chain(&rows).any(Vec::is_empty) {
                return Err(ErrorKind::Value);
            }
            let skip = env.opt_bool(a, 3, false)?;
            let pad = match a.get(5) {
                None | Some(Expr::Missing) => Value::Error(ErrorKind::Na),
                Some(e) => env.scalar(e),
            };
            let t = env.tab(&a[0])?;
            if t.len() == 1 {
                let text = t.get(0, 0).to_text()?;
                return split_matrix(&text, &cols, &rows, ci, skip, pad);
            }
            // Several texts: each cell keeps only the first piece, as
            // arrays can't nest.
            let data = (0..t.len())
                .map(|i| match t.nth(i).to_text() {
                    Ok(s) => match split_matrix(&s, &cols, &rows, ci, skip, pad.clone()) {
                        Ok(Value::Array(m)) => m.data[0].clone(),
                        Ok(v) => v,
                        Err(k) => Value::Error(k),
                    },
                    Err(k) => Value::Error(k),
                })
                .collect();
            Matrix::new(t.rows(), t.cols(), data).into_value()
        }
        RegexTest => {
            need(2, 3)?;
            let text = env.text(&a[0])?;
            let re = regex(&env.text(&a[1])?, flag(env, a, 2)?)?;
            Value::Bool(re.is_match(&text))
        }
        RegexExtract => {
            need(2, 4)?;
            let text = env.text(&a[0])?;
            let pat = env.text(&a[1])?;
            let mode = env.opt_num(a, 2, 0.0)?.trunc();
            let re = regex(&pat, flag(env, a, 3)?)?;
            match mode as i64 {
                0 => Value::text(re.find(&text).ok_or(ErrorKind::Na)?.as_str()),
                1 => {
                    let all: Vec<Value> = re.find_iter(&text).map(|m| Value::text(m.as_str())).collect();
                    if all.is_empty() {
                        return Err(ErrorKind::Na);
                    }
                    Matrix::new(all.len(), 1, all).into_value()
                }
                2 => {
                    let c = re.captures(&text).ok_or(ErrorKind::Na)?;
                    if c.len() < 2 {
                        return Err(ErrorKind::Na);
                    }
                    let groups: Vec<Value> = c.iter().skip(1).map(|g| Value::text(g.map_or("", |m| m.as_str()))).collect();
                    Matrix::new(1, groups.len(), groups).into_value()
                }
                _ => return Err(ErrorKind::Value),
            }
        }
        RegexReplace => {
            need(3, 5)?;
            let text = env.text(&a[0])?;
            let pat = env.text(&a[1])?;
            let rep = braced_groups(&env.text(&a[2])?);
            let occ = env.opt_num(a, 3, 0.0)?.trunc();
            let re = regex(&pat, flag(env, a, 4)?)?;
            if occ == 0.0 {
                // Checked before it's made: each match may bring the
                // replacement and, through each `$`, up to all the text.
                let worst = re.find_iter(&text).count().saturating_mul(rep.len()).saturating_add(text.len().saturating_mul(1 + rep.matches('$').count()));
                if worst > MAX_TEXT_BYTES {
                    return Err(ErrorKind::Value);
                }
                return Ok(Value::text(re.replace_all(&text, rep.as_str()).into_owned()));
            }
            let all: Vec<regex::Captures> = re.captures_iter(&text).collect();
            let i = if occ > 0.0 { occ - 1.0 } else { all.len() as f64 + occ };
            let Some(c) = (i >= 0.0).then(|| all.get(i as usize)).flatten() else { return Ok(Value::Text(text)) };
            let Some(m) = c.get(0) else { return Ok(Value::Text(text)) };
            let mut out = String::from(&text[..m.start()]);
            c.expand(&rep, &mut out);
            out.push_str(&text[m.end()..]);
            Value::text(out)
        }
        UniChar => {
            let x = env.num1(a)?.trunc();
            if !(1.0..=1_114_111.0).contains(&x) {
                return Err(ErrorKind::Value);
            }
            Value::text(char::from_u32(x as u32).ok_or(ErrorKind::Na)?.to_string())
        }
        Unicode => Value::Number(env.text1(a)?.chars().next().ok_or(ErrorKind::Value)? as u32 as f64),
        Clean => Value::text(env.text1(a)?.chars().filter(|&c| c as u32 >= 32).collect::<String>()),
        NumberValue => {
            need(1, 3)?;
            let text = match env.scalar(&a[0]) {
                Value::Number(x) => return Ok(Value::Number(x)),
                Value::Empty => return Ok(Value::Number(0.0)),
                Value::Text(t) => t,
                Value::Error(k) => return Err(k),
                _ => return Err(ErrorKind::Value),
            };
            let sep = |i: usize, d: char| -> R<char> {
                match a.get(i) {
                    None | Some(Expr::Missing) => Ok(d),
                    Some(e) => env.text(e)?.chars().next().ok_or(ErrorKind::Value),
                }
            };
            let (dec, group) = (sep(1, env.locale.decimal)?, sep(2, env.locale.thousands)?);
            num_ok(number_value(&text, dec, group).ok_or(ErrorKind::Value)?)
        }
        Fixed => {
            need(1, 3)?;
            let x = env.num(&a[0])?;
            let d = decimals(env, a)?;
            let plain = env.opt_bool(a, 2, false)?;
            let (neg, body) = fixed(x, d, env.locale.decimal, (!plain).then_some(env.locale.thousands));
            Value::text(if neg { format!("-{body}") } else { body })
        }
        Dollar => {
            need(1, 2)?;
            let x = env.num(&a[0])?;
            let d = decimals(env, a)?;
            let loc = &env.locale;
            let (neg, body) = fixed(x, d, loc.decimal, Some(loc.thousands));
            Value::text(match (loc.currency_after, neg) {
                (true, false) => format!("{body} {}", loc.currency),
                (true, true) => format!("-{body} {}", loc.currency),
                (false, false) => format!("{}{body}", loc.currency),
                (false, true) => format!("({}{body})", loc.currency),
            })
        }
        UsDollar => {
            need(1, 2)?;
            let (neg, body) = fixed(env.num(&a[0])?, decimals(env, a)?, '.', Some(','));
            Value::text(if neg { format!("(${body})") } else { format!("${body}") })
        }
        T => match env.eval1(a)? {
            Value::Text(t) => Value::Text(t),
            Value::Error(k) => return Err(k),
            _ => Value::text(""),
        },
        N => match env.eval1(a)? {
            Value::Number(x) => Value::Number(x),
            Value::Bool(b) => Value::Number(b as u8 as f64),
            Value::Error(k) => return Err(k),
            _ => Value::Number(0.0),
        },
        ValueToText => {
            need(1, 2)?;
            let v = env.scalar(&a[0]);
            let strict = flag(env, a, 1)?;
            Value::text(show(&v, strict, &env.locale))
        }
        ArrayToText => {
            need(1, 2)?;
            let t = env.tab(&a[0])?;
            let strict = flag(env, a, 1)?;
            let es = env.locale.decimal == ',';
            let cell = |r: usize, c: usize| show(&t.get(r, c), strict, &env.locale);
            // Too long a result is #VALUE!: before building it.
            let mut bytes = 0usize;
            for i in 0..t.len() {
                bytes += cell(i / t.cols(), i % t.cols()).len() + 2;
                if bytes > crate::functions::MAX_TEXT_BYTES {
                    return Err(ErrorKind::Value);
                }
            }
            if strict {
                let (col, row) = if es { ("\\", ";") } else { (",", ";") };
                let rows: Vec<String> = (0..t.rows()).map(|r| (0..t.cols()).map(|c| cell(r, c)).collect::<Vec<_>>().join(col)).collect();
                Value::text(format!("{{{}}}", rows.join(row)))
            } else {
                let all: Vec<String> = (0..t.len()).map(|i| cell(i / t.cols(), i % t.cols())).collect();
                Value::text(all.join(if es { "; " } else { ", " }))
            }
        }
        _ => return Err(ErrorKind::Name),
    })
}

/// An optional 0/1 argument (match mode, case sensitivity, strict format).
fn flag(env: &Env, a: &[Expr], i: usize) -> R<bool> {
    let x = env.opt_num(a, i, 0.0)?.trunc();
    if x == 0.0 || x == 1.0 { Ok(x == 1.0) } else { Err(ErrorKind::Value) }
}

/// Lowercases char by char so positions match the original text.
fn fold(s: &str, ci: bool) -> Vec<char> {
    s.chars()
        .map(|c| {
            if !ci {
                return c;
            }
            let mut l = c.to_lowercase();
            match (l.next(), l.next()) {
                (Some(x), None) => x,
                _ => c,
            }
        })
        .collect()
}

/// A delimiter argument: one text or an array of alternatives.
fn delims(env: &Env, e: &Expr, ci: bool) -> R<Vec<Vec<char>>> {
    let t = env.tab(e)?;
    (0..t.len()).map(|i| Ok(fold(&t.nth(i).to_text()?, ci))).collect()
}

/// Length of the longest delimiter starting at `i`.
fn match_at(h: &[char], i: usize, ds: &[Vec<char>]) -> Option<usize> {
    ds.iter().filter(|d| !d.is_empty() && h[i..].starts_with(d)).map(Vec::len).max()
}

/// Span of the `k`-th delimiter (from the end when negative). With
/// `at_end`, the end (or start) of the text counts as one more delimiter.
fn find_nth(h: &[char], ds: &[Vec<char>], k: i64, at_end: bool) -> Option<(usize, usize)> {
    let len = h.len();
    // An empty delimiter matches right away.
    if ds.iter().any(Vec::is_empty) {
        return Some(if k > 0 { (0, 0) } else { (len, len) });
    }
    let want = k.unsigned_abs();
    let mut count = 0;
    if k > 0 {
        let mut i = 0;
        while i < len {
            match match_at(h, i, ds) {
                Some(l) => {
                    count += 1;
                    if count == want {
                        return Some((i, i + l));
                    }
                    i += l;
                }
                None => i += 1,
            }
        }
        (at_end && count + 1 == want).then_some((len, len))
    } else {
        let mut e = len;
        while e > 0 {
            match ds.iter().filter(|d| h[..e].ends_with(d)).map(Vec::len).max() {
                Some(l) => {
                    count += 1;
                    if count == want {
                        return Some((e - l, e));
                    }
                    e -= l;
                }
                None => e -= 1,
            }
        }
        (at_end && count + 1 == want).then_some((0, 0))
    }
}

/// Pieces of `h[from..to]` between delimiters, as spans.
fn split(h: &[char], from: usize, to: usize, ds: &[Vec<char>]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let (mut start, mut i) = (from, from);
    while i < to {
        match match_at(&h[..to], i, ds) {
            Some(l) => {
                out.push((start, i));
                i += l;
                start = i;
            }
            None => i += 1,
        }
    }
    out.push((start, to));
    out
}

fn split_matrix(text: &str, cols: &[Vec<char>], rows: &[Vec<char>], ci: bool, skip: bool, pad: Value) -> R<Value> {
    let s: Vec<char> = text.chars().collect();
    let h = fold(text, ci);
    let mut grid: Vec<Vec<(usize, usize)>> = Vec::new();
    for (a, b) in split(&h, 0, h.len(), rows) {
        let mut line = split(&h, a, b, cols);
        if skip {
            line.retain(|(x, y)| x < y);
            if line.is_empty() {
                continue;
            }
        }
        grid.push(line);
    }
    let width = grid.iter().map(Vec::len).max().unwrap_or(0);
    if grid.is_empty() || width == 0 {
        return Err(ErrorKind::Calc);
    }
    if grid.len() * width > MAX_ARRAY {
        return Err(ErrorKind::Num);
    }
    let mut data = Vec::with_capacity(grid.len() * width);
    for line in &grid {
        data.extend(line.iter().map(|&(x, y)| Value::text(s[x..y].iter().collect::<String>())));
        data.extend(std::iter::repeat_n(pad.clone(), width - line.len()));
    }
    Ok(Matrix::new(grid.len(), width, data).into_value())
}

thread_local! {
    /// The last compiled pattern: the same one repeats over whole arrays.
    static LAST_RE: std::cell::RefCell<Option<(String, bool, Regex)>> = const { std::cell::RefCell::new(None) };
}

fn regex(pat: &str, ci: bool) -> R<Regex> {
    LAST_RE.with(|c| {
        if let Some((p, i, re)) = &*c.borrow() {
            if p == pat && *i == ci {
                return Ok(re.clone());
            }
        }
        let re = RegexBuilder::new(pat).case_insensitive(ci).build().map_err(|_| ErrorKind::Value)?;
        *c.borrow_mut() = Some((pat.to_string(), ci, re.clone()));
        Ok(re)
    })
}

/// `$12` means group 12 followed by nothing else (as in PCRE); the regex
/// crate would read `$1a` as a group named "1a".
fn braced_groups(rep: &str) -> String {
    let mut out = String::with_capacity(rep.len());
    let mut it = rep.chars().peekable();
    while let Some(c) = it.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match it.peek() {
            Some('$') => {
                it.next();
                out.push_str("$$");
            }
            Some(d) if d.is_ascii_digit() => {
                out.push_str("${");
                while let Some(d) = it.next_if(char::is_ascii_digit) {
                    out.push(d);
                }
                out.push('}');
            }
            _ => out.push('$'),
        }
    }
    out
}

/// NUMBERVALUE parsing: spaces are ignored, group separators only count
/// before the decimal one, trailing `%` signs divide by 100 each.
fn number_value(s: &str, dec: char, group: char) -> Option<f64> {
    let mut t: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if s.is_empty() {
        return Some(0.0);
    }
    let mut div = 1.0;
    while let Some(r) = t.strip_suffix('%') {
        t.truncate(r.len());
        div *= 100.0;
    }
    let mut out = String::with_capacity(t.len());
    let mut seen_dec = false;
    for c in t.chars() {
        if c == dec {
            if seen_dec {
                return None;
            }
            seen_dec = true;
            out.push('.');
        } else if c == group {
            if seen_dec {
                return None;
            }
        } else if c.is_ascii_digit() || matches!(c, '+' | '-' | 'e' | 'E') {
            out.push(c);
        } else {
            return None;
        }
    }
    out.parse::<f64>().ok().map(|x| x / div)
}

/// The `decimals` argument of FIXED and DOLLAR.
fn decimals(env: &Env, a: &[Expr]) -> R<i32> {
    let d = env.opt_num(a, 1, 2.0)?.trunc();
    if d > 127.0 {
        return Err(ErrorKind::Value);
    }
    Ok(d.max(-400.0) as i32)
}

/// ROUND semantics (half away from zero), also for negative `d`.
fn round_half(x: f64, d: i32) -> f64 {
    // Nudge to counter binary representation (2.675 -> 2.68).
    let half = |y: f64| {
        let t = y.trunc();
        if (y - t).abs() >= 0.5 - 1e-9 { t + y.signum() } else { t }
    };
    if d >= 0 {
        let p = 10f64.powi(d);
        let y = x * p;
        if y.is_finite() { half(y) / p } else { x }
    } else {
        let p = 10f64.powi(-d);
        if p.is_finite() { half(x / p) * p } else { 0.0 }
    }
}

/// `x` rounded to `d` decimals with separators; returns (negative, digits).
fn fixed(x: f64, d: i32, dec: char, group: Option<char>) -> (bool, String) {
    let r = round_half(x, d);
    let places = d.max(0) as usize;
    let a = r.abs();
    // Beyond 15 significant digits Excel shows zeros.
    let s = if a >= 1e15 {
        let e = format!("{a:.14e}");
        let (mant, exp) = e.split_once('e').unwrap_or((&e, "0"));
        let digits: String = mant.chars().filter(char::is_ascii_digit).collect();
        let int_len = exp.parse::<usize>().unwrap_or(0) + 1;
        let int: String = digits.chars().chain(std::iter::repeat('0')).take(int_len).collect();
        if places > 0 { format!("{int}.{}", "0".repeat(places)) } else { int }
    } else {
        format!("{a:.places$}")
    };
    let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            if let Some(g) = group {
                out.push(g);
            }
        }
        out.push(c);
    }
    if !frac.is_empty() {
        out.push(dec);
        out.push_str(frac);
    }
    (r < 0.0, out)
}

/// A value as VALUETOTEXT/ARRAYTOTEXT write it; `strict` quotes text.
fn show(v: &Value, strict: bool, loc: &Locale) -> String {
    match v {
        Value::Empty => String::new(),
        Value::Number(x) => format::general_number(*x, loc.decimal),
        Value::Text(t) if strict => format!("\"{}\"", t.replace('"', "\"\"")),
        Value::Text(t) => t.to_string(),
        Value::Bool(_) => NumFormat::general().format(v, loc).0,
        Value::Error(k) => k.as_str().to_string(),
        Value::Array(_) | Value::Lambda(_) => ErrorKind::Value.as_str().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use crate::format::Locale;
    use crate::value::{ErrorKind, Matrix, Value};
    use crate::{CellRef, Workbook};

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

    fn err(k: ErrorKind) -> Value {
        Value::Error(k)
    }

    fn arr(rows: usize, cols: usize, v: &[Value]) -> Value {
        Value::Array(std::sync::Arc::new(Matrix::new(rows, cols, v.to_vec())))
    }

    #[test]
    fn text_before_after() {
        let w = wb();
        let s = "\"Little Red Riding Hood's red hood\"";
        assert_eq!(ev(&w, &format!("=TEXTBEFORE({s},\"Red\")")), t("Little "));
        assert_eq!(ev(&w, &format!("=TEXTBEFORE({s},\"red\",2)")), err(ErrorKind::Na));
        assert_eq!(ev(&w, &format!("=TEXTBEFORE({s},\"red\",-2,1)")), t("Little "));
        assert_eq!(ev(&w, &format!("=TEXTBEFORE({s},\"Red\",,1)")), t("Little "));
        assert_eq!(ev(&w, &format!("=TEXTBEFORE({s},\"red\",2,1)")), t("Little Red Riding Hood's "));
        assert_eq!(ev(&w, &format!("=TEXTAFTER({s},\"hood\")")), t(""));
        assert_eq!(ev(&w, &format!("=TEXTAFTER({s},\"hood\",1,1)")), t("'s red hood"));
        assert_eq!(ev(&w, &format!("=TEXTAFTER({s},\"red\",-1)")), t(" hood"));
        assert_eq!(ev(&w, &format!("=TEXTAFTER({s},\"basket\")")), err(ErrorKind::Na));
        assert_eq!(ev(&w, &format!("=TEXTAFTER({s},\"basket\",,,,\"none\")")), t("none"));
        assert_eq!(ev(&w, &format!("=TEXTBEFORE({s},\"red\",0)")), err(ErrorKind::Value));
        assert_eq!(ev(&w, &format!("=TEXTBEFORE({s},\"red\",99)")), err(ErrorKind::Value));
        assert_eq!(ev(&w, &format!("=TEXTBEFORE({s},\"red\",3,1)")), err(ErrorKind::Na));
        // match_end: the end of the text is one more delimiter.
        assert_eq!(ev(&w, &format!("=TEXTBEFORE({s},\"red\",3,1,1)")), t("Little Red Riding Hood's red hood"));
        assert_eq!(ev(&w, &format!("=TEXTAFTER({s},\"red\",3,1,1)")), t(""));
        assert_eq!(ev(&w, "=TEXTAFTER(\"a-b\",\"x\",-1,,1)"), t("a-b"));
        // Several delimiters, empty delimiter.
        assert_eq!(ev(&w, "=TEXTBEFORE(\"a;b,c\",{\",\",\";\"})"), t("a"));
        assert_eq!(ev(&w, "=TEXTAFTER(\"a;b,c\",{\",\",\";\"},2)"), t("c"));
        assert_eq!(ev(&w, "=TEXTBEFORE(\"abc\",\"\")"), t(""));
        assert_eq!(ev(&w, "=TEXTAFTER(\"abc\",\"\")"), t("abc"));
        // Arrays in the text argument repeat the call.
        assert_eq!(ev(&w, "=TEXTBEFORE({\"a-b\",\"c-d\"},\"-\")"), arr(1, 2, &[t("a"), t("c")]));
    }

    #[test]
    fn text_split() {
        let mut w = wb();
        assert_eq!(ev(&w, "=TEXTSPLIT(\"Dakota Lennon Sanchez\",\" \")"), arr(1, 3, &[t("Dakota"), t("Lennon"), t("Sanchez")]));
        assert_eq!(ev(&w, "=TEXTSPLIT(\"1,2,3;4,5,6\",\",\",\";\")"), arr(2, 3, &[t("1"), t("2"), t("3"), t("4"), t("5"), t("6")]));
        let quote = "\"Do. Or do not. There is no try. -Anonymous\"";
        assert_eq!(ev(&w, &format!("=TEXTSPLIT({quote},,\".\")")), arr(4, 1, &[t("Do"), t(" Or do not"), t(" There is no try"), t(" -Anonymous")]));
        assert_eq!(ev(&w, &format!("=TEXTSPLIT({quote},,{{\".\",\"-\"}},TRUE)")), arr(5, 1, &[t("Do"), t(" Or do not"), t(" There is no try"), t(" "), t("Anonymous")]));
        assert_eq!(ev(&w, "=TEXTSPLIT(\"a,b;c\",\",\",\";\")"), arr(2, 2, &[t("a"), t("b"), t("c"), err(ErrorKind::Na)]));
        assert_eq!(ev(&w, "=TEXTSPLIT(\"a,b;c\",\",\",\";\",,,\"-\")"), arr(2, 2, &[t("a"), t("b"), t("c"), t("-")]));
        assert_eq!(ev(&w, "=TEXTSPLIT(\"a,,b\",\",\")"), arr(1, 3, &[t("a"), t(""), t("b")]));
        assert_eq!(ev(&w, "=TEXTSPLIT(\"a,,b\",\",\",,TRUE)"), arr(1, 2, &[t("a"), t("b")]));
        assert_eq!(ev(&w, "=TEXTSPLIT(\"a;;b\",\",\",\";\",TRUE)"), arr(2, 1, &[t("a"), t("b")]));
        assert_eq!(ev(&w, "=TEXTSPLIT(\"aXbxc\",\"x\",,,1)"), arr(1, 3, &[t("a"), t("b"), t("c")]));
        assert_eq!(ev(&w, "=TEXTSPLIT(\"aXbxc\",\"x\")"), arr(1, 2, &[t("aXb"), t("c")]));
        assert_eq!(ev(&w, "=TEXTSPLIT(\"abc\",\"\")"), err(ErrorKind::Value));
        assert_eq!(ev(&w, "=TEXTSPLIT(\",\",\",\",,TRUE)"), err(ErrorKind::Calc));
        w.set_input(0, CellRef::new(0, 0), "x-y");
        w.set_input(0, CellRef::new(1, 0), "z-w");
        assert_eq!(ev(&w, "=TEXTSPLIT(A1:A2,\"-\")"), arr(2, 1, &[t("x"), t("z")]));
    }

    #[test]
    fn regex_functions() {
        let w = wb();
        assert_eq!(ev(&w, "=REGEXTEST(\"alpha\",\"[a-z]+\")"), Value::Bool(true));
        assert_eq!(ev(&w, "=REGEXTEST(\"ALPHA\",\"^[a-z]+$\")"), Value::Bool(false));
        assert_eq!(ev(&w, "=REGEXTEST(\"ALPHA\",\"^[a-z]+$\",1)"), Value::Bool(true));
        assert_eq!(ev(&w, "=REGEXTEST(\"a\",\"(\")"), err(ErrorKind::Value));
        let s = "\"DylanWilliams\"";
        assert_eq!(ev(&w, &format!("=REGEXEXTRACT({s},\"[A-Z][a-z]+\")")), t("Dylan"));
        assert_eq!(ev(&w, &format!("=REGEXEXTRACT({s},\"[A-Z][a-z]+\",1)")), arr(2, 1, &[t("Dylan"), t("Williams")]));
        assert_eq!(ev(&w, &format!("=REGEXEXTRACT({s},\"([A-Z][a-z]+)([A-Z][a-z]+)\",2)")), arr(1, 2, &[t("Dylan"), t("Williams")]));
        assert_eq!(ev(&w, &format!("=REGEXEXTRACT({s},\"[0-9]\")")), err(ErrorKind::Na));
        assert_eq!(ev(&w, &format!("=REGEXEXTRACT({s},\"dylan\",0,1)")), t("Dylan"));
        assert_eq!(ev(&w, &format!("=REGEXEXTRACT({s},\"x\",3)")), err(ErrorKind::Value));
        let p = "\"(\\d{3})-(\\d{3})-(\\d{4})\"";
        assert_eq!(ev(&w, &format!("=REGEXREPLACE(\"Phone: 425-555-2345\",{p},\"***-***-$3\")")), t("Phone: ***-***-2345"));
        assert_eq!(ev(&w, "=REGEXREPLACE(\"a1b2c3\",\"[0-9]\",\"#\")"), t("a#b#c#"));
        assert_eq!(ev(&w, "=REGEXREPLACE(\"a1b2c3\",\"[0-9]\",\"#\",2)"), t("a1b#c3"));
        assert_eq!(ev(&w, "=REGEXREPLACE(\"a1b2c3\",\"[0-9]\",\"#\",-1)"), t("a1b2c#"));
        assert_eq!(ev(&w, "=REGEXREPLACE(\"a1b2c3\",\"[0-9]\",\"#\",9)"), t("a1b2c3"));
        assert_eq!(ev(&w, "=REGEXREPLACE(\"AbAB\",\"ab\",\"x\",0,1)"), t("xx"));
        assert_eq!(ev(&w, "=REGEXREPLACE(\"ab\",\"(a)\",\"$1x\")"), t("axb"));
    }

    #[test]
    fn unicode_clean() {
        let w = wb();
        assert_eq!(ev(&w, "=UNICHAR(66)"), t("B"));
        assert_eq!(ev(&w, "=UNICHAR(32)"), t(" "));
        assert_eq!(ev(&w, "=UNICHAR(0)"), err(ErrorKind::Value));
        assert_eq!(ev(&w, "=UNICHAR(55296)"), err(ErrorKind::Na));
        assert_eq!(ev(&w, "=UNICODE(\"B\")"), Value::Number(66.0));
        assert_eq!(ev(&w, "=UNICODE(\"€uro\")"), Value::Number(8364.0));
        assert_eq!(ev(&w, "=UNICODE(\"\")"), err(ErrorKind::Value));
        assert_eq!(ev(&w, "=CLEAN(CHAR(9)&\"Monthly report\"&CHAR(10))"), t("Monthly report"));
    }

    #[test]
    fn number_value() {
        let w = wb();
        assert_eq!(ev(&w, "=NUMBERVALUE(\"2.500,27\",\",\",\".\")"), Value::Number(2500.27));
        assert_eq!(ev(&w, "=NUMBERVALUE(\"3.5%\")"), Value::Number(0.035));
        assert_eq!(ev(&w, "=NUMBERVALUE(\" 1 234.5 \")"), Value::Number(1234.5));
        assert_eq!(ev(&w, "=NUMBERVALUE(\"9%%\")"), Value::Number(0.0009));
        assert_eq!(ev(&w, "=NUMBERVALUE(\"\")"), Value::Number(0.0));
        assert_eq!(ev(&w, "=NUMBERVALUE(\"1.2.3\")"), err(ErrorKind::Value));
        assert_eq!(ev(&w, "=NUMBERVALUE(\"1.2,3\")"), err(ErrorKind::Value));
        assert_eq!(ev(&w, "=NUMBERVALUE(\"abc\")"), err(ErrorKind::Value));
        assert_eq!(ev(&w, "=NUMBERVALUE(\"1,5\",\",\")"), Value::Number(1.5));
    }

    #[test]
    fn fixed_dollar() {
        let mut w = wb();
        assert_eq!(ev(&w, "=FIXED(1234.567,1)"), t("1,234.6"));
        assert_eq!(ev(&w, "=FIXED(1234.567,-1)"), t("1,230"));
        assert_eq!(ev(&w, "=FIXED(-1234.567,-1,TRUE)"), t("-1230"));
        assert_eq!(ev(&w, "=FIXED(44.332)"), t("44.33"));
        assert_eq!(ev(&w, "=FIXED(2.675,2)"), t("2.68"));
        assert_eq!(ev(&w, "=FIXED(-0.001,2)"), t("0.00"));
        assert_eq!(ev(&w, "=FIXED(1,128)"), err(ErrorKind::Value));
        assert_eq!(ev(&w, "=FIXED(123456789012345678,0)"), t("123,456,789,012,346,000"));
        assert_eq!(ev(&w, "=DOLLAR(1234.567,2)"), t("$1,234.57"));
        assert_eq!(ev(&w, "=DOLLAR(1234.567,-2)"), t("$1,200"));
        assert_eq!(ev(&w, "=DOLLAR(-1234.567,-2)"), t("($1,200)"));
        assert_eq!(ev(&w, "=DOLLAR(-0.123,4)"), t("($0.1230)"));
        assert_eq!(ev(&w, "=DOLLAR(99.888)"), t("$99.89"));
        w.locale = Locale::ES;
        assert_eq!(ev(&w, "=USDOLLAR(-1234,567;1)"), t("($1,234.6)"));
        assert_eq!(ev(&w, "=DOLLAR(1234,567)"), t("1.234,57 €"));
        assert_eq!(ev(&w, "=DOLLAR(-1234,567;0)"), t("-1.235 €"));
        assert_eq!(ev(&w, "=FIXED(1234,567;1)"), t("1.234,6"));
    }

    #[test]
    fn t_n_value_to_text() {
        let mut w = wb();
        w.set_input(0, CellRef::new(0, 0), "Rainfall");
        w.set_input(0, CellRef::new(1, 0), "19");
        w.set_input(0, CellRef::new(2, 0), "TRUE");
        assert_eq!(ev(&w, "=T(A1)"), t("Rainfall"));
        assert_eq!(ev(&w, "=T(A2)"), t(""));
        assert_eq!(ev(&w, "=T(A3)"), t(""));
        assert_eq!(ev(&w, "=T(NA())"), err(ErrorKind::Na));
        assert_eq!(ev(&w, "=N(A2)"), Value::Number(19.0));
        assert_eq!(ev(&w, "=N(A1)"), Value::Number(0.0));
        assert_eq!(ev(&w, "=N(A3)"), Value::Number(1.0));
        assert_eq!(ev(&w, "=N(\"7\")"), Value::Number(0.0));
        assert_eq!(ev(&w, "=VALUETOTEXT(A1)"), t("Rainfall"));
        assert_eq!(ev(&w, "=VALUETOTEXT(A1,1)"), t("\"Rainfall\""));
        assert_eq!(ev(&w, "=VALUETOTEXT(A2,1)"), t("19"));
        assert_eq!(ev(&w, "=VALUETOTEXT(A3)"), t("TRUE"));
        assert_eq!(ev(&w, "=VALUETOTEXT(NA())"), t("#N/A"));
        assert_eq!(ev(&w, "=VALUETOTEXT(1.5,2)"), err(ErrorKind::Value));
        w.set_input(0, CellRef::new(0, 1), "1234.01234");
        w.set_input(0, CellRef::new(1, 1), "Seattle");
        assert_eq!(ev(&w, "=ARRAYTOTEXT(A1:B2)"), t("Rainfall, 1234.01234, 19, Seattle"));
        assert_eq!(ev(&w, "=ARRAYTOTEXT(A1:B2,1)"), t("{\"Rainfall\",1234.01234;19,\"Seattle\"}"));
        assert_eq!(ev(&w, "=ARRAYTOTEXT({1,\"a\";TRUE,#N/A},1)"), t("{1,\"a\";TRUE,#N/A}"));
        assert_eq!(ev(&w, "=ARRAYTOTEXT(A1:A3,2)"), err(ErrorKind::Value));
    }
}
