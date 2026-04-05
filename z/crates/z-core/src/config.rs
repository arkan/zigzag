use std::collections::HashMap;
use std::path::PathBuf;

use kdl::{KdlDocument, KdlNode};

use crate::domain::{Layout, Pane, Project, Tab};
use crate::error::{Result, ZError};

/// Global configuration from `~/.config/z/config.kdl`.
#[derive(Debug, Default, Clone)]
pub struct GlobalConfig {
    pub default_layout: Option<Layout>,
    /// Navigation style: `"arrows"` or `"vim"`.
    pub navigation: Option<String>,
    pub notifications: NotificationsConfig,
    /// Tool name → minimum version requirement string (e.g. `">=0.44.0"`).
    pub deps: HashMap<String, String>,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct NotificationsConfig {
    pub macos_native: bool,
    pub telegram: bool,
    pub tui: bool,
}

// ---------------------------------------------------------------------------
// env:VAR resolution
// ---------------------------------------------------------------------------

/// Resolve an `env:VAR` token to the actual env var value.
/// Returns the original string unchanged if it doesn't start with `env:`.
pub fn resolve_env_token(value: &str) -> Result<String> {
    match value.strip_prefix("env:") {
        Some(var_name) => std::env::var(var_name)
            .map_err(|_| ZError::EnvVarNotFound(var_name.to_string())),
        None => Ok(value.to_string()),
    }
}

// ---------------------------------------------------------------------------
// projects.kdl parsing
// ---------------------------------------------------------------------------

/// Parse the contents of `~/.config/z/projects.kdl` into a list of projects.
pub fn parse_projects_kdl(content: &str) -> Result<Vec<Project>> {
    let doc: KdlDocument = content
        .parse()
        .map_err(|e| ZError::ConfigParse(format!("{}", e)))?;

    doc.nodes()
        .iter()
        .filter(|n| n.name().value() == "project")
        .map(parse_project_node)
        .collect()
}

fn parse_project_node(node: &KdlNode) -> Result<Project> {
    let name = node
        .entries()
        .first()
        .and_then(|e| e.value().as_string())
        .ok_or_else(|| ZError::ConfigParse("project node missing name".to_string()))?
        .to_string();

    let mut path: Option<PathBuf> = None;
    let mut host: Option<String> = None;
    let mut token: Option<String> = None;

    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "path" => {
                    let raw = child
                        .entries()
                        .first()
                        .and_then(|e| e.value().as_string())
                        .ok_or_else(|| {
                            ZError::ConfigParse(format!(
                                "project '{}': path node missing value",
                                name
                            ))
                        })?;
                    path = Some(expand_tilde(raw));
                }
                "host" => {
                    let h = child
                        .entries()
                        .first()
                        .and_then(|e| e.value().as_string())
                        .ok_or_else(|| {
                            ZError::ConfigParse(format!(
                                "project '{}': host node missing value",
                                name
                            ))
                        })?;
                    host = Some(h.to_string());
                }
                "token" => {
                    let raw = child
                        .entries()
                        .first()
                        .and_then(|e| e.value().as_string())
                        .ok_or_else(|| {
                            ZError::ConfigParse(format!(
                                "project '{}': token node missing value",
                                name
                            ))
                        })?;
                    token = Some(resolve_env_token(raw)?);
                }
                "layout" => {} // layout reference, handled later
                _ => {}        // forward-compatible: ignore unknown nodes
            }
        }
    }

    let path =
        path.ok_or_else(|| ZError::ConfigParse(format!("project '{}' missing path", name)))?;

    Ok(Project {
        name,
        path,
        host,
        token,
    })
}

fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home);
        }
    } else if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

// ---------------------------------------------------------------------------
// config.kdl parsing
// ---------------------------------------------------------------------------

/// Parse the contents of `~/.config/z/config.kdl` into a `GlobalConfig`.
pub fn parse_global_config_kdl(content: &str) -> Result<GlobalConfig> {
    let doc: KdlDocument = content
        .parse()
        .map_err(|e| ZError::ConfigParse(format!("{}", e)))?;

    let mut config = GlobalConfig::default();

    let config_node = match doc.nodes().iter().find(|n| n.name().value() == "config") {
        Some(n) => n,
        None => return Ok(config),
    };

    if let Some(children) = config_node.children() {
        for node in children.nodes() {
            match node.name().value() {
                "default-layout" => {
                    config.default_layout = Some(parse_layout_node(node)?);
                }
                "keybindings" => {
                    if let Some(kb_children) = node.children() {
                        for child in kb_children.nodes() {
                            if child.name().value() == "navigation" {
                                config.navigation = child
                                    .entries()
                                    .first()
                                    .and_then(|e| e.value().as_string())
                                    .map(|s| s.to_string());
                            }
                        }
                    }
                }
                "notifications" => {
                    config.notifications = parse_notifications_node(node)?;
                }
                "deps" => {
                    config.deps = parse_deps_node(node)?;
                }
                _ => {}
            }
        }
    }

    Ok(config)
}

