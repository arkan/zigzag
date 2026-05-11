use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Transport protocol for remote connections.
#[derive(Debug, Clone, PartialEq)]
pub enum Transport {
    Ssh,
    Mosh,
}

/// A project managed by z.
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub name: String,
    pub path: PathBuf,
    /// SSH host for remote projects (e.g. `"vps"`, `"user@vps"`).
    pub host: Option<String>,
    /// Transport protocol for interactive sessions (`ssh` default, `mosh` for iOS).
    pub transport: Option<Transport>,
}

/// A Zellij session, named `{project}:{branch}` (slashes in branch replaced by `-`).
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    /// Full session name, e.g. `myapp:feat-login`.
    pub name: String,
    pub project: String,
    pub branch: String,
}

impl Session {
    pub fn new(project: &str, branch: &str) -> Self {
        let normalized = sanitize_branch_name(branch);
        Self {
            name: format!("{}:{}", project, normalized),
            project: project.to_string(),
            branch: branch.to_string(),
        }
    }
}

/// A git worktree managed by worktrunk (`wt`).
#[derive(Debug, Clone, PartialEq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    pub project: String,
}

/// A GitHub pull request.
#[derive(Debug, Clone, PartialEq)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub state: PrState,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

/// CI run status.
#[derive(Debug, Clone, PartialEq)]
pub enum CiStatus {
    Passing,
    Failing,
    Pending,
    Unknown,
}

/// A Zellij session layout (tabs + panes).
#[derive(Debug, Clone)]
pub struct Layout {
    pub tabs: Vec<Tab>,
    /// Optional working directory for the session. When set, all panes open in this directory.
    pub cwd: Option<PathBuf>,
    /// Optional session name to set as `Z_SESSION_NAME` in the layout env block,
    /// so child processes (e.g. OpenCode plugins) can identify the session.
    pub session_name_env: Option<String>,
}

/// Normalize a branch name to a session-safe string (replace `/` with `-`).
pub fn sanitize_branch_name(branch: &str) -> String {
    branch.replace('/', "-")
}

/// Convert a title into a URL/branch-safe slug.
///
/// Lowercase, non-ASCII-alphanumeric replaced with `-`, consecutive dashes
/// collapsed, trimmed to 40 chars, trailing `-` removed.
pub fn slugify(title: &str) -> String {
    let raw: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    // Collapse consecutive dashes
    let mut slug = String::with_capacity(raw.len());
    let mut prev_dash = false;
    for c in raw.chars() {
        if c == '-' {
            if !prev_dash && !slug.is_empty() {
                slug.push('-');
            }
            prev_dash = true;
        } else {
            slug.push(c);
            prev_dash = false;
        }
    }
    // Truncate to 40 chars
    if slug.len() > 40 {
        slug.truncate(40);
    }
    slug.trim_end_matches('-').to_string()
}

#[derive(Debug, Clone)]
pub struct Tab {
    pub name: String,
    pub panes: Vec<Pane>,
}

#[derive(Debug, Clone)]
pub struct Pane {
    pub command: Option<String>,
    pub args: Vec<String>,
}

/// Review status for a pull request, used to evaluate `has_new_comments`.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewStatus {
    /// True if reviews/comments exist after the last pushed commit.
    pub has_new_comments: bool,
    /// Total number of review comments on the PR.
    pub comment_count: u32,
    /// ISO 8601 timestamp of the most recent review, if any.
    pub last_review_at: Option<String>,
}

/// Notification severity level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotifyLevel {
    Info,
    Warning,
    Error,
}

// =============================================================================
// Worktree Identity & Discovery (Phase 1)
// =============================================================================

/// Canonical identity for a worktree: host + project_root + worktree_path.
///
/// This is the durable identifier for all metadata, notifications, and
/// destructive actions. A Zellij session name is only a derived alias.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorktreeIdentity {
    pub host: Option<String>,
    pub project_root: PathBuf,
    pub worktree_path: PathBuf,
}

/// A worktree discovered from git worktree listing.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredWorktree {
    pub identity: WorktreeIdentity,
    pub project_name: String,
    pub branch: Option<String>,
    /// True when `worktree_path == project_root` (the primary checkout).
    pub is_primary_checkout: bool,
}

