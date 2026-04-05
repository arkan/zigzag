mod config_store;
mod depcheck_impl;
mod prune;
mod session_manager;
mod worktree_manager;

use std::io::Write as _;

use std::fs;

use z_core::config::{effective_layout, parse_global_config_kdl, parse_per_repo_config_kdl,
    GlobalConfig, PerRepoConfig};
use z_core::depcheck::{check_deps, format_dep_error, DepCheckStatus};
use z_core::domain::Session;
use z_core::traits::{ProjectStore, SessionManager, WorktreeManager};

use crate::config_store::KdlProjectStore;
use crate::depcheck_impl::ProcessDepChecker;
use crate::session_manager::{parse_session_name, ZellijSessionManager};
use crate::worktree_manager::WtWorktreeManager;

use z_tui::{Navigation, ProjectEntry, TuiAction};

fn main() {
    let checker = ProcessDepChecker;
    let results = check_deps(&checker);

    let mut failed = false;
    for result in &results {
        match &result.status {
            DepCheckStatus::Ok { version } => {
                eprintln!("  ✓ {} {}", result.tool, version);
            }
            _ => {
                eprintln!("{}", format_dep_error(result));
                failed = true;
            }
        }
    }

    if failed {
        eprintln!("\nz requires all dependencies to be installed. Aborting.");
        std::process::exit(1);
    }

    run();
}

