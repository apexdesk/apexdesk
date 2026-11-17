# Changes to tiberius 0.13.0

Upstream: `tiberius` 0.13.0 from crates.io. `upstream.patch` in this
folder is the exact diff from it (`patch -p1 < upstream.patch` inside an
unpacked `tiberius-0.13.0`). Upstream's docker files, docs, examples and
tests are not copied. The changes are marked "ApexDesk patch";
vendor/README.md explains them.

| File | Lines added / removed | Why |
|---|---|---|
| `Cargo.toml` | +3 / -0 | `tokio-rustls` without default features, with `logging`, `tls12` and `ring`: no aws-lc (a C library) |
| `src/client/tls_stream/rustls_tls_stream.rs` | +3 / -3 | the fallback crypto provider is ring's |
