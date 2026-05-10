use kdl::{KdlDocument, KdlNode};
use z_core::error::{ZError, Result};

/// Trigger events that can start a workflow.
#[derive(Debug, Clone, PartialEq)]
pub enum Trigger {
    PostPush,
    PrApproved,
    PrReviewReceived,
    PrOpenedByDependabot,
    PostMergeMain,
    NewCommitsOnMain,
    Manual,
}

impl Trigger {
    pub fn from_str(s: &str) -> Option<Trigger> {
        match s {
            "post-push" => Some(Trigger::PostPush),
            "pr-approved" => Some(Trigger::PrApproved),
            "pr-review-received" => Some(Trigger::PrReviewReceived),
            "pr-opened-by-dependabot" => Some(Trigger::PrOpenedByDependabot),
            "post-merge-main" => Some(Trigger::PostMergeMain),
            "new-commits-on-main" => Some(Trigger::NewCommitsOnMain),
            "manual" => Some(Trigger::Manual),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Trigger::PostPush => "post-push",
            Trigger::PrApproved => "pr-approved",
            Trigger::PrReviewReceived => "pr-review-received",
            Trigger::PrOpenedByDependabot => "pr-opened-by-dependabot",
            Trigger::PostMergeMain => "post-merge-main",
            Trigger::NewCommitsOnMain => "new-commits-on-main",
            Trigger::Manual => "manual",
        }
    }
}

/// The action a step performs.
#[derive(Debug, Clone, PartialEq)]
pub enum StepAction {
    Run { command: String },
    Notify { message: String },
    Confirm { prompt: String },
}

/// A single step in a workflow.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub name: String,
    pub action: StepAction,
    pub max_retries: Option<u32>,
    pub timeout: Option<String>,
    /// Transition on successful exit (exit code 0).
    pub on_success: Option<String>,
    /// Transition on failed exit (non-zero exit code).
    pub on_failure: Option<String>,
    /// Transition regardless of exit code (overrides on_success/on_failure when set).
    pub on_complete: Option<String>,
    /// Transition when max_retries is exhausted.
    pub on_max_retries: Option<String>,
    /// For confirm steps: transition when accepted.
    pub on_accept: Option<String>,
    /// For confirm steps: transition when rejected.
    pub on_reject: Option<String>,
}

/// A complete autopilot workflow definition.
#[derive(Debug, Clone, PartialEq)]
pub struct AutopilotWorkflow {
    pub name: String,
    pub description: Option<String>,
    pub trigger: Trigger,
    pub poll_interval: Option<String>,
    pub steps: Vec<Step>,
    /// Per-workflow override: if set, overrides the project-level auto_push setting.
    pub auto_push: Option<bool>,
    /// Per-workflow override: if set, overrides the project-level review setting.
    pub review: Option<bool>,
}

/// Extract a boolean positional arg from a KDL node, returning an error if
/// a positional arg exists but is not a boolean (e.g. `auto-push "false"`).
pub(crate) fn require_bool_arg(node: &KdlNode, context: &str) -> Result<Option<bool>> {
    let entry = node.entries().iter().find(|e| e.name().is_none());
    match entry {
        None => Ok(None),
        Some(e) => match e.value().as_bool() {
            Some(b) => Ok(Some(b)),
            None => Err(ZError::ConfigParse(format!(
                "{context}: '{}' expects a boolean (true/false), got {}", node.name().value(), e.value()
            ))),
        },
    }
}

fn first_string_arg(node: &KdlNode) -> Option<&str> {
    node.entries().iter().find_map(|e| {
        if e.name().is_none() {
            e.value().as_string()
        } else {
            None
        }
    })
}

