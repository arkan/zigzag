use crate::state::WorkflowRun;
use std::path::{Path, PathBuf};
use z_core::error::{Result, ZError};

/// Return the file path for a workflow run's persisted state.
/// Format: `{state_dir}/{project}/{workflow_name}.json`
fn run_path(state_dir: &Path, project: &str, workflow_name: &str) -> PathBuf {
    state_dir.join(project).join(format!("{workflow_name}.json"))
}

/// Persist a workflow run to disk as JSON.
///
/// Creates parent directories if they do not exist.
pub fn save_run(run: &WorkflowRun, state_dir: &Path) -> Result<()> {
    let path = run_path(state_dir, &run.project, &run.workflow_name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ZError::Io(format!("create dirs {}: {e}", parent.display())))?;
    }
    let json = serde_json::to_string_pretty(run)
        .map_err(|e| ZError::Io(format!("serialize run: {e}")))?;
    std::fs::write(&path, json)
        .map_err(|e| ZError::Io(format!("write {}: {e}", path.display())))?;
    Ok(())
}

/// Load a workflow run from disk. Returns `None` if the file does not exist.
pub fn load_run(project: &str, workflow_name: &str, state_dir: &Path) -> Result<Option<WorkflowRun>> {
    let path = run_path(state_dir, project, workflow_name);
    if !path.exists() {
        return Ok(None);
    }
    let json = std::fs::read_to_string(&path)
        .map_err(|e| ZError::Io(format!("read {}: {e}", path.display())))?;
    let run: WorkflowRun = serde_json::from_str(&json)
        .map_err(|e| ZError::Io(format!("deserialize run: {e}")))?;
    Ok(Some(run))
}

/// Delete a persisted workflow run. Missing files are treated as already deleted.
pub fn delete_run(project: &str, workflow_name: &str, state_dir: &Path) -> Result<()> {
    let path = run_path(state_dir, project, workflow_name);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ZError::Io(format!("delete {}: {e}", path.display()))),
    }
}

/// List all persisted workflow runs under `state_dir`.
pub fn list_runs(state_dir: &Path) -> Result<Vec<WorkflowRun>> {
    if !state_dir.exists() {
        return Ok(Vec::new());
    }
    let mut runs = Vec::new();
    collect_runs(state_dir, &mut runs)?;
    Ok(runs)
}

/// Delete terminal workflow runs and return the runs that were removed.
pub fn prune_terminal_runs(state_dir: &Path, project_filter: Option<&str>) -> Result<Vec<WorkflowRun>> {
    let runs = list_runs(state_dir)?;
    let mut removed = Vec::new();
    for run in runs {
        if project_filter.map_or(false, |project| run.project != project) {
            continue;
        }
        if is_terminal_run(&run) {
            delete_run(&run.project, &run.workflow_name, state_dir)?;
            removed.push(run);
        }
    }
    Ok(removed)
}

fn is_terminal_run(run: &WorkflowRun) -> bool {
    !matches!(run.status, crate::state::WorkflowStatus::Running)
}

