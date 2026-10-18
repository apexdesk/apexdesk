//! Tests of the data model: relationships, DAX measures and pivot tables
//! over related tables.

use super::eval::{Context, Engine, check_measure, compile_all};
use super::*;
use crate::analysis::pivot::{Field, FieldFilter, ModelSource, PivotSpec, PivotTable, ValueField};
use crate::value::{ErrorKind, Value};
use crate::workbook::Workbook;
use crate::{CellRef, Range};

/// Sales (A:D) by product (F:H), products by category (J:K).
fn book() -> Workbook {
    let mut wb = Workbook::new();
    let mut inputs = Vec::new();
    let mut put = |c: u32, rows: &[&[&str]]| {
        for (r, row) in rows.iter().enumerate() {
            for (k, v) in row.iter().enumerate() {
                inputs.push((CellRef::new(r as u32, c + k as u32), v.to_string()));
            }
        }
    };
    put(0, &[&["OrderID", "ProductID", "Qty", "Amount"], &["1", "P1", "2", "10"], &["2", "P2", "1", "20"], &["3", "P1", "5", "50"], &["4", "P3", "3", "9"], &["5", "P3", "1", "3"], &["6", "P2", "2", "40"], &["7", "P1", "1", "10"]]);
    put(5, &[&["ProductID", "Name", "Category"], &["P1", "Pen", "Office"], &["P2", "Ink", "Office"], &["P3", "Apple", "Food"], &["P4", "Bread", "Food"]]);
    put(9, &[&["Category", "Dept"], &["Office", "Stationery"], &["Food", "Grocery"]]);
    wb.set_inputs(0, inputs);
    for (r, name) in [("A1:D8", "Sales"), ("F1:H5", "Products"), ("J1:K3", "Categories")] {
        let id = wb.create_table(0, Range::parse(r).unwrap(), true, "TableStyleMedium2").unwrap();
        wb.rename_table(id, name).unwrap();
    }
    wb
}

fn measures() -> Vec<Measure> {
    let m = |name: &str, formula: &str| Measure { name: name.into(), table: "Sales".into(), formula: formula.into(), format: String::new() };
    vec![
        m("Total", "SUM(Sales[Amount])"),
        m("Orders", "COUNTROWS(Sales)"),
        m("Avg Order", "DIVIDE([Total], [Orders])"),
        m("Office", "CALCULATE([Total], Products[Category] = \"Office\")"),
        m("All Total", "CALCULATE([Total], ALL(Products))"),
        m("Share", "DIVIDE([Total], [All Total])"),
        m("Big", "CALCULATE([Total], FILTER(Sales, Sales[Amount] >= 20))"),
        m("Sold", "DISTINCTCOUNT(Sales[ProductID])"),
        m("Double Qty", "SUMX(Sales, Sales[Qty] * 2)"),
        m("Grocery Orders", "COUNTROWS(FILTER(Sales, RELATED(Categories[Dept]) = \"Grocery\"))"),
        m("Spread", "MAX(Sales[Amount]) - MIN(Sales[Amount])"),
        m("Mean Qty", "AVERAGE(Sales[Qty])"),
        m("Count", "COUNT(Sales[Amount]) + 0.5 * 2 ^ 2"),
        m("Busy Products", "COUNTROWS(FILTER(Products, [Orders] >= 3))"),
        m("Label", "IF([Total] > 100, \"big\" & \"!\", BLANK())"),
    ]
}

fn model_book() -> Workbook {
    let mut wb = book();
    let data = wb.model_data();
    let rels = detect_relationships(&data, &[]);
    wb.set_data_model(DataModel { relationships: rels, measures: measures() });
    wb
}

