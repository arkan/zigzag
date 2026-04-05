mod config_store;
mod depcheck_impl;
mod session_manager;
mod worktree_manager;

use z_core::depcheck::{check_deps, format_dep_error, DepCheckStatus};
use z_core::traits::{ProjectStore, SessionManager, WorktreeManager};

use crate::config_store::KdlProjectStore;
use crate::depcheck_impl::ProcessDepChecker;
use crate::session_manager::ZellijSessionManager;
use crate::worktree_manager::WtWorktreeManager;

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
            eprintln!("TUI mode not yet implemented (phase 1b).");
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
        Some(cmd) => {
            eprintln!("CLI command not yet implemented: {:?}", cmd);
        }
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

    let mut layout = z_core::layout::default_layout();
    layout.cwd = Some(cwd);
    session_mgr.create_session(&project.name, effective_branch, layout)?;

    Ok(())
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
