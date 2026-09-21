//! One open popup: the row snapshot fzf is showing and the state kept
//! beside it.
//!
//! `switch` creates the snapshot and exports its path as `TMM_SNAPSHOT`, so
//! every subcommand fzf runs finds the same popup through [`Popup::current`].
//! Small bits of state live in *sidecars*: files next to the snapshot, one
//! per [`Sidecar`] variant, so they survive each subcommand being a fresh
//! process and vanish with the popup. Nothing else on disk is per popup.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;

use anyhow::{Context, Result};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::paths;

/// What the list shows. `tab` cycles through them in this order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Sessions,
    Windows,
    Projects,
}

impl Mode {
    /// The prompt names the mode, since tmux cannot retitle an open popup.
    pub fn prompt(self) -> &'static str {
        match self {
            Mode::Sessions => "sessions> ",
            Mode::Windows => "windows> ",
            Mode::Projects => "projects> ",
        }
    }

    pub fn next(self) -> Mode {
        match self {
            Mode::Sessions => Mode::Windows,
            Mode::Windows => Mode::Projects,
            Mode::Projects => Mode::Sessions,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Mode::Sessions => "sessions",
            Mode::Windows => "windows",
            Mode::Projects => "projects",
        }
    }

    fn parse(text: &str) -> Mode {
        match text.trim() {
            "windows" => Mode::Windows,
            "projects" => Mode::Projects,
            _ => Mode::Sessions,
        }
    }
}

/// The kinds of state a popup keeps beside its snapshot.
#[derive(Clone, Copy)]
pub enum Sidecar {
    /// The mode, by name; absent means sessions.
    Mode,
    /// The open inline prompt, as JSON (`actions::Pending`).
    Prompt,
    /// Each session's chosen preview window, as JSON (`preview`).
    Preview,
    /// The fzf action to replay after the full-screen preview.
    View,
    /// Present while the key legend is hidden.
    Help,
}

impl Sidecar {
    const ALL: [Sidecar; 5] = [
        Sidecar::Mode,
        Sidecar::Prompt,
        Sidecar::Preview,
        Sidecar::View,
        Sidecar::Help,
    ];

    /// The extension that tells the files apart.
    fn extension(self) -> &'static str {
        match self {
            Sidecar::Mode => "mode",
            Sidecar::Prompt => "prompt",
            Sidecar::Preview => "preview",
            Sidecar::View => "view",
            Sidecar::Help => "help",
        }
    }
}

pub struct Popup {
    snapshot: PathBuf,
}

impl Popup {
    /// The popup this subcommand runs inside, from `TMM_SNAPSHOT`.
    pub fn current() -> Result<Popup> {
        Ok(Popup::at(paths::snapshot()?))
    }

    pub fn at(snapshot: impl Into<PathBuf>) -> Popup {
        Popup {
            snapshot: snapshot.into(),
        }
    }

    /// A popup for this process, after sweeping what popups that were
    /// killed rather than closed left behind.
    pub fn create() -> Result<Popup> {
        let run = paths::run_dir()?;
        remove_stale(&run)?;
        Ok(Popup::at(run.join(format!("switch-{}.txt", process::id()))))
    }

    pub fn snapshot(&self) -> &Path {
        &self.snapshot
    }

    pub fn exists(&self) -> bool {
        self.snapshot.exists()
    }

    pub fn read_snapshot(&self) -> Result<String> {
        fs::read_to_string(&self.snapshot)
            .with_context(|| format!("reading {}", self.snapshot.display()))
    }

    pub fn write_snapshot(&self, text: &str) -> Result<()> {
        paths::write_atomic(&self.snapshot, text)
    }

    fn path(&self, sidecar: Sidecar) -> PathBuf {
        self.snapshot.with_extension(sidecar.extension())
    }

    pub fn has(&self, sidecar: Sidecar) -> bool {
        self.path(sidecar).exists()
    }

    pub fn read(&self, sidecar: Sidecar) -> Option<String> {
        fs::read_to_string(self.path(sidecar)).ok()
    }

    pub fn write(&self, sidecar: Sidecar, text: &str) -> Result<()> {
        paths::write_atomic(&self.path(sidecar), text)
    }

    /// Gone afterwards, whether or not it was there.
    pub fn remove(&self, sidecar: Sidecar) -> Result<()> {
        match fs::remove_file(self.path(sidecar)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        }
    }

    /// Read and remove: for state that is consumed once.
    pub fn take(&self, sidecar: Sidecar) -> Result<Option<String>> {
        let text = self.read(sidecar);
        if text.is_some() {
            self.remove(sidecar)?;
        }
        Ok(text)
    }

    pub fn read_json<T: DeserializeOwned>(&self, sidecar: Sidecar) -> Result<Option<T>> {
        self.read(sidecar)
            .map(|text| serde_json::from_str(&text))
            .transpose()
            .with_context(|| format!("parsing {}", self.path(sidecar).display()))
    }

    pub fn write_json<T: Serialize>(&self, sidecar: Sidecar, value: &T) -> Result<()> {
        self.write(sidecar, &serde_json::to_string(value)?)
    }

    pub fn mode(&self) -> Mode {
        self.read(Sidecar::Mode)
            .map(|text| Mode::parse(&text))
            .unwrap_or(Mode::Sessions)
    }

    pub fn set_mode(&self, mode: Mode) -> Result<()> {
        if mode == Mode::Sessions {
            self.remove(Sidecar::Mode)
        } else {
            self.write(Sidecar::Mode, mode.name())
        }
    }

    /// Everything this popup left on disk.
    pub fn cleanup(&self) {
        let _ = fs::remove_file(&self.snapshot);
        for sidecar in Sidecar::ALL {
            let _ = fs::remove_file(self.path(sidecar));
        }
    }
}

/// Snapshots left behind by popups that were killed rather than closed.
fn remove_stale(run: &Path) -> Result<()> {
    for entry in fs::read_dir(run)? {
        let path = entry?.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let Some(pid) = name
            .strip_prefix("switch-")
            .and_then(|n| n.strip_suffix(".txt"))
        else {
            continue;
        };
        // Signal 0 checks whether the process exists without touching it.
        let alive = unsafe { libc::kill(pid.parse()?, 0) } == 0;
        if !alive {
            Popup::at(path.clone()).cleanup();
        }
    }
    Ok(())
}
