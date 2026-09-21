//! Agent activity: one state per tmux window.
//!
//! Two sources feed it. The harnesses' lifecycle hooks call `tmm hook`, which
//! stores a small JSON record on the pane as a tmux user option. Without
//! hooks, or when the record is stale, the last screen lines say whether the
//! agent is waiting. What a hook event or a screen means belongs to the
//! harness that produced it (`harness.rs`); this file finds the processes,
//! keeps the records and merges the answers.
//!
//! Cost matters because the switcher refreshes every second. The collector
//! only runs `ps` when at least one pane looks like it could hold an agent,
//! and only captures the screen of panes that do.

use std::cell::OnceCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::io::{self, Read};
use std::os::unix::process::parent_id;
use std::process::Command;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::harness::{self, Harness, MAIN, Record, Records};
use crate::tmux::{self, Pane};

/// The tmux pane option holding a record slot.
pub fn option(slot: &str) -> String {
    if slot == MAIN {
        "@tmm-agent".to_string()
    } else {
        format!("@tmm-agent-{slot}")
    }
}

/// Every slot some harness writes, `main` first and each once: the record
/// columns `tmux::panes` fetches.
pub fn slots() -> Vec<&'static str> {
    let mut slots = vec![MAIN];
    for harness in harness::ALL {
        for slot in harness.slots() {
            if !slots.contains(slot) {
                slots.push(slot);
            }
        }
    }
    slots
}

/// Ordered by priority: waiting beats working beats plain, so the strongest
/// state of a set of panes is simply the maximum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Plain,
    Working,
    Waiting,
}

struct Process {
    parent: i32,
    started: String,
    command: String,
}

type Table = HashMap<i32, Process>;

/// Only the processes attached to the given terminals (`ttys003`, `pts/2`):
/// everything running inside those panes. Far cheaper than the whole table,
/// because `ps -t` asks the kernel for just those.
fn processes_on(ttys: &[&str]) -> Result<Table> {
    if ttys.is_empty() {
        return Ok(Table::new());
    }
    let output = Command::new("ps")
        .args(["-t", &ttys.join(","), "-o", "pid=,ppid=,lstart=,command="])
        .output()
        .context("running ps")?;
    // ps exits non-zero when a terminal has no processes; the output is still fine.
    Ok(parse_ps(&String::from_utf8_lossy(&output.stdout)))
}

/// `/dev/ttys003` as `ps -t` wants it.
fn tty_name(tty: &str) -> &str {
    tty.strip_prefix("/dev/").unwrap_or(tty)
}

fn parse_ps(text: &str) -> Table {
    let mut table = Table::new();
    for line in text.lines() {
        // pid ppid <5 words of lstart> command...
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 8 {
            continue;
        }
        let (Ok(pid), Ok(parent)) = (fields[0].parse(), fields[1].parse()) else {
            continue;
        };
        table.insert(
            pid,
            Process {
                parent,
                started: fields[2..7].join(" "),
                command: fields[7..].join(" "),
            },
        );
    }
    table
}

/// `root` and everything below it.
fn descendants(root: i32, table: &Table) -> HashSet<i32> {
    let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
    for (pid, process) in table {
        children.entry(process.parent).or_default().push(*pid);
    }
    let mut found = HashSet::new();
    let mut pending = vec![root];
    while let Some(pid) = pending.pop() {
        if found.insert(pid)
            && let Some(kids) = children.get(&pid)
        {
            pending.extend(kids);
        }
    }
    found
}

/// State of one pane whose process tree starts at `root`, from the records
/// stored on it by slot. Each harness found in the tree reads its own live
/// records: a record is live when its pid runs that harness and its start
/// time matches, so a reused PID cannot resurrect a stale one. The screen is
/// captured at most once, and only if a harness asks for it.
fn pane_state(
    root: i32,
    records: &HashMap<&'static str, String>,
    table: &Table,
    capture: impl Fn() -> String,
) -> State {
    let agents: HashMap<i32, &'static dyn Harness> = descendants(root, table)
        .into_iter()
        .filter_map(|pid| harness::detect(&table.get(&pid)?.command).map(|h| (pid, h)))
        .collect();
    if agents.is_empty() {
        return State::Plain;
    }
    let cell = OnceCell::new();
    let screen = || cell.get_or_init(&capture).clone();
    let mut seen = HashSet::new();
    let mut state = State::Plain;
    for harness in agents.values() {
        if !seen.insert(harness.name()) {
            continue;
        }
        let live: Records = harness
            .slots()
            .iter()
            .filter_map(|slot| {
                let saved: Record = serde_json::from_str(records.get(slot)?).ok()?;
                let owner = agents.get(&saved.pid)?;
                let process = table.get(&saved.pid)?;
                (owner.name() == harness.name() && process.started == saved.started)
                    .then_some((*slot, saved))
            })
            .collect();
        state = state.max(harness.pane_state(&live, &screen));
    }
    state
}

