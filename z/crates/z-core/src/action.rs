use kdl::{KdlDocument, KdlNode};

use crate::domain::{CiStatus, PullRequest, ReviewStatus};
use crate::error::{Result, ZError};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Condition that must be true for an action to appear in the menu.
#[derive(Debug, Clone, PartialEq)]
pub enum ActionCondition {
    Always,
    HasPr,
    HasCiFailure,
    HasNewComments,
}

/// Context in which an action is available.
#[derive(Debug, Clone, PartialEq)]
pub enum ActionContext {
    Project,
    Session,
}

/// Zellij pane type used to execute an action.
#[derive(Debug, Clone, PartialEq)]
pub enum PaneType {
    Float,
    FloatFullscreen,
    Split,
    Tab,
}

/// What an action does when executed.
#[derive(Debug, Clone, PartialEq)]
pub enum ActionType {
    Run { command: String },
    OpenUrl { url: String },
}

/// A parsed action definition (from KDL config or built-in).
#[derive(Debug, Clone, PartialEq)]
pub struct ActionDef {
    pub name: String,
    pub action: ActionType,
    pub condition: ActionCondition,
    pub context: ActionContext,
    pub pane: PaneType,
    pub icon: Option<String>,
    pub disabled: bool,
}

/// Runtime environment used to evaluate conditions and interpolate variables.
#[derive(Debug, Clone)]
pub struct ActionEnv {
    pub project: String,
    pub project_path: String,
    pub repo: Option<String>,
    pub branch: Option<String>,
    pub session: Option<String>,
    pub pr_number: Option<u64>,
    pub pr_url: Option<String>,
    pub ci_status: Option<CiStatus>,
    pub has_new_comments: bool,
    pub review_tool: String,
}

/// Preview-derived action context.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ActionPreview {
    pub pr_number: Option<u64>,
    pub pr_url: Option<String>,
    pub ci_status: Option<CiStatus>,
    pub has_new_comments: bool,
}

impl ActionPreview {
    pub fn from_forge_data(
        pr: Option<&PullRequest>,
        ci_status: Option<CiStatus>,
        review: Option<&ReviewStatus>,
    ) -> Self {
        Self {
            pr_number: pr.map(|pr| pr.number),
            pr_url: pr.map(|pr| pr.url.clone()),
            ci_status,
            has_new_comments: review.map_or(false, |review| review.has_new_comments),
        }
    }
}

impl ActionEnv {
    pub fn for_project(
        project: String,
        project_path: String,
        review_tool: String,
        preview: ActionPreview,
    ) -> Self {
        Self {
            project,
            project_path,
            repo: None,
            branch: None,
            session: None,
            pr_number: preview.pr_number,
            pr_url: preview.pr_url,
            ci_status: preview.ci_status,
            has_new_comments: preview.has_new_comments,
            review_tool,
        }
    }

    pub fn for_session(
        project: String,
        project_path: String,
        session: String,
        branch: String,
        review_tool: String,
        preview: ActionPreview,
    ) -> Self {
        Self {
            project,
            project_path,
            repo: None,
            branch: Some(branch),
            session: Some(session),
            pr_number: preview.pr_number,
            pr_url: preview.pr_url,
            ci_status: preview.ci_status,
            has_new_comments: preview.has_new_comments,
            review_tool,
        }
    }
}

/// A fully resolved action, ready for execution.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedAction {
    pub name: String,
    pub action: ActionType,
    pub pane: PaneType,
    pub icon: Option<String>,
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

fn first_string_arg(node: &KdlNode) -> Option<&str> {
    node.entries().iter().find_map(|e| {
        if e.name().is_none() {
            e.value().as_string()
        } else {
            None
        }
    })
}

/// Parse actions from a KDL string containing an `actions { ... }` block.
pub fn parse_actions_kdl(input: &str) -> Result<Vec<ActionDef>> {
    let doc: KdlDocument = input
        .parse()
        .map_err(|e| ZError::ConfigParse(format!("invalid KDL: {e}")))?;

    let mut actions = Vec::new();

    for node in doc.nodes() {
        if node.name().value() == "action" {
            actions.push(parse_action_node(node)?);
        }
    }

    Ok(actions)
}

