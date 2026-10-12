#!/usr/bin/env bash
# docx-corpus (superdoc, MIT; https://docxcorp.us): about a million .docx
# files from Common Crawl, named by a SHA-256 (not of the file itself). A deterministic
# sample: every DOCX_STEP-th document of the English manifest (default
# 80, about 3000 files) and every (DOCX_STEP/4)-th of eight other
# languages (about 9000 more: 12 300 files, 1.9 GB in all).
source "$(dirname "$0")/lib/common.sh"

DIR="$(corpus_dir docxcorpus)"
API="https://api.docxcorp.us/manifest"
STEP="${DOCX_STEP:-80}"
mkdir -p "$DIR/files"

for lang in en es de fr ja zh ar he ru; do
    list="$DIR/.manifest-$lang.txt"
    [ -s "$list" ] || curl -fsSL "$API?lang=$lang" -o "$list"
    step="$STEP"
    [ "$lang" = en ] || step=$((STEP / 4 > 0 ? STEP / 4 : 1))
    awk -v s="$step" 'NR % s == 1' "$list" | while read -r url; do
        name="$(basename "$url")"
        out="$DIR/files/$name"
        [ -s "$out" ] && continue
        curl -fsSL --retry 3 -o "$out.part" "$url" || { rm -f "$out.part"; continue; }
        # a ZIP package, not an error page
        if [ "$(head -c 2 "$out.part")" = "PK" ]; then
            mv "$out.part" "$out"
        else
            echo "not a docx: $url" >&2
            rm -f "$out.part"
        fi
    done
done
write_manifest "$DIR"
