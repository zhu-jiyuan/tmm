//! Project directories: the roots named by the `@tmm-projects` tmux option,
//! scanned a level or two deep, and how a directory becomes a session.
//!
//! A project row's id is its absolute path. No session (`$n`) or window
//! (`@n`) id looks like that, so the row keys know to leave it alone, and
//! Enter knows to open it.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::{paths, tmux};

pub struct Project {
    /// Canonical, so it compares with a session's start directory.
    pub path: PathBuf,
    /// What its session is called.
    pub name: String,
}

/// `~` and `~/x` relative to the home directory; anything else as is.
pub fn expand(text: &str) -> PathBuf {
    match text.strip_prefix("~") {
        Some("") => paths::home(),
        Some(rest) if rest.starts_with('/') => paths::home().join(&rest[1..]),
        _ => PathBuf::from(text),
    }
}

/// A path with the home directory shortened to `~`.
pub fn shorten(path: &Path) -> String {
    match path.strip_prefix(paths::home()) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// The directory name, with the characters tmux refuses in session names replaced.
pub fn session_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().replace(['.', ':'], "_"))
        .unwrap_or_else(|| "root".to_string())
}

/// The option's entries: a path, optionally with `:depth` (default 1).
fn roots() -> Vec<(PathBuf, usize)> {
    tmux::run_lossy(&["show-option", "-gqv", "@tmm-projects"])
        .split_whitespace()
        .map(|entry| match entry.rsplit_once(':') {
            Some((path, depth))
                if !depth.is_empty() && depth.bytes().all(|b| b.is_ascii_digit()) =>
            {
                (expand(path), depth.parse().unwrap_or(1))
            }
            _ => (expand(entry), 1),
        })
        .collect()
}

/// Directories under `root`, up to `depth` levels down, hidden ones skipped.
fn walk(root: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        // Real directories only, as `find -type d` would see them.
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let path = entry.path();
        walk(&path, depth - 1, out);
        out.push(path);
    }
}

/// Every project, each path once, in name order.
pub fn scan() -> Vec<Project> {
    let mut found = Vec::new();
    for (root, depth) in roots() {
        walk(&root, depth, &mut found);
    }
    let mut seen = BTreeSet::new();
    let mut projects: Vec<Project> = found
        .into_iter()
        .filter_map(|path| fs::canonicalize(path).ok())
        .filter(|path| seen.insert(path.clone()))
        .map(|path| Project {
            name: session_name(&path),
            path,
        })
        .collect();
    projects.sort_by_cached_key(|project| (project.name.to_lowercase(), project.path.clone()));
    projects
}

/// Every session: id, name, start directory.
fn sessions() -> Result<Vec<(String, String, PathBuf)>> {
    Ok(tmux::run(&[
        "list-sessions",
        "-F",
        "#{session_id}\t#{session_name}\t#{session_path}",
    ])?
    .lines()
    .filter_map(|line| {
        let mut fields = line.splitn(3, '\t');
        Some((
            fields.next()?.to_string(),
            fields.next()?.to_string(),
            PathBuf::from(fields.next()?),
        ))
    })
    .collect())
}

/// The session started in `dir`, or a new one made there; its id either way.
pub fn open(dir: &Path) -> Result<String> {
    let path = fs::canonicalize(dir).with_context(|| dir.display().to_string())?;
    if !path.is_dir() {
        bail!("{} is not a directory", path.display());
    }
    let sessions = sessions()?;
    let started_here = |start: &Path| fs::canonicalize(start).is_ok_and(|start| start == path);
    if let Some((id, _, _)) = sessions.iter().find(|(_, _, start)| started_here(start)) {
        return Ok(id.clone());
    }
    let taken = |name: &str| sessions.iter().any(|(_, existing, _)| existing == name);
    let base = session_name(&path);
    let mut name = base.clone();
    if taken(&name) {
        // A namesake started elsewhere: say where this one lives.
        let parent = path.parent().map(session_name).unwrap_or_default();
        name = format!("{base}-{parent}");
        let mut n = 2;
        while taken(&name) {
            name = format!("{base}-{parent}-{n}");
            n += 1;
        }
    }
    tmux::run(&[
        "new-session",
        "-d",
        "-s",
        &name,
        "-c",
        &path.to_string_lossy(),
        "-P",
        "-F",
        "#{session_id}",
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_tmux_rules() {
        assert_eq!(session_name(Path::new("/x/my.app")), "my_app");
        assert_eq!(session_name(Path::new("/x/a:b")), "a_b");
        assert_eq!(session_name(Path::new("/")), "root");
    }

    #[test]
    fn home_expands_and_shortens() {
        let home = paths::home();
        assert_eq!(expand("~"), home);
        assert_eq!(expand("~/x/y"), home.join("x/y"));
        assert_eq!(expand("/abs"), PathBuf::from("/abs"));
        assert_eq!(expand("~x"), PathBuf::from("~x"));
        assert_eq!(shorten(&home.join("x")), "~/x");
        assert_eq!(shorten(&home), "~");
        assert_eq!(shorten(Path::new("/opt/x")), "/opt/x");
    }
}
