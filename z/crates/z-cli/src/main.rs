mod activity_store;
mod autopilot_runner;
mod config_store;
mod depcheck_impl;
mod forge;
mod git_preview;
mod log;
mod notify;
mod preview;
mod remote;
mod repo_config;
mod session_manager;
mod session_open;
mod workspace;
mod worktree_manager;
mod worktree_metadata_store;
mod zellij_action;

use std::collections::HashSet;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use std::fs;

use z_core::config::{effective_layout, parse_global_config_kdl, GlobalConfig, PerRepoConfig};
use z_core::depcheck::{check_deps, format_dep_error, DepCheckStatus};
use z_core::domain::{NotifyLevel, Session};
use z_core::traits::{
    Notifier, ProjectStore, ProjectStoreWriter, SessionManager, WorktreeManager,
    WorktreeMetadataStore,
};

use z_autopilot::builtin::builtin_workflows;
use z_autopilot::dsl::AutopilotWorkflow;
use z_autopilot::persist::{list_runs, prune_terminal_runs};
use z_autopilot::run_loop::{execute_workflow_run, RunLoopOptions, RunLoopReport, RunLoopStop};
use z_autopilot::state::{WorkflowRun, WorkflowStatus};

use crate::config_store::KdlProjectStore;
use crate::depcheck_impl::ProcessDepChecker;
use crate::notify::DispatchNotifier;
use crate::session_manager::{
    list_all_z_sessions_with_ages, parse_session_name, ZellijSessionManager, ZellijSessionRefresher,
};
use crate::worktree_manager::WtWorktreeManager;

use z_tui::{Navigation, PreviewContext, PreviewDataSource, ProjectEntry, TuiAction};

/// Resolve the absolute path to the current `z` binary.
/// Falls back to `"z"` (assumes PATH) if `current_exe()` fails.
fn resolve_bin_path() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_else(|| "z".to_string())
}

/// Resolve the session name from the environment.
///
/// Checks `Z_SESSION_NAME` first (set by Zellij layout env block),
/// then falls back to `ZELLIJ_SESSION_NAME` (set by Zellij itself).
/// Returns `None` if neither is set.
fn resolve_session_env() -> Option<String> {
    std::env::var("Z_SESSION_NAME")
        .ok()
        .filter(|session| !session.is_empty())
        .or_else(|| std::env::var("ZELLIJ_SESSION_NAME").ok())
        .filter(|session| !session.is_empty())
}

fn resolve_required_session_env(command: &str) -> z_core::error::Result<String> {
    resolve_session_env()
        .filter(|session| !session.is_empty())
        .ok_or_else(|| {
            z_core::error::ZError::Session(format!(
                "z {command} must be run inside a Zellij session \
             (neither Z_SESSION_NAME nor ZELLIJ_SESSION_NAME is set)"
            ))
        })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let is_doctor = args.first().map(|s| s.as_str()) == Some("doctor");

    let checker = ProcessDepChecker;
    let results = check_deps(&checker);

    let mut failed = false;
    for result in &results {
        if !matches!(result.status, DepCheckStatus::Ok { .. }) {
            eprintln!("{}", format_dep_error(result));
            failed = true;
        }
    }

    if failed && !is_doctor {
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
            if let Err(e) = cmd_open(project, branch, None) {
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
        Some("session") => {
            let sub = args.get(1).map(|s| s.as_str()).unwrap_or("");
            match sub {
                "kill" => {
                    let project = args.get(2).map(|s| s.as_str()).unwrap_or("");
                    let branch = args.get(3).map(|s| s.as_str()).unwrap_or("");
                    if project.is_empty() || branch.is_empty() {
                        eprintln!("usage: z session kill <project> <branch>");
                        std::process::exit(1);
                    }
                    if let Err(e) = cmd_session_kill(project, branch) {
                        eprintln!("error: {}", e);
                        std::process::exit(1);
                    }
                }
                _ => {
                    eprintln!("usage: z session kill <project> <branch>");
                    std::process::exit(1);
                }
            }
        }
        Some("worktree") => {
            let sub = args.get(1).map(|s| s.as_str()).unwrap_or("");
            match sub {
                "delete" => {
                    let project = args.get(2).map(|s| s.as_str()).unwrap_or("");
                    let branch = args.get(3).map(|s| s.as_str()).unwrap_or("");
                    let confirmed = args
                        .windows(2)
                        .any(|pair| pair[0] == "--confirm" && pair[1] == branch);
                    if project.is_empty() || branch.is_empty() {
                        eprintln!(
                            "usage: z worktree delete <project> <branch> [--confirm <branch>]"
                        );
                        std::process::exit(1);
                    }
                    if args.iter().any(|arg| arg == "--force") {
                        eprintln!("error: use --confirm <branch> after explicit confirmation; --force is not supported");
                        std::process::exit(1);
                    }
                    match cmd_worktree_delete(project, branch, confirmed) {
                        Ok(message) => println!("{}", message),
                        Err(e) => {
                            eprintln!("error: {}", e);
                            std::process::exit(1);
                        }
                    }
                }
                _ => {
                    eprintln!("usage: z worktree delete <project> <branch> [--confirm <branch>]");
                    std::process::exit(1);
                }
            }
        }
        Some("project") => {
            let sub = args.get(1).map(|s| s.as_str()).unwrap_or("");
            match sub {
                "delete" => {
                    let project = args.get(2).map(|s| s.as_str()).unwrap_or("");
                    if project.is_empty() {
                        eprintln!("usage: z project delete <project>");
                        std::process::exit(1);
                    }
                    match cmd_project_delete(project) {
                        Ok(message) => println!("{}", message),
                        Err(e) => {
                            eprintln!("error: {}", e);
                            std::process::exit(1);
                        }
                    }
                }
                _ => {
                    eprintln!("usage: z project delete <project>");
                    std::process::exit(1);
                }
            }
        }
        Some("doctor") => {
            let fix = args.iter().any(|a| a == "--fix");
            let interactive = args.iter().any(|a| a == "--interactive");
            match cmd_doctor(fix, interactive) {
                Ok(report) => println!("{}", report),
                Err(e) => {
                    eprintln!("error: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Some("notify") => {
            let env_session = resolve_session_env();
            match resolve_notify_command_args(&args[1..], env_session.as_deref()) {
                Ok(NotifyCommand::Legacy {
                    session,
                    message,
                    level,
                }) => {
                    if let Err(e) = cmd_notify(&session, &message, level) {
                        eprintln!("error: {}", e);
                        std::process::exit(1);
                    }
                }
                Ok(NotifyCommand::Event(event)) => {
                    if let Err(e) = cmd_notify_event(event) {
                        eprintln!("error: {}", e);
                        std::process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
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
        Some("logs-viewer") => {
            if let Err(e) = cmd_logs_viewer() {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        }
        Some("actions") => {
            if let Err(e) = cmd_actions() {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        }
        Some(cmd) => {
            eprintln!("unknown command: {:?}", cmd);
            eprintln!("usage: z [list|open|close|project|session|worktree|doctor|notify|autopilot|logs|switch|logs-viewer|actions]");
            std::process::exit(1);
        }
    }
}

/// Launch the interactive TUI and execute whatever action the user chooses.
/// Loops back into the TUI after adding a project so the user stays in context.
fn cmd_tui() -> z_core::error::Result<()> {
    let store = KdlProjectStore::new();
    let global = load_global_config();

    let navigation = match global.navigation.as_deref() {
        Some("vim") => Navigation::Vim,
        _ => Navigation::Arrows,
    };

    // Track the name of the most recently added project for auto-selection.
    let mut initial_project: Option<String> = None;
    let mut status_message: Option<String> = None;

    // Load built-in workflows once; they are the same for every project.
    let builtin: Vec<AutopilotWorkflow> = builtin_workflows().unwrap_or_default();

    /// Build a fresh list of project entries with worktree topology.
    ///
    /// Local sessions and worktrees are loaded for each project. Uses
    /// `assemble_worktree_entries` to build the Worktree-first topology.
    fn build_entries(
        store: &KdlProjectStore,
        builtin: &[AutopilotWorkflow],
    ) -> z_core::error::Result<Vec<ProjectEntry>> {
        use std::collections::HashMap;
        use z_core::domain::{
            DiscoveredWorktree, SessionLink, WorktreeDiagnostic, WorktreeEntry, WorktreeIdentity,
            WorktreeStatus,
        };
        use z_core::worktree_topology::assemble_worktree_entries;

        let projects = store.list_projects()?;

        // One subprocess call for all local sessions.
        let local_stdout = std::process::Command::new("zellij")
            .arg("list-sessions")
            .output()
            .ok()
            .map(|o| {
                let raw = String::from_utf8_lossy(&o.stdout);
                session_manager::strip_ansi(&raw)
            });

        let activity = activity_store::FileActivityStore::default().load_activity();
        let metadata_store = crate::worktree_metadata_store::LocalWorktreeMetadataStore::default();
        let _ = migrate_local_metadata_if_needed();
        let local_metadata = metadata_store.read_metadata().ok();

        let mut entries: Vec<ProjectEntry> = Vec::with_capacity(projects.len());
        for project in &projects {
            let remote_name = project.name.clone();

            let mut topology_diagnostics: Vec<WorktreeDiagnostic> = Vec::new();

            // Discover worktrees (includes detached/no-branch entries)
            let discovered_worktrees: Vec<DiscoveredWorktree> = if project.host.is_none() {
                let wt_output = std::process::Command::new("git")
                    .args(["worktree", "list", "--porcelain"])
                    .current_dir(&project.path)
                    .output()
                    .ok()
                    .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                    .unwrap_or_default();
                worktree_manager::parse_git_worktree_porcelain_detailed(
                    &wt_output,
                    &project.name,
                    &project.path,
                    None,
                )
            } else {
                match project.host.as_deref() {
                    Some(host) => match crate::worktree_manager::list_remote_worktrees_detailed(
                        host,
                        &project.path,
                        &remote_name,
                    ) {
                        Ok(worktrees) => worktrees,
                        Err(_) => {
                            topology_diagnostics.push(WorktreeDiagnostic::RemoteUnavailable);
                            Vec::new()
                        }
                    },
                    None => Vec::new(),
                }
            };

            // Get active sessions
            let sessions = if project.host.is_none() {
                local_stdout
                    .as_deref()
                    .map(|s| session_manager::parse_zellij_sessions(s, &project.name))
                    .unwrap_or_default()
            } else {
                match project.host.as_deref() {
                    Some(host) => match remote::list_remote_sessions(host, &remote_name) {
                        Ok(sessions) => sessions,
                        Err(_) => {
                            topology_diagnostics.push(WorktreeDiagnostic::RemoteUnavailable);
                            Vec::new()
                        }
                    },
                    None => Vec::new(),
                }
            };

            // Compute safety status for each worktree
            let safety_map: HashMap<WorktreeIdentity, z_core::domain::GitSafetyStatus> =
                discovered_worktrees
                    .iter()
                    .filter_map(|wt| {
                        if wt.identity.host.is_some() {
                            return None;
                        }
                        let result =
                            crate::worktree_manager::check_git_safety(&wt.identity.worktree_path);
                        match result {
                            Ok(safety) => Some((wt.identity.clone(), safety)),
                            Err(_) => None,
                        }
                    })
                    .collect();

            // Get metadata records from the side where the Project lives.
            let project_metadata = if let Some(host) = project.host.as_deref() {
                match crate::worktree_metadata_store::RemoteWorktreeMetadataStore::new(host)
                    .read_metadata()
                {
                    Ok(metadata) => Some(metadata),
                    Err(_) => {
                        topology_diagnostics.push(WorktreeDiagnostic::MetadataUnavailable);
                        None
                    }
                }
            } else {
                local_metadata.clone()
            };

            let metadata_records = project_metadata
                .as_ref()
                .map(|m| m.worktrees.as_slice())
                .unwrap_or(&[]);

            // Assemble topology
            let mut worktree_entries = assemble_worktree_entries(
                project,
                discovered_worktrees,
                &sessions,
                metadata_records,
                safety_map,
            );

            if topology_diagnostics.contains(&WorktreeDiagnostic::MetadataUnavailable) {
                for entry in &mut worktree_entries {
                    entry
                        .diagnostics
                        .push(WorktreeDiagnostic::MetadataUnavailable);
                }
            }

            if topology_diagnostics.contains(&WorktreeDiagnostic::RemoteUnavailable) {
                for entry in &mut worktree_entries {
                    if !entry
                        .diagnostics
                        .contains(&WorktreeDiagnostic::RemoteUnavailable)
                    {
                        entry
                            .diagnostics
                            .push(WorktreeDiagnostic::RemoteUnavailable);
                    }
                }
            }

            if worktree_entries.is_empty()
                && topology_diagnostics.contains(&WorktreeDiagnostic::RemoteUnavailable)
            {
                worktree_entries.push(WorktreeEntry {
                    discovered: DiscoveredWorktree {
                        identity: WorktreeIdentity {
                            host: project.host.clone(),
                            project_root: project.path.clone(),
                            worktree_path: project.path.clone(),
                        },
                        project_name: remote_name.clone(),
                        branch: None,
                        is_primary_checkout: true,
                    },
                    status: WorktreeStatus::Unsupported,
                    diagnostics: topology_diagnostics.clone(),
                    safety: None,
                    session_link: SessionLink::None,
                    metadata: None,
                });
            }

            // Extract active sessions from worktree entries for legacy usage
            let active_sessions: Vec<Session> = worktree_entries
                .iter()
                .filter_map(|wt| match &wt.session_link {
                    z_core::domain::SessionLink::Active(s) => Some(s.clone()),
                    _ => None,
                })
                .collect();

            let repo_config_path = project.path.join(".config").join("z.kdl");
            let repo_config = fs::read_to_string(&repo_config_path)
                .map(|content| workspace::parse_repo_workspace_config(&content))
                .unwrap_or_default();

            entries.push(workspace::build_project_entry(
                workspace::WorkspaceEntryInput {
                    project: project.clone(),
                    worktrees: worktree_entries,
                    sessions: active_sessions,
                    custom_workflows: repo_config.custom_workflows,
                    repo_actions: repo_config.repo_actions,
                },
                builtin,
                &activity,
            ));
        }
        Ok(entries)
    }

    loop {
        let entries = build_entries(&store, &builtin)?;

        // Load pending notifications so the TUI can display 🔔 badges.
        let notifications = load_notification_aliases();

        // Auto-select the newly added project if one was just added.
        let initial_idx = initial_project
            .as_deref()
            .and_then(|name| entries.iter().position(|e| e.project.name == name));

        let theme = z_core::theme::Theme::from_name(global.theme);

        let callbacks = z_tui::TuiCallbacks {
            log_fn: &|max_lines| {
                let l = log::FileLogger::new();
                let entries = l.read_recent(max_lines);
                Ok(entries.iter().map(|e| e.format()).collect())
            },
            swap_fn: &|a, b| {
                let mut s = config_store::KdlProjectStore::new();
                s.swap_projects(a, b)
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))
            },
            kill_session_fn: &|session_name| {
                let (project_name, branch) = parse_session_name(session_name).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "invalid session name {:?}: expected project:branch",
                            session_name
                        ),
                    )
                })?;
                let sess = z_core::domain::Session {
                    name: session_name.to_string(),
                    project: project_name,
                    branch,
                };
                (ZellijSessionManager {
                    bin_path: resolve_bin_path(),
                })
                .kill_session(&sess)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                Ok(())
            },
            delete_worktree_fn: &|project_name, branch, force| {
                cmd_worktree_delete(project_name, branch, force)
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))
            },
            doctor_fn: &|fix| {
                cmd_doctor(fix, false)
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))
            },
            add_project_fn: &|path, name, host, transport| {
                let transport = match transport {
                    Some("mosh") => Some(z_core::domain::Transport::Mosh),
                    Some("ssh") => Some(z_core::domain::Transport::Ssh),
                    _ => None,
                };
                let project = z_core::domain::Project {
                    name: name.to_string(),
                    path: std::path::PathBuf::from(path),
                    host: host.map(String::from),
                    transport,
                };
                let mut s = config_store::KdlProjectStore::new();
                s.add_project(&project)
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))
            },
            edit_project_fn: &|original_name, path, name, host, transport| {
                let mut s = config_store::KdlProjectStore::new();
                s.remove_project(original_name)
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                let transport = match transport {
                    Some("mosh") => Some(z_core::domain::Transport::Mosh),
                    Some("ssh") => Some(z_core::domain::Transport::Ssh),
                    _ => None,
                };
                let project = z_core::domain::Project {
                    name: name.to_string(),
                    path: std::path::PathBuf::from(path),
                    host: host.map(String::from),
                    transport,
                };
                s.add_project(&project)
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))
            },
            delete_project_fn: &|name| {
                let mut s = config_store::KdlProjectStore::new();
                s.remove_project(name)
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))
            },
            reload_fn: &|| {
                let s = config_store::KdlProjectStore::new();
                let entries = build_entries(&s, &builtin)
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
                let notifications = load_notification_aliases();
                Ok((entries, notifications))
            },
        };

        let action = z_tui::run_tui(
            entries,
            navigation.clone(),
            notifications,
            initial_idx,
            status_message.take(),
            callbacks,
            Box::new(preview::CliPreviewDataSource::new(Box::new(
                forge::GhForgeClient,
            ))),
            Box::new(ZellijSessionRefresher),
            theme,
            global.actions.clone(),
            global.review_tool.clone(),
        )
        .map_err(|e| z_core::error::ZError::Io(e.to_string()))?;

        match action {
            TuiAction::Quit => return Ok(()),

            TuiAction::Open { project, branch } => {
                cmd_open(&project, branch.as_deref(), None)?;
                initial_project = Some(project);
            }

            TuiAction::New { project, branch } => {
                cmd_open(&project, Some(&branch), None)?;
                initial_project = Some(project);
            }

            TuiAction::NewFromIssue {
                project,
                number,
                title,
                slug,
            } => {
                let branch = format!("grill/{}-{}", number, slug);
                let global = load_global_config();
                let per_repo = load_per_repo_config_for_project(&project);
                let template = z_core::config::effective_issue_prompt_template(&global, &per_repo);
                let mut vars = std::collections::HashMap::new();
                let num_str = number.to_string();
                vars.insert("number", num_str.as_str());
                vars.insert("title", title.as_str());
                let prompt = z_core::template::resolve_template(&template, &vars);
                cmd_open(&project, Some(&branch), Some(&prompt))?;
                initial_project = Some(project);
            }

            TuiAction::NewFromPr {
                project,
                number,
                title,
                branch,
            } => {
                let global = load_global_config();
                let per_repo = load_per_repo_config_for_project(&project);
                let template = z_core::config::effective_pr_prompt_template(&global, &per_repo);
                let mut vars = std::collections::HashMap::new();
                let num_str = number.to_string();
                vars.insert("number", num_str.as_str());
                vars.insert("title", title.as_str());
                vars.insert("branch", branch.as_str());
                let prompt = z_core::template::resolve_template(&template, &vars);
                cmd_open(&project, Some(&branch), Some(&prompt))?;
                initial_project = Some(project);
            }

            TuiAction::EditPerRepoConfig { project_path } => {
                cmd_edit_per_repo_config(&project_path)?;
            }

            TuiAction::RunAction {
                session,
                command,
                pane_type,
            } => {
                let request = zellij_action::ZellijActionRequest {
                    session: Some(session),
                    tab_name: None,
                    pane_type,
                    command,
                };
                let is_tab = matches!(request.pane_type, z_core::action::PaneType::Tab);
                let result = zellij_action::run_action(&request);
                match result {
                    Ok(s) if s.success() => {
                        status_message = Some(if is_tab {
                            "Action launched in new tab.".to_string()
                        } else {
                            "Action launched.".to_string()
                        });
                    }
                    Ok(_) => {
                        status_message = Some("Action failed to launch.".to_string());
                    }
                    Err(e) => {
                        status_message = Some(format!("Failed to run action: {e}"));
                    }
                }
            }
            TuiAction::RunWorkflow { project, workflow } => {
                cmd_autopilot_run(&project, &workflow)?;
            }
        }
    }
}

