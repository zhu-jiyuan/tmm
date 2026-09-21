//! The popup itself: build the fzf command line, run it, act on the choice.
//!
//! Keys follow fzf-lua's habits: typing filters right away, movement is
//! `ctrl-j`/`ctrl-k`, everything else is a control key. Arrows and function
//! keys are avoided on purpose: they need Fn on an HHKB. All of it is fzf
//! `--bind` strings, so the only process alive while the user navigates is
//! fzf itself; each key runs one short `tmm` subcommand (see `actions`).
//!
//! The popup has three modes (see `popup::Mode`): sessions, windows and
//! projects. `prefix + s`, `prefix + w` and `prefix + f` pick the initial
//! one and `tab` cycles.
//!
//! Window rows have two name columns (see `rows`): a tree for browsing and
//! breadcrumbs for filtering. fzf cannot match text it does not show, so the
//! `change` binding swaps the displayed column as the query empties or fills.

use std::env;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

use crate::popup::{Mode, Popup};
use crate::rows::{self, RowId};
use crate::state::Switcher;
use crate::{fzf, projects, tmux};

/// Four short lines: the footer only spans the list half of the popup.
pub(crate) const LEGEND: &str = "ctrl-j/k move · ctrl-f/b page · tab mode\n\
                                 ctrl-o new · ctrl-r rename · ctrl-x close\n\
                                 ctrl-s star · ctrl-l next preview window\n\
                                 ctrl-v preview · ctrl-t full · ctrl-/ help · esc";

/// `--with-nth` while the query is empty (tree) and while filtering (breadcrumbs).
pub(crate) const TREE_FIELDS: &str = "3,5,6";
pub(crate) const FILTER_FIELDS: &str = "4,5,6";

fn message(client: &str, text: &str) {
    let _ = tmux::run(&["display-message", "-c", client, text]);
}

/// The key bindings, each running one `tmm` subcommand. `{1}` is the row
/// id, `{2}` the session name; fzf quotes them for the shell. Keys fzf
/// already binds for editing and moving (ctrl-a/e/u/w, ctrl-j/k/n/p) are
/// left alone.
fn binds(me: &str, snapshot: &Path) -> Vec<String> {
    let cmd = |tail: &str| format!("{me} {tail}");
    let reload = format!("reload-sync({})", cmd("list"));
    vec![
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
    ]
}

pub fn run(client: &str, mode: Mode) -> Result<()> {
    let Some(fzf_bin) = fzf::locate() else {
        message(client, "tmm: fzf not found (brew install fzf)");
        return Ok(());
    };
    if let Err(error) = fzf::check_version(&fzf_bin) {
        message(client, &format!("tmm: {error}"));
        return Ok(());
    }

    let popup = Popup::create()?;
    popup.set_mode(mode)?;
    let text = rows::text(mode)?;
    popup.write_snapshot(&text)?;

    let me = fzf::me();
    let mut command = Command::new(&fzf_bin);
    command.args([
        "--ansi",
        "--reverse",
        "--no-multi",
        "--cycle",
        "--info=inline-right",
        "--print-query",
        "--prompt",
        mode.prompt(),
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
        &format!("{me} preview {{1}}"),
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
    for bind in binds(&me, popup.snapshot()) {
        command.args(["--bind", &bind]);
    }
    command
        .env("SHELL", "/bin/sh")
        .env("TMM_SNAPSHOT", popup.snapshot())
        // The dots carry meaning; never let NO_COLOR blank them.
        .env_remove("NO_COLOR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().context("starting fzf")?;
    // fzf may exit before reading everything; that is not our problem.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }
    let output = child.wait_with_output()?;
    popup.cleanup();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    let query = lines.next().unwrap_or("").trim();
    let selected = lines.next().and_then(|line| line.split('\t').next());
    let target = match choice(output.status.code(), query, selected) {
        Ok(Some(target)) => target,
        Ok(None) => return Ok(()),
        Err(error) => {
            message(client, &format!("tmm: {error}"));
            return Ok(());
        }
    };
    // A window id switches the client to its session and selects the window.
    tmux::run(&["switch-client", "-c", client, "-t", &target])?;
    Ok(())
}

/// What fzf's exit code, query and selected row amount to: a target to
/// switch to, created first when Enter meant "make this".
fn choice(status: Option<i32>, query: &str, selected: Option<&str>) -> Result<Option<String>> {
    match (status, selected.and_then(RowId::parse)) {
        // A project row: its session, made in the directory if need be.
        (Some(0), Some(RowId::Project(path))) => projects::open(&path).map(Some),
        (Some(0), Some(id)) => Ok(Some(id.to_string())),
        // Enter on an empty match list: open the typed directory, else
        // create the typed session.
        (Some(1), _) if !query.is_empty() => {
            let typed = projects::expand(query);
            if (query.starts_with('/') || query.starts_with('~')) && typed.is_dir() {
                projects::open(&typed).map(Some)
            } else {
                tmux::run(&[
                    "new-session",
                    "-d",
                    "-s",
                    query,
                    "-P",
                    "-F",
                    "#{session_id}",
                ])
                .map(Some)
            }
        }
        _ => Ok(None),
    }
}

/// fzf `reload` source. Also refreshes the snapshot so the background
/// refresh compares against what fzf is really showing.
pub fn list() -> Result<()> {
    let popup = Popup::current()?;
    let text = rows::text(popup.mode())?;
    if popup.exists() {
        popup.write_snapshot(&text)?;
    }
    print!("{text}");
    Ok(())
}

/// Background poll. Rows are only replaced when the user has paused, and
/// only if something actually changed, so the cursor never jumps.
pub fn refresh(snapshot: &Path) -> Result<()> {
    let idle: u64 = env::var("FZF_IDLE_TIME_MS")?.parse()?;
    let popup = Popup::at(snapshot);
    if idle < 1000 || !popup.exists() {
        return Ok(());
    }
    let settings = || (Switcher::load(), popup.mode());
    let before = settings();
    let text = rows::text(before.1)?;
    if settings() != before {
        // A key changed the favorites or the mode while we were computing;
        // it reloads on its own, and these rows would flash the old state.
        return Ok(());
    }
    if popup.read_snapshot()? == text {
        return Ok(());
    }
    popup.write_snapshot(&text)?;
    println!(
        "reload-sync(cat {})",
        fzf::quote(&snapshot.to_string_lossy())
    );
    Ok(())
}
