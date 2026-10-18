#!/bin/sh
# Builds a signed, notarized installer package for macOS. UNTESTED (written
# on Linux; pkgbuild, productbuild and notarytool exist only on macOS).
#
#   packaging/macos/build.sh    first: target/macos/ApexDesk.app
#   packaging/macos/pkg.sh      -> dist/ApexDesk-<version>.pkg
#
# The installer puts ApexDesk.app (see build.sh) in
# /Applications and offers the optional dictionaries (off by default) when
# APEXDESK_DICT_DIR (.aff/.dic files) is set; they go to
# /Library/Application Support/ApexDesk/dictionaries.
#
# Signing: APEXDESK_SIGN_ID ("Developer ID Application: ...") signs the
# program with the hardened runtime, APEXDESK_INSTALLER_ID ("Developer ID
# Installer: ...") the package, and APEXDESK_NOTARY_PROFILE (a profile
# saved with `xcrun notarytool store-credentials`) notarizes and staples it.
# Silent install: sudo installer -pkg ApexDesk-<version>.pkg -target /
# Policies: deploy packaging/macos/ApexDesk-policy.mobileconfig with the MDM.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)
app=$root/target/macos/ApexDesk.app
[ -d "$app" ] || { echo "Run packaging/macos/build.sh first." >&2; exit 1; }
work=$root/target/macos/pkg
rm -rf "$work"
mkdir -p "$work/app/Applications" "$work/pkgs" "$root/dist"

cp -R "$app" "$work/app/Applications/"
if [ -n "${APEXDESK_SIGN_ID:-}" ]; then
    codesign --force --deep --options runtime --timestamp --sign "$APEXDESK_SIGN_ID" "$work/app/Applications/ApexDesk.app"
fi
pkgbuild --root "$work/app" --identifier com.github.apexdesk.apexdesk --version "$version" \
    --install-location / "$work/pkgs/apexdesk.pkg"

choices=""
lines=""
support="/Library/Application Support/ApexDesk"
component() {
    id=$1 title=$2 src=$3 dest=$4
    [ -n "$src" ] || return 0
    mkdir -p "$work/$id$support/$dest"
    cp -R "$src"/. "$work/$id$support/$dest/"
    pkgbuild --root "$work/$id" --identifier "com.github.apexdesk.apexdesk.$id" --version "$version" \
        --install-location / "$work/pkgs/$id.pkg"
    choices="$choices<choice id=\"$id\" title=\"$title\" start_selected=\"false\"><pkg-ref id=\"com.github.apexdesk.apexdesk.$id\"/></choice>"
    lines="$lines<line choice=\"$id\"/>"
    refs="${refs:-}<pkg-ref id=\"com.github.apexdesk.apexdesk.$id\" version=\"$version\">$id.pkg</pkg-ref>"
}
component dictionaries "Spelling dictionaries" "${APEXDESK_DICT_DIR:-}" dictionaries

cat > "$work/distribution.xml" <<EOF
<?xml version="1.0" encoding="utf-8"?>
<installer-gui-script minSpecVersion="2">
  <title>ApexDesk $version</title>
  <options customize="allow" require-scripts="false" hostArchitectures="arm64,x86_64"/>
  <domains enable_localSystem="true"/>
  <choices-outline>
    <line choice="main"/>$lines
  </choices-outline>
  <choice id="main" title="ApexDesk" enabled="false" start_selected="true"><pkg-ref id="com.github.apexdesk.apexdesk"/></choice>
  $choices
  <pkg-ref id="com.github.apexdesk.apexdesk" version="$version">apexdesk.pkg</pkg-ref>
  ${refs:-}
</installer-gui-script>
EOF

out=$root/dist/ApexDesk-$version.pkg
if [ -n "${APEXDESK_INSTALLER_ID:-}" ]; then
    productbuild --distribution "$work/distribution.xml" --package-path "$work/pkgs" --sign "$APEXDESK_INSTALLER_ID" --timestamp "$out"
else
    productbuild --distribution "$work/distribution.xml" --package-path "$work/pkgs" "$out"
fi
if [ -n "${APEXDESK_NOTARY_PROFILE:-}" ]; then
    xcrun notarytool submit "$out" --keychain-profile "$APEXDESK_NOTARY_PROFILE" --wait
    xcrun stapler staple "$out"
fi
echo "$out"
