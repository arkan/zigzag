use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use z_core::domain::{Layout, Session};
use z_core::error::{Result, ZError};
use z_core::traits::SessionManager;

static LAYOUT_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A `SessionManager` that shells out to `zellij` to manage sessions.
pub struct ZellijSessionManager;

impl SessionManager for ZellijSessionManager {
    fn list_sessions(&self, project: &str) -> Result<Vec<Session>> {
        match Command::new("zellij").arg("list-sessions").output() {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let sessions = parse_zellij_sessions(&stdout, project);
                Ok(sessions)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // zellij not installed or not running — no sessions.
                Ok(Vec::new())
            }
            Err(e) => Err(ZError::Session(e.to_string())),
        }
    }

    fn create_session(&self, project: &str, branch: &str, layout: Layout) -> Result<Session> {
        let session = Session::new(project, branch);
        let kdl = z_core::layout::generate_layout_kdl(&layout);
        let layout_path = write_temp_layout(&kdl)?;
        let result = Command::new("zellij")
            .args(["--session", &session.name, "--layout", &layout_path])
            .status()
            .map_err(|e| ZError::Session(e.to_string()));
        // Clean up temp file regardless of outcome.
        let _ = std::fs::remove_file(&layout_path);
        let status = result?;
        if !status.success() {
            return Err(ZError::Session(format!(
                "zellij exited with status {}",
                status
            )));
        }
        Ok(session)
    }

    fn attach_session(&self, session: &Session) -> Result<()> {
        let status = Command::new("zellij")
            .args(["attach", &session.name])
            .status()
            .map_err(|e| ZError::Session(e.to_string()))?;
        if !status.success() {
            return Err(ZError::Session(format!(
                "zellij attach exited with status {}",
                status
            )));
        }
        Ok(())
    }

    fn detach_session(&self, session: &Session) -> Result<()> {
        let status = Command::new("zellij")
            .args(["attach", &session.name, "--detach"])
            .status()
            .map_err(|e| ZError::Session(e.to_string()))?;
        if !status.success() {
            return Err(ZError::Session(format!(
                "zellij attach --detach exited with status {}",
                status
            )));
        }
        Ok(())
    }

    fn kill_session(&self, session: &Session) -> Result<()> {
        let status = Command::new("zellij")
            .args(["delete-session", &session.name])
            .status()
            .map_err(|e| ZError::Session(e.to_string()))?;
        if !status.success() {
            return Err(ZError::Session(format!(
                "zellij delete-session exited with status {}",
                status
            )));
        }
        Ok(())
    }
}

/// Parse a session name string `"project:branch"` into `(project, branch)`.
///
/// Returns `None` if the string does not contain a `:` separator or either part is empty.
pub fn parse_session_name(s: &str) -> Option<(String, String)> {
    let mut parts = s.splitn(2, ':');
    let project = parts.next()?.to_string();
    let branch = parts.next()?.to_string();
    if project.is_empty() || branch.is_empty() {
        return None;
    }
    Some((project, branch))
}

/// Write a Zellij KDL layout string to a temporary file and return its path.
///
/// The caller is responsible for removing the file after use.
pub fn write_temp_layout(content: &str) -> Result<String> {
    let seq = LAYOUT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = format!("/tmp/z-layout-{}-{}.kdl", std::process::id(), seq);
    std::fs::write(&path, content)
        .map_err(|e| ZError::Io(format!("failed to write temp layout: {}", e)))?;
    Ok(path)
}

