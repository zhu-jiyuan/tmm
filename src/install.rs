//! Merge `tmm hook` into the agents' configuration without replacing what is there.
//!
//! Claude Code and Codex read hook lists from JSON files. Existing entries
//! are kept, earlier tmm entries are replaced, and any changed file is
//! backed up first.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::{fzf, paths};

const COMMON: &[&str] = &[
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PermissionRequest",
    "Stop",
    "PreCompact",
];

fn is_ours(item: &Value) -> bool {
    item.get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| command.contains("tmm") && command.contains(" hook "))
}

/// Returns whether the file changed.
fn merge(path: &Path, events: &[&str], command: &str) -> Result<bool> {
    let mut data: Value = match fs::read_to_string(path) {
        Ok(text) => {
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?
        }
        Err(_) => json!({}),
    };
    let Some(root) = data.as_object_mut() else {
        bail!("{} is not a JSON object", path.display())
    };
    let original = Value::Object(root.clone());
    let hooks = root.entry("hooks").or_insert_with(|| json!({}));
    let Some(hooks) = hooks.as_object_mut() else {
        bail!("\"hooks\" in {} is not an object", path.display())
    };
    for event in events {
        let entries = hooks.entry(*event).or_insert_with(|| json!([]));
        let Some(entries) = entries.as_array_mut() else {
            continue;
        };
        for entry in entries.iter_mut() {
            if let Some(items) = entry.get_mut("hooks").and_then(Value::as_array_mut) {
                items.retain(|item| !is_ours(item));
            }
        }
        entries.retain(|entry| {
            entry
                .get("hooks")
                .and_then(Value::as_array)
                .is_some_and(|items| !items.is_empty())
        });
        entries.push(json!({"hooks": [{"type": "command", "command": command, "timeout": 5}]}));
    }
    if data == original {
        return Ok(false);
    }
    if path.exists() {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let backup = path.with_file_name(format!(
            "{}.before-tmm-{stamp}",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
        fs::copy(path, &backup)?;
        println!("backed up {} to {}", path.display(), backup.display());
    }
    paths::write_atomic(path, &(serde_json::to_string_pretty(&data)? + "\n"))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(true)
}

pub fn install_hooks() -> Result<()> {
    let base = format!("{} hook ", fzf::me());
    let home = paths::home();

    let claude: Vec<&str> = COMMON
        .iter()
        .copied()
        .chain(["PostToolUseFailure", "StopFailure", "Notification"])
        .collect();
    merge(
        &home.join(".claude/settings.json"),
        &claude,
        &format!("{base}claude"),
    )?;

    let codex: Vec<&str> = COMMON
        .iter()
        .copied()
        .chain(["Interrupt", "PostCompact"])
        .collect();
    merge(
        &home.join(".codex/hooks.json"),
        &codex,
        &format!("{base}codex"),
    )?;

    println!("Installed Claude Code and Codex hooks.");
    println!("Restart the agents to load them. Review the new hooks in Codex's /hooks UI.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::env;

    use super::*;

    #[test]
    fn merge_keeps_foreign_hooks_and_replaces_ours() {
        let dir = env::temp_dir().join(format!("tmm-install-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        fs::write(
            &path,
            r#"{"other": 1, "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "theirs"}, {"type": "command", "command": "/old/tmm hook claude"}]}]}}"#,
        )
        .unwrap();
        assert!(merge(&path, &["Stop", "SessionStart"], "'/new/tmm' hook claude").unwrap());
        let data: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(data["other"], 1);
        let stop = data["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0]["hooks"][0]["command"], "theirs");
        assert_eq!(stop[1]["hooks"][0]["command"], "'/new/tmm' hook claude");
        assert_eq!(
            data["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            "'/new/tmm' hook claude"
        );
        assert!(
            !merge(&path, &["Stop", "SessionStart"], "'/new/tmm' hook claude").unwrap(),
            "second run is a no-op"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
