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
  - the shell or other programs (`Shell`, `SendKeys`, `Declare`, objects
    other than Collection, Dictionary and FileSystemObject);
  - the registry and environment variables;
  - the network.
- Files (`crates/apex-vba/src/files.rs`): a macro reaches only the
  workbook's folder (below it) and what the user picks in a file dialog.
  - Links, devices and paths that leave those folders are refused.
  - Changing anything (writing, deleting, renaming, saving the workbook
    elsewhere) asks the user once per run, naming the folder.
  - "Allow always for this file" skips that question for the file while
    its macros' code is the same: the settings keep a SHA-256 of the
    file's path and its project, so other code (or another file) asks
    again. Options > Macros forgets them. It only skips the question:
    the limits below still hold.
  - Run Macro > Try runs it on a copy and changes no file: each change
    is refused without asking (granted files too) and named in the
    report; nothing is saved and no OnKey or OnTime is left. The copy
    reaches the workbook only when the user applies it.
  - A workbook in a trusted location reaches any file; changing files
    still asks.
  - Hidden files and folders (`.ssh`, `.bashrc`) are never reached.
  - Programs, scripts, shortcuts and documents that can hold macros are
    never written (Save As of the workbook itself excepted).
  - A function called from a cell reaches no file.
- Files whose code looks like malware (code that runs on opening along
  with starting programs, reaching the internet, calling Windows or
  hiding what it does) get a red bar listing why, and enabling them asks
  twice (`crates/apex-vba/src/suspicious.rs`).
- Workbook_Open runs by itself only for files in the trusted locations.
- "Only signed macros" runs a project only with a V3 signature made for its
  code as it is now, by a code-signing certificate that chains to a
  trusted root or is one of the trusted publishers. Users add publishers
  from the bar of a signed file, after seeing the certificate's full name
  and SHA-256.
- Users sign their own projects (Sign… in the macro editor) only with a
  code-signing certificate. The project is written so the V3 hash has a
  single reading, and the signature is checked before it is kept. Any
  change to the code drops the signature.
- Policies:
  - `MacroSecurity`;
  - `TrustedLocations`;
  - `TrustedPublishers`: when set, only macros signed by these
    certificates run, and users can't add more;
  - `DisableMacroEditor` (business license).

- A VBA project locked with a password is shown read-only until its
  password is typed (checked against the salted SHA-1 hash in the
  file, compared in constant time). As in Office, the lock doesn't
  encrypt the code: the editor says so where the password is set.

- `apexdesk run-macro` runs a macro only with `--trust`. It refuses when
  the organization turned macros off, for files from the Internet, for
  unsigned code under "signed only", and for code the detector flags
  (unless `--allow-suspicious`). File changes need `--allow-writes`;
  MsgBox gets its default button and InputBox is cancelled.
- The API's `run_macro` goes through the same checks and needs
  `trust: true`. Over MCP, `run_macro` runs only in the open workbook,
  only once its macros are enabled, and never in Protected View;
  `write_macro` turns the workbook's macros off again and can be undone.

**Automation API** (`apexdesk api`)

- Off by default; business license only.
- It serves only the process that started it: no network port.
- Policy: `AllowAutomationAPI`.
