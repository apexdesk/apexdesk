#!/bin/bash
# Automatic QA of ApexDesk: runs the deterministic checks (the ignored
# `qa_` tests of apex-app) over the whole window, without a screen, and
# joins their reports. It fixes nothing: what it finds is the report.
#
#   scripts/qa.sh          quick run (the default), about 12 minutes
#   scripts/qa.sh full     everything, much longer
#   scripts/qa.sh --accept-screens
#                          this run's screenshots become the baseline
#                          (target/qa/baseline/) the next runs compare with
#
# APEXDESK_QA_MODE=full does the same as the argument. The report goes to
# target/qa/<date-time>/report.md; see docs/qa.md.
set -u
cd "$(dirname "$0")/.." || exit 2

mode=${APEXDESK_QA_MODE:-quick}
accept=0
for arg in "$@"; do
    case "$arg" in
        quick|full) mode=$arg ;;
        --accept-screens) accept=1 ;;
        *) echo "usage: scripts/qa.sh [quick|full] [--accept-screens]" >&2; exit 2 ;;
    esac
done

out="$PWD/target/qa/$(date +%Y%m%d-%H%M%S)"
mkdir -p "$out"
log="$out/cargo.log"
start=$(date +%s)

# Capped like every heavy job on this machine: memory, no swap storm,
# six compile jobs, no incremental cache (it fills the disk).
APEXDESK_QA_MODE=$mode APEXDESK_QA_OUT=$out APEXDESK_QA_ACCEPT_SCREENS=$accept CARGO_INCREMENTAL=0 \
    systemd-run --user --scope -q -p MemoryMax=12G -p MemorySwapMax=1G \
    cargo test -q -j 6 -p apex-app --no-fail-fast -- --ignored qa_ >"$log" 2>&1
status=$?

# The sections' reports, one after another.
{
    echo "# ApexDesk QA ($mode, $(date '+%Y-%m-%d %H:%M'))"
    echo
    cat "$out"/*.sum 2>/dev/null
    for md in "$out"/*.md; do
        [ "$(basename "$md")" = report.md ] && continue
        [ -e "$md" ] || continue
        echo
        sed 's/^## /### /; t; s/^# /## /' "$md"
    done
} >"$out/report.md.tmp"
mv "$out/report.md.tmp" "$out/report.md"

cat "$out"/*.sum 2>/dev/null
new=$(for md in "$out"/*.md; do [ "$(basename "$md")" = report.md ] || grep -h '^- ' "$md"; done 2>/dev/null | head -15)
if [ -n "$new" ]; then
    echo
    echo "First new failures:"
    echo "$new" | cut -c1-300
fi
echo
echo "Report: $out/report.md ($(( $(date +%s) - start )) s)"

# A section that finds failures doesn't fail; the QA itself breaking does.
if [ $status -ne 0 ]; then
    echo "The QA run itself failed (exit $status): see $log" >&2
    grep -E "panicked|^error" "$log" | head -5 >&2
    exit 1
fi
