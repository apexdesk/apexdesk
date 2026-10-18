//! Marks the build as this crate's: crates/apex-app/src/main.rs is empty
//! for any target but the browser's here.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-cfg=apex_web");
}
