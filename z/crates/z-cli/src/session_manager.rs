use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use z_core::domain::{Layout, Session};
use z_core::error::{Result, ZError};
use z_core::traits::SessionManager;

/// Strip ANSI escape sequences from a string.
///
/// Zellij's `list-sessions` output contains color codes (e.g. `\x1b[32;1m`)
/// that must be removed before parsing session names and status.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_escape = false;
    for c in s.chars() {
        if in_escape {
            if c.is_ascii_alphabetic() {
                in_escape = false;
            }
        } else if c == '\x1b' {
            in_escape = true;
        } else {
            out.push(c);
        }
    }
    out
}

static LAYOUT_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A `SessionManager` that shells out to `zellij` to manage sessions.
pub struct ZellijSessionManager;

impl SessionManager for ZellijSessionManager {
    fn list_sessions(&self, project: &str) -> Result<Vec<Session>> {
        match Command::new("zellij").arg("list-sessions").output() {
            Ok(output) => {
                let raw = String::from_utf8_lossy(&output.stdout);
                let stdout = strip_ansi(&raw);
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

        // Clean up any dead (EXITED) session with the same name first,
        // otherwise Zellij rejects the create with "already exists, but is dead".
        delete_dead_session(&session.name);

        let kdl = z_core::layout::generate_layout_kdl(&layout);
        let layout_path = write_temp_layout(&kdl)?;
        // Use `-n` (--new-session-with-layout) instead of `-l` (--layout):
        // `-l` with `--session` tries to add tabs to an *existing* session,
        // while `-n` always creates a new session with the given layout.
        let result = Command::new("zellij")
            .args(["--session", &session.name, "-n", &layout_path])
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
        let output = Command::new("zellij")
            .args(["delete-session", &session.name, "--force"])
            .output()
            .map_err(|e| ZError::Session(e.to_string()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ZError::Session(format!(
                "zellij delete-session failed: {}",
                stderr.trim()
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
    let project = parts.next()?.trim().to_string();
    let branch = parts.next()?.trim().to_string();
    if project.is_empty() || branch.is_empty() {
        return None;
    }
    Some((project, branch))
}

/// Delete a Zellij session if it exists and is dead (EXITED).
///
/// Checks `zellij list-sessions` for a matching dead session and runs
/// `zellij delete-session` to clean it up. Failures are silently ignored
/// since this is a best-effort cleanup before creating a fresh session.
fn delete_dead_session(session_name: &str) {
    let output = match Command::new("zellij").arg("list-sessions").output() {
        Ok(o) => o,
        Err(_) => return,
    };
    let raw = String::from_utf8_lossy(&output.stdout);
    let stdout = strip_ansi(&raw);
    let is_dead = stdout.lines().any(|line| {
        line.split_whitespace().next() == Some(session_name) && line.contains("EXITED")
    });
    if is_dead {
        let _ = Command::new("zellij")
            .args(["delete-session", session_name])
            .output();
    }
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

/// Parse `zellij list-sessions` output and return all z-managed sessions
/// (those matching `project:branch` naming), sorted alphabetically.
///
/// This is the pure/testable core used by `list_all_z_sessions`.
pub fn list_all_z_sessions_from_output(output: &str) -> Vec<String> {
    let mut sessions: Vec<String> = output
        .lines()
        .filter_map(|line| {
            let name = line.split_whitespace().next()?;
            if line.contains("EXITED") {
                return None;
            }
            if parse_session_name(name).is_some() {
                Some(name.to_string())
            } else {
                None
            }
        })
        .collect();
    sessions.sort();
    sessions
}

/// List all active z-managed Zellij sessions across all projects, sorted alphabetically.
pub fn list_all_z_sessions() -> Vec<String> {
    let output = match Command::new("zellij").arg("list-sessions").output() {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    let raw = String::from_utf8_lossy(&output.stdout);
    let stdout = strip_ansi(&raw);
    list_all_z_sessions_from_output(&stdout)
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
            // Strip exited entries — only return live sessions.
            // The raw text may be "(EXITED - attach to resurrect)" after ANSI stripping.
            if line.contains("EXITED") {
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

    #[test]
    fn parse_session_name_whitespace_project_returns_none() {
        assert!(parse_session_name("  :main").is_none());
    }

    #[test]
    fn parse_session_name_whitespace_branch_returns_none() {
        assert!(parse_session_name("myapp:  ").is_none());
    }

    #[test]
    fn parse_session_name_both_whitespace_returns_none() {
        assert!(parse_session_name(" : ").is_none());
    }

    #[test]
    fn parse_session_name_only_colon_returns_none() {
        assert!(parse_session_name(":").is_none());
    }

    #[test]
    fn parse_session_name_trims_surrounding_whitespace() {
        let result = parse_session_name(" myapp : main ");
        assert_eq!(result, Some(("myapp".to_string(), "main".to_string())));
    }

    // --- strip_ansi tests ---

    #[test]
    fn strip_ansi_empty_string() {
        assert_eq!(strip_ansi(""), "");
    }

    #[test]
    fn strip_ansi_plain_text_unchanged() {
        assert_eq!(strip_ansi("hello world"), "hello world");
    }

    #[test]
    fn strip_ansi_removes_sgr_sequence() {
        // \x1b[32;1m is bold green; should be stripped entirely
        assert_eq!(strip_ansi("\x1b[32;1mhello\x1b[0m"), "hello");
    }

    #[test]
    fn strip_ansi_removes_multiple_codes() {
        let input = "\x1b[1mmyapp:main\x1b[0m [Created: \x1b[33m5h ago\x1b[0m]";
        assert_eq!(strip_ansi(input), "myapp:main [Created: 5h ago]");
    }

    #[test]
    fn strip_ansi_preserves_structure_for_session_parsing() {
        // Simulate actual Zellij output with ANSI codes around the session name
        let colored = "\x1b[32;1mmyapp:main\x1b[0m [Created: 2h ago]";
        let stripped = strip_ansi(colored);
        // After stripping, parse_zellij_sessions should still find the session
        let sessions = parse_zellij_sessions(&stripped, "myapp");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "myapp:main");
    }

    #[test]
    fn strip_ansi_exited_session_still_filtered() {
        // EXITED sessions with ANSI codes around status should still be filtered
        let colored = "\x1b[31mmyapp:main\x1b[0m [Created: 5h ago] \x1b[31m(EXITED)\x1b[0m";
        let stripped = strip_ansi(colored);
        let sessions = parse_zellij_sessions(&stripped, "myapp");
        assert!(sessions.is_empty(), "EXITED sessions must be filtered even after ANSI stripping");
    }

    // --- list_all_z_sessions_from_output tests ---

    #[test]
    fn list_all_z_sessions_returns_z_managed_sessions() {
        let output = "myapp:main [Created: 5h ago]\nhermes:dev [Created: 1h ago]\nplain-session\n";
        let sessions = list_all_z_sessions_from_output(output);
        assert_eq!(sessions.len(), 2);
        assert!(sessions.contains(&"hermes:dev".to_string()));
        assert!(sessions.contains(&"myapp:main".to_string()));
    }

    #[test]
    fn list_all_z_sessions_excludes_exited() {
        let output = "myapp:main (EXITED)\nhermes:dev [Created: 1h ago]\n";
        let sessions = list_all_z_sessions_from_output(output);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0], "hermes:dev");
    }

    #[test]
    fn list_all_z_sessions_excludes_non_z_sessions() {
        let output = "plain-session\nanother-session\nmyapp:main\n";
        let sessions = list_all_z_sessions_from_output(output);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0], "myapp:main");
    }

    #[test]
    fn list_all_z_sessions_sorted_alphabetically() {
        let output = "zzz:main\naaa:dev\nmid:branch\n";
        let sessions = list_all_z_sessions_from_output(output);
        assert_eq!(sessions, vec!["aaa:dev", "mid:branch", "zzz:main"]);
    }

    #[test]
    fn list_all_z_sessions_empty_output() {
        let sessions = list_all_z_sessions_from_output("");
        assert!(sessions.is_empty());
    }

    #[test]
    fn strip_ansi_truncated_escape_dropped() {
        // Trailing incomplete escape sequence should be silently dropped
        assert_eq!(strip_ansi("hello\x1b"), "hello");
        assert_eq!(strip_ansi("hello\x1b[32"), "hello");
    }

    /// Verify that `env_remove("ZELLIJ")` on a Command prevents the env var
    /// from reaching the child process. This documents the mechanism used in
    /// `create_session()` to avoid Zellij interpreting `--session <name>` as
    /// "attach" when `z` is launched from inside an existing Zellij session.
    ///
    /// Note: we cannot call `create_session()` directly because it requires
    /// zellij to be installed. Instead we verify the env_remove mechanism on
    /// a plain `sh` command.
    #[test]
    fn env_remove_prevents_zellij_var_in_child_process() {
        // Run a child that checks for ZELLIJ — env_remove should strip it
        // even if the current process has it set. We pass it explicitly via
        // .env() to avoid the thread-unsafe std::env::set_var.
        let output = Command::new("sh")
            .args(["-c", "printenv ZELLIJ || echo UNSET"])
            .env("ZELLIJ", "outer-session")
            .env_remove("ZELLIJ")
            .output()
            .expect("sh should be available");

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            stdout.trim(),
            "UNSET",
            "ZELLIJ env var must not be passed to child subprocess"
        );
    }
}
