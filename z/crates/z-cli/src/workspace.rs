use std::collections::HashMap;

use z_autopilot::dsl::AutopilotWorkflow;
use z_core::action::ActionDef;
use z_core::domain::{Project, Session};
use z_tui::{ProjectEntry, WorkflowInfo};

#[derive(Debug, Default)]
pub(crate) struct RepoWorkspaceConfig {
    pub(crate) custom_workflows: Vec<AutopilotWorkflow>,
    pub(crate) repo_actions: Vec<ActionDef>,
}

pub(crate) struct WorkspaceEntryInput {
    pub(crate) project: Project,
    pub(crate) sessions: Vec<Session>,
    pub(crate) worktree_count: usize,
    pub(crate) custom_workflows: Vec<AutopilotWorkflow>,
    pub(crate) repo_actions: Vec<ActionDef>,
}

pub(crate) fn parse_repo_workspace_config(content: &str) -> RepoWorkspaceConfig {
    let custom_workflows = z_autopilot::dsl::parse_autopilot_workflows(content).unwrap_or_default();
    let repo_actions = z_core::config::parse_per_repo_config_kdl(content)
        .map(|config| config.actions)
        .unwrap_or_default();

    RepoWorkspaceConfig {
        custom_workflows,
        repo_actions,
    }
}

pub(crate) fn build_project_entry(
    input: WorkspaceEntryInput,
    builtin_workflows: &[AutopilotWorkflow],
    activity: &HashMap<String, u64>,
) -> ProjectEntry {
    let mut sessions = input.sessions;
    z_core::activity::sort_sessions_by_recent_attach(&mut sessions, activity);

    let workflows = builtin_workflows
        .iter()
        .chain(input.custom_workflows.iter())
        .map(|workflow| WorkflowInfo {
            name: workflow.name.clone(),
            trigger: workflow.trigger.as_str().to_string(),
            description: workflow.description.clone().unwrap_or_default(),
        })
        .collect();

    ProjectEntry {
        project: input.project,
        sessions,
        worktree_count: input.worktree_count,
        workflows,
        repo_actions: input.repo_actions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use z_autopilot::dsl::Trigger;
    use z_core::domain::Transport;

    fn project() -> Project {
        Project {
            name: "myapp".to_string(),
            path: PathBuf::from("/repo/myapp"),
            host: None,
            transport: Some(Transport::Ssh),
        }
    }

    fn workflow(name: &str, description: Option<&str>) -> AutopilotWorkflow {
        AutopilotWorkflow {
            name: name.to_string(),
            description: description.map(str::to_string),
            trigger: Trigger::Manual,
            poll_interval: None,
            steps: Vec::new(),
            auto_push: None,
            review: None,
        }
    }

    #[test]
    fn build_project_entry_sorts_sessions_and_maps_workflows() {
        let input = WorkspaceEntryInput {
            project: project(),
            sessions: vec![Session::new("myapp", "old"), Session::new("myapp", "new")],
            worktree_count: 2,
            custom_workflows: vec![workflow("custom", Some("Custom workflow"))],
            repo_actions: Vec::new(),
        };
        let builtin = vec![workflow("builtin", None)];
        let activity = HashMap::from([
            ("myapp:old".to_string(), 100),
            ("myapp:new".to_string(), 200),
        ]);

        let entry = build_project_entry(input, &builtin, &activity);

        assert_eq!(entry.sessions[0].name, "myapp:new");
        assert_eq!(entry.sessions[1].name, "myapp:old");
        assert_eq!(entry.worktree_count, 2);
        assert_eq!(entry.workflows[0].name, "builtin");
        assert_eq!(entry.workflows[1].name, "custom");
        assert_eq!(entry.workflows[1].description, "Custom workflow");
    }

    #[test]
    fn parse_repo_workspace_config_collects_workflows_and_actions() {
        let content = r#"
autopilot "manual-check" {
    trigger "manual"
    step "notify" {
        notify "done"
    }
}

actions {
    action "Run tests" {
        run "cargo test"
        context "project"
    }
}
"#;

        let config = parse_repo_workspace_config(content);

        assert_eq!(config.custom_workflows.len(), 1);
        assert_eq!(config.custom_workflows[0].name, "manual-check");
        assert_eq!(config.repo_actions.len(), 1);
        assert_eq!(config.repo_actions[0].name, "Run tests");
    }
}
