#!/bin/sh
# Builds a single-file Flatpak bundle from the release's Linux portable,
# with plain `flatpak build` (no flatpak-builder, no compile inside the
# sandbox: the binary only needs libc and libasound, both in the runtime).
#
#   packaging/linux/flatpak/bundle.sh apexdesk-X-linux-x86_64-portable.tar.gz OUT_DIR
#
# Needs org.freedesktop.Platform and org.freedesktop.Sdk 25.08 installed.
set -eu
tarball=$(realpath "$1")
out=$(realpath "$2")
here=$(cd "$(dirname "$0")" && pwd)
linux=$here/..
id=io.github.apexdesk.ApexDesk
ver=$(basename "$tarball" | sed -n 's/^apexdesk-\(.*\)-linux-.*/\1/p')
arch=$(basename "$tarball" | sed -n 's/.*-linux-\([a-z0-9_]*\)-portable.*/\1/p')
work=$out/flatpak-work
rm -rf "$work"
mkdir -p "$work/src"
tar xzf "$tarball" -C "$work/src" --strip-components=1

flatpak build-init --arch="$arch" "$work/app" $id org.freedesktop.Sdk org.freedesktop.Platform 25.08
a=$work/app/files
install -Dm755 "$work/src/apexdesk" "$a/bin/apexdesk"
for f in LICENSE NOTICE EULA.md EULA.es.md THIRD-PARTY-NOTICES.md; do
  [ -f "$work/src/$f" ] && install -Dm644 "$work/src/$f" "$a/share/licenses/$id/$f"
done
sed "s/^Icon=apexdesk/Icon=$id/" "$linux/apexdesk.desktop" > "$work/$id.desktop"
install -Dm644 "$work/$id.desktop" "$a/share/applications/$id.desktop"
install -Dm644 "$linux/apexdesk-mime.xml" "$a/share/mime/packages/$id.xml"
install -Dm644 "$linux/apexdesk.svg" "$a/share/icons/hicolor/scalable/apps/$id.svg"
install -Dm644 "$linux/apexdesk-256.png" "$a/share/icons/hicolor/256x256/apps/$id.png"
install -Dm644 "$here/$id.metainfo.xml" "$a/share/metainfo/$id.metainfo.xml"

flatpak build-finish "$work/app" --command=apexdesk \
  --share=ipc --socket=x11 --socket=wayland --device=dri --share=network \
  --filesystem=home --filesystem=xdg-run/pipewire-0:ro --socket=pulseaudio \
  --socket=cups --talk-name=org.freedesktop.portal.Desktop
flatpak build-export --arch="$arch" "$work/repo" "$work/app"
flatpak build-bundle --arch="$arch" --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
  "$work/repo" "$out/apexdesk-$ver-linux-$arch.flatpak" $id
rm -rf "$work"
echo "$out/apexdesk-$ver-linux-$arch.flatpak"
