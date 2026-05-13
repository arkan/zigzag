use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use crate::domain::*;

/// Assemble `WorktreeEntry` values from a project, discovered worktrees,
/// active sessions, metadata records, and optional safety info.
///
/// This is the central topology function that:
/// - Classifies each discovered worktree as Active / Inactive / Conflict / Unsupported
/// - Attaches matching sessions, collisions, diagnostics, safety, and metadata
/// - Sorts entries according to the dashboard ordering rules
pub fn assemble_worktree_entries(
    _project: &Project,
    discovered_worktrees: Vec<DiscoveredWorktree>,
    active_sessions: &[Session],
    metadata_records: &[WorktreeMetadataRecord],
    safety_map: HashMap<WorktreeIdentity, GitSafetyStatus>,
) -> Vec<WorktreeEntry> {
    let session_map: HashMap<&str, &Session> = active_sessions
        .iter()
        .map(|s| (s.name.as_str(), s))
        .collect();

    let metadata_map: HashMap<WorktreeIdentity, WorktreeMetadata> = metadata_records
        .iter()
        .map(|r| {
            let id = WorktreeIdentity {
                host: r.host.clone(),
                project_root: r.project_root.clone(),
                worktree_path: r.path.clone(),
            };
            let meta = WorktreeMetadata {
                last_opened_at: r.last_opened_at,
                last_session_name: r.last_session_name.clone(),
            };
            (id, meta)
        })
        .collect();

    // First pass: build entries without collision info
    let mut entries: Vec<WorktreeEntry> = discovered_worktrees
        .into_iter()
        .map(|wt| {
            let (status, session_link, diagnostics) = compute_basic_status(&wt, &session_map);
            let safety = safety_map.get(&wt.identity).cloned();
            let metadata = metadata_map.get(&wt.identity).cloned();
            WorktreeEntry {
                discovered: wt,
                status,
                diagnostics,
                safety,
                session_link,
                metadata,
            }
        })
        .collect();

    // Second pass: detect collisions and upgrade affected entries
    apply_collision_diagnostics(&mut entries);

    sort_worktree_entries(&mut entries);
    entries
}

/// Determine basic status (Unsupported / Active / Inactive) and session link.
/// Collision detection is handled in a separate pass.
fn compute_basic_status(
    wt: &DiscoveredWorktree,
    session_map: &HashMap<&str, &Session>,
) -> (WorktreeStatus, SessionLink, Vec<WorktreeDiagnostic>) {
    // Detached / no-branch worktrees are always Unsupported
    if wt.branch.is_none() {
        return (
            WorktreeStatus::Unsupported,
            SessionLink::None,
            vec![WorktreeDiagnostic::DetachedHead],
        );
    }

    let branch = wt.branch.as_deref().unwrap();
    let session_name = derive_session_name(&wt.project_name, branch);

    // Active session match
    if let Some(session) = session_map.get(session_name.as_str()) {
        return (
            WorktreeStatus::Active,
            SessionLink::Active((*session).clone()),
            Vec::new(),
        );
    }

    (WorktreeStatus::Inactive, SessionLink::None, Vec::new())
}

/// Detect session-name collisions and mark affected entries as Conflict.
///
/// Two or more worktrees whose branches produce the same derived session
/// name (via `/` → `-` normalization) are set to `WorktreeStatus::Conflict`
/// with a `SessionNameCollision` diagnostic.
fn apply_collision_diagnostics(entries: &mut [WorktreeEntry]) {
    // Group worktree indices by derived session name
    let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, entry) in entries.iter().enumerate() {
        if let Some(ref branch) = entry.discovered.branch {
            let name = derive_session_name(&entry.discovered.project_name, branch);
            groups.entry(name).or_default().push(i);
        }
    }

    // Mark groups with size > 1 as collisions
    for (_session_name, indices) in groups {
        if indices.len() <= 1 {
            continue;
        }

        let colliding: Vec<DiscoveredWorktree> = indices
            .iter()
            .map(|i| entries[*i].discovered.clone())
            .collect();
        let branch_names: Vec<String> = colliding
            .iter()
            .filter_map(|w| w.branch.as_deref().map(String::from))
            .collect();

        for idx in &indices {
            let entry = &mut entries[*idx];
            entry.status = WorktreeStatus::Conflict;
            entry.session_link = SessionLink::Collision(colliding.clone());
            entry
                .diagnostics
                .push(WorktreeDiagnostic::SessionNameCollision(
                    branch_names.clone(),
                ));
        }
    }
}

