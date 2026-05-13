use crate::domain::{CiStatus, PrState, PullRequest, ReviewStatus};
use crate::error::{Result, ZError};

/// A GitHub item (issue or PR) parsed from `gh` CLI JSON output.
#[derive(Debug, Clone, PartialEq)]
pub struct GhItem {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub url: String,
    /// Only set for PRs (`headRefName`).
    pub branch: Option<String>,
}

/// Parse `gh issue list --json number,title,body,url` output.
pub fn parse_gh_issues(json: &str) -> Result<Vec<GhItem>> {
    let arr: Vec<serde_json::Value> =
        serde_json::from_str(json).map_err(|e| ZError::ConfigParse(format!("gh JSON: {e}")))?;
    arr.iter().map(|v| parse_item(v, false)).collect()
}

/// Parse `gh pr list --json number,title,body,url,headRefName` output.
pub fn parse_gh_prs(json: &str) -> Result<Vec<GhItem>> {
    let arr: Vec<serde_json::Value> =
        serde_json::from_str(json).map_err(|e| ZError::ConfigParse(format!("gh JSON: {e}")))?;
    arr.iter().map(|v| parse_item(v, true)).collect()
}

fn parse_item(v: &serde_json::Value, with_branch: bool) -> Result<GhItem> {
    let number = v["number"]
        .as_u64()
        .ok_or_else(|| ZError::ConfigParse("missing number".into()))?;
    let title = v["title"].as_str().unwrap_or("").to_string();
    let body = v["body"].as_str().unwrap_or("").to_string();
    let url = v["url"].as_str().unwrap_or("").to_string();
    let branch = if with_branch {
        v["headRefName"].as_str().map(|s| s.to_string())
    } else {
        None
    };
    Ok(GhItem {
        number,
        title,
        body,
        url,
        branch,
    })
}

/// Parse `gh pr view --json number,state,title,url` output.
pub fn parse_pr_view_json(json: &str) -> Option<PullRequest> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let number = value.get("number")?.as_u64()?;
    let state_raw = value.get("state").and_then(|v| v.as_str()).unwrap_or("");
    let state = match state_raw.to_uppercase().as_str() {
        "OPEN" => PrState::Open,
        "MERGED" => PrState::Merged,
        _ => PrState::Closed,
    };
    Some(PullRequest {
        number,
        state,
        title: string_field(&value, "title"),
        url: string_field(&value, "url"),
    })
}

/// Parse `gh run list --json conclusion,status` output.
pub fn parse_ci_status_json(json: &str) -> CiStatus {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return CiStatus::Unknown;
    };
    let Some(run) = first_array_item_or_object(&value) else {
        return CiStatus::Unknown;
    };

    match run.get("conclusion").and_then(|v| v.as_str()).unwrap_or("") {
        "success" => CiStatus::Passing,
        "failure" | "timed_out" => CiStatus::Failing,
        "" => match run.get("status").and_then(|v| v.as_str()).unwrap_or("") {
            "in_progress" | "queued" | "waiting" => CiStatus::Pending,
            _ => CiStatus::Unknown,
        },
        _ => CiStatus::Unknown,
    }
}

/// Parse `gh pr view --json reviews,latestReviews,commits` output.
pub fn parse_review_status_json(json: &str) -> Option<ReviewStatus> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let review_timestamps = collect_string_fields(&value, "submittedAt");
    let commit_timestamps = collect_string_fields(&value, "committedDate");

    let last_review_at = review_timestamps.iter().max().cloned();
    let last_commit_at = commit_timestamps.iter().max().cloned();
    let has_new_comments = match (&last_review_at, &last_commit_at) {
        (Some(review), Some(commit)) => review > commit,
        _ => false,
    };

    Some(ReviewStatus {
        has_new_comments,
        comment_count: review_timestamps.len() as u32,
        last_review_at,
    })
}

