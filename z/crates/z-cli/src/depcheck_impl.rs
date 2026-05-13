use std::process::Command;

use z_core::depcheck::DepChecker;
use z_core::error::{Result, ZError};

/// Real `DepChecker` that shells out to each tool's `--version` flag.
pub struct ProcessDepChecker;

impl DepChecker for ProcessDepChecker {
    fn get_version_output(&self, tool: &str) -> Result<Option<String>> {
        match Command::new(tool).arg("--version").output() {
            Ok(output) => {
                // Some tools (e.g. older gh) print to stderr.
                let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
                let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
                let text = if stdout.trim().is_empty() {
                    stderr
                } else {
                    stdout
                };
                Ok(Some(text))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(ZError::Io(e.to_string())),
        }
    }
}
