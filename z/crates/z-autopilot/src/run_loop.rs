use crate::dsl::AutopilotWorkflow;
use crate::lifecycle::{execute_current_step, ExecuteStepOutcome, StepExecutor};
use crate::notify::{notify_autopilot_event, AutopilotEvent};
use crate::state::{WorkflowRun, WorkflowStatus};
use z_core::error::{Result, ZError};
use z_core::traits::Notifier;

/// Persistence Interface for Autopilot run state.
pub trait RunStore {
    fn load_run(&self, project: &str, workflow_name: &str) -> Result<Option<WorkflowRun>>;
    fn save_run(&self, run: &WorkflowRun) -> Result<()>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunLoopOptions {
    pub max_steps: usize,
}

impl Default for RunLoopOptions {
    fn default() -> Self {
        Self { max_steps: 100 }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RunLoopStop {
    Terminal,
    StepLimitReached,
}

#[derive(Debug, Clone)]
pub struct RunLoopReport {
    pub run: WorkflowRun,
    pub outcomes: Vec<ExecuteStepOutcome>,
    pub stop: RunLoopStop,
    pub last_event: Option<AutopilotEvent>,
}

/// Load an in-progress run when possible; otherwise start a new run at the first step.
pub fn load_or_start_run(
    workflow: &AutopilotWorkflow,
    project: &str,
    host: Option<String>,
    store: &dyn RunStore,
) -> Result<WorkflowRun> {
    if let Some(run) = store.load_run(project, &workflow.name)? {
        if run.status == WorkflowStatus::Running && run.current_step.is_some() {
            return Ok(run);
        }
    }

    let first_step = workflow.steps.first().ok_or_else(|| {
        ZError::ConfigParse(format!(
            "workflow '{}' has no steps to execute",
            workflow.name
        ))
    })?;
    let mut run = WorkflowRun::new(&workflow.name, project, &first_step.name);
    if let Some(host) = host {
        run = run.with_host(host);
    }
    Ok(run)
}

/// Execute a workflow until it reaches a terminal state or the step limit.
///
/// The run loop owns the ordering-sensitive protocol: load/start, persist,
/// execute, advance, persist again, and notify when a lifecycle event appears.
pub fn execute_workflow_run(
    workflow: &AutopilotWorkflow,
    project: &str,
    host: Option<String>,
    executor: &dyn StepExecutor,
    store: &dyn RunStore,
    notifier: &dyn Notifier,
    options: RunLoopOptions,
) -> Result<RunLoopReport> {
    let mut run = load_or_start_run(workflow, project, host, store)?;
    store.save_run(&run)?;

    let mut outcomes = Vec::new();
    let mut last_event = None;

    while run.status == WorkflowStatus::Running && run.current_step.is_some() {
        if outcomes.len() >= options.max_steps {
            return Ok(RunLoopReport {
                run,
                outcomes,
                stop: RunLoopStop::StepLimitReached,
                last_event,
            });
        }

        let outcome = execute_current_step(workflow, &mut run, executor)?;
        store.save_run(&run)?;
        if let Some(event) = &outcome.advance.event {
            notify_autopilot_event(notifier, event)?;
            last_event = Some(event.clone());
        }
        outcomes.push(outcome);
    }

    Ok(RunLoopReport {
        run,
        outcomes,
        stop: RunLoopStop::Terminal,
        last_event,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::parse_autopilot_workflow;
    use crate::state::StepResult;
    use std::cell::RefCell;
    use z_core::domain::NotifyLevel;

    const WF_KDL: &str = r#"
autopilot "manual-check" {
    trigger "manual"
    step "run" {
        run "cargo test"
        on-success "notify-done"
    }
    step "notify-done" {
        notify "done"
    }
}
"#;

    struct FakeStore {
        loaded: RefCell<Option<WorkflowRun>>,
        saved: RefCell<Vec<WorkflowRun>>,
    }

    impl FakeStore {
        fn empty() -> Self {
            Self {
                loaded: RefCell::new(None),
                saved: RefCell::new(Vec::new()),
            }
        }

        fn with_loaded(run: WorkflowRun) -> Self {
            Self {
                loaded: RefCell::new(Some(run)),
                saved: RefCell::new(Vec::new()),
            }
        }
    }

    impl RunStore for FakeStore {
        fn load_run(&self, _: &str, _: &str) -> Result<Option<WorkflowRun>> {
            Ok(self.loaded.borrow().clone())
        }

        fn save_run(&self, run: &WorkflowRun) -> Result<()> {
            self.saved.borrow_mut().push(run.clone());
            Ok(())
        }
    }

    struct FakeExecutor;

    impl StepExecutor for FakeExecutor {
        fn run_command(&self, _: &str) -> Result<StepResult> {
            Ok(StepResult::Success { output: Some("ok".to_string()) })
        }

        fn notify(&self, _: &str) -> Result<()> {
            Ok(())
        }

        fn confirm(&self, _: &str) -> Result<bool> {
            Ok(true)
        }
    }

    struct FakeNotifier {
        calls: RefCell<Vec<(String, NotifyLevel)>>,
    }

    impl FakeNotifier {
        fn new() -> Self {
            Self { calls: RefCell::new(Vec::new()) }
        }
    }

    impl Notifier for FakeNotifier {
        fn notify(&self, message: &str, level: NotifyLevel) -> Result<()> {
            self.calls.borrow_mut().push((message.to_string(), level));
            Ok(())
        }
    }

    fn workflow() -> AutopilotWorkflow {
        parse_autopilot_workflow(WF_KDL).unwrap()
    }

    #[test]
    fn load_or_start_run_resumes_running_state() {
        let workflow = workflow();
        let existing = WorkflowRun::new("manual-check", "myproject", "notify-done");
        let store = FakeStore::with_loaded(existing.clone());

        let run = load_or_start_run(&workflow, "myproject", None, &store).unwrap();

        assert_eq!(run.current_step, existing.current_step);
    }

    #[test]
    fn execute_workflow_run_persists_each_transition_and_notifies_terminal_event() {
        let workflow = workflow();
        let store = FakeStore::empty();
        let notifier = FakeNotifier::new();

        let report = execute_workflow_run(
            &workflow,
            "myproject",
            None,
            &FakeExecutor,
            &store,
            &notifier,
            RunLoopOptions::default(),
        )
        .unwrap();

        assert_eq!(report.stop, RunLoopStop::Terminal);
        assert_eq!(report.outcomes.len(), 2);
        assert_eq!(report.run.status, WorkflowStatus::Completed);
        assert!(store.saved.borrow().len() >= 3);
        assert_eq!(notifier.calls.borrow().len(), 1);
        assert!(notifier.calls.borrow()[0].0.contains("completed"));
    }

    #[test]
    fn execute_workflow_run_stops_at_step_limit() {
        let workflow = workflow();
        let store = FakeStore::empty();
        let notifier = FakeNotifier::new();

        let report = execute_workflow_run(
            &workflow,
            "myproject",
            None,
            &FakeExecutor,
            &store,
            &notifier,
            RunLoopOptions { max_steps: 1 },
        )
        .unwrap();

        assert_eq!(report.stop, RunLoopStop::StepLimitReached);
        assert_eq!(report.outcomes.len(), 1);
        assert_eq!(report.run.status, WorkflowStatus::Running);
    }
}
