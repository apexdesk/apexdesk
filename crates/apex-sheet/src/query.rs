//! Get Data queries: where the data comes from (a file, a folder, a web
//! page, a database…) and the steps that shape it, loaded into a table of
//! the workbook. This is only the definition; the engine that runs it is
//! the `apex-query` crate, and the file formats keep it in a part of their
//! own. Passwords are never part of it.

/// A query of the workbook.
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    /// Unique in the workbook (case-insensitive).
    pub name: String,
    pub source: Source,
    pub steps: Vec<Step>,
    /// The table the result is loaded into; None keeps it as a connection
    /// only (other queries can still append or merge it).
    pub table: Option<String>,
    /// Refresh when the workbook is opened (asks first).
    pub refresh_on_open: bool,
}

impl Query {
    pub fn new(name: impl Into<String>, source: Source) -> Query {
        Query { name: name.into(), source, steps: Vec::new(), table: None, refresh_on_open: false }
    }
}

/// A place to read from: a local path or an `http(s)://` URL.
pub fn is_url(location: &str) -> bool {
    let l = location.trim_start().to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://")
}

/// Whether a path names a file on another computer (`\\\\server\\share`,
/// `//server/share`, `file://server/...`): reading it goes out to the
/// network, and on Windows signs in there with the user's account.
pub fn is_network_path(path: &str) -> bool {
    let p = path.trim_start();
    if p.starts_with("\\\\") || p.starts_with("//") || p.starts_with("\\/") || p.starts_with("/\\") {
        return true;
    }
    let l = p.to_ascii_lowercase();
    // Windows' object names (\??\UNC\server\share) reach shares too.
    let bare = l.strip_prefix("file:").unwrap_or(&l).trim_start_matches(['/', '\\']);
    if bare.starts_with("??\\") || bare.starts_with("??/") {
        return true;
    }
    let Some(rest) = l.strip_prefix("file:") else { return false };
    let slashes = rest.chars().take_while(|c| *c == '/' || *c == '\\').count();
    let host: String = rest[slashes..].chars().take_while(|c| *c != '/' && *c != '\\').collect();
    // file://server/share (not file:///C:/ nor file://localhost/), or file:////server/share.
    (slashes == 2 && !host.is_empty() && host != "localhost" && !host.ends_with(':')) || slashes >= 4
}

/// Database servers reached over the network.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DbKind {
    Postgres,
    MySql,
    SqlServer,
}

impl DbKind {
    pub fn key(self) -> &'static str {
        match self {
            DbKind::Postgres => "postgres",
            DbKind::MySql => "mysql",
            DbKind::SqlServer => "sqlserver",
        }
    }

    pub fn from_key(s: &str) -> Option<DbKind> {
        match s {
            "postgres" => Some(DbKind::Postgres),
            "mysql" => Some(DbKind::MySql),
            "sqlserver" => Some(DbKind::SqlServer),
            _ => None,
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            DbKind::Postgres => 5432,
            DbKind::MySql => 3306,
            DbKind::SqlServer => 1433,
        }
    }
}

/// Where a query reads its data.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// Delimited text (CSV, TSV…) from a file or a URL. `delimiter` None
    /// guesses it, an empty `encoding` guesses it (UTF-8/16 or Windows-1252).
    Text { location: String, delimiter: Option<char>, encoding: String, header: bool, skip: u32 },
    /// A sheet or a table of another workbook (xlsx, xls, ods…); an empty
    /// `item` takes the first sheet.
    Workbook { path: String, item: String, header: bool },
    /// Every file of a folder with extension `extension` (empty: any text,
    /// JSON or workbook file), one after another; `file_column` adds the
    /// file name as the first column.
    Folder { path: String, extension: String, header: bool, file_column: bool },
    /// JSON from a file or a URL (an API); `path` walks down to the list of
    /// records ("data.items"), objects inside are flattened ("a.b").
    Json { location: String, path: String },
    /// XML from a file or a URL: one row per `element` (empty: the most
    /// repeated one), its attributes and child elements as columns.
    Xml { location: String, element: String },
    /// The `index`th table (from 0) of a web page or an HTML file.
    Html { location: String, index: usize },
    /// The `index`th table (from 0) found in a PDF file (drawn as a grid
    /// or as aligned columns of text); `header`: its first row names the
    /// columns.
    Pdf { location: String, index: usize, header: bool },
    /// A SQL query (or a table name) on a SQLite file.
    Sqlite { path: String, sql: String },
    /// A table of an Access (.mdb, .accdb) or dBase (.dbf) file.
    DbFile { path: String, table: String },
    /// A SQL query on a PostgreSQL or MySQL/MariaDB server.
    Database { kind: DbKind, host: String, port: u16, database: String, user: String, sql: String, tls: bool },
    /// A table (or a defined name) of this workbook.
    Table { name: String },
    /// The result of another query.
    Query { name: String },
}

