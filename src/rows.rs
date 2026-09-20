//! The lines fed to fzf.
//!
//! Each line is tab-separated: `id`, session name, padded tree name, padded
//! breadcrumb name, padded activity dots, badge. fzf shows one of the two
//! name columns followed by the dots and the badge, searches the name column
//! it shows, and uses field 1 (a session `$n` or window `@n` id) to keep the
//! cursor on the same row across reloads. Everything comes from one
//! `list-panes -a` call plus whatever the agent collector needs.
//!
//! In windows mode a session row is a header, its name in bold, and its
//! windows follow behind dim `├`/`└` guides with no session name of their
//! own. That reads well until a filter hides the header, and fzf cannot match
//! text it does not show. So the breadcrumb column repeats each window's
//! `session:index`, and the popup switches to it while the query is non-empty.

use anyhow::Result;

use crate::agent::{self, State};
use crate::ansi::{bold, pad, tint, visible_width};
use crate::state::Switcher;
use crate::tmux::{self, Pane};

/// Guides, indexes, breadcrumbs, badges and idle dots: present but quiet.
const DIM: &str = "brightblack";

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

fn badge(count: usize, noun: &str, suffix: &str) -> String {
    let plural = if count == 1 { "" } else { "s" };
    tint(&format!("{count} {noun}{plural}{suffix}"), DIM)
}

pub struct Row {
    pub id: String,
    pub name: String,
    /// The name column while the query is empty.
    pub tree: String,
    /// The name column while filtering: a window row names its session.
    pub breadcrumb: String,
    pub dots: String,
    pub badge: String,
}

/// A session with one representative pane per window, windows in index order.
struct Group<'a> {
    session: &'a Pane,
    windows: Vec<&'a Pane>,
}

fn groups(panes: &[Pane]) -> Vec<Group<'_>> {
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

/// Starred sessions first, then the rest, each group in name order. In
/// windows mode every session row is followed by its windows.
pub fn rows(windows: bool) -> Result<Vec<Row>> {
    let panes = tmux::panes()?;
    let states = agent::states(&panes)?;
    let favorites = Switcher::load().favorites;
    let groups = groups(&panes);
    // Right-aligned indexes keep the names in a column past the tenth window.
    let index_width = groups
        .iter()
        .flat_map(|g| &g.windows)
        .map(|w| w.window_index.to_string().len())
        .max()
        .unwrap_or(1);

    let mut starred = Vec::new();
    let mut rest = Vec::new();
    for group in groups {
        let session = group.session;
        let star = favorites.contains(&session.session_name);
        let out = if star { &mut starred } else { &mut rest };
        let window_states: Vec<State> =
            group.windows.iter().map(|w| states[&w.window_id]).collect();
        let attached = if session.session_attached {
            " · attached"
        } else {
            ""
        };
        // In windows mode the session row is the header of the rows below.
        let name = if windows {
            bold(&session.session_name)
        } else {
            session.session_name.clone()
        };
        let shown = format!("{} {name}", if star { "★" } else { " " });
        out.push(Row {
            id: session.session_id.clone(),
            name: session.session_name.clone(),
            breadcrumb: shown.clone(),
            tree: shown,
            dots: dots(&window_states),
            badge: badge(group.windows.len(), "window", attached),
        });
        if !windows {
            continue;
        }
        let last = group.windows.len() - 1;
        for (position, window) in group.windows.iter().enumerate() {
            let guide = if position == last { "└" } else { "├" };
            let index = format!("{:>index_width$}", window.window_index);
            let crumb = format!("{}:{}", session.session_name, window.window_index);
            // Only what is worth reading: a lone pane says nothing.
            let mut notes = Vec::new();
            if window.window_panes > 1 {
                notes.push(format!("{} panes", window.window_panes));
            }
            if window.window_active {
                notes.push("active".to_string());
            }
            let badge = if notes.is_empty() {
                String::new()
            } else {
                tint(&notes.join(" · "), DIM)
            };
            out.push(Row {
                id: window.window_id.clone(),
                name: session.session_name.clone(),
                tree: format!(
                    "  {}  {}",
                    tint(&format!("{guide} {index}"), DIM),
                    window.window_name
                ),
                breadcrumb: format!("  {}  {}", tint(&crumb, DIM), window.window_name),
                dots: dots(&[states[&window.window_id]]),
                badge,
            });
        }
    }
    starred.extend(rest);
    Ok(starred)
}

pub fn lines(windows: bool) -> Result<Vec<String>> {
    let rows = rows(windows)?;
    let widest = |column: fn(&Row) -> &str| {
        rows.iter()
            .map(|row| visible_width(column(row)))
            .max()
            .unwrap_or(0)
    };
    // One width for both name columns, so swapping them moves nothing else.
    let name_width = widest(|row| &row.tree).max(widest(|row| &row.breadcrumb));
    let dots_width = widest(|row| &row.dots);
    Ok(rows
        .iter()
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
        .collect())
}

pub fn join(lines: &[String]) -> String {
    let mut text = lines.join("\n");
    text.push('\n');
    text
}
