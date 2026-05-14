use zigzag_core::domain::{sanitize_branch_name, Session, Worktree};

/// Find sessions that have no corresponding worktree.
///
/// A session is considered orphaned when none of the known worktrees for
/// the same project has a branch that normalizes (via `sanitize_branch_name`)
/// to the session's branch name.
pub fn find_orphaned_sessions(sessions: &[Session], worktrees: &[Worktree]) -> Vec<Session> {
    sessions
        .iter()
        .filter(|session| {
            let session_normalized = sanitize_branch_name(&session.branch);
            !worktrees
                .iter()
                .any(|wt| sanitize_branch_name(&wt.branch) == session_normalized)
        })
        .cloned()
        .collect()
}

/// Find worktrees that have no corresponding active session.
///
/// Main/master branch worktrees are always excluded — they represent the
/// project root and cannot be pruned via `wt remove`.
pub fn find_orphaned_worktrees(worktrees: &[Worktree], sessions: &[Session]) -> Vec<Worktree> {
    worktrees
        .iter()
        .filter(|wt| {
            if wt.branch == "main" || wt.branch == "master" {
                return false;
            }
            let normalized = sanitize_branch_name(&wt.branch);
            !sessions
                .iter()
                .any(|s| sanitize_branch_name(&s.branch) == normalized)
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_session(project: &str, branch: &str) -> Session {
        Session {
            name: format!("{}:{}", project, branch),
            project: project.to_string(),
            branch: branch.to_string(),
        }
    }

    fn make_worktree(project: &str, branch: &str) -> Worktree {
        Worktree {
            path: PathBuf::from(format!("/tmp/{}/{}", project, branch)),
            branch: branch.to_string(),
            project: project.to_string(),
        }
    }

    // --- orphaned sessions ---

    #[test]
    fn orphaned_session_when_no_worktrees_exist() {
        let sessions = vec![make_session("proj", "feat-login")];
        let worktrees = vec![];
        let orphans = find_orphaned_sessions(&sessions, &worktrees);
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].branch, "feat-login");
    }

    #[test]
    fn session_with_exact_matching_worktree_not_orphaned() {
        let sessions = vec![make_session("proj", "main")];
        let worktrees = vec![make_worktree("proj", "main")];
        let orphans = find_orphaned_sessions(&sessions, &worktrees);
        assert!(orphans.is_empty());
    }

    #[test]
    fn session_with_normalized_matching_worktree_not_orphaned() {
        // Session stores "feat-login" (dash), worktree stores "feat/login" (slash).
        let sessions = vec![make_session("proj", "feat-login")];
        let worktrees = vec![make_worktree("proj", "feat/login")];
        let orphans = find_orphaned_sessions(&sessions, &worktrees);
        assert!(orphans.is_empty());
    }

    #[test]
    fn only_orphaned_sessions_returned_in_mixed_list() {
        let sessions = vec![
            make_session("proj", "feat-login"),  // has matching worktree
            make_session("proj", "stale-branch"), // no worktree
        ];
        let worktrees = vec![make_worktree("proj", "feat/login")];
        let orphans = find_orphaned_sessions(&sessions, &worktrees);
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].branch, "stale-branch");
    }

    #[test]
    fn empty_sessions_returns_empty_orphaned_sessions() {
        let worktrees = vec![make_worktree("proj", "main")];
        let orphans = find_orphaned_sessions(&[], &worktrees);
        assert!(orphans.is_empty());
    }

    #[test]
    fn all_sessions_orphaned_when_worktrees_empty() {
        let sessions = vec![
            make_session("proj", "feat-a"),
            make_session("proj", "feat-b"),
        ];
        let orphans = find_orphaned_sessions(&sessions, &[]);
        assert_eq!(orphans.len(), 2);
    }

    #[test]
    fn session_branch_deep_slash_normalization() {
        // feat/user/auth → feat-user-auth
        let sessions = vec![make_session("proj", "feat-user-auth")];
        let worktrees = vec![make_worktree("proj", "feat/user/auth")];
        let orphans = find_orphaned_sessions(&sessions, &worktrees);
        assert!(orphans.is_empty());
    }

    // --- orphaned worktrees ---

    #[test]
    fn orphaned_worktree_when_no_sessions_exist() {
        let worktrees = vec![make_worktree("proj", "feat/login")];
        let orphans = find_orphaned_worktrees(&worktrees, &[]);
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].branch, "feat/login");
    }

    #[test]
    fn worktree_with_matching_session_not_orphaned() {
        let worktrees = vec![make_worktree("proj", "feat/login")];
        let sessions = vec![make_session("proj", "feat-login")];
        let orphans = find_orphaned_worktrees(&worktrees, &sessions);
        assert!(orphans.is_empty());
    }

    #[test]
    fn main_worktree_never_orphaned() {
        let worktrees = vec![make_worktree("proj", "main")];
        let orphans = find_orphaned_worktrees(&worktrees, &[]);
        assert!(orphans.is_empty());
    }

    #[test]
    fn master_worktree_never_orphaned() {
        let worktrees = vec![make_worktree("proj", "master")];
        let orphans = find_orphaned_worktrees(&worktrees, &[]);
        assert!(orphans.is_empty());
    }

    #[test]
    fn only_orphaned_worktrees_returned_in_mixed_list() {
        let worktrees = vec![
            make_worktree("proj", "feat/login"),  // has session
            make_worktree("proj", "old/feature"), // no session
        ];
        let sessions = vec![make_session("proj", "feat-login")];
        let orphans = find_orphaned_worktrees(&worktrees, &sessions);
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].branch, "old/feature");
    }

    #[test]
    fn empty_worktrees_returns_empty_orphaned_worktrees() {
        let sessions = vec![make_session("proj", "main")];
        let orphans = find_orphaned_worktrees(&[], &sessions);
        assert!(orphans.is_empty());
    }

    #[test]
    fn all_worktrees_orphaned_when_sessions_empty_excluding_main() {
        let worktrees = vec![
            make_worktree("proj", "main"),
            make_worktree("proj", "feat/a"),
            make_worktree("proj", "fix/b"),
        ];
        let orphans = find_orphaned_worktrees(&worktrees, &[]);
        assert_eq!(orphans.len(), 2);
        assert!(orphans.iter().all(|w| w.branch != "main"));
    }

    #[test]
    fn worktree_deep_slash_branch_matched_by_session() {
        let worktrees = vec![make_worktree("proj", "feat/user/auth")];
        let sessions = vec![make_session("proj", "feat-user-auth")];
        let orphans = find_orphaned_worktrees(&worktrees, &sessions);
        assert!(orphans.is_empty());
    }

    // --- edge cases: session.branch may contain slashes (Session::new stores raw) ---

    #[test]
    fn session_with_unsanitized_slash_branch_matches_worktree() {
        // Session::new stores the original branch with slashes; the prune
        // function must normalize both sides to avoid a false orphan.
        let sessions = vec![Session {
            name: "proj:feat-login".to_string(),
            project: "proj".to_string(),
            branch: "feat/login".to_string(), // raw, as Session::new stores it
        }];
        let worktrees = vec![make_worktree("proj", "feat/login")];
        let orphans = find_orphaned_sessions(&sessions, &worktrees);
        assert!(orphans.is_empty());
    }

    #[test]
    fn worktree_matches_session_with_unsanitized_slash_branch() {
        let sessions = vec![Session {
            name: "proj:feat-login".to_string(),
            project: "proj".to_string(),
            branch: "feat/login".to_string(), // raw slash form
        }];
        let worktrees = vec![make_worktree("proj", "feat/login")];
        let orphans = find_orphaned_worktrees(&worktrees, &sessions);
        assert!(orphans.is_empty());
    }

    // --- both inputs empty ---

    #[test]
    fn both_empty_returns_no_orphaned_sessions() {
        assert!(find_orphaned_sessions(&[], &[]).is_empty());
    }

    #[test]
    fn both_empty_returns_no_orphaned_worktrees() {
        assert!(find_orphaned_worktrees(&[], &[]).is_empty());
    }

    // --- worktree with already-dashed branch (no slashes) ---

    #[test]
    fn worktree_with_dashed_branch_matches_dashed_session() {
        let worktrees = vec![make_worktree("proj", "feat-login")];
        let sessions = vec![make_session("proj", "feat-login")];
        let orphans = find_orphaned_worktrees(&worktrees, &sessions);
        assert!(orphans.is_empty());
    }

    #[test]
    fn session_with_dashed_branch_matches_dashed_worktree() {
        let sessions = vec![make_session("proj", "feat-login")];
        let worktrees = vec![make_worktree("proj", "feat-login")];
        let orphans = find_orphaned_sessions(&sessions, &worktrees);
        assert!(orphans.is_empty());
    }
}
