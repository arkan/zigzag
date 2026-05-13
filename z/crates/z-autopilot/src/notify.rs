/// Autopilot notification events and dispatch.
///
/// Provides:
/// - `AutopilotEvent` — the kinds of events an autopilot run can emit
/// - `event_from_advance` — inspect a `WorkflowRun` after `advance()` to get the event
/// - `build_message` — human-readable message for an event
/// - `event_level` — notification severity for an event
/// - `notify_autopilot_event` — dispatch a notification via any `Notifier`
use z_core::domain::NotifyLevel;
use z_core::error::Result;
use z_core::traits::Notifier;

use crate::state::{StepStatus, WorkflowRun, WorkflowStatus};

// ---------------------------------------------------------------------------
// AutopilotEvent
// ---------------------------------------------------------------------------

/// A notable event emitted by an autopilot workflow run.
#[derive(Debug, Clone, PartialEq)]
pub enum AutopilotEvent {
    /// Workflow completed successfully (terminal step with no outgoing transitions).
    Completed {
        workflow_name: String,
        /// Name of the last step that executed.
        final_step: String,
    },
    /// Workflow ended in failure (terminal failure step with no outgoing transitions).
    Failed {
        workflow_name: String,
        /// Name of the last step that executed.
        final_step: String,
    },
    /// Workflow is stuck: a step exhausted its max retries and there is no
    /// `on_max_retries` / `on_failure` / `on_complete` transition to continue.
    Stuck {
        workflow_name: String,
        step_name: String,
        retry_count: u32,
    },
    /// A step exhausted its retries, but the workflow continues via `on_max_retries`.
    MaxRetriesExhausted {
        workflow_name: String,
        step_name: String,
        retry_count: u32,
    },
}

// ---------------------------------------------------------------------------
// event_from_advance
// ---------------------------------------------------------------------------

