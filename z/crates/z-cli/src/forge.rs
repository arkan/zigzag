use std::process::Command;

use z_core::domain::{CiStatus, PrState, PullRequest};
use z_core::error::{Result, ZError};
use z_core::traits::ForgeClient;

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
        Ok(parse_pr_json(&json))
    }

    fn get_ci_status(&self, _project: &str, branch: &str) -> Result<CiStatus> {
        if branch.is_empty() {
            return Ok(CiStatus::Unknown);
        }
        let out = Command::new("gh")
            .args([
                "run", "list", "--branch", branch, "--limit", "1", "--json",
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

/// Parse `gh pr view --json number,state,title,url` output.
fn parse_pr_json(json: &str) -> Option<PullRequest> {
    let number = extract_json_u64(json, "number")?;
    let state_raw = extract_json_string(json, "state").unwrap_or_default();
    let state = match state_raw.to_uppercase().as_str() {
        "OPEN" => PrState::Open,
        "MERGED" => PrState::Merged,
        _ => PrState::Closed,
    };
    let title = extract_json_string(json, "title").unwrap_or_default();
    let url = extract_json_string(json, "url").unwrap_or_default();
    Some(PullRequest {
        number,
        title,
        state,
        url,
    })
}

/// Parse `gh run list --json conclusion,status` output (an array).
fn parse_ci_status_json(json: &str) -> CiStatus {
    match extract_json_string(json, "conclusion")
        .as_deref()
        .unwrap_or("")
    {
        "success" => CiStatus::Passing,
        "failure" | "timed_out" => CiStatus::Failing,
        "" => match extract_json_string(json, "status")
            .as_deref()
            .unwrap_or("")
        {
            "in_progress" | "queued" | "waiting" => CiStatus::Pending,
            _ => CiStatus::Unknown,
        },
        _ => CiStatus::Unknown,
    }
}

/// Extract a u64 value from a simple JSON object: `"key": 42`.
fn extract_json_u64(json: &str, key: &str) -> Option<u64> {
    let needle = format!("\"{}\":", key);
    let start = json.find(&needle)? + needle.len();
    let rest = json[start..].trim_start();
    rest.split(|c: char| !c.is_ascii_digit())
        .next()
        .and_then(|s| s.parse().ok())
}

/// Extract a string value from a simple JSON object: `"key": "value"`.
fn extract_json_string(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\":", key);
    let after_colon = json.find(&needle)? + needle.len();
    let trimmed = json[after_colon..].trim_start();
    if !trimmed.starts_with('"') {
        return None;
    }
    let rest = &trimmed[1..];
    let mut result = String::new();
    let mut chars = rest.chars().peekable();
    loop {
        match chars.next()? {
            '"' => break,
            '\\' => {
                if let Some(c) = chars.next() {
                    result.push(c);
                }
            }
            c => result.push(c),
        }
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pr_json_valid() {
        let json = r#"{"number":42,"state":"OPEN","title":"feat: login","url":"https://github.com/foo/bar/pull/42"}"#;
        let pr = parse_pr_json(json).expect("should parse valid PR JSON");
        assert_eq!(pr.number, 42);
        assert_eq!(pr.state, PrState::Open);
        assert_eq!(pr.title, "feat: login");
        assert_eq!(pr.url, "https://github.com/foo/bar/pull/42");
    }

    #[test]
    fn parse_pr_json_merged() {
        let json = r#"{"number":7,"state":"MERGED","title":"fix: typo","url":"https://example.com/pr/7"}"#;
        let pr = parse_pr_json(json).unwrap();
        assert_eq!(pr.state, PrState::Merged);
    }

    #[test]
    fn parse_pr_json_closed() {
        let json = r#"{"number":3,"state":"CLOSED","title":"wip","url":""}"#;
        let pr = parse_pr_json(json).unwrap();
        assert_eq!(pr.state, PrState::Closed);
    }

    #[test]
    fn parse_pr_json_returns_none_on_missing_number() {
        let json = r#"{"state":"OPEN","title":"no number"}"#;
        assert!(parse_pr_json(json).is_none());
    }

    #[test]
    fn parse_pr_json_returns_none_on_empty_input() {
        assert!(parse_pr_json("").is_none());
    }

    #[test]
    fn parse_ci_status_success() {
        let json = r#"[{"conclusion":"success","status":"completed"}]"#;
        assert_eq!(parse_ci_status_json(json), CiStatus::Passing);
    }

    #[test]
    fn parse_ci_status_failure() {
        let json = r#"[{"conclusion":"failure","status":"completed"}]"#;
        assert_eq!(parse_ci_status_json(json), CiStatus::Failing);
    }

    #[test]
    fn parse_ci_status_timed_out() {
        let json = r#"[{"conclusion":"timed_out","status":"completed"}]"#;
        assert_eq!(parse_ci_status_json(json), CiStatus::Failing);
    }

    #[test]
    fn parse_ci_status_in_progress() {
        let json = r#"[{"conclusion":"","status":"in_progress"}]"#;
        assert_eq!(parse_ci_status_json(json), CiStatus::Pending);
    }

    #[test]
    fn parse_ci_status_queued() {
        let json = r#"[{"conclusion":"","status":"queued"}]"#;
        assert_eq!(parse_ci_status_json(json), CiStatus::Pending);
    }

    #[test]
    fn parse_ci_status_unknown_on_empty_array() {
        let json = "[]";
        assert_eq!(parse_ci_status_json(json), CiStatus::Unknown);
    }

    #[test]
    fn parse_ci_status_unknown_on_unexpected_conclusion() {
        let json = r#"[{"conclusion":"cancelled","status":"completed"}]"#;
        assert_eq!(parse_ci_status_json(json), CiStatus::Unknown);
    }
}
