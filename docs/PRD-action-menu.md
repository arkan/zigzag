# Z — Action Menu PRD

See also: [Main PRD](./PRD.md) | [Specs](./SPECS.md)

---

## Problem Statement

Developers using `zigzag` have no way to trigger contextual actions on their projects or sessions from within the TUI. Common workflows — reviewing a PR with an AI tool, fixing CI failures, opening a PR in a browser, addressing review comments — require leaving the TUI, remembering the right command, and manually providing context (branch name, PR number, project path). This friction is amplified on remote machines accessed via SSH, where browser-based actions are unavailable entirely.

There is no extensible mechanism to define custom project-specific or global actions that can leverage the rich context `zigzag` already has (git state, PR data, CI status, review comments).

## Solution

A generic, contextual **action menu** accessible from the TUI via a keyboard shortcut. The menu displays a filtered list of actions based on the current context (project or session) and conditions (has PR, has CI failure, has new review comments). Actions execute shell commands in Zellij panes with full variable interpolation — project path, branch, PR number, etc.

Actions are defined at three levels (built-in, global config, per-repo config) following the existing config merge pattern. Built-in actions cover universal workflows: AI-powered PR review (Codex), CI fix (Claude), review comment resolution (Claude), and opening PRs via OSC 8 hyperlinks.

## User Stories

### Action Menu

1. As a developer, I want to press a shortcut (e.g. `x`) to open an action menu on the currently selected project or session, so that I can trigger contextual actions without leaving the TUI
2. As a developer, I want the action menu to show only actions relevant to my current context (project vs session), so that I don't see irrelevant options
3. As a developer, I want the action menu to filter actions based on conditions (has PR, has CI failure, has new comments), so that I only see actions that make sense right now
4. As a developer, I want to navigate the action menu with arrow keys or vim keys and press Enter to execute, so that it feels consistent with the rest of the TUI
5. As a developer, I want to see a status message after triggering an action, so that I know it was launched successfully
6. As a developer, I want to press Escape to close the action menu without executing anything, so that I can cancel if I changed my mind

### Action Configuration

7. As a developer, I want built-in actions available by default, so that the menu is useful without any configuration
8. As a developer, I want to define custom actions in my global config (`~/.config/zigzag/config.kdl`), so that I have personal actions available across all projects
9. As a developer, I want to define project-specific actions in `.config/zigzag.kdl`, so that each project can have tailored workflows (e.g. `npm test` for Node, `cargo clippy` for Rust)
10. As a developer, I want per-repo actions to override global actions of the same name, so that I can customize behavior per project
11. As a developer, I want to specify which Zellij pane type an action runs in (`float`, `split`, `tab`), so that I can choose the right UX for each action — floating for quick scripts, tab for long-running AI tools
12. As a developer, I want `float` as the default pane type, so that actions are non-intrusive without extra config
13. As a developer, I want to define a `when` condition on actions (`has_pr`, `has_ci_failure`, `has_new_comments`, `always`), so that actions only appear when they are applicable
14. As a developer, I want to configure the default AI review tool globally (default: `codex`), so that all AI-powered built-in actions use my preferred tool without per-action overrides

### Variable Interpolation

15. As a developer, I want action commands to support variable interpolation (`${branch}`, `${pr_number}`, `${project_path}`, etc.), so that scripts receive the right context automatically
16. As a developer, I want variables to be resolved at execution time, so that they always reflect the current state
17. As a developer, I want a clear error message if a variable cannot be resolved (e.g. `${pr_number}` when there is no PR), so that I understand why an action failed

### Built-in Actions

18. As a developer, I want a built-in "Open PR in browser" action that renders the PR URL as an OSC 8 hyperlink, so that I can click it even when working over SSH from a modern terminal (iTerm2, Ghostty, etc.)
19. As a developer, I want a built-in "Review PR" action that launches Codex with a security+architecture+patterns focused prompt, so that I get meaningful AI code review with one keystroke
20. As a developer, I want a built-in "Fix CI" action that launches Claude with the failed CI logs, so that CI failures are resolved without manual log hunting
21. As a developer, I want a built-in "Address review comments" action that launches Claude with the PR review comments, so that I can resolve feedback automatically

### Review Status Enrichment

22. As a developer, I want to see in the preview pane whether a PR has new review comments since the last push, so that I know at a glance if attention is needed
23. As a developer, I want the review comment count displayed in the preview pane, so that I can gauge the volume of feedback
24. As a developer, I want the `has_new_comments` condition to compare review timestamps against the last pushed commit, so that it accurately reflects unaddressed feedback
25. As a developer, I want review status fetched in background alongside PR/CI data, so that the TUI remains responsive

### Remote Compatibility

26. As a developer working over SSH from iOS, I want actions to work identically on remote machines, so that I have the same capabilities everywhere
27. As a developer on a remote machine, I want PR URLs rendered as OSC 8 hyperlinks instead of launching a browser, so that I can still access PRs from a headless server
28. As a developer, I want action scripts to execute in the Zellij session on the machine where `zigzag` runs, so that remote execution is transparent

