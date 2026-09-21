//! The lines fed to fzf.
//!
//! Each line is tab-separated: `id`, session name, padded tree name, padded
//! breadcrumb name, padded activity dots, badge. fzf shows one of the two
//! name columns followed by the dots and the badge, searches the name column
//! it shows, and uses field 1 (a [`RowId`]) to keep the cursor on the same
//! row across reloads.
//!
//! [`fetch`] asks tmux and the agent collector for everything once, and
//! [`build`] turns that into rows without touching tmux again, so layout is
//! plain code with unit tests. Each mode has its own file: sessions and
//! windows in `sessions`, projects in `projects`.

mod id;
mod projects;
mod sessions;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::PathBuf;

use anyhow::Result;

use crate::agent::{self, State};
use crate::ansi::{pad, tint, visible_width};
use crate::popup::Mode;
use crate::projects::Project;
use crate::state::Switcher;
use crate::tmux::{self, Pane};

pub use id::RowId;

/// Guides, indexes, breadcrumbs, badges and idle dots: present but quiet.
const DIM: &str = "brightblack";

pub struct Row {
    pub id: RowId,
    /// The session's name, which `ctrl-s` stars; for a project that is not
    /// open yet, the name its session will have.
    pub name: String,
    /// The name column while the query is empty.
    pub tree: String,
    /// The name column while filtering: a window row names its session.
    pub breadcrumb: String,
    pub dots: String,
    pub badge: String,
}

/// Everything the rows are made from, gathered once per listing.
pub struct Data {
    pub panes: Vec<Pane>,
    /// Agent state per window id.
    pub states: BTreeMap<String, State>,
    pub favorites: BTreeSet<String>,
    /// Each session's start directory, canonical, by session id. Projects
    /// mode only.
    pub session_dirs: HashMap<String, PathBuf>,
    /// Projects mode only.
    pub projects: Vec<Project>,
}

pub fn fetch(mode: Mode) -> Result<Data> {
    let panes = tmux::panes()?;
    let states = agent::states(&panes)?;
    let favorites = Switcher::load().favorites;
    let mut session_dirs = HashMap::new();
    let mut projects = Vec::new();
    if mode == Mode::Projects {
        for pane in &panes {
            if !session_dirs.contains_key(&pane.session_id)
                && let Ok(dir) = fs::canonicalize(&pane.session_path)
            {
                session_dirs.insert(pane.session_id.clone(), dir);
            }
        }
        projects = crate::projects::scan();
    }
    Ok(Data {
        panes,
        states,
        favorites,
        session_dirs,
        projects,
    })
}

/// The rows of a mode, in display order.
pub fn build(data: &Data, mode: Mode) -> Vec<Row> {
    let groups = Group::all(&data.panes);
    match mode {
        Mode::Sessions => sessions::rows(&groups, data, false),
        Mode::Windows => sessions::rows(&groups, data, true),
        Mode::Projects => projects::rows(&groups, data),
    }
}

pub fn lines(mode: Mode) -> Result<Vec<String>> {
    Ok(layout(&build(&fetch(mode)?, mode)))
}

/// One tab-separated line per row. Both name columns share a width, so the
/// popup can swap them without moving the dots and badges.
pub fn layout(rows: &[Row]) -> Vec<String> {
    let widest = |column: fn(&Row) -> &str| {
        rows.iter()
            .map(|row| visible_width(column(row)))
            .max()
            .unwrap_or(0)
    };
    let name_width = widest(|row| &row.tree).max(widest(|row| &row.breadcrumb));
    let dots_width = widest(|row| &row.dots);
    rows.iter()
        .map(|row| {
            format!(
                "{}\t{}\t{} \t{} \t{} \t{}",
                row.id,
                row.name,
                pad(&row.tree, name_width),
                pad(&row.breadcrumb, name_width),
                pad(&row.dots, dots_width),
                row.badge
            )
        })
        .collect()
}

