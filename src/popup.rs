//! The popup: build the fzf command line, run it, act on the choice.
//!
//! Keys follow fzf-lua's habits: typing filters right away, movement is
//! `ctrl-j`/`ctrl-k`, everything else is a control key. Arrows and function
//! keys are avoided on purpose: they need Fn on an HHKB. All of it is fzf
//! `--bind` strings, so the only process alive while the user navigates is
//! fzf itself; each key runs one short `tmm` subcommand (see `actions`).
//!
//! The popup has two modes. Sessions mode lists one row per session. Windows
//! mode lists each session's windows below it. The mode is a per-popup flag
//! kept in a sidecar of the snapshot; `prefix + s` and `prefix + w` pick the
//! initial one and `tab` toggles it.
//!
//! Window rows have two name columns (see `rows`): a tree for browsing and
//! breadcrumbs for filtering. fzf cannot match text it does not show, so the
//! `change` binding swaps the displayed column as the query empties or fills.

use std::io::Write;
use std::path::Path;
use std::process::{self, Command, Stdio};
use std::{env, fs};

use anyhow::{Context, Result};

use crate::state::Switcher;
use crate::{fzf, paths, rows, tmux};

/// Four short lines: the footer only spans the list half of the popup.
pub(crate) const LEGEND: &str = "ctrl-j/k move · ctrl-f/b page · tab windows\n\
                                 ctrl-o new · ctrl-r rename · ctrl-x close\n\
                                 ctrl-s star · ctrl-l next preview window\n\
                                 ctrl-v preview · ctrl-t full · ctrl-/ help · esc";

/// `--with-nth` while the query is empty (tree) and while filtering (breadcrumbs).
pub(crate) const TREE_FIELDS: &str = "3,5,6";
pub(crate) const FILTER_FIELDS: &str = "4,5,6";

/// The prompt names the mode, since tmux cannot retitle an open popup.
pub(crate) fn base_prompt(windows: bool) -> &'static str {
    if windows { "windows> " } else { "sessions> " }
}

/// Whether the current popup is in windows mode: the sidecar exists.
pub(crate) fn windows_mode() -> Result<bool> {
    Ok(paths::sidecar(&paths::snapshot()?, "windows").exists())
}

const SIDECARS: [&str; 5] = ["preview", "help", "view", "windows", "prompt"];

fn cleanup(snapshot: &Path) {
    let _ = fs::remove_file(snapshot);
    for extension in SIDECARS {
        let _ = fs::remove_file(paths::sidecar(snapshot, extension));
    }
}

/// Snapshots left behind by popups that were killed rather than closed.
fn remove_stale(run: &Path) -> Result<()> {
    for entry in fs::read_dir(run)? {
        let path = entry?.path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let Some(pid) = name
            .strip_prefix("switch-")
            .and_then(|n| n.strip_suffix(".txt"))
        else {
            continue;
        };
        // Signal 0 checks whether the process exists without touching it.
        let alive = unsafe { libc::kill(pid.parse()?, 0) } == 0;
        if !alive {
            cleanup(&path);
        }
    }
    Ok(())
}

fn message(client: &str, text: &str) {
    let _ = tmux::run(&["display-message", "-c", client, text]);
}

