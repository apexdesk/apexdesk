# Changes to egui-winit 0.36.2

Upstream: `egui-winit` 0.36.2 from crates.io. `upstream.patch` in this
folder is the exact diff from it (`patch -p1 < upstream.patch` inside an
unpacked `egui-winit-0.36.2`; the upstream `Cargo.lock` and
`Cargo.toml.orig` are left out). The changes are marked "ApexDesk patch";
vendor/README.md explains them.

| File | Lines added / removed | Why |
|---|---|---|
| `src/lib.rs` | +12 / -0 | the first window takes the launcher's activation token (`XDG_ACTIVATION_TOKEN` / `DESKTOP_STARTUP_ID`), so the desktop stops waiting for the program to start |
| `src/clipboard.rs` | +19 / -0 | Android: the system clipboard's read and write, handed in by the program (`clipboard::ANDROID`) |
