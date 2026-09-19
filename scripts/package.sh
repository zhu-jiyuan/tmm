#!/usr/bin/env bash
# Build a release binary for one target and bundle it with the plugin script:
#   scripts/package.sh aarch64-apple-darwin
# Produces dist/tmm-v<version>-<target>.tar.gz and its .sha256.
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

cd dist
if command -v sha256sum >/dev/null; then
  sha256sum "$name.tar.gz" > "$name.tar.gz.sha256"
else
  shasum -a 256 "$name.tar.gz" > "$name.tar.gz.sha256"
fi
cat "$name.tar.gz.sha256"
