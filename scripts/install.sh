#!/usr/bin/env bash
# Builds Countersign and installs it for the current user: binaries in
# ~/.local/bin, the desktop entry and icon under ~/.local/share. Nothing
# else is changed; the Hyprland lines it prints are for you to add.
set -euo pipefail

cd "$(dirname "$0")/.."
features=()
if ! pkg-config --exists vte-2.91-gtk4; then
  echo "vte4 is not installed: sign-in will open your own terminal instead of running inside Countersign."
  features=(--no-default-features)
fi
cargo build --release "${features[@]}"

bin=${XDG_BIN_HOME:-$HOME/.local/bin}
data=${XDG_DATA_HOME:-$HOME/.local/share}
install -Dm755 target/release/countersign "$bin/countersign"
install -Dm755 target/release/countersignd "$bin/countersignd"
install -Dm644 data/io.github.bvisagie.Countersign.desktop "$data/applications/io.github.bvisagie.Countersign.desktop"
install -Dm644 data/icons/io.github.bvisagie.Countersign.svg "$data/icons/hicolor/scalable/apps/io.github.bvisagie.Countersign.svg"

cat <<'NEXT'
Installed. To open quick access with a key and start Countersign with Hyprland, add:

  ~/.config/hypr/bindings.lua:   o.bind("SUPER + SHIFT + P", "Passwords", "countersign --quick")
  ~/.config/hypr/autostart.lua:  o.launch_on_start("countersign --background")
NEXT
