#!/bin/bash
# Security checks before a release (see docs/security.md):
#
#   scripts/security-check.sh              dependencies: known vulnerabilities
#                                          (cargo audit), licenses, sources and
#                                          yanked crates (cargo deny)
#   scripts/security-check.sh --fuzz 2h    the same, then fuzzing of the file
#                                          readers for that long
#
# It fixes nothing: it exits non-zero when something needs a look. The
# tools are installed once with
#   cargo install --locked cargo-audit cargo-deny
set -u
cd "$(dirname "$0")/.." || exit 2

fuzz=""
while [ $# -gt 0 ]; do
    case "$1" in
        --fuzz) fuzz=${2:-}; shift 2 ;;
        *) echo "usage: scripts/security-check.sh [--fuzz DURATION]" >&2; exit 2 ;;
    esac
done

failed=0
step() { printf '\n== %s\n' "$1"; }

step "Known vulnerabilities (cargo audit)"
if cargo audit --version >/dev/null 2>&1; then
    # --deny warnings: unmaintained and unsound crates count too.
    cargo audit --deny warnings || failed=1
else
    echo "cargo-audit is missing: cargo install --locked cargo-audit"
    failed=1
fi

step "Licenses, sources, bans and advisories (cargo deny)"
if cargo deny --version >/dev/null 2>&1; then
    cargo deny --all-features check || failed=1
else
    echo "cargo-deny is missing: cargo install --locked cargo-deny"
    failed=1
fi

if [ -n "$fuzz" ]; then
    step "Fuzzing the readers for $fuzz"
    out="$PWD/target/security/fuzz-$(date +%Y%m%d-%H%M%S)"
    # Symbols and unwinding, so a failure has a trace and the fuzzer
    # survives it (see crates/apex-bench/src/fuzz.rs).
    CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_STRIP=false CARGO_PROFILE_RELEASE_DEBUG=line-tables-only \
        CARGO_PROFILE_RELEASE_PANIC=unwind CARGO_PROFILE_RELEASE_LTO=thin \
        nice -n 10 cargo build -q --release -j 4 -p apex-bench || exit 1
    nice -n 10 target/release/apex-bench fuzz --time "$fuzz" --jobs 2 --out "$out" || failed=1
    # One trace per distinct failure, next to its reduced file.
    crashes=$(find "$out/crashes" -name '*.trace.txt' 2>/dev/null | wc -l)
    echo "report: $out/report.md"
    if [ "$crashes" -gt 0 ]; then
        echo "$crashes distinct failure(s) in $out/crashes: each is a bug to look at before the release"
        failed=1
    fi
fi

echo
if [ $failed -eq 0 ]; then echo "security checks passed"; else echo "security checks FAILED"; fi
exit $failed
