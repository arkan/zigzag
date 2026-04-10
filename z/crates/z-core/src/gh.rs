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
    let title = v["title"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let body = v["body"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let url = v["url"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let branch = if with_branch {
        v["headRefName"].as_str().map(|s| s.to_string())
    } else {
        None
    };
    Ok(GhItem { number, title, body, url, branch })
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
}
