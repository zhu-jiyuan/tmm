//! Persistent preferences: which sessions are starred.

use std::collections::BTreeSet;
use std::fs;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::paths;

#[derive(Default, PartialEq, Serialize, Deserialize)]
pub struct Switcher {
    /// Session names. A `BTreeSet` keeps the file sorted and diff-friendly.
    pub favorites: BTreeSet<String>,
}

impl Switcher {
    /// A missing file means "no favorites yet".
    pub fn load() -> Self {
        match fs::read_to_string(paths::switcher_file()) {
            Ok(text) => serde_json::from_str(&text).expect("a valid switcher.json"),
            Err(_) => Switcher::default(),
        }
    }

    pub fn save(&self) -> Result<()> {
        let text = serde_json::to_string_pretty(self)? + "\n";
        paths::write_atomic(&paths::switcher_file(), &text)
    }
}
