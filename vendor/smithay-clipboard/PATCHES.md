# Changes to smithay-clipboard 0.7.3

Upstream: `smithay-clipboard` 0.7.3 from crates.io. `upstream.patch` in
this folder is the exact diff from it (`patch -p1 < upstream.patch` inside
an unpacked `smithay-clipboard-0.7.3`). Upstream's `examples/`, `.github/`,
`CHANGELOG.md`, `README.md`, `doc_index.html`, `.gitignore`,
`rustfmt.toml`, `Cargo.lock` and `Cargo.toml.orig` are not copied. The
changes are marked "ApexDesk patch"; vendor/README.md explains them.

| File | Lines added / removed | Why |
|---|---|---|
| `src/lib.rs` | +121 / -1 | the live clipboard kept in a global; `active()`, `store_formats()`, `load_format()`, `formats()`; dropped files (`take_dropped()`, `hovering()`, `set_drop_waker()`) |
| `src/state.rs` | +216 / -10 | the stored formats offered with the selection and served on request; the offer's formats listed and read; `text/uri-list` drops read and kept |
| `src/worker.rs` | +30 / -3 | commands to store several formats, load one and list them, answered by channel |