/// Sort worktree entries per the dashboard ordering rules:
///
/// 1. Active sessions, sorted by recent activity (`last_opened_at`) descending.
/// 2. Inactive worktrees with recent activity (have `last_opened_at`).
/// 3. Primary checkout (if still inactive).
/// 4. Branch name alphabetically, then path alphabetically.
#[allow(clippy::ptr_arg)]
pub fn sort_worktree_entries(entries: &mut Vec<WorktreeEntry>) {
    sort_worktree_entries_slice(entries.as_mut_slice());
}

fn sort_worktree_entries_slice(entries: &mut [WorktreeEntry]) {
    entries.sort_by(|a, b| {
        // Active vs inactive
        let a_is_active = matches!(a.status, WorktreeStatus::Active);
        let b_is_active = matches!(b.status, WorktreeStatus::Active);
        if a_is_active != b_is_active {
            return b_is_active.cmp(&a_is_active); // active first
        }

        // Within active: by last_opened_at descending
        if a_is_active {
            let a_time = a
                .metadata
                .as_ref()
                .and_then(|m| m.last_opened_at)
                .unwrap_or(0);
            let b_time = b
                .metadata
                .as_ref()
                .and_then(|m| m.last_opened_at)
                .unwrap_or(0);
            return b_time.cmp(&a_time);
        }

        // Both inactive: with recent activity first
        let a_has_activity = a.metadata.as_ref().and_then(|m| m.last_opened_at).is_some();
        let b_has_activity = b.metadata.as_ref().and_then(|m| m.last_opened_at).is_some();
        if a_has_activity != b_has_activity {
            return b_has_activity.cmp(&a_has_activity);
        }

        // Primary checkout comes first (true < false in sort order)
        if a.discovered.is_primary_checkout != b.discovered.is_primary_checkout {
            return b
                .discovered
                .is_primary_checkout
                .cmp(&a.discovered.is_primary_checkout);
        }

        // Alpha by branch name
        let a_branch = a.discovered.branch.as_deref().unwrap_or("");
        let b_branch = b.discovered.branch.as_deref().unwrap_or("");
        match a_branch.cmp(b_branch) {
            Ordering::Equal => {}
            non_eq => return non_eq,
        }

        // Then by worktree path
        a.discovered
            .identity
            .worktree_path
            .cmp(&b.discovered.identity.worktree_path)
    });
}

