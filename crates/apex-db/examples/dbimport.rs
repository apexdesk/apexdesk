//! Imports Access or dBase files into SQLite ones and prints the reports:
//! `dbimport <out dir> <file>...`

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((out, files)) = args.split_first() else { return };
    for f in files {
        let src = std::path::Path::new(f);
        let dst = std::path::Path::new(out).join(format!("{}.sqlite", src.file_stem().unwrap().to_string_lossy()));
        match apex_db::import::import_file(src, &dst) {
            Ok(r) => println!("{f}: {} tables, {} rows, {} relations, {} queries, untranslated {:?}, {} warnings", r.tables, r.rows, r.relations, r.queries, r.untranslated, r.warnings.len()),
            Err(e) => println!("{f}: ERROR {e}"),
        }
    }
}
