//! Projects mode: every directory under `@tmm-projects`. One with a session
//! is that session's row, dots and all; one without carries its path as the
//! id, which Enter turns into a session. Starred first, then open, then the
//! rest, each in name order.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::{Data, Group, Row, RowId, badge, count, star};

pub(super) fn rows(groups: &[Group], data: &Data) -> Vec<Row> {
    // A session belongs to the project it was started in. Failing that, a
    // namesake no other project has claimed will do.
    let mut by_dir: HashMap<&Path, &Group> = HashMap::new();
    let mut by_name: HashMap<&str, &Group> = HashMap::new();
    for group in groups {
        if let Some(dir) = data.session_dirs.get(&group.session.session_id) {
            by_dir.entry(dir.as_path()).or_insert(group);
        }
        by_name
            .entry(group.session.session_name.as_str())
            .or_insert(group);
    }
    let mut open: Vec<Option<&Group>> = data
        .projects
        .iter()
        .map(|project| by_dir.get(project.path.as_path()).copied())
        .collect();
    let mut claimed: HashSet<&str> = open
        .iter()
        .flatten()
        .map(|group| group.session.session_id.as_str())
        .collect();
    for (project, slot) in data.projects.iter().zip(&mut open) {
        if slot.is_none()
            && let Some(group) = by_name.get(project.name.as_str())
            && claimed.insert(group.session.session_id.as_str())
        {
            *slot = Some(group);
        }
    }

    let mut rows: Vec<Row> = data
        .projects
        .iter()
        .zip(open)
        .map(|(project, group)| {
            let place = crate::projects::shorten(project.path.parent().unwrap_or(&project.path));
            match group {
                Some(group) => {
                    let badge = format!(
                        "{place} · {}{}",
                        count(group.windows.len(), "window"),
                        group.attached()
                    );
                    group.row(data, &group.session.session_name, &badge)
                }
                None => {
                    let starred = data.favorites.contains(&project.name);
                    Row {
                        id: RowId::Project(project.path.clone()),
                        name: project.name.clone(),
                        tree: star(starred, &project.name),
                        breadcrumb: star(starred, &project.name),
                        dots: String::new(),
                        badge: badge(&place),
                    }
                }
            }
        })
        .collect();
    rows.sort_by_key(|row| {
        (
            !data.favorites.contains(&row.name),
            !matches!(row.id, RowId::Session(_)),
        )
    });
    rows
}