## Implementation Decisions

### Modules to Build/Modify

#### New: Action Engine (in zigzag-core)

A deep module responsible for:
- Parsing action definitions from KDL
- Merging actions across 3 tiers (built-in → global → per-repo), keyed by action name
- Evaluating `when` conditions against a `ActionContext` struct
- Interpolating variables into command strings
- Producing a filtered, ready-to-execute list of actions for a given context

Interface:
- `ActionRegistry` — loads and merges action definitions from all 3 tiers
- `ActionResolver` — takes an `ActionContext` (project, session, git info, PR info, review status) and returns `Vec<ResolvedAction>` with conditions evaluated and variables interpolated
- `ResolvedAction` — name, resolved command string, pane type, icon

This module is pure logic, no I/O. Testable in isolation.

#### Modified: ForgeClient trait (in zigzag-core)

Add method: `get_review_status(&self, project: &str, branch: &str) -> Result<ReviewStatus>`

New type:
- `ReviewStatus { has_new_comments: bool, comment_count: u32, last_review_at: Option<DateTime> }`

The `gh` implementation batches review data into the existing `gh pr view --json` call by requesting additional fields (`reviews`, `latestReviews`, `comments`, `commits`), comparing the latest review timestamp against the latest commit push timestamp.

#### Modified: ForgeClient impl (in zigzag-cli)

Implement `get_review_status()` using `gh pr view --json reviews,latestReviews,commits`. Compare `lastEditedAt` / `submittedAt` of reviews against the `committedDate` of the latest commit to determine `has_new_comments`.

#### Modified: TUI (zigzag-tui)