fn parse_layout_node(node: &KdlNode) -> Result<Layout> {
    let mut tabs = Vec::new();
    if let Some(children) = node.children() {
        for child in children.nodes() {
            if child.name().value() == "tab" {
                tabs.push(parse_tab_node(child)?);
            }
        }
    }
    Ok(Layout { tabs, cwd: None })
}

fn parse_tab_node(node: &KdlNode) -> Result<Tab> {
    let name = node
        .entries()
        .iter()
        .find(|e| e.name().map(|n| n.value()) == Some("name"))
        .and_then(|e| e.value().as_string())
        .unwrap_or("unnamed")
        .to_string();

    let mut panes = Vec::new();
    if let Some(children) = node.children() {
        for child in children.nodes() {
            if child.name().value() == "pane" {
                panes.push(parse_pane_node(child)?);
            }
        }
    }

    Ok(Tab { name, panes })
}

fn parse_pane_node(node: &KdlNode) -> Result<Pane> {
    let command = node
        .entries()
        .iter()
        .find(|e| e.name().map(|n| n.value()) == Some("command"))
        .and_then(|e| e.value().as_string())
        .map(|s| s.to_string());

    let mut args = Vec::new();
    if let Some(children) = node.children() {
        for child in children.nodes() {
            if child.name().value() == "args" {
                for entry in child.entries() {
                    if let Some(s) = entry.value().as_string() {
                        args.push(s.to_string());
                    }
                }
            }
        }
    }

    Ok(Pane { command, args })
}

fn parse_notifications_node(node: &KdlNode) -> Result<NotificationsConfig> {
    let mut cfg = NotificationsConfig::default();
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "macos-native" => {
                    cfg.macos_native = child
                        .entries()
                        .first()
                        .and_then(|e| e.value().as_bool())
                        .unwrap_or(false);
                }
                "telegram" => {
                    cfg.telegram = child
                        .entries()
                        .first()
                        .and_then(|e| e.value().as_bool())
                        .unwrap_or(false);
                }
                "tui" => {
                    cfg.tui = child
                        .entries()
                        .first()
                        .and_then(|e| e.value().as_bool())
                        .unwrap_or(false);
                }
                _ => {}
            }
        }
    }
    Ok(cfg)
}

