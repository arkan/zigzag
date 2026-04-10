# New Session Menu (N key) — Issue/PR context

## Goal
Replace the `N` key's simple branch input with a 3-choice menu: Blank / From Issue / From PR.
Issue/PR modes create a worktree, open the session, and inject a `/grill-me` prompt with context.

## Design Decisions
- Fuzzy finder: internal (reuse `fuzzy_match` pattern), not external `fzf`
- Prompt template: overridable in KDL config with placeholder injection
- Placeholders: `{number}`, `{title}`, `{body}`, `{url}`, `{branch}`
- Default templates:
  - Issue: `/grill-me We are going to work on issue #{number}: {title}. Fetch full context and all comments with gh issue view {number} --comments`
  - PR: `/grill-me We are going to review PR #{number}: {title}. Fetch full context, diff, and all comments with gh pr view {number} --comments`
- Branch naming: issue → `grill/<number>-<slug>`, PR → existing branch
- Esc at any step → cancel everything
- Prompt passed via `claude --prompt "..."` arg in layout pane

## Implementation

### Phase 1: Domain & Config (z-core)

**1a. domain.rs — add GH item types**
- Add `GhIssue { number, title, body, url }` struct
- Add `GhPr { number, title, body, url, branch }` struct

**1b. config.rs — add prompt templates to config**
- Add to `GlobalConfig`:
  ```rust
  pub issue_prompt_template: Option<String>,
  pub pr_prompt_template: Option<String>,
  ```
- Add to `PerRepoConfig`:
  ```rust
  pub issue_prompt_template: Option<String>,
  pub pr_prompt_template: Option<String>,
  ```
- Parse `issue-prompt-template` / `pr-prompt-template` from KDL config block
- Add `resolve_prompt_template(template: &str, vars: &HashMap<&str, String>) -> String` — replace `{key}` with values

**1c. layout.rs — no changes needed**
- Prompt is just an extra arg on the Claude pane, handled at session creation time

### Phase 2: TUI (z-tui)

**2a. Modal enum — add 2 new variants**
```rust
Modal::NewSessionMenu { project, selected: usize }  // 0=Blank, 1=Issue, 2=PR
Modal::GhPicker { project, kind: GhPickerKind, items: Vec<GhPickerItem>, filtered: Vec<usize>, query: String, selected: usize, loading: bool }
```
Where:
```rust
enum GhPickerKind { Issue, Pr }
struct GhPickerItem { number: u64, title: String, branch: Option<String> }
```

**2b. ModalOutcome — add new variant**
```rust
ModalOutcome::NewFromIssue { project, number, title, slug }
ModalOutcome::NewFromPr { project, number, title, branch }
```

**2c. TuiAction — add new variant**
```rust
TuiAction::NewFromIssue { project, number, title, slug }
TuiAction::NewFromPr { project, number, title, branch }
```

**2d. Key handling — N key**
- Change `KeyCode::Char('n')` to open `Modal::NewSessionMenu` instead of `Modal::BranchInput`

**2e. Key handling — NewSessionMenu modal**
- Up/Down/j/k: navigate 3 choices
- Enter on Blank → open `Modal::BranchInput` (existing)
- Enter on Issue → spawn `gh issue list --json number,title --limit 50` in background, open `Modal::GhPicker { kind: Issue, loading: true }`
- Enter on PR → spawn `gh pr list --json number,title,headRefName --limit 50` in background, open `Modal::GhPicker { kind: Pr, loading: true }`
- Esc → close

**2f. Key handling — GhPicker modal**
- Typing → updates query, refilters items with `fuzzy_match`
- Up/Down → navigate filtered list
- Enter → emit `ModalOutcome::NewFromIssue` or `NewFromPr`
- Esc → back to NewSessionMenu (or close entirely)

**2g. Rendering**
- `render_new_session_menu()` — 3-item list in centered modal
- `render_gh_picker()` — search input + scrollable list, similar to BranchInput but with list

**2h. Background gh fetch**
- Use `std::process::Command` in a thread, send results back via channel
- While loading, show spinner in GhPicker modal

### Phase 3: CLI (z-cli)

**3a. main.rs — handle new TuiActions**
- `TuiAction::NewFromIssue { project, number, title, slug }`:
  1. Branch name: `grill/{number}-{slug}`
  2. Create worktree via `wt_mgr.create_worktree()`
  3. Build prompt from template (resolve placeholders)
  4. Call `cmd_open_with_prompt(&project, &branch, Some(prompt))`
- `TuiAction::NewFromPr { project, number, title, branch }`:
  1. Create worktree (checkout existing branch)
  2. Build prompt from template
  3. Call `cmd_open_with_prompt(&project, &branch, Some(prompt))`

**3b. cmd_open_with_prompt — new function (or extend cmd_open)**
- Same as `cmd_open` but accepts `prompt: Option<String>`
- When prompt is Some, inject `--prompt "<escaped>"` into Claude pane args before creating session
- Modify the layout's Claude pane args in-place before passing to `session_mgr.create_session()`

### Phase 4: Slug generation utility
- `fn slugify(title: &str) -> String` in domain.rs
- Lowercase, replace non-alphanum with `-`, truncate to ~40 chars, trim trailing `-`

## File changes summary

| File | Changes |
|------|---------|
| `z-core/src/domain.rs` | Add `GhIssue`, `GhPr`, `slugify()` |
| `z-core/src/config.rs` | Add template fields + parsing + `resolve_prompt_template()` |
| `z-tui/src/lib.rs` | New modal variants, outcomes, key handlers, renderers, bg fetch |
| `z-cli/src/main.rs` | New TuiAction variants, `cmd_open_with_prompt()`, template resolution |

## Open questions
None — all resolved during grill-me session.
