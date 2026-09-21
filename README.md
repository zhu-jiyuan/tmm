# tmm

[中文](README.zh.md)

A personal tmux plugin: one fzf popup to switch between sessions and windows,
create, rename and close them, with a dot per window showing whether Claude
Code or Codex is working or waiting for you. A single Rust binary; every key
runs one subcommand that takes a few milliseconds.

![tmm popup](docs/tmm.svg)

## Install

Needs tmux 3.3+ and fzf 0.74.3+. Either `cargo build --release` or download
the tarball for your platform from Releases and unpack it. Then add one line to
your tmux config and reload:

```tmux
run-shell /path/to/tmm.tmux
```

For the agent dots run `tmm install-hooks` once. It merges its hooks into
`~/.claude/settings.json` and `~/.codex/hooks.json`, backing them up first.

## Keys

`prefix + s` opens the session list, `prefix + w` the window list and
`prefix + f` the project list. Just type to filter.

| Key | Action |
|---|---|
| `Ctrl-j` `Ctrl-k` | move; `Ctrl-f` `Ctrl-b` page |
| `Enter` | switch; with no match, create a session named after the query |
| `Tab` | cycle the session, window and project lists |
| `Ctrl-o` `Ctrl-r` `Ctrl-x` | new / rename / close, all on the input line |
| `Ctrl-s` | star, pins the session to the top |
| `Ctrl-l` | preview the next window |
| `Ctrl-v` `Ctrl-t` | toggle the preview / full-screen preview |
| `Ctrl-/` | key legend |
| `Esc` | close |

Options, set before the `run-shell` line: `@tmm-session-key`,
`@tmm-window-key`, `@tmm-project-key`, `@tmm-projects`, `@tmm-switch-width`,
`@tmm-switch-height`, `@tmm-bin`.

## Projects

Point `@tmm-projects` at the directories that hold your projects and
`prefix + f` lists them, open or not. `Enter` switches to a project's
session, creating it in that directory when there is none yet.

```tmux
set -g @tmm-projects "~/work ~/oss:2"
```

Each root is scanned one level deep, or `:N` levels; hidden directories are
skipped. A session belongs to the directory it was started in, so a project
opened some other way still shows as open. With no match, typing a path and
pressing `Enter` opens that directory. Without this option `prefix + f`
keeps its tmux meaning.