fn parse_action_node(node: &KdlNode) -> Result<ActionDef> {
    let name = first_string_arg(node)
        .ok_or_else(|| ZError::ConfigParse("action missing name".into()))?
        .to_string();

    let children = node.children().ok_or_else(|| {
        ZError::ConfigParse(format!("action '{name}' has no body"))
    })?;

    let mut action: Option<ActionType> = None;
    let mut condition = ActionCondition::Always;
    let mut context = ActionContext::Session;
    let mut pane = PaneType::Float;
    let mut icon: Option<String> = None;
    let mut disabled = false;

    for child in children.nodes() {
        match child.name().value() {
            "run" => {
                let cmd = first_string_arg(child).ok_or_else(|| {
                    ZError::ConfigParse(format!("action '{name}': run missing command"))
                })?;
                action = Some(ActionType::Run { command: cmd.to_string() });
            }
            "open-url" => {
                let url = first_string_arg(child).ok_or_else(|| {
                    ZError::ConfigParse(format!("action '{name}': open-url missing URL"))
                })?;
                action = Some(ActionType::OpenUrl { url: url.to_string() });
            }
            "when" => {
                let raw = first_string_arg(child).ok_or_else(|| {
                    ZError::ConfigParse(format!("action '{name}': when missing value"))
                })?;
                condition = ActionCondition::from_str(raw).ok_or_else(|| {
                    ZError::ConfigParse(format!("action '{name}': unknown condition '{raw}'"))
                })?;
            }
            "context" => {
                let raw = first_string_arg(child).ok_or_else(|| {
                    ZError::ConfigParse(format!("action '{name}': context missing value"))
                })?;
                context = ActionContext::from_str(raw).ok_or_else(|| {
                    ZError::ConfigParse(format!("action '{name}': unknown context '{raw}'"))
                })?;
            }
            "pane" => {
                let raw = first_string_arg(child).ok_or_else(|| {
                    ZError::ConfigParse(format!("action '{name}': pane missing value"))
                })?;
                pane = PaneType::from_str(raw).ok_or_else(|| {
                    ZError::ConfigParse(format!("action '{name}': unknown pane type '{raw}'"))
                })?;
            }
            "icon" => {
                icon = first_string_arg(child).map(str::to_string);
            }
            "disabled" => {
                disabled = child
                    .entries()
                    .iter()
                    .find(|e| e.name().is_none())
                    .and_then(|e| e.value().as_bool())
                    .unwrap_or(true);
            }
            _ => {} // forward-compatible
        }
    }

    let action = action.ok_or_else(|| {
        ZError::ConfigParse(format!("action '{name}' has no action (run/open-url)"))
    })?;

    Ok(ActionDef {
        name,
        action,
        condition,
        context,
        pane,
        icon,
        disabled,
    })
}

// ---------------------------------------------------------------------------
// Enum conversions
// ---------------------------------------------------------------------------

impl ActionCondition {
    pub fn from_str(s: &str) -> Option<ActionCondition> {
        match s {
            "always" => Some(ActionCondition::Always),
            "has_pr" => Some(ActionCondition::HasPr),
            "has_ci_failure" => Some(ActionCondition::HasCiFailure),
            "has_new_comments" => Some(ActionCondition::HasNewComments),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ActionCondition::Always => "always",
            ActionCondition::HasPr => "has_pr",
            ActionCondition::HasCiFailure => "has_ci_failure",
            ActionCondition::HasNewComments => "has_new_comments",
        }
    }
}

impl ActionContext {
    pub fn from_str(s: &str) -> Option<ActionContext> {
        match s {
            "project" => Some(ActionContext::Project),
            "session" => Some(ActionContext::Session),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ActionContext::Project => "project",
            ActionContext::Session => "session",
        }
    }
}

impl PaneType {
    pub fn from_str(s: &str) -> Option<PaneType> {
        match s {
            "float" => Some(PaneType::Float),
            "float-fullscreen" => Some(PaneType::FloatFullscreen),
            "split" => Some(PaneType::Split),
            "tab" => Some(PaneType::Tab),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            PaneType::Float => "float",
            PaneType::FloatFullscreen => "float-fullscreen",
            PaneType::Split => "split",
            PaneType::Tab => "tab",
        }
    }
}

// ---------------------------------------------------------------------------
// Built-in actions
// ---------------------------------------------------------------------------