fn run() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        None => {
            if let Err(e) = cmd_tui() {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        }
        Some("list") => {
            if let Err(e) = cmd_list() {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        }
        Some("open") => {
            let project = args.get(1).map(|s| s.as_str()).unwrap_or("");
            if project.is_empty() {
                eprintln!("usage: z open <project> [branch]");
                std::process::exit(1);
            }
            let branch = args.get(2).map(|s| s.as_str());
            if let Err(e) = cmd_open(project, branch) {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        }
        Some("close") => {
            let session = args.get(1).map(|s| s.as_str());
            if let Err(e) = cmd_close(session) {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        }
        Some("delete") => {
            let session = args.get(1).map(|s| s.as_str()).unwrap_or("");
            if session.is_empty() {
                eprintln!("usage: z delete <project:branch>");
                std::process::exit(1);
            }
            if let Err(e) = cmd_delete(session) {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        }
        Some("prune") => {
            let dry_run = args.iter().any(|a| a == "--dry-run");
            if let Err(e) = cmd_prune(dry_run) {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        }
        Some(cmd) => {
            eprintln!("CLI command not yet implemented: {:?}", cmd);
        }
    }
}

/// Launch the interactive TUI and execute whatever action the user chooses.
fn cmd_tui() -> z_core::error::Result<()> {
    let store = KdlProjectStore::new();
    let session_mgr = ZellijSessionManager;
    let global = load_global_config();

    let projects = store.list_projects()?;

    let mut entries: Vec<ProjectEntry> = Vec::with_capacity(projects.len());
    for project in &projects {
        let sessions = session_mgr.list_sessions(&project.name)?;
        entries.push(ProjectEntry { project: project.clone(), sessions });
    }

    let navigation = match global.navigation.as_deref() {
        Some("vim") => Navigation::Vim,
        _ => Navigation::Arrows,
    };

    let action = z_tui::run_tui(entries, navigation)
        .map_err(|e| z_core::error::ZError::Io(e.to_string()))?;

    match action {
        TuiAction::Quit => {}

        TuiAction::Open { project, session } => {
            // Extract branch from "project:branch" session name, or open default.
            let branch_owned: Option<String> = session
                .as_deref()
                .and_then(|s| parse_session_name(s))
                .map(|(_, b)| b);
            cmd_open(&project, branch_owned.as_deref())?;
        }

        TuiAction::New { project } => {
            cmd_open(&project, None)?;
        }

        TuiAction::Delete { session } => {
            cmd_delete(&session)?;
        }

        TuiAction::Prune => {
            eprintln!("prune: not yet implemented");
        }

        TuiAction::Autopilot { project: _ } => {
            eprintln!("autopilot: not yet implemented");
        }
    }

    Ok(())
}

fn load_global_config() -> GlobalConfig {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let path = std::path::PathBuf::from(home)
        .join(".config")
        .join("z")
        .join("config.kdl");
    match fs::read_to_string(&path) {
        Ok(content) => parse_global_config_kdl(&content).unwrap_or_default(),
        Err(_) => GlobalConfig::default(),
    }
}

fn load_per_repo_config(project_path: &std::path::Path) -> PerRepoConfig {
    let path = project_path.join(".config").join("z.kdl");
    match fs::read_to_string(&path) {
        Ok(content) => parse_per_repo_config_kdl(&content).unwrap_or_default(),
        Err(_) => PerRepoConfig::default(),
    }
}

fn cmd_open(project_name: &str, branch: Option<&str>) -> z_core::error::Result<()> {
    let store = KdlProjectStore::new();
    let session_mgr = ZellijSessionManager;

    // Resolve project — returns ProjectNotFound if not in config.
    let project = store.get_project(project_name)?;

    let effective_branch = branch.unwrap_or("main");

    // Build the expected session name (branch "/" → "-" normalization applied).
    let target_session = z_core::domain::Session::new(&project.name, effective_branch);

    // Check for an existing live session.
    let sessions = session_mgr.list_sessions(&project.name)?;
    if let Some(existing) = sessions.iter().find(|s| s.name == target_session.name) {
        return session_mgr.attach_session(existing);
    }

    // Session doesn't exist — create it.
    let cwd = if let Some(branch_name) = branch {
        // Branch specified: find or create the worktree.
        let wt_mgr = WtWorktreeManager::new(project.path.clone());
        let worktrees = wt_mgr.list_worktrees(&project.name)?;
        let worktree_path = if let Some(existing_wt) =
            worktrees.iter().find(|w| w.branch == branch_name)
        {
            // Worktree already exists — just reuse its path.
            existing_wt.path.clone()
        } else {
            // Create new worktree via `wt switch -c <branch>`.
            let new_wt = wt_mgr.create_worktree(&project.name, branch_name)?;
            new_wt.path
        };
        worktree_path
    } else {
        // No branch — open in the project root.
        project.path.clone()
    };

    // Merge global + per-repo config and create with effective layout.
    let global = load_global_config();
    let per_repo = load_per_repo_config(&project.path);
    let mut layout = effective_layout(&global, &per_repo);
    layout.cwd = Some(cwd);
    session_mgr.create_session(&project.name, effective_branch, layout)?;

    Ok(())
}

/// Detach from a Zellij session, keeping it running in the background.
///
/// `session_name` — if `None`, detects the current session from `ZELLIJ_SESSION_NAME`.
fn cmd_close(session_name: Option<&str>) -> z_core::error::Result<()> {
    let session_mgr = ZellijSessionManager;

    let name = match session_name {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => std::env::var("ZELLIJ_SESSION_NAME").map_err(|_| {
            z_core::error::ZError::Session(
                "no session specified and not inside a Zellij session \
                 (ZELLIJ_SESSION_NAME not set)"
                    .to_string(),
            )
        })?,
    };

    let (project, branch) =
        parse_session_name(&name).ok_or_else(|| {
            z_core::error::ZError::Session(format!(
                "invalid session name {:?}: expected project:branch",
                name
            ))
        })?;

    let session = Session { name: name.clone(), project, branch };
    session_mgr.detach_session(&session)?;
    println!("Detached from session: {}", name);
    Ok(())
}

/// Kill a Zellij session and optionally remove its worktree.
fn cmd_delete(session_name: &str) -> z_core::error::Result<()> {
    let session_mgr = ZellijSessionManager;

    let (project, branch) =
        parse_session_name(session_name).ok_or_else(|| {
            z_core::error::ZError::Session(format!(
                "invalid session name {:?}: expected project:branch",
                session_name
            ))
        })?;

    let session = Session {
        name: session_name.to_string(),
        project,
        branch: branch.clone(),
    };

    session_mgr.kill_session(&session)?;
    println!("Session {} killed.", session_name);

    // Prompt user to optionally remove the worktree.
    eprint!("Delete worktree {}? (y/N) ", branch);
    let _ = std::io::stderr().flush();

    let mut response = String::new();
    std::io::stdin()
        .read_line(&mut response)
        .map_err(|e| z_core::error::ZError::Io(e.to_string()))?;

    if parse_confirm_response(&response) {
        remove_worktree(&branch)?;
        println!("Worktree {} removed.", branch);
    } else {
        println!("Worktree kept.");
    }

    Ok(())
}

/// Returns `true` if the user typed "y" or "Y".
pub fn parse_confirm_response(response: &str) -> bool {
    matches!(response.trim().to_lowercase().as_str(), "y")
}

/// Shell out to `wt remove <branch>` to remove a worktree.
fn remove_worktree(branch: &str) -> z_core::error::Result<()> {
    let status = std::process::Command::new("wt")
        .args(["remove", branch])
        .status()
        .map_err(|e| z_core::error::ZError::Worktree(e.to_string()))?;
    if !status.success() {
        return Err(z_core::error::ZError::Worktree(format!(
            "wt remove exited with status {}",
            status
        )));
    }
    Ok(())
}

/// Clean up orphaned Zellij sessions and worktrees across all projects.
///
/// A session is orphaned when no worktree exists for its branch.
/// A worktree is orphaned when no active session exists for its branch
/// (main/master worktrees are always excluded).
///
/// Passes `--dry-run` to preview what would be cleaned without acting.
fn cmd_prune(dry_run: bool) -> z_core::error::Result<()> {
    let store = KdlProjectStore::new();
    let session_mgr = ZellijSessionManager;

    let projects = store.list_projects()?;

    let mut all_orphaned_sessions: Vec<z_core::domain::Session> = Vec::new();
    let mut all_orphaned_worktrees: Vec<(z_core::domain::Worktree, std::path::PathBuf)> =
        Vec::new();

    for project in &projects {
        let wt_mgr = WtWorktreeManager::new(project.path.clone());
        let sessions = session_mgr.list_sessions(&project.name)?;
        let worktrees = wt_mgr.list_worktrees(&project.name)?;

        let orphaned_sessions = prune::find_orphaned_sessions(&sessions, &worktrees);
        let orphaned_worktrees = prune::find_orphaned_worktrees(&worktrees, &sessions);

        all_orphaned_sessions.extend(orphaned_sessions);
        for wt in orphaned_worktrees {
            all_orphaned_worktrees.push((wt, project.path.clone()));
        }
    }

    if all_orphaned_sessions.is_empty() && all_orphaned_worktrees.is_empty() {
        println!("Nothing to prune.");
        return Ok(());
    }

    if !all_orphaned_sessions.is_empty() {
        println!("Orphaned sessions (no matching worktree):");
        for session in &all_orphaned_sessions {
            println!("  - {}", session.name);
        }
    }

    if !all_orphaned_worktrees.is_empty() {
        println!("Orphaned worktrees (no active session):");
        for (wt, _) in &all_orphaned_worktrees {
            println!("  - {} ({})", wt.branch, wt.path.display());
        }
    }

    if dry_run {
        println!("\nDry run — no changes made.");
        return Ok(());
    }

    eprint!("\nProceed with cleanup? (y/N) ");
    let _ = std::io::stderr().flush();

    let mut response = String::new();
    std::io::stdin()
        .read_line(&mut response)
        .map_err(|e| z_core::error::ZError::Io(e.to_string()))?;

    if !parse_confirm_response(&response) {
        println!("Aborted.");
        return Ok(());
    }

    let mut killed = 0usize;
    let mut removed = 0usize;

    for session in &all_orphaned_sessions {
        session_mgr.kill_session(session)?;
        println!("Killed session: {}", session.name);
        killed += 1;
    }

    for (wt, project_path) in &all_orphaned_worktrees {
        let wt_mgr = WtWorktreeManager::new(project_path.clone());
        wt_mgr.remove_worktree(wt)?;
        println!("Removed worktree: {}", wt.branch);
        removed += 1;
    }

    println!(
        "\nPrune complete: {} session(s) killed, {} worktree(s) removed.",
        killed, removed
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirm_response_y_returns_true() {
        assert!(parse_confirm_response("y"));
    }

    #[test]
    fn confirm_response_y_uppercase_returns_true() {
        assert!(parse_confirm_response("Y"));
    }

    #[test]
    fn confirm_response_y_with_newline_returns_true() {
        assert!(parse_confirm_response("y\n"));
    }

    #[test]
    fn confirm_response_n_returns_false() {
        assert!(!parse_confirm_response("n"));
    }

    #[test]
    fn confirm_response_empty_returns_false() {
        assert!(!parse_confirm_response(""));
    }

    #[test]
    fn confirm_response_yes_returns_false() {
        // Only single "y" is accepted, not "yes"
        assert!(!parse_confirm_response("yes"));
    }

    #[test]
    fn confirm_response_whitespace_only_returns_false() {
        assert!(!parse_confirm_response("   "));
    }

    #[test]
    fn confirm_response_y_with_surrounding_whitespace_returns_true() {
        assert!(parse_confirm_response("  y  "));
    }

    #[test]
    fn confirm_response_y_with_crlf_returns_true() {
        assert!(parse_confirm_response("y\r\n"));
    }

    #[test]
    fn confirm_response_n_uppercase_returns_false() {
        assert!(!parse_confirm_response("N"));
    }

    #[test]
    fn confirm_response_random_text_returns_false() {
        assert!(!parse_confirm_response("maybe"));
    }
}

fn cmd_list() -> z_core::error::Result<()> {
    let store = KdlProjectStore::new();
    let session_mgr = ZellijSessionManager;

    let projects = store.list_projects()?;

    if projects.is_empty() {
        println!("No projects found. Add projects to ~/.config/z/projects.kdl");
        return Ok(());
    }

    println!("Projects:\n");

    for project in &projects {
        let sessions = session_mgr.list_sessions(&project.name)?;

        let remote_indicator = if project.host.is_some() { " 🌐" } else { "" };

        if sessions.is_empty() {
            println!("  {}{}", project.name, remote_indicator);
        } else {
            println!("  {}{}  ●", project.name, remote_indicator);
            for session in &sessions {
                println!("    └─ {}", session.name);
            }
        }
    }

    Ok(())
}
