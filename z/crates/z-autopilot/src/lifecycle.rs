use crate::dsl::AutopilotWorkflow;
use crate::notify::{event_from_advance, AutopilotEvent};
use crate::state::{advance, StepResult, WorkflowRun};
use z_core::error::Result;

/// Result of advancing a workflow run by one executed step.
#[derive(Debug, Clone, PartialEq)]
pub struct AdvanceOutcome {
    pub next_step: Option<String>,
    pub event: Option<AutopilotEvent>,
}

/// Advance a workflow run and capture the event produced by that transition.
///
/// This Module keeps the ordering-sensitive Implementation local: callers no
/// longer need to remember to call `event_from_advance` immediately after
/// `advance`.
pub fn advance_run(
    workflow: &AutopilotWorkflow,
    run: &mut WorkflowRun,
    result: StepResult,
) -> Result<AdvanceOutcome> {
    let next_step = advance(workflow, run, result)?;
    let event = event_from_advance(run);
    Ok(AdvanceOutcome { next_step, event })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::parse_autopilot_workflow;

    const WF_KDL: &str = r#"
autopilot "pr-ci-fix" {
    trigger "post-push"
    step "monitor-ci" {
        run "gh run watch --exit-status"
        on-failure "fix-ci"
        on-success "notify-done"
    }
    step "fix-ci" {
        run "claude 'fix'"
        max-retries 1
        on-complete "monitor-ci"
        on-max-retries "notify-stuck"
    }
    step "notify-done" {
        notify "PR CI passing"
    }
    step "notify-stuck" {
        notify "PR CI stuck"
    }
}
"#;

    fn workflow() -> AutopilotWorkflow {
        parse_autopilot_workflow(WF_KDL).unwrap()
    }

    fn run_at(step: &str) -> WorkflowRun {
        WorkflowRun::new("pr-ci-fix", "myproject", step)
    }

    #[test]
    fn returns_next_step_without_event_while_running() {
        let workflow = workflow();
        let mut run = run_at("monitor-ci");

        let outcome =
            advance_run(&workflow, &mut run, StepResult::Failure { output: None }).unwrap();

        assert_eq!(outcome.next_step.as_deref(), Some("fix-ci"));
        assert!(outcome.event.is_none());
    }

    #[test]
    fn returns_completed_event_for_terminal_success() {
        let workflow = workflow();
        let mut run = run_at("notify-done");

        let outcome =
            advance_run(&workflow, &mut run, StepResult::Success { output: None }).unwrap();

        assert_eq!(outcome.next_step, None);
        assert_eq!(
            outcome.event,
            Some(AutopilotEvent::Completed {
                workflow_name: "pr-ci-fix".to_string(),
                final_step: "notify-done".to_string(),
            })
        );
    }

    #[test]
    fn captures_max_retries_event_before_next_advance_can_overwrite_it() {
        let workflow = workflow();
        let mut run = run_at("fix-ci");
        run.retry_count = 1;

        let outcome =
            advance_run(&workflow, &mut run, StepResult::Failure { output: None }).unwrap();

        assert_eq!(outcome.next_step.as_deref(), Some("notify-stuck"));
        assert_eq!(
            outcome.event,
            Some(AutopilotEvent::MaxRetriesExhausted {
                workflow_name: "pr-ci-fix".to_string(),
                step_name: "fix-ci".to_string(),
                retry_count: 1,
            })
        );
    }
}
