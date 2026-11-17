#!/bin/sh
# Checks that the version written by hand in the packaging files is the
# one in Cargo.toml, and that CI builds with the toolchain
# rust-toolchain.toml pins. Exits non-zero, saying which file is behind,
# when they disagree:
#
#   scripts/check-version.sh
set -u
cd "$(dirname "$0")/.." || exit 2

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
[ -n "$version" ] || { echo "no version in Cargo.toml" >&2; exit 2; }
failed=0
check() {
    # check FILE FOUND: FOUND must be the version.
    if [ "$2" != "$version" ]; then
        echo "$1: version '${2:-none}', Cargo.toml has $version" >&2
        failed=1
    fi
}

# The newest release of the AppStream metadata comes first.
check packaging/linux/flatpak/io.github.apexdesk.ApexDesk.metainfo.xml \
    "$(sed -n 's/.*<release version="\([^"]*\)".*/\1/p' packaging/linux/flatpak/io.github.apexdesk.ApexDesk.metainfo.xml | head -1)"
check packaging/linux/aur/PKGBUILD "$(sed -n 's/^pkgver=//p' packaging/linux/aur/PKGBUILD)"
check packaging/linux/aur/.SRCINFO "$(sed -n 's/^[[:space:]]*pkgver = //p' packaging/linux/aur/.SRCINFO)"
# The download in .SRCINFO names the version twice.
for v in $(grep -o 'download/v[0-9][0-9.]*/apexdesk-[0-9][0-9.]*-' packaging/linux/aur/.SRCINFO | sed 's|download/v\([0-9.]*\)/apexdesk-\([0-9.]*\)-|\1 \2|'); do
    check "packaging/linux/aur/.SRCINFO (source)" "$v"
done
# MSIX versions have four parts (msix.cmd rewrites it from Cargo.toml).
check packaging/windows/msix/AppxManifest.xml \
    "$(sed -n 's/^[[:space:]]*Version="\([0-9.]*\)\.0"$/\1/p' packaging/windows/msix/AppxManifest.xml | head -1)"

# The toolchain: rust-toolchain.toml and the one CI installs.
if [ -f rust-toolchain.toml ]; then
    rust=$(sed -n 's/^channel = "\(.*\)"/\1/p' rust-toolchain.toml)
    for ci in $(grep -o 'toolchain: "*[0-9][0-9.]*' .github/workflows/ci.yml | grep -o '[0-9][0-9.]*$' | sort -u) \
              $(grep -o 'image: rust:[0-9][0-9.]*' .gitlab-ci.yml | grep -o '[0-9][0-9.]*$' | sort -u); do
        if [ "$ci" != "$rust" ]; then
            echo "CI installs Rust $ci, rust-toolchain.toml pins $rust" >&2
            failed=1
        fi
    done
fi

[ $failed -eq 0 ] && echo "version $version everywhere"
exit $failed
