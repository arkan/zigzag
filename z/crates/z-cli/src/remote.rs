use std::process::Command;

use z_core::domain::{Session, Worktree};
use z_core::error::{Result, ZError};

use crate::session_manager::parse_zellij_sessions;
use crate::worktree_manager::parse_git_worktree_porcelain;

// ---------------------------------------------------------------------------
// Remote git info
// ---------------------------------------------------------------------------

/// Git info fetched from a remote host via SSH.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteGitInfo {
    pub branch: String,
    pub is_dirty: bool,
    pub commits: Vec<(String, String)>,
    pub ahead: usize,
    pub behind: usize,
}

const GIT_SEP: &str = "---GIT-SEP---";

/// Build the combined git command to run on the remote.
pub fn build_remote_git_command(project_path: &str) -> String {
    format!(
        "cd {} && git symbolic-ref --short HEAD && echo '{}' && git status --short && echo '{}' && git log --oneline -5 2>/dev/null && echo '{}' && git rev-list --left-right --count @{{u}}...HEAD 2>/dev/null",
        shell_quote(project_path), GIT_SEP, GIT_SEP, GIT_SEP
    )
}

/// Parse the combined git output from a remote SSH call into `RemoteGitInfo`.
///
/// Expected format (sections separated by `---GIT-SEP---`):
/// 1. Branch name (single line)
/// 2. `git status --short` output (may be empty)
/// 3. `git log --oneline -5` output (may be empty)
/// 4. `git rev-list --left-right --count` output: `<behind>\t<ahead>` (may be empty)
pub fn parse_remote_git_output(output: &str) -> Result<RemoteGitInfo> {
    let sections: Vec<&str> = output.split(GIT_SEP).collect();
    if sections.len() < 2 {
        return Err(ZError::Session("unexpected remote git output format".to_string()));
    }

    let branch = sections[0].trim().to_string();
    if branch.is_empty() {
        return Err(ZError::Session("remote git: empty branch name".to_string()));
    }

    let is_dirty = sections.get(1).map_or(false, |s| !s.trim().is_empty());

    let commits = sections
        .get(2)
        .map(|s| {
            s.trim()
                .lines()
                .filter_map(|line| {
                    let mut parts = line.splitn(2, ' ');
                    let hash = parts.next()?.to_string();
                    let msg = parts.next().unwrap_or("").to_string();
                    Some((hash, msg))
                })
                .collect()
        })
        .unwrap_or_default();

    let (ahead, behind) = sections
        .get(3)
        .and_then(|s| {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return None;
            }
            let mut parts = trimmed.split('\t');
            let behind_str = parts.next()?;
            let ahead_str = parts.next()?;
            Some((
                ahead_str.parse::<usize>().unwrap_or(0),
                behind_str.parse::<usize>().unwrap_or(0),
            ))
        })
        .unwrap_or((0, 0));

    Ok(RemoteGitInfo {
        branch,
        is_dirty,
        commits,
        ahead,
        behind,
    })
}

