use std::fs;
use std::path::PathBuf;

use z_core::config::parse_projects_kdl;
use z_core::domain::Project;
use z_core::error::{Result, ZError};
use z_core::traits::ProjectStore;

/// Returns the path to `~/.config/z/projects.kdl`.
pub fn projects_kdl_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config").join("z").join("projects.kdl")
}

/// A `ProjectStore` that reads from `~/.config/z/projects.kdl`.
pub struct KdlProjectStore {
    projects_path: PathBuf,
}

impl KdlProjectStore {
    pub fn new() -> Self {
        Self {
            projects_path: projects_kdl_path(),
        }
    }

    /// For testing: construct with an explicit path.
    #[cfg(test)]
    pub fn with_path(path: PathBuf) -> Self {
        Self {
            projects_path: path,
        }
    }
}

impl ProjectStore for KdlProjectStore {
    fn list_projects(&self) -> Result<Vec<Project>> {
        match fs::read_to_string(&self.projects_path) {
            Ok(content) => parse_projects_kdl(&content),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Missing projects.kdl is not an error — just return empty list.
                Ok(Vec::new())
            }
            Err(e) => Err(ZError::Io(e.to_string())),
        }
    }

    fn get_project(&self, name: &str) -> Result<Project> {
        let projects = self.list_projects()?;
        projects
            .into_iter()
            .find(|p| p.name == name)
            .ok_or_else(|| ZError::ProjectNotFound(name.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn write_temp_kdl(content: &str) -> PathBuf {
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut path = std::env::temp_dir();
        path.push(format!("z_test_cs_{}_{}.kdl", std::process::id(), id));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn kdl_project_store_reads_file() {
        let kdl = r#"
project "myapp" {
    path "/code/myapp"
}
"#;
        let path = write_temp_kdl(kdl);
        let store = KdlProjectStore::with_path(path.clone());
        let projects = store.list_projects().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "myapp");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn kdl_project_store_missing_file_returns_empty() {
        let path = PathBuf::from("/tmp/z_nonexistent_test_file_xyz.kdl");
        let store = KdlProjectStore::with_path(path);
        let projects = store.list_projects().unwrap();
        assert!(projects.is_empty());
    }

    #[test]
    fn kdl_project_store_get_project_found() {
        let kdl = r#"
project "alpha" {
    path "/code/alpha"
}
project "beta" {
    path "/code/beta"
}
"#;
        let path = write_temp_kdl(kdl);
        let store = KdlProjectStore::with_path(path.clone());
        let p = store.get_project("beta").unwrap();
        assert_eq!(p.name, "beta");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn kdl_project_store_get_project_not_found() {
        let kdl = r#"
project "alpha" {
    path "/code/alpha"
}
"#;
        let path = write_temp_kdl(kdl);
        let store = KdlProjectStore::with_path(path.clone());
        let err = store.get_project("missing").unwrap_err();
        assert!(matches!(err, ZError::ProjectNotFound(_)));
        std::fs::remove_file(path).ok();
    }
}
