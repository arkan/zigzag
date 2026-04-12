use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use std::collections::HashSet;

use z_core::domain::{Layout, Project, Session};
use z_core::error::{Result, ZError};
use z_core::traits::{SessionManager, SessionRefresher};

/// Strip ANSI escape sequences from a string.
///
/// Zellij's `list-sessions` output contains color codes (e.g. `\x1b[32;1m`)
/// that must be removed before parsing session names and status.
pub(crate) fn strip_ansi(s: &str) -> String {
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
pub struct ZellijSessionManager {
    /// Absolute path to the `z` binary, used in generated Zellij keybinds.
    pub bin_path: String,
}

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

    fn create_session(&self, project: &str, branch: &str, layout: Layout, theme: &z_core::theme::Theme) -> Result<Session> {
        let session = Session::new(project, branch);

        // Clean up any dead (EXITED) session with the same name first,
        // otherwise Zellij rejects the create with "already exists, but is dead".
        delete_dead_session(&session.name);

        let kdl = z_core::layout::generate_layout_kdl(&layout, &self.bin_path, theme);
        let layout_path = write_temp_layout(&kdl)?;
        // Use `-n` (--new-session-with-layout) instead of `-l` (--layout):
        // `-l` with `--session` tries to add tabs to an *existing* session,
        // while `-n` always creates a new session with the given layout.
        let result = Command::new("zellij")
            .args(["--session", &session.name, "-n", &layout_path])
            .status()
            .map_err(|e| ZError::Session(e.to_string()));
        // Keep temp file for debugging — will be overwritten on next run.
        // let _ = std::fs::remove_file(&layout_path);
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

/// A `SessionRefresher` that shells out to `zellij list-sessions` once and
/// scans `/tmp/z/notifications/` to refresh sessions and notification badges.
pub struct ZellijSessionRefresher;

impl SessionRefresher for ZellijSessionRefresher {
    fn fetch_all_sessions(&self, projects: &[Project]) -> Vec<(String, Vec<Session>)> {
        // Split into local and remote projects.
        let (local, remote): (Vec<_>, Vec<_>) =
            projects.iter().partition(|p| p.host.is_none());

        // Local: one `zellij list-sessions` call, parse per project.
        let mut results: Vec<(String, Vec<Session>)> = Vec::with_capacity(projects.len());
        let local_stdout = Command::new("zellij")
            .arg("list-sessions")
            .output()
            .ok()
            .map(|o| {
                let raw = String::from_utf8_lossy(&o.stdout);
                strip_ansi(&raw)
            });
        for p in &local {
            let sessions = local_stdout
                .as_deref()
                .map(|s| parse_zellij_sessions(s, &p.name))
                .unwrap_or_default();
            results.push((p.name.clone(), sessions));
        }

        // Remote: SSH into each host in parallel to avoid blocking on slow connections.
        std::thread::scope(|s| {
            let handles: Vec<_> = remote
                .iter()
                .map(|p| {
                    let name = p.name.clone();
                    let host = p.host.as_deref().unwrap_or("").to_string();
                    let remote_name = p
                        .path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(&p.name)
                        .to_string();
                    s.spawn(move || {
                        let sessions = crate::remote::list_remote_sessions(&host, &remote_name)
                            .unwrap_or_default();
                        (name, sessions)
                    })
                })
                .collect();
            for h in handles {
                if let Ok(result) = h.join() {
                    results.push(result);
                }
            }
        });

        results
    }

    fn fetch_notifications(&self) -> HashSet<String> {
        z_core::notification::sessions_with_notifications()
            .into_iter()
            .collect()
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

/// Parse the session age from a `zellij list-sessions` output line.
///
/// Handles both `[Created 2h 30m 5s ago]` and `[Created: 5h ago]` formats.
/// Returns the largest meaningful unit: `"Xd"`, `"Xh"`, `"Xm"`, or `"Xs"`.
/// Returns `None` if no age can be parsed.
pub fn parse_session_age(line: &str) -> Option<String> {
    // Handle "Created: " (with colon) and "Created " (without colon)
    let duration_str = if let Some(idx) = line.find("Created: ") {
        let rest = &line[idx + "Created: ".len()..];
        rest.split(" ago").next()?
    } else if let Some(idx) = line.find("Created ") {
        let rest = &line[idx + "Created ".len()..];
        rest.split(" ago").next()?
    } else {
        return None;
    };
    compact_duration(duration_str.trim())
}

/// Convert a zellij duration string (e.g. `"2h 30m 5s"`) to the largest unit.
fn compact_duration(s: &str) -> Option<String> {
    if let Some(n) = extract_unit(s, 'd') { if n > 0 { return Some(format!("{}d", n)); } }
    if let Some(n) = extract_unit(s, 'h') { if n > 0 { return Some(format!("{}h", n)); } }
    if let Some(n) = extract_unit(s, 'm') { if n > 0 { return Some(format!("{}m", n)); } }
    if let Some(n) = extract_unit(s, 's') { if n > 0 { return Some(format!("{}s", n)); } }
    None
}

/// Extract the numeric value preceding `unit` char in `s`.
fn extract_unit(s: &str, unit: char) -> Option<u64> {
    let idx = s.find(unit)?;
    let before = &s[..idx];
    let digits: String = before
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// Parse `zellij list-sessions` output and return active z-managed sessions with their ages.
///
/// Each entry is `(session_name, age)` where `age` is `None` when the age cannot be parsed.
/// Sorted alphabetically by session name.
pub fn list_all_z_sessions_with_ages_from_output(output: &str) -> Vec<(String, Option<String>)> {
    let mut sessions: Vec<(String, Option<String>)> = output
        .lines()
        .filter_map(|line| {
            let name = line.split_whitespace().next()?;
            if line.contains("EXITED") {
                return None;
            }
            if parse_session_name(name).is_some() {
                let age = parse_session_age(line);
                Some((name.to_string(), age))
            } else {
                None
            }
        })
        .collect();
    sessions.sort_by(|a, b| a.0.cmp(&b.0));
    sessions
}

/// List all active z-managed sessions with their ages.
pub fn list_all_z_sessions_with_ages() -> Vec<(String, Option<String>)> {
    let output = match Command::new("zellij").arg("list-sessions").output() {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    let raw = String::from_utf8_lossy(&output.stdout);
    let stdout = strip_ansi(&raw);
    list_all_z_sessions_with_ages_from_output(&stdout)
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

/// Separator used in the combined SSH command for remote session + notification fetch.
pub const REMOTE_SEP: &str = "---SEP---";

/// Parse the combined SSH output that contains both session list and notification names.
///
/// Format:
/// ```text
/// session1 [Created 2h ago]
/// session2 [Created 5m ago]
/// ---SEP---
/// project:branch1
/// project:branch2
/// ```
///
/// Returns `(sessions_raw, notification_names)`.
pub fn parse_combined_remote_output(output: &str) -> (&str, Vec<String>) {
    match output.split_once(REMOTE_SEP) {
        Some((sessions_part, notif_part)) => {
            let notifications = notif_part
                .trim()
                .lines()
                .filter(|l| !l.is_empty())
                .map(|l| l.trim().to_string())
                .collect();
            (sessions_part, notifications)
        }
        None => (output, Vec::new()),
    }
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
        let theme = z_core::theme::Theme::default();
        let kdl = generate_layout_kdl(&layout, "/usr/local/bin/z", &theme);
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

    #[test]
    fn strip_ansi_truncated_escape_dropped() {
        // Trailing incomplete escape sequence should be silently dropped
        assert_eq!(strip_ansi("hello\x1b"), "hello");
        assert_eq!(strip_ansi("hello\x1b[32"), "hello");
    }

    // --- parse_session_age tests ---

    #[test]
    fn parse_session_age_hours() {
        assert_eq!(
            parse_session_age("myapp:main [Created 2h 30m 5s ago]"),
            Some("2h".to_string())
        );
    }

    #[test]
    fn parse_session_age_minutes_only() {
        assert_eq!(
            parse_session_age("myapp:main [Created 30m 5s ago]"),
            Some("30m".to_string())
        );
    }

    #[test]
    fn parse_session_age_seconds_only() {
        assert_eq!(
            parse_session_age("myapp:main [Created 45s ago]"),
            Some("45s".to_string())
        );
    }

    #[test]
    fn parse_session_age_days() {
        assert_eq!(
            parse_session_age("myapp:main [Created 3d 2h ago]"),
            Some("3d".to_string())
        );
    }

    #[test]
    fn parse_session_age_colon_format() {
        // Zellij sometimes uses "Created: Xh ago"
        assert_eq!(
            parse_session_age("myapp:main [Created: 5h ago]"),
            Some("5h".to_string())
        );
    }

    #[test]
    fn parse_session_age_no_created_tag() {
        assert_eq!(parse_session_age("myapp:main"), None);
    }

    #[test]
    fn parse_session_age_empty_line() {
        assert_eq!(parse_session_age(""), None);
    }

    #[test]
    fn parse_session_age_hours_only() {
        assert_eq!(
            parse_session_age("proj:branch [Created 7h ago]"),
            Some("7h".to_string())
        );
    }

    // --- list_all_z_sessions_with_ages_from_output tests ---

    #[test]
    fn sessions_with_ages_returns_name_and_age() {
        let output = "myapp:main [Created 2h 30m ago]\nhermes:dev [Created: 1h ago]\n";
        let sessions = list_all_z_sessions_with_ages_from_output(output);
        assert_eq!(sessions.len(), 2);
        assert!(sessions.contains(&("hermes:dev".to_string(), Some("1h".to_string()))));
        assert!(sessions.contains(&("myapp:main".to_string(), Some("2h".to_string()))));
    }

    #[test]
    fn sessions_with_ages_none_when_no_age() {
        let output = "myapp:main\n";
        let sessions = list_all_z_sessions_with_ages_from_output(output);
        assert_eq!(sessions, vec![("myapp:main".to_string(), None)]);
    }

    #[test]
    fn sessions_with_ages_excludes_exited() {
        let output = "myapp:main (EXITED)\nhermes:dev [Created 1h ago]\n";
        let sessions = list_all_z_sessions_with_ages_from_output(output);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].0, "hermes:dev");
    }

    #[test]
    fn sessions_with_ages_sorted_alphabetically() {
        let output = "zzz:main [Created 5h ago]\naaa:dev [Created 1h ago]\n";
        let sessions = list_all_z_sessions_with_ages_from_output(output);
        assert_eq!(sessions[0].0, "aaa:dev");
        assert_eq!(sessions[1].0, "zzz:main");
    }

    #[test]
    fn sessions_with_ages_excludes_non_z_sessions() {
        let output = "plain-session\nanother-session\nmyapp:main [Created 1h ago]\n";
        let sessions = list_all_z_sessions_with_ages_from_output(output);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].0, "myapp:main");
    }

    #[test]
    fn sessions_with_ages_empty_output() {
        let sessions = list_all_z_sessions_with_ages_from_output("");
        assert!(sessions.is_empty());
    }

    #[test]
    fn sessions_with_ages_whitespace_only_lines() {
        let output = "  \n\t\n  \n";
        let sessions = list_all_z_sessions_with_ages_from_output(output);
        assert!(sessions.is_empty());
    }

    #[test]
    fn sessions_with_ages_duplicates_preserved() {
        let output = "myapp:main [Created: 5h ago]\nmyapp:main [Created: 1h ago]\n";
        let sessions = list_all_z_sessions_with_ages_from_output(output);
        assert_eq!(sessions.len(), 2, "duplicates are preserved (no dedup)");
    }

    #[test]
    fn sessions_with_ages_exited_with_extra_metadata() {
        let output = "myapp:main [Created: 5h ago] (EXITED - 2 hours ago)\n";
        let sessions = list_all_z_sessions_with_ages_from_output(output);
        assert!(sessions.is_empty(), "EXITED anywhere in the line should exclude it");
    }

    #[test]
    fn parse_session_age_all_zeros() {
        assert_eq!(parse_session_age("myapp:main [Created 0h 0m 0s ago]"), None);
    }

    #[test]
    fn parse_session_age_no_duration_between_created_and_ago() {
        assert_eq!(parse_session_age("myapp:main [Created  ago]"), None);
    }

    #[test]
    fn parse_session_age_colon_format_with_minutes() {
        assert_eq!(
            parse_session_age("myapp:main [Created: 15m ago]"),
            Some("15m".to_string())
        );
    }

    #[test]
    fn compact_duration_empty_string() {
        assert_eq!(compact_duration(""), None);
    }

    #[test]
    fn extract_unit_non_digit_before_unit() {
        // "abc5m" — only the "5" immediately before "m" should be captured
        assert_eq!(extract_unit("abc5m", 'm'), Some(5));
    }

    #[test]
    fn extract_unit_no_digits_before_unit() {
        assert_eq!(extract_unit("m", 'm'), None);
        assert_eq!(extract_unit("xm", 'm'), None);
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

    // --- parse_combined_remote_output ---

    #[test]
    fn parse_combined_remote_output_both_present() {
        let output = "myapp:main [Created 2h ago]\nmyapp:feat [Created 5m ago]\n---SEP---\nmyapp:main\nmyapp:feat\n";
        let (sessions, notifs) = parse_combined_remote_output(output);
        assert!(sessions.contains("myapp:main"));
        assert!(sessions.contains("myapp:feat"));
        assert_eq!(notifs, vec!["myapp:main", "myapp:feat"]);
    }

    #[test]
    fn parse_combined_remote_output_no_notifications() {
        let output = "myapp:main [Created 2h ago]\n";
        let (sessions, notifs) = parse_combined_remote_output(output);
        assert!(sessions.contains("myapp:main"));
        assert!(notifs.is_empty());
    }

    #[test]
    fn parse_combined_remote_output_empty() {
        let (sessions, notifs) = parse_combined_remote_output("");
        assert!(sessions.is_empty());
        assert!(notifs.is_empty());
    }

    #[test]
    fn parse_combined_remote_output_only_notifications() {
        let output = "---SEP---\nmyapp:main\n";
        let (sessions, notifs) = parse_combined_remote_output(output);
        assert!(sessions.trim().is_empty());
        assert_eq!(notifs, vec!["myapp:main"]);
    }
}