fn parse_deps_node(node: &KdlNode) -> Result<HashMap<String, String>> {
    let mut deps = HashMap::new();
    if let Some(children) = node.children() {
        for child in children.nodes() {
            let tool = child.name().value().to_string();
            if let Some(req) = child.entries().first().and_then(|e| e.value().as_string()) {
                deps.insert(tool, req.to_string());
            }
        }
    }
    Ok(deps)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // resolve_env_token
    // -----------------------------------------------------------------------

    #[test]
    fn resolve_env_token_plain_value() {
        assert_eq!(
            resolve_env_token("https://vps.example.com").unwrap(),
            "https://vps.example.com"
        );
    }

    #[test]
    fn resolve_env_token_env_prefix_set() {
        std::env::set_var("Z_TEST_TOKEN_ABC", "secret123");
        assert_eq!(resolve_env_token("env:Z_TEST_TOKEN_ABC").unwrap(), "secret123");
        std::env::remove_var("Z_TEST_TOKEN_ABC");
    }

    #[test]
    fn resolve_env_token_env_prefix_missing() {
        std::env::remove_var("Z_TEST_TOKEN_MISSING_XYZ");
        let err = resolve_env_token("env:Z_TEST_TOKEN_MISSING_XYZ").unwrap_err();
        assert!(matches!(err, ZError::EnvVarNotFound(ref v) if v == "Z_TEST_TOKEN_MISSING_XYZ"));
    }

    // -----------------------------------------------------------------------
    // parse_projects_kdl
    // -----------------------------------------------------------------------

    #[test]
    fn parse_projects_minimal() {
        let kdl = r#"
project "myapp" {
    path "~/Code/myapp"
}
"#;
        let projects = parse_projects_kdl(kdl).unwrap();
        assert_eq!(projects.len(), 1);
        let p = &projects[0];
        assert_eq!(p.name, "myapp");
        // tilde should expand (or at least be a non-empty path)
        assert!(p.path.to_str().unwrap().contains("myapp"));
        assert!(p.host.is_none());
        assert!(p.token.is_none());
    }

    #[test]
    fn parse_projects_multiple() {
        let kdl = r#"
project "alpha" {
    path "/code/alpha"
}
project "beta" {
    path "/code/beta"
}
"#;
        let projects = parse_projects_kdl(kdl).unwrap();
        assert_eq!(projects.len(), 2);
        assert_eq!(projects[0].name, "alpha");
        assert_eq!(projects[1].name, "beta");
    }

    #[test]
    fn parse_projects_with_host() {
        let kdl = r#"
project "prod-api" {
    path "/code/prod-api"
    host "https://vps.example.com:8082"
}
"#;
        let projects = parse_projects_kdl(kdl).unwrap();
        assert_eq!(projects[0].host.as_deref(), Some("https://vps.example.com:8082"));
    }

    #[test]
    fn parse_projects_with_token_env_var() {
        std::env::set_var("Z_TEST_VPS_TOKEN", "tok_abc");
        let kdl = r#"
project "prod-api" {
    path "/code/prod-api"
    host "https://vps.example.com:8082"
    token "env:Z_TEST_VPS_TOKEN"
}
"#;
        let projects = parse_projects_kdl(kdl).unwrap();
        assert_eq!(projects[0].token.as_deref(), Some("tok_abc"));
        std::env::remove_var("Z_TEST_VPS_TOKEN");
    }

    #[test]
    fn parse_projects_token_missing_env_var_is_error() {
        std::env::remove_var("Z_TEST_TOKEN_GONE");
        let kdl = r#"
project "prod-api" {
    path "/code/prod-api"
    token "env:Z_TEST_TOKEN_GONE"
}
"#;
        assert!(matches!(
            parse_projects_kdl(kdl).unwrap_err(),
            ZError::EnvVarNotFound(_)
        ));
    }

    #[test]
    fn parse_projects_missing_path_is_error() {
        let kdl = r#"
project "broken" {
}
"#;
        assert!(matches!(
            parse_projects_kdl(kdl).unwrap_err(),
            ZError::ConfigParse(_)
        ));
    }

    #[test]
    fn parse_projects_missing_name_is_error() {
        // A project node with no string entry for name
        let kdl = r#"
project {
    path "/code/foo"
}
"#;
        assert!(matches!(
            parse_projects_kdl(kdl).unwrap_err(),
            ZError::ConfigParse(_)
        ));
    }

    #[test]
    fn parse_projects_malformed_kdl_is_error() {
        let kdl = "project { this is not valid kdl !!!";
        assert!(matches!(
            parse_projects_kdl(kdl).unwrap_err(),
            ZError::ConfigParse(_)
        ));
    }

    #[test]
    fn parse_projects_tilde_expands_with_home() {
        std::env::set_var("HOME", "/home/testuser");
        let kdl = r#"
project "myapp" {
    path "~/Code/myapp"
}
"#;
        let projects = parse_projects_kdl(kdl).unwrap();
        assert_eq!(
            projects[0].path,
            PathBuf::from("/home/testuser/Code/myapp")
        );
    }

    #[test]
    fn parse_projects_empty_file() {
        let projects = parse_projects_kdl("").unwrap();
        assert!(projects.is_empty());
    }

    // -----------------------------------------------------------------------
    // parse_global_config_kdl
    // -----------------------------------------------------------------------

    #[test]
    fn parse_global_config_empty_file() {
        let cfg = parse_global_config_kdl("").unwrap();
        assert!(cfg.default_layout.is_none());
        assert!(cfg.navigation.is_none());
        assert_eq!(cfg.notifications, NotificationsConfig::default());
        assert!(cfg.deps.is_empty());
    }

    #[test]
    fn parse_global_config_full() {
        let kdl = r#"
config {
    default-layout {
        tab name="claude" {
            pane command="claude"
        }
        tab name="shell" {
            pane
        }
    }

    keybindings {
        navigation "vim"
    }

    notifications {
        macos-native true
        telegram false
        tui true
    }

    deps {
        zellij ">=0.44.0"
        wt ">=0.34.0"
        gh ">=2.0.0"
    }
}
"#;
        let cfg = parse_global_config_kdl(kdl).unwrap();

        // Layout
        let layout = cfg.default_layout.unwrap();
        assert_eq!(layout.tabs.len(), 2);
        assert_eq!(layout.tabs[0].name, "claude");
        assert_eq!(layout.tabs[0].panes.len(), 1);
        assert_eq!(
            layout.tabs[0].panes[0].command.as_deref(),
            Some("claude")
        );
        assert_eq!(layout.tabs[1].name, "shell");
        assert_eq!(layout.tabs[1].panes.len(), 1);
        assert!(layout.tabs[1].panes[0].command.is_none());

        // Keybindings
        assert_eq!(cfg.navigation.as_deref(), Some("vim"));

        // Notifications
        assert!(cfg.notifications.macos_native);
        assert!(!cfg.notifications.telegram);
        assert!(cfg.notifications.tui);

        // Deps
        assert_eq!(cfg.deps.get("zellij").map(|s| s.as_str()), Some(">=0.44.0"));
        assert_eq!(cfg.deps.get("wt").map(|s| s.as_str()), Some(">=0.34.0"));
        assert_eq!(cfg.deps.get("gh").map(|s| s.as_str()), Some(">=2.0.0"));
    }

    #[test]
    fn parse_global_config_no_config_node_is_ok() {
        let kdl = "// just a comment\n";
        let cfg = parse_global_config_kdl(kdl).unwrap();
        assert!(cfg.default_layout.is_none());
    }

    #[test]
    fn parse_global_config_malformed_kdl_is_error() {
        let kdl = "config { this is not valid kdl !!!";
        assert!(matches!(
            parse_global_config_kdl(kdl).unwrap_err(),
            ZError::ConfigParse(_)
        ));
    }

    #[test]
    fn parse_layout_pane_with_args() {
        let kdl = r#"
config {
    default-layout {
        tab name="server" {
            pane command="npm" {
                args "run" "dev"
            }
        }
        tab name="logs" {
            pane command="tail" {
                args "-f" "/var/log/app.log"
            }
        }
    }
}
"#;
        let cfg = parse_global_config_kdl(kdl).unwrap();
        let layout = cfg.default_layout.unwrap();
        assert_eq!(layout.tabs[0].panes[0].args, vec!["run", "dev"]);
        assert_eq!(
            layout.tabs[1].panes[0].args,
            vec!["-f", "/var/log/app.log"]
        );
    }

    // -----------------------------------------------------------------------
    // expand_tilde edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn expand_tilde_bare_tilde() {
        std::env::set_var("HOME", "/home/testuser");
        assert_eq!(expand_tilde("~"), PathBuf::from("/home/testuser"));
    }

    #[test]
    fn expand_tilde_no_tilde() {
        assert_eq!(expand_tilde("/absolute/path"), PathBuf::from("/absolute/path"));
    }

    #[test]
    fn expand_tilde_tilde_in_middle_not_expanded() {
        assert_eq!(expand_tilde("/some/~/path"), PathBuf::from("/some/~/path"));
    }

    // -----------------------------------------------------------------------
    // resolve_env_token edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn resolve_env_token_empty_var_name_is_error() {
        let err = resolve_env_token("env:").unwrap_err();
        assert!(matches!(err, ZError::EnvVarNotFound(ref v) if v.is_empty()));
    }

    // -----------------------------------------------------------------------
    // parse_projects_kdl edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn parse_projects_with_plain_token() {
        let kdl = r#"
