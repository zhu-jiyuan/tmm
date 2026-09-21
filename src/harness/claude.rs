//! Claude Code.

use std::sync::OnceLock;

use anyhow::Result;
use regex::Regex;

use super::{CommandLine, Harness, Hook, MAIN, Records, Update, read_screen};
use crate::agent::State;
use crate::{install, paths};

pub struct Claude;

const HOOKS_FILE: &str = ".claude/settings.json";
const EVENTS: &[&str] = &[
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "Notification",
    "Stop",
    "StopFailure",
    "PreCompact",
];
/// Tools whose `PreToolUse` means the agent is asking the user something.
const WAIT_TOOLS: &[&str] = &["askuserquestion", "enterplanmode", "exitplanmode"];
/// The notifications that ask for the user, out of the many kinds sent.
const WAIT_NOTIFICATIONS: &[&str] = &["permission_prompt", "idle_prompt", "elicitation_dialog"];

impl Harness for Claude {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn runs(&self, command: &str) -> bool {
        let Some(command) = CommandLine::parse(command) else {
            return false;
        };
        command.executable == "claude"
            || command.path.contains("/claude/versions/")
            || command.runs_script(&["node", "bun"], "/@anthropic-ai/claude-code/")
    }

    fn suspect(&self, command: &str) -> bool {
        command.starts_with("claude") || matches!(command, "node" | "bun")
    }

    fn install(&self, command: &str) -> Result<()> {
        let path = paths::home().join(HOOKS_FILE);
        if install::merge(&path, EVENTS, command)? {
            println!("Claude Code: hooks written to {}", path.display());
        } else {
            println!("Claude Code: hooks already in place");
        }
        Ok(())
    }

    fn hook(&self, event: Option<&str>, stdin: &str) -> Option<Update> {
        let hook = Hook::parse(event, stdin);
        // Hooks fired inside a subagent carry its id. A background subagent
        // must not overwrite the interactive parent's state.
        if !hook.string("agent_id").is_empty() {
            return None;
        }
        let state = match hook.event.as_str() {
            "SessionEnd" => State::Plain,
            "SessionStart" | "Stop" | "StopFailure" | "PermissionRequest" => State::Waiting,
            "Notification" if WAIT_NOTIFICATIONS.contains(&hook.string("notification_type")) => {
                State::Waiting
            }
            "PreToolUse" => {
                let tool = hook.string("tool_name").to_ascii_lowercase();
                if WAIT_TOOLS.contains(&tool.as_str()) {
                    State::Waiting
                } else {
                    State::Working
                }
            }
            "UserPromptSubmit" | "PostToolUse" | "PostToolUseFailure" | "PreCompact" => {
                State::Working
            }
            _ => return None,
        };
        Some(Update {
            slot: MAIN,
            state,
            event: hook.event,
        })
    }

    fn pane_state(&self, records: &Records, screen: &dyn Fn() -> String) -> State {
        match records.get(MAIN) {
            // A permission hook fires before approval, not when the tool
            // starts. Only a visible busy hint proves the agent has resumed.
            Some(record)
                if record.state == State::Waiting && record.event == "PermissionRequest" =>
            {
                screen_state(&screen())
            }
            Some(record) => record.state,
            None => screen_state(&screen()),
        }
    }
}

/// Claude Code's own status line: its permission and plan prompts, its
/// limit messages, and the busy hint it shows while a turn runs.
fn screen_state(screen: &str) -> State {
    static WAITING: OnceLock<Regex> = OnceLock::new();
    static WORKING: OnceLock<Regex> = OnceLock::new();
    let waiting = WAITING.get_or_init(|| {
        Regex::new(
            r"do you want to proceed|enter to confirm|waiting for (?:your|user)|you.ve hit your limit|usage limit reached|rate limit exceeded",
        )
        .unwrap()
    });
    let working = WORKING.get_or_init(|| {
        Regex::new(r"esc(?:ape)? to (?:interrupt|stop)|ctrl[+-]c to interrupt").unwrap()
    });
    read_screen(screen, waiting, working)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::Record;

    fn state(event: Option<&str>, stdin: &str) -> Option<State> {
        Claude.hook(event, stdin).map(|update| update.state)
    }

    #[test]
    fn events_map_to_states() {
        for event in ["UserPromptSubmit", "PostToolUse", "PreCompact"] {
            assert_eq!(state(Some(event), ""), Some(State::Working), "{event}");
        }
        for event in ["Stop", "StopFailure", "PermissionRequest", "SessionStart"] {
            assert_eq!(state(Some(event), ""), Some(State::Waiting), "{event}");
        }
        assert_eq!(state(Some("SessionEnd"), ""), Some(State::Plain));
        assert_eq!(
            state(Some("Interrupt"), ""),
            None,
            "not a Claude Code event"
        );
        assert_eq!(
            state(
                None,
                r#"{"hook_event_name":"PreToolUse","tool_name":"AskUserQuestion"}"#
            ),
            Some(State::Waiting)
        );
        assert_eq!(
            state(
                None,
                r#"{"hook_event_name":"PreToolUse","tool_name":"Bash"}"#
            ),
            Some(State::Working)
        );
        assert_eq!(
            state(
                None,
                r#"{"hook_event_name":"Notification","notification_type":"unrelated"}"#
            ),
            None
        );
        assert_eq!(
            state(
                None,
                r#"{"hook_event_name":"Notification","notification_type":"idle_prompt"}"#
            ),
            Some(State::Waiting)
        );
        assert_eq!(
            state(
                None,
                r#"{"hook_event_name":"PostToolUse","agent_id":"sub-1"}"#
            ),
            None,
            "a subagent's hooks say nothing about the parent"
        );
        let update = Claude.hook(Some("Stop"), "").unwrap();
        assert_eq!((update.slot, update.event.as_str()), (MAIN, "Stop"));
    }

    #[test]
    fn records_settle_the_state_unless_a_permission_is_pending() {
        let record = |state: State, event: &str| {
            Records::from([(
                MAIN,
                Record {
                    state,
                    pid: 1,
                    started: "x".into(),
                    harness: "claude".into(),
                    event: event.into(),
                },
            )])
        };
        let busy = || "Working (esc to interrupt)".to_string();
        assert_eq!(
            Claude.pane_state(&record(State::Waiting, "Stop"), &busy),
            State::Waiting,
            "a record wins over the screen"
        );
        assert_eq!(
            Claude.pane_state(&record(State::Waiting, "PermissionRequest"), &busy),
            State::Working,
            "a busy hint after a permission hook means it resumed"
        );
        assert_eq!(
            Claude.pane_state(&record(State::Waiting, "PermissionRequest"), &|| "❯ "
                .into()),
            State::Waiting
        );
        assert_eq!(
            Claude.pane_state(&Records::new(), &busy),
            State::Working,
            "no record: the screen decides"
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
}
