#!/usr/bin/env bash
# Enron spreadsheets (Hermans and Murphy-Hill, ICSE 2015): 15 929
# spreadsheets from the Enron e-mail archive (2000-2001), converted to
# .xlsx by Excel for the dataset, with the values Excel computed for every
# formula. CC BY, figshare doi:10.6084/m9.figshare.1221767
# (spreadsheets.7z, 1 GB; 1.8 GB unpacked; 7z needed).
source "$(dirname "$0")/lib/common.sh"

DIR="$(corpus_dir enron)"
URL="https://ndownloader.figshare.com/files/3242531"
MD5="188ec98d04c83a4725d9205cc75ce5a2"

if [ ! -f "$DIR/.done" ]; then
    command -v 7z >/dev/null || { echo "enron: 7z is needed (p7zip)" >&2; exit 1; }
    fetch "$URL" "$DIR/.spreadsheets.7z"
    echo "$MD5  $DIR/.spreadsheets.7z" | md5sum -c --quiet -
    7z x -y -bd -o"$DIR/files" "$DIR/.spreadsheets.7z" >/dev/null
    rm -f "$DIR/.spreadsheets.7z"
    touch "$DIR/.done"
fi
write_manifest "$DIR"
