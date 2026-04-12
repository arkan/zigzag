use std::fs;
use std::path::PathBuf;

use z_core::config::{parse_projects_kdl, swap_project_nodes};
use z_core::domain::Project;
use z_core::error::{Result, ZError};
use z_core::traits::{ProjectStore, ProjectStoreWriter};

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
// Writer implementation
// ---------------------------------------------------------------------------

/// Escape backslashes and double-quotes for use inside a KDL quoted string.
fn escape_kdl_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Formats a project as a KDL node string.
fn format_project_kdl(project: &Project) -> String {
    let name = escape_kdl_string(&project.name);
    let path_str = escape_kdl_string(&project.path.to_string_lossy());
    let mut s = format!("project \"{}\" {{\n    path \"{}\"\n", name, path_str);
    if let Some(host) = &project.host {
        s.push_str(&format!("    host \"{}\"\n", escape_kdl_string(host)));
    }
    if let Some(token) = &project.token {
        s.push_str(&format!("    token \"{}\"\n", escape_kdl_string(token)));
    }
    s.push_str("}\n");
    s
}

/// Removes the project block for `name` from KDL content using text manipulation.
/// Returns `None` if the project is not found.
fn remove_project_from_kdl(content: &str, name: &str) -> Option<String> {
    let marker = format!("project \"{}\"", name);
    // Find the marker, skipping occurrences inside comment lines.
    let mut search_from = 0;
    let start = loop {
        let pos = content[search_from..].find(&marker)?;
        let abs = search_from + pos;
        let line_start = content[..abs].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let before_on_line = content[line_start..abs].trim_start();
        if before_on_line.starts_with("//") {
            // This match is inside a comment — skip past it.
            search_from = abs + marker.len();
            continue;
        }
        break abs;
    };

    // Find where this line starts
    let line_start = content[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);

    // Find the opening brace after the marker
    let brace_rel = content[start..].find('{')?;
    let brace_start = start + brace_rel;

    // Track brace depth to find matching close
    let mut depth = 0usize;
    let mut end = content.len();
    for (i, c) in content[brace_start..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = brace_start + i + 1;
                    // Skip one trailing newline
                    if content.as_bytes().get(end) == Some(&b'\n') {
                        end += 1;
                    }
                    break;
                }
            }
            _ => {}
        }
    }

    let before = &content[..line_start];
    let after = &content[end..];
    Some(format!("{}{}", before, after))
}

impl ProjectStoreWriter for KdlProjectStore {
    fn add_project(&mut self, project: &Project) -> Result<()> {
        // Read existing content (empty string if file doesn't exist)
        let existing = match fs::read_to_string(&self.projects_path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(ZError::Io(e.to_string())),
        };

        // Check for duplicate project names
        if !existing.trim().is_empty() {
            let projects = parse_projects_kdl(&existing)?;
            if projects.iter().any(|p| p.name == project.name) {
                return Err(ZError::ConfigParse(format!(
                    "project '{}' already exists",
                    project.name
                )));
            }
        }

        // Build new KDL node string
        let new_node = format_project_kdl(project);

        // Append to existing content, preserving all existing text (comments, formatting)
        let content = if existing.trim().is_empty() {
            new_node
        } else {
            format!("{}\n{}", existing.trim_end_matches('\n'), new_node)
        };

        // Create parent directories if they don't exist
        if let Some(parent) = self.projects_path.parent() {
            fs::create_dir_all(parent).map_err(|e| ZError::Io(e.to_string()))?;
        }

        // Write the file
        fs::write(&self.projects_path, content).map_err(|e| ZError::Io(e.to_string()))?;
        Ok(())
    }

    fn update_project(&mut self, project: &Project) -> Result<()> {
        self.remove_project(&project.name)?;
        self.add_project(project)
    }

    fn remove_project(&mut self, name: &str) -> Result<()> {
        let content = match fs::read_to_string(&self.projects_path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(ZError::ProjectNotFound(name.to_string()));
            }
            Err(e) => return Err(ZError::Io(e.to_string())),
        };

        let new_content = remove_project_from_kdl(&content, name)
            .ok_or_else(|| ZError::ProjectNotFound(name.to_string()))?;

