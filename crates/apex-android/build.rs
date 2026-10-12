//! Marks the build as this crate's: crates/apex-app/src/main.rs is empty
//! for any system but Android here.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-cfg=apex_android");
}
