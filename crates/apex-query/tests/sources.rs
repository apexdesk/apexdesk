//! Local sources end to end: files written here, queries run over them.

use std::path::PathBuf;

use apex_query::{Context, Error, Val, run};
use apex_sheet::query::{AggFn, Aggregate, FilterOp, JoinKind, Query, Source, Step};

fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("apex-query").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn text(location: &str) -> Source {
    Source::Text { location: location.into(), delimiter: None, encoding: String::new(), header: true, skip: 0 }
}

#[test]
fn csv_with_steps_and_relative_paths() {
    let d = dir("csv");
    // Windows-1252 bytes: "Múnich".
    let mut data = b"City;Sales;Day\r\nM".to_vec();
    data.push(0xFA);
    data.extend_from_slice(b"nich;1.200,50;2024-02-01\r\nOslo;300;2024-02-02\r\nOslo;100;2024-02-03\r\n");
    std::fs::write(d.join("sales.csv"), &data).unwrap();
    let mut q = Query::new("Sales", text("sales.csv"));
    q.steps = vec![
        Step::Group { by: vec!["City".into()], aggs: vec![Aggregate { column: "Sales".into(), func: AggFn::Sum, name: "Total".into() }, Aggregate { column: "Day".into(), func: AggFn::Max, name: "Last".into() }] },
        Step::Sort(vec![("Total".into(), false)]),
    ];
    let ctx = Context::new(vec![q.clone()], Some(d.clone()));
    let t = run(&q, &ctx).unwrap();
    assert_eq!(t.columns, ["City", "Total", "Last"]);
    assert_eq!(t.rows[0], vec![Val::text("Múnich"), Val::Num(1200.5), Val::Date(45323.0)]);
    assert_eq!(t.rows[1], vec![Val::text("Oslo"), Val::Num(400.0), Val::Date(45325.0)]);
    let r = t.to_result();
    assert_eq!(r.formats[2].as_deref(), Some("yyyy-mm-dd"));
    // An explicit encoding.
    std::fs::write(d.join("utf16.csv"), [0xFF, 0xFE, b'a', 0, b',', 0, b'b', 0, b'\n', 0, b'1', 0, b',', 0, b'2', 0]).unwrap();
    let q = Query::new("U", text("utf16.csv"));
    let t = run(&q, &Context::new(vec![], Some(d.clone()))).unwrap();
    assert_eq!(t.rows, vec![vec![Val::Num(1.0), Val::Num(2.0)]]);
    let q = Query::new("L", Source::Text { location: "sales.csv".into(), delimiter: Some(';'), encoding: "latin1".into(), header: false, skip: 1 });
    let t = run(&q, &Context::new(vec![], Some(d.clone()))).unwrap();
    assert_eq!(t.rows[0][0], Val::text("Múnich"));
    assert!(matches!(run(&Query::new("M", text("missing.csv")), &Context::new(vec![], Some(d))), Err(Error::Io(_))));
}

