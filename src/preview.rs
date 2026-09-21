//! The preview pane: the selected row's window, redrawn by fzf once a second.
//!
//! A session row previews one of its windows; `ctrl-l` moves on to the next
//! without changing which window tmux considers active. That choice lives in
//! the popup's `Preview` sidecar, so it survives fzf restarting the preview
//! command whenever the cursor moves. A project that is not open previews
//! its directory instead: where it is, its git state, what is in it.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use std::{env, io};

use anyhow::{Context, Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal;

use crate::ansi::{clip_line, fit_frame};
use crate::popup::{Popup, Sidecar};
use crate::rows::RowId;
use crate::{projects, tmux};

const GONE: &str = "\x1b[1mGone: this row no longer exists\x1b[0m";

/// Window ids of a session in index order, and whether each is active.
fn windows_of(session: &str) -> Result<Vec<(String, bool)>> {
    Ok(tmux::run(&[
        "list-windows",
        "-t",
        session,
        "-F",
        "#{window_id}\t#{window_active}",
    ])?
    .lines()
    .filter_map(|line| {
        let (id, active) = line.split_once('\t')?;
        Some((id.to_string(), active == "1"))
    })
    .collect())
}

/// Each session's chosen preview window.
fn choices(popup: &Popup) -> Result<BTreeMap<String, String>> {
    Ok(popup.read_json(Sidecar::Preview)?.unwrap_or_default())
}

/// The chosen window if it still exists, else the active one.
fn current(
    session: &str,
    windows: &[(String, bool)],
    choices: &BTreeMap<String, String>,
) -> Option<String> {
    choices
        .get(session)
        .filter(|id| windows.iter().any(|(w, _)| w == *id))
        .cloned()
        .or_else(|| {
            windows
                .iter()
                .find(|(_, active)| *active)
                .or(windows.first())
                .map(|(id, _)| id.clone())
        })
}

/// The window a row previews: a window row is its own; a session row shows
/// its chosen window.
fn target(popup: &Popup, id: &RowId) -> Result<String> {
    match id {
        RowId::Window(window) => Ok(window.clone()),
        RowId::Session(session) => {
            let windows = windows_of(session)?;
            let chosen = current(session, &windows, &choices(popup)?)
                .context("a session with no windows")?;
            Ok(format!("{session}:{chosen}"))
        }
        RowId::Project(path) => bail!("{} has no window", path.display()),
    }
}

/// `ctrl-l`: move a session row's preview to its next window, wrapping.
pub fn next(row: &str) -> Result<()> {
    let Some(RowId::Session(session)) = RowId::parse(row) else {
        return Ok(());
    };
    let popup = Popup::current()?;
    let windows = windows_of(&session)?;
    let mut choices = choices(&popup)?;
    let Some(now) = current(&session, &windows, &choices) else {
        return Ok(());
    };
    let position = windows.iter().position(|(id, _)| *id == now).unwrap_or(0);
    let following = windows[(position + 1) % windows.len()].0.clone();
    choices.insert(session, following);
    popup.write_json(Sidecar::Preview, &choices)
}

/// One frame: a bold title line, a blank line, then the captured screen,
/// clipped to the viewport so fzf shows no scroll indicator.
fn render(popup: &Popup, id: &RowId, columns: usize, lines: usize) -> String {
    if let RowId::Project(path) = id {
        return directory(path, columns, lines);
    }
    let Ok(target) = target(popup, id) else {
        return GONE.to_string();
    };
    let info = tmux::run_lossy(&[
        "display-message",
        "-p",
        "-t",
        &target,
        "#{session_name}\t#{window_index}\t#{window_name}\t#{window_panes}",
    ]);
    let [name, index, window, panes] = info.split('\t').collect::<Vec<_>>()[..] else {
        return GONE.to_string();
    };
    let mut title = format!("{name}  ·  {index}: {window}");
    if panes != "1" {
        title.push_str(&format!("  ·  {panes} panes"));
    }
    let capture = tmux::run_lossy(&["capture-pane", "-p", "-e", "-t", &target]);
    let frame = fit_frame(&capture, columns, lines.saturating_sub(2));
    let text = format!("\x1b[1m{}\x1b[0m\n\n{frame}", clip_line(&title, columns));
    // An empty capture would leave trailing newlines; fzf counts those as lines.
    text.trim_end_matches('\n').to_string()
}

fn git(path: &Path, args: &[&str]) -> String {
    Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default()
}

/// A project with no session yet: a bold path, the git branch and last
/// commit when there is a repository, then what is inside, directories first.
fn directory(path: &Path, columns: usize, lines: usize) -> String {
    let mut out = vec![
        format!("\x1b[1m{}\x1b[0m", projects::shorten(path)),
        String::new(),
    ];
    if path.join(".git").exists() {
        let branch = git(path, &["symbolic-ref", "--short", "-q", "HEAD"]);
        let last = git(path, &["log", "-1", "--format=%h %s"]);
        if !last.is_empty() {
            let branch = if branch.is_empty() { "HEAD" } else { &branch };
            out.push(format!("\x1b[90m{branch}\x1b[0m {last}"));
            out.push(String::new());
        }
    }
    let mut entries: Vec<(bool, String)> = fs::read_dir(path)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with('.') {
                        return None;
                    }
                    Some((entry.file_type().ok()?.is_dir(), name))
                })
                .collect()
        })
        .unwrap_or_default();
    entries.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(a.1.to_lowercase().cmp(&b.1.to_lowercase()))
    });
    for (dir, name) in entries {
        out.push(if dir {
            format!("\x1b[34m{name}/\x1b[0m")
        } else {
            name
        });
    }
    out.truncate(lines);
    out.iter()
        .map(|line| clip_line(line, columns))
        .collect::<Vec<_>>()
        .join("\n")
}

