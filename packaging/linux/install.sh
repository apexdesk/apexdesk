#!/bin/sh
# Installs ApexDesk for the current user: binary, icon and launcher entry, so
# it shows in the applications menu and in "Open with" for its file types.
#
#   packaging/linux/install.sh              build (release) and install
#   packaging/linux/install.sh --uninstall  remove everything it installed
#
# The program goes in ~/.local/lib/apexdesk, as the packages do in
# /usr/lib/apexdesk; ~/.local/bin/apexdesk links to it.
#
# Nothing is made the default application; to do that for a type, use
# "Open with → Always use" in the file manager, or for example:
#   xdg-mime default apexdesk.desktop application/vnd.openxmlformats-officedocument.spreadsheetml.sheet
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
data=${XDG_DATA_HOME:-$HOME/.local/share}
bin=$HOME/.local/bin
lib=$HOME/.local/lib/apexdesk
icons=$data/icons/hicolor
doc=$data/doc/apexdesk

if [ "${1:-}" = "--uninstall" ]; then
    rm -rf "$lib"
    rm -f "$bin/apexdesk" "$data/applications/apexdesk.desktop" \
        "$icons/scalable/apps/apexdesk.svg" "$icons/256x256/apps/apexdesk.png" \
        "$doc/THIRD-PARTY-NOTICES.md" "$doc/LICENSE" "$doc/EULA.md" "$doc/EULA.es.md" "$data/mime/packages/apexdesk.xml"
    rmdir "$doc" 2>/dev/null || true
    update-desktop-database -q "$data/applications" 2>/dev/null || true
    update-mime-database "$data/mime" 2>/dev/null || true
    echo "ApexDesk uninstalled."
    exit 0
fi

(cd "$root" && cargo build --release -p apex-app)

# The program was called DemosOffice and GensWorkOffice for a while: those installs
# go (the settings move to the new folder when the program first starts).
for old in demosoffice gensworkoffice; do
    rm -rf "$HOME/.local/lib/$old" "$data/doc/$old"
    rm -f "$bin/$old" "$data/applications/$old.desktop" \
        "$icons/scalable/apps/$old.svg" "$icons/256x256/apps/$old.png" \
        "$data/mime/packages/$old.xml"
done

install -Dm755 "$root/target/release/apexdesk" "$lib/apexdesk"
# pdfium came with earlier versions; PDF pages are now ApexDesk's own.
rm -rf "$lib/components/pdfium"
mkdir -p "$bin"
ln -sf "$lib/apexdesk" "$bin/apexdesk"
install -Dm644 "$here/apexdesk.svg" "$icons/scalable/apps/apexdesk.svg"
install -Dm644 "$here/apexdesk-256.png" "$icons/256x256/apps/apexdesk.png"
install -Dm644 "$root/THIRD-PARTY-NOTICES.md" "$doc/THIRD-PARTY-NOTICES.md"
install -Dm644 "$root/LICENSE" "$doc/LICENSE"
install -Dm644 "$root/EULA.md" "$doc/EULA.md"
install -Dm644 "$root/EULA.es.md" "$doc/EULA.es.md"
install -Dm644 "$here/apexdesk-mime.xml" "$data/mime/packages/apexdesk.xml"
mkdir -p "$data/applications"
# Absolute path: ~/.local/bin is not always in the launcher's PATH.
sed "s|^Exec=apexdesk|Exec=$bin/apexdesk|" "$here/apexdesk.desktop" > "$data/applications/apexdesk.desktop"

update-desktop-database -q "$data/applications" 2>/dev/null || true
update-mime-database "$data/mime" 2>/dev/null || true
touch "$icons" 2>/dev/null || true
gtk-update-icon-cache -q -t "$icons" 2>/dev/null || true

echo "ApexDesk installed in $bin/apexdesk."
echo "It now appears in the applications menu and under \"Open with\"."
