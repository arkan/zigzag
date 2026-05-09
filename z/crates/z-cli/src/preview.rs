use std::path::{Path, PathBuf};
use std::process::Command;

use z_core::domain::CiStatus;
use z_core::traits::ForgeClient;
use z_tui::{CommitInfo, GitInfo, PreviewContext, PreviewDataSource, PreviewExtraData, ZellijInfo};

use crate::remote;

/// Process-backed Adapter for TUI Preview acquisition.
pub struct CliPreviewDataSource {
    forge_client: Box<dyn ForgeClient + Send + Sync>,
}

impl CliPreviewDataSource {
    pub fn new(forge_client: Box<dyn ForgeClient + Send + Sync>) -> Self {
        Self { forge_client }
    }
}

impl PreviewDataSource for CliPreviewDataSource {
    fn load_git_preview(&self, context: &PreviewContext) -> Result<GitInfo, String> {
        let effective = if !context.branch.is_empty() {
            resolve_worktree_path(
                &context.project_path,
                &context.branch,
                context.host.as_deref(),
            )
            .unwrap_or_else(|| context.project_path.clone())
        } else {
            context.project_path.clone()
        };

        if let Some(host) = &context.host {
            let info = remote::fetch_remote_git_info(host, &effective.to_string_lossy())
                .map_err(|e| e.to_string())?;
            return Ok(GitInfo {
                branch: info.branch,
                is_dirty: info.is_dirty,
                ahead: info.ahead,
                behind: info.behind,
                commits: info
                    .commits
                    .into_iter()
                    .map(|(hash, message)| CommitInfo { hash, message })
                    .collect(),
                pr: None,
                ci: None,
                zellij: None,
                review: None,
            });
        }

        fetch_git_info(&effective.to_string_lossy())
    }

    fn load_extra_preview(&self, context: &PreviewContext) -> Result<PreviewExtraData, String> {
        Ok(PreviewExtraData {
            pr: self
                .forge_client
                .get_pr(&context.project_name, &context.branch)
                .ok()
                .flatten(),
            ci: self
                .forge_client
                .get_ci_status(&context.project_name, &context.branch)
                .unwrap_or(CiStatus::Unknown),
            zellij: fetch_zellij_info(&context.session_name),
            review: self
                .forge_client
                .get_review_status(&context.project_name, &context.branch)
                .ok()
                .flatten(),
        })
    }
}

fn fetch_git_info(path: &str) -> Result<GitInfo, String> {
    let branch_out = Command::new("git")
        .args(["-C", path, "symbolic-ref", "--short", "HEAD"])
        .output()
        .map_err(|e| format!("git error: {}", e))?;

    if !branch_out.status.success() {
        return Err("not a git repository".to_string());
    }
    let branch = String::from_utf8_lossy(&branch_out.stdout)
        .trim()
        .to_string();

    let status_out = Command::new("git")
        .args(["-C", path, "status", "--short"])
        .output()
        .map_err(|e| format!("git error: {}", e))?;
    let is_dirty = !String::from_utf8_lossy(&status_out.stdout)
        .trim()
        .is_empty();

    let (ahead, behind) = fetch_ahead_behind(path);

    let log_out = Command::new("git")
        .args(["-C", path, "log", "--oneline", "-5"])
        .output()
        .map_err(|e| format!("git error: {}", e))?;
    let commits = String::from_utf8_lossy(&log_out.stdout)
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut parts = line.splitn(2, ' ');
            CommitInfo {
                hash: parts.next().unwrap_or("").to_string(),
                message: parts.next().unwrap_or("").to_string(),
            }
        })
        .collect();

    Ok(GitInfo {
        branch,
        ahead,
        behind,
        is_dirty,
        commits,
        pr: None,
        ci: None,
        zellij: None,
        review: None,
    })
}