/// Returns the default built-in actions.
pub fn builtin_actions() -> Vec<ActionDef> {
    vec![
        ActionDef {
            name: "Open PR".into(),
            action: ActionType::OpenUrl { url: "${pr_url}".into() },
            condition: ActionCondition::HasPr,
            context: ActionContext::Session,
            pane: PaneType::Float,
            icon: Some("\u{1f310}".into()), // 🌐
            disabled: false,
        },
        ActionDef {
            name: "Review code".into(),
            action: ActionType::Run {
                command: "${review_tool} review --base main".into(),
            },
            condition: ActionCondition::Always,
            context: ActionContext::Session,
            pane: PaneType::Tab,
            icon: Some("\u{1f50d}".into()), // 🔍
            disabled: false,
        },
        ActionDef {
            name: "Fix CI".into(),
            action: ActionType::Run {
                command: "claude 'Fix the CI failure based on: $(gh run view --log-failed)'".into(),
            },
            condition: ActionCondition::HasCiFailure,
            context: ActionContext::Session,
            pane: PaneType::Tab,
            icon: Some("\u{1f527}".into()), // 🔧
            disabled: false,
        },
        ActionDef {
            name: "Address review comments".into(),
            action: ActionType::Run {
                command: "claude 'Address all PR review comments: $(gh pr view --json reviews -q .reviews)'".into(),
            },
            condition: ActionCondition::HasNewComments,
            context: ActionContext::Session,
            pane: PaneType::Tab,
            icon: Some("\u{1f4ac}".into()), // 💬
            disabled: false,
        },
        ActionDef {
            name: "Lazygit".into(),
            action: ActionType::Run {
                command: "lazygit".into(),
            },
            condition: ActionCondition::Always,
            context: ActionContext::Session,
            pane: PaneType::FloatFullscreen,
            icon: Some("\u{1f500}".into()), // 🔀
            disabled: false,
        },
    ]
}

// ---------------------------------------------------------------------------
// Merge
// ---------------------------------------------------------------------------

/// Merge multiple layers of action definitions. Later layers override earlier
/// ones by name. Actions with `disabled: true` are removed.
pub fn merge_actions(layers: &[Vec<ActionDef>]) -> Vec<ActionDef> {
    let mut merged: Vec<ActionDef> = Vec::new();

    for layer in layers {
        for action in layer {
            if let Some(pos) = merged.iter().position(|a| a.name == action.name) {
                merged[pos] = action.clone();
            } else {
                merged.push(action.clone());
            }
        }
    }

    merged.retain(|a| !a.disabled);
    merged
}

// ---------------------------------------------------------------------------
// Resolve
// ---------------------------------------------------------------------------

/// Evaluate conditions and interpolate variables, returning only the actions
/// that are applicable to the given environment.
pub fn resolve_actions(actions: &[ActionDef], env: &ActionEnv) -> Result<Vec<ResolvedAction>> {
    let mut resolved = Vec::new();

    for action in actions {
        // Context filter: session actions require a branch
        if action.context == ActionContext::Session && env.branch.is_none() {
            continue;
        }

        // Condition filter
        if !eval_condition(&action.condition, env) {
            continue;
        }

        let resolved_action = match &action.action {
            ActionType::Run { command } => {
                ActionType::Run { command: interpolate(command, env)? }
            }
            ActionType::OpenUrl { url } => {
                ActionType::OpenUrl { url: interpolate(url, env)? }
            }
        };

        resolved.push(ResolvedAction {
            name: action.name.clone(),
            action: resolved_action,
            pane: action.pane.clone(),
            icon: action.icon.clone(),
        });
    }

    Ok(resolved)
}

fn eval_condition(cond: &ActionCondition, env: &ActionEnv) -> bool {
    match cond {
        ActionCondition::Always => true,
        ActionCondition::HasPr => env.pr_number.is_some(),
        ActionCondition::HasCiFailure => env.ci_status == Some(CiStatus::Failing),
        ActionCondition::HasNewComments => env.has_new_comments,
    }
}

fn interpolate(template: &str, env: &ActionEnv) -> Result<String> {
    let mut result = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '$' {
            if chars.peek() == Some(&'{') {
                chars.next(); // consume '{'
                let mut var_name = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(c) => var_name.push(c),
                        None => {
                            return Err(ZError::ConfigParse(
                                "unterminated ${...} variable".into(),
                            ));
                        }
                    }
                }
                let value = resolve_var(&var_name, env)?;
                result.push_str(&value);
            } else if chars.peek() == Some(&'(') {
                // $(...) subshell — leave untouched for runtime execution
                result.push('$');
            } else {
                result.push('$');
            }
        } else {
            result.push(ch);
        }
    }

    Ok(result)
}

