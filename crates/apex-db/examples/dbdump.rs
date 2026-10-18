//! Prints what apex-db reads from an Access or dBase file: tables with
//! their columns and rows, indexes, relations and saved queries.
//!
//! cargo run -p apex-db --example dbdump -- file.mdb [table]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("usage: dbdump FILE [TABLE]");
        std::process::exit(2);
    };
    if args.len() == 3 && args[0] == "--tsv" {
        tsv(&args[1], &args[2]);
        return;
    }
    let db = match apex_db::open(std::path::Path::new(path)) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    for t in &db.tables {
        if args.get(1).is_some_and(|n| n != &t.name) {
            continue;
        }
        println!("== {} ({} rows)", t.name, t.rows.len());
        for c in &t.columns {
            println!("  {} {:?} size={} p={} s={}{}", c.name, c.ty, c.size, c.precision, c.scale, if c.auto { " auto" } else { "" });
        }
        for i in &t.indexes {
            println!("  index {} {:?}{}{}", i.name, i.columns, if i.primary { " primary" } else { "" }, if i.unique { " unique" } else { "" });
        }
        for r in &t.rows {
            println!("  | {}", r.iter().map(|v| v.to_text()).collect::<Vec<_>>().join(" | "));
        }
    }
    if args.len() < 2 {
        for r in &db.relations {
            println!("relation {}: {}{:?} -> {}{:?} enforce={} upd={} del={}", r.name, r.table, r.columns, r.ref_table, r.ref_columns, r.enforce, r.cascade_update, r.cascade_delete);
        }
        for q in &db.queries {
            println!("query {} {:?}:\n{}", q.name, q.kind, q.sql);
        }
        for w in &db.warnings {
            println!("warning: {w}");
        }
    }
}

/// One table as tab-separated lines (escaped like `mdb-export -e`), for
/// comparing with mdbtools.
fn tsv(path: &str, table: &str) {
    let t = match apex_db::read_table(std::path::Path::new(path), table) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    for r in &t.rows {
        let cells: Vec<String> = r
            .iter()
            .map(|v| match v {
                apex_db::Value::Blob(b) => b.iter().map(|x| format!("{x:02X}")).collect(),
                apex_db::Value::Date(d) => {
                    let s = apex_db::model::fmt_date(*d);
                    if s.len() == 10 { format!("{s} 00:00:00") } else { s }
                }
                v => v.to_text().replace('\\', "\\\\").replace('\r', "\\r").replace('\n', "\\n").replace('\t', "\\t"),
            })
            .collect();
        println!("{}", cells.join("\t"));
    }
}
