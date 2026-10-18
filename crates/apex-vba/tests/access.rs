//! Macros of databases: Access's objects on a database held in memory.

mod common;

use apex_vba::access::{Database, DbRequest, FormState, GoTo, Rows};
use apex_vba::value::Value;
use apex_vba::{DbOutcome, Options};
use common::TestUi;

/// A table "People" (Id, Name, Age), and the SQL it was asked.
#[derive(Default)]
struct Mem {
    cols: Vec<String>,
    rows: Vec<(i64, Vec<Value>)>,
    next: i64,
    sql: Vec<String>,
}

impl Mem {
    fn new() -> Mem {
        let p = |id: i32, n: &str, a: i32| (id as i64, vec![Value::Long(id), Value::str(n), Value::Long(a)]);
        Mem { cols: vec!["Id".into(), "Name".into(), "Age".into()], rows: vec![p(1, "Ann", 30), p(2, "Bob", 41), p(3, "Cy", 25)], next: 4, sql: Vec::new() }
    }

    fn col(&self, name: &str) -> usize {
        self.cols.iter().position(|c| c.eq_ignore_ascii_case(name)).unwrap()
    }
}

impl Database for Mem {
    fn rows(&mut self, source: &str) -> Result<Rows, String> {
        if source.eq_ignore_ascii_case("people") {
            return Ok(Rows { columns: self.cols.clone(), rows: self.rows.iter().map(|r| r.1.clone()).collect(), table: Some(("People".into(), self.rows.iter().map(|r| r.0).collect())) });
        }
        if source.eq_ignore_ascii_case("adults") {
            return Ok(Rows { columns: vec!["Name".into()], rows: vec![vec![Value::str("Bob")]], table: None });
        }
        // SQL: one value, how many rows there are.
        self.sql.push(source.to_string());
        Ok(Rows { columns: vec!["x".into()], rows: vec![vec![Value::Long(self.rows.len() as i32)]], table: None })
    }

    fn execute(&mut self, sql: &str) -> Result<usize, String> {
        self.sql.push(sql.to_string());
        if sql.starts_with("DELETE") || sql == "Purge" {
            let n = self.rows.len();
            self.rows.clear();
            return Ok(n);
        }
        Err("no such table: Nope".into())
    }

    fn is_action_query(&mut self, name: &str) -> Option<bool> {
        match name {
            "Purge" => Some(true),
            "Adults" => Some(false),
            _ => None,
        }
    }

    fn update(&mut self, _table: &str, rowid: i64, values: &[(String, Value)]) -> Result<(), String> {
        let row = self.rows.iter().position(|r| r.0 == rowid).ok_or("no row")?;
        for (c, v) in values {
            let i = self.col(c);
            self.rows[row].1[i] = v.clone();
        }
        Ok(())
    }

    fn insert(&mut self, _table: &str, values: &[(String, Value)]) -> Result<i64, String> {
        let id = self.next;
        self.next += 1;
        let mut row = vec![Value::Null; self.cols.len()];
        row[0] = Value::Long(id as i32);
        for (c, v) in values {
            let i = self.col(c);
            row[i] = v.clone();
        }
        self.rows.push((id, row));
        Ok(id)
    }

    fn delete(&mut self, _table: &str, rowid: i64) -> Result<(), String> {
        self.rows.retain(|r| r.0 != rowid);
        Ok(())
    }
}

fn form() -> FormState {
    FormState {
        name: "People".into(),
        caption: "People".into(),
        source: "People".into(),
        fields: vec![("Id".into(), Value::Long(2)), ("Name".into(), Value::str("Bob")), ("Age".into(), Value::Long(41))],
        controls: vec![("txtName".into(), "Name".into())],
        ..Default::default()
    }
}

fn run(db: &mut Mem, body: &str) -> (DbOutcome, TestUi) {
    let p = common::project(&format!("Sub Main()\n{body}\nEnd Sub\n"));
    let mut ui = TestUi::default();
    let out = apex_vba::run_db_macro(&p, db, vec![form()], Some("People".into()), &mut ui, &Options::default(), "Main");
    (out, ui)
}

fn ok(db: &mut Mem, body: &str) -> (Vec<String>, Vec<DbRequest>) {
    let (out, ui) = run(db, body);
    if let Some(e) = out.error {
        panic!("line {}: {}", e.line, e.message);
    }
    (ui.printed, out.requests)
}

