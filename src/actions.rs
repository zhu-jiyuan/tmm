//! What the popup's keys do, one subcommand each. fzf runs them as
//! `transform` or `execute-silent` actions and applies whatever they print.
//!
//! `ctrl-o` and `ctrl-r` never leave the popup: they borrow fzf's query line
//! as the input field. The prompt changes, searching pauses so the list stays
//! put, and a header says what is being asked. Enter applies, Esc cancels,
//! and both put everything back, filter text included. The pending question
//! lives in the `prompt` sidecar so `enter` knows which row it was about.
//!
//! Row ids arrive from fzf's `{1}`, which is empty when nothing matches the
//! filter; tmux would read an empty target as the current session, so those
//! keys do nothing then.

use std::path::PathBuf;
use std::{env, fs};

use anyhow::{Context, Result};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::popup::{self, LEGEND};
use crate::state::Switcher;
use crate::{fzf, paths, tmux};

fn reload() -> String {
    format!("reload-sync({} list)", fzf::me())
}

/// An fzf action argument, wrapped in the first bracket pair the value does
/// not contain.
fn arg(value: &str) -> String {
    let (open, close) = [('(', ')'), ('[', ']'), ('{', '}'), ('<', '>')]
        .into_iter()
        .find(|(_, close)| !value.contains(*close))
        .expect("a name containing every kind of bracket");
    format!("{open}{value}{close}")
}

fn is_window(id: &str) -> bool {
    id.starts_with('@')
}

#[derive(Clone, Copy, ValueEnum)]
pub enum FavoriteMode {
    Star,
    Unstar,
    Toggle,
}

/// `ctrl-s`.
pub fn favorite(mode: FavoriteMode, name: &str) -> Result<()> {
    let mut state = Switcher::load();
    let star = match mode {
        FavoriteMode::Star => true,
        FavoriteMode::Unstar => false,
        FavoriteMode::Toggle => !state.favorites.contains(name),
    };
    if star {
        state.favorites.insert(name.to_string());
    } else {
        state.favorites.remove(name);
    }
    state.save()
}

#[derive(Clone, Copy, ValueEnum)]
pub enum ModeAction {
    Sessions,
    Windows,
    Toggle,
}

/// `tab`: flip the mode, rename the prompt, reload the rows. An open prompt
/// is cancelled, since its question was about the old list.
pub fn mode(action: ModeAction) -> Result<()> {
    let flag = paths::sidecar(&paths::snapshot()?, "windows");
    let windows = match action {
        ModeAction::Sessions => false,
        ModeAction::Windows => true,
        ModeAction::Toggle => !flag.exists(),
    };
    if windows {
        fs::write(&flag, "")?;
    } else if flag.exists() {
        fs::remove_file(&flag)?;
    }
    let back = match take_pending()? {
        Some(pending) => restore(&pending.query)?,
        None => format!("change-prompt({})", popup::base_prompt(windows)),
    };
    println!("{back}+{}", reload());
    Ok(())
}

/// `ctrl-/`: an empty footer hides the legend.
pub fn help() -> Result<()> {
    let flag = paths::sidecar(&paths::snapshot()?, "help");
    if flag.exists() {
        fs::remove_file(&flag)?;
        println!("{LEGEND}");
    } else {
        fs::write(&flag, "")?;
    }
    Ok(())
}

/// Move every client attached to `session` onto another session, so killing
/// it does not take the client (and this popup) down with it.
fn park_clients(session: &str) -> Result<()> {
    let sessions = tmux::sessions()?;
    let Some(fallback) = sessions.iter().find(|id| *id != session) else {
        return Ok(());
    };
    for line in tmux::run(&["list-clients", "-F", "#{client_name}\t#{session_id}"])?.lines() {
        let (client, attached) = line.split_once('\t').unwrap();
        if attached == session {
            tmux::run(&["switch-client", "-c", client, "-t", fallback])?;
        }
    }
    Ok(())
}

fn kill(id: &str) -> Result<()> {
    if is_window(id) {
        // The last window takes its session with it, clients included.
        let info = tmux::run(&[
            "display-message",
            "-p",
            "-t",
            id,
            "#{session_windows}\t#{session_id}",
        ])?;
        let (windows, session) = info.split_once('\t').unwrap();
        if windows == "1" {
            park_clients(session)?;
        }
        tmux::run(&["kill-window", "-t", id])?;
    } else {
        park_clients(id)?;
        tmux::run(&["kill-session", "-t", id])?;
    }
    Ok(())
}

