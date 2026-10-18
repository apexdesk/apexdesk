#!/bin/sh
# Builds the Windows (x86-64) program from Linux and packs it in a zip:
#
#   packaging/windows/build.sh     -> dist/apexdesk-<version>-windows-x86_64.zip
#
# Needs, once:
#   rustup target add x86_64-pc-windows-msvc
#   cargo install cargo-xwin --locked
#   clang / lld (lld-link) and llvm-rc for the icon (apt install clang lld llvm)
# On its first run cargo-xwin downloads the Microsoft CRT and Windows SDK
# (about 1 GB in ~/.cache/cargo-xwin); XWIN_ACCEPT_LICENSE=1 accepts their
# license (https://go.microsoft.com/fwlink/?LinkId=2086102).
#
# On Windows itself: `cargo build --release -p apex-app` gives
# target\release\apexdesk.exe (with Visual Studio's build tools installed).
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
target=x86_64-pc-windows-msvc

cd "$root"
# Distributions install the LLVM tools with a version suffix (llvm-lib-21);
# the plain names are in /usr/lib/llvm-<n>/bin. The newest one is used.
if ! command -v llvm-lib >/dev/null 2>&1; then
    llvm=$(ls -d /usr/lib/llvm-*/bin 2>/dev/null | sort -V | tail -1)
    [ -n "$llvm" ] && PATH=$llvm:$PATH && export PATH
fi
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
XWIN_ACCEPT_LICENSE=${XWIN_ACCEPT_LICENSE:-1} cargo xwin build --release -p apex-app --target $target

name=apexdesk-$version-windows-x86_64
stage=$root/target/$target/$name
rm -rf "$stage"
mkdir -p "$stage" "$root/dist"
cp "$root/target/$target/release/apexdesk.exe" "$stage/ApexDesk.exe"
cp "$here/README.txt" "$stage/"
cp "$root/THIRD-PARTY-NOTICES.md" "$root/LICENSE" "$root/EULA.md" "$root/EULA.es.md" "$stage/"
rm -f "$root/dist/$name.zip"
(cd "$stage/.." && zip -qr9 "$root/dist/$name.zip" "$name")
echo "dist/$name.zip"

# The portable copy: the same program with a "portable" marker, so its
# settings and data stay in a "data" folder next to it (a USB stick).
pname=$name-portable
rm -rf "$stage/../$pname" "$root/dist/$pname.zip"
cp -r "$stage" "$stage/../$pname"
printf 'ApexDesk keeps its settings and data in the "data" folder here.\r\n' > "$stage/../$pname/portable"
(cd "$stage/.." && zip -qr9 "$root/dist/$pname.zip" "$pname")
echo "dist/$pname.zip"
