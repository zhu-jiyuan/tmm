//! tmm: a personal tmux session and window switcher built on fzf, with
//! agent activity dots.
//!
//! One binary, many small subcommands. `switch` opens the popup, and every
//! fzf key binding calls back into this same binary. Each call is a fresh
//! process, so startup cost and the number of `tmux` invocations per call are
//! what decide how the popup feels.

mod actions;
mod agent;
mod ansi;
mod fzf;
mod install;
mod paths;
mod popup;
mod preview;
mod rows;
mod state;
mod tmux;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "tmm",
    version,
    about = "tmux session and window switcher on fzf, with agent activity dots",
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Open the popup for a tmux client (prefix + s, prefix + w)
    Switch {
        client: String,
        /// Start in windows mode
        #[arg(long)]
        windows: bool,
    },
    /// Switch the open popup between sessions and windows mode (tab)
    Mode { action: actions::ModeAction },
    /// Print the rows; fzf reloads from this
    List,
    /// Reload fzf when the rows changed (runs in the background every second)
    Refresh { snapshot: PathBuf },
    /// Star, unstar or toggle a session (ctrl-s)
    Favorite {
        mode: actions::FavoriteMode,
        name: String,
    },
    /// Render a row's window for the preview pane
    Preview { row: String },
    /// Move a session row's preview to its next window (ctrl-l)
    PreviewNext { row: String },
    /// Take over the popup for a full-size preview (ctrl-t)
    PreviewFull { row: String },
    /// After the full view: replay the action it left behind
    ViewReturn,
    /// Toggle the key legend in the footer (ctrl-/)
    Help,
    /// Open the inline prompt for a row (ctrl-o, ctrl-r)
    Prompt {
        kind: actions::PromptKind,
        id: String,
    },
    /// Close a session or window (ctrl-x)
    Close { id: String },
    /// Enter: answer the open prompt or accept the row
    Enter,
    /// Esc: cancel the open prompt or close the popup
    Esc,
    /// Record an agent lifecycle event (called by Claude Code and Codex hooks)
    Hook {
        provider: String,
        event: Option<String>,
    },
    /// Print per-window agent activity as JSON
    Activity,
    /// Merge the hooks into Claude Code and Codex configuration
    InstallHooks,
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Switch { client, windows } => popup::switch(&client, windows),
        Command::Mode { action } => actions::mode(action),
        Command::List => popup::list(),
        Command::Refresh { snapshot } => popup::refresh(&snapshot),
        Command::Favorite { mode, name } => actions::favorite(mode, &name),
        Command::Preview { row } => preview::run(&row),
        Command::PreviewNext { row } => preview::next(&row),
        Command::PreviewFull { row } => preview::full(&row),
        Command::ViewReturn => preview::view_return(),
        Command::Help => actions::help(),
        Command::Prompt { kind, id } => actions::prompt(kind, &id),
        Command::Close { id } => actions::close(&id),
        Command::Enter => actions::enter(),
        Command::Esc => actions::escape(),
        // Hooks only observe. Whatever goes wrong, an agent must never see a failure.
        Command::Hook { provider, event } => {
            let _ = agent::hook(&provider, event.as_deref());
            Ok(())
        }
        Command::Activity => tmux::panes()
            .and_then(|panes| agent::states(&panes))
            .and_then(|states| {
                println!("{}", serde_json::to_string(&states)?);
                Ok(())
            }),
        Command::InstallHooks => install::install_hooks(),
    };
    if let Err(error) = result {
        eprintln!("tmm: {error:#}");
        std::process::exit(1);
    }
}
