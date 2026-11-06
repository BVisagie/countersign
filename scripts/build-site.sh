#!/usr/bin/env bash
# Assembles the web page in a folder: site/, plus the version, screenshots and
# icon it shows. .github/workflows/pages.yml publishes the result.
#
#   scripts/build-site.sh [out-dir]
set -euo pipefail

cd "$(dirname "$0")/.."
out=${1:-target/site}
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
[ -n "$version" ] || { echo "No version in Cargo.toml." >&2; exit 1; }

mkdir -p "$out/assets"
cp -r site/. "$out/"
cp docs/screenshots/main.png docs/screenshots/edit.png docs/screenshots/sends.png "$out/assets/"
cp data/icons/io.github.bvisagie.Countersign.svg "$out/assets/icon.svg"
sed -i "s/@VERSION@/$version/g" "$out/index.html"
