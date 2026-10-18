# Helpers for the sources of tools/corpus.sh (sourced by tools/corpus/*.sh
# through $CORPUS_LIB).

# Extensions of the files kept from source repositories.
CORPUS_EXTS="doc docx docm dot dotx dotm xls xlsx xlsm xlsb xlt xltx xltm odt ods ott ots fodt fods rtf csv"
# Presentations (tools/corpus/slides.sh).
SLIDES_EXTS="pptx pptm potx potm ppsx ppsm ppt pps pot odp otp fodp"

# sparse_clone URL DEST DIR... : a shallow clone of URL into DEST that
# checks out only the files with CORPUS_EXTS under each DIR ("" is the
# whole repository); blobs are fetched only for those files. Does nothing
# when DEST is already a clone (CORPUS_UPDATE=1 pulls the latest).
sparse_clone() {
    local url="$1" dest="$2"
    shift 2
    if [ -f "$dest/.git/corpus-done" ]; then
        if [ "${CORPUS_UPDATE:-0}" = 1 ]; then
            git -C "$dest" pull -q --depth 1 --ff-only || return 1
        else
            echo "already downloaded: $dest"
        fi
        return 0
    fi
    rm -rf "$dest"
    git clone -q --depth 1 --filter=blob:none --no-checkout "$url" "$dest" || return 1
    git -C "$dest" sparse-checkout init --no-cone
    local patterns=() dir ext up
    for dir in "$@"; do
        for ext in $CORPUS_EXTS; do
            up="$(printf '%s' "$ext" | tr '[:lower:]' '[:upper:]')"
            patterns+=("/${dir:+$dir/}**/*.$ext" "/${dir:+$dir/}**/*.$up")
        done
    done
    printf '%s\n' "${patterns[@]}" > "$dest/.git/info/sparse-checkout"
    git -C "$dest" checkout -q || return 1
    touch "$dest/.git/corpus-done"
    echo "$(find "$dest" -type f -not -path '*/.git/*' | wc -l) files in $dest"
}