pub fn switch(client: &str, windows: bool) -> Result<()> {
    let Some(fzf_bin) = fzf::locate() else {
        message(client, "tmm: fzf not found (brew install fzf)");
        return Ok(());
    };
    if let Err(error) = fzf::check_version(&fzf_bin) {
        message(client, &format!("tmm: {error}"));
        return Ok(());
    }

    let run = paths::run_dir()?;
    remove_stale(&run)?;
    let snapshot = run.join(format!("switch-{}.txt", process::id()));
    if windows {
        fs::write(paths::sidecar(&snapshot, "windows"), "")?;
    }
    let text = rows::join(&rows::lines(windows)?);
    paths::write_atomic(&snapshot, &text)?;

    let me = fzf::me();
    let cmd = |tail: &str| format!("{me} {tail}");
    let reload = format!("reload-sync({})", cmd("list"));

    // {1} is the row id (a session `$n` or window `@n`), {2} the session
    // name; fzf quotes them for the shell. Keys fzf already binds for
    // editing and moving (ctrl-a/e/u/w, ctrl-j/k/n/p) are left alone.
    let binds = [
        "ctrl-f:half-page-down".to_string(),
        "ctrl-b:half-page-up".to_string(),
        format!("tab:transform:{}", cmd("mode toggle")),
        format!("ctrl-o:transform:{}", cmd("prompt create {1}")),
        format!("ctrl-r:transform:{}", cmd("prompt rename {1}")),
        format!("ctrl-x:transform:{}", cmd("close {1}")),
        format!(
            "ctrl-s:execute-silent({})+{reload}",
            cmd("favorite toggle {2}")
        ),
        format!(
            "ctrl-l:execute-silent({})+refresh-preview",
            cmd("preview-next {1}")
        ),
        "ctrl-v:toggle-preview".to_string(),
        format!(
            "ctrl-t:execute({})+transform({})",
            cmd("preview-full {1}"),
            cmd("view-return")
        ),
        format!("ctrl-/:transform-footer({})", cmd("help")),
        format!("enter:transform:{}", cmd("enter")),
        format!("esc:transform:{}", cmd("esc")),
        format!("change:transform-with-nth:{}", cmd("with-nth")),
        // One timer for both: redraw the preview (a short-lived process each
        // time, so fzf shows no spinner) and refresh the rows in the background.
        format!(
            "every(1):refresh-preview+bg-transform({})",
            cmd(&format!(
                "refresh {}",
                fzf::quote(&snapshot.to_string_lossy())
            ))
        ),
    ];

    let mut command = Command::new(&fzf_bin);
    command.args([
        "--ansi",
        "--reverse",
        "--no-multi",
        "--cycle",
        "--info=inline-right",
        "--print-query",
        "--prompt",
        base_prompt(windows),
        "--delimiter",
        "\t",
        "--with-nth",
        TREE_FIELDS,
        "--nth",
        "1",
        "--tabstop",
        "1",
        "--track",
        "--id-nth",
        "1",
        "--preview",
        &cmd("preview {1}"),
        "--preview-window",
        "right,50%,border-left,nowrap",
        "--footer",
        LEGEND,
        // No gutter bar: fzf would otherwise paint every row's first column.
        "--gutter",
        " ",
        "--color",
        &fzf::colours().join(","),
    ]);
    for bind in &binds {
        command.args(["--bind", bind]);
    }
    command
        .env("SHELL", "/bin/sh")
        .env("TMM_SNAPSHOT", &snapshot)
        // The dots carry meaning; never let NO_COLOR blank them.
        .env_remove("NO_COLOR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().context("starting fzf")?;
    // fzf may exit before reading everything; that is not our problem.
    let _ = child.stdin.take().unwrap().write_all(text.as_bytes());
    let output = child.wait_with_output()?;
    cleanup(&snapshot);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    let query = lines.next().unwrap_or("").trim();
    let selected = lines.next().and_then(|line| line.split('\t').next());
    let target = match (output.status.code(), selected) {
        (Some(0), Some(id)) => id.to_string(),
        // Enter on an empty match list: create the typed session.
        (Some(1), _) if !query.is_empty() => {
            match tmux::run(&[
                "new-session",
                "-d",
                "-s",
                query,
                "-P",
                "-F",
                "#{session_id}",
            ]) {
                Ok(id) => id,
                Err(error) => {
                    message(client, &format!("tmm: {error}"));
                    return Ok(());
                }
            }
        }
        _ => return Ok(()),
    };
    // A window id switches the client to its session and selects the window.
    tmux::run(&["switch-client", "-c", client, "-t", &target])?;
    Ok(())
}

/// fzf `reload` source. Also refreshes the snapshot so the background
/// refresh compares against what fzf is really showing.
pub fn list() -> Result<()> {
    let text = rows::join(&rows::lines(windows_mode()?)?);
    let snapshot = paths::snapshot()?;
    if snapshot.exists() {
        paths::write_atomic(&snapshot, &text)?;
    }
    print!("{text}");
    Ok(())
}

/// Background poll. Rows are only replaced when the user has paused, and
/// only if something actually changed, so the cursor never jumps.
pub fn refresh(snapshot: &Path) -> Result<()> {
    let idle: u64 = env::var("FZF_IDLE_TIME_MS")?.parse()?;
    if idle < 1000 || !snapshot.exists() {
        return Ok(());
    }
    let settings = || -> Result<_> { Ok((Switcher::load(), windows_mode()?)) };
    let before = settings()?;
    let text = rows::join(&rows::lines(before.1)?);
    if settings()? != before {
        // A key changed the favorites or the mode while we were computing;
        // it reloads on its own, and these rows would flash the old state.
        return Ok(());
    }
    if fs::read_to_string(snapshot)? == text {
        return Ok(());
    }
    paths::write_atomic(snapshot, &text)?;
    println!(
        "reload-sync(cat {})",
        fzf::quote(&snapshot.to_string_lossy())
    );
    Ok(())
}