/// Parse `zellij list-sessions` output and return sessions belonging to `project`.
///
/// Zellij output format (one session per line):
/// ```
/// myapp:main [Created: 5h ago]
/// myapp:feat-login [Created: 1h ago] (EXITED)
/// other-project:main
/// ```
///
/// We match lines whose session name starts with `"{project}:"`.
pub fn parse_zellij_sessions(output: &str, project: &str) -> Vec<Session> {
    let prefix = format!("{}:", project);
    output
        .lines()
        .filter_map(|line| {
            let name = line.split_whitespace().next()?;
            // Strip `(EXITED)` entries — only return live sessions.
            if line.contains("(EXITED)") {
                return None;
            }
            if name.starts_with(&prefix) {
                let branch_normalized = &name[prefix.len()..];
                // Reverse the dash-normalization to recover a branch-like name.
                // We store the normalized form as the session's branch since the
                // original branch name is not available from Zellij output.
                Some(Session {
                    name: name.to_string(),
                    project: project.to_string(),
                    branch: branch_normalized.to_string(),
                })
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sessions_empty_output() {
        let sessions = parse_zellij_sessions("", "myapp");
        assert!(sessions.is_empty());
    }

    #[test]
    fn parse_sessions_no_matching_project() {
        let output = "other:main\nother:feat\n";
        let sessions = parse_zellij_sessions(output, "myapp");
        assert!(sessions.is_empty());
    }

    #[test]
    fn parse_sessions_one_match() {
        let output = "myapp:main [Created: 5h ago]\nother:feat\n";
        let sessions = parse_zellij_sessions(output, "myapp");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "myapp:main");
        assert_eq!(sessions[0].project, "myapp");
        assert_eq!(sessions[0].branch, "main");
    }

    #[test]
    fn parse_sessions_multiple_matches() {
        let output =
            "myapp:main [Created: 5h ago]\nmyapp:feat-login [Created: 1h ago]\nother:main\n";
        let sessions = parse_zellij_sessions(output, "myapp");
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].name, "myapp:main");
        assert_eq!(sessions[1].name, "myapp:feat-login");
    }

    #[test]
    fn parse_sessions_skips_exited() {
        let output = "myapp:main [Created: 5h ago] (EXITED)\nmyapp:feat-login [Created: 1h ago]\n";
        let sessions = parse_zellij_sessions(output, "myapp");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "myapp:feat-login");
    }

    #[test]
    fn parse_sessions_branch_with_dashes() {
        let output = "myapp:feat-some-long-branch [Created: 2h ago]\n";
        let sessions = parse_zellij_sessions(output, "myapp");
        assert_eq!(sessions[0].branch, "feat-some-long-branch");
    }

    #[test]
    fn parse_sessions_whitespace_only_lines_ignored() {
        let output = "  \n\nmyapp:main\n  \n";
        let sessions = parse_zellij_sessions(output, "myapp");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "myapp:main");
    }

    #[test]
    fn parse_sessions_no_false_prefix_match() {
        // "myapp-ext:main" should NOT match project "myapp"
        let output = "myapp-ext:main\nmyapp:dev\n";
        let sessions = parse_zellij_sessions(output, "myapp");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "myapp:dev");
    }

    #[test]
    fn parse_sessions_all_exited() {
        let output = "myapp:main (EXITED)\nmyapp:dev (EXITED)\n";
        let sessions = parse_zellij_sessions(output, "myapp");
        assert!(sessions.is_empty());
    }

    #[test]
    fn write_temp_layout_creates_file_with_content() {
        let content = "layout {\n    tab name=\"test\" {\n        pane\n    }\n}\n";
        let path = write_temp_layout(content).expect("write_temp_layout should succeed");
        assert!(path.starts_with("/tmp/z-layout-"));
        assert!(path.ends_with(".kdl"));
        let read_back = std::fs::read_to_string(&path).expect("temp file should be readable");
        assert_eq!(read_back, content);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn write_temp_layout_default_layout_roundtrip() {
        use z_core::layout::{default_layout, generate_layout_kdl};
        let layout = default_layout();
        let kdl = generate_layout_kdl(&layout);
        let path = write_temp_layout(&kdl).expect("write_temp_layout should succeed");
        let read_back = std::fs::read_to_string(&path).expect("temp file should be readable");
        assert!(read_back.contains("tab name=\"claude\""));
        assert!(read_back.contains("tab name=\"shell\""));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn write_temp_layout_concurrent_calls_get_unique_paths() {
        let path1 = write_temp_layout("a").expect("first write");
        let path2 = write_temp_layout("b").expect("second write");
        assert_ne!(path1, path2);
        let _ = std::fs::remove_file(&path1);
        let _ = std::fs::remove_file(&path2);
    }

    #[test]
    fn parse_session_name_valid() {
        let result = parse_session_name("myapp:main");
        assert_eq!(result, Some(("myapp".to_string(), "main".to_string())));
    }

    #[test]
    fn parse_session_name_with_dashes() {
        let result = parse_session_name("myapp:feat-login");
        assert_eq!(result, Some(("myapp".to_string(), "feat-login".to_string())));
    }

    #[test]
    fn parse_session_name_colon_in_branch() {
        // splitn(2, ':') — only first colon splits; remainder is branch
        let result = parse_session_name("myapp:feat:extra");
        assert_eq!(result, Some(("myapp".to_string(), "feat:extra".to_string())));
    }

    #[test]
    fn parse_session_name_no_colon_returns_none() {
        assert!(parse_session_name("myapp-main").is_none());
    }

    #[test]
    fn parse_session_name_empty_project_returns_none() {
        assert!(parse_session_name(":main").is_none());
    }

    #[test]
    fn parse_session_name_empty_branch_returns_none() {
        assert!(parse_session_name("myapp:").is_none());
    }

    #[test]
    fn parse_session_name_empty_string_returns_none() {
        assert!(parse_session_name("").is_none());
    }
}
