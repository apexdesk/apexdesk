#!/usr/bin/env bash
# Downloads the corpora of real-world files for apex-bench (see
# docs/plan-bloque5.md) into target/corpus/<source>/, then writes
# target/corpus/manifest.tsv: one line per distinct file (by SHA-256) with
# its path, source, size and hash; the copies left out go to
# duplicates.tsv. `apex-bench run target/corpus` reads the manifest.
#
# Every tools/corpus/*.sh is one source: it is run with CORPUS_DIR (the
# corpus root), SOURCE_DIR (its own directory, CORPUS_DIR/<script name>)
# and CORPUS_LIB (helpers it may source). A source already downloaded is
# not downloaded again (each script checks), so running this again is
# cheap.
#
# Usage: tools/corpus.sh [SOURCE...]    (default: every source)
#   CORPUS_DIR=/path tools/corpus.sh     another root (default: target/corpus
#                                        of the main checkout)
#   CORPUS_UPDATE=1 tools/corpus.sh      pull the latest of git sources
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
if [ -z "${CORPUS_DIR:-}" ]; then
    # The main checkout's target/, shared by every worktree.
    common="$(git -C "$here" rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)"
    if [ -n "$common" ]; then
        CORPUS_DIR="$(dirname "$common")/target/corpus"
    else
        CORPUS_DIR="$here/../target/corpus"
    fi
fi
mkdir -p "$CORPUS_DIR"
CORPUS_DIR="$(cd "$CORPUS_DIR" && pwd)"
export CORPUS_DIR
export CORPUS_LIB="$here/corpus-lib.sh"

failed=()
for script in "$here"/corpus/*.sh; do
    [ -e "$script" ] || continue
    name="$(basename "$script" .sh)"
    if [ $# -gt 0 ] && ! printf '%s\n' "$@" | grep -qx "$name"; then
        continue
    fi
    echo "== $name"
    export SOURCE_DIR="$CORPUS_DIR/$name"
    mkdir -p "$SOURCE_DIR"
    if ! bash "$script"; then
        echo "corpus.sh: source $name failed" >&2
        failed+=("$name")
    fi
done

# Manifest of the distinct files of the formats ApexDesk reads (first
# path in sorted order wins).
echo "== manifest"
exts='doc|docx|docm|dot|dotx|dotm|xls|xlsx|xlsm|xlsb|xlt|xltx|xltm|odt|ods|ott|ots|fodt|fods|rtf|csv|tsv|pptx|pptm|potx|potm|ppsx|ppsm|ppt|pps|pot|odp|otp|fodp'
sums="$CORPUS_DIR/.sums.tmp"
(cd "$CORPUS_DIR" && find . -mindepth 2 -type f -not -path '*/.git/*' -regextype posix-extended -iregex ".*\.($exts)" -print0 \
    | sort -z | xargs -0 -r -n 200 sha256sum) | sed 's#^\([0-9a-f]*\)  \./#\1\t#' > "$sums"
declare -A seen=()
{
    printf 'path\tsource\tsize\tsha256\n'
    while IFS=$'\t' read -r sum path; do
        [ -z "${seen[$sum]:-}" ] || continue
        seen[$sum]="$path"
        size=$(stat -c %s "$CORPUS_DIR/$path" 2>/dev/null || stat -f %z "$CORPUS_DIR/$path")
        printf '%s\t%s\t%s\t%s\n' "$path" "${path%%/*}" "$size" "$sum"
    done < "$sums"
} > "$CORPUS_DIR/manifest.tsv.tmp"
{
    printf 'path\tsame_as\n'
    while IFS=$'\t' read -r sum path; do
        [ "${seen[$sum]}" = "$path" ] || printf '%s\t%s\n' "$path" "${seen[$sum]}"
    done < "$sums"
} > "$CORPUS_DIR/duplicates.tsv"
mv "$CORPUS_DIR/manifest.tsv.tmp" "$CORPUS_DIR/manifest.tsv"
rm -f "$sums"
echo "$(($(wc -l < "$CORPUS_DIR/manifest.tsv") - 1)) distinct files, $(($(wc -l < "$CORPUS_DIR/duplicates.tsv") - 1)) duplicates: $CORPUS_DIR/manifest.tsv"

if [ ${#failed[@]} -gt 0 ]; then
    echo "corpus.sh: failed sources: ${failed[*]}" >&2
    exit 1
fi
