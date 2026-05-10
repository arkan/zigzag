# Repository Atlas: z

## Project Responsibility
`z` is a Rust-first TUI/CLI project manager for Zellij-based development. It coordinates project discovery, Zellij session lifecycle, git worktree management through `wt`, GitHub PR/CI/review signals through `gh`, notification delivery, and KDL-defined Autopilot workflows from one dashboard.

## System Entry Points
- `README.md`: Human-facing overview, install instructions, usage, configuration, and architecture orientation.
- `package.json`: Repository utility manifest. Exposes `npm test` as `cargo test --manifest-path z/Cargo.toml --all`, `npm run typecheck` as workspace `cargo check`, and a Sandcastle postinstall patch hook.
- `Makefile`: Developer command shortcuts for building/installing the Rust CLI.
- `flake.nix`: Nix development environment and reproducible dependency shell.
- `install.sh`: Release installer that places the `z` binary into the selected install directory.
- `.opencode/plugins/z-notify.js`: OpenCode notification bridge that shells out to `z notify` when an OpenCode session becomes idle, needs permission, or errors.
- `z/Cargo.toml`: Rust workspace manifest for all runtime crates.

## Repository Directory Map
| Directory | Responsibility Summary | Detailed Map |
|-----------|------------------------|--------------|
| `z/` | Rust workspace for the `z` runtime: CLI binary, TUI, shared domain crate, Autopilot engine, and future extension crates. | [View Map](z/codemap.md) |
| `z/crates/` | Collection of Rust crates with a layered dependency graph: `z-core` at the center, concrete adapters in `z-cli`, UI in `z-tui`, workflow engine in `z-autopilot`, and stubs for web/plugin phases. | [View Map](z/crates/codemap.md) |
| `z/crates/z-core/` | I/O-agnostic domain and policy crate: config parsing, action resolution, layout generation, traits, themes, notification/activity helpers, Forge parsers, and session entry effects. | [View Map](z/crates/z-core/codemap.md) |
| `z/crates/z-core/src/` | Source modules for the pure domain layer and trait Interfaces consumed by CLI/TUI/Autopilot adapters. | [View Map](z/crates/z-core/src/codemap.md) |
| `z/crates/z-cli/` | Binary crate that owns command dispatch, concrete process/filesystem adapters, TUI wiring, OpenCode/notification hooks, worktree/session orchestration, and Autopilot CLI commands. | [View Map](z/crates/z-cli/codemap.md) |
| `z/crates/z-cli/src/` | Source modules implementing CLI subcommands, process adapters for `zellij`, `wt`, `git`, `gh`, `ssh`, filesystem stores, Preview acquisition, repo config projection, and session/action orchestration. | [View Map](z/crates/z-cli/src/codemap.md) |
| `z/crates/z-tui/` | Ratatui frontend crate that renders projects/sessions/preview panes and emits side-effect-free `TuiAction` values through callback injection. | [View Map](z/crates/z-tui/codemap.md) |
| `z/crates/z-tui/src/` | TUI state machine, event loop, modal handling, async worker polling, preview transitions, refresh merge logic, and standalone pickers. | [View Map](z/crates/z-tui/src/codemap.md) |
| `z/crates/z-autopilot/` | Workflow engine crate for KDL-defined Autopilot runs: DSL parsing, state transitions, execution lifecycle, persistence, notifications, triggers, and config resolution. | [View Map](z/crates/z-autopilot/codemap.md) |
| `z/crates/z-autopilot/src/` | Autopilot source modules for workflow definitions, run lifecycle, StepExecutor/RunStore seams, retry semantics, persistence layout, and notification mapping. | [View Map](z/crates/z-autopilot/src/codemap.md) |
| `z/crates/z-web/` | Phase-gated web crate reserved for an HTTP/WebSocket bridge over `z-core` capabilities. Currently a stub. | [View Map](z/crates/z-web/codemap.md) |
| `z/crates/z-web/src/` | Stub source module documenting the intended axum/ratatui-WASM web surface. | [View Map](z/crates/z-web/src/codemap.md) |
| `z/crates/z-plugin/` | Phase-gated plugin crate reserved for future WASM plugin runtime integration. Currently a stub. | [View Map](z/crates/z-plugin/codemap.md) |
| `z/crates/z-plugin/src/` | Empty plugin source namespace used to reserve the crate and dependency position. | [View Map](z/crates/z-plugin/src/codemap.md) |
| `scripts/` | Repository automation scripts for Sandcastle patching and Docker-authenticated Sandcastle runs. | [View Map](scripts/codemap.md) |

## Core Data & Control Flow
1. CLI entry starts in `z-cli/src/main.rs`, loads global/per-repo config, checks dependencies, and dispatches subcommands or the TUI.
2. TUI data starts as `ProjectEntry` values from `z-cli::workspace`, then `z-tui` renders state and returns `TuiAction` values to CLI callbacks.
3. Session entry flows through `z-core::session_entry` for activity/notification effects, then `z-cli` adapters call Zellij, `wt`, git, SSH/Mosh, or filesystem stores.
4. Preview data flows through `z_tui::PreviewDataSource`; `z-cli::preview` supplies concrete git/worktree/Zellij/GitHub acquisition while `z-tui::preview_state` owns pure state transitions.
5. Autopilot flows through `z-autopilot` DSL/config parsing, run persistence, `run_loop`, and `lifecycle`; `z-cli::autopilot_runner` provides process-backed step execution and notifications.

## Integration Points
- External commands: `zellij`, `wt`, `git`, `gh`, `ssh`, `mosh`, `claude`, configured review tools.
- Filesystem contracts: `~/.config/z/projects.kdl`, per-repo `.config/z.kdl`, `~/.local/state/z/autopilot`, session notification/activity stores, generated Zellij layout files.
- Config format: KDL parsed by `z-core::config`, `z-cli::repo_config`, and `z-autopilot::dsl`.
- Notification channels: `z notify`, macOS notification adapter, Telegram hook support, and OpenCode plugin bridge.
- Agent navigation: start here, then follow the `Repository Directory Map` to the relevant crate or source-folder codemap before editing.
