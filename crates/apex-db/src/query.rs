//! Queries: the visual designer's model ([`QueryDesign`]), which builds the
//! SQL, the criteria typed the way Access takes them ("> 5", "Like A*",
//! "Between 1 And 9") and the translation of Access SQL to SQLite's
//! ([`from_access`]), used for imported queries and for expressions typed in
//! the designer.
//!
//! A saved query is an object of kind [`Kind::Query`](crate::objects::Kind):
//! its SQL (`sql`) and, when made in the designer, the design.

use crate::model::{fmt_date, parse_date};
use crate::objects::Record;

/// An identifier in double quotes.
fn q(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum QueryType {
    #[default]
    Select,
    /// Changes fields of the rows found (to each field's `update_to`).
    Update,
    Delete,
    /// Adds the rows found to the table `target`.
    Append,
    /// Makes the table `target` from the rows found.
    MakeTable,
}

impl QueryType {
    pub const ALL: [QueryType; 5] = [QueryType::Select, QueryType::Update, QueryType::Delete, QueryType::Append, QueryType::MakeTable];

    pub fn key(self) -> &'static str {
        match self {
            QueryType::Select => "select",
            QueryType::Update => "update",
            QueryType::Delete => "delete",
            QueryType::Append => "append",
            QueryType::MakeTable => "maketable",
        }
    }

    pub fn from_key(k: &str) -> QueryType {
        QueryType::ALL.into_iter().find(|t| t.key() == k).unwrap_or_default()
    }

    /// It changes the data (and is run after a confirmation).
    pub fn is_action(self) -> bool {
        self != QueryType::Select
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Sort {
    #[default]
    None,
    Asc,
    Desc,
}

/// The totals row of the designer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Total {
    #[default]
    GroupBy,
    Sum,
    Avg,
    Count,
    Min,
    Max,
    /// A field only used for criteria (not grouped, not shown).
    Where,
    /// An expression of aggregates, taken as written.
    Expression,
}

impl Total {
    pub const ALL: [Total; 8] = [Total::GroupBy, Total::Sum, Total::Avg, Total::Count, Total::Min, Total::Max, Total::Where, Total::Expression];

    pub fn key(self) -> &'static str {
        match self {
            Total::GroupBy => "group",
            Total::Sum => "sum",
            Total::Avg => "avg",
            Total::Count => "count",
            Total::Min => "min",
            Total::Max => "max",
            Total::Where => "where",
            Total::Expression => "expression",
        }
    }

    pub fn from_key(k: &str) -> Total {
        Total::ALL.into_iter().find(|t| t.key() == k).unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum JoinKind {
    /// Only rows with a match in both.
    #[default]
    Inner,
    /// Every row of the left table.
    Left,
    /// Every row of the right table.
    Right,
}

impl JoinKind {
    fn sql(self) -> &'static str {
        match self {
            JoinKind::Inner => "INNER JOIN",
            JoinKind::Left => "LEFT JOIN",
            JoinKind::Right => "RIGHT JOIN",
        }
    }

    fn key(self) -> &'static str {
        match self {
            JoinKind::Inner => "inner",
            JoinKind::Left => "left",
            JoinKind::Right => "right",
        }
    }

    fn from_key(k: &str) -> JoinKind {
        match k {
            "left" => JoinKind::Left,
            "right" => JoinKind::Right,
            _ => JoinKind::Inner,
        }
    }
}

/// A table on the designer (and where it is drawn).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct QTable {
    pub name: String,
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Join {
    pub left: String,
    pub left_col: String,
    pub right: String,
    pub right_col: String,
    pub kind: JoinKind,
}

/// A column of the designer's grid.
#[derive(Clone, Debug, PartialEq)]
pub struct QField {
    /// Empty when `field` is an expression.
    pub table: String,
    /// A column name, "*", or an expression in Access syntax.
    pub field: String,
    pub alias: String,
    pub show: bool,
    pub sort: Sort,
    pub total: Total,
    /// Criteria: one per "or" row.
    pub criteria: Vec<String>,
    /// The new value (update queries), an expression.
    pub update_to: String,
}

impl Default for QField {
    fn default() -> QField {
        QField { table: String::new(), field: String::new(), alias: String::new(), show: true, sort: Sort::None, total: Total::GroupBy, criteria: Vec::new(), update_to: String::new() }
    }
}

impl QField {
    pub fn column(table: &str, field: &str) -> QField {
        QField { table: table.to_string(), field: field.to_string(), ..Default::default() }
    }

    /// The field in SQL (without its alias).
    pub fn expr_sql(&self) -> String {
        if self.table.is_empty() {
            from_access(&self.field)
        } else if self.field == "*" {
            format!("{}.*", q(&self.table))
        } else {
            format!("{}.{}", q(&self.table), q(&self.field))
        }
    }

