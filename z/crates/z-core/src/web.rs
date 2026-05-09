//! Pure helpers for the `z web` dashboard view.
//!
//! The functions here are I/O-agnostic: callers supply sessions, projects,
//! activity, and notification counts, and get back a structure ready for
//! template rendering. No filesystem, network, or process access happens
//! in this module.

use std::collections::HashMap;

use crate::activity::sort_by_recent_attach;
use crate::domain::{Project, Session};

/// A session as presented in the web dashboard.
#[derive(Debug, Clone, PartialEq)]
pub struct DashboardSession {
    /// Full session name, e.g. `myapp:main`.
    pub name: String,
    pub project: String,
    pub branch: String,
    /// Unix timestamp (seconds) of the most recent attach, if recorded.
    pub last_attach: Option<u64>,
    /// Number of pending notifications for this session.
    pub notification_count: usize,
}

/// A project and its dashboard sessions, grouped for rendering.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectGroup {
    pub project: Project,
    pub sessions: Vec<DashboardSession>,
}

/// Build the grouped dashboard view from the raw inputs.
///
/// Keeps only:
/// - sessions whose `project` matches a `Project` in `projects`;
/// - projects with no `host` set (remote projects are hidden in v1).
///
/// Groups are ordered alphabetically by project name. Sessions within a
/// group are ordered by last-attach timestamp (most recent first); sessions
/// without a recorded attach fall to the end in their input order.
pub fn dashboard_groups(
    sessions: &[Session],
    projects: &[Project],
    activity: &HashMap<String, u64>,
    notification_counts: &HashMap<String, usize>,
) -> Vec<ProjectGroup> {
    let mut groups: Vec<ProjectGroup> = Vec::new();
    for project in projects {
        if project.host.is_some() {
            continue;
        }
        let mut dash_sessions: Vec<DashboardSession> = sessions
            .iter()
            .filter(|s| s.project == project.name)
            .map(|s| DashboardSession {
                name: s.name.clone(),
                project: s.project.clone(),
                branch: s.branch.clone(),
                last_attach: activity.get(&s.name).copied(),
                notification_count: notification_counts.get(&s.name).copied().unwrap_or(0),
            })
            .collect();
        sort_by_recent_attach(&mut dash_sessions, activity, |s| s.name.as_str());
        if !dash_sessions.is_empty() {
            groups.push(ProjectGroup {
                project: project.clone(),
                sessions: dash_sessions,
            });
        }
    }
    groups.sort_by(|a, b| a.project.name.cmp(&b.project.name));
    groups
}

/// Build the URL of a specific Zellij web session.
///
/// The URL shape targets `zellij web` 0.44.x. `session` is percent-encoded
/// to be safe for any path segment; `host` is wrapped in brackets if it
/// looks like an IPv6 literal.
pub fn zellij_session_url(host: &str, port: u16, session: &str) -> String {
    let host_part = if host.contains(':') {
        format!("[{}]", host)
    } else {
        host.to_string()
    };
    format!("http://{}:{}/session/{}", host_part, port, percent_encode(session))
}

