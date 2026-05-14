/// Pure parsing for Zellij session data.
///
/// Process execution stays in CLI/TUI adapters; this Module only turns
/// `zellij list-sessions --json` output into domain-ready data.

#[derive(Debug, Clone, PartialEq)]
pub struct ZellijSessionInfo {
    pub tab_count: usize,
    pub pane_count: usize,
    pub uptime: String,
}

pub fn parse_zellij_session_info(json: &str, session_name: &str) -> Option<ZellijSessionInfo> {
    if session_name.is_empty() {
        return None;
    }

    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let session = value
        .as_array()?
        .iter()
        .find(|item| item.get("name").and_then(|name| name.as_str()) == Some(session_name))?;

    Some(ZellijSessionInfo {
        tab_count: count_field(session, &["tabs", "tab_count"]),
        pane_count: count_field(session, &["panes", "pane_count"]),
        uptime: string_field(session, &["uptime"]).unwrap_or_else(|| "unknown".to_string()),
    })
}

fn count_field(value: &serde_json::Value, names: &[&str]) -> usize {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(|field| field.as_u64()))
        .map(|count| usize::try_from(count).unwrap_or(usize::MAX))
        .unwrap_or(0)
}

fn string_field(value: &serde_json::Value, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(|field| field.as_str()))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_matching_session() {
        let json =
            r#"[{"name":"other","tabs":10,"panes":20},{"name":"target","tabs":3,"panes":5}]"#;
        let info = parse_zellij_session_info(json, "target").unwrap();
        assert_eq!(info.tab_count, 3);
        assert_eq!(info.pane_count, 5);
        assert_eq!(info.uptime, "unknown");
    }

    #[test]
    fn supports_alternate_count_field_names() {
        let json = r#"[{"name":"target","tab_count":2,"pane_count":4,"uptime":"1h"}]"#;
        let info = parse_zellij_session_info(json, "target").unwrap();
        assert_eq!(info.tab_count, 2);
        assert_eq!(info.pane_count, 4);
        assert_eq!(info.uptime, "1h");
    }

    #[test]
    fn returns_none_when_session_is_missing() {
        let json = r#"[{"name":"other","tabs":2,"panes":3}]"#;
        assert!(parse_zellij_session_info(json, "missing").is_none());
    }

    #[test]
    fn returns_none_for_malformed_json() {
        assert!(parse_zellij_session_info("not json", "target").is_none());
    }

    #[test]
    fn handles_escaped_session_name_and_uptime() {
        let json = r#"[{"name":"target \"quoted\"","tabs":1,"panes":2,"uptime":"3h"}]"#;
        let info = parse_zellij_session_info(json, "target \"quoted\"").unwrap();
        assert_eq!(info.tab_count, 1);
        assert_eq!(info.pane_count, 2);
        assert_eq!(info.uptime, "3h");
    }
}
