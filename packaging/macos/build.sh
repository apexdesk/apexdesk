#!/bin/sh
# Builds ApexDesk.app for macOS (Apple silicon and Intel in one program) and
# packs it in a disk image. Runs on a Mac:
#
#   packaging/macos/build.sh     -> dist/apexdesk-<version>-macos.dmg
#
# Needs, once: the Xcode command line tools (xcode-select --install) and
#   rustup target add aarch64-apple-darwin x86_64-apple-darwin
# APEXDESK_ARCHS="aarch64" builds for one architecture only.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
archs=${APEXDESK_ARCHS:-"aarch64 x86_64"}

cd "$root"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
bins=""
for a in $archs; do
    cargo build --release -p apex-app --target "$a-apple-darwin"
    bins="$bins target/$a-apple-darwin/release/apexdesk"
done

stage=$root/target/macos
app=$stage/ApexDesk.app
rm -rf "$stage"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$root/dist"
# shellcheck disable=SC2086
lipo -create $bins -output "$app/Contents/MacOS/apexdesk"
sed "s/@VERSION@/$version/g" "$here/Info.plist" > "$app/Contents/Info.plist"

# The icon, from the 256-pixel PNG.
set_dir=$stage/apexdesk.iconset
mkdir -p "$set_dir"
for s in 16 32 64 128 256; do
    sips -z $s $s "$root/packaging/linux/apexdesk-256.png" --out "$set_dir/icon_${s}x${s}.png" >/dev/null
done
for s in 16 32 64 128; do
    cp "$set_dir/icon_$((s * 2))x$((s * 2)).png" "$set_dir/icon_${s}x${s}@2x.png" 2>/dev/null || true
done
iconutil -c icns "$set_dir" -o "$app/Contents/Resources/apexdesk.icns"

# Without a Developer ID the program is signed ad hoc: Gatekeeper asks the
# first time (right click → Open).
codesign --force --deep --sign "${APEXDESK_SIGN_ID:--}" "$app"

name=apexdesk-$version-macos
rm -f "$root/dist/$name.dmg"
# hdiutil fails now and then with "Resource busy" (Spotlight or XProtect
# looking at the new app); a few tries get past it.
for try in 1 2 3 4 5; do
    hdiutil create -volname ApexDesk -srcfolder "$app" -ov -format UDZO "$root/dist/$name.dmg" && break
    [ "$try" = 5 ] && exit 1
    sleep 10
done
echo "dist/$name.dmg"