#[test]
fn detects_relationships() {
    let wb = book();
    let data = wb.model_data();
    let rels = detect_relationships(&data, &[]);
    let got: Vec<String> = rels.iter().map(|r| format!("{}[{}]->{}[{}]", r.from_table, r.from_column, r.to_table, r.to_column)).collect();
    assert_eq!(got, ["Sales[ProductID]->Products[ProductID]", "Products[Category]->Categories[Category]"]);
    assert!(detect_relationships(&data, &rels).is_empty());
    // The "one" side must have unique keys.
    let bad = Relationship::new("Products", "ProductID", "Sales", "ProductID");
    assert_eq!(check_relationship(&data, &rels, &bad, None), Err(RelError::NotUnique));
    assert_eq!(check_relationship(&data, &rels, &rels[0], None), Err(RelError::Duplicate));
    assert_eq!(check_relationship(&data, &rels, &rels[0], Some(0)), Ok(()));
    assert_eq!(check_relationship(&data, &rels, &Relationship::new("Sales", "Nope", "Products", "ProductID"), None), Err(RelError::NoColumn));
}

#[test]
fn measures_without_filters() {
    let wb = model_book();
    let data = wb.model_data();
    let ms = measures();
    let compiled = compile_all(&data, &ms);
    for (n, e) in &compiled {
        assert!(e.is_ok(), "{n}: {e:?}");
    }
    let eng = Engine::new(&data, &compiled);
    let v = |n: &str| eng.measure(n, &Context::default());
    assert_eq!(v("Total"), Value::Number(142.0));
    assert_eq!(v("Orders"), Value::Number(7.0));
    assert_eq!(v("Avg Order"), Value::Number(142.0 / 7.0));
    assert_eq!(v("Office"), Value::Number(130.0));
    assert_eq!(v("Share"), Value::Number(1.0));
    assert_eq!(v("Big"), Value::Number(110.0));
    assert_eq!(v("Sold"), Value::Number(3.0));
    assert_eq!(v("Double Qty"), Value::Number(30.0));
    assert_eq!(v("Grocery Orders"), Value::Number(2.0));
    assert_eq!(v("Spread"), Value::Number(47.0));
    assert_eq!(v("Mean Qty"), Value::Number(15.0 / 7.0));
    assert_eq!(v("Count"), Value::Number(9.0));
    // Context transition: [Orders] counts each product's sales.
    assert_eq!(v("Busy Products"), Value::Number(1.0));
    assert_eq!(v("Label"), Value::text("big!"));
    assert_eq!(wb.measure_total("Total", None), Ok(Value::Number(142.0)));
    assert_eq!(wb.measure_total("Probe", Some("=[Total] / 2")), Ok(Value::Number(71.0)));
    assert_eq!(wb.measure_total("Probe", Some("DIVIDE(1, 0, -1)")), Ok(Value::Number(-1.0)));
    assert_eq!(wb.measure_total("Probe", Some("1 / 0")), Ok(Value::Error(ErrorKind::Div0)));
}

#[test]
fn clear_errors() {
    let wb = model_book();
    let data = wb.model_data();
    let ms = measures();
    let err = |f: &str| check_measure("New", f, &data, &ms).unwrap_err();
    let e = err("SUM(Sales[Nope])");
    assert_eq!((e.kind, e.arg.as_str()), (DaxErrorKind::UnknownColumn, "Sales[Nope]"));
    let e = err("FOO(1)");
    assert_eq!((e.kind, e.arg.as_str(), e.pos), (DaxErrorKind::UnknownFunction, "FOO", 1));
    let e = err("= SUM(Sales[Amount]");
    assert_eq!((e.kind, e.arg.as_str()), (DaxErrorKind::Expected, ")"));
    assert_eq!(err("[Nope] + 1").kind, DaxErrorKind::UnknownMeasure);
    assert_eq!(err("SUM(Sales)").kind, DaxErrorKind::NeedColumn);
    assert_eq!(err("COUNTROWS(1)").kind, DaxErrorKind::NeedTable);
    assert_eq!(err("DIVIDE(1)").kind, DaxErrorKind::Arity);
    assert_eq!(err("Nope[x]").kind, DaxErrorKind::UnknownTable);
    assert_eq!(err("[Amount]").kind, DaxErrorKind::NeedRowContext);
    assert_eq!(err("CALCULATE([Total], Sales[Qty] > Sales[Amount])").kind, DaxErrorKind::FilterColumn);
    let e = err("1 + * 2");
    assert_eq!((e.kind, e.arg.as_str(), e.pos), (DaxErrorKind::Syntax, "*", 5));
    assert_eq!(err("  ").kind, DaxErrorKind::Empty);
    // A measure that leads back to itself.
    let mut ms2 = ms.clone();
    ms2.push(Measure { name: "A".into(), formula: "[New] + 1".into(), ..Default::default() });
    let e = check_measure("New", "[A] * 2", &data, &ms2).unwrap_err();
    assert_eq!(e.kind, DaxErrorKind::Circular);
    // Bare columns inside FILTER take its table; strings, comments.
    assert!(check_measure("New", "COUNTROWS(FILTER(Sales, [Qty] > 1 && [ProductID] <> \"P\"\"1\")) // note", &data, &ms).is_ok());
    assert!(check_measure("New", "CALCULATE(SUM('Sales'[Amount]); ALL(Sales[Qty]))", &data, &ms).is_ok());
}

