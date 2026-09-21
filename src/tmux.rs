//! A thin wrapper over the `tmux` command line.
//!
//! Every call here is one subprocess (about 4 ms), so callers pack all the
//! fields they need into a single format string instead of asking twice.
//! Which server we talk to comes from the `TMUX` environment variable, exactly
//! as it would for a user typing `tmux` inside a session.

use std::collections::HashMap;
use std::process::Command;

use anyhow::{Result, bail};

use crate::agent;

/// Run tmux and return its stdout, or an error carrying tmux's stderr.
pub fn run(args: &[&str]) -> Result<String> {
    let output = Command::new("tmux").args(args).output()?;
    if !output.status.success() {
        bail!(
            "tmux {}: {}",
            args[0],
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end_matches('\n')
        .to_string())
}

/// Like [`run`], but a failure (say, a session that just vanished) yields "".
pub fn run_lossy(args: &[&str]) -> String {
    run(args).unwrap_or_default()
}

/// Ids of all sessions on the server.
pub fn sessions() -> Result<Vec<String>> {
    Ok(run(&["list-sessions", "-F", "#{session_id}"])?
        .lines()
        .map(str::to_string)
        .collect())
}

/// One pane with everything the rows and the agent collector need about it,
/// its window and its session.
pub struct Pane {
    pub session_id: String,
    pub session_name: String,
    pub session_attached: bool,
    /// The directory the session was started in.
    pub session_path: String,
    pub window_id: String,
    pub window_index: usize,
    pub window_name: String,
    pub window_active: bool,
    pub window_panes: usize,
    pub id: String,
    pub pid: i32,
    pub dead: bool,
    pub tty: String,
    pub command: String,
    /// The records the harnesses' hooks stored on the pane, by slot; "" when none.
    pub records: HashMap<&'static str, String>,
}

/// Every pane on the server in one call, sessions and windows in tmux's
/// order. The record columns are whatever slots the harnesses declare; names
/// come last because they may contain anything, even tabs.
pub fn panes() -> Result<Vec<Pane>> {
    const HEAD: usize = 11;
    const TAIL: usize = 3;
    let slots = agent::slots();
    let mut format = String::from(
        "#{session_id}\t#{session_attached}\t#{window_id}\t#{window_index}\t#{window_active}\t#{window_panes}\t\
         #{pane_id}\t#{pane_pid}\t#{pane_dead}\t#{pane_tty}\t#{pane_current_command}",
    );
    for slot in &slots {
        format.push_str(&format!("\t#{{{}}}", agent::option(slot)));
    }
    format.push_str("\t#{session_path}\t#{session_name}\t#{window_name}");
    let total = HEAD + slots.len() + TAIL;
    run(&["list-panes", "-a", "-F", &format])?
        .lines()
        .map(|line| {
            let fields: Vec<&str> = line.splitn(total, '\t').collect();
            if fields.len() != total {
                bail!("list-panes: {line}");
            }
            let (head, rest) = fields.split_at(HEAD);
            let (records, tail) = rest.split_at(slots.len());
            let [
                session_id,
                attached,
                window_id,
                window_index,
                window_active,
                window_panes,
                id,
                pid,
                dead,
                tty,
                command,
            ] = head[..]
            else {
                bail!("list-panes: {line}");
            };
            let [session_path, session_name, window_name] = tail[..] else {
                bail!("list-panes: {line}");
            };
            Ok(Pane {
                session_id: session_id.to_string(),
                session_name: session_name.to_string(),
                session_attached: attached != "0",
                session_path: session_path.to_string(),
                window_id: window_id.to_string(),
                window_index: window_index.parse()?,
                window_name: window_name.to_string(),
                window_active: window_active == "1",
                window_panes: window_panes.parse()?,
                id: id.to_string(),
                pid: pid.parse()?,
                dead: dead == "1",
                tty: tty.to_string(),
                command: command.to_string(),
                records: slots
                    .iter()
                    .copied()
                    .zip(records.iter().map(|record| record.to_string()))
                    .collect(),
            })
        })
        .collect()
}