fn resolve_var(name: &str, env: &ActionEnv) -> Result<String> {
    match name {
        "project" => Ok(env.project.clone()),
        "project_path" => Ok(env.project_path.clone()),
        "repo" => env
            .repo
            .clone()
            .ok_or_else(|| ZError::ConfigParse("${repo} is not available (no git remote)".into())),
        "branch" => env
            .branch
            .clone()
            .ok_or_else(|| ZError::ConfigParse("${branch} is not available (no session selected)".into())),
        "session" => env
            .session
            .clone()
            .ok_or_else(|| ZError::ConfigParse("${session} is not available (no session selected)".into())),
        "pr_number" => env
            .pr_number
            .map(|n| n.to_string())
            .ok_or_else(|| ZError::ConfigParse("${pr_number} is not available (no PR found)".into())),
        "pr_url" => env
            .pr_url
            .clone()
            .ok_or_else(|| ZError::ConfigParse("${pr_url} is not available (no PR found)".into())),
        "ci_status" => Ok(match &env.ci_status {
            Some(CiStatus::Passing) => "passing".into(),
            Some(CiStatus::Failing) => "failing".into(),
            Some(CiStatus::Pending) => "pending".into(),
            Some(CiStatus::Unknown) | None => "unknown".into(),
        }),
        "review_tool" => Ok(env.review_tool.clone()),
        _ => Err(ZError::ConfigParse(format!("unknown variable '${{{name}}}'"))),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // parse_actions_kdl — tracer bullet
    // -----------------------------------------------------------------------

    const FULL_ACTION_KDL: &str = r#"
action "Review PR (Codex)" {
    run "codex -q 'Review PR #${pr_number}'"
    when "has_pr"
    context "session"
    pane "tab"
    icon "🔍"
}
"#;

    #[test]
    fn parse_action_full() {
        let actions = parse_actions_kdl(FULL_ACTION_KDL).unwrap();
        assert_eq!(actions.len(), 1);
        let a = &actions[0];
        assert_eq!(a.name, "Review PR (Codex)");
        assert_eq!(
            a.action,
            ActionType::Run { command: "codex -q 'Review PR #${pr_number}'".into() }
        );
        assert_eq!(a.condition, ActionCondition::HasPr);
        assert_eq!(a.context, ActionContext::Session);
        assert_eq!(a.pane, PaneType::Tab);
        assert_eq!(a.icon.as_deref(), Some("🔍"));
        assert!(!a.disabled);
    }

    #[test]
    fn parse_multiple_actions() {
        let kdl = r#"
action "A" {
    run "cmd-a"
}
action "B" {
    open-url "https://example.com"
    when "has_pr"
}
"#;
        let actions = parse_actions_kdl(kdl).unwrap();
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].name, "A");
        assert_eq!(actions[1].name, "B");
    }

    #[test]
    fn parse_open_url_action() {
        let kdl = r#"
action "Open PR" {
    open-url "${pr_url}"
    when "has_pr"
    context "session"
    icon "🌐"
}
"#;
        let actions = parse_actions_kdl(kdl).unwrap();
        let a = &actions[0];
        assert_eq!(a.action, ActionType::OpenUrl { url: "${pr_url}".into() });
        assert_eq!(a.condition, ActionCondition::HasPr);
    }

    // -----------------------------------------------------------------------
    // Enum from_str / as_str roundtrips
    // -----------------------------------------------------------------------

    #[test]
    fn condition_roundtrip() {
        let conditions = [
            ActionCondition::Always,
            ActionCondition::HasPr,
            ActionCondition::HasCiFailure,
            ActionCondition::HasNewComments,
        ];
        for c in &conditions {
            assert_eq!(ActionCondition::from_str(c.as_str()).as_ref(), Some(c));
        }
    }

    #[test]
    fn condition_unknown_returns_none() {
        assert_eq!(ActionCondition::from_str("nonexistent"), None);
    }

    #[test]
    fn context_roundtrip() {
        let contexts = [ActionContext::Project, ActionContext::Session];
        for c in &contexts {
            assert_eq!(ActionContext::from_str(c.as_str()).as_ref(), Some(c));
        }
    }

    #[test]
    fn context_unknown_returns_none() {
        assert_eq!(ActionContext::from_str("unknown"), None);
    }

    #[test]
    fn pane_type_roundtrip() {
        let types = [PaneType::Float, PaneType::FloatFullscreen, PaneType::Split, PaneType::Tab];
        for t in &types {
            assert_eq!(PaneType::from_str(t.as_str()).as_ref(), Some(t));
        }
    }

    #[test]
    fn pane_type_unknown_returns_none() {
        assert_eq!(PaneType::from_str("popup"), None);
    }

    // -----------------------------------------------------------------------
    // Parse defaults — missing optional fields
    // -----------------------------------------------------------------------

    #[test]
    fn parse_action_defaults() {
        let kdl = r#"
action "Run tests" {
    run "cargo test"
}
"#;
        let actions = parse_actions_kdl(kdl).unwrap();
        let a = &actions[0];
        assert_eq!(a.condition, ActionCondition::Always);
        assert_eq!(a.context, ActionContext::Session);
        assert_eq!(a.pane, PaneType::Float);
        assert!(a.icon.is_none());
        assert!(!a.disabled);
    }

    #[test]
    fn parse_action_disabled() {
        let kdl = r#"
action "Open PR" {
    run "echo disabled"
    disabled true
}
"#;
        let actions = parse_actions_kdl(kdl).unwrap();
        assert!(actions[0].disabled);
    }

    #[test]
    fn parse_action_context_project() {
        let kdl = r#"
action "Build" {
    run "make build"
    context "project"
}
"#;
        let actions = parse_actions_kdl(kdl).unwrap();
        assert_eq!(actions[0].context, ActionContext::Project);
    }

    // -----------------------------------------------------------------------
    // Parse errors
    // -----------------------------------------------------------------------

    #[test]
    fn parse_action_missing_name_is_error() {
        let kdl = r#"
action {
    run "cmd"
}
"#;
        assert!(parse_actions_kdl(kdl).is_err());
    }

    #[test]
    fn parse_action_no_body_is_error() {
        let kdl = r#"action "Empty""#;
        assert!(parse_actions_kdl(kdl).is_err());
    }

    #[test]
    fn parse_action_no_action_is_error() {
        let kdl = r#"
action "NoAction" {
    when "always"
    pane "float"
}
"#;
        let err = parse_actions_kdl(kdl).unwrap_err();
        assert!(err.to_string().contains("no action"));
    }

    #[test]
    fn parse_action_unknown_when_is_error() {
        let kdl = r#"
action "Bad" {
    run "cmd"
    when "is_friday"
}
"#;
        let err = parse_actions_kdl(kdl).unwrap_err();
        assert!(err.to_string().contains("unknown condition"));
    }

    #[test]
    fn parse_action_unknown_context_is_error() {
        let kdl = r#"
action "Bad" {
    run "cmd"
    context "workspace"
}
"#;
        let err = parse_actions_kdl(kdl).unwrap_err();
        assert!(err.to_string().contains("unknown context"));
    }

    #[test]
    fn parse_action_unknown_pane_is_error() {
        let kdl = r#"
action "Bad" {
    run "cmd"
    pane "popup"
}
"#;
        let err = parse_actions_kdl(kdl).unwrap_err();
        assert!(err.to_string().contains("unknown pane type"));
    }

    #[test]
    fn parse_invalid_kdl_is_error() {
        assert!(parse_actions_kdl("{{{{not valid kdl").is_err());
    }

    // -----------------------------------------------------------------------
    // Built-in actions
    // -----------------------------------------------------------------------

    #[test]
    fn builtin_actions_count() {
        let builtins = builtin_actions();
        assert_eq!(builtins.len(), 5);
    }

    #[test]
    fn builtin_actions_names() {
        let builtins = builtin_actions();
        let names: Vec<&str> = builtins.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"Open PR"));
        assert!(names.contains(&"Review code"));
        assert!(names.contains(&"Fix CI"));
        assert!(names.contains(&"Address review comments"));
        assert!(names.contains(&"Lazygit"));
    }

    #[test]
    fn builtin_open_pr_is_open_url() {
        let builtins = builtin_actions();
        let open_pr = builtins.iter().find(|a| a.name == "Open PR").unwrap();
        assert!(matches!(open_pr.action, ActionType::OpenUrl { .. }));
        assert_eq!(open_pr.condition, ActionCondition::HasPr);
    }

    #[test]
    fn builtin_review_code_uses_review_tool_var() {
        let builtins = builtin_actions();
        let review = builtins.iter().find(|a| a.name == "Review code").unwrap();
        assert_eq!(review.condition, ActionCondition::Always);
        if let ActionType::Run { command } = &review.action {
            assert!(command.contains("${review_tool}"));
            assert!(command.contains("review"));
        } else {
            panic!("expected Run action");
        }
    }

    #[test]
    fn builtin_fix_ci_condition() {
        let builtins = builtin_actions();
        let fix_ci = builtins.iter().find(|a| a.name == "Fix CI").unwrap();
        assert_eq!(fix_ci.condition, ActionCondition::HasCiFailure);
        assert_eq!(fix_ci.pane, PaneType::Tab);
    }

    #[test]
    fn builtin_address_comments_condition() {
        let builtins = builtin_actions();
        let addr = builtins.iter().find(|a| a.name == "Address review comments").unwrap();
        assert_eq!(addr.condition, ActionCondition::HasNewComments);
    }

    // -----------------------------------------------------------------------
    // Merge
    // -----------------------------------------------------------------------

    #[test]
    fn merge_override_by_name() {
        let builtin = vec![ActionDef {
            name: "Open PR".into(),
            action: ActionType::Run { command: "original".into() },
            condition: ActionCondition::HasPr,
            context: ActionContext::Session,
            pane: PaneType::Float,
            icon: None,
            disabled: false,
        }];
        let user = vec![ActionDef {
            name: "Open PR".into(),
            action: ActionType::Run { command: "override".into() },
            condition: ActionCondition::HasPr,
            context: ActionContext::Session,
            pane: PaneType::Tab,
            icon: None,
            disabled: false,
        }];
        let merged = merge_actions(&[builtin, user]);
        assert_eq!(merged.len(), 1);
        assert_eq!(
            merged[0].action,
            ActionType::Run { command: "override".into() }
        );
        assert_eq!(merged[0].pane, PaneType::Tab);
    }

    #[test]
    fn merge_disabled_removes_action() {
        let builtin = vec![ActionDef {
            name: "Open PR".into(),
            action: ActionType::Run { command: "cmd".into() },
            condition: ActionCondition::Always,
            context: ActionContext::Session,
            pane: PaneType::Float,
            icon: None,
            disabled: false,
        }];
        let user = vec![ActionDef {
            name: "Open PR".into(),
            action: ActionType::Run { command: "cmd".into() },
            condition: ActionCondition::Always,
            context: ActionContext::Session,
            pane: PaneType::Float,
            icon: None,
            disabled: true,
        }];
        let merged = merge_actions(&[builtin, user]);
        assert!(merged.is_empty());
    }

    #[test]
    fn merge_preserves_order_and_appends_new() {
        let builtin = vec![
            ActionDef {
                name: "A".into(),
                action: ActionType::Run { command: "a".into() },
                condition: ActionCondition::Always,
                context: ActionContext::Session,
                pane: PaneType::Float,
                icon: None,
                disabled: false,
            },
            ActionDef {
                name: "B".into(),
                action: ActionType::Run { command: "b".into() },
                condition: ActionCondition::Always,
                context: ActionContext::Session,
                pane: PaneType::Float,
                icon: None,
                disabled: false,
            },
        ];
        let user = vec![ActionDef {
            name: "C".into(),
            action: ActionType::Run { command: "c".into() },
            condition: ActionCondition::Always,
            context: ActionContext::Session,
            pane: PaneType::Float,
            icon: None,
            disabled: false,
        }];
        let merged = merge_actions(&[builtin, user]);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].name, "A");
        assert_eq!(merged[1].name, "B");
        assert_eq!(merged[2].name, "C");
    }

    #[test]
    fn merge_three_layers() {
        let builtin = vec![ActionDef {
            name: "X".into(),
            action: ActionType::Run { command: "v1".into() },
            condition: ActionCondition::Always,
            context: ActionContext::Session,
            pane: PaneType::Float,
            icon: None,
            disabled: false,
        }];
        let global = vec![ActionDef {
            name: "X".into(),
            action: ActionType::Run { command: "v2".into() },
            condition: ActionCondition::Always,
            context: ActionContext::Session,
            pane: PaneType::Float,
            icon: None,
            disabled: false,
        }];
        let repo = vec![ActionDef {
            name: "X".into(),
            action: ActionType::Run { command: "v3".into() },
            condition: ActionCondition::Always,
            context: ActionContext::Session,
            pane: PaneType::Float,
            icon: None,
            disabled: false,
        }];
        let merged = merge_actions(&[builtin, global, repo]);
        assert_eq!(merged.len(), 1);
        assert_eq!(
            merged[0].action,
            ActionType::Run { command: "v3".into() }
        );
    }

    // -----------------------------------------------------------------------
    // Condition evaluation + context filtering
    // -----------------------------------------------------------------------

    fn make_env() -> ActionEnv {
        ActionEnv {
            project: "myapp".into(),
            project_path: "/home/user/myapp".into(),
            repo: Some("user/myapp".into()),
            branch: Some("feat/login".into()),
            session: Some("myapp:feat-login".into()),
            pr_number: Some(42),
            pr_url: Some("https://github.com/user/myapp/pull/42".into()),
            ci_status: Some(CiStatus::Passing),
            has_new_comments: false,
            review_tool: "codex".into(),
        }
    }

    #[test]
    fn action_env_for_project_has_no_session_fields() {
        let env = ActionEnv::for_project(
            "myapp".to_string(),
            "/repo/myapp".to_string(),
            "codex".to_string(),
            ActionPreview::default(),
        );

        assert_eq!(env.branch, None);
        assert_eq!(env.session, None);
    }

    #[test]
    fn action_preview_from_forge_data_maps_pr_ci_and_review() {
        let pr = PullRequest {
            number: 42,
            title: "Fix CI".to_string(),
            url: "https://example.com/pr/42".to_string(),
            state: crate::domain::PrState::Open,
        };
        let review = ReviewStatus {
            has_new_comments: true,
            comment_count: 2,
            last_review_at: Some("2026-05-10T09:00:00Z".to_string()),
        };

        let preview = ActionPreview::from_forge_data(
            Some(&pr),
            Some(CiStatus::Failing),
            Some(&review),
        );

        assert_eq!(preview.pr_number, Some(42));
        assert_eq!(preview.pr_url.as_deref(), Some("https://example.com/pr/42"));
        assert_eq!(preview.ci_status, Some(CiStatus::Failing));
        assert!(preview.has_new_comments);
    }

    #[test]
    fn action_env_for_session_sets_branch_and_session_together() {
        let env = ActionEnv::for_session(
            "myapp".to_string(),
            "/repo/myapp".to_string(),
            "myapp:main".to_string(),
            "main".to_string(),
            "codex".to_string(),
            ActionPreview {
                pr_number: Some(42),
                pr_url: Some("https://example.com/pr/42".to_string()),
                ci_status: Some(CiStatus::Failing),
                has_new_comments: true,
            },
        );

        assert_eq!(env.branch.as_deref(), Some("main"));
        assert_eq!(env.session.as_deref(), Some("myapp:main"));
        assert_eq!(env.pr_number, Some(42));
        assert_eq!(env.ci_status, Some(CiStatus::Failing));
        assert!(env.has_new_comments);
    }

    fn make_action(name: &str, condition: ActionCondition, context: ActionContext) -> ActionDef {
        ActionDef {
            name: name.into(),
            action: ActionType::Run { command: "echo test".into() },
            condition,
            context,
            pane: PaneType::Float,
            icon: None,
            disabled: false,
        }
    }

    #[test]
    fn resolve_filters_by_has_pr() {
        let env = make_env();
        let actions = vec![
            make_action("A", ActionCondition::HasPr, ActionContext::Session),
        ];
        let resolved = resolve_actions(&actions, &env).unwrap();
        assert_eq!(resolved.len(), 1);

        // No PR
        let mut env_no_pr = make_env();
        env_no_pr.pr_number = None;
        let resolved = resolve_actions(&actions, &env_no_pr).unwrap();
        assert!(resolved.is_empty());
    }

    #[test]
    fn resolve_filters_by_has_ci_failure() {
        let mut env = make_env();
        env.ci_status = Some(CiStatus::Failing);
        let actions = vec![
            make_action("Fix", ActionCondition::HasCiFailure, ActionContext::Session),
        ];
        let resolved = resolve_actions(&actions, &env).unwrap();
        assert_eq!(resolved.len(), 1);

        env.ci_status = Some(CiStatus::Passing);
        let resolved = resolve_actions(&actions, &env).unwrap();
        assert!(resolved.is_empty());
    }

    #[test]
    fn resolve_filters_by_has_new_comments() {
        let mut env = make_env();
        env.has_new_comments = true;
        let actions = vec![
            make_action("Review", ActionCondition::HasNewComments, ActionContext::Session),
        ];
        let resolved = resolve_actions(&actions, &env).unwrap();
        assert_eq!(resolved.len(), 1);

        env.has_new_comments = false;
        let resolved = resolve_actions(&actions, &env).unwrap();
        assert!(resolved.is_empty());
    }

    #[test]
    fn resolve_always_condition_always_passes() {
        let env = make_env();
        let actions = vec![
            make_action("Always", ActionCondition::Always, ActionContext::Session),
        ];
        let resolved = resolve_actions(&actions, &env).unwrap();
        assert_eq!(resolved.len(), 1);
    }

    #[test]
    fn resolve_session_action_filtered_without_branch() {
        let mut env = make_env();
        env.branch = None;
        env.session = None;
        let actions = vec![
            make_action("SessionOnly", ActionCondition::Always, ActionContext::Session),
        ];
        let resolved = resolve_actions(&actions, &env).unwrap();
        assert!(resolved.is_empty());
    }

    #[test]
    fn resolve_project_action_available_without_branch() {
        let mut env = make_env();
        env.branch = None;
        env.session = None;
        let actions = vec![
            make_action("ProjectWide", ActionCondition::Always, ActionContext::Project),
        ];
        let resolved = resolve_actions(&actions, &env).unwrap();
        assert_eq!(resolved.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Variable interpolation
    // -----------------------------------------------------------------------

    #[test]
    fn interpolate_all_variables() {
        let env = make_env();
        let actions = vec![ActionDef {
            name: "test".into(),
            action: ActionType::Run {
                command: "${project} ${project_path} ${repo} ${branch} ${session} ${pr_number} ${pr_url} ${ci_status} ${review_tool}".into(),
            },
            condition: ActionCondition::Always,
            context: ActionContext::Session,
            pane: PaneType::Float,
            icon: None,
            disabled: false,
        }];
        let resolved = resolve_actions(&actions, &env).unwrap();
        if let ActionType::Run { command } = &resolved[0].action {
            assert_eq!(
                command,
                "myapp /home/user/myapp user/myapp feat/login myapp:feat-login 42 https://github.com/user/myapp/pull/42 passing codex"
            );
        } else {
            panic!("expected Run");
        }
    }

    #[test]
    fn interpolate_missing_variable_is_error() {
        let mut env = make_env();
        env.pr_number = None;
        let actions = vec![ActionDef {
            name: "test".into(),
            action: ActionType::Run { command: "echo ${pr_number}".into() },
            condition: ActionCondition::Always,
            context: ActionContext::Session,
            pane: PaneType::Float,
            icon: None,
            disabled: false,
        }];
        let err = resolve_actions(&actions, &env).unwrap_err();
        assert!(err.to_string().contains("${pr_number}"));
        assert!(err.to_string().contains("not available"));
    }

    #[test]
    fn interpolate_unknown_variable_is_error() {
        let env = make_env();
        let actions = vec![ActionDef {
            name: "test".into(),
            action: ActionType::Run { command: "echo ${nonexistent}".into() },
            condition: ActionCondition::Always,
            context: ActionContext::Session,
            pane: PaneType::Float,
            icon: None,
            disabled: false,
        }];
        let err = resolve_actions(&actions, &env).unwrap_err();
        assert!(err.to_string().contains("unknown variable"));
    }

    #[test]
    fn interpolate_subshell_left_untouched() {
        let env = make_env();
        let actions = vec![ActionDef {
            name: "test".into(),
            action: ActionType::Run {
                command: "claude 'Fix: $(gh run view --log-failed)'".into(),
            },
            condition: ActionCondition::Always,
            context: ActionContext::Session,
            pane: PaneType::Float,
            icon: None,
            disabled: false,
        }];
        let resolved = resolve_actions(&actions, &env).unwrap();
        if let ActionType::Run { command } = &resolved[0].action {
            assert!(command.contains("$(gh run view --log-failed)"));
        } else {
            panic!("expected Run");
        }
    }

    #[test]
    fn interpolate_unterminated_var_is_error() {
        let env = make_env();
        let result = interpolate("echo ${unclosed", &env);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unterminated"));
    }

    #[test]
    fn interpolate_dollar_without_brace_is_literal() {
        let env = make_env();
        let result = interpolate("price is $5", &env).unwrap();
        assert_eq!(result, "price is $5");
    }

    #[test]
    fn interpolate_open_url_variables() {
        let env = make_env();
        let actions = vec![ActionDef {
            name: "test".into(),
            action: ActionType::OpenUrl { url: "${pr_url}".into() },
            condition: ActionCondition::Always,
            context: ActionContext::Session,
            pane: PaneType::Float,
            icon: None,
            disabled: false,
        }];
        let resolved = resolve_actions(&actions, &env).unwrap();
        if let ActionType::OpenUrl { url } = &resolved[0].action {
            assert_eq!(url, "https://github.com/user/myapp/pull/42");
        } else {
            panic!("expected OpenUrl");
        }
    }

    // -----------------------------------------------------------------------
    // ReviewStatus (domain type used by conditions)
    // -----------------------------------------------------------------------

    #[test]
    fn ci_status_failing_triggers_has_ci_failure() {
        let mut env = make_env();
        env.ci_status = Some(CiStatus::Failing);
        assert!(eval_condition(&ActionCondition::HasCiFailure, &env));

        env.ci_status = Some(CiStatus::Pending);
        assert!(!eval_condition(&ActionCondition::HasCiFailure, &env));

        env.ci_status = None;
        assert!(!eval_condition(&ActionCondition::HasCiFailure, &env));
    }

    #[test]
    fn ci_status_interpolates_all_variants() {
        let mut env = make_env();
        for (status, expected) in [
            (Some(CiStatus::Passing), "passing"),
            (Some(CiStatus::Failing), "failing"),
            (Some(CiStatus::Pending), "pending"),
            (Some(CiStatus::Unknown), "unknown"),
            (None, "unknown"),
        ] {
            env.ci_status = status;
            let result = resolve_var("ci_status", &env).unwrap();
            assert_eq!(result, expected);
        }
    }
}