    /// The name of its result column.
    pub fn out_name(&self) -> String {
        if !self.alias.is_empty() { self.alias.clone() } else { self.field.clone() }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct QueryDesign {
    pub kind: QueryType,
    pub tables: Vec<QTable>,
    pub joins: Vec<Join>,
    pub fields: Vec<QField>,
    /// The totals row is shown (GROUP BY).
    pub totals: bool,
    pub distinct: bool,
    /// Only the first rows (0: all).
    pub top: usize,
    /// The table of append and make-table queries.
    pub target: String,
}

impl QueryDesign {
    fn from_sql(&self) -> String {
        let Some(first) = self.tables.first() else { return String::new() };
        let mut placed = vec![first.name.clone()];
        let mut out = q(&first.name);
        let mut used = vec![false; self.joins.len()];
        // Tables joined to those already placed, until none is left.
        loop {
            let mut progress = false;
            for (i, j) in self.joins.iter().enumerate() {
                if used[i] {
                    continue;
                }
                let has = |t: &str| placed.iter().any(|p| p == t);
                let (new, kind) = if has(&j.left) && !has(&j.right) {
                    (&j.right, j.kind)
                } else if has(&j.right) && !has(&j.left) {
                    // Seen from the other side, left and right swap.
                    (&j.left, match j.kind { JoinKind::Left => JoinKind::Right, JoinKind::Right => JoinKind::Left, k => k })
                } else if has(&j.left) && has(&j.right) {
                    // A second condition between placed tables: part of the ON of the last join is
                    // simplest as WHERE, but that changes outer joins; add it as a join condition.
                    used[i] = true;
                    out.push_str(&format!(" AND {}.{} = {}.{}", q(&j.left), q(&j.left_col), q(&j.right), q(&j.right_col)));
                    progress = true;
                    continue;
                } else {
                    continue;
                };
                used[i] = true;
                progress = true;
                out = format!("{out} {} {} ON {}.{} = {}.{}", kind.sql(), q(new), q(&j.left), q(&j.left_col), q(&j.right), q(&j.right_col));
                placed.push(new.clone());
            }
            if !progress {
                break;
            }
        }
        for t in &self.tables {
            if !placed.contains(&t.name) {
                out = format!("{out}, {}", q(&t.name));
                placed.push(t.name.clone());
            }
        }
        out
    }

    fn where_sql(&self) -> String {
        let rows = self.fields.iter().map(|f| f.criteria.len()).max().unwrap_or(0);
        let mut ors = Vec::new();
        for r in 0..rows {
            let ands: Vec<String> = self
                .fields
                .iter()
                .filter(|f| !(self.totals && !matches!(f.total, Total::GroupBy | Total::Where)))
                .filter_map(|f| f.criteria.get(r).filter(|c| !c.trim().is_empty()).map(|c| criterion(&f.expr_sql(), c)))
                .collect();
            if !ands.is_empty() {
                ors.push(ands.join(" AND "));
            }
        }
        match ors.len() {
            0 => String::new(),
            1 => format!(" WHERE {}", ors[0]),
            _ => format!(" WHERE ({})", ors.join(") OR (")),
        }
    }

    /// Criteria on aggregates (HAVING).
    fn having_sql(&self) -> String {
        let rows = self.fields.iter().map(|f| f.criteria.len()).max().unwrap_or(0);
        let mut ors = Vec::new();
        for r in 0..rows {
            let ands: Vec<String> = self
                .fields
                .iter()
                .filter(|f| self.totals && !matches!(f.total, Total::GroupBy | Total::Where))
                .filter_map(|f| f.criteria.get(r).filter(|c| !c.trim().is_empty()).map(|c| criterion(&self.agg(f), c)))
                .collect();
            if !ands.is_empty() {
                ors.push(ands.join(" AND "));
            }
        }
        match ors.len() {
            0 => String::new(),
            1 => format!(" HAVING {}", ors[0]),
            _ => format!(" HAVING ({})", ors.join(") OR (")),
        }
    }

    fn agg(&self, f: &QField) -> String {
        let e = f.expr_sql();
        if !self.totals {
            return e;
        }
        match f.total {
            Total::GroupBy | Total::Where | Total::Expression => e,
            Total::Sum => format!("SUM({e})"),
            Total::Avg => format!("AVG({e})"),
            Total::Count => format!("COUNT({e})"),
            Total::Min => format!("MIN({e})"),
            Total::Max => format!("MAX({e})"),
        }
    }

    /// The SELECT of the rows found.
    pub fn select_sql(&self) -> String {
        let mut cols = Vec::new();
        for f in &self.fields {
            if !f.show || (self.totals && f.total == Total::Where) {
                continue;
            }
            let e = self.agg(f);
            let alias = if !f.alias.is_empty() {
                f.alias.clone()
            } else if self.totals && !matches!(f.total, Total::GroupBy | Total::Expression) {
                format!("{}Of{}", title(f.total.key()), f.field)
            } else {
                String::new()
            };
            cols.push(if alias.is_empty() { e } else { format!("{e} AS {}", q(&alias)) });
        }
        if cols.is_empty() {
            cols.push("*".into());
        }
        let mut sql = format!("SELECT {}{} FROM {}", if self.distinct { "DISTINCT " } else { "" }, cols.join(", "), self.from_sql());
        sql.push_str(&self.where_sql());
        if self.totals {
            let groups: Vec<String> = self.fields.iter().filter(|f| f.total == Total::GroupBy).map(|f| f.expr_sql()).collect();
            if !groups.is_empty() {
                sql.push_str(&format!(" GROUP BY {}", groups.join(", ")));
            }
            sql.push_str(&self.having_sql());
        }
        let order: Vec<String> = self
            .fields
            .iter()
            .filter(|f| f.sort != Sort::None)
            .map(|f| format!("{} {}", self.agg(f), if f.sort == Sort::Asc { "ASC" } else { "DESC" }))
            .collect();
        if !order.is_empty() {
            sql.push_str(&format!(" ORDER BY {}", order.join(", ")));
        }
        if self.top > 0 {
            sql.push_str(&format!(" LIMIT {}", self.top));
        }
        sql
    }

    /// The query's SQL (by its type).
    pub fn to_sql(&self) -> String {
        match self.kind {
            QueryType::Select => self.select_sql(),
            QueryType::MakeTable => format!("CREATE TABLE {} AS {}", q(&self.target), self.select_sql()),
            QueryType::Append => {
                let cols: Vec<String> = self.fields.iter().filter(|f| f.show && f.field != "*").map(|f| q(&f.out_name())).collect();
                let into = if cols.is_empty() { String::new() } else { format!(" ({})", cols.join(", ")) };
                format!("INSERT INTO {}{into} {}", q(&self.target), self.select_sql())
            }
            QueryType::Delete => {
                let t = self.tables.first().map(|t| t.name.as_str()).unwrap_or("");
                format!("DELETE FROM {}{}", q(t), self.where_sql())
            }
            QueryType::Update => {
                let t = self.tables.first().map(|t| t.name.as_str()).unwrap_or("");
                let sets: Vec<String> = self.fields.iter().filter(|f| !f.update_to.trim().is_empty() && !f.table.is_empty()).map(|f| format!("{} = {}", q(&f.field), from_access(&f.update_to))).collect();
                format!("UPDATE {} SET {}{}", q(t), sets.join(", "), self.where_sql())
            }
        }
    }

    /// The design as the records of a saved query (with its SQL).
    pub fn to_records(&self) -> Vec<Record> {
        let mut head = Record::default();
        head.set("sql", self.to_sql()).set("type", self.kind.key()).set_flag("totals", self.totals).set_flag("distinct", self.distinct).set("top", self.top).set("target", &self.target);
        let mut out = vec![head];
        for t in &self.tables {
            let mut r = Record::new("table");
            r.set("name", &t.name).set("x", t.x).set("y", t.y);
            out.push(r);
        }
        for j in &self.joins {
            let mut r = Record::new("join");
            r.set("left", &j.left).set("left_col", &j.left_col).set("right", &j.right).set("right_col", &j.right_col).set("kind", j.kind.key());
            out.push(r);
        }
        for f in &self.fields {
            let mut r = Record::new("field");
            r.set("table", &f.table).set("field", &f.field).set("alias", &f.alias).set_flag("show", f.show);
            r.set("sort", match f.sort { Sort::None => "", Sort::Asc => "asc", Sort::Desc => "desc" }).set("total", f.total.key()).set("criteria", f.criteria.join("\t")).set("update_to", &f.update_to);
            out.push(r);
        }
        out
    }

    /// A design from a saved query's records (None when it was written in
    /// SQL).
    pub fn from_records(recs: &[Record]) -> Option<QueryDesign> {
        let head = recs.first()?;
        if !recs.iter().any(|r| r.section == "table") {
            return None;
        }
        let mut d = QueryDesign {
            kind: QueryType::from_key(head.get("type")),
            totals: head.flag("totals"),
            distinct: head.flag("distinct"),
            top: head.int("top", 0).max(0) as usize,
            target: head.get("target").to_string(),
            ..Default::default()
        };
        // A designer shows a few of each: a file may bring any number.
        const MOST: usize = 256;
        for r in recs {
            match r.section.as_str() {
                "table" | "join" | "field" if d.tables.len().max(d.joins.len()).max(d.fields.len()) >= MOST => {}
                "table" => d.tables.push(QTable { name: r.get("name").to_string(), x: r.num("x", 0.0), y: r.num("y", 0.0) }),
                "join" => d.joins.push(Join {
                    left: r.get("left").to_string(),
                    left_col: r.get("left_col").to_string(),
                    right: r.get("right").to_string(),
                    right_col: r.get("right_col").to_string(),
                    kind: JoinKind::from_key(r.get("kind")),
                }),
                "field" => d.fields.push(QField {
                    table: r.get("table").to_string(),
                    field: r.get("field").to_string(),
                    alias: r.get("alias").to_string(),
                    show: r.flag("show"),
                    sort: match r.get("sort") { "asc" => Sort::Asc, "desc" => Sort::Desc, _ => Sort::None },
                    total: Total::from_key(r.get("total")),
                    criteria: if r.get("criteria").is_empty() { Vec::new() } else { r.get("criteria").split('\t').take(64).map(str::to_string).collect() },
                    update_to: r.get("update_to").to_string(),
                }),
                _ => {}
            }
        }
        Some(d)
    }
}

fn title(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// A criterion typed in the designer as an SQL condition on `col`.
pub fn criterion(col: &str, text: &str) -> String {
    criterion_in(col, text, 0)
}

/// [`criterion`] at `depth` NOTs and ORs within others.
fn criterion_in(col: &str, text: &str, depth: usize) -> String {
    let t = text.trim().trim_start_matches('=').trim();
    let upper = t.to_ascii_uppercase();
    let value = |v: &str| -> String {
        let v = v.trim();
        if v.is_empty() {
            return "''".into();
        }
        let first = v.chars().next().unwrap();
        let is_num = v.parse::<f64>().is_ok();
        let is_word = ["NULL", "TRUE", "FALSE", "YES", "NO"].contains(&v.to_uppercase().as_str());
        if is_num || is_word || matches!(first, '\'' | '"' | '#' | '[' | '(') || v.contains('(') {
            from_access(v)
        } else {
            lit(v)
        }
    };
    for op in ["<>", "<=", ">=", "<", ">"] {
        if let Some(rest) = t.strip_prefix(op) {
            return format!("{col} {op} {}", value(rest));
        }
    }
    if upper == "IS NULL" || upper == "IS NOT NULL" {
        return format!("{col} {upper}");
    }
    if upper.starts_with("LIKE ") || upper.starts_with("NOT LIKE ") {
        let (not, rest) = if upper.starts_with("NOT ") { ("NOT ", &t[9..]) } else { ("", &t[5..]) };
        let rest = rest.trim();
        let pat = match rest.strip_prefix(['"', '\'']) {
            Some(r) => r.strip_suffix(['"', '\'']).unwrap_or(r).to_string(),
            None => rest.to_string(),
        };
        return format!("{col} {not}LIKE {}", lit(&like_pattern(&pat)));
    }
    if upper.starts_with("BETWEEN ") {
        if let Some(i) = upper[8..].find(" AND ").map(|i| i + 8) {
            return format!("{col} BETWEEN {} AND {}", value(&t[8..i]), value(&t[i + 5..]));
        }
    }
    if upper.starts_with("IN ") || upper.starts_with("IN(") || upper.starts_with("NOT IN") {
        return format!("{col} {}", from_access(t));
    }
    // Typed criteria nest a few deep at most.
    if depth >= 32 {
        return format!("{col} = {}", lit(t));
    }
    if upper.starts_with("NOT ") {
        return format!("NOT ({})", criterion_in(col, &t[4..], depth + 1));
    }
    // "Ana Or Bea"
    let parts = split_word(t, " OR ");
    if parts.len() > 1 {
        return format!("({})", parts.iter().map(|p| criterion_in(col, p, depth + 1)).collect::<Vec<_>>().join(" OR "));
    }
    if t.contains('*') || t.contains('?') {
        let pat = t.trim_matches('"');
        return format!("{col} LIKE {}", lit(&like_pattern(pat)));
    }
    format!("{col} = {}", value(t))
}

/// Splits at a word (any case) outside quotes.
fn split_word<'a>(s: &'a str, word: &str) -> Vec<&'a str> {
    // ASCII case only: the same byte positions as `s`.
    let up = s.to_ascii_uppercase();
    let mut out = Vec::new();
    let mut start = 0;
    let mut quote = None;
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match quote {
            Some(qc) if c == qc => quote = None,
            Some(_) => {}
            None if c == b'"' || c == b'\'' => quote = Some(c),
            None if up.as_bytes()[i..].starts_with(word.as_bytes()) => {
                out.push(&s[start..i]);
                i += word.len();
                start = i;
                continue;
            }
            None => {}
        }
        i += 1;
    }
    out.push(&s[start..]);
    out
}

