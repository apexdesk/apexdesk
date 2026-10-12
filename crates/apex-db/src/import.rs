//! Importing a database read from another program (Access, dBase) into a
//! SQLite one: tables with their fields' types, keys, indexes, defaults and
//! validation rules; rows; relations (enforced ones as foreign keys, the
//! others drawn on the diagram); and saved queries translated to SQLite's
//! SQL (select ones also become views).
//!
//! What can't be kept is said in the report's warnings: attachments go to a
//! table of their own (`<table>_<field>`), rules and defaults SQLite
//! doesn't take are dropped, queries it can't run are saved with their
//! Access SQL.

use std::path::Path;

use crate::model::{ColType, Database, Relation, Table, Value};
use crate::objects::Record;
use crate::query::{from_access, translatable};
use crate::sqlite::{q, Db, FieldDef, FieldType, IndexDef, TableDesign, ForeignKey, RefAction};
use crate::Error;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub tables: usize,
    pub rows: usize,
    pub relations: usize,
    pub queries: usize,
    /// Queries saved with SQL that doesn't run in SQLite.
    pub untranslated: Vec<String>,
    pub warnings: Vec<String>,
}

fn field_type(ty: ColType) -> FieldType {
    match ty {
        ColType::Bool => FieldType::Bool,
        ColType::Byte | ColType::Int16 | ColType::Int32 | ColType::Int64 => FieldType::Integer,
        ColType::Money | ColType::Decimal => FieldType::Currency,
        ColType::Single | ColType::Double => FieldType::Number,
        ColType::DateTime => FieldType::DateTime,
        ColType::Binary | ColType::Ole => FieldType::Blob,
        ColType::Text | ColType::Guid => FieldType::Text,
        ColType::Memo | ColType::Attachment | ColType::MultiValue => FieldType::LongText,
    }
}

/// An Access default value ("=Date()", "0", "\"none\"") as SQL.
fn default_sql(d: &str) -> String {
    let d = d.trim().trim_start_matches('=').trim();
    if d.is_empty() {
        return String::new();
    }
    let up = d.to_ascii_uppercase();
    match up.as_str() {
        "DATE()" => "CURRENT_DATE".into(),
        "NOW()" => "CURRENT_TIMESTAMP".into(),
        "TIME()" => "CURRENT_TIME".into(),
        _ => from_access(d),
    }
}

/// The design of a table read from another program.
pub fn design_of(t: &Table, rels: &[Relation]) -> TableDesign {
    let pk = t.primary_key();
    let mut fields = Vec::new();
    for c in &t.columns {
        let mut ty = field_type(c.ty);
        let is_pk = pk.iter().any(|p| p.eq_ignore_ascii_case(&c.name));
        if c.auto && c.ty.is_integer() && is_pk && pk.len() == 1 {
            ty = FieldType::AutoNumber;
        }
        let mut f = FieldDef::new(&c.name, ty);
        f.size = if ty == FieldType::Text { if c.ty == ColType::Guid { 38 } else { c.size } } else { 0 };
        f.required = c.required;
        f.pk = is_pk;
        f.default = default_sql(&c.default);
        f.check = if c.validation.trim().is_empty() { String::new() } else { from_access(c.validation.trim()) };
        f.validation_text = c.validation_text.clone();
        f.description = c.description.clone();
        fields.push(f);
    }
    let mut indexes = Vec::new();
    for i in &t.indexes {
        if i.primary || i.foreign || i.columns.is_empty() {
            continue;
        }
        indexes.push(IndexDef { name: format!("{}_{}", t.name, i.name), columns: i.columns.iter().map(|c| c.0.clone()).collect(), unique: i.unique });
    }
    let foreign_keys = rels
        .iter()
        .filter(|r| r.enforce && r.table.eq_ignore_ascii_case(&t.name))
        .map(|r| ForeignKey {
            columns: r.columns.clone(),
            ref_table: r.ref_table.clone(),
            ref_columns: r.ref_columns.clone(),
            on_update: if r.cascade_update { RefAction::Cascade } else { RefAction::Restrict },
            on_delete: if r.cascade_delete { RefAction::Cascade } else { RefAction::Restrict },
        })
        .collect();
    TableDesign { name: t.name.clone(), fields, indexes, foreign_keys }
}