pub fn join(lines: &[String]) -> String {
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

fn dots(states: &[State]) -> String {
    states
        .iter()
        .map(|state| {
            let colour = match state {
                State::Plain => DIM,
                State::Working => "brightgreen",
                State::Waiting => "brightyellow",
            };
            tint("●", colour)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn count(n: usize, noun: &str) -> String {
    let plural = if n == 1 { "" } else { "s" };
    format!("{n} {noun}{plural}")
}

fn badge(text: &str) -> String {
    tint(text, DIM)
}

/// The name column: a star for favorites, a space to keep the rest aligned.
fn star(starred: bool, name: &str) -> String {
    format!("{} {name}", if starred { "★" } else { " " })
}

/// A session with one representative pane per window, windows in index order.
struct Group<'a> {
    session: &'a Pane,
    windows: Vec<&'a Pane>,
}

impl<'a> Group<'a> {
    /// Every session, in name order.
    fn all(panes: &'a [Pane]) -> Vec<Group<'a>> {
        let mut groups: Vec<Group> = Vec::new();
        for pane in panes {
            let index = match groups
                .iter()
                .position(|g| g.session.session_id == pane.session_id)
            {
                Some(index) => index,
                None => {
                    groups.push(Group {
                        session: pane,
                        windows: Vec::new(),
                    });
                    groups.len() - 1
                }
            };
            let group = &mut groups[index];
            if !group.windows.iter().any(|w| w.window_id == pane.window_id) {
                group.windows.push(pane);
            }
        }
        groups.sort_by_cached_key(|g| g.session.session_name.to_lowercase());
        for group in &mut groups {
            group.windows.sort_by_key(|w| w.window_index);
        }
        groups
    }

    fn window_states(&self, data: &Data) -> Vec<State> {
        self.windows
            .iter()
            .map(|w| {
                data.states
                    .get(&w.window_id)
                    .copied()
                    .unwrap_or(State::Plain)
            })
            .collect()
    }

    fn attached(&self) -> &'static str {
        if self.session.session_attached {
            " · attached"
        } else {
            ""
        }
    }

    /// This session's own row, however it is shown.
    fn row(&self, data: &Data, shown_name: &str, badge_text: &str) -> Row {
        let session = self.session;
        let starred = data.favorites.contains(&session.session_name);
        Row {
            id: RowId::Session(session.session_id.clone()),
            name: session.session_name.clone(),
            tree: star(starred, shown_name),
            breadcrumb: star(starred, shown_name),
            dots: dots(&self.window_states(data)),
            badge: badge(badge_text),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ansi::strip;

    /// One pane, standing for one window of a session.
    fn pane(session: (&str, &str), window: (&str, usize, &str)) -> Pane {
        Pane {
            session_id: session.0.to_string(),
            session_name: session.1.to_string(),
            session_attached: false,
            session_path: format!("/nowhere/{}", session.1),
            window_id: window.0.to_string(),
            window_index: window.1,
            window_name: window.2.to_string(),
            window_active: window.1 == 0,
            window_panes: 1,
            id: format!("%{}", &window.0[1..]),
            pid: 1,
            dead: false,
            tty: String::new(),
            command: "zsh".to_string(),
            agent: String::new(),
        }
    }

    fn data(panes: Vec<Pane>) -> Data {
        Data {
            panes,
            states: BTreeMap::new(),
            favorites: BTreeSet::new(),
            session_dirs: HashMap::new(),
            projects: Vec::new(),
        }
    }

    fn fields(row: &Row) -> [String; 4] {
        [
            strip(&row.tree),
            strip(&row.breadcrumb),
            strip(&row.dots),
            strip(&row.badge),
        ]
    }

    #[test]
    fn sessions_are_starred_first_with_a_dot_per_window() {
        let mut d = data(vec![
            pane(("$0", "alpha"), ("@0", 0, "zsh")),
            pane(("$0", "alpha"), ("@1", 1, "editor")),
            pane(("$1", "beta"), ("@2", 0, "zsh")),
        ]);
        d.panes[2].session_attached = true;
        d.favorites.insert("beta".into());
        d.states.insert("@1".into(), State::Working);

        let rows = build(&d, Mode::Sessions);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["beta", "alpha"]);
        let [tree, _, dots, badge] = fields(&rows[0]);
        assert_eq!(
            (tree.as_str(), dots.as_str(), badge.as_str()),
            ("★ beta", "●", "1 window · attached")
        );
        let [tree, _, dots, badge] = fields(&rows[1]);
        assert_eq!(
            (tree.as_str(), dots.as_str(), badge.as_str()),
            ("  alpha", "● ●", "2 windows")
        );
        assert!(
            rows[1].dots.contains("\x1b[92m"),
            "working is green: {:?}",
            rows[1].dots
        );
        assert!(
            !rows[1].tree.contains("\x1b[1m"),
            "no bold outside windows mode"
        );
        assert_eq!(rows[1].id, RowId::Session("$0".into()));
    }

    #[test]
    fn windows_mode_draws_a_tree_under_bold_headers() {
        let mut d = data(vec![
            pane(("$0", "alpha"), ("@0", 0, "zsh")),
            pane(("$0", "alpha"), ("@1", 1, "editor")),
        ]);
        d.panes[0].window_panes = 2;

        let rows = build(&d, Mode::Windows);
        assert!(rows[0].tree.contains("\x1b[1malpha"), "{:?}", rows[0].tree);
        let [tree, crumb, dots, badge] = fields(&rows[1]);
        assert_eq!(
            (tree.as_str(), crumb.as_str(), dots.as_str(), badge.as_str()),
            ("  ├ 0 → zsh", "  alpha:0 → zsh", "●", "2 panes")
        );
        let [tree, crumb, _, badge] = fields(&rows[2]);
        assert_eq!(
            (tree.as_str(), crumb.as_str(), badge.as_str()),
            ("  └ 1   editor", "  alpha:1   editor", "")
        );
        assert_eq!(rows[2].id, RowId::Window("@1".into()));
        assert_eq!(
            rows[2].name, "alpha",
            "window rows carry their session for ctrl-s"
        );
    }

    #[test]
    fn a_lone_window_gets_no_arrow() {
        let d = data(vec![pane(("$0", "solo"), ("@0", 0, "zsh"))]);

        let rows = build(&d, Mode::Windows);
        let [tree, crumb, _, badge] = fields(&rows[1]);
        assert_eq!(
            (tree.as_str(), crumb.as_str(), badge.as_str()),
            ("  └ 0   zsh", "  solo:0   zsh", "")
        );
    }

    #[test]
    fn projects_match_sessions_by_directory_then_by_unclaimed_name() {
        let mut d = data(vec![
            pane(("$0", "api"), ("@0", 0, "zsh")),
            pane(("$1", "zed"), ("@1", 0, "zsh")),
        ]);
        d.session_dirs.insert("$0".into(), "/p/nested/api".into());
        d.favorites.insert("tools".into());
        for path in ["/p/api", "/p/nested/api", "/p/tools", "/p/zed"] {
            let path = PathBuf::from(path);
            d.projects.push(Project {
                name: crate::projects::session_name(&path),
                path,
            });
        }

        let rows = build(&d, Mode::Projects);
        let summary: Vec<(String, String, String)> = rows
            .iter()
            .map(|r| (r.id.to_string(), fields(r)[0].clone(), fields(r)[3].clone()))
            .collect();
        assert_eq!(
            summary,
            [
                (
                    "/p/tools".to_string(),
                    "★ tools".to_string(),
                    "/p".to_string()
                ),
                (
                    "$0".to_string(),
                    "  api".to_string(),
                    "/p/nested · 1 window".to_string()
                ),
                (
                    "$1".to_string(),
                    "  zed".to_string(),
                    "/p · 1 window".to_string()
                ),
                ("/p/api".to_string(), "  api".to_string(), "/p".to_string()),
            ],
            "starred, then open (the session started in nested/api claims it, \
             so the namesake stays unopened), then the rest"
        );
    }

    #[test]
    fn layout_pads_both_name_columns_alike() {
        let d = data(vec![
            pane(("$0", "a"), ("@0", 0, "zsh")),
            pane(("$0", "a"), ("@1", 1, "a-very-long-window-name")),
        ]);
        for line in layout(&build(&d, Mode::Windows)) {
            let cells: Vec<&str> = line.split('\t').collect();
            assert_eq!(cells.len(), 6, "{line:?}");
            assert_eq!(visible_width(cells[2]), visible_width(cells[3]), "{line:?}");
        }
    }
}