fn load_notification_aliases() -> HashSet<String> {
    KdlProjectStore::new()
        .list_projects()
        .map(|projects| session_manager::fetch_project_notification_aliases(&projects))
        .unwrap_or_default()
}

fn discover_local_worktrees() -> z_core::error::Result<Vec<z_core::domain::DiscoveredWorktree>> {
    let store = KdlProjectStore::new();
    let projects = store.list_projects()?;
    Ok(discover_local_worktrees_for_projects(&projects))
}

fn discover_local_worktrees_for_projects(
    projects: &[z_core::domain::Project],
) -> Vec<z_core::domain::DiscoveredWorktree> {
    projects
        .iter()
        .filter(|project| project.host.is_none())
        .flat_map(|project| {
            std::process::Command::new("git")
                .args(["worktree", "list", "--porcelain"])
                .current_dir(&project.path)
                .output()
                .ok()
                .filter(|output| output.status.success())
                .map(|output| String::from_utf8_lossy(&output.stdout).to_string())
                .map(|stdout| {
                    worktree_manager::parse_git_worktree_porcelain_detailed(
                        &stdout,
                        &project.name,
                        &project.path,
                        None,
                    )
                })
                .unwrap_or_default()
        })
        .collect()
}

fn migrate_local_metadata_if_needed() -> z_core::error::Result<()> {
    let store = KdlProjectStore::new();
    let projects = store.list_projects()?;
    let discovered = discover_local_worktrees_for_projects(&projects);

    let metadata_store = worktree_metadata_store::LocalWorktreeMetadataStore::default();
    // One-shot activity migration (no-op if metadata already exists)
    metadata_store.migrate_legacy_activity(&discovered)?;
    // Always drain legacy notification files (idempotent)
    metadata_store.drain_legacy_notifications(&discovered, None)?;
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
        Ok(content) => repo_config::parse_repo_config_projection(&content)
            .map(|projection| projection.per_repo)
            .unwrap_or_default(),
        Err(_) => PerRepoConfig::default(),
    }
}

fn load_per_repo_config_for_project(project_name: &str) -> PerRepoConfig {
    let store = KdlProjectStore::new();
    match store.get_project(project_name) {
        Ok(project) => load_per_repo_config(&project.path),
        Err(_) => PerRepoConfig::default(),
    }
}

fn cmd_edit_per_repo_config(project_path: &std::path::Path) -> z_core::error::Result<()> {
    let config_dir = project_path.join(".config");
    let config_file = config_dir.join("z.kdl");

    // Create .config/ directory if missing (create_dir_all is a no-op if it exists).
    fs::create_dir_all(&config_dir).map_err(|e| z_core::error::ZError::Io(e.to_string()))?;

    // Create the file with a commented template if it doesn't exist.
    if !config_file.exists() {
        let template = "\
// Per-repo z configuration
// Available options are shown below (all optional).

// layout {
//   tab name=\"claude\" {
//     pane command=\"claude\" {
//       args \"--dangerously-skip-permissions\"
//     }
//   }
//   tab name=\"shell\" {
//     pane
//   }
// }

// deploy {
//   command \"npm run deploy\"   // command run by `z deploy`
// }

// autopilot {
//   auto-push true     // automatically push commits
//   review true        // open a PR after each autopilot session
// }
";
        fs::write(&config_file, template).map_err(|e| z_core::error::ZError::Io(e.to_string()))?;
    }

    // Determine editor: $EDITOR, falling back to vi.
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());

    // Spawn editor and wait for it to exit.
    let status = std::process::Command::new(&editor)
        .arg(&config_file)
        .status()
        .map_err(|e| {
            z_core::error::ZError::Io(format!("failed to launch editor '{}': {}", editor, e))
        })?;

    if !status.success() {
        eprintln!("editor exited with status: {}", status);
    }

    Ok(())
}