/// Puts `src` into `db` (which should be new: tables of the same names are
/// replaced).
pub fn import(src: &Database, db: &mut Db) -> Result<Report, Error> {
    let mut rep = Report { warnings: src.warnings.clone(), ..Default::default() };
    db.conn().execute_batch("PRAGMA foreign_keys = OFF").map_err(|e| Error::Db(e.to_string()))?;
    let r = import_inner(src, db, &mut rep);
    db.conn().execute_batch("PRAGMA foreign_keys = ON").map_err(|e| Error::Db(e.to_string()))?;
    r?;
    Ok(rep)
}

fn exec(db: &Db, sql: &str) -> Result<(), Error> {
    db.conn().execute_batch(sql).map_err(|e| Error::Db(e.to_string()))
}

fn import_inner(src: &Database, db: &mut Db, rep: &mut Report) -> Result<(), Error> {
    let existing = db.table_names();
    let tables: Vec<&Table> = src.tables.iter().filter(|t| t.linked.is_none()).collect();
    for t in src.tables.iter().filter(|t| t.linked.is_some()) {
        rep.warnings.push(format!("{}: linked table, not imported", t.name));
    }
    // Every table first (relations may name tables made later), without
    // indexes and rules: they go on after the rows, and are dropped when the
    // rows break them.
    let mut designs = Vec::new();
    for t in &tables {
        if existing.iter().any(|e| e.eq_ignore_ascii_case(&t.name)) {
            db.drop_table(&t.name)?;
        }
        let mut full = design_of(t, &src.relations);
        for (i, f) in full.fields.iter_mut().enumerate() {
            if f.required && t.rows.iter().any(|r| r.get(i).is_none_or(Value::is_null)) {
                rep.warnings.push(format!("{}.{}: rows without a value, not required", t.name, f.name));
                f.required = false;
            }
        }
        let mut plain = full.clone();
        plain.indexes.clear();
        for f in &mut plain.fields {
            f.check.clear();
        }
        if db.apply_design(None, &plain).is_err() {
            for (f, g) in plain.fields.iter_mut().zip(&mut full.fields) {
                if !f.default.is_empty() {
                    rep.warnings.push(format!("{}.{}: default value {} not kept", t.name, f.name, f.default));
                    f.default.clear();
                    g.default.clear();
                }
            }
            db.apply_design(None, &plain)?;
        }
        designs.push(full);
    }
    for (t, full) in tables.iter().zip(&mut designs) {
        rep.rows += insert_rows(db, t)?;
        attachments(db, t, rep)?;
        // Under the time limit of reads: the rules come from the file.
        let has = |sql: &str| db.bounded(|c| c.prepare(sql).and_then(|mut s| s.query_row([], |r| r.get::<_, i64>(0))).map_err(|e| Error::Db(e.to_string()))).ok();
        // Each rule alone: it must be SQL, not change with the time, and hold for the rows.
        for f in &mut full.fields {
            f.from = Some(f.name.clone());
            if f.check.is_empty() {
                continue;
            }
            let rule = crate::sqlite::rule_sql(&f.name, &f.check);
            // Nor read other tables (a CHECK can't, and a query could run for ever).
            let timely = ["'now'", "CURRENT_", "random", "SELECT", "WITH", "blob("].iter().any(|w| rule.to_ascii_uppercase().contains(&w.to_ascii_uppercase()));
            if timely || has(&format!("SELECT count(*) FROM {} WHERE NOT ({rule})", q(&t.name))) != Some(0) {
                rep.warnings.push(format!("{}.{}: validation rule {} not kept", t.name, f.name, f.check));
                f.check.clear();
            }
        }
        let cols: Vec<&str> = t.columns.iter().filter(|c| matches!(c.ty, ColType::Attachment | ColType::MultiValue)).map(|c| c.name.as_str()).collect();
        full.indexes.retain(|i| {
            if i.columns.iter().any(|c| cols.contains(&c.as_str())) {
                return false;
            }
            let list = i.columns.iter().map(|c| q(c)).collect::<Vec<_>>().join(", ");
            let not_null = i.columns.iter().map(|c| format!("{} IS NOT NULL", q(c))).collect::<Vec<_>>().join(" AND ");
            let dup = i.unique && has(&format!("SELECT count(*) FROM (SELECT 1 FROM {} WHERE {not_null} GROUP BY {list} HAVING count(*) > 1)", q(&t.name))) != Some(0);
            if dup {
                rep.warnings.push(format!("{}.{}: repeated values, index not unique", t.name, i.name));
            }
            !dup
        });
        for i in &mut full.indexes {
            if db.conn().execute_batch(&crate::sqlite::index_sql(&t.name, i)).is_err() {
                i.unique = false;
            }
        }
        if full.fields.iter().any(|f| !f.check.is_empty()) {
            // Indexes are made again by the rebuild.
            for i in &full.indexes {
                let name = if i.name.is_empty() { format!("{}_{}", t.name, i.columns.join("_")) } else { i.name.clone() };
                let _ = db.conn().execute_batch(&format!("DROP INDEX IF EXISTS {}", q(&name)));
            }
            if db.apply_design(Some(&t.name), full).is_err() {
                rep.warnings.push(format!("{}: validation rules not kept", t.name));
                for f in &mut full.fields {
                    f.check.clear();
                }
                db.apply_design(Some(&t.name), full)?;
            }
        }
        rep.tables += 1;
    }
    for r in &src.relations {
        if !r.enforce {
            if db.add_relation(r).is_ok() {
                rep.relations += 1;
            }
        } else {
            rep.relations += 1;
        }
    }
    let bad: i64 = db.conn().query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r.get(0)).unwrap_or(0);
    if bad > 0 {
        rep.warnings.push(format!("{bad} rows point to rows that don't exist"));
    }
    // Queries using others are made after them: passes until none is added.
    let mut left: Vec<&crate::model::SavedQuery> = src.queries.iter().collect();
    loop {
        let before = left.len();
        let mut next = Vec::new();
        for sq in left {
            let sql = if translatable(&sq.sql) { from_access(&sq.sql) } else { String::new() };
            let mut rec = Record::default();
            rec.set("sql", &sql).set("type", kind_key(sq.kind)).set("access_sql", &sq.sql);
            if !sql.is_empty() && db.save_query(&sq.name, &[rec.clone()])? {
                rep.queries += 1;
            } else {
                next.push(sq);
            }
        }
        left = next;
        if left.is_empty() || left.len() == before {
            break;
        }
    }
    for sq in left {
        let mut rec = Record::default();
        rec.set("sql", if translatable(&sq.sql) { from_access(&sq.sql) } else { String::new() }).set("type", kind_key(sq.kind)).set("access_sql", &sq.sql).set_flag("untranslated", true);
        db.save_query(&sq.name, &[rec])?;
        rep.queries += 1;
        rep.untranslated.push(sq.name.clone());
    }
    Ok(())
}

