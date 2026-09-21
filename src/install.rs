//! Merge `tmm hook` into a harness's hooks file without replacing what is there.
//!
//! `tmm install-hooks` lets every harness install itself; the ones that keep
//! their hooks in a JSON file share [`merge`]. Existing entries are kept,
//! earlier tmm entries are replaced, and any changed file is backed up first.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::{fzf, harness, paths};

fn is_ours(item: &Value) -> bool {
    item.get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| command.contains("tmm") && command.contains(" hook "))
}

/// Returns whether the file changed.
pub fn merge(path: &Path, events: &[&str], command: &str) -> Result<bool> {
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

/// Merge into `~/<file>` and say what happened; whether the file changed.
pub fn merge_home(label: &str, file: &str, events: &[&str], command: &str) -> Result<bool> {
    let path = paths::home().join(file);
    let changed = merge(&path, events, command)?;
    if changed {
        println!("{label}: hooks written to {}", path.display());
    } else {
        println!("{label}: hooks already in place");
    }
    Ok(changed)
}

/// Every harness installs its own hook, a command baking in the absolute
/// path of this binary: moving the binary means running this again.
pub fn install_hooks() -> Result<()> {
    let base = format!("{} hook ", fzf::me());
    for harness in harness::ALL {
        harness.install(&format!("{base}{}", harness.name()))?;
    }
    println!("Restart the agents to load the hooks.");
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
