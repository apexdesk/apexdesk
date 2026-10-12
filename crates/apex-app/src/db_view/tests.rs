//! Tests of the database tab without a window: designs, edits, forms,
//! queries, reports and imports, with their undo.

use std::path::Path;

use apex_db::forms::{Action, FormKind};
use apex_db::objects::Kind;
use apex_db::sqlite::{Db, FieldDef, FieldType, Filter, RowSource};
use apex_db::{Relation, Value};

use super::*;

fn view() -> DbView {
    DbView::new(Db::in_memory().unwrap(), Path::new("Test.apexdb"))
}

/// Customers(Id, Name, Vip) and Orders(Id, Customer, Amount) with rows.
fn sample() -> DbView {
    let mut v = view();
    let mut d = design::Designer::new(None, design::Designer::blank(&v.db));
    d.design.name = "Customers".into();
    let mut name = FieldDef::new("Name", FieldType::Text);
    name.required = true;
    d.design.fields.push(name);
    d.design.fields.push(FieldDef::new("Vip", FieldType::Bool));
    assert!(d.save(&mut v));
    let mut d = design::Designer::new(None, design::Designer::blank(&v.db));
    d.design.name = "Orders".into();
    d.design.fields.push(FieldDef::new("Customer", FieldType::Integer));
    let mut amount = FieldDef::new("Amount", FieldType::Number);
    amount.check = ">= 0".into();
    amount.validation_text = "No negative amounts".into();
    d.design.fields.push(amount);
    assert!(d.save(&mut v));
    for (n, vip) in [("Ann", true), ("Bob", false)] {
        v.db.insert_row("Customers", &["Name".into(), "Vip".into()], &[Value::Text(n.into()), Value::Bool(vip)], None).unwrap();
    }
    for (c, a) in [(1, 10.0), (1, 5.0), (2, 7.0)] {
        v.db.insert_row("Orders", &["Customer".into(), "Amount".into()], &[Value::Num(c as f64), Value::Num(a)], None).unwrap();
    }
    v.confirmed(ConfirmAction::Relation(Relation { name: "co".into(), table: "Orders".into(), columns: vec!["Customer".into()], ref_table: "Customers".into(), ref_columns: vec!["Id".into()], enforce: true, ..Default::default() }, true));
    v.undo.clear();
    v
}

fn count(v: &DbView, t: &str) -> usize {
    v.db.count(&RowSource::Table(t.into()), &Filter::default()).unwrap()
}

#[test]
fn designs_and_undo() {
    let mut v = sample();
    assert_eq!(v.db.table_names(), ["Customers", "Orders"]);
    assert_eq!(v.db.relations().len(), 1);
    // A new field, then undone: the table is as before, rows kept.
    let mut d = design::Designer::new(Some("Customers"), v.db.design("Customers").unwrap());
    d.design.fields.push(FieldDef::new("City", FieldType::Text));
    assert!(d.save(&mut v));
    assert!(v.db.design("Customers").unwrap().field("City").is_some());
    v.undo();
    assert!(v.db.design("Customers").unwrap().field("City").is_none());
    assert_eq!(count(&v, "Customers"), 2);
    v.redo();
    assert!(v.db.design("Customers").unwrap().field("City").is_some());
    // Two AutoNumbers aren't a design.
    let mut d = design::Designer::new(None, design::Designer::blank(&v.db));
    d.design.fields.push(FieldDef::new("Other", FieldType::AutoNumber));
    assert!(d.problem().is_some());
    assert!(!d.save(&mut v));
}

/// The zoom of the status bar enlarges the datasheet's text.
#[test]
fn datasheet_zooms() {
    let mut v = sample();
    let mut s = sheet::Datasheet::new(sheet::Src::Table("Customers".into()));
    s.load(&mut v);
    let ctx = egui::Context::default();
    let mut height = |v: &mut DbView| {
        let mut out = ctx.run_ui(Default::default(), |ui| s.grid(ui, v));
        out.textures_delta.clear();
        out.shapes.iter().find_map(|c| match &c.shape {
            egui::Shape::Text(t) if t.galley.text() == "Ann" => Some(t.galley.size().y),
            _ => None,
        }).expect("Ann drawn")
    };
    let one = height(&mut v);
    v.zoom = 2.0;
    let two = height(&mut v);
    assert!(two > one * 1.8, "{one} → {two}");
}

