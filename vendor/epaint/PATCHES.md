# Changes to epaint 0.36.2

Upstream: `epaint` 0.36.2 from crates.io. `upstream.patch` in this folder
is the exact diff from it (`patch -p1 < upstream.patch` inside an unpacked
`epaint-0.36.2` gives this folder; the upstream `Cargo.lock` and
`Cargo.toml.orig` are left out). Every change is marked "ApexDesk patch" in
the code; vendor/README.md explains each one at length.

| File | Lines added / removed | Why |
|---|---|---|
| `Cargo.toml` | +3 / -0 | `unicode-bidi`, for right-to-left runs in `text_layout.rs` |
| `src/text/text_layout.rs` | +304 / -61 | shaping cache (`ShapeCache`, `shape_segment()`), unrounded rows, kinsoku line breaking, justification at spaces, hanging indents, right-to-left runs (bidi), rows kept together |
| `src/text/text_layout_types.rs` | +105 / -1 | the options those use: `LayoutJob::vertical` and `rtl`, `TextWrapping::kinsoku`, `hanging_punctuation`, `hanging_spaces`, `keep_together`, `indent_rest`, `indent_all`; `Glyph::underline_y()` |
| `src/text/kinsoku.rs` | new, 97 lines | East Asian line breaking rules and the characters kept upright in vertical text |
| `src/text/fonts.rs` | +24 / -2 | one shape cache per font family; hanging indents and kept-together ranges carried to each paragraph |
| `src/text/font.rs` | +10 / -2 | glyphs too big for the atlas are not drawn (a file's huge font size); each glyph drawn on this thread (`num_threads: 0`) |
| `src/text/mod.rs` | +2 / -0 | the `kinsoku` module |
| `src/tessellator.rs` | +5 / -1 | rows of unrounded galleys snapped to pixels when drawn |
| `src/texture_atlas.rs` | +3 / -0 | an overflowing atlas grows to fit what restarts below, instead of writing past its end |

To update epaint: unpack the new version, apply `upstream.patch` (fixing
what no longer applies), and write the new diff here.