fn cmd_open(
    project_name: &str,
    branch: Option<&str>,
    prompt: Option<&str>,
) -> z_core::error::Result<()> {
    let store = KdlProjectStore::new();
    let session_mgr = ZellijSessionManager {
        bin_path: resolve_bin_path(),
    };

    // Resolve project — returns ProjectNotFound if not in config.
    let project = store.get_project(project_name)?;

    // Remote project: SSH worktree setup + Zellij HTTPS attach.
    if let Some(host) = project.host.clone() {
        return cmd_open_remote(&project, &host, branch);
    }

    // Discover the actual primary checkout branch instead of assuming "main".
    let effective_branch = match branch {
        Some(b) => b.to_string(),
        None => worktree_manager::discover_primary_branch(&project.path)
            .unwrap_or_else(|_| "main".to_string()),
    };

    let wt_mgr = WtWorktreeManager::new(project.path.clone());
    let discovered_worktrees = wt_mgr.list_worktrees(&project.name)?;
    let mut target_worktree_path = if branch.is_some() {
        discovered_worktrees
            .iter()
            .find(|worktree| worktree.branch == effective_branch)
            .map(|worktree| worktree.path.clone())
    } else {
        Some(project.path.clone())
    };

    // Build the expected Session name (branch "/" -> "-" normalization applied).
    let sessions = session_mgr.list_sessions(&project.name)?;
    let open_plan = session_open::plan_open_session(&project, &effective_branch, &sessions);

    let logger = log::FileLogger::new();

    // Check for an existing live Session.
    if let Some(existing) = &open_plan.existing_session {
        let Some(path) = target_worktree_path.clone() else {
            return Err(z_core::error::ZError::Worktree(format!(
                "active session {} has no matching worktree for branch {}",
                existing.name, effective_branch
            )));
        };
        log::log_info(&logger, &format!("session {} attached", existing.name));
        let result = session_mgr.attach_session(existing);
        result?;
        record_worktree_entry(&project, &effective_branch, &path, &existing.name)?;
        return Ok(());
    }

    // Session doesn't exist — create it.
    let cwd = if let Some(branch_name) = branch {
        // Branch specified: find or create the worktree.
        let worktree_path = if let Some(path) = target_worktree_path.take() {
            // Worktree already exists — just reuse its path.
            path
        } else {
            // Create new worktree via `wt switch -c <branch>`.
            let new_wt = wt_mgr.create_worktree(&project.name, branch_name)?;
            log::log_info(
                &logger,
                &format!("worktree {} created for {}", branch_name, project_name),
            );
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
    layout.cwd = Some(cwd.clone());
    layout.session_name_env = Some(open_plan.target_session.name.clone());
    if let Some(prompt_text) = prompt {
        z_core::layout::inject_prompt_into_layout(&mut layout, prompt_text);
    }
    inject_claude_stop_hook(&cwd);
    let theme = z_core::theme::Theme::from_name(global.theme);
    session_mgr.create_session(&project.name, &effective_branch, layout, &theme)?;
    log::log_info(
        &logger,
        &format!("session {} created", open_plan.target_session.name),
    );
    record_worktree_entry(
        &project,
        &effective_branch,
        &cwd,
        &open_plan.target_session.name,
    )?;

    Ok(())
}

fn record_worktree_entry(
    project: &z_core::domain::Project,
    branch: &str,
    path: &std::path::Path,
    session_name: &str,
) -> z_core::error::Result<()> {
    if project.host.is_none() {
        migrate_local_metadata_if_needed()?;
    }
    let discovered = z_core::domain::DiscoveredWorktree {
        identity: z_core::domain::WorktreeIdentity {
            host: project.host.clone(),
            project_root: project.path.clone(),
            worktree_path: path.to_path_buf(),
        },
        project_name: project.name.clone(),
        branch: Some(branch.to_string()),
        is_primary_checkout: path == project.path,
    };
    let store = worktree_metadata_store::LocalWorktreeMetadataStore::default();
    store.record_opened(&discovered, session_name)?;
    store.clear_notifications(&discovered.identity)?;
    z_core::session_entry::record_session_attach(
        &activity_store::FileActivityStore::default(),
        session_name,
    );
    Ok(())
}

/// Open a session on a remote project by SSH/Mosh-ing into the host and running
/// `z open <project> <branch>` there. Zellij runs on the remote machine.
///
/// The configured Project name is used on the remote as well; no basename
/// guessing is performed.
fn cmd_open_remote(
    project: &z_core::domain::Project,
    host: &str,
    branch: Option<&str>,
) -> z_core::error::Result<()> {
    let remote_name = project.name.as_str();

    let remote_cmd = if let Some(branch) = branch {
        format!(
            "cd {} && z open {} {}",
            remote::shell_quote(&project.path.display().to_string()),
            remote::shell_quote(remote_name),
            remote::shell_quote(branch),
        )
    } else {
        format!(
            "cd {} && z open {}",
            remote::shell_quote(&project.path.display().to_string()),
            remote::shell_quote(remote_name),
        )
    };

    let use_mosh = matches!(project.transport, Some(z_core::domain::Transport::Mosh));

    // Wrap in login shell so nix/direnv PATH is available on the remote.
    let wrapped = format!("bash -l -c {}", remote::shell_quote(&remote_cmd));

    let status = if use_mosh {
        // mosh's Perl wrapper shell-quotes args for SSH transport, and mosh-server
        // passes them to bash via execvp. Do NOT add our own shell_quote layer —
        // that would double-quote and break the command.
        std::process::Command::new("mosh")
            .args([host, "--", "bash", "-l", "-c", &remote_cmd])
            .status()
    } else {
        // -t: allocate TTY for interactive Zellij session.
        std::process::Command::new("ssh")
            .args(["-t", "-o", "ConnectTimeout=10", host, &wrapped])
            .status()
    }
    .map_err(|e| z_core::error::ZError::Session(e.to_string()))?;

    if !status.success() {
        let transport = if use_mosh { "mosh" } else { "ssh" };
        return Err(z_core::error::ZError::Session(format!(
            "{} to {} failed with status {}",
            transport, host, status
        )));
    }
    Ok(())
}

/// Detach from a Zellij session, keeping it running in the background.
///
/// `session_name` — if `None`, detects the current session from `Z_SESSION_NAME`
/// with `ZELLIJ_SESSION_NAME` fallback.
fn cmd_close(session_name: Option<&str>) -> z_core::error::Result<()> {
    let session_mgr = ZellijSessionManager {
        bin_path: resolve_bin_path(),
    };

    let name = match session_name {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => resolve_required_session_env("close")?,
    };

    let (project, branch) = parse_session_name(&name).ok_or_else(|| {
        z_core::error::ZError::Session(format!(
            "invalid session name {:?}: expected project:branch",
            name
        ))
    })?;

    let session = Session {
        name: name.clone(),
        project,
        branch,
    };
    session_mgr.detach_session(&session)?;
    println!("Detached from session: {}", name);
    Ok(())
}

fn delete_project_from_store(
    store: &mut impl ProjectStoreWriter,
    project_name: &str,
) -> z_core::error::Result<String> {
    let name = project_name.trim();
    if name.is_empty() {
        return Err(z_core::error::ZError::ConfigParse(
            "project name is required".to_string(),
        ));
    }
    store.remove_project(name)?;
    Ok(format!("Project '{}' deleted.", name))
}

fn cmd_project_delete(project_name: &str) -> z_core::error::Result<String> {
    let mut store = KdlProjectStore::new();
    delete_project_from_store(&mut store, project_name)
}

/// Kill a Zellij session by project+branch (z session kill <project> <branch>).
///
/// For remote projects, delegates to the remote machine via SSH.
fn cmd_session_kill(project_name: &str, branch: &str) -> z_core::error::Result<()> {
    let session_mgr = ZellijSessionManager {
        bin_path: resolve_bin_path(),
    };

    // Look up the project to check if it's remote.
    let store = KdlProjectStore::new();
    let project = store.get_project(project_name).ok();

    if let Some(proj) = &project {
        if let Some(host) = &proj.host {
            let remote_name = proj.name.as_str();
            let session_name = z_core::domain::Session::new(remote_name, branch).name;
            remote::delete_remote_session(host, &session_name)?;
            println!("Session {} killed.", session_name);
            return Ok(());
        }
    }

    let session_name = z_core::domain::Session::new(project_name, branch).name;

    let session = Session {
        name: session_name.clone(),
        project: project_name.to_string(),
        branch: branch.to_string(),
    };

    session_mgr.kill_session(&session)?;
    println!("Session {} killed.", session_name);
    Ok(())
}

/// Delete a worktree with preflight checks: primary checkout, dirty, ahead,
/// no upstream, and active session protections.
///
/// Uses `WtWorktreeManager::remove_worktree` (the `wt remove` adapter).
/// For protected cases, returns an error message explaining the protection.
/// `confirmed` is only true after an exact branch-name confirmation from TUI
/// or `--confirm <branch>` from CLI.
fn cmd_worktree_delete(
    project_name: &str,
    branch: &str,
    confirmed: bool,
) -> z_core::error::Result<String> {
    let store = KdlProjectStore::new();
    let project = store.get_project(project_name)?;

    if let Some(host) = project.host.as_deref() {
        if !confirmed {
            return Err(z_core::error::ZError::Worktree(
                "remote worktree delete requires --confirm <branch> after explicit confirmation"
                    .to_string(),
            ));
        }
        let remote_name = project.name.as_str();
        let remote_cmd = format!(
            "cd {} && z worktree delete {} {} --confirm {}",
            remote::shell_quote(&project.path.display().to_string()),
            remote::shell_quote(remote_name),
            remote::shell_quote(branch),
            remote::shell_quote(branch),
        );
        remote::ssh_run_remote(host, &remote_cmd)?;
        let msg = format!(
            "Remote worktree '{}' for '{}' deleted.",
            branch, project_name
        );
        return Ok(msg);
    }

    let wt_mgr = WtWorktreeManager::new(project.path.clone());

    // Discover worktrees to find the target
    let worktrees = wt_mgr.list_worktrees(&project.name)?;
    let target = worktrees
        .iter()
        .find(|w| w.branch == branch)
        .ok_or_else(|| {
            z_core::error::ZError::Worktree(format!(
                "worktree '{}' not found for project '{}'",
                branch, project_name
            ))
        })?;

    let target_session_name = z_core::domain::Session::new(project_name, branch).name;
    let detailed_output = std::process::Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(&project.path)
        .output()
        .map_err(|e| z_core::error::ZError::Worktree(format!("git worktree list failed: {e}")))?;
    if !detailed_output.status.success() {
        return Err(z_core::error::ZError::Worktree(format!(
            "git worktree list failed with status {}",
            detailed_output.status
        )));
    }
    let detailed_stdout = String::from_utf8_lossy(&detailed_output.stdout);
    let colliding_branches: Vec<String> = worktree_manager::parse_git_worktree_porcelain_detailed(
        &detailed_stdout,
        &project.name,
        &project.path,
        None,
    )
    .into_iter()
    .filter_map(|worktree| worktree.branch)
    .filter(|candidate_branch| {
        z_core::domain::Session::new(project_name, candidate_branch).name == target_session_name
    })
    .collect();
    let has_session_name_collision = colliding_branches.len() > 1;
    if has_session_name_collision && !confirmed {
        return Err(z_core::error::ZError::Worktree(format!(
            "Worktree '{}' has a Session name collision with: {}. Re-run with --confirm {} after choosing the exact branch.",
            branch,
            colliding_branches.join(", "),
            branch
        )));
    }

    // Preflight: protect primary checkout
    if target.path == project.path {
        return Err(z_core::error::ZError::Worktree(
            "Cannot delete primary checkout worktree.".to_string(),
        ));
    }

    // Preflight: check git safety
    let safety = worktree_manager::check_git_safety(&target.path).map_err(|e| {
        z_core::error::ZError::Worktree(format!(
            "Cannot verify git safety for Worktree '{}': {}. Refusing deletion.",
            branch, e
        ))
    })?;
    if safety.dirty && !confirmed {
        return Err(z_core::error::ZError::Worktree(format!(
            "Worktree '{}' has uncommitted changes. Commit or stash first, then re-run with --confirm {} after reviewing the risk.",
            branch,
            branch
        )));
    }
    if !safety.has_upstream && !confirmed {
        return Err(z_core::error::ZError::Worktree(format!(
            "Worktree '{}' has no upstream branch. Push first, then re-run with --confirm {} after reviewing the risk.",
            branch,
            branch
        )));
    }
    if safety.ahead > 0 && !confirmed {
        return Err(z_core::error::ZError::Worktree(format!(
            "Worktree '{}' is {} commit(s) ahead of upstream. Push first, then re-run with --confirm {} after reviewing the risk.",
            branch, safety.ahead, branch
        )));
    }

    // Preflight: check for active session
    let sessions = ZellijSessionManager {
        bin_path: resolve_bin_path(),
    }
    .list_sessions(project_name)?;
    let session_name = target_session_name;
    let has_active_session = sessions.iter().any(|s| s.name == session_name);
    if has_active_session {
        if has_session_name_collision {
            return Err(z_core::error::ZError::Worktree(format!(
                "Worktree '{}' shares Session name '{}' with another Worktree. Resolve the collision before deleting an active Worktree.",
                branch, session_name
            )));
        }
        if !confirmed {
            return Err(z_core::error::ZError::Worktree(format!(
                "Worktree '{}' has an active Session. Re-run with --confirm {} after explicit confirmation.",
                branch,
                branch
            )));
        }
        // Kill session first
        let session = Session {
            name: session_name.clone(),
            project: project_name.to_string(),
            branch: branch.to_string(),
        };
        ZellijSessionManager {
            bin_path: resolve_bin_path(),
        }
        .kill_session(&session)
        .map_err(|e| z_core::error::ZError::Session(format!("Failed to kill session: {e}")))?;
        let _ = activity_store::FileActivityStore::default().remove_entry(&session_name);
    }

    // Execute worktree removal via the existing `wt remove` adapter
    wt_mgr.remove_worktree(target, confirmed)?;

    // Clean metadata
    let metadata_store = crate::worktree_metadata_store::LocalWorktreeMetadataStore::default();
    let identity = z_core::domain::WorktreeIdentity {
        host: None,
        project_root: project.path.clone(),
        worktree_path: target.path.clone(),
    };
    let _ = metadata_store.remove_worktree(&identity);

    let msg = format!("Worktree '{}' for '{}' deleted.", branch, project_name);
    Ok(msg)
}

/// Run doctor diagnostics.
///
/// Reports dependency status, topology issues, and metadata health.
/// Can run even when external dependencies are missing.
fn cmd_doctor(fix: bool, interactive: bool) -> z_core::error::Result<String> {
    let mut diagnostics: Vec<String> = Vec::new();
    let global = load_global_config();

    // 1. Check dependencies (always runs, even on failure)
    let checker = ProcessDepChecker;
    let dep_results = check_deps(&checker);
    for result in &dep_results {
        match &result.status {
            DepCheckStatus::Ok { version } => {
                diagnostics.push(format!("  ✓ {} ({})", result.tool, version));
            }
            DepCheckStatus::Missing => {
                diagnostics.push(format!("  ✗ {}: not installed or not in PATH", result.tool));
            }
            DepCheckStatus::VersionTooLow { found, required } => {
                diagnostics.push(format!(
                    "  ✗ {}: version {} does not satisfy {}",
                    result.tool, found, required
                ));
            }
            DepCheckStatus::VersionUnparseable { output } => {
                diagnostics.push(format!(
                    "  ✗ {}: unparseable version output: {:?}",
                    result.tool, output
                ));
            }
        }
    }

    if !global.switcher.invalid_priorities.is_empty() {
        diagnostics.push(String::new());
        diagnostics.push(" Config:".to_string());
        for priority in &global.switcher.invalid_priorities {
            diagnostics.push(format!(
                "  ⚠ unknown switcher priority criterion: {priority}"
            ));
        }
    }

    diagnostics.push(String::new());
    diagnostics.push(" Worktree topology:".to_string());

    // 2. Check projects and worktree topology
    let _projects = match KdlProjectStore::new().list_projects() {
        Ok(projects) => {
            if projects.is_empty() {
                diagnostics.push("  No projects configured.".to_string());
            } else {
                for project in &projects {
                    let path_str = project.path.display();
                    if let Some(host) = project.host.as_deref() {
                        let remote_name = project.name.as_str();
                        match worktree_manager::list_remote_worktrees_detailed(
                            host,
                            &project.path,
                            remote_name,
                        ) {
                            Ok(worktrees) => {
                                diagnostics.push(format!(
                                    "  ✓ {} remote:{} ({} worktree(s))",
                                    project.name,
                                    host,
                                    worktrees.len()
                                ));
                                for worktree in &worktrees {
                                    if worktree.branch.is_none() {
                                        diagnostics.push(format!(
                                            "    ? unsupported detached worktree: {}",
                                            worktree.identity.worktree_path.display()
                                        ));
                                    }
                                }
                            }
                            Err(e) => diagnostics
                                .push(format!("  ✗ {} remote unavailable: {}", project.name, e)),
                        }
                    } else if !project.path.exists() {
                        diagnostics.push(format!(
                            "  ✗ {}: path does not exist ({})",
                            project.name, path_str
                        ));
                    } else if !project.path.join(".git").exists() {
                        diagnostics.push(format!(
                            "  ✗ {}: not a git repository ({})",
                            project.name, path_str
                        ));
                    } else {
                        let branch = worktree_manager::discover_primary_branch(&project.path)
                            .unwrap_or_else(|_| "unknown".to_string());
                        diagnostics.push(format!(
                            "  ✓ {} ({}, primary: {})",
                            project.name, path_str, branch
                        ));
                        if let Ok(output) = std::process::Command::new("git")
                            .args(["worktree", "list", "--porcelain"])
                            .current_dir(&project.path)
                            .output()
                        {
                            let stdout = String::from_utf8_lossy(&output.stdout);
                            let worktrees = worktree_manager::parse_git_worktree_porcelain_detailed(
                                &stdout,
                                &project.name,
                                &project.path,
                                None,
                            );
                            let sessions = ZellijSessionManager {
                                bin_path: resolve_bin_path(),
                            }
                            .list_sessions(&project.name)
                            .unwrap_or_default();
                            let entries = z_core::worktree_topology::assemble_worktree_entries(
                                project,
                                worktrees.clone(),
                                &sessions,
                                &[],
                                std::collections::HashMap::new(),
                            );
                            for entry in &entries {
                                match entry.status {
                                    z_core::domain::WorktreeStatus::Conflict => {
                                        diagnostics.push(format!(
                                            "    ⚠ session-name conflict: {}",
                                            entry
                                                .discovered
                                                .branch
                                                .as_deref()
                                                .unwrap_or("(detached)")
                                        ))
                                    }
                                    z_core::domain::WorktreeStatus::Unsupported => diagnostics
                                        .push(format!(
                                            "    ? unsupported detached worktree: {}",
                                            entry.discovered.identity.worktree_path.display()
                                        )),
                                    _ => {}
                                }
                            }
                            for session in z_core::worktree_topology::find_orphan_sessions(
                                &sessions, &worktrees,
                            ) {
                                diagnostics.push(format!("    ⚠ orphan session: {}", session.name));
                            }
                        }
                    }
                }
            }
            projects
        }
        Err(e) => {
            diagnostics.push(format!("  ✗ Failed to list projects: {e}"));
            Vec::new()
        }
    };

    // 3. Check metadata health
    diagnostics.push(String::new());
    diagnostics.push(" Metadata:".to_string());
    let metadata_store = crate::worktree_metadata_store::LocalWorktreeMetadataStore::default();
    match metadata_store.read_metadata() {
        Ok(meta) => {
            diagnostics.push(format!(
                "  {} worktree record(s), {} notification(s), {} LLM status record(s)",
                meta.worktrees.len(),
                meta.notifications.len(),
                meta.llm_status.len()
            ));
            if !meta.migration_diagnostics.is_empty() {
                for d in &meta.migration_diagnostics {
                    diagnostics.push(format!("  ⚠ migration: {d}"));
                }
            }
            if !meta.unattached_activity.is_empty() {
                diagnostics.push(format!(
                    "  ⚠ {} unattached activity record(s)",
                    meta.unattached_activity.len()
                ));
            }
            if !meta.unattached_notifications.is_empty() {
                diagnostics.push(format!(
                    "  ⚠ {} unattached notification(s)",
                    meta.unattached_notifications.len()
                ));
            }
        }
        Err(e) => {
            diagnostics.push(format!("  ✗ Metadata read failed: {e}"));
        }
    }

    if fix {
        diagnostics.push(String::new());
        match doctor_fix_metadata() {
            Ok(message) => diagnostics.push(format!(" --fix: {message}")),
            Err(e) => diagnostics.push(format!(" --fix failed: {e}")),
        }
    }

    if interactive {
        diagnostics.push(String::new());
        diagnostics.push(
            " --interactive: use dashboard confirmations or explicit z session/worktree commands for destructive fixes."
                .to_string(),
        );
    }

    Ok(diagnostics.join("\n"))
}

fn doctor_fix_metadata() -> z_core::error::Result<String> {
    let metadata_store = worktree_metadata_store::LocalWorktreeMetadataStore::default();
    let mut metadata = metadata_store.read_metadata()?;
    let before_worktrees = metadata.worktrees.len();
    let before_notifications = metadata.notifications.len();
    let before_llm_status = metadata.llm_status.len();

    metadata
        .worktrees
        .retain(|record| record.host.is_some() || record.path.exists());
    metadata.notifications.retain(|notification| {
        metadata.worktrees.iter().any(|record| {
            record.host == notification.target.host
                && record.project_root == notification.target.project_root
                && record.path == notification.target.worktree_path
        })
    });
    metadata.llm_status.retain(|status| {
        metadata.worktrees.iter().any(|record| {
            record.host == status.target.host
                && record.project_root == status.target.project_root
                && record.path == status.target.worktree_path
        })
    });
    let removed_worktrees = before_worktrees.saturating_sub(metadata.worktrees.len());
    let removed_notifications = before_notifications.saturating_sub(metadata.notifications.len());
    let removed_llm_status = before_llm_status.saturating_sub(metadata.llm_status.len());
    metadata_store.write_metadata(&metadata)?;

    Ok(format!(
        "removed {} stale metadata record(s), {} stale notification(s), {} stale LLM status record(s); preserved unattached diagnostics",
        removed_worktrees, removed_notifications, removed_llm_status
    ))
}

/// Returns `true` if the user typed "y" or "Y".
pub fn parse_confirm_response(response: &str) -> bool {
    matches!(response.trim().to_lowercase().as_str(), "y")
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

struct SwitchLockGuard {
    path: PathBuf,
}

impl Drop for SwitchLockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn acquire_switch_lock(path: impl AsRef<Path>) -> io::Result<Option<SwitchLockGuard>> {
    let path = path.as_ref();
    loop {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(mut file) => {
                writeln!(file, "{}", std::process::id())?;
                return Ok(Some(SwitchLockGuard {
                    path: path.to_path_buf(),
                }));
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                if switch_lock_owner_is_active(path) {
                    return Ok(None);
                }
                match fs::remove_file(path) {
                    Ok(()) => continue,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                    Err(e) => return Err(e),
                }
            }
            Err(e) => return Err(e),
        }
    }
}

fn switch_lock_owner_is_active(path: &Path) -> bool {
    let Ok(content) = fs::read_to_string(path) else {
        return false;
    };
    let Some(pid) = content
        .lines()
        .next()
        .and_then(|line| line.trim().parse::<u32>().ok())
    else {
        return false;
    };
    process_looks_like_z_switch(pid)
}

fn process_looks_like_z_switch(pid: u32) -> bool {
    let cmdline_path = format!("/proc/{pid}/cmdline");
    let Ok(cmdline) = fs::read(cmdline_path) else {
        return false;
    };
    cmdline.split(|byte| *byte == 0).any(|arg| arg == b"switch")
}

/// Launch the interactive session switch picker.
///
/// Guard: must be run inside a Zellij session (`$Z_SESSION_NAME` or `$ZELLIJ_SESSION_NAME` set).
/// Lists all z-managed sessions, presents a picker TUI, and if the user
/// selects a session runs `zellij action switch-session <name>`.
fn cmd_switch() -> z_core::error::Result<()> {
    let current_session = resolve_required_session_env("switch")?;

    // Prevent multiple switch pickers from opening simultaneously, while
    // allowing stale lock files from crashed/interrupted panes to self-heal.
    let Some(_lock) = acquire_switch_lock("/tmp/z-switch.lock")
        .map_err(|e| z_core::error::ZError::Io(e.to_string()))?
    else {
        // Another switcher is already running — exit silently so the floating
        // pane closes immediately via close_on_exit.
        return Ok(());
    };

    let global = load_global_config();
    let activity = activity_store::FileActivityStore::default().load_activity();
    let discovered = discover_local_worktrees().unwrap_or_default();
    let metadata = worktree_metadata_store::LocalWorktreeMetadataStore::default()
        .read_metadata()
        .ok();
    let mut sessions = list_all_z_sessions_with_ages();
    z_core::activity::sort_by_recent_attach(&mut sessions, &activity, |s| s.0.as_str());
    let mut switch_entries =
        build_switch_entries(sessions, metadata.as_ref(), &discovered, &global);
    z_tui::sort_switch_entries(&mut switch_entries, &global.switcher.priority);

    let selected = z_tui::run_switch_picker_with_entries(switch_entries, current_session)
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
        z_core::session_entry::record_session_attach(
            &activity_store::FileActivityStore::default(),
            &session_name,
        );
        clear_metadata_after_session_entry(&session_name);
    }

    Ok(())
}

fn build_switch_entries(
    sessions: Vec<(String, Option<String>)>,
    metadata: Option<&z_core::domain::WorktreeMetadataFile>,
    discovered: &[z_core::domain::DiscoveredWorktree],
    global: &GlobalConfig,
) -> Vec<z_tui::SwitchSessionEntry> {
    let now_ms = unix_now_ms();
    let ttl_ms = global.llm.working_ttl_seconds.saturating_mul(1000);

    sessions
        .into_iter()
        .map(|(session_name, age)| {
            let mut notification_count = 0usize;
            let mut notifications = Vec::new();
            let mut activity = None;

            if let Some(metadata) = metadata {
                if let z_core::domain::SessionAliasResolution::Unique(worktree) =
                    z_core::domain::resolve_session_alias(&session_name, discovered)
                {
                    let mut metadata_notifications = metadata
                        .notifications
                        .iter()
                        .filter(|notification| notification.target == worktree.identity)
                        .cloned()
                        .collect::<Vec<_>>();
                    metadata_notifications.sort_by(|a, b| b.created_at.cmp(&a.created_at));
                    notification_count = metadata_notifications.len();
                    notifications = metadata_notifications
                        .into_iter()
                        .map(|notification| z_tui::SwitchNotification {
                            level: notification.level,
                            message: notification.message,
                            created_at_ms: Some(notification.created_at),
                        })
                        .collect();
                    activity = metadata
                        .llm_status
                        .iter()
                        .filter(|status| status.target == worktree.identity)
                        .filter_map(|status| match status.state {
                            z_core::domain::AgentActivityState::Waiting => {
                                Some(z_tui::SwitchAgentActivity {
                                    tool: status.tool.clone(),
                                    state: z_tui::SwitchAgentActivityState::Waiting,
                                    updated_at_ms: status.updated_at_ms,
                                    reason: status.reason.clone(),
                                })
                            }
                            z_core::domain::AgentActivityState::Working => {
                                if z_core::agent_activity::working_status_is_fresh(
                                    status, now_ms, ttl_ms,
                                ) {
                                    Some(z_tui::SwitchAgentActivity {
                                        tool: status.tool.clone(),
                                        state: z_tui::SwitchAgentActivityState::Working,
                                        updated_at_ms: status.updated_at_ms,
                                        reason: status.reason.clone(),
                                    })
                                } else {
                                    None
                                }
                            }
                        })
                        .max_by_key(|activity| match activity.state {
                            z_tui::SwitchAgentActivityState::Waiting => {
                                (1u8, activity.updated_at_ms)
                            }
                            z_tui::SwitchAgentActivityState::Working => {
                                (0u8, activity.updated_at_ms)
                            }
                        });
                }
            }

            z_tui::SwitchSessionEntry {
                session_name,
                age,
                notification_count,
                notifications,
                activity,
            }
        })
        .collect()
}

fn clear_metadata_after_session_entry(session_name: &str) {
    match resolve_notify_target(session_name) {
        Ok(NotifyTarget::Local(identity)) => {
            let _ = worktree_metadata_store::LocalWorktreeMetadataStore::default()
                .clear_notifications(&identity);
        }
        Ok(NotifyTarget::Remote { host, identity, .. }) => {
            let _ = worktree_metadata_store::RemoteWorktreeMetadataStore::new(host)
                .clear_notifications(&identity);
        }
        Err(_) => {}
    }
}

fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Logs viewer command (standalone, for Zellij floating pane)
// ---------------------------------------------------------------------------

fn cmd_logs_viewer() -> z_core::error::Result<()> {
    let logger = log::FileLogger::new();
    let entries = logger.read_recent(500);
    let lines: Vec<String> = entries.iter().map(|e| e.format()).collect();
    z_tui::run_log_viewer(lines).map_err(|e| z_core::error::ZError::Io(e.to_string()))
}

/// Run the action picker inside a Zellij floating pane.
/// Reads `Z_SESSION_NAME` with `ZELLIJ_SESSION_NAME` fallback to detect project/branch context,
/// resolves available actions, shows a picker, and executes the selection.
fn cmd_actions() -> z_core::error::Result<()> {
    let session_name = resolve_required_session_env("actions")?;

    let (project_name, branch) =
        session_manager::parse_session_name(&session_name).ok_or_else(|| {
            z_core::error::ZError::Session(format!(
                "cannot parse session name '{session_name}' (expected project:branch)"
            ))
        })?;

    let store = config_store::KdlProjectStore::new();
    let project = store.get_project(&project_name)?;

    let global = load_global_config();
    let per_repo = load_per_repo_config(&project.path);

    let preview_source = preview::CliPreviewDataSource::new(Box::new(forge::GhForgeClient));
    let preview_context = PreviewContext {
        project_path: project.path.clone(),
        host: project.host.clone(),
        project_name: project.name.clone(),
        branch: branch.clone(),
        session_name: session_name.clone(),
    };
    let action_preview = preview_source
        .load_extra_preview(&preview_context)
        .map(|extra| {
            z_core::action::ActionPreview::from_forge_data(
                extra.pr.as_ref(),
                Some(extra.ci),
                extra.review.as_ref(),
            )
        })
        .unwrap_or_default();

    let env = z_core::action::ActionEnv::for_session(
        project.name.clone(),
        project.path.to_string_lossy().to_string(),
        session_name.clone(),
        branch.clone(),
        global.review_tool.clone(),
        action_preview,
    );

    let merged = z_core::action::merge_actions(&[
        z_core::action::builtin_actions(),
        global.actions.clone(),
        per_repo.actions.clone(),
    ]);

    let actions = z_core::action::resolve_actions(&merged, &env).unwrap_or_default();

    if actions.is_empty() {
        eprintln!("No actions available for {session_name}");
        return Ok(());
    }

    let selected =
        z_tui::run_action_picker(actions).map_err(|e| z_core::error::ZError::Io(e.to_string()))?;

    if let Some(action) = selected {
        match &action.action {
            z_core::action::ActionType::Run { command } => {
                let status = zellij_action::run_action(&zellij_action::ZellijActionRequest {
                    session: None,
                    tab_name: Some(action.name.clone()),
                    pane_type: action.pane.clone(),
                    command: command.clone(),
                })
                .map_err(|e| z_core::error::ZError::Io(e.to_string()))?;
                if !status.success() {
                    return Err(z_core::error::ZError::Io("action command failed".into()));
                }
            }
            z_core::action::ActionType::OpenUrl { url } => {
                // Print OSC 8 hyperlink
                print!("\x1b]8;;{url}\x1b\\{url}\x1b]8;;\x1b\\");
                println!();
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Claude Code hook injection
// ---------------------------------------------------------------------------

/// Inject (or update) the Z stop hook in `.claude/settings.json` at `cwd`.
///
/// Best-effort: failures are silently ignored so they never block session creation.
fn inject_claude_stop_hook(cwd: &std::path::Path) {
    let settings_dir = cwd.join(".claude");
    let settings_path = settings_dir.join("settings.json");

    let existing: Option<serde_json::Value> = std::fs::read_to_string(&settings_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());

    let merged = z_core::claude_hook::merge_stop_hook(
        existing,
        "z notify \"Claude a terminé: ${Z_SESSION_NAME:-$ZELLIJ_SESSION_NAME}\"",
    );

    let _ = std::fs::create_dir_all(&settings_dir);
    let _ = std::fs::write(
        &settings_path,
        serde_json::to_string_pretty(&merged).unwrap_or_default(),
    );
}

// ---------------------------------------------------------------------------
// Notification command
// ---------------------------------------------------------------------------

/// Write a metadata-backed notification for `session` and dispatch configured channels.
fn cmd_notify(session: &str, message: &str, level: NotifyLevel) -> z_core::error::Result<()> {
    migrate_local_metadata_if_needed()?;
    if let Ok(target) = resolve_notify_target(session) {
        match target {
            NotifyTarget::Local(identity) => {
                worktree_metadata_store::LocalWorktreeMetadataStore::default().add_notification(
                    identity,
                    level.clone(),
                    message,
                )?;
            }
            NotifyTarget::Remote {
                host, session_name, ..
            } => {
                let remote_cmd = format!(
                    "z notify {} {} --level {}",
                    remote::shell_quote(&session_name),
                    remote::shell_quote(message),
                    remote::shell_quote(notify_level_arg(&level)),
                );
                remote::ssh_run_remote(&host, &remote_cmd)?;
                return Ok(());
            }
        }
    } else {
        worktree_metadata_store::LocalWorktreeMetadataStore::default()
            .add_unattached_notification(session, level.clone(), message)?;
    }

    let global = load_global_config();
    let dispatcher = DispatchNotifier::from_config(&global.notifications, session);
    dispatcher.notify(message, level)?;
    Ok(())
}

fn cmd_notify_event(event: NotifyEventCommand) -> z_core::error::Result<()> {
    let target = resolve_notify_target(&event.session)?;
    match target {
        NotifyTarget::Local(identity) => {
            let global = load_global_config();
            let settings = z_core::agent_activity::AgentActivitySettings::from_seconds(
                global.llm.working_update_min_interval_seconds,
            );
            let activity_event = match event.kind {
                NotifyEventKind::LlmWorking => z_core::agent_activity::AgentActivityEvent::Working,
                NotifyEventKind::LlmIdle => z_core::agent_activity::AgentActivityEvent::Idle,
                NotifyEventKind::LlmWaiting => {
                    z_core::agent_activity::AgentActivityEvent::Waiting {
                        level: event.level.clone(),
                        message: event.message.clone().unwrap_or_else(|| {
                            default_waiting_message(&event.tool, event.reason.as_deref())
                        }),
                    }
                }
            };
            apply_event_after_successful_migration(migrate_local_metadata_if_needed(), || {
                worktree_metadata_store::LocalWorktreeMetadataStore::default()
                    .apply_agent_activity(
                        identity,
                        &event.tool,
                        activity_event,
                        event.reason,
                        settings,
                    )?;
                Ok(())
            })
        }
        NotifyTarget::Remote {
            host, session_name, ..
        } => {
            let remote_cmd = event.to_remote_command(&session_name);
            remote::ssh_run_remote(&host, &remote_cmd)
        }
    }
}

fn apply_event_after_successful_migration<F>(
    migration: z_core::error::Result<()>,
    apply: F,
) -> z_core::error::Result<()>
where
    F: FnOnce() -> z_core::error::Result<()>,
{
    migration?;
    apply()
}

enum NotifyTarget {
    Local(z_core::domain::WorktreeIdentity),
    Remote {
        host: String,
        session_name: String,
        identity: z_core::domain::WorktreeIdentity,
    },
}

fn resolve_notify_target(session_name: &str) -> z_core::error::Result<NotifyTarget> {
    let store = KdlProjectStore::new();
    for project in store.list_projects()? {
        let lookup_project_name = project.name.clone();

        if !session_name.starts_with(&format!("{}:", lookup_project_name)) {
            continue;
        }

        let discovered = if let Some(host) = project.host.as_deref() {
            worktree_manager::list_remote_worktrees_detailed(
                host,
                &project.path,
                &lookup_project_name,
            )?
        } else {
            let output = std::process::Command::new("git")
                .args(["worktree", "list", "--porcelain"])
                .current_dir(&project.path)
                .output()
                .map_err(|e| {
                    z_core::error::ZError::Worktree(format!("git worktree list failed: {e}"))
                })?;
            if !output.status.success() {
                return Err(z_core::error::ZError::Worktree(format!(
                    "git worktree list exited with status {}",
                    output.status
                )));
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            worktree_manager::parse_git_worktree_porcelain_detailed(
                &stdout,
                &lookup_project_name,
                &project.path,
                None,
            )
        };

        return match z_core::domain::resolve_session_alias(session_name, &discovered) {
            z_core::domain::SessionAliasResolution::Unique(worktree) => {
                if let Some(host) = project.host {
                    Ok(NotifyTarget::Remote {
                        host,
                        session_name: session_name.to_string(),
                        identity: worktree.identity,
                    })
                } else {
                    Ok(NotifyTarget::Local(worktree.identity))
                }
            }
            z_core::domain::SessionAliasResolution::Ambiguous(_) => {
                Err(z_core::error::ZError::Session(format!(
                    "session {} resolves to multiple worktrees",
                    session_name
                )))
            }
            z_core::domain::SessionAliasResolution::None => Err(z_core::error::ZError::Session(
                format!("session {} does not resolve to a worktree", session_name),
            )),
        };
    }

    Err(z_core::error::ZError::Session(format!(
        "session {} does not match any configured project",
        session_name
    )))
}

fn notify_level_arg(level: &NotifyLevel) -> &'static str {
    match level {
        NotifyLevel::Info => "info",
        NotifyLevel::Warning => "warning",
        NotifyLevel::Error => "error",
    }
}

#[derive(Debug, Clone, PartialEq)]
enum NotifyCommand {
    Legacy {
        session: String,
        message: String,
        level: NotifyLevel,
    },
    Event(NotifyEventCommand),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NotifyEventKind {
    LlmWorking,
    LlmIdle,
    LlmWaiting,
}

#[derive(Debug, Clone, PartialEq)]
struct NotifyEventCommand {
    session: String,
    kind: NotifyEventKind,
    tool: String,
    reason: Option<String>,
    message: Option<String>,
    level: NotifyLevel,
}

impl NotifyEventCommand {
    fn event_name(&self) -> &'static str {
        match self.kind {
            NotifyEventKind::LlmWorking => "llm.working",
            NotifyEventKind::LlmIdle => "llm.idle",
            NotifyEventKind::LlmWaiting => "llm.waiting",
        }
    }

    fn to_remote_command(&self, session: &str) -> String {
        let mut command = format!(
            "z notify --event {} --tool {} --session {}",
            remote::shell_quote(self.event_name()),
            remote::shell_quote(&self.tool),
            remote::shell_quote(session)
        );
        if let Some(reason) = &self.reason {
            command.push_str(" --reason ");
            command.push_str(&remote::shell_quote(reason));
        }
        if let Some(message) = &self.message {
            command.push_str(" --message ");
            command.push_str(&remote::shell_quote(message));
        }
        if self.kind == NotifyEventKind::LlmWaiting {
            command.push_str(" --level ");
            command.push_str(&remote::shell_quote(notify_level_arg(&self.level)));
        }
        command
    }
}

fn default_waiting_message(tool: &str, reason: Option<&str>) -> String {
    match (tool, reason) {
        ("opencode", Some("permission")) => "OpenCode needs permission".to_string(),
        ("opencode", Some("error")) => "OpenCode encountered an error".to_string(),
        ("opencode", _) => "OpenCode needs attention".to_string(),
        (_, Some(reason)) => format!("{tool} waiting: {reason}"),
        (_, None) => format!("{tool} waiting"),
    }
}

fn resolve_notify_command_args(
    args: &[String],
    env_session: Option<&str>,
) -> Result<NotifyCommand, String> {
    if args.iter().any(|arg| arg == "--event") {
        resolve_notify_event_args(args, env_session).map(NotifyCommand::Event)
    } else {
        let (session, message, level) = resolve_notify_args(args, env_session)?;
        Ok(NotifyCommand::Legacy {
            session,
            message,
            level,
        })
    }
}

fn resolve_notify_event_args(
    args: &[String],
    env_session: Option<&str>,
) -> Result<NotifyEventCommand, String> {
    let mut event: Option<NotifyEventKind> = None;
    let mut session: Option<String> = None;
    let mut tool: Option<String> = None;
    let mut reason: Option<String> = None;
    let mut message: Option<String> = None;
    let mut level = NotifyLevel::Warning;
    let mut level_seen = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--event" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--event requires a value".to_string())?;
                event = Some(match value.as_str() {
                    "llm.working" => NotifyEventKind::LlmWorking,
                    "llm.idle" => NotifyEventKind::LlmIdle,
                    "llm.waiting" => NotifyEventKind::LlmWaiting,
                    _ => return Err(format!("unknown notify event: {value}")),
                });
                i += 2;
            }
            "--session" => {
                session = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "--session requires a value".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--tool" => {
                tool = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "--tool requires a value".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--reason" => {
                reason = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "--reason requires a value".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--message" => {
                message = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "--message requires a value".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--level" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "--level requires a value".to_string())?;
                level_seen = true;
                level = match value.as_str() {
                    "info" => NotifyLevel::Info,
                    "warning" => NotifyLevel::Warning,
                    "error" => NotifyLevel::Error,
                    _ => {
                        return Err("event mode --level must be info, warning, or error".to_string())
                    }
                };
                i += 2;
            }
            arg if arg.starts_with("--") => {
                return Err(format!("unknown notify event flag: {arg}"))
            }
            arg => {
                return Err(format!(
                    "event mode does not accept positional argument: {arg}"
                ))
            }
        }
    }

    let kind = event.ok_or_else(|| "--event is required".to_string())?;
    let tool = tool.ok_or_else(|| "--tool is required for llm events".to_string())?;
    let session = session
        .or_else(|| env_session.map(str::to_string))
        .ok_or_else(|| {
            "no session specified and neither Z_SESSION_NAME nor ZELLIJ_SESSION_NAME is set"
                .to_string()
        })?;

    match kind {
        NotifyEventKind::LlmWorking | NotifyEventKind::LlmIdle if message.is_some() => Err(
            "llm.working and llm.idle do not accept --message because they are not visible notifications"
                .to_string(),
        ),
        NotifyEventKind::LlmWorking | NotifyEventKind::LlmIdle if level_seen => {
            Err("--level is only valid for llm.waiting".to_string())
        }
        _ => Ok(NotifyEventCommand {
            session,
            kind,
            tool,
            reason,
            message,
            level,
        }),
    }
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

/// Parse notify arguments and resolve the session.
///
/// `args` — everything after the `notify` subcommand.
/// `env_session` — resolved session name from environment
/// (`Z_SESSION_NAME` with `ZELLIJ_SESSION_NAME` fallback), if any.
///
/// Returns `(session, message, level)` or an error string.
fn resolve_notify_args(
    args: &[String],
    env_session: Option<&str>,
) -> Result<(String, String, NotifyLevel), String> {
    // Collect positional arguments (everything that isn't --level or its value).
    let mut positional = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--level" {
            i += 2; // skip flag + value
        } else {
            positional.push(args[i].as_str());
            i += 1;
        }
    }

    let (session, message) = match positional.len() {
        2 => (positional[0].to_string(), positional[1].to_string()),
        1 => {
            let s = env_session.ok_or_else(|| {
                "no session specified and neither Z_SESSION_NAME nor ZELLIJ_SESSION_NAME is set"
                    .to_string()
            })?;
            (s.to_string(), positional[0].to_string())
        }
        _ => {
            return Err(
                "usage: z notify [session] <message> [--level info|warning|error]".to_string(),
            );
        }
    };

    let level = parse_notify_level(args.iter());
    Ok((session, message, level))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Mutex to serialize tests that mutate the EDITOR environment variable,
    /// preventing race conditions when the test suite runs in parallel.
    static EDITOR_MUTEX: Mutex<()> = Mutex::new(());

    /// Mutex to serialize tests that mutate session environment variables.
    static SESSION_ENV_MUTEX: Mutex<()> = Mutex::new(());

    /// Mutex to serialize tests that manipulate switch lock files.
    static SWITCH_LOCK_MUTEX: Mutex<()> = Mutex::new(());

    #[derive(Default)]
    struct RecordingProjectStore {
        removed: Vec<String>,
    }

    impl ProjectStoreWriter for RecordingProjectStore {
        fn add_project(&mut self, _project: &z_core::domain::Project) -> z_core::error::Result<()> {
            Ok(())
        }

        fn update_project(
            &mut self,
            _project: &z_core::domain::Project,
        ) -> z_core::error::Result<()> {
            Ok(())
        }

        fn remove_project(&mut self, name: &str) -> z_core::error::Result<()> {
            self.removed.push(name.to_string());
            Ok(())
        }

        fn swap_projects(&mut self, _a: usize, _b: usize) -> z_core::error::Result<()> {
            Ok(())
        }
    }

    fn clear_session_env() {
        std::env::remove_var("Z_SESSION_NAME");
        std::env::remove_var("ZELLIJ_SESSION_NAME");
    }

    fn test_project_name(suffix: &str) -> String {
        format!("switch-entry-test-{}-{suffix}", std::process::id())
    }

    fn test_identity(project_name: &str) -> z_core::domain::WorktreeIdentity {
        z_core::domain::WorktreeIdentity {
            host: None,
            project_root: PathBuf::from(format!("/repo/{project_name}")),
            worktree_path: PathBuf::from(format!("/repo/{project_name}")),
        }
    }

    fn test_discovered_worktree(project_name: &str) -> z_core::domain::DiscoveredWorktree {
        z_core::domain::DiscoveredWorktree {
            identity: test_identity(project_name),
            project_name: project_name.to_string(),
            branch: Some("main".to_string()),
            is_primary_checkout: true,
        }
    }

    fn temp_switch_lock_path(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "z-switch-lock-test-{}-{}",
            std::process::id(),
            name
        ));
        let _ = fs::remove_file(&path);
        path
    }

    #[test]
    fn build_switch_entries_attaches_metadata_notifications_and_waiting_activity() {
        let project_name = test_project_name("metadata");
        let metadata = z_core::domain::WorktreeMetadataFile {
            version: 2,
            worktrees: vec![],
            notifications: vec![z_core::domain::NotificationRecord {
                id: "n1".to_string(),
                target: test_identity(&project_name),
                level: z_core::domain::NotifyLevel::Warning,
                message: "OpenCode needs permission".to_string(),
                created_at: 1_000,
                source: None,
            }],
            unattached_notifications: vec![],
            unattached_activity: vec![],
            migration_diagnostics: vec![],
            llm_status: vec![z_core::domain::AgentActivityStatus {
                target: test_identity(&project_name),
                tool: "opencode".to_string(),
                state: z_core::domain::AgentActivityState::Waiting,
                updated_at_ms: 1_500,
                reason: Some("permission".to_string()),
                auto_resolve_key: Some("opencode:permission".to_string()),
            }],
            migrated_legacy_ids: std::collections::HashSet::new(),
        };
        let global = GlobalConfig::default();

        let entries = build_switch_entries(
            vec![(format!("{project_name}:main"), Some("2m".to_string()))],
            Some(&metadata),
            &[test_discovered_worktree(&project_name)],
            &global,
        );

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].notification_count, 1);
        assert_eq!(
            entries[0].notifications[0].message,
            "OpenCode needs permission"
        );
        assert_eq!(
            entries[0].activity.as_ref().map(|activity| activity.state),
            Some(z_tui::SwitchAgentActivityState::Waiting)
        );
    }

    #[test]
    fn build_switch_entries_drops_stale_working_activity() {
        let project_name = test_project_name("stale-working");
        let mut global = GlobalConfig::default();
        global.llm.working_ttl_seconds = 1;
        let metadata = z_core::domain::WorktreeMetadataFile {
            version: 2,
            worktrees: vec![],
            notifications: vec![],
            unattached_notifications: vec![],
            unattached_activity: vec![],
            migration_diagnostics: vec![],
            llm_status: vec![z_core::domain::AgentActivityStatus {
                target: test_identity(&project_name),
                tool: "opencode".to_string(),
                state: z_core::domain::AgentActivityState::Working,
                updated_at_ms: 1,
                reason: None,
                auto_resolve_key: None,
            }],
            migrated_legacy_ids: std::collections::HashSet::new(),
        };
        let entries = build_switch_entries(
            vec![(format!("{project_name}:main"), None)],
            Some(&metadata),
            &[test_discovered_worktree(&project_name)],
            &global,
        );

        assert!(entries[0].activity.is_none());
    }

    #[test]
    fn build_switch_entries_counts_metadata_notifications() {
        let project_name = test_project_name("metadata-only");
        let session_name = format!("{project_name}:main");
        let metadata = z_core::domain::WorktreeMetadataFile {
            version: 2,
            worktrees: vec![],
            notifications: vec![z_core::domain::NotificationRecord {
                id: "n1".to_string(),
                target: test_identity(&project_name),
                level: z_core::domain::NotifyLevel::Info,
                message: "build finished".to_string(),
                created_at: 1_000,
                source: None,
            }],
            unattached_notifications: vec![],
            unattached_activity: vec![],
            migration_diagnostics: vec![],
            llm_status: vec![],
            migrated_legacy_ids: std::collections::HashSet::new(),
        };
        let entries = build_switch_entries(
            vec![(session_name.clone(), None)],
            Some(&metadata),
            &[test_discovered_worktree(&project_name)],
            &GlobalConfig::default(),
        );

        assert_eq!(entries[0].notification_count, 1);
        assert!(entries[0]
            .notifications
            .iter()
            .any(|notification| notification.message == "build finished"));
    }

    #[test]
    fn notify_event_apply_is_blocked_when_migration_fails() {
        let mut called = false;

        let result = apply_event_after_successful_migration(
            Err(z_core::error::ZError::MetadataCorrupt(
                "bad json".to_string(),
            )),
            || {
                called = true;
                Ok(())
            },
        );

        assert!(result.is_err());
        assert!(
            !called,
            "event mutation must not run after migration failure"
        );
    }

    #[test]
    fn acquire_switch_lock_removes_stale_pid_lock() {
        let _guard = SWITCH_LOCK_MUTEX.lock().unwrap();
        let path = temp_switch_lock_path("stale-pid");
        fs::write(&path, "999999999\n").unwrap();

        let lock = acquire_switch_lock(&path).unwrap();

        assert!(
            lock.is_some(),
            "stale lock should be replaced by a live lock"
        );
        assert_eq!(
            fs::read_to_string(&path).unwrap().trim(),
            std::process::id().to_string()
        );
        drop(lock);
        assert!(
            !path.exists(),
            "lock guard should remove the lock file on drop"
        );
    }

    #[test]
    fn acquire_switch_lock_removes_invalid_lock_contents() {
        let _guard = SWITCH_LOCK_MUTEX.lock().unwrap();
        let path = temp_switch_lock_path("invalid");
        fs::write(&path, "not-a-pid\n").unwrap();

        let lock = acquire_switch_lock(&path).unwrap();

        assert!(
            lock.is_some(),
            "invalid lock contents should be treated as stale"
        );
        drop(lock);
        assert!(
            !path.exists(),
            "lock guard should remove the lock file on drop"
        );
    }

    #[test]
    fn delete_project_from_store_trims_name_and_reports_success() {
        let mut store = RecordingProjectStore::default();

        let message = delete_project_from_store(&mut store, "  arkan  ").unwrap();

        assert_eq!(store.removed, vec!["arkan"]);
        assert_eq!(message, "Project 'arkan' deleted.");
    }

    #[test]
    fn delete_project_from_store_rejects_empty_name() {
        let mut store = RecordingProjectStore::default();

        let err = delete_project_from_store(&mut store, "   ").unwrap_err();

        assert!(matches!(err, z_core::error::ZError::ConfigParse(_)));
        assert!(store.removed.is_empty());
    }

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

    // ── resolve_notify_args tests ────────────────────────────────────────

    #[test]
    fn resolve_notify_args_two_positional_returns_session_and_message() {
        let args = vec!["my-session".into(), "hello world".into()];
        let (session, message, level) = resolve_notify_args(&args, None).unwrap();
        assert_eq!(session, "my-session");
        assert_eq!(message, "hello world");
        assert_eq!(level, NotifyLevel::Info);
    }

    #[test]
    fn resolve_notify_args_one_positional_with_env_uses_env_session() {
        let args = vec!["hello world".into()];
        let (session, message, level) = resolve_notify_args(&args, Some("env-session")).unwrap();
        assert_eq!(session, "env-session");
        assert_eq!(message, "hello world");
        assert_eq!(level, NotifyLevel::Info);
    }

    #[test]
    fn resolve_notify_args_one_positional_no_env_returns_error() {
        let args = vec!["hello world".into()];
        let err = resolve_notify_args(&args, None).unwrap_err();
        assert!(
            err.contains("Z_SESSION_NAME"),
            "error should mention env var: {err}"
        );
    }

    #[test]
    fn resolve_notify_args_no_args_returns_usage_error() {
        let args: Vec<String> = vec![];
        let err = resolve_notify_args(&args, Some("env-session")).unwrap_err();
        assert!(err.contains("usage:"), "error should show usage: {err}");
    }

    #[test]
    fn resolve_notify_args_two_positional_with_level() {
        let args: Vec<String> = vec![
            "my-session".into(),
            "deploy done".into(),
            "--level".into(),
            "warning".into(),
        ];
        let (session, message, level) = resolve_notify_args(&args, None).unwrap();
        assert_eq!(session, "my-session");
        assert_eq!(message, "deploy done");
        assert_eq!(level, NotifyLevel::Warning);
    }

    #[test]
    fn resolve_notify_args_one_positional_with_level_and_env() {
        let args: Vec<String> = vec!["deploy done".into(), "--level".into(), "error".into()];
        let (session, message, level) = resolve_notify_args(&args, Some("env-session")).unwrap();
        assert_eq!(session, "env-session");
        assert_eq!(message, "deploy done");
        assert_eq!(level, NotifyLevel::Error);
    }

    #[test]
    fn resolve_notify_command_legacy_preserved() {
        let args: Vec<String> = vec!["my-session".into(), "hello".into()];

        let command = resolve_notify_command_args(&args, None).unwrap();

        assert_eq!(
            command,
            NotifyCommand::Legacy {
                session: "my-session".to_string(),
                message: "hello".to_string(),
                level: NotifyLevel::Info,
            }
        );
    }

    #[test]
    fn resolve_notify_event_working_uses_env_session() {
        let args: Vec<String> = vec![
            "--event".into(),
            "llm.working".into(),
            "--tool".into(),
            "opencode".into(),
        ];

        let command = resolve_notify_event_args(&args, Some("myapp:main")).unwrap();

        assert_eq!(command.session, "myapp:main");
        assert_eq!(command.kind, NotifyEventKind::LlmWorking);
        assert_eq!(command.tool, "opencode");
        assert!(command.message.is_none());
    }

    #[test]
    fn resolve_notify_event_waiting_accepts_message_and_error_level() {
        let args: Vec<String> = vec![
            "--event".into(),
            "llm.waiting".into(),
            "--tool".into(),
            "opencode".into(),
            "--session".into(),
            "myapp:main".into(),
            "--reason".into(),
            "permission".into(),
            "--message".into(),
            "OpenCode needs permission".into(),
            "--level".into(),
            "error".into(),
        ];

        let command = resolve_notify_event_args(&args, None).unwrap();

        assert_eq!(command.session, "myapp:main");
        assert_eq!(command.kind, NotifyEventKind::LlmWaiting);
        assert_eq!(command.reason.as_deref(), Some("permission"));
        assert_eq!(
            command.message.as_deref(),
            Some("OpenCode needs permission")
        );
        assert_eq!(command.level, NotifyLevel::Error);
    }

    #[test]
    fn resolve_notify_event_waiting_accepts_info_level() {
        let args: Vec<String> = vec![
            "--event".into(),
            "llm.waiting".into(),
            "--tool".into(),
            "opencode".into(),
            "--session".into(),
            "myapp:main".into(),
            "--reason".into(),
            "input".into(),
            "--message".into(),
            "OpenCode is waiting for input".into(),
            "--level".into(),
            "info".into(),
        ];

        let command = resolve_notify_event_args(&args, None).unwrap();

        assert_eq!(command.session, "myapp:main");
        assert_eq!(command.kind, NotifyEventKind::LlmWaiting);
        assert_eq!(command.reason.as_deref(), Some("input"));
        assert_eq!(
            command.message.as_deref(),
            Some("OpenCode is waiting for input")
        );
        assert_eq!(command.level, NotifyLevel::Info);
    }

    #[test]
    fn resolve_notify_event_rejects_positional_args() {
        let args: Vec<String> = vec![
            "--event".into(),
            "llm.working".into(),
            "myapp:main".into(),
            "--tool".into(),
            "opencode".into(),
        ];

        let err = resolve_notify_event_args(&args, Some("env-session")).unwrap_err();

        assert!(err.contains("positional"));
    }

    #[test]
    fn resolve_notify_event_rejects_message_for_working() {
        let args: Vec<String> = vec![
            "--event".into(),
            "llm.working".into(),
            "--tool".into(),
            "opencode".into(),
            "--message".into(),
            "busy".into(),
        ];

        let err = resolve_notify_event_args(&args, Some("env-session")).unwrap_err();

        assert!(err.contains("do not accept --message"));
    }

    #[test]
    fn resolve_notify_event_requires_tool() {
        let args: Vec<String> = vec!["--event".into(), "llm.idle".into()];

        let err = resolve_notify_event_args(&args, Some("env-session")).unwrap_err();

        assert!(err.contains("--tool"));
    }

    #[test]
    fn resolve_session_env_uses_z_session_name() {
        let _guard = SESSION_ENV_MUTEX.lock().unwrap();
        clear_session_env();
        std::env::set_var("Z_SESSION_NAME", "z-session");

        assert_eq!(resolve_session_env().as_deref(), Some("z-session"));

        clear_session_env();
    }

    #[test]
    fn resolve_session_env_falls_back_to_zellij_session_name() {
        let _guard = SESSION_ENV_MUTEX.lock().unwrap();
        clear_session_env();
        std::env::set_var("ZELLIJ_SESSION_NAME", "zellij-session");

        assert_eq!(resolve_session_env().as_deref(), Some("zellij-session"));

        clear_session_env();
    }

    #[test]
    fn resolve_session_env_ignores_empty_z_session_name() {
        let _guard = SESSION_ENV_MUTEX.lock().unwrap();
        clear_session_env();
        std::env::set_var("Z_SESSION_NAME", "");
        std::env::set_var("ZELLIJ_SESSION_NAME", "zellij-session");

        assert_eq!(resolve_session_env().as_deref(), Some("zellij-session"));

        clear_session_env();
    }

    #[test]
    fn resolve_session_env_ignores_empty_zellij_session_name() {
        let _guard = SESSION_ENV_MUTEX.lock().unwrap();
        clear_session_env();
        std::env::set_var("ZELLIJ_SESSION_NAME", "");

        assert_eq!(resolve_session_env(), None);

        clear_session_env();
    }

    #[test]
    fn resolve_required_session_env_accepts_z_session_name() {
        let _guard = SESSION_ENV_MUTEX.lock().unwrap();
        clear_session_env();
        std::env::set_var("Z_SESSION_NAME", "myapp:main");

        let session = resolve_required_session_env("switch").unwrap();

        assert_eq!(session, "myapp:main");
        clear_session_env();
    }

    #[test]
    fn resolve_required_session_env_falls_back_to_zellij_session_name() {
        let _guard = SESSION_ENV_MUTEX.lock().unwrap();
        clear_session_env();
        std::env::set_var("ZELLIJ_SESSION_NAME", "myapp:main");

        let session = resolve_required_session_env("switch").unwrap();

        assert_eq!(session, "myapp:main");
        clear_session_env();
    }

    #[test]
    fn resolve_required_session_env_mentions_both_env_vars_when_missing() {
        let _guard = SESSION_ENV_MUTEX.lock().unwrap();
        clear_session_env();

        let err = resolve_required_session_env("switch")
            .unwrap_err()
            .to_string();

        assert!(
            err.contains("Z_SESSION_NAME"),
            "error should mention Z_SESSION_NAME: {err}"
        );
        assert!(
            err.contains("ZELLIJ_SESSION_NAME"),
            "error should mention ZELLIJ_SESSION_NAME: {err}"
        );
        clear_session_env();
    }

    #[test]
    fn resolve_session_env_prefers_z_session_name() {
        let _guard = SESSION_ENV_MUTEX.lock().unwrap();
        clear_session_env();
        std::env::set_var("Z_SESSION_NAME", "z-session");
        std::env::set_var("ZELLIJ_SESSION_NAME", "zellij-session");

        assert_eq!(resolve_session_env().as_deref(), Some("z-session"));

        clear_session_env();
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
        assert!(
            output.contains("post-push"),
            "must show trigger for pr-ci-fix"
        );
    }

    #[test]
    fn format_workflow_list_includes_description() {
        let wfs = builtin_workflows().unwrap();
        let output = format_workflow_list(&wfs);
        assert!(
            output.contains("Monitor CI"),
            "must include pr-ci-fix description"
        );
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

    #[test]
    fn format_prune_status_empty_returns_noop_message() {
        let output = format_prune_status(&[]);

        assert_eq!(output, "No terminal workflow runs to prune.");
    }

    #[test]
    fn format_prune_status_lists_removed_runs() {
        let mut run = z_autopilot::state::WorkflowRun::new("wf1", "proj", "step1");
        run.status = WorkflowStatus::Completed;
        run.current_step = None;

        let output = format_prune_status(&[run]);

        assert!(output.contains("Pruned 1 terminal workflow run"));
        assert!(output.contains("proj / wf1"));
        assert!(output.contains("Completed"));
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
        assert!(
            contents.contains("layout"),
            "template should mention layout"
        );
        assert!(
            contents.contains("claude"),
            "template should mention claude"
        );
        assert!(
            contents.contains("deploy"),
            "template should mention deploy"
        );
        assert!(
            contents.contains("autopilot"),
            "template should mention autopilot"
        );
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
        assert_eq!(
            contents, original,
            "existing file should not be overwritten"
        );
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
        assert!(
            result.is_err(),
            "should fail when editor binary doesn't exist"
        );
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

        assert!(
            marker.exists(),
            "unrelated files in .config/ should be preserved"
        );
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
            println!("  list [project]            — list available workflows (built-in + per-repo custom)");
            println!("  status [project]          — show persisted workflow run states");
            println!("  prune [project]           — delete terminal workflow run states");
            println!("  run <project> <workflow>  — run or resume an autopilot workflow");
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
        Some("prune") => {
            let project_filter = args.get(1).map(|s| s.as_str());
            cmd_autopilot_prune(project_filter)
        }
        Some("run") => {
            let project = args.get(1).map(|s| s.as_str()).unwrap_or("");
            let workflow = args.get(2).map(|s| s.as_str()).unwrap_or("");
            if project.is_empty() || workflow.is_empty() {
                return Err(z_core::error::ZError::Io(
                    "usage: z autopilot run <project> <workflow>".to_string(),
                ));
            }
            cmd_autopilot_run(project, workflow)
        }
        Some(unknown) => Err(z_core::error::ZError::Io(format!(
            "unknown autopilot subcommand: {:?}\nusage: z autopilot [list|status|prune|run]",
            unknown
        ))),
    }
}

