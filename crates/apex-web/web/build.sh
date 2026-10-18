#!/bin/sh
# Builds ApexDesk for the browser into target/web (or $1): the program
# (apexdesk_bg.wasm with wasm-bindgen's apexdesk.js), the page and the
# fonts it draws documents with (the browser lends the page none of the
# system's). Serve the folder with any web server, or with
# `apexdesk serve`, which adds the users' files on the server.
#
# Needs the wasm32-unknown-unknown target and wasm-bindgen-cli of the
# version in Cargo.lock:
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version <version in Cargo.lock>
set -eu
root=$(cd "$(dirname "$0")/../../.." && pwd)
out=${1:-$root/target/web}
profile=${PROFILE:-release}
cd "$root"
cargo build -p apex-web --target wasm32-unknown-unknown --profile "$profile"
dir=$profile
[ "$profile" = dev ] && dir=debug
mkdir -p "$out/fonts"
wasm-bindgen --target web --no-typescript --out-dir "$out" "target/wasm32-unknown-unknown/$dir/apexdesk.wasm"
cp crates/apex-web/web/index.html crates/apex-web/web/start.js "$out/"
# The icon, from the 64x64 RGBA one the window uses.
python3 -I - crates/apex-app/assets/icon64.rgba "$out/icon.png" <<'PY'
import struct, sys, zlib
raw = open(sys.argv[1], 'rb').read()
rows = b''.join(b'\0' + raw[y * 256:(y + 1) * 256] for y in range(64))
chunk = lambda t, d: struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d))
png = b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', 64, 64, 8, 6, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(rows, 9)) + chunk(b'IEND', b'')
open(sys.argv[2], 'wb').write(png)
PY
# Free fonts with the metrics of the usual ones (Arial, Times New Roman,
# Courier New, Calibri, Cambria) and the interface's, from this system.
: > "$out/fonts/index.txt"
for f in LiberationSans-Regular LiberationSans-Bold LiberationSans-Italic LiberationSans-BoldItalic \
         LiberationSerif-Regular LiberationSerif-Bold LiberationSerif-Italic LiberationSerif-BoldItalic \
         LiberationMono-Regular LiberationMono-Bold LiberationMono-Italic LiberationMono-BoldItalic \
         Carlito-Regular Carlito-Bold Carlito-Italic Carlito-BoldItalic Caladea-Regular Caladea-Bold \
         NotoSans-Regular NotoSans-Bold DejaVuSans DejaVuSans-Bold; do
    src=$(find /usr/share/fonts /usr/local/share/fonts "${HOME:-/nonexistent}/.local/share/fonts" -name "$f.ttf" 2>/dev/null | head -n 1)
    if [ -n "$src" ]; then
        cp "$src" "$out/fonts/"
        echo "$f.ttf" >> "$out/fonts/index.txt"
    else
        echo "web/build.sh: $f.ttf not found, left out" >&2
    fi
done
echo "ApexDesk for the browser: $out"
