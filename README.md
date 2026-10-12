# ApexDesk

**English** · [Español](#español)

ApexDesk is a light, fast office suite: documents, spreadsheets,
presentations, PDF, databases and notebooks in one program. It opens and
saves Microsoft Office (.docx, .xlsx, .pptx) and LibreOffice (.odt, .ods,
.odp) files. Written in Rust: one small native executable, no runtime
dependencies.

- No account, no cloud, no telemetry. It works offline.
- One small program that starts instantly.
- Windows, Linux and Android.

This repository holds the source code. Ready-made installers are under
**[Releases](../../releases/latest)**, and on the
[website](https://apexdesk.github.io/download.html).

## License

The source code is free software: you can redistribute it and/or modify it
under the terms of the **GNU General Public License, version 3** or (at
your option) any later version. See [LICENSE](LICENSE).

**ApexDesk Empresa**, the business features (business licenses, batch
processing, document control, check before sharing, bank reconciliation,
company templates, signing circuits, file inventories and the automation
API), is not in this repository and is offered under a commercial license.
Companies that would rather not take on the GPL's terms can get ApexDesk
under a commercial license too.

The official builds under Releases come with the
[license agreement](EULA.md): free for individuals, the self-employed,
companies with up to 100 employees and US$1 million in annual revenue,
education, small municipalities, volunteer associations and religious
denominations; free until 31 December 2027 for other companies. Licenses
from larger companies, together with voluntary donations, pay for support
and updates for everyone else.

Contributions are welcome. So that ApexDesk can still be offered under both
licenses, by sending a pull request you agree that your contribution may be
distributed under the GPL and under ApexDesk's commercial licenses.

The open-source components it uses are listed in
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

## Features

- **Spreadsheet**: formulas (English and Spanish function names) with
  autocomplete, argument hints, colored references and a function wizard;
  number formats and a Format Cells dialog; fonts, borders, fill, alignment,
  indent, rotation and merged cells; paste special; format painter; insert and
  delete cells, rows and columns; hidden rows, columns and sheets; tab colors;
  frozen panes; fill series; custom sort; find & replace; Go To; AutoSum;
  selection statistics; undo/redo with history.
- **Data tools**: autofilter (value lists with search, text/number/date
  filters, top 10, filter by color, sort from the filter); data validation
  with dropdown lists, input messages, error alerts and invalid-data circles;
  conditional formatting (cell rules, top/bottom, above average, duplicates,
  formulas, data bars, color scales and icon sets) with a rules manager;
  remove duplicates; text to columns; grouping and outline of rows and
  columns; automatic subtotals; autocomplete from the column's values.
- **Analysis**: tables (Ctrl+T) with styles, a total row, structured
  references, automatic expansion and calculated columns; pivot tables with a
  field list (filters, columns, rows, values), summarize by, show values as,
  grouping and refresh; Goal Seek, data tables, scenarios and Solver
  (simplex and nonlinear); Data Analysis tools (descriptive statistics,
  correlation, regression, histogram, t-tests, ANOVA...); Flash Fill
  (Ctrl+E) and custom lists; split window, new window and side by side.
- **Functions**: about 370 Excel functions (math, statistics and
  distributions, text with regular expressions, date, lookup, financial,
  information and engineering); dynamic arrays that spill (FILTER, SORT,
  UNIQUE, SEQUENCE, VSTACK...) with `A1#` references; LET and LAMBDA with MAP,
  REDUCE, SCAN, BYROW, BYCOL and MAKEARRAY. Formula auditing: trace
  precedents and dependents, error indicators with quick fixes, an error
  checking dialog and Calculate now (F9).
- **Notes, links and protection**: cell notes (red indicator, shown on hover,
  show all, previous/next); hyperlinks to web pages, e-mail addresses or
  places in the workbook (Ctrl+K, Ctrl+click to follow); protect sheets and
  the workbook structure with an optional password, locked and hidden cells
  and a choice of what users may still do (compatible with Excel and
  LibreOffice). In documents: hyperlinks with screen tips, bookmarks and
  automatic links while typing. Both programs have an Insert Symbol dialog
  with Unicode names and recently used symbols.
- **Page setup, printing and PDF**: paper size, orientation and margins;
  headers and footers with page, pages, date, time, file and sheet fields
  (different first page and odd/even pages); for sheets also scaling and fit
  to pages, print area, rows/columns to repeat, centering, gridlines and
  headings, page order, manual page breaks and a page break preview. Print
  (Ctrl+P) with a page preview, printer, copies and page range (through CUPS),
  and export to PDF with embedded font subsets, working links and bookmarks
  from headings or sheets. All of it is kept in xlsx, ods, docx and odt.
- **Tables and pictures**: document tables inserted from a grid, with rows and
  columns to add or delete, merged and split cells, column widths dragged with
  the mouse, borders, shading, table styles with header/banded options,
  repeated header rows across pages, text to table and back, and sorting.
  Pictures from a file or the clipboard, in line with the text or wrapped
  (square, top and bottom), resized with handles, cropped, rotated and with
  alt text; in sheets, pictures float over the cells, move and resize with
  the mouse, and print. Kept in docx, odt, xlsx and ods (Markdown gets pipe
  tables and embedded pictures).
- **Documents**: paginated editing with system fonts, paragraph styles,
  bullet and numbering styles, indents and spacing, line spacing, shading, a
  Paragraph dialog, a ruler with draggable indents and margins, formatting
  marks, change case, format painter, paste without formatting, find &
  replace with whole words and a match counter, Go To page, date and time,
  undo/redo with history. Section and column breaks, text columns and
  orientation per section, watermark, page color and page borders, drop
  caps, tab stops with leaders and paragraph borders. Footnotes and
  endnotes, a table of contents that updates, cross-references and fields
  (page, number of pages, date, author, title) and a navigation pane.
  A styles gallery and pane: modify styles, new styles from the selection
  and character styles, saved in docx and odt.
- **Review**: spell checking as you type (Hunspell dictionaries from the
  system, suggestions in the context menu, a personal dictionary) and a
  Spelling dialog (F7) in both programs; AutoCorrect (replacements, smart
  quotes, capital at the start of sentences, automatic lists) with an
  options dialog; word count (Ctrl+Shift+G). In documents, comments in the
  margin with replies and resolve, track changes (accept and reject) and
  Compare documents.
- **Charts**: column, bar, line, area, pie, doughnut, scatter, combo,
  waterfall and histogram charts with a chart editor (data, elements, axes,
  styles), live data, move and resize; sparklines (line, column, win/loss);
  copy a chart as a picture into a document. Saved in xlsx and ods.
- **Equations**: an equation editor with linear input in the style of
  UnicodeMath (fractions, scripts, roots, n-ary operators with limits,
  brackets, matrices) and a live preview, inline or display; saved as OMML in
  docx, MathML in odt and LaTeX in Markdown.
- **Shapes and text boxes** (both programs): rectangles, ovals, triangles,
  polygons, block arrows, lines and arrows, stars and callouts with fill,
  outline, dash, rotation and text; draw, move, resize, rotate, reorder and
  format them. In documents they sit in front of or behind the text, in line
  or with square and top-and-bottom wrapping; in sheets they float over the
  cells and move with them. Saved in docx, odt, xlsx and ods.
- **Mailings, templates and export**: mail merge with data from a
  spreadsheet or CSV file (merge fields, merge to a new document or to PDF),
  labels (Avery sheets) and envelopes; new from template and save as template
  (built-in letter, report, minutes, budget and invoice templates); export
  documents to HTML and EPUB.
- **Shared**: an adaptive toolbar that folds into a » menu in narrow windows,
  split buttons with recent colors and a color picker, zoom controls, full
  screen and a searchable shortcuts window (Help → Keyboard shortcuts).
- **Cell styles and themes**: Excel's built-in cell styles (Good/Bad/Neutral,
  data and model, titles and headings, themed accents, number formats) in a
  gallery, plus new, modified, duplicated and deleted styles; color and font
  themes (Office, Grayscale, Blue, Green, Paper…) shared by spreadsheets
  (Page Layout) and documents (Design), with theme colors in the color pickers
  and +Headings/+Body fonts that follow the theme. Saved to `.xlsx`/`.docx`
  (theme part, theme colors and fonts) and `.ods`/`.odt` (named styles,
  resolved theme values).
- **Formats**: `.xlsx`, `.ods`, `.fods`, `.csv`, `.tsv` · `.docx`, `.odt`, `.fodt`, `.rtf`, `.md`, `.txt` (export to `.html`, `.epub` and PDF).
  Read only: the binary `.xls` (Excel 5.0 to 2003), `.xlsb` and `.doc`
  (Word 97-2003) — cells, formulas, styles, names, notes and links; text,
  formatting, lists, tables, pictures, notes, headers and footers — saved as
  another format. Encrypted binary files aren't supported.
- **Languages**: English (default), Spanish, French, German, Italian and
  Portuguese; more can be added without touching the code (see below).

## Building

```sh
cargo build --release          # binary: target/release/apexdesk
cargo run --release -- file.xlsx
```

Linux needs the usual X11/Wayland development headers that `winit` uses.

## Installing on Linux

```sh
packaging/linux/install.sh              # build and install for the current user
packaging/linux/install.sh --uninstall
```

This puts `apexdesk` in `~/.local/bin` and adds the icon and a launcher
entry, so ApexDesk shows up in the applications menu and under **Open with**
for `.xlsx`, `.ods`, `.fods`, `.csv`, `.tsv`, `.docx`, `.odt`, `.fodt`, `.md` and
`.txt` files.
It does not change your default applications. Files opened while ApexDesk is
running open as new tabs in the existing window.

Files can also be dragged onto the window. winit's Wayland backend cannot
receive dropped files, so on Linux ApexDesk runs through XWayland when it is
available; set `APEXDESK_WAYLAND=1` to use native Wayland instead (without
drag and drop).

## Building for Windows

From Linux, with `cargo-xwin` (setup steps are at the top of the script):

```sh
packaging/windows/build.sh    # dist/apexdesk-<version>-windows-x86_64.zip
```

The zip holds a single `ApexDesk.exe` that needs no installation. The C runtime
is linked in statically, so no Visual C++ redistributable is needed. On
Windows itself, `cargo build --release` works with Visual Studio's build tools.
There are a few differences from Linux:

- Printing goes through the PDF viewer registered for printing.
- Spelling dictionaries come from LibreOffice, or from a `dictionaries`
  folder next to the program.

## Building for macOS

On a Mac with the Xcode command line tools:

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
packaging/macos/build.sh      # dist/apexdesk-<version>-macos.dmg
```

This makes `ApexDesk.app` for both Apple silicon and Intel, signed ad hoc
(the first time, open it with right click → Open). `APEXDESK_SIGN_ID` signs
with a Developer ID instead.

Files dropped on the window open as tabs. Double clicking a document in the
Finder (**Open with** → ApexDesk) opens it the same way.

## Command line and terminal

The same `apexdesk` binary works without a window. `apexdesk help` lists the
commands, `apexdesk <command> --help` their options; exit status is 0 on
success, 1 on errors and 2 on wrong usage.

```sh
apexdesk convert *.xlsx --to pdf --out pdf/      # also ods csv tsv html md txt
apexdesk convert report.docx --to md             # docx odt rtf md txt pdf html epub
apexdesk convert book.xlsx --to csv --sheet all  # one CSV per sheet
apexdesk cat book.xlsx --sheet 2 --range A1:D20 --format json   # table csv tsv json md
apexdesk cat report.docx                         # text with box-drawn tables
apexdesk eval book.xlsx "SUM(B2:B10)/2"          # English names, ',' between arguments
apexdesk get book.xlsx B3 Totals!A1:C4
apexdesk set book.xlsx B3 42 C3 "=B3*2"          # saves the file (or --output other.ods)
apexdesk recalc book.xlsx
apexdesk merge letter.docx people.csv --out letters/ --name "{n}-{Name}" [--pdf] [--single]
apexdesk info book.xlsm --json                   # sheets, pages, author, macros...
```

`apexdesk tui [file]` (or `apexdesk --tui`) edits a spreadsheet or document in
the terminal: the whole grid with formulas, autocomplete, sorting, filters,
find and replace, formats, undo and mouse; documents in a reading view with
editing that keeps their formatting. F1 shows the keys, F10 or Ctrl+P every
command. Files are read and saved exactly as in the window.

On Windows the release binary is a GUI program: `cmd` does not wait for it,
so run commands as `start /wait apexdesk ...` (or pipe the output) to see
their output before the prompt and get `%ERRORLEVEL%`; `tui` opens its own
console window.

## Workspace layout

| Crate        | Purpose                                                   |
|--------------|-----------------------------------------------------------|
| `apex-sheet` | Spreadsheet engine: cells, formulas, recalculation, undo  |
| `apex-doc`   | Document model: paragraphs, styled runs, editing, undo    |
| `apex-io`    | File formats (zip/xml readers and writers, binary Office readers, csv, markdown) |
| `apex-i18n`  | Interface texts in every language, function help          |
| `apex-tui`   | The terminal interface (`apexdesk tui`)                   |
| `apex-app`   | The desktop application and command line (`apexdesk` binary) |

## Adding a language

Translations live in `crates/apex-app/i18n/<code>.txt`, one `key = text` per
line. Copy `en.txt` to e.g. `fr.txt`, set `lang.name = Français` and translate
the texts; keys you leave out fall back to English. The build embeds every
file in that folder automatically, and the new language appears under
**View → Language**. `cargo test -p apex-app` checks that all keys exist.

## Testing the UI

The app can be driven by a script for end-to-end and performance checks:

```sh
APEXDESK_SCRIPT=script.txt cargo run -p apex-app
```

See `crates/apex-app/src/script.rs` for the commands (typing, keys, clicks,
screenshots).

---

## Español

ApexDesk es una suite ofimática ligera y rápida: documentos, hojas de
cálculo, presentaciones, PDF, bases de datos y cuadernos de notas en un
solo programa, escrito en Rust. Los instaladores están en
**[Releases](../../releases/latest)** y en la
[web](https://apexdesk.github.io/download.html).

**Licencia:** el código fuente es software libre bajo la **Licencia Pública
General de GNU, versión 3** o posterior ([LICENSE](LICENSE)). Los recursos
de empresa (**ApexDesk Empresa**) no están en este repositorio y se ofrecen
con licencia comercial, igual que ApexDesk para las empresas que prefieran
no asumir las condiciones de la GPL. Las versiones oficiales se rigen por el
[contrato de licencia](EULA.es.md): gratis para particulares, autónomos,
empresas de hasta 100 empleados y 1 millón de dólares de facturación anual,
educación, pequeños municipios, asociaciones de voluntarios y confesiones
religiosas; gratis hasta el 31 de diciembre de 2027 para las demás. Las
licencias de las empresas grandes, junto con las donaciones voluntarias,
pagan el soporte y las actualizaciones para todos los demás.

Las contribuciones son bienvenidas; al enviar un pull request aceptas que
tu contribución se distribuya bajo la GPL y bajo las licencias comerciales
de ApexDesk.