fn string_field(value: &serde_json::Value, name: &str) -> String {
    value
        .get(name)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn first_array_item_or_object(value: &serde_json::Value) -> Option<&serde_json::Value> {
    value
        .as_array()
        .and_then(|items| items.first())
        .or_else(|| value.as_object().map(|_| value))
}

fn collect_string_fields(value: &serde_json::Value, field: &str) -> Vec<String> {
    let mut out = Vec::new();
    collect_string_fields_into(value, field, &mut out);
    out
}

fn collect_string_fields_into(value: &serde_json::Value, field: &str, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(found) = map.get(field).and_then(|v| v.as_str()) {
                out.push(found.to_string());
            }
            for child in map.values() {
                collect_string_fields_into(child, field, out);
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                collect_string_fields_into(child, field, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_issues_valid() {
        let json = r#"[
            {"number": 42, "title": "fix login", "body": "details", "url": "https://github.com/o/r/issues/42"}
        ]"#;
        let items = parse_gh_issues(json).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].number, 42);
        assert_eq!(items[0].title, "fix login");
        assert_eq!(items[0].body, "details");
        assert!(items[0].branch.is_none());
    }

    #[test]
    fn parse_prs_valid() {
        let json = r#"[
            {"number": 99, "title": "refactor", "body": "", "url": "https://github.com/o/r/pull/99", "headRefName": "feat/refactor"}
        ]"#;
        let items = parse_gh_prs(json).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].number, 99);
        assert_eq!(items[0].branch, Some("feat/refactor".to_string()));
    }

    #[test]
    fn parse_empty_array() {
        let items = parse_gh_issues("[]").unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn parse_malformed_json_errors() {
        assert!(parse_gh_issues("not json").is_err());
    }

    #[test]
    fn parse_missing_number_errors() {
        let json = r#"[{"title": "no number"}]"#;
        assert!(parse_gh_issues(json).is_err());
    }

    #[test]
    fn parse_multiple_items() {
        let json = r#"[
            {"number": 1, "title": "a", "body": "", "url": ""},
            {"number": 2, "title": "b", "body": "", "url": ""}
        ]"#;
        let items = parse_gh_issues(json).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].number, 1);
        assert_eq!(items[1].number, 2);
    }

    #[test]
    fn parse_pr_view_json_valid() {
        let json = r#"{"number":42,"state":"OPEN","title":"feat: login","url":"https://github.com/foo/bar/pull/42"}"#;
        let pr = parse_pr_view_json(json).expect("should parse valid PR JSON");
        assert_eq!(pr.number, 42);
        assert_eq!(pr.state, PrState::Open);
        assert_eq!(pr.title, "feat: login");
        assert_eq!(pr.url, "https://github.com/foo/bar/pull/42");
    }

    #[test]
    fn parse_pr_view_json_merged() {
        let json =
            r#"{"number":7,"state":"MERGED","title":"fix: typo","url":"https://example.com/pr/7"}"#;
        let pr = parse_pr_view_json(json).unwrap();
        assert_eq!(pr.state, PrState::Merged);
    }

    #[test]
    fn parse_pr_view_json_closed() {
        let json = r#"{"number":3,"state":"CLOSED","title":"wip","url":""}"#;
        let pr = parse_pr_view_json(json).unwrap();
        assert_eq!(pr.state, PrState::Closed);
    }

    #[test]
    fn parse_pr_view_json_returns_none_on_missing_number() {
        let json = r#"{"state":"OPEN","title":"no number"}"#;
        assert!(parse_pr_view_json(json).is_none());
    }

    #[test]
    fn parse_pr_view_json_returns_none_on_empty_input() {
        assert!(parse_pr_view_json("").is_none());
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
        assert_eq!(parse_ci_status_json("[]"), CiStatus::Unknown);
    }

    #[test]
    fn parse_ci_status_unknown_on_unexpected_conclusion() {
        let json = r#"[{"conclusion":"cancelled","status":"completed"}]"#;
        assert_eq!(parse_ci_status_json(json), CiStatus::Unknown);
    }

    #[test]
    fn review_after_last_commit_has_new_comments() {
        let json = r#"{
            "reviews": [
                {"submittedAt": "2026-04-09T15:00:00Z", "body": "LGTM"}
            ],
            "commits": [
                {"committedDate": "2026-04-09T14:00:00Z"}
            ]
        }"#;
        let status = parse_review_status_json(json).unwrap();
        assert!(status.has_new_comments);
        assert_eq!(status.comment_count, 1);
        assert_eq!(
            status.last_review_at.as_deref(),
            Some("2026-04-09T15:00:00Z")
        );
    }

    #[test]
    fn review_before_last_commit_no_new_comments() {
        let json = r#"{
            "reviews": [
                {"submittedAt": "2026-04-09T13:00:00Z", "body": "needs changes"}
            ],
            "commits": [
                {"committedDate": "2026-04-09T14:00:00Z"}
            ]
        }"#;
        let status = parse_review_status_json(json).unwrap();
        assert!(!status.has_new_comments);
        assert_eq!(status.comment_count, 1);
    }

    #[test]
    fn no_reviews_no_new_comments() {
        let json = r#"{
            "reviews": [],
            "commits": [
                {"committedDate": "2026-04-09T14:00:00Z"}
            ]
        }"#;
        let status = parse_review_status_json(json).unwrap();
        assert!(!status.has_new_comments);
        assert_eq!(status.comment_count, 0);
        assert!(status.last_review_at.is_none());
    }

    #[test]
    fn multiple_reviews_uses_latest() {
        let json = r#"{
            "reviews": [
                {"submittedAt": "2026-04-09T13:00:00Z", "body": "early"},
                {"submittedAt": "2026-04-09T15:00:00Z", "body": "late"}
            ],
            "commits": [
                {"committedDate": "2026-04-09T14:00:00Z"}
            ]
        }"#;
        let status = parse_review_status_json(json).unwrap();
        assert!(status.has_new_comments);
        assert_eq!(status.comment_count, 2);
        assert_eq!(
            status.last_review_at.as_deref(),
            Some("2026-04-09T15:00:00Z")
        );
    }

    #[test]
    fn no_commits_no_new_comments() {
        let json = r#"{
            "reviews": [
                {"submittedAt": "2026-04-09T15:00:00Z", "body": "review"}
            ],
            "commits": []
        }"#;
        let status = parse_review_status_json(json).unwrap();
        assert!(!status.has_new_comments);
        assert_eq!(status.comment_count, 1);
    }

    #[test]
    fn empty_json_object_review_status() {
        let status = parse_review_status_json(r#"{}"#).unwrap();
        assert!(!status.has_new_comments);
        assert_eq!(status.comment_count, 0);
        assert!(status.last_review_at.is_none());
    }

    #[test]
    fn multiple_commits_uses_latest() {
        let json = r#"{
            "reviews": [
                {"submittedAt": "2026-04-09T15:00:00Z", "body": "review"}
            ],
            "commits": [
                {"committedDate": "2026-04-09T14:00:00Z"},
                {"committedDate": "2026-04-09T16:00:00Z"}
            ]
        }"#;
        let status = parse_review_status_json(json).unwrap();
        assert!(!status.has_new_comments);
    }
}