/// Percent-encode every byte that is not an RFC 3986 "unreserved" character
/// (`A-Z`, `a-z`, `0-9`, `-`, `_`, `.`, `~`). Safe inside any URL component.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{:02X}", b));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn local_project(name: &str) -> Project {
        Project {
            name: name.to_string(),
            path: PathBuf::from(format!("/tmp/{}", name)),
            host: None,
            transport: None,
        }
    }

    #[test]
    fn single_local_session_returns_one_group_with_one_session() {
        let projects = vec![local_project("myapp")];
        let sessions = vec![Session::new("myapp", "main")];
        let activity: HashMap<String, u64> = HashMap::new();
        let counts: HashMap<String, usize> = HashMap::new();

        let groups = dashboard_groups(&sessions, &projects, &activity, &counts);

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].project.name, "myapp");
        assert_eq!(groups[0].sessions.len(), 1);
        let s = &groups[0].sessions[0];
        assert_eq!(s.name, "myapp:main");
        assert_eq!(s.project, "myapp");
        assert_eq!(s.branch, "main");
        assert_eq!(s.last_attach, None);
        assert_eq!(s.notification_count, 0);
    }

    // ── zellij_session_url ───────────────────────────────────────────

    #[test]
    fn url_plain_host_and_session() {
        let url = zellij_session_url("127.0.0.1", 8082, "myapp-main");
        assert_eq!(url, "http://127.0.0.1:8082/session/myapp-main");
    }

    #[test]
    fn url_encodes_colon_in_session_name() {
        let url = zellij_session_url("127.0.0.1", 8082, "myapp:main");
        assert_eq!(url, "http://127.0.0.1:8082/session/myapp%3Amain");
    }

    #[test]
    fn url_encodes_slash_in_session_name() {
        let url = zellij_session_url("127.0.0.1", 8082, "a/b");
        assert_eq!(url, "http://127.0.0.1:8082/session/a%2Fb");
    }

    #[test]
    fn url_encodes_non_ascii_in_session_name() {
        // "café" → UTF-8 bytes c3 a9 for é
        let url = zellij_session_url("127.0.0.1", 8082, "café");
        assert_eq!(url, "http://127.0.0.1:8082/session/caf%C3%A9");
    }

    #[test]
    fn url_custom_port_is_reflected() {
        let url = zellij_session_url("host.local", 9090, "app-main");
        assert_eq!(url, "http://host.local:9090/session/app-main");
    }

    #[test]
    fn url_ipv6_host_is_bracketed() {
        let url = zellij_session_url("::1", 8082, "app-main");
        assert_eq!(url, "http://[::1]:8082/session/app-main");
    }

    // ── dashboard_groups ─────────────────────────────────────────────

    #[test]
    fn remote_project_is_excluded() {
        let remote = Project {
            name: "prod".to_string(),
            path: PathBuf::from("/tmp/prod"),
            host: Some("vps.example.com".to_string()),
            transport: None,
        };
        let projects = vec![local_project("app"), remote];
        let sessions = vec![Session::new("app", "main"), Session::new("prod", "main")];
        let groups = dashboard_groups(&sessions, &projects, &HashMap::new(), &HashMap::new());
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].project.name, "app");
    }

    #[test]
    fn session_without_matching_project_is_excluded() {
        let projects = vec![local_project("app")];
        let sessions = vec![
            Session::new("app", "main"),
            Session::new("orphan", "main"),
        ];
        let groups = dashboard_groups(&sessions, &projects, &HashMap::new(), &HashMap::new());
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].sessions.len(), 1);
        assert_eq!(groups[0].sessions[0].project, "app");
    }

    #[test]
    fn project_with_no_sessions_produces_no_group() {
        let projects = vec![local_project("a"), local_project("b")];
        let sessions = vec![Session::new("a", "main")];
        let groups = dashboard_groups(&sessions, &projects, &HashMap::new(), &HashMap::new());
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].project.name, "a");
    }

    #[test]
    fn groups_are_ordered_alphabetically_by_project_name() {
        let projects = vec![
            local_project("zeta"),
            local_project("alpha"),
            local_project("mid"),
        ];
        let sessions = vec![
            Session::new("zeta", "main"),
            Session::new("alpha", "main"),
            Session::new("mid", "main"),
        ];
        let groups = dashboard_groups(&sessions, &projects, &HashMap::new(), &HashMap::new());
        let names: Vec<&str> = groups.iter().map(|g| g.project.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "mid", "zeta"]);
    }

    #[test]
    fn last_attach_and_notification_count_are_populated() {
        let projects = vec![local_project("app")];
        let sessions = vec![Session::new("app", "main")];
        let activity: HashMap<String, u64> = [("app:main".to_string(), 1_700_000_000)]
            .into_iter()
            .collect();
        let counts: HashMap<String, usize> = [("app:main".to_string(), 3)].into_iter().collect();
        let groups = dashboard_groups(&sessions, &projects, &activity, &counts);
        let s = &groups[0].sessions[0];
        assert_eq!(s.last_attach, Some(1_700_000_000));
        assert_eq!(s.notification_count, 3);
    }

    #[test]
    fn sessions_within_group_sorted_by_last_attach_desc() {
        let projects = vec![local_project("app")];
        let sessions = vec![
            Session::new("app", "old"),
            Session::new("app", "new"),
            Session::new("app", "mid"),
        ];
        let activity: HashMap<String, u64> = [
            ("app:old".to_string(), 100),
            ("app:new".to_string(), 300),
            ("app:mid".to_string(), 200),
        ]
        .into_iter()
        .collect();
        let counts: HashMap<String, usize> = HashMap::new();

        let groups = dashboard_groups(&sessions, &projects, &activity, &counts);

        let names: Vec<&str> = groups[0].sessions.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["app:new", "app:mid", "app:old"]);
    }
}