/// Git safety status for a worktree.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GitSafetyStatus {
    pub dirty: bool,
    pub ahead: u32,
    pub behind: u32,
    /// False when the branch has no upstream (distinct from ahead=0, behind=0).
    pub has_upstream: bool,
}

/// Dashboard status for a worktree.
#[derive(Debug, Clone, PartialEq)]
pub enum WorktreeStatus {
    /// A Zellij session currently exists.
    Active,
    /// Worktree is restorable, no session exists.
    Inactive,
    /// Session-name collision prevents automatic open/restore.
    Conflict,
    /// Detached/no-branch worktree, not auto-restorable.
    Unsupported,
}

/// Diagnostic conditions for a worktree.
#[derive(Debug, Clone, PartialEq)]
pub enum WorktreeDiagnostic {
    DetachedHead,
    SessionNameCollision(Vec<String>),
    StaleMetadataPath { stored: PathBuf, actual: PathBuf },
    StaleMetadataBranch { stored: String, actual: String },
    StaleProjectName { stored: String, actual: String },
    NoUpstream,
    RemoteUnavailable,
    MetadataUnavailable,
    SessionOrphan { session_name: String },
    UnattachedNotification { id: String },
}

/// Describes how a worktree relates to a Zellij session.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionLink {
    /// No session link established (worktree is inactive/unsupported).
    None,
    /// Exactly one active session maps to this worktree.
    Active(Session),
    /// Multiple worktrees resolve to the same session name.
    Collision(Vec<DiscoveredWorktree>),
    /// A session exists but its matching worktree was not found.
    Orphan(Session),
}

/// Result of resolving a session alias (name) against discovered worktrees.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionAliasResolution {
    Unique(DiscoveredWorktree),
    Ambiguous(Vec<DiscoveredWorktree>),
    None,
}

// =============================================================================
// Topology Assembly Types (Phase 2)
// =============================================================================

/// A fully assembled worktree entry combining discovery, status, diagnostics,
/// safety, session link, and metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct WorktreeEntry {
    pub discovered: DiscoveredWorktree,
    pub status: WorktreeStatus,
    pub diagnostics: Vec<WorktreeDiagnostic>,
    pub safety: Option<GitSafetyStatus>,
    pub session_link: SessionLink,
    pub metadata: Option<WorktreeMetadata>,
}

// =============================================================================
// Persistence Schema Types
// =============================================================================

/// Per-worktree metadata stored in `worktree-metadata.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorktreeMetadata {
    pub last_opened_at: Option<u64>,
    pub last_session_name: Option<String>,
}

/// Serialized record for one worktree in the metadata file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorktreeMetadataRecord {
    pub project_name: String,
    pub project_root: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The worktree path on disk (alias for `worktree_path` in identity).
    pub path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_opened_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_session_name: Option<String>,
}

/// A notification targeting a specific worktree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NotificationRecord {
    pub id: String,
    pub target: WorktreeIdentity,
    pub level: NotifyLevel,
    pub message: String,
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<NotificationSource>,
}

/// Structured source metadata for notifications created from agent activity events.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NotificationSource {
    pub tool: String,
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_resolve_key: Option<String>,
    #[serde(default)]
    pub auto_resolve: bool,
}

/// Active agent activity state stored in metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AgentActivityState {
    Working,
    Waiting,
}

/// A sparse status record for one agent tool on one Worktree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentActivityStatus {
    pub target: WorktreeIdentity,
    pub tool: String,
    pub state: AgentActivityState,
    pub updated_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_resolve_key: Option<String>,
}

/// A notification whose target session name could not be resolved to a worktree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UnattachedNotification {
    pub id: String,
    pub session_name: String,
    pub level: NotifyLevel,
    pub message: String,
    pub created_at: u64,
}

/// Activity record whose session name could not be resolved to a worktree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UnattachedActivity {
    pub session_name: String,
    pub last_attached_at: u64,
}

