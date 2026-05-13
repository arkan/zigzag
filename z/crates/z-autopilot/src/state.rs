use crate::dsl::{AutopilotWorkflow, Step};
use serde::{Deserialize, Serialize};
use z_core::error::{Result, ZError};

/// Result of executing a step's action.
#[derive(Debug, Clone, PartialEq)]
pub enum StepResult {
    /// Step succeeded (exit code 0 or confirmed).
    Success { output: Option<String> },
    /// Step failed (non-zero exit or rejected).
    Failure { output: Option<String> },
}

/// Execution status of a single step attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StepStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    MaxRetriesExhausted,
}

/// Record of one step execution (possibly one of multiple retries).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepExecution {
    pub step_name: String,
    pub status: StepStatus,
    pub retry_count: u32,
    pub output: Option<String>,
}

/// Overall workflow run status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WorkflowStatus {
    Running,
    Completed,
    Failed,
    /// Max retries exhausted on a step — workflow is stuck.
    Stuck,
}

/// Persisted state of a running or completed workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRun {
    pub workflow_name: String,
    pub project: String,
    /// Remote host this workflow targets (None = local).
    pub host: Option<String>,
    pub status: WorkflowStatus,
    pub current_step: Option<String>,
    pub retry_count: u32,
    pub history: Vec<StepExecution>,
}

impl WorkflowRun {
    pub fn new(
        workflow_name: impl Into<String>,
        project: impl Into<String>,
        first_step: impl Into<String>,
    ) -> Self {
        WorkflowRun {
            workflow_name: workflow_name.into(),
            project: project.into(),
            host: None,
            status: WorkflowStatus::Running,
            current_step: Some(first_step.into()),
            retry_count: 0,
            history: Vec::new(),
        }
    }

    /// Set the remote host for this workflow run.
    pub fn with_host(mut self, host: impl Into<String>) -> Self {
        self.host = Some(host.into());
        self
    }
}

/// Determine the next step name (or None if the workflow ends) after a step result.
///
/// This is the core state machine transition logic.
pub fn advance(
    workflow: &AutopilotWorkflow,
    run: &mut WorkflowRun,
    result: StepResult,
) -> Result<Option<String>> {
    let step_name = match &run.current_step {
        Some(s) => s.clone(),
        None => return Ok(None),
    };

    let step = workflow
        .steps
        .iter()
        .find(|s| s.name == step_name)
        .ok_or_else(|| {
            ZError::ConfigParse(format!(
                "unknown step '{step_name}' in workflow '{}'",
                workflow.name
            ))
        })?;

    let succeeded = matches!(result, StepResult::Success { .. });
    let output = match &result {
        StepResult::Success { output } | StepResult::Failure { output } => output.clone(),
    };

    // Record this execution.
    let status = if succeeded {
        StepStatus::Succeeded
    } else {
        StepStatus::Failed
    };
    run.history.push(StepExecution {
        step_name: step_name.clone(),
        status: status.clone(),
        retry_count: run.retry_count,
        output,
    });

    // Determine next step.
    let next = if !succeeded {
        // Check max-retries before transitioning.
        if let Some(max) = step.max_retries {
            if run.retry_count < max {
                // Retry the same step.
                run.retry_count += 1;
                // Update last history entry to reflect retry in progress.
                return Ok(Some(step_name));
            } else {
                // Max retries exhausted.
                run.history.last_mut().unwrap().status = StepStatus::MaxRetriesExhausted;
                if let Some(target) = &step.on_max_retries {
                    run.retry_count = 0;
                    target.clone()
                } else if let Some(target) = &step.on_failure {
                    run.retry_count = 0;
                    target.clone()
                } else if let Some(target) = &step.on_complete {
                    run.retry_count = 0;
                    target.clone()
                } else {
                    // No transition target at all: workflow is stuck.
                    run.status = WorkflowStatus::Stuck;
                    run.current_step = None;
                    return Ok(None);
                }
            }
        // on_reject is the confirm-step equivalent of on_failure.
        } else if let Some(target) = &step.on_reject {
            run.retry_count = 0;
            target.clone()
        } else if let Some(target) = &step.on_failure {
            run.retry_count = 0;
            target.clone()
        } else if let Some(target) = &step.on_complete {
            run.retry_count = 0;
            target.clone()
        } else {
            // No transition: workflow ends (failure).
            run.status = WorkflowStatus::Failed;
            run.current_step = None;
            return Ok(None);
        }
    } else {
        run.retry_count = 0;
        // on_accept is the confirm-step equivalent of on_success.
        if let Some(target) = &step.on_accept {
            target.clone()
        } else if let Some(target) = &step.on_success {
            target.clone()
        } else if let Some(target) = &step.on_complete {
            target.clone()
        } else {
            // Terminal step — workflow completed.
            run.status = WorkflowStatus::Completed;
            run.current_step = None;
            return Ok(None);
        }
    };

    run.current_step = Some(next.clone());
    Ok(Some(next))
}

