#!/usr/bin/env bash
# Put the tmm binary for this machine next to tmm.tmux.
#
# tmm.tmux runs this when a checkout has no binary, or when the binary it
# fetched earlier is not the version the checkout expects (after `prefix + U`
# in TPM). It can also be run by hand:
#   ~/.tmux/plugins/tmm/scripts/install.sh
#
# The release tarball for this platform is downloaded from GitHub and checked
# against the SHA-256 digest GitHub publishes for the asset. When there is no
# release for the platform, or the download fails, it builds with cargo. An
# existing binary is replaced only once the new one is known to run.
set -euo pipefail

dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repo="zhu-jiyuan/tmm"
version="$(grep -m1 '^version' "$dir/Cargo.toml" | cut -d'"' -f2)"
bin="$dir/tmm"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

say() { printf 'tmm: %s\n' "$*"; }

# The release target for this machine, or nothing.
target() {
  case "$(uname -s)-$(uname -m)" in
    Darwin-arm64) echo aarch64-apple-darwin ;;
    Darwin-x86_64) echo x86_64-apple-darwin ;;
    Linux-x86_64) echo x86_64-unknown-linux-gnu ;;
    Linux-aarch64 | Linux-arm64) echo aarch64-unknown-linux-gnu ;;
  esac
}

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

# The sha256 GitHub recorded for a release asset, or nothing.
published_digest() {
  curl -fsSL "https://api.github.com/repos/$repo/releases/tags/v$version" 2>/dev/null \
    | tr -d '\n ' \
    | grep -o "\"name\":\"$1\".*" \
    | grep -o '"digest":"sha256:[0-9a-f]*"' \
    | head -1 \
    | sed 's/.*sha256://; s/"//'
}

# Download the release for `$1`, verify it, and leave the binary in $tmp/tmm.
download() {
  local name="tmm-v$version-$1"
  local url="https://github.com/$repo/releases/download/v$version/$name.tar.gz"
  command -v curl >/dev/null 2>&1 || return 1
  say "downloading $url"
  curl -fsSL --retry 2 -o "$tmp/$name.tar.gz" "$url" || return 1
  local expected actual
  expected="$(published_digest "$name.tar.gz" || true)"
  actual="$(sha256 "$tmp/$name.tar.gz")"
  if [ -z "$expected" ]; then
    say "GitHub published no digest for $name.tar.gz; not verified"
  elif [ -z "$actual" ]; then
    say "no sha256sum or shasum on this machine; not verified"
  elif [ "$expected" != "$actual" ]; then
    say "checksum mismatch for $name.tar.gz: expected $expected, got $actual"
    return 1
  fi
  tar -xzf "$tmp/$name.tar.gz" -C "$tmp" "$name/tmm" || return 1
  mv "$tmp/$name/tmm" "$tmp/tmm"
}

build() {
  command -v cargo >/dev/null 2>&1 || return 1
  say "building tmm $version with cargo"
  (cd "$dir" && cargo build --release --locked) || return 1
  cp "$dir/target/release/tmm" "$tmp/tmm"
}

# Move the built or downloaded binary into place once it is known to run.
install() {
  chmod +x "$tmp/tmm"
  "$tmp/tmm" --version >/dev/null 2>&1 || return 1
  mv "$tmp/tmm" "$bin.new" && mv "$bin.new" "$bin"
}

if [ -L "$bin" ]; then
  say "$bin is a symlink; leaving it alone"
  exit 0
fi
if [ -x "$bin" ] && [ "$("$bin" --version 2>/dev/null)" = "tmm $version" ]; then
  say "tmm $version is already installed"
  exit 0
fi

target="$(target)"
if [ -n "$target" ] && download "$target" && install; then
  say "installed tmm $version at $bin"
  exit 0
fi
if [ -n "$target" ]; then
  say "no usable release for $target"
else
  say "no release for $(uname -s)/$(uname -m)"
fi
if build && install; then
  say "built tmm $version at $bin"
  exit 0
fi
say "could not install tmm $version; build it yourself and set @tmm-bin"
exit 1