/// Top-level schema for `~/.config/z/worktree-metadata.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorktreeMetadataFile {
    pub version: u32,
    #[serde(default)]
    pub worktrees: Vec<WorktreeMetadataRecord>,
    #[serde(default)]
    pub notifications: Vec<NotificationRecord>,
    #[serde(default)]
    pub unattached_notifications: Vec<UnattachedNotification>,
    #[serde(default)]
    pub unattached_activity: Vec<UnattachedActivity>,
    #[serde(default)]
    pub migration_diagnostics: Vec<String>,
    #[serde(default)]
    pub llm_status: Vec<AgentActivityStatus>,
    /// IDs of already-migrated legacy notification files to prevent duplicate
    /// migration across repeated `drain_legacy_notifications` calls.
    /// Format: `"{session_name}/{filename}"`.
    #[serde(default)]
    pub migrated_legacy_ids: std::collections::HashSet<String>,
}

// =============================================================================
// Session Name Helpers
// =============================================================================

/// Derive a Zellij session name from project + branch.
///
/// This is the forward transformation; it applies `sanitize_branch_name` (replaces
/// `/` with `-`) internally. The result is non-injective because different branches
/// like `feat/login` and `feat-login` produce the same session name.
pub fn derive_session_name(project: &str, branch: &str) -> String {
    Session::new(project, branch).name
}

