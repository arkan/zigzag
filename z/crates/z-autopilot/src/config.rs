use kdl::KdlDocument;
use z_core::error::{ZError, Result};
use crate::dsl::{AutopilotWorkflow, parse_autopilot_workflows, require_bool_arg};

/// Project-level autopilot configuration.
///
/// Parsed from the `autopilot { ... }` block (no name) in `.config/z.kdl`.
///
/// - `auto_push`: if true (default), autopilot pushes directly without human review.
/// - `review`: if true, autopilot pauses before push and waits for user approval.
#[derive(Debug, Clone, PartialEq)]
pub struct AutopilotConfig {
    pub auto_push: bool,
    pub review: bool,
}

impl Default for AutopilotConfig {
    fn default() -> Self {
        AutopilotConfig {
            auto_push: true,
            review: false,
        }
    }
}

/// Full autopilot configuration from `.config/z.kdl`: project-level settings +
/// any custom workflow definitions.
#[derive(Debug, Clone)]
pub struct RepoAutopilotConfig {
    /// Project-level settings (auto_push, review).
    pub config: AutopilotConfig,
    /// Custom workflow definitions in this repo.
    pub workflows: Vec<AutopilotWorkflow>,
}

impl Default for RepoAutopilotConfig {
    fn default() -> Self {
        RepoAutopilotConfig {
            config: AutopilotConfig::default(),
            workflows: Vec::new(),
        }
    }
}

/// Parse the `autopilot { ... }` config block (no name) from `.config/z.kdl`.
///
/// Named `autopilot "name" { ... }` blocks are ignored here — use
/// `parse_autopilot_workflows` (from `dsl`) to load those.
fn parse_autopilot_config_block(content: &str) -> Result<AutopilotConfig> {
    let doc: KdlDocument = content.parse().map_err(|e| {
        ZError::ConfigParse(format!("KDL parse error: {e}"))
    })?;

    let mut config = AutopilotConfig::default();

    for node in doc.nodes() {
        if node.name().value() != "autopilot" {
            continue;
        }
        // Only process the unnamed `autopilot { ... }` block (no positional args).
        let has_name_arg = node.entries().iter().any(|e| e.name().is_none());
        if has_name_arg {
            continue; // named workflow block — skip
        }

        let children = match node.children() {
            Some(c) => c,
            None => continue,
        };

        for child in children.nodes() {
            match child.name().value() {
                "auto-push" => {
                    if let Some(v) = require_bool_arg(child, "autopilot config")? {
                        config.auto_push = v;
                    }
                }
                "review" => {
                    if let Some(v) = require_bool_arg(child, "autopilot config")? {
                        config.review = v;
                    }
                }
                _ => {} // forward-compatible
            }
        }
    }

    Ok(config)
}

/// Parse both autopilot config settings and custom workflow definitions from
/// the contents of a per-repo `.config/z.kdl` file.
pub fn parse_repo_autopilot_config(content: &str) -> Result<RepoAutopilotConfig> {
    let config = parse_autopilot_config_block(content)?;
    let workflows = parse_autopilot_workflows(content)?;
    Ok(RepoAutopilotConfig { config, workflows })
}

/// Resolve the effective `AutopilotConfig` for a specific workflow by merging
/// the project-level config with any per-workflow overrides.
///
/// Per-workflow values (if set) take precedence over project-level settings.
pub fn resolve_config(project: &AutopilotConfig, workflow: &AutopilotWorkflow) -> AutopilotConfig {
    AutopilotConfig {
        auto_push: workflow.auto_push.unwrap_or(project.auto_push),
        review: workflow.review.unwrap_or(project.review),
    }
}

/// What to do before a push, based on the resolved config.
#[derive(Debug, Clone, PartialEq)]
pub enum PushDecision {
    /// Full-auto: push immediately without human intervention.
    Push,
    /// `auto_push: false` — do not push; queue the result for human review.
    QueueForReview,
    /// `review: true` — notify user and wait for explicit approval before pushing.
    WaitForApproval,
}