fn text(t: &PivotTable) -> Vec<Vec<String>> {
    t.cells
        .iter()
        .map(|r| {
            r.iter()
                .map(|c| match &c.value {
                    Value::Number(n) => format!("{}", (n * 1000.0).round() / 1000.0),
                    Value::Text(s) => s.to_string(),
                    Value::Error(e) => e.as_str().to_string(),
                    _ => String::new(),
                })
                .collect()
        })
        .collect()
}

#[test]
fn pivot_over_related_tables() {
    let mut wb = model_book();
    let mut ms = ModelSource::default();
    let cat = ms.col_of("Products", "Category");
    let amount = ms.col_of("Sales", "Amount");
    let total = ms.measure_of("Total");
    let share = ms.measure_of("Share");
    let spec = PivotSpec { rows: vec![Field::new(cat)], values: vec![ValueField { col: amount, ..Default::default() }, ValueField { col: total, ..Default::default() }, ValueField { col: share, ..Default::default() }], model: Some(ms.clone()), ..Default::default() };
    let g = text(&wb.pivot_compute(&spec).unwrap());
    assert_eq!(g[0], ["Row Labels", "Sum of Amount", "Total", "Share"]);
    assert_eq!(g[1], ["Food", "12", "12", "0.085"]);
    assert_eq!(g[2], ["Office", "130", "130", "0.915"]);
    assert_eq!(g[3], ["Grand Total", "142", "142", "1"]);

    // Two hops (sales -> products -> categories), and a column field.
    let dept = ms.col_of("Categories", "Dept");
    let name = ms.col_of("Products", "Name");
    let spec2 = PivotSpec { rows: vec![Field::new(dept)], cols: vec![Field::new(name)], values: vec![ValueField { col: total, ..Default::default() }], model: Some(ms.clone()), ..Default::default() };
    let g = text(&wb.pivot_compute(&spec2).unwrap());
    assert_eq!(g[1], ["Row Labels", "Apple", "Bread", "Ink", "Pen", "Grand Total"]);
    assert_eq!(g[2], ["Grocery", "12", "", "", "", "12"]);
    assert_eq!(g[3], ["Stationery", "", "", "60", "70", "130"]);
    // ALL(Products) clears the filters on the tables Products relates to
    // (its expanded table) as well.
    let spec2b = PivotSpec { rows: vec![Field::new(dept)], values: vec![ValueField { col: share, ..Default::default() }], model: Some(ms.clone()), ..Default::default() };
    let g = text(&wb.pivot_compute(&spec2b).unwrap());
    assert_eq!(g[1..3], [["Grocery", "0.085"], ["Stationery", "0.915"]]);

    // Only product fields and a measure: every product is listed, unsold
    // ones blank.
    let spec3 = PivotSpec { rows: vec![Field::new(name)], values: vec![ValueField { col: total, ..Default::default() }], model: Some(ms.clone()), ..Default::default() };
    let g = text(&wb.pivot_compute(&spec3).unwrap());
    assert_eq!(g[1..], [["Apple", "12"], ["Bread", ""], ["Ink", "60"], ["Pen", "70"], ["Grand Total", "142"]]);

    // A report filter on a related table filters measures too.
    let mut spec4 = spec3.clone();
    spec4.filters.push(FieldFilter { field: Field::new(cat), items: vec!["Office".into()] });
    let g = text(&wb.pivot_compute(&spec4).unwrap());
    assert_eq!(g.last().unwrap(), &["Grand Total", "130"]);
    assert_eq!(g[3..5], [["Ink", "60"], ["Pen", "70"]]);

    // Created in the workbook, it refreshes when a measure changes and
    // follows a table rename.
    let i = wb.create_pivot(spec3, None).unwrap();
    let at = |wb: &Workbook| {
        let p = &wb.analysis.pivots[i];
        let si = wb.sheet_index(&p.sheet).unwrap();
        wb.sheets[si].value(CellRef::new(p.at.row + 5, p.at.col + 1)).clone()
    };
    assert_eq!(at(&wb), Value::Number(142.0));
    let mut m = wb.analysis.model.clone();
    m.measures[0].formula = "SUM(Sales[Amount]) * 10".into();
    wb.set_data_model(m);
    assert_eq!(at(&wb), Value::Number(1420.0));
    assert!(wb.undo());
    assert_eq!(at(&wb), Value::Number(142.0));
    assert_eq!(wb.analysis.model.measures[0].formula, "SUM(Sales[Amount])");
    let id = wb.find_table("Products").unwrap();
    wb.rename_table(id, "Items").unwrap();
    assert_eq!(wb.analysis.model.relationships[0].to_table, "Items");
    assert_eq!(wb.analysis.model.measures[3].formula, "CALCULATE([Total], Items[Category] = \"Office\")");
    assert_eq!(wb.analysis.pivots[i].spec.model.as_ref().unwrap().cols[0].0, "Items");
    wb.refresh_pivot(i).unwrap();
    assert_eq!(at(&wb), Value::Number(142.0));
    // Show Details of a cell lists its records.
    let p = wb.analysis.pivots[i].clone();
    let si = wb.sheet_index(&p.sheet).unwrap();
    let s = wb.pivot_details(si, CellRef::new(p.at.row + 4, p.at.col + 1)).unwrap();
    assert_eq!(wb.sheets[s].value(CellRef::new(0, 0)), &Value::text("Name"));
    assert_eq!(wb.sheets[s].value(CellRef::new(1, 0)), &Value::text("Pen"));
    assert!(wb.sheets[s].value(CellRef::new(2, 0)).is_empty());
}