/// Resolve a session alias (format `project:branch`) to discovered worktrees.
///
/// Returns:
/// - `Unique(worktree)` – exactly one discovered worktree maps to this session name.
/// - `Ambiguous(list)` – multiple worktrees map to it (collision from `/` → `-`).
/// - `None` – no worktree maps to this session name.
pub fn resolve_session_alias(
    session_name: &str,
    worktrees: &[DiscoveredWorktree],
) -> SessionAliasResolution {
    let matching: Vec<&DiscoveredWorktree> = worktrees
        .iter()
        .filter(|wt| {
            let branch = wt.branch.as_deref().unwrap_or("HEAD");
            let derived = derive_session_name(&wt.project_name, branch);
            derived == session_name
        })
        .collect();

    match matching.len() {
        0 => SessionAliasResolution::None,
        1 => SessionAliasResolution::Unique(matching[0].clone()),
        _ => SessionAliasResolution::Ambiguous(matching.into_iter().cloned().collect()),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_plain_branch() {
        assert_eq!(sanitize_branch_name("main"), "main");
    }

    #[test]
    fn sanitize_slash_to_dash() {
        assert_eq!(sanitize_branch_name("feat/login"), "feat-login");
    }

    #[test]
    fn sanitize_multiple_slashes() {
        assert_eq!(sanitize_branch_name("feat/user/auth"), "feat-user-auth");
    }

    #[test]
    fn sanitize_no_slashes_unchanged() {
        assert_eq!(sanitize_branch_name("fix-bug-123"), "fix-bug-123");
    }

    #[test]
    fn sanitize_empty_string() {
        assert_eq!(sanitize_branch_name(""), "");
    }

    #[test]
    fn session_new_normalizes_slash() {
        let s = Session::new("myapp", "feat/login");
        assert_eq!(s.name, "myapp:feat-login");
        assert_eq!(s.branch, "feat/login");
        assert_eq!(s.project, "myapp");
    }

    #[test]
    fn session_new_main() {
        let s = Session::new("myapp", "main");
        assert_eq!(s.name, "myapp:main");
    }

    #[test]
    fn session_new_preserves_original_branch() {
        let s = Session::new("proj", "feat/a/b");
        assert_eq!(s.branch, "feat/a/b");
        assert_eq!(s.name, "proj:feat-a-b");
    }

    #[test]
    fn sanitize_leading_slash() {
        assert_eq!(sanitize_branch_name("/leading"), "-leading");
    }

    #[test]
    fn sanitize_trailing_slash() {
        assert_eq!(sanitize_branch_name("trailing/"), "trailing-");
    }

    #[test]
    fn sanitize_consecutive_slashes() {
        assert_eq!(sanitize_branch_name("a//b"), "a--b");
    }

    #[test]
    fn session_new_uses_sanitize_branch_name() {
        // Verify Session::new produces the same result as sanitize_branch_name
        let branch = "feat/complex/nested/branch";
        let s = Session::new("proj", branch);
        assert_eq!(s.name, format!("proj:{}", sanitize_branch_name(branch)));
    }

    // ---- slugify ----

    #[test]
    fn slugify_clean_title() {
        assert_eq!(slugify("add auth middleware"), "add-auth-middleware");
    }

    #[test]
    fn slugify_special_chars() {
        assert_eq!(slugify("fix: login bug (#42)"), "fix-login-bug-42");
    }

    #[test]
    fn slugify_long_title_truncated() {
        let long = "a]".repeat(30); // 60 chars worth of content
        let result = slugify(&long);
        assert!(result.len() <= 40);
        assert!(!result.ends_with('-'));
    }

    #[test]
    fn slugify_empty() {
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn slugify_already_clean() {
        assert_eq!(slugify("simple-slug"), "simple-slug");
    }

    #[test]
    fn slugify_unicode() {
        assert_eq!(slugify("café résumé"), "caf-r-sum");
    }

    #[test]
    fn slugify_consecutive_special_chars() {
        assert_eq!(slugify("hello---world"), "hello-world");
    }

    // ---- WorktreeIdentity ----

    #[test]
    fn worktree_identity_equality() {
        let a = WorktreeIdentity {
            host: None,
            project_root: PathBuf::from("/repo"),
            worktree_path: PathBuf::from("/repo/.worktrees/feat"),
        };
        let b = WorktreeIdentity {
            host: None,
            project_root: PathBuf::from("/repo"),
            worktree_path: PathBuf::from("/repo/.worktrees/feat"),
        };
        assert_eq!(a, b);
    }

    #[test]
    fn worktree_identity_different_host_are_unequal() {
        let a = WorktreeIdentity {
            host: None,
            project_root: PathBuf::from("/repo"),
            worktree_path: PathBuf::from("/repo/.worktrees/feat"),
        };
        let b = WorktreeIdentity {
            host: Some("server".into()),
            project_root: PathBuf::from("/repo"),
            worktree_path: PathBuf::from("/repo/.worktrees/feat"),
        };
        assert_ne!(a, b);
    }

    #[test]
    fn worktree_identity_hash_consistent() {
        use std::collections::HashSet;
        let a = WorktreeIdentity {
            host: None,
            project_root: PathBuf::from("/repo"),
            worktree_path: PathBuf::from("/repo/.worktrees/feat"),
        };
        let b = WorktreeIdentity {
            host: None,
            project_root: PathBuf::from("/repo"),
            worktree_path: PathBuf::from("/repo/.worktrees/feat"),
        };
        let mut set = HashSet::new();
        set.insert(a);
        assert!(set.contains(&b));
    }

    #[test]
    fn worktree_identity_serde_roundtrip() {
        let id = WorktreeIdentity {
            host: Some("myserver".into()),
            project_root: PathBuf::from("/home/user/proj"),
            worktree_path: PathBuf::from("/home/user/proj/.worktrees/feat-x"),
        };
        let json = serde_json::to_string(&id).unwrap();
        let back: WorktreeIdentity = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    // ---- DiscoveredWorktree ----

    #[test]
    fn discovered_worktree_primary_checkout() {
        let wt = DiscoveredWorktree {
            identity: WorktreeIdentity {
                host: None,
                project_root: PathBuf::from("/repo"),
                worktree_path: PathBuf::from("/repo"),
            },
            project_name: "myapp".into(),
            branch: Some("main".into()),
            is_primary_checkout: true,
        };
        assert!(wt.is_primary_checkout);
        assert_eq!(wt.branch.as_deref(), Some("main"));
    }

    #[test]
    fn discovered_worktree_detached_head() {
        let wt = DiscoveredWorktree {
            identity: WorktreeIdentity {
                host: None,
                project_root: PathBuf::from("/repo"),
                worktree_path: PathBuf::from("/repo/.worktrees/detached"),
            },
            project_name: "myapp".into(),
            branch: None,
            is_primary_checkout: false,
        };
        assert!(wt.branch.is_none());
        assert!(!wt.is_primary_checkout);
    }

    // ---- GitSafetyStatus ----

    #[test]
    fn git_safety_status_default_is_clean() {
        let s = GitSafetyStatus::default();
        assert!(!s.dirty);
        assert_eq!(s.ahead, 0);
        assert_eq!(s.behind, 0);
        assert!(!s.has_upstream);
    }

    #[test]
    fn git_safety_status_no_upstream_distinct_from_zero() {
        // has_upstream = false with ahead=0, behind=0 means "no upstream configured"
        let s = GitSafetyStatus {
            dirty: false,
            ahead: 0,
            behind: 0,
            has_upstream: false,
        };
        assert!(!s.has_upstream);
        assert!(!s.dirty);
    }

    // ---- WorktreeStatus ----

    #[test]
    fn worktree_status_variants() {
        match WorktreeStatus::Active {
            WorktreeStatus::Active => {}
            _ => panic!("expected Active"),
        }
        match WorktreeStatus::Inactive {
            WorktreeStatus::Inactive => {}
            _ => panic!("expected Inactive"),
        }
        match WorktreeStatus::Conflict {
            WorktreeStatus::Conflict => {}
            _ => panic!("expected Conflict"),
        }
        match WorktreeStatus::Unsupported {
            WorktreeStatus::Unsupported => {}
            _ => panic!("expected Unsupported"),
        }
    }

    // ---- SessionLink ----

    #[test]
    fn session_link_none() {
        assert_eq!(format!("{:?}", SessionLink::None), "None");
    }

    #[test]
    fn session_link_active_holds_session() {
        let s = Session::new("myapp", "main");
        let link = SessionLink::Active(s.clone());
        if let SessionLink::Active(ref inner) = link {
            assert_eq!(inner.name, "myapp:main");
        } else {
            panic!("expected Active");
        }
    }

    // ---- Derive Session Name ----

    #[test]
    fn derive_session_name_simple() {
        assert_eq!(derive_session_name("myapp", "main"), "myapp:main");
    }

    #[test]
    fn derive_session_name_normalizes_slash() {
        assert_eq!(
            derive_session_name("myapp", "feat/login"),
            "myapp:feat-login"
        );
    }

    #[test]
    fn derive_session_name_matches_session_new() {
        let branch = "feat/complex/nested";
        assert_eq!(
            derive_session_name("proj", branch),
            Session::new("proj", branch).name
        );
    }

    // ---- Resolve Session Alias ----

    fn make_wt(
        proj: &str,
        proj_root: &str,
        wt_path: &str,
        branch: Option<&str>,
    ) -> DiscoveredWorktree {
        DiscoveredWorktree {
            identity: WorktreeIdentity {
                host: None,
                project_root: PathBuf::from(proj_root),
                worktree_path: PathBuf::from(wt_path),
            },
            project_name: proj.to_string(),
            branch: branch.map(String::from),
            is_primary_checkout: proj_root == wt_path,
        }
    }

    #[test]
    fn resolve_unique_match_by_branch() {
        let worktrees = vec![
            make_wt(
                "myapp",
                "/repo",
                "/repo/.worktrees/feat-login",
                Some("feat/login"),
            ),
            make_wt("myapp", "/repo", "/repo/.worktrees/other", Some("other")),
        ];
        let result = resolve_session_alias("myapp:feat-login", &worktrees);
        assert!(matches!(result, SessionAliasResolution::Unique(_)));
        if let SessionAliasResolution::Unique(wt) = result {
            assert_eq!(wt.branch.as_deref(), Some("feat/login"));
        }
    }

    #[test]
    fn resolve_none_when_no_match() {
        let worktrees = vec![make_wt(
            "myapp",
            "/repo",
            "/repo/.worktrees/feat-login",
            Some("feat/login"),
        )];
        let result = resolve_session_alias("myapp:nonexistent", &worktrees);
        assert!(matches!(result, SessionAliasResolution::None));
    }

    #[test]
    fn resolve_ambiguous_on_collision() {
        // Two different branches that produce the same sanitized session name
        let worktrees = vec![
            make_wt(
                "myapp",
                "/repo",
                "/repo/.worktrees/feat-login",
                Some("feat/login"),
            ),
            make_wt(
                "myapp",
                "/repo",
                "/repo/.worktrees/feat-login-2",
                Some("feat-login"),
            ),
        ];
        let result = resolve_session_alias("myapp:feat-login", &worktrees);
        assert!(matches!(result, SessionAliasResolution::Ambiguous(_)));
    }

    #[test]
    fn resolve_none_on_empty_worktrees() {
        let result = resolve_session_alias("myapp:main", &[]);
        assert!(matches!(result, SessionAliasResolution::None));
    }

    #[test]
    fn resolve_unique_respects_project_name() {
        let worktrees = vec![make_wt(
            "app1",
            "/repo1",
            "/repo1/.worktrees/feat",
            Some("feat"),
        )];
        // Different project → no match
        let result = resolve_session_alias("app2:feat", &worktrees);
        assert!(matches!(result, SessionAliasResolution::None));
    }

    #[test]
    fn resolve_detached_head_uses_head_as_branch() {
        let worktrees = vec![make_wt("myapp", "/repo", "/repo/.worktrees/detached", None)];
        let result = resolve_session_alias("myapp:HEAD", &worktrees);
        assert!(matches!(result, SessionAliasResolution::Unique(_)));
    }

    // ---- WorktreeMetadataRecord serde ----

    #[test]
    fn metadata_record_serde_roundtrip() {
        let record = WorktreeMetadataRecord {
            project_name: "myapp".into(),
            project_root: PathBuf::from("/repo/myapp"),
            host: None,
            branch: Some("feat/login".into()),
            path: PathBuf::from("/repo/myapp/.worktrees/feat-login"),
            last_opened_at: Some(1_710_000_000),
            last_session_name: None,
        };
        let json = serde_json::to_string_pretty(&record).unwrap();
        let back: WorktreeMetadataRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record, back);
    }

    #[test]
    fn metadata_record_serializes_path_not_worktree_path() {
        let record = WorktreeMetadataRecord {
            project_name: "myapp".into(),
            project_root: PathBuf::from("/repo"),
            host: None,
            branch: None,
            path: PathBuf::from("/repo/.worktrees/x"),
            last_opened_at: None,
            last_session_name: None,
        };
        let json = serde_json::to_string(&record).unwrap();
        // The JSON key for the worktree path must be "path" (not "worktree_path")
        assert!(json.contains(r#""path":""#));
        assert!(!json.contains(r#""worktree_path":""#));
    }

    #[test]
    fn metadata_file_defaults_empty_arrays() {
        let json = r#"{"version": 1}"#;
        let file: WorktreeMetadataFile = serde_json::from_str(json).unwrap();
        assert_eq!(file.version, 1);
        assert!(file.worktrees.is_empty());
        assert!(file.notifications.is_empty());
        assert!(file.unattached_notifications.is_empty());
        assert!(file.unattached_activity.is_empty());
        assert!(file.migration_diagnostics.is_empty());
        assert!(file.llm_status.is_empty());
    }

    #[test]
    fn metadata_file_full_roundtrip() {
        let file = WorktreeMetadataFile {
            version: 1,
            worktrees: vec![WorktreeMetadataRecord {
                project_name: "myapp".into(),
                project_root: PathBuf::from("/repo"),
                host: None,
                branch: Some("main".into()),
                path: PathBuf::from("/repo"),
                last_opened_at: Some(1_710_000_000),
                last_session_name: None,
            }],
            notifications: vec![NotificationRecord {
                id: "n1".into(),
                target: WorktreeIdentity {
                    host: None,
                    project_root: PathBuf::from("/repo"),
                    worktree_path: PathBuf::from("/repo"),
                },
                level: NotifyLevel::Warning,
                message: "CI failed".into(),
                created_at: 1_710_000_001,
                source: None,
            }],
            unattached_notifications: vec![],
            unattached_activity: vec![UnattachedActivity {
                session_name: "myapp:stale".into(),
                last_attached_at: 1_700_000_000,
            }],
            migration_diagnostics: vec!["legacy session not resolvable".into()],
            llm_status: vec![],
            migrated_legacy_ids: std::collections::HashSet::new(),
        };
        let json = serde_json::to_string_pretty(&file).unwrap();
        let back: WorktreeMetadataFile = serde_json::from_str(&json).unwrap();
        assert_eq!(file, back);
    }

    #[test]
    fn notify_level_serde_roundtrip() {
        for level in &[NotifyLevel::Info, NotifyLevel::Warning, NotifyLevel::Error] {
            let json = serde_json::to_string(level).unwrap();
            let back: NotifyLevel = serde_json::from_str(&json).unwrap();
            assert_eq!(*level, back);
        }
    }

    #[test]
    fn worktree_metadata_default() {
        let m = WorktreeMetadata {
            last_opened_at: None,
            last_session_name: None,
        };
        assert!(m.last_opened_at.is_none());
        assert!(m.last_session_name.is_none());
    }
}
