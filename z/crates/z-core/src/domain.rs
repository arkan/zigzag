use std::path::PathBuf;

/// A project managed by z.
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub name: String,
    pub path: PathBuf,
    /// Remote host URL for remote projects (e.g. `https://vps.example.com:8082`).
    pub host: Option<String>,
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
