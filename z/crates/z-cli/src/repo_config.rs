use z_autopilot::dsl::{parse_autopilot_workflows_doc, AutopilotWorkflow};
use z_core::config::{parse_per_repo_config_doc, PerRepoConfig};
use z_core::error::{Result, ZError};

/// All CLI projections from a repo-local `.config/z.kdl` file.
#[derive(Debug, Clone, Default)]
pub struct RepoConfigProjection {
    pub per_repo: PerRepoConfig,
    pub workflows: Vec<AutopilotWorkflow>,
}

/// Parse `.config/z.kdl` once and project the slices needed by CLI/TUI callers.
pub fn parse_repo_config_projection(content: &str) -> Result<RepoConfigProjection> {
    let doc: kdl::KdlDocument = content
        .parse()
        .map_err(|e| ZError::ConfigParse(format!("KDL parse error: {e}")))?;
    let per_repo = parse_per_repo_config_doc(&doc)?;
    let workflows = parse_autopilot_workflows_doc(&doc)?;

    Ok(RepoConfigProjection { per_repo, workflows })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_per_repo_actions_and_workflows_from_one_projection() {
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

        let projection = parse_repo_config_projection(content).unwrap();

        assert_eq!(projection.workflows.len(), 1);
        assert_eq!(projection.workflows[0].name, "manual-check");
        assert_eq!(projection.per_repo.actions.len(), 1);
        assert_eq!(projection.per_repo.actions[0].name, "Run tests");
    }

    #[test]
    fn rejects_malformed_kdl_before_projection() {
        let err = parse_repo_config_projection("layout { invalid !!!").unwrap_err();

        assert!(err.to_string().contains("KDL parse error"));
    }
}