/// Fetch git info from a remote host via SSH.
pub fn fetch_remote_git_info(ssh_host: &str, project_path: &str) -> Result<RemoteGitInfo> {
    let cmd = build_remote_git_command(project_path);
    let output = build_ssh_command(ssh_host, &cmd)
        .output()
        .map_err(|e| {
            ZError::Session(format!(
                "SSH to {} failed while fetching git info: {}",
                ssh_host, e
            ))
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_remote_git_output(&stdout)
}

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

/// Build an SSH health-check command with a shorter timeout.
pub fn build_ssh_health_command(ssh_host: &str) -> Command {
    let mut cmd = Command::new("ssh");
    cmd.args(["-o", "ConnectTimeout=5", ssh_host, "echo ok"]);
    cmd
}

/// Check SSH connectivity to a remote host.
///
/// Runs `echo ok` with a 5-second timeout. Returns a clear error on failure.
pub fn check_ssh_health(ssh_host: &str) -> Result<()> {
    let output = build_ssh_health_command(ssh_host)
        .output()
        .map_err(|e| ZError::Session(format!(
            "SSH health check to {} failed to launch: {}. Check your SSH config.",
            ssh_host, e
        )))?;
    if !output.status.success() {
        return Err(ZError::Session(format!(
            "SSH health check to {} failed (exit {}). Check your SSH config and that the host is reachable.",
            ssh_host, output.status
        )));
    }
    Ok(())
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
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_zellij_sessions(&stdout, project))
}

/// Kill a Zellij session on a remote host via SSH.
pub fn delete_remote_session(ssh_host: &str, session_name: &str) -> Result<()> {
    let cmd = format!("zellij delete-session {}", shell_quote(session_name));
    ssh_run_remote(ssh_host, &cmd)
}

/// List worktrees on a remote host via SSH.
///
/// Runs `git worktree list --porcelain` on the remote and parses the output
/// using the same parser as local worktree discovery.
pub fn list_remote_worktrees(ssh_host: &str, project_path: &str, project_name: &str) -> Result<Vec<Worktree>> {
    let cmd = format!(
        "cd {} && git worktree list --porcelain",
        shell_quote(project_path)
    );
    let output = build_ssh_command(ssh_host, &cmd)
        .output()
        .map_err(|e| {
            ZError::Worktree(format!(
                "SSH to {} failed while listing worktrees: {}",
                ssh_host, e
            ))
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_git_worktree_porcelain(&stdout, project_name))
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

    // --- build_ssh_health_command ---

    #[test]
    fn build_ssh_health_command_has_short_timeout() {
        let cmd = build_ssh_health_command("myhost");
        let args: Vec<&OsStr> = cmd.get_args().collect();
        assert!(
            args.contains(&OsStr::new("ConnectTimeout=5")),
            "health check should use 5s timeout, not 10s"
        );
    }

    // --- parse_remote_git_output ---

    #[test]
    fn parse_remote_git_output_clean_repo() {
        let output = "main\n---GIT-SEP---\n\n---GIT-SEP---\nabc1234 initial commit\ndef5678 add feature\n---GIT-SEP---\n0\t0\n";
        let info = parse_remote_git_output(output).unwrap();
        assert_eq!(info.branch, "main");
        assert!(!info.is_dirty);
        assert_eq!(info.commits.len(), 2);
        assert_eq!(info.commits[0], ("abc1234".to_string(), "initial commit".to_string()));
        assert_eq!(info.ahead, 0);
        assert_eq!(info.behind, 0);
    }

    #[test]
    fn parse_remote_git_output_dirty_repo() {
        let output = "feat/login\n---GIT-SEP---\n M src/main.rs\n?? new.txt\n---GIT-SEP---\nabc1234 fix bug\n---GIT-SEP---\n2\t3\n";
        let info = parse_remote_git_output(output).unwrap();
        assert_eq!(info.branch, "feat/login");
        assert!(info.is_dirty);
        assert_eq!(info.commits.len(), 1);
        assert_eq!(info.ahead, 3);
        assert_eq!(info.behind, 2);
    }

    #[test]
    fn parse_remote_git_output_no_upstream() {
        let output = "detached\n---GIT-SEP---\n\n---GIT-SEP---\nabc1234 commit\n---GIT-SEP---\n\n";
        let info = parse_remote_git_output(output).unwrap();
        assert_eq!(info.ahead, 0);
        assert_eq!(info.behind, 0);
    }

    #[test]
    fn parse_remote_git_output_empty_log() {
        let output = "main\n---GIT-SEP---\n\n---GIT-SEP---\n\n---GIT-SEP---\n0\t0\n";
        let info = parse_remote_git_output(output).unwrap();
        assert!(info.commits.is_empty());
    }

    // --- list_remote_worktrees command ---

    #[test]
    fn list_remote_worktrees_builds_correct_command() {
        // We can't test actual SSH, but verify the shell command is correct.
        let cmd = format!(
            "cd {} && git worktree list --porcelain",
            shell_quote("/home/user/myapp")
        );
        assert_eq!(cmd, "cd '/home/user/myapp' && git worktree list --porcelain");
    }

    #[test]
    fn list_remote_worktrees_shell_quotes_path_with_spaces() {
        let cmd = format!(
            "cd {} && git worktree list --porcelain",
            shell_quote("/home/user/my app")
        );
        assert_eq!(cmd, "cd '/home/user/my app' && git worktree list --porcelain");
    }

    #[test]
    fn build_ssh_health_command_runs_echo_ok() {
        let cmd = build_ssh_health_command("myhost");
        let args: Vec<&OsStr> = cmd.get_args().collect();
        assert!(args.contains(&OsStr::new("echo ok")));
        assert!(args.contains(&OsStr::new("myhost")));
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