fn parse_step(node: &KdlNode) -> Result<Step> {
    let name = first_string_arg(node)
        .ok_or_else(|| ZError::ConfigParse("step missing name".into()))?
        .to_string();

    let children = node.children().ok_or_else(|| {
        ZError::ConfigParse(format!("step '{name}' has no body"))
    })?;

    let mut action: Option<StepAction> = None;
    let mut max_retries: Option<u32> = None;
    let mut timeout: Option<String> = None;
    let mut on_success: Option<String> = None;
    let mut on_failure: Option<String> = None;
    let mut on_complete: Option<String> = None;
    let mut on_max_retries: Option<String> = None;
    let mut on_accept: Option<String> = None;
    let mut on_reject: Option<String> = None;

    for child in children.nodes() {
        match child.name().value() {
            "run" => {
                let cmd = first_string_arg(child).ok_or_else(|| {
                    ZError::ConfigParse(format!("step '{name}': run missing command"))
                })?;
                action = Some(StepAction::Run { command: cmd.to_string() });
            }
            "notify" => {
                let msg = first_string_arg(child).ok_or_else(|| {
                    ZError::ConfigParse(format!("step '{name}': notify missing message"))
                })?;
                action = Some(StepAction::Notify { message: msg.to_string() });
            }
            "confirm" => {
                let prompt = first_string_arg(child).ok_or_else(|| {
                    ZError::ConfigParse(format!("step '{name}': confirm missing prompt"))
                })?;
                action = Some(StepAction::Confirm { prompt: prompt.to_string() });
            }
            "max-retries" => {
                let v = child.entries().iter()
                    .find(|e| e.name().is_none())
                    .and_then(|e| e.value().as_i64())
                    .ok_or_else(|| ZError::ConfigParse(format!("step '{name}': max-retries must be an integer")))?;
                if v < 0 {
                    return Err(ZError::ConfigParse(format!("step '{name}': max-retries must be non-negative, got {v}")));
                }
                max_retries = Some(v as u32);
            }
            "timeout" => {
                let v = first_string_arg(child).ok_or_else(|| {
                    ZError::ConfigParse(format!("step '{name}': timeout missing value"))
                })?;
                timeout = Some(v.to_string());
            }
            "on-success" => {
                on_success = first_string_arg(child).map(str::to_string);
            }
            "on-failure" => {
                on_failure = first_string_arg(child).map(str::to_string);
            }
            "on-complete" => {
                on_complete = first_string_arg(child).map(str::to_string);
            }
            "on-max-retries" => {
                on_max_retries = first_string_arg(child).map(str::to_string);
            }
            "on-accept" => {
                on_accept = first_string_arg(child).map(str::to_string);
            }
            "on-reject" => {
                on_reject = first_string_arg(child).map(str::to_string);
            }
            _ => {} // forward-compatible: ignore unknown nodes
        }
    }

    let action = action.ok_or_else(|| {
        ZError::ConfigParse(format!("step '{name}' has no action (run/notify/confirm)"))
    })?;

    Ok(Step {
        name,
        action,
        max_retries,
        timeout,
        on_success,
        on_failure,
        on_complete,
        on_max_retries,
        on_accept,
        on_reject,
    })
}

fn parse_autopilot_node(node: &KdlNode) -> Result<AutopilotWorkflow> {
    let name = first_string_arg(node)
        .ok_or_else(|| ZError::ConfigParse("autopilot missing name".into()))?
        .to_string();

    let children = node.children().ok_or_else(|| {
        ZError::ConfigParse(format!("autopilot '{name}' has no body"))
    })?;

    let mut description: Option<String> = None;
    let mut trigger: Option<Trigger> = None;
    let mut poll_interval: Option<String> = None;
    let mut steps: Vec<Step> = Vec::new();
    let mut auto_push: Option<bool> = None;
    let mut review: Option<bool> = None;

    for child in children.nodes() {
        match child.name().value() {
            "description" => {
                description = first_string_arg(child).map(str::to_string);
            }
            "trigger" => {
                let raw = first_string_arg(child).ok_or_else(|| {
                    ZError::ConfigParse(format!("autopilot '{name}': trigger missing value"))
                })?;
                trigger = Some(Trigger::from_str(raw).ok_or_else(|| {
                    ZError::ConfigParse(format!("autopilot '{name}': unknown trigger '{raw}'"))
                })?);
            }
            "poll-interval" => {
                poll_interval = first_string_arg(child).map(str::to_string);
            }
            "step" => {
                steps.push(parse_step(child)?);
            }
            "auto-push" => {
                auto_push = require_bool_arg(child, &format!("autopilot '{name}'"))?;
            }
            "review" => {
                review = require_bool_arg(child, &format!("autopilot '{name}'"))?;
            }
            _ => {} // forward-compatible
        }
    }

    let trigger = trigger.ok_or_else(|| {
        ZError::ConfigParse(format!("autopilot '{name}' missing trigger"))
    })?;

    Ok(AutopilotWorkflow { name, description, trigger, poll_interval, steps, auto_push, review })
}

/// Parse all `autopilot` nodes from a KDL document string.
pub fn parse_autopilot_workflows(content: &str) -> Result<Vec<AutopilotWorkflow>> {
    let doc: KdlDocument = content.parse().map_err(|e| {
        ZError::ConfigParse(format!("KDL parse error: {e}"))
    })?;

    parse_autopilot_workflows_doc(&doc)
}