fn fetch_ahead_behind(path: &str) -> (usize, usize) {
    let out = Command::new("git")
        .args([
            "-C",
            path,
            "rev-list",
            "--left-right",
            "--count",
            "HEAD...@{u}",
        ])
        .output();

    match out {
        Ok(output) if output.status.success() => {
            let s = String::from_utf8_lossy(&output.stdout);
            let mut parts = s.split_whitespace();
            let ahead = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            let behind = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            (ahead, behind)
        }
        _ => (0, 0),
    }
}

fn resolve_worktree_path(project_path: &Path, branch: &str, ssh_host: Option<&str>) -> Option<PathBuf> {
    let stdout = if let Some(host) = ssh_host {
        let path_str = project_path.to_string_lossy();
        let remote_cmd = format!(
            "cd '{}' && git worktree list --porcelain",
            path_str.replace('\'', "'\\''")
        );
        let wrapped = format!(
            "bash -l -c '{}'",
            remote_cmd.replace('\'', "'\\''")
        );
        let output = Command::new("ssh")
            .args(["-o", "ConnectTimeout=10", host, &wrapped])
            .output()
            .ok()?;
        String::from_utf8_lossy(&output.stdout).to_string()
    } else {
        let output = Command::new("git")
            .args(["worktree", "list", "--porcelain"])
            .current_dir(project_path)
            .output()
            .ok()?;
        String::from_utf8_lossy(&output.stdout).to_string()
    };

    parse_worktree_path(&stdout, branch)
}

fn parse_worktree_path(stdout: &str, branch: &str) -> Option<PathBuf> {
    let mut current_path: Option<PathBuf> = None;
    for line in stdout.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            current_path = Some(PathBuf::from(path));
        } else if let Some(git_branch) = line.strip_prefix("branch refs/heads/") {
            if z_core::domain::sanitize_branch_name(git_branch) == branch {
                return current_path;
            }
        }
    }
    None
}

fn fetch_zellij_info(session_name: &str) -> Option<ZellijInfo> {
    if session_name.is_empty() {
        return None;
    }

    let out = Command::new("zellij")
        .args(["list-sessions", "--json"])
        .output()
        .ok()?;

    if out.status.success() {
        let json = String::from_utf8_lossy(&out.stdout);
        if let Some(info) = z_core::zellij::parse_zellij_session_info(&json, session_name) {
            return Some(ZellijInfo {
                tab_count: info.tab_count,
                pane_count: info.pane_count,
                uptime: info.uptime,
            });
        }
    }

    let out = Command::new("zellij").args(["list-sessions"]).output().ok()?;
    if !out.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        if line.contains(session_name) {
            return Some(ZellijInfo {
                tab_count: 0,
                pane_count: 0,
                uptime: extract_zellij_uptime(line).unwrap_or_else(|| "unknown".to_string()),
            });
        }
    }
    None
}

fn extract_zellij_uptime(line: &str) -> Option<String> {
    if let Some(start) = line.find("Created ") {
        let rest = &line[start + "Created ".len()..];
        if let Some(end) = rest.find(" ago") {
            return Some(rest[..end].to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_worktree_path_for_sanitized_branch() {
        let stdout = "worktree /repo/main\nbranch refs/heads/main\n\nworktree /repo/feat-login\nbranch refs/heads/feat/login\n";

        assert_eq!(
            parse_worktree_path(stdout, "feat-login"),
            Some(PathBuf::from("/repo/feat-login"))
        );
    }

    #[test]
    fn missing_worktree_path_returns_none() {
        let stdout = "worktree /repo/main\nbranch refs/heads/main\n";

        assert!(parse_worktree_path(stdout, "missing").is_none());
    }

    #[test]
    fn extract_zellij_uptime_no_pattern_returns_none() {
        assert!(extract_zellij_uptime("myapp:main [EXITED]").is_none());
    }

    #[test]
    fn extract_zellij_uptime_extracts_duration() {
        let line = "myapp:main [Created 3h12m ago]";
        assert_eq!(extract_zellij_uptime(line), Some("3h12m".to_string()));
    }
}
