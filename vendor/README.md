# Vendored crates

## epaint 0.36.2 (shaping cache, unrounded rows)

A copy of `epaint` 0.36.2 from crates.io with two changes, used through
`[patch.crates-io]` in the workspace `Cargo.toml`.

**Why.** harfrust shapes text in font units, without a pixel size, but
epaint split every paragraph into font runs and shaped it again for each new
galley. A zoom lays out all visible text at a new size, so every step paid
for the shaping again. With the cache, laying out 1500 cell texts at a new
size goes from about 8.8 ms to 4 ms, and a zoom step in a large sheet runs
about 25 % fewer instructions. The rendering is pixel-identical.

**What changed** (all in `src/text/`, marked "ApexDesk patch"):

- `text_layout.rs`
  - `ShapeCache`, `ShapedSegment`, `ShapedRun` and `shape_segment()`.
  - `layout_section()` gets the shaped runs of each segment from the cache
    and only positions them.
  - `layout_shaped_run()` takes the glyph slices instead of a `GlyphBuffer`.
  - `TextRun` is `Clone` and `pub(crate)`.
- `fonts.rs`: `CachedFamily::shape_cache` (one cache per font family).

The cache is keyed by the segment text and its variation coordinates. It
keeps two generations of up to 16 384 segments and about 4 MB each, so a
font family holds at most about 8 MB of shaped glyphs.

**Unrounded rows.** `galley_from_rows()` rounded every row height and row
position to pixels, so a document's line pitch, and with it its page count,
changed with the zoom (a 12 pt line of Liberation Serif is 18.4 px at 100 %, drawn
as 18). A `LayoutJob` with `round_output_to_gui == false` now keeps the exact
heights, and the tessellator (`src/tessellator.rs`, `tessellate_text`)
snaps each row to a pixel instead, so text stays sharp. Rounded jobs are
unchanged: their rows are already on pixels.