#[test]
fn walks_a_recordset() {
    let mut db = Mem::new();
    let (p, _) = ok(
        &mut db,
        r#"Dim rs, n
Set rs = CurrentDb.OpenRecordset("People", dbOpenDynaset)
Debug.Print rs.RecordCount
Do Until rs.EOF
    n = n + rs!Age
    Debug.Print rs("Name") & " " & rs.Fields("Age").Value
    rs.MoveNext
Loop
Debug.Print n
rs.MoveLast
Debug.Print rs.Fields(1).Name & "=" & rs!Name
rs.Close"#,
    );
    assert_eq!(p, [" 3 ", "Ann 30", "Bob 41", "Cy 25", " 96 ", "Name=Cy"]);
}

#[test]
fn changes_rows_through_a_recordset() {
    let mut db = Mem::new();
    let (out, _) = run(
        &mut db,
        r#"Dim rs
Set rs = CurrentDb.OpenRecordset("People")
rs.Edit
rs!Age = 31
rs.Update
rs.AddNew
rs!Name = "Dee"
rs("Age") = 19
rs.Update
rs.MoveFirst
rs.MoveNext
rs.Delete"#,
    );
    assert!(out.error.is_none(), "{:?}", out.error);
    assert!(out.changed);
    let names: Vec<String> = db.rows.iter().map(|r| r.1[1].to_str().unwrap()).collect();
    assert_eq!(names, ["Ann", "Cy", "Dee"]);
    assert_eq!(db.rows[0].1[2].to_i64().unwrap(), 31);
    assert_eq!(db.rows[2].1[2].to_i64().unwrap(), 19);
    // A field changed without Edit: Access's error 3020.
    let (out, _) = run(&mut db, "Dim rs\nSet rs = CurrentDb.OpenRecordset(\"People\")\nrs!Age = 1");
    assert_eq!(out.error.unwrap().number, 3020);
    // A query's rows can't be changed.
    let (out, _) = run(&mut db, "Dim rs\nSet rs = CurrentDb.OpenRecordset(\"Adults\")\nrs.Edit");
    assert_eq!(out.error.unwrap().number, 3027);
}

#[test]
fn runs_sql_and_domain_functions() {
    let mut db = Mem::new();
    let (p, _) = ok(&mut db, "Debug.Print DCount(\"*\", \"People\", \"Age > 30\")\nDebug.Print Nz(Null, 7)");
    assert_eq!(p, [" 3 ", " 7 "]);
    assert_eq!(db.sql, ["SELECT COUNT(*) FROM [People] WHERE Age > 30"]);
    let (out, _) = run(&mut db, "CurrentDb.Execute \"DELETE FROM People\", dbFailOnError\nDebug.Print CurrentDb.RecordsAffected");
    assert!(out.changed);
    assert!(db.rows.is_empty());
    // The database's refusal stops the macro, as an error it can trap.
    let (p, _) = ok(&mut db, "On Error Resume Next\nDoCmd.RunSQL \"UPDATE Nope SET x = 1\"\nDebug.Print Err.Number");
    assert_eq!(p, [" 3061 "]);
}

#[test]
fn reads_and_changes_the_form() {
    let mut db = Mem::new();
    let (p, r) = ok(
        &mut db,
        r#"Debug.Print Me!txtName & " " & Me!Age & " " & Me.Name
Debug.Print Forms("People")!Name
Debug.Print Forms!People.RecordSource
Me!txtName = "Robert"
Debug.Print Me.Dirty & " " & Me!Name
Me.Filter = "Age > 30"
Me.FilterOn = True
DoCmd.GoToRecord , , acNext
DoCmd.GoToRecord , , acNewRec
DoCmd.OpenForm "Orders", , , "PersonId = 2"
DoCmd.OpenQuery "Adults"
DoCmd.OpenQuery "Purge"
DoCmd.Close"#,
    );
    assert_eq!(p, ["Bob 41 People", "Bob", "People", "True Robert"]);
    assert_eq!(
        r,
        [
            DbRequest::SetField { form: "People".into(), field: "Name".into(), value: "Robert".into() },
            DbRequest::Filter { form: "People".into(), filter: Some("Age > 30".into()) },
            DbRequest::GoTo { form: "People".into(), to: GoTo::Next },
            DbRequest::GoTo { form: "People".into(), to: GoTo::New },
            DbRequest::OpenForm { name: "Orders".into(), filter: Some("PersonId = 2".into()) },
            DbRequest::OpenQuery("Adults".into()),
            DbRequest::Close("People".into()),
        ]
    );
    // The action query ran at once.
    assert!(db.rows.is_empty());
    let (out, _) = run(&mut db, "Debug.Print Me!Nope");
    assert_eq!(out.error.unwrap().number, 2465);
    // Excel's objects aren't there.
    let (out, _) = run(&mut db, "Range(\"A1\").Value = 1");
    assert!(out.error.is_some());
}
