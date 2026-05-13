use crate::dsl::{AutopilotWorkflow, Trigger};

/// An event that can activate one or more workflows.
#[derive(Debug, Clone, PartialEq)]
pub enum TriggerEvent {
    PostPush,
    PrApproved,
    PrReviewReceived,
    PrOpenedByDependabot,
    PostMergeMain,
    NewCommitsOnMain,
    /// Manually trigger a specific workflow by name.
    Manual {
        workflow_name: String,
    },
}

/// Returns `true` if the workflow's trigger matches the given event.
pub fn matches_trigger(workflow: &AutopilotWorkflow, event: &TriggerEvent) -> bool {
    match (&workflow.trigger, event) {
        (Trigger::PostPush, TriggerEvent::PostPush) => true,
        (Trigger::PrApproved, TriggerEvent::PrApproved) => true,
        (Trigger::PrReviewReceived, TriggerEvent::PrReviewReceived) => true,
        (Trigger::PrOpenedByDependabot, TriggerEvent::PrOpenedByDependabot) => true,
        (Trigger::PostMergeMain, TriggerEvent::PostMergeMain) => true,
        (Trigger::NewCommitsOnMain, TriggerEvent::NewCommitsOnMain) => true,
        (Trigger::Manual, TriggerEvent::Manual { workflow_name }) => {
            &workflow.name == workflow_name
        }
        _ => false,
    }
}

/// Filter a list of workflows to those that match the given event.
pub fn matching_workflows<'a>(
    workflows: &'a [AutopilotWorkflow],
    event: &TriggerEvent,
) -> Vec<&'a AutopilotWorkflow> {
    workflows
        .iter()
        .filter(|wf| matches_trigger(wf, event))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::parse_autopilot_workflow;

    fn make_workflow(trigger: &str) -> AutopilotWorkflow {
        let kdl = format!(
            "autopilot \"test-wf\" {{\n    trigger \"{trigger}\"\n    step \"s\" {{\n        notify \"hi\"\n    }}\n}}\n"
        );
        parse_autopilot_workflow(&kdl).unwrap()
    }

    #[test]
    fn test_post_push_matches() {
        let wf = make_workflow("post-push");
        assert!(matches_trigger(&wf, &TriggerEvent::PostPush));
        assert!(!matches_trigger(&wf, &TriggerEvent::PrApproved));
    }

    #[test]
    fn test_pr_approved_matches() {
        let wf = make_workflow("pr-approved");
        assert!(matches_trigger(&wf, &TriggerEvent::PrApproved));
        assert!(!matches_trigger(&wf, &TriggerEvent::PostPush));
    }

    #[test]
    fn test_pr_review_received_matches() {
        let wf = make_workflow("pr-review-received");
        assert!(matches_trigger(&wf, &TriggerEvent::PrReviewReceived));
    }

    #[test]
    fn test_pr_opened_by_dependabot_matches() {
        let wf = make_workflow("pr-opened-by-dependabot");
        assert!(matches_trigger(&wf, &TriggerEvent::PrOpenedByDependabot));
    }

    #[test]
    fn test_post_merge_main_matches() {
        let wf = make_workflow("post-merge-main");
        assert!(matches_trigger(&wf, &TriggerEvent::PostMergeMain));
    }

    #[test]
    fn test_new_commits_on_main_matches() {
        let wf = make_workflow("new-commits-on-main");
        assert!(matches_trigger(&wf, &TriggerEvent::NewCommitsOnMain));
    }

    #[test]
    fn test_manual_matches_by_name() {
        let wf = make_workflow("manual");
        assert!(matches_trigger(
            &wf,
            &TriggerEvent::Manual {
                workflow_name: "test-wf".into()
            }
        ));
        assert!(!matches_trigger(
            &wf,
            &TriggerEvent::Manual {
                workflow_name: "other-wf".into()
            }
        ));
    }

    #[test]
    fn test_manual_does_not_match_non_manual_event() {
        let wf = make_workflow("manual");
        assert!(!matches_trigger(&wf, &TriggerEvent::PostPush));
    }

    #[test]
    fn test_matching_workflows_filters_correctly() {
        let wf1 = make_workflow("post-push");
        let wf2 = make_workflow("pr-approved");
        let kdl = r#"
autopilot "named" {
    trigger "manual"
    step "s" {
        notify "hi"
    }
}
"#;
        let wf3 = parse_autopilot_workflow(kdl).unwrap();

        let workflows = vec![wf1, wf2, wf3];

        let matches = matching_workflows(&workflows, &TriggerEvent::PostPush);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "test-wf");

        let manual_matches = matching_workflows(
            &workflows,
            &TriggerEvent::Manual {
                workflow_name: "named".into(),
            },
        );
        assert_eq!(manual_matches.len(), 1);
        assert_eq!(manual_matches[0].name, "named");
    }

    #[test]
    fn test_no_match_returns_empty() {
        let wf = make_workflow("post-push");
        let workflows = [wf];
        let matches = matching_workflows(&workflows, &TriggerEvent::PrApproved);
        assert!(matches.is_empty());
    }
}
