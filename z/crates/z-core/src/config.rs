use std::collections::HashMap;
use std::path::PathBuf;

use kdl::{KdlDocument, KdlNode};

use crate::domain::{Layout, Pane, Project, Tab};
use crate::error::{Result, ZError};
use crate::layout::default_layout;
use crate::theme::ThemeName;

/// Global configuration from `~/.config/z/config.kdl`.
#[derive(Debug, Default, Clone)]
pub struct GlobalConfig {
    pub default_layout: Option<Layout>,
    /// Navigation style: `"arrows"` or `"vim"`.
    pub navigation: Option<String>,
    pub notifications: NotificationsConfig,
    /// Tool name → minimum version requirement string (e.g. `">=0.44.0"`).
    pub deps: HashMap<String, String>,
    /// TUI color theme.
    pub theme: ThemeName,
}

/// Per-repo configuration from `.config/z.kdl` in the project root.
#[derive(Debug, Default, Clone)]
pub struct PerRepoConfig {
    /// Layout override — if set, replaces the global default layout.
    pub layout: Option<Layout>,
    /// Extra arguments passed to the `claude` command (e.g. `["--resume"]`).
    pub claude_args: Vec<String>,
    /// Shell command to run for deployment.
    pub deploy_command: Option<String>,
    /// Autopilot behaviour overrides.
    pub autopilot: AutopilotConfig,
}

/// Autopilot behaviour overrides from per-repo config.
#[derive(Debug, Clone, PartialEq)]
pub struct AutopilotConfig {
    /// Whether to auto-push after commits. Default: `true`.
    pub auto_push: bool,
    /// Whether to request review. Default: `false`.
    pub review: bool,
}