/// fzf's preview command; fzf re-runs it on a timer, and a process that
/// stayed alive would make it show a spinner.
pub fn run(row: &str) -> Result<()> {
    let Some(id) = RowId::parse(row) else {
        return Ok(());
    };
    let popup = Popup::current()?;
    let size = |name: &str| -> Result<usize> { Ok(env::var(name)?.parse()?) };
    let frame = render(
        &popup,
        &id,
        size("FZF_PREVIEW_COLUMNS")?,
        size("FZF_PREVIEW_LINES")?,
    );
    io::stdout().write_all(frame.as_bytes())?;
    Ok(())
}

/// Restores the terminal even if rendering fails half-way. Dropping the guard
/// runs the cleanup, which is Rust's way of writing `finally`.
struct RawTerminal {
    tty: fs::File,
}

impl RawTerminal {
    fn enter() -> Result<Self> {
        let mut tty = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .context("opening /dev/tty")?;
        terminal::enable_raw_mode()?;
        tty.write_all(b"\x1b[?1049h\x1b[?25l")?;
        Ok(RawTerminal { tty })
    }
}

impl Drop for RawTerminal {
    fn drop(&mut self) {
        let _ = self.tty.write_all(b"\x1b[?25h\x1b[?1049l");
        let _ = terminal::disable_raw_mode();
    }
}

/// `ctrl-t`: fzf suspends its list and we fill the popup with the preview
/// until a key asks to leave. The fzf action to run on return is left in
/// the `View` sidecar for [`view_return`].
pub fn full(row: &str) -> Result<()> {
    let Some(id) = RowId::parse(row) else {
        return Ok(());
    };
    let popup = Popup::current()?;
    let mut terminal = RawTerminal::enter()?;
    let back = loop {
        let (columns, lines) = terminal::size()?;
        let frame = render(&popup, &id, columns as usize, lines as usize).replace('\n', "\r\n");
        write!(terminal.tty, "\x1b[H\x1b[2J{frame}")?;
        terminal.tty.flush()?;
        if !event::poll(Duration::from_secs(1))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match (key.code, key.modifiers) {
            // Only forward: ctrl-h reaches us as Backspace through tmux, so a
            // single wrapping key is the honest option.
            (KeyCode::Char('l'), _) => next(row)?,
            (KeyCode::Char('c'), KeyModifiers::CONTROL) | (KeyCode::Esc, _) => break "abort",
            (KeyCode::Enter, _) => break "accept",
            (KeyCode::Char('v'), KeyModifiers::CONTROL) => break "hide-preview",
            (KeyCode::Char('t'), KeyModifiers::CONTROL) => break "",
            _ => {}
        }
    };
    drop(terminal);
    popup.write(Sidecar::View, back)
}

/// After the full view: replay the action it left behind.
pub fn view_return() -> Result<()> {
    let popup = Popup::current()?;
    println!("{}", popup.take(Sidecar::View)?.unwrap_or_default());
    Ok(())
}
