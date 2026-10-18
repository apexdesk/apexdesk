//! Embeds every language file of `apex-app/i18n/<code>.txt` (the
//! translations stay next to the program that uses most of them).

use std::fmt::Write;

fn main() {
    // Read when the script runs: a script built for another checkout
    // (sharing the target folder) must not embed that one's files.
    let dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir")).join("../apex-app/i18n");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("i18n directory")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "txt"))
        .collect();
    files.sort();
    let mut out = String::from("pub static LANG_FILES: &[(&str, &str)] = &[\n");
    for p in files {
        let code = p.file_stem().unwrap().to_str().unwrap();
        println!("cargo:rerun-if-changed={}", p.display());
        writeln!(out, "    ({:?}, include_str!({:?})),", code, p.display().to_string()).unwrap();
    }
    out.push_str("];\n");
    let dest = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("langs.rs");
    std::fs::write(dest, out).unwrap();
}
