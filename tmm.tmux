#!/usr/bin/env bash
# tmm - tmux session and window switcher on fzf.
# Load it with TPM:
#   set -g @plugin 'zhu-jiyuan/tmm'
# or from ~/.tmux.conf directly:
#   run-shell ~/path/to/tmm/tmm.tmux
# Options go before that line:
#   set -g @tmm-bin           "/path/to/tmm"   # default: tmm next to this script or target/release/tmm, else a fetched release (checkouts), else ~/.cargo/bin/tmm or PATH
#   set -g @tmm-session-key   "s"              # prefix + key: session list (replaces choose-tree -s)
#   set -g @tmm-window-key    "w"              # prefix + key: window list  (replaces choose-tree -w)
#   set -g @tmm-switch-width  "75%"            # popup size, columns or percent
#   set -g @tmm-switch-height "65%"
#   set -g @tmm-projects      "~/work ~/oss:2"  # project roots, scanned 1 level deep (or :N); enables the project list
#   set -g @tmm-project-key   "f"              # prefix + key: project list (replaces find-window), only with @tmm-projects

CURRENT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

get_opt() {
  local value
  value="$(tmux show-option -gqv "$1")"
  printf '%s' "${value:-$2}"
}

find_bin() {
  for candidate in "$@"; do
    if [ -n "$candidate" ] && [ -x "$candidate" ]; then
      bin="$candidate"
      return
    fi
  done
}

bin="$(get_opt "@tmm-bin" "")"
# A release tarball puts the binary next to this script; a developer's checkout has a build.
[ -n "$bin" ] || find_bin "$CURRENT_DIR/tmm" "$CURRENT_DIR/target/release/tmm"

# A checkout (TPM clones one) has neither: fetch the release for this platform
# next to this script, and fetch again when the checkout moves to another
# version. A symlinked tmm is a developer's own and is left alone.
fetched="$CURRENT_DIR/tmm"
version="$(grep -m1 '^version' "$CURRENT_DIR/Cargo.toml" 2>/dev/null | cut -d'"' -f2)"
if [ -n "$version" ] && [ ! -L "$fetched" ]; then
  if [ -z "$bin" ] || { [ "$bin" = "$fetched" ] && [ "$("$bin" --version 2>/dev/null)" != "tmm $version" ]; }; then
    if "$CURRENT_DIR/scripts/install.sh"; then
      bin="$fetched"
      tmux display-message "tmm $version installed"
    elif [ -z "$bin" ]; then
      tmux display-message "tmm: could not install the binary; see scripts/install.sh or set @tmm-bin"
    fi
  fi
fi

# Last resorts: a binary installed some other way.
[ -n "$bin" ] || find_bin "$HOME/.cargo/bin/tmm" "$(command -v tmm 2>/dev/null)"
if [ -z "$bin" ]; then
  tmux display-message "tmm: binary not found; run cargo build --release or set @tmm-bin"
  exit 0
fi

# display-popup does not expand formats in its command (tmux 3.4), so run-shell
# fills in the client name and opens the popup on that client. The border takes
# the status bar's background colour, or the default when it has none.
size="-w '$(get_opt "@tmm-switch-width" "75%")' -h '$(get_opt "@tmm-switch-height" "65%")'"
border="-S 'fg=#{?#{m/r:bg=,#{status-style}},#{s/.*bg=([^,]*).*/\\1/:status-style},default}'"

bind_popup() {
  # $1 key, $2 popup title, $3 extra tmm arguments
  tmux bind-key "$1" run-shell -b "tmux display-popup -c '#{q:client_name}' -E -b rounded -T '#[fg=white] $2 ' $size $border \"'$bin' switch '#{q:client_name}' $3\""
}

bind_popup "$(get_opt "@tmm-session-key" "s")" "tmm" ""
bind_popup "$(get_opt "@tmm-window-key" "w")" "tmm" "--windows"
if [ -n "$(get_opt "@tmm-projects" "")" ]; then
  bind_popup "$(get_opt "@tmm-project-key" "f")" "tmm" "--projects"
fi
