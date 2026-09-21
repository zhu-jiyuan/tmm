//! What a row stands for, encoded in fzf's first field.
//!
//! fzf hands the id back as `{1}` to every row key. A session is `$n` and a
//! window `@n`, tmux's own ids and valid `-t` targets; a project that has no
//! session yet is its absolute path. When nothing matches the filter `{1}`
//! is empty, and `parse` says so, because tmux would take an empty target to
//! mean the current session.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum RowId {
    Session(String),
    Window(String),
    Project(PathBuf),
}

impl RowId {
    pub fn parse(text: &str) -> Option<RowId> {
        match text.as_bytes().first()? {
            b'$' => Some(RowId::Session(text.to_string())),
            b'@' => Some(RowId::Window(text.to_string())),
            b'/' => Some(RowId::Project(PathBuf::from(text))),
            _ => None,
        }
    }

    /// The tmux target, for what tmux knows about.
    pub fn target(&self) -> Option<&str> {
        match self {
            RowId::Session(id) | RowId::Window(id) => Some(id),
            RowId::Project(_) => None,
        }
    }

    pub fn is_window(&self) -> bool {
        matches!(self, RowId::Window(_))
    }
}

impl fmt::Display for RowId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RowId::Session(id) | RowId::Window(id) => f.write_str(id),
            RowId::Project(path) => write!(f, "{}", path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_reject_the_rest() {
        assert_eq!(RowId::parse("$3"), Some(RowId::Session("$3".into())));
        assert_eq!(RowId::parse("@7"), Some(RowId::Window("@7".into())));
        assert_eq!(RowId::parse("/x/y"), Some(RowId::Project("/x/y".into())));
        assert_eq!(RowId::parse(""), None, "an empty {{1}} is not a row");
        assert_eq!(RowId::parse("name"), None);
        for text in ["$3", "@7", "/x/y"] {
            assert_eq!(RowId::parse(text).unwrap().to_string(), text);
        }
        assert_eq!(RowId::parse("/x").unwrap().target(), None);
        assert_eq!(RowId::parse("@7").unwrap().target(), Some("@7"));
    }
}