        fs::write(&self.projects_path, new_content).map_err(|e| ZError::Io(e.to_string()))?;
        Ok(())
    }

    fn swap_projects(&mut self, a: usize, b: usize) -> Result<()> {
        let content = fs::read_to_string(&self.projects_path)
            .map_err(|e| ZError::Io(e.to_string()))?;

        let new_content = swap_project_nodes(&content, a, b)?;

        fs::write(&self.projects_path, new_content).map_err(|e| ZError::Io(e.to_string()))?;
        Ok(())
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

    // ── ProjectStoreWriter tests ───────────────────────────────────────────

    fn make_project(name: &str, path: &str) -> Project {
        Project {
            name: name.to_string(),
            path: std::path::PathBuf::from(path),
            host: None,
            token: None,
            transport: None,
        }
    }

    #[test]
    fn add_project_to_empty_file_creates_it() {
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut path = std::env::temp_dir();
        path.push(format!("z_test_add_{}_{}.kdl", std::process::id(), id));
        // File must not exist
        let _ = std::fs::remove_file(&path);

        let mut store = KdlProjectStore::with_path(path.clone());
        let project = make_project("newapp", "/code/newapp");
        store.add_project(&project).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("project \"newapp\""), "should contain project node");
        assert!(content.contains("path \"/code/newapp\""), "should contain path");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn add_project_to_existing_file_preserves_existing() {
        let kdl = r#"project "alpha" {
    path "/code/alpha"
}
"#;
        let path = write_temp_kdl(kdl);
        let mut store = KdlProjectStore::with_path(path.clone());
        let project = make_project("beta", "/code/beta");
        store.add_project(&project).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("project \"alpha\""), "original project preserved");
        assert!(content.contains("project \"beta\""), "new project added");
        // Verify parse round-trip works
        let projects = store.list_projects().unwrap();
        assert_eq!(projects.len(), 2);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn add_project_preserves_comments() {
        let kdl = "// This is my project list\nproject \"alpha\" {\n    path \"/code/alpha\"\n}\n";
        let path = write_temp_kdl(kdl);
        let mut store = KdlProjectStore::with_path(path.clone());
        let project = make_project("beta", "/code/beta");
        store.add_project(&project).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("// This is my project list"), "comment preserved");
        assert!(content.contains("project \"beta\""), "new project added");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn add_project_duplicate_name_returns_error() {
        let kdl = r#"project "alpha" {
    path "/code/alpha"
}
"#;
        let path = write_temp_kdl(kdl);
        let mut store = KdlProjectStore::with_path(path.clone());
        let project = make_project("alpha", "/code/alpha2");
        let err = store.add_project(&project).unwrap_err();
        assert!(matches!(err, ZError::ConfigParse(_)), "should return ConfigParse error");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn add_project_with_optional_fields() {
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut path = std::env::temp_dir();
        path.push(format!("z_test_opt_{}_{}.kdl", std::process::id(), id));
        let _ = std::fs::remove_file(&path);

        let mut store = KdlProjectStore::with_path(path.clone());
        let project = Project {
            name: "remote-app".to_string(),
            path: std::path::PathBuf::from("/code/remote-app"),
            host: Some("https://vps.example.com:8082".to_string()),
            token: Some("mytoken".to_string()),
            transport: None,
        };
        store.add_project(&project).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("host \"https://vps.example.com:8082\""));
        assert!(content.contains("token \"mytoken\""));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn add_project_creates_parent_dirs() {
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut dir = std::env::temp_dir();
        dir.push(format!("z_test_newdir_{}_{}", std::process::id(), id));
        let path = dir.join("projects.kdl");
        // Ensure dir doesn't exist
        let _ = std::fs::remove_dir_all(&dir);

        let mut store = KdlProjectStore::with_path(path.clone());
        let project = make_project("app", "/code/app");
        store.add_project(&project).unwrap();

        assert!(path.exists(), "file should be created");
        std::fs::remove_dir_all(dir).ok();
    }

    // ── remove_project_from_kdl tests ─────────────────────────────────────

    #[test]
    fn remove_project_from_kdl_removes_block() {
        let kdl = r#"project "alpha" {
    path "/code/alpha"
}
project "beta" {
    path "/code/beta"
}
"#;
        let result = remove_project_from_kdl(kdl, "alpha").unwrap();
        assert!(!result.contains("project \"alpha\""), "alpha should be removed");
        assert!(result.contains("project \"beta\""), "beta should remain");
    }

    #[test]
    fn remove_project_from_kdl_nonexistent_returns_none() {
        let kdl = "project \"alpha\" {\n    path \"/code/alpha\"\n}\n";
        let result = remove_project_from_kdl(kdl, "missing");
        assert!(result.is_none());
    }

    #[test]
    fn remove_project_from_kdl_skips_comment_containing_marker() {
        let kdl = r#"// old: project "alpha" was here
project "alpha" {
    path "/code/alpha"
}
"#;
        let result = remove_project_from_kdl(kdl, "alpha").unwrap();
        assert!(result.contains("// old: project \"alpha\" was here"), "comment preserved");
        assert!(!result.contains("path \"/code/alpha\""), "project block removed");
    }

    #[test]
    fn remove_project_from_kdl_preserves_surrounding_comments() {
        let kdl = "// header comment\nproject \"alpha\" {\n    path \"/code/alpha\"\n}\n// footer comment\n";
        let result = remove_project_from_kdl(kdl, "alpha").unwrap();
        assert!(result.contains("// header comment"), "header preserved");
        assert!(result.contains("// footer comment"), "footer preserved");
    }

    #[test]
    fn remove_project_from_kdl_only_project_in_file() {
        let kdl = "project \"only\" {\n    path \"/code/only\"\n}\n";
        let result = remove_project_from_kdl(kdl, "only").unwrap();
        assert!(result.trim().is_empty(), "file should be empty after removing only project");
    }

    #[test]
    fn format_project_kdl_escapes_quotes_in_path() {
        let project = Project {
            name: "app".to_string(),
            path: std::path::PathBuf::from("/code/my \"app\""),
            host: None,
            token: None,
            transport: None,
        };
        let kdl = format_project_kdl(&project);
        assert!(kdl.contains(r#"path "/code/my \"app\"""#), "quotes in path should be escaped");
    }

    // ── update_project / rename tests ─────────────────────────────────────

    #[test]
    fn update_project_updates_path() {
        let kdl = "project \"myapp\" {\n    path \"/code/old\"\n}\n";
        let path = write_temp_kdl(kdl);
        let mut store = KdlProjectStore::with_path(path.clone());
        let updated = Project {
            name: "myapp".to_string(),
            path: std::path::PathBuf::from("/code/new"),
            host: None,
            token: None,
            transport: None,
        };
        store.update_project(&updated).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("path \"/code/new\""), "path should be updated");
        assert!(!content.contains("/code/old"), "old path should be gone");
        let projects = store.list_projects().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].path, std::path::PathBuf::from("/code/new"));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn update_project_rename_changes_name() {
        // Rename simulated via remove + add (same as cmd_tui does it).
        let kdl = "project \"old-name\" {\n    path \"/code/app\"\n}\n";
        let path = write_temp_kdl(kdl);
        let mut store = KdlProjectStore::with_path(path.clone());
        store.remove_project("old-name").unwrap();
        let renamed = Project {
            name: "new-name".to_string(),
            path: std::path::PathBuf::from("/code/app"),
            host: None,
            token: None,
            transport: None,
        };
        store.add_project(&renamed).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("project \"new-name\""), "new name should be present");
        assert!(!content.contains("project \"old-name\""), "old name should be gone");
        let projects = store.list_projects().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "new-name");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn update_project_preserves_comments() {
        let kdl =
            "// header comment\nproject \"myapp\" {\n    path \"/code/old\"\n}\n// footer comment\n";
        let path = write_temp_kdl(kdl);
        let mut store = KdlProjectStore::with_path(path.clone());
        let updated = Project {
            name: "myapp".to_string(),
            path: std::path::PathBuf::from("/code/new"),
            host: None,
            token: None,
            transport: None,
        };
        store.update_project(&updated).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("// header comment"), "header comment preserved");
        assert!(content.contains("// footer comment"), "footer comment preserved");
        assert!(content.contains("path \"/code/new\""), "path updated");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn format_project_kdl_escapes_backslash_in_name() {
        let project = Project {
            name: r"back\slash".to_string(),
            path: std::path::PathBuf::from("/code/app"),
            host: None,
            token: None,
            transport: None,
        };
        let kdl = format_project_kdl(&project);
        assert!(kdl.contains(r#"project "back\\slash""#), "backslash in name should be escaped");
    }
}
