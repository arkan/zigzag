use std::path::{Path, PathBuf};
use std::process::Command;

use zigzag_core::domain::{CiStatus, PullRequest, ReviewStatus};
use zigzag_core::traits::{ForgeClient, WorktreeManager};
use zigzag_tui::{GitInfo, PreviewContext, PreviewDataSource, PreviewExtraData, ZellijInfo};

use crate::git_preview;
use crate::worktree_manager::{self, WtWorktreeManager};

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
                &context.project_name,
                &context.project_path,
                &context.branch,
                context.host.as_deref(),
            )
            .unwrap_or_else(|| context.project_path.clone())
        } else {
            context.project_path.clone()
        };

        let info = if let Some(host) = &context.host {
            git_preview::fetch_remote_git_preview(host, &effective)
        } else {
            git_preview::fetch_local_git_preview(&effective)
        }
        .map_err(|e| e.to_string())?;
        Ok(git_preview::to_tui_git_info(info))
    }

    fn load_extra_preview(&self, context: &PreviewContext) -> Result<PreviewExtraData, String> {
        let forge = load_forge_preview(&*self.forge_client, &context.project_name, &context.branch);
        Ok(PreviewExtraData {
            pr: forge.pr,
            ci: forge.ci,
            zellij: fetch_zellij_info(&context.session_name),
            review: forge.review,
        })
    }
}

struct ForgePreviewSnapshot {
    pr: Option<PullRequest>,
    ci: CiStatus,
    review: Option<ReviewStatus>,
}

fn load_forge_preview(
    forge_client: &(dyn ForgeClient + Send + Sync),
    project: &str,
    branch: &str,
) -> ForgePreviewSnapshot {
    std::thread::scope(|scope| {
        let pr = scope.spawn(|| forge_client.get_pr(project, branch));
        let ci = scope.spawn(|| forge_client.get_ci_status(project, branch));
        let review = scope.spawn(|| forge_client.get_review_status(project, branch));

        ForgePreviewSnapshot {
            pr: pr.join().unwrap_or(Ok(None)).ok().flatten(),
            ci: ci
                .join()
                .unwrap_or(Ok(CiStatus::Unknown))
                .unwrap_or(CiStatus::Unknown),
            review: review.join().unwrap_or(Ok(None)).ok().flatten(),
        }
    })
}

fn resolve_worktree_path(
    project: &str,
    project_path: &Path,
    branch: &str,
    ssh_host: Option<&str>,
) -> Option<PathBuf> {
    let worktrees = if let Some(host) = ssh_host {
        worktree_manager::list_remote_worktrees(host, project_path, project).ok()?
    } else {
        WtWorktreeManager::new(project_path.to_path_buf())
            .list_worktrees(project)
            .ok()?
    };

    worktree_manager::find_worktree_path_for_branch(&worktrees, branch)
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
        if let Some(info) = zigzag_core::zellij::parse_zellij_session_info(&json, session_name) {
            return Some(ZellijInfo {
                tab_count: info.tab_count,
                pane_count: info.pane_count,
                uptime: info.uptime,
            });
        }
    }

    let out = Command::new("zellij")
        .args(["list-sessions"])
        .output()
        .ok()?;
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
    fn extract_zellij_uptime_no_pattern_returns_none() {
        assert!(extract_zellij_uptime("myapp:main [EXITED]").is_none());
    }

    #[test]
    fn extract_zellij_uptime_extracts_duration() {
        let line = "myapp:main [Created 3h12m ago]";
        assert_eq!(extract_zellij_uptime(line), Some("3h12m".to_string()));
    }
}
