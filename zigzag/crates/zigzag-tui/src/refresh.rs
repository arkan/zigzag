use std::collections::{HashMap, HashSet};

use zigzag_core::domain::{
    derive_session_name, Session, SessionLink, WorktreeIdentity, WorktreeStatus,
};

use crate::ProjectEntry;

/// Data returned by a background refresh thread.
pub struct RefreshData {
    /// Sessions grouped by project name.
    pub sessions: Vec<(String, Vec<Session>)>,
    /// Session names with pending notifications.
    pub notifications: HashSet<String>,
    /// Session name → last-attach unix timestamp. Used to sort sessions with
    /// the most recently attached first.
    pub activity: HashMap<String, u64>,
}

/// Refresh data tagged with the TUI state revision it was spawned from.
pub struct RefreshMessage {
    pub state_revision: u64,
    pub data: RefreshData,
}

/// Result of merging refresh data into existing entries.
pub struct MergeResult {
    pub selected_project: usize,
    pub selected_session: usize,
}

/// Decide whether an async refresh may still mutate the current TUI state.
pub fn should_apply_refresh(
    current_revision: u64,
    refresh_revision: u64,
    modal_open: bool,
) -> bool {
    current_revision == refresh_revision && !modal_open
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
    let sel_worktree_identity: Option<WorktreeIdentity> =
        sel_project_name.as_ref().and_then(|_| {
            entries
                .get(selected_project)
                .and_then(|e| e.worktrees.get(selected_session))
                .map(|w| w.discovered.identity.clone())
        });
    let sel_session_name = sel_project_name.as_ref().and_then(|_| {
        entries
            .get(selected_project)
            .and_then(|e| e.sessions.get(selected_session))
            .map(|s| s.name.clone())
    });

    // Update sessions and Worktree session links per project, sorting by most-recent attach.
    for (proj_name, mut new_sessions) in data.sessions {
        if let Some(entry) = entries.iter_mut().find(|e| e.project.name == proj_name) {
            zigzag_core::activity::sort_sessions_by_recent_attach(
                &mut new_sessions,
                &data.activity,
            );
            let sessions_by_name: HashMap<String, Session> = new_sessions
                .iter()
                .map(|session| (session.name.clone(), session.clone()))
                .collect();
            for worktree in &mut entry.worktrees {
                if matches!(
                    worktree.status,
                    WorktreeStatus::Conflict | WorktreeStatus::Unsupported
                ) {
                    continue;
                }
                let Some(branch) = worktree.discovered.branch.as_deref() else {
                    continue;
                };
                let session_name = derive_session_name(&worktree.discovered.project_name, branch);
                if let Some(session) = sessions_by_name.get(&session_name) {
                    worktree.status = WorktreeStatus::Active;
                    worktree.session_link = SessionLink::Active(session.clone());
                } else {
                    worktree.status = WorktreeStatus::Inactive;
                    worktree.session_link = SessionLink::None;
                }
            }
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

    let new_session_idx = entries
        .get(new_project_idx)
        .and_then(|entry| {
            if !entry.worktrees.is_empty() {
                sel_worktree_identity.as_ref().and_then(|identity| {
                    entry
                        .worktrees
                        .iter()
                        .position(|w| &w.discovered.identity == identity)
                })
            } else {
                sel_session_name
                    .as_ref()
                    .and_then(|name| entry.sessions.iter().position(|s| &s.name == name))
            }
        })
        .unwrap_or_else(|| {
            let max = entries
                .get(new_project_idx)
                .map(|e| {
                    if !e.worktrees.is_empty() {
                        e.worktrees.len().saturating_sub(1)
                    } else {
                        e.sessions.len().saturating_sub(1)
                    }
                })
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
    use std::path::PathBuf;
    use zigzag_core::domain::Project;

    fn make_project(name: &str) -> Project {
        Project {
            name: name.to_string(),
            path: PathBuf::from(format!("/tmp/{}", name)),
            host: None,
            transport: None,
        }
    }

    fn make_entry(name: &str, sessions: &[&str]) -> ProjectEntry {
        ProjectEntry {
            project: make_project(name),
            worktrees: vec![],
            sessions: sessions.iter().map(|b| Session::new(name, b)).collect(),
            workflows: vec![],
            repo_actions: vec![],
        }
    }

    fn make_refresh(sessions: Vec<(&str, Vec<&str>)>) -> RefreshData {
        RefreshData {
            sessions: sessions
                .into_iter()
                .map(|(proj, branches)| {
                    let ss = branches.iter().map(|b| Session::new(proj, b)).collect();
                    (proj.to_string(), ss)
                })
                .collect(),
            notifications: HashSet::new(),
            activity: HashMap::new(),
        }
    }

    #[test]
    fn all_sessions_removed_resets_session_index() {
        let mut entries = vec![make_entry("alpha", &["main", "dev"])];
        let mut notifications = HashSet::new();
        let data = make_refresh(vec![("alpha", vec![])]);

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
        let data = make_refresh(vec![("alpha", vec!["main", "dev", "feat"])]);

        merge_refresh(&mut entries, &mut notifications, data, 1, 0);

        assert_eq!(entries[0].sessions.len(), 3); // alpha updated
        assert_eq!(entries[1].sessions.len(), 1); // beta untouched
        assert_eq!(entries[1].sessions[0].branch, "main");
    }

    #[test]
    fn notifications_replaced() {
        let mut entries = vec![make_entry("alpha", &["main", "dev"])];
        let mut notifications: HashSet<String> = ["alpha:main".to_string()].into_iter().collect();
        let mut data = make_refresh(vec![("alpha", vec!["main", "dev"])]);
        data.notifications = ["alpha:dev".to_string()].into_iter().collect();

        merge_refresh(&mut entries, &mut notifications, data, 0, 0);

        assert!(!notifications.contains("alpha:main"));
        assert!(notifications.contains("alpha:dev"));
    }

    #[test]
    fn stale_refresh_is_not_applicable() {
        assert!(!should_apply_refresh(2, 1, false));
    }

    #[test]
    fn current_refresh_is_not_applicable_while_modal_open() {
        assert!(!should_apply_refresh(1, 1, true));
    }

    #[test]
    fn current_refresh_is_applicable_when_modal_closed() {
        assert!(should_apply_refresh(1, 1, false));
    }

    #[test]
    fn selected_session_removed_clamps_cursor() {
        let mut entries = vec![make_entry("alpha", &["main", "dev", "feat"])];
        let mut notifications = HashSet::new();
        // Cursor on "feat" (index 2), which gets removed
        let data = make_refresh(vec![("alpha", vec!["main", "dev"])]);

        let result = merge_refresh(&mut entries, &mut notifications, data, 0, 2);

        assert_eq!(result.selected_project, 0);
        // Legacy session-only entry clamps to last valid session index.
        assert_eq!(result.selected_session, 1);
    }

    #[test]
    fn other_session_removed_cursor_follows_by_name() {
        let mut entries = vec![make_entry("alpha", &["main", "dev", "feat"])];
        let mut notifications = HashSet::new();
        // "dev" removed; cursor was on "feat" (index 2)
        let data = make_refresh(vec![("alpha", vec!["main", "feat"])]);

        let result = merge_refresh(&mut entries, &mut notifications, data, 0, 2);

        assert_eq!(result.selected_project, 0);
        // Legacy session-only entry follows the selected session by name.
        assert_eq!(result.selected_session, 1);
        assert_eq!(entries[0].sessions.len(), 2);
    }

    #[test]
    fn sessions_sorted_by_activity_after_merge() {
        use std::collections::HashMap;
        let mut entries = vec![make_entry("alpha", &[])];
        let mut notifications = HashSet::new();
        let mut data = make_refresh(vec![("alpha", vec!["old", "new", "mid"])]);
        let mut activity: HashMap<String, u64> = HashMap::new();
        activity.insert("alpha:old".to_string(), 100);
        activity.insert("alpha:new".to_string(), 300);
        activity.insert("alpha:mid".to_string(), 200);
        data.activity = activity;

        merge_refresh(&mut entries, &mut notifications, data, 0, 0);

        let names: Vec<&str> = entries[0]
            .sessions
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(names, vec!["alpha:new", "alpha:mid", "alpha:old"]);
    }

    #[test]
    fn session_added_preserves_cursor() {
        let mut entries = vec![make_entry("alpha", &["main"])];
        let mut notifications = HashSet::new();
        let data = make_refresh(vec![("alpha", vec!["main", "dev"])]);

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
        let data = make_refresh(vec![("alpha", vec!["main", "dev"]), ("beta", vec!["main"])]);

        let result = merge_refresh(&mut entries, &mut notifications, data, 1, 0);

        assert_eq!(result.selected_project, 1);
        assert_eq!(result.selected_session, 0);
        assert_eq!(entries[0].sessions.len(), 2);
        assert_eq!(entries[1].sessions.len(), 1);
    }
}
