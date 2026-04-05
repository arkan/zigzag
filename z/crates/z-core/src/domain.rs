use std::path::PathBuf;

/// A project managed by z.
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub name: String,
    pub path: PathBuf,
    /// Remote host URL for remote projects (e.g. `https://vps.example.com:8082`).
    pub host: Option<String>,
    /// Authentication token for remote hosts, resolved from `env:VAR` at parse time.
    pub token: Option<String>,
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
        let normalized = branch.replace('/', "-");
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
}
