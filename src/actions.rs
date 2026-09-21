//! What the popup's keys do, one subcommand each. fzf runs them as
//! `transform` or `execute-silent` actions and applies whatever they print.
//!
//! `ctrl-o` and `ctrl-r` never leave the popup: they borrow fzf's query line
//! as the input field. The prompt changes, searching pauses so the list stays
//! put, and a header says what is being asked. Enter applies, Esc cancels,
//! and both put everything back, filter text included. The pending question
//! lives in the `Prompt` sidecar so `enter` knows which row it was about.
//!
//! Row ids arrive from fzf's `{1}` and go through [`RowId::parse`], which
//! turns an empty id (nothing matched the filter) into "do nothing" and
//! tells a project with no session, which these keys leave alone, from the
//! sessions and windows tmux knows about.

use std::env;

use anyhow::{Context, Result};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::popup::{Mode, Popup, Sidecar};
use crate::rows::RowId;
use crate::state::Switcher;
use crate::switch::{FILTER_FIELDS, LEGEND, TREE_FIELDS};
use crate::{fzf, tmux};

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
    Projects,
    Toggle,
}

/// `tab`: move to the next mode, rename the prompt, reload the rows. An
/// open prompt is cancelled, since its question was about the old list.
pub fn mode(action: ModeAction) -> Result<()> {
    let popup = Popup::current()?;
    let mode = match action {
        ModeAction::Sessions => Mode::Sessions,
        ModeAction::Windows => Mode::Windows,
        ModeAction::Projects => Mode::Projects,
        ModeAction::Toggle => popup.mode().next(),
    };
    popup.set_mode(mode)?;
    let back = match take_pending(&popup)? {
        Some(pending) => restore(&popup, &pending.query),
        None => format!("change-prompt({})", mode.prompt()),
    };
    println!("{back}+{}", reload());
    Ok(())
}

/// `ctrl-/`: an empty footer hides the legend.
pub fn help() -> Result<()> {
    let popup = Popup::current()?;
    if popup.has(Sidecar::Help) {
        popup.remove(Sidecar::Help)?;
        println!("{LEGEND}");
    } else {
        popup.write(Sidecar::Help, "")?;
    }
    Ok(())
}

/// `change`: which name column to show. The tree hides a window's session
/// behind the guides, and fzf cannot match hidden text, so filtering switches
/// to the breadcrumbs. An open prompt borrows the query line; the filter it
/// saved is the one that counts then.
pub fn with_nth() -> Result<()> {
    let popup = Popup::current()?;
    let query = match pending(&popup)? {
        Some(pending) => pending.query,
        None => env::var("FZF_QUERY").unwrap_or_default(),
    };
    let fields = if query.is_empty() {
        TREE_FIELDS
    } else {
        FILTER_FIELDS
    };
    println!("{fields}");
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
        let Some((client, attached)) = line.split_once('\t') else {
            continue;
        };
        if attached == session {
            tmux::run(&["switch-client", "-c", client, "-t", fallback])?;
        }
    }
    Ok(())
}

fn kill(id: &RowId) -> Result<()> {
    match id {
        RowId::Window(window) => {
            // The last window takes its session with it, clients included.
            let info = tmux::run(&[
                "display-message",
                "-p",
                "-t",
                window,
                "#{session_windows}\t#{session_id}",
            ])?;
            let (windows, session) = info.split_once('\t').context("display-message output")?;
            if windows == "1" {
                park_clients(session)?;
            }
            tmux::run(&["kill-window", "-t", window])?;
        }
        RowId::Session(session) => {
            park_clients(session)?;
            tmux::run(&["kill-session", "-t", session])?;
        }
        RowId::Project(_) => {}
    }
    Ok(())
}

