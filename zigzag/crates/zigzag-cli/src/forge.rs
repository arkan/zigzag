use std::process::Command;

use zigzag_core::domain::{CiStatus, PullRequest, ReviewStatus};
use zigzag_core::error::{Result, ZError};
use zigzag_core::gh::{parse_ci_status_json, parse_pr_view_json, parse_review_status_json};
use zigzag_core::traits::ForgeClient;

/// Real `ForgeClient` that delegates to the `gh` CLI.
pub struct GhForgeClient;

impl ForgeClient for GhForgeClient {
    fn get_pr(&self, _project: &str, branch: &str) -> Result<Option<PullRequest>> {
        if branch.is_empty() {
            return Ok(None);
        }
        let out = Command::new("gh")
            .args(["pr", "view", branch, "--json", "number,state,title,url"])
            .output()
            .map_err(|e| ZError::Forge(e.to_string()))?;
        if !out.status.success() {
            return Ok(None);
        }
        let json = String::from_utf8_lossy(&out.stdout);
        Ok(parse_pr_view_json(&json))
    }

    fn get_review_status(&self, _project: &str, branch: &str) -> Result<Option<ReviewStatus>> {
        if branch.is_empty() {
            return Ok(None);
        }
        let out = Command::new("gh")
            .args([
                "pr",
                "view",
                branch,
                "--json",
                "reviews,latestReviews,commits",
            ])
            .output()
            .map_err(|e| ZError::Forge(e.to_string()))?;
        if !out.status.success() {
            return Ok(None);
        }
        let json = String::from_utf8_lossy(&out.stdout);
        Ok(parse_review_status_json(&json))
    }

    fn get_ci_status(&self, _project: &str, branch: &str) -> Result<CiStatus> {
        if branch.is_empty() {
            return Ok(CiStatus::Unknown);
        }
        let out = Command::new("gh")
            .args([
                "run",
                "list",
                "--branch",
                branch,
                "--limit",
                "1",
                "--json",
                "conclusion,status",
            ])
            .output()
            .map_err(|e| ZError::Forge(e.to_string()))?;
        if !out.status.success() {
            return Ok(CiStatus::Unknown);
        }
        let json = String::from_utf8_lossy(&out.stdout);
        Ok(parse_ci_status_json(&json))
    }
}
