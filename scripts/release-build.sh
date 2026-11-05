#!/usr/bin/env bash
# Builds, checks and packs a release in a throwaway Arch Linux container, as
# root, from the repository's root. The release workflow runs it on x86_64 in
# Arch Linux and on aarch64 in Arch Linux ARM. It changes the system's
# packages, so never run it on your own computer.
#
# Packages come from Omarchy's stable mirror for the machine's architecture,
# so the binaries link the library versions Omarchy users have. Then it runs
# the repository's checks, packs dist/countersign-<arch>.tar.gz with
# scripts/package.sh, and starts the packed app offscreen on the demo vault,
# saving its screenshots in smoke/.
set -euo pipefail

cd "$(dirname "$0")/.."

arch=$(uname -m)
# Single quotes: pacman fills in $repo and $arch.
# shellcheck disable=SC2016
case $arch in
  x86_64) server='https://stable-mirror.omarchy.org/$repo/os/$arch' ;;
  aarch64) server='https://arm-stable-mirror.omarchy.org/$arch/$repo' ;;
  *)
    echo "Omarchy has no mirror for $arch" >&2
    exit 1
    ;;
esac
echo "Server = $server" >/etc/pacman.d/mirrorlist
# Inside a container pacman cannot work out free space, and cannot sandbox
# its downloads. Packages are still checked against their signatures.
sed -i 's/^CheckSpace/#CheckSpace/' /etc/pacman.conf
grep -q '^DisableSandbox' /etc/pacman.conf || sed -i '/^\[options\]/a DisableSandbox' /etc/pacman.conf

if [[ $arch == aarch64 ]]; then
  # The Arch Linux ARM root filesystem ships its keyring uninitialized, and a
  # kernel and firmware a container never uses; updating them is slow.
  pacman-key --init
  pacman-key --populate archlinuxarm
  pacman -Qq | grep -E '^linux-(aarch64|firmware)' | xargs -r pacman -Rdd --noconfirm
fi

# -uu also downgrades, so every package matches the mirror's version.
pacman -Syuu --noconfirm --needed base-devel git gtk4 libadwaita gtk4-layer-shell vte4 rustup ttf-dejavu

# The workspace belongs to the runner's user; git refuses it otherwise, and
# package.sh needs the commit's time.
git config --global --add safe.directory "$PWD"

rustup default stable
rustup component add clippy rustfmt
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

scripts/package.sh

# Start the packed app, not the build tree's, so an archive that cannot start
# fails here. The app needs a session bus; inside a container there is no
# desktop for dbus-run-session to disturb.
unpacked=$(mktemp -d)
tar -C "$unpacked" -xzf "dist/countersign-$arch.tar.gz"
rm -rf smoke
COUNTERSIGN_BIN="$unpacked/countersign-$arch/bin/countersign" dbus-run-session -- scripts/screenshots.sh smoke
rm -rf "$unpacked"
if [[ ! -s smoke/current/main.png ]]; then
  echo "The packed app did not draw its main window" >&2
  exit 1
fi
echo "The packed app started and drew $(find smoke -name '*.png' | wc -l) screenshots"
