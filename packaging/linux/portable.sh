#!/bin/sh
# Builds the portable Linux copy, a folder to unpack anywhere and run:
#
#   packaging/linux/portable.sh   -> dist/apexdesk-<version>-linux-<arch>-portable.tar.gz
#
# The program is linked against glibc 2.31 (Ubuntu 20.04, Debian 11, Fedora 32
# and newer) with cargo-zigbuild; the deb and rpm take the same binary through
# APEXDESK_BIN. Needs, once: zig 0.14 on the PATH and
# `cargo install --locked cargo-zigbuild`.
#
# APEXDESK_BIN uses an already built program instead of building one;
# APEXDESK_ARCH (x86_64 or aarch64) builds for another architecture than
# this computer's.
set -eu
umask 022

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)
arch=${APEXDESK_ARCH:-$(uname -m)}
bin=${APEXDESK_BIN:-}
if [ -z "$bin" ]; then
    (cd "$root" && cargo zigbuild --release -p apex-app --target "$arch-unknown-linux-gnu.2.31")
    bin=$root/target/$arch-unknown-linux-gnu/release/apexdesk
fi

name=apexdesk-$version-linux-$arch-portable
stage=$root/target/$name
rm -rf "$stage" "$root/dist/$name.tar.gz"
mkdir -p "$stage" "$root/dist"
install -m755 "$bin" "$stage/apexdesk"
install -m644 "$here/apexdesk-256.png" "$stage/apexdesk.png"
install -m644 "$here/README-portable.txt" "$stage/README.txt"
install -m644 "$root/THIRD-PARTY-NOTICES.md" "$root/LICENSE" "$root/EULA.md" "$root/EULA.es.md" "$stage/"
printf 'ApexDesk keeps its settings and data in the "data" folder here.\n' > "$stage/portable"
tar -C "$root/target" --owner=0 --group=0 --numeric-owner -czf "$root/dist/$name.tar.gz" "$name"
rm -rf "$stage"
echo "dist/$name.tar.gz"
