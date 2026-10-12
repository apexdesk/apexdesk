# Shared by the corpus scripts in tools/corpus/ (sourced, not run).
#
# corpus_dir NAME   prints (and creates) target/corpus/NAME of the main
#                   checkout, also when run from a git worktree; CORPUS_DIR
#                   overrides the base directory.
# fetch URL FILE    downloads URL to FILE unless it is already there.
# write_manifest D  writes D/manifest.sha256 (sha256sum format, sorted).

set -euo pipefail

TOOLS_CORPUS="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$(cd "$TOOLS_CORPUS/../.." && pwd)"

corpus_dir() {
    local base="${CORPUS_DIR:-}"
    if [ -z "$base" ]; then
        # The main checkout's target/, shared by every worktree.
        local common
        common="$(git -C "$REPO_ROOT" rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)"
        if [ -n "$common" ]; then
            base="$(dirname "$common")/target/corpus"
        else
            base="$REPO_ROOT/target/corpus"
        fi
    fi
    mkdir -p "$base/$1"
    echo "$base/$1"
}

fetch() {
    local url="$1" out="$2"
    if [ -s "$out" ]; then
        return 0
    fi
    echo "downloading $url" >&2
    curl -fsSL --retry 5 --retry-delay 5 -C - -o "$out.part" "$url"
    mv "$out.part" "$out"
}

write_manifest() {
    local dir="$1"
    (cd "$dir" && find . -type f ! -name 'manifest.sha256' ! -name '*.part' ! -name '.*' -print0 \
        | sort -z | xargs -0 -r sha256sum > manifest.sha256.new && mv manifest.sha256.new manifest.sha256)
    echo "$(wc -l < "$dir/manifest.sha256") files in $dir" >&2
}