fn collect_runs(dir: &Path, runs: &mut Vec<WorkflowRun>) -> Result<()> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| ZError::Io(format!("read dir {}: {e}", dir.display())))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_runs(&path, runs)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("json") {
            match std::fs::read_to_string(&path) {
                Ok(json) => {
                    if let Ok(run) = serde_json::from_str::<WorkflowRun>(&json) {
                        runs.push(run);
                    }
                }
                Err(_) => {}
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{WorkflowStatus, WorkflowRun};
    use std::fs;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "z-autopilot-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_run() -> WorkflowRun {
        WorkflowRun::new("pr-ci-fix", "myproject", "monitor-ci")
    }

    #[test]
    fn test_save_and_load_run() {
        let dir = temp_dir();
        let run = sample_run();
        save_run(&run, &dir).unwrap();
        let loaded = load_run("myproject", "pr-ci-fix", &dir).unwrap().unwrap();
        assert_eq!(loaded.workflow_name, "pr-ci-fix");
        assert_eq!(loaded.project, "myproject");
        assert_eq!(loaded.current_step.as_deref(), Some("monitor-ci"));
        assert_eq!(loaded.status, WorkflowStatus::Running);
    }

    #[test]
    fn test_load_nonexistent_returns_none() {
        let dir = temp_dir();
        let result = load_run("noproject", "no-workflow", &dir).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_delete_run_removes_file() {
        let dir = temp_dir();
        let run = sample_run();
        save_run(&run, &dir).unwrap();

        delete_run("myproject", "pr-ci-fix", &dir).unwrap();

        assert!(load_run("myproject", "pr-ci-fix", &dir).unwrap().is_none());
    }

    #[test]
    fn test_delete_run_missing_file_is_ok() {
        let dir = temp_dir();

        delete_run("missing", "missing", &dir).unwrap();
    }

    #[test]
    fn test_save_overwrites_existing() {
        let dir = temp_dir();
        let mut run = sample_run();
        save_run(&run, &dir).unwrap();

        run.status = WorkflowStatus::Completed;
        run.current_step = None;
        save_run(&run, &dir).unwrap();

        let loaded = load_run("myproject", "pr-ci-fix", &dir).unwrap().unwrap();
        assert_eq!(loaded.status, WorkflowStatus::Completed);
        assert!(loaded.current_step.is_none());
    }

    #[test]
    fn test_list_runs_empty_dir() {
        let dir = temp_dir();
        let runs = list_runs(&dir).unwrap();
        assert!(runs.is_empty());
    }

    #[test]
    fn test_list_runs_nonexistent_dir() {
        let dir = std::env::temp_dir().join("z-autopilot-nonexistent-12345");
        let runs = list_runs(&dir).unwrap();
        assert!(runs.is_empty());
    }

    #[test]
    fn test_list_runs_multiple() {
        let dir = temp_dir();

        let run1 = WorkflowRun::new("wf1", "project-a", "step1");
        let run2 = WorkflowRun::new("wf2", "project-b", "step2");
        save_run(&run1, &dir).unwrap();
        save_run(&run2, &dir).unwrap();

        let mut runs = list_runs(&dir).unwrap();
        runs.sort_by(|a, b| a.workflow_name.cmp(&b.workflow_name));
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].workflow_name, "wf1");
        assert_eq!(runs[1].workflow_name, "wf2");
    }

    #[test]
    fn test_prune_terminal_runs_keeps_running_runs() {
        let dir = temp_dir();
        let running = WorkflowRun::new("running", "myproject", "step1");
        let mut completed = WorkflowRun::new("completed", "myproject", "step1");
        completed.status = WorkflowStatus::Completed;
        completed.current_step = None;
        save_run(&running, &dir).unwrap();
        save_run(&completed, &dir).unwrap();

        let removed = prune_terminal_runs(&dir, None).unwrap();

        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].workflow_name, "completed");
        assert!(load_run("myproject", "running", &dir).unwrap().is_some());
        assert!(load_run("myproject", "completed", &dir).unwrap().is_none());
    }

    #[test]
    fn test_prune_terminal_runs_respects_project_filter() {
        let dir = temp_dir();
        let mut keep = WorkflowRun::new("done", "keep", "step1");
        keep.status = WorkflowStatus::Completed;
        keep.current_step = None;
        let mut remove = WorkflowRun::new("done", "remove", "step1");
        remove.status = WorkflowStatus::Completed;
        remove.current_step = None;
        save_run(&keep, &dir).unwrap();
        save_run(&remove, &dir).unwrap();

        let removed = prune_terminal_runs(&dir, Some("remove")).unwrap();

        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].project, "remove");
        assert!(load_run("keep", "done", &dir).unwrap().is_some());
        assert!(load_run("remove", "done", &dir).unwrap().is_none());
    }

    #[test]
    fn test_run_path_structure() {
        let dir = PathBuf::from("/state");
        let path = run_path(&dir, "myproject", "pr-ci-fix");
        assert_eq!(path, PathBuf::from("/state/myproject/pr-ci-fix.json"));
    }

    #[test]
    fn test_save_creates_parent_dirs() {
        let dir = temp_dir().join("deeply").join("nested");
        // dir doesn't exist yet
        let run = sample_run();
        save_run(&run, &dir).unwrap();
        assert!(run_path(&dir, "myproject", "pr-ci-fix").exists());
    }
}
