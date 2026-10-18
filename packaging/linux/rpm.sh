#!/bin/sh
# Builds the Fedora/RHEL/openSUSE package with rpmbuild:
#
#   packaging/linux/rpm.sh   -> dist/apexdesk-<version>-1.<arch>.rpm
#
# Optional components: GStreamer for videos and sounds (Recommends: its
# base and good plugins; skipped with dnf --setopt=install_weak_deps=False), dictionaries (Suggests:
# hunspell-*). APEXDESK_BIN uses an already built program; DIST changes the
# output folder. Silent install: dnf install -y ./apexdesk-*.rpm
set -eu
umask 022

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
dist=${DIST:-$root/dist}
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)
bin=${APEXDESK_BIN:-}
if [ -z "$bin" ]; then
    (cd "$root" && cargo build --release -p apex-app)
    bin=$root/target/release/apexdesk
fi

top=$(mktemp -d "${TMPDIR:-/tmp}/apexdesk-rpm.XXXXXX")
trap 'rm -rf "$top"' EXIT
src=$top/SOURCES
mkdir -p "$src"
cp "$bin" "$src/apexdesk"
cp "$here/apexdesk.desktop" "$here/apexdesk-mime.xml" "$here/apexdesk.svg" "$here/apexdesk-256.png" "$here/policy.example" "$src/"
cp "$root/LICENSE" "$root/EULA.md" "$root/EULA.es.md" "$root/THIRD-PARTY-NOTICES.md" "$src/"
rpmbuild -bb --quiet \
    --define "_topdir $top" \
    --define "_rpmdir $top/RPMS" \
    --define "apex_version $version" \
    --define "apex_src $src" \
    "$here/apexdesk.spec"
mkdir -p "$dist"
find "$top/RPMS" -name '*.rpm' -exec cp {} "$dist/" \; -exec sh -c 'echo "$1/$(basename "$2")"' _ "$dist" {} \;
