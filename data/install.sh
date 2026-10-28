#!/bin/sh
# Installs TabExtend for the current user (no root needed).
# Run from the unpacked release folder: ./install.sh   Remove: ./install.sh --uninstall
set -eu
here=$(dirname "$0")
bin="${XDG_BIN_HOME:-$HOME/.local/bin}"
share="${XDG_DATA_HOME:-$HOME/.local/share}"
id=io.github.st3inberg.TabExtend
if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$bin/tabextend" "$bin/tabextend-app" \
        "$share/applications/$id.desktop" "$share/icons/hicolor/scalable/apps/$id.svg"
    echo "Removed. Pairings stay in $share/tabextend; delete that folder to forget them."
    exit 0
fi
mkdir -p "$bin" "$share/applications" "$share/icons/hicolor/scalable/apps"
for program in tabextend tabextend-app; do
    if [ -f "$here/$program" ]; then install -m 755 "$here/$program" "$bin/"; fi
done
install -m 644 "$here/$id.desktop" "$share/applications/"
install -m 644 "$here/$id.svg" "$share/icons/hicolor/scalable/apps/"
echo "Installed to $bin. TabExtend is now in your app list."
case ":$PATH:" in *":$bin:"*) ;; *) echo "Note: $bin is not on your PATH." ;; esac
