# Architecture deepening review after connected seams

## Goal
- Re-audit the codebase after `1ea7cbe refactor: connect architecture seams`.
- Surface new deepening opportunities only; do not propose Interfaces yet.
- Respect existing decisions: `z-core` I/O-agnostic, Worktree via `wt`, Forge via `gh`, GitHub-only.

## Mapping
- Architecture entry point: `README.md`, `docs/SPECS.md`.
- Domain glossary: inferred from `docs/PRD.md` and `docs/SPECS.md`.
- ADRs: decision log in `docs/SPECS.md`.
- Anti-patterns / forbidden patterns: no custom Worktree management; no I/O in `z-core`; GitHub-only Forge.
- Boundaries / ownership: no generated/no-touch zones found.
- Architecture patterns: Rust workspace in `docs/SPECS.md`; I/O behind traits/Adapters.
- Plans directory: `docs/plans/`.

## Checklist
- [x] Read architecture entry point and decision log.
- [x] Read PRD/SPECS glossary vocabulary.
- [x] Explore post-commit friction across crates.
- [x] Consolidate candidates using deletion test.
- [x] Validate audit file and present candidates.

## Notes
- This is an audit-only plan. Accepted candidates need a separate implementation plan.

## Candidate Summary
1. Preview Worktree Module: remove duplicate `git worktree list --porcelain` parsing from Preview and reuse the existing Worktree Adapter path.
2. Action context Module: make `z actions` and TUI action menu use one Preview-derived action context path.
3. Autopilot run persistence Module: make run state lifecycle include terminal run cleanup/prune, not only load/save/list.
4. Configuration projection Module: parse `.config/z.kdl` once and project repo config/actions/workflows from one local Module; split the oversized config file afterward.
5. TUI transition Module: continue moving implicit state transitions out of ad hoc polling and callbacks into a deeper state transition Module.
