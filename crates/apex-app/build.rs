//! On Windows, the program icon (the language files are embedded by the
//! apex-i18n crate) and the main thread's stack.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    windows_icon();
    windows_stack();
}

/// 8 MiB of stack for the main thread, as on Linux and macOS (Windows
/// gives 1 MiB): formulas are evaluated on it.
fn windows_stack() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
        Ok("msvc") => println!("cargo:rustc-link-arg-bins=/STACK:8388608"),
        _ => println!("cargo:rustc-link-arg-bins=-Wl,--stack,8388608"),
    }
}

/// Compiles `packaging/windows/apexdesk.rc` (the .exe icon) with `rc` or
/// `llvm-rc` and links it in. Without a resource compiler the program just
/// has no icon in Explorer.
fn windows_icon() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") || std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/windows");
    let rc = dir.join("apexdesk.rc");
    println!("cargo:rerun-if-changed={}", rc.display());
    println!("cargo:rerun-if-changed={}", dir.join("apexdesk.ico").display());
    println!("cargo:rerun-if-env-changed=RC");
    let res = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("apexdesk.res");
    let mut tools: Vec<String> = std::env::var("RC").into_iter().collect();
    tools.extend(["rc", "llvm-rc"].map(String::from));
    tools.extend((15..=30).rev().map(|v| format!("llvm-rc-{v}")));
    for tool in tools {
        let fo = format!("/fo{}", res.display());
        let ok = std::process::Command::new(&tool).current_dir(&dir).args(["/nologo", &fo]).arg(&rc).status().is_ok_and(|s| s.success());
        if ok {
            println!("cargo:rustc-link-arg-bins={}", res.display());
            return;
        }
    }
    println!("cargo:warning=no resource compiler (rc / llvm-rc): the .exe has no icon");
}
