#!/usr/bin/env bash
# GovDocs1 (Garfinkel et al., Digital Corpora): one million files from
# US government web sites in 1000 ZIP files of 1000. Taken with HTTP range
# requests, without downloading the ZIP files: every .docx and .xlsx of
# the corpus (about 170 and 50; the central directories of all 1000 ZIP
# files are read), and the .doc and .xls of the first GOVDOCS_ZIPS ZIP
# files (default 10, about 800 .doc and 650 .xls).
source "$(dirname "$0")/lib/common.sh"

DIR="$(corpus_dir govdocs1)"
BASE="https://digitalcorpora.s3.amazonaws.com/corpora/files/govdocs1/zipfiles"
OLD="${GOVDOCS_ZIPS:-10}"

for i in $(seq 0 999); do
    n="$(printf %03d "$i")"
    [ -f "$DIR/.done/$n" ] && continue
    ext="docx,xlsx"
    [ "$i" -lt "$OLD" ] && ext="docx,xlsx,doc,xls"
    python3 "$TOOLS_CORPUS/lib/zipsample.py" "$BASE/$n.zip" "$DIR/files" --ext "$ext"
    mkdir -p "$DIR/.done" && touch "$DIR/.done/$n"
done
write_manifest "$DIR"
