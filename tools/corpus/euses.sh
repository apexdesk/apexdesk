#!/usr/bin/env bash
# EUSES spreadsheet corpus (Fisher and Rothermel, 2005): about 4 500 .xls
# files found on the web, grouped by subject. The original site
# (esquared.unl.edu) and tera-PROMISE are gone; the tera-PROMISE copy is
# on Zenodo, record 581673 (200 MB in 11 ZIP files).
source "$(dirname "$0")/lib/common.sh"

DIR="$(corpus_dir euses)"
API="https://zenodo.org/api/records/581673"

if [ ! -f "$DIR/.done" ]; then
    # name, md5 and URL of each file of the record
    curl -fsSL "$API" | python3 -c '
import json, sys
for f in json.load(sys.stdin)["files"]:
    print(f["key"], f["checksum"].split(":")[1], f["links"]["self"])' |
    while read -r name md5 url; do
        case "$name" in *.zip) ;; *) continue ;; esac
        fetch "$url" "$DIR/.$name"
        echo "$md5  $DIR/.$name" | md5sum -c --quiet -
        mkdir -p "$DIR/files/${name%.zip}"
        unzip -q -o "$DIR/.$name" -d "$DIR/files/${name%.zip}"
        rm -f "$DIR/.$name"
    done
    touch "$DIR/.done"
fi
write_manifest "$DIR"
