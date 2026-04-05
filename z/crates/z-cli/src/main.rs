mod config_store;
mod depcheck_impl;
mod session_manager;

use z_core::depcheck::{check_deps, format_dep_error, DepCheckStatus};
use z_core::traits::{ProjectStore, SessionManager};

use crate::config_store::KdlProjectStore;
use crate::depcheck_impl::ProcessDepChecker;
use crate::session_manager::ZellijSessionManager;

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
        Some(cmd) => {
            eprintln!("CLI command not yet implemented: {:?}", cmd);
        }
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
