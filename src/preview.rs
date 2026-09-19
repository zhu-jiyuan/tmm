//! The preview pane: the selected row's window, redrawn by fzf once a second.
//!
//! A session row previews one of its windows; `ctrl-l` moves on to the next
//! without changing which window tmux considers active. That choice lives in
//! a sidecar of the popup's snapshot, so it survives fzf restarting the
//! preview command whenever the cursor moves.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;
use std::{env, io};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal;

use crate::ansi::{clip_line, fit_frame};
use crate::{paths, tmux};

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
    .map(|line| {
        let (id, active) = line.split_once('\t').unwrap();
        (id.to_string(), active == "1")
    })
    .collect())
}

/// Each session's chosen preview window, from the `preview` sidecar.
fn choices() -> Result<(PathBuf, BTreeMap<String, String>)> {
    let path = paths::sidecar(&paths::snapshot()?, "preview");
    let choices = match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)?,
        Err(_) => BTreeMap::new(),
    };
    Ok((path, choices))
}

/// The chosen window if it still exists, else the active one.
fn current(
    session: &str,
    windows: &[(String, bool)],
    choices: &BTreeMap<String, String>,
) -> String {
    choices
        .get(session)
        .filter(|id| windows.iter().any(|(w, _)| w == *id))
        .cloned()
        .unwrap_or_else(|| {
            windows
                .iter()
                .find(|(_, active)| *active)
                .unwrap()
                .0
                .clone()
        })
}

/// The window a row previews: a window row is its own; a session row shows
/// its chosen window.
fn target(row: &str) -> Result<String> {
    if row.starts_with('@') {
        return Ok(row.to_string());
    }
    let windows = windows_of(row)?;
    let (_, choices) = choices()?;
    Ok(format!("{row}:{}", current(row, &windows, &choices)))
}

/// `ctrl-l`: move a session row's preview to its next window, wrapping.
pub fn next(row: &str) -> Result<()> {
    if row.is_empty() || row.starts_with('@') {
        return Ok(());
    }
    let windows = windows_of(row)?;
    let (path, mut choices) = choices()?;
    let now = current(row, &windows, &choices);
    let position = windows.iter().position(|(id, _)| *id == now).unwrap();
    let following = windows[(position + 1) % windows.len()].0.clone();
    choices.insert(row.to_string(), following);
    paths::write_atomic(&path, &serde_json::to_string(&choices)?)
}

/// One frame: a bold title line, a blank line, then the captured screen,
/// clipped to the viewport so fzf shows no scroll indicator.
fn render(row: &str, columns: usize, lines: usize) -> String {
    let Ok(target) = target(row) else {
        return "\x1b[1mGone: this row no longer exists\x1b[0m".to_string();
    };
    let info = tmux::run_lossy(&[
        "display-message",
        "-p",
        "-t",
        &target,
        "#{session_name}\t#{window_index}\t#{window_name}\t#{window_panes}",
    ]);
    let [name, index, window, panes] = info.split('\t').collect::<Vec<_>>()[..] else {
        return "\x1b[1mGone: this row no longer exists\x1b[0m".to_string();
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

/// fzf's preview command; fzf re-runs it on a timer, and a process that
/// stayed alive would make it show a spinner.
pub fn run(row: &str) -> Result<()> {
    if row.is_empty() {
        return Ok(());
    }
    let size = |name: &str| -> Result<usize> { Ok(env::var(name)?.parse()?) };
    let frame = render(
        row,
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
/// until a key asks to leave. The fzf action to run on return is left in the
/// `view` sidecar for [`view_return`].
pub fn full(row: &str) -> Result<()> {
    if row.is_empty() {
        return Ok(());
    }
    let mut terminal = RawTerminal::enter()?;
    let back = loop {
        let (columns, lines) = terminal::size()?;
        let frame = render(row, columns as usize, lines as usize).replace('\n', "\r\n");
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
    fs::write(paths::sidecar(&paths::snapshot()?, "view"), back)?;
    Ok(())
}

/// After the full view: replay the action it left behind.
pub fn view_return() -> Result<()> {
    let file = paths::sidecar(&paths::snapshot()?, "view");
    let back = fs::read_to_string(&file)?;
    fs::remove_file(&file)?;
    println!("{back}");
    Ok(())
}