impl Default for AutopilotConfig {
    fn default() -> Self {
        Self {
            auto_push: true,
            review: false,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct NotificationsConfig {
    pub macos_native: bool,
    pub telegram: bool,
    pub tui: bool,
    /// Telegram bot token (plain string or `env:VAR`).
    pub telegram_token: Option<String>,
    /// Telegram chat ID to send messages to.
    pub telegram_chat_id: Option<String>,
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
// projects.kdl reordering
// ---------------------------------------------------------------------------

/// Swap two project nodes by index in a KDL document string.
/// Preserves all formatting, comments, and whitespace via KDL round-trip.
pub fn swap_project_nodes(content: &str, a: usize, b: usize) -> Result<String> {
    let mut doc: KdlDocument = content
        .parse()
        .map_err(|e| ZError::ConfigParse(format!("{}", e)))?;

    let project_indices: Vec<usize> = doc
        .nodes()
        .iter()
        .enumerate()
        .filter(|(_, n)| n.name().value() == "project")
        .map(|(i, _)| i)
        .collect();

    let doc_a = *project_indices
        .get(a)
        .ok_or_else(|| ZError::ConfigParse(format!("project index {} out of bounds", a)))?;
    let doc_b = *project_indices
        .get(b)
        .ok_or_else(|| ZError::ConfigParse(format!("project index {} out of bounds", b)))?;

    doc.nodes_mut().swap(doc_a, doc_b);
    Ok(doc.to_string())
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
                "theme" => {
                    if let Some(name_str) = node
                        .entries()
                        .first()
                        .and_then(|e| e.value().as_string())
                    {
                        config.theme = ThemeName::from_str(name_str).ok_or_else(|| {
                            ZError::ConfigParse(format!("unknown theme: {name_str:?}"))
                        })?;
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
                "telegram-token" => {
                    if let Some(raw) = child
                        .entries()
                        .first()
                        .and_then(|e| e.value().as_string())
                    {
                        cfg.telegram_token = resolve_env_token(raw).ok();
                    }
                }
                "telegram-chat-id" => {
                    if let Some(raw) = child
                        .entries()
                        .first()
                        .and_then(|e| e.value().as_string())
                    {
                        cfg.telegram_chat_id = resolve_env_token(raw).ok();
                    }
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
// per-repo config parsing
// ---------------------------------------------------------------------------

/// Parse the contents of `.config/z.kdl` (in the project root) into a `PerRepoConfig`.
pub fn parse_per_repo_config_kdl(content: &str) -> Result<PerRepoConfig> {
    let doc: KdlDocument = content
        .parse()
        .map_err(|e| ZError::ConfigParse(format!("{}", e)))?;

    let mut cfg = PerRepoConfig::default();

    for node in doc.nodes() {
        match node.name().value() {
            "layout" => {
                cfg.layout = Some(parse_layout_node(node)?);
            }
            "claude" => {
                cfg.claude_args = parse_claude_node(node);
            }
            "deploy" => {
                cfg.deploy_command = parse_deploy_node(node);
            }
            "autopilot" => {
                cfg.autopilot = parse_autopilot_config_node(node);
            }
            _ => {} // forward-compatible: ignore unknown nodes
        }
    }

    Ok(cfg)
}

fn parse_claude_node(node: &KdlNode) -> Vec<String> {
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
    args
}

fn parse_deploy_node(node: &KdlNode) -> Option<String> {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            if child.name().value() == "command" {
                return child
                    .entries()
                    .first()
                    .and_then(|e| e.value().as_string())
                    .map(|s| s.to_string());
            }
        }
    }
    None
}

fn parse_autopilot_config_node(node: &KdlNode) -> AutopilotConfig {
    let mut cfg = AutopilotConfig::default();
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "auto-push" => {
                    if let Some(v) = child.entries().first().and_then(|e| e.value().as_bool()) {
                        cfg.auto_push = v;
                    }
                }
                "review" => {
                    if let Some(v) = child.entries().first().and_then(|e| e.value().as_bool()) {
                        cfg.review = v;
                    }
                }
                _ => {}
            }
        }
    }
    cfg
}

// ---------------------------------------------------------------------------
// Three-tier config merging
// ---------------------------------------------------------------------------

/// Determine the effective layout using three-tier merging:
/// hardcoded default < global `default_layout` < per-repo `layout`.
///
/// If `claude_args` in the per-repo config is non-empty, they are injected into
/// any pane whose command is `"claude"`.
pub fn effective_layout(global: &GlobalConfig, per_repo: &PerRepoConfig) -> Layout {
    let mut layout = if let Some(ref l) = per_repo.layout {
        l.clone()
    } else if let Some(ref l) = global.default_layout {
        l.clone()
    } else {
        default_layout()
    };

    if !per_repo.claude_args.is_empty() {
        apply_claude_args(&mut layout, &per_repo.claude_args);
    }

    layout
}

/// Inject `args` into every pane whose command is `"claude"`.
pub fn apply_claude_args(layout: &mut Layout, args: &[String]) {
    for tab in &mut layout.tabs {
        for pane in &mut tab.panes {
            if pane.command.as_deref() == Some("claude") {
                pane.args = args.to_vec();
            }
        }
    }
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
    // parse_global_config_kdl — theme field
    // -----------------------------------------------------------------------

    #[test]
    fn parse_global_config_with_theme_dracula() {
        let kdl = r#"
config {
    theme "dracula"
}
"#;
        let cfg = parse_global_config_kdl(kdl).unwrap();
        assert_eq!(cfg.theme, ThemeName::Dracula);
    }

    #[test]
    fn parse_global_config_without_theme_defaults_to_dracula() {
        let kdl = r#"
config {
    keybindings {
        navigation "vim"
    }
}
"#;
        let cfg = parse_global_config_kdl(kdl).unwrap();
        assert_eq!(cfg.theme, ThemeName::Dracula);
    }

    #[test]
    fn parse_global_config_unknown_theme_is_error() {
        let kdl = r#"
config {
    theme "nord"
}
"#;
        assert!(matches!(
            parse_global_config_kdl(kdl).unwrap_err(),
            ZError::ConfigParse(_)
        ));
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

    // -----------------------------------------------------------------------
    // parse_per_repo_config_kdl
    // -----------------------------------------------------------------------

    #[test]
    fn parse_per_repo_empty_file() {
        let cfg = parse_per_repo_config_kdl("").unwrap();
        assert!(cfg.layout.is_none());
        assert!(cfg.claude_args.is_empty());
        assert!(cfg.deploy_command.is_none());
        assert_eq!(cfg.autopilot, AutopilotConfig::default());
    }

    #[test]
    fn parse_per_repo_full_config() {
        let kdl = r#"
layout {
    tab name="claude" {
        pane command="claude"
    }
    tab name="shell" {
        pane
    }
    tab name="server" {
        pane command="npm" {
            args "run" "dev"
        }
    }
}

claude {
    args "--resume"
}

deploy {
    command "./deploy.sh"
}

autopilot {
    auto-push true
    review false
}
"#;
        let cfg = parse_per_repo_config_kdl(kdl).unwrap();

        let layout = cfg.layout.unwrap();
        assert_eq!(layout.tabs.len(), 3);
        assert_eq!(layout.tabs[0].name, "claude");
        assert_eq!(layout.tabs[1].name, "shell");
        assert_eq!(layout.tabs[2].name, "server");
        assert_eq!(layout.tabs[2].panes[0].command.as_deref(), Some("npm"));
        assert_eq!(layout.tabs[2].panes[0].args, vec!["run", "dev"]);

        assert_eq!(cfg.claude_args, vec!["--resume"]);
        assert_eq!(cfg.deploy_command.as_deref(), Some("./deploy.sh"));
        assert!(cfg.autopilot.auto_push);
        assert!(!cfg.autopilot.review);
    }

    #[test]
    fn parse_per_repo_claude_multiple_args() {
        let kdl = r#"
claude {
    args "--resume" "--model" "opus"
}
"#;
        let cfg = parse_per_repo_config_kdl(kdl).unwrap();
        assert_eq!(cfg.claude_args, vec!["--resume", "--model", "opus"]);
    }

    #[test]
    fn parse_per_repo_autopilot_review_true() {
        let kdl = r#"
autopilot {
    auto-push false
    review true
}
"#;
        let cfg = parse_per_repo_config_kdl(kdl).unwrap();
        assert!(!cfg.autopilot.auto_push);
        assert!(cfg.autopilot.review);
    }

    #[test]
    fn parse_per_repo_malformed_is_error() {
        let kdl = "layout { this is not valid kdl !!!";
        assert!(matches!(
            parse_per_repo_config_kdl(kdl).unwrap_err(),
            ZError::ConfigParse(_)
        ));
    }

    #[test]
    fn parse_per_repo_ignores_unknown_nodes() {
        let kdl = r#"
unknown-node "foo"
claude {
    args "--resume"
}
"#;
        let cfg = parse_per_repo_config_kdl(kdl).unwrap();
        assert_eq!(cfg.claude_args, vec!["--resume"]);
    }

    // -----------------------------------------------------------------------
    // effective_layout / apply_claude_args
    // -----------------------------------------------------------------------

    #[test]
    fn effective_layout_uses_hardcoded_default_when_no_overrides() {
        let global = GlobalConfig::default();
        let per_repo = PerRepoConfig::default();
        let layout = effective_layout(&global, &per_repo);
        // hardcoded default has claude + shell
        assert_eq!(layout.tabs.len(), 2);
        assert_eq!(layout.tabs[0].name, "claude");
        assert_eq!(layout.tabs[1].name, "shell");
    }

    #[test]
    fn effective_layout_global_overrides_hardcoded_default() {
        let global = GlobalConfig {
            default_layout: Some(crate::domain::Layout {
                tabs: vec![crate::domain::Tab {
                    name: "custom".to_string(),
                    panes: vec![],
                }],
                cwd: None,
            }),
            ..Default::default()
        };
        let per_repo = PerRepoConfig::default();
        let layout = effective_layout(&global, &per_repo);
        assert_eq!(layout.tabs.len(), 1);
        assert_eq!(layout.tabs[0].name, "custom");
    }

    #[test]
    fn effective_layout_per_repo_overrides_global() {
        let global = GlobalConfig {
            default_layout: Some(crate::domain::Layout {
                tabs: vec![crate::domain::Tab {
                    name: "global-tab".to_string(),
                    panes: vec![],
                }],
                cwd: None,
            }),
            ..Default::default()
        };
        let per_repo = PerRepoConfig {
            layout: Some(crate::domain::Layout {
                tabs: vec![
                    crate::domain::Tab {
                        name: "claude".to_string(),
                        panes: vec![crate::domain::Pane {
                            command: Some("claude".to_string()),
                            args: vec![],
                        }],
                    },
                    crate::domain::Tab {
                        name: "server".to_string(),
                        panes: vec![],
                    },
                ],
                cwd: None,
            }),
            ..Default::default()
        };
        let layout = effective_layout(&global, &per_repo);
        assert_eq!(layout.tabs.len(), 2);
        assert_eq!(layout.tabs[0].name, "claude");
        assert_eq!(layout.tabs[1].name, "server");
    }

    #[test]
    fn effective_layout_injects_claude_args_into_claude_pane() {
        let global = GlobalConfig::default();
        let per_repo = PerRepoConfig {
            claude_args: vec!["--resume".to_string()],
            ..Default::default()
        };
        let layout = effective_layout(&global, &per_repo);
        let claude_tab = layout.tabs.iter().find(|t| t.name == "claude").unwrap();
        assert_eq!(claude_tab.panes[0].args, vec!["--resume"]);
    }

    #[test]
    fn effective_layout_claude_args_only_applied_to_claude_pane() {
        let global = GlobalConfig::default();
        let per_repo = PerRepoConfig {
            claude_args: vec!["--resume".to_string()],
            ..Default::default()
        };
        let layout = effective_layout(&global, &per_repo);
        let shell_tab = layout.tabs.iter().find(|t| t.name == "shell").unwrap();
        assert!(shell_tab.panes[0].args.is_empty());
    }

    #[test]
    fn apply_claude_args_modifies_claude_pane() {
        let mut layout = default_layout();
        apply_claude_args(&mut layout, &["--resume".to_string()]);
        assert_eq!(layout.tabs[0].panes[0].args, vec!["--resume"]);
        // shell pane unaffected
        assert!(layout.tabs[1].panes[0].args.is_empty());
    }

    #[test]
    fn effective_layout_claude_args_applied_to_global_layout() {
        // When per-repo has no layout but has claude_args, they should still
        // be injected into the global layout's claude pane.
        let global = GlobalConfig {
            default_layout: Some(crate::domain::Layout {
                tabs: vec![crate::domain::Tab {
                    name: "claude".to_string(),
                    panes: vec![crate::domain::Pane {
                        command: Some("claude".to_string()),
                        args: vec![],
                    }],
                }],
                cwd: None,
            }),
            ..Default::default()
        };
        let per_repo = PerRepoConfig {
            claude_args: vec!["--resume".to_string(), "--model".to_string(), "opus".to_string()],
            ..Default::default()
        };
        let layout = effective_layout(&global, &per_repo);
        assert_eq!(layout.tabs[0].panes[0].args, vec!["--resume", "--model", "opus"]);
    }

    #[test]
    fn effective_layout_claude_args_overwrite_layout_pane_args() {
        // If a per-repo layout already has args on the claude pane AND separate
        // claude_args are set, the claude_args overwrite the layout pane args.
        let global = GlobalConfig::default();
        let per_repo = PerRepoConfig {
            layout: Some(crate::domain::Layout {
                tabs: vec![crate::domain::Tab {
                    name: "claude".to_string(),
                    panes: vec![crate::domain::Pane {
                        command: Some("claude".to_string()),
                        args: vec!["--verbose".to_string()],
                    }],
                }],
                cwd: None,
            }),
            claude_args: vec!["--resume".to_string()],
            ..Default::default()
        };
        let layout = effective_layout(&global, &per_repo);
        // claude_args replaces, does not merge
        assert_eq!(layout.tabs[0].panes[0].args, vec!["--resume"]);
    }

    #[test]
    fn parse_per_repo_claude_no_children() {
        // A claude node with no children block produces empty args.
        let kdl = "claude\n";
        let cfg = parse_per_repo_config_kdl(kdl).unwrap();
        assert!(cfg.claude_args.is_empty());
    }

    #[test]
    fn parse_per_repo_deploy_no_command_child() {
        // A deploy node with children but no "command" child.
        let kdl = r#"
deploy {
    description "something"
}
"#;
        let cfg = parse_per_repo_config_kdl(kdl).unwrap();
        assert!(cfg.deploy_command.is_none());
    }

    #[test]
    fn parse_per_repo_duplicate_nodes_last_wins() {
        // When the same top-level node appears twice, the last one wins.
        let kdl = r#"
claude {
    args "--first"
}
claude {
    args "--second"
}
deploy {
    command "deploy-v1.sh"
}
deploy {
    command "deploy-v2.sh"
}
"#;
        let cfg = parse_per_repo_config_kdl(kdl).unwrap();
        assert_eq!(cfg.claude_args, vec!["--second"]);
        assert_eq!(cfg.deploy_command.as_deref(), Some("deploy-v2.sh"));
    }

    #[test]
    fn apply_claude_args_no_claude_pane_no_panic() {
        let mut layout = crate::domain::Layout {
            tabs: vec![crate::domain::Tab {
                name: "shell".to_string(),
                panes: vec![crate::domain::Pane {
                    command: None,
                    args: vec![],
                }],
            }],
            cwd: None,
        };
        // Should not panic even when there's no claude pane
        apply_claude_args(&mut layout, &["--resume".to_string()]);
        assert!(layout.tabs[0].panes[0].args.is_empty());
    }

    // -----------------------------------------------------------------------
    // swap_project_nodes
    // -----------------------------------------------------------------------

    #[test]
    fn swap_project_nodes_preserves_formatting() {
        let kdl = r#"project "alpha" {
    path "/code/alpha"
    host "https://example.com"
}
project "beta" {
    path "/code/beta"
}
"#;
        let result = swap_project_nodes(kdl, 0, 1).unwrap();
        // Alpha block should still have its host line with original formatting
        assert!(result.contains("    host \"https://example.com\""));
        // Verify the swap happened
        let projects = parse_projects_kdl(&result).unwrap();
        assert_eq!(projects[0].name, "beta");
        assert_eq!(projects[1].name, "alpha");
        assert_eq!(projects[1].host.as_deref(), Some("https://example.com"));
    }

    #[test]
    fn swap_project_nodes_out_of_bounds() {
        let kdl = r#"project "alpha" {
    path "/code/alpha"
}
project "beta" {
    path "/code/beta"
}
"#;
        let err = swap_project_nodes(kdl, 0, 5).unwrap_err();
        assert!(matches!(err, ZError::ConfigParse(_)));
    }

    #[test]
    fn swap_project_nodes_single_project() {
        let kdl = r#"project "only" {
    path "/code/only"
}
"#;
        let err = swap_project_nodes(kdl, 0, 1).unwrap_err();
        assert!(matches!(err, ZError::ConfigParse(_)));
    }

    #[test]
    fn swap_project_nodes_preserves_comments() {
        let kdl = r#"// header
project "alpha" {
    path "/code/alpha"
}
// between projects
project "beta" {
    path "/code/beta"
}
// footer
"#;
        let result = swap_project_nodes(kdl, 0, 1).unwrap();
        assert!(result.contains("// header"), "header comment preserved");
        assert!(result.contains("// between projects"), "middle comment preserved");
        assert!(result.contains("// footer"), "footer comment preserved");
        let projects = parse_projects_kdl(&result).unwrap();
        assert_eq!(projects[0].name, "beta");
        assert_eq!(projects[1].name, "alpha");
    }

    #[test]
    fn swap_project_nodes_adjacent() {
        let kdl = r#"project "alpha" {
    path "/code/alpha"
}
project "beta" {
    path "/code/beta"
}
"#;
        let result = swap_project_nodes(kdl, 0, 1).unwrap();
        let projects = parse_projects_kdl(&result).unwrap();
        assert_eq!(projects[0].name, "beta");
        assert_eq!(projects[1].name, "alpha");
    }
}
