//! Locating fzf and translating tmux's colours into fzf's.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, bail};

use crate::tmux;

/// `bg-transform`, `every()`, `--footer`, `--gutter`, `--id-nth`, `strip`.
const MIN_VERSION: (u32, u32, u32) = (0, 74, 3);

/// fzf on `PATH`, else in the usual Homebrew places (tmux's own PATH can be short).
pub fn locate() -> Option<PathBuf> {
    let on_path = env::var_os("PATH")
        .into_iter()
        .flat_map(|path| env::split_paths(&path).collect::<Vec<_>>());
    on_path
        .map(|dir| dir.join("fzf"))
        .chain(["/opt/homebrew/bin/fzf", "/usr/local/bin/fzf"].map(PathBuf::from))
        .find(|candidate| candidate.is_file())
}

pub fn check_version(fzf: &Path) -> Result<()> {
    let output = Command::new(fzf).arg("--version").output()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let first = text.split_whitespace().next().unwrap_or("");
    let parts: Vec<u32> = first
        .split('.')
        .take(3)
        .filter_map(|part| part.parse().ok())
        .collect();
    let version = match parts[..] {
        [major, minor, patch] => (major, minor, patch),
        _ => bail!("cannot read fzf version from {first:?}"),
    };
    if version < MIN_VERSION {
        bail!("update fzf to 0.74.3+ (found {first})");
    }
    Ok(())
}

/// Single-quote for `/bin/sh`, the shell fzf runs our commands with.
pub fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// This binary, quoted, for the commands fzf runs on each key.
pub fn me() -> String {
    quote(&env::current_exe().expect("own path").to_string_lossy())
}

/// A tmux colour name as fzf spells it, or `None` for the terminal default.
pub fn colour_name(value: &str) -> Option<String> {
    let value = value.to_ascii_lowercase();
    if matches!(value.as_str(), "" | "default" | "terminal") {
        return None;
    }
    if let Some(n) = value
        .strip_prefix("colour")
        .or_else(|| value.strip_prefix("color"))
        && !n.is_empty()
        && n.bytes().all(|b| b.is_ascii_digit())
    {
        return Some(n.to_string());
    }
    if let Some(base) = value.strip_prefix("bright") {
        return Some(format!("bright-{base}"));
    }
    Some(value)
}

/// `--color` entries painting the current line like the tmux status bar.
///
/// `strip` drops the badge's own colour on the highlighted row, and `nth`
/// keeps the name and dots as they are, so the activity colours survive.
pub fn colours() -> Vec<String> {
    let style = tmux::run_lossy(&["display-message", "-p", "#{status-style}"]);
    let bg = style
        .split(',')
        .find_map(|part| part.strip_prefix("bg="))
        .and_then(colour_name);
    let mut colours = vec!["gutter:-1".to_string(), "nth:regular".to_string()];
    match bg {
        Some(bg) => colours.extend([
            "fg+:black:regular:strip".to_string(),
            format!("bg+:{bg}"),
            format!("hl:{bg}"),
            format!("pointer:{bg}"),
            format!("prompt:{bg}"),
            "hl+:black:underline".to_string(),
        ]),
        None => colours.push("fg+:-1:regular:strip".to_string()),
    }
    colours
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_names() {
        let name = |c: &str| colour_name(c);
        assert_eq!(name("green").as_deref(), Some("green"));
        assert_eq!(name("colour235").as_deref(), Some("235"));
        assert_eq!(name("color7").as_deref(), Some("7"));
        assert_eq!(name("brightred").as_deref(), Some("bright-red"));
        assert_eq!(name("#ff8800").as_deref(), Some("#ff8800"));
        assert_eq!(name("default"), None);
    }

    #[test]
    fn shell_quoting() {
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("it's"), "'it'\\''s'");
    }
}
