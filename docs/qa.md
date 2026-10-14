# Automatic QA

`scripts/qa.sh` runs deterministic checks (no AI, no screen) over the
whole ApexDesk window and writes a report. It fixes nothing: what it finds
is the result. The checks are ignored tests of `apex-app` named `qa_…`
(module `crates/apex-app/src/app/qa/`, one file per section), driving the
window with the button audit's harness (`crates/apex-app/src/app/audit.rs`):
controls are found in the accessibility tree and clicked with the mouse
where they are drawn, file dialogs are answered by a hook, and nothing
reaches the desktop (clipboard, browser, printer, mail).

## Running it

    scripts/qa.sh            # quick, the default (about 12 minutes)
    scripts/qa.sh full       # everything (much longer)
    scripts/qa.sh --accept-screens   # the screenshots become the baseline

`APEXDESK_QA_MODE=full` does the same as the argument. The script builds
and runs `cargo test -p apex-app --no-fail-fast -- --ignored qa_` inside a
memory cap (`systemd-run … MemoryMax=12G`, `-j 6`, `CARGO_INCREMENTAL=0`)
and prints only the one-line summary of each section, the first 15 new
failures and where the report is. A section that finds failures doesn't
fail the run; the script fails only when the QA itself breaks.

Each section checks every view in a process of its own (the fonts, the
language and a panic are the process's), four at a time
(`APEXDESK_QA_JOBS` changes it). One section alone:

    APEXDESK_QA_OUT=target/qa/mine cargo test -p apex-app -- --ignored --exact app::qa::monkey::qa_monkey

One view of a section, in this process (the findings go to stderr):

    APEXDESK_QA_JOB=Sheet cargo test -p apex-app -- --ignored --exact --nocapture app::qa::invariants::qa_invariants

`APEXDESK_AUDIT_PROGRESS=<file>` follows a run step by step.

## Reading the report

The run's folder is `target/qa/<date-time>/`:

- `report.md`: all the sections, one after another.
- `<section>.md`: a section's detail: the counts, the new failures (lines
  starting with `- `), the known ones (`* `), and notes (what was covered,
  the slowest actions, memory).
- `<section>.sum`: one line, `<section>: N comprobaciones, M fallos, K conocidos`
  (M counts only new failures).

A failure line says the rule broken in brackets, the view, the control
(the clicks that reach it, `Button "Insert" → Button "Page break"`), what
went wrong, how to reproduce it, and its id:

    - [undo] Doc: Button "Insert" → Button "Page break" — undo leaves p0000 text … changed. Repro: open the Doc sample, click …, then Undo [id: Doc / undo / Button "Insert" → Button "Page break"]

The views are samples of medium size built by the QA itself
(`qa/model.rs`): `Doc` (a few pages of headings, lists, tables),
`DocSel` (four body paragraphs selected), `Sheet` (120 rows with
formulas and dates, two sheets), `SheetSel` (A2:C11 selected), `Slides`
(twelve slides with text), `SlidesSel` (the first slide's objects
selected), `Pdf` (that document as a PDF), `Db` (a database with one
table) and `Code` (the document in the code view, View > Code).

