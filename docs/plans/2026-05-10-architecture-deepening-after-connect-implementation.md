# Architecture Deepening After Connected Seams Implementation

## Goal
- Implement points 1-5 from `docs/plans/2026-05-10-architecture-deepening-after-connect-review.md`.
- Keep each change small, permanent, and mechanically validated.

## Checklist
- [x] Point 1: Preview Worktree Module.
- [x] Point 2: Action context Module.
- [x] Point 3: Autopilot run persistence Module.
- [x] Point 4: Configuration projection Module.
- [x] Point 5: TUI transition Module.
- [x] Full validation.

## Strategy
- Build on existing seams from `1ea7cbe`.
- Prefer reusing current traits and Adapters before introducing new Interfaces.
- Keep I/O in `z-cli` or `z-autopilot` persistence Adapters; keep `z-core` pure.
- Validate focused behavior after every point and run full workspace tests at the end.

## Results
- Point 1 result: Preview no longer parses Worktree porcelain directly; Worktree lookup lives in `z-cli::worktree_manager` and reuses the existing local `WtWorktreeManager` plus the shared porcelain parser for SSH discovery.
- Point 2 result: `ActionPreview::from_forge_data` is the shared mapping for PR/CI/review Action context, and `z actions` now uses `CliPreviewDataSource` instead of an always-empty preview.
- Point 3 result: Autopilot persistence now owns delete/prune semantics, `load_or_start_run` removes non-resumable state before restart, and CLI exposes `z autopilot prune [project]`.
- Point 4 result: `.config/z.kdl` projection now has document-level parse functions and a CLI `repo_config` Module that parses once for per-repo config/actions/workflows.
- Point 5 result: Preview state transitions now live in `z-tui::preview_state`; channel polling remains in the TUI Adapter while transition behavior is tested directly.

## Validation
- `cargo test --manifest-path z/Cargo.toml --all` — passed.
- `git diff --check` — passed.