/// An Access pattern (`*`, `?`, `#`) as a LIKE one.
pub fn like_pattern(p: &str) -> String {
    p.chars().map(|c| match c { '*' => '%', '?' => '_', c => c }).collect()
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ws(String),
    Str(String),
    Ident(String),
    /// `[name]`
    Quoted(String),
    Date(String),
    Num(String),
    Punct(String),
}

fn tokenize(s: &str) -> Vec<Tok> {
    let c: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        let start = i;
        if ch.is_whitespace() {
            while i < c.len() && c[i].is_whitespace() {
                i += 1;
            }
            out.push(Tok::Ws(c[start..i].iter().collect()));
        } else if ch == '\'' || ch == '"' {
            i += 1;
            let mut v = String::new();
            while i < c.len() {
                if c[i] == ch {
                    if c.get(i + 1) == Some(&ch) {
                        v.push(ch);
                        i += 2;
                        continue;
                    }
                    break;
                }
                v.push(c[i]);
                i += 1;
            }
            i += 1;
            out.push(Tok::Str(v));
        } else if ch == '[' {
            i += 1;
            let mut v = String::new();
            while i < c.len() && c[i] != ']' {
                v.push(c[i]);
                i += 1;
            }
            i += 1;
            out.push(Tok::Quoted(v));
        } else if ch == '`' {
            i += 1;
            let mut v = String::new();
            while i < c.len() && c[i] != '`' {
                v.push(c[i]);
                i += 1;
            }
            i += 1;
            out.push(Tok::Quoted(v));
        } else if ch == '#' && c[i + 1..].iter().take(30).any(|&x| x == '#') && c.get(i + 1).is_some_and(|x| x.is_ascii_digit()) {
            i += 1;
            let mut v = String::new();
            while i < c.len() && c[i] != '#' {
                v.push(c[i]);
                i += 1;
            }
            i += 1;
            out.push(Tok::Date(v));
        } else if ch.is_ascii_digit() || (ch == '.' && c.get(i + 1).is_some_and(|x| x.is_ascii_digit())) {
            while i < c.len() && (c[i].is_ascii_alphanumeric() || c[i] == '.') {
                i += 1;
            }
            out.push(Tok::Num(c[start..i].iter().collect()));
        } else if ch.is_alphabetic() || ch == '_' {
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') {
                i += 1;
            }
            out.push(Tok::Ident(c[start..i].iter().collect()));
        } else {
            let two: String = c[i..(i + 2).min(c.len())].iter().collect();
            if ["<>", "<=", ">=", "!=", "||"].contains(&two.as_str()) {
                i += 2;
                out.push(Tok::Punct(two));
            } else {
                i += 1;
                out.push(Tok::Punct(ch.to_string()));
            }
        }
    }
    out
}

