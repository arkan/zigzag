use std::collections::HashSet;

use z_core::domain::Session;

use crate::ProjectEntry;

/// Data returned by a background refresh thread.
pub struct RefreshData {
    /// Sessions grouped by project name.
    pub sessions: Vec<(String, Vec<Session>)>,
    /// Session names with pending notifications.
    pub notifications: HashSet<String>,
}

/// Result of merging refresh data into existing entries.
pub struct MergeResult {
    pub selected_project: usize,
    pub selected_session: usize,
}

/// Merge new session/notification data into existing entries,
/// preserving cursor selection by name.
pub fn merge_refresh(
    entries: &mut [ProjectEntry],
    notifications: &mut HashSet<String>,
    data: RefreshData,
    selected_project: usize,
    selected_session: usize,
) -> MergeResult {
    // Remember selected names before mutation.
    let sel_project_name = entries
        .get(selected_project)
        .map(|e| e.project.name.clone());
    let sel_session_name = sel_project_name.as_ref().and_then(|_| {
        entries
            .get(selected_project)
            .and_then(|e| e.sessions.get(selected_session))
            .map(|s| s.name.clone())
    });

    // Update sessions per project.
    for (proj_name, new_sessions) in data.sessions {
        if let Some(entry) = entries.iter_mut().find(|e| e.project.name == proj_name) {
            entry.sessions = new_sessions;
        }
    }

    // Update notifications.
    *notifications = data.notifications;

    // Restore cursor by name.
    let new_project_idx = sel_project_name
        .as_ref()
        .and_then(|name| entries.iter().position(|e| &e.project.name == name))
        .unwrap_or_else(|| selected_project.min(entries.len().saturating_sub(1)));

    let new_session_idx = sel_session_name
        .as_ref()
        .and_then(|name| {
            entries
                .get(new_project_idx)
                .and_then(|e| e.sessions.iter().position(|s| &s.name == name))
        })
        .unwrap_or_else(|| {
            let max = entries
                .get(new_project_idx)
                .map(|e| e.sessions.len().saturating_sub(1))
                .unwrap_or(0);
            selected_session.min(max)
        });

    MergeResult {
        selected_project: new_project_idx,
        selected_session: new_session_idx,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use z_core::domain::Project;
    use std::path::PathBuf;

    fn make_project(name: &str) -> Project {
        Project {
            name: name.to_string(),
            path: PathBuf::from(format!("/tmp/{}", name)),
            host: None,
            token: None,
        }
    }

    fn make_entry(name: &str, sessions: &[&str]) -> ProjectEntry {
        ProjectEntry {
            project: make_project(name),
            sessions: sessions
                .iter()
                .map(|b| Session::new(name, b))
                .collect(),
            worktree_count: 0,
            workflows: vec![],
            repo_actions: vec![],
        }
    }

    fn make_refresh(sessions: Vec<(&str, Vec<&str>)>) -> RefreshData {
        RefreshData {
            sessions: sessions
                .into_iter()
                .map(|(proj, branches)| {
                    let ss = branches
                        .iter()
                        .map(|b| Session::new(proj, b))
                        .collect();
                    (proj.to_string(), ss)
                })
                .collect(),
            notifications: HashSet::new(),
        }
    }

    #[test]
    fn all_sessions_removed_resets_session_index() {
        let mut entries = vec![
            make_entry("alpha", &["main", "dev"]),
        ];
        let mut notifications = HashSet::new();
        let data = make_refresh(vec![
            ("alpha", vec![]),
        ]);

        let result = merge_refresh(&mut entries, &mut notifications, data, 0, 1);

        assert_eq!(result.selected_project, 0);
        assert_eq!(result.selected_session, 0);
        assert!(entries[0].sessions.is_empty());
    }

    #[test]
    fn empty_entries_no_panic() {
        let mut entries: Vec<ProjectEntry> = vec![];
        let mut notifications = HashSet::new();
        let data = make_refresh(vec![]);

        let result = merge_refresh(&mut entries, &mut notifications, data, 0, 0);

        assert_eq!(result.selected_project, 0);
        assert_eq!(result.selected_session, 0);
    }

    #[test]
    fn project_missing_from_refresh_keeps_sessions() {
        let mut entries = vec![
            make_entry("alpha", &["main", "dev"]),
            make_entry("beta", &["main"]),
        ];
        let mut notifications = HashSet::new();
        // Only alpha in refresh data; beta should be untouched
        let data = make_refresh(vec![
            ("alpha", vec!["main", "dev", "feat"]),
        ]);

        merge_refresh(&mut entries, &mut notifications, data, 1, 0);

        assert_eq!(entries[0].sessions.len(), 3); // alpha updated
        assert_eq!(entries[1].sessions.len(), 1); // beta untouched
        assert_eq!(entries[1].sessions[0].branch, "main");
    }

    #[test]
    fn notifications_replaced() {
        let mut entries = vec![
            make_entry("alpha", &["main", "dev"]),
        ];
        let mut notifications: HashSet<String> =
            ["alpha:main".to_string()].into_iter().collect();
        let mut data = make_refresh(vec![
            ("alpha", vec!["main", "dev"]),
        ]);
        data.notifications = ["alpha:dev".to_string()].into_iter().collect();

        merge_refresh(&mut entries, &mut notifications, data, 0, 0);

        assert!(!notifications.contains("alpha:main"));
        assert!(notifications.contains("alpha:dev"));
    }

    #[test]
    fn selected_session_removed_clamps_cursor() {
        let mut entries = vec![
            make_entry("alpha", &["main", "dev", "feat"]),
        ];
        let mut notifications = HashSet::new();
        // Cursor on "feat" (index 2), which gets removed
        let data = make_refresh(vec![
            ("alpha", vec!["main", "dev"]),
        ]);

        let result = merge_refresh(&mut entries, &mut notifications, data, 0, 2);

        assert_eq!(result.selected_project, 0);
        // Clamped to last valid index (1)
        assert_eq!(result.selected_session, 1);
    }

    #[test]
    fn other_session_removed_cursor_follows_by_name() {
        let mut entries = vec![
            make_entry("alpha", &["main", "dev", "feat"]),
        ];
        let mut notifications = HashSet::new();
        // "dev" removed; cursor was on "feat" (index 2)
        let data = make_refresh(vec![
            ("alpha", vec!["main", "feat"]),
        ]);

        let result = merge_refresh(&mut entries, &mut notifications, data, 0, 2);

        assert_eq!(result.selected_project, 0);
        // "feat" is now at index 1
        assert_eq!(result.selected_session, 1);
        assert_eq!(entries[0].sessions.len(), 2);
    }

    #[test]
    fn session_added_preserves_cursor() {
        let mut entries = vec![
            make_entry("alpha", &["main"]),
        ];
        let mut notifications = HashSet::new();
        let data = make_refresh(vec![
            ("alpha", vec!["main", "dev"]),
        ]);

        let result = merge_refresh(&mut entries, &mut notifications, data, 0, 0);

        assert_eq!(result.selected_project, 0);
        assert_eq!(result.selected_session, 0);
        assert_eq!(entries[0].sessions.len(), 2);
        assert_eq!(entries[0].sessions[1].branch, "dev");
    }

    #[test]
    fn no_changes_preserves_cursor() {
        let mut entries = vec![
            make_entry("alpha", &["main", "dev"]),
            make_entry("beta", &["main"]),
        ];
        let mut notifications = HashSet::new();
        let data = make_refresh(vec![
            ("alpha", vec!["main", "dev"]),
            ("beta", vec!["main"]),
        ]);

        let result = merge_refresh(&mut entries, &mut notifications, data, 1, 0);

        assert_eq!(result.selected_project, 1);
        assert_eq!(result.selected_session, 0);
        assert_eq!(entries[0].sessions.len(), 2);
        assert_eq!(entries[1].sessions.len(), 1);
    }
}
