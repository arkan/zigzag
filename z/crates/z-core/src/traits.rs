use crate::domain::{CiStatus, Layout, NotifyLevel, PullRequest, Project, Session, Worktree};
use crate::error::Result;
use crate::theme::Theme;

pub trait ProjectStore {
    fn list_projects(&self) -> Result<Vec<Project>>;
    fn get_project(&self, name: &str) -> Result<Project>;
}

pub trait ProjectStoreWriter {
    fn add_project(&mut self, project: &Project) -> Result<()>;
    fn update_project(&mut self, project: &Project) -> Result<()>;
    fn remove_project(&mut self, name: &str) -> Result<()>;
    fn swap_projects(&mut self, a: usize, b: usize) -> Result<()>;
}

pub trait SessionManager {
    fn list_sessions(&self, project: &str) -> Result<Vec<Session>>;
    fn create_session(&self, project: &str, branch: &str, layout: Layout, theme: &Theme) -> Result<Session>;
    fn attach_session(&self, session: &Session) -> Result<()>;
    fn detach_session(&self, session: &Session) -> Result<()>;
    fn kill_session(&self, session: &Session) -> Result<()>;
}

pub trait WorktreeManager {
    fn list_worktrees(&self, project: &str) -> Result<Vec<Worktree>>;
    fn create_worktree(&self, project: &str, branch: &str) -> Result<Worktree>;
    fn remove_worktree(&self, worktree: &Worktree, force: bool) -> Result<()>;
}

pub trait ForgeClient {
    fn get_pr(&self, project: &str, branch: &str) -> Result<Option<PullRequest>>;
    fn get_ci_status(&self, project: &str, branch: &str) -> Result<CiStatus>;
}

pub trait Notifier {
    fn notify(&self, message: &str, level: NotifyLevel) -> Result<()>;
}
