use std::path::PathBuf;

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
#[derive(Debug, Clone, PartialEq)]
pub enum NotifyLevel {
    Info,
    Warning,
    Error,
}

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
        assert_eq!(
            s.name,
            format!("proj:{}", sanitize_branch_name(branch))
        );
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
}