#[test]
fn json_xml_and_merge() {
    let d = dir("json");
    std::fs::write(d.join("orders.json"), r#"{"orders": [{"id": 1, "customer": {"code": "C1"}, "total": 10.5}, {"id": 2, "customer": {"code": "C2"}, "total": 4}, {"id": 3, "customer": {"code": "C9"}, "total": 1}]}"#).unwrap();
    std::fs::write(d.join("customers.xml"), r#"<customers><customer code="C1"><name>Ann</name></customer><customer code="C2"><name>Bo</name></customer></customers>"#).unwrap();
    let customers = Query::new("Customers", Source::Xml { location: "customers.xml".into(), element: String::new() });
    let mut orders = Query::new("Orders", Source::Json { location: d.join("orders.json").to_string_lossy().into_owned(), path: "orders".into() });
    orders.steps = vec![
        Step::Merge { query: "Customers".into(), keys: vec![("customer.code".into(), "code".into())], join: JoinKind::Left, columns: vec!["name".into()], names: vec![] },
        Step::Filter { column: "total".into(), op: FilterOp::Gt, value: "2".into(), case: false },
        Step::AddColumn { name: "Label".into(), formula: "[name] & \": \" & [total]".into() },
        Step::ChooseColumns(vec!["id".into(), "Label".into()]),
    ];
    let ctx = Context::new(vec![customers, orders.clone()], Some(d));
    let t = run(&orders, &ctx).unwrap();
    assert_eq!(t.columns, ["id", "Label"]);
    assert_eq!(t.rows, vec![vec![Val::Num(1.0), Val::text("Ann: 10.5")], vec![Val::Num(2.0), Val::text("Bo: 4")]]);
}

#[test]
fn folder_combines_files() {
    let d = dir("folder");
    std::fs::write(d.join("jan.csv"), "Item,Qty\nPens,3\n").unwrap();
    std::fs::write(d.join("feb.csv"), "Item,Qty,Note\nInk,2,rush\n").unwrap();
    std::fs::write(d.join("notes.md"), "not data").unwrap();
    std::fs::write(d.join(".~lock.jan.csv#"), "x").unwrap();
    let q = Query::new("All", Source::Folder { path: d.to_string_lossy().into_owned(), extension: String::new(), header: true, file_column: true });
    let t = run(&q, &Context::default()).unwrap();
    assert_eq!(t.columns, ["Source.Name", "Item", "Qty", "Note"]);
    assert_eq!(t.rows, vec![vec![Val::text("feb.csv"), Val::text("Ink"), Val::Num(2.0), Val::text("rush")], vec![Val::text("jan.csv"), Val::text("Pens"), Val::Num(3.0), Val::Null]]);
    let q = Query::new("Json", Source::Folder { path: d.to_string_lossy().into_owned(), extension: "json".into(), header: true, file_column: false });
    assert!(run(&q, &Context::default()).is_err());
}

#[test]
fn workbook_file_and_own_tables() {
    let d = dir("book");
    let mut wb = apex_sheet::Workbook::new();
    wb.set_input(0, apex_sheet::CellRef::new(0, 0), "Name");
    wb.set_input(0, apex_sheet::CellRef::new(0, 1), "Score");
    wb.set_input(0, apex_sheet::CellRef::new(1, 0), "Ann");
    wb.set_input(0, apex_sheet::CellRef::new(1, 1), "7");
    let path = d.join("scores.xlsx");
    apex_io::save_sheet(&wb, &path, apex_io::Format::Xlsx).unwrap();
    let q = Query::new("S", Source::Workbook { path: "scores.xlsx".into(), item: "Sheet1".into(), header: true });
    let t = run(&q, &Context::new(vec![], Some(d))).unwrap();
    assert_eq!(t.rows, vec![vec![Val::text("Ann"), Val::Num(7.0)]]);
    // A table of the workbook itself, and a query reading another one.
    wb.create_table(0, apex_sheet::Range::parse("A1:B2").unwrap(), true, "TableStyleLight1").unwrap();
    let a = Query::new("A", Source::Table { name: "table1".into() });
    let mut b = Query::new("B", Source::Query { name: "A".into() });
    b.steps.push(Step::Append { query: "A".into() });
    let ctx = Context::for_workbook(&wb, vec![a, b.clone()], None);
    assert_eq!(run(&b, &ctx).unwrap().rows.len(), 2);
}

#[cfg(feature = "sqlite")]
#[test]
fn sqlite_file() {
    let d = dir("sqlite");
    let path = d.join("shop.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE sales (id INTEGER, product TEXT, price REAL, sold DATE, note BLOB); INSERT INTO sales VALUES (1, 'Pen', 1.5, '2024-03-01', x'0102'), (2, 'Ink', NULL, '2024-03-02', NULL);").unwrap();
    drop(conn);
    let q = Query::new("S", Source::Sqlite { path: path.to_string_lossy().into_owned(), sql: "sales".into() });
    let t = run(&q, &Context::default()).unwrap();
    assert_eq!(t.columns, ["id", "product", "price", "sold", "note"]);
    assert_eq!(t.rows[0], vec![Val::Num(1.0), Val::text("Pen"), Val::Num(1.5), Val::Date(45352.0), Val::text("(2 bytes)")]);
    assert_eq!(t.rows[1][2], Val::Null);
    let q = Query::new("S", Source::Sqlite { path: path.to_string_lossy().into_owned(), sql: "SELECT product FROM sales WHERE id = 2".into() });
    assert_eq!(run(&q, &Context::default()).unwrap().rows, vec![vec![Val::text("Ink")]]);
    assert_eq!(apex_query::sources::db::list_tables(&q.source, "").unwrap(), ["sales"]);
    let q = Query::new("S", Source::Sqlite { path: path.to_string_lossy().into_owned(), sql: "DELETE FROM sales".into() });
    assert!(matches!(run(&q, &Context::default()), Err(Error::Database(_))));
}

#[test]
fn database_asks_for_a_password() {
    let q = Query::new("P", Source::Database { kind: apex_sheet::query::DbKind::Postgres, host: "127.0.0.1".into(), port: 1, database: "x".into(), user: "u".into(), sql: "t".into(), tls: false });
    assert_eq!(run(&q, &Context::default()), Err(Error::NeedsPassword("postgres://u@127.0.0.1:1/x".into())));
}

/// Serves `body` to each request on a local port, in the background.
#[cfg(feature = "web")]
fn serve(bodies: Vec<(&'static str, &'static str)>) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
            let reply = match bodies.iter().find(|(p, _)| *p == path) {
                Some((_, b)) => format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}", b.len()),
                None => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
            };
            let _ = s.write_all(reply.as_bytes());
        }
    });
    format!("http://{addr}")
}

