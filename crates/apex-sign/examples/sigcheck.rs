//! Prints the signatures of files (or of the files in folders):
//! `cargo run -p apex-sign --example sigcheck -- file.pdf folder…`

fn main() {
    let mut files = Vec::new();
    for arg in std::env::args().skip(1) {
        match std::fs::read_dir(&arg) {
            Ok(dir) => {
                let mut v: Vec<String> = dir.filter_map(|e| e.ok()).map(|e| e.path().to_string_lossy().into_owned()).collect();
                v.sort();
                files.extend(v);
            }
            Err(_) => files.push(arg),
        }
    }
    for path in files {
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(e) => {
                println!("{path}: {e}");
                continue;
            }
        };
        let sigs = apex_sign::verify(&data);
        println!("{path}: {} signature(s)", sigs.len());
        for s in sigs {
            println!(
                "  {:?} {:?} [{}] signer={:?} time={:?}{} intact={:?} valid={:?} trust={:?} changed_after={} note={:?} changed={:?} unsigned={:?}",
                s.status(),
                s.kind,
                s.format,
                s.signer(),
                s.time.map(apex_sign::time::format_utc),
                if s.timestamped { " (TSA)" } else { "" },
                s.intact,
                s.valid,
                s.trust,
                s.changed_after,
                s.note,
                s.changed_parts,
                s.unsigned_parts,
            );
        }
    }
}
