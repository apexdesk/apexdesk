#!/usr/bin/env python3
"""Regenerate Gregorian date orders from pinned Unicode CLDR JSON data.

Usage: python3 scripts/date_orders.py [cldr-dates-full-48.2.0.tgz]
The optional archive supports offline regeneration. Normal builds use the
committed table and need neither network access nor Python.
"""

import hashlib
import io
import json
from pathlib import Path
import re
import sys
import tarfile
from urllib.request import urlopen

VERSION = "48.2.0"
SHA256 = "0256f1cefeca14f7d515be4872dda48fcdd7e75381430f25aafd0143eae5b430"
URL = f"https://registry.npmjs.org/cldr-dates-full/-/cldr-dates-full-{VERSION}.tgz"
ROOT = Path(__file__).resolve().parent.parent

data = Path(sys.argv[1]).read_bytes() if len(sys.argv) > 1 else urlopen(URL).read()
if hashlib.sha256(data).hexdigest() != SHA256:
    raise SystemExit("CLDR archive checksum mismatch")
orders = {}
with tarfile.open(fileobj=io.BytesIO(data)) as archive:
    license_text = archive.extractfile("package/LICENSE").read().decode()
    for name in archive.getnames():
        if not name.endswith("/ca-gregorian.json"):
            continue
        tag = name.split("/")[2]
        locale = json.load(archive.extractfile(name))["main"][tag]
        pattern = locale["dates"]["calendars"]["gregorian"]["dateFormats"]["short"]
        if isinstance(pattern, dict):
            pattern = pattern["_value"]
        # CLDR quotes literal text; doubled apostrophes are literals too.
        pattern = re.sub(r"'(?:[^']|'')*'", "", pattern)
        fields = []
        for field in pattern:
            field = {"y": "y", "M": "m", "L": "m", "d": "d"}.get(field)
            if field and field not in fields:
                fields.append(field)
        order = "".join(fields).capitalize()
        if order not in ("Dmy", "Mdy", "Ymd"):
            raise SystemExit(f"Unsupported date pattern: {tag}: {pattern}")
        orders[tag.lower()] = order

lines = [
    f"// Generated from Unicode CLDR {VERSION} Gregorian short-date patterns.",
    "// Regenerate: python3 scripts/date_orders.py",
    "// SPDX-License-Identifier: Unicode-3.0",
    *[f"// {line}".rstrip() for line in license_text.splitlines()],
    "",
    "use super::DateOrder;",
    "",
    "pub(super) const ORDERS: &[(&str, DateOrder)] = &[",
    *[f'    ("{tag}", DateOrder::{order}),' for tag, order in sorted(orders.items())],
    "];",
    "",
]
output = ROOT / "crates/apex-sheet/src/format/date_orders.rs"
output.parent.mkdir(exist_ok=True)
output.write_text("\n".join(lines))
print(f"Wrote {len(orders)} locale date orders to {output}")
