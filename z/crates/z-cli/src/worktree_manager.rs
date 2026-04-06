use std::path::PathBuf;
use std::process::Command;

use z_core::domain::Worktree;
use z_core::error::{Result, ZError};
use z_core::traits::WorktreeManager;

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
        // Use wt switch -c to create the worktree (worktrunk convention).
        let status = Command::new("wt")
            .args(["switch", "-c", branch])
            .current_dir(&self.project_path)
            .status()
            .map_err(|e| ZError::Worktree(format!("wt switch failed: {}", e)))?;

        if !status.success() {
            return Err(ZError::Worktree(format!(
                "wt switch -c {} exited with status {}",
                branch, status
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
/// The main worktree (first entry) is included. Detached HEAD entries (no `branch` line)
/// are skipped.
pub fn parse_git_worktree_porcelain(output: &str, project: &str) -> Vec<Worktree> {
    let mut worktrees = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_branch: Option<String> = None;

    for line in output.lines() {
        if line.starts_with("worktree ") {
            // Flush previous entry if complete.
            if let (Some(path), Some(branch)) = (current_path.take(), current_branch.take()) {
                worktrees.push(Worktree {
                    path,
                    branch,
                    project: project.to_string(),
                });
            }
            current_path = Some(PathBuf::from(&line["worktree ".len()..]));
            current_branch = None;
        } else if let Some(refs) = line.strip_prefix("branch refs/heads/") {
            current_branch = Some(refs.to_string());
        }
    }

    // Flush last entry.
    if let (Some(path), Some(branch)) = (current_path, current_branch) {
        worktrees.push(Worktree {
            path,
            branch,
            project: project.to_string(),
        });
    }

    worktrees
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // Bare repos produce a "bare" line instead of "branch refs/heads/..."
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
        // A worktree entry with only the path line and HEAD, no branch
        let output = "worktree /path/to/wt\nHEAD abc123\n";
        let worktrees = parse_git_worktree_porcelain(output, "proj");
        assert!(worktrees.is_empty());
    }

    #[test]
    fn parse_ignores_unknown_lines() {
        // Future git versions might add extra fields
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
}
