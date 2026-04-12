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
// URL helpers
// ---------------------------------------------------------------------------

/// Extract the SSH hostname from a Zellij host URL.
///
/// Strips the scheme (`https://` / `http://`), port, and path:
/// - `"https://vps.example.com:8082"` → `"vps.example.com"`
/// - `"https://example.com"` → `"example.com"`
/// - `"http://dev.example.com:8080"` → `"dev.example.com"`
/// - `"https://host/path"` → `"host"`
///
/// Returns an error if the resulting hostname is empty (e.g. `"https://"`).
pub fn extract_ssh_host(host_url: &str) -> Result<String> {
    let without_scheme = host_url
        .strip_prefix("https://")
        .or_else(|| host_url.strip_prefix("http://"))
        .unwrap_or(host_url);
    // Strip port (`:…`) or path (`/…`) — take only the hostname portion.
    let hostname = without_scheme
        .split(&[':', '/'][..])
        .next()
        .unwrap_or(without_scheme);
    if hostname.is_empty() {
        return Err(ZError::Session(format!(
            "cannot extract SSH host from {:?}: empty hostname",
            host_url
        )));
    }
    Ok(hostname.to_string())
}

/// Build the full Zellij HTTPS attach URL for a remote session.
///
/// # Examples
/// - `("https://vps.example.com:8082", "prod-api:feat-x")` →
///   `"https://vps.example.com:8082/prod-api:feat-x"`
pub fn build_remote_attach_url(host: &str, session_name: &str) -> String {
    format!("{}/{}", host.trim_end_matches('/'), session_name)
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
pub fn build_ssh_command(ssh_host: &str, command: &str) -> Command {
    let mut cmd = Command::new("ssh");
    cmd.args(["-o", "ConnectTimeout=10", ssh_host, command]);
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
        assert!(args.contains(&OsStr::new("ls -la")));
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

    // --- extract_ssh_host ---

    #[test]
    fn extract_ssh_host_https_with_port() {
        let result = extract_ssh_host("https://vps.example.com:8082").unwrap();
        assert_eq!(result, "vps.example.com");
    }

    #[test]
    fn extract_ssh_host_https_without_port() {
        let result = extract_ssh_host("https://example.com").unwrap();
        assert_eq!(result, "example.com");
    }

    #[test]
    fn extract_ssh_host_http_with_port() {
        let result = extract_ssh_host("http://dev.example.com:8080").unwrap();
        assert_eq!(result, "dev.example.com");
    }

    #[test]
    fn extract_ssh_host_no_scheme_with_port() {
        // Falls through to raw splitting on ':'
        let result = extract_ssh_host("myhost:8080").unwrap();
        assert_eq!(result, "myhost");
    }

    #[test]
    fn extract_ssh_host_bare_hostname() {
        let result = extract_ssh_host("myhost").unwrap();
        assert_eq!(result, "myhost");
    }

    #[test]
    fn extract_ssh_host_empty_string_returns_error() {
        assert!(extract_ssh_host("").is_err());
    }

    #[test]
    fn extract_ssh_host_scheme_only_returns_error() {
        // "https://" → without_scheme = "" → error
        assert!(extract_ssh_host("https://").is_err());
    }

    #[test]
    fn extract_ssh_host_scheme_only_http_returns_error() {
        assert!(extract_ssh_host("http://").is_err());
    }

    #[test]
    fn extract_ssh_host_subdomain() {
        let result = extract_ssh_host("https://api.prod.example.com:9000").unwrap();
        assert_eq!(result, "api.prod.example.com");
    }

    #[test]
    fn extract_ssh_host_localhost() {
        let result = extract_ssh_host("https://localhost:8082").unwrap();
        assert_eq!(result, "localhost");
    }

    // --- build_remote_attach_url ---

    #[test]
    fn build_remote_attach_url_basic() {
        let url = build_remote_attach_url("https://vps.example.com:8082", "prod-api:feat-x");
        assert_eq!(url, "https://vps.example.com:8082/prod-api:feat-x");
    }

    #[test]
    fn build_remote_attach_url_trailing_slash_trimmed() {
        let url = build_remote_attach_url("https://vps.example.com:8082/", "prod-api:main");
        assert_eq!(url, "https://vps.example.com:8082/prod-api:main");
    }

    #[test]
    fn build_remote_attach_url_sanitized_branch() {
        // Branch slashes already replaced by '-' in session name
        let url =
            build_remote_attach_url("https://vps.example.com:8082", "prod-api:feat-user-auth");
        assert_eq!(url, "https://vps.example.com:8082/prod-api:feat-user-auth");
    }

    #[test]
    fn build_remote_attach_url_main_session() {
        let url = build_remote_attach_url("https://vps.example.com:8082", "myapp:main");
        assert_eq!(url, "https://vps.example.com:8082/myapp:main");
    }

    #[test]
    fn build_remote_attach_url_http_host() {
        let url = build_remote_attach_url("http://localhost:8082", "dev:test-branch");
        assert_eq!(url, "http://localhost:8082/dev:test-branch");
    }

    // --- extract_ssh_host: path stripping ---

    #[test]
    fn extract_ssh_host_url_with_path_no_port() {
        // Path component must be stripped even without a port.
        let result = extract_ssh_host("https://host.example.com/some/path").unwrap();
        assert_eq!(result, "host.example.com");
    }

    #[test]
    fn extract_ssh_host_url_with_path_and_port() {
        let result = extract_ssh_host("https://host.example.com:8082/some/path").unwrap();
        assert_eq!(result, "host.example.com");
    }

    #[test]
    fn extract_ssh_host_bare_host_with_path() {
        let result = extract_ssh_host("myhost/path").unwrap();
        assert_eq!(result, "myhost");
    }

    #[test]
    fn extract_ssh_host_port_only_after_scheme() {
        // "https://:8082" → empty hostname → error
        assert!(extract_ssh_host("https://:8082").is_err());
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

    // --- build_remote_attach_url: edge cases ---

    #[test]
    fn build_remote_attach_url_multiple_trailing_slashes() {
        let url = build_remote_attach_url("https://host:8082///", "proj:main");
        // trim_end_matches('/') removes all trailing slashes
        assert_eq!(url, "https://host:8082/proj:main");
    }

    #[test]
    fn build_remote_attach_url_empty_session_name() {
        let url = build_remote_attach_url("https://host:8082", "");
        assert_eq!(url, "https://host:8082/");
    }
}
