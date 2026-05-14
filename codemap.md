# Repository Atlas: Zigzag

## Project Responsibility
`zigzag` is a Rust-first TUI/CLI project manager for Zellij-based development. It coordinates project discovery, Zellij session lifecycle, git worktree management through `wt`, GitHub PR/CI/review signals through `gh`, notification delivery, and KDL-defined Autopilot workflows from one dashboard.

## System Entry Points
- `README.md`: Human-facing overview, install instructions, usage, configuration, and architecture orientation.
- `package.json`: Repository utility manifest. Exposes `npm test` as `cargo test --manifest-path zigzag/Cargo.toml --all`, `npm run typecheck` as workspace `cargo check`, and a Sandcastle postinstall patch hook.
- `Makefile`: Developer command shortcuts for building/installing the Rust CLI.
- `flake.nix`: Nix development environment plus default `zigzag` package/app outputs for `nix build`, `nix run`, and `nix profile install`.
- `package.nix`: Reusable Nix package expression for the Rust `zigzag` CLI, wrapping common runtime tools (`zellij`, `wt`, `git`, `gh`, `ssh`, `mosh`).
- `install.sh`: Release installer that places the `zigzag` binary into the selected install directory.
- `agents/opencode/plugins/zigzag-notify.js`: OpenCode notification bridge that shells out to `zigzag notify` when an OpenCode session becomes idle, needs permission, or errors.
- `zigzag/Cargo.toml`: Rust workspace manifest for all runtime crates.

## Repository Directory Map
| Directory | Responsibility Summary | Detailed Map |
|-----------|------------------------|--------------|
| `zigzag/` | Rust workspace for the `zigzag` runtime: CLI binary, TUI, shared domain crate, Autopilot engine, and future extension crates. | [View Map](zigzag/codemap.md) |
| `zigzag/crates/` | Collection of Rust crates with a layered dependency graph: `zigzag-core` at the center, concrete adapters in `zigzag-cli`, UI in `zigzag-tui`, workflow engine in `zigzag-autopilot`, and stubs for web/plugin phases. | [View Map](zigzag/crates/codemap.md) |
| `zigzag/crates/zigzag-core/` | I/O-agnostic domain and policy crate: config parsing, action resolution, layout generation, traits, themes, notification/activity helpers, Forge parsers, and session entry effects. | [View Map](zigzag/crates/zigzag-core/codemap.md) |
| `zigzag/crates/zigzag-core/src/` | Source modules for the pure domain layer and trait Interfaces consumed by CLI/TUI/Autopilot adapters. | [View Map](zigzag/crates/zigzag-core/src/codemap.md) |
| `zigzag/crates/zigzag-cli/` | Binary crate that owns command dispatch, concrete process/filesystem adapters, TUI wiring, OpenCode/notification hooks, worktree/session orchestration, and Autopilot CLI commands. | [View Map](zigzag/crates/zigzag-cli/codemap.md) |
| `zigzag/crates/zigzag-cli/src/` | Source modules implementing CLI subcommands, process adapters for `zellij`, `wt`, `git`, `gh`, `ssh`, filesystem stores, Preview acquisition, repo config projection, and session/action orchestration. | [View Map](zigzag/crates/zigzag-cli/src/codemap.md) |
| `zigzag/crates/zigzag-tui/` | Ratatui frontend crate that renders projects/sessions/preview panes and emits side-effect-free `TuiAction` values through callback injection. | [View Map](zigzag/crates/zigzag-tui/codemap.md) |
| `zigzag/crates/zigzag-tui/src/` | TUI state machine, event loop, modal handling, async worker polling, preview transitions, refresh merge logic, and standalone pickers. | [View Map](zigzag/crates/zigzag-tui/src/codemap.md) |
| `zigzag/crates/zigzag-autopilot/` | Workflow engine crate for KDL-defined Autopilot runs: DSL parsing, state transitions, execution lifecycle, persistence, notifications, triggers, and config resolution. | [View Map](zigzag/crates/zigzag-autopilot/codemap.md) |
| `zigzag/crates/zigzag-autopilot/src/` | Autopilot source modules for workflow definitions, run lifecycle, StepExecutor/RunStore seams, retry semantics, persistence layout, and notification mapping. | [View Map](zigzag/crates/zigzag-autopilot/src/codemap.md) |
| `zigzag/crates/zigzag-web/` | Phase-gated web crate reserved for an HTTP/WebSocket bridge over `zigzag-core` capabilities. Currently a stub. | [View Map](zigzag/crates/zigzag-web/codemap.md) |
| `zigzag/crates/zigzag-web/src/` | Stub source module documenting the intended axum/ratatui-WASM web surface. | [View Map](zigzag/crates/zigzag-web/src/codemap.md) |
| `zigzag/crates/zigzag-plugin/` | Phase-gated plugin crate reserved for future WASM plugin runtime integration. Currently a stub. | [View Map](zigzag/crates/zigzag-plugin/codemap.md) |
| `zigzag/crates/zigzag-plugin/src/` | Empty plugin source namespace used to reserve the crate and dependency position. | [View Map](zigzag/crates/zigzag-plugin/src/codemap.md) |
| `scripts/` | Repository automation scripts for Sandcastle patching and Docker-authenticated Sandcastle runs. | [View Map](scripts/codemap.md) |

## Core Data & Control Flow
1. CLI entry starts in `zigzag-cli/src/main.rs`, loads global/per-repo config, checks dependencies, and dispatches subcommands or the TUI.
2. TUI data starts as `ProjectEntry` values from `zigzag-cli::workspace`, then `zigzag-tui` renders state and returns `TuiAction` values to CLI callbacks.
3. Session entry flows through `zigzag-core::session_entry` for activity/notification effects, then `zigzag-cli` adapters call Zellij, `wt`, git, SSH/Mosh, or filesystem stores.
4. Preview data flows through `zigzag_tui::PreviewDataSource`; `zigzag-cli::preview` supplies concrete git/worktree/Zellij/GitHub acquisition while `zigzag-tui::preview_state` owns pure state transitions.
5. Autopilot flows through `zigzag-autopilot` DSL/config parsing, run persistence, `run_loop`, and `lifecycle`; `zigzag-cli::autopilot_runner` provides process-backed step execution and notifications.

## Integration Points
- External commands: `zellij`, `wt`, `git`, `gh`, `ssh`, `mosh`, `claude`, configured review tools.
- Filesystem contracts: `~/.config/zigzag/projects.kdl`, per-repo `.config/zigzag.kdl`, `~/.local/state/zigzag/zigzag.log`, `~/.local/share/zigzag/autopilot`, session notification/activity stores, generated Zellij layout files.
- Config format: KDL parsed by `zigzag-core::config`, `zigzag-cli::repo_config`, and `zigzag-autopilot::dsl`.
- Notification channels: `zigzag notify`, macOS notification adapter, Telegram hook support, and OpenCode plugin bridge.
- Agent navigation: start here, then follow the `Repository Directory Map` to the relevant crate or source-folder codemap before editing.