/// One state per window, keyed by window id. `ps` and `capture-pane` run
/// only for panes that could hold an agent: a hook record is present, or
/// some harness accepts the foreground command. `TMM_AGENT_SCAN=always`
/// checks every pane.
pub fn states(panes: &[Pane]) -> Result<BTreeMap<String, State>> {
    let always = env::var("TMM_AGENT_SCAN").is_ok_and(|v| v == "always");
    let suspicious = |pane: &Pane| {
        !pane.dead
            && (always
                || pane.records.values().any(|record| !record.is_empty())
                || harness::suspect(&pane.command))
    };
    let ttys: Vec<&str> = panes
        .iter()
        .filter(|pane| suspicious(pane))
        .map(|pane| tty_name(&pane.tty))
        .collect();
    let table = processes_on(&ttys)?;

    let mut windows = BTreeMap::new();
    for pane in panes {
        let state = if suspicious(pane) {
            pane_state(pane.pid, &pane.records, &table, || {
                tmux::run_lossy(&["capture-pane", "-p", "-t", &pane.id, "-S", "-20"])
            })
        } else {
            State::Plain
        };
        let window = windows
            .entry(pane.window_id.clone())
            .or_insert(State::Plain);
        *window = (*window).max(state);
    }
    Ok(windows)
}

/// Called by a harness's hook. Asks the harness what the call means, finds
/// the harness's process above us, and records the state on the pane.
/// Silent whenever it cannot: hooks must not disturb agents.
pub fn hook(name: &str, event: Option<&str>) -> Result<()> {
    let pane = env::var("TMUX_PANE").unwrap_or_default();
    if !pane.starts_with('%') || env::var_os("TMUX").is_none() {
        return Ok(());
    }
    let Some(harness) = harness::by_name(name) else {
        return Ok(());
    };
    let mut stdin = String::new();
    if event.is_none() {
        io::stdin().read_to_string(&mut stdin)?;
    }
    let Some(update) = harness.hook(event, &stdin) else {
        return Ok(());
    };
    // Everything in this pane shares its terminal, so ask ps for just that.
    let tty = tmux::run(&["display-message", "-p", "-t", &pane, "#{pane_tty}"])?;
    let table = processes_on(&[tty_name(&tty)])?;
    let mut owner = None;
    let mut pid = parent_id() as i32;
    while let Some(process) = table.get(&pid) {
        if harness.runs(&process.command) {
            owner = Some((pid, process.started.clone()));
            break;
        }
        if process.parent == pid {
            break;
        }
        pid = process.parent;
    }
    let Some((pid, started)) = owner else {
        return Ok(());
    };
    let record = Record {
        state: update.state,
        pid,
        started,
        harness: name.to_string(),
        event: update.event,
    };
    tmux::run(&[
        "set-option",
        "-p",
        "-t",
        &pane,
        &option(update.slot),
        &serde_json::to_string(&record)?,
    ])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn slots_are_options_on_the_pane() {
        assert_eq!(option(MAIN), "@tmm-agent");
        assert_eq!(option("subagent"), "@tmm-agent-subagent");
        let slots = slots();
        assert_eq!(slots[0], MAIN);
        let unique: HashSet<&str> = slots.iter().copied().collect();
        assert_eq!(unique.len(), slots.len(), "each slot once: {slots:?}");
    }

    #[test]
    fn stale_records_fall_back_to_the_screen() {
        let table = |command: &str| {
            let mut t = Table::new();
            t.insert(
                10,
                Process {
                    parent: 1,
                    started: "new".into(),
                    command: command.into(),
                },
            );
            t
        };
        let records = |text: &str| HashMap::from([(MAIN, text.to_string())]);
        let stale = json!({"pid": 10, "started": "old", "state": "working", "harness": "codex", "event": "x"}).to_string();
        assert_eq!(
            pane_state(10, &records(&stale), &table("codex"), || "ready".into()),
            State::Waiting
        );
        assert_eq!(
            pane_state(10, &records(&stale), &table("bash"), || {
                "esc to interrupt".into()
            }),
            State::Plain,
            "no harness process, no state"
        );
        let fresh = json!({"pid": 10, "started": "new", "state": "working", "harness": "codex", "event": "x"}).to_string();
        assert_eq!(
            pane_state(10, &records(&fresh), &table("codex"), || "ready".into()),
            State::Working
        );
        assert_eq!(
            pane_state(10, &HashMap::new(), &table("codex"), || "ready".into()),
            State::Waiting,
            "no record: the harness reads the screen"
        );
    }

    #[test]
    fn ps_output_parses() {
        let table = parse_ps(
            "  123     1 Fri Sep  9 21:47:00 2026 /bin/zsh -l\n 99 123 Fri Sep  9 21:48:00 2026 codex\nbad line\n",
        );
        assert_eq!(table[&123].started, "Fri Sep 9 21:47:00 2026");
        assert_eq!(table[&99].parent, 123);
        assert_eq!(descendants(123, &table), HashSet::from([123, 99]));
    }
}