/// List all available autopilot workflows: built-in + per-repo custom workflows
/// for the given project path (if provided).
pub fn cmd_autopilot_list(project_path: Option<&std::path::Path>) -> z_core::error::Result<()> {
    let all_workflows = load_autopilot_workflows(project_path)?;

    println!("{}", format_workflow_list(&all_workflows));
    Ok(())
}

fn load_autopilot_workflows(
    project_path: Option<&std::path::Path>,
) -> z_core::error::Result<Vec<AutopilotWorkflow>> {
    let mut all_workflows: Vec<AutopilotWorkflow> = builtin_workflows()
        .map_err(|e| z_core::error::ZError::Io(format!("load built-in workflows: {e}")))?;

    if let Some(path) = project_path {
        let repo_config_path = path.join(".config").join("z.kdl");
        if let Ok(content) = fs::read_to_string(&repo_config_path) {
            match repo_config::parse_repo_config_projection(&content) {
                Ok(projection) => all_workflows.extend(projection.workflows),
                Err(e) => eprintln!(
                    "warning: failed to parse {}: {}",
                    repo_config_path.display(),
                    e
                ),
            }
        }
    }

    Ok(all_workflows)
}

fn cmd_autopilot_run(project_name: &str, workflow_name: &str) -> z_core::error::Result<()> {
    let store = KdlProjectStore::new();
    let project = store.get_project(project_name)?;
    let workflows = load_autopilot_workflows(Some(&project.path))?;
    let workflow = workflows
        .into_iter()
        .find(|workflow| workflow.name == workflow_name)
        .ok_or_else(|| {
            z_core::error::ZError::ConfigParse(format!(
                "unknown autopilot workflow '{}' for project '{}'",
                workflow_name, project_name
            ))
        })?;

    let global = load_global_config();
    let notify_session =
        resolve_session_env().unwrap_or_else(|| format!("{}:autopilot", project.name));
    let event_notifier = DispatchNotifier::from_config(&global.notifications, &notify_session);
    let step_notifier = DispatchNotifier::from_config(&global.notifications, &notify_session);
    let executor = autopilot_runner::CliStepExecutor::new(
        project.path.clone(),
        project.host.clone(),
        Box::new(step_notifier),
    );
    let run_store = autopilot_runner::FileRunStore::new(autopilot_state_dir());

    let report = execute_workflow_run(
        &workflow,
        &project.name,
        project.host.clone(),
        &executor,
        &run_store,
        &event_notifier,
        RunLoopOptions::default(),
    )?;

    println!("{}", format_run_loop_report(&report));
    Ok(())
}

