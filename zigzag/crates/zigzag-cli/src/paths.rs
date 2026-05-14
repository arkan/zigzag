use std::path::{Path, PathBuf};

pub const APP_DIR_NAME: &str = "zigzag";
pub const BIN_NAME: &str = "zigzag";
pub const GLOBAL_CONFIG_FILE: &str = "config.kdl";
pub const PROJECTS_FILE: &str = "projects.kdl";
pub const REPO_CONFIG_FILE: &str = "zigzag.kdl";
pub const WORKTREE_METADATA_FILE: &str = "worktree-metadata.json";
pub const LOG_FILE: &str = "zigzag.log";
pub const SESSION_ENV_VAR: &str = "ZIGZAG_SESSION_NAME";
pub const SWITCH_LOCK_PATH: &str = "/tmp/zigzag-switch.lock";
pub const NOTIFICATIONS_DIR: &str = "/tmp/zigzag/notifications";

fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()))
}

pub fn config_dir() -> PathBuf {
    home_dir().join(".config").join(APP_DIR_NAME)
}

pub fn state_dir() -> PathBuf {
    home_dir().join(".local").join("state").join(APP_DIR_NAME)
}

pub fn share_dir() -> PathBuf {
    home_dir().join(".local").join("share").join(APP_DIR_NAME)
}

pub fn projects_path() -> PathBuf {
    config_dir().join(PROJECTS_FILE)
}

pub fn global_config_path() -> PathBuf {
    config_dir().join(GLOBAL_CONFIG_FILE)
}

pub fn log_path() -> PathBuf {
    state_dir().join(LOG_FILE)
}

pub fn autopilot_state_dir() -> PathBuf {
    share_dir().join("autopilot")
}

pub fn repo_config_path(project_path: &Path) -> PathBuf {
    project_path.join(".config").join(REPO_CONFIG_FILE)
}