- New modal: `Modal::ActionMenu { actions: Vec<ResolvedAction>, selected: usize }`
- New keybinding: `x` (or configurable) → build `ActionContext` from current state → call `ActionResolver` → open `Modal::ActionMenu`
- New `TuiAction::RunAction { session, command, pane_type }` — returned when user selects an action, handled by the caller to invoke `zellij run`
- OSC 8 hyperlink rendering for "Open PR" action — write `\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\` sequence
- Preview pane enrichment: display review status (comment count, "new comments" indicator)
- Background fetch: add `get_review_status()` call to the existing refresh thread alongside PR/CI

#### Modified: Config parsing (zigzag-core / zigzag-cli)

Extend KDL config parser to handle the `actions { ... }` block in both global and per-repo configs. Same parsing pattern as existing layout/autopilot config.

#### Modified: Session Manager / main.rs (zigzag-cli)

Handle `TuiAction::RunAction` by calling `zellij run` with the appropriate flags (`--floating`, `--in-place`, or creating a new tab) in the target session.

### KDL Schema

```kdl
// In config.kdl or .config/zigzag.kdl
actions {
    // Global settings
    review-tool "codex"  // default AI tool for review actions

    action "Review PR (Codex)" {
        run "${review_tool} -q 'Review PR #${pr_number} on branch ${branch}. Focus on: 1) SECURITY: auth flaws, injection vectors, secret leaks, unsafe deserialization, OWASP top 10. 2) ARCHITECTURE: trait/interface boundaries, coupling, separation of concerns, I/O purity. 3) PATTERNS: idiomatic Rust, error handling, ownership, lifetime correctness, unnecessary allocations. Read the full diff with gh pr diff ${pr_number}. Post findings as a PR review using gh pr review ${pr_number} --comment --body <your review>. Only comment on real issues. No nitpicks, no style-only remarks.'"
        when "has_pr"
        context "session"
        pane "tab"
        icon "🔍"
    }

    action "Fix CI" {
        run "claude 'Fix the CI failure based on: $(gh run view --log-failed)'"
        when "has_ci_failure"
        context "session"
        pane "tab"
        icon "🔧"
    }

    action "Address review comments" {
        run "claude 'Address all PR review comments: $(gh pr view --json reviews -q .reviews)'"
        when "has_new_comments"
        context "session"
        pane "tab"
        icon "💬"
    }

    action "Open PR" {
        open-url "${pr_url}"
        when "has_pr"
        context "session"
        icon "🌐"
    }
}
```

### Variable Interpolation

Variables are resolved at action execution time from `ActionContext`:

| Variable | Source | Available when |
|---|---|---|
| `${project}` | Project config name | always |
| `${project_path}` | Project config path | always |
| `${branch}` | Selected session branch | context=session |
| `${session}` | Session name (`project:branch`) | context=session |
| `${repo}` | Git remote origin | always |
| `${pr_number}` | ForgeClient | has_pr |
| `${pr_url}` | ForgeClient | has_pr |
| `${ci_status}` | ForgeClient | always |
| `${review_tool}` | Global config `actions.review-tool` | always (default: codex) |

Unresolvable variables produce a clear error in the status bar: `"Cannot run action: ${pr_number} is not available (no PR found)"`.

### Condition Evaluation

| Condition | True when |
|---|---|
| `always` | Always (default if no `when`) |
| `has_pr` | `ForgeClient.get_pr()` returned `Some(pr)` |
| `has_ci_failure` | `ForgeClient.get_ci_status()` returned `CiStatus::Failed` |
| `has_new_comments` | `ForgeClient.get_review_status().has_new_comments == true` |

### OSC 8 Hyperlinks

For the "Open PR" built-in action, instead of `run`, use `open-url` which renders an OSC 8 hyperlink sequence. This is terminal-native, works over SSH, and is supported by iTerm2, Ghostty, WezTerm, Kitty, Windows Terminal, and most modern terminals. The action writes the sequence to stdout, displays the URL in the status bar as fallback, and does not spawn a pane.

### Pane Execution

Actions with `run` execute via `zellij run` in the target project session:

| Pane type | Zellij command |
|---|---|
| `float` (default) | `zellij -s {session} run --floating -- sh -c "{command}"` |
| `split` | `zellij -s {session} run -- sh -c "{command}"` |
| `tab` | `zellij -s {session} action new-tab --name "{action_name}" && zellij -s {session} run -- sh -c "{command}"` |

### Action Merge Strategy

Same-name actions are overridden (not merged) at each tier:
1. Built-in actions (hardcoded in Rust)
2. Global config `actions { ... }` — overrides built-in by name
3. Per-repo `.config/zigzag.kdl` `actions { ... }` — overrides global by name

To disable a built-in action: `action "Open PR" { disabled true }`

## Testing Decisions

### What makes a good test

Tests should verify external behavior through the module interfaces: "given these action definitions and this context, does the engine produce the correct filtered and interpolated action list?" — not "does it parse KDL node X with method Y?"

### Modules to test

#### Action Engine (priority: high)

- **KDL parsing**: valid action definitions, missing fields with defaults, malformed input
- **3-tier merge**: built-in + global + per-repo, override by name, `disabled true`
- **Condition evaluation**: each `when` condition against various `ActionContext` states
- **Variable interpolation**: all variables, missing variables produce errors, nested `$()` subshells are left untouched (they execute at runtime)
- **Action filtering**: correct actions returned for project context vs session context, conditions evaluated correctly

#### ForgeClient — ReviewStatus (priority: high)

- **Timestamp comparison**: reviews before last commit → `has_new_comments: false`, reviews after → `true`
- **Edge cases**: no reviews, no commits, PR with only bot comments
- **JSON parsing**: `gh pr view` output parsing for review data

#### TUI — ActionMenu modal (priority: low)

- Snapshot tests for rendered action menu using ratatui TestBackend
- Navigation: up/down selection, enter triggers, escape closes
- Prior art: existing modal tests in zigzag-tui

### Prior art

- `zigzag-core` config parsing tests (KDL parsing + merge)
- `zigzag-core` layout generation tests (variable interpolation pattern)
- `zigzag-tui` modal tests (TestBackend snapshots)
- `zigzag-autopilot` DSL parsing tests (KDL workflow definitions, similar structure)

## Out of Scope

- **Interactive actions** (actions that require user input beyond selection) — actions are fire-and-forget shell commands
- **Action chaining** (run action A then B) — use autopilot workflows for multi-step sequences
- **Action output capture** — the action runs in a Zellij pane, output is visible there, not piped back to the TUI
- **Custom conditions** (user-defined `when` expressions) — only the 4 built-in conditions for now
- **Action permissions / confirmation dialogs** — all actions execute immediately on selection
- **Autopilot integration** — actions are one-shot; autopilots handle recurring/reactive workflows
- **Non-GitHub forges** — `has_pr`, `has_ci_failure`, `has_new_comments` depend on `gh` CLI

## Further Notes

- The action engine is designed as a deep module: complex internals (KDL parsing, 3-tier merge, condition evaluation, interpolation) behind a simple interface (`resolve(context) → Vec<ResolvedAction>`). This makes it highly testable and keeps the TUI thin.
- The `review-tool` config variable allows swapping between `codex`, `claude`, or any other CLI tool globally. Built-in action prompts reference `${review_tool}` so changing the tool is a one-line config change.
- OSC 8 hyperlinks are the correct solution for remote/SSH scenarios. They degrade gracefully — terminals that don't support OSC 8 show the raw text, and the URL is always displayed in the status bar as fallback.
- The `has_new_comments` condition requires comparing review timestamps against push timestamps. The `gh pr view --json` API provides both. This is a single API call batched with existing PR/CI fetches — no additional latency.
- This feature is independent of Phase 4 (WASM plugin). The action engine lives in zigzag-core (I/O-agnostic), so it will be available in the WASM plugin when Phase 4 ships. Pane execution will need a different implementation in the plugin context.
