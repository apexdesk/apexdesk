#!/bin/sh
# Writes FILE.sha256 next to each FILE given, in the format `sha256sum -c`
# checks (a hash, two spaces, the file's name):
#
#   scripts/sha256.sh dist/apexdesk_1.2.0_amd64.deb
#
# The packaging scripts call it on what they make. sha256sum on Linux,
# shasum on macOS.
set -eu
if command -v sha256sum >/dev/null 2>&1; then
    sum() { sha256sum "$1"; }
elif command -v shasum >/dev/null 2>&1; then
    sum() { shasum -a 256 "$1"; }
else
    echo "sha256.sh: neither sha256sum nor shasum is installed" >&2
    exit 1
fi
for f in "$@"; do
    # The name alone, so the check runs from the file's own folder.
    (cd "$(dirname "$f")" && sum "$(basename "$f")") > "$f.sha256"
done
