#!/usr/bin/env python3
"""Regenerate THIRD-PARTY-NOTICES.md at the repository root.

Walks the `apex-app` binary's normal (non-dev) dependency closure via
`cargo metadata`, picks the permissive side of each dual/triple-licensed
crate, and groups crates by the resulting license text. License texts are
not retyped by hand: for each license family we reuse the verbatim file
found in a crate's own package directory under `~/.cargo/registry/src`
(these are the crate's own words, already publicly distributed with it),
so wording is never guessed. Per-crate copyright lines are extracted from
that same file when present.

Three non-crate items are added by hand at the bottom of the script:
- vendor/epaint (a local `[patch.crates-io]` fork of the `epaint` crate,
  MIT OR Apache-2.0 like upstream egui, but shipped without a bundled
  LICENSE file in this tree).
- crates/apex-doc/data/preset-shapes.xml (ECMA-376 preset shape geometry
  data, Copyright Ecma International; used under the ECMA International
  standard's own usage terms, not a traditional open-source license).
- pdfium's fonts for the standard 14 PDF fonts, built into the program.

Usage: python3 scripts/third_party_notices.py
Only needs the Python standard library and a local `cargo` + populated
`~/.cargo/registry/src` cache (i.e. the workspace must have been built at
least once).
"""

import json
import os
import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
OUT_PATH = REPO_ROOT / "THIRD-PARTY-NOTICES.md"

# When a crate's Cargo.toml license expression offers a choice, prefer the
# most permissive/simplest option in this order.
LICENSE_PRIORITY = [
    "MIT", "Apache-2.0", "BSD-3-Clause", "BSD-2-Clause", "0BSD", "Zlib",
    "ISC", "BSL-1.0", "Unicode-3.0", "CDLA-Permissive-2.0", "MPL-2.0",
    "Unlicense",
]

# Copyleft identifiers that would block a proprietary build if they were
# the ONLY option for a crate (i.e. not just one arm of an OR).
COPYLEFT = {"GPL-2.0-only", "GPL-2.0-or-later", "GPL-3.0-only",
            "GPL-3.0-or-later", "LGPL-2.1-only", "LGPL-2.1-or-later",
            "LGPL-3.0-only", "LGPL-3.0-or-later", "MPL-2.0", "EPL-1.0",
            "EPL-2.0", "AGPL-3.0-only", "AGPL-3.0-or-later"}

LICENSE_FILENAME_HINTS = {
    "MIT": ["LICENSE-MIT", "LICENSE_MIT", "LICENSE-mit", "MIT-LICENSE"],
    "Apache-2.0": ["LICENSE-APACHE", "LICENSE_APACHE", "APACHE-LICENSE"],
    "BSD-3-Clause": ["LICENSE-BSD", "LICENSE-BSD-3-Clause"],
    "BSD-2-Clause": ["LICENSE-BSD-2-Clause"],
    "Zlib": ["LICENSE-ZLIB", "LICENSE-Zlib"],
    "ISC": ["LICENSE-ISC"],
    "0BSD": ["LICENSE-0BSD"],
    "BSL-1.0": ["LICENSE-BOOST", "LICENSE_1_0.txt"],
    "Unicode-3.0": ["LICENSE-UNICODE", "LICENSE-Unicode"],
    "CDLA-Permissive-2.0": ["LICENSE-CDLA-Permissive-2.0"],
    "OFL-1.1": ["OFL.txt", "OFL-1.1.txt"],
    "Ubuntu-font-1.0": ["UFL.txt"],
}


def sh(*args):
    return subprocess.run(args, cwd=REPO_ROOT, check=True, capture_output=True, text=True).stdout


def cargo_metadata():
    out = sh("cargo", "metadata", "--format-version=1", "-q")
    return json.loads(out)


