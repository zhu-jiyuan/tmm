//! Agent activity: one state per tmux window.
//!
//! Two sources feed it. Lifecycle hooks in Claude Code and Codex call
//! `tmm hook`, which stores a small JSON record on the pane as the tmux user
//! option `@tmm-agent`. Without hooks, the collector falls back to the process
//! tree (is an agent running in this pane?) and the last screen lines (is it
//! waiting for the user?).
//!
//! Cost matters because the switcher refreshes every second. The
//! collector only runs `ps` when at least one pane looks like it could hold an
//! agent, and only captures the screen of panes that do.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::io;
use std::os::unix::process::parent_id;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tmux::{self, Pane};

/// tmux pane option that hooks write and the collector reads.
pub const OPTION: &str = "@tmm-agent";

/// Ordered by priority: waiting beats working beats plain, so the strongest
/// state of a set of panes is simply the maximum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Plain,
    Working,
    Waiting,
}

/// What a hook stores on the pane. `pid` and `started` tie the record to one
/// specific agent process so a stale record cannot describe a reused PID.
#[derive(Serialize, Deserialize)]
pub struct Record {
    pub state: State,
    pub pid: i32,
    pub started: String,
    pub provider: String,
    pub event: String,
}

pub struct Process {
    pub parent: i32,
    pub started: String,
    pub command: String,
}

pub type Table = HashMap<i32, Process>;

/// Only the processes attached to the given terminals (`ttys003`, `pts/2`):
/// everything running inside those panes. Far cheaper than the whole table,
/// because `ps -t` asks the kernel for just those.
pub fn processes_on(ttys: &[&str]) -> Result<Table> {
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

/// Which agent a command line runs, judged by its executable, never by prose in its arguments.
pub fn agent_name(command: &str) -> Option<&'static str> {
    let mut words = command.split_whitespace();
    let first = words.next()?;
    let executable = Path::new(first).file_name()?.to_str()?;
    match executable {
        "codex" => return Some("codex"),
        "claude" => return Some("claude"),
        _ => {}
    }
    if first.contains("/claude/versions/") {
        return Some("claude");
    }
    if executable.starts_with("codex-aarch64-") || executable.starts_with("codex-x86_64-") {
        return Some("codex");
    }
    if matches!(executable, "node" | "bun") {
        let script = words.next()?;
        if script.contains("/@anthropic-ai/claude-code/") {
            return Some("claude");
        }
        if script.contains("/@openai/codex/") {
            return Some("codex");
        }
    }
    None
}

const WAIT_TOOLS: &[&str] = &[
    "askuserquestion",
    "request_user_input",
    "request_user_input_async",
    "enterplanmode",
    "exitplanmode",
];
const WORK_EVENTS: &[&str] = &[
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PreCompact",
    "PostCompact",
];
const WAIT_EVENTS: &[&str] = &[
    "SessionStart",
    "Stop",
    "StopFailure",
    "Interrupt",
    "PermissionRequest",
];
const END_EVENTS: &[&str] = &["SessionEnd"];

/// The state a lifecycle event implies, or `None` when it says nothing about activity.
pub fn event_state(event: &str, payload: &Value) -> Option<State> {
    if END_EVENTS.contains(&event) {
        return Some(State::Plain);
    }
    if event == "Notification" {
        let kind = payload
            .get("notification_type")
            .and_then(Value::as_str)
            .unwrap_or("");
        return matches!(
            kind,
            "permission_prompt" | "idle_prompt" | "elicitation_dialog"
        )
        .then_some(State::Waiting);
    }
    if WAIT_EVENTS.contains(&event) {
        return Some(State::Waiting);
    }
    if WORK_EVENTS.contains(&event) {
        let tool = payload
            .get("tool_name")
            .and_then(Value::as_str)
            .unwrap_or("");
        let tool = tool.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        let waiting = event == "PreToolUse" && WAIT_TOOLS.contains(&tool.as_str());
        return Some(if waiting {
            State::Waiting
        } else {
            State::Working
        });
    }
    None
}

fn waiting_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"usage limit reached|you.ve hit your limit|rate limit exceeded|do you want to proceed|would you like to run|enter to confirm|waiting for (?:your|user)|requires? (?:your )?approval",
        )
        .unwrap()
    })
}

fn working_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"esc(?:ape)? to (?:interrupt|stop)|ctrl[+-]c to interrupt").unwrap()
    })
}

/// Best-effort reading of an agent's own status line. A question or limit
/// message wins over a busy hint; no busy hint at all means it is waiting.
pub fn screen_state(text: &str) -> State {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(14);
    let tail = lines[start..].join("\n").to_lowercase();
    if waiting_pattern().is_match(&tail) {
        State::Waiting
    } else if working_pattern().is_match(&tail) {
        State::Working
    } else {
        State::Waiting
    }
}

/// State of one pane whose process tree starts at `root`.
pub fn pane_state(root: i32, record: &str, table: &Table, capture: impl Fn() -> String) -> State {
    let candidates: HashSet<i32> = descendants(root, table)
        .into_iter()
        .filter(|pid| {
            table
                .get(pid)
                .is_some_and(|p| agent_name(&p.command).is_some())
        })
        .collect();
    if candidates.is_empty() {
        return State::Plain;
    }
    if let Ok(saved) = serde_json::from_str::<Record>(record) {
        let same_process =
            candidates.contains(&saved.pid) && table[&saved.pid].started == saved.started;
        if same_process {
            // A permission hook fires before approval, not when the tool
            // starts. Only a visible busy hint proves the agent has resumed.
            if saved.state == State::Waiting && saved.event == "PermissionRequest" {
                return screen_state(&capture());
            }
            return saved.state;
        }
    }
    screen_state(&capture())
}

