# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`tmm` is a personal tmux plugin: an fzf popup for switching, creating, renaming and closing sessions and windows, with a coloured dot per window showing whether Claude Code or Codex is working or waiting. It is a single Rust binary (`src/`) plus a tmux entry script (`tmm.tmux`). README.md and README.zh.md are twins; keep both in sync when keys or options change.

## Commands

Rust edition 2024 with let-chains, so the toolchain must be 1.88+. CI runs exactly these, in this order:

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

Single tests:

```sh
cargo test --locked agent::tests::screen_hints            # one unit test
cargo test --locked --test integration inline_prompts_and_closing
```

The integration tests (`tests/integration.rs`) need `tmux` on PATH and skip silently without it. Each test starts its own server on a private socket (`tmux -L tmm-test-<pid>-<tag>`), so they never touch the real tmux server, and they drive the subcommands exactly as fzf would: `TMUX`, `TMM_STATE_DIR`, `TMM_SNAPSHOT`, `FZF_QUERY`, `FZF_PREVIEW_COLUMNS/LINES`, `FZF_IDLE_TIME_MS` set in the environment, assertions on the printed fzf action strings.

Running it for real: `cargo build --release`. `tmm.tmux` resolves the binary once, when tmux sources it (`@tmm-bin`, then next to the script, then `target/release/tmm`, `~/.cargo/bin/tmm`, PATH). A rebuilt binary at the same path is picked up by the next popup with no reload; a change to `tmm.tmux` itself needs the tmux config re-sourced. `tmm activity` prints the per-window agent states as JSON and is the easiest way to poke at the collector from a shell inside tmux.

Release: `scripts/package.sh <rust-target>` builds `dist/tmm-v<version>-<target>.tar.gz` (binary + `tmm.tmux` + README + LICENSE). Pushing a `v*` tag runs `.github/workflows/release.yml`, which does this for four targets and publishes a GitHub release. The version lives only in `Cargo.toml`.

## Architecture

**One binary, one process per keystroke.** `tmm switch <client>` is the only long-lived command: it writes the rows, spawns fzf, waits, and acts on the result. Every fzf key binding (`popup.rs`, the `binds` array) runs a fresh `tmm <subcommand>` via `transform`/`execute-silent`, and the subcommand *prints an fzf action string* (`reload-sync(...)`, `change-prompt(...)+change-query(...)`, `accept`, `abort`, ...) that fzf then applies. `actions::arg()` wraps values in whichever bracket pair they do not contain. Because each call is a fresh process, what matters for feel is startup cost and the number of `tmux` subprocesses per call (~4 ms each). `tmux::panes()` deliberately fetches everything the rows and the agent collector need in a single `list-panes -a` format string; keep it that way rather than adding second queries.

**Row protocol.** `rows.rs` emits one tab-separated line per row: `id`, session name, padded tree name, padded breadcrumb name, padded dots, badge. fzf displays `3,5,6` while the query is empty and `4,5,6` while filtering (`TREE_FIELDS`/`FILTER_FIELDS` in `popup.rs`), searches only the first displayed field, and uses field 1 (`$n` session id or `@n` window id) via `--id-nth` to keep the cursor stable across reloads. In windows mode the tree column shows a window as `├ 0  name` under a bold session header, while the breadcrumb column shows `session:0  name`. fzf cannot match hidden text, so the `change` binding runs `tmm with-nth` to swap the columns; that swap is what keeps "type a session name, see its windows" working. Both name columns are padded to one width so the swap moves nothing else. Window rows put the session name in field 2 so `ctrl-s` (which takes `{2}`) stars the right session. fzf hands over an empty `{1}` when nothing matches the filter; since tmux treats an empty target as the current session, every row-keyed subcommand must no-op on an empty id (tested).

**Per-popup state is files, not memory.** `switch` writes the row snapshot to `<state>/run/switch-<pid>.txt` and exports its path as `TMM_SNAPSHOT` to every child. Small bits of state are *sidecars* of that snapshot (`paths::sidecar`), distinguished only by extension: `.windows` (exists = windows mode), `.prompt` (JSON of the open inline prompt), `.preview` (chosen preview window per session), `.view` (fzf action to replay after the full-screen preview), `.help` (exists = legend hidden). `popup::cleanup` removes all of them on exit and `remove_stale` sweeps snapshots whose pid is dead. Adding a new kind of per-popup state means a new extension in `SIDECARS`.