def normal_closure(md, root_name="apex-app"):
    packages = {p["id"]: p for p in md["packages"]}
    nodes = {n["id"]: n for n in md["resolve"]["nodes"]}
    root_id = next(p["id"] for p in md["packages"] if p["name"] == root_name)
    seen = set()
    stack = [root_id]
    while stack:
        cur = stack.pop()
        if cur in seen:
            continue
        seen.add(cur)
        node = nodes.get(cur)
        if not node:
            continue
        for dep in node.get("deps", []):
            kinds = dep.get("dep_kinds", [])
            is_normal = (not kinds) or any(dk.get("kind") is None for dk in kinds)
            if is_normal:
                stack.append(dep["pkg"])
    seen.discard(root_id)
    members = set(md["workspace_members"])
    return [packages[i] for i in seen if i not in members]


def split_or(expr):
    # Handle "A OR B", "A/B", "A / B" as equivalent alternatives.
    parts = re.split(r"\s+OR\s+|\s*/\s*", expr.strip())
    return [p.strip().strip("()") for p in parts if p.strip()]


def split_and(expr):
    expr = expr.strip()
    if expr.startswith("(") and expr.endswith(")"):
        inner_depth = 0
        balanced = True
        for i, c in enumerate(expr):
            if c == "(":
                inner_depth += 1
            elif c == ")":
                inner_depth -= 1
                if inner_depth == 0 and i != len(expr) - 1:
                    balanced = False
        if balanced:
            expr = expr[1:-1]
    # Split top-level " AND " only (license exprs here have no nesting deeper
    # than one paren group in practice).
    return [p.strip() for p in re.split(r"\)\s+AND\s+\(|\s+AND\s+", expr)]


def resolve_license_expr(pkg, registry_roots):
    """Return an SPDX-like expression string for a package, falling back to
    sniffing its `license_file` when `license` is absent (some crates only
    declare a `license-file`, e.g. webpki's Cargo.toml points at its own
    ISC-style LICENSE without an SPDX id)."""
    expr = pkg.get("license")
    if expr:
        return expr
    lf = pkg.get("license_file")
    if lf:
        cdir = crate_dir(pkg, registry_roots)
        cand = cdir / lf
        if not cand.is_file():
            matches = list(cdir.rglob(Path(lf).name)) if cdir.is_dir() else []
            cand = matches[0] if matches else None
        if cand and cand.is_file():
            try:
                sniffed = sniff(cand.read_text(encoding="utf-8", errors="ignore"))
                if sniffed:
                    return sniffed
            except OSError:
                pass
    return ""


def choose_license(expr):
    """Return list of chosen SPDX ids (one per AND-clause) for a license expr."""
    if not expr:
        return []
    clauses = split_and(expr)
    chosen = []
    for clause in clauses:
        clause = clause.strip().strip("()")
        options = split_or(clause)
        pick = None
        for pref in LICENSE_PRIORITY:
            if pref in options:
                pick = pref
                break
        if pick is None:
            pick = options[0] if options else clause
        chosen.append(pick)
    return chosen


def find_registry_roots():
    cargo_home = Path(os.environ.get("CARGO_HOME", str(Path.home() / ".cargo")))
    src = cargo_home / "registry" / "src"
    if not src.is_dir():
        return []
    return [d for d in src.iterdir() if d.is_dir()]


def crate_dir(pkg, registry_roots):
    source = pkg.get("source")
    manifest = Path(pkg["manifest_path"])
    if source is None:
        # Path dependency (workspace-local or vendored patch): use its own dir.
        return manifest.parent
    for root in registry_roots:
        cand = root / f"{pkg['name']}-{pkg['version']}"
        if cand.is_dir():
            return cand
    return manifest.parent


# Real copyright notices name a year (or year range) right after the word;
# this also rules out Apache-2.0's own prose ("...as indicated by a
# copyright notice...") and its unfilled "Copyright [yyyy] [name]" appendix
# template, neither of which contains an actual digit year here.
COPYRIGHT_RE = re.compile(
    r"^\s*(?:SPDX-FileCopyrightText:\s*)?Copyright\b\s*(?:\(c\)|©)?\s*\d{4}.*$",
    re.IGNORECASE | re.MULTILINE,
)