/// Show persisted workflow run states, optionally filtered to a project.
pub fn cmd_autopilot_status(project_filter: Option<&str>) -> z_core::error::Result<()> {
    let state_dir = autopilot_state_dir();
    let runs = list_runs(&state_dir)?;

    let filtered: Vec<&WorkflowRun> = runs
        .iter()
        .filter(|r| project_filter.map_or(true, |p| r.project == p))
        .collect();

    println!("{}", format_run_status(&filtered));
    Ok(())
}

/// Delete terminal workflow run states, optionally filtered to a project.
pub fn cmd_autopilot_prune(project_filter: Option<&str>) -> z_core::error::Result<()> {
    let state_dir = autopilot_state_dir();
    let removed = prune_terminal_runs(&state_dir, project_filter)?;

    println!("{}", format_prune_status(&removed));
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
        out.push_str(&format!(
            "  {:30}  trigger: {:25}  {}\n",
            wf.name, trigger, desc
        ));
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

pub fn format_prune_status(removed: &[WorkflowRun]) -> String {
    if removed.is_empty() {
        return "No terminal workflow runs to prune.".to_string();
    }

    let mut out = format!("Pruned {} terminal workflow run(s):\n", removed.len());
    for run in removed {
        out.push_str(&format!(
            "  {} / {} ({:?})\n",
            run.project, run.workflow_name, run.status
        ));
    }
    out
}

pub fn format_run_loop_report(report: &RunLoopReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Autopilot run: {} for {}\n",
        report.run.workflow_name, report.run.project
    ));
    out.push_str(&format!("Status: {:?}\n", report.run.status));
    out.push_str(&format!("Executed steps: {}\n", report.outcomes.len()));
    match report.stop {
        RunLoopStop::Terminal => out.push_str("Stop: terminal\n"),
        RunLoopStop::StepLimitReached => out.push_str("Stop: step limit reached\n"),
    }
    if let Some(step) = &report.run.current_step {
        out.push_str(&format!("Next step: {}\n", step));
    }
    out
}

fn cmd_list() -> z_core::error::Result<()> {
    let store = KdlProjectStore::new();
    let session_mgr = ZellijSessionManager {
        bin_path: resolve_bin_path(),
    };

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
            match remote::list_remote_sessions(host, &project.name) {
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
