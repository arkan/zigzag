use std::path::PathBuf;
use std::process::Command;

use z_core::domain::{DiscoveredWorktree, GitSafetyStatus, Worktree, WorktreeIdentity};
use z_core::error::{Result, ZError};
use z_core::traits::WorktreeManager;

use crate::remote;

/// A `WorktreeManager` that delegates to `wt` (worktrunk) and `git worktree`.
///
/// Worktree creation uses `wt switch -c <branch>` run from the project directory.
/// Worktree discovery uses `git worktree list --porcelain` (reliable output format).
pub struct WtWorktreeManager {
    pub project_path: PathBuf,
}

impl WtWorktreeManager {
    pub fn new(project_path: PathBuf) -> Self {
        Self { project_path }
    }
}

impl WorktreeManager for WtWorktreeManager {
    fn list_worktrees(&self, project: &str) -> Result<Vec<Worktree>> {
        let output = Command::new("git")
            .args(["worktree", "list", "--porcelain"])
            .current_dir(&self.project_path)
            .output()
            .map_err(|e| ZError::Worktree(format!("git worktree list failed: {}", e)))?;

        if !output.status.success() {
            return Err(ZError::Worktree(format!(
                "git worktree list exited with status {}",
                output.status
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(parse_git_worktree_porcelain(&stdout, project))
    }

    fn create_worktree(&self, project: &str, branch: &str) -> Result<Worktree> {
        // Fetch latest remote state so the worktree starts from the newest main.
        let fetch = Command::new("git")
            .args(["fetch", "origin"])
            .current_dir(&self.project_path)
            .output()
            .map_err(|e| ZError::Worktree(format!("git fetch origin failed: {}", e)))?;

        if !fetch.status.success() {
            let stderr = String::from_utf8_lossy(&fetch.stderr);
            return Err(ZError::Worktree(format!(
                "git fetch origin failed: {}",
                stderr.trim()
            )));
        }

        // Use wt switch -c to create the worktree (worktrunk convention).
        let output = Command::new("wt")
            .args(["switch", "-c", branch])
            .current_dir(&self.project_path)
            .output()
            .map_err(|e| ZError::Worktree(format!("wt switch failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ZError::Worktree(format!(
                "wt switch -c {} failed: {}",
                branch,
                stderr.trim()
            )));
        }

        // Discover the newly created worktree path via git.
        let worktrees = self.list_worktrees(project)?;
        worktrees
            .into_iter()
            .find(|w| w.branch == branch)
            .ok_or_else(|| {
                ZError::Worktree(format!(
                    "worktree for branch '{}' not found after creation",
                    branch
                ))
            })
    }

    fn remove_worktree(&self, worktree: &Worktree, force: bool) -> Result<()> {
        let mut cmd = Command::new("wt");
        cmd.args(["remove", &worktree.branch]);
        if force {
            cmd.arg("--force");
        }
        let output = cmd
            .current_dir(&self.project_path)
            .output()
            .map_err(|e| ZError::Worktree(format!("wt remove failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ZError::Worktree(format!(
                "wt remove {} failed: {}",
                worktree.branch,
                stderr.trim()
            )));
        }

        Ok(())
    }
}

// =============================================================================
// Shared porcelain parsing
// =============================================================================

/// Parse `git worktree list --porcelain` output into raw (path, optional branch) pairs.
///
/// Unlike the higher-level parsers below, this function does not filter out
/// detached or bare entries — it preserves all worktree blocks as-is.
fn parse_git_worktree_porcelain_blocks(output: &str) -> Vec<(PathBuf, Option<String>)> {
    let mut entries = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_branch: Option<String> = None;

    for line in output.lines() {
        if line.starts_with("worktree ") {
            // Flush previous entry.
            if let Some(path) = current_path.take() {
                entries.push((path, current_branch.take()));
            }
            current_path = Some(PathBuf::from(&line["worktree ".len()..]));
            current_branch = None;
        } else if let Some(refs) = line.strip_prefix("branch refs/heads/") {
            current_branch = Some(refs.to_string());
        }
        // "detached", "bare", and unknown lines are ignored;
        // branch stays None for such entries.
    }

    // Flush last entry.
    if let Some(path) = current_path {
        entries.push((path, current_branch));
    }

    entries
}

// =============================================================================
// Public parsers (backward-compatible + detailed)
// =============================================================================

/// Parse `git worktree list --porcelain` output into a list of `Worktree`s.
///
/// Output format (one block per worktree, separated by blank lines):
/// ```text
/// worktree /absolute/path
/// HEAD abc123...
/// branch refs/heads/branchname
///
/// worktree /absolute/path2
/// HEAD def456...
/// branch refs/heads/feat/login
/// ```
///
/// Only entries with an explicit `branch refs/heads/...` line are included.
/// Detached HEAD, bare, and other branchless entries are **silently skipped**
/// to preserve backward compatibility.
///
/// See [`parse_git_worktree_porcelain_detailed`] for a parser that preserves
/// all entries.
pub fn parse_git_worktree_porcelain(output: &str, project: &str) -> Vec<Worktree> {
    parse_git_worktree_porcelain_blocks(output)
        .into_iter()
        .filter_map(|(path, branch)| {
            branch.map(|b| Worktree {
                path,
                branch: b,
                project: project.to_string(),
            })
        })
        .collect()
}

/// Parse `git worktree list --porcelain` output into `DiscoveredWorktree` entries.
///
/// This parser preserves **all** entries, including detached HEAD and bare
/// worktrees. Each entry has an `Option<String>` branch and an
/// `is_primary_checkout` flag determined by comparing `worktree_path` to
/// `project_root`.
///
/// Use this when you need the full worktree topology — the old
/// [`parse_git_worktree_porcelain`] skips branchless entries for backward
/// compatibility.
pub fn parse_git_worktree_porcelain_detailed(
    output: &str,
    project_name: &str,
    project_root: &std::path::Path,
    host: Option<&str>,
) -> Vec<DiscoveredWorktree> {
    parse_git_worktree_porcelain_blocks(output)
        .into_iter()
        .map(|(path, branch)| {
            let is_primary = path == project_root;
            DiscoveredWorktree {
                identity: WorktreeIdentity {
                    host: host.map(String::from),
                    project_root: project_root.to_path_buf(),
                    worktree_path: path,
                },
                project_name: project_name.to_string(),
                branch,
                is_primary_checkout: is_primary,
            }
        })
        .collect()
}

/// Find the path for a branch in a list of discovered Worktrees.
pub fn find_worktree_path_for_branch(worktrees: &[Worktree], branch: &str) -> Option<PathBuf> {
    worktrees
        .iter()
        .find(|worktree| z_core::domain::sanitize_branch_name(&worktree.branch) == branch)
        .map(|worktree| worktree.path.clone())
}

/// Discover Worktrees on a remote host via SSH, reusing the same porcelain parser as local Worktrees.
pub fn list_remote_worktrees(
    ssh_host: &str,
    project_path: &std::path::Path,
    project: &str,
) -> Result<Vec<Worktree>> {
    let cmd = format!(
        "cd {} && git worktree list --porcelain",
        remote::shell_quote(&project_path.to_string_lossy())
    );
    let output = remote::build_ssh_command(ssh_host, &cmd)
        .output()
        .map_err(|e| {
            ZError::Worktree(format!(
                "SSH to {} failed while listing worktrees: {}",
                ssh_host, e
            ))
        })?;
    if !output.status.success() {
        return Err(ZError::Worktree(format!(
            "remote git worktree list exited with status {}",
            output.status
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_git_worktree_porcelain(&stdout, project))
}

/// Discover detailed Worktree topology on a remote host via SSH.
pub fn list_remote_worktrees_detailed(
    ssh_host: &str,
    project_path: &std::path::Path,
    project: &str,
) -> Result<Vec<DiscoveredWorktree>> {
    let cmd = format!(
        "cd {} && git worktree list --porcelain",
        remote::shell_quote(&project_path.to_string_lossy())
    );
    let output = remote::build_ssh_command(ssh_host, &cmd)
        .output()
        .map_err(|e| {
            ZError::Worktree(format!(
                "SSH to {} failed while listing worktrees: {}",
                ssh_host, e
            ))
        })?;
    if !output.status.success() {
        return Err(ZError::Worktree(format!(
            "remote git worktree list exited with status {}",
            output.status
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_git_worktree_porcelain_detailed(
        &stdout,
        project,
        project_path,
        Some(ssh_host),
    ))
}

// =============================================================================
// Git safety helpers
// =============================================================================

/// Discover the current branch of the primary checkout at `project_path`.
///
/// Runs `git symbolic-ref --short HEAD` in the project directory.
pub fn discover_primary_branch(project_path: &std::path::Path) -> Result<String> {
    let output = Command::new("git")
        .args(["symbolic-ref", "--short", "HEAD"])
        .current_dir(project_path)
        .output()
        .map_err(|e| ZError::Worktree(format!("git symbolic-ref failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ZError::Worktree(format!(
            "git symbolic-ref failed: {}",
            stderr.trim()
        )));
    }

    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() {
        return Err(ZError::Worktree(
            "could not determine primary branch".into(),
        ));
    }

    Ok(branch)
}

/// Check git safety status for a worktree path.
///
/// Returns `GitSafetyStatus` with dirty flag, ahead/behind counts, and
/// explicit `has_upstream` field (no-upstream is distinct from ahead=0, behind=0).
pub fn check_git_safety(worktree_path: &std::path::Path) -> Result<GitSafetyStatus> {
    // Check dirty via `git status --porcelain` (empty output = clean).
    let status_output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(worktree_path)
        .output()
        .map_err(|e| ZError::Worktree(format!("git status failed: {}", e)))?;

    if !status_output.status.success() {
        let stderr = String::from_utf8_lossy(&status_output.stderr);
        return Err(ZError::Worktree(format!(
            "git status failed: {}",
            stderr.trim()
        )));
    }

    let dirty = !status_output.stdout.is_empty();

    // Check ahead/behind via `git rev-list --left-right --count HEAD...@{u}`.
    // If no upstream is configured, the command fails — we treat that as
    // has_upstream = false.
    let rev_output = Command::new("git")
        .args(["rev-list", "--left-right", "--count", "HEAD...@{u}"])
        .current_dir(worktree_path)
        .output()
        .map_err(|e| ZError::Worktree(format!("git rev-list failed: {}", e)))?;

    let (ahead, behind, has_upstream) = if rev_output.status.success() {
        let stdout = String::from_utf8_lossy(&rev_output.stdout);
        let (ahead, behind) = parse_ahead_behind_strict(&stdout)?;
        (ahead, behind, true)
    } else {
        let stderr = String::from_utf8_lossy(&rev_output.stderr);
        if is_no_upstream_error(&stderr) {
            (0, 0, false)
        } else {
            return Err(ZError::Worktree(format!(
                "git rev-list failed: {}",
                stderr.trim()
            )));
        }
    };

    Ok(GitSafetyStatus {
        dirty,
        ahead,
        behind,
        has_upstream,
    })
}

fn is_no_upstream_error(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("no upstream")
        || lower.contains("no such branch: '@{u}'")
        || lower.contains("no such branch: \"@{u}\"")
}

fn parse_ahead_behind_strict(output: &str) -> Result<(u32, u32)> {
    let mut parts = output.split_whitespace();
    let ahead = parts
        .next()
        .ok_or_else(|| ZError::Worktree("git rev-list returned empty output".to_string()))?
        .parse::<u32>()
        .map_err(|e| ZError::Worktree(format!("invalid git ahead count: {e}")))?;
    let behind = parts
        .next()
        .ok_or_else(|| ZError::Worktree("git rev-list returned missing behind count".to_string()))?
        .parse::<u32>()
        .map_err(|e| ZError::Worktree(format!("invalid git behind count: {e}")))?;
    Ok((ahead, behind))
}

/// Parse `git rev-list --left-right --count HEAD...@{u}` output.
///
/// Format: `{ahead}\t{behind}\n`
fn parse_ahead_behind(output: &str) -> (u32, u32) {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return (0, 0);
    }
    let mut parts = trimmed.split_whitespace();
    let ahead = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let behind = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    (ahead, behind)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Old parser backward compat ----

    #[test]
    fn parse_empty_output() {
        let worktrees = parse_git_worktree_porcelain("", "myapp");
        assert!(worktrees.is_empty());
    }

    #[test]
    fn parse_single_main_worktree() {
        let output = "worktree /home/user/myapp\nHEAD abc123\nbranch refs/heads/main\n";
        let worktrees = parse_git_worktree_porcelain(output, "myapp");
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].path, PathBuf::from("/home/user/myapp"));
        assert_eq!(worktrees[0].branch, "main");
        assert_eq!(worktrees[0].project, "myapp");
    }

    #[test]
    fn parse_multiple_worktrees() {
        let output = concat!(
            "worktree /home/user/myapp\n",
            "HEAD abc123\n",
            "branch refs/heads/main\n",
            "\n",
            "worktree /home/user/myapp-feat-login\n",
            "HEAD def456\n",
            "branch refs/heads/feat/login\n",
        );
        let worktrees = parse_git_worktree_porcelain(output, "myapp");
        assert_eq!(worktrees.len(), 2);
        assert_eq!(worktrees[0].branch, "main");
        assert_eq!(worktrees[1].branch, "feat/login");
        assert_eq!(
            worktrees[1].path,
            PathBuf::from("/home/user/myapp-feat-login")
        );
    }

    #[test]
    fn parse_skips_detached_head() {
        let output = concat!(
            "worktree /home/user/myapp\n",
            "HEAD abc123\n",
            "branch refs/heads/main\n",
            "\n",
            "worktree /home/user/myapp-detached\n",
            "HEAD deadbeef\n",
            "detached\n",
        );
        let worktrees = parse_git_worktree_porcelain(output, "myapp");
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].branch, "main");
    }

    #[test]
    fn parse_branch_with_slashes() {
        let output = concat!(
            "worktree /home/user/myapp-feat-user-auth\n",
            "HEAD abc123\n",
            "branch refs/heads/feat/user/auth\n",
        );
        let worktrees = parse_git_worktree_porcelain(output, "myapp");
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].branch, "feat/user/auth");
    }

