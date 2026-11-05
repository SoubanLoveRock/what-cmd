#!/usr/bin/env bash
# Build a release archive for one target: dist/whatcmd-<version>-<target>.tar.gz plus a .sha256 file.
# Usage: scripts/package.sh [target]   (defaults to the host target)
set -euo pipefail

cd "$(dirname "$0")/.."
target="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
name="whatcmd-${version}-${target}"

cargo build --release --locked --target "$target"

stage="dist/${name}"
rm -rf "$stage"
mkdir -p "$stage/shell"
cp "target/${target}/release/whatcmd" "$stage/"
cp shell/whatcmd.zsh "$stage/shell/"
cp README.md "$stage/"
cp config.example.toml "$stage/"
[[ -f LICENSE ]] && cp LICENSE "$stage/"

tar -C dist -czf "dist/${name}.tar.gz" "$name"
rm -rf "$stage"
(cd dist && shasum -a 256 "${name}.tar.gz" > "${name}.tar.gz.sha256")
echo "dist/${name}.tar.gz"
