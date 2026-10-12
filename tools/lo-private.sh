#!/bin/sh
# A private LibreOffice with Impress and Draw under target/tools/lo, for
# the presentation judge when the system's LibreOffice lacks them
# (Debian and Ubuntu only). The system's own LibreOffice is copied and
# the missing packages, of the same version, are downloaded with
# apt-get download and unpacked over it: nothing is installed in the
# system or the home folder.
#
# Usage: tools/lo-private.sh      then run target/tools/lo/program/soffice
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
dest="$root/target/tools/lo"
debs="$root/target/tools/lo-debs"
if [ -x "$dest/program/soffice" ] && [ -e "$dest/program/libsdlo.so" ]; then
    echo "$dest/program/soffice"
    exit 0
fi
command -v apt-get >/dev/null || { echo "needs apt-get (Debian, Ubuntu)" >&2; exit 1; }
[ -d /usr/lib/libreoffice/program ] || { echo "needs the system's LibreOffice" >&2; exit 1; }
mkdir -p "$debs"
cd "$debs"
for p in libreoffice-impress libreoffice-draw libreoffice-uiconfig-impress libreoffice-uiconfig-draw; do
    ls "$p"_*.deb >/dev/null 2>&1 || apt-get download "$p"
done
rm -rf "$dest" "$debs/root"
mkdir -p "$debs/root"
for d in "$debs"/*.deb; do
    dpkg-deb -x "$d" "$debs/root"
done
cp -a /usr/lib/libreoffice "$dest"
# The packages' files, over the copy (links into /etc and /usr/share are
# resolved by the copy of the system's own).
cp -a "$debs/root/usr/lib/libreoffice/." "$dest/"
rm -rf "$debs/root"
# The copy must read its own configuration (with impress.xcd), not the
# system's: its base folder is hard-coded in fundamentalrc.
sed -i 's|^BRAND_BASE_DIR=file:///usr/lib/libreoffice$|BRAND_BASE_DIR=${ORIGIN}/..|' "$dest/program/fundamentalrc"
echo "$dest/program/soffice"
