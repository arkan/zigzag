use thiserror::Error;

#[derive(Debug, Error)]
pub enum ZError {
    #[error("I/O error: {0}")]
    Io(String),

    #[error("dependency check failed: {0}")]
    DepCheck(String),

    #[error("project not found: {0}")]
    ProjectNotFound(String),

    #[error("session error: {0}")]
    Session(String),

    #[error("worktree error: {0}")]
    Worktree(String),

    #[error("forge error: {0}")]
    Forge(String),
}

pub type Result<T> = std::result::Result<T, ZError>;