/// Return the step definition for the current step in the run.
pub fn current_step<'a>(workflow: &'a AutopilotWorkflow, run: &WorkflowRun) -> Option<&'a Step> {
    run.current_step
        .as_ref()
        .and_then(|name| workflow.steps.iter().find(|s| &s.name == name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::parse_autopilot_workflow;

    fn make_run(name: &str, first_step: &str) -> WorkflowRun {
        WorkflowRun::new(name, "myproject", first_step)
    }

    const PR_CI_FIX_KDL: &str = r#"
autopilot "pr-ci-fix" {
    trigger "post-push"
    step "monitor-ci" {
        run "gh run watch --exit-status"
        on-failure "fix-ci"
        on-success "notify-done"
    }
    step "fix-ci" {
        run "claude 'fix'"
        max-retries 3
        on-complete "monitor-ci"
        on-max-retries "notify-stuck"
    }
    step "notify-done" {
        notify "PR CI passing ✅"
    }
    step "notify-stuck" {
        notify "PR CI stuck ❌"
    }
}
"#;

    #[test]
    fn test_success_transition() {
        let wf = parse_autopilot_workflow(PR_CI_FIX_KDL).unwrap();
        let mut run = make_run("pr-ci-fix", "monitor-ci");
        let next = advance(&wf, &mut run, StepResult::Success { output: None }).unwrap();
        assert_eq!(next.as_deref(), Some("notify-done"));
        assert_eq!(run.current_step.as_deref(), Some("notify-done"));
    }

    #[test]
    fn test_failure_transition() {
        let wf = parse_autopilot_workflow(PR_CI_FIX_KDL).unwrap();
        let mut run = make_run("pr-ci-fix", "monitor-ci");
        let next = advance(&wf, &mut run, StepResult::Failure { output: None }).unwrap();
        assert_eq!(next.as_deref(), Some("fix-ci"));
        assert_eq!(run.current_step.as_deref(), Some("fix-ci"));
    }

    #[test]
    fn test_on_complete_transition() {
        let wf = parse_autopilot_workflow(PR_CI_FIX_KDL).unwrap();
        let mut run = make_run("pr-ci-fix", "fix-ci");
        // fix-ci uses on-complete, so both success and failure go to monitor-ci.
        let next = advance(&wf, &mut run, StepResult::Success { output: None }).unwrap();
        assert_eq!(next.as_deref(), Some("monitor-ci"));
    }

    #[test]
    fn test_on_complete_on_failure() {
        let wf = parse_autopilot_workflow(PR_CI_FIX_KDL).unwrap();
        let mut run = make_run("pr-ci-fix", "fix-ci");
        // failure with max_retries not yet exhausted → retry (count = 1).
        let next = advance(&wf, &mut run, StepResult::Failure { output: None }).unwrap();
        assert_eq!(next.as_deref(), Some("fix-ci"));
        assert_eq!(run.retry_count, 1);
    }

    #[test]
    fn test_max_retries_exhausted_transitions_to_on_max_retries() {
        let wf = parse_autopilot_workflow(PR_CI_FIX_KDL).unwrap();
        let mut run = make_run("pr-ci-fix", "fix-ci");
        run.retry_count = 3; // already at max

        let next = advance(&wf, &mut run, StepResult::Failure { output: None }).unwrap();
        assert_eq!(next.as_deref(), Some("notify-stuck"));
        assert_eq!(run.retry_count, 0); // reset after transition
    }

    #[test]
    fn test_terminal_step_completes_workflow() {
        let wf = parse_autopilot_workflow(PR_CI_FIX_KDL).unwrap();
        let mut run = make_run("pr-ci-fix", "notify-done");
        let next = advance(&wf, &mut run, StepResult::Success { output: None }).unwrap();
        assert!(next.is_none());
        assert_eq!(run.status, WorkflowStatus::Completed);
        assert!(run.current_step.is_none());
    }

    #[test]
    fn test_failure_no_transition_fails_workflow() {
        let kdl = r#"
autopilot "test" {
    trigger "manual"
    step "only" {
        run "cmd"
    }
}
"#;
        let wf = parse_autopilot_workflow(kdl).unwrap();
        let mut run = make_run("test", "only");
        let next = advance(&wf, &mut run, StepResult::Failure { output: None }).unwrap();
        assert!(next.is_none());
        assert_eq!(run.status, WorkflowStatus::Failed);
    }

    #[test]
    fn test_history_records_executions() {
        let wf = parse_autopilot_workflow(PR_CI_FIX_KDL).unwrap();
        let mut run = make_run("pr-ci-fix", "monitor-ci");
        advance(
            &wf,
            &mut run,
            StepResult::Success {
                output: Some("ok".into()),
            },
        )
        .unwrap();
        assert_eq!(run.history.len(), 1);
        assert_eq!(run.history[0].step_name, "monitor-ci");
        assert_eq!(run.history[0].status, StepStatus::Succeeded);
    }

    #[test]
    fn test_max_retries_no_on_max_retries_makes_workflow_stuck() {
        let kdl = r#"
autopilot "test" {
    trigger "manual"
    step "flaky" {
        run "cmd"
        max-retries 2
    }
}
"#;
        let wf = parse_autopilot_workflow(kdl).unwrap();
        let mut run = make_run("test", "flaky");
        run.retry_count = 2; // already at max

        let next = advance(&wf, &mut run, StepResult::Failure { output: None }).unwrap();
        assert!(next.is_none());
        assert_eq!(run.status, WorkflowStatus::Stuck);
    }

    #[test]
    fn test_current_step_returns_step_definition() {
        let wf = parse_autopilot_workflow(PR_CI_FIX_KDL).unwrap();
        let run = make_run("pr-ci-fix", "monitor-ci");
        let step = current_step(&wf, &run).unwrap();
        assert_eq!(step.name, "monitor-ci");
    }

    #[test]
    fn test_max_retries_exhausted_falls_back_to_on_failure() {
        let kdl = r#"
autopilot "test" {
    trigger "manual"
    step "flaky" {
        run "cmd"
        max-retries 1
        on-failure "recover"
    }
    step "recover" {
        notify "Recovered"
    }
}
"#;
        let wf = parse_autopilot_workflow(kdl).unwrap();
        let mut run = make_run("test", "flaky");
        run.retry_count = 1; // at max

        let next = advance(&wf, &mut run, StepResult::Failure { output: None }).unwrap();
        assert_eq!(next.as_deref(), Some("recover"));
        assert_eq!(run.retry_count, 0);
    }

    #[test]
    fn test_max_retries_exhausted_falls_back_to_on_complete() {
        let kdl = r#"
autopilot "test" {
    trigger "manual"
    step "flaky" {
        run "cmd"
        max-retries 1
        on-complete "next"
    }
    step "next" {
        notify "Done"
    }
}
"#;
        let wf = parse_autopilot_workflow(kdl).unwrap();
        let mut run = make_run("test", "flaky");
        run.retry_count = 1;

        let next = advance(&wf, &mut run, StepResult::Failure { output: None }).unwrap();
        assert_eq!(next.as_deref(), Some("next"));
    }

    #[test]
    fn test_max_retries_prefers_on_max_retries_over_on_failure() {
        let kdl = r#"
autopilot "test" {
    trigger "manual"
    step "flaky" {
        run "cmd"
        max-retries 1
        on-failure "fallback"
        on-max-retries "exhausted"
    }
    step "fallback" {
        notify "fallback"
    }
    step "exhausted" {
        notify "exhausted"
    }
}
"#;
        let wf = parse_autopilot_workflow(kdl).unwrap();
        let mut run = make_run("test", "flaky");
        run.retry_count = 1;

        let next = advance(&wf, &mut run, StepResult::Failure { output: None }).unwrap();
        assert_eq!(next.as_deref(), Some("exhausted"));
    }

    #[test]
    fn test_advance_on_already_completed_workflow() {
        let wf = parse_autopilot_workflow(PR_CI_FIX_KDL).unwrap();
        let mut run = make_run("pr-ci-fix", "monitor-ci");
        run.status = WorkflowStatus::Completed;
        run.current_step = None;

        let next = advance(&wf, &mut run, StepResult::Success { output: None }).unwrap();
        assert!(next.is_none());
    }

    #[test]
    fn test_advance_unknown_step_is_error() {
        let wf = parse_autopilot_workflow(PR_CI_FIX_KDL).unwrap();
        let mut run = make_run("pr-ci-fix", "nonexistent-step");

        let result = advance(&wf, &mut run, StepResult::Success { output: None });
        assert!(result.is_err());
    }

    #[test]
    fn test_retry_increments_then_transitions() {
        let kdl = r#"
autopilot "test" {
    trigger "manual"
    step "flaky" {
        run "cmd"
        max-retries 2
        on-max-retries "done"
    }
    step "done" {
        notify "done"
    }
}
"#;
        let wf = parse_autopilot_workflow(kdl).unwrap();
        let mut run = make_run("test", "flaky");

        // First failure: retry (count 0 -> 1)
        let next = advance(&wf, &mut run, StepResult::Failure { output: None }).unwrap();
        assert_eq!(next.as_deref(), Some("flaky"));
        assert_eq!(run.retry_count, 1);

        // Second failure: retry (count 1 -> 2)
        let next = advance(&wf, &mut run, StepResult::Failure { output: None }).unwrap();
        assert_eq!(next.as_deref(), Some("flaky"));
        assert_eq!(run.retry_count, 2);

        // Third failure: exhausted (count == max), transition to on-max-retries
        let next = advance(&wf, &mut run, StepResult::Failure { output: None }).unwrap();
        assert_eq!(next.as_deref(), Some("done"));
        assert_eq!(run.retry_count, 0);
        assert_eq!(
            run.history.last().unwrap().status,
            StepStatus::MaxRetriesExhausted
        );
    }

    #[test]
    fn test_success_resets_retry_count() {
        let kdl = r#"
autopilot "test" {
    trigger "manual"
    step "flaky" {
        run "cmd"
        max-retries 3
        on-success "done"
    }
    step "done" {
        notify "done"
    }
}
"#;
        let wf = parse_autopilot_workflow(kdl).unwrap();
        let mut run = make_run("test", "flaky");
        run.retry_count = 2; // had some retries

        let next = advance(&wf, &mut run, StepResult::Success { output: None }).unwrap();
        assert_eq!(next.as_deref(), Some("done"));
        assert_eq!(run.retry_count, 0);
    }

    #[test]
    fn test_confirm_step_accept_uses_on_accept() {
        let kdl = r#"
autopilot "test" {
    trigger "manual"
    step "ask" {
        confirm "Proceed?"
        on-accept "yes-path"
        on-reject "no-path"
    }
    step "yes-path" {
        notify "Accepted"
    }
    step "no-path" {
        notify "Rejected"
    }
}
"#;
        let wf = parse_autopilot_workflow(kdl).unwrap();
        let mut run = make_run("test", "ask");
        let next = advance(&wf, &mut run, StepResult::Success { output: None }).unwrap();
        assert_eq!(next.as_deref(), Some("yes-path"));
    }

    #[test]
    fn test_confirm_step_reject_uses_on_reject() {
        let kdl = r#"
autopilot "test" {
    trigger "manual"
    step "ask" {
        confirm "Proceed?"
        on-accept "yes-path"
        on-reject "no-path"
    }
    step "yes-path" {
        notify "Accepted"
    }
    step "no-path" {
        notify "Rejected"
    }
}
"#;
        let wf = parse_autopilot_workflow(kdl).unwrap();
        let mut run = make_run("test", "ask");
        let next = advance(&wf, &mut run, StepResult::Failure { output: None }).unwrap();
        assert_eq!(next.as_deref(), Some("no-path"));
    }

    #[test]
    fn test_confirm_step_reject_falls_back_to_on_failure() {
        let kdl = r#"
autopilot "test" {
    trigger "manual"
    step "ask" {
        confirm "Proceed?"
        on-accept "done"
        on-failure "fallback"
    }
    step "done" {
        notify "ok"
    }
    step "fallback" {
        notify "fell back"
    }
}
"#;
        let wf = parse_autopilot_workflow(kdl).unwrap();
        let mut run = make_run("test", "ask");
        // No on-reject set, should fall through to on-failure
        let next = advance(&wf, &mut run, StepResult::Failure { output: None }).unwrap();
        assert_eq!(next.as_deref(), Some("fallback"));
    }

    #[test]
    fn test_confirm_step_reject_prefers_on_reject_over_on_failure() {
        let kdl = r#"
autopilot "test" {
    trigger "manual"
    step "ask" {
        confirm "Proceed?"
        on-accept "done"
        on-reject "rejected"
        on-failure "failed"
    }
    step "done" {
        notify "ok"
    }
    step "rejected" {
        notify "rejected"
    }
    step "failed" {
        notify "failed"
    }
}
"#;
        let wf = parse_autopilot_workflow(kdl).unwrap();
        let mut run = make_run("test", "ask");
        let next = advance(&wf, &mut run, StepResult::Failure { output: None }).unwrap();
        assert_eq!(
            next.as_deref(),
            Some("rejected"),
            "on_reject should take priority over on_failure"
        );
    }

    #[test]
    fn test_workflow_run_new() {
        let run = WorkflowRun::new("my-wf", "my-project", "first-step");
        assert_eq!(run.workflow_name, "my-wf");
        assert_eq!(run.project, "my-project");
        assert_eq!(run.current_step.as_deref(), Some("first-step"));
        assert_eq!(run.status, WorkflowStatus::Running);
        assert_eq!(run.retry_count, 0);
        assert!(run.history.is_empty());
    }

    #[test]
    fn test_workflow_run_new_has_no_host() {
        let run = WorkflowRun::new("wf", "proj", "step1");
        assert!(run.host.is_none());
    }

    #[test]
    fn test_workflow_run_with_host_serializes() {
        let run = WorkflowRun::new("wf", "proj", "step1").with_host("vps.example.com");
        assert_eq!(run.host.as_deref(), Some("vps.example.com"));

        let json = serde_json::to_string(&run).unwrap();
        let deserialized: WorkflowRun = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.host.as_deref(), Some("vps.example.com"));
        assert_eq!(deserialized.workflow_name, "wf");
    }

    #[test]
    fn test_advance_works_with_host_set() {
        let wf = parse_autopilot_workflow(PR_CI_FIX_KDL).unwrap();
        let mut run =
            WorkflowRun::new("pr-ci-fix", "myproject", "monitor-ci").with_host("vps.example.com");
        let next = advance(&wf, &mut run, StepResult::Success { output: None }).unwrap();
        assert_eq!(next.as_deref(), Some("notify-done"));
        assert_eq!(run.host.as_deref(), Some("vps.example.com"));
    }
}