#[cfg(feature = "web")]
#[test]
fn web_page_and_api() {
    let base = serve(vec![
        ("/page", "<html><body><table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>x</td></tr></table></body></html>"),
        ("/api", r#"{"data": [{"a": 1}, {"a": 2}]}"#),
        ("/file.csv", "k,v\nx,1\n"),
    ]);
    let t = run(&Query::new("H", Source::Html { location: format!("{base}/page"), index: 0 }), &Context::default()).unwrap();
    assert_eq!(t.rows, vec![vec![Val::Num(1.0), Val::text("x")]]);
    let t = run(&Query::new("J", Source::Json { location: format!("{base}/api"), path: String::new() }), &Context::default()).unwrap();
    assert_eq!(t.rows, vec![vec![Val::Num(1.0)], vec![Val::Num(2.0)]]);
    let t = run(&Query::new("C", text(&format!("{base}/file.csv"))), &Context::default()).unwrap();
    assert_eq!(t.rows, vec![vec![Val::text("x"), Val::Num(1.0)]]);
    let e = run(&Query::new("N", Source::Html { location: format!("{base}/missing"), index: 0 }), &Context::default());
    assert!(matches!(e, Err(Error::Network(_))));
    let e = run(&Query::new("I", Source::Html { location: format!("{base}/page"), index: 3 }), &Context::default());
    assert!(matches!(e, Err(Error::Parse(_))));
}

/// A one-page PDF drawing `content` with Helvetica as /F1.
fn pdf(content: &str) -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

#[test]
fn pdf_tables() {
    let d = dir("pdf");
    let mut c = String::from("BT /F1 14 Tf 72 740 Td (Statement) Tj ET\n");
    for (r, row) in [["2024-01-02", "Rent", "-700.5"], ["2024-01-05", "Salary", "2100"], ["2024-01-09", "Groceries", "-86.4"]].iter().enumerate() {
        let y = 684 - r * 16;
        for (x, v) in [72, 170, 400].iter().zip(row) {
            c += &format!("BT /F1 10 Tf {x} {y} Td ({v}) Tj ET\n");
        }
    }
    for (x, v) in [72, 170, 400].iter().zip(["Date", "Concept", "Amount"]) {
        c += &format!("BT /F1 10 Tf {x} 700 Td ({v}) Tj ET\n");
    }
    std::fs::write(d.join("statement.pdf"), pdf(&c)).unwrap();
    let q = Query::new("S", Source::Pdf { location: "statement.pdf".into(), index: 0, header: true });
    let ctx = Context::new(vec![q.clone()], Some(d.clone()));
    let t = run(&q, &ctx).unwrap();
    assert_eq!(t.columns, ["Date", "Concept", "Amount"]);
    assert_eq!(t.rows.len(), 3);
    assert_eq!(t.rows[1][1], Val::text("Salary"));
    assert_eq!(t.rows[1][2], Val::Num(2100.0));
    // No such table.
    let q = Query::new("S", Source::Pdf { location: "statement.pdf".into(), index: 1, header: true });
    assert!(matches!(run(&q, &ctx), Err(Error::Parse(_))));
}

#[test]
fn a_converted_power_query_query_runs() {
    let d = dir("power-query");
    std::fs::write(d.join("ventas.csv"), "Fecha;Región;Importe\n2024-01-02;Norte;1500\n2024-01-03;Sur;900\n2024-01-04;Norte;80\n2024-01-05;Norte;2000\n").unwrap();
    let m = r#"let
    Origen = Csv.Document(File.Contents("ventas.csv"),[Delimiter=";", Columns=3, Encoding=65001, QuoteStyle=QuoteStyle.None]),
    #"Encabezados promovidos" = Table.PromoteHeaders(Origen, [PromoteAllScalars=true]),
    #"Tipo cambiado" = Table.TransformColumnTypes(#"Encabezados promovidos",{{"Fecha", type date}, {"Región", type text}, {"Importe", Int64.Type}}),
    #"Filas filtradas" = Table.SelectRows(#"Tipo cambiado", each [Región] = "Norte" and [Importe] > 100),
    #"Personalizada agregada" = Table.AddColumn(#"Filas filtradas", "IVA", each Number.Round([Importe] * 0.21, 0), type number),
    #"Agrupado" = Table.Group(#"Personalizada agregada", {"Región"}, {{"Total", each List.Sum([Importe]), type number}, {"IVA", each List.Sum([IVA]), type number}, {"Filas", each Table.RowCount(_), Int64.Type}})
in
    #"Agrupado""#;
    let tr = apex_query::m::translate("Ventas", m, &[]);
    assert!(tr.untranslated.is_empty(), "{:?}", tr.untranslated);
    let q = tr.query.unwrap();
    let t = run(&q, &Context::new(vec![q.clone()], Some(d))).unwrap();
    assert_eq!(t.columns, ["Región", "Total", "IVA", "Filas"]);
    assert_eq!(t.rows, vec![vec![Val::text("Norte"), Val::Num(3500.0), Val::Num(735.0), Val::Num(2.0)]]);
}
