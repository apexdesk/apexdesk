//! Exports documents to HTML and EPUB and workbooks to HTML, to check the
//! exporters over a corpus: `webexport OUT_DIR FILES...`.

use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((out, files)) = args.split_first() else {
        eprintln!("usage: webexport OUT_DIR FILES...");
        return;
    };
    let out = Path::new(out);
    let _ = std::fs::create_dir_all(out);
    for f in files {
        let p = Path::new(f);
        let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        match apex_io::open(p) {
            Ok((apex_io::Loaded::Doc(d), _)) => {
                let _ = std::fs::write(out.join(format!("{stem}.html")), apex_io::html::doc_html(&d));
                let _ = std::fs::write(out.join(format!("{stem}.epub")), apex_io::html::doc_epub(&d));
            }
            Ok((apex_io::Loaded::Sheet(wb), _)) => {
                let _ = std::fs::write(out.join(format!("{stem}.html")), apex_io::html::sheet_html(&wb, &stem));
            }
            Ok((apex_io::Loaded::Slides(_), _)) => eprintln!("{f}: presentations are exported by the app"),
            Err(e) => eprintln!("{f}: {e}"),
        }
    }
}
