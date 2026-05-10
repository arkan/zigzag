# Architecture Deepening Post-Commit Review

## Goal
- Re-run architecture deepening review after commit `053d864`.
- Surface next deepening opportunities only; do not propose concrete Interfaces yet.

## Mapping
- Architecture entry point: `README.md`, `docs/SPECS.md`.
- Glossary: inferred from `docs/PRD.md` and `docs/SPECS.md`.
- ADRs / decisions: implementation decisions in `docs/PRD.md` and architecture constraints in `docs/SPECS.md`.
- Plans directory: `docs/plans/`.

## Constraints
- Keep `z-core` I/O-agnostic.
- Keep Worktree management delegated to `wt`.
- Keep Forge access delegated to `gh` through CLI Adapters.
- Use Module, Interface, Implementation, Depth, Seam, Adapter, Leverage, Locality vocabulary.

## Exploration Summary
- Read project orientation docs and latest follow-up implementation plan.
- Explored `z-core`, `z-cli`, `z-tui`, `z-autopilot`, `z-web`, and `z-plugin` after the previous deepening commit.
- Validated key candidate locations with targeted searches and reads.

## Candidate Themes
1. Autopilot run execution is still not connected to CLI/TUI entry points.
2. TUI workflow selection exists but discards the selected workflow.
3. TUI state refresh mixes synchronous reloads with background refresh messages.
4. Preview acquisition now has a Seam, but local/remote Git and Forge calls remain shallow and sequential.
5. Configuration and action context Modules still carry broad, weakly-structured Interfaces.

## Validation
- `git status --short` before review: clean.
- `git diff --check` should be run after writing this plan if needed.