impl Source {
    /// The i18n key naming the kind of source.
    pub fn kind_key(&self) -> &'static str {
        match self {
            Source::Text { .. } => "getdata.src.text",
            Source::Workbook { .. } => "getdata.src.workbook",
            Source::Folder { .. } => "getdata.src.folder",
            Source::Json { .. } => "getdata.src.json",
            Source::Xml { .. } => "getdata.src.xml",
            Source::Html { .. } => "getdata.src.html",
            Source::Pdf { .. } => "getdata.src.pdf",
            Source::Sqlite { .. } => "getdata.src.sqlite",
            Source::DbFile { .. } => "getdata.src.dbfile",
            Source::Database { kind: DbKind::Postgres, .. } => "getdata.src.postgres",
            Source::Database { kind: DbKind::MySql, .. } => "getdata.src.mysql",
            Source::Database { kind: DbKind::SqlServer, .. } => "getdata.src.sqlserver",
            Source::Table { .. } => "getdata.src.table",
            Source::Query { .. } => "getdata.src.query",
        }
    }

    /// What it reads, in a few words (a path, a URL, a server).
    pub fn describe(&self) -> String {
        match self {
            Source::Text { location, .. } | Source::Json { location, .. } | Source::Xml { location, .. } | Source::Html { location, .. } | Source::Pdf { location, .. } => location.clone(),
            Source::Workbook { path, item, .. } | Source::DbFile { path, table: item } if !item.is_empty() => format!("{path} [{item}]"),
            Source::Workbook { path, .. } | Source::Folder { path, .. } | Source::Sqlite { path, .. } | Source::DbFile { path, .. } => path.clone(),
            // The server first, never "user@server": a user named like a
            // server would pass for it.
            Source::Database { user, host, port, database, .. } if user.is_empty() => format!("{host}:{port}/{database}"),
            Source::Database { user, host, port, database, .. } => format!("{host}:{port}/{database} ({user})"),
            Source::Table { name } | Source::Query { name } => name.clone(),
        }
    }

    /// Whether running it goes out to the network (or a database server).
    pub fn is_remote(&self) -> bool {
        match self {
            Source::Text { location, .. } | Source::Json { location, .. } | Source::Xml { location, .. } | Source::Html { location, .. } | Source::Pdf { location, .. } => is_url(location) || is_network_path(location),
            Source::Workbook { path, .. } | Source::Folder { path, .. } | Source::Sqlite { path, .. } | Source::DbFile { path, .. } => is_network_path(path),
            Source::Database { .. } => true,
            _ => false,
        }
    }
}

/// How a row is compared in a filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterOp {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    Contains,
    NotContains,
    Begins,
    Ends,
    Empty,
    NotEmpty,
}

impl FilterOp {
    pub const ALL: [FilterOp; 12] = [FilterOp::Eq, FilterOp::Ne, FilterOp::Gt, FilterOp::Ge, FilterOp::Lt, FilterOp::Le, FilterOp::Contains, FilterOp::NotContains, FilterOp::Begins, FilterOp::Ends, FilterOp::Empty, FilterOp::NotEmpty];

    pub fn key(self) -> &'static str {
        match self {
            FilterOp::Eq => "eq",
            FilterOp::Ne => "ne",
            FilterOp::Gt => "gt",
            FilterOp::Ge => "ge",
            FilterOp::Lt => "lt",
            FilterOp::Le => "le",
            FilterOp::Contains => "contains",
            FilterOp::NotContains => "not_contains",
            FilterOp::Begins => "begins",
            FilterOp::Ends => "ends",
            FilterOp::Empty => "empty",
            FilterOp::NotEmpty => "not_empty",
        }
    }

    pub fn from_key(s: &str) -> Option<FilterOp> {
        FilterOp::ALL.into_iter().find(|o| o.key() == s)
    }

    /// Whether it compares with a value (`Empty` and `NotEmpty` don't).
    pub fn takes_value(self) -> bool {
        !matches!(self, FilterOp::Empty | FilterOp::NotEmpty)
    }
}

