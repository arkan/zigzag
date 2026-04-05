use std::process::Command;

use z_core::domain::{Layout, Session};
use z_core::error::{Result, ZError};
use z_core::traits::SessionManager;

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

    fn create_session(&self, project: &str, branch: &str, _layout: Layout) -> Result<Session> {
        let session = Session::new(project, branch);
        let status = Command::new("zellij")
            .args(["--session", &session.name])
            .status()
            .map_err(|e| ZError::Session(e.to_string()))?;
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
}