def find_license_file(cdir, spdx_id):
    if not cdir.is_dir():
        return None
    hints = LICENSE_FILENAME_HINTS.get(spdx_id, [])
    for h in hints:
        cand = cdir / h
        if cand.is_file():
            return cand
        # Some crates (notably bundled font packages) nest the license a
        # directory or two down (e.g. fonts/OFL.txt).
        matches = list(cdir.rglob(h))
        if matches:
            return matches[0]
    # Fall back to a generic LICENSE/COPYING file, content-sniffed.
    generic = []
    for name in ("LICENSE", "LICENSE.md", "LICENSE.txt", "COPYING", "UNLICENSE"):
        cand = cdir / name
        if cand.is_file():
            generic.append(cand)
    for cand in generic:
        try:
            text = cand.read_text(encoding="utf-8", errors="ignore")
        except OSError:
            continue
        if sniff(text) == spdx_id:
            return cand
    return generic[0] if generic and len(hints) == 0 else None


def sniff(text):
    t = text
    if "Apache License" in t and "Version 2.0" in t:
        return "Apache-2.0"
    if "SIL OPEN FONT LICENSE" in t.upper():
        return "OFL-1.1"
    if "UBUNTU FONT LICENCE" in t.upper():
        return "Ubuntu-font-1.0"
    if "Community Data License Agreement" in t:
        return "CDLA-Permissive-2.0"
    if "UNICODE LICENSE" in t.upper() or ("UNICODE" in t.upper() and "DATA FILES" in t.upper()):
        return "Unicode-3.0"
    if "Boost Software License" in t:
        return "BSL-1.0"
    if t.strip().startswith("This is free and unencumbered software"):
        return "Unlicense"
    if "Redistribution and use in source and binary forms" in t:
        return "BSD-3-Clause" if "Neither the name" in t else "BSD-2-Clause"
    if "zlib License" in t or ("must not be misrepresented" in t and "altered source versions" in t):
        return "Zlib"
    if "Permission to use, copy, modify, and/or distribute this software" in t:
        return "ISC"
    if "Permission is hereby granted, free of charge" in t and "MIT" in t.upper():
        return "MIT"
    if "Permission is hereby granted, free of charge" in t:
        return "MIT"
    return None