The model is compared by parts: `p0003 text`, `p0003 format` (a
paragraph), `s0 values`, `s0 formats`, `s0 layout` (a sheet), `slide002`,
`masters`, `page000` (a PDF page), `table People` (a database table),
`code` (the code view's text), and so on.

## The sections

### invariants (`qa/invariants.rs`)

In each view every control shown is clicked, and every control that shows
after it (a toolstrip tab's buttons, a menu's entries): two clicks down in
the quick run, three in the full one (a dialog's controls). Each from a
fresh window. Then:

- **undo**: when the click changed the model, undo brings it back to its
  print before the click; **redo** to the one after it. Controls that
  change no model are left out.
- **toggle**: a control with an on/off state clicked twice leaves the
  window as it was (the accessibility tree, the settings, the app's
  state); if it edits, the model. One that stays on when clicked again
  (a tab, one choice among several) isn't a toggle.
- **selection** (`DocSel`, `SheetSel`, `SlidesSel`): a format (an action
  that changes only selected elements, and none's text) reaches all the
  selected paragraphs, cells or objects, or none. Borders are left out:
  they rightly go on the edges only.
- **time**: the frames of the click and the one after it take less than
  250 ms (in the test build, unoptimized); the slowest go to the notes.
- **persist** (full only): after an action that changed the model, the
  document is saved in each native format of the view (docx/odt/md,
  xlsx/ods/csv, pptx/odp) and read back, and so is the document before
  the action; the parts the action changed must differ between the two
  as well (Markdown only compares text, CSV only the first sheet's
  values). The line names the format that loses the change.
- **panic**: a click that panics.

### monkey (`qa/monkey.rs`)

In each view (`Doc`, `Sheet`, `Slides`, `Pdf`, `Db`, `Code`, `Note`, and
`Macros`: a workbook with macros enabled, in the macro editor), sequences
drawn from fixed seeds (quick: 3 seeds × 150 steps; full: 20 × 500) of
clicks on the controls shown, clicks and right-clicks in the page or the
cells, keys (arrows, Enter, Delete, Ctrl+Z/Y/B/I/C/X/V…) and typed text.
The tab's close cross and Quit are left alone. Checks:

- **panic**: none.
- **error**: no error reported (what `Notice::error` shows and logs).
- **undo-all**: undoing everything afterwards brings the first model back.
- **memory**: the process grows by less than 512 MB over a sequence.

A failing sequence is cut down (delta debugging, within a budget of
replays) to the fewest steps that still fail the same way; the line gives
the seed and those steps. The same seed always draws the same steps.

### The screens (`qa/scenes.rs`)

The next sections look at the same screens: the start screen, every
toolstrip tab of each view (`Doc`, `Sheet`, `Slides`, `Pdf`, `Db`; not
the contextual ones), and the dialogs Options, Print, Find, Font,
Paragraph and Insert Table over the document. A screen named `Doc/review`
is the Review tab of the Doc sample; `Doc/Font dialog` the dialog open on
it. They are measured on what egui lays out and paints (the controls'
rectangles, the texts drawn and where they are clipped), not on pixels.
A screen that can't be reached (a button folded away in a small window)
goes to the notes as "not reached".

### layout (`qa/layout.rs`)

Every screen at 1280×800 and at the window's smallest size, 640×400, in
es, en and de (quick) or the ten languages (full); one job a language.

- **cut**: a text drawn wider than the space it is clipped to, or
  shortened with "…".
- **text-overlap**: two different texts drawn over each other.
- **outside**: a control past the window's edge (not one in a list that
  scrolls: its visible part must reach that edge).
- **overlap**: two controls of the same layer over each other, neither
  inside the other.

The id is `<lang> / <size> / <screen> / <rule> / <what>`. What the
document shows (paragraphs, cells, slides) is left out.

### i18n (`qa/i18n.rs`)

The tables `crates/apex-app/i18n/<code>.txt`, against the sources (job
`tables`) and the screens in English (job `en`):

- **missing**: a key English has, or a source asks for by name
  (`tr`/`trf` with a literal), that a language lacks.
- **orphan**: an English key that no source names, whole or by a prefix a
  key is built from (`format!("fn.{name}")`).
- **placeholders**: a translation whose `{name}`s differ from English.
- **brand**: a text naming another suite's feature (Ribbon, SmartArt,
  WordArt, PivotTable).
- **raw-key**: a key's name (`doc.paragraph`) drawn on a screen or given
  as a control's name.
- Values the same as in English only go to the notes, per language, with
  names, units, formats and acronyms left out (`UNIVERSAL` in the file).

### a11y (`qa/a11y.rs`)

Every screen at 1280×800 in English, one job a view (and `DocSel`,
`SheetSel`, `SlidesSel` with something selected):

- **name**: a control with no name a screen reader can say.
- **tooltip**: a button or drop-down of the window (not in a dialog) that
  doesn't show its whole name as text (an icon, a cut label) and shows no
  tooltip when the pointer rests on it.
- **disabled** (the `Sel` jobs): a control that is off although there is
  a target selected, and doesn't say why when pointed at. Some are right
  to be off (Undo with nothing done): put those in `known.txt`.
- **shortcut**: one key combination shown for two different commands of a
  view (from the command search's list).

### screens (`qa/screens.rs`)

Every screen at 1280×800 in English, painted without a screen (the
meshes egui would send to the GPU, filled on the CPU: `qa/raster.rs`) and
compared with the baseline in `target/qa/baseline/<screen>.png` (not in
git: it depends on the fonts of this machine). A screen with more than
0.5 % of its pixels changed is a failure; the line gives the baseline,
the new picture and a diff (the changes in red) under
`<run>/screens/`. With no baseline yet, the screens are saved as it and
the notes say so. After a change that is meant:

    scripts/qa.sh --accept-screens

makes this run's pictures the baseline.

### align (`qa/align.rs`)

- **ruler**: the horizontal ruler's zero (worked out from the numbers it
  draws) stands where the first paragraph's text starts, in print layout
  and in draft, at zoom 100 % and 160 % (full: also 75 %).
- **vruler**: the vertical ruler's zero stands at the text's top (the top
  margin) in print layout.
- **line-numbers**: in the code view, each line number is drawn on the
  row its line starts at.
- **borders**: All Borders, No Border, and All then No, from Table Layout
  on the middle cell of a 3×3 table: the cell's four sides and the facing
  sides of the cells around it follow.

### behave (`qa/behave.rs`)

As a person testing the window sees it: in each view, in a 1280×800
window, in Spanish (full: also German), every control (and what it shows)
is clicked from a fresh window. Checks:

- **drift**: left alone for 30 frames, nothing moves (a panel growing, a
  dialog creeping).
- **inert**: a tool picked changes something when dragged on the page.
- **jump**: using a tool on the page doesn't move the whole view.
- **stuck**: the tool's button clicked again (or Escape) lets it go, also
  after text was selected while it was on.
- **cut**: a dialog it opens fits in the window.
- **esc**: a dialog it opens closes with Escape.
- **error**: no error reported.

`APEXDESK_QA_SHOTS=<dir>` saves the pictures of the failures found.

### stories (`qa/stories.rs`)

Short tasks done as a person would, judged by what shows on the screen
and what is in the file: text selected shows its selection on the text,
strike out on and off, tools that let go, the hand dragging the pages, an
arrow with its head, a stamp in place, typing, bold, a value and a
formula in a sheet, shapes drawn and moved on a slide, a new slide, a
transition, typing in a notebook page, Options closed with Escape in
every view. Each runs in Spanish and German (longer words).

## Known failures

`crates/apex-app/src/app/qa/known.txt` lists the failures accepted for
now, one a line:

    <section>|<id>|<why>

The id is the one in brackets at the end of the report's line
(`[id: Doc / undo / Button "Review" → Button "Track Changes"]` gives
`invariants|Doc / undo / Button "Review" → Button "Track Changes"|…`).
Known failures are counted apart (`K conocidos`) and listed under
"Known failures", not as new. Lines starting with `#` are comments. Take
a line out when the failure is fixed.

## Adding a section

A section is a file in `crates/apex-app/src/app/qa/` with an ignored test
`qa_<section>` that calls `common::run(section, test path, jobs, f)`: `f`
checks one job (a view) in a child process and reports with
`Sink::check`, `Sink::fail(id, line)` and `Sink::note`; `run` writes
`<section>.md` and `<section>.sum`.