/// The type of a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColType {
    /// Whatever each value is.
    Any,
    Text,
    Number,
    Integer,
    Bool,
    Date,
    DateTime,
}

impl ColType {
    pub const ALL: [ColType; 7] = [ColType::Any, ColType::Text, ColType::Number, ColType::Integer, ColType::Bool, ColType::Date, ColType::DateTime];

    pub fn key(self) -> &'static str {
        match self {
            ColType::Any => "any",
            ColType::Text => "text",
            ColType::Number => "number",
            ColType::Integer => "integer",
            ColType::Bool => "bool",
            ColType::Date => "date",
            ColType::DateTime => "datetime",
        }
    }

    pub fn from_key(s: &str) -> Option<ColType> {
        ColType::ALL.into_iter().find(|o| o.key() == s)
    }
}

/// How values of a group are combined.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AggFn {
    /// Rows of the group (the column is not used).
    Count,
    CountDistinct,
    Sum,
    Average,
    Min,
    Max,
    First,
    Last,
}

impl AggFn {
    pub const ALL: [AggFn; 8] = [AggFn::Count, AggFn::CountDistinct, AggFn::Sum, AggFn::Average, AggFn::Min, AggFn::Max, AggFn::First, AggFn::Last];

    pub fn key(self) -> &'static str {
        match self {
            AggFn::Count => "count",
            AggFn::CountDistinct => "count_distinct",
            AggFn::Sum => "sum",
            AggFn::Average => "average",
            AggFn::Min => "min",
            AggFn::Max => "max",
            AggFn::First => "first",
            AggFn::Last => "last",
        }
    }

    pub fn from_key(s: &str) -> Option<AggFn> {
        AggFn::ALL.into_iter().find(|o| o.key() == s)
    }
}

/// A column of a grouping: `func` of `column`, named `name`.
#[derive(Clone, Debug, PartialEq)]
pub struct Aggregate {
    pub column: String,
    pub func: AggFn,
    pub name: String,
}

/// Rows a merge keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinKind {
    /// Rows with a match on both sides.
    Inner,
    /// Every row of this query, with the matches of the other.
    Left,
    /// Every row of the other query.
    Right,
    /// Every row of both.
    Full,
    /// Rows of this query without a match.
    Anti,
}

impl JoinKind {
    pub const ALL: [JoinKind; 5] = [JoinKind::Inner, JoinKind::Left, JoinKind::Right, JoinKind::Full, JoinKind::Anti];

    pub fn key(self) -> &'static str {
        match self {
            JoinKind::Inner => "inner",
            JoinKind::Left => "left",
            JoinKind::Right => "right",
            JoinKind::Full => "full",
            JoinKind::Anti => "anti",
        }
    }

    pub fn from_key(s: &str) -> Option<JoinKind> {
        JoinKind::ALL.into_iter().find(|o| o.key() == s)
    }
}

