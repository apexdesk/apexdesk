#!/bin/sh
# Fetches the pdfium library, a development tool that ApexDesk's own PDF
# renderer is compared against (no installer ships it), from the builds
# at github.com/bblanchon/pdfium-binaries (pinned version, checked against
# its SHA-256), and leaves the library and its licenses in a folder:
#
#   tools/pdfium-fetch.sh                    this computer's, in target/pdfium
#                                            (where builds run from target/
#                                            find it)
#   tools/pdfium-fetch.sh PLATFORM DEST      PLATFORM's in DEST
#
# PLATFORM is linux-x64, linux-arm64, win-x64 or mac-univ. DEST gets the
# library in lib/ (libpdfium.so, pdfium.dll or libpdfium.dylib), LICENSE,
# VERSION and licenses/ (pdfium and the libraries built into it: all permissive).
# Downloads are cached in target/pdfium-cache.
set -eu

version=8076
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)

if [ $# -eq 0 ]; then
    case "$(uname -s)-$(uname -m)" in
        Linux-x86_64) platform=linux-x64 ;;
        Linux-aarch64 | Linux-arm64) platform=linux-arm64 ;;
        Darwin-*) platform=mac-univ ;;
        *) echo "No pdfium build for $(uname -s) $(uname -m)." >&2; exit 1 ;;
    esac
    dest=$root/target/pdfium
else
    platform=$1
    dest=$2
fi

case $platform in
    linux-x64) sum=d9d67bc40af03aef4fe28a60b19b1086f28ace019c8c9caf19cb7fe3d14ceca3 lib=lib/libpdfium.so ;;
    linux-arm64) sum=d7247b33ae5545615a5e877235dd97afc879e3a8805689684f528cae3339d352 lib=lib/libpdfium.so ;;
    win-x64) sum=808d36da9bc5a3104315fb307c80998121f565ee53953633bf33e80d7429e5ac lib=bin/pdfium.dll ;;
    mac-univ) sum=3bdb93e229298dfdf083dc8ccc7d1a8cf87790b6917e5073335504fe2ff0bdc1 lib=lib/libpdfium.dylib ;;
    *) echo "Unknown platform: $platform (linux-x64, linux-arm64, win-x64, mac-univ)." >&2; exit 1 ;;
esac

cache=$root/target/pdfium-cache
archive=$cache/pdfium-$version-$platform.tgz
mkdir -p "$cache"
check() { [ "$( (sha256sum "$1" 2>/dev/null || shasum -a 256 "$1") | cut -c1-64)" = "$sum" ]; }
if ! [ -f "$archive" ] || ! check "$archive"; then
    curl -fsSL -o "$archive.part" "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/$version/pdfium-$platform.tgz"
    mv "$archive.part" "$archive"
    check "$archive" || { rm -f "$archive"; echo "pdfium-$platform.tgz: wrong SHA-256." >&2; exit 1; }
fi

unpack=$(mktemp -d "${TMPDIR:-/tmp}/apexdesk-pdfium.XXXXXX")
trap 'rm -rf "$unpack"' EXIT
tar xzf "$archive" -C "$unpack"
rm -rf "$dest"
mkdir -p "$dest/lib"
# lib/ beside a build's target/debug (see pdf_view/engine.rs); the
# installers take the folder's contents as components/pdfium.
cp "$unpack/$lib" "$dest/lib/"
cp -R "$unpack/licenses" "$dest/"
cp "$unpack/LICENSE" "$unpack/VERSION" "$dest/"
echo "$dest"