**East Asian line breaking.** Stock epaint breaks lines only at spaces
(or anywhere, as a last resort), so Japanese and Chinese text, which has no
spaces, broke at the edge with no rules. `src/text/kinsoku.rs` adds the
kinsoku shori rules: a row may break between any two CJK characters, but no
row starts with closing punctuation or small kana and no row ends with an
opening bracket. `TextWrapping::kinsoku` (on by default) turns the rules on,
and `TextWrapping::hanging_punctuation` lets `、` and `。` hang past the right
edge (Word's `w:overflowPunct`). The row breaker in `text_layout.rs`
(`RowBreakCandidates`) uses them. `TextWrapping::hanging_spaces` lets
spaces hang past the edge too, as in word processors, where spaces at the
end of a row never wrap it.

**Vertical text and text kept together.** `LayoutJob::vertical` marks a
galley that is drawn turned a quarter clockwise as vertical East Asian
text: `tessellate_glyphs()` turns the quads of Chinese, Japanese and Korean
characters (`kinsoku::upright_in_vertical`) back about the middle of their
em box, so they stand upright while Latin text lies on its side.
`TextWrapping::keep_together` lists ranges of section indices a row never
breaks inside (a phonetic guide's base text); `fonts.rs` moves the ranges
to each paragraph's own sections. The vertical forms of punctuation join
the kinsoku lists.

**Glyphs drawn in one thread.** The PDF renderer turns on vello_cpu's
`multithreading` feature, and with it `RenderContext::new()` would hand
every glyph `font.rs` draws to a pool of threads (which also needs a
flush). `allocate_glyph_uncached()` asks for none (`num_threads: 0`), so
glyphs are drawn as before.

**Updating egui.** Copy the new `epaint` from `~/.cargo/registry/src/*/`
over this folder, then apply the change again: the diff against the
original is small. If a new epaint already caches shaping, remove this folder
and the `[patch.crates-io]` entry instead.

## egui-winit 0.36.2 (launch activation token, Android's clipboard)

A copy of `egui-winit` 0.36.2 from crates.io with two changes, used through
`[patch.crates-io]` in the workspace `Cargo.toml`.

**Why.** A dock or launcher (GNOME Shell, a file manager) starts a program
with an activation token in `XDG_ACTIVATION_TOKEN` (Wayland) or
`DESKTOP_STARTUP_ID` (X11), and waits for the program's first window to show
with it. egui-winit never handed the token to winit, so GNOME kept ApexDesk
"starting" until its timeout and ignored clicks on the dock icon meanwhile:
reopened soon after closing, ApexDesk didn't open.

**What changed** (`src/lib.rs`, `create_winit_window_attributes()`, marked
"ApexDesk patch"): the first window gets the token
(`with_activation_token`) and the variables are cleared, so no later window
or program started from ApexDesk uses it again.

**Android's clipboard** (`src/clipboard.rs`, marked "ApexDesk patch"):
without arboard there, egui-winit kept copied text to itself. The program
hands in the system clipboard's read and write (`clipboard::ANDROID`, through
Java), so text crosses to and from other apps. Other systems are unchanged.

**Updating egui.** Copy the new `egui-winit` over this folder and apply the
changes again. If egui-winit reads the token itself, remove this folder and
its `[patch.crates-io]` entry.

## arboard 3.6.1 (clipboard in several formats)

A copy of `arboard` 3.6.1 from crates.io with one change, used through
`[patch.crates-io]` in the workspace `Cargo.toml`.

**Why.** arboard reads and writes text, HTML and images, one at a time.
Formatting crosses between programs only when the same copy is offered in
several formats at once (text, HTML, RTF, and ApexDesk's own formats for
other ApexDesk windows), and is read in whichever format the other
program offers.

**What changed** (marked "ApexDesk patch"):

- `src/lib.rs`: `Get::format()` (the first of several formats the
  clipboard holds), `Get::formats()` (all of them) and `Set::formats()`
  (one copy in several formats). Formats are named by MIME type.
- `src/platform/linux/x11.rs`: the X11 side of the three, by atom. Text
  is also served for the targets `text/plain`, `text/plain;charset=utf-8`
  and `TEXT`, which XWayland asks for when a Wayland program pastes; stock
  arboard only served `UTF8_STRING`, so Wayland programs pasted nothing.
- `src/platform/windows.rs`: `text/html` is "HTML Format" (with its
  header), `text/rtf` "Rich Text Format", any other name a registered
  clipboard format.
- `src/platform/osx.rs`: text and HTML.
- `src/platform/linux/mod.rs`: the Wayland data-control backend answers
  that it can't (ApexDesk uses smithay-clipboard there).

**Updating.** Copy the new `arboard` over this folder and apply the change
again. If arboard gains multi-format copies, remove this folder and its
`[patch.crates-io]` entry.

## smithay-clipboard 0.7.3 (clipboard formats and dropped files on Wayland)

A copy of `smithay-clipboard` 0.7.3 from crates.io with one change, used
through `[patch.crates-io]` in the workspace `Cargo.toml`.

**Why.** On the Wayland backend egui-winit owns the clipboard through
smithay-clipboard, which only knows text. A second connection can't take
the selection (Wayland only lets the focused window set it, with the serial
of a key press), so the formats go through the clipboard egui-winit runs.

**What changed** (marked "ApexDesk patch"):

- `src/lib.rs`: the live clipboard is kept in a global, and `active()`,
  `store_formats()`, `load_format()` and `formats()` use it.
- `src/worker.rs`: commands to store several formats, load one and list
  them; replies go back by channel with the request's number.
- `src/state.rs`: the stored formats are offered with the selection and
  served when asked for; the offer's formats are listed and read.
- `src/state.rs`, `src/lib.rs`: files dragged onto the window
  (`text/uri-list`, copy only) are read on drop and kept for
  `take_dropped()`; `hovering()` tells a drag is over the window and
  `set_drop_waker()` sets what to call when either changes. winit 0.30
  only gives dropped files on X11.

**Updating.** Copy the new `smithay-clipboard` over this folder and apply
the change again.

## tiberius 0.13.0 (TLS with ring)

A copy of `tiberius` 0.13.0 from crates.io, used through `[patch.crates-io]`
in the workspace `Cargo.toml`, without its docker files, docs, examples and
tests.

**Why.** tiberius's `rustls` feature builds `tokio-rustls` with its default
features, which bring aws-lc, a C crypto library, and it falls back to the
aws-lc provider. ApexDesk's other TLS (web, PostgreSQL, MySQL) uses ring.

**What changed** (marked "ApexDesk patch"):

- `Cargo.toml`: `tokio-rustls` without default features, with `logging`,
  `tls12` and `ring`.
- `src/client/tls_stream/rustls_tls_stream.rs`: the fallback provider is
  ring's (imported under the old name, so the rest of the file is unchanged).

## read-fonts 0.41.0 (a budget for CFF charstrings)

A copy of `read-fonts` 0.41.0 from crates.io, used through
`[patch.crates-io]` in the workspace `Cargo.toml`.

**Why.** The charstring interpreter limits how deep subroutines nest (10)
but not how many times they are called, so a subroutine that calls the
next one ten times, ten levels down, runs ten billion times: a few bytes
of a font embedded in a document hang the program and fill the memory
with outline.

**What changed** (`src/ps/cs.rs`, marked "ApexDesk patch"): the evaluator
counts the operators and operands it runs for a glyph, subroutines and
seac accents included, and stops with an error past 2^20.