def load_license_texts(closure, registry_roots):
    """For each SPDX id actually used, find one representative verbatim
    text file among the crates that declare it (directly, not via an OR
    we didn't pick), preferring crates whose OWN license field is exactly
    that single id (so we know the bundled file really is that license).
    """
    texts = {}
    sources = {}
    single_id_crates = []
    other_crates = []
    for pkg in closure:
        expr = resolve_license_expr(pkg, registry_roots)
        ids = [e.strip() for e in re.split(r"\s+OR\s+|\s+AND\s+|/", expr) if e.strip()]
        if len(ids) == 1:
            single_id_crates.append((pkg, ids[0]))
        else:
            other_crates.append(pkg)
    # Deterministic order, and prefer a candidate whose file has NO filled-in
    # copyright line (a pure template) so the shared, one-per-type text below
    # never reads as if a single crate's author owned the whole bucket; the
    # real per-crate copyrights are listed individually in "Used by:".
    single_id_crates.sort(key=lambda t: (t[0]["name"], t[0]["version"]))
    for spdx_id in {sid for _, sid in single_id_crates}:
        candidates = [pkg for pkg, sid in single_id_crates if sid == spdx_id]
        best = None
        for pkg in candidates:
            cdir = crate_dir(pkg, registry_roots)
            f = find_license_file(cdir, spdx_id)
            if not f:
                continue
            try:
                raw = f.read_text(encoding="utf-8", errors="ignore").strip()
            except OSError:
                continue
            has_copyright = bool(COPYRIGHT_RE.search(raw))
            if best is None or (best[2] and not has_copyright):
                best = (pkg, raw, has_copyright)
            if not has_copyright:
                break
        if best:
            pkg, raw, has_copyright = best
            if has_copyright:
                raw = "\n".join(
                    line for line in raw.splitlines() if not COPYRIGHT_RE.match(line)
                ).strip()
            texts[spdx_id] = raw
            sources[spdx_id] = f"{pkg['name']} {pkg['version']}"
    # Second pass: any bucket still missing a text, scan every crate dir for
    # a file that sniffs as that id.
    still_missing = set()
    for pkg in closure:
        for spdx_id in choose_license(resolve_license_expr(pkg, registry_roots)):
            if spdx_id not in texts:
                still_missing.add(spdx_id)
    if still_missing:
        for pkg in closure:
            if not still_missing:
                break
            cdir = crate_dir(pkg, registry_roots)
            if not cdir.is_dir():
                continue
            candidates = []
            for name in ("LICENSE", "LICENSE.md", "LICENSE.txt", "LICENSE-MIT",
                         "LICENSE-APACHE", "LICENSE-ISC", "LICENSE-ZLIB",
                         "LICENSE-0BSD", "OFL.txt", "UFL.txt", "COPYING"):
                candidates.extend(cdir.rglob(name))
            for cand in candidates:
                if not cand.is_file():
                    continue
                try:
                    text = cand.read_text(encoding="utf-8", errors="ignore")
                except OSError:
                    continue
                sniffed = sniff(text)
                if sniffed in still_missing:
                    texts[sniffed] = text.strip()
                    sources[sniffed] = f"{pkg['name']} {pkg['version']}"
                    still_missing.discard(sniffed)
    return texts, sources, still_missing


PLACEHOLDER_MARKERS = ("[yyyy]", "[name", "{yyyy}", "{name", "<year>", "<name>", "[year]")


def crate_copyright(pkg, spdx_id, registry_roots):
    cdir = crate_dir(pkg, registry_roots)
    f = find_license_file(cdir, spdx_id)
    if not f:
        return None
    try:
        text = f.read_text(encoding="utf-8", errors="ignore")
    except OSError:
        return None
    for m in COPYRIGHT_RE.finditer(text):
        line = m.group(0).strip()
        low = line.lower()
        # Skip the unfilled Apache-2.0 APPENDIX template line, and any other
        # obvious placeholder rather than a real copyright notice.
        if any(marker in low for marker in PLACEHOLDER_MARKERS):
            continue
        return line
    return None


