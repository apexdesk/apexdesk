//! JSON: a small parser, and records flattened into a table. Nested
//! objects become `parent.child` columns; lists of plain values are
//! joined with ", "; other lists stay as JSON text.

use crate::table::{DataTable, Val, unique_names};
use crate::Error;

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    /// Keys in their order.
    Obj(Vec<(String, Json)>),
}

struct P<'a> {
    b: &'a [u8],
    s: &'a str,
    i: usize,
    depth: usize,
}

impl P<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && self.b[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn err(&self, what: &str) -> Error {
        Error::Parse(format!("JSON: {what} at byte {}", self.i))
    }

    fn value(&mut self) -> Result<Json, Error> {
        self.ws();
        self.depth += 1;
        if self.depth > 500 {
            return Err(self.err("too deep"));
        }
        let v = match self.b.get(self.i) {
            None => return Err(self.err("unexpected end")),
            Some(b'{') => {
                self.i += 1;
                let mut out = Vec::new();
                self.ws();
                if self.b.get(self.i) == Some(&b'}') {
                    self.i += 1;
                } else {
                    loop {
                        self.ws();
                        if self.b.get(self.i) != Some(&b'"') {
                            return Err(self.err("expected a key"));
                        }
                        let k = self.string()?;
                        self.ws();
                        if self.b.get(self.i) != Some(&b':') {
                            return Err(self.err("expected :"));
                        }
                        self.i += 1;
                        let v = self.value()?;
                        out.push((k, v));
                        self.ws();
                        match self.b.get(self.i) {
                            Some(b',') => self.i += 1,
                            Some(b'}') => {
                                self.i += 1;
                                break;
                            }
                            _ => return Err(self.err("expected , or }")),
                        }
                    }
                }
                Json::Obj(out)
            }
            Some(b'[') => {
                self.i += 1;
                let mut out = Vec::new();
                self.ws();
                if self.b.get(self.i) == Some(&b']') {
                    self.i += 1;
                } else {
                    loop {
                        out.push(self.value()?);
                        self.ws();
                        match self.b.get(self.i) {
                            Some(b',') => self.i += 1,
                            Some(b']') => {
                                self.i += 1;
                                break;
                            }
                            _ => return Err(self.err("expected , or ]")),
                        }
                    }
                }
                Json::Arr(out)
            }
            Some(b'"') => Json::Str(self.string()?),
            Some(b't') if self.s[self.i..].starts_with("true") => {
                self.i += 4;
                Json::Bool(true)
            }
            Some(b'f') if self.s[self.i..].starts_with("false") => {
                self.i += 5;
                Json::Bool(false)
            }
            Some(b'n') if self.s[self.i..].starts_with("null") => {
                self.i += 4;
                Json::Null
            }
            Some(c) if c.is_ascii_digit() || *c == b'-' => {
                let start = self.i;
                while self.i < self.b.len() && matches!(self.b[self.i], b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E') {
                    self.i += 1;
                }
                Json::Num(self.s[start..self.i].parse().map_err(|_| self.err("bad number"))?)
            }
            Some(_) => return Err(self.err("unexpected character")),
        };
        self.depth -= 1;
        Ok(v)
    }

    fn string(&mut self) -> Result<String, Error> {
        self.i += 1;
        let mut out = String::new();
        loop {
            let start = self.i;
            while self.i < self.b.len() && self.b[self.i] != b'"' && self.b[self.i] != b'\\' {
                self.i += 1;
            }
            out.push_str(&self.s[start..self.i.min(self.s.len())]);
            match self.b.get(self.i) {
                None => return Err(self.err("unterminated string")),
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                _ => {
                    let c = *self.b.get(self.i + 1).ok_or_else(|| self.err("bad escape"))?;
                    self.i += 2;
                    match c {
                        b'n' => out.push('\n'),
                        b't' => out.push('\t'),
                        b'r' => out.push('\r'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'u' => {
                            let hex = |p: &mut Self| -> Result<u32, Error> {
                                let h = p.s.get(p.i..p.i + 4).ok_or_else(|| p.err("bad \\u"))?;
                                p.i += 4;
                                u32::from_str_radix(h, 16).map_err(|_| p.err("bad \\u"))
                            };
                            let mut cp = hex(self)?;
                            // A surrogate pair.
                            if (0xD800..0xDC00).contains(&cp) && self.s[self.i..].starts_with("\\u") {
                                self.i += 2;
                                let lo = hex(self)?;
                                cp = 0x10000 + ((cp - 0xD800) << 10) + (lo.wrapping_sub(0xDC00) & 0x3FF);
                            }
                            out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
                        }
                        c if c.is_ascii() => out.push(c as char),
                        // A whole character escaped: kept as it is.
                        _ => {
                            let ch = self.s[self.i - 1..].chars().next().unwrap_or('\u{fffd}');
                            out.push(ch);
                            self.i += ch.len_utf8() - 1;
                        }
                    }
                }
            }
        }
    }
}

pub fn parse(s: &str) -> Result<Json, Error> {
    let s = s.strip_prefix('\u{feff}').unwrap_or(s);
    let mut p = P { b: s.as_bytes(), s, i: 0, depth: 0 };
    let v = p.value()?;
    p.ws();
    if p.i < p.b.len() {
        // JSON Lines: one value per line.
        let mut all = vec![v];
        while p.i < p.b.len() {
            all.push(p.value()?);
            p.ws();
        }
        return Ok(Json::Arr(all));
    }
    Ok(v)
}

/// JSON text of a value.
pub fn to_text(v: &Json) -> String {
    fn esc(s: &str, out: &mut String) {
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('"');
    }
    fn go(v: &Json, out: &mut String) {
        match v {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => out.push_str(&crate::table::fmt_num(*n)),
            Json::Str(s) => esc(s, out),
            Json::Arr(a) => {
                out.push('[');
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    go(x, out);
                }
                out.push(']');
            }
            Json::Obj(o) => {
                out.push('{');
                for (i, (k, x)) in o.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    esc(k, out);
                    out.push(':');
                    go(x, out);
                }
                out.push('}');
            }
        }
    }
    let mut out = String::new();
    go(v, &mut out);
    out
}

