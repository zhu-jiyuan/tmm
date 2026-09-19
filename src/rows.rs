//! The lines fed to fzf.
//!
//! Each line is tab-separated: `id`, session name, padded shown name, padded
//! activity dots, badge. fzf displays fields 3–5, searches field 3 and uses
//! field 1 (a session `$n` or window `@n` id) to keep the cursor on the same
//! row across reloads. Everything comes from one `list-panes -a` call plus
//! whatever the agent collector needs.

use anyhow::Result;

use crate::agent::{self, State};
use crate::ansi::{pad, tint, visible_width};
use crate::state::Switcher;
use crate::tmux::{self, Pane};

fn dots(states: &[State]) -> String {
    states
        .iter()
        .map(|state| {
            let colour = match state {
                State::Plain => "brightwhite",
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
    tint(&format!("{count} {noun}{plural}{suffix}"), "brightblack")
}

pub struct Row {
    pub id: String,
    pub name: String,
    pub shown: String,
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

    let mut starred = Vec::new();
    let mut rest = Vec::new();
    for group in groups(&panes) {
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
        out.push(Row {
            id: session.session_id.clone(),
            name: session.session_name.clone(),
            shown: format!("{} {}", if star { "★" } else { " " }, session.session_name),
            dots: dots(&window_states),
            badge: badge(group.windows.len(), "window", attached),
        });
        if !windows {
            continue;
        }
        for window in &group.windows {
            let active = if window.window_active {
                " · active"
            } else {
                ""
            };
            out.push(Row {
                id: window.window_id.clone(),
                name: session.session_name.clone(),
                // The session name is part of the searchable text, so a
                // filter like "beta ed" narrows to beta's editor window.
                shown: format!(
                    "  {} / {}: {}",
                    session.session_name, window.window_index, window.window_name
                ),
                dots: dots(&[states[&window.window_id]]),
                badge: badge(window.window_panes, "pane", active),
            });
        }
    }
    starred.extend(rest);
    Ok(starred)
}

pub fn lines(windows: bool) -> Result<Vec<String>> {
    let rows = rows(windows)?;
    let name_width = rows
        .iter()
        .map(|row| visible_width(&row.shown))
        .max()
        .unwrap_or(0);
    let dots_width = rows
        .iter()
        .map(|row| visible_width(&row.dots))
        .max()
        .unwrap_or(0);
    Ok(rows
        .iter()
        .map(|row| {
            format!(
                "{}\t{}\t{} \t{} \t{}",
                row.id,
                row.name,
                pad(&row.shown, name_width),
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