project "myapp" {
    path "/code/myapp"
    token "literal-token-value"
}
"#;
        let projects = parse_projects_kdl(kdl).unwrap();
        assert_eq!(projects[0].token.as_deref(), Some("literal-token-value"));
    }

    #[test]
    fn parse_projects_ignores_non_project_nodes() {
        let kdl = r#"
something-else "foo"
project "myapp" {
    path "/code/myapp"
}
another-thing "bar"
"#;
        let projects = parse_projects_kdl(kdl).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "myapp");
    }

    #[test]
    fn parse_projects_duplicate_names_both_returned() {
        let kdl = r#"
project "dup" {
    path "/code/dup1"
}
project "dup" {
    path "/code/dup2"
}
"#;
        let projects = parse_projects_kdl(kdl).unwrap();
        assert_eq!(projects.len(), 2);
        assert_eq!(projects[0].path, PathBuf::from("/code/dup1"));
        assert_eq!(projects[1].path, PathBuf::from("/code/dup2"));
    }

    // -----------------------------------------------------------------------
    // parse_global_config_kdl edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn parse_global_config_notifications_missing_values_default_false() {
        let kdl = r#"
config {
    notifications {
        macos-native
        telegram
        tui
    }
}
"#;
        let cfg = parse_global_config_kdl(kdl).unwrap();
        assert!(!cfg.notifications.macos_native);
        assert!(!cfg.notifications.telegram);
        assert!(!cfg.notifications.tui);
    }

    #[test]
    fn parse_global_config_deps_tool_without_version_ignored() {
        let kdl = r#"
config {
    deps {
        zellij ">=0.44.0"
        wt
    }
}
"#;
        let cfg = parse_global_config_kdl(kdl).unwrap();
        assert_eq!(cfg.deps.len(), 1);
        assert!(cfg.deps.contains_key("zellij"));
        assert!(!cfg.deps.contains_key("wt"));
    }

    #[test]
    fn parse_global_config_empty_layout() {
        let kdl = r#"
config {
    default-layout {
    }
}
"#;
        let cfg = parse_global_config_kdl(kdl).unwrap();
        let layout = cfg.default_layout.unwrap();
        assert!(layout.tabs.is_empty());
    }
}