/// An Access date literal's text ("1/31/2024", "2024-01-31 10:00") as
/// ISO text.
fn access_date(s: &str) -> String {
    let s = s.trim();
    if let Some(d) = parse_date(s) {
        return fmt_date(d);
    }
    let (date, time) = s.split_once(' ').unwrap_or((s, ""));
    let p: Vec<&str> = date.split(['/', '-', '.']).collect();
    if p.len() == 3 {
        if let (Ok(m), Ok(d), Ok(y)) = (p[0].parse::<u32>(), p[1].parse::<u32>(), p[2].parse::<i64>()) {
            let y = if y < 30 { 2000 + y } else if y < 100 { 1900 + y } else { y };
            let iso = format!("{y:04}-{m:02}-{d:02} {time}");
            if let Some(v) = parse_date(iso.trim()) {
                return fmt_date(v);
            }
        }
    }
    s.to_string()
}

fn emit(toks: &[Tok]) -> String {
    let mut out = String::new();
    for t in toks {
        match t {
            Tok::Ws(s) | Tok::Ident(s) | Tok::Num(s) | Tok::Punct(s) => out.push_str(s),
            Tok::Str(s) => out.push_str(&lit(s)),
            Tok::Quoted(s) => out.push_str(&q(s)),
            Tok::Date(s) => out.push_str(&lit(&access_date(s))),
        }
    }
    out
}