#[test]
fn datasheet_edits_and_rules() {
    let mut v = sample();
    let mut s = sheet::Datasheet::new(sheet::Src::Table("Orders".into()));
    s.load(&mut v);
    assert_eq!(s.total, 3);
    let amount = s.page.columns.iter().position(|c| c == "Amount").unwrap();
    // The rule refuses a negative amount with its message.
    s.edit = Some((0, amount, "-3".into()));
    s.commit(&mut v);
    assert_eq!(s.error.as_deref(), Some("No negative amounts"));
    s.edit = Some((0, amount, "12".into()));
    s.commit(&mut v);
    s.load(&mut v);
    assert_eq!(s.page.rows[0][amount], Value::Num(12.0));
    v.undo();
    s.load(&mut v);
    assert_eq!(s.page.rows[0][amount], Value::Num(10.0));
    // A row pointing nowhere is refused by the relation.
    let cust = s.page.columns.iter().position(|c| c == "Customer").unwrap();
    s.new_row[cust] = "99".into();
    s.insert_new(&mut v);
    assert_eq!(s.error.as_deref(), Some(tr("db.fk_failed")));
    s.new_row[cust] = "2".into();
    s.insert_new(&mut v);
    assert_eq!(count(&v, "Orders"), 4);
    s.load(&mut v);
    s.delete_row(&mut v, 0);
    assert_eq!(count(&v, "Orders"), 3);
    v.undo();
    v.undo();
    assert_eq!(count(&v, "Orders"), 3);
    assert_eq!(v.db.page(&RowSource::Table("Orders".into()), &Filter::default(), &[], 0, 10).unwrap().rows.len(), 3);
}

#[test]
fn forms_run_records() {
    let mut v = sample();
    let w = {
        let mut w = wizards::Wizard::new(wizards::WizKind::Form, &v.db);
        w.set_source(&v.db, "Customers");
        w.form_kind = FormKind::WithSubform;
        w
    };
    let def = w.form(&v.db);
    assert_eq!(def.sub_source, "Orders");
    assert_eq!(def.child_field, "Customer");
    w.finish(&mut v);
    assert_eq!(v.db.objects(Kind::Form), ["Customers"]);
    let Some(Pane::Form(f)) = v.panes.pop() else { panic!("the form opens") };
    let mut f = f;
    f.load(&v);
    assert_eq!((f.pos, f.total), (0, 2));
    assert_eq!(f.values[f.cols.iter().position(|c| c == "Name").unwrap()], "Ann");
    f.act(&mut v, Action::Next, "");
    f.load(&v);
    assert_eq!(f.values[1], "Bob");
    // Editing and moving saves.
    f.values[1] = "Bobby".into();
    f.act(&mut v, Action::Previous, "");
    f.load(&v);
    assert_eq!(f.pos, 0);
    assert_eq!(cell_at(&v, "Customers", 1, "Name"), Value::Text("Bobby".into()));
    // A new record without its required name is refused.
    f.act(&mut v, Action::New, "");
    f.values[2] = "1".into();
    f.act(&mut v, Action::Save, "");
    assert!(f.error.is_some());
    f.values[1] = "Cy".into();
    f.act(&mut v, Action::Save, "");
    assert_eq!(count(&v, "Customers"), 3);
    f.load(&v);
    assert_eq!((f.pos, f.values[1].as_str()), (2, "Cy"));
    f.act(&mut v, Action::Delete, "");
    assert_eq!(count(&v, "Customers"), 2);
    v.undo();
    assert_eq!(count(&v, "Customers"), 3);
    f.act(&mut v, Action::Close, "");
    assert!(f.close);
    // Dropdown choices come from the other table.
    assert_eq!(form_run::choices(&v.db, "Customers", "Id", "Name")[0], ("1".to_string(), "Ann".to_string()));
}

