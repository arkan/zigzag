//! Built-in autopilot workflow templates bundled with the binary.
//!
//! These are the 6 production workflows defined in docs/SPECS.md §9.3.
//! Each is parsed and validated at load time via [`builtin_workflows`].

use z_core::error::Result;
use crate::dsl::{parse_autopilot_workflow, validate_workflow, AutopilotWorkflow};

// ── KDL sources ─────────────────────────────────────────────────────────────

/// Monitor CI, fix failures with Claude, retry (max 3 attempts).
pub const PR_CI_FIX_KDL: &str = r#"autopilot "pr-ci-fix" {
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
}"#;

/// Resolve PR review comments with Claude, then push.
pub const PR_REVIEW_FIX_KDL: &str = r#"autopilot "pr-review-fix" {
    description "Resolve PR review comments with Claude"
    trigger "pr-review-received"

    step "fix-comments" {
        run "claude 'Resolve all PR review comments: $(gh pr view --json reviews)'"
        on-complete "push-fixes"
    }

    step "push-fixes" {
        run "git push"
        on-complete "notify-done"
    }

    step "notify-done" {
        notify "PR review comments resolved ✅"
    }
}"#;

/// Auto-merge when PR approved + CI green, then clean up session/worktree.
pub const PR_MERGE_WHEN_READY_KDL: &str = r#"autopilot "pr-merge-when-ready" {
    description "Auto-merge when PR approved + CI green"
    trigger "pr-approved"

    step "wait-ci" {
        run "gh run watch --exit-status"
        on-success "merge"
        on-failure "notify-ci-fail"
    }

    step "merge" {
        run "gh pr merge --squash --delete-branch"
        on-complete "cleanup"
    }

    step "cleanup" {
        run "z delete {session}"
        on-complete "notify-done"
    }

    step "notify-done" {
        notify "PR merged and cleaned up ✅"
    }

    step "notify-ci-fail" {
        notify "PR approved but CI failing ❌"
    }
}"#;

/// Auto-merge Dependabot PRs if tests pass.
pub const DEPENDABOT_AUTO_KDL: &str = r#"autopilot "dependabot-auto" {
    description "Auto-merge Dependabot PRs if tests pass"
    trigger "pr-opened-by-dependabot"

    step "run-tests" {
        run "gh run watch --exit-status"
        on-success "merge"
        on-failure "notify-fail"
    }

    step "merge" {
        run "gh pr merge --squash --delete-branch"
        on-complete "notify-done"
    }

    step "notify-done" {
        notify "Dependabot PR merged ✅"
    }

    step "notify-fail" {
        notify "Dependabot PR failing ❌ — review needed"
    }
}"#;

/// Monitor deploy after merge to main; rollback on failure.
pub const DEPLOY_WATCH_KDL: &str = r#"autopilot "deploy-watch" {
    description "Monitor deploy after merge, rollback if error"
    trigger "post-merge-main"

    step "monitor-deploy" {
        run "deploy_command --status"
        timeout "10m"
        on-success "notify-done"
        on-failure "rollback"
    }

    step "rollback" {
        run "deploy_command --rollback"
        on-complete "notify-rollback"
    }

    step "notify-done" {
        notify "Deploy successful ✅"
    }

    step "notify-rollback" {
        notify "Deploy failed, rolled back ⚠️"
    }
}"#;

/// Poll main for new commits, confirm with user, then deploy.
pub const DEPLOY_SYNC_KDL: &str = r#"autopilot "deploy-sync" {
    description "Pull main changes, confirm, deploy"
    trigger "new-commits-on-main"
    poll-interval "5m"

    step "pull" {
        run "git pull origin main"
        on-complete "diff-summary"
    }

    step "diff-summary" {
        run "git log --oneline @{1}..HEAD"
        on-complete "confirm-deploy"
    }

    step "confirm-deploy" {
        confirm "Deploy these changes?"
        on-accept "deploy"
        on-reject "notify-skipped"
    }

    step "deploy" {
        run "deploy_command"
        on-success "notify-done"
        on-failure "notify-fail"
    }

    step "notify-done" {
        notify "Deploy successful ✅"
    }

    step "notify-skipped" {
        notify "Deploy skipped by user"
    }

    step "notify-fail" {
        notify "Deploy failed ❌"
    }
}"#;

// ── Public API ───────────────────────────────────────────────────────────────