/// Find sessions whose derived session name does not match any discovered
/// worktree. These are "orphan" sessions — active Zellij sessions whose
/// worktree is missing or was deleted.
pub fn find_orphan_sessions<'a>(
    sessions: &'a [Session],
    worktrees: &[DiscoveredWorktree],
) -> Vec<&'a Session> {
    let wt_names: HashSet<String> = worktrees
        .iter()
        .filter_map(|wt| {
            wt.branch
                .as_ref()
                .map(|b| derive_session_name(&wt.project_name, b))
        })
        .collect();

    sessions
        .iter()
        .filter(|s| !wt_names.contains(&s.name))
        .collect()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ---- Helpers ----

    fn make_project(name: &str, path: &str) -> Project {
        Project {
            name: name.to_string(),
            path: PathBuf::from(path),
            host: None,
            transport: None,
        }
    }

    fn make_wt(
        proj: &str,
        proj_root: &str,
        wt_path: &str,
        branch: Option<&str>,
        is_primary: bool,
    ) -> DiscoveredWorktree {
        DiscoveredWorktree {
            identity: WorktreeIdentity {
                host: None,
                project_root: PathBuf::from(proj_root),
                worktree_path: PathBuf::from(wt_path),
            },
            project_name: proj.to_string(),
            branch: branch.map(String::from),
            is_primary_checkout: is_primary,
        }
    }

    fn make_meta(
        proj: &str,
        root: &str,
        path: &str,
        last_opened: Option<u64>,
    ) -> WorktreeMetadataRecord {
        WorktreeMetadataRecord {
            project_name: proj.to_string(),
            project_root: PathBuf::from(root),
            host: None,
            branch: None,
            path: PathBuf::from(path),
            last_opened_at: last_opened,
            last_session_name: None,
        }
    }

    // ---- Active session match ----

    #[test]
    fn active_session_match() {
        let project = make_project("myapp", "/repo");
        let worktrees = vec![make_wt(
            "myapp",
            "/repo",
            "/repo/.worktrees/feat-login",
            Some("feat/login"),
            false,
        )];
        let sessions = vec![Session::new("myapp", "feat/login")];
        let metadata = vec![];
        let safety = HashMap::new();

        let entries = assemble_worktree_entries(&project, worktrees, &sessions, &metadata, safety);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, WorktreeStatus::Active);
        assert!(matches!(entries[0].session_link, SessionLink::Active(_)));
        assert!(entries[0].diagnostics.is_empty());
    }

    #[test]
    fn active_matches_by_session_name() {
        // Verify the session name is matched even though the derived name
        // differs from the raw branch (feat/x vs feat-x session name)
        let project = make_project("myapp", "/repo");
        let worktrees = vec![make_wt(
            "myapp",
            "/repo",
            "/repo/.worktrees/feat-x",
            Some("feat/x"),
            false,
        )];
        let sessions = vec![Session::new("myapp", "feat/x")];
        let metadata = vec![];
        let safety = HashMap::new();

        let entries = assemble_worktree_entries(&project, worktrees, &sessions, &metadata, safety);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, WorktreeStatus::Active);
    }

    // ---- Inactive ----

    #[test]
    fn inactive_no_session() {
        let project = make_project("myapp", "/repo");
        let worktrees = vec![make_wt(
            "myapp",
            "/repo",
            "/repo/.worktrees/feat-login",
            Some("feat/login"),
            false,
        )];
        let sessions = vec![];
        let metadata = vec![];
        let safety = HashMap::new();

        let entries = assemble_worktree_entries(&project, worktrees, &sessions, &metadata, safety);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, WorktreeStatus::Inactive);
        assert_eq!(entries[0].session_link, SessionLink::None);
        assert!(entries[0].diagnostics.is_empty());
    }

    // ---- Collision detection ----

    #[test]
    fn collision_between_slash_and_dash_branches() {
        let project = make_project("myapp", "/repo");
        let worktrees = vec![
            make_wt(
                "myapp",
                "/repo",
                "/repo/.worktrees/feat-login",
                Some("feat/login"),
                false,
            ),
            make_wt(
                "myapp",
                "/repo",
                "/repo/.worktrees/feat-login-2",
                Some("feat-login"),
                false,
            ),
        ];
        let sessions = vec![];
        let metadata = vec![];
        let safety = HashMap::new();

        let entries = assemble_worktree_entries(&project, worktrees, &sessions, &metadata, safety);
        assert_eq!(entries.len(), 2);
        for entry in &entries {
            assert_eq!(
                entry.status,
                WorktreeStatus::Conflict,
                "expected Conflict for branch {:?}",
                entry.discovered.branch
            );
            assert!(
                matches!(entry.session_link, SessionLink::Collision(_)),
                "expected Collision link for branch {:?}",
                entry.discovered.branch
            );
            assert!(
                entry
                    .diagnostics
                    .iter()
                    .any(|d| matches!(d, WorktreeDiagnostic::SessionNameCollision(_))),
                "expected SessionNameCollision diagnostic for {:?}",
                entry.discovered.branch
            );
        }
    }

    #[test]
    fn collision_overrides_active() {
        // Even if one of the colliding worktrees has an active session,
        // both should show Conflict.
        let project = make_project("myapp", "/repo");
        let worktrees = vec![
            make_wt(
                "myapp",
                "/repo",
                "/repo/.worktrees/feat-login",
                Some("feat/login"),
                false,
            ),
            make_wt(
                "myapp",
                "/repo",
                "/repo/.worktrees/feat-login-2",
                Some("feat-login"),
                false,
            ),
        ];
        let sessions = vec![Session::new("myapp", "feat/login")];
        let metadata = vec![];
        let safety = HashMap::new();

        let entries = assemble_worktree_entries(&project, worktrees, &sessions, &metadata, safety);
        assert_eq!(entries.len(), 2);
        for entry in &entries {
            assert_eq!(entry.status, WorktreeStatus::Conflict);
        }
    }

    #[test]
    fn no_collision_for_distinct_branches() {
        let project = make_project("myapp", "/repo");
        let worktrees = vec![
            make_wt(
                "myapp",
                "/repo",
                "/repo/.worktrees/feat-a",
                Some("feat/a"),
                false,
            ),
            make_wt(
                "myapp",
                "/repo",
                "/repo/.worktrees/feat-b",
                Some("feat/b"),
                false,
            ),
        ];
        let sessions = vec![];
        let metadata = vec![];
        let safety = HashMap::new();

        let entries = assemble_worktree_entries(&project, worktrees, &sessions, &metadata, safety);
        assert_eq!(entries.len(), 2);
        for entry in &entries {
            assert_eq!(entry.status, WorktreeStatus::Inactive);
        }
    }

    // ---- Unsupported / detached head ----

    #[test]
    fn unsupported_detached_head() {
        let project = make_project("myapp", "/repo");
        let worktrees = vec![make_wt(
            "myapp",
            "/repo",
            "/repo/.worktrees/detached",
            None,
            false,
        )];
        let sessions = vec![];
        let metadata = vec![];
        let safety = HashMap::new();

        let entries = assemble_worktree_entries(&project, worktrees, &sessions, &metadata, safety);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, WorktreeStatus::Unsupported);
        assert!(entries[0]
            .diagnostics
            .contains(&WorktreeDiagnostic::DetachedHead));
        assert_eq!(entries[0].session_link, SessionLink::None);
    }

    #[test]
    fn primary_checkout_with_branch_is_supported() {
        let project = make_project("myapp", "/repo");
        let worktrees = vec![make_wt(
            "myapp",
            "/repo",
            "/repo",
            Some("main"),
            true, // is_primary
        )];
        let sessions = vec![];
        let metadata = vec![];
        let safety = HashMap::new();

        let entries = assemble_worktree_entries(&project, worktrees, &sessions, &metadata, safety);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, WorktreeStatus::Inactive);
        assert!(entries[0].discovered.is_primary_checkout);
    }

    // ---- Metadata attachment ----

    #[test]
    fn metadata_attached_by_identity() {
        let project = make_project("myapp", "/repo");
        let wt_path = "/repo/.worktrees/feat-x";
        let worktrees = vec![make_wt("myapp", "/repo", wt_path, Some("feat/x"), false)];
        let metadata = vec![make_meta("myapp", "/repo", wt_path, Some(1_710_000_000))];
        let sessions = vec![];
        let safety = HashMap::new();

        let entries = assemble_worktree_entries(&project, worktrees, &sessions, &metadata, safety);
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].metadata.as_ref().unwrap().last_opened_at,
            Some(1_710_000_000)
        );
    }

    #[test]
    fn metadata_not_found_when_identity_differs() {
        let project = make_project("myapp", "/repo");
        let worktrees = vec![make_wt(
            "myapp",
            "/repo",
            "/repo/.worktrees/feat-x",
            Some("feat/x"),
            false,
        )];
        // Metadata for a different path
        let metadata = vec![make_meta(
            "myapp",
            "/repo",
            "/repo/.worktrees/other",
            Some(999),
        )];
        let sessions = vec![];
        let safety = HashMap::new();

        let entries = assemble_worktree_entries(&project, worktrees, &sessions, &metadata, safety);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].metadata.is_none());
    }

    // ---- Sorting ----

    #[test]
    fn sort_active_before_inactive() {
        let project = make_project("myapp", "/repo");
        let wt_path_a = "/repo/.worktrees/active-wt";
        let wt_path_b = "/repo/.worktrees/inactive-wt";
        let worktrees = vec![
            make_wt("myapp", "/repo", wt_path_b, Some("inactive-wt"), false),
            make_wt("myapp", "/repo", wt_path_a, Some("active-wt"), false),
        ];
        let sessions = vec![Session::new("myapp", "active-wt")];
        let metadata = vec![];
        let safety = HashMap::new();

        let entries = assemble_worktree_entries(&project, worktrees, &sessions, &metadata, safety);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].status, WorktreeStatus::Active);
        assert_eq!(entries[1].status, WorktreeStatus::Inactive);
    }

    #[test]
    fn sort_active_by_recent_activity() {
        let project = make_project("myapp", "/repo");
        let worktrees = vec![
            make_wt("myapp", "/repo", "/repo/.worktrees/old", Some("old"), false),
            make_wt("myapp", "/repo", "/repo/.worktrees/new", Some("new"), false),
        ];
        let sessions = vec![Session::new("myapp", "old"), Session::new("myapp", "new")];
        let metadata = vec![
            make_meta("myapp", "/repo", "/repo/.worktrees/old", Some(100)),
            make_meta("myapp", "/repo", "/repo/.worktrees/new", Some(200)),
        ];
        let safety = HashMap::new();

        let entries = assemble_worktree_entries(&project, worktrees, &sessions, &metadata, safety);
        assert_eq!(entries.len(), 2);
        // newest activity first
        assert_eq!(entries[0].discovered.branch.as_deref(), Some("new"));
        assert_eq!(entries[1].discovered.branch.as_deref(), Some("old"));
    }

    #[test]
    fn sort_inactive_with_activity_before_no_activity() {
        let project = make_project("myapp", "/repo");
        let worktrees = vec![
            make_wt(
                "myapp",
                "/repo",
                "/repo/.worktrees/no-activity",
                Some("no-activity"),
                false,
            ),
            make_wt(
                "myapp",
                "/repo",
                "/repo/.worktrees/has-activity",
                Some("has-activity"),
                false,
            ),
        ];
        let sessions = vec![];
        let metadata = vec![make_meta(
            "myapp",
            "/repo",
            "/repo/.worktrees/has-activity",
            Some(500),
        )];
        let safety = HashMap::new();

        let entries = assemble_worktree_entries(&project, worktrees, &sessions, &metadata, safety);
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].discovered.branch.as_deref(),
            Some("has-activity")
        );
        assert_eq!(entries[1].discovered.branch.as_deref(), Some("no-activity"));
    }

    #[test]
    fn sort_primary_checkout_before_other_inactive() {
        let project = make_project("myapp", "/repo");
        let worktrees = vec![
            make_wt(
                "myapp",
                "/repo",
                "/repo/.worktrees/other",
                Some("other"),
                false,
            ),
            make_wt("myapp", "/repo", "/repo", Some("main"), true),
        ];
        let sessions = vec![];
        let metadata = vec![];
        let safety = HashMap::new();

        let entries = assemble_worktree_entries(&project, worktrees, &sessions, &metadata, safety);
        assert_eq!(entries.len(), 2);
        assert!(entries[0].discovered.is_primary_checkout);
        assert!(!entries[1].discovered.is_primary_checkout);
    }

    #[test]
    fn sort_inactive_by_branch_alpha() {
        let project = make_project("myapp", "/repo");
        let worktrees = vec![
            make_wt("myapp", "/repo", "/repo/.worktrees/zzz", Some("zzz"), false),
            make_wt("myapp", "/repo", "/repo/.worktrees/aaa", Some("aaa"), false),
        ];
        let sessions = vec![];
        let metadata = vec![];
        let safety = HashMap::new();

        let entries = assemble_worktree_entries(&project, worktrees, &sessions, &metadata, safety);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].discovered.branch.as_deref(), Some("aaa"));
        assert_eq!(entries[1].discovered.branch.as_deref(), Some("zzz"));
    }

    // ---- Orphan session detection ----

    #[test]
    fn find_orphan_sessions_detects_orphans() {
        let sessions = vec![Session::new("myapp", "orphaned-branch")];
        let worktrees = vec![make_wt("myapp", "/repo", "/repo", Some("main"), true)];

        let orphans = find_orphan_sessions(&sessions, &worktrees);
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].name, "myapp:orphaned-branch");
    }

    #[test]
    fn find_orphan_sessions_no_orphans() {
        let sessions = vec![Session::new("myapp", "main")];
        let worktrees = vec![make_wt("myapp", "/repo", "/repo", Some("main"), true)];

        let orphans = find_orphan_sessions(&sessions, &worktrees);
        assert!(orphans.is_empty());
    }

    #[test]
    fn find_orphan_sessions_ignores_detached() {
        // Detached worktrees have no branch, so they don't contribute
        // to the set of valid session names.
        let sessions = vec![Session::new("myapp", "main")];
        let worktrees = vec![make_wt("myapp", "/repo", "/repo/detached", None, false)];

        let orphans = find_orphan_sessions(&sessions, &worktrees);
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].name, "myapp:main");
    }

    // ---- Safety attachment ----

    #[test]
    fn safety_attached_by_identity() {
        let project = make_project("myapp", "/repo");
        let id = WorktreeIdentity {
            host: None,
            project_root: PathBuf::from("/repo"),
            worktree_path: PathBuf::from("/repo/.worktrees/feat-x"),
        };
        let worktrees = vec![make_wt(
            "myapp",
            "/repo",
            "/repo/.worktrees/feat-x",
            Some("feat/x"),
            false,
        )];
        let safety: HashMap<WorktreeIdentity, GitSafetyStatus> = [(
            id.clone(),
            GitSafetyStatus {
                dirty: true,
                ahead: 3,
                behind: 1,
                has_upstream: true,
            },
        )]
        .into_iter()
        .collect();
        let sessions = vec![];
        let metadata = vec![];

        let entries = assemble_worktree_entries(&project, worktrees, &sessions, &metadata, safety);
        assert_eq!(entries.len(), 1);
        let s = entries[0].safety.as_ref().unwrap();
        assert!(s.dirty);
        assert_eq!(s.ahead, 3);
        assert_eq!(s.behind, 1);
        assert!(s.has_upstream);
    }

    // ---- Collision + orphan interaction ----

    #[test]
    fn colliding_worktrees_not_orphaned_by_each_other() {
        // Both colliding worktrees produce the same session name, which means
        // that session name IS covered. No orphan for that name.
        let sessions = vec![Session::new("myapp", "feat/login")];
        let worktrees = vec![
            make_wt(
                "myapp",
                "/repo",
                "/repo/.worktrees/feat-login",
                Some("feat/login"),
                false,
            ),
            make_wt(
                "myapp",
                "/repo",
                "/repo/.worktrees/feat-login-2",
                Some("feat-login"),
                false,
            ),
        ];

        let orphans = find_orphan_sessions(&sessions, &worktrees);
        assert!(
            orphans.is_empty(),
            "colliding worktrees should cover the session name"
        );
    }
}