/// Whether a pane's foreground command may be, or may be running, an agent.
/// Prefix matches because the kernel truncates command names (`codex-aarch64-ap`).
fn suspect_command(command: &str) -> bool {
    command.starts_with("claude")
        || command.starts_with("codex")
        || matches!(command, "node" | "bun")
}

/// One state per window, keyed by window id. `ps` and `capture-pane` run
/// only for panes that could hold an agent: a hook record is present, or
/// the foreground command looks like one. `TMM_AGENT_SCAN=always` checks
/// every pane.
pub fn states(panes: &[Pane]) -> Result<BTreeMap<String, State>> {
    let always = env::var("TMM_AGENT_SCAN").is_ok_and(|v| v == "always");
    let suspicious = |pane: &Pane| {
        !pane.dead && (always || !pane.agent.is_empty() || suspect_command(&pane.command))
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
            pane_state(pane.pid, &pane.agent, &table, || {
                tmux::run_lossy(&["capture-pane", "-p", "-t", &pane.id, "-S", "-20"])
            })
        } else {
            State::Plain
        };
        let slot = windows
            .entry(pane.window_id.clone())
            .or_insert(State::Plain);
        *slot = (*slot).max(state);
    }
    Ok(windows)
}

/// Called by an agent's hook. Reads the event payload from stdin unless the
/// event name is given, finds the agent process above us, and records the
/// state on the pane. Silent whenever it cannot: hooks must not disturb agents.
pub fn hook(provider: &str, event: Option<&str>) -> Result<()> {
    let pane = env::var("TMUX_PANE").unwrap_or_default();
    if !pane.starts_with('%') || env::var_os("TMUX").is_none() {
        return Ok(());
    }
    let payload: Value = match event {
        Some(_) => Value::Null,
        None => serde_json::from_reader(io::stdin())?,
    };
    // A background subagent must not overwrite the interactive parent's state.
    if payload
        .get("agent_id")
        .is_some_and(|id| !id.is_null() && id.as_str() != Some(""))
    {
        return Ok(());
    }
    let name = event
        .map(str::to_string)
        .or_else(|| {
            payload
                .get("hook_event_name")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    let Some(state) = event_state(&name, &payload) else {
        return Ok(());
    };
    // Everything in this pane shares its terminal, so ask ps for just that.
    let tty = tmux::run(&["display-message", "-p", "-t", &pane, "#{pane_tty}"])?;
    let table = processes_on(&[tty_name(&tty)])?;
    let mut owner = None;
    let mut pid = parent_id() as i32;
    while let Some(process) = table.get(&pid) {
        if agent_name(&process.command).is_some() {
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
        state,
        pid,
        started,
        provider: provider.to_string(),
        event: name,
    };
    tmux::run(&[
        "set-option",
        "-p",
        "-t",
        &pane,
        OPTION,
        &serde_json::to_string(&record)?,
    ])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn events_map_to_states() {
        for event in ["UserPromptSubmit", "PostToolUse", "PostCompact"] {
            assert_eq!(
                event_state(event, &Value::Null),
                Some(State::Working),
                "{event}"
            );
        }
        for event in ["Stop", "Interrupt", "PermissionRequest", "SessionStart"] {
            assert_eq!(
                event_state(event, &Value::Null),
                Some(State::Waiting),
                "{event}"
            );
        }
        assert_eq!(
            event_state(
                "PreToolUse",
                &json!({"tool_name": "functions.request_user_input"})
            ),
            Some(State::Waiting)
        );
        assert_eq!(
            event_state("PreToolUse", &json!({"tool_name": "AskUserQuestion"})),
            Some(State::Waiting)
        );
        assert_eq!(
            event_state("PreToolUse", &json!({"tool_name": "Bash"})),
            Some(State::Working)
        );
        assert_eq!(event_state("SessionEnd", &Value::Null), Some(State::Plain));
        assert_eq!(
            event_state("Notification", &json!({"notification_type": "unrelated"})),
            None
        );
        assert_eq!(
            event_state("Notification", &json!({"notification_type": "idle_prompt"})),
            Some(State::Waiting)
        );
    }

    #[test]
    fn agents_are_recognised_by_executable_not_prose() {
        assert_eq!(agent_name("bash -c 'echo codex working'"), None);
        assert_eq!(
            agent_name("node /usr/lib/node_modules/@openai/codex/bin/codex.js"),
            Some("codex")
        );
        assert_eq!(
            agent_name("/Users/me/.local/share/claude/versions/2.1.263"),
            Some("claude")
        );
        assert_eq!(
            agent_name("/opt/homebrew/bin/codex --full-auto"),
            Some("codex")
        );
        assert_eq!(agent_name(""), None);
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
        let stale = json!({"pid": 10, "started": "old", "state": "working", "provider": "codex", "event": "x"}).to_string();
        assert_eq!(
            pane_state(10, &stale, &table("codex"), || "ready".into()),
            State::Waiting
        );
        assert_eq!(
            pane_state(10, &stale, &table("bash"), || "esc to interrupt".into()),
            State::Plain
        );
        let fresh = json!({"pid": 10, "started": "new", "state": "working", "provider": "codex", "event": "x"}).to_string();
        assert_eq!(
            pane_state(10, &fresh, &table("codex"), || "ready".into()),
            State::Working
        );
    }

    #[test]
    fn screen_hints() {
        assert_eq!(screen_state("Working (esc to interrupt)"), State::Working);
        assert_eq!(
            screen_state("esc to interrupt\nDo you want to proceed?"),
            State::Waiting
        );
        assert_eq!(screen_state("You’ve hit your limit"), State::Waiting);
        assert_eq!(screen_state("$ "), State::Waiting);
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