/// A transformation of the rows so far.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// The first row becomes the column names.
    PromoteHeaders,
    /// Drops `skip` rows from the top and keeps `take` (0: all) after them.
    KeepRows { skip: usize, take: usize },
    /// Rows whose `column` meets `op` with `value`; text compared telling
    /// capitals apart when `case`.
    Filter { column: String, op: FilterOp, value: String, case: bool },
    RemoveColumns(Vec<String>),
    /// Keeps these columns, in this order.
    ChooseColumns(Vec<String>),
    /// (old, new) names.
    Rename(Vec<(String, String)>),
    /// Moves a column to position `to` (from 0).
    Move { column: String, to: usize },
    ChangeType { column: String, ty: ColType },
    /// Splits a column at each `delimiter` into `name.1`, `name.2`…
    Split { column: String, delimiter: String },
    /// A new column computed from the others (see the engine's formulas:
    /// `[Price] * [Qty]`, `UPPER([Name])`…).
    AddColumn { name: String, formula: String },
    /// Text replaced in a column (whole values when `whole`).
    Replace { column: String, find: String, replace: String, whole: bool },
    /// Keeps the first row of each set of equal values in these columns
    /// (none: every column).
    RemoveDuplicates(Vec<String>),
    /// (column, ascending), the first key first.
    Sort(Vec<(String, bool)>),
    Group { by: Vec<String>, aggs: Vec<Aggregate> },
    /// One column per distinct value of `column`, holding `func` of
    /// `values`.
    Pivot { column: String, values: String, func: AggFn },
    /// These columns become rows: (attribute, value) pairs.
    Unpivot { columns: Vec<String>, attribute: String, value: String },
    /// The rows of another query after these (columns matched by name).
    Append { query: String },
    /// Joins another query on (this column, its column) pairs, bringing
    /// its `columns` (none: all but the keys). `names` gives some of them
    /// another name (its column, the name here); the rest keep theirs, as
    /// "query.name" when this table has one so named.
    Merge { query: String, keys: Vec<(String, String)>, join: JoinKind, columns: Vec<String>, names: Vec<(String, String)> },
}

impl Step {
    /// The i18n key naming the kind of step.
    pub fn kind_key(&self) -> &'static str {
        match self {
            Step::PromoteHeaders => "getdata.step.promote",
            Step::KeepRows { .. } => "getdata.step.keep_rows",
            Step::Filter { .. } => "getdata.step.filter",
            Step::RemoveColumns(_) => "getdata.step.remove_cols",
            Step::ChooseColumns(_) => "getdata.step.choose_cols",
            Step::Rename(_) => "getdata.step.rename",
            Step::Move { .. } => "getdata.step.move",
            Step::ChangeType { .. } => "getdata.step.type",
            Step::Split { .. } => "getdata.step.split",
            Step::AddColumn { .. } => "getdata.step.add_col",
            Step::Replace { .. } => "getdata.step.replace",
            Step::RemoveDuplicates(_) => "getdata.step.dedupe",
            Step::Sort(_) => "getdata.step.sort",
            Step::Group { .. } => "getdata.step.group",
            Step::Pivot { .. } => "getdata.step.pivot",
            Step::Unpivot { .. } => "getdata.step.unpivot",
            Step::Append { .. } => "getdata.step.append",
            Step::Merge { .. } => "getdata.step.merge",
        }
    }

    /// Other queries it reads.
    pub fn uses_query(&self) -> Option<&str> {
        match self {
            Step::Append { query } | Step::Merge { query, .. } => Some(query),
            _ => None,
        }
    }
}

/// The queries `q` reads, directly (its source and its steps).
pub fn dependencies(q: &Query) -> Vec<&str> {
    let mut out: Vec<&str> = q.steps.iter().filter_map(Step::uses_query).collect();
    if let Source::Query { name } = &q.source {
        out.insert(0, name);
    }
    out
}

/// A query name not used by `queries`: `base`, `base (2)`…
pub fn unique_name(queries: &[Query], base: &str) -> String {
    let taken = |n: &str| queries.iter().any(|q| q.name.eq_ignore_ascii_case(n));
    if !taken(base) {
        return base.to_string();
    }
    (2..).map(|i| format!("{base} ({i})")).find(|n| !taken(n)).unwrap_or_else(|| base.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_paths_are_remote() {
        for p in ["\\\\attacker\\share\\x.csv", "//attacker/share/x.csv", "file://attacker/share/x.csv", "file:////attacker/share/x.csv", " \\\\h\\s", "\\??\\UNC\\h\\s\\x.csv", "file:///\\??\\UNC\\h\\s", "\\/h/s"] {
            assert!(is_network_path(p), "{p}");
        }
        for p in ["C:\\data\\x.csv", "data/x.csv", "/home/u/x.csv", "file:///C:/data/x.csv", "file://localhost/home/u/x.csv", "file:///home/u/x.csv"] {
            assert!(!is_network_path(p), "{p}");
        }
        let s = Source::Workbook { path: "\\\\h\\s\\book.xlsx".into(), item: String::new(), header: true };
        assert!(s.is_remote());
        let s = Source::Workbook { path: "book.xlsx".into(), item: String::new(), header: true };
        assert!(!s.is_remote());
    }
}
