mod config_store;
mod depcheck_impl;
mod log;
mod notify;
mod prune;
mod remote;
mod session_manager;
mod worktree_manager;

use std::collections::HashSet;
use std::io::{self, Write as _};

use std::fs;

use z_core::config::{effective_layout, parse_global_config_kdl, parse_per_repo_config_kdl,
    GlobalConfig, PerRepoConfig};
use z_core::depcheck::{check_deps, format_dep_error, DepCheckStatus};
use z_core::domain::{NotifyLevel, Session};
use z_core::traits::{ProjectStore, ProjectStoreWriter, SessionManager, WorktreeManager, Notifier};

use z_autopilot::builtin::builtin_workflows;
use z_autopilot::dsl::AutopilotWorkflow;
use z_autopilot::persist::list_runs;
use z_autopilot::state::{WorkflowRun, WorkflowStatus};

use crate::config_store::KdlProjectStore;
use crate::depcheck_impl::ProcessDepChecker;
use crate::notify::DispatchNotifier;
use crate::session_manager::{
    list_all_z_sessions_with_ages, parse_session_name, ZellijSessionManager,
};
use crate::worktree_manager::WtWorktreeManager;

use z_tui::{Navigation, ProjectEntry, TuiAction, WorkflowInfo};

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
        Some("notify") => {
            // Usage: z notify <session> <message> [--level info|warning|error]
            let session = args.get(1).map(|s| s.as_str()).unwrap_or("");
            let message = args.get(2).map(|s| s.as_str()).unwrap_or("");
            if session.is_empty() || message.is_empty() {
                eprintln!("usage: z notify <session> <message> [--level info|warning|error]");
                std::process::exit(1);
            }
            let level = parse_notify_level(args.iter().skip(3));
            if let Err(e) = cmd_notify(session, message, level) {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        }
        Some("autopilot") => {
            let sub = args.get(1).map(|s| s.as_str());
            if let Err(e) = cmd_autopilot_dispatch(sub, &args[1..]) {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        }
        Some("logs") => {
            let n: usize = args
                .iter()
                .position(|a| a == "-n")
                .and_then(|i| args.get(i + 1))
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            if let Err(e) = cmd_logs(n) {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        }
        Some("switch") => {
            if let Err(e) = cmd_switch() {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        }
        Some(cmd) => {
            eprintln!("unknown command: {:?}", cmd);
            eprintln!("usage: z [list|open|close|delete|prune|notify|autopilot|logs|switch]");
            std::process::exit(1);
        }
    }
}

/// Launch the interactive TUI and execute whatever action the user chooses.
/// Loops back into the TUI after adding a project so the user stays in context.
fn cmd_tui() -> z_core::error::Result<()> {
    let mut store = KdlProjectStore::new();
    let session_mgr = ZellijSessionManager;
    let global = load_global_config();

    let navigation = match global.navigation.as_deref() {
        Some("vim") => Navigation::Vim,
        _ => Navigation::Arrows,
    };

    // Track the name of the most recently added project for auto-selection.
    let mut initial_project: Option<String> = None;
    let mut status_message: Option<String> = None;
    let logger = log::FileLogger::new();

    // Load built-in workflows once; they are the same for every project.
    let builtin: Vec<AutopilotWorkflow> = builtin_workflows().unwrap_or_default();

    loop {
        let projects = store.list_projects()?;

        let mut entries: Vec<ProjectEntry> = Vec::with_capacity(projects.len());
        for project in &projects {
            let sessions = session_mgr.list_sessions(&project.name)?;
            let worktree_count = WtWorktreeManager::new(project.path.clone())
                .list_worktrees(&project.name)
                .map(|wts| wts.len())
                .unwrap_or(0);

            // Combine built-in workflows with any per-repo custom workflows.
            let mut all_workflows: Vec<AutopilotWorkflow> = builtin.clone();
            let repo_config_path = project.path.join(".config").join("z.kdl");
            if let Ok(content) = fs::read_to_string(&repo_config_path) {
                if let Ok(custom) = z_autopilot::dsl::parse_autopilot_workflows(&content) {
                    all_workflows.extend(custom);
                }
            }

            let workflows: Vec<WorkflowInfo> = all_workflows
                .iter()
                .map(|wf| WorkflowInfo {
                    name: wf.name.clone(),
                    trigger: wf.trigger.as_str().to_string(),
                    description: wf.description.clone().unwrap_or_default(),
                })
                .collect();

            entries.push(ProjectEntry { project: project.clone(), sessions, worktree_count, workflows });
        }

        // Load pending notifications so the TUI can display 🔔 badges.
        let notifications: HashSet<String> =
            z_core::notification::sessions_with_notifications().into_iter().collect();

        // Auto-select the newly added project if one was just added.
        let initial_idx = initial_project
            .as_deref()
            .and_then(|name| entries.iter().position(|e| e.project.name == name));

        let action = z_tui::run_tui(
            entries,
            navigation.clone(),
            notifications,
            initial_idx,
            status_message.take(),
            |force| prune_summary(force).map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string())),
            |max_lines| {
                let l = log::FileLogger::new();
                let entries = l.read_recent(max_lines);
                Ok(entries.iter().map(|e| e.format()).collect())
            },
        )
        .map_err(|e| z_core::error::ZError::Io(e.to_string()))?;

        match action {
            TuiAction::Quit => return Ok(()),

            TuiAction::AddProject { path, name, host, token } => {
                let project = z_core::domain::Project {
                    name: name.clone(),
                    path: std::path::PathBuf::from(path),
                    host,
                    token,
                };
                store.add_project(&project)?;
                log::log_info(&logger, &format!("project {} added", name));
                initial_project = Some(name);
            }

            TuiAction::Open { project, session } => {
                // Clear notifications for the session being opened.
                if let Some(ref s) = session {
                    let _ = z_core::notification::clear_notifications(s);
                }
                // Extract branch from "project:branch" session name, or open default.
                let branch_owned: Option<String> = session
                    .as_deref()
                    .and_then(|s| parse_session_name(s))
                    .map(|(_, b)| b);
                cmd_open(&project, branch_owned.as_deref())?;
                // Loop back to re-enter the TUI after the session ends,
                // with the same project re-selected.
                initial_project = Some(project);
            }

            TuiAction::New { project, branch } => {
                cmd_open(&project, Some(&branch))?;
                // Loop back to re-enter the TUI after the session ends,
                // with the same project re-selected.
                initial_project = Some(project);
            }

            TuiAction::Delete { session } => {
                // Kill the Zellij session and loop back to the TUI.
                // Worktree cleanup is handled separately via prune.
                let (project_name, _branch) =
                    parse_session_name(&session).ok_or_else(|| {
                        z_core::error::ZError::Session(format!(
                            "invalid session name {:?}: expected project:branch",
                            session
                        ))
                    })?;
                let sess = z_core::domain::Session {
                    name: session.clone(),
                    project: project_name.clone(),
                    branch: _branch,
                };
                match ZellijSessionManager.kill_session(&sess) {
                    Ok(()) => {
                        log::log_info(&logger, &format!("session {} killed", session));
                        status_message = Some(format!("Session {} killed.", session));
                    }
                    Err(e) => {
                        log::log_error(&logger, &format!("session kill failed: {}", e));
                        status_message = Some(format!("Error: {}", e));
                    }
                }
                initial_project = Some(project_name);
            }

            TuiAction::Autopilot { project, workflow: _ } => {
                // Workflow selected — keep the selected project highlighted and
                // loop back into the TUI. Actual workflow execution is handled
                // by the `z autopilot run` subcommand (future work).
                initial_project = Some(project);
                // continue the loop
            }

            TuiAction::EditPerRepoConfig { project_path } => {
                // Edit the config, then loop back to re-enter the TUI.
                cmd_edit_per_repo_config(&project_path)?;
                // continue the loop → re-enter the TUI
            }

            TuiAction::EditProject { original_name, path, name, host, token } => {
                // Remove the old entry by original name, then add the (possibly
                // renamed / updated) project. Using remove+add preserves all
                // surrounding KDL comments while updating the block in-place.
                store.remove_project(&original_name)?;
                let project = z_core::domain::Project {
                    name: name.clone(),
                    path: std::path::PathBuf::from(path),
                    host,
                    token,
                };
                store.add_project(&project)?;
                initial_project = Some(name);
                // Loop back to re-enter TUI with the edited project selected.
            }

            TuiAction::DeleteProject { project } => {
                // Determine nearest-neighbor name before removal so the TUI
                // can auto-select it after the project list reloads.
                let neighbor = {
                    let all = store.list_projects().unwrap_or_default();
                    let idx = all.iter().position(|p| p.name == project).unwrap_or(0);
                    let remaining: Vec<_> = all.iter().filter(|p| p.name != project).collect();
                    if remaining.is_empty() {
                        None
                    } else {
                        let neighbor_idx = idx.min(remaining.len() - 1);
                        remaining.get(neighbor_idx).map(|p| p.name.clone())
                    }
                };
                store.remove_project(&project)?;
                log::log_info(&logger, &format!("project {} deleted", project));
                initial_project = neighbor;
            }
        }
    }
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

fn cmd_edit_per_repo_config(project_path: &std::path::Path) -> z_core::error::Result<()> {
    let config_dir = project_path.join(".config");
    let config_file = config_dir.join("z.kdl");

    // Create .config/ directory if missing (create_dir_all is a no-op if it exists).
    fs::create_dir_all(&config_dir)
        .map_err(|e| z_core::error::ZError::Io(e.to_string()))?;

    // Create the file with a commented template if it doesn't exist.
    if !config_file.exists() {
        let template = "\
// Per-repo z configuration
// Available options are shown below (all optional).

// layout \"compact\"    // override the default layout: \"default\" | \"compact\" | \"minimal\"

// claude {
//   args \"--model\" \"claude-3-7-sonnet-20250219\"   // extra CLI args passed to claude
// }

// deploy {
//   command \"npm run deploy\"   // command run by `z deploy`
// }

// autopilot {
//   auto-push true     // automatically push commits
//   review true        // open a PR after each autopilot session
// }
";
        fs::write(&config_file, template)
            .map_err(|e| z_core::error::ZError::Io(e.to_string()))?;
    }

    // Determine editor: $EDITOR, falling back to vi.
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());

    // Spawn editor and wait for it to exit.
    let status = std::process::Command::new(&editor)
        .arg(&config_file)
        .status()
        .map_err(|e| z_core::error::ZError::Io(format!("failed to launch editor '{}': {}", editor, e)))?;

    if !status.success() {
        eprintln!("editor exited with status: {}", status);
    }

    Ok(())
}