/// Parse all workflow `autopilot` nodes from an already-parsed KDL document.
pub fn parse_autopilot_workflows_doc(doc: &KdlDocument) -> Result<Vec<AutopilotWorkflow>> {
    let mut workflows = Vec::new();
    for node in doc.nodes() {
        if node.name().value() == "autopilot" {
            // Skip unnamed `autopilot { ... }` config blocks (no positional string arg).
            let has_name = node.entries().iter().any(|e| e.name().is_none());
            if has_name {
                workflows.push(parse_autopilot_node(node)?);
            }
        }
    }
    Ok(workflows)
}

/// Parse a single `autopilot` block from a KDL document string.
pub fn parse_autopilot_workflow(content: &str) -> Result<AutopilotWorkflow> {
    let mut all = parse_autopilot_workflows(content)?;
    if all.is_empty() {
        return Err(ZError::ConfigParse("no autopilot block found".into()));
    }
    Ok(all.remove(0))
}

/// Validate a workflow:
/// - All transition targets must name existing steps.
/// - No duplicate step names.
/// - No orphan steps (every step reachable from the first).
/// - Cycles are intentionally allowed (retry loops are the core autopilot pattern).
/// - Trigger must be valid (already guaranteed by parsing).
pub fn validate_workflow(workflow: &AutopilotWorkflow) -> Result<()> {
    let step_names: std::collections::HashSet<&str> =
        workflow.steps.iter().map(|s| s.name.as_str()).collect();

    // Check for duplicate step names.
    if step_names.len() != workflow.steps.len() {
        let mut seen = std::collections::HashSet::new();
        for step in &workflow.steps {
            if !seen.insert(step.name.as_str()) {
                return Err(ZError::ConfigParse(format!(
                    "workflow '{}': duplicate step name '{}'",
                    workflow.name, step.name
                )));
            }
        }
    }

    // Check all transition targets exist.
    for step in &workflow.steps {
        let check = |opt: &Option<String>| -> Result<()> {
            if let Some(target) = opt {
                if !step_names.contains(target.as_str()) {
                    return Err(ZError::ConfigParse(format!(
                        "workflow '{}': step '{}' references unknown step '{target}'",
                        workflow.name, step.name
                    )));
                }
            }
            Ok(())
        };
        check(&step.on_success)?;
        check(&step.on_failure)?;
        check(&step.on_complete)?;
        check(&step.on_max_retries)?;
        check(&step.on_accept)?;
        check(&step.on_reject)?;
    }

    // Check on_accept/on_reject only on confirm steps, and on_success/on_failure not on confirm steps.
    for step in &workflow.steps {
        let is_confirm = matches!(step.action, StepAction::Confirm { .. });
        if !is_confirm && (step.on_accept.is_some() || step.on_reject.is_some()) {
            return Err(ZError::ConfigParse(format!(
                "workflow '{}': step '{}' has on-accept/on-reject but is not a confirm step",
                workflow.name, step.name
            )));
        }
    }

    // Check no orphan steps (every step except the first must be reachable).
    if workflow.steps.is_empty() {
        return Err(ZError::ConfigParse(format!(
            "workflow '{}' has no steps",
            workflow.name
        )));
    }

    let mut reachable: std::collections::HashSet<&str> = std::collections::HashSet::new();
    reachable.insert(workflow.steps[0].name.as_str());
    let mut changed = true;
    while changed {
        changed = false;
        for step in &workflow.steps {
            if !reachable.contains(step.name.as_str()) {
                continue;
            }
            let targets = [
                &step.on_success,
                &step.on_failure,
                &step.on_complete,
                &step.on_max_retries,
                &step.on_accept,
                &step.on_reject,
            ];
            for t in targets.into_iter().flatten() {
                if reachable.insert(t.as_str()) {
                    changed = true;
                }
            }
        }
    }

    for step in &workflow.steps {
        if !reachable.contains(step.name.as_str()) {
            return Err(ZError::ConfigParse(format!(
                "workflow '{}': step '{}' is unreachable (orphan)",
                workflow.name, step.name
            )));
        }
    }

    // Note: intentional retry loops (e.g. monitor-ci → fix-ci → monitor-ci) are valid
    // in autopilot workflows. We do NOT reject cycles — they are the core retry pattern.

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PR_CI_FIX_KDL: &str = r#"
autopilot "pr-ci-fix" {
    description "Monitor CI, fix failures with Claude, retry"
    trigger "post-push"

    step "monitor-ci" {
        run "gh run watch --exit-status"
        on-failure "fix-ci"
        on-success "notify-done"
    }

    step "fix-ci" {
        run "claude 'Fix the CI failure based on: $(gh run view --log-failed)'"
        max-retries 3
        on-complete "monitor-ci"
        on-max-retries "notify-stuck"
    }

    step "notify-done" {
        notify "PR CI passing ✅"
    }

    step "notify-stuck" {
        notify "PR CI stuck after 3 attempts ❌"
    }
}
"#;

    #[test]
    fn test_parse_pr_ci_fix() {
        let wf = parse_autopilot_workflow(PR_CI_FIX_KDL).unwrap();
        assert_eq!(wf.name, "pr-ci-fix");
        assert_eq!(wf.description.as_deref(), Some("Monitor CI, fix failures with Claude, retry"));
        assert_eq!(wf.trigger, Trigger::PostPush);
        assert_eq!(wf.steps.len(), 4);
    }

    #[test]
    fn test_parse_step_run_transitions() {
        let wf = parse_autopilot_workflow(PR_CI_FIX_KDL).unwrap();
        let monitor = &wf.steps[0];
        assert_eq!(monitor.name, "monitor-ci");
        assert_eq!(monitor.action, StepAction::Run { command: "gh run watch --exit-status".into() });
        assert_eq!(monitor.on_failure.as_deref(), Some("fix-ci"));
        assert_eq!(monitor.on_success.as_deref(), Some("notify-done"));
        assert!(monitor.on_complete.is_none());
    }

    #[test]
    fn test_parse_step_max_retries() {
        let wf = parse_autopilot_workflow(PR_CI_FIX_KDL).unwrap();
        let fix = &wf.steps[1];
        assert_eq!(fix.name, "fix-ci");
        assert_eq!(fix.max_retries, Some(3));
        assert_eq!(fix.on_complete.as_deref(), Some("monitor-ci"));
        assert_eq!(fix.on_max_retries.as_deref(), Some("notify-stuck"));
    }

    #[test]
    fn test_parse_step_notify() {
        let wf = parse_autopilot_workflow(PR_CI_FIX_KDL).unwrap();
        let notify = &wf.steps[2];
        assert_eq!(notify.name, "notify-done");
        assert_eq!(notify.action, StepAction::Notify { message: "PR CI passing ✅".into() });
    }

    #[test]
    fn test_parse_confirm_step() {
        let kdl = r#"
autopilot "deploy-sync" {
    trigger "new-commits-on-main"
    poll-interval "5m"

    step "pull" {
        run "git pull origin main"
        on-complete "confirm-deploy"
    }

    step "confirm-deploy" {
        confirm "Deploy these changes?"
        on-accept "notify-done"
        on-reject "notify-skipped"
    }

    step "notify-done" {
        notify "Deploy successful ✅"
    }

    step "notify-skipped" {
        notify "Deploy skipped by user"
    }
}
"#;
        let wf = parse_autopilot_workflow(kdl).unwrap();
        assert_eq!(wf.trigger, Trigger::NewCommitsOnMain);
        assert_eq!(wf.poll_interval.as_deref(), Some("5m"));
        let confirm_step = &wf.steps[1];
        assert_eq!(confirm_step.action, StepAction::Confirm { prompt: "Deploy these changes?".into() });
        assert_eq!(confirm_step.on_accept.as_deref(), Some("notify-done"));
        assert_eq!(confirm_step.on_reject.as_deref(), Some("notify-skipped"));
    }

    #[test]
    fn test_parse_multiple_workflows() {
        let kdl = format!("{PR_CI_FIX_KDL}\n{}", r#"
autopilot "manual-test" {
    trigger "manual"
    step "do-stuff" {
        run "./scripts/test.sh"
        on-complete "notify"
    }
    step "notify" {
        notify "Done ✅"
    }
}
"#);
        let workflows = parse_autopilot_workflows(&kdl).unwrap();
        assert_eq!(workflows.len(), 2);
        assert_eq!(workflows[0].name, "pr-ci-fix");
        assert_eq!(workflows[1].name, "manual-test");
        assert_eq!(workflows[1].trigger, Trigger::Manual);
    }

    #[test]
    fn test_parse_all_triggers() {
        for (s, expected) in [
            ("post-push", Trigger::PostPush),
            ("pr-approved", Trigger::PrApproved),
            ("pr-review-received", Trigger::PrReviewReceived),
            ("pr-opened-by-dependabot", Trigger::PrOpenedByDependabot),
            ("post-merge-main", Trigger::PostMergeMain),
            ("new-commits-on-main", Trigger::NewCommitsOnMain),
            ("manual", Trigger::Manual),
        ] {
            assert_eq!(Trigger::from_str(s), Some(expected));
        }
        assert_eq!(Trigger::from_str("unknown-trigger"), None);
    }

    #[test]
    fn test_parse_timeout() {
        let kdl = r#"
autopilot "deploy-watch" {
    trigger "post-merge-main"
    step "monitor-deploy" {
        run "deploy_command --status"
        timeout "10m"
        on-success "notify-done"
        on-failure "rollback"
    }
    step "rollback" {
        run "deploy_command --rollback"
        on-complete "notify-done"
    }
    step "notify-done" {
        notify "Done"
    }
}
"#;
        let wf = parse_autopilot_workflow(kdl).unwrap();
        assert_eq!(wf.steps[0].timeout.as_deref(), Some("10m"));
    }

    #[test]
    fn test_parse_missing_trigger_error() {
        let kdl = r#"
autopilot "bad" {
    step "s" {
        run "cmd"
    }
}
"#;
        assert!(parse_autopilot_workflow(kdl).is_err());
    }

    #[test]
    fn test_parse_unknown_trigger_error() {
        let kdl = r#"
autopilot "bad" {
    trigger "on-monday-morning"
    step "s" {
        run "cmd"
    }
}
"#;
        let err = parse_autopilot_workflow(kdl).unwrap_err();
        assert!(err.to_string().contains("unknown trigger"));
    }

    #[test]
    fn test_parse_step_missing_action_error() {
        let kdl = r#"
autopilot "bad" {
    trigger "manual"
    step "s" {
        on-complete "s"
    }
}
"#;
        let err = parse_autopilot_workflow(kdl).unwrap_err();
        assert!(err.to_string().contains("no action"));
    }

    #[test]
    fn test_validate_valid_workflow() {
        let wf = parse_autopilot_workflow(PR_CI_FIX_KDL).unwrap();
        assert!(validate_workflow(&wf).is_ok());
    }

    #[test]
    fn test_validate_unknown_transition_target() {
        let kdl = r#"
autopilot "bad" {
    trigger "manual"
    step "start" {
        run "cmd"
        on-complete "nonexistent"
    }
}
"#;
        let wf = parse_autopilot_workflow(kdl).unwrap();
        let err = validate_workflow(&wf).unwrap_err();
        assert!(err.to_string().contains("unknown step"));
    }

    #[test]
    fn test_validate_orphan_step() {
        let kdl = r#"
autopilot "test" {
    trigger "manual"
    step "start" {
        run "cmd"
        on-complete "end"
    }
    step "end" {
        notify "Done"
    }
    step "orphan" {
        notify "Never reached"
    }
}
"#;
        let wf = parse_autopilot_workflow(kdl).unwrap();
        let err = validate_workflow(&wf).unwrap_err();
        assert!(err.to_string().contains("orphan") || err.to_string().contains("unreachable"));
    }

    #[test]
    fn test_validate_retry_loops_are_allowed() {
        // Autopilot workflows intentionally have retry loops (e.g., monitor → fix → monitor).
        // These are valid and must NOT be rejected.
        let kdl = r#"
autopilot "test" {
    trigger "manual"
    step "a" {
        run "cmd"
        on-complete "b"
    }
    step "b" {
        run "cmd"
        on-complete "a"
    }
}
"#;
        let wf = parse_autopilot_workflow(kdl).unwrap();
        // Cycles are allowed — this should validate successfully.
        assert!(validate_workflow(&wf).is_ok());
    }

    #[test]
    fn test_validate_no_steps_error() {
        let kdl = r#"
autopilot "empty" {
    trigger "manual"
}
"#;
        let wf = parse_autopilot_workflow(kdl).unwrap();
        let err = validate_workflow(&wf).unwrap_err();
        assert!(err.to_string().contains("no steps"));
    }

    #[test]
    fn test_parse_empty_document() {
        let workflows = parse_autopilot_workflows("").unwrap();
        assert!(workflows.is_empty());
    }

    #[test]
    fn test_parse_negative_max_retries_error() {
        let kdl = r#"
autopilot "bad" {
    trigger "manual"
    step "s" {
        run "cmd"
        max-retries -1
    }
}
"#;
        let err = parse_autopilot_workflow(kdl).unwrap_err();
        assert!(err.to_string().contains("non-negative"));
    }

    #[test]
    fn test_validate_duplicate_step_names() {
        let kdl = r#"
autopilot "test" {
    trigger "manual"
    step "s" {
        run "cmd1"
        on-complete "s"
    }
    step "s" {
        run "cmd2"
    }
}
"#;
        let wf = parse_autopilot_workflow(kdl).unwrap();
        let err = validate_workflow(&wf).unwrap_err();
        assert!(err.to_string().contains("duplicate step name"));
    }

    #[test]
    fn test_parse_autopilot_unnamed_block_is_config_not_workflow() {
        // An unnamed `autopilot { ... }` block is a config block (auto-push/review settings),
        // not a workflow definition — it is silently skipped by parse_autopilot_workflows.
        let kdl = r#"
autopilot {
    auto-push false
    review true
}
"#;
        // No workflow definitions found — parse_autopilot_workflow returns "no autopilot block found".
        let err = parse_autopilot_workflow(kdl).unwrap_err();
        assert!(err.to_string().contains("no autopilot block found"));
        // parse_autopilot_workflows returns empty list.
        let workflows = parse_autopilot_workflows(kdl).unwrap();
        assert!(workflows.is_empty());
    }

    #[test]
    fn test_parse_step_missing_name_error() {
        let kdl = r#"
autopilot "test" {
    trigger "manual"
    step {
        run "cmd"
    }
}
"#;
        let err = parse_autopilot_workflow(kdl).unwrap_err();
        assert!(err.to_string().contains("step missing name"));
    }

    #[test]
    fn test_trigger_as_str_roundtrips() {
        let triggers = [
            Trigger::PostPush,
            Trigger::PrApproved,
            Trigger::PrReviewReceived,
            Trigger::PrOpenedByDependabot,
            Trigger::PostMergeMain,
            Trigger::NewCommitsOnMain,
            Trigger::Manual,
        ];
        for t in &triggers {
            assert_eq!(Trigger::from_str(t.as_str()).as_ref(), Some(t));
        }
    }

    #[test]
    fn test_validate_on_accept_on_non_confirm_step_error() {
        let kdl = r#"
autopilot "bad" {
    trigger "manual"
    step "s" {
        run "cmd"
        on-accept "s"
    }
}
"#;
        let wf = parse_autopilot_workflow(kdl).unwrap();
        let err = validate_workflow(&wf).unwrap_err();
        assert!(err.to_string().contains("on-accept/on-reject"));
        assert!(err.to_string().contains("not a confirm step"));
    }

    #[test]
    fn test_validate_on_reject_on_non_confirm_step_error() {
        let kdl = r#"
autopilot "bad" {
    trigger "manual"
    step "start" {
        notify "hi"
        on-reject "start"
    }
}
"#;
        let wf = parse_autopilot_workflow(kdl).unwrap();
        let err = validate_workflow(&wf).unwrap_err();
        assert!(err.to_string().contains("on-accept/on-reject"));
    }

    #[test]
    fn test_validate_on_accept_on_confirm_step_ok() {
        let kdl = r#"
autopilot "good" {
    trigger "manual"
    step "ask" {
        confirm "Do it?"
        on-accept "done"
        on-reject "done"
    }
    step "done" {
        notify "ok"
    }
}
"#;
        let wf = parse_autopilot_workflow(kdl).unwrap();
        assert!(validate_workflow(&wf).is_ok());
    }

    #[test]
    fn test_parse_ignores_non_autopilot_nodes() {
        let kdl = r#"
config {
    something "irrelevant"
}
autopilot "real" {
    trigger "manual"
    step "s" {
        notify "hi"
    }
}
"#;
        let workflows = parse_autopilot_workflows(kdl).unwrap();
        assert_eq!(workflows.len(), 1);
        assert_eq!(workflows[0].name, "real");
    }

    #[test]
    fn test_parse_workflow_review_string_value_is_error() {
        let kdl = r#"
autopilot "wf" {
    trigger "manual"
    review "true"
    step "s" {
        run "cmd"
    }
}
"#;
        let err = parse_autopilot_workflow(kdl).unwrap_err();
        assert!(err.to_string().contains("boolean"));
    }

    #[test]
    fn test_parse_workflow_auto_push_integer_is_error() {
        let kdl = r#"
autopilot "wf" {
    trigger "manual"
    auto-push 1
    step "s" {
        run "cmd"
    }
}
"#;
        let err = parse_autopilot_workflow(kdl).unwrap_err();
        assert!(err.to_string().contains("boolean"));
    }
}
