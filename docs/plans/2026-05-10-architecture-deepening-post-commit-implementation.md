# Architecture Deepening Post-Commit Implementation

## Goal
- Implement points 1-5 from `docs/plans/2026-05-10-architecture-deepening-post-commit-review.md`.

## Checklist
- [x] Point 1: Autopilot run Module.
- [x] Point 2: TUI state transition Module.
- [x] Point 3: Preview transport Module.
- [x] Point 4: Configuration Module.
- [x] Point 5: Session entry / Action context Module.
- [x] Full validation.

## Strategy
- Build on the existing seams introduced by the previous architecture commits.
- Prefer pure Modules and CLI Adapters over direct process/filesystem calls in domain crates.
- Keep Worktree delegated to `wt` and Forge delegated to `gh`.
- Validate each point with focused tests, then validate the whole workspace.

## Notes
- Previous durable loop archived at `.opencode-goal/archive/20260510-014423/`.
- Point 1 evidence: lifecycle/persist/notify are implemented but not composed by `z-cli`; add a run loop Module plus CLI Adapter.
- Point 1 result: `z-autopilot::run_loop` owns execution/persistence/notification ordering; `z-cli::autopilot_runner` provides concrete file/process Adapters; `z autopilot run <project> <workflow>` and TUI workflow selection now use it.
- Point 2 evidence: async refresh can return after synchronous reload and still apply data; reload helpers mutate `entries`/`notifications` directly.
- Point 2 result: refresh messages carry a state revision, sync reloads go through `apply_reloaded_entries`, and stale async refreshes are discarded before mutating TUI state.
- Point 3 evidence: local Preview Git and remote Preview Git use different commands/parsers; Forge snapshot aggregation is implicit in `load_extra_preview`.
- Point 3 result: `z-cli::git_preview` owns shared local/remote Git Preview parsing and command construction; `remote` remains the SSH Adapter; Forge Preview aggregation is explicit in `z-cli::preview`.
- Point 4 result: `z_core::config::AutopilotConfig` is canonical; unnamed Autopilot config parsing lives in `z-core`; `z-autopilot::config` re-exports the type and only owns workflow-specific resolution.
- Point 5 evidence: notification clearing/activity recording are scattered across session entry paths; ActionEnv construction is repeated manually with many optional fields.
- Point 5 result: `z-core::session_entry` owns best-effort Session entry effects; `ActionEnv` constructors make project vs session action context explicit for CLI and TUI callers.

## Validation
- `cargo test --manifest-path z/Cargo.toml --all` — passed.
- `git diff --check` — passed.
