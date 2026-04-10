# PRD: New Session Menu — Create sessions from GitHub Issues and PRs

## Problem Statement

When starting a new coding session in z, the user must manually create a branch, open the session, then type a `/grill-me` prompt with the relevant issue or PR context. This is repetitive friction: the user already knows they want to work on a specific issue or review a specific PR, but must perform several manual steps to get Claude bootstrapped with the right context. There is no way to go from "I want to work on issue #42" to a ready-to-go Claude session in a single interaction.

## Solution

Replace the current `N` (new session) key with a 3-choice menu: **Blank**, **From Issue**, **From PR**. The Issue and PR options present a fuzzy-searchable list fetched from GitHub, then automatically create a git worktree, open the session, and inject a `/grill-me` prompt with the selected item's context into the Claude pane. The prompt template is overridable in the KDL config with placeholder support.

## User Stories

1. As a developer, I want to press `N` and see a menu of session creation modes, so that I can choose the workflow that matches my intent.
2. As a developer, I want to select "From Issue" and fuzzy-search through open issues, so that I can quickly find the issue I want to work on.
3. As a developer, I want to select "From PR" and fuzzy-search through open PRs, so that I can quickly pick a PR to review.
4. As a developer, I want the tool to automatically create a git worktree with a meaningful branch name when I select an issue, so that I don't have to manually create and name branches.
5. As a developer, I want the tool to checkout the PR's existing branch in a new worktree when I select a PR, so that I'm immediately on the right code.
6. As a developer, I want Claude to start with a pre-filled `/grill-me` prompt that includes the issue/PR number and title, so that Claude has context from the start.
7. As a developer, I want Claude's prompt to instruct fetching full context and comments via `gh`, so that Claude has comprehensive information without me copying anything.
8. As a developer, I want to press Esc at any step of the flow to cancel and return to the main view, so that I never get stuck in a partial state.
9. As a developer, I want "Blank" to behave exactly like the current `N` key (branch name input), so that existing workflows are preserved.
10. As a developer, I want to customize the prompt template in my global or per-repo KDL config, so that I can adapt the initial prompt to my workflow.
11. As a developer, I want to use placeholders like `{number}`, `{title}`, `{body}`, `{url}`, and `{branch}` in prompt templates, so that templates are dynamic and reusable.
12. As a developer, I want per-repo config to override global config for prompt templates, so that different projects can have different workflows.
13. As a developer, I want the fuzzy finder to work inline in the TUI (not suspend to an external process), so that the experience is seamless.
14. As a developer, I want to see a loading indicator while GitHub data is being fetched, so that I know the tool is working.
15. As a developer, I want branch names for issues to follow a `grill/<number>-<slug>` convention, so that branches are identifiable and organized.

## Implementation Decisions

### Modal Flow (TUI state machine)

The `N` key opens a new `NewSessionMenu` modal with 3 choices. Selecting "Blank" transitions to the existing `BranchInput` modal. Selecting "Issue" or "PR" transitions to a new `GhPicker` modal that fetches items via `gh` CLI in a background thread and presents them with inline fuzzy filtering.

### New Modal Variants

- `NewSessionMenu`: 3-item vertical list (Blank / From Issue / From PR). Navigation with arrows/j/k, Enter to select, Esc to close.
- `GhPicker`: Search input + scrollable filtered list. Typing filters, arrows navigate, Enter selects, Esc cancels back to main view.

### New TuiAction Variants

- `NewFromIssue { project, number, title, slug }` — triggers worktree creation on `grill/<number>-<slug>` branch + session with prompt.
- `NewFromPr { project, number, title, branch }` — triggers worktree checkout of PR branch + session with prompt.

### GitHub Data Fetching

- Issues: `gh issue list --json number,title,body,url --limit 50`
- PRs: `gh pr list --json number,title,body,url,headRefName --limit 50`
- Executed in a background thread via `std::process::Command`, results sent back via channel to avoid blocking the TUI event loop.

### Prompt Template Engine

A pure function `resolve_template(template: &str, vars: &HashMap<&str, &str>) -> String` that replaces `{key}` placeholders with values from the map. Supported placeholders: `{number}`, `{title}`, `{body}`, `{url}`, `{branch}`.

### Default Templates

- Issue: `/grill-me We are going to work on issue #{number}: {title}. Fetch full context and all comments with gh issue view {number} --comments`
- PR: `/grill-me We are going to review PR #{number}: {title}. Fetch full context, diff, and all comments with gh pr view {number} --comments`

### Config Override

Two new optional fields in both `GlobalConfig` and `PerRepoConfig`: `issue_prompt_template` and `pr_prompt_template`. Parsed from KDL as `issue-prompt-template "..."` and `pr-prompt-template "..."` inside the `config` block. Resolution order: per-repo > global > hardcoded default.

### Prompt Injection into Claude Pane

The prompt is passed as `--prompt "<escaped_text>"` added to the Claude pane's args in the Layout before session creation. The existing `cmd_open` function is extended with an optional `prompt` parameter. When present, it finds the Claude pane in the effective layout and appends `--prompt` to its args.

### Branch Naming

- Issue: `grill/<number>-<slugified_title>` where slug is lowercase, non-alphanum replaced with `-`, truncated to ~40 chars, trailing `-` trimmed.
- PR: uses the PR's existing `headRefName` branch.

### Worktree Creation

Reuses the existing `WtWorktreeManager` — same `wt switch -c <branch>` flow as the current "Blank" new session.

## Testing Decisions

Good tests verify external behavior through the public interface, not implementation details. They should be deterministic, fast, and describe *what* the module does rather than *how*.

### Modules to test

1. **Prompt Template Engine** — unit tests for placeholder resolution: single placeholder, multiple placeholders, missing placeholder (left as-is), empty template, special characters in values.
2. **Slugify** — unit tests for edge cases: spaces, special chars, unicode, long titles, empty string, already-clean input.
3. **GitHub JSON Parsing** — unit tests with fixture JSON strings: parse issues, parse PRs, handle empty results, handle malformed JSON gracefully.
4. **Config Parsing** — extend existing config parsing tests: verify `issue-prompt-template` / `pr-prompt-template` are parsed from KDL, verify per-repo overrides global, verify absent fields default to None.

### Prior art

Existing tests in `z-core/src/domain.rs` (sanitize_branch_name, Session::new) and `z-core/src/layout.rs` (generate_layout_kdl) follow the same pattern: pure function + assert on output.

## Out of Scope

- Modifying the `/grill-me` skill itself — the prompt is injected as a user message, no skill changes needed.
- Filtering issues/PRs by label, assignee, or milestone — just list open items.
- Caching GitHub API results between sessions.
- Creating GitHub issues or PRs from the TUI.
- Supporting other Git forges (GitLab, etc.) — `gh` CLI only.

## Further Notes

- The `gh` CLI must be installed and authenticated for Issue/PR modes to work. If `gh` is not available, the menu should still show the options but display an error when selected.
- The fuzzy matching reuses the existing `fuzzy_match()` function already in `z-tui`.
- The background thread for `gh` fetching should have a reasonable timeout (~5s) to avoid hanging on slow networks.
