# Architecture Deepening Review Plan

## Goal
- Surface deepening opportunities that improve Depth, Locality, Leverage, testability, and AI-navigability.

## Steps
- [x] Map repo docs: glossary, architecture entry point, ADRs, anti-patterns, boundaries, patterns, plans.
- [x] Read available architecture/domain docs and infer project vocabulary.
- [x] Explore codebase friction with @explorer.
- [x] Apply deletion test to candidate Modules.
- [x] Present numbered opportunities only; do not propose Interfaces yet.

## Verification
- [x] Candidates avoid no-touch zones and documented anti-patterns.
- [x] Candidate language uses Module, Interface, Implementation, Depth, Seam, Adapter, Leverage, Locality.
- [x] Ask which candidate to explore next.

## Review
- Architecture entry point: README.md architecture section and docs/SPECS.md.
- Glossary: inferred from docs/PRD.md and docs/SPECS.md; no dedicated glossary file found.
- ADRs: docs/SPECS.md decision log.
- Plans directory: docs/plans/.
- Main candidates: Session workspace orchestration, Session signal store, Preview data acquisition, Autopilot run lifecycle, Configuration resolution.

## Open questions
- None before exploration.
