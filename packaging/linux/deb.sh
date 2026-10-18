#!/bin/sh
# Builds the Debian/Ubuntu packages:
#
#   packaging/linux/deb.sh   -> dist/apexdesk_<version>_<arch>.deb
#
# Optional components:
#   media         the distribution's GStreamer, for videos and sounds
#                 (Recommends: its base and good plugins; left out with
#                 apt install --no-install-recommends)
#   dictionaries  the distribution's hunspell-* packages (Suggests), which
#                 ApexDesk finds in /usr/share/hunspell
#
# APEXDESK_BIN uses an already built program instead of building one;
# DIST changes the output folder. Silent install: apt install -y ./apexdesk_*.deb
set -eu
umask 022

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
dist=${DIST:-$root/dist}
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)
arch=$(dpkg --print-architecture 2>/dev/null || echo amd64)
bin=${APEXDESK_BIN:-}
if [ -z "$bin" ]; then
    (cd "$root" && cargo build --release -p apex-app)
    bin=$root/target/release/apexdesk
fi

stage=$(mktemp -d "${TMPDIR:-/tmp}/apexdesk-deb.XXXXXX")
trap 'rm -rf "$stage"' EXIT
pkg=$stage/apexdesk
# The program lives in /usr/lib/apexdesk (next to its optional components,
# as on the other systems); /usr/bin/apexdesk links to it.
install -Dm755 "$bin" "$pkg/usr/lib/apexdesk/apexdesk"
mkdir -p "$pkg/usr/bin"
ln -s ../lib/apexdesk/apexdesk "$pkg/usr/bin/apexdesk"
install -Dm644 "$here/apexdesk.desktop" "$pkg/usr/share/applications/apexdesk.desktop"
install -Dm644 "$here/apexdesk-mime.xml" "$pkg/usr/share/mime/packages/apexdesk.xml"
install -Dm644 "$here/apexdesk.svg" "$pkg/usr/share/icons/hicolor/scalable/apps/apexdesk.svg"
install -Dm644 "$here/apexdesk-256.png" "$pkg/usr/share/icons/hicolor/256x256/apps/apexdesk.png"
install -Dm644 "$here/policy.example" "$pkg/usr/share/doc/apexdesk/policy.example"
for f in LICENSE EULA.md EULA.es.md THIRD-PARTY-NOTICES.md; do
    install -Dm644 "$root/$f" "$pkg/usr/share/doc/apexdesk/$f"
done
size=$(du -sk "$pkg" | cut -f1)
mkdir -p "$pkg/DEBIAN"
cat > "$pkg/DEBIAN/control" <<EOF
Package: apexdesk
Version: $version
Architecture: $arch
Maintainer: ApexDesk <apexdesk@users.noreply.gitlab.com>
Installed-Size: $size
Section: editors
Priority: optional
Depends: libc6, libgcc-s1, libasound2t64 | libasound2, libx11-6, libxkbcommon0, libgl1 | libvulkan1, curl
Recommends: gstreamer1.0-plugins-base, gstreamer1.0-plugins-good, xdg-utils
Suggests: hunspell-en-us | hunspell-dictionary
Replaces: apexdesk-pdfium
Breaks: apexdesk-pdfium
Homepage: https://github.com/apexdesk/apexdesk
Description: Lightweight spreadsheet and word processor
 ApexDesk opens and saves xlsx, docx, ods, odt, csv and Markdown files,
 with formulas, charts, mail merge, digital signatures and PDF export.
EOF
cat > "$pkg/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
update-desktop-database -q /usr/share/applications 2>/dev/null || true
update-mime-database /usr/share/mime 2>/dev/null || true
gtk-update-icon-cache -q -t /usr/share/icons/hicolor 2>/dev/null || true
EOF
cp "$pkg/DEBIAN/postinst" "$pkg/DEBIAN/postrm"
chmod 755 "$pkg/DEBIAN/postinst" "$pkg/DEBIAN/postrm"
mkdir -p "$dist"
out=$dist/apexdesk_${version}_$arch.deb
dpkg-deb --root-owner-group -Zxz --build "$pkg" "$out" >/dev/null
echo "$out"