/// Inspect a `WorkflowRun` *after* calling `advance()` to see if a
/// notification event occurred.
///
/// Returns:
/// - `Some(Completed)` — workflow just completed successfully
/// - `Some(Failed)`    — workflow just failed with no way to continue
/// - `Some(Stuck)`     — max retries exhausted with no available transition
/// - `Some(MaxRetriesExhausted)` — retries exhausted but workflow continues
/// - `None`            — workflow is still running normally
pub fn event_from_advance(run: &WorkflowRun) -> Option<AutopilotEvent> {
    let last_step_name = || {
        run.history
            .last()
            .map(|h| h.step_name.clone())
            .unwrap_or_default()
    };

    match run.status {
        WorkflowStatus::Completed => {
            return Some(AutopilotEvent::Completed {
                workflow_name: run.workflow_name.clone(),
                final_step: last_step_name(),
            });
        }
        WorkflowStatus::Failed => {
            return Some(AutopilotEvent::Failed {
                workflow_name: run.workflow_name.clone(),
                final_step: last_step_name(),
            });
        }
        WorkflowStatus::Stuck => {
            let (step_name, retry_count) = run
                .history
                .last()
                .map(|h| (h.step_name.clone(), h.retry_count))
                .unwrap_or_default();
            return Some(AutopilotEvent::Stuck {
                workflow_name: run.workflow_name.clone(),
                step_name,
                retry_count,
            });
        }
        WorkflowStatus::Running => {}
    }

    // Still running — check whether the last history entry signals that
    // retries were just exhausted (workflow continues via on_max_retries).
    if let Some(last) = run.history.last() {
        if last.status == StepStatus::MaxRetriesExhausted {
            return Some(AutopilotEvent::MaxRetriesExhausted {
                workflow_name: run.workflow_name.clone(),
                step_name: last.step_name.clone(),
                retry_count: last.retry_count,
            });
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Message / level helpers
// ---------------------------------------------------------------------------

/// Build a human-readable notification message for an autopilot event.
///
/// The message includes the workflow name, the step name, and a contextual
/// outcome indicator.
pub fn build_message(event: &AutopilotEvent) -> String {
    match event {
        AutopilotEvent::Completed {
            workflow_name,
            final_step,
        } => format!(
            "Autopilot \"{}\" completed at step \"{}\" \u{2705}",
            workflow_name, final_step
        ),
        AutopilotEvent::Failed {
            workflow_name,
            final_step,
        } => format!(
            "Autopilot \"{}\" failed at step \"{}\" \u{274c}",
            workflow_name, final_step
        ),
        AutopilotEvent::Stuck {
            workflow_name,
            step_name,
            retry_count,
        } => format!(
            "Autopilot \"{}\" stuck at step \"{}\" after {} retries \u{274c}",
            workflow_name, step_name, retry_count
        ),
        AutopilotEvent::MaxRetriesExhausted {
            workflow_name,
            step_name,
            retry_count,
        } => format!(
            "Autopilot \"{}\" step \"{}\" exhausted {} retries \u{26a0}\u{fe0f}",
            workflow_name, step_name, retry_count
        ),
    }
}

/// Return the `NotifyLevel` for an autopilot event.
pub fn event_level(event: &AutopilotEvent) -> NotifyLevel {
    match event {
        AutopilotEvent::Completed { .. } => NotifyLevel::Info,
        AutopilotEvent::Failed { .. } | AutopilotEvent::Stuck { .. } => NotifyLevel::Error,
        AutopilotEvent::MaxRetriesExhausted { .. } => NotifyLevel::Warning,
    }
}

// ---------------------------------------------------------------------------
// notify_autopilot_event
// ---------------------------------------------------------------------------

/// Dispatch a notification for an autopilot event via the given `Notifier`.
///
/// Builds the message and level from the event and calls `notifier.notify()`.
pub fn notify_autopilot_event(notifier: &dyn Notifier, event: &AutopilotEvent) -> Result<()> {
    let message = build_message(event);
    let level = event_level(event);
    notifier.notify(&message, level)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::parse_autopilot_workflow;
    use crate::state::{advance, StepExecution, StepResult, WorkflowRun, WorkflowStatus};
    use std::sync::{Arc, Mutex};
    use z_core::error::ZError;

    // ── Mock notifier ─────────────────────────────────────────────────────

    struct MockNotifier {
        calls: Arc<Mutex<Vec<(String, NotifyLevel)>>>,
    }

    impl MockNotifier {
        fn new() -> (Self, Arc<Mutex<Vec<(String, NotifyLevel)>>>) {
            let calls = Arc::new(Mutex::new(Vec::new()));
            let n = Self {
                calls: Arc::clone(&calls),
            };
            (n, calls)
        }
    }

    impl Notifier for MockNotifier {
        fn notify(&self, message: &str, level: NotifyLevel) -> Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push((message.to_string(), level));
            Ok(())
        }
    }

    // ── Workflow fixture ─────────────────────────────────────────────────

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
        max-retries 3
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

    fn make_run(first_step: &str) -> WorkflowRun {
        WorkflowRun::new("pr-ci-fix", "myproject", first_step)
    }

    // ── event_from_advance: normal running returns None ──────────────────

    #[test]
    fn no_event_when_workflow_still_running() {
        let wf = parse_autopilot_workflow(WF_KDL).unwrap();
        let mut run = make_run("monitor-ci");
        advance(&wf, &mut run, StepResult::Failure { output: None }).unwrap();
        // Transitions to fix-ci — still Running.
        assert_eq!(run.status, WorkflowStatus::Running);
        assert!(event_from_advance(&run).is_none());
    }

    // ── event_from_advance: Completed ────────────────────────────────────

    #[test]
    fn completed_event_when_workflow_completed() {
        let wf = parse_autopilot_workflow(WF_KDL).unwrap();
        let mut run = make_run("notify-done");
        advance(&wf, &mut run, StepResult::Success { output: None }).unwrap();
        assert_eq!(run.status, WorkflowStatus::Completed);

        let event = event_from_advance(&run).expect("should produce Completed event");
        assert_eq!(
            event,
            AutopilotEvent::Completed {
                workflow_name: "pr-ci-fix".to_string(),
                final_step: "notify-done".to_string(),
            }
        );
    }

    // ── event_from_advance: Failed ───────────────────────────────────────

    #[test]
    fn failed_event_when_workflow_failed() {
        let kdl = r#"
autopilot "test" {
    trigger "manual"
    step "only" {
        run "cmd"
    }
}
"#;
        let wf = parse_autopilot_workflow(kdl).unwrap();
        let mut run = WorkflowRun::new("test", "proj", "only");
        advance(&wf, &mut run, StepResult::Failure { output: None }).unwrap();
        assert_eq!(run.status, WorkflowStatus::Failed);

        let event = event_from_advance(&run).expect("should produce Failed event");
        assert_eq!(
            event,
            AutopilotEvent::Failed {
                workflow_name: "test".to_string(),
                final_step: "only".to_string(),
            }
        );
    }

    // ── event_from_advance: Stuck ────────────────────────────────────────

    #[test]
    fn stuck_event_when_workflow_stuck() {
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
        let mut run = WorkflowRun::new("test", "proj", "flaky");
        run.retry_count = 2; // already at max

        advance(&wf, &mut run, StepResult::Failure { output: None }).unwrap();
        assert_eq!(run.status, WorkflowStatus::Stuck);

        let event = event_from_advance(&run).expect("should produce Stuck event");
        assert_eq!(
            event,
            AutopilotEvent::Stuck {
                workflow_name: "test".to_string(),
                step_name: "flaky".to_string(),
                retry_count: 2,
            }
        );
    }

    // ── event_from_advance: MaxRetriesExhausted (workflow continues) ──────

    #[test]
    fn max_retries_exhausted_event_when_workflow_continues() {
        let wf = parse_autopilot_workflow(WF_KDL).unwrap();
        let mut run = make_run("fix-ci");
        run.retry_count = 3; // at max

        // Failure → on_max_retries → notify-stuck (workflow still Running)
        advance(&wf, &mut run, StepResult::Failure { output: None }).unwrap();
        assert_eq!(run.status, WorkflowStatus::Running);
        assert_eq!(run.current_step.as_deref(), Some("notify-stuck"));

        let event = event_from_advance(&run).expect("should produce MaxRetriesExhausted event");
        assert_eq!(
            event,
            AutopilotEvent::MaxRetriesExhausted {
                workflow_name: "pr-ci-fix".to_string(),
                step_name: "fix-ci".to_string(),
                retry_count: 3,
            }
        );
    }

    // ── event_from_advance: fresh run (no history) returns None ──────────

    #[test]
    fn no_event_for_fresh_run() {
        let run = make_run("monitor-ci");
        assert!(event_from_advance(&run).is_none());
    }

    // ── build_message ─────────────────────────────────────────────────────

    #[test]
    fn build_message_completed() {
        let event = AutopilotEvent::Completed {
            workflow_name: "my-wf".to_string(),
            final_step: "deploy".to_string(),
        };
        let msg = build_message(&event);
        assert!(msg.contains("my-wf"), "should include workflow name");
        assert!(msg.contains("deploy"), "should include final step");
        assert!(msg.contains('\u{2705}'), "should include ✅");
    }

    #[test]
    fn build_message_failed() {
        let event = AutopilotEvent::Failed {
            workflow_name: "my-wf".to_string(),
            final_step: "test".to_string(),
        };
        let msg = build_message(&event);
        assert!(msg.contains("my-wf"));
        assert!(msg.contains("test"));
        assert!(msg.contains('\u{274c}'), "should include ❌");
    }

    #[test]
    fn build_message_stuck() {
        let event = AutopilotEvent::Stuck {
            workflow_name: "my-wf".to_string(),
            step_name: "fix-ci".to_string(),
            retry_count: 3,
        };
        let msg = build_message(&event);
        assert!(msg.contains("my-wf"));
        assert!(msg.contains("fix-ci"));
        assert!(msg.contains('3'), "should include retry count");
        assert!(msg.contains('\u{274c}'), "should include ❌");
    }

    #[test]
    fn build_message_max_retries_exhausted() {
        let event = AutopilotEvent::MaxRetriesExhausted {
            workflow_name: "my-wf".to_string(),
            step_name: "fix-ci".to_string(),
            retry_count: 2,
        };
        let msg = build_message(&event);
        assert!(msg.contains("my-wf"));
        assert!(msg.contains("fix-ci"));
        assert!(msg.contains('2'), "should include retry count");
    }

    // ── event_level ───────────────────────────────────────────────────────

    #[test]
    fn completed_level_is_info() {
        let event = AutopilotEvent::Completed {
            workflow_name: "w".into(),
            final_step: "s".into(),
        };
        assert_eq!(event_level(&event), NotifyLevel::Info);
    }

    #[test]
    fn failed_level_is_error() {
        let event = AutopilotEvent::Failed {
            workflow_name: "w".into(),
            final_step: "s".into(),
        };
        assert_eq!(event_level(&event), NotifyLevel::Error);
    }

    #[test]
    fn stuck_level_is_error() {
        let event = AutopilotEvent::Stuck {
            workflow_name: "w".into(),
            step_name: "s".into(),
            retry_count: 1,
        };
        assert_eq!(event_level(&event), NotifyLevel::Error);
    }

    #[test]
    fn max_retries_level_is_warning() {
        let event = AutopilotEvent::MaxRetriesExhausted {
            workflow_name: "w".into(),
            step_name: "s".into(),
            retry_count: 1,
        };
        assert_eq!(event_level(&event), NotifyLevel::Warning);
    }

    // ── notify_autopilot_event ────────────────────────────────────────────

    #[test]
    fn notify_dispatches_completed_message_at_info_level() {
        let (notifier, calls) = MockNotifier::new();
        let event = AutopilotEvent::Completed {
            workflow_name: "deploy-wf".to_string(),
            final_step: "push".to_string(),
        };
        notify_autopilot_event(&notifier, &event).unwrap();

        let c = calls.lock().unwrap();
        assert_eq!(c.len(), 1);
        assert!(c[0].0.contains("deploy-wf"));
        assert!(c[0].0.contains("push"));
        assert_eq!(c[0].1, NotifyLevel::Info);
    }

    #[test]
    fn notify_dispatches_failed_message_at_error_level() {
        let (notifier, calls) = MockNotifier::new();
        let event = AutopilotEvent::Failed {
            workflow_name: "ci-wf".to_string(),
            final_step: "build".to_string(),
        };
        notify_autopilot_event(&notifier, &event).unwrap();

        let c = calls.lock().unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].1, NotifyLevel::Error);
    }

    #[test]
    fn notify_dispatches_stuck_message_at_error_level() {
        let (notifier, calls) = MockNotifier::new();
        let event = AutopilotEvent::Stuck {
            workflow_name: "ci-wf".to_string(),
            step_name: "fix".to_string(),
            retry_count: 3,
        };
        notify_autopilot_event(&notifier, &event).unwrap();

        let c = calls.lock().unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].1, NotifyLevel::Error);
        assert!(c[0].0.contains('3'));
    }

    #[test]
    fn notify_dispatches_max_retries_at_warning_level() {
        let (notifier, calls) = MockNotifier::new();
        let event = AutopilotEvent::MaxRetriesExhausted {
            workflow_name: "ci-wf".to_string(),
            step_name: "fix".to_string(),
            retry_count: 2,
        };
        notify_autopilot_event(&notifier, &event).unwrap();

        let c = calls.lock().unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].1, NotifyLevel::Warning);
    }

    // ── Full integration: advance → event_from_advance → notify ──────────

    #[test]
    fn full_flow_workflow_completion_triggers_notification() {
        let wf = parse_autopilot_workflow(WF_KDL).unwrap();
        let mut run = make_run("notify-done");

        // Step executes successfully → workflow Completed.
        advance(&wf, &mut run, StepResult::Success { output: None }).unwrap();

        let event = event_from_advance(&run).expect("should produce event");
        let (notifier, calls) = MockNotifier::new();
        notify_autopilot_event(&notifier, &event).unwrap();

        let c = calls.lock().unwrap();
        assert_eq!(c.len(), 1);
        assert!(c[0].0.contains("pr-ci-fix"));
        assert_eq!(c[0].1, NotifyLevel::Info);
    }

    #[test]
    fn full_flow_max_retries_then_terminal_step() {
        let wf = parse_autopilot_workflow(WF_KDL).unwrap();
        let mut run = make_run("fix-ci");
        run.retry_count = 3;

        // Max retries exhausted → transitions to notify-stuck, still Running.
        advance(&wf, &mut run, StepResult::Failure { output: None }).unwrap();
        let event = event_from_advance(&run).expect("should produce MaxRetriesExhausted event");
        assert!(matches!(event, AutopilotEvent::MaxRetriesExhausted { .. }));

        // notify-stuck completes → workflow Completed.
        let next_step = advance(&wf, &mut run, StepResult::Success { output: None }).unwrap();
        assert!(next_step.is_none()); // notify-stuck is terminal
        let event2 = event_from_advance(&run).expect("should produce Completed event");
        assert!(matches!(event2, AutopilotEvent::Completed { .. }));
    }

    // ── Retry counting in events is accurate ─────────────────────────────

    #[test]
    fn stuck_event_includes_correct_retry_count() {
        let kdl = r#"
autopilot "test" {
    trigger "manual"
    step "step1" {
        run "cmd"
        max-retries 5
    }
}
"#;
        let wf = parse_autopilot_workflow(kdl).unwrap();
        let mut run = WorkflowRun::new("test", "proj", "step1");
        run.retry_count = 5;

        advance(&wf, &mut run, StepResult::Failure { output: None }).unwrap();

        if let Some(AutopilotEvent::Stuck { retry_count, .. }) = event_from_advance(&run) {
            assert_eq!(retry_count, 5);
        } else {
            panic!("expected Stuck event");
        }
    }

    // ── Notifier error is propagated ──────────────────────────────────────

    #[test]
    fn notify_propagates_notifier_error() {
        struct FailingNotifier;
        impl Notifier for FailingNotifier {
            fn notify(&self, _message: &str, _level: NotifyLevel) -> Result<()> {
                Err(ZError::Io("mock failure".into()))
            }
        }
        let event = AutopilotEvent::Completed {
            workflow_name: "w".into(),
            final_step: "s".into(),
        };
        let result = notify_autopilot_event(&FailingNotifier, &event);
        assert!(result.is_err());
    }

    // ── Edge case: Stuck status with empty history ──────────────────────

    #[test]
    fn stuck_event_with_empty_history_returns_defaults() {
        // Stuck status but no history entries (e.g. deserialized corrupt state).
        // Should still return Some(Stuck) with default step_name and retry_count.
        let mut run = make_run("monitor-ci");
        run.status = WorkflowStatus::Stuck;
        run.current_step = None;

        let event =
            event_from_advance(&run).expect("should produce Stuck event even with empty history");
        assert_eq!(
            event,
            AutopilotEvent::Stuck {
                workflow_name: "pr-ci-fix".to_string(),
                step_name: String::new(),
                retry_count: 0,
            }
        );
    }

    // ── Edge case: Completed/Failed with empty history ──────────────────

    #[test]
    fn completed_event_with_empty_history_has_empty_final_step() {
        let mut run = make_run("monitor-ci");
        run.status = WorkflowStatus::Completed;
        run.current_step = None;

        let event = event_from_advance(&run).expect("should produce Completed event");
        assert_eq!(
            event,
            AutopilotEvent::Completed {
                workflow_name: "pr-ci-fix".to_string(),
                final_step: String::new(),
            }
        );
    }

    #[test]
    fn failed_event_with_empty_history_has_empty_final_step() {
        let mut run = make_run("monitor-ci");
        run.status = WorkflowStatus::Failed;
        run.current_step = None;

        let event = event_from_advance(&run).expect("should produce Failed event");
        assert_eq!(
            event,
            AutopilotEvent::Failed {
                workflow_name: "pr-ci-fix".to_string(),
                final_step: String::new(),
            }
        );
    }

    // ── Edge case: empty / special-character workflow/step names ─────────

    #[test]
    fn build_message_with_empty_names() {
        let event = AutopilotEvent::Completed {
            workflow_name: String::new(),
            final_step: String::new(),
        };
        let msg = build_message(&event);
        assert!(
            msg.contains("\"\""),
            "empty names should appear as quoted empty strings"
        );
    }

    #[test]
    fn build_message_with_quotes_in_names() {
        let event = AutopilotEvent::Failed {
            workflow_name: "wf with \"quotes\"".to_string(),
            final_step: "step \"x\"".to_string(),
        };
        let msg = build_message(&event);
        assert!(msg.contains("wf with \"quotes\""));
        assert!(msg.contains("step \"x\""));
    }

    // ── Edge case: zero retry count in messages ─────────────────────────

    #[test]
    fn build_message_stuck_zero_retries() {
        let event = AutopilotEvent::Stuck {
            workflow_name: "w".into(),
            step_name: "s".into(),
            retry_count: 0,
        };
        let msg = build_message(&event);
        assert!(msg.contains("0 retries"));
    }

    #[test]
    fn build_message_max_retries_zero_retries() {
        let event = AutopilotEvent::MaxRetriesExhausted {
            workflow_name: "w".into(),
            step_name: "s".into(),
            retry_count: 0,
        };
        let msg = build_message(&event);
        assert!(msg.contains("0 retries"));
    }

    // ── Edge case: Running with non-MaxRetriesExhausted last entry ──────

    #[test]
    fn no_event_when_running_and_last_entry_is_succeeded() {
        let mut run = make_run("monitor-ci");
        run.history.push(StepExecution {
            step_name: "monitor-ci".into(),
            status: StepStatus::Succeeded,
            retry_count: 0,
            output: None,
        });
        assert!(event_from_advance(&run).is_none());
    }

    #[test]
    fn no_event_when_running_and_last_entry_is_failed() {
        let mut run = make_run("monitor-ci");
        run.history.push(StepExecution {
            step_name: "monitor-ci".into(),
            status: StepStatus::Failed,
            retry_count: 1,
            output: None,
        });
        assert!(event_from_advance(&run).is_none());
    }

    // ── Edge case: multiple advance calls — MaxRetriesExhausted is lost ──

    #[test]
    fn max_retries_event_lost_after_second_advance() {
        let wf = parse_autopilot_workflow(WF_KDL).unwrap();
        let mut run = make_run("fix-ci");
        run.retry_count = 3;

        // First advance: max retries exhausted → transitions to notify-stuck.
        advance(&wf, &mut run, StepResult::Failure { output: None }).unwrap();
        let event = event_from_advance(&run);
        assert!(matches!(
            event,
            Some(AutopilotEvent::MaxRetriesExhausted { .. })
        ));

        // Second advance: notify-stuck completes → workflow Completed.
        advance(&wf, &mut run, StepResult::Success { output: None }).unwrap();
        // The MaxRetriesExhausted event is now gone — last history entry is notify-stuck.
        let event = event_from_advance(&run);
        assert!(
            matches!(event, Some(AutopilotEvent::Completed { .. })),
            "after second advance, MaxRetriesExhausted should not persist"
        );
    }

    // ── Edge case: AutopilotEvent Clone and PartialEq ────────────────────

    #[test]
    fn autopilot_event_clone_and_eq() {
        let event = AutopilotEvent::MaxRetriesExhausted {
            workflow_name: "w".into(),
            step_name: "s".into(),
            retry_count: 42,
        };
        let cloned = event.clone();
        assert_eq!(event, cloned);
    }
}