def main():
    md = cargo_metadata()
    closure = normal_closure(md)
    registry_roots = find_registry_roots()

    blocked = []
    buckets = {}  # spdx_id -> list of (pkg, chosen_ids)
    for pkg in closure:
        expr = resolve_license_expr(pkg, registry_roots)
        chosen = choose_license(expr)
        if not chosen:
            chosen = ["(unknown)"]
        # Flag only if EVERY option for some AND-clause is copyleft (i.e. no
        # permissive alternative existed at all).
        clauses = split_and(expr) if expr else []
        for clause in clauses:
            opts = set(split_or(clause))
            if opts and opts <= COPYLEFT:
                blocked.append((pkg["name"], pkg["version"], expr))
        for spdx_id in chosen:
            buckets.setdefault(spdx_id, []).append(pkg)

    texts, sources, missing = load_license_texts(closure, registry_roots)

    lines = []
    lines.append("# Third-Party Notices")
    lines.append("")
    lines.append(
        "ApexDesk is proprietary software (see LICENSE), but it is built on "
        "many open-source Rust crates. This file lists every crate that "
        "compiles into the `apexdesk` binary as a normal (non-dev, non-test) "
        "dependency, grouped by the permissive license each one offers, plus "
        "the ECMA-376 preset shape data, a vendored `epaint` patch and "
        "pdfium's standard 14 fonts."
    )
    lines.append("")
    lines.append(
        f"Generated by `scripts/third_party_notices.py` from `cargo metadata` "
        f"and the local `~/.cargo/registry` cache — {len(closure)} crates. "
        "Re-run the script after dependency changes; do not hand-edit the "
        "crate lists below."
    )
    lines.append("")

    if blocked:
        lines.append("## ⚠ Non-permissive licenses found")
        lines.append("")
        for name, version, expr in blocked:
            lines.append(f"- `{name}` {version}: `{expr}`")
        lines.append("")

    lines.append("## ECMA-376 preset shape geometry")
    lines.append("")
    lines.append(
        "`crates/apex-doc/data/preset-shapes.xml` holds the preset shape "
        "geometries from ECMA-376 Part 1 (Office Open XML), "
        "`presetShapeDefinitions.xml`, with adjust handles and white space "
        "stripped. Copyright Ecma International; used as the ECMA "
        "International standard permits implementers to use it, not under "
        "an open-source license."
    )
    lines.append("")

    lines.append("## vendor/epaint")
    lines.append("")
    lines.append(
        "`vendor/epaint` is a local copy of the `epaint` crate (part of the "
        "`egui` project) 0.36.2, used through `[patch.crates-io]` (see "
        "`vendor/README.md` for the two changes made). Upstream `epaint` is "
        "MIT OR Apache-2.0, like the rest of the egui workspace; this local "
        "copy does not bundle a separate LICENSE file, so no crate-specific "
        "copyright line is reproduced here beyond the MIT and Apache-2.0 "
        "texts already included below for the crates.io copies of `egui`, "
        "`ecolor`, `emath` and related crates."
    )
    lines.append("")

    lines.append("## pdfium's standard 14 fonts")
    lines.append("")
    lines.append(
        "The program has built in pdfium's fonts for the standard 14 PDF fonts (`crates/apex-io/src/pdf/render/std14`, BSD-3-Clause, Copyright 2014 The PDFium Authors), so that its own PDF renderer draws them as pdfium (the PDF engine of the Chromium project) does. pdfium itself is not distributed: it is only a development tool (`tools/pdfium-fetch.sh`) that pages are compared against."
    )
    lines.append("")

    lines.append("## Unicode CLDR date orders")
    lines.append("")
    lines.append(
        "`crates/apex-sheet/src/format/date_orders.rs` contains Gregorian date orders "
        "derived from Unicode CLDR 48.2.0. Copyright © 2004-2026 Unicode, Inc. "
        "Distributed under the Unicode-3.0 license reproduced below; regenerated by "
        "`scripts/date_orders.py`."
    )
    lines.append("")

    order = sorted(buckets, key=lambda k: (-len(buckets[k]), k))
    for spdx_id in order:
        pkgs = sorted(buckets[spdx_id], key=lambda p: (p["name"], p["version"]))
        lines.append(f"## {spdx_id}")
        lines.append("")
        if spdx_id in texts:
            lines.append(f"<details><summary>License text (from `{sources[spdx_id]}`)</summary>")
            lines.append("")
            lines.append("```")
            lines.append(texts[spdx_id])
            lines.append("```")
            lines.append("</details>")
        else:
            lines.append(f"*(no bundled license text found in the registry cache for {spdx_id})*")
        lines.append("")
        lines.append("Used by:")
        lines.append("")
        for pkg in pkgs:
            cr = crate_copyright(pkg, spdx_id, registry_roots)
            if cr:
                lines.append(f"- `{pkg['name']}` {pkg['version']} — {cr}")
            else:
                lines.append(f"- `{pkg['name']}` {pkg['version']}")
        lines.append("")

    OUT_PATH.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"Wrote {OUT_PATH} ({len(closure)} crates, {len(buckets)} license buckets)")
    if missing:
        print(f"WARNING: no license text found for: {sorted(missing)}", file=sys.stderr)
    if blocked:
        print(f"WARNING: {len(blocked)} crate(s) with no permissive option, see the file's warning section", file=sys.stderr)


if __name__ == "__main__":
    main()