    #[test]
    fn find_worktree_path_for_sanitized_branch() {
        let worktrees = vec![
            Worktree {
                path: PathBuf::from("/repo/main"),
                branch: "main".to_string(),
                project: "myapp".to_string(),
            },
            Worktree {
                path: PathBuf::from("/repo/feat-login"),
                branch: "feat/login".to_string(),
                project: "myapp".to_string(),
            },
        ];

        assert_eq!(
            find_worktree_path_for_branch(&worktrees, "feat-login"),
            Some(PathBuf::from("/repo/feat-login"))
        );
    }

    #[test]
    fn find_worktree_path_returns_none_for_missing_branch() {
        let worktrees = vec![Worktree {
            path: PathBuf::from("/repo/main"),
            branch: "main".to_string(),
            project: "myapp".to_string(),
        }];

        assert!(find_worktree_path_for_branch(&worktrees, "missing").is_none());
    }

    #[test]
    fn parse_sets_project_name() {
        let output = "worktree /home/user/proj\nHEAD abc\nbranch refs/heads/dev\n";
        let worktrees = parse_git_worktree_porcelain(output, "myproject");
        assert_eq!(worktrees[0].project, "myproject");
    }

    #[test]
    fn parse_no_blank_lines_between_entries() {
        // git worktree list --porcelain sometimes omits blank lines on older git versions.
        let output = concat!(
            "worktree /path/a\n",
            "HEAD aaa\n",
            "branch refs/heads/branchA\n",
            "worktree /path/b\n",
            "HEAD bbb\n",
            "branch refs/heads/branchB\n",
        );
        let worktrees = parse_git_worktree_porcelain(output, "proj");
        assert_eq!(worktrees.len(), 2);
        assert_eq!(worktrees[0].branch, "branchA");
        assert_eq!(worktrees[1].branch, "branchB");
    }

