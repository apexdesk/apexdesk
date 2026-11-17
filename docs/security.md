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
  - `Zone.Identifier` (Internet or untrusted zone) on Windows;
  - `com.apple.quarantine` or `kMDItemWhereFroms` on macOS;
  - `user.xdg.origin.url` or `user.xdg.referrer.url` on Linux.
- On Linux few programs set those marks, and some file systems keep none:
  files in the downloads folder (`XDG_DOWNLOAD_DIR`), the temporary
  folders (`$TMPDIR`, `/tmp`, `/var/tmp`, where mail programs open
  attachments) and the cache folder open in Protected View too. The
  folder isn't a mark: it doesn't follow the copies saved from the file,
  and it doesn't stop `run-macro`.
- Such files open read-only, with macros off, until the user enables
  editing. View > New Window of such a file is in Protected View too;
  enabling editing in one window enables it in all of them.
- A file asks once. One the user enabled editing in, or saved from
  ApexDesk, is kept in `trusted-documents.txt` in the settings folder
  (path, length and time of last change, the newest 1000) and opens as
  usual from then on; changed by another program, or downloaded again
  under the same name, it asks again. Macros can't reach that file.
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
  - Hidden files and folders (`.ssh`, `.bashrc`) are never reached, and
    neither are ApexDesk's own files (its settings, the personal macro
    workbook, the add-ins, the policy file), on any system and even
    when the user picked them: a macro can't make itself run at every
    start or lift a policy.
  - Network shares and device paths are refused before the system is
    asked about them, unless the workbook is on that share or the user
    picked a file there: a path to someone's server would hand it the
    user's sign-in.
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
  and SHA-256. A self-signed certificate gets a warning there, and
  Cancel is the default button. A V3 signature made with SHA-1 doesn't
  count. An authority with name constraints vouches only for the names
  it allows (e-mail, DNS and directory names; one with constraints of
  another kind vouches for no one).
- Users sign their own projects (Sign… in the macro editor) only with a
  code-signing certificate. The project is written so the V3 hash has a
  single reading, and the signature is checked before it is kept. Any
  change to the code drops the signature.
- Policies:
  - `MacroSecurity`;
  - `TrustedLocations`;
  - `TrustedPublishers`: when set, only macros signed by these
    certificates run, and users can't add more;
  - `MacroRevocation`: `check` (default), `off` or `require`. Before
    macros signed through an authority are enabled, the authority is
    asked whether the certificate was revoked (OCSP, else its CRL, 5
    seconds at most; the answer is kept for a day). A revoked
    certificate never runs; with no answer the macros run with a notice,
    or not at all under `require`. An answer that has expired, or is
    dated in the future, is no answer. `run-macro`, the API and the
    add-ins loaded at start are held to the same. Self-signed certificates and
    publishers trusted by name aren't asked about;
  - `DisableMacroEditor`.

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
  Macros are enabled for the code the user saw, not for the tab: code
  that changes without the user typing it (an assistant's edit, an undo
  or redo that brings other code back) is not enabled, and a run that
  was waiting doesn't start.

**Automation API** (`apexdesk api`)

- Off by default; business license only.
- It serves only the process that started it: no network port.
- Policy: `AllowAutomationAPI`.

**Policies** (`crates/apex-app/src/policy.rs`)

- A policy that only takes something away applies without a business
  license: the safety policies (macros, links, Protected View, Get Data,
  certificates, signatures), `DisableMacroEditor`, `OnlySharedTemplates`,
  `CheckBeforeSharing` (`warn`, `block`), and `AllowAIAssistants` and
  `AllowAutomationAPI` set to 0. The others need one; those left out are
  listed by `apexdesk policy` and written to the log.
- A policy file is read as UTF-8 (a byte order mark is skipped) or, when
  it isn't, as Latin-1. `Name: value` and `Name value` lines of a known
  policy count as `Name = value`.
- A safety policy with a value that can't be read takes its safest one
  (`MacroSecurity = disabled`, `ProtectedView = 1`, `LinkSecurity =
  block`, `MacroRevocation = require`, the switches off). It and any
  other value ignored are shown in Options and written to the log.
- A policy file that exists but can't be read is written to the log.

**Get Data from the web** (`crates/apex-query/src/sources/web.rs`)

- An address the user wrote as local (an address of this computer or
  network, localhost, a name without dots or under `.local`, `.lan`…)
  is reached as written.
- From an address on the internet, nothing leads into this computer or
  the local network:
  - the addresses a name resolves to are checked when connecting, so a
    name that resolves into the network (or changes to, DNS rebinding)
    is not reached;
  - redirections are followed one by one and each is checked the same
    way;
  - through a proxy, names are resolved here and refused when they lead
    inside. The proxy resolves them again; what its DNS says can't be
    checked.
- Hosts that hide an address are never fetched: numbers in another form
  (`2130706433`, `0x7f.1`, `0177.0.0.1`, `127.1`) and local IPv4
  addresses inside IPv6 ones (`::ffff:127.0.0.1`, NAT64, 6to4).
- Policy: `DisableGetData`.

**Database servers** (`crates/apex-query/src/sources/db.rs`, `sqlserver.rs`)

- New connections are encrypted with TLS and check that the server's
  certificate is valid for its name: against the Mozilla roots built in
  (PostgreSQL, MySQL; a certificate of an organization's own authority
  isn't accepted there) or the system's (SQL Server). A server that
  doesn't offer TLS is refused, never asked again in clear text.
  Turning TLS off is the user's choice, on the form;
  on the command line, `?tls=off`. Connections saved without TLS keep
  working as saved; a database's linked tables say so when asking for
  the password.
- Without TLS, PostgreSQL and MySQL send everything in clear text, and
  SQL Server is encrypted without checking the certificate.
- Queries only read (see `read_only_sql`).

**Serving the browser version** (`apexdesk serve`)

- Passwords are kept as Argon2 hashes. A wrong user takes as long as a
  wrong password.
- Failed sign-ins are counted by client address (an IPv6 client by its
  /64), before the password is checked: five in 15 minutes and that
  address waits. Other addresses can still sign in. Behind a proxy on
  the same computer, `--proxied` takes the client's address from the
  last `X-Forwarded-For`; without it, all clients share the proxy's
  address and its limit.
- Limits before signing in: 20 seconds for a request's headers (16 KB)
  and the TLS handshake, 60 seconds for a body of up to 1 MB (4 KB for
  a sign-in), 32 connections from one address and 256 in all. Larger
  bodies are only read from signed-in users.
- Requests for a name the server doesn't go by are refused (DNS
  rebinding): on 127.0.0.1 only addresses and localhost, plus the names
  `--host` gives. Listening on the network without `--host`, any name
  goes.
- Changes need a request from the server's own pages (`Origin`). The
  session cookie is `HttpOnly` and `SameSite=Strict`, and `Secure` over
  HTTPS (or behind `--proxied` when the proxy says HTTPS). Every reply
  has a strict Content-Security-Policy, `nosniff`, `X-Frame-Options:
  DENY` and, over HTTPS, `Strict-Transport-Security`.
- A user reaches only their own folder: plain names, no hidden files,
  links leading out are refused.
- Get Data reaches only the database servers `--db-hosts` names, and
  only with a user and password the page gives: never the server's own
  Windows account, nor a database's sign-in without a password.
