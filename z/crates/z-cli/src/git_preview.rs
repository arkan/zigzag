use std::path::Path;
use std::process::Command;

use z_core::error::{Result, ZError};
use z_tui::{CommitInfo, GitInfo};

use crate::remote;

const GIT_SEP: &str = "---Z-GIT-PREVIEW-SEP---";

#[derive(Debug, Clone, PartialEq)]
pub struct GitPreviewInfo {
    pub branch: String,
    pub is_dirty: bool,
    pub commits: Vec<(String, String)>,
    pub ahead: usize,
    pub behind: usize,
}

/// Build one Git command whose output can be parsed by `parse_git_preview_output`.
pub fn build_git_preview_command(project_path: &str) -> String {
    format!(
        "cd {} && git symbolic-ref --short HEAD && echo '{}' && git status --short && echo '{}' && (git log --oneline -5 2>/dev/null || true) && echo '{}' && (git rev-list --left-right --count HEAD...@{{u}} 2>/dev/null || true)",
        remote::shell_quote(project_path), GIT_SEP, GIT_SEP, GIT_SEP
    )
}

pub fn parse_git_preview_output(output: &str) -> Result<GitPreviewInfo> {
    let sections: Vec<&str> = output.split(GIT_SEP).collect();
    if sections.len() < 2 {
        return Err(ZError::Session(
            "unexpected git preview output format".to_string(),
        ));
    }

    let branch = sections[0].trim().to_string();
    if branch.is_empty() {
        return Err(ZError::Session(
            "git preview: empty branch name".to_string(),
        ));
    }

    let is_dirty = sections.get(1).is_some_and(|s| !s.trim().is_empty());
    let commits = sections
        .get(2)
        .map(|s| parse_commits(s))
        .unwrap_or_default();
    let (ahead, behind) = sections
        .get(3)
        .map(|s| parse_ahead_behind(s))
        .unwrap_or((0, 0));

    Ok(GitPreviewInfo {
        branch,
        is_dirty,
        commits,
        ahead,
        behind,
    })
}

pub fn fetch_local_git_preview(project_path: &Path) -> Result<GitPreviewInfo> {
    let command = build_git_preview_command(&project_path.display().to_string());
    let output = Command::new("sh")
        .args(["-c", &command])
        .output()
        .map_err(|e| ZError::Session(format!("git preview failed: {e}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_git_preview_output(&stdout)
}

pub fn fetch_remote_git_preview(ssh_host: &str, project_path: &Path) -> Result<GitPreviewInfo> {
    let command = build_git_preview_command(&project_path.display().to_string());
    let output = remote::build_ssh_command(ssh_host, &command)
        .output()
        .map_err(|e| {
            ZError::Session(format!(
                "SSH to {ssh_host} failed while fetching git preview: {e}"
            ))
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_git_preview_output(&stdout)
}

pub fn to_tui_git_info(info: GitPreviewInfo) -> GitInfo {
    GitInfo {
        branch: info.branch,
        ahead: info.ahead,
        behind: info.behind,
        is_dirty: info.is_dirty,
        commits: info
            .commits
            .into_iter()
            .map(|(hash, message)| CommitInfo { hash, message })
            .collect(),
        pr: None,
        ci: None,
        zellij: None,
        review: None,
    }
}

fn parse_commits(output: &str) -> Vec<(String, String)> {
    output
        .trim()
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(2, ' ');
            let hash = parts.next()?;
            if hash.is_empty() {
                return None;
            }
            Some((hash.to_string(), parts.next().unwrap_or("").to_string()))
        })
        .collect()
}

fn parse_ahead_behind(output: &str) -> (usize, usize) {
    let mut parts = output.split_whitespace();
    let ahead = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let behind = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    (ahead, behind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clean_git_preview_output() {
        let output = "main\n---Z-GIT-PREVIEW-SEP---\n\n---Z-GIT-PREVIEW-SEP---\nabc1234 initial commit\ndef5678 add feature\n---Z-GIT-PREVIEW-SEP---\n0\t0\n";
        let info = parse_git_preview_output(output).unwrap();

        assert_eq!(info.branch, "main");
        assert!(!info.is_dirty);
        assert_eq!(info.commits.len(), 2);
        assert_eq!(info.ahead, 0);
        assert_eq!(info.behind, 0);
    }

    #[test]
    fn parses_dirty_git_preview_output() {
        let output = "feat/login\n---Z-GIT-PREVIEW-SEP---\n M src/main.rs\n?? new.txt\n---Z-GIT-PREVIEW-SEP---\nabc1234 fix bug\n---Z-GIT-PREVIEW-SEP---\n3\t2\n";
        let info = parse_git_preview_output(output).unwrap();

        assert_eq!(info.branch, "feat/login");
        assert!(info.is_dirty);
        assert_eq!(
            info.commits,
            vec![("abc1234".to_string(), "fix bug".to_string())]
        );
        assert_eq!(info.ahead, 3);
        assert_eq!(info.behind, 2);
    }

    #[test]
    fn parses_no_upstream_as_zero_counts() {
        let output = "main\n---Z-GIT-PREVIEW-SEP---\n\n---Z-GIT-PREVIEW-SEP---\nabc1234 commit\n---Z-GIT-PREVIEW-SEP---\n\n";
        let info = parse_git_preview_output(output).unwrap();

        assert_eq!(info.ahead, 0);
        assert_eq!(info.behind, 0);
    }

    #[test]
    fn build_git_preview_command_quotes_project_path() {
        let command = build_git_preview_command("/tmp/project with spaces");

        assert!(command.contains("'/tmp/project with spaces'"));
    }
}
