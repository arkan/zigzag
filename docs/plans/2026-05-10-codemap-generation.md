# Codemap Generation

## Goal
- Generate hierarchical codemaps for the repository.
- Include core code/config only; exclude tests, docs, build outputs, dependencies, translations.

## Checklist
- [x] Check `.slim/codemap.json` or legacy `.slim/cartography.json`.
- [x] Initialize or detect codemap changes.
- [x] Write directory `codemap.md` files for affected source folders.
- [x] Write root `codemap.md` atlas.
- [x] Register the atlas in `AGENTS.md`.
- [x] Validate codemap state and git diff.

## Strategy
- No existing `.slim` state was found, so initialize from core Rust workspace files and repository integration config.
- Use `codemap.mjs init` with explicit include/exclude globs.
- Delegate source-folder codemap writing to focused fixers, then aggregate the root atlas in this session.
- Run `codemap.mjs update` and `git diff --check` before completion.

## Result
- Initialized `.slim/codemap.json` with 67 core code/config files.
- Wrote 15 directory codemaps plus the root repository atlas.
- Created `AGENTS.md` with the repository map discovery section.

## Validation
- `node ~/.config/opencode/skills/codemap/scripts/codemap.mjs update --root ./` — passed.
- `node ~/.config/opencode/skills/codemap/scripts/codemap.mjs changes --root ./` — passed, no changes detected.
- `git diff --check` — passed.
