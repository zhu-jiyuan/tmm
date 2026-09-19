# tmm

[English](README.md)

个人用的 tmux 管理插件：一个 fzf 弹窗，用来在 session 和窗口之间切换、新建、
改名、关闭，并用圆点显示每个窗口里 Claude Code / Codex 是在工作还是在等你。
Rust 单二进制，每个按键只跑一次几毫秒的子命令。

![tmm 弹窗](docs/tmm.svg)

## 安装

依赖 tmux 3.3+ 和 fzf 0.74.3+。`cargo build --release`，或从 Releases 下载
对应平台的 tarball 解压。然后在 tmux 配置里加一行并重载：

```tmux
run-shell /path/to/tmm.tmux
```

想看 agent 的工作 / 等待状态，跑一次 `tmm install-hooks`，它会把 hook 合并进
`~/.claude/settings.json` 和 `~/.codex/hooks.json`（先备份）。

## 键位

`prefix + s` 打开 session 列表，`prefix + w` 打开窗口列表。打开后直接打字过滤。

| 键 | 动作 |
|---|---|
| `Ctrl-j` `Ctrl-k` | 移动，`Ctrl-f` `Ctrl-b` 翻页 |
| `Enter` | 切换过去；没有匹配时新建同名 session |
| `Tab` | session 列表 / 窗口列表切换 |
| `Ctrl-o` `Ctrl-r` `Ctrl-x` | 新建 / 改名 / 关闭，都在输入行内完成 |
| `Ctrl-s` | 收藏，置顶 |
| `Ctrl-l` | 预览下一个窗口 |
| `Ctrl-v` `Ctrl-t` | 开关预览 / 全屏预览 |
| `Ctrl-/` | 键位提示 |
| `Esc` | 关闭 |

选项：`@tmm-session-key`、`@tmm-window-key`、`@tmm-switch-width`、
`@tmm-switch-height`、`@tmm-bin`，写在 `run-shell` 之前。
