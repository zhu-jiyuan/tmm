#!/usr/bin/env bash
# Build a release binary for one target and bundle it with the plugin script:
#   scripts/package.sh aarch64-apple-darwin
# Produces dist/tmm-v<version>-<target>.tar.gz.
set -euo pipefail
cd "$(dirname "$0")/.."

target="$1"
version="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
name="tmm-v${version}-${target}"

cargo build --release --locked --target "$target"

rm -rf "dist/$name"
mkdir -p "dist/$name"
cp "target/$target/release/tmm" tmm.tmux README.md LICENSE "dist/$name/"
tar -C dist -czf "dist/$name.tar.gz" "$name"
rm -rf "dist/$name"
ls -l "dist/$name.tar.gz"
