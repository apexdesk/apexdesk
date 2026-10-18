#!/bin/bash
# Timing and memory figures for the website (website/js/figures.js).
#
#   tools/website-figures/measure.sh OUT_DIR APEXDESK_BINARY [RUNS]
#
# Needs a private X server on DISPLAY (default :99, e.g. `Xvfb :99 -screen 0
# 1600x900x24`), xwininfo, python3 with openpyxl and, for the comparison,
# LibreOffice (`soffice`) with a throwaway profile inside OUT_DIR.
# For each tool and file: one warm-up start, then RUNS starts (default 7),
# alternating tools. Each line of OUT_DIR/results.txt is
#   tool file run ms_to_title ms_to_first_frame rss_mb
# ms_to_title: launch -> a top-level window whose title has the file name
#   (with no file: any window of the program). Same method for both tools.
# ms_to_first_frame: ApexDesk only, launch -> its first screenshot (APEXDESK_SCRIPT).
# rss_mb: resident memory of the whole process tree, 4 s after the window.
set -u
OUT=$(realpath "$1"); APEX=$(realpath "$2"); RUNS=${3:-7}
HERE=$(dirname "$(realpath "$0")")
export DISPLAY=${DISPLAY_FIGURES:-:99} GDK_BACKEND=x11 LANG=${LANG_FIGURES:-es_ES.UTF-8}
unset WAYLAND_DISPLAY
mkdir -p "$OUT/files"
[ -f "$OUT/files/big.xlsx" ] || python3 "$HERE/mkfiles.py" "$OUT/files"

tree_rss() { # VmRSS (kB) of $1 and its descendants
    local p=$1 s c
    s=$(awk '/VmRSS/{print $2}' /proc/$p/status 2>/dev/null)
    for c in $(cat /proc/$p/task/*/children 2>/dev/null); do s=$(( ${s:-0} + $(tree_rss $c) )); done
    echo ${s:-0}
}

one() { # one TOOL FILE RUN
    local tool=$1 file=$2 run=$3 args=() pat t0 now t1="" ts="" pid rss
    rm -rf "$OUT/cfg" "$OUT/shot1.ppm" "$OUT/shot2.ppm" "$OUT"/files/.~lock*
    mkdir -p "$OUT/cfg"
    [ "$file" != "-" ] && args=("$OUT/files/$file")
    if [ "$file" = "-" ]; then pat="ApexDesk|Work Office|LibreOffice"; else pat="$file"; fi
    printf 'shot %s/shot1.ppm\nsleep 4000\nshot %s/shot2.ppm\nquit\n' "$OUT" "$OUT" > "$OUT/script.txt"
    t0=$(date +%s%N)
    if [ $tool = apex ]; then
        APEXDESK_SCRIPT="$OUT/script.txt" APEXDESK_SIZE=1400x860 XDG_CONFIG_HOME="$OUT/cfg" \
            systemd-run --user --scope -q -p MemoryMax=8G -p MemorySwapMax=0 "$APEX" "${args[@]}" >/dev/null 2>&1 &
    else
        systemd-run --user --scope -q -p MemoryMax=8G -p MemorySwapMax=0 \
            soffice -env:UserInstallation=file://$OUT/lo --norestore --nologo "${args[@]}" >/dev/null 2>&1 &
    fi
    pid=$!
    while :; do
        now=$(date +%s%N)
        [ $(( (now - t0) / 1000000 )) -gt 40000 ] && break
        if [ -z "$ts" ] && [ -s "$OUT/shot1.ppm" ]; then ts=$(( (now - t0) / 1000000 )); fi
        if [ -z "$t1" ] && xwininfo -root -tree 2>/dev/null | grep -Eq "\"[^\"]*($pat)[^\"]*\": \(\""; then t1=$(( (now - t0) / 1000000 )); fi
        [ -n "$t1" ] && { [ $tool = lo ] || [ -n "$ts" ]; } && break
        sleep 0.01
    done
    sleep 4
    rss=$(( $(tree_rss $pid) / 1024 ))
    if [ $tool = lo ]; then
        for c in $(cat /proc/$pid/task/*/children 2>/dev/null); do kill $c 2>/dev/null; done
        kill $pid 2>/dev/null
    fi
    for i in $(seq 1 100); do kill -0 $pid 2>/dev/null || break; sleep 0.1; done
    kill -9 $pid 2>/dev/null
    rm -f "$OUT"/files/.~lock*
    echo "$tool $file $run ${t1:-NA} ${ts:-NA} $rss"
}

: > "$OUT/results.txt"
for f in - big.xlsx big.docx; do
    one apex $f w >/dev/null; one lo $f w >/dev/null
    for r in $(seq 1 $RUNS); do one apex $f $r; one lo $f $r; done | tee -a "$OUT/results.txt"
done
python3 "$HERE/median.py" "$OUT/results.txt"
