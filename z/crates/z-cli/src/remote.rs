use std::io::Write as _;
use std::process::{Command, Stdio};

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
/// Uses `zellij list-sessions` on the remote and returns an error on SSH or
/// command failure so callers can surface remote-unavailable states.
pub fn list_remote_sessions(ssh_host: &str, project: &str) -> Result<Vec<Session>> {
    let output = build_ssh_command(ssh_host, "zellij list-sessions 2>/dev/null")
        .output()
        .map_err(|e| {
            ZError::Session(format!(
                "SSH to {} failed while listing sessions: {}",
                ssh_host, e
            ))
        })?;
    if !output.status.success() {
        return Err(ZError::Session(format!(
            "SSH to {} failed while listing sessions with status {}",
            ssh_host, output.status
        )));
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    let stdout = crate::session_manager::strip_ansi(&raw);
    Ok(parse_zellij_sessions(&stdout, project))
}

/// Kill a Zellij session on a remote host via SSH.
pub fn delete_remote_session(ssh_host: &str, session_name: &str) -> Result<()> {
    let cmd = format!("zellij delete-session {}", shell_quote(session_name));
    ssh_run_remote(ssh_host, &cmd)
}

/// Read a file under the remote `~/.config/z/` directory.
pub fn read_remote_z_config_file(ssh_host: &str, filename: &str) -> Result<Option<String>> {
    let cmd = format!(
        "dir=\"${{XDG_CONFIG_HOME:-$HOME/.config}}/z\"; file=\"$dir/{}\"; if [ -f \"$file\" ]; then cat \"$file\"; else exit 3; fi",
        filename.replace('"', "")
    );
    let output = build_ssh_command(ssh_host, &cmd)
        .output()
        .map_err(|e| ZError::Io(format!("SSH to {} failed while reading config: {}", ssh_host, e)))?;
    if output.status.success() {
        return Ok(Some(String::from_utf8_lossy(&output.stdout).to_string()));
    }
    if output.status.code() == Some(3) {
        return Ok(None);
    }
    Err(ZError::Io(format!(
        "remote config read on {} exited with status {}",
        ssh_host, output.status
    )))
}

/// Atomically write a file under the remote `~/.config/z/` directory with a mkdir lock.
pub fn write_remote_z_config_file_atomic(ssh_host: &str, filename: &str, bytes: &[u8]) -> Result<()> {
    let safe_filename = filename.replace('"', "");
    let cmd = format!(
        "set -e; dir=\"${{XDG_CONFIG_HOME:-$HOME/.config}}/z\"; mkdir -p \"$dir\"; file=\"$dir/{0}\"; lock=\"$dir/{0}.lock\"; tmp=\"$dir/{0}.tmp.$$\"; i=0; while ! mkdir \"$lock\" 2>/dev/null; do i=$((i+1)); if [ $i -ge 10 ]; then echo 'metadata lock unavailable' >&2; exit 4; fi; sleep 0.05; done; trap 'rm -rf \"$lock\" \"$tmp\"' EXIT; cat > \"$tmp\"; mv \"$tmp\" \"$file\"; rm -rf \"$lock\"; trap - EXIT",
        safe_filename
    );
    let mut child = build_ssh_command(ssh_host, &cmd)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| ZError::Io(format!("SSH to {} failed while writing config: {}", ssh_host, e)))?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(bytes)
            .map_err(|e| ZError::Io(format!("write remote config stdin: {e}")))?;
    }
    let status = child
        .wait()
        .map_err(|e| ZError::Io(format!("wait remote config write: {e}")))?;
    if !status.success() {
        return Err(ZError::Io(format!(
            "remote config write on {} exited with status {}",
            ssh_host, status
        )));
    }
    Ok(())
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
