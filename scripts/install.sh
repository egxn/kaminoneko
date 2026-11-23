#!/usr/bin/env bash
# Installs Kaminoneko for the current user: binary, icon and app menu entry.
#
#   ./install.sh               install or update
#   ./install.sh --autostart   also start the top bar icon on login
#   ./install.sh --uninstall   remove everything this script installs
#
# Works from a release tarball (binary next to this script) and from the
# repository (builds with cargo first).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
data_dir="${XDG_DATA_HOME:-$HOME/.local/share}"
config_dir="${XDG_CONFIG_HOME:-$HOME/.config}"

bin="$bin_dir/kaminoneko"
desktop="$data_dir/applications/kaminoneko.desktop"
icon="$data_dir/icons/hicolor/scalable/apps/kaminoneko.svg"
autostart="$config_dir/autostart/kaminoneko.desktop"

refresh_menus() {
    if command -v update-desktop-database >/dev/null; then
        update-desktop-database -q "$data_dir/applications" || true
    fi
    if command -v gtk-update-icon-cache >/dev/null; then
        gtk-update-icon-cache -q -t -f "$data_dir/icons/hicolor" || true
    fi
}

autostart_wanted=no
case "${1:-}" in
    "") ;;
    --autostart) autostart_wanted=yes ;;
    --uninstall)
        rm -f "$bin" "$desktop" "$icon" "$autostart"
        refresh_menus
        echo "Kaminoneko removed. Quit it from its top bar menu if it is still running."
        exit 0
        ;;
    -h | --help)
        sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'
        exit 0
        ;;
    *)
        echo "Unknown option: $1 (see --help)" >&2
        exit 2
        ;;
esac

# --- find the binary and the icon
if [[ -x "$here/kaminoneko" ]]; then
    src_bin="$here/kaminoneko"
    src_icon="$here/kaminoneko.svg"
else
    root="$(dirname "$here")"
    src_bin="$root/target/release/kaminoneko"
    src_icon="$root/assets/kaminoneko.svg"
    if command -v cargo >/dev/null; then
        echo "Building the release binary…"
        cargo build --release --quiet --manifest-path "$root/Cargo.toml"
    elif [[ ! -x "$src_bin" ]]; then
        echo "No binary found and cargo is not installed. Use a release tarball or install Rust." >&2
        exit 1
    fi
fi

# --- install
mkdir -p "$bin_dir" "$(dirname "$desktop")" "$(dirname "$icon")"
# Copy, then rename: updating works while Kaminoneko is running.
install -m 755 "$src_bin" "$bin.new"
mv -f "$bin.new" "$bin"
install -m 644 "$src_icon" "$icon"

# Exec uses the full path: ~/.local/bin is not always in the session's PATH.
entry() {
    cat <<EOF
[Desktop Entry]
Type=Application
Name=Kaminoneko
GenericName=Screenshot printer
Comment=Take a screenshot and print it on the kids' camera
Exec="$bin"
Icon=kaminoneko
Terminal=false
StartupNotify=false
Categories=Utility;
Keywords=print;printer;screenshot;thermal;camera;
Actions=capture;open;

[Desktop Action capture]
Name=Take screenshot and print
Exec="$bin" capture

[Desktop Action open]
Name=Print an image…
Exec="$bin" open
EOF
}
entry >"$desktop"

# Keep an autostart entry up to date if the user already had one.
if [[ $autostart_wanted == yes || -f $autostart ]]; then
    mkdir -p "$(dirname "$autostart")"
    entry >"$autostart"
    autostart_wanted=yes
fi

if command -v desktop-file-validate >/dev/null; then
    desktop-file-validate "$desktop" || true
fi
refresh_menus

echo "Installed $("$bin" --version 2>/dev/null || echo Kaminoneko) to $bin"
echo "  App menu: search for “Kaminoneko”. Right-click it for “Take screenshot and print”"
echo "  and “Print an image…”. Opening it while the cat is in the top bar takes a screenshot."
if [[ $autostart_wanted == yes ]]; then
    echo "  Starts on login (undo: rm \"$autostart\")."
else
    echo "  Does not start on login (add it with: $0 --autostart)."
fi
case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) echo "  Note: $bin_dir is not in PATH; use the full path in a terminal." ;;
esac
