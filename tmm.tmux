#!/usr/bin/env bash
# tmm - tmux session and window switcher on fzf.
# Load it from ~/.tmux.conf with:
#   run-shell ~/path/to/tmm/tmm.tmux
# Options go before that line:
#   set -g @tmm-bin           "/path/to/tmm"   # default: tmm next to this script, target/release/tmm, ~/.cargo/bin/tmm, or PATH
#   set -g @tmm-session-key   "s"              # prefix + key: session list (replaces choose-tree -s)
#   set -g @tmm-window-key    "w"              # prefix + key: window list  (replaces choose-tree -w)
#   set -g @tmm-switch-width  "75%"            # popup size, columns or percent
#   set -g @tmm-switch-height "65%"

CURRENT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

get_opt() {
  local value
  value="$(tmux show-option -gqv "$1")"
  printf '%s' "${value:-$2}"
}

bin="$(get_opt "@tmm-bin" "")"
if [ -z "$bin" ]; then
  # A release tarball puts the binary next to this script.
  for candidate in "$CURRENT_DIR/tmm" "$CURRENT_DIR/target/release/tmm" "$HOME/.cargo/bin/tmm" "$(command -v tmm 2>/dev/null)"; do
    if [ -n "$candidate" ] && [ -x "$candidate" ]; then
      bin="$candidate"
      break
    fi
  done
fi
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