fn cell_at(v: &DbView, t: &str, row: usize, col: &str) -> Value {
    let p = v.db.page(&RowSource::Table(t.into()), &Filter::default(), &[], row, 1).unwrap();
    let c = p.columns.iter().position(|x| x == col).unwrap();
    p.rows[0][c].clone()
}

#[test]
fn queries_select_and_act() {
    let mut v = sample();
    let mut q = query_ui::QueryEditor::new("Big");
    q.add_table(&v.db, "Customers");
    q.add_table(&v.db, "Orders");
    // The relation joins them.
    assert_eq!(q.design.joins.len(), 1);
    q.design.fields.push(apex_db::query::QField::column("Customers", "Name"));
    let mut amount = apex_db::query::QField::column("Orders", "Amount");
    amount.criteria = vec![">6".into()];
    q.design.fields.push(amount);
    q.run(&mut v);
    let Some(Pane::Sheet(s)) = v.panes.last_mut() else { panic!("the result opens") };
    let mut s = std::mem::replace(s, sheet::Datasheet::new(sheet::Src::Table(String::new())));
    s.load(&mut v);
    assert_eq!(s.total, 2);
    q.save(&mut v);
    assert!(v.db.view_names().contains(&"Big".to_string()));
    // Reopened, the design comes back.
    let back = query_ui::QueryEditor::load(&v.db, "Big");
    assert!(!back.sql_mode);
    assert_eq!(back.design.fields.len(), 2);
    // An action query asks, then runs as one undo step.
    let mut u = query_ui::QueryEditor::new("Raise");
    u.sql_mode = true;
    u.sql = "UPDATE Orders SET Amount = Amount * 2".into();
    u.run(&mut v);
    assert!(v.confirm.as_ref().is_some_and(|c| c.text.contains('3')));
    let c = v.confirm.take().unwrap();
    v.confirmed(c.action);
    assert_eq!(cell_at(&v, "Orders", 0, "Amount"), Value::Num(20.0));
    v.undo();
    assert_eq!(cell_at(&v, "Orders", 0, "Amount"), Value::Num(10.0));
    assert_eq!(query_ui::sql_kind("delete from x"), apex_db::query::QueryType::Delete);
}

#[test]
fn a_new_report_waits_for_its_source() {
    let mut v = sample();
    v.create_named(Kind::Report, "Blank");
    let Some(Pane::Report(r)) = v.panes.last() else { panic!("no report pane") };
    assert!(!r.has_source());
    // Printed, it is its title, not a database error.
    let text = v.print_doc().to_plain_text();
    assert!(text.contains("Blank") && !text.contains("no such table"), "{text}");
}

#[test]
fn reports_and_labels() {
    let mut v = sample();
    let mut w = wizards::Wizard::new(wizards::WizKind::Report, &v.db);
    w.set_source(&v.db, "Orders");
    w.group = "Customer".into();
    w.group_break = true;
    w.sums = vec!["Amount".into()];
    let def = w.report();
    assert_eq!(def.groups.len(), 1);
    assert!(def.fields.iter().all(|f| f.name != "Customer"));
    let doc = report_ui::document(&v.db, &def);
    let text = doc.to_plain_text();
    assert!(text.contains("Customer: 1") && text.contains("Customer: 2"), "{text}");
    // Page break before the second group.
    assert!(doc.paragraphs.iter().filter(|p| p.style.page_break_before).count() >= 1);
    assert!(text.contains("15"), "the first group's sum: {text}");
    let labels = report_ui::labels(&v.db, &apex_db::report::ReportDef::plain("L", "Customers", &["Name".into()]), 0).unwrap();
    assert!(labels.to_plain_text().contains("Ann"));
    w.finish(&mut v);
    assert_eq!(v.db.objects(Kind::Report), ["Orders"]);
    assert!(matches!(v.panes.last(), Some(Pane::Report(_))));
    let printed = v.print_doc();
    assert!(printed.to_plain_text().contains("Customer: 2"));
}

