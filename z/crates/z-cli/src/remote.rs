use std::process::Command;

use z_core::domain::Session;
use z_core::error::{Result, ZError};

use crate::session_manager::parse_zellij_sessions;

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

/// Run a shell command on a remote host via SSH and wait for it to complete.
///
/// Returns an error if SSH fails to launch or exits non-zero.
pub fn ssh_run_remote(ssh_host: &str, command: &str) -> Result<()> {
    let status = Command::new("ssh")
        .args([ssh_host, command])
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
    let output = Command::new("ssh")
        .args([ssh_host, "zellij list-sessions 2>/dev/null || true"])
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
