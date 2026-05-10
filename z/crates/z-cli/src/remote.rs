use std::process::Command;

use z_core::domain::Session;
use z_core::error::{Result, ZError};

use crate::session_manager::parse_zellij_sessions;

// ---------------------------------------------------------------------------
// Shell quoting
// ---------------------------------------------------------------------------

/// POSIX shell-quote a value so it is safe to embed in a command string.
///
/// Wraps the value in single quotes, escaping any embedded single quotes
/// with the `'\''` idiom (end quote, escaped quote, restart quote).
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

// ---------------------------------------------------------------------------
// SSH execution
// ---------------------------------------------------------------------------

/// Build an SSH `Command` with connection timeout.
///
/// All SSH operations go through this to ensure consistent timeout behaviour.
/// The remote command is wrapped in `bash -l -c '…'` so that the login shell
/// profile is loaded (required for nix/direnv environments).
pub fn build_ssh_command(ssh_host: &str, command: &str) -> Command {
    let wrapped = format!("bash -l -c {}", shell_quote(command));
    let mut cmd = Command::new("ssh");
    cmd.args(["-o", "ConnectTimeout=10", ssh_host, &wrapped]);
    cmd
}

/// Run a shell command on a remote host via SSH and wait for it to complete.
///
/// Returns an error if SSH fails to launch or exits non-zero.
pub fn ssh_run_remote(ssh_host: &str, command: &str) -> Result<()> {
    let status = build_ssh_command(ssh_host, command)
        .status()
        .map_err(|e| ZError::Session(format!("SSH to {} failed to launch: {}", ssh_host, e)))?;
    if !status.success() {
        return Err(ZError::Session(format!(
            "SSH command on {} exited with status {}: {}",
            ssh_host, status, command
        )));
    }
    Ok(())
}

/// List Zellij sessions on a remote host for a given project, via SSH.
///
/// Uses `zellij list-sessions` on the remote; ignores errors from Zellij
/// not running (`|| true`). Returns an empty list if the remote is unreachable.
pub fn list_remote_sessions(ssh_host: &str, project: &str) -> Result<Vec<Session>> {
    let output = build_ssh_command(ssh_host, "zellij list-sessions 2>/dev/null || true")
        .output()
        .map_err(|e| {
            ZError::Session(format!(
                "SSH to {} failed while listing sessions: {}",
                ssh_host, e
            ))
        })?;
    let raw = String::from_utf8_lossy(&output.stdout);
    let stdout = crate::session_manager::strip_ansi(&raw);
    Ok(parse_zellij_sessions(&stdout, project))
}

/// Kill a Zellij session on a remote host via SSH.
pub fn delete_remote_session(ssh_host: &str, session_name: &str) -> Result<()> {
    let cmd = format!("zellij delete-session {}", shell_quote(session_name));
    ssh_run_remote(ssh_host, &cmd)
}

/// Remove a worktree on a remote host via SSH.
///
/// Runs `cd {project_path} && wt remove {branch}` on the remote.
pub fn remove_remote_worktree(ssh_host: &str, project_path: &str, branch: &str) -> Result<()> {
    let cmd = format!(
        "cd {} && wt remove {}",
        shell_quote(project_path),
        shell_quote(branch)
    );
    ssh_run_remote(ssh_host, &cmd)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    // --- build_ssh_command ---

    #[test]
    fn build_ssh_command_includes_timeout() {
        let cmd = build_ssh_command("myhost", "echo hi");
        let args: Vec<&OsStr> = cmd.get_args().collect();
        assert!(args.contains(&OsStr::new("-o")), "should have -o flag");
        assert!(
            args.contains(&OsStr::new("ConnectTimeout=10")),
            "should have ConnectTimeout=10"
        );
    }

    #[test]
    fn build_ssh_command_has_correct_host_and_cmd() {
        let cmd = build_ssh_command("vps.example.com", "ls -la");
        let args: Vec<&OsStr> = cmd.get_args().collect();
        assert!(args.contains(&OsStr::new("vps.example.com")));
        // Command is wrapped in login shell: bash -l -c '...'
        let joined: String = args.iter().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" ");
        assert!(joined.contains("bash -l -c"), "should wrap in login shell");
        assert!(joined.contains("ls -la"), "should contain the original command");
    }

    // --- shell_quote ---

    #[test]
    fn shell_quote_simple_string() {
        assert_eq!(shell_quote("hello"), "'hello'");
    }

    #[test]
    fn shell_quote_empty_string() {
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn shell_quote_with_spaces() {
        assert_eq!(shell_quote("hello world"), "'hello world'");
    }

    #[test]
    fn shell_quote_with_semicolon() {
        // Prevents shell injection via `;`
        assert_eq!(shell_quote("feat; rm -rf /"), "'feat; rm -rf /'");
    }

    #[test]
    fn shell_quote_with_single_quotes() {
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn shell_quote_with_backticks() {
        assert_eq!(shell_quote("$(whoami)"), "'$(whoami)'");
    }

    #[test]
    fn shell_quote_with_dollar_and_braces() {
        assert_eq!(shell_quote("${HOME}"), "'${HOME}'");
    }

    #[test]
    fn shell_quote_with_newline() {
        assert_eq!(shell_quote("a\nb"), "'a\nb'");
    }

}