fn cmd_open(project_name: &str, branch: Option<&str>) -> z_core::error::Result<()> {
    let store = KdlProjectStore::new();
    let session_mgr = ZellijSessionManager;

    // Resolve project — returns ProjectNotFound if not in config.
    let project = store.get_project(project_name)?;

    let effective_branch = branch.unwrap_or("main");

    // Remote project: SSH worktree setup + Zellij HTTPS attach.
    if let Some(host) = project.host.clone() {
        return cmd_open_remote(&project, &host, effective_branch);
    }

    // Build the expected session name (branch "/" → "-" normalization applied).
    let target_session = z_core::domain::Session::new(&project.name, effective_branch);

    let logger = log::FileLogger::new();

    // Check for an existing live session.
    let sessions = session_mgr.list_sessions(&project.name)?;
    if let Some(existing) = sessions.iter().find(|s| s.name == target_session.name) {
        log::log_info(&logger, &format!("session {} attached", existing.name));
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
            log::log_info(&logger, &format!("worktree {} created for {}", branch_name, project_name));
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
    log::log_info(&logger, &format!("session {} created", target_session.name));

    Ok(())
}

/// Open a session on a remote project:
/// 1. SSH to the remote host and run `wt switch -c <branch>` to set up the worktree.
/// 2. Attach to the remote Zellij session via HTTPS.
fn cmd_open_remote(
    project: &z_core::domain::Project,
    host: &str,
    branch: &str,
) -> z_core::error::Result<()> {
    let ssh_host = remote::extract_ssh_host(host)?;

    // SSH: set up (or reuse) the worktree on the remote machine.
    let ssh_cmd = format!(
        "cd {} && wt switch -c {}",
        remote::shell_quote(&project.path.display().to_string()),
        remote::shell_quote(branch)
    );
    remote::ssh_run_remote(&ssh_host, &ssh_cmd)?;

    // Build session name and HTTPS attach URL.
    let session = z_core::domain::Session::new(&project.name, branch);
    let url = remote::build_remote_attach_url(host, &session.name);

    // Attach via Zellij HTTPS (with optional token).
    let mut cmd = std::process::Command::new("zellij");
    cmd.args(["attach", &url]);
    if let Some(token) = &project.token {
        if !token.is_empty() {
            cmd.args(["--token", token]);
        }
    }
    let status = cmd
        .status()
        .map_err(|e| z_core::error::ZError::Session(e.to_string()))?;
    if !status.success() {
        return Err(z_core::error::ZError::Session(format!(
            "zellij attach {} failed with status {}",
            url, status
        )));
    }
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
///
/// For remote projects (those with a `host` field), delegates session killing
/// and worktree removal to the remote machine via SSH.
fn cmd_delete(session_name: &str) -> z_core::error::Result<()> {
    let session_mgr = ZellijSessionManager;

    let (project_name, branch) =
        parse_session_name(session_name).ok_or_else(|| {
            z_core::error::ZError::Session(format!(
                "invalid session name {:?}: expected project:branch",
                session_name
            ))
        })?;

    // Look up the project to check if it's remote.
    let store = KdlProjectStore::new();
    let project = store.get_project(&project_name).ok();

    if let Some(proj) = &project {
        if let Some(host) = &proj.host {
            return cmd_delete_remote(proj, host, session_name, &branch);
        }
    }

    // Local session flow.
    let session = Session {
        name: session_name.to_string(),
        project: project_name,
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

/// Kill a remote Zellij session via SSH and optionally remove its remote worktree.
fn cmd_delete_remote(
    project: &z_core::domain::Project,
    host: &str,
    session_name: &str,
    branch: &str,
) -> z_core::error::Result<()> {
    let ssh_host = remote::extract_ssh_host(host)?;

    remote::delete_remote_session(&ssh_host, session_name)?;
    println!("Session {} killed.", session_name);

    // Prompt user to optionally remove the worktree on the remote machine.
    eprint!("Delete remote worktree {}? (y/N) ", branch);
    let _ = std::io::stderr().flush();

    let mut response = String::new();
    std::io::stdin()
        .read_line(&mut response)
        .map_err(|e| z_core::error::ZError::Io(e.to_string()))?;

    if parse_confirm_response(&response) {
        let project_path = project.path.to_string_lossy();
        remote::remove_remote_worktree(&ssh_host, &project_path, branch)?;
        println!("Remote worktree {} removed.", branch);
    } else {
        println!("Remote worktree kept.");
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
        wt_mgr.remove_worktree(wt, true)?;
        println!("Removed worktree: {}", wt.branch);
        removed += 1;
    }

    println!(
        "\nPrune complete: {} session(s) killed, {} worktree(s) removed.",
        killed, removed
    );

    Ok(())
}

/// Non-interactive prune for use in TUI mode.
///
/// Finds and immediately removes orphaned sessions and worktrees without
/// asking for confirmation (the TUI 'p' binding acts as implicit confirmation).
/// Returns a one-line summary string to display in the status bar.
fn prune_summary(force: bool) -> z_core::error::Result<String> {
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

        all_orphaned_sessions
            .extend(prune::find_orphaned_sessions(&sessions, &worktrees));
        for wt in prune::find_orphaned_worktrees(&worktrees, &sessions) {
            all_orphaned_worktrees.push((wt, project.path.clone()));
        }
    }

    if all_orphaned_sessions.is_empty() && all_orphaned_worktrees.is_empty() {
        return Ok("Nothing to prune.".to_string());
    }

    let mut killed = 0usize;
    let mut removed = 0usize;
    let mut skipped = 0usize;
    let logger = log::FileLogger::new();

    for session in &all_orphaned_sessions {
        match session_mgr.kill_session(session) {
            Ok(()) => {
                log::log_info(&logger, &format!("PRUNE KILL {}", session.name));
                killed += 1;
            }
            Err(e) => {
                log::log_error(&logger, &format!("PRUNE ERROR killing {}: {}", session.name, e));
            }
        }
    }

    for (wt, project_path) in &all_orphaned_worktrees {
        let wt_mgr = WtWorktreeManager::new(project_path.clone());
        match wt_mgr.remove_worktree(wt, force) {
            Ok(()) => {
                log::log_info(&logger, &format!("PRUNE REMOVE {}", wt.branch));
                removed += 1;
            }
            Err(e) => {
                log::log_info(&logger, &format!("PRUNE SKIP {}: {}", wt.branch, e));
                skipped += 1;
            }
        }
    }

    let mut msg = format!("Pruned: {} session(s) killed, {} worktree(s) removed.", killed, removed);
    if skipped > 0 {
        msg.push_str(&format!(" {} worktree(s) skipped (uncommitted changes).", skipped));
    }
    log::log_info(&logger, &format!("PRUNE OK {}", msg));
    Ok(msg)
}

// ---------------------------------------------------------------------------
// Logs command
// ---------------------------------------------------------------------------

/// Print log entries. If `n` is 0, print all; otherwise print last `n` lines.
fn cmd_logs(n: usize) -> z_core::error::Result<()> {
    let logger = log::FileLogger::new();
    if n == 0 {
        match logger.read_all() {
            Ok(content) => print!("{}", content),
            Err(_) => println!("No logs found."),
        }
    } else {
        let entries = logger.read_recent(n);
        if entries.is_empty() {
            println!("No logs found.");
        } else {
            for entry in &entries {
                println!("{}", entry.format());
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Switch command
// ---------------------------------------------------------------------------

/// Launch the interactive session switch picker.
///
/// Guard: must be run inside a Zellij session (`$ZELLIJ_SESSION_NAME` set).
/// Lists all z-managed sessions, presents a picker TUI, and if the user
/// selects a session runs `zellij action switch-session <name>`.
fn cmd_switch() -> z_core::error::Result<()> {
    let current_session = std::env::var("ZELLIJ_SESSION_NAME").unwrap_or_default();
    if current_session.is_empty() {
        eprintln!("z switch must be run inside a Zellij session");
        std::process::exit(1);
    }

    let sessions: Vec<(String, Option<String>, usize)> = list_all_z_sessions_with_ages()
        .into_iter()
        .map(|(name, age)| {
            let count = z_core::notification::count_notifications(&name);
            (name, age, count)
        })
        .collect();

    let selected = z_tui::run_switch_picker(sessions, current_session)
        .map_err(|e| z_core::error::ZError::Io(e.to_string()))?;

    if let Some(session_name) = selected {
        let output = std::process::Command::new("zellij")
            .args(["action", "switch-session", &session_name])
            .output()
            .map_err(|e| z_core::error::ZError::Session(e.to_string()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(z_core::error::ZError::Session(format!(
                "zellij action switch-session failed: {}",
                stderr.trim()
            )));
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Notification command
// ---------------------------------------------------------------------------

/// Write a notification for `session` and dispatch it to configured channels.
///
/// This is the integration point for Claude Code hooks and other external
/// triggers. The file is always written (for TUI badge); additional channels
/// (macOS native, Telegram) are dispatched based on `~/.config/z/config.kdl`.
fn cmd_notify(session: &str, message: &str, level: NotifyLevel) -> z_core::error::Result<()> {
    let global = load_global_config();
    let dispatcher = DispatchNotifier::from_config(&global.notifications, session);
    dispatcher.notify(message, level)?;
    Ok(())
}

/// Parse `--level <value>` from an argument iterator.
/// Defaults to `NotifyLevel::Info` when absent or unrecognised.
fn parse_notify_level<'a>(mut args: impl Iterator<Item = &'a String>) -> NotifyLevel {
    while let Some(arg) = args.next() {
        if arg == "--level" {
            if let Some(val) = args.next() {
                return match val.as_str() {
                    "warning" => NotifyLevel::Warning,
                    "error" => NotifyLevel::Error,
                    _ => NotifyLevel::Info,
                };
            }
        }
    }
    NotifyLevel::Info
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Mutex to serialize tests that mutate the EDITOR environment variable,
    /// preventing race conditions when the test suite runs in parallel.
    static EDITOR_MUTEX: Mutex<()> = Mutex::new(());

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

    // ── parse_notify_level tests ──────────────────────────────────────────

    #[test]
    fn parse_notify_level_default_is_info() {
        let args: Vec<String> = vec![];
        assert_eq!(parse_notify_level(args.iter()), NotifyLevel::Info);
    }

    #[test]
    fn parse_notify_level_warning() {
        let args: Vec<String> = vec!["--level".to_string(), "warning".to_string()];
        assert_eq!(parse_notify_level(args.iter()), NotifyLevel::Warning);
    }

    #[test]
    fn parse_notify_level_error() {
        let args: Vec<String> = vec!["--level".to_string(), "error".to_string()];
        assert_eq!(parse_notify_level(args.iter()), NotifyLevel::Error);
    }

    #[test]
    fn parse_notify_level_info_explicit() {
        let args: Vec<String> = vec!["--level".to_string(), "info".to_string()];
        assert_eq!(parse_notify_level(args.iter()), NotifyLevel::Info);
    }

    #[test]
    fn parse_notify_level_unknown_defaults_to_info() {
        let args: Vec<String> = vec!["--level".to_string(), "critical".to_string()];
        assert_eq!(parse_notify_level(args.iter()), NotifyLevel::Info);
    }

    #[test]
    fn parse_notify_level_flag_without_value_defaults_to_info() {
        let args: Vec<String> = vec!["--level".to_string()];
        assert_eq!(parse_notify_level(args.iter()), NotifyLevel::Info);
    }

    // ── format_workflow_list tests ────────────────────────────────────────────

    #[test]
    fn format_workflow_list_empty_returns_no_workflows_message() {
        let output = format_workflow_list(&[]);
        assert_eq!(output, "No workflows available.");
    }

    #[test]
    fn format_workflow_list_includes_header() {
        let wfs = builtin_workflows().unwrap();
        let output = format_workflow_list(&wfs);
        assert!(output.contains("Available workflows:"), "must have header");
    }

    #[test]
    fn format_workflow_list_includes_all_builtin_names() {
        let wfs = builtin_workflows().unwrap();
        let output = format_workflow_list(&wfs);
        assert!(output.contains("pr-ci-fix"));
        assert!(output.contains("pr-review-fix"));
        assert!(output.contains("pr-merge-when-ready"));
        assert!(output.contains("dependabot-auto"));
        assert!(output.contains("deploy-watch"));
        assert!(output.contains("deploy-sync"));
    }

    #[test]
    fn format_workflow_list_includes_trigger() {
        let wfs = builtin_workflows().unwrap();
        let output = format_workflow_list(&wfs);
        assert!(output.contains("post-push"), "must show trigger for pr-ci-fix");
    }

    #[test]
    fn format_workflow_list_includes_description() {
        let wfs = builtin_workflows().unwrap();
        let output = format_workflow_list(&wfs);
        assert!(output.contains("Monitor CI"), "must include pr-ci-fix description");
    }

    // ── format_run_status tests ───────────────────────────────────────────────

    #[test]
    fn format_run_status_empty_returns_no_runs_message() {
        let output = format_run_status(&[]);
        assert_eq!(output, "No active or completed workflow runs.");
    }

    #[test]
    fn format_run_status_includes_header() {
        let run = z_autopilot::state::WorkflowRun::new("pr-ci-fix", "myapp", "monitor-ci");
        let runs = vec![&run];
        let output = format_run_status(&runs);
        assert!(output.contains("Workflow runs:"), "must have header");
    }

    #[test]
    fn format_run_status_shows_project_and_workflow() {
        let run = z_autopilot::state::WorkflowRun::new("pr-ci-fix", "myapp", "monitor-ci");
        let runs = vec![&run];
        let output = format_run_status(&runs);
        assert!(output.contains("myapp"), "must show project name");
        assert!(output.contains("pr-ci-fix"), "must show workflow name");
    }

    #[test]
    fn format_run_status_shows_running_status() {
        let run = z_autopilot::state::WorkflowRun::new("pr-ci-fix", "myapp", "monitor-ci");
        let runs = vec![&run];
        let output = format_run_status(&runs);
        assert!(output.contains("running"), "must show running status");
    }

    #[test]
    fn format_run_status_shows_current_step() {
        let run = z_autopilot::state::WorkflowRun::new("pr-ci-fix", "myapp", "monitor-ci");
        let runs = vec![&run];
        let output = format_run_status(&runs);
        assert!(output.contains("monitor-ci"), "must show current step");
    }

    #[test]
    fn format_run_status_shows_completed_status() {
        use z_autopilot::state::WorkflowStatus;
        let mut run = z_autopilot::state::WorkflowRun::new("pr-ci-fix", "myapp", "monitor-ci");
        run.status = WorkflowStatus::Completed;
        run.current_step = None;
        let runs = vec![&run];
        let output = format_run_status(&runs);
        assert!(output.contains("completed"), "must show completed status");
        assert!(output.contains('-'), "current step must be '-' when None");
    }

    #[test]
    fn format_run_status_shows_failed_status() {
        let mut run = z_autopilot::state::WorkflowRun::new("wf1", "proj", "step1");
        run.status = WorkflowStatus::Failed;
        let runs = vec![&run];
        let output = format_run_status(&runs);
        assert!(output.contains("failed"), "must show failed status");
    }

    #[test]
    fn format_run_status_shows_stuck_status() {
        let mut run = z_autopilot::state::WorkflowRun::new("wf1", "proj", "step1");
        run.status = WorkflowStatus::Stuck;
        let runs = vec![&run];
        let output = format_run_status(&runs);
        assert!(output.contains("stuck"), "must show stuck status");
    }

    #[test]
    fn format_workflow_list_single_with_no_description() {
        use z_autopilot::dsl::{AutopilotWorkflow, Trigger};
        let wf = AutopilotWorkflow {
            name: "test-wf".to_string(),
            description: None,
            trigger: Trigger::Manual,
            poll_interval: None,
            steps: vec![],
            auto_push: None,
            review: None,
        };
        let output = format_workflow_list(&[wf]);
        assert!(output.contains("test-wf"));
        assert!(output.contains("manual"));
    }

    #[test]
    fn format_run_status_multiple_runs() {
        let run1 = z_autopilot::state::WorkflowRun::new("wf1", "proj-a", "step1");
        let run2 = z_autopilot::state::WorkflowRun::new("wf2", "proj-b", "step2");
        let runs = vec![&run1, &run2];
        let output = format_run_status(&runs);
        assert!(output.contains("proj-a"));
        assert!(output.contains("proj-b"));
        assert!(output.contains("wf1"));
        assert!(output.contains("wf2"));
    }

    // ── cmd_edit_per_repo_config tests ────────────────────────────────────────

    fn unique_test_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("z_test_{}", name));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn edit_per_repo_config_creates_config_dir_and_template() {
        let _guard = EDITOR_MUTEX.lock().unwrap();
        let project_path = unique_test_dir("create_template");
        // Neither .config/ nor .config/z.kdl exist yet.
        assert!(!project_path.join(".config").exists());

        // Use "true" as editor so the test doesn't open an actual editor.
        std::env::set_var("EDITOR", "true");
        cmd_edit_per_repo_config(&project_path).expect("should succeed");

        let config_file = project_path.join(".config").join("z.kdl");
        assert!(config_file.exists(), ".config/z.kdl should be created");
        let contents = fs::read_to_string(&config_file).unwrap();
        assert!(contents.contains("layout"), "template should mention layout");
        assert!(contents.contains("claude"), "template should mention claude");
        assert!(contents.contains("deploy"), "template should mention deploy");
        assert!(contents.contains("autopilot"), "template should mention autopilot");
    }

    #[test]
    fn edit_per_repo_config_does_not_overwrite_existing_file() {
        let _guard = EDITOR_MUTEX.lock().unwrap();
        let project_path = unique_test_dir("no_overwrite");
        let config_dir = project_path.join(".config");
        fs::create_dir_all(&config_dir).unwrap();
        let config_file = config_dir.join("z.kdl");
        let original = "layout \"compact\"\n";
        fs::write(&config_file, original).unwrap();

        std::env::set_var("EDITOR", "true");
        cmd_edit_per_repo_config(&project_path).expect("should succeed");

        let contents = fs::read_to_string(&config_file).unwrap();
        assert_eq!(contents, original, "existing file should not be overwritten");
    }

    #[test]
    fn edit_per_repo_config_succeeds_with_editor_set() {
        let _guard = EDITOR_MUTEX.lock().unwrap();
        let project_path = unique_test_dir("editor_set");
        std::env::set_var("EDITOR", "true");
        cmd_edit_per_repo_config(&project_path).expect("should succeed with EDITOR=true");
    }

    #[test]
    fn edit_per_repo_config_returns_error_for_missing_editor() {
        let _guard = EDITOR_MUTEX.lock().unwrap();
        let project_path = unique_test_dir("missing_editor");
        std::env::set_var("EDITOR", "/nonexistent/editor/binary");
        let result = cmd_edit_per_repo_config(&project_path);
        assert!(result.is_err(), "should fail when editor binary doesn't exist");
    }

    #[test]
    fn edit_per_repo_config_preserves_existing_config_dir() {
        let _guard = EDITOR_MUTEX.lock().unwrap();
        let project_path = unique_test_dir("existing_config_dir");
        let config_dir = project_path.join(".config");
        fs::create_dir_all(&config_dir).unwrap();
        // Place an unrelated file to prove the directory isn't recreated/destroyed.
        let marker = config_dir.join("other.txt");
        fs::write(&marker, "keep me").unwrap();

        std::env::set_var("EDITOR", "true");
        cmd_edit_per_repo_config(&project_path).expect("should succeed");

        assert!(marker.exists(), "unrelated files in .config/ should be preserved");
        assert_eq!(fs::read_to_string(&marker).unwrap(), "keep me");
    }
}

// ---------------------------------------------------------------------------
// Autopilot commands
// ---------------------------------------------------------------------------

/// Dispatch `z autopilot [subcommand] [args]`.
fn cmd_autopilot_dispatch(sub: Option<&str>, args: &[String]) -> z_core::error::Result<()> {
    match sub {
        None | Some("help") => {
            println!("usage: z autopilot <subcommand>");
            println!();
            println!("subcommands:");
            println!("  list [project]   — list available workflows (built-in + per-repo custom)");
            println!("  status [project] — show persisted workflow run states");
            Ok(())
        }
        Some("list") => {
            let project_name = args.get(1).map(|s| s.as_str());
            let project_path: Option<std::path::PathBuf> = if let Some(name) = project_name {
                let store = KdlProjectStore::new();
                store.get_project(name).ok().map(|p| p.path)
            } else {
                None
            };
            cmd_autopilot_list(project_path.as_deref())
        }
        Some("status") => {
            let project_filter = args.get(1).map(|s| s.as_str());
            cmd_autopilot_status(project_filter)
        }
        Some(unknown) => {
            Err(z_core::error::ZError::Io(format!(
                "unknown autopilot subcommand: {:?}\nusage: z autopilot [list|status]",
                unknown
            )))
        }
    }
}

/// List all available autopilot workflows: built-in + per-repo custom workflows
/// for the given project path (if provided).
pub fn cmd_autopilot_list(project_path: Option<&std::path::Path>) -> z_core::error::Result<()> {
    let mut all_workflows: Vec<AutopilotWorkflow> = builtin_workflows()
        .map_err(|e| z_core::error::ZError::Io(format!("load built-in workflows: {e}")))?;

    // Append per-repo custom workflows when a project path is given.
    if let Some(path) = project_path {
        let repo_config_path = path.join(".config").join("z.kdl");
        if let Ok(content) = fs::read_to_string(&repo_config_path) {
            match z_autopilot::dsl::parse_autopilot_workflows(&content) {
                Ok(custom) => all_workflows.extend(custom),
                Err(e) => eprintln!("warning: failed to parse {}: {}", repo_config_path.display(), e),
            }
        }
    }

    println!("{}", format_workflow_list(&all_workflows));
    Ok(())
}

/// Show persisted workflow run states, optionally filtered to a project.
pub fn cmd_autopilot_status(project_filter: Option<&str>) -> z_core::error::Result<()> {
    let state_dir = autopilot_state_dir();
    let runs = list_runs(&state_dir)?;

    let filtered: Vec<&WorkflowRun> = runs.iter()
        .filter(|r| project_filter.map_or(true, |p| r.project == p))
        .collect();

    println!("{}", format_run_status(&filtered));
    Ok(())
}

/// Returns the directory where autopilot run state is persisted.
fn autopilot_state_dir() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("z")
        .join("autopilot")
}

/// Format a list of workflows for display.
pub fn format_workflow_list(workflows: &[AutopilotWorkflow]) -> String {
    if workflows.is_empty() {
        return "No workflows available.".to_string();
    }
    let mut out = String::new();
    out.push_str("Available workflows:\n");
    for wf in workflows {
        let desc = wf.description.as_deref().unwrap_or("");
        let trigger = wf.trigger.as_str();
        out.push_str(&format!("  {:30}  trigger: {:25}  {}\n", wf.name, trigger, desc));
    }
    out
}

/// Format a list of workflow run states for display.
pub fn format_run_status(runs: &[&WorkflowRun]) -> String {
    if runs.is_empty() {
        return "No active or completed workflow runs.".to_string();
    }
    let mut out = String::new();
    out.push_str("Workflow runs:\n");
    for run in runs {
        let status = match run.status {
            WorkflowStatus::Running => "running  ",
            WorkflowStatus::Completed => "completed",
            WorkflowStatus::Failed => "failed   ",
            WorkflowStatus::Stuck => "stuck    ",
        };
        let step = run.current_step.as_deref().unwrap_or("-");
        out.push_str(&format!(
            "  {}  {}  {:20}  step: {}\n",
            run.project, status, run.workflow_name, step
        ));
    }
    out
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
        // Remote projects: list sessions on the remote host via SSH.
        // Local projects: query the local Zellij instance.
        let sessions = if let Some(host) = &project.host {
            match remote::extract_ssh_host(host)
                .and_then(|ssh_host| remote::list_remote_sessions(&ssh_host, &project.name))
            {
                Ok(s) => s,
                Err(e) => {
                    eprintln!(
                        "  warning: could not list remote sessions for {}: {}",
                        project.name, e
                    );
                    Vec::new()
                }
            }
        } else {
            session_mgr.list_sessions(&project.name)?
        };

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