fn is_kw(t: &Tok, k: &str) -> bool {
    matches!(t, Tok::Ident(s) if s.eq_ignore_ascii_case(k))
}

/// Splits tokens at top-level commas.
fn args(toks: &[Tok]) -> Vec<&[Tok]> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, t) in toks.iter().enumerate() {
        match t {
            Tok::Punct(p) if p == "(" => depth += 1,
            Tok::Punct(p) if p == ")" => depth -= 1,
            Tok::Punct(p) if p == "," && depth == 0 => {
                out.push(&toks[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if !toks.is_empty() {
        out.push(&toks[start..]);
    }
    out
}

/// Function calls nested in one another that are translated; deeper ones
/// are kept as written (an expression of a file may nest them without end).
const MAX_CALLS: usize = 64;

thread_local! {
    static CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Translates tokens of an expression: functions, operators and values.
fn expr(toks: &[Tok]) -> String {
    CALLS.with(|c| c.set(c.get() + 1));
    struct Out;
    impl Drop for Out {
        fn drop(&mut self) {
            CALLS.with(|c| c.set(c.get() - 1));
        }
    }
    let _out = Out;
    let deep = CALLS.with(|c| c.get()) > MAX_CALLS;
    // Where each "(" closes, found in one pass (looking for each one's
    // would take quadratic time on many left open).
    let mut closes = vec![None; toks.len()];
    let mut open = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        match t {
            Tok::Punct(p) if p == "(" => open.push(i),
            Tok::Punct(p) if p == ")" => {
                if let Some(o) = open.pop() {
                    closes[o] = Some(i);
                }
            }
            _ => {}
        }
    }
    let mut out = String::new();
    let mut i = 0;
    let mut after_like = false;
    while i < toks.len() {
        let t = &toks[i];
        // A function call.
        if let Tok::Ident(name) = t {
            let mut k = i + 1;
            while matches!(toks.get(k), Some(Tok::Ws(_))) {
                k += 1;
            }
            const WORDS: [&str; 12] = ["IN", "EXISTS", "NOT", "AND", "OR", "VALUES", "AS", "ON", "FROM", "WHERE", "SELECT", "JOIN"];
            let word = WORDS.iter().any(|w| name.eq_ignore_ascii_case(w));
            if !word && !deep && matches!(toks.get(k), Some(Tok::Punct(p)) if p == "(") {
                if let Some(close) = closes[k] {
                    let a: Vec<String> = args(&toks[k + 1..close]).into_iter().map(|a| expr(a).trim().to_string()).collect();
                    out.push_str(&function(name, &a));
                    i = close + 1;
                    continue;
                }
            }
            let up = name.to_ascii_uppercase();
            match up.as_str() {
                "TRUE" | "YES" => out.push('1'),
                "FALSE" | "NO" => out.push('0'),
                "MOD" => out.push('%'),
                "DISTINCTROW" => out.push_str("DISTINCT"),
                "LIKE" | "ALIKE" => {
                    out.push_str("LIKE");
                    after_like = true;
                    i += 1;
                    continue;
                }
                _ => out.push_str(name),
            }
            i += 1;
            continue;
        }
        match t {
            Tok::Str(s) if after_like => {
                out.push_str(&lit(&like_pattern(s)));
                after_like = false;
            }
            Tok::Punct(p) if p == "&" => out.push_str("||"),
            Tok::Punct(p) if p == "\\" => out.push('/'),
            Tok::Punct(p) if p == "!" => out.push('.'),
            Tok::Ws(_) => out.push_str(&emit(std::slice::from_ref(t))),
            _ => {
                out.push_str(&emit(std::slice::from_ref(t)));
                after_like = false;
            }
        }
        i += 1;
    }
    out
}

/// An Access function call as SQLite's.
fn function(name: &str, a: &[String]) -> String {
    let a0 = a.first().cloned().unwrap_or_default();
    let a1 = a.get(1).cloned().unwrap_or_default();
    let part = |f: &str| format!("CAST(strftime('{f}', {a0}) AS INTEGER)");
    match name.to_ascii_lowercase().as_str() {
        "iif" => format!("IIF({})", a.join(", ")),
        "nz" if a.len() == 1 => format!("IFNULL({a0}, '')"),
        "nz" => format!("IFNULL({})", a.join(", ")),
        "mid" => format!("substr({})", a.join(", ")),
        "left" => format!("substr({a0}, 1, {a1})"),
        "right" => format!("substr({a0}, -({a1}))"),
        "len" => format!("length({a0})"),
        "ucase" => format!("upper({a0})"),
        "lcase" => format!("lower({a0})"),
        "instr" if a.len() == 2 => format!("instr({a0}, {a1})"),
        "now" => "datetime('now', 'localtime')".into(),
        "date" if a.is_empty() => "date('now', 'localtime')".into(),
        "time" if a.is_empty() => "time('now', 'localtime')".into(),
        "year" => part("%Y"),
        "month" => part("%m"),
        "day" => part("%d"),
        "hour" => part("%H"),
        "minute" => part("%M"),
        "second" => part("%S"),
        "int" | "fix" => format!("CAST({a0} AS INTEGER)"),
        "clng" | "cint" => format!("CAST(round({a0}) AS INTEGER)"),
        "cdbl" | "csng" | "ccur" | "val" => format!("CAST({a0} AS REAL)"),
        "cstr" => format!("CAST({a0} AS TEXT)"),
        "cbool" => format!("({a0} <> 0)"),
        "isnull" => format!("({a0} IS NULL)"),
        "sgn" => format!("sign({a0})"),
        "sqr" => format!("sqrt({a0})"),
        "space" => format!("printf('%*s', {a0}, '')"),
        "string" => format!("replace(printf('%*s', {a0}, ''), ' ', substr({a1}, 1, 1))"),
        "first" => format!("MIN({a0})"),
        "last" => format!("MAX({a0})"),
        "stdev" | "var" | "stdevp" | "varp" => format!("{}({})", name.to_ascii_uppercase(), a.join(", ")),
        _ => format!("{name}({})", a.join(", ")),
    }
}

/// An expression or statement in Access SQL as SQLite SQL: `[names]`,
/// `#dates#`, `"text"`, `&`, `*` and `?` in LIKE, `TOP n`, `DISTINCTROW`,
/// `SELECT ... INTO` (make-table), `DELETE t.* FROM`, `PARAMETERS` and the
/// usual functions (IIf, Nz, Mid, Left, Right, Len, UCase, Year...).
pub fn from_access(sql: &str) -> String {
    let mut toks = tokenize(sql.trim().trim_end_matches(';').trim());
    // PARAMETERS a Text, b Long;
    if toks.first().is_some_and(|t| is_kw(t, "PARAMETERS")) {
        if let Some(p) = toks.iter().position(|t| matches!(t, Tok::Punct(p) if p == ";")) {
            toks.drain(..=p);
            while matches!(toks.first(), Some(Tok::Ws(_))) {
                toks.remove(0);
            }
        }
    }
    let mut limit = None;
    let mut prefix = String::new();
    // TOP n [PERCENT] after SELECT [DISTINCT].
    if let Some(sel) = toks.iter().position(|t| is_kw(t, "SELECT")) {
        let mut k = sel + 1;
        let skip_ws = |k: &mut usize, toks: &[Tok]| {
            while matches!(toks.get(*k), Some(Tok::Ws(_))) {
                *k += 1;
            }
        };
        skip_ws(&mut k, &toks);
        if toks.get(k).is_some_and(|t| is_kw(t, "DISTINCT") || is_kw(t, "DISTINCTROW")) {
            k += 1;
            skip_ws(&mut k, &toks);
        }
        if toks.get(k).is_some_and(|t| is_kw(t, "TOP")) {
            let mut e = k + 1;
            skip_ws(&mut e, &toks);
            if let Some(Tok::Num(n)) = toks.get(e) {
                limit = Some(n.clone());
                e += 1;
                let mut e2 = e;
                skip_ws(&mut e2, &toks);
                if toks.get(e2).is_some_and(|t| is_kw(t, "PERCENT")) {
                    e = e2 + 1;
                }
                toks.drain(k..e);
                while k > 0 && matches!(toks.get(k), Some(Tok::Ws(_))) && matches!(toks.get(k - 1), Some(Tok::Ws(_))) {
                    toks.remove(k);
                }
            }
        }
        // SELECT ... INTO t FROM: a make-table query.
        let top_into = {
            let mut depth = 0;
            let mut found = None;
            for (i, t) in toks.iter().enumerate().skip(sel) {
                match t {
                    Tok::Punct(p) if p == "(" => depth += 1,
                    Tok::Punct(p) if p == ")" => depth -= 1,
                    t if depth == 0 && is_kw(t, "INTO") => {
                        found = Some(i);
                        break;
                    }
                    t if depth == 0 && is_kw(t, "FROM") => break,
                    _ => {}
                }
            }
            found
        };
        if let Some(into) = top_into {
            if toks[..sel].iter().all(|t| matches!(t, Tok::Ws(_))) {
                let mut e = into + 1;
                skip_ws(&mut e, &toks);
                if let Some(Tok::Ident(n) | Tok::Quoted(n)) = toks.get(e).cloned() {
                    prefix = format!("CREATE TABLE {} AS ", q(&n));
                    toks.drain(into..=e);
                    while into > 0 && matches!(toks.get(into), Some(Tok::Ws(_))) && matches!(toks.get(into - 1), Some(Tok::Ws(_))) {
                        toks.remove(into);
                    }
                }
            }
        }
    }
    // DELETE t.* FROM t
    if toks.first().is_some_and(|t| is_kw(t, "DELETE")) {
        if let Some(from) = toks.iter().position(|t| is_kw(t, "FROM")) {
            toks.drain(1..from);
            toks.insert(1, Tok::Ws(" ".into()));
        }
    }
    let mut out = prefix + expr(&toks).trim();
    if let Some(n) = limit {
        out.push_str(&format!(" LIMIT {n}"));
    }
    out
}

/// Whether Access SQL can be run after [`from_access`] (crosstab queries,
/// pass-through ones and UPDATE with joins can't).
pub fn translatable(sql: &str) -> bool {
    let toks = tokenize(sql);
    let first = toks.iter().find(|t| !matches!(t, Tok::Ws(_)));
    if first.is_some_and(|t| is_kw(t, "TRANSFORM")) {
        return false;
    }
    if first.is_some_and(|t| is_kw(t, "UPDATE")) {
        let set = toks.iter().position(|t| is_kw(t, "SET")).unwrap_or(toks.len());
        return !toks[..set].iter().any(|t| is_kw(t, "JOIN"));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_sql() {
        assert_eq!(
            from_access("SELECT TOP 5 [Order ID], [First Name] & \" \" & [Last Name] AS Name FROM [Orders] WHERE [Date] > #1/31/2024# AND Name Like \"A*\";"),
            "SELECT \"Order ID\", \"First Name\" || ' ' || \"Last Name\" AS Name FROM \"Orders\" WHERE \"Date\" > '2024-01-31' AND Name LIKE 'A%' LIMIT 5"
        );
        assert_eq!(from_access("SELECT IIf(Nz([x])>0, Left([n], 2), UCase(Mid(n,2,3))) FROM t"), "SELECT IIF(IFNULL(\"x\", '')>0, substr(\"n\", 1, 2), upper(substr(n, 2, 3))) FROM t");
        assert_eq!(from_access("SELECT DISTINCTROW a INTO NewT FROM t"), "CREATE TABLE \"NewT\" AS SELECT DISTINCT a FROM t");
        assert_eq!(from_access("DELETE Orders.* FROM Orders WHERE Paid = True"), "DELETE FROM Orders WHERE Paid = 1");
        assert_eq!(from_access("PARAMETERS d DateTime; SELECT Year(d) FROM t"), "SELECT CAST(strftime('%Y', d) AS INTEGER) FROM t");
        assert!(!translatable("TRANSFORM Sum(x) SELECT a FROM t GROUP BY a PIVOT b"));
        assert!(!translatable("UPDATE a INNER JOIN b ON a.x=b.x SET a.y=1"));
    }

    #[test]
    fn criteria() {
        assert_eq!(criterion("c", "> 5"), "c > 5");
        assert_eq!(criterion("c", "Ana"), "c = 'Ana'");
        assert_eq!(criterion("c", "Like A*"), "c LIKE 'A%'");
        assert_eq!(criterion("c", "Between 1 And 9"), "c BETWEEN 1 AND 9");
        assert_eq!(criterion("c", "Is Null"), "c IS NULL");
        assert_eq!(criterion("c", "Ana Or Bea"), "(c = 'Ana' OR c = 'Bea')");
        assert_eq!(criterion("c", "Not 3"), "NOT (c = 3)");
        assert_eq!(criterion("c", ">= #2024-01-01#"), "c >= '2024-01-01'");
        assert_eq!(criterion("c", "In (1, 2)"), "c In (1, 2)");
    }

    #[test]
    fn design() {
        let mut d = QueryDesign {
            tables: vec![QTable { name: "Customers".into(), ..Default::default() }, QTable { name: "Orders".into(), ..Default::default() }],
            joins: vec![Join { left: "Customers".into(), left_col: "Id".into(), right: "Orders".into(), right_col: "Customer".into(), kind: JoinKind::Left }],
            fields: vec![QField::column("Customers", "Name"), QField { total: Total::Sum, ..QField::column("Orders", "Amount") }],
            totals: true,
            ..Default::default()
        };
        d.fields[0].sort = Sort::Asc;
        d.fields[0].criteria = vec!["Like A*".into()];
        d.fields[1].criteria = vec!["> 100".into()];
        assert_eq!(
            d.to_sql(),
            "SELECT \"Customers\".\"Name\", SUM(\"Orders\".\"Amount\") AS \"SumOfAmount\" FROM \"Customers\" LEFT JOIN \"Orders\" ON \"Customers\".\"Id\" = \"Orders\".\"Customer\" WHERE \"Customers\".\"Name\" LIKE 'A%' GROUP BY \"Customers\".\"Name\" HAVING SUM(\"Orders\".\"Amount\") > 100 ORDER BY \"Customers\".\"Name\" ASC"
        );
        let back = QueryDesign::from_records(&crate::objects::read(&crate::objects::write(&d.to_records()))).unwrap();
        assert_eq!(back, d);
        let u = QueryDesign {
            kind: QueryType::Update,
            tables: vec![QTable { name: "T".into(), ..Default::default() }],
            fields: vec![QField { update_to: "[Price] * 2".into(), criteria: vec!["> 1".into()], ..QField::column("T", "Price") }],
            ..Default::default()
        };
        assert_eq!(u.to_sql(), "UPDATE \"T\" SET \"Price\" = \"Price\" * 2 WHERE \"T\".\"Price\" > 1");
    }
}
