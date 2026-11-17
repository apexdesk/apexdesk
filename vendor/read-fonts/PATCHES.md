# Changes to read-fonts 0.41.0

Upstream: `read-fonts` 0.41.0 from crates.io. `upstream.patch` in this
folder is the exact diff from it (`patch -p1 < upstream.patch` inside an
unpacked `read-fonts-0.41.0`; the upstream `Cargo.lock` is left out). The
change is marked "ApexDesk patch"; vendor/README.md explains it.

| File | Lines added / removed | Why |
|---|---|---|
| `src/ps/cs.rs` | +12 / -0 | the charstring evaluator counts the operators and operands it runs for a glyph (subroutines and seac accents included) and stops past 2^20 (`MAX_WORK`): a few bytes of a CFF font could otherwise run for billions of steps |
