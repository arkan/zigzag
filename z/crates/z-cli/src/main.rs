mod depcheck_impl;

use z_core::depcheck::{check_deps, format_dep_error, DepCheckStatus};

use crate::depcheck_impl::ProcessDepChecker;

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

    // Dependency checks passed — proceed with normal startup.
    run();
}

fn run() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        // No args → TUI mode (phase 1b).
        eprintln!("TUI mode not yet implemented (phase 1b).");
    } else {
        // CLI mode.
        eprintln!("CLI command not yet implemented: {:?}", args);
    }
}
