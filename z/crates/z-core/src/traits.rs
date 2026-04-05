use crate::domain::{CiStatus, Layout, NotifyLevel, PullRequest, Project, Session, Worktree};
use crate::error::Result;

pub trait ProjectStore {
    fn list_projects(&self) -> Result<Vec<Project>>;
    fn get_project(&self, name: &str) -> Result<Project>;
}

pub trait SessionManager {
    fn list_sessions(&self, project: &str) -> Result<Vec<Session>>;
    fn create_session(&self, project: &str, branch: &str, layout: Layout) -> Result<Session>;
    fn attach_session(&self, session: &Session) -> Result<()>;
    fn detach_session(&self, session: &Session) -> Result<()>;
    fn kill_session(&self, session: &Session) -> Result<()>;
}

pub trait WorktreeManager {
    fn list_worktrees(&self, project: &str) -> Result<Vec<Worktree>>;
    fn create_worktree(&self, project: &str, branch: &str) -> Result<Worktree>;
    fn remove_worktree(&self, worktree: &Worktree) -> Result<()>;
}

pub trait ForgeClient {
    fn get_pr(&self, project: &str, branch: &str) -> Result<Option<PullRequest>>;
    fn get_ci_status(&self, project: &str, branch: &str) -> Result<CiStatus>;
}

pub trait Notifier {
    fn notify(&self, message: &str, level: NotifyLevel) -> Result<()>;
}
