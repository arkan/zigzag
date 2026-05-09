# Architecture Deepening Follow-up Implementation Plan

## Goal
- Implement durable slices for follow-up architecture points 1-5.

## Points
- [x] Point 1: Session signal Module.
- [x] Point 2: Preview acquisition Module.
- [x] Point 3: TUI action/open orchestration Module.
- [x] Point 4: Autopilot execution lifecycle Module.
- [x] Point 5: Forge data Module.

## Strategy
- Research each target before editing.
- Prefer pure Modules and thin Adapters.
- Validate each slice with focused tests, then run full workspace validation before completion.

## Acceptance
- Improved Depth, Leverage, and Locality for each selected point.
- No behavior regressions.
- Full validation passes.

## Validation
- `cargo test --manifest-path z/Cargo.toml --all` — passed.
- `git diff --check` — passed.
- `cargo fmt --manifest-path z/Cargo.toml --all --check` — reported broad existing formatting diffs in TUI files; not used as the completion gate.

## Results
- Point 1: `z-core` now contains pure Session signal traits/helpers; file-backed Activity and Notification persistence live in `z-cli` Adapters. `SessionRefresher` supplies Activity to the TUI refresh seam.
- Point 2: Preview acquisition now goes through `z_tui::PreviewDataSource`; concrete Git/Zellij/SSH/Forge acquisition lives in `z-cli::preview`.
- Point 3: Zellij Action execution and Open Session attach-vs-create planning are extracted into dedicated CLI Modules.
- Point 4: `z-autopilot::lifecycle` now owns step execution orchestration through `StepExecutor`, then advances state and captures events in one Module.
- Point 5: `z-core::gh` now owns PR view, CI status, and review status JSON parsing. `z-cli::forge` is a thin `gh` Adapter.
