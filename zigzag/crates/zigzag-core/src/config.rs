use std::collections::HashMap;
use std::path::PathBuf;

use kdl::{KdlDocument, KdlNode};

use crate::action::{self, ActionDef};
use crate::domain::{Layout, Pane, Project, Tab, Transport};
use crate::error::{Result, ZError};
use crate::layout::default_layout;
use crate::theme::ThemeName;

/// Global configuration from `~/.config/zigzag/config.kdl`.
#[derive(Debug, Clone)]
pub struct GlobalConfig {
    pub default_layout: Option<Layout>,
    /// Navigation style: `"arrows"` or `"vim"`.
    pub navigation: Option<String>,
    pub notifications: NotificationsConfig,
    /// Tool name → minimum version requirement string (e.g. `">=0.44.0"`).
    pub deps: HashMap<String, String>,
    /// TUI color theme.
    pub theme: ThemeName,
    /// User-defined actions from the `actions { ... }` block.
    pub actions: Vec<ActionDef>,
    /// Default AI review tool (e.g. `"codex"`). Read from `actions { review-tool "..." }`.
    pub review_tool: String,
    /// Prompt template for sessions created from a GitHub issue.
    pub issue_prompt_template: Option<String>,
    /// Prompt template for sessions created from a GitHub PR.
    pub pr_prompt_template: Option<String>,
    /// Agent activity runtime behavior.
    pub llm: LlmConfig,
    /// In-session switcher ordering behavior.
    pub switcher: SwitcherConfig,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            default_layout: None,
            navigation: None,
            notifications: NotificationsConfig::default(),
            deps: HashMap::new(),
            theme: ThemeName::default(),
            actions: Vec::new(),
            review_tool: "codex".to_string(),
            issue_prompt_template: None,
            pr_prompt_template: None,
            llm: LlmConfig::default(),
            switcher: SwitcherConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmConfig {
    pub working_ttl_seconds: u64,
    pub working_update_min_interval_seconds: u64,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            working_ttl_seconds: 120,
            working_update_min_interval_seconds: 5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SwitcherPriorityCriterion {
    Waiting,
    Error,
    Working,
    Notifications,
    Recent,
}

impl SwitcherPriorityCriterion {
    /// Backwards-compatible alias for callers that used the pre-Rust-1.95 helper.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(value: &str) -> Option<Self> {
        Self::parse_str(value)
    }

    pub fn parse_str(value: &str) -> Option<Self> {
        match value {
            "waiting" => Some(Self::Waiting),
            "error" => Some(Self::Error),
            "working" => Some(Self::Working),
            "notifications" => Some(Self::Notifications),
            "recent" => Some(Self::Recent),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Error => "error",
            Self::Working => "working",
            Self::Notifications => "notifications",
            Self::Recent => "recent",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitcherConfig {
    pub priority: Vec<SwitcherPriorityCriterion>,
    pub invalid_priorities: Vec<String>,
}

impl Default for SwitcherConfig {
    fn default() -> Self {
        Self {
            priority: default_switcher_priority(),
            invalid_priorities: Vec::new(),
        }
    }
}

pub fn default_switcher_priority() -> Vec<SwitcherPriorityCriterion> {
    vec![
        SwitcherPriorityCriterion::Recent,
        SwitcherPriorityCriterion::Waiting,
        SwitcherPriorityCriterion::Error,
        SwitcherPriorityCriterion::Working,
        SwitcherPriorityCriterion::Notifications,
    ]
}

/// Per-repo configuration from `.config/zigzag.kdl` in the project root.
#[derive(Debug, Default, Clone)]
pub struct PerRepoConfig {
    /// Layout override — if set, replaces the global default layout.
    pub layout: Option<Layout>,
    /// Shell command to run for deployment.
    pub deploy_command: Option<String>,
    /// Autopilot behaviour overrides.
    pub autopilot: AutopilotConfig,
    /// Project-specific actions from the `actions { ... }` block.
    pub actions: Vec<ActionDef>,
    /// Prompt template for sessions created from a GitHub issue.
    pub issue_prompt_template: Option<String>,
    /// Prompt template for sessions created from a GitHub PR.
    pub pr_prompt_template: Option<String>,
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
    resolve_env_token_with_environment(value, &ProcessConfigEnvironment)
}

/// Environment Interface used by config parsing.
pub trait ConfigEnvironment {
    fn var(&self, name: &str) -> Option<String>;
}

/// Adapter backed by the current process environment.
#[derive(Debug, Default, Clone, Copy)]
pub struct ProcessConfigEnvironment;

impl ConfigEnvironment for ProcessConfigEnvironment {
    fn var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

/// Resolve an `env:VAR` token through an injected environment.
pub fn resolve_env_token_with_environment(
    value: &str,
    env: &impl ConfigEnvironment,
) -> Result<String> {
    match value.strip_prefix("env:") {
        Some(var_name) => env
            .var(var_name)
            .ok_or_else(|| ZError::EnvVarNotFound(var_name.to_string())),
        None => Ok(value.to_string()),
    }
}

// ---------------------------------------------------------------------------
// projects.kdl parsing
// ---------------------------------------------------------------------------

/// Parse the contents of `~/.config/zigzag/projects.kdl` into a list of projects.
pub fn parse_projects_kdl(content: &str) -> Result<Vec<Project>> {
    parse_projects_kdl_with_environment(content, &ProcessConfigEnvironment)
}

/// Parse projects through an injected environment for deterministic path expansion.
pub fn parse_projects_kdl_with_environment(
    content: &str,
    env: &impl ConfigEnvironment,
) -> Result<Vec<Project>> {
    let doc: KdlDocument = content
        .parse()
        .map_err(|e| ZError::ConfigParse(format!("{}", e)))?;

    doc.nodes()
        .iter()
        .filter(|n| n.name().value() == "project")
        .map(|node| parse_project_node(node, env))
        .collect()
}

fn parse_project_node(node: &KdlNode, env: &impl ConfigEnvironment) -> Result<Project> {
    let name = node
        .entries()
        .first()
        .and_then(|e| e.value().as_string())
        .ok_or_else(|| ZError::ConfigParse("project node missing name".to_string()))?
        .to_string();

    let mut path: Option<PathBuf> = None;
    let mut host: Option<String> = None;
    let mut transport: Option<Transport> = None;

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
                    path = Some(expand_tilde_with_environment(raw, env));
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
                "token" => {} // deprecated, ignored for backward compatibility
                "transport" => {
                    let val = child
                        .entries()
                        .first()
                        .and_then(|e| e.value().as_string())
                        .ok_or_else(|| {
                            ZError::ConfigParse(format!(
                                "project '{}': transport node missing value",
                                name
                            ))
                        })?;
                    transport = Some(match val {
                        "ssh" => Transport::Ssh,
                        "mosh" => Transport::Mosh,
                        other => {
                            return Err(ZError::ConfigParse(format!(
                                "project '{}': unknown transport {:?} (expected \"ssh\" or \"mosh\")",
                                name, other
                            )));
                        }
                    });
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
        transport,
    })
}

#[cfg(test)]
fn expand_tilde(path: &str) -> PathBuf {
    expand_tilde_with_environment(path, &ProcessConfigEnvironment)
}

fn expand_tilde_with_environment(path: &str, env: &impl ConfigEnvironment) -> PathBuf {
    if path == "~" {
        if let Some(home) = env.var("HOME") {
            return PathBuf::from(home);
        }
    } else if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = env.var("HOME") {
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

/// Parse the contents of `~/.config/zigzag/config.kdl` into a `GlobalConfig`.
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
                    if let Some(name_str) =
                        node.entries().first().and_then(|e| e.value().as_string())
                    {
                        config.theme = ThemeName::parse_str(name_str).ok_or_else(|| {
                            ZError::ConfigParse(format!("unknown theme: {name_str:?}"))
                        })?;
                    }
                }
                "notifications" => {
                    config.notifications = parse_notifications_node(node)?;
                }
                "llm" => {
                    config.llm = parse_llm_config_node(node);
                }
                "switcher" => {
                    config.switcher = parse_switcher_config_node(node);
                }
                "deps" => {
                    config.deps = parse_deps_node(node)?;
                }
                "actions" => {
                    parse_actions_config_node(node, &mut config)?;
                }
                "issue-prompt-template" => {
                    config.issue_prompt_template = node
                        .entries()
                        .first()
                        .and_then(|e| e.value().as_string())
                        .map(|s| s.to_string());
                }
                "pr-prompt-template" => {
                    config.pr_prompt_template = node
                        .entries()
                        .first()
                        .and_then(|e| e.value().as_string())
                        .map(|s| s.to_string());
                }
                _ => {}
            }
        }
    }

    Ok(config)
}

fn parse_actions_config_node(node: &KdlNode, config: &mut GlobalConfig) -> Result<()> {
    if let Some(children) = node.children() {
        // Collect action nodes into a KDL string for parse_actions_kdl
        let mut action_kdl = String::new();
        for child in children.nodes() {
            match child.name().value() {
                "review-tool" => {
                    if let Some(val) = child
                        .entries()
                        .iter()
                        .find(|e| e.name().is_none())
                        .and_then(|e| e.value().as_string())
                    {
                        config.review_tool = val.to_string();
                    }
                }
                "action" => {
                    action_kdl.push_str(&child.to_string());
                    action_kdl.push('\n');
                }
                _ => {} // forward-compatible
            }
        }
        if !action_kdl.is_empty() {
            config.actions = action::parse_actions_kdl(&action_kdl)?;
        }
    }
    Ok(())
}

fn parse_llm_config_node(node: &KdlNode) -> LlmConfig {
    let mut config = LlmConfig::default();
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "working-ttl-seconds" => {
                    if let Some(value) = child.entries().first().and_then(|e| e.value().as_i64()) {
                        if value > 0 {
                            config.working_ttl_seconds = value as u64;
                        }
                    }
                }
                "working-update-min-interval-seconds" => {
                    if let Some(value) = child.entries().first().and_then(|e| e.value().as_i64()) {
                        if value > 0 {
                            config.working_update_min_interval_seconds = value as u64;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    config
}

fn parse_switcher_config_node(node: &KdlNode) -> SwitcherConfig {
    let mut configured = Vec::new();
    let mut invalid = Vec::new();

    if let Some(children) = node.children() {
        for child in children.nodes() {
            if child.name().value() != "priority" {
                continue;
            }
            let Some(value) = child.entries().first().and_then(|e| e.value().as_string()) else {
                continue;
            };
            match SwitcherPriorityCriterion::parse_str(value) {
                Some(criterion) if !configured.contains(&criterion) => configured.push(criterion),
                Some(_) => {}
                None => invalid.push(value.to_string()),
            }
        }
    }

    if configured.is_empty() {
        configured = default_switcher_priority();
    } else {
        for criterion in default_switcher_priority() {
            if !configured.contains(&criterion) {
                configured.push(criterion);
            }
        }
    }

    SwitcherConfig {
        priority: configured,
        invalid_priorities: invalid,
    }
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
    Ok(Layout {
        tabs,
        cwd: None,
        session_name_env: None,
    })
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
                    if let Some(raw) = child.entries().first().and_then(|e| e.value().as_string()) {
                        cfg.telegram_token = resolve_env_token(raw).ok();
                    }
                }
                "telegram-chat-id" => {
                    if let Some(raw) = child.entries().first().and_then(|e| e.value().as_string()) {
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

/// Parse the contents of `.config/zigzag.kdl` (in the project root) into a `PerRepoConfig`.
pub fn parse_per_repo_config_kdl(content: &str) -> Result<PerRepoConfig> {
    let doc: KdlDocument = content
        .parse()
        .map_err(|e| ZError::ConfigParse(format!("{}", e)))?;

    parse_per_repo_config_doc(&doc)
}

/// Project `.config/zigzag.kdl` projection from an already-parsed KDL document.
pub fn parse_per_repo_config_doc(doc: &KdlDocument) -> Result<PerRepoConfig> {
    let mut cfg = PerRepoConfig::default();

    for node in doc.nodes() {
        match node.name().value() {
            "layout" => {
                cfg.layout = Some(parse_layout_node(node)?);
            }
            "deploy" => {
                cfg.deploy_command = parse_deploy_node(node);
            }
            "autopilot" => {
                apply_autopilot_config_node(node, &mut cfg.autopilot)?;
            }
            "actions" => {
                if let Some(children) = node.children() {
                    let mut action_kdl = String::new();
                    for child in children.nodes() {
                        if child.name().value() == "action" {
                            action_kdl.push_str(&child.to_string());
                            action_kdl.push('\n');
                        }
                    }
                    if !action_kdl.is_empty() {
                        cfg.actions = action::parse_actions_kdl(&action_kdl)?;
                    }
                }
            }
            "issue-prompt-template" => {
                cfg.issue_prompt_template = node
                    .entries()
                    .first()
                    .and_then(|e| e.value().as_string())
                    .map(|s| s.to_string());
            }
            "pr-prompt-template" => {
                cfg.pr_prompt_template = node
                    .entries()
                    .first()
                    .and_then(|e| e.value().as_string())
                    .map(|s| s.to_string());
            }
            _ => {} // forward-compatible: ignore unknown nodes
        }
    }

    Ok(cfg)
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

/// Parse the unnamed `autopilot { ... }` config blocks from KDL content.
///
/// Named `autopilot "workflow" { ... }` blocks belong to the Autopilot DSL and
/// are intentionally ignored here.
pub fn parse_autopilot_config_kdl(content: &str) -> Result<AutopilotConfig> {
    let doc: KdlDocument = content
        .parse()
        .map_err(|e| ZError::ConfigParse(format!("{}", e)))?;

    parse_autopilot_config_doc(&doc)
}

/// Unnamed Autopilot config projection from an already-parsed KDL document.
pub fn parse_autopilot_config_doc(doc: &KdlDocument) -> Result<AutopilotConfig> {
    let mut cfg = AutopilotConfig::default();
    for node in doc.nodes() {
        if node.name().value() == "autopilot" {
            apply_autopilot_config_node(node, &mut cfg)?;
        }
    }
    Ok(cfg)
}

fn apply_autopilot_config_node(node: &KdlNode, cfg: &mut AutopilotConfig) -> Result<()> {
    if node.entries().iter().any(|entry| entry.name().is_none()) {
        return Ok(());
    }
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "auto-push" => {
                    if let Some(v) = require_bool_arg(child, "autopilot config")? {
                        cfg.auto_push = v;
                    }
                }
                "review" => {
                    if let Some(v) = require_bool_arg(child, "autopilot config")? {
                        cfg.review = v;
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn require_bool_arg(node: &KdlNode, context: &str) -> Result<Option<bool>> {
    let entry = node.entries().iter().find(|entry| entry.name().is_none());
    match entry {
        None => Ok(None),
        Some(entry) => entry.value().as_bool().map(Some).ok_or_else(|| {
            ZError::ConfigParse(format!(
                "{context}: '{}' expects a boolean (true/false), got {}",
                node.name().value(),
                entry.value()
            ))
        }),
    }
}

// ---------------------------------------------------------------------------
// Three-tier config merging
// ---------------------------------------------------------------------------

/// Determine the effective layout using three-tier merging:
/// hardcoded default < global `default_layout` < per-repo `layout`.
/// The lowest tier wins entirely — no partial merge.
pub fn effective_layout(global: &GlobalConfig, per_repo: &PerRepoConfig) -> Layout {
    if let Some(ref l) = per_repo.layout {
        l.clone()
    } else if let Some(ref l) = global.default_layout {
        l.clone()
    } else {
        default_layout()
    }
}

/// Resolve the effective issue prompt template: per-repo > global > hardcoded default.
pub fn effective_issue_prompt_template(global: &GlobalConfig, per_repo: &PerRepoConfig) -> String {
    per_repo
        .issue_prompt_template
        .clone()
        .or_else(|| global.issue_prompt_template.clone())
        .unwrap_or_else(|| crate::template::DEFAULT_ISSUE_TEMPLATE.to_string())
}

/// Resolve the effective PR prompt template: per-repo > global > hardcoded default.
pub fn effective_pr_prompt_template(global: &GlobalConfig, per_repo: &PerRepoConfig) -> String {
    per_repo
        .pr_prompt_template
        .clone()
        .or_else(|| global.pr_prompt_template.clone())
        .unwrap_or_else(|| crate::template::DEFAULT_PR_TEMPLATE.to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct TestEnvironment {
        vars: HashMap<String, String>,
    }

    impl TestEnvironment {
        fn with_var(mut self, name: &str, value: &str) -> Self {
            self.vars.insert(name.to_string(), value.to_string());
            self
        }
    }

    impl ConfigEnvironment for TestEnvironment {
        fn var(&self, name: &str) -> Option<String> {
            self.vars.get(name).cloned()
        }
    }

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
        assert_eq!(
            resolve_env_token("env:Z_TEST_TOKEN_ABC").unwrap(),
            "secret123"
        );
        std::env::remove_var("Z_TEST_TOKEN_ABC");
    }

    #[test]
    fn resolve_env_token_env_prefix_missing() {
        std::env::remove_var("Z_TEST_TOKEN_MISSING_XYZ");
        let err = resolve_env_token("env:Z_TEST_TOKEN_MISSING_XYZ").unwrap_err();
        assert!(matches!(err, ZError::EnvVarNotFound(ref v) if v == "Z_TEST_TOKEN_MISSING_XYZ"));
    }

    #[test]
    fn resolve_env_token_with_environment_uses_injected_value() {
        let env = TestEnvironment::default().with_var("TOKEN", "secret123");
        assert_eq!(
            resolve_env_token_with_environment("env:TOKEN", &env).unwrap(),
            "secret123"
        );
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
    host "vps.example.com"
}
"#;
        let projects = parse_projects_kdl(kdl).unwrap();
        assert_eq!(projects[0].host.as_deref(), Some("vps.example.com"));
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
        assert_eq!(projects[0].path, PathBuf::from("/home/testuser/Code/myapp"));
    }

    #[test]
    fn parse_projects_kdl_with_environment_expands_tilde() {
        let env = TestEnvironment::default().with_var("HOME", "/home/testuser");
        let kdl = r#"
project "myapp" {
    path "~/Code/myapp"
}
"#;
        let projects = parse_projects_kdl_with_environment(kdl, &env).unwrap();
        assert_eq!(projects[0].path, PathBuf::from("/home/testuser/Code/myapp"));
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
        assert_eq!(cfg.llm, LlmConfig::default());
        assert_eq!(cfg.switcher, SwitcherConfig::default());
    }

    #[test]
    fn parse_global_config_llm_settings_inside_config_node() {
        let kdl = r#"
config {
    llm {
        working-ttl-seconds 240
        working-update-min-interval-seconds 7
    }
}
"#;

        let cfg = parse_global_config_kdl(kdl).unwrap();

        assert_eq!(cfg.llm.working_ttl_seconds, 240);
        assert_eq!(cfg.llm.working_update_min_interval_seconds, 7);
    }

    #[test]
    fn parse_global_config_switcher_priorities_append_missing_defaults() {
        let kdl = r#"
config {
    switcher {
        priority "recent"
        priority "waiting"
    }
}
"#;

        let cfg = parse_global_config_kdl(kdl).unwrap();

        assert_eq!(
            cfg.switcher.priority,
            vec![
                SwitcherPriorityCriterion::Recent,
                SwitcherPriorityCriterion::Waiting,
                SwitcherPriorityCriterion::Error,
                SwitcherPriorityCriterion::Working,
                SwitcherPriorityCriterion::Notifications,
            ]
        );
        assert!(cfg.switcher.invalid_priorities.is_empty());
    }

    #[test]
    fn default_switcher_priority_starts_with_recent() {
        let priority = default_switcher_priority();
        assert_eq!(priority[0], SwitcherPriorityCriterion::Recent);
        assert!(priority.contains(&SwitcherPriorityCriterion::Waiting));
        assert!(priority.contains(&SwitcherPriorityCriterion::Error));
    }

    #[test]
    fn parse_global_config_switcher_invalid_priorities_are_diagnostic() {
        let kdl = r#"
config {
    switcher {
        priority "waiting"
        priority "made-up"
    }
}
"#;

        let cfg = parse_global_config_kdl(kdl).unwrap();

        assert_eq!(cfg.switcher.priority[0], SwitcherPriorityCriterion::Waiting);
        assert_eq!(cfg.switcher.invalid_priorities, vec!["made-up".to_string()]);
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
        assert_eq!(layout.tabs[0].panes[0].command.as_deref(), Some("claude"));
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
        assert_eq!(layout.tabs[1].panes[0].args, vec!["-f", "/var/log/app.log"]);
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
    fn expand_tilde_with_environment_missing_home_leaves_path() {
        let env = TestEnvironment::default();
        assert_eq!(
            expand_tilde_with_environment("~/Code", &env),
            PathBuf::from("~/Code")
        );
    }

    #[test]
    fn expand_tilde_no_tilde() {
        assert_eq!(
            expand_tilde("/absolute/path"),
            PathBuf::from("/absolute/path")
        );
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
    fn parse_projects_token_field_is_ignored() {
        let kdl = r#"
project "myapp" {
    path "/code/myapp"
    token "literal-token-value"
}
"#;
        let projects = parse_projects_kdl(kdl).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "myapp");
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
        assert!(cfg.deploy_command.is_none());
        assert_eq!(cfg.autopilot, AutopilotConfig::default());
    }

    #[test]
    fn parse_per_repo_full_config() {
        let kdl = r#"
layout {
    tab name="claude" {
        pane command="claude" {
            args "--resume"
        }
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
        assert_eq!(layout.tabs[0].panes[0].args, vec!["--resume"]);
        assert_eq!(layout.tabs[1].name, "shell");
        assert_eq!(layout.tabs[2].name, "server");
        assert_eq!(layout.tabs[2].panes[0].command.as_deref(), Some("npm"));
        assert_eq!(layout.tabs[2].panes[0].args, vec!["run", "dev"]);

        assert_eq!(cfg.deploy_command.as_deref(), Some("./deploy.sh"));
        assert!(cfg.autopilot.auto_push);
        assert!(!cfg.autopilot.review);
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
    fn parse_per_repo_autopilot_named_workflow_is_ignored_by_config() {
        let kdl = r#"
autopilot "workflow" {
    auto-push false
}
"#;
        let cfg = parse_per_repo_config_kdl(kdl).unwrap();

        assert_eq!(cfg.autopilot, AutopilotConfig::default());
    }

    #[test]
    fn parse_autopilot_config_kdl_rejects_string_bool() {
        let kdl = "autopilot {\n    auto-push \"false\"\n}\n";
        let err = parse_autopilot_config_kdl(kdl).unwrap_err();

        assert!(err.to_string().contains("boolean"));
    }

    #[test]
    fn parse_autopilot_config_kdl_merges_multiple_unnamed_blocks() {
        let kdl = r#"
autopilot {
    auto-push false
}
autopilot {
    review true
}
"#;
        let cfg = parse_autopilot_config_kdl(kdl).unwrap();

        assert!(!cfg.auto_push);
        assert!(cfg.review);
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
deploy {
    command "./deploy.sh"
}
"#;
        let cfg = parse_per_repo_config_kdl(kdl).unwrap();
        assert_eq!(cfg.deploy_command.as_deref(), Some("./deploy.sh"));
    }

    // -----------------------------------------------------------------------
    // effective_layout
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
        // claude pane carries default args from hardcoded layout
        assert_eq!(
            layout.tabs[0].panes[0].args,
            vec!["--dangerously-skip-permissions"]
        );
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
                session_name_env: None,
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
                session_name_env: None,
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
                            args: vec!["--resume".to_string()],
                        }],
                    },
                    crate::domain::Tab {
                        name: "server".to_string(),
                        panes: vec![],
                    },
                ],
                cwd: None,
                session_name_env: None,
            }),
            ..Default::default()
        };
        let layout = effective_layout(&global, &per_repo);
        assert_eq!(layout.tabs.len(), 2);
        assert_eq!(layout.tabs[0].name, "claude");
        assert_eq!(layout.tabs[0].panes[0].args, vec!["--resume"]);
        assert_eq!(layout.tabs[1].name, "server");
    }

    #[test]
    fn effective_layout_per_repo_args_on_any_command() {
        let global = GlobalConfig::default();
        let per_repo = PerRepoConfig {
            layout: Some(crate::domain::Layout {
                tabs: vec![crate::domain::Tab {
                    name: "server".to_string(),
                    panes: vec![crate::domain::Pane {
                        command: Some("npm".to_string()),
                        args: vec!["run".to_string(), "dev".to_string()],
                    }],
                }],
                cwd: None,
                session_name_env: None,
            }),
            ..Default::default()
        };
        let layout = effective_layout(&global, &per_repo);
        assert_eq!(layout.tabs[0].panes[0].args, vec!["run", "dev"]);
    }

    #[test]
    fn parse_per_repo_ignores_legacy_claude_block() {
        // Old-style `claude {}` block is silently ignored.
        let kdl = r#"
claude {
    args "--resume"
}
"#;
        let cfg = parse_per_repo_config_kdl(kdl).unwrap();
        assert!(cfg.layout.is_none());
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
deploy {
    command "deploy-v1.sh"
}
deploy {
    command "deploy-v2.sh"
}
"#;
        let cfg = parse_per_repo_config_kdl(kdl).unwrap();
        assert_eq!(cfg.deploy_command.as_deref(), Some("deploy-v2.sh"));
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
        assert!(
            result.contains("// between projects"),
            "middle comment preserved"
        );
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

    // -----------------------------------------------------------------------
    // actions in global config
    // -----------------------------------------------------------------------

    #[test]
    fn parse_global_config_with_actions() {
        let kdl = r#"
config {
    actions {
        review-tool "codex"

        action "Run tests" {
            run "cargo test"
            context "project"
        }

        action "Review PR" {
            run "codex review #${pr_number}"
            when "has_pr"
            context "session"
            pane "tab"
            icon "🔍"
        }
    }
}
"#;
        let config = parse_global_config_kdl(kdl).unwrap();
        assert_eq!(config.review_tool, "codex");
        assert_eq!(config.actions.len(), 2);
        assert_eq!(config.actions[0].name, "Run tests");
        assert_eq!(config.actions[1].name, "Review PR");
    }

    #[test]
    fn parse_global_config_review_tool_override() {
        let kdl = r#"
config {
    actions {
        review-tool "claude"
    }
}
"#;
        let config = parse_global_config_kdl(kdl).unwrap();
        assert_eq!(config.review_tool, "claude");
        assert!(config.actions.is_empty());
    }

    #[test]
    fn parse_global_config_no_actions_block() {
        let kdl = r#"
config {
    keybindings {
        navigation "vim"
    }
}
"#;
        let config = parse_global_config_kdl(kdl).unwrap();
        assert!(config.actions.is_empty());
        assert_eq!(config.review_tool, "codex"); // default
    }

    #[test]
    fn parse_global_config_actions_with_disabled() {
        let kdl = r#"
config {
    actions {
        action "Open PR" {
            run "echo disabled"
            disabled true
        }
    }
}
"#;
        let config = parse_global_config_kdl(kdl).unwrap();
        assert_eq!(config.actions.len(), 1);
        assert!(config.actions[0].disabled);
    }

    // -----------------------------------------------------------------------
    // actions in per-repo config
    // -----------------------------------------------------------------------

    #[test]
    fn parse_per_repo_config_with_actions() {
        let kdl = r#"
actions {
    action "Run tests" {
        run "npm test"
        context "project"
    }

    action "Lint" {
        run "npm run lint"
        context "project"
        pane "float"
    }
}
"#;
        let config = parse_per_repo_config_kdl(kdl).unwrap();
        assert_eq!(config.actions.len(), 2);
        assert_eq!(config.actions[0].name, "Run tests");
        assert_eq!(config.actions[1].name, "Lint");
    }

    #[test]
    fn parse_per_repo_config_no_actions() {
        let kdl = r#"
layout {
    tab name="shell" {
        pane
    }
}
"#;
        let config = parse_per_repo_config_kdl(kdl).unwrap();
        assert!(config.actions.is_empty());
    }

    #[test]
    fn parse_per_repo_config_actions_override_builtin() {
        let kdl = r#"
actions {
    action "Open PR" {
        run "gh pr view --web"
        when "has_pr"
        context "session"
    }
}
"#;
        let config = parse_per_repo_config_kdl(kdl).unwrap();
        assert_eq!(config.actions.len(), 1);
        assert_eq!(config.actions[0].name, "Open PR");
    }

    // -----------------------------------------------------------------------
    // prompt templates in config
    // -----------------------------------------------------------------------

    #[test]
    fn parse_global_config_with_prompt_templates() {
        let kdl = r#"
config {
    issue-prompt-template "custom issue {number}"
    pr-prompt-template "custom pr {number}"
}
"#;
        let cfg = parse_global_config_kdl(kdl).unwrap();
        assert_eq!(
            cfg.issue_prompt_template.as_deref(),
            Some("custom issue {number}")
        );
        assert_eq!(
            cfg.pr_prompt_template.as_deref(),
            Some("custom pr {number}")
        );
    }

    #[test]
    fn parse_global_config_without_prompt_templates() {
        let kdl = r#"
config {
    keybindings {
        navigation "vim"
    }
}
"#;
        let cfg = parse_global_config_kdl(kdl).unwrap();
        assert!(cfg.issue_prompt_template.is_none());
        assert!(cfg.pr_prompt_template.is_none());
    }

    #[test]
    fn parse_per_repo_with_prompt_templates() {
        let kdl = r#"
issue-prompt-template "repo issue {number}: {title}"
pr-prompt-template "repo pr {number}: {title}"
"#;
        let cfg = parse_per_repo_config_kdl(kdl).unwrap();
        assert_eq!(
            cfg.issue_prompt_template.as_deref(),
            Some("repo issue {number}: {title}")
        );
        assert_eq!(
            cfg.pr_prompt_template.as_deref(),
            Some("repo pr {number}: {title}")
        );
    }

    #[test]
    fn parse_per_repo_without_prompt_templates() {
        let cfg = parse_per_repo_config_kdl("").unwrap();
        assert!(cfg.issue_prompt_template.is_none());
        assert!(cfg.pr_prompt_template.is_none());
    }

    // -----------------------------------------------------------------------
    // effective_prompt_template
    // -----------------------------------------------------------------------

    #[test]
    fn effective_issue_template_default() {
        let global = GlobalConfig::default();
        let per_repo = PerRepoConfig::default();
        assert_eq!(
            effective_issue_prompt_template(&global, &per_repo),
            crate::template::DEFAULT_ISSUE_TEMPLATE
        );
    }

    #[test]
    fn effective_pr_template_default() {
        let global = GlobalConfig::default();
        let per_repo = PerRepoConfig::default();
        assert_eq!(
            effective_pr_prompt_template(&global, &per_repo),
            crate::template::DEFAULT_PR_TEMPLATE
        );
    }

    #[test]
    fn effective_issue_template_global_overrides_default() {
        let global = GlobalConfig {
            issue_prompt_template: Some("global issue".to_string()),
            ..Default::default()
        };
        let per_repo = PerRepoConfig::default();
        assert_eq!(
            effective_issue_prompt_template(&global, &per_repo),
            "global issue"
        );
    }

    #[test]
    fn effective_issue_template_per_repo_overrides_global() {
        let global = GlobalConfig {
            issue_prompt_template: Some("global".to_string()),
            ..Default::default()
        };
        let per_repo = PerRepoConfig {
            issue_prompt_template: Some("repo".to_string()),
            ..Default::default()
        };
        assert_eq!(effective_issue_prompt_template(&global, &per_repo), "repo");
    }

    #[test]
    fn effective_pr_template_per_repo_overrides_global() {
        let global = GlobalConfig {
            pr_prompt_template: Some("global pr".to_string()),
            ..Default::default()
        };
        let per_repo = PerRepoConfig {
            pr_prompt_template: Some("repo pr".to_string()),
            ..Default::default()
        };
        assert_eq!(effective_pr_prompt_template(&global, &per_repo), "repo pr");
    }

    // --- transport field ---

    #[test]
    fn parse_projects_with_transport_mosh() {
        let kdl = r#"
project "ios-app" {
    path "/code/ios"
    host "vps.example.com"
    transport "mosh"
}
"#;
        let projects = parse_projects_kdl(kdl).unwrap();
        assert_eq!(projects[0].transport, Some(Transport::Mosh));
    }

    #[test]
    fn parse_projects_with_transport_ssh() {
        let kdl = r#"
project "api" {
    path "/code/api"
    host "vps.example.com"
    transport "ssh"
}
"#;
        let projects = parse_projects_kdl(kdl).unwrap();
        assert_eq!(projects[0].transport, Some(Transport::Ssh));
    }

    #[test]
    fn parse_projects_without_transport() {
        let kdl = r#"
project "local" {
    path "/code/local"
}
"#;
        let projects = parse_projects_kdl(kdl).unwrap();
        assert!(projects[0].transport.is_none());
    }

    #[test]
    fn parse_projects_with_invalid_transport() {
        let kdl = r#"
project "bad" {
    path "/code/bad"
    transport "pigeons"
}
"#;
        assert!(matches!(
            parse_projects_kdl(kdl).unwrap_err(),
            ZError::ConfigParse(_)
        ));
    }
}
