#!/usr/bin/env bash
# Installs Countersign for the current user: binaries in ~/.local/bin, the
# desktop entry and icons under ~/.local/share. In a release archive it
# installs the binaries that came in bin/; in a source checkout it builds
# them first. If a library Countersign needs is missing, it offers to install
# it with pacman, which shows what it will install and asks first. Nothing
# else is changed; the Hyprland lines it prints are for you to add.
set -euo pipefail

cd "$(dirname "$0")/.."

# A release archive holds the binaries in bin/.
prebuilt=0
[[ -x bin/countersign && -x bin/countersignd ]] && prebuilt=1

# Each library Countersign links against: its pkg-config module, then the
# Arch package that provides it.
libraries=(
  "gtk4 gtk4"
  "libadwaita-1 libadwaita"
  "gtk4-layer-shell-0 gtk4-layer-shell"
  "vte-2.91-gtk4 vte4"
)

# Sets `modules` and `packages` to what is missing.
check() {
  modules=() packages=()
  local have_pkg_config=1 library module package
  if ((prebuilt)); then
    # Running needs only the packages; the build tools are not needed.
    for library in "${libraries[@]}"; do
      read -r module package <<<"$library"
      if command -v pacman >/dev/null && ! pacman -T "$package" >/dev/null; then
        modules+=("$module") packages+=("$package")
      fi
    done
    return
  fi
  if ! command -v pkg-config >/dev/null; then
    have_pkg_config=0 modules=(pkg-config) packages=(base-devel)
  fi
  for library in "${libraries[@]}"; do
    read -r module package <<<"$library"
    if ((!have_pkg_config)) || ! pkg-config --exists "$module"; then
      modules+=("$module") packages+=("$package")
    fi
  done
}

if ((prebuilt)); then
  need=run
else
  need=build
fi

check
if ((${#packages[@]})); then
  to_install=(sudo pacman -S --needed "${packages[@]}")
  if ! command -v pacman >/dev/null; then
    echo "Countersign needs these libraries to $need: ${modules[*]}." >&2
    echo "Install them with your package manager, then run this script again." >&2
    exit 1
  fi
  if [[ ! -t 0 ]]; then
    echo "Countersign needs these packages to $need: ${packages[*]}. Install them, then run this script again:" >&2
    echo "  ${to_install[*]}" >&2
    exit 1
  fi
  echo "Countersign needs these packages to $need: ${packages[*]}"
  echo "Installing them with pacman, which asks before it changes anything:"
  if ! "${to_install[@]}"; then
    echo "They were not installed. Install them, then run this script again." >&2
    exit 1
  fi
  check
  if ((${#packages[@]})); then
    echo "Still missing after installing: ${modules[*]}" >&2
    exit 1
  fi
fi

if ((prebuilt)); then
  built=bin
else
  cargo build --release
  built=target/release
fi

bin=${XDG_BIN_HOME:-$HOME/.local/bin}
data=${XDG_DATA_HOME:-$HOME/.local/share}
install -Dm755 "$built/countersign" "$bin/countersign"
install -Dm755 "$built/countersignd" "$bin/countersignd"
install -Dm644 data/io.github.bvisagie.Countersign.desktop "$data/applications/io.github.bvisagie.Countersign.desktop"
install -Dm644 data/icons/io.github.bvisagie.Countersign.svg "$data/icons/hicolor/scalable/apps/io.github.bvisagie.Countersign.svg"
install -Dm644 data/icons/io.github.bvisagie.Countersign-symbolic.svg "$data/icons/hicolor/symbolic/apps/io.github.bvisagie.Countersign-symbolic.svg"

cat <<'NEXT'
Installed. Open Countersign from your app launcher, or run: countersign

To open quick access with Super+Shift+P and start Countersign when you log in,
add these lines. The key replaces Omarchy's Google Photos shortcut.

  ~/.config/hypr/bindings.lua:   o.rebind("SUPER + SHIFT + P", "Passwords", "countersign --quick")
  ~/.config/hypr/autostart.lua:  o.launch_on_start("countersign --background")
NEXT