    #[test]
    fn parse_path_with_spaces() {
        let output =
            "worktree /home/user/my projects/myapp\nHEAD abc\nbranch refs/heads/main\n";
        let worktrees = parse_git_worktree_porcelain(output, "myapp");
        assert_eq!(
            worktrees[0].path,
            PathBuf::from("/home/user/my projects/myapp")
        );
    }

    #[test]
    fn parse_skips_bare_worktree() {
        let output = concat!(
            "worktree /home/user/myapp.git\n",
            "HEAD abc123\n",
            "bare\n",
            "\n",
            "worktree /home/user/myapp-main\n",
            "HEAD def456\n",
            "branch refs/heads/main\n",
        );
        let worktrees = parse_git_worktree_porcelain(output, "myapp");
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].branch, "main");
    }

    #[test]
    fn parse_worktree_only_header_no_branch() {
        let output = "worktree /path/to/wt\nHEAD abc123\n";
        let worktrees = parse_git_worktree_porcelain(output, "proj");
        assert!(worktrees.is_empty());
    }

    #[test]
    fn create_worktree_fails_when_fetch_fails() {
        let mgr = WtWorktreeManager::new(PathBuf::from("/nonexistent/path"));
        let result = mgr.create_worktree("proj", "feat-test");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("git fetch"),
            "expected error about git fetch, got: {err}"
        );
    }

    #[test]
    fn parse_ignores_unknown_lines() {
        let output = concat!(
            "worktree /path/a\n",
            "HEAD abc\n",
            "branch refs/heads/main\n",
            "prunable gitdir file points to non-existent location\n",
        );
        let worktrees = parse_git_worktree_porcelain(output, "proj");
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].branch, "main");
    }

    // ---- Detailed parser tests ----

    #[test]
    fn detailed_detects_primary_checkout() {
        let output = concat!(
            "worktree /repo\n",
            "HEAD abc\n",
            "branch refs/heads/main\n",
            "\n",
            "worktree /repo/.worktrees/feat\n",
            "HEAD def\n",
            "branch refs/heads/feat/x\n",
        );
        let entries =
            parse_git_worktree_porcelain_detailed(output, "myapp", &PathBuf::from("/repo"), None);
        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_primary_checkout);
        assert_eq!(entries[0].branch.as_deref(), Some("main"));
        assert!(!entries[1].is_primary_checkout);
        assert_eq!(entries[1].branch.as_deref(), Some("feat/x"));
    }

    #[test]
    fn detailed_includes_detached_head() {
        let output = concat!(
            "worktree /repo\n",
            "HEAD abc\n",
            "branch refs/heads/main\n",
            "\n",
            "worktree /repo/.worktrees/detached\n",
            "HEAD dead\n",
            "detached\n",
        );
        let entries =
            parse_git_worktree_porcelain_detailed(output, "myapp", &PathBuf::from("/repo"), None);
        assert_eq!(entries.len(), 2);
        // Detached entry is present but has no branch
        assert!(entries[1].branch.is_none());
        assert!(!entries[1].is_primary_checkout);
    }

    #[test]
    fn detailed_all_entries_including_bare() {
        let output = concat!(
            "worktree /repo\n",
            "HEAD abc\n",
            "branch refs/heads/main\n",
            "\n",
            "worktree /repo.git\n",
            "HEAD dead\n",
            "bare\n",
        );
        let entries =
            parse_git_worktree_porcelain_detailed(output, "myapp", &PathBuf::from("/repo"), None);
        assert_eq!(entries.len(), 2);
        assert!(entries[0].branch.is_some());
        assert!(entries[1].branch.is_none());
    }

    #[test]
    fn detailed_sets_host_and_identity() {
        let output = "worktree /repo/.worktrees/feat\nHEAD abc\nbranch refs/heads/feat\n";
        let entries = parse_git_worktree_porcelain_detailed(
            output,
            "myapp",
            &PathBuf::from("/repo"),
            Some("myserver"),
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].identity.host.as_deref(),
            Some("myserver")
        );
        assert_eq!(entries[0].identity.project_root, PathBuf::from("/repo"));
        assert_eq!(
            entries[0].identity.worktree_path,
            PathBuf::from("/repo/.worktrees/feat")
        );
    }

    #[test]
    fn detailed_supports_spaces_in_path() {
        let output = "worktree /home/user/my project\nHEAD abc\nbranch refs/heads/main\n";
        let entries = parse_git_worktree_porcelain_detailed(
            output,
            "myapp",
            &PathBuf::from("/home/user/my project"),
            None,
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].identity.worktree_path,
            PathBuf::from("/home/user/my project")
        );
        assert!(entries[0].is_primary_checkout);
    }

    // ---- Old parser backward compat via blocks ----

    #[test]
    fn old_parser_derives_from_blocks() {
        // Verify the old parser's output matches filtering the detailed output
        let output = concat!(
            "worktree /repo\n",
            "HEAD abc\n",
            "branch refs/heads/main\n",
            "\n",
            "worktree /repo/.worktrees/detached\n",
            "HEAD def\n",
            "detached\n",
        );
        let old = parse_git_worktree_porcelain(output, "myapp");
        let detailed =
            parse_git_worktree_porcelain_detailed(output, "myapp", &PathBuf::from("/repo"), None);
        let filtered: Vec<&DiscoveredWorktree> = detailed.iter().filter(|d| d.branch.is_some()).collect();
        assert_eq!(old.len(), filtered.len());
        assert_eq!(old[0].branch, filtered[0].branch.as_deref().unwrap());
        assert_eq!(old[0].path, filtered[0].identity.worktree_path);
    }

    // ---- parse_ahead_behind ----

    #[test]
    fn parse_ahead_behind_typical() {
        assert_eq!(parse_ahead_behind("5\t3\n"), (5, 3));
    }

    #[test]
    fn parse_ahead_behind_zero() {
        assert_eq!(parse_ahead_behind("0\t0\n"), (0, 0));
    }

    #[test]
    fn parse_ahead_behind_empty() {
        assert_eq!(parse_ahead_behind(""), (0, 0));
    }

    #[test]
    fn parse_ahead_behind_whitespace_only() {
        assert_eq!(parse_ahead_behind("  \n"), (0, 0));
    }

    #[test]
    fn parse_ahead_behind_strict_rejects_empty_output() {
        assert!(parse_ahead_behind_strict("").is_err());
    }

    #[test]
    fn parse_ahead_behind_strict_rejects_invalid_counts() {
        assert!(parse_ahead_behind_strict("abc\t1\n").is_err());
    }

    #[test]
    fn detects_no_upstream_errors() {
        assert!(is_no_upstream_error("fatal: no upstream configured for branch 'main'"));
        assert!(is_no_upstream_error("fatal: no such branch: '@{u}'"));
        assert!(!is_no_upstream_error("fatal: not a git repository"));
    }

    // ---- discover_primary_branch / check_git_safety (process-backed) ----

    fn init_temp_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("create temp dir");
        let repo_path = dir.path().to_path_buf();

        let git_init = Command::new("git")
            .args(["init"])
            .current_dir(&repo_path)
            .output()
            .expect("git init");
        assert!(git_init.status.success());

        // Set user config for commits
        Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(&repo_path)
            .output()
            .ok();
        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(&repo_path)
            .output()
            .ok();

        // Initial commit on the default branch
        std::fs::write(repo_path.join("README.md"), "# Test").ok();
        let add = Command::new("git")
            .args(["add", "README.md"])
            .current_dir(&repo_path)
            .output()
            .expect("git add");
        assert!(add.status.success());

        let commit = Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(&repo_path)
            .output()
            .expect("git commit");
        assert!(commit.status.success());

        (dir, repo_path)
    }

    #[test]
    fn discover_primary_branch_detects_default_branch() {
        let (_dir, repo_path) = init_temp_repo();
        let branch = discover_primary_branch(&repo_path).expect("discover primary branch");
        assert!(
            branch == "main" || branch == "master",
            "expected 'main' or 'master' as default branch, got: {branch}",
        );
    }

    #[test]
    fn discover_primary_branch_fails_on_non_repo() {
        let result = discover_primary_branch(&std::path::Path::new("/nonexistent"));
        assert!(result.is_err());
    }

    #[test]
    fn check_git_safety_clean_no_upstream() {
        let (_dir, repo_path) = init_temp_repo();
        let safety = check_git_safety(&repo_path).expect("check safety");
        // Repo is clean, no upstream configured
        assert!(!safety.dirty);
        assert_eq!(safety.ahead, 0);
        assert_eq!(safety.behind, 0);
        assert!(!safety.has_upstream);
    }

    #[test]
    fn check_git_safety_detects_dirty() {
        let (_dir, repo_path) = init_temp_repo();
        // Make an uncommitted change
        std::fs::write(repo_path.join("dirty.txt"), "dirty").ok();
        let safety = check_git_safety(&repo_path).expect("check safety");
        assert!(safety.dirty);
    }

    #[test]
    fn check_git_safety_fails_on_non_repo() {
        let result = check_git_safety(&std::path::Path::new("/nonexistent"));
        assert!(result.is_err());
    }
}
