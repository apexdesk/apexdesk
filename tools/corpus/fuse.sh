#!/usr/bin/env bash
# FUSE (Barik et al., MSR 2015): 249 376 spreadsheets from Common Crawl.
# static.barik.net/fuse/ is gone; the tera-PROMISE copy is on Zenodo,
# record 581678 (fuse.zip, 9.4 GB, CC BY 4.0): a split 7z archive that
# stores the corpus as one .tar.gz. The archive is read as a stream (9.4
# GB of download, nothing of it stored) and every FUSE_STEP-th file is
# kept (default 60, about 4000 .xls/.xlsx files).
source "$(dirname "$0")/lib/common.sh"

DIR="$(corpus_dir fuse)"
URL="https://zenodo.org/api/records/581678/files/fuse.zip/content"
STEP="${FUSE_STEP:-60}"

if [ ! -f "$DIR/.done-$STEP" ]; then
    python3 "$TOOLS_CORPUS/lib/fuse_stream.py" "$URL" "$DIR/files" --step "$STEP"
    touch "$DIR/.done-$STEP"
fi
write_manifest "$DIR"