**Inline prompts.** `ctrl-o`/`ctrl-r` never leave fzf: `actions::prompt` swaps the prompt, prefills the query, sets a header, disables search, and saves a `Pending` (kind, row id, old name, the user's filter) to the `.prompt` sidecar. `enter` and `esc` are both routed through tmm so they can check for a pending prompt first; otherwise they print plain `accept`/`abort`. `tab` (mode toggle) cancels any pending prompt. `restore()` builds the action that puts prompt, query, header and search back. While a prompt is open, `with-nth` decides by the filter saved in the sidecar rather than the live query, so the columns do not flip as the user types a name.

**Refresh loop.** One fzf `every(1)` binding does `refresh-preview` plus a background `tmm refresh <snapshot>`. `refresh` only reloads when fzf reports ≥1 s idle, the rows actually differ from the snapshot, and neither favorites nor mode changed while it was computing (a key that changed them reloads on its own). The preview command must stay a short-lived process; a lingering one makes fzf draw a spinner.

**Persistent state.** Only starred sessions, in `switcher.json` under the state dir: `$TMM_STATE_DIR`, else `$XDG_STATE_HOME/tmm`, else `~/.local/state/tmm`. Written atomically via `paths::write_atomic` (temp file + rename); use it for anything fzf might read concurrently.

**Agent activity (`agent.rs`).** Two sources, merged per window:
- *Hooks*: Claude Code and Codex lifecycle hooks run `tmm hook <provider> [event]` inside the agent's pane. It maps the event to a `State` (`event_state`), walks up the process tree from its own parent to find the agent process, and stores a `Record` (state, pid, process start time, provider, event) on the pane as tmux user option `@tmm-agent`. pid + start time tie the record to one specific process so a reused PID cannot resurrect a stale record. `main.rs` swallows every hook error: a hook must never fail in front of an agent.
- *Fallback*: when there is no record or it is stale, `pane_state` checks the pane's process tree for an agent executable (`agent_name`, judged by executable path, never by prose in arguments) and then reads the last screen lines with regexes (`screen_state`): a question or limit message means waiting, a busy hint means working, nothing means waiting. A `PermissionRequest` record is always re-checked against the screen, since that hook fires before approval.
- *Cost control*: `states()` runs `ps -t` only for the ttys of "suspicious" panes (a record exists, or the foreground command starts with `claude`/`codex` or is `node`/`bun`) and `capture-pane` only for those. `TMM_AGENT_SCAN=always` scans every pane; the integration test needs it because its fake agent is a `/bin/sh` symlinked as `claude`.
- `State` is ordered `Plain < Working < Waiting`; a window's state is the max over its panes.

**Hook installation (`install.rs`).** `tmm install-hooks` merges command hooks into `~/.claude/settings.json` and `~/.codex/hooks.json`, keeping foreign entries, replacing earlier tmm entries (recognised by a command containing `tmm` and ` hook `), backing the file up first, and chmod 600. The command bakes in the absolute path of the running binary (`fzf::me()`), so moving the binary means re-running `install-hooks`. The event lists per provider are in that file; `agent.rs` must know how to map any event added there.

**Styling follows tmux.** `fzf::colours()` reads `#{status-style}` and paints the cursor row with the status bar's bg/fg, or leaves the highlight to the user's own fzf theme (`FZF_DEFAULT_OPTS`) when the bar has no bg. Rows keep their own ANSI colours and attributes on the cursor row (`regular` without `strip`), so the dots stay readable there and only session headers are bold. `tmm.tmux` gives the popup border the same bg. Dot colours are fixed ANSI (`rows::dots`: dim grey = no agent, green = working, yellow = waiting) and `NO_COLOR` is stripped from fzf's environment on purpose. `ansi.rs` measures visible width (skipping CSI sequences, counting CJK as two cells) for column padding and preview clipping; use `visible_width`/`pad`/`clip_line` rather than `.len()` on anything that reaches the terminal.

## Conventions

- Key bindings follow fzf-lua habits and are control keys only, no arrows or function keys (HHKB). fzf's own editing keys (`ctrl-a/e/u/w`, `ctrl-j/k/n/p`) are left alone. A new key touches the `binds` array and `LEGEND` in `popup.rs`, plus the key tables in both READMEs.
- `fzf::MIN_VERSION` lists the fzf features that pin it; bump it (and both READMEs) when a new fzf feature is used. tmux 3.3+ is required for `display-popup` with a border style.
- fzf runs our callbacks with `SHELL=/bin/sh`; quote anything passed on a command line with `fzf::quote`.