#[test]
fn unrelated_tables_are_refused() {
    let mut wb = book();
    let mut ms = ModelSource::default();
    let dept = ms.col_of("Categories", "Dept");
    let qty = ms.col_of("Sales", "Qty");
    let spec = PivotSpec { rows: vec![Field::new(dept)], values: vec![ValueField { col: qty, ..Default::default() }], model: Some(ms), ..Default::default() };
    assert_eq!(wb.pivot_compute(&spec).unwrap_err(), crate::workbook::AnalysisError::NoRelationship);
    let data = wb.model_data();
    let rels = detect_relationships(&data, &[]);
    wb.set_data_model(DataModel { relationships: rels, measures: Vec::new() });
    let g = text(&wb.pivot_compute(&spec).unwrap());
    assert_eq!(g[1..], [["Grocery", "4"], ["Stationery", "11"], ["Grand Total", "15"]]);
}

#[test]
fn renames_in_formulas() {
    assert_eq!(dax::rename_table("SUM(Sales[x]) + COUNTROWS('Sales') + Sales2[y]", "sales", "Big Sales"), "SUM('Big Sales'[x]) + COUNTROWS('Big Sales') + Sales2[y]");
    assert_eq!(dax::rename_table("SUM(T[x])", "T", "U"), "SUM(U[x])");
}

#[test]
fn deep_dax_is_refused() {
    let no = |_: &str| false;
    assert!(super::dax::parse(&format!("{}1{}", "(".repeat(100_000), ")".repeat(100_000)), &no).is_err());
    assert!(super::dax::parse(&format!("{}1", "-".repeat(5000)), &no).is_err());
    assert!(super::dax::parse("-(1 + 2) ^ 2", &no).is_ok());
}
