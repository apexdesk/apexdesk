# Changes to arboard 3.6.1

Upstream: `arboard` 3.6.1 from crates.io. `upstream.patch` in this folder
is the exact diff from it (`patch -p1 < upstream.patch` inside an unpacked
`arboard-3.6.1`). Upstream's `examples/`, `tools/`, `.github/`,
`.gitignore`, `rustfmt.toml`, `Cargo.lock` and `Cargo.toml.orig` are not
copied. The changes are marked "ApexDesk patch"; vendor/README.md explains
them.

| File | Lines added / removed | Why |
|---|---|---|
| `src/lib.rs` | +22 / -0 | `Get::format()`, `Get::formats()`, `Set::formats()`: one copy in several formats, named by MIME type |
| `src/platform/linux/x11.rs` | +118 / -2 | the same on X11, by atom; text also served as `text/plain`, `text/plain;charset=utf-8` and `TEXT` (what XWayland asks for) |
| `src/platform/windows.rs` | +107 / -0 | `text/html` as "HTML Format" with its header, `text/rtf` as "Rich Text Format", other names as registered formats |
| `src/platform/osx.rs` | +36 / -0 | text and HTML |
| `src/platform/linux/mod.rs` | +26 / -0 | the Wayland data-control backend says it can't (smithay-clipboard does it there) |