/// Parse and validate all 6 built-in workflows.
///
/// Returns them in canonical order:
/// `pr-ci-fix`, `pr-review-fix`, `pr-merge-when-ready`,
/// `dependabot-auto`, `deploy-watch`, `deploy-sync`.
pub fn builtin_workflows() -> Result<Vec<AutopilotWorkflow>> {
    let sources = [
        PR_CI_FIX_KDL,
        PR_REVIEW_FIX_KDL,
        PR_MERGE_WHEN_READY_KDL,
        DEPENDABOT_AUTO_KDL,
        DEPLOY_WATCH_KDL,
        DEPLOY_SYNC_KDL,
    ];
    let mut workflows = Vec::with_capacity(sources.len());
    for src in sources {
        let wf = parse_autopilot_workflow(src)?;
        validate_workflow(&wf)?;
        workflows.push(wf);
    }
    Ok(workflows)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::{StepAction, Trigger};

    // ── builtin_workflows() ──────────────────────────────────────────────────

    #[test]
    fn test_all_builtin_workflows_parse_and_validate() {
        let wfs = builtin_workflows().expect("all builtins must parse and validate");
        assert_eq!(wfs.len(), 6);
    }

    #[test]
    fn test_builtin_workflow_names() {
        let wfs = builtin_workflows().unwrap();
        let names: Vec<&str> = wfs.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(
            names,
            ["pr-ci-fix", "pr-review-fix", "pr-merge-when-ready",
             "dependabot-auto", "deploy-watch", "deploy-sync"]
        );
    }

    // ── pr-ci-fix ────────────────────────────────────────────────────────────

    #[test]
    fn test_pr_ci_fix_trigger_and_steps() {
        let wf = parse_autopilot_workflow(PR_CI_FIX_KDL).unwrap();
        assert_eq!(wf.trigger, Trigger::PostPush);
        assert_eq!(wf.steps.len(), 4);
    }

    #[test]
    fn test_pr_ci_fix_monitor_ci_step() {
        let wf = parse_autopilot_workflow(PR_CI_FIX_KDL).unwrap();
        let step = &wf.steps[0];
        assert_eq!(step.name, "monitor-ci");
        assert_eq!(step.action, StepAction::Run { command: "gh run watch --exit-status".into() });
        assert_eq!(step.on_failure.as_deref(), Some("fix-ci"));
        assert_eq!(step.on_success.as_deref(), Some("notify-done"));
        assert!(step.on_complete.is_none());
    }

    #[test]
    fn test_pr_ci_fix_fix_ci_max_retries() {
        let wf = parse_autopilot_workflow(PR_CI_FIX_KDL).unwrap();
        let step = &wf.steps[1];
        assert_eq!(step.name, "fix-ci");
        assert_eq!(step.max_retries, Some(3));
        assert_eq!(step.on_complete.as_deref(), Some("monitor-ci"));
        assert_eq!(step.on_max_retries.as_deref(), Some("notify-stuck"));
    }

    #[test]
    fn test_pr_ci_fix_notify_steps() {
        let wf = parse_autopilot_workflow(PR_CI_FIX_KDL).unwrap();
        assert_eq!(wf.steps[2].name, "notify-done");
        assert_eq!(wf.steps[3].name, "notify-stuck");
        assert!(matches!(wf.steps[2].action, StepAction::Notify { .. }));
        assert!(matches!(wf.steps[3].action, StepAction::Notify { .. }));
    }

    #[test]
    fn test_pr_ci_fix_validates() {
        let wf = parse_autopilot_workflow(PR_CI_FIX_KDL).unwrap();
        assert!(validate_workflow(&wf).is_ok());
    }

    // ── pr-review-fix ────────────────────────────────────────────────────────

    #[test]
    fn test_pr_review_fix_trigger_and_steps() {
        let wf = parse_autopilot_workflow(PR_REVIEW_FIX_KDL).unwrap();
        assert_eq!(wf.trigger, Trigger::PrReviewReceived);
        assert_eq!(wf.steps.len(), 3);
    }

    #[test]
    fn test_pr_review_fix_step_sequence() {
        let wf = parse_autopilot_workflow(PR_REVIEW_FIX_KDL).unwrap();
        assert_eq!(wf.steps[0].name, "fix-comments");
        assert_eq!(wf.steps[0].on_complete.as_deref(), Some("push-fixes"));
        assert_eq!(wf.steps[1].name, "push-fixes");
        assert_eq!(wf.steps[1].on_complete.as_deref(), Some("notify-done"));
        assert_eq!(wf.steps[2].name, "notify-done");
    }

    #[test]
    fn test_pr_review_fix_fix_comments_runs_claude() {
        let wf = parse_autopilot_workflow(PR_REVIEW_FIX_KDL).unwrap();
        let step = &wf.steps[0];
        if let StepAction::Run { command } = &step.action {
            assert!(command.contains("claude"), "fix-comments must invoke claude");
            assert!(command.contains("gh pr view"), "fix-comments must fetch PR reviews");
        } else {
            panic!("fix-comments must be a run step");
        }
    }

    #[test]
    fn test_pr_review_fix_validates() {
        let wf = parse_autopilot_workflow(PR_REVIEW_FIX_KDL).unwrap();
        assert!(validate_workflow(&wf).is_ok());
    }

    // ── pr-merge-when-ready ──────────────────────────────────────────────────

    #[test]
    fn test_pr_merge_when_ready_trigger_and_steps() {
        let wf = parse_autopilot_workflow(PR_MERGE_WHEN_READY_KDL).unwrap();
        assert_eq!(wf.trigger, Trigger::PrApproved);
        assert_eq!(wf.steps.len(), 5);
    }

    #[test]
    fn test_pr_merge_when_ready_wait_ci_transitions() {
        let wf = parse_autopilot_workflow(PR_MERGE_WHEN_READY_KDL).unwrap();
        let step = &wf.steps[0];
        assert_eq!(step.name, "wait-ci");
        assert_eq!(step.on_success.as_deref(), Some("merge"));
        assert_eq!(step.on_failure.as_deref(), Some("notify-ci-fail"));
    }

    #[test]
    fn test_pr_merge_when_ready_merge_uses_squash() {
        let wf = parse_autopilot_workflow(PR_MERGE_WHEN_READY_KDL).unwrap();
        let step = &wf.steps[1];
        assert_eq!(step.name, "merge");
        if let StepAction::Run { command } = &step.action {
            assert!(command.contains("--squash"), "merge must use squash");
            assert!(command.contains("--delete-branch"), "merge must delete branch");
        } else {
            panic!("merge must be a run step");
        }
        assert_eq!(step.on_complete.as_deref(), Some("cleanup"));
    }

    #[test]
    fn test_pr_merge_when_ready_cleanup_step() {
        let wf = parse_autopilot_workflow(PR_MERGE_WHEN_READY_KDL).unwrap();
        let step = &wf.steps[2];
        assert_eq!(step.name, "cleanup");
        assert_eq!(step.on_complete.as_deref(), Some("notify-done"));
    }

    #[test]
    fn test_pr_merge_when_ready_validates() {
        let wf = parse_autopilot_workflow(PR_MERGE_WHEN_READY_KDL).unwrap();
        assert!(validate_workflow(&wf).is_ok());
    }

    // ── dependabot-auto ──────────────────────────────────────────────────────

    #[test]
    fn test_dependabot_auto_trigger_and_steps() {
        let wf = parse_autopilot_workflow(DEPENDABOT_AUTO_KDL).unwrap();
        assert_eq!(wf.trigger, Trigger::PrOpenedByDependabot);
        assert_eq!(wf.steps.len(), 4);
    }

    #[test]
    fn test_dependabot_auto_run_tests_transitions() {
        let wf = parse_autopilot_workflow(DEPENDABOT_AUTO_KDL).unwrap();
        let step = &wf.steps[0];
        assert_eq!(step.name, "run-tests");
        assert_eq!(step.on_success.as_deref(), Some("merge"));
        assert_eq!(step.on_failure.as_deref(), Some("notify-fail"));
    }

    #[test]
    fn test_dependabot_auto_merge_step() {
        let wf = parse_autopilot_workflow(DEPENDABOT_AUTO_KDL).unwrap();
        let step = &wf.steps[1];
        assert_eq!(step.name, "merge");
        assert_eq!(step.on_complete.as_deref(), Some("notify-done"));
    }

    #[test]
    fn test_dependabot_auto_validates() {
        let wf = parse_autopilot_workflow(DEPENDABOT_AUTO_KDL).unwrap();
        assert!(validate_workflow(&wf).is_ok());
    }

    // ── deploy-watch ─────────────────────────────────────────────────────────

    #[test]
    fn test_deploy_watch_trigger_and_steps() {
        let wf = parse_autopilot_workflow(DEPLOY_WATCH_KDL).unwrap();
        assert_eq!(wf.trigger, Trigger::PostMergeMain);
        assert_eq!(wf.steps.len(), 4);
    }

    #[test]
    fn test_deploy_watch_monitor_deploy_timeout() {
        let wf = parse_autopilot_workflow(DEPLOY_WATCH_KDL).unwrap();
        let step = &wf.steps[0];
        assert_eq!(step.name, "monitor-deploy");
        assert_eq!(step.timeout.as_deref(), Some("10m"));
        assert_eq!(step.on_success.as_deref(), Some("notify-done"));
        assert_eq!(step.on_failure.as_deref(), Some("rollback"));
    }

    #[test]
    fn test_deploy_watch_rollback_step() {
        let wf = parse_autopilot_workflow(DEPLOY_WATCH_KDL).unwrap();
        let step = &wf.steps[1];
        assert_eq!(step.name, "rollback");
        assert_eq!(step.on_complete.as_deref(), Some("notify-rollback"));
    }

    #[test]
    fn test_deploy_watch_validates() {
        let wf = parse_autopilot_workflow(DEPLOY_WATCH_KDL).unwrap();
        assert!(validate_workflow(&wf).is_ok());
    }

    // ── deploy-sync ──────────────────────────────────────────────────────────

    #[test]
    fn test_deploy_sync_trigger_poll_interval() {
        let wf = parse_autopilot_workflow(DEPLOY_SYNC_KDL).unwrap();
        assert_eq!(wf.trigger, Trigger::NewCommitsOnMain);
        assert_eq!(wf.poll_interval.as_deref(), Some("5m"));
        assert_eq!(wf.steps.len(), 7);
    }

    #[test]
    fn test_deploy_sync_pull_step() {
        let wf = parse_autopilot_workflow(DEPLOY_SYNC_KDL).unwrap();
        let step = &wf.steps[0];
        assert_eq!(step.name, "pull");
        assert_eq!(step.on_complete.as_deref(), Some("diff-summary"));
    }

    #[test]
    fn test_deploy_sync_confirm_step() {
        let wf = parse_autopilot_workflow(DEPLOY_SYNC_KDL).unwrap();
        let step = &wf.steps[2];
        assert_eq!(step.name, "confirm-deploy");
        assert_eq!(step.action, StepAction::Confirm { prompt: "Deploy these changes?".into() });
        assert_eq!(step.on_accept.as_deref(), Some("deploy"));
        assert_eq!(step.on_reject.as_deref(), Some("notify-skipped"));
    }

    #[test]
    fn test_deploy_sync_deploy_step() {
        let wf = parse_autopilot_workflow(DEPLOY_SYNC_KDL).unwrap();
        let step = &wf.steps[3];
        assert_eq!(step.name, "deploy");
        assert_eq!(step.on_success.as_deref(), Some("notify-done"));
        assert_eq!(step.on_failure.as_deref(), Some("notify-fail"));
    }

    #[test]
    fn test_deploy_sync_validates() {
        let wf = parse_autopilot_workflow(DEPLOY_SYNC_KDL).unwrap();
        assert!(validate_workflow(&wf).is_ok());
    }

    // ── integration: trigger matching ────────────────────────────────────────

    #[test]
    fn test_builtin_workflows_trigger_matching() {
        use crate::trigger::{TriggerEvent, matching_workflows};
        let wfs = builtin_workflows().unwrap();

        // post-push matches pr-ci-fix
        let matches = matching_workflows(&wfs, &TriggerEvent::PostPush);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "pr-ci-fix");

        // pr-review-received matches pr-review-fix
        let matches = matching_workflows(&wfs, &TriggerEvent::PrReviewReceived);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "pr-review-fix");

        // pr-approved matches pr-merge-when-ready
        let matches = matching_workflows(&wfs, &TriggerEvent::PrApproved);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "pr-merge-when-ready");

        // pr-opened-by-dependabot matches dependabot-auto
        let matches = matching_workflows(&wfs, &TriggerEvent::PrOpenedByDependabot);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "dependabot-auto");

        // post-merge-main matches deploy-watch
        let matches = matching_workflows(&wfs, &TriggerEvent::PostMergeMain);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "deploy-watch");

        // new-commits-on-main matches deploy-sync
        let matches = matching_workflows(&wfs, &TriggerEvent::NewCommitsOnMain);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "deploy-sync");
    }

    #[test]
    fn test_manual_trigger_matches_none_of_builtins() {
        use crate::trigger::{TriggerEvent, matching_workflows};
        let wfs = builtin_workflows().unwrap();
        // A manual event for an unknown name should match no builtin (none use manual trigger).
        let matches = matching_workflows(&wfs, &TriggerEvent::Manual { workflow_name: "no-such-workflow".into() });
        assert_eq!(matches.len(), 0, "no builtin workflow has manual trigger");
    }

    // ── integration: state machine steps ────────────────────────────────────

    fn make_run(wf: &AutopilotWorkflow) -> crate::state::WorkflowRun {
        crate::state::WorkflowRun::new(
            &wf.name,
            "test-project",
            wf.steps[0].name.as_str(),
        )
    }

    #[test]
    fn test_pr_ci_fix_step_transitions_via_state_machine() {
        use crate::state::{WorkflowStatus, StepResult, advance};
        let wf = parse_autopilot_workflow(PR_CI_FIX_KDL).unwrap();
        let mut run = make_run(&wf);

        // monitor-ci succeeds → notify-done
        let next = advance(&wf, &mut run, StepResult::Success { output: None }).unwrap();
        assert_eq!(next.as_deref(), Some("notify-done"));

        // notify-done (terminal) → workflow completes
        let next = advance(&wf, &mut run, StepResult::Success { output: None }).unwrap();
        assert!(next.is_none());
        assert_eq!(run.status, WorkflowStatus::Completed);
    }

    #[test]
    fn test_pr_ci_fix_retry_loop_via_state_machine() {
        use crate::state::{StepResult, advance};
        let wf = parse_autopilot_workflow(PR_CI_FIX_KDL).unwrap();
        let mut run = make_run(&wf);

        // monitor-ci fails → fix-ci
        let next = advance(&wf, &mut run, StepResult::Failure { output: None }).unwrap();
        assert_eq!(next.as_deref(), Some("fix-ci"));

        // fix-ci succeeds → on-complete → monitor-ci (retry loop)
        let next = advance(&wf, &mut run, StepResult::Success { output: None }).unwrap();
        assert_eq!(next.as_deref(), Some("monitor-ci"));
    }

    #[test]
    fn test_pr_ci_fix_max_retries_exhausted() {
        use crate::state::{StepResult, advance};
        let wf = parse_autopilot_workflow(PR_CI_FIX_KDL).unwrap();
        let mut run = make_run(&wf);

        // monitor-ci fails → fix-ci
        advance(&wf, &mut run, StepResult::Failure { output: None }).unwrap();

        // fix-ci has max-retries 3: first 3 failures retry in place (retry_count 0→1→2→3),
        // 4th failure exhausts retries → on-max-retries → notify-stuck
        for _ in 0..4 {
            advance(&wf, &mut run, StepResult::Failure { output: None }).unwrap();
        }
        assert_eq!(run.current_step.as_deref(), Some("notify-stuck"));
    }

    #[test]
    fn test_deploy_sync_confirm_reject_path() {
        use crate::state::{WorkflowStatus, StepResult, advance};
        let wf = parse_autopilot_workflow(DEPLOY_SYNC_KDL).unwrap();
        let mut run = make_run(&wf);

        // pull → diff-summary
        let next = advance(&wf, &mut run, StepResult::Success { output: None }).unwrap();
        assert_eq!(next.as_deref(), Some("diff-summary"));

        // diff-summary → confirm-deploy
        let next = advance(&wf, &mut run, StepResult::Success { output: None }).unwrap();
        assert_eq!(next.as_deref(), Some("confirm-deploy"));

        // confirm-deploy rejected (Failure) → on-reject → notify-skipped
        let next = advance(&wf, &mut run, StepResult::Failure { output: None }).unwrap();
        assert_eq!(next.as_deref(), Some("notify-skipped"));

        // notify-skipped (terminal) → completed
        let next = advance(&wf, &mut run, StepResult::Success { output: None }).unwrap();
        assert!(next.is_none());
        assert_eq!(run.status, WorkflowStatus::Completed);
    }
}