/// Compute the push decision from the resolved config.
///
/// Rules:
/// - `auto_push: false` → `QueueForReview` (never push automatically)
/// - `auto_push: true`, `review: true` → `WaitForApproval` (push only after user approves)
/// - `auto_push: true`, `review: false` → `Push` (full-auto, default)
pub fn push_decision(config: &AutopilotConfig) -> PushDecision {
    if !config.auto_push {
        PushDecision::QueueForReview
    } else if config.review {
        PushDecision::WaitForApproval
    } else {
        PushDecision::Push
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------------------
    // AutopilotConfig defaults
    // ---------------------------------------------------------------------------

    #[test]
    fn test_default_config_auto_push_true_review_false() {
        let cfg = AutopilotConfig::default();
        assert!(cfg.auto_push);
        assert!(!cfg.review);
    }

    // ---------------------------------------------------------------------------
    // parse_repo_autopilot_config — config block
    // ---------------------------------------------------------------------------

    #[test]
    fn test_parse_autopilot_config_defaults_when_no_block() {
        let kdl = r#"
layout {
    tab name="shell" {
        pane
    }
}
"#;
        let repo = parse_repo_autopilot_config(kdl).unwrap();
        assert!(repo.config.auto_push);
        assert!(!repo.config.review);
        assert!(repo.workflows.is_empty());
    }

    #[test]
    fn test_parse_autopilot_config_auto_push_false() {
        let kdl = r#"
autopilot {
    auto-push false
}
"#;
        let repo = parse_repo_autopilot_config(kdl).unwrap();
        assert!(!repo.config.auto_push);
        assert!(!repo.config.review);
    }

    #[test]
    fn test_parse_autopilot_config_review_true() {
        let kdl = r#"
autopilot {
    auto-push true
    review true
}
"#;
        let repo = parse_repo_autopilot_config(kdl).unwrap();
        assert!(repo.config.auto_push);
        assert!(repo.config.review);
    }

    #[test]
    fn test_parse_autopilot_config_both_false() {
        let kdl = r#"
autopilot {
    auto-push false
    review false
}
"#;
        let repo = parse_repo_autopilot_config(kdl).unwrap();
        assert!(!repo.config.auto_push);
        assert!(!repo.config.review);
    }

    // ---------------------------------------------------------------------------
    // parse_repo_autopilot_config — custom workflows
    // ---------------------------------------------------------------------------

    #[test]
    fn test_parse_repo_config_loads_custom_workflows() {
        let kdl = r#"
autopilot {
    auto-push false
    review true
}

autopilot "my-deploy" {
    trigger "manual"

    step "run-deploy" {
        run "./deploy.sh"
        on-complete "notify"
    }

    step "notify" {
        notify "Deployed ✅"
    }
}
"#;
        let repo = parse_repo_autopilot_config(kdl).unwrap();
        assert!(!repo.config.auto_push);
        assert!(repo.config.review);
        assert_eq!(repo.workflows.len(), 1);
        assert_eq!(repo.workflows[0].name, "my-deploy");
    }

    #[test]
    fn test_parse_repo_config_multiple_custom_workflows() {
        let kdl = r#"
autopilot "wf-a" {
    trigger "manual"
    step "s" {
        run "echo a"
        on-complete "done"
    }
    step "done" {
        notify "a done"
    }
}

autopilot "wf-b" {
    trigger "post-push"
    step "s" {
        run "echo b"
        on-complete "done"
    }
    step "done" {
        notify "b done"
    }
}
"#;
        let repo = parse_repo_autopilot_config(kdl).unwrap();
        // No config block — defaults apply.
        assert!(repo.config.auto_push);
        assert!(!repo.config.review);
        assert_eq!(repo.workflows.len(), 2);
        assert_eq!(repo.workflows[0].name, "wf-a");
        assert_eq!(repo.workflows[1].name, "wf-b");
    }

    #[test]
    fn test_parse_repo_config_empty_document() {
        let repo = parse_repo_autopilot_config("").unwrap();
        assert!(repo.config.auto_push);
        assert!(!repo.config.review);
        assert!(repo.workflows.is_empty());
    }

    // ---------------------------------------------------------------------------
    // Per-workflow overrides in dsl
    // ---------------------------------------------------------------------------

    #[test]
    fn test_workflow_auto_push_override_parsed() {
        let kdl = r#"
autopilot "careful-deploy" {
    trigger "manual"
    auto-push false
    review true

    step "deploy" {
        run "./deploy.sh"
        on-complete "notify"
    }

    step "notify" {
        notify "Done"
    }
}
"#;
        let repo = parse_repo_autopilot_config(kdl).unwrap();
        assert_eq!(repo.workflows.len(), 1);
        let wf = &repo.workflows[0];
        assert_eq!(wf.auto_push, Some(false));
        assert_eq!(wf.review, Some(true));
    }

    #[test]
    fn test_workflow_no_override_has_none() {
        let kdl = r#"
autopilot "simple" {
    trigger "manual"

    step "s" {
        run "cmd"
        on-complete "done"
    }

    step "done" {
        notify "Done"
    }
}
"#;
        let repo = parse_repo_autopilot_config(kdl).unwrap();
        let wf = &repo.workflows[0];
        assert_eq!(wf.auto_push, None);
        assert_eq!(wf.review, None);
    }

    // ---------------------------------------------------------------------------
    // resolve_config
    // ---------------------------------------------------------------------------

    fn make_workflow(auto_push: Option<bool>, review: Option<bool>) -> AutopilotWorkflow {
        use crate::dsl::{Trigger, Step, StepAction};
        AutopilotWorkflow {
            name: "test".into(),
            description: None,
            trigger: Trigger::Manual,
            poll_interval: None,
            steps: vec![Step {
                name: "s".into(),
                action: StepAction::Run { command: "cmd".into() },
                max_retries: None,
                timeout: None,
                on_success: None,
                on_failure: None,
                on_complete: None,
                on_max_retries: None,
                on_accept: None,
                on_reject: None,
            }],
            auto_push,
            review,
        }
    }

    #[test]
    fn test_resolve_config_no_override_uses_project() {
        let project = AutopilotConfig { auto_push: true, review: false };
        let wf = make_workflow(None, None);
        let resolved = resolve_config(&project, &wf);
        assert_eq!(resolved.auto_push, true);
        assert_eq!(resolved.review, false);
    }

    #[test]
    fn test_resolve_config_workflow_overrides_project() {
        let project = AutopilotConfig { auto_push: true, review: false };
        let wf = make_workflow(Some(false), Some(true));
        let resolved = resolve_config(&project, &wf);
        assert_eq!(resolved.auto_push, false);
        assert_eq!(resolved.review, true);
    }

    #[test]
    fn test_resolve_config_partial_override() {
        let project = AutopilotConfig { auto_push: true, review: false };
        let wf = make_workflow(None, Some(true));
        let resolved = resolve_config(&project, &wf);
        assert_eq!(resolved.auto_push, true); // from project
        assert_eq!(resolved.review, true);    // from workflow
    }

    #[test]
    fn test_resolve_config_workflow_overrides_auto_push_only() {
        let project = AutopilotConfig { auto_push: true, review: true };
        let wf = make_workflow(Some(false), None);
        let resolved = resolve_config(&project, &wf);
        assert_eq!(resolved.auto_push, false); // from workflow
        assert_eq!(resolved.review, true);     // from project
    }

    // ---------------------------------------------------------------------------
    // push_decision
    // ---------------------------------------------------------------------------

    #[test]
    fn test_push_decision_full_auto() {
        let cfg = AutopilotConfig { auto_push: true, review: false };
        assert_eq!(push_decision(&cfg), PushDecision::Push);
    }

    #[test]
    fn test_push_decision_auto_push_false_queues_for_review() {
        let cfg = AutopilotConfig { auto_push: false, review: false };
        assert_eq!(push_decision(&cfg), PushDecision::QueueForReview);
    }

    #[test]
    fn test_push_decision_review_true_waits_for_approval() {
        let cfg = AutopilotConfig { auto_push: true, review: true };
        assert_eq!(push_decision(&cfg), PushDecision::WaitForApproval);
    }

    #[test]
    fn test_push_decision_auto_push_false_takes_priority_over_review() {
        // If auto_push is false, we never push — QueueForReview even if review is also set.
        let cfg = AutopilotConfig { auto_push: false, review: true };
        assert_eq!(push_decision(&cfg), PushDecision::QueueForReview);
    }

    #[test]
    fn test_push_decision_default_config_is_full_auto() {
        let cfg = AutopilotConfig::default();
        assert_eq!(push_decision(&cfg), PushDecision::Push);
    }

    // ---------------------------------------------------------------------------
    // End-to-end: per-project config + workflow override + push_decision
    // ---------------------------------------------------------------------------

    #[test]
    fn test_e2e_project_auto_push_false_workflow_no_override() {
        let kdl = r#"
autopilot {
    auto-push false
}

autopilot "my-wf" {
    trigger "manual"
    step "s" {
        run "cmd"
        on-complete "done"
    }
    step "done" {
        notify "Done"
    }
}
"#;
        let repo = parse_repo_autopilot_config(kdl).unwrap();
        let resolved = resolve_config(&repo.config, &repo.workflows[0]);
        assert_eq!(push_decision(&resolved), PushDecision::QueueForReview);
    }

    #[test]
    fn test_e2e_project_default_workflow_review_override() {
        let kdl = r#"
autopilot "careful" {
    trigger "manual"
    review true

    step "s" {
        run "cmd"
        on-complete "done"
    }
    step "done" {
        notify "Done"
    }
}
"#;
        let repo = parse_repo_autopilot_config(kdl).unwrap();
        let resolved = resolve_config(&repo.config, &repo.workflows[0]);
        // Project default: auto_push=true, review=false
        // Workflow override: review=true
        // Resolved: auto_push=true, review=true → WaitForApproval
        assert_eq!(push_decision(&resolved), PushDecision::WaitForApproval);
    }

    #[test]
    fn test_run_step_accepts_arbitrary_shell_command() {
        // The `run` step is the escape hatch for arbitrary automation.
        // Verify any shell command string is accepted without restriction.
        let kdl = r#"
autopilot "escape-hatch" {
    trigger "manual"

    step "arbitrary" {
        run "bash -c 'echo hello && curl https://example.com | jq .'"
        on-complete "done"
    }

    step "done" {
        notify "Done"
    }
}
"#;
        let repo = parse_repo_autopilot_config(kdl).unwrap();
        assert_eq!(repo.workflows.len(), 1);
        let step = &repo.workflows[0].steps[0];
        use crate::dsl::StepAction;
        assert_eq!(
            step.action,
            StepAction::Run {
                command: "bash -c 'echo hello && curl https://example.com | jq .'".into()
            }
        );
    }

    // ---------------------------------------------------------------------------
    // Edge cases: non-boolean values for auto-push/review
    // ---------------------------------------------------------------------------

    #[test]
    fn test_parse_config_auto_push_string_false_is_error() {
        // A user writing `auto-push "false"` (string) instead of `auto-push false` (bool)
        // must be caught — silently defaulting to true would be a safety issue.
        let kdl = "autopilot {\n    auto-push \"false\"\n}\n";
        let err = parse_repo_autopilot_config(kdl).unwrap_err();
        assert!(err.to_string().contains("boolean"));
    }

    #[test]
    fn test_parse_config_review_string_true_is_error() {
        let kdl = "autopilot {\n    review \"true\"\n}\n";
        let err = parse_repo_autopilot_config(kdl).unwrap_err();
        assert!(err.to_string().contains("boolean"));
    }

    #[test]
    fn test_parse_config_auto_push_integer_is_error() {
        let kdl = "autopilot {\n    auto-push 0\n}\n";
        let err = parse_repo_autopilot_config(kdl).unwrap_err();
        assert!(err.to_string().contains("boolean"));
    }

    #[test]
    fn test_workflow_auto_push_string_is_error() {
        let kdl = r#"
autopilot "wf" {
    trigger "manual"
    auto-push "false"
    step "s" {
        run "cmd"
        on-complete "done"
    }
    step "done" {
        notify "Done"
    }
}
"#;
        let err = parse_repo_autopilot_config(kdl).unwrap_err();
        assert!(err.to_string().contains("boolean"));
    }

    // ---------------------------------------------------------------------------
    // Edge cases: multiple config blocks, empty body, unknown children
    // ---------------------------------------------------------------------------

    #[test]
    fn test_parse_config_multiple_unnamed_blocks_last_wins() {
        let kdl = r#"
autopilot {
    auto-push false
}
autopilot {
    auto-push true
    review true
}
"#;
        let repo = parse_repo_autopilot_config(kdl).unwrap();
        // Second block overwrites first
        assert!(repo.config.auto_push);
        assert!(repo.config.review);
    }

    #[test]
    fn test_parse_config_empty_autopilot_body_uses_defaults() {
        let kdl = "autopilot {\n}\n";
        let repo = parse_repo_autopilot_config(kdl).unwrap();
        assert!(repo.config.auto_push);
        assert!(!repo.config.review);
    }

    #[test]
    fn test_parse_config_unknown_children_ignored() {
        let kdl = r#"
autopilot {
    auto-push false
    future-setting "something"
}
"#;
        let repo = parse_repo_autopilot_config(kdl).unwrap();
        assert!(!repo.config.auto_push);
    }

    #[test]
    fn test_parse_config_auto_push_no_arg_keeps_default() {
        // `auto-push` with no positional arg — node exists but no value
        // This is technically valid KDL. The node is recognized but no value is extracted.
        let kdl = "autopilot {\n    auto-push\n}\n";
        let repo = parse_repo_autopilot_config(kdl).unwrap();
        assert!(repo.config.auto_push); // default preserved
    }

    // ---------------------------------------------------------------------------
    // Edge cases: resolve_config with all combinations
    // ---------------------------------------------------------------------------

    #[test]
    fn test_resolve_config_both_overrides_same_as_project() {
        let project = AutopilotConfig { auto_push: false, review: true };
        let wf = make_workflow(Some(false), Some(true));
        let resolved = resolve_config(&project, &wf);
        assert_eq!(resolved.auto_push, false);
        assert_eq!(resolved.review, true);
    }

    #[test]
    fn test_resolve_config_workflow_flips_both() {
        let project = AutopilotConfig { auto_push: false, review: true };
        let wf = make_workflow(Some(true), Some(false));
        let resolved = resolve_config(&project, &wf);
        assert_eq!(resolved.auto_push, true);
        assert_eq!(resolved.review, false);
    }
}