#[test]
fn import_and_export() {
    let dir = std::env::temp_dir().join(format!("apex_dbview_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("people.csv");
    std::fs::write(&csv, "Name,Age\nAnn,30\nBob,41\n").unwrap();
    let mut v = sample();
    transfer::import_path(&mut v, &csv);
    assert!(v.db.table_names().iter().any(|t| t == "people"), "{:?}", v.db.table_names());
    assert_eq!(count(&v, "people"), 2);
    // Again: a new name.
    transfer::import_path(&mut v, &csv);
    assert!(v.db.table_names().iter().any(|t| t == "people2"));
    v.undo();
    assert!(!v.db.table_names().iter().any(|t| t == "people2"));
    let out = dir.join("out.csv");
    transfer::write_csv(&v, &RowSource::Table("people".into()), &out).unwrap();
    assert_eq!(std::fs::read_to_string(&out).unwrap().replace("\r\n", "\n"), "Name,Age\nAnn,30\nBob,41\n");
    let wb = transfer::workbook(&v, "people", &RowSource::Table("people".into())).unwrap();
    assert_eq!(wb.sheets[0].name, "people");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn relations_diagram_proposes() {
    let mut v = sample();
    let mut d = relations::Diagram::load(&v.db);
    assert_eq!(d.rels.len(), 1);
    let c = d.tables.iter().position(|t| t.0 == "Customers").unwrap();
    let o = d.tables.iter().position(|t| t.0 == "Orders").unwrap();
    // Dragging Customers.Id onto Orders.Amount proposes Orders → Customers.
    let amount = d.tables[o].2.iter().position(|f| f == "Amount").unwrap();
    d.propose((c, 0), (o, amount));
    let (r, new) = d.pending.clone().unwrap();
    assert!(new);
    assert_eq!((r.table.as_str(), r.ref_table.as_str()), ("Orders", "Customers"));
    // Removing the relation, then undoing.
    let r = d.rels[0].clone();
    v.confirmed(ConfirmAction::Relation(r, false));
    assert!(v.db.relations().is_empty());
    v.undo();
    assert_eq!(v.db.relations().len(), 1);
}

/// Keys the tab builds from names (types, totals, controls...) exist.
#[test]
fn built_keys_exist() {
    let base = &crate::i18n::langs()[0];
    let missing: Vec<String> = built_keys().into_iter().filter(|k| base.get(k).is_none()).collect();
    assert!(missing.is_empty(), "{missing:?}");
}

// "Add index" twice gives two names; the same name twice is told before
// saving, and a design that fails leaves the database as it was.
#[test]
fn indexes_named_once_and_failures_change_nothing() {
    let mut v = view();
    let names = |v: &DbView| -> Vec<String> {
        let mut st = v.db.conn().prepare("SELECT name FROM sqlite_master ORDER BY name").unwrap();
        st.query_map([], |r| r.get(0)).unwrap().flatten().collect()
    };
    let before = names(&v);
    let ix = |n: &str, c: &str| apex_db::sqlite::IndexDef { name: n.into(), columns: vec![c.into()], unique: false };
    let with = |v: &DbView, indexes| design::Designer::new(None, apex_db::sqlite::TableDesign { indexes, ..design::Designer::blank(&v.db) });
    assert!(with(&v, vec![ix("T_Id", "Id"), ix("t_id", "Id")]).problem().is_some());
    // A change that fails halfway (here after making the table).
    let d = with(&v, Vec::new()).design;
    assert!(!v.with_snapshot("design change", |db| {
        db.apply_design(None, &d)?;
        Err(apex_db::Error::Db("later".into()))
    }));
    assert_eq!(names(&v), before);
    assert!(v.undo.is_empty());
}
