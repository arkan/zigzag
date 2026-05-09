# Architecture Deepening Implementation Plan

## Goal
- Implement selected deepening points 1-5 safely, one validated slice at a time.

## Current Strategy
- Start with Point 2 because `z-core/src/activity.rs` is unmodified, isolated, and has passing focused tests.
- Avoid files with staged plus unstaged changes until their state is stable.
- Keep external process and filesystem Implementation behind Adapter edges.

## Checklist
- [x] Point 2: deepen the Session activity Module with a concrete path-scoped Adapter and focused tests.
- [x] Point 3: move Zellij session JSON parsing behind a pure `z-core::zellij` Module.
- [x] Point 5: add a `ConfigEnvironment` Seam for env token and tilde expansion.
- [x] Point 1: extract ProjectEntry assembly and repo workspace config parsing from `cmd_tui()`.
- [x] Point 4: add a pure lifecycle Module for advance-plus-event ordering.

## Validation
- `cargo test --manifest-path z/Cargo.toml -p z-core -- activity`
- `cargo check --manifest-path z/Cargo.toml --all`
- Full workspace tests before DONE.

## Results
- Point 1: `z-cli::workspace` now owns ProjectEntry assembly and per-repo workspace config parsing.
- Point 2: `z_core::activity::ActivityLog` concentrates persisted Session activity.
- Point 3: `z_core::zellij` owns pure Zellij JSON parsing for Preview data.
- Point 4: `z_autopilot::lifecycle::advance_run` combines transition and event capture.
- Point 5: `z_core::config::ConfigEnvironment` makes env token and tilde resolution injectable.
- Validation: `cargo test --manifest-path z/Cargo.toml --all` passed; `git diff --check` passed.

## Open Questions
- None for Point 2 first slice.
- Remaining points require a new confidence checkpoint before edits.
