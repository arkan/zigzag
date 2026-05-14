use std::io::{self, Write as _};
use std::path::PathBuf;
use std::process::Command;

use zigzag_autopilot::lifecycle::StepExecutor;
use zigzag_autopilot::persist::{delete_run, load_run, save_run};
use zigzag_autopilot::run_loop::RunStore;
use zigzag_autopilot::state::{StepResult, WorkflowRun};
use zigzag_core::domain::NotifyLevel;
use zigzag_core::error::{Result, ZError};
use zigzag_core::traits::Notifier;

use crate::remote;

/// File-backed Adapter for Autopilot run persistence.
pub struct FileRunStore {
    state_dir: PathBuf,
}

impl FileRunStore {
    pub fn new(state_dir: PathBuf) -> Self {
        Self { state_dir }
    }
}

impl RunStore for FileRunStore {
    fn load_run(&self, project: &str, workflow_name: &str) -> Result<Option<WorkflowRun>> {
        load_run(project, workflow_name, &self.state_dir)
    }

    fn save_run(&self, run: &WorkflowRun) -> Result<()> {
        save_run(run, &self.state_dir)
    }

    fn delete_run(&self, project: &str, workflow_name: &str) -> Result<()> {
        delete_run(project, workflow_name, &self.state_dir)
    }
}

/// Process-backed Adapter for Autopilot step execution.
pub struct CliStepExecutor {
    cwd: PathBuf,
    host: Option<String>,
    notifier: Box<dyn Notifier>,
}

impl CliStepExecutor {
    pub fn new(cwd: PathBuf, host: Option<String>, notifier: Box<dyn Notifier>) -> Self {
        Self {
            cwd,
            host,
            notifier,
        }
    }

    fn run_local_command(&self, command: &str) -> Result<StepResult> {
        let output = Command::new("sh")
            .args(["-c", command])
            .current_dir(&self.cwd)
            .output()
            .map_err(|e| ZError::Io(format!("run autopilot command: {e}")))?;
        Ok(step_result_from_output(
            output.status.success(),
            &output.stdout,
            &output.stderr,
        ))
    }

    fn run_remote_command(&self, host: &str, command: &str) -> Result<StepResult> {
        let remote_cmd = format!(
            "cd {} && {}",
            remote::shell_quote(&self.cwd.display().to_string()),
            command
        );
        let wrapped = format!("bash -l -c {}", remote::shell_quote(&remote_cmd));
        let output = Command::new("ssh")
            .args(["-o", "ConnectTimeout=10", host, &wrapped])
            .output()
            .map_err(|e| ZError::Io(format!("run remote autopilot command: {e}")))?;
        Ok(step_result_from_output(
            output.status.success(),
            &output.stdout,
            &output.stderr,
        ))
    }
}

impl StepExecutor for CliStepExecutor {
    fn run_command(&self, command: &str) -> Result<StepResult> {
        if let Some(host) = &self.host {
            self.run_remote_command(host, command)
        } else {
            self.run_local_command(command)
        }
    }

    fn notify(&self, message: &str) -> Result<()> {
        self.notifier.notify(message, NotifyLevel::Info)
    }

    fn confirm(&self, prompt: &str) -> Result<bool> {
        print!("{prompt} [y/N] ");
        io::stdout()
            .flush()
            .map_err(|e| ZError::Io(format!("flush confirmation prompt: {e}")))?;
        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .map_err(|e| ZError::Io(format!("read confirmation: {e}")))?;
        Ok(matches!(input.trim(), "y" | "Y" | "yes" | "YES" | "Yes"))
    }
}

fn step_result_from_output(success: bool, stdout: &[u8], stderr: &[u8]) -> StepResult {
    let output = command_output(stdout, stderr);
    if success {
        StepResult::Success { output }
    } else {
        StepResult::Failure { output }
    }
}

fn command_output(stdout: &[u8], stderr: &[u8]) -> Option<String> {
    let mut text = String::new();
    let stdout = String::from_utf8_lossy(stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(stderr).trim().to_string();

    if !stdout.is_empty() {
        text.push_str(&stdout);
    }
    if !stderr.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&stderr);
    }

    if text.is_empty() {
        None
    } else {
        Some(truncate_output(&text, 16 * 1024))
    }
}

fn truncate_output(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut truncated = text[..limit].to_string();
    truncated.push_str("\n[output truncated]");
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_output_combines_stdout_and_stderr() {
        assert_eq!(
            command_output(b"ok\n", b"warn\n"),
            Some("ok\nwarn".to_string())
        );
    }

    #[test]
    fn command_output_returns_none_when_empty() {
        assert_eq!(command_output(b"", b""), None);
    }

    #[test]
    fn step_result_from_output_marks_failure() {
        assert_eq!(
            step_result_from_output(false, b"", b"failed"),
            StepResult::Failure {
                output: Some("failed".to_string())
            }
        );
    }
}
