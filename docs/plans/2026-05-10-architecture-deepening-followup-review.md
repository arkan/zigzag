# Architecture Deepening Follow-up Review Plan

## Goal
- Surface new deepening opportunities after the first architecture seams commit.

## Mapping
- Architecture entry point: `README.md` architecture section and `docs/SPECS.md`.
- Glossary: inferred from `docs/PRD.md` and `docs/SPECS.md`; no dedicated glossary file found.
- ADRs: `docs/SPECS.md` decision log.
- Boundaries / anti-patterns: no dedicated file found; respect the documented `z-core` I/O-agnostic constraint and `wt` worktree decision.
- Plans directory: `docs/plans/`.

## Steps
- [x] Map repo docs and decisions.
- [x] Read architecture and product vocabulary docs.
- [x] Explore post-commit friction with subagents.
- [x] Apply deletion test to candidate Modules.
- [x] Present numbered candidates only; do not propose Interfaces yet.

## Review
- Main candidates: Session signal store, Preview acquisition, TUI action/open orchestration, Autopilot execution lifecycle, Forge parsing, Autopilot config locality.
- Strongest decision tension: `z-core` still contains filesystem/environment I/O despite the documented I/O-agnostic constraint.
- No ADR conflict requires reopening `wt` or `gh` decisions.

## Verification
- Candidates use Module, Interface, Implementation, Depth, Seam, Adapter, Leverage, Locality.
- Candidates avoid re-litigating `wt` worktree management and `z-core` WASM/I/O intent unless the friction warrants surfacing a contradiction.
- Ask which candidate to explore next.

## Open Questions
- None before exploration.
