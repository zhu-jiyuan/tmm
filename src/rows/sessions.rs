//! Sessions mode and windows mode: a row per session, starred ones first,
//! and in windows mode each session's windows beneath it as a tree.
//!
//! Window rows have no session name of their own behind the dim `├`/`└`
//! guides. That reads well until a filter hides the header, and fzf cannot
//! match text it does not show, so their breadcrumb column repeats
//! `session:index` and the popup switches to it while the query is non-empty.

use super::{Data, Group, Row, RowId, badge, count, dots};
use crate::agent::State;
use crate::ansi::{bold, faint};

pub(super) fn rows(groups: &[Group], data: &Data, windows: bool) -> Vec<Row> {
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
        let out = if data.favorites.contains(&session.session_name) {
            &mut starred
        } else {
            &mut rest
        };
        // In windows mode the session row is the header of the rows below.
        let name = if windows {
            bold(&session.session_name)
        } else {
            session.session_name.clone()
        };
        let badge = format!(
            "{}{}",
            count(group.windows.len(), "window"),
            group.attached()
        );
        out.push(group.row(data, &name, &badge));
        if windows {
            out.extend(window_rows(group, data, index_width));
        }
    }
    starred.extend(rest);
    starred
}

fn window_rows(group: &Group, data: &Data, index_width: usize) -> Vec<Row> {
    let session = group.session;
    let last = group.windows.len().saturating_sub(1);
    group
        .windows
        .iter()
        .enumerate()
        .map(|(position, window)| {
            let guide = if position == last { "└" } else { "├" };
            let index = format!("{:>index_width$}", window.window_index);
            let crumb = format!("{}:{}", session.session_name, window.window_index);
            let state = data
                .states
                .get(&window.window_id)
                .copied()
                .unwrap_or(State::Plain);
            // Only what is worth reading: a lone pane says nothing, and the
            // arrow marks where a switch would land only when the session
            // has more than one window to land on.
            let mark = if window.window_active && group.windows.len() > 1 {
                "→"
            } else {
                " "
            };
            Row {
                id: RowId::Window(window.window_id.clone()),
                name: session.session_name.clone(),
                tree: format!(
                    "  {} {mark} {}",
                    faint(&format!("{guide} {index}")),
                    window.window_name
                ),
                breadcrumb: format!("  {} {mark} {}", faint(&crumb), window.window_name),
                dots: dots(&[state]),
                badge: if window.window_panes > 1 {
                    badge(&count(window.window_panes, "pane"))
                } else {
                    String::new()
                },
            }
        })
        .collect()
}