/// Walks `path` ("data.items", "results.0.rows") down from `v`.
fn walk<'a>(v: &'a Json, path: &str) -> Result<&'a Json, Error> {
    let mut cur = v;
    for part in path.split(['.', '/']).map(str::trim).filter(|p| !p.is_empty()) {
        cur = match cur {
            Json::Obj(o) => o.iter().find(|(k, _)| k == part).map(|(_, x)| x),
            Json::Arr(a) => part.parse::<usize>().ok().and_then(|i| a.get(i)),
            _ => None,
        }
        .ok_or_else(|| Error::Parse(format!("JSON: no \"{part}\" in the path {path}")))?;
    }
    Ok(cur)
}

/// The biggest list of objects near the top of `v` (an API's records).
fn find_records(v: &Json) -> &Json {
    fn best<'a>(v: &'a Json, depth: usize, out: &mut Option<(usize, &'a Json)>) {
        match v {
            Json::Arr(a) if a.iter().any(|x| matches!(x, Json::Obj(_))) => {
                if out.is_none_or(|(n, _)| a.len() > n) {
                    *out = Some((a.len(), v));
                }
            }
            Json::Obj(o) if depth < 3 => {
                for (_, x) in o {
                    best(x, depth + 1, out);
                }
            }
            _ => {}
        }
    }
    let mut out = None;
    best(v, 0, &mut out);
    out.map_or(v, |(_, x)| x)
}

/// Flattens `v` into (column, value) pairs under `prefix`.
fn flatten(prefix: &str, v: &Json, out: &mut Vec<(String, Val)>) {
    let name = |k: &str| if prefix.is_empty() { k.to_string() } else { format!("{prefix}.{k}") };
    match v {
        Json::Obj(o) if !o.is_empty() => {
            for (k, x) in o {
                flatten(&name(k), x, out);
            }
        }
        Json::Obj(_) => out.push((prefix.to_string(), Val::Null)),
        Json::Arr(a) if a.iter().all(|x| !matches!(x, Json::Obj(_) | Json::Arr(_))) => {
            let parts: Vec<String> = a.iter().map(|x| scalar(x).to_text()).collect();
            out.push((prefix.to_string(), if parts.is_empty() { Val::Null } else { Val::Text(parts.join(", ")) }));
        }
        Json::Arr(_) => out.push((prefix.to_string(), Val::Text(to_text(v)))),
        _ => out.push((prefix.to_string(), scalar(v))),
    }
}

fn scalar(v: &Json) -> Val {
    match v {
        Json::Null => Val::Null,
        Json::Bool(b) => Val::Bool(*b),
        Json::Num(n) => Val::Num(*n),
        // ISO dates are the usual way JSON carries them.
        Json::Str(s) => crate::table::parse_date(s).filter(|_| s.len() >= 10 && s.as_bytes()[4] == b'-').map_or_else(|| Val::Text(s.clone()), Val::Date),
        _ => Val::Text(to_text(v)),
    }
}

