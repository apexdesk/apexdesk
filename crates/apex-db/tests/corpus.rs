//! Reads the test databases of target/corpus-db when they are there
//! (jackcess, mdbtools and dbf test files); does nothing otherwise.

use std::path::{Path, PathBuf};

use apex_db::Value;

fn corpus(rel: &str) -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/corpus-db").join(rel);
    p.exists().then_some(p)
}

fn cell(t: &apex_db::Table, row: usize, col: &str) -> String {
    let i = t.columns.iter().position(|c| c.name == col).unwrap();
    t.rows[row][i].to_text()
}

#[test]
fn northwind_jet3() {
    let Some(p) = corpus("mdbtestdata/data/nwind.mdb") else { return };
    let db = apex_db::open(&p).unwrap();
    let c = db.table("Customers").unwrap();
    assert_eq!(c.rows.len(), 91);
    assert!(!c.primary_key().is_empty());
    assert!(!db.relations.is_empty());
    assert!(db.queries.iter().any(|q| q.sql.to_ascii_uppercase().starts_with("SELECT")));
    // Its forms, reports and modules are counted, not imported.
    let l = db.left_out;
    assert!(l.forms > 5 && l.reports > 5 && l.modules > 0, "{l:?}");
}

#[test]
fn northwind_into_sqlite() {
    let Some(p) = corpus("mdbtestdata/data/nwind.mdb") else { return };
    let src = apex_db::open(&p).unwrap();
    let mut db = apex_db::sqlite::Db::in_memory().unwrap();
    let rep = apex_db::import::import(&src, &mut db).unwrap();
    assert_eq!(rep.tables, src.tables.iter().filter(|t| t.linked.is_none()).count());
    let all = apex_db::sqlite::Filter::default();
    assert_eq!(db.count(&apex_db::sqlite::RowSource::Table("Customers".into()), &all).unwrap(), 91);
    assert!(db.relations().iter().any(|r| r.enforce));
    // Most saved queries run in SQLite.
    assert!(rep.untranslated.len() * 3 < rep.queries, "{:?}", rep.untranslated);
}

#[test]
fn calculated_columns() {
    let Some(p) = corpus("jackcess/src/test/data/V2010/calcFieldTestV2010.accdb") else { return };
    let t = apex_db::read_table(&p, "Table1").unwrap();
    assert_eq!(cell(&t, 0, "LastFirst"), "Wayne, Bruce");
    assert_eq!(cell(&t, 0, "MonthlySalary"), "83333.3333");
    assert_eq!(cell(&t, 0, "WeeklySalary"), "19230.7692307692");
    assert_eq!(cell(&t, 0, "DecimalTest"), "50.325");
    assert_eq!(cell(&t, 0, "AllNames"), "Wayne, Bruce=Wayne, Bruce");
    assert!(t.columns.iter().any(|c| !c.expression.is_empty()));
}

#[test]
fn extended_dates() {
    let Some(p) = corpus("jackcess/src/test/data/V2019/extDateTestV2019.accdb") else { return };
    let t = apex_db::read_table(&p, "Table1").unwrap();
    assert_eq!(cell(&t, 6, "DateExt"), "1765-06-14 12:45:00");
    assert_eq!(cell(&t, 7, "DateExt"), "0100-06-14 12:45:00");
}

#[test]
fn attachments_and_multi_values() {
    let Some(p) = corpus("jackcess/src/test/data/V2010/complexDataTestV2010.accdb") else { return };
    let t = apex_db::read_table(&p, "Table1").unwrap();
    assert!(t.rows.iter().flatten().any(|v| matches!(v, Value::Attachments(a) if !a.is_empty() && !a[0].data.is_empty())));
}

#[test]
fn encoded_and_encrypted() {
    if let Some(p) = corpus("jencrypt/src/test/data/db-enc.mdb") {
        assert_eq!(apex_db::read_table(&p, "Table1").unwrap().rows.len(), 2);
    }
    if let Some(p) = corpus("jencrypt/src/test/data/db2013-enc.accdb") {
        assert_eq!(apex_db::open(&p).unwrap_err(), apex_db::Error::Encrypted);
    }
}

#[test]
fn dbase_files() {
    if let Some(p) = corpus("rubydbf/spec/fixtures/dbase_02.dbf") {
        let t = apex_db::read_table(&p, "").unwrap();
        assert_eq!(t.rows.len(), 9);
        assert_eq!(cell(&t, 0, "LAST"), "Stegman");
    }
    if let Some(p) = corpus("rubydbf/spec/fixtures/dbase_03_cyrillic.dbf") {
        let t = apex_db::read_table(&p, "").unwrap();
        assert_eq!(t.columns[0].name, "ШАР");
    }
    if let Some(p) = corpus("rubydbf/spec/fixtures/dbase_8b.dbf") {
        let t = apex_db::read_table(&p, "").unwrap();
        assert!(t.rows[0].iter().any(|v| v.to_text().trim_end() == "First memo"));
    }
    if let Some(p) = corpus("rubydbf/spec/fixtures/dbase_f5.dbf") {
        let t = apex_db::read_table(&p, "").unwrap();
        assert!(t.rows[0].iter().any(|v| v.to_text() == "baix penedès"));
    }
}