/// `ctrl-x`: close a session or window right away.
pub fn close(id: &str) -> Result<()> {
    let Some(id) = RowId::parse(id) else {
        return Ok(());
    };
    if id.target().is_none() {
        return Ok(());
    }
    match kill(&id) {
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

/// The open inline prompt.
#[derive(Serialize, Deserialize)]
struct Pending {
    kind: PromptKind,
    /// The row the question is about.
    id: RowId,
    /// The name it had when asked, so an unchanged answer does nothing.
    old: String,
    /// The filter the user had typed, put back when the prompt closes.
    query: String,
}

fn pending(popup: &Popup) -> Result<Option<Pending>> {
    popup.read_json(Sidecar::Prompt)
}

/// The open prompt, if any, removed so it is answered or cancelled once.
fn take_pending(popup: &Popup) -> Result<Option<Pending>> {
    let pending = pending(popup)?;
    if pending.is_some() {
        popup.remove(Sidecar::Prompt)?;
    }
    Ok(pending)
}

/// `session_name`, `window_index`, `window_name` of a session or window.
fn describe(target: &str) -> Result<[String; 3]> {
    let text = tmux::run(&[
        "display-message",
        "-p",
        "-t",
        target,
        "#{session_name}\t#{window_index}\t#{window_name}",
    ])?;
    let fields: Vec<String> = text.splitn(3, '\t').map(str::to_string).collect();
    fields.try_into().ok().context("display-message output")
}

/// Everything back to filtering: prompt, query, header and search.
fn restore(popup: &Popup, query: &str) -> String {
    format!(
        "change-prompt({})+change-query{}+change-header()+enable-search",
        popup.mode().prompt(),
        arg(query)
    )
}

/// `ctrl-o` and `ctrl-r`: open the inline prompt for a row.
pub fn prompt(kind: PromptKind, id: &str) -> Result<()> {
    let Some(id) = RowId::parse(id) else {
        return Ok(());
    };
    let Some(target) = id.target() else {
        return Ok(());
    };
    let popup = Popup::current()?;
    let [session, index, window] = describe(target)?;
    let (old, header) = match kind {
        PromptKind::Rename if id.is_window() => (
            window.clone(),
            format!("Rename window {index}: {window} · Enter / Esc"),
        ),
        PromptKind::Rename => (
            session.clone(),
            format!("Rename session {session} · Enter / Esc"),
        ),
        PromptKind::Create if popup.mode() == Mode::Windows => (
            String::new(),
            format!("New window in {session} (name optional) · Enter / Esc"),
        ),
        PromptKind::Create => (String::new(), "New session name · Enter / Esc".to_string()),
    };
    let pending = Pending {
        kind,
        id,
        old: old.clone(),
        query: env::var("FZF_QUERY")?,
    };
    popup.write_json(Sidecar::Prompt, &pending)?;
    println!(
        "change-prompt({})+change-query{}+change-header{}+disable-search",
        kind.prompt(),
        arg(&old),
        arg(&header),
    );
    Ok(())
}

/// Carry out an answered prompt; `false` means there was nothing to do.
fn apply(popup: &Popup, pending: &Pending, answer: &str) -> Result<bool> {
    let Some(target) = pending.id.target() else {
        return Ok(false);
    };
    match pending.kind {
        PromptKind::Rename => {
            if answer.is_empty() || answer == pending.old {
                return Ok(false);
            }
            let command = if pending.id.is_window() {
                "rename-window"
            } else {
                "rename-session"
            };
            tmux::run(&[command, "-t", target, answer])?;
        }
        PromptKind::Create if popup.mode() == Mode::Windows => {
            let session = tmux::run(&["display-message", "-p", "-t", target, "#{session_id}"])?;
            let session = format!("{session}:");
            let mut args = vec!["new-window", "-d", "-t", &session];
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
    let popup = Popup::current()?;
    let Some(pending) = take_pending(&popup)? else {
        println!("accept");
        return Ok(());
    };
    let answer = env::var("FZF_QUERY")?;
    let restore = restore(&popup, &pending.query);
    match apply(&popup, &pending, answer.trim()) {
        Ok(true) => println!("{restore}+{}", reload()),
        Ok(false) => println!("{restore}"),
        Err(error) => println!("{restore}+change-header{}", arg(&format!("tmm: {error}"))),
    }
    Ok(())
}

/// Esc: cancel the open prompt, or close the popup.
pub fn escape() -> Result<()> {
    let popup = Popup::current()?;
    match take_pending(&popup)? {
        Some(pending) => println!("{}", restore(&popup, &pending.query)),
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
