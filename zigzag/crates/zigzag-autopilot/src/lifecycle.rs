use crate::dsl::{AutopilotWorkflow, StepAction};
use crate::notify::{event_from_advance, AutopilotEvent};
use crate::state::{advance, current_step, StepResult, WorkflowRun};
use zigzag_core::error::{Result, ZError};

/// Result of advancing a workflow run by one executed step.
#[derive(Debug, Clone, PartialEq)]
pub struct AdvanceOutcome {
    pub next_step: Option<String>,
    pub event: Option<AutopilotEvent>,
}

/// Outcome of executing the current step and advancing the run.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecuteStepOutcome {
    pub step_name: String,
    pub result: StepResult,
    pub advance: AdvanceOutcome,
}

/// Execution Interface for Autopilot step actions.
///
/// Concrete Adapters decide how commands run, how confirmations are collected,
/// and how notifications are delivered. The lifecycle Module owns the ordering:
/// execute one action, convert it to `StepResult`, advance state, capture event.
pub trait StepExecutor {
    fn run_command(&self, command: &str) -> Result<StepResult>;
    fn notify(&self, message: &str) -> Result<()>;
    fn confirm(&self, prompt: &str) -> Result<bool>;
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

/// Execute the run's current step through `executor`, then advance the run.
pub fn execute_current_step(
    workflow: &AutopilotWorkflow,
    run: &mut WorkflowRun,
    executor: &dyn StepExecutor,
) -> Result<ExecuteStepOutcome> {
    let step = current_step(workflow, run).ok_or_else(|| {
        ZError::ConfigParse(format!(
            "workflow '{}' has no executable current step",
            run.workflow_name
        ))
    })?;
    let step_name = step.name.clone();
    let result = match &step.action {
        StepAction::Run { command } => executor.run_command(command)?,
        StepAction::Notify { message } => {
            executor.notify(message)?;
            StepResult::Success { output: None }
        }
        StepAction::Confirm { prompt } => {
            if executor.confirm(prompt)? {
                StepResult::Success { output: None }
            } else {
                StepResult::Failure { output: None }
            }
        }
    };
    let advance = advance_run(workflow, run, result.clone())?;
    Ok(ExecuteStepOutcome {
        step_name,
        result,
        advance,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::parse_autopilot_workflow;
    use std::cell::RefCell;

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

    const EXEC_WF_KDL: &str = r#"
autopilot "manual-check" {
    trigger "manual"
    step "ask" {
        confirm "Proceed?"
        on-accept "run"
        on-reject "notify-failed"
    }
    step "run" {
        run "cargo test"
        on-success "notify-done"
        on-failure "notify-failed"
    }
    step "notify-done" {
        notify "done"
    }
    step "notify-failed" {
        notify "failed"
    }
}
"#;

    struct FakeExecutor {
        run_result: StepResult,
        confirm_result: bool,
        calls: RefCell<Vec<String>>,
    }

    impl FakeExecutor {
        fn new(run_result: StepResult, confirm_result: bool) -> Self {
            Self {
                run_result,
                confirm_result,
                calls: RefCell::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
    }

    impl StepExecutor for FakeExecutor {
        fn run_command(&self, command: &str) -> Result<StepResult> {
            self.calls.borrow_mut().push(format!("run:{command}"));
            Ok(self.run_result.clone())
        }

        fn notify(&self, message: &str) -> Result<()> {
            self.calls.borrow_mut().push(format!("notify:{message}"));
            Ok(())
        }

        fn confirm(&self, prompt: &str) -> Result<bool> {
            self.calls.borrow_mut().push(format!("confirm:{prompt}"));
            Ok(self.confirm_result)
        }
    }

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

    #[test]
    fn execute_current_step_confirms_and_advances() {
        let workflow = parse_autopilot_workflow(EXEC_WF_KDL).unwrap();
        let mut run = WorkflowRun::new("manual-check", "myproject", "ask");
        let executor = FakeExecutor::new(StepResult::Success { output: None }, true);

        let outcome = execute_current_step(&workflow, &mut run, &executor).unwrap();

        assert_eq!(outcome.step_name, "ask");
        assert_eq!(outcome.result, StepResult::Success { output: None });
        assert_eq!(outcome.advance.next_step.as_deref(), Some("run"));
        assert_eq!(executor.calls(), vec!["confirm:Proceed?"]);
    }

    #[test]
    fn execute_current_step_runs_command_and_advances() {
        let workflow = parse_autopilot_workflow(EXEC_WF_KDL).unwrap();
        let mut run = WorkflowRun::new("manual-check", "myproject", "run");
        let executor = FakeExecutor::new(
            StepResult::Failure {
                output: Some("failed".to_string()),
            },
            true,
        );

        let outcome = execute_current_step(&workflow, &mut run, &executor).unwrap();

        assert_eq!(outcome.step_name, "run");
        assert_eq!(outcome.advance.next_step.as_deref(), Some("notify-failed"));
        assert_eq!(executor.calls(), vec!["run:cargo test"]);
    }

    #[test]
    fn execute_current_step_notifies_and_captures_terminal_event() {
        let workflow = parse_autopilot_workflow(EXEC_WF_KDL).unwrap();
        let mut run = WorkflowRun::new("manual-check", "myproject", "notify-done");
        let executor = FakeExecutor::new(StepResult::Success { output: None }, true);

        let outcome = execute_current_step(&workflow, &mut run, &executor).unwrap();

        assert_eq!(outcome.step_name, "notify-done");
        assert_eq!(outcome.advance.next_step, None);
        assert_eq!(
            outcome.advance.event,
            Some(AutopilotEvent::Completed {
                workflow_name: "manual-check".to_string(),
                final_step: "notify-done".to_string(),
            })
        );
        assert_eq!(executor.calls(), vec!["notify:done"]);
    }
}