/// A table of the records at `path` (empty: found by itself).
pub fn table(text: &str, path: &str) -> Result<DataTable, Error> {
    let root = parse(text)?;
    let v = if path.trim().is_empty() { find_records(&root) } else { walk(&root, path)? };
    let items: Vec<&Json> = match v {
        Json::Arr(a) => a.iter().collect(),
        // An object of objects ({"id1": {...}, "id2": {...}}): one row each,
        // the key first.
        Json::Obj(o) if o.len() > 1 && o.iter().all(|(_, x)| matches!(x, Json::Obj(_))) => {
            let mut rows: Vec<Vec<(String, Val)>> = Vec::new();
            for (k, x) in o {
                let mut r = vec![("Key".to_string(), Val::Text(k.clone()))];
                flatten("", x, &mut r);
                rows.push(r);
            }
            return Ok(from_pairs(rows));
        }
        other => vec![other],
    };
    let rows = items
        .into_iter()
        .map(|x| {
            let mut r = Vec::new();
            match x {
                Json::Obj(_) => flatten("", x, &mut r),
                _ => r.push(("Value".to_string(), if matches!(x, Json::Arr(_)) { Val::Text(to_text(x)) } else { scalar(x) })),
            }
            r
        })
        .collect();
    Ok(from_pairs(rows))
}

/// A table from rows of (column, value): the columns in the order they
/// first appear.
pub fn from_pairs(rows: Vec<Vec<(String, Val)>>) -> DataTable {
    let mut cols: Vec<String> = Vec::new();
    let mut index = std::collections::HashMap::new();
    for r in &rows {
        for (k, _) in r {
            if !index.contains_key(k) {
                index.insert(k.clone(), cols.len());
                cols.push(k.clone());
            }
        }
    }
    let mut t = DataTable::new(unique_names(cols.clone()));
    // Each row gets every column: as many rows as the cells allowed.
    let most = crate::steps::MAX_CELLS / cols.len().max(1);
    for r in rows.into_iter().take(most) {
        let mut row = vec![Val::Null; cols.len()];
        for (k, v) in r {
            row[index[&k]] = v;
        }
        t.rows.push(row);
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        let v = parse(r#"{"a": [1, -2.5e1, true, null, "x\"é😀"], "b": {}}"#).unwrap();
        assert_eq!(v, Json::Obj(vec![("a".into(), Json::Arr(vec![Json::Num(1.0), Json::Num(-25.0), Json::Bool(true), Json::Null, Json::Str("x\"é😀".into())])), ("b".into(), Json::Obj(vec![]))]));
        assert!(parse("{\"a\": }").is_err());
        assert!(parse("[1, 2").is_err());
        assert_eq!(parse("{\"a\":1}\n{\"a\":2}\n").unwrap(), Json::Arr(vec![Json::Obj(vec![("a".into(), Json::Num(1.0))]), Json::Obj(vec![("a".into(), Json::Num(2.0))])]));
    }

    #[test]
    fn an_escaped_wide_character_is_kept() {
        assert!(matches!(parse("[\"a\\é\"]"), Ok(_)));
    }

    #[test]
    fn flattens() {
        let src = r#"{"meta": {"count": 2}, "data": {"items": [
            {"id": 1, "name": "Ann", "address": {"city": "Oslo", "zip": "0150"}, "tags": ["a", "b"], "since": "2020-05-01"},
            {"id": 2, "name": "Bo", "extra": true, "orders": [{"n": 1}]}
        ]}}"#;
        for path in ["data.items", ""] {
            let t = table(src, path).unwrap();
            assert_eq!(t.columns, ["id", "name", "address.city", "address.zip", "tags", "since", "extra", "orders"]);
            assert_eq!(t.rows[0][2], Val::text("Oslo"));
            assert_eq!(t.rows[0][3], Val::text("0150"));
            assert_eq!(t.rows[0][4], Val::text("a, b"));
            assert_eq!(t.rows[0][5], Val::Date(43952.0));
            assert_eq!(t.rows[1][6], Val::Bool(true));
            assert_eq!(t.rows[1][7], Val::text(r#"[{"n":1}]"#));
        }
        assert!(table(src, "data.nope").is_err());
        let t = table(r#"{"x": {"a": 1}, "y": {"a": 2}}"#, "").unwrap();
        assert_eq!(t.columns, ["Key", "a"]);
        let t = table("[1, 2, 3]", "").unwrap();
        assert_eq!(t.columns, ["Value"]);
        assert_eq!(t.rows.len(), 3);
    }
}