fn kind_key(k: crate::model::QueryKind) -> &'static str {
    use crate::model::QueryKind as K;
    match k {
        K::Select | K::Union => "select",
        K::MakeTable => "maketable",
        K::Append => "append",
        K::Update => "update",
        K::Delete => "delete",
        _ => "other",
    }
}

fn insert_rows(db: &Db, t: &Table) -> Result<usize, Error> {
    if t.rows.is_empty() {
        return Ok(0);
    }
    exec(db, "BEGIN")?;
    let cols = t.columns.iter().map(|c| q(&c.name)).collect::<Vec<_>>().join(", ");
    let marks = (1..=t.columns.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
    let r = (|| {
        let mut st = db.conn().prepare(&format!("INSERT INTO {} ({cols}) VALUES ({marks})", q(&t.name))).map_err(|e| Error::Db(e.to_string()))?;
        for row in &t.rows {
            st.execute(rusqlite::params_from_iter(row.iter().map(crate::sqlite::to_sql))).map_err(|e| Error::Db(format!("{}: {e}", t.name)))?;
        }
        Ok(t.rows.len())
    })();
    exec(db, if r.is_ok() { "COMMIT" } else { "ROLLBACK" })?;
    r
}

/// Attachment fields' files, in a table `<table>_<field>` (Row is the
/// row's rowid in the table).
fn attachments(db: &mut Db, t: &Table, rep: &mut Report) -> Result<(), Error> {
    for (ci, c) in t.columns.iter().enumerate() {
        if c.ty != ColType::Attachment {
            continue;
        }
        let name = format!("{}_{}", t.name, c.name);
        let mut id = FieldDef::new("Id", FieldType::AutoNumber);
        id.pk = true;
        let d = TableDesign { name: name.clone(), fields: vec![id, FieldDef::new("Row", FieldType::Integer), FieldDef::new("FileName", FieldType::Text), FieldDef::new("Data", FieldType::Blob)], ..Default::default() };
        if db.table_names().iter().any(|n| n.eq_ignore_ascii_case(&name)) {
            db.drop_table(&name)?;
        }
        db.apply_design(None, &d)?;
        let mut files = Table::new(name.clone(), vec![crate::model::Column::new("Row", ColType::Int64), crate::model::Column::new("FileName", ColType::Text), crate::model::Column::new("Data", ColType::Ole)]);
        for (ri, row) in t.rows.iter().enumerate() {
            if let Some(Value::Attachments(a)) = row.get(ci) {
                for f in a {
                    files.rows.push(vec![Value::Int(ri as i64 + 1), Value::Text(f.name.clone()), Value::Blob(f.data.clone())]);
                }
            }
        }
        insert_rows(db, &files)?;
        rep.warnings.push(format!("{}.{}: attachments are in table {name}", t.name, c.name));
    }
    Ok(())
}

/// Reads an Access or dBase file and puts it in a new SQLite database at
/// `dst` (replaced if it exists).
pub fn import_file(src: &Path, dst: &Path) -> Result<Report, Error> {
    let data = crate::open(src)?;
    let mut db = Db::create(dst)?;
    import(&data, &mut db)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Column, Index, QueryKind, SavedQuery};
    use crate::sqlite::{Filter, RowSource};

    #[test]
    fn access_to_sqlite() {
        let mut id = Column::new("ID", ColType::Int32);
        id.auto = true;
        let mut name = Column::new("Name", ColType::Text);
        name.size = 50;
        name.validation = "<> \"x\"".into();
        let mut cust = Table::new("Customers", vec![id, name]);
        cust.indexes.push(Index { name: "PrimaryKey".into(), columns: vec![("ID".into(), true)], primary: true, ..Default::default() });
        cust.rows = vec![vec![Value::Int(1), Value::Text("Ana".into())], vec![Value::Int(2), Value::Text("Bea".into())]];
        let mut ord = Table::new("Orders", vec![Column::new("OrderID", ColType::Int32), Column::new("CustomerID", ColType::Int32), Column::new("Amount", ColType::Money), Column::new("Files", ColType::Attachment)]);
        ord.indexes.push(Index { name: "PrimaryKey".into(), columns: vec![("OrderID".into(), true)], primary: true, ..Default::default() });
        ord.rows = vec![vec![Value::Int(10), Value::Int(1), Value::Money(125000), Value::Attachments(vec![crate::model::Attachment { name: "a.txt".into(), data: b"hi".to_vec() }])]];
        let src = Database {
            tables: vec![cust, ord],
            relations: vec![Relation { name: "CustOrders".into(), table: "Orders".into(), columns: vec!["CustomerID".into()], ref_table: "Customers".into(), ref_columns: vec!["ID".into()], enforce: true, cascade_delete: true, ..Default::default() }],
            queries: vec![
                SavedQuery { name: "Big".into(), kind: QueryKind::Select, sql: "SELECT * FROM [Orders] WHERE Amount > 10;".into() },
                SavedQuery { name: "BigNames".into(), kind: QueryKind::Select, sql: "SELECT Name FROM Customers INNER JOIN Big ON Customers.ID = Big.CustomerID".into() },
                SavedQuery { name: "Cross".into(), kind: QueryKind::Crosstab, sql: "TRANSFORM Sum(Amount) SELECT CustomerID FROM Orders GROUP BY CustomerID PIVOT OrderID".into() },
            ],
            warnings: vec![],
        };
        let mut db = Db::in_memory().unwrap();
        let rep = import(&src, &mut db).unwrap();
        assert_eq!((rep.tables, rep.rows, rep.relations, rep.queries), (2, 3, 1, 3));
        assert_eq!(rep.untranslated, ["Cross"]);
        let d = db.design("Customers").unwrap();
        assert_eq!(d.fields[0].ty, FieldType::AutoNumber);
        assert_eq!(d.fields[1].check, "<> 'x'");
        assert_eq!(db.relations().len(), 1);
        let p = db.page(&RowSource::Table("BigNames".into()), &Filter::default(), &[], 0, 10).unwrap();
        assert_eq!(p.rows, vec![vec![Value::Text("Ana".into())]]);
        assert_eq!(db.count(&RowSource::Table("Orders_Files".into()), &Filter::default()).unwrap(), 1);
        // Enforced: deleting a customer deletes its orders.
        db.delete_row("Customers", 1).unwrap();
        assert_eq!(db.count(&RowSource::Table("Orders".into()), &Filter::default()).unwrap(), 0);
    }
}
