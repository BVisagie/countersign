#!/usr/bin/env bash
# Builds a release and packs it for a GitHub release, in dist/:
#
#   countersign-<arch>.tar.gz  the binaries, desktop entry, icons, license
#                              and install script, in countersign-<arch>/
#   SHA256SUMS                 its checksum, for `sha256sum -c`
#
# The names carry no version, so the same commands download, unpack and
# install any release: releases/latest/download/countersign-x86_64.tar.gz
# always works, and a newer archive unpacks over an older one.
set -euo pipefail

cd "$(dirname "$0")/.."

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
arch=$(uname -m)
archive=countersign-$arch.tar.gz
stage=dist/countersign-$arch

cargo build --release --locked

rm -rf dist
install -Dm755 target/release/countersign "$stage/bin/countersign"
install -Dm755 target/release/countersignd "$stage/bin/countersignd"
install -Dm755 scripts/install.sh "$stage/scripts/install.sh"
install -Dm644 data/io.github.bvisagie.Countersign.desktop "$stage/data/io.github.bvisagie.Countersign.desktop"
install -Dm644 data/icons/io.github.bvisagie.Countersign.svg "$stage/data/icons/io.github.bvisagie.Countersign.svg"
install -Dm644 data/icons/io.github.bvisagie.Countersign-symbolic.svg "$stage/data/icons/io.github.bvisagie.Countersign-symbolic.svg"
install -Dm644 LICENSE "$stage/LICENSE"

# The same commit gives the same file list, order, owners and times.
tar -C dist --sort=name --owner=0 --group=0 --numeric-owner \
  --mtime="@$(git log -1 --format=%ct)" -czf "dist/$archive" "${stage#dist/}"
rm -rf "$stage"
(cd dist && sha256sum "$archive" >SHA256SUMS)

echo "Packed dist/$archive for Countersign $version"
