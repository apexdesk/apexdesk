# Security checks

Rust rules out whole classes of bugs, such as buffer overflows and use after
free. It does not rule out:

- a vulnerable or abandoned dependency;
- a reader that panics, hangs or eats all memory on a crafted file;
- a document that gets the program to open something dangerous.

This page covers what is checked before each release, and how.

## Before each release

```sh
scripts/security-check.sh --fuzz 2h
```

The script fixes nothing. It exits non-zero when something needs a look. It
runs three checks.

**1. `cargo audit`**

- Checks the dependencies in `Cargo.lock` against the RustSec advisories.
- Warnings fail too: unmaintained and unsound crates count.

**2. `cargo deny check`**

It follows `deny.toml`. It checks:

- the advisories again, plus crates yanked from crates.io;
- licenses: only those that allow a closed program, listed in `deny.toml`;
- sources: only crates.io, with our own patches in `vendor/`;
- wildcard versions.

**3. `apex-bench fuzz`**

- Runs for the time given; `--fuzz` can be left out for a quick check.
- It mutates real files for every reader (xlsx, xlsb, xls, ods, csv, docx,
  doc, odt, rtf, md, txt, VBA projects, encrypted files).
- Each case is opened, recalculated, saved and opened again, in a child
  process with limits of time and memory.
- Every distinct failure is reduced and kept in
  `target/security/fuzz-<date>/crashes/` next to its trace.
- `report.md` in the same folder sums the run up.

A release goes out when all three are clean.

**Advisories.** An advisory that does not apply to how ApexDesk uses the
crate goes in `deny.toml` under `[advisories] ignore`, with the reason.

**Fuzz failures.** Each failure is a bug to fix. Replay one with:

```sh
target/release/apex-bench fuzz --replay FILE
```

The tools are installed once:

```sh
cargo install --locked cargo-audit cargo-deny
```

## Between releases

Fuzz regularly, for example overnight once a week:

```sh
scripts/security-check.sh --fuzz 8h
```

**Readers to prioritise.** Put the most time into the readers that take files
from outside:

- the Office formats (`--format xlsx,docx,xls,doc`);
- encrypted files (`crypto`);
- VBA projects (`vba`).

**Seeds.** New seeds find new paths.

- `apex-bench fuzz --make-seeds DIR` writes a varied set.
- Files that once broke a reader are good seeds too. Put them in the fuzz
  output's `seeds/` folder.

## In the program

These protections live in the code and are covered by its tests.

**Links from documents** (`crates/apex-app/src/links.rs`)

- Web and mail links open.
- These never open:
  - programs and scripts;
  - schemes that run code;
  - network paths on Windows.
- Everything else asks first.
- Policy: `LinkSecurity`.

**Embedded objects** (`crates/apex-app/src/app/objects.rs`): a program or
script embedded in a document is never opened.

**Protected View** (`crates/apex-app/src/app/protected.rs`)

- Covers files the system marks as downloaded:
  - `Zone.Identifier` on Windows;
  - `com.apple.quarantine` on macOS;
  - `user.xdg.origin.url` on Linux.
- Such files open read-only, with macros off, until the user enables
  editing.
- Files in the trusted locations are exempt.
- Policy: `ProtectedView`.

**Macros**

- They are off by default.
- When turned on, they run in the `apex-vba` interpreter, which has no access
  to:
  - files;
  - the shell;
  - the registry;
  - other programs.
- Workbook_Open runs by itself only for files in the trusted locations.
- Policy: `MacroSecurity`.

**Automation API** (`apexdesk api`)

- Off by default; business license only.
- It serves only the process that started it: no network port.
- Policy: `AllowAutomationAPI`.