/// `ctrl-x`: close a session or window right away.
pub fn close(id: &str) -> Result<()> {
    if id.is_empty() {
        return Ok(());
    }
    match kill(id) {
        Ok(()) => println!("{}", reload()),
        Err(error) => println!("change-header{}", arg(&format!("tmm: {error}"))),
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptKind {
    Create,
    Rename,
}

impl PromptKind {
    fn prompt(self) -> &'static str {
        match self {
            PromptKind::Create => "create> ",
            PromptKind::Rename => "rename> ",
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Pending {
    kind: PromptKind,
    /// The row the question is about.
    id: String,
    /// The name it had when asked, so an unchanged answer does nothing.
    old: String,
    /// The filter the user had typed, put back when the prompt closes.
    query: String,
}

fn pending_file() -> Result<PathBuf> {
    Ok(paths::sidecar(&paths::snapshot()?, "prompt"))
}

/// The open prompt, if any, removed so it is answered or cancelled once.
fn take_pending() -> Result<Option<Pending>> {
    let file = pending_file()?;
    let Ok(text) = fs::read_to_string(&file) else {
        return Ok(None);
    };
    fs::remove_file(&file)?;
    Ok(Some(serde_json::from_str(&text)?))
}

/// `session_name`, `window_index`, `window_name` of a session or window id.
fn describe(id: &str) -> Result<[String; 3]> {
    let text = tmux::run(&[
        "display-message",
        "-p",
        "-t",
        id,
        "#{session_name}\t#{window_index}\t#{window_name}",
    ])?;
    let fields: Vec<String> = text.splitn(3, '\t').map(str::to_string).collect();
    fields.try_into().ok().context("display-message output")
}

/// Everything back to filtering: prompt, query, header and search.
fn restore(query: &str) -> Result<String> {
    Ok(format!(
        "change-prompt({})+change-query{}+change-header()+enable-search",
        popup::base_prompt(popup::windows_mode()?),
        arg(query)
    ))
}

/// `ctrl-o` and `ctrl-r`: open the inline prompt for a row.
pub fn prompt(kind: PromptKind, id: &str) -> Result<()> {
    if id.is_empty() {
        return Ok(());
    }
    let [session, index, window] = describe(id)?;
    let (old, header) = match kind {
        PromptKind::Rename if is_window(id) => (
            window.clone(),
            format!("Rename window {index}: {window} · Enter / Esc"),
        ),
        PromptKind::Rename => (
            session.clone(),
            format!("Rename session {session} · Enter / Esc"),
        ),
        PromptKind::Create if popup::windows_mode()? => (
            String::new(),
            format!("New window in {session} (name optional) · Enter / Esc"),
        ),
        PromptKind::Create => (String::new(), "New session name · Enter / Esc".to_string()),
    };
    let pending = Pending {
        kind,
        id: id.to_string(),
        old: old.clone(),
        query: env::var("FZF_QUERY")?,
    };
    paths::write_atomic(&pending_file()?, &serde_json::to_string(&pending)?)?;
    println!(
        "change-prompt({})+change-query{}+change-header{}+disable-search",
        kind.prompt(),
        arg(&old),
        arg(&header),
    );
    Ok(())
}

/// Carry out an answered prompt; `false` means there was nothing to do.
fn apply(pending: &Pending, answer: &str) -> Result<bool> {
    let id = pending.id.as_str();
    match pending.kind {
        PromptKind::Rename => {
            if answer.is_empty() || answer == pending.old {
                return Ok(false);
            }
            let command = if is_window(id) {
                "rename-window"
            } else {
                "rename-session"
            };
            tmux::run(&[command, "-t", id, answer])?;
        }
        PromptKind::Create if popup::windows_mode()? => {
            let session = tmux::run(&["display-message", "-p", "-t", id, "#{session_id}"])?;
            let target = format!("{session}:");
            let mut args = vec!["new-window", "-d", "-t", &target];
            if !answer.is_empty() {
                args.extend(["-n", answer]);
            }
            tmux::run(&args)?;
        }
        PromptKind::Create => {
            if answer.is_empty() {
                return Ok(false);
            }
            tmux::run(&["new-session", "-d", "-s", answer])?;
        }
    }
    Ok(true)
}

/// Enter: answer the open prompt, or accept the row.
pub fn enter() -> Result<()> {
    let Some(pending) = take_pending()? else {
        println!("accept");
        return Ok(());
    };
    let answer = env::var("FZF_QUERY")?;
    let restore = restore(&pending.query)?;
    match apply(&pending, answer.trim()) {
        Ok(true) => println!("{restore}+{}", reload()),
        Ok(false) => println!("{restore}"),
        Err(error) => println!("{restore}+change-header{}", arg(&format!("tmm: {error}"))),
    }
    Ok(())
}

/// Esc: cancel the open prompt, or close the popup.
pub fn escape() -> Result<()> {
    match take_pending()? {
        Some(pending) => println!("{}", restore(&pending.query)?),
        None => println!("abort"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_arguments_pick_a_safe_bracket() {
        assert_eq!(arg("plain"), "(plain)");
        assert_eq!(arg("a (b)"), "[a (b)]");
        assert_eq!(arg("a (b) [c]"), "{a (b) [c]}");
    }
}
